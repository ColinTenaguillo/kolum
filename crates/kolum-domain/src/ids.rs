//! Identifiers. Each is its own type so a key cannot be passed where an id
//! is wanted: on Jira `PROJ-12` and `10042` are both strings and both name
//! the same issue, but only one of them goes in a URL a person reads.

use serde::{Deserialize, Serialize};
use std::fmt;

macro_rules! id {
    ($(#[$doc:meta])* $name:ident) => {
        $(#[$doc])*
        #[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(pub String);

        impl $name {
            pub fn new(s: impl Into<String>) -> Self {
                Self(s.into())
            }
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }
    };
}

/// What a person reads and types: `PROJ-123`, `repo#42`, `draft-ccc333`.
/// Most are a number in a project, a Jira project or a GitHub repository;
/// one that is not (a GitHub draft) has neither.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(from = "String", into = "String")]
pub struct IssueKey {
    text: String,
    /// Where the project ends and its number starts in `text`. Always
    /// read from `text`, so two keys that read the same are the same.
    parts: Option<(usize, usize)>,
}

impl IssueKey {
    /// `new("ACME", "-", 12)` is `ACME-12`, issue 12 of project `ACME`.
    /// Written, then read back: a provider cannot make a key whose
    /// project differs from what its text says.
    pub fn new(project: &str, separator: &str, number: u64) -> Self {
        let key = Self::parse(format!("{project}{separator}{number}"));
        debug_assert_eq!(key.project(), Some(project), "{key} does not read as written");
        key
    }

    /// A key from its text, as a provider sends it, a file keeps it or a
    /// person types it: the one place a key's syntax is read. A number
    /// after the last `#` is in the repository before it (GitHub's
    /// `repo#12`; a repository's name may have dashes); a number after the
    /// first `-` is in the Jira project before it (`PROJ-12`: a capital,
    /// then capitals, digits or `_`). Anything else is in no project.
    pub fn parse(text: impl Into<String>) -> Self {
        let text = text.into();
        Self { parts: parts(&text), text }
    }

    pub fn as_str(&self) -> &str {
        &self.text
    }

    /// A key as a person types or pastes it, or a link holds it: trimmed,
    /// one word, in a project; a Jira key in capitals whatever case it came
    /// in (`proj-12` is `PROJ-12`), GitHub's `repo#12` as it is.
    pub fn typed(text: &str) -> Option<Self> {
        let text = text.trim();
        if text.contains(char::is_whitespace) {
            return None;
        }
        let key = match text.contains('#') {
            true => Self::parse(text),
            false => Self::parse(text.to_uppercase()),
        };
        key.project().is_some().then_some(key)
    }

    /// The Jira project or the GitHub repository the issue is numbered in.
    pub fn project(&self) -> Option<&str> {
        self.parts.map(|(end, _)| &self.text[..end])
    }

    /// Issue `number` of the same project, written the same way.
    pub fn with_number(&self, number: u64) -> Option<Self> {
        let (_, start) = self.parts?;
        Some(Self::parse(format!("{}{number}", &self.text[..start])))
    }
}

fn parts(text: &str) -> Option<(usize, usize)> {
    let (project, number) = match text.rsplit_once('#') {
        Some(split) => split,
        None => text.split_once('-').filter(|(project, _)| is_jira_project(project))?,
    };
    let numbered = !number.is_empty() && number.bytes().all(|b| b.is_ascii_digit());
    (numbered && !project.is_empty()).then_some((project.len(), project.len() + 1))
}

fn is_jira_project(text: &str) -> bool {
    text.starts_with(|c: char| c.is_ascii_uppercase())
        && text.bytes().all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_')
}

impl From<String> for IssueKey {
    fn from(text: String) -> Self {
        Self::parse(text)
    }
}

impl From<IssueKey> for String {
    fn from(key: IssueKey) -> Self {
        key.text
    }
}

impl fmt::Display for IssueKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.text)
    }
}

id!(
    /// The provider's internal id for an issue.
    IssueId
);
id!(BoardId);
id!(StatusId);
id!(TransitionId);
id!(UserId);
id!(
    /// A provider field, custom or not. Custom field ids change from one
    /// Jira instance to the next, which is why the config can name them.
    FieldId
);

/// The fields an issue holds as members, by the id every provider gives
/// them: an adapter names its own so (Jira's `duedate` is `due`). Any
/// other id is a custom field's.
impl FieldId {
    pub const TITLE: &'static str = "title";
    pub const DESCRIPTION: &'static str = "description";
    pub const LABELS: &'static str = "labels";
    pub const PRIORITY: &'static str = "priority";
    pub const DUE: &'static str = "due";
    pub const ASSIGNEE: &'static str = "assignee";
    pub const REPORTER: &'static str = "reporter";
    /// The parent issue, an epic or another.
    pub const EPIC: &'static str = "epic";
    /// The estimate in points.
    pub const ESTIMATE: &'static str = "estimate";
    pub const SPRINT: &'static str = "sprint";
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_typed_key_is_one_word_in_a_project() {
        let typed = |t: &str| IssueKey::typed(t).map(|k| k.to_string());
        assert_eq!(typed(" proj-12\n"), Some("PROJ-12".into()));
        assert_eq!(typed("Web#4"), Some("Web#4".into()), "a repository keeps its case");
        assert_eq!(typed("PROJ-12 and more"), None);
        assert_eq!(typed("dashboards"), None);
        assert_eq!(typed(""), None);
    }

    #[test]
    fn a_key_knows_its_project_and_reads_as_written() {
        let jira = IssueKey::new("ACME", "-", 12);
        let github = IssueKey::new("python-language-server", "#", 199);
        assert_eq!((jira.to_string(), jira.project()), ("ACME-12".into(), Some("ACME")));
        assert_eq!(github.to_string(), "python-language-server#199");
        assert_eq!(github.project(), Some("python-language-server"));
        let seven = github.with_number(7).unwrap();
        assert_eq!(
            (seven.as_str(), seven.project()),
            ("python-language-server#7", github.project())
        );
    }

    #[test]
    fn a_key_that_is_no_number_in_a_project_has_neither() {
        // A GitHub draft, Jira-looking text that is not a Jira key, a key
        // with no number.
        for text in
            ["draft-ccc333", "draft-123456", "web-12", "2FA-1", "ACME-", "ACME", "-12", "#12"]
        {
            let key = IssueKey::parse(text);
            assert_eq!((key.as_str(), key.project()), (text, None));
            assert_eq!(key.with_number(7), None, "{text}");
        }
    }

    #[test]
    fn a_saved_key_reads_back_as_the_one_its_provider_made() {
        for made in [
            IssueKey::new("ACME", "-", 12),
            IssueKey::new("MY_P2", "-", 7),
            IssueKey::new("python-language-server", "#", 199),
            IssueKey::parse("draft-ccc333"),
        ] {
            // What serde writes and reads: the text alone, as before.
            let saved = String::from(made.clone());
            assert_eq!(saved, made.to_string());
            let read = IssueKey::from(saved);
            assert_eq!((&read, read.project()), (&made, made.project()));
        }
    }
}
