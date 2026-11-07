//! The screen as a user sees it: keys in, a rendered terminal out. The
//! board, the picker, the detail and the help page are pinned as
//! snapshots; the rest is checked on what the keys did.

use kolum::config::{PickerLayout, UiConfig};
use kolum::keymap::{Key, Keymap, parse_keys};
use kolum::ui::{App, EditPurpose, Effect, Settings};
use kolum_app::{Command, Event, IssueDetail, load_board};
use kolum_domain::{IssueKey, IssueReader, UserDirectory};
use kolum_provider_fake::FakeProvider;
use ratatui::{Terminal, backend::TestBackend};
use std::sync::Arc;
use std::time::Instant;

fn settings() -> Settings {
    Settings {
        keymap: Keymap::defaults(),
        theme_name: "dark".into(),
        themes_dir: None,
        theme_overrides: toml::Table::new(),
        // No sync on its own: deadlines stay the test's to decide.
        ui: UiConfig { sync_secs: 0, ..UiConfig::default() },
        picker_layout: PickerLayout::Horizontal,
        problems: vec![],
        board: None,
        writable: true,
        images: None,
        remote_debounce: std::time::Duration::from_millis(250),
        filter_applied: false,
    }
}

async fn loaded_with(settings: Settings) -> App {
    let fake = FakeProvider::new();
    let mut app = App::new(settings, Arc::new(|| {}));
    assert_eq!(app.take_effects(), [Effect::Send(Command::LoadBoard(None, 1))]);
    app.on_event(Event::Me(fake.me().await.unwrap()));
    app.on_event(Event::BoardLoaded {
        snapshot: load_board(&fake, None).await.unwrap(),
        fresh: true,
        request: 1,
    });
    app
}

async fn loaded() -> App {
    loaded_with(settings()).await
}

fn press(app: &mut App, keys: &str) {
    for key in parse_keys(keys, Key::char(' ')).unwrap() {
        app.on_key(key, Instant::now());
    }
}

/// Matching runs on nucleo's threads; give it the moment it needs.
fn settle(app: &mut App) {
    for _ in 0..20 {
        app.tick();
    }
}

fn screen(app: &mut App, width: u16, height: u16) -> String {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
    terminal.draw(|f| app.draw(f)).unwrap();
    terminal.backend().to_string()
}

fn selected(app: &App) -> &str {
    app.selected_issue().map(IssueKey::as_str).unwrap_or("")
}

#[tokio::test]
async fn the_board() {
    let mut app = loaded().await;
    insta::assert_snapshot!(screen(&mut app, 150, 40));
}

#[tokio::test]
async fn hjkl_counts_and_jumps_move_the_highlight() {
    let mut app = loaded().await;
    assert_eq!(selected(&app), "ACME-9", "the first card of the first column");
    press(&mut app, "j");
    assert_eq!(selected(&app), "ACME-16");
    press(&mut app, "ll");
    assert_eq!(selected(&app), "ACME-5", "the first card of In Progress");
    press(&mut app, "3j");
    assert_eq!(selected(&app), "ACME-13", "5, 10, 11, then 13");
    press(&mut app, "G");
    assert_eq!(selected(&app), "ACME-42");
    press(&mut app, "gg");
    assert_eq!(selected(&app), "ACME-5");
    press(&mut app, "$");
    assert_eq!(selected(&app), "ACME-8", "Done");
    press(&mut app, "0k");
    assert_eq!(selected(&app), "ACME-9", "back to the first column, at its top");
    press(&mut app, "9l");
    assert_eq!(selected(&app), "ACME-8", "a count past the last column stops there");
}

#[tokio::test]
async fn the_filter_narrows_the_board_and_n_steps_through_it() {
    let mut app = loaded().await;
    press(&mut app, "/billing<CR>");
    let shown = screen(&mut app, 150, 40);
    assert!(shown.contains("/billing"), "the header shows the filter");
    assert!(!shown.contains("ACME-9 "), "a card without the label is gone");
    let first = selected(&app).to_string();
    press(&mut app, "n");
    assert_ne!(selected(&app), first);
    press(&mut app, "N");
    assert_eq!(selected(&app), first);
    press(&mut app, "<Esc>");
    assert!(!screen(&mut app, 150, 40).contains("/billing"), "Esc clears it");
}

#[tokio::test]
async fn the_issue_picker_matches_as_you_type() {
    let mut app = loaded().await;
    press(&mut app, "<Space>flogin");
    settle(&mut app);
    insta::assert_snapshot!(screen(&mut app, 150, 40));
}

#[tokio::test]
async fn picker_prefixes_filter_before_matching() {
    let mut app = loaded().await;
    press(&mut app, "<Space>f@me #bug");
    settle(&mut app);
    let shown = screen(&mut app, 150, 40);
    assert!(shown.contains("ACME-5 Login times out"), "my bug");
    assert!(shown.contains("ACME-29 Stripe"), "my other bug");
    assert!(!shown.contains("ACME-13 Webhooks"), "mine, but a story");
    assert!(!shown.contains("ACME-10 Search"), "a bug, not mine");
}

#[tokio::test]
async fn enter_in_the_picker_opens_the_issue() {
    let mut app = loaded().await;
    press(&mut app, "<Space>fstripe");
    settle(&mut app);
    press(&mut app, "<CR>");
    assert_eq!(app.open_issue().map(IssueKey::as_str), Some("ACME-29"));
    assert_eq!(selected(&app), "ACME-29", "the board follows");
    assert_eq!(app.take_effects(), [Effect::Send(Command::LoadIssue(IssueKey::parse("ACME-29")))]);
}

#[tokio::test]
async fn the_picker_has_a_normal_mode_and_resumes() {
    let mut app = loaded().await;
    press(&mut app, "<Space>fsso");
    settle(&mut app);
    press(&mut app, "<Esc>j");
    let shown = screen(&mut app, 150, 40);
    assert!(shown.contains("NORMAL"), "Esc leaves insert mode, and j moves instead of typing");
    assert!(shown.contains("\u{f002} sso "), "{shown}");
    press(&mut app, "q");
    assert!(!screen(&mut app, 150, 40).contains("\u{f002} sso"), "q closes it in normal mode");
    press(&mut app, "<Space>'");
    assert!(screen(&mut app, 150, 40).contains("\u{f002} sso"), "<leader>' reopens it as it was");
}

#[tokio::test]
async fn the_issue_detail() {
    let mut app = loaded().await;
    press(&mut app, "ll<CR>");
    assert_eq!(app.open_issue().map(IssueKey::as_str), Some("ACME-5"));
    let fake = FakeProvider::new();
    let key = IssueKey::parse("ACME-5");
    let detail = IssueDetail {
        issue: fake.issue(&key).await.unwrap(),
        comments: fake.comments(&key).await.unwrap(),
        activity: fake.activity(&key).await.unwrap(),
        layout: None,
        children: vec![],
    };
    app.on_event(Event::IssueLoaded(std::sync::Arc::new(detail)));
    insta::assert_snapshot!(screen(&mut app, 150, 60));
    press(&mut app, "q");
    assert_eq!(app.open_issue(), None, "q closes the detail, not the app");
    assert!(!app.quit);
}

#[tokio::test]
async fn a_narrow_detail_stacks_its_columns() {
    let mut app = loaded().await;
    press(&mut app, "<CR>");
    let shown = screen(&mut app, 70, 60);
    let description = shown.find("An admin switch").unwrap();
    let status = shown.find("Status").unwrap();
    assert!(status > description, "the fields come after the content");
}

#[tokio::test]
async fn the_help_page() {
    let mut app = loaded().await;
    press(&mut app, "?");
    insta::assert_snapshot!(screen(&mut app, 120, 50));
}

/// `/` on the help page keeps the keys whose line has every word typed;
/// Esc clears the filter, then closes the page.
#[tokio::test]
async fn the_help_page_filters_as_you_type() {
    let mut app = loaded().await;
    press(&mut app, "?/open parent");
    let shown = screen(&mut app, 150, 50);
    assert!(shown.contains("Open the parent issue"), "{shown}");
    assert!(!shown.contains("Change the status"), "the rest is hidden");
    assert!(!shown.contains("[keys.picker]"), "and the sections left empty");
    press(&mut app, "<CR>");
    assert!(!screen(&mut app, 150, 50).contains("Change the status"), "kept after Enter");
    press(&mut app, "<Esc>");
    assert!(screen(&mut app, 150, 50).contains("Change the status"), "Esc clears it");
    press(&mut app, "<Esc>");
    assert!(!screen(&mut app, 150, 50).contains("[keys.board]"), "then closes the page");
    assert!(!app.quit);
}

#[tokio::test]
async fn the_help_page_follows_a_rebinding() {
    let mut settings = settings();
    let keys = toml::from_str("[board]\n\"J\" = \"move_card_right\"\n").unwrap();
    settings.keymap = Keymap::from_config(&keys).0;
    let mut app = loaded_with(settings).await;
    press(&mut app, "?");
    let shown = screen(&mut app, 120, 50);
    assert!(shown.contains("L J "), "{shown}");
}

#[tokio::test]
async fn a_prefix_shows_what_can_follow() {
    let mut app = loaded().await;
    press(&mut app, "y");
    insta::assert_snapshot!(screen(&mut app, 100, 30));
}

#[tokio::test]
async fn copying_and_opening_hand_the_work_to_the_event_loop() {
    let mut app = loaded().await;
    app.take_effects();
    press(&mut app, "yy");
    assert_eq!(app.take_effects(), [Effect::Copy("ACME-9".into())]);
    press(&mut app, "o");
    assert_eq!(
        app.take_effects(),
        [Effect::Open("https://demo.kolum.invalid/browse/ACME-9".into())]
    );
}

#[tokio::test]
async fn config_problems_are_shown_until_a_key() {
    let mut settings = settings();
    settings.problems = vec!["unknown setting `ui.thme`".into()];
    let mut app = loaded_with(settings).await;
    assert!(screen(&mut app, 100, 30).contains("unknown setting `ui.thme`"));
    press(&mut app, "j");
    assert!(!screen(&mut app, 100, 30).contains("unknown setting"));
    assert_eq!(selected(&app), "ACME-9", "the key that closed the box did nothing else");
}

#[tokio::test]
async fn the_theme_changes_live_and_a_preview_is_undone_on_close() {
    let mut app = loaded().await;
    press(&mut app, ":theme gruvbox<CR>");
    assert_eq!(app.theme(), &kolum::theme::Theme::builtin("gruvbox").unwrap());
    press(&mut app, "<Space>tkanag");
    settle(&mut app);
    assert_eq!(app.theme(), &kolum::theme::Theme::builtin("kanagawa").unwrap(), "previewed");
    press(&mut app, "<Esc><Esc>");
    assert_eq!(app.theme(), &kolum::theme::Theme::builtin("gruvbox").unwrap(), "restored");
    press(&mut app, ":theme nope<CR>");
    assert!(screen(&mut app, 100, 30).contains("no theme `nope`"));
}

#[tokio::test]
async fn unfinished_actions_say_so_instead_of_doing_nothing() {
    let mut app = loaded().await;
    press(&mut app, ":assign<CR>");
    assert!(screen(&mut app, 120, 30).contains(":assign is not available yet"));
}

#[tokio::test]
async fn q_on_the_board_quits() {
    let mut app = loaded().await;
    press(&mut app, "<Esc>");
    assert!(!app.quit, "Esc out of habit does not quit");
    press(&mut app, "q");
    assert!(app.quit);
}

#[tokio::test]
async fn every_screen_survives_a_tiny_terminal() {
    for keys in ["", "<CR>", "<Space>flogin", "?", "y", ":theme"] {
        for (w, h) in [(0, 0), (1, 1), (10, 3), (30, 8), (79, 12)] {
            let mut app = loaded().await;
            if !keys.is_empty() {
                press(&mut app, keys);
            }
            settle(&mut app);
            // Drawn, not printed: TestBackend cannot print a 0-wide buffer.
            let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
            terminal.draw(|f| app.draw(f)).unwrap();
        }
    }
}

async fn detail_of(app: &mut App, key: &str) {
    let fake = FakeProvider::new();
    let key = IssueKey::parse(key);
    let detail = IssueDetail {
        issue: fake.issue(&key).await.unwrap(),
        comments: fake.comments(&key).await.unwrap(),
        activity: fake.activity(&key).await.unwrap(),
        layout: None,
        children: vec![],
    };
    app.on_event(Event::IssueLoaded(std::sync::Arc::new(detail)));
}

#[tokio::test]
async fn the_detail_scrolls_and_switches_tabs() {
    let mut app = loaded().await;
    press(&mut app, "ll<CR>");
    detail_of(&mut app, "ACME-5").await;
    let top = screen(&mut app, 120, 20);
    assert!(top.contains("After the SSO redirect"));
    press(&mut app, "G");
    let bottom = screen(&mut app, 120, 20);
    assert!(!bottom.contains("After the SSO redirect"), "G scrolled past the description");
    assert!(bottom.contains("single-use anyway"), "and reached the last comment");
    press(&mut app, "gg");
    assert_eq!(screen(&mut app, 120, 20), top);
    press(&mut app, "3j<C-d><C-u>3k");
    assert_eq!(screen(&mut app, 120, 20), top, "down and back up again");

    press(&mut app, "<Tab>");
    let history = screen(&mut app, 120, 60);
    assert!(history.contains("Status: Backlog → In Progress"), "{history}");
    assert!(!history.contains("I can reproduce"), "History has no comments");
    press(&mut app, "<Tab>");
    let all = screen(&mut app, 120, 60);
    assert!(all.contains("Status: ∅ → Backlog") && all.contains("I can reproduce"));
    press(&mut app, "<S-Tab><S-Tab>");
    assert!(!screen(&mut app, 120, 60).contains("Status: ∅"), "back on Comments");
}

/// Rich text kept in custom fields, as Jira's bug templates do, shows
/// under the description: every field that has some, or the ones listed.
#[tokio::test]
async fn the_detail_shows_rich_custom_fields_under_the_description() {
    let open = |app: &mut App| {
        press(app, "/ACME-10<CR><CR>");
        screen(app, 140, 40)
    };
    let shown = open(&mut loaded().await);
    assert!(
        shown.contains("\u{f036} Description"),
        "an icon and a title head each section: {shown}"
    );
    assert!(shown.contains("\u{f036} Acceptance criteria"));
    assert!(shown.contains("Tokens refresh before they expire"));
    let mut app = loaded().await;
    press(&mut app, "/ACME-11<CR><CR>");
    let empty = screen(&mut app, 140, 40);
    assert!(empty.contains("ACME-11") && !empty.contains("Acceptance"), "an empty one is left out");

    let mut settings = settings();
    settings.ui.detail_sections = Some(vec![]);
    let none = open(&mut loaded_with(settings).await);
    assert!(!none.contains("Acceptance criteria"), "only the ones listed");
    let mut settings = self::settings();
    settings.ui.detail_sections = Some(vec!["customfield_10030".into()]);
    assert!(open(&mut loaded_with(settings).await).contains("Tokens refresh"), "by id");
}

/// The board's copy of an issue carries fewer fields than the detail
/// loaded (Jira sends rich text fields only for one issue): a sync must
/// not take them away from the open detail.
#[tokio::test]
async fn a_board_sync_keeps_the_fields_only_the_detail_loaded() {
    let fake = FakeProvider::new();
    let mut app = loaded().await;
    press(&mut app, "/ACME-10<CR><CR>");
    let key = IssueKey::parse("ACME-10");
    let issue = fake.issue(&key).await.unwrap();
    let detail =
        IssueDetail { issue, comments: vec![], activity: vec![], layout: None, children: vec![] };
    app.on_event(Event::IssueLoaded(std::sync::Arc::new(detail)));
    let mut snapshot = load_board(&fake, None).await.unwrap();
    for issue in &mut snapshot.issues {
        issue.fields.clear();
    }
    app.on_event(Event::BoardLoaded { snapshot, fresh: true, request: 1 });
    assert!(screen(&mut app, 140, 40).contains("Tokens refresh before they expire"));
}

/// The board does not carry every field; an issue opened again starts from
/// what its last opening read, so its right-hand column does not fill in twice.
#[tokio::test]
async fn an_issue_opened_again_starts_from_what_it_last_read() {
    let fake = FakeProvider::new();
    let mut app = loaded().await;
    let mut snapshot = load_board(&fake, None).await.unwrap();
    for issue in &mut snapshot.issues {
        issue.fields.clear();
    }
    app.on_event(Event::BoardLoaded { snapshot, fresh: true, request: 1 });
    press(&mut app, "/ACME-10<CR><CR>");
    let issue = fake.issue(&IssueKey::parse("ACME-10")).await.unwrap();
    let detail =
        IssueDetail { issue, comments: vec![], activity: vec![], layout: None, children: vec![] };
    app.on_event(Event::IssueLoaded(std::sync::Arc::new(detail)));
    press(&mut app, "q");
    press(&mut app, "<CR>");
    assert_eq!(selected(&app), "ACME-10");
    let shown = screen(&mut app, 140, 40);
    assert!(shown.contains("Tokens refresh before they expire"), "{shown}");
}

/// What an earlier run read in full is there at the first opening.
#[tokio::test]
async fn an_issue_read_on_an_earlier_run_opens_with_its_fields() {
    let fake = FakeProvider::new();
    let mut app = loaded().await;
    let mut snapshot = load_board(&fake, None).await.unwrap();
    for issue in &mut snapshot.issues {
        issue.fields.clear();
    }
    app.on_event(Event::BoardLoaded { snapshot, fresh: true, request: 1 });
    let issue = fake.issue(&IssueKey::parse("ACME-10")).await.unwrap();
    let comments = fake.comments(&issue.key).await.unwrap();
    assert!(!comments.is_empty());
    let detail = IssueDetail { issue, comments, activity: vec![], layout: None, children: vec![] };
    app.on_event(Event::ReadIssues(vec![std::sync::Arc::new(detail)]));
    press(&mut app, "/ACME-10<CR><CR>");
    let shown = screen(&mut app, 140, 60);
    assert!(shown.contains("Tokens refresh before they expire"), "{shown}");
    assert!(shown.contains("Comments (1)"), "its comments too: {shown}");
}

/// Which read of an issue wins is the service's to say: the screen shows
/// the copy it sent last.
#[tokio::test]
async fn the_screen_shows_the_read_the_service_sent_last() {
    let fake = FakeProvider::new();
    let mut app = loaded().await;
    let issue = fake.issue(&IssueKey::parse("ACME-10")).await.unwrap();
    let with = |n: usize, issue: &kolum_domain::Issue| IssueDetail {
        issue: issue.clone(),
        comments: (0..n)
            .map(|i| kolum_domain::Comment {
                id: format!("c{i}"),
                author: issue.reporter.clone().unwrap(),
                body: kolum_domain::RichText::from_markdown("hello"),
                created: issue.created,
            })
            .collect(),
        activity: vec![],
        layout: None,
        children: vec![],
    };
    let comments = |app: &mut App| {
        press(app, "/ACME-10<CR><CR>");
        let shown = screen(app, 140, 60);
        press(app, "q<Esc>");
        shown
    };
    app.on_event(Event::ReadIssues(vec![std::sync::Arc::new(with(3, &issue))]));
    assert!(comments(&mut app).contains("Comments (3)"));
    app.on_event(Event::IssueLoaded(std::sync::Arc::new(with(2, &issue))));
    assert!(comments(&mut app).contains("Comments (2)"));
    app.on_event(Event::ReadIssues(vec![std::sync::Arc::new(with(1, &issue))]));
    assert!(comments(&mut app).contains("Comments (1)"));
}

/// Jira Cloud sends a board's descriptions as wiki markup, an open issue's
/// as rich text: a sync while the issue is open keeps the rich one.
#[tokio::test]
async fn a_board_sync_keeps_the_description_the_detail_loaded() {
    let fake = FakeProvider::new();
    let mut app = loaded().await;
    press(&mut app, "/ACME-10<CR><CR>");
    let key = IssueKey::parse("ACME-10");
    let issue = fake.issue(&key).await.unwrap();
    let detail =
        IssueDetail { issue, comments: vec![], activity: vec![], layout: None, children: vec![] };
    app.on_event(Event::IssueLoaded(std::sync::Arc::new(detail)));
    let mut snapshot = load_board(&fake, None).await.unwrap();
    for issue in &mut snapshot.issues {
        issue.description = Some(kolum_domain::RichText::from_markdown("h3. Context{{raw}}"));
    }
    app.on_event(Event::BoardLoaded { snapshot, fresh: true, request: 1 });
    let shown = screen(&mut app, 140, 40);
    assert!(shown.contains("Offset pagination over a non-unique sort"), "{shown}");
    assert!(!shown.contains("h3. Context"), "{shown}");
}

/// A board copy newer than the detail's read takes its description when it
/// carries one (Data Center's boards do), emptied or not, and keeps the
/// detail's when it carries none (Jira Cloud's do not).
#[tokio::test]
async fn a_board_sync_brings_a_newer_description() {
    let fake = FakeProvider::new();
    let mut app = loaded().await;
    press(&mut app, "/ACME-10<CR><CR>");
    let key = IssueKey::parse("ACME-10");
    let issue = fake.issue(&key).await.unwrap();
    let updated = issue.updated;
    let detail =
        IssueDetail { issue, comments: vec![], activity: vec![], layout: None, children: vec![] };
    app.on_event(Event::IssueLoaded(std::sync::Arc::new(detail)));
    let sync = |minutes, description: Option<&str>| {
        let fake = &fake;
        let key = &key;
        let description = description.map(kolum_domain::RichText::from_markdown);
        async move {
            let mut snapshot = load_board(fake, None).await.unwrap();
            let issue = snapshot.issues.iter_mut().find(|i| &i.key == key).unwrap();
            issue.updated = updated + time::Duration::minutes(minutes);
            issue.description = description;
            Event::BoardLoaded { snapshot, fresh: true, request: 1 }
        }
    };
    let read = "Offset pagination over a non-unique sort";
    app.on_event(sync(1, None).await);
    assert!(screen(&mut app, 140, 40).contains(read), "a Cloud board's copy carries none");
    app.on_event(sync(2, Some("Rewritten by a colleague")).await);
    let shown = screen(&mut app, 140, 40);
    assert!(shown.contains("Rewritten by a colleague"), "{shown}");
    assert!(!shown.contains(read), "{shown}");
    app.on_event(sync(3, Some("")).await);
    let shown = screen(&mut app, 140, 40);
    assert!(!shown.contains("Rewritten by a colleague"), "emptied since: {shown}");
}

/// Unset, the right-hand column shows every field: the built-in ones, then
/// each custom field the issue fills that is not rich text.
#[tokio::test]
async fn the_detail_shows_every_field_unless_the_config_lists_some() {
    let mut app = loaded().await;
    press(&mut app, "/ACME-10<CR><CR>");
    let shown = screen(&mut app, 140, 60);
    for label in ["Status", "Reporter", "Updated", "Team"] {
        assert!(shown.contains(label), "{label}: {shown}");
    }
    assert!(shown.contains("Platform"));
    let right: String = shown.lines().map(|l| l.chars().skip(95).collect::<String>()).collect();
    assert!(!right.contains("Acceptance"), "rich text stays in the middle: {right}");
}

/// With the provider's layout, the detail shows the fields the web does,
/// in its order: beside the issue after the status, and in the body.
#[tokio::test]
async fn the_detail_follows_the_providers_layout() {
    let open = |settings| async move {
        let mut app = loaded_with(settings).await;
        press(&mut app, "/ACME-10<CR><CR>");
        let mut issue = FakeProvider::new().issue(&IssueKey::parse("ACME-10")).await.unwrap();
        issue.due = Some(time::macros::date!(2026 - 12 - 01));
        let ids = |ids: &[&str]| ids.iter().map(|id| kolum_domain::FieldId::new(*id)).collect();
        let named = |rows: &[(&str, &str)]| {
            let field = |(id, name): &(&str, &str)| kolum_domain::LayoutField {
                id: kolum_domain::FieldId::new(id.trim_end_matches('?')),
                name: name.to_string(),
                only_filled: id.ends_with('?'),
            };
            rows.iter().map(field).collect()
        };
        let layout = kolum_domain::FieldLayout {
            side: named(&[
                ("title", "Summary"),
                ("due", "Due date"),
                ("customfield_10020", "Team"),
                ("assignee", "Assignee"),
                ("customfield_10050", "Code Owner"),
                ("labels", "Labels"),
                ("epic", "Parent"),
                ("estimate", "Story Points"),
                ("customfield_10060?", "Platforms"),
                ("priority?", "Priority"),
            ]),
            body: ids(&["customfield_10030", "description"]),
        };
        let detail = IssueDetail {
            issue,
            comments: vec![],
            activity: vec![],
            layout: Some(layout),
            children: vec![],
        };
        app.on_event(Event::IssueLoaded(std::sync::Arc::new(detail)));
        screen(&mut app, 140, 60)
    };
    let shown = open(settings()).await;
    let at = |label: &str| shown.find(label).unwrap_or_else(|| panic!("{label}: {shown}"));
    let side = [
        "Status",
        "Summary",
        "Due date",
        "Team",
        "Assignee",
        "Code Owner",
        "Labels",
        "Parent",
        "Story Points",
        "Priority",
    ];
    let places: Vec<usize> = side.iter().map(|l| at(l)).collect();
    assert!(places.is_sorted(), "{side:?} in this order: {shown}");
    let lines: Vec<&str> = shown.lines().collect();
    let summary = lines.iter().position(|l| l.contains("│Summary")).unwrap();
    assert!(lines[summary + 1].contains("Search API returns"), "a pinned title shows: {shown}");
    assert!(!shown.contains("Reporter") && !shown.contains("Updated"), "only the layout's");
    assert!(!shown.contains("customfield_"), "an empty field goes by its name too: {shown}");
    assert!(!shown.contains("Platforms"), "shown only once filled, and it is empty");
    assert!(at("\u{f036} Acceptance criteria") < at("\u{f036} Description"), "the body's order");

    let mut settings = settings();
    settings.ui.detail_fields = Some(vec!["reporter".into()]);
    let configured = open(settings).await;
    assert!(configured.contains("Reporter") && !configured.contains("Due date"), "config wins");
}

/// The layout is the same for every issue of a type: an issue opens with
/// the one its type had last time, before its own fields arrive.
#[tokio::test]
async fn an_issue_opens_with_the_layout_its_type_had() {
    use kolum_domain::{FieldId, FieldLayout, LayoutField};
    let issue = FakeProvider::new().issue(&IssueKey::parse("ACME-10")).await.unwrap();
    let layout = |name: &str| FieldLayout {
        side: vec![LayoutField {
            id: FieldId::new("customfield_10020"),
            name: name.into(),
            only_filled: false,
        }],
        body: vec![FieldId::new("description")],
    };
    let mut app = loaded().await;
    let kept = std::collections::BTreeMap::from([(kolum_app::layout_key(&issue), layout("Squad"))]);
    app.on_event(Event::FieldLayouts(kept));
    press(&mut app, "/ACME-10<CR><CR>");
    let shown = screen(&mut app, 140, 40);
    assert!(shown.contains("Squad") && !shown.contains("Reporter"), "at once: {shown}");

    let detail = IssueDetail {
        issue,
        comments: vec![],
        activity: vec![],
        layout: Some(layout("Team now")),
        children: vec![],
    };
    app.on_event(Event::IssueLoaded(std::sync::Arc::new(detail)));
    press(&mut app, "q<CR>");
    assert!(screen(&mut app, 140, 40).contains("Team now"), "the latest one is kept");
}

#[tokio::test]
async fn the_detail_lists_links_and_configured_custom_fields() {
    let mut settings = settings();
    settings.ui.detail_fields =
        Some(vec!["status".into(), "customfield_10020".into(), "Story points".into()]);
    let mut app = loaded_with(settings).await;
    press(&mut app, "ll<CR>");
    press(&mut app, ":q<CR>");
    assert!(app.quit, ":q quits from anywhere");

    let mut settings = self::settings();
    settings.ui.detail_fields =
        Some(vec!["status".into(), "customfield_10020".into(), "Story points".into()]);
    let mut app = loaded_with(settings).await;
    press(&mut app, "/ACME-10<CR><CR>");
    let shown = screen(&mut app, 140, 40);
    assert!(shown.contains("is blocked by"), "{shown}");
    assert!(shown.contains("ACME-34 GraphQL endpoint"));
    assert!(shown.contains("Team") && shown.contains("Platform"), "a custom field by id");
    assert!(shown.contains("Story points") && shown.contains("None"), "a field the issue lacks");
    assert!(!shown.contains("Reporter"), "only what the config lists");
    press(&mut app, "yu");
    assert_eq!(
        app.take_effects().pop(),
        Some(Effect::Copy("https://demo.kolum.invalid/browse/ACME-10".into()))
    );
}

#[tokio::test]
async fn the_help_page_scrolls_within_its_length() {
    let mut app = loaded().await;
    press(&mut app, "?");
    let top = screen(&mut app, 120, 20);
    press(&mut app, "k");
    assert_eq!(screen(&mut app, 120, 20), top, "already at the top");
    press(&mut app, "<C-d>");
    assert_ne!(screen(&mut app, 120, 20), top);
    press(&mut app, "G");
    let bottom = screen(&mut app, 120, 20);
    press(&mut app, "j");
    assert_eq!(screen(&mut app, 120, 20), bottom, "already at the bottom");
    press(&mut app, "gg");
    assert_eq!(screen(&mut app, 120, 20), top);
    press(&mut app, "5j<C-u>q");
    assert!(!screen(&mut app, 120, 20).contains("Keys  (j/k"), "q closes it");
}

#[tokio::test]
async fn the_picker_remembers_searches() {
    let mut app = loaded().await;
    for query in ["stripe", "webhook"] {
        press(&mut app, &format!("<Space>f{query}"));
        settle(&mut app);
        press(&mut app, "<CR>q");
    }
    press(&mut app, "<Space>f<Up>");
    assert!(screen(&mut app, 150, 40).contains("\u{f002} webhook"), "the latest first");
    press(&mut app, "<Up><Up>");
    assert!(
        screen(&mut app, 150, 40).contains("\u{f002} stripe"),
        "and no further than the oldest"
    );
    press(&mut app, "<Down>");
    assert!(screen(&mut app, 150, 40).contains("\u{f002} webhook"));
    press(&mut app, "<Down>");
    assert!(screen(&mut app, 150, 40).contains("\u{f002} \u{20}"), "past the newest: empty again");
}

#[tokio::test]
async fn marks_make_an_action_apply_to_several_issues() {
    let mut app = loaded().await;
    app.take_effects();
    press(&mut app, "<Space>f#bug @me");
    settle(&mut app);
    press(&mut app, "<Tab><Tab><C-y>");
    let Some(Effect::Copy(keys)) = app.take_effects().pop() else { panic!("expected a copy") };
    assert_eq!(keys.split(' ').count(), 2, "{keys}");
}

#[tokio::test]
async fn picker_actions_reach_the_highlighted_issue() {
    let mut app = loaded().await;
    app.take_effects();
    press(&mut app, "<Space>fstripe");
    settle(&mut app);
    press(&mut app, "<C-o>");
    assert_eq!(
        app.take_effects(),
        [Effect::Open("https://demo.kolum.invalid/browse/ACME-29".into())]
    );
    press(&mut app, "<Space>fstripe");
    settle(&mut app);
    app.take_effects();
    press(&mut app, "<C-t>");
    assert_eq!(
        app.take_effects(),
        [Effect::Send(Command::Transitions(IssueKey::parse("ACME-29")))]
    );
    press(&mut app, "<Space>fstripe");
    settle(&mut app);
    press(&mut app, "<C-d><C-u><Esc>Gggkj");
    press(&mut app, "<C-Space>");
    assert!(screen(&mut app, 150, 40).contains("[remote"), "the picker went remote");
}

#[tokio::test]
async fn the_command_palette_runs_what_it_lists() {
    let mut app = loaded().await;
    press(&mut app, "<Space>pshow the keys");
    settle(&mut app);
    press(&mut app, "<CR>");
    assert!(screen(&mut app, 120, 40).contains("Keys  (/ to search"), "it ran `help`");
}

/// Every action is a `:` command too, by its name.
#[tokio::test]
async fn an_action_runs_from_the_command_line() {
    use kolum_domain::IssueReader;
    let parent =
        FakeProvider::new().issue(&IssueKey::parse("ACME-5")).await.unwrap().parent.unwrap().key;
    let mut app = loaded().await;
    press(&mut app, "ll"); // ACME-5
    app.take_effects();
    press(&mut app, ":open_p");
    assert!(screen(&mut app, 140, 40).contains("Open the parent issue"), "proposed");
    press(&mut app, "arent<CR>");
    assert!(app.take_effects().contains(&Effect::Send(Command::LoadIssue(parent))));
}

#[tokio::test]
async fn the_command_line_reports_what_it_cannot_do() {
    let mut app = loaded().await;
    press(&mut app, ":frobnicate<CR>");
    assert!(screen(&mut app, 100, 20).contains("Not a command: frobnicate"));
    press(&mut app, ":jql project = X<CR>");
    let shown = screen(&mut app, 100, 20);
    assert!(
        shown.contains("\u{f002} !project = X") && shown.contains("[remote"),
        ":jql opens a remote search: {shown}"
    );
    press(&mut app, "<Esc><Esc>");
    press(&mut app, ":theme<CR>");
    assert!(screen(&mut app, 100, 20).contains(" Themes "), ":theme alone opens the picker");
    press(&mut app, "<Esc><Esc>:<Esc>");
    app.take_effects();
    press(&mut app, ":refresh<CR>");
    assert_eq!(
        app.take_effects(),
        [Effect::Send(Command::ReloadBoard(Some(kolum_domain::BoardId::new("1")), 2))]
    );
}

#[tokio::test]
async fn a_refresh_keeps_the_highlight_on_the_same_issue() {
    let mut app = loaded().await;
    press(&mut app, "ll3j");
    let before = selected(&app).to_string();
    app.on_event(Event::BoardLoaded {
        snapshot: load_board(&FakeProvider::new(), None).await.unwrap(),
        fresh: true,
        request: 1,
    });
    assert_eq!(selected(&app), before);
    app.on_event(Event::Failed {
        what: "loading the board".into(),
        error: kolum_domain::DomainError::Transport("timed out".into()),
    });
    assert!(screen(&mut app, 120, 30).contains("loading the board failed: transport: timed out"));
}

fn saved_filters(app: &mut App) -> Vec<String> {
    app.take_effects()
        .into_iter()
        .filter_map(|e| match e {
            Effect::SaveState(state) => state.applied_filter,
            _ => None,
        })
        .collect()
}

/// The board filter is kept for the next run once applied, not while typed.
#[tokio::test]
async fn the_board_filter_is_kept_when_applied_or_cleared() {
    let mut app = loaded().await;
    app.take_effects();
    press(&mut app, "/@me");
    assert!(saved_filters(&mut app).is_empty(), "not while typed");
    press(&mut app, "<CR>");
    assert_eq!(saved_filters(&mut app), ["@me"], "Enter");
    press(&mut app, "/x<Esc>");
    assert_eq!(saved_filters(&mut app), [""], "Esc clears it");
    press(&mut app, "<Space>ubruno");
    settle(&mut app);
    press(&mut app, "<CR>");
    assert_eq!(saved_filters(&mut app), ["@BrunoCosta"], "Space u");
    press(&mut app, "?/zz<CR>");
    assert!(saved_filters(&mut app).is_empty(), "the help page's filter is not the board's");
}

/// The filter of the last run, or the config's, is on at start.
#[tokio::test]
async fn the_board_opens_with_the_kept_filter() {
    let mut settings = settings();
    settings.ui.filter = "ACME-29".into();
    let mut app = loaded_with(settings).await;
    let shown = screen(&mut app, 150, 30);
    assert!(shown.contains("/ACME-29") && shown.contains("No cards"), "{shown}");
}

/// A toggle kept for the next run keeps the config's filter as the
/// config's: changed there, it is the new one that starts the next run.
#[tokio::test]
async fn a_toggle_does_not_keep_the_configs_filter_as_applied() {
    let kept = |app: &mut App| -> Vec<Option<String>> {
        app.take_effects()
            .into_iter()
            .filter_map(|e| match e {
                Effect::SaveState(state) => Some(state.applied_filter),
                _ => None,
            })
            .collect()
    };
    let mut settings = settings();
    settings.ui.filter = "@me".into();
    let mut app = loaded_with(settings).await;
    app.take_effects();
    press(&mut app, "ze");
    assert_eq!(kept(&mut app), [None]);
    press(&mut app, "/<CR>ze");
    assert_eq!(kept(&mut app), [Some("@me".into()), Some("@me".into())], "applied, then kept");
}

/// "Refreshing…" and "Opening…" go once what they wait for has come.
#[tokio::test]
async fn a_message_of_waiting_goes_once_it_is_done() {
    let fake = FakeProvider::new();
    let mut app = loaded().await;
    press(&mut app, "r");
    assert!(screen(&mut app, 150, 30).contains("Refreshing…"));
    // The epic of ACME-9 off the board: `gp` waits for it to be read.
    let epic = IssueKey::parse("ACME-1");
    let mut snapshot = load_board(&fake, None).await.unwrap();
    snapshot.issues.retain(|i| i.key != epic);
    app.on_event(Event::BoardLoaded { snapshot: snapshot.clone(), fresh: true, request: 1 });
    assert!(!screen(&mut app, 150, 30).contains("Refreshing…"));

    assert_eq!(selected(&app), "ACME-9");
    press(&mut app, "gp");
    assert!(screen(&mut app, 150, 30).contains("Opening ACME-1…"));
    let issue = fake.issue(&epic).await.unwrap();
    let detail =
        IssueDetail { issue, comments: vec![], activity: vec![], layout: None, children: vec![] };
    app.on_event(Event::IssueLoaded(std::sync::Arc::new(detail)));
    assert!(!screen(&mut app, 150, 30).contains("Opening ACME-1…"));

    press(&mut app, "q");
    press(&mut app, "r");
    app.on_event(Event::Failed {
        what: "renaming".into(),
        error: kolum_domain::DomainError::Transport("down".into()),
    });
    app.on_event(Event::BoardLoaded { snapshot, fresh: true, request: 1 });
    assert!(screen(&mut app, 150, 30).contains("down"), "a message said since stays");
}

#[tokio::test]
async fn empty_columns_can_be_hidden() {
    let mut app = loaded().await;
    press(&mut app, "/ACME-29<CR>");
    assert!(screen(&mut app, 150, 30).contains("No cards"));
    app.take_effects();
    press(&mut app, "ze");
    let saved = kolum::config::UiState {
        hide_empty_columns: true,
        swimlanes: kolum::config::Swimlanes::None,
        applied_filter: Some("ACME-29".into()),
    };
    assert_eq!(app.take_effects(), [Effect::SaveState(saved)], "kept for the next run");
    let shown = screen(&mut app, 150, 30);
    assert!(!shown.contains("No cards") && shown.contains("IN REVIEW"), "{shown}");
    assert_eq!(selected(&app), "ACME-29");
}

#[tokio::test]
async fn half_pages_move_by_what_is_on_screen() {
    let mut app = loaded().await;
    press(&mut app, "ll");
    screen(&mut app, 150, 36); // 5 cards a column fit: a half page is 2
    press(&mut app, "<C-d>");
    assert_eq!(selected(&app), "ACME-11");
    press(&mut app, "<C-u>");
    assert_eq!(selected(&app), "ACME-5");
}

#[tokio::test]
async fn the_board_says_it_is_loading() {
    let mut app = App::new(settings(), Arc::new(|| {}));
    assert!(screen(&mut app, 80, 20).contains("Loading the board…"));
}

#[tokio::test]
async fn the_command_line_edits_like_readline() {
    let mut app = loaded().await;
    press(&mut app, ":theme gruvbx<Left><Left><Right>o<Right><CR>");
    assert_eq!(app.theme(), &kolum::theme::Theme::builtin("gruvbox").unwrap());
}

#[tokio::test]
async fn theme_files_join_the_theme_picker() {
    let dir = std::env::temp_dir().join(format!("kolum-screens-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("zebra.toml"), "base = \"light\"\naccent = \"#123456\"\n").unwrap();
    std::fs::write(dir.join("dark.toml"), "base = \"light\"\n").unwrap();
    let mut settings = settings();
    settings.themes_dir = Some(dir.clone());
    let mut app = loaded_with(settings).await;
    press(&mut app, "<Space>t");
    settle(&mut app);
    let shown = screen(&mut app, 120, 40);
    assert_eq!(
        shown.matches("dark").count(),
        1,
        "a file named like a built-in is not listed twice"
    );
    press(&mut app, "zebra");
    settle(&mut app);
    press(&mut app, "<CR>");
    assert_eq!(
        app.theme().accent,
        ratatui::style::Color::Rgb(0x12, 0x34, 0x56),
        "confirmed, not just previewed"
    );
    assert!(!screen(&mut app, 120, 40).contains(" Themes "));
    std::fs::remove_dir_all(dir).unwrap();
}

/// Runs whatever falls due by `at`: a debounced search, a key timeout.
fn wait_until(app: &mut App, at: Instant) {
    app.on_timeout(at);
}

fn searches(effects: Vec<Effect>) -> Vec<(u64, kolum_domain::Query)> {
    effects
        .into_iter()
        .filter_map(|e| match e {
            Effect::Send(Command::Search { id, query }) => Some((id, query)),
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn a_remote_search_waits_for_typing_to_pause_and_drops_stale_replies() {
    use kolum_domain::{Filter, IssueSearch, Query, UserSel};
    let mut app = loaded().await;
    app.take_effects();
    press(&mut app, "<Space>F");
    let due = app.deadline().expect("going remote searches straight away");
    wait_until(&mut app, due);
    assert_eq!(searches(app.take_effects()), [(1, Query::Filter(Filter::default()))]);
    assert!(screen(&mut app, 150, 40).contains("searching…"), "searching");

    let typed = Instant::now();
    press(&mut app, "@me");
    wait_until(&mut app, typed + std::time::Duration::from_millis(150));
    assert!(searches(app.take_effects()).is_empty(), "still typing");
    wait_until(&mut app, typed + std::time::Duration::from_millis(300));
    let mine = Query::Filter(Filter { assignees: vec![UserSel::Me], ..Filter::default() });
    assert_eq!(searches(app.take_effects()), [(2, mine.clone())]);

    let fake = FakeProvider::new();
    let everything = fake.search(&Query::Filter(Filter::default()), None).await.unwrap().items;
    app.on_event(Event::SearchResults { id: 1, result: Ok(everything) });
    settle(&mut app);
    assert!(!screen(&mut app, 150, 40).contains("ACME-9 "), "the reply to search 1 came too late");
    let found = fake.search(&mine, None).await.unwrap().items;
    app.on_event(Event::SearchResults { id: 2, result: Ok(found) });
    settle(&mut app);
    let shown = screen(&mut app, 150, 40);
    assert!(shown.contains("ACME-5 Login") && shown.contains("[remote]"), "{shown}");
    assert!(!shown.contains("ACME-9 "), "only what the provider answered");

    app.on_event(Event::SearchResults {
        id: 2,
        result: Err(kolum_domain::DomainError::Unauthorized("HTTP 401".into())),
    });
    assert!(screen(&mut app, 150, 40).contains("search failed: unauthorized: HTTP 401"));
    press(&mut app, "<C-Space>");
    assert!(screen(&mut app, 150, 40).contains("[local]"), "and back");
    assert_eq!(app.deadline(), None, "nothing pending once local");
}

/// From the first key, not once the debounce is over, and turning until
/// the provider answers.
#[tokio::test]
async fn a_remote_search_shows_it_is_searching() {
    let mut app = loaded().await;
    press(&mut app, "<Space>F");
    let due = app.deadline().unwrap();
    wait_until(&mut app, due);
    app.take_effects();
    app.on_event(Event::SearchResults { id: 1, result: Ok(vec![]) });
    assert!(!screen(&mut app, 150, 40).contains("searching"), "answered");

    press(&mut app, "login");
    let typed = screen(&mut app, 150, 40);
    assert!(typed.contains("\u{f002} login") && typed.contains("⠋ searching…"), "{typed}");
    let next_frame = app.deadline().unwrap();
    wait_until(&mut app, next_frame);
    assert!(screen(&mut app, 150, 40).contains("⠙ searching…"), "it turns");

    while searches(app.take_effects()).is_empty() {
        let due = app.deadline().unwrap();
        wait_until(&mut app, due);
    }
    app.on_event(Event::SearchResults { id: 2, result: Ok(vec![]) });
    assert!(!screen(&mut app, 150, 40).contains("searching"), "answered");
    assert_eq!(app.deadline(), None, "and still once idle");
}

#[tokio::test]
async fn a_remote_search_for_a_number_asks_for_that_issue_of_the_board() {
    use kolum_domain::{Filter, Query};
    let mut app = loaded().await;
    let typed = Instant::now();
    press(&mut app, "<Space>F12983");
    wait_until(&mut app, typed + std::time::Duration::from_millis(300));
    let wanted = Query::Filter(Filter { text: Some("ACME-12983".into()), ..Filter::default() });
    assert_eq!(searches(app.take_effects()), [(1, wanted)]);
}

/// `E` picks one rich text field; only that one goes to `$EDITOR` and back.
#[tokio::test]
async fn one_field_is_edited_at_a_time() {
    use kolum_domain::{FieldValue, IssueUpdate, RichText};
    let mut app = loaded().await;
    press(&mut app, "/ACME-10<CR><CR>");
    app.take_effects();
    edit_field(&mut app, "accept").await;
    let Some(Effect::Edit { purpose, text }) = app.take_effects().pop() else { panic!() };
    assert!(text.starts_with("- Tokens refresh before they expire\n"), "{text}");
    assert!(!text.contains("cannot keep"), "nothing flattened here");

    app.on_edited(purpose.clone(), Ok(text.clone()));
    assert!(screen(&mut app, 140, 40).contains("No change"));
    app.on_edited(purpose.clone(), Ok(String::new()));
    assert!(screen(&mut app, 140, 40).contains("Nothing written"));
    app.on_edited(purpose, Ok(text.replacen("refresh", "rotate", 1)));
    let rotate = RichText::from_markdown("- Tokens rotate before they expire");
    let update = IssueUpdate {
        fields: vec![edit(
            "customfield_10030",
            "Acceptance criteria",
            FieldValue::RichText(rotate),
        )],
        ..Default::default()
    };
    let sent = writes(app.take_effects());
    let op = sent[0].0;
    let sent: Vec<_> = sent.into_iter().map(|(_, k, w)| (k, w)).collect();
    assert_eq!(sent, [("ACME-10".to_string(), kolum_app::Write::Update(update))]);
    assert!(screen(&mut app, 140, 40).contains("Tokens rotate before they expire"), "at once");
    let key = IssueKey::parse("ACME-10");
    let error = kolum_domain::DomainError::Conflict("stale".into());
    app.on_event(Event::WriteFailed { op, key: key.clone(), error });
    assert!(app.take_effects().is_empty(), "nothing to read back");
    let shown = screen(&mut app, 140, 40);
    assert!(shown.contains("Tokens refresh before they expire"), "undone at once: {shown}");

    // A field Jira had to flatten says so before it is overwritten.
    let mut issue = FakeProvider::new().issue(&IssueKey::parse("ACME-10")).await.unwrap();
    for field in &mut issue.fields {
        if let FieldValue::RichText(text) = &mut field.value {
            text.lossy = true;
        }
    }
    app.on_event(Event::IssueLoaded(std::sync::Arc::new(IssueDetail {
        issue,
        comments: vec![],
        activity: vec![],
        layout: None,
        children: vec![],
    })));
    edit_field(&mut app, "accept").await;
    let Some(Effect::Edit { text, .. }) = app.take_effects().pop() else { panic!() };
    assert!(text.contains("Markdown cannot keep"), "{text}");

    edit_field(&mut app, "description").await;
    let Some(Effect::Edit { purpose, text }) = app.take_effects().pop() else { panic!() };
    app.on_edited(purpose, Ok(format!("Rewritten\n{text}")));
    let Some((_, _, kolum_app::Write::Update(update))) = writes(app.take_effects()).pop() else {
        panic!()
    };
    assert_eq!((update.title, update.fields), (None, vec![]), "the description alone");
    assert!(update.description.unwrap().plain_text().starts_with("Rewritten"));
}

/// A field set by id, as `E` sends it.
fn edit(id: &str, name: &str, value: kolum_domain::FieldValue) -> kolum_domain::FieldEdit {
    let id = kolum_domain::FieldId::new(id);
    kolum_domain::FieldEdit {
        provider_id: id.clone(),
        id,
        name: Some(name.into()),
        value: Some(value),
    }
}

/// What the provider says the issue lets kolum set, as the service would.
async fn editable(key: &str) -> Event {
    use kolum_domain::IssueWriter;
    let key = IssueKey::parse(key);
    let result = FakeProvider::new().editable_fields(&key).await;
    Event::EditableFields { key, result }
}

/// `E` on the open issue, then the field whose name starts with `field`.
async fn edit_field(app: &mut App, field: &str) {
    press(app, "E");
    app.on_event(editable("ACME-10").await);
    press(app, field);
    settle(app);
    press(app, "<CR>");
    settle(app);
}

fn updates(app: &mut App) -> Vec<kolum_domain::IssueUpdate> {
    writes(app.take_effects())
        .into_iter()
        .filter_map(|(_, _, w)| match w {
            kolum_app::Write::Update(u) => Some(u),
            _ => None,
        })
        .collect()
}

/// Every field the provider lets set, each edited its own way: a choice
/// in a picker, a number or a date on the bottom line.
#[tokio::test]
async fn e_edits_every_field_the_issue_lets_set() {
    use kolum_domain::{FieldId, FieldValue, IssueUpdate, Priority};
    let mut app = loaded().await;
    press(&mut app, "/ACME-10<CR><CR>");
    app.take_effects();
    press(&mut app, "E");
    let asked = app.take_effects();
    assert_eq!(asked, [Effect::Send(Command::EditableFields(IssueKey::parse("ACME-10")))]);
    app.on_event(editable("ACME-10").await);
    settle(&mut app);
    let shown = screen(&mut app, 140, 40);
    for name in ["Edit ACME-10", "Status", "Team", "Story points", "Due date"] {
        assert!(shown.contains(name), "{name}: {shown}");
    }
    press(&mut app, "<Esc><Esc>");

    edit_field(&mut app, "team").await;
    assert!(screen(&mut app, 140, 40).contains("Mobile"), "the options");
    press(&mut app, "mobile");
    settle(&mut app);
    press(&mut app, "<CR>");
    let team = edit("customfield_10020", "Team", FieldValue::Text("Mobile".into()));
    assert_eq!(
        updates(&mut app),
        [IssueUpdate { fields: vec![team.clone()], ..Default::default() }]
    );

    edit_field(&mut app, "story").await;
    assert!(screen(&mut app, 140, 40).contains("Story points: 3"), "the bottom line");
    press(&mut app, "<C-u>5<CR>");
    let points = edit(FieldId::ESTIMATE, "Story points", FieldValue::Number(5.0));
    assert_eq!(updates(&mut app), [IssueUpdate { fields: vec![points], ..Default::default() }]);

    edit_field(&mut app, "due").await;
    press(&mut app, "soon<CR>");
    assert!(screen(&mut app, 140, 40).contains("a date reads YYYY-MM-DD"));
    assert_eq!(updates(&mut app), [], "nothing sent");

    edit_field(&mut app, "priority").await;
    press(&mut app, "lowest");
    settle(&mut app);
    press(&mut app, "<CR>");
    let lowest = IssueUpdate { priority: Some(Priority::Lowest), ..Default::default() };
    assert_eq!(updates(&mut app), [lowest]);

    edit_field(&mut app, "summary").await;
    assert!(screen(&mut app, 140, 40).contains("Summary: Search API returns duplicates"));
    press(&mut app, "<C-u><CR>");
    assert!(screen(&mut app, 140, 40).contains("Summary is required"));
    assert_eq!(updates(&mut app), []);

    edit_field(&mut app, "reviewer").await;
    assert!(screen(&mut app, 140, 40).contains("Reviewer of ACME-10"));
    press(&mut app, "ana");
    settle(&mut app);
    press(&mut app, "<CR>");
    let [IssueUpdate { fields, .. }] = &updates(&mut app)[..] else { panic!() };
    let [kolum_domain::FieldEdit { id, value: Some(FieldValue::User(ana)), .. }] = &fields[..]
    else {
        panic!("{fields:?}")
    };
    assert_eq!((id.as_str(), ana.display_name.as_str()), ("customfield_10050", "Ana Silva"));

    edit_field(&mut app, "platforms").await;
    press(&mut app, "<Tab><Tab><CR>");
    let both = FieldValue::Options(vec!["Web".into(), "iOS".into()]);
    let platforms = edit("customfield_10060", "Platforms", both);
    assert_eq!(updates(&mut app), [IssueUpdate { fields: vec![platforms], ..Default::default() }]);

    edit_field(&mut app, "team").await;
    press(&mut app, "none");
    settle(&mut app);
    press(&mut app, "<CR>");
    let emptied = kolum_domain::FieldEdit { value: None, ..team };
    assert_eq!(updates(&mut app), [IssueUpdate { fields: vec![emptied], ..Default::default() }]);

    edit_field(&mut app, "sprint").await;
    let board = kolum_domain::BoardId::new("1");
    assert!(app.take_effects().contains(&Effect::Send(Command::Sprints(board.clone()))));
    let sprints = kolum_domain::BoardSource::sprints(&FakeProvider::new(), &board).await;
    app.on_event(Event::Sprints(sprints));
    settle(&mut app);
    assert!(screen(&mut app, 140, 40).contains("Sprint of ACME-10"));
    press(&mut app, "15");
    settle(&mut app);
    press(&mut app, "<CR>");
    let fifteen = kolum_domain::Sprint { id: "15".into(), name: "Sprint 15".into() };
    let sprint = edit(FieldId::SPRINT, "Sprint", FieldValue::Sprint(fifteen));
    assert_eq!(updates(&mut app), [IssueUpdate { fields: vec![sprint], ..Default::default() }]);
    let error = kolum_domain::DomainError::Transport("kanban".into());
    app.on_event(Event::Sprints(Err(error.clone())));
    assert!(!screen(&mut app, 140, 40).contains("kanban"), "nothing asked for them");
    edit_field(&mut app, "sprint").await;
    app.on_event(Event::Sprints(Err(error)));
    assert!(screen(&mut app, 140, 40).contains("The board's sprints: transport: kanban"));

    press(&mut app, "E");
    let key = IssueKey::parse("ACME-10");
    let error = kolum_domain::DomainError::Transport("timed out".into());
    app.on_event(Event::EditableFields { key, result: Err(error) });
    assert!(screen(&mut app, 140, 40).contains("Reading the fields of ACME-10: transport"));

    edit_field(&mut app, "status").await;
    let asked = Effect::Send(Command::Transitions(IssueKey::parse("ACME-10")));
    assert!(app.take_effects().contains(&asked), "the transitions, as t asks");
}

/// A field set with `E` shows at once, before the provider answers: a
/// field the issue holds as a member (the due date), set then emptied,
/// and a custom field the issue had nothing in yet.
#[tokio::test]
async fn a_field_set_with_e_shows_at_once() {
    let mut app = loaded().await;
    press(&mut app, "/ACME-10<CR><CR>");
    let due_line = |app: &mut App| {
        let shown = screen(app, 140, 40);
        let lines: Vec<&str> = shown.lines().collect();
        let at = lines.iter().position(|l| l.contains("Due date")).expect("a due date line");
        lines[at + 1].to_string()
    };
    assert!(!due_line(&mut app).contains("2026-12-01"));

    edit_field(&mut app, "due").await;
    press(&mut app, "2026-12-01<CR>");
    assert!(due_line(&mut app).contains("2026-12-01"), "set: {}", due_line(&mut app));

    edit_field(&mut app, "due").await;
    assert!(screen(&mut app, 140, 40).contains("Due date: 2026-12-01"), "the value to edit");
    press(&mut app, "<C-u><CR>");
    assert!(due_line(&mut app).contains("None"), "emptied: {}", due_line(&mut app));

    assert!(!screen(&mut app, 140, 60).contains("Reviewer"), "empty, so not shown");
    edit_field(&mut app, "reviewer").await;
    press(&mut app, "ana");
    settle(&mut app);
    press(&mut app, "<CR>");
    let shown = screen(&mut app, 140, 60);
    assert!(!shown.contains("customfield_10050"), "by its name: {shown}");
    let lines: Vec<&str> = shown.lines().collect();
    let at = lines.iter().position(|l| l.contains("Reviewer"));
    let at = at.unwrap_or_else(|| panic!("{shown}"));
    assert!(lines[at + 1].contains("Ana Silva"), "{shown}");
}

/// The line under a label in the detail's right-hand column.
fn detail_value(app: &mut App, label: &str) -> String {
    let shown = screen(app, 140, 60);
    let lines: Vec<&str> = shown.lines().collect();
    let at = lines.iter().position(|l| l.contains(&format!("│{label} ")));
    let at = at.unwrap_or_else(|| panic!("{label}: {shown}"));
    lines[at + 1].to_string()
}

/// What `E`'s preview on ACME-10 shows for the field whose name starts so.
async fn preview(app: &mut App, field: &str) -> String {
    press(app, "E");
    app.on_event(editable("ACME-10").await);
    press(app, field);
    settle(app);
    let shown = screen(app, 140, 40);
    press(app, "<Esc><Esc>");
    shown.lines().nth(4).unwrap_or("").rsplit("││").next().unwrap_or("").to_string()
}

/// The sprint and the story points are the issue's own: `E` shows them
/// and sets them at once, with no custom field beside.
#[tokio::test]
async fn e_reads_and_sets_the_sprint_and_the_estimate_of_the_issue() {
    let mut app = loaded().await;
    press(&mut app, "/ACME-10<CR><CR>");
    let sprint = preview(&mut app, "sprint").await;
    assert!(sprint.starts_with("Sprint 14"), "{sprint:?}");
    let points = preview(&mut app, "story").await;
    assert!(points.starts_with("3 "), "{points:?}");

    edit_field(&mut app, "story").await;
    press(&mut app, "<C-u>5<CR>");
    assert!(
        detail_value(&mut app, "Estimate").contains('5'),
        "{}",
        detail_value(&mut app, "Estimate")
    );
    edit_field(&mut app, "sprint").await;
    let board = kolum_domain::BoardId::new("1");
    let sprints = kolum_domain::BoardSource::sprints(&FakeProvider::new(), &board).await;
    app.on_event(Event::Sprints(sprints));
    press(&mut app, "15");
    settle(&mut app);
    press(&mut app, "<CR>");
    assert!(detail_value(&mut app, "Sprint").contains("Sprint 15"));
    let shown = screen(&mut app, 140, 60);
    assert!(!shown.contains("customfield_") && !shown.contains("Story points"), "{shown}");
}

/// A parent set by key shows whole when the board has that issue; one it
/// does not have waits for the read-back rather than show a blank.
#[tokio::test]
async fn a_parent_set_with_e_shows_the_issue_the_board_knows() {
    let mut app = loaded().await;
    press(&mut app, "/ACME-10<CR><CR>");
    assert!(detail_value(&mut app, "Parent").contains("None"));
    edit_field(&mut app, "parent").await;
    press(&mut app, "ACME-1<CR>");
    assert!(detail_value(&mut app, "Parent").contains("None"), "not on the board: as it was");
    edit_field(&mut app, "parent").await;
    press(&mut app, "<C-u>acme-13<CR>");
    let parent = detail_value(&mut app, "Parent");
    assert!(parent.contains("◆ Webhooks for invoice events"), "{parent}");
}

/// A provider that cannot list its fields still lets `E` edit the
/// description and the status.
#[tokio::test]
async fn e_falls_back_to_the_description_and_the_status() {
    let mut app = loaded().await;
    press(&mut app, "/ACME-10<CR><CR>E");
    let key = IssueKey::parse("ACME-10");
    let unsupported =
        kolum_domain::DomainError::Unsupported(kolum_domain::Capability::CustomFields);
    app.on_event(Event::EditableFields { key, result: Err(unsupported) });
    settle(&mut app);
    let shown = screen(&mut app, 140, 40);
    assert!(shown.contains("Status") && shown.contains("Description"), "{shown}");
    assert!(!shown.contains("Due date"), "only what every provider has");
}

/// The board keeps up with Jira by itself: what changed, every `sync_secs`;
/// and 5 s after a change of yours, once Jira automations have run on it.
/// Each board request under the next number.
#[tokio::test]
async fn the_board_syncs_by_itself_and_soon_after_a_change() {
    use std::time::Duration;
    // The board as it was asked for: the provider's first here.
    let sync = |n| Effect::Send(Command::SyncBoard(None, n));
    assert_eq!(loaded().await.deadline(), None, "sync_secs = 0: none on its own");

    let mut settings = settings();
    settings.ui.sync_secs = 30;
    let start = Instant::now();
    let mut app = loaded_with(settings).await;
    let due = app.deadline().expect("the next sync");
    assert!(due >= start + Duration::from_secs(29), "in 30 s");
    wait_until(&mut app, due);
    assert_eq!(app.take_effects(), [sync(2)]);

    press(&mut app, "<Space>A"); // assign the card to me: on its way
    let (op, key, _) = writes(app.take_effects()).remove(0);
    let due = app.deadline().unwrap();
    wait_until(&mut app, due);
    assert!(app.take_effects().contains(&sync(3)), "the write on its way shows over it");
    let saved = Instant::now();
    app.on_event(Event::Written { op, key: IssueKey::parse(&key), issue: None });
    let due = app.deadline().unwrap();
    assert!(due <= saved + Duration::from_secs(6), "soon after Jira has it");
    wait_until(&mut app, due);
    assert!(app.take_effects().contains(&sync(4)));

    press(&mut app, ":board 7<CR>");
    app.on_event(Event::Written { op: 99, key: IssueKey::parse("ACME-5"), issue: None });
    let due = app.deadline().unwrap();
    wait_until(&mut app, due);
    let seven = Effect::Send(Command::SyncBoard(Some(kolum_domain::BoardId::new("7")), 6));
    assert!(app.take_effects().contains(&seven), "the board asked for since");
}

#[tokio::test]
async fn the_boards_picker_opens_another_board() {
    let mut app = loaded().await;
    app.take_effects();
    press(&mut app, "<Space>b");
    assert_eq!(app.take_effects(), [Effect::Send(Command::LoadBoards)]);
    let boards = kolum_domain::BoardSource::boards(&FakeProvider::new()).await.unwrap();
    app.on_event(Event::Boards(boards));
    settle(&mut app);
    assert!(screen(&mut app, 150, 40).contains("Acme Web board  ACME  Kanban #1"));
    press(&mut app, "<CR>");
    assert_eq!(
        app.take_effects(),
        [Effect::Send(Command::LoadBoard(Some(kolum_domain::BoardId::new("1")), 2))]
    );
    press(&mut app, ":board 7<CR>");
    assert_eq!(
        app.take_effects(),
        [Effect::Send(Command::LoadBoard(Some(kolum_domain::BoardId::new("7")), 3))]
    );
}

#[tokio::test]
async fn a_cached_board_says_so_until_the_sync_lands() {
    let fake = FakeProvider::new();
    let snapshot = load_board(&fake, None).await.unwrap();
    let mut app = App::new(settings(), Arc::new(|| {}));
    app.on_event(Event::BoardLoaded { snapshot: snapshot.clone(), fresh: false, request: 1 });
    assert!(screen(&mut app, 150, 40).contains("cached, syncing…"));
    app.on_event(Event::BoardLoaded { snapshot, fresh: true, request: 1 });
    assert!(!screen(&mut app, 150, 40).contains("cached"));
}

#[tokio::test]
async fn pickers_without_a_remote_say_so() {
    let mut app = loaded().await;
    press(&mut app, "<Space>t<C-Space>");
    assert!(screen(&mut app, 150, 40).contains("This picker has no remote search"));
}

/// The transitions the fake has for `key`, as the service would answer.
async fn transitions_event(key: &str) -> Event {
    use kolum_domain::IssueWriter;
    let key = IssueKey::parse(key);
    let result = FakeProvider::new().transitions(&key).await;
    Event::Transitions { key, result }
}

fn writes(effects: Vec<Effect>) -> Vec<(u64, String, kolum_app::Write)> {
    effects
        .into_iter()
        .filter_map(|e| match e {
            Effect::Send(Command::Write { op, key, write }) => Some((op, key.to_string(), write)),
            _ => None,
        })
        .collect()
}

/// Sideways the board scrolls like a list: from the right edge, going
/// left moves the highlight first; the columns slide only once it would
/// leave the screen.
#[tokio::test]
async fn the_columns_slide_only_when_the_highlight_leaves_the_screen() {
    let mut app = loaded().await;
    let columns = ["BACKLOG", "TO DO", "IN PROGRESS", "IN REVIEW", "DONE"];
    let mut shown = |keys: &str| {
        if !keys.is_empty() {
            press(&mut app, keys);
        }
        let top: String = screen(&mut app, 90, 40).lines().take(3).collect();
        columns.into_iter().filter(|c| top.contains(c)).collect::<Vec<_>>()
    };
    assert_eq!(shown(""), ["BACKLOG", "TO DO", "IN PROGRESS"], "three fit");
    let right = ["IN PROGRESS", "IN REVIEW", "DONE"];
    assert_eq!(shown("$"), right);
    assert_eq!(shown("h"), right, "the highlight moves, not the columns");
    assert_eq!(shown("h"), right, "still on screen");
    assert_eq!(shown("h"), ["TO DO", "IN PROGRESS", "IN REVIEW"], "past the left edge");
    assert_eq!(shown("ll"), ["TO DO", "IN PROGRESS", "IN REVIEW"]);
    assert_eq!(shown("l"), right, "past the right edge");
}

fn column_of(app: &mut App, key: &str) -> usize {
    // The board draws columns left to right; find which one holds the key.
    let shown = screen(app, 150, 60);
    let line = shown.lines().find(|l| l.contains(&format!("{key} "))).unwrap();
    let at = line.find(&format!("{key} ")).unwrap();
    line[..at].chars().count() / 30
}

#[tokio::test]
async fn l_moves_a_card_at_once_and_a_refusal_puts_it_back() {
    let mut app = loaded().await;
    app.take_effects();
    assert_eq!(column_of(&mut app, "ACME-9"), 0);
    press(&mut app, "L");
    assert_eq!(app.take_effects(), [Effect::Send(Command::Transitions(IssueKey::parse("ACME-9")))]);
    app.on_event(transitions_event("ACME-9").await);
    let sent = writes(app.take_effects());
    assert_eq!(sent.len(), 1, "one transition leads to To Do: no question asked");
    assert_eq!(sent[0].2, kolum_app::Write::Transition(kolum_domain::TransitionId::new("to-todo")));
    assert_eq!(column_of(&mut app, "ACME-9"), 1, "moved before the provider answered");
    assert_eq!(selected(&app), "ACME-9", "the highlight follows the card");

    let error = kolum_domain::DomainError::Conflict("workflow changed".into());
    app.on_event(Event::WriteFailed { op: sent[0].0, key: IssueKey::parse("ACME-9"), error });
    assert_eq!(column_of(&mut app, "ACME-9"), 0, "undone");
    assert!(screen(&mut app, 150, 60).contains("ACME-9: conflict: workflow changed; undone"));
}

/// The board as the provider has it now: ACME-9 renamed and moved to Done
/// by a colleague, ACME-11 ranked to the top.
async fn synced_meanwhile() -> kolum_app::BoardSnapshot {
    let mut fresh = load_board(&FakeProvider::new(), None).await.unwrap();
    let done = fresh.issues.iter().find(|i| i.key.as_str() == "ACME-8").unwrap().status.clone();
    let nine = fresh.issues.iter_mut().find(|i| i.key.as_str() == "ACME-9").unwrap();
    nine.title = "Renamed by Ana".into();
    nine.status = done;
    let eleven = fresh.issues.iter().position(|i| i.key.as_str() == "ACME-11").unwrap();
    let eleven = fresh.issues.remove(eleven);
    fresh.issues.insert(0, eleven);
    fresh
}

#[tokio::test]
async fn a_sync_before_the_reply_keeps_the_move_and_a_refusal_keeps_the_sync() {
    let mut app = loaded().await;
    app.take_effects();
    press(&mut app, "L");
    app.on_event(transitions_event("ACME-9").await);
    let op = writes(app.take_effects())[0].0;
    app.on_event(Event::BoardLoaded {
        snapshot: synced_meanwhile().await,
        fresh: true,
        request: 1,
    });
    assert_eq!(column_of(&mut app, "ACME-9"), 1, "still where it was put");
    assert!(screen(&mut app, 150, 60).contains("Renamed by Ana"));

    let error = kolum_domain::DomainError::Conflict("workflow changed".into());
    app.on_event(Event::WriteFailed { op, key: IssueKey::parse("ACME-9"), error });
    assert_eq!(column_of(&mut app, "ACME-9"), 4, "where the sync has it: Done");
    assert!(screen(&mut app, 150, 60).contains("Renamed by Ana"), "the colleague's change kept");
}

#[tokio::test]
async fn a_sync_before_the_reply_keeps_the_rank_and_a_refusal_keeps_the_synced_order() {
    let mut app = loaded().await;
    press(&mut app, "ll"); // In Progress: ACME-5, 10, 11…
    app.take_effects();
    press(&mut app, "J");
    let op = writes(app.take_effects())[0].0;
    app.on_event(Event::BoardLoaded {
        snapshot: synced_meanwhile().await,
        fresh: true,
        request: 1,
    });
    assert_eq!(&column_order(&mut app, 2)[..3], ["ACME-11", "ACME-10", "ACME-5"], "both");

    let error = kolum_domain::DomainError::Conflict("Rank is not configured".into());
    app.on_event(Event::WriteFailed { op, key: IssueKey::parse("ACME-5"), error });
    assert_eq!(&column_order(&mut app, 2)[..3], ["ACME-11", "ACME-5", "ACME-10"], "the sync's");
}

/// The provider's copy of an issue read back after a write: a minute
/// newer than the board's.
async fn read_back(
    key: &str,
    change: impl FnOnce(&mut kolum_domain::Issue),
) -> kolum_domain::Issue {
    let mut issue = FakeProvider::new().issue(&IssueKey::parse(key)).await.unwrap();
    change(&mut issue);
    issue.updated += time::Duration::minutes(1);
    issue
}

/// Replies come back out of order: a sync that left before the write's
/// reply, and lands after it, does not undo the write.
#[tokio::test]
async fn a_sync_slower_than_the_write_does_not_undo_it() {
    let mut app = loaded().await;
    app.take_effects();
    press(&mut app, "L");
    app.on_event(transitions_event("ACME-9").await);
    let op = writes(app.take_effects())[0].0;
    let todo = load_board(&FakeProvider::new(), None).await.unwrap().issues;
    let todo = todo.iter().find(|i| i.status.name == "To Do").unwrap().status.clone();
    let issue = read_back("ACME-9", |i| i.status = todo).await;
    app.on_event(Event::Written { op, key: issue.key.clone(), issue: Some(Box::new(issue)) });
    let stale = load_board(&FakeProvider::new(), None).await.unwrap();
    app.on_event(Event::BoardLoaded { snapshot: stale, fresh: true, request: 1 });
    assert_eq!(column_of(&mut app, "ACME-9"), 1, "the write's copy is newer");
}

/// A read sent before a write, answered after its reply, does not undo
/// the write on the open issue.
#[tokio::test]
async fn a_read_slower_than_the_write_does_not_undo_it_on_the_open_issue() {
    let mut app = loaded().await;
    press(&mut app, "/ACME-10<CR><CR>");
    let before = FakeProvider::new().issue(&IssueKey::parse("ACME-10")).await.unwrap();
    press(&mut app, "<Space>A");
    let op = writes(app.take_effects())[0].0;
    let me = FakeProvider::new().me().await.unwrap();
    let issue = read_back("ACME-10", |i| i.assignee = Some(me)).await;
    app.on_event(Event::Written { op, key: issue.key.clone(), issue: Some(Box::new(issue)) });
    let detail = IssueDetail {
        issue: before,
        comments: vec![],
        activity: vec![],
        layout: None,
        children: vec![],
    };
    app.on_event(Event::IssueLoaded(std::sync::Arc::new(detail)));
    assert!(detail_value(&mut app, "Assignee").contains("Colin Tenaguillo"));
}

/// An epic, in progress but not a card: what the provider reads back
/// after a write does not put it on the board.
#[tokio::test]
async fn a_write_to_an_issue_off_the_board_leaves_it_off() {
    let mut app = loaded().await;
    let epic = FakeProvider::new().issue(&IssueKey::parse("ACME-1")).await.unwrap();
    let key = epic.key.clone();
    app.on_event(Event::Written { op: 1, key, issue: Some(Box::new(epic)) });
    assert!(!column_order(&mut app, 2).contains(&"ACME-1".to_string()));
}

/// Before the board has arrived, or after it failed, an issue opened from
/// a search is still written to.
#[tokio::test]
async fn writes_reach_the_provider_with_no_board() {
    use kolum::ui::EditPurpose;
    let fake = FakeProvider::new();
    let mut app = App::new(settings(), Arc::new(|| {}));
    app.on_event(Event::Me(fake.me().await.unwrap()));
    let key = IssueKey::parse("ACME-9");
    press(&mut app, "<Space>F");
    let due = app.deadline().unwrap();
    wait_until(&mut app, due);
    let issue = fake.issue(&key).await.unwrap();
    app.on_event(Event::SearchResults { id: 1, result: Ok(vec![issue.clone()]) });
    settle(&mut app);
    press(&mut app, "<CR>");
    let detail =
        IssueDetail { issue, comments: vec![], activity: vec![], layout: None, children: vec![] };
    app.on_event(Event::IssueLoaded(std::sync::Arc::new(detail)));
    assert_eq!(app.open_issue(), Some(&key));
    app.take_effects();

    press(&mut app, "<Space>A");
    let me = kolum_app::Write::Assign(Some(kolum_domain::UserId::new("u-colin")));
    assert_eq!(writes(app.take_effects()).pop().map(|w| w.2), Some(me));
    press(&mut app, "t");
    app.on_event(transitions_event("ACME-9").await);
    settle(&mut app);
    press(&mut app, "<CR>");
    let sent = writes(app.take_effects()).pop().map(|w| w.2);
    assert!(matches!(sent, Some(kolum_app::Write::Transition(_))), "{sent:?}");
    app.on_edited(EditPurpose::Comment(key), Ok("Looks good".into()));
    let sent = writes(app.take_effects()).pop().map(|w| w.2);
    assert!(matches!(sent, Some(kolum_app::Write::Comment(_))), "{sent:?}");
}

/// A field set at once on the open issue goes again when the provider
/// refuses it, without waiting for a read.
#[tokio::test]
async fn a_refused_field_leaves_the_detail_at_once() {
    let mut app = loaded().await;
    press(&mut app, "/ACME-10<CR><CR>");
    edit_field(&mut app, "reviewer").await;
    press(&mut app, "ana");
    settle(&mut app);
    press(&mut app, "<CR>");
    let op = writes(app.take_effects())[0].0;
    assert!(screen(&mut app, 140, 60).contains("Reviewer"), "shown at once");
    let error = kolum_domain::DomainError::Conflict("stale".into());
    app.on_event(Event::WriteFailed { op, key: IssueKey::parse("ACME-10"), error });
    assert!(!screen(&mut app, 140, 60).contains("Reviewer"), "gone with the refusal");
}

#[tokio::test]
async fn a_move_into_a_column_of_two_statuses_asks_which() {
    let mut app = loaded().await;
    app.take_effects();
    press(&mut app, "jL"); // ACME-16 is in the Backlog; To Do first
    app.on_event(transitions_event("ACME-16").await);
    let first = writes(app.take_effects());
    app.on_event(Event::Written { op: first[0].0, key: IssueKey::parse("ACME-16"), issue: None });
    press(&mut app, "L");
    app.on_event(transitions_event("ACME-16").await);
    settle(&mut app);
    let shown = screen(&mut app, 150, 40);
    assert!(
        shown.contains("Move ACME-16")
            && shown.contains("In Progress")
            && shown.contains("Blocked"),
        "{shown}"
    );
    press(&mut app, "<C-n><CR>");
    let sent = writes(app.take_effects());
    assert_eq!(
        sent[0].2,
        kolum_app::Write::Transition(kolum_domain::TransitionId::new("to-blocked"))
    );
}

#[tokio::test]
async fn transitions_that_cannot_be_done_here_say_why() {
    let mut app = loaded().await;
    press(&mut app, "H");
    assert!(screen(&mut app, 120, 30).contains("No column that way"));
    app.take_effects();
    press(&mut app, "t");
    let mut required = transitions_event("ACME-9").await;
    if let Event::Transitions { result: Ok(ts), .. } = &mut required {
        ts.truncate(1);
        ts[0].required_fields = vec![kolum_domain::FieldId::new("resolution")];
    }
    app.on_event(required);
    settle(&mut app);
    press(&mut app, "<CR>");
    assert!(writes(app.take_effects()).is_empty());
    assert!(
        screen(&mut app, 150, 30).contains("asks for resolution: press o to do it in the browser")
    );

    press(&mut app, "L");
    app.on_event(Event::Transitions { key: IssueKey::parse("ACME-9"), result: Ok(vec![]) });
    assert!(screen(&mut app, 150, 30).contains("No transition takes ACME-9 to To Do from here"));
    press(&mut app, "t");
    app.on_event(Event::Transitions { key: IssueKey::parse("ACME-1"), result: Ok(vec![]) });
    assert!(
        !screen(&mut app, 150, 30).contains("No transition"),
        "a reply for another issue is ignored"
    );
}

/// Picked while a colleague moved the card there: nothing is sent, and
/// nothing claims a move.
#[tokio::test]
async fn a_transition_to_where_the_card_already_is_sends_nothing() {
    let mut app = loaded().await;
    app.take_effects();
    press(&mut app, "t");
    let mut to_done = transitions_event("ACME-9").await;
    if let Event::Transitions { result: Ok(ts), .. } = &mut to_done {
        ts.retain(|t| t.to.name == "Done");
    }
    app.on_event(to_done);
    settle(&mut app);
    app.on_event(Event::BoardLoaded {
        snapshot: synced_meanwhile().await,
        fresh: true,
        request: 1,
    });
    press(&mut app, "<CR>");
    assert!(writes(app.take_effects()).is_empty());
    let shown = screen(&mut app, 150, 30);
    assert!(shown.contains("ACME-9 already in Done"), "{shown}");
}

#[tokio::test]
async fn assigning_from_the_picker_and_to_myself() {
    let mut app = loaded().await;
    app.take_effects();
    press(&mut app, "<Space>a");
    settle(&mut app);
    let shown = screen(&mut app, 150, 40);
    let me = shown.find("Colin Tenaguillo (CT)").unwrap();
    assert!(me < shown.find("Ana Silva (AS)").unwrap(), "me first");
    assert!(shown.contains("Unassigned"));
    press(&mut app, "ana");
    settle(&mut app);
    press(&mut app, "<CR>");
    let sent = writes(app.take_effects());
    assert_eq!(sent[0].2, kolum_app::Write::Assign(Some(kolum_domain::UserId::new("u-ana"))));
    press(&mut app, "<Space>A");
    let sent = writes(app.take_effects());
    assert_eq!(sent[0].2, kolum_app::Write::Assign(Some(kolum_domain::UserId::new("u-colin"))));
    assert!(screen(&mut app, 150, 40).contains("Assigned to Colin Tenaguillo"));
}

#[tokio::test]
async fn a_comment_goes_through_the_editor() {
    use kolum::ui::EditPurpose;
    let mut app = loaded().await;
    app.take_effects();
    press(&mut app, "i");
    let Some(Effect::Edit { purpose, text }) = app.take_effects().pop() else {
        panic!("expected the editor")
    };
    assert_eq!(purpose, EditPurpose::Comment(IssueKey::parse("ACME-9")));
    assert!(text.contains("<!-- kolum:") && text.contains("ACME-9"));

    app.on_edited(purpose.clone(), Ok(text.clone()));
    assert!(writes(app.take_effects()).is_empty(), "left empty: cancelled");
    app.on_edited(purpose.clone(), Ok(format!("Looks **good**\n{text}")));
    let sent = writes(app.take_effects());
    assert_eq!(
        sent[0].2,
        kolum_app::Write::Comment(kolum_domain::RichText::from_markdown("Looks **good**"))
    );
    app.on_edited(purpose, Err("vim exited with 1".into()));
    assert!(screen(&mut app, 120, 30).contains("$EDITOR: vim exited with 1"));
}

#[tokio::test]
async fn editing_sends_only_what_changed() {
    use kolum::ui::EditPurpose;
    let mut app = loaded().await;
    press(&mut app, "<CR>");
    app.take_effects();
    // The board's copy is no ground for an edit: the issue is read first.
    press(&mut app, "e");
    let key = IssueKey::parse("ACME-9");
    assert_eq!(app.take_effects(), [Effect::Send(Command::LoadIssue(key.clone()))]);
    assert!(screen(&mut app, 120, 30).contains("Reading ACME-9…"));
    let issue = FakeProvider::new().issue(&key).await.unwrap();
    let detail =
        IssueDetail { issue, comments: vec![], activity: vec![], layout: None, children: vec![] };
    app.on_event(Event::IssueLoaded(std::sync::Arc::new(detail)));
    let Some(Effect::Edit { purpose, text }) = app.take_effects().pop() else { panic!() };
    assert!(matches!(&purpose, EditPurpose::Describe { key: k, .. } if *k == key), "{purpose:?}");
    assert!(text.starts_with("# Enforce SSO per organisation\n\nAn admin switch"), "{text}");
    assert!(!screen(&mut app, 120, 30).contains("Reading ACME-9…"));

    app.on_edited(purpose.clone(), Ok(text.clone()));
    assert!(writes(app.take_effects()).is_empty());
    assert!(screen(&mut app, 120, 30).contains("No change"));

    app.on_edited(purpose.clone(), Ok(text.replacen("# Enforce SSO", "# Require SSO", 1)));
    let sent = writes(app.take_effects());
    let kolum_app::Write::Update(update) = &sent[0].2 else { panic!() };
    assert_eq!(update.title.as_deref(), Some("Require SSO per organisation"));
    assert_eq!(update.description, None, "the description did not move");
    assert!(
        screen(&mut app, 120, 30).contains("Require SSO per organisation"),
        "the detail shows it at once"
    );

    // Edited again, from the title as it is now.
    press(&mut app, "e");
    let Some(Effect::Edit { purpose, .. }) = app.take_effects().pop() else { panic!() };
    app.on_edited(purpose, Ok("# Require SSO per organisation\n\nNew *body*".into()));
    assert_eq!(app.take_effects(), [Effect::Send(Command::LoadIssue(key.clone()))], "read first");
    let issue = FakeProvider::new().issue(&key).await.unwrap();
    let detail =
        IssueDetail { issue, comments: vec![], activity: vec![], layout: None, children: vec![] };
    app.on_event(Event::IssueLoaded(std::sync::Arc::new(detail)));
    let sent = writes(app.take_effects());
    let kolum_app::Write::Update(update) = &sent[0].2 else { panic!() };
    assert_eq!(
        (update.title.as_ref(), update.description.as_ref().map(|d| d.plain_text())),
        (None, Some("New body".into()))
    );
}

/// An edit starts from the issue read whole, never from an older read: the
/// board's copy lacks what Markdown cannot hold, or the description itself.
#[tokio::test]
async fn an_edit_starts_from_a_read_as_new_as_the_board() {
    let fake = FakeProvider::new();
    let mut app = loaded().await;
    let key = IssueKey::parse("ACME-9");
    let mut read = fake.issue(&key).await.unwrap();
    read.description = read.description.map(|d| kolum_domain::RichText { lossy: true, ..d });
    let detail = |issue| IssueDetail {
        issue,
        comments: vec![],
        activity: vec![],
        layout: None,
        children: vec![],
    };
    app.on_event(Event::ReadIssues(vec![std::sync::Arc::new(detail(read.clone()))]));
    app.take_effects();
    press(&mut app, "e");
    let Some(Effect::Edit { purpose, text }) = app.take_effects().pop() else { panic!() };
    assert!(text.contains("An admin switch"), "{text}");
    assert!(text.contains("Markdown cannot keep"), "the loss is said: {text}");

    // The issue changed on Jira since, a comment say: it is read again,
    // and the description, which did not move, is saved.
    let mut snapshot = load_board(&fake, None).await.unwrap();
    let on_board = snapshot.issues.iter_mut().find(|i| i.key == key).unwrap();
    on_board.updated = read.updated + time::Duration::minutes(1);
    app.on_event(Event::BoardLoaded { snapshot, fresh: true, request: 1 });
    app.on_edited(purpose, Ok(text.replace("An admin switch", "A switch")));
    assert_eq!(app.take_effects(), [Effect::Send(Command::LoadIssue(key.clone()))], "read again");
    let mut commented = read.clone();
    commented.updated = read.updated + time::Duration::minutes(1);
    app.on_event(Event::IssueLoaded(std::sync::Arc::new(detail(commented))));
    let sent = writes(app.take_effects());
    let kolum_app::Write::Update(update) = &sent[0].2 else { panic!("{sent:?}") };
    let saved = update.description.as_ref().map(|d| d.plain_text()).unwrap_or_default();
    assert!(saved.starts_with("A switch"), "{saved}");
    app.on_event(Event::Written { op: sent[0].0, key, issue: None });
    press(&mut app, "e");
    assert!(
        matches!(app.take_effects().pop(), Some(Effect::Edit { .. })),
        "no read left waiting: the editor opens"
    );
}

/// Someone else rewrote the description since the last sync: the save
/// reads the issue right before writing, and does not write over theirs,
/// nor lose the text; the editor opens again on it, theirs below for
/// reference, and saving that goes through.
#[tokio::test]
async fn a_description_changed_meanwhile_reopens_the_editor_on_the_text() {
    let fake = FakeProvider::new();
    let mut app = loaded().await;
    let key = IssueKey::parse("ACME-9");
    let read = fake.issue(&key).await.unwrap();
    let detail = |issue| IssueDetail {
        issue,
        comments: vec![],
        activity: vec![],
        layout: None,
        children: vec![],
    };
    app.on_event(Event::ReadIssues(vec![std::sync::Arc::new(detail(read.clone()))]));
    app.take_effects();
    press(&mut app, "e");
    let Some(Effect::Edit { purpose, text }) = app.take_effects().pop() else { panic!() };

    let mut theirs = read.clone();
    theirs.description = Some(kolum_domain::RichText::from_markdown("Their rewrite"));
    theirs.updated = read.updated + time::Duration::minutes(1);
    app.on_edited(purpose, Ok(text.replace("An admin switch", "My switch")));
    let read_first = [Effect::Send(Command::LoadIssue(key.clone()))];
    assert_eq!(app.take_effects(), read_first, "read right before writing");
    app.on_event(Event::IssueLoaded(std::sync::Arc::new(detail(theirs.clone()))));
    let effects = app.take_effects();
    assert!(writes(effects.clone()).is_empty(), "not saved over theirs");
    let Some(Effect::Edit { purpose, text }) = effects.into_iter().last() else {
        panic!("the editor again")
    };
    let (mine, below) = text.split_once("<!-- kolum:").unwrap();
    assert!(mine.contains("My switch"), "{mine}");
    assert!(below.contains("Their rewrite"), "{below}");
    assert!(screen(&mut app, 120, 30).contains("changed meanwhile"));

    app.on_edited(purpose, Ok(text));
    assert_eq!(app.take_effects(), read_first);
    app.on_event(Event::IssueLoaded(std::sync::Arc::new(detail(theirs))));
    let sent = writes(app.take_effects());
    let kolum_app::Write::Update(update) = &sent[0].2 else { panic!("{sent:?}") };
    let saved = update.description.as_ref().map(|d| d.plain_text()).unwrap_or_default();
    assert!(saved.contains("My switch") && !saved.contains("Their rewrite"), "{saved}");
}

/// The open issue shows a saved description at once, though the save
/// waited on a read; and while it waits, `e` waits too, rather than
/// dropping the text being saved.
#[tokio::test]
async fn a_saved_description_shows_at_once_and_e_waits_for_the_save() {
    let fake = FakeProvider::new();
    let mut app = loaded().await;
    let key = IssueKey::parse("ACME-9");
    let read = fake.issue(&key).await.unwrap();
    let detail = |issue| IssueDetail {
        issue,
        comments: vec![],
        activity: vec![],
        layout: None,
        children: vec![],
    };
    press(&mut app, "<CR>");
    app.on_event(Event::IssueLoaded(std::sync::Arc::new(detail(read.clone()))));
    app.take_effects();
    press(&mut app, "e");
    let Some(Effect::Edit { purpose, text }) = app.take_effects().pop() else { panic!() };
    app.on_edited(purpose, Ok(text.replace("An admin switch", "My switch")));
    assert_eq!(app.take_effects(), [Effect::Send(Command::LoadIssue(key.clone()))]);
    press(&mut app, "e");
    assert!(app.take_effects().is_empty(), "`e` waits for the save");
    assert!(screen(&mut app, 120, 40).contains("ACME-9 is still being saved"));

    app.on_event(Event::IssueLoaded(std::sync::Arc::new(detail(read))));
    assert_eq!(writes(app.take_effects()).len(), 1);
    let shown = screen(&mut app, 120, 40);
    assert!(shown.contains("My switch"), "{shown}");
}

/// A read that failed leaves nothing waiting on it: a later read of the
/// same issue, for another reason, does not open the editor or the issue;
/// and a description waiting on it to be saved is kept as a draft.
#[tokio::test]
async fn a_failed_read_leaves_nothing_waiting_on_it() {
    let fake = FakeProvider::new();
    let mut app = loaded().await;
    let key = IssueKey::parse("ACME-9");
    let read = fake.issue(&key).await.unwrap();
    let detail = |issue| IssueDetail {
        issue,
        comments: vec![],
        activity: vec![],
        layout: None,
        children: vec![],
    };
    let failed = |key: &IssueKey| Event::IssueLoadFailed {
        key: key.clone(),
        error: kolum_domain::DomainError::Transport("down".into()),
    };
    app.take_effects();
    press(&mut app, "e");
    assert_eq!(app.take_effects(), [Effect::Send(Command::LoadIssue(key.clone()))]);
    app.on_event(failed(&key));
    assert!(screen(&mut app, 120, 30).contains("loading ACME-9 failed"));
    app.on_event(Event::IssueLoaded(std::sync::Arc::new(detail(read.clone()))));
    assert!(app.take_effects().is_empty(), "no editor out of the blue");

    // The epic of ACME-9 off the board: `gp` waits for it to be read.
    let epic = IssueKey::parse("ACME-1");
    let mut snapshot = load_board(&fake, None).await.unwrap();
    snapshot.issues.retain(|i| i.key != epic);
    app.on_event(Event::BoardLoaded { snapshot, fresh: true, request: 1 });
    press(&mut app, "gp");
    app.on_event(failed(&epic));
    app.on_event(Event::IssueLoaded(std::sync::Arc::new(detail(fake.issue(&epic).await.unwrap()))));
    assert_eq!(app.open_issue(), None, "not opened out of the blue");

    // A save waits on a read; it fails, offline say: the text is kept as a
    // draft, which the next `e` on the issue opens, not the editor at once.
    press(&mut app, "e");
    let Some(Effect::Edit { purpose, text }) = app.take_effects().pop() else { panic!() };
    app.on_edited(purpose.clone(), Ok(text.replace("An admin switch", "My switch")));
    assert_eq!(app.take_effects(), [Effect::Send(Command::LoadIssue(key.clone()))]);
    app.on_event(failed(&key));
    assert!(app.take_effects().is_empty(), "no editor over whatever is open");
    assert!(screen(&mut app, 120, 30).contains("kept as a draft"));
    press(&mut app, "e");
    let Some(Effect::Edit { purpose: again, text }) = app.take_effects().pop() else {
        panic!("the draft in the editor")
    };
    assert_eq!(again, purpose, "from where the editing began");
    assert!(text.split("<!-- kolum:").next().unwrap().contains("My switch"), "{text}");
    press(&mut app, "e");
    let Some(Effect::Edit { text, .. }) = app.take_effects().pop() else { panic!() };
    assert!(text.contains("An admin switch"), "the draft opens once: {text}");
}

/// The board's copy has no description on Jira Cloud: the picker's preview
/// takes it from the issue read whole.
#[tokio::test]
async fn the_picker_previews_the_description_read_whole() {
    let fake = FakeProvider::new();
    let mut app = loaded().await;
    let mut snapshot = load_board(&fake, None).await.unwrap();
    for issue in &mut snapshot.issues {
        issue.description = None;
    }
    app.on_event(Event::BoardLoaded { snapshot, fresh: true, request: 1 });
    let issue = fake.issue(&IssueKey::parse("ACME-10")).await.unwrap();
    let detail =
        IssueDetail { issue, comments: vec![], activity: vec![], layout: None, children: vec![] };
    app.on_event(Event::ReadIssues(vec![std::sync::Arc::new(detail)]));
    press(&mut app, "<Space>fACME-10");
    settle(&mut app);
    let shown = screen(&mut app, 150, 40);
    assert!(shown.contains("Offset pagination over a non-unique sort"), "{shown}");
}

#[tokio::test]
async fn a_saved_write_shows_the_providers_version_and_refreshes_the_detail() {
    let mut app = loaded().await;
    press(&mut app, "<CR>");
    app.take_effects();
    let issues = load_board(&FakeProvider::new(), None).await.unwrap().issues;
    let mut issue = issues.into_iter().find(|i| i.key.as_str() == "ACME-9").unwrap();
    issue.title = "As the provider has it".into();
    let key = issue.key.clone();
    app.on_event(Event::Written { op: 9, key: key.clone(), issue: Some(Box::new(issue)) });
    assert!(screen(&mut app, 120, 30).contains("As the provider has it"));
    assert_eq!(app.take_effects(), [Effect::Send(Command::LoadIssue(key))]);
}

#[tokio::test]
async fn a_read_only_provider_is_told_not_tried() {
    let mut settings = settings();
    settings.writable = false;
    let mut app = loaded_with(settings).await;
    app.take_effects();
    for keys in ["L", "t", "<Space>a", "i", "e"] {
        press(&mut app, keys);
        assert!(app.take_effects().is_empty(), "{keys} sent something");
        assert!(screen(&mut app, 120, 30).contains("read-only"), "{keys}");
    }
}

fn bare(issue: kolum_domain::Issue) -> IssueDetail {
    IssueDetail { issue, comments: vec![], activity: vec![], layout: None, children: vec![] }
}

/// `e` on the open issue, its read answered, the editor's text changed by
/// `edit`, and the save sent: a title alone at once, a description once
/// the issue is read again. The write's op.
async fn describe(app: &mut App, edit: impl Fn(&str) -> String) -> u64 {
    let key = IssueKey::parse("ACME-9");
    let read = FakeProvider::new().issue(&key).await.unwrap();
    press(app, "e");
    let asked = app.take_effects();
    if asked.contains(&Effect::Send(Command::LoadIssue(key.clone()))) {
        app.on_event(Event::IssueLoaded(std::sync::Arc::new(bare(read.clone()))));
    }
    let Some(Effect::Edit { purpose, text }) = app.take_effects().pop().or(asked.last().cloned())
    else {
        panic!("expected the editor")
    };
    app.on_edited(purpose, Ok(edit(&text)));
    let mut sent = writes(app.take_effects());
    if sent.is_empty() {
        app.on_event(Event::IssueLoaded(std::sync::Arc::new(bare(read))));
        sent = writes(app.take_effects());
    }
    sent[0].0
}

/// A refused save is kept against the text its editing began from, not a
/// board copy, which on Jira Cloud has no description.
#[tokio::test]
async fn a_refused_description_keeps_the_text_it_was_edited_from() {
    let mut app = loaded().await;
    press(&mut app, "<CR>");
    app.take_effects();
    let op = describe(&mut app, |t| t.replace("An admin switch", "My switch")).await;
    let mut cloud = load_board(&FakeProvider::new(), None).await.unwrap();
    let nine = cloud.issues.iter_mut().find(|i| i.key.as_str() == "ACME-9").unwrap();
    nine.description = None;
    nine.updated += time::Duration::minutes(1);
    app.on_event(Event::BoardLoaded { snapshot: cloud, fresh: true, request: 1 });
    let error = kolum_domain::DomainError::Conflict("stale".into());
    app.on_event(Event::WriteFailed { op, key: IssueKey::parse("ACME-9"), error });
    press(&mut app, "e");
    let Some(Effect::Edit { purpose: EditPurpose::Describe { from, .. }, text }) =
        app.take_effects().pop()
    else {
        panic!("expected the draft")
    };
    assert!(text.contains("My switch"), "{text}");
    assert!(from.contains("An admin switch"), "the text it was edited from: {from}");
}

/// Two saves of one issue refused, in either order: the draft keeps the
/// title typed in one and the description typed in the other.
#[tokio::test]
async fn two_refused_saves_make_one_draft() {
    for title_first in [true, false] {
        let mut app = loaded().await;
        press(&mut app, "<CR>");
        app.take_effects();
        let title = describe(&mut app, |t| t.replacen("# Enforce SSO", "# Require SSO", 1)).await;
        let body = describe(&mut app, |t| t.replace("An admin switch", "My switch")).await;
        let key = IssueKey::parse("ACME-9");
        let ops = if title_first { [title, body] } else { [body, title] };
        for op in ops {
            let error = kolum_domain::DomainError::Conflict("stale".into());
            app.on_event(Event::WriteFailed { op, key: key.clone(), error });
        }
        press(&mut app, "e");
        let Some(Effect::Edit { purpose: EditPurpose::Describe { from, .. }, text }) =
            app.take_effects().pop()
        else {
            panic!("expected the draft")
        };
        assert!(text.contains("# Require SSO") && text.contains("My switch"), "{text}");
        assert!(from.contains("# Enforce SSO") && from.contains("An admin switch"), "{from}");
    }
}

/// The issue read back after a write is the issue whole: it replaces the
/// open one, description included, over a board copy that has none.
#[tokio::test]
async fn a_read_back_replaces_the_open_issue_whole() {
    let mut app = loaded().await;
    press(&mut app, "<CR>");
    let read = FakeProvider::new().issue(&IssueKey::parse("ACME-9")).await.unwrap();
    app.on_event(Event::IssueLoaded(std::sync::Arc::new(bare(read.clone()))));
    press(&mut app, "<Space>A");
    let op = writes(app.take_effects())[0].0;
    let mut saved = read.clone();
    saved.description = Some(kolum_domain::RichText::from_markdown("Rewritten meanwhile"));
    saved.updated += time::Duration::minutes(1);
    let mut cloud = load_board(&FakeProvider::new(), None).await.unwrap();
    let nine = cloud.issues.iter_mut().find(|i| i.key.as_str() == "ACME-9").unwrap();
    (nine.description, nine.updated) = (None, saved.updated);
    app.on_event(Event::BoardLoaded { snapshot: cloud, fresh: true, request: 1 });
    app.on_event(Event::Written { op, key: read.key.clone(), issue: Some(Box::new(saved)) });
    let shown = screen(&mut app, 120, 40);
    assert!(shown.contains("Rewritten meanwhile"), "{shown}");
}

/// A read kept from before is not overwritten by an older board copy when
/// the issue opens.
#[tokio::test]
async fn an_issue_opens_from_a_read_newer_than_the_board() {
    let mut app = loaded().await;
    let mut read = FakeProvider::new().issue(&IssueKey::parse("ACME-9")).await.unwrap();
    read.title = "Renamed since the board".into();
    read.updated += time::Duration::minutes(1);
    app.on_event(Event::ReadIssues(vec![std::sync::Arc::new(bare(read))]));
    press(&mut app, "<CR>");
    assert!(screen(&mut app, 120, 40).contains("Renamed since the board"));
}

/// A description on its way: `e` waits for it rather than open the old
/// one; refused, the text typed is kept as a draft that `e` opens.
#[tokio::test]
async fn a_refused_description_is_kept_as_a_draft_and_e_waits_while_it_is_saved() {
    let fake = FakeProvider::new();
    let mut app = loaded().await;
    let key = IssueKey::parse("ACME-9");
    let read = fake.issue(&key).await.unwrap();
    let detail = |issue| IssueDetail {
        issue,
        comments: vec![],
        activity: vec![],
        layout: None,
        children: vec![],
    };
    press(&mut app, "<CR>");
    app.on_event(Event::IssueLoaded(std::sync::Arc::new(detail(read.clone()))));
    app.take_effects();
    press(&mut app, "e");
    let Some(Effect::Edit { purpose, text }) = app.take_effects().pop() else { panic!() };
    app.on_edited(purpose, Ok(text.replace("An admin switch", "My switch")));
    app.on_event(Event::IssueLoaded(std::sync::Arc::new(detail(read))));
    let op = writes(app.take_effects())[0].0;
    press(&mut app, "e");
    assert!(app.take_effects().is_empty(), "`e` waits for the write");
    assert!(screen(&mut app, 120, 40).contains("ACME-9 is still being saved"));

    let error = kolum_domain::DomainError::Conflict("stale".into());
    app.on_event(Event::WriteFailed { op, key, error });
    assert!(screen(&mut app, 150, 40).contains("kept as a draft"));
    press(&mut app, "e");
    let Some(Effect::Edit { text, .. }) = app.take_effects().pop() else { panic!() };
    assert!(text.contains("My switch"), "{text}");
}

/// A new issue and the fake's board with and without it.
async fn created() -> (kolum_domain::Issue, kolum_app::BoardSnapshot, kolum_app::BoardSnapshot) {
    use kolum_domain::IssueWriter;
    let fake = FakeProvider::new();
    let without = load_board(&fake, None).await.unwrap();
    let template = fake.issue(&IssueKey::parse("ACME-12")).await.unwrap();
    let new = kolum_domain::NewIssue {
        project: "ACME".into(),
        kind: template.kind,
        title: "Later".into(),
        description: template.description.unwrap_or_default(),
        parent: None,
    };
    let created = fake.create(new).await.unwrap();
    (created, without, load_board(&fake, None).await.unwrap())
}

/// A new issue the board's query leaves out is not highlighted later, by
/// a sync long after.
#[tokio::test]
async fn a_new_issue_not_on_the_next_sync_is_forgotten() {
    let mut app = loaded().await;
    let (created, without, with) = created().await;
    app.on_event(Event::Created(Box::new(created)));
    let due = app.deadline().unwrap();
    wait_until(&mut app, due);
    assert!(app.take_effects().contains(&Effect::Send(Command::SyncBoard(None, 2))));
    app.on_event(Event::BoardLoaded { snapshot: without, fresh: true, request: 2 });
    let before = selected(&app).to_string();
    press(&mut app, ":refresh<CR>");
    app.on_event(Event::BoardLoaded { snapshot: with, fresh: true, request: 3 });
    assert_eq!(selected(&app), before);
}

/// A board asked for before the issue was created says nothing of it: the
/// first asked for after does.
#[tokio::test]
async fn a_new_issue_is_highlighted_by_the_first_board_asked_after_it() {
    let mut app = loaded().await;
    let (created, without, with) = created().await;
    press(&mut app, ":refresh<CR>"); // request 2, on its way
    app.on_event(Event::Created(Box::new(created)));
    app.on_event(Event::BoardLoaded { snapshot: without, fresh: true, request: 2 });
    let due = app.deadline().unwrap();
    wait_until(&mut app, due);
    assert!(app.take_effects().contains(&Effect::Send(Command::SyncBoard(None, 3))));
    app.on_event(Event::BoardLoaded { snapshot: with, fresh: true, request: 3 });
    assert_eq!(selected(&app), "ACME-45");
}

/// A board asked for before a write was taken may not hold it: the write
/// shows over it until one asked for after lands, which says where the
/// provider put the card. A read back after a rank does not say.
#[tokio::test]
async fn a_board_asked_before_a_write_was_taken_does_not_undo_it() {
    let mut app = loaded().await;
    press(&mut app, "ll"); // In Progress: ACME-5, 10, 11…
    app.take_effects();
    press(&mut app, "J");
    let op = writes(app.take_effects())[0].0;
    let read = read_back("ACME-5", |_| {}).await;
    app.on_event(Event::Written { op, key: read.key.clone(), issue: Some(Box::new(read)) });
    let before = load_board(&FakeProvider::new(), None).await.unwrap();
    app.on_event(Event::BoardLoaded { snapshot: before.clone(), fresh: true, request: 1 });
    assert_eq!(&column_order(&mut app, 2)[..3], ["ACME-10", "ACME-5", "ACME-11"], "still moved");

    let due = app.deadline().unwrap();
    wait_until(&mut app, due);
    assert!(app.take_effects().contains(&Effect::Send(Command::SyncBoard(None, 2))));
    app.on_event(Event::BoardLoaded { snapshot: before, fresh: true, request: 2 });
    assert_eq!(&column_order(&mut app, 2)[..3], ["ACME-5", "ACME-10", "ACME-11"], "settled");
}

/// Each column keeps its highlight on the same card when a sync moves the
/// cards above it.
#[tokio::test]
async fn a_sync_keeps_each_columns_highlight_on_its_card() {
    let mut app = loaded().await;
    press(&mut app, "jj");
    let held = selected(&app).to_string();
    press(&mut app, "ll");
    let mut synced = load_board(&FakeProvider::new(), None).await.unwrap();
    let done = synced.issues.iter().find(|i| i.key.as_str() == "ACME-8").unwrap().status.clone();
    synced.issues.iter_mut().find(|i| i.key.as_str() == "ACME-9").unwrap().status = done;
    app.on_event(Event::BoardLoaded { snapshot: synced, fresh: true, request: 1 });
    press(&mut app, "hh");
    assert_eq!(selected(&app), held);
}

#[tokio::test]
async fn c_creates_through_the_editor_and_capital_c_clones() {
    use kolum::ui::EditPurpose;
    let mut app = loaded().await;
    app.take_effects();
    press(&mut app, "c");
    let Some(Effect::Edit { purpose, text }) = app.take_effects().pop() else {
        panic!("expected the editor")
    };
    let EditPurpose::Create { project, kind } = &purpose else { panic!("{purpose:?}") };
    assert_eq!(
        (project.as_str(), kind.name.as_str()),
        ("ACME", "Story"),
        "the board's project, the card's type"
    );
    assert!(text.contains("a new Story in ACME"));

    app.on_edited(purpose.clone(), Ok(text.clone()));
    assert!(app.take_effects().is_empty(), "no title: cancelled");
    app.on_edited(purpose, Ok(format!("# Write the release notes\n\nFor *0.1*\n{text}")));
    let Some(Effect::Send(Command::Create(new))) = app.take_effects().pop() else { panic!() };
    assert_eq!(new.title, "Write the release notes");
    assert_eq!(new.description.plain_text(), "For 0.1");

    use kolum_domain::IssueWriter;
    let fake = FakeProvider::new();
    let created = fake.create(new).await.unwrap();
    app.on_event(Event::Created(Box::new(created)));
    assert!(screen(&mut app, 150, 40).contains("Created ACME-45"));
    assert_ne!(selected(&app), "ACME-45", "on the board once the sync says it is");
    let due = app.deadline().unwrap();
    wait_until(&mut app, due);
    assert!(app.take_effects().contains(&Effect::Send(Command::SyncBoard(None, 2))), "soon");
    let snapshot = load_board(&fake, None).await.unwrap();
    app.on_event(Event::BoardLoaded { snapshot, fresh: true, request: 2 });
    assert_eq!(selected(&app), "ACME-45", "the new card is highlighted");

    press(&mut app, "C");
    assert_eq!(app.take_effects(), [Effect::Send(Command::Clone(IssueKey::parse("ACME-45")))]);
}

#[tokio::test]
async fn visual_mode_acts_on_a_range_of_cards() {
    let mut app = loaded().await;
    app.take_effects();
    press(&mut app, "ll");
    press(&mut app, "v2j");
    let shown = screen(&mut app, 150, 50);
    assert!(shown.contains("VISUAL"));
    press(&mut app, "yy");
    assert_eq!(app.take_effects(), [Effect::Copy("ACME-5 ACME-10 ACME-11".into())]);
    assert!(!screen(&mut app, 150, 50).contains("VISUAL"), "an action ends it");

    press(&mut app, "vkL");
    assert_eq!(
        app.take_effects(),
        [Effect::Send(Command::Transitions(IssueKey::parse("ACME-10")))]
    );
    app.on_event(transitions_event("ACME-10").await);
    let sent = writes(app.take_effects());
    assert_eq!(
        sent.iter().map(|w| w.1.as_str()).collect::<Vec<_>>(),
        ["ACME-10", "ACME-11"],
        "both move"
    );
    let picked = kolum_domain::TransitionId::new("to-review");
    assert_eq!(sent[0].2, kolum_app::Write::Transition(picked.clone()));
    let kolum_app::Write::TransitionTo { id, to } = &sent[1].2 else {
        panic!("ACME-11 is in another status: it goes by its own transition: {sent:?}")
    };
    assert_eq!((id, to.name.as_str()), (&picked, "In Review"));

    press(&mut app, "vj<Esc>");
    assert!(!screen(&mut app, 150, 50).contains("VISUAL"));
    press(&mut app, "vjl");
    assert!(!screen(&mut app, 150, 50).contains("VISUAL"), "another column ends it");
    press(&mut app, "vv");
    assert!(!screen(&mut app, 150, 50).contains("VISUAL"), "v again ends it");
}

/// In a selection, a card in the first's status goes by the transition
/// picked, one in another by its own to the same status, and one already
/// there stays, unwritten.
#[tokio::test]
async fn a_selection_moves_each_card_by_what_its_status_allows() {
    let mut app = loaded().await;
    app.take_effects();
    press(&mut app, "llv2jL"); // ACME-5 in progress, ACME-10 blocked, ACME-11 in progress
    app.on_event(transitions_event("ACME-5").await);
    let sent: Vec<_> = writes(app.take_effects()).into_iter().map(|w| (w.1, w.2)).collect();
    let picked = kolum_domain::TransitionId::new("to-review");
    let by_it = kolum_app::Write::Transition(picked.clone());
    let Some((_, kolum_app::Write::TransitionTo { id, .. })) = sent.get(1) else {
        panic!("{sent:?}")
    };
    assert_eq!(id, &picked);
    assert_eq!(
        [&sent[0], &sent[2]],
        [&("ACME-5".to_string(), by_it.clone()), &("ACME-11".to_string(), by_it)],
        "no read for the cards in the first's status"
    );

    let mut app = loaded().await;
    press(&mut app, "llvjt");
    app.on_event(transitions_event("ACME-5").await);
    press(&mut app, "blocked");
    settle(&mut app);
    press(&mut app, "<CR>");
    let sent = writes(app.take_effects());
    assert_eq!(sent.iter().map(|w| w.1.as_str()).collect::<Vec<_>>(), ["ACME-5"], "{sent:?}");
}

#[tokio::test]
async fn swimlanes_group_each_column_and_navigation_follows_them() {
    let mut app = loaded().await;
    press(&mut app, "ll");
    let before = selected(&app).to_string();
    press(&mut app, "s");
    let shown = screen(&mut app, 150, 60);
    assert!(shown.contains("Swimlanes by epic"));
    assert!(shown.contains("── ◆ Billing revamp") && shown.contains("── No epic"), "{shown}");
    assert_eq!(selected(&app), before, "the highlight stays on its card");
    press(&mut app, "gg");
    assert_eq!(selected(&app), "ACME-13", "In Progress starts with the Billing revamp lane");
    press(&mut app, "G");
    let last = selected(&app).to_string();
    assert!(
        ["ACME-10", "ACME-11", "ACME-20", "ACME-21", "ACME-30"].contains(&last.as_str()),
        "{last}: no epic, last"
    );

    press(&mut app, "s");
    assert!(screen(&mut app, 150, 60).contains("── Colin Tenaguillo"));
    press(&mut app, "s");
    assert!(screen(&mut app, 150, 60).contains("── Highest"));
    press(&mut app, "s");
    let off = screen(&mut app, 150, 60);
    assert!(
        !off.contains("── ◆") && !off.contains("── No epic") && !off.contains("── Highest"),
        "and off again"
    );
}

#[tokio::test]
async fn the_detail_shows_the_assignees_avatar() {
    use kolum_domain::UserDirectory;
    let mut settings = settings();
    settings.images = Some(ratatui_image::picker::Picker::halfblocks());
    let mut app = loaded_with(settings).await;
    press(&mut app, "ll"); // ACME-5, assigned to me
    app.take_effects();
    press(&mut app, "<CR>");
    let me = FakeProvider::new().me().await.unwrap();
    let url = me.avatar_url.clone().unwrap();
    assert!(app.take_effects().contains(&Effect::Send(Command::Image(url.clone()))));
    press(&mut app, "q<CR>");
    assert!(
        !app.take_effects().contains(&Effect::Send(Command::Image(url.clone()))),
        "asked for once"
    );

    let before = screen(&mut app, 140, 40);
    let bytes = FakeProvider::new().image(&url).await;
    app.on_event(Event::Image { url: url.clone(), result: bytes });
    let after = screen(&mut app, 140, 40);
    assert_ne!(before, after, "the picture is drawn");
    assert!(after.contains("Colin Tenaguillo"));

    app.on_event(Event::Image { url, result: Ok(b"not an image".to_vec()) });
    screen(&mut app, 140, 40); // a broken picture is no picture, not a crash
}

/// `gi` shows the issue's pictures full screen, one at a time; the keys
/// mean what they mean elsewhere: h/l the one before or after, o the
/// browser, yu the URL, q back to the issue.
#[tokio::test]
async fn gi_views_the_pictures_full_screen() {
    let mut settings = settings();
    settings.images = Some(ratatui_image::picker::Picker::halfblocks());
    let mut app = loaded_with(settings).await;
    press(&mut app, "ll<CR>"); // ACME-5: a screenshot and a log
    app.take_effects();
    press(&mut app, "gi");
    let shown = screen(&mut app, 140, 40);
    assert!(shown.contains("session-expired.png · 47 KB · 1/1"), "{shown}");
    assert!(!shown.contains("callback.log"), "a log is not a picture");
    let waiting = screen(&mut app, 140, 40);
    let bytes = FakeProvider::new().image("fake://shot/5").await;
    app.on_event(Event::Image { url: "fake://shot/5".into(), result: bytes });
    assert_ne!(screen(&mut app, 140, 40), waiting, "the picture is drawn");
    press(&mut app, "l");
    assert!(screen(&mut app, 140, 40).contains("1/1"), "nothing after the last");
    press(&mut app, "o");
    assert_eq!(app.take_effects(), [Effect::Open("fake://shot/5".into())]);
    press(&mut app, "yu");
    assert_eq!(app.take_effects(), [Effect::Copy("fake://shot/5".into())]);
    press(&mut app, "q");
    let back = screen(&mut app, 140, 40);
    assert!(back.contains("callback.log"), "back on the issue: {back}");

    press(&mut app, "qj<CR>gi");
    assert!(screen(&mut app, 140, 40).contains("No pictures in ACME-"), "an issue without any");
}

/// `gp` opens the parent, from the issue or from its card; an epic is not
/// on the board, so it opens once read, in place of the issue.
#[tokio::test]
async fn gp_opens_the_parent() {
    use kolum_domain::IssueReader;
    let fake = FakeProvider::new();
    let parent = fake.issue(&IssueKey::parse("ACME-5")).await.unwrap().parent.unwrap().key;
    let mut app = loaded().await;
    press(&mut app, "ll<CR>"); // ACME-5
    app.take_effects();
    press(&mut app, "gp");
    assert!(app.take_effects().contains(&Effect::Send(Command::LoadIssue(parent.clone()))));
    assert!(screen(&mut app, 140, 40).contains(&format!("Opening {parent}")));
    let issue = fake.issue(&parent).await.unwrap();
    let title = issue.title.clone();
    app.on_event(Event::IssueLoaded(std::sync::Arc::new(IssueDetail {
        issue,
        comments: vec![],
        activity: vec![],
        layout: None,
        children: vec![],
    })));
    let shown = screen(&mut app, 140, 40);
    assert!(shown.contains(&title) && shown.contains(&format!("· {parent}")), "{shown}");

    press(&mut app, "q");
    app.take_effects();
    press(&mut app, "gp");
    let asked = app.take_effects();
    assert!(
        asked.contains(&Effect::Send(Command::LoadIssue(parent.clone()))),
        "from the card: {asked:?}"
    );
    press(&mut app, "<CR>"); // ACME-5 again, before the parent arrives
    let issue = fake.issue(&parent).await.unwrap();
    let late =
        IssueDetail { issue, comments: vec![], activity: vec![], layout: None, children: vec![] };
    app.on_event(Event::IssueLoaded(std::sync::Arc::new(late)));
    assert!(
        screen(&mut app, 140, 40).contains("· ACME-5"),
        "opened since: the parent waits no more"
    );

    let board = load_board(&fake, None).await.unwrap();
    let orphan = board.issues.iter().find(|i| i.parent.is_none()).unwrap().key.clone();
    app.on_event(Event::BoardLoaded { snapshot: board, fresh: true, request: 1 });
    press(&mut app, &format!("q/{orphan}<CR>gp"));
    assert!(screen(&mut app, 140, 40).contains(&format!("{orphan} has no parent")));
}

#[tokio::test]
async fn without_pictures_gi_says_so() {
    let mut app = loaded().await;
    press(&mut app, "ll<CR>gi");
    assert!(screen(&mut app, 140, 40).contains("Pictures are off in this terminal"));
}

#[tokio::test]
async fn without_pictures_no_avatar_is_asked_for() {
    let mut app = loaded().await;
    press(&mut app, "ll");
    app.take_effects();
    press(&mut app, "<CR>");
    assert!(!app.take_effects().iter().any(|e| matches!(e, Effect::Send(Command::Image(_)))));
}

#[tokio::test]
async fn attachments_are_listed_and_the_first_picture_previewed() {
    use kolum_domain::UserDirectory;
    let mut settings = settings();
    settings.images = Some(ratatui_image::picker::Picker::halfblocks());
    let mut app = loaded_with(settings).await;
    press(&mut app, "ll");
    app.take_effects();
    press(&mut app, "<CR>");
    let asked = app.take_effects();
    assert!(asked.contains(&Effect::Send(Command::Image("fake://shot/5".into()))), "{asked:?}");
    assert!(
        !asked.contains(&Effect::Send(Command::Image("fake://file/5".into()))),
        "a log is not fetched"
    );
    let shown = screen(&mut app, 140, 60);
    assert!(shown.contains("▪ session-expired.png  image/png  47 KB"), "{shown}");
    assert!(shown.contains("▪ callback.log  text/plain  1 KB"));

    let bytes = FakeProvider::new().image("fake://shot/5").await;
    app.on_event(Event::Image { url: "fake://shot/5".into(), result: bytes });
    let previewed = screen(&mut app, 140, 60);
    assert_ne!(shown, previewed);
    let title_row = previewed.lines().filter(|l| l.contains("session-expired.png")).count();
    assert_eq!(title_row, 2, "listed on the left and titled over the preview on the right");
}

#[tokio::test]
async fn a_search_hit_that_is_not_on_the_board_still_opens() {
    use kolum_domain::IssueReader;
    let mut app = loaded().await;
    app.take_effects();
    press(&mut app, "<Space>F");
    let due = app.deadline().unwrap();
    wait_until(&mut app, due);
    app.take_effects();
    // An epic: in the project, not a card on the board.
    let fake = FakeProvider::new();
    let epic = fake.issue(&IssueKey::parse("ACME-1")).await.unwrap();
    app.on_event(Event::SearchResults { id: 1, result: Ok(vec![epic.clone()]) });
    settle(&mut app);
    press(&mut app, "<CR>");
    assert_eq!(app.take_effects(), [Effect::Send(Command::LoadIssue(IssueKey::parse("ACME-1")))]);
    assert!(screen(&mut app, 120, 30).contains("Opening ACME-1"));
    let children = kolum_domain::IssueReader::children(&fake, &epic.key).await.unwrap();
    let detail =
        IssueDetail { issue: epic, comments: vec![], activity: vec![], layout: None, children };
    app.on_event(Event::IssueLoaded(std::sync::Arc::new(detail)));
    assert_eq!(app.open_issue().map(IssueKey::as_str), Some("ACME-1"), "opened once loaded");
    let shown = screen(&mut app, 120, 50);
    assert!(shown.contains("Single sign-on") && shown.contains("\u{f0e8} Child issues"), "{shown}");
    assert!(shown.contains("ACME-36"), "its children, found by their parent");
}

#[tokio::test]
async fn the_transitions_picker_leaves_out_the_status_the_issue_is_in() {
    let mut app = loaded().await;
    app.take_effects();
    press(&mut app, "t");
    let mut event = transitions_event("ACME-9").await;
    // Jira's team-managed projects offer a transition to where it is.
    if let Event::Transitions { result: Ok(ts), .. } = &mut event {
        let mut to_itself = ts[0].clone();
        to_itself.to.id = kolum_domain::StatusId::new("backlog");
        to_itself.name = "Backlog".into();
        to_itself.to.name = "Backlog".into();
        ts.push(to_itself);
    }
    app.on_event(event);
    settle(&mut app);
    let shown = screen(&mut app, 150, 40);
    assert!(shown.contains("Move ACME-9") && shown.contains("To Do"), "{shown}");
    assert!(!shown.contains("Backlog "), "not to where it already is: {shown}");
}

/// Every card key on screen.
fn cards_shown(app: &mut App) -> Vec<String> {
    let shown = screen(app, 150, 80);
    let mut keys: Vec<String> = shown
        .split(|c: char| !(c.is_ascii_alphanumeric() || c == '-'))
        .filter(|w| w.starts_with("ACME-"))
        .map(String::from)
        .collect();
    keys.sort();
    keys.dedup();
    keys
}

#[tokio::test]
async fn the_board_filter_takes_the_pickers_prefixes() {
    let all = load_board(&FakeProvider::new(), None).await.unwrap().issues;
    let keys_where = |pred: &dyn Fn(&kolum_domain::Issue) -> bool| {
        let mut keys: Vec<String> =
            all.iter().filter(|i| pred(i)).map(|i| i.key.to_string()).collect();
        keys.sort();
        keys
    };
    let assigned_to = |name: &'static str| {
        move |i: &kolum_domain::Issue| i.assignee.as_ref().is_some_and(|a| a.display_name == name)
    };

    let mut app = loaded().await;
    press(&mut app, "/@me<CR>");
    assert_eq!(
        cards_shown(&mut app),
        keys_where(&assigned_to("Colin Tenaguillo")),
        "only my cards"
    );
    assert!(
        selected(&app).is_empty()
            || keys_where(&assigned_to("Colin Tenaguillo")).contains(&selected(&app).to_string())
    );

    press(&mut app, "/<C-u>@ana<CR>");
    assert_eq!(
        cards_shown(&mut app),
        keys_where(&assigned_to("Ana Silva")),
        "a name, in part and in any case"
    );

    press(&mut app, "/<C-u>@me #bug<CR>");
    let my_bugs = keys_where(&|i| assigned_to("Colin Tenaguillo")(i) && i.kind.name == "Bug");
    assert_eq!(cards_shown(&mut app), my_bugs, "prefixes combine");
    assert!(!my_bugs.is_empty());

    press(&mut app, "/<C-u>@me login<CR>");
    assert_eq!(cards_shown(&mut app), ["ACME-5"], "with text");

    press(&mut app, "/<C-u>l:mobile<CR>");
    assert_eq!(cards_shown(&mut app), keys_where(&|i| i.labels.iter().any(|l| l == "mobile")));

    press(&mut app, "/<C-u>@");
    assert_eq!(cards_shown(&mut app).len(), all.len(), "a lone @ while typing hides nothing");
    press(&mut app, "<Esc>");
    assert_eq!(cards_shown(&mut app).len(), all.len());
}

#[tokio::test]
async fn space_u_picks_one_or_several_people_to_show() {
    let all = load_board(&FakeProvider::new(), None).await.unwrap().issues;
    let of = |names: &[&str]| {
        let mut keys: Vec<String> = all
            .iter()
            .filter(|i| match &i.assignee {
                Some(a) => names.contains(&a.display_name.as_str()),
                None => names.contains(&"nobody"),
            })
            .map(|i| i.key.to_string())
            .collect();
        keys.sort();
        keys
    };
    let mut app = loaded().await;
    press(&mut app, "<Space>u");
    settle(&mut app);
    let menu = screen(&mut app, 150, 50);
    let at = |s: &str| menu.find(s).unwrap_or_else(|| panic!("{s} missing from {menu}"));
    assert!(at("Everyone") < at("Colin Tenaguillo (me)"), "everyone, then me first");
    assert!(at("Colin Tenaguillo (me)") < at("Ana Silva"), "then the others by name");
    assert!(menu.contains("Unassigned"));
    assert!(menu.contains("6 cards"), "with how many each has: {menu}");

    press(&mut app, "ana");
    settle(&mut app);
    press(&mut app, "<Tab><C-w>colin");
    settle(&mut app);
    press(&mut app, "<Tab><CR>");
    let header = screen(&mut app, 150, 50).lines().next().unwrap_or("").to_string();
    assert!(header.contains("/@AnaSilva @me") || header.contains("/@me @AnaSilva"), "{header}");
    assert_eq!(cards_shown(&mut app), of(&["Ana Silva", "Colin Tenaguillo"]), "both, together");

    press(&mut app, "<Space>u");
    settle(&mut app);
    let again = screen(&mut app, 150, 50);
    let shown: Vec<&str> = again.lines().filter(|l| l.contains("· shown")).collect();
    assert_eq!(shown.len(), 2, "the people already shown say so: {again}");
    assert!(
        !again.lines().any(|l| l.contains("+ ")),
        "but are not marked: Enter takes the highlighted one"
    );

    press(&mut app, "<C-w>unassigned");
    settle(&mut app);
    press(&mut app, "<Esc>"); // to normal mode
    press(&mut app, "<Esc>"); // and out: nothing changed
    assert_eq!(cards_shown(&mut app), of(&["Ana Silva", "Colin Tenaguillo"]));

    press(&mut app, "<Space>ueveryone");
    settle(&mut app);
    press(&mut app, "<Tab><CR>");
    assert_eq!(cards_shown(&mut app).len(), all.len(), "everyone again");
    assert!(!screen(&mut app, 150, 50).contains("/@"));
}

#[tokio::test]
async fn enter_in_space_u_takes_the_highlighted_person_even_when_someone_is_shown() {
    let mut app = loaded().await;
    press(&mut app, "/@AnaSilva<CR>");
    press(&mut app, "<Space>ubruno");
    settle(&mut app);
    press(&mut app, "<CR>");
    let header = screen(&mut app, 150, 50).lines().next().unwrap_or("").to_string();
    assert!(
        header.contains("/@BrunoCosta") && !header.contains("Ana"),
        "Bruno instead of Ana: {header}"
    );
}

#[tokio::test]
async fn a_card_has_its_epic_under_the_title_and_its_labels_under_the_epic() {
    let mut app = loaded().await;
    let shown = screen(&mut app, 150, 40);
    let lines: Vec<&str> = shown.lines().collect();
    let row = |text: &str| {
        lines.iter().position(|l| l.contains(text)).unwrap_or_else(|| panic!("{text} in {shown}"))
    };
    // ACME-9, the first card of the Backlog, under the cursor.
    let title = row("┃Enforce SSO per organisation ");
    assert!(lines[title + 1].starts_with("\"┃ "), "two lines kept for the title: {shown}");
    assert_eq!(row("┃◆ Single sign-on"), title + 2, "then the epic, whole");
    assert!(lines[title + 3].contains("┃\u{e0b6}auth\u{e0b4} "), "then the labels: {shown}");
    assert!(
        lines[title + 4].contains("┃■ ACME-9 ↓ ") && lines[title + 4].contains("5 -- "),
        "then the type, key, priority, estimate and assignee: {shown}"
    );
    // A card with no epic keeps its lines where they are.
    let ten = row("▍● ACME-10");
    assert_eq!(row("▍Search API returns"), ten - 4, "an empty epic line between");
    assert!(lines[ten - 1].contains("▍\u{e0b6}backend\u{e0b4} \u{e0b6}api\u{e0b4}"), "{shown}");
    assert!(lines[ten].contains("3 DP"), "{shown}");
}

fn column_order(app: &mut App, column: usize) -> Vec<String> {
    let shown = screen(app, 160, 120);
    shown
        .lines()
        .filter_map(|l| {
            let cell: String = l.chars().skip(column * 32).take(32).collect();
            cell.split(|c: char| !(c.is_ascii_alphanumeric() || c == '-'))
                .find(|w| w.starts_with("ACME-"))
                .map(String::from)
        })
        .collect()
}

#[tokio::test]
async fn shift_j_and_k_move_a_card_within_its_column() {
    use kolum_domain::{IssueKey as K, Rank};
    let mut app = loaded().await;
    press(&mut app, "ll"); // In Progress: ACME-5, 10, 11, 13…
    app.take_effects();
    let start = column_order(&mut app, 2);
    assert_eq!(&start[..3], ["ACME-5", "ACME-10", "ACME-11"]);

    press(&mut app, "J");
    assert_eq!(&column_order(&mut app, 2)[..3], ["ACME-10", "ACME-5", "ACME-11"], "moved at once");
    assert_eq!(selected(&app), "ACME-5", "the highlight follows it");
    let sent = writes(app.take_effects());
    assert_eq!(sent[0].2, kolum_app::Write::Rank(Rank::After(K::parse("ACME-10"))));

    press(&mut app, "K");
    assert_eq!(column_order(&mut app, 2), start);
    assert_eq!(
        writes(app.take_effects())[0].2,
        kolum_app::Write::Rank(Rank::Before(K::parse("ACME-10")))
    );

    press(&mut app, "2J");
    assert_eq!(
        &column_order(&mut app, 2)[..3],
        ["ACME-10", "ACME-11", "ACME-5"],
        "a count jumps that many"
    );
    let op = writes(app.take_effects())[0].0;
    app.on_event(Event::WriteFailed {
        op,
        key: K::parse("ACME-5"),
        error: kolum_domain::DomainError::Conflict("Rank is not configured".into()),
    });
    assert_eq!(column_order(&mut app, 2), start, "a refusal puts the order back");
    assert!(screen(&mut app, 160, 40).contains("Rank is not configured; undone"));

    press(&mut app, "ggK");
    assert!(app.take_effects().is_empty());
    assert!(screen(&mut app, 160, 40).contains("Already at the top"));
}

#[tokio::test]
async fn with_swimlanes_a_card_stays_in_its_lane() {
    let mut app = loaded().await;
    press(&mut app, "lls"); // In Progress by epic: Billing revamp (ACME-13) first
    press(&mut app, "gg");
    assert_eq!(selected(&app), "ACME-13");
    app.take_effects();
    press(&mut app, "J");
    assert!(app.take_effects().is_empty(), "the next card is in another lane");
    assert!(screen(&mut app, 160, 40).contains("Already at the bottom of its lane"));
}

#[tokio::test]
async fn the_command_line_cursor_sits_after_what_was_typed() {
    let mut app = loaded().await;
    for (keys, after) in [(":ab", 3), ("<Left>", 2), ("<Esc>/x", 2)] {
        press(&mut app, keys);
        let mut terminal = Terminal::new(TestBackend::new(80, 10)).unwrap();
        terminal.draw(|f| app.draw(f)).unwrap();
        let last = terminal.backend().buffer().content.chunks(80).last().unwrap();
        let lead = last.iter().position(|c| c.symbol() == ":" || c.symbol() == "/").unwrap();
        let cursor = terminal.get_cursor_position().unwrap();
        assert_eq!((cursor.x, cursor.y), (lead as u16 + after, 9), "after {keys}");
    }
}

#[tokio::test]
async fn the_command_line_proposes_and_tab_walks_through_them() {
    let mut app = loaded().await;
    press(&mut app, ":");
    let shown = screen(&mut app, 100, 30);
    let listed = shown.lines().any(|l| l.contains("refresh ") && l.contains("Reload the board"));
    assert!(listed, "before any key: {shown}");
    press(&mut app, "<Tab><Tab><Tab><Tab>");
    let shown = screen(&mut app, 100, 30);
    assert!(shown.contains(":theme "), "the fourth command is written in");
    assert!(shown.contains("Reload the board"), "the commands stay listed while walking");
    press(&mut app, "<S-Tab>");
    assert!(screen(&mut app, 100, 30).contains(":refresh"));
    press(&mut app, "<C-e>");
    assert!(!screen(&mut app, 100, 30).contains("Reload the board"), "hidden");
    press(&mut app, "<C-u>th");
    let shown = screen(&mut app, 100, 30);
    assert!(shown.contains("Switch theme") && !shown.contains("Reload the board"), "narrowed");
    press(&mut app, "<Tab>");
    let shown = screen(&mut app, 100, 30);
    assert!(shown.contains(":theme ") && shown.contains("gruvbox"), "a lone match goes on");
    press(&mut app, "<Esc>:<Tab><Tab><Tab><Tab><C-y>");
    assert!(screen(&mut app, 100, 30).contains("tokyonight"), "taken, it lists what follows");
}

#[tokio::test]
async fn walking_through_the_themes_previews_them_until_escape_or_enter() {
    let mut app = loaded().await;
    let theme = |name| kolum::theme::Theme::builtin(name).unwrap();
    press(&mut app, ":theme <Tab><Tab>");
    assert_eq!(app.theme(), &theme("light"), "dark, then light");
    press(&mut app, "<S-Tab><S-Tab>");
    assert_eq!(app.theme(), &theme("gruvbox"), "back past the first to the last");
    press(&mut app, "<Esc>");
    assert_eq!(app.theme(), &theme("dark"), "escape puts it back");
    press(&mut app, ":theme <Tab><Tab><Tab><CR>");
    assert_eq!(app.theme(), &theme("kanagawa"), "enter keeps it");
    press(&mut app, ":theme <Tab><Tab><C-u>help<CR>");
    assert_eq!(app.theme(), &theme("kanagawa"), "another command drops the preview");
}

#[tokio::test]
async fn the_command_line_proposals() {
    let mut app = loaded().await;
    press(&mut app, ":<Tab><Tab>");
    insta::assert_snapshot!(screen(&mut app, 100, 14));
}

#[tokio::test]
async fn a_card_stripe_has_the_colour_of_its_priority() {
    let mut app = loaded().await;
    let mut terminal = Terminal::new(TestBackend::new(150, 40)).unwrap();
    terminal.draw(|f| app.draw(f)).unwrap();
    let buffer = terminal.backend().buffer();
    let stripe_of = |key: &str| {
        for y in 0..buffer.area.height {
            let row: Vec<&str> = (0..buffer.area.width).map(|x| buffer[(x, y)].symbol()).collect();
            if let Some(x) = (0..row.len()).find(|&x| row[x..].concat().starts_with(key)) {
                // The key line reads `▍● KEY`: the stripe is three cells before it.
                return buffer[(x as u16 - 3, y)].fg;
            }
        }
        panic!("{key} on the board")
    };
    let theme = kolum::theme::Theme::builtin("dark").unwrap();
    use kolum_domain::Priority;
    assert_eq!(stripe_of("ACME-7 "), theme.priority(Priority::Medium), "a story, medium");
    assert_eq!(stripe_of("ACME-10 "), theme.priority(Priority::High), "a bug, high");
    assert_eq!(stripe_of("ACME-26 "), theme.priority(Priority::Lowest), "a task, lowest");
}

#[tokio::test]
async fn a_card_without_labels_has_no_labels_line() {
    use kolum::config::CardField::*;
    let mut settings = settings();
    settings.ui.card_fields = vec![Type, Key, Priority, Epic, Estimate, Assignee];
    let mut app = loaded_with(settings).await;
    let shown = screen(&mut app, 150, 40);
    let lines: Vec<&str> = shown.lines().collect();
    let row = |text: &str| {
        lines.iter().position(|l| l.contains(text)).unwrap_or_else(|| panic!("{text} in {shown}"))
    };
    let title = row("┃Enforce SSO per");
    assert!(lines[title + 3].contains("┃■ ACME-9 ↓ "), "the key line under the epic: {shown}");
    assert_eq!(row("▍Usage-based pricing"), title + 5, "after one blank line: {shown}");
}

/// The fake's board with GitHub's keys, `repo#12`, and no project of its own.
async fn repository_board() -> App {
    let fake = FakeProvider::new();
    let mut snapshot = load_board(&fake, None).await.unwrap();
    snapshot.board.project = None;
    for (n, issue) in snapshot.issues.iter_mut().enumerate() {
        issue.key = IssueKey::new("python-language-server", "#", n as u64);
    }
    let mut app = App::new(settings(), Arc::new(|| {}));
    app.on_event(Event::BoardLoaded { snapshot, fresh: true, request: 1 });
    app.take_effects();
    app
}

#[tokio::test]
async fn c_on_a_board_without_a_project_creates_in_the_highlighted_cards_repository() {
    use kolum::ui::EditPurpose;
    let mut app = repository_board().await;
    press(&mut app, "c");
    let Some(Effect::Edit { purpose: EditPurpose::Create { project, .. }, .. }) =
        app.take_effects().pop()
    else {
        panic!("expected the editor")
    };
    assert_eq!(project, "python-language-server");
}

#[tokio::test]
async fn the_detail_names_the_repository_an_issue_is_in() {
    let mut app = repository_board().await;
    press(&mut app, "<CR>");
    let key = app.open_issue().cloned().unwrap();
    let mut issue = FakeProvider::new().issue(&IssueKey::parse("ACME-5")).await.unwrap();
    // Read after the board: as new as its copy at least.
    (issue.key, issue.parent, issue.updated) = (key.clone(), None, time::OffsetDateTime::now_utc());
    let detail =
        IssueDetail { issue, comments: vec![], activity: vec![], layout: None, children: vec![] };
    app.on_event(Event::IssueLoaded(std::sync::Arc::new(detail)));
    let shown = screen(&mut app, 150, 40);
    assert!(shown.contains(&format!(" python-language-server · {key}")), "{shown}");
}
