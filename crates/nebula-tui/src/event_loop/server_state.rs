//! Extracted event-loop helper section.

use super::*;

pub(crate) fn refusal_flash(message: &str) -> String {
    let first = message.split(". ").next().unwrap_or(message);
    format!(
        "couldn't start this session: {}",
        first.trim_end_matches('.')
    )
}

/// The Ack of a create: select the new session and show it — `focus` also
/// enters and locks the pane. A QUICK PROMPT's stand-in row, when one was
/// up for this create, becomes the created row first, so the cursor and
/// the pane carry over instead of jumping.
///
/// Without `follow` — the user navigated away while the DAEMON worked —
/// the row is in the list and that is all: no cursor, no FOCUS and no
/// worktree switch goes back to it. A pane still on the stand-in (the
/// user stepped into it, or onto a pull request, which leaves the pane as
/// it was) goes on showing that session, now the real one.
pub(crate) fn attach_created(
    app: &mut App,
    id: EntityId,
    focus: bool,
    placeholder: Option<AgentId>,
    follow: bool,
    out: &mut Vec<ClientRequest>,
) {
    if let (Some(stand_in), EntityId::Agent(real)) = (&placeholder, &id) {
        placeholder::resolve_agent(app, stand_in, real);
    }
    // Where the user is, by id: the lead taken below re-sorts the band
    // the card landed in, and the cursor there is a row index.
    let before = selection_snapshot(app);
    // The card leads the sessions lists until its first turn starts, with
    // or without `follow`: where the cursor goes is one question, and
    // where the newest session sorts is another.
    if let EntityId::Agent(real) = &id {
        app.just_launched = Some(real.clone());
    }
    let sref = match id {
        EntityId::Agent(id) => Some(SessionRef::Agent(id)),
        EntityId::Terminal(id) => Some(SessionRef::Terminal(id)),
        _ => None,
    };
    let Some(sref) = sref else {
        return;
    };
    if !follow {
        reconcile_selection_inner(app, before, out);
        let stand_in_shown = placeholder.is_some_and(|stand_in| {
            app.term
                .as_ref()
                .is_some_and(|t| t.sref == SessionRef::Agent(stand_in))
        });
        if stand_in_shown {
            attach_now(app, sref, out);
        }
        return;
    }
    if matches!(sref, SessionRef::Terminal(_)) && app.launcher_active() {
        launcher::show_created_terminal(app);
    }
    app.select_when_seen = Some(sref.clone());
    // Its upsert usually lands just before this Ack; land the selection
    // now, or on the upsert otherwise.
    land_pending_selection(app, out);
    attach_now(app, sref, out);
    // Without `focus` the row is selected and the pane shows it, but the
    // cursor stays where the create was fired from — see
    // `quick_prompt_focus`.
    if focus {
        app.focus = Focus::Terminal;
        app.term_locked = true;
    }
}

/// A refused request's prompt comes back with what was typed in it.
pub(crate) fn reopen_prompt_with(app: &mut App, mut kind: PromptKind, text: String) {
    if let PromptKind::QuickPrompt(launch) = &mut kind {
        crate::quick_prompt::restack(app, launch);
    }
    open_prompt(app, kind);
    if let Some(Overlay::Prompt(prompt)) = &mut app.overlay {
        prompt.input.set_text(text);
    }
}

pub(crate) fn apply_upsert(app: &mut App, entity: nebula_core::Entity) {
    use nebula_core::Entity;
    match entity {
        Entity::Project(p) => {
            let selected = app.selected_project().map(|p| p.id.clone());
            upsert_by(&mut app.tree.projects, p, |x, y| x.id == y.id);
            // Reorders arrive as plain upserts with new sort_orders; stable
            // sort keeps snapshot order for legacy all-zero ties. The
            // selection follows the project it was on, so children stay put.
            app.tree.projects.sort_by_key(|x| x.sort_order);
            if let Some(id) = selected {
                let found = app
                    .project_rows()
                    .iter()
                    .position(|i| app.tree.projects[*i].id == id);
                if let Some(i) = found {
                    app.sel_project = i;
                }
            }
        }
        Entity::Worktree(w) => upsert_by(&mut app.tree.worktrees, w, |x, y| x.id == y.id),
        Entity::Agent(a) => {
            // A row the daemon re-homed (`nebula worktree`, a hook cwd in
            // another checkout) takes the cursor with it when it was the
            // selected session — otherwise the selection would silently
            // land on whatever row slid into its place.
            let selected = app.selected_session().map(|s| s.id);
            let moved = app
                .tree
                .agents
                .iter()
                .any(|x| x.id == a.id && x.worktree_id != a.worktree_id);
            let id = a.id.clone();
            upsert_by(&mut app.tree.agents, a, |x, y| x.id == y.id);
            if moved && selected.as_ref() == Some(&id) {
                app.select_when_seen = Some(SessionRef::Agent(id));
            }
        }
        Entity::Terminal(t) => {
            let id = t.id.clone();
            upsert_by(&mut app.tree.terminals, t, |x, y| x.id == y.id);
            if app
                .run_flash_when_seen
                .as_ref()
                .is_some_and(|(want, _)| *want == id)
            {
                if let (Some(command), Some((_, branch))) =
                    (run_command_of(app, &id), app.run_flash_when_seen.take())
                {
                    app.flash = Some(format!("▶ running {command} in {branch}"));
                }
            }
        }
        Entity::Link(l) => upsert_by(&mut app.tree.links, l, |x, y| x.id == y.id),
    }
}

/// The command a RUN TERMINAL in the tree runs; None for a plain shell or
/// a row not seen yet.
pub(crate) fn run_command_of(app: &App, id: &TerminalId) -> Option<String> {
    app.tree
        .terminals
        .iter()
        .find(|t| &t.id == id)
        .and_then(|t| t.run_command.clone())
}

/// Replace the first entry of `list` that `same` pairs with `item`, or
/// append `item` when there is none.
pub(crate) fn upsert_by<T>(list: &mut Vec<T>, item: T, same: impl Fn(&T, &T) -> bool) {
    match list.iter_mut().find(|x| same(x, &item)) {
        Some(existing) => *existing = item,
        None => list.push(item),
    }
}

pub(crate) fn apply_removal(app: &mut App, id: &nebula_core::EntityId) {
    use nebula_core::EntityId;
    match id {
        EntityId::Project(id) => {
            // Children cascade server-side; mirror that here.
            let wt_ids: Vec<_> = app
                .tree
                .worktrees
                .iter()
                .filter(|w| &w.project_id == id)
                .map(|w| w.id.clone())
                .collect();
            app.tree.agents.retain(|a| !wt_ids.contains(&a.worktree_id));
            app.tree
                .terminals
                .retain(|t| !wt_ids.contains(&t.worktree_id));
            app.tree.links.retain(|l| !wt_ids.contains(&l.worktree_id));
            app.pull_requests.retain(|w, _| !wt_ids.contains(w));
            app.pr_recheck.retain(|w, _| !wt_ids.contains(w));
            app.open_prs.remove(id);
            app.open_prs_failed.remove(id);
            app.pr_cache_dirty = true;
            app.tree.worktrees.retain(|w| &w.project_id != id);
            app.tree.projects.retain(|p| &p.id != id);
        }
        EntityId::Worktree(id) => {
            app.tree.agents.retain(|a| &a.worktree_id != id);
            app.tree.terminals.retain(|t| &t.worktree_id != id);
            app.tree.links.retain(|l| &l.worktree_id != id);
            app.pr_cache_dirty |= app.pull_requests.remove(id).is_some();
            app.pr_recheck.remove(id);
            app.tree.worktrees.retain(|w| &w.id != id);
        }
        EntityId::Agent(id) => app.tree.agents.retain(|a| &a.id != id),
        EntityId::Terminal(id) => {
            app.tree.terminals.retain(|t| &t.id != id);
            app.terminal_tails.remove(id);
        }
        EntityId::Link(id) => app.tree.links.retain(|l| &l.id != id),
    }
}

/// Optimistically remove a worktree row and its agent rows, returning a
/// snapshot that `restore_worktree_rows` can reinsert if the daemon-side
/// delete fails. None when the worktree isn't in the tree.
pub(crate) fn remove_worktree_rows(app: &mut App, id: &WorktreeId) -> Option<WorktreeRollback> {
    let index = app.tree.worktrees.iter().position(|w| &w.id == id)?;
    let worktree = app.tree.worktrees.remove(index);
    let mut agents = Vec::new();
    let mut kept = Vec::with_capacity(app.tree.agents.len());
    for (i, a) in std::mem::take(&mut app.tree.agents).into_iter().enumerate() {
        if &a.worktree_id == id {
            agents.push((i, a));
        } else {
            kept.push(a);
        }
    }
    app.tree.agents = kept;
    clamp_selections(app);
    Some(WorktreeRollback {
        index,
        worktree,
        agents,
    })
}

/// Rollback of `remove_worktree_rows`: reinsert the rows at (or near) their
/// old positions. Skips anything the daemon re-upserted in the meantime.
pub(crate) fn restore_worktree_rows(app: &mut App, rollback: WorktreeRollback) {
    let WorktreeRollback {
        index,
        worktree,
        agents,
    } = rollback;
    if !app.tree.worktrees.iter().any(|w| w.id == worktree.id) {
        let at = index.min(app.tree.worktrees.len());
        app.tree.worktrees.insert(at, worktree);
    }
    for (i, a) in agents {
        if !app.tree.agents.iter().any(|x| x.id == a.id) {
            let at = i.min(app.tree.agents.len());
            app.tree.agents.insert(at, a);
        }
    }
    clamp_selections(app);
    app.dirty = true;
}

/// Keep an open `/` palette in sync with tree changes (renames, removals,
/// new entities) so its rows never go stale under the user's cursor.
pub(crate) fn refresh_palette(app: &mut App) {
    if let Some(Overlay::Palette(palette)) = &mut app.overlay {
        palette.rebuild(&app.tree, &app.open_prs, app.hide_draft_prs);
    }
}

/// What each panel cursor pointed at, captured with `selection_snapshot`
/// before a tree mutation so `reconcile_selection` can compare afterwards.
#[derive(PartialEq)]
pub(crate) struct SelectionSnapshot {
    project: Option<nebula_core::ProjectId>,
    worktree: Option<WorktreeId>,
    /// The OPEN PRS row under the Worktrees cursor, by URL, when it is on
    /// one (`worktree` is None then). The pull requests list below the
    /// checkouts, so every checkout that comes or goes shifts them: left
    /// to its index, the cursor slid onto the next pull request — and a
    /// launch with no box (`n`, a `skip`-task preset) then ran against a
    /// pull request nobody picked.
    pr: Option<String>,
    /// The ISSUES row under it, the same way.
    issue: Option<String>,
    session: Option<SessionRef>,
    /// The Sessions panel row the cursor was on, and the group that row
    /// sat in. Following onto an archived row is only right when the
    /// cursor was already in the archived group, and a row that leaves
    /// hands the cursor to a neighbor in its own group when it can.
    session_index: usize,
    session_group: Option<SessionGroup>,
}

/// The Sessions panel's groups, in the order it lists them: the live
/// agents, TERMINALS, PULL REQUESTS, ARCHIVED. Each is one unbroken run
/// of rows, so a row's group changes only where the next one starts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SessionGroup {
    Live,
    Terminals,
    Links,
    Archived,
}

pub(crate) fn session_group(row: &SessionRow) -> SessionGroup {
    match row {
        SessionRow::Agent(a) if a.archived => SessionGroup::Archived,
        SessionRow::Agent(_) => SessionGroup::Live,
        SessionRow::Terminal(_) => SessionGroup::Terminals,
        SessionRow::Link(_) => SessionGroup::Links,
    }
}

pub(crate) fn selection_snapshot(app: &App) -> SelectionSnapshot {
    let row = app.selected_session_row();
    SelectionSnapshot {
        project: app.selected_project().map(|p| p.id.clone()),
        worktree: app.selected_worktree().map(|w| w.id.clone()),
        pr: app.selected_worktree_pr().map(|pr| pr.url.clone()),
        issue: app.selected_worktree_issue().map(|i| i.url.clone()),
        session_index: app.sel_session,
        session_group: row.as_ref().map(session_group),
        session: row.and_then(|r| r.sref()),
    }
}

/// Re-point the panel cursors after the tree changed. Each cursor follows
/// the entity it was on when rows merely shifted; when that entity left its
/// list — deleted, archived away, re-homed — the cursor has landed on a
/// neighbor, and that neighbor gets shown exactly as if the user had moved
/// there (restore_context / restore_session / preview). The invariant: the
/// terminal pane always shows the highlighted session, never a stale or
/// blank one.
/// Re-seat the selection after the tree changed under it, then attach at
/// once. A delete, an archive or a move is an explicit act and the row the
/// cursor gets pushed onto is where it stays — there is no sweep to wait
/// out, unlike the key-walking that `attach`'s debounce exists for.
pub(crate) fn reconcile_selection(
    app: &mut App,
    before: SelectionSnapshot,
    out: &mut Vec<ClientRequest>,
) {
    reconcile_selection_inner(app, before, out);
    fire_pending_attach(app, out);
}

pub(crate) fn reconcile_selection_inner(
    app: &mut App,
    before: SelectionSnapshot,
    out: &mut Vec<ClientRequest>,
) {
    clamp_selections(app);
    if let Some(pid) = &before.project {
        if !app.tree.projects.iter().any(|p| &p.id == pid) {
            // The selected row's project is gone; the cursor landed on a
            // neighbor — bring up its remembered worktree + session, and
            // the band it was left with open.
            app.launcher_open_bands.remove(pid);
            app.launcher_expanded = None;
            carry_open_band(app, None);
            restore_context(app, out);
            return;
        }
        if app.selected_project().map(|p| p.id.clone()).as_ref() != Some(pid) {
            let rows = app.project_rows();
            let found = rows.iter().position(|i| &app.tree.projects[*i].id == pid);
            if let Some(i) = found {
                app.sel_project = i;
            }
        }
    }
    if let Some(wid) = &before.worktree {
        if app.selected_worktree().map(|w| w.id.clone()).as_ref() != Some(wid) {
            match app.worktree_row_of(wid) {
                Some(i) => app.sel_worktree = i,
                None => {
                    restore_session(app, out);
                    return;
                }
            }
        }
    } else if let Some(url) = &before.pr {
        // A pull request that is still listed keeps the cursor; one that
        // left is followed nowhere, and the cursor stays where it landed.
        if app.selected_worktree_pr().map(|pr| &pr.url) != Some(url) {
            if let Some(i) = app.open_pr_row_of(url) {
                app.sel_worktree = i;
            }
        }
    } else if let Some(url) = &before.issue {
        if app.selected_worktree_issue().map(|i| &i.url) != Some(url) {
            if let Some(i) = app.issue_row_of(url) {
                app.sel_worktree = i;
            }
        }
    }
    if let Some(sref) = &before.session {
        let rows = app.visible_session_rows();
        if rows.get(app.sel_session).and_then(|r| r.sref()).as_ref() != Some(sref) {
            let found = rows.iter().position(|r| {
                r.sref().as_ref() == Some(sref)
                    && (before.session_group == Some(SessionGroup::Archived)
                        || !r.is_archived_agent())
            });
            match found {
                Some(i) => app.sel_session = i,
                None => {
                    // The row below slid up into the cursor's slot, so
                    // that is the neighbor it lands on: archive the top
                    // agent and the cursor moves down. But the last row of
                    // a group has no row below it in the group — the slot
                    // now holds the next group's first row, a terminal
                    // under the last live agent — so the cursor steps up
                    // onto the row above instead, while one is left in the
                    // group. A cursor the list's shrinking already pulled
                    // up (the removed row was the very last) stays put.
                    let group_at = |i: usize| rows.get(i).map(session_group);
                    if app.sel_session == before.session_index
                        && app.sel_session > 0
                        && group_at(app.sel_session) != before.session_group
                        && group_at(app.sel_session - 1) == before.session_group
                    {
                        app.sel_session -= 1;
                    }
                    preview_selected(app, out);
                    // Nothing previewable left (empty list, or only archived
                    // rows): don't keep showing a session that's gone.
                    if let Some(tref) = app.term.as_ref().map(|t| t.sref.clone()) {
                        let alive = match &tref {
                            SessionRef::Agent(id) => app.tree.agents.iter().any(|a| &a.id == id),
                            SessionRef::Terminal(id) => {
                                app.tree.terminals.iter().any(|t| &t.id == id)
                            }
                        };
                        if !alive {
                            detach_if_attached(app, &tref, out);
                        }
                    }
                }
            }
        }
    }
}

/// Keep selections valid after the tree shrinks.
pub(crate) fn clamp_selections(app: &mut App) {
    let project_rows = app.project_rows().len();
    app.sel_project = clamp_selection(app.sel_project as i64, project_rows);
    let wt_len = app.worktree_row_count();
    app.sel_worktree = clamp_selection(app.sel_worktree as i64, wt_len);
    let sess_len = app.visible_session_rows().len();
    app.sel_session = clamp_selection(app.sel_session as i64, sess_len);
}
