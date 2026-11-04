//! The screen as a user sees it: keys in, a rendered terminal out. The
//! board, the picker, the detail and the help page are pinned as
//! snapshots; the rest is checked on what the keys did.

use kolum::config::{PickerLayout, UiConfig};
use kolum::keymap::{Key, Keymap, parse_keys};
use kolum::ui::{App, Effect, Settings};
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
        ui: UiConfig::default(),
        picker_layout: PickerLayout::Horizontal,
        problems: vec![],
        board: None,
        writable: true,
        images: None,
        remote_debounce: std::time::Duration::from_millis(250),
    }
}

async fn loaded_with(settings: Settings) -> App {
    let fake = FakeProvider::new();
    let mut app = App::new(settings, Arc::new(|| {}));
    assert_eq!(app.take_effects(), [Effect::Send(Command::LoadBoard(None))]);
    app.on_event(Event::Me(fake.me().await.unwrap()));
    app.on_event(Event::BoardLoaded {
        snapshot: load_board(&fake, None).await.unwrap(),
        fresh: true,
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
    assert_eq!(app.take_effects(), [Effect::Send(Command::LoadIssue(IssueKey::new("ACME-29")))]);
}

#[tokio::test]
async fn the_picker_has_a_normal_mode_and_resumes() {
    let mut app = loaded().await;
    press(&mut app, "<Space>fsso");
    settle(&mut app);
    press(&mut app, "<Esc>j");
    let shown = screen(&mut app, 150, 40);
    assert!(shown.contains("NORMAL"), "Esc leaves insert mode, and j moves instead of typing");
    assert!(shown.contains("> sso "), "{shown}");
    press(&mut app, "q");
    assert!(!screen(&mut app, 150, 40).contains("> sso"), "q closes it in normal mode");
    press(&mut app, "<Space>'");
    assert!(screen(&mut app, 150, 40).contains("> sso"), "<leader>' reopens it as it was");
}

#[tokio::test]
async fn the_issue_detail() {
    let mut app = loaded().await;
    press(&mut app, "ll<CR>");
    assert_eq!(app.open_issue().map(IssueKey::as_str), Some("ACME-5"));
    let fake = FakeProvider::new();
    let key = IssueKey::new("ACME-5");
    let detail = IssueDetail {
        issue: fake.issue(&key).await.unwrap(),
        comments: fake.comments(&key).await.unwrap(),
        activity: fake.activity(&key).await.unwrap(),
        layout: None,
    };
    app.on_event(Event::IssueLoaded(Box::new(detail)));
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
    let key = IssueKey::new(key);
    let detail = IssueDetail {
        issue: fake.issue(&key).await.unwrap(),
        comments: fake.comments(&key).await.unwrap(),
        activity: fake.activity(&key).await.unwrap(),
        layout: None,
    };
    app.on_event(Event::IssueLoaded(Box::new(detail)));
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
    assert!(shown.contains("── Description ────"), "a rule heads each section: {shown}");
    assert!(shown.contains("── Acceptance criteria ────"));
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
    let key = IssueKey::new("ACME-10");
    let issue = fake.issue(&key).await.unwrap();
    let detail = IssueDetail { issue, comments: vec![], activity: vec![], layout: None };
    app.on_event(Event::IssueLoaded(Box::new(detail)));
    let mut snapshot = load_board(&fake, None).await.unwrap();
    for issue in &mut snapshot.issues {
        issue.fields.clear();
    }
    app.on_event(Event::BoardLoaded { snapshot, fresh: true });
    assert!(screen(&mut app, 140, 40).contains("Tokens refresh before they expire"));
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
        let mut issue = FakeProvider::new().issue(&IssueKey::new("ACME-10")).await.unwrap();
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
        let detail =
            IssueDetail { issue, comments: vec![], activity: vec![], layout: Some(layout) };
        app.on_event(Event::IssueLoaded(Box::new(detail)));
        screen(&mut app, 140, 60)
    };
    let shown = open(settings()).await;
    let at = |label: &str| shown.find(label).unwrap_or_else(|| panic!("{label}: {shown}"));
    let side = [
        "Status",
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
    assert!(!shown.contains("Reporter") && !shown.contains("Updated"), "only the layout's");
    assert!(!shown.contains("customfield_"), "an empty field goes by its name too: {shown}");
    assert!(!shown.contains("Platforms"), "shown only once filled, and it is empty");
    assert!(at("── Acceptance criteria") < at("── Description"), "the body's order");

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
    let issue = FakeProvider::new().issue(&IssueKey::new("ACME-10")).await.unwrap();
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

    let detail =
        IssueDetail { issue, comments: vec![], activity: vec![], layout: Some(layout("Team now")) };
    app.on_event(Event::IssueLoaded(Box::new(detail)));
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
    assert!(screen(&mut app, 150, 40).contains("> webhook"), "the latest first");
    press(&mut app, "<Up><Up>");
    assert!(screen(&mut app, 150, 40).contains("> stripe"), "and no further than the oldest");
    press(&mut app, "<Down>");
    assert!(screen(&mut app, 150, 40).contains("> webhook"));
    press(&mut app, "<Down>");
    assert!(screen(&mut app, 150, 40).contains("> \u{20}"), "past the newest: empty again");
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
    assert_eq!(app.take_effects(), [Effect::Send(Command::Transitions(IssueKey::new("ACME-29")))]);
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
    assert!(screen(&mut app, 120, 40).contains("Keys  (j/k"), "it ran `help`");
}

#[tokio::test]
async fn the_command_line_reports_what_it_cannot_do() {
    let mut app = loaded().await;
    press(&mut app, ":frobnicate<CR>");
    assert!(screen(&mut app, 100, 20).contains("Not a command: frobnicate"));
    press(&mut app, ":jql project = X<CR>");
    let shown = screen(&mut app, 100, 20);
    assert!(
        shown.contains("> !project = X") && shown.contains("[remote"),
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
        [Effect::Send(Command::ReloadBoard(Some(kolum_domain::BoardId::new("1"))))]
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
    });
    assert_eq!(selected(&app), before);
    app.on_event(Event::Failed {
        what: "loading the board".into(),
        error: kolum_domain::DomainError::Transport("timed out".into()),
    });
    assert!(screen(&mut app, 120, 30).contains("loading the board failed: transport: timed out"));
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
    screen(&mut app, 150, 40); // 5 cards a column fit: a half page is 2
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
    assert!(typed.contains("> login") && typed.contains("⠋ searching…"), "{typed}");
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
    use kolum_domain::{FieldId, FieldValue, IssueUpdate, RichText};
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
        fields: vec![(FieldId::new("customfield_10030"), Some(FieldValue::RichText(rotate)))],
        ..Default::default()
    };
    let sent = writes(app.take_effects());
    let op = sent[0].0;
    let sent: Vec<_> = sent.into_iter().map(|(_, k, w)| (k, w)).collect();
    assert_eq!(sent, [("ACME-10".to_string(), kolum_app::Write::Update(update))]);
    assert!(screen(&mut app, 140, 40).contains("Tokens rotate before they expire"), "at once");
    let key = IssueKey::new("ACME-10");
    let error = kolum_domain::DomainError::Conflict("stale".into());
    app.on_event(Event::WriteFailed { op, key: key.clone(), error });
    assert_eq!(app.take_effects(), [Effect::Send(Command::LoadIssue(key))], "read back");

    // A field Jira had to flatten says so before it is overwritten.
    let mut issue = FakeProvider::new().issue(&IssueKey::new("ACME-10")).await.unwrap();
    for field in &mut issue.fields {
        if let FieldValue::RichText(text) = &mut field.value {
            text.lossy = true;
        }
    }
    app.on_event(Event::IssueLoaded(Box::new(IssueDetail {
        issue,
        comments: vec![],
        activity: vec![],
        layout: None,
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

/// What the provider says the issue lets kolum set, as the service would.
async fn editable(key: &str) -> Event {
    use kolum_domain::IssueWriter;
    let key = IssueKey::new(key);
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
    assert_eq!(asked, [Effect::Send(Command::EditableFields(IssueKey::new("ACME-10")))]);
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
    let team = (FieldId::new("customfield_10020"), Some(FieldValue::Text("Mobile".into())));
    assert_eq!(updates(&mut app), [IssueUpdate { fields: vec![team], ..Default::default() }]);

    edit_field(&mut app, "story").await;
    assert!(screen(&mut app, 140, 40).contains("Story points: "), "the bottom line");
    press(&mut app, "5<CR>");
    let points = (FieldId::new("customfield_10016"), Some(FieldValue::Number(5.0)));
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
    let [(id, Some(FieldValue::User(ana)))] = &fields[..] else { panic!("{fields:?}") };
    assert_eq!((id.as_str(), ana.display_name.as_str()), ("customfield_10050", "Ana Silva"));

    edit_field(&mut app, "platforms").await;
    press(&mut app, "<Tab><Tab><CR>");
    let both = FieldValue::Options(vec!["Web".into(), "iOS".into()]);
    let platforms = (FieldId::new("customfield_10060"), Some(both));
    assert_eq!(updates(&mut app), [IssueUpdate { fields: vec![platforms], ..Default::default() }]);

    edit_field(&mut app, "team").await;
    press(&mut app, "none");
    settle(&mut app);
    press(&mut app, "<CR>");
    let emptied = (FieldId::new("customfield_10020"), None);
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
    let sprint = (FieldId::new("customfield_10040"), Some(FieldValue::Number(15.0)));
    assert_eq!(updates(&mut app), [IssueUpdate { fields: vec![sprint], ..Default::default() }]);
    let error = kolum_domain::DomainError::Transport("kanban".into());
    app.on_event(Event::Sprints(Err(error.clone())));
    assert!(!screen(&mut app, 140, 40).contains("kanban"), "nothing asked for them");
    edit_field(&mut app, "sprint").await;
    app.on_event(Event::Sprints(Err(error)));
    assert!(screen(&mut app, 140, 40).contains("The board's sprints: transport: kanban"));

    press(&mut app, "E");
    let key = IssueKey::new("ACME-10");
    let error = kolum_domain::DomainError::Transport("timed out".into());
    app.on_event(Event::EditableFields { key, result: Err(error) });
    assert!(screen(&mut app, 140, 40).contains("Reading the fields of ACME-10: transport"));

    edit_field(&mut app, "status").await;
    let asked = Effect::Send(Command::Transitions(IssueKey::new("ACME-10")));
    assert!(app.take_effects().contains(&asked), "the transitions, as t asks");
}

/// A provider that cannot list its fields still lets `E` edit the
/// description and the status.
#[tokio::test]
async fn e_falls_back_to_the_description_and_the_status() {
    let mut app = loaded().await;
    press(&mut app, "/ACME-10<CR><CR>E");
    let key = IssueKey::new("ACME-10");
    let unsupported =
        kolum_domain::DomainError::Unsupported(kolum_domain::Capability::CustomFields);
    app.on_event(Event::EditableFields { key, result: Err(unsupported) });
    settle(&mut app);
    let shown = screen(&mut app, 140, 40);
    assert!(shown.contains("Status") && shown.contains("Description"), "{shown}");
    assert!(!shown.contains("Due date"), "only what every provider has");
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
        [Effect::Send(Command::LoadBoard(Some(kolum_domain::BoardId::new("1"))))]
    );
    press(&mut app, ":board 7<CR>");
    assert_eq!(
        app.take_effects(),
        [Effect::Send(Command::LoadBoard(Some(kolum_domain::BoardId::new("7"))))]
    );
}

#[tokio::test]
async fn a_cached_board_says_so_until_the_sync_lands() {
    let fake = FakeProvider::new();
    let snapshot = load_board(&fake, None).await.unwrap();
    let mut app = App::new(settings(), Arc::new(|| {}));
    app.on_event(Event::BoardLoaded { snapshot: snapshot.clone(), fresh: false });
    assert!(screen(&mut app, 150, 40).contains("cached, syncing…"));
    app.on_event(Event::BoardLoaded { snapshot, fresh: true });
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
    let key = IssueKey::new(key);
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
    assert_eq!(app.take_effects(), [Effect::Send(Command::Transitions(IssueKey::new("ACME-9")))]);
    app.on_event(transitions_event("ACME-9").await);
    let sent = writes(app.take_effects());
    assert_eq!(sent.len(), 1, "one transition leads to To Do: no question asked");
    assert_eq!(sent[0].2, kolum_app::Write::Transition(kolum_domain::TransitionId::new("to-todo")));
    assert_eq!(column_of(&mut app, "ACME-9"), 1, "moved before the provider answered");
    assert_eq!(selected(&app), "ACME-9", "the highlight follows the card");

    let error = kolum_domain::DomainError::Conflict("workflow changed".into());
    app.on_event(Event::WriteFailed { op: sent[0].0, key: IssueKey::new("ACME-9"), error });
    assert_eq!(column_of(&mut app, "ACME-9"), 0, "undone");
    assert!(screen(&mut app, 150, 60).contains("ACME-9: conflict: workflow changed; undone"));
}

#[tokio::test]
async fn a_move_into_a_column_of_two_statuses_asks_which() {
    let mut app = loaded().await;
    app.take_effects();
    press(&mut app, "jL"); // ACME-16 is in the Backlog; To Do first
    app.on_event(transitions_event("ACME-16").await);
    let first = writes(app.take_effects());
    app.on_event(Event::Written { op: first[0].0, key: IssueKey::new("ACME-16"), issue: None });
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
    app.on_event(Event::Transitions { key: IssueKey::new("ACME-9"), result: Ok(vec![]) });
    assert!(screen(&mut app, 150, 30).contains("No transition takes ACME-9 to To Do from here"));
    press(&mut app, "t");
    app.on_event(Event::Transitions { key: IssueKey::new("ACME-1"), result: Ok(vec![]) });
    assert!(
        !screen(&mut app, 150, 30).contains("No transition"),
        "a reply for another issue is ignored"
    );
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
    assert_eq!(purpose, EditPurpose::Comment(IssueKey::new("ACME-9")));
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
    press(&mut app, "e");
    let Some(Effect::Edit { purpose, text }) = app.take_effects().pop() else { panic!() };
    assert_eq!(purpose, EditPurpose::Describe(IssueKey::new("ACME-9")));
    assert!(text.starts_with("# Enforce SSO per organisation\n\nAn admin switch"), "{text}");

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

    app.on_edited(purpose, Ok("# Require SSO per organisation\n\nNew *body*".into()));
    let sent = writes(app.take_effects());
    let kolum_app::Write::Update(update) = &sent[0].2 else { panic!() };
    assert_eq!(
        (update.title.as_ref(), update.description.as_ref().map(|d| d.plain_text())),
        (None, Some("New body".into()))
    );
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
    let created = FakeProvider::new().create(new).await.unwrap();
    app.on_event(Event::Created(Box::new(created)));
    assert_eq!(selected(&app), "ACME-45", "the new card is highlighted");
    assert!(screen(&mut app, 150, 40).contains("Created ACME-45"));

    press(&mut app, "C");
    assert_eq!(app.take_effects(), [Effect::Send(Command::Clone(IssueKey::new("ACME-45")))]);
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
    assert_eq!(app.take_effects(), [Effect::Send(Command::Transitions(IssueKey::new("ACME-10")))]);
    app.on_event(transitions_event("ACME-10").await);
    let sent = writes(app.take_effects());
    assert_eq!(
        sent.iter().map(|w| w.1.as_str()).collect::<Vec<_>>(),
        ["ACME-10", "ACME-11"],
        "both move"
    );

    press(&mut app, "vj<Esc>");
    assert!(!screen(&mut app, 150, 50).contains("VISUAL"));
    press(&mut app, "vjl");
    assert!(!screen(&mut app, 150, 50).contains("VISUAL"), "another column ends it");
    press(&mut app, "vv");
    assert!(!screen(&mut app, 150, 50).contains("VISUAL"), "v again ends it");
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
    let epic = fake.issue(&IssueKey::new("ACME-1")).await.unwrap();
    app.on_event(Event::SearchResults { id: 1, result: Ok(vec![epic.clone()]) });
    settle(&mut app);
    press(&mut app, "<CR>");
    assert_eq!(app.take_effects(), [Effect::Send(Command::LoadIssue(IssueKey::new("ACME-1")))]);
    assert!(screen(&mut app, 120, 30).contains("Opening ACME-1"));
    let detail = IssueDetail { issue: epic, comments: vec![], activity: vec![], layout: None };
    app.on_event(Event::IssueLoaded(Box::new(detail)));
    assert_eq!(app.open_issue().map(IssueKey::as_str), Some("ACME-1"), "opened once loaded");
    assert!(screen(&mut app, 120, 30).contains("Single sign-on"));
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
    // ACME-9, the first card of the Backlog.
    let key = row("■ ACME-9");
    assert_eq!(row("Enforce SSO per"), key + 1, "the title under the key");
    assert_eq!(row("organisation"), key + 2, "on two lines");
    assert_eq!(row("┃◆ Single sign-on"), key + 3, "then the epic, whole");
    assert!(
        lines[key + 4].contains("┃ auth ") && lines[key + 4].contains("5 --┃"),
        "then the labels, estimate and assignee"
    );
    // A card with no epic keeps its lines where they are.
    let ten = row("● ACME-10");
    assert!(
        lines[ten + 4].contains(" backend ") && lines[ten + 4].contains("3 DP"),
        "labels stay on the last line"
    );
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
    assert_eq!(sent[0].2, kolum_app::Write::Rank(Rank::After(K::new("ACME-10"))));

    press(&mut app, "K");
    assert_eq!(column_order(&mut app, 2), start);
    assert_eq!(
        writes(app.take_effects())[0].2,
        kolum_app::Write::Rank(Rank::Before(K::new("ACME-10")))
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
        key: K::new("ACME-5"),
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
    assert!(screen(&mut app, 100, 30).contains("refresh  Reload the board"), "before any key");
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
