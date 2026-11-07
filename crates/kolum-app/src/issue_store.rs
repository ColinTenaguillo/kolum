//! The issues read in full, held by one task: the only one that decides
//! which read of an issue is current. It holds each current copy shared
//! with the TUI, which shows it, and hands the copies that win to the
//! writer, the only one that writes them.

use crate::service::{Event, IssueDetail, load_issue};
use crate::writer::{Job, Writer};
use kolum_domain::{IssueKey, Provider};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use time::OffsetDateTime;
use tokio::sync::{mpsc, oneshot, watch};

/// The board's issues and when each was last updated, as the board has them.
pub(crate) type Wanted = Vec<(IssueKey, OffsetDateTime)>;

/// How whole a read is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ReadBy {
    /// An opening's: every comment and change, the children, the layout.
    Opening,
    /// A bulk read's, or one kept by an earlier run: its comments and
    /// history maybe cut short, no children nor layout of its own.
    Bulk,
}

/// The read current for an issue, the same copy the TUI was sent, and
/// what deciding on the next one needs.
#[derive(Debug, Clone, PartialEq)]
struct Held {
    current: Arc<IssueDetail>,
    by: ReadBy,
    /// Seen on a board this run, or written: its file is kept a month.
    seen: bool,
    /// The version an older opening had the issue read again for: once.
    asked_again: Option<OffsetDateTime>,
}

/// `arrived` as it lands over what is held: `Ok` with it when it wins,
/// `Err` with it when it loses. The newer copy wins; the same version again
/// only when it is an opening's (the same `updated` is the same version,
/// as for `supersedes`), so a bulk read never replaces a whole read of the
/// same version. A bulk read has no children nor layout: it takes those
/// held.
fn landed(
    held: Option<&Held>,
    mut arrived: IssueDetail,
    by: ReadBy,
) -> Result<IssueDetail, Box<IssueDetail>> {
    let Some(held) = held else { return Ok(arrived) };
    let current = &held.current;
    let wins = match by {
        ReadBy::Opening => arrived.issue.updated >= current.issue.updated,
        ReadBy::Bulk => arrived.issue.updated > current.issue.updated,
    };
    if !wins {
        return Err(Box::new(arrived));
    }
    if by == ReadBy::Bulk {
        arrived.children = current.children.clone();
        arrived.layout = current.layout.clone();
    }
    Ok(arrived)
}

/// A bulk read's copy and an opening's older read of the issue as one: the
/// copy's issue, the comments and the history of both, each once and in
/// order, and the opening's children and layout, which the copy has not.
fn merged(bulk: &IssueDetail, opening: IssueDetail) -> IssueDetail {
    let mut comments = bulk.comments.clone();
    let new = |c: &kolum_domain::Comment| !bulk.comments.iter().any(|b| b.id == c.id);
    comments.extend(opening.comments.into_iter().filter(new));
    comments.sort_by_key(|c| c.created);
    let mut activity = bulk.activity.clone();
    activity.extend(opening.activity.into_iter().filter(|a| !bulk.activity.contains(a)));
    activity.sort_by_key(|a| a.at);
    let (layout, children) = (opening.layout, opening.children);
    IssueDetail { issue: bulk.issue.clone(), comments, activity, layout, children }
}

/// Of the board's issues, those not held as new as the board's copy.
fn stale(held: &HashMap<IssueKey, Held>, wanted: Wanted) -> Wanted {
    let fresh = |key: &IssueKey, at| held.get(key).is_some_and(|h| h.current.issue.updated >= at);
    wanted.into_iter().filter(|(key, at)| !fresh(key, *at)).collect()
}

enum Landing {
    /// The issues earlier runs kept, read once at the start.
    Kept(Vec<IssueDetail>),
    Opened(Box<IssueDetail>),
    Bulk(Vec<IssueDetail>),
    /// A board's issues, seen on it just now.
    Seen(Vec<IssueKey>),
    /// Asks which of a board's issues to read again.
    Stale(Wanted, oneshot::Sender<Wanted>),
}

/// The handle on the task that holds the reads.
#[derive(Clone)]
pub(crate) struct IssueStore(mpsc::UnboundedSender<Landing>);

impl IssueStore {
    /// Starts the task. With a writer, `kept` brings what earlier runs kept,
    /// sent on as `ReadIssues`; until then the board's questions wait, the
    /// openings do not. An older opening has the issue read again from
    /// `provider`, by the store itself.
    pub fn spawn(
        writer: Option<(Writer, oneshot::Receiver<Vec<IssueDetail>>)>,
        events: mpsc::UnboundedSender<Event>,
        provider: Arc<dyn Provider>,
    ) -> Self {
        let (landings, rx) = mpsc::unbounded_channel();
        let (writer, waiting) = match writer {
            Some((writer, kept)) => {
                let landing = landings.clone();
                tokio::spawn(async move {
                    let _ = landing.send(Landing::Kept(kept.await.unwrap_or_default()));
                });
                (writer, Some(vec![]))
            }
            None => (Writer::default(), None),
        };
        let again = Again { provider, landings: landings.downgrade() };
        let holder = Holder { held: HashMap::new(), events, writer, again, waiting };
        tokio::spawn(hold(holder, rx));
        IssueStore(landings)
    }

    /// One whose task has ended, for tests of what happens then.
    #[cfg(test)]
    pub fn ended() -> Self {
        IssueStore(mpsc::unbounded_channel().0)
    }

    /// An opening's read: `IssueLoaded` answers it with the copy current
    /// after it. Given back when the store is gone.
    pub fn opened(&self, detail: IssueDetail) -> Result<(), Box<IssueDetail>> {
        match self.0.send(Landing::Opened(Box::new(detail))) {
            Err(mpsc::error::SendError(Landing::Opened(detail))) => Err(detail),
            _ => Ok(()),
        }
    }

    /// A prefetch's reads: those that win come back as `ReadIssues`.
    pub fn bulk(&self, read: Vec<IssueDetail>) {
        let _ = self.0.send(Landing::Bulk(read));
    }

    /// A board's issues: those held are kept as long as if just read.
    pub fn seen(&self, keys: Vec<IssueKey>) {
        let _ = self.0.send(Landing::Seen(keys));
    }

    /// The board's issues not held as new as the board has them; `None`
    /// once the store is gone.
    pub async fn stale(&self, wanted: Wanted) -> Option<Wanted> {
        let (reply, answer) = oneshot::channel();
        self.0.send(Landing::Stale(wanted, reply)).ok()?;
        answer.await.ok()
    }
}

/// The store's own reads of an issue: what they read lands as an opening's;
/// a failure is only logged, as no one waits on it.
struct Again {
    provider: Arc<dyn Provider>,
    landings: mpsc::WeakUnboundedSender<Landing>,
}

impl Again {
    fn read(&self, key: IssueKey) {
        let Some(landings) = self.landings.upgrade() else { return };
        let provider = self.provider.clone();
        tokio::spawn(async move {
            match load_issue(provider.as_ref(), &key).await {
                Ok(detail) => {
                    let _ = landings.send(Landing::Opened(Box::new(detail)));
                }
                Err(e) => tracing::info!("reading {key} again: {e}"),
            }
        });
    }
}

struct Holder {
    held: HashMap<IssueKey, Held>,
    events: mpsc::UnboundedSender<Event>,
    writer: Writer,
    again: Again,
    /// The board's questions until what earlier runs kept is in.
    waiting: Option<Vec<Landing>>,
}

async fn hold(mut holder: Holder, mut landings: mpsc::UnboundedReceiver<Landing>) {
    while let Some(landing) = landings.recv().await {
        holder.take(landing);
    }
}

impl Holder {
    fn take(&mut self, landing: Landing) {
        if let Some(waiting) = &mut self.waiting
            && matches!(landing, Landing::Seen(_) | Landing::Stale(..))
        {
            return waiting.push(landing);
        }
        match landing {
            Landing::Kept(read) => self.kept(read),
            Landing::Opened(detail) => self.opened(*detail),
            Landing::Bulk(read) => self.bulk(read),
            Landing::Seen(keys) => self.seen(keys),
            Landing::Stale(wanted, reply) => {
                let _ = reply.send(stale(&self.held, wanted));
            }
        }
    }

    /// `arrived` among the reads: the copy current when it won, given back
    /// when not. One read this run is written now, which keeps its file as
    /// being seen on a board does.
    fn land(
        &mut self,
        arrived: IssueDetail,
        by: ReadBy,
        now: bool,
    ) -> Result<Arc<IssueDetail>, Box<IssueDetail>> {
        let key = arrived.issue.key.clone();
        let held = self.held.get(&key);
        let seen = now || held.is_some_and(|h| h.seen);
        let asked_again = held.and_then(|h| h.asked_again);
        let current = Arc::new(landed(held, arrived, by)?);
        self.held.insert(key, Held { current: current.clone(), by, seen, asked_again });
        Ok(current)
    }

    /// What earlier runs kept, on disk already: what wins goes on to the
    /// TUI, then the board's questions that waited are answered.
    fn kept(&mut self, read: Vec<IssueDetail>) {
        let won: Vec<_> =
            read.into_iter().filter_map(|d| self.land(d, ReadBy::Bulk, false).ok()).collect();
        if !won.is_empty() {
            let _ = self.events.send(Event::ReadIssues(won));
        }
        for landing in self.waiting.take().unwrap_or_default() {
            self.take(landing);
        }
    }

    fn opened(&mut self, detail: IssueDetail) {
        let (current, changed) = match self.land(detail, ReadBy::Opening, true) {
            Ok(won) => (won, true),
            Err(older) => self.older(*older),
        };
        let _ = self.events.send(Event::IssueLoaded(current.clone()));
        if changed {
            self.writer.send(Job::Issues(vec![current]));
        }
    }

    /// An opening's read older than the copy held. A whole copy stays as
    /// it is. A bulk read's becomes one with the opening's: its comments and
    /// history are cut short, its children and layout none; and, a newer
    /// whole version existing, the issue is read whole again, once for that
    /// version. The copy current after, and whether it changed.
    fn older(&mut self, older: IssueDetail) -> (Arc<IssueDetail>, bool) {
        let key = older.issue.key.clone();
        let held = self.held.get_mut(&key).expect("a read loses only to one held");
        if held.by == ReadBy::Opening {
            return (held.current.clone(), false);
        }
        held.current = Arc::new(merged(&held.current, older));
        let version = held.current.issue.updated;
        let current = held.current.clone();
        if held.asked_again.replace(version) != Some(version) {
            self.again.read(key);
        }
        (current, true)
    }

    fn bulk(&mut self, read: Vec<IssueDetail>) {
        let won: Vec<_> =
            read.into_iter().filter_map(|d| self.land(d, ReadBy::Bulk, true).ok()).collect();
        if won.is_empty() {
            return;
        }
        let _ = self.events.send(Event::ReadIssues(won.clone()));
        self.writer.send(Job::Issues(won));
    }

    /// Each issue held is touched the first time a board shows it in a
    /// run: once is enough to keep its file a month.
    fn seen(&mut self, keys: Vec<IssueKey>) {
        let first = |key: &IssueKey, held: &mut HashMap<IssueKey, Held>| {
            held.get_mut(key).is_some_and(|h| !std::mem::replace(&mut h.seen, true))
        };
        let keys: Vec<IssueKey> = keys.into_iter().filter(|k| first(k, &mut self.held)).collect();
        if !keys.is_empty() {
            self.writer.send(Job::Touch(keys));
        }
    }
}

/// The board's issues read in full in the background, so each opens whole:
/// one task, fed the latest board, reading what the store does not hold
/// as new; each batch goes to the store as it is read. A refusal (a rate
/// limit most likely) or a failure ends it for the run, keeping what was
/// read, so that the user's own requests pass; so does the store's end.
pub(crate) async fn prefetch(
    provider: Arc<dyn Provider>,
    store: IssueStore,
    mut boards: watch::Receiver<Wanted>,
) {
    // Asked and not read, refused or unreadable, at the version asked: not
    // asked again until it changes.
    let mut unread: HashMap<IssueKey, OffsetDateTime> = HashMap::new();
    while boards.changed().await.is_ok() {
        let wanted = boards.borrow_and_update().clone();
        let Some(stale) = store.stale(wanted).await else { return };
        let ask: Wanted = stale.into_iter().filter(|(k, at)| unread.get(k) != Some(at)).collect();
        if ask.is_empty() {
            continue;
        }
        let keys: Vec<IssueKey> = ask.iter().map(|(key, _)| key.clone()).collect();
        let mut got = HashSet::new();
        let mut take = |read: Vec<kolum_domain::IssueInFull>| {
            got.extend(read.iter().map(|r| r.issue.key.clone()));
            store.bulk(read.into_iter().map(IssueDetail::from).collect());
        };
        if let Err(error) = provider.issues_in_full(&keys, &mut take).await {
            tracing::info!("prefetching stopped for this run: {error}");
            return;
        }
        unread.extend(ask.into_iter().filter(|(key, _)| !got.contains(key)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::IssueArchive;
    use crate::store::tests::Memory;
    use kolum_domain::IssueReader;
    use kolum_provider_fake::FakeProvider;

    async fn read(key: &str, comments: usize) -> IssueDetail {
        let fake = FakeProvider::new();
        let key = IssueKey::parse(key);
        let mut detail = IssueDetail {
            issue: fake.issue(&key).await.unwrap(),
            comments: fake.comments(&key).await.unwrap(),
            activity: vec![],
            layout: Some(kolum_domain::FieldLayout::default()),
            children: vec![fake.issue(&IssueKey::parse("ACME-6")).await.unwrap().as_ref()],
        };
        detail.comments.truncate(comments);
        detail
    }

    fn cut(mut detail: IssueDetail) -> IssueDetail {
        (detail.layout, detail.children) = (None, vec![]);
        detail
    }

    fn later(mut detail: IssueDetail) -> IssueDetail {
        detail.issue.updated += time::Duration::minutes(1);
        detail
    }

    fn held(detail: &IssueDetail, by: ReadBy) -> Held {
        Held { current: Arc::new(detail.clone()), by, seen: false, asked_again: None }
    }

    /// The newest copy wins; at the same version an opening's wins, never
    /// a bulk read's over it; a bulk read takes the children and layout.
    #[tokio::test]
    async fn the_newest_read_wins_and_a_bulk_read_never_replaces_a_whole_one() {
        use ReadBy::{Bulk, Opening};
        let whole = read("ACME-5", 3).await;
        let bulk = cut(read("ACME-5", 2).await);
        let (opened, bulked) = (held(&whole, Opening), held(&bulk, Bulk));
        assert_eq!(landed(None, bulk.clone(), Bulk), Ok(bulk.clone()), "nothing held");
        let same = landed(Some(&opened), bulk.clone(), Bulk);
        assert_eq!(same, Err(Box::new(bulk.clone())), "same version");
        assert_eq!(landed(Some(&bulked), whole.clone(), Opening), Ok(whole.clone()), "whole");
        assert_eq!(landed(Some(&opened), whole.clone(), Opening), Ok(whole.clone()), "again");
        assert!(landed(Some(&bulked), bulk.clone(), Bulk).is_err(), "the same bulk read again");

        let newer = cut(later(read("ACME-5", 2).await));
        let won = landed(Some(&opened), newer.clone(), Bulk).unwrap();
        assert_eq!(won.issue.updated, newer.issue.updated, "newer");
        assert_eq!(won.children, whole.children, "the opening's children stay");
        assert_eq!(won.layout, whole.layout);
        assert_eq!(won.comments, newer.comments);
        let newer = held(&newer, Bulk);
        assert_eq!(landed(Some(&newer), whole.clone(), Opening), Err(Box::new(whole)), "older");
    }

    #[tokio::test]
    async fn stale_is_what_the_board_has_newer_than_held() {
        let five = read("ACME-5", 0).await;
        let at = five.issue.updated;
        let kept = HashMap::from([(five.issue.key.clone(), held(&five, ReadBy::Bulk))]);
        let (k5, k6) = (IssueKey::parse("ACME-5"), IssueKey::parse("ACME-6"));
        let minute = time::Duration::minutes(1);
        let s = stale(&kept, vec![(k5.clone(), at), (k6.clone(), at)]);
        assert_eq!(s, [(k6.clone(), at)], "not held");
        assert!(stale(&kept, vec![(k5.clone(), at - minute)]).is_empty(), "held newer");
        assert_eq!(stale(&kept, vec![(k5.clone(), at + minute)]), [(k5, at + minute)], "changed");
    }

    /// A bulk read's copy and an older opening's read as one: the copy's
    /// issue, the comments and history of both once each, in order, and
    /// the opening's children and layout.
    #[tokio::test]
    async fn an_older_opening_and_a_bulk_copy_merge_their_comments_and_history() {
        let whole = read("ACME-5", 3).await;
        let mut bulk = cut(later(read("ACME-5", 1).await));
        let late = kolum_domain::Comment {
            id: "c-late".into(),
            created: bulk.issue.updated,
            ..whole.comments[0].clone()
        };
        bulk.comments.push(late.clone());
        let change = |minutes| kolum_domain::ActivityEntry {
            author: late.author.clone(),
            at: whole.issue.created + time::Duration::minutes(minutes),
            changes: vec![],
        };
        let mut opening = whole.clone();
        opening.activity = vec![change(1), change(2)];
        bulk.activity = vec![change(2), change(3)];
        let one = merged(&bulk, opening);
        assert_eq!(one.issue, bulk.issue, "the copy's issue");
        let ids: Vec<&str> = one.comments.iter().map(|c| c.id.as_str()).collect();
        let mut wanted: Vec<&str> = whole.comments.iter().map(|c| c.id.as_str()).collect();
        wanted.push("c-late");
        assert_eq!(ids, wanted, "both, once each, in order");
        assert_eq!(one.activity, [change(1), change(2), change(3)]);
        assert_eq!((one.children, one.layout), (whole.children, whole.layout));
    }

    /// An epic opened in a read older than a bulk read kept: it shows the
    /// two as one, which is kept for the next run, and is read again by
    /// the store once for that version; read again, older again, it is not
    /// read a third time.
    #[tokio::test]
    async fn an_older_opening_is_merged_kept_and_read_again_once() {
        let memory = Arc::new(Memory::default());
        let fake: Arc<dyn Provider> = Arc::new(FakeProvider::new());
        let epic = IssueKey::parse("ACME-1");
        let opening = load_issue(fake.as_ref(), &epic).await.unwrap();
        let bulk = cut(later(opening.clone()));
        memory.save_issue(&bulk).unwrap();
        let started = crate::writer::start(memory.clone());
        let (events, mut answers) = mpsc::unbounded_channel();
        let store = IssueStore::spawn(Some((started.writer, started.kept)), events, fake.clone());
        let Some(Event::ReadIssues(_)) = answers.recv().await else { panic!("the kept copy") };
        store.opened(opening.clone()).unwrap();
        let shown = |event: Option<Event>| match event {
            Some(Event::IssueLoaded(detail)) => detail,
            other => panic!("{other:?}"),
        };
        let one = shown(answers.recv().await);
        assert_eq!((one.issue.updated, &one.children), (bulk.issue.updated, &opening.children));
        let again = shown(answers.recv().await);
        assert_eq!(again, one, "read again by the store, older again: as it was");
        let wait = std::time::Duration::from_millis(300);
        assert!(tokio::time::timeout(wait, answers.recv()).await.is_err(), "not a third time");

        // Read whole since, newer: an older opening then leaves it as it is.
        let whole = later(later(opening.clone()));
        store.opened(whole.clone()).unwrap();
        assert_eq!(*shown(answers.recv().await), whole);
        store.opened(opening.clone()).unwrap();
        assert_eq!(*shown(answers.recv().await), whole, "the whole copy stays");
        drop(store);
        started.done.await.unwrap();
        // The bulk copy, the merged one twice, the whole one: not the older.
        assert_eq!(memory.saves(), 4, "nothing written for the older one");
        let next_run = crate::writer::start(memory.clone());
        let kept = next_run.kept.await.unwrap();
        assert_eq!(kept.iter().find(|d| d.issue.key == epic), Some(&whole));
    }

    /// The merged copy is kept for the next run.
    #[tokio::test]
    async fn an_older_opening_merged_is_kept_for_the_next_run() {
        let memory = Arc::new(Memory::default());
        let fake: Arc<dyn Provider> = Arc::new(FakeProvider::new());
        let epic = IssueKey::parse("ACME-1");
        let opening = load_issue(fake.as_ref(), &epic).await.unwrap();
        let bulk = cut(later(opening.clone()));
        memory.save_issue(&bulk).unwrap();
        let started = crate::writer::start(memory.clone());
        let (events, mut answers) = mpsc::unbounded_channel();
        let store = IssueStore::spawn(Some((started.writer, started.kept)), events, fake.clone());
        let Some(Event::ReadIssues(_)) = answers.recv().await else { panic!("the kept copy") };
        store.opened(opening.clone()).unwrap();
        let Some(Event::IssueLoaded(one)) = answers.recv().await else { panic!() };
        drop(store);
        started.done.await.unwrap();
        let next_run = crate::writer::start(memory.clone());
        let kept = next_run.kept.await.unwrap();
        assert_eq!(kept.iter().find(|d| d.issue.key == epic), Some(&*one), "kept as shown");
    }
}
