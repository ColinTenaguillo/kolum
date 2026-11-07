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

/// How urgent an issue is, by the provider's own name ("Blocker", "P1").
/// `rank` orders them, the most urgent at 0: the adapter derives it from
/// the provider's own order, so `Ord` sorts the urgent ones to the top.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Priority {
    pub rank: u32,
    pub name: String,
}

impl Priority {
    /// How many levels of urgency a colour or a glyph tells apart.
    pub const LEVELS: usize = 5;

    /// The level shown, 0 the most urgent: a rank past the last level
    /// shows as the last.
    pub fn level(&self) -> usize {
        (self.rank as usize).min(Self::LEVELS - 1)
    }

    /// The level a name in common use means, 0 the most urgent: P0 to P4,
    /// Jira's two schemes and the usual words. `None` for a project's own.
    pub fn known_level(name: &str) -> Option<u32> {
        let name = name.trim().to_lowercase();
        if let Some(n) = name.strip_prefix('p').and_then(|n| n.parse().ok()) {
            return Some(n);
        }
        Some(match name.as_str() {
            "highest" | "blocker" | "critical" | "urgent" => 0,
            "high" | "major" => 1,
            "medium" | "normal" => 2,
            "low" | "minor" => 3,
            "lowest" | "trivial" => 4,
            _ => return None,
        })
    }

    /// A priority by its name: a name in common use at the level it
    /// means, wherever it is listed; one of a project's own at its `place`
    /// (index, length) in the list that defines it, spread over the
    /// levels; with neither, the middle. The same name, read from the same
    /// list, ranks the same in the picker, on the card and read back.
    pub fn ranked(name: &str, place: Option<(usize, usize)>) -> Priority {
        let last = Self::LEVELS - 1;
        let spread = place.filter(|&(_, len)| len > 1).map(|(at, len)| {
            let steps = len - 1;
            ((at.min(steps) * last + steps / 2) / steps) as u32
        });
        let rank = Self::known_level(name).or(spread).unwrap_or(last as u32 / 2);
        Priority { rank, name: name.into() }
    }

    /// A provider's priorities in its order, each [`Priority::ranked`]:
    /// read from the end when the names it knows go from the least urgent
    /// up ([Low, Medium, High]).
    pub fn list(names: &[&str]) -> Vec<Priority> {
        let known: Vec<u32> = names.iter().filter_map(|n| Self::known_level(n)).collect();
        let upward = known.first() > known.last();
        let len = names.len();
        let place = |at: usize| if upward { len - 1 - at } else { at };
        (0..len).map(|at| Self::ranked(names[at], Some((place(at), len)))).collect()
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
    Sprint(Sprint),
    /// Another issue: a parent, an epic.
    Issue(IssueRef),
    Priority(Priority),
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
    /// `None` when this copy does not carry it: a Jira Cloud board's does
    /// not. An empty one is a description emptied.
    pub description: Option<RichText>,
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

/// What a copy that carries no description shows: none written.
static NO_DESCRIPTION: RichText = RichText { blocks: Vec::new(), lossy: false };

impl Issue {
    /// The description to show or edit: one not carried shows as none.
    pub fn description_or_empty(&self) -> &RichText {
        self.description.as_ref().unwrap_or(&NO_DESCRIPTION)
    }

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

    /// What a field holds now, by its id: a member for the ones
    /// [`FieldId`] names, a custom field otherwise. `None` when empty.
    pub fn value(&self, id: &FieldId) -> Option<FieldValue> {
        match member(id) {
            Some(member) => (member.read)(self),
            None => self.fields.iter().find(|f| &f.id == id).map(|f| f.value.clone()),
        }
    }

    /// A field set to `value`, or emptied, the way [`Issue::value`] reads
    /// it back. A custom field the issue has no value in yet takes `name`,
    /// and stays out when it has none: the issue does not hold it.
    fn set(&mut self, id: &FieldId, name: Option<&str>, value: Option<&FieldValue>) {
        let held = self.fields.iter_mut().find(|f| &f.id == id);
        match (member(id), value, held, name) {
            (Some(member), value, _, _) => (member.write)(self, value),
            (None, None, _, _) => self.fields.retain(|f| &f.id != id),
            (None, Some(value), Some(field), _) => field.value = value.clone(),
            (None, Some(value), None, Some(name)) => self.fields.push(CustomField {
                id: id.clone(),
                name: name.to_string(),
                value: value.clone(),
            }),
            (None, Some(_), None, None) => {}
        }
    }
}

/// A field the issue holds as a member: how it reads as a value; how a
/// value of its kind sets it and `None` empties it, a value of another
/// kind changing nothing; and the slot of [`IssueUpdate`] it goes in, for
/// the ones providers write by a slot.
struct Member {
    id: &'static str,
    label: &'static str,
    read: fn(&Issue) -> Option<FieldValue>,
    write: fn(&mut Issue, Option<&FieldValue>),
    slot: fn(&mut IssueUpdate, &FieldValue) -> bool,
}

/// No slot: the member goes by its id in [`IssueUpdate::fields`].
const BY_ID: fn(&mut IssueUpdate, &FieldValue) -> bool = |_, _| false;

/// Every field [`FieldId`] names, each read and written in one place, in
/// the order a detail lists them.
const MEMBERS: [Member; 10] = [
    Member {
        id: FieldId::TITLE,
        label: "Title",
        read: |i| Some(FieldValue::Text(i.title.clone())),
        // An issue always has one: it is never emptied.
        write: |i, v| {
            if let Some(FieldValue::Text(t)) = v {
                i.title = t.clone()
            }
        },
        slot: |u, v| match v {
            FieldValue::Text(t) => {
                u.title = Some(t.clone());
                true
            }
            _ => false,
        },
    },
    Member {
        id: FieldId::DESCRIPTION,
        label: "Description",
        read: |i| i.description.clone().map(FieldValue::RichText),
        write: |i, v| match v {
            None => i.description = Some(RichText::default()),
            Some(FieldValue::RichText(t)) => i.description = Some(t.clone()),
            Some(_) => {}
        },
        slot: |u, v| match v {
            FieldValue::RichText(t) => {
                u.description = Some(t.clone());
                true
            }
            _ => false,
        },
    },
    Member {
        id: FieldId::ASSIGNEE,
        label: "Assignee",
        read: |i| i.assignee.clone().map(FieldValue::User),
        write: |i, v| match v {
            None => i.assignee = None,
            Some(FieldValue::User(u)) => i.assignee = Some(u.clone()),
            Some(_) => {}
        },
        slot: BY_ID,
    },
    Member {
        id: FieldId::REPORTER,
        label: "Reporter",
        read: |i| i.reporter.clone().map(FieldValue::User),
        write: |i, v| match v {
            None => i.reporter = None,
            Some(FieldValue::User(u)) => i.reporter = Some(u.clone()),
            Some(_) => {}
        },
        slot: BY_ID,
    },
    Member {
        id: FieldId::PRIORITY,
        label: "Priority",
        read: |i| i.priority.clone().map(FieldValue::Priority),
        write: |i, v| match v {
            None => i.priority = None,
            Some(FieldValue::Priority(p)) => i.priority = Some(p.clone()),
            Some(_) => {}
        },
        slot: |u, v| match v {
            FieldValue::Priority(p) => {
                u.priority = Some(p.clone());
                true
            }
            _ => false,
        },
    },
    Member {
        id: FieldId::SPRINT,
        label: "Sprint",
        read: |i| i.sprint.clone().map(FieldValue::Sprint),
        write: |i, v| match v {
            None => i.sprint = None,
            Some(FieldValue::Sprint(s)) => i.sprint = Some(s.clone()),
            Some(_) => {}
        },
        slot: BY_ID,
    },
    Member {
        id: FieldId::EPIC,
        label: "Parent",
        read: |i| i.parent.clone().map(FieldValue::Issue),
        // A key alone does not say what the parent is: it waits for the
        // provider to read it back.
        write: |i, v| match v {
            None => i.parent = None,
            Some(FieldValue::Issue(parent)) => i.parent = Some(parent.clone()),
            Some(_) => {}
        },
        slot: BY_ID,
    },
    Member {
        id: FieldId::LABELS,
        label: "Labels",
        read: |i| Some(FieldValue::Options(i.labels.clone())),
        write: |i, v| match v {
            None => i.labels.clear(),
            Some(FieldValue::Options(o)) => i.labels = o.clone(),
            Some(_) => {}
        },
        slot: |u, v| match v {
            FieldValue::Options(o) => {
                u.labels = Some(o.clone());
                true
            }
            _ => false,
        },
    },
    Member {
        id: FieldId::ESTIMATE,
        label: "Estimate",
        // A time estimate is no points.
        read: |i| match i.estimate {
            Some(Estimate::Points(p)) => Some(FieldValue::Number(p)),
            _ => None,
        },
        write: |i, v| match v {
            None => i.estimate = None,
            Some(FieldValue::Number(n)) => i.estimate = Some(Estimate::Points(*n)),
            Some(_) => {}
        },
        slot: BY_ID,
    },
    Member {
        id: FieldId::DUE,
        label: "Due date",
        read: |i| i.due.map(FieldValue::Date),
        write: |i, v| match v {
            None => i.due = None,
            Some(FieldValue::Date(d)) => i.due = Some(*d),
            Some(_) => {}
        },
        slot: BY_ID,
    },
];

impl FieldId {
    /// The fields the issue holds as members, by id and label, in the
    /// order a detail lists them.
    pub fn members() -> impl Iterator<Item = (&'static str, &'static str)> {
        MEMBERS.iter().map(|m| (m.id, m.label))
    }
}

fn member(id: &FieldId) -> Option<&'static Member> {
    MEMBERS.iter().find(|m| m.id == id.as_str())
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
    /// kolum's id: a member's ([`FieldId::DUE`]) for a field the issue
    /// holds as a member.
    pub id: FieldId,
    /// The provider's own, handed back to it with the edit.
    pub provider_id: FieldId,
    pub name: String,
    pub kind: FieldKind,
    /// It cannot be emptied.
    pub required: bool,
    /// The provider reads it back into the issue, so an edit shows on the
    /// issue before that read.
    pub on_issue: bool,
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
    /// One of the provider's priorities, the most urgent first.
    Priority(Vec<Priority>),
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
            FieldValue::Sprint(s) => s.name.clone(),
            FieldValue::Issue(i) => i.key.to_string(),
            FieldValue::Priority(p) => p.name.clone(),
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
    /// Any other field to set, by id.
    pub fields: Vec<FieldEdit>,
}

/// One field set by id, as [`EditableField`] offered it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FieldEdit {
    pub id: FieldId,
    pub provider_id: FieldId,
    /// What the issue calls it; `None` when the issue does not hold it.
    pub name: Option<String>,
    /// `None` empties it.
    pub value: Option<FieldValue>,
}

impl IssueUpdate {
    /// The edit that sets one field, or empties it: in the slot its member
    /// goes in when it has one, by id otherwise.
    pub fn set(field: &EditableField, value: Option<FieldValue>) -> IssueUpdate {
        let mut update = IssueUpdate::default();
        let slotted = match (member(&field.id), &value) {
            (Some(member), Some(value)) => (member.slot)(&mut update, value),
            _ => false,
        };
        if !slotted {
            update.fields = vec![FieldEdit {
                id: field.id.clone(),
                provider_id: field.provider_id.clone(),
                name: field.on_issue.then(|| field.name.clone()),
                value,
            }];
        }
        update
    }

    /// What the edit does to the issue, as the provider will once it has
    /// it.
    pub fn apply_to(&self, issue: &mut Issue) {
        if let Some(t) = &self.title {
            issue.title = t.clone();
        }
        if let Some(d) = &self.description {
            issue.description = Some(d.clone());
        }
        if let Some(l) = &self.labels {
            issue.labels = l.clone();
        }
        if let Some(p) = &self.priority {
            issue.priority = Some(p.clone());
        }
        for edit in &self.fields {
            issue.set(&edit.id, edit.name.as_deref(), edit.value.as_ref());
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
        let others = [
            (FieldValue::User(user("Ana Silva")), "Ana Silva"),
            (FieldValue::RichText(RichText::from_markdown("**a** b")), "a b"),
            (sprint("15"), "Sprint 15"),
            (epic("ACME-1"), "ACME-1"),
        ];
        for (value, line) in others {
            assert_eq!(value.to_input(), line);
        }
    }

    fn id(id: &str) -> FieldId {
        FieldId::new(id)
    }

    /// A field as a provider offers it, named after its id.
    fn field(id: &str) -> EditableField {
        EditableField {
            id: FieldId::new(id),
            provider_id: FieldId::new(format!("{id}-p")),
            name: format!("{id} name"),
            kind: FieldKind::Text,
            required: false,
            on_issue: true,
        }
    }

    fn edit(issue: &mut Issue, field_id: &str, value: Option<FieldValue>) {
        IssueUpdate::set(&field(field_id), value).apply_to(issue)
    }

    fn sprint(id: &str) -> FieldValue {
        FieldValue::Sprint(Sprint { id: id.into(), name: format!("Sprint {id}") })
    }

    fn epic(key: &str) -> FieldValue {
        let key = IssueKey::parse(key);
        FieldValue::Issue(IssueRef {
            title: format!("{key} title"),
            key,
            kind: IssueKind::Epic,
            status: None,
        })
    }

    #[test]
    fn a_field_reads_from_its_member_or_its_custom_field() {
        let mut issue = crate::query::tests::issue();
        issue.estimate = Some(Estimate::Points(3.0));
        issue.fields.push(CustomField {
            id: id("customfield_1"),
            name: "Team".into(),
            value: FieldValue::Text("Mobile".into()),
        });
        let text = |t: &str| Some(FieldValue::Text(t.into()));
        assert_eq!(issue.value(&id(FieldId::TITLE)), text("Login times out"));
        let priority = issue.priority.clone().map(FieldValue::Priority);
        assert_eq!(issue.value(&id(FieldId::PRIORITY)), priority);
        let parent = issue.parent.clone().map(FieldValue::Issue);
        assert_eq!(issue.value(&id(FieldId::EPIC)), parent);
        let labels = Some(FieldValue::Options(issue.labels.clone()));
        assert_eq!(issue.value(&id(FieldId::LABELS)), labels);
        let ana = issue.assignee.clone().map(FieldValue::User);
        assert_eq!(issue.value(&id(FieldId::ASSIGNEE)), ana);
        assert_eq!(issue.value(&id(FieldId::ESTIMATE)), Some(FieldValue::Number(3.0)));
        let sprint = issue.sprint.clone().map(FieldValue::Sprint);
        assert_eq!(issue.value(&id(FieldId::SPRINT)), sprint);
        assert_eq!(issue.value(&id("customfield_1")), text("Mobile"));
        for empty in [FieldId::DUE, FieldId::REPORTER, "customfield_2"] {
            assert_eq!(issue.value(&id(empty)), None, "{empty}");
        }
        issue.estimate = Some(Estimate::Seconds(3600));
        assert_eq!(issue.value(&id(FieldId::ESTIMATE)), None, "a time estimate is no points");
    }

    /// Each field set then emptied, as the provider will once it has the
    /// edit: what the issue reads back is what was set.
    #[test]
    fn an_update_sets_and_empties_members_and_custom_fields_alike() {
        let date = FieldValue::Date(time::macros::date!(2026 - 12 - 01));
        let bob = FieldValue::User(user("Bob Stone"));
        let cases = [
            (FieldId::TITLE, FieldValue::Text("Renamed".into())),
            (FieldId::DESCRIPTION, FieldValue::RichText(RichText::from_markdown("New"))),
            (FieldId::LABELS, FieldValue::Options(vec!["ui".into()])),
            (FieldId::PRIORITY, FieldValue::Priority(priority(4, "Lowest"))),
            (FieldId::DUE, date.clone()),
            (FieldId::ASSIGNEE, bob.clone()),
            (FieldId::REPORTER, bob),
            (FieldId::EPIC, epic("PROJ-7")),
            (FieldId::ESTIMATE, FieldValue::Number(5.0)),
            (FieldId::SPRINT, sprint("15")),
            ("customfield_1", date),
        ];
        for (field, value) in cases {
            let mut issue = crate::query::tests::issue();
            edit(&mut issue, field, Some(value.clone()));
            assert_eq!(issue.value(&id(field)), Some(value), "{field} set");
            edit(&mut issue, field, None);
            let emptied = issue.value(&id(field));
            // A title is never emptied; an empty description or list is one.
            match field {
                FieldId::TITLE => assert_eq!(emptied, Some(FieldValue::Text("Renamed".into()))),
                FieldId::DESCRIPTION => {
                    assert_eq!(emptied, Some(FieldValue::RichText(RichText::default())))
                }
                FieldId::LABELS => assert_eq!(emptied, Some(FieldValue::Options(vec![]))),
                _ => assert_eq!(emptied, None, "{field} emptied"),
            }
        }
    }

    #[test]
    fn a_custom_field_an_edit_fills_first_goes_by_the_name_the_provider_gives() {
        let mut issue = crate::query::tests::issue();
        edit(&mut issue, "customfield_1", Some(FieldValue::Number(2.0)));
        assert_eq!(issue.fields[0].name, "customfield_1 name");
    }

    /// A field the provider never reads back into the issue is sent, and
    /// not shown: it would vanish on the next read as if undone.
    #[test]
    fn a_field_the_issue_does_not_hold_is_sent_not_shown() {
        let mut issue = crate::query::tests::issue();
        let environment = EditableField { on_issue: false, ..field("environment") };
        let update = IssueUpdate::set(&environment, Some(FieldValue::Text("prod".into())));
        assert_eq!(update.fields[0].provider_id.as_str(), "environment-p", "sent");
        update.apply_to(&mut issue);
        assert_eq!(issue.fields, []);
    }

    /// The members in the order a detail lists them, each with its label.
    #[test]
    fn the_members_have_their_labels() {
        let labels: Vec<(&str, &str)> = FieldId::members().collect();
        assert_eq!(labels.len(), MEMBERS.len());
        assert_eq!(
            labels[..3],
            [("title", "Title"), ("description", "Description"), ("assignee", "Assignee")]
        );
        assert!(labels.contains(&(FieldId::EPIC, "Parent")));
    }

    /// What a member cannot hold leaves it as it was: a value of another
    /// kind, a priority by its name alone, a parent known by its key
    /// alone, until the provider reads it back.
    #[test]
    fn a_value_a_member_cannot_hold_changes_nothing() {
        let date = FieldValue::Date(time::macros::date!(2026 - 12 - 01));
        let text = |t: &str| FieldValue::Text(t.into());
        let mut issue = crate::query::tests::issue();
        issue.due = Some(time::macros::date!(2026 - 11 - 30));
        let before = issue.clone();
        for field in [
            FieldId::TITLE,
            FieldId::DESCRIPTION,
            FieldId::LABELS,
            FieldId::PRIORITY,
            FieldId::ASSIGNEE,
            FieldId::REPORTER,
            FieldId::EPIC,
            FieldId::ESTIMATE,
            FieldId::SPRINT,
        ] {
            edit(&mut issue, field, Some(date.clone()));
        }
        edit(&mut issue, FieldId::DUE, Some(text("tomorrow")));
        edit(&mut issue, FieldId::PRIORITY, Some(text("Blocker")));
        edit(&mut issue, FieldId::EPIC, Some(text("PROJ-7")));
        assert_eq!(issue, before);
    }

    /// The members a provider writes by name take the value; the rest goes
    /// by id, a priority by its name alone among them.
    #[test]
    fn an_update_to_one_field_goes_through_the_member_the_provider_writes() {
        let text = |t: &str| Some(FieldValue::Text(t.into()));
        let set = |id: &str, value| IssueUpdate::set(&field(id), value);
        let by_id = |id: &str, value| {
            vec![FieldEdit {
                id: FieldId::new(id),
                provider_id: FieldId::new(format!("{id}-p")),
                name: Some(format!("{id} name")),
                value,
            }]
        };
        assert_eq!(set(FieldId::TITLE, text("T")).title.as_deref(), Some("T"));
        let rich = RichText::from_markdown("d");
        let described = set(FieldId::DESCRIPTION, Some(FieldValue::RichText(rich.clone())));
        assert_eq!(described.description, Some(rich));
        let labels = set(FieldId::LABELS, Some(FieldValue::Options(vec!["a".into()])));
        assert_eq!(labels.labels, Some(vec!["a".into()]));
        let high = priority(1, "High");
        let picked = set(FieldId::PRIORITY, Some(FieldValue::Priority(high.clone())));
        assert_eq!(picked.priority, Some(high));
        let blocker = set(FieldId::PRIORITY, text("Blocker"));
        assert_eq!(blocker.fields, by_id(FieldId::PRIORITY, text("Blocker")));
        assert_eq!(set(FieldId::TITLE, None).fields, by_id(FieldId::TITLE, None));
        assert_eq!(set(FieldId::DUE, text("x")).fields, by_id(FieldId::DUE, text("x")));
        assert_eq!(set("customfield_1", text("x")).fields, by_id("customfield_1", text("x")));
    }

    /// A priority picked among the provider's own (Jira's older scheme
    /// here) is on the issue at once, by that name, before any read-back.
    #[test]
    fn a_priority_picked_among_the_providers_own_shows_at_once() {
        let names = ["Blocker", "Critical", "Major", "Minor", "Trivial"];
        let all: Vec<Priority> =
            (0..).zip(names).map(|(rank, name)| priority(rank, name)).collect();
        let picked = Some(FieldValue::Priority(all[2].clone()));
        let field = EditableField { kind: FieldKind::Priority(all), ..field(FieldId::PRIORITY) };
        let mut issue = crate::query::tests::issue();
        IssueUpdate::set(&field, picked).apply_to(&mut issue);
        let shown = issue.value(&FieldId::new(FieldId::PRIORITY)).map(|v| v.to_input());
        assert_eq!(shown.as_deref(), Some("Major"));
        assert_eq!(issue.priority, Some(priority(2, "Major")), "ranked as the provider has it");
    }

    fn priority(rank: u32, name: &str) -> Priority {
        Priority { rank, name: name.into() }
    }

    fn user(name: &str) -> User {
        User { id: UserId::new("u"), display_name: name.into(), avatar_url: None }
    }

    /// A name in common use takes the level it means, wherever it is
    /// listed; one of a project's own takes its place in the list that
    /// defines it, spread over the five levels; with neither, the middle.
    #[test]
    fn a_priority_ranks_by_its_meaning_else_by_its_place() {
        let rank = |name, place| Priority::ranked(name, place).rank;
        let known = ["P0", "p3", "Blocker", "Major", "normal", "Trivial"].map(|n| rank(n, None));
        assert_eq!(known, [0, 3, 0, 1, 2, 4]);
        assert_eq!(rank("Blocker", Some((6, 11))), 0, "listed after Highest…Lowest, still 0");
        let own = [(0, 3), (1, 3), (2, 3), (2, 9), (8, 9)].map(|place| rank("Hot", Some(place)));
        assert_eq!(own, [0, 2, 4, 1, 4]);
        assert_eq!((rank("Hot", None), rank("Hot", Some((0, 1)))), (2, 2), "nothing to go by");
        assert_eq!(Priority::ranked("Soon", None).name, "Soon", "its own name kept");
    }

    /// A provider's list, each ranked: known names by their meaning, so a
    /// three-option field shows Low as low; its own by their place, read
    /// from the end when the known names go from the least urgent up.
    #[test]
    fn a_list_of_priorities_ranks_each_by_meaning_or_place() {
        let ranks =
            |names: &[&str]| Priority::list(names).iter().map(|p| p.rank).collect::<Vec<_>>();
        assert_eq!(ranks(&["High", "Medium", "Low"]), [1, 2, 3]);
        assert_eq!(ranks(&["Low", "Medium", "High"]), [3, 2, 1]);
        assert_eq!(ranks(&["P0", "P1", "P2"]), [0, 1, 2]);
        assert_eq!(ranks(&["Now", "Soon", "Later"]), [0, 2, 4], "none known: as listed");
        assert_eq!(ranks(&["Later", "Low", "Soon", "Urgent"]), [4, 3, 1, 0], "least urgent first");
        assert_eq!(Priority::list(&["Soon"])[0].name, "Soon");
    }

    #[test]
    fn priorities_sort_by_rank_urgent_first_and_show_in_five_levels() {
        let mut shuffled = [priority(3, "Low"), priority(1, "High"), priority(0, "Highest")];
        shuffled.sort();
        let names: Vec<_> = shuffled.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["Highest", "High", "Low"], "by rank, not by name");
        let levels: Vec<_> = [0, 3, 4, 9].map(|rank| priority(rank, "p").level()).to_vec();
        assert_eq!(levels, [0, 3, 4, 4], "past the last level, the last");
    }

    #[test]
    fn initials_take_the_first_and_last_word() {
        assert_eq!(user("Colin Tenaguillo").initials(), "CT");
        assert_eq!(user("Ana María López").initials(), "AL");
        assert_eq!(user("colin").initials(), "CO");
        assert_eq!(user("").initials(), "");
    }
}
