use crate::ids::{BoardId, StatusId};
use serde::{Deserialize, Serialize};

/// A space that holds boards: a Jira project, a GitHub repository or org.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Project {
    pub key: String,
    pub name: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum BoardKind {
    Kanban,
    Scrum,
    Other,
}

/// A board as a list of boards shows it, before its configuration is read.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BoardSummary {
    pub id: BoardId,
    pub name: String,
    pub kind: BoardKind,
    pub project: Option<Project>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Board {
    pub id: BoardId,
    pub name: String,
    pub kind: BoardKind,
    pub project: Option<Project>,
    /// In display order, left to right.
    pub columns: Vec<Column>,
}

/// A column groups one or more statuses: "In Progress" can hold both
/// "In Progress" and "Blocked". A status in no column is not on the board.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Column {
    pub name: String,
    pub statuses: Vec<StatusId>,
    pub wip_limit: Option<u32>,
}

impl Board {
    /// The column a status lands in, if the board shows it at all.
    pub fn column_of(&self, status: &StatusId) -> Option<usize> {
        self.columns.iter().position(|c| c.statuses.contains(status))
    }
}
