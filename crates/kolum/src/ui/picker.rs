//! The fuzzy picker, after Telescope: a prompt, ranked results with the
//! matched characters lit, and a preview. One component behind every
//! choice; a new picker is a [`PickerSource`].
//!
//! Matching runs on nucleo's threads. The screen asks for whatever the
//! latest snapshot holds, so a keystroke never waits on a sort.

use super::input::Input;
use super::text::truncate;
use crate::config::PickerLayout;
use crate::keymap::{Action, Key};
use crate::theme::Theme;
use nucleo::pattern::{CaseMatching, Normalization};
use nucleo::{Config, Matcher, Nucleo, Utf32String};
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, BorderType, Clear, Paragraph, Wrap};
use std::collections::BTreeSet;
use std::sync::Arc;

/// What a picker hands back to the app.
#[derive(Debug, Clone, PartialEq)]
pub enum Pick {
    OpenIssue(kolum_domain::IssueKey),
    Copy(String),
    OpenUrl(String),
    Theme(String),
    Board(kolum_domain::BoardId),
    /// Show these people's cards: `@` tokens for the board filter. An
    /// empty token is everyone.
    People(Vec<String>),
    /// An issue action the app carries out, on these issues.
    OnIssues(Action, Vec<kolum_domain::IssueKey>),
    Transition {
        keys: Vec<kolum_domain::IssueKey>,
        transition: kolum_domain::Transition,
    },
    Assign {
        keys: Vec<kolum_domain::IssueKey>,
        user: Option<kolum_domain::User>,
    },
    /// Run an action as if its key were pressed (the command palette).
    Run(Action),
    /// The action means something here, but not yet.
    Unavailable(Action),
}

pub trait PickerSource: 'static {
    type Item: Clone + Send + Sync + 'static;

    fn title(&self) -> String;
    /// The items a query chooses from. Most sources ignore the query; the
    /// issue source narrows by its prefixes (`@me`, `#bug`).
    fn candidates(&self, query: &str) -> Vec<Self::Item>;
    /// The part of the query that decides [`Self::candidates`]. When it
    /// changes, the items are fed to the matcher again.
    fn scope(&self, _query: &str) -> String {
        String::new()
    }
    /// The part of the query that is matched fuzzily.
    fn pattern(&self, query: &str) -> String {
        query.to_string()
    }
    /// The text matched and shown as the row.
    fn haystack(&self, item: &Self::Item) -> String;
    fn preview(&self, _item: &Self::Item, _theme: &Theme, _cols: u16) -> Option<Text<'static>> {
        None
    }
    /// What an action does to the selection (the marked items, or the
    /// highlighted one). `None` when the action has no meaning here.
    fn act(&self, items: &[Self::Item], action: Action) -> Option<Pick>;
    /// What highlighting an item does by itself: a theme previews.
    fn hover(&self, _item: &Self::Item) -> Option<Pick> {
        None
    }
    /// Switches between local matching and the provider's search, and
    /// says which is now in force; `None` when the source has no remote.
    fn toggle_remote(&mut self) -> Option<bool> {
        None
    }
    /// The provider's answer to the latest remote query.
    fn receive(&mut self, _issues: Vec<kolum_domain::Issue>) {}
}

/// A picker of any source, as the app holds it.
pub trait AnyPicker {
    fn is_insert(&self) -> bool;
    fn set_insert(&mut self, insert: bool);
    fn query(&self) -> &str;
    fn set_query(&mut self, query: &str);
    /// A key the keymap left to the prompt.
    fn type_key(&mut self, key: Key);
    /// Takes in whatever the matcher finished since the last call.
    fn tick(&mut self);
    /// Moves the highlight; `count` rows, clamped.
    fn move_by(&mut self, delta: i64);
    fn move_to(&mut self, index: Option<u32>);
    fn toggle_mark(&mut self);
    fn scroll_preview(&mut self, delta: i32);
    fn act(&mut self, action: Action) -> Option<Pick>;
    fn hovered(&self) -> Option<Pick>;
    fn toggle_remote(&mut self) -> Option<bool>;
    fn is_remote(&self) -> bool;
    fn receive(&mut self, issues: Vec<kolum_domain::Issue>);
    /// A remote search is under way, or about to go out.
    fn set_loading(&mut self, loading: bool);
    fn is_loading(&self) -> bool;
    /// The spinner's next frame, while loading.
    fn spin(&mut self);
    /// The status words in the bottom border: `[local]`.
    fn draw(&mut self, frame: &mut Frame, area: Rect, theme: &Theme, layout: PickerLayout);
}

pub struct Picker<S: PickerSource> {
    source: S,
    nucleo: Nucleo<S::Item>,
    matcher: Matcher,
    input: Input,
    insert: bool,
    selected: u32,
    /// By haystack: it names the item and survives a re-rank.
    marks: BTreeSet<String>,
    scope: String,
    pattern: String,
    preview_scroll: u16,
    remote: bool,
    /// The spinner's frame while a remote search is under way.
    loading: Option<usize>,
}

const SPINNER: [char; 10] = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];

pub type Waker = Arc<dyn Fn() + Send + Sync>;

impl<S: PickerSource> Picker<S> {
    pub fn new(source: S, waker: Waker) -> Self {
        let nucleo = Nucleo::new(Config::DEFAULT, waker, None, 1);
        let mut picker = Picker {
            source,
            nucleo,
            matcher: Matcher::new(Config::DEFAULT),
            input: Input::default(),
            insert: true,
            selected: 0,
            marks: BTreeSet::new(),
            scope: String::new(),
            pattern: String::new(),
            preview_scroll: 0,
            remote: false,
            loading: None,
        };
        picker.inject();
        picker
    }

    fn inject(&mut self) {
        let injector = self.nucleo.injector();
        for item in self.source.candidates(self.input.text()) {
            let text = self.source.haystack(&item);
            injector.push(item, |_, cols| cols[0] = Utf32String::from(text.as_str()));
        }
    }

    /// Re-reads the query: new candidates if its scope moved, a new
    /// pattern otherwise. Appending to the pattern lets nucleo narrow the
    /// last results instead of starting over.
    fn refresh(&mut self) {
        let query = self.input.text().to_string();
        let scope = self.source.scope(&query);
        if scope != self.scope {
            self.scope = scope;
            self.nucleo.restart(true);
            self.inject();
        }
        let pattern = self.source.pattern(&query);
        let append = pattern.starts_with(&self.pattern);
        self.nucleo.pattern.reparse(0, &pattern, CaseMatching::Smart, Normalization::Smart, append);
        self.pattern = pattern;
        self.selected = 0;
        self.preview_scroll = 0;
    }

    /// Feeds the matcher again even though the query did not move: the
    /// candidates themselves changed.
    fn reinject(&mut self) {
        self.scope = "\0".into();
        self.refresh();
    }

    fn current(&self) -> Option<&S::Item> {
        self.nucleo.snapshot().get_matched_item(self.selected).map(|i| i.data)
    }

    /// The marked items in rank order, or the highlighted one.
    /// The marked items, in the order they were given, whatever the query
    /// now shows (a mark made before typing something else still counts);
    /// or the highlighted one when nothing is marked.
    fn targets(&self) -> Vec<S::Item> {
        let snapshot = self.nucleo.snapshot();
        let marked: Vec<S::Item> = (0..snapshot.item_count())
            .filter_map(|i| snapshot.get_item(i))
            .filter(|i| self.marks.contains(&self.source.haystack(i.data)))
            .map(|i| i.data.clone())
            .collect();
        if marked.is_empty() { self.current().cloned().into_iter().collect() } else { marked }
    }
    fn source_label(&self) -> &'static str {
        if self.remote { "remote" } else { "local" }
    }

    /// What is typed, the cursor in it, and the spinner at the far end.
    fn draw_prompt(&self, frame: &mut Frame, line: Rect, theme: &Theme) {
        let prompt = Line::from(vec![
            Span::styled("> ", Style::new().fg(theme.accent)),
            Span::raw(self.input.text().to_string()),
        ]);
        frame.render_widget(Paragraph::new(prompt), line);
        if let Some(spinner) = self.spinner() {
            frame.render_widget(
                Line::styled(spinner, Style::new().fg(theme.accent)).right_aligned(),
                line,
            );
        }
        if self.insert {
            let x = line.x + 2 + self.input.text().chars().take(self.input.cursor()).count() as u16;
            frame.set_cursor_position((x.min(line.right().saturating_sub(1)), line.y));
        }
    }

    fn spinner(&self) -> Option<String> {
        self.loading.map(|frame| format!("{} searching…", SPINNER[frame % SPINNER.len()]))
    }

    fn row(&mut self, index: u32, theme: &Theme, cols: usize) -> Line<'static> {
        let snapshot = self.nucleo.snapshot();
        let Some(item) = snapshot.get_matched_item(index) else { return Line::default() };
        let mut hits = vec![];
        snapshot.pattern().column_pattern(0).indices(
            item.matcher_columns[0].slice(..),
            &mut self.matcher,
            &mut hits,
        );
        let hits: BTreeSet<u32> = hits.into_iter().collect();
        let text = truncate(&self.source.haystack(item.data), cols.saturating_sub(2));
        let selected = index == self.selected;
        let marked = self.marks.contains(&self.source.haystack(item.data));
        let base = if selected {
            Style::new().bg(theme.selection).fg(theme.fg).add_modifier(Modifier::BOLD)
        } else {
            Style::new().fg(theme.fg)
        };
        let lead = match (selected, marked) {
            (_, true) => Span::styled("+ ", base.fg(theme.accent)),
            (true, false) => Span::styled("▶ ", base.fg(theme.accent)),
            _ => Span::styled("  ", base),
        };
        let mut spans = vec![lead];
        for (i, c) in text.chars().enumerate() {
            let style = if hits.contains(&(i as u32)) {
                base.fg(theme.match_highlight).add_modifier(Modifier::BOLD)
            } else {
                base
            };
            spans.push(Span::styled(c.to_string(), style));
        }
        let pad = cols.saturating_sub(super::text::width(&text) + 2);
        spans.push(Span::styled(" ".repeat(pad), base));
        Line::from(spans)
    }
}

impl<S: PickerSource> AnyPicker for Picker<S> {
    fn is_insert(&self) -> bool {
        self.insert
    }

    fn set_insert(&mut self, insert: bool) {
        self.insert = insert;
    }

    fn query(&self) -> &str {
        self.input.text()
    }

    fn set_query(&mut self, query: &str) {
        self.input.set(query);
        self.refresh();
    }

    fn type_key(&mut self, key: Key) {
        if self.input.handle(key) {
            self.refresh();
        }
    }

    fn tick(&mut self) {
        self.nucleo.tick(10);
        let count = self.nucleo.snapshot().matched_item_count();
        self.selected = self.selected.min(count.saturating_sub(1));
    }

    fn move_by(&mut self, delta: i64) {
        let count = i64::from(self.nucleo.snapshot().matched_item_count());
        if count > 0 {
            self.selected = (i64::from(self.selected) + delta).clamp(0, count - 1) as u32;
            self.preview_scroll = 0;
        }
    }

    fn move_to(&mut self, index: Option<u32>) {
        let last = self.nucleo.snapshot().matched_item_count().saturating_sub(1);
        self.selected = index.unwrap_or(last).min(last);
        self.preview_scroll = 0;
    }

    fn toggle_mark(&mut self) {
        if let Some(item) = self.current() {
            let key = self.source.haystack(item);
            if !self.marks.remove(&key) {
                self.marks.insert(key);
            }
            self.move_by(1);
        }
    }

    fn scroll_preview(&mut self, delta: i32) {
        self.preview_scroll = (i32::from(self.preview_scroll) + delta).max(0) as u16;
    }

    fn act(&mut self, action: Action) -> Option<Pick> {
        let targets = self.targets();
        if targets.is_empty() {
            return None;
        }
        self.source.act(&targets, action)
    }

    fn hovered(&self) -> Option<Pick> {
        self.current().and_then(|i| self.source.hover(i))
    }

    fn toggle_remote(&mut self) -> Option<bool> {
        let remote = self.source.toggle_remote()?;
        self.remote = remote;
        self.loading = None;
        self.reinject();
        Some(remote)
    }

    fn is_remote(&self) -> bool {
        self.remote
    }

    fn receive(&mut self, issues: Vec<kolum_domain::Issue>) {
        self.source.receive(issues);
        self.loading = None;
        self.reinject();
    }

    /// Already loading keeps its frame: typing does not restart the turn.
    fn set_loading(&mut self, loading: bool) {
        self.loading = if loading { self.loading.or(Some(0)) } else { None };
    }

    fn is_loading(&self) -> bool {
        self.loading.is_some()
    }

    fn spin(&mut self) {
        self.loading = self.loading.map(|f| f + 1);
    }

    fn draw(&mut self, frame: &mut Frame, area: Rect, theme: &Theme, layout: PickerLayout) {
        frame.render_widget(Clear, area);
        // Too narrow for two panes side by side: stack them, or drop the
        // preview when there is no height for it either.
        let layout = match layout {
            PickerLayout::Horizontal if area.width < 80 => PickerLayout::Vertical,
            PickerLayout::Vertical if area.height < 20 => PickerLayout::NoPreview,
            l => l,
        };
        let (list_area, preview_area) = match layout {
            PickerLayout::Horizontal => {
                let [l, r] = Layout::horizontal([Constraint::Percentage(50); 2]).areas(area);
                (l, Some(r))
            }
            PickerLayout::Vertical => {
                let [t, b] = Layout::vertical([Constraint::Percentage(50); 2]).areas(area);
                (b, Some(t))
            }
            PickerLayout::NoPreview => (area, None),
        };
        let border = Style::new().fg(theme.border_focus);
        let snapshot = self.nucleo.snapshot();
        let counter = format!(" {}/{} ", snapshot.matched_item_count(), snapshot.item_count());
        let mode = if self.insert { "" } else { " NORMAL " };
        let list = Block::bordered()
            .border_type(BorderType::Rounded)
            .border_style(border)
            .title(Span::styled(
                format!(" {} ", self.source.title()),
                Style::new().fg(theme.accent),
            ))
            .title_bottom(Line::from(counter).left_aligned())
            .title_bottom(Line::from(format!("{mode}[{}] ", self.source_label())).right_aligned())
            .style(Style::new().bg(theme.bg).fg(theme.fg));
        let inner = list.inner(list_area);
        frame.render_widget(list, list_area);
        if inner.height == 0 {
            return;
        }
        self.draw_prompt(frame, Rect { height: 1, ..inner }, theme);
        let rows = inner.height.saturating_sub(2) as u32;
        let offset = self.selected.saturating_sub(rows.saturating_sub(1));
        let count = self.nucleo.snapshot().matched_item_count();
        let lines: Vec<Line> = (offset..count.min(offset + rows))
            .map(|i| self.row(i, theme, inner.width as usize))
            .collect();
        let results = Rect { y: inner.y + 2, height: inner.height.saturating_sub(2), ..inner };
        frame.render_widget(Paragraph::new(lines), results);

        if let Some(area) = preview_area {
            let block = Block::bordered()
                .border_type(BorderType::Rounded)
                .border_style(Style::new().fg(theme.border))
                .title(" Preview ")
                .style(Style::new().bg(theme.bg).fg(theme.fg));
            let inner = block.inner(area);
            let text = self.current().and_then(|i| self.source.preview(i, theme, inner.width));
            let preview = Paragraph::new(text.unwrap_or_default())
                .block(block)
                .wrap(Wrap { trim: false })
                .scroll((self.preview_scroll, 0));
            frame.render_widget(preview, area);
        }
    }
}
