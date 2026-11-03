//! The small signs cards and lists share. Single-width characters only:
//! emoji are two cells wide in some terminals and one in others, which
//! breaks every column after them.

use crate::theme::Theme;
use kolum_domain::{Estimate, Issue, IssueKind, Priority};
use ratatui::style::{Modifier, Style};
use ratatui::text::Span;

pub fn kind(kind: IssueKind) -> &'static str {
    match kind {
        IssueKind::Bug => "●",
        IssueKind::Story => "■",
        IssueKind::Task => "✓",
        IssueKind::Epic => "◆",
        IssueKind::Subtask => "↳",
        IssueKind::Other => "○",
    }
}

pub fn priority(p: Priority) -> &'static str {
    match p {
        Priority::Highest => "⇈",
        Priority::High => "↑",
        Priority::Medium => "=",
        Priority::Low => "↓",
        Priority::Lowest => "⇊",
    }
}

pub fn kind_span(issue: &Issue, theme: &Theme) -> Span<'static> {
    Span::styled(kind(issue.kind.kind), Style::new().fg(theme.issue_type(issue.kind.kind)))
}

pub fn priority_span(p: Priority, theme: &Theme) -> Span<'static> {
    Span::styled(priority(p), Style::new().fg(theme.priority(p)).add_modifier(Modifier::BOLD))
}

/// The status as a coloured badge, as the web app draws it.
pub fn status_badge(issue: &Issue, theme: &Theme) -> Span<'static> {
    Span::styled(
        format!(" {} ", issue.status.name.to_uppercase()),
        Style::new()
            .bg(theme.status(issue.status.category))
            .fg(theme.bg)
            .add_modifier(Modifier::BOLD),
    )
}

pub fn label_badge(label: &str, theme: &Theme) -> Span<'static> {
    Span::styled(format!(" {label} "), Style::new().bg(theme.label(label)).fg(theme.bg))
}

pub fn estimate(e: &Estimate) -> String {
    match e {
        Estimate::Points(p) if p.fract() == 0.0 => format!("{p:.0}"),
        Estimate::Points(p) => format!("{p}"),
        Estimate::Seconds(s) if s % 3600 == 0 => format!("{}h", s / 3600),
        Estimate::Seconds(s) => format!("{}m", s / 60),
    }
}

/// `2026-09-03 14:00`: the same width everywhere, no locale.
pub fn datetime(t: time::OffsetDateTime) -> String {
    let format = time::macros::format_description!("[year]-[month]-[day] [hour]:[minute]");
    t.format(&format).unwrap_or_default()
}
