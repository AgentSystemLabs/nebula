//! The BRANCH SWITCHER (`c`): the project's ROOT WORKTREE moved onto
//! another branch without leaving nebula. A fuzzy list of every local
//! branch and every remote branch that has no local twin, filtered as you
//! type; `Enter` switches. A checkout with uncommitted changes stops and
//! asks how they should travel — the choices the IDEs offer at the same
//! moment: stash them, bring them along, commit them first, or throw them
//! away.
//!
//! Only the root checkout switches. A linked worktree is named after the
//! branch it was cut for, and moving it would leave the directory name
//! lying about its contents; git itself refuses to check a branch out in
//! two places, so a branch another worktree holds is listed but refused.
//!
//! Everything here is client-side git, like the diff viewer: the list is
//! one `git for-each-ref` (milliseconds, painted from a per-checkout cache
//! on reopen), the changed-file count one `git status`, and a background
//! `git fetch --all` refreshes the remotes at most once a minute per
//! checkout, re-listing when it lands. Every call runs off the loop with
//! its answer on `App::branch_switch.tx`. The DAEMON is not asked: its
//! worktree sync already notices a root `HEAD` that moved and upserts the
//! row's branch; the TUI renames the row itself the moment git says yes,
//! so the panel never lags the switch by a sync tick.
//!
//! Git that writes (the switch, a stash, a commit) and git that talks to a
//! remote run in a session of their own with stdin closed. The TUI owns a
//! terminal, and an `ssh` asking for a passphrase — or a signing agent
//! asking for a PIN — would otherwise open `/dev/tty` and paint over the
//! frame; detached, it fails instead, and the failure is reported.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

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
    truncate, visible_positions, NO_MATCHES,
};

/// Outer (width, height) of the modal: the find-file modal's footprint,
/// wider for the commit subjects.
const SIZE: (u16, u16) = (92, 24);
/// How long a background fetch may run before it is killed — the PR
/// lookups' budget, for the same reason: a stalled network retries.
const FETCH_TIMEOUT: Duration = Duration::from_secs(20);
/// The least time between two background fetches of one checkout, so
/// opening and closing the modal never hammers a remote.
const FETCH_GAP: Duration = Duration::from_secs(60);
/// `git for-each-ref` record: NUL between fields, RS after each record (a
/// subject is one line, but a record separator costs nothing).
const REF_FORMAT: &str = "--format=%(refname)%00%(symref)%00%(HEAD)%00%(worktreepath)%00%(committerdate:unix)%00%(contents:subject)%1e";

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

/// Every branch the root checkout could switch to, as [`parse_refs`] orders
/// them. `Err` is a one-line message for the modal.
pub fn list_branches(root: &Path) -> Result<Vec<Branch>, String> {
    let out = git_command(root)
        .args(["for-each-ref", "--sort=-committerdate", REF_FORMAT])
        .args(["refs/heads", "refs/remotes"])
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("failed to run git: {e}"))?;
    if !out.status.success() {
        return Err(git_error(&String::from_utf8_lossy(&out.stderr)));
    }
    Ok(parse_refs(&String::from_utf8_lossy(&out.stdout)))
}

/// git's complaint as one line: the first thing it said, without the
/// `error:` / `fatal:` it leads with.
fn git_error(stderr: &str) -> String {
    let line = stderr
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("git failed");
    line.strip_prefix("error: ")
        .or_else(|| line.strip_prefix("fatal: "))
        .unwrap_or(line)
        .to_string()
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

/// `git fetch --all`, killed — with the `ssh` it may have started, which
/// shares its session — past [`FETCH_TIMEOUT`]. True when it finished.
pub fn fetch(root: &Path) -> bool {
    extern "C" {
        fn kill(pid: i32, sig: i32) -> i32;
    }
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
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(50)),
            _ => {
                // The child leads its own process group (setsid), so the
                // negative pid reaches everything it spawned.
                // SAFETY: plain syscall on a pid this process owns.
                unsafe { kill(-(child.id() as i32), 9) };
                let _ = child.wait();
                return false;
            }
        }
    }
}

/// How uncommitted changes travel with a switch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Carry {
    /// Expect a clean checkout: if it isn't, stop and ask.
    Ask,
    /// `git stash push --include-untracked`, then switch.
    Stash,
    /// A plain `git switch`, which keeps changes that don't collide with
    /// the target and refuses otherwise.
    Bring,
    /// Commit everything — untracked files included — with this message,
    /// then switch.
    Commit(String),
    /// `git switch --discard-changes`: tracked changes are thrown away,
    /// untracked files stay.
    Discard,
}

/// What a switch came to.
#[derive(Debug, Clone, PartialEq)]
pub enum Outcome {
    /// The checkout is on `branch`; `note` says what happened to the
    /// changes on the way.
    Switched {
        branch: String,
        note: Option<String>,
    },
    /// [`Carry::Ask`] found these changes; nothing was touched.
    Dirty(Vec<DiffFile>),
    Failed(String),
}

/// The stash's top entry, to tell whether a `stash push` saved anything
/// (it exits 0 with nothing to save).
fn stash_top(root: &Path) -> Option<String> {
    run(root, &["rev-parse", "-q", "--verify", "refs/stash"])
        .ok()
        .map(|s| s.trim().to_string())
}

/// Move the checkout at `root` from `from` onto `target`, carrying its
/// changes as `carry` says. A switch that fails after a stash pops the
/// stash straight back, so a refused switch leaves the checkout as it was.
pub fn switch(root: &Path, from: &str, target: &Branch, carry: &Carry) -> Outcome {
    let to = target.local_name().to_string();
    let mut args = vec!["switch"];
    if *carry == Carry::Discard {
        args.push("--discard-changes");
    }
    if target.remote {
        args.push("--track");
    }
    args.push(&target.name);

    let mut note = None;
    let mut stashed = false;
    match carry {
        Carry::Ask => match crate::git_diff::changed_files(root) {
            Ok(files) if !files.is_empty() => return Outcome::Dirty(files),
            Ok(_) => {}
            Err(e) => return Outcome::Failed(e),
        },
        Carry::Bring | Carry::Discard => {}
        Carry::Stash => {
            let before = stash_top(root);
            let message = format!("nebula: {from} before switching to {to}");
            let pushed = [
                "stash",
                "push",
                "--include-untracked",
                "-m",
                message.as_str(),
            ];
            if let Err(e) = run(root, &pushed) {
                return Outcome::Failed(format!("stash failed: {e}"));
            }
            stashed = stash_top(root) != before;
            if stashed {
                note = Some(format!(
                    "changes stashed (git stash pop on {from} restores them)"
                ));
            }
        }
        Carry::Commit(message) => {
            if let Err(e) = run(root, &["add", "--all"]) {
                return Outcome::Failed(format!("git add failed: {e}"));
            }
            if let Err(e) = run(root, &["commit", "--quiet", "-m", message]) {
                return Outcome::Failed(format!("commit failed: {e}"));
            }
            let sha = run(root, &["rev-parse", "--short", "HEAD"]).unwrap_or_default();
            note = Some(format!("committed {} on {from}", sha.trim()));
        }
    }

    match run(root, &args) {
        Ok(_) => Outcome::Switched { branch: to, note },
        Err(e) => {
            let e = if e.contains("would be overwritten") {
                format!("your changes collide with {to} — stash or commit them instead")
            } else {
                e
            };
            if stashed
                && run(root, &["stash", "pop", "--index"]).is_err()
                && run(root, &["stash", "pop"]).is_err()
            {
                return Outcome::Failed(format!("{e} (your changes are still in the stash)"));
            }
            Outcome::Failed(e)
        }
    }
}

// ---- state ----

/// What the event loop hands back to [`land_answer`].
#[derive(Debug)]
pub enum Answer {
    Listed {
        worktree: WorktreeId,
        list: Result<Vec<Branch>, String>,
    },
    /// The checkout's changed-file count; None when git couldn't say.
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
        outcome: Outcome,
    },
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
    /// Checkouts with a background fetch running…
    pub fetching: HashSet<WorktreeId>,
    /// …and when each last started one ([`FETCH_GAP`]).
    pub fetched: HashMap<WorktreeId, Instant>,
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

    fn detail(self, from: &str, to: &str) -> String {
        match self {
            Choice::Stash => format!("stash them; git stash pop on {from} brings them back"),
            Choice::Bring => format!("keep them on {to} — refused if they collide with it"),
            Choice::Commit => format!("commit everything on {from} first, untracked files too"),
            Choice::Discard => "throw away changes to tracked files; untracked files stay".into(),
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
    /// `discard_armed` is the first press of the destructive choice.
    Dirty {
        target: Branch,
        files: Vec<DiffFile>,
        choice: usize,
        discard_armed: bool,
    },
    /// Typing the message for [`Choice::Commit`].
    Commit {
        target: Branch,
        files: Vec<DiffFile>,
        message: TextInput,
    },
    /// git is running. `files` is what the DIRTY prompt listed (empty for a
    /// clean switch), so a failure goes back to the prompt it came from.
    Working {
        target: Branch,
        carry: Carry,
        files: Vec<DiffFile>,
    },
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
    /// Whether a listing has landed (a cached one counts).
    pub listed: bool,
    pub list_error: Option<String>,
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
            listed: false,
            list_error: None,
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
    /// the first branch the checkout isn't already on — so `c` `Enter`
    /// goes somewhere.
    fn home_selection(&mut self) {
        self.selected = if self.query.as_str().trim().is_empty() {
            self.matches
                .iter()
                .position(|(i, _)| !self.branches[*i].current)
                .unwrap_or(0)
        } else {
            0
        };
    }

    /// A listing landed: the cursor stays on the branch it was on, by
    /// name, or goes home.
    pub fn set_branches(&mut self, branches: Vec<Branch>) {
        let keep = self.selected_branch().map(|b| b.name.clone());
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
    }
}

// ---- opening and answers ----

/// The hotkey. From a WORKTREES PANEL or SESSIONS PANEL row (and the pane)
/// it acts on the selected checkout, which must be the root; from the
/// PROJECTS PANEL, the WORKSPACES BAR, or with no checkout under the
/// cursor (an OPEN PRS row), on the project's root.
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

/// Open the modal on `worktree`: the cached listing (if any) at once, a
/// fresh one and the changed-file count off the loop, and a background
/// fetch when the last one is a minute old.
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
        view.set_branches(cached.clone());
    }
    app.overlay = Some(Overlay::BranchSwitch(view));
    request_list(app, w.id.clone(), w.path.clone());
    request_fetch(app, w.id, w.path);
    app.dirty = true;
}

/// List the branches and count the changes, off the loop (inline in the
/// unit tests). A checkout that isn't on disk is said so without a process.
fn request_list(app: &mut App, worktree: WorktreeId, root: PathBuf) {
    if !root.is_dir() {
        let list = Err(format!("{} is not on disk", root.display()));
        land_answer(app, Answer::Listed { worktree, list });
        return;
    }
    let ask = move |send: &mut dyn FnMut(Answer)| {
        send(Answer::Listed {
            worktree: worktree.clone(),
            list: list_branches(&root),
        });
        let changes = crate::git_diff::changed_files(&root).ok().map(|f| f.len());
        send(Answer::Changes { worktree, changes });
    };
    match app.branch_switch.tx.clone() {
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
/// [`FETCH_GAP`] of the last, and in the unit tests.
fn request_fetch(app: &mut App, worktree: WorktreeId, root: PathBuf) {
    let Some(tx) = app.branch_switch.tx.clone() else {
        return;
    };
    let shared = &mut app.branch_switch;
    let recent = shared
        .fetched
        .get(&worktree)
        .is_some_and(|at| at.elapsed() < FETCH_GAP);
    if !root.is_dir() || recent || shared.fetching.contains(&worktree) {
        return;
    }
    shared.fetching.insert(worktree.clone());
    shared.fetched.insert(worktree.clone(), Instant::now());
    tokio::task::spawn_blocking(move || {
        let ok = fetch(&root);
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
            if let Some(view) = view_for(app, &worktree) {
                view.changes = changes;
            }
        }
        Answer::Fetched { worktree, ok } => {
            app.branch_switch.fetching.remove(&worktree);
            if let Some(root) = view_for(app, &worktree).map(|v| v.root.clone()) {
                if ok {
                    request_list(app, worktree, root);
                }
            }
        }
        Answer::Switched { worktree, outcome } => land_switch(app, worktree, outcome),
    }
    app.dirty = true;
}

fn land_switch(app: &mut App, worktree: WorktreeId, outcome: Outcome) {
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
        Outcome::Dirty(files) => {
            if let Some(view) = view_for(app, &worktree) {
                if let Stage::Working { target, .. } = &view.stage {
                    view.changes = Some(files.len());
                    view.stage = Stage::Dirty {
                        target: target.clone(),
                        files,
                        choice: 0,
                        discard_armed: false,
                    };
                }
            }
        }
        Outcome::Failed(error) => match view_for(app, &worktree) {
            Some(view) => {
                view.stage = match std::mem::replace(&mut view.stage, Stage::Pick) {
                    Stage::Working {
                        target,
                        carry,
                        files,
                    } if !files.is_empty() => Stage::Dirty {
                        target,
                        files,
                        choice: carry_choice(&carry).index(),
                        discard_armed: false,
                    },
                    _ => Stage::Pick,
                };
                view.status = Some(Status::error(error));
            }
            None => app.flash = Some(format!("switch branch failed: {error}")),
        },
    }
}

fn carry_choice(carry: &Carry) -> Choice {
    match carry {
        Carry::Commit(_) => Choice::Commit,
        Carry::Bring => Choice::Bring,
        Carry::Discard => Choice::Discard,
        Carry::Ask | Carry::Stash => Choice::Stash,
    }
}

/// Run the switch off the loop (inline in the unit tests).
fn start_switch(app: &mut App, target: Branch, carry: Carry, files: Vec<DiffFile>) {
    let Some(Overlay::BranchSwitch(view)) = &mut app.overlay else {
        return;
    };
    let (worktree, root, from) = (
        view.worktree.clone(),
        view.root.clone(),
        view.current.clone(),
    );
    view.status = None;
    view.stage = Stage::Working {
        target: target.clone(),
        carry: carry.clone(),
        files,
    };
    match app.branch_switch.tx.clone() {
        Some(tx) => {
            tokio::task::spawn_blocking(move || {
                let outcome = switch(&root, &from, &target, &carry);
                let _ = tx.send(Answer::Switched { worktree, outcome });
            });
        }
        None => {
            let outcome = switch(&root, &from, &target, &carry);
            land_answer(app, Answer::Switched { worktree, outcome });
        }
    }
    app.dirty = true;
}

// ---- keys and mouse ----

/// `Enter` on a row.
fn activate_selected(app: &mut App) {
    let Some(Overlay::BranchSwitch(view)) = &mut app.overlay else {
        return;
    };
    let Some(branch) = view.selected_branch().cloned() else {
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
    start_switch(app, branch, Carry::Ask, Vec::new());
}

/// Pick one of the [`CHOICES`] on the DIRTY prompt.
fn choose(app: &mut App, choice: Choice) {
    let Some(Overlay::BranchSwitch(view)) = &mut app.overlay else {
        return;
    };
    let Stage::Dirty {
        target,
        files,
        choice: at,
        discard_armed,
    } = &mut view.stage
    else {
        return;
    };
    let armed = *discard_armed && *at == choice.index();
    *at = choice.index();
    *discard_armed = false;
    let (target, files) = (target.clone(), files.clone());
    match choice {
        Choice::Stash => start_switch(app, target, Carry::Stash, files),
        Choice::Bring => start_switch(app, target, Carry::Bring, files),
        Choice::Commit => {
            view.status = None;
            view.stage = Stage::Commit {
                target,
                files,
                message: TextInput::new(),
            };
        }
        Choice::Discard if armed => start_switch(app, target, Carry::Discard, files),
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
                    view.apply_filter();
                    view.home_selection();
                }
                KeyCode::Esc => app.overlay = None,
                // j/k stay typeable in the query; Ctrl+n/p mirror ↑/↓.
                KeyCode::Down => view.select(selected + 1),
                KeyCode::Up => view.select(selected - 1),
                KeyCode::Char('n') if ctrl => view.select(selected + 1),
                KeyCode::Char('p') if ctrl => view.select(selected - 1),
                KeyCode::PageDown => view.select(selected + page),
                KeyCode::PageUp => view.select(selected - page),
                KeyCode::Enter => activate_selected(app),
                _ => {
                    if view.query.handle_key(&key).changed() {
                        view.apply_filter();
                        view.home_selection();
                        view.status = None;
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
            message,
        } => match key.code {
            KeyCode::Esc => {
                view.stage = Stage::Dirty {
                    target: target.clone(),
                    files: files.clone(),
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
                    let (target, files) = (target.clone(), files.clone());
                    start_switch(app, target, Carry::Commit(text), files);
                }
            }
            _ => {
                message.handle_key(&key);
            }
        },
        // git is already running; Esc only hides the modal, and the result
        // still lands in the footer.
        Stage::Working { .. } => {
            if key.code == KeyCode::Esc {
                app.overlay = None;
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
            view.apply_filter();
            view.home_selection();
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
    match &mut view.stage {
        Stage::Pick => {
            if delta != 0 {
                view.select(view.selected as i64 + delta);
            } else if mouse.kind == MouseEventKind::Down(MouseButton::Left)
                && view.list_area.contains(pos)
            {
                let start = window_start(view.selected, view.list_area.height as usize);
                view.select((start + (pos.y - view.list_area.y) as usize) as i64);
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
            } else if mouse.kind == MouseEventKind::Down(MouseButton::Left)
                && view.choices_area.contains(pos)
            {
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
        Stage::Pick => "type: filter branches  ↑/↓ ^n/^p: move  Enter: switch  Ctrl+u: clear  Esc: clear/close",
        Stage::Dirty { .. } => "s: stash  b: bring along  c: commit  d: discard  ↑/↓ Enter: choose  Esc: back to the list",
        Stage::Commit { .. } => "type the commit message  Enter: commit & switch  Esc: back",
        Stage::Working { .. } => "git is running  Esc: hide (the result lands in the footer)",
    }
}

fn border_hint(stage: &Stage) -> &'static str {
    match stage {
        Stage::Pick => " Enter switch · ↑↓ move · Esc clear/close ",
        Stage::Dirty { .. } => " s/b/c/d or ↑↓ Enter · Esc back ",
        Stage::Commit { .. } => " Enter commit & switch · Esc back ",
        Stage::Working { .. } => " working… ",
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

/// One list row: glyph, name, subject, then the tag and age pinned right.
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
        ("in a worktree", th.warn)
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
    let right_w = tag.chars().count() + age.chars().count() + usize::from(!tag.is_empty()) + 1;
    let budget = width.saturating_sub(2 + right_w);
    let name = truncate(&branch.name, budget.min(48).max(budget / 2));
    let name_w = name.chars().count();
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
        let subject = truncate(&branch.subject, subject_w);
        used += 2 + subject.chars().count();
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
        } => {
            choices_area = draw_dirty(f, view, target, files, *choice, *discard_armed, body, th);
        }
        Stage::Working {
            target,
            files,
            carry,
        } if !files.is_empty() => {
            let choice = carry_choice(carry).index();
            draw_dirty(f, view, target, files, choice, false, body, th);
        }
        Stage::Commit {
            target,
            files,
            message,
        } => draw_commit(f, view, target, files, message, body, th),
        Stage::Pick | Stage::Working { .. } => {
            (list_area, selected) = draw_list(f, view, body, th);
        }
    }

    // The status line: what just went wrong or right, else what's running,
    // else what the user should know before switching.
    let fetching = app.branch_switch.fetching.contains(&view.worktree);
    let (text, color) = if let Stage::Working { target, carry, .. } = &view.stage {
        let doing = match carry {
            Carry::Stash => "stashing and switching",
            Carry::Commit(_) => "committing and switching",
            Carry::Discard => "discarding and switching",
            Carry::Ask | Carry::Bring => "switching",
        };
        (format!("{doing} to {}…", target.local_name()), th.warn)
    } else if let Some(status) = &view.status {
        (
            status.text.clone(),
            if status.error { th.err } else { th.ok },
        )
    } else {
        let mut parts: Vec<String> = Vec::new();
        if let Some(n) = view.changes.filter(|n| *n > 0) {
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
        }
        (parts.join(" · "), th.dim)
    };
    let text = truncate(&text, status_row.width.saturating_sub(1) as usize);
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
        let text = match (&view.list_error, view.listed) {
            (Some(e), _) => format!("couldn't list branches: {e}"),
            (None, false) => "reading branches…".into(),
            (None, true) if view.branches.is_empty() => "no branches".into(),
            (None, true) => NO_MATCHES.into(),
        };
        empty_list_row(f, list, &text, th);
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
        Span::raw(truncate(&file.path, width.saturating_sub(7))),
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
    if let Some(row) = row_rect(body, 0) {
        let question = format!(
            " {} has {} — how should they travel to {to}?",
            view.current,
            changes_text(files.len())
        );
        f.render_widget(
            Paragraph::new(Span::styled(
                truncate(&question, body.width as usize),
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
        let detail = if armed {
            format!("press d again to throw away {}", changes_text(files.len()))
        } else {
            c.detail(&view.current, to)
        };
        let label_color = if *c == Choice::Discard {
            th.err
        } else {
            th.text
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
                truncate(&detail, (body.width as usize).saturating_sub(26)),
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
                truncate(&line, body.width as usize),
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

    fn head(repo: &Path) -> String {
        git(repo, &["rev-parse", "--abbrev-ref", "HEAD"])
    }

    fn local(name: &str) -> Branch {
        Branch {
            name: name.into(),
            remote: false,
            current: false,
            checked_out_at: None,
            committed: 0,
            subject: String::new(),
        }
    }

    fn remote(name: &str) -> Branch {
        Branch {
            remote: true,
            ..local(name)
        }
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
    fn git_errors_read_as_one_line_without_the_prefix() {
        assert_eq!(
            git_error("error: pathspec 'x' did not match\nhint: whatever\n"),
            "pathspec 'x' did not match"
        );
        assert_eq!(
            git_error("\nfatal: not a git repository\n"),
            "not a git repository"
        );
        assert_eq!(git_error(""), "git failed");
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
        let Outcome::Dirty(files) = switch(&repo, "main", &local("feature"), &Carry::Ask) else {
            panic!("a dirty checkout must ask");
        };
        assert_eq!(files.len(), 2);
        assert_eq!(head(&repo), "main");
        assert_eq!(
            std::fs::read_to_string(repo.join("b.txt")).unwrap(),
            "edited\n"
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
        assert!(note.unwrap().contains("stash"));
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
    fn a_switch_that_fails_after_the_stash_puts_the_changes_back() {
        let dir = tempfile::tempdir().unwrap();
        let repo = repo(&dir);
        std::fs::write(repo.join("b.txt"), "edited\n").unwrap();
        // `git switch --track origin/nope` — no such remote branch.
        let Outcome::Failed(_) = switch(&repo, "main", &remote("origin/nope"), &Carry::Stash)
        else {
            panic!("the switch must fail");
        };
        assert_eq!(head(&repo), "main");
        assert_eq!(
            std::fs::read_to_string(repo.join("b.txt")).unwrap(),
            "edited\n"
        );
        assert_eq!(
            git(&repo, &["stash", "list"]),
            "",
            "the stash was popped back"
        );
    }

    #[test]
    fn bring_keeps_changes_that_do_not_collide_and_refuses_ones_that_do() {
        let dir = tempfile::tempdir().unwrap();
        let repo = repo(&dir);
        std::fs::write(repo.join("b.txt"), "edited\n").unwrap();
        let outcome = switch(&repo, "main", &local("feature"), &Carry::Bring);
        assert!(matches!(outcome, Outcome::Switched { .. }), "{outcome:?}");
        assert_eq!(
            std::fs::read_to_string(repo.join("b.txt")).unwrap(),
            "edited\n"
        );

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
    fn discard_and_switch_drops_tracked_changes_and_keeps_untracked_files() {
        let dir = tempfile::tempdir().unwrap();
        let repo = repo(&dir);
        std::fs::write(repo.join("a.txt"), "collides\n").unwrap();
        std::fs::write(repo.join("new.txt"), "untracked\n").unwrap();
        let outcome = switch(&repo, "main", &local("feature"), &Carry::Discard);
        assert!(matches!(outcome, Outcome::Switched { .. }), "{outcome:?}");
        assert_eq!(
            std::fs::read_to_string(repo.join("a.txt")).unwrap(),
            "feature\n"
        );
        assert!(repo.join("new.txt").exists());
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
    fn a_fetch_with_no_remotes_finishes() {
        let dir = tempfile::tempdir().unwrap();
        assert!(fetch(&repo(&dir)));
    }

    // ---- the modal ----

    /// An App whose one project's root checkout is `path`.
    fn app_on(path: &Path) -> App {
        let mut app = App::new();
        app.tree.projects.push(Project {
            workspace_id: Default::default(),
            id: ProjectId("p1".into()),
            name: "demo".into(),
            repo_path: path.to_path_buf(),
            sort_order: 0,
        });
        let root = Worktree {
            id: WorktreeId("w1".into()),
            project_id: ProjectId("p1".into()),
            path: path.to_path_buf(),
            branch: "main".into(),
            is_main: true,
            sort_order: 0,
        };
        app.tree.worktrees.push(root);
        app
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
    fn the_cursor_opens_on_the_newest_other_branch_and_the_filter_narrows() {
        let dir = tempfile::tempdir().unwrap();
        let repo = repo(&dir);
        git(&repo, &["branch", "release-1.2", "feature"]);
        let mut app = app_on(&repo);
        open_for(&mut app, &WorktreeId("w1".into()));
        let v = view(&app);
        assert!(v.listed);
        assert_eq!(v.changes, Some(0));
        assert!(
            !v.selected_branch().unwrap().current,
            "c Enter goes somewhere"
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
        open_for(&mut app, &WorktreeId("w1".into()));
        type_text(&mut app, "feat");
        key(&mut app, KeyCode::Enter);
        assert!(app.overlay.is_none(), "{:?}", app.overlay);
        assert_eq!(head(&repo), "feature");
        assert_eq!(app.tree.worktrees[0].branch, "feature");
        assert_eq!(app.flash.as_deref(), Some("⌂ root is on feature"));
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
        open_for(&mut app, &WorktreeId("w1".into()));
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
        open_for(&mut app, &WorktreeId("w1".into()));
        type_text(&mut app, "feat");
        key(&mut app, KeyCode::Enter);
        assert!(
            matches!(view(&app).stage, Stage::Dirty { .. }),
            "{:?}",
            view(&app).stage
        );
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
        open_for(&mut app, &WorktreeId("w1".into()));
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
        assert_eq!(
            std::fs::read_to_string(repo.join("a.txt")).unwrap(),
            "feature\n"
        );
    }

    #[test]
    fn c_asks_for_a_message_and_commits_before_switching() {
        let dir = tempfile::tempdir().unwrap();
        let repo = repo(&dir);
        std::fs::write(repo.join("a.txt"), "collides\n").unwrap();
        let mut app = app_on(&repo);
        open_for(&mut app, &WorktreeId("w1".into()));
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
        open_for(&mut app, &WorktreeId("w1".into()));
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

    #[test]
    fn the_list_draws_rows_tags_and_the_status_line_and_survives_a_tiny_frame() {
        let dir = tempfile::tempdir().unwrap();
        let repo = repo(&dir);
        std::fs::write(repo.join("b.txt"), "edited\n").unwrap();
        let mut app = app_on(&repo);
        open_for(&mut app, &WorktreeId("w1".into()));
        let text = screen(&mut app, 120, 30);
        assert!(
            text.contains("Switch branch — demo ⌂ root · on main"),
            "{text}"
        );
        assert!(text.contains("current"), "{text}");
        assert!(text.contains("feature work"), "subjects show: {text}");
        assert!(text.contains("1 uncommitted change"), "{text}");
        for (w, h) in [(20, 6), (8, 3), (1, 1)] {
            screen(&mut app, w, h);
        }
    }

    #[test]
    fn a_checkout_that_is_not_on_disk_says_so() {
        let mut app = app_on(Path::new("/nonexistent-nebula-branch-switch"));
        open_for(&mut app, &WorktreeId("w1".into()));
        let v = view(&app);
        assert!(v.list_error.as_deref().unwrap().contains("not on disk"));
        assert!(screen(&mut app, 100, 24).contains("couldn't list branches"));
    }
}
