//! The panel walk: focus moving across the columns — Tab / ⇧Tab and
//! ^⇧L / ^⇧H one panel at a time, `h`/`l` (←/→) as their vim twins —
//! and the double tap that jumps a walk edge: `l`,`l` at Sessions into the
//! pane, `h`,`h` or `k`,`k` up into the Workspaces bar, `j`,`j` back down
//! out of it. `event_loop.rs` dispatches the keys; this module decides
//! where focus lands. The state it drives is `App::focus`, `App::edge_tap`,
//! `App::bar_return` and the pane's input lock.

use super::fire_pending_attach;
use crate::app::{App, Focus};
use nebula_core::ClientRequest;
use std::time::Duration;

/// Cross into the terminal pane and take the input lock, so what the user
/// types after the walk reaches the agent instead of the panels. An empty
/// or dead pane is focused but never locked: there is nothing to type into,
/// and a lock would only send them hunting for an escape hatch. Taking the
/// lock is a commitment to this session, so a debounced attach stops
/// waiting: keystrokes are about to need it.
pub(super) fn enter_terminal_pane(app: &mut App, out: &mut Vec<ClientRequest>) {
    app.focus = Focus::Terminal;
    if app.term.as_ref().is_some_and(|t| !t.exited) {
        app.term_locked = true;
        fire_pending_attach(app, out);
    }
}

/// Two presses of the same edge key this close together are one gesture.
/// Matches `DOUBLE_CLICK`: the row's double-click is the same "again,
/// deliberately" and the two shouldn't feel different.
pub(super) const DOUBLE_TAP: Duration = Duration::from_millis(400);

/// `h`/`l` (or ←/→) has landed on the end of the panel row. The first
/// press arms and stays put, telling the user in the footer what a second
/// one does; a second press of the same action inside `DOUBLE_TAP` — with
/// nothing else in between, see the `take()` in `handle_key` — reports
/// `true` so the caller can jump the boundary. A slow second press re-arms
/// rather than jumping: the gap says it was two single presses.
pub(super) fn double_tapped(
    app: &mut App,
    action: crate::keymap::Action,
    armed: Option<(crate::keymap::Action, std::time::Instant)>,
    chord: &crate::keymap::KeyChord,
    does: &str,
) -> bool {
    let now = std::time::Instant::now();
    if armed.is_some_and(|(a, at)| a == action && now.duration_since(at) <= DOUBLE_TAP) {
        return true;
    }
    app.edge_tap = Some((action, now));
    app.flash = Some(format!("{} again: {does}", chord.display()));
    false
}

/// The forward panel walk — Tab / ^⇧L, and l/→ (double-tapped at the
/// end) — one visible column right (a hidden Projects or Worktrees panel
/// is skipped), stopping dead at the terminal pane so leaning on the key
/// can't spill past it and back round to the Workspaces bar. Landing on
/// the pane takes the input lock: walking that far means the user is
/// going to type at the agent, and the preview under the Sessions cursor
/// is already the session they picked.
pub(super) fn walk_focus_forward(app: &mut App, out: &mut Vec<ClientRequest>) {
    match app.next_visible_focus(app.focus) {
        Focus::Terminal => enter_terminal_pane(app, out),
        next => app.focus = next,
    }
}

/// The backward panel walk — ⇧Tab / ^⇧H, and h/← (double-tapped at the
/// end) — one visible column left, stopping dead at the first stop: the
/// Workspaces bar while it's shown, otherwise the first visible sidebar.
/// Never wraps into the pane: ^⇧H is also the unlock hatch out of a locked
/// pane, so a wrap made the key cycle first column → pane → Sessions → …
/// forever, with nothing to stop against. Forward is the way into the
/// pane, and Ctrl+→ crosses into it without taking the input lock.
pub(super) fn walk_focus_back(app: &mut App) {
    match app.previous_visible_focus(app.focus) {
        Focus::Workspaces => enter_workspaces_bar(app),
        prev => app.focus = prev,
    }
}

/// Step up into the Workspaces bar — the walk back, h,h / k,k, or a click
/// on a tab — remembering the panel the cursor came from so j,j in the bar
/// can drop back onto it. The terminal pane is not a panel under the bar:
/// coming from there, the way back lands on Sessions, the column whose
/// cursor the pane previews. Already in the bar, the memory stands.
pub(super) fn enter_workspaces_bar(app: &mut App) {
    app.bar_return = match app.focus {
        Focus::Workspaces => app.bar_return,
        Focus::Terminal => Focus::Sessions,
        panel => panel,
    };
    app.focus = Focus::Workspaces;
}

/// Where j,j out of the Workspaces bar lands: the panel focus came up from
/// (Projects until it has ever come up) — unless that panel has been hidden
/// since (⇧P / ⇧B), in which case the first visible sidebar stands in, the
/// way Enter in the bar does. A hidden panel can't own focus.
pub(super) fn bar_return_target(app: &App) -> Focus {
    if app.focus_visible(app.bar_return) {
        app.bar_return
    } else {
        app.first_sidebar_focus()
    }
}

/// j,j in the Workspaces bar: back down onto `bar_return_target`. The
/// cursor there is untouched — the row it was on is the row it lands on.
pub(super) fn leave_workspaces_bar(app: &mut App) {
    app.focus = bar_return_target(app);
}

/// Whether the focused panel's cursor sits on its first row — the top
/// edge, where k/↑ has nowhere left to go and a double tap steps up into
/// the Workspaces bar instead. An empty panel counts: its cursor is at
/// row 0 with nothing above or below.
pub(super) fn at_top_row(app: &App) -> bool {
    match app.focus {
        Focus::Projects => app.sel_project == 0,
        Focus::Worktrees => app.sel_worktree == 0,
        Focus::Sessions => app.sel_session == 0,
        Focus::Workspaces | Focus::Terminal => false,
    }
}

/// The panel's name as the footer flash says it: "j again: back to
/// sessions".
pub(super) fn panel_name(focus: Focus) -> &'static str {
    match focus {
        Focus::Workspaces => "workspaces",
        Focus::Projects => "projects",
        Focus::Worktrees => "worktrees",
        Focus::Sessions => "sessions",
        Focus::Terminal => "terminal",
    }
}
