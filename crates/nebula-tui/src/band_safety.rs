//! What an idle band says a delete would lose, before anyone presses `d`.
//!
//! An EMPTY BAND used to read `nothing running` whether its checkout was a
//! merged leftover or the only copy of a day's work, so every one needed a
//! `d` — or a question to an agent — before it could be cleared. Each idle
//! band now carries the DELETE CHECK (`ClientRequest::CheckWorktree` with
//! `band`, which leaves out the gitignored files), asked in the
//! background on the git tick: one band per tick, the least recently read
//! first, at most one in flight, and none read again sooner than
//! [`FRESH_FOR`]. Its verdict takes the place of `nothing running`, in
//! git's own compact vocabulary (`git status -sb`, shell prompts):
//!
//! - `✓ clean · merged` — nothing destroyed, and the branch has landed;
//! - `✓ clean · detached` — nothing destroyed, on no branch;
//! - `↑4 unmerged` — the branch holds commits its base lacks (a delete
//!   keeps the branch, and them);
//! - `⚠ 3 modified · 2 untracked`, `⚠ 6 unpushed in submodules`,
//!   `⚠ detached · 2 unreachable` — what a delete destroys;
//! - `● in use by just, cargo` — a process outside nebula's sessions has
//!   its working directory in the checkout (a build, a shell, a tool's
//!   scratch run): `0 sessions` alone never says that. It leads the git
//!   verdict, `● in use by zsh · clean · detached`, and follows a warning,
//!   `⚠ 3 modified · in use by cargo`.
//!
//! "Merged" is counted by content, not ancestry: a branch landed by squash
//! merge, rebase or cherry-pick reads merged. Before the first read a band
//! says what it always did. A daemon too old to answer is never asked.

use crate::app::{App, PendingIntent};
use nebula_core::{ClientRequest, WorktreeCheck, WorktreeId, BRANCH_LANDING_PROTOCOL};
use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

/// A band read this recently is not read again: an idle checkout changes
/// rarely, and a project with one idle band would otherwise pay a `git
/// status`, a `git cherry` and a walk of its submodules every tick.
pub(crate) const FRESH_FOR: Duration = Duration::from_secs(30);

/// How many unsettled checks in a row — a failure, or a git operation
/// stopped part-way — a band re-reads on the very next tick before it
/// goes back to [`FRESH_FOR`], so a repo that stays broken (or a rebase
/// left for the evening) does not spawn git every two seconds. A git
/// still running in the checkout is re-read every tick however long it
/// takes: it ends on its own.
pub(crate) const FAST_RETRIES: u32 = 3;

/// How long a band check may stay out before the sweep stops waiting for
/// it and moves on; its answer, if it ever comes, is dropped.
pub(crate) const PATIENCE: Duration = Duration::from_secs(30);

/// The band checks: the last answer per checkout and when it came, the
/// one in flight, and the ones given up on.
#[derive(Debug, Default)]
pub struct BandChecks {
    pub entries: HashMap<WorktreeId, (WorktreeCheck, Instant)>,
    inflight: Option<(u64, WorktreeId, Instant)>,
    /// Checks the sweep stopped waiting for: their answer or error is
    /// dropped without a word.
    dropped: HashSet<u64>,
    /// How many unsettled checks in a row each band has had
    /// ([`FAST_RETRIES`]); a settled one clears it.
    unsettled: HashMap<WorktreeId, u32>,
}

/// Is `check` one to read again soon — a failure, or a git operation
/// under way?
fn unsettled(check: &WorktreeCheck) -> bool {
    check.git_error.is_some() || check.landing.as_ref().is_some_and(|l| l.git_op.is_some())
}

/// Is a git process at work in the checkout `check` read?
fn git_running(check: &WorktreeCheck) -> bool {
    check
        .landing
        .as_ref()
        .is_some_and(|l| l.git_op.is_some() && l.git_op_live)
}

/// Ask about one more idle band, when one is due. Run on the git tick.
pub(crate) fn sweep(app: &mut App, out: &mut Vec<ClientRequest>) {
    if app.requests.daemon_protocol < BRANCH_LANDING_PROTOCOL {
        return;
    }
    let tree = &app.tree.worktrees;
    let checks = &mut app.jobs.band_checks;
    checks
        .entries
        .retain(|id, _| tree.iter().any(|w| &w.id == id));
    if let Some((req_id, _, since)) = &checks.inflight {
        if since.elapsed() < PATIENCE {
            return;
        }
        checks.dropped.insert(*req_id);
        checks.inflight = None;
    }
    let Some(id) = target(app) else {
        return;
    };
    let req_id = app.alloc_req_id(PendingIntent::None);
    out.push(ClientRequest::CheckWorktree {
        req_id,
        id: id.clone(),
        band: true,
    });
    app.jobs.band_checks.inflight = Some((req_id, id, Instant::now()));
}

/// The idle band this tick reads: of the EMPTY BANDS the grid draws (the
/// ROOT WORKTREE's aside: it is never deleted), the one read least
/// recently, never read first — and none read within [`FRESH_FOR`],
/// unless its last read was unsettled ([`due`]).
pub(crate) fn target(app: &App) -> Option<WorktreeId> {
    if !app.launcher_grid() {
        return None;
    }
    let entries = &app.jobs.band_checks.entries;
    crate::launcher::bands(app)
        .into_iter()
        .filter(|b| b.cards.is_empty() && !b.is_main && !app.is_placeholder_worktree(&b.worktree))
        .map(|b| b.worktree)
        .filter(|id| due(&app.jobs.band_checks, id))
        .min_by_key(|id| entries.get(id).map(|(_, at)| *at))
}

/// Is band `id` due a read? Never read, read [`FRESH_FOR`] ago, or last
/// read unsettled: a git still running there, or a failure or paused
/// operation within its first [`FAST_RETRIES`].
fn due(checks: &BandChecks, id: &WorktreeId) -> bool {
    let Some((check, at)) = checks.entries.get(id) else {
        return true;
    };
    at.elapsed() >= FRESH_FOR
        || git_running(check)
        || (unsettled(check) && checks.unsettled.get(id).copied().unwrap_or(0) <= FAST_RETRIES)
}

/// A `WorktreeChecked` for the band check in flight: kept for the band's
/// line. False when `req_id` is no band check's, and the answer is
/// someone else's (a confirm's).
pub(crate) fn land(app: &mut App, req_id: u64, id: &WorktreeId, check: &WorktreeCheck) -> bool {
    let checks = &mut app.jobs.band_checks;
    if checks.dropped.remove(&req_id) {
        app.requests.pending.remove(&req_id);
        return true;
    }
    if !matches!(&checks.inflight, Some((r, _, _)) if *r == req_id) {
        return false;
    }
    checks.inflight = None;
    app.requests.pending.remove(&req_id);
    note(app, id, check.clone());
    true
}

/// A confirm's check, or a band's: the band shows the newest. Only a check
/// that says where the branch stands is any use to the band.
pub(crate) fn note(app: &mut App, id: &WorktreeId, check: WorktreeCheck) {
    if check.landing.is_none() {
        return;
    }
    if unsettled(&check) {
        *app.jobs
            .band_checks
            .unsettled
            .entry(id.clone())
            .or_default() += 1;
    } else {
        app.jobs.band_checks.unsettled.remove(id);
    }
    let before = app
        .jobs
        .band_checks
        .entries
        .insert(id.clone(), (check.clone(), Instant::now()));
    if before.is_none_or(|(was, _)| verdict(&was) != verdict(&check)) {
        app.chrome.dirty = true;
    }
}

/// An `Error` for a band check: the band says it could not look, and is
/// asked again in its turn — never a flash, which a background read the
/// user did not ask for has no business raising. True when `req_id` was
/// a band check's.
pub(crate) fn land_error(app: &mut App, req_id: u64, message: &str) -> bool {
    let checks = &mut app.jobs.band_checks;
    if checks.dropped.remove(&req_id) {
        app.requests.pending.remove(&req_id);
        return true;
    }
    let Some((_, id, _)) = checks.inflight.take_if(|(r, _, _)| *r == req_id) else {
        return false;
    };
    app.requests.pending.remove(&req_id);
    let failed = WorktreeCheck {
        git_error: Some(message.to_string()),
        landing: Some(Default::default()),
        ..WorktreeCheck::default()
    };
    note(app, &id, failed);
    true
}

/// How a band's verdict is drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Tone {
    /// Nothing a delete loses: dim, like the rest of the line.
    Clean,
    /// Commits the branch keeps.
    Ahead,
    /// What a delete destroys: the warning color.
    Warn,
    /// Something outside nebula's sessions is working in the checkout:
    /// the warning color, though a delete would not destroy it.
    InUse,
}

/// What a band's line says about its checkout.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Verdict {
    pub text: String,
    pub tone: Tone,
}

/// Process names a band lists before it says "+N".
const IN_USE_NAMED: usize = 2;

/// The band's verdict on `check`: None when there is nothing to say — a
/// daemon that did not say where the branch stands, or a branch that is
/// its base's own, with nothing at work in it.
pub(crate) fn verdict(check: &WorktreeCheck) -> Option<Verdict> {
    let git = git_verdict(check);
    if check.landing.is_none() || check.processes.is_empty() || git_running(check) {
        // A git at work names itself (`⋯ git submodule update running`).
        return git;
    }
    let in_use = format!("in use by {}", process_names(check));
    Some(match git {
        Some(v) if v.tone == Tone::Warn => Verdict {
            text: format!("{} · {in_use}", v.text),
            tone: Tone::Warn,
        },
        Some(v) => Verdict {
            text: format!("● {in_use} · {}", v.text.trim_start_matches("✓ ")),
            tone: Tone::InUse,
        },
        None => Verdict {
            text: format!("● {in_use}"),
            tone: Tone::InUse,
        },
    })
}

/// The distinct names of the processes at work in the checkout, in pid
/// order, the first [`IN_USE_NAMED`] of them and how many more.
fn process_names(check: &WorktreeCheck) -> String {
    let mut names: Vec<&str> = Vec::new();
    for p in &check.processes {
        let name = if p.name.is_empty() {
            "?"
        } else {
            p.name.as_str()
        };
        if !names.contains(&name) {
            names.push(name);
        }
    }
    let more = names.len().saturating_sub(IN_USE_NAMED);
    let mut text = names[..names.len().min(IN_USE_NAMED)].join(", ");
    if more > 0 {
        text.push_str(&format!(" +{more}"));
    }
    text
}

/// What git alone says of the checkout.
fn git_verdict(check: &WorktreeCheck) -> Option<Verdict> {
    let landing = check.landing.as_ref()?;
    // A git at work: the checkout is in flux, and that is what there is to
    // say — not an error, and not a warning. One stopped part-way (a
    // rebase on a conflict) says so too, but never hides what a delete
    // would destroy: that leads, in the warning color, below.
    let paused = match &landing.git_op {
        Some(op) if landing.git_op_live || check.git_error.is_some() => {
            let state = if landing.git_op_live {
                "running"
            } else {
                "in progress"
            };
            return Some(Verdict {
                text: format!("⋯ git {op} {state}"),
                tone: Tone::Clean,
            });
        }
        Some(op) => Some(format!("git {op} in progress")),
        None => None,
    };
    if check.git_error.is_some() {
        let text = match &landing.failed_submodule {
            Some(path) => format!("? can't read submodule {path} · retrying"),
            None => "? can't read git state · retrying".into(),
        };
        return Some(Verdict {
            text,
            tone: Tone::Clean,
        });
    }
    let mut lost = Vec::new();
    let modified = check.changes.saturating_sub(landing.untracked);
    if modified > 0 {
        lost.push(format!("{modified} modified"));
    }
    if landing.untracked > 0 {
        lost.push(format!("{} untracked", landing.untracked));
    }
    if landing.submodule_unpushed > 0 {
        lost.push(format!(
            "{} unpushed in submodules",
            landing.submodule_unpushed
        ));
    }
    if check.detached && check.ahead > 0 {
        lost.push(format!("detached · {} unreachable", check.ahead));
    }
    let unmerged =
        (!check.detached && check.ahead > 0).then(|| format!("↑{} unmerged", check.ahead));
    if !lost.is_empty() {
        let mut text = format!("⚠ {}", lost.join(" · "));
        // Mid-operation the branch's count means little; the operation
        // is what to know beside what would be lost.
        if let Some(paused) = &paused {
            text.push_str(" · ");
            text.push_str(paused);
        } else if let Some(unmerged) = unmerged {
            text.push_str(" · ");
            text.push_str(&unmerged);
        }
        return Some(Verdict {
            text,
            tone: Tone::Warn,
        });
    }
    if let Some(paused) = paused {
        return Some(Verdict {
            text: format!("⋯ {paused}"),
            tone: Tone::Clean,
        });
    }
    if let Some(text) = unmerged {
        return Some(Verdict {
            text,
            tone: Tone::Ahead,
        });
    }
    let text = if check.detached {
        "✓ clean · detached"
    } else if landing.branch_landed {
        "✓ clean · merged"
    } else {
        return None;
    };
    Some(Verdict {
        text: text.into(),
        tone: Tone::Clean,
    })
}

/// The verdict for `id`'s band, once one has been read.
pub(crate) fn band_verdict(app: &App, id: &WorktreeId) -> Option<Verdict> {
    let (check, _) = app.jobs.band_checks.entries.get(id)?;
    verdict(check)
}

#[cfg(test)]
mod tests {
    use super::*;
    use nebula_core::WorktreeLanding;

    fn with(landing: WorktreeLanding, check: WorktreeCheck) -> WorktreeCheck {
        WorktreeCheck {
            landing: Some(landing),
            ..check
        }
    }

    fn text(check: &WorktreeCheck) -> Option<(String, Tone)> {
        verdict(check).map(|v| (v.text, v.tone))
    }

    #[test]
    fn each_state_reads_in_gits_own_words() {
        let landed = WorktreeLanding {
            branch_landed: true,
            ..WorktreeLanding::default()
        };
        assert_eq!(
            text(&with(landed.clone(), WorktreeCheck::default())),
            Some(("✓ clean · merged".into(), Tone::Clean))
        );
        let detached = WorktreeCheck {
            detached: true,
            ..WorktreeCheck::default()
        };
        assert_eq!(
            text(&with(WorktreeLanding::default(), detached.clone())),
            Some(("✓ clean · detached".into(), Tone::Clean))
        );
        let ahead = WorktreeCheck {
            ahead: 4,
            base: Some("origin/main".into()),
            ..WorktreeCheck::default()
        };
        assert_eq!(
            text(&with(WorktreeLanding::default(), ahead)),
            Some(("↑4 unmerged".into(), Tone::Ahead))
        );
        let dirty = WorktreeCheck {
            changes: 5,
            ..WorktreeCheck::default()
        };
        let untracked = WorktreeLanding {
            untracked: 2,
            ..WorktreeLanding::default()
        };
        assert_eq!(
            text(&with(untracked, dirty)),
            Some(("⚠ 3 modified · 2 untracked".into(), Tone::Warn))
        );
        let submodules = WorktreeLanding {
            submodule_unpushed: 6,
            branch_landed: true,
            ..WorktreeLanding::default()
        };
        assert_eq!(
            text(&with(submodules, WorktreeCheck::default())),
            Some(("⚠ 6 unpushed in submodules".into(), Tone::Warn))
        );
        let loose = WorktreeCheck {
            ahead: 2,
            ..detached
        };
        assert_eq!(
            text(&with(WorktreeLanding::default(), loose)),
            Some(("⚠ detached · 2 unreachable".into(), Tone::Warn))
        );
    }

    #[test]
    fn what_a_delete_destroys_leads_and_the_branchs_commits_follow() {
        let check = WorktreeCheck {
            changes: 3,
            ahead: 4,
            ..WorktreeCheck::default()
        };
        assert_eq!(
            text(&with(WorktreeLanding::default(), check)),
            Some(("⚠ 3 modified · ↑4 unmerged".into(), Tone::Warn))
        );
    }

    #[test]
    fn an_old_daemon_a_failed_check_or_the_bases_own_branch_say_no_more() {
        assert_eq!(text(&WorktreeCheck::default()), None, "no landing");
        assert_eq!(
            text(&with(WorktreeLanding::default(), WorktreeCheck::default())),
            None,
            "not landed, nothing ahead: the base's own branch"
        );
        let failed = WorktreeCheck {
            git_error: Some("boom".into()),
            ..WorktreeCheck::default()
        };
        let (words, tone) = text(&with(WorktreeLanding::default(), failed)).unwrap();
        assert!(!words.starts_with('✓'), "{words}");
        assert_eq!(tone, Tone::Clean);
    }

    #[test]
    fn a_git_at_work_or_a_failure_says_what_and_never_warns() {
        let live = WorktreeLanding {
            git_op: Some("submodule update".into()),
            git_op_live: true,
            ..WorktreeLanding::default()
        };
        assert_eq!(
            text(&with(live, WorktreeCheck::default())),
            Some(("⋯ git submodule update running".into(), Tone::Clean))
        );
        let paused = WorktreeLanding {
            git_op: Some("rebase".into()),
            ..WorktreeLanding::default()
        };
        let dirty = WorktreeCheck {
            changes: 2,
            ..WorktreeCheck::default()
        };
        assert_eq!(
            text(&with(paused.clone(), WorktreeCheck::default())),
            Some(("⋯ git rebase in progress".into(), Tone::Clean))
        );
        assert_eq!(
            text(&with(paused, dirty)),
            Some(("⚠ 2 modified · git rebase in progress".into(), Tone::Warn)),
            "a paused rebase never hides what a delete destroys"
        );
        let failed = WorktreeCheck {
            git_error: Some("boom".into()),
            ..WorktreeCheck::default()
        };
        let submodule = WorktreeLanding {
            failed_submodule: Some("lib/a".into()),
            ..WorktreeLanding::default()
        };
        assert_eq!(
            text(&with(submodule, failed.clone())),
            Some((
                "? can't read submodule lib/a · retrying".into(),
                Tone::Clean
            ))
        );
        assert_eq!(
            text(&with(WorktreeLanding::default(), failed)),
            Some(("? can't read git state · retrying".into(), Tone::Clean))
        );
    }

    fn working(names: &[&str]) -> Vec<nebula_core::WorktreeProcess> {
        names
            .iter()
            .enumerate()
            .map(|(i, name)| nebula_core::WorktreeProcess {
                pid: 100 + i as u32,
                name: (*name).into(),
            })
            .collect()
    }

    #[test]
    fn a_process_outside_nebula_says_the_checkout_is_in_use() {
        let in_use = WorktreeCheck {
            detached: true,
            processes: working(&["just"]),
            ..WorktreeCheck::default()
        };
        assert_eq!(
            text(&with(WorktreeLanding::default(), in_use)),
            Some(("● in use by just · clean · detached".into(), Tone::InUse)),
            "a clean checkout something is working in reads in use, not ✓"
        );
        let unlanded = WorktreeCheck {
            processes: working(&["zsh"]),
            ..WorktreeCheck::default()
        };
        assert_eq!(
            text(&with(WorktreeLanding::default(), unlanded)),
            Some(("● in use by zsh".into(), Tone::InUse)),
            "in use even when git has nothing to say"
        );
        let dirty = WorktreeCheck {
            changes: 3,
            processes: working(&["cargo", "rustc", "rustc", "cc", "ld"]),
            ..WorktreeCheck::default()
        };
        assert_eq!(
            text(&with(WorktreeLanding::default(), dirty)),
            Some((
                "⚠ 3 modified · in use by cargo, rustc +2".into(),
                Tone::Warn
            )),
            "what a delete destroys leads; names are distinct"
        );
        let live = WorktreeLanding {
            git_op: Some("submodule update".into()),
            git_op_live: true,
            ..WorktreeLanding::default()
        };
        let cloning = WorktreeCheck {
            processes: working(&["git"]),
            ..WorktreeCheck::default()
        };
        assert_eq!(
            text(&with(live, cloning)),
            Some(("⋯ git submodule update running".into(), Tone::Clean)),
            "a git at work names itself"
        );
        let no_landing = WorktreeCheck {
            processes: working(&["just"]),
            ..WorktreeCheck::default()
        };
        assert_eq!(
            text(&no_landing),
            None,
            "an older daemon's check says nothing"
        );
    }
}
