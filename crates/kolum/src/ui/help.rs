//! `?`: the keys, generated from the live key table, so the page is right
//! after any rebinding.

use crate::keymap::{Context, Keymap, display_keys};
use crate::theme::Theme;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

/// Contexts in the order a newcomer needs them.
const ORDER: [Context; 7] = [
    Context::Global,
    Context::Board,
    Context::Detail,
    Context::Picker,
    Context::PickerInsert,
    Context::Command,
    Context::Help,
];

pub fn lines(keymap: &Keymap, theme: &Theme) -> Vec<Line<'static>> {
    let muted = Style::new().fg(theme.muted);
    let mut out = vec![Line::from(vec![
        Span::styled("leader ", muted),
        Span::styled(display_keys(&[keymap.leader]), Style::new().fg(theme.accent)),
        Span::styled(format!("   timeoutlen {} ms", keymap.timeout.as_millis()), muted),
    ])];
    for context in ORDER {
        let rows = keymap.help_rows(context);
        if rows.is_empty() {
            continue;
        }
        out.push(Line::default());
        out.push(Line::from(vec![
            Span::styled(
                context.title(),
                Style::new().fg(theme.accent).add_modifier(Modifier::BOLD),
            ),
            Span::styled(format!("  [keys.{}]", context.name()), muted),
        ]));
        for (keys, action) in rows {
            out.push(Line::from(vec![
                Span::styled(format!("  {keys:<18}"), Style::new().fg(theme.match_highlight)),
                Span::raw(format!("{:<44}", action.description())),
                Span::styled(action.name(), muted),
            ]));
        }
    }
    out
}
