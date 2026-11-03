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
    }
}

async fn loaded_with(settings: Settings) -> App {
    let fake = FakeProvider::new();
    let mut app = App::new(settings, Arc::new(|| {}));
    assert_eq!(app.take_effects(), [Effect::Send(Command::LoadBoard(None))]);
    app.on_event(Event::Me(fake.me().await.unwrap()));
    app.on_event(Event::BoardLoaded(load_board(&fake, None).await.unwrap()));
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
    press(&mut app, "L");
    assert!(screen(&mut app, 120, 30).contains("`move_card_right` is not available yet"));
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

#[tokio::test]
async fn the_detail_lists_links_and_configured_custom_fields() {
    let mut settings = settings();
    settings.ui.detail_fields =
        vec!["status".into(), "customfield_10020".into(), "Story points".into()];
    let mut app = loaded_with(settings).await;
    press(&mut app, "ll<CR>");
    press(&mut app, ":q<CR>");
    assert!(app.quit, ":q quits from anywhere");

    let mut settings = self::settings();
    settings.ui.detail_fields =
        vec!["status".into(), "customfield_10020".into(), "Story points".into()];
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
    press(&mut app, "<C-t>");
    assert!(screen(&mut app, 150, 40).contains("`transition` is not available yet"));
    press(&mut app, "<C-d><C-u><Esc>Gggkj");
    press(&mut app, "<C-Space>");
    assert!(screen(&mut app, 150, 40).contains("Remote search comes with the Jira provider"));
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
    assert!(screen(&mut app, 100, 20).contains(":jql is not available yet"));
    press(&mut app, ":theme<CR>");
    assert!(screen(&mut app, 100, 20).contains(" Themes "), ":theme alone opens the picker");
    press(&mut app, "<Esc><Esc>:<Esc>");
    app.take_effects();
    press(&mut app, ":refresh<CR>");
    assert_eq!(
        app.take_effects(),
        [Effect::Send(Command::LoadBoard(Some(kolum_domain::BoardId::new("1"))))]
    );
}

#[tokio::test]
async fn a_refresh_keeps_the_highlight_on_the_same_issue() {
    let mut app = loaded().await;
    press(&mut app, "ll3j");
    let before = selected(&app).to_string();
    app.on_event(Event::BoardLoaded(load_board(&FakeProvider::new(), None).await.unwrap()));
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
    press(&mut app, "ze");
    let shown = screen(&mut app, 150, 30);
    assert!(!shown.contains("No cards") && shown.contains("IN REVIEW"), "{shown}");
    assert_eq!(selected(&app), "ACME-29");
}

#[tokio::test]
async fn half_pages_move_by_what_is_on_screen() {
    let mut app = loaded().await;
    press(&mut app, "ll");
    screen(&mut app, 150, 40); // 6 cards a column fit: a half page is 3
    press(&mut app, "<C-d>");
    assert_eq!(selected(&app), "ACME-13");
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
    assert_eq!(shown.matches("dark").count(), 1, "a file named like a built-in is not listed twice");
    press(&mut app, "zebra");
    settle(&mut app);
    press(&mut app, "<CR>");
    assert_eq!(app.theme().accent, ratatui::style::Color::Rgb(0x12, 0x34, 0x56), "confirmed, not just previewed");
    assert!(!screen(&mut app, 120, 40).contains(" Themes "));
    std::fs::remove_dir_all(dir).unwrap();
}
