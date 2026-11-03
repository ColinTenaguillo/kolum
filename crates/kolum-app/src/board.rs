use kolum_domain::{Board, BoardId, Issue, Provider, Result};

/// A board and its cards, as last loaded.
#[derive(Debug, Clone, PartialEq)]
pub struct BoardSnapshot {
    pub board: Board,
    /// In the provider's order (its rank), which is the order of a column.
    pub issues: Vec<Issue>,
}

impl BoardSnapshot {
    /// The cards of each column, left to right. A card whose status is in
    /// no column is not on the board, as in the web app.
    pub fn columns(&self) -> Vec<Vec<&Issue>> {
        let mut columns = vec![Vec::new(); self.board.columns.len()];
        for issue in &self.issues {
            if let Some(c) = self.board.column_of(&issue.status.id) {
                columns[c].push(issue);
            }
        }
        columns
    }
}

/// The board's configuration and every page of its cards. `None` opens the
/// first board the provider lists.
pub async fn load_board(provider: &dyn Provider, id: Option<BoardId>) -> Result<BoardSnapshot> {
    let id = match id {
        Some(id) => id,
        None => match provider.boards().await?.into_iter().next() {
            Some(first) => first.id,
            None => return Err(kolum_domain::DomainError::NotFound("any board".into())),
        },
    };
    let board = provider.board(&id).await?;
    let mut issues = Vec::new();
    let mut token = None;
    loop {
        let page = provider.board_issues(&id, None, token).await?;
        issues.extend(page.items);
        match page.next {
            Some(next) => token = Some(next),
            None => break,
        }
    }
    Ok(BoardSnapshot { board, issues })
}

#[cfg(test)]
mod tests {
    use super::*;
    use kolum_provider_fake::FakeProvider;

    #[tokio::test]
    async fn every_page_is_loaded_and_cards_fall_into_their_columns() {
        let snapshot = load_board(&FakeProvider::new(), None).await.unwrap();
        let columns = snapshot.columns();
        assert_eq!(columns.len(), 5);
        assert_eq!(columns.iter().map(Vec::len).sum::<usize>(), snapshot.issues.len());
        assert!(snapshot.issues.len() > 20, "more than one page of cards");
        // "In Progress" holds both In Progress and Blocked.
        let names: std::collections::BTreeSet<_> =
            columns[2].iter().map(|i| i.status.name.as_str()).collect();
        assert_eq!(names.into_iter().collect::<Vec<_>>(), ["Blocked", "In Progress"]);
    }

    #[tokio::test]
    async fn a_status_in_no_column_keeps_the_card_off_the_board() {
        let mut snapshot = load_board(&FakeProvider::new(), None).await.unwrap();
        snapshot.board.columns.retain(|c| c.name != "Done");
        let shown: usize = snapshot.columns().iter().map(Vec::len).sum();
        let done = snapshot.issues.iter().filter(|i| i.status.name == "Done").count();
        assert!(done > 0);
        assert_eq!(shown, snapshot.issues.len() - done);
    }

    #[tokio::test]
    async fn an_unknown_board_is_not_found() {
        let missing = load_board(&FakeProvider::new(), Some(BoardId::new("nope"))).await;
        assert!(matches!(missing, Err(kolum_domain::DomainError::NotFound(_))));
    }
}
