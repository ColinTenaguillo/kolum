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
    let key = IssueKey::parse("ACME-5");
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
    let mine = Query::Filter(Filter { assignees: vec![UserSel::Me], ..Default::default() });
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
    let key = IssueKey::parse("ACME-9");
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
    let key = IssueKey::parse("ACME-12");
    let before = p.issue(&key).await.unwrap().updated;
    p.transitions(&key).await.unwrap();
    assert_eq!(p.issue(&key).await.unwrap().updated, before);

    let criteria = RichText::from_markdown("Exports open in Excel");
    let update = IssueUpdate {
        title: Some("New".into()),
        fields: vec![(
            FieldId::new("customfield_10030"),
            Some(FieldValue::RichText(criteria.clone())),
        )],
        ..Default::default()
    };
    p.update(&key, update).await.unwrap();
    let after = p.issue(&key).await.unwrap();
    assert_eq!(after.title, "New");
    let field = after.fields.iter().find(|f| f.id.as_str() == "customfield_10030").unwrap();
    assert_eq!(field.value, FieldValue::RichText(criteria));
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
    let key = IssueKey::parse("ACME-5");
    let current = p.issue(&key).await.unwrap().status;
    let transitions = p.transitions(&key).await.unwrap();
    assert_eq!(transitions.len(), 5, "every status but the current one");
    assert!(transitions.iter().all(|t| t.to.id != current.id));
    assert!(transitions.iter().all(|t| t.name == t.to.name));
    let unknown = p.transition(&key, &TransitionId::new("to-nowhere")).await;
    assert_eq!(unknown, Err(DomainError::NotFound("transition to-nowhere".into())));
    assert!(p.transition(&IssueKey::parse("ACME-999"), &transitions[0].id).await.is_err());
}

#[tokio::test]
async fn comments_come_after_their_issue() {
    let p = FakeProvider::new();
    for key in ["ACME-5", "ACME-13"] {
        let key = IssueKey::parse(key);
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
        parent: Some(IssueKey::parse("ACME-1")),
    };
    let issue = p.create(new).await.unwrap();
    assert_eq!((issue.key.as_str(), issue.key.project()), ("ACME-45", Some("ACME")));
    assert_eq!((issue.status.name.as_str(), issue.kind.kind), ("Backlog", IssueKind::Task));
    assert_eq!(issue.parent.unwrap().key.as_str(), "ACME-1");
    assert!(issue.assignee.is_none() && issue.labels.is_empty());
    assert_eq!(
        p.issue(&IssueKey::parse("ACME-45")).await.unwrap().title,
        "Write the release notes"
    );
}

#[tokio::test]
async fn avatars_are_generated_per_user() {
    let p = FakeProvider::new();
    let me = p.me().await.unwrap();
    let ppm = p.image(me.avatar_url.as_deref().unwrap()).await.unwrap();
    assert!(ppm.starts_with(b"P6\n16 16\n255\n"));
    assert_eq!(ppm.len(), b"P6\n16 16\n255\n".len() + 16 * 16 * 3);
    let ana = p.find_users("ana").await.unwrap().remove(0);
    assert_ne!(p.image(ana.avatar_url.as_deref().unwrap()).await.unwrap(), ppm, "each their own");
    assert!(p.image("fake://avatar/nobody").await.is_err());
    assert!(p.capabilities().supports(Capability::Images));
}

/// The demo data's shape, pinned: the board, the snapshots and the
/// screens all lean on it.
#[tokio::test]
async fn the_demo_data_follows_its_rules() {
    let p = FakeProvider::new();
    let get = |n: u32| {
        let p = &p;
        async move { p.issue(&IssueKey::parse(format!("ACME-{n}"))).await.unwrap() }
    };
    let (five, six, nine, sso) = (get(5).await, get(6).await, get(9).await, get(1).await);
    let base = time::macros::datetime!(2026-09-01 09:00 UTC);
    assert_eq!(five.created, base + time::Duration::hours(35), "7 hours per issue number");
    assert_eq!(six.created - five.created, time::Duration::hours(7));
    assert_eq!(five.updated, five.created + time::Duration::hours(15), "3 hours per n mod 9");
    assert_eq!(get(9).await.updated, get(9).await.created, "9 mod 9 is 0");

    assert_eq!(five.sprint.as_ref().unwrap().name, "Sprint 14", "in progress: in the sprint");
    assert_eq!(nine.sprint, None, "the backlog is not in the sprint");
    assert_eq!(sso.sprint, None, "nor are epics");

    assert_eq!(
        five.due,
        Some((five.created + time::Duration::days(10)).date()),
        "Highest has a due date"
    );
    assert_eq!(six.due, None);

    // As on Jira: an epic's children are found by their parent, not listed
    // as its sub-tasks.
    assert!(sso.subtasks.is_empty());
    let children = p.children(&sso.key).await.unwrap();
    let children: Vec<&str> = children.iter().map(|r| r.key.as_str()).collect();
    assert_eq!(children, ["ACME-5", "ACME-6", "ACME-7", "ACME-8", "ACME-9", "ACME-36"]);
    assert!(p.children(&five.key).await.unwrap().is_empty(), "only epics have children");

    let activity = |key: &str| {
        let p = &p;
        let key = IssueKey::parse(key);
        async move { p.activity(&key).await.unwrap() }
    };
    let backlog = activity("ACME-9").await;
    assert_eq!(backlog.len(), 1, "still in the backlog: created, never moved");
    assert_eq!(backlog[0].author.display_name, "Colin Tenaguillo", "by its reporter");
    let moved = activity("ACME-5").await;
    assert_eq!(moved.len(), 2);
    assert_eq!(moved[1].author.display_name, "Colin Tenaguillo", "moved by its assignee");
    assert_eq!(activity("ACME-26").await[0].author.display_name, "Emma Johansson");

    // `since` includes an issue updated at exactly that moment.
    let changed = p.board_issues(&board_id(), Some(five.updated), None).await.unwrap().items;
    assert!(changed.iter().any(|i| i.key.as_str() == "ACME-5"));
}

#[tokio::test]
async fn the_sso_bug_carries_a_screenshot_and_a_log() {
    let p = FakeProvider::new();
    let bug = p.issue(&IssueKey::parse("ACME-5")).await.unwrap();
    let names: Vec<&str> = bug.attachments.iter().map(|a| a.name.as_str()).collect();
    assert_eq!(names, ["session-expired.png", "callback.log"]);
    assert!(bug.attachments[0].is_image() && !bug.attachments[1].is_image());
    let shot = p.image(&bug.attachments[0].url).await.unwrap();
    assert!(shot.starts_with(b"P6\n48 27\n255\n"));
    assert_eq!(shot.len(), b"P6\n48 27\n255\n".len() + 48 * 27 * 3);
    assert!(p.image(&bug.attachments[1].url).await.is_err(), "a log is no picture");
    assert!(p.issue(&IssueKey::parse("ACME-6")).await.unwrap().attachments.is_empty());
}

#[tokio::test]
async fn ranking_moves_an_issue_next_to_another() {
    let p = FakeProvider::new();
    let order = |issues: Vec<Issue>| issues.iter().map(|i| i.key.to_string()).collect::<Vec<_>>();
    let before = order(all_board_issues(&p).await);
    assert_eq!(&before[..3], ["ACME-5", "ACME-6", "ACME-7"]);
    p.rank(&IssueKey::parse("ACME-5"), &Rank::After(IssueKey::parse("ACME-7"))).await.unwrap();
    let after = order(all_board_issues(&p).await);
    assert_eq!(&after[..3], ["ACME-6", "ACME-7", "ACME-5"]);
    p.rank(&IssueKey::parse("ACME-5"), &Rank::Before(IssueKey::parse("ACME-6"))).await.unwrap();
    assert_eq!(order(all_board_issues(&p).await), before, "and back");
    assert!(
        p.rank(&IssueKey::parse("ACME-5"), &Rank::After(IssueKey::parse("NOPE-1"))).await.is_err()
    );
    assert!(p.capabilities().supports(Capability::Rank));
}

#[tokio::test]
async fn the_fake_says_which_fields_can_be_set() {
    let fields = FakeProvider::new().editable_fields(&IssueKey::parse("ACME-10")).await.unwrap();
    let team = fields.iter().find(|f| f.name == "Team").unwrap();
    let options = vec!["Platform".to_string(), "Mobile".to_string()];
    assert_eq!(team.kind, FieldKind::Choice { options, many: false });
    assert!(
        fields.iter().any(|f| f.kind == FieldKind::RichText && f.name == "Acceptance criteria")
    );
}

#[tokio::test]
async fn the_fake_board_has_a_sprint_running_and_one_to_come() {
    let sprints = FakeProvider::new().sprints(&BoardId::new("1")).await.unwrap();
    let names: Vec<&str> = sprints.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(names, ["Sprint 14", "Sprint 15"]);
}
