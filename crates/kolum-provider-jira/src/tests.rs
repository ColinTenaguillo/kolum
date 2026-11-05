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
        schemas: map::schemas(&fields),
    }
}

fn issues() -> Vec<Issue> {
    let page: OffsetPage = serde_json::from_str(fixture!("board_issues.json")).unwrap();
    page.issues.iter().map(|i| map::issue(i, &ctx()).unwrap()).collect()
}

/// Each field type takes its value in its own shape; `None` empties it.
#[test]
fn a_value_is_written_in_the_shape_its_field_takes() {
    use serde_json::json;
    let schema = |kind: &str, items: Option<&str>, custom: Option<&str>| dto::FieldSchemaDto {
        kind: kind.into(),
        items: items.map(String::from),
        custom: custom.map(String::from),
    };
    let rich = |t: &RichText| json!({ "adf": t.plain_text() });
    let ana = User { id: UserId::new("557058:ana"), display_name: "Ana".into(), avatar_url: None };
    let words = |w: &[&str]| FieldValue::Options(w.iter().map(|s| s.to_string()).collect());
    let date = time::macros::date!(2026 - 11 - 30);
    let cases = [
        (
            schema("option", None, None),
            Some(FieldValue::Text("Mobile".into())),
            json!({"value": "Mobile"}),
        ),
        (
            schema("array", Some("option"), None),
            Some(words(&["a", "b"])),
            json!([{"value": "a"}, {"value": "b"}]),
        ),
        (schema("array", Some("string"), None), Some(words(&["x"])), json!(["x"])),
        (
            schema("array", Some("version"), None),
            Some(words(&["3.1.0"])),
            json!([{"name": "3.1.0"}]),
        ),
        (schema("array", Some("component"), None), Some(words(&["api"])), json!([{"name": "api"}])),
        (
            schema("priority", None, None),
            Some(FieldValue::Text("High".into())),
            json!({"name": "High"}),
        ),
        (
            schema("user", None, None),
            Some(FieldValue::User(ana)),
            json!({"accountId": "557058:ana"}),
        ),
        (schema("number", None, None), Some(FieldValue::Number(3.5)), json!(3.5)),
        (schema("date", None, None), Some(FieldValue::Date(date)), json!("2026-11-30")),
        (
            schema("string", None, None),
            Some(FieldValue::RichText(RichText::from_markdown("hi"))),
            json!({"adf": "hi"}),
        ),
        (schema("string", None, None), Some(FieldValue::Text("x".into())), json!("x")),
        (
            schema("issuelink", None, None),
            Some(FieldValue::Text("IC-1".into())),
            json!({"key": "IC-1"}),
        ),
        (
            schema("any", None, Some("com.pyxis.greenhopper.jira:gh-epic-link")),
            Some(FieldValue::Text("IC-1".into())),
            json!("IC-1"),
        ),
        (
            schema("array", Some("json"), Some("com.pyxis.greenhopper.jira:gh-sprint")),
            Some(FieldValue::Number(42.0)),
            json!(42),
        ),
        (schema("array", Some("option"), None), None, json!([])),
        (schema("option", None, None), None, json!(null)),
    ];
    for (schema, value, wanted) in cases {
        let got = map::field_json(&schema, value.as_ref(), "accountId", &rich);
        assert_eq!(got, wanted, "{schema:?} {value:?}");
    }
}

/// What Jira's editmeta lets kolum set, by how each field is edited:
/// fields kolum cannot set (attachments, a date with a time) are left out.
#[test]
fn the_editable_fields_say_how_each_is_edited() {
    let meta: dto::EditMetaDto = serde_json::from_str(fixture!("editmeta.json")).unwrap();
    let fields = map::editable_fields(&meta);
    let shown: Vec<(&str, &FieldKind, bool)> =
        fields.iter().map(|f| (f.name.as_str(), &f.kind, f.required)).collect();
    let choice = |o: &[&str], many| FieldKind::Choice {
        options: o.iter().map(|s| s.to_string()).collect(),
        many,
    };
    assert_eq!(
        shown,
        [
            ("Assignee", &FieldKind::User, false),
            ("Bug report", &FieldKind::RichText, true),
            ("Deals", &choice(&["Arrow", "Cirrus"], true), false),
            ("Description", &FieldKind::RichText, false),
            ("Documentation Link", &FieldKind::Text, false),
            ("Due date", &FieldKind::Date, false),
            ("Epic Link", &FieldKind::Issue, false),
            ("Fix versions", &choice(&["3.1.0", "3.2.0"], true), false),
            ("Labels", &FieldKind::Words, false),
            ("Parent", &FieldKind::Issue, false),
            ("Priority", &choice(&["High", "Medium"], false), false),
            ("Reviewer", &FieldKind::User, false),
            ("Sprint", &FieldKind::Sprint, false),
            ("Story Points", &FieldKind::Number, false),
            ("Summary", &FieldKind::Text, true),
            ("Team", &choice(&["Platform", "Mobile"], false), false),
        ]
    );
    assert_eq!(fields[0].id, FieldId::new("assignee"));
}

/// The web's issue view, the same for every issue of a type: pinned fields
/// first, the details shown even empty, the others only once filled; the
/// body's rich text in its order.
#[test]
fn the_field_layout_is_the_web_issue_view() {
    let response: serde_json::Value = serde_json::from_str(fixture!("field_layout.json")).unwrap();
    let layout = map::field_layout(&response, &ctx().fields).unwrap();
    let ids = |v: &[FieldId]| v.iter().map(|f| f.as_str().to_string()).collect::<Vec<_>>();
    let side: Vec<String> = layout
        .side
        .iter()
        .map(|f| format!("{}{}", f.id, if f.only_filled { "?" } else { "" }))
        .collect();
    let names: Vec<&str> = layout.side.iter().map(|f| f.name.as_str()).collect();
    assert_eq!(names[..4], ["Due date", "Assignee", "Reviewer", "Labels"], "as the web names them");
    // In kolum's names where it has one, so it reads like `detail_fields`;
    // the old Epic Link says the same as Parent, once is enough. `?`: only
    // once filled, as the web hides the old Parent Link while it is empty.
    assert_eq!(
        side,
        [
            "due",
            "assignee",
            "customfield_10070",
            "labels",
            "customfield_10099?",
            "epic",
            "estimate",
            "sprint",
            "fixVersions?",
            "priority?",
            "reporter?"
        ]
    );
    assert_eq!(ids(&layout.body), ["customfield_10080", "description"]);
    let refused = serde_json::json!({"errors": [{"message": "not allowed"}], "data": null});
    let refused = map::field_layout(&refused, &ctx().fields);
    assert_eq!(refused, Err(DomainError::Transport("not allowed".into())));
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
    let versions = issue.fields.iter().find(|f| f.id.as_str() == "fixVersions").unwrap();
    let released = FieldValue::Options(vec!["3.1.0".into()]);
    assert_eq!((versions.name.as_str(), &versions.value), ("Fix versions", &released));
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
    assert_eq!(
        issue.attachments,
        [Attachment {
            name: "callback.png".into(),
            url: "https://acme.atlassian.net/rest/api/3/attachment/content/10100".into(),
            mime: "image/png".into(),
            size: 48213,
        }]
    );
    assert!(issue.attachments[0].is_image());
    let custom: Vec<(&str, &FieldValue)> =
        issue.fields.iter().map(|f| (f.name.as_str(), &f.value)).collect();
    assert_eq!(custom.len(), 4, "the known ones and the empty one are left out: {custom:?}");
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
    assert_eq!(issue.description.to_markdown(), "## Steps\n\nIt **fails**.\n");
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

fn route(method: &str, target: &str) -> Option<(&'static str, &'static str)> {
    let path = target.split('?').next().unwrap_or("");
    match (method, path) {
        ("POST", "/rest/api/3/issue/ACME-5/transitions") => return Some(("204 No Content", "")),
        ("PUT", "/rest/api/3/issue/ACME-5/assignee") => return Some(("204 No Content", "")),
        ("PUT", "/rest/api/3/issue/ACME-5") => return Some(("204 No Content", "")),
        ("PUT", "/rest/agile/1.0/issue/rank") => return Some(("204 No Content", "")),
        ("POST", "/rest/api/3/issue/ACME-5/comment") => {
            return Some(("201 Created", fixture!("comment_created.json")));
        }
        ("POST", "/gateway/api/graphql") => return Some(("200 OK", fixture!("field_layout.json"))),
        ("POST", "/rest/api/3/issue/bulkfetch") => {
            return Some(("200 OK", fixture!("bulkfetch.json")));
        }
        ("POST", "/rest/api/3/issue") => {
            return Some(("201 Created", r#"{"id":"10100","key":"ACME-5","self":"x"}"#));
        }
        ("GET", _) => {}
        _ => return None,
    }
    Some((
        "200 OK",
        match path {
            "/rest/api/3/field" => fixture!("fields.json"),
            "/_edge/tenant_info" => r#"{"cloudId": "cloud-1"}"#,
            "/rest/agile/1.0/board" => fixture!("boards.json"),
            // Board 7: the same, on a Jira whose private API does not answer.
            "/rest/agile/1.0/board/42" | "/rest/agile/1.0/board/7" => fixture!("board.json"),
            "/rest/agile/1.0/board/42/configuration" | "/rest/agile/1.0/board/7/configuration" => {
                fixture!("board_configuration.json")
            }
            "/rest/greenhopper/1.0/rapidviewconfig/editmodel.json"
                if target.contains("rapidViewId=42") =>
            {
                fixture!("board_editmodel.json")
            }
            "/rest/agile/1.0/board/42/issue" | "/rest/agile/1.0/board/7/issue" => {
                fixture!("board_issues.json")
            }
            "/rest/api/3/issue/ACME-5" if target.contains("expand=changelog") => {
                fixture!("issue_changelog.json")
            }
            "/rest/api/3/issue/ACME-5" => fixture!("issue.json"),
            "/rest/api/3/issue/ACME-5/comment" => fixture!("comments.json"),
            "/rest/api/3/issue/ACME-5/transitions" => fixture!("transitions.json"),
            "/rest/api/3/issue/ACME-5/editmeta" => fixture!("editmeta.json"),
            "/rest/agile/1.0/board/42/sprint" => fixture!("sprints.json"),
            "/rest/api/3/search/jql" if target.contains("nextPageToken") => {
                fixture!("search_jql_last.json")
            }
            "/rest/api/3/search/jql" => fixture!("search_jql.json"),
            "/rest/api/3/myself" => fixture!("myself.json"),
            "/rest/api/3/user/search" => fixture!("user_search.json"),
            _ => return None,
        },
    ))
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
                let head_end = loop {
                    if let Some(at) = request.windows(4).position(|w| w == b"\r\n\r\n") {
                        break at + 4;
                    }
                    match socket.read(&mut buf).await {
                        Ok(0) | Err(_) => return,
                        Ok(n) => request.extend_from_slice(&buf[..n]),
                    }
                };
                let head = String::from_utf8_lossy(&request[..head_end]).to_string();
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
                let body = String::from_utf8_lossy(&request[head_end..]).to_string();
                let method = head.split_whitespace().next().unwrap_or("").to_string();
                let target = head.split_whitespace().nth(1).unwrap_or("").to_string();
                let auth = head
                    .lines()
                    .find_map(|l| l.strip_prefix("authorization: "))
                    .unwrap_or("")
                    .to_string();
                let entry = if method == "GET" {
                    format!("{target} [{auth}]")
                } else {
                    format!("{method} {target}\n{body}\n[{auth}]")
                };
                log.lock().unwrap().push(entry);
                let first_hit = fail_once.is_some_and(|p| target.starts_with(p))
                    && !std::mem::replace(&mut *failed.lock().unwrap(), true);
                let (status, extra, body) = match route(&method, &target) {
                    _ if first_hit => {
                        ("429 Too Many Requests", "Retry-After: 0\r\n", String::new())
                    }
                    Some((status, body)) => (status, "", body.to_string()),
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
    let sprints = p.sprints(&BoardId::new("42")).await.unwrap();
    assert_eq!(
        sprints,
        [
            Sprint { id: "14".into(), name: "Sprint 14".into() },
            Sprint { id: "15".into(), name: "Sprint 15".into() }
        ]
    );
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
        !issue_request.contains("description"),
        "Cloud sends a board's descriptions as wiki markup: the bulk read brings them: {issue_request}"
    );
    assert!(
        seen.iter().any(|r| r.contains("%29+AND+updated+%3E%3D+%222026-09-01+09%3A00%22")),
        "{seen:?}"
    );
    assert!(seen.iter().any(|r| r.contains("/sprint?state=active%2Cfuture")), "{seen:?}");
    let sub = "%28resolution+%3D+EMPTY+OR+resolution+changed+after+-2w%29+AND+\
               %28status+not+in+%2810002%29+OR+resolved+is+EMPTY+OR+resolved+%3E%3D+startOfDay%28-1w%29%29";
    let issue_requests: Vec<_> =
        seen.iter().filter(|r| r.starts_with("/rest/agile/1.0/board/42/issue")).collect();
    assert!(
        issue_requests.len() == 2 && issue_requests.iter().all(|r| r.contains(sub)),
        "the board's sub-filter and its cutoff for completed issues on every page, as the web \
         board applies them: {issue_requests:?}"
    );
}

#[tokio::test]
async fn without_the_private_api_the_board_keeps_its_sub_filter_alone() {
    let jira = fake_jira(None).await;
    let p = provider(&jira);
    let board = p.board(&BoardId::new("7")).await.unwrap();
    assert_eq!(board.columns.len(), 4);
    p.board_issues(&BoardId::new("7"), None, None).await.unwrap();
    let seen = jira.seen.lock().unwrap().clone();
    let issues = seen.iter().find(|r| r.starts_with("/rest/agile/1.0/board/7/issue")).unwrap();
    assert!(
        issues.contains("jql=%28resolution+%3D+EMPTY") && !issues.contains("resolved"),
        "{issues}"
    );
}

/// The order, and what is still on the board: only keys, a thousand to a page.
#[tokio::test]
async fn the_board_keys_come_in_its_order_and_its_scope() {
    let jira = fake_jira(None).await;
    let p = provider(&jira);
    let keys = p.board_keys(&BoardId::new("42")).await.unwrap();
    assert_eq!(keys.iter().map(IssueKey::as_str).collect::<Vec<_>>(), ["ACME-5", "ACME-6"]);
    let seen = jira.seen.lock().unwrap().clone();
    let asked = seen.iter().find(|r| r.starts_with("/rest/agile/1.0/board/42/issue")).unwrap();
    assert!(
        asked.contains("fields=key")
            && asked.contains("maxResults=1000")
            && asked.contains("jql=%28resolution+%3D+EMPTY"),
        "{asked}"
    );
}

#[tokio::test]
async fn the_board_sub_filter_is_read_when_the_board_was_not() {
    let jira = fake_jira(None).await;
    provider(&jira).board_issues(&BoardId::new("42"), None, None).await.unwrap();
    let seen = jira.seen.lock().unwrap().clone();
    let issues = seen.iter().find(|r| r.starts_with("/rest/agile/1.0/board/42/issue")).unwrap();
    assert!(issues.contains("jql=%28resolution+%3D+EMPTY"), "{issues}");
    assert!(issues.contains("resolved+%3E%3D+startOfDay%28-1w%29"), "{issues}");
}

#[tokio::test]
async fn issues_are_read_in_full_with_their_comments_and_history_in_one_request() {
    let jira = fake_jira(None).await;
    let p = provider(&jira);
    assert!(p.capabilities().supports(Capability::BulkRead));
    let read = p.issues_in_full(&[IssueKey::new("ACME-5")]).await.unwrap();
    assert_eq!(read.len(), 1);
    assert_eq!(read[0].issue.title, "Login times out after SSO redirect");
    assert_eq!(read[0].comments[0].body.plain_text(), "I can reproduce with MFA on.");
    assert_eq!(read[0].comments.len(), 2, "one comment of an odd shape leaves the others");
    assert_eq!(read[0].activity[0].changes[0].to.as_deref(), Some("In Progress"));
    let seen = jira.seen.lock().unwrap().clone();
    let asked = seen.iter().find(|r| r.contains("bulkfetch")).unwrap();
    assert!(
        asked.contains(r#""issueIdsOrKeys":["ACME-5"]"#)
            && asked.contains(r#""fields":["*all"]"#)
            && asked.contains(r#""expand":["changelog"]"#),
        "{asked}"
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
    let layout = p.field_layout(&key).await.unwrap();
    assert_eq!((layout.side[0].id.as_str(), layout.side[0].name.as_str()), ("due", "Due date"));
    let asked = jira.seen.lock().unwrap().iter().find(|r| r.contains("graphql")).cloned().unwrap();
    assert!(
        asked.contains(r#""cloudId":"cloud-1""#) && asked.contains(r#""project":"ACME""#),
        "{asked}"
    );
    let children = p.children(&key).await.unwrap();
    assert_eq!(children.iter().map(|c| c.key.as_str()).collect::<Vec<_>>(), ["ACME-6"]);
    let asked = jira.seen.lock().unwrap().iter().rev().find(|r| r.contains("search/jql")).cloned();
    assert!(asked.unwrap().contains("jql=parent+%3D+%22ACME-5%22"), "by parent");
    let editable = p.editable_fields(&key).await.unwrap();
    assert!(editable.iter().any(|f| f.name == "Team"), "{editable:?}");
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
        assignees: vec![UserSel::Named("ana".into())],
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

/// `text ~` never matches a key: the issue is looked up by its key and
/// comes first, ahead of what the text search found.
#[tokio::test]
async fn a_search_for_an_issue_key_finds_that_issue() {
    let jira = fake_jira(None).await;
    let p = provider(&jira);
    let by_key =
        |text: &str| Query::Filter(Filter { text: Some(text.into()), ..Filter::default() });
    let page = p.search(&by_key("acme-5"), None).await.unwrap();
    let keys: Vec<&str> = page.items.iter().map(|i| i.key.as_str()).collect();
    assert_eq!(keys, ["ACME-5", "ACME-6"]);

    let missing = p.search(&by_key("ACME-404"), None).await.unwrap();
    let keys: Vec<&str> = missing.items.iter().map(|i| i.key.as_str()).collect();
    assert_eq!(keys, ["ACME-6"], "a key that is not there is only text");
}

#[tokio::test]
async fn a_rate_limit_is_waited_out() {
    let jira = fake_jira(Some("/rest/api/3/myself")).await;
    let p = provider(&jira);
    assert_eq!(p.me().await.unwrap().display_name, "Colin Tenaguillo");
    let seen = jira.seen.lock().unwrap();
    assert_eq!(seen.iter().filter(|r| r.starts_with("/rest/api/3/myself")).count(), 2);
}

fn sent(jira: &FakeJira, method: &str) -> Vec<String> {
    jira.seen
        .lock()
        .unwrap()
        .iter()
        .filter(|r| r.starts_with(&format!("{method} ")))
        .cloned()
        .collect()
}

/// A logged write: its request line, and its body as JSON.
fn parts(entry: &str) -> (&str, serde_json::Value) {
    let mut lines = entry.lines();
    let line = lines.next().unwrap();
    (line, serde_json::from_str(lines.next().unwrap()).unwrap())
}

#[tokio::test]
async fn writes_send_what_jira_expects() {
    let jira = fake_jira(None).await;
    let p = provider(&jira);
    assert!(p.capabilities().supports(Capability::Write));
    let key = IssueKey::new("ACME-5");
    p.transition(&key, &TransitionId::new("21")).await.unwrap();
    p.assign(&key, Some(&UserId::new("557058:ana"))).await.unwrap();
    p.assign(&key, None).await.unwrap();
    let posted = p.comment(&key, RichText::from_markdown("Looks **good**")).await.unwrap();
    assert_eq!(posted.body.plain_text(), "Looks good");
    let update = IssueUpdate {
        title: Some("New title".into()),
        priority: Some(Priority::High),
        labels: Some(vec!["a".into()]),
        description: None,
        fields: vec![
            (FieldId::new("customfield_10050"), Some(FieldValue::Text("Mobile".into()))),
            (FieldId::new("customfield_10060"), None),
        ],
    };
    p.update(&key, update).await.unwrap();
    p.update(&key, IssueUpdate::default()).await.unwrap();
    let unknown =
        IssueUpdate { fields: vec![(FieldId::new("customfield_9"), None)], ..Default::default() };
    assert_eq!(
        p.update(&key, unknown).await,
        Err(DomainError::Transport("Jira lists no field customfield_9".into()))
    );

    use serde_json::json;
    let posts = sent(&jira, "POST");
    assert!(posts.iter().all(|p| p.ends_with("[Basic dGVzdA==]")));
    assert_eq!(
        parts(&posts[0]),
        ("POST /rest/api/3/issue/ACME-5/transitions", json!({"transition": {"id": "21"}}))
    );
    let comment = adf::from_rich_text(&RichText::from_markdown("Looks **good**"));
    assert_eq!(
        parts(&posts[1]),
        ("POST /rest/api/3/issue/ACME-5/comment", json!({ "body": comment }))
    );
    let puts = sent(&jira, "PUT");
    assert_eq!(
        parts(&puts[0]),
        ("PUT /rest/api/3/issue/ACME-5/assignee", json!({"accountId": "557058:ana"}))
    );
    assert_eq!(
        parts(&puts[1]),
        ("PUT /rest/api/3/issue/ACME-5/assignee", json!({"accountId": null}))
    );
    let fields = json!({"fields": {
        "summary": "New title",
        "priority": {"name": "High"},
        "labels": ["a"],
        "customfield_10050": {"value": "Mobile"},
        "customfield_10060": [],
    }});
    assert_eq!(parts(&puts[2]), ("PUT /rest/api/3/issue/ACME-5", fields));
    assert_eq!(puts.len(), 3, "an empty update sends nothing");
}

#[tokio::test]
async fn creating_and_cloning_read_the_new_issue_back() {
    let jira = fake_jira(None).await;
    let p = provider(&jira);
    let clone = p.clone_issue(&IssueKey::new("ACME-5")).await.unwrap();
    assert_eq!(clone.key.as_str(), "ACME-5", "read back by the key Jira gave");
    let posts = sent(&jira, "POST");
    let (line, body) = parts(&posts[0]);
    assert_eq!(line, "POST /rest/api/3/issue");
    let fields = &body["fields"];
    assert_eq!(fields["project"]["key"], "ACME");
    assert_eq!(fields["issuetype"]["name"], "Bug");
    assert_eq!(fields["summary"], "CLONE - Login times out after SSO redirect");
    assert_eq!(fields["parent"]["key"], "ACME-1");
    assert_eq!(fields["description"]["type"], "doc");
}

#[tokio::test]
async fn a_refused_write_is_a_typed_error() {
    let jira = fake_jira(None).await;
    let p = provider(&jira);
    let refused = p.transition(&IssueKey::new("ACME-404"), &TransitionId::new("21")).await;
    assert_eq!(refused, Err(DomainError::NotFound("No such thing".into())));
}

#[tokio::test]
async fn avatars_are_fetched_with_credentials_only_from_jira_itself() {
    let jira = fake_jira(None).await;
    let elsewhere = fake_jira(None).await;
    let p = provider(&jira);
    p.image(&format!("{}/rest/api/3/myself", jira.url)).await.unwrap();
    p.image(&format!("{}/rest/api/3/myself", elsewhere.url)).await.unwrap();
    assert!(jira.seen.lock().unwrap()[0].ends_with("[Basic dGVzdA==]"));
    assert!(elsewhere.seen.lock().unwrap()[0].ends_with("[]"), "no credentials to another site");
    assert!(matches!(p.image(&format!("{}/nope", jira.url)).await, Err(DomainError::NotFound(_))));
}

#[tokio::test]
async fn ranking_puts_the_issue_before_or_after_another() {
    use serde_json::json;
    let jira = fake_jira(None).await;
    let p = provider(&jira);
    p.rank(&IssueKey::new("ACME-5"), &Rank::After(IssueKey::new("ACME-6"))).await.unwrap();
    p.rank(&IssueKey::new("ACME-5"), &Rank::Before(IssueKey::new("ACME-6"))).await.unwrap();
    let puts = sent(&jira, "PUT");
    assert_eq!(
        parts(&puts[0]),
        (
            "PUT /rest/agile/1.0/issue/rank",
            json!({"issues": ["ACME-5"], "rankAfterIssue": "ACME-6"})
        )
    );
    assert_eq!(
        parts(&puts[1]),
        (
            "PUT /rest/agile/1.0/issue/rank",
            json!({"issues": ["ACME-5"], "rankBeforeIssue": "ACME-6"})
        )
    );
}

#[test]
fn a_207_with_a_refused_issue_is_a_failure() {
    use serde_json::json;
    let refused = json!({"entries": [{"issueId": 10005, "issueKey": "ACME-5", "status": 400, "errors": ["Rank is not configured"]}]});
    assert_eq!(rank_errors(&refused), Some("Rank is not configured".into()));
    let fine = json!({"entries": [{"issueId": 10005, "issueKey": "ACME-5", "status": 200}]});
    assert_eq!(rank_errors(&fine), None);
    assert_eq!(rank_errors(&serde_json::Value::Null), None, "a 204");
}
