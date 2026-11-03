//! The use cases between the TUI and a provider. The TUI only ever sees
//! this crate: it sends a [`Command`] and later receives an [`Event`], and
//! never waits on the network itself.

mod board;
mod query;
mod service;

pub use board::{BoardSnapshot, load_board};
pub use query::parse_query;
pub use service::{Command, Event, IssueDetail, Service};
