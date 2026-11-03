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

/// The fields an edit changes. `None` leaves the field as it is.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct IssueUpdate {
    pub title: Option<String>,
    pub description: Option<RichText>,
    pub labels: Option<Vec<String>>,
    pub priority: Option<Priority>,
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

    fn user(name: &str) -> User {
        User { id: UserId::new("u"), display_name: name.into(), avatar_url: None }
    }

    #[test]
    fn initials_take_the_first_and_last_word() {
        assert_eq!(user("Colin Tenaguillo").initials(), "CT");
        assert_eq!(user("Ana María López").initials(), "AL");
        assert_eq!(user("colin").initials(), "CO");
        assert_eq!(user("").initials(), "");
    }
}
