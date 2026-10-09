//! Extracted event-loop helper section.

use super::*;

pub(crate) fn detach_if_attached(app: &mut App, sref: &SessionRef, out: &mut Vec<ClientRequest>) {
    // The daemon may hold this session even when the pane has already moved
    // on to another one whose attach is still debounced — release it either
    // way, or the connection stays attached to a row that no longer exists.
    let showing = app.pane.term.as_ref().is_some_and(|t| &t.sref == sref);
    if app.pane.attached_sref.as_ref() == Some(sref) || showing {
        out.push(ClientRequest::Detach {
            session: sref.clone(),
        });
        if app.pane.attached_sref.as_ref() == Some(sref) {
            app.pane.attached_sref = None;
        }
    }
    if showing {
        app.pane.pending_attach = None;
        app.pane.term = None;
        // The row went away under the pane — not a key anyone pressed, so
        // it says that the keys are the grid's now.
        app.release_terminal();
        if app.nav.focus == Focus::Terminal {
            app.nav.focus = Focus::Sessions;
        }
    }
    // Archived, deleted or killed: whatever comes back under this ref is
    // a new process, so its kept screen is stale.
    app.pane.term_cache.retain(|t| &t.sref != sref);
}

pub(crate) fn shellexpand_home(path: &str) -> std::path::PathBuf {
    if let Some(rest) = path.strip_prefix("~/") {
        if let Some(home) = nebula_core::env::home_dir() {
            return home.join(rest);
        }
    }
    std::path::PathBuf::from(path)
}

/// Snapshot the context being left — which worktree row the project was
/// on, and which session row that worktree was on — so switching back
/// restores both. Call BEFORE moving the selection away.
pub(crate) fn remember_context(app: &mut App) {
    let Some(wid) = app.selected_worktree().map(|w| w.id.clone()) else {
        return;
    };
    if let Some(pid) = app.selected_project().map(|p| p.id.clone()) {
        app.nav.last_worktree_for_project.insert(pid, wid.clone());
    }
    let row = app.selected_session_row();
    // A link row is not a session. Leaving the worktree with the cursor
    // parked on one must not forget which session it was last on — the way
    // back would land on the top row instead of the one it was reading.
    if row.as_ref().is_some_and(|r| r.as_link().is_some()) {
        return;
    }
    match row.and_then(|r| r.sref()) {
        Some(sref) => {
            app.nav.last_session_for_worktree.insert(wid, sref);
        }
        None => {
            app.nav.last_session_for_worktree.remove(&wid);
        }
    }
}

/// After a project switch: land on the project's remembered worktree (its
/// main checkout otherwise), then re-show that worktree's session.
pub(crate) fn restore_context(app: &mut App, out: &mut Vec<ClientRequest>) {
    restore_project_cursors(app);
    restore_session(app, out);
}

/// Everything [`restore_context`] does except bring a session up: the new
/// project's sweeps rescheduled and its remembered checkout under the
/// Worktrees cursor. The LAUNCHER VIEW's PROJECT SWITCHER takes this half
/// on its own — its grid draws no pane, so attaching the project's last
/// session would boot a PTY nobody is looking at.
pub(crate) fn restore_project_cursors(app: &mut App) {
    app.nav.sel_worktree = 0;
    schedule_open_prs_lookup(app);
    crate::issues::schedule_prefetch(app);
    schedule_pr_detail(app);
    if let Some(pid) = app.selected_project().map(|p| p.id.clone()) {
        if let Some(wid) = app.nav.last_worktree_for_project.get(&pid).cloned() {
            if let Some(i) = app.worktree_row_of(&wid) {
                app.nav.sel_worktree = i;
            }
        }
    }
}

/// After a worktree switch: select and re-attach the worktree's remembered
/// session. With nothing remembered (a first visit), or a remembered row
/// that is gone or archived, land on the top row instead — the most
/// recently interacted session, or the first shell terminal when no agent
/// is live — so a worktree with something to show never comes up blank.
/// Only a worktree with no attachable row at all blanks the pane, rather
/// than keep showing the previous context's session.
pub(crate) fn restore_session(app: &mut App, out: &mut Vec<ClientRequest>) {
    app.nav.sel_session = 0;
    schedule_prewarm(app);
    schedule_pr_lookup(app);
    let rows = app.visible_session_rows();
    let remembered = app
        .selected_worktree()
        .and_then(|w| app.nav.last_session_for_worktree.get(&w.id).cloned());
    let attachable = |r: &SessionRow| r.sref().is_some() && !r.is_archived_agent();
    let target = remembered
        .and_then(|sref| {
            rows.iter()
                .position(|r| r.sref().as_ref() == Some(&sref) && attachable(r))
        })
        .or_else(|| rows.iter().position(attachable))
        .and_then(|i| rows[i].sref().map(|sref| (i, sref)));
    match target {
        Some((index, sref)) => {
            app.nav.sel_session = index;
            // A worktree the NESTED layout has folded is its header under
            // the cursor, not the card it was left on: the row is kept
            // for when the band opens, and nothing off screen is attached
            // — or marked read (`App::on_folded_band`).
            if !app.on_folded_band() {
                attach(app, sref, out);
            }
        }
        None => {
            if app.pane.term.is_some() {
                detach_pane(app, out);
            }
        }
    }
}

/// Land the selection on `select_when_seen`: a session just created, or
/// one moved into another worktree (of another project too, whose grid
/// it then opens). Directly when its row is
/// visible under the selected worktree, else by switching to the worktree it
/// landed under first. Clears the pending follow once it lands; a no-op
/// until the session's upsert has arrived.
pub(crate) fn land_pending_selection(app: &mut App, out: &mut Vec<ClientRequest>) {
    let Some(pending_sref) = app.requests.select_when_seen.clone() else {
        return;
    };
    if let Some(index) = app
        .visible_session_rows()
        .iter()
        .position(|r| r.sref().as_ref() == Some(&pending_sref))
    {
        app.nav.sel_session = index;
        app.requests.select_when_seen = None;
        // A card landed on is a card on screen (the NESTED layout's fold).
        launcher::unfold_cursor_band(app);
        // The pane follows the cursor; a session about to be attached
        // outright (the create flow's Ack) dedupes in attach().
        preview_selected(app, out);
        return;
    }
    let landed_worktree = match &pending_sref {
        SessionRef::Agent(id) => app
            .tree
            .agents
            .iter()
            .find(|a| &a.id == id)
            .map(|a| a.worktree_id.clone()),
        SessionRef::Terminal(id) => app
            .tree
            .terminals
            .iter()
            .find(|t| &t.id == id)
            .map(|t| t.worktree_id.clone()),
    };
    if let Some(wt_id) = landed_worktree {
        // Moved into another project (the MOVE PICKER, a drop on a
        // PROJECT TAB): the grid goes there with it.
        let project = app
            .tree
            .worktrees
            .iter()
            .find(|w| w.id == wt_id)
            .map(|w| w.project_id.clone());
        if let Some(project) = project {
            if app.selected_project().map(|p| &p.id) != Some(&project) {
                select_project_row_by_id(app, &project);
            }
        }
        if select_worktree_by_id(app, &wt_id, out) {
            if let Some(index) = app
                .visible_session_rows()
                .iter()
                .position(|r| r.sref().as_ref() == Some(&pending_sref))
            {
                app.nav.sel_session = index;
                launcher::unfold_cursor_band(app);
                preview_selected(app, out);
            }
            app.requests.select_when_seen = None;
        }
    }
}

/// Select the worktree row for `id` within the selected project; returns
/// false when it isn't in the tree yet (its upsert hasn't arrived).
pub(crate) fn select_worktree_by_id(
    app: &mut App,
    id: &nebula_core::WorktreeId,
    out: &mut Vec<ClientRequest>,
) -> bool {
    let Some(index) = app.worktree_row_of(id) else {
        return false;
    };
    if app.nav.sel_worktree != index {
        remember_context(app);
        app.nav.sel_worktree = index;
        restore_session(app, out);
    }
    // Land on the sessions panel so `n` immediately creates a session here.
    app.nav.focus = Focus::Sessions;
    true
}

/// Land on a project we just added, the way a pick of it from the PROJECT
/// DROPDOWN would (`launcher::open_project`): its tab lit, and — with no
/// session in it yet, which is every project just added — the empty grid,
/// the pane folded away under it rather than left reading the session or
/// terminal it was on in the project before. False when its upsert hasn't
/// arrived yet.
pub(crate) fn select_created_project(
    app: &mut App,
    id: &nebula_core::ProjectId,
    out: &mut Vec<ClientRequest>,
) -> bool {
    if !app
        .project_rows()
        .iter()
        .any(|i| &app.tree.projects[*i].id == id)
    {
        return false;
    }
    launcher::open_project(app, id, out);
    true
}

/// Select the Projects-panel row for project `id`, with the manual-move
/// bookkeeping (drop pending selection-follows, remember the context being
/// left). Does NOT restore the target's remembered worktree/session — the
/// caller decides. False when the project is gone from the tree.
pub(crate) fn select_project_row_by_id(app: &mut App, id: &nebula_core::ProjectId) -> bool {
    let rows = app.project_rows();
    let Some(row) = rows.iter().position(|i| &app.tree.projects[*i].id == id) else {
        return false;
    };
    app.requests.select_worktree_when_seen = None;
    remember_context(app);
    let left = app.selected_project().map(|p| p.id.clone());
    app.nav.sel_project = row;
    app.reopen_projects();
    carry_open_band(app, left);
    true
}

/// After the cursor moved from project `left` to whichever is selected
/// now: the ACCORDION's open band is one per project, so the band open on
/// the grid being left is filed away under `left`
/// ([`App::launcher_open_bands`]) and the one the new project was left
/// with comes back open — switching away and back finds it as it was,
/// whatever was opened in between. Nothing moves when the project did
/// not change.
pub(crate) fn carry_open_band(app: &mut App, left: Option<ProjectId>) {
    let now = app.selected_project().map(|p| p.id.clone());
    if now == left {
        return;
    }
    if let Some(pid) = left {
        match app.launcher.launcher_expanded.take() {
            Some(open) => app.launcher.launcher_open_bands.insert(pid, open),
            None => app.launcher.launcher_open_bands.remove(&pid),
        };
    }
    app.launcher.launcher_expanded =
        now.and_then(|pid| app.launcher.launcher_open_bands.get(&pid).cloned());
}

/// Land the panel selections on a `/` palette pick. A project or worktree
/// pick moves the selection (restoring remembered child rows, like a manual
/// switch), then hands focus to the next visible child panel, since picking
/// either by name is a step towards one of its children, not an errand in
/// the column it names. A session pick with `attach` opens
/// it immediately, exactly like Enter on its row; without, it only lands
/// on the row in the Sessions panel, previewing like ↑/↓ there. Targets
/// are re-validated against the
/// tree — a pick can race a removal, in which case it flashes instead of
/// jumping.
/// Land the palette/finder on `target`, attaching without the debounce —
/// the user typed a query and picked a row, which is as explicit as it gets.
pub(crate) fn jump_to_target(
    app: &mut App,
    target: PaletteTarget,
    landing: Landing,
    out: &mut Vec<ClientRequest>,
) {
    jump_to_target_inner(app, target, landing, out);
    fire_pending_attach(app, out);
}

pub(crate) fn jump_to_target_inner(
    app: &mut App,
    target: PaletteTarget,
    landing: Landing,
    out: &mut Vec<ClientRequest>,
) {
    match target {
        // A project picked by name lands as its tab would
        // (`launcher::open_tab`): the card it was last left on, or with no
        // session in it the empty grid and no pane.
        PaletteTarget::Project(id) => launcher::open_tab(app, &id, out),
        PaletteTarget::Worktree(id) => {
            if app.selected_worktree().is_some_and(|w| w.id == id) {
                app.nav.focus = Focus::Sessions;
                return;
            }
            let found = app
                .tree
                .worktrees
                .iter()
                .find(|w| w.id == id)
                .map(|w| w.project_id.clone())
                .is_some_and(|pid| select_project_row_by_id(app, &pid));
            let index = found.then(|| app.worktree_row_of(&id)).flatten();
            let Some(index) = index else {
                app.chrome.flash = Some("worktree no longer exists".into());
                return;
            };
            app.nav.sel_worktree = index;
            restore_session(app, out);
            app.nav.focus = Focus::Sessions;
            // A checkout picked by name is its BAND on the grid, with the
            // pane on the card it was last left on.
            launcher::land_on_grid(app);
        }
        PaletteTarget::Session(id) => {
            let worktree = app
                .tree
                .agents
                .iter()
                .find(|a| a.id == id)
                .map(|a| a.worktree_id.clone());
            let found = worktree.as_ref().is_some_and(|wid| {
                app.tree
                    .worktrees
                    .iter()
                    .find(|w| &w.id == wid)
                    .map(|w| w.project_id.clone())
                    .is_some_and(|pid| select_project_row_by_id(app, &pid))
            });
            let wt_index = found
                .then(|| worktree.as_ref().and_then(|wid| app.worktree_row_of(wid)))
                .flatten();
            let Some(wt_index) = wt_index else {
                app.chrome.flash = Some(SESSION_GONE.into());
                return;
            };
            app.nav.sel_worktree = wt_index;
            let Some(index) = app
                .visible_session_rows()
                .iter()
                .position(|r| matches!(r, SessionRow::Agent(a) if a.id == id))
            else {
                // Vanished (or got archived out of view) mid-pick: land on
                // its worktree instead of attaching.
                restore_session(app, out);
                app.nav.focus = Focus::Sessions;
                app.chrome.flash = Some(SESSION_GONE.into());
                return;
            };
            app.nav.sel_session = index;
            // A session picked by name is its card on the grid, aimed at
            // — and on screen: its worktree opens if it was folded.
            launcher::land_on_card(app);
            match landing {
                Landing::Attach => attach_selected(app, out),
                Landing::FocusOnly => {
                    app.nav.focus = Focus::Sessions;
                    preview_selected(app, out);
                }
            }
        }
        // A pull request is a row of its project's OPEN PRS group, under
        // the checkouts: the jump selects that project (restoring its
        // context like a manual switch), unfolds the group if it was
        // folded — a folded group has no rows to land on — and puts the
        // Worktrees cursor on the row, so the pane reads the pull request
        // exactly as ↓ onto it would (`App::previewed_pr`). Nothing else
        // is needed for a PR with no checkout or session of its own. The
        // browser stays an explicit ask: `Attach` is "what Enter on its
        // row does", and on this row that is the browser.
        PaletteTarget::PullRequest { project, url } => {
            let changed = app
                .selected_project()
                .map(|p| p.id != project)
                .unwrap_or(true);
            if !select_project_row_by_id(app, &project) {
                app.chrome.flash = Some("project no longer exists".into());
                return;
            }
            if changed {
                restore_context(app, out);
            }
            if app.launcher.open_prs_collapsed {
                app.launcher.open_prs_collapsed = false;
                app.chrome.dirty = true;
            }
            let Some(row) = app.open_pr_row_of(&url) else {
                app.chrome.flash = Some(PR_GONE.into());
                return;
            };
            // Re-picking the row the cursor is already on keeps the
            // reader's scroll; a move re-arms the detail fetch as any
            // cursor move onto the row does.
            if app.nav.sel_worktree != row {
                select_worktree_row(app, row, out);
            }
            app.nav.focus = Focus::Worktrees;
            if landing == Landing::Attach {
                open_link(app, &url, out);
            }
        }
    }
}

/// Land the panel selection on `sref`'s session and attach it — the metrics
/// modal's Enter. The same walk as the palette's session jump, generalized
/// to terminal tabs.
pub(crate) fn open_session(app: &mut App, sref: SessionRef, out: &mut Vec<ClientRequest>) {
    let worktree = match &sref {
        SessionRef::Agent(id) => app
            .tree
            .agents
            .iter()
            .find(|a| &a.id == id)
            .map(|a| a.worktree_id.clone()),
        SessionRef::Terminal(id) => app
            .tree
            .terminals
            .iter()
            .find(|t| &t.id == id)
            .map(|t| t.worktree_id.clone()),
    };
    let found = worktree.as_ref().is_some_and(|wid| {
        app.tree
            .worktrees
            .iter()
            .find(|w| &w.id == wid)
            .map(|w| w.project_id.clone())
            .is_some_and(|pid| select_project_row_by_id(app, &pid))
    });
    let wt_index = found
        .then(|| worktree.as_ref().and_then(|wid| app.worktree_row_of(wid)))
        .flatten();
    let Some(wt_index) = wt_index else {
        app.chrome.flash = Some(SESSION_GONE.into());
        return;
    };
    app.nav.sel_worktree = wt_index;
    let Some(index) = app
        .visible_session_rows()
        .iter()
        .position(|r| r.sref().as_ref() == Some(&sref))
    else {
        restore_session(app, out);
        app.nav.focus = Focus::Sessions;
        app.chrome.flash = Some(SESSION_GONE.into());
        return;
    };
    app.nav.sel_session = index;
    attach_selected(app, out);
}

/// What a palette pick does once it has found its session row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Landing {
    /// Open the session in the pane, exactly like Enter on its row.
    Attach,
    /// Only land the cursor on the row, previewing like ↑/↓ there.
    FocusOnly,
}

impl Landing {
    /// The palette's Enter: attaches when the setting says so.
    pub(crate) fn for_enter(attaches: bool) -> Self {
        if attaches {
            Landing::Attach
        } else {
            Landing::FocusOnly
        }
    }

    /// The palette's Enter on `target`: a session attaches when the setting
    /// says so; a pull request only ever lands on its row with the pane
    /// reading it — the setting is about attaching, and a pull request has
    /// nothing to attach — so `Ctrl+O` stays the one palette key that also
    /// hands it to the browser.
    ///
    /// One target overrides the setting: a session that NEEDS FEEDBACK —
    /// the red dot, a permission prompt or a question sitting unanswered —
    /// always attaches, because the only thing to do with a jump to a
    /// question is answer it, and the quiet landing would leave it asking.
    /// `Ctrl+O` / `Ctrl+F` still name their own landing, so this is Enter's
    /// rule alone.
    pub(crate) fn for_enter_on(app: &App, target: &PaletteTarget, attaches: bool) -> Self {
        match target {
            PaletteTarget::PullRequest { .. } => Landing::FocusOnly,
            PaletteTarget::Session(id) if session_needs_feedback(app, id) => Landing::Attach,
            _ => Landing::for_enter(attaches),
        }
    }
}

/// Whether `id` is a live session waiting on the user — the red row that
/// [`Landing::for_enter_on`] attaches whatever the setting says.
pub(crate) fn session_needs_feedback(app: &App, id: &AgentId) -> bool {
    app.tree
        .agents
        .iter()
        .any(|a| &a.id == id && a.status == nebula_core::AgentStatus::NeedsFeedback)
}

/// `.` / `,`: land on the next (`step` 1) or previous (`step` -1) session
/// in the PALETTE's attention order — NEEDS FEEDBACK, then RUNNING, then
/// UNSEEN, then everything else by recency — across every project,
/// wrapping at both ends. It is `/` `Enter` with no modal: the ring is the
/// palette's session rows before a query is typed
/// ([`crate::palette::attention_sessions`]), and the landing is the
/// palette's, so a session in another project selects that project and
/// its worktree, and puts the cursor on the row — reading it, if it was an unwatched finish. The walk starts from
/// the session under the SESSIONS PANEL cursor; when nothing on the ring is
/// selected (a terminal, a link, an archived row, an empty worktree) `.`
/// starts at the top and `,` at the bottom, so the two stay each other's
/// reverse.
pub(crate) fn jump_attention(
    app: &mut App,
    step: i64,
    attaches: bool,
    out: &mut Vec<ClientRequest>,
) {
    let ring = crate::palette::attention_sessions(&app.tree);
    if ring.is_empty() {
        app.chrome.flash = Some(NO_SESSIONS_TO_JUMP.into());
        return;
    }
    let len = ring.len() as i64;
    let at = app
        .selected_session()
        .and_then(|a| ring.iter().position(|id| *id == a.id));
    let next = match at {
        Some(i) => (i as i64 + step).rem_euclid(len),
        None if step > 0 => 0,
        None => len - 1,
    };
    let target = PaletteTarget::Session(ring[next as usize].clone());
    let landing = Landing::for_enter_on(app, &target, attaches);
    jump_to_target(app, target, landing, out);
}

pub(crate) fn move_selection(app: &mut App, delta: i64, out: &mut Vec<ClientRequest>) {
    // (row count, cursor) of the focused column.
    let (len, sel) = match app.nav.focus {
        Focus::Projects => (app.project_rows().len(), app.nav.sel_project),
        Focus::Worktrees => {
            // Stepping down off the last row into a folded OPEN PRS group
            // opens it and lands on its first pull request: the rows are
            // where the cursor is headed, so there is no sense stopping
            // at the header.
            let checkouts = app.worktree_row_count();
            if delta > 0
                && app.launcher.open_prs_collapsed
                && app.nav.sel_worktree + 1 >= checkouts
                && !app.listed_open_prs().is_empty()
            {
                app.launcher.open_prs_collapsed = false;
                app.chrome.dirty = true;
                // The first pull request's row, once the group is open —
                // a checkout that just moved under one leaves the plain
                // rows, so it is not simply the old count.
                let first = app
                    .worktree_rows()
                    .iter()
                    .position(|row| row.open_pr().is_some())
                    .unwrap_or(checkouts);
                select_worktree_row(app, first, out);
                return;
            }
            // And off the last row above a folded ISSUES group — the last
            // pull request, or the last checkout when there are none —
            // into that group, the same way.
            if delta > 0
                && app.launcher.issues_collapsed
                && app.nav.sel_worktree + 1 >= checkouts
                && !app.listed_issues().is_empty()
            {
                app.launcher.issues_collapsed = false;
                app.chrome.dirty = true;
                let first = app
                    .worktree_rows()
                    .iter()
                    .position(|row| row.open_issue().is_some())
                    .unwrap_or(checkouts);
                select_worktree_row(app, first, out);
                return;
            }
            (checkouts, app.nav.sel_worktree)
        }
        Focus::Sessions => (app.visible_session_rows().len(), app.nav.sel_session),
        Focus::Terminal => return,
    };
    if len == 0 {
        return;
    }
    let new = (sel as i64 + delta).clamp(0, len as i64 - 1) as usize;
    if new == sel {
        return;
    }
    // Selecting a different parent resets child selections.
    match app.nav.focus {
        Focus::Projects => select_project_row(app, new, out),
        Focus::Worktrees => select_worktree_row(app, new, out),
        Focus::Sessions => select_session_row(app, new, ATTACH_DEBOUNCE, out),
        Focus::Terminal => {}
    }
}

/// Move the Projects cursor to a *different* row `i`: the context being
/// left is remembered, and the new project's is restored.
pub(crate) fn select_project_row(app: &mut App, i: usize, out: &mut Vec<ClientRequest>) {
    // A manual move outranks any pending selection-follows.
    app.requests.select_worktree_when_seen = None;
    remember_context(app);
    let owner_before = app.selected_project().map(|p| p.id.clone());
    app.nav.sel_project = i;
    app.reopen_projects();
    carry_open_band(app, owner_before.clone());
    if app.selected_project().map(|p| p.id.clone()) != owner_before {
        restore_context(app, out);
    }
}

/// Move the Worktrees cursor to a *different* row `i` and bring up its
/// session. Stepping onto an open-PR or issue row is not a worktree
/// switch: it has no sessions to restore and nothing to attach, so the
/// pane is left exactly as it was.
pub(crate) fn select_worktree_row(app: &mut App, i: usize, out: &mut Vec<ClientRequest>) {
    app.requests.select_worktree_when_seen = None;
    remember_context(app);
    app.nav.sel_worktree = i;
    if app.selected_worktree().is_some() {
        restore_session(app, out);
    }
    schedule_pr_detail(app);
    crate::issues::schedule_detail(app);
}

/// Show the selected session in the terminal pane WITHOUT taking focus or
/// the input lock — walking the list with ↑/↓ (or single-clicking a row)
/// previews each session so it can be read; Enter (or a double-click) is
/// what commits: focus + lock. Archived rows don't preview.
/// Debounced only for a session the daemon has reaped — see `preview_inner`.
pub(crate) fn preview_selected(app: &mut App, out: &mut Vec<ClientRequest>) {
    preview_inner(app, ATTACH_DEBOUNCE, out);
}

/// Preview with no debounce — a click points at exactly one row, so there is
/// no sweep to wait out.
pub(crate) fn preview_selected_now(app: &mut App, out: &mut Vec<ClientRequest>) {
    preview_inner(app, Duration::ZERO, out);
}

pub(crate) fn preview_inner(app: &mut App, delay: Duration, out: &mut Vec<ClientRequest>) {
    // A folded worktree's header (the NESTED layout) has no card under
    // the cursor for the pane to read (`App::on_folded_band`).
    if app.on_folded_band() {
        return;
    }
    let Some(row) = app.selected_session_row() else {
        return;
    };
    if row.is_archived_agent() {
        return;
    }
    // A link row has no session behind it: leave whatever was in the pane
    // rather than blanking it while the cursor passes through the group.
    let Some(sref) = row.sref() else {
        return;
    };
    attach_inner(app, sref, delay, out);
}

/// Enter on the Sessions panel: attach the session under the cursor, or —
/// on a link row — hand its URL to the browser and stay put.
pub(crate) fn attach_selected(app: &mut App, out: &mut Vec<ClientRequest>) {
    let rows = app.visible_session_rows();
    let Some(row) = rows.get(app.nav.sel_session) else {
        return;
    };
    let Some(sref) = row.sref() else {
        if let Some(link) = row.as_link() {
            open_link(app, link.url(), out);
        }
        return;
    };
    // A Cloud row leads out of nebula too: there is no PTY to lock into,
    // only the session's page.
    if let Some(url) = cloud_session_url_of(app, &sref) {
        open_link(app, &url, out);
        return;
    }
    activate::attach(app, sref, out);
}

/// The claude.ai page behind `sref`, when it is a Claude Cloud row.
pub(crate) fn cloud_session_url_of(app: &App, sref: &SessionRef) -> Option<String> {
    let SessionRef::Agent(id) = sref else {
        return None;
    };
    app.tree
        .agents
        .iter()
        .find(|a| &a.id == id)
        .and_then(|a| a.cloud_session_url())
}

/// Full-screen whatever the pane is showing, with the input lock on:
/// the grid gives way and the keys go to the PTY. Enter and a
/// double-click on a card step into the pane under the cards
/// (`launcher::enter_pane`); only a body too short to draw that pane
/// comes here instead (`launcher::open_session`). [`leave_terminal_lock`]
/// is its undo.
pub(crate) fn zoom_pane(app: &mut App, out: &mut Vec<ClientRequest>) -> bool {
    // Readers cover the pane according to focus, so ask as the full-screen
    // surface would have it.
    let previous_focus = std::mem::replace(&mut app.nav.focus, Focus::Terminal);
    if !app.pane_shows_terminal() {
        app.nav.focus = previous_focus;
        app.chrome.flash = Some(NOTHING_TO_FULL_SCREEN.into());
        return false;
    }
    app.pane.collapsed = true;
    app.pane.term_locked = true;
    if app.pane_accepts_input() {
        fire_pending_attach(app, out);
    }
    true
}

/// What `^F` says with nothing in the pane to full-screen.
pub(crate) const NOTHING_TO_FULL_SCREEN: &str = "no session in the pane — j/k onto one, then ^F";

/// Leave a locked pane for the cards (`Focus::Sessions`). Also ends a
/// full screen, so there is something on screen to land in — which is
/// what takes a full-screen session back to the LAUNCHER VIEW's GRID.
pub(crate) fn leave_terminal_lock(app: &mut App) {
    app.pane.collapsed = false;
    app.pane.term_locked = false;
    app.nav.focus = Focus::Sessions;
}

/// Open a saved link in the browser, reporting either way — the browser
/// comes up in front of the terminal, so a silent failure would read as
/// "nebula did nothing". A pull request is marked read on the way out: the
/// conversation is about to be on screen, so the row's unread count starts
/// again from here.
pub(crate) fn open_link(app: &mut App, url: &str, out: &mut Vec<ClientRequest>) {
    if open_url(url) {
        app.chrome.flash = Some(format!("opened {}", crate::app::pretty_url(url)));
        mark_pr_seen(app, url, out);
    } else {
        app.chrome.flash = Some(format!("couldn't open {url}"));
    }
}

/// Record that this pull request has been read up to whatever nebula knows
/// about it. Applied locally as well as sent, so the badge clears on this
/// frame instead of waiting for the daemon to say so — and skipped when the
/// URL isn't a PR, or when the mark wouldn't move.
pub(crate) fn mark_pr_seen(app: &mut App, url: &str, out: &mut Vec<ClientRequest>) {
    let Some(marker) = app
        .github
        .pull_requests
        .values()
        .flatten()
        .find(|pr| pr.url == url)
        .map(|pr| pr.seen_marker().to_string())
    else {
        return;
    };
    if app.github.pr_seen.get(url) == Some(&marker) {
        return;
    }
    app.github.pr_seen.insert(url.to_string(), marker.clone());
    app.chrome.dirty = true;
    out.push(ClientRequest::MarkPrSeen {
        url: url.to_string(),
        marker,
    });
}

/// Record that this agent's session is on screen, so a turn it finished
/// unwatched (`Agent::unseen`) stops counting on its worktree and project
/// rows. Applied locally as well as sent, so the counts drop on this frame
/// instead of waiting for the daemon's upsert — and skipped entirely when
/// there is nothing to clear.
pub(crate) fn mark_agent_seen(app: &mut App, id: &AgentId, out: &mut Vec<ClientRequest>) {
    let Some(a) = app.tree.agents.iter_mut().find(|a| &a.id == id && a.unseen) else {
        return;
    };
    a.unseen = false;
    app.chrome.dirty = true;
    out.push(ClientRequest::MarkAgentSeen { id: id.clone() });
}

/// Show `sref` in the pane, telling the daemon once the selection settles.
/// The pane swaps immediately — the header must never name a session other
/// than the selected one — and a live session is attached on the spot; only
/// a reaped one waits out [`ATTACH_DEBOUNCE`], because attaching it makes
/// the daemon fork an agent CLI, and a cursor merely passing through a row
/// has not asked for that.
pub(crate) fn attach(app: &mut App, sref: SessionRef, out: &mut Vec<ClientRequest>) {
    attach_inner(app, sref, ATTACH_DEBOUNCE, out);
}

/// Attach with no debounce: the user named this row outright (Enter, a
/// click, the menu, a session they just created), so there is nothing to
/// wait to see whether they meant it.
pub(crate) fn attach_now(app: &mut App, sref: SessionRef, out: &mut Vec<ClientRequest>) {
    attach_inner(app, sref, Duration::ZERO, out);
}

pub(crate) fn attach_inner(
    app: &mut App,
    sref: SessionRef,
    delay: Duration,
    out: &mut Vec<ClientRequest>,
) {
    // Whatever lands in the pane has been looked at — walking the cursor
    // onto a row previews it here, so this is where the counts come down.
    // Keyed to the pane swap, not to the Attach: the user is reading the
    // screen during the debounce just the same.
    if let SessionRef::Agent(id) = &sref {
        mark_agent_seen(app, id, out);
    }
    // A Cloud row has nothing to attach: its create PTY is gone seconds
    // after printing the session id, and the daemon refuses to boot a
    // local CLI in its name. The pane shows the CLOUD SESSION PANEL from
    // the row itself (`App::previewed_cloud`), so let go of whatever was
    // held and leave the pane empty underneath it.
    if cloud_session_url_of(app, &sref).is_some() {
        detach_pane(app, out);
        return;
    }
    let showing = app
        .pane
        .term
        .as_ref()
        .is_some_and(|t| t.sref == sref && !t.exited);
    let live = app.session_is_live(&sref);
    if !showing {
        let (cols, rows) = pane_size(app);
        // Fresh screen, so any persisted selection would point at stale cells.
        app.pane.term_selection = None;
        // The screen being left goes aside for a quick return, and the one
        // arriving comes back from there when it was shown recently: its
        // last screen is up on this frame, and the Attach below asks only
        // for what it missed. Anything else starts blank and replays.
        if let Some(leaving) = app.pane.term.take() {
            app.stash_term(leaving);
        }
        let term = match app.take_cached_term(&sref) {
            Some(mut kept) => {
                kept.resize(cols, rows);
                kept.set_scroll(0);
                kept
            }
            None => {
                let mut fresh = AttachedTerm::new(sref.clone(), cols, rows);
                // A reaped session is about to be booted by this attach:
                // the pane can say so while it waits, rather than show
                // the void a live session's replay fills within a frame.
                fresh.booting = !live;
                fresh
            }
        };
        app.pane.term = Some(term);
        app.chrome.dirty = true;
    }
    // Attaching a session the daemon still holds only replays its ring —
    // there is no CLI to fork, so there is nothing to wait to see whether
    // the user meant it. Walking onto one, or switching a worktree or
    // project onto one, is as immediate as clicking it. Only
    // a reaped session — the one case the debounce exists for — keeps the
    // wait.
    let delay = if live { Duration::ZERO } else { delay };
    if delay.is_zero() {
        app.pane.pending_attach = None;
        send_attach(app, sref, out);
    } else if app.pane.attached_sref.as_ref() == Some(&sref) {
        // The daemon already holds it; nothing to send, nothing to wait for.
        app.pane.pending_attach = None;
    } else {
        app.pane.pending_attach = Some((sref, std::time::Instant::now() + delay));
    }
}

/// Move the daemon-side attachment to `sref`, releasing whatever it held.
/// Idempotent, so every caller can just ask for the session it wants.
pub(crate) fn send_attach(app: &mut App, sref: SessionRef, out: &mut Vec<ClientRequest>) {
    if app.pane.attached_sref.as_ref() == Some(&sref) {
        return;
    }
    if let Some(old) = app.pane.attached_sref.take() {
        out.push(ClientRequest::Detach { session: old });
    }
    // A QUICK PROMPT stand-in has no PTY behind it yet: the pane keeps its
    // "starting…" with nothing attached, and the create's Ack attaches
    // the real session. Letting go of the previous one above still
    // matters — a keystroke must not land there through a stale hold.
    if app.is_placeholder_session(&sref) {
        return;
    }
    let (cols, rows) = pane_size(app);
    // A screen kept from an earlier visit asks for the bytes it missed
    // rather than the whole ring; the daemon answers with a replay that
    // starts exactly there (`AttachedTerm::apply_scrollback` appends it
    // onto the screen), or with the whole ring when that point has fallen
    // off, which rebuilds the screen as a first attach would.
    // …unless the whole ring is the point: a history being brought back.
    let from_seq = app
        .pane
        .term
        .as_ref()
        .filter(|t| t.sref == sref && t.painted && t.pending_scroll.is_none())
        .map(|t| t.next_seq);
    app.pane.attached_sref = Some(sref.clone());
    out.push(ClientRequest::Attach {
        session: sref,
        from_seq,
        cols,
        rows,
    });
}

/// The user scrolled up in a pane whose history was let go while its screen
/// sat in the cache (`AttachedTerm::drop_history`): ask the DAEMON for the
/// whole ring again. The replay rebuilds the screen and everything above
/// it, and lands the reader on `scroll` — the notch that asked. One wheel
/// notch late, once per return, is what the instant return costs.
pub(crate) fn rehydrate_history(app: &mut App, scroll: usize, out: &mut Vec<ClientRequest>) {
    let Some(term) = &mut app.pane.term else {
        return;
    };
    if !term.history_dropped {
        return;
    }
    term.history_dropped = false;
    term.pending_scroll = Some(scroll);
    let sref = term.sref.clone();
    // Let go first, so the forwarder of the attachment being replaced is
    // gone before the replay that supersedes it is sent.
    if app.pane.attached_sref.as_ref() == Some(&sref) {
        app.pane.attached_sref = None;
        out.push(ClientRequest::Detach {
            session: sref.clone(),
        });
    }
    app.pane.pending_attach = None;
    send_attach(app, sref, out);
}

/// Send the armed attach now — the selection settled, or something needs
/// the session live this instant (a keystroke about to be forwarded).
pub(crate) fn fire_pending_attach(app: &mut App, out: &mut Vec<ClientRequest>) {
    let Some((sref, _)) = app.pane.pending_attach.take() else {
        return;
    };
    send_attach(app, sref, out);
}

/// Release the daemon-side attachment: whatever the daemon holds, or — when
/// an attach is still debounced — the session the pane is showing, so a
/// caller that only knows about the pane still lets go. A Detach the daemon
/// has no attachment for costs it a hash lookup and nothing else.
pub(crate) fn release_attachment(app: &mut App, out: &mut Vec<ClientRequest>) {
    app.pane.pending_attach = None;
    // A QUICK PROMPT stand-in in the pane was never attached — `send_attach`
    // stops at it — so there is nothing to let go of there.
    let session = app.pane.attached_sref.take().or_else(|| {
        app.pane
            .term
            .as_ref()
            .map(|t| t.sref.clone())
            .filter(|s| !app.is_placeholder_session(s))
    });
    if let Some(session) = session {
        out.push(ClientRequest::Detach { session });
    }
}

/// Blank the pane and release the daemon-side attachment. The screen is
/// kept for a quick return (`App::term_cache`); the session is still there.
pub(crate) fn detach_pane(app: &mut App, out: &mut Vec<ClientRequest>) {
    release_attachment(app, out);
    if let Some(leaving) = app.pane.term.take() {
        app.stash_term(leaving);
    }
    app.pane.term_locked = false;
}

/// Terminal-pane grid for spawn/attach requests; the fallback keeps
/// pre-first-draw requests from booting a 0×0 PTY.
pub(crate) fn pane_size(app: &App) -> (u16, u16) {
    let area = app.pane.term_area;
    if pane_usable(area) {
        (area.width, area.height)
    } else {
        FALLBACK_PANE
    }
}

/// Arm the debounced session prewarm for the selected worktree; the main
/// loop fires it once the selection has rested there (PREWARM_DEBOUNCE).
pub(crate) fn schedule_prewarm(app: &mut App) {
    app.requests.pending_prewarm = app
        .selected_worktree()
        .map(|w| (w.id.clone(), std::time::Instant::now() + PREWARM_DEBOUNCE));
}

/// Send the armed worktree-sessions prewarm. Re-firing for an already-warm
/// worktree is a cheap daemon-side no-op, so staleness needs no handling
/// beyond the daemon skipping rows that no longer exist.
pub(crate) fn fire_pending_prewarm(app: &mut App, out: &mut Vec<ClientRequest>) {
    let Some((worktree, _)) = app.requests.pending_prewarm.take() else {
        return;
    };
    // A QUICK PROMPT stand-in checkout is not on disk yet; the launch that
    // made it re-arms this once the real one is.
    if app.is_placeholder_worktree(&worktree) {
        return;
    }
    let (cols, rows) = pane_size(app);
    out.push(ClientRequest::PrewarmWorktreeSessions {
        worktree: worktree.clone(),
        cols,
        rows,
    });
    // The selected worktree also keeps one Claude session standing by, so
    // creating a session there adopts an already-booted CLI.
    out.extend(default_claude_prewarm(worktree));
    app.requests.next_keepwarm = Some(std::time::Instant::now() + KEEPWARM_REFRESH);
}

/// Flashed at a launch, a delete or a terminal aimed at a QUICK PROMPT
/// stand-in checkout the DAEMON has not cut yet.
pub(crate) const WORKTREE_STILL_CREATING: &str = "worktree is still being created";

/// The PROJECT a checkout belongs to — what `CreatePrAgent` is addressed
/// to. None only if the row went away between the picker and Enter.
pub(crate) fn project_of_worktree(app: &App, worktree: &WorktreeId) -> Option<ProjectId> {
    app.tree
        .worktrees
        .iter()
        .find(|w| &w.id == worktree)
        .map(|w| w.project_id.clone())
}

pub(crate) fn create_agent(app: &mut App, draft: AgentLaunchDraft, out: &mut Vec<ClientRequest>) {
    // Every launch is work in its project, whose tab goes to the far left
    // — a BACKGROUND LAUNCH's too, though nothing else it does moves. Not
    // a create that already carries its stand-in row: that is the second
    // half of a launch counted when its Enter was pressed, and the user
    // may have gone on to work somewhere else while the checkout was cut.
    if draft.placeholder.is_none() {
        worked_in(app, &draft.worktree);
    }
    // A stand-in checkout is not a place the DAEMON knows. A launch that
    // made it (a QUICK PROMPT's, a PR SESSION's) follows on its own Ack;
    // one fired into the NEW WORKTREE modal's row waits on that Ack
    // instead of being refused.
    if app.is_placeholder_worktree(&draft.worktree) {
        placeholder::defer_launch(app, draft, out);
        return;
    }
    let AgentLaunchDraft {
        worktree,
        kind,
        custom,
        model,
        effort,
        name,
        cloud_prompt,
        starting_prompt,
        reopen_on_error,
        pr,
        issue_url,
        focus_pane,
        placeholder,
        follow,
    } = draft;
    // A PR SESSION is addressed to the PROJECT, not to a checkout: the
    // DAEMON runs it in the PR head branch's own worktree, creating that
    // checkout the first time. `worktree` here only names which project.
    let pr = match pr {
        Some(pr) => {
            let Some(project) = project_of_worktree(app, &worktree) else {
                app.chrome.flash = Some("worktree no longer exists".into());
                return;
            };
            // The checkout it runs in: the project's worktree already on
            // the head branch, or one the DAEMON cuts behind this create
            // — a fetch, `git worktree add` and the WORKTREE HOOK, seconds
            // during which the panels used to show nothing. So the rows
            // go up now, as a QUICK PROMPT's do, and the upsert and the
            // Ack turn them into the real ones. A launch while that
            // checkout is still being cut waits, as every launch into a
            // stand-in does.
            let on_head = app
                .tree
                .worktrees
                .iter()
                .find(|w| w.project_id == project && w.branch == pr.head)
                .map(|w| w.id.clone());
            let rows = match on_head {
                Some(id) if app.is_placeholder_worktree(&id) => {
                    // The box is already closed; a task typed into it
                    // comes back with the refusal, to send again once the
                    // checkout is real.
                    if let Some((kind, text)) = reopen_on_error {
                        reopen_prompt_with(app, kind, text);
                    }
                    app.chrome.flash = Some(WORKTREE_STILL_CREATING.into());
                    return;
                }
                Some(_) => None,
                None => Some(placeholder::stage(
                    app,
                    project.clone(),
                    pr.head.clone(),
                    kind,
                    custom.clone(),
                    model.clone(),
                    effort.clone(),
                    starting_prompt.is_some(),
                    follow,
                    out,
                )),
            };
            Some((project, pr, rows))
        }
        None => None,
    };
    let pr_rows = pr.as_ref().and_then(|(_, _, rows)| rows.clone());
    let pr_url = pr.as_ref().map(|(_, pr, _)| pr.url.clone());
    let placeholder = placeholder.or_else(|| pr_rows.as_ref().map(|rows| rows.agent.clone()));
    // A stand-in row was named for this very create when it went up, so
    // the name comes off it — `default_session_name` would count it as
    // taken and move on to the next number.
    let stand_in_name = placeholder
        .as_ref()
        .and_then(|id| app.tree.agents.iter().find(|a| &a.id == id))
        .map(|a| a.name.clone());
    let intent = match (reopen_on_error, &cloud_prompt, pr_rows) {
        (reopen, _, Some(rows)) => PendingIntent::AttachCreatedPrSession {
            focus: focus_pane,
            placeholder: rows,
            reopen,
            pr_url: pr_url.clone().unwrap_or_default(),
        },
        (Some((kind, task)), _, None) => PendingIntent::AttachCreatedWithCloudRetry {
            kind,
            task,
            focus: focus_pane,
            placeholder,
        },
        (None, Some(task), None) => PendingIntent::AttachCreatedWithCloudRetry {
            kind: PromptKind::ClaudeCloudTask {
                worktree: worktree.clone(),
                name: name.clone(),
                model: model.clone(),
                effort: effort.clone(),
            },
            task: task.clone(),
            focus: focus_pane,
            placeholder,
        },
        (None, None, None) => PendingIntent::AttachCreated {
            focus: focus_pane,
            placeholder,
        },
    };
    let auto_title = name.is_empty();
    let name = match (auto_title, stand_in_name) {
        (false, _) => name,
        (true, Some(name)) => name,
        (true, None) => app.default_session_name("agent"),
    };
    let cloud = cloud_prompt.is_some();
    let with_first_prompt = starting_prompt.is_some();
    let req_id = app.alloc_req_id(intent);
    // The second half of a launch the user already navigated away from:
    // its Ack leaves the cursors where they are, as the first half's did.
    if !follow {
        app.requests.left_behind.insert(req_id);
    }
    out.push(match pr {
        Some((project, pr, _)) => {
            debug_assert!(!cloud);
            ClientRequest::CreatePrAgent {
                req_id,
                project,
                name,
                kind,
                custom_harness: custom.clone(),
                model,
                effort,
                auto_title,
                pr_url: pr.url,
                head: pr.head,
                starting_prompt,
            }
        }
        None => ClientRequest::CreateAgent {
            req_id,
            worktree: worktree.clone(),
            name,
            kind,
            custom_harness: custom.clone(),
            model,
            effort,
            auto_title,
            cloud_prompt,
            starting_prompt,
            issue_url,
        },
    });
    // The create consumes (or, off-spec, discards) the worktree's warm
    // Claude slot; refill it so the next create is instant too. A cloud
    // launch, and any launch carrying a STARTING PROMPT (a preset, the
    // QUICK PROMPT), never touches the slot, so there is nothing to refill.
    // Nor does a PR SESSION: the DAEMON adopts no unscoped spare for one,
    // and `worktree` there is the ROOT WORKTREE, which only names the
    // project — a refill would boot a Claude in the main checkout for a
    // launch that never runs there.
    if kind == AgentKind::Claude && !cloud && !with_first_prompt && pr_url.is_none() {
        out.extend(default_claude_prewarm(worktree));
    }
}

/// The one spec kept permanently warm: a Claude CLI at the configured
/// default model/effort. Creates matching it adopt the warm session
/// instantly; any other spec launches cold on purpose — off-default CLIs
/// would sit idle holding memory for a spec the user rarely repeats.
/// None while Claude is disabled in Settings: a harness the user turned
/// off should not keep a 150–300 MB WARM SPARE booted behind their back.
pub(crate) fn default_claude_prewarm(worktree: WorktreeId) -> Option<ClientRequest> {
    let cfg = crate::config::Config::load();
    if !cfg.kind_enabled(AgentKind::Claude) {
        return None;
    }
    Some(ClientRequest::PrewarmAgent {
        worktree,
        kind: AgentKind::Claude,
        model: cfg.default_model(AgentKind::Claude),
        effort: cfg.default_effort(AgentKind::Claude),
    })
}

/// Periodic re-assert of the standing warm Claude session for the selected
/// worktree. A young same-spec session makes this a daemon-side no-op and an
/// aging one is recycled in place, so without this tick the daemon's reaper
/// would empty the slot at its max age and the next create would boot cold.
pub(crate) fn fire_keepwarm(app: &mut App, out: &mut Vec<ClientRequest>) {
    let Some(worktree) = app.selected_worktree().map(|w| w.id.clone()) else {
        app.requests.next_keepwarm = None;
        return;
    };
    // The beat keeps going past a QUICK PROMPT stand-in — it is the real
    // checkout a moment later — but nothing is asked for it.
    if !app.is_placeholder_worktree(&worktree) {
        out.extend(default_claude_prewarm(worktree));
    }
    app.requests.next_keepwarm = Some(std::time::Instant::now() + KEEPWARM_REFRESH);
}
