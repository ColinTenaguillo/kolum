//! An issue, laid out like the web app: the content on the left (about
//! two thirds), the fields on the right. On a narrow terminal the fields
//! go under the content.

use super::{glyph, rich};
use crate::theme::Theme;
use kolum_app::IssueDetail;
use kolum_domain::{FieldValue, Issue, IssueRef, UserId};
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use std::cell::Cell;

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
    const ORDER: [Tab; 3] = [Tab::Comments, Tab::History, Tab::All];

    pub fn step(self, delta: i64) -> Tab {
        let i = Tab::ORDER.iter().position(|&t| t == self).unwrap_or(0) as i64;
        Tab::ORDER[(i + delta).rem_euclid(3) as usize]
    }
}

pub struct DetailView {
    pub issue: Issue,
    /// Comments and history arrive after the view opens.
    pub loaded: Option<IssueDetail>,
    pub tab: Tab,
    pub scroll: u16,
    /// Rows of content on screen at the last draw, for half-page moves.
    page: Cell<u16>,
    /// Content rows at the last draw, so `G` and scrolling stop at the end.
    length: Cell<u16>,
}

impl DetailView {
    pub fn new(issue: Issue) -> Self {
        DetailView {
            issue,
            loaded: None,
            tab: Tab::Comments,
            scroll: 0,
            page: Cell::new(10),
            length: Cell::new(0),
        }
    }

    pub fn scroll_by(&mut self, delta: i64) {
        let max = self.length.get().saturating_sub(self.page.get()) as i64;
        self.scroll = (self.scroll as i64 + delta).clamp(0, max.max(0)) as u16;
    }

    pub fn half_page(&self) -> i64 {
        (self.page.get() / 2).max(1) as i64
    }

    pub fn draw(
        &self,
        frame: &mut Frame,
        area: Rect,
        theme: &Theme,
        fields: &[String],
        me: Option<&UserId>,
        avatar: Option<&mut ratatui_image::protocol::StatefulProtocol>,
    ) {
        let issue = &self.issue;
        let [head, body] =
            Layout::vertical([Constraint::Length(3), Constraint::Min(0)]).areas(area);
        frame.render_widget(Paragraph::new(header(issue, theme)), head);

        let side = body.width >= SIDE_BY_SIDE_MIN_WIDTH;
        let [left, right] = if side {
            Layout::horizontal([Constraint::Percentage(65), Constraint::Percentage(35)]).areas(body)
        } else {
            [body, Rect::default()]
        };
        let mut content = self.content(theme, left.width.saturating_sub(2));
        let field_lines = field_lines(issue, fields, theme, me);
        if side {
            let right = self.draw_avatar(frame, right, theme, avatar);
            let block =
                Block::new().borders(Borders::LEFT).border_style(Style::new().fg(theme.border));
            let fields = Paragraph::new(field_lines).block(block).wrap(Wrap { trim: false });
            frame.render_widget(fields, right);
        } else {
            content.push(Line::default());
            content.extend(field_lines);
        }
        let paragraph = Paragraph::new(content).wrap(Wrap { trim: false });
        self.page.set(left.height);
        self.length.set(paragraph.line_count(left.width.saturating_sub(1)) as u16);
        frame.render_widget(
            paragraph.scroll((self.scroll, 0)),
            Rect { width: left.width.saturating_sub(1), ..left },
        );
    }

    /// The assignee's picture and name above the fields, when there is a
    /// picture; the rest of the column is what is left.
    fn draw_avatar(
        &self,
        frame: &mut Frame,
        area: Rect,
        theme: &Theme,
        avatar: Option<&mut ratatui_image::protocol::StatefulProtocol>,
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

    fn content(&self, theme: &Theme, cols: u16) -> Vec<Line<'static>> {
        let issue = &self.issue;
        let section = |t: &str| {
            Line::from(Span::styled(
                t.to_string(),
                Style::new().fg(theme.fg).add_modifier(Modifier::BOLD),
            ))
        };
        let mut lines = vec![];
        if issue.description.is_empty() {
            lines.push(Line::from(Span::styled("No description", Style::new().fg(theme.muted))));
        } else {
            lines.extend(rich::lines(&issue.description, theme, cols));
        }
        if !issue.subtasks.is_empty() {
            lines.extend([Line::default(), section("Child issues")]);
            lines.extend(issue.subtasks.iter().map(|r| ref_line(r, "", theme)));
        }
        if !issue.links.is_empty() {
            lines.extend([Line::default(), section("Linked issues")]);
            lines.extend(issue.links.iter().map(|l| ref_line(&l.issue, &l.relation, theme)));
        }
        lines.push(Line::default());
        lines.push(self.tabs(theme));
        lines.push(Line::from(Span::styled(
            "─".repeat(cols as usize),
            Style::new().fg(theme.border),
        )));
        match &self.loaded {
            None => lines.push(Line::from(Span::styled("Loading…", Style::new().fg(theme.muted)))),
            Some(detail) => lines.extend(activity(detail, self.tab, theme, cols)),
        }
        lines
    }

    fn tabs(&self, theme: &Theme) -> Line<'static> {
        let comments = self.loaded.as_ref().map_or(0, |d| d.comments.len());
        let names = [
            (Tab::Comments, format!("Comments ({comments})")),
            (Tab::History, "History".into()),
            (Tab::All, "All".into()),
        ];
        let mut spans = vec![];
        for (tab, name) in names {
            let style = if tab == self.tab {
                Style::new().fg(theme.accent).add_modifier(Modifier::BOLD | Modifier::UNDERLINED)
            } else {
                Style::new().fg(theme.muted)
            };
            spans.extend([Span::styled(name, style), Span::raw("   ")]);
        }
        Line::from(spans)
    }
}

fn header(issue: &Issue, theme: &Theme) -> Vec<Line<'static>> {
    let muted = Style::new().fg(theme.muted);
    let mut crumbs = vec![];
    let project = issue.key.as_str().split('-').next().unwrap_or_default().to_string();
    crumbs.push(Span::styled(project, muted));
    if let Some(parent) = &issue.parent {
        crumbs.push(Span::styled(" › ", muted));
        crumbs.push(Span::styled(
            glyph::kind(parent.kind),
            Style::new().fg(theme.issue_type(parent.kind)),
        ));
        crumbs.push(Span::styled(format!(" {} {}", parent.key, parent.title), muted));
    }
    crumbs.push(Span::styled(" › ", muted));
    crumbs.push(Span::styled(issue.key.to_string(), muted));
    vec![
        Line::from(crumbs),
        Line::from(vec![
            glyph::kind_span(issue, theme),
            Span::raw(" "),
            Span::styled(
                issue.title.clone(),
                Style::new().fg(theme.fg).add_modifier(Modifier::BOLD),
            ),
        ]),
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

/// The right-hand column, in the order the config lists the fields. A
/// field the issue has no value for is shown as such rather than left
/// out, so the column keeps its shape from one issue to the next.
fn field_lines(
    issue: &Issue,
    fields: &[String],
    theme: &Theme,
    me: Option<&UserId>,
) -> Vec<Line<'static>> {
    let muted = Style::new().fg(theme.muted);
    let none = || vec![Span::styled("None", muted)];
    let user = |u: &Option<kolum_domain::User>| match u {
        Some(u) => {
            let mut s = vec![Span::raw(u.display_name.clone())];
            if me.is_some_and(|m| *m == u.id) {
                s.push(Span::styled(" ★", Style::new().fg(theme.warn)));
            }
            s
        }
        None => vec![Span::styled("Unassigned", muted)],
    };
    let mut lines = vec![];
    for name in fields {
        let (label, value): (String, Vec<Span<'static>>) = match name.as_str() {
            "status" => ("Status".into(), vec![glyph::status_badge(issue, theme)]),
            "assignee" => ("Assignee".into(), user(&issue.assignee)),
            "reporter" => ("Reporter".into(), user(&issue.reporter)),
            "priority" => (
                "Priority".into(),
                match issue.priority {
                    Some(p) => {
                        vec![glyph::priority_span(p, theme), Span::raw(format!(" {}", p.name()))]
                    }
                    None => none(),
                },
            ),
            "sprint" => (
                "Sprint".into(),
                issue.sprint.as_ref().map_or_else(none, |s| vec![Span::raw(s.name.clone())]),
            ),
            "epic" => (
                "Epic".into(),
                issue.parent.as_ref().map_or_else(none, |p| {
                    vec![Span::styled(format!("◆ {}", p.title), Style::new().fg(theme.type_epic))]
                }),
            ),
            "labels" if issue.labels.is_empty() => ("Labels".into(), none()),
            "labels" => (
                "Labels".into(),
                issue
                    .labels
                    .iter()
                    .flat_map(|l| [glyph::label_badge(l, theme), Span::raw(" ")])
                    .collect(),
            ),
            "estimate" => (
                "Estimate".into(),
                issue.estimate.as_ref().map_or_else(none, |e| vec![Span::raw(glyph::estimate(e))]),
            ),
            "due" => {
                ("Due".into(), issue.due.map_or_else(none, |d| vec![Span::raw(d.to_string())]))
            }
            "created" => ("Created".into(), vec![Span::raw(glyph::datetime(issue.created))]),
            "updated" => ("Updated".into(), vec![Span::raw(glyph::datetime(issue.updated))]),
            other => match issue
                .fields
                .iter()
                .find(|f| f.id.as_str() == other || f.name.eq_ignore_ascii_case(other))
            {
                Some(f) => (f.name.clone(), vec![Span::raw(field_value(&f.value))]),
                None => (other.to_string(), none()),
            },
        };
        lines.push(Line::from(Span::styled(label, muted)));
        lines.push(Line::from(value));
        lines.push(Line::default());
    }
    lines
}

fn field_value(value: &FieldValue) -> String {
    match value {
        FieldValue::Text(t) => t.clone(),
        FieldValue::Number(n) => n.to_string(),
        FieldValue::Date(d) => d.to_string(),
        FieldValue::User(u) => u.display_name.clone(),
        FieldValue::Options(o) => o.join(", "),
        FieldValue::RichText(r) => r.plain_text(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kolum_domain::{RichText, User};

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
