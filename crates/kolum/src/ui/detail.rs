//! An issue, laid out like the web app: the content on the left (about
//! two thirds), the fields on the right. On a narrow terminal the fields
//! go under the content.

use super::{glyph, rich, search};
use crate::keymap::Action;
use crate::theme::Theme;
use kolum_app::IssueDetail;
use kolum_domain::{
    Attachment, CustomField, FieldId, FieldLayout, FieldValue, Issue, IssueRef, RichText, UserId,
};
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use std::cell::Cell;

type Picture = ratatui_image::protocol::StatefulProtocol;

/// What the detail can draw besides text, when it was fetched and decoded.
pub struct Pictures<'a> {
    pub avatar: Option<&'a mut Picture>,
    pub shot: Option<&'a mut Picture>,
}

/// The height of an attachment's preview, in rows.
const SHOT_ROWS: u16 = 10;

/// `48 KB`, `1.8 MB`: a size the way a file browser says it.
pub fn size(bytes: u64) -> String {
    match bytes {
        b if b < 1024 => format!("{b} B"),
        b if b < 1024 * 1024 => format!("{} KB", b / 1024),
        b => format!("{:.1} MB", b as f64 / (1024.0 * 1024.0)),
    }
}

/// An avatar's height in rows; twice that in columns makes it square.
const AVATAR_ROWS: u16 = 4;

/// Below this, two columns leave neither enough room.
const SIDE_BY_SIDE_MIN_WIDTH: u16 = 100;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Comments,
    History,
    All,
}

impl Tab {
    /// Left to right, as they show; `gt` goes round them in this order.
    const ORDER: [Tab; 3] = [Tab::Comments, Tab::History, Tab::All];

    /// Its place in [`Tab::ORDER`].
    fn index(self) -> usize {
        Tab::ORDER.iter().position(|&t| t == self).unwrap_or_default()
    }

    pub fn step(self, delta: i64) -> Tab {
        let at = (self.index() as i64 + delta).rem_euclid(Tab::ORDER.len() as i64);
        Tab::ORDER[at as usize]
    }
}

pub struct DetailView {
    /// What is shown: `latest` with the writes in flight on top, set by
    /// [`DetailView::show`] alone.
    issue: Issue,
    /// The issue as the provider last sent it.
    pub latest: Issue,
    /// Comments and history arrive after the view opens.
    pub loaded: Option<std::sync::Arc<IssueDetail>>,
    pub tab: Tab,
    pub scroll: u16,
    /// Rows of content on screen at the last draw, for half-page moves.
    page: Cell<u16>,
    /// Content rows at the last draw, so `G` and scrolling stop at the end.
    length: Cell<u16>,
    search: Search,
}

/// The `/` search of the open issue.
#[derive(Default)]
struct Search {
    query: String,
    /// The match the view was last moved to.
    at: usize,
    /// How many the last draw found.
    count: usize,
    /// A move the next draw makes, once it knows where the matches fall
    /// in the wrapped text.
    jump: Option<Jump>,
}

/// Moves asked for between two draws, added up.
#[derive(Clone, Copy, Default)]
struct Jump {
    /// From the first match at the top of the view or below it.
    below: bool,
    /// Then so many matches on, round the ends.
    by: i64,
}

impl DetailView {
    pub fn new(issue: Issue) -> Self {
        DetailView {
            latest: issue.clone(),
            issue,
            loaded: None,
            tab: Tab::Comments,
            scroll: 0,
            page: Cell::new(10),
            length: Cell::default(),
            search: Search::default(),
        }
    }

    pub fn query(&self) -> &str {
        &self.search.query
    }

    /// Searches for `query` from the top of the view; an empty one clears.
    pub fn search_for(&mut self, query: String) {
        let jump = Some(Jump { below: true, by: 0 });
        self.search = Search { query, jump, ..Search::default() };
    }

    /// The match the view is on, from 1, and how many there are; none
    /// without a search.
    pub fn search_count(&self) -> Option<(usize, usize)> {
        let Search { query, at, count, .. } = &self.search;
        (!query.is_empty()).then(|| (if *count == 0 { 0 } else { at + 1 }, *count))
    }

    /// `n` matches on: steps add up until the next draw lands them.
    fn step_match(&mut self, n: i64) {
        self.search.jump.get_or_insert_default().by += n;
    }

    /// Lands the jump asked for on the matches: the row each starts on in
    /// the scrolled text, or none for those always on screen (the header,
    /// the fields beside the text). A step counts from the match the view
    /// is on, or from the top of the view when it was scrolled off it.
    fn land(&mut self, rows: &[Option<u16>]) {
        let (scroll, page) = (u32::from(self.scroll), u32::from(self.page.get()));
        let s = &mut self.search;
        s.count = rows.len();
        s.at = s.at.min(rows.len().saturating_sub(1));
        let Some(jump) = s.jump.take().filter(|_| !rows.is_empty()) else { return };
        let below = |r: &Option<u16>| r.is_some_and(|r| u32::from(r) >= scroll);
        let above = rows.iter().filter(|r| r.is_some_and(|r| u32::from(r) < scroll)).count();
        let on_screen = rows[s.at].is_none_or(|r| (scroll..scroll + page).contains(&r.into()));
        let from = match () {
            _ if jump.below => rows.iter().position(below).unwrap_or_default() as i64,
            _ if on_screen => s.at as i64,
            // Between the last match above the view and the first in it.
            _ if jump.by > 0 => above as i64 - 1,
            _ => above as i64,
        };
        s.at = (from + jump.by).rem_euclid(rows.len() as i64) as usize;
        if let Some(row) = rows[s.at] {
            self.scroll = 0;
            self.scroll_by(i64::from(row));
        }
    }

    pub fn issue(&self) -> &Issue {
        &self.issue
    }

    /// Shows the provider's copy with the writes in flight on top.
    pub fn show(&mut self, pending: &kolum_app::Pending) {
        self.issue = pending.on(self.latest.clone());
    }

    pub fn scroll_by(&mut self, delta: i64) {
        let max = self.length.get().saturating_sub(self.page.get()) as i64;
        self.scroll = (self.scroll as i64 + delta).clamp(0, max) as u16;
    }

    /// Its sub-tasks, then the issues found by their parent, once each.
    pub fn children(&self) -> Vec<&IssueRef> {
        let found = self.loaded.iter().flat_map(|d| &d.children);
        let mut children: Vec<&IssueRef> = self.issue.subtasks.iter().collect();
        for child in found {
            if !children.iter().any(|c| c.key == child.key) {
                children.push(child);
            }
        }
        children
    }

    pub fn half_page(&self) -> i64 {
        (self.page.get() / 2).max(1) as i64
    }

    /// Scrolling and the tabs; false for any other action.
    pub fn motion(&mut self, action: Action, n: i64) -> bool {
        match action {
            Action::NextTab => self.tab = self.tab.step(n),
            Action::PrevTab => self.tab = self.tab.step(-n),
            Action::NextMatch => self.step_match(n),
            Action::PrevMatch => self.step_match(-n),
            _ => match super::scroll_step(action, n, self.half_page()) {
                Some(delta) => self.scroll_by(delta),
                None => return false,
            },
        }
        true
    }

    pub fn draw(
        &mut self,
        frame: &mut Frame,
        area: Rect,
        theme: &Theme,
        shown: Shown,
        me: Option<&UserId>,
        pictures: Pictures,
    ) {
        let lit = Style::new().bg(theme.match_highlight).fg(theme.badge_fg);
        let query = self.search.query.clone();
        let found = |lines, width| found(lines, &query, lit, width);
        let issue = &self.issue;
        let [head, body] =
            Layout::vertical([Constraint::Length(HEADER_ROWS), Constraint::Min(0)]).areas(area);
        let (header, head_rows, _) = found(header(issue, theme, head.width), head.width);
        frame.render_widget(Paragraph::new(header), head);

        let side = body.width >= SIDE_BY_SIDE_MIN_WIDTH;
        let [left, right] = if side {
            Layout::horizontal([Constraint::Percentage(65), Constraint::Percentage(35)]).areas(body)
        } else {
            [body, Rect::default()]
        };
        // A column of gap before the fields.
        let text = Rect { width: left.width.saturating_sub(1), ..left };
        let sections = body_sections(issue, shown.sections, shown.layout);
        let mut content = self.content(theme, left.width.saturating_sub(2), &sections);
        let names = field_names(issue, shown.fields, shown.layout);
        let field_lines = field_lines(issue, &names, theme, me);
        let mut side_rows = vec![];
        if side {
            let right = self.draw_avatar(frame, right, theme, pictures.avatar);
            let right = self.draw_shot(frame, right, theme, pictures.shot);
            let block =
                Block::new().borders(Borders::LEFT).border_style(Style::new().fg(theme.border));
            let (field_lines, rows, _) = found(field_lines, right.width);
            side_rows = rows;
            let fields = Paragraph::new(field_lines).block(block).wrap(Wrap { trim: false });
            frame.render_widget(fields, right);
        } else {
            content.push(Line::default());
            content.extend(field_lines);
        }
        let (content, rows, length) = found(content, text.width);
        let paragraph = Paragraph::new(content).wrap(Wrap { trim: false });
        self.page.set(text.height);
        self.length.set(length.unwrap_or_else(|| paragraph.line_count(text.width) as u16));
        let fixed = |rows: Vec<u16>| rows.into_iter().map(|_| None);
        let rows = fixed(head_rows).chain(rows.into_iter().map(Some)).chain(fixed(side_rows));
        self.land(&rows.collect::<Vec<_>>());
        frame.render_widget(paragraph.scroll((self.scroll, 0)), text);
    }

    /// The assignee's picture and name above the fields, when there is a
    /// picture; the rest of the column is what is left.
    fn draw_avatar(
        &self,
        frame: &mut Frame,
        area: Rect,
        theme: &Theme,
        avatar: Option<&mut Picture>,
    ) -> Rect {
        let (Some(protocol), Some(who)) = (avatar, &self.issue.assignee) else { return area };
        if area.height < AVATAR_ROWS + 4 || area.width < 20 {
            return area;
        }
        let [top, rest] =
            Layout::vertical([Constraint::Length(AVATAR_ROWS + 1), Constraint::Min(0)]).areas(area);
        let picture = Rect { x: top.x + 2, y: top.y, width: AVATAR_ROWS * 2, height: AVATAR_ROWS };
        frame.render_stateful_widget(
            ratatui_image::StatefulImage::<ratatui_image::protocol::StatefulProtocol>::new(),
            picture,
            protocol,
        );
        let name = Rect {
            x: picture.right() + 2,
            y: top.y + AVATAR_ROWS / 2 - 1,
            width: top.width.saturating_sub(picture.width + 4),
            height: 2,
        };
        let lines = vec![
            Line::from(Span::styled("Assignee", Style::new().fg(theme.muted))),
            Line::from(Span::styled(
                who.display_name.clone(),
                Style::new().fg(theme.fg).add_modifier(Modifier::BOLD),
            )),
        ];
        frame.render_widget(Paragraph::new(lines), name);
        rest
    }

    /// The first image attachment, at the foot of the right-hand column,
    /// when there is room for it; the rest of the column is what is left.
    fn draw_shot(
        &self,
        frame: &mut Frame,
        area: Rect,
        theme: &Theme,
        shot: Option<&mut Picture>,
    ) -> Rect {
        let (Some(protocol), Some(file)) =
            (shot, self.issue.attachments.iter().find(|a| a.is_image()))
        else {
            return area;
        };
        if area.height < SHOT_ROWS + 8 {
            return area;
        }
        let [rest, title, picture] = Layout::vertical([
            Constraint::Min(0),
            Constraint::Length(1),
            Constraint::Length(SHOT_ROWS),
        ])
        .areas(area);
        let label =
            Line::from(Span::styled(format!("  {}", file.name), Style::new().fg(theme.muted)));
        frame.render_widget(Paragraph::new(label), title);
        let picture = Rect { x: picture.x + 2, width: picture.width.saturating_sub(3), ..picture };
        frame.render_stateful_widget(
            ratatui_image::StatefulImage::<Picture>::new(),
            picture,
            protocol,
        );
        rest
    }

    fn content(
        &self,
        theme: &Theme,
        cols: u16,
        sections: &[(&str, &RichText)],
    ) -> Vec<Line<'static>> {
        let issue = &self.issue;
        let mut lines = vec![];
        for (i, (name, text)) in sections.iter().enumerate() {
            if i > 0 {
                lines.push(Line::default());
            }
            lines.extend(heading(glyph::icon::TEXT, name, theme));
            if text.is_empty() {
                lines
                    .push(Line::from(Span::styled("No description", Style::new().fg(theme.muted))));
            } else {
                lines.extend(rich::lines(text, theme, cols));
            }
        }
        let children = self.children().into_iter().map(|r| ref_line(r, "", theme)).collect();
        titled(&mut lines, (glyph::icon::CHILDREN, "Child issues"), children, theme);
        let links = issue.links.iter().map(|l| ref_line(&l.issue, &l.relation, theme)).collect();
        titled(&mut lines, (glyph::icon::LINK, "Linked issues"), links, theme);
        let attachments = issue.attachments.iter().map(|a| attachment(a, theme)).collect();
        titled(&mut lines, (glyph::icon::ATTACHMENT, "Attachments"), attachments, theme);
        lines.push(Line::default());
        let (tabs, lit) = self.tabs(theme);
        lines.push(Line::from(tabs));
        lines.push(glyph::rule("─", cols as usize, lit, theme));
        match &self.loaded {
            None => lines.push(Line::from(Span::styled("Loading…", Style::new().fg(theme.muted)))),
            Some(detail) => lines.extend(activity(detail, self.tab, theme, cols)),
        }
        lines
    }

    fn tabs(&self, theme: &Theme) -> (Vec<Span<'static>>, std::ops::Range<usize>) {
        let comments = self.loaded.as_ref().map_or(0, |d| d.comments.len());
        let names = [
            format!("{} Comments ({comments})", glyph::icon::COMMENTS),
            "History".into(),
            "All".into(),
        ];
        glyph::tabs(&names, self.tab.index(), theme)
    }
}

/// The lines with each match of `query` lit, the row each match starts
/// on once they are wrapped to `width`, and how many rows they take; no
/// rows without a query.
fn found(
    lines: Vec<Line<'static>>,
    query: &str,
    lit: Style,
    width: u16,
) -> (Vec<Line<'static>>, Vec<u16>, Option<u16>) {
    if query.is_empty() {
        return (lines, vec![], None);
    }
    let rows_of = |line: Line<'static>| {
        Paragraph::new(line).wrap(Wrap { trim: false }).line_count(width) as u16
    };
    let mut rows = vec![];
    let mut row: u16 = 0;
    let mut out = vec![];
    for line in lines {
        // The spans as they hold it: `Line`'s `Display` drops newlines,
        // and `mark` cuts the spans by their own bytes.
        let text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
        let ranges = search::find(&text, query);
        // Known limit: the text before a match is wrapped alone, which can
        // put the match a row off when the line breaks right there.
        let start = |r: &std::ops::Range<usize>| {
            row + rows_of(Line::raw(text[..r.start].to_string())).saturating_sub(1)
        };
        rows.extend(ranges.iter().map(start));
        row += rows_of(line.clone());
        out.push(search::mark(line, &ranges, lit));
    }
    (out, rows, Some(row))
}

/// The crumbs, the title, a blank line, the status and who reported it.
const HEADER_ROWS: u16 = 7;

/// gh-dash's preview header: the crumbs, the title bold in the middle of
/// three rows; then the status as a pill and who opened the issue, when.
fn header(issue: &Issue, theme: &Theme, cols: u16) -> Vec<Line<'static>> {
    let muted = Style::new().fg(theme.muted);
    let mut crumbs = vec![Span::raw(" ")];
    crumbs.extend(issue.key.project().map(|p| Span::styled(p.to_string(), muted)));
    if let Some(parent) = &issue.parent {
        crumbs.push(Span::styled(" › ", muted));
        crumbs.push(Span::styled(
            glyph::kind(parent.kind),
            Style::new().fg(theme.issue_type(parent.kind)),
        ));
        crumbs.push(Span::styled(format!(" {} {}", parent.key, parent.title), muted));
    }
    crumbs.push(Span::styled(" · ", muted));
    crumbs.push(Span::styled(issue.key.to_string(), muted));
    let title = super::text::truncate(&issue.title, (cols as usize).saturating_sub(4));
    let mut status = vec![Span::raw(" ")];
    status.extend(glyph::status_badge(issue, theme));
    status.push(Span::styled(
        format!(" {} {}", glyph::kind(issue.kind.kind), issue.kind.name),
        Style::new().fg(theme.issue_type(issue.kind.kind)),
    ));
    if let Some(r) = &issue.reporter {
        status.push(Span::styled(" ⋅ by ", muted));
        status.push(Span::styled(
            r.display_name.clone(),
            Style::new().fg(theme.fg).add_modifier(Modifier::BOLD),
        ));
    }
    status.push(Span::styled(format!(" ⋅ {}", glyph::datetime(issue.created)), muted));
    vec![
        Line::from(crumbs),
        Line::default(),
        Line::from(vec![
            Span::raw(" "),
            glyph::kind_span(issue, theme),
            Span::raw(" "),
            Span::styled(title, Style::new().fg(theme.fg).add_modifier(Modifier::BOLD)),
        ]),
        Line::default(),
        Line::default(),
        Line::from(status),
        Line::default(),
    ]
}

fn ref_line(r: &IssueRef, relation: &str, theme: &Theme) -> Line<'static> {
    let mut spans = vec![];
    if !relation.is_empty() {
        spans.push(Span::styled(format!("{relation:<14} "), Style::new().fg(theme.muted)));
    }
    spans.push(Span::styled(glyph::kind(r.kind), Style::new().fg(theme.issue_type(r.kind))));
    spans.push(Span::styled(format!(" {} ", r.key), Style::new().fg(theme.muted)));
    spans.push(Span::raw(r.title.clone()));
    if let Some(status) = &r.status {
        spans.push(Span::styled(
            format!("  {}", status.name),
            Style::new().fg(theme.status(status.category)),
        ));
    }
    Line::from(spans)
}

/// Comments, history, or both by date, newest last as on the web.
fn activity(detail: &IssueDetail, tab: Tab, theme: &Theme, cols: u16) -> Vec<Line<'static>> {
    let mut entries: Vec<(time::OffsetDateTime, Vec<Line<'static>>)> = vec![];
    let who = |name: &str, at| {
        Line::from(vec![
            Span::styled(name.to_string(), Style::new().fg(theme.fg).add_modifier(Modifier::BOLD)),
            Span::styled(format!("  {}", glyph::datetime(at)), Style::new().fg(theme.muted)),
        ])
    };
    if tab != Tab::History {
        for c in &detail.comments {
            let mut lines = vec![who(&c.author.display_name, c.created)];
            lines.extend(rich::lines(&c.body, theme, cols));
            entries.push((c.created, lines));
        }
    }
    if tab != Tab::Comments {
        for a in &detail.activity {
            let mut lines = vec![who(&a.author.display_name, a.at)];
            for ch in &a.changes {
                lines.push(Line::from(vec![
                    Span::styled(format!("{}: ", ch.field), Style::new().fg(theme.muted)),
                    Span::raw(ch.from.clone().unwrap_or_else(|| "∅".into())),
                    Span::styled(" → ", Style::new().fg(theme.muted)),
                    Span::raw(ch.to.clone().unwrap_or_else(|| "∅".into())),
                ]));
            }
            entries.push((a.at, lines));
        }
    }
    entries.sort_by_key(|(at, _)| *at);
    if entries.is_empty() {
        return vec![Line::from(Span::styled("Nothing yet", Style::new().fg(theme.muted)))];
    }
    let mut out = vec![];
    for (i, (_, lines)) in entries.into_iter().enumerate() {
        if i > 0 {
            out.push(Line::default());
        }
        out.extend(lines);
    }
    out
}

/// Which fields the detail shows: the config's lists where it has them,
/// else the provider's layout, else kolum's own choice.
pub struct Shown<'a> {
    pub fields: Option<&'a [String]>,
    pub sections: Option<&'a [String]>,
    pub layout: Option<&'a FieldLayout>,
}

/// The fields of the right-hand column, in the order kolum shows them by
/// default: the members, but the title and the description, which have
/// their own places.
fn builtin_fields() -> impl Iterator<Item = String> {
    let beside = FieldId::members()
        .map(|(id, _)| id)
        .filter(|id| ![FieldId::TITLE, FieldId::DESCRIPTION].contains(id));
    std::iter::once("status").chain(beside).chain(["created", "updated"]).map(String::from)
}

/// The `wanted` fields, or the built-in ones then every custom field the
/// issue fills that is not rich text (that shows under the description).
/// Each with the label the provider's view gives it, when it has a layout.
fn field_names(
    issue: &Issue,
    wanted: Option<&[String]>,
    layout: Option<&FieldLayout>,
) -> Vec<(String, Option<String>)> {
    if let Some(names) = wanted {
        return names.iter().map(|n| (n.clone(), None)).collect();
    }
    // The web shows the status apart, above its fields; here it heads them.
    if let Some(layout) = layout {
        return std::iter::once(("status".to_string(), None))
            .chain(
                layout
                    .side
                    .iter()
                    .filter(|f| !f.only_filled || filled(issue, f.id.as_str()))
                    .map(|f| (f.id.to_string(), Some(f.name.clone()))),
            )
            .collect();
    }
    let custom = issue
        .fields
        .iter()
        .filter(|f| !matches!(f.value, FieldValue::RichText(_)) && !blank(&f.value));
    let builtin = builtin_fields();
    builtin.chain(custom.map(|f| f.id.to_string())).map(|n| (n, None)).collect()
}

/// Whether the issue holds something in a field: by kolum's id for the
/// ones it reads into the issue itself, by id or name for the others.
fn filled(issue: &Issue, field: &str) -> bool {
    match field {
        "status" | "created" | "updated" => true,
        // A time estimate is no field value, but it is an estimate.
        FieldId::ESTIMATE => issue.estimate.is_some(),
        other => value_of(issue, other).is_some_and(|v| !blank(&v)),
    }
}

/// A member's label, when the field is one.
fn member_label(field: &str) -> Option<&'static str> {
    FieldId::members().find(|(id, _)| *id == field).map(|(_, label)| label)
}

/// A field's value: a member's from the member alone, a custom field's by
/// id or name.
fn value_of(issue: &Issue, field: &str) -> Option<FieldValue> {
    match member_label(field) {
        Some(_) => issue.value(&FieldId::new(field)),
        None => find_field(issue, field).map(|f| f.value.clone()),
    }
}

fn label_of(issue: &Issue, field: &str) -> String {
    match member_label(field) {
        Some(label) => label.to_string(),
        None => find_field(issue, field).map_or_else(|| field.to_string(), |f| f.name.clone()),
    }
}

fn blank(value: &FieldValue) -> bool {
    match value {
        FieldValue::Text(t) => t.is_empty(),
        FieldValue::Options(o) => o.is_empty(),
        FieldValue::RichText(t) => t.is_empty(),
        _ => false,
    }
}

/// A custom field by id (`customfield_10016`) or by name, in any case.
fn find_field<'a>(issue: &'a Issue, name: &str) -> Option<&'a CustomField> {
    issue.fields.iter().find(|f| f.id.as_str() == name || f.name.eq_ignore_ascii_case(name))
}

/// The rich text custom fields the issue fills: the `wanted` ones in
/// their order, or every one when the config lists none.
fn body_sections<'a>(
    issue: &'a Issue,
    wanted: Option<&[String]>,
    layout: Option<&FieldLayout>,
) -> Vec<(&'a str, &'a RichText)> {
    let names: Vec<String> = match (wanted, layout) {
        (Some(names), _) => {
            std::iter::once("description".into()).chain(names.iter().cloned()).collect()
        }
        (None, Some(layout)) => layout.body.iter().map(|id| id.to_string()).collect(),
        (None, None) => std::iter::once("description".into())
            .chain(issue.fields.iter().map(|f| f.id.to_string()))
            .collect(),
    };
    names
        .iter()
        .filter_map(|name| match find_field(issue, name) {
            _ if name == "description" => Some(("Description", issue.description_or_empty())),
            Some(CustomField { name, value: FieldValue::RichText(text), .. })
                if !text.is_empty() =>
            {
                Some((name.as_str(), text))
            }
            _ => None,
        })
        .collect()
}

/// The right-hand column, in the order the config lists the fields. A
/// field the issue has no value for is shown as such rather than left
/// out, so the column keeps its shape from one issue to the next.
fn field_lines(
    issue: &Issue,
    fields: &[(String, Option<String>)],
    theme: &Theme,
    me: Option<&UserId>,
) -> Vec<Line<'static>> {
    let muted = Style::new().fg(theme.muted);
    let none = || vec![Span::styled("None", muted)];
    let mut lines = vec![];
    for (name, shown_as) in fields {
        let (label, value): (String, Vec<Span<'static>>) = match name.as_str() {
            "status" => ("Status".into(), glyph::status_badge(issue, theme).into()),
            FieldId::ESTIMATE => (
                label_of(issue, FieldId::ESTIMATE),
                issue.estimate.as_ref().map_or_else(none, |e| vec![Span::raw(glyph::estimate(e))]),
            ),
            "created" => ("Created".into(), vec![Span::raw(glyph::datetime(issue.created))]),
            "updated" => ("Updated".into(), vec![Span::raw(glyph::datetime(issue.updated))]),
            other => {
                (label_of(issue, other), value_spans(other, value_of(issue, other), theme, me))
            }
        };
        let label = shown_as.clone().unwrap_or(label);
        lines.push(Line::from(Span::styled(label, muted.add_modifier(Modifier::BOLD))));
        lines.push(Line::from(value));
        lines.push(Line::default());
    }
    lines
}

/// A field's value as the column shows it: the members each their way
/// (a priority with its glyph, labels as badges, you starred), any other
/// as text.
fn value_spans(
    field: &str,
    value: Option<FieldValue>,
    theme: &Theme,
    me: Option<&UserId>,
) -> Vec<Span<'static>> {
    let muted = Style::new().fg(theme.muted);
    match (field, value) {
        (FieldId::ASSIGNEE | FieldId::REPORTER, None) => vec![Span::styled("Unassigned", muted)],
        (_, None) => vec![Span::styled("None", muted)],
        (FieldId::ASSIGNEE | FieldId::REPORTER, Some(FieldValue::User(u))) => {
            user_spans(&u, theme, me)
        }
        (FieldId::PRIORITY, Some(FieldValue::Priority(p))) => {
            vec![glyph::priority_span(&p, theme), Span::raw(format!(" {}", p.name))]
        }
        (FieldId::LABELS, Some(FieldValue::Options(labels))) => label_spans(&labels, theme),
        (FieldId::EPIC, Some(FieldValue::Issue(p))) => {
            vec![Span::styled(format!("◆ {}", p.title), Style::new().fg(theme.type_epic))]
        }
        (_, Some(value)) => vec![Span::raw(field_value(&value))],
    }
}

/// A person, starred when it is you.
fn user_spans(u: &kolum_domain::User, theme: &Theme, me: Option<&UserId>) -> Vec<Span<'static>> {
    let mut s = vec![Span::raw(u.display_name.clone())];
    if me.is_some_and(|m| *m == u.id) {
        s.push(Span::styled(" ★", Style::new().fg(theme.warn)));
    }
    s
}

fn label_spans(labels: &[String], theme: &Theme) -> Vec<Span<'static>> {
    if labels.is_empty() {
        return vec![Span::styled("None", Style::new().fg(theme.muted))];
    }
    labels
        .iter()
        .flat_map(|l| glyph::label_badge(l, theme).into_iter().chain([Span::raw(" ")]))
        .collect()
}

fn field_value(value: &FieldValue) -> String {
    match value {
        FieldValue::Options(o) => o.join(", "),
        other => other.to_input(),
    }
}

/// gh-dash's section titles: an icon, the name bold and underlined, and a
/// blank line under it.
fn heading(icon: &str, title: &str, theme: &Theme) -> Vec<Line<'static>> {
    vec![
        Line::from(Span::styled(
            format!("{icon} {title}"),
            Style::new().fg(theme.fg).add_modifier(Modifier::BOLD | Modifier::UNDERLINED),
        )),
        Line::default(),
    ]
}

/// A section under its title, after a blank line; none when it is empty.
fn titled(
    lines: &mut Vec<Line<'static>>,
    (icon, title): (&str, &str),
    body: Vec<Line<'static>>,
    theme: &Theme,
) {
    if body.is_empty() {
        return;
    }
    lines.push(Line::default());
    lines.extend(heading(icon, title, theme));
    lines.extend(body);
}

fn attachment(a: &Attachment, theme: &Theme) -> Line<'static> {
    Line::from(vec![
        Span::styled("▪ ", Style::new().fg(theme.muted)),
        Span::raw(a.name.clone()),
        Span::styled(format!("  {}  {}", a.mime, size(a.size)), Style::new().fg(theme.muted)),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::test_support::{find, render, theme};

    #[test]
    fn sizes_read_like_a_file_browser() {
        assert_eq!(size(512), "512 B");
        assert_eq!(size(48213), "47 KB");
        assert_eq!(size(1_900_000), "1.8 MB");
        assert_eq!([size(1023), size(1024)], ["1023 B", "1 KB"]);
        assert_eq!([size(1024 * 1024 - 1), size(1024 * 1024)], ["1023 KB", "1.0 MB"]);
    }

    async fn acme(key: &str) -> Issue {
        use kolum_domain::IssueReader;
        let key = kolum_domain::IssueKey::parse(key);
        kolum_provider_fake::FakeProvider::new().issue(&key).await.unwrap()
    }

    fn picture() -> Picture {
        let picker = ratatui_image::picker::Picker::halfblocks();
        let red = image::RgbImage::from_pixel(16, 16, image::Rgb([255, 0, 0]));
        picker.new_resize_protocol(image::DynamicImage::ImageRgb8(red))
    }

    /// The detail drawn on a `width` by `height` screen, with the
    /// pictures given.
    fn drawn(
        view: &mut DetailView,
        (width, height): (u16, u16),
        avatar: Option<&mut Picture>,
        shot: Option<&mut Picture>,
    ) -> ratatui::buffer::Buffer {
        let shown = Shown { fields: None, sections: None, layout: None };
        let pictures = Pictures { avatar, shot };
        render(width, height, |f| view.draw(f, f.area(), &theme(), shown, None, pictures))
    }

    #[tokio::test]
    async fn the_fields_go_beside_the_text_from_a_hundred_columns() {
        let mut view = DetailView::new(acme("ACME-10").await);
        let (x, _) = find(&drawn(&mut view, (100, 40), None, None), "Status").unwrap();
        assert!(x >= 60, "beside, at {x}");
        let (x, _) = find(&drawn(&mut view, (99, 60), None, None), "Status").unwrap();
        assert!(x < 10, "under, at {x}");
    }

    #[tokio::test]
    async fn a_screen_as_tall_as_the_header_shows_nothing_under_it() {
        let mut view = DetailView::new(acme("ACME-10").await);
        let screen = drawn(&mut view, (60, 7), None, None);
        let last: String = (0..60).map(|x| screen[(x, 6)].symbol()).collect();
        assert_eq!(last.trim(), "", "the header's own blank row");
    }

    /// The text keeps a blank column before the fields' border.
    #[tokio::test]
    async fn a_gap_column_separates_the_text_from_the_fields() {
        let mut issue = acme("ACME-10").await;
        issue.description = Some(RichText::from_markdown(&"x".repeat(300)));
        let mut view = DetailView::new(issue);
        let screen = drawn(&mut view, (100, 40), None, None);
        let (_, y) = find(&screen, "xxxxxxxxxx").unwrap();
        let border = (0..100).find(|&x| screen[(x, y)].symbol() == "│").unwrap();
        assert_eq!(screen[(border - 1, y)].symbol(), " ");
    }

    #[tokio::test]
    async fn half_a_page_is_half_the_text_area_and_one_line_at_least() {
        let mut view = DetailView::new(acme("ACME-10").await);
        drawn(&mut view, (100, 47), None, None);
        assert_eq!(view.half_page(), 20, "40 rows under the header");
        drawn(&mut view, (100, 8), None, None);
        assert_eq!(view.half_page(), 1);
    }

    #[tokio::test]
    async fn the_text_starts_with_its_first_heading_and_the_tab_counts_no_comment_yet() {
        let view = DetailView::new(acme("ACME-10").await);
        let empty = RichText::default();
        let lines = view.content(&theme(), 80, &[("Description", &empty), ("Notes", &empty)]);
        assert!(lines[0].to_string().contains("Description"), "{:?}", lines[0]);
        let notes = lines.iter().position(|l| l.to_string().contains("Notes")).unwrap();
        assert_eq!(lines[notes - 1].to_string(), "", "a blank line between sections");
        let (tabs, _) = view.tabs(&theme());
        let tabs: String = tabs.iter().map(|s| s.content.to_string()).collect();
        assert!(tabs.contains("Comments (0)"), "{tabs}");
        let mut view = view;
        view.tab = view.tab.step(1);
        assert_eq!(view.tab, Tab::History);
        let (spans, lit) = view.tabs(&theme());
        let text: String = spans.iter().map(|s| s.content.to_string()).collect();
        let lit: String = text.chars().skip(lit.start).take(lit.len()).collect();
        assert!(lit.contains("History"), "the tab shown is the one lit: {lit:?}");
        assert_eq!(view.tab.step(-2), Tab::All, "round the other way");
    }

    #[tokio::test]
    async fn the_assignees_picture_heads_the_fields_with_the_name_beside_it() {
        let issue = acme("ACME-10").await;
        let who = issue.assignee.clone().unwrap().display_name;
        let mut view = DetailView::new(issue);
        let mut avatar = picture();
        let screen = drawn(&mut view, (100, 40), Some(&mut avatar), None);
        let (x, y) = find(&screen, "Assignee").unwrap();
        let (nx, ny) = find(&screen, &who).unwrap();
        // The column starts at 65 of 100; the picture is 8 wide, 2 in.
        assert_eq!((x, y, nx, ny), (65 + 2 + 8 + 2, 7 + 1, 65 + 12, 7 + 2));
        let (_, status) = find(&screen, "Status").unwrap();
        assert_eq!(status, 7 + 5, "the fields under the picture and a blank row");
    }

    #[tokio::test]
    async fn no_room_no_picture() {
        let mut view = DetailView::new(acme("ACME-10").await);
        let mut avatar = picture();
        // The right-hand column: 35 of 100, under the 7 header rows.
        let screen = drawn(&mut view, (100, 7 + 7), Some(&mut avatar), None);
        assert_eq!(find(&screen, "Assignee").map(|(x, _)| x < 70), Some(true), "the field");
        let screen = drawn(&mut view, (100, 7 + 8), Some(&mut avatar), None);
        assert_eq!(find(&screen, "Assignee").map(|(x, _)| x), Some(77), "the picture");
        let area = Rect { x: 0, y: 0, width: 19, height: 20 };
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(19, 20)).unwrap();
        let mut long = view.issue.clone();
        long.assignee.as_mut().unwrap().display_name = "Maximilian Bartholomew".into();
        let view = DetailView::new(long);
        terminal
            .draw(|f| assert_eq!(view.draw_avatar(f, area, &theme(), Some(&mut avatar)), area))
            .unwrap();
        let area = Rect { width: 20, ..area };
        terminal
            .draw(|f| assert_ne!(view.draw_avatar(f, area, &theme(), Some(&mut avatar)), area))
            .unwrap();
        let mut wide = ratatui::Terminal::new(ratatui::backend::TestBackend::new(40, 20)).unwrap();
        wide.draw(|f| {
            view.draw_avatar(f, area, &theme(), Some(&mut avatar));
        })
        .unwrap();
        let screen = wide.backend().buffer().clone();
        assert!(
            (0..20).all(|y| (20..40).all(|x| screen[(x, y)].symbol() == " ")),
            "a long name stops at the column"
        );
    }

    #[tokio::test]
    async fn an_image_attachment_shows_at_the_foot_of_the_fields() {
        let mut issue = acme("ACME-10").await;
        issue.attachments = vec![kolum_domain::Attachment {
            name: "shot.png".into(),
            url: "u".into(),
            mime: "image/png".into(),
            size: 1,
        }];
        let view = DetailView::new(issue);
        let mut shot = picture();
        let area = Rect { x: 0, y: 0, width: 30, height: SHOT_ROWS + 8 };
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(30, SHOT_ROWS + 8)).unwrap();
        terminal
            .draw(|f| {
                let rest = view.draw_shot(f, area, &theme(), Some(&mut shot));
                assert_eq!(rest.height, 7, "what is left above its title");
            })
            .unwrap();
        let screen = terminal.backend().buffer().clone();
        assert_eq!(find(&screen, "shot.png"), Some((2, 7)));
        assert_eq!(screen[(1, 8)], ratatui::buffer::Cell::default(), "the picture starts two in");
        assert_ne!(screen[(2, 8)], ratatui::buffer::Cell::default());
        let short = Rect { height: SHOT_ROWS + 7, ..area };
        terminal
            .draw(|f| assert_eq!(view.draw_shot(f, short, &theme(), Some(&mut shot)), short))
            .unwrap();
    }

    #[tokio::test]
    async fn the_fields_list_the_custom_ones_that_hold_text_and_status_is_always_there() {
        let mut issue = acme("ACME-10").await;
        issue.fields = vec![
            CustomField {
                id: FieldId::new("c1"),
                name: "Team".into(),
                value: FieldValue::Text("Mobile".into()),
            },
            CustomField {
                id: FieldId::new("c2"),
                name: "Notes".into(),
                value: FieldValue::RichText(RichText::from_markdown("n")),
            },
            CustomField {
                id: FieldId::new("c3"),
                name: "Empty".into(),
                value: FieldValue::Text(String::new()),
            },
        ];
        let names: Vec<String> =
            field_names(&issue, None, None).into_iter().map(|(n, _)| n).collect();
        assert!(names.contains(&"c1".to_string()));
        assert!(
            !names.contains(&"c2".to_string()) && !names.contains(&"c3".to_string()),
            "{names:?}"
        );
        assert!(["status", "created", "updated"].iter().all(|f| filled(&issue, f)));
    }
    use kolum_domain::{RichText, User};

    /// A member reads its member alone: a custom field of the same name
    /// (a GitHub single-select "Epic", a "Due") is not its value.
    #[tokio::test]
    async fn a_member_reads_only_its_member() {
        use kolum_domain::IssueReader;
        let key = kolum_domain::IssueKey::parse("ACME-10");
        let mut issue = kolum_provider_fake::FakeProvider::new().issue(&key).await.unwrap();
        let text = |t: &str| FieldValue::Text(t.into());
        for (name, value) in [("Epic", "Q4 launch"), ("Due", "Friday")] {
            let id = FieldId::new(name);
            issue.fields.push(CustomField { id, name: name.into(), value: text(value) });
        }
        assert_eq!((issue.parent.as_ref(), issue.due), (None, None));
        assert!(!filled(&issue, FieldId::EPIC) && !filled(&issue, FieldId::DUE));
        assert_eq!(value_of(&issue, FieldId::EPIC), None);
        assert_eq!(value_of(&issue, "epic "), None, "no such field");
        assert_eq!(value_of(&issue, "Epic"), Some(text("Q4 launch")), "the custom one, by name");
        assert_eq!(value_of(&issue, "due"), None);
    }

    /// A history entry holds the description's text, newlines and all.
    #[test]
    fn a_match_after_a_newline_is_lit_where_it_is() {
        let line = Line::from(vec![Span::raw("a"), Span::raw("\nÉté")]);
        let lit = Style::new().bg(ratatui::style::Color::Yellow);
        let (lines, rows, length) = found(vec![line], "été", lit, 80);
        let spans: Vec<(&str, Style)> =
            lines[0].spans.iter().map(|s| (s.content.as_ref(), s.style)).collect();
        assert_eq!(spans, [("a", Style::new()), ("\n", Style::new()), ("Été", lit)]);
        assert_eq!((rows, length), (vec![0], Some(1)));
        assert_eq!(found(vec![Line::raw("x")], "x", lit, 0).1, [0], "no room, no underflow");
    }

    #[test]
    fn every_custom_field_value_reads_as_text() {
        let ana = User { id: UserId::new("a"), display_name: "Ana".into(), avatar_url: None };
        let cases = [
            (FieldValue::Text("Mobile".into()), "Mobile"),
            (FieldValue::Number(3.5), "3.5"),
            (FieldValue::Date(time::macros::date!(2026 - 11 - 03)), "2026-11-03"),
            (FieldValue::User(ana), "Ana"),
            (FieldValue::Options(vec!["a".into(), "b".into()]), "a, b"),
            (FieldValue::RichText(RichText::from_markdown("**hi** there")), "hi there"),
        ];
        for (value, text) in cases {
            assert_eq!(field_value(&value), text);
        }
    }
}
