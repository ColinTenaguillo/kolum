//! What the application keeps between runs, on disk: one directory per
//! profile, one JSON file per board, field layout and issue. The adapter
//! of kolum-app's `Store` and `IssueArchive`.

use kolum_app::{BoardSnapshot, IssueArchive, IssueDetail, Store};
use kolum_domain::{Board, BoardId, FieldLayout, Issue, IssueKey};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use time::OffsetDateTime;

/// Bumped when the shape of what is stored changes: an older file is then
/// ignored, not misread. 2: fields by the domain's ids (`estimate`, `due`).
/// 3: a copy says whether it carries the description; a Jira Cloud board's
/// kept by 2 has an empty one, which 3 would read as emptied.
const VERSION: u32 = 3;
/// The layouts' own: 3 left their shape as it was, so they stay at 2.
const LAYOUT_VERSION: u32 = 2;

#[derive(Serialize, Deserialize)]
struct Entry {
    version: u32,
    #[serde(with = "time::serde::rfc3339")]
    synced_at: OffsetDateTime,
    board: Board,
    issues: Vec<Issue>,
}

/// A field layout, one file per project and issue type: two issues opened
/// at once write two files, never the same one.
#[derive(Serialize, Deserialize)]
struct LayoutEntry {
    version: u32,
    key: String,
    layout: FieldLayout,
}

const LAYOUT_FILE: &str = "layout-";

/// An issue as last read in full, by its opening or a prefetch, one file
/// each: the fields the board does not carry, its comments and history.
#[derive(Serialize, Deserialize)]
struct IssueEntry {
    version: u32,
    detail: IssueDetail,
}

const ISSUE_FILE: &str = "issue-";
/// An issue neither read nor seen on a board for this long is forgotten:
/// its file's time is when it was last either.
const ISSUE_DAYS: u64 = 30;

fn read_issue(path: &Path) -> Option<IssueDetail> {
    let entry: IssueEntry = serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()?;
    (entry.version == VERSION).then_some(entry.detail)
}

/// One directory per profile: two Jira instances can both have a board 42.
#[derive(Debug, Clone)]
pub struct Cache {
    dir: PathBuf,
}

/// A board id as a file name, whatever the provider put in it.
fn file_name(key: &str) -> String {
    let safe: String = key
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '_' })
        .collect();
    format!("{safe}.json")
}

/// A file being written that is older than this is what a crash left.
const ABANDONED: std::time::Duration = std::time::Duration::from_secs(3600);

impl Cache {
    /// Opens the directory, removing what a crash mid-write left in it.
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        let cache = Cache { dir: dir.into() };
        cache.sweep();
        cache
    }

    fn sweep(&self) {
        let Ok(files) = std::fs::read_dir(&self.dir) else { return };
        let abandoned = |f: &std::fs::DirEntry| {
            let age = f.metadata().and_then(|m| m.modified()).ok().and_then(|t| t.elapsed().ok());
            f.file_name().to_string_lossy().ends_with(".tmp") && age.is_some_and(|a| a > ABANDONED)
        };
        for file in files.flatten().filter(abandoned) {
            let _ = std::fs::remove_file(file.path());
        }
    }

    fn path(&self, key: &str) -> PathBuf {
        self.dir.join(file_name(key))
    }

    /// Written beside and renamed into place, so a crash mid-write leaves
    /// the previous file rather than half of a new one. Each write has its
    /// own file beside, by process and by write: two at once, of one file,
    /// never write into the same one, and the last renamed wins whole.
    fn write(&self, key: &str, bytes: &[u8]) -> std::io::Result<()> {
        static WRITES: AtomicU64 = AtomicU64::new(0);
        std::fs::create_dir_all(&self.dir)?;
        let path = self.path(key);
        let n = WRITES.fetch_add(1, Ordering::Relaxed);
        let tmp = path.with_extension(format!("json.{}.{n}.tmp", std::process::id()));
        std::fs::write(&tmp, bytes)?;
        std::fs::rename(&tmp, &path).inspect_err(|_| {
            let _ = std::fs::remove_file(&tmp);
        })
    }

    /// The files whose name starts with `prefix`, but those being written.
    fn files(&self, prefix: &str) -> Vec<std::fs::DirEntry> {
        let Ok(files) = std::fs::read_dir(&self.dir) else { return vec![] };
        let kept = |f: &std::fs::DirEntry| {
            let name = f.file_name();
            let name = name.to_string_lossy();
            name.starts_with(prefix) && name.ends_with(".json")
        };
        files.flatten().filter(kept).collect()
    }
}

/// The file of a board: its id, or `first` for "the provider's first".
fn board_key(id: Option<&BoardId>) -> String {
    id.map_or("first".into(), |id| id.to_string())
}

impl Store for Cache {
    /// The snapshot and when the sync that produced it started. A file
    /// that is missing, unreadable or from another version is no cache.
    fn board(&self, id: Option<&BoardId>) -> Option<(BoardSnapshot, OffsetDateTime)> {
        let key = board_key(id);
        let text = std::fs::read_to_string(self.path(&key)).ok()?;
        match serde_json::from_str::<Entry>(&text) {
            Ok(e) if e.version == VERSION => {
                Some((BoardSnapshot { board: e.board, issues: e.issues }, e.synced_at))
            }
            Ok(_) => None,
            Err(e) => {
                tracing::warn!("ignoring the cache for {key}: {e}");
                None
            }
        }
    }

    fn save_board(
        &self,
        id: Option<&BoardId>,
        snapshot: &BoardSnapshot,
        synced_at: OffsetDateTime,
    ) -> std::io::Result<()> {
        let entry = Entry {
            version: VERSION,
            synced_at,
            board: snapshot.board.clone(),
            issues: snapshot.issues.clone(),
        };
        self.write(&board_key(id), &serde_json::to_vec(&entry)?)
    }

    /// Every layout kept, by project and issue type; what does not read is
    /// left out.
    fn layouts(&self) -> BTreeMap<String, FieldLayout> {
        let read = |f: std::fs::DirEntry| {
            let entry: LayoutEntry =
                serde_json::from_str(&std::fs::read_to_string(f.path()).ok()?).ok()?;
            (entry.version == LAYOUT_VERSION).then_some((entry.key, entry.layout))
        };
        self.files(LAYOUT_FILE).into_iter().filter_map(read).collect()
    }

    /// The field layout of a project's issue type, as the provider last gave it.
    fn save_layout(&self, key: &str, layout: &FieldLayout) -> std::io::Result<()> {
        let entry =
            LayoutEntry { version: LAYOUT_VERSION, key: key.into(), layout: layout.clone() };
        self.write(&format!("{LAYOUT_FILE}{key}"), &serde_json::to_vec(&entry)?)
    }
}

impl IssueArchive for Cache {
    /// Every issue kept; the files of those neither read nor seen on a
    /// board for `ISSUE_DAYS` are removed, what does not read is left out.
    fn issues(&self) -> Vec<IssueDetail> {
        let max_age = std::time::Duration::from_secs(ISSUE_DAYS * 86_400);
        let fresh = |f: &std::fs::DirEntry| {
            let age = f.metadata().and_then(|m| m.modified()).ok().and_then(|t| t.elapsed().ok());
            let old = age.is_some_and(|a| a > max_age);
            if old {
                let _ = std::fs::remove_file(f.path());
            }
            !old
        };
        self.files(ISSUE_FILE)
            .into_iter()
            .filter(fresh)
            .filter_map(|f| read_issue(&f.path()))
            .collect()
    }

    fn save_issue(&self, detail: &IssueDetail) -> std::io::Result<()> {
        let entry = IssueEntry { version: VERSION, detail: detail.clone() };
        self.write(&format!("{ISSUE_FILE}{}", detail.issue.key), &serde_json::to_vec(&entry)?)
    }

    /// Each file's time set to now, which its age goes by.
    fn touch(&self, keys: &[IssueKey]) {
        let now = std::time::SystemTime::now();
        for key in keys {
            let file =
                std::fs::File::options().write(true).open(self.path(&format!("{ISSUE_FILE}{key}")));
            if let Err(e) = file.and_then(|f| f.set_modified(now)) {
                tracing::debug!("touching {key}: {e}");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kolum_app::load_board;
    use kolum_provider_fake::FakeProvider;

    fn one() -> BoardId {
        BoardId::new("1")
    }

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("kolum-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[tokio::test]
    async fn a_snapshot_round_trips() {
        let dir = temp_dir("cache-trip");
        let cache = Cache::new(&dir);
        assert!(cache.board(Some(&one())).is_none(), "nothing yet");
        let snapshot = load_board(&FakeProvider::new(), None).await.unwrap();
        let at = time::macros::datetime!(2026-11-03 10:00 UTC);
        cache.save_board(Some(&one()), &snapshot, at).unwrap();
        assert_eq!(cache.board(Some(&one())), Some((snapshot, at)));
        let left: Vec<_> =
            std::fs::read_dir(&dir).unwrap().flatten().map(|f| f.file_name()).collect();
        assert_eq!(left, ["1.json"], "no temporary file left behind");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn a_key_is_kept_as_its_text_and_read_back_with_its_project() {
        let dir = temp_dir("cache-keys");
        let cache = Cache::new(&dir);
        let snapshot = load_board(&FakeProvider::new(), None).await.unwrap();
        cache.save_board(Some(&one()), &snapshot, time::OffsetDateTime::now_utc()).unwrap();
        let file = std::fs::read_to_string(dir.join("1.json")).unwrap();
        assert!(file.contains(r#""key":"ACME-12""#), "a string, as files already on disk have");
        let github = file.replace(r#""ACME-12""#, r#""python-language-server#12""#);
        std::fs::write(dir.join("1.json"), github).unwrap();
        let (loaded, _) = cache.board(Some(&one())).unwrap();
        let key = loaded.issues.iter().map(|i| &i.key).find(|k| k.as_str().ends_with("#12"));
        assert_eq!(key.and_then(IssueKey::project), Some("python-language-server"));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_damaged_or_older_file_is_no_cache() {
        let dir = temp_dir("cache-bad");
        std::fs::create_dir_all(&dir).unwrap();
        let cache = Cache::new(&dir);
        std::fs::write(dir.join("1.json"), "{ not json").unwrap();
        assert!(cache.board(Some(&one())).is_none());
        std::fs::write(
            dir.join("2.json"),
            r#"{"version":0,"synced_at":"2026-01-01T00:00:00Z","board":null,"issues":[]}"#,
        )
        .unwrap();
        assert!(cache.board(Some(&BoardId::new("2"))).is_none());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn layouts_are_kept_one_file_each() {
        use kolum_domain::{FieldId, FieldLayout};
        let dir = temp_dir("cache-layouts");
        let cache = Cache::new(&dir);
        assert!(cache.layouts().is_empty(), "nothing yet");
        let bug = FieldLayout { side: vec![], body: vec![FieldId::new("description")] };
        let story = FieldLayout::default();
        cache.save_layout("ACME/Bug", &bug).unwrap();
        cache.save_layout("ACME/Story", &story).unwrap();
        std::fs::write(dir.join("1.json"), "a board, not a layout").unwrap();
        let wanted = std::collections::BTreeMap::from([
            ("ACME/Bug".to_string(), bug),
            ("ACME/Story".to_string(), story),
        ]);
        assert_eq!(cache.layouts(), wanted);
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// Version 1 kept Jira's own field ids (`customfield_10016` for the
    /// story points): its layouts are dropped, not read with the old ids.
    #[test]
    fn a_layout_from_before_the_domains_field_ids_is_dropped() {
        let dir = temp_dir("cache-old-layouts");
        let cache = Cache::new(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let old = r#"{"version":1,"key":"ACME/Bug","layout":{"side":[],"body":[]}}"#;
        std::fs::write(dir.join(file_name(&format!("{LAYOUT_FILE}ACME/Bug"))), old).unwrap();
        assert!(cache.layouts().is_empty());
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// What changed the boards' and issues' version left the layouts'
    /// shape as it was: one kept by version 2 still reads.
    #[test]
    fn a_layout_kept_by_version_2_still_reads() {
        let dir = temp_dir("cache-layouts-2");
        let cache = Cache::new(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let kept = r#"{"version":2,"key":"ACME/Bug","layout":{"side":[],"body":["description"]}}"#;
        std::fs::write(dir.join(file_name(&format!("{LAYOUT_FILE}ACME/Bug"))), kept).unwrap();
        let body = cache.layouts().get("ACME/Bug").map(|l| l.body.len());
        assert_eq!(body, Some(1));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn issues_are_kept_one_file_each_for_thirty_days_since_read_or_seen() {
        let dir = temp_dir("cache-issues");
        let cache = Cache::new(&dir);
        assert!(cache.issues().is_empty(), "nothing yet");
        let snapshot = load_board(&FakeProvider::new(), None).await.unwrap();
        let detail = |issue: &Issue| IssueDetail {
            issue: issue.clone(),
            comments: vec![],
            activity: vec![],
            layout: None,
            children: vec![],
        };
        let (recent, old) = (detail(&snapshot.issues[0]), detail(&snapshot.issues[1]));
        let seen = detail(&snapshot.issues[2]);
        cache.save_issue(&recent).unwrap();
        cache.save_issue(&old).unwrap();
        cache.save_issue(&seen).unwrap();
        let old_file = dir.join(file_name(&format!("{ISSUE_FILE}{}", old.issue.key)));
        let days_ago = |file: &std::path::Path, days: u64| {
            let at = std::time::SystemTime::now() - std::time::Duration::from_secs(days * 86_400);
            std::fs::File::options().write(true).open(file).unwrap().set_modified(at).unwrap();
        };
        days_ago(&old_file, 31);
        days_ago(&dir.join(file_name(&format!("{ISSUE_FILE}{}", recent.issue.key))), 29);
        days_ago(&dir.join(file_name(&format!("{ISSUE_FILE}{}", seen.issue.key))), 31);
        cache.touch(&[seen.issue.key.clone(), IssueKey::parse("NOPE-1")]);
        std::fs::write(dir.join("1.json"), "a board, not an issue").unwrap();
        let mut kept = cache.issues();
        kept.sort_by_key(|d| d.issue.key.clone());
        let mut wanted = [recent, seen];
        wanted.sort_by_key(|d| d.issue.key.clone());
        assert_eq!(kept, wanted, "the one seen on a board just now is kept");
        assert!(!old_file.exists(), "an issue neither read nor seen for a month goes");
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// Two writers of one file at once, a LoadBoard and a SyncBoard of the
    /// same board say: each write goes through, and the file is one of them
    /// whole, never half of each.
    #[tokio::test]
    async fn concurrent_saves_of_one_board_all_succeed_and_leave_a_whole_file() {
        let dir = temp_dir("cache-concurrent");
        let cache = Cache::new(&dir);
        let snapshot = load_board(&FakeProvider::new(), None).await.unwrap();
        let at = time::macros::datetime!(2026-11-03 10:00 UTC);
        std::thread::scope(|s| {
            let writers: Vec<_> = (0..8)
                .map(|_| {
                    s.spawn(|| (0..25).map(|_| cache.save_board(None, &snapshot, at)).collect())
                })
                .collect();
            for writer in writers {
                let saved: std::io::Result<Vec<()>> = writer.join().unwrap();
                saved.unwrap();
            }
        });
        assert_eq!(cache.board(None), Some((snapshot, at)));
        let left: Vec<_> =
            std::fs::read_dir(&dir).unwrap().flatten().map(|f| f.file_name()).collect();
        assert_eq!(left, ["first.json"], "no temporary file left behind");
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// A crash between a write and its rename leaves the file beside: one
    /// older than an hour is removed when the cache is opened; one younger
    /// may be another run's, still writing.
    #[test]
    fn a_temporary_file_a_crash_left_is_removed_on_opening() {
        let dir = temp_dir("cache-crashed");
        std::fs::create_dir_all(&dir).unwrap();
        let (old, young) = (dir.join("1.json.9.0.tmp"), dir.join("1.json.9.1.tmp"));
        for file in [&old, &young] {
            std::fs::write(file, "half").unwrap();
        }
        let two_hours_ago = std::time::SystemTime::now() - std::time::Duration::from_secs(7200);
        std::fs::File::options()
            .write(true)
            .open(&old)
            .unwrap()
            .set_modified(two_hours_ago)
            .unwrap();
        Cache::new(&dir);
        assert!(!old.exists() && young.exists());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn any_board_id_makes_a_file_name() {
        assert_eq!(file_name("42"), "42.json");
        assert_eq!(file_name("../etc/passwd"), "___etc_passwd.json");
        assert_eq!(file_name("Org/Project 12"), "Org_Project_12.json");
    }
}
