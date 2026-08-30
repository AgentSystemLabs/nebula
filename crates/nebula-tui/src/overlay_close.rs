//! The three ways out of an OVERLAY, in one place.
//!
//! Every modal answers to Esc, to a CLICK OUTSIDE and to the HARDWIRED
//! UNLOCK. The Esc arms stay beside the rest of each overlay's keys, because
//! several of them are deliberately two-stage — the first press peels a
//! typed filter or an open submenu and only the second closes. The other two
//! exits live here: both need the same two facts about all fourteen
//! variants, the box the overlay was last drawn in and what has to be put
//! back when it goes, and spelling either of those out per variant is how
//! the list drifts.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::{Position, Rect};

use nebula_core::protocol::ClientRequest;

use crate::app::{App, Overlay};

/// The box `overlay` was last drawn in.
///
/// `ui::draw_overlay` matches on a *clone* of `app.overlay`, so each arm
/// writes its rect back into the live overlay when it is done; a variant
/// that has not been drawn yet still holds the zero rect it was built with.
pub(crate) fn overlay_area(overlay: &Overlay) -> Rect {
    match overlay {
        Overlay::Menu(v) => v.area,
        Overlay::Confirm(v) => v.area,
        Overlay::Prompt(v) => v.area,
        Overlay::Help(v) => v.area,
        Overlay::Settings(v) => v.area,
        Overlay::Diff(v) => v.area,
        Overlay::Palette(v) => v.area,
        Overlay::Files(v) => v.area,
        Overlay::Grep(v) => v.area,
        Overlay::Tree(v) => v.area,
        Overlay::Metrics(v) => v.area,
        Overlay::Hosts(v) => v.area,
        Overlay::AgentPresets(v) => v.area,
        Overlay::AgentPresetEditor(v) => v.area,
    }
}

/// True when a left-click at `pos` landed outside the open overlay.
///
/// A zero-width box has never reached the screen, and without the guard
/// every click would count as outside it — which is what lets a test press
/// keys into a modal it never drew.
pub(crate) fn click_is_outside(overlay: &Overlay, pos: Position) -> bool {
    let area = overlay_area(overlay);
    area.width > 0 && !area.contains(pos)
}

/// Dismiss the open overlay after a click landed outside it — "exactly as
/// Esc would", which for half of them means running Esc itself: a CONFIRM
/// DIALOG cancels and lands back in the SETTINGS OVERLAY or WORKSPACE
/// SWITCHER it came from, a PROMPT DIALOG restores the warm slot's spec, the
/// AGENT PRESETS list in QUICK PROMPT picker mode hands the box back with
/// its text, and the PRESET EDITOR backs out to its list.
pub(crate) fn click_outside(app: &mut App, out: &mut Vec<ClientRequest>) {
    match &app.overlay {
        None => {}
        // Closing the SETTINGS OVERLAY stamps the row to reopen on, however
        // it closes.
        Some(Overlay::Settings(_)) => crate::event_loop::close_settings(app),
        // A menu clicked away from goes outright rather than backing out one
        // submenu level at a time — but a picker opened from the QUICK
        // PROMPT still owes that box back.
        Some(Overlay::Menu(_)) => close_menu(app),
        // Nothing to unwind on the way out.
        Some(
            Overlay::Help(_)
            | Overlay::Diff(_)
            | Overlay::Palette(_)
            | Overlay::Files(_)
            | Overlay::Grep(_)
            | Overlay::Tree(_)
            | Overlay::Metrics(_)
            | Overlay::Hosts(_),
        ) => app.overlay = None,
        // Confirm, Prompt, the AGENT PRESETS list and the PRESET EDITOR each
        // have a side effect on the way out that their own Esc already
        // spells out; none of the four stages it.
        Some(_) => crate::event_loop::handle_overlay_key(app, esc(), out),
    }
}

/// The HARDWIRED UNLOCK pressed with an overlay open: whatever is on screen
/// goes, in one press, from any state — a typed filter, an open submenu, a
/// live HOTKEY CAPTURE, the PRESET EDITOR nested over its list. Returns
/// false when there was no overlay to close.
///
/// This is the one exit that never stages and never hands anything back: its
/// whole contract is that a rebind, a nested modal or a half-typed field can
/// never trap the user, so it lands on the panels every time. Only the
/// cleanup that would otherwise leave the daemon or the config wrong still
/// runs.
pub(crate) fn force_close(app: &mut App, out: &mut Vec<ClientRequest>) -> bool {
    let Some(overlay) = &app.overlay else {
        return false;
    };
    match overlay {
        Overlay::Settings(_) => crate::event_loop::close_settings(app),
        // Abandoning a Claude name prompt can leave the warm slot holding
        // the submenu's off-default spec (its prewarm fired on kind-pick);
        // put the standing default spec back, as its Esc does.
        Overlay::Prompt(prompt) => {
            let restore = crate::event_loop::abandoned_prompt_prewarm(&prompt.kind);
            app.overlay = None;
            out.extend(restore);
        }
        _ => app.overlay = None,
    }
    true
}

/// Close a CONTEXT MENU from any depth, handing the QUICK PROMPT its box
/// back when the picker was opened from one.
fn close_menu(app: &mut App) {
    let Some(Overlay::Menu(menu)) = &mut app.overlay else {
        return;
    };
    // The return trip is pinned to the root picker's rows, so walk up first.
    while let Some(parent) = menu.parent.take() {
        *menu = *parent;
    }
    let back = crate::event_loop::menu_quick_return(menu);
    app.overlay = None;
    if let Some(back) = back {
        crate::quick_prompt::reopen(app, back.launch, &back.text);
    }
}

fn esc() -> KeyEvent {
    KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)
}
