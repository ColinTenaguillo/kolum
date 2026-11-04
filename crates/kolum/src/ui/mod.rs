//! The screen: what has the focus, what a key does there, and drawing it.
//!
//! [`App`] is a state machine with no I/O. Keys and service events go in;
//! drawing and a list of [`Effect`]s for the event loop come out. That is
//! what lets the tests drive it exactly as the binary does.

pub mod board;
mod detail;
mod glyph;
mod help;
pub mod input;
pub mod picker;
mod rich;
mod sources;
mod text;

use crate::config::{PickerLayout, UiConfig};
use crate::keymap::{Action, Context, Key, Keymap, Layers, Outcome, Resolver};
use crate::theme::{self, Theme};
use board::{BoardView, DrawCtx};
use detail::DetailView;
use input::Input;
use kolum_app::{Command, Event};
use kolum_domain::{IssueKey, User};
use picker::{AnyPicker, Pick, Picker, Waker};
use ratatui::Frame;
use ratatui::layout::{Constraint, Flex, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Clear, Paragraph, Wrap};
use std::cell::Cell;
use std::path::PathBuf;
use std::time::Instant;
use text::width;

/// What the event loop does on the app's behalf.
#[derive(Debug, Clone, PartialEq)]
pub enum Effect {
    Send(Command),
    Copy(String),
    Open(String),
    /// The board toggles changed: keep them for the next run.
    SaveState(crate::config::UiState),
    /// Hand the terminal to `$EDITOR` with this text, and give the result
    /// to [`App::on_edited`].
    Edit {
        purpose: EditPurpose,
        text: String,
    },
}

/// What an `$EDITOR` session was for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EditPurpose {
    Comment(IssueKey),
    /// The title and the description, as `# Title` then Markdown.
    Describe(IssueKey),
    /// A new issue of this type in this project.
    Create {
        project: String,
        kind: kolum_domain::IssueType,
    },
}

/// Everything from here down is instructions, not content.
const MARKER: &str = "<!-- kolum:";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    Info,
    Warn,
    Error,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PromptKind {
    Command,
    Filter,
}

struct Prompt {
    kind: PromptKind,
    input: Input,
}

/// What has the focus, topmost first.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Focus {
    Problems,
    Help,
    Picker,
    Prompt,
    Detail,
    Board,
}

/// Everything the app is built from, read from the config by the binary.
pub struct Settings {
    pub keymap: Keymap,
    pub theme_name: String,
    pub themes_dir: Option<PathBuf>,
    pub theme_overrides: toml::Table,
    pub ui: UiConfig,
    pub picker_layout: PickerLayout,
    /// Shown in a box at start until a key is pressed.
    pub problems: Vec<String>,
    /// The board to open; the provider's first when `None`.
    pub board: Option<kolum_domain::BoardId>,
    /// The provider takes writes. When it does not, write actions say so
    /// instead of failing at the provider.
    pub writable: bool,
    /// How this terminal draws pictures; `None` draws none.
    pub images: Option<ratatui_image::picker::Picker>,
    /// How long typing must pause before a remote search goes out.
    pub remote_debounce: std::time::Duration,
}

pub struct App {
    keymap: Keymap,
    resolver: Resolver,
    theme: Theme,
    theme_name: String,
    themes_dir: Option<PathBuf>,
    theme_overrides: toml::Table,
    /// The theme in force before the theme picker began previewing.
    theme_before_preview: Option<(String, Theme)>,
    ui: UiConfig,
    picker_layout: PickerLayout,
    me: Option<User>,
    board: BoardView,
    detail: Option<DetailView>,
    picker: Option<Box<dyn AnyPicker>>,
    last_picker: Option<Box<dyn AnyPicker>>,
    history: Vec<String>,
    history_pos: Option<usize>,
    help: Option<u16>,
    help_page: Cell<u16>,
    prompt: Option<Prompt>,
    problems: Vec<String>,
    message: Option<(String, Level)>,
    effects: Vec<Effect>,
    waker: Waker,
    /// The board on screen is the cached copy; the sync is still out.
    board_cached: bool,
    /// The latest remote search sent; replies to any other are stale.
    search_id: u64,
    /// When the pending remote search goes out, once typing pauses.
    search_due: Option<Instant>,
    remote_debounce: std::time::Duration,
    /// The time of the key being handled, for the debounce.
    now: Instant,
    /// Changes shown before the provider confirmed them.
    ledger: kolum_app::Ledger,
    /// Transitions asked for these issues; `Some(column)` when they are
    /// for a move into that column.
    awaiting: Option<(Vec<IssueKey>, Option<usize>)>,
    writable: bool,
    /// An issue asked to be opened that is not on the board: it opens when
    /// it has been read.
    opening: Option<IssueKey>,
    images: Option<ratatui_image::picker::Picker>,
    /// Avatars by URL: asked for, drawn, or not to be had.
    avatars: std::collections::HashMap<String, Avatar>,
    pub quit: bool,
}

/// A picture ready to draw, if it is.
fn ready(p: Option<&mut Avatar>) -> Option<&mut ratatui_image::protocol::StatefulProtocol> {
    match p {
        Some(Avatar::Ready(protocol)) => Some(protocol.as_mut()),
        _ => None,
    }
}

enum Avatar {
    Loading,
    Ready(Box<ratatui_image::protocol::StatefulProtocol>),
    Failed,
}

impl App {
    /// Asks for the board straight away.
    pub fn new(settings: Settings, waker: Waker) -> Self {
        let mut problems = settings.problems;
        let theme = match Theme::load(&settings.theme_name, settings.themes_dir.as_deref()) {
            Ok((mut theme, mut p)) => {
                p.extend(theme.apply(&settings.theme_overrides, "[theme.overrides]"));
                problems.extend(p);
                theme
            }
            Err(e) => {
                problems.push(format!("ui.theme: {e}"));
                Theme::builtin("dark").expect("dark is built in")
            }
        };
        let mut board = BoardView::default();
        board.hide_empty = settings.ui.hide_empty_columns;
        board.swimlanes = settings.ui.swimlanes;
        App {
            keymap: settings.keymap,
            resolver: Resolver::default(),
            theme,
            theme_name: settings.theme_name,
            themes_dir: settings.themes_dir,
            theme_overrides: settings.theme_overrides,
            theme_before_preview: None,
            ui: settings.ui,
            picker_layout: settings.picker_layout,
            me: None,
            board,
            detail: None,
            picker: None,
            last_picker: None,
            history: vec![],
            history_pos: None,
            help: None,
            help_page: Cell::new(10),
            prompt: None,
            problems,
            message: None,
            effects: vec![Effect::Send(Command::LoadBoard(settings.board))],
            waker,
            board_cached: false,
            search_id: 0,
            search_due: None,
            remote_debounce: settings.remote_debounce,
            now: Instant::now(),
            ledger: kolum_app::Ledger::default(),
            awaiting: None,
            writable: settings.writable,
            opening: None,
            images: settings.images,
            avatars: std::collections::HashMap::new(),
            quit: false,
        }
    }

    pub fn take_effects(&mut self) -> Vec<Effect> {
        std::mem::take(&mut self.effects)
    }

    /// The next moment something happens without a key: a sequence
    /// times out, or a debounced search goes out.
    pub fn deadline(&self) -> Option<Instant> {
        [self.resolver.deadline(), self.search_due].into_iter().flatten().min()
    }

    pub fn theme(&self) -> &Theme {
        &self.theme
    }

    /// The highlighted card on the board.
    pub fn selected_issue(&self) -> Option<&IssueKey> {
        self.board.selected().map(|i| &i.key)
    }

    /// The issue open in the detail view.
    pub fn open_issue(&self) -> Option<&IssueKey> {
        self.detail.as_ref().map(|d| &d.issue.key)
    }

    fn focus(&self) -> Focus {
        if !self.problems.is_empty() {
            Focus::Problems
        } else if self.help.is_some() {
            Focus::Help
        } else if self.picker.is_some() {
            Focus::Picker
        } else if self.prompt.is_some() {
            Focus::Prompt
        } else if self.detail.is_some() {
            Focus::Detail
        } else {
            Focus::Board
        }
    }

    /// The contexts in force, most specific first, and whether keys type.
    fn contexts(&self) -> (Vec<Context>, bool) {
        match self.focus() {
            Focus::Problems => (vec![], false),
            Focus::Help => (vec![Context::Help, Context::Global], false),
            Focus::Picker if self.picker.as_ref().is_some_and(|p| p.is_insert()) => {
                (vec![Context::PickerInsert, Context::Picker], true)
            }
            Focus::Picker => (vec![Context::Picker], false),
            Focus::Prompt => (vec![Context::Command], true),
            Focus::Detail => (vec![Context::Detail, Context::Global], false),
            Focus::Board if self.board.visual.is_some() => {
                (vec![Context::Visual, Context::Board, Context::Global], false)
            }
            Focus::Board => (vec![Context::Board, Context::Global], false),
        }
    }

    pub fn on_key(&mut self, key: Key, now: Instant) {
        if !self.problems.is_empty() {
            self.problems.clear();
            return;
        }
        self.now = now;
        if self.resolver.is_idle() {
            self.message = None;
        }
        let (contexts, text) = self.contexts();
        let layers =
            Layers { tables: contexts.iter().map(|&c| self.keymap.table(c)).collect(), text };
        let outcome = self.resolver.feed(key, &layers, now, self.keymap.timeout);
        self.apply(outcome);
    }

    pub fn on_timeout(&mut self, now: Instant) {
        self.now = now;
        if self.search_due.is_some_and(|due| now >= due) {
            self.search();
        }
        let (contexts, text) = self.contexts();
        let layers =
            Layers { tables: contexts.iter().map(|&c| self.keymap.table(c)).collect(), text };
        if let Some(outcome) = self.resolver.on_timeout(&layers, now) {
            self.apply(outcome);
        }
    }

    fn apply(&mut self, outcome: Outcome) {
        match outcome {
            Outcome::Fire { action, count } => self.dispatch(action, count),
            Outcome::Unhandled(key) => self.type_key(key),
            Outcome::Pending | Outcome::Dropped => {}
        }
        self.preview_hovered_theme();
    }

    /// A key no binding took, for whichever text field has the focus.
    fn type_key(&mut self, key: Key) {
        if let Some(picker) = &mut self.picker {
            if picker.is_insert() {
                picker.type_key(key);
                self.history_pos = None;
                if picker.is_remote() {
                    self.search_due = Some(self.now + self.remote_debounce);
                }
            }
        } else if let Some(prompt) = &mut self.prompt
            && prompt.input.handle(key)
            && prompt.kind == PromptKind::Filter
        {
            self.board.filter = prompt.input.text().to_string();
            self.board.clamp();
        }
    }

    pub fn on_event(&mut self, event: Event) {
        match event {
            Event::Me(user) => {
                self.board.me = Some(user.id.clone());
                self.me = Some(user);
                self.board.clamp();
            }
            Event::BoardLoaded { snapshot, fresh } => {
                self.board_cached = !fresh;
                if let Some(d) = &mut self.detail
                    && let Some(fresh) = snapshot.issues.iter().find(|i| i.key == d.issue.key)
                {
                    d.issue = fresh.clone();
                }
                self.board.set_snapshot(snapshot);
            }
            Event::IssueLoaded(detail)
                if self.detail.is_none() && self.opening.as_ref() == Some(&detail.issue.key) =>
            {
                self.opening = None;
                let mut view = DetailView::new(detail.issue.clone());
                view.loaded = Some(*detail);
                self.detail = Some(view);
                self.want_avatar();
            }
            Event::IssueLoaded(detail) => {
                if let Some(d) = &mut self.detail
                    && d.issue.key == detail.issue.key
                {
                    d.issue = detail.issue.clone();
                    d.loaded = Some(*detail);
                    self.want_avatar();
                }
            }
            Event::Boards(boards) => {
                let source = sources::BoardChoice { boards };
                self.open_picker(Box::new(Picker::new(source, self.waker.clone())));
            }
            Event::SearchResults { id, result } => {
                let latest = self.search_id;
                let Some(p) = self.picker.as_mut().filter(|p| p.is_remote() && id == latest) else {
                    return;
                };
                match result {
                    Ok(issues) => p.receive(issues),
                    Err(error) => {
                        p.set_loading(false);
                        self.say(format!("search failed: {error}"), Level::Error);
                    }
                }
            }
            Event::Transitions { key, result } => self.on_transitions(key, result),
            Event::Written { op, key, issue } => {
                self.ledger.settle(op);
                if let (Some(snapshot), Some(issue)) = (&mut self.board.snapshot, issue) {
                    snapshot.merge(vec![*issue]);
                    self.board.clamp();
                }
                self.sync_detail(&key);
                if self.open_issue() == Some(&key) {
                    self.effects.push(Effect::Send(Command::LoadIssue(key.clone())));
                }
                self.say(format!("{key} saved"), Level::Info);
            }
            Event::WriteFailed { op, key, error } => {
                if let Some(snapshot) = &mut self.board.snapshot {
                    self.ledger.rollback(snapshot, op);
                    self.board.clamp();
                }
                self.sync_detail(&key);
                self.say(format!("{key}: {error}; undone"), Level::Error);
            }
            Event::Image { url, result } => {
                let decoded = result.map_err(|e| e.to_string()).and_then(|bytes| {
                    let picker = self.images.as_ref().ok_or("no pictures here")?;
                    let image = image::load_from_memory(&bytes).map_err(|e| e.to_string())?;
                    Ok(picker.new_resize_protocol(image))
                });
                let avatar = match decoded {
                    Ok(protocol) => Avatar::Ready(Box::new(protocol)),
                    Err(e) => {
                        tracing::info!("avatar {url}: {e}");
                        Avatar::Failed
                    }
                };
                self.avatars.insert(url, avatar);
            }
            Event::Created(issue) => {
                let key = issue.key.clone();
                if let Some(snapshot) = &mut self.board.snapshot {
                    snapshot.merge(vec![*issue]);
                    self.board.clamp();
                    self.board.select(&key);
                }
                self.say(format!("Created {key}"), Level::Info);
            }
            Event::Failed { what, error } => {
                self.say(format!("{what} failed: {error}"), Level::Error)
            }
        }
    }

    /// Lets the picker take in finished matches. Called before each draw.
    pub fn tick(&mut self) {
        if let Some(p) = &mut self.picker {
            p.tick();
        }
        self.preview_hovered_theme();
    }

    fn say(&mut self, text: impl Into<String>, level: Level) {
        self.message = Some((text.into(), level));
    }

    fn unavailable(&mut self, action: Action) {
        self.say(format!("`{}` is not available yet", action.name()), Level::Warn);
    }

    fn dispatch(&mut self, action: Action, count: Option<u32>) {
        let n = i64::from(count.unwrap_or(1));
        match action {
            Action::Quit => self.quit = true,
            Action::Close => self.close(),
            Action::Help => self.help = Some(0),
            Action::CommandLine => {
                self.prompt = Some(Prompt { kind: PromptKind::Command, input: Input::default() })
            }
            Action::Refresh => {
                let id = self.board.snapshot.as_ref().map(|s| s.board.id.clone());
                self.effects.push(Effect::Send(Command::ReloadBoard(id)));
                self.say("Refreshing…", Level::Info);
            }
            Action::PickerIssues => self.open_issue_picker(),
            Action::PickerRemote => {
                self.open_issue_picker();
                self.toggle_source();
            }
            Action::PickerBoards => {
                self.effects.push(Effect::Send(Command::LoadBoards));
                self.say("Listing the boards…", Level::Info);
            }
            Action::PickerCommands => self.open_picker(Box::new(Picker::new(
                sources::CommandSource::new(&self.keymap),
                self.waker.clone(),
            ))),
            Action::PickerThemes => self.open_theme_picker(),
            Action::PickerResume => match self.last_picker.take() {
                Some(p) => self.picker = Some(p),
                None => self.say("No picker to reopen", Level::Info),
            },
            _ => match self.focus() {
                Focus::Problems => {}
                Focus::Help => self.help_action(action, n),
                Focus::Picker => self.picker_action(action, n),
                Focus::Prompt => self.prompt_action(action),
                Focus::Detail => self.detail_action(action, count),
                Focus::Board => self.board_action(action, count),
            },
        }
    }

    /// Closes what is on top; on the bare board, quits.
    fn close(&mut self) {
        match self.focus() {
            Focus::Problems => self.problems.clear(),
            Focus::Help => self.help = None,
            Focus::Picker => self.close_picker(),
            Focus::Prompt => {
                if self.prompt.take().is_some_and(|p| p.kind == PromptKind::Filter) {
                    self.board.filter.clear();
                    self.board.clamp();
                }
            }
            Focus::Detail => self.detail = None,
            Focus::Board => self.quit = true,
        }
    }

    fn board_action(&mut self, action: Action, count: Option<u32>) {
        let n = i64::from(count.unwrap_or(1));
        let b = &mut self.board;
        // The range belongs to its column: leaving the column ends it.
        if matches!(action, Action::Left | Action::Right | Action::FirstColumn | Action::LastColumn)
        {
            b.visual = None;
        }
        match action {
            Action::VisualMode => {
                b.visual = match b.visual {
                    Some(_) => None,
                    None => b.selected().map(|_| b.cursor_row()),
                };
            }
            Action::NormalMode => b.visual = None,
            Action::Left => b.move_column(-n),
            Action::Right => b.move_column(n),
            Action::Down => b.move_card(n),
            Action::Up => b.move_card(-n),
            Action::Top => b.card_at(Some(count.map_or(0, |c| c as usize - 1))),
            Action::Bottom => b.card_at(count.map(|c| c as usize - 1)),
            Action::FirstColumn => b.column_edge(false),
            Action::LastColumn => b.column_edge(true),
            Action::HalfPageDown => b.move_card(b.half_page() * n),
            Action::HalfPageUp => b.move_card(-b.half_page() * n),
            Action::NextMatch => b.step_match(n),
            Action::PrevMatch => b.step_match(-n),
            Action::ClearFilter => {
                b.filter.clear();
                b.clamp();
            }
            Action::Filter => {
                let mut input = Input::default();
                input.set(&b.filter);
                self.prompt = Some(Prompt { kind: PromptKind::Filter, input });
            }
            Action::ToggleSwimlanes => self.toggle_swimlanes(),
            Action::ToggleEmptyColumns => self.toggle_empty_columns(),
            Action::OpenDetail => {
                if let Some(key) = b.selected().map(|i| i.key.clone()) {
                    self.open_detail(key);
                }
            }
            Action::CopyKey if b.visual.is_some() => self.copy_range(),
            Action::CopyKey | Action::CopyUrl | Action::OpenBrowser => {
                if let Some(issue) = b.selected().cloned() {
                    self.issue_effect(action, &issue);
                }
            }
            Action::Create => {
                if self.can_write() {
                    self.create();
                }
            }
            Action::PickerUsers => self.open_people_picker(),
            Action::MoveCardDown => self.rank_card(n),
            Action::MoveCardUp => self.rank_card(-n),
            Action::MoveCardLeft => self.move_card(-n),
            Action::MoveCardRight => self.move_card(n),
            _ => {
                let keys = b.targets();
                b.visual = None;
                self.issue_action(action, keys);
            }
        }
    }

    /// Everyone, me, then whoever has a card on the board by name, then no
    /// one; with how many cards each has, and those already shown flagged.
    fn open_people_picker(&mut self) {
        let issues = self.board.snapshot.iter().flat_map(|s| &s.issues);
        let mut counts: Vec<(Option<kolum_domain::User>, usize)> = vec![];
        for issue in issues {
            match counts
                .iter_mut()
                .find(|(u, _)| u.as_ref().map(|u| &u.id) == issue.assignee.as_ref().map(|a| &a.id))
            {
                Some((_, n)) => *n += 1,
                None => counts.push((issue.assignee.clone(), 1)),
            }
        }
        let me = self.me.as_ref().map(|u| u.id.clone());
        let cards = |n: usize| if n == 1 { "1 card".to_string() } else { format!("{n} cards") };
        let count_of = |id: Option<&kolum_domain::UserId>| {
            counts.iter().find(|(u, _)| u.as_ref().map(|u| &u.id) == id).map_or(0, |(_, n)| *n)
        };
        let mut people = vec![(String::new(), "Everyone".to_string())];
        if let Some(me_user) = &self.me {
            people.push((
                "@me".into(),
                format!("{} (me)  {}", me_user.display_name, cards(count_of(me.as_ref()))),
            ));
        }
        let mut others: Vec<&kolum_domain::User> = counts
            .iter()
            .filter_map(|(u, _)| u.as_ref())
            .filter(|u| Some(&u.id) != me.as_ref())
            .collect();
        others.sort_by(|a, b| a.display_name.cmp(&b.display_name));
        for u in others {
            let token = format!("@{}", u.display_name.replace(' ', ""));
            people.push((token, format!("{}  {}", u.display_name, cards(count_of(Some(&u.id))))));
        }
        let unassigned = count_of(None);
        if unassigned > 0 {
            people.push(("@none".into(), format!("Unassigned  {}", cards(unassigned))));
        }
        // Those already shown say so rather than being marked: with marks,
        // Enter would act on them and not on the person highlighted.
        let shown: Vec<&str> =
            self.board.filter.split_whitespace().filter(|w| w.starts_with('@')).collect();
        for (token, label) in &mut people {
            if shown.contains(&token.as_str()) {
                label.push_str("  · shown");
            }
        }
        let picker = Picker::new(sources::PeopleChoice { people }, self.waker.clone());
        self.open_picker(Box::new(picker));
    }

    /// The `@` part of the board filter becomes these people; the rest of
    /// what was typed stays.
    fn show_people(&mut self, tokens: &[String]) {
        let everyone = tokens.iter().any(String::is_empty);
        let mut words: Vec<&str> =
            self.board.filter.split_whitespace().filter(|w| !w.starts_with('@')).collect();
        if !everyone {
            words.extend(tokens.iter().map(String::as_str));
        }
        self.board.filter = words.join(" ");
        self.board.clamp();
        let msg = if everyone || tokens.is_empty() {
            "Everyone's cards".to_string()
        } else {
            format!("Cards of {}", tokens.join(" "))
        };
        self.say(msg, Level::Info);
    }

    fn toggle_swimlanes(&mut self) {
        let b = &mut self.board;
        let key = b.selected().map(|i| i.key.clone());
        b.swimlanes = b.swimlanes.next();
        if let Some(key) = key {
            b.select(&key);
        }
        let name = b.swimlanes.name();
        self.say(name[..1].to_uppercase() + &name[1..], Level::Info);
        self.save_state();
    }

    fn toggle_empty_columns(&mut self) {
        let b = &mut self.board;
        b.hide_empty = !b.hide_empty;
        b.clamp();
        let what = if b.hide_empty { "Hiding" } else { "Showing" };
        self.say(format!("{what} empty columns"), Level::Info);
        self.save_state();
    }

    /// `yy` in visual mode: every key in the range, space-separated.
    fn copy_range(&mut self) {
        let keys: Vec<String> = self.board.targets().iter().map(|k| k.to_string()).collect();
        self.board.visual = None;
        self.say(format!("Copied {}", keys.join(" ")), Level::Info);
        self.effects.push(Effect::Copy(keys.join(" ")));
    }

    fn save_state(&mut self) {
        let state = crate::config::UiState {
            hide_empty_columns: self.board.hide_empty,
            swimlanes: self.board.swimlanes,
        };
        self.effects.push(Effect::SaveState(state));
    }

    fn detail_action(&mut self, action: Action, count: Option<u32>) {
        let n = i64::from(count.unwrap_or(1));
        let Some(d) = &mut self.detail else { return };
        match action {
            Action::Down => d.scroll_by(n),
            Action::Up => d.scroll_by(-n),
            Action::Top => d.scroll = 0,
            Action::Bottom => d.scroll_by(i64::from(u16::MAX)),
            Action::HalfPageDown => d.scroll_by(d.half_page() * n),
            Action::HalfPageUp => d.scroll_by(-d.half_page() * n),
            Action::NextTab => d.tab = d.tab.step(n),
            Action::PrevTab => d.tab = d.tab.step(-n),
            Action::CopyKey | Action::CopyUrl | Action::OpenBrowser => {
                let issue = d.issue.clone();
                self.issue_effect(action, &issue);
            }
            _ => {
                let keys = vec![d.issue.key.clone()];
                self.issue_action(action, keys);
            }
        }
    }

    fn help_action(&mut self, action: Action, n: i64) {
        let half = i64::from(self.help_page.get() / 2).max(1);
        let delta = match action {
            Action::Down => n,
            Action::Up => -n,
            Action::HalfPageDown => half * n,
            Action::HalfPageUp => -half * n,
            Action::Top => i64::from(i16::MIN),
            Action::Bottom => i64::from(u16::MAX),
            _ => return,
        };
        let max = help::lines(&self.keymap, &self.theme)
            .len()
            .saturating_sub(self.help_page.get() as usize);
        let at = self.help.unwrap_or(0) as i64 + delta;
        self.help = Some(at.clamp(0, max as i64) as u16);
    }

    fn issue_effect(&mut self, action: Action, issue: &kolum_domain::Issue) {
        match (action, &issue.url) {
            (Action::CopyKey, _) => {
                self.effects.push(Effect::Copy(issue.key.to_string()));
                self.say(format!("Copied {}", issue.key), Level::Info);
            }
            (Action::CopyUrl, Some(url)) => {
                self.effects.push(Effect::Copy(url.clone()));
                self.say(format!("Copied {url}"), Level::Info);
            }
            (Action::OpenBrowser, Some(url)) => self.effects.push(Effect::Open(url.clone())),
            _ => self.say(format!("{} has no URL", issue.key), Level::Warn),
        }
    }

    /// The actions that change issues, wherever they were asked from.
    fn issue_action(&mut self, action: Action, keys: Vec<IssueKey>) {
        let writes = matches!(
            action,
            Action::Transition
                | Action::Assign
                | Action::AssignMe
                | Action::Comment
                | Action::Edit
                | Action::Clone
        );
        if keys.is_empty() || (writes && !self.can_write()) {
            return;
        }
        match action {
            Action::Transition => self.ask_transitions(keys, None),
            Action::Assign => self.open_user_picker(keys),
            Action::AssignMe => match self.me.clone() {
                Some(me) => self.assign(keys, Some(me)),
                None => self.say("Still finding out who you are", Level::Warn),
            },
            Action::Comment => self.edit(EditPurpose::Comment(keys[0].clone())),
            Action::Edit => self.edit(EditPurpose::Describe(keys[0].clone())),
            Action::Clone => {
                self.effects.push(Effect::Send(Command::Clone(keys[0].clone())));
                self.say(format!("Cloning {}…", keys[0]), Level::Info);
            }
            _ => self.unavailable(action),
        }
    }

    fn can_write(&mut self) -> bool {
        if !self.writable {
            self.say("This provider is read-only in kolum for now", Level::Warn);
        }
        self.writable
    }

    fn issue(&self, key: &IssueKey) -> Option<&kolum_domain::Issue> {
        let on_board =
            self.board.snapshot.as_ref().and_then(|s| s.issues.iter().find(|i| &i.key == key));
        on_board.or(self.detail.as_ref().map(|d| &d.issue).filter(|i| &i.key == key))
    }

    /// `H` and `L`: to the next visible column, by whichever transition
    /// leads there.
    fn move_card(&mut self, delta: i64) {
        let keys = self.board.targets();
        self.board.visual = None;
        if keys.is_empty() {
            return;
        }
        let visible = self.board.visible();
        let pos = visible.iter().position(|&c| c == self.board.column).unwrap_or(0) as i64 + delta;
        match usize::try_from(pos).ok().and_then(|p| visible.get(p)) {
            Some(&column) if self.can_write() => self.ask_transitions(keys, Some(column)),
            Some(_) => {}
            None => self.say("No column that way", Level::Info),
        }
    }

    /// `J` and `K`: next to the card `delta` places away, in the provider's
    /// order, shown at once.
    fn rank_card(&mut self, delta: i64) {
        self.board.visual = None;
        let Some(key) = self.board.selected().map(|i| i.key.clone()) else { return };
        let other = match self.board.neighbour(delta) {
            Ok(other) => other,
            Err(why) => return self.say(why, Level::Info),
        };
        if !self.can_write() {
            return;
        }
        let rank = if delta > 0 {
            kolum_domain::Rank::After(other)
        } else {
            kolum_domain::Rank::Before(other)
        };
        if let Some(op) = self.optimistic(&key, kolum_app::Change::Rank(rank.clone())) {
            let write = kolum_app::Write::Rank(rank);
            self.effects.push(Effect::Send(Command::Write { op, key: key.clone(), write }));
        }
        let way = if delta > 0 { "down" } else { "up" };
        self.say(format!("{key} moved {way}"), Level::Info);
    }

    fn ask_transitions(&mut self, keys: Vec<IssueKey>, column: Option<usize>) {
        self.effects.push(Effect::Send(Command::Transitions(keys[0].clone())));
        self.awaiting = Some((keys, column));
    }

    fn on_transitions(
        &mut self,
        key: IssueKey,
        result: Result<Vec<kolum_domain::Transition>, kolum_domain::DomainError>,
    ) {
        let Some((keys, column)) = self.awaiting.take_if(|(keys, _)| keys[0] == key) else {
            return;
        };
        let transitions = match result {
            Ok(t) => t,
            Err(e) => return self.say(format!("{key}: {e}"), Level::Error),
        };
        let board = self.board.snapshot.as_ref().map(|s| &s.board);
        // Jira's team-managed projects offer a transition to the status the
        // issue is already in: valid there, nothing to choose here.
        let here = self.issue(&key).map(|i| i.status.id.clone());
        let transitions: Vec<_> =
            transitions.into_iter().filter(|t| Some(&t.to.id) != here.as_ref()).collect();
        let transitions = match (column, board) {
            (Some(c), Some(board)) => kolum_app::transitions_into(board, c, &transitions),
            _ => transitions,
        };
        match (column, transitions.len()) {
            (Some(c), 0) => {
                let name = board
                    .and_then(|b| b.columns.get(c))
                    .map_or("there", |c| c.name.as_str())
                    .to_string();
                self.say(format!("No transition takes {key} to {name} from here"), Level::Warn)
            }
            (None, 0) => self.say(format!("No transition is open to {key}"), Level::Warn),
            (Some(_), 1) => self.transition(keys, transitions[0].clone()),
            _ => {
                let source = sources::TransitionChoice { keys, transitions };
                self.open_picker(Box::new(Picker::new(source, self.waker.clone())));
            }
        }
    }

    /// A transition that asks for fields (a resolution on the way to Done)
    /// is left to the web app, which has the form for them.
    fn transition(&mut self, keys: Vec<IssueKey>, t: kolum_domain::Transition) {
        if !t.required_fields.is_empty() {
            let fields: Vec<&str> = t.required_fields.iter().map(|f| f.as_str()).collect();
            let msg = format!(
                "`{}` asks for {}: press o to do it in the browser",
                t.name,
                fields.join(", ")
            );
            return self.say(msg, Level::Warn);
        }
        for key in &keys {
            if let Some(op) = self.optimistic(key, kolum_app::Change::Status(t.to.clone())) {
                let write = kolum_app::Write::Transition(t.id.clone());
                self.effects.push(Effect::Send(Command::Write { op, key: key.clone(), write }));
            }
        }
        self.board.select(&keys[0]);
        let names: Vec<&str> = keys.iter().map(|k| k.as_str()).collect();
        self.say(format!("{} → {}", names.join(", "), t.to.name), Level::Info);
    }

    /// Who an issue can go to: me, then whoever is on the board.
    fn open_user_picker(&mut self, keys: Vec<IssueKey>) {
        let mut users: Vec<kolum_domain::User> = self.me.clone().into_iter().collect();
        let mut others: Vec<kolum_domain::User> = vec![];
        for issue in self.board.snapshot.iter().flat_map(|s| &s.issues) {
            for u in [&issue.assignee, &issue.reporter].into_iter().flatten() {
                if !users.iter().chain(&others).any(|o| o.id == u.id) {
                    others.push(u.clone());
                }
            }
        }
        others.sort_by(|a, b| a.display_name.cmp(&b.display_name));
        users.extend(others);
        let mut choices: Vec<Option<kolum_domain::User>> = users.into_iter().map(Some).collect();
        choices.push(None);
        let source = sources::UserChoice { keys, users: choices };
        self.open_picker(Box::new(Picker::new(source, self.waker.clone())));
    }

    fn assign(&mut self, keys: Vec<IssueKey>, user: Option<kolum_domain::User>) {
        for key in &keys {
            if let Some(op) = self.optimistic(key, kolum_app::Change::Assignee(user.clone())) {
                let write = kolum_app::Write::Assign(user.as_ref().map(|u| u.id.clone()));
                self.effects.push(Effect::Send(Command::Write { op, key: key.clone(), write }));
            }
        }
        let who = user.as_ref().map_or("nobody".to_string(), |u| u.display_name.clone());
        self.say(format!("Assigned to {who}"), Level::Info);
    }

    /// Shows the change on the board now; the id is the write's.
    fn optimistic(&mut self, key: &IssueKey, change: kolum_app::Change) -> Option<u64> {
        let snapshot = self.board.snapshot.as_mut()?;
        let op = self.ledger.begin(snapshot, key, &change);
        self.board.clamp();
        self.board.select(key);
        self.sync_detail(key);
        Some(op)
    }

    /// The detail shows the board's copy of its issue, changes included.
    fn sync_detail(&mut self, key: &IssueKey) {
        let fresh = self
            .board
            .snapshot
            .as_ref()
            .and_then(|s| s.issues.iter().find(|i| &i.key == key))
            .cloned();
        if let (Some(d), Some(fresh)) = (&mut self.detail, fresh)
            && &d.issue.key == key
        {
            d.issue = fresh;
        }
    }

    /// `c`: a new issue in the board's project, of the highlighted card's
    /// type (a Task when there is none).
    fn create(&mut self) {
        let selected = self.board.selected();
        let project = self
            .board
            .snapshot
            .as_ref()
            .and_then(|s| s.board.project.as_ref().map(|p| p.key.clone()))
            .or_else(|| {
                selected.map(|i| i.key.as_str().split('-').next().unwrap_or_default().to_string())
            });
        let Some(project) = project else {
            return self.say("No project to create the issue in", Level::Warn);
        };
        let task =
            kolum_domain::IssueType { name: "Task".into(), kind: kolum_domain::IssueKind::Task };
        let kind = selected.map_or(task, |i| i.kind.clone());
        let text = format!(
            "# \n\n{MARKER} a new {} in {project}: the first `# ` line is the title, the rest the description, in Markdown. No title cancels. -->\n",
            kind.name
        );
        self.effects.push(Effect::Edit { purpose: EditPurpose::Create { project, kind }, text });
    }

    fn edit(&mut self, purpose: EditPurpose) {
        let (EditPurpose::Comment(key) | EditPurpose::Describe(key)) = &purpose else { return };
        let Some(issue) = self.issue(key) else { return };
        let text = match &purpose {
            EditPurpose::Comment(_) => format!(
                "\n\n{MARKER} a comment on {}: {}. Markdown above this line; leave it empty to cancel. -->\n",
                issue.key, issue.title
            ),
            EditPurpose::Create { .. } => return,
            EditPurpose::Describe(_) => format!(
                "# {}\n\n{}\n{MARKER} {}: the first `# ` line is the title, the rest the description, in Markdown. -->\n",
                issue.title,
                issue.description.to_markdown(),
                issue.key
            ),
        };
        self.effects.push(Effect::Edit { purpose, text });
    }

    /// What came back from `$EDITOR`.
    pub fn on_edited(&mut self, purpose: EditPurpose, result: Result<String, String>) {
        let text = match result {
            Ok(text) => text.split(MARKER).next().unwrap_or_default().trim().to_string(),
            Err(e) => return self.say(format!("$EDITOR: {e}"), Level::Error),
        };
        match purpose {
            EditPurpose::Comment(_) if text.is_empty() => {
                self.say("Nothing written: no comment", Level::Info)
            }
            EditPurpose::Comment(key) => {
                let change = kolum_app::Change::Update(kolum_domain::IssueUpdate::default());
                if let Some(op) = self.optimistic(&key, change) {
                    let write =
                        kolum_app::Write::Comment(kolum_domain::RichText::from_markdown(&text));
                    self.effects.push(Effect::Send(Command::Write { op, key: key.clone(), write }));
                    self.say(format!("Commenting on {key}…"), Level::Info);
                }
            }
            EditPurpose::Create { project, kind } => {
                let (title, body) = split_title(&text);
                let Some(title) = title.filter(|t| !t.is_empty()) else {
                    return self.say("No title: nothing created", Level::Info);
                };
                let description = kolum_domain::RichText::from_markdown(body);
                let new = kolum_domain::NewIssue {
                    project,
                    kind,
                    title: title.to_string(),
                    description,
                    parent: None,
                };
                self.effects.push(Effect::Send(Command::Create(new)));
                self.say("Creating…", Level::Info);
            }
            EditPurpose::Describe(key) => {
                let Some(issue) = self.issue(&key) else { return };
                let update = description_update(issue, &text);
                if update == kolum_domain::IssueUpdate::default() {
                    return self.say("No change", Level::Info);
                }
                if let Some(op) = self.optimistic(&key, kolum_app::Change::Update(update.clone())) {
                    let write = kolum_app::Write::Update(update);
                    self.effects.push(Effect::Send(Command::Write { op, key: key.clone(), write }));
                    self.say(format!("Saving {key}…"), Level::Info);
                }
            }
        }
    }

    /// The pictures of the open issue: its assignee's avatar, and its
    /// first image attachment.
    fn pictures(&self) -> [Option<String>; 2] {
        let Some(d) = &self.detail else { return [None, None] };
        let avatar = d.issue.assignee.as_ref().and_then(|a| a.avatar_url.clone());
        let shot = d.issue.attachments.iter().find(|a| a.is_image()).map(|a| a.url.clone());
        [avatar, shot]
    }

    /// Asks for the open issue's pictures, once per URL, when pictures can
    /// be drawn here.
    fn want_avatar(&mut self) {
        if self.images.is_none() {
            return;
        }
        for url in self.pictures().into_iter().flatten() {
            if !self.avatars.contains_key(&url) {
                self.avatars.insert(url.clone(), Avatar::Loading);
                self.effects.push(Effect::Send(Command::Image(url)));
            }
        }
    }

    /// Opens at once from the board's copy; an issue not on the board (a
    /// remote search hit) opens when it has been read.
    fn open_detail(&mut self, key: IssueKey) {
        let issue =
            self.board.snapshot.as_ref().and_then(|s| s.issues.iter().find(|i| i.key == key));
        match issue {
            Some(issue) => {
                self.detail = Some(DetailView::new(issue.clone()));
                self.want_avatar();
            }
            None => {
                self.say(format!("Opening {key}…"), Level::Info);
                self.opening = Some(key.clone());
            }
        }
        self.effects.push(Effect::Send(Command::LoadIssue(key)));
    }
    fn open_picker(&mut self, picker: Box<dyn AnyPicker>) {
        self.close_picker();
        self.picker = Some(picker);
        self.history_pos = None;
    }

    fn open_issue_picker(&mut self) {
        let issues = self.board.snapshot.as_ref().map(|s| s.issues.clone()).unwrap_or_default();
        let me = self.me.as_ref().map(|u| u.id.clone());
        self.open_picker(Box::new(Picker::new(
            sources::IssueSource { issues, me, remote: None },
            self.waker.clone(),
        )));
    }

    fn open_theme_picker(&mut self) {
        let mut names: Vec<String> = theme::BUILTIN.iter().map(|s| s.to_string()).collect();
        if let Some(dir) = &self.themes_dir
            && let Ok(entries) = std::fs::read_dir(dir)
        {
            let mut files: Vec<String> = entries
                .filter_map(|e| e.ok()?.path().file_stem()?.to_str().map(String::from))
                .filter(|n| !names.contains(n))
                .collect();
            files.sort();
            names.extend(files);
        }
        self.open_picker(Box::new(Picker::new(sources::ThemeSource { names }, self.waker.clone())));
    }

    /// Keeps the picker for `picker_resume`, and undoes a theme preview
    /// that was not confirmed.
    fn close_picker(&mut self) {
        if let Some(p) = self.picker.take() {
            self.last_picker = Some(p);
        }
        if let Some((name, theme)) = self.theme_before_preview.take() {
            self.theme_name = name;
            self.theme = theme;
        }
    }

    fn picker_action(&mut self, action: Action, n: i64) {
        let Some(p) = &mut self.picker else { return };
        match action {
            Action::Down => p.move_by(n),
            Action::Up => p.move_by(-n),
            Action::Top => p.move_to(Some(0)),
            Action::Bottom => p.move_to(None),
            Action::InsertMode => p.set_insert(true),
            Action::NormalMode => p.set_insert(false),
            Action::ToggleMark => p.toggle_mark(),
            Action::PreviewDown => p.scroll_preview(5 * n as i32),
            Action::PreviewUp => p.scroll_preview(-5 * n as i32),
            Action::HistoryPrev | Action::HistoryNext => self.recall(action == Action::HistoryPrev),
            Action::ToggleSource => self.toggle_source(),
            action => {
                let query = p.query().to_string();
                match p.act(action) {
                    Some(pick) => {
                        if !query.is_empty() && self.history.last() != Some(&query) {
                            self.history.push(query);
                        }
                        self.pick(pick);
                    }
                    None if action == Action::Confirm => {}
                    None => self.unavailable(action),
                }
            }
        }
    }

    /// Local or remote. Going remote searches straight away, with what is
    /// already typed.
    fn toggle_source(&mut self) {
        let Some(p) = &mut self.picker else { return };
        match p.toggle_remote() {
            Some(true) => self.search_due = Some(self.now),
            Some(false) => self.search_due = None,
            None => self.say("This picker has no remote search", Level::Warn),
        }
    }

    fn search(&mut self) {
        self.search_due = None;
        let Some(p) = self.picker.as_mut().filter(|p| p.is_remote()) else { return };
        self.search_id += 1;
        p.set_loading(true);
        let query = kolum_app::parse_query(p.query());
        self.effects.push(Effect::Send(Command::Search { id: self.search_id, query }));
    }

    /// Steps through past queries, newest first.
    fn recall(&mut self, older: bool) {
        let len = self.history.len();
        let pos = match (self.history_pos, older) {
            (None, true) if len > 0 => Some(len - 1),
            (Some(i), true) => Some(i.saturating_sub(1)),
            (Some(i), false) if i + 1 < len => Some(i + 1),
            _ => None,
        };
        self.history_pos = pos;
        let query = pos.map(|i| self.history[i].clone()).unwrap_or_default();
        if let Some(p) = &mut self.picker {
            p.set_query(&query);
            if p.is_remote() {
                self.search_due = Some(self.now + self.remote_debounce);
            }
        }
    }

    fn pick(&mut self, pick: Pick) {
        match pick {
            Pick::OpenIssue(key) => {
                self.close_picker();
                self.board.select(&key);
                self.open_detail(key);
            }
            Pick::Copy(text) => {
                self.close_picker();
                self.say(format!("Copied {text}"), Level::Info);
                self.effects.push(Effect::Copy(text));
            }
            Pick::OpenUrl(url) => {
                self.close_picker();
                self.effects.push(Effect::Open(url));
            }
            Pick::Theme(name) => {
                self.theme_before_preview = None;
                self.close_picker();
                self.set_theme(&name);
            }
            Pick::OnIssues(action, keys) => {
                self.close_picker();
                self.issue_action(action, keys);
            }
            Pick::Transition { keys, transition } => {
                self.close_picker();
                self.transition(keys, transition);
            }
            Pick::Assign { keys, user } => {
                self.close_picker();
                self.assign(keys, user);
            }
            Pick::People(tokens) => {
                self.close_picker();
                self.show_people(&tokens);
            }
            Pick::Board(id) => {
                self.close_picker();
                self.detail = None;
                self.effects.push(Effect::Send(Command::LoadBoard(Some(id))));
            }
            Pick::Run(action) => {
                self.close_picker();
                self.dispatch(action, None);
            }
            Pick::Unavailable(action) => self.unavailable(action),
        }
    }

    fn load_theme(&self, name: &str) -> Result<Theme, String> {
        let (mut theme, _) = Theme::load(name, self.themes_dir.as_deref())?;
        theme.apply(&self.theme_overrides, "[theme.overrides]");
        Ok(theme)
    }

    fn set_theme(&mut self, name: &str) {
        match self.load_theme(name) {
            Ok(theme) => {
                self.theme = theme;
                self.theme_name = name.to_string();
                self.say(format!("Theme {name}"), Level::Info);
            }
            Err(e) => self.say(e, Level::Error),
        }
    }

    /// The theme picker shows the highlighted theme at once.
    fn preview_hovered_theme(&mut self) {
        let Some(Pick::Theme(name)) = self.picker.as_ref().and_then(|p| p.hovered()) else {
            return;
        };
        if name == self.theme_name {
            return;
        }
        if let Ok(theme) = self.load_theme(&name) {
            let before = std::mem::replace(&mut self.theme, theme);
            let old_name = std::mem::replace(&mut self.theme_name, name);
            self.theme_before_preview.get_or_insert((old_name, before));
        }
    }

    fn prompt_action(&mut self, action: Action) {
        if action != Action::Confirm {
            return;
        }
        let Some(prompt) = self.prompt.take() else { return };
        if prompt.kind == PromptKind::Command {
            self.run_command(prompt.input.text().trim());
        }
    }

    /// The `:` line.
    fn run_command(&mut self, line: &str) {
        let (name, arg) = line.split_once(' ').map_or((line, ""), |(n, a)| (n, a.trim()));
        match name {
            "" => {}
            "q" | "q!" | "qa" | "quit" => self.quit = true,
            "h" | "help" => self.help = Some(0),
            "e" | "refresh" => self.dispatch(Action::Refresh, None),
            "theme" if arg.is_empty() => self.open_theme_picker(),
            "theme" => self.set_theme(arg),
            "board" if arg.is_empty() => self.dispatch(Action::PickerBoards, None),
            "board" => {
                let id = kolum_domain::BoardId::new(arg);
                self.effects.push(Effect::Send(Command::LoadBoard(Some(id))));
            }
            "jql" => {
                self.dispatch(Action::PickerRemote, None);
                if let Some(p) = self.picker.as_mut().filter(|_| !arg.is_empty()) {
                    p.set_query(&format!("!{arg}"));
                }
            }
            "assign" => self.say(":assign is not available yet", Level::Warn),
            _ => self.say(format!("Not a command: {name}"), Level::Error),
        }
    }

    pub fn draw(&mut self, frame: &mut Frame) {
        let theme = self.theme.clone();
        let area = frame.area();
        frame.render_widget(Block::new().style(Style::new().bg(theme.bg).fg(theme.fg)), area);
        let [main, status] =
            Layout::vertical([Constraint::Min(0), Constraint::Length(1)]).areas(area);
        let me = self.me.as_ref().map(|u| &u.id);
        let [avatar_url, shot_url] = self.pictures();
        let none = String::new();
        let (a, s) = (avatar_url.as_ref().unwrap_or(&none), shot_url.as_ref().unwrap_or(&none));
        // Two pictures from one map: both borrowed at once, by distinct keys.
        let [avatar, shot] = if a == s {
            [self.avatars.get_mut(a), None]
        } else {
            self.avatars.get_disjoint_mut([a, s])
        };
        let pictures = detail::Pictures { avatar: ready(avatar), shot: ready(shot) };
        match &self.detail {
            Some(d) => d.draw(frame, main, &theme, &self.ui.detail_fields, me, pictures),
            None => {
                let [head, body] =
                    Layout::vertical([Constraint::Length(1), Constraint::Min(0)]).areas(main);
                frame.render_widget(Paragraph::new(self.board_header(&theme)), head);
                let ctx = DrawCtx { theme: &theme, fields: &self.ui.card_fields, me };
                self.board.draw(frame, body, &ctx);
            }
        }
        if let Some(p) = &mut self.picker {
            p.draw(frame, centered(main, 90, 85), &theme, self.picker_layout);
        }
        if let Some(scroll) = self.help {
            let area = centered(main, 80, 90);
            let block = popup("Keys  (j/k to scroll, q to close)", &theme);
            self.help_page.set(block.inner(area).height);
            let lines = help::lines(&self.keymap, &theme);
            frame.render_widget(Clear, area);
            frame.render_widget(Paragraph::new(lines).block(block).scroll((scroll, 0)), area);
        }
        if !self.problems.is_empty() {
            let area = centered(main, 70, 60);
            let mut lines: Vec<Line> = self
                .problems
                .iter()
                .map(|p| {
                    Line::from(vec![
                        Span::styled("• ", Style::new().fg(theme.warn)),
                        Span::raw(p.clone()),
                    ])
                })
                .collect();
            lines.extend([
                Line::default(),
                Line::from(Span::styled("Press any key", Style::new().fg(theme.muted))),
            ]);
            frame.render_widget(Clear, area);
            let block = popup("Config problems", &theme);
            frame
                .render_widget(Paragraph::new(lines).block(block).wrap(Wrap { trim: false }), area);
        }
        self.draw_which_key(frame, main, &theme);
        self.draw_status(frame, status, &theme);
    }

    fn board_header(&self, theme: &Theme) -> Line<'static> {
        let mut spans = vec![];
        if let Some(s) = &self.board.snapshot {
            spans.push(Span::styled(
                format!(" {} ", s.board.name),
                Style::new().fg(theme.accent).add_modifier(Modifier::BOLD),
            ));
            spans.push(Span::styled(format!("{:?}", s.board.kind), Style::new().fg(theme.muted)));
            if self.board_cached {
                spans.push(Span::styled("  cached, syncing…", Style::new().fg(theme.warn)));
            }
        }
        if !self.board.filter.is_empty() {
            spans.push(Span::styled(
                format!("   /{}", self.board.filter),
                Style::new().fg(theme.match_highlight),
            ));
        }
        Line::from(spans)
    }

    fn draw_status(&self, frame: &mut Frame, area: Rect, theme: &Theme) {
        let (mode, color) = match self.focus() {
            Focus::Prompt => ("COMMAND", theme.mode_command),
            Focus::Picker if self.picker.as_ref().is_some_and(|p| p.is_insert()) => {
                ("INSERT", theme.ok)
            }
            Focus::Board if self.board.visual.is_some() => ("VISUAL", theme.mode_visual),
            _ => ("NORMAL", theme.accent),
        };
        let mut left = vec![
            Span::styled(
                format!(" {mode} "),
                Style::new().bg(color).fg(theme.badge_fg).add_modifier(Modifier::BOLD),
            ),
            Span::raw(" "),
        ];
        if let Some(prompt) = &self.prompt {
            let lead = if prompt.kind == PromptKind::Command { ":" } else { "/" };
            let text = prompt.input.text();
            let typed: String = text.chars().take(prompt.input.cursor()).collect();
            let x =
                area.x + (Line::from(left.clone()).width() + width(lead) + width(&typed)) as u16;
            left.push(Span::raw(format!("{lead}{text}")));
            frame.set_cursor_position((x, area.y));
        } else if let Some((text, level)) = &self.message {
            let color = match level {
                Level::Info => theme.fg,
                Level::Warn => theme.warn,
                Level::Error => theme.error,
            };
            left.push(Span::styled(text.clone(), Style::new().fg(color)));
        }
        let right = Line::from(vec![
            Span::styled(self.resolver.typed(), Style::new().fg(theme.match_highlight)),
            Span::styled("  ? help ", Style::new().fg(theme.muted)),
        ]);
        frame.render_widget(Paragraph::new(Line::from(left)), area);
        frame.render_widget(Paragraph::new(right).right_aligned(), area);
    }

    /// After a prefix like `g` or `<leader>`, what can follow.
    fn draw_which_key(&self, frame: &mut Frame, area: Rect, theme: &Theme) {
        let (contexts, text) = self.contexts();
        let layers =
            Layers { tables: contexts.iter().map(|&c| self.keymap.table(c)).collect(), text };
        let next = self.resolver.continuations(&layers);
        if next.is_empty() {
            return;
        }
        let lines: Vec<Line> = next
            .iter()
            .map(|(keys, action)| {
                Line::from(vec![
                    Span::styled(format!(" {keys:<8}"), Style::new().fg(theme.match_highlight)),
                    Span::raw(action.description()),
                ])
            })
            .collect();
        let width = (lines.iter().map(Line::width).max().unwrap_or(0) as u16 + 3).min(area.width);
        let height = (lines.len() as u16 + 2).min(area.height);
        let rect = Rect { x: area.right() - width, y: area.bottom() - height, width, height };
        frame.render_widget(Clear, rect);
        let title = format!("{} …", self.resolver.typed());
        frame.render_widget(Paragraph::new(lines).block(popup(&title, theme)), rect);
    }
}

/// `# Title` then Markdown, against the issue as it is: only what moved is
/// sent. The description is compared as Markdown, so a rich text that does
/// not survive the round trip exactly is not rewritten for nothing.
fn description_update(issue: &kolum_domain::Issue, text: &str) -> kolum_domain::IssueUpdate {
    let (title, body) = split_title(text);
    kolum_domain::IssueUpdate {
        title: title.filter(|t| !t.is_empty() && *t != issue.title).map(String::from),
        description: (body != issue.description.to_markdown().trim())
            .then(|| kolum_domain::RichText::from_markdown(body)),
        ..Default::default()
    }
}

/// `# Title` on the first line, if there is one, and the rest.
fn split_title(text: &str) -> (Option<&str>, &str) {
    let text = text.trim_start();
    match text.split_once('\n') {
        Some((first, rest)) if first.trim() == "#" || first.starts_with("# ") => {
            (Some(first.trim_start_matches('#').trim()), rest.trim())
        }
        None if text.starts_with("# ") || text.trim() == "#" => {
            (Some(text.trim_start_matches('#').trim()), "")
        }
        _ => (None, text.trim()),
    }
}

fn popup(title: &str, theme: &Theme) -> Block<'static> {
    Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(theme.border_focus))
        .title(Span::styled(format!(" {title} "), Style::new().fg(theme.accent)))
        .style(Style::new().bg(theme.bg).fg(theme.fg))
}

fn centered(area: Rect, width_pct: u16, height_pct: u16) -> Rect {
    let [row] =
        Layout::vertical([Constraint::Percentage(height_pct)]).flex(Flex::Center).areas(area);
    let [cell] =
        Layout::horizontal([Constraint::Percentage(width_pct)]).flex(Flex::Center).areas(row);
    cell
}
