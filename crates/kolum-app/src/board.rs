use kolum_domain::{Board, BoardId, DomainError, Issue, Provider, Query, Result};
use time::OffsetDateTime;

/// A board and its cards, as last loaded.
#[derive(Debug, Clone, PartialEq)]
pub struct BoardSnapshot {
    pub board: Board,
    /// In the provider's order (its rank), which is the order of a column.
    pub issues: Vec<Issue>,
}

impl BoardSnapshot {
    /// Takes in changed issues: each replaces the one with its key, or
    /// joins the end when it is new. A sync may repeat an issue (Jira's
    /// `updated >=` has minute grain), which this makes harmless.
    pub fn merge(&mut self, changed: Vec<Issue>) {
        for issue in changed {
            match self.issues.iter_mut().find(|i| i.key == issue.key) {
                Some(old) => *old = issue,
                None => self.issues.push(issue),
            }
        }
    }

    /// A number alone, `12983`, read as the key of this board's project:
    /// `ACME-12983`. Only when the board's keys read that way; GitHub's
    /// `repo#12` do not.
    pub fn qualify(&self, query: Query) -> Query {
        let Query::Filter(mut filter) = query else { return query };
        let number = filter
            .text
            .as_deref()
            .map(str::trim)
            .filter(|t| !t.is_empty() && t.chars().all(|c| c.is_ascii_digit()));
        let prefix = self.board.project.as_ref().map(|p| format!("{}-", p.key));
        let prefix = prefix.filter(|p| self.issues.iter().any(|i| i.key.as_str().starts_with(p)));
        if let (Some(number), Some(prefix)) = (number, prefix) {
            filter.text = Some(format!("{prefix}{number}"));
        }
        Query::Filter(filter)
    }

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
        None => first_board(provider).await?,
    };
    let board = provider.board(&id).await?;
    let issues = all_issues(provider, &id, None).await?;
    Ok(BoardSnapshot { board, issues })
}

/// Brings a cached snapshot up to date: the configuration again (it is
/// small and columns do change), and only the issues updated since.
///
/// An issue that left the board (deleted, or out of its filter) is not
/// reported by an incremental sync, so it stays until a full reload.
pub async fn sync_board(
    provider: &dyn Provider,
    mut cached: BoardSnapshot,
    since: OffsetDateTime,
) -> Result<BoardSnapshot> {
    let id = cached.board.id.clone();
    cached.board = provider.board(&id).await?;
    let changed = all_issues(provider, &id, Some(since)).await?;
    cached.merge(changed);
    Ok(cached)
}

async fn all_issues(
    provider: &dyn Provider,
    id: &BoardId,
    since: Option<OffsetDateTime>,
) -> Result<Vec<Issue>> {
    let mut issues = Vec::new();
    let mut token = None;
    loop {
        let page = provider.board_issues(id, since, token).await?;
        issues.extend(page.items);
        token = page.next;
        if token.is_none() {
            return Ok(issues);
        }
    }
}

async fn first_board(provider: &dyn Provider) -> Result<BoardId> {
    let first = provider.boards().await?.into_iter().next();
    first.map(|b| b.id).ok_or_else(|| DomainError::NotFound("any board".into()))
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
    async fn a_bare_number_is_an_issue_of_the_board_project() {
        let mut snapshot = load_board(&FakeProvider::new(), None).await.unwrap();
        let text = |s: &BoardSnapshot, q: &str| match s.qualify(crate::parse_query(q)) {
            Query::Filter(f) => f.text,
            Query::Native(n) => Some(format!("native {n}")),
        };
        assert_eq!(text(&snapshot, " 12983 "), Some("ACME-12983".into()));
        assert_eq!(text(&snapshot, "#bug 12"), Some("ACME-12".into()));
        assert_eq!(text(&snapshot, "login 12"), Some("login 12".into()), "a number among words");
        assert_eq!(text(&snapshot, "!12"), Some("native 12".into()));
        let mut no_project = snapshot.clone();
        no_project.board.project = None;
        assert_eq!(text(&no_project, "12"), Some("12".into()), "no project");
        for key in snapshot.issues.iter_mut() {
            key.key = kolum_domain::IssueKey::new(format!("web#{}", key.key.as_str().len()));
        }
        assert_eq!(text(&snapshot, "12"), Some("12".into()), "keys not of the project's form");
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
    async fn a_sync_merges_only_what_changed_since() {
        use kolum_domain::{IssueKey, IssueReader, IssueUpdate, IssueWriter, NewIssue};
        let fake = FakeProvider::new();
        let cached = load_board(&fake, None).await.unwrap();
        let since = time::OffsetDateTime::now_utc();
        let key = IssueKey::new("ACME-12");
        fake.update(
            &key,
            IssueUpdate { title: Some("Export as CSV and XLSX".into()), ..Default::default() },
        )
        .await
        .unwrap();
        let template = fake.issue(&key).await.unwrap();
        let created = fake
            .create(NewIssue {
                project: "ACME".into(),
                kind: template.kind,
                title: "Brand new".into(),
                description: template.description,
                parent: None,
            })
            .await
            .unwrap();

        let synced = sync_board(&fake, cached.clone(), since).await.unwrap();
        assert_eq!(synced.issues.len(), cached.issues.len() + 1, "the new one joined");
        let title = |s: &BoardSnapshot, k: &IssueKey| {
            s.issues.iter().find(|i| &i.key == k).map(|i| i.title.clone())
        };
        assert_eq!(title(&synced, &key).as_deref(), Some("Export as CSV and XLSX"));
        assert_eq!(title(&synced, &created.key).as_deref(), Some("Brand new"));
        let position = |s: &BoardSnapshot| s.issues.iter().position(|i| i.key == key);
        assert_eq!(position(&synced), position(&cached), "a changed issue keeps its place");
    }

    #[test]
    fn merging_twice_is_merging_once() {
        let fake_issue = |key: &str, title: &str| {
            let mut i = sample_issue();
            i.key = kolum_domain::IssueKey::new(key);
            i.title = title.into();
            i
        };
        let mut s = BoardSnapshot { board: sample_board(), issues: vec![fake_issue("A-1", "one")] };
        let changed = vec![fake_issue("A-1", "uno"), fake_issue("A-2", "two")];
        s.merge(changed.clone());
        let once = s.clone();
        s.merge(changed);
        assert_eq!(s, once);
        assert_eq!(s.issues.iter().map(|i| i.title.as_str()).collect::<Vec<_>>(), ["uno", "two"]);
    }

    fn sample_board() -> Board {
        Board {
            id: BoardId::new("1"),
            name: "b".into(),
            kind: kolum_domain::BoardKind::Kanban,
            project: None,
            columns: vec![],
        }
    }

    fn sample_issue() -> Issue {
        let rt = tokio::runtime::Builder::new_current_thread().build().unwrap();
        rt.block_on(async {
            load_board(&FakeProvider::new(), None).await.unwrap().issues.remove(0)
        })
    }

    #[tokio::test]
    async fn an_unknown_board_is_not_found() {
        let missing = load_board(&FakeProvider::new(), Some(BoardId::new("nope"))).await;
        assert!(matches!(missing, Err(kolum_domain::DomainError::NotFound(_))));
    }
}
