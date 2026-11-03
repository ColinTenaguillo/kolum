//! The board: columns of cards, navigated with hjkl.

use super::glyph;
use super::text::{truncate, width, wrap};
use crate::config::{CardField, Swimlanes};
use crate::theme::Theme;
use kolum_app::{BoardSnapshot, parse_query};
use kolum_domain::{Filter, Query};
use kolum_domain::{Issue, IssueKey, UserId};
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Paragraph};
use std::cell::Cell;

/// Border, key line, two lines of title, meta line, border.
const CARD_HEIGHT: u16 = 6;
const MIN_COLUMN_WIDTH: u16 = 28;

#[derive(Debug, Default)]
pub struct BoardView {
    pub snapshot: Option<BoardSnapshot>,
    /// Index into the board's columns (not the visible ones).
    pub column: usize,
    /// The highlighted card of each column.
    cursor: Vec<usize>,
    /// The live `/` filter: cards whose key, title or labels contain it.
    pub filter: String,
    pub hide_empty: bool,
    pub swimlanes: Swimlanes,
    /// Who `@me` in the filter is.
    pub me: Option<UserId>,
    /// Cards per column on screen at the last draw, for half-page moves.
    page: Cell<usize>,
    /// Visual mode: where the range started, in the current column.
    pub visual: Option<usize>,
}

/// The `/` filter, read as the picker reads a query: `@me`, `@name`,
/// `#bug`, `s:`, `l:`, `e:`, `sp:` constrain, and every other word must
/// appear in the key, the title or a label.
struct BoardFilter {
    fields: Filter,
    words: Vec<String>,
}

/// What a prefix looks like before its value is typed: ignored, so the
/// board does not empty itself mid-word.
const BARE_PREFIXES: &[&str] = &["@", "#", "s:", "l:", "e:", "sp:", "!"];

impl BoardFilter {
    fn parse(input: &str) -> Self {
        let fields = match parse_query(input) {
            Query::Filter(f) => f,
            // A raw provider query means nothing on the board: its words
            // are text.
            Query::Native(text) => Filter { text: Some(text), ..Filter::default() },
        };
        let words = fields
            .text
            .iter()
            .flat_map(|t| t.split_whitespace())
            .filter(|w| !BARE_PREFIXES.contains(w))
            .map(str::to_lowercase)
            .collect();
        BoardFilter { fields: Filter { text: None, ..fields }, words }
    }

    fn matches(&self, issue: &Issue, me: &UserId) -> bool {
        let found = |w: &String| {
            issue.key.as_str().to_lowercase().contains(w)
                || issue.title.to_lowercase().contains(w)
                || issue.labels.iter().any(|l| l.to_lowercase().contains(w))
        };
        self.fields.matches_fields(issue, me) && self.words.iter().all(found)
    }
}
impl BoardView {
    /// A fresh snapshot keeps the highlight on the same issue when it is
    /// still there: a refresh must not throw the user back to the top.
    pub fn set_snapshot(&mut self, snapshot: BoardSnapshot) {
        let keep = self.selected().map(|i| i.key.clone());
        self.cursor = vec![0; snapshot.board.columns.len()];
        self.snapshot = Some(snapshot);
        match keep {
            Some(key) => self.select(&key),
            None => self.column = self.visible().first().copied().unwrap_or(0),
        }
        self.clamp();
    }

    /// The cards of every column, after the filter.
    pub fn cards(&self) -> Vec<Vec<&Issue>> {
        let Some(snapshot) = &self.snapshot else { return vec![] };
        let mode = self.swimlanes;
        let filter = BoardFilter::parse(&self.filter);
        let me = self.me.clone().unwrap_or_else(|| UserId::new(""));
        snapshot
            .columns()
            .into_iter()
            .map(|col| {
                let mut col: Vec<&Issue> =
                    col.into_iter().filter(|i| filter.matches(i, &me)).collect();
                // Stable: within a lane, the provider's order stands.
                col.sort_by_cached_key(|i| lane(mode, i).map(|(key, _)| key));
                col
            })
            .collect()
    }

    /// The columns on screen. Hiding empty ones never hides them all.
    pub fn visible(&self) -> Vec<usize> {
        let cards = self.cards();
        let all: Vec<usize> = (0..cards.len()).collect();
        if !self.hide_empty {
            return all;
        }
        let some: Vec<usize> = all.iter().copied().filter(|&c| !cards[c].is_empty()).collect();
        if some.is_empty() { all } else { some }
    }

    pub fn selected(&self) -> Option<&Issue> {
        let cards = self.cards();
        let col = cards.get(self.column)?;
        col.get(*self.cursor.get(self.column)?).copied()
    }

    pub fn select(&mut self, key: &IssueKey) {
        let found = self
            .cards()
            .iter()
            .enumerate()
            .find_map(|(c, col)| col.iter().position(|i| &i.key == key).map(|row| (c, row)));
        if let Some((c, row)) = found {
            self.column = c;
            self.cursor[c] = row;
        }
    }

    /// After anything that changes what is shown: the column must be a
    /// visible one and the cursor inside it. When a filter empties the
    /// current column, the highlight moves to the nearest column that
    /// still has a card, as vim's `/` jumps to a match.
    pub fn clamp(&mut self) {
        let visible = self.visible();
        let lens: Vec<usize> = self.cards().iter().map(Vec::len).collect();
        let with_cards: Vec<usize> = visible.iter().copied().filter(|&c| lens[c] > 0).collect();
        let candidates = if with_cards.is_empty() { &visible } else { &with_cards };
        if !candidates.contains(&self.column) {
            let nearest = candidates.iter().min_by_key(|&&c| c.abs_diff(self.column));
            self.column = nearest.copied().unwrap_or(0);
        }
        for (c, len) in lens.into_iter().enumerate() {
            if let Some(cur) = self.cursor.get_mut(c) {
                *cur = (*cur).min(len.saturating_sub(1));
            }
        }
    }

    pub fn move_column(&mut self, delta: i64) {
        let visible = self.visible();
        if let Some(pos) = visible.iter().position(|&c| c == self.column) {
            let to = (pos as i64 + delta).clamp(0, visible.len() as i64 - 1) as usize;
            self.column = visible[to];
        }
    }

    pub fn column_edge(&mut self, last: bool) {
        let visible = self.visible();
        let edge = if last { visible.last() } else { visible.first() };
        self.column = edge.copied().unwrap_or(0);
    }

    pub fn move_card(&mut self, delta: i64) {
        let len = self.cards().get(self.column).map_or(0, Vec::len) as i64;
        if let Some(cur) = self.cursor.get_mut(self.column)
            && len > 0
        {
            *cur = (*cur as i64 + delta).clamp(0, len - 1) as usize;
        }
    }

    /// `index` from the top; `None` is the last card.
    pub fn card_at(&mut self, index: Option<usize>) {
        let len = self.cards().get(self.column).map_or(0, Vec::len);
        if let Some(cur) = self.cursor.get_mut(self.column) {
            *cur = index.unwrap_or(usize::MAX).min(len.saturating_sub(1));
        }
    }

    /// The visual range, first and last row, in the current column.
    pub fn range(&self) -> Option<(usize, usize)> {
        let anchor = self.visual?;
        let cursor = self.cursor.get(self.column).copied().unwrap_or(0);
        Some((anchor.min(cursor), anchor.max(cursor)))
    }

    /// What an action acts on: the visual range, or the highlighted card.
    pub fn targets(&self) -> Vec<IssueKey> {
        let cards = self.cards();
        match (self.range(), cards.get(self.column)) {
            (Some((a, b)), Some(col)) => {
                col.iter().skip(a).take(b + 1 - a).map(|i| i.key.clone()).collect()
            }
            _ => self.selected().map(|i| i.key.clone()).into_iter().collect(),
        }
    }

    pub fn cursor_row(&self) -> usize {
        self.cursor.get(self.column).copied().unwrap_or(0)
    }

    /// A column's cards as drawn: a heading where a new lane starts.
    fn items(&self, cards: &[&Issue]) -> Vec<Item> {
        let mut items = vec![];
        let mut current: Option<String> = None;
        for (row, issue) in cards.iter().enumerate() {
            if let Some((_, label)) = lane(self.swimlanes, issue)
                && current.as_ref() != Some(&label)
            {
                items.push(Item::Lane(label.clone()));
                current = Some(label);
            }
            items.push(Item::Card(row));
        }
        items
    }

    pub fn half_page(&self) -> i64 {
        (self.page.get() / 2).max(1) as i64
    }

    /// `n` and `N`: the next or previous card in reading order (down each
    /// column, then on to the next), wrapping around.
    pub fn step_match(&mut self, delta: i64) {
        let cards = self.cards();
        let order: Vec<(usize, usize)> = self
            .visible()
            .into_iter()
            .flat_map(|c| (0..cards[c].len()).map(move |r| (c, r)))
            .collect();
        if order.is_empty() {
            return;
        }
        let here = (self.column, self.cursor.get(self.column).copied().unwrap_or(0));
        let pos = order.iter().position(|&p| p == here).unwrap_or(0) as i64;
        let (c, r) = order[(pos + delta).rem_euclid(order.len() as i64) as usize];
        self.column = c;
        self.cursor[c] = r;
    }

    pub fn draw(&self, frame: &mut Frame, area: Rect, ctx: &DrawCtx) {
        let Some(snapshot) = &self.snapshot else {
            let loading =
                Paragraph::new("Loading the board…").style(Style::new().fg(ctx.theme.muted));
            frame.render_widget(
                loading.centered(),
                Rect { y: area.y + area.height / 2, height: 1, ..area },
            );
            return;
        };
        let cards = self.cards();
        let visible = self.visible();
        let fit = (area.width / MIN_COLUMN_WIDTH).max(1) as usize;
        let shown = visible.len().min(fit);
        let pos = visible.iter().position(|&c| c == self.column).unwrap_or(0);
        // Scroll sideways only as far as it takes to show the selection.
        let first = (pos + 1).saturating_sub(shown);
        let width = area.width / shown.max(1) as u16;
        for (slot, &c) in visible.iter().skip(first).take(shown).enumerate() {
            let x = area.x + slot as u16 * width;
            let w = if slot + 1 == shown { area.right() - x } else { width };
            let rect = Rect { x, y: area.y, width: w, height: area.height };
            let column = &snapshot.board.columns[c];
            self.draw_column(frame, rect, &column.name, column.wip_limit, &cards[c], c, ctx);
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_column(
        &self,
        frame: &mut Frame,
        area: Rect,
        name: &str,
        wip: Option<u32>,
        cards: &[&Issue],
        c: usize,
        ctx: &DrawCtx,
    ) {
        let theme = ctx.theme;
        let focused = c == self.column;
        let count = cards.len();
        let over = wip.is_some_and(|w| count as u32 > w);
        let count_text = match wip {
            Some(w) => format!(" {count}/{w} "),
            None => format!(" {count} "),
        };
        let count_style = Style::new().fg(if over { theme.error } else { theme.muted });
        let title_style = Style::new()
            .fg(if focused { theme.accent } else { theme.fg })
            .add_modifier(Modifier::BOLD);
        let block = Block::bordered()
            .border_type(BorderType::Plain)
            .border_style(Style::new().fg(if focused { theme.border_focus } else { theme.border }))
            .title(Line::from(vec![
                Span::styled(format!(" {} ", name.to_uppercase()), title_style),
                Span::styled(count_text, count_style),
            ]));
        let inner = block.inner(area);
        frame.render_widget(block, area);
        // Whole cards only: a card cut in half reads as a different card.
        self.page.set((inner.height / CARD_HEIGHT).max(1) as usize);
        let cursor = self.cursor.get(c).copied().unwrap_or(0);
        let items = self.items(cards);
        let start = first_shown(&items, cursor, inner.height);
        let range = self.range().filter(|_| focused);
        let mut y = inner.y;
        let mut last_card = None;
        for item in &items[start..] {
            let height = item.height();
            if y + height > inner.bottom() {
                break;
            }
            let rect = Rect { y, height, ..inner };
            match item {
                Item::Lane(label) => {
                    let text = truncate(&format!("── {label} "), inner.width as usize);
                    let rule = "─".repeat((inner.width as usize).saturating_sub(width(&text)));
                    let line = Line::from(Span::styled(
                        format!("{text}{rule}"),
                        Style::new().fg(theme.muted),
                    ));
                    frame.render_widget(Paragraph::new(line), rect);
                }
                &Item::Card(row) => {
                    let marked = range.is_some_and(|(a, b)| (a..=b).contains(&row));
                    card(frame, rect, cards[row], focused && row == cursor, marked, ctx);
                    last_card = Some(row);
                }
            }
            y += height;
        }
        let below = count.saturating_sub(last_card.map_or(0, |r| r + 1));
        if below > 0 && inner.height > 0 {
            let more =
                Line::from(Span::styled(format!(" ↓ {below} more "), Style::new().fg(theme.muted)));
            let bottom = Rect { y: area.bottom() - 1, height: 1, ..inner };
            frame.render_widget(Paragraph::new(more).right_aligned(), bottom);
        }
        if count == 0 {
            let empty = Paragraph::new("No cards").style(Style::new().fg(theme.muted)).centered();
            frame.render_widget(empty, Rect { height: 1, y: inner.y + 1, ..inner });
        }
    }
}

/// One row of a column: a card, or the heading of the swimlane after it.
enum Item {
    Lane(String),
    Card(usize),
}

impl Item {
    fn height(&self) -> u16 {
        match self {
            Item::Lane(_) => 1,
            Item::Card(_) => CARD_HEIGHT,
        }
    }
}

/// The first row to draw so that `cursor`'s card is on screen: as far
/// down as needed and no further, the lane heading above it included.
fn first_shown(items: &[Item], cursor: usize, height: u16) -> usize {
    let Some(at) = items.iter().position(|i| matches!(i, Item::Card(r) if *r == cursor)) else {
        return 0;
    };
    let mut used = 0;
    let mut start = at + 1;
    while start > 0 && used + items[start - 1].height() <= height {
        start -= 1;
        used += items[start].height();
    }
    start.min(at)
}

/// The lane an issue belongs to, as a sort key and a heading. Issues with
/// no value go last, in a lane of their own.
fn lane(mode: Swimlanes, issue: &Issue) -> Option<((bool, String), String)> {
    let (value, none): (Option<(String, String)>, &str) = match mode {
        Swimlanes::None => return None,
        Swimlanes::Epic => (
            issue.parent.as_ref().map(|p| (p.title.to_lowercase(), format!("◆ {}", p.title))),
            "No epic",
        ),
        Swimlanes::Assignee => (
            issue
                .assignee
                .as_ref()
                .map(|u| (u.display_name.to_lowercase(), u.display_name.clone())),
            "Unassigned",
        ),
        Swimlanes::Priority => (
            issue.priority.map(|p| (format!("{:?}", p as u8), p.name().to_string())),
            "No priority",
        ),
    };
    Some(match value {
        Some((key, label)) => ((false, key), label),
        None => ((true, String::new()), none.to_string()),
    })
}

/// What drawing needs from the app beyond the view itself.
pub struct DrawCtx<'a> {
    pub theme: &'a Theme,
    pub fields: &'a [CardField],
    pub me: Option<&'a UserId>,
}

fn card(frame: &mut Frame, area: Rect, issue: &Issue, selected: bool, marked: bool, ctx: &DrawCtx) {
    let theme = ctx.theme;
    let show = |f: CardField| ctx.fields.contains(&f);
    let block = Block::bordered()
        .border_type(if selected { BorderType::Thick } else { BorderType::Rounded })
        .border_style(Style::new().fg(if selected || marked {
            theme.border_focus
        } else {
            theme.border
        }))
        .style(if selected || marked {
            Style::new().bg(theme.selection)
        } else {
            Style::default()
        });
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let cols = inner.width as usize;

    let mut head = vec![];
    if show(CardField::Type) {
        head.extend([glyph::kind_span(issue, theme), Span::raw(" ")]);
    }
    if show(CardField::Key) {
        head.push(Span::styled(issue.key.to_string(), Style::new().fg(theme.muted)));
    }
    let mut lines = vec![right_aligned(head, priority(issue, ctx), cols)];
    let title_style = Style::new().fg(theme.fg).add_modifier(Modifier::BOLD);
    let mut title = wrap(&issue.title, cols, 2);
    title.resize(2, String::new());
    lines.extend(title.into_iter().map(|t| Line::from(Span::styled(t, title_style))));
    lines.push(meta(issue, ctx, cols));
    frame.render_widget(Paragraph::new(lines), inner);
}

fn priority(issue: &Issue, ctx: &DrawCtx) -> Vec<Span<'static>> {
    match issue.priority {
        Some(p) if ctx.fields.contains(&CardField::Priority) => {
            vec![glyph::priority_span(p, ctx.theme)]
        }
        _ => vec![],
    }
}

/// Labels, epic and estimate on the left, the assignee on the right; the
/// left side gives way when the line is too short for both.
fn meta(issue: &Issue, ctx: &DrawCtx, cols: usize) -> Line<'static> {
    let theme = ctx.theme;
    let show = |f: CardField| ctx.fields.contains(&f);
    let mut right = vec![];
    if show(CardField::Estimate)
        && let Some(e) = &issue.estimate
    {
        right.push(Span::styled(format!("{} ", glyph::estimate(e)), Style::new().fg(theme.muted)));
    }
    if show(CardField::Assignee) {
        let mine = ctx.me.is_some_and(|me| issue.is_assigned_to(me));
        match &issue.assignee {
            Some(a) => right.push(Span::styled(
                a.initials(),
                Style::new()
                    .fg(if mine { theme.accent } else { theme.fg })
                    .add_modifier(Modifier::BOLD),
            )),
            None => right.push(Span::styled("--", Style::new().fg(theme.muted))),
        }
        if mine {
            right.push(Span::styled("★", Style::new().fg(theme.warn)));
        }
    }
    let room = cols.saturating_sub(right.iter().map(|s| width(&s.content)).sum::<usize>() + 1);
    let mut wanted: Vec<Span<'static>> = vec![];
    if show(CardField::Labels) {
        wanted.extend(issue.labels.iter().map(|l| glyph::label_badge(l, theme)));
    }
    let labels_width: usize = wanted.iter().map(|s| width(&s.content) + 1).sum();
    if show(CardField::Epic)
        && let Some(epic) = &issue.parent
    {
        // The epic takes what the labels leave, cut short rather than lost.
        let text = truncate(&format!("◆ {}", epic.title), room.saturating_sub(labels_width).max(6));
        wanted.push(Span::styled(text, Style::new().fg(theme.type_epic)));
    }
    let mut left: Vec<Span<'static>> = vec![];
    let mut used = 0;
    for span in wanted {
        let w = width(&span.content);
        if used + w <= room {
            used += w + 1;
            left.extend([span, Span::raw(" ")]);
        }
    }
    right_aligned(left, right, cols)
}

fn right_aligned(
    mut left: Vec<Span<'static>>,
    right: Vec<Span<'static>>,
    cols: usize,
) -> Line<'static> {
    let used: usize = left.iter().chain(&right).map(|s| width(&s.content)).sum();
    left.push(Span::raw(" ".repeat(cols.saturating_sub(used))));
    left.extend(right);
    Line::from(left)
}
