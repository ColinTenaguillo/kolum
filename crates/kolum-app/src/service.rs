use crate::board::{BoardSnapshot, load_board, sync_board};
use crate::cache::Cache;
use crate::write::Write;
use kolum_domain::{
    ActivityEntry, BULK_READ_MAX, BoardId, BoardSummary, Capabilities, Capability, Comment,
    DomainError, Issue, IssueInFull, IssueKey, Provider, Query, User,
};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use time::OffsetDateTime;
use tokio::sync::mpsc;

/// What the TUI asks for.
#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    /// The cached board at once if there is one, then synced. `None` opens
    /// the provider's first board. The number of each board request comes
    /// back on its [`Event::BoardLoaded`].
    LoadBoard(Option<BoardId>, u64),
    /// Everything again, ignoring the cache: what `r` does.
    ReloadBoard(Option<BoardId>, u64),
    LoadBoards,
    /// What changed since the last sync, from the cached board when there
    /// is one, which is not shown again first.
    SyncBoard(Option<BoardId>, u64),
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
    /// A change, under the id [`crate::Pending`] gave it.
    Write {
        op: u64,
        key: IssueKey,
        write: Write,
    },
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct IssueDetail {
    pub issue: Issue,
    pub comments: Vec<Comment>,
    pub activity: Vec<ActivityEntry>,
    /// How the provider's own issue view lays the fields out, if it says.
    pub layout: Option<kolum_domain::FieldLayout>,
    /// The issues whose parent it is, beyond its sub-tasks.
    pub children: Vec<kolum_domain::IssueRef>,
}

/// What a bulk read gives: no layout (kept by issue type) and no children,
/// which its opening reads.
impl From<IssueInFull> for IssueDetail {
    fn from(read: IssueInFull) -> Self {
        IssueDetail {
            issue: read.issue,
            comments: read.comments,
            activity: read.activity,
            layout: None,
            children: vec![],
        }
    }
}

/// The background read of a board's issues in full, so each opens whole:
/// shared by the commands, which run side by side.
#[derive(Default)]
struct Prefetch {
    /// When each issue kept was last updated, as kept: one updated since
    /// is read again.
    kept: Mutex<HashMap<IssueKey, OffsetDateTime>>,
    /// Set by a refusal (a rate limit) or a failure: none for the rest of
    /// the run, so that the user's own requests pass.
    stopped: AtomicBool,
    /// One at a time; what changes meanwhile is caught after the next sync.
    running: AtomicBool,
    /// Set once the issues of earlier runs are read: what is kept is known.
    loaded: tokio::sync::watch::Sender<bool>,
}

impl Prefetch {
    fn keep(&self, issue: &Issue) {
        self.kept.lock().unwrap().insert(issue.key.clone(), issue.updated);
    }
}

/// The project and the issue type an issue's field layout belongs to:
/// every issue of a type in a project has the same. A key in no project
/// (a GitHub draft) has an empty one, which no project's name is.
pub fn layout_key(issue: &Issue) -> String {
    format!("{}/{}", issue.key.project().unwrap_or_default(), issue.kind.name)
}

/// What comes back, in whatever order the requests finish.
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    Me(User),
    /// `fresh` is false for the cached copy that arrives before the sync.
    /// `request` is the number of the board request it answers.
    BoardLoaded {
        snapshot: BoardSnapshot,
        fresh: bool,
        request: u64,
    },
    Boards(Vec<BoardSummary>),
    /// The field layouts kept from earlier runs, by `layout_key`, first of all.
    FieldLayouts(std::collections::BTreeMap<String, kolum_domain::FieldLayout>),
    /// Issues read in full: those of earlier runs, right after the layouts,
    /// then those a board prefetches.
    ReadIssues(Vec<IssueDetail>),
    Sprints(Result<Vec<kolum_domain::Sprint>, DomainError>),
    IssueLoaded(Box<IssueDetail>),
    /// The `LoadIssue` of this issue failed.
    IssueLoadFailed {
        key: IssueKey,
        error: DomainError,
    },
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
        let prefetch = Arc::new(Prefetch::default());
        // Every issue kept, read off the way of the board's first showing.
        let (kept, sent, state) = (cache.clone(), events.clone(), prefetch.clone());
        tokio::task::spawn_blocking(move || {
            let read = kept.as_ref().map(Cache::issues).unwrap_or_default();
            read.iter().for_each(|d| state.keep(&d.issue));
            if !read.is_empty() {
                let _ = sent.send(Event::ReadIssues(read));
            }
            state.loaded.send_replace(true);
        });
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
            let mut turns = Turns::default();
            while let Some(command) = rx.recv().await {
                let job = run(
                    provider.clone(),
                    cache.clone(),
                    command.clone(),
                    events.clone(),
                    prefetch.clone(),
                );
                let task = turns.spawn(&command, job);
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

/// Writes to one issue run one after the other, in the order sent, so
/// their replies come back in that order: the last one shown is the last
/// asked. Writes to other issues run meanwhile.
// One entry stays per issue ever written, a few bytes each: nothing to
// prune in a run of a few hundred writes. Drop an entry once its turn is
// over if runs ever write to many thousands of issues.
#[derive(Default)]
struct Turns(HashMap<IssueKey, tokio::sync::oneshot::Receiver<()>>);

impl Turns {
    /// The command's job on its own task; a write's waits for the write
    /// before it to the same issue.
    fn spawn(
        &mut self,
        command: &Command,
        job: impl std::future::Future<Output = ()> + Send + 'static,
    ) -> tokio::task::JoinHandle<()> {
        let Command::Write { key, .. } = command else { return tokio::spawn(job) };
        // This turn ends when `mine` is dropped, done or aborted.
        let (mine, next) = tokio::sync::oneshot::channel::<()>();
        let before = self.0.insert(key.clone(), next);
        tokio::spawn(async move {
            if let Some(before) = before {
                let _ = before.await;
            }
            job.await;
            drop(mine);
        })
    }
}

/// How long a write may take before it counts as refused: two HTTP
/// requests (a transition looked up, then made) of 30 s each. It also
/// ends the issue's turn, so a provider that never answers holds up no
/// later write to it.
const WRITE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);

/// The request's own result, or a transport error once `limit` passed.
async fn within<T>(
    limit: std::time::Duration,
    request: impl std::future::Future<Output = kolum_domain::Result<T>>,
) -> kolum_domain::Result<T> {
    let late = |_| DomainError::Transport(format!("no answer in {} s", limit.as_secs()));
    tokio::time::timeout(limit, request).await.map_err(late)?
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
    prefetched: Arc<Prefetch>,
) {
    let event = match command {
        Command::LoadBoard(id, request) => {
            // The cache first, then the whole board: a board filtered by date
            // loses issues nobody touched, which no incremental sync reports.
            if let Some((snapshot, _)) = cache.as_ref().and_then(|c| c.load(&cache_key(&id))) {
                let _ = events.send(Event::BoardLoaded { snapshot, fresh: false, request });
            }
            fetch_board(provider.as_ref(), cache.as_ref(), (id, request), None).await
        }
        Command::ReloadBoard(id, request) => {
            fetch_board(provider.as_ref(), cache.as_ref(), (id, request), None).await
        }
        Command::SyncBoard(id, request) => {
            let cached = cache.as_ref().and_then(|c| c.load(&cache_key(&id)));
            fetch_board(provider.as_ref(), cache.as_ref(), (id, request), cached).await
        }
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
        Command::Write { op, key, write } => written(provider.as_ref(), op, key, write).await,
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
                prefetched.keep(&detail.issue);
                if let Some(cache) = &cache
                    && let Err(e) = cache.save_issue(&detail)
                {
                    tracing::warn!("keeping {key}: {e}");
                }
                Event::IssueLoaded(Box::new(detail))
            }
            Err(error) => {
                tracing::warn!("loading {key}: {error}");
                Event::IssueLoadFailed { key, error }
            }
        },
    };
    if let Event::Failed { what, error } = &event {
        tracing::warn!("{what}: {error}");
    }
    if let Event::BoardLoaded { snapshot, fresh: true, .. } = &event {
        start_prefetch(&provider, &cache, snapshot, &events, prefetched);
    }
    let _ = events.send(event);
}

/// A board just read from the provider: its issues read in full in the
/// background, if the provider can.
fn start_prefetch(
    provider: &Arc<dyn Provider>,
    cache: &Option<Cache>,
    snapshot: &BoardSnapshot,
    events: &mpsc::UnboundedSender<Event>,
    state: Arc<Prefetch>,
) {
    if provider.capabilities().supports(Capability::BulkRead) {
        let wanted = snapshot.issues.iter().map(|i| (i.key.clone(), i.updated)).collect();
        tokio::spawn(prefetch(provider.clone(), cache.clone(), wanted, events.clone(), state));
    }
}

/// The issues of `wanted` not kept as last updated, read in full a batch
/// at a time, each kept and sent on as it comes.
async fn prefetch(
    provider: Arc<dyn Provider>,
    cache: Option<Cache>,
    wanted: Vec<(IssueKey, OffsetDateTime)>,
    events: mpsc::UnboundedSender<Event>,
    state: Arc<Prefetch>,
) {
    let _ = state.loaded.subscribe().wait_for(|loaded| *loaded).await;
    if state.stopped.load(Ordering::Relaxed) || state.running.swap(true, Ordering::AcqRel) {
        return;
    }
    let stale: Vec<IssueKey> = {
        let kept = state.kept.lock().unwrap();
        wanted.into_iter().filter(|(k, at)| kept.get(k) != Some(at)).map(|(k, _)| k).collect()
    };
    for batch in stale.chunks(BULK_READ_MAX) {
        match provider.issues_in_full(batch).await {
            Ok(read) => {
                let mut read: Vec<IssueDetail> = read.into_iter().map(IssueDetail::from).collect();
                for detail in &mut read {
                    // A bulk read has no children nor layout: an opening's stay.
                    if let Some(kept) = cache.as_ref().and_then(|c| c.issue(&detail.issue.key)) {
                        detail.children = kept.children;
                        detail.layout = kept.layout;
                    }
                    state.keep(&detail.issue);
                    if let Some(cache) = &cache
                        && let Err(e) = cache.save_issue(detail)
                    {
                        tracing::warn!("keeping {}: {e}", detail.issue.key);
                    }
                }
                let _ = events.send(Event::ReadIssues(read));
            }
            Err(error) => {
                tracing::info!("prefetching stopped for this run: {error}");
                state.stopped.store(true, Ordering::Relaxed);
                break;
            }
        }
    }
    state.running.store(false, Ordering::Release);
}

/// From the provider, synced from `cached` when there is one, and saved
/// back with the time the sync started: whatever changes while it runs is
/// caught by the next one.
async fn fetch_board(
    provider: &dyn Provider,
    cache: Option<&Cache>,
    (id, request): (Option<BoardId>, u64),
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
            Event::BoardLoaded { snapshot, fresh: true, request }
        }
        Err(error) => Event::Failed { what: "loading the board".into(), error },
    }
}

/// The write sent, then read back: the board then shows the provider's
/// truth, not our guess.
async fn written(provider: &dyn Provider, op: u64, key: IssueKey, write: Write) -> Event {
    match within(WRITE_TIMEOUT, send_write(provider, &key, write)).await {
        Ok(()) => Event::Written { op, issue: provider.issue(&key).await.ok().map(Box::new), key },
        Err(error) => Event::WriteFailed { op, key, error },
    }
}

async fn send_write(
    provider: &dyn Provider,
    key: &IssueKey,
    write: Write,
) -> kolum_domain::Result<()> {
    match write {
        Write::Transition(t) => provider.transition(key, &t).await,
        Write::TransitionTo { id, to } => {
            let id = crate::write::transition_to(&provider.transitions(key).await?, &id, &to)?;
            provider.transition(key, &id).await
        }
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

        service.send(Command::LoadBoard(None, 7));
        let Some(Event::BoardLoaded { snapshot, fresh: true, request: 7 }) =
            next(&mut events).await
        else {
            panic!("the board, answering request 7")
        };
        assert_eq!(snapshot.board.columns.len(), 5);
        service.send(Command::LoadBoards);
        let Some(Event::Boards(boards)) = next(&mut events).await else { panic!() };
        assert_eq!(boards.len(), 1);

        service.send(Command::LoadIssue(IssueKey::parse("ACME-5")));
        let Some(Event::IssueLoaded(detail)) = next(&mut events).await else { panic!() };
        assert_eq!(detail.comments.len(), 3);
        let layout = detail.layout.as_ref().expect("the fake has a layout");
        assert!(detail.children.is_empty(), "ACME-5 is no epic");
        service.send(Command::LoadIssue(IssueKey::parse("ACME-1")));
        let Some(Event::IssueLoaded(epic)) = next(&mut events).await else { panic!() };
        assert_eq!(epic.children.len(), 6, "an epic comes with its children");
        assert_eq!(layout.body[0].as_str(), "description");
        assert!(!detail.activity.is_empty());
    }

    #[tokio::test]
    async fn a_failure_names_what_failed() {
        let (service, mut events) = Service::spawn(Arc::new(FakeProvider::new()), None);
        events.recv().await; // Me
        service.send(Command::LoadIssue(IssueKey::parse("ACME-999")));
        let Some(Event::IssueLoadFailed { key, error }) = events.recv().await else { panic!() };
        assert_eq!(key, IssueKey::parse("ACME-999"));
        assert!(matches!(error, DomainError::NotFound(_)));
    }

    /// Writes to one issue run in the order sent, the first slower or not:
    /// their replies come back in that order, and the issue ends as last
    /// asked. Another issue's write goes on meanwhile.
    #[tokio::test]
    async fn writes_to_one_issue_run_in_turn() {
        use kolum_domain::{IssueReader, TransitionId};
        let fake = Arc::new(FakeProvider::new());
        fake.delay_next_transition(std::time::Duration::from_millis(200));
        let (service, mut events) = Service::spawn(fake.clone(), None);
        let to = |op, key: &str, status: &str| Command::Write {
            op,
            key: IssueKey::parse(key),
            write: Write::Transition(TransitionId::new(format!("to-{status}"))),
        };
        service.send(to(1, "ACME-12", "review"));
        service.send(to(2, "ACME-12", "done"));
        service.send(to(3, "ACME-5", "done"));
        let mut written = vec![];
        while written.len() < 3 {
            match next(&mut events).await {
                Some(Event::Written { op, .. }) => written.push(op),
                Some(Event::Me(_)) => {}
                other => panic!("{other:?}"),
            }
        }
        assert_eq!(written, [3, 1, 2]);
        let issue = fake.issue(&IssueKey::parse("ACME-12")).await.unwrap();
        assert_eq!(issue.status.name, "Done", "the last asked");
    }

    /// A provider that never answers a write fails it in time, and the
    /// next write to the issue goes on.
    #[tokio::test(start_paused = true)]
    async fn a_write_with_no_answer_fails_and_the_next_one_goes_on() {
        use kolum_domain::TransitionId;
        let fake = Arc::new(FakeProvider::new());
        fake.delay_next_transition(std::time::Duration::from_secs(3600));
        let (service, mut events) = Service::spawn(fake, None);
        let to = |op, status: &str| Command::Write {
            op,
            key: IssueKey::parse("ACME-12"),
            write: Write::Transition(TransitionId::new(format!("to-{status}"))),
        };
        service.send(to(1, "review"));
        service.send(to(2, "done"));
        let mut answers = vec![];
        while answers.len() < 2 {
            match next(&mut events).await {
                Some(Event::WriteFailed { op, error, .. }) => answers.push((op, error.to_string())),
                Some(Event::Written { op, .. }) => answers.push((op, "saved".into())),
                Some(Event::Me(_)) => {}
                other => panic!("{other:?}"),
            }
        }
        assert_eq!(answers[1], (2, "saved".into()));
        assert_eq!(answers[0].0, 1);
        assert!(answers[0].1.contains("no answer"), "{}", answers[0].1);
    }

    /// The next event but the issues a board prefetches, which come when
    /// they come.
    async fn next(events: &mut mpsc::UnboundedReceiver<Event>) -> Option<Event> {
        loop {
            match events.recv().await {
                Some(Event::ReadIssues(_)) => continue,
                other => return other,
            }
        }
    }

    async fn next_board(events: &mut mpsc::UnboundedReceiver<Event>) -> (BoardSnapshot, bool) {
        loop {
            match next(events).await {
                Some(Event::BoardLoaded { snapshot, fresh, .. }) => return (snapshot, fresh),
                Some(Event::Me(_)) => continue,
                other => panic!("expected a board, got {other:?}"),
            }
        }
    }

    /// The periodic sync starts from the cache but does not show it again:
    /// only what came of the sync.
    #[tokio::test]
    async fn a_sync_brings_only_what_changed_without_the_cache_first() {
        use kolum_domain::{IssueUpdate, IssueWriter};
        let dir = crate::cache::tests::temp_dir("service-sync");
        let fake = Arc::new(FakeProvider::new());
        let (service, mut events) = Service::spawn(fake.clone(), Some(Cache::new(&dir)));
        service.send(Command::LoadBoard(None, 1));
        next_board(&mut events).await;
        let key = IssueKey::parse("ACME-12");
        let update = IssueUpdate { title: Some("By an automation".into()), ..Default::default() };
        fake.update(&key, update).await.unwrap();
        service.send(Command::SyncBoard(None, 1));
        let (synced, fresh) = next_board(&mut events).await;
        assert!(fresh, "straight to the synced board, no cached one before it");
        let title = synced.issues.iter().find(|i| i.key == key).map(|i| i.title.as_str());
        assert_eq!(title, Some("By an automation"));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn a_warm_start_shows_the_cache_then_the_sync() {
        use kolum_domain::{IssueUpdate, IssueWriter};
        let dir = crate::cache::tests::temp_dir("service-warm");
        let fake = Arc::new(FakeProvider::new());
        let (service, mut events) = Service::spawn(fake.clone(), Some(Cache::new(&dir)));
        service.send(Command::LoadBoard(None, 1));
        let (cold, fresh) = next_board(&mut events).await;
        assert!(fresh, "nothing cached: straight from the provider");
        assert!(dir.join("first.json").exists());

        let key = IssueKey::parse("ACME-12");
        fake.update(&key, IssueUpdate { title: Some("Renamed".into()), ..Default::default() })
            .await
            .unwrap();
        let (service, mut events) = Service::spawn(fake.clone(), Some(Cache::new(&dir)));
        service.send(Command::LoadBoard(None, 1));
        let (warm, fresh) = next_board(&mut events).await;
        assert!(!fresh);
        assert_eq!(warm, cold, "the cache first, as it was");
        let (synced, fresh) = next_board(&mut events).await;
        assert!(fresh);
        assert_eq!(synced.issues.iter().find(|i| i.key == key).unwrap().title, "Renamed");

        service.send(Command::ReloadBoard(None, 1));
        let (reloaded, fresh) = next_board(&mut events).await;
        assert!(fresh, "a reload skips the cache");
        assert_eq!(reloaded, synced);
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// A board filtered by date (completed issues older than a week
    /// hidden) loses issues nobody touched, which no incremental sync
    /// reports: a warm start reads the whole board again.
    #[tokio::test]
    async fn a_warm_start_drops_what_left_the_board_since() {
        let dir = crate::cache::tests::temp_dir("service-left");
        let fake = Arc::new(FakeProvider::new());
        let mut cached = load_board(fake.as_ref(), None).await.unwrap();
        let mut gone = cached.issues[0].clone();
        gone.key = IssueKey::parse("ACME-999");
        cached.issues.push(gone);
        Cache::new(&dir).save("first", &cached, time::OffsetDateTime::now_utc()).unwrap();
        let (service, mut events) = Service::spawn(fake, Some(Cache::new(&dir)));
        service.send(Command::LoadBoard(None, 1));
        let has_gone = |b: &BoardSnapshot| b.issues.iter().any(|i| i.key.as_str() == "ACME-999");
        let (warm, _) = next_board(&mut events).await;
        assert!(has_gone(&warm), "the cache first, as it was");
        let (loaded, fresh) = next_board(&mut events).await;
        assert!(fresh && !has_gone(&loaded), "then the board as it is");
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
        let mut issue = FakeProvider::new().issue(&IssueKey::parse("ACME-12")).await.unwrap();
        issue.kind.name = "Bug".into();
        assert_eq!(layout_key(&issue), "ACME/Bug");
        issue.key = IssueKey::parse("python-language-server#199");
        assert_eq!(layout_key(&issue), "python-language-server/Bug", "a repository's own dashes");
    }

    #[tokio::test]
    async fn an_issue_layout_is_kept_for_the_next_run() {
        let dir = crate::cache::tests::temp_dir("service-layouts");
        let fake = Arc::new(FakeProvider::new());
        let (service, mut events) = Service::spawn(fake.clone(), Some(Cache::new(&dir)));
        events.recv().await; // Me
        service.send(Command::LoadIssue(IssueKey::parse("ACME-5")));
        let Some(Event::IssueLoaded(detail)) = events.recv().await else { panic!() };
        let (_again, mut events) = Service::spawn(fake, Some(Cache::new(&dir)));
        let Some(Event::FieldLayouts(layouts)) = events.recv().await else {
            panic!("the layouts come first")
        };
        assert_eq!(layouts.get(&layout_key(&detail.issue)), detail.layout.as_ref());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn an_issue_read_is_kept_for_the_next_run() {
        let dir = crate::cache::tests::temp_dir("service-issues");
        let fake = Arc::new(FakeProvider::new());
        let (service, mut events) = Service::spawn(fake.clone(), Some(Cache::new(&dir)));
        events.recv().await; // Me
        service.send(Command::LoadIssue(IssueKey::parse("ACME-5")));
        let Some(Event::IssueLoaded(detail)) = events.recv().await else { panic!() };
        let (_again, mut events) = Service::spawn(fake, Some(Cache::new(&dir)));
        let read = loop {
            match events.recv().await {
                Some(Event::ReadIssues(read)) => break read,
                Some(Event::FieldLayouts(_) | Event::Me(_)) => continue,
                other => panic!("the issues read, not {other:?}"),
            }
        };
        assert_eq!(read, [*detail]);
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// The next `ReadIssues`, or `None` when none comes in a moment.
    async fn next_read(events: &mut mpsc::UnboundedReceiver<Event>) -> Option<Vec<IssueDetail>> {
        let wait = std::time::Duration::from_millis(300);
        loop {
            match tokio::time::timeout(wait, events.recv()).await {
                Ok(Some(Event::ReadIssues(read))) => return Some(read),
                Ok(Some(_)) => continue,
                _ => return None,
            }
        }
    }

    #[tokio::test]
    async fn a_board_prefetches_its_issues_in_full_then_only_what_changed() {
        use kolum_domain::{IssueUpdate, IssueWriter};
        let dir = crate::cache::tests::temp_dir("service-prefetch");
        let fake = Arc::new(FakeProvider::new());
        let (service, mut events) = Service::spawn(fake.clone(), Some(Cache::new(&dir)));
        service.send(Command::LoadBoard(None, 1));
        let read = next_read(&mut events).await.expect("the board's issues, read in full");
        let board = load_board(fake.as_ref(), None).await.unwrap();
        assert_eq!(read.len(), board.issues.len());
        let five = read.iter().find(|d| d.issue.key.as_str() == "ACME-5").unwrap();
        assert!(
            !five.comments.is_empty() && !five.activity.is_empty(),
            "with comments and history"
        );
        assert_eq!(Cache::new(&dir).issues().len(), board.issues.len(), "kept for the next run");

        service.send(Command::SyncBoard(None, 1));
        assert_eq!(next_read(&mut events).await, None, "nothing changed, nothing read");

        let key = IssueKey::parse("ACME-12");
        fake.update(&key, IssueUpdate { title: Some("Renamed".into()), ..Default::default() })
            .await
            .unwrap();
        service.send(Command::SyncBoard(None, 1));
        let read = next_read(&mut events).await.expect("what changed");
        assert_eq!(read.iter().map(|d| d.issue.key.clone()).collect::<Vec<_>>(), [key]);

        // The next run starts from what is kept: nothing to read again.
        let (service, mut events) = Service::spawn(fake.clone(), Some(Cache::new(&dir)));
        service.send(Command::LoadBoard(None, 1));
        let kept = next_read(&mut events).await.expect("what the last run kept");
        assert_eq!(kept.len(), board.issues.len());
        assert_eq!(next_read(&mut events).await, None, "and no prefetch over it");
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// A prefetch reads no children nor layout: those an opening kept stay.
    #[tokio::test]
    async fn a_prefetch_keeps_the_children_an_opening_read() {
        use kolum_domain::IssueReader;
        let dir = crate::cache::tests::temp_dir("service-children");
        let fake = Arc::new(FakeProvider::new());
        // ACME-5 as an earlier opening kept it, since updated.
        let key = IssueKey::parse("ACME-5");
        let mut issue = fake.issue(&key).await.unwrap();
        issue.updated -= time::Duration::days(1);
        let child = fake.issue(&IssueKey::parse("ACME-6")).await.unwrap().as_ref();
        let opened = IssueDetail {
            issue,
            comments: vec![],
            activity: vec![],
            layout: Some(fake.field_layout(&key).await.unwrap()),
            children: vec![child],
        };
        Cache::new(&dir).save_issue(&opened).unwrap();
        let (service, mut events) = Service::spawn(fake.clone(), Some(Cache::new(&dir)));
        next_read(&mut events).await.expect("what the last run kept");
        service.send(Command::LoadBoard(None, 1));
        let read = next_read(&mut events).await.expect("the board read in full");
        let prefetched = read.iter().find(|d| d.issue.key == key).unwrap();
        assert!(prefetched.issue.updated > opened.issue.updated, "read again");
        assert_eq!(prefetched.children, opened.children);
        assert_eq!(prefetched.layout, opened.layout);
        let kept = Cache::new(&dir).issues();
        let kept = kept.iter().find(|d| d.issue.key == key).unwrap();
        assert_eq!(kept.children, opened.children, "on disk too");
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// A refusal, a rate limit most likely, stops the prefetch for the run:
    /// the user's own requests come first.
    #[tokio::test]
    async fn a_failed_prefetch_stops_for_the_run() {
        let fake: Arc<dyn Provider> = Arc::new(FakeProvider::new());
        let (events, mut rx) = mpsc::unbounded_channel();
        let state = Arc::new(Prefetch::default());
        state.loaded.send_replace(true);
        let at = time::OffsetDateTime::now_utc();
        // Two batches, the first failing: the second is not asked either.
        let mut wanted = vec![(IssueKey::parse("ACME-999"), at)];
        wanted.extend((0..BULK_READ_MAX).map(|_| (IssueKey::parse("ACME-5"), at)));
        prefetch(fake.clone(), None, wanted, events.clone(), state.clone()).await;
        let five = vec![(IssueKey::parse("ACME-5"), at)];
        prefetch(fake, None, five, events, state).await;
        assert!(rx.try_recv().is_err(), "nothing read after the failure");
    }

    #[tokio::test]
    async fn the_fields_an_issue_lets_set_come_back_for_it() {
        let (service, mut events) = Service::spawn(Arc::new(FakeProvider::new()), None);
        events.recv().await; // Me
        let key = IssueKey::parse("ACME-9");
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
        let key = IssueKey::parse("ACME-9");
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

    /// A transition by its target status: the issue's own, read first.
    #[tokio::test]
    async fn a_transition_to_a_status_goes_by_the_issues_own() {
        let (service, mut events) = Service::spawn(Arc::new(FakeProvider::new()), None);
        events.recv().await; // Me
        let key = IssueKey::parse("ACME-9");
        service.send(Command::Transitions(key.clone()));
        let Some(Event::Transitions { result: Ok(ts), .. }) = events.recv().await else { panic!() };
        let t = ts.iter().find(|t| t.name == "Done").unwrap();
        let done = t.to.clone();
        let write = Write::TransitionTo { id: t.id.clone(), to: done.clone() };
        service.send(Command::Write { op: 1, key: key.clone(), write: write.clone() });
        let Some(Event::Written { op: 1, issue: Some(issue), .. }) = events.recv().await else {
            panic!()
        };
        assert_eq!(issue.status, done);
        service.send(Command::Write { op: 2, key, write });
        let Some(Event::WriteFailed { op: 2, error: DomainError::Conflict(_), .. }) =
            events.recv().await
        else {
            panic!("already there: no transition to it")
        };
    }

    #[tokio::test]
    async fn create_and_clone_answer_with_the_new_issue() {
        let (service, mut events) = Service::spawn(Arc::new(FakeProvider::new()), None);
        events.recv().await; // Me
        service.send(Command::Clone(IssueKey::parse("ACME-5")));
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
        service.send(Command::Clone(IssueKey::parse("NOPE-1")));
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
