//! What the pull-request lookups learned, kept on disk between launches.
//!
//! Every answer `gh` gives — each checkout's own pull request (the PR ROW),
//! each project's open list (the PROJECT OPEN PRS GROUP), the body and
//! conversation the pane read, the diff the modal showed — is remembered
//! here, so the next launch paints all of it at once from what the last
//! one knew, and the lookups that would otherwise leave the panels blank
//! for their first seconds run underneath to bring it up to date. Stale for
//! a moment beats empty for a moment: a row that says `merged` in purple
//! before the sweep has confirmed it is still the right row to be looking
//! at.
//!
//! The cache is never the source of truth. GitHub is, and every hydrated
//! entry is re-asked on the same beats a fresh answer would be — the
//! project's list on arrival, the selected checkout on the next tick, its
//! neighbours one per tick, a cached body the moment the cursor rests on
//! its row. What lands replaces what was hydrated; what fails to land
//! leaves it (see `pull_request::Lookup`).
//!
//! Layout, under the DATA DIR (`NEBULA_DATA_DIR` isolates it for tests and
//! parallel instances, like everything else there):
//!
//! ```text
//! pr-cache/pull-requests.json   rows, lists and bodies, one document
//! pr-cache/diffs/<url>.diff     one whole `gh pr diff` per pull request
//! ```
//!
//! The document is rewritten whole, atomically, whenever something in it
//! changed — at most once per GIT POLL, and once more on quit. Diffs are
//! big and change on their own schedule, so each lives in its own file,
//! read only when `g` asks for it, and pruned along with the document to
//! the pull requests still on some row.
//!
//! Nothing here touches the disk unless an [`App`] carries a [`PrCache`]:
//! the main loop installs one at startup, the unit tests never do, so no
//! test can read or clobber the real user's cache.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use nebula_core::{ProjectId, WorktreeId};
use serde::{Deserialize, Serialize};

use crate::app::{App, OpenPrs, OPEN_PRS_MIN_AGE};
use crate::event_loop::{OPEN_PRS_RECHECK_MIN, OPEN_PRS_REFRESH};
use crate::pull_request::{OpenPr, PrDetail, PullRequest};

/// Directory under the DATA DIR everything below lives in.
const DIR: &str = "pr-cache";
/// The one document: rows, lists and bodies.
const STORE_FILE: &str = "pull-requests.json";
/// Where the per-pull-request diffs go, inside [`DIR`].
const DIFFS_DIR: &str = "diffs";
/// Bumped when the document's shape changes incompatibly; an older
/// document is ignored rather than half-read. Field additions don't need
/// it — `#[serde(default)]` covers those.
const VERSION: u32 = 1;

/// The document on disk. Keyed the way the app keys the same things:
/// checkout rows by worktree id, open lists by project id, bodies by URL.
/// Only *found* pull requests are written for the checkouts — a checkout
/// without one paints the same whether the fact is remembered or not, and
/// remembering it would only stop a new PR from showing up until the
/// backoff expired.
#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
pub struct Store {
    pub version: u32,
    /// Unix seconds; informational, for anyone reading the file.
    pub saved_at: u64,
    #[serde(default)]
    pub worktrees: HashMap<WorktreeId, PullRequest>,
    #[serde(default)]
    pub projects: HashMap<ProjectId, Vec<OpenPr>>,
    #[serde(default)]
    pub details: HashMap<String, PrDetail>,
}

/// Where this instance keeps its cache. Cheap to clone: it is a path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrCache {
    root: PathBuf,
}

impl PrCache {
    /// A cache rooted at `root` — the tests' way in, with a temp dir.
    pub fn at(root: PathBuf) -> Self {
        Self { root }
    }

    /// The real one: `<data dir>/pr-cache`.
    pub fn default_location() -> Self {
        Self::at(nebula_core::paths::data_dir().join(DIR))
    }

    fn store_path(&self) -> PathBuf {
        self.root.join(STORE_FILE)
    }

    fn diffs_dir(&self) -> PathBuf {
        self.root.join(DIFFS_DIR)
    }

    /// The document, when there is one this build can read. A missing,
    /// malformed or older-versioned file is simply no cache.
    pub fn load_store(&self) -> Option<Store> {
        let path = self.store_path();
        let raw = std::fs::read_to_string(&path).ok()?;
        let store: Store = match serde_json::from_str(&raw) {
            Ok(store) => store,
            Err(err) => {
                tracing::warn!("ignoring malformed {}: {err}", path.display());
                return None;
            }
        };
        (store.version == VERSION).then_some(store)
    }

    /// Write the document whole. Atomic — a temp file beside it, then a
    /// rename — so a crash mid-write leaves the previous document, not
    /// half of the new one.
    pub fn save_store(&self, store: &Store) -> std::io::Result<()> {
        let path = self.store_path();
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
        std::fs::rename(&tmp, &path)
    }

    fn diff_path(&self, url: &str) -> PathBuf {
        self.diffs_dir().join(diff_file_name(url))
    }

    /// The last diff read for `url`, if one was kept. The file's first line
    /// is the URL it was fetched for, and a file whose line disagrees is
    /// not this pull request's — two URLs can sanitise to one name — so it
    /// is a miss, never someone else's diff.
    pub fn load_diff(&self, url: &str) -> Option<String> {
        let raw = std::fs::read_to_string(self.diff_path(url)).ok()?;
        let (header, diff) = raw.split_once('\n')?;
        (header == url).then(|| diff.to_string())
    }

    /// Keep `diff` as the last one read for `url`. Atomic, like the
    /// document.
    pub fn store_diff(&self, url: &str, diff: &str) -> std::io::Result<()> {
        let path = self.diff_path(url);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let tmp = path.with_extension("diff.tmp");
        std::fs::write(&tmp, format!("{url}\n{diff}"))?;
        std::fs::rename(&tmp, &path)
    }

    /// Drop every diff file that isn't one of `live`'s — the pull requests
    /// still on some row — and any temp file a crash left behind. The
    /// directory is nebula's own, so anything in it that isn't a live diff
    /// is garbage by definition.
    pub fn prune_diffs(&self, live: &HashSet<String>) {
        let keep: HashSet<String> = live.iter().map(|url| diff_file_name(url)).collect();
        let Ok(entries) = std::fs::read_dir(self.diffs_dir()) else {
            return;
        };
        for entry in entries.flatten() {
            let name = entry.file_name();
            if !keep.contains(name.to_string_lossy().as_ref()) {
                let _ = std::fs::remove_file(entry.path());
            }
        }
    }
}

/// A file name for a pull request's diff: the URL with everything that
/// isn't a letter or digit turned into `_`. Readable in a directory
/// listing, safe on every filesystem, and unique enough for GitHub's
/// `owner/repo/pull/N` shape — [`PrCache::load_diff`] checks the URL it
/// finds inside the file anyway.
pub fn diff_file_name(url: &str) -> String {
    let stem: String = url
        .trim_start_matches("https://")
        .trim_start_matches("http://")
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect();
    format!("{stem}.diff")
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// Startup: read the document and paint the app from it. Every hydrated
/// entry is armed to be re-asked as soon as its beat allows — see
/// [`install`].
pub fn hydrate(app: &mut App) {
    let Some(store) = app.pr_cache.as_ref().and_then(PrCache::load_store) else {
        return;
    };
    install(app, store);
}

/// Put a document's contents into the app, without touching anything the
/// app already knows — a lookup that has answered beats a cached copy of
/// the same row.
///
/// * Checkout rows land as-is. Their recheck timers are left unarmed, so
///   the selected checkout is re-asked on the first tick and its
///   neighbours one per tick after — the same first minute a launch with
///   no cache spends, only with the rows already painted.
/// * Open lists land *due*: `at` sits past [`OPEN_PRS_MIN_AGE`] so arriving
///   at the project re-asks at once, and the step is the one a fresh answer
///   of the same shape would have left, so the backoff picks up where the
///   last run's did.
/// * Bodies land marked stale (`pr_detail_stale`): the pane shows them the
///   instant the cursor rests on the row, and the same rest that would
///   have fetched a missing body fetches a fresh copy over the top.
pub fn install(app: &mut App, store: Store) {
    for (worktree, pr) in store.worktrees {
        app.pull_requests.entry(worktree).or_insert(Some(pr));
    }
    let now = std::time::Instant::now();
    let at = now.checked_sub(OPEN_PRS_MIN_AGE).unwrap_or(now);
    for (project, list) in store.projects {
        if app.open_prs.contains_key(&project) {
            continue;
        }
        let step = if list.is_empty() {
            OPEN_PRS_RECHECK_MIN
        } else {
            OPEN_PRS_REFRESH
        };
        app.open_prs.insert(
            project,
            OpenPrs {
                list,
                at,
                due: now,
                step,
            },
        );
    }
    for (url, detail) in store.details {
        if app.pr_detail.contains_key(&url) {
            continue;
        }
        app.pr_detail.insert(url.clone(), detail);
        app.pr_detail_stale.insert(url);
    }
    app.dirty = true;
}

/// The document as the app would write it now.
pub fn snapshot(app: &App) -> Store {
    Store {
        version: VERSION,
        saved_at: now_secs(),
        worktrees: app
            .pull_requests
            .iter()
            .filter_map(|(worktree, pr)| Some((worktree.clone(), pr.clone()?)))
            .collect(),
        projects: app
            .open_prs
            .iter()
            .map(|(project, open)| (project.clone(), open.list.clone()))
            .collect(),
        details: app.pr_detail.clone(),
    }
}

/// Everything a flush writes: the cache to write to, the document, and the
/// pull requests whose diffs may stay. `None` when nothing changed since
/// the last flush or the app has no cache; either way the dirty flag is
/// taken, so the caller — the loop, off-thread; the quit path, inline —
/// only ever writes once per change.
pub fn take_flush(app: &mut App) -> Option<(PrCache, Store, HashSet<String>)> {
    if !std::mem::take(&mut app.pr_cache_dirty) {
        return None;
    }
    let cache = app.pr_cache.clone()?;
    Some((cache, snapshot(app), app.live_pr_urls()))
}

/// Write a flush out: the document, then the diff prune. Failures are
/// logged, never surfaced — a cache that couldn't be written is a slower
/// next launch, not a broken one.
pub fn write_all(cache: &PrCache, store: &Store, live: &HashSet<String>) {
    if let Err(err) = cache.save_store(store) {
        tracing::warn!("pull-request cache not written: {err}");
    }
    cache.prune_diffs(live);
}

/// Keep the diff just read for `url`, when the app has a cache. Only a
/// diff with files in it is worth keeping: an empty one is a flash, not a
/// modal, and would be re-fetched to find that out anyway.
pub fn remember_diff(app: &App, url: &str, diff: &str) {
    let Some(cache) = &app.pr_cache else {
        return;
    };
    if crate::pull_request::split_unified_diff(diff).is_empty() {
        return;
    }
    if let Err(err) = cache.store_diff(url, diff) {
        tracing::warn!("pull-request diff not cached: {err}");
    }
}

/// The diff last read for `url`, when the app has a cache and kept one.
/// Read on the loop: it is a local file, and `g` is waiting on it.
pub fn recall_diff(app: &App, url: &str) -> Option<String> {
    app.pr_cache.as_ref()?.load_diff(url)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pull_request::{PrComment, STATE_MERGED, STATE_OPEN};

    fn cache() -> (tempfile::TempDir, PrCache) {
        let dir = tempfile::tempdir().unwrap();
        let cache = PrCache::at(dir.path().join("pr-cache"));
        (dir, cache)
    }

    fn pr(number: u64, state: &str) -> PullRequest {
        PullRequest {
            number,
            url: format!("https://github.com/o/r/pull/{number}"),
            title: format!("PR {number}"),
            state: state.into(),
            is_draft: false,
            activity: vec!["2024-04-25T19:55:42Z".into()],
        }
    }

    fn open(number: u64) -> OpenPr {
        OpenPr {
            number,
            title: format!("PR {number}"),
            url: format!("https://github.com/o/r/pull/{number}"),
            is_draft: number % 2 == 1,
            head: format!("head-{number}"),
        }
    }

    fn detail(number: u64) -> PrDetail {
        PrDetail {
            number,
            url: format!("https://github.com/o/r/pull/{number}"),
            title: format!("PR {number}"),
            state: STATE_OPEN.into(),
            is_draft: false,
            author: "kate".into(),
            base: "main".into(),
            head: format!("head-{number}"),
            additions: 1,
            deletions: 2,
            changed_files: 3,
            body: "Closes #1\n\nMakes the row.".into(),
            comments: vec![PrComment {
                author: "steiza".into(),
                at: "2024-04-26T21:44:55Z".into(),
                review_state: "APPROVED".into(),
                body: "nice".into(),
            }],
        }
    }

    fn store() -> Store {
        Store {
            version: VERSION,
            saved_at: 1,
            worktrees: [
                (WorktreeId("w1".into()), pr(7, STATE_OPEN)),
                (WorktreeId("w2".into()), pr(8, STATE_MERGED)),
            ]
            .into_iter()
            .collect(),
            projects: [
                (ProjectId("p1".into()), vec![open(7), open(9)]),
                (ProjectId("p2".into()), vec![]),
            ]
            .into_iter()
            .collect(),
            details: [(detail(7).url.clone(), detail(7))].into_iter().collect(),
        }
    }

    /// The document survives the disk byte for byte — ids as JSON keys,
    /// the conversation's timestamps, a merged state, an empty list — and
    /// a missing file is simply no cache.
    #[test]
    fn the_document_round_trips() {
        let (_dir, cache) = cache();
        assert!(cache.load_store().is_none(), "nothing written yet");
        let store = store();
        cache.save_store(&store).unwrap();
        let back = cache.load_store().expect("readable");
        assert_eq!(back, store);
        assert!(
            !cache.store_path().with_extension("json.tmp").exists(),
            "the temp file was renamed into place"
        );
    }

    /// A document from a build with a different shape, or one that isn't
    /// JSON at all, is ignored rather than half-read.
    #[test]
    fn a_foreign_or_broken_document_is_no_cache() {
        let (_dir, cache) = cache();
        let mut store = store();
        store.version = VERSION + 1;
        cache.save_store(&store).unwrap();
        assert!(cache.load_store().is_none(), "a newer version is not ours");
        std::fs::write(cache.store_path(), "{not json").unwrap();
        assert!(cache.load_store().is_none());
    }

    /// Hydration paints the rows, arms every list to be re-asked at once
    /// and marks every body stale — and never overwrites what a lookup
    /// already answered.
    #[test]
    fn hydration_paints_rows_and_arms_their_refresh() {
        let mut app = App::new();
        // A lookup that already answered for w1 wins over the cache.
        app.pull_requests
            .insert(WorktreeId("w1".into()), Some(pr(70, STATE_OPEN)));
        install(&mut app, store());

        assert_eq!(
            app.pull_requests[&WorktreeId("w1".into())]
                .as_ref()
                .map(|p| p.number),
            Some(70),
            "the live answer stays"
        );
        assert_eq!(
            app.pull_requests[&WorktreeId("w2".into())]
                .as_ref()
                .map(|p| p.badge()),
            Some("merged"),
            "a cached merged row paints purple from the first frame"
        );
        assert!(
            app.pr_lookup_due(&WorktreeId("w2".into())),
            "a hydrated row is re-asked on its first turn"
        );

        let p1 = ProjectId("p1".into());
        let p2 = ProjectId("p2".into());
        assert_eq!(app.open_prs[&p1].list.len(), 2);
        assert!(app.open_prs_lookup_due(&p1), "hydrated lists are due");
        assert_eq!(app.open_prs[&p1].step, OPEN_PRS_REFRESH);
        assert_eq!(
            app.open_prs[&p2].step, OPEN_PRS_RECHECK_MIN,
            "an empty list resumes the backoff, not the steady beat"
        );
        assert!(
            app.open_prs[&p1].at + OPEN_PRS_MIN_AGE <= std::time::Instant::now(),
            "old enough that arriving at the project re-asks at once"
        );

        let url = detail(7).url;
        assert!(app.pr_detail.contains_key(&url));
        assert!(app.pr_detail_stale.contains(&url));
        assert!(app.dirty);
    }

    /// What is written is what the app knows, minus the checkouts known to
    /// have no pull request — remembering those would only hide a new one.
    #[test]
    fn the_snapshot_writes_found_rows_only() {
        let mut app = App::new();
        install(&mut app, store());
        app.pull_requests.insert(WorktreeId("w3".into()), None);
        let snap = snapshot(&app);
        assert_eq!(snap.version, VERSION);
        let mut ids: Vec<&str> = snap.worktrees.keys().map(|w| w.as_str()).collect();
        ids.sort();
        assert_eq!(ids, ["w1", "w2"]);
        assert_eq!(snap.projects.len(), 2);
        assert_eq!(snap.details.len(), 1);
    }

    /// A flush is taken once per change, and only by an app with a cache.
    #[test]
    fn a_flush_is_taken_once_per_change() {
        let mut app = App::new();
        app.pr_cache_dirty = true;
        assert!(take_flush(&mut app).is_none(), "no cache, nothing to write");
        assert!(!app.pr_cache_dirty, "but the flag is spent");

        let (_dir, cache) = cache();
        app.pr_cache = Some(cache.clone());
        assert!(take_flush(&mut app).is_none(), "clean");
        app.pr_cache_dirty = true;
        let (to, store, _live) = take_flush(&mut app).expect("dirty");
        assert_eq!(to, cache);
        assert_eq!(store.version, VERSION);
        assert!(take_flush(&mut app).is_none(), "spent");
    }

    /// Diffs come back for their own URL only, and the prune keeps just
    /// the live pull requests' — temp files and strangers included.
    #[test]
    fn diffs_round_trip_per_url_and_prune_to_the_live_set() {
        let (_dir, cache) = cache();
        let seven = "https://github.com/o/r/pull/7";
        let nine = "https://github.com/o/r/pull/9";
        assert!(cache.load_diff(seven).is_none());
        cache.store_diff(seven, "diff --git a/x b/x\n+y\n").unwrap();
        cache.store_diff(nine, "diff --git a/z b/z\n+w\n").unwrap();
        assert_eq!(
            cache.load_diff(seven).as_deref(),
            Some("diff --git a/x b/x\n+y\n")
        );
        // A file whose header names another URL is a miss, not a wrong
        // diff — the sanitised names can collide, the headers can't.
        std::fs::write(
            cache.diff_path("https://github.com/o/r/pull/8"),
            "https://github.com/o_r/pull/8\nnot yours\n",
        )
        .unwrap();
        assert!(cache.load_diff("https://github.com/o/r/pull/8").is_none());
        std::fs::write(cache.diffs_dir().join("leftover.diff.tmp"), "x").unwrap();

        cache.prune_diffs(&[seven.to_string()].into_iter().collect());
        let mut left: Vec<String> = std::fs::read_dir(cache.diffs_dir())
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        left.sort();
        assert_eq!(left, [diff_file_name(seven)]);
        assert!(cache.load_diff(nine).is_none());
    }

    /// The file name is the URL, readable and filesystem-safe.
    #[test]
    fn diff_file_names_are_readable_and_safe() {
        assert_eq!(
            diff_file_name("https://github.com/o/r/pull/42"),
            "github_com_o_r_pull_42.diff"
        );
        assert_eq!(
            diff_file_name("http://ghe.corp/Org-1/re.po/pull/1"),
            "ghe_corp_Org_1_re_po_pull_1.diff"
        );
    }

    /// The app-level helpers are no-ops without a cache, and only keep a
    /// diff that has files in it.
    #[test]
    fn diff_helpers_need_a_cache_and_a_real_diff() {
        let mut app = App::new();
        let url = "https://github.com/o/r/pull/7";
        remember_diff(&app, url, "diff --git a/x b/x\n+y\n");
        assert!(recall_diff(&app, url).is_none(), "no cache, nothing kept");

        let (_dir, cache) = cache();
        app.pr_cache = Some(cache);
        remember_diff(&app, url, "");
        assert!(
            recall_diff(&app, url).is_none(),
            "an empty diff is not kept"
        );
        remember_diff(&app, url, "diff --git a/x b/x\n+y\n");
        assert_eq!(
            recall_diff(&app, url).as_deref(),
            Some("diff --git a/x b/x\n+y\n")
        );
    }
}
