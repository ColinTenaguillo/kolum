//! The adapter against Jira Cloud's own response shapes (fixtures in
//! `tests/fixtures`), with no network: mapping on its own, then the whole
//! provider through a local HTTP server that answers with the fixtures.

use super::*;
use kolum_domain::rich_text::{Block, Inline, PanelKind};
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

macro_rules! fixture {
    ($name:literal) => {
        include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/", $name))
    };
}

fn ctx() -> Ctx {
    let fields: Vec<FieldDto> = serde_json::from_str(fixture!("fields.json")).unwrap();
    Ctx {
        base_url: "https://acme.atlassian.net".into(),
        fields: map::detect_fields(&fields, &map::FieldIds::default()),
        names: map::custom_names(&fields),
    }
}

fn issues() -> Vec<Issue> {
    let page: OffsetPage = serde_json::from_str(fixture!("board_issues.json")).unwrap();
    page.issues.iter().map(|i| map::issue(i, &ctx()).unwrap()).collect()
}

#[test]
fn custom_fields_are_found_by_schema_and_name() {
    let ids = ctx().fields;
    assert_eq!(ids.estimate.as_deref(), Some("customfield_10016"));
    assert_eq!(ids.sprint.as_deref(), Some("customfield_10020"));
    assert_eq!(ids.epic_link.as_deref(), Some("customfield_10014"));
    let fields: Vec<FieldDto> = serde_json::from_str(fixture!("fields.json")).unwrap();
    let forced = map::FieldIds { estimate: Some("customfield_99".into()), ..Default::default() };
    assert_eq!(map::detect_fields(&fields, &forced).estimate.as_deref(), Some("customfield_99"));
}

#[test]
fn an_issue_maps_every_field() {
    let issue = &issues()[0];
    assert_eq!(issue.key.as_str(), "ACME-5");
    assert_eq!(issue.id.as_str(), "10005");
    assert_eq!(issue.kind, IssueType { name: "Bug".into(), kind: IssueKind::Bug });
    assert_eq!(issue.status.category, StatusCategory::Doing);
    assert_eq!(issue.status.id.as_str(), "3");
    assert_eq!(issue.priority, Some(Priority::High));
    assert_eq!(issue.assignee.as_ref().unwrap().id.as_str(), "557058:colin");
    assert_eq!(
        issue.assignee.as_ref().unwrap().avatar_url.as_deref(),
        Some("https://avatar.invalid/colin-48.png")
    );
    assert_eq!(issue.reporter.as_ref().unwrap().display_name, "Ana Silva");
    assert_eq!(issue.labels, ["auth", "backend"]);
    let parent = issue.parent.as_ref().unwrap();
    assert_eq!((parent.key.as_str(), parent.kind), ("ACME-1", IssueKind::Epic));
    assert_eq!(issue.subtasks[0].kind, IssueKind::Subtask);
    assert_eq!(issue.links[0].relation, "blocks");
    assert_eq!(issue.links[0].issue.key.as_str(), "ACME-34");
    assert_eq!(issue.links[1].relation, "relates to");
    assert_eq!(issue.estimate, Some(Estimate::Points(3.0)));
    assert_eq!(issue.sprint.as_ref().unwrap().name, "Sprint 15", "the active sprint wins");
    assert_eq!(issue.created, time::macros::datetime!(2026-09-02 20:00 UTC));
    assert_eq!(issue.updated, time::macros::datetime!(2026-09-03 09:00 UTC), "+0200 understood");
    assert_eq!(issue.due, Some(time::macros::date!(2026 - 09 - 12)));
    assert_eq!(issue.url.as_deref(), Some("https://acme.atlassian.net/browse/ACME-5"));
    let custom: Vec<(&str, &FieldValue)> =
        issue.fields.iter().map(|f| (f.name.as_str(), &f.value)).collect();
    assert_eq!(custom.len(), 3, "the known ones and the empty one are left out: {custom:?}");
    assert!(custom.contains(&("Team", &FieldValue::Text("Platform".into()))));
    assert!(
        custom.contains(&("Platforms", &FieldValue::Options(vec!["iOS".into(), "Android".into()])))
    );
    assert!(
        matches!(custom.iter().find(|c| c.0 == "Reviewer").unwrap().1, FieldValue::User(u) if u.display_name == "Ana Silva")
    );
}

#[test]
fn a_sparse_issue_falls_back_sensibly() {
    let issue = &issues()[1];
    assert_eq!(issue.status.category, StatusCategory::Todo);
    assert_eq!(issue.priority, Some(Priority::Low), "Minor, from the older scheme");
    assert!(issue.assignee.is_none() && issue.reporter.is_none());
    assert_eq!(issue.estimate, Some(Estimate::Seconds(7200)), "no points: the time estimate");
    assert_eq!(
        issue.parent.as_ref().unwrap().key.as_str(),
        "ACME-1",
        "from the old Epic Link field"
    );
    assert!(issue.description.is_empty() && issue.sprint.is_none());
}

#[test]
fn adf_becomes_rich_text() {
    let blocks = &issues()[0].description.blocks;
    let text = issues()[0].description.plain_text();
    assert!(matches!(&blocks[0], Block::Heading { level: 2, .. }));
    assert!(
        matches!(&blocks[2], Block::List { ordered: true, items } if items[1].len() == 2),
        "nested list"
    );
    assert_eq!(
        blocks[3],
        Block::Code {
            language: Some("log".into()),
            text: "ERROR auth::session expired\nWARN retrying".into()
        }
    );
    assert!(matches!(&blocks[4], Block::Panel { kind: PanelKind::Warning, .. }));
    let Block::Paragraph(inlines) = &blocks[5] else { panic!("{:?}", blocks[5]) };
    assert_eq!(
        inlines[0],
        Inline::Mention { id: Some(UserId::new("557058:ana")), name: "Ana Silva".into() }
    );
    assert!(inlines.contains(&Inline::Emoji("👀".into())));
    assert!(inlines.contains(&Inline::Break));
    assert!(
        inlines.contains(&Inline::Link {
            text: "docs".into(),
            url: "https://docs.invalid/sso".into()
        })
    );
    assert!(text.contains("[BLOCKED]") && text.contains("2026-09-01"), "{text}");
    assert!(
        matches!(&blocks[6], Block::Table { header: true, rows } if rows.len() == 2 && rows[1][1].contains(&Inline::Break))
    );
    assert!(text.contains("☑ Reproduce") && text.contains("☐ Fix"));
    assert!(text.contains("Raw headers\nX-Trace: 7f3a"), "an expand keeps its title");
    assert!(text.contains("[attachment]") && text.contains("quoted") && text.contains("kept"));
    assert!(blocks.contains(&Block::Rule));
}

#[test]
fn the_board_configuration_gives_the_columns() {
    let summary: BoardDto = serde_json::from_str(fixture!("board.json")).unwrap();
    let config: BoardConfigDto =
        serde_json::from_str(fixture!("board_configuration.json")).unwrap();
    let board = map::board(&summary, &config);
    assert_eq!(board.kind, BoardKind::Kanban);
    assert_eq!(board.project.unwrap().key, "ACME");
    let names: Vec<&str> = board.columns.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(names, ["Backlog", "Selected for Development", "In Progress", "Done"]);
    assert_eq!(board.columns[2].statuses.len(), 2);
    assert_eq!(board.columns[2].wip_limit, Some(4));
}

#[test]
fn data_center_sprints_and_wiki_descriptions_are_read() {
    let mut dto: IssueDto = serde_json::from_str(fixture!("issue.json")).unwrap();
    dto.fields.insert(
        "customfield_10020".into(),
        serde_json::json!(["com.atlassian.greenhopper.service.sprint.Sprint@1f[id=3,rapidViewId=4,state=ACTIVE,name=Sprint 3,startDate=x]"]),
    );
    dto.fields.insert("description".into(), serde_json::json!("h2. Steps\n\nIt *fails*."));
    let issue = map::issue(&dto, &ctx()).unwrap();
    assert_eq!(issue.sprint, Some(Sprint { id: "3".into(), name: "Sprint 3".into() }));
    assert_eq!(issue.description.plain_text(), "h2. Steps\nIt *fails*.");
}

#[test]
fn an_issue_without_a_status_is_an_error_not_a_panic() {
    let mut dto: IssueDto = serde_json::from_str(fixture!("issue.json")).unwrap();
    dto.fields.remove("status");
    assert!(
        matches!(map::issue(&dto, &ctx()), Err(DomainError::Transport(m)) if m.contains("status"))
    );
}

#[test]
fn names_map_to_kinds_and_priorities() {
    let t = |name: &str| IssueTypeDto { name: name.into(), subtask: false, hierarchy_level: None };
    assert_eq!(map::kind(&t("Defect")), IssueKind::Bug);
    assert_eq!(map::kind(&t("User Story")), IssueKind::Story);
    assert_eq!(map::kind(&t("Task")), IssueKind::Task);
    assert_eq!(map::kind(&t("Epic")), IssueKind::Epic);
    assert_eq!(map::kind(&t("Spike")), IssueKind::Other);
    for (name, p) in [
        ("Blocker", Priority::Highest),
        ("Major", Priority::High),
        ("Normal", Priority::Medium),
        ("Trivial", Priority::Lowest),
    ] {
        assert_eq!(map::priority(name), Some(p));
    }
    assert_eq!(map::priority("P3"), None);
    assert_eq!(
        map::datetime("2026-09-02T20:00:00+00:00"),
        Some(time::macros::datetime!(2026-09-02 20:00 UTC))
    );
    assert_eq!(map::datetime("yesterday"), None);
}

/// A local stand-in for Jira: answers each path with a fixture and keeps
/// every request it saw. `fail_once` answers its path with a 429 first.
struct FakeJira {
    url: String,
    seen: Arc<Mutex<Vec<String>>>,
}

fn route(target: &str) -> Option<&'static str> {
    let path = target.split('?').next().unwrap_or("");
    Some(match path {
        "/rest/api/3/field" => fixture!("fields.json"),
        "/rest/agile/1.0/board" => fixture!("boards.json"),
        "/rest/agile/1.0/board/42" => fixture!("board.json"),
        "/rest/agile/1.0/board/42/configuration" => fixture!("board_configuration.json"),
        "/rest/agile/1.0/board/42/issue" => fixture!("board_issues.json"),
        "/rest/api/3/issue/ACME-5" if target.contains("expand=changelog") => {
            fixture!("issue_changelog.json")
        }
        "/rest/api/3/issue/ACME-5" => fixture!("issue.json"),
        "/rest/api/3/issue/ACME-5/comment" => fixture!("comments.json"),
        "/rest/api/3/issue/ACME-5/transitions" => fixture!("transitions.json"),
        "/rest/api/3/search/jql" => fixture!("search_jql.json"),
        "/rest/api/3/myself" => fixture!("myself.json"),
        "/rest/api/3/user/search" => fixture!("user_search.json"),
        _ => return None,
    })
}

async fn fake_jira(fail_once: Option<&'static str>) -> FakeJira {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let seen = Arc::new(Mutex::new(vec![]));
    let log = seen.clone();
    let failed = Arc::new(Mutex::new(false));
    tokio::spawn(async move {
        loop {
            let Ok((mut socket, _)) = listener.accept().await else { return };
            let (log, failed) = (log.clone(), failed.clone());
            tokio::spawn(async move {
                let mut request = vec![];
                let mut buf = [0u8; 4096];
                while !request.windows(4).any(|w| w == b"\r\n\r\n") {
                    match socket.read(&mut buf).await {
                        Ok(0) | Err(_) => return,
                        Ok(n) => request.extend_from_slice(&buf[..n]),
                    }
                }
                let request = String::from_utf8_lossy(&request).to_string();
                let target = request.split_whitespace().nth(1).unwrap_or("").to_string();
                let auth = request
                    .lines()
                    .find_map(|l| l.strip_prefix("authorization: "))
                    .unwrap_or("")
                    .to_string();
                log.lock().unwrap().push(format!("{target} [{auth}]"));
                let first_hit = fail_once.is_some_and(|p| target.starts_with(p))
                    && !std::mem::replace(&mut *failed.lock().unwrap(), true);
                let (status, extra, body) = match route(&target) {
                    _ if first_hit => {
                        ("429 Too Many Requests", "Retry-After: 0\r\n", String::new())
                    }
                    Some(body) => ("200 OK", "", body.to_string()),
                    None => {
                        ("404 Not Found", "", r#"{"errorMessages":["No such thing"]}"#.to_string())
                    }
                };
                let response = format!(
                    "HTTP/1.1 {status}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n{extra}\r\n{body}",
                    body.len()
                );
                let _ = socket.write_all(response.as_bytes()).await;
            });
        }
    });
    FakeJira { url, seen }
}

fn provider(jira: &FakeJira) -> JiraProvider {
    JiraProvider::new(JiraSettings {
        base_url: jira.url.clone(),
        flavor: JiraFlavor::Cloud,
        auth: "Basic dGVzdA==".into(),
        board: Some(42),
        fields: map::FieldIds::default(),
    })
}

#[tokio::test]
async fn the_provider_reads_a_board_end_to_end() {
    let jira = fake_jira(None).await;
    let p = provider(&jira);
    assert_eq!(p.board_id(), Some(BoardId::new("42")));
    let boards = p.boards().await.unwrap();
    assert_eq!(
        boards.iter().map(|b| b.name.as_str()).collect::<Vec<_>>(),
        ["ACME board", "Mobile sprints"]
    );
    let board = p.board(&BoardId::new("42")).await.unwrap();
    assert_eq!(board.columns.len(), 4);
    let page = p.board_issues(&BoardId::new("42"), None, None).await.unwrap();
    assert_eq!(page.items.len(), 2);
    assert_eq!(page.next, None, "two of two");
    let since = time::macros::datetime!(2026-09-01 9:00 UTC);
    p.board_issues(&BoardId::new("42"), Some(since), None).await.unwrap();

    let seen = jira.seen.lock().unwrap().clone();
    assert!(
        seen.iter().all(|r| r.ends_with("[Basic dGVzdA==]")),
        "every request authenticated: {seen:?}"
    );
    assert_eq!(
        seen.iter().filter(|r| r.starts_with("/rest/api/3/field")).count(),
        1,
        "fields read once"
    );
    let issue_request =
        seen.iter().find(|r| r.starts_with("/rest/agile/1.0/board/42/issue")).unwrap();
    assert!(
        issue_request.contains("customfield_10016") && issue_request.contains("customfield_10020"),
        "{issue_request}"
    );
    assert!(
        seen.iter().any(|r| r.contains("jql=updated+%3E%3D+%222026-09-01+09%3A00%22")),
        "{seen:?}"
    );
}

#[tokio::test]
async fn the_provider_reads_an_issue_its_comments_and_history() {
    let jira = fake_jira(None).await;
    let p = provider(&jira);
    let key = IssueKey::new("ACME-5");
    assert_eq!(p.issue(&key).await.unwrap().title, "Login times out after SSO redirect");
    let comments = p.comments(&key).await.unwrap();
    assert_eq!(comments[0].body.plain_text(), "I can reproduce with MFA on.");
    assert_eq!(comments[1].author.display_name, "Someone", "a deleted author");
    let history = p.activity(&key).await.unwrap();
    assert_eq!(history[0].changes[0].to.as_deref(), Some("In Progress"));
    assert_eq!(history.len(), 2);
    let transitions = p.transitions(&key).await.unwrap();
    assert_eq!(transitions[1].required_fields, [FieldId::new("resolution")]);
    assert_eq!(p.me().await.unwrap().display_name, "Colin Tenaguillo");
    let missing = p.issue(&IssueKey::new("ACME-404")).await;
    assert_eq!(missing, Err(DomainError::NotFound("No such thing".into())));
}

#[tokio::test]
async fn search_translates_the_query_and_resolves_names() {
    let jira = fake_jira(None).await;
    let p = provider(&jira);
    let filter = Filter {
        assignee: Some(UserSel::Named("ana".into())),
        text: Some("saml".into()),
        ..Filter::default()
    };
    let page = p.search(&Query::Filter(filter), None).await.unwrap();
    assert_eq!(page.items[0].key.as_str(), "ACME-6");
    assert_eq!(page.next, Some(PageToken("CAEaAggD".into())));
    p.search(&Query::Native("project = ACME".into()), page.next).await.unwrap();
    let seen = jira.seen.lock().unwrap().clone();
    let searches: Vec<&String> =
        seen.iter().filter(|r| r.starts_with("/rest/api/3/search/jql")).collect();
    assert!(searches[0].contains("assignee+%3D+%22557058%3Aana%22"), "{}", searches[0]);
    assert!(
        searches[1].contains("jql=project+%3D+ACME")
            && searches[1].contains("nextPageToken=CAEaAggD")
    );
}

#[tokio::test]
async fn a_rate_limit_is_waited_out() {
    let jira = fake_jira(Some("/rest/api/3/myself")).await;
    let p = provider(&jira);
    assert_eq!(p.me().await.unwrap().display_name, "Colin Tenaguillo");
    let seen = jira.seen.lock().unwrap();
    assert_eq!(seen.iter().filter(|r| r.starts_with("/rest/api/3/myself")).count(), 2);
}

#[tokio::test]
async fn writes_are_refused_up_front() {
    let jira = fake_jira(None).await;
    let p = provider(&jira);
    assert!(!p.capabilities().supports(Capability::Write));
    let key = IssueKey::new("ACME-5");
    let refused = Err(DomainError::Unsupported(Capability::Write));
    assert_eq!(p.assign(&key, None).await, refused);
    assert_eq!(p.transition(&key, &TransitionId::new("21")).await, refused);
    assert!(jira.seen.lock().unwrap().is_empty(), "nothing was sent");
}
