//! The screen: what has the focus, what a key does there, and drawing it.
//!
//! [`App`] is a state machine with no I/O. Keys and service events go in;
//! drawing and a list of [`Effect`]s for the event loop come out. That is
//! what lets the tests drive it exactly as the binary does.

pub mod board;
mod complete;
mod detail;
mod glyph;
mod help;
pub mod input;
pub mod picker;
mod rich;
mod sources;
mod text;

use crate::config::{PickerLayout, Swimlanes, UiConfig};
use crate::keymap::{Action, Context, Key, Keymap, Layers, Outcome, Resolver};
use crate::theme::{self, Theme};
use board::{BoardView, DrawCtx};
use detail::DetailView;
use input::Input;
use kolum_app::{Command, Event, IssueDetail};
use kolum_domain::{IssueKey, User};
use picker::{AnyPicker, Pick, Picker, Waker};
use ratatui::Frame;
use ratatui::layout::{Constraint, Flex, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, Paragraph, Wrap};
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
    /// The title and the description, as `# Title` then Markdown, edited
    /// from `from`, the same: what moved is sent against it.
    Describe {
        key: IssueKey,
        from: String,
    },
    /// A new issue of this type in this project.
    Create {
        project: String,
        kind: kolum_domain::IssueType,
    },
    /// One rich text field, alone.
    Field {
        key: IssueKey,
        field: kolum_domain::EditableField,
    },
}

/// What waits on the read of an issue, to know its description as it is.
enum AfterRead {
    /// `e`: its editor opens.
    Describe,
    /// A description back from `$EDITOR`, edited from `from`: saved, unless
    /// it moved meanwhile.
    Save { from: String, text: String },
}

/// An issue's pictures, and the one on screen.
struct Viewer {
    pictures: Vec<kolum_domain::Attachment>,
    at: usize,
}

/// A row of `E`'s picker.
#[derive(Debug, Clone, PartialEq)]
pub enum FieldEntry {
    /// Through the transitions, as `t` does.
    Status,
    Field(kolum_domain::EditableField),
}

/// How long after a change of yours the board syncs: time for Jira's
/// automations to run on it.
const AFTER_WRITE: std::time::Duration = std::time::Duration::from_secs(5);

/// How often the picker's spinner turns.
const SPIN: std::time::Duration = std::time::Duration::from_millis(80);

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
    /// A value for the field `App::field_prompt` names.
    Field,
}

struct Prompt {
    kind: PromptKind,
    input: Input,
    /// The line the proposals are for: what was typed, not what Tab wrote.
    typed: String,
    /// The proposal Tab last wrote in.
    chosen: Option<usize>,
    hidden: bool,
    themes: Vec<String>,
}

impl Prompt {
    fn new(kind: PromptKind, input: Input, themes: Vec<String>) -> Self {
        let typed = input.text().to_string();
        Prompt { kind, input, typed, chosen: None, hidden: false, themes }
    }

    fn proposals(&self) -> (usize, Vec<complete::Proposal>) {
        match self.kind {
            PromptKind::Command => complete::proposals(&self.typed, &self.themes),
            PromptKind::Filter | PromptKind::Field => (0, vec![]),
        }
    }

    /// Proposals for the line as it now is.
    fn retype(&mut self) {
        self.typed = self.input.text().to_string();
        self.chosen = None;
        self.hidden = false;
    }

    /// Writes in the next or the previous proposal; a lone one is taken at
    /// once, so what follows it is proposed. Returns the theme written in.
    fn walk(&mut self, step: isize) -> Option<String> {
        let (start, list) = self.proposals();
        let n = list.len() as isize;
        if n == 0 {
            return None;
        }
        let i = match self.chosen {
            Some(i) => (i as isize + step).rem_euclid(n),
            None if step > 0 => 0,
            None => n - 1,
        } as usize;
        let is_theme = start > 0;
        self.input.set(&format!("{}{}", &self.typed[..start], list[i].text));
        self.hidden = false;
        if n == 1 {
            self.retype()
        } else {
            self.chosen = Some(i)
        }
        is_theme.then(|| list[i].text.clone())
    }
}

/// What has the focus, topmost first.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Focus {
    Problems,
    Help,
    Picker,
    Prompt,
    Viewer,
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
    /// `ui.filter` is one applied in an earlier run, not the config's.
    pub filter_applied: bool,
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
    /// The field the bottom line is typing a value for.
    field_prompt: Option<(IssueKey, kolum_domain::EditableField)>,
    /// `gi`'s pictures, over the issue.
    viewer: Option<Viewer>,
    /// The help page's `/` filter.
    help_filter: String,
    /// The sprint field waiting for the board's sprints.
    sprint_for: Option<(IssueKey, kolum_domain::EditableField)>,
    /// The board as it was asked for: its cache goes by that.
    board_asked: Option<kolum_domain::BoardId>,
    /// When the board next syncs on its own.
    sync_due: Option<Instant>,
    /// The field layout of each project's issue types, by `layout_key`: an
    /// issue opens with its type's before its own arrive.
    layouts: std::collections::BTreeMap<String, kolum_domain::FieldLayout>,
    /// The last full read of each issue, opened or prefetched: it opens from
    /// there, comments and history included, rather than from the board's
    /// copy, which lacks fields.
    read: std::collections::HashMap<IssueKey, std::sync::Arc<IssueDetail>>,
    /// Waiting on a read of this issue: `e` with none as new as the board's
    /// copy, or a description's save, which reads the issue right before.
    describe_after: Option<(IssueKey, AfterRead)>,
    /// Descriptions not saved, as their read failed: `from` and the text,
    /// which the next `e` on the issue opens.
    drafts: std::collections::HashMap<IssueKey, (String, String)>,
    /// The saves of `e` on their way, by write: `from` and the text, kept
    /// as a draft if the provider refuses them.
    saving: std::collections::HashMap<u64, (String, String)>,
    remote_debounce: std::time::Duration,
    /// The board filter is one applied, kept for the next run; until then
    /// the config's starts each.
    filter_applied: bool,
    /// The time of the key being handled, for the debounce.
    now: Instant,
    /// The number of the last board request sent.
    board_request: u64,
    /// The board as the provider last sent it; `board` shows it with the
    /// writes in flight on top.
    latest: Option<kolum_app::BoardSnapshot>,
    /// The writes not yet seen in a board the provider sent.
    pending: kolum_app::Pending,
    /// A new issue, highlighted once a sync brings it to the board.
    /// With the last board asked for then: the next one says.
    created: Option<(IssueKey, u64)>,
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
        board.filter = settings.ui.filter.clone();
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
            effects: vec![Effect::Send(Command::LoadBoard(settings.board.clone(), 1))],
            board_request: 1,
            board_asked: settings.board.clone(),
            sync_due: None,
            waker,
            board_cached: false,
            search_id: 0,
            search_due: None,
            field_prompt: None,
            sprint_for: None,
            viewer: None,
            help_filter: String::new(),
            layouts: Default::default(),
            read: Default::default(),
            describe_after: None,
            drafts: Default::default(),
            saving: Default::default(),
            remote_debounce: settings.remote_debounce,
            filter_applied: settings.filter_applied,
            now: Instant::now(),
            latest: None,
            pending: kolum_app::Pending::default(),
            created: None,
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
    /// times out, a debounced search goes out, or the spinner turns.
    pub fn deadline(&self) -> Option<Instant> {
        let spin = self.picker.as_ref().filter(|p| p.is_loading()).map(|_| self.now + SPIN);
        [self.resolver.deadline(), self.search_due, spin, self.sync_due].into_iter().flatten().min()
    }

    fn load_board(&mut self, id: kolum_domain::BoardId) {
        self.board_asked = Some(id.clone());
        self.ask_board(|n| Command::LoadBoard(Some(id), n));
    }

    /// A board request, under the next number.
    fn ask_board(&mut self, command: impl FnOnce(u64) -> Command) {
        self.board_request += 1;
        self.effects.push(Effect::Send(command(self.board_request)));
    }

    /// The next sync on its own, `sync_secs` from `now`; none at 0.
    fn next_sync(&self, now: Instant) -> Option<Instant> {
        let every = self.ui.sync_secs;
        (every > 0).then(|| now + std::time::Duration::from_secs(every))
    }

    /// What changed on the board; a change of yours on its way shows over it.
    fn sync_board(&mut self, now: Instant) {
        let id = self.board_asked.clone();
        self.ask_board(|n| Command::SyncBoard(id, n));
        self.sync_due = self.next_sync(now);
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
        self.detail.as_ref().map(|d| &d.issue().key)
    }

    fn focus(&self) -> Focus {
        if !self.problems.is_empty() {
            Focus::Problems
        } else if self.prompt.is_some() {
            // Over the help page too: `/` there filters it.
            Focus::Prompt
        } else if self.help.is_some() {
            Focus::Help
        } else if self.picker.is_some() {
            Focus::Picker
        } else if self.viewer.is_some() {
            Focus::Viewer
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
            Focus::Viewer => (vec![Context::Viewer, Context::Global], false),
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
        if let Some(p) = &mut self.picker {
            p.spin();
        }
        if self.search_due.is_some_and(|due| now >= due) {
            self.search();
        }
        if self.sync_due.is_some_and(|due| now >= due) {
            self.sync_board(now);
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
                    picker.set_loading(true);
                    self.search_due = Some(self.now + self.remote_debounce);
                }
            }
        } else if let Some(prompt) = &mut self.prompt
            && prompt.input.handle(key)
        {
            prompt.retype();
            if prompt.kind == PromptKind::Filter {
                let text = prompt.input.text().to_string();
                self.set_filter(text);
            }
        }
    }

    pub fn on_event(&mut self, event: Event) {
        let waited = match &event {
            Event::IssueLoaded(detail) => self.keep_read(detail),
            _ => None,
        };
        match event {
            Event::FieldLayouts(kept) => self.layouts.extend(kept),
            // The copies that won, which the service decides.
            Event::ReadIssues(read) => {
                self.read.extend(read.into_iter().map(|d| (d.issue.key.clone(), d)));
            }
            Event::Me(user) => {
                self.board.me = Some(user.id.clone());
                self.me = Some(user);
                self.board.clamp();
            }
            Event::BoardLoaded { snapshot, fresh, request } => {
                self.board_loaded(snapshot, fresh, request)
            }
            // In place of the open issue, if any: `gp` waits on the parent there.
            Event::IssueLoaded(detail) if self.opening.as_ref() == Some(&detail.issue.key) => {
                self.opening = None;
                self.said_done(&opening(&detail.issue.key));
                let mut view = DetailView::new(detail.issue.clone());
                view.loaded = Some(detail);
                self.detail = Some(view);
                self.show_detail();
                self.want_avatar();
            }
            Event::IssueLoaded(detail) => {
                // A read slower than a write's reply does not undo it.
                if let Some(d) = &mut self.detail
                    && d.latest.key == detail.issue.key
                    && kolum_app::supersedes(&detail.issue, &d.latest)
                {
                    d.latest = detail.issue.clone();
                    d.loaded = Some(detail);
                    self.show_detail();
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
            Event::EditableFields { key, result } => self.open_field_picker(key, result),
            Event::Sprints(result) => self.open_sprint_picker(result),
            Event::Written { op, key, issue } => self.written(op, key, issue.as_deref()),
            Event::WriteFailed { op, key, error } => self.write_failed(op, key, error),
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
            // On the board if its query takes it in, which the sync says.
            Event::Created(issue) => {
                self.sync_soon();
                self.say(format!("Created {}", issue.key), Level::Info);
                self.created = Some((issue.key, self.board_request));
            }
            Event::IssueLoadFailed { key, error } => self.read_failed(key, error),
            Event::Failed { what, error } => {
                self.say(format!("{what} failed: {error}"), Level::Error)
            }
        }
        // Once the detail shows the read: a save shows over it.
        if let Some((key, after)) = waited {
            self.read_done(key, after);
        }
    }

    /// The provider's board, the cached copy or a fresh one: what is
    /// shown is it, with the writes in flight on top.
    fn board_loaded(&mut self, snapshot: kolum_app::BoardSnapshot, fresh: bool, request: u64) {
        self.board_cached = !fresh;
        if fresh {
            self.fresh_board(request);
        }
        let snapshot = kolum_app::land(self.latest.as_ref(), snapshot);
        if let Some(d) = &mut self.detail
            && let Some(copy) = snapshot.issues.iter().find(|i| i.key == d.latest.key)
            && kolum_app::supersedes(copy, &d.latest)
        {
            kolum_app::refresh(&mut d.latest, copy);
        }
        self.latest = Some(snapshot);
        self.show();
        // The first board asked for after it says whether the new issue is
        // on the board; a cached one, older than any, says nothing.
        if let Some((key, _)) = self.created.take_if(|(_, asked)| fresh && request > *asked)
            && self.board.has(&key)
        {
            self.board.select(&key);
        }
    }

    /// A board from the provider, not from the cache, answering `request`:
    /// the writes it holds are settled, and the next sync is due.
    fn fresh_board(&mut self, request: u64) {
        self.said_done(REFRESHING);
        self.pending.landed(request);
        if self.sync_due.is_none() {
            self.sync_due = self.next_sync(self.now);
        }
    }

    /// An issue read whole, kept; and what waited on its read, if anything.
    fn keep_read(&mut self, detail: &std::sync::Arc<IssueDetail>) -> Option<(IssueKey, AfterRead)> {
        if let Some(layout) = &detail.layout {
            self.layouts.insert(kolum_app::layout_key(&detail.issue), layout.clone());
        }
        self.read.insert(detail.issue.key.clone(), detail.clone());
        self.describe_after.take_if(|(key, _)| *key == detail.issue.key)
    }

    /// What waited on the read of this issue goes on.
    fn read_done(&mut self, key: IssueKey, after: AfterRead) {
        self.said_done(&reading(&key));
        match after {
            AfterRead::Describe => self.describe(key),
            AfterRead::Save { from, text } => self.save_read(key, from, text),
        }
    }

    /// A read that failed: nothing waits on it any more, and a description
    /// waiting on it to be saved is kept as a draft, for the next `e`.
    fn read_failed(&mut self, key: IssueKey, error: kolum_domain::DomainError) {
        self.say(format!("loading {key} failed: {error}"), Level::Error);
        self.opening.take_if(|opening| *opening == key);
        let Some((_, AfterRead::Save { from, text })) =
            self.describe_after.take_if(|(waiting, _)| *waiting == key)
        else {
            return;
        };
        let why = format!("loading {key} failed: {error}; not saved, kept as a draft for `e`");
        self.say(why, Level::Error);
        self.keep_draft(key, (from, text));
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

    /// `said` goes once what it waited for has come, unless something else
    /// was said since.
    fn said_done(&mut self, said: &str) {
        if self.message.as_ref().is_some_and(|(text, _)| text == said) {
            self.message = None;
        }
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
                let themes = self.theme_names();
                self.prompt = Some(Prompt::new(PromptKind::Command, Input::default(), themes))
            }
            Action::Refresh => {
                let id = self.board.snapshot.as_ref().map(|s| s.board.id.clone());
                self.ask_board(|n| Command::ReloadBoard(id, n));
                self.say(REFRESHING, Level::Info);
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
                Focus::Viewer => self.viewer_action(action, n),
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
                self.undo_theme_preview();
                if self.prompt.take().is_some_and(|p| p.kind == PromptKind::Filter) {
                    self.set_filter(String::new());
                    if self.help.is_none() {
                        self.save_filter();
                    }
                }
            }
            Focus::Viewer => self.viewer = None,
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
                self.save_filter();
            }
            Action::Filter => {
                let mut input = Input::default();
                input.set(&b.filter);
                self.prompt = Some(Prompt::new(PromptKind::Filter, input, vec![]));
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
            Action::OpenParent => {
                if let Some(issue) = b.selected().cloned() {
                    self.open_parent(&issue);
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
        self.save_filter();
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
            applied_filter: self.filter_applied.then(|| self.board.filter.clone()),
        };
        self.effects.push(Effect::SaveState(state));
    }

    /// The board filter applied, or cleared: from now on it is kept.
    fn save_filter(&mut self) {
        self.filter_applied = true;
        self.save_state();
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
                let issue = d.issue().clone();
                self.issue_effect(action, &issue);
            }
            Action::ViewPictures => self.open_viewer(),
            Action::OpenParent => {
                let issue = d.issue().clone();
                self.open_parent(&issue);
            }
            _ => {
                let keys = vec![d.issue().key.clone()];
                self.issue_action(action, keys);
            }
        }
    }

    /// The `/` line filters the help page when it is open, the board otherwise.
    fn set_filter(&mut self, text: String) {
        if self.help.is_some() {
            self.help_filter = text;
            self.help = Some(0);
        } else {
            self.board.filter = text;
            self.board.clamp();
        }
    }

    fn help_action(&mut self, action: Action, n: i64) {
        match action {
            Action::Filter => {
                let mut input = Input::default();
                input.set(&self.help_filter);
                self.prompt = Some(Prompt::new(PromptKind::Filter, input, vec![]));
                return;
            }
            // Clears the filter first, then closes the page.
            Action::ClearFilter if self.help_filter.is_empty() => return self.help = None,
            Action::ClearFilter => return self.set_filter(String::new()),
            _ => {}
        }
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
        let max = help::lines(&self.keymap, &self.theme, &self.help_filter)
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
                | Action::EditField
                | Action::Clone
        );
        if keys.is_empty() || (writes && !self.can_write()) {
            return;
        }
        match action {
            Action::Transition => self.ask_transitions(keys, None),
            Action::Assign => self.open_user_picker(keys, None),
            Action::AssignMe => match self.me.clone() {
                Some(me) => self.assign(keys, Some(me)),
                None => self.say("Still finding out who you are", Level::Warn),
            },
            Action::Comment => self.edit(EditPurpose::Comment(keys[0].clone())),
            Action::Edit => self.describe(keys[0].clone()),
            Action::EditField => {
                self.effects.push(Effect::Send(Command::EditableFields(keys[0].clone())));
            }
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
        on_board.or(self.detail.as_ref().map(|d| d.issue()).filter(|i| &i.key == key))
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
        self.write(vec![(
            key.clone(),
            kolum_app::Change::Rank(rank.clone()),
            kolum_app::Write::Rank(rank),
        )]);
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
        if let Some(why) = kolum_app::asks_for(&t) {
            return self.say(format!("{why}: press o to do it in the browser"), Level::Warn);
        }
        // The transitions are those of the first.
        let status = |app: &Self, key| app.issue(key).map(|i| i.status.id.clone());
        let first = status(self, &keys[0]);
        let writes = keys.iter().filter_map(|key| {
            let write = kolum_app::transition_write(&t, first.as_ref(), status(self, key).as_ref());
            write.map(|write| (key.clone(), kolum_app::Change::Status(t.to.clone()), write))
        });
        self.write(writes.collect());
        let names: Vec<&str> = keys.iter().map(|k| k.as_str()).collect();
        self.say(format!("{} → {}", names.join(", "), t.to.name), Level::Info);
    }

    /// Who an issue can go to: me, then whoever is on the board.
    /// People to assign the issues to, or to put in `field`.
    fn open_user_picker(
        &mut self,
        keys: Vec<IssueKey>,
        field: Option<kolum_domain::EditableField>,
    ) {
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
        let source = sources::UserChoice { keys, users: choices, field };
        self.open_picker(Box::new(Picker::new(source, self.waker.clone())));
    }

    fn assign(&mut self, keys: Vec<IssueKey>, user: Option<kolum_domain::User>) {
        let write = kolum_app::Write::Assign(user.as_ref().map(|u| u.id.clone()));
        let change = kolum_app::Change::Assignee(user.clone());
        self.write(keys.iter().map(|key| (key.clone(), change.clone(), write.clone())).collect());
        let who = user.as_ref().map_or("nobody".to_string(), |u| u.display_name.clone());
        self.say(format!("Assigned to {who}"), Level::Info);
    }

    /// Sends the writes of one action, each shown at once as its change
    /// until the provider answers, on the board or off it; the first card
    /// highlighted. Their ops, in order.
    fn write(&mut self, writes: Vec<(IssueKey, kolum_app::Change, kolum_app::Write)>) -> Vec<u64> {
        let first = writes.first().map(|(key, ..)| key.clone());
        let ops = writes.into_iter().map(|(key, change, write)| {
            let op = self.pending.begin(key.clone(), change);
            self.effects.push(Effect::Send(Command::Write { op, key, write }));
            op
        });
        let ops = ops.collect();
        self.show();
        if let Some(key) = first {
            self.board.select(&key);
        }
        ops
    }

    /// The provider took a write: it joins the provider's copies, under
    /// the issue read back after, and the open issue is read again.
    fn written(&mut self, op: u64, key: IssueKey, read: Option<&kolum_domain::Issue>) {
        self.pending.written(op, read.is_some(), self.board_request);
        self.sync_soon();
        self.saving.remove(&op);
        if let (Some(latest), Some(read)) = (&mut self.latest, read) {
            kolum_app::written(latest, read);
        }
        if let Some(d) = self.detail.as_mut().filter(|d| d.latest.key == key) {
            // The issue read whole, as an opening read is.
            if let Some(read) = read.filter(|read| kolum_app::supersedes(read, &d.latest)) {
                d.latest = read.clone();
            }
            self.effects.push(Effect::Send(Command::LoadIssue(key.clone())));
        }
        self.show();
        self.say(format!("{key} saved"), Level::Info);
    }

    /// The provider refused a write: it is no longer shown, and the text of
    /// an `e` is kept as a draft, for the next `e`.
    fn write_failed(&mut self, op: u64, key: IssueKey, error: kolum_domain::DomainError) {
        self.pending.refused(op);
        self.show();
        let draft = match self.saving.remove(&op) {
            Some(draft) => {
                self.keep_draft(key.clone(), draft);
                "; yours kept as a draft for `e`"
            }
            None => "",
        };
        self.say(format!("{key}: {error}; undone{draft}"), Level::Error);
    }

    /// A draft of the issue, `from` and the text, over one kept already:
    /// what either typed survives.
    fn keep_draft(&mut self, key: IssueKey, draft: (String, String)) {
        let draft = match self.drafts.remove(&key) {
            Some(kept) => merge_drafts(&kept, &draft),
            None => draft,
        };
        self.drafts.insert(key, draft);
    }

    /// Jira's automations act on a change now: sync once they have.
    fn sync_soon(&mut self) {
        let soon = Instant::now() + AFTER_WRITE;
        self.sync_due = Some(self.sync_due.map_or(soon, |due| due.min(soon)));
    }

    /// What is on screen: the provider's latest board and open issue, with
    /// the writes in flight on top. [`App::show_board`] and
    /// [`App::show_detail`] are the one place each is written.
    fn show(&mut self) {
        self.show_board();
        self.show_detail();
    }

    fn show_board(&mut self) {
        if let Some(latest) = &self.latest {
            self.board.set_snapshot(self.pending.over(latest));
        }
    }

    fn show_detail(&mut self) {
        if let Some(d) = &mut self.detail {
            d.show(&self.pending);
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
            .or_else(|| selected.and_then(|i| i.key.project()).map(String::from));
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

    /// `E`'s picker: the status, then every field the provider lets set,
    /// each with its value; the description alone when it cannot say.
    fn open_field_picker(
        &mut self,
        key: IssueKey,
        result: kolum_domain::Result<Vec<kolum_domain::EditableField>>,
    ) {
        let fields = match result {
            Ok(fields) => fields,
            Err(kolum_domain::DomainError::Unsupported(_)) => vec![kolum_domain::EditableField {
                id: kolum_domain::FieldId::new(kolum_domain::FieldId::DESCRIPTION),
                provider_id: kolum_domain::FieldId::new(kolum_domain::FieldId::DESCRIPTION),
                name: "Description".into(),
                kind: kolum_domain::FieldKind::RichText,
                required: false,
                on_issue: true,
            }],
            Err(e) => return self.say(format!("Reading the fields of {key}: {e}"), Level::Error),
        };
        let Some(issue) = self.full_issue(&key) else { return };
        let plain = |s: String| kolum_domain::RichText {
            blocks: vec![kolum_domain::rich_text::Block::Paragraph(vec![
                kolum_domain::rich_text::Inline::text(s),
            ])],
            lossy: false,
        };
        let mut rows =
            vec![(FieldEntry::Status, "Status".to_string(), plain(issue.status.name.clone()))];
        rows.extend(fields.into_iter().map(|f| {
            let shown = match issue.value(&f.id) {
                Some(kolum_domain::FieldValue::RichText(text)) => text,
                Some(value) => plain(value.to_input()),
                None => plain("None".into()),
            };
            (FieldEntry::Field(f.clone()), f.name, shown)
        }));
        let source = sources::FieldChoice { key, fields: rows };
        self.open_picker(Box::new(Picker::new(source, self.waker.clone())));
    }

    /// The board's sprints for the sprint field `E` picked, and `None` to
    /// take the issue out of its sprint.
    fn open_sprint_picker(&mut self, result: kolum_domain::Result<Vec<kolum_domain::Sprint>>) {
        let Some((key, field)) = self.sprint_for.take() else { return };
        let sprints = match result {
            Ok(sprints) => sprints,
            Err(e) => return self.say(format!("The board's sprints: {e}"), Level::Error),
        };
        let mut choices: Vec<Option<kolum_domain::Sprint>> =
            sprints.into_iter().map(Some).collect();
        choices.push(None);
        let source = sources::SprintChoice { key, field, sprints: choices };
        self.open_picker(Box::new(Picker::new(source, self.waker.clone())));
    }

    /// The editor a field's kind calls for.
    fn edit_field(&mut self, key: IssueKey, entry: FieldEntry) {
        let field = match entry {
            FieldEntry::Status => return self.ask_transitions(vec![key], None),
            FieldEntry::Field(field) => field,
        };
        match &field.kind {
            kolum_domain::FieldKind::RichText => self.edit(EditPurpose::Field { key, field }),
            kolum_domain::FieldKind::User
                if field.id.as_str() == kolum_domain::FieldId::ASSIGNEE =>
            {
                self.open_user_picker(vec![key], None)
            }
            kolum_domain::FieldKind::User => self.open_user_picker(vec![key], Some(field)),
            kolum_domain::FieldKind::Choice { options, .. } => {
                let options = options.clone();
                self.open_option_picker(key, field, options)
            }
            kolum_domain::FieldKind::Sprint => self.ask_sprints(key, field),
            _ => self.ask_field_line(key, field),
        }
    }

    /// The field's options, and none when it may be emptied.
    fn open_option_picker(
        &mut self,
        key: IssueKey,
        field: kolum_domain::EditableField,
        options: Vec<String>,
    ) {
        let mut choices: Vec<Option<String>> = options.into_iter().map(Some).collect();
        if !field.required {
            choices.push(None);
        }
        let source = sources::OptionChoice { key, field, options: choices };
        self.open_picker(Box::new(Picker::new(source, self.waker.clone())));
    }

    /// The board's sprints, for the sprint picker of `field`.
    fn ask_sprints(&mut self, key: IssueKey, field: kolum_domain::EditableField) {
        let Some(board) = self.board.snapshot.as_ref().map(|s| s.board.id.clone()) else {
            return;
        };
        self.effects.push(Effect::Send(Command::Sprints(board)));
        self.sprint_for = Some((key, field));
    }

    /// The bottom line, holding what the field holds now.
    fn ask_field_line(&mut self, key: IssueKey, field: kolum_domain::EditableField) {
        let mut input = Input::default();
        let current = self.full_issue(&key).and_then(|i| i.value(&field.id));
        input.set(&current.map(|v| v.to_input()).unwrap_or_default());
        self.prompt = Some(Prompt::new(PromptKind::Field, input, vec![]));
        self.field_prompt = Some((key, field));
    }

    /// The bottom line's value for the field it was opened for.
    fn confirm_field(&mut self, text: &str) {
        let Some((key, field)) = self.field_prompt.take() else { return };
        match field.kind.parse(text) {
            Ok(value) => {
                let issue = field.kind == kolum_domain::FieldKind::Issue;
                let value = value.map(|v| if issue { self.as_board_issue(v) } else { v });
                self.set_field(key, &field, value)
            }
            Err(e) => self.say(e, Level::Warn),
        }
    }

    /// An issue typed by its key, whole when the board has it, so it shows
    /// at once; the key alone otherwise, which the read-back fills.
    fn as_board_issue(&self, value: kolum_domain::FieldValue) -> kolum_domain::FieldValue {
        let kolum_domain::FieldValue::Text(key) = &value else { return value };
        let key = IssueKey::parse(key.as_str());
        let issues = self.board.snapshot.iter().flat_map(|s| &s.issues);
        match issues.into_iter().find(|i| i.key == key) {
            Some(issue) => kolum_domain::FieldValue::Issue(issue.as_ref()),
            None => value,
        }
    }

    /// One field set to `value`, or emptied, unless it is required.
    fn set_field(
        &mut self,
        key: IssueKey,
        field: &kolum_domain::EditableField,
        value: Option<kolum_domain::FieldValue>,
    ) {
        if value.is_none() && field.required {
            return self.say(format!("{} is required", field.name), Level::Warn);
        }
        self.save(key, kolum_domain::IssueUpdate::set(field, value));
    }

    /// The issue whole, from its last full read brought up to the board's
    /// copy: see `kolum_app::in_full`.
    fn issue_in_full(&self, key: &IssueKey) -> Option<kolum_domain::Issue> {
        kolum_app::in_full(&self.read.get(key)?.issue, self.issue(key))
    }

    fn full_issue(&self, key: &IssueKey) -> Option<&kolum_domain::Issue> {
        let open = self.detail.as_ref().map(|d| d.issue()).filter(|i| &i.key == key);
        open.or_else(|| self.issue(key))
    }

    fn edit(&mut self, purpose: EditPurpose) {
        if let EditPurpose::Field { key, field } = &purpose {
            let Some(issue) = self.full_issue(key) else { return };
            let rich = rich_value(issue, &field.id);
            let name = &field.name;
            let flattened = if rich.lossy { LOSSY } else { "" };
            let text = format!(
                "{}\n\n{MARKER} {name} of {}, in Markdown above this line. {flattened}Empty or unchanged leaves it as it is. -->\n",
                rich.to_markdown().trim(),
                issue.key
            );
            self.effects.push(Effect::Edit { purpose, text });
            return;
        }
        let EditPurpose::Comment(key) = &purpose else { return };
        let Some(issue) = self.issue(key) else { return };
        let text = format!(
            "\n\n{MARKER} a comment on {}: {}. Markdown above this line; leave it empty to cancel. -->\n",
            issue.key, issue.title
        );
        self.effects.push(Effect::Edit { purpose, text });
    }

    /// `e`: the title and the description in `$EDITOR`, from the issue read
    /// whole, read first when no read is as new as the board's copy.
    fn describe(&mut self, key: IssueKey) {
        let saving = match &self.describe_after {
            Some((saving, AfterRead::Save { .. })) => Some(saving.clone()),
            _ => self.pending.saves_description(&key).then(|| key.clone()),
        };
        if let Some(saving) = saving {
            return self.say(format!("{saving} is still being saved: `e` once it is"), Level::Warn);
        }
        if let Some((from, text)) = self.drafts.remove(&key) {
            let text = format!(
                "{text}\n{MARKER} {key}: the draft not saved. The first `# ` line is the title, the rest the description, in Markdown. -->\n"
            );
            let purpose = EditPurpose::Describe { key, from };
            return self.effects.push(Effect::Edit { purpose, text });
        }
        match self.issue_in_full(&key) {
            Some(issue) => self.edit_description(&issue, None),
            None => self.read_then(key, AfterRead::Describe),
        }
    }

    fn read_then(&mut self, key: IssueKey, after: AfterRead) {
        self.say(reading(&key), Level::Info);
        self.effects.push(Effect::Send(Command::LoadIssue(key.clone())));
        self.describe_after = Some((key, after));
    }

    /// `$EDITOR` on the issue's title and description, or on `mine` in
    /// their place, theirs then shown below it.
    fn edit_description(&mut self, issue: &kolum_domain::Issue, mine: Option<&str>) {
        let from = titled(issue);
        let flattened = if issue.description_or_empty().lossy { LOSSY } else { "" };
        let key = issue.key.clone();
        let help = format!(
            "{MARKER} {key}: the first `# ` line is the title, the rest the description, in Markdown. {flattened}-->\n"
        );
        let text = match mine {
            None => format!("{from}\n{help}"),
            Some(mine) => format!("{mine}\n{help}\nAs it is now, changed meanwhile:\n\n{from}\n"),
        };
        self.effects.push(Effect::Edit { purpose: EditPurpose::Describe { key, from }, text });
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
                // A comment changes nothing on the card.
                let change = kolum_app::Change::Update(kolum_domain::IssueUpdate::default());
                let write = kolum_app::Write::Comment(kolum_domain::RichText::from_markdown(&text));
                self.say(format!("Commenting on {key}…"), Level::Info);
                self.write(vec![(key, change, write)]);
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
            EditPurpose::Field { key, field } => self.save_field(key, field, &text),
            EditPurpose::Describe { key, from } => self.save_description(key, from, text),
        }
    }

    /// An edit shown at once and sent; undone if the provider refuses it.
    /// The write's op.
    fn save(&mut self, key: IssueKey, update: kolum_domain::IssueUpdate) -> u64 {
        self.say(format!("Saving {key}…"), Level::Info);
        let change = kolum_app::Change::Update(update.clone());
        self.write(vec![(key, change, kolum_app::Write::Update(update))])[0]
    }

    /// The title and the description back from `$EDITOR`. A description is
    /// written only over the one the editing began from: the issue is read
    /// right before, to know.
    fn save_description(&mut self, key: IssueKey, from: String, text: String) {
        let update = description_update(&from, &text);
        if update == kolum_domain::IssueUpdate::default() {
            return self.say("No change", Level::Info);
        }
        if update.description.is_none() {
            return self.save_typed(key, from, text);
        }
        self.read_then(key, AfterRead::Save { from, text });
    }

    /// The description saved, now the issue is read: one that moved since
    /// the editing began goes back to the editor, with the text.
    fn save_read(&mut self, key: IssueKey, from: String, text: String) {
        let Some(issue) = self.read.get(&key).map(|d| d.issue.clone()) else { return };
        if issue.description_or_empty().to_markdown().trim() != split_title(&from).1 {
            self.edit_description(&issue, Some(&text));
            let why = format!("{key}: the description changed meanwhile; yours is in the editor");
            return self.say(why, Level::Warn);
        }
        self.save_typed(key, from, text);
    }

    /// What `e` saves: only what moved since `from`, which a refusal keeps.
    fn save_typed(&mut self, key: IssueKey, from: String, text: String) {
        let op = self.save(key, description_update(&from, &text));
        self.saving.insert(op, (from, text));
    }

    /// One rich text field back from `$EDITOR`: empty or unchanged leaves it.
    fn save_field(&mut self, key: IssueKey, field: kolum_domain::EditableField, text: &str) {
        if text.is_empty() {
            return self.say("Nothing written: the field stays as it is", Level::Info);
        }
        let Some(issue) = self.full_issue(&key) else { return };
        if text == rich_value(issue, &field.id).to_markdown().trim() {
            return self.say("No change", Level::Info);
        }
        let value = kolum_domain::FieldValue::RichText(kolum_domain::RichText::from_markdown(text));
        self.set_field(key, &field, Some(value));
    }

    /// The pictures of the open issue: its assignee's avatar, and its
    /// first image attachment.
    fn pictures(&self) -> [Option<String>; 2] {
        let Some(d) = &self.detail else { return [None, None] };
        let avatar = d.issue().assignee.as_ref().and_then(|a| a.avatar_url.clone());
        let shot = d.issue().attachments.iter().find(|a| a.is_image()).map(|a| a.url.clone());
        [avatar, shot]
    }

    /// Asks for the open issue's pictures, once per URL, when pictures can
    /// be drawn here.
    fn want_avatar(&mut self) {
        for url in self.pictures().into_iter().flatten() {
            self.want_picture(url);
        }
    }

    /// Fetched once, when this terminal can draw pictures at all.
    fn want_picture(&mut self, url: String) {
        if self.images.is_some() && !self.avatars.contains_key(&url) {
            self.avatars.insert(url.clone(), Avatar::Loading);
            self.effects.push(Effect::Send(Command::Image(url)));
        }
    }

    /// `gi`: the open issue's pictures, full screen, from the first.
    fn open_viewer(&mut self) {
        let Some(d) = &self.detail else { return };
        if self.images.is_none() {
            return self.say("Pictures are off in this terminal", Level::Warn);
        }
        let pictures: Vec<_> =
            d.issue().attachments.iter().filter(|a| a.is_image()).cloned().collect();
        if pictures.is_empty() {
            return self.say(format!("No pictures in {}", d.issue().key), Level::Info);
        }
        let url = pictures[0].url.clone();
        self.viewer = Some(Viewer { pictures, at: 0 });
        self.want_picture(url);
    }

    /// The keys mean what they do elsewhere, for the picture shown.
    fn viewer_action(&mut self, action: Action, n: i64) {
        let Some(v) = &mut self.viewer else { return };
        let delta = match action {
            Action::Left => -n,
            Action::Right => n,
            Action::OpenBrowser => {
                let url = v.pictures[v.at].url.clone();
                return self.effects.push(Effect::Open(url));
            }
            Action::CopyUrl => {
                let url = v.pictures[v.at].url.clone();
                self.say(format!("Copied {url}"), Level::Info);
                return self.effects.push(Effect::Copy(url));
            }
            _ => return,
        };
        v.at = (v.at as i64 + delta).clamp(0, v.pictures.len() as i64 - 1) as usize;
        let url = v.pictures[v.at].url.clone();
        self.want_picture(url);
    }

    /// The picture `gi` shows, scaled to the screen, under its name.
    fn draw_viewer(&mut self, frame: &mut Frame, area: Rect, theme: &Theme) {
        let Some(v) = &self.viewer else { return };
        let file = &v.pictures[v.at];
        let title = format!(
            "{} · {} · {}/{}",
            file.name,
            detail::size(file.size),
            v.at + 1,
            v.pictures.len()
        );
        let block = popup(&title, theme)
            .title_bottom(Line::from(" h/l · o browser · yu URL · q back ").right_aligned());
        let inner = block.inner(area);
        let url = file.url.clone();
        frame.render_widget(Clear, area);
        frame.render_widget(block, area);
        let note = match self.avatars.get_mut(&url) {
            Some(Avatar::Ready(protocol)) => {
                let image =
                    ratatui_image::StatefulImage::<ratatui_image::protocol::StatefulProtocol>::new(
                    )
                    .resize(ratatui_image::Resize::Scale(None));
                return frame.render_stateful_widget(image, inner, protocol.as_mut());
            }
            Some(Avatar::Failed) => "This picture could not be drawn",
            _ => "Loading…",
        };
        let line = Line::styled(note, Style::new().fg(theme.muted)).centered();
        frame.render_widget(line, Rect { y: inner.y + inner.height / 2, height: 1, ..inner });
    }

    /// Opens at once from the board's copy, over what its last opening read
    /// if any; an issue not on the board (a remote search hit) opens when it
    /// has been read.
    fn open_detail(&mut self, key: IssueKey) {
        let issue = self.latest.as_ref().and_then(|s| s.issues.iter().find(|i| i.key == key));
        match issue {
            Some(issue) => {
                let view = match self.read.get(&key) {
                    Some(read) => {
                        let mut read = read.clone();
                        if kolum_app::supersedes(issue, &read.issue) {
                            kolum_app::refresh(
                                &mut std::sync::Arc::make_mut(&mut read).issue,
                                issue,
                            );
                        }
                        let mut view = DetailView::new(read.issue.clone());
                        view.loaded = Some(read);
                        view
                    }
                    None => DetailView::new(issue.clone()),
                };
                self.detail = Some(view);
                self.show_detail();
                self.opening = None;
                self.want_avatar();
            }
            None => {
                self.say(opening(&key), Level::Info);
                self.opening = Some(key.clone());
            }
        }
        self.effects.push(Effect::Send(Command::LoadIssue(key)));
    }
    /// `gp`: the issue's parent, an epic most often.
    fn open_parent(&mut self, issue: &kolum_domain::Issue) {
        match &issue.parent {
            Some(parent) => self.open_detail(parent.key.clone()),
            None => self.say(format!("{} has no parent", issue.key), Level::Info),
        }
    }

    fn open_picker(&mut self, picker: Box<dyn AnyPicker>) {
        self.close_picker();
        self.picker = Some(picker);
        self.history_pos = None;
    }

    /// The board's issues, each as read whole when it has been: the board's
    /// copy has no description on Jira Cloud.
    fn open_issue_picker(&mut self) {
        let board = self.board.snapshot.as_ref().map(|s| s.issues.as_slice()).unwrap_or_default();
        let issues =
            board.iter().map(|i| self.issue_in_full(&i.key).unwrap_or_else(|| i.clone())).collect();
        let me = self.me.as_ref().map(|u| u.id.clone());
        self.open_picker(Box::new(Picker::new(
            sources::IssueSource { issues, me, remote: None },
            self.waker.clone(),
        )));
    }

    /// The built-in themes, then the theme files.
    fn theme_names(&self) -> Vec<String> {
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
        names
    }

    fn open_theme_picker(&mut self) {
        let names = self.theme_names();
        self.open_picker(Box::new(Picker::new(sources::ThemeSource { names }, self.waker.clone())));
    }

    /// Keeps the picker for `picker_resume`, and undoes a theme preview
    /// that was not confirmed.
    fn close_picker(&mut self) {
        if let Some(p) = self.picker.take() {
            self.last_picker = Some(p);
        }
        self.undo_theme_preview();
    }

    fn undo_theme_preview(&mut self) {
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
        let mut query = kolum_app::parse_query(p.query());
        if let Some(snapshot) = &self.board.snapshot {
            query = snapshot.qualify(query);
        }
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
                p.set_loading(true);
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
                self.load_board(id);
            }
            Pick::EditField { key, entry } => {
                self.close_picker();
                self.edit_field(key, entry);
            }
            Pick::SetField { key, field, value } => {
                self.close_picker();
                self.set_field(key, &field, value);
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
        if let Some(Pick::Theme(name)) = self.picker.as_ref().and_then(|p| p.hovered()) {
            self.preview_theme(name);
        }
    }

    /// Until confirmed: closing puts the theme from before back.
    fn preview_theme(&mut self, name: String) {
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
        let Some(prompt) = &mut self.prompt else { return };
        match action {
            Action::Down | Action::Up => {
                if let Some(name) = prompt.walk(if action == Action::Down { 1 } else { -1 }) {
                    self.preview_theme(name);
                }
            }
            Action::AcceptProposal => prompt.retype(),
            Action::HideProposals => prompt.hidden = true,
            Action::Confirm => {
                let Some(prompt) = self.prompt.take() else { return };
                self.undo_theme_preview();
                if prompt.kind == PromptKind::Command {
                    self.run_command(prompt.input.text().trim());
                }
                if prompt.kind == PromptKind::Field {
                    self.confirm_field(prompt.input.text());
                }
                if prompt.kind == PromptKind::Filter && self.help.is_none() {
                    self.save_filter();
                }
            }
            _ => {}
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
                self.load_board(kolum_domain::BoardId::new(arg));
            }
            "jql" => {
                self.dispatch(Action::PickerRemote, None);
                if let Some(p) = self.picker.as_mut().filter(|_| !arg.is_empty()) {
                    p.set_query(&format!("!{arg}"));
                }
            }
            "assign" => self.say(":assign is not available yet", Level::Warn),
            // Any action, by the name its key binding gives it.
            _ => match Action::from_name(name) {
                Some(action) => self.dispatch(action, None),
                None => self.say(format!("Not a command: {name}"), Level::Error),
            },
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
            Some(d) => {
                let shown = detail::Shown {
                    fields: self.ui.detail_fields.as_deref(),
                    sections: self.ui.detail_sections.as_deref(),
                    layout: self.layouts.get(&kolum_app::layout_key(d.issue())),
                };
                d.draw(frame, main, &theme, shown, me, pictures)
            }
            None => {
                let [head, rule, body] = Layout::vertical([
                    Constraint::Length(1),
                    Constraint::Length(1),
                    Constraint::Min(0),
                ])
                .areas(main);
                let (header, lit) = self.board_header(&theme);
                frame.render_widget(Paragraph::new(header), head);
                frame.render_widget(Paragraph::new(logo(&theme)).right_aligned(), head);
                frame.render_widget(glyph::rule("━", rule.width as usize, lit, &theme), rule);
                let ctx = DrawCtx { theme: &theme, fields: &self.ui.card_fields, me };
                self.board.draw(frame, body, &ctx);
            }
        }
        self.draw_viewer(frame, main, &theme);
        if let Some(p) = &mut self.picker {
            p.draw(frame, centered(main, 90, 85), &theme, self.picker_layout);
        }
        if let Some(scroll) = self.help {
            let area = centered(main, 80, 90);
            let title = match self.help_filter.as_str() {
                "" => "Keys  (/ to search, j/k to scroll, q to close)".to_string(),
                filter => format!("Keys matching “{filter}”  (Esc to clear)"),
            };
            let block = popup(&title, &theme);
            self.help_page.set(block.inner(area).height);
            let lines = help::lines(&self.keymap, &theme, &self.help_filter);
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
        let lead_x = self.draw_status(frame, status, &theme);
        self.draw_proposals(frame, main, lead_x, &theme);
    }

    /// gh-dash's tab row: the board and its size, then the swimlanes as
    /// tabs `s` steps through, then what narrows or delays it.
    /// Also the cells of the active tab, for the rule under it.
    fn board_header(&self, theme: &Theme) -> (Line<'static>, std::ops::Range<usize>) {
        let mut spans = vec![];
        let mut lit = 0..0;
        if let Some(s) = &self.board.snapshot {
            spans.push(Span::styled(
                format!(" {} ", s.board.name),
                Style::new().fg(theme.accent).add_modifier(Modifier::BOLD),
            ));
            spans.push(Span::styled(
                format!("{:?} · {} issues ", s.board.kind, s.issues.len()),
                Style::new().fg(theme.muted),
            ));
            spans.push(Span::styled("│", Style::new().fg(theme.border)));
            let lanes =
                [Swimlanes::None, Swimlanes::Epic, Swimlanes::Assignee, Swimlanes::Priority];
            let active = lanes.iter().position(|&l| l == self.board.swimlanes).unwrap_or(0);
            let at = Line::from(spans.clone()).width();
            let (tabs, tab) =
                glyph::tabs(&["No lanes", "Epic", "Assignee", "Priority"], active, theme);
            spans.extend(tabs);
            lit = at + tab.start..at + tab.end;
            spans.push(Span::styled("│", Style::new().fg(theme.border)));
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
        (Line::from(spans), lit)
    }

    /// The status line's badge.
    fn mode(&self, theme: &Theme) -> (&'static str, ratatui::style::Color) {
        match self.focus() {
            Focus::Prompt => ("COMMAND", theme.mode_command),
            Focus::Picker if self.picker.as_ref().is_some_and(|p| p.is_insert()) => {
                ("INSERT", theme.ok)
            }
            Focus::Board if self.board.visual.is_some() => ("VISUAL", theme.mode_visual),
            _ => ("NORMAL", theme.accent),
        }
    }

    /// Returns the column where the `:` or `/` line starts.
    fn draw_status(&self, frame: &mut Frame, area: Rect, theme: &Theme) -> u16 {
        let (mode, color) = self.mode(theme);
        let mut left = vec![
            Span::styled(
                format!(" {mode} "),
                Style::new().bg(color).fg(theme.badge_fg).add_modifier(Modifier::BOLD),
            ),
            Span::raw(" "),
        ];
        let lead_x = area.x + Line::from(left.clone()).width() as u16;
        if let Some(prompt) = &self.prompt {
            let lead = match (prompt.kind, &self.field_prompt) {
                (PromptKind::Command, _) => ":".to_string(),
                (PromptKind::Field, Some((_, field))) => format!("{}: ", field.name),
                _ => "/".to_string(),
            };
            let text = prompt.input.text();
            let typed: String = text.chars().take(prompt.input.cursor()).collect();
            let x = lead_x + (width(&lead) + width(&typed)) as u16;
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
        let bar = Style::new().bg(theme.selection);
        frame.render_widget(Paragraph::new(Line::from(left)).style(bar), area);
        frame.render_widget(Paragraph::new(right).right_aligned(), area);
        lead_x
    }

    /// The `:` line's proposals, just above it, from its first column.
    fn draw_proposals(&self, frame: &mut Frame, area: Rect, lead_x: u16, theme: &Theme) {
        let Some(prompt) = self.prompt.as_ref().filter(|p| !p.hidden) else { return };
        let (_, list) = prompt.proposals();
        if list.is_empty() {
            return;
        }
        let name_width = list.iter().map(|p| width(p.text.trim_end())).max().unwrap_or(0);
        let lines: Vec<Line> = list
            .iter()
            .enumerate()
            .map(|(i, p)| {
                let name = p.text.trim_end();
                let pad = " ".repeat(name_width - width(name));
                let line = Line::from(vec![
                    Span::styled(format!(" {name}{pad}"), Style::new().fg(theme.accent)),
                    Span::styled(format!("  {} ", p.about), Style::new().fg(theme.muted)),
                ]);
                if prompt.chosen == Some(i) {
                    line.style(Style::new().bg(theme.selection))
                } else {
                    line
                }
            })
            .collect();
        let width = (lines.iter().map(Line::width).max().unwrap_or(0) as u16 + 2).min(area.width);
        let height = (lines.len() as u16 + 2).min(area.height);
        let rows = height.saturating_sub(2) as usize;
        let scroll = prompt.chosen.map_or(0, |i| (i + 1).saturating_sub(rows)) as u16;
        let x = lead_x.saturating_sub(1).min(area.right().saturating_sub(width));
        let rect = Rect { x, y: area.bottom() - height, width, height };
        let block = glyph::panel("", false, theme);
        frame.render_widget(Clear, rect);
        frame.render_widget(Paragraph::new(lines).block(block).scroll((scroll, 0)), rect);
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

/// A rich text field's text, empty when it has none yet.
fn rich_value(issue: &kolum_domain::Issue, id: &kolum_domain::FieldId) -> kolum_domain::RichText {
    match issue.value(id) {
        Some(kolum_domain::FieldValue::RichText(text)) => text,
        _ => kolum_domain::RichText::default(),
    }
}

const REFRESHING: &str = "Refreshing…";

const LOSSY: &str = "Jira holds things here that Markdown cannot keep (images, colours, expands): saving rewrites the field without them. ";

fn reading(key: &IssueKey) -> String {
    format!("Reading {key}…")
}

fn opening(key: &IssueKey) -> String {
    format!("Opening {key}…")
}

/// Two drafts of one issue as one: the title, and the description, each
/// from the later draft when it changed it, else from the one kept; with
/// the text it was changed from, so a save sends only what moved.
fn merge_drafts(kept: &(String, String), later: &(String, String)) -> (String, String) {
    let (kept_from, kept_text) = (split_title(&kept.0), split_title(&kept.1));
    let (later_from, later_text) = (split_title(&later.0), split_title(&later.1));
    let (from_title, title) = match later_text.0 != later_from.0 {
        true => (later_from.0, later_text.0),
        false => (kept_from.0, kept_text.0),
    };
    let (from_body, body) = match later_text.1 != later_from.1 {
        true => (later_from.1, later_text.1),
        false => (kept_from.1, kept_text.1),
    };
    let join =
        |title: Option<&str>, body: &str| format!("# {}\n\n{body}", title.unwrap_or_default());
    (join(from_title, from_body), join(title, body))
}

/// The title and the description as `$EDITOR` has them: `# Title`, then
/// Markdown.
fn titled(issue: &kolum_domain::Issue) -> String {
    format!("# {}\n\n{}", issue.title, issue.description_or_empty().to_markdown())
}

/// `# Title` then Markdown, against the same it was edited from: only what
/// moved is sent. The description is compared as Markdown, so a rich text
/// that does not survive the round trip exactly is not rewritten for nothing.
fn description_update(from: &str, text: &str) -> kolum_domain::IssueUpdate {
    let (was_title, was) = split_title(from);
    let (title, body) = split_title(text);
    kolum_domain::IssueUpdate {
        title: title.filter(|t| !t.is_empty() && Some(*t) != was_title).map(String::from),
        description: (body != was).then(|| kolum_domain::RichText::from_markdown(body)),
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

/// gh-dash's logo, on the right of the tab row: the name on a pill of
/// the selection, and the version.
fn logo(theme: &Theme) -> Line<'static> {
    Line::from(vec![
        Span::styled(
            " KOLUM ",
            Style::new().fg(theme.accent).bg(theme.selection).add_modifier(Modifier::BOLD),
        ),
        Span::styled(format!(" {} ", env!("CARGO_PKG_VERSION")), Style::new().fg(theme.muted)),
    ])
}

fn popup(title: &str, theme: &Theme) -> Block<'static> {
    glyph::panel(title, true, theme)
}

fn centered(area: Rect, width_pct: u16, height_pct: u16) -> Rect {
    let [row] =
        Layout::vertical([Constraint::Percentage(height_pct)]).flex(Flex::Center).areas(area);
    let [cell] =
        Layout::horizontal([Constraint::Percentage(width_pct)]).flex(Flex::Center).areas(row);
    cell
}
