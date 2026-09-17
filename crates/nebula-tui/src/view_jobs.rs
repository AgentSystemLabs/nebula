//! BACKGROUND READS for the worktree views — the DIFF VIEWER (`g`), the FILE
//! FINDER (`f`), its grep view (`F`) and the TREE BROWSER (`b`).
//!
//! Every one of them is git and the disk: `git status -uall` to list what
//! changed, `git ls-files` for the finder and the tree, a `git diff` per
//! file walked past, a `git grep` per character typed, a file read and
//! highlighted per tree row. They used to run inside the key handler, on
//! the grounds that they are fast — and on a small checkout they are: 10 to
//! 50 ms each under the INPUT LATENCY PROBE. On a ten-thousand-file
//! checkout the same `git status` is 80 ms warm and over a second cold, and
//! `git grep` is 200 ms per keystroke, all of it with the whole UI frozen:
//! no paint, no PTY output, no next key.
//!
//! So the views ask and the answer lands: a view that holds a [`Jobs`]
//! handle runs its read on the blocking pool and is handed the result by
//! the main loop (`event_loop::land_view_answer`), keyed by a [`ticket`]
//! so an answer nobody is waiting for any more — the query moved on, the
//! cursor left the file, the modal closed — is dropped. A view without a
//! handle (every unit test that builds one directly, and nothing else)
//! reads inline exactly as before, so the two paths share every parser.
//!
//! Nothing here holds memory between reads beyond what the view already
//! showed; the one cache — the DIFF VIEWER's — is bounded and dies with
//! the modal (`DiffView::cache`).

use crate::git_diff::DiffFile;
use crate::grep_search::GrepHit;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

/// How long a view keeps showing what it had while the replacement loads.
/// A read that lands inside it swaps the content in place — no blank
/// frame between two files that each take a few milliseconds; one that
/// does not is told so ([`Answer::Slow`]) and says "loading…" instead of
/// leaving the last file's text under the next file's name.
pub const STALE_GRACE: Duration = Duration::from_millis(60);

/// How long the grep view waits for the next character before it spends a
/// `git grep` on the query: a word typed at speed searches once, for the
/// word.
pub const GREP_DEBOUNCE: Duration = Duration::from_millis(40);

/// The changed-file list behind the DIFF VIEWER, with everything else its
/// opening used to read inline.
#[derive(Debug)]
pub struct DiffListing {
    pub files: Vec<DiffFile>,
    /// HEAD's OID; None on an unborn HEAD.
    pub head: Option<String>,
    /// The reviewed ✓ marks that still apply — each stored mark checked
    /// against the file's diff as it is now.
    pub reviewed: HashMap<String, u64>,
}

#[derive(Debug)]
pub enum Answer {
    Grep {
        ticket: u64,
        result: Result<(Vec<GrepHit>, bool), String>,
    },
    /// `git ls-files`, for the FILE FINDER and the TREE BROWSER.
    Files {
        ticket: u64,
        result: Result<Vec<String>, String>,
    },
    DiffListing {
        ticket: u64,
        result: Result<DiffListing, String>,
    },
    /// One file's diff text. `prefetch` is the row after the cursor, read
    /// ahead: it goes into the view's cache and is not shown.
    DiffText {
        /// The `DiffView::id` that asked.
        view: u64,
        ticket: u64,
        path: String,
        diff: String,
        prefetch: bool,
    },
    Preview {
        ticket: u64,
        preview: Box<crate::tree_browser::Preview>,
    },
    /// [`STALE_GRACE`] is up on `ticket`.
    Slow { ticket: u64 },
    /// The system clipboard could not be written off the loop: hand the
    /// copy to the terminal instead (OSC 52), and say so.
    ClipboardViaTerminal { payload: String, flash: String },
    /// This process's resident set, for the footer's memory readout and the
    /// memory modal: a `ps`, on a five-second beat.
    ClientRss(u64),
    /// The outcome of something a key started and did not wait for, in the
    /// footer's words (`Shift+G`: which page was opened, or why not).
    Flash(String),
}

/// A view's way onto the blocking pool and back. Cheap to clone — the
/// views are cloned every frame.
#[derive(Debug, Clone)]
pub struct Jobs {
    tx: tokio::sync::mpsc::UnboundedSender<Answer>,
}

/// A fresh ticket. One counter for every view, so an answer can never be
/// mistaken for another modal's.
pub fn ticket() -> u64 {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

/// Set when the view stops waiting on a read, so a job that has not
/// started its git yet never does, and one that is streaming stops.
#[derive(Debug, Clone, Default)]
pub struct Cancel(Arc<AtomicBool>);

impl Cancel {
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Relaxed);
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }
}

impl Jobs {
    pub fn new(tx: tokio::sync::mpsc::UnboundedSender<Answer>) -> Self {
        Self { tx }
    }

    /// Run `work` on the blocking pool and land what it returns. Must be
    /// called from inside the runtime — which is where every key handler
    /// runs; the unit tests that have no runtime have no `Jobs` either.
    pub fn run(&self, work: impl FnOnce() -> Option<Answer> + Send + 'static) {
        let tx = self.tx.clone();
        tokio::task::spawn_blocking(move || {
            if let Some(answer) = work() {
                let _ = tx.send(answer);
            }
        });
    }

    /// [`Jobs::run`], with [`Answer::Slow`] landing first when the work
    /// outlasts [`STALE_GRACE`].
    pub fn run_with_grace(
        &self,
        ticket: u64,
        work: impl FnOnce() -> Option<Answer> + Send + 'static,
    ) {
        let done = Cancel::default();
        let (tx, finished) = (self.tx.clone(), done.clone());
        tokio::spawn(async move {
            tokio::time::sleep(STALE_GRACE).await;
            if !finished.is_cancelled() {
                let _ = tx.send(Answer::Slow { ticket });
            }
        });
        self.run(move || {
            let answer = work();
            done.cancel();
            answer
        });
    }
}
