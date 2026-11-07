//! What the application keeps between runs, for a warm start: the ports a
//! storage adapter implements. Only ever read back as a starting point; the
//! provider stays the truth.

use crate::board::BoardSnapshot;
use crate::service::IssueDetail;
use kolum_domain::{BoardId, FieldLayout, IssueKey};
use std::collections::BTreeMap;
use time::OffsetDateTime;

/// The boards as last synced, and the field layouts as last read.
pub trait Store: Send + Sync {
    /// The board and when the sync that produced it started; `None` is the
    /// provider's first board. Nothing kept, or nothing that reads, is none.
    fn board(&self, id: Option<&BoardId>) -> Option<(BoardSnapshot, OffsetDateTime)>;
    fn save_board(
        &self,
        id: Option<&BoardId>,
        snapshot: &BoardSnapshot,
        synced_at: OffsetDateTime,
    ) -> std::io::Result<()>;
    /// Every layout kept, by `layout_key`.
    fn layouts(&self) -> BTreeMap<String, FieldLayout>;
    fn save_layout(&self, key: &str, layout: &FieldLayout) -> std::io::Result<()>;
}

/// The issues as last read in full, by an opening or a prefetch. Only the
/// issue store holds it: one writer.
pub trait IssueArchive: Send + Sync {
    /// Every issue kept, but those neither read nor seen on a board for long.
    fn issues(&self) -> Vec<IssueDetail>;
    fn save_issue(&self, detail: &IssueDetail) -> std::io::Result<()>;
    /// These issues were seen on a board just now: they are kept as long as
    /// one just read. What is not kept is no matter.
    fn touch(&self, keys: &[IssueKey]);
}

/// Both, as one adapter gives them.
pub trait Storage: Store + IssueArchive {}

impl<T: Store + IssueArchive> Storage for T {}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::sync::Mutex;

    type Boards = BTreeMap<Option<BoardId>, (BoardSnapshot, OffsetDateTime)>;

    /// A store in memory, for the service's tests: shared by two services
    /// as a directory is by two runs.
    #[derive(Default)]
    pub struct Memory {
        boards: Mutex<Boards>,
        layouts: Mutex<BTreeMap<String, FieldLayout>>,
        issues: Mutex<BTreeMap<IssueKey, IssueDetail>>,
        pub touched: Mutex<Vec<IssueKey>>,
        saves: std::sync::atomic::AtomicUsize,
    }

    impl Store for Memory {
        fn board(&self, id: Option<&BoardId>) -> Option<(BoardSnapshot, OffsetDateTime)> {
            self.boards.lock().unwrap().get(&id.cloned()).cloned()
        }
        fn save_board(
            &self,
            id: Option<&BoardId>,
            snapshot: &BoardSnapshot,
            synced_at: OffsetDateTime,
        ) -> std::io::Result<()> {
            self.boards.lock().unwrap().insert(id.cloned(), (snapshot.clone(), synced_at));
            Ok(())
        }
        fn layouts(&self) -> BTreeMap<String, FieldLayout> {
            self.layouts.lock().unwrap().clone()
        }
        fn save_layout(&self, key: &str, layout: &FieldLayout) -> std::io::Result<()> {
            self.layouts.lock().unwrap().insert(key.into(), layout.clone());
            Ok(())
        }
    }

    impl IssueArchive for Memory {
        fn issues(&self) -> Vec<IssueDetail> {
            self.issues.lock().unwrap().values().cloned().collect()
        }
        fn save_issue(&self, detail: &IssueDetail) -> std::io::Result<()> {
            self.saves.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            self.issues.lock().unwrap().insert(detail.issue.key.clone(), detail.clone());
            Ok(())
        }
        fn touch(&self, keys: &[IssueKey]) {
            self.touched.lock().unwrap().extend_from_slice(keys);
        }
    }

    impl Memory {
        /// How many issues were written so far.
        pub fn saves(&self) -> usize {
            self.saves.load(std::sync::atomic::Ordering::SeqCst)
        }

        pub fn issue(&self, key: &IssueKey) -> Option<IssueDetail> {
            self.issues.lock().unwrap().get(key).cloned()
        }
    }
}
