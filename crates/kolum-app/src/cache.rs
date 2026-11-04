//! The board as last seen, on disk, for a warm start: the screen shows it
//! at once while the sync catches up. Only ever read back as a starting
//! point; the provider stays the truth.

use crate::board::BoardSnapshot;
use kolum_domain::{Board, FieldLayout, Issue};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use time::OffsetDateTime;

/// Bumped when the shape of what is stored changes: an older file is then
/// ignored, not misread.
const VERSION: u32 = 1;

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

impl Cache {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Cache { dir: dir.into() }
    }

    fn path(&self, key: &str) -> PathBuf {
        self.dir.join(file_name(key))
    }

    /// The snapshot and when the sync that produced it started. A file
    /// that is missing, unreadable or from another version is no cache.
    pub fn load(&self, key: &str) -> Option<(BoardSnapshot, OffsetDateTime)> {
        let text = std::fs::read_to_string(self.path(key)).ok()?;
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

    /// Written beside and renamed into place, so a crash mid-write leaves
    /// the previous file rather than half of a new one.
    pub fn save(
        &self,
        key: &str,
        snapshot: &BoardSnapshot,
        synced_at: OffsetDateTime,
    ) -> std::io::Result<()> {
        std::fs::create_dir_all(&self.dir)?;
        let entry = Entry {
            version: VERSION,
            synced_at,
            board: snapshot.board.clone(),
            issues: snapshot.issues.clone(),
        };
        self.write(key, &serde_json::to_vec(&entry)?)
    }

    /// Written beside and renamed into place, so a crash mid-write leaves
    /// the previous file rather than half of a new one.
    fn write(&self, key: &str, bytes: &[u8]) -> std::io::Result<()> {
        std::fs::create_dir_all(&self.dir)?;
        let path = self.path(key);
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, bytes)?;
        std::fs::rename(&tmp, &path)
    }

    /// The field layout of a project's issue type, as the provider last gave it.
    pub fn save_layout(&self, key: &str, layout: &FieldLayout) -> std::io::Result<()> {
        let entry = LayoutEntry { version: VERSION, key: key.into(), layout: layout.clone() };
        self.write(&format!("{LAYOUT_FILE}{key}"), &serde_json::to_vec(&entry)?)
    }

    /// Every layout kept, by project and issue type; what does not read is
    /// left out.
    pub fn layouts(&self) -> BTreeMap<String, FieldLayout> {
        let Ok(files) = std::fs::read_dir(&self.dir) else { return BTreeMap::new() };
        files
            .flatten()
            .filter(|f| f.file_name().to_string_lossy().starts_with(LAYOUT_FILE))
            .filter_map(|f| {
                let entry: LayoutEntry =
                    serde_json::from_str(&std::fs::read_to_string(f.path()).ok()?).ok()?;
                (entry.version == VERSION).then_some((entry.key, entry.layout))
            })
            .collect()
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::load_board;
    use kolum_provider_fake::FakeProvider;

    pub fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("kolum-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[tokio::test]
    async fn a_snapshot_round_trips() {
        let dir = temp_dir("cache-trip");
        let cache = Cache::new(&dir);
        assert!(cache.load("1").is_none(), "nothing yet");
        let snapshot = load_board(&FakeProvider::new(), None).await.unwrap();
        let at = time::macros::datetime!(2026-11-03 10:00 UTC);
        cache.save("1", &snapshot, at).unwrap();
        assert_eq!(cache.load("1"), Some((snapshot, at)));
        assert!(!dir.join("1.json.tmp").exists());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_damaged_or_older_file_is_no_cache() {
        let dir = temp_dir("cache-bad");
        std::fs::create_dir_all(&dir).unwrap();
        let cache = Cache::new(&dir);
        std::fs::write(dir.join("1.json"), "{ not json").unwrap();
        assert!(cache.load("1").is_none());
        std::fs::write(
            dir.join("2.json"),
            r#"{"version":0,"synced_at":"2026-01-01T00:00:00Z","board":null,"issues":[]}"#,
        )
        .unwrap();
        assert!(cache.load("2").is_none());
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

    #[test]
    fn any_board_id_makes_a_file_name() {
        assert_eq!(file_name("42"), "42.json");
        assert_eq!(file_name("../etc/passwd"), "___etc_passwd.json");
        assert_eq!(file_name("Org/Project 12"), "Org_Project_12.json");
    }
}
