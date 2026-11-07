use crate::board::{BoardSnapshot, load_board, sync_board};
use crate::issue_store::{IssueStore, Wanted, prefetch};
use crate::store::Storage;
use crate::write::Write;
use crate::writer::{Job, Reader, Writer};
use kolum_domain::{
    ActivityEntry, BoardId, BoardSummary, Capabilities, Capability, Comment, DomainError, Issue,
    IssueInFull, IssueKey, Provider, Query, User,
};
use std::collections::HashMap;
use std::sync::Arc;
use time::OffsetDateTime;
use tokio::sync::{mpsc, watch};

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
    /// then those a board prefetches. Only the copies that won: each
    /// replaces the one held. Shared with the service, which holds them too.
    ReadIssues(Vec<Arc<IssueDetail>>),
    Sprints(Result<Vec<kolum_domain::Sprint>, DomainError>),
    /// The answer to a `LoadIssue`, or the issue read again: the copy
    /// current after the read, which replaces the one held.
    IssueLoaded(Arc<IssueDetail>),
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
    /// The writer's end: once every command's task is done.
    written: Option<tokio::task::JoinHandle<()>>,
}

impl Service {
    /// Starts the service on the current tokio runtime. Asks for the
    /// signed-in user straight away: the board marks that user's cards.
    pub fn spawn(
        provider: Arc<dyn Provider>,
        store: Option<Arc<dyn Storage>>,
    ) -> (Self, mpsc::UnboundedReceiver<Event>) {
        let (commands, mut rx) = mpsc::unbounded_channel();
        let (events, events_rx) = mpsc::unbounded_channel();
        let started = store.map(crate::writer::start);
        let kept = started.as_ref().map(|s| s.reader.layouts()).unwrap_or_default();
        if !kept.is_empty() {
            let _ = events.send(Event::FieldLayouts(kept));
        }
        let (writer, reader, written) = match started {
            Some(s) => (Some((s.writer, s.kept)), Some(s.reader), Some(s.done)),
            None => (None, None, None),
        };
        let service = Service {
            commands,
            capabilities: provider.capabilities(),
            provider: provider.name().to_string(),
            written,
        };
        let writes = writer.as_ref().map(|(w, _)| w.clone()).unwrap_or_default();
        let issues = IssueStore::spawn(writer, events.clone(), provider.clone());
        let (boards, wanted) = watch::channel(Wanted::new());
        if provider.capabilities().supports(Capability::BulkRead) {
            tokio::spawn(prefetch(provider.clone(), issues.clone(), wanted));
        }
        let ports = Ports {
            provider: provider.clone(),
            reader,
            writer: writes,
            issues,
            boards,
            events: events.clone(),
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
            let mut turns = Turns::default();
            while let Some(command) = rx.recv().await {
                let job = run(ports.clone(), command.clone());
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

    /// No more commands: what was sent to be kept is written before this
    /// ends, or `limit` passes, past which it is given up.
    pub async fn close(self, limit: std::time::Duration) {
        let Service { commands, written, .. } = self;
        drop(commands);
        if let Some(written) = written
            && tokio::time::timeout(limit, written).await.is_err()
        {
            tracing::warn!("quitting before everything read was kept");
        }
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

/// What each command's task works with.
#[derive(Clone)]
struct Ports {
    provider: Arc<dyn Provider>,
    /// What is kept, to read; none without storage.
    reader: Option<Reader>,
    /// What to keep goes there.
    writer: Writer,
    issues: IssueStore,
    /// The latest board's issues, for the prefetch.
    boards: watch::Sender<Wanted>,
    events: mpsc::UnboundedSender<Event>,
}

/// One command, answered by its event. A flat dispatch: each arm one call.
async fn run(ports: Ports, command: Command) {
    let provider = ports.provider.as_ref();
    let event = match command {
        Command::LoadBoard(id, request) => ports.load_board(id, request).await,
        Command::ReloadBoard(id, request) => ports.fetch_board(id, request, None).await,
        Command::SyncBoard(id, request) => ports.sync_board(id, request).await,
        Command::LoadIssue(key) => match ports.load_issue(key).await {
            Some(answer) => answer,
            None => return,
        },
        Command::Sprints(board) => Event::Sprints(provider.sprints(&board).await),
        Command::LoadBoards => {
            or_failed(provider.boards().await, Event::Boards, || "listing the boards".into())
        }
        Command::Transitions(key) => {
            Event::Transitions { result: provider.transitions(&key).await, key }
        }
        Command::EditableFields(key) => {
            Event::EditableFields { result: provider.editable_fields(&key).await, key }
        }
        Command::Write { op, key, write } => written(provider, op, key, write).await,
        Command::Image(url) => Event::Image { result: provider.image(&url).await, url },
        Command::Create(new) => {
            or_failed(provider.create(new).await, created, || "creating the issue".into())
        }
        Command::Clone(key) => {
            or_failed(provider.clone_issue(&key).await, created, || format!("cloning {key}"))
        }
        Command::Search { id, query } => search(provider, id, query).await,
    };
    ports.announce(event);
}

async fn search(provider: &dyn Provider, id: u64, query: kolum_domain::Query) -> Event {
    let result = provider.search(&query, None).await.map(|page| page.items);
    Event::SearchResults { id, result }
}

fn created(issue: Issue) -> Event {
    Event::Created(Box::new(issue))
}

/// The answer, or what failed, in words for a status line.
fn or_failed<T>(
    result: kolum_domain::Result<T>,
    answer: impl FnOnce(T) -> Event,
    what: impl FnOnce() -> String,
) -> Event {
    match result {
        Ok(value) => answer(value),
        Err(error) => Event::Failed { what: what(), error },
    }
}

impl Ports {
    /// The cache first, then the whole board: a board filtered by date
    /// loses issues nobody touched, which no incremental sync reports.
    async fn load_board(&self, id: Option<BoardId>, request: u64) -> Event {
        if let Some((snapshot, _)) = self.kept_board(id.clone()).await {
            let _ = self.events.send(Event::BoardLoaded { snapshot, fresh: false, request });
        }
        self.fetch_board(id, request, None).await
    }

    /// What changed since the board kept, or the whole board when none is.
    async fn sync_board(&self, id: Option<BoardId>, request: u64) -> Event {
        let cached = self.kept_board(id.clone()).await;
        self.fetch_board(id, request, cached).await
    }

    async fn kept_board(&self, id: Option<BoardId>) -> Option<(BoardSnapshot, OffsetDateTime)> {
        self.reader.as_ref()?.board(id).await
    }

    /// From the provider, synced from `cached` when there is one, and kept
    /// with the time the sync started: whatever changes while it runs is
    /// caught by the next one.
    async fn fetch_board(
        &self,
        id: Option<BoardId>,
        request: u64,
        cached: Option<(BoardSnapshot, OffsetDateTime)>,
    ) -> Event {
        let provider = self.provider.as_ref();
        let started = OffsetDateTime::now_utc();
        let result = match cached {
            Some((snapshot, since)) => sync_board(provider, snapshot, since).await,
            None => load_board(provider, id.clone()).await,
        };
        match result {
            Ok(snapshot) => {
                self.writer.send(Job::Board(id, snapshot.clone(), started));
                Event::BoardLoaded { snapshot, fresh: true, request }
            }
            Err(error) => Event::Failed { what: "loading the board".into(), error },
        }
    }

    /// The issue read whole goes to the issue store, which answers; its
    /// layout is kept here. `Some` when there is no one else to answer.
    async fn load_issue(&self, key: IssueKey) -> Option<Event> {
        let detail = match load_issue(self.provider.as_ref(), &key).await {
            Ok(detail) => detail,
            Err(error) => {
                tracing::warn!("loading {key}: {error}");
                return Some(Event::IssueLoadFailed { key, error });
            }
        };
        if let Some(layout) = &detail.layout {
            self.writer.send(Job::Layout(layout_key(&detail.issue), layout.clone()));
        }
        // The store gone, the read goes straight to the screen.
        Some(Event::IssueLoaded(Arc::from(self.issues.opened(detail).err()?)))
    }

    /// The event sent, a failure logged; a board fresh from the provider
    /// is seen by the issue store and fed to the prefetch.
    fn announce(&self, event: Event) {
        if let Event::Failed { what, error } = &event {
            tracing::warn!("{what}: {error}");
        }
        if let Event::BoardLoaded { snapshot, fresh: true, .. } = &event {
            self.issues.seen(snapshot.issues.iter().map(|i| i.key.clone()).collect());
            let wanted = snapshot.issues.iter().map(|i| (i.key.clone(), i.updated)).collect();
            self.boards.send_replace(wanted);
        }
        let _ = self.events.send(event);
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

pub(crate) async fn load_issue(
    provider: &dyn Provider,
    key: &IssueKey,
) -> kolum_domain::Result<IssueDetail> {
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
    use crate::store::tests::Memory;
    use crate::store::{IssueArchive, Store};
    use kolum_provider_fake::FakeProvider;
    use std::sync::atomic::Ordering;
    use time::OffsetDateTime;

    /// The next event, or `None` after two seconds without one: a test
    /// that waits on an event never sent fails rather than hangs.
    trait Within {
        async fn within(&mut self) -> Option<Event>;
    }

    impl Within for mpsc::UnboundedReceiver<Event> {
        async fn within(&mut self) -> Option<Event> {
            let wait = std::time::Duration::from_secs(2);
            tokio::time::timeout(wait, self.recv()).await.ok().flatten()
        }
    }

    #[tokio::test]
    async fn commands_come_back_as_events() {
        let (service, mut events) = Service::spawn(Arc::new(FakeProvider::new()), None);
        assert_eq!(service.provider_name(), "fake");
        let Some(Event::Me(me)) = events.within().await else { panic!("expected Me first") };
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
        service.send(Command::Sprints(boards[0].id.clone()));
        let Some(Event::Sprints(Ok(sprints))) = next(&mut events).await else { panic!() };
        assert_eq!(sprints.len(), 2, "the running one and the next");

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
        events.within().await; // Me
        service.send(Command::LoadIssue(IssueKey::parse("ACME-999")));
        let Some(Event::IssueLoadFailed { key, error }) = events.within().await else { panic!() };
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
            // A paused clock: a bounded wait would time out first.
            match events.recv().await {
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
            match events.within().await {
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
        let store = Arc::new(Memory::default());
        let fake = Arc::new(FakeProvider::new());
        let (service, mut events) = Service::spawn(fake.clone(), Some(store.clone()));
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
    }

    #[tokio::test]
    async fn a_warm_start_shows_the_cache_then_the_sync() {
        use kolum_domain::{IssueUpdate, IssueWriter};
        let store = Arc::new(Memory::default());
        let fake = Arc::new(FakeProvider::new());
        let (service, mut events) = Service::spawn(fake.clone(), Some(store.clone()));
        service.send(Command::LoadBoard(None, 1));
        let (cold, fresh) = next_board(&mut events).await;
        assert!(fresh, "nothing cached: straight from the provider");
        service.close(std::time::Duration::from_secs(1)).await;
        assert!(store.board(None).is_some(), "kept");

        let key = IssueKey::parse("ACME-12");
        fake.update(&key, IssueUpdate { title: Some("Renamed".into()), ..Default::default() })
            .await
            .unwrap();
        let (service, mut events) = Service::spawn(fake.clone(), Some(store.clone()));
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
    }

    /// A board filtered by date (completed issues older than a week
    /// hidden) loses issues nobody touched, which no incremental sync
    /// reports: a warm start reads the whole board again.
    #[tokio::test]
    async fn a_warm_start_drops_what_left_the_board_since() {
        let store = Arc::new(Memory::default());
        let fake = Arc::new(FakeProvider::new());
        let mut cached = load_board(fake.as_ref(), None).await.unwrap();
        let mut gone = cached.issues[0].clone();
        gone.key = IssueKey::parse("ACME-999");
        cached.issues.push(gone);
        store.save_board(None, &cached, time::OffsetDateTime::now_utc()).unwrap();
        let (service, mut events) = Service::spawn(fake, Some(store.clone()));
        service.send(Command::LoadBoard(None, 1));
        let has_gone = |b: &BoardSnapshot| b.issues.iter().any(|i| i.key.as_str() == "ACME-999");
        let (warm, _) = next_board(&mut events).await;
        assert!(has_gone(&warm), "the cache first, as it was");
        let (loaded, fresh) = next_board(&mut events).await;
        assert!(fresh && !has_gone(&loaded), "then the board as it is");
    }

    #[tokio::test]
    async fn search_results_carry_their_request_id() {
        use kolum_domain::{Filter, UserSel};
        let (service, mut events) = Service::spawn(Arc::new(FakeProvider::new()), None);
        events.within().await; // Me
        let mine = Query::Filter(Filter { assignees: vec![UserSel::Me], ..Default::default() });
        service.send(Command::Search { id: 7, query: mine });
        let Some(Event::SearchResults { id: 7, result: Ok(issues) }) = events.within().await else {
            panic!()
        };
        assert!(!issues.is_empty());
        service.send(Command::Search { id: 8, query: Query::Native("x".into()) });
        let Some(Event::SearchResults { id: 8, result: Err(DomainError::Unsupported(_)) }) =
            events.within().await
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
        let store = Arc::new(Memory::default());
        let fake = Arc::new(FakeProvider::new());
        let (service, mut events) = Service::spawn(fake.clone(), Some(store.clone()));
        events.within().await; // Me
        service.send(Command::LoadIssue(IssueKey::parse("ACME-5")));
        let Some(Event::IssueLoaded(detail)) = events.within().await else { panic!() };
        service.close(std::time::Duration::from_secs(1)).await;
        let (_again, mut events) = Service::spawn(fake, Some(store.clone()));
        let Some(Event::FieldLayouts(layouts)) = events.within().await else {
            panic!("the layouts come first")
        };
        assert_eq!(layouts.get(&layout_key(&detail.issue)), detail.layout.as_ref());
    }

    #[tokio::test]
    async fn an_issue_read_is_kept_for_the_next_run() {
        let store = Arc::new(Memory::default());
        let fake = Arc::new(FakeProvider::new());
        let (service, mut events) = Service::spawn(fake.clone(), Some(store.clone()));
        events.within().await; // Me
        service.send(Command::LoadIssue(IssueKey::parse("ACME-5")));
        let Some(Event::IssueLoaded(detail)) = events.within().await else { panic!() };
        // Written after it is sent on.
        until(|| store.issue(&detail.issue.key).is_some()).await;
        let (_again, mut events) = Service::spawn(fake, Some(store.clone()));
        let read = loop {
            match events.within().await {
                Some(Event::ReadIssues(read)) => break read,
                Some(Event::FieldLayouts(_) | Event::Me(_)) => continue,
                other => panic!("the issues read, not {other:?}"),
            }
        };
        assert_eq!(read, [detail]);
    }

    /// The next `ReadIssues`, or `None` when none comes in a moment.
    async fn next_read(events: &mut mpsc::UnboundedReceiver<Event>) -> Option<Vec<IssueDetail>> {
        let wait = std::time::Duration::from_millis(300);
        loop {
            match tokio::time::timeout(wait, events.recv()).await {
                Ok(Some(Event::ReadIssues(read))) => {
                    return Some(read.into_iter().map(|d| (*d).clone()).collect());
                }
                Ok(Some(_)) => continue,
                _ => return None,
            }
        }
    }

    /// Every `ReadIssues` until none comes in a moment, as one.
    async fn all_read(events: &mut mpsc::UnboundedReceiver<Event>) -> Vec<IssueDetail> {
        let mut all = vec![];
        while let Some(read) = next_read(events).await {
            all.extend(read);
        }
        all
    }

    #[tokio::test]
    async fn a_board_prefetches_its_issues_in_full_then_only_what_changed() {
        use kolum_domain::{IssueUpdate, IssueWriter};
        let store = Arc::new(Memory::default());
        let fake = Arc::new(FakeProvider::new());
        let (service, mut events) = Service::spawn(fake.clone(), Some(store.clone()));
        service.send(Command::LoadBoard(None, 1));
        let read = all_read(&mut events).await;
        let board = load_board(fake.as_ref(), None).await.unwrap();
        assert_eq!(read.len(), board.issues.len());
        let five = read.iter().find(|d| d.issue.key.as_str() == "ACME-5").unwrap();
        assert!(
            !five.comments.is_empty() && !five.activity.is_empty(),
            "with comments and history"
        );

        service.send(Command::SyncBoard(None, 1));
        assert_eq!(next_read(&mut events).await, None, "nothing changed, nothing read");
        assert_eq!(store.issues().len(), board.issues.len(), "kept for the next run");

        let key = IssueKey::parse("ACME-12");
        fake.update(&key, IssueUpdate { title: Some("Renamed".into()), ..Default::default() })
            .await
            .unwrap();
        service.send(Command::SyncBoard(None, 1));
        let read = next_read(&mut events).await.expect("what changed");
        assert_eq!(read.iter().map(|d| d.issue.key.clone()).collect::<Vec<_>>(), [key]);

        // The next run starts from what is kept: nothing to read again.
        service.close(std::time::Duration::from_secs(1)).await;
        let (service, mut events) = Service::spawn(fake.clone(), Some(store.clone()));
        service.send(Command::LoadBoard(None, 1));
        let kept = next_read(&mut events).await.expect("what the last run kept");
        assert_eq!(kept.len(), board.issues.len());
        assert_eq!(next_read(&mut events).await, None, "and no prefetch over it");
    }

    /// A prefetch reads no children nor layout: those an opening kept stay.
    #[tokio::test]
    async fn a_prefetch_keeps_the_children_an_opening_read() {
        use kolum_domain::IssueReader;
        let store = Arc::new(Memory::default());
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
        store.save_issue(&opened).unwrap();
        let (service, mut events) = Service::spawn(fake.clone(), Some(store.clone()));
        next_read(&mut events).await.expect("what the last run kept");
        service.send(Command::LoadBoard(None, 1));
        let read = next_read(&mut events).await.expect("the board read in full");
        let prefetched = read.iter().find(|d| d.issue.key == key).unwrap();
        assert!(prefetched.issue.updated > opened.issue.updated, "read again");
        assert_eq!(prefetched.children, opened.children);
        assert_eq!(prefetched.layout, opened.layout);
        let kept = store.issues();
        let kept = kept.iter().find(|d| d.issue.key == key).unwrap();
        assert_eq!(kept.children, opened.children, "on disk too");
    }

    /// An issue kept and on a board is seen there: it is kept as long as
    /// if just read, though nothing reads it again.
    #[tokio::test]
    async fn a_board_keeps_the_issues_on_it() {
        use kolum_domain::IssueReader;
        let store = Arc::new(Memory::default());
        let fake = Arc::new(FakeProvider::new());
        let key = IssueKey::parse("ACME-5");
        let issue = fake.issue(&key).await.unwrap();
        let kept = IssueDetail {
            issue,
            comments: vec![],
            activity: vec![],
            layout: None,
            children: vec![],
        };
        store.save_issue(&kept).unwrap();
        let (service, mut events) = Service::spawn(fake, Some(store.clone()));
        // Opened just before: its file was just written, no need.
        service.send(Command::LoadIssue(IssueKey::parse("ACME-6")));
        loaded_within(&mut events, std::time::Duration::from_secs(1)).await;
        service.send(Command::LoadBoard(None, 1));
        next_board(&mut events).await;
        until(|| !store.touched.lock().unwrap().is_empty()).await;
        assert_eq!(*store.touched.lock().unwrap(), std::slice::from_ref(&key), "the one kept");
        service.send(Command::SyncBoard(None, 2));
        next_board(&mut events).await;
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        assert_eq!(*store.touched.lock().unwrap(), [key], "once a run, not every sync");
    }

    /// Until `done`, which fails the test after a second.
    async fn until(done: impl Fn() -> bool) {
        let wait = async {
            while !done() {
                tokio::time::sleep(std::time::Duration::from_millis(1)).await;
            }
        };
        tokio::time::timeout(std::time::Duration::from_secs(1), wait).await.expect("in time");
    }

    /// The prefetch fed boards by hand, with what it sends on.
    fn prefetching(
        fake: &Arc<FakeProvider>,
    ) -> (watch::Sender<Wanted>, mpsc::UnboundedReceiver<Event>, tokio::task::JoinHandle<()>) {
        let (events, rx) = mpsc::unbounded_channel();
        let (boards, wanted) = watch::channel(Wanted::new());
        let store = IssueStore::spawn(None, events, fake.clone());
        (boards, rx, tokio::spawn(prefetch(fake.clone(), store, wanted)))
    }

    /// A refusal, a rate limit most likely, ends the prefetch for the run:
    /// the user's own requests come first.
    #[tokio::test]
    async fn a_refused_prefetch_stops_for_the_run_keeping_what_it_read() {
        let fake = Arc::new(FakeProvider::new());
        fake.refuse_bulk_reads_after(1);
        let (boards, mut events, prefetch) = prefetching(&fake);
        let board = load_board(fake.as_ref(), None).await.unwrap();
        boards.send_replace(board.issues.iter().map(|i| (i.key.clone(), i.updated)).collect());
        let wait = std::time::Duration::from_secs(1);
        tokio::time::timeout(wait, prefetch)
            .await
            .expect("ended, the boards still coming")
            .unwrap();
        let Some(read) = next_read(&mut events).await else { panic!("the batch read first") };
        assert_eq!(read.len(), kolum_provider_fake::BULK_BATCH);
        assert_eq!(next_read(&mut events).await, None, "nothing after the refusal");
        assert_eq!(fake.bulk_reads().len(), 1);
    }

    /// An issue asked and not read, one Jira refuses or kolum cannot read,
    /// is not asked again every sync, until it changes.
    #[tokio::test]
    async fn an_issue_not_read_is_not_asked_again_until_it_changes() {
        let fake = Arc::new(FakeProvider::new());
        let (boards, mut events, _prefetch) = prefetching(&fake);
        let at = OffsetDateTime::now_utc();
        let (five, nope) = (IssueKey::parse("ACME-5"), IssueKey::parse("NOPE-1"));
        let board = vec![(five.clone(), at), (nope.clone(), at)];
        boards.send_replace(board.clone());
        assert!(next_read(&mut events).await.is_some());
        boards.send_replace(board);
        let six = IssueKey::parse("ACME-6");
        boards.send_replace(vec![(nope.clone(), at), (six.clone(), at)]);
        assert!(next_read(&mut events).await.is_some());
        let later = at + time::Duration::minutes(1);
        boards.send_replace(vec![(nope.clone(), later)]);
        until(|| fake.bulk_reads().len() >= 3).await;
        let asked = vec![vec![five, nope.clone()], vec![six], vec![nope]];
        assert_eq!(fake.bulk_reads(), asked, "again once it changed");
    }

    /// An opening's read lands while a slower prefetch of the same issue
    /// is out: the prefetch's copy, of the same version and cut short,
    /// replaces it neither in what is sent nor in what is kept.
    #[tokio::test]
    async fn a_full_read_stays_over_a_slower_bulk_read_of_the_same_issue() {
        let store = Arc::new(Memory::default());
        let fake = Arc::new(FakeProvider::new());
        fake.delay_next_bulk_read(std::time::Duration::from_millis(150));
        let (service, mut events) = Service::spawn(fake.clone(), Some(store.clone()));
        service.send(Command::LoadBoard(None, 1));
        next_board(&mut events).await;
        until(|| !fake.bulk_reads().is_empty()).await;
        let key = IssueKey::parse("ACME-5");
        service.send(Command::LoadIssue(key.clone()));
        let opened = loop {
            match events.within().await {
                Some(Event::IssueLoaded(detail)) => break detail,
                Some(Event::ReadIssues(_)) => panic!("the prefetch is the slower"),
                _ => continue,
            }
        };
        assert_eq!(opened.comments.len(), 3, "whole");
        let read = next_read(&mut events).await.expect("the prefetch");
        let five = read.iter().find(|d| d.issue.key == key);
        assert!(five.is_none_or(|d| d.comments.len() == 3), "not sent over the opening's");
        assert_eq!(store.issue(&key).map(|d| d.comments.len()), Some(3), "nor kept over it");
    }

    /// An archive slow to write, which counts the writes going on at once.
    #[derive(Default)]
    struct Slow {
        kept: Memory,
        took: std::time::Duration,
        /// How long reading what earlier runs kept takes.
        reading: std::time::Duration,
        /// How long writing a board takes.
        board: std::time::Duration,
        writing: std::sync::atomic::AtomicUsize,
        most: std::sync::atomic::AtomicUsize,
    }

    impl Store for Slow {
        fn board(&self, id: Option<&BoardId>) -> Option<(BoardSnapshot, OffsetDateTime)> {
            self.kept.board(id)
        }
        fn save_board(
            &self,
            id: Option<&BoardId>,
            snapshot: &BoardSnapshot,
            at: OffsetDateTime,
        ) -> std::io::Result<()> {
            std::thread::sleep(self.board);
            self.kept.save_board(id, snapshot, at)
        }
        fn layouts(&self) -> std::collections::BTreeMap<String, kolum_domain::FieldLayout> {
            self.kept.layouts()
        }
        fn save_layout(
            &self,
            key: &str,
            layout: &kolum_domain::FieldLayout,
        ) -> std::io::Result<()> {
            self.kept.save_layout(key, layout)
        }
    }

    impl IssueArchive for Slow {
        fn issues(&self) -> Vec<IssueDetail> {
            std::thread::sleep(self.reading);
            self.kept.issues()
        }
        fn touch(&self, keys: &[IssueKey]) {
            self.kept.touch(keys)
        }
        fn save_issue(&self, detail: &IssueDetail) -> std::io::Result<()> {
            let now = self.writing.fetch_add(1, Ordering::SeqCst) + 1;
            self.most.fetch_max(now, Ordering::SeqCst);
            std::thread::sleep(self.took);
            let saved = self.kept.save_issue(detail);
            self.writing.fetch_sub(1, Ordering::SeqCst);
            saved
        }
    }

    /// The store reads an issue again on its own: when that read fails, no
    /// one waited on it, so the screen is not told.
    #[tokio::test]
    async fn the_stores_own_read_failing_is_only_logged() {
        use kolum_domain::IssueReader;
        let store = Arc::new(Memory::default());
        let fake = Arc::new(FakeProvider::new());
        let epic = IssueKey::parse("ACME-1");
        let mut newer = fake.issue(&epic).await.unwrap();
        newer.updated += time::Duration::minutes(1);
        let bulk = IssueDetail {
            issue: newer,
            comments: vec![],
            activity: vec![],
            layout: None,
            children: vec![],
        };
        store.save_issue(&bulk).unwrap();
        let (service, mut events) = Service::spawn(fake.clone(), Some(store));
        next_read(&mut events).await.expect("the bulk read kept");
        fake.refuse_comments_after(1);
        service.send(Command::LoadIssue(epic));
        answer(&mut events).await;
        let wait = std::time::Duration::from_millis(300);
        while let Ok(Some(event)) = tokio::time::timeout(wait, events.recv()).await {
            assert!(!matches!(event, Event::IssueLoadFailed { .. }), "told: {event:?}");
        }
    }

    /// The issue as the screen shows it once a `LoadIssue` is answered.
    async fn answer(events: &mut mpsc::UnboundedReceiver<Event>) -> IssueDetail {
        loop {
            match events.within().await {
                Some(Event::IssueLoaded(detail)) => return (*detail).clone(),
                None => panic!("no answer"),
                Some(_) => {}
            }
        }
    }

    /// With the issue store gone, an opening is still answered: the read
    /// goes straight to the screen.
    #[tokio::test]
    async fn an_opening_is_answered_without_the_issue_store() {
        let (events, mut rx) = mpsc::unbounded_channel();
        let ports = Ports {
            provider: Arc::new(FakeProvider::new()),
            reader: None,
            writer: Writer::default(),
            issues: IssueStore::ended(),
            boards: watch::channel(Wanted::new()).0,
            events,
        };
        run(ports, Command::LoadIssue(IssueKey::parse("ACME-5"))).await;
        let Ok(Event::IssueLoaded(detail)) = rx.try_recv() else { panic!("answered") };
        assert_eq!(detail.comments.len(), 3);
    }

    /// With the issue store gone, the prefetch ends rather than read every
    /// board as wholly stale.
    #[tokio::test]
    async fn the_prefetch_ends_with_the_issue_store() {
        let fake = Arc::new(FakeProvider::new());
        let (boards, wanted) = watch::channel(Wanted::new());
        let prefetch = tokio::spawn(prefetch(fake.clone(), IssueStore::ended(), wanted));
        boards.send_replace(vec![(IssueKey::parse("ACME-5"), OffsetDateTime::now_utc())]);
        let wait = std::time::Duration::from_secs(1);
        tokio::time::timeout(wait, prefetch).await.expect("ended").unwrap();
        assert!(fake.bulk_reads().is_empty(), "nothing read for no one");
    }

    /// The next `IssueLoaded`, within `limit`.
    async fn loaded_within(
        events: &mut mpsc::UnboundedReceiver<Event>,
        limit: std::time::Duration,
    ) -> Arc<IssueDetail> {
        let loaded = async {
            loop {
                if let Some(Event::IssueLoaded(detail)) = events.within().await {
                    return detail;
                }
            }
        };
        tokio::time::timeout(limit, loaded).await.expect("answered in time")
    }

    /// The issue store writes on a task of its own: an opening is answered
    /// while a prefetch's forty issues are still being written.
    #[tokio::test]
    async fn an_opening_answers_while_the_issues_are_being_written() {
        let ms = std::time::Duration::from_millis;
        let store = Arc::new(Slow { took: ms(50), ..Default::default() });
        let (service, mut events) =
            Service::spawn(Arc::new(FakeProvider::new()), Some(store.clone()));
        service.send(Command::LoadBoard(None, 1));
        until(|| store.writing.load(Ordering::SeqCst) > 0).await;
        service.send(Command::LoadIssue(IssueKey::parse("ACME-5")));
        loaded_within(&mut events, ms(500)).await;
    }

    /// Nor does reading what earlier runs kept hold up an opening.
    #[tokio::test]
    async fn an_opening_answers_while_the_kept_issues_are_read() {
        let ms = std::time::Duration::from_millis;
        let store = Arc::new(Slow { reading: ms(1000), ..Default::default() });
        let (service, mut events) = Service::spawn(Arc::new(FakeProvider::new()), Some(store));
        service.send(Command::LoadIssue(IssueKey::parse("ACME-5")));
        loaded_within(&mut events, ms(500)).await;
    }

    /// An opening read older than a bulk read kept: the issue is read whole
    /// again, as a newer whole version exists; meanwhile the bulk copy
    /// shows with the opening's children.
    #[tokio::test]
    async fn an_opening_older_than_a_bulk_read_kept_is_read_again() {
        use kolum_domain::IssueReader;
        let store = Arc::new(Memory::default());
        let fake = Arc::new(FakeProvider::new());
        let epic = IssueKey::parse("ACME-1");
        let mut newer = fake.issue(&epic).await.unwrap();
        newer.updated += time::Duration::minutes(1);
        let bulk = IssueDetail {
            issue: newer,
            comments: vec![],
            activity: vec![],
            layout: None,
            children: vec![],
        };
        store.save_issue(&bulk).unwrap();
        let (service, mut events) = Service::spawn(fake, Some(store));
        next_read(&mut events).await.expect("the bulk read kept");
        service.send(Command::LoadIssue(epic.clone()));
        let shown = answer(&mut events).await;
        assert_eq!(shown.issue.updated, bulk.issue.updated, "the newer copy shows");
        assert_eq!(shown.children.len(), 6, "with the opening's children");
        let again = answer(&mut events).await;
        assert_eq!(again.issue.key, epic, "read again, unasked");
    }

    /// Quitting waits for what was read to be kept: two openings written
    /// one after the other, the second still queued when the user quits.
    #[test]
    fn quitting_keeps_what_was_read() {
        let took = std::time::Duration::from_millis(200);
        let store = Arc::new(Slow { took, ..Default::default() });
        let runtime = tokio::runtime::Runtime::new().unwrap();
        runtime.block_on(async {
            let fake = Arc::new(FakeProvider::new());
            let (service, mut events) = Service::spawn(fake, Some(store.clone()));
            for key in ["ACME-5", "ACME-6"] {
                service.send(Command::LoadIssue(IssueKey::parse(key)));
                loaded_within(&mut events, std::time::Duration::from_secs(1)).await;
            }
            service.close(std::time::Duration::from_secs(2)).await;
        });
        drop(runtime);
        assert_eq!(store.kept.issues().len(), 2, "both kept");
    }

    /// The service keeps a board off the runtime's threads: a reply due
    /// while a large board is being written is not held up.
    #[tokio::test]
    async fn a_board_written_holds_up_no_reply() {
        let ms = std::time::Duration::from_millis;
        let store = Arc::new(Slow { board: ms(1000), ..Default::default() });
        let (service, mut events) = Service::spawn(Arc::new(FakeProvider::new()), Some(store));
        service.send(Command::LoadBoard(None, 1));
        let asked = std::time::Instant::now();
        service.send(Command::Transitions(IssueKey::parse("ACME-9")));
        loop {
            match next(&mut events).await {
                Some(Event::Transitions { .. }) => break,
                None => panic!("no reply"),
                Some(_) => {}
            }
        }
        let waited = asked.elapsed();
        assert!(waited < ms(500), "{waited:?} for the reply: the board's writing held it up");
    }

    /// Openings and a prefetch at once: one writer of the issues, so no
    /// two writes of them ever overlap.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn the_issues_are_written_one_at_a_time() {
        let took = std::time::Duration::from_millis(20);
        let store = Arc::new(Slow { took, ..Default::default() });
        let fake = Arc::new(FakeProvider::new());
        let (service, mut events) = Service::spawn(fake.clone(), Some(store.clone()));
        service.send(Command::LoadBoard(None, 1));
        let keys = ["ACME-5", "ACME-6", "ACME-7", "ACME-9", "ACME-10", "ACME-12"];
        for key in keys {
            service.send(Command::LoadIssue(IssueKey::parse(key)));
        }
        let board = load_board(fake.as_ref(), None).await.unwrap().issues.len();
        let all_kept = async {
            while store.kept.issues().len() < board || store.writing.load(Ordering::SeqCst) > 0 {
                let _ = tokio::time::timeout(took, events.recv()).await;
            }
        };
        tokio::time::timeout(std::time::Duration::from_secs(5), all_kept).await.unwrap();
        assert_eq!(store.most.load(Ordering::SeqCst), 1);
    }

    /// The prefetch writes what it read off the runtime's threads: a reply
    /// due while it writes is not held up by the disk.
    #[tokio::test]
    async fn a_prefetch_writing_holds_up_no_reply() {
        use kolum_domain::TransitionId;
        let ms = std::time::Duration::from_millis;
        let store = Arc::new(Slow { took: ms(20), ..Default::default() });
        let fake = Arc::new(FakeProvider::new());
        let (service, mut events) = Service::spawn(fake.clone(), Some(store));
        fake.delay_next_bulk_read(ms(50));
        service.send(Command::LoadBoard(None, 1));
        next_board(&mut events).await;
        // Its reply due once the prefetch has its 40-odd issues to write.
        fake.delay_next_transition(ms(100));
        let asked = std::time::Instant::now();
        let to_done = Write::Transition(TransitionId::new("to-done"));
        service.send(Command::Write { op: 1, key: IssueKey::parse("ACME-9"), write: to_done });
        loop {
            match next(&mut events).await {
                Some(Event::Written { .. }) => break,
                None => panic!("no reply"),
                Some(_) => {}
            }
        }
        let waited = asked.elapsed();
        assert!(waited < ms(500), "{waited:?} for the reply: the disk held it up");
    }

    #[tokio::test]
    async fn the_fields_an_issue_lets_set_come_back_for_it() {
        let (service, mut events) = Service::spawn(Arc::new(FakeProvider::new()), None);
        events.within().await; // Me
        let key = IssueKey::parse("ACME-9");
        service.send(Command::EditableFields(key.clone()));
        let Some(Event::EditableFields { key: k, result: Ok(fields) }) = events.within().await
        else {
            panic!()
        };
        assert_eq!(k, key);
        assert!(fields.iter().any(|f| f.name == "Team"));
    }

    #[tokio::test]
    async fn a_write_comes_back_with_the_issue_as_the_provider_has_it() {
        let (service, mut events) = Service::spawn(Arc::new(FakeProvider::new()), None);
        events.within().await; // Me
        let key = IssueKey::parse("ACME-9");
        service.send(Command::Transitions(key.clone()));
        let Some(Event::Transitions { result: Ok(ts), .. }) = events.within().await else {
            panic!()
        };
        let to_done = ts.iter().find(|t| t.name == "Done").unwrap().id.clone();
        service.send(Command::Write { op: 1, key: key.clone(), write: Write::Transition(to_done) });
        let Some(Event::Written { op: 1, issue: Some(issue), .. }) = events.within().await else {
            panic!()
        };
        assert_eq!(issue.status.name, "Done");
        let hi = Write::Comment(kolum_domain::RichText::from_markdown("hi"));
        service.send(Command::Write { op: 2, key: key.clone(), write: hi });
        assert!(matches!(events.within().await, Some(Event::Written { op: 2, .. })));
        let nobody = Write::Assign(Some(kolum_domain::UserId::new("nobody")));
        service.send(Command::Write { op: 3, key, write: nobody });
        let Some(Event::WriteFailed { op: 3, error: DomainError::NotFound(_), .. }) =
            events.within().await
        else {
            panic!()
        };
    }

    /// A transition by its target status: the issue's own, read first.
    #[tokio::test]
    async fn a_transition_to_a_status_goes_by_the_issues_own() {
        let (service, mut events) = Service::spawn(Arc::new(FakeProvider::new()), None);
        events.within().await; // Me
        let key = IssueKey::parse("ACME-9");
        service.send(Command::Transitions(key.clone()));
        let Some(Event::Transitions { result: Ok(ts), .. }) = events.within().await else {
            panic!()
        };
        let t = ts.iter().find(|t| t.name == "Done").unwrap();
        let done = t.to.clone();
        let write = Write::TransitionTo { id: t.id.clone(), to: done.clone() };
        service.send(Command::Write { op: 1, key: key.clone(), write: write.clone() });
        let Some(Event::Written { op: 1, issue: Some(issue), .. }) = events.within().await else {
            panic!()
        };
        assert_eq!(issue.status, done);
        service.send(Command::Write { op: 2, key, write });
        let Some(Event::WriteFailed { op: 2, error: DomainError::Conflict(_), .. }) =
            events.within().await
        else {
            panic!("already there: no transition to it")
        };
    }

    #[tokio::test]
    async fn create_and_clone_answer_with_the_new_issue() {
        let (service, mut events) = Service::spawn(Arc::new(FakeProvider::new()), None);
        events.within().await; // Me
        service.send(Command::Clone(IssueKey::parse("ACME-5")));
        let Some(Event::Created(clone)) = events.within().await else { panic!() };
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
        let Some(Event::Created(created)) = events.within().await else { panic!() };
        assert_eq!(created.title, "T");
        service.send(Command::Clone(IssueKey::parse("NOPE-1")));
        assert!(
            matches!(events.within().await, Some(Event::Failed { what, .. }) if what == "cloning NOPE-1")
        );
    }

    #[tokio::test]
    async fn an_avatar_comes_back_by_its_url() {
        let (service, mut events) = Service::spawn(Arc::new(FakeProvider::new()), None);
        let Some(Event::Me(me)) = events.within().await else { panic!() };
        let url = me.avatar_url.unwrap();
        service.send(Command::Image(url.clone()));
        let Some(Event::Image { url: back, result: Ok(bytes) }) = events.within().await else {
            panic!()
        };
        assert_eq!(back, url);
        assert!(bytes.starts_with(b"P6"));
    }
}
