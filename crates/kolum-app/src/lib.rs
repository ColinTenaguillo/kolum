//! The use cases between the TUI and a provider. The TUI only ever sees
//! this crate: it sends a [`Command`] and later receives an [`Event`], and
//! never waits on the network itself.

mod board;
mod cache;
mod query;
mod service;
mod write;

pub use board::{BoardSnapshot, load_board, sync_board};
pub use cache::Cache;
pub use query::parse_query;
pub use service::{Command, Event, IssueDetail, Service, layout_key};
pub use write::{Change, Ledger, Write, apply, move_issue, transitions_into};
