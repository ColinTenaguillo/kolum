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
