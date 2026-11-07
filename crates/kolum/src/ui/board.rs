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
use ratatui::widgets::Paragraph;
use std::cell::Cell;

/// The tallest card: two lines of title, the epic, the labels, the key
/// line, and a blank one. A card with no labels has no labels line.
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
    /// The first visible column on screen at the last draw: the columns
    /// slide only when the highlight would leave the screen.
    first: Cell<usize>,
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
    /// still there: a refresh must not throw the user back to the top. Of
    /// the same board, each column keeps its own on the same card, or on
    /// the same row when the card left: a change shown at once, or a sync,
    /// leaves the other columns alone.
    pub fn set_snapshot(&mut self, snapshot: BoardSnapshot) {
        let keep = self.selected().map(|i| i.key.clone());
        let same = self.snapshot.as_ref().is_some_and(|s| s.board.id == snapshot.board.id);
        let held = if same { self.highlighted() } else { vec![] };
        if !same {
            self.cursor.clear();
        }
        self.cursor.resize(snapshot.board.columns.len(), 0);
        self.snapshot = Some(snapshot);
        self.highlight(&held);
        match keep {
            Some(key) => self.select(&key),
            None => self.column = self.visible().first().copied().unwrap_or(0),
        }
        self.clamp();
    }

    /// The card highlighted in each column.
    fn highlighted(&self) -> Vec<Option<IssueKey>> {
        let cards = self.cards();
        cards
            .iter()
            .zip(&self.cursor)
            .map(|(col, &at)| col.get(at).map(|i| i.key.clone()))
            .collect()
    }

    /// Each column's highlight on its card, where the card still is.
    fn highlight(&mut self, held: &[Option<IssueKey>]) {
        let found: Vec<Option<usize>> = (self.cards().iter().zip(held))
            .map(|(col, key)| key.as_ref().and_then(|k| col.iter().position(|i| &i.key == k)))
            .collect();
        for (cursor, at) in self.cursor.iter_mut().zip(found) {
            *cursor = at.unwrap_or(*cursor);
        }
    }

    /// Whether the issue is on the board, filtered out or not.
    pub fn has(&self, key: &IssueKey) -> bool {
        self.snapshot.as_ref().is_some_and(|s| s.issues.iter().any(|i| &i.key == key))
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

    /// The range belongs to its column: leaving the column ends it.
    pub fn move_column(&mut self, delta: i64) {
        self.visual = None;
        let visible = self.visible();
        if let Some(pos) = visible.iter().position(|&c| c == self.column) {
            let to = (pos as i64 + delta).clamp(0, visible.len() as i64 - 1) as usize;
            self.column = visible[to];
        }
    }

    /// The first or the last column; the range ends, as it does for
    /// [`BoardView::move_column`].
    pub fn column_edge(&mut self, last: bool) {
        self.visual = None;
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

    /// The targets, as [`BoardView::targets`]; the range ends with them.
    pub fn take_targets(&mut self) -> Vec<IssueKey> {
        let keys = self.targets();
        self.visual = None;
        keys
    }

    /// Visual mode on, from the highlighted card if there is one; or off.
    pub fn toggle_visual(&mut self) {
        self.visual = match self.visual {
            Some(_) => None,
            None => self.selected().map(|_| self.cursor_row()),
        };
    }

    /// The card `delta` places below (or above) the highlighted one in its
    /// column, as far as that goes without leaving its swimlane. `Err`
    /// says why there is none: the edge of the column, or of the lane.
    pub fn neighbour(&self, delta: i64) -> Result<IssueKey, &'static str> {
        let cards = self.cards();
        let column = cards.get(self.column).ok_or("No card here")?;
        let from = self.cursor_row();
        let here = column.get(from).ok_or("No card here")?;
        let lane_of = |i: &Issue| lane(self.swimlanes, i).map(|(key, _)| key);
        let mut to = from;
        for _ in 0..delta.unsigned_abs() {
            let next = if delta > 0 { to + 1 } else { to.wrapping_sub(1) };
            match column.get(next) {
                Some(card) if lane_of(card) == lane_of(here) => to = next,
                Some(_) if to == from => {
                    return Err(if delta > 0 {
                        "Already at the bottom of its lane"
                    } else {
                        "Already at the top of its lane"
                    });
                }
                Some(_) => break,
                None if to == from => {
                    return Err(if delta > 0 {
                        "Already at the bottom"
                    } else {
                        "Already at the top"
                    });
                }
                None => break,
            }
        }
        Ok(column[to].key.clone())
    }

    pub fn cursor_row(&self) -> usize {
        self.cursor.get(self.column).copied().unwrap_or(0)
    }

    /// A column's cards as drawn: a heading where a new lane starts.
    fn items(&self, cards: &[&Issue], ctx: &DrawCtx, cols: usize) -> Vec<Item> {
        let mut items = vec![];
        let mut current: Option<String> = None;
        for (row, issue) in cards.iter().enumerate() {
            if let Some((_, label)) = lane(self.swimlanes, issue)
                && current.as_ref() != Some(&label)
            {
                items.push(Item::Lane(label.clone()));
                current = Some(label);
            }
            let height = CARD_HEIGHT - u16::from(labels(issue, ctx, cols).spans.is_empty());
            items.push(Item::Card(row, height));
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
        // Scroll sideways like a list: only as far as it takes to show the
        // selection, and never past the last column.
        let lowest = (pos + 1).saturating_sub(shown);
        let first = self.first.get().max(lowest).min(pos).min(visible.len().saturating_sub(shown));
        self.first.set(first);
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
        // gh-dash's table: a header over a rule; no box, a gap between
        // columns.
        let area = Rect { width: area.width.saturating_sub(1), ..area };
        let head_style = Style::new()
            .fg(if focused { theme.accent } else { theme.muted })
            .add_modifier(Modifier::BOLD);
        let head = Line::from(vec![
            Span::styled(format!(" {} ", name.to_uppercase()), head_style),
            Span::styled(count_text, count_style),
        ]);
        frame.render_widget(Paragraph::new(head), Rect { height: 1, ..area });
        let rule = Style::new().fg(if focused { theme.border_focus } else { theme.border });
        let rule = Span::styled("─".repeat(area.width as usize), rule);
        frame.render_widget(Paragraph::new(rule), Rect { y: area.y + 1, height: 1, ..area });
        let inner = Rect { y: area.y + 2, height: area.height.saturating_sub(3), ..area };
        // Counted at the tallest, so a half page never skips past the screen.
        self.page.set((inner.height / CARD_HEIGHT).max(1) as usize);
        let cursor = self.cursor.get(c).copied().unwrap_or(0);
        let items = self.items(cards, ctx, (inner.width as usize).saturating_sub(1));
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
                &Item::Card(row, _) => {
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
    /// Its row, and its height.
    Card(usize, u16),
}

impl Item {
    fn height(&self) -> u16 {
        match self {
            Item::Lane(_) => 1,
            &Item::Card(_, height) => height,
        }
    }
}

/// The first row to draw so that `cursor`'s card is on screen: as far
/// down as needed and no further, the lane heading above it included.
fn first_shown(items: &[Item], cursor: usize, height: u16) -> usize {
    let Some(at) = items.iter().position(|i| matches!(i, Item::Card(r, _) if *r == cursor)) else {
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
            issue.priority.as_ref().map(|p| (format!("{:010} {}", p.rank, p.name), p.name.clone())),
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
    // The stripe says the priority; the cursor's is thicker, as its border was.
    let stripe = Span::styled(
        if selected { "┃" } else { "▍" },
        Style::new().fg(match &issue.priority {
            _ if selected => theme.border_focus,
            Some(p) if ctx.fields.contains(&CardField::Priority) => theme.priority(p),
            _ => theme.border,
        }),
    );
    let cols = (area.width as usize).saturating_sub(1);
    let title_style = Style::new().fg(theme.fg).add_modifier(Modifier::BOLD);
    let mut title = wrap(&issue.title, cols, 2);
    title.resize(2, String::new());
    let mut lines: Vec<Line> = title
        .into_iter()
        .map(|t| Line::from(vec![stripe.clone(), Span::styled(t, title_style)]))
        .collect();
    let labels = Some(labels(issue, ctx, cols)).filter(|l| !l.spans.is_empty());
    for mut line in
        [Some(epic(issue, ctx, cols)), labels, Some(meta(issue, ctx, cols))].into_iter().flatten()
    {
        line.spans.insert(0, stripe.clone());
        lines.push(line);
    }
    let style =
        if selected || marked { Style::new().bg(theme.selection) } else { Style::default() };
    frame.render_widget(
        Paragraph::new(lines).style(style),
        Rect { height: area.height - 1, ..area },
    );
}

/// The epic on a line of its own, under the title. Empty when there is
/// none, so every card's lines stay where the eye expects them.
fn epic(issue: &Issue, ctx: &DrawCtx, cols: usize) -> Line<'static> {
    match &issue.parent {
        Some(epic) if ctx.fields.contains(&CardField::Epic) => Line::from(Span::styled(
            truncate(&format!("◆ {}", epic.title), cols),
            Style::new().fg(ctx.theme.type_epic),
        )),
        _ => Line::default(),
    }
}

/// The labels on a line of their own, under the epic; those that do not
/// fit are left off rather than cut.
fn labels(issue: &Issue, ctx: &DrawCtx, cols: usize) -> Line<'static> {
    let mut spans = vec![];
    let mut used = 0;
    if ctx.fields.contains(&CardField::Labels) {
        for pill in issue.labels.iter().map(|l| glyph::label_badge(l, ctx.theme)) {
            let w: usize = pill.iter().map(|s| width(&s.content)).sum();
            if used + w <= cols {
                used += w + 1;
                spans.extend(pill);
                spans.push(Span::raw(" "));
            }
        }
    }
    Line::from(spans)
}

/// The type, key and priority on the left, the estimate and the assignee
/// on the right.
fn meta(issue: &Issue, ctx: &DrawCtx, cols: usize) -> Line<'static> {
    let theme = ctx.theme;
    let show = |f: CardField| ctx.fields.contains(&f);
    let mut left = vec![];
    if show(CardField::Type) {
        left.extend([glyph::kind_span(issue, theme), Span::raw(" ")]);
    }
    if show(CardField::Key) {
        left.push(Span::styled(format!("{} ", issue.key), Style::new().fg(theme.muted)));
    }
    if show(CardField::Priority)
        && let Some(p) = &issue.priority
    {
        left.push(glyph::priority_span(p, theme));
    }
    right_aligned(left, estimate_and_assignee(issue, ctx), cols)
}

/// The right end of a card's key line.
fn estimate_and_assignee(issue: &Issue, ctx: &DrawCtx) -> Vec<Span<'static>> {
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
    right
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

#[cfg(test)]
mod tests {
    use super::*;
    use kolum_domain::Priority;

    async fn board() -> BoardView {
        let fake = kolum_provider_fake::FakeProvider::new();
        let mut view = BoardView::default();
        view.set_snapshot(kolum_app::load_board(&fake, None).await.unwrap());
        view
    }

    /// The lanes of each column, top to bottom.
    fn lanes(view: &BoardView) -> Vec<Vec<String>> {
        let lanes_of = |col: &Vec<&Issue>| {
            let mut labels: Vec<String> =
                col.iter().filter_map(|i| lane(view.swimlanes, i).map(|(_, l)| l)).collect();
            labels.dedup();
            labels
        };
        view.cards().iter().map(lanes_of).collect()
    }

    /// Lanes by priority go by its rank, the most urgent first, whatever
    /// its name; the cards with none last.
    #[tokio::test]
    async fn priority_lanes_go_by_rank_not_by_name() {
        let mut view = board().await;
        let mut snapshot = view.snapshot.take().unwrap();
        let named = |rank, name: &str| Some(Priority { rank, name: name.into() });
        for (n, issue) in snapshot.issues.iter_mut().enumerate() {
            issue.priority = [named(1, "Alpha"), named(0, "Zulu"), None][n % 3].clone();
        }
        view.set_snapshot(snapshot);
        view.swimlanes = Swimlanes::Priority;
        let order = ["Zulu", "Alpha", "No priority"];
        for column in lanes(&view) {
            let at: Vec<usize> =
                column.iter().map(|l| order.iter().position(|o| o == l).unwrap()).collect();
            assert!(at.is_sorted(), "{column:?}");
        }
        assert!(lanes(&view).iter().any(|c| c.len() == 3), "all three somewhere");
    }
}
