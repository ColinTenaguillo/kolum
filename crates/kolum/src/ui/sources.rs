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
        // The issue view's header, small: the key and the title on a band
        // of the selection, then the status pill and who has it.
        let muted = Style::new().fg(theme.muted);
        let band = |mut spans: Vec<Span<'static>>| {
            let used: usize = spans.iter().map(|s| super::text::width(&s.content)).sum();
            spans.push(Span::raw(" ".repeat((cols as usize).saturating_sub(used))));
            Line::from(spans).style(Style::new().bg(theme.selection))
        };
        let mut head = vec![
            Span::styled(format!(" {}", issue.key), muted),
            Span::styled(" · ", muted),
            glyph::kind_span(issue, theme),
            Span::styled(format!(" {}", issue.kind.name), muted),
        ];
        if let Some(p) = issue.priority {
            head.extend([
                Span::styled(" · ", muted),
                glyph::priority_span(p, theme),
                Span::styled(format!(" {}", p.name()), muted),
            ]);
        }
        let title = super::text::truncate(&issue.title, (cols as usize).saturating_sub(2));
        let assignee =
            issue.assignee.as_ref().map_or("Unassigned".into(), |u| u.display_name.clone());
        let mut status = vec![Span::raw(" ")];
        status.extend(glyph::status_badge(issue, theme));
        status.extend([
            Span::styled(" ⋅ ", muted),
            Span::styled(assignee, Style::new().add_modifier(Modifier::BOLD)),
        ]);
        let mut lines = vec![
            band(head),
            band(vec![
                Span::raw(" "),
                Span::styled(title, Style::new().fg(theme.fg).add_modifier(Modifier::BOLD)),
            ]),
            Line::default(),
            Line::from(status),
            Line::default(),
        ];
        lines.extend(rich::lines(issue.description_or_empty(), theme, cols));
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
        titled("Move", &self.keys)
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
    /// A user field to set instead of the assignee.
    pub field: Option<kolum_domain::EditableField>,
}

impl PickerSource for UserChoice {
    type Item = Option<kolum_domain::User>;

    fn title(&self) -> String {
        match &self.field {
            Some(field) => format!("{} of {}", field.name, self.keys[0]),
            None => titled("Assign", &self.keys),
        }
    }

    fn candidates(&self, _query: &str) -> Vec<Self::Item> {
        self.users.clone()
    }

    fn haystack(&self, user: &Self::Item) -> String {
        user.as_ref()
            .map_or("Unassigned".into(), |u| format!("{} ({})", u.display_name, u.initials()))
    }

    fn act(&self, users: &[Self::Item], action: Action) -> Option<Pick> {
        if action != Action::Confirm {
            return None;
        }
        let user = users[0].clone();
        Some(match &self.field {
            Some(field) => Pick::SetField {
                key: self.keys[0].clone(),
                field: field.clone(),
                value: user.map(kolum_domain::FieldValue::User),
            },
            None => Pick::Assign { keys: self.keys.clone(), user },
        })
    }
}

/// The fields of an issue, each with what it holds now: the one picked
/// opens the editor its kind calls for.
pub struct FieldChoice {
    pub key: kolum_domain::IssueKey,
    pub fields: Vec<(super::FieldEntry, String, kolum_domain::RichText)>,
}

impl PickerSource for FieldChoice {
    type Item = (super::FieldEntry, String, kolum_domain::RichText);

    fn title(&self) -> String {
        titled("Edit", std::slice::from_ref(&self.key))
    }

    fn candidates(&self, _query: &str) -> Vec<Self::Item> {
        self.fields.clone()
    }

    fn haystack(&self, (_, name, _): &Self::Item) -> String {
        name.clone()
    }

    fn preview(
        &self,
        (_, _, text): &Self::Item,
        theme: &Theme,
        cols: u16,
    ) -> Option<Text<'static>> {
        Some(Text::from(rich::lines(text, theme, cols)))
    }

    fn act(&self, fields: &[Self::Item], action: Action) -> Option<Pick> {
        (action == Action::Confirm)
            .then(|| Pick::EditField { key: self.key.clone(), entry: fields[0].0.clone() })
    }
}

/// The board's sprints for a sprint field; `None` takes the issue out.
pub struct SprintChoice {
    pub key: kolum_domain::IssueKey,
    pub field: kolum_domain::EditableField,
    pub sprints: Vec<Option<kolum_domain::Sprint>>,
}

impl PickerSource for SprintChoice {
    type Item = Option<kolum_domain::Sprint>;

    fn title(&self) -> String {
        format!("{} of {}", self.field.name, self.key)
    }

    fn candidates(&self, _query: &str) -> Vec<Self::Item> {
        self.sprints.clone()
    }

    fn haystack(&self, sprint: &Self::Item) -> String {
        sprint.as_ref().map_or("None".into(), |s| s.name.clone())
    }

    fn act(&self, sprints: &[Self::Item], action: Action) -> Option<Pick> {
        (action == Action::Confirm).then(|| Pick::SetField {
            key: self.key.clone(),
            field: self.field.clone(),
            value: sprints[0].clone().map(kolum_domain::FieldValue::Sprint),
        })
    }
}

/// The values a choice field allows, and `None` to empty it when it may
/// be empty. Tab marks several for a field that takes many.
pub struct OptionChoice {
    pub key: kolum_domain::IssueKey,
    pub field: kolum_domain::EditableField,
    pub options: Vec<Option<String>>,
}

impl PickerSource for OptionChoice {
    type Item = Option<String>;

    fn title(&self) -> String {
        format!("{} of {}", self.field.name, self.key)
    }

    fn candidates(&self, _query: &str) -> Vec<Self::Item> {
        self.options.clone()
    }

    fn haystack(&self, option: &Self::Item) -> String {
        option.clone().unwrap_or_else(|| "None".into())
    }

    fn act(&self, chosen: &[Self::Item], action: Action) -> Option<Pick> {
        use kolum_domain::{FieldKind, FieldValue};
        if action != Action::Confirm {
            return None;
        }
        let picked: Vec<String> = chosen.iter().flatten().cloned().collect();
        let many = matches!(self.field.kind, FieldKind::Choice { many: true, .. });
        let value = match picked.first() {
            None => None,
            Some(_) if many => Some(FieldValue::Options(picked)),
            Some(one) => Some(FieldValue::Text(one.clone())),
        };
        Some(Pick::SetField { key: self.key.clone(), field: self.field.clone(), value })
    }
}

/// "Move ACME-1, ACME-2": what a picker does, to which issues.
fn titled(verb: &str, keys: &[kolum_domain::IssueKey]) -> String {
    let keys: Vec<&str> = keys.iter().map(|k| k.as_str()).collect();
    format!("{verb} {}", keys.join(", "))
}

/// People to show the cards of: everyone, me, the board's assignees by
/// name, no one. Each row is an `@` token for the board filter and what
/// it shows.
pub struct PeopleChoice {
    pub people: Vec<(String, String)>,
}

impl PickerSource for PeopleChoice {
    type Item = (String, String);

    fn title(&self) -> String {
        "Show the cards of (Tab marks several)".into()
    }

    fn candidates(&self, _query: &str) -> Vec<Self::Item> {
        self.people.clone()
    }

    fn haystack(&self, (_, label): &Self::Item) -> String {
        label.clone()
    }

    fn act(&self, people: &[Self::Item], action: Action) -> Option<Pick> {
        (action == Action::Confirm)
            .then(|| Pick::People(people.iter().map(|(token, _)| token.clone()).collect()))
    }
}
