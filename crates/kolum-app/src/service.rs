use crate::board::{BoardSnapshot, load_board, sync_board};
use crate::cache::Cache;
use crate::write::Write;
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
    /// The board's running and coming sprints.
    Sprints(kolum_domain::BoardId),
    LoadIssue(IssueKey),
    /// A search for the picker. A new one cancels the one before: only the
    /// latest query's results matter.
    Search {
        id: u64,
        query: Query,
    },
    /// The transitions open to an issue now.
    Transitions(IssueKey),
    /// The fields the issue lets kolum set.
    EditableFields(IssueKey),
    /// An avatar's bytes.
    Image(String),
    Create(kolum_domain::NewIssue),
    Clone(IssueKey),
    /// A change, under the id the optimistic ledger gave it.
    Write {
        op: u64,
        key: IssueKey,
        write: Write,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct IssueDetail {
    pub issue: Issue,
    pub comments: Vec<Comment>,
    pub activity: Vec<ActivityEntry>,
    /// How the provider's own issue view lays the fields out, if it says.
    pub layout: Option<kolum_domain::FieldLayout>,
    /// The issues whose parent it is, beyond its sub-tasks.
    pub children: Vec<kolum_domain::IssueRef>,
}

/// The project and the issue type an issue's field layout belongs to:
/// every issue of a type in a project has the same.
pub fn layout_key(issue: &Issue) -> String {
    let project = issue.key.as_str().split(['-', '#']).next().unwrap_or_default();
    format!("{project}/{}", issue.kind.name)
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
    /// The field layouts kept from earlier runs, by `layout_key`, first of all.
    FieldLayouts(std::collections::BTreeMap<String, kolum_domain::FieldLayout>),
    Sprints(Result<Vec<kolum_domain::Sprint>, DomainError>),
    IssueLoaded(Box<IssueDetail>),
    /// For the search with this id; stale ones were cancelled.
    SearchResults {
        id: u64,
        result: Result<Vec<Issue>, DomainError>,
    },
    Transitions {
        key: IssueKey,
        result: Result<Vec<kolum_domain::Transition>, DomainError>,
    },
    EditableFields {
        key: IssueKey,
        result: Result<Vec<kolum_domain::EditableField>, DomainError>,
    },
    /// The write went through; `issue` is the provider's view of it after,
    /// when it could be read back.
    Written {
        op: u64,
        key: IssueKey,
        issue: Option<Box<Issue>>,
    },
    WriteFailed {
        op: u64,
        key: IssueKey,
        error: DomainError,
    },
    Image {
        url: String,
        result: Result<Vec<u8>, DomainError>,
    },
    /// A new issue, made by Create or Clone.
    Created(Box<Issue>),
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
        let kept = cache.as_ref().map(Cache::layouts).unwrap_or_default();
        if !kept.is_empty() {
            let _ = events.send(Event::FieldLayouts(kept));
        }
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
        Command::Sprints(board) => Event::Sprints(provider.sprints(&board).await),
        Command::LoadBoards => match provider.boards().await {
            Ok(boards) => Event::Boards(boards),
            Err(error) => Event::Failed { what: "listing the boards".into(), error },
        },
        Command::Transitions(key) => {
            let result = provider.transitions(&key).await;
            Event::Transitions { key, result }
        }
        Command::EditableFields(key) => {
            let result = provider.editable_fields(&key).await;
            Event::EditableFields { key, result }
        }
        Command::Write { op, key, write } => match send_write(provider.as_ref(), &key, write).await
        {
            // Read back what the provider made of it: the board then shows
            // its truth, not our guess.
            Ok(()) => {
                Event::Written { op, issue: provider.issue(&key).await.ok().map(Box::new), key }
            }
            Err(error) => Event::WriteFailed { op, key, error },
        },
        Command::Image(url) => {
            let result = provider.image(&url).await;
            Event::Image { url, result }
        }
        Command::Create(new) => match provider.create(new).await {
            Ok(issue) => Event::Created(Box::new(issue)),
            Err(error) => Event::Failed { what: "creating the issue".into(), error },
        },
        Command::Clone(key) => match provider.clone_issue(&key).await {
            Ok(issue) => Event::Created(Box::new(issue)),
            Err(error) => Event::Failed { what: format!("cloning {key}"), error },
        },
        Command::Search { id, query } => {
            let result = provider.search(&query, None).await.map(|page| page.items);
            Event::SearchResults { id, result }
        }
        Command::LoadIssue(key) => match load_issue(provider.as_ref(), &key).await {
            Ok(detail) => {
                if let (Some(cache), Some(layout)) = (&cache, &detail.layout)
                    && let Err(e) = cache.save_layout(&layout_key(&detail.issue), layout)
                {
                    tracing::warn!("keeping the field layout of {key}: {e}");
                }
                Event::IssueLoaded(Box::new(detail))
            }
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

async fn send_write(
    provider: &dyn Provider,
    key: &IssueKey,
    write: Write,
) -> kolum_domain::Result<()> {
    match write {
        Write::Transition(t) => provider.transition(key, &t).await,
        Write::Assign(user) => provider.assign(key, user.as_ref()).await,
        Write::Comment(body) => provider.comment(key, body).await.map(|_| ()),
        Write::Update(update) => provider.update(key, update).await,
        Write::Rank(rank) => provider.rank(key, &rank).await,
    }
}

async fn load_issue(provider: &dyn Provider, key: &IssueKey) -> kolum_domain::Result<IssueDetail> {
    let (detail, layout, children) = tokio::join!(
        async {
            tokio::try_join!(provider.issue(key), provider.comments(key), provider.activity(key))
        },
        provider.field_layout(key),
        provider.children(key),
    );
    let (issue, comments, activity) = detail?;
    // Niceties: without them the issue still opens, with what it has.
    let layout = nicety(layout, "the field layout", key);
    let children = nicety(children, "the children", key).unwrap_or_default();
    Ok(IssueDetail { issue, comments, activity, layout, children })
}

/// What a provider cannot say is no news; what fails is logged.
fn nicety<T>(result: kolum_domain::Result<T>, what: &str, key: &IssueKey) -> Option<T> {
    if let Err(e) = &result
        && !matches!(e, DomainError::Unsupported(_))
    {
        tracing::warn!("{what} of {key}: {e}");
    }
    result.ok()
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
        let layout = detail.layout.as_ref().expect("the fake has a layout");
        assert!(detail.children.is_empty(), "ACME-5 is no epic");
        service.send(Command::LoadIssue(IssueKey::new("ACME-1")));
        let Some(Event::IssueLoaded(epic)) = events.recv().await else { panic!() };
        assert_eq!(epic.children.len(), 6, "an epic comes with its children");
        assert_eq!(layout.body[0].as_str(), "description");
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
        let mine = Query::Filter(Filter { assignees: vec![UserSel::Me], ..Default::default() });
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

    #[tokio::test]
    async fn a_layout_is_the_project_and_the_type_of_an_issue() {
        use kolum_domain::IssueReader;
        let mut issue = FakeProvider::new().issue(&IssueKey::new("ACME-12")).await.unwrap();
        issue.kind.name = "Bug".into();
        assert_eq!(layout_key(&issue), "ACME/Bug");
    }

    #[tokio::test]
    async fn an_issue_layout_is_kept_for_the_next_run() {
        let dir = crate::cache::tests::temp_dir("service-layouts");
        let fake = Arc::new(FakeProvider::new());
        let (service, mut events) = Service::spawn(fake.clone(), Some(Cache::new(&dir)));
        events.recv().await; // Me
        service.send(Command::LoadIssue(IssueKey::new("ACME-5")));
        let Some(Event::IssueLoaded(detail)) = events.recv().await else { panic!() };
        let (_again, mut events) = Service::spawn(fake, Some(Cache::new(&dir)));
        let Some(Event::FieldLayouts(layouts)) = events.recv().await else {
            panic!("the layouts come first")
        };
        assert_eq!(layouts.get(&layout_key(&detail.issue)), detail.layout.as_ref());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn the_fields_an_issue_lets_set_come_back_for_it() {
        let (service, mut events) = Service::spawn(Arc::new(FakeProvider::new()), None);
        events.recv().await; // Me
        let key = IssueKey::new("ACME-9");
        service.send(Command::EditableFields(key.clone()));
        let Some(Event::EditableFields { key: k, result: Ok(fields) }) = events.recv().await else {
            panic!()
        };
        assert_eq!(k, key);
        assert!(fields.iter().any(|f| f.name == "Team"));
    }

    #[tokio::test]
    async fn a_write_comes_back_with_the_issue_as_the_provider_has_it() {
        let (service, mut events) = Service::spawn(Arc::new(FakeProvider::new()), None);
        events.recv().await; // Me
        let key = IssueKey::new("ACME-9");
        service.send(Command::Transitions(key.clone()));
        let Some(Event::Transitions { result: Ok(ts), .. }) = events.recv().await else { panic!() };
        let to_done = ts.iter().find(|t| t.name == "Done").unwrap().id.clone();
        service.send(Command::Write { op: 1, key: key.clone(), write: Write::Transition(to_done) });
        let Some(Event::Written { op: 1, issue: Some(issue), .. }) = events.recv().await else {
            panic!()
        };
        assert_eq!(issue.status.name, "Done");
        let hi = Write::Comment(kolum_domain::RichText::from_markdown("hi"));
        service.send(Command::Write { op: 2, key: key.clone(), write: hi });
        assert!(matches!(events.recv().await, Some(Event::Written { op: 2, .. })));
        let nobody = Write::Assign(Some(kolum_domain::UserId::new("nobody")));
        service.send(Command::Write { op: 3, key, write: nobody });
        let Some(Event::WriteFailed { op: 3, error: DomainError::NotFound(_), .. }) =
            events.recv().await
        else {
            panic!()
        };
    }

    #[tokio::test]
    async fn create_and_clone_answer_with_the_new_issue() {
        let (service, mut events) = Service::spawn(Arc::new(FakeProvider::new()), None);
        events.recv().await; // Me
        service.send(Command::Clone(IssueKey::new("ACME-5")));
        let Some(Event::Created(clone)) = events.recv().await else { panic!() };
        assert_eq!(clone.title, "CLONE - Login times out after SSO redirect");
        let kind =
            kolum_domain::IssueType { name: "Task".into(), kind: kolum_domain::IssueKind::Task };
        let new = kolum_domain::NewIssue {
            project: "ACME".into(),
            kind,
            title: "T".into(),
            description: Default::default(),
            parent: None,
        };
        service.send(Command::Create(new));
        let Some(Event::Created(created)) = events.recv().await else { panic!() };
        assert_eq!(created.title, "T");
        service.send(Command::Clone(IssueKey::new("NOPE-1")));
        assert!(
            matches!(events.recv().await, Some(Event::Failed { what, .. }) if what == "cloning NOPE-1")
        );
    }

    #[tokio::test]
    async fn an_avatar_comes_back_by_its_url() {
        let (service, mut events) = Service::spawn(Arc::new(FakeProvider::new()), None);
        let Some(Event::Me(me)) = events.recv().await else { panic!() };
        let url = me.avatar_url.unwrap();
        service.send(Command::Image(url.clone()));
        let Some(Event::Image { url: back, result: Ok(bytes) }) = events.recv().await else {
            panic!()
        };
        assert_eq!(back, url);
        assert!(bytes.starts_with(b"P6"));
    }
}
