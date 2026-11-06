//! The provider-neutral model of a ticket tracker.
//!
//! Jira, GitHub Projects or Linear each map their own shapes onto these
//! types; nothing in this crate names one of them. The words a provider
//! would use (JQL, ADF, a Jira sprint field id) belong in its adapter.

mod board;
mod error;
mod ids;
mod issue;
mod ports;
mod query;
pub mod rich_text;

pub use board::{Board, BoardKind, BoardSummary, Column, Project};
pub use error::{DomainError, Result};
pub use ids::{BoardId, FieldId, IssueId, IssueKey, StatusId, TransitionId, UserId};
pub use issue::{
    ActivityEntry, Attachment, Comment, CustomField, EditableField, Estimate, FieldChange,
    FieldEdit, FieldKind, FieldLayout, FieldValue, Issue, IssueKind, IssueLink, IssueRef,
    IssueType, IssueUpdate, LayoutField, NewIssue, Priority, Sprint, Status, StatusCategory,
    Transition, User,
};
pub use ports::{
    BULK_READ_MAX, BoardSource, Capabilities, Capability, IssueInFull, IssueReader, IssueSearch,
    IssueWriter, Page, PageToken, Provider, Rank, UserDirectory,
};
pub use query::{Filter, Query, UserSel};
pub use rich_text::RichText;
