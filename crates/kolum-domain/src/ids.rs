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

id!(
    /// What a person reads and types: `PROJ-123`, `#42`.
    IssueKey
);
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
