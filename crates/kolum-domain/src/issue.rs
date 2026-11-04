use crate::ids::{FieldId, IssueId, IssueKey, StatusId, TransitionId, UserId};
use crate::rich_text::RichText;
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct User {
    pub id: UserId,
    pub display_name: String,
    pub avatar_url: Option<String>,
}

impl User {
    /// "Colin Tenaguillo" -> "CT", "colin" -> "CO". Two letters keep every
    /// card's meta line the same width.
    pub fn initials(&self) -> String {
        let mut words = self.display_name.split_whitespace();
        let first = words.next().unwrap_or("");
        let initials: String = match words.last() {
            Some(last) => first.chars().take(1).chain(last.chars().take(1)).collect(),
            None => first.chars().take(2).collect(),
        };
        initials.to_uppercase()
    }
}

/// Where a status sits in the flow. Providers name statuses freely; this is
/// what the theme colours by, so "QA" and "Review" both read as in progress.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum StatusCategory {
    Todo,
    Doing,
    Done,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Status {
    pub id: StatusId,
    pub name: String,
    pub category: StatusCategory,
}

/// The kind an issue type falls into, for its icon and colour. The name is
/// the provider's own ("Defect", "Spike").
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum IssueKind {
    Bug,
    Story,
    Task,
    Epic,
    Subtask,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IssueType {
    pub name: String,
    pub kind: IssueKind,
}

/// Highest first, so `Ord` sorts the urgent ones to the top.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum Priority {
    Highest,
    High,
    Medium,
    Low,
    Lowest,
}

impl Priority {
    pub const ALL: [Priority; 5] =
        [Priority::Highest, Priority::High, Priority::Medium, Priority::Low, Priority::Lowest];

    pub fn from_name(name: &str) -> Option<Priority> {
        Priority::ALL.into_iter().find(|p| p.name().eq_ignore_ascii_case(name))
    }

    pub fn name(self) -> &'static str {
        match self {
            Priority::Highest => "Highest",
            Priority::High => "High",
            Priority::Medium => "Medium",
            Priority::Low => "Low",
            Priority::Lowest => "Lowest",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Estimate {
    Points(f64),
    Seconds(u64),
}

/// A time box: a Jira sprint, a Linear cycle, a GitHub iteration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Sprint {
    pub id: String,
    pub name: String,
}

/// Another issue, as much of it as a line in a list needs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IssueRef {
    pub key: IssueKey,
    pub title: String,
    pub kind: IssueKind,
    pub status: Option<Status>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IssueLink {
    /// As the provider phrases it from this issue's side: "blocks",
    /// "is blocked by", "relates to".
    pub relation: String,
    pub issue: IssueRef,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum FieldValue {
    Text(String),
    Number(f64),
    Date(time::Date),
    User(User),
    Options(Vec<String>),
    RichText(RichText),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CustomField {
    pub id: FieldId,
    pub name: String,
    pub value: FieldValue,
}

/// The aggregate. Everything a card, a detail view or a search hit shows.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Issue {
    pub id: IssueId,
    pub key: IssueKey,
    pub title: String,
    pub kind: IssueType,
    pub status: Status,
    pub priority: Option<Priority>,
    pub assignee: Option<User>,
    pub reporter: Option<User>,
    pub labels: Vec<String>,
    /// The epic or the parent issue.
    pub parent: Option<IssueRef>,
    pub subtasks: Vec<IssueRef>,
    pub links: Vec<IssueLink>,
    pub estimate: Option<Estimate>,
    pub sprint: Option<Sprint>,
    #[serde(with = "time::serde::rfc3339")]
    pub created: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub updated: OffsetDateTime,
    pub due: Option<time::Date>,
    pub description: RichText,
    pub fields: Vec<CustomField>,
    /// Where the issue lives in the provider's web app.
    pub url: Option<String>,
    pub attachments: Vec<Attachment>,
}

/// A file on an issue. Its bytes come through `UserDirectory::image`, with
/// the provider's credentials, when it is a picture worth showing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Attachment {
    pub name: String,
    pub url: String,
    pub mime: String,
    pub size: u64,
}

impl Attachment {
    pub fn is_image(&self) -> bool {
        self.mime.starts_with("image/") && self.mime != "image/svg+xml"
    }
}

impl Issue {
    pub fn is_assigned_to(&self, user: &UserId) -> bool {
        self.assignee.as_ref().is_some_and(|a| &a.id == user)
    }

    pub fn as_ref(&self) -> IssueRef {
        IssueRef {
            key: self.key.clone(),
            title: self.title.clone(),
            kind: self.kind.kind,
            status: Some(self.status.clone()),
        }
    }
}

/// A move from the current status to another. Some workflows want fields
/// filled in on the way ("Resolution" on the way to Done).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Transition {
    pub id: TransitionId,
    pub name: String,
    pub to: Status,
    pub required_fields: Vec<FieldId>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Comment {
    pub id: String,
    pub author: User,
    pub body: RichText,
    #[serde(with = "time::serde::rfc3339")]
    pub created: OffsetDateTime,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FieldChange {
    pub field: String,
    pub from: Option<String>,
    pub to: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ActivityEntry {
    pub author: User,
    #[serde(with = "time::serde::rfc3339")]
    pub at: OffsetDateTime,
    pub changes: Vec<FieldChange>,
}

/// Which fields the provider's own issue view shows, in its order: the
/// same for every issue of a type in a project.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FieldLayout {
    /// Beside the issue: what the user pinned, then the details.
    pub side: Vec<LayoutField>,
    /// Under the title: the rich text fields, the description among them.
    pub body: Vec<FieldId>,
}

/// A field beside the issue, with the name the provider's view gives it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LayoutField {
    pub id: FieldId,
    pub name: String,
    /// Shown only once the issue holds something in it.
    pub only_filled: bool,
}

/// A field kolum may set on an issue, as the provider describes it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EditableField {
    pub id: FieldId,
    pub name: String,
    pub kind: FieldKind,
    /// It cannot be emptied.
    pub required: bool,
}

/// How a field is edited, and so what value it takes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum FieldKind {
    /// Markdown, in `$EDITOR`.
    RichText,
    /// One line: a summary, a URL.
    Text,
    Number,
    Date,
    User,
    /// Free words, like labels.
    Words,
    /// Another issue, by key: a parent, an epic.
    Issue,
    /// One of `options`, or several of them when `many`.
    Choice {
        options: Vec<String>,
        many: bool,
    },
    /// One of the board's sprints.
    Sprint,
}

impl FieldKind {
    /// What a line typed for a field of this kind sets it to. Empty
    /// empties the field.
    pub fn parse(&self, text: &str) -> std::result::Result<Option<FieldValue>, String> {
        let text = text.trim();
        if text.is_empty() {
            return Ok(None);
        }
        Ok(Some(match self {
            FieldKind::Number => {
                FieldValue::Number(text.parse().map_err(|_| format!("{text} is not a number"))?)
            }
            FieldKind::Date => {
                let format = time::macros::format_description!("[year]-[month]-[day]");
                let date = time::Date::parse(text, &format);
                FieldValue::Date(date.map_err(|_| "a date reads YYYY-MM-DD".to_string())?)
            }
            FieldKind::Words => {
                FieldValue::Options(text.split_whitespace().map(String::from).collect())
            }
            FieldKind::Issue => FieldValue::Text(text.to_uppercase()),
            _ => FieldValue::Text(text.to_string()),
        }))
    }
}

impl FieldValue {
    /// The value as a line to edit, which [`FieldKind::parse`] reads back.
    pub fn to_input(&self) -> String {
        match self {
            FieldValue::Text(t) => t.clone(),
            FieldValue::Number(n) => n.to_string(),
            FieldValue::Date(d) => d.to_string(),
            FieldValue::User(u) => u.display_name.clone(),
            FieldValue::Options(o) => o.join(" "),
            FieldValue::RichText(r) => r.plain_text(),
        }
    }
}

/// The fields an edit changes. `None` leaves the field as it is.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct IssueUpdate {
    pub title: Option<String>,
    pub description: Option<RichText>,
    pub labels: Option<Vec<String>>,
    pub priority: Option<Priority>,
    /// Any other field to set, by id; `None` empties it.
    pub fields: Vec<(FieldId, Option<FieldValue>)>,
}

impl IssueUpdate {
    /// What the edit does to the issue, as the provider will once it has
    /// it. Only fields the issue has are set: they are the ones offered.
    pub fn apply_to(&self, issue: &mut Issue) {
        if let Some(t) = &self.title {
            issue.title = t.clone();
        }
        if let Some(d) = &self.description {
            issue.description = d.clone();
        }
        if let Some(l) = &self.labels {
            issue.labels = l.clone();
        }
        if self.priority.is_some() {
            issue.priority = self.priority;
        }
        for (id, value) in &self.fields {
            match value {
                Some(value) => {
                    if let Some(field) = issue.fields.iter_mut().find(|f| f.id == *id) {
                        field.value = value.clone();
                    }
                }
                None => issue.fields.retain(|f| f.id != *id),
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NewIssue {
    pub project: String,
    pub kind: IssueType,
    pub title: String,
    pub description: RichText,
    pub parent: Option<IssueKey>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_typed_line_becomes_the_value_its_field_takes() {
        let words = |w: &[&str]| FieldValue::Options(w.iter().map(|s| s.to_string()).collect());
        assert_eq!(FieldKind::Number.parse(" 3.5 "), Ok(Some(FieldValue::Number(3.5))));
        assert_eq!(FieldKind::Number.parse("lots"), Err("lots is not a number".into()));
        let date = time::macros::date!(2026 - 11 - 30);
        assert_eq!(FieldKind::Date.parse("2026-11-30"), Ok(Some(FieldValue::Date(date))));
        assert_eq!(FieldKind::Date.parse("soon"), Err("a date reads YYYY-MM-DD".into()));
        assert_eq!(FieldKind::Words.parse("api  ui"), Ok(Some(words(&["api", "ui"]))));
        assert_eq!(FieldKind::Issue.parse("ic-12"), Ok(Some(FieldValue::Text("IC-12".into()))));
        assert_eq!(FieldKind::Text.parse(" a b "), Ok(Some(FieldValue::Text("a b".into()))));
        assert_eq!(FieldKind::Text.parse("  "), Ok(None), "empty empties the field");
        for value in [FieldValue::Number(5.0), FieldValue::Date(date), words(&["api", "ui"])] {
            let kind = match value {
                FieldValue::Number(_) => FieldKind::Number,
                FieldValue::Date(_) => FieldKind::Date,
                _ => FieldKind::Words,
            };
            assert_eq!(kind.parse(&value.to_input()), Ok(Some(value.clone())), "{value:?}");
        }
        assert_eq!(FieldValue::Number(5.0).to_input(), "5");
    }

    #[test]
    fn a_priority_is_found_by_its_name() {
        assert_eq!(Priority::from_name("high"), Some(Priority::High));
        assert_eq!(Priority::from_name("Urgent"), None);
    }

    fn user(name: &str) -> User {
        User { id: UserId::new("u"), display_name: name.into(), avatar_url: None }
    }

    #[test]
    fn priorities_sort_urgent_first_and_have_names() {
        let mut shuffled =
            [Priority::Low, Priority::Highest, Priority::Lowest, Priority::Medium, Priority::High];
        shuffled.sort();
        assert_eq!(shuffled, Priority::ALL);
        let names: Vec<_> = Priority::ALL.iter().map(|p| p.name()).collect();
        assert_eq!(names, ["Highest", "High", "Medium", "Low", "Lowest"]);
    }

    #[test]
    fn initials_take_the_first_and_last_word() {
        assert_eq!(user("Colin Tenaguillo").initials(), "CT");
        assert_eq!(user("Ana María López").initials(), "AL");
        assert_eq!(user("colin").initials(), "CO");
        assert_eq!(user("").initials(), "");
    }
}
