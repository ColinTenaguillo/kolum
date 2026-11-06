//! The adapter against GitHub's GraphQL shapes (fixtures in
//! `tests/fixtures`), with no network: a local stand-in answers each
//! query by what it asks for.

use super::*;
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

macro_rules! fixture {
    ($name:literal) => {
        include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/", $name))
    };
}

/// The answer to a GraphQL request, by the operation it names.
fn answer(query: &str, variables: &Value) -> &'static str {
    let has = |s: &str| query.contains(s);
    match () {
        _ if variables["id"] == "PVTI_new999" => fixture!("draft_item.json"),
        _ if has("projectV2(number") => fixture!("project.json"),
        _ if variables["after"] == "Y3Vyc29yOjM=" => fixture!("items_last.json"),
        _ if has("items(first") => fixture!("items.json"),
        _ if has("addProjectV2DraftIssue") => fixture!("add_draft.json"),
        _ if has("addComment") => fixture!("add_comment.json"),
        _ if has("mutation") => fixture!("ok.json"),
        _ if has("comments(first") => fixture!("comments.json"),
        _ if has("type: ISSUE") => fixture!("search.json"),
        _ if has("type: USER") => fixture!("users.json"),
        _ if has("viewer") => fixture!("viewer.json"),
        _ => fixture!("item.json"),
    }
}

struct FakeGithub {
    url: String,
    /// Every request's body, parsed: `query` and `variables`.
    seen: Arc<Mutex<Vec<Value>>>,
}

async fn fake_github(status: u16, body: Option<&'static str>) -> FakeGithub {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/graphql", listener.local_addr().unwrap());
    let seen = Arc::new(Mutex::new(vec![]));
    let log = seen.clone();
    tokio::spawn(async move {
        loop {
            let Ok((mut socket, _)) = listener.accept().await else { return };
            let log = log.clone();
            tokio::spawn(async move {
                let mut request = vec![];
                let mut buf = [0u8; 8192];
                let head_end = loop {
                    if let Some(at) = request.windows(4).position(|w| w == b"\r\n\r\n") {
                        break at + 4;
                    }
                    match socket.read(&mut buf).await {
                        Ok(0) | Err(_) => return,
                        Ok(n) => request.extend_from_slice(&buf[..n]),
                    }
                };
                let head = String::from_utf8_lossy(&request[..head_end]).to_lowercase();
                let length: usize = head
                    .lines()
                    .find_map(|l| l.strip_prefix("content-length: "))
                    .and_then(|n| n.trim().parse().ok())
                    .unwrap_or(0);
                while request.len() < head_end + length {
                    match socket.read(&mut buf).await {
                        Ok(0) | Err(_) => return,
                        Ok(n) => request.extend_from_slice(&buf[..n]),
                    }
                }
                let mut sent: Value =
                    serde_json::from_slice(&request[head_end..]).unwrap_or(Value::Null);
                sent["authorization"] = json!(
                    head.lines().find_map(|l| l.strip_prefix("authorization: ")).unwrap_or("")
                );
                let reply = body.unwrap_or_else(|| {
                    answer(sent["query"].as_str().unwrap_or(""), &sent["variables"])
                });
                log.lock().unwrap().push(sent);
                let response = format!(
                    "HTTP/1.1 {status} X\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{reply}",
                    reply.len()
                );
                let _ = socket.write_all(response.as_bytes()).await;
            });
        }
    });
    FakeGithub { url, seen }
}

fn provider(gh: &FakeGithub) -> GithubProvider {
    GithubProvider::new(GithubSettings {
        api_url: gh.url.clone(),
        owner: "ColinTenaguillo".into(),
        number: 12,
        token: "gho_test".into(),
        status_field: "Status".into(),
    })
}

async fn loaded(gh: &FakeGithub) -> (GithubProvider, Vec<Issue>) {
    let p = provider(gh);
    let board = p.boards().await.unwrap().remove(0).id;
    let issues = p.board_issues(&board, None, None).await.unwrap().items;
    (p, issues)
}

#[tokio::test]
async fn the_project_is_a_board_and_its_status_options_are_columns() {
    let gh = fake_github(200, None).await;
    let p = provider(&gh);
    let boards = p.boards().await.unwrap();
    assert_eq!((boards[0].id.as_str(), boards[0].name.as_str()), ("PVT_1", "kolum roadmap"));
    let board = p.board(&boards[0].id).await.unwrap();
    let names: Vec<&str> = board.columns.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(names, ["No Status", "Todo", "In Progress", "Done"]);
    assert_eq!(board.columns[2].statuses, [StatusId::new("o_prog")]);
    assert!(p.board(&BoardId::new("other")).await.is_err());
    // The project itself, which numbers no issue: its cards are numbered
    // in their repositories, even one named as its owner is.
    let project = board.project.as_ref().map(|p| p.key.as_str());
    assert_eq!(project, Some("ColinTenaguillo/12"));
    assert_eq!(boards[0].project, board.project);
    let seen = gh.seen.lock().unwrap();
    assert_eq!(seen.len(), 1, "the project is read once");
    assert_eq!(seen[0]["authorization"], "bearer gho_test");
    assert_eq!(seen[0]["variables"], json!({ "owner": "ColinTenaguillo", "number": 12 }));
}

#[tokio::test]
async fn items_become_issues_with_their_project_fields() {
    let gh = fake_github(200, None).await;
    let p = provider(&gh);
    let page = p.board_issues(&BoardId::new("PVT_1"), None, None).await.unwrap();
    assert_eq!(page.next, Some(PageToken("Y3Vyc29yOjM=".into())));
    let [issue, pr, draft] = &page.items[..] else { panic!("{:?}", page.items) };
    let in_repo = (issue.key.as_str(), issue.key.project());
    assert_eq!(in_repo, ("kolum#42", Some("kolum")), "its repository");
    assert_eq!(issue.id.as_str(), "PVTI_aaa111", "the project item, which the fields belong to");
    assert_eq!(
        (issue.status.name.as_str(), issue.status.category),
        ("In Progress", StatusCategory::Doing)
    );
    assert_eq!(issue.kind.kind, IssueKind::Bug, "labelled bug");
    assert_eq!(issue.priority, Some(Priority::High));
    assert_eq!(issue.estimate, Some(Estimate::Points(3.0)));
    assert_eq!(issue.sprint.as_ref().unwrap().name, "Iteration 4");
    assert_eq!(issue.assignee.as_ref().unwrap().display_name, "Colin Tenaguillo");
    assert_eq!(issue.reporter.as_ref().unwrap().display_name, "anasilva", "no name: the login");
    let parent = &issue.parent.as_ref().unwrap().key;
    assert_eq!((parent.as_str(), parent.project()), ("kolum#10", Some("kolum")));
    assert_eq!(issue.description.plain_text(), "It takes 2 s.\non Jira\nwith 2k issues");
    assert_eq!(issue.fields[0].name, "Team");
    assert_eq!(pr.kind.name, "Pull request");
    assert_eq!((draft.key.as_str(), draft.key.project()), ("draft-ccc333", None), "no repository");
    assert_eq!((draft.status.name.as_str(), draft.kind.name.as_str()), ("No Status", "Draft"));
}

#[tokio::test]
async fn an_issue_reads_again_by_its_item_and_its_comments_by_its_content() {
    let gh = fake_github(200, None).await;
    let (p, _) = loaded(&gh).await;
    let key = IssueKey::parse("kolum#42");
    assert_eq!(p.issue(&key).await.unwrap().title, "Picker opens slowly");
    let comments = p.comments(&key).await.unwrap();
    assert_eq!(comments[0].body.plain_text(), "Seen with --release too.");
    assert_eq!(comments[1].author.display_name, "ghost", "a deleted account");
    assert!(p.comments(&IssueKey::parse("draft-ccc333")).await.unwrap().is_empty());
    assert!(p.activity(&key).await.unwrap().is_empty());
    assert!(matches!(p.issue(&IssueKey::parse("kolum#999")).await, Err(DomainError::NotFound(_))));
    let seen = gh.seen.lock().unwrap();
    assert!(seen.iter().any(|r| r["variables"]["id"] == "PVTI_aaa111"));
    assert!(seen.iter().any(|r| r["variables"]["id"] == "I_42"));
}

#[tokio::test]
async fn writes_are_the_mutations_github_expects() {
    let gh = fake_github(200, None).await;
    let (p, _) = loaded(&gh).await;
    let key = IssueKey::parse("kolum#42");
    let ts = p.transitions(&key).await.unwrap();
    assert_eq!(
        ts.iter().map(|t| t.name.as_str()).collect::<Vec<_>>(),
        ["Todo", "Done"],
        "not the current one"
    );
    p.transition(&key, &ts[1].id).await.unwrap();
    p.assign(&key, Some(&UserId::new("U_ana"))).await.unwrap();
    let posted = p.comment(&key, RichText::from_markdown("Looks **good**")).await.unwrap();
    assert_eq!(posted.body.plain_text(), "Looks good");
    p.update(&key, IssueUpdate { title: Some("Faster picker".into()), ..Default::default() })
        .await
        .unwrap();
    p.update(
        &IssueKey::parse("draft-ccc333"),
        IssueUpdate { description: Some(RichText::from_markdown("x")), ..Default::default() },
    )
    .await
    .unwrap();
    let refused = p.update(&key, IssueUpdate { labels: Some(vec![]), ..Default::default() }).await;
    assert_eq!(refused, Err(DomainError::Unsupported(Capability::CustomFields)));
    let id = FieldId::new("Notes");
    let notes = FieldEdit { id: id.clone(), provider_id: id, name: None, value: None };
    let rich = IssueUpdate { fields: vec![notes], ..Default::default() };
    let refused = p.update(&key, rich).await;
    assert_eq!(refused, Err(DomainError::Unsupported(Capability::CustomFields)), "rich fields");
    assert_eq!(
        p.assign(&IssueKey::parse("draft-ccc333"), None).await,
        Err(DomainError::Unsupported(Capability::Write))
    );

    let seen = gh.seen.lock().unwrap();
    let mutations: Vec<&Value> =
        seen.iter().filter(|r| r["query"].as_str().unwrap().starts_with("mutation")).collect();
    let vars = |i: usize| mutations[i]["variables"].clone();
    assert!(mutations[0]["query"].as_str().unwrap().contains("updateProjectV2ItemFieldValue"));
    assert_eq!(
        vars(0),
        json!({ "project": "PVT_1", "item": "PVTI_aaa111", "field": "PVTSSF_status", "option": "o_done" })
    );
    assert!(
        mutations[1]["query"].as_str().unwrap().contains("removeAssignees"),
        "the old assignee goes first"
    );
    assert_eq!(vars(1), json!({ "id": "I_42", "users": ["U_colin"] }));
    assert_eq!(vars(2), json!({ "id": "I_42", "users": ["U_ana"] }));
    assert_eq!(vars(3), json!({ "id": "I_42", "body": "Looks **good**\n" }));
    assert!(mutations[4]["query"].as_str().unwrap().contains("updateIssue"));
    assert_eq!(vars(4), json!({ "id": "I_42", "title": "Faster picker", "body": null }));
    assert!(mutations[5]["query"].as_str().unwrap().contains("updateProjectV2DraftIssue"));
}

#[tokio::test]
async fn creating_and_cloning_add_a_draft_to_the_project() {
    let gh = fake_github(200, None).await;
    let (p, _) = loaded(&gh).await;
    let clone = p.clone_issue(&IssueKey::parse("kolum#42")).await.unwrap();
    assert_eq!(clone.key.as_str(), "draft-new999");
    assert_eq!(clone.title, "CLONE - Picker opens slowly");
    let seen = gh.seen.lock().unwrap();
    let add = seen
        .iter()
        .find(|r| r["query"].as_str().unwrap().contains("addProjectV2DraftIssue"))
        .unwrap();
    assert_eq!(add["variables"]["project"], "PVT_1");
    assert_eq!(add["variables"]["title"], "CLONE - Picker opens slowly");
}

#[tokio::test]
async fn search_runs_inside_the_project_and_keeps_what_the_board_knows() {
    let gh = fake_github(200, None).await;
    let (p, _) = loaded(&gh).await;
    let mine = Query::Filter(Filter {
        assignees: vec![UserSel::Me],
        text: Some("picker".into()),
        ..Default::default()
    });
    let hits = p.search(&mine, None).await.unwrap().items;
    assert_eq!(hits[0].status.name, "In Progress", "on the board: the board's status");
    assert_eq!(
        (hits[1].key.as_str(), hits[1].status.name.as_str()),
        ("kolum#50", "Closed"),
        "not on it: open or closed"
    );
    assert_eq!(
        p.issue(&IssueKey::parse("kolum#50")).await.unwrap().title,
        "Crash on empty board",
        "remembered to open"
    );
    let seen = gh.seen.lock().unwrap();
    let search =
        seen.iter().find(|r| r["query"].as_str().unwrap().contains("type: ISSUE")).unwrap();
    assert_eq!(search["variables"]["q"], "project:ColinTenaguillo/12 picker assignee:@me");
}

#[test]
fn the_neutral_query_in_github_search_syntax() {
    let q = |f: Filter| search_query(&Query::Filter(f), "octo", 5);
    assert_eq!(q(Filter::default()), "project:octo/5");
    let f = Filter {
        assignees: vec![UserSel::Named("ana".into())],
        labels: vec!["good first issue".into()],
        types: vec!["pr".into(), "bug".into()],
        statuses: vec!["Done".into()],
        ..Default::default()
    };
    assert_eq!(
        q(f),
        "project:octo/5 assignee:ana label:\"good first issue\" is:pr label:bug is:closed"
    );
    assert_eq!(
        search_query(&Query::Native("repo:a/b is:open".into()), "octo", 5),
        "repo:a/b is:open"
    );
    let several = Filter { assignees: vec![UserSel::Me, UserSel::Nobody], ..Default::default() };
    assert_eq!(q(several), "project:octo/5 (assignee:@me OR no:assignee)");
}

#[tokio::test]
async fn me_and_users() {
    let gh = fake_github(200, None).await;
    let p = provider(&gh);
    assert_eq!(p.me().await.unwrap().id.as_str(), "U_colin");
    let users = p.find_users("ana").await.unwrap();
    assert_eq!(users.len(), 1, "an empty node is skipped");
    assert!(
        p.capabilities().supports(Capability::Write)
            && !p.capabilities().supports(Capability::Activity)
    );
}

#[tokio::test]
async fn errors_are_typed() {
    let unauthorized = fake_github(401, Some(r#"{"message":"Bad credentials"}"#)).await;
    assert_eq!(
        provider(&unauthorized).me().await,
        Err(DomainError::Unauthorized("Bad credentials".into()))
    );
    let limited = fake_github(403, Some(r#"{"message":"API rate limit exceeded"}"#)).await;
    assert_eq!(
        provider(&limited).me().await,
        Err(DomainError::RateLimited { retry_after_secs: None })
    );
    let scopes = fake_github(
        200,
        Some(r#"{"data":null,"errors":[{"type":"INSUFFICIENT_SCOPES","message":"needs read:project"}]}"#),
    )
    .await;
    assert_eq!(
        provider(&scopes).boards().await,
        Err(DomainError::Unauthorized("needs read:project".into()))
    );
    let missing = fake_github(200, Some(r#"{"data":{"user":null,"organization":null}}"#)).await;
    assert!(
        matches!(provider(&missing).boards().await, Err(DomainError::NotFound(m)) if m.contains("ColinTenaguillo/12"))
    );
    let broken = fake_github(502, Some("<html>")).await;
    assert!(
        matches!(provider(&broken).me().await, Err(DomainError::Transport(m)) if m.starts_with("HTTP 502"))
    );
}

#[tokio::test]
async fn avatars_are_fetched_without_the_token() {
    let gh = fake_github(200, Some("PNGBYTES")).await;
    let p = provider(&gh);
    assert_eq!(p.image(&gh.url).await.unwrap(), b"PNGBYTES");
    assert_eq!(gh.seen.lock().unwrap()[0]["authorization"], "", "public: no token");
    let missing = fake_github(404, Some("")).await;
    assert!(matches!(provider(&missing).image(&missing.url).await, Err(DomainError::NotFound(_))));
}

#[tokio::test]
async fn ranking_places_the_item_after_another_in_the_projects_order() {
    let gh = fake_github(200, None).await;
    let (p, _) = loaded(&gh).await; // items aaa111 (#42), bbb222 (#43), ccc333 (draft)
    let (issue, pr, draft) =
        (IssueKey::parse("kolum#42"), IssueKey::parse("kolum#43"), IssueKey::parse("draft-ccc333"));
    p.rank(&issue, &Rank::After(pr.clone())).await.unwrap();
    p.rank(&draft, &Rank::Before(pr.clone())).await.unwrap();
    p.rank(&pr, &Rank::Before(issue.clone())).await.unwrap();
    p.rank(&issue, &Rank::Before(pr.clone())).await.unwrap();
    let seen = gh.seen.lock().unwrap();
    let moves: Vec<Value> = seen
        .iter()
        .filter(|r| r["query"].as_str().unwrap().contains("updateProjectV2ItemPosition"))
        .map(|r| r["variables"].clone())
        .collect();
    assert_eq!(
        moves[0],
        json!({ "project": "PVT_1", "item": "PVTI_aaa111", "after": "PVTI_bbb222" })
    );
    assert_eq!(moves[1]["after"], "PVTI_aaa111", "before #43 is after the one before it");
    assert_eq!(moves[2]["after"], Value::Null, "before the first is first");
    assert_eq!(moves[3]["after"], Value::Null, "before #43, skipping itself: first");
}

#[tokio::test]
async fn a_sync_drops_an_item_the_project_no_longer_holds() {
    let gh = fake_github(200, None).await;
    let p = provider(&gh);
    let fresh = kolum_app::load_board(&p, None).await.unwrap();
    let mut cached = fresh.clone();
    let mut gone = cached.issues[0].clone();
    gone.key = IssueKey::parse("python-language-server#199");
    cached.issues.insert(0, gone);
    let synced = kolum_app::sync_board(&p, cached, OffsetDateTime::now_utc()).await.unwrap();
    assert_eq!(synced, fresh, "the cards and their order are the project's");
}

#[test]
fn a_parent_with_no_number_is_no_parent() {
    let content = |parent: Value| {
        json!({ "__typename": "Issue", "id": "I_1", "number": 3, "title": "t",
                "repository": { "name": "loki" }, "createdAt": "2026-01-01T00:00:00Z",
                "parent": parent })
    };
    let numbered =
        content(json!({ "number": 9, "title": "Epic", "repository": { "name": "tempo" } }));
    let issue = issue_of(&numbered, &[], None, "Status").unwrap().issue;
    assert_eq!(issue.parent.map(|p| p.key.to_string()).as_deref(), Some("tempo#9"));
    let unnumbered = content(json!({ "title": "Epic", "repository": { "name": "tempo" } }));
    assert_eq!(issue_of(&unnumbered, &[], None, "Status").unwrap().issue.parent, None);
}

#[tokio::test]
async fn a_github_issue_has_no_field_layout_to_keep() {
    let gh = fake_github(200, None).await;
    let (p, issues) = loaded(&gh).await;
    let layout = p.field_layout(&issues[0].key).await;
    assert!(matches!(layout, Err(DomainError::Unsupported(_))), "{layout:?}");
}
