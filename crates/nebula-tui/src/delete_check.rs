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

use crate::app::{App, PendingAction};
use nebula_core::{ClientRequest, WorktreeCheck, WorktreeId, CHECK_WORKTREE_PROTOCOL};
use std::collections::HashMap;
use std::time::{Duration, Instant};

/// How long a confirm waits on the check before letting Enter through
/// anyway. Long enough for `git status` on a big checkout with
/// submodules; short enough that a wedged daemon never traps the user.
pub(crate) const CHECK_PATIENCE: Duration = Duration::from_secs(10);

/// How long a confirm waiting on its check stays unshown, the footer
/// saying it is checking. A check that answers within it — any normal
/// one: a fraction of a second, a second or so on a big repo with many
/// submodules — opens the dialog once, already filled in, never a
/// "Checking…" body that then grows into the findings, which reads as a
/// second dialog replacing the first. Past it, something is wrong, and
/// the dialog shows with "Checking…" so the user can see why Enter waits.
pub(crate) const SHOW_AFTER: Duration = Duration::from_secs(5);

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
    /// The footer line said while a confirm is held for its check, so its
    /// answer can take it down again — and nothing else's.
    status: Option<String>,
}

#[derive(Debug, Clone)]
enum CheckState {
    Pending { req_id: u64, since: Instant },
    Done(WorktreeCheck),
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

/// Is the open confirm held back, unshown, for its check? True while a
/// check is out and younger than [`SHOW_AFTER`]. (A confirm opened this
/// turn is asked about by [`sync`] at the end of the turn, before the next
/// frame is drawn.) Only cancelling and `Enter` / `y` (held by
/// [`still_checking`]) reach a held confirm. A DAEMON too old to be asked
/// never holds one: it has no check out.
pub(crate) fn held(app: &App) -> bool {
    oldest_pending(app).is_some_and(|since| since.elapsed() < SHOW_AFTER)
}

/// When a held confirm is to be shown, its check not having answered: the
/// event loop wakes then to draw it.
pub(crate) fn reveal_at(app: &App) -> Option<Instant> {
    if !held(app) {
        return None;
    }
    oldest_pending(app).map(|since| since + SHOW_AFTER)
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
            *state = CheckState::Done(check);
            app.chrome.dirty = true;
            if oldest_pending(app).is_none() {
                clear_status(app);
            }
        }
    }
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
    match states.as_slice() {
        [] => Vec::new(),
        [(branch, state)] if ids.len() == 1 => single_lines(branch, state, now_ms),
        _ => bulk_lines(&states, now_ms),
    }
}

fn single_lines(branch: &str, state: &CheckState, now_ms: i64) -> Vec<CheckLine> {
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
        CheckState::Done(check) if !check.has_findings() => vec![CheckLine::note(
            "Checked: nothing is running in it, uncommitted, or missing from its base.".into(),
        )],
        CheckState::Done(check) => findings(check, now_ms)
            .into_iter()
            .map(|f| CheckLine::warn(format!("⚠ {f}")))
            .collect(),
    }
}

fn bulk_lines(states: &[(String, &CheckState)], now_ms: i64) -> Vec<CheckLine> {
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
    lines
}

/// A finished check's findings, one sentence each.
fn findings(check: &WorktreeCheck, now_ms: i64) -> Vec<String> {
    let mut out = Vec::new();
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
    if check.ahead > 0 {
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
    if check.process_error.is_some() || check.git_error.is_some() {
        out.push("not fully checked".into());
    }
    out
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
    use nebula_core::WorktreeProcess;

    fn done(check: WorktreeCheck) -> CheckState {
        CheckState::Done(check)
    }

    #[test]
    fn a_clean_check_says_so_and_a_busy_one_names_each_finding() {
        let clean = single_lines("feat", &done(WorktreeCheck::default()), 0);
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
        let text: Vec<String> = single_lines("feat", &done(busy), now)
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
    fn a_detached_head_warns_that_only_the_reflog_keeps_its_commits() {
        let check = WorktreeCheck {
            ahead: 1,
            detached: true,
            ..WorktreeCheck::default()
        };
        let text = &single_lines("feat", &done(check), 0)[0].text;
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
        let lines = single_lines("feat", &done(partial), 0);
        assert!(lines[0].warn && lines[0].text.contains("lsof failed"));
        let failed = single_lines("feat", &CheckState::Failed("gone".into()), 0);
        assert!(failed[0].warn && failed[0].text.contains("gone"));
    }

    #[test]
    fn a_check_out_past_patience_stops_holding_enter_and_says_why() {
        let stale = CheckState::Pending {
            req_id: 1,
            since: Instant::now() - CHECK_PATIENCE,
        };
        let lines = single_lines("feat", &stale, 0);
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
        let lines = bulk_lines(&states, 0);
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].text, "⚠ 'a': 1 uncommitted");

        let states = [("a".to_string(), &busy), ("c".to_string(), &waiting)];
        let lines = bulk_lines(&states, 0);
        assert_eq!(
            lines.last().unwrap().text,
            "Checking 1 worktree(s) for work in progress…"
        );

        let states = [("b".to_string(), &clean)];
        assert!(!bulk_lines(&states, 0)[0].warn);
    }
}
