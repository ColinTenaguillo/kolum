//! The small signs cards and lists share. Single-width characters only:
//! emoji are two cells wide in some terminals and one in others, which
//! breaks every column after them.

use crate::theme::Theme;
use kolum_domain::{Estimate, Issue, IssueKind, Priority};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType};
use std::ops::Range;

/// Nerd Font icons for the issue view's headings, as gh-dash's.
pub mod icon {
    pub const TEXT: &str = "\u{f036}";
    pub const CHILDREN: &str = "\u{f0e8}";
    pub const LINK: &str = "\u{f0c1}";
    pub const ATTACHMENT: &str = "\u{f0c6}";
    pub const COMMENTS: &str = "\u{f0e6}";
    pub const SEARCH: &str = "\u{f002}";
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

/// One glyph per level, the most urgent first.
pub fn priority(p: &Priority) -> &'static str {
    const GLYPHS: [&str; Priority::LEVELS] = ["⇈", "↑", "=", "↓", "⇊"];
    GLYPHS[p.level()]
}

pub fn kind_span(issue: &Issue, theme: &Theme) -> Span<'static> {
    Span::styled(kind(issue.kind.kind), Style::new().fg(theme.issue_type(issue.kind.kind)))
}

pub fn priority_span(p: &Priority, theme: &Theme) -> Span<'static> {
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
/// one bold in the accent, the others faint, a bar between them. Also
/// the cells the active one takes, for the rule under it to light.
pub fn tabs<S: AsRef<str>>(
    names: &[S],
    active: usize,
    theme: &Theme,
) -> (Vec<Span<'static>>, Range<usize>) {
    let mut spans: Vec<Span<'static>> = vec![];
    let mut lit = 0..0;
    for (i, name) in names.iter().enumerate() {
        if i > 0 {
            spans.push(Span::styled("│", Style::new().fg(theme.border)));
        }
        let style = if i == active {
            Style::new().fg(theme.accent).add_modifier(Modifier::BOLD)
        } else {
            Style::new().fg(theme.muted)
        };
        let start: usize = spans.iter().map(|s| s.width()).sum();
        let tab = Span::styled(format!("  {}  ", name.as_ref()), style);
        if i == active {
            lit = start..start + tab.width();
        }
        spans.push(tab);
    }
    (spans, lit)
}

/// A rule of `ch` across `width` cells, faint but for the `lit` ones, in
/// the accent: the active tab above it reads as raised from the rule.
pub fn rule(ch: &str, width: usize, lit: Range<usize>, theme: &Theme) -> Line<'static> {
    let lit = lit.start.min(width)..lit.end.min(width);
    Line::from(vec![
        Span::styled(ch.repeat(lit.start), Style::new().fg(theme.border)),
        Span::styled(ch.repeat(lit.len()), Style::new().fg(theme.border_focus)),
        Span::styled(ch.repeat(width - lit.end), Style::new().fg(theme.border)),
    ])
}

/// Every overlay's frame: rounded, its title in the accent and bold, on
/// no background. Unfocused, the border stays faint.
pub fn panel(title: &str, focused: bool, theme: &Theme) -> Block<'static> {
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(if focused { theme.border_focus } else { theme.border }))
        .style(Style::new().bg(theme.bg).fg(theme.fg));
    if title.is_empty() {
        return block;
    }
    block.title(Span::styled(
        format!(" {title} "),
        Style::new().fg(theme.accent).add_modifier(Modifier::BOLD),
    ))
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
    fn the_rule_lights_under_the_active_tab_only() {
        let theme = Theme::builtin("dark").unwrap();
        let (spans, lit) = tabs(&["No lanes", "Epic"], 1, &theme);
        let row: String = spans.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(row, "  No lanes  │  Epic  ");
        assert_eq!(lit, 13..21, "the cells of \"  Epic  \"");
        let rule = rule("━", 30, lit, &theme);
        let lit_cells: Vec<usize> = rule
            .spans
            .iter()
            .filter(|s| s.style.fg == Some(theme.border_focus))
            .map(|s| s.content.chars().count())
            .collect();
        assert_eq!(lit_cells, [8]);
        assert_eq!(rule.width(), 30);
    }

    #[test]
    fn badges_keep_their_text_colour_over_a_transparent_background() {
        let mut theme = Theme::builtin("catppuccin").unwrap();
        let overrides = toml::from_str("bg = \"reset\"\nbadge_fg = \"#010203\"").unwrap();
        assert!(theme.apply(&overrides, "test").is_empty());
        assert_eq!(label_badge("LISA", &theme)[1].style.fg, Some(Color::Rgb(1, 2, 3)));
    }
}
