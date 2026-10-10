//! The DELETE CHECK as a confirm shows it. A dialog that would take a
//! worktree off disk — `d` on its band, the worktree folded into a card's
//! delete, `Shift+D` — asks the DAEMON what the checkout still holds
//! (`ClientRequest::CheckWorktree`) and lists it under the question:
//! processes working in it, uncommitted changes and how recently they were
//! written, commits its base lacks. Until the answer lands, Enter waits —
//! the user never confirms a delete the check could still warn about — but
//! only for [`CHECK_PATIENCE`]; a check that never answers says so and
//! lets the confirm through. A daemon too old to answer is not asked, and
//! its confirm reads as it always did.
//!
//! Why the rows are not enough: an agent whose session lives in one
//! checkout often works in another through absolute paths — a background
//! subagent keeps its parent's working directory — so a band that reads
//! "nothing running" can have a build running in it and edits nobody
//! committed.
//!
//! From a daemon at [`BRANCH_LANDING_PROTOCOL`] the check also says where
//! the checkout's branch stands, and the confirm decides it: a branch
//! landed on its base (by content — squash merges and cherry-picks count)
//! goes with the checkout by default (the **Delete landed branch**
//! setting), `k` keeping it; a branch with unlanded commits is kept by
//! default, `D` discarding it. The confirm is the decision: the DAEMON
//! deletes what it was told, short of the branches no delete takes.

use crate::app::{App, PendingAction};
use nebula_core::{
    ClientRequest, WorktreeCheck, WorktreeId, BRANCH_LANDING_PROTOCOL, CHECK_WORKTREE_PROTOCOL,
};
use std::collections::HashMap;
use std::time::{Duration, Instant};

/// How long a confirm waits on the check before letting Enter through
/// anyway. Long enough for `git status` on a big checkout with
/// submodules; short enough that a wedged daemon never traps the user.
pub(crate) const CHECK_PATIENCE: Duration = Duration::from_secs(10);

/// Process names a check line spells out before it says "+N more".
const NAMED_PROCESSES: usize = 3;

/// The checks behind the open confirm, by worktree. Emptied when the
/// confirm closes, so reopening it asks again.
#[derive(Debug, Default)]
pub struct DeleteChecks {
    entries: HashMap<WorktreeId, CheckState>,
    /// Checks still out when their confirm closed: their answer, or their
    /// error, is dropped without a word.
    dropped: std::collections::HashSet<u64>,
    /// The **Delete landed branch** setting, read when the confirm's
    /// checks were asked for (a confirm is modal: it cannot change under
    /// one), so drawing never reads the config file.
    delete_landed: bool,
    /// The footer line said while a confirm waits for its check, so its
    /// answer can take it down again — and nothing else's.
    status: Option<String>,
}

#[derive(Debug, Clone)]
enum CheckState {
    Pending { req_id: u64, since: Instant },
    Done(Box<WorktreeCheck>),
    Failed(String),
}

/// The worktrees `action` takes off disk once confirmed.
pub(crate) fn worktrees_deleted_by(action: &PendingAction) -> Vec<WorktreeId> {
    match action {
        PendingAction::DeleteWorktree(id) => vec![id.clone()],
        PendingAction::ThenDeleteWorktree { worktree, .. } => vec![worktree.clone()],
        PendingAction::DeleteAllWorktrees(ids) => ids.clone(),
        _ => Vec::new(),
    }
}

/// The worktrees the open confirm would delete; empty with no confirm.
fn confirm_targets(app: &App) -> Vec<WorktreeId> {
    match &app.modals.overlay {
        Some(crate::app::Overlay::Confirm(c)) => worktrees_deleted_by(&c.action),
        _ => Vec::new(),
    }
}

/// Keep the checks in step with the open confirm: ask about each worktree
/// it would delete that has not been asked about, and forget the rest.
/// Run by the event loop after every turn, so every way a confirm opens —
/// key, menu, a card's delete — is covered without each knowing about it.
pub(crate) fn sync(app: &mut App, out: &mut Vec<ClientRequest>) {
    let targets = confirm_targets(app);
    let checks = &mut app.requests.delete_checks;
    let mut dropped = Vec::new();
    checks.entries.retain(|id, state| {
        let keep = targets.contains(id);
        if let (false, CheckState::Pending { req_id, .. }) = (keep, &*state) {
            dropped.push(*req_id);
        }
        keep
    });
    checks.dropped.extend(dropped);
    if targets.is_empty() {
        clear_status(app);
    }
    if app.requests.daemon_protocol < CHECK_WORKTREE_PROTOCOL {
        return;
    }
    if targets
        .iter()
        .any(|id| !app.requests.delete_checks.entries.contains_key(id))
    {
        app.requests.delete_checks.delete_landed =
            crate::config::Config::load().delete_landed_branch;
    }
    let asking: Vec<WorktreeId> = targets
        .iter()
        .filter(|id| !app.requests.delete_checks.entries.contains_key(*id))
        .cloned()
        .collect();
    if let [one] = asking.as_slice() {
        let status = format!("checking '{}' for work in progress…", branch_of(app, one));
        app.chrome.flash = Some(status.clone());
        app.requests.delete_checks.status = Some(status);
    } else if !asking.is_empty() {
        let status = format!("checking {} worktrees for work in progress…", asking.len());
        app.chrome.flash = Some(status.clone());
        app.requests.delete_checks.status = Some(status);
    }
    for id in targets {
        if app.requests.delete_checks.entries.contains_key(&id) {
            continue;
        }
        let req_id = app.alloc_req_id(crate::app::PendingIntent::None);
        out.push(ClientRequest::CheckWorktree {
            req_id,
            id: id.clone(),
            git_only: false,
        });
        app.requests.delete_checks.entries.insert(
            id,
            CheckState::Pending {
                req_id,
                since: Instant::now(),
            },
        );
    }
}

/// Say `status` in the footer while the confirm waits on its check; the
/// answer takes it down ([`clear_status`]).
pub(crate) fn say_status(app: &mut App, status: String) {
    app.chrome.flash = Some(status.clone());
    app.requests.delete_checks.status = Some(status);
}

/// Take down the footer's "checking…" line, if it is still ours.
fn clear_status(app: &mut App) {
    let Some(status) = app.requests.delete_checks.status.take() else {
        return;
    };
    if app.chrome.flash.as_deref() == Some(status.as_str()) {
        app.chrome.flash = None;
        app.chrome.dirty = true;
    }
}

/// The open confirm's oldest check still out, when it went out.
fn oldest_pending(app: &App) -> Option<Instant> {
    confirm_targets(app)
        .iter()
        .filter_map(|id| match app.requests.delete_checks.entries.get(id)? {
            CheckState::Pending { since, .. } => Some(*since),
            _ => None,
        })
        .min()
}

/// Wake once when an outstanding check times out, so an idle dialog
/// updates its warning without waiting for another key or daemon event.
pub(crate) fn timeout_at(app: &App) -> Option<Instant> {
    let deadline = oldest_pending(app)? + CHECK_PATIENCE;
    (deadline > Instant::now()).then_some(deadline)
}

/// Make every check the open confirm waits on as old as `by` — for tests
/// of what a slow check shows.
#[cfg(test)]
pub(crate) fn age_checks(app: &mut App, by: Duration) {
    for state in app.requests.delete_checks.entries.values_mut() {
        if let CheckState::Pending { since, .. } = state {
            *since -= by;
        }
    }
}

/// The DAEMON's answer to a check. One the confirm has stopped waiting on
/// — it closed, or was reopened and asked again — is dropped.
pub(crate) fn land(app: &mut App, req_id: u64, id: WorktreeId, check: WorktreeCheck) {
    app.requests.pending.remove(&req_id);
    app.requests.delete_checks.dropped.remove(&req_id);
    if let Some(state) = app.requests.delete_checks.entries.get_mut(&id) {
        if matches!(state, CheckState::Pending { req_id: r, .. } if *r == req_id) {
            *state = CheckState::Done(Box::new(check.clone()));
            app.chrome.dirty = true;
            if oldest_pending(app).is_none() {
                clear_status(app);
            }
            // The newest word on the checkout: its band shows it too.
            crate::band_safety::note(app, &id, check);
        }
    }
}

/// Where a checkout's branch stands, as its finished check says — what
/// the confirm offers to do with it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum BranchStanding {
    /// Every commit is on `base` by content: deleting it loses nothing.
    Landed { base: String },
    /// `commits` are on the branch and not on `base`.
    Unlanded { commits: u32, base: String },
}

/// Where `id`'s branch stands, once its check has said — and only when it
/// said for certain: a DAEMON older than [`BRANCH_LANDING_PROTOCOL`], a
/// check still out or failed, a detached HEAD or a base it could not find
/// all leave the branch out of it, and kept.
pub(crate) fn branch_standing(app: &App, id: &WorktreeId) -> Option<BranchStanding> {
    if app.requests.daemon_protocol < BRANCH_LANDING_PROTOCOL {
        return None;
    }
    let CheckState::Done(check) = app.requests.delete_checks.entries.get(id)? else {
        return None;
    };
    standing_of(check)
}

fn standing_of(check: &WorktreeCheck) -> Option<BranchStanding> {
    let landing = check.landing.as_ref()?;
    if check.detached || check.git_error.is_some() {
        return None;
    }
    let base = check.base.clone()?;
    // A branch the delete could take is one the check named, with its tip
    // — and never mid-operation: a rebase or an update under way has the
    // branch in flux, so it stays whatever the check counted.
    landing.branch.as_ref()?;
    if landing.git_op.is_some() {
        return None;
    }
    if landing.branch_landed {
        Some(BranchStanding::Landed { base })
    } else if check.ahead > 0 {
        Some(BranchStanding::Unlanded {
            commits: check.ahead,
            base,
        })
    } else {
        // Nothing of its own, yet not landed: the base's own branch, or
        // the root's, which no delete takes.
        None
    }
}

/// The branch and tip `id`'s finished check judged — what a delete that
/// takes the branch hands back, so the DAEMON deletes exactly that.
pub(crate) fn judged_tip(app: &App, id: &WorktreeId) -> Option<nebula_core::BranchTip> {
    branch_standing(app, id)?;
    let CheckState::Done(check) = app.requests.delete_checks.entries.get(id)? else {
        return None;
    };
    check.branch_tip().cloned()
}

/// What `Enter` does with `id`'s branch: delete it at the tip the check
/// judged — only a landed one, and only with **Delete landed branch** on
/// — or keep it (None).
pub(crate) fn default_delete_branch(app: &App, id: &WorktreeId) -> Option<nebula_core::BranchTip> {
    let landed = matches!(
        branch_standing(app, id),
        Some(BranchStanding::Landed { .. })
    );
    if landed && app.requests.delete_checks.delete_landed {
        judged_tip(app, id)
    } else {
        None
    }
}

/// The branch keys a worktree's own confirm offers beside `Enter`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BranchKeys {
    /// `Enter` deletes the branch too; `k` keeps it.
    DeleteByDefault,
    /// `Enter` keeps the branch; `D` deletes it, and its `unlanded`
    /// commits with it.
    KeepByDefault { unlanded: u32 },
}

/// The worktree whose branch the confirm for `action` decides: a band's
/// own `d`, or a card's delete that takes its emptied worktree too — one
/// worktree either way (a bulk delete takes the default for each).
pub(crate) fn branch_decided_by(action: &PendingAction) -> Option<&WorktreeId> {
    match action {
        PendingAction::DeleteWorktree(id) => Some(id),
        PendingAction::ThenDeleteWorktree { worktree, .. } => Some(worktree),
        _ => None,
    }
}

/// The branch keys the confirm for `action` offers, once its check has
/// said where the branch stands ([`branch_decided_by`]).
pub(crate) fn branch_keys(app: &App, action: &PendingAction) -> Option<BranchKeys> {
    let id = branch_decided_by(action)?;
    Some(match branch_standing(app, id)? {
        BranchStanding::Landed { .. } if app.requests.delete_checks.delete_landed => {
            BranchKeys::DeleteByDefault
        }
        BranchStanding::Landed { .. } => BranchKeys::KeepByDefault { unlanded: 0 },
        BranchStanding::Unlanded { commits, .. } => BranchKeys::KeepByDefault { unlanded: commits },
    })
}

/// An `Error` the DAEMON sent for a check: the confirm says the check
/// failed instead of the footer flashing it. True when `req_id` was a
/// check's, and the error is handled.
pub(crate) fn land_error(app: &mut App, req_id: u64, message: &str) -> bool {
    if app.requests.delete_checks.dropped.remove(&req_id) {
        app.requests.pending.remove(&req_id);
        return true;
    }
    let Some(state) = app
        .requests
        .delete_checks
        .entries
        .values_mut()
        .find(|s| matches!(s, CheckState::Pending { req_id: r, .. } if *r == req_id))
    else {
        return false;
    };
    *state = CheckState::Failed(message.to_string());
    app.requests.pending.remove(&req_id);
    app.chrome.dirty = true;
    if oldest_pending(app).is_none() {
        clear_status(app);
    }
    true
}

/// The branch of a worktree `action` deletes whose check is still out and
/// still within [`CHECK_PATIENCE`] — the reason Enter waits.
pub(crate) fn still_checking(app: &App, action: &PendingAction) -> Option<String> {
    worktrees_deleted_by(action).into_iter().find_map(|id| {
        match app.requests.delete_checks.entries.get(&id)? {
            CheckState::Pending { since, .. } if since.elapsed() < CHECK_PATIENCE => {
                Some(branch_of(app, &id))
            }
            _ => None,
        }
    })
}

fn branch_of(app: &App, id: &WorktreeId) -> String {
    app.tree
        .worktrees
        .iter()
        .find(|w| &w.id == id)
        .map_or_else(|| id.0.clone(), |w| w.branch.clone())
}

/// One line under a confirm's question; `warn` lines are findings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CheckLine {
    pub text: String,
    pub warn: bool,
}

impl CheckLine {
    fn note(text: String) -> Self {
        Self { text, warn: false }
    }
    fn warn(text: String) -> Self {
        Self { text, warn: true }
    }
}

/// What the confirm for `action` says about its worktrees' checks: one
/// line per finding for a single worktree, one line per worktree with
/// findings for a bulk delete. Nothing when no check was asked for — an
/// older DAEMON, or a dialog that deletes no worktree.
pub(crate) fn lines(app: &App, action: &PendingAction, now_ms: i64) -> Vec<CheckLine> {
    let ids = worktrees_deleted_by(action);
    let states: Vec<(String, &CheckState)> = ids
        .iter()
        .filter_map(|id| {
            Some((
                branch_of(app, id),
                app.requests.delete_checks.entries.get(id)?,
            ))
        })
        .collect();
    // Whether a landed branch goes with its checkout: what `Enter` does
    // ([`default_delete_branch`]), said before it does it.
    let delete_landed = app.requests.delete_checks.delete_landed
        && app.requests.daemon_protocol >= BRANCH_LANDING_PROTOCOL;
    match states.as_slice() {
        [] => Vec::new(),
        [(branch, state)] if ids.len() == 1 => single_lines(branch, state, now_ms, delete_landed),
        _ => bulk_lines(&states, now_ms, delete_landed),
    }
}

fn single_lines(
    branch: &str,
    state: &CheckState,
    now_ms: i64,
    delete_landed: bool,
) -> Vec<CheckLine> {
    match state {
        CheckState::Pending { since, .. } if since.elapsed() < CHECK_PATIENCE => {
            vec![CheckLine::note(format!(
                "Checking '{branch}' for work in progress…"
            ))]
        }
        CheckState::Pending { .. } => vec![CheckLine::warn(format!(
            "⚠ Checking '{branch}' is taking too long — confirm only if nothing is working there."
        ))],
        CheckState::Failed(message) => vec![CheckLine::warn(format!(
            "⚠ Could not check '{branch}': {message}"
        ))],
        CheckState::Done(check) if check.landing.is_some() => {
            landing_lines(check, now_ms, delete_landed)
        }
        CheckState::Done(check) if !check.has_findings() => vec![CheckLine::note(
            "Checked: nothing is running in it, uncommitted, or missing from its base.".into(),
        )],
        CheckState::Done(check) => findings(check, now_ms)
            .into_iter()
            .map(|f| CheckLine::warn(format!("⚠ {f}")))
            .collect(),
    }
}

/// A finished check that says where the branch stands: what the delete
/// destroys first, each a `⚠` line as before, then the branch — kept with
/// its unlanded commits, or landed and going with the checkout (unless
/// **Delete landed branch** is off).
fn landing_lines(check: &WorktreeCheck, now_ms: i64, delete_landed: bool) -> Vec<CheckLine> {
    let standing = standing_of(check);
    // The branch's own commits are its line below, not a warning: the
    // branch keeps them.
    let branch_line = matches!(standing, Some(BranchStanding::Unlanded { .. }));
    let mut lines: Vec<CheckLine> = findings_but(check, now_ms, branch_line)
        .into_iter()
        .map(|f| CheckLine::warn(format!("⚠ {f}")))
        .collect();
    let unpushed = check.submodule_unpushed();
    if unpushed > 0 {
        lines.push(CheckLine::warn(format!(
            "⚠ {unpushed} {} in its submodules {} on no remote, and {} with it",
            plural(unpushed as usize, "commit", "commits"),
            plural(unpushed as usize, "is", "are"),
            plural(unpushed as usize, "goes", "go"),
        )));
    }
    if let Some(line) = ignored_line(check) {
        lines.push(CheckLine::warn(line));
    }
    // A git stopped part-way (a running one is a process line already).
    if let Some(op) = check
        .landing
        .as_ref()
        .filter(|l| !l.git_op_live)
        .and_then(|l| l.git_op.as_ref())
    {
        lines.push(CheckLine::warn(format!(
            "⚠ a git {op} is stopped part-way in it; its branch is kept"
        )));
    }
    if lines.is_empty() && !matches!(standing, Some(BranchStanding::Unlanded { .. })) {
        lines.push(CheckLine::note(
            "Checked: nothing is running in it, uncommitted, or unlanded.".into(),
        ));
    }
    match standing {
        Some(BranchStanding::Landed { base }) if delete_landed => lines.push(CheckLine::note(
            format!("Its branch has landed on {base} and is deleted with it."),
        )),
        Some(BranchStanding::Landed { base }) => {
            lines.push(CheckLine::note(format!("Its branch has landed on {base}.")))
        }
        Some(BranchStanding::Unlanded { commits, base }) => {
            let n = commits as usize;
            lines.push(CheckLine::note(format!(
                "{n} {} on this branch and not on {base}.",
                plural(n, "commit is", "commits are")
            )));
        }
        None => {}
    }
    lines
}

fn bulk_lines(
    states: &[(String, &CheckState)],
    now_ms: i64,
    delete_landed: bool,
) -> Vec<CheckLine> {
    let waiting = states
        .iter()
        .filter(|(_, s)| matches!(s, CheckState::Pending { since, .. } if since.elapsed() < CHECK_PATIENCE))
        .count();
    let mut lines: Vec<CheckLine> = states
        .iter()
        .filter_map(|(branch, state)| match state {
            CheckState::Pending { since, .. } if since.elapsed() < CHECK_PATIENCE => None,
            CheckState::Pending { .. } => Some(format!("'{branch}': not checked in time")),
            CheckState::Failed(message) => Some(format!("'{branch}': could not check: {message}")),
            CheckState::Done(check) if check.has_findings() => Some(format!(
                "'{branch}': {}",
                short_findings(check, now_ms).join(" · ")
            )),
            CheckState::Done(_) => None,
        })
        .map(|text| CheckLine::warn(format!("⚠ {text}")))
        .collect();
    if waiting > 0 {
        lines.push(CheckLine::note(format!(
            "Checking {waiting} worktree(s) for work in progress…"
        )));
    } else if lines.is_empty() {
        lines.push(CheckLine::note(
            "Checked: nothing is running in them, uncommitted, or missing from a base.".into(),
        ));
    }
    let landed = states
        .iter()
        .filter(|(_, state)| {
            matches!(state, CheckState::Done(check)
                if matches!(standing_of(check), Some(BranchStanding::Landed { .. })))
        })
        .count();
    if delete_landed && waiting == 0 && landed > 0 {
        lines.push(CheckLine::note(format!(
            "{landed} landed {} deleted with {}.",
            plural(landed, "branch is", "branches are"),
            plural(landed, "its worktree", "their worktrees"),
        )));
    }
    lines
}

/// The gitignored files a delete takes that are not build output — which
/// `git status` never shows, so nothing else would: `⚠ 2 ignored files
/// are deleted too: .env, .claude/settings.local.json`.
fn ignored_line(check: &WorktreeCheck) -> Option<String> {
    let landing = check.landing.as_ref()?;
    let total = landing.ignored_total as usize;
    if total == 0 {
        return None;
    }
    let mut names = landing.ignored_files.join(", ");
    let rest = total.saturating_sub(landing.ignored_files.len());
    if rest > 0 {
        names.push_str(&format!(" and {rest} more"));
    }
    Some(format!(
        "⚠ {total} ignored {} deleted too: {names}",
        plural(total, "file is", "files are")
    ))
}

/// A finished check's findings, one sentence each.
fn findings(check: &WorktreeCheck, now_ms: i64) -> Vec<String> {
    findings_but(check, now_ms, false)
}

/// [`findings`], less the branch's unlanded commits when `skip_ahead`:
/// the confirm words those as the branch's own line.
fn findings_but(check: &WorktreeCheck, now_ms: i64, skip_ahead: bool) -> Vec<String> {
    let mut out = Vec::new();
    // First: the one finding someone stated on purpose.
    match check.locked.as_deref() {
        Some("") => {
            out.push("locked (git worktree lock, no reason given) — deleting unlocks it".into())
        }
        Some(reason) => out.push(format!(
            "locked: {} — deleting unlocks it",
            one_line(reason)
        )),
        None => {}
    }
    if !check.processes.is_empty() {
        let n = check.processes.len();
        out.push(format!(
            "{n} {} working in it: {}",
            plural(n, "process is", "processes are"),
            process_names(check)
        ));
    }
    if let Some(e) = &check.process_error {
        out.push(format!("could not list the processes working in it: {e}"));
    }
    if check.changes > 0 {
        let n = check.changes as usize;
        let mut line = format!("{n} uncommitted {}", plural(n, "change", "changes"));
        if let Some(ago) = written_ago(check, now_ms) {
            line.push_str(&format!(", the newest written {ago}"));
        }
        out.push(line);
    }
    if check.ahead > 0 && !skip_ahead {
        let n = check.ahead as usize;
        let commits = plural(n, "commit", "commits");
        out.push(match (&check.base, check.detached) {
            (_, true) => format!(
                "{n} {commits} on a detached HEAD that no branch holds — only the reflog would keep them"
            ),
            (Some(base), false) => {
                format!("{n} {commits} {base} lacks (the branch keeps them)")
            }
            (None, false) => format!("{n} {commits} not on its base (the branch keeps them)"),
        });
    }
    if let Some(e) = &check.git_error {
        out.push(format!("could not ask git about it: {e}"));
    }
    out
}

/// A finished check's findings as a bulk delete's line lists them.
fn short_findings(check: &WorktreeCheck, now_ms: i64) -> Vec<String> {
    let mut out = Vec::new();
    match check.locked.as_deref() {
        Some("") => out.push("locked".into()),
        Some(reason) => out.push(format!("locked: {}", one_line(reason))),
        None => {}
    }
    if !check.processes.is_empty() {
        out.push(format!("running {}", process_names(check)));
    }
    if check.changes > 0 {
        let mut part = format!("{} uncommitted", check.changes);
        if let Some(ago) = written_ago(check, now_ms) {
            part.push_str(&format!(" (newest {ago})"));
        }
        out.push(part);
    }
    if check.ahead > 0 {
        out.push(match (&check.base, check.detached) {
            (_, true) => format!("{} commit(s) only the reflog holds", check.ahead),
            (Some(base), false) => format!("{} commit(s) {base} lacks", check.ahead),
            (None, false) => format!("{} commit(s) off its base", check.ahead),
        });
    }
    let unpushed = check.submodule_unpushed();
    if unpushed > 0 {
        out.push(format!("{unpushed} unpushed in submodules"));
    }
    let ignored = check.ignored_total();
    if ignored > 0 {
        out.push(format!("{ignored} ignored"));
    }
    if check.process_error.is_some() || check.git_error.is_some() {
        out.push("not fully checked".into());
    }
    out
}

/// A lock reason as one line of a dialog: git keeps a reason's newlines.
fn one_line(reason: &str) -> String {
    reason.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn process_names(check: &WorktreeCheck) -> String {
    let mut names: Vec<String> = check
        .processes
        .iter()
        .take(NAMED_PROCESSES)
        .map(|p| format!("{} ({})", p.name, p.pid))
        .collect();
    let rest = check.processes.len().saturating_sub(NAMED_PROCESSES);
    if rest > 0 {
        names.push(format!("+{rest} more"));
    }
    names.join(", ")
}

fn written_ago(check: &WorktreeCheck, now_ms: i64) -> Option<String> {
    let at = check.newest_change_ms?;
    Some(crate::hosts::ago_label(now_ms - at)).filter(|s| !s.is_empty())
}

fn plural<'a>(n: usize, one: &'a str, many: &'a str) -> &'a str {
    if n == 1 {
        one
    } else {
        many
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nebula_core::{WorktreeLanding, WorktreeProcess};

    fn done(check: WorktreeCheck) -> CheckState {
        CheckState::Done(Box::new(check))
    }

    #[test]
    fn a_clean_check_says_so_and_a_busy_one_names_each_finding() {
        let clean = single_lines("feat", &done(WorktreeCheck::default()), 0, true);
        assert_eq!(clean.len(), 1);
        assert!(!clean[0].warn);

        let now = 1_000_000_000;
        let busy = WorktreeCheck {
            processes: (1..=5)
                .map(|pid| WorktreeProcess {
                    pid,
                    name: format!("p{pid}"),
                })
                .collect(),
            changes: 3,
            newest_change_ms: Some(now - 120_000),
            ahead: 2,
            base: Some("origin/main".into()),
            ..WorktreeCheck::default()
        };
        let text: Vec<String> = single_lines("feat", &done(busy), now, true)
            .into_iter()
            .inspect(|l| assert!(l.warn))
            .map(|l| l.text)
            .collect();
        assert_eq!(
            text,
            [
                "⚠ 5 processes are working in it: p1 (1), p2 (2), p3 (3), +2 more",
                "⚠ 3 uncommitted changes, the newest written 2m ago",
                "⚠ 2 commits origin/main lacks (the branch keeps them)",
            ]
        );
    }

    #[test]
    fn a_locked_checkout_leads_with_its_lock_and_reason() {
        let check = WorktreeCheck {
            locked: Some("merge queue:\nreused by every push".into()),
            changes: 1,
            ..WorktreeCheck::default()
        };
        let text: Vec<String> = single_lines("probe", &done(check.clone()), 0, true)
            .into_iter()
            .inspect(|l| assert!(l.warn))
            .map(|l| l.text)
            .collect();
        assert_eq!(
            text,
            [
                "⚠ locked: merge queue: reused by every push — deleting unlocks it",
                "⚠ 1 uncommitted change",
            ]
        );
        assert_eq!(
            short_findings(&check, 0)[0],
            "locked: merge queue: reused by every push"
        );

        let bare = WorktreeCheck {
            locked: Some(String::new()),
            ..WorktreeCheck::default()
        };
        assert!(bare.has_findings(), "a lock alone is a finding");
        let text = &single_lines("probe", &done(bare), 0, true)[0].text;
        assert!(text.contains("no reason given"), "{text}");
    }

    #[test]
    fn a_detached_head_warns_that_only_the_reflog_keeps_its_commits() {
        let check = WorktreeCheck {
            ahead: 1,
            detached: true,
            ..WorktreeCheck::default()
        };
        let text = &single_lines("feat", &done(check), 0, true)[0].text;
        assert!(
            text.contains("detached HEAD") && text.contains("reflog"),
            "{text}"
        );
    }

    #[test]
    fn a_failed_or_partial_check_is_never_read_as_clean() {
        let partial = WorktreeCheck {
            process_error: Some("lsof failed".into()),
            ..WorktreeCheck::default()
        };
        let lines = single_lines("feat", &done(partial), 0, true);
        assert!(lines[0].warn && lines[0].text.contains("lsof failed"));
        let failed = single_lines("feat", &CheckState::Failed("gone".into()), 0, true);
        assert!(failed[0].warn && failed[0].text.contains("gone"));
    }

    #[test]
    fn a_check_out_past_patience_stops_holding_enter_and_says_why() {
        let stale = CheckState::Pending {
            req_id: 1,
            since: Instant::now() - CHECK_PATIENCE,
        };
        let lines = single_lines("feat", &stale, 0, true);
        assert!(lines[0].warn && lines[0].text.contains("too long"));
    }

    #[test]
    fn a_bulk_delete_lists_only_the_worktrees_with_findings() {
        let busy = done(WorktreeCheck {
            changes: 1,
            ..WorktreeCheck::default()
        });
        let clean = done(WorktreeCheck::default());
        let waiting = CheckState::Pending {
            req_id: 9,
            since: Instant::now(),
        };
        let states = [("a".to_string(), &busy), ("b".to_string(), &clean)];
        let lines = bulk_lines(&states, 0, true);
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].text, "⚠ 'a': 1 uncommitted");

        let states = [("a".to_string(), &busy), ("c".to_string(), &waiting)];
        let lines = bulk_lines(&states, 0, true);
        assert_eq!(
            lines.last().unwrap().text,
            "Checking 1 worktree(s) for work in progress…"
        );

        let states = [("b".to_string(), &clean)];
        assert!(!bulk_lines(&states, 0, true)[0].warn);

        let landed = done(WorktreeCheck {
            base: Some("origin/main".into()),
            landing: Some(nebula_core::WorktreeLanding {
                branch_landed: true,
                branch: Some(tip()),
                ..Default::default()
            }),
            ..WorktreeCheck::default()
        });
        let states = [("a".to_string(), &busy), ("d".to_string(), &landed)];
        let lines = bulk_lines(&states, 0, true);
        assert_eq!(
            lines.last().unwrap().text,
            "1 landed branch is deleted with its worktree."
        );
        assert!(bulk_lines(&states, 0, false)
            .iter()
            .all(|l| !l.text.contains("landed")));
    }

    fn texts(lines: Vec<CheckLine>) -> Vec<String> {
        lines.into_iter().map(|l| l.text).collect()
    }

    fn landed() -> WorktreeCheck {
        WorktreeCheck {
            base: Some("origin/main".into()),
            landing: Some(WorktreeLanding {
                branch_landed: true,
                branch: Some(tip()),
                ..WorktreeLanding::default()
            }),
            ..WorktreeCheck::default()
        }
    }

    fn tip() -> nebula_core::BranchTip {
        nebula_core::BranchTip {
            name: "feat".into(),
            oid: "f00d".into(),
        }
    }

    #[test]
    fn a_landed_branch_is_said_to_go_with_the_checkout() {
        assert_eq!(
            texts(single_lines("feat", &done(landed()), 0, true)),
            [
                "Checked: nothing is running in it, uncommitted, or unlanded.",
                "Its branch has landed on origin/main and is deleted with it.",
            ]
        );
        assert_eq!(
            texts(single_lines("feat", &done(landed()), 0, false))[1],
            "Its branch has landed on origin/main.",
            "the setting off keeps it"
        );
        assert_eq!(
            standing_of(&landed()),
            Some(BranchStanding::Landed {
                base: "origin/main".into()
            })
        );
    }

    #[test]
    fn an_unlanded_branch_is_counted_not_warned_about() {
        let check = WorktreeCheck {
            ahead: 4,
            landing: Some(WorktreeLanding {
                branch: Some(tip()),
                ..WorktreeLanding::default()
            }),
            ..landed()
        };
        let lines = single_lines("feat", &done(check.clone()), 0, true);
        assert_eq!(
            texts(lines.clone()),
            ["4 commits are on this branch and not on origin/main."]
        );
        assert!(!lines[0].warn, "the branch keeps them");
        assert_eq!(
            standing_of(&check),
            Some(BranchStanding::Unlanded {
                commits: 4,
                base: "origin/main".into()
            })
        );
    }

    #[test]
    fn what_a_delete_destroys_comes_first_and_submodules_are_named() {
        let check = WorktreeCheck {
            changes: 2,
            landing: Some(WorktreeLanding {
                branch_landed: true,
                submodule_unpushed: 6,
                branch: Some(tip()),
                ..WorktreeLanding::default()
            }),
            ..landed()
        };
        let lines = single_lines("feat", &done(check), 0, true);
        assert_eq!(
            texts(lines.clone()),
            [
                "⚠ 2 uncommitted changes",
                "⚠ 6 commits in its submodules are on no remote, and go with it",
                "Its branch has landed on origin/main and is deleted with it.",
            ]
        );
        assert!(lines[0].warn && lines[1].warn && !lines[2].warn);
    }

    #[test]
    fn a_detached_failed_or_baseless_check_leaves_the_branch_alone() {
        let detached = WorktreeCheck {
            detached: true,
            ..landed()
        };
        assert_eq!(standing_of(&detached), None);
        let failed = WorktreeCheck {
            git_error: Some("x".into()),
            ..landed()
        };
        assert_eq!(standing_of(&failed), None);
        let baseless = WorktreeCheck {
            base: None,
            ..landed()
        };
        assert_eq!(standing_of(&baseless), None);
        let nameless = WorktreeCheck {
            landing: Some(WorktreeLanding {
                branch_landed: true,
                ..WorktreeLanding::default()
            }),
            ..landed()
        };
        assert_eq!(standing_of(&nameless), None, "no branch named, none taken");
        let mid_rebase = WorktreeCheck {
            landing: Some(WorktreeLanding {
                branch_landed: true,
                branch: Some(tip()),
                git_op: Some("rebase".into()),
                ..WorktreeLanding::default()
            }),
            ..landed()
        };
        assert_eq!(standing_of(&mid_rebase), None, "mid-operation: kept");
        let lines = texts(single_lines("feat", &done(mid_rebase), 0, true));
        assert_eq!(
            lines,
            ["⚠ a git rebase is stopped part-way in it; its branch is kept"]
        );
        assert_eq!(standing_of(&WorktreeCheck::default()), None, "old daemon");
    }

    #[test]
    fn ignored_files_that_are_no_build_output_are_named() {
        let check = WorktreeCheck {
            landing: Some(WorktreeLanding {
                branch_landed: true,
                branch: Some(tip()),
                ignored_files: vec![".env".into(), ".claude/settings.local.json".into()],
                ignored_total: 2,
                ..WorktreeLanding::default()
            }),
            ..landed()
        };
        let lines = single_lines("feat", &done(check), 0, true);
        assert_eq!(
            texts(lines.clone()),
            [
                "⚠ 2 ignored files are deleted too: .env, .claude/settings.local.json",
                "Its branch has landed on origin/main and is deleted with it.",
            ]
        );
        assert!(lines[0].warn);

        let many = WorktreeCheck {
            landing: Some(WorktreeLanding {
                ignored_files: vec!["a".into(), "b".into(), "c".into()],
                ignored_total: 5,
                branch: Some(tip()),
                ..WorktreeLanding::default()
            }),
            ..landed()
        };
        assert_eq!(
            ignored_line(&many).as_deref(),
            Some("⚠ 5 ignored files are deleted too: a, b, c and 2 more")
        );
    }
}
