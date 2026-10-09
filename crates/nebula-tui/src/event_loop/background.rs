//! Extracted event-loop helper section.

use super::*;

pub(crate) fn request_git_changes(
    app: &mut App,
    git_tx: &tokio::sync::mpsc::UnboundedSender<ChangedFiles>,
) {
    if app.jobs.git_changes_inflight.is_some() {
        return;
    }
    let Some((id, path)) = app
        .selected_worktree()
        .map(|w| (w.id.clone(), w.path.clone()))
    else {
        return;
    };
    app.jobs.git_changes_inflight = Some(id.clone());
    let git_tx = git_tx.clone();
    tokio::task::spawn_blocking(move || {
        let files = crate::git_diff::changed_files(&path).ok();
        let lines = line_changes_of(&path, files.as_deref());
        let _ = git_tx.send((id, files, lines));
    });
}

/// What the badge's `git status` found in a checkout, and the lines behind
/// it; None when git could not say.
pub(crate) type ChangedFiles = (
    WorktreeId,
    Option<Vec<crate::git_diff::DiffFile>>,
    Option<crate::git_diff::LineChanges>,
);

/// The sweep's answer for one checkout: its changed-file count and its
/// line counts.
pub(crate) type SweptChanges = (
    WorktreeId,
    Option<usize>,
    Option<crate::git_diff::LineChanges>,
);

/// The line counts behind `files` when the `git status` read them; a
/// second git process, so only then.
pub(crate) fn line_changes_of(
    path: &std::path::Path,
    files: Option<&[crate::git_diff::DiffFile]>,
) -> Option<crate::git_diff::LineChanges> {
    files.and_then(|files| crate::git_diff::line_changes(path, files))
}

/// Record the line counts a read found in `worktree`, for its cards; None
/// (unreadable) or nothing changed by the line drops what was there. A
/// change redraws.
pub(crate) fn note_worktree_lines(
    app: &mut App,
    worktree: &WorktreeId,
    lines: Option<crate::git_diff::LineChanges>,
) {
    let lines = lines.filter(|l| !l.is_empty());
    let before = match lines {
        Some(l) => app.jobs.worktree_lines.insert(worktree.clone(), l),
        None => app.jobs.worktree_lines.remove(worktree),
    };
    if before != lines {
        app.chrome.dirty = true;
    }
}

/// The most changed files worth holding on to between polls for `g` to open
/// on: a couple of hundred kilobytes of paths at the outside. A checkout
/// with more than this changed waits for its own `git status` instead.
const CHANGED_FILES_KEEP: usize = 2000;

/// Keep the list the badge's count was taken from. The poll has already
/// paid for it, every two seconds, for the checkout the user is in — which
/// is the checkout `g` opens the DIFF VIEWER on, and the `git status` that
/// `g` would otherwise wait for before it can show a single file
/// (`open_diff_view`).
pub(crate) fn keep_changed_files(
    app: &mut App,
    worktree: &WorktreeId,
    files: Option<Vec<crate::git_diff::DiffFile>>,
) {
    app.jobs.changed_files = files
        .filter(|files| files.len() <= CHANGED_FILES_KEEP)
        .map(|files| (worktree.clone(), files));
}

/// Record a checkout's changed-file count. Stored whichever checkout it is
/// for — `App::selected_worktree_changes` shows it only while that one is
/// selected — and a value change redraws.
pub(crate) fn land_git_changes(app: &mut App, worktree: WorktreeId, count: Option<usize>) {
    app.jobs.git_changes_inflight = None;
    note_worktree_changes(app, worktree.clone(), count);
    let next = Some((worktree, count));
    if app.jobs.git_changes != next {
        app.jobs.git_changes = next;
        app.chrome.dirty = true;
    }
}

/// Record what a `git status` found in `worktree` for the cards that print
/// it, stamped now so the sweep knows how old it is; a changed count
/// redraws.
pub(crate) fn note_worktree_changes(app: &mut App, worktree: WorktreeId, count: Option<usize>) {
    let now = std::time::Instant::now();
    let before = app.jobs.worktree_changes.insert(worktree, (count, now));
    if before.map(|(was, _)| was) != Some(count) {
        app.chrome.dirty = true;
    }
}

/// Read the changed-file count of one more checkout the LAUNCHER VIEW's
/// grid has a card for, off the loop, so every card can print its own and
/// not only the one under the cursor (`request_git_changes` keeps that one
/// fresh on every tick). One process per tick at most, and none while the
/// last is still out; the answer lands in `land_swept_changes`.
pub(crate) fn sweep_git_changes(
    app: &mut App,
    tx: &tokio::sync::mpsc::UnboundedSender<SweptChanges>,
) {
    if app.jobs.worktree_changes_inflight.is_some() {
        return;
    }
    let Some((id, path)) = changes_sweep_target(app) else {
        return;
    };
    app.jobs.worktree_changes_inflight = Some(id.clone());
    let tx = tx.clone();
    tokio::task::spawn_blocking(move || {
        let files = crate::git_diff::changed_files(&path).ok();
        let lines = line_changes_of(&path, files.as_deref());
        let _ = tx.send((id, files.map(|f| f.len()), lines));
    });
}

/// The checkout the cards' sweep spends this tick on: of the unselected
/// checkouts the grid has a card for, the one read least recently — never
/// read at all first, so a card that just appeared gets its count on the
/// next tick. Nothing off the grid: the collapsed view draws no cards.
pub(crate) fn changes_sweep_target(app: &App) -> Option<(WorktreeId, std::path::PathBuf)> {
    if !app.launcher_grid() {
        return None;
    }
    let selected = app.selected_worktree().map(|w| &w.id);
    let held: std::collections::HashSet<WorktreeId> = crate::launcher::rows(app)
        .into_iter()
        .map(|row| row.agent.worktree_id)
        .collect();
    app.tree
        .worktrees
        .iter()
        .filter(|w| held.contains(&w.id) && Some(&w.id) != selected)
        .min_by_key(|w| app.jobs.worktree_changes.get(&w.id).map(|(_, at)| *at))
        .map(|w| (w.id.clone(), w.path.clone()))
}

/// Land the sweep's count: it frees the slot and feeds the cards only —
/// the selected checkout's `git_changes` is its own reads' to set.
pub(crate) fn land_swept_changes(app: &mut App, worktree: WorktreeId, count: Option<usize>) {
    app.jobs.worktree_changes_inflight = None;
    note_worktree_changes(app, worktree, count);
}

/// `request_git_changes` and `land_git_changes` in one synchronous step,
/// for tests of the badge.
#[cfg(test)]
pub(crate) fn refresh_git_changes(app: &mut App) {
    let Some((id, path)) = app
        .selected_worktree()
        .map(|w| (w.id.clone(), w.path.clone()))
    else {
        return;
    };
    let count = crate::git_diff::changed_files(&path).ok().map(|f| f.len());
    land_git_changes(app, id, count);
}

/// Ask `gh` for the selected worktree's pull request, off the loop. Skipped
/// while one is in flight — a repaint must never stack `gh` processes — and
/// until the timer the last answer armed expires. The reply arrives on
/// `pr_tx`.
pub(crate) fn lookup_pull_request(
    app: &mut App,
    pr_tx: &tokio::sync::mpsc::UnboundedSender<(WorktreeId, Lookup)>,
) {
    let Some((id, path)) = app
        .selected_worktree()
        .map(|w| (w.id.clone(), w.path.clone()))
    else {
        return;
    };
    if !app.pr_lookup_due(&id) {
        return;
    }
    // A checkout that isn't on disk (deleted outside nebula) has no branch
    // for gh to resolve; don't spend a process finding that out — but let
    // the backoff run, since a worktree can be restored underneath us.
    if !path.is_dir() {
        note_pr_answer(app, &id, false);
        app.chrome.dirty |= app.github.pull_requests.insert(id, None) != Some(None);
        return;
    }
    app.github.pr_inflight.insert(id.clone());
    let pr_tx = pr_tx.clone();
    tokio::spawn(async move {
        let pr = crate::pull_request::lookup(&path).await;
        let _ = pr_tx.send((id, pr));
    });
}

/// Ask `gh` about one of the selected project's *other* checkouts, off the
/// loop — the sweep that turns a worktree purple without the cursor ever
/// visiting it. One process per tick at most, whatever the number of
/// worktrees: the first unselected checkout in row order whose timer has
/// expired gets this tick, the next one waits for the next tick, so a
/// project of thirty checkouts fills in over a minute at startup and then
/// idles on `PR_SWEEP_REFRESH`. The reply lands on `pr_tx` like any other.
pub(crate) fn sweep_pull_request(
    app: &mut App,
    pr_tx: &tokio::sync::mpsc::UnboundedSender<(WorktreeId, Lookup)>,
) {
    let Some((id, path)) = sweep_target(app) else {
        return;
    };
    app.github.pr_inflight.insert(id.clone());
    let pr_tx = pr_tx.clone();
    tokio::spawn(async move {
        let pr = crate::pull_request::lookup(&path).await;
        let _ = pr_tx.send((id, pr));
    });
}

/// The checkout the sweep should spend this tick on: the first of the
/// selected project's unselected, non-root worktrees, in row order, that
/// is due and on disk. The ROOT WORKTREE is skipped — nobody deletes it
/// over a merged pull request, and its own turn comes when it is selected.
/// A checkout that isn't on disk (deleted outside nebula) is noted as a
/// miss on the way past, without spending a process or the tick, since
/// its branch has nothing for `gh` to resolve; the backoff still runs, as
/// a worktree can be restored underneath us.
pub(crate) fn sweep_target(app: &mut App) -> Option<(WorktreeId, std::path::PathBuf)> {
    let selected = app.selected_worktree().map(|w| w.id.clone());
    // The LAUNCHER VIEW names a pull request under every session it lists,
    // so the sweep there covers every checkout of the SESSIONS level's
    // list — the selected project's, the root aside — rather than only
    // the one under the worktree cursor. Walk the level into another
    // project and the sweep goes with it.
    let listed: Vec<&nebula_core::Worktree> = if app.launcher_active() {
        let held: std::collections::HashSet<WorktreeId> = crate::launcher::rows(app)
            .into_iter()
            .map(|row| row.agent.worktree_id)
            .collect();
        app.tree
            .worktrees
            .iter()
            .filter(|w| held.contains(&w.id))
            .collect()
    } else {
        app.visible_worktrees()
    };
    let candidates: Vec<(WorktreeId, std::path::PathBuf)> = listed
        .iter()
        .filter(|w| !w.is_main && Some(&w.id) != selected.as_ref())
        .map(|w| (w.id.clone(), w.path.clone()))
        .collect();
    for (id, path) in candidates {
        if !app.pr_lookup_due(&id) {
            continue;
        }
        if !path.is_dir() {
            note_pr_answer(app, &id, false);
            app.chrome.dirty |= app.github.pull_requests.insert(id, None) != Some(None);
            continue;
        }
        return Some((id, path));
    }
    None
}

/// Record what a lookup came back with, and arm the next one. A found PR
/// settles onto a steady beat — it keeps being asked because its comment
/// count and its state have to keep up with GitHub — `PR_REFRESH` for the
/// checkout the cursor is resting on, the slower `PR_SWEEP_REFRESH` for
/// any other, since a row nobody is reading only has to keep up with its
/// merge. An empty answer arms the next attempt one backoff step further
/// out, so a checkout that never grows a PR settles at `PR_RECHECK_MAX`
/// instead of asking every few seconds forever.
pub(crate) fn note_pr_answer(app: &mut App, worktree: &WorktreeId, found: bool) {
    let selected = app.selected_worktree().is_some_and(|w| &w.id == worktree);
    let step = if found && selected {
        PR_REFRESH
    } else if found {
        PR_SWEEP_REFRESH
    } else {
        match app.github.pr_recheck.get(worktree) {
            Some((_, prev)) => (*prev * 2).min(PR_RECHECK_MAX),
            None => PR_RECHECK_MIN,
        }
    };
    app.github
        .pr_recheck
        .insert(worktree.clone(), (std::time::Instant::now() + step, step));
}

/// A branch lookup landed. The row takes a found pull request, or clears on
/// a definite "no pull request"; a call that never reached GitHub leaves it
/// as it was — this run's last answer, or the one the cache hydrated — and
/// only backs off the next attempt (`note_pr_answer`). A row that changed
/// goes to the cache at the next flush.
pub(crate) fn land_pull_request(app: &mut App, worktree: WorktreeId, answer: Lookup) {
    app.github.pr_inflight.remove(&worktree);
    let row = match answer {
        Lookup::Found(pr) => Some(Some(pr)),
        Lookup::Absent => Some(None),
        Lookup::Unavailable => None,
    };
    note_pr_answer(app, &worktree, matches!(row, Some(Some(_))));
    let Some(row) = row else {
        return;
    };
    let changed = app.github.pull_requests.get(&worktree) != Some(&row);
    // A merge seen to happen — the last answer was anything but merged, this
    // one is — starts the row's ONE-SHOT SWEEP. No last answer at all is a
    // checkout met for the first time: whenever that merged, it wasn't now.
    let is_merged = |pr: &Option<crate::pull_request::PullRequest>| {
        pr.as_ref()
            .is_some_and(|pr| pr.standing() == crate::pull_request::Standing::Merged)
    };
    let landed = is_merged(&row)
        && app
            .github
            .pull_requests
            .get(&worktree)
            .is_some_and(|last| !is_merged(last));
    if landed {
        app.note_merge_landed(worktree.clone());
    }
    app.github.pull_requests.insert(worktree, row);
    app.chrome.dirty |= changed;
    app.github.pr_cache_dirty |= changed;
}

pub(crate) fn maybe_auto_cleanup_merged(
    app: &mut App,
    worktree_id: &WorktreeId,
    out: &mut Vec<ClientRequest>,
) {
    if !app.launcher.auto_cleanup_merged || app.github.auto_cleanup_requested.contains(worktree_id)
    {
        return;
    }
    let Some(worktree) = app
        .tree
        .worktrees
        .iter()
        .find(|worktree| &worktree.id == worktree_id)
        .cloned()
    else {
        return;
    };
    if worktree.is_main {
        return;
    }
    let Some(pr) = app
        .github
        .pull_requests
        .get(worktree_id)
        .and_then(Option::as_ref)
        .filter(|pr| pr.standing() == crate::pull_request::Standing::Merged)
        .cloned()
    else {
        return;
    };
    if pr.head != worktree.branch || pr.head_sha.is_empty() {
        return;
    }
    if app
        .github
        .open_prs
        .values()
        .flat_map(|open| &open.list)
        .any(|open| open.base == worktree.branch)
    {
        return;
    }
    app.github
        .auto_cleanup_requested
        .insert(worktree.id.clone());
    let branch = worktree.branch.clone();
    let pr_number = pr.number;
    let pr_url = pr.url.clone();
    let head_sha = pr.head_sha.clone();
    tracing::info!(
        branch,
        pr_number,
        pr_url = %pr_url,
        "requesting merged worktree cleanup"
    );
    send(app, out, |req_id| ClientRequest::CleanupMergedWorktree {
        req_id,
        id: worktree.id,
        branch,
        pr_number,
        pr_url,
        head_sha,
    });
}

/// Ask `gh` for every pull request open on the selected project's repo, off
/// the loop. Only the selected project is ever asked — the group only shows
/// for the project on screen, and a machine with thirty repos must not
/// cost thirty API calls a beat. Skipped while one is in flight and until the
/// timer the last answer armed. The reply arrives on `prs_tx`.
pub(crate) fn lookup_open_prs(
    app: &mut App,
    prs_tx: &tokio::sync::mpsc::UnboundedSender<(
        nebula_core::ProjectId,
        Option<Vec<crate::pull_request::OpenPr>>,
    )>,
    out: &mut Vec<ClientRequest>,
) {
    let Some((id, path)) = app
        .selected_project()
        .map(|p| (p.id.clone(), p.repo_path.clone()))
    else {
        return;
    };
    if !app.open_prs_lookup_due(&id) {
        return;
    }
    // A repo that isn't on disk has nothing for `gh` to resolve against;
    // don't spend a process finding that out, but let the backoff run — the
    // checkout can come back (an unmounted volume, a restored directory).
    if !path.is_dir() {
        note_open_prs_answer(app, id, None, out);
        return;
    }
    app.github.open_prs_inflight.insert(id.clone());
    let prs_tx = prs_tx.clone();
    tokio::spawn(async move {
        let list = crate::pull_request::list(&path).await;
        let _ = prs_tx.send((id, list));
    });
}

/// Ask `gh` for the open list of one project the cursor is *not* on, off
/// the loop — the background pass that keeps every project's group, and
/// the cache the next launch paints from, warm without the user visiting
/// it. One process per tick at most: the first project in row order whose
/// list is missing, or older than `OPEN_PRS_SWEEP_REFRESH` (or its own
/// backoff, when that is longer), gets this tick. The reply lands on
/// `prs_tx` like the selected project's own, which stays `lookup_open_prs`'s
/// on its faster beat.
pub(crate) fn sweep_open_prs(
    app: &mut App,
    prs_tx: &tokio::sync::mpsc::UnboundedSender<(
        nebula_core::ProjectId,
        Option<Vec<crate::pull_request::OpenPr>>,
    )>,
    out: &mut Vec<ClientRequest>,
) {
    let Some((id, path)) = open_prs_sweep_target(app) else {
        return;
    };
    // Not on disk: noted as a miss without spending a process, and left to
    // the backoff — the checkout can come back.
    if !path.is_dir() {
        note_open_prs_answer(app, id, None, out);
        return;
    }
    app.github.open_prs_inflight.insert(id.clone());
    let prs_tx = prs_tx.clone();
    tokio::spawn(async move {
        let list = crate::pull_request::list(&path).await;
        let _ = prs_tx.send((id, list));
    });
}

/// The project the sweep should spend this tick on, if any: the first
/// project, in row order, that isn't selected, isn't in flight, and was never asked or was last asked longer ago than the
/// sweep's beat allows.
pub(crate) fn open_prs_sweep_target(app: &App) -> Option<(ProjectId, std::path::PathBuf)> {
    let selected = app.selected_project().map(|p| p.id.clone());
    let now = std::time::Instant::now();
    app.project_rows()
        .into_iter()
        .map(|i| &app.tree.projects[i])
        .filter(|p| {
            Some(&p.id) != selected.as_ref() && !app.github.open_prs_inflight.contains(&p.id)
        })
        .find(|p| match app.github.open_prs.get(&p.id) {
            Some(open) => now >= open.at + open.step.max(OPEN_PRS_SWEEP_REFRESH),
            None => true,
        })
        .map(|p| (p.id.clone(), p.repo_path.clone()))
}

/// Record what a list lookup came back with, and arm the next one. A repo
/// with pull requests open settles onto the steady `OPEN_PRS_REFRESH` beat;
/// an empty answer — or one `gh` couldn't give at all — arms the next
/// attempt a backoff step further out, so a repo with no PRs (or a machine
/// with no `gh`) settles at `OPEN_PRS_RECHECK_MAX` instead of asking all
/// day. A failed call keeps whatever list was already on screen: one flaky
/// network round trip is no reason to blank the group. It is noted,
/// though (`App::open_prs_failed`), so a call that keeps failing shows as
/// a list that `couldn't refresh` rather than a current one.
pub(crate) fn note_open_prs_answer(
    app: &mut App,
    project: nebula_core::ProjectId,
    list: Option<Vec<crate::pull_request::OpenPr>>,
    out: &mut Vec<ClientRequest>,
) {
    // Which pull request the cursor is resting on, before the list under it
    // changes. A refresh that retires a merged PR must not slide the
    // selection onto whatever row inherits its index.
    let cursor = app.selected_worktree_pr().cloned();
    // And which checkout: the answer can move one under a pull request
    // just opened from its branch (or back out, once that merges), and
    // the cursor goes with it — a checkout is never lost to a re-list.
    let checkout = app.selected_worktree().map(|w| w.id.clone());
    app.github.open_prs_inflight.remove(&project);
    let failed = list.is_none();
    if failed != app.github.open_prs_failed.contains(&project) {
        app.chrome.dirty = true;
        if failed {
            app.github.open_prs_failed.insert(project.clone());
        } else {
            app.github.open_prs_failed.remove(&project);
        }
    }
    let previous = app.github.open_prs.get(&project);
    let found = list.as_ref().is_some_and(|l| !l.is_empty());
    let step = if found {
        OPEN_PRS_REFRESH
    } else {
        match previous {
            Some(open) => (open.step * 2).min(OPEN_PRS_RECHECK_MAX),
            None => OPEN_PRS_RECHECK_MIN,
        }
    };
    let now = std::time::Instant::now();
    // The pull requests that were open at the last answer and are not in
    // this one: each has merged or closed since, and if a checkout of this
    // project is on one of those branches its row is about to change
    // colour. Only a real answer says so — a failed call keeps the old list
    // and retires nothing.
    let left: Vec<String> = match (previous, &list) {
        (Some(open), Some(fresh)) => open
            .list
            .iter()
            .filter(|was| !fresh.iter().any(|pr| pr.url == was.url))
            .map(|was| was.url.clone())
            .collect(),
        _ => Vec::new(),
    };
    let mut list = list.unwrap_or_else(|| previous.map(|o| o.list.clone()).unwrap_or_default());
    // Drafts sink to the bottom of the group here, on the one path every
    // answer lands through; the cursor reconcile below follows its PR by
    // URL, so the reorder never moves the selection off it.
    crate::pull_request::drafts_last(&mut list);
    let changed = previous.map(|o| &o.list) != Some(&list);
    app.chrome.dirty |= changed;
    app.github.pr_cache_dirty |= changed;
    reask_checkouts_whose_pr_left(app, &left);
    app.github.open_prs.insert(
        project,
        crate::app::OpenPrs {
            list,
            at: now,
            due: now + step,
            step,
        },
    );
    forget_retired_prs(app);
    reconcile_open_pr_cursor(app, cursor, out);
    follow_checkout(app, checkout.as_ref());
    // An open `/` palette lists these rows too: a pull request marked
    // ready for review (or turned back into a draft) since the last answer
    // must change its word there as well, and one merged since must leave.
    if changed {
        refresh_palette(app);
    }
    // So does the PULL REQUESTS MODAL, whose cursor follows its pull
    // request across the new list.
    crate::pr_modal::list_changed(app);
}

/// Pull the lookup of every checkout whose *open* pull request is among
/// `left` — the URLs that just dropped out of the project's open list —
/// forward to the next tick, past whatever `PR_SWEEP_REFRESH` timer the
/// sweep had armed. The list is the fast signal that a branch has merged
/// (it runs every `OPEN_PRS_REFRESH`, and a merged PR stops coming back);
/// the checkout's own `gh pr view` is what says *merged* rather than
/// *closed* and repaints the row purple, so the one is made to follow the
/// other within seconds instead of minutes. A checkout whose PR is already
/// known merged or closed has nothing left to learn and is left on its beat.
pub(crate) fn reask_checkouts_whose_pr_left(app: &mut App, left: &[String]) {
    if left.is_empty() {
        return;
    }
    let due: Vec<WorktreeId> = app
        .github
        .pull_requests
        .iter()
        .filter_map(|(wt, pr)| {
            let pr = pr.as_ref()?;
            (pr.is_open() && left.contains(&pr.url)).then(|| wt.clone())
        })
        .collect();
    for wt in due {
        app.github.pr_recheck.remove(&wt);
    }
}

/// Follow the Worktrees cursor across a change to the open-pull-request
/// list. Three cases, and the row count moved under all of them:
///
/// * the cursor wasn't on a pull request — nothing to do;
/// * its pull request is still open — keep the cursor on *it*, wherever the
///   new list put it, rather than on whatever now holds its old index;
/// * its pull request has been merged or closed — the row is gone, so the
///   cursor lands on the nearest surviving one, and if that is a checkout
///   the pane has to be given that checkout's session (the same
///   `restore_session` an arrow key would have run). Without it the PTY
///   underneath — deliberately left attached while the cursor is in the PR
///   group — would keep showing a session that belongs to a different
///   worktree.
pub(crate) fn reconcile_open_pr_cursor(
    app: &mut App,
    was: Option<crate::pull_request::OpenPr>,
    out: &mut Vec<ClientRequest>,
) {
    let Some(was) = was else {
        return;
    };
    match app.open_pr_row_of(&was.url) {
        // Same pull request, possibly at a new index. Nothing is re-armed:
        // `schedule_pr_detail` zeroes `pr_preview_scroll`, and a refresh
        // landing every minute must not yank a reader back to the top of a
        // conversation they're halfway down.
        Some(i) => app.nav.sel_worktree = i,
        None => {
            let rows = app.worktree_row_count();
            if app.nav.sel_worktree >= rows {
                app.nav.sel_worktree = rows.saturating_sub(1);
            }
            if app.selected_worktree().is_some() {
                restore_session(app, out);
            }
            // The pane is showing something else now: rewind it and fetch
            // whatever the cursor landed on. Say why, too — a row that
            // evaporates mid-read is otherwise just the cursor jumping.
            schedule_pr_detail(app);
            app.chrome.flash = Some(format!("#{} is no longer open", was.number));
            app.chrome.dirty = true;
        }
    }
}

/// Forget the description and conversation of every pull request that is no
/// longer on any row. `pr_detail` is a session cache — deliberately, a PR's
/// body doesn't change while you read it — so without this an instance left
/// running for a week accumulates the full text of every pull request that
/// has since been merged, and `pr_detail_failed` keeps refusing to re-ask
/// about numbers that have long stopped being on screen. A checkout's own
/// PR ROW counts as a row whatever its state: a merged pull request stays
/// on it (see `pull_request::PullRequest`), and forgetting its body on every
/// list refresh would re-fetch it every time the pane is read. The diffs
/// on disk are pruned to the same set, at the next flush.
pub(crate) fn forget_retired_prs(app: &mut App) {
    let live = app.live_pr_urls();
    let before = app.github.pr_detail.len();
    app.github.pr_detail.retain(|url, _| live.contains(url));
    app.github.pr_detail_stale.retain(|url| live.contains(url));
    app.github.pr_detail_failed.retain(|url| live.contains(url));
    app.github.pr_cache_dirty |= app.github.pr_detail.len() != before;
}

/// Carry the state GitHub just gave for one pull request over to the
/// checkout row that shows the same one, ahead of that row's own
/// `PR_REFRESH` beat. The PR ROW keeps a merged or closed pull request
/// rather than retiring it, so what changes here is its badge — `ready` to
/// `merged` the moment the pane learns it, not up to fifteen seconds later
/// — and, on the Sessions panel, its look.
pub(crate) fn adopt_pr_state(app: &mut App, detail: &crate::pull_request::PrDetail) {
    use crate::pull_request::Standing;
    // Checkouts whose row turns merged right here: the same seen-to-happen
    // merge `land_pull_request` stamps, learned a beat earlier.
    let mut landed = Vec::new();
    for (worktree, pr) in app.github.pull_requests.iter_mut() {
        let Some(pr) = pr else { continue };
        if pr.url != detail.url {
            continue;
        }
        if pr.state != detail.state || pr.is_draft != detail.is_draft || pr.health != detail.health
        {
            let was_merged = pr.standing() == Standing::Merged;
            pr.state = detail.state.clone();
            pr.is_draft = detail.is_draft;
            // The pane's answer is the newer one: a conflict resolved (or
            // found) since the row's own lookup turns the row on the spot.
            pr.health = detail.health;
            if !was_merged && pr.standing() == Standing::Merged {
                landed.push(worktree.clone());
            }
            app.chrome.dirty = true;
        }
    }
    for worktree in landed {
        app.note_merge_landed(worktree);
    }
}

/// Retire one pull request from every project's list ahead of the next
/// list lookup, because GitHub has just told us — in the detail fetched
/// for the row the cursor is resting on — that it is merged or closed.
/// The list refresh would catch it within the minute anyway; this is for
/// the case where the user is looking straight at it.
pub(crate) fn drop_retired_pr(app: &mut App, url: &str, out: &mut Vec<ClientRequest>) {
    let cursor = app.selected_worktree_pr().cloned();
    let mut removed = false;
    for open in app.github.open_prs.values_mut() {
        let before = open.list.len();
        open.list.retain(|pr| pr.url != url);
        removed |= open.list.len() != before;
    }
    if !removed {
        return;
    }
    reconcile_open_pr_cursor(app, cursor, out);
    refresh_palette(app);
    crate::pr_modal::list_changed(app);
    app.chrome.dirty = true;
}

/// Drop every cached pull-request row and list whose checkout or project is
/// not in the tree — a worktree deleted while nebula was closed, a project
/// removed — and the bodies that hung off them. What the daemon sends is
/// the truth about what exists; the cache only ever said what those rows
/// last showed.
pub(crate) fn prune_pull_requests_to_tree(app: &mut App) {
    let worktrees: std::collections::HashSet<WorktreeId> =
        app.tree.worktrees.iter().map(|w| w.id.clone()).collect();
    let projects: std::collections::HashSet<ProjectId> =
        app.tree.projects.iter().map(|p| p.id.clone()).collect();
    let before = (app.github.pull_requests.len(), app.github.open_prs.len());
    app.github
        .pull_requests
        .retain(|w, _| worktrees.contains(w));
    app.github.pr_recheck.retain(|w, _| worktrees.contains(w));
    app.github.open_prs.retain(|p, _| projects.contains(p));
    app.github.open_prs_failed.retain(|p| projects.contains(p));
    app.github.pr_cache_dirty |=
        before != (app.github.pull_requests.len(), app.github.open_prs.len());
    forget_retired_prs(app);
}

/// Arm (or disarm) the debounced fetch of the pull request the pane is
/// reading (`App::previewed_pr`) — the Worktrees cursor's open-PR row or the
/// Sessions cursor's PR ROW. Called wherever the Worktrees cursor moves, and
/// from `note_preview_change` whenever the previewed URL changes for any
/// other reason. A PR already fetched, already in flight, or already known
/// to be unanswerable arms nothing — the pane has something to show either
/// way, and re-asking would spend an API call on a row the user is only
/// passing through. A body the cache hydrated (`pr_detail_stale`) is the
/// one exception: the pane shows it at once, and the rest that would have
/// fetched a missing body fetches a fresh copy over it. While the PULL
/// REQUESTS MODAL is up the slot is its cursor's (`pr_modal::schedule_detail`):
/// a list refresh re-arming the pane behind it must not take the fetch of
/// the row the modal is reading.
pub(crate) fn schedule_pr_detail(app: &mut App) {
    if matches!(&app.modals.overlay, Some(Overlay::PullRequests(_))) {
        crate::pr_modal::schedule_detail(app);
        return;
    }
    let pending = app.previewed_pr().and_then(|pr| {
        let url = pr.url;
        let fresh =
            app.github.pr_detail.contains_key(&url) && !app.github.pr_detail_stale.contains(&url);
        if fresh
            || app.github.pr_detail_inflight.contains(&url)
            || app.github.pr_detail_failed.contains(&url)
        {
            return None;
        }
        // Either row lives in the selected project's repo; `gh pr view`
        // resolves the number from any checkout of it.
        let dir = app.selected_project().map(|p| p.repo_path.clone())?;
        Some(crate::app::PendingPrDetail {
            url,
            number: pr.number,
            dir,
        })
    });
    // Landing on a different row resets the scroll: the pane is showing
    // something else now.
    app.github.pr_preview_scroll = 0;
    app.github.pending_pr_detail =
        pending.map(|p| (p, std::time::Instant::now() + PR_DETAIL_DEBOUNCE));
}

/// The one place "the pane is reading something else now" is noticed: the
/// loop takes `reading_url()` — the pull request's or the issue's — before
/// handling an event and hands it back here after. A different URL — the
/// Sessions cursor stepped onto or off the PR ROW, focus left the Sessions
/// panel for the pane, a refresh retired the row, the Worktrees cursor
/// landed on an issue — re-arms the detail fetch (the pull request's, and
/// the issue's comments) and rewinds the scroll; the same URL leaves a
/// reader exactly where they were. Keyed on URL, not the whole row, so a
/// re-titled PR arriving on the GIT POLL is not a change.
pub(crate) fn note_preview_change(app: &mut App, before: Option<String>) {
    if app.reading_url() != before {
        schedule_pr_detail(app);
        crate::issues::schedule_detail(app);
    }
}

/// Fire the debounced fetch. Disarms first, so a `gh` that never answers
/// can't re-fire on every loop turn.
pub(crate) fn lookup_pr_detail(
    app: &mut App,
    detail_tx: &tokio::sync::mpsc::UnboundedSender<(String, Option<crate::pull_request::PrDetail>)>,
) {
    let Some((pending, _)) = app.github.pending_pr_detail.take() else {
        return;
    };
    if !pending.dir.is_dir() {
        app.github.pr_detail_failed.insert(pending.url);
        app.chrome.dirty = true;
        return;
    }
    app.github.pr_detail_inflight.insert(pending.url.clone());
    let detail_tx = detail_tx.clone();
    tokio::spawn(async move {
        let detail = crate::pull_request::detail(&pending.dir, pending.number).await;
        let _ = detail_tx.send((pending.url, detail));
    });
}

/// A body and conversation landed. It replaces whatever the pane was
/// showing for the URL — most often the copy the cache hydrated — and
/// GitHub's word on this one pull request is the authoritative one: if it
/// has been merged or closed since the list was fetched, the row goes now
/// rather than at the next refresh (`adopt_pr_state`, `drop_retired_pr`).
/// A fetch that failed leaves a cached copy where it is; stale is still
/// the best answer there is.
pub(crate) fn land_pr_detail(
    app: &mut App,
    url: String,
    detail: Option<crate::pull_request::PrDetail>,
    out: &mut Vec<ClientRequest>,
) {
    app.github.pr_detail_inflight.remove(&url);
    match detail {
        Some(detail) => {
            let retired = !detail.is_open();
            adopt_pr_state(app, &detail);
            let changed = app.github.pr_detail.get(&url) != Some(&detail);
            app.github.pr_detail.insert(url.clone(), detail);
            app.github.pr_detail_stale.remove(&url);
            app.github.pr_cache_dirty |= changed;
            if retired {
                drop_retired_pr(app, &url, out);
            }
        }
        None => {
            app.github.pr_detail_failed.insert(url);
        }
    }
    app.chrome.dirty = true;
}

/// `y` on a pull request row — a PROJECT OPEN PRS GROUP row or the
/// Sessions panel's PR ROW — or **Comment…** from either row's menu: the
/// COMMENT BOX, a multi-row task box titled with the row, whose Enter
/// posts the text on that pull request through `gh pr comment`
/// (`post_pr_comment`). Off a pull request row the key only says what it
/// wants. A draft a refused post handed back while another modal was up
/// (`pr_comment_drafts`) fills the box, so the refusal cost nothing typed.
pub(crate) fn open_pr_comment(app: &mut App) {
    // The pane reading a pull request (a `/` jump lands on one) is the one
    // to comment on; otherwise, on the grid, the card's — the pull request
    // `⇧V` opens.
    let found = match app.previewed_pr() {
        Some(pr) => Ok(pr),
        None if app.launcher_grid() => {
            launcher::card_pull_request(app, launcher::NO_CARD_FOR_COMMENT).map(|pr| {
                crate::app::PreviewedPr {
                    label: crate::pull_request::numbered_label(pr.number, &pr.title),
                    number: pr.number,
                    url: pr.url,
                }
            })
        }
        None => Err("move onto a pull request row to comment on it".into()),
    };
    let pr = match found {
        Ok(pr) => pr,
        Err(why) => {
            app.chrome.flash = Some(why);
            app.chrome.dirty = true;
            return;
        }
    };
    let draft = app
        .github
        .pr_comment_drafts
        .remove(&pr.url)
        .unwrap_or_default();
    reopen_prompt_with(
        app,
        PromptKind::PrComment {
            number: pr.number,
            url: pr.url,
            label: pr.label,
            back: None,
        },
        draft,
    );
}

/// Enter in the COMMENT BOX: post `body` on pull request `number` off the
/// loop, as the `gh` user, from the selected project's checkout — `gh pr
/// comment` resolves the number from any checkout of the repo, as the
/// detail fetch does — and say it is on its way. The outcome lands in
/// `land_pr_comment`. A post that cannot even start — one already running
/// on this pull request, no checkout on disk — says why and hands the box
/// straight back with its text. A box that stood in for the PULL REQUESTS
/// MODAL (`back`) puts the modal back on its row as the post goes out.
pub(crate) fn post_pr_comment(
    app: &mut App,
    number: u64,
    url: String,
    label: String,
    body: String,
    back: Option<Box<crate::pr_modal::PullRequestsView>>,
) {
    let dir = app.selected_project().map(|p| p.repo_path.clone());
    let refused = if app.github.pr_comment_inflight.contains(&url) {
        Some(format!("still posting the last comment on #{number}…"))
    } else {
        match &dir {
            Some(dir) if !dir.is_dir() => {
                Some(format!("repo path missing on disk: {}", dir.display()))
            }
            None => Some("no project selected to post from".into()),
            // Never in the running TUI: the loop installs the channel at
            // startup. Handing the box back beats losing the text.
            Some(_) if app.github.pr_comment_tx.is_none() => Some("not ready to post yet".into()),
            Some(_) => None,
        }
    };
    if let Some(why) = refused {
        app.chrome.flash = Some(why);
        reopen_prompt_with(
            app,
            PromptKind::PrComment {
                number,
                url,
                label,
                back,
            },
            body,
        );
        app.chrome.dirty = true;
        return;
    }
    if let Some(view) = back {
        crate::pr_modal::reopen(app, *view);
    }
    let (Some(dir), Some(tx)) = (dir, app.github.pr_comment_tx.clone()) else {
        return;
    };
    app.github.pr_comment_inflight.insert(url.clone());
    app.chrome.flash = Some(format!("posting a comment on #{number}…"));
    app.chrome.dirty = true;
    tokio::spawn(async move {
        let result = crate::pull_request::comment(&dir, number, &body).await;
        let _ = tx.send(PrCommentAnswer {
            number,
            url,
            label,
            body,
            result,
        });
    });
}

/// A `gh pr comment` landed. Posted: say so, and read the pull request
/// again so the pane's conversation carries the new comment — in place
/// when the pane is still on it (`refetch_pr_detail`, which keeps the
/// reader's scroll), and on the next visit otherwise (`pr_detail_stale`).
/// Refused: say why, in `gh`'s words, and put the box back with the text
/// — at once when nothing else is on screen (or only the PULL REQUESTS
/// MODAL, which the box stands in for again), and otherwise the next `y`
/// on that pull request starts from it, so no refusal costs what was
/// typed.
pub(crate) fn land_pr_comment(app: &mut App, answer: PrCommentAnswer) {
    let PrCommentAnswer {
        number,
        url,
        label,
        body,
        result,
    } = answer;
    app.github.pr_comment_inflight.remove(&url);
    match result {
        Ok(_) => {
            app.chrome.flash = Some(format!("comment posted on #{number}"));
            app.github.pr_detail_stale.insert(url.clone());
            if matches!(&app.modals.overlay, Some(Overlay::PullRequests(_))) {
                // The modal reads its row again, the new comment in.
                crate::pr_modal::schedule_detail(app);
            } else if app.previewed_pr().is_some_and(|pr| pr.url == url) {
                refetch_pr_detail(app);
            }
        }
        Err(why) => {
            app.chrome.flash = Some(format!("couldn't post the comment on #{number}: {why}"));
            let back = match &app.modals.overlay {
                None => Some(None),
                Some(Overlay::PullRequests(view)) => Some(Some(Box::new(view.clone()))),
                Some(_) => None,
            };
            match back {
                Some(back) => reopen_prompt_with(
                    app,
                    PromptKind::PrComment {
                        number,
                        url,
                        label,
                        back,
                    },
                    body,
                ),
                None => {
                    app.github.pr_comment_drafts.insert(url, body);
                }
            }
        }
    }
    app.chrome.dirty = true;
}

/// `g` on an open-PR row: fetch the whole pull request diff off the loop and
/// open the ordinary diff modal on it when it lands. One `gh pr diff` gets
/// every file at once, which is why this view carries its diffs with it
/// instead of shelling out per file the way the worktree view does.
///
/// A diff read before — this run or a previous one (`pr_cache`) — opens
/// the modal at once instead, and the fetch runs underneath it: what lands
/// replaces the modal's contents in place when it differs, and only goes to
/// the cache if the modal has since been closed (`land_pr_diff`).
pub(crate) fn request_pr_diff(app: &mut App) {
    let Some(pr) = app.previewed_pr() else {
        return;
    };
    request_pr_diff_for(app, pr.number, pr.url, pr.label);
}

/// [`request_pr_diff`] for pull request `number` of the selected project,
/// titled `title` — the pane's row, or the PULL REQUESTS MODAL's (`g`
/// there), whose modal the diff's replaces.
pub(crate) fn request_pr_diff_for(app: &mut App, number: u64, url: String, title: String) {
    if app.github.pr_diff_inflight == Some(number) {
        app.chrome.flash = Some(format!("still fetching the diff for #{number}…"));
        return;
    }
    let Some(dir) = app.selected_project().map(|p| p.repo_path.clone()) else {
        return;
    };
    if !dir.is_dir() {
        app.chrome.flash = Some(format!("repo path missing on disk: {}", dir.display()));
        return;
    }
    let Some(prdiff_tx) = app.github.pr_diff_tx.clone() else {
        return; // never: the loop installs it at startup
    };
    if open_cached_pr_diff(app, number, &url, &title) {
        app.github.pr_diff_refreshing.insert(url.clone());
    } else {
        app.chrome.flash = Some(format!("fetching the diff for #{number}…"));
    }
    app.github.pr_diff_inflight = Some(number);
    app.chrome.dirty = true;
    tokio::spawn(async move {
        let diff = crate::pull_request::diff(&dir, number).await;
        let _ = prdiff_tx.send(PrDiffAnswer {
            number,
            url,
            title,
            diff,
        });
    });
}

/// Open the modal on the diff last read for `url`, when the cache kept one
/// with files in it. Whether it did.
pub(crate) fn open_cached_pr_diff(app: &mut App, number: u64, url: &str, title: &str) -> bool {
    let Some(cached) = crate::pr_cache::recall_diff(app, url) else {
        return false;
    };
    open_pr_diff_view(app, number, url, title.to_string(), Some(cached));
    matches!(&app.modals.overlay, Some(Overlay::Diff(view)) if view.pr_url.as_deref() == Some(url))
}

/// A `gh pr diff` landed. It goes to the cache for next time, and then: a
/// modal opened on this pull request's cached copy is refreshed in place
/// (`refresh_pr_diff_view`), the reader keeping their file and their place
/// in it; one closed since stays closed; and a request that had nothing
/// cached opens the modal now, as it always did.
pub(crate) fn land_pr_diff(app: &mut App, answer: PrDiffAnswer) {
    let PrDiffAnswer {
        number,
        url,
        title,
        diff,
    } = answer;
    if let Some(diff) = &diff {
        crate::pr_cache::remember_diff(app, &url, diff);
    }
    if !app.github.pr_diff_refreshing.remove(&url) {
        open_pr_diff_view(app, number, &url, title, diff);
        return;
    }
    if app.github.pr_diff_inflight == Some(number) {
        app.github.pr_diff_inflight = None;
    }
    // A fetch that failed leaves the cached copy on screen: it was the best
    // answer there was when `g` was pressed, and still is.
    let Some(diff) = diff else {
        return;
    };
    let Some(Overlay::Diff(view)) = &mut app.modals.overlay else {
        return;
    };
    if view.pr_url.as_deref() != Some(url.as_str()) {
        return;
    }
    if refresh_pr_diff_view(view, &diff) {
        app.chrome.flash = Some(format!("#{number}'s diff changed since it was last read"));
        app.chrome.dirty = true;
    }
}

/// Swap a fresh diff into an open pull-request modal: the file list and the
/// per-file chunks are rebuilt, the selection follows the file the reader
/// was on by path — keeping their scroll in it — and falls back to the
/// first row when that file is no longer in the diff. Whether anything
/// changed; an identical diff leaves the view untouched.
pub(crate) fn refresh_pr_diff_view(view: &mut DiffView, diff: &str) -> bool {
    let chunks = crate::pull_request::split_unified_diff(diff);
    let fresh: std::collections::HashMap<String, String> = chunks.iter().cloned().collect();
    if view.prefetched.as_ref() == Some(&fresh) {
        return false;
    }
    let was_on = view.selected_path().map(str::to_string);
    let scroll = view.scroll;
    view.prefetched = Some(fresh);
    view.replace_files(pr_diff_files(&chunks));
    let same = was_on.is_some_and(|path| view.select_path(&path));
    crate::git_diff::load_selected_diff(view);
    if same {
        view.scroll = scroll.min(view.max_scroll());
    }
    true
}

/// The file rows of a pull-request diff: one per chunk, in git's order,
/// every entry marked `M` — a pull request's own diff already renders the
/// add/delete headers, and porcelain codes would be an invention.
pub(crate) fn pr_diff_files(chunks: &[(String, String)]) -> Vec<crate::git_diff::DiffFile> {
    chunks
        .iter()
        .map(|(path, _)| crate::git_diff::DiffFile {
            path: path.clone(),
            orig_path: None,
            xy: ['M', ' '],
        })
        .collect()
}

/// Land a fetched pull-request diff in the diff modal. The files come from
/// splitting the unified diff rather than from `git status`
/// (`pr_diff_files`), and the view is tagged with the pull request's URL
/// so a later fetch can tell it is still the one on screen.
pub(crate) fn open_pr_diff_view(
    app: &mut App,
    number: u64,
    url: &str,
    title: String,
    diff: Option<String>,
) {
    if app.github.pr_diff_inflight == Some(number) {
        app.github.pr_diff_inflight = None;
    }
    let Some(diff) = diff else {
        app.chrome.flash = Some(format!(
            "couldn't read the diff for #{number} — is `gh` set up?"
        ));
        return;
    };
    let chunks = crate::pull_request::split_unified_diff(&diff);
    if chunks.is_empty() {
        app.chrome.flash = Some(format!("#{number} changes no files"));
        return;
    }
    let files = pr_diff_files(&chunks);
    // `root` is only ever used to shell out at git, which a prefetched view
    // never does — but the reviewed-mark store keys on it, so it stays the
    // repo path rather than something invented.
    let root = app
        .selected_project()
        .map(|p| p.repo_path.clone())
        .unwrap_or_default();
    let mut view = DiffView::new(root, title, files, true);
    view.prefetched = Some(chunks.into_iter().collect());
    view.pr_url = Some(url.to_string());
    view.files_width = app.modals.diff_files_width;
    view.split = app.modals.diff_split;
    if app.modals.diff_tree {
        view.toggle_tree();
    }
    crate::git_diff::load_selected_diff(&mut view);
    app.modals.overlay = Some(Overlay::Diff(view));
    app.chrome.flash = None;
    app.chrome.dirty = true;
}

/// Arm the selected project for a prompt open-pull-request lookup: arriving
/// at a project is exactly when the user wants to know what's still open on
/// it. Floored at `OPEN_PRS_MIN_AGE` past the last answer, so bouncing
/// between two projects re-reads the cache instead of spending an API call
/// per switch.
pub(crate) fn schedule_open_prs_lookup(app: &mut App) {
    let Some(id) = app.selected_project().map(|p| p.id.clone()) else {
        return;
    };
    if let Some(open) = app.github.open_prs.get_mut(&id) {
        open.due = open.due.min(open.at + crate::app::OPEN_PRS_MIN_AGE);
    }
}

/// Arm the selected worktree for a prompt pull-request lookup: switching
/// into a checkout is exactly when the user wants to see the PR a session
/// opened there — and, once it's known, whether anyone has commented since
/// — so drop whatever timer had accumulated and ask on the next tick.
pub(crate) fn schedule_pr_lookup(app: &mut App) {
    if let Some(id) = app.selected_worktree().map(|w| w.id.clone()) {
        app.github.pr_recheck.remove(&id);
    }
}

/// Arm every checkout of the selected project for a prompt lookup: drop
/// the timers the sweep armed, so its next passes re-ask each row in turn.
pub(crate) fn schedule_pr_sweep(app: &mut App) {
    let ids: Vec<WorktreeId> = app
        .visible_worktrees()
        .iter()
        .map(|w| w.id.clone())
        .collect();
    for id in ids {
        app.github.pr_recheck.remove(&id);
    }
}

/// Pull both pull-request lookups forward — the project's open list and
/// the selected worktree's own PR — so the next `GIT_POLL` tick asks GitHub
/// again. Run on the gestures that mean "I want fresh data now": the
/// terminal window regaining focus and the cursor entering a sidebar panel.
/// The list keeps its `OPEN_PRS_MIN_AGE` floor, so a flurry of focus events
/// costs one call, not one per event. Deliberately does *not* touch the PR
/// preview: `schedule_pr_detail` resets its scroll, and a timer-shaped
/// caller must never yank a reader back to the top.
pub(crate) fn schedule_pull_request_refresh(app: &mut App) {
    schedule_open_prs_lookup(app);
    schedule_pr_lookup(app);
}

/// What `Shift+R` says in the footer the moment it is heard.
pub(crate) const RELOAD_FLASH: &str = "reloading pull requests and issues from GitHub…";

/// `Shift+R`, from any panel: reload from GitHub *now* — the selected
/// project's open list and its open issues (`issues::reload_selected`),
/// every one of its checkouts' own PR, and the body
/// and conversation of the pull request the pane is reading — past
/// every timer and floor the beats keep. `schedule_pull_request_refresh`
/// is what a focus event may do; this is what a deliberate keypress may
/// do, so the list's `OPEN_PRS_MIN_AGE` floor does not apply and the two
/// list lookups fire on the loop's next turn rather than the next git
/// tick. The other checkouts follow at the sweep's one-per-tick pace, so
/// the key spends one process per row over the next seconds, never a
/// burst. The flash is the only immediate feedback: the rows repaint once
/// the answers land, and a machine with no `gh` never repaints at all.
pub(crate) fn refresh_pull_requests(app: &mut App) {
    let Some(project) = app.selected_project().map(|p| p.id.clone()) else {
        return;
    };
    if let Some(open) = app.github.open_prs.get_mut(&project) {
        open.due = std::time::Instant::now();
    }
    schedule_pr_lookup(app);
    schedule_pr_sweep(app);
    refetch_pr_detail(app);
    app.github.pr_refresh_requested = true;
    crate::issues::reload_selected(app);
    app.chrome.flash = Some(RELOAD_FLASH.into());
    app.chrome.dirty = true;
}

/// Fetch the previewed pull request's body and conversation again, over
/// the cached copy. Unlike `schedule_pr_detail` this asks even when an
/// answer is cached or was refused — that is the point — and leaves the
/// scroll alone: the reader asked for this, so they must not be sent back
/// to the top. The cached copy stays on screen until the new one lands;
/// a lookup already in flight is left to land on its own.
pub(crate) fn refetch_pr_detail(app: &mut App) {
    let Some(pr) = app.previewed_pr() else {
        return;
    };
    if app.github.pr_detail_inflight.contains(&pr.url) {
        return;
    }
    let Some(dir) = app.selected_project().map(|p| p.repo_path.clone()) else {
        return;
    };
    app.github.pr_detail_failed.remove(&pr.url);
    app.github.pending_pr_detail = Some((
        crate::app::PendingPrDetail {
            url: pr.url,
            number: pr.number,
            dir,
        },
        std::time::Instant::now(),
    ));
}

/// The panel focus just moved. Landing on the Worktrees or Sessions panel —
/// the two that show pull requests — is a reason to re-ask GitHub; walking
/// off them into the pane is not.
pub(crate) fn note_focus_change(app: &mut App) {
    if matches!(app.nav.focus, Focus::Worktrees | Focus::Sessions) {
        schedule_pull_request_refresh(app);
    }
}

/// Fire one memory reading for the metrics modal: sample this client's own
/// RSS now (the daemon can't see us), ask the daemon for itself plus every
/// session's process tree. The reply arrives as `ServerEvent::Metrics`.
///
/// The client's own reading is a `ps`, and this runs on the footer's beat —
/// every five seconds for as long as the TUI is up — so the `ps` runs off
/// the loop and lands like any other BACKGROUND READ, rather than stall
/// whatever key arrives during it.
pub(crate) fn request_metrics(app: &mut App, out: &mut Vec<ClientRequest>) {
    let own_rss = || nebula_core::mem::process_rss_bytes(std::process::id()).unwrap_or(0);
    match app.jobs.view_jobs.clone() {
        Some(jobs) => jobs.run(move || Some(crate::view_jobs::Answer::ClientRss(own_rss()))),
        None => land_client_rss(app, own_rss()),
    }
    send(app, out, |req_id| ClientRequest::GetMetrics { req_id });
}

/// Ask the daemon what each TERMINAL whose card the last frame drew has
/// printed since its card last heard ([`App::tail_cards`],
/// [`App::terminal_tails`]), for the lines under its name. The one the
/// pane is on is skipped — its screen is right here, live — and so is a
/// dead one: its ring went with its shell, and the card keeps the lines
/// it had. Each ask carries the ring end the card has, so an idle shell
/// answers with no bytes. The frame's notes are read, not taken: a grid
/// nothing repaints keeps asking after the same cards until the next
/// frame says otherwise. The reply is `ServerEvent::OutputTail`.
pub(crate) fn request_terminal_tails(app: &mut App, out: &mut Vec<ClientRequest>) {
    let attached = app.pane.term.as_ref().map(|t| t.sref.clone());
    let mut asked = std::collections::HashSet::new();
    for id in app.pane.tail_cards.clone() {
        if !asked.insert(id.clone()) {
            continue;
        }
        let sref = SessionRef::Terminal(id.clone());
        if attached.as_ref() == Some(&sref) {
            continue;
        }
        if !app.tree.terminals.iter().any(|t| t.id == id && t.alive) {
            continue;
        }
        let after_seq = app.pane.terminal_tails.get(&id).map(|t| t.end_seq);
        send(app, out, |req_id| ClientRequest::TailOutput {
            req_id,
            session: sref,
            max_bytes: TAIL_BYTES,
            after_seq,
        });
    }
}

/// The daemon's answer to [`request_terminal_tails`]: the end of the ring,
/// laid out through a throwaway screen the PTY's size and kept as the
/// card's lines. No bytes means nothing new since the card last heard,
/// and no tail at all that the shell is gone — either way the lines stay.
/// A frame only when they changed.
pub(crate) fn land_terminal_tail(
    app: &mut App,
    id: TerminalId,
    tail: Option<nebula_core::OutputTail>,
) {
    let Some(tail) = tail else { return };
    let entry = app.pane.terminal_tails.entry(id).or_default();
    if tail.data.is_empty() {
        entry.end_seq = tail.end_seq;
        return;
    }
    let lines = crate::terminal_tail::parse_tail(
        &tail.data,
        tail.cols,
        tail.rows,
        crate::launcher::PROMPT_LINES,
    );
    if entry.lines != lines {
        entry.lines = lines;
        app.chrome.dirty = true;
    }
    entry.end_seq = tail.end_seq;
}

/// Open the memory modal — `⇧M`, and a click on the footer's readout.
/// A reading is requested right away: the main loop's poll may be up to
/// FOOTER_METRICS_POLL out.
pub(crate) fn open_metrics(app: &mut App, out: &mut Vec<ClientRequest>) {
    app.modals.overlay = Some(Overlay::Metrics(MetricsView::new()));
    request_metrics(app, out);
}

pub(crate) fn land_client_rss(app: &mut App, bytes: u64) {
    app.jobs.client_rss_bytes = bytes;
    if let Some(Overlay::Metrics(view)) = &mut app.modals.overlay {
        view.client_rss_bytes = bytes;
    }
}
