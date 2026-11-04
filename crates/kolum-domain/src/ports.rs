//! The ports a provider implements. Small traits rather than one large one,
//! so a read-only adapter is honest about it: what it cannot do it answers
//! with `Unsupported`, and says so up front in `Capabilities`.

use crate::board::{Board, BoardSummary};
use crate::error::Result;
use crate::ids::{BoardId, IssueKey, TransitionId, UserId};
use crate::issue::{
    ActivityEntry, Comment, EditableField, Issue, IssueUpdate, NewIssue, Transition, User,
};
use crate::query::Query;
use crate::rich_text::RichText;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use time::OffsetDateTime;

/// An opaque cursor: an offset on one provider, a `nextPageToken` on Jira
/// Cloud. Only the provider that issued it reads it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PageToken(pub String);

#[derive(Debug, Clone, PartialEq)]
pub struct Page<T> {
    pub items: Vec<T>,
    pub next: Option<PageToken>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum Capability {
    Sprints,
    Epics,
    Estimates,
    WipLimits,
    NativeQuery,
    CustomFields,
    Comments,
    Activity,
    Write,
    /// Avatars can be fetched.
    Images,
    /// Cards can be put in order within a column.
    Rank,
}

/// Where a card goes in its column's order, relative to another card.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Rank {
    Before(IssueKey),
    After(IssueKey),
}

/// What a provider can do. The UI hides the rest instead of offering it
/// and failing.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Capabilities(BTreeSet<Capability>);

impl Capabilities {
    pub fn new(caps: impl IntoIterator<Item = Capability>) -> Self {
        Self(caps.into_iter().collect())
    }
    pub fn supports(&self, cap: Capability) -> bool {
        self.0.contains(&cap)
    }
}

#[async_trait]
pub trait BoardSource: Send + Sync {
    async fn boards(&self) -> Result<Vec<BoardSummary>>;
    /// The columns and the statuses each one holds.
    async fn board(&self, id: &BoardId) -> Result<Board>;
    /// One page of the board's issues. With `since`, only those updated
    /// since then: the incremental sync after a warm start.
    async fn board_issues(
        &self,
        id: &BoardId,
        since: Option<OffsetDateTime>,
        page: Option<PageToken>,
    ) -> Result<Page<Issue>>;
}

#[async_trait]
pub trait IssueReader: Send + Sync {
    async fn issue(&self, key: &IssueKey) -> Result<Issue>;
    async fn comments(&self, key: &IssueKey) -> Result<Vec<Comment>>;
    async fn activity(&self, key: &IssueKey) -> Result<Vec<ActivityEntry>>;
}

#[async_trait]
pub trait IssueSearch: Send + Sync {
    async fn search(&self, query: &Query, page: Option<PageToken>) -> Result<Page<Issue>>;
}

#[async_trait]
pub trait IssueWriter: Send + Sync {
    /// The transitions open from the issue's current status.
    async fn transitions(&self, key: &IssueKey) -> Result<Vec<Transition>>;
    async fn transition(&self, key: &IssueKey, transition: &TransitionId) -> Result<()>;
    /// `None` unassigns.
    async fn assign(&self, key: &IssueKey, user: Option<&UserId>) -> Result<()>;
    async fn comment(&self, key: &IssueKey, body: RichText) -> Result<Comment>;
    async fn update(&self, key: &IssueKey, update: IssueUpdate) -> Result<()>;
    async fn create(&self, issue: NewIssue) -> Result<Issue>;
    async fn clone_issue(&self, key: &IssueKey) -> Result<Issue>;

    /// The fields kolum may set on the issue, and how each is edited.
    async fn editable_fields(&self, _key: &IssueKey) -> Result<Vec<EditableField>> {
        Err(crate::DomainError::Unsupported(Capability::CustomFields))
    }

    /// Moves the issue in the board's order, next to another.
    async fn rank(&self, _key: &IssueKey, _rank: &Rank) -> Result<()> {
        Err(crate::DomainError::Unsupported(Capability::Rank))
    }
}

#[async_trait]
pub trait UserDirectory: Send + Sync {
    async fn me(&self) -> Result<User>;
    async fn find_users(&self, query: &str) -> Result<Vec<User>>;

    /// The bytes behind an avatar URL, fetched the way this provider can
    /// reach it (Jira's often need the session's credentials).
    async fn image(&self, _url: &str) -> Result<Vec<u8>> {
        Err(crate::DomainError::Unsupported(Capability::Images))
    }
}

/// Everything the application needs from one provider instance.
pub trait Provider: BoardSource + IssueReader + IssueSearch + IssueWriter + UserDirectory {
    /// Lower-case and stable: it names the cache directory.
    fn name(&self) -> &str;
    fn capabilities(&self) -> Capabilities;
}
