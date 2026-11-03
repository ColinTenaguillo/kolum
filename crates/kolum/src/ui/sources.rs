//! The pickers the app opens. Each is a few lines over [`PickerSource`].

use super::glyph;
use super::picker::{Pick, PickerSource};
use super::rich;
use crate::keymap::{Action, Keymap};
use crate::theme::Theme;
use kolum_app::parse_query;
use kolum_domain::{Filter, Issue, Query, UserId};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span, Text};

/// The board's issues, matched locally.
pub struct IssueSource {
    pub issues: Vec<Issue>,
    pub me: Option<UserId>,
}

fn filter_of(query: &str) -> Filter {
    match parse_query(query) {
        Query::Filter(f) => f,
        // A native query means nothing locally: everything stays a
        // candidate, and the text after `!` is matched as typed.
        Query::Native(text) => Filter { text: Some(text), ..Filter::default() },
    }
}

impl PickerSource for IssueSource {
    type Item = Issue;

    fn title(&self) -> String {
        "Issues".into()
    }

    fn candidates(&self, query: &str) -> Vec<Issue> {
        let filter = filter_of(query);
        let me = self.me.clone().unwrap_or_else(|| UserId::new(""));
        self.issues.iter().filter(|i| filter.matches_fields(i, &me)).cloned().collect()
    }

    fn scope(&self, query: &str) -> String {
        format!("{:?}", Filter { text: None, ..filter_of(query) })
    }

    fn pattern(&self, query: &str) -> String {
        filter_of(query).text.unwrap_or_default()
    }

    fn haystack(&self, issue: &Issue) -> String {
        format!("{} {}", issue.key, issue.title)
    }

    fn preview(&self, issue: &Issue, theme: &Theme, cols: u16) -> Option<Text<'static>> {
        let muted = Style::new().fg(theme.muted);
        let mut head = vec![
            Span::styled(issue.key.to_string(), Style::new().add_modifier(Modifier::BOLD)),
            Span::raw("  "),
            glyph::kind_span(issue, theme),
            Span::raw(format!(" {}", issue.kind.name)),
        ];
        if let Some(p) = issue.priority {
            head.extend([
                Span::raw("  "),
                glyph::priority_span(p, theme),
                Span::raw(format!(" {}", p.name())),
            ]);
        }
        let assignee =
            issue.assignee.as_ref().map_or("Unassigned".into(), |u| u.display_name.clone());
        let mut lines = vec![
            Line::from(head),
            Line::from(Span::styled(
                issue.title.clone(),
                Style::new().add_modifier(Modifier::BOLD),
            )),
            Line::from(vec![Span::styled("Status    ", muted), glyph::status_badge(issue, theme)]),
            Line::from(vec![Span::styled("Assignee  ", muted), Span::raw(assignee)]),
            Line::from(Span::styled("─".repeat(cols as usize), Style::new().fg(theme.border))),
        ];
        lines.extend(rich::lines(&issue.description, theme, cols));
        Some(Text::from(lines))
    }

    fn act(&self, issues: &[Issue], action: Action) -> Option<Pick> {
        let first = issues.first()?;
        Some(match action {
            Action::Confirm => Pick::OpenIssue(first.key.clone()),
            Action::OpenBrowser => Pick::OpenUrl(first.url.clone()?),
            Action::CopyKey => {
                Pick::Copy(issues.iter().map(|i| i.key.to_string()).collect::<Vec<_>>().join(" "))
            }
            Action::Transition | Action::Assign => Pick::Unavailable(action),
            _ => return None,
        })
    }
}

/// Every action, with the keys it has now.
pub struct CommandSource {
    pub rows: Vec<(Action, Option<String>)>,
}

impl CommandSource {
    pub fn new(keymap: &Keymap) -> Self {
        CommandSource { rows: Action::ALL.iter().map(|&a| (a, keymap.keys_for(a))).collect() }
    }
}

impl PickerSource for CommandSource {
    type Item = (Action, Option<String>);

    fn title(&self) -> String {
        "Commands".into()
    }

    fn candidates(&self, _query: &str) -> Vec<Self::Item> {
        self.rows.clone()
    }

    fn haystack(&self, (action, keys): &Self::Item) -> String {
        let keys = keys.as_deref().unwrap_or("");
        format!("{:<40} {:<22} {keys}", action.description(), action.name())
    }

    fn act(&self, items: &[Self::Item], action: Action) -> Option<Pick> {
        (action == Action::Confirm).then(|| Pick::Run(items[0].0))
    }
}

/// Theme names; highlighting one shows it at once.
pub struct ThemeSource {
    pub names: Vec<String>,
}

impl PickerSource for ThemeSource {
    type Item = String;

    fn title(&self) -> String {
        "Themes".into()
    }

    fn candidates(&self, _query: &str) -> Vec<String> {
        self.names.clone()
    }

    fn haystack(&self, name: &String) -> String {
        name.clone()
    }

    fn act(&self, names: &[String], action: Action) -> Option<Pick> {
        (action == Action::Confirm).then(|| Pick::Theme(names[0].clone()))
    }

    fn hover(&self, name: &String) -> Option<Pick> {
        Some(Pick::Theme(name.clone()))
    }
}
