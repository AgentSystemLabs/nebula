//! The panel walk: focus moving across the columns — Tab / ⇧Tab and
//! ^⇧L / ^⇧H one panel at a time, `h`/`l` (←/→) as their vim twins —
//! and the double tap that jumps a walk edge: `l`,`l` at Sessions into the
//! pane. `event_loop.rs` dispatches the keys; this module decides where
//! focus lands. The state it drives is `App::focus`, `App::edge_tap` and
//! the pane's input lock.

use super::fire_pending_attach;
use crate::app::{App, Focus, HitTarget};
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
        // Both presses as one combo on the KEY COMBO DISPLAY, over the
        // single key `handle_key` noted a moment ago.
        crate::key_combo::note_double_tap(app, chord, does);
        return true;
    }
    app.edge_tap = Some((action, now));
    app.flash = Some(format!("{} again: {does}", chord.display()));
    false
}

/// The forward panel walk — Tab / ^⇧L, and l/→ (double-tapped at the
/// end) — one visible column right (a hidden Projects or Worktrees panel
/// is skipped), stopping dead at the terminal pane so leaning on the key
/// can't spill past it and back round to the first column. Landing on
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
/// end) — one visible column left, stopping dead at the first visible
/// sidebar. Never wraps into the pane: ^⇧H is also the unlock hatch out of a locked
/// pane, so a wrap made the key cycle first column → pane → Sessions → …
/// forever, with nothing to stop against. Forward is the way into the
/// pane, and Ctrl+→ crosses into it without taking the input lock.
pub(super) fn walk_focus_back(app: &mut App) {
    app.focus = app.previous_visible_focus(app.focus);
}

/// Where the click that dismissed a modal lands: on the focus of whatever
/// the pointer was over, and nothing else. The user aimed that click at a
/// panel, not at the modal's margin, so the panel takes focus as a click
/// on it would — but the click itself was spent closing the modal: it
/// moves no cursor, previews no session, switches no project and never
/// opens a prompt, or dismissing a modal would be the one click in nebula
/// that acts on a row the user could not see it land on. The pane is the
/// exception that the walk already makes: entering it is a commitment to
/// type at the agent, so it takes the input lock the way the click and Tab
/// both do. The LAUNCHER VIEW's pane edge and its header's tabs are
/// buttons: none of them is somewhere focus lives.
pub(super) fn land_click_focus(app: &mut App, column: u16, row: u16, out: &mut Vec<ClientRequest>) {
    match app.hit_at(column, row) {
        Some(
            HitTarget::LauncherCard(_)
            | HitTarget::LauncherBand(_)
            | HitTarget::LauncherBandPr(_)
            | HitTarget::LauncherCardIssue(_)
            | HitTarget::LauncherStripLeft(_)
            | HitTarget::LauncherStripRight(_)
            | HitTarget::LauncherBandMore(_),
        ) => app.focus = Focus::Sessions,
        Some(HitTarget::PanelBg(focus)) => app.focus = focus,
        Some(HitTarget::TerminalPane | HitTarget::CloudSessionLink) => {
            enter_terminal_pane(app, out)
        }
        // The crumb is a button out of a full-screen session, not
        // somewhere focus lives: its own handler is what moves focus. So
        // are the PANE's own TAB STRIP tabs — a click on one says what
        // the pane reads, and typing into it is the separate commitment
        // the pane itself takes.
        Some(
            HitTarget::LauncherPaneSplitter
            | HitTarget::LauncherCrumb
            | HitTarget::LauncherTab(_)
            | HitTarget::LauncherTabClose(_)
            | HitTarget::LauncherTabAdd
            | HitTarget::LauncherPaneClose
            | HitTarget::LauncherPaneSide
            | HitTarget::LauncherPaneZoom
            | HitTarget::LauncherPullRequests
            | HitTarget::LauncherIssues
            | HitTarget::LauncherWelcomePrompt
            | HitTarget::FooterUsage
            | HitTarget::ModalBrowser,
        )
        | None => {}
    }
}
