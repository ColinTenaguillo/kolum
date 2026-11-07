//! A search, in terms every provider can translate: Jira turns it into
//! JQL, GitHub into its search syntax. `Native` is the way out for what
//! the neutral form cannot say.

use crate::ids::UserId;
use crate::issue::Issue;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Query {
    Filter(Filter),
    /// Sent to the provider as written: JQL for Jira.
    Native(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum UserSel {
    Me,
    /// Matched against the display name, so `@colin` finds "Colin T.",
    /// and against it without its spaces, so `@AnaSilva` is one person.
    Named(String),
    /// No one.
    Nobody,
}

/// Every field is a constraint; an empty one constrains nothing. Values of
/// one field are alternatives (`#bug #story` is either), fields combine
/// (`#bug @me` is both).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Filter {
    pub text: Option<String>,
    /// Alternatives, like every other field: `@me @ana` is either.
    pub assignees: Vec<UserSel>,
    pub statuses: Vec<String>,
    pub types: Vec<String>,
    pub labels: Vec<String>,
    pub epic: Option<String>,
    pub sprint: Option<String>,
}

fn contains(haystack: &str, needle: &str) -> bool {
    haystack.to_lowercase().contains(&needle.to_lowercase())
}

fn any_or_empty(values: &[String], pred: impl Fn(&str) -> bool) -> bool {
    values.is_empty() || values.iter().any(|v| pred(v))
}

impl Filter {
    /// Whether the issue meets every constraint except `text`. Free text is
    /// left out because locally it is matched fuzzily and ranked, which is
    /// not a yes or no.
    pub fn matches_fields(&self, issue: &Issue, me: &UserId) -> bool {
        let assignee = self.assignees.is_empty()
            || self.assignees.iter().any(|sel| match (sel, &issue.assignee) {
                (UserSel::Me, _) => issue.is_assigned_to(me),
                (UserSel::Nobody, a) => a.is_none(),
                (UserSel::Named(n), Some(a)) => {
                    contains(&a.display_name, n) || contains(&a.display_name.replace(' ', ""), n)
                }
                (UserSel::Named(_), None) => false,
            });
        let epic = self.epic.as_ref().is_none_or(|e| {
            issue
                .parent
                .as_ref()
                .is_some_and(|p| p.key.as_str().eq_ignore_ascii_case(e) || contains(&p.title, e))
        });
        let sprint = self
            .sprint
            .as_ref()
            .is_none_or(|s| issue.sprint.as_ref().is_some_and(|sp| contains(&sp.name, s)));
        assignee
            && epic
            && sprint
            && any_or_empty(&self.statuses, |s| contains(&issue.status.name, s))
            && any_or_empty(&self.types, |t| contains(&issue.kind.name, t))
            && any_or_empty(&self.labels, |l| {
                issue.labels.iter().any(|x| x.eq_ignore_ascii_case(l))
            })
    }

    pub fn has_fields(&self) -> bool {
        Filter { text: None, ..self.clone() } != Filter::default()
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::*;

    /// An issue to test on: PROJ-1, assigned to Ana, in an epic.
    pub(crate) fn issue() -> Issue {
        let ana =
            User { id: UserId::new("ana"), display_name: "Ana Silva".into(), avatar_url: None };
        Issue {
            id: IssueId::new("1"),
            key: IssueKey::parse("PROJ-1"),
            title: "Login times out".into(),
            kind: IssueType { name: "Bug".into(), kind: IssueKind::Bug },
            status: Status {
                id: StatusId::new("3"),
                name: "In Review".into(),
                category: StatusCategory::Doing,
            },
            priority: Some(Priority { rank: 1, name: "High".into() }),
            assignee: Some(ana),
            reporter: None,
            labels: vec!["backend".into()],
            parent: Some(IssueRef {
                key: IssueKey::parse("PROJ-10"),
                title: "Single sign-on".into(),
                kind: IssueKind::Epic,
                status: None,
            }),
            subtasks: vec![],
            links: vec![],
            estimate: None,
            sprint: Some(Sprint { id: "s1".into(), name: "Sprint 14".into() }),
            created: time::OffsetDateTime::UNIX_EPOCH,
            updated: time::OffsetDateTime::UNIX_EPOCH,
            due: None,
            description: Some(RichText::default()),
            fields: vec![],
            url: None,
            attachments: vec![],
        }
    }

    fn matches(f: Filter) -> bool {
        f.matches_fields(&issue(), &UserId::new("ana"))
    }

    #[test]
    fn an_empty_filter_matches_everything() {
        assert!(matches(Filter::default()));
        assert!(!Filter::default().has_fields());
    }

    #[test]
    fn each_field_constrains() {
        let who = |sel: Vec<UserSel>| Filter { assignees: sel, ..Default::default() };
        assert!(matches(who(vec![UserSel::Me])));
        assert!(
            matches(who(vec![UserSel::Named("AnaSilva".into())])),
            "the name without its spaces"
        );
        assert!(!matches(who(vec![UserSel::Nobody])), "Ana has it");
        assert!(matches(who(vec![UserSel::Named("bob".into()), UserSel::Me])), "alternatives");
        assert!(matches(Filter {
            assignees: vec![UserSel::Named("ana".into())],
            ..Default::default()
        }));
        assert!(!matches(Filter {
            assignees: vec![UserSel::Named("bob".into())],
            ..Default::default()
        }));
        assert!(matches(Filter { statuses: vec!["review".into()], ..Default::default() }));
        assert!(!matches(Filter { statuses: vec!["done".into()], ..Default::default() }));
        assert!(matches(Filter { labels: vec!["BACKEND".into()], ..Default::default() }));
        assert!(!matches(Filter { labels: vec!["back".into()], ..Default::default() }));
        assert!(matches(Filter { epic: Some("proj-10".into()), ..Default::default() }));
        assert!(matches(Filter { epic: Some("sign-on".into()), ..Default::default() }));
        assert!(matches(Filter { sprint: Some("14".into()), ..Default::default() }));
        assert!(!matches(Filter { sprint: Some("15".into()), ..Default::default() }));
    }

    #[test]
    fn values_of_one_field_are_alternatives_and_fields_combine() {
        assert!(matches(Filter {
            types: vec!["story".into(), "bug".into()],
            ..Default::default()
        }));
        assert!(!matches(Filter {
            types: vec!["bug".into()],
            statuses: vec!["done".into()],
            ..Default::default()
        }));
    }
}
