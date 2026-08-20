//! Persistent "reviewed ✓" marks for the git-diff modal.
//!
//! Pure nebula-side bookkeeping: marking a file never stages it or runs any
//! git command — marks live in `reviewed.json` beside `config.json` in the
//! data dir. Reads of the repo (HEAD, diffs) stay in git_diff.rs; this
//! module only stores and validates what the user approved.
//!
//! Scoping and reset rules:
//! - Marks are keyed by worktree path and scoped to the HEAD OID they were
//!   made under. Any HEAD move — commit, amend, checkout, rebase — makes
//!   `load_marks` return nothing for that worktree: after a commit the next
//!   diff is new work to review.
//! - Each mark carries a fingerprint of the diff text the user approved.
//!   The open path (`open_diff_view`) recomputes the fingerprint and drops
//!   marks whose diff changed since, so a file an agent kept editing comes
//!   back unreviewed.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Whole `reviewed.json`: worktree path → its marks.
#[derive(Debug, Default, Serialize, Deserialize)]
struct StoreFile {
    #[serde(default)]
    worktrees: HashMap<String, WorktreeMarks>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct WorktreeMarks {
    /// HEAD OID the marks were made under; empty for an unborn HEAD.
    #[serde(default)]
    head: String,
    /// File path (relative to the worktree root) → diff fingerprint.
    #[serde(default)]
    files: HashMap<String, u64>,
}

/// FNV-1a 64 of the diff text. Deliberately hand-rolled: `DefaultHasher`
/// is only stable within one std release, and these hashes are persisted.
pub fn fingerprint(text: &str) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in text.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100_0000_01b3);
    }
    hash
}

/// The marks stored for `worktree`, or empty when none were saved or they
/// were made under a different HEAD. Read-only — a stale entry is left in
/// place until the next `store_marks` overwrites it.
pub fn load_marks(worktree: &Path, head: &str) -> HashMap<String, u64> {
    let store = read_store(&store_path());
    match store.worktrees.get(worktree.to_string_lossy().as_ref()) {
        Some(marks) if marks.head == head => marks.files.clone(),
        _ => HashMap::new(),
    }
}

/// Replace `worktree`'s marks (an empty map removes its entry). Load-modify-
/// write so parallel TUIs only clobber each other's marks for the *same*
/// worktree; entries for worktrees gone from disk are pruned on the way.
/// Persistence failures are logged, not surfaced — the in-modal state still
/// works for this run.
pub fn store_marks(worktree: &Path, head: &str, files: &HashMap<String, u64>) {
    let path = store_path();
    let mut store = read_store(&path);
    store.worktrees.retain(|root, _| Path::new(root).is_dir());
    let key = worktree.to_string_lossy().into_owned();
    if files.is_empty() {
        store.worktrees.remove(&key);
    } else {
        store.worktrees.insert(
            key,
            WorktreeMarks {
                head: head.to_string(),
                files: files.clone(),
            },
        );
    }
    if let Err(err) = write_store(&path, &store) {
        tracing::warn!("failed to save {}: {err}", path.display());
    }
}

fn read_store(path: &Path) -> StoreFile {
    let Ok(raw) = std::fs::read_to_string(path) else {
        return StoreFile::default();
    };
    serde_json::from_str(&raw).unwrap_or_else(|err| {
        tracing::warn!("ignoring malformed {}: {err}", path.display());
        StoreFile::default()
    })
}

fn write_store(path: &Path, store: &StoreFile) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut bytes = serde_json::to_vec_pretty(store)
        .map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidData, err))?;
    if !bytes.ends_with(b"\n") {
        bytes.push(b'\n');
    }
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, &bytes)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

fn store_path() -> PathBuf {
    #[cfg(test)]
    {
        if let Some(path) = STORE_PATH_OVERRIDE.with(|p| p.borrow().clone()) {
            return path;
        }
    }
    nebula_core::paths::data_dir().join("reviewed.json")
}

#[cfg(test)]
thread_local! {
    static STORE_PATH_OVERRIDE: std::cell::RefCell<Option<PathBuf>> =
        const { std::cell::RefCell::new(None) };
}

#[cfg(test)]
pub fn with_store_path<T>(path: PathBuf, f: impl FnOnce() -> T) -> T {
    STORE_PATH_OVERRIDE.with(|slot| {
        let prev = slot.replace(Some(path));
        let out = f();
        slot.replace(prev);
        out
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fingerprint_is_stable_and_content_sensitive() {
        // Persisted across runs — the constants below must never drift.
        assert_eq!(fingerprint(""), 0xcbf2_9ce4_8422_2325);
        assert_eq!(fingerprint("+a"), fingerprint("+a"));
        assert_ne!(fingerprint("+a"), fingerprint("+b"));
    }

    #[test]
    fn marks_roundtrip_and_reset_on_head_change() {
        let dir = tempfile::tempdir().unwrap();
        let wt = dir.path().join("wt");
        std::fs::create_dir(&wt).unwrap();
        with_store_path(dir.path().join("reviewed.json"), || {
            let marks = HashMap::from([("a.rs".to_string(), 7_u64)]);
            store_marks(&wt, "head1", &marks);
            assert_eq!(load_marks(&wt, "head1"), marks);
            // A commit moves HEAD; the old marks no longer apply.
            assert!(load_marks(&wt, "head2").is_empty());
            // An unknown worktree has no marks.
            assert!(load_marks(&wt.join("other"), "head1").is_empty());
        });
    }

    #[test]
    fn empty_marks_remove_the_entry_and_dead_worktrees_are_pruned() {
        let dir = tempfile::tempdir().unwrap();
        let wt1 = dir.path().join("wt1");
        let wt2 = dir.path().join("wt2");
        std::fs::create_dir(&wt1).unwrap();
        std::fs::create_dir(&wt2).unwrap();
        with_store_path(dir.path().join("reviewed.json"), || {
            let marks = HashMap::from([("a.rs".to_string(), 7_u64)]);
            store_marks(&wt1, "h", &marks);
            store_marks(&wt2, "h", &marks);

            store_marks(&wt1, "h", &HashMap::new());
            assert!(load_marks(&wt1, "h").is_empty(), "empty map removes entry");
            assert_eq!(load_marks(&wt2, "h"), marks);

            // A deleted worktree's entry is dropped on the next write.
            std::fs::remove_dir(&wt2).unwrap();
            store_marks(&wt1, "h", &marks);
            std::fs::create_dir(&wt2).unwrap();
            assert!(load_marks(&wt2, "h").is_empty(), "dead entry pruned");
        });
    }

    #[test]
    fn malformed_store_is_ignored_and_recovered() {
        let dir = tempfile::tempdir().unwrap();
        let wt = dir.path().join("wt");
        std::fs::create_dir(&wt).unwrap();
        let path = dir.path().join("reviewed.json");
        std::fs::write(&path, "not json").unwrap();
        with_store_path(path, || {
            assert!(load_marks(&wt, "h").is_empty());
            let marks = HashMap::from([("a.rs".to_string(), 1_u64)]);
            store_marks(&wt, "h", &marks);
            assert_eq!(load_marks(&wt, "h"), marks);
        });
    }
}
