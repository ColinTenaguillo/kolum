//! The ports a provider implements. Small traits rather than one large one,
//! so a read-only adapter is honest about it: what it cannot do it answers
//! with `Unsupported`, and says so up front in `Capabilities`.

use crate::board::{Board, BoardSummary};
use crate::error::Result;
use crate::ids::{BoardId, IssueKey, TransitionId, UserId};
use crate::issue::{
    ActivityEntry, Comment, EditableField, FieldLayout, Issue, IssueRef, IssueUpdate, NewIssue,
    Sprint, Transition, User,
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
    /// The board's issues can be listed by when they changed, so a sync
    /// reads only those; without it, a sync reads the whole board again.
    ChangedSince,
    /// Several issues with their comments and history in one request:
    /// what a board prefetches so each opens whole.
    BulkRead,
}

/// An issue with its comments and history, read along with others. On a
/// long one they may be cut short; opening the issue reads them whole.
#[derive(Debug, Clone, PartialEq)]
pub struct IssueInFull {
    pub issue: Issue,
    pub comments: Vec<Comment>,
    pub activity: Vec<ActivityEntry>,
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
    /// since then: the periodic incremental sync, asked only of a provider
    /// with `ChangedSince`.
    async fn board_issues(
        &self,
        id: &BoardId,
        since: Option<OffsetDateTime>,
        page: Option<PageToken>,
    ) -> Result<Page<Issue>>;

    /// The keys of the board's issues in its order: what an incremental
    /// sync reads for the order, which a rank changes without updating any
    /// issue, and for what left the board. Every page of `board_issues`
    /// unless the provider has a lighter way.
    async fn board_keys(&self, id: &BoardId) -> Result<Vec<IssueKey>> {
        let mut keys = vec![];
        let mut token = None;
        loop {
            let page = self.board_issues(id, None, token).await?;
            keys.extend(page.items.into_iter().map(|i| i.key));
            token = page.next;
            if token.is_none() {
                return Ok(keys);
            }
        }
    }

    /// The board's running and coming sprints, to put an issue in one.
    async fn sprints(&self, _board: &BoardId) -> Result<Vec<Sprint>> {
        Err(crate::DomainError::Unsupported(Capability::Sprints))
    }
}

#[async_trait]
pub trait IssueReader: Send + Sync {
    async fn issue(&self, key: &IssueKey) -> Result<Issue>;
    async fn comments(&self, key: &IssueKey) -> Result<Vec<Comment>>;
    async fn activity(&self, key: &IssueKey) -> Result<Vec<ActivityEntry>>;

    /// Any number of issues in full, asked only of a provider with
    /// `BulkRead`, handed to `read` a batch at a time as each comes: a
    /// failure ends the call, and what was handed before it stays read.
    /// One it cannot read, refused or of an odd shape, is left out rather
    /// than failing the others.
    async fn issues_in_full(
        &self,
        _keys: &[IssueKey],
        _read: &mut (dyn FnMut(Vec<IssueInFull>) + Send),
    ) -> Result<()> {
        Err(crate::DomainError::Unsupported(Capability::BulkRead))
    }

    /// The issues whose parent it is: an epic's stories, a story's sub-tasks.
    async fn children(&self, _key: &IssueKey) -> Result<Vec<IssueRef>> {
        Err(crate::DomainError::Unsupported(Capability::Epics))
    }

    /// The fields the provider's own issue view shows for the issue, in its
    /// order, when it has a layout to say so.
    async fn field_layout(&self, _key: &IssueKey) -> Result<FieldLayout> {
        Err(crate::DomainError::Unsupported(Capability::CustomFields))
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::future::Future;
    use std::pin::pin;
    use std::task::{Context, Poll, Waker};

    /// Two pages of one issue each.
    struct TwoPages;

    #[async_trait]
    impl BoardSource for TwoPages {
        async fn boards(&self) -> Result<Vec<BoardSummary>> {
            unreachable!()
        }
        async fn board(&self, _: &BoardId) -> Result<Board> {
            unreachable!()
        }
        async fn board_issues(
            &self,
            _: &BoardId,
            _: Option<OffsetDateTime>,
            page: Option<PageToken>,
        ) -> Result<Page<Issue>> {
            let mut issue = crate::query::tests::issue();
            issue.key = IssueKey::parse(if page.is_none() { "PROJ-1" } else { "PROJ-2" });
            let next = page.is_none().then(|| PageToken("2".into()));
            Ok(Page { items: vec![issue], next })
        }
    }

    /// Nothing here waits on I/O, so one poll finishes it.
    fn now<T>(future: impl Future<Output = T>) -> T {
        match pin!(future).poll(&mut Context::from_waker(Waker::noop())) {
            Poll::Ready(value) => value,
            Poll::Pending => unreachable!(),
        }
    }

    #[test]
    fn the_board_keys_are_every_page_of_issues_in_order() {
        let keys = now(TwoPages.board_keys(&BoardId::new("1"))).unwrap();
        assert_eq!(keys, [IssueKey::parse("PROJ-1"), IssueKey::parse("PROJ-2")]);
    }
}
