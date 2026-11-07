//! The use cases between the TUI and a provider. The TUI only ever sees
//! this crate: it sends a [`Command`] and later receives an [`Event`], and
//! never waits on the network itself.

mod board;
mod describe;
mod issue_store;
mod people;
mod query;
mod service;
mod store;
mod write;
mod writer;

pub use board::{BoardSnapshot, load_board, sync_board};
pub use describe::{changed_since, description_update, merge_drafts, new_issue, titled};
pub use people::{assignable, assignees};
pub use query::parse_query;
pub use service::{Command, Event, IssueDetail, Service, layout_key};
pub use store::{IssueArchive, Storage, Store};
pub use write::{
    Change, Closed, Offer, Outgoing, Pending, Unmoved, Write, assign_writes, comment_write,
    field_choices, field_update, in_full, land, offer, rank_write, refresh, supersedes,
    transition_to, transition_writes, update_write, written,
};
