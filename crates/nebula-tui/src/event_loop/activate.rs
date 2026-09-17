//! What a row does once it is chosen — INPUT IS NOT ACTION.
//!
//! A row can be chosen three ways: Enter with the cursor on it, a click on
//! it, a second click on the row the cursor is already on. The three used
//! to be three implementations, each written where its event is read — the
//! key arm in `handle_overlay_key`, the click arm in `handle_mouse` — and
//! every feature added to one of them was a bug in the others until someone
//! noticed: Enter on a markdown file in the file finder opened the FILE
//! TABS reader while a click on the same row opened the raw editor; Enter
//! in the PR SESSION preset picker launched the PR SESSION while a click
//! launched a plain session into the ROOT WORKTREE.
//!
//! So the handlers only translate. A key arm and a click arm each say
//! *which row* (`list_hit::row_at` for the pointer) and then call the one
//! function here that says *what choosing it does*. Anything that closes a
//! modal, sends a request, spawns a process or moves a cursor as the result
//! of choosing a row belongs in this file (or, for a modal that lives in a
//! module of its own, in that module's single `activate_selected` / `Cmd`
//! executor — `preset_overlays`, `file_tabs`, `issues`, `branch_switch`),
//! never inline in an input handler. `handle_mouse` holding an action of
//! its own is the smell to look for in review.

use super::{
    attach_now, jump_to_target, open_link, open_session, run_menu_action, Landing, SettingsCmd,
    WORKTREE_STILL_CREATING,
};
use crate::app::{App, ConfirmDialog, DiffView, Focus, Overlay, PendingAction};
use nebula_core::{AgentId, ClientRequest, SessionRef, WorktreeId};

/// A CONTEXT MENU row — Enter on the hovered row, a click on any: the menu
/// goes and the row's action runs. A row that is not there (a click on a
/// blank line) does nothing.
pub(super) fn menu_row(app: &mut App, index: usize, out: &mut Vec<ClientRequest>) {
    let Some(Overlay::Menu(menu)) = &app.overlay else {
        return;
    };
    let Some(action) = menu.items.get(index).map(|item| item.action.clone()) else {
        return;
    };
    app.overlay = None;
    run_menu_action(app, action, out);
}

/// The PALETTE's selected row — Enter, a click, `Ctrl+O`, `Ctrl+F`: the
/// palette goes and the panels land on the row's target. `landing` is how:
/// None is Enter's own rule (the `palette_enter_attaches` SETTING, or the
/// browser for a pull request), which a click follows too; the two chords
/// name theirs.
pub(super) fn palette_row(app: &mut App, landing: Option<Landing>, out: &mut Vec<ClientRequest>) {
    let Some(Overlay::Palette(palette)) = &app.overlay else {
        return;
    };
    let Some(target) = palette.selected_target().cloned() else {
        return;
    };
    let landing = landing.unwrap_or_else(|| Landing::for_enter_on(&target, palette.enter_attaches));
    app.overlay = None;
    jump_to_target(app, target, landing, out);
}

/// A destination of the HOSTS PICKER — Enter on a row, a click on one,
/// Enter on a typed destination: nebula leaves this machine's UI for a
/// fresh `nebula ssh` at it (the daemon and its sessions stay up).
pub(super) fn host(app: &mut App, entry: crate::hosts::HostEntry) {
    app.overlay = None;
    app.pending_ssh = Some(entry);
    app.should_quit = true;
}

/// The METRICS modal's selected row — Enter, or a click on the row the
/// cursor is already on: the modal goes and the panels land on that
/// session. Nebula's own rows (the daemon, this UI) carry no session and
/// do nothing.
pub(super) fn metrics_row(app: &mut App, out: &mut Vec<ClientRequest>) {
    let Some(Overlay::Metrics(view)) = &app.overlay else {
        return;
    };
    let Some(Some(sref)) = view.rows.get(view.selected).cloned() else {
        return;
    };
    app.overlay = None;
    open_session(app, sref, out);
}

/// Move the DIFF modal's file cursor to `index` (clamped) and read that
/// file's diff when the cursor actually moved — ↑/↓ and a click on a file
/// row alike.
pub(super) fn diff_file(view: &mut DiffView, index: i64) {
    if view.select(index) {
        crate::git_diff::load_selected_diff(view);
    }
}

/// A row of the DIFF modal's list chosen — a click on it, or Enter on the
/// cursor's own: the cursor lands there, and a tree directory's row folds
/// or unfolds as well. On a file's row that is all there is to choose, so
/// in the flat list this is `diff_file`.
pub(super) fn diff_row(view: &mut DiffView, index: i64) {
    let moved = view.select(index);
    if view.toggle_dir(view.cursor()) || moved {
        crate::git_diff::load_selected_diff(view);
    }
}

/// `→` / `←` in the DIFF modal's tree: open or fold the directory under the
/// cursor, stepping into an open one or out to the parent's row, and read
/// whatever the cursor came to rest on.
pub(super) fn diff_tree_step(view: &mut DiffView, inward: bool) {
    let moved = if inward {
        view.expand_selected()
    } else {
        view.collapse_selected()
    };
    if moved {
        crate::git_diff::load_selected_diff(view);
    }
}

/// `Ctrl+t` in the DIFF modal: the file list's other shape. The cursor
/// keeps its file, and its place in the diff with it; only a cursor that
/// had to move — off a directory's row, which the flat list has none of —
/// reads a diff.
pub(super) fn diff_tree_toggled(view: &mut DiffView) {
    if view.toggle_tree() {
        crate::git_diff::load_selected_diff(view);
    }
}

/// The DIFF modal's filter text changed — typed, pasted, or cleared by
/// Esc: the file list narrows, and when that moved the cursor onto another
/// file its diff is read.
pub(super) fn diff_filter_changed(view: &mut DiffView) {
    if view.apply_filter() {
        crate::git_diff::load_selected_diff(view);
    }
}

/// What Enter means on the SETTINGS OVERLAY's selected row, and so what a
/// second click on it means: a hotkey row starts a rebind, any other
/// applies the setting (toggles it, opens its box, cycles it in place).
pub(super) fn settings_row_cmd(hotkeys: bool, selected: usize) -> SettingsCmd {
    if hotkeys {
        SettingsCmd::Capture { add: false }
    } else {
        SettingsCmd::Apply(selected, 0)
    }
}

/// Enter on the WORKTREES PANEL's row — and a double-click on it: a pull
/// request leads out of nebula, so it is handed to the browser and the
/// cursor stays put; a checkout hands FOCUS one column right, to its
/// sessions.
pub(super) fn worktrees_row(app: &mut App, out: &mut Vec<ClientRequest>) {
    match app.selected_worktree_pr().map(|pr| pr.url.clone()) {
        Some(url) => open_link(app, &url, out),
        None => app.focus = Focus::Sessions,
    }
}

/// The CLOUD SESSION PANEL's link — Enter with the pane focused, a click on
/// the URL: the session's page opens in the browser. False when the pane
/// is not showing a cloud session, so Enter can mean the pane's own thing.
pub(super) fn cloud_link(app: &mut App, out: &mut Vec<ClientRequest>) -> bool {
    let Some(url) = app.previewed_cloud().map(|cloud| cloud.url.clone()) else {
        return false;
    };
    open_link(app, &url, out);
    true
}

/// Attach `sref` and step into it — Enter on a session row, a double-click
/// on one, **Attach** in its CONTEXT MENU: the pane shows the session, takes
/// FOCUS and the input lock.
pub(super) fn attach(app: &mut App, sref: SessionRef, out: &mut Vec<ClientRequest>) {
    attach_now(app, sref, out);
    app.focus = Focus::Terminal;
    app.term_locked = true;
}

/// Bring an archived agent back — `u` on its row, **Unarchive** in its
/// CONTEXT MENU.
pub(super) fn unarchive(app: &mut App, id: AgentId, out: &mut Vec<ClientRequest>) {
    super::optimistic::set_archived(app, id, false, out);
}

/// Ask before a checkout is deleted from disk — `d` on its row, **Delete
/// worktree** in its CONTEXT MENU. One gate and one wording for both: the
/// ROOT WORKTREE is never deleted, a stand-in git is still cutting has
/// nothing on disk yet, and the confirm says how many live sessions go
/// down with the checkout. The menu used to build a confirm of its own,
/// which left that warning out.
pub(super) fn delete_worktree(app: &mut App, id: &WorktreeId) {
    let Some(w) = app.tree.worktrees.iter().find(|w| &w.id == id) else {
        return;
    };
    if w.is_main {
        app.flash = Some("cannot delete the main checkout".into());
        return;
    }
    if app.is_placeholder_worktree(id) {
        app.flash = Some(WORKTREE_STILL_CREATING.into());
        return;
    }
    let live_here = app
        .tree
        .agents
        .iter()
        .filter(|a| &a.worktree_id == id && !a.archived)
        .count()
        + app
            .tree
            .terminals
            .iter()
            .filter(|t| &t.worktree_id == id)
            .count();
    app.overlay = Some(Overlay::Confirm(ConfirmDialog {
        title: "Delete worktree".into(),
        message: format!(
            "Delete worktree '{}' from disk? {live_here} session(s) will be killed.",
            w.branch
        ),
        action: PendingAction::DeleteWorktree(id.clone()),
        area: ratatui::layout::Rect::default(),
    }));
}
