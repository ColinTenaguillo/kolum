use crate::board::{BoardSnapshot, load_board, sync_board};
use crate::cache::Cache;
use kolum_domain::{
    ActivityEntry, BoardId, BoardSummary, Capabilities, Comment, DomainError, Issue, IssueKey,
    Provider, Query, User,
};
use std::sync::Arc;
use tokio::sync::mpsc;

/// What the TUI asks for.
#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    /// The cached board at once if there is one, then synced. `None` opens
    /// the provider's first board.
    LoadBoard(Option<BoardId>),
    /// Everything again, ignoring the cache: what `r` does.
    ReloadBoard(Option<BoardId>),
    LoadBoards,
    LoadIssue(IssueKey),
    /// A search for the picker. A new one cancels the one before: only the
    /// latest query's results matter.
    Search {
        id: u64,
        query: Query,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct IssueDetail {
    pub issue: Issue,
    pub comments: Vec<Comment>,
    pub activity: Vec<ActivityEntry>,
}

/// What comes back, in whatever order the requests finish.
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    Me(User),
    /// `fresh` is false for the cached copy that arrives before the sync.
    BoardLoaded {
        snapshot: BoardSnapshot,
        fresh: bool,
    },
    Boards(Vec<BoardSummary>),
    IssueLoaded(Box<IssueDetail>),
    /// For the search with this id; stale ones were cancelled.
    SearchResults {
        id: u64,
        result: Result<Vec<Issue>, DomainError>,
    },
    /// `what` says which request failed, in words for a status line.
    Failed {
        what: String,
        error: DomainError,
    },
}

/// The TUI's handle on a provider. Every command runs on its own task, so a
/// slow request never holds up the next one, nor the screen.
pub struct Service {
    commands: mpsc::UnboundedSender<Command>,
    capabilities: Capabilities,
    provider: String,
}

impl Service {
    /// Starts the service on the current tokio runtime. Asks for the
    /// signed-in user straight away: the board marks that user's cards.
    pub fn spawn(
        provider: Arc<dyn Provider>,
        cache: Option<Cache>,
    ) -> (Self, mpsc::UnboundedReceiver<Event>) {
        let (commands, mut rx) = mpsc::unbounded_channel();
        let (events, events_rx) = mpsc::unbounded_channel();
        let service = Service {
            commands,
            capabilities: provider.capabilities(),
            provider: provider.name().to_string(),
        };
        let me = (provider.clone(), events.clone());
        tokio::spawn(async move {
            let event = match me.0.me().await {
                Ok(user) => Event::Me(user),
                Err(error) => Event::Failed { what: "who am I".into(), error },
            };
            let _ = me.1.send(event);
        });
        tokio::spawn(async move {
            let mut search: Option<tokio::task::JoinHandle<()>> = None;
            while let Some(command) = rx.recv().await {
                let task = tokio::spawn(run(
                    provider.clone(),
                    cache.clone(),
                    command.clone(),
                    events.clone(),
                ));
                if matches!(command, Command::Search { .. })
                    && let Some(previous) = search.replace(task)
                {
                    previous.abort();
                }
            }
        });
        (service, events_rx)
    }

    /// A send fails only once the service task is gone, which happens only
    /// at shutdown; there is nobody left to tell.
    pub fn send(&self, command: Command) {
        let _ = self.commands.send(command);
    }

    pub fn capabilities(&self) -> &Capabilities {
        &self.capabilities
    }

    pub fn provider_name(&self) -> &str {
        &self.provider
    }
}

/// The cache key of a board: its id, or `first` for "the provider's first".
fn cache_key(id: &Option<BoardId>) -> String {
    id.as_ref().map_or("first".into(), |id| id.to_string())
}

async fn run(
    provider: Arc<dyn Provider>,
    cache: Option<Cache>,
    command: Command,
    events: mpsc::UnboundedSender<Event>,
) {
    let event = match command {
        Command::LoadBoard(id) => {
            let cached = cache.as_ref().and_then(|c| c.load(&cache_key(&id)));
            if let Some((snapshot, _)) = &cached {
                let _ =
                    events.send(Event::BoardLoaded { snapshot: snapshot.clone(), fresh: false });
            }
            fetch_board(provider.as_ref(), cache.as_ref(), id, cached).await
        }
        Command::ReloadBoard(id) => fetch_board(provider.as_ref(), cache.as_ref(), id, None).await,
        Command::LoadBoards => match provider.boards().await {
            Ok(boards) => Event::Boards(boards),
            Err(error) => Event::Failed { what: "listing the boards".into(), error },
        },
        Command::Search { id, query } => {
            let result = provider.search(&query, None).await.map(|page| page.items);
            Event::SearchResults { id, result }
        }
        Command::LoadIssue(key) => match load_issue(provider.as_ref(), &key).await {
            Ok(detail) => Event::IssueLoaded(Box::new(detail)),
            Err(error) => Event::Failed { what: format!("loading {key}"), error },
        },
    };
    if let Event::Failed { what, error } = &event {
        tracing::warn!("{what}: {error}");
    }
    let _ = events.send(event);
}

/// From the provider, synced from `cached` when there is one, and saved
/// back with the time the sync started: whatever changes while it runs is
/// caught by the next one.
async fn fetch_board(
    provider: &dyn Provider,
    cache: Option<&Cache>,
    id: Option<BoardId>,
    cached: Option<(BoardSnapshot, time::OffsetDateTime)>,
) -> Event {
    let key = cache_key(&id);
    let started = time::OffsetDateTime::now_utc();
    let result = match cached {
        Some((snapshot, since)) => sync_board(provider, snapshot, since).await,
        None => load_board(provider, id).await,
    };
    match result {
        Ok(snapshot) => {
            if let Some(cache) = cache
                && let Err(e) = cache.save(&key, &snapshot, started)
            {
                tracing::warn!("saving the cache in {}: {e}", cache.dir().display());
            }
            Event::BoardLoaded { snapshot, fresh: true }
        }
        Err(error) => Event::Failed { what: "loading the board".into(), error },
    }
}

async fn load_issue(provider: &dyn Provider, key: &IssueKey) -> kolum_domain::Result<IssueDetail> {
    let (issue, comments, activity) =
        tokio::try_join!(provider.issue(key), provider.comments(key), provider.activity(key))?;
    Ok(IssueDetail { issue, comments, activity })
}

#[cfg(test)]
mod tests {
    use super::*;
    use kolum_provider_fake::FakeProvider;

    #[tokio::test]
    async fn commands_come_back_as_events() {
        let (service, mut events) = Service::spawn(Arc::new(FakeProvider::new()), None);
        assert_eq!(service.provider_name(), "fake");
        let Some(Event::Me(me)) = events.recv().await else { panic!("expected Me first") };
        assert_eq!(me.display_name, "Colin Tenaguillo");

        service.send(Command::LoadBoard(None));
        let Some(Event::BoardLoaded { snapshot, fresh: true }) = events.recv().await else {
            panic!()
        };
        assert_eq!(snapshot.board.columns.len(), 5);
        service.send(Command::LoadBoards);
        let Some(Event::Boards(boards)) = events.recv().await else { panic!() };
        assert_eq!(boards.len(), 1);

        service.send(Command::LoadIssue(IssueKey::new("ACME-5")));
        let Some(Event::IssueLoaded(detail)) = events.recv().await else { panic!() };
        assert_eq!(detail.comments.len(), 3);
        assert!(!detail.activity.is_empty());
    }

    #[tokio::test]
    async fn a_failure_names_what_failed() {
        let (service, mut events) = Service::spawn(Arc::new(FakeProvider::new()), None);
        events.recv().await; // Me
        service.send(Command::LoadIssue(IssueKey::new("ACME-999")));
        let Some(Event::Failed { what, error }) = events.recv().await else { panic!() };
        assert_eq!(what, "loading ACME-999");
        assert!(matches!(error, DomainError::NotFound(_)));
    }

    async fn next_board(events: &mut mpsc::UnboundedReceiver<Event>) -> (BoardSnapshot, bool) {
        loop {
            match events.recv().await {
                Some(Event::BoardLoaded { snapshot, fresh }) => return (snapshot, fresh),
                Some(Event::Me(_)) => continue,
                other => panic!("expected a board, got {other:?}"),
            }
        }
    }

    #[tokio::test]
    async fn a_warm_start_shows_the_cache_then_the_sync() {
        use kolum_domain::{IssueUpdate, IssueWriter};
        let dir = crate::cache::tests::temp_dir("service-warm");
        let fake = Arc::new(FakeProvider::new());
        let (service, mut events) = Service::spawn(fake.clone(), Some(Cache::new(&dir)));
        service.send(Command::LoadBoard(None));
        let (cold, fresh) = next_board(&mut events).await;
        assert!(fresh, "nothing cached: straight from the provider");
        assert!(dir.join("first.json").exists());

        let key = IssueKey::new("ACME-12");
        fake.update(&key, IssueUpdate { title: Some("Renamed".into()), ..Default::default() })
            .await
            .unwrap();
        let (service, mut events) = Service::spawn(fake.clone(), Some(Cache::new(&dir)));
        service.send(Command::LoadBoard(None));
        let (warm, fresh) = next_board(&mut events).await;
        assert!(!fresh);
        assert_eq!(warm, cold, "the cache first, as it was");
        let (synced, fresh) = next_board(&mut events).await;
        assert!(fresh);
        assert_eq!(synced.issues.iter().find(|i| i.key == key).unwrap().title, "Renamed");

        service.send(Command::ReloadBoard(None));
        let (reloaded, fresh) = next_board(&mut events).await;
        assert!(fresh, "a reload skips the cache");
        assert_eq!(reloaded, synced);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn search_results_carry_their_request_id() {
        use kolum_domain::{Filter, UserSel};
        let (service, mut events) = Service::spawn(Arc::new(FakeProvider::new()), None);
        events.recv().await; // Me
        let mine = Query::Filter(Filter { assignee: Some(UserSel::Me), ..Default::default() });
        service.send(Command::Search { id: 7, query: mine });
        let Some(Event::SearchResults { id: 7, result: Ok(issues) }) = events.recv().await else {
            panic!()
        };
        assert!(!issues.is_empty());
        service.send(Command::Search { id: 8, query: Query::Native("x".into()) });
        let Some(Event::SearchResults { id: 8, result: Err(DomainError::Unsupported(_)) }) =
            events.recv().await
        else {
            panic!()
        };
    }
}
