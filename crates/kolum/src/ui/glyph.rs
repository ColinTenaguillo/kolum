//! The small signs cards and lists share. Single-width characters only:
//! emoji are two cells wide in some terminals and one in others, which
//! breaks every column after them.

use crate::theme::Theme;
use kolum_domain::{Estimate, Issue, IssueKind, Priority};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Span;

/// Nerd Font icons for the issue view's headings, as gh-dash's.
pub mod icon {
    pub const TEXT: &str = "\u{f036}";
    pub const CHILDREN: &str = "\u{f0e8}";
    pub const LINK: &str = "\u{f0c1}";
    pub const ATTACHMENT: &str = "\u{f0c6}";
    pub const COMMENTS: &str = "\u{f0e6}";
}

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

/// A pill, as gh-dash draws its states and labels: the text on a colour,
/// between two rounded caps of that colour. The caps are Nerd Font
/// glyphs, one cell wide like every other sign here.
pub fn pill(text: &str, bg: Color, fg: Color) -> [Span<'static>; 3] {
    [
        Span::styled("\u{e0b6}", Style::new().fg(bg)),
        Span::styled(text.to_string(), Style::new().bg(bg).fg(fg)),
        Span::styled("\u{e0b4}", Style::new().fg(bg)),
    ]
}

/// The status as a coloured pill, as the web app draws it.
pub fn status_badge(issue: &Issue, theme: &Theme) -> [Span<'static>; 3] {
    let [l, mut text, r] = pill(
        &issue.status.name.to_uppercase(),
        theme.status(issue.status.category),
        theme.badge_fg,
    );
    text.style = text.style.add_modifier(Modifier::BOLD);
    [l, text, r]
}

pub fn label_badge(label: &str, theme: &Theme) -> [Span<'static>; 3] {
    pill(label, theme.label(label), theme.badge_fg)
}

/// A row of tabs, as gh-dash draws its sections: each padded, the active
/// one bold on the selection, the others faint, a bar between them.
pub fn tabs<S: AsRef<str>>(names: &[S], active: usize, theme: &Theme) -> Vec<Span<'static>> {
    let mut spans = vec![];
    for (i, name) in names.iter().enumerate() {
        if i > 0 {
            spans.push(Span::styled("│", Style::new().fg(theme.border)));
        }
        let style = if i == active {
            Style::new().fg(theme.fg).bg(theme.selection).add_modifier(Modifier::BOLD)
        } else {
            Style::new().fg(theme.muted)
        };
        spans.push(Span::styled(format!("  {}  ", name.as_ref()), style));
    }
    spans
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn badges_keep_their_text_colour_over_a_transparent_background() {
        let mut theme = Theme::builtin("catppuccin").unwrap();
        let overrides = toml::from_str("bg = \"reset\"\nbadge_fg = \"#010203\"").unwrap();
        assert!(theme.apply(&overrides, "test").is_empty());
        assert_eq!(label_badge("LISA", &theme)[1].style.fg, Some(Color::Rgb(1, 2, 3)));
    }
}
