//! Extracted event-loop helper section.

use super::*;

pub(crate) fn send(
    app: &mut App,
    out: &mut Vec<ClientRequest>,
    make: impl FnOnce(u64) -> ClientRequest,
) {
    send_with(app, out, PendingIntent::None, make);
}

/// Queue a request built around a fresh `req_id`, remembering `intent` to
/// run when the Ack (or Error) for it arrives.
pub(crate) fn send_with(
    app: &mut App,
    out: &mut Vec<ClientRequest>,
    intent: PendingIntent,
    make: impl FnOnce(u64) -> ClientRequest,
) {
    send_with_follow(app, out, intent, true, make);
}

/// [`send_with`], with the Ack born LEFT BEHIND when `follow` is false:
/// the intent still runs, but nothing it does moves a cursor, the pane or
/// FOCUS onto what it made. That is how a BACKGROUND LAUNCH sends its
/// first half — the checkout it cuts for a prompt fired into another
/// project — so the screen stays where the user left it, the same way a
/// launch the user navigated away from is treated (`App::left_behind`).
pub(crate) fn send_with_follow(
    app: &mut App,
    out: &mut Vec<ClientRequest>,
    intent: PendingIntent,
    follow: bool,
    make: impl FnOnce(u64) -> ClientRequest,
) {
    let req_id = app.alloc_req_id(intent);
    if !follow {
        app.left_behind.insert(req_id);
    }
    out.push(make(req_id));
}

pub(crate) fn log_server_event(ev: &ServerEvent) {
    match ev {
        ServerEvent::Output { .. } | ServerEvent::Scrollback { .. } => {}
        other => tracing::debug!(event = ?other, "server event"),
    }
}

pub(crate) fn ui_state_json(app: &App) -> String {
    use crate::app::UiState;
    let state = UiState {
        project: app.selected_project().map(|p| p.id.to_string()),
        worktree: app.selected_worktree().map(|w| w.id.to_string()),
        session_agent: app.selected_session().map(|a| a.id.to_string()),
        show_archived: app.show_archived,
        collapsed: app.collapsed,
        open_prs_collapsed: app.open_prs_collapsed,
        issues_collapsed: app.issues_collapsed,
        diff_files_width: Some(app.diff_files_width),
        diff_tree: app.diff_tree,
        launcher_pane_h: app.launcher_pane_h,
        launcher_pane_w: app.launcher_pane_w,
        launcher_pane_hidden: app.launcher_pane_hidden,
        launcher_expanded: app.launcher_expanded.as_ref().map(|w| w.to_string()),
        launcher_open_bands: saved_open_bands(app),
        launcher_folded: saved_folded_bands(app),
        launcher_thread_open: saved_open_threads(app),
        launcher_tabs: app.launcher_tabs.iter().map(|id| id.to_string()).collect(),
        projects_closed: app.projects_closed,
    };
    serde_json::to_string(&state).unwrap_or_else(|_| "{}".into())
}

/// The band every project was left with open, for the blob: the other
/// projects' ([`App::launcher_open_bands`]) with the one on screen filed
/// under its own project, less any project or checkout the tree no
/// longer has.
pub(crate) fn saved_open_bands(app: &App) -> std::collections::HashMap<String, String> {
    let mut bands = app.launcher_open_bands.clone();
    if let Some(pid) = app.selected_project().map(|p| p.id.clone()) {
        match &app.launcher_expanded {
            Some(open) => bands.insert(pid, open.clone()),
            None => bands.remove(&pid),
        };
    }
    bands
        .into_iter()
        .filter(|(pid, wid)| {
            app.tree
                .worktrees
                .iter()
                .any(|w| &w.id == wid && &w.project_id == pid)
        })
        .map(|(pid, wid)| (pid.to_string(), wid.to_string()))
        .collect()
}

/// The worktrees the NESTED layout has folded ([`App::launcher_folded`]),
/// for the blob: less any the tree no longer has, in a stable order so
/// the blob only changes when the set does.
pub(crate) fn saved_folded_bands(app: &App) -> Vec<String> {
    saved_worktree_set(&app.launcher_folded, app)
}

/// The worktrees the NESTED layout has expanded
/// ([`App::launcher_thread_open`]), for the blob, on the same terms as
/// [`saved_folded_bands`].
pub(crate) fn saved_open_threads(app: &App) -> Vec<String> {
    saved_worktree_set(&app.launcher_thread_open, app)
}

pub(crate) fn saved_worktree_set(
    ids: &std::collections::HashSet<WorktreeId>,
    app: &App,
) -> Vec<String> {
    let mut out: Vec<String> = ids
        .iter()
        .filter(|wid| app.tree.worktrees.iter().any(|w| &w.id == *wid))
        .map(|wid| wid.to_string())
        .collect();
    out.sort();
    out
}

/// Re-seat the cursor from the persisted blob. Returns whether the
/// remembered session landed under it, so the caller can bring its pane
/// back too — the selection alone is a cursor on a blank screen.
pub(crate) fn restore_ui_state(app: &mut App, json: &str) -> bool {
    use crate::app::UiState;
    let Ok(state) = serde_json::from_str::<UiState>(json) else {
        return false;
    };
    app.show_archived = state.show_archived;
    app.open_prs_collapsed = state.open_prs_collapsed;
    app.issues_collapsed = state.issues_collapsed;
    if let Some(w) = state.diff_files_width {
        // The draw re-caps it to the actual modal width.
        app.diff_files_width = w.clamp(crate::app::MIN_DIFF_FILES_W, MAX_RESTORED_WIDTH);
    }
    app.diff_tree = state.diff_tree;
    // The next draw re-fits it to the body actually on screen
    // (`launcher::pane_height`); the cap here only keeps a nonsense blob
    // from carrying a wild number around.
    app.launcher_pane_h = state
        .launcher_pane_h
        .map(|h| h.clamp(crate::launcher::PANE_MIN_H, MAX_RESTORED_WIDTH));
    app.launcher_pane_w = state
        .launcher_pane_w
        .map(|w| w.clamp(crate::launcher::PANE_MIN_W, MAX_RESTORED_WIDTH));
    // A pane folded away with `^~` stays folded across a restart, as its
    // height does. Nothing is unselected on the way back in: the restore
    // lands on the cards either way.
    app.launcher_pane_hidden = state.launcher_pane_hidden;
    app.launcher_expanded = state.launcher_expanded.map(nebula_core::WorktreeId::from);
    app.launcher_folded = state.launcher_folded.into_iter().map(WorktreeId).collect();
    app.launcher_thread_open = state
        .launcher_thread_open
        .into_iter()
        .map(WorktreeId)
        .collect();
    // Every other project's open band too; the restored project's own is
    // `launcher_expanded` above, which the next switch away files here.
    app.launcher_open_bands = state
        .launcher_open_bands
        .into_iter()
        .filter(|(pid, _)| state.project.as_ref() != Some(pid))
        .map(|(pid, wid)| (ProjectId(pid), WorktreeId(wid)))
        .collect();
    // The PROJECT TABS come back in the order they were left, less any
    // project the tree no longer has; the draw's settle gives the
    // restored project its tab if it had none.
    app.launcher_tabs = state
        .launcher_tabs
        .into_iter()
        .map(ProjectId)
        .filter(|id| app.tree.projects.iter().any(|p| &p.id == id))
        .collect();
    app.projects_closed = state.projects_closed && app.launcher_tabs.is_empty();
    if let Some(pid) = &state.project {
        let row = app
            .project_rows()
            .iter()
            .position(|i| app.tree.projects[*i].id.as_str() == pid);
        if let Some(i) = row {
            app.sel_project = i;
        }
    }
    if let Some(wid) = &state.worktree {
        if let Some(i) = app.worktree_row_of(&WorktreeId(wid.clone())) {
            app.sel_worktree = i;
        }
    }
    let mut session_landed = false;
    if let Some(sid) = state.session_agent {
        if let Some(i) = app
            .visible_session_rows()
            .iter()
            .position(|r| matches!(r, SessionRow::Agent(a) if a.id.as_str() == sid))
        {
            app.sel_session = i;
            session_landed = true;
        }
    }
    session_landed
}
