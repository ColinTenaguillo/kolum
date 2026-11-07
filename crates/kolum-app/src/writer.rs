//! The only writer of what the application keeps: one task, one job at a
//! time, each on a thread of its own, as files and JSON block. Whoever
//! keeps something sends it a job; the rest of the application only reads,
//! through a handle that cannot write.

use crate::board::BoardSnapshot;
use crate::service::IssueDetail;
use crate::store::Storage;
use kolum_domain::{BoardId, FieldLayout, IssueKey};
use std::collections::BTreeMap;
use std::sync::Arc;
use time::OffsetDateTime;
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;

/// What the writer does, in the order sent.
pub(crate) enum Job {
    Issues(Vec<Arc<IssueDetail>>),
    /// Issues seen on a board: kept as long as if just read.
    Touch(Vec<IssueKey>),
    Board(Option<BoardId>, BoardSnapshot, OffsetDateTime),
    Layout(String, FieldLayout),
}

/// The handle to send the writer its jobs; without storage, one that
/// keeps nothing.
#[derive(Clone, Default)]
pub(crate) struct Writer(Option<mpsc::UnboundedSender<Job>>);

impl Writer {
    pub fn send(&self, job: Job) {
        if let Some(jobs) = &self.0 {
            let _ = jobs.send(job);
        }
    }
}

/// The storage, for reading only.
#[derive(Clone)]
pub(crate) struct Reader(Arc<dyn Storage>);

impl Reader {
    /// The board as last kept, read off the runtime's threads.
    pub async fn board(&self, id: Option<BoardId>) -> Option<(BoardSnapshot, OffsetDateTime)> {
        let storage = self.0.clone();
        tokio::task::spawn_blocking(move || storage.board(id.as_ref())).await.ok().flatten()
    }

    pub fn layouts(&self) -> BTreeMap<String, FieldLayout> {
        self.0.layouts()
    }
}

/// The writer, started, with what goes with it.
pub(crate) struct Started {
    pub writer: Writer,
    pub reader: Reader,
    /// The issues earlier runs kept, which the writer reads first.
    pub kept: oneshot::Receiver<Vec<IssueDetail>>,
    /// Ends once every `Writer` is dropped and the jobs sent are done.
    pub done: JoinHandle<()>,
}

pub(crate) fn start(storage: Arc<dyn Storage>) -> Started {
    let (jobs, rx) = mpsc::unbounded_channel();
    let (kept_tx, kept) = oneshot::channel();
    let done = tokio::spawn(write(storage.clone(), rx, kept_tx));
    Started { writer: Writer(Some(jobs)), reader: Reader(storage), kept, done }
}

async fn write(
    storage: Arc<dyn Storage>,
    mut jobs: mpsc::UnboundedReceiver<Job>,
    kept: oneshot::Sender<Vec<IssueDetail>>,
) {
    let read = blocking(&storage, |s| s.issues()).await.unwrap_or_default();
    let _ = kept.send(read);
    while let Some(job) = jobs.recv().await {
        blocking(&storage, move |s| run(s, job)).await;
    }
}

/// One job; what fails is logged, the rest of it still done.
fn run(storage: &dyn Storage, job: Job) {
    let saved: Vec<std::io::Result<()>> = match job {
        Job::Issues(won) => won.iter().map(|d| storage.save_issue(d)).collect(),
        Job::Touch(keys) => {
            storage.touch(&keys);
            vec![]
        }
        Job::Board(id, snapshot, at) => vec![storage.save_board(id.as_ref(), &snapshot, at)],
        Job::Layout(key, layout) => vec![storage.save_layout(&key, &layout)],
    };
    for e in saved.into_iter().filter_map(Result::err) {
        tracing::warn!("keeping what was read: {e}");
    }
}

async fn blocking<T: Send + 'static>(
    storage: &Arc<dyn Storage>,
    f: impl FnOnce(&dyn Storage) -> T + Send + 'static,
) -> Option<T> {
    let storage = storage.clone();
    tokio::task::spawn_blocking(move || f(storage.as_ref())).await.ok()
}
