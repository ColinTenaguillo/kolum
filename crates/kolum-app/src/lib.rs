//! The use cases between the TUI and a provider. The TUI only ever sees
//! this crate: it sends a [`Command`] and later receives an [`Event`], and
//! never waits on the network itself.

mod board;
mod query;
mod service;
mod store;
mod write;

pub use board::{BoardSnapshot, load_board, sync_board};
pub use query::parse_query;
pub use service::{Command, Event, IssueDetail, Service, layout_key};
pub use store::{IssueArchive, Storage, Store};
pub use write::{
    Change, Pending, Write, asks_for, land, move_issue, supersedes, transition_to,
    transition_write, transitions_into, written,
};
