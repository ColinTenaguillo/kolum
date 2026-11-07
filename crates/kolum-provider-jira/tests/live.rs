//! Against a real Jira, when one is named: ignored by default, run with
//!
//! ```sh
//! KOLUM_JIRA_URL=https://you.atlassian.net KOLUM_JIRA_EMAIL=you@x KOLUM_TOKEN=... \
//! KOLUM_JIRA_BOARD=3 cargo test -p kolum-provider-jira --test live -- --ignored --nocapture
//! ```
//!
//! The writes test only changes what it then puts back (a status, the
//! assignee, the title), and runs only with KOLUM_LIVE_WRITES=1.

use kolum_domain::*;
use kolum_provider_jira::JiraProvider;

fn provider() -> (JiraProvider, BoardId) {
    let var = |k: &str| std::env::var(k).unwrap_or_else(|_| panic!("{k} is not set"));
    let mut table = toml::Table::new();
    table.insert("base_url".into(), var("KOLUM_JIRA_URL").into());
    table.insert("email".into(), var("KOLUM_JIRA_EMAIL").into());
    let board = var("KOLUM_JIRA_BOARD");
    (JiraProvider::from_profile(&table).unwrap(), BoardId::new(board))
}

async fn all_issues(p: &JiraProvider, board: &BoardId) -> Vec<Issue> {
    let mut out = vec![];
    let mut token = None;
    loop {
        let page = p.board_issues(board, None, token).await.unwrap();
        out.extend(page.items);
        match page.next {
            Some(t) => token = Some(t),
            None => return out,
        }
    }
}

#[tokio::test]
#[ignore]
async fn reads_a_real_board() {
    let (p, id) = provider();
    let me = p.me().await.unwrap();
    eprintln!("me: {} ({})", me.display_name, me.id);
    let boards = p.boards().await.unwrap();
    eprintln!(
        "boards: {:?}",
        boards.iter().map(|b| (&b.id.0, &b.name, b.kind)).collect::<Vec<_>>()
    );
    let board = p.board(&id).await.unwrap();
    for c in &board.columns {
        eprintln!(
            "column {:<24} statuses {:?} wip {:?}",
            c.name,
            c.statuses.iter().map(|s| &s.0).collect::<Vec<_>>(),
            c.wip_limit
        );
    }
    let issues = all_issues(&p, &id).await;
    eprintln!("{} issues", issues.len());
    for i in &issues {
        let column = board
            .column_of(&i.status.id)
            .map(|c| board.columns[c].name.as_str())
            .unwrap_or("(none)");
        eprintln!(
            "  {:<8} {:<10} {:<12} {:<14} {:?} {:<30} parent {:?} est {:?} sprint {:?} labels {:?} files {}",
            i.key.as_str(),
            i.kind.name,
            i.status.name,
            column,
            i.priority,
            i.title.chars().take(30).collect::<String>(),
            i.parent.as_ref().map(|p| p.key.as_str()),
            i.estimate,
            i.sprint.as_ref().map(|s| &s.name),
            i.labels,
            i.attachments.len(),
        );
    }
    let Some(first) = issues.first() else { return };
    let issue = p.issue(&first.key).await.unwrap();
    eprintln!(
        "detail {}: {} custom fields, description {} blocks",
        issue.key,
        issue.fields.len(),
        issue.description.as_ref().map_or(0, |d| d.blocks.len())
    );
    eprintln!(
        "description: {:?}",
        issue.description.unwrap_or_default().plain_text().chars().take(200).collect::<String>()
    );
    let comments = p.comments(&first.key).await.unwrap();
    let activity = p.activity(&first.key).await.unwrap();
    let transitions = p.transitions(&first.key).await.unwrap();
    eprintln!("{} comments, {} history entries", comments.len(), activity.len());
    eprintln!(
        "transitions: {:?}",
        transitions
            .iter()
            .map(|t| (&t.id.0, &t.name, &t.to.name, &t.required_fields))
            .collect::<Vec<_>>()
    );
    let text = Query::Filter(Filter {
        text: Some(first.title.split_whitespace().next().unwrap_or("a").into()),
        ..Default::default()
    });
    eprintln!("search by text: {} hits", p.search(&text, None).await.unwrap().items.len());
    let mine = Query::Filter(Filter { assignees: vec![UserSel::Me], ..Default::default() });
    eprintln!("search @me: {} hits", p.search(&mine, None).await.unwrap().items.len());
    let project = first.key.project().unwrap();
    let native = p
        .search(&Query::Native(format!("project = {project} ORDER BY created DESC")), None)
        .await
        .unwrap();
    eprintln!("native JQL: {} hits, more: {}", native.items.len(), native.next.is_some());
    eprintln!("users matching my name: {}", p.find_users(&me.display_name).await.unwrap().len());
    if let Some(url) = me.avatar_url {
        eprintln!("my avatar: {} bytes", p.image(&url).await.map(|b| b.len()).unwrap_or(0));
    }
}

#[tokio::test]
#[ignore]
async fn writes_and_puts_back() {
    if std::env::var("KOLUM_LIVE_WRITES").as_deref() != Ok("1") {
        eprintln!("KOLUM_LIVE_WRITES=1 is not set: nothing written");
        return;
    }
    let (p, id) = provider();
    let me = p.me().await.unwrap();
    let issue = all_issues(&p, &id).await.into_iter().next().expect("an issue to write to");
    let key = issue.key.clone();
    eprintln!(
        "writing to {key}: {} / {:?} / {:?}",
        issue.status.name,
        issue.assignee.as_ref().map(|a| &a.display_name),
        issue.title
    );

    // A status, and back.
    let away = p
        .transitions(&key)
        .await
        .unwrap()
        .into_iter()
        .find(|t| t.to.id != issue.status.id && t.required_fields.is_empty());
    if let Some(away) = away {
        p.transition(&key, &away.id).await.unwrap();
        assert_eq!(p.issue(&key).await.unwrap().status.id, away.to.id);
        let back = p
            .transitions(&key)
            .await
            .unwrap()
            .into_iter()
            .find(|t| t.to.id == issue.status.id)
            .expect("a way back");
        p.transition(&key, &back.id).await.unwrap();
        assert_eq!(p.issue(&key).await.unwrap().status.id, issue.status.id);
        eprintln!("status: {} -> {} -> {}", issue.status.name, away.to.name, issue.status.name);
    }

    // The assignee, and back.
    p.assign(&key, Some(&me.id)).await.unwrap();
    assert_eq!(p.issue(&key).await.unwrap().assignee.map(|a| a.id), Some(me.id.clone()));
    p.assign(&key, issue.assignee.as_ref().map(|a| &a.id)).await.unwrap();
    assert_eq!(
        p.issue(&key).await.unwrap().assignee.map(|a| a.id),
        issue.assignee.as_ref().map(|a| a.id.clone())
    );
    eprintln!("assignee: round trip done");

    // A place in the board's order, and back. Jira's reads show a new rank
    // a few seconds after it took it: the order is waited for.
    let order = |issues: Vec<Issue>| issues.into_iter().map(|i| i.key).collect::<Vec<_>>();
    let before = order(all_issues(&p, &id).await);
    if let [first, second, ..] = &before[..] {
        let mut expected = before.clone();
        expected.swap(0, 1);
        p.rank(first, &Rank::After(second.clone())).await.unwrap();
        assert!(order_becomes(&p, &id, &expected).await, "{first} after {second}");
        p.rank(first, &Rank::Before(second.clone())).await.unwrap();
        assert!(order_becomes(&p, &id, &before).await, "and back");
        eprintln!("rank: {first} after {second}, and back");
    }

    // The title, and back.
    let probe = format!("{} (kolum test)", issue.title);
    p.update(&key, IssueUpdate { title: Some(probe.clone()), ..Default::default() }).await.unwrap();
    assert_eq!(p.issue(&key).await.unwrap().title, probe);
    p.update(&key, IssueUpdate { title: Some(issue.title.clone()), ..Default::default() })
        .await
        .unwrap();
    assert_eq!(p.issue(&key).await.unwrap().title, issue.title);
    eprintln!("title: round trip done");
}

/// Whether the board's order becomes `expected` within ten seconds.
async fn order_becomes(p: &JiraProvider, board: &BoardId, expected: &[IssueKey]) -> bool {
    for _ in 0..20 {
        let now: Vec<IssueKey> = all_issues(p, board).await.into_iter().map(|i| i.key).collect();
        if now == expected {
            return true;
        }
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    }
    false
}
