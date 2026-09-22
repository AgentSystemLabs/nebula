//! The BRANCH SWITCHER (`c`): the project's ROOT WORKTREE moved onto
//! another branch without leaving nebula. A fuzzy list of every local
//! branch and every remote branch that has no local twin, filtered as you
//! type; `Enter` switches, or — with nothing matching — creates the typed
//! branch. A checkout with uncommitted changes stops and asks how they
//! should travel — the choices the IDEs offer at the same moment: stash
//! them, bring them along, commit them first, or throw them away.
//!
//! Only the root checkout switches. A linked worktree is named after the
//! branch it was cut for, and moving it would leave the directory name
//! lying about its contents; git itself refuses to check a branch out in
//! two places, so a branch another worktree holds is listed but refused.
//!
//! Everything here is client-side git, like the diff viewer: the list is
//! one `git for-each-ref` (milliseconds, painted from a per-checkout cache
//! on reopen), the changed-file count one `git status`, and opening the
//! modal starts a background `git fetch --all` — at most once a minute per
//! checkout — that re-lists when it lands. Every call runs off the loop
//! with its answer on `App::branch_switch.tx`, and none is stacked on one
//! still running. The DAEMON is not asked: its worktree sync already
//! notices a root `HEAD` that moved and upserts the row's branch; the TUI
//! renames the row itself the moment git says yes, so the panel never lags
//! the switch by a sync tick.
//!
//! Git that writes (the switch, a stash, a commit) and git that talks to a
//! remote run in a session of their own with stdin closed. The TUI owns a
//! terminal, and an `ssh` asking for a passphrase or a host key would
//! otherwise open `/dev/tty` and paint over the frame; detached, it fails
//! instead. (A pinentry that opens `$GPG_TTY` by path is beyond its reach.)
//!
//! Agents run git in the same repository while a switch runs, so nothing
//! here trusts a position or a moment: a stash entry is found by commit and
//! message, the prompt's list is fingerprinted so a discard never throws
//! away an edit it did not show, and a checkout git abandons part-way is
//! undone only for files byte-identical to the branch it was writing.

use std::collections::{HashMap, HashSet};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant, UNIX_EPOCH};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use nebula_core::WorktreeId;
use ratatui::layout::{Constraint, Layout, Position, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph};
use ratatui::Frame;

use crate::app::{clamp_selection, window_start, App, Focus, Overlay};
use crate::git_diff::{git_command, DiffFile};
use crate::text_input::TextInput;
use crate::theme::Theme;
use crate::ui::{
    centered_rect, empty_list_row, input_spans, modal_block, render_row, row_rect, search_line,
    visible_positions, NO_MATCHES,
};

/// Outer (width, height) of the modal: the find-file modal's footprint,
/// wider for the commit subjects.
const SIZE: (u16, u16) = (92, 24);
/// How long a background fetch may run. Generous — a fetch writes packs,
/// and one cut short starts over on the next open — but bounded, since a
/// stalled remote would otherwise hold it forever.
const FETCH_TIMEOUT: Duration = Duration::from_secs(120);
/// Between the SIGTERM that lets git remove its lock and temporary pack
/// files and the SIGKILL for a fetch that lingers.
const FETCH_GRACE: Duration = Duration::from_secs(2);
/// The least time between two background fetches of one checkout, so
/// opening and closing the modal never hammers a remote.
const FETCH_GAP: Duration = Duration::from_secs(60);
/// `git for-each-ref` record: NUL between fields, RS after each record (a
/// subject is one line, but a record separator costs nothing).
const REF_FORMAT: &str = "--format=%(refname)%00%(symref)%00%(HEAD)%00%(worktreepath)%00%(committerdate:unix)%00%(contents:subject)%1e";
/// Paths per `git hash-object` / `git checkout` call, well inside any
/// argument-length limit.
const PATH_CHUNK: usize = 200;

extern "C" {
    fn kill(pid: i32, sig: i32) -> i32;
}
const SIGTERM: i32 = 15;
const SIGKILL: i32 = 9;

// ---- width ----

/// Screen cells `s` takes: a gitmoji or a CJK subject is two a character.
fn cells(s: &str) -> usize {
    Span::raw(s).width()
}

/// `s` cut to `max` cells with a `…` marking the cut — `ui::truncate` by
/// cells rather than chars, so wide text can't push past its budget.
fn fit(s: &str, max: usize) -> String {
    if cells(s) <= max {
        return s.to_string();
    }
    let mut out = String::new();
    let mut used = 0;
    let mut buf = [0u8; 4];
    for c in s.chars() {
        let w = cells(c.encode_utf8(&mut buf));
        if used + w + 1 > max {
            break;
        }
        out.push(c);
        used += w;
    }
    if max > 0 {
        out.push('…');
    }
    out
}

// ---- git ----

/// One row: a local branch, or a remote-tracking branch nothing local
/// shadows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Branch {
    /// What the row shows and the filter matches: `feature-x`, or
    /// `origin/feature-x` for a remote branch.
    pub name: String,
    pub remote: bool,
    /// The branch the root checkout is on.
    pub current: bool,
    /// The other checkout a local branch is checked out in. git keeps a
    /// branch in one checkout at a time, so the row is refused.
    pub checked_out_at: Option<PathBuf>,
    /// Committer date of the tip, unix seconds.
    pub committed: i64,
    pub subject: String,
}

impl Branch {
    /// A branch the switch creates: nothing about it is known yet.
    fn new_local(name: String) -> Self {
        Self {
            name,
            remote: false,
            current: false,
            checked_out_at: None,
            committed: 0,
            subject: String::new(),
        }
    }

    /// The local branch a switch lands on: the name itself, or a remote
    /// branch's name without its remote (`origin/feature-x` → `feature-x`,
    /// what `git switch --track` creates).
    pub fn local_name(&self) -> &str {
        if self.remote {
            self.name
                .split_once('/')
                .map_or(&self.name, |(_, rest)| rest)
        } else {
            &self.name
        }
    }
}

/// Parse `git for-each-ref` output in [`REF_FORMAT`] into rows: the
/// current branch first, the other local branches next, then the remote
/// branches with no local branch of the same name — switching to one of
/// those would land on the local branch anyway. Within each group git's
/// newest-commit-first order holds. `origin/HEAD` points at a branch rather
/// than being one, and is dropped.
pub fn parse_refs(out: &str) -> Vec<Branch> {
    let mut local: Vec<Branch> = Vec::new();
    let mut remote: Vec<Branch> = Vec::new();
    for record in out.split('\x1e') {
        let record = record.trim_start_matches('\n');
        let mut fields = record.split('\0');
        let (Some(refname), Some(symref), Some(head), Some(worktree), Some(date)) = (
            fields.next(),
            fields.next(),
            fields.next(),
            fields.next(),
            fields.next(),
        ) else {
            continue;
        };
        let committed = date.trim().parse().unwrap_or(0);
        let subject = fields.next().unwrap_or("").trim().to_string();
        if let Some(name) = refname.strip_prefix("refs/heads/") {
            let current = head == "*";
            local.push(Branch {
                name: name.to_string(),
                remote: false,
                current,
                checked_out_at: (!current && !worktree.is_empty()).then(|| worktree.into()),
                committed,
                subject,
            });
        } else if let Some(name) = refname.strip_prefix("refs/remotes/") {
            if !symref.is_empty() || !name.contains('/') {
                continue;
            }
            remote.push(Branch {
                name: name.to_string(),
                remote: true,
                current: false,
                checked_out_at: None,
                committed,
                subject,
            });
        }
    }
    remote.retain(|r| !local.iter().any(|l| l.name == r.local_name()));
    // Stable: git's recency order survives within both halves.
    local.sort_by_key(|b| !b.current);
    local.extend(remote);
    local
}

/// git's complaint as one line: the first `error:` or `fatal:` it printed,
/// without the prefix, else the first thing it said at all.
fn git_error(stderr: &str) -> String {
    let lines: Vec<&str> = stderr
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect();
    let line = lines
        .iter()
        .find(|l| l.starts_with("error: ") || l.starts_with("fatal: "))
        .or(lines.first())
        .copied()
        .unwrap_or("git failed");
    line.strip_prefix("error: ")
        .or_else(|| line.strip_prefix("fatal: "))
        .unwrap_or(line)
        .to_string()
}

/// Read-only `git -C root <args>`: stdout, or git's one-line complaint.
fn read(root: &Path, args: &[&str]) -> Result<String, String> {
    let out = git_command(root)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("failed to run git: {e}"))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    } else {
        Err(git_error(&String::from_utf8_lossy(&out.stderr)))
    }
}

/// `git -C root <args>` in a session of its own with stdin closed, for the
/// calls that write or reach a remote — see the module docs for why none
/// of them may find the TUI's terminal.
fn detached(root: &Path, args: &[&str]) -> Command {
    use std::os::unix::process::CommandExt;
    let mut cmd = git_command(root);
    cmd.args(args)
        .stdin(Stdio::null())
        .env("GIT_TERMINAL_PROMPT", "0");
    // SAFETY: setsid is async-signal-safe and touches nothing but the child.
    unsafe {
        cmd.pre_exec(|| {
            if crate::ipc::libc_setsid() < 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    cmd
}

/// [`detached`], run to completion: stdout on success, git's one-line
/// complaint otherwise.
fn run(root: &Path, args: &[&str]) -> Result<String, String> {
    let out = detached(root, args)
        .output()
        .map_err(|e| format!("failed to run git: {e}"))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    } else {
        Err(git_error(&String::from_utf8_lossy(&out.stderr)))
    }
}

/// Every branch the root checkout could switch to, as [`parse_refs`] orders
/// them. `Err` is a one-line message for the modal.
pub fn list_branches(root: &Path) -> Result<Vec<Branch>, String> {
    let args = [
        "for-each-ref",
        "--sort=-committerdate",
        REF_FORMAT,
        "refs/heads",
        "refs/remotes",
    ];
    read(root, &args).map(|out| parse_refs(&out))
}

/// `git fetch --all`, stopped past [`FETCH_TIMEOUT`] or once `quit` is
/// raised (the TUI is leaving, and the runtime's shutdown waits on this
/// thread). True when it finished.
pub fn fetch(root: &Path, quit: &AtomicBool) -> bool {
    let Ok(mut child) = detached(root, &["fetch", "--all", "--quiet"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    else {
        return false;
    };
    let deadline = Instant::now() + FETCH_TIMEOUT;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return status.success(),
            Ok(None) if Instant::now() < deadline && !quit.load(Ordering::Relaxed) => {
                std::thread::sleep(Duration::from_millis(50))
            }
            _ => {
                stop(&mut child);
                return false;
            }
        }
    }
}

/// End a fetch and the `ssh` it may have started, which share its session
/// and so its process group: SIGTERM first, so git cleans up its lock and
/// temporary pack files, and SIGKILL only for one that lingers.
fn stop(child: &mut std::process::Child) {
    let group = -(child.id() as i32);
    // SAFETY: plain syscalls on a process group this process started and
    // has not yet reaped the leader of, so the id cannot have been reused.
    unsafe { kill(group, SIGTERM) };
    let grace = Instant::now() + FETCH_GRACE;
    while Instant::now() < grace {
        if let Ok(Some(_)) = child.try_wait() {
            return;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    unsafe { kill(group, SIGKILL) };
    let _ = child.wait();
}

/// An untracked directory that is a git repository of its own — a clone or
/// worktree nested in the root, like `.claude/worktrees/x`. No switch moves
/// it, a stash skips it, and a commit must not turn it into a gitlink, so
/// it is not a change.
fn nested_repo(root: &Path, file: &DiffFile) -> bool {
    file.is_untracked() && file.path.ends_with('/') && root.join(&file.path).join(".git").exists()
}

/// A path git has left conflicted.
fn unmerged(file: &DiffFile) -> bool {
    file.xy.contains(&'U') || file.xy == ['A', 'A'] || file.xy == ['D', 'D']
}

/// The checkout's uncommitted changes, nested repositories left out.
pub fn changes(root: &Path) -> Result<Vec<DiffFile>, String> {
    let mut files = crate::git_diff::changed_files(root)?;
    files.retain(|f| !nested_repo(root, f));
    Ok(files)
}

/// Each change as a prompt shows it: status, path, and the file's size and
/// modification time, so another edit to a file already listed still reads
/// as a change.
fn fingerprints(root: &Path, files: &[DiffFile]) -> Vec<String> {
    files
        .iter()
        .map(|f| {
            let stamp = std::fs::symlink_metadata(root.join(&f.path))
                .map(|m| {
                    let modified = m
                        .modified()
                        .ok()
                        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                        .map_or(0, |d| d.as_nanos());
                    format!("{} {modified}", m.len())
                })
                .unwrap_or_else(|_| "-".into());
            format!("{}{} {} {stamp}", f.xy[0], f.xy[1], f.path)
        })
        .collect()
}

/// The checkout's changes, fingerprinted; empty when git can't say.
fn fingerprint_now(root: &Path) -> Vec<String> {
    changes(root)
        .map(|files| fingerprints(root, &files))
        .unwrap_or_default()
}

/// The operation git is in the middle of, if any. No switch is safe then:
/// git refuses most of them, and a commit would conclude a merge with its
/// conflict markers in.
fn in_progress(root: &Path) -> Option<&'static str> {
    const MARKS: [(&str, &str); 5] = [
        ("MERGE_HEAD", "a merge"),
        ("rebase-merge", "a rebase"),
        ("rebase-apply", "a rebase"),
        ("CHERRY_PICK_HEAD", "a cherry-pick"),
        ("REVERT_HEAD", "a revert"),
    ];
    let mut args = vec!["rev-parse"];
    for (mark, _) in MARKS {
        args.extend(["--git-path", mark]);
    }
    let out = read(root, &args).ok()?;
    out.lines()
        .zip(MARKS)
        .find(|(path, _)| root.join(path).exists())
        .map(|(_, (_, what))| what)
}

/// The branch HEAD is on; None when it is detached.
fn head_branch(root: &Path) -> Option<String> {
    read(root, &["symbolic-ref", "-q", "--short", "HEAD"])
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// The stash stack, top first, as (commit, subject).
fn stash_entries(root: &Path) -> Vec<(String, String)> {
    read(root, &["stash", "list", "--format=%H%x00%s"])
        .unwrap_or_default()
        .lines()
        .filter_map(|l| l.split_once('\0'))
        .map(|(commit, subject)| (commit.to_string(), subject.to_string()))
        .collect()
}

/// Stash the checkout's changes under `message` and return the entry's
/// commit — found as the new entry carrying the message, not as the top of
/// a stack other checkouts push onto too. None when there was nothing to
/// save.
fn stash_push(root: &Path, message: &str) -> Result<Option<String>, String> {
    let before: HashSet<String> = stash_entries(root).into_iter().map(|(c, _)| c).collect();
    run(
        root,
        &[
            "stash",
            "push",
            "--quiet",
            "--include-untracked",
            "-m",
            message,
        ],
    )?;
    Ok(stash_entries(root)
        .into_iter()
        .find(|(commit, subject)| !before.contains(commit) && subject.ends_with(message))
        .map(|(commit, _)| commit))
}

/// Apply the stash entry `commit` back and drop it. False when it would
/// not apply, which leaves the entry where it is. The drop names a
/// position, so the position is checked against the commit right before —
/// another checkout's push in between would otherwise drop its entry.
fn stash_restore(root: &Path, commit: &str) -> bool {
    let applied = run(root, &["stash", "apply", "--quiet", "--index", commit]).is_ok()
        || run(root, &["stash", "apply", "--quiet", commit]).is_ok();
    if applied {
        if let Some(i) = stash_entries(root).iter().position(|(c, _)| c == commit) {
            let entry = format!("stash@{{{i}}}");
            if read(root, &["rev-parse", &entry]).is_ok_and(|c| c.trim() == commit) {
                let _ = run(root, &["stash", "drop", "--quiet", &entry]);
            }
        }
    }
    applied
}

/// The index file's bytes, saved before a commit or a discard rewrites it,
/// to put back exactly — intent-to-add entries and all, which a
/// `write-tree` / `read-tree` round trip loses.
struct SavedIndex {
    path: PathBuf,
    bytes: Vec<u8>,
}

fn save_index(root: &Path) -> Option<SavedIndex> {
    let path = root.join(
        read(root, &["rev-parse", "--git-path", "index"])
            .ok()?
            .trim(),
    );
    let bytes = std::fs::read(&path).ok()?;
    Some(SavedIndex { path, bytes })
}

/// Write the saved index back the way git writes one — into
/// `index.lock`, renamed over the index — so it never lands under a lock
/// another git holds. False when the lock was taken or the write failed.
fn restore_index(saved: &SavedIndex) -> bool {
    let mut lock = saved.path.clone().into_os_string();
    lock.push(".lock");
    let lock = PathBuf::from(lock);
    let Ok(mut file) = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&lock)
    else {
        return false;
    };
    let written = file.write_all(&saved.bytes).and_then(|_| file.sync_all());
    drop(file);
    if written.is_ok() && std::fs::rename(&lock, &saved.path).is_ok() {
        return true;
    }
    let _ = std::fs::remove_file(&lock);
    false
}

/// Undo what a `git switch` wrote before it stopped part-way. A filter git
/// cannot run — git-lfs missing from the TUI's PATH — kills the checkout
/// after some of the target's files are on disk but before HEAD moves, and
/// the stash can't be applied over them. Only a file byte-identical to the
/// target's version is touched (a tracked one restored from HEAD, an
/// untracked one removed), plus a tracked file the target lacks that is
/// gone from disk (restored): anything else — a change the user or an
/// agent made — is left alone, and nothing touched is lost, since the
/// target branch still holds it.
fn undo_partial_checkout(root: &Path, target: &str) -> Result<(), String> {
    let now = changes(root)?;
    if now.is_empty() {
        return Ok(());
    }
    let theirs: HashMap<String, String> = read(root, &["ls-tree", "-r", "-z", target])?
        .split('\0')
        .filter_map(|entry| {
            let (meta, path) = entry.split_once('\t')?;
            let mut meta = meta.split_whitespace();
            let (_, kind, sha) = (meta.next()?, meta.next()?, meta.next()?);
            (kind == "blob").then(|| (path.to_string(), sha.to_string()))
        })
        .collect();
    let candidates: Vec<&DiffFile> = now
        .iter()
        .filter(|f| theirs.contains_key(&f.path))
        .filter(|f| std::fs::symlink_metadata(root.join(&f.path)).is_ok_and(|m| m.is_file()))
        .collect();
    let mut written: Vec<&DiffFile> = Vec::new();
    for chunk in candidates.chunks(PATH_CHUNK) {
        let mut args = vec!["hash-object", "--no-filters", "--"];
        args.extend(chunk.iter().map(|f| f.path.as_str()));
        let hashes = read(root, &args)?;
        for (file, hash) in chunk.iter().zip(hashes.lines()) {
            if theirs.get(&file.path).is_some_and(|sha| sha == hash.trim()) {
                written.push(file);
            }
        }
    }
    let restore: Vec<&str> = written
        .iter()
        .filter(|f| !f.is_untracked())
        .map(|f| f.path.as_str())
        .chain(
            now.iter()
                .filter(|f| f.xy == [' ', 'D'] && !theirs.contains_key(&f.path))
                .map(|f| f.path.as_str()),
        )
        .collect();
    for chunk in restore.chunks(PATH_CHUNK) {
        let mut args = vec!["--literal-pathspecs", "checkout", "-q", "HEAD", "--"];
        args.extend(chunk.iter().copied());
        run(root, &args)?;
    }
    for file in written.iter().filter(|f| f.is_untracked()) {
        std::fs::remove_file(root.join(&file.path))
            .map_err(|e| format!("couldn't remove {}: {e}", file.path))?;
    }
    Ok(())
}

/// How uncommitted changes travel with a switch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Carry {
    /// Expect a clean checkout: if it isn't, stop and ask.
    Ask,
    /// `git stash push --include-untracked`, then switch; a switch git then
    /// refuses applies the entry straight back.
    Stash,
    /// A plain `git switch`, which keeps changes that don't collide with
    /// the target and refuses otherwise.
    Bring,
    /// Commit everything — untracked files included — with this message,
    /// then switch. Refused on a detached HEAD, where the commit would
    /// belong to no branch.
    Commit(String),
    /// `git switch --discard-changes`, after unstaging everything: a staged
    /// file that was never committed would otherwise be deleted, and the
    /// prompt promises untracked files stay. Carries the [`fingerprints`]
    /// of the changes the prompt showed; if the checkout no longer matches
    /// them, nothing is discarded and the prompt asks again.
    Discard(Vec<String>),
    /// `git switch --create`: a new branch off HEAD, which every change
    /// comes along to.
    Create,
}

/// What a switch came to.
#[derive(Debug, Clone, PartialEq)]
pub enum Outcome {
    /// The checkout is on `branch`; `note` says what happened to the
    /// changes on the way, or what git complained of once it had switched.
    Switched {
        branch: String,
        note: Option<String>,
    },
    /// The checkout has these changes and the switch needs an answer about
    /// them; nothing was touched. `keys` are their [`fingerprints`].
    Dirty {
        files: Vec<DiffFile>,
        keys: Vec<String>,
    },
    /// Nothing moved: the checkout is as the prompt left it.
    Failed(String),
    /// The switch failed and the checkout is no longer what the prompt
    /// listed — a commit went in, or git left something behind — so the
    /// prompt's list is stale.
    Stopped(String),
}

/// Move the checkout at `root` from `from` onto `target`, carrying its
/// changes as `carry` says. Nothing runs mid-merge or with conflicts, and a
/// switch git refuses leaves the checkout as it found it: what git wrote
/// before stopping part-way is undone, a stash applied back, a discard's
/// unstaging put back.
pub fn switch(root: &Path, from: &str, target: &Branch, carry: &Carry) -> Outcome {
    let to = target.local_name().to_string();
    if let Some(what) = in_progress(root) {
        return Outcome::Failed(format!(
            "the checkout is in the middle of {what} — finish or abort it first"
        ));
    }
    let all = match crate::git_diff::changed_files(root) {
        Ok(files) => files,
        Err(e) => return Outcome::Failed(e),
    };
    if let Some(file) = all.iter().find(|f| unmerged(f)) {
        return Outcome::Failed(format!(
            "{} has unresolved conflicts — resolve them first",
            file.path
        ));
    }
    let (nested, files): (Vec<DiffFile>, Vec<DiffFile>) =
        all.into_iter().partition(|f| nested_repo(root, f));
    let head = head_branch(root);

    let mut note: Option<String> = None;
    let mut stash: Option<String> = None;
    let mut saved_index: Option<SavedIndex> = None;
    let mut args = vec!["switch", "--quiet"];
    match carry {
        Carry::Ask => {
            if !files.is_empty() {
                let keys = fingerprints(root, &files);
                return Outcome::Dirty { files, keys };
            }
        }
        Carry::Bring => {}
        Carry::Create => {
            // `origin/x` as a local name would shadow the remote branch
            // everywhere git resolves the name.
            let remotes = read(root, &["remote"]).unwrap_or_default();
            let prefix = |r: &str| format!("{r}/");
            if let Some(remote) = remotes
                .lines()
                .map(str::trim)
                .find(|r| !r.is_empty() && target.name.starts_with(&prefix(r)))
            {
                return Outcome::Failed(format!(
                    "a branch named {} would be mistaken for {remote}'s — pick another name",
                    target.name
                ));
            }
        }
        Carry::Stash => {
            let message = format!("nebula: {from} before switching to {to}");
            match stash_push(root, &message) {
                Ok(entry) => {
                    if entry.is_some() {
                        note = Some(format!("changes stashed as \"{message}\""));
                    }
                    stash = entry;
                }
                Err(e) => return Outcome::Failed(format!("stash failed: {e}")),
            }
        }
        Carry::Commit(message) => {
            if head.is_none() {
                return Outcome::Failed(
                    "HEAD is detached — a commit here would belong to no branch; stash instead"
                        .into(),
                );
            }
            // A failing hook must not leave the user's staging replaced by
            // "everything": the index goes back as it was.
            let saved = save_index(root);
            let excludes: Vec<String> = nested
                .iter()
                .map(|f| format!(":(exclude,literal){}", f.path))
                .collect();
            let mut add = vec!["add", "--all", "--", "."];
            add.extend(excludes.iter().map(String::as_str));
            let committed = run(root, &add)
                .map_err(|e| format!("git add failed: {e}"))
                .and_then(|_| {
                    run(root, &["commit", "--quiet", "-m", message])
                        .map_err(|e| format!("commit failed: {e}"))
                });
            if let Err(e) = committed {
                if let Some(saved) = &saved {
                    restore_index(saved);
                }
                return Outcome::Failed(e);
            }
            let sha = read(root, &["rev-parse", "--short", "HEAD"]).unwrap_or_default();
            note = Some(format!("committed {} on {from}", sha.trim()));
        }
        Carry::Discard(expected) => {
            let keys = fingerprints(root, &files);
            if &keys != expected {
                return Outcome::Dirty { files, keys };
            }
            saved_index = save_index(root);
            if let Err(e) = run(root, &["reset", "--quiet"]) {
                return Outcome::Failed(format!("unstaging failed: {e}"));
            }
            args.push("--discard-changes");
        }
    }
    if target.remote {
        args.push("--track");
    }
    if *carry == Carry::Create {
        args.push("--create");
    }
    args.push(&target.name);

    // What the checkout holds as the switch starts — clean for a stash, a
    // commit or a clean checkout — to tell a refused switch from one git
    // abandoned part-way.
    let before = fingerprint_now(root);
    let error = match run(root, &args) {
        Ok(_) => return Outcome::Switched { branch: to, note },
        Err(e) => e,
    };
    // git exits non-zero when a post-checkout hook fails — after HEAD has
    // moved. That switch happened, and putting the changes back now would
    // land them on the wrong branch.
    if head_branch(root).as_deref() == Some(to.as_str()) && head.as_deref() != Some(to.as_str()) {
        let warning = format!("git complained after switching: {error}");
        let note = match note {
            Some(note) => format!("{note} · {warning}"),
            None => warning,
        };
        return Outcome::Switched {
            branch: to,
            note: Some(note),
        };
    }
    let mut error = if error.contains("would be overwritten") {
        format!("files in the checkout collide with {to} — stash or commit them instead")
    } else {
        error
    };
    let mut changed = false;
    if fingerprint_now(root) != before {
        let undone = undo_partial_checkout(root, &target.name);
        if undone.is_ok() && fingerprint_now(root) == before {
            error = format!("{error} (git stopped part-way; what it wrote was put back)");
        } else {
            changed = true;
            error = format!(
                "{error} — git stopped part-way and left files from {to} in the checkout; check git status"
            );
        }
    }
    if let Some(saved) = &saved_index {
        restore_index(saved);
    }
    if let Some(entry) = &stash {
        if !stash_restore(root, entry) {
            return Outcome::Stopped(format!("{error} (your changes are still in the stash)"));
        }
    }
    if let (Carry::Commit(_), Some(note)) = (carry, note) {
        return Outcome::Stopped(format!("{note}, but the switch failed: {error}"));
    }
    if changed {
        return Outcome::Stopped(error);
    }
    Outcome::Failed(error)
}

// ---- state ----

/// What the event loop hands back to [`land_answer`].
#[derive(Debug)]
pub enum Answer {
    Listed {
        worktree: WorktreeId,
        list: Result<Vec<Branch>, String>,
    },
    /// The checkout's changed-file count; None when git couldn't say. It
    /// follows its listing, and closes that request.
    Changes {
        worktree: WorktreeId,
        changes: Option<usize>,
    },
    Fetched {
        worktree: WorktreeId,
        ok: bool,
    },
    Switched {
        worktree: WorktreeId,
        request: u64,
        outcome: Outcome,
    },
}

/// One switch, from the keypress that starts it until its answer lands.
#[derive(Debug, Clone)]
pub struct Job {
    /// Tells this job's answer from any other's.
    pub request: u64,
    pub target: Branch,
    pub carry: Carry,
    /// What the DIRTY prompt listed (empty for a clean switch) and its
    /// fingerprints, so a failure goes back to the prompt it came from.
    pub files: Vec<DiffFile>,
    pub keys: Vec<String>,
}

/// The switcher's state that outlives the modal, on `App::branch_switch`.
#[derive(Debug, Default)]
pub struct Shared {
    /// Where the off-loop git answers go; installed at startup like
    /// `issues_tx`. `None` in the unit tests, which then run the local git
    /// inline and never fetch.
    pub tx: Option<tokio::sync::mpsc::UnboundedSender<Answer>>,
    /// Each checkout's last listing, so a reopen paints at once while the
    /// fresh one lands underneath.
    pub lists: HashMap<WorktreeId, Vec<Branch>>,
    /// Checkouts with a listing in flight — never two at once — and the
    /// ones asked for again meanwhile, listed once the first lands.
    pub listing: HashSet<WorktreeId>,
    pub relist: HashSet<WorktreeId>,
    /// Checkouts with a background fetch running, and when each last
    /// started one ([`FETCH_GAP`]).
    pub fetching: HashSet<WorktreeId>,
    pub fetched: HashMap<WorktreeId, Instant>,
    /// The switch running in each checkout. One at a time: two gits on one
    /// index trip over `index.lock`, and one's stash restore could land on
    /// the branch the other moved to.
    pub switching: HashMap<WorktreeId, Job>,
    /// The last [`Job::request`] handed out.
    pub requests: u64,
    /// Raised when the TUI goes away, so a fetch still running is stopped
    /// rather than holding the runtime's shutdown for its whole budget.
    pub quit: Arc<AtomicBool>,
}

impl Drop for Shared {
    fn drop(&mut self) {
        self.quit.store(true, Ordering::Relaxed);
    }
}

/// The four ways uncommitted changes can travel, in the order offered —
/// the safe, reversible one first, so `Enter` on the prompt stashes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Choice {
    Stash,
    Bring,
    Commit,
    Discard,
}

pub const CHOICES: [Choice; 4] = [
    Choice::Stash,
    Choice::Bring,
    Choice::Commit,
    Choice::Discard,
];

impl Choice {
    pub fn key(self) -> char {
        match self {
            Choice::Stash => 's',
            Choice::Bring => 'b',
            Choice::Commit => 'c',
            Choice::Discard => 'd',
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Choice::Stash => "Stash & switch",
            Choice::Bring => "Bring changes along",
            Choice::Commit => "Commit & switch",
            Choice::Discard => "Discard & switch",
        }
    }

    fn detail(self, from: &str, to: &str, detached: bool) -> String {
        match self {
            Choice::Stash => "stash them in a nebula entry — git stash list shows it".into(),
            Choice::Bring => format!("keep them on {to} — refused if they collide with it"),
            Choice::Commit if detached => {
                "unavailable: HEAD is detached, the commit would belong to no branch".into()
            }
            Choice::Commit => format!("commit everything on {from} first, untracked files too"),
            Choice::Discard => "throw away tracked changes; untracked and new files stay".into(),
        }
    }

    fn index(self) -> usize {
        CHOICES.iter().position(|c| *c == self).unwrap_or(0)
    }
}

/// Where the modal is in a switch.
#[derive(Debug, Clone)]
pub enum Stage {
    /// Filtering the list.
    Pick,
    /// The checkout has changes: how should they travel to `target`?
    /// `keys` fingerprint them as shown; `discard_armed` is the first press
    /// of the destructive choice.
    Dirty {
        target: Branch,
        files: Vec<DiffFile>,
        keys: Vec<String>,
        choice: usize,
        discard_armed: bool,
    },
    /// Typing the message for [`Choice::Commit`].
    Commit {
        target: Branch,
        files: Vec<DiffFile>,
        keys: Vec<String>,
        message: TextInput,
    },
    /// git is running.
    Working(Job),
}

/// The line above the bottom border.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Status {
    pub text: String,
    pub error: bool,
}

impl Status {
    fn info(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            error: false,
        }
    }

    fn error(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            error: true,
        }
    }
}

#[derive(Debug, Clone)]
pub struct BranchSwitchView {
    pub worktree: WorktreeId,
    pub root: PathBuf,
    pub project_name: String,
    /// The branch the checkout is on, as the tree had it on open.
    pub current: String,
    /// Sessions running in the checkout — they will see its files change.
    pub live_sessions: usize,
    pub query: TextInput,
    pub branches: Vec<Branch>,
    /// Indices into `branches` with their matched char positions, best
    /// first.
    pub matches: Vec<(usize, Vec<usize>)>,
    /// Index into `matches`.
    pub selected: usize,
    /// Whether the user has moved the cursor or typed: from then on a
    /// landing listing keeps the cursor on its branch.
    pub touched: bool,
    /// Whether a listing has landed (a cached one counts).
    pub listed: bool,
    pub list_error: Option<String>,
    pub fetch_failed: bool,
    pub changes: Option<usize>,
    pub stage: Stage,
    pub status: Option<Status>,
    /// Rects from the last draw, for the mouse.
    pub area: Rect,
    pub list_area: Rect,
    pub choices_area: Rect,
}

impl BranchSwitchView {
    pub fn new(
        worktree: WorktreeId,
        root: PathBuf,
        project_name: String,
        current: String,
        live_sessions: usize,
    ) -> Self {
        Self {
            worktree,
            root,
            project_name,
            current,
            live_sessions,
            query: TextInput::new(),
            branches: Vec::new(),
            matches: Vec::new(),
            selected: 0,
            touched: false,
            listed: false,
            list_error: None,
            fetch_failed: false,
            changes: None,
            stage: Stage::Pick,
            status: None,
            area: Rect::default(),
            list_area: Rect::default(),
            choices_area: Rect::default(),
        }
    }

    pub fn selected_branch(&self) -> Option<&Branch> {
        let (i, _) = self.matches.get(self.selected)?;
        self.branches.get(*i)
    }

    /// HEAD sits on no branch: a listing landed and no row is current.
    pub fn detached(&self) -> bool {
        self.listed
            && self.list_error.is_none()
            && !self.branches.is_empty()
            && !self.branches.iter().any(|b| b.current)
    }

    /// Re-rank against the query, list order breaking ties. The cursor is
    /// only clamped; callers decide where it goes.
    pub fn apply_filter(&mut self) {
        self.matches = crate::fuzzy::rank_by(
            self.query.as_str(),
            self.branches.iter().map(|b| b.name.as_str()),
            |i, _| i,
        );
        self.selected = clamp_selection(self.selected as i64, self.matches.len());
    }

    /// Where the cursor starts: the best match for a query, and with none
    /// the first branch a switch can actually land on — not the one the
    /// checkout is on, not one another worktree holds — so `c` `Enter`
    /// goes somewhere.
    fn home_selection(&mut self) {
        self.selected = if self.query.as_str().trim().is_empty() {
            self.matches
                .iter()
                .position(|(i, _)| {
                    let b = &self.branches[*i];
                    !b.current && b.checked_out_at.is_none()
                })
                .unwrap_or(0)
        } else {
            0
        };
    }

    /// A listing landed. Once the user has put the cursor somewhere it
    /// stays on that branch, by name; until then it goes home, so a stale
    /// cached `current` can't strand it on the branch the checkout is on.
    pub fn set_branches(&mut self, branches: Vec<Branch>) {
        let keep = if self.touched {
            self.selected_branch().map(|b| b.name.clone())
        } else {
            None
        };
        self.branches = branches;
        self.listed = true;
        self.apply_filter();
        match keep.and_then(|name| {
            self.matches
                .iter()
                .position(|(i, _)| self.branches[*i].name == name)
        }) {
            Some(i) => self.selected = i,
            None => self.home_selection(),
        }
    }

    pub fn select(&mut self, index: i64) {
        self.selected = clamp_selection(index, self.matches.len());
        self.touched = true;
    }

    /// The query changed: re-rank, cursor on the best match.
    fn requery(&mut self) {
        self.apply_filter();
        self.home_selection();
        self.touched = true;
        self.status = None;
    }
}

// ---- opening and answers ----

/// The hotkey. From a WORKTREES PANEL or SESSIONS PANEL row (and the pane)
/// it acts on the selected checkout, which must be the root; from the
/// PROJECTS PANEL, or with no checkout under the cursor (an OPEN PRS
/// row), on the project's root.
pub(crate) fn open_branch_switch(app: &mut App) {
    let Some(project) = app.selected_project().map(|p| p.id.clone()) else {
        app.flash = Some("switch branch: select a project first".into());
        return;
    };
    let on_checkout = matches!(
        app.focus,
        Focus::Worktrees | Focus::Sessions | Focus::Terminal
    );
    let target = match app.selected_worktree().filter(|_| on_checkout) {
        Some(w) if !w.is_main => {
            app.flash = Some(
                "switch branch is for the ⌂ root checkout — a worktree stays on the branch it was cut for"
                    .into(),
            );
            return;
        }
        Some(w) => Some(w.id.clone()),
        None => app
            .tree
            .worktrees
            .iter()
            .find(|w| w.project_id == project && w.is_main)
            .map(|w| w.id.clone()),
    };
    match target {
        Some(id) => open_for(app, &id),
        None => app.flash = Some("switch branch: the project has no root checkout yet".into()),
    }
}

/// A cached listing with `current` re-derived from the branch the tree
/// says the checkout is on: something may have switched it since — an
/// agent's own `git switch`.
fn recurrent(cached: &[Branch], branch: &str) -> Vec<Branch> {
    let mut rows: Vec<Branch> = cached
        .iter()
        .cloned()
        .map(|mut b| {
            if !b.remote {
                b.current = b.name == branch;
                if b.current {
                    b.checked_out_at = None;
                }
            }
            b
        })
        .collect();
    rows.sort_by_key(|b| (b.remote, !b.current));
    rows
}

/// Open the modal on `worktree`: the cached listing (if any) at once, a
/// fresh one and the changed-file count off the loop, and a background
/// fetch when the last one is a minute old. A switch still running there
/// shows as the working modal it was hidden from.
pub(crate) fn open_for(app: &mut App, worktree: &WorktreeId) {
    let Some(w) = app
        .tree
        .worktrees
        .iter()
        .find(|w| &w.id == worktree)
        .cloned()
    else {
        return;
    };
    let project_name = app
        .tree
        .projects
        .iter()
        .find(|p| p.id == w.project_id)
        .map(|p| p.name.clone())
        .unwrap_or_default();
    let live = app
        .tree
        .agents
        .iter()
        .filter(|a| a.worktree_id == w.id && a.alive)
        .count()
        + app
            .tree
            .terminals
            .iter()
            .filter(|t| t.worktree_id == w.id && t.alive)
            .count();
    let mut view = BranchSwitchView::new(
        w.id.clone(),
        w.path.clone(),
        project_name,
        w.branch.clone(),
        live,
    );
    if let Some(cached) = app.branch_switch.lists.get(&w.id) {
        view.set_branches(recurrent(cached, &w.branch));
    }
    if let Some(job) = app.branch_switch.switching.get(&w.id) {
        view.stage = Stage::Working(job.clone());
    }
    app.overlay = Some(Overlay::BranchSwitch(view));
    request_list(app, w.id.clone(), w.path.clone());
    request_fetch(app, w.id, w.path, false);
    app.dirty = true;
}

/// List the branches and count the changes, off the loop (inline in the
/// unit tests). One request per checkout at a time; asking again while one
/// runs lists once more when it lands. A checkout that isn't on disk is
/// said so without a process.
fn request_list(app: &mut App, worktree: WorktreeId, root: PathBuf) {
    if !root.is_dir() {
        let list = Err(format!("{} is not on disk", root.display()));
        land_answer(app, Answer::Listed { worktree, list });
        return;
    }
    let tx = app.branch_switch.tx.clone();
    if tx.is_some() && !app.branch_switch.listing.insert(worktree.clone()) {
        app.branch_switch.relist.insert(worktree);
        return;
    }
    let ask = move |send: &mut dyn FnMut(Answer)| {
        send(Answer::Listed {
            worktree: worktree.clone(),
            list: list_branches(&root),
        });
        let changes = changes(&root).ok().map(|f| f.len());
        send(Answer::Changes { worktree, changes });
    };
    match tx {
        Some(tx) => {
            tokio::task::spawn_blocking(move || {
                ask(&mut |answer| {
                    let _ = tx.send(answer);
                })
            });
        }
        None => {
            let mut answers = Vec::new();
            ask(&mut |answer| answers.push(answer));
            for answer in answers {
                land_answer(app, answer);
            }
        }
    }
}

/// Refresh the remotes in the background: skipped while one runs, within
/// [`FETCH_GAP`] of the last unless `force`d, and in the unit tests.
fn request_fetch(app: &mut App, worktree: WorktreeId, root: PathBuf, force: bool) {
    let Some(tx) = app.branch_switch.tx.clone() else {
        return;
    };
    let shared = &mut app.branch_switch;
    let recent = shared
        .fetched
        .get(&worktree)
        .is_some_and(|at| at.elapsed() < FETCH_GAP);
    if !root.is_dir() || (recent && !force) || shared.fetching.contains(&worktree) {
        return;
    }
    shared.fetching.insert(worktree.clone());
    shared.fetched.insert(worktree.clone(), Instant::now());
    let quit = shared.quit.clone();
    tokio::task::spawn_blocking(move || {
        let ok = fetch(&root, &quit);
        let _ = tx.send(Answer::Fetched { worktree, ok });
    });
}

/// The open modal, when it is `worktree`'s.
fn view_for<'a>(app: &'a mut App, worktree: &WorktreeId) -> Option<&'a mut BranchSwitchView> {
    match &mut app.overlay {
        Some(Overlay::BranchSwitch(view)) if &view.worktree == worktree => Some(view),
        _ => None,
    }
}

/// A git answer landed.
pub(crate) fn land_answer(app: &mut App, answer: Answer) {
    match answer {
        Answer::Listed { worktree, list } => {
            if let Ok(list) = &list {
                app.branch_switch
                    .lists
                    .insert(worktree.clone(), list.clone());
            }
            if let Some(view) = view_for(app, &worktree) {
                match list {
                    Ok(list) => {
                        view.list_error = None;
                        view.set_branches(list);
                    }
                    Err(e) => {
                        view.listed = true;
                        view.list_error = Some(e);
                    }
                }
            }
        }
        Answer::Changes { worktree, changes } => {
            app.branch_switch.listing.remove(&worktree);
            let root = view_for(app, &worktree).map(|view| {
                view.changes = changes;
                view.root.clone()
            });
            if app.branch_switch.relist.remove(&worktree) {
                if let Some(root) = root {
                    request_list(app, worktree, root);
                }
            }
        }
        Answer::Fetched { worktree, ok } => {
            app.branch_switch.fetching.remove(&worktree);
            let root = view_for(app, &worktree).map(|view| {
                view.fetch_failed = !ok;
                view.root.clone()
            });
            if let (true, Some(root)) = (ok, root) {
                request_list(app, worktree, root);
            }
        }
        Answer::Switched {
            worktree,
            request,
            outcome,
        } => land_switch(app, worktree, request, outcome),
    }
    app.dirty = true;
}

fn land_switch(app: &mut App, worktree: WorktreeId, request: u64, outcome: Outcome) {
    // Only the running job's answer counts.
    match app.branch_switch.switching.get(&worktree) {
        Some(job) if job.request == request => {}
        _ => return,
    }
    let Some(job) = app.branch_switch.switching.remove(&worktree) else {
        return;
    };
    let from = app
        .tree
        .worktrees
        .iter()
        .find(|w| w.id == worktree)
        .map(|w| w.branch.clone())
        .unwrap_or_default();
    // Whether the modal is still showing this job: `Esc` hides it while git
    // works, and the answer then goes to the footer.
    let showing = matches!(
        &app.overlay,
        Some(Overlay::BranchSwitch(v))
            if v.worktree == worktree && matches!(&v.stage, Stage::Working(j) if j.request == request)
    );
    match outcome {
        Outcome::Switched { branch, note } => {
            // The row renames now rather than on the DAEMON's next sync,
            // and everything cached about the old branch goes: the listing
            // (its `current` mark), the changed-file badge, the PR row.
            if let Some(w) = app.tree.worktrees.iter_mut().find(|w| w.id == worktree) {
                w.branch = branch.clone();
            }
            app.branch_switch.lists.remove(&worktree);
            if app
                .git_changes
                .as_ref()
                .is_some_and(|(id, _)| id == &worktree)
            {
                app.git_changes = None;
            }
            app.worktree_changes.remove(&worktree);
            app.worktree_lines.remove(&worktree);
            app.pull_requests.remove(&worktree);
            app.pr_recheck.remove(&worktree);
            if view_for(app, &worktree).is_some() {
                app.overlay = None;
            }
            app.flash = Some(match note {
                Some(note) => format!("⌂ root is on {branch} · {note}"),
                None => format!("⌂ root is on {branch}"),
            });
        }
        Outcome::Dirty { files, keys } if showing => {
            if let Some(view) = view_for(app, &worktree) {
                view.changes = Some(files.len());
                if matches!(job.carry, Carry::Discard(_)) {
                    view.status = Some(Status::error(
                        "the changes moved since the prompt opened — look again before discarding",
                    ));
                }
                view.stage = Stage::Dirty {
                    target: job.target,
                    files,
                    keys,
                    choice: 0,
                    discard_armed: false,
                };
            }
        }
        Outcome::Dirty { files, .. } => {
            app.flash = Some(format!(
                "not switched: {from} has {} — c to choose what happens to them",
                changes_text(files.len())
            ));
        }
        Outcome::Failed(error) if showing => {
            if let Some(view) = view_for(app, &worktree) {
                let choice = carry_choice(&job.carry).index();
                view.stage = if job.files.is_empty() {
                    Stage::Pick
                } else {
                    Stage::Dirty {
                        target: job.target,
                        files: job.files,
                        keys: job.keys,
                        choice,
                        discard_armed: false,
                    }
                };
                view.status = Some(Status::error(error));
            }
        }
        // The prompt's list no longer describes the checkout: back to the
        // branches, with the count asked for again.
        Outcome::Stopped(error) if showing => {
            let root = view_for(app, &worktree).map(|view| {
                view.stage = Stage::Pick;
                view.status = Some(Status::error(error));
                view.root.clone()
            });
            if let Some(root) = root {
                request_list(app, worktree, root);
            }
        }
        Outcome::Failed(error) | Outcome::Stopped(error) => {
            app.flash = Some(format!("switch branch failed: {error}"))
        }
    }
}

fn carry_choice(carry: &Carry) -> Choice {
    match carry {
        Carry::Commit(_) => Choice::Commit,
        Carry::Bring => Choice::Bring,
        Carry::Discard(_) => Choice::Discard,
        Carry::Ask | Carry::Stash | Carry::Create => Choice::Stash,
    }
}

/// Run a switch off the loop (inline in the unit tests) — unless one is
/// already running in the checkout.
fn start_switch(
    app: &mut App,
    target: Branch,
    carry: Carry,
    files: Vec<DiffFile>,
    keys: Vec<String>,
) {
    let Some(Overlay::BranchSwitch(view)) = &mut app.overlay else {
        return;
    };
    let shared = &mut app.branch_switch;
    if shared.switching.contains_key(&view.worktree) {
        view.status = Some(Status::error(
            "a switch is already running in this checkout",
        ));
        return;
    }
    shared.requests += 1;
    let job = Job {
        request: shared.requests,
        target,
        carry,
        files,
        keys,
    };
    shared.switching.insert(view.worktree.clone(), job.clone());
    let (worktree, root, from) = (
        view.worktree.clone(),
        view.root.clone(),
        view.current.clone(),
    );
    view.status = None;
    view.stage = Stage::Working(job.clone());
    let request = job.request;
    match shared.tx.clone() {
        Some(tx) => {
            tokio::task::spawn_blocking(move || {
                let outcome = switch(&root, &from, &job.target, &job.carry);
                let _ = tx.send(Answer::Switched {
                    worktree,
                    request,
                    outcome,
                });
            });
        }
        None => {
            let outcome = switch(&root, &from, &job.target, &job.carry);
            land_answer(
                app,
                Answer::Switched {
                    worktree,
                    request,
                    outcome,
                },
            );
        }
    }
    app.dirty = true;
}

// ---- keys and mouse ----

/// `Enter` on the list.
fn activate_selected(app: &mut App) {
    let Some(Overlay::BranchSwitch(view)) = &mut app.overlay else {
        return;
    };
    let Some(branch) = view.selected_branch().cloned() else {
        // Nothing matches: create the typed branch off the current one, as
        // an IDE's switcher offers to.
        let name = view.query.as_str().trim().to_string();
        if view.listed && !name.is_empty() {
            let target = Branch::new_local(name);
            start_switch(app, target, Carry::Create, Vec::new(), Vec::new());
        }
        return;
    };
    if branch.current {
        view.status = Some(Status::info(format!("already on {}", branch.name)));
        return;
    }
    if let Some(path) = &branch.checked_out_at {
        view.status = Some(Status::error(format!(
            "{} is checked out in {} — git keeps a branch in one checkout",
            branch.name,
            path.display()
        )));
        return;
    }
    start_switch(app, branch, Carry::Ask, Vec::new(), Vec::new());
}

/// `Ctrl+r`: fetch the remotes now, past the minute's gap, and list again.
fn refresh(app: &mut App) {
    let Some(Overlay::BranchSwitch(view)) = &mut app.overlay else {
        return;
    };
    view.fetch_failed = false;
    let (worktree, root) = (view.worktree.clone(), view.root.clone());
    request_list(app, worktree.clone(), root.clone());
    request_fetch(app, worktree, root, true);
}

/// Pick one of the [`CHOICES`] on the DIRTY prompt.
fn choose(app: &mut App, choice: Choice) {
    let Some(Overlay::BranchSwitch(view)) = &mut app.overlay else {
        return;
    };
    let detached = view.detached();
    let Stage::Dirty {
        target,
        files,
        keys,
        choice: at,
        discard_armed,
    } = &mut view.stage
    else {
        return;
    };
    let armed = *discard_armed && *at == choice.index();
    *at = choice.index();
    *discard_armed = false;
    let (target, files, keys) = (target.clone(), files.clone(), keys.clone());
    match choice {
        Choice::Stash => start_switch(app, target, Carry::Stash, files, keys),
        Choice::Bring => start_switch(app, target, Carry::Bring, files, keys),
        Choice::Commit if detached => {
            view.status = Some(Status::error(
                "HEAD is detached — a commit here would belong to no branch; stash instead",
            ));
        }
        Choice::Commit => {
            view.status = None;
            view.stage = Stage::Commit {
                target,
                files,
                keys,
                message: TextInput::new(),
            };
        }
        Choice::Discard if armed => {
            let carry = Carry::Discard(keys.clone());
            start_switch(app, target, carry, files, keys);
        }
        Choice::Discard => {
            if let Stage::Dirty { discard_armed, .. } = &mut view.stage {
                *discard_armed = true;
            }
        }
    }
}

/// Keys in the BRANCH SWITCHER.
pub(crate) fn handle_key(app: &mut App, key: KeyEvent) {
    let Some(Overlay::BranchSwitch(view)) = &mut app.overlay else {
        return;
    };
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    app.dirty = true;
    match &mut view.stage {
        Stage::Pick => {
            let page = view.list_area.height.max(1) as i64;
            let selected = view.selected as i64;
            match key.code {
                // Two-stage, like every fuzzy overlay: the query first.
                KeyCode::Esc if !view.query.as_str().is_empty() => {
                    view.query.clear();
                    view.requery();
                }
                KeyCode::Esc => app.overlay = None,
                // j/k stay typeable in the query; Ctrl+n/p mirror ↑/↓.
                KeyCode::Down => view.select(selected + 1),
                KeyCode::Up => view.select(selected - 1),
                KeyCode::Char('n') if ctrl => view.select(selected + 1),
                KeyCode::Char('p') if ctrl => view.select(selected - 1),
                KeyCode::PageDown => view.select(selected + page),
                KeyCode::PageUp => view.select(selected - page),
                KeyCode::Char('r') if ctrl => refresh(app),
                KeyCode::Enter => activate_selected(app),
                _ => {
                    if view.query.handle_key(&key).changed() {
                        view.requery();
                    }
                }
            }
        }
        Stage::Dirty {
            choice,
            discard_armed,
            ..
        } => match key.code {
            KeyCode::Esc => {
                view.stage = Stage::Pick;
                view.status = None;
            }
            KeyCode::Down | KeyCode::Char('j') => {
                *choice = (*choice + 1).min(CHOICES.len() - 1);
                *discard_armed = false;
            }
            KeyCode::Up | KeyCode::Char('k') => {
                *choice = choice.saturating_sub(1);
                *discard_armed = false;
            }
            KeyCode::Enter => {
                let picked = CHOICES[(*choice).min(CHOICES.len() - 1)];
                choose(app, picked);
            }
            KeyCode::Char(c) if !ctrl => {
                if let Some(picked) = CHOICES.iter().find(|ch| ch.key() == c) {
                    choose(app, *picked);
                }
            }
            _ => {}
        },
        Stage::Commit {
            target,
            files,
            keys,
            message,
        } => match key.code {
            KeyCode::Esc => {
                view.stage = Stage::Dirty {
                    target: target.clone(),
                    files: files.clone(),
                    keys: keys.clone(),
                    choice: Choice::Commit.index(),
                    discard_armed: false,
                };
                view.status = None;
            }
            KeyCode::Enter => {
                let text = message.as_str().trim().to_string();
                if text.is_empty() {
                    view.status = Some(Status::error("type a commit message first"));
                } else {
                    let (target, files, keys) = (target.clone(), files.clone(), keys.clone());
                    start_switch(app, target, Carry::Commit(text), files, keys);
                }
            }
            _ => {
                message.handle_key(&key);
            }
        },
        // git is already running: Esc only hides the modal (`c` brings it
        // back), and the result lands in the footer.
        Stage::Working(_) => {
            if key.code == KeyCode::Esc {
                app.overlay = None;
                app.flash = Some("still switching — c shows it, the result lands here".into());
            }
        }
    }
}

/// A bracketed paste: into the query or the commit message, whichever is
/// being typed. False when neither is.
pub(crate) fn paste(view: &mut BranchSwitchView, text: &str) -> bool {
    match &mut view.stage {
        Stage::Pick => {
            view.query.insert_str(text);
            view.requery();
            true
        }
        Stage::Commit { message, .. } => {
            message.insert_str(text);
            true
        }
        _ => false,
    }
}

/// Mouse in the BRANCH SWITCHER: the wheel walks the list (or the choices),
/// a click on a branch selects it — `Enter` switches, so a stray click
/// never moves the checkout — and a click on a choice picks it, the
/// CONTEXT MENU's rule for rows that are actions. A click outside closes
/// (`overlay_close`).
pub(crate) fn handle_mouse(app: &mut App, mouse: MouseEvent, pos: Position) {
    let Some(Overlay::BranchSwitch(view)) = &mut app.overlay else {
        return;
    };
    app.dirty = true;
    let delta = match mouse.kind {
        MouseEventKind::ScrollUp => -1,
        MouseEventKind::ScrollDown => 1,
        _ => 0,
    };
    let click = mouse.kind == MouseEventKind::Down(MouseButton::Left);
    match &mut view.stage {
        Stage::Pick => {
            if delta != 0 {
                view.select(view.selected as i64 + delta);
            } else if click {
                let first = window_start(view.selected, view.list_area.height as usize);
                if let Some(index) =
                    crate::list_hit::row_at(view.list_area, first, view.matches.len(), pos)
                {
                    view.select(index as i64);
                }
            }
        }
        Stage::Dirty {
            choice,
            discard_armed,
            ..
        } => {
            if delta != 0 {
                *choice = (*choice as i64 + delta).clamp(0, CHOICES.len() as i64 - 1) as usize;
                *discard_armed = false;
            } else if click && view.choices_area.contains(pos) {
                if let Some(picked) = CHOICES.get((pos.y - view.choices_area.y) as usize) {
                    choose(app, *picked);
                }
            }
        }
        _ => {}
    }
}

// ---- drawing ----

/// The FOOTER's hint while the modal is up.
pub fn footer_hint(view: &BranchSwitchView) -> &'static str {
    match view.stage {
        Stage::Pick => "type: filter  ↑/↓ ^n/^p: move  Enter: switch (nothing matching: create)  ^r: fetch  Esc: clear/close",
        Stage::Dirty { .. } => "s: stash  b: bring along  c: commit  d: discard  ↑/↓ Enter: choose  Esc: back to the list",
        Stage::Commit { .. } => "type the commit message  Enter: commit & switch  Esc: back",
        Stage::Working(_) => "git is running  Esc: hide (c shows it again; a result that lands while hidden goes to the footer)",
    }
}

fn border_hint(stage: &Stage) -> &'static str {
    match stage {
        Stage::Pick => " Enter switch · ↑↓ move · ^r fetch · Esc clear/close ",
        Stage::Dirty { .. } => " s/b/c/d or ↑↓ Enter · Esc back ",
        Stage::Commit { .. } => " Enter commit & switch · Esc back ",
        Stage::Working(_) => " working… · Esc hide ",
    }
}

/// `n change` / `n changes`.
fn changes_text(n: usize) -> String {
    format!("{n} uncommitted change{}", if n == 1 { "" } else { "s" })
}

/// A branch name with the filter's matches lit, over `base`.
fn name_spans(shown: &str, positions: &[usize], base: Style, th: Theme) -> Vec<Span<'static>> {
    let lit = Style::default().fg(th.accent).add_modifier(Modifier::BOLD);
    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut run = String::new();
    let mut run_lit = false;
    for (i, c) in shown.chars().enumerate() {
        let on = positions.binary_search(&i).is_ok();
        if on != run_lit && !run.is_empty() {
            spans.push(Span::styled(
                std::mem::take(&mut run),
                if run_lit { lit } else { base },
            ));
        }
        run_lit = on;
        run.push(c);
    }
    if !run.is_empty() {
        spans.push(Span::styled(run, if run_lit { lit } else { base }));
    }
    spans
}

/// One list row: glyph, name, subject, then the tag and age pinned right —
/// measured in cells, so a wide subject can't push the age off the row.
fn branch_row(
    branch: &Branch,
    positions: &[usize],
    width: usize,
    now: i64,
    th: Theme,
) -> Vec<Span<'static>> {
    let elsewhere = branch.checked_out_at.is_some();
    let (glyph, glyph_color) = if branch.current {
        ("● ", th.ok)
    } else {
        ("○ ", th.dim)
    };
    let (tag, tag_color) = if branch.current {
        ("current", th.ok)
    } else if elsewhere {
        ("in a worktree", th.dim)
    } else if branch.remote {
        ("remote", th.dim)
    } else {
        ("", th.dim)
    };
    let age = if branch.committed > 0 {
        crate::hosts::ago_label((now - branch.committed) * 1000)
    } else {
        String::new()
    };
    // `render_row` spends a column on its marker; after it come the glyph,
    // the name and subject (`budget`), at least one space, the tag and age,
    // and a column of margin so the age never touches the frame.
    let right_w = cells(tag) + usize::from(!tag.is_empty()) + cells(&age);
    let budget = width.saturating_sub(1 + cells(glyph) + 1 + right_w + 1);
    let name = fit(&branch.name, budget.min(48).max(budget / 2));
    let name_w = cells(&name);
    let base = if elsewhere {
        Style::default().fg(th.dim)
    } else if branch.current {
        Style::default().fg(th.ok).add_modifier(Modifier::BOLD)
    } else {
        Style::default()
    };
    let mut spans = vec![Span::styled(glyph, Style::default().fg(glyph_color))];
    spans.extend(name_spans(
        &name,
        visible_positions(positions, &name, &branch.name),
        base,
        th,
    ));
    let subject_w = budget.saturating_sub(name_w + 2);
    let mut used = name_w;
    if subject_w >= 8 && !branch.subject.is_empty() {
        let subject = fit(&branch.subject, subject_w);
        used += 2 + cells(&subject);
        spans.push(Span::styled(
            format!("  {subject}"),
            Style::default().fg(th.dim),
        ));
    }
    spans.push(Span::raw(" ".repeat(budget.saturating_sub(used) + 1)));
    if !tag.is_empty() {
        spans.push(Span::styled(
            format!("{tag} "),
            Style::default().fg(tag_color),
        ));
    }
    spans.push(Span::styled(age, Style::default().fg(th.dim)));
    spans
}

pub(crate) fn draw(f: &mut Frame, app: &mut App, view: &BranchSwitchView, th: Theme) {
    let area = centered_rect(f.area(), SIZE.0, SIZE.1);
    f.render_widget(Clear, area);
    let title = format!(
        " Switch branch — {} ⌂ root · on {} ",
        view.project_name, view.current
    );
    let block = modal_block(title, th).title_bottom(
        Line::from(Span::styled(
            border_hint(&view.stage),
            Style::default().fg(th.dim),
        ))
        .left_aligned(),
    );
    let inner = block.inner(area);
    f.render_widget(block, area);
    let [body, status_row] =
        Layout::vertical([Constraint::Min(1), Constraint::Length(1)]).areas(inner);

    let mut list_area = Rect::default();
    let mut choices_area = Rect::default();
    let mut selected = view.selected;
    match &view.stage {
        Stage::Dirty {
            target,
            files,
            choice,
            discard_armed,
            ..
        } => {
            choices_area = draw_dirty(f, view, target, files, *choice, *discard_armed, body, th);
        }
        Stage::Working(job) if !job.files.is_empty() => {
            let choice = carry_choice(&job.carry).index();
            draw_dirty(f, view, &job.target, &job.files, choice, false, body, th);
        }
        Stage::Commit {
            target,
            files,
            message,
            ..
        } => draw_commit(f, view, target, files, message, body, th),
        Stage::Pick | Stage::Working(_) => {
            (list_area, selected) = draw_list(f, view, body, th);
        }
    }

    // The status line: what's running, else what just went wrong or right,
    // else what the user should know before switching.
    let fetching = app.branch_switch.fetching.contains(&view.worktree);
    let (text, color) = if let Stage::Working(job) = &view.stage {
        let doing = match job.carry {
            Carry::Stash => "stashing and switching",
            Carry::Commit(_) => "committing and switching",
            Carry::Discard(_) => "discarding and switching",
            Carry::Create => "creating and switching",
            Carry::Ask | Carry::Bring => "switching",
        };
        (format!("{doing} to {}…", job.target.local_name()), th.warn)
    } else if let Some(status) = &view.status {
        (
            status.text.clone(),
            if status.error { th.err } else { th.ok },
        )
    } else {
        let mut parts: Vec<String> = Vec::new();
        // The prompts already lead with the count.
        if let (Stage::Pick, Some(n)) = (&view.stage, view.changes.filter(|n| *n > 0)) {
            parts.push(changes_text(n));
        }
        if view.live_sessions > 0 {
            parts.push(format!(
                "{} session{} running in this checkout",
                view.live_sessions,
                if view.live_sessions == 1 { "" } else { "s" }
            ));
        }
        if fetching {
            parts.push("fetching remotes…".into());
        } else if view.fetch_failed {
            parts.push("couldn't reach the remotes (^r tries again)".into());
        }
        // With rows on screen, a failed refresh would otherwise go unseen.
        if let (Some(e), false) = (&view.list_error, view.branches.is_empty()) {
            parts.push(format!("couldn't refresh the list: {e}"));
        }
        (parts.join(" · "), th.dim)
    };
    let text = fit(&text, status_row.width.saturating_sub(1) as usize);
    f.render_widget(
        Paragraph::new(Span::styled(format!(" {text}"), Style::default().fg(color))),
        status_row,
    );

    // Write-back (draw works on a clone).
    if let Some(Overlay::BranchSwitch(v)) = &mut app.overlay {
        v.area = area;
        v.list_area = list_area;
        v.choices_area = choices_area;
        v.selected = selected;
    }
}

/// The PICK stage: the query row, then the list. Returns the list's rect
/// and the clamped cursor.
fn draw_list(f: &mut Frame, view: &BranchSwitchView, body: Rect, th: Theme) -> (Rect, usize) {
    if let Some(query_area) = row_rect(body, 0) {
        let line = search_line(
            &view.query,
            "type to filter branches and remotes…",
            query_area,
            th,
        );
        f.render_widget(Paragraph::new(line), query_area);
    }
    let list = Rect {
        y: body.y + 1,
        height: body.height.saturating_sub(1),
        ..body
    };
    if view.matches.is_empty() {
        let query = view.query.as_str().trim();
        let text = match (&view.list_error, view.listed) {
            (_, true) if view.branches.is_empty() && view.list_error.is_none() => {
                "no branches".to_string()
            }
            (Some(e), _) if view.branches.is_empty() => format!("couldn't list branches: {e}"),
            (_, false) => "reading branches…".into(),
            _ if !query.is_empty() => format!(
                "{NO_MATCHES} — Enter creates \"{query}\" off {}",
                view.current
            ),
            _ => NO_MATCHES.into(),
        };
        empty_list_row(f, list, &fit(&text, list.width as usize), th);
    }
    let selected = view.selected.min(view.matches.len().saturating_sub(1));
    let start = window_start(selected, list.height as usize);
    let now = crate::app::now_ms() / 1000;
    for (row, (i, (index, positions))) in view.matches.iter().enumerate().skip(start).enumerate() {
        let Some(row_area) = row_rect(list, row) else {
            break;
        };
        let spans = branch_row(
            &view.branches[*index],
            positions,
            list.width as usize,
            now,
            th,
        );
        render_row(f, row_area, spans, i == selected, true, th);
    }
    (list, selected)
}

/// One `M  path` line of the changed files.
fn file_line(file: &DiffFile, width: usize, th: Theme) -> Line<'static> {
    let code: String = file.xy.iter().collect();
    Line::from(vec![
        Span::styled(format!("   {code} "), Style::default().fg(th.warn)),
        Span::raw(fit(&file.path, width.saturating_sub(7))),
    ])
}

/// The changed files under a prompt, from `from_row` down, with a `… and N
/// more` when they run past the bottom.
fn draw_files(f: &mut Frame, files: &[DiffFile], body: Rect, from_row: usize, th: Theme) {
    let Some(header) = row_rect(body, from_row) else {
        return;
    };
    f.render_widget(
        Paragraph::new(Span::styled(
            " CHANGES",
            Style::default().fg(th.dim).add_modifier(Modifier::BOLD),
        )),
        header,
    );
    let room = (body.height as usize).saturating_sub(from_row + 1);
    let shown = if files.len() > room {
        room.saturating_sub(1)
    } else {
        files.len()
    };
    for (i, file) in files.iter().take(shown).enumerate() {
        if let Some(row) = row_rect(body, from_row + 1 + i) {
            f.render_widget(
                Paragraph::new(file_line(file, body.width as usize, th)),
                row,
            );
        }
    }
    if shown < files.len() {
        if let Some(row) = row_rect(body, from_row + 1 + shown) {
            f.render_widget(
                Paragraph::new(Span::styled(
                    format!("   … and {} more", files.len() - shown),
                    Style::default().fg(th.dim),
                )),
                row,
            );
        }
    }
}

/// The DIRTY prompt: the question, the four choices, the files. Returns
/// the choices' rect.
#[allow(clippy::too_many_arguments)]
fn draw_dirty(
    f: &mut Frame,
    view: &BranchSwitchView,
    target: &Branch,
    files: &[DiffFile],
    choice: usize,
    discard_armed: bool,
    body: Rect,
    th: Theme,
) -> Rect {
    let to = target.local_name();
    let detached = view.detached();
    if let Some(row) = row_rect(body, 0) {
        let question = format!(
            " {} has {} — how should they travel to {to}?",
            view.current,
            changes_text(files.len())
        );
        f.render_widget(
            Paragraph::new(Span::styled(
                fit(&question, body.width as usize),
                Style::default().add_modifier(Modifier::BOLD),
            )),
            row,
        );
    }
    let choices = Rect {
        y: body.y + 2,
        height: (CHOICES.len() as u16).min(body.height.saturating_sub(2)),
        ..body
    };
    for (i, c) in CHOICES.iter().enumerate() {
        let Some(row) = row_rect(choices, i) else {
            break;
        };
        let armed = discard_armed && *c == Choice::Discard;
        let unavailable = detached && *c == Choice::Commit;
        let detail = if armed {
            format!("press d again to throw away {}", changes_text(files.len()))
        } else {
            c.detail(&view.current, to, detached)
        };
        let label_color = match c {
            _ if unavailable => th.dim,
            Choice::Discard => th.err,
            _ => th.text,
        };
        let spans = vec![
            Span::styled(
                format!("{} ", c.key()),
                Style::default().fg(th.accent).add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!("{:<21}", c.label()),
                Style::default().fg(label_color),
            ),
            Span::styled(
                fit(&detail, (body.width as usize).saturating_sub(26)),
                Style::default().fg(if armed { th.err } else { th.dim }),
            ),
        ];
        render_row(f, row, spans, i == choice, true, th);
    }
    draw_files(f, files, body, 2 + CHOICES.len() + 1, th);
    choices
}

/// The COMMIT prompt: what is about to happen, the message field, the
/// files.
#[allow(clippy::too_many_arguments)]
fn draw_commit(
    f: &mut Frame,
    view: &BranchSwitchView,
    target: &Branch,
    files: &[DiffFile],
    message: &TextInput,
    body: Rect,
    th: Theme,
) {
    if let Some(row) = row_rect(body, 0) {
        let line = format!(
            " Commit all {} on {}, then switch to {}",
            changes_text(files.len()),
            view.current,
            target.local_name()
        );
        f.render_widget(
            Paragraph::new(Span::styled(
                fit(&line, body.width as usize),
                Style::default().add_modifier(Modifier::BOLD),
            )),
            row,
        );
    }
    if let Some(row) = row_rect(body, 2) {
        let label = " message  ";
        let budget = (body.width as usize).saturating_sub(label.len() + 1);
        let mut spans = vec![Span::styled(label, Style::default().fg(th.dim))];
        spans.extend(input_spans(message, budget, th.accent, th));
        f.render_widget(Paragraph::new(Line::from(spans)), row);
    }
    draw_files(f, files, body, 4, th);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyEvent;
    use nebula_core::{Project, ProjectId, Worktree};
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    fn git(repo: &Path, args: &[&str]) -> String {
        let out = std::process::Command::new("git")
            .arg("-C")
            .arg(repo)
            .args(args)
            .output()
            .expect("run git");
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    /// A repo with `main` (a.txt = "main\n") and `feature` (a.txt =
    /// "feature\n"), on `main`.
    fn repo(dir: &tempfile::TempDir) -> PathBuf {
        let repo = dir.path().join("repo");
        std::fs::create_dir(&repo).unwrap();
        git(&repo, &["init", "-q", "-b", "main"]);
        git(&repo, &["config", "user.email", "t@t"]);
        git(&repo, &["config", "user.name", "t"]);
        git(&repo, &["config", "commit.gpgsign", "false"]);
        std::fs::write(repo.join("a.txt"), "main\n").unwrap();
        std::fs::write(repo.join("b.txt"), "shared\n").unwrap();
        git(&repo, &["add", "."]);
        git(&repo, &["commit", "-q", "-m", "init"]);
        git(&repo, &["switch", "-q", "-c", "feature"]);
        std::fs::write(repo.join("a.txt"), "feature\n").unwrap();
        git(&repo, &["commit", "-q", "-am", "feature work"]);
        git(&repo, &["switch", "-q", "main"]);
        repo
    }

    /// A branch `lfs` whose checkout dies part-way, the way one does when
    /// git-lfs is missing from PATH: its `.gitattributes` names a required
    /// process filter whose binary does not exist, so git writes `a.txt`
    /// and `.gitattributes`, fails on the first filtered file, and exits
    /// with HEAD unmoved.
    fn lfs_branch(repo: &Path) {
        git(repo, &["switch", "-q", "-c", "lfs"]);
        std::fs::write(repo.join("a.txt"), "lfs\n").unwrap();
        std::fs::write(repo.join("p1.txt"), "large\n").unwrap();
        git(repo, &["add", "."]);
        git(repo, &["commit", "-q", "-m", "files"]);
        std::fs::write(repo.join(".gitattributes"), "p*.txt filter=broken\n").unwrap();
        git(repo, &["add", ".gitattributes"]);
        git(repo, &["commit", "-q", "-m", "attributes"]);
        git(repo, &["switch", "-q", "main"]);
        git(
            repo,
            &[
                "config",
                "filter.broken.process",
                "no-such-git-lfs filter-process",
            ],
        );
        git(repo, &["config", "filter.broken.required", "true"]);
    }

    fn head(repo: &Path) -> String {
        git(repo, &["rev-parse", "--abbrev-ref", "HEAD"])
    }

    fn read_file(repo: &Path, name: &str) -> String {
        std::fs::read_to_string(repo.join(name)).unwrap()
    }

    fn hook(repo: &Path, name: &str, body: &str) {
        use std::os::unix::fs::PermissionsExt;
        let path = repo.join(".git/hooks").join(name);
        std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    fn local(name: &str) -> Branch {
        Branch::new_local(name.into())
    }

    fn remote(name: &str) -> Branch {
        Branch {
            remote: true,
            ..local(name)
        }
    }

    fn keys(repo: &Path) -> Vec<String> {
        fingerprint_now(repo)
    }

    fn record(
        refname: &str,
        symref: &str,
        head: &str,
        wt: &str,
        date: i64,
        subject: &str,
    ) -> String {
        format!("{refname}\0{symref}\0{head}\0{wt}\0{date}\0{subject}\x1e\n")
    }

    #[test]
    fn refs_parse_current_first_then_locals_then_remotes_nothing_local_shadows() {
        let out = [
            record("refs/heads/newest", "", " ", "", 30, "newest work"),
            record("refs/heads/main", "", "*", "/repo", 20, "on main"),
            record(
                "refs/remotes/origin/HEAD",
                "refs/remotes/origin/main",
                " ",
                "",
                20,
                "",
            ),
            record("refs/remotes/origin/main", "", " ", "", 20, "on main"),
            record(
                "refs/heads/held",
                "",
                " ",
                "/repo-worktrees/held",
                10,
                "held",
            ),
            record(
                "refs/remotes/origin/only-remote",
                "",
                " ",
                "",
                5,
                "far away",
            ),
            record("refs/remotes/upstream/feat/deep", "", " ", "", 4, "nested"),
        ]
        .concat();
        let rows = parse_refs(&out);
        let names: Vec<&str> = rows.iter().map(|b| b.name.as_str()).collect();
        assert_eq!(
            names,
            [
                "main",
                "newest",
                "held",
                "origin/only-remote",
                "upstream/feat/deep"
            ],
            "origin/HEAD is a pointer and origin/main has a local twin"
        );
        assert!(rows[0].current && rows[0].checked_out_at.is_none());
        assert_eq!(rows[2].checked_out_at, Some("/repo-worktrees/held".into()));
        assert!(rows[3].remote && rows[3].local_name() == "only-remote");
        assert_eq!(rows[4].local_name(), "feat/deep");
        assert_eq!(rows[1].subject, "newest work");
        assert_eq!(rows[1].committed, 30);
    }

    #[test]
    fn git_errors_read_as_the_error_line_without_its_prefix() {
        assert_eq!(
            git_error("error: pathspec 'x' did not match\nhint: whatever\n"),
            "pathspec 'x' did not match"
        );
        assert_eq!(
            git_error("\nfatal: not a git repository\n"),
            "not a git repository"
        );
        assert_eq!(
            git_error("Switched to branch 'feature'\nfatal: hook said no\n"),
            "hook said no",
            "the error, not the chatter before it"
        );
        assert_eq!(git_error("something odd\n"), "something odd");
        assert_eq!(git_error(""), "git failed");
    }

    #[test]
    fn cells_count_wide_characters_twice_and_fit_cuts_by_them() {
        assert_eq!(cells("ab"), 2);
        assert_eq!(cells("修复"), 4);
        let cut = fit("修复登录重定向", 7);
        assert!(cells(&cut) <= 7, "{cut:?}");
        assert!(cut.ends_with('…'));
        assert_eq!(fit("short", 10), "short");
    }

    #[test]
    fn a_real_repo_lists_its_branches_its_remotes_and_the_ones_held_elsewhere() {
        let dir = tempfile::tempdir().unwrap();
        let origin = repo(&dir);
        git(&origin, &["branch", "only-on-origin"]);
        let clone = dir.path().join("clone");
        git(
            dir.path(),
            &["clone", "-q", origin.to_str().unwrap(), "clone"],
        );
        git(&clone, &["branch", "held"]);
        git(
            &clone,
            &[
                "worktree",
                "add",
                "-q",
                dir.path().join("wt").to_str().unwrap(),
                "held",
            ],
        );
        let rows = list_branches(&clone).unwrap();
        let names: Vec<&str> = rows.iter().map(|b| b.name.as_str()).collect();
        assert_eq!(names[0], "main", "the current branch leads: {names:?}");
        assert!(rows[0].current);
        assert!(names.contains(&"origin/feature"), "{names:?}");
        assert!(names.contains(&"origin/only-on-origin"), "{names:?}");
        assert!(!names.contains(&"origin/main"), "main is local: {names:?}");
        assert!(!names.iter().any(|n| n.ends_with("/HEAD")), "{names:?}");
        let held = rows.iter().find(|b| b.name == "held").unwrap();
        assert!(held.checked_out_at.is_some(), "{held:?}");
    }

    #[test]
    fn a_clean_checkout_switches_without_asking() {
        let dir = tempfile::tempdir().unwrap();
        let repo = repo(&dir);
        let outcome = switch(&repo, "main", &local("feature"), &Carry::Ask);
        assert_eq!(
            outcome,
            Outcome::Switched {
                branch: "feature".into(),
                note: None
            }
        );
        assert_eq!(head(&repo), "feature");
    }

    #[test]
    fn a_dirty_checkout_asks_and_touches_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let repo = repo(&dir);
        std::fs::write(repo.join("b.txt"), "edited\n").unwrap();
        std::fs::write(repo.join("new.txt"), "untracked\n").unwrap();
        let Outcome::Dirty { files, keys } = switch(&repo, "main", &local("feature"), &Carry::Ask)
        else {
            panic!("a dirty checkout must ask");
        };
        assert_eq!(files.len(), 2);
        assert_eq!(keys.len(), 2);
        assert_eq!(head(&repo), "main");
        assert_eq!(read_file(&repo, "b.txt"), "edited\n");
    }

    #[test]
    fn a_nested_repository_is_not_a_change_and_is_never_committed() {
        let dir = tempfile::tempdir().unwrap();
        let repo = repo(&dir);
        let nested = repo.join(".claude/worktrees/x");
        std::fs::create_dir_all(&nested).unwrap();
        git(&nested, &["init", "-q", "-b", "x"]);
        git(
            &nested,
            &[
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "commit",
                "-q",
                "--allow-empty",
                "-m",
                "n",
            ],
        );
        assert!(changes(&repo).unwrap().is_empty(), "{:?}", changes(&repo));
        assert!(matches!(
            switch(&repo, "main", &local("feature"), &Carry::Ask),
            Outcome::Switched { .. }
        ));

        std::fs::write(repo.join("b.txt"), "edited\n").unwrap();
        let carry = Carry::Commit("keep b".into());
        let outcome = switch(&repo, "feature", &local("main"), &carry);
        assert!(matches!(outcome, Outcome::Switched { .. }), "{outcome:?}");
        let tree = git(&repo, &["ls-tree", "-r", "--name-only", "feature"]);
        assert!(
            !tree.contains(".claude"),
            "no gitlink for the nested repo: {tree}"
        );
    }

    #[test]
    fn stash_and_switch_leaves_the_changes_in_a_named_stash() {
        let dir = tempfile::tempdir().unwrap();
        let repo = repo(&dir);
        std::fs::write(repo.join("a.txt"), "collides\n").unwrap();
        std::fs::write(repo.join("new.txt"), "untracked\n").unwrap();
        let Outcome::Switched { branch, note } =
            switch(&repo, "main", &local("feature"), &Carry::Stash)
        else {
            panic!("stash then switch");
        };
        assert_eq!(branch, "feature");
        assert!(note.unwrap().contains("stashed"));
        assert_eq!(head(&repo), "feature");
        assert!(
            !repo.join("new.txt").exists(),
            "untracked files are stashed too"
        );
        let stashes = git(&repo, &["stash", "list"]);
        assert!(
            stashes.contains("nebula: main before switching to feature"),
            "{stashes}"
        );
    }

    #[test]
    fn a_refused_switch_after_the_stash_applies_its_own_entry_back_by_commit() {
        let dir = tempfile::tempdir().unwrap();
        let repo = repo(&dir);
        // Someone else's entry, already on the shared stack.
        std::fs::write(repo.join("b.txt"), "someone else\n").unwrap();
        git(&repo, &["stash", "push", "-q", "-m", "someone else"]);
        std::fs::write(repo.join("b.txt"), "edited\n").unwrap();
        // `git switch --track origin/nope` — no such remote branch.
        let Outcome::Failed(_) = switch(&repo, "main", &remote("origin/nope"), &Carry::Stash)
        else {
            panic!("the switch must fail");
        };
        assert_eq!(head(&repo), "main");
        assert_eq!(read_file(&repo, "b.txt"), "edited\n");
        let stashes = git(&repo, &["stash", "list"]);
        assert_eq!(
            stashes.lines().count(),
            1,
            "only the other entry is left: {stashes}"
        );
        assert!(stashes.contains("someone else"), "{stashes}");
    }

    #[test]
    fn a_checkout_git_abandons_part_way_is_undone_and_the_stash_applied_back() {
        let dir = tempfile::tempdir().unwrap();
        let repo = repo(&dir);
        lfs_branch(&repo);
        std::fs::write(repo.join("a.txt"), "mine\n").unwrap();
        std::fs::write(repo.join("mine.txt"), "an untracked file of mine\n").unwrap();
        let Outcome::Failed(error) = switch(&repo, "main", &local("lfs"), &Carry::Stash) else {
            panic!("the checkout dies, and is put back");
        };
        assert!(error.contains("part-way"), "{error}");
        assert_eq!(head(&repo), "main");
        assert_eq!(
            read_file(&repo, "a.txt"),
            "mine\n",
            "the stash applied back"
        );
        assert_eq!(read_file(&repo, "mine.txt"), "an untracked file of mine\n");
        assert!(
            !repo.join(".gitattributes").exists(),
            "what git wrote is gone"
        );
        assert_eq!(git(&repo, &["stash", "list"]), "");
    }

    #[test]
    fn a_part_way_checkout_leaves_an_edit_that_is_not_the_targets_alone() {
        let dir = tempfile::tempdir().unwrap();
        let repo = repo(&dir);
        lfs_branch(&repo);
        let Outcome::Failed(error) = switch(&repo, "main", &local("lfs"), &Carry::Ask) else {
            panic!("a clean checkout's part-way switch is undone");
        };
        assert!(error.contains("part-way"), "{error}");
        assert_eq!(read_file(&repo, "a.txt"), "main\n");
        assert!(changes(&repo).unwrap().is_empty());

        // Bring: the user's own edit is not the target's bytes, so the undo
        // must not touch it — the switch is reported as having changed the
        // checkout only if git's writes could not all be undone.
        std::fs::write(repo.join("b.txt"), "edited\n").unwrap();
        let outcome = switch(&repo, "main", &local("lfs"), &Carry::Bring);
        assert!(
            matches!(&outcome, Outcome::Failed(e) if e.contains("part-way")),
            "{outcome:?}"
        );
        assert_eq!(read_file(&repo, "b.txt"), "edited\n");
        assert_eq!(read_file(&repo, "a.txt"), "main\n");
    }

    #[test]
    fn a_failing_post_checkout_hook_is_still_a_switch_and_the_stash_stays_put() {
        let dir = tempfile::tempdir().unwrap();
        let repo = repo(&dir);
        hook(&repo, "post-checkout", "echo 'lfs is missing' >&2; exit 1");
        std::fs::write(repo.join("a.txt"), "collides\n").unwrap();
        let Outcome::Switched { branch, note } =
            switch(&repo, "main", &local("feature"), &Carry::Stash)
        else {
            panic!("HEAD moved, so it switched");
        };
        assert_eq!(branch, "feature");
        let note = note.unwrap();
        assert!(
            note.contains("stashed") && note.contains("complained"),
            "{note}"
        );
        assert_eq!(head(&repo), "feature");
        assert_eq!(
            read_file(&repo, "a.txt"),
            "feature\n",
            "no stash popped onto feature"
        );
        assert!(git(&repo, &["stash", "list"]).contains("nebula: main"));
    }

    #[test]
    fn bring_keeps_changes_that_do_not_collide_and_refuses_ones_that_do() {
        let dir = tempfile::tempdir().unwrap();
        let repo = repo(&dir);
        std::fs::write(repo.join("b.txt"), "edited\n").unwrap();
        let outcome = switch(&repo, "main", &local("feature"), &Carry::Bring);
        assert!(matches!(outcome, Outcome::Switched { .. }), "{outcome:?}");
        assert_eq!(read_file(&repo, "b.txt"), "edited\n");

        std::fs::write(repo.join("a.txt"), "collides\n").unwrap();
        let Outcome::Failed(error) = switch(&repo, "feature", &local("main"), &Carry::Bring) else {
            panic!("a colliding change must refuse");
        };
        assert!(error.contains("stash or commit"), "{error}");
        assert_eq!(head(&repo), "feature");
    }

    #[test]
    fn commit_and_switch_commits_everything_first() {
        let dir = tempfile::tempdir().unwrap();
        let repo = repo(&dir);
        std::fs::write(repo.join("a.txt"), "collides\n").unwrap();
        std::fs::write(repo.join("new.txt"), "untracked\n").unwrap();
        let carry = Carry::Commit("wip before feature".into());
        let Outcome::Switched { note, .. } = switch(&repo, "main", &local("feature"), &carry)
        else {
            panic!("commit then switch");
        };
        assert!(note.unwrap().starts_with("committed "));
        assert_eq!(head(&repo), "feature");
        assert_eq!(
            git(&repo, &["log", "-1", "--format=%s", "main"]),
            "wip before feature"
        );
        let tree = git(&repo, &["ls-tree", "--name-only", "main"]);
        assert!(
            tree.contains("new.txt"),
            "untracked files are committed: {tree}"
        );
    }

    #[test]
    fn commit_is_refused_on_a_detached_head() {
        let dir = tempfile::tempdir().unwrap();
        let repo = repo(&dir);
        git(&repo, &["switch", "-q", "--detach", "main"]);
        std::fs::write(repo.join("b.txt"), "edited\n").unwrap();
        let before = git(&repo, &["rev-parse", "HEAD"]);
        let carry = Carry::Commit("lost".into());
        let Outcome::Failed(error) = switch(&repo, "detached", &local("feature"), &carry) else {
            panic!("a detached commit must be refused");
        };
        assert!(error.contains("detached"), "{error}");
        assert_eq!(git(&repo, &["rev-parse", "HEAD"]), before);
        assert_eq!(read_file(&repo, "b.txt"), "edited\n");
    }

    #[test]
    fn a_commit_a_hook_refuses_puts_the_index_back_exactly() {
        let dir = tempfile::tempdir().unwrap();
        let repo = repo(&dir);
        hook(&repo, "pre-commit", "exit 1");
        std::fs::write(repo.join("b.txt"), "staged\n").unwrap();
        git(&repo, &["add", "b.txt"]);
        std::fs::write(repo.join("ita.txt"), "intent to add\n").unwrap();
        git(&repo, &["add", "-N", "ita.txt"]);
        std::fs::write(repo.join("new.txt"), "not staged\n").unwrap();
        let carry = Carry::Commit("blocked".into());
        let Outcome::Failed(error) = switch(&repo, "main", &local("feature"), &carry) else {
            panic!("the hook refuses");
        };
        assert!(error.starts_with("commit failed"), "{error}");
        assert_eq!(git(&repo, &["diff", "--cached", "--name-only"]), "b.txt");
        let status = git(&repo, &["status", "--porcelain"]);
        assert!(
            status.contains(" A ita.txt"),
            "intent-to-add survives: {status}"
        );
        assert!(status.contains("?? new.txt"), "{status}");
        assert_eq!(head(&repo), "main");
    }

    #[test]
    fn nothing_switches_in_the_middle_of_a_merge() {
        let dir = tempfile::tempdir().unwrap();
        let repo = repo(&dir);
        std::fs::write(repo.join("a.txt"), "main again\n").unwrap();
        git(&repo, &["commit", "-q", "-am", "diverge"]);
        let merge = std::process::Command::new("git")
            .arg("-C")
            .arg(&repo)
            .args(["merge", "-q", "feature"])
            .output()
            .unwrap();
        assert!(!merge.status.success(), "the merge conflicts");
        for carry in [Carry::Ask, Carry::Commit("markers".into()), Carry::Stash] {
            let Outcome::Failed(error) = switch(&repo, "main", &local("feature"), &carry) else {
                panic!("{carry:?} must refuse mid-merge");
            };
            assert!(error.contains("middle of a merge"), "{error}");
        }
        assert_eq!(head(&repo), "main");
        assert!(read_file(&repo, "a.txt").contains("<<<<<<<"));
    }

    #[test]
    fn discard_drops_tracked_changes_and_keeps_untracked_and_newly_added_files() {
        let dir = tempfile::tempdir().unwrap();
        let repo = repo(&dir);
        std::fs::write(repo.join("a.txt"), "collides\n").unwrap();
        std::fs::write(repo.join("added.txt"), "staged, never committed\n").unwrap();
        git(&repo, &["add", "added.txt"]);
        std::fs::write(repo.join("untracked.txt"), "untracked\n").unwrap();
        let carry = Carry::Discard(keys(&repo));
        let outcome = switch(&repo, "main", &local("feature"), &carry);
        assert!(matches!(outcome, Outcome::Switched { .. }), "{outcome:?}");
        assert_eq!(read_file(&repo, "a.txt"), "feature\n");
        assert_eq!(read_file(&repo, "added.txt"), "staged, never committed\n");
        assert!(repo.join("untracked.txt").exists());
    }

    #[test]
    fn discard_asks_again_when_the_changes_moved_since_the_prompt() {
        let dir = tempfile::tempdir().unwrap();
        let repo = repo(&dir);
        std::fs::write(repo.join("a.txt"), "collides\n").unwrap();
        let stale = keys(&repo);
        std::fs::write(repo.join("b.txt"), "an agent wrote this meanwhile\n").unwrap();
        let outcome = switch(&repo, "main", &local("feature"), &Carry::Discard(stale));
        assert!(
            matches!(&outcome, Outcome::Dirty { files, .. } if files.len() == 2),
            "{outcome:?}"
        );
        assert_eq!(read_file(&repo, "b.txt"), "an agent wrote this meanwhile\n");

        // Another edit to a file the prompt already listed moves it too.
        let shown = keys(&repo);
        std::fs::write(repo.join("a.txt"), "collides, and then some more\n").unwrap();
        let outcome = switch(&repo, "main", &local("feature"), &Carry::Discard(shown));
        assert!(matches!(outcome, Outcome::Dirty { .. }), "{outcome:?}");
        assert_eq!(read_file(&repo, "a.txt"), "collides, and then some more\n");
        assert_eq!(head(&repo), "main");
    }

    #[test]
    fn a_remote_branch_becomes_a_tracking_branch() {
        let dir = tempfile::tempdir().unwrap();
        let origin = repo(&dir);
        git(
            dir.path(),
            &["clone", "-q", origin.to_str().unwrap(), "clone"],
        );
        let clone = dir.path().join("clone");
        let outcome = switch(&clone, "main", &remote("origin/feature"), &Carry::Ask);
        assert_eq!(
            outcome,
            Outcome::Switched {
                branch: "feature".into(),
                note: None
            }
        );
        assert_eq!(head(&clone), "feature");
        assert_eq!(
            git(&clone, &["rev-parse", "--abbrev-ref", "feature@{upstream}"]),
            "origin/feature"
        );
    }

    #[test]
    fn create_starts_a_branch_off_head_with_the_changes_along_but_never_a_remotes_name() {
        let dir = tempfile::tempdir().unwrap();
        let origin = repo(&dir);
        git(
            dir.path(),
            &["clone", "-q", origin.to_str().unwrap(), "clone"],
        );
        let clone = dir.path().join("clone");
        std::fs::write(clone.join("b.txt"), "edited\n").unwrap();
        let Outcome::Failed(error) = switch(&clone, "main", &local("origin/x"), &Carry::Create)
        else {
            panic!("origin/x would shadow the remote branch");
        };
        assert!(error.contains("origin"), "{error}");

        let outcome = switch(&clone, "main", &local("brand-new"), &Carry::Create);
        assert!(matches!(outcome, Outcome::Switched { .. }), "{outcome:?}");
        assert_eq!(head(&clone), "brand-new");
        assert_eq!(read_file(&clone, "b.txt"), "edited\n");
    }

    #[test]
    fn a_fetch_with_no_remotes_finishes_and_a_raised_quit_stops_one() {
        let dir = tempfile::tempdir().unwrap();
        let repo = repo(&dir);
        assert!(fetch(&repo, &AtomicBool::new(false)));
        // Whatever the fetch was doing, a raised flag ends the wait at once.
        let started = Instant::now();
        let _ = fetch(&repo, &AtomicBool::new(true));
        assert!(started.elapsed() < FETCH_GRACE + Duration::from_secs(1));
    }

    // ---- the modal ----

    /// An App whose one project's root checkout is `path`.
    fn app_on(path: &Path) -> App {
        let mut app = App::new();
        app.tree.projects.push(Project {
            id: ProjectId("p1".into()),
            name: "demo".into(),
            repo_path: path.to_path_buf(),
            sort_order: 0,
        });
        app.tree.worktrees.push(Worktree {
            id: WorktreeId("w1".into()),
            project_id: ProjectId("p1".into()),
            path: path.to_path_buf(),
            branch: "main".into(),
            is_main: true,
            sort_order: 0,
        });
        app
    }

    fn w1() -> WorktreeId {
        WorktreeId("w1".into())
    }

    fn key(app: &mut App, code: KeyCode) {
        handle_key(app, KeyEvent::new(code, KeyModifiers::NONE));
    }

    fn type_text(app: &mut App, text: &str) {
        for c in text.chars() {
            key(app, KeyCode::Char(c));
        }
    }

    fn view(app: &App) -> &BranchSwitchView {
        match &app.overlay {
            Some(Overlay::BranchSwitch(view)) => view,
            other => panic!("no branch switcher: {other:?}"),
        }
    }

    fn view_mut(app: &mut App) -> &mut BranchSwitchView {
        match &mut app.overlay {
            Some(Overlay::BranchSwitch(view)) => view,
            other => panic!("no branch switcher: {other:?}"),
        }
    }

    fn screen(app: &mut App, w: u16, h: u16) -> String {
        let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
        term.draw(|f| {
            let Some(Overlay::BranchSwitch(v)) = app.overlay.clone() else {
                panic!("no switcher");
            };
            draw(f, app, &v, app.theme);
        })
        .unwrap();
        let buf = term.backend().buffer().clone();
        (0..h)
            .map(|y| {
                (0..w)
                    .map(|x| buf[(x, y)].symbol().to_string())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn the_cursor_opens_on_the_newest_branch_it_can_switch_to_and_the_filter_narrows() {
        let dir = tempfile::tempdir().unwrap();
        let repo = repo(&dir);
        git(&repo, &["branch", "release-1.2", "feature"]);
        // Same tip, so git's name order puts it ahead of `feature` — and a
        // worktree holds it, so the cursor must pass it by.
        let held = dir.path().join("held");
        git(
            &repo,
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                "aaa-held",
                held.to_str().unwrap(),
                "feature",
            ],
        );
        let mut app = app_on(&repo);
        open_for(&mut app, &w1());
        let v = view(&app);
        assert!(v.listed);
        assert_eq!(v.changes, Some(0));
        assert_eq!(
            v.selected_branch().unwrap().name,
            "feature",
            "c Enter goes to a branch it can switch to"
        );

        type_text(&mut app, "rel");
        assert_eq!(view(&app).matches.len(), 1);
        assert_eq!(view(&app).selected_branch().unwrap().name, "release-1.2");
        key(&mut app, KeyCode::Esc);
        assert_eq!(
            view(&app).query.as_str(),
            "",
            "the first Esc clears the query"
        );
        key(&mut app, KeyCode::Esc);
        assert!(app.overlay.is_none(), "the second closes");
    }

    #[test]
    fn enter_switches_a_clean_root_and_renames_its_row_at_once() {
        let dir = tempfile::tempdir().unwrap();
        let repo = repo(&dir);
        let mut app = app_on(&repo);
        open_for(&mut app, &w1());
        type_text(&mut app, "feat");
        key(&mut app, KeyCode::Enter);
        assert!(app.overlay.is_none(), "{:?}", app.overlay);
        assert_eq!(head(&repo), "feature");
        assert_eq!(app.tree.worktrees[0].branch, "feature");
        assert_eq!(app.flash.as_deref(), Some("⌂ root is on feature"));
        assert!(app.branch_switch.switching.is_empty());
    }

    #[test]
    fn enter_with_nothing_matching_creates_the_typed_branch() {
        let dir = tempfile::tempdir().unwrap();
        let repo = repo(&dir);
        let mut app = app_on(&repo);
        open_for(&mut app, &w1());
        type_text(&mut app, "brand-new");
        assert!(view(&app).matches.is_empty());
        assert!(screen(&mut app, 110, 30).contains("Enter creates \"brand-new\" off main"));
        key(&mut app, KeyCode::Enter);
        assert_eq!(head(&repo), "brand-new");
        assert_eq!(app.flash.as_deref(), Some("⌂ root is on brand-new"));
    }

    #[test]
    fn enter_on_the_current_branch_or_one_held_elsewhere_says_why_not() {
        let dir = tempfile::tempdir().unwrap();
        let repo = repo(&dir);
        git(
            &repo,
            &[
                "worktree",
                "add",
                "-q",
                dir.path().join("wt").to_str().unwrap(),
                "feature",
            ],
        );
        let mut app = app_on(&repo);
        open_for(&mut app, &w1());
        type_text(&mut app, "main");
        key(&mut app, KeyCode::Enter);
        assert_eq!(view(&app).status.as_ref().unwrap().text, "already on main");
        key(&mut app, KeyCode::Esc);
        type_text(&mut app, "feature");
        key(&mut app, KeyCode::Enter);
        let status = view(&app).status.clone().unwrap();
        assert!(
            status.error && status.text.contains("checked out in"),
            "{status:?}"
        );
        assert_eq!(head(&repo), "main");
    }

    #[test]
    fn a_dirty_root_asks_and_s_stashes_then_switches() {
        let dir = tempfile::tempdir().unwrap();
        let repo = repo(&dir);
        std::fs::write(repo.join("a.txt"), "collides\n").unwrap();
        let mut app = app_on(&repo);
        open_for(&mut app, &w1());
        type_text(&mut app, "feat");
        key(&mut app, KeyCode::Enter);
        assert!(
            matches!(view(&app).stage, Stage::Dirty { .. }),
            "{:?}",
            view(&app).stage
        );
        assert!(app.branch_switch.switching.is_empty(), "the ask is over");
        let text = screen(&mut app, 110, 30);
        assert!(text.contains("main has 1 uncommitted change"), "{text}");
        assert!(
            text.contains("Stash & switch") && text.contains("Discard & switch"),
            "{text}"
        );
        assert!(text.contains("M a.txt"), "{text}");

        key(&mut app, KeyCode::Esc);
        assert!(
            matches!(view(&app).stage, Stage::Pick),
            "Esc backs out to the list"
        );
        key(&mut app, KeyCode::Enter);
        key(&mut app, KeyCode::Char('s'));
        assert!(app.overlay.is_none());
        assert_eq!(head(&repo), "feature");
        assert!(app.flash.as_deref().unwrap().contains("stashed"));
    }

    #[test]
    fn discard_needs_a_second_press() {
        let dir = tempfile::tempdir().unwrap();
        let repo = repo(&dir);
        std::fs::write(repo.join("a.txt"), "collides\n").unwrap();
        let mut app = app_on(&repo);
        open_for(&mut app, &w1());
        type_text(&mut app, "feat");
        key(&mut app, KeyCode::Enter);
        key(&mut app, KeyCode::Char('d'));
        let Stage::Dirty { discard_armed, .. } = view(&app).stage else {
            panic!("still asking");
        };
        assert!(discard_armed);
        assert!(screen(&mut app, 110, 30).contains("press d again"));
        assert_eq!(head(&repo), "main");
        key(&mut app, KeyCode::Char('d'));
        assert_eq!(head(&repo), "feature");
        assert_eq!(read_file(&repo, "a.txt"), "feature\n");
    }

    #[test]
    fn c_asks_for_a_message_and_commits_before_switching() {
        let dir = tempfile::tempdir().unwrap();
        let repo = repo(&dir);
        std::fs::write(repo.join("a.txt"), "collides\n").unwrap();
        let mut app = app_on(&repo);
        open_for(&mut app, &w1());
        type_text(&mut app, "feat");
        key(&mut app, KeyCode::Enter);
        key(&mut app, KeyCode::Char('c'));
        assert!(matches!(view(&app).stage, Stage::Commit { .. }));
        key(&mut app, KeyCode::Enter);
        assert!(
            view(&app).status.as_ref().unwrap().error,
            "an empty message is refused"
        );
        type_text(&mut app, "save my work");
        assert!(screen(&mut app, 110, 30).contains("save my work"));
        key(&mut app, KeyCode::Enter);
        assert!(app.overlay.is_none(), "{:?}", app.overlay);
        assert_eq!(head(&repo), "feature");
        assert_eq!(
            git(&repo, &["log", "-1", "--format=%s", "main"]),
            "save my work"
        );
    }

    #[test]
    fn a_refused_bring_goes_back_to_the_prompt_with_the_reason() {
        let dir = tempfile::tempdir().unwrap();
        let repo = repo(&dir);
        std::fs::write(repo.join("a.txt"), "collides\n").unwrap();
        let mut app = app_on(&repo);
        open_for(&mut app, &w1());
        type_text(&mut app, "feat");
        key(&mut app, KeyCode::Enter);
        key(&mut app, KeyCode::Char('b'));
        let v = view(&app);
        let Stage::Dirty { choice, .. } = v.stage else {
            panic!("back on the prompt: {:?}", v.stage);
        };
        assert_eq!(CHOICES[choice], Choice::Bring);
        assert!(v.status.as_ref().unwrap().text.contains("stash or commit"));
        assert_eq!(head(&repo), "main");
    }

    /// A switch that failed after changing the checkout — a commit that went
    /// in — can't go back to a prompt listing changes that are gone: it lands
    /// on the list with the reason, and the count is asked for again.
    #[test]
    fn a_stopped_switch_lands_on_the_list_not_a_stale_prompt() {
        let dir = tempfile::tempdir().unwrap();
        let repo = repo(&dir);
        std::fs::write(repo.join("a.txt"), "collides\n").unwrap();
        let mut app = app_on(&repo);
        open_for(&mut app, &w1());
        let files = changes(&repo).unwrap();
        let job = Job {
            request: 1,
            target: local("feature"),
            carry: Carry::Commit("went in".into()),
            keys: fingerprints(&repo, &files),
            files,
        };
        app.branch_switch.switching.insert(w1(), job.clone());
        view_mut(&mut app).stage = Stage::Working(job);
        land_answer(
            &mut app,
            Answer::Switched {
                worktree: w1(),
                request: 1,
                outcome: Outcome::Stopped("committed abc on main, but the switch failed".into()),
            },
        );
        let v = view(&app);
        assert!(matches!(v.stage, Stage::Pick), "{:?}", v.stage);
        assert!(v
            .status
            .as_ref()
            .unwrap()
            .text
            .contains("but the switch failed"));
        assert_eq!(v.changes, Some(1), "asked again");
    }

    /// One switch per checkout: a running one reopens as the working modal,
    /// `Enter` can't start a second, and the answer to a hidden modal lands
    /// in the footer — including a dirty checkout, which has to be said.
    #[test]
    fn a_running_switch_blocks_a_second_and_its_hidden_answer_lands_in_the_footer() {
        let dir = tempfile::tempdir().unwrap();
        let repo = repo(&dir);
        let mut app = app_on(&repo);
        let job = Job {
            request: 7,
            target: local("feature"),
            carry: Carry::Ask,
            files: Vec::new(),
            keys: Vec::new(),
        };
        app.branch_switch.switching.insert(w1(), job);
        app.branch_switch.requests = 7;

        open_for(&mut app, &w1());
        assert!(matches!(view(&app).stage, Stage::Working(ref j) if j.request == 7));
        key(&mut app, KeyCode::Enter);
        assert!(
            matches!(view(&app).stage, Stage::Working(_)),
            "Enter is inert"
        );

        view_mut(&mut app).stage = Stage::Pick;
        type_text(&mut app, "feat");
        key(&mut app, KeyCode::Enter);
        let status = view(&app).status.clone().unwrap();
        assert!(status.text.contains("already running"), "{status:?}");
        assert_eq!(head(&repo), "main");

        let running = app.branch_switch.switching[&w1()].clone();
        view_mut(&mut app).stage = Stage::Working(running);
        key(&mut app, KeyCode::Esc);
        assert!(app.overlay.is_none(), "Esc hides the working modal");

        // A stale answer is nobody's.
        land_answer(
            &mut app,
            Answer::Switched {
                worktree: w1(),
                request: 3,
                outcome: Outcome::Failed("stale".into()),
            },
        );
        assert!(app.branch_switch.switching.contains_key(&w1()));

        let files = vec![DiffFile {
            path: "a.txt".into(),
            orig_path: None,
            xy: [' ', 'M'],
        }];
        land_answer(
            &mut app,
            Answer::Switched {
                worktree: w1(),
                request: 7,
                outcome: Outcome::Dirty {
                    files,
                    keys: Vec::new(),
                },
            },
        );
        assert!(app.branch_switch.switching.is_empty());
        assert_eq!(
            app.flash.as_deref(),
            Some("not switched: main has 1 uncommitted change — c to choose what happens to them")
        );
    }

    #[test]
    fn a_cached_listing_takes_current_from_the_tree_and_the_cursor_goes_home_until_moved() {
        let mut app = app_on(Path::new("/nonexistent-nebula-branch-switch"));
        let mut main = local("main");
        main.current = true;
        let rows = vec![main, local("feature"), local("other")];
        app.branch_switch.lists.insert(w1(), rows);
        app.tree.worktrees[0].branch = "feature".into();
        open_for(&mut app, &w1());
        let v = view(&app);
        let current: Vec<&str> = v
            .branches
            .iter()
            .filter(|b| b.current)
            .map(|b| b.name.as_str())
            .collect();
        assert_eq!(current, ["feature"], "an agent switched it meanwhile");
        assert_eq!(v.branches[0].name, "feature", "current leads");
        assert_eq!(v.selected_branch().unwrap().name, "main");

        // Untouched, a landing listing sends the cursor home; moved, it
        // keeps its branch.
        let mut v = BranchSwitchView::new(w1(), "/x".into(), "demo".into(), "main".into(), 0);
        v.set_branches(rows_named(&["*main", "feature", "other"]));
        assert_eq!(v.selected_branch().unwrap().name, "feature");
        v.select(2);
        v.set_branches(rows_named(&["*main", "other", "feature"]));
        assert_eq!(v.selected_branch().unwrap().name, "other");
    }

    /// Rows from names, `*` marking the current one.
    fn rows_named(names: &[&str]) -> Vec<Branch> {
        names
            .iter()
            .map(|n| {
                let mut b = local(n.trim_start_matches('*'));
                b.current = n.starts_with('*');
                b
            })
            .collect()
    }

    #[test]
    fn a_wide_subject_never_pushes_the_tag_and_age_off_the_row() {
        let th = App::new().theme;
        let mut branch = remote("origin/修复-login");
        branch.subject =
            "✨ 修复登录重定向 ✨ 修复登录重定向 ✨ 修复登录重定向 ✨ 修复登录重定向".into();
        branch.committed = 100;
        for width in [90, 60, 40] {
            let spans = branch_row(&branch, &[], width, 100 + 7200, th);
            let used: usize = spans.iter().map(|s| s.width()).sum();
            assert!(used < width, "{used} cells in a {width}-cell row");
            assert_eq!(spans.last().unwrap().content, "2h ago");
        }
    }

    #[test]
    fn the_list_draws_rows_tags_and_the_status_line_and_survives_a_tiny_frame() {
        let dir = tempfile::tempdir().unwrap();
        let repo = repo(&dir);
        std::fs::write(repo.join("b.txt"), "edited\n").unwrap();
        let mut app = app_on(&repo);
        open_for(&mut app, &w1());
        let text = screen(&mut app, 120, 30);
        assert!(
            text.contains("Switch branch — demo ⌂ root · on main"),
            "{text}"
        );
        let current = text
            .lines()
            .find(|l| l.contains("current"))
            .unwrap_or_else(|| panic!("no current row: {text}"));
        assert!(
            current.contains("current just now │"),
            "the tag and age end one column short of the frame: {current:?}"
        );
        assert!(text.contains("feature work"), "subjects show: {text}");
        assert!(text.contains("1 uncommitted change"), "{text}");

        land_answer(
            &mut app,
            Answer::Fetched {
                worktree: w1(),
                ok: false,
            },
        );
        assert!(screen(&mut app, 120, 30).contains("couldn't reach the remotes"));
        for (w, h) in [(20, 6), (8, 3), (1, 1)] {
            screen(&mut app, w, h);
        }
    }

    #[test]
    fn a_checkout_that_is_not_on_disk_says_so() {
        let mut app = app_on(Path::new("/nonexistent-nebula-branch-switch"));
        open_for(&mut app, &w1());
        let v = view(&app);
        assert!(v.list_error.as_deref().unwrap().contains("not on disk"));
        assert!(screen(&mut app, 100, 24).contains("couldn't list branches"));
    }
}
