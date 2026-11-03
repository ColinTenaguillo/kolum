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

/// The board's issues, matched locally; or, in remote mode, whatever the
/// provider's search last answered, in its order.
pub struct IssueSource {
    pub issues: Vec<Issue>,
    pub me: Option<UserId>,
    pub remote: Option<Vec<Issue>>,
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
        if let Some(found) = &self.remote {
            return found.clone();
        }
        let filter = filter_of(query);
        let me = self.me.clone().unwrap_or_else(|| UserId::new(""));
        self.issues.iter().filter(|i| filter.matches_fields(i, &me)).cloned().collect()
    }

    fn scope(&self, query: &str) -> String {
        match self.remote {
            Some(_) => "remote".into(),
            None => format!("{:?}", Filter { text: None, ..filter_of(query) }),
        }
    }

    /// Remote results come ranked by the provider and already match.
    fn pattern(&self, query: &str) -> String {
        match self.remote {
            Some(_) => String::new(),
            None => filter_of(query).text.unwrap_or_default(),
        }
    }

    fn toggle_remote(&mut self) -> Option<bool> {
        self.remote = match self.remote {
            Some(_) => None,
            None => Some(vec![]),
        };
        Some(self.remote.is_some())
    }

    fn receive(&mut self, issues: Vec<Issue>) {
        if self.remote.is_some() {
            self.remote = Some(issues);
        }
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
            Action::Transition | Action::Assign => {
                Pick::OnIssues(action, issues.iter().map(|i| i.key.clone()).collect())
            }
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

/// The provider's boards.
pub struct BoardChoice {
    pub boards: Vec<kolum_domain::BoardSummary>,
}

impl PickerSource for BoardChoice {
    type Item = kolum_domain::BoardSummary;

    fn title(&self) -> String {
        "Boards".into()
    }

    fn candidates(&self, _query: &str) -> Vec<Self::Item> {
        self.boards.clone()
    }

    fn haystack(&self, b: &Self::Item) -> String {
        let project = b.project.as_ref().map(|p| format!("  {}", p.key)).unwrap_or_default();
        format!("{}{project}  {:?} #{}", b.name, b.kind, b.id)
    }

    fn act(&self, boards: &[Self::Item], action: Action) -> Option<Pick> {
        (action == Action::Confirm).then(|| Pick::Board(boards[0].id.clone()))
    }
}

/// The transitions open to the issues in `keys`.
pub struct TransitionChoice {
    pub keys: Vec<kolum_domain::IssueKey>,
    pub transitions: Vec<kolum_domain::Transition>,
}

impl PickerSource for TransitionChoice {
    type Item = kolum_domain::Transition;

    fn title(&self) -> String {
        let keys: Vec<&str> = self.keys.iter().map(|k| k.as_str()).collect();
        format!("Move {}", keys.join(", "))
    }

    fn candidates(&self, _query: &str) -> Vec<Self::Item> {
        self.transitions.clone()
    }

    fn haystack(&self, t: &Self::Item) -> String {
        if t.name == t.to.name { t.name.clone() } else { format!("{} → {}", t.name, t.to.name) }
    }

    fn act(&self, ts: &[Self::Item], action: Action) -> Option<Pick> {
        (action == Action::Confirm)
            .then(|| Pick::Transition { keys: self.keys.clone(), transition: ts[0].clone() })
    }
}

/// Who to assign the issues in `keys` to: `None` unassigns.
pub struct UserChoice {
    pub keys: Vec<kolum_domain::IssueKey>,
    pub users: Vec<Option<kolum_domain::User>>,
}

impl PickerSource for UserChoice {
    type Item = Option<kolum_domain::User>;

    fn title(&self) -> String {
        let keys: Vec<&str> = self.keys.iter().map(|k| k.as_str()).collect();
        format!("Assign {}", keys.join(", "))
    }

    fn candidates(&self, _query: &str) -> Vec<Self::Item> {
        self.users.clone()
    }

    fn haystack(&self, user: &Self::Item) -> String {
        user.as_ref()
            .map_or("Unassigned".into(), |u| format!("{} ({})", u.display_name, u.initials()))
    }

    fn act(&self, users: &[Self::Item], action: Action) -> Option<Pick> {
        (action == Action::Confirm)
            .then(|| Pick::Assign { keys: self.keys.clone(), user: users[0].clone() })
    }
}
