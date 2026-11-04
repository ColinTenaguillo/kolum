//! `?`: the keys, generated from the live key table, so the page is right
//! after any rebinding.

use crate::keymap::{Context, Keymap, display_keys};
use crate::theme::Theme;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

/// Contexts in the order a newcomer needs them.
const ORDER: [Context; 9] = [
    Context::Global,
    Context::Board,
    Context::Visual,
    Context::Detail,
    Context::Viewer,
    Context::Picker,
    Context::PickerInsert,
    Context::Command,
    Context::Help,
];

/// With `filter`, only the keys whose line holds every word of it, in any
/// case, and the sections that keep some.
pub fn lines(keymap: &Keymap, theme: &Theme, filter: &str) -> Vec<Line<'static>> {
    let words: Vec<String> = filter.to_lowercase().split_whitespace().map(String::from).collect();
    let muted = Style::new().fg(theme.muted);
    let mut out = vec![Line::from(vec![
        Span::styled("leader ", muted),
        Span::styled(display_keys(&[keymap.leader]), Style::new().fg(theme.accent)),
        Span::styled(format!("   timeoutlen {} ms", keymap.timeout.as_millis()), muted),
    ])];
    for context in ORDER {
        let mut rows = keymap.help_rows(context);
        rows.retain(|(keys, action)| {
            let line = format!("{keys} {} {}", action.description(), action.name()).to_lowercase();
            words.iter().all(|w| line.contains(w.as_str()))
        });
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
                // Always a space before the name, however long the description.
                Span::raw(format!("{:<43} ", action.description())),
                Span::styled(action.name(), muted),
            ]));
        }
    }
    out
}
