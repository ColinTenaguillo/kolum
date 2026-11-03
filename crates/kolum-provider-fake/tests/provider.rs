use kolum_domain::*;
use kolum_provider_fake::FakeProvider;

fn board_id() -> BoardId {
    BoardId::new("1")
}

async fn all_board_issues(p: &FakeProvider) -> Vec<Issue> {
    let mut out = vec![];
    let mut token = None;
    loop {
        let page = p.board_issues(&board_id(), None, token).await.unwrap();
        out.extend(page.items);
        match page.next {
            Some(t) => token = Some(t),
            None => return out,
        }
    }
}

#[tokio::test]
async fn the_board_has_five_columns_and_about_forty_cards_across_pages() {
    let p = FakeProvider::new();
    let board = p.board(&board_id()).await.unwrap();
    assert_eq!(board.columns.len(), 5);
    let first = p.board_issues(&board_id(), None, None).await.unwrap();
    assert!(first.next.is_some(), "the demo board should span several pages");
    let issues = all_board_issues(&p).await;
    assert!((38..=45).contains(&issues.len()), "{} cards", issues.len());
    // Every card lands in a column: a status outside them would vanish.
    assert!(issues.iter().all(|i| board.column_of(&i.status.id).is_some()));
    assert!(issues.iter().all(|i| i.kind.kind != IssueKind::Epic));
}

#[tokio::test]
async fn a_transition_moves_the_issue_and_is_recorded() {
    let p = FakeProvider::new();
    let key = IssueKey::new("ACME-5");
    let to_done = p
        .transitions(&key)
        .await
        .unwrap()
        .into_iter()
        .find(|t| t.to.category == StatusCategory::Done)
        .unwrap();
    p.transition(&key, &to_done.id).await.unwrap();
    assert_eq!(p.issue(&key).await.unwrap().status.name, "Done");
    let last = p.activity(&key).await.unwrap().pop().unwrap();
    assert_eq!(last.changes[0].to.as_deref(), Some("Done"));
}

#[tokio::test]
async fn search_filters_by_fields_and_text_and_refuses_native_queries() {
    let p = FakeProvider::new();
    let mine = Query::Filter(Filter { assignee: Some(UserSel::Me), ..Default::default() });
    let hits = p.search(&mine, None).await.unwrap().items;
    assert!(!hits.is_empty());
    assert!(hits.iter().all(|i| i.assignee.as_ref().unwrap().display_name == "Colin Tenaguillo"));

    let text = Query::Filter(Filter { text: Some("sso redirect".into()), ..Default::default() });
    let hits = p.search(&text, None).await.unwrap().items;
    assert_eq!(hits.iter().map(|i| i.key.as_str()).collect::<Vec<_>>(), ["ACME-5"]);

    let native = p.search(&Query::Native("project = ACME".into()), None).await;
    assert_eq!(native, Err(DomainError::Unsupported(Capability::NativeQuery)));
}

#[tokio::test]
async fn writes_change_what_reads_return() {
    let p = FakeProvider::new();
    let key = IssueKey::new("ACME-9");
    p.assign(&key, Some(&UserId::new("u-ana"))).await.unwrap();
    assert_eq!(p.issue(&key).await.unwrap().assignee.unwrap().display_name, "Ana Silva");
    p.assign(&key, None).await.unwrap();
    assert!(p.issue(&key).await.unwrap().assignee.is_none());

    p.comment(&key, RichText::from_markdown("hi")).await.unwrap();
    assert_eq!(p.comments(&key).await.unwrap().len(), 1);

    let clone = p.clone_issue(&key).await.unwrap();
    assert!(clone.title.starts_with("CLONE - "));
    assert_eq!(p.issue(&clone.key).await.unwrap().status.name, "Backlog");
    assert!(p.assign(&key, Some(&UserId::new("nobody"))).await.is_err());
}

#[tokio::test]
async fn reading_does_not_touch_and_writing_stamps_updated() {
    let p = FakeProvider::new();
    let key = IssueKey::new("ACME-12");
    let before = p.issue(&key).await.unwrap().updated;
    p.transitions(&key).await.unwrap();
    assert_eq!(p.issue(&key).await.unwrap().updated, before);

    p.update(&key, IssueUpdate { title: Some("New".into()), ..Default::default() }).await.unwrap();
    let after = p.issue(&key).await.unwrap();
    assert_eq!(after.title, "New");
    assert!(after.updated > before);
    // Only what changed comes back in an incremental sync.
    // The seeded data is all from September 2026.
    let since = Some(time::macros::datetime!(2026-10-01 0:00 UTC));
    let changed = p.board_issues(&board_id(), since, None).await.unwrap().items;
    assert_eq!(changed.iter().map(|i| &i.key).collect::<Vec<_>>(), [&key]);
}

#[tokio::test]
async fn transitions_lead_away_from_the_current_status_by_its_name() {
    let p = FakeProvider::new();
    let key = IssueKey::new("ACME-5");
    let current = p.issue(&key).await.unwrap().status;
    let transitions = p.transitions(&key).await.unwrap();
    assert_eq!(transitions.len(), 5, "every status but the current one");
    assert!(transitions.iter().all(|t| t.to.id != current.id));
    assert!(transitions.iter().all(|t| t.name == t.to.name));
    let unknown = p.transition(&key, &TransitionId::new("to-nowhere")).await;
    assert_eq!(unknown, Err(DomainError::NotFound("transition to-nowhere".into())));
    assert!(p.transition(&IssueKey::new("ACME-999"), &transitions[0].id).await.is_err());
}

#[tokio::test]
async fn comments_come_after_their_issue() {
    let p = FakeProvider::new();
    for key in ["ACME-5", "ACME-13"] {
        let key = IssueKey::new(key);
        let created = p.issue(&key).await.unwrap().created;
        let comments = p.comments(&key).await.unwrap();
        assert!(!comments.is_empty());
        assert!(comments.iter().all(|c| c.created > created));
        assert!(comments.windows(2).all(|w| w[0].created < w[1].created), "in order");
    }
}

#[tokio::test]
async fn the_signed_in_user_and_user_search() {
    let p = FakeProvider::new();
    assert_eq!(p.me().await.unwrap().display_name, "Colin Tenaguillo");
    let found = p.find_users("SIL").await.unwrap();
    assert_eq!(found.iter().map(|u| u.display_name.as_str()).collect::<Vec<_>>(), ["Ana Silva"]);
    assert!(p.capabilities().supports(Capability::Write));
    assert!(!p.capabilities().supports(Capability::NativeQuery));
}

#[tokio::test]
async fn a_created_issue_starts_in_the_backlog_with_its_own_key() {
    let p = FakeProvider::new();
    let new = NewIssue {
        project: "ACME".into(),
        kind: IssueType { name: "Task".into(), kind: IssueKind::Task },
        title: "Write the release notes".into(),
        description: RichText::from_markdown("For 0.1"),
        parent: Some(IssueKey::new("ACME-1")),
    };
    let issue = p.create(new).await.unwrap();
    assert_eq!(issue.key.as_str(), "ACME-45");
    assert_eq!((issue.status.name.as_str(), issue.kind.kind), ("Backlog", IssueKind::Task));
    assert_eq!(issue.parent.unwrap().key.as_str(), "ACME-1");
    assert!(issue.assignee.is_none() && issue.labels.is_empty());
    assert_eq!(p.issue(&IssueKey::new("ACME-45")).await.unwrap().title, "Write the release notes");
}
