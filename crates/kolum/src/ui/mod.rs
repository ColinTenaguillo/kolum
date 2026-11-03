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

/// What the event loop does on the app's behalf.
#[derive(Debug, Clone, PartialEq)]
pub enum Effect {
    Send(Command),
    Copy(String),
    Open(String),
}

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
    pub quit: bool,
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
            effects: vec![Effect::Send(Command::LoadBoard(None))],
            waker,
            quit: false,
        }
    }

    pub fn take_effects(&mut self) -> Vec<Effect> {
        std::mem::take(&mut self.effects)
    }

    pub fn deadline(&self) -> Option<Instant> {
        self.resolver.deadline()
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
            Focus::Board => (vec![Context::Board, Context::Global], false),
        }
    }

    pub fn on_key(&mut self, key: Key, now: Instant) {
        if !self.problems.is_empty() {
            self.problems.clear();
            return;
        }
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
            Event::Me(user) => self.me = Some(user),
            Event::BoardLoaded(snapshot) => {
                if let Some(d) = &mut self.detail
                    && let Some(fresh) = snapshot.issues.iter().find(|i| i.key == d.issue.key)
                {
                    d.issue = fresh.clone();
                }
                self.board.set_snapshot(snapshot);
            }
            Event::IssueLoaded(detail) => {
                if let Some(d) = &mut self.detail
                    && d.issue.key == detail.issue.key
                {
                    d.issue = detail.issue.clone();
                    d.loaded = Some(*detail);
                }
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
                self.effects.push(Effect::Send(Command::LoadBoard(id)));
                self.say("Refreshing…", Level::Info);
            }
            Action::PickerIssues => self.open_issue_picker(),
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
        match action {
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
            Action::ToggleEmptyColumns => {
                b.hide_empty = !b.hide_empty;
                b.clamp();
                let what = if b.hide_empty { "Hiding" } else { "Showing" };
                self.say(format!("{what} empty columns"), Level::Info);
            }
            Action::OpenDetail => {
                if let Some(key) = b.selected().map(|i| i.key.clone()) {
                    self.open_detail(key);
                }
            }
            Action::CopyKey | Action::CopyUrl | Action::OpenBrowser => {
                if let Some(issue) = b.selected().cloned() {
                    self.issue_effect(action, &issue);
                }
            }
            _ => self.unavailable(action),
        }
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
            _ => self.unavailable(action),
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

    fn open_detail(&mut self, key: IssueKey) {
        let issue =
            self.board.snapshot.as_ref().and_then(|s| s.issues.iter().find(|i| i.key == key));
        match issue {
            Some(issue) => self.detail = Some(DetailView::new(issue.clone())),
            None => self.say(format!("{key} is not on this board"), Level::Warn),
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
            sources::IssueSource { issues, me },
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
            Action::ToggleSource => {
                self.say("Remote search comes with the Jira provider", Level::Warn)
            }
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
            "jql" | "board" | "assign" => {
                self.say(format!(":{name} is not available yet"), Level::Warn)
            }
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
        match &self.detail {
            Some(d) => d.draw(frame, main, &theme, &self.ui.detail_fields, me),
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
            Focus::Prompt => ("COMMAND", theme.warn),
            Focus::Picker if self.picker.as_ref().is_some_and(|p| p.is_insert()) => {
                ("INSERT", theme.ok)
            }
            _ => ("NORMAL", theme.accent),
        };
        let mut left = vec![
            Span::styled(
                format!(" {mode} "),
                Style::new().bg(color).fg(theme.bg).add_modifier(Modifier::BOLD),
            ),
            Span::raw(" "),
        ];
        if let Some(prompt) = &self.prompt {
            let lead = if prompt.kind == PromptKind::Command { ":" } else { "/" };
            let x = area.x
                + 1
                + mode.len() as u16
                + 2
                + prompt.input.text()[..].chars().take(prompt.input.cursor()).count() as u16;
            left.push(Span::raw(format!("{lead}{}", prompt.input.text())));
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
