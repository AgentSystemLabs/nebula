//! The LAUNCHER VIEW's keys and clicks (`crate::launcher` is the view's
//! model, `ui::launcher_view` its drawing). Every arm here translates and
//! calls one function per intent, as the panels' do: a card chosen by key
//! or by pointer lands through [`select`], a session is stepped into
//! through [`enter_pane`], and the
//! box opens through [`open_box`] — so `j` and a click on the card below,
//! or Enter and a double-click, end in the same state.

use super::{
    build_submenu, enter_terminal_pane, is_double_click, jump_to_target, jump_to_target_inner,
    open_prompt, restore_project_cursors, select_project_row_by_id, Landing,
};
use crate::app::{
    App, ConfirmDialog, ContextMenu, Focus, HitTarget, MenuAction, MenuFilter, MenuItem, Overlay,
    PendingAction, PromptKind, SessionRow,
};
use crate::keymap::{Action, KeyChord};
use crate::launcher::{self as view, BoxField, CardRef, ProjectPicker};
use crate::palette::PaletteTarget;
use crate::quick_prompt::{QuickLaunch, QuickReturn, QuickTarget};
use crate::text_input::TextInput;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use nebula_core::{AgentId, ClientRequest, ProjectId, SessionRef, WorktreeId};

/// Rows of cards `Ctrl+d` / `Ctrl+u` jump.
const HALF_PAGE: i64 = 2;

/// What `⇧A` says each way: which of the two lists the grid is now of.
const ARCHIVED_VIEW: &str = "archived sessions — u unarchives one, ⇧A back to the live ones";
const LIVE_VIEW: &str = "live sessions — ⇧A reads the archived ones";

/// What the grid says when there is nothing to step through yet — of
/// whichever of the two lists it is showing ([`toggle_archived`]).
const NO_SESSIONS: &str = "no sessions yet — p starts one";
const NO_ARCHIVED_SESSIONS: &str = "nothing archived here — ⇧A back to the live sessions";

/// Is the card under this id one of the ARCHIVED VIEW's? Its session was
/// reaped when it was archived, so nothing on it can be stepped into.
fn is_archived(app: &App, id: &AgentId) -> bool {
    app.tree.agents.iter().any(|a| &a.id == id && a.archived)
}

/// The one of those two this grid means.
fn nothing_here(app: &App) -> &'static str {
    if app.show_archived {
        NO_ARCHIVED_SESSIONS
    } else {
        NO_SESSIONS
    }
}

/// What the PROJECT DROPDOWN says with no project to list — never on
/// screen in practice: the view is only up once there is one.
const NO_PROJECTS: &str = "no projects yet — o opens a folder";

/// What folding the PANE away says, and what bringing it back says. The
/// first names what went with it: the card under the cursor is let go of
/// too, as an Esc lets it go.
const PANE_HIDDEN: &str = "pane hidden, nothing selected — ^` brings it back";
const PANE_SHOWN: &str = "pane back under the cards";

/// What the pane's SIDE BUTTON says once it has moved the pane.
const PANE_MOVED_RIGHT: &str = "pane moved beside the cards";
const PANE_MOVED_BOTTOM: &str = "pane moved under the cards";
/// What a double-click on the pane's edge says.
const PANE_CENTERED: &str = "pane edge centered";

/// What the fold key did as the first of its two presses from inside the
/// PANE ([`fold_key`]), for the KEY COMBO DISPLAY.
const BACK_TO_CARD: &str = "Back to the card";

/// What letting the card under the cursor go says: nothing in the GRID is
/// selected any more, and the PANE has no session to read.
pub(super) const UNAIMED: &str = "nothing selected — j/k or a click picks a card again";

/// The box: the QUICK PROMPT, aimed at the project under the list's cursor
/// (the selected project). It lands in the checkout under the grid's
/// cursor — the band the cursor is on, or the worktree the grid is inside
/// — on the project's ROOT BRANCH with the aim let go (Esc off the band),
/// or on a fresh worktree when the `quick_prompt_new_worktree` SETTING
/// says so (`view::target_for`); `^N` flips only the box that is up. `p`
/// opens it on the Settings → Agents harness; `n` asks which harness
/// first ([`open_new_session`]).
///
/// The worktree, not the card: a prompt sent with a worktree's band
/// selected starts a new session beside the ones already running in it —
/// more work in one session's own conversation is its FOLLOW-UP (Space
/// on the card) — so which of the worktree's cards the cursor last
/// rested on does not matter, only which worktree.
pub(super) fn open_box(app: &mut App) {
    if let Some(launch) = box_launch(app) {
        crate::quick_prompt::open_box(app, launch);
    }
}

/// `n`: the NEW SESSION PICKER first — which harness, `→` its model and
/// effort — and the box after it, set to the pick, in the checkout `p`
/// would take. The picker's rows carry the box they owe
/// (`QuickReturn::from_box` false: no box is up yet), so Enter on a row
/// OPENS the box rather than launching, and Esc closes the picker and
/// opens nothing. The cursor starts on the harness `p` would have used.
pub(super) fn open_new_session(app: &mut App) {
    let Some(launch) = box_launch(app) else {
        return;
    };
    // The checkout the rows are built against; the launch keeps its own
    // target either way (`quick_prompt::open_launch_picker` does the same).
    let Some(context) = crate::quick_prompt::picker_context(app, &launch) else {
        app.flash = Some("project no longer exists".into());
        return;
    };
    let back = QuickReturn {
        launch,
        text: String::new(),
        from_box: false,
    };
    crate::agent_picker::open_kind_picker(
        app,
        crate::agent_picker::KindPicker::new_session_box(context, back),
    );
}

/// The launch a box opened from the grid starts from: the Settings →
/// Agents harness, aimed as [`open_box`] says. None, with the flash set,
/// when there is no project to aim at.
fn box_launch(app: &mut App) -> Option<QuickLaunch> {
    let project = app.selected_project().map(|p| p.id.clone()).or_else(|| {
        app.project_rows()
            .first()
            .and_then(|i| app.tree.projects.get(*i))
            .map(|p| p.id.clone())
    });
    let Some(project) = project else {
        app.flash = Some("add a project first".into());
        return None;
    };
    let cfg = crate::config::Config::load();
    let target = view::target_for(app, &project, cfg.quick_prompt_new_worktree);
    Some(QuickLaunch::from_config(target, &cfg))
}

/// Put the cursor back on a card: anything that lands on one — a key that
/// walks the grid, a click on a card, a project opened — takes the aim
/// back, so the box reads a checkout again and the PANE along the bottom
/// opens on that card ([`App::launcher_split`]). Only the aim: the keys
/// stay on the cards, and crossing into the pane is a second click or
/// Enter ([`enter_pane`]).
pub(super) fn take_aim(app: &mut App) {
    // The card is wanted on screen, wheel or no wheel: the next frame
    // scrolls the grid to it (`App::launcher_reveal`).
    app.launcher_reveal = true;
    if app.launcher_unaimed {
        app.launcher_unaimed = false;
        app.dirty = true;
    }
}

/// Let the card under the cursor go: no card is drawn wearing the cursor,
/// and the PANE along the bottom collapses — it is the selected session, so with nothing selected there
/// is nothing for it to be and the grid takes the whole body back
/// ([`App::launcher_split`]). What the cursor was on is only let go of and
/// not forgotten: [`take_aim`] brings both the card and the pane back, and
/// `j` steps from where the eye last saw it.
///
/// INPUT PARITY: the one function behind the first Esc ([`escape`]) and
/// the fold of the PANE ([`toggle_pane`]), so both end in the same state
/// and say the same word. A click on the air between the cards is NOT one
/// of them: a miss with the pointer folds nothing away — see the
/// `PanelBg` arm in `event_loop`.
pub(super) fn clear_aim(app: &mut App) {
    app.launcher_unaimed = true;
    app.flash = Some(UNAIMED.into());
    app.dirty = true;
}

/// `⇧A`: swap the grid between a project's LIVE sessions and its
/// ARCHIVED ones. The cards are the same cards — an archived one wears
/// the same name, checkout and last prompt — so what changes is which
/// list the grid is of, and `u` on a card there unarchives it where it
/// stands. The cursor lands on the first card of whichever list arrives,
/// so the pane reads something at once instead of sitting blank under a
/// full grid; an empty list lets the aim go, the way a folded pane does.
///
/// INPUT PARITY: the one function behind the `⇧A` key and the card
/// menu's **Show/hide archived**, so both end in the same state.
pub(crate) fn toggle_archived(app: &mut App, out: &mut Vec<ClientRequest>) {
    app.show_archived = !app.show_archived;
    match view::rows(app).first().map(|row| row.agent.id.clone()) {
        Some(first) => select(app, first, out),
        None => clear_aim(app),
    }
    app.flash = Some(
        if app.show_archived {
            ARCHIVED_VIEW
        } else {
            LIVE_VIEW
        }
        .into(),
    );
    app.dirty = true;
}

/// `^``, and every other chord the pane fold answers to: the way out of
/// the PANE, and then the way it folds. With the keys in the pane —
/// typing into it, or standing in it unlocked — the first press only
/// hands them back to the card it reads, the cursor still on it; pressed
/// again from the cards it folds the pane away ([`toggle_pane`]), and
/// once more brings it back. Returns what it did, for the KEY COMBO
/// DISPLAY.
///
/// INPUT PARITY: the one function behind the chord, whether a LOCKED
/// PANE let it through (`event_loop::handle_key`) or the grid got it
/// ([`handle_action`]).
pub(super) fn fold_key(app: &mut App) -> &'static str {
    if app.focus == Focus::Terminal && !app.launcher_pane_hidden {
        super::leave_terminal_lock(app);
        app.dirty = true;
        return BACK_TO_CARD;
    }
    toggle_pane(app);
    if app.launcher_pane_hidden {
        "Hide the pane"
    } else {
        "Show the pane"
    }
}

/// `^``: fold the PANE under the cards away and give the grid the whole
/// body, or bring it back. Folding it lets the card under the cursor go
/// with it — nothing selected, nothing being read — and bringing the pane
/// back takes the aim again, since the pane reads the card it is aimed at.
///
/// It goes by whether the pane is on screen, not by the fold alone: one
/// let go of for want of a card ([`clear_aim`] — an Esc, or a project
/// with no sessions) is off screen too, and the key brings it back rather
/// than folding away a pane that is already gone.
pub(super) fn toggle_pane(app: &mut App) {
    app.launcher_pane_hidden = !app.launcher_pane_hidden && app.launcher_aimed();
    if app.launcher_pane_hidden {
        clear_aim(app);
        // FOCUS cannot stay in a pane that is no longer drawn: the keys
        // come back to the cards, the way `^q` hands them back.
        app.focus = Focus::Sessions;
        app.term_locked = false;
        app.flash = Some(PANE_HIDDEN.into());
    } else {
        take_aim(app);
        app.flash = Some(PANE_SHOWN.into());
    }
    app.dirty = true;
}

/// The pane's SIDE BUTTON: move the PANE to the other side of the cards —
/// from along the bottom to down the right, or back — by writing Settings
/// → Appearance → **Session pane** and adopting it, exactly as cycling
/// that row does, so the move outlives the restart and the row reads it.
/// Nothing where the button is not drawn (`App::launcher_pane_move_to`).
pub(super) fn move_pane(app: &mut App) {
    let Some(to) = app.launcher_pane_move_to() else {
        return;
    };
    let mut cfg = crate::config::Config::load();
    cfg.session_pane = to.as_str().into();
    if super::save_config(app, &cfg) {
        super::apply_config(app, &cfg);
        app.flash = Some(
            match to {
                view::PaneSide::Right => PANE_MOVED_RIGHT,
                view::PaneSide::Bottom => PANE_MOVED_BOTTOM,
            }
            .into(),
        );
    }
    app.dirty = true;
}

/// A double-click on the pane's edge: the edge snaps to the middle of the
/// body, the cards and the pane sharing it evenly — a row each way under
/// the cards, a column each way beside them — and that size is remembered
/// as a drag there would be ([`App::set_launcher_pane`] does the same
/// clamping, so a body too small for an even split rests the edge against
/// the nearer stop). A drag that has wandered off to one end comes back to
/// the middle in one gesture instead of being felt for.
///
/// INPUT PARITY: the one function behind the double-click
/// (`event_loop::handle_mouse`), for any key that comes to want it.
pub(super) fn center_pane(app: &mut App) {
    if app.launcher_pane_boundary().is_none() {
        return;
    }
    app.set_launcher_pane(app.launcher_pane_midpoint());
    app.flash = Some(PANE_CENTERED.into());
    app.dirty = true;
}

// ---- the BANDS ----

/// What `` ` `` says with no terminal to walk to in the checkout.
const NO_TERMINALS: &str = "no terminal in this checkout — t opens one";

/// What it says over a full-screen session, which has no grid to walk.
pub(super) const NO_PANE_HERE: &str = "the terminals are cards on the grid — ^q back to it";

/// What `t` says when it opened the terminal on the ROOT checkout for
/// want of a band under the cursor.
const TERMINAL_ON_ROOT: &str = "nothing selected — the terminal opens on the root checkout";
/// And what it says with no root checkout to fall back on.
const NO_ROOT_FOR_TERMINAL: &str = "no checkout selected — j/k onto a worktree, then t";

/// The band the cursor wears — None once the aim has been let go of
/// (`App::launcher_unaimed`), as `ui::launcher_view::wearing` draws it.
fn wearing_band(app: &App, bands: &[view::Band]) -> Option<usize> {
    (!app.launcher_unaimed)
        .then(|| view::band_cursor(app, bands))
        .flatten()
}

/// The card the cursor wears inside `band`, by the same rule.
fn wearing_card(app: &App, band: &view::Band) -> Option<usize> {
    (!app.launcher_unaimed)
        .then(|| view::card_cursor(app, band))
        .flatten()
}

/// A jump — the `/` PALETTE, the attention walk, a card clicked or
/// stepped onto, `j`/`k` along the bands — has landed the selection on a
/// session or a checkout: the grid aims at it, the pane on the card (a
/// checkout's is the one it was last left on, `restore_session`'s
/// choice). Run by `event_loop::jump_to_target_inner`'s session and
/// worktree arms, so every way onto a card or a band ends aimed at it.
pub(super) fn land_on_grid(app: &mut App) {
    if !app.launcher_active() {
        return;
    }
    take_aim(app);
}

/// The checkout of the EMPTY BAND under the grid's cursor — a band with
/// no card on it, which only **Show all worktrees** draws. What `d` and
/// a right-click act on there: the worktree itself, as nothing else is.
/// None with the grid down, or the cursor on a card or a band of them.
pub(super) fn empty_band(app: &App) -> Option<WorktreeId> {
    if !app.launcher_active() || app.selected_session_row().is_some() {
        return None;
    }
    let bands = view::bands(app);
    let band = &bands[view::band_cursor(app, &bands)?];
    band.cards.is_empty().then(|| band.worktree.clone())
}

/// The cursor onto `worktree`'s BAND: the checkout under the panels'
/// cursor through the jump the `/` PALETTE takes for a worktree, its
/// remembered card — the session it was last left on, else its first
/// attachable row — under the pane, and the grid at the band level.
///
/// INPUT PARITY: the one landing behind `j`/`k` along the bands, a click
/// on a band's rule and a click on the pull request on it, so the key
/// and the pointer end in the same state.
pub(super) fn select_band(app: &mut App, worktree: WorktreeId, out: &mut Vec<ClientRequest>) {
    // The FOLLOW-UP STRIP is aimed at the card it was opened on: the
    // cursor leaving for another checkout folds it rather than sending
    // the next turn to a session the cursor has left.
    app.follow_up = None;
    take_aim(app);
    jump_to_target_inner(
        app,
        PaletteTarget::Worktree(worktree),
        Landing::FocusOnly,
        out,
    );
    app.focus = Focus::Sessions;
    app.dirty = true;
}

/// The cursor onto `sref`'s card, inside its worktree: a session through
/// the jump the `/` PALETTE takes ([`select`]), a terminal through the
/// same landing on its row ([`select_terminal`]). The pane follows onto
/// the card, the keys stay on the grid.
pub(super) fn select_card(app: &mut App, sref: SessionRef, out: &mut Vec<ClientRequest>) {
    match sref {
        SessionRef::Agent(id) => select(app, id, out),
        SessionRef::Terminal(id) => select_terminal(app, id, out),
    }
}

/// What Tab says on a LIST band that already lists every entry it has.
const ALL_LISTED: &str = "every session in this worktree is already listed";

/// What a terminal that left the tree between the draw and the key says.
const TERMINAL_GONE: &str = "that terminal is gone";

/// [`select`] for a TERMINAL: the selection onto its row — its project,
/// its checkout, the row in that checkout's list — and the pane reading
/// it, as walking onto a card does. The `/` PALETTE has no terminal
/// rows, so this landing is the grid's own.
fn select_terminal(app: &mut App, id: nebula_core::TerminalId, out: &mut Vec<ClientRequest>) {
    app.follow_up = None;
    take_aim(app);
    let worktree = app
        .tree
        .terminals
        .iter()
        .find(|t| t.id == id)
        .map(|t| t.worktree_id.clone());
    let landed = worktree.as_ref().and_then(|wid| {
        let pid = app
            .tree
            .worktrees
            .iter()
            .find(|w| &w.id == wid)
            .map(|w| w.project_id.clone())?;
        select_project_row_by_id(app, &pid)
            .then(|| app.worktree_row_of(wid))
            .flatten()
    });
    let Some(wt_index) = landed else {
        app.flash = Some(TERMINAL_GONE.into());
        return;
    };
    app.sel_worktree = wt_index;
    let Some(index) = app
        .visible_session_rows()
        .iter()
        .position(|r| matches!(r, SessionRow::Terminal(t) if t.id == id))
    else {
        app.flash = Some(TERMINAL_GONE.into());
        return;
    };
    app.sel_session = index;
    app.focus = Focus::Sessions;
    app.dirty = true;
    super::preview_selected(app, out);
}

/// Tab on the GRID: the ACCORDION opens the band under the cursor — its
/// cards wrapped into rows under its rule in place of the collapsed
/// STRIP's one row — closing whichever other one was open, so at most
/// one band is open at a time. On the one already open it closes
/// instead. With no band aimed at — Esc let it go — the first band,
/// opened.
///
/// Never what Enter does: Enter always opens the card itself
/// ([`enter_pane`]), expanded or not.
///
/// INPUT PARITY: the one function behind the key and a second click on
/// a band's rule ([`click_band`]).
pub(super) fn toggle_band_expand(app: &mut App, out: &mut Vec<ClientRequest>) {
    let bands = view::bands(app);
    if bands.is_empty() {
        app.flash = Some(nothing_here(app).into());
        return;
    }
    let index = match wearing_band(app, &bands) {
        Some(index) => index,
        None => {
            select_band(app, bands[0].worktree.clone(), out);
            0
        }
    };
    if bands[index].cards.is_empty() {
        // An EMPTY BAND has no cards to open onto rows.
        app.flash = Some(NO_SESSIONS.into());
        take_aim(app);
        return;
    }
    let worktree = bands[index].worktree.clone();
    let open = app.launcher_expanded.as_ref() == Some(&worktree);
    if app.launcher_list && !open && bands[index].cards.len() <= view::LIST_RECENT {
        // The LIST already shows every entry of a band this short:
        // opening it would change nothing on screen but the rule's word.
        app.flash = Some(ALL_LISTED.into());
        take_aim(app);
        return;
    }
    app.launcher_expanded = if open { None } else { Some(worktree) };
    take_aim(app);
    app.dirty = true;
}

/// A terminal just opened — `t`, a card menu's **New terminal** —
/// comes up as its card on the grid with the PANE on it: unfolded and
/// aimed, so the shell that was asked for is on screen rather than
/// behind a fold. Run by the create's Ack (`event_loop::attach_created`),
/// which lands the selection on the row and attaches it.
pub(super) fn show_created_terminal(app: &mut App) {
    app.launcher_pane_hidden = false;
    take_aim(app);
    app.dirty = true;
}

/// `t` on the grid: a shell terminal in the cursor's checkout — the band
/// the cursor is on, or the worktree the grid is inside — and, with no
/// band aimed at (Esc let it go), the project's ROOT checkout, the footer
/// saying so. The Ack lands on the new chip inside its worktree with the
/// keys in the pane ([`show_created_terminal`]). The grid offers no `+`
/// for a terminal: the key, and a card menu's **New terminal**, are how
/// one opens.
pub(super) fn new_terminal(app: &mut App, out: &mut Vec<ClientRequest>) {
    if !app.launcher_unaimed && app.selected_worktree().is_some() {
        super::create_terminal_for_context(app, out);
        return;
    }
    let Some(project) = app.selected_project().map(|p| p.id.clone()) else {
        app.flash = Some("add a project first".into());
        return;
    };
    let Some(root) = view::root_checkout(app, &project) else {
        app.flash = Some(NO_ROOT_FOR_TERMINAL.into());
        return;
    };
    super::create_terminal(app, root, out);
    app.flash = Some(TERMINAL_ON_ROOT.into());
}

/// `` ` `` on the grid: the next TERMINAL chip in the cursor's checkout —
/// inside it, from the band level as from a card — and back round to
/// the first, so the shells of one checkout are a loop the key walks
/// without passing through its sessions.
pub(super) fn walk_terminals(app: &mut App, out: &mut Vec<ClientRequest>) {
    let bands = view::bands(app);
    let Some(band) = view::band_cursor(app, &bands) else {
        app.flash = Some(nothing_here(app).into());
        return;
    };
    let cards = &bands[band].cards;
    let terminals: Vec<usize> = cards
        .iter()
        .enumerate()
        .filter(|(_, c)| c.is_terminal())
        .map(|(i, _)| i)
        .collect();
    if terminals.is_empty() {
        app.flash = Some(NO_TERMINALS.into());
        return;
    }
    let at = wearing_card(app, &bands[band]);
    let next = match at.and_then(|i| terminals.iter().position(|&t| t == i)) {
        Some(p) => terminals[(p + 1) % terminals.len()],
        None => terminals[0],
    };
    select_card(app, cards[next].sref(), out);
}

/// `h` / `l` (`←` / `→`) along a collapsed band: the cursor one card
/// along the band it is on — its sessions, then its terminals, the order
/// the row draws them — stopping at either end rather than wrapping, the
/// pane swapping onto each card as it passes: Enter opens the card the
/// walk stopped on. The row scrolls under the cursor to keep its card on
/// screen (`launcher::BandsLayout::strip_at`), the `❮` / `❯` beside it
/// saying which way the rest went. With no band under the cursor — the
/// selection on a checkout with nothing running — the first band, as
/// `j` takes it.
///
/// INPUT PARITY: the one walk behind the keys and a click on either
/// arrow ([`click_strip_arrow`]).
pub(super) fn walk_band(app: &mut App, dx: i64, out: &mut Vec<ClientRequest>) {
    let bands = view::bands(app);
    let Some(band) = view::band_cursor(app, &bands) else {
        match bands.first() {
            Some(first) => select_band(app, first.worktree.clone(), out),
            None => app.flash = Some(nothing_here(app).into()),
        }
        return;
    };
    let cards = &bands[band].cards;
    let at = view::card_cursor(app, &bands[band]);
    // One row of every card: `dx` along it, held to its ends.
    let Some(next) = view::grid_stepped(at, dx, 0, cards.len(), cards.len()) else {
        return;
    };
    if Some(next) == at {
        take_aim(app);
        return;
    }
    select_card(app, cards[next].sref(), out);
}

/// A click on the `❮` / `❯` beside a band's row: the cursor onto that
/// band first if it was on another — the pane opening on it as a click
/// on its rule does ([`select_band_row`]) — then one card along it that
/// way, the very step `h` / `l` take ([`walk_band`]).
pub(super) fn click_strip_arrow(
    app: &mut App,
    index: usize,
    dx: i64,
    out: &mut Vec<ClientRequest>,
) {
    let bands = view::bands(app);
    if index >= bands.len() {
        return;
    }
    if view::band_cursor(app, &bands) != Some(index) || app.launcher_pane_hidden {
        select_band_row(app, index, out);
    }
    walk_band(app, dx, out);
}

/// Esc in the GRID: first close the ACCORDION's open band, if one is
/// open; then let the card under the cursor go ([`clear_aim`]). With
/// nothing selected already, and nothing open, there is nothing left to
/// let go of, and Esc does nothing: the grid is the top of the view.
///
/// With the PROJECT TABS holding the keys ([`focus_tabs`]) Esc is only
/// the way back down: the cards get the keys again, on the project the
/// header's cursor walked to and the card it shows.
pub(super) fn escape(app: &mut App) {
    if app.launcher_tab_cursor.is_some() {
        leave_tabs(app);
    } else if app.open_band(&view::bands(app)).is_some() {
        app.launcher_expanded = None;
        app.dirty = true;
    } else if !app.launcher_unaimed {
        clear_aim(app);
    }
}

/// A panel key while the GRID is up — true when the view took it: `h` and
/// `l` walk a row of cards, `j` and `k` the column under the cursor, the
/// ways into a session step down into the PANE along the bottom, `p` /
/// `n` open the box, the PROJECT TABS
/// walk with `[` / `]` / a digit and close with `x`; every other key falls
/// through to its panel meaning, which reads the same selection the
/// grid's cursor is.
///
/// Nothing here fires while a session is full-screen: the keys are the
/// PTY's then, and `^q` (`leave_terminal_lock`) is the way back to the
/// grid.
///
/// `armed` and `chord` are the DOUBLE TAP's (`focus_walk::double_tapped`):
/// `k`,`k` on the top row of cards walks up into the PROJECT TABS, and
/// `j`,`j` there walks back down ([`tabs_action`]).
pub(super) fn handle_action(
    app: &mut App,
    action: Action,
    armed: Option<(Action, std::time::Instant)>,
    chord: &KeyChord,
    out: &mut Vec<ClientRequest>,
) -> bool {
    if app.launcher_tab_cursor.is_some() && tabs_action(app, action, armed, chord, out) {
        return true;
    }
    match action {
        Action::MoveDown => step_grid(app, 0, 1, out),
        Action::MoveUp => step_up(app, armed, chord, out),
        Action::FocusRight => step_grid(app, 1, 0, out),
        Action::FocusLeft => step_grid(app, -1, 0, out),
        Action::HalfPageDown => step_grid(app, 0, HALF_PAGE, out),
        Action::HalfPageUp => step_grid(app, 0, -HALF_PAGE, out),
        // Enter always opens the card under the cursor into the PANE
        // beside the cards, where the session is already running and
        // reading it only takes the keys — expanded or not, and on a
        // band aimed at rather than any one card, its remembered card
        // (`enter_pane`'s own `cursor_or_first`).
        Action::Activate => enter_pane(app, out),
        // Tab — the panels' "next panel" — is the grid's own here: the
        // ACCORDION, opening the band under the cursor's cards in place
        // or folding them back up. Never what Enter does.
        Action::FocusNext => toggle_band_expand(app, out),
        // `` ` ``: the checkout's TERMINAL chips, one after another.
        Action::PaneTabs => walk_terminals(app, out),
        // `t`: a terminal in the cursor's checkout, as its chip.
        Action::NewTerminal => new_terminal(app, out),
        // `p`: the box on the Settings → Agents harness. `n`: the harness
        // first, then the same box set to it.
        Action::QuickPrompt => open_box(app),
        Action::New => open_new_session(app),
        // `⇧A` swaps the grid for the project's archived sessions, and
        // back. It is the grid's own key here rather than the panels'
        // group fold: there is no ARCHIVED group to open, only the other
        // list of cards.
        Action::ToggleArchived => toggle_archived(app, out),
        // Space on a card: its FOLLOW-UP MODAL, a box over the grid —
        // the card itself has no room to grow one, and the pane stays
        // exactly as it is.
        Action::FollowUp => follow_up(app),
        // `⇧V` on a card: the pull request its `#42 title` line names, in
        // the browser. `⇧I`: the issue it was started from.
        Action::OpenPullRequest => open_pull_request(app, out),
        Action::OpenIssue => open_issue(app, out),
        // `⇧P` on a card: another session with its settings, nothing typed.
        Action::DuplicateSession => duplicate_session(app),
        // The fold is a preference the view keeps, and its key is the way
        // out of the pane first ([`fold_key`]).
        Action::ToggleLauncherPane => {
            fold_key(app);
        }
        // `^F` on a card: that session full-screen.
        Action::ToggleFullScreen => {
            toggle_full_screen(app, out);
        }
        // The PROJECT TABS across the header.
        Action::NextProjectTab => step_tab(app, 1, out),
        Action::PrevProjectTab => step_tab(app, -1, out),
        Action::CloseProjectTab => close_active_tab(app, out),
        Action::SelectProjectTab(n) => open_tab_slot(app, n, out),
        // `+` (or `⌘P`): the list the header's `+` drops, the click's own
        // [`open_project_menu`].
        Action::ProjectDropdown => open_project_menu(app),
        _ => return false,
    }
    true
}

/// Space on a card, and **Follow-up prompt** in its menu: the FOLLOW-UP
/// MODAL for the session under the cursor — a small box floating over the
/// grid, where the SESSIONS PANEL expands the card itself. Enter there
/// sends what was typed down that session's PTY as its next turn
/// (`event_loop::send_turn`) and the box closes onto the grid.
///
/// Nothing about the PANE moves: it is not unfolded, not swapped onto the
/// card and not focused. Prompting a card is not opening it — the point of
/// the box is to hand a session its next turn and move to the next card,
/// without ever stepping into one.
///
/// What a row refuses, and the word it refuses with, comes from the same
/// [`super::activate::no_follow_up`] the panel's box asks.
pub(super) fn follow_up(app: &mut App) {
    let Some(row) = app.selected_session_row() else {
        app.flash = Some(nothing_here(app).into());
        return;
    };
    if let Some(why) = super::activate::no_follow_up(app, &row) {
        app.flash = Some(why);
        return;
    }
    let crate::app::SessionRow::Agent(agent) = row else {
        return;
    };
    super::open_follow_up(app, agent.id, String::new());
}

/// What `⇧V` says with no card under the cursor to read a pull request
/// off — the aim let go of, or a grid with no sessions in it.
const NO_CARD_FOR_PR: &str = "no card selected — j/k onto one, then ⇧V opens its pull request";

/// `⇧V` on the grid, and **Open pull request** in a card's menu: the pull
/// request of the checkout under the cursor — the `#42 title` on its
/// band's rule (`view::RowPr`) — in the browser, without stepping into a
/// session or opening the PULL REQUESTS MODAL to find it. The opening is
/// `event_loop::open_link`'s, so the pull request is marked read on the
/// way out, as it is from every other place a PR opens.
///
/// INPUT PARITY: the menu row's `MenuAction::OpenLink` carries the URL
/// this reads, and ends in the same `open_link`.
pub(super) fn open_pull_request(app: &mut App, out: &mut Vec<ClientRequest>) {
    match card_pull_request(app, NO_CARD_FOR_PR) {
        Ok(pr) => super::open_link(app, &pr.url, out),
        Err(why) => app.flash = Some(why),
    }
}

/// What `y` says with no card under the cursor to comment on the pull
/// request of.
pub(super) const NO_CARD_FOR_COMMENT: &str =
    "no card selected — j/k onto one, then y comments on its pull request";

/// The pull request of the checkout under the grid's cursor — the
/// `#42 title` on its band's rule — or what to say instead: `no_card`
/// with the aim let go of, or that the checkout has none yet. `⇧V` opens
/// it in the browser; `y` comments on it.
pub(super) fn card_pull_request(app: &App, no_card: &str) -> Result<view::RowPr, String> {
    let aimed = !(app.launcher_grid() && app.launcher_unaimed);
    let bands = view::bands(app);
    let Some(band) = view::band_cursor(app, &bands).filter(|_| aimed) else {
        return Err(no_card.into());
    };
    let band = &bands[band];
    band.pr.clone().ok_or_else(|| {
        format!(
            "no pull request on {} yet — ⇧R reloads from GitHub",
            band.branch
        )
    })
}

/// A click on the pull request on a band's rule — its `↗ #42 title`
/// (`HitTarget::LauncherBandPr`): the cursor lands on that band, as the
/// pointer landing on its rule puts it there ([`select_band`]) — unless
/// the grid is already on that checkout, inside it or not, where the
/// cursor stays exactly as it is — and the pull request opens in the
/// browser through the very [`open_pull_request`] `⇧V` runs — marked
/// read on the way out, the footer saying where it went. INPUT PARITY:
/// the click and the key end in the same state. Unlike a click on a
/// card ([`point_at`]), it leaves a folded pane folded: the link leaves
/// nebula for the browser, and a pane unfolded under a window that just
/// took the screen is no card read.
pub(super) fn click_pull_request(
    app: &mut App,
    worktree: &WorktreeId,
    out: &mut Vec<ClientRequest>,
) {
    if app.selected_worktree().map(|w| &w.id) != Some(worktree) {
        select_band(app, worktree.clone(), out);
    }
    take_aim(app);
    open_pull_request(app, out);
}

/// What `⇧I` says with no card under the cursor to read an issue off.
const NO_CARD_FOR_ISSUE: &str = "no card selected — j/k onto one, then ⇧I opens its issue";

/// What `⇧I` says on a card that was not started from an issue.
const NO_ISSUE: &str = "this session wasn't started from an issue — i lists the project's issues";

/// `⇧I` on a card, and **Open issue** in its menu: the GitHub issue the
/// card's session was started from (an ISSUE SESSION, launched out of the
/// ISSUES MODAL — `Agent::issue_url`), in the browser. `i` lists the
/// project's issues in nebula; the shifted key goes to GitHub, as `⇧V`
/// does for `v`'s pull requests.
///
/// INPUT PARITY: the menu row's `MenuAction::OpenLink` carries the URL
/// this reads, and ends in the same `open_link`.
pub(super) fn open_issue(app: &mut App, out: &mut Vec<ClientRequest>) {
    let aimed = !(app.launcher_grid() && app.launcher_unaimed);
    let Some(agent) = app.selected_session().filter(|_| aimed) else {
        app.flash = Some(NO_CARD_FOR_ISSUE.into());
        return;
    };
    match agent.issue_url {
        Some(url) => super::open_link(app, &url, out),
        None => app.flash = Some(NO_ISSUE.into()),
    }
}

/// A click on the ISSUE NUMBER on a session's card — its `#15`
/// (`HitTarget::LauncherCardIssue`): the cursor lands on that card, as
/// the pointer landing on it puts it there ([`select_card`]), and the
/// issue opens in the browser through the very [`open_issue`] `⇧I` runs.
/// INPUT PARITY: the click and the key end in the same state. Like a
/// click on a band's pull request ([`click_pull_request`]), it leaves a
/// folded pane folded: the link leaves nebula for the browser.
pub(super) fn click_issue(app: &mut App, id: &AgentId, out: &mut Vec<ClientRequest>) {
    select_card(app, SessionRef::Agent(id.clone()), out);
    take_aim(app);
    open_issue(app, out);
}

/// A right-click's first half on a card's ISSUE NUMBER: the chip is the
/// card to this button — the cursor onto it and the pane open on it, as
/// on the rest of the card ([`select_card_row`]); the menu it opens
/// carries **Open issue** already. False for a session no longer on the
/// grid.
pub(super) fn select_issue_card(app: &mut App, id: &AgentId, out: &mut Vec<ClientRequest>) -> bool {
    let bands = view::bands(app);
    let at = bands.iter().enumerate().find_map(|(band, b)| {
        b.cards
            .iter()
            .position(|c| c.sref() == SessionRef::Agent(id.clone()))
            .map(|card| CardRef { band, card })
    });
    at.is_some_and(|at| select_card_row(app, at, out))
}

/// What `⇧P` says with no card under the cursor to copy the settings of.
const NO_CARD_TO_DUPLICATE: &str =
    "no card selected — j/k onto one, then ⇧P opens the quick prompt on its settings";

/// `⇧P` on a card, and **Duplicate** in its menu: the QUICK PROMPT, set to
/// launch what the card runs — the same harness, model and effort, into
/// the same checkout, for the same issue where the card is an ISSUE
/// SESSION — so the task is all there is to type. Enter there is the
/// box's own send (`quick_launch::submit`): the settings read off the
/// card go out on the `CreateAgent` exactly as a spec picked through the
/// box's `Tab` / `^O` would, and the box's pickers still work on it. `p`
/// opens the box on the Agents tab defaults and a fresh worktree; the
/// shifted key opens it on the card. Nothing of the card's conversation
/// is copied: the new session is a fresh CLI on the same footing.
///
/// A CLOUD card comes up as a CLOUD box on its model and effort
/// (`QuickLaunch::with_cloud`), since a cloud launch is nothing without
/// its task. A PR SESSION's pull-request scope is the DAEMON's own and
/// never on the card, so its box is an ordinary one into the same
/// checkout.
///
/// INPUT PARITY: the menu row's `MenuAction::DuplicateAgent` carries the
/// id this reads off the cursor, and ends in the same [`duplicate_agent`].
pub(super) fn duplicate_session(app: &mut App) {
    let aimed = !(app.launcher_grid() && app.launcher_unaimed);
    let Some(agent) = app.selected_session().filter(|_| aimed) else {
        app.flash = Some(NO_CARD_TO_DUPLICATE.into());
        return;
    };
    duplicate_agent(app, agent.id);
}

/// The box behind [`duplicate_session`] and the card menu's
/// **Duplicate**: the QUICK PROMPT on session `id`'s settings, its text
/// empty. It goes up as it stands, never as the parked draft a previous
/// Esc left for the same checkout (`quick_prompt::open_box`): the card's
/// spec is the point of the key, and the draft keeps for the next `p`.
pub(super) fn duplicate_agent(app: &mut App, id: AgentId) {
    let Some(agent) = app.tree.agents.iter().find(|a| a.id == id).cloned() else {
        return;
    };
    let Some(worktree) = app
        .tree
        .worktrees
        .iter()
        .find(|w| w.id == agent.worktree_id)
        .cloned()
    else {
        app.flash = Some("worktree no longer exists".into());
        return;
    };
    // A stand-in checkout git is still cutting: the box would only be
    // refused at Enter, as `p` says on one.
    if app.is_placeholder_worktree(&worktree.id) {
        app.flash = Some("quick prompt: worktree is still being created".into());
        return;
    }
    let issue = agent
        .issue_url
        .as_deref()
        .and_then(|url| issue_ref(app, &worktree.project_id, url));
    let launch = QuickLaunch::of_kind(
        QuickTarget::Worktree(worktree.id),
        agent.kind,
        agent.custom_harness,
        agent.model,
        agent.effort,
        &crate::config::Config::load(),
    )
    .with_issue(issue)
    .with_cloud(agent.cloud_session_id.is_some());
    crate::quick_prompt::reopen(app, launch, "");
}

/// The issue an ISSUE SESSION's card was started from, as the box carries
/// one: its number off the URL's tail, and its title from the project's
/// fetched list when it is in there. The box's title names the number
/// and an empty send fixes the issue, with or without a title.
fn issue_ref(app: &App, project: &ProjectId, url: &str) -> Option<crate::issues::IssueRef> {
    let number: u64 = url.trim_end_matches('/').rsplit('/').next()?.parse().ok()?;
    let title = app
        .issues
        .get(project)
        .and_then(|issues| issues.list.iter().find(|i| i.url == url))
        .map(|i| i.title.clone())
        .unwrap_or_default();
    Some(crate::issues::IssueRef {
        url: url.to_string(),
        number,
        title,
    })
}

/// The PROJECT's own menu — a right-click on its PROJECT TAB ([`tab_menu`]):
/// what the PROJECTS and WORKTREES panels' rows carried between them — a
/// checkout cut in it, the project renamed or dropped, and the RUN COMMAND
/// of the checkout the grid would launch into started or stopped. These have no key of their own on the
/// grid, whose letters belong to the session under the cursor, so the
/// project's menu is where they live. `at` is where it hangs.
fn project_menu(app: &mut App, at: (u16, u16)) {
    let Some(project) = app.selected_project().map(|p| p.id.clone()) else {
        return;
    };
    let mut items = vec![MenuItem::new(
        "New worktree",
        MenuAction::NewWorktree(project.clone()),
    )];
    if let Some(w) = crate::launcher::checkout_for(app, &project) {
        let running = app.worktree_running(&w);
        items.push(MenuItem::new(
            if running { "Stop run" } else { "Run" },
            MenuAction::ToggleRun(w.clone()),
        ));
        items.push(MenuItem::new("Open", MenuAction::OpenWorktree(w.clone())));
        if !app.tree.worktrees.iter().any(|x| x.id == w && x.is_main) {
            items.push(MenuItem::destructive(
                "Delete worktree",
                MenuAction::DeleteWorktree(w),
            ));
        }
    }
    items.push(MenuItem::new(
        "Rename",
        MenuAction::RenameProject(project.clone()),
    ));
    items.push(MenuItem::destructive(
        "Remove from list",
        MenuAction::RemoveProject(project),
    ));
    super::open_menu(app, items, at);
}

/// A right-click on a PROJECT TAB: that project opened, as a left click
/// opens it ([`open_tab`]), with its menu hung under the tab — so the
/// menu is always of the project on screen, and what it does is seen.
pub(super) fn tab_menu(app: &mut App, id: &ProjectId, out: &mut Vec<ClientRequest>) {
    open_tab(app, id, out);
    let at = crumb_anchor(app, &HitTarget::LauncherTab(id.clone()));
    project_menu(app, at);
}

/// `h` / `j` / `k` / `l` (and the half-page jumps). `j` and `k` walk the
/// BANDS — the cursor `dy` checkouts down, the pane swapping onto each
/// one's remembered card as it passes — and `h` and `l` walk the cards
/// along a collapsed band's own row ([`walk_band`]). On the band open as
/// the ACCORDION the four walk its rows of cards instead: `h`/`l` along
/// a row, `j`/`k` down the rows of sessions and on into the terminals
/// under them (`launcher::ExpandedLayout::stepped`) — and `j` off its
/// last row, or `k` off its first, steps onto the next band down or up,
/// so the open band sits in the walk rather than trapping it.
pub(super) fn step_grid(app: &mut App, dx: i64, dy: i64, out: &mut Vec<ClientRequest>) {
    let bands = view::bands(app);
    if bands.is_empty() {
        app.flash = Some(nothing_here(app).into());
        return;
    }
    // The cursor itself, aimed or not: a step from a card let go of
    // (Esc, the fold) starts where the eye last saw it, and takes the aim
    // back on landing. In the compact LIST every band is a column of
    // entries, so the keys walk its lines wherever the cursor is.
    if let Some((band, layout)) = app.walked_band(&bands) {
        let at = view::card_cursor(app, &bands[band]);
        // An EMPTY BAND (**Show all worktrees**) has no card to step to:
        // `j`/`k` go straight on to the band above or below it.
        let next = layout.stepped(at, dx, dy);
        if let Some(next) = next.filter(|&next| Some(next) != at) {
            let sref = bands[band].cards[next].sref();
            select_card(app, sref, out);
            return;
        }
        if dy == 0 {
            take_aim(app);
            return;
        }
        // Against the open band's first or last row: on to the band
        // above or below it, as from any other band.
    } else if dy == 0 {
        walk_band(app, dx, out);
        return;
    }
    let at = view::band_cursor(app, &bands);
    let last = bands.len() as i64 - 1;
    let next = match at {
        None if dy > 0 => 0,
        None => last as usize,
        Some(b) => (b as i64 + dy).clamp(0, last) as usize,
    };
    if app.launcher_list && Some(next) != at {
        // The LIST reads as one column down every band: `j` off a band's
        // last line lands on the next band's first, `k` off its first on
        // the band above's last — not on whichever card that band last
        // had, which may be lines away from where the eye is.
        let entries = view::list_layout(app.body_area, &bands[next], false, None).rows;
        let edge = if dy > 0 {
            entries.first()
        } else {
            entries.last()
        };
        if let Some(&card) = edge.and_then(|row| row.first()) {
            select_card(app, bands[next].cards[card].sref(), out);
            return;
        }
    }
    if Some(next) == at {
        // On the band already, but on none of its cards — the selection
        // on a row the grid has no card for: its first card, as `h`/`l`
        // take it. An EMPTY BAND has none: the cursor is on all of it.
        if view::card_cursor(app, &bands[next]).is_none() {
            match bands[next].cards.first() {
                Some(first) => select_card(app, first.sref(), out),
                None => take_aim(app),
            }
            return;
        }
        take_aim(app);
        return;
    }
    select_band(app, bands[next].worktree.clone(), out);
}

/// `k` (↑): a row up the grid — and on the top row, where there is no
/// row above, the edge of a DOUBLE TAP: the first press stays put and
/// says what a second one does, the second walks up into the PROJECT
/// TABS ([`focus_tabs`]). The top row is the first band — the first row
/// of its cards when it is the one open as the ACCORDION; a grid with no
/// cards on it is all top row. Only `k` itself: `^u`'s half page stops
/// against the top like any other edge, so leaning on it never lands in
/// the header.
fn step_up(
    app: &mut App,
    armed: Option<(Action, std::time::Instant)>,
    chord: &KeyChord,
    out: &mut Vec<ClientRequest>,
) {
    let bands = view::bands(app);
    let top = match app.walked_band(&bands) {
        Some((band, layout)) => {
            band == 0 && layout.on_top_row(view::card_cursor(app, &bands[band]))
        }
        None => view::band_cursor(app, &bands).map_or(bands.is_empty(), |b| b == 0),
    };
    if !top {
        step_grid(app, 0, -1, out);
    } else if super::double_tapped(app, Action::MoveUp, armed, chord, "project tabs") {
        focus_tabs(app);
    }
}

/// Rows a notch of the wheel scrolls the grid: a third of a card, the PR
/// PREVIEW's step.
const GRID_WHEEL_ROWS: i32 = 3;

/// A notch of the wheel over the GRID: the whole panel — every band's
/// rule, the ACCORDION's open one's cards under it — a few rows, under a
/// cursor that stays put — the pane keeps reading the card it was on, so
/// a trackpad never swaps it out from under you — and the scroll held at
/// the panel's ends. The next key that walks the grid brings the
/// cursor's card back on screen ([`take_aim`], `App::launcher_reveal`).
/// A notch over a panel that fits the screen moves nothing.
pub(super) fn wheel_grid(app: &mut App, up: bool) {
    let bands = view::bands(app);
    if bands.is_empty() {
        return;
    }
    let panel = app.panel_layout(&bands);
    let max = panel.max_scroll();
    let delta = if up {
        -GRID_WHEEL_ROWS
    } else {
        GRID_WHEEL_ROWS
    };
    let next = crate::app::scrolled_by(panel.clamp(app.launcher_scroll), delta, max);
    if next == app.launcher_scroll {
        return;
    }
    app.launcher_scroll = next;
    app.launcher_scroll_held = true;
    app.dirty = true;
}

// ---- the PROJECT TABS ----

/// Where a dropdown hangs off the header — the PROJECT DROPDOWN under the
/// `+`, a project's menu under its tab: the row under it, at its own left
/// edge, so the list reads as belonging to it rather than to wherever the
/// pointer happened to land. A word the header had no room to draw falls
/// back to the keyboard's anchor.
fn crumb_anchor(app: &App, crumb: &HitTarget) -> (u16, u16) {
    app.hit_rect(crumb)
        .map(|rect| (rect.x, rect.y + 1))
        .unwrap_or(super::KEYBOARD_MENU_ANCHOR)
}

/// What the tab keys say over a full-screen session, where the header
/// they walk is not on screen.
pub(super) const NO_TABS_HERE: &str = "project tabs are on the grid's header — ^q back to it";
/// What the tab keys say with nothing to move between.
const NO_TABS: &str = "no projects open — + in the header opens one";
const ONE_TAB: &str = "one project open — + in the header opens another";
/// The PROJECT DROPDOWN's last row: a folder that is not a project yet.
const OPEN_FOLDER: &str = "+ open a folder…";
/// What closing the last tab says: nebula is back on the splash, and
/// nothing about the projects changed.
const LAST_TAB: &str = "all projects closed — their sessions run on; + opens one again";

/// A click on a PROJECT TAB, `[` / `]` onto it, and the tab that slides
/// into a closed one's place: that project's sessions, through the one
/// [`open_project`] the `+` dropdown takes — so the grid lands where it
/// would have landed however the project was opened.
/// INPUT IS NOT ACTION: the key and the click both end here.
///
/// The tab the grid is already on is left alone: opening it again would
/// throw the cursor off the card it was walked to.
pub(super) fn open_tab(app: &mut App, id: &ProjectId, out: &mut Vec<ClientRequest>) {
    if app.selected_project().is_some_and(|p| &p.id == id) {
        return;
    }
    open_project(app, id, out);
}

/// A digit (or ⌘N): the Nth PROJECT TAB from the left, as a click on it
/// opens it ([`open_tab`]) — the browser's own shortcut for its tabs.
fn open_tab_slot(app: &mut App, slot: u8, out: &mut Vec<ClientRequest>) {
    app.settle_project_tabs();
    let Some(id) = open_tabs(app)
        .get(usize::from(slot).saturating_sub(1))
        .cloned()
    else {
        app.flash = Some(format!("no project tab {slot} — + in the header opens one"));
        return;
    };
    open_tab(app, &id, out);
}

/// The tabs the header draws, by project ([`view::project_tabs`]), which
/// are the ones the keys walk and the ones a closed tab's neighbor is
/// found among.
fn open_tabs(app: &App) -> Vec<ProjectId> {
    view::project_tabs(app).into_iter().map(|t| t.id).collect()
}

/// `]` and `[`: the tab `delta` along from the one the grid is on,
/// stopping at either end rather than wrapping round — from no tab at
/// all, the first or the last.
pub(super) fn step_tab(app: &mut App, delta: i64, out: &mut Vec<ClientRequest>) {
    app.settle_project_tabs();
    let tabs = open_tabs(app);
    let Some(last) = tabs.len().checked_sub(1) else {
        app.flash = Some(NO_TABS.into());
        return;
    };
    let at = app
        .selected_project()
        .and_then(|p| tabs.iter().position(|t| t == &p.id));
    let next = match at {
        Some(at) => (at as i64 + delta).clamp(0, last as i64) as usize,
        None if delta < 0 => last,
        None => 0,
    };
    // The end of the row — `[` on the first tab, `]` on the last — goes
    // nowhere.
    if Some(next) == at {
        if last == 0 {
            app.flash = Some(ONE_TAB.into());
        }
        return;
    }
    open_tab(app, &tabs[next], out);
}

/// `k`,`k` on the top row of cards: the keys go up to the PROJECT TABS,
/// their cursor on the tab of the project on screen. `h` / `l` walk the
/// cursor along the tabs and the grid comes with it, each project shown
/// on the card it was last left on ([`walk_tab_cursor`]); Enter, `j`,`j`
/// back down, or Esc hands the keys back to that card ([`choose_tab`]).
/// The card under the grid's cursor stays under it, drawn unfocused, and
/// the pane keeps reading it.
pub(super) fn focus_tabs(app: &mut App) {
    app.settle_project_tabs();
    let tabs = open_tabs(app);
    let Some(first) = tabs.first().cloned() else {
        app.flash = Some(NO_TABS.into());
        return;
    };
    let on = app
        .selected_project()
        .map(|p| p.id.clone())
        .filter(|id| tabs.contains(id))
        .unwrap_or(first);
    app.launcher_tab_cursor = Some(on);
    app.dirty = true;
}

/// The keys back to the cards, on whichever project the header's cursor
/// walked the grid to: Esc, a click, and any key the header has no use
/// for, which then goes on to mean what it means on the grid.
pub(super) fn leave_tabs(app: &mut App) {
    if app.launcher_tab_cursor.take().is_some() {
        app.dirty = true;
    }
}

/// A key while the PROJECT TABS have the keyboard: `h` / `l` (and `[` /
/// `]`) walk the header's cursor and the grid with it, Enter goes back
/// down to the cards of the tab under it, `j`,`j` does too — the double
/// tap back down the way `k`,`k` came up — `x` closes it, and `d`,
/// `Delete` or `Backspace` asks first ([`confirm_close_cursor_tab`]).
/// `k` has nowhere higher to go. Any other key hands the keys
/// back to the cards and is false, so the grid's own meaning of it runs:
/// `p` opens the box, `/` the palette, a digit a tab outright.
fn tabs_action(
    app: &mut App,
    action: Action,
    armed: Option<(Action, std::time::Instant)>,
    chord: &KeyChord,
    out: &mut Vec<ClientRequest>,
) -> bool {
    let Some(on) = app.launcher_tab_cursor.clone() else {
        return false;
    };
    match action {
        Action::FocusLeft | Action::PrevProjectTab => walk_tab_cursor(app, -1, out),
        Action::FocusRight | Action::NextProjectTab => walk_tab_cursor(app, 1, out),
        Action::Activate => choose_tab(app, &on, out),
        Action::MoveDown => {
            let name = app
                .tree
                .projects
                .iter()
                .find(|p| p.id == on)
                .map(|p| p.name.clone())
                .unwrap_or_default();
            if super::double_tapped(app, action, armed, chord, &format!("into {name}")) {
                choose_tab(app, &on, out);
            }
        }
        Action::MoveUp => {}
        Action::CloseProjectTab => close_cursor_tab(app, &on, out),
        // The DELETE keys (`d`, `Delete`, `Backspace`) aimed at the tab
        // under the cursor rather than the card they left: the same close,
        // behind the confirm every delete key in nebula puts up first.
        Action::Delete => confirm_close_cursor_tab(app, &on),
        _ => {
            leave_tabs(app);
            return false;
        }
    }
    true
}

/// `h` / `l` with the PROJECT TABS holding the keys: the header's cursor
/// `delta` tabs along, stopping at either end as `[` / `]` do on the
/// grid — and the grid under it switches to that project at once, on the
/// card it was last left on, with the pane reading that session
/// ([`open_tab`], the tab click's own move). No Enter is needed to see a
/// project: the header walks the projects the way `h` / `l` walk the
/// cards, and the keys stay up here until they go back down.
fn walk_tab_cursor(app: &mut App, delta: i64, out: &mut Vec<ClientRequest>) {
    app.settle_project_tabs();
    let tabs = open_tabs(app);
    let Some(at) = app
        .launcher_tab_cursor
        .as_ref()
        .and_then(|on| tabs.iter().position(|t| t == on))
    else {
        return;
    };
    if tabs.len() == 1 {
        app.flash = Some(ONE_TAB.into());
        return;
    }
    let next = (at as i64 + delta).clamp(0, tabs.len() as i64 - 1) as usize;
    if next == at {
        return;
    }
    app.launcher_tab_cursor = Some(tabs[next].clone());
    app.dirty = true;
    open_tab(app, &tabs[next], out);
}

/// Enter on the header's cursor, `j`,`j` down from it, and a click on a
/// tab while the header has the keys ([`click_tab`]): the keys go back to
/// the cards of project `id`. The walk has already put the grid on the
/// project under the cursor, so that is only handing the keys down to the
/// card it shows; a click on another tab opens that one first, on its
/// last-focused card ([`open_tab`]).
pub(super) fn choose_tab(app: &mut App, id: &ProjectId, out: &mut Vec<ClientRequest>) {
    leave_tabs(app);
    open_tab(app, id, out);
}

/// A left click on a PROJECT TAB. With the keys on the cards it is the
/// tab key's twin ([`open_tab`]); with the header holding them it is
/// Enter on that tab ([`choose_tab`]) — so the pointer never means
/// something else than the key would in the same state.
pub(super) fn click_tab(app: &mut App, id: &ProjectId, out: &mut Vec<ClientRequest>) {
    if app.launcher_tab_cursor.is_some() {
        choose_tab(app, id, out);
    } else {
        open_tab(app, id, out);
    }
}

/// `x` with the PROJECT TABS holding the keys — and the answered confirm
/// of `d`, `Delete` or `Backspace` there ([`confirm_close_cursor_tab`]):
/// close the tab the header's cursor is on, through the same
/// [`close_tab`] the `×` takes, and put the cursor on the tab that slid
/// into its place. The last tab closes to the SPLASH ([`close_tab`]). A
/// cursor that has moved off the tab by the time the dialog is
/// answered stays where it is.
pub(super) fn close_cursor_tab(app: &mut App, on: &ProjectId, out: &mut Vec<ClientRequest>) {
    let at = open_tabs(app).iter().position(|t| t == on);
    close_tab(app, on, out);
    let tabs = open_tabs(app);
    if tabs.contains(on) || app.launcher_tab_cursor.as_ref() != Some(on) {
        return;
    }
    let next = at
        .and_then(|at| tabs.get(at.min(tabs.len().saturating_sub(1))))
        .cloned();
    app.launcher_tab_cursor = next;
    app.dirty = true;
}

/// `d`, `Delete` or `Backspace` with the PROJECT TABS holding the keys:
/// the confirm before the tab under the header's cursor closes. Its Enter
/// is [`close_cursor_tab`], the `x`; its Esc leaves the tab and the cursor
/// where they are.
fn confirm_close_cursor_tab(app: &mut App, on: &ProjectId) {
    let name = app
        .tree
        .projects
        .iter()
        .find(|p| &p.id == on)
        .map(|p| p.name.clone())
        .unwrap_or_default();
    app.overlay = Some(Overlay::Confirm(confirm_close_tab(&name, on.clone())));
}

/// The confirm before a PROJECT TAB is closed from a delete key. It says
/// what the `×` never has to: only the tab goes, and the `+` brings it
/// back.
fn confirm_close_tab(name: &str, id: ProjectId) -> ConfirmDialog {
    ConfirmDialog {
        title: "Close project tab".into(),
        message: format!("Close the tab for '{name}'? Its sessions run on — + opens it again."),
        action: PendingAction::CloseProjectTab(id),
        area: ratatui::layout::Rect::default(),
    }
}

/// `x`: close the tab the grid is on — the selected project's.
fn close_active_tab(app: &mut App, out: &mut Vec<ClientRequest>) {
    app.settle_project_tabs();
    let active = app
        .selected_project()
        .map(|p| p.id.clone())
        .filter(|id| open_tabs(app).contains(id));
    match active {
        Some(id) => close_tab(app, &id, out),
        None => app.flash = Some(NO_TABS.into()),
    }
}

/// Drop project `id`'s tab from the header — the `×` on it, or `x` on the
/// one the grid is on. Nothing about the project changes: its sessions
/// run on, and the `+` dropdown opens it again, back at the far left.
///
/// Closing the tab the grid is on moves the grid to the tab that slides
/// into its place — the one to its right, else the one to its left — as
/// closing a TERMINAL tab moves the pane ([`tab_after`]). Closing any
/// other tab moves nothing. Closing the last tab leaves nebula where it
/// starts before there is any project: the SPLASH, the pane let go
/// ([`App::projects_closed`]). Enter, `+` or `o` there opens one again.
pub(super) fn close_tab(app: &mut App, id: &ProjectId, out: &mut Vec<ClientRequest>) {
    let mut tabs = open_tabs(app);
    let Some(at) = tabs.iter().position(|t| t == id) else {
        return;
    };
    if tabs.len() == 1 {
        close_every_project(app, out);
        return;
    }
    let showing = app.selected_project().is_some_and(|p| &p.id == id);
    app.launcher_tabs.retain(|t| t != id);
    tabs.remove(at);
    app.dirty = true;
    if !showing {
        return;
    }
    let next = tabs
        .get(at)
        .or_else(|| at.checked_sub(1).and_then(|before| tabs.get(before)))
        .cloned();
    if let Some(next) = next {
        open_tab(app, &next, out);
    }
}

/// The last PROJECT TAB closed: the grid goes and the SPLASH comes back.
/// The pane lets go of its session — nothing on the splash shows it — and
/// the header's cursor, the aim and the accordion go with the grid.
fn close_every_project(app: &mut App, out: &mut Vec<ClientRequest>) {
    app.launcher_tabs.clear();
    app.launcher_tab_cursor = None;
    app.launcher_expanded = None;
    app.follow_up = None;
    clear_aim(app);
    if app.term.is_some() {
        super::detach_pane(app, out);
    }
    app.focus = Focus::Sessions;
    app.projects_closed = true;
    app.flash = Some(LAST_TAB.into());
    app.dirty = true;
}

/// The PROJECT DROPDOWN: a click on the `+` after the PROJECT TABS drops
/// every project on this machine under it ([`view::project_cards`]) —
/// the ones wanting a human first, then the most recently worked in —
/// the one in front of you ticked, each with how many sessions it holds.
/// Enter on a row opens that project's sessions ([`open_project`]) and,
/// if it had none, a tab for it at the far left. The last row opens a
/// folder that is not a project yet — the same prompt `o` opens — so the
/// `+` is the one place to reach for any project, known or not.
///
/// `+` or `⌘P` ([`Action::ProjectDropdown`]) drops the same list from the
/// keyboard — from the cards, or from inside the PANE under them
/// (`event_loop::drops_project_dropdown`) — and puts it away again; the
/// key `+` does it from the cards in terminals that never send ⌘.
/// INPUT IS NOT ACTION: the click and the keys all land here.
///
/// It takes TYPE-AHEAD, the same one the MODEL and EFFORT submenus have
/// ([`crate::app::MenuFilter`]): letters narrow the rows to what they
/// fuzzy-match, best first, so a tree of thirty projects is three letters
/// and Enter away from any of them rather than a scroll. ↑/↓ move,
/// Backspace widens, Esc clears the text before it closes — and a letter
/// no project matches is refused, so the list never empties.
///
/// The tick rides at the END of a row, which is where
/// [`crate::app::ContextMenu::set_filter`] looks for it: clearing the
/// query hands the cursor back to the project you are already in rather
/// than to the top of the list.
pub(super) fn open_project_menu(app: &mut App) {
    // With every tab closed the selection under the splash is no project
    // in front of you, so nothing is ticked.
    let open = app
        .launcher_active()
        .then(|| app.selected_project().map(|p| p.id.clone()))
        .flatten();
    let cards = view::project_cards(app);
    if cards.is_empty() {
        app.flash = Some(NO_PROJECTS.into());
        return;
    }
    let mut items: Vec<MenuItem> = cards
        .iter()
        .map(|card| {
            MenuItem::new(
                format!(
                    "{}  ({}){}",
                    card.name,
                    card.sessions.len(),
                    if Some(&card.id) == open.as_ref() {
                        " ✓"
                    } else {
                        ""
                    }
                ),
                MenuAction::OpenProject(card.id.clone()),
            )
        })
        .collect();
    items.push(MenuItem::new(OPEN_FOLDER, MenuAction::AddProject));
    let hover = open
        .as_ref()
        .and_then(|id| cards.iter().position(|card| &card.id == id))
        .unwrap_or(0);
    // On the splash there is no header `+` to hang it off, so it sits in
    // the middle of the screen, over the nebula, rather than in a corner.
    let at = (!app.splash_showing()).then(|| crumb_anchor(app, &HitTarget::LauncherTabAdd));
    app.overlay = Some(Overlay::Menu(ContextMenu {
        title: Some("Project".into()),
        filter: Some(MenuFilter {
            query: String::new(),
            all: items.clone(),
        }),
        items,
        at,
        hover,
        area: ratatui::layout::Rect::default(),
        parent: None,
    }));
    app.dirty = true;
}

/// Put the cursor on project `id` — the grid's scope, and the lit tab.
/// Nothing is attached here: [`open_project`] puts the cursor on a card
/// next, and that is what brings its session up in the pane.
fn select_project(app: &mut App, id: &ProjectId) {
    take_aim(app);
    if !select_project_row_by_id(app, id) {
        app.flash = Some("project no longer exists".into());
        return;
    }
    restore_project_cursors(app);
    app.dirty = true;
}

/// Into project `id` — the one move, so a tab, a digit and a pick from the
/// PROJECT DROPDOWN end in the same state. INPUT IS NOT ACTION: the key,
/// the click and the dropdown row all land here.
///
/// The cursor lands on the card the project was last left on
/// ([`last_focused`]), so switching away and back puts you where you
/// were, the pane reading the same session; on a first visit, or with
/// that session gone or archived since, on the project's FIRST card.
///
/// A project with nothing in it yet lands on the empty grid, which names
/// the key that starts a session — no BOX. Opening one here would put a
/// modal over a screen the user only asked to look at: switching to a
/// project is not asking to start a session in it.
pub(super) fn open_project(app: &mut App, id: &ProjectId, out: &mut Vec<ClientRequest>) {
    let Some(card) = view::project_cards(app).into_iter().find(|c| &c.id == id) else {
        app.flash = Some("project no longer exists".into());
        return;
    };
    let land = last_focused(app, &card).or_else(|| card.sessions.first().map(|a| a.id.clone()));
    select_project(app, id);
    match land {
        Some(id) => select(app, id, out),
        // No session, but a terminal keeps a band on the grid: the
        // cursor lands on it rather than on nothing.
        None => match view::bands(app).first().map(|b| b.worktree.clone()) {
            Some(worktree) => select_band(app, worktree, out),
            None => fold_empty_grid(app, out),
        },
    }
}

/// Nothing to aim at: with no card the PANE along the bottom has no
/// session to be, so it folds and the grid takes the body — the hero, and
/// the word for how to fill it.
///
/// INPUT PARITY: the one landing for a grid with no cards, whether a
/// project with none was opened ([`open_project`]) or the last card was
/// archived or deleted out of it ([`keep_cursor`]).
fn fold_empty_grid(app: &mut App, out: &mut Vec<ClientRequest>) {
    let word = nothing_here(app);
    clear_aim(app);
    // Nothing left to keep open either: the grid is the (empty) bands.
    app.launcher_expanded = None;
    // And nothing left attached behind the fold: what the pane read
    // belongs to cards this grid no longer holds — another project's, or
    // the one just archived — so bringing the pane back (`^``) opens it
    // empty, never on a session from somewhere else.
    if app.term.is_some() {
        super::detach_pane(app, out);
    }
    app.flash = Some(word.into());
}

/// The card project `card` was last left on: the session under the cursor
/// when the grid last moved off the project (`remember_context` keeps its
/// checkout and that checkout's session), while it is still one of the
/// project's cards. Read before [`select_project`], whose own bookkeeping
/// rewrites it.
fn last_focused(app: &App, card: &view::ProjectCard) -> Option<AgentId> {
    let wid = app.last_worktree_for_project.get(&card.id)?;
    let SessionRef::Agent(id) = app.last_session_for_worktree.get(wid)? else {
        return None;
    };
    card.sessions
        .iter()
        .any(|a| &a.id == id)
        .then(|| id.clone())
}

/// Put the cursor on session `id`, through the jump the `/` PALETTE takes
/// — so the panels' selection and every verb that reads it agree with the
/// card. FOCUS lands on the grid.
///
/// [`Landing::FocusOnly`]: the card the cursor lands on is the one the
/// PANE under the grid reads, so walking the cards swaps the pane exactly
/// as ↑/↓ down the SESSIONS PANEL previews a row — the same debounced
/// attach, and the same mark-as-seen, since that session is now on screen.
/// FOCUS stays on the grid: the pane is opened, never entered.
fn select(app: &mut App, id: AgentId, out: &mut Vec<ClientRequest>) {
    // A pointer moved onto another card is the one way the cursor leaves
    // an open FOLLOW-UP STRIP while it holds the keyboard: the strip is
    // aimed at the card it was opened on, so it folds rather than sending
    // the next turn to a session the cursor has left.
    if app.follow_up.as_ref().is_some_and(|f| f.agent != id) {
        app.follow_up = None;
    }
    take_aim(app);
    jump_to_target_inner(app, PaletteTarget::Session(id), Landing::FocusOnly, out);
    app.focus = Focus::Sessions;
}

/// A click on the card at `at`: the cursor lands on it, which opens the
/// PANE beside the cards on that session ([`point_at`]) without taking
/// the keys off the cards — the session is there to read, not yet to
/// type into. The grid stays at the level it was: at the band level the
/// click aims the band and its pane at the card, as `j`/`k` do, and
/// nothing under the pointer moves. A second click on the same card is
/// Enter, twice if need be — into the worktree, then down into the pane,
/// where that session is already running.
pub(super) fn click_card(app: &mut App, at: CardRef, out: &mut Vec<ClientRequest>) {
    let Some(sref) = point_at(app, at, out) else {
        return;
    };
    if is_double_click(
        &mut app.last_session_click,
        crate::app::RowKey::Session(sref),
    ) {
        enter_pane(app, out);
    }
}

/// A click on a BAND's rule: the cursor onto that band, the pane on its
/// remembered card — what `j`/`k` walking onto it do — unfolding the pane
/// as a click on a card does. A second click on the same rule opens the
/// ACCORDION on it, or closes it — what `z` does. INPUT PARITY:
/// [`select_band`] and [`toggle_band_expand`].
pub(super) fn click_band(app: &mut App, index: usize, out: &mut Vec<ClientRequest>) {
    let bands = view::bands(app);
    let Some(worktree) = bands.get(index).map(|b| b.worktree.clone()) else {
        return;
    };
    select_band_row(app, index, out);
    if is_double_click(
        &mut app.last_session_click,
        crate::app::RowKey::Worktree(worktree),
    ) {
        toggle_band_expand(app, out);
    }
}

/// A click on the MORE HINT under a band's row (`HitTarget::LauncherBandMore`):
/// the cursor onto the band, then the band opens as the ACCORDION.
///
/// INPUT PARITY: the open is [`toggle_band_expand`], the one Tab runs.
pub(super) fn click_band_more(app: &mut App, index: usize, out: &mut Vec<ClientRequest>) {
    if !select_band_row(app, index, out) {
        return;
    }
    let bands = view::bands(app);
    if app.launcher_expanded.as_ref() != bands.get(index).map(|b| &b.worktree) {
        toggle_band_expand(app, out);
    }
}

/// A right-click's first half on a card, `select_clicked_row`'s arm: the
/// cursor on the card at `at` and the pane open on it, as a left click
/// leaves them. False off the grid.
pub(super) fn select_card_row(app: &mut App, at: CardRef, out: &mut Vec<ClientRequest>) -> bool {
    point_at(app, at, out).is_some()
}

/// The same for a band's rule: the cursor onto the band at `index`.
/// False for a band no longer on the grid.
pub(super) fn select_band_row(app: &mut App, index: usize, out: &mut Vec<ClientRequest>) -> bool {
    let bands = view::bands(app);
    let Some(band) = bands.get(index) else {
        return false;
    };
    let worktree = band.worktree.clone();
    if app.launcher_pane_hidden {
        toggle_pane(app);
    }
    select_band(app, worktree, out);
    true
}

/// And for the band of checkout `worktree`, wherever it sits.
pub(super) fn select_band_of(
    app: &mut App,
    worktree: &WorktreeId,
    out: &mut Vec<ClientRequest>,
) -> bool {
    let bands = view::bands(app);
    match bands.iter().position(|b| &b.worktree == worktree) {
        Some(index) => select_band_row(app, index, out),
        None => false,
    }
}

/// The pointer landing on the card at `at`, either button: the cursor
/// goes there and the PANE beside the cards opens on it — ALWAYS, folded
/// away (`^~`) or not. A card clicked is a card to read, so the fold only
/// lasts while the grid is walked with the keys; a click unfolds it the
/// way Enter does ([`enter_pane`]). The ACCORDION is left as it is: a
/// card clicked on a collapsed band aims it without opening it, and the
/// pointer's second click finds the card where the first left it. None
/// off the grid.
fn point_at(app: &mut App, at: CardRef, out: &mut Vec<ClientRequest>) -> Option<SessionRef> {
    let bands = view::bands(app);
    let sref = view::card_at(&bands, at)?.sref();
    if app.launcher_pane_hidden {
        toggle_pane(app);
    }
    select_card(app, sref.clone(), out);
    Some(sref)
}

/// The cursor ahead of an input event, for [`keep_cursor`]: the band it
/// was on, the card in it — inside the worktree, or the band's card the
/// pane was reading — and the project whose bands the grid was listing.
pub(super) struct CursorCard {
    /// The band's checkout, and where the band sat on the grid.
    worktree: WorktreeId,
    band_index: usize,
    /// The card, when the cursor was on one: its session or terminal,
    /// where it sat in the band, and its neighbours by identity — the
    /// card before it and the one after — so [`keep_cursor`] lands on
    /// the card that slid up into the slot rather than on whatever a bare
    /// index points at once the list has moved.
    sref: Option<SessionRef>,
    index: usize,
    before: Option<SessionRef>,
    after: Option<SessionRef>,
    /// The grid's project, and which of its two lists it was showing. A
    /// switch to another project, or to the ARCHIVED VIEW and back, takes
    /// every band off the grid at once, which [`keep_cursor`] has to tell
    /// from one card leaving it.
    project: Option<ProjectId>,
    archived: bool,
}

pub(super) fn cursor_entry(app: &App) -> Option<CursorCard> {
    let bands = view::bands(app);
    let band_index = view::band_cursor(app, &bands)?;
    let band = &bands[band_index];
    let card = view::card_cursor(app, band);
    let sref_at = |i: usize| band.cards.get(i).map(|c| c.sref());
    Some(CursorCard {
        worktree: band.worktree.clone(),
        band_index,
        sref: card.and_then(sref_at),
        index: card.unwrap_or(0),
        before: card.and_then(|c| c.checked_sub(1)).and_then(sref_at),
        after: card.and_then(|c| sref_at(c + 1)),
        project: app.selected_project().map(|p| p.id.clone()),
        archived: app.show_archived,
    })
}

/// After an input event: the card the cursor was on left the grid —
/// archived (`a`), deleted (`d`, its confirm), a terminal closed, from
/// the grid or a menu. The cursor takes the card AFTER it in its band —
/// the one that slides up into the slot it left, so an archiving sweep
/// along a band reads on from where it was — with the pane on it; the
/// card BEFORE it when the one that left was the last, since there is
/// nothing after that. The band's LAST card leaving takes the band with
/// it: the grid is the bands again, the cursor on the one that slid up
/// into its slot — and with no band left at all, the pane folds away
/// ([`fold_empty_grid`]), as it does on a project opened with none.
///
/// The grid settles this itself rather than leaving it to the PANELS'
/// own reseat (`reconcile_selection`), which runs on the same archive:
/// their list is one checkout's, in tree order, while a band's cards
/// are ordered by recency — so the neighbour they hand the cursor to is
/// some other card, and taking it threw the cursor across the band.
pub(super) fn keep_cursor(app: &mut App, before: CursorCard, out: &mut Vec<ClientRequest>) {
    // Not this grid any more: another project's bands are up. Every card
    // left it, this one with them, and whatever put the cursor on one of
    // the new ones meant to.
    if app.selected_project().map(|p| p.id.clone()) != before.project
        || app.show_archived != before.archived
        // A create is still being followed onto its own row (`n`, the
        // box): that landing is the one the user asked for.
        || app.select_when_seen.is_some()
    {
        return;
    }
    let bands = view::bands(app);
    if bands.is_empty() {
        fold_empty_grid(app, out);
        return;
    }
    let Some(band_index) = bands.iter().position(|b| b.worktree == before.worktree) else {
        let next = before.band_index.min(bands.len() - 1);
        tracing::debug!(
            left = %before.worktree.0,
            was_at = before.band_index,
            lands_on = %bands[next].worktree.0,
            "launcher grid: the cursor's band left the grid"
        );
        // A band that leaves the grid takes its ACCORDION with it: the
        // one that slides up is collapsed, as every other band is.
        if app.launcher_expanded.as_ref() == Some(&before.worktree) {
            app.launcher_expanded = None;
        }
        select_band(app, bands[next].worktree.clone(), out);
        return;
    };
    let band = &bands[band_index];
    let Some(sref) = before.sref else {
        return;
    };
    if band.position(&sref).is_some() {
        return;
    }
    if band.cards.is_empty() {
        // Its last card gone, the band stayed as an EMPTY BAND (**Show
        // all worktrees**): the cursor stays on it, on the checkout.
        select_band(app, band.worktree.clone(), out);
        return;
    }
    let at = |s: &Option<SessionRef>| s.as_ref().and_then(|s| band.position(s));
    let next = at(&before.after)
        .or_else(|| at(&before.before))
        .unwrap_or_else(|| before.index.min(band.cards.len() - 1));
    let next_sref = band.cards[next].sref();
    tracing::debug!(
        was_at = before.index,
        lands_on = %band.cards[next].name(),
        "launcher grid: the cursor's card left the band"
    );
    select_card(app, next_sref, out);
}

/// Enter anywhere on the GRID (or a double-click on a card): the
/// card under the cursor in the PANE beside the cards, with the input
/// lock on — focus crosses into the pane where it stands, the grid still
/// up over it (expanded or not — this never touches the ACCORDION), and
/// `^`` ([`fold_key`]) hands the keys back to the cards, exactly as it
/// does after a click into the pane. Only a body too short to draw the
/// pane gives the session the whole screen instead ([`open_session`]). A
/// TERMINAL's chip attaches its shell the same way.
///
/// The jump attaches the card outright, so a card the pane's debounce had
/// not reached yet is the one the keys reach.
pub(super) fn enter_pane(app: &mut App, out: &mut Vec<ClientRequest>) {
    // A pane folded away (`^~`) is brought back rather than stepped over:
    // opening a session here means the pane beside the cards, so asking
    // for one unfolds it instead of taking the whole screen out from
    // under the grid. With the pane already drawn this changes nothing
    // and focus lands in it below, so a second ask, with the keys already
    // there, is a no-op.
    if app.launcher_pane_hidden {
        toggle_pane(app);
    }
    // Nor is a pane collapsed for want of a card under the cursor
    // ([`clear_aim`] — Esc, a click on the air) stepped over: the aim
    // comes back first, so the pane opens on the card Enter is about to
    // cross into instead of the keys landing on nothing.
    take_aim(app);
    // A body too short for a pane worth the name draws none ([`has_pane`]):
    // there is nothing beside the cards to cross into, so the session takes
    // the whole screen rather than the keys going somewhere off-screen.
    if !has_pane(app) {
        open_session(app, out);
        return;
    }
    let Some(sref) = cursor_or_first(app) else {
        app.flash = Some(nothing_here(app).into());
        return;
    };
    match sref {
        SessionRef::Terminal(id) => {
            select_card(app, SessionRef::Terminal(id.clone()), out);
            super::attach_now(app, SessionRef::Terminal(id), out);
            enter_terminal_pane(app, out);
        }
        SessionRef::Agent(id) => {
            // An ARCHIVED card has no session to read: the daemon reaped
            // it when it was archived. Say what to press rather than
            // handing the keys to an empty pane.
            if is_archived(app, &id) {
                app.flash = Some(super::AGENT_ARCHIVED.into());
                return;
            }
            jump_to_target(app, PaletteTarget::Session(id), Landing::Attach, out);
            // A Cloud row's Enter is its browser page, not a PTY: the jump
            // has already opened it and there is nothing to type into.
            if app.term.is_some() {
                enter_terminal_pane(app, out);
            }
        }
    }
}

/// The card under the cursor full-screen — the grid and its pane give
/// way to the PTY with the input lock on, and `^q` comes back to the
/// grid. Enter's fallback ([`enter_pane`]) on a body too short to draw
/// the pane: there is nothing beside the cards to step into, so the
/// session takes the whole screen. The jump attaches the card outright,
/// so a card the pane's debounce had not reached yet is the one that
/// comes up.
pub(super) fn open_session(app: &mut App, out: &mut Vec<ClientRequest>) {
    let Some(sref) = cursor_or_first(app) else {
        app.flash = Some(nothing_here(app).into());
        return;
    };
    match sref {
        SessionRef::Terminal(id) => {
            select_card(app, SessionRef::Terminal(id.clone()), out);
            super::attach_now(app, SessionRef::Terminal(id), out);
            super::zoom_pane(app, out);
        }
        SessionRef::Agent(id) => {
            if is_archived(app, &id) {
                app.flash = Some(super::AGENT_ARCHIVED.into());
                return;
            }
            take_aim(app);
            jump_to_target(app, PaletteTarget::Session(id), Landing::Attach, out);
            // A Cloud row's Enter is its browser page, not a PTY: the jump
            // has already opened it and there is nothing to full-screen.
            if app.term.is_some() {
                super::zoom_pane(app, out);
            }
        }
    }
}

/// `^F`, and the FULL-SCREEN BUTTON in the pane's header: the session
/// in the PANE takes the whole screen, the input lock on — or, from a
/// full-screen session, comes back down to the pane beside the cards
/// with the keys still in it, exactly where `^F` took it from. From the
/// cards it full-screens the one under the cursor ([`open_session`]).
/// A session full-screened for want of a pane (a body too short to draw
/// one, the pane folded away) has nothing to come back down to, and
/// lands on the grid the way the crumb always took it. Returns what it
/// did, for the KEY COMBO DISPLAY.
///
/// INPUT PARITY: the one function behind the chord (from the grid, or let
/// through a LOCKED PANE), `^q` and `^`` in a full-screen session, the
/// `‹ sessions` crumb and the header's button.
pub(super) fn toggle_full_screen(app: &mut App, out: &mut Vec<ClientRequest>) -> &'static str {
    app.dirty = true;
    if app.collapsed {
        app.collapsed = false;
        if app.launcher_pane_hidden || !has_pane(app) {
            super::leave_terminal_lock(app);
            return "Back to the grid";
        }
        return NORMAL_SIZE;
    }
    if app.focus == Focus::Terminal && app.term.is_some() {
        super::zoom_pane(app, out);
    } else {
        open_session(app, out);
    }
    FULL_SCREEN
}

/// What [`toggle_full_screen`] says it did, for the KEY COMBO DISPLAY.
pub(super) const FULL_SCREEN: &str = "Full screen";
pub(super) const NORMAL_SIZE: &str = "Normal size";

/// Is the PANE beside the cards on screen? False with the pane folded
/// away (`^~`), on a body too short to hold the header, a row of cards
/// and a pane worth the name, where a session is only ever seen
/// full-screen — and before the first draw, which no key beats.
fn has_pane(app: &App) -> bool {
    app.launcher_split(app.launcher_body).1.is_some()
}

/// The card under the cursor — inside a worktree, or the band's card the
/// pane reads — or the first card of the first band when it is on none:
/// the pane shows no session until something is walked onto, so a way in
/// from a fresh launch takes the newest card rather than saying there is
/// nothing to enter. None with no card anywhere, which [`NO_SESSIONS`]
/// answers.
fn cursor_or_first(app: &App) -> Option<SessionRef> {
    let bands = view::bands(app);
    let band = view::band_cursor(app, &bands).or((!bands.is_empty()).then_some(0))?;
    let band = &bands[band];
    let at = view::card_cursor(app, band).unwrap_or(0);
    band.cards.get(at).map(|c| c.sref())
}

// ---- the box's own keys ----

/// Is `prompt`'s key one of the view's box chords? `^P` (project), `^T`
/// (worktree), `^O` (model) and `^N` (fresh worktree or not) — the rest
/// of the box's keys are the QUICK PROMPT's own. True when the key was
/// taken.
pub(super) fn handle_box_key(
    app: &mut App,
    key: &KeyEvent,
    launch: &QuickLaunch,
    input: &TextInput,
) -> bool {
    if !key.modifiers.contains(KeyModifiers::CONTROL) {
        return false;
    }
    let back = QuickReturn {
        launch: launch.clone(),
        text: input.as_str().to_string(),
        from_box: true,
    };
    match key.code {
        KeyCode::Char('p' | 'P') => open_box_field(app, BoxField::Project, back),
        KeyCode::Char('t' | 'T') => open_box_field(app, BoxField::Worktree, back),
        KeyCode::Char('o' | 'O') => open_box_field(app, BoxField::Model, back),
        KeyCode::Char('n' | 'N') => toggle_new_worktree(app, launch.clone(), input.clone()),
        _ => return false,
    }
    true
}

/// The picker behind one of the box's details, whichever way it was
/// asked for — the chord beside it or a click on it. Input is not action:
/// there is one of these per field and both ways in end here.
///
/// `Tab`'s own arm (`event_loop`'s prompt keys) is the harness picker for
/// every QUICK PROMPT, LAUNCHER VIEW or not, and it opens the same
/// [`crate::quick_prompt::open_launch_picker`] this does.
pub(super) fn open_box_field(app: &mut App, field: BoxField, back: QuickReturn) {
    match field {
        BoxField::Project => open_project_picker(app, back),
        BoxField::Worktree => open_worktree_picker(app, back),
        BoxField::Agent => crate::quick_prompt::open_launch_picker(app, back),
        BoxField::Model => open_model_picker(app, back),
    }
}

/// A click on one of the box's details: the launch and the text read off
/// the box that is up, then the same [`open_box_field`] the chord takes.
pub(super) fn click_box_field(app: &mut App, field: BoxField) {
    let Some(crate::app::Overlay::Prompt(prompt)) = &app.overlay else {
        return;
    };
    let crate::app::PromptKind::QuickPrompt(launch) = &prompt.kind else {
        return;
    };
    let back = QuickReturn {
        launch: launch.clone(),
        text: prompt.input.as_str().to_string(),
        from_box: true,
    };
    open_box_field(app, field, back);
}

/// `^P`: the PROJECT PICKER over the box. A box already bound to one
/// project's work — an issue's, a pull request's — keeps its project.
/// Only ever reached from a box that is up, so its `back` always hands
/// one back ([`handle_picker_key`]).
fn open_project_picker(app: &mut App, back: QuickReturn) {
    if let Some(issue) = &back.launch.issue {
        app.flash = Some(format!(
            "this box is for issue #{} — its project is fixed",
            issue.number
        ));
        return;
    }
    if let Some(pr) = &back.launch.pr {
        app.flash = Some(format!(
            "this box is for PR #{} — its project is fixed",
            pr.number
        ));
        return;
    }
    let picker = ProjectPicker::new(app, back);
    app.overlay = Some(Overlay::ProjectPicker(picker));
}

/// `^O`: the MODEL list of the box's harness — the submenu `Tab` reaches
/// with `→` on its row, opened straight onto: Enter takes a model (and
/// `→` on one its effort list) and hands the box back, Esc hands it back
/// as it was.
fn open_model_picker(app: &mut App, back: QuickReturn) {
    let (kind, custom) = (back.launch.kind, back.launch.custom.clone());
    if crate::config::model_choices(kind, custom.as_deref()).is_empty() {
        let harness = crate::agent_picker::harness_label(kind, custom.as_deref());
        app.flash = Some(format!(
            "{harness} has no model list — Tab picks the harness"
        ));
        return;
    }
    let Some(worktree) = crate::quick_prompt::picker_context(app, &back.launch) else {
        app.flash = Some("project no longer exists".into());
        return;
    };
    let pr = back.launch.pr.clone();
    let row = MenuItem::new(
        String::new(),
        MenuAction::NewAgentOfKind {
            worktree,
            kind,
            custom,
            model: None,
            effort: None,
            // A model picked for a CLAUDE CLOUD box keeps it one.
            cloud: back.launch.cloud,
            pr,
            quick: Some(Box::new(back)),
        },
    );
    if let Some(menu) = build_submenu(&row) {
        app.overlay = Some(Overlay::Menu(menu));
    }
}

/// A click on the box's `[ ] new worktree` toggle: the same flip `^N` is,
/// read off the box that is up — input is not action, so there is one
/// [`toggle_new_worktree`] and both ways in call it.
pub(super) fn click_new_worktree(app: &mut App) {
    let Some(crate::app::Overlay::Prompt(prompt)) = &app.overlay else {
        return;
    };
    let crate::app::PromptKind::QuickPrompt(launch) = &prompt.kind else {
        return;
    };
    let (launch, input) = (launch.clone(), prompt.input.clone());
    toggle_new_worktree(app, launch, input);
}

/// `^N` in the view's box: flip this launch between a fresh worktree and
/// an existing checkout of the project the box is aimed at — the one
/// under the grid's cursor, else the ROOT BRANCH (`view::launch_checkout`);
/// the project is not always the one under the list's cursor (`^P` moves
/// it). Only this box: the next one starts from the
/// `quick_prompt_new_worktree` SETTING. A PR SESSION's checkout is the
/// DAEMON's to pick, so it has nothing to flip.
fn toggle_new_worktree(app: &mut App, launch: QuickLaunch, input: TextInput) {
    match view::flipped_target(app, &launch) {
        Ok(target) => reopen_with(app, QuickLaunch { target, ..launch }, input),
        Err(why) if launch.pr.is_some() => app.flash = Some(format!("quick prompt: {why}")),
        Err(why) => app.flash = Some(why.into()),
    }
}

/// The WORKTREE PICKER for the box `back` owes — `^T`, or a click on the
/// branch in the box's details row (`worktree main ^T`), dropped down
/// from that branch over the box, which stays on screen under it as it
/// does under `Tab` and `^O` (`ui::draw_overlay`). It is the manual pick of where
/// this one launch runs, and never a branch switch: every checkout keeps
/// the branch it is on. First a fresh worktree — the branch it would be cut on
/// named, the one already minted when the box is aimed at one — then the
/// project's checkouts, the ROOT WORKTREE first and the rest most recently
/// worked in first, as the WORKTREES PANEL lists them. Never a stand-in git
/// is still cutting, nor a root the project hides. The row the box is
/// aimed at wears the ✓ and starts highlighted; letters narrow the list.
///
/// Every row carries the box back, so Esc and a click outside hand it
/// back as it was (`menu_quick_return`), and a pick hands it back aimed
/// at the row ([`pick_launch_worktree`]). A PR SESSION has nothing to
/// pick: the DAEMON runs it in the pull request's own checkout.
fn open_worktree_picker(app: &mut App, back: QuickReturn) {
    use std::cmp::Reverse;
    if back.launch.pr.is_some() {
        app.flash =
            Some("quick prompt: a PR session runs in the pull request's own checkout".into());
        return;
    }
    let Some(project) = view::project_of(app, &back.launch.target) else {
        app.flash = Some("project no longer exists".into());
        return;
    };
    let mut checkouts: Vec<_> = app
        .tree
        .worktrees
        .iter()
        .filter(|w| w.project_id == project && !app.is_placeholder_worktree(&w.id))
        .collect();
    let now = crate::app::now_ms();
    checkouts.sort_by_key(|w| {
        let r = crate::app::worktree_recency(&app.tree, &w.id, now);
        (
            Reverse(w.is_main),
            Reverse(r.interacted),
            Reverse(r.stamped),
        )
    });

    let tick = |on: bool| if on { " ✓" } else { "" };
    let row = |label: String, target: QuickTarget| {
        MenuItem::new(
            label,
            MenuAction::PickLaunchWorktree {
                target,
                back: Box::new(back.clone()),
            },
        )
    };
    let fresh = match &back.launch.target {
        QuickTarget::NewWorktree { .. } => back.launch.target.clone(),
        QuickTarget::Worktree(_) => view::fresh_worktree(app, project.clone(), &back.launch),
    };
    let QuickTarget::NewWorktree { branch, .. } = &fresh else {
        unreachable!("fresh_worktree mints a new worktree")
    };
    let mut items = vec![row(
        format!(
            "+ new worktree  {branch}{}",
            tick(back.launch.is_new_worktree())
        ),
        fresh.clone(),
    )];
    for w in checkouts {
        let aimed = back.launch.target == QuickTarget::Worktree(w.id.clone());
        let root = if w.is_main { "  (root)" } else { "" };
        items.push(row(
            format!("{}{root}{}", w.branch, tick(aimed)),
            QuickTarget::Worktree(w.id.clone()),
        ));
    }
    let hover = items
        .iter()
        .position(|i| i.label.ends_with(" ✓"))
        .unwrap_or(0);
    app.overlay = Some(Overlay::Menu(crate::app::ContextMenu {
        title: Some("Worktree".into()),
        items: items.clone(),
        at: None,
        hover,
        area: ratatui::layout::Rect::default(),
        parent: None,
        filter: Some(crate::app::MenuFilter {
            query: String::new(),
            all: items,
        }),
    }));
}

/// A row of the WORKTREE PICKER: the box back, aimed at `target`, with the
/// text it had. Like `^N`, only this box: the next one starts from the
/// `quick_prompt_new_worktree` SETTING.
pub(super) fn pick_launch_worktree(app: &mut App, target: QuickTarget, back: QuickReturn) {
    let launch = QuickLaunch {
        target,
        ..back.launch
    };
    crate::quick_prompt::reopen(app, launch, &back.text);
}

/// Put the box back up on `launch` with `input` — text and caret — as it
/// was.
fn reopen_with(app: &mut App, launch: QuickLaunch, input: TextInput) {
    open_prompt(app, PromptKind::QuickPrompt(launch));
    if let Some(Overlay::Prompt(prompt)) = &mut app.overlay {
        prompt.input = input;
    }
}

// ---- the PROJECT PICKER ----

/// A key in the PROJECT PICKER: ↑/↓ (and `^P` / `^N`, fzf's) move, Enter
/// picks, Esc clears a typed query and then hands the box back, and
/// everything else edits the query, the list narrowing as you type.
pub(super) fn handle_picker_key(app: &mut App, key: KeyEvent) {
    let Some(Overlay::ProjectPicker(picker)) = &mut app.overlay else {
        return;
    };
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    match key.code {
        KeyCode::Esc if !picker.query.is_empty() => {
            picker.query = TextInput::new();
            picker.apply_filter();
        }
        KeyCode::Esc => {
            let back = picker.back.clone();
            app.overlay = None;
            // A picker with no box under it — `p` with nothing selected —
            // closes on Esc rather than putting up a box nobody asked
            // for, exactly as the preset picker reached off a pull
            // request does (`QuickReturn::from_box`).
            if back.from_box {
                crate::quick_prompt::reopen(app, back.launch, &back.text);
            }
        }
        KeyCode::Enter => pick(app),
        KeyCode::Down => picker.select(1),
        KeyCode::Up => picker.select(-1),
        KeyCode::Char('n' | 'j') if ctrl => picker.select(1),
        KeyCode::Char('p' | 'k') if ctrl => picker.select(-1),
        _ => {
            let before = picker.query.as_str().to_string();
            picker.query.handle_key(&key);
            if picker.query.as_str() != before {
                picker.apply_filter();
            }
        }
    }
}

/// A click in the PROJECT PICKER's list: the row under the pointer is
/// picked, as Enter on it would pick it.
pub(super) fn click_picker_row(app: &mut App, index: usize) {
    let Some(Overlay::ProjectPicker(picker)) = &mut app.overlay else {
        return;
    };
    if index >= picker.matches.len() {
        return;
    }
    picker.selected = index;
    pick(app);
}

/// Enter in the PROJECT PICKER: the box comes back aimed at the project
/// under the cursor, text kept, a fresh worktree as the box had it or the
/// checkout a box aimed there lands in — that project's root branch, the
/// grid's cursor being on this one (`view::target_for`). Aiming the box is not navigation: the grid behind it stays
/// on the project you are working in. The launch that follows is a
/// BACKGROUND LAUNCH (`view::is_background`) — it starts the session over
/// there and leaves the screen here.
fn pick(app: &mut App) {
    let Some(Overlay::ProjectPicker(picker)) = &app.overlay else {
        return;
    };
    let Some(project) = picker.selected_project().cloned() else {
        return;
    };
    let back = picker.back.clone();
    app.overlay = None;
    let target = view::target_for(app, &project.id, back.launch.is_new_worktree());
    let launch = QuickLaunch {
        target,
        ..back.launch
    };
    if back.from_box {
        crate::quick_prompt::reopen(app, launch, &back.text);
        return;
    }
    // No box was under the picker: this pick OPENS one, so it goes
    // through the door every other way into the box takes — which is
    // what hands back the DRAFT the last abandoned box left
    // (`quick_prompt::open_box`).
    crate::quick_prompt::open_box(app, launch);
}

#[cfg(test)]
mod tests {
    use super::super::tests::{
        buffer_text, hse, seed_issues, seed_open_prs, seed_tree, with_config_json,
        with_default_config,
    };
    use super::super::{handle_terminal_event, sweep_target};
    use crate::app::{App, Focus, HitTarget, Overlay, PendingAction, PromptKind};
    use crate::launcher::BoxField;
    use crate::launcher::CardRef;
    use crate::quick_prompt::{QuickLaunch, QuickTarget};
    use crossterm::event::{KeyCode, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
    use nebula_core::{
        Agent, AgentId, AgentKind, AgentStatus, ClientRequest, Entity, Project, ProjectId,
        ServerEvent, SessionRef, TerminalId, TerminalTab, Worktree, WorktreeId,
    };
    use ratatui::backend::TestBackend;
    use ratatui::style::{Color, Modifier};
    use ratatui::Terminal;

    /// `seed_tree`'s `demo` project (its root `main`, session `agent-1`),
    /// plus a second checkout of it — `feat`, running `polish-nav`, the
    /// newer session and so the grid's first card — and a second project,
    /// `web`, whose root runs `tidy-css`.
    ///
    /// The grid is one project's, so it holds demo's two cards and `web`
    /// is a tab away — which is what makes both the walk and the switch
    /// testable off one tree. The view is on; nothing is owed at boot.
    fn two_sessions() -> App {
        let mut app = App::new();
        // Along the bottom: these tests were written against that
        // geometry — the row the pane's strip lands on, how many cards a
        // row holds — and the side is the setting's own tests' business
        // (`the_side_button_moves_the_pane_to_the_bottom_and_back`).
        app.launcher_pane_at = crate::launcher::PaneSide::Bottom;
        seed_tree(&mut app);
        seed_feat(&mut app, "/tmp/demo-feat".into());
        seed_web(&mut app);
        app
    }

    /// A second checkout of `demo` — `feat` at `feat_path` — with
    /// `polish-nav` running in it.
    fn seed_feat(app: &mut App, feat_path: std::path::PathBuf) {
        hse(
            app,
            ServerEvent::EntityUpserted {
                entity: Entity::Worktree(Worktree {
                    id: WorktreeId("w2".into()),
                    project_id: ProjectId("p1".into()),
                    path: feat_path,
                    branch: "feat".into(),
                    is_main: false,
                    sort_order: 1,
                }),
            },
        );
        hse(
            app,
            ServerEvent::EntityUpserted {
                entity: Entity::Agent(Agent {
                    id: AgentId("a2".into()),
                    worktree_id: WorktreeId("w2".into()),
                    name: "polish-nav".into(),
                    status: AgentStatus::Running,
                    archived: false,
                    archived_at: 0,
                    unseen: false,
                    kind: AgentKind::Claude,
                    custom_harness: None,
                    model: None,
                    effort: None,
                    session_id: None,
                    cloud_session_id: None,
                    sort_order: 0,
                    status_changed_at: 0,
                    alive: true,
                    issue_url: None,
                    recent_prompts: Vec::new(),
                }),
            },
        );
    }

    /// [`two_sessions`] with a third card: `ship-docs`, running in
    /// `demo`'s ROOT beside `agent-1`. The grid reads the project's three
    /// by recency — `ship-docs`, `polish-nav`, `agent-1` — while the
    /// SESSIONS PANEL lists the root's two on their own, so the card each
    /// list would hand the cursor to when one leaves is a different card.
    fn three_sessions() -> App {
        let mut app = two_sessions();
        seed_running(&mut app, "a9", "w1", "ship-docs");
        app
    }

    /// One more running session, `id`, in checkout `worktree`.
    fn seed_running(app: &mut App, id: &str, worktree: &str, name: &str) {
        hse(
            app,
            ServerEvent::EntityUpserted {
                entity: Entity::Agent(Agent {
                    id: AgentId(id.into()),
                    worktree_id: WorktreeId(worktree.into()),
                    name: name.into(),
                    status: AgentStatus::Running,
                    archived: false,
                    archived_at: 0,
                    unseen: false,
                    kind: AgentKind::Claude,
                    custom_harness: None,
                    model: None,
                    effort: None,
                    session_id: None,
                    cloud_session_id: None,
                    sort_order: 0,
                    status_changed_at: 0,
                    alive: true,
                    issue_url: None,
                    recent_prompts: Vec::new(),
                }),
            },
        );
    }

    fn seed_web(app: &mut App) {
        hse(
            app,
            ServerEvent::EntityUpserted {
                entity: Entity::Project(Project {
                    id: ProjectId("p2".into()),
                    name: "web".into(),
                    repo_path: "/tmp/web".into(),
                    sort_order: 1,
                }),
            },
        );
        hse(
            app,
            ServerEvent::EntityUpserted {
                entity: Entity::Worktree(Worktree {
                    id: WorktreeId("w2root".into()),
                    project_id: ProjectId("p2".into()),
                    path: "/tmp/web".into(),
                    branch: "main".into(),
                    is_main: true,
                    sort_order: 0,
                }),
            },
        );
        hse(
            app,
            ServerEvent::EntityUpserted {
                entity: Entity::Agent(Agent {
                    id: AgentId("a3".into()),
                    worktree_id: WorktreeId("w2root".into()),
                    name: "tidy-css".into(),
                    status: AgentStatus::Running,
                    archived: false,
                    archived_at: 0,
                    unseen: false,
                    kind: AgentKind::Claude,
                    custom_harness: None,
                    model: None,
                    effort: None,
                    session_id: None,
                    cloud_session_id: None,
                    sort_order: 0,
                    status_changed_at: 0,
                    alive: true,
                    issue_url: None,
                    recent_prompts: Vec::new(),
                }),
            },
        );
    }

    /// Wide enough for three cards a row (`launcher::grid`), so the two
    /// sessions sit side by side and `h`/`l` are what walks them.
    fn draw(app: &mut App) -> Terminal<TestBackend> {
        draw_at(app, 130, 34)
    }

    /// Narrow enough for one card a row, so the grid is a single column
    /// and `j`/`k` are what walks it.
    fn draw_narrow(app: &mut App) -> Terminal<TestBackend> {
        draw_at(app, 44, 34)
    }

    fn draw_at(app: &mut App, w: u16, h: u16) -> Terminal<TestBackend> {
        let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
        terminal.draw(|f| crate::ui::draw(f, app)).unwrap();
        terminal
    }

    /// The session under the cursor full-screen, the one way left to it:
    /// Enter on a body too short to draw the pane ([`super::enter_pane`]
    /// falling through to [`super::open_session`]). Full-screen is a
    /// state, not a fit, so the next `draw` at the usual size keeps it.
    fn full_screen(app: &mut App) {
        draw_at(app, 130, 20);
        key(app, KeyCode::Enter, KeyModifiers::NONE);
    }

    /// INPUT PARITY for the top edge of a PANE along the bottom: the grab
    /// zone the draw registers is where the pane actually starts, a drag
    /// through the loop's own entry point moves that boundary, and the
    /// next frame lays the grid and the pane out at the height it was
    /// left at — grip and grab zone moving with it. The draw and the
    /// handler measure the edge by the same arithmetic, so neither can
    /// drift from the other.
    #[test]
    fn the_panes_top_edge_drags_the_grid_and_the_pane() {
        with_default_config(|| {
            let mut app = two_sessions();
            let edge = |app: &App| {
                app.hits
                    .iter()
                    .find(|(_, hit)| *hit == HitTarget::LauncherPaneSplitter)
                    .map(|(rect, _)| *rect)
            };
            let row = |terminal: &Terminal<TestBackend>, y: u16| {
                buffer_text(terminal)
                    .lines()
                    .nth(y as usize)
                    .unwrap_or_default()
                    .to_string()
            };

            let terminal = draw(&mut app);
            let zone = edge(&app).expect("the pane's edge was registered");
            let body = app.launcher_body;
            let pane_h = crate::launcher::pane_height(body, None).expect("34 rows fits a pane");
            let boundary = body.y + body.height - pane_h;
            assert_eq!(
                (zone.y, zone.height),
                (boundary - 1, 2),
                "the zone is the pane's opening row and the grid row over it"
            );
            assert!(
                row(&terminal, boundary).contains('━'),
                "the grip marks the edge"
            );

            // Pull it up four rows: the pane takes them off the cards.
            mouse(
                &mut app,
                MouseEventKind::Down(MouseButton::Left),
                60,
                boundary,
            );
            mouse(
                &mut app,
                MouseEventKind::Drag(MouseButton::Left),
                60,
                boundary - 4,
            );
            mouse(
                &mut app,
                MouseEventKind::Up(MouseButton::Left),
                60,
                boundary - 4,
            );
            assert_eq!(app.launcher_pane_h, Some(pane_h + 4));

            // The next frame lays it out there, grip and grab zone with it.
            let terminal = draw(&mut app);
            assert_eq!(
                edge(&app).expect("still draggable").y,
                zone.y - 4,
                "the edge moved up with the drag"
            );
            assert!(row(&terminal, boundary - 4).contains('━'));
            assert!(
                !row(&terminal, boundary).contains('━'),
                "and left the row it came from"
            );
        });
    }

    /// INPUT PARITY for a pane BESIDE the cards (Settings → Appearance →
    /// **Session pane**): the draw lays it down the right side, registers
    /// the grab zone on the edge facing the cards, and a drag through the
    /// loop's own entry point moves that edge sideways — the next frame
    /// laying the pane out at the width it was left at, and the height it
    /// had under the cards left alone.
    #[test]
    fn a_side_panes_edge_drags_sideways() {
        use crate::launcher::PaneSide;
        with_default_config(|| {
            let side = PaneSide::Right;
            let mut app = two_sessions();
            app.launcher_pane_at = side;
            let edge = |app: &App| {
                app.hits
                    .iter()
                    .find(|(_, hit)| *hit == HitTarget::LauncherPaneSplitter)
                    .map(|(rect, _)| *rect)
            };
            let col = |terminal: &Terminal<TestBackend>, x: u16| {
                buffer_text(terminal)
                    .lines()
                    .filter_map(|line| line.chars().nth(x as usize))
                    .collect::<String>()
            };

            let terminal = draw(&mut app);
            let body = app.launcher_body;
            let (_, pane) = app.launcher_split(body);
            let pane = pane.expect("130 columns fits a pane beside the cards");
            assert_eq!(app.launcher_pane_side(), side);
            assert_eq!((pane.y, pane.height), (body.y, body.height));
            let zone = edge(&app).expect("the pane's edge was registered");
            let grip_x = crate::launcher::pane_edge(side, pane).x;
            assert!(zone.x <= grip_x && grip_x < zone.x + zone.width);
            assert_eq!(zone.height, pane.height, "the whole edge is grabbable");
            assert!(col(&terminal, grip_x).contains('┃'), "the grip marks it");

            // Four columns toward the cards: the pane takes them.
            let to = grip_x - 4;
            let row = pane.y + pane.height / 2;
            mouse(
                &mut app,
                MouseEventKind::Down(MouseButton::Left),
                grip_x,
                row,
            );
            mouse(&mut app, MouseEventKind::Drag(MouseButton::Left), to, row);
            mouse(&mut app, MouseEventKind::Up(MouseButton::Left), to, row);
            assert_eq!(app.launcher_pane_w, Some(pane.width + 4));
            assert_eq!(app.launcher_pane_h, None, "the bottom's height untouched");

            let terminal = draw(&mut app);
            let (_, moved) = app.launcher_split(app.launcher_body);
            let moved = moved.expect("still a pane");
            assert_eq!(moved.width, pane.width + 4);
            assert_eq!(crate::launcher::pane_edge(side, moved).x, to);
            assert!(col(&terminal, to).contains('┃'), "the grip moved with it");
        });
    }

    /// INPUT PARITY: a double-click on the edge beside the cards is
    /// [`center_pane`] — the edge snaps to the middle column of the body,
    /// however far a drag had taken it, and no drag is armed by the press
    /// that did it. The next frame lays the pane out at half the body, and
    /// the height kept for a pane under the cards is left alone.
    #[test]
    fn a_double_click_on_a_side_panes_edge_centers_it() {
        use crate::launcher::PaneSide;
        with_default_config(|| {
            let mut app = two_sessions();
            app.launcher_pane_at = PaneSide::Right;
            draw(&mut app);
            let body = app.launcher_body;
            let (_, pane) = app.launcher_split(body);
            let pane = pane.expect("130 columns fits a pane beside the cards");
            let grip_x = crate::launcher::pane_edge(PaneSide::Right, pane).x;
            let row = pane.y + pane.height / 2;

            // Drag it well off center first: the pane at nearly its
            // narrowest.
            let to = body.x + body.width - crate::launcher::PANE_MIN_W - 2;
            mouse(
                &mut app,
                MouseEventKind::Down(MouseButton::Left),
                grip_x,
                row,
            );
            mouse(&mut app, MouseEventKind::Drag(MouseButton::Left), to, row);
            mouse(&mut app, MouseEventKind::Up(MouseButton::Left), to, row);
            draw(&mut app);
            let (_, narrow) = app.launcher_split(body);
            let narrow = narrow.expect("still a pane");
            assert_ne!(narrow.width, body.width / 2, "the drag left center");
            let edge_x = crate::launcher::pane_edge(PaneSide::Right, narrow).x;

            // A press straight after the drag is a fresh first press: the
            // drag was not the first half of a double-click, so this arms
            // a drag rather than snapping the edge away from the pointer.
            mouse(
                &mut app,
                MouseEventKind::Down(MouseButton::Left),
                edge_x,
                row,
            );
            assert_eq!(
                app.launcher_pane_drag,
                Some(0),
                "a click after a drag only starts another drag"
            );
            mouse(&mut app, MouseEventKind::Up(MouseButton::Left), edge_x, row);
            // The second press on the edge in a row: the double.
            mouse(
                &mut app,
                MouseEventKind::Down(MouseButton::Left),
                edge_x,
                row,
            );
            assert!(
                app.launcher_pane_drag.is_none(),
                "the double-click arms no drag"
            );
            assert_eq!(app.flash.as_deref(), Some(super::PANE_CENTERED));
            mouse(&mut app, MouseEventKind::Up(MouseButton::Left), edge_x, row);

            draw(&mut app);
            let (grid, centered) = app.launcher_split(body);
            let centered = centered.expect("still a pane");
            assert_eq!(
                centered.x,
                body.x + body.width / 2,
                "the edge sits on the middle column"
            );
            assert_eq!(grid.width + centered.width, body.width);
            assert_eq!(app.launcher_pane_h, None, "the bottom's height untouched");
        });
    }

    /// The same double-click on a pane ALONG THE BOTTOM snaps its top edge
    /// to the middle row — half the body each, where the default share is
    /// a third — and a single click on the edge is still only the start of
    /// a drag.
    #[test]
    fn a_double_click_on_the_panes_top_edge_centers_it() {
        use crate::launcher::PaneSide;
        with_default_config(|| {
            let mut app = two_sessions();
            app.launcher_pane_at = PaneSide::Bottom;
            draw(&mut app);
            let body = app.launcher_body;
            let pane_h = crate::launcher::pane_height(body, None).expect("34 rows fits a pane");
            let boundary = body.y + body.height - pane_h;
            assert_ne!(pane_h, body.height / 2, "the default share is not half");

            // One press: a drag armed, nothing centered.
            mouse(
                &mut app,
                MouseEventKind::Down(MouseButton::Left),
                60,
                boundary,
            );
            assert_eq!(
                app.launcher_pane_drag,
                Some(0),
                "a single click starts a drag"
            );
            assert_eq!(app.launcher_pane_h, None, "…and moves nothing");
            mouse(
                &mut app,
                MouseEventKind::Up(MouseButton::Left),
                60,
                boundary,
            );

            // The second press, on the other grab row: still the edge.
            mouse(
                &mut app,
                MouseEventKind::Down(MouseButton::Left),
                60,
                boundary - 1,
            );
            mouse(
                &mut app,
                MouseEventKind::Up(MouseButton::Left),
                60,
                boundary - 1,
            );
            assert!(app.launcher_pane_drag.is_none());
            assert_eq!(app.launcher_pane_w, None, "the side's width untouched");

            draw(&mut app);
            let (grid, centered) = app.launcher_split(body);
            let centered = centered.expect("still a pane");
            assert_eq!(
                centered.y,
                body.y + body.height / 2,
                "the edge sits on the middle row"
            );
            assert_eq!(grid.height + centered.height, body.height);
        });
    }

    /// The SIDE BUTTON on the pane's TAB STRIP, just before its `×`: a
    /// click on the pane down the right — where it opens out of the box —
    /// moves it under the cards, and the same button — now showing the
    /// way back — moves it down the right again. Each move is the
    /// **Session pane** setting's, written to the config as the settings
    /// row writes it, so it outlives the restart.
    #[test]
    fn the_side_button_moves_the_pane_to_the_bottom_and_back() {
        use crate::launcher::PaneSide;
        with_default_config(|| {
            let mut app = two_sessions();
            app.launcher_pane_at = PaneSide::default();
            let click = |app: &mut App| {
                let at = tab_at(app, HitTarget::LauncherPaneSide);
                let close = tab_at(app, HitTarget::LauncherPaneClose);
                assert_eq!(at.y, close.y, "on the strip's own row");
                assert_eq!(at.x + at.width, close.x, "just before the `×`");
                mouse(app, MouseEventKind::Down(MouseButton::Left), at.x + 1, at.y);
            };
            let side_strip = |app: &App, terminal: &Terminal<TestBackend>| {
                let pane = app.launcher_split(app.launcher_body).1.expect("a pane");
                buffer_text(terminal)
                    .lines()
                    .nth(usize::from(pane.y) + 1)
                    .map(|line| line.chars().skip(usize::from(pane.x)).collect::<String>())
                    .unwrap_or_default()
            };

            let terminal = draw(&mut app);
            assert_eq!(app.launcher_pane_side(), PaneSide::Right, "out of the box");
            let strip = side_strip(&app, &terminal);
            assert!(
                strip.contains('⬓'),
                "down the right it pictures the bottom: {strip}"
            );
            assert!(!strip.contains('◨'));
            click(&mut app);
            assert_eq!(app.launcher_pane_at, PaneSide::Bottom);
            assert_eq!(crate::config::Config::load().pane_side(), PaneSide::Bottom);
            assert_eq!(app.flash.as_deref(), Some(super::PANE_MOVED_BOTTOM));

            let terminal = draw(&mut app);
            assert_eq!(app.launcher_pane_side(), PaneSide::Bottom);
            assert!(
                head_row(&app, &terminal).contains('◨'),
                "along the bottom it pictures the pane on the right"
            );
            click(&mut app);
            assert_eq!(app.launcher_pane_at, PaneSide::Right);
            assert_eq!(crate::config::Config::load().pane_side(), PaneSide::Right);
            assert_eq!(app.flash.as_deref(), Some(super::PANE_MOVED_RIGHT));
            let terminal = draw(&mut app);
            assert_eq!(app.launcher_pane_side(), PaneSide::Right);
            assert!(
                side_strip(&app, &terminal).contains('⬓'),
                "and the way back again"
            );
        });
    }

    /// No SIDE BUTTON where there is nowhere to move the pane: along the
    /// bottom of a window too narrow to stand it beside the cards, a move
    /// to the right would change the setting and nothing on screen.
    #[test]
    fn no_side_button_on_a_window_too_narrow_for_the_right() {
        with_default_config(|| {
            let mut app = two_sessions();
            draw_at(&mut app, 70, 34);
            assert_eq!(app.launcher_pane_move_to(), None);
            assert!(
                !app.hits
                    .iter()
                    .any(|(_, h)| *h == HitTarget::LauncherPaneSide),
                "no button drawn"
            );
            assert!(
                app.hits
                    .iter()
                    .any(|(_, h)| *h == HitTarget::LauncherPaneClose),
                "the close button stays"
            );
        });
    }

    /// Too narrow for the pane beside the cards: it goes along the bottom
    /// rather than away, and its edge is dragged up and down there.
    #[test]
    fn a_side_pane_on_a_narrow_window_lies_along_the_bottom() {
        use crate::launcher::PaneSide;
        with_default_config(|| {
            let mut app = two_sessions();
            app.launcher_pane_at = PaneSide::Right;
            draw_at(&mut app, 70, 34);
            assert_eq!(app.launcher_pane_side(), PaneSide::Bottom);
            let body = app.launcher_body;
            let pane = app.launcher_split(body).1.expect("a pane under the cards");
            assert_eq!((pane.x, pane.width), (body.x, body.width), "full width");
            assert_eq!(pane.y + pane.height, body.y + body.height, "at the bottom");
        });
    }

    /// A digit opens the PROJECT TAB it counts to from the left — the
    /// click on that tab, through the same [`open_tab`] — and one past the
    /// last tab says so rather than doing nothing. `w` means nothing on
    /// the grid: there is no level above it to walk to.
    #[test]
    fn the_digits_open_the_project_tabs() {
        with_default_config(|| {
            let mut by_key = two_tabs();
            key(&mut by_key, KeyCode::Char('1'), KeyModifiers::NONE);
            let mut by_click = two_tabs();
            let (x, y) = crumb_cell(&by_click, HitTarget::LauncherTab(ProjectId("p2".into())));
            mouse(&mut by_click, MouseEventKind::Down(MouseButton::Left), x, y);
            assert_eq!(tab_state(&by_key), tab_state(&by_click));
            assert_eq!(tab_state(&by_key).0.as_deref(), Some("web"));

            key(&mut by_key, KeyCode::Char('2'), KeyModifiers::NONE);
            assert_eq!(tab_state(&by_key).0.as_deref(), Some("demo"));

            key(&mut by_key, KeyCode::Char('9'), KeyModifiers::NONE);
            assert_eq!(tab_state(&by_key).0.as_deref(), Some("demo"));
            assert!(
                by_key
                    .flash
                    .as_deref()
                    .is_some_and(|f| f.contains("no project tab 9")),
                "{:?}",
                by_key.flash
            );

            let before = tab_state(&by_key);
            key(&mut by_key, KeyCode::Char('w'), KeyModifiers::NONE);
            assert!(by_key.overlay.is_none(), "w opened {:?}", by_key.overlay);
            assert_eq!(tab_state(&by_key), before);
        });
    }

    /// A key through the loop's own entry point, as the terminal delivers
    /// it — the view's cursor keeping runs around the handler there.
    fn key(app: &mut App, code: KeyCode, mods: KeyModifiers) -> Vec<ClientRequest> {
        key_kind(app, code, mods, crossterm::event::KeyEventKind::Press)
    }

    /// A key of any kind — the repeats and the release a host under the
    /// RELEASE WATCH's flags reports for a held key, as well as the press.
    fn key_kind(
        app: &mut App,
        code: KeyCode,
        mods: KeyModifiers,
        kind: crossterm::event::KeyEventKind,
    ) -> Vec<ClientRequest> {
        let mut out = Vec::new();
        let event = crossterm::event::Event::Key(crossterm::event::KeyEvent::new_with_kind(
            code, mods, kind,
        ));
        handle_terminal_event(app, event, &mut out);
        out
    }

    fn mouse(app: &mut App, kind: MouseEventKind, column: u16, row: u16) {
        let event = MouseEvent {
            kind,
            column,
            row,
            modifiers: KeyModifiers::NONE,
        };
        handle_terminal_event(app, crossterm::event::Event::Mouse(event), &mut Vec::new());
    }

    /// INPUT PARITY: the box's `[ ] new worktree` toggle is a button, and
    /// a click on it is the same flip `^N` is — one
    /// [`toggle_new_worktree`], reached both ways. A click off it is inert
    /// (it lands in the editor instead), so the state only moves when the
    /// toggle itself is hit.
    #[test]
    fn clicking_the_worktree_toggle_flips_it_as_ctrl_n_does() {
        with_default_config(|| {
            let mut app = two_sessions();
            key(&mut app, KeyCode::Char('p'), KeyModifiers::NONE);
            let fresh = |app: &App| launch(app).0.is_new_worktree();
            let start = fresh(&app);

            // `^N` first, so the click has a state to come back from.
            key(&mut app, KeyCode::Char('n'), KeyModifiers::CONTROL);
            assert_eq!(fresh(&app), !start, "^N flips it");

            let mut terminal = draw_at(&mut app, 140, 40);
            let toggle = match &app.overlay {
                Some(Overlay::Prompt(p)) => p.toggle_area,
                other => panic!("expected the box, got {other:?}"),
            };
            assert!(toggle.width > 0, "the toggle was drawn");
            assert!(
                buffer_text(&terminal).contains("new worktree"),
                "the toggle is on screen"
            );

            mouse(
                &mut app,
                MouseEventKind::Down(MouseButton::Left),
                toggle.x + 1,
                toggle.y,
            );
            assert_eq!(fresh(&app), start, "a click flips it back");

            // And again, so a click is not a one-way trip.
            terminal = draw_at(&mut app, 140, 40);
            let _ = &terminal;
            let toggle = match &app.overlay {
                Some(Overlay::Prompt(p)) => p.toggle_area,
                other => panic!("expected the box, got {other:?}"),
            };
            mouse(
                &mut app,
                MouseEventKind::Down(MouseButton::Left),
                toggle.x + 1,
                toggle.y,
            );
            assert_eq!(fresh(&app), !start, "and back again");

            // A click a row above the toggle is not the toggle.
            let before = fresh(&app);
            mouse(
                &mut app,
                MouseEventKind::Down(MouseButton::Left),
                toggle.x + 1,
                toggle.y - 1,
            );
            assert_eq!(fresh(&app), before, "only the toggle is the button");
        });
    }

    /// The columns `field` was drawn in on the box that is up.
    fn detail(app: &App, field: BoxField) -> ratatui::layout::Rect {
        let Some(Overlay::Prompt(prompt)) = &app.overlay else {
            panic!("expected the box, got {:?}", app.overlay);
        };
        prompt
            .detail_areas
            .iter()
            .find(|(drawn, _)| *drawn == field)
            .map(|(_, area)| *area)
            .unwrap_or_else(|| panic!("{field:?} was not drawn: {:?}", prompt.detail_areas))
    }

    /// INPUT PARITY: the box's four details — `project ^P`, `worktree ^T`,
    /// `agent Tab`, `model ^O` — are buttons as much as the toggle under
    /// them. A click on one opens exactly what its chord opens, down to
    /// the rows in it, and Esc hands the box back with what was typed
    /// either way. The air
    /// between two fields is not a button: a click there leaves the box up,
    /// as any other miss in it does.
    #[test]
    fn clicking_a_box_detail_opens_the_picker_its_chord_does() {
        with_default_config(|| {
            // What is up, in enough detail that two pickers of the same
            // shape cannot pass for one another. A field that refuses
            // (no model list, a project the box is bound to) leaves the
            // box up with a flash, and that is a shape too — the click
            // has to be refused exactly as the chord is.
            fn shape(app: &App) -> String {
                match &app.overlay {
                    Some(Overlay::ProjectPicker(picker)) => format!(
                        "projects[{}]: {}",
                        picker.selected,
                        picker
                            .projects
                            .iter()
                            .map(|p| p.name.as_str())
                            .collect::<Vec<_>>()
                            .join(",")
                    ),
                    Some(Overlay::Menu(menu)) => format!(
                        "menu {:?}: {}",
                        menu.title,
                        menu.items
                            .iter()
                            .map(|i| match i.label.split_once("+ new worktree  ") {
                                // The fresh row's branch is minted anew
                                // for every picker: the row, not the name.
                                Some((before, _)) => format!("{before}+ new worktree  <minted>"),
                                None => i.label.clone(),
                            })
                            .collect::<Vec<_>>()
                            .join(",")
                    ),
                    Some(Overlay::Prompt(_)) => format!("box: {:?}", app.flash),
                    other => panic!("expected a picker or the box, got {other:?}"),
                }
            }

            const TYPED: &str = "ship it";
            let opened = |app: &mut App| {
                key(app, KeyCode::Char('p'), KeyModifiers::NONE);
                type_text(app, TYPED);
                assert_eq!(launch(app).1, TYPED);
            };

            for (field, code, mods) in [
                (BoxField::Project, KeyCode::Char('p'), KeyModifiers::CONTROL),
                (
                    BoxField::Worktree,
                    KeyCode::Char('t'),
                    KeyModifiers::CONTROL,
                ),
                (BoxField::Agent, KeyCode::Tab, KeyModifiers::NONE),
                (BoxField::Model, KeyCode::Char('o'), KeyModifiers::CONTROL),
            ] {
                let mut by_key = two_sessions();
                opened(&mut by_key);
                key(&mut by_key, code, mods);
                let want = shape(&by_key);

                let mut by_click = two_sessions();
                opened(&mut by_click);
                draw_at(&mut by_click, 140, 40);
                let area = detail(&by_click, field);
                assert!(area.width > 0, "{field:?} was drawn with no columns");
                mouse(
                    &mut by_click,
                    MouseEventKind::Down(MouseButton::Left),
                    area.x + 1,
                    area.y,
                );
                assert_eq!(
                    shape(&by_click),
                    want,
                    "{field:?}: the click opened something other than the chord does"
                );

                // And the trip back: the box returns with what was typed,
                // whichever way it was left.
                key(&mut by_click, KeyCode::Esc, KeyModifiers::NONE);
                assert_eq!(
                    launch(&by_click).1,
                    TYPED,
                    "{field:?}: the click lost the text on the way there"
                );
            }

            // The gap between two fields is not either of them.
            let mut app = two_sessions();
            opened(&mut app);
            draw_at(&mut app, 140, 40);
            let project = detail(&app, BoxField::Project);
            mouse(
                &mut app,
                MouseEventKind::Down(MouseButton::Left),
                project.x + project.width,
                project.y,
            );
            assert_eq!(
                launch(&app).1,
                TYPED,
                "a click in the air between two fields left the box up"
            );
        });
    }

    /// A shell TERMINAL in checkout `worktree`. Terminals are the
    /// checkout's, not the session's, which is the whole reason the PANE's
    /// TAB STRIP names the branch beside them.
    fn seed_terminal(app: &mut App, id: &str, worktree: &str, name: &str) {
        hse(
            app,
            ServerEvent::EntityUpserted {
                entity: Entity::Terminal(TerminalTab {
                    id: TerminalId(id.into()),
                    worktree_id: WorktreeId(worktree.into()),
                    name: name.into(),
                    sort_order: 0,
                    alive: true,
                    run_command: None,
                }),
            },
        );
    }

    /// What the PANE is reading, as its tab strip would have to agree.
    fn reading(app: &App) -> Option<SessionRef> {
        app.term.as_ref().map(|t| t.sref.clone())
    }

    /// Where the draw put the tab for `hit`.
    fn tab_at(app: &App, hit: HitTarget) -> ratatui::layout::Rect {
        app.hits
            .iter()
            .find(|(_, h)| *h == hit)
            .map(|(r, _)| *r)
            .unwrap_or_else(|| panic!("no tab was registered for {hit:?}"))
    }

    /// The PANE's header row, as the last draw left it.
    fn head_row(app: &App, terminal: &Terminal<TestBackend>) -> String {
        let body = app.launcher_body;
        let pane_h =
            crate::launcher::pane_height(body, app.launcher_pane_h).expect("a pane worth drawing");
        let y = body.y + body.height - pane_h + 1;
        buffer_text(terminal)
            .lines()
            .nth(y as usize)
            .unwrap_or_default()
            .to_string()
    }

    /// Walk the cursor onto the card at `index` — its place in the flat
    /// `launcher::rows` list — with the keys: `j`/`k` along the bands to
    /// its checkout, Enter into it, then `h`/`l`/`j`/`k` across its
    /// cards ([`step_grid`]), as many presses as it takes — for a test
    /// that is about the KEYS walking the grid, where a pointer landing
    /// on the card would be a different ask.
    fn walk_to(app: &mut App, index: usize) {
        let mut out = Vec::new();
        let id = crate::launcher::rows(app)[index].agent.id.clone();
        let want = SessionRef::Agent(id);
        for _ in 0..32 {
            let bands = crate::launcher::bands(app);
            let target = bands
                .iter()
                .position(|b| b.position(&want).is_some())
                .expect("the card is on the grid");
            let on = crate::launcher::band_cursor(app, &bands).expect("the cursor is on a band");
            if on != target {
                super::step_grid(app, 0, (target as i64 - on as i64).signum(), &mut out);
                continue;
            }
            if app.launcher_expanded.as_ref() != Some(&bands[target].worktree) {
                super::toggle_band_expand(app, &mut out);
                continue;
            }
            let band = &bands[target];
            let at = crate::launcher::card_cursor(app, band).expect("the cursor is on a card");
            let to = band.position(&want).expect("the card is in the band");
            if at == to {
                return;
            }
            let layout = crate::launcher::expanded_layout(app.body_area, band);
            let (row_at, col_at) = layout.row_of(at).expect("a row");
            let (row_to, col_to) = layout.row_of(to).expect("a row");
            let (dx, dy) = if row_at != row_to {
                (0, (row_to as i64 - row_at as i64).signum())
            } else {
                ((col_to as i64 - col_at as i64).signum(), 0)
            };
            super::step_grid(app, dx, dy, &mut out);
        }
        panic!("the keys never reached card {index}");
    }

    /// INPUT PARITY: the right end of the PANE's header is its CLOSE
    /// BUTTON, not the `INPUT` tag the panels' pane wears while locked —
    /// and one click on it folds the pane away exactly as `^`` pressed
    /// twice from inside does (out to the card, then the fold), keys and
    /// all.
    #[test]
    fn the_close_button_on_the_pane_folds_it_like_the_key() {
        with_default_config(|| {
            let locked = || {
                let mut app = two_sessions();
                draw(&mut app);
                key(&mut app, KeyCode::Char('h'), KeyModifiers::NONE);
                key(&mut app, KeyCode::Enter, KeyModifiers::NONE);
                if app.term.is_none() {
                    app.term = Some(crate::app::AttachedTerm::new(
                        SessionRef::Agent(AgentId("a2".into())),
                        40,
                        10,
                    ));
                }
                assert!(app.term_locked, "Enter put the keys in the pane");
                app
            };

            let mut by_click = locked();
            let terminal = draw(&mut by_click);
            let head = head_row(&by_click, &terminal);
            assert!(!head.contains("INPUT"), "no lock tag on the strip: {head}");
            assert!(
                head.trim_end().ends_with('×'),
                "the close button ends the row: {head}"
            );
            let close = tab_at(&by_click, HitTarget::LauncherPaneClose);
            mouse(
                &mut by_click,
                MouseEventKind::Down(MouseButton::Left),
                close.x + 1,
                close.y,
            );

            let mut by_key = locked();
            draw(&mut by_key);
            key(&mut by_key, KeyCode::Char('`'), KeyModifiers::CONTROL);
            key(&mut by_key, KeyCode::Char('`'), KeyModifiers::CONTROL);

            for app in [&by_click, &by_key] {
                assert!(app.launcher_pane_hidden, "the pane folded away");
                assert_eq!(app.focus, Focus::Sessions, "the keys are the cards'");
                assert!(!app.term_locked);
            }
            assert_eq!(by_click.launcher_unaimed, by_key.launcher_unaimed);
            assert_eq!(by_click.flash, by_key.flash);
        });
    }

    /// `d` follows the cursor: on a TERMINAL's chip it closes that
    /// terminal, and on a session's card it is the card's delete — the
    /// card is never deleted by a key aimed at a shell.
    #[test]
    fn d_closes_the_terminal_the_strip_is_on_and_the_card_otherwise() {
        with_default_config(|| {
            let mut app = two_sessions();
            seed_terminal(&mut app, "t1", "w2", "shell-1");
            draw(&mut app);
            to_feat(&mut app);

            // On the card, `d` is the card's.
            key(&mut app, KeyCode::Char('d'), KeyModifiers::NONE);
            assert!(
                matches!(&app.overlay, Some(Overlay::Confirm(c))
                    if matches!(c.action, PendingAction::DeleteAgent(_))),
                "the card's delete: {:?}",
                app.overlay
            );
            key(&mut app, KeyCode::Esc, KeyModifiers::NONE);

            // On the terminal's chip it is the terminal's.
            key(&mut app, KeyCode::Char('`'), KeyModifiers::NONE);
            key(&mut app, KeyCode::Char('d'), KeyModifiers::NONE);
            assert!(
                matches!(&app.overlay, Some(Overlay::Confirm(c))
                    if matches!(c.action, PendingAction::CloseTerminal(_))),
                "the terminal's close: {:?}",
                app.overlay
            );
        });
    }

    /// [`two_sessions`] with **Show all worktrees** on and a third
    /// checkout of `demo` — `idle`, with nothing running in it — its
    /// EMPTY BAND the grid's last, under `feat`'s.
    fn with_empty_band() -> App {
        let mut app = two_sessions();
        app.show_all_worktrees = true;
        hse(
            &mut app,
            ServerEvent::EntityUpserted {
                entity: Entity::Worktree(Worktree {
                    id: WorktreeId("w3".into()),
                    project_id: ProjectId("p1".into()),
                    path: "/tmp/demo-idle".into(),
                    branch: "idle".into(),
                    is_main: false,
                    sort_order: 2,
                }),
            },
        );
        app
    }

    /// Tall enough for all three of [`with_empty_band`]'s bands over
    /// the pane.
    fn draw_tall(app: &mut App) -> Terminal<TestBackend> {
        draw_at(app, 130, 70)
    }

    fn screen_text(terminal: &Terminal<TestBackend>) -> String {
        let buf = terminal.backend().buffer();
        (0..buf.area.height)
            .map(|y| {
                (0..buf.area.width)
                    .filter_map(|x| buf.cell((x, y)))
                    .map(|c| c.symbol())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Where the EMPTY BAND `index` was drawn: the band's whole area.
    fn band_area(app: &App, index: usize) -> ratatui::layout::Rect {
        app.hits
            .iter()
            .find(|(_, hit)| *hit == HitTarget::LauncherBand(index))
            .map(|(rect, _)| *rect)
            .unwrap_or_else(|| panic!("band {index} was not drawn"))
    }

    /// With **Show all worktrees** on a checkout with nothing running is
    /// on the grid as an EMPTY BAND — its branch on the rule, and under
    /// it what can be done there — which `j` walks onto like any band.
    /// Off, the same tree has no such band.
    #[test]
    fn an_empty_worktree_is_a_band_the_cursor_walks_onto() {
        with_default_config(|| {
            let mut off = with_empty_band();
            off.show_all_worktrees = false;
            let screen = screen_text(&draw_tall(&mut off));
            assert!(!screen.contains("idle"), "{screen}");

            let mut app = with_empty_band();
            let screen = screen_text(&draw_tall(&mut app));
            assert!(screen.contains("idle"), "{screen}");
            assert!(
                screen.contains(
                    "nothing running · p: new session · t: terminal · d: delete worktree"
                ),
                "{screen}"
            );
            keys(&mut app, &[KeyCode::Char('j'), KeyCode::Char('j')]);
            assert_eq!(
                app.selected_worktree().map(|w| w.id.clone()),
                Some(WorktreeId("w3".into()))
            );
            assert!(app.selected_session_row().is_none(), "no card to be on");
            // Past the last band nothing moves, and back up is `feat`.
            key(&mut app, KeyCode::Char('j'), KeyModifiers::NONE);
            assert_eq!(
                app.selected_worktree().map(|w| w.id.clone()),
                Some(WorktreeId("w3".into()))
            );
            key(&mut app, KeyCode::Char('k'), KeyModifiers::NONE);
            assert_eq!(
                app.selected_worktree().map(|w| w.id.clone()),
                Some(WorktreeId("w2".into()))
            );
            // Tab has no cards to open, and says so.
            key(&mut app, KeyCode::Char('j'), KeyModifiers::NONE);
            key(&mut app, KeyCode::Tab, KeyModifiers::NONE);
            assert_eq!(app.launcher_expanded, None);
            assert_eq!(app.flash.as_deref(), Some(super::NO_SESSIONS));
        });
    }

    /// INPUT PARITY: `d` on an EMPTY BAND and **Delete worktree** in its
    /// right-click menu open the one confirm — the worktree's own, the
    /// same `activate::delete_worktree` asks — and nothing is deleted
    /// before it is answered.
    #[test]
    fn d_and_the_menu_on_an_empty_band_ask_to_delete_the_worktree() {
        with_default_config(|| {
            let is_worktree_confirm = |app: &App| {
                matches!(&app.overlay, Some(Overlay::Confirm(c))
                    if c.action == PendingAction::DeleteWorktree(WorktreeId("w3".into())))
            };

            let mut by_key = with_empty_band();
            draw_tall(&mut by_key);
            keys(&mut by_key, &[KeyCode::Char('j'), KeyCode::Char('j')]);
            let sent = key(&mut by_key, KeyCode::Char('d'), KeyModifiers::NONE);
            assert!(is_worktree_confirm(&by_key), "{:?}", by_key.overlay);
            assert!(sent.is_empty(), "asked first: {sent:?}");

            let mut by_click = with_empty_band();
            draw_tall(&mut by_click);
            let area = band_area(&by_click, 2);
            mouse(
                &mut by_click,
                MouseEventKind::Down(MouseButton::Right),
                area.x + 4,
                area.y + 1,
            );
            let at = match &by_click.overlay {
                Some(Overlay::Menu(menu)) => menu
                    .items
                    .iter()
                    .position(|i| i.label == "Delete worktree")
                    .unwrap_or_else(|| panic!("no Delete worktree in {menu:?}")),
                other => panic!("expected the worktree's menu, got {other:?}"),
            };
            for _ in 0..at {
                key(&mut by_click, KeyCode::Down, KeyModifiers::NONE);
            }
            key(&mut by_click, KeyCode::Enter, KeyModifiers::NONE);
            assert!(is_worktree_confirm(&by_click), "{:?}", by_click.overlay);
            let id = |app: &App| app.selected_worktree().map(|w| w.id.clone());
            assert_eq!(id(&by_click), id(&by_key));
        });
    }

    /// With **Show all worktrees** on, deleting a worktree's last card
    /// never offers the worktree: the card's ordinary confirm, a delete
    /// of the card alone, and its band stays on the grid — empty, the
    /// cursor on it — for `d` to delete when that is wanted.
    #[test]
    fn deleting_the_last_card_keeps_the_worktree_with_show_all_worktrees() {
        with_default_config(|| {
            let mut app = with_empty_band();
            draw_tall(&mut app);
            to_feat(&mut app);
            key(&mut app, KeyCode::Char('d'), KeyModifiers::NONE);
            match &app.overlay {
                Some(Overlay::Confirm(c)) => {
                    assert!(
                        matches!(c.action, PendingAction::DeleteAgent(_)),
                        "{:?}",
                        c.action
                    );
                    assert!(!c.message.contains("worktree"), "{}", c.message);
                }
                other => panic!("expected the card's confirm, got {other:?}"),
            }
            let sent = key(&mut app, KeyCode::Enter, KeyModifiers::NONE);
            assert!(
                sent.iter()
                    .any(|r| matches!(r, ClientRequest::DeleteAgent { .. })),
                "{sent:?}"
            );
            assert!(
                !sent
                    .iter()
                    .any(|r| matches!(r, ClientRequest::DeleteWorktree { .. })),
                "{sent:?}"
            );
            let bands = crate::launcher::bands(&app);
            let feat = bands
                .iter()
                .position(|b| b.worktree == WorktreeId("w2".into()))
                .expect("feat's band stays");
            assert!(bands[feat].cards.is_empty());
            assert_eq!(
                app.selected_worktree().map(|w| w.id.clone()),
                Some(WorktreeId("w2".into())),
                "the cursor stays on the emptied band"
            );
            draw_tall(&mut app);
        });
    }

    /// Space on a card opens the FOLLOW-UP MODAL over the grid, aimed at
    /// the card under the cursor — and touches nothing else: the PANE goes
    /// on reading what it was reading, at the size it was, with the keys
    /// still on the cards.
    #[test]
    fn space_opens_the_follow_up_modal_over_the_grid() {
        with_default_config(|| {
            let mut app = two_sessions();
            draw(&mut app);
            let (rows, reading, focus) = (app.term_area, pane(&app), app.focus);
            let id = app.selected_session().map(|a| a.id.clone()).unwrap();
            let name = app.selected_session().map(|a| a.name.clone()).unwrap();

            key(&mut app, KeyCode::Char(' '), KeyModifiers::NONE);
            assert!(
                matches!(&app.overlay, Some(Overlay::Prompt(p))
                    if p.kind == PromptKind::FollowUp { id: id.clone() }),
                "the box is aimed at the card under the cursor: {:?}",
                app.overlay
            );

            let text = buffer_text(&draw(&mut app));
            assert!(
                text.contains(&format!("Follow-up · {name}")),
                "the modal names the session it will prompt:\n{text}"
            );
            assert_eq!(app.term_area, rows, "the pane kept its rows");
            assert_eq!(pane(&app), reading, "and went on reading the same thing");
            assert_eq!(app.focus, focus, "the keys never left the cards");
        });
    }

    /// Enter sends what was typed down that session's PTY as its next turn
    /// — the text, then the carriage return, two Inputs so the child reads
    /// the prompt before the Enter — and the box closes onto the grid.
    #[test]
    fn the_modal_sends_the_turn_and_closes() {
        with_default_config(|| {
            let mut app = two_sessions();
            draw(&mut app);
            let id = app.selected_session().map(|a| a.id.clone()).unwrap();
            let name = app.selected_session().map(|a| a.name.clone()).unwrap();

            key(&mut app, KeyCode::Char(' '), KeyModifiers::NONE);
            type_text(&mut app, "rebase onto main");
            let out = key(&mut app, KeyCode::Enter, KeyModifiers::NONE);

            assert_eq!(
                inputs_to(&out, &id),
                vec![b"rebase onto main".to_vec(), b"\r".to_vec()],
                "the prompt, then the Enter that submits it: {out:?}"
            );
            assert!(app.overlay.is_none(), "the box closed: {:?}", app.overlay);
            assert_eq!(
                app.flash.as_deref(),
                Some(format!("sent to {name}").as_str())
            );
        });
    }

    /// The point of the modal: hand one card after another its next turn
    /// without ever stepping into a session. The pane is never attached to,
    /// never unfolded and never focused — with it folded away entirely the
    /// turns still go out.
    #[test]
    fn cards_can_be_prompted_one_after_another_without_opening_the_pane() {
        with_default_config(|| {
            let mut app = three_sessions();
            draw(&mut app);
            key(&mut app, KeyCode::Char('~'), KeyModifiers::NONE);
            assert!(app.launcher_pane_hidden, "the pane is folded away");

            let mut sent = Vec::new();
            for (turn, step) in [("first", 'h'), ("second", 'l')] {
                // Along the row to the other card of the root band (which
                // takes the aim back, the fold having let it go), then
                // prompt it.
                key(&mut app, KeyCode::Char(step), KeyModifiers::NONE);
                let id = app.selected_session().map(|a| a.id.clone()).unwrap();
                key(&mut app, KeyCode::Char(' '), KeyModifiers::NONE);
                type_text(&mut app, turn);
                let out = key(&mut app, KeyCode::Enter, KeyModifiers::NONE);
                assert_eq!(
                    inputs_to(&out, &id),
                    vec![turn.as_bytes().to_vec(), b"\r".to_vec()],
                    "{turn} went to the card it was typed on"
                );
                assert!(
                    !out.iter()
                        .any(|r| matches!(r, ClientRequest::Attach { .. })),
                    "{turn} attached a pane nobody asked for: {out:?}"
                );
                sent.push(id);
            }
            assert_ne!(sent[0], sent[1], "two different cards were prompted");
            assert!(app.launcher_pane_hidden, "and the pane stayed folded away");
            assert_eq!(app.focus, Focus::Sessions, "the keys never left the cards");
        });
    }

    /// Esc closes the box without sending, and leaves the grid where it
    /// was.
    #[test]
    fn esc_closes_the_modal_without_sending() {
        with_default_config(|| {
            let mut app = two_sessions();
            draw(&mut app);
            key(&mut app, KeyCode::Char(' '), KeyModifiers::NONE);
            type_text(&mut app, "never mind");

            let out = key(&mut app, KeyCode::Esc, KeyModifiers::NONE);
            assert!(app.overlay.is_none(), "the box closed");
            assert!(
                !out.iter().any(|r| matches!(r, ClientRequest::Input { .. })),
                "nothing was sent: {out:?}"
            );
            assert_eq!(
                app.selected_project().map(|p| p.name.as_str()),
                Some("demo"),
                "and the grid stayed where it was"
            );
        });
    }

    /// INPUT PARITY: **Follow-up prompt** in the card's right-click menu
    /// opens the same modal Space does.
    #[test]
    fn the_menu_row_opens_the_same_modal_space_does() {
        with_default_config(|| {
            let mut app = two_sessions();
            draw(&mut app);
            let id = app.selected_session().map(|a| a.id.clone()).unwrap();

            right_click_card(&mut app);
            let at = match &app.overlay {
                Some(Overlay::Menu(menu)) => menu
                    .items
                    .iter()
                    .position(|i| i.label == "Follow-up prompt")
                    .expect("the row is on the card's menu"),
                other => panic!("expected the menu, got {other:?}"),
            };
            for _ in 0..at {
                key(&mut app, KeyCode::Down, KeyModifiers::NONE);
            }
            key(&mut app, KeyCode::Enter, KeyModifiers::NONE);

            assert!(
                matches!(&app.overlay, Some(Overlay::Prompt(p))
                    if p.kind == PromptKind::FollowUp { id: id.clone() }),
                "the menu row opened the box too: {:?}",
                app.overlay
            );
        });
    }

    /// A row that takes no follow-up says why rather than opening a box
    /// over it — the SESSIONS PANEL's own message, from the one
    /// `activate::no_follow_up` both composers ask.
    #[test]
    fn a_cloud_card_says_what_it_takes_instead() {
        with_default_config(|| {
            let mut app = two_sessions();
            draw(&mut app);
            let id = app.selected_session().map(|a| a.id.clone()).unwrap();
            if let Some(a) = app.tree.agents.iter_mut().find(|a| a.id == id) {
                a.cloud_session_id = Some("cs-1".into());
            }
            key(&mut app, KeyCode::Char(' '), KeyModifiers::NONE);
            assert!(app.overlay.is_none(), "no box over a cloud session");
            assert_eq!(
                app.flash.as_deref(),
                Some("cloud sessions take a queued message — right-click, then Send to cloud session"),
            );
        });
    }

    /// The Inputs `out` carries for session `id`, in order.
    fn inputs_to(out: &[ClientRequest], id: &AgentId) -> Vec<Vec<u8>> {
        out.iter()
            .filter_map(|r| match r {
                ClientRequest::Input { session, data }
                    if session == &SessionRef::Agent(id.clone()) =>
                {
                    Some(data.clone())
                }
                _ => None,
            })
            .collect()
    }

    /// The branch in the box's header, as drawn.
    fn branch_button(app: &App) -> ratatui::layout::Rect {
        match &app.overlay {
            Some(Overlay::Prompt(p)) => p.branch_area,
            other => panic!("expected the box, got {other:?}"),
        }
    }

    /// The WORKTREE PICKER's rows and the one under its cursor.
    fn picker_rows(app: &App) -> (Vec<String>, usize) {
        match &app.overlay {
            Some(Overlay::Menu(m)) if m.title.as_deref() == Some("Worktree") => {
                (m.items.iter().map(|i| i.label.clone()).collect(), m.hover)
            }
            other => panic!("expected the worktree picker, got {other:?}"),
        }
    }

    /// Click the branch, then draw, so the picker's rows are clickable.
    fn open_worktree_picker(app: &mut App) {
        draw_at(app, 140, 40);
        let button = branch_button(app);
        assert!(button.width > 0, "the branch was drawn as a button");
        mouse(
            app,
            MouseEventKind::Down(MouseButton::Left),
            button.x + 1,
            button.y,
        );
        draw_at(app, 140, 40);
    }

    /// Click the picker's row that starts with `label`, as drawn.
    fn click_picker_row(app: &mut App, label: &str) {
        let (area, index) = match &app.overlay {
            Some(Overlay::Menu(m)) => (
                m.area,
                m.items
                    .iter()
                    .position(|i| i.label.starts_with(label))
                    .unwrap_or_else(|| panic!("no {label:?} row")),
            ),
            other => panic!("expected the worktree picker, got {other:?}"),
        };
        mouse(
            app,
            MouseEventKind::Down(MouseButton::Left),
            area.x + 2,
            area.y + 1 + index as u16,
        );
    }

    /// A click on the branch in the box's header (`^T` opens the same, the
    /// parity test below says so) opens the WORKTREE PICKER over the box,
    /// hung right under the branch: a fresh worktree first,
    /// then every checkout of the box's project — the root first, nothing
    /// from another project — with the one the box is aimed at ticked and
    /// under the cursor. A pick aims the launch there and hands the box
    /// back with the task and the harness kept; the crumb follows it. It
    /// picks where the session runs: no checkout changes branch.
    #[test]
    fn clicking_the_branch_picks_the_worktree_the_launch_runs_in() {
        with_default_config(|| {
            let mut app = two_sessions();
            key(&mut app, KeyCode::Char('p'), KeyModifiers::NONE);
            type_text(&mut app, "fix the nav");
            let (start, _) = launch(&app);
            assert_eq!(
                start.target,
                QuickTarget::Worktree(WorktreeId("w1".into())),
                "the box opens on the root"
            );

            open_worktree_picker(&mut app);
            let (rows, hover) = picker_rows(&app);
            assert!(rows[0].starts_with("+ new worktree  "), "{rows:?}");
            assert!(!rows[0].ends_with(" ✓"), "{rows:?}");
            assert_eq!(rows[1..], ["main  (root) ✓", "feat"], "{rows:?}");
            assert_eq!(hover, 1, "the cursor starts on the ✓");

            // Over the box, which stays on screen behind it.
            let text = buffer_text(&draw_at(&mut app, 140, 40));
            assert!(text.contains("New session"), "the box's title: {text}");
            assert!(text.contains("fix the nav"), "the task: {text}");
            assert!(text.contains("+ new worktree"), "the picker: {text}");

            click_picker_row(&mut app, "feat");
            let (after, text) = launch(&app);
            assert_eq!(after.target, QuickTarget::Worktree(WorktreeId("w2".into())));
            assert_eq!(text, "fix the nav", "the task survives the trip");
            assert_eq!(
                (after.kind, &after.model, &after.effort),
                (start.kind, &start.model, &start.effort),
                "the harness is not the picker's to change"
            );
            let text = buffer_text(&draw_at(&mut app, 140, 40));
            assert!(
                text.contains("worktree feat ^T"),
                "the details row follows: {text}"
            );
            assert!(
                app.tree
                    .worktrees
                    .iter()
                    .any(|w| w.id.0 == "w2" && w.branch == "feat"),
                "the checkout keeps its branch"
            );

            // Opened again it ticks the checkout; Esc hands the box back
            // as it was.
            open_worktree_picker(&mut app);
            let (rows, hover) = picker_rows(&app);
            assert_eq!(rows[hover], "feat ✓");
            key(&mut app, KeyCode::Esc, KeyModifiers::NONE);
            let (kept, text) = launch(&app);
            assert_eq!(kept.target, after.target);
            assert_eq!(text, "fix the nav");

            // And the fresh row cuts a worktree again.
            open_worktree_picker(&mut app);
            click_picker_row(&mut app, "+ new worktree");
            let (fresh, text) = launch(&app);
            assert!(
                matches!(&fresh.target, QuickTarget::NewWorktree { project, .. } if project.0 == "p1"),
                "{:?}",
                fresh.target
            );
            assert_eq!(text, "fix the nav");
        });
    }

    /// The picker hangs from the branch it was opened on — its top edge on
    /// the row under the crumb, its rows' text in the branch's column —
    /// and keeps hanging from it when the terminal is resized under it:
    /// the anchor is where the box draws its branch this frame, not where
    /// the click happened to land.
    #[test]
    fn the_worktree_picker_hangs_under_the_branch() {
        with_default_config(|| {
            let mut app = two_sessions();
            key(&mut app, KeyCode::Char('p'), KeyModifiers::NONE);
            let mut buttons = Vec::new();
            for (w, h) in [(140, 40), (110, 30)] {
                draw_at(&mut app, w, h);
                buttons.push(((w, h), branch_button(&app)));
            }
            // Opened at 140x40 (the helper's size), then drawn at each.
            open_worktree_picker(&mut app);
            for ((w, h), button) in buttons {
                draw_at(&mut app, w, h);
                let area = match &app.overlay {
                    Some(Overlay::Menu(m)) => m.area,
                    other => panic!("expected the worktree picker, got {other:?}"),
                };
                assert_eq!(area.y, button.y + 1, "{w}x{h}: {area:?} under {button:?}");
                assert_eq!(area.x + 2, button.x, "{w}x{h}: {area:?} on {button:?}");
            }
        });
    }

    fn selected(app: &App) -> Option<String> {
        app.selected_session().map(|a| a.id.0)
    }

    /// The grid's cards, in the order they are laid out.
    fn cards(app: &App) -> Vec<String> {
        crate::launcher::rows(app)
            .iter()
            .map(|row| row.agent.id.0.clone())
            .collect()
    }

    fn pane(app: &App) -> Option<SessionRef> {
        app.term.as_ref().map(|t| t.sref.clone())
    }

    fn launch(app: &App) -> (QuickLaunch, String) {
        match &app.overlay {
            Some(Overlay::Prompt(prompt)) => match &prompt.kind {
                PromptKind::QuickPrompt(launch) => {
                    (launch.clone(), prompt.input.as_str().to_string())
                }
                other => panic!("expected the box, got {other:?}"),
            },
            other => panic!("expected the box, got {other:?}"),
        }
    }

    fn type_text(app: &mut App, text: &str) {
        for c in text.chars() {
            key(app, KeyCode::Char(c), KeyModifiers::NONE);
        }
    }

    /// A cell inside the card of the session at `index` in the flat
    /// `launcher::rows` list, as drawn.
    fn row_cell(app: &App, index: usize) -> (u16, u16) {
        let id = crate::launcher::rows(app)[index].agent.id.clone();
        card_cell(app, &SessionRef::Agent(id))
    }

    /// A right-click on the card under the cursor: its CONTEXT MENU.
    fn right_click_card(app: &mut App) {
        let id = app.selected_session().expect("a card under the cursor").id;
        let (x, y) = card_cell(app, &SessionRef::Agent(id));
        mouse(app, MouseEventKind::Down(MouseButton::Right), x, y);
    }

    /// A cell inside the card of `sref`, as drawn — whichever band it is
    /// in, at either level of the grid.
    fn card_cell(app: &App, sref: &SessionRef) -> (u16, u16) {
        let bands = crate::launcher::bands(app);
        let (rect, _) = app
            .hits
            .iter()
            .find(|(_, hit)| match hit {
                HitTarget::LauncherCard(at) => {
                    crate::launcher::card_at(&bands, *at).is_some_and(|c| &c.sref() == sref)
                }
                _ => false,
            })
            .unwrap_or_else(|| panic!("{sref:?}'s card was not drawn"));
        (rect.x + 3, rect.y + 1)
    }

    /// nebula opens on the GRID and on nothing else: no snapshot puts a
    /// modal up, not the first one and not the one that brings a first
    /// run's first project. The box is a key away — `p` opens it, aimed at
    /// the selected project and its existing checkout (the new-worktree
    /// SETTING is off by default), with the harness the settings name.
    #[test]
    fn no_modal_opens_at_boot() {
        with_default_config(|| {
            let mut app = App::new();
            seed_tree(&mut app);
            assert!(app.overlay.is_none(), "the first snapshot opens nothing");
            seed_web(&mut app);
            assert!(app.overlay.is_none(), "nor any later one");

            key(&mut app, KeyCode::Char('p'), KeyModifiers::NONE);
            let (launch, text) = launch(&app);
            assert!(text.is_empty());
            assert!(matches!(launch.target, QuickTarget::Worktree(_)));
            assert_eq!(
                crate::launcher::project_of(&app, &launch.target).map(|p| p.0),
                Some("p1".to_string())
            );
            assert_eq!(launch.kind, AgentKind::Claude, "the settings' harness");
        });
    }

    /// The grid is the BANDS: `j`/`k` walk the checkouts — the pane
    /// swapping onto each band's remembered card as the cursor passes,
    /// as ↑/↓ down the SESSIONS PANEL previews a row — and `h`/`l` walk
    /// the cards along a band. Tab opens the band under the cursor as
    /// the ACCORDION, its cards wrapped into rows in place with the
    /// cursor where it was, and Tab or Esc closes it again, the band
    /// still aimed at; opening another closes it, one open at a time.
    /// The grid's cursor is the panels' selection, so the verbs that read
    /// it name the same session.
    #[test]
    fn hjkl_walk_the_grid_and_the_pane_follows() {
        with_default_config(|| {
            let mut app = two_sessions();
            draw(&mut app);
            assert_eq!(app.focus, Focus::Sessions, "the grid has the keys");
            assert_eq!(app.launcher_expanded, None, "every band collapsed");
            assert_eq!(
                selected(&app).as_deref(),
                Some("a1"),
                "the cursor starts on the selected session — the root band's"
            );

            // Alone in its band, the card has nowhere to walk.
            key(&mut app, KeyCode::Char('h'), KeyModifiers::NONE);
            key(&mut app, KeyCode::Char('l'), KeyModifiers::NONE);
            assert_eq!(selected(&app).as_deref(), Some("a1"));

            key(&mut app, KeyCode::Char('j'), KeyModifiers::NONE);
            assert_eq!(selected(&app).as_deref(), Some("a2"), "the next band down");
            assert_eq!(
                pane(&app),
                Some(SessionRef::Agent(AgentId("a2".into()))),
                "the band walked onto is what the pane reads"
            );
            assert_eq!(
                app.selected_project().map(|p| p.name.as_str()),
                Some("demo"),
                "the walk stays inside the project the grid is scoped to"
            );
            key(&mut app, KeyCode::Char('j'), KeyModifiers::NONE);
            assert_eq!(selected(&app).as_deref(), Some("a2"), "the last band stays");

            app.flash = None;
            key(&mut app, KeyCode::Char('h'), KeyModifiers::NONE);
            assert_eq!(
                selected(&app).as_deref(),
                Some("a2"),
                "h walks the band's cards, and this band has the one"
            );
            assert_eq!(app.launcher_expanded, None, "without opening it");
            assert_eq!(app.flash, None, "and says nothing of it");

            key(&mut app, KeyCode::Char('k'), KeyModifiers::NONE);
            assert_eq!(selected(&app).as_deref(), Some("a1"));
            assert_eq!(
                pane(&app),
                Some(SessionRef::Agent(AgentId("a1".into()))),
                "and swaps with the cursor"
            );
            key(&mut app, KeyCode::Char('k'), KeyModifiers::NONE);
            assert_eq!(
                selected(&app).as_deref(),
                Some("a1"),
                "the first band stays"
            );
            assert_eq!(app.launcher_expanded, None);

            // Tab opens the band under the cursor in place, the cursor on
            // the card the pane was reading and the keys still the grid's;
            // Tab again closes it, the band still aimed at.
            let (root, feat) = (WorktreeId("w1".into()), WorktreeId("w2".into()));
            key(&mut app, KeyCode::Tab, KeyModifiers::NONE);
            assert_eq!(
                app.launcher_expanded,
                Some(root.clone()),
                "the root band is open"
            );
            assert_eq!(selected(&app).as_deref(), Some("a1"));
            assert_eq!(app.focus, Focus::Sessions, "the keys stay on the grid");
            key(&mut app, KeyCode::Tab, KeyModifiers::NONE);
            assert_eq!(app.launcher_expanded, None, "Tab again closes it");
            assert!(!app.launcher_unaimed, "with the band still aimed at");
            assert_eq!(selected(&app).as_deref(), Some("a1"));

            // Esc closes it too, and only then lets the aim go.
            key(&mut app, KeyCode::Tab, KeyModifiers::NONE);
            key(&mut app, KeyCode::Esc, KeyModifiers::NONE);
            assert_eq!(app.launcher_expanded, None, "Esc closes the open band");
            assert!(!app.launcher_unaimed, "the band still aimed at");

            // One band open at a time: Tab on another closes the first.
            key(&mut app, KeyCode::Tab, KeyModifiers::NONE);
            key(&mut app, KeyCode::Char('j'), KeyModifiers::NONE);
            assert_eq!(
                selected(&app).as_deref(),
                Some("a2"),
                "j walks on past the open band"
            );
            assert_eq!(app.launcher_expanded, Some(root), "which stays open");
            key(&mut app, KeyCode::Tab, KeyModifiers::NONE);
            assert_eq!(
                app.launcher_expanded,
                Some(feat),
                "feat's band open, the root's closed"
            );
        });
    }

    /// On a collapsed band `h`/`l` walk the band's own cards — its
    /// sessions, then its terminals, as the row draws them — the pane
    /// following onto each and the band staying collapsed, stopping at
    /// the row's ends rather than wrapping. The row scrolls under the
    /// cursor to keep its card on screen and says with `❯` / `❮` beside
    /// it which way the rest went. INPUT PARITY: a click on either arrow
    /// is the same one-card step.
    #[test]
    fn h_and_l_walk_the_bands_cards_and_the_arrows_click_the_same_step() {
        with_default_config(|| {
            let mut app = two_sessions();
            for (id, name) in [("a4", "second"), ("a5", "third"), ("a6", "fourth")] {
                seed_running(&mut app, id, "w1", name);
            }
            seed_terminal(&mut app, "t1", "w1", "shell");
            draw(&mut app);
            assert_eq!(app.launcher_expanded, None, "the band is collapsed");
            let bands = crate::launcher::bands(&app);
            let cards: Vec<SessionRef> = bands[0].cards.iter().map(|c| c.sref()).collect();
            assert_eq!(cards.len(), 5, "four sessions and the terminal");
            let last = cards.len() - 1;
            let at = |app: &App| {
                let bands = crate::launcher::bands(app);
                crate::launcher::card_cursor(app, &bands[0])
            };

            // Back to the row's start: `h` stops there.
            for _ in 0..8 {
                key(&mut app, KeyCode::Char('h'), KeyModifiers::NONE);
            }
            assert_eq!(at(&app), Some(0), "h stops at the first card");
            assert!(app.launcher_expanded.is_none(), "and never goes in");
            let term = draw(&mut app);
            assert!(
                app.hit_rect(&HitTarget::LauncherStripLeft(0)).is_none(),
                "nothing to the left: no ❮"
            );
            let right = app
                .hit_rect(&HitTarget::LauncherStripRight(0))
                .expect("cards past the right edge: a ❯");
            // Beside the cards, on their rows: the button their full
            // height, the glyph on their middle line — on screen, not at
            // the row's place counted from the top of the panel.
            let card = app
                .hit_rect(&HitTarget::LauncherCard(CardRef { band: 0, card: 0 }))
                .expect("the first card");
            assert_eq!((right.y, right.height), (card.y, card.height));
            let mid = card.y + crate::launcher::CARD_H / 2;
            assert_eq!(term.backend().buffer()[(right.x + 1, mid)].symbol(), "❯");
            assert_eq!(
                app.hit_at(right.x + 1, right.y + 2),
                Some(HitTarget::LauncherStripRight(0)),
                "the glyph's cell is the button"
            );

            // `l` walks them one at a time, the pane following.
            for (i, card) in cards.iter().enumerate().skip(1) {
                key(&mut app, KeyCode::Char('l'), KeyModifiers::NONE);
                assert_eq!(at(&app), Some(i), "l onto card {i}");
                assert_eq!(pane(&app).as_ref(), Some(card), "the pane follows");
                assert!(app.launcher_expanded.is_none(), "at the band level still");
            }
            key(&mut app, KeyCode::Char('l'), KeyModifiers::NONE);
            assert_eq!(at(&app), Some(last), "l stops at the last card");
            draw(&mut app);
            assert!(
                app.hit_rect(&HitTarget::LauncherStripRight(0)).is_none(),
                "nothing past the right edge any more"
            );
            let left = app
                .hit_rect(&HitTarget::LauncherStripLeft(0))
                .expect("the cards scrolled off the left: a ❮");
            assert_eq!(
                app.hit_at(left.x, left.y + 2),
                Some(HitTarget::LauncherStripLeft(0))
            );

            // Tab opens the band on the card the walk stopped on, and
            // closes it again on the same card.
            key(&mut app, KeyCode::Tab, KeyModifiers::NONE);
            assert!(app.launcher_expanded.is_some());
            assert_eq!(pane(&app).as_ref(), Some(&cards[last]));
            key(&mut app, KeyCode::Tab, KeyModifiers::NONE);
            assert!(app.launcher_expanded.is_none());
            assert_eq!(at(&app), Some(last));

            // A click on ❮ is `h`, on ❯ is `l`.
            draw(&mut app);
            let left = app.hit_rect(&HitTarget::LauncherStripLeft(0)).expect("❮");
            click_at(&mut app, left.x + 1, left.y + 2);
            assert_eq!(at(&app), Some(last - 1), "one card back");
            assert_eq!(pane(&app).as_ref(), Some(&cards[last - 1]));
            assert!(
                app.launcher_expanded.is_none(),
                "the click keeps the level too"
            );
            draw(&mut app);
            let right = app
                .hit_rect(&HitTarget::LauncherStripRight(0))
                .expect("❯ again, with the last card off the edge");
            click_at(&mut app, right.x, right.y + 2);
            assert_eq!(at(&app), Some(last), "and forward again");
        });
    }

    /// A collapsed band hiding cards says so on the row of air under
    /// them — `▾ 3 more · Tab: see all 5` — and a click on it opens the
    /// band, the toggle Tab runs (INPUT PARITY). Open, it is gone: every
    /// card is on screen.
    #[test]
    fn the_more_hint_under_a_band_says_what_is_hidden_and_opens_it() {
        with_default_config(|| {
            let mut app = two_sessions();
            for (id, name) in [("a4", "second"), ("a5", "third"), ("a6", "fourth")] {
                seed_running(&mut app, id, "w1", name);
            }
            seed_terminal(&mut app, "t1", "w1", "shell");
            let term = draw(&mut app);
            let bands = crate::launcher::bands(&app);
            let hidden = bands[0].cards.len()
                - app
                    .hits
                    .iter()
                    .filter(|(_, h)| matches!(h, HitTarget::LauncherCard(c) if c.band == 0))
                    .count();
            assert!(hidden > 0, "the row leaves cards off");
            let hint = app
                .hit_rect(&HitTarget::LauncherBandMore(0))
                .expect("a hint under the row");
            let card = app
                .hits
                .iter()
                .find_map(|(r, h)| {
                    matches!(h, HitTarget::LauncherCard(c) if c.band == 0).then_some(*r)
                })
                .expect("a card on the row");
            assert_eq!(hint.y, card.y + card.height, "right under the cards");
            let buf = term.backend().buffer();
            let text: String = (hint.x..hint.x + hint.width)
                .map(|x| buf[(x, hint.y)].symbol().to_string())
                .collect();
            assert!(text.contains(&format!("▾ {hidden} more")), "{text:?}");
            assert!(text.contains("see all 5"), "{text:?}");

            click_at(&mut app, hint.x + 1, hint.y);
            assert_eq!(
                app.launcher_expanded.as_ref(),
                Some(&bands[0].worktree),
                "the click opens the band"
            );
            draw(&mut app);
            assert!(
                app.hit_rect(&HitTarget::LauncherBandMore(0)).is_none(),
                "open, nothing is hidden"
            );
        });
    }

    /// Walking onto a card reads it: the pane under the grid is showing
    /// that session, so its unread badge comes down there — the same rule
    /// the SESSIONS PANEL's cursor follows, keyed to the pane swap.
    #[test]
    fn walking_onto_a_card_reads_it_in_the_pane() {
        with_default_config(|| {
            let mut app = two_sessions();
            for a in app.tree.agents.iter_mut() {
                a.unseen = true;
            }
            draw(&mut app);
            key(&mut app, KeyCode::Esc, KeyModifiers::NONE);
            let out = key(&mut app, KeyCode::Char('j'), KeyModifiers::NONE);
            assert!(
                out.iter()
                    .any(|r| matches!(r, ClientRequest::MarkAgentSeen { id }
                    if id.0 == "a2")),
                "the card in the pane is read: {out:?}"
            );
            // And the one the cursor never reached keeps its badge.
            assert!(
                app.tree.agents.iter().any(|a| a.id.0 == "a1" && a.unseen),
                "the card walked away from is untouched"
            );
        });
    }

    /// The PANE beside the cards reads the card under the cursor — at the
    /// band level, the band's remembered card — and its header names the
    /// card and its checkout; walking the bands swaps it.
    #[test]
    fn the_pane_along_the_bottom_reads_the_card_under_the_cursor() {
        with_default_config(|| {
            let mut app = two_sessions();
            draw(&mut app);
            keys(&mut app, &[KeyCode::Esc, KeyCode::Char('j')]);
            let text = buffer_text(&draw(&mut app));
            assert!(
                text.contains("polish-nav  ↳ feat"),
                "the pane names the cursor's card and its checkout: {text}"
            );
            assert_eq!(
                tabs_drawn(&app),
                ["demo"],
                "the grid is still up over it: {text}"
            );
            let below = app
                .hits
                .iter()
                .filter_map(|(r, hit)| {
                    matches!(hit, HitTarget::LauncherCard(_)).then_some(r.y + r.height)
                })
                .max()
                .expect("the grid drew cards");
            assert!(
                app.term_area.y >= below,
                "the pane is under the cards, not beside them: {:?} vs {below}",
                app.term_area
            );

            // And the walk keeps swapping it.
            key(&mut app, KeyCode::Char('k'), KeyModifiers::NONE);
            let text = buffer_text(&draw(&mut app));
            assert!(
                text.contains("agent-1  ⌂ main"),
                "the band above takes the pane: {text}"
            );
        });
    }

    /// A click into the PANE under the grid types into the session it is
    /// showing, where it stands — the same [`enter_terminal_pane`] a click
    /// into the panels' pane is, so the grid stays up over it — and the
    /// hatch hands the keys back to the cards.
    #[test]
    fn a_click_into_the_pane_types_into_the_card_it_shows() {
        with_default_config(|| {
            let mut app = two_sessions();
            draw(&mut app);
            to_feat(&mut app);
            draw(&mut app);

            let pane = app.term_area;
            mouse(
                &mut app,
                MouseEventKind::Down(MouseButton::Left),
                pane.x + 2,
                pane.y + 1,
            );
            assert_eq!(app.focus, Focus::Terminal, "the pane has the keys");
            assert!(app.term_locked);
            assert!(!app.collapsed, "and the grid is still up over it");

            let out = key(&mut app, KeyCode::Char('x'), KeyModifiers::NONE);
            assert!(
                out.iter()
                    .any(|r| matches!(r, ClientRequest::Input { session, .. }
                    if *session == SessionRef::Agent(AgentId("a2".into())))),
                "typing goes to the card in the pane: {out:?}"
            );
            // The view's draw must not snatch the focus back off the pane.
            draw(&mut app);
            assert_eq!(app.focus, Focus::Terminal);

            key(&mut app, KeyCode::Char('q'), KeyModifiers::CONTROL);
            assert_eq!(app.focus, Focus::Sessions, "the hatch is back to the cards");
            assert!(!app.term_locked);
            keys(&mut app, &[KeyCode::Esc, KeyCode::Char('k')]);
            assert_eq!(
                selected(&app).as_deref(),
                Some("a1"),
                "and the grid walks again"
            );
        });
    }

    /// Enter crosses into the PANE along the bottom and takes its input,
    /// with the grid still up over it — the state a click into the pane
    /// leaves; the hatch hands the keys back to the cards.
    #[test]
    fn enter_crosses_into_the_pane_and_the_hatch_returns_to_the_cards() {
        with_default_config(|| {
            let mut app = two_sessions();
            draw(&mut app);
            to_feat(&mut app);
            key(&mut app, KeyCode::Enter, KeyModifiers::NONE);
            assert_eq!(app.focus, Focus::Terminal);
            assert!(app.term_locked);
            assert!(!app.collapsed, "the pane under the grid, not full-screen");
            let text = buffer_text(&draw(&mut app));
            assert_eq!(
                tabs_drawn(&app),
                ["demo"],
                "the grid is still up over the pane: {text}"
            );

            let out = key(&mut app, KeyCode::Char('x'), KeyModifiers::NONE);
            assert!(
                out.iter()
                    .any(|r| matches!(r, ClientRequest::Input { session, .. }
                    if *session == SessionRef::Agent(AgentId("a2".into())))),
                "typing goes to the card in the pane: {out:?}"
            );

            key(&mut app, KeyCode::Char('q'), KeyModifiers::CONTROL);
            assert_eq!(app.focus, Focus::Sessions, "the hatch is back to the cards");
            assert!(!app.term_locked);
            assert_eq!(selected(&app).as_deref(), Some("a2"));
        });
    }

    /// A full-screen session — Enter on a body too short to draw the
    /// pane — takes the input; the hatch hands the keys back to the grid,
    /// the cursor where it was.
    #[test]
    fn the_hatch_returns_a_full_screen_session_to_the_grid() {
        with_default_config(|| {
            let mut app = two_sessions();
            draw(&mut app);
            to_feat(&mut app);
            full_screen(&mut app);
            assert_eq!(app.focus, Focus::Terminal);
            assert!(app.term_locked);
            assert!(app.collapsed, "full-screen, not a pane beside the grid");
            let text = buffer_text(&draw(&mut app));
            assert!(
                text.contains("‹ sessions / ● polish-nav"),
                "the breadcrumb names the session: {text}"
            );
            assert!(
                tabs_drawn(&app).is_empty(),
                "the grid's header is gone: {text}"
            );

            let out = key(&mut app, KeyCode::Char('x'), KeyModifiers::NONE);
            assert!(
                out.iter()
                    .any(|r| matches!(r, ClientRequest::Input { session, .. }
                    if *session == SessionRef::Agent(AgentId("a2".into())))),
                "typing goes to the session: {out:?}"
            );

            key(&mut app, KeyCode::Char('q'), KeyModifiers::CONTROL);
            assert_eq!(app.focus, Focus::Sessions);
            assert!(!app.term_locked);
            assert!(!app.collapsed, "back out of full-screen");
            assert_eq!(selected(&app).as_deref(), Some("a2"));
            let text = buffer_text(&draw(&mut app));
            assert_eq!(
                tabs_drawn(&app),
                ["demo"],
                "the grid, not the panels: {text}"
            );
            assert!(text.contains("1 session"), "inside feat: {text}");
        });
    }

    /// The PROJECT TABS the last draw laid down, by project name, left to
    /// right — none at all while the grid's header is not on screen.
    fn tabs_drawn(app: &App) -> Vec<String> {
        app.hits
            .iter()
            .filter_map(|(_, h)| match h {
                HitTarget::LauncherTab(id) => app
                    .tree
                    .projects
                    .iter()
                    .find(|p| &p.id == id)
                    .map(|p| p.name.clone()),
                _ => None,
            })
            .collect()
    }

    /// A cell inside the crumb `hit` names, as the header drew it.
    fn crumb_cell(app: &App, hit: HitTarget) -> (u16, u16) {
        let (rect, _) = app
            .hits
            .iter()
            .find(|(_, h)| *h == hit)
            .unwrap_or_else(|| panic!("{hit:?} was not drawn"));
        (rect.x, rect.y)
    }

    /// The header is the PROJECT TABS: the `+`, then a tab for the project
    /// the view opened on, with its `×` — no `nebula`, no trail.
    #[test]
    fn the_header_is_the_project_tabs_and_nothing_else() {
        with_default_config(|| {
            let mut app = two_sessions();
            let terminal = draw(&mut app);
            let head = head_line(&terminal);
            assert!(!head.contains("nebula"), "{head}");
            assert!(!head.contains("demo / sessions"), "{head}");

            let demo = ProjectId("p1".into());
            let head: Vec<HitTarget> = app
                .hits
                .iter()
                .filter(|(r, h)| {
                    r.y == 1
                        && matches!(
                            h,
                            HitTarget::LauncherTab(_)
                                | HitTarget::LauncherTabClose(_)
                                | HitTarget::LauncherTabAdd
                        )
                })
                .map(|(_, h)| h.clone())
                .collect();
            assert_eq!(
                head,
                vec![
                    HitTarget::LauncherTabAdd,
                    HitTarget::LauncherTab(demo.clone()),
                    HitTarget::LauncherTabClose(demo),
                ],
                "{head:?}"
            );
        });
    }

    /// What the header row underlines: the word under the pointer, and
    /// nothing else.
    fn underlined_head(terminal: &Terminal<TestBackend>) -> String {
        let buf = terminal.backend().buffer();
        (0..buf.area.width)
            .filter_map(|x| buf.cell((x, 1)))
            .filter(|c| c.modifier.contains(Modifier::UNDERLINED))
            .map(|c| c.symbol())
            .collect()
    }

    /// The header row, as it was drawn.
    fn head_line(terminal: &Terminal<TestBackend>) -> String {
        let buf = terminal.backend().buffer();
        (0..buf.area.width)
            .filter_map(|x| buf.cell((x, 1)))
            .map(|c| c.symbol())
            .collect()
    }

    /// The `+` after the tabs is a SWITCH: a click on it drops every
    /// project under it — the one in front of you ticked and under the
    /// cursor, a row for opening a folder last — and the list hangs off
    /// the `+` rather than in the middle of the screen. Picking a row
    /// re-aims the grid and gives the project a tab at the far left.
    #[test]
    fn the_plus_drops_the_projects_under_it() {
        with_default_config(|| {
            let mut app = two_sessions();
            draw(&mut app);

            // The `+`: every project, `demo` ticked and hovered, the list
            // under the button.
            let (x, y) = crumb_cell(&app, HitTarget::LauncherTabAdd);
            mouse(&mut app, MouseEventKind::Down(MouseButton::Left), x, y);
            let Some(Overlay::Menu(menu)) = &app.overlay else {
                panic!("the + drops a list: {:?}", app.overlay);
            };
            assert!(menu.is_project_picker());
            assert_eq!(menu.at, Some((x, y + 1)), "it hangs off the +");
            let labels: Vec<&str> = menu.items.iter().map(|i| i.label.as_str()).collect();
            assert_eq!(
                labels,
                vec!["demo  (2) ✓", "web  (1)", super::OPEN_FOLDER],
                "{labels:?}"
            );
            assert_eq!(menu.hover, 0, "the cursor starts on the open project");
            assert_eq!(
                app.selected_project().map(|p| p.name.as_str()),
                Some("demo"),
                "the grid stays put until a row is picked"
            );

            // Enter on `web`: the grid, aimed at the other project. The
            // rows take type-ahead, so the arrows move here, not j/k.
            key(&mut app, KeyCode::Down, KeyModifiers::NONE);
            key(&mut app, KeyCode::Enter, KeyModifiers::NONE);
            assert!(app.overlay.is_none(), "the list closes: {:?}", app.overlay);
            assert_eq!(
                app.selected_project().map(|p| p.name.clone()),
                Some("web".into())
            );
            draw(&mut app);
            assert_eq!(
                tabs_drawn(&app),
                ["web", "demo"],
                "the project just opened leads the tabs"
            );
        });
    }

    /// TYPE-AHEAD in the PROJECT DROPDOWN: letters narrow the rows to what
    /// they fuzzy-match rather than jumping the cursor, so a project is
    /// found by name instead of by scrolling. Backspace widens, Esc clears
    /// the query before it closes the list, and a letter nothing matches
    /// is refused so the list never empties.
    #[test]
    fn typing_in_the_project_dropdown_narrows_it_to_the_name() {
        with_default_config(|| {
            let mut app = two_sessions();
            seed_empty_project(&mut app);
            draw(&mut app);

            let (x, y) = crumb_cell(&app, HitTarget::LauncherTabAdd);
            mouse(&mut app, MouseEventKind::Down(MouseButton::Left), x, y);
            let labels = |app: &App| -> Vec<String> {
                let Some(Overlay::Menu(menu)) = &app.overlay else {
                    panic!("the dropdown closed: {:?}", app.overlay);
                };
                menu.items.iter().map(|i| i.label.clone()).collect()
            };
            assert_eq!(
                labels(&app),
                vec!["demo  (2) ✓", "web  (1)", "docs  (0)", super::OPEN_FOLDER],
                "every project, before a letter is typed"
            );

            // Inside the dropdown a letter is a letter: `w` narrows to
            // `web`.
            key(&mut app, KeyCode::Char('w'), KeyModifiers::NONE);
            assert_eq!(labels(&app), vec!["web  (1)"]);

            // Backspace widens back to the whole list.
            key(&mut app, KeyCode::Backspace, KeyModifiers::NONE);
            assert_eq!(labels(&app).len(), 4);

            // `do` finds `docs` past `demo`, and Enter opens it.
            key(&mut app, KeyCode::Char('d'), KeyModifiers::NONE);
            key(&mut app, KeyCode::Char('o'), KeyModifiers::NONE);
            assert_eq!(labels(&app)[0], "docs  (0)");
            key(&mut app, KeyCode::Enter, KeyModifiers::NONE);
            assert!(app.overlay.is_none(), "{:?}", app.overlay);
            assert_eq!(
                app.selected_project().map(|p| p.name.clone()),
                Some("docs".into())
            );

            // A letter nothing matches leaves the list as it was, and says
            // so; Esc then clears the query before it closes the list. The
            // `+` moved right to make room for `docs`'s new tab.
            draw(&mut app);
            let (x, y) = crumb_cell(&app, HitTarget::LauncherTabAdd);
            mouse(&mut app, MouseEventKind::Down(MouseButton::Left), x, y);
            key(&mut app, KeyCode::Char('d'), KeyModifiers::NONE);
            let narrowed = labels(&app);
            key(&mut app, KeyCode::Char('z'), KeyModifiers::NONE);
            assert_eq!(labels(&app), narrowed, "the list never empties");
            assert!(app.flash.is_some(), "and the refusal says so");
            key(&mut app, KeyCode::Esc, KeyModifiers::NONE);
            assert_eq!(labels(&app).len(), 4, "Esc clears the query first");
            key(&mut app, KeyCode::Esc, KeyModifiers::NONE);
            assert!(app.overlay.is_none(), "and then closes: {:?}", app.overlay);
        });
    }

    /// `⌘P` is the click on the header's `+` from the keyboard — INPUT
    /// PARITY: both are [`super::open_project_menu`], so it is the same
    /// list hung off the same `+`. It works from the cards and from inside
    /// the PANE under them, where the chord is taken rather than typed
    /// into the agent and Esc hands the keys straight back. The chord
    /// again puts the list away instead of typing a `p` into its
    /// type-ahead, and a full-screen session, with no header on screen,
    /// keeps the chord for itself.
    #[test]
    fn cmd_p_drops_the_project_dropdown_from_the_cards_and_the_pane() {
        with_default_config(|| {
            let cmd_p = |app: &mut App| key(app, KeyCode::Char('p'), KeyModifiers::SUPER);
            let dropdown = |app: &App| -> (Vec<String>, Option<(u16, u16)>, usize) {
                let Some(Overlay::Menu(menu)) = &app.overlay else {
                    panic!("no PROJECT DROPDOWN: {:?}", app.overlay);
                };
                assert!(menu.is_project_picker());
                let labels = menu.items.iter().map(|i| i.label.clone()).collect();
                (labels, menu.at, menu.hover)
            };

            let mut by_click = two_sessions();
            draw(&mut by_click);
            let (x, y) = crumb_cell(&by_click, HitTarget::LauncherTabAdd);
            mouse(&mut by_click, MouseEventKind::Down(MouseButton::Left), x, y);
            let mut by_key = two_sessions();
            draw(&mut by_key);
            cmd_p(&mut by_key);
            assert_eq!(dropdown(&by_key), dropdown(&by_click));
            let mut by_plus = two_sessions();
            draw(&mut by_plus);
            key(&mut by_plus, KeyCode::Char('+'), KeyModifiers::NONE);
            assert_eq!(
                dropdown(&by_plus),
                dropdown(&by_click),
                "+ where ⌘ never arrives"
            );

            cmd_p(&mut by_key);
            assert!(by_key.overlay.is_none(), "{:?}", by_key.overlay);

            // Inside the pane under the cards.
            let mut app = two_sessions();
            draw(&mut app);
            key(&mut app, KeyCode::Enter, KeyModifiers::NONE);
            assert!(app.term_locked && !app.collapsed);
            draw(&mut app);
            let out = cmd_p(&mut app);
            assert!(
                !out.iter().any(|r| matches!(r, ClientRequest::Input { .. })),
                "the agent was sent the chord: {out:?}"
            );
            assert_eq!(dropdown(&app).1, dropdown(&by_click).1, "off the same +");
            key(&mut app, KeyCode::Esc, KeyModifiers::NONE);
            assert!(app.overlay.is_none(), "{:?}", app.overlay);
            assert_eq!(app.focus, Focus::Terminal, "the keys are the pane's again");
            assert!(app.term_locked);

            // A project picked from over the pane lands on its cards.
            cmd_p(&mut app);
            key(&mut app, KeyCode::Char('w'), KeyModifiers::NONE);
            key(&mut app, KeyCode::Enter, KeyModifiers::NONE);
            assert!(app.overlay.is_none(), "{:?}", app.overlay);
            assert_eq!(
                app.selected_project().map(|p| p.name.clone()),
                Some("web".into())
            );
            assert_eq!(app.focus, Focus::Sessions);

            // Full-screen: no header, so no list.
            full_screen(&mut app);
            assert!(app.collapsed && app.term_locked);
            cmd_p(&mut app);
            assert!(app.overlay.is_none(), "{:?}", app.overlay);
        });
    }

    /// Switching projects is asking to LOOK at one, never to start
    /// something in it: a pick from the PROJECT DROPDOWN that lands on a
    /// project with nothing in it yet leaves the box shut, and the grid
    /// says what starts one instead. The same for its tab and a digit —
    /// all take the one [`open_project`].
    #[test]
    fn switching_to_an_empty_project_opens_no_box() {
        with_default_config(|| {
            let mut app = two_sessions();
            seed_empty_project(&mut app);
            draw(&mut app);

            // Down the `+`'s list to `docs`, the one with no sessions.
            let (x, y) = crumb_cell(&app, HitTarget::LauncherTabAdd);
            mouse(&mut app, MouseEventKind::Down(MouseButton::Left), x, y);
            let Some(Overlay::Menu(menu)) = &app.overlay else {
                panic!("the + drops a list: {:?}", app.overlay);
            };
            let labels: Vec<&str> = menu.items.iter().map(|i| i.label.as_str()).collect();
            assert_eq!(
                labels,
                vec!["demo  (2) ✓", "web  (1)", "docs  (0)", super::OPEN_FOLDER],
                "{labels:?}"
            );
            key(&mut app, KeyCode::Down, KeyModifiers::NONE);
            key(&mut app, KeyCode::Down, KeyModifiers::NONE);
            key(&mut app, KeyCode::Enter, KeyModifiers::NONE);

            assert!(
                app.overlay.is_none(),
                "switching projects put a modal up: {:?}",
                app.overlay
            );
            assert_eq!(
                app.selected_project().map(|p| p.name.clone()),
                Some("docs".into())
            );
            assert!(app.launcher_unaimed, "no card to aim at, so the pane folds");
            assert_eq!(app.flash.as_deref(), Some(super::NO_SESSIONS));
            let text = buffer_text(&draw(&mut app));
            assert_eq!(tabs_drawn(&app), ["docs", "demo"], "{text}");
            assert!(
                text.contains("press  p  to prompt"),
                "the grid says what starts one: {text}"
            );
        });
    }

    /// A project just added has nothing for the pane to read: it folds
    /// away, off whatever the old project's pane was on — a terminal's
    /// chip included — whichever of the Ack and the upsert lands first.
    #[test]
    fn a_project_just_added_folds_away_the_pane_its_terminal_was_in() {
        use crate::app::PendingIntent;
        use nebula_core::EntityId;
        with_default_config(|| {
            for ack_first in [true, false] {
                let mut app = two_sessions();
                seed_terminal(&mut app, "t1", "w2", "shell-1");
                draw(&mut app);
                let feat = crate::launcher::rows(&app)
                    .iter()
                    .position(|r| r.agent.name == "polish-nav")
                    .expect("a card for polish-nav");
                walk_to(&mut app, feat);
                key(&mut app, KeyCode::Char('`'), KeyModifiers::NONE);
                draw(&mut app);
                assert!(super::has_pane(&app), "the pane is up to start with");
                assert_eq!(
                    reading(&app),
                    Some(SessionRef::Terminal(TerminalId("t1".into())))
                );

                let req_id = app.alloc_req_id(PendingIntent::SelectCreatedProject);
                let ack = ServerEvent::Ack {
                    req_id,
                    created: Some(EntityId::Project(ProjectId("p3".into()))),
                };
                if ack_first {
                    hse(&mut app, ack);
                    seed_empty_project(&mut app);
                } else {
                    seed_empty_project(&mut app);
                    hse(&mut app, ack);
                }
                let text = buffer_text(&draw(&mut app));

                assert_eq!(
                    app.selected_project().map(|p| p.name.clone()),
                    Some("docs".into()),
                    "ack_first={ack_first}"
                );
                assert!(
                    !super::has_pane(&app),
                    "ack_first={ack_first}: the pane folded away: {text}"
                );
                assert_eq!(app.flash.as_deref(), Some(super::NO_SESSIONS));
                assert!(text.contains("Welcome to nebula"), "{text}");
                assert!(!text.contains("shell-1"), "no terminal of demo's: {text}");
            }
        });
    }

    /// A project picked by name from the `/` PALETTE lands as its tab
    /// does: with no session in it, the empty grid and no pane.
    #[test]
    fn a_palette_pick_of_an_empty_project_folds_the_pane_away() {
        with_default_config(|| {
            let mut app = two_sessions();
            seed_empty_project(&mut app);
            draw(&mut app);
            super::select_card_row(&mut app, CardRef { band: 0, card: 0 }, &mut Vec::new());
            draw(&mut app);
            assert!(super::has_pane(&app), "the pane is up to start with");

            super::super::jump_to_target(
                &mut app,
                crate::palette::PaletteTarget::Project(ProjectId("p3".into())),
                super::super::Landing::FocusOnly,
                &mut Vec::new(),
            );
            draw(&mut app);

            assert_eq!(
                app.selected_project().map(|p| p.name.clone()),
                Some("docs".into())
            );
            assert!(!super::has_pane(&app), "the pane folded away");
            assert_eq!(app.flash.as_deref(), Some(super::NO_SESSIONS));
        });
    }

    /// A project with no sessions has nothing for the PANE to read, so
    /// `^`` opens it EMPTY — in one press, since a pane let go of for want
    /// of a card is off screen and there is nothing to fold — and never on
    /// the session it read in the project before. Its header offers no
    /// button: `t` opens a terminal on the project's root, and it comes
    /// up as its card inside the checkout's band with the keys in the
    /// pane, whichever of its Ack and its upsert arrives first.
    #[test]
    fn an_empty_projects_pane_opens_empty_and_offers_a_terminal() {
        use nebula_core::EntityId;
        let creates = |out: &[ClientRequest]| {
            out.iter().find_map(|r| match r {
                ClientRequest::CreateTerminal {
                    req_id, worktree, ..
                } => Some((*req_id, worktree.clone())),
                _ => None,
            })
        };
        with_default_config(|| {
            for ack_first in [true, false] {
                let mut app = two_sessions();
                seed_empty_project(&mut app);
                draw(&mut app);
                let mut out = Vec::new();
                super::select_card_row(&mut app, CardRef { band: 0, card: 0 }, &mut out);
                let polish_nav = SessionRef::Agent(AgentId("a2".into()));
                super::super::attach_now(&mut app, polish_nav.clone(), &mut out);
                draw(&mut app);
                assert!(super::has_pane(&app) && reading(&app) == Some(polish_nav));

                super::open_project(&mut app, &ProjectId("p3".into()), &mut out);
                assert_eq!(reading(&app), None, "nothing of demo's left attached");

                key(&mut app, KeyCode::Char('~'), KeyModifiers::CONTROL);
                let text = buffer_text(&draw(&mut app));
                assert!(
                    super::has_pane(&app),
                    "one press brings the pane up: {text}"
                );
                assert_eq!(reading(&app), None, "and it reads nothing");
                assert!(
                    !text.contains("polish-nav"),
                    "no other project's session: {text}"
                );

                let by_key = key(&mut app, KeyCode::Char('t'), KeyModifiers::NONE);
                let (req_id, worktree) = creates(&by_key).expect("t asks for a terminal");
                assert_eq!(worktree, WorktreeId("w3root".into()), "in docs' own root");

                let ack = ServerEvent::Ack {
                    req_id,
                    created: Some(EntityId::Terminal(TerminalId("t9".into()))),
                };
                if ack_first {
                    hse(&mut app, ack);
                    // A frame between the two: the landing has to outlast it.
                    draw(&mut app);
                    seed_terminal(&mut app, "t9", "w3root", "term-1");
                } else {
                    seed_terminal(&mut app, "t9", "w3root", "term-1");
                    hse(&mut app, ack);
                }
                let text = buffer_text(&draw(&mut app));

                assert!(super::has_pane(&app), "ack_first={ack_first}: {text}");
                assert_eq!(
                    reading(&app),
                    Some(SessionRef::Terminal(TerminalId("t9".into())))
                );
                let bands = crate::launcher::bands(&app);
                assert_eq!(
                    crate::launcher::cursor(&app, &bands),
                    Some(CardRef { band: 0, card: 0 }),
                    "ack_first={ack_first}: on the chip"
                );
                assert!(text.contains("term-1"), "the chip is drawn: {text}");
                assert_eq!(app.focus, Focus::Terminal, "and the keys are in it");
                let card = tab_at(&app, HitTarget::LauncherCard(CardRef { band: 0, card: 0 }));
                assert_eq!(
                    card.height,
                    crate::launcher::CARD_H,
                    "ack_first={ack_first}: the card is on the grid"
                );
            }
        });
    }

    /// The grid offers no `+` for a terminal any more — not on a band's
    /// strip, not under the `terminals` rule, not on the pane's header:
    /// `t` is how one opens, in the cursor's checkout. A terminal's card
    /// on the grid spans two of the grid's columns, gap included.
    #[test]
    fn t_opens_a_terminal_and_the_grid_has_no_plus() {
        let creates = |out: &[ClientRequest]| {
            out.iter().find_map(|r| match r {
                ClientRequest::CreateTerminal { worktree, .. } => Some(worktree.clone()),
                _ => None,
            })
        };
        with_default_config(|| {
            let mut app = two_sessions();
            seed_terminal(&mut app, "t1", "w2", "shell-1");
            draw(&mut app);
            let feat = crate::launcher::rows(&app)
                .iter()
                .position(|r| r.agent.name == "polish-nav")
                .expect("a card for polish-nav");
            walk_to(&mut app, feat);
            let terminal = draw_at(&mut app, 130, 50);
            let text = buffer_text(&terminal);
            assert!(super::has_pane(&app), "{text}");
            assert!(text.contains("shell-1"), "the card is on the grid: {text}");
            let bands = crate::launcher::bands(&app);
            let is_terminal = |h: &HitTarget| matches!(h, HitTarget::LauncherCard(at) if bands[at.band].cards[at.card].is_terminal());
            let shell = app
                .hits
                .iter()
                .find(|(_, h)| is_terminal(h))
                .map(|(r, _)| *r)
                .expect("shell-1's card");
            let session = app
                .hits
                .iter()
                .find(|(_, h)| matches!(h, HitTarget::LauncherCard(_)) && !is_terminal(h))
                .map(|(r, _)| *r)
                .expect("a session card");
            assert_eq!(
                shell.width,
                session.width * 2 + crate::launcher::GAP_X,
                "two columns wide: {text}"
            );
            // No `+` anywhere under the header: the tab strip's own is the
            // one on screen.
            let buffer = terminal.backend().buffer();
            let pluses: Vec<(u16, u16)> = (crate::launcher::HEAD_H..50u16)
                .flat_map(|y| (0..130u16).map(move |x| (x, y)))
                .filter(|&(x, y)| buffer[(x, y)].symbol() == "+")
                .collect();
            assert!(pluses.is_empty(), "no + on the grid: {pluses:?}\n{text}");

            let by_key = key(&mut app, KeyCode::Char('t'), KeyModifiers::NONE);
            assert_eq!(creates(&by_key), Some(WorktreeId("w2".into())));
        });
    }

    /// [`two_sessions`] plus `docs` from [`seed_empty_project`], opened:
    /// a project with no sessions in front of you, so an empty grid.
    fn on_an_empty_project() -> App {
        let mut app = two_sessions();
        seed_empty_project(&mut app);
        super::open_project(&mut app, &ProjectId("p3".into()), &mut Vec::new());
        app
    }

    /// The empty grid is a welcome and nothing else: the name, and the key
    /// that starts a session as a key cap. The nebula it is drawn over
    /// ticks while it is on screen, and stops under the box that key puts
    /// up. INPUT PARITY: a click on the key cap is the key — the same
    /// `open_box`, so the same box, aimed at the same checkout.
    #[test]
    fn the_empty_grid_welcomes_you_and_its_key_cap_is_the_key() {
        with_default_config(|| {
            let mut by_click = on_an_empty_project();
            let terminal = draw(&mut by_click);
            let text = buffer_text(&terminal);
            assert!(text.contains("Welcome to nebula"), "{text}");
            assert!(text.contains("press  p  to prompt"), "{text}");
            assert!(!text.contains("type a task"), "only the welcome: {text}");
            let (x, y) = crumb_cell(&by_click, HitTarget::LauncherWelcomePrompt);
            let cap = &terminal.backend().buffer()[(x + 7, y)];
            assert_eq!(cap.symbol(), "p", "{text}");
            assert_eq!(cap.bg, by_click.theme.accent, "the key is a key cap");
            assert!(by_click.welcome_active(), "the nebula ticks while it is up");

            mouse(&mut by_click, MouseEventKind::Down(MouseButton::Left), x, y);
            let mut by_key = on_an_empty_project();
            draw(&mut by_key);
            key(&mut by_key, KeyCode::Char('p'), KeyModifiers::NONE);
            let launch = |app: &App| match &app.overlay {
                Some(Overlay::Prompt(p)) => match &p.kind {
                    PromptKind::QuickPrompt(launch) => launch.clone(),
                    other => panic!("not the box: {other:?}"),
                },
                other => panic!("no box: {other:?}"),
            };
            assert_eq!(launch(&by_click), launch(&by_key));

            draw(&mut by_click);
            assert!(!by_click.welcome_active(), "nothing ticks under the box");
        });
    }

    /// The welcome sits under the splash's nebula: dust in the sky over
    /// it, clear black right around the words — and on a grid too small
    /// for a sky, the words alone.
    #[test]
    fn the_welcome_sits_under_a_nebula() {
        const DUST: &[&str] = &[".", ":", "·", "+", "*", "o", "@"];
        with_default_config(|| {
            let glyphs = |terminal: &Terminal<TestBackend>, rows: std::ops::Range<u16>| {
                let buf = terminal.backend().buffer();
                rows.flat_map(|y| (0..buf.area.width).map(move |x| (x, y)))
                    .filter(|&(x, y)| DUST.contains(&buf[(x, y)].symbol()))
                    .count()
            };
            let row = |terminal: &Terminal<TestBackend>, y: u16| {
                let buf = terminal.backend().buffer();
                (0..buf.area.width)
                    .map(|x| buf[(x, y)].symbol())
                    .collect::<String>()
            };

            let mut app = on_an_empty_project();
            // Animations off hold one finished frame, well past the fade.
            app.animations = false;
            let terminal = draw(&mut app);
            let grid = crate::launcher::grid(app.body_area).area;
            let (_, key_y) = crumb_cell(&app, HitTarget::LauncherWelcomePrompt);
            let sky = glyphs(&terminal, grid.y..key_y - 3);
            assert!(sky > 40, "{sky} specks of dust: {}", buffer_text(&terminal));
            assert!(row(&terminal, key_y - 2).contains("   Welcome to nebula   "));
            assert!(row(&terminal, key_y).contains("   press  p  to prompt   "));
            assert!(!app.welcome_active(), "animations off: a still frame");

            let mut small = on_an_empty_project();
            small.animations = false;
            let terminal = draw_at(&mut small, 40, 12);
            let grid = crate::launcher::grid(small.body_area).area;
            let (_, key_y) = crumb_cell(&small, HitTarget::LauncherWelcomePrompt);
            assert!(row(&terminal, key_y - 2).contains("Welcome to nebula"));
            let specks =
                glyphs(&terminal, grid.y..key_y - 2) + glyphs(&terminal, key_y + 1..grid.bottom());
            assert_eq!(specks, 0, "{}", buffer_text(&terminal));
        });
    }

    /// `/`, the query, Enter: the fuzzy jump, the way it is typed.
    fn jump(app: &mut App, query: &str) {
        key(app, KeyCode::Char('/'), KeyModifiers::NONE);
        for c in query.chars() {
            key(app, KeyCode::Char(c), KeyModifiers::NONE);
        }
        key(app, KeyCode::Enter, KeyModifiers::NONE);
        assert!(app.overlay.is_none(), "the jump landed: {:?}", app.overlay);
    }

    /// A `/` jump into another project ADDS its tab: the header keeps
    /// every project already open, so the jump reads as one more tab at
    /// the far left — never as the first tab changing its name. A click on
    /// the tab left behind goes back and moves no tab, and `[` steps
    /// back across them.
    #[test]
    fn a_jump_into_another_project_adds_a_tab() {
        with_default_config(|| {
            let mut app = two_sessions();
            draw(&mut app);
            assert_eq!(tabs_drawn(&app), ["demo"]);

            jump(&mut app, "tidy-css");
            assert_eq!(
                app.selected_project().map(|p| p.name.clone()),
                Some("web".into())
            );
            draw(&mut app);
            assert_eq!(tabs_drawn(&app), ["web", "demo"]);

            let (x, y) = crumb_cell(&app, HitTarget::LauncherTab(ProjectId("p1".into())));
            mouse(&mut app, MouseEventKind::Down(MouseButton::Left), x, y);
            assert_eq!(
                app.selected_project().map(|p| p.name.clone()),
                Some("demo".into())
            );
            draw(&mut app);
            assert_eq!(tabs_drawn(&app), ["web", "demo"], "no tab moved");

            key(&mut app, KeyCode::Char('['), KeyModifiers::NONE);
            assert_eq!(
                app.selected_project().map(|p| p.name.clone()),
                Some("web".into())
            );
        });
    }

    /// The PROJECT DROPDOWN's last row opens a folder that is not a
    /// project yet: the open-project prompt `o` opens, over the grid.
    #[test]
    fn the_dropdowns_last_row_opens_a_folder() {
        with_default_config(|| {
            let mut app = two_sessions();
            draw(&mut app);
            let (x, y) = crumb_cell(&app, HitTarget::LauncherTabAdd);
            mouse(&mut app, MouseEventKind::Down(MouseButton::Left), x, y);
            for _ in 0..2 {
                key(&mut app, KeyCode::Down, KeyModifiers::NONE);
            }
            key(&mut app, KeyCode::Enter, KeyModifiers::NONE);
            let Some(Overlay::Prompt(prompt)) = &app.overlay else {
                panic!("the row opens the prompt: {:?}", app.overlay);
            };
            assert!(matches!(prompt.kind, PromptKind::AddProject));
            assert_eq!(prompt.title, "Open project");
        });
    }

    /// A right-click on a PROJECT TAB opens that project — the left
    /// click's [`open_tab`] — with the project's own menu hung under the
    /// tab: its checkouts, its run command, its name and its place in the
    /// list, the verbs the PROJECTS PANEL's rows used to carry.
    #[test]
    fn a_right_click_on_a_tab_opens_its_projects_menu() {
        with_default_config(|| {
            let mut app = two_tabs();
            let (x, y) = crumb_cell(&app, HitTarget::LauncherTab(ProjectId("p2".into())));
            mouse(&mut app, MouseEventKind::Down(MouseButton::Right), x, y);
            assert_eq!(
                app.selected_project().map(|p| p.name.as_str()),
                Some("web"),
                "the tab opened"
            );
            let Some(Overlay::Menu(menu)) = &app.overlay else {
                panic!("the tab's menu: {:?}", app.overlay);
            };
            let labels: Vec<&str> = menu.items.iter().map(|i| i.label.as_str()).collect();
            for want in ["New worktree", "Rename", "Remove from list"] {
                assert!(labels.contains(&want), "{want} in {labels:?}");
            }
            assert_eq!(menu.at.map(|(_, row)| row), Some(y + 1), "under the tab");
        });
    }

    /// `m` opens no menu — not a card's, and with no card selected not
    /// the project's either. The menus are the right button's alone.
    #[test]
    fn m_opens_no_menu() {
        with_default_config(|| {
            let mut app = two_sessions();
            draw(&mut app);
            key(&mut app, KeyCode::Char('m'), KeyModifiers::NONE);
            assert!(app.overlay.is_none(), "on a card: {:?}", app.overlay);

            keys(&mut app, &[KeyCode::Esc, KeyCode::Esc]);
            assert!(app.launcher_unaimed);
            key(&mut app, KeyCode::Char('m'), KeyModifiers::NONE);
            assert!(app.overlay.is_none(), "unaimed: {:?}", app.overlay);
        });
    }

    /// [`two_sessions`] with both projects open — `web` opened last, so
    /// its tab leads — and the grid on `demo`, the tab on the right.
    fn two_tabs() -> App {
        let mut app = two_sessions();
        draw(&mut app);
        app.launcher_tabs = vec![ProjectId("p2".into()), ProjectId("p1".into())];
        draw(&mut app);
        app
    }

    /// Everything a tab switch or a close moves, for INPUT PARITY: the
    /// project, the card and the tabs themselves.
    fn tab_state(app: &App) -> (Option<String>, Option<String>, Vec<String>) {
        (
            app.selected_project().map(|p| p.name.clone()),
            selected(app),
            app.launcher_tabs.iter().map(|t| t.0.clone()).collect(),
        )
    }

    /// Click on a PROJECT TAB and the key that walks onto it end in the
    /// same state — one [`open_tab`] — and `[` / `]` stop at either end.
    /// A switch moves no tab: only working in a project does.
    #[test]
    fn a_click_on_a_tab_is_the_key_that_walks_to_it() {
        with_default_config(|| {
            let mut by_key = two_tabs();
            key(&mut by_key, KeyCode::Char('['), KeyModifiers::NONE);

            let mut by_click = two_tabs();
            let (x, y) = crumb_cell(&by_click, HitTarget::LauncherTab(ProjectId("p2".into())));
            mouse(&mut by_click, MouseEventKind::Down(MouseButton::Left), x, y);

            assert_eq!(tab_state(&by_key), tab_state(&by_click));
            assert_eq!(pane(&by_key), pane(&by_click));
            assert_eq!(
                tab_state(&by_key).0.as_deref(),
                Some("web"),
                "`[` from the right-hand tab is the one on its left"
            );
            assert_eq!(selected(&by_key).as_deref(), Some("a3"), "web's session");
            assert_eq!(tab_state(&by_key).2, ["p2", "p1"], "no tab moved");

            // Neither end wraps round: `]` on the last tab and `[` on the
            // first stay where they are, quietly.
            let mut app = two_tabs();
            let before = tab_state(&app);
            key(&mut app, KeyCode::Char(']'), KeyModifiers::NONE);
            assert_eq!(tab_state(&app), before, "`]` on the last tab");
            assert_eq!(app.flash, None);
            key(&mut app, KeyCode::Char('['), KeyModifiers::NONE);
            assert_eq!(tab_state(&app).0.as_deref(), Some("web"));
            let first = tab_state(&app);
            key(&mut app, KeyCode::Char('['), KeyModifiers::NONE);
            assert_eq!(tab_state(&app), first, "`[` on the first tab");
            assert_eq!(app.flash, None);
            key(&mut app, KeyCode::Char(']'), KeyModifiers::NONE);
            assert_eq!(tab_state(&app).0.as_deref(), Some("demo"));

            // A click on the lit tab changes nothing — not even the card
            // the cursor was walked to.
            key(&mut app, KeyCode::Char('l'), KeyModifiers::NONE);
            let before = tab_state(&app);
            draw(&mut app);
            let (x, y) = crumb_cell(&app, HitTarget::LauncherTab(ProjectId("p1".into())));
            mouse(&mut app, MouseEventKind::Down(MouseButton::Left), x, y);
            assert_eq!(tab_state(&app), before);
        });
    }

    /// The PROJECT TABS read from the project last worked in: a session
    /// launched from the box takes its project's tab to the far left, and
    /// the grid, the lit tab and a later switch stay as they were.
    #[test]
    fn a_launch_brings_its_project_tab_to_the_far_left() {
        with_default_config(|| {
            let mut app = two_tabs();
            assert_eq!(tab_state(&app).2, ["p2", "p1"], "demo is on the right");

            key(&mut app, KeyCode::Char('p'), KeyModifiers::NONE);
            type_text(&mut app, "tidy the nav");
            let out = key(&mut app, KeyCode::Enter, KeyModifiers::NONE);
            assert!(
                out.iter()
                    .any(|r| matches!(r, ClientRequest::CreateAgent { .. })),
                "the launch went out: {out:?}"
            );
            assert_eq!(tab_state(&app).2, ["p1", "p2"], "demo leads now");
            assert_eq!(
                app.selected_project().map(|p| p.name.as_str()),
                Some("demo"),
                "the grid stayed where it was"
            );
            let lit: Vec<bool> = crate::launcher::project_tabs(&app)
                .iter()
                .map(|t| t.active)
                .collect();
            assert_eq!(lit, [true, false], "the lit tab went with it");

            // Looking at web moves nothing; launching there does.
            key(&mut app, KeyCode::Char(']'), KeyModifiers::NONE);
            assert_eq!(tab_state(&app).0.as_deref(), Some("web"));
            assert_eq!(tab_state(&app).2, ["p1", "p2"], "a switch only looks");
            key(&mut app, KeyCode::Char('p'), KeyModifiers::NONE);
            type_text(&mut app, "fix the css");
            key(&mut app, KeyCode::Enter, KeyModifiers::NONE);
            assert_eq!(tab_state(&app).2, ["p2", "p1"]);
        });
    }

    /// A BACKGROUND LAUNCH — the box re-aimed with `^P` at a project with
    /// no tab — is work in that project: it gets a tab at the far left,
    /// while the grid goes on showing the project in front of the user.
    #[test]
    fn a_background_launch_puts_its_project_at_the_far_left() {
        with_default_config(|| {
            let mut app = two_sessions();
            draw(&mut app);
            assert_eq!(tab_state(&app).2, ["p1"], "only demo is open");

            key(&mut app, KeyCode::Char('p'), KeyModifiers::NONE);
            type_text(&mut app, "tidy the nav");
            key(&mut app, KeyCode::Char('p'), KeyModifiers::CONTROL);
            type_text(&mut app, "we");
            key(&mut app, KeyCode::Enter, KeyModifiers::NONE);
            key(&mut app, KeyCode::Enter, KeyModifiers::NONE);

            assert_eq!(tab_state(&app).2, ["p2", "p1"], "web leads");
            assert_eq!(
                app.selected_project().map(|p| p.name.as_str()),
                Some("demo"),
                "the grid never left demo"
            );
            draw(&mut app);
            assert_eq!(tab_state(&app).2, ["p2", "p1"], "the draw keeps it");
        });
    }

    /// Typing at a session in the PANE is work in its project too — a key
    /// and a paste alike bring the tab to the far left, so the project
    /// just typed in is never stuck at the right-hand end.
    #[test]
    fn typing_at_a_session_brings_its_project_tab_to_the_far_left() {
        with_default_config(|| {
            for paste in [false, true] {
                let mut app = two_tabs();
                app.term = Some(crate::app::AttachedTerm::new(
                    SessionRef::Agent(AgentId("a2".into())),
                    40,
                    10,
                ));
                app.focus = Focus::Terminal;
                app.term_locked = true;

                let mut out = Vec::new();
                let event = if paste {
                    crossterm::event::Event::Paste("hello".into())
                } else {
                    crossterm::event::Event::Key(crossterm::event::KeyEvent::new(
                        KeyCode::Char('y'),
                        KeyModifiers::NONE,
                    ))
                };
                handle_terminal_event(&mut app, event, &mut out);

                assert!(
                    matches!(
                        out.as_slice(),
                        [ClientRequest::Input { session, .. }]
                            if *session == SessionRef::Agent(AgentId("a2".into()))
                    ),
                    "paste {paste}: it reached polish-nav: {out:?}"
                );
                assert_eq!(tab_state(&app).2, ["p1", "p2"], "paste {paste}");
            }
        });
    }

    /// Back onto a tab, by `[` / `]`, a digit or a click, the grid lands on
    /// the card the project was left on — its session back in the pane —
    /// not on its first card. A session archived since falls back to the
    /// first.
    #[test]
    fn a_tab_comes_back_on_the_card_it_was_left_on() {
        with_default_config(|| {
            let mut app = two_tabs();
            keys(
                &mut app,
                &[KeyCode::Char('h'), KeyCode::Char('h'), KeyCode::Char('l')],
            );
            assert_eq!(selected(&app).as_deref(), Some("a1"), "demo's second card");

            keys(&mut app, &[KeyCode::Char('['), KeyCode::Char(']')]);
            assert_eq!(tab_state(&app).0.as_deref(), Some("demo"));
            assert_eq!(selected(&app).as_deref(), Some("a1"), "by `[` / `]`");
            assert_eq!(pane(&app), Some(SessionRef::Agent(AgentId("a1".into()))));

            keys(&mut app, &[KeyCode::Char('1'), KeyCode::Char('2')]);
            assert_eq!(selected(&app).as_deref(), Some("a1"), "by the digits");

            keys(&mut app, &[KeyCode::Char('[')]);
            draw(&mut app);
            let (x, y) = crumb_cell(&app, HitTarget::LauncherTab(ProjectId("p1".into())));
            mouse(&mut app, MouseEventKind::Down(MouseButton::Left), x, y);
            assert_eq!(selected(&app).as_deref(), Some("a1"), "by a click");

            // Gone from the cards since: the first card instead.
            keys(&mut app, &[KeyCode::Char('[')]);
            for a in app.tree.agents.iter_mut().filter(|a| a.id.0 == "a1") {
                a.archived = true;
            }
            keys(&mut app, &[KeyCode::Char(']')]);
            assert_eq!(tab_state(&app).0.as_deref(), Some("demo"));
            assert_eq!(selected(&app).as_deref(), Some("a2"), "the first card");
        });
    }

    /// `x` and the `×` on the lit tab close it the same way: the tab goes
    /// and the grid lands on the one that slides into its place. The last
    /// tab closes too, back to the SPLASH nebula opens on with no project,
    /// the projects and their sessions untouched.
    #[test]
    fn closing_the_lit_tab_lands_on_the_one_beside_it() {
        with_default_config(|| {
            let mut by_key = two_tabs();
            key(&mut by_key, KeyCode::Char('x'), KeyModifiers::NONE);

            let mut by_click = two_tabs();
            let (x, y) = crumb_cell(
                &by_click,
                HitTarget::LauncherTabClose(ProjectId("p1".into())),
            );
            mouse(&mut by_click, MouseEventKind::Down(MouseButton::Left), x, y);

            assert_eq!(tab_state(&by_key), tab_state(&by_click));
            assert_eq!(
                tab_state(&by_key),
                (Some("web".into()), Some("a3".into()), vec!["p2".into()])
            );
            // The project itself is untouched: its sessions run on.
            assert!(by_key.tree.projects.iter().any(|p| p.name == "demo"));

            key(&mut by_key, KeyCode::Char('x'), KeyModifiers::NONE);
            assert!(by_key.launcher_tabs.is_empty(), "the last tab closes");
            assert!(by_key.projects_closed);
            assert!(!by_key.launcher_active() && by_key.splash_showing());
            assert_eq!(by_key.flash.as_deref(), Some(super::LAST_TAB));
            assert_eq!(by_key.tree.projects.len(), 2, "no project went");
            draw(&mut by_key);
            assert!(by_key.launcher_tabs.is_empty(), "the draw gives none back");
        });
    }

    /// With every tab closed nebula is back on the SPLASH it opens on with
    /// no project: the grid's keys walk nothing under it, and the `+` there
    /// lists every project as the header's does, in the middle of the
    /// screen — a pick brings the grid back on it, with its tab. The next
    /// start opens on the splash too.
    #[test]
    fn closing_every_tab_goes_back_to_the_splash() {
        with_default_config(|| {
            let mut app = two_tabs();
            keys(&mut app, &[KeyCode::Char('x'), KeyCode::Char('x')]);
            assert!(app.projects_closed && app.splash_showing());
            assert!(app.term.is_none(), "the pane let go");
            draw(&mut app);
            assert!(app.launcher_tabs.is_empty());

            // Keys that would walk the hidden rows do nothing.
            let before = (app.sel_project, app.sel_worktree, app.sel_session);
            keys(&mut app, &[KeyCode::Char('j'), KeyCode::Char('l')]);
            assert_eq!((app.sel_project, app.sel_worktree, app.sel_session), before);
            assert!(app.projects_closed);

            let json = super::super::ui_state_json(&app);
            let mut next = two_sessions();
            super::super::restore_ui_state(&mut next, &json);
            draw(&mut next);
            assert!(next.projects_closed && next.launcher_tabs.is_empty());

            // `+`: every project, none ticked; picking one reopens it.
            key(&mut app, KeyCode::Char('+'), KeyModifiers::NONE);
            let Some(Overlay::Menu(menu)) = &app.overlay else {
                panic!("no dropdown: {:?}", app.overlay);
            };
            let labels: Vec<_> = menu.items.iter().map(|i| i.label.clone()).collect();
            assert_eq!(labels, vec!["demo  (2)", "web  (1)", super::OPEN_FOLDER]);
            assert_eq!(menu.at, None, "no header + here: it sits mid-screen");
            key(&mut app, KeyCode::Enter, KeyModifiers::NONE);
            assert!(app.overlay.is_none(), "{:?}", app.overlay);
            assert!(!app.projects_closed && app.launcher_active());
            draw(&mut app);
            assert_eq!(tabs_drawn(&app), ["demo"]);
        });
    }

    /// Closing a tab the grid is not on moves nothing: not the project,
    /// not the card, not the pane.
    #[test]
    fn closing_another_tab_moves_nothing() {
        with_default_config(|| {
            let mut app = two_tabs();
            let before = tab_state(&app);
            let pane_before = pane(&app);
            let (x, y) = crumb_cell(&app, HitTarget::LauncherTabClose(ProjectId("p2".into())));
            mouse(&mut app, MouseEventKind::Down(MouseButton::Left), x, y);
            let after = tab_state(&app);
            assert_eq!((&after.0, &after.1), (&before.0, &before.1));
            assert_eq!(after.2, ["p1"]);
            assert_eq!(pane(&app), pane_before);
            draw(&mut app);
            assert_eq!(tabs_drawn(&app), ["demo"]);
        });
    }

    /// From `demo`'s root band onto `polish-nav`, whose card runs in the
    /// `feat` checkout — the next band down, its one card under the
    /// cursor as the band's remembered card.
    fn to_feat(app: &mut App) {
        key(app, KeyCode::Char('j'), KeyModifiers::NONE);
    }

    /// Press `code` with no modifiers, once per entry.
    fn keys(app: &mut App, codes: &[KeyCode]) {
        for code in codes {
            key(app, *code, KeyModifiers::NONE);
        }
    }

    /// The header row's cells that wear the header's cursor — the accent
    /// as a block — as text.
    fn tab_cursor_drawn(app: &App, terminal: &Terminal<TestBackend>) -> String {
        let buf = terminal.backend().buffer();
        (0..buf.area.width)
            .filter_map(|x| buf.cell((x, 1)))
            .filter(|c| c.bg == app.theme.accent)
            .map(|c| c.symbol())
            .collect::<String>()
            .trim()
            .to_string()
    }

    /// How a band's rule on the grid looks: the fg its dashes wear,
    /// whether its branch is bold, and whether it opens on the CURSOR
    /// MARK `❯` rather than `──` — the row found by the checkout it
    /// spells (`⌂ main`), since bands scroll.
    fn band_rule_look(terminal: &Terminal<TestBackend>, checkout: &str) -> (Color, bool, bool) {
        let buf = terminal.backend().buffer();
        let want: Vec<String> = checkout.chars().map(String::from).collect();
        for y in 0..buf.area.height {
            let cells: Vec<_> = (0..buf.area.width)
                .filter_map(|x| buf.cell((x, y)))
                .collect();
            let Some(at) = cells
                .windows(want.len())
                .position(|w| w.iter().zip(&want).all(|(c, s)| c.symbol() == s))
            else {
                continue;
            };
            // The rule opens on `──`, or on `❯ ` with the keys on it; its
            // dashes run on past the checkout either way, and those say
            // what the rule wears.
            let marked = cells[..at].iter().any(|c| c.symbol() == "❯");
            if !marked && !cells[..at].iter().any(|c| c.symbol() == "─") {
                continue;
            }
            let Some(dash) = cells[at..].iter().find(|c| c.symbol() == "─") else {
                continue;
            };
            let bold = cells[at + want.len() - 1].modifier.contains(Modifier::BOLD);
            return (dash.fg, bold, marked);
        }
        let rows: Vec<String> = (0..buf.area.height)
            .map(|y| {
                (0..buf.area.width)
                    .filter_map(|x| buf.cell((x, y)))
                    .map(|c| c.symbol().to_string())
                    .collect()
            })
            .collect();
        panic!("no band rule spells {checkout:?}:\n{}", rows.join("\n"))
    }

    /// The band under the cursor wears the accent only while the keys are
    /// on it: `k`,`k` up into the PROJECT TABS turns its rule gray — the
    /// header's cursor is the one lit thing — with its branch still bold,
    /// since the pane still reads that checkout; Enter back down lights
    /// it again. The keys in the pane under the grid gray it the same way.
    /// The CURSOR MARK `❯` at the rule's left goes with the accent: it
    /// says Enter acts here, and up on the tabs or down in the pane it
    /// does not.
    #[test]
    fn the_band_under_the_cursor_goes_gray_while_the_tabs_have_the_keys() {
        with_default_config(|| {
            let mut app = two_tabs();
            let terminal = draw(&mut app);
            let th = app.theme;
            assert_eq!(app.launcher_tab_cursor, None);
            assert_eq!(
                band_rule_look(&terminal, "⌂ main"),
                (th.accent, true, true),
                "the keys are on the root band"
            );
            keys(&mut app, &[KeyCode::Char('k'), KeyCode::Char('k')]);
            assert_eq!(app.launcher_tab_cursor, Some(ProjectId("p1".into())));
            let terminal = draw(&mut app);
            assert!(
                !tab_cursor_drawn(&app, &terminal).is_empty(),
                "the header's cursor is the lit thing"
            );
            assert_eq!(
                band_rule_look(&terminal, "⌂ main"),
                (th.edge, true, false),
                "the band's rule is gray, its branch still bold"
            );

            key(&mut app, KeyCode::Enter, KeyModifiers::NONE);
            assert_eq!(app.launcher_tab_cursor, None);
            let terminal = draw(&mut app);
            assert_eq!(
                band_rule_look(&terminal, "⌂ main"),
                (th.accent, true, true),
                "the keys are the band's again"
            );

            // The pane under the grid holding the keys grays it too. The
            // attach is the daemon's to answer; the pane's hold on the
            // band's card is built by hand, as the pane's own tests do.
            if app.term.is_none() {
                app.term = Some(crate::app::AttachedTerm::new(
                    SessionRef::Agent(AgentId("a1".into())),
                    40,
                    10,
                ));
            }
            let pane = app.term_area;
            mouse(
                &mut app,
                MouseEventKind::Down(MouseButton::Left),
                pane.x + 2,
                pane.y + 1,
            );
            assert_eq!(app.focus, Focus::Terminal, "the pane has the keys");
            let terminal = draw(&mut app);
            assert_eq!(
                band_rule_look(&terminal, "⌂ main"),
                (th.edge, true, false),
                "the band's rule is gray with the keys in the pane"
            );
        });
    }

    /// Up into the PROJECT TABS: `k` on the top row of cards stays put and
    /// says what a second press does, and `k`,`k` hands the keys to the
    /// header with its cursor on the lit tab. `h` / `l` walk that cursor
    /// — stopping at either end, as `[` / `]` do — and the grid switches
    /// with it, no Enter needed: each project comes up on the card it was
    /// left on, its session in the pane. Enter hands the keys back to the
    /// cards of the project on screen.
    #[test]
    fn k_k_on_the_top_row_walks_up_into_the_project_tabs() {
        with_default_config(|| {
            let mut app = two_tabs();
            draw(&mut app);
            assert_eq!(
                selected(&app).as_deref(),
                Some("a1"),
                "the root band's card"
            );
            let before = tab_state(&app);

            key(&mut app, KeyCode::Char('k'), KeyModifiers::NONE);
            assert_eq!(app.launcher_tab_cursor, None, "one press stays put");
            assert!(
                app.flash
                    .as_deref()
                    .is_some_and(|f| f.ends_with("again: project tabs")),
                "{:?}",
                app.flash
            );
            key(&mut app, KeyCode::Char('k'), KeyModifiers::NONE);
            assert_eq!(
                app.launcher_tab_cursor,
                Some(ProjectId("p1".into())),
                "on the lit tab"
            );
            assert_eq!(tab_state(&app), before, "nothing opened");

            key(&mut app, KeyCode::Char('h'), KeyModifiers::NONE);
            assert_eq!(app.launcher_tab_cursor, Some(ProjectId("p2".into())));
            let on_web = (Some("web".into()), Some("a3".into()), before.2.clone());
            assert_eq!(tab_state(&app), on_web, "the grid comes with the cursor");
            assert_eq!(
                pane(&app),
                Some(SessionRef::Agent(AgentId("a3".into()))),
                "and the pane reads web's session"
            );
            key(&mut app, KeyCode::Left, KeyModifiers::NONE);
            assert_eq!(
                app.launcher_tab_cursor,
                Some(ProjectId("p2".into())),
                "← on the first tab stays on it"
            );
            keys(&mut app, &[KeyCode::Right, KeyCode::Right]);
            assert_eq!(
                app.launcher_tab_cursor,
                Some(ProjectId("p1".into())),
                "→ past the last tab stays on it"
            );
            assert_eq!(tab_state(&app), before, "demo, on the card it was left on");
            key(&mut app, KeyCode::Char('['), KeyModifiers::NONE);
            assert_eq!(
                app.launcher_tab_cursor,
                Some(ProjectId("p2".into())),
                "`[` walks the cursor too"
            );
            assert_eq!(tab_state(&app), on_web);

            key(&mut app, KeyCode::Enter, KeyModifiers::NONE);
            assert_eq!(
                app.launcher_tab_cursor, None,
                "the keys are the cards' again"
            );
            assert_eq!(
                tab_state(&app),
                (
                    Some("web".into()),
                    Some("a3".into()),
                    vec!["p2".into(), "p1".into()]
                )
            );
            assert_eq!(app.focus, Focus::Sessions);
        });
    }

    /// Only the top row walks up: from a band with one over it `k` is the
    /// step onto that band, and the double tap starts there. `↑` is the
    /// same key.
    #[test]
    fn k_below_the_top_row_is_a_step_up_the_grid() {
        with_default_config(|| {
            let mut app = two_tabs();
            draw(&mut app);
            keys(&mut app, &[KeyCode::Esc, KeyCode::Char('j')]);
            assert_eq!(selected(&app).as_deref(), Some("a2"), "the second band");
            assert_eq!(app.launcher_tab_cursor, None);

            key(&mut app, KeyCode::Up, KeyModifiers::NONE);
            assert_eq!(selected(&app).as_deref(), Some("a1"), "a band up");
            assert_eq!(app.launcher_tab_cursor, None, "and not armed by the step");
            key(&mut app, KeyCode::Up, KeyModifiers::NONE);
            assert_eq!(app.launcher_tab_cursor, None, "the top band's first press");
            key(&mut app, KeyCode::Up, KeyModifiers::NONE);
            assert_eq!(app.launcher_tab_cursor, Some(ProjectId("p1".into())));
        });
    }

    /// `j`,`j` is the way back down: the first press stays in the header
    /// and says where a second one goes, the second hands the keys back to
    /// the card the grid is showing — the one walked to before going up,
    /// not the top of the grid.
    #[test]
    fn j_j_from_the_tabs_goes_back_down_to_the_card_left() {
        with_default_config(|| {
            let mut app = two_tabs();
            keys(
                &mut app,
                &[KeyCode::Char('h'), KeyCode::Char('h'), KeyCode::Char('l')],
            );
            assert_eq!(selected(&app).as_deref(), Some("a1"), "the second card");
            keys(&mut app, &[KeyCode::Char('k'), KeyCode::Char('k')]);

            key(&mut app, KeyCode::Char('j'), KeyModifiers::NONE);
            assert_eq!(app.launcher_tab_cursor, Some(ProjectId("p1".into())));
            assert!(
                app.flash
                    .as_deref()
                    .is_some_and(|f| f.ends_with("again: into demo")),
                "{:?}",
                app.flash
            );
            key(&mut app, KeyCode::Char('j'), KeyModifiers::NONE);
            assert_eq!(app.launcher_tab_cursor, None);
            assert_eq!(tab_state(&app).0.as_deref(), Some("demo"));
            assert_eq!(selected(&app).as_deref(), Some("a1"), "the card it was on");

            // And down into the tab the cursor was walked onto.
            keys(
                &mut app,
                &[
                    KeyCode::Char('k'),
                    KeyCode::Char('k'),
                    KeyCode::Char('h'),
                    KeyCode::Down,
                    KeyCode::Down,
                ],
            );
            assert_eq!(app.launcher_tab_cursor, None);
            assert_eq!(tab_state(&app).0.as_deref(), Some("web"));
            assert_eq!(selected(&app).as_deref(), Some("a3"));

            // And back up and over to demo, which comes up on the card it
            // was left on rather than its first.
            keys(
                &mut app,
                &[KeyCode::Char('k'), KeyCode::Char('k'), KeyCode::Char('l')],
            );
            assert_eq!(tab_state(&app).0.as_deref(), Some("demo"));
            assert_eq!(selected(&app).as_deref(), Some("a1"));
        });
    }

    /// Esc goes back down on the project the header's cursor walked the
    /// grid to, its card still selected. So does any key the header has
    /// no use for, which then means what it means on the grid — `p` the
    /// box.
    #[test]
    fn esc_or_another_key_hands_the_keys_back_to_the_cards() {
        with_default_config(|| {
            let mut app = two_tabs();
            keys(
                &mut app,
                &[KeyCode::Char('h'), KeyCode::Char('h'), KeyCode::Char('l')],
            );
            keys(
                &mut app,
                &[KeyCode::Char('k'), KeyCode::Char('k'), KeyCode::Char('h')],
            );
            assert!(app.launcher_tab_cursor.is_some());
            let walked = tab_state(&app);
            assert_eq!(walked.0.as_deref(), Some("web"));

            key(&mut app, KeyCode::Esc, KeyModifiers::NONE);
            assert_eq!(app.launcher_tab_cursor, None);
            assert_eq!(tab_state(&app), walked, "still on the project walked to");
            assert!(!app.launcher_unaimed, "and the card is still selected");

            keys(
                &mut app,
                &[KeyCode::Char('k'), KeyCode::Char('k'), KeyCode::Char('p')],
            );
            assert_eq!(app.launcher_tab_cursor, None);
            launch(&app);
        });
    }

    /// INPUT PARITY: with the header holding the keys, a click on a tab is
    /// the walk onto it and Enter — one [`choose_tab`], onto the card the
    /// project was left on — and a click on a card hands the keys back to
    /// the cards.
    #[test]
    fn a_click_on_a_tab_from_the_header_is_enter_on_it() {
        with_default_config(|| {
            let up = [
                KeyCode::Char('h'),
                KeyCode::Char('h'),
                KeyCode::Char('l'),
                KeyCode::Char('k'),
                KeyCode::Char('k'),
            ];
            let mut by_key = two_tabs();
            keys(&mut by_key, &up);
            key(&mut by_key, KeyCode::Enter, KeyModifiers::NONE);

            let mut by_click = two_tabs();
            keys(&mut by_click, &up);
            draw(&mut by_click);
            let (x, y) = crumb_cell(&by_click, HitTarget::LauncherTab(ProjectId("p1".into())));
            mouse(&mut by_click, MouseEventKind::Down(MouseButton::Left), x, y);

            assert_eq!(tab_state(&by_key), tab_state(&by_click));
            assert_eq!(pane(&by_key), pane(&by_click));
            assert_eq!(by_key.launcher_tab_cursor, by_click.launcher_tab_cursor);
            assert_eq!(selected(&by_key).as_deref(), Some("a1"), "the card left");

            // Onto the other tab: `h` and Enter, or a click on it.
            let mut by_key = two_tabs();
            keys(&mut by_key, &up);
            keys(&mut by_key, &[KeyCode::Char('h'), KeyCode::Enter]);

            let mut by_click = two_tabs();
            keys(&mut by_click, &up);
            draw(&mut by_click);
            let (x, y) = crumb_cell(&by_click, HitTarget::LauncherTab(ProjectId("p2".into())));
            mouse(&mut by_click, MouseEventKind::Down(MouseButton::Left), x, y);

            assert_eq!(tab_state(&by_key), tab_state(&by_click));
            assert_eq!(pane(&by_key), pane(&by_click));
            assert_eq!(by_key.launcher_tab_cursor, by_click.launcher_tab_cursor);
            assert_eq!(tab_state(&by_key).0.as_deref(), Some("web"));

            let mut app = two_tabs();
            keys(&mut app, &up);
            draw(&mut app);
            let (x, y) = card_cell(&app, &SessionRef::Agent(AgentId("a1".into())));
            mouse(&mut app, MouseEventKind::Down(MouseButton::Left), x, y);
            assert_eq!(app.launcher_tab_cursor, None);
            assert_eq!(selected(&app).as_deref(), Some("a1"));
        });
    }

    /// `x` with the header holding the keys closes the tab under its
    /// cursor — the project the walk put on screen — and the grid and the
    /// cursor both take the tab that slid into its place, the grid on the
    /// card it was left on.
    #[test]
    fn x_in_the_header_closes_the_tab_under_its_cursor() {
        with_default_config(|| {
            let mut app = two_tabs();
            keys(&mut app, &[KeyCode::Char('h'), KeyCode::Char('h')]);
            let before = tab_state(&app);
            keys(
                &mut app,
                &[
                    KeyCode::Char('k'),
                    KeyCode::Char('k'),
                    KeyCode::Char('h'),
                    KeyCode::Char('x'),
                ],
            );
            assert_eq!(
                app.launcher_tabs,
                [ProjectId("p1".into())],
                "web's tab went"
            );
            assert_eq!(
                (tab_state(&app).0, tab_state(&app).1),
                (before.0, before.1),
                "the grid back on demo, on its card"
            );
            assert_eq!(app.launcher_tab_cursor, Some(ProjectId("p1".into())));
        });
    }

    /// `Backspace`, `Delete` and `d` with the header holding the keys ask
    /// before closing the tab under the cursor: the tab is still there
    /// behind the dialog, `Esc` keeps it with the cursor on it, and the
    /// dialog's Enter is `x` — the grid and the cursor land where `x` lands
    /// them — the last tab too, which closes to the SPLASH. Down on the
    /// cards the same keys are still the card's own delete.
    #[test]
    fn the_delete_keys_in_the_header_ask_before_closing_the_tab() {
        with_default_config(|| {
            let up = [
                KeyCode::Char('h'),
                KeyCode::Char('h'),
                KeyCode::Char('k'),
                KeyCode::Char('k'),
                KeyCode::Char('h'),
            ];
            let mut by_x = two_tabs();
            keys(&mut by_x, &up);
            key(&mut by_x, KeyCode::Char('x'), KeyModifiers::NONE);
            assert_eq!(by_x.launcher_tabs, [ProjectId("p1".into())]);

            for code in [KeyCode::Backspace, KeyCode::Delete, KeyCode::Char('d')] {
                let mut app = two_tabs();
                keys(&mut app, &up);
                let before = tab_state(&app);
                key(&mut app, code, KeyModifiers::NONE);
                let closing = match &app.overlay {
                    Some(Overlay::Confirm(c)) => match &c.action {
                        PendingAction::CloseProjectTab(id) => Some(id.clone()),
                        _ => None,
                    },
                    _ => None,
                };
                assert_eq!(
                    closing,
                    Some(ProjectId("p2".into())),
                    "{code:?} asks about web's tab: {:?}",
                    app.overlay
                );
                assert_eq!(tab_state(&app), before, "{code:?} closed nothing yet");

                // Esc keeps the tab, and the cursor on it.
                key(&mut app, KeyCode::Esc, KeyModifiers::NONE);
                assert!(app.overlay.is_none());
                assert_eq!(tab_state(&app), before, "{code:?} Esc kept the tab");
                assert_eq!(app.launcher_tab_cursor, Some(ProjectId("p2".into())));

                // Enter on the dialog is x.
                key(&mut app, code, KeyModifiers::NONE);
                key(&mut app, KeyCode::Enter, KeyModifiers::NONE);
                assert!(app.overlay.is_none());
                assert_eq!(tab_state(&app), tab_state(&by_x), "{code:?} Enter is x");
                assert_eq!(app.launcher_tab_cursor, by_x.launcher_tab_cursor);
                assert_eq!(app.launcher_tab_cursor, Some(ProjectId("p1".into())));

                // The only tab left asks too, and its Enter closes it.
                key(&mut app, code, KeyModifiers::NONE);
                assert!(
                    matches!(&app.overlay, Some(Overlay::Confirm(c))
                        if c.action == PendingAction::CloseProjectTab(ProjectId("p1".into()))),
                    "{code:?} asks about the last tab: {:?}",
                    app.overlay
                );
                key(&mut app, KeyCode::Enter, KeyModifiers::NONE);
                assert!(app.launcher_tabs.is_empty());
                assert_eq!(app.launcher_tab_cursor, None);
                assert!(app.projects_closed);
            }

            // Down on the cards the same key is the card's own delete.
            let mut app = two_tabs();
            keys(&mut app, &[KeyCode::Char('h'), KeyCode::Char('h')]);
            key(&mut app, KeyCode::Backspace, KeyModifiers::NONE);
            assert!(
                matches!(&app.overlay, Some(Overlay::Confirm(c))
                    if matches!(c.action, PendingAction::DeleteAgent(_))),
                "{:?}",
                app.overlay
            );
            assert_eq!(app.launcher_tabs.len(), 2);
        });
    }

    /// The header's cursor is drawn: the tab it is on wears the accent as
    /// a block, apart from the lit tab, and the footer names the header's
    /// keys. Esc takes both away.
    #[test]
    fn the_header_cursor_is_drawn_on_its_tab() {
        with_default_config(|| {
            let mut app = two_tabs();
            keys(&mut app, &[KeyCode::Char('h'), KeyCode::Char('h')]);
            let terminal = draw(&mut app);
            assert_eq!(tab_cursor_drawn(&app, &terminal), "");

            keys(&mut app, &[KeyCode::Char('k'), KeyCode::Char('k')]);
            let terminal = draw(&mut app);
            assert_eq!(tab_cursor_drawn(&app, &terminal), "demo");
            key(&mut app, KeyCode::Char('h'), KeyModifiers::NONE);
            let terminal = draw(&mut app);
            assert_eq!(tab_cursor_drawn(&app, &terminal), "web");
            assert!(
                buffer_text(&terminal).contains("switch project"),
                "{}",
                buffer_text(&terminal)
            );

            key(&mut app, KeyCode::Esc, KeyModifiers::NONE);
            let terminal = draw(&mut app);
            assert_eq!(tab_cursor_drawn(&app, &terminal), "");
            assert!(!buffer_text(&terminal).contains("switch project"));
        });
    }

    /// The tabs are remembered across a restart, in the order they were
    /// left — less any project the tree no longer has.
    #[test]
    fn the_tabs_come_back_after_a_restart() {
        with_default_config(|| {
            let mut app = two_tabs();
            app.launcher_tabs.insert(1, ProjectId("gone".into()));
            let json = super::super::ui_state_json(&app);

            let mut next = two_sessions();
            super::super::restore_ui_state(&mut next, &json);
            assert_eq!(
                next.launcher_tabs,
                [ProjectId("p2".into()), ProjectId("p1".into())]
            );
            draw(&mut next);
            assert_eq!(tabs_drawn(&next), ["web", "demo"]);
        });
    }

    /// A third project, `docs`, with a checkout and no sessions in it.
    fn seed_empty_project(app: &mut App) {
        hse(
            app,
            ServerEvent::EntityUpserted {
                entity: Entity::Project(Project {
                    id: ProjectId("p3".into()),
                    name: "docs".into(),
                    repo_path: "/tmp/docs".into(),
                    sort_order: 2,
                }),
            },
        );
        hse(
            app,
            ServerEvent::EntityUpserted {
                entity: Entity::Worktree(Worktree {
                    id: WorktreeId("w3root".into()),
                    project_id: ProjectId("p3".into()),
                    path: "/tmp/docs".into(),
                    branch: "main".into(),
                    is_main: true,
                    sort_order: 0,
                }),
            },
        );
    }

    /// A tab is a word, and a word says nothing about being clickable —
    /// so the one under the pointer is underlined for as long as it rests
    /// there, and only that one: a tab's name, or the `+`.
    #[test]
    fn the_pointer_underlines_the_tab_it_rests_on() {
        with_default_config(|| {
            let mut app = two_sessions();
            let terminal = draw(&mut app);
            assert_eq!(
                underlined_head(&terminal),
                "",
                "nothing is under the pointer yet"
            );

            // The tab, with the pointer on it: its name alone underlines.
            let demo = HitTarget::LauncherTab(ProjectId("p1".into()));
            let (demo_x, demo_y) = crumb_cell(&app, demo.clone());
            mouse(&mut app, MouseEventKind::Moved, demo_x, demo_y);
            assert_eq!(app.hover_crumb, Some(demo));
            assert_eq!(underlined_head(&draw(&mut app)), "demo");

            // The `+` is a button too.
            let (add_x, add_y) = crumb_cell(&app, HitTarget::LauncherTabAdd);
            mouse(&mut app, MouseEventKind::Moved, add_x + 1, add_y);
            assert_eq!(app.hover_crumb, Some(HitTarget::LauncherTabAdd));
            assert_eq!(underlined_head(&draw(&mut app)), "+");

            // And the pointer off the row takes the underline with it.
            mouse(&mut app, MouseEventKind::Moved, demo_x, demo_y + 6);
            assert_eq!(app.hover_crumb, None);
            assert_eq!(underlined_head(&draw(&mut app)), "");

            // The hatch out of a full-screen session is the same kind of
            // button, and wears the same underline.
            let mut full = two_sessions();
            draw(&mut full);
            full_screen(&mut full);
            draw(&mut full);
            let (x, y) = crumb_cell(&full, HitTarget::LauncherCrumb);
            mouse(&mut full, MouseEventKind::Moved, x, y);
            assert_eq!(underlined_head(&draw(&mut full)), "‹ sessions");
        });
    }

    /// INPUT PARITY: the `‹ sessions` crumb is the hatch — a click on it
    /// leaves the full-screen session for the grid exactly as `^q` does.
    #[test]
    fn a_click_on_the_crumb_is_the_hatch_out() {
        with_default_config(|| {
            let mut by_key = two_sessions();
            draw(&mut by_key);
            full_screen(&mut by_key);
            draw(&mut by_key);
            key(&mut by_key, KeyCode::Char('q'), KeyModifiers::CONTROL);

            let mut by_click = two_sessions();
            draw(&mut by_click);
            full_screen(&mut by_click);
            draw(&mut by_click);
            let (rect, _) = by_click
                .hits
                .iter()
                .find(|(_, hit)| *hit == HitTarget::LauncherCrumb)
                .expect("the crumb is a button");
            let (x, y) = (rect.x + 1, rect.y);
            mouse(&mut by_click, MouseEventKind::Down(MouseButton::Left), x, y);

            assert_eq!(by_click.focus, by_key.focus);
            assert_eq!(by_click.collapsed, by_key.collapsed);
            assert_eq!(by_click.term_locked, by_key.term_locked);
            assert_eq!(selected(&by_click), selected(&by_key));
        });
    }

    /// `^F` in the pane full-screens its session, keys still in it and
    /// the chord never forwarded to the agent; pressed again it comes back
    /// down to the pane beside the cards. In full-screen `^q` and `^``
    /// bring it back down too — to the pane, not straight out to the grid.
    #[test]
    fn ctrl_f_full_screens_the_pane_and_brings_it_back_down() {
        with_default_config(|| {
            let ctrl_f = |app: &mut App| key(app, KeyCode::Char('f'), KeyModifiers::CONTROL);
            let in_pane = |app: &App| {
                assert_eq!(app.focus, Focus::Terminal, "the keys stay in the session");
                assert!(app.term_locked);
                assert!(!app.collapsed, "back in the pane beside the cards");
            };
            let mut app = two_sessions();
            draw(&mut app);
            to_feat(&mut app);
            key(&mut app, KeyCode::Enter, KeyModifiers::NONE);
            in_pane(&app);

            for back_down in [
                (KeyCode::Char('f'), KeyModifiers::CONTROL),
                (KeyCode::Char('q'), KeyModifiers::CONTROL),
                (KeyCode::Char('`'), KeyModifiers::CONTROL),
            ] {
                let out = ctrl_f(&mut app);
                assert!(
                    !out.iter().any(|r| matches!(r, ClientRequest::Input { .. })),
                    "^F is nebula's, not the agent's: {out:?}"
                );
                assert!(app.collapsed, "full-screen");
                assert_eq!(app.focus, Focus::Terminal);
                assert!(app.term_locked);
                let text = buffer_text(&draw(&mut app));
                assert!(text.contains("‹ sessions"), "the crumb header: {text}");
                assert!(tabs_drawn(&app).is_empty(), "no grid header: {text}");

                let out = key(&mut app, back_down.0, back_down.1);
                assert!(
                    !out.iter().any(|r| matches!(r, ClientRequest::Input { .. })),
                    "{back_down:?} is nebula's: {out:?}"
                );
                in_pane(&app);
                draw(&mut app);
                assert_eq!(tabs_drawn(&app), ["demo"], "the grid is back");
                assert_eq!(pane(&app), Some(SessionRef::Agent(AgentId("a2".into()))));
            }
        });
    }

    /// `^F` from the cards full-screens the card under the cursor.
    #[test]
    fn ctrl_f_on_a_card_full_screens_it() {
        with_default_config(|| {
            let mut app = two_sessions();
            draw(&mut app);
            to_feat(&mut app);
            assert_eq!(app.focus, Focus::Sessions);
            key(&mut app, KeyCode::Char('f'), KeyModifiers::CONTROL);
            assert!(app.collapsed);
            assert_eq!(app.focus, Focus::Terminal);
            assert!(app.term_locked);
            assert_eq!(pane(&app), Some(SessionRef::Agent(AgentId("a2".into()))));
        });
    }

    /// INPUT PARITY: the pane header's `⤢` is `^F`, and a full-screen
    /// session's `⤡` is `^F` pressed there — one toggle behind all four.
    #[test]
    fn the_full_screen_buttons_are_ctrl_f() {
        with_default_config(|| {
            let entered = || {
                let mut app = two_sessions();
                draw(&mut app);
                to_feat(&mut app);
                key(&mut app, KeyCode::Enter, KeyModifiers::NONE);
                draw(&mut app);
                app
            };
            let click = |app: &mut App| {
                let (x, y) = crumb_cell(app, HitTarget::LauncherPaneZoom);
                mouse(app, MouseEventKind::Down(MouseButton::Left), x + 1, y);
            };
            let same = |a: &App, b: &App| {
                assert_eq!(a.collapsed, b.collapsed);
                assert_eq!(a.focus, b.focus);
                assert_eq!(a.term_locked, b.term_locked);
                assert_eq!(pane(a), pane(b));
            };

            let mut by_click = entered();
            let head = buffer_text(&draw(&mut by_click));
            assert!(head.contains('⤢'), "the pane has the button: {head}");
            click(&mut by_click);
            let mut by_key = entered();
            key(&mut by_key, KeyCode::Char('f'), KeyModifiers::CONTROL);
            assert!(by_click.collapsed);
            same(&by_click, &by_key);

            let head = buffer_text(&draw(&mut by_click));
            assert!(head.contains('⤡'), "full-screen has its way back: {head}");
            click(&mut by_click);
            draw(&mut by_key);
            key(&mut by_key, KeyCode::Char('f'), KeyModifiers::CONTROL);
            assert!(!by_click.collapsed);
            same(&by_click, &by_key);
        });
    }

    /// Enter with the cursor on a card the pane is not showing yet — the
    /// grid never shows one — brings that session up in the pane, rather
    /// than saying there is nothing to enter.
    #[test]
    fn enter_brings_up_the_cursors_session_first() {
        with_default_config(|| {
            let mut app = two_sessions();
            draw(&mut app);
            assert_eq!(pane(&app), None);
            key(&mut app, KeyCode::Enter, KeyModifiers::NONE);
            assert_eq!(pane(&app), Some(SessionRef::Agent(AgentId("a1".into()))));
            assert_eq!(app.focus, Focus::Terminal);
            assert!(app.term_locked);
            assert!(!app.collapsed, "the pane under the grid, not full-screen");
        });
    }

    /// A terminal too short for a pane draws none, so the ways into a
    /// session full-screen it instead of handing the keys to a pane that
    /// is not on the screen.
    #[test]
    fn enter_full_screens_a_session_when_the_body_has_no_pane() {
        with_default_config(|| {
            let mut app = two_sessions();
            draw_at(&mut app, 130, 20);
            assert!(
                crate::launcher::split(app.launcher_body, app.launcher_pane_h)
                    .1
                    .is_none(),
                "too short for a pane"
            );
            key(&mut app, KeyCode::Enter, KeyModifiers::NONE);
            assert_eq!(app.focus, Focus::Terminal);
            assert!(app.term_locked);
            assert!(app.collapsed, "full-screen: there is no pane to step into");
        });
    }

    /// A held `a` opens ONE confirm and archives nothing by itself. The
    /// host repeats a held key — marked as repeats under the kitty
    /// protocol, as more presses without it — and every one lands on the
    /// dialog the first press opened, where `a` is nothing. Enter on the
    /// dialog is the one archive, and the card the cursor lands on needs
    /// `a` pressed again. No RELEASE WATCH is armed: nothing ran on the
    /// press (event_loop/release_watch.rs is `u`'s).
    #[test]
    fn a_held_down_opens_one_confirm_and_archives_nothing_by_itself() {
        use crossterm::event::KeyEventKind::{Release, Repeat};
        with_default_config(|| {
            let mut app = three_sessions();
            draw(&mut app);
            assert_eq!(cards(&app), ["a9", "a2", "a1"]);
            let (x, y) = row_cell(&app, 2);
            mouse(&mut app, MouseEventKind::Down(MouseButton::Left), x, y);
            assert_eq!(selected(&app).as_deref(), Some("a1"), "the last card");

            key(&mut app, KeyCode::Char('a'), KeyModifiers::NONE);
            assert!(
                matches!(&app.overlay, Some(Overlay::Confirm(c))
                    if c.action == PendingAction::ArchiveAgent(AgentId("a1".into()))),
                "the press asks: {:?}",
                app.overlay
            );
            assert_eq!(cards(&app), ["a9", "a2", "a1"], "and archives nothing yet");
            assert!(
                app.release_watch.is_none(),
                "nothing ran, so no key to watch"
            );
            for _ in 0..5 {
                key_kind(&mut app, KeyCode::Char('a'), KeyModifiers::NONE, Repeat);
                key(&mut app, KeyCode::Char('a'), KeyModifiers::NONE);
            }
            assert!(
                matches!(&app.overlay, Some(Overlay::Confirm(_))),
                "marked or not, the repeats land on the one dialog: {:?}",
                app.overlay
            );
            assert_eq!(
                cards(&app),
                ["a9", "a2", "a1"],
                "a held `a` archives nothing"
            );
            key_kind(&mut app, KeyCode::Char('a'), KeyModifiers::NONE, Release);

            key(&mut app, KeyCode::Enter, KeyModifiers::NONE);
            assert!(app.overlay.is_none(), "Enter answers it");
            assert_eq!(cards(&app), ["a9", "a2"], "and that is the archive");
            assert_eq!(
                selected(&app).as_deref(),
                Some("a9"),
                "the card before it in its band"
            );
            assert!(app.release_watch.is_none(), "still nothing to watch");
        });
    }

    /// The watch is the unarchive key's alone: a held `h` keeps walking
    /// the row while it is on, and another key let go — rolled keys
    /// release `h` after `u` went down — leaves it on, so the `u` still
    /// held unarchives nothing; any fresh press ends it.
    #[test]
    fn the_watch_swallows_only_the_unarchive_key() {
        use crossterm::event::KeyEventKind::{Release, Repeat};
        with_default_config(|| {
            let mut app = three_sessions();
            for agent in &mut app.tree.agents {
                agent.archived = true;
            }
            draw(&mut app);
            key(&mut app, KeyCode::Char('A'), KeyModifiers::SHIFT);
            draw(&mut app);
            assert_eq!(
                cards(&app),
                ["a9", "a2", "a1"],
                "the archived view, all three"
            );
            let (x, y) = row_cell(&app, 2);
            mouse(&mut app, MouseEventKind::Down(MouseButton::Left), x, y);
            assert_eq!(selected(&app).as_deref(), Some("a1"), "the last card");
            key(&mut app, KeyCode::Char('u'), KeyModifiers::NONE);
            assert_eq!(cards(&app), ["a9", "a2"], "the press unarchives one");
            assert_eq!(
                selected(&app).as_deref(),
                Some("a9"),
                "the card before it in its band"
            );
            assert!(app.release_watch.is_some(), "and the key is watched");

            key_kind(&mut app, KeyCode::Char('h'), KeyModifiers::NONE, Release);
            assert!(
                app.release_watch.is_some(),
                "another key let go changes nothing"
            );
            key_kind(&mut app, KeyCode::Char('u'), KeyModifiers::NONE, Repeat);
            assert_eq!(cards(&app), ["a9", "a2"], "the held `u` is still swallowed");

            key_kind(&mut app, KeyCode::Char('h'), KeyModifiers::NONE, Repeat);
            assert_eq!(
                selected(&app).as_deref(),
                Some("a9"),
                "a held `h` still walks"
            );
            assert!(app.release_watch.is_some(), "with the watch still on");

            key(&mut app, KeyCode::Char('l'), KeyModifiers::NONE);
            assert!(
                app.release_watch.is_none(),
                "a fresh press of anything ends it"
            );
        });
    }

    /// `u` in the ARCHIVED VIEW is the same key the other way: held, it
    /// unarchives one card.
    #[test]
    fn u_held_down_unarchives_one_card() {
        use crossterm::event::KeyEventKind::Repeat;
        with_default_config(|| {
            let mut app = three_sessions();
            for agent in &mut app.tree.agents {
                agent.archived = true;
            }
            draw(&mut app);
            key(&mut app, KeyCode::Char('A'), KeyModifiers::SHIFT);
            draw(&mut app);
            assert_eq!(cards(&app).len(), 3, "the archived view, all three");
            // The tree holds other projects' sessions too: count the
            // archived ones relative to before the press.
            let archived = |app: &App| app.tree.agents.iter().filter(|a| a.archived).count();
            let before = archived(&app);
            key(&mut app, KeyCode::Char('u'), KeyModifiers::NONE);
            assert_eq!(archived(&app), before - 1, "the press unarchives one");
            assert!(app.release_watch.is_some(), "and the key is watched");
            for _ in 0..5 {
                key_kind(&mut app, KeyCode::Char('u'), KeyModifiers::NONE, Repeat);
            }
            assert_eq!(archived(&app), before - 1, "held, it unarchives no more");
        });
    }

    /// `a`, answered, archives the session under the cursor. The cursor
    /// takes the card after it — the one that slides up into its place —
    /// and on the first card that is the whole of the grid's landing.
    #[test]
    fn archiving_the_first_card_takes_the_card_that_slides_up() {
        with_default_config(|| {
            let mut app = two_sessions();
            draw(&mut app);
            to_feat(&mut app);
            assert_eq!(selected(&app).as_deref(), Some("a2"));
            key(&mut app, KeyCode::Char('a'), KeyModifiers::NONE);
            key(&mut app, KeyCode::Enter, KeyModifiers::NONE);
            assert!(
                app.tree.agents.iter().any(|a| a.id.0 == "a2" && a.archived),
                "archived on the confirm's Enter"
            );
            assert_eq!(
                selected(&app).as_deref(),
                Some("a1"),
                "its band went with it: the root band's card, at the band level"
            );
            assert!(
                app.launcher_expanded.is_none(),
                "the bands, not an empty worktree"
            );
            assert_eq!(app.focus, Focus::Sessions);
        });
    }

    /// The only card of a band leaving takes the band with it — and its
    /// ACCORDION, when it was the band open: the cursor lands on the band
    /// that slid up into its slot, collapsed like the rest, and the pane
    /// on that band's card. The archive runs on the confirm's Enter, a
    /// second input event, and that is the one the landing is kept
    /// across.
    #[test]
    fn archiving_a_bands_only_card_lands_on_the_band_that_slides_up() {
        with_default_config(|| {
            let mut app = three_sessions();
            draw(&mut app);
            assert_eq!(cards(&app), ["a9", "a2", "a1"], "newest first");
            let root_card = selected(&app);
            draw_at(&mut app, 130, 50);
            let (x, y) = row_cell(&app, 1);
            mouse(&mut app, MouseEventKind::Down(MouseButton::Left), x, y);
            key(&mut app, KeyCode::Tab, KeyModifiers::NONE);
            assert_eq!(selected(&app).as_deref(), Some("a2"), "feat's one card");
            assert_eq!(
                app.launcher_expanded,
                Some(WorktreeId("w2".into())),
                "feat's band open"
            );

            key(&mut app, KeyCode::Char('a'), KeyModifiers::NONE);
            key(&mut app, KeyCode::Enter, KeyModifiers::NONE);
            assert_eq!(cards(&app), ["a9", "a1"]);
            assert_eq!(
                app.launcher_expanded, None,
                "feat's band is gone, and its accordion with it"
            );
            assert_eq!(
                app.selected_worktree().map(|w| w.id.0.clone()).as_deref(),
                Some("w1"),
                "on the band that slid up"
            );
            assert_eq!(selected(&app), root_card, "on the card it was left on");
            assert_eq!(
                pane(&app).map(|s| matches!(s, SessionRef::Agent(_))),
                Some(true),
                "and the pane reads it"
            );
            assert_eq!(app.focus, Focus::Sessions);
        });
    }

    /// `d`, answered, lands where `a` lands: INPUT PARITY across the two
    /// verbs that take a card off the grid — the band that slides up when
    /// the deleted card was its band's last, and the card before it when
    /// it was the last of a band with more.
    #[test]
    fn deleting_a_card_lands_where_archiving_it_lands() {
        with_default_config(|| {
            // feat's only card: its band goes, and the root band takes
            // the cursor.
            let mut app = three_sessions();
            draw(&mut app);
            assert_eq!(cards(&app), ["a9", "a2", "a1"], "newest first");
            super::select(&mut app, AgentId("a2".into()), &mut Vec::new());
            key(&mut app, KeyCode::Char('d'), KeyModifiers::NONE);
            key(&mut app, KeyCode::Char('y'), KeyModifiers::NONE);
            assert_eq!(cards(&app), ["a9", "a1"], "the delete went through");
            assert!(
                app.launcher_expanded.is_none(),
                "its band went with it: the bands"
            );
            assert_eq!(
                app.selected_worktree().map(|w| w.id.0.clone()).as_deref(),
                Some("w1"),
                "on the band that slid up"
            );

            // The last card of a band: nothing after it, so the one before.
            let mut app = three_sessions();
            draw(&mut app);
            super::select(&mut app, AgentId("a1".into()), &mut Vec::new());
            key(&mut app, KeyCode::Char('d'), KeyModifiers::NONE);
            key(&mut app, KeyCode::Char('y'), KeyModifiers::NONE);
            assert_eq!(cards(&app), ["a9", "a2"], "the delete went through");
            assert_eq!(selected(&app).as_deref(), Some("a9"), "the card before it");
        });
    }

    /// The PANELS reseat their own cursor on the same archive, onto the
    /// next row of the CHECKOUT in tree order — which is not the card
    /// that slid up into the slot in a band ordered by recency. The grid
    /// settles its own landing — the card after the one archived, as the
    /// band lists them — instead of letting that stand.
    #[test]
    fn archiving_a_card_ignores_the_panels_own_neighbor() {
        with_default_config(|| {
            let mut app = three_sessions();
            draw(&mut app);
            let (x, y) = row_cell(&app, 0);
            mouse(&mut app, MouseEventKind::Down(MouseButton::Left), x, y);
            assert_eq!(selected(&app).as_deref(), Some("a9"), "the first card");
            assert_eq!(
                app.selected_worktree().map(|w| w.id.0.clone()).as_deref(),
                Some("w1"),
                "whose checkout also holds the LAST card's session",
            );

            key(&mut app, KeyCode::Char('a'), KeyModifiers::NONE);
            key(&mut app, KeyCode::Enter, KeyModifiers::NONE);
            assert_eq!(cards(&app), ["a2", "a1"]);
            assert_eq!(
                selected(&app).as_deref(),
                Some("a1"),
                "the card after it in its band"
            );
            assert_eq!(
                app.selected_worktree().map(|w| w.id.0.clone()).as_deref(),
                Some("w1"),
                "still on the root band"
            );
        });
    }

    /// The DAEMON's own events land after the optimistic archive — the
    /// Ack, then the row as it now has it, its process killed and its
    /// stamp moved by the kill. The PANELS reseat on that upsert with no
    /// `keep_cursor` behind them, so the landing has to survive it.
    #[test]
    fn the_daemons_own_archive_upsert_keeps_the_landing() {
        with_default_config(|| {
            let mut app = three_sessions();
            draw(&mut app);
            super::select(&mut app, AgentId("a2".into()), &mut Vec::new());
            assert_eq!(selected(&app).as_deref(), Some("a2"), "the middle card");
            key(&mut app, KeyCode::Char('a'), KeyModifiers::NONE);
            let out = key(&mut app, KeyCode::Enter, KeyModifiers::NONE);
            assert_eq!(selected(&app).as_deref(), Some("a1"), "the card after it");

            // What the DAEMON answers with: the Ack for the request, then
            // the row as it now has it — archived, its process killed, its
            // stamp moved by the kill.
            let req_id = out
                .iter()
                .find_map(|r| match r {
                    ClientRequest::ArchiveAgent { req_id, .. } => Some(*req_id),
                    _ => None,
                })
                .expect("the archive was asked of the daemon");
            hse(
                &mut app,
                ServerEvent::Ack {
                    req_id,
                    created: None,
                },
            );
            let mut archived = app
                .tree
                .agents
                .iter()
                .find(|a| a.id.0 == "a2")
                .cloned()
                .unwrap();
            archived.archived = true;
            archived.alive = false;
            archived.status = AgentStatus::Finished;
            archived.status_changed_at = crate::app::now_ms();
            hse(
                &mut app,
                ServerEvent::EntityUpserted {
                    entity: Entity::Agent(archived),
                },
            );
            assert_eq!(
                selected(&app).as_deref(),
                Some("a1"),
                "the daemon's own events leave the landing alone"
            );
        });
    }

    /// The list moving in the same breath as the archive — a row arriving
    /// from the DAEMON, a turn starting and re-sorting the grid, the launch
    /// pin dropping — must not drag the landing along with it. The cursor
    /// takes the card that was after it BY NAME; counting the bare `index`
    /// landed it a card past that one, skipping the card that slid up.
    #[test]
    fn a_list_that_moves_under_the_archive_still_lands_on_the_card_after() {
        with_default_config(|| {
            let mut app = three_sessions();
            draw(&mut app);
            assert_eq!(cards(&app), ["a9", "a2", "a1"], "newest first");
            super::select(&mut app, AgentId("a2".into()), &mut Vec::new());

            // What the input event opens with: the cursor on the middle
            // card.
            let before = super::cursor_entry(&app).expect("a card under the cursor");
            // ...and in the same breath a newer session leads the grid, so
            // every index below it has moved by one.
            seed_running(&mut app, "az", "w1", "fresh-one");
            assert_eq!(cards(&app), ["az", "a9", "a2", "a1"], "the newcomer leads");

            let mut out = Vec::new();
            super::super::archive_agent_now(&mut app, AgentId("a2".into()), &mut out);
            super::keep_cursor(&mut app, before, &mut out);
            assert_eq!(cards(&app), ["az", "a9", "a1"]);
            assert_eq!(
                selected(&app).as_deref(),
                Some("a1"),
                "the card that was after it, not the one a shifted index points at"
            );
        });
    }

    /// The LAST card of a band — the oldest, the end of its row, the one
    /// an archiving sweep reaches last — has no card after it to take, so
    /// the cursor steps back onto the card before it.
    #[test]
    fn archiving_the_last_card_lands_on_the_card_before_it() {
        with_default_config(|| {
            let mut app = three_sessions();
            draw(&mut app);
            assert_eq!(cards(&app), ["a9", "a2", "a1"], "newest first");
            let (x, y) = row_cell(&app, 2);
            mouse(&mut app, MouseEventKind::Down(MouseButton::Left), x, y);
            assert_eq!(selected(&app).as_deref(), Some("a1"), "the last card");

            key(&mut app, KeyCode::Char('a'), KeyModifiers::NONE);
            key(&mut app, KeyCode::Enter, KeyModifiers::NONE);
            assert_eq!(cards(&app), ["a9", "a2"]);
            assert_eq!(selected(&app).as_deref(), Some("a9"), "the card before it");
        });
    }

    /// The ONLY card leaving — archived with `a` or deleted with `d`, each
    /// behind its confirm — leaves nothing for the PANE to read, so it
    /// folds away and the empty grid takes the body, the way a project
    /// opened with no sessions lands. INPUT PARITY: both verbs end in the
    /// same state.
    #[test]
    fn the_last_card_leaving_folds_the_pane_away() {
        with_default_config(|| {
            for keys in [&['a', 'y'][..], &['d', 'y'][..]] {
                let mut app = two_sessions();
                super::open_project(&mut app, &ProjectId("p2".into()), &mut Vec::new());
                draw(&mut app);
                assert_eq!(cards(&app), ["a3"], "web's one card");
                assert!(super::has_pane(&app), "{keys:?}: the pane reads it");

                for c in keys {
                    key(&mut app, KeyCode::Char(*c), KeyModifiers::NONE);
                }
                assert!(cards(&app).is_empty(), "{keys:?}: the card left");
                assert!(app.launcher_unaimed, "{keys:?}: nothing left to aim at");
                assert!(!super::has_pane(&app), "{keys:?}: and the pane folded");
                assert_eq!(app.flash.as_deref(), Some(super::NO_SESSIONS));
                let text = buffer_text(&draw(&mut app));
                assert!(
                    text.contains("press  p  to prompt"),
                    "{keys:?}: the empty grid says what starts one: {text}"
                );
            }
        });
    }

    /// INPUT PARITY: a click on a card lands where the keys that walk to
    /// it land — cursor, FOCUS, project, the grid's level — and a second
    /// click is Enter, twice over from the bands: into the worktree, then
    /// into the pane.
    #[test]
    fn a_click_on_a_card_is_the_keys_that_walk_to_it() {
        with_default_config(|| {
            let mut by_key = two_sessions();
            draw_at(&mut by_key, 130, 50);
            keys(&mut by_key, &[KeyCode::Esc, KeyCode::Char('j')]);

            let mut by_click = two_sessions();
            draw_at(&mut by_click, 130, 50);
            // Out to the bands, where every card is drawn to be clicked.
            key(&mut by_click, KeyCode::Esc, KeyModifiers::NONE);
            draw_at(&mut by_click, 130, 50);
            let (x, y) = row_cell(&by_click, 0);
            mouse(&mut by_click, MouseEventKind::Down(MouseButton::Left), x, y);

            assert_eq!(selected(&by_click), selected(&by_key));
            assert_eq!(pane(&by_click), pane(&by_key));
            assert_eq!(by_click.focus, by_key.focus);
            assert_eq!(by_click.sel_project, by_key.sel_project);
            assert_ne!(
                by_click.focus,
                Focus::Terminal,
                "a click aims, it does not attach"
            );

            keys(&mut by_key, &[KeyCode::Enter, KeyCode::Enter]);
            mouse(&mut by_click, MouseEventKind::Down(MouseButton::Left), x, y);
            assert_eq!(
                by_click.focus,
                Focus::Terminal,
                "the second click is Enter: into the pane"
            );
            assert_eq!(by_click.term_locked, by_key.term_locked);
            assert_eq!(
                by_click.collapsed, by_key.collapsed,
                "the pane beside the grid either way"
            );
            assert!(!by_click.collapsed, "not full-screen");
        });
    }

    /// INPUT PARITY: with the pane folded away (`^~`), a click on a card
    /// brings the pane back beside the grid, and the second click — like
    /// Enter, the key behind it — puts the keys in it. Neither
    /// full-screens the session. A third click, with the keys already in
    /// the pane, leaves it exactly where it is.
    #[test]
    fn a_click_on_a_card_unfolds_the_pane_rather_than_full_screening() {
        with_default_config(|| {
            let mut by_key = two_sessions();
            draw(&mut by_key);
            key(&mut by_key, KeyCode::Char('~'), KeyModifiers::NONE);
            assert!(by_key.launcher_pane_hidden, "^~ folded the pane away");
            to_feat(&mut by_key);
            assert!(
                by_key.launcher_pane_hidden,
                "walking the grid leaves the fold be"
            );
            key(&mut by_key, KeyCode::Enter, KeyModifiers::NONE);
            assert!(!by_key.launcher_pane_hidden, "Enter brought the pane back");
            assert!(
                !by_key.collapsed,
                "the pane beside the grid, not full-screen"
            );
            assert_eq!(by_key.focus, Focus::Terminal, "and the keys are in it");

            let mut by_click = two_sessions();
            draw(&mut by_click);
            key(&mut by_click, KeyCode::Char('~'), KeyModifiers::NONE);
            assert!(by_click.launcher_pane_hidden);
            key(&mut by_click, KeyCode::Esc, KeyModifiers::NONE);
            draw(&mut by_click);
            let (x, y) = row_cell(&by_click, 0);
            mouse(&mut by_click, MouseEventKind::Down(MouseButton::Left), x, y);
            assert!(
                !by_click.launcher_pane_hidden,
                "one click on a card always brings the folded pane back"
            );
            assert!(!by_click.launcher_unaimed, "on the card clicked");
            assert_eq!(
                by_click.focus,
                Focus::Sessions,
                "and the keys stay on the cards until the second click"
            );
            mouse(&mut by_click, MouseEventKind::Down(MouseButton::Left), x, y);

            assert!(
                !by_click.launcher_pane_hidden,
                "the second click brought the pane back"
            );
            assert_eq!(
                by_click.collapsed, by_key.collapsed,
                "the pane beside the grid either way"
            );
            assert!(!by_click.collapsed, "not full-screen");
            assert_eq!(by_click.focus, by_key.focus, "and the keys are in it");
            assert_eq!(pane(&by_click), pane(&by_key));
            assert_eq!(
                by_click.launcher_expanded.is_some(),
                by_key.launcher_expanded.is_some()
            );
            let text = buffer_text(&draw(&mut by_click));
            assert_eq!(
                tabs_drawn(&by_click),
                ["demo"],
                "the grid is still up over the pane: {text}"
            );

            // Again, with the keys already there: nothing moves.
            let (x, y) = row_cell(&by_click, 0);
            mouse(&mut by_click, MouseEventKind::Down(MouseButton::Left), x, y);
            mouse(&mut by_click, MouseEventKind::Down(MouseButton::Left), x, y);
            assert!(!by_click.launcher_pane_hidden, "the pane stayed");
            assert!(!by_click.collapsed, "and never took the whole screen");
            assert_eq!(by_click.focus, Focus::Terminal);
        });
    }

    /// INPUT PARITY: a right-click lands on a card exactly as a left click
    /// does — the pane folded away (`^~`) comes back open on that card,
    /// with the menu over it — while the keys walk the grid under the fold
    /// and leave it folded.
    #[test]
    fn a_right_click_on_a_card_unfolds_the_pane_as_a_left_click_does() {
        with_default_config(|| {
            let mut by_left = two_sessions();
            let mut by_right = two_sessions();
            for app in [&mut by_left, &mut by_right] {
                draw(app);
                key(app, KeyCode::Char('~'), KeyModifiers::NONE);
                keys(app, &[KeyCode::Esc, KeyCode::Char('j')]);
                assert!(
                    app.launcher_pane_hidden,
                    "a key walking the bands leaves the fold be"
                );
                draw(app);
            }
            let (x, y) = row_cell(&by_left, 0);
            mouse(&mut by_left, MouseEventKind::Down(MouseButton::Left), x, y);
            mouse(
                &mut by_right,
                MouseEventKind::Down(MouseButton::Right),
                x,
                y,
            );

            for app in [&by_left, &by_right] {
                assert!(!app.launcher_pane_hidden, "the click brought the pane back");
                assert!(!app.launcher_unaimed, "on the card clicked");
                assert_eq!(app.focus, Focus::Sessions, "the keys stay on the cards");
            }
            assert_eq!(
                by_right.selected_session().map(|a| a.id.clone()),
                by_left.selected_session().map(|a| a.id.clone()),
                "both buttons land on the same card"
            );
            assert!(
                matches!(by_right.overlay, Some(Overlay::Menu(_))),
                "and the right one opens its menu: {:?}",
                by_right.overlay
            );
        });
    }

    /// The wheel over the grid leaves the cursor where it is. A notch
    /// used to walk it a row of cards, which swaps the pane onto another
    /// session — a trackpad did that by accident while you were reading
    /// the card you were on. Only the keys walk the grid now: the wheel
    /// scrolls the panel under the cursor — here the band opened with
    /// Tab, whose cards outrun the narrow body — three rows a notch,
    /// held at the panel's ends, and the draw keeps it where the wheel
    /// left it — until a key walks the cursor, or asks for its card at
    /// the edge, which brings that card whole back on screen.
    #[test]
    fn the_wheel_over_the_grid_scrolls_the_cards_and_leaves_the_cursor_alone() {
        with_default_config(|| {
            let mut app = two_sessions();
            // Three cards in one column: a row and a half more than the
            // narrow body holds once the band is open, so there is
            // something to scroll.
            let home = app
                .tree
                .agents
                .iter()
                .find(|a| a.id.0 == "a1")
                .map(|a| a.worktree_id.0.clone())
                .expect("a1's checkout");
            seed_running(&mut app, "a5", &home, "five");
            seed_running(&mut app, "a6", &home, "six");
            draw_narrow(&mut app);
            key(&mut app, KeyCode::Tab, KeyModifiers::NONE);
            draw_narrow(&mut app);
            let bands = crate::launcher::bands(&app);
            let band = app
                .cursor_in_open_band(&bands)
                .expect("Tab opened the cursor's band");
            let panel = crate::launcher::panel_layout(
                app.body_area,
                &bands,
                app.launcher_expanded.as_ref(),
            );
            let pb = &panel.bands[band];
            assert!(panel.overflows(), "the cards outrun the body");
            let whole = |app: &App, card: usize| {
                crate::launcher::place(panel.window(), app.launcher_scroll, pb.cell(card).unwrap())
                    .is_some_and(|p| p.whole())
            };
            let (x, y) = card_cell(&app, &SessionRef::Agent(AgentId("a1".into())));
            let before = selected(&app);
            assert_eq!(before.as_deref(), Some("a1"), "the cursor starts here");
            // The first frame scrolled to the cursor's card, wherever the
            // order put it: it is whole on screen.
            let a1 = bands[band]
                .position(&SessionRef::Agent(AgentId("a1".into())))
                .expect("a1's card");
            assert!(whole(&app, a1));

            for _ in 0..20 {
                mouse(&mut app, MouseEventKind::ScrollUp, x, y);
            }
            assert_eq!(selected(&app), before, "the wheel up moves nothing");
            assert_eq!(app.launcher_scroll, 0, "the top is the top");
            mouse(&mut app, MouseEventKind::ScrollDown, x, y);
            assert_eq!(selected(&app), before, "and neither does the wheel down");
            assert_eq!(
                app.launcher_scroll, 3,
                "it scrolls the cards, three rows a notch"
            );
            draw_narrow(&mut app);
            assert_eq!(app.launcher_scroll, 3, "and the draw keeps them there");
            assert!(app.launcher_scroll_held);

            for _ in 0..20 {
                mouse(&mut app, MouseEventKind::ScrollDown, x, y);
            }
            assert_eq!(app.launcher_scroll, panel.max_scroll(), "held at the end");
            draw_narrow(&mut app);
            assert!(
                panel.hidden(app.launcher_scroll).above > 0,
                "the top of the grid has scrolled off"
            );

            // A key walks the cursor a row — `k` up, or `j` down from the
            // top row — and the frame brings the card it lands on whole
            // back on screen: the scroll is the keys' again.
            let (row, _) = pb.content.as_ref().unwrap().row_of(a1).expect("a1's row");
            let step = if row == 0 { 'j' } else { 'k' };
            key(&mut app, KeyCode::Char(step), KeyModifiers::NONE);
            draw_narrow(&mut app);
            assert!(!app.launcher_scroll_held, "the keys took the scroll back");
            let at = crate::launcher::card_cursor(&app, &bands[band]).expect("on a card");
            assert_ne!(at, a1, "{step} walked the cursor");
            assert!(
                whole(&app, at),
                "the card under the cursor is whole on screen"
            );
        });
    }

    /// `Tab` and `^O` layer their lists over the box the same way `^P`
    /// does: the harness picker, the model picker and every submenu
    /// under them float over the box, which stays on screen — its title,
    /// its details row and the task typed into it — under the list.
    /// Picking what runs the task should never take the task away. A
    /// list wide enough covers the middle of the task, which is what
    /// being on top of it means; the head of it still reads.
    #[test]
    fn the_harness_and_model_pickers_are_drawn_over_the_box() {
        with_default_config(|| {
            let mut app = two_sessions();
            draw(&mut app);
            key(&mut app, KeyCode::Char('p'), KeyModifiers::NONE);
            type_text(&mut app, "fix the nav");
            let box_behind = |app: &mut App, what: &str| {
                let text = buffer_text(&draw(app));
                assert!(text.contains("New session"), "{what}: the title: {text}");
                assert!(
                    text.contains("project demo ^P"),
                    "{what}: the details row: {text}"
                );
                assert!(text.contains("fix the"), "{what}: the task: {text}");
                text
            };

            // `Tab`: the harness list, over the box.
            key(&mut app, KeyCode::Tab, KeyModifiers::NONE);
            let text = box_behind(&mut app, "the harness picker");
            assert!(text.contains("Quick prompt agent"), "{text}");

            // A submenu under it keeps the box too — the chain never
            // drops the layer it was opened from.
            key(&mut app, KeyCode::Right, KeyModifiers::NONE);
            let text = box_behind(&mut app, "a submenu of it");
            assert!(text.contains("model"), "{text}");

            // `^O`: the model list of the box's harness, over the box.
            key(&mut app, KeyCode::Esc, KeyModifiers::NONE);
            key(&mut app, KeyCode::Esc, KeyModifiers::NONE);
            key(&mut app, KeyCode::Char('o'), KeyModifiers::CONTROL);
            let text = box_behind(&mut app, "the model picker");
            assert!(text.contains("Claude model"), "{text}");

            // And Esc all the way out still hands the box back whole.
            key(&mut app, KeyCode::Esc, KeyModifiers::NONE);
            let text = buffer_text(&draw(&mut app));
            assert!(!text.contains("Claude model"), "{text}");
            assert!(text.contains("fix the nav"), "{text}");
        });
    }

    /// `^P` layers the PROJECT PICKER over the box instead of taking the
    /// box away: the box's frame, its title, the details row and the task
    /// already typed into it are all still on screen around the list, so
    /// aiming the launch never costs you sight of what you are launching.
    #[test]
    fn the_project_picker_is_drawn_over_the_box() {
        with_default_config(|| {
            let mut app = two_sessions();
            draw(&mut app);
            key(&mut app, KeyCode::Char('p'), KeyModifiers::NONE);
            type_text(&mut app, "fix the nav");
            key(&mut app, KeyCode::Char('p'), KeyModifiers::CONTROL);
            let text = buffer_text(&draw(&mut app));
            assert!(text.contains("type a project name"), "the picker: {text}");
            assert!(text.contains("New session"), "the box's title: {text}");
            assert!(text.contains("project demo ^P"), "its details row: {text}");
            assert!(text.contains("fix the nav"), "and the task in it: {text}");

            // The box goes when the picker hands it back, not before: one
            // box on screen either way, never two frames of it.
            key(&mut app, KeyCode::Enter, KeyModifiers::NONE);
            let text = buffer_text(&draw(&mut app));
            assert!(!text.contains("type a project name"), "{text}");
            assert!(text.contains("fix the nav"), "{text}");
        });
    }

    /// `^P` in the box: every project, narrowed as you type; Enter puts
    /// the box back on the pick with what was typed kept.
    #[test]
    fn ctrl_p_picks_the_project_by_typing_and_keeps_the_task() {
        with_config_json(r#"{"quick_prompt_new_worktree": true}"#, || {
            let mut app = two_sessions();
            draw(&mut app);
            key(&mut app, KeyCode::Char('p'), KeyModifiers::NONE);
            type_text(&mut app, "fix it");
            key(&mut app, KeyCode::Char('p'), KeyModifiers::CONTROL);
            let Some(Overlay::ProjectPicker(picker)) = &app.overlay else {
                panic!("expected the project picker, got {:?}", app.overlay);
            };
            assert_eq!(picker.matches.len(), 2);

            type_text(&mut app, "we");
            key(&mut app, KeyCode::Enter, KeyModifiers::NONE);
            let (launch, text) = launch(&app);
            assert_eq!(text, "fix it", "the task survives the trip");
            assert!(matches!(
                &launch.target,
                QuickTarget::NewWorktree { project, .. } if project.0 == "p2"
            ));

            // Esc from the picker hands the box back as it was.
            key(&mut app, KeyCode::Char('p'), KeyModifiers::CONTROL);
            key(&mut app, KeyCode::Esc, KeyModifiers::NONE);
            let (after, text) = self::launch(&app);
            assert_eq!(text, "fix it");
            assert_eq!(after.target, launch.target);
        });
    }

    /// A BACKGROUND LAUNCH: a box re-aimed at another project with `^P`
    /// starts its session over there and leaves the screen here. The
    /// SESSIONS level, the card under the cursor and the pane are all
    /// where they were — the whole point of aiming the box elsewhere is
    /// to keep working on what is in front of you — and the footer names
    /// the project the prompt went to, since nothing else moved.
    #[test]
    fn a_launch_into_another_project_leaves_the_screen_where_it_is() {
        with_config_json(r#"{"quick_prompt_new_worktree": true}"#, || {
            let mut app = two_sessions();
            draw(&mut app);
            assert_eq!(
                app.selected_project().map(|p| p.name.as_str()),
                Some("demo")
            );
            let (card, shown) = (selected(&app), pane(&app));

            key(&mut app, KeyCode::Char('p'), KeyModifiers::NONE);
            type_text(&mut app, "tidy the nav");
            key(&mut app, KeyCode::Char('p'), KeyModifiers::CONTROL);
            type_text(&mut app, "we");
            key(&mut app, KeyCode::Enter, KeyModifiers::NONE);
            let out = key(&mut app, KeyCode::Enter, KeyModifiers::NONE);

            assert!(app.overlay.is_none(), "the box launched");
            assert!(
                matches!(
                    out.as_slice(),
                    [ClientRequest::CreateWorktree { project, .. }] if project.0 == "p2"
                ),
                "the checkout is cut in the project the box was aimed at: {out:?}"
            );
            assert_eq!(
                app.selected_project().map(|p| p.name.as_str()),
                Some("demo"),
                "the level stayed on the project being worked in"
            );
            assert_eq!(selected(&app), card, "and so did the cursor");
            assert_eq!(pane(&app), shown, "and the pane");
            assert_eq!(
                app.flash.as_deref(),
                Some("started a session in web"),
                "the footer is the only sign of it"
            );

            // The session went up all the same — in web's list, not this one.
            let staged = app
                .tree
                .agents
                .iter()
                .find(|a| !["a1", "a2", "a3"].contains(&a.id.0.as_str()))
                .expect("a stand-in session for the launch");
            let project = app
                .tree
                .worktrees
                .iter()
                .find(|w| w.id == staged.worktree_id)
                .map(|w| w.project_id.clone());
            assert_eq!(project, Some(ProjectId("p2".into())));
            let rows = crate::launcher::rows(&app);
            assert!(
                !rows.iter().any(|r| r.agent.id == staged.id),
                "no card for it in demo's grid: {:?}",
                rows.iter()
                    .map(|r| r.agent.name.clone())
                    .collect::<Vec<_>>()
            );
        });
    }

    /// The same with the new-worktree SETTING off, where there is no checkout to cut and the
    /// create goes straight out: it is born LEFT BEHIND, so the Ack that
    /// comes back seconds later cannot pull the screen over to it either.
    #[test]
    fn a_background_launch_into_an_existing_checkout_is_born_left_behind() {
        with_default_config(|| {
            let mut app = two_sessions();
            draw(&mut app);
            let (card, shown) = (selected(&app), pane(&app));

            key(&mut app, KeyCode::Char('p'), KeyModifiers::NONE);
            type_text(&mut app, "tidy the nav");
            key(&mut app, KeyCode::Char('p'), KeyModifiers::CONTROL);
            type_text(&mut app, "we");
            key(&mut app, KeyCode::Enter, KeyModifiers::NONE);
            // Into web's own checkout: the new-worktree SETTING is off.
            let out = key(&mut app, KeyCode::Enter, KeyModifiers::NONE);

            let req_id = match out.as_slice() {
                [ClientRequest::CreateAgent {
                    req_id, worktree, ..
                }] if worktree.0 == "w2root" => *req_id,
                other => panic!("one CreateAgent into web's checkout: {other:?}"),
            };
            assert!(
                app.left_behind.contains(&req_id),
                "the Ack moves nothing back"
            );
            assert_eq!(
                app.selected_project().map(|p| p.name.as_str()),
                Some("demo")
            );
            assert_eq!(selected(&app), card);
            assert_eq!(pane(&app), shown);

            // And the Ack keeps its word.
            hse(
                &mut app,
                ServerEvent::EntityUpserted {
                    entity: Entity::Agent(Agent {
                        id: AgentId("a9".into()),
                        worktree_id: WorktreeId("w2root".into()),
                        name: "agent-1".into(),
                        status: AgentStatus::Fresh,
                        archived: false,
                        archived_at: 0,
                        unseen: false,
                        kind: AgentKind::Claude,
                        custom_harness: None,
                        model: None,
                        effort: None,
                        session_id: None,
                        cloud_session_id: None,
                        sort_order: 0,
                        status_changed_at: crate::app::now_ms(),
                        alive: true,
                        issue_url: None,
                        recent_prompts: Vec::new(),
                    }),
                },
            );
            let mut sink = Vec::new();
            super::super::handle_server_event(
                &mut app,
                ServerEvent::Ack {
                    req_id,
                    created: Some(nebula_core::EntityId::Agent(AgentId("a9".into()))),
                },
                &mut sink,
            );
            assert_eq!(
                app.selected_project().map(|p| p.name.as_str()),
                Some("demo"),
                "the level stayed put through the Ack"
            );
            assert_eq!(selected(&app), card);
            assert_eq!(pane(&app), shown);
        });
    }

    /// Picking another project in the box aims the box there without
    /// switching to it: the grid behind the box goes on showing the work
    /// in front of the user, and no tab moves.
    #[test]
    fn picking_another_project_in_the_box_leaves_the_grid_where_it_is() {
        with_default_config(|| {
            let mut app = two_sessions();
            app.tree.projects.push(Project {
                id: ProjectId("p3".into()),
                name: "away".into(),
                repo_path: "/tmp/away".into(),
                sort_order: 2,
            });
            draw(&mut app);
            let tabs = app.launcher_tabs.clone();

            key(&mut app, KeyCode::Char('p'), KeyModifiers::NONE);
            key(&mut app, KeyCode::Char('p'), KeyModifiers::CONTROL);
            type_text(&mut app, "away");
            let out = key(&mut app, KeyCode::Enter, KeyModifiers::NONE);

            let (launch, _) = launch(&app);
            assert_eq!(
                crate::launcher::project_of(&app, &launch.target),
                Some(ProjectId("p3".into())),
                "the box is aimed at it"
            );
            let _ = out;
            assert_eq!(app.launcher_tabs, tabs, "no tab was added");
            assert_eq!(
                app.selected_project().map(|p| p.name.as_str()),
                Some("demo")
            );
        });
    }

    /// `^O` in the box: the harness's model list straight away; a pick
    /// comes back to the box with the text kept and the model set.
    #[test]
    fn ctrl_o_picks_the_model_and_comes_back_to_the_box() {
        with_default_config(|| {
            let mut app = two_sessions();
            draw(&mut app);
            key(&mut app, KeyCode::Char('p'), KeyModifiers::NONE);
            type_text(&mut app, "hi");
            key(&mut app, KeyCode::Char('o'), KeyModifiers::CONTROL);
            let Some(Overlay::Menu(menu)) = &app.overlay else {
                panic!("expected the model list, got {:?}", app.overlay);
            };
            assert_eq!(menu.title.as_deref(), Some("Claude model"));
            key(&mut app, KeyCode::Down, KeyModifiers::NONE);
            let Some(Overlay::Menu(menu)) = &app.overlay else {
                unreachable!()
            };
            let crate::app::MenuAction::NewAgentOfKind {
                model: Some(want), ..
            } = &menu.items[menu.hover].action
            else {
                panic!("a model row");
            };
            let want = want.clone();
            key(&mut app, KeyCode::Enter, KeyModifiers::NONE);
            let (launch, text) = launch(&app);
            assert_eq!(text, "hi");
            let expected = Some(want).filter(|m| m != "default");
            assert_eq!(launch.model, expected);
        });
    }

    /// `^N` flips the box between the project's own checkout — the
    /// project the box is aimed at — and a fresh worktree, and the next
    /// box starts from the SETTING again (off by default).
    #[test]
    fn ctrl_n_flips_one_box_and_the_next_starts_from_the_setting() {
        with_config_json(r#"{"quick_prompt_new_worktree": true}"#, || {
            let mut app = two_sessions();
            draw(&mut app);
            key(&mut app, KeyCode::Char('p'), KeyModifiers::NONE);
            let (launch, _) = launch(&app);
            assert!(launch.is_new_worktree(), "the setting is on");
            key(&mut app, KeyCode::Char('n'), KeyModifiers::CONTROL);
            let (launch, _) = self::launch(&app);
            let QuickTarget::Worktree(worktree) = &launch.target else {
                panic!("expected an existing checkout, got {:?}", launch.target);
            };
            let project = crate::launcher::project_of(&app, &launch.target);
            assert_eq!(project, app.selected_project().map(|p| p.id.clone()));
            assert!(app.tree.worktrees.iter().any(|w| &w.id == worktree));

            key(&mut app, KeyCode::Esc, KeyModifiers::NONE);
            key(&mut app, KeyCode::Char('p'), KeyModifiers::NONE);
            let (launch, _) = self::launch(&app);
            assert!(launch.is_new_worktree(), "the next box follows the setting");
        });
    }

    /// `⇧A` swaps the grid for the project's ARCHIVED sessions and back:
    /// the live cards go, the archived ones arrive, the header counts
    /// them under their own word, and `u` on one unarchives it where it
    /// stands. An archived card has no session to step into, so Enter
    /// says to unarchive it first.
    #[test]
    fn shift_a_swaps_the_grid_for_the_archived_sessions_and_back() {
        with_default_config(|| {
            let mut app = two_sessions();
            // `polish-nav` archived, `agent-1` still live.
            for agent in &mut app.tree.agents {
                if agent.id == AgentId("a2".into()) {
                    agent.archived = true;
                }
            }
            let text = buffer_text(&draw(&mut app));
            assert!(text.contains("agent-1"), "{text}");
            assert!(!text.contains("polish-nav"), "the live grid: {text}");

            key(&mut app, KeyCode::Char('A'), KeyModifiers::SHIFT);
            assert!(app.show_archived, "the archived view is on");
            let text = buffer_text(&draw(&mut app));
            assert!(text.contains("polish-nav"), "the archived card: {text}");
            assert!(!text.contains("agent-1"), "and only that one: {text}");
            assert!(
                text.contains("1 archived session"),
                "the header counts them under their own word: {text}"
            );
            assert_eq!(
                app.selected_session().map(|a| a.id),
                Some(AgentId("a2".into())),
                "the cursor lands on the first card of the list that arrived"
            );

            // Enter has no session to step into there.
            app.flash = None;
            key(&mut app, KeyCode::Enter, KeyModifiers::NONE);
            assert_eq!(
                app.flash.as_deref(),
                Some(crate::event_loop::AGENT_ARCHIVED),
                "inside={} unaimed={} focus={:?} hidden={} collapsed={} overlay={}",
                app.launcher_expanded.is_some(),
                app.launcher_unaimed,
                app.focus,
                app.launcher_pane_hidden,
                app.collapsed,
                app.overlay.is_some()
            );

            // `u` unarchives the card under the cursor where it stands.
            let out = key(&mut app, KeyCode::Char('u'), KeyModifiers::NONE);
            assert!(
                out.iter().any(|r| matches!(
                    r,
                    ClientRequest::UnarchiveAgent { id, .. } if *id == AgentId("a2".into())
                )),
                "{out:?}"
            );

            // And `⇧A` again is the live grid — inside the checkout of the
            // card just unarchived, the bands a step out.
            key(&mut app, KeyCode::Char('A'), KeyModifiers::SHIFT);
            assert!(!app.show_archived);
            let text = buffer_text(&draw(&mut app));
            assert!(!text.contains("archived session"), "{text}");
            assert!(text.contains("polish-nav"), "{text}");
            key(&mut app, KeyCode::Esc, KeyModifiers::NONE);
            let text = buffer_text(&draw_at(&mut app, 130, 50));
            assert!(text.contains("agent-1"), "{text}");
        });
    }

    /// An empty ARCHIVED VIEW says so and names the way back, rather than
    /// showing the first-run hero, which would be advice about the other
    /// list.
    #[test]
    fn an_empty_archived_view_names_the_way_back() {
        with_default_config(|| {
            let mut app = two_sessions();
            key(&mut app, KeyCode::Char('A'), KeyModifiers::SHIFT);
            let text = buffer_text(&draw(&mut app));
            assert!(text.contains("0 archived sessions"), "{text}");
            assert!(text.contains("nothing archived"), "{text}");
            assert!(
                !text.contains("type a task"),
                "not the first-run hero: {text}"
            );
        });
    }

    /// A PANE that loses the KEYBOARD says so. The input lock decides
    /// where a keystroke lands, so a drop nobody asked for — here a
    /// redraw that finds the pane folded out from under a locked cursor —
    /// leaves the very next key meaning something else entirely: `x`
    /// closes the project's tab, `⇧M` opens the memory modal.
    /// Handing the keys over in silence is the bug the flash closes.
    #[test]
    fn a_pane_losing_the_keyboard_says_so() {
        with_default_config(|| {
            let mut app = two_sessions();
            app.term = Some(crate::app::AttachedTerm::new(
                SessionRef::Agent(AgentId("a2".into())),
                40,
                10,
            ));
            app.focus = Focus::Terminal;
            app.term_locked = true;

            // Nothing was pressed at the pane: it simply is not drawn any
            // more, and the draw settles focus back onto the cards.
            app.launcher_pane_hidden = true;
            app.flash = None;
            draw(&mut app);

            assert!(!app.term_locked, "the keys are the grid's now");
            assert_eq!(
                app.flash.as_deref(),
                Some(crate::app::TERMINAL_RELEASED),
                "and the handover said so"
            );
            assert!(
                buffer_text(&draw(&mut app)).contains("the keys are the grid's again"),
                "and it is on screen, not just in the field"
            );

            // Said once, on the handover — not repainted every frame.
            app.flash = None;
            draw(&mut app);
            assert_eq!(app.flash, None, "once, not every frame");
        });
    }

    /// A pane that was only being PREVIEWED says nothing when focus
    /// settles off it: the keys were the grid's all along, so nothing
    /// changed hands and there is nothing to report.
    #[test]
    fn a_previewed_pane_losing_focus_says_nothing() {
        with_default_config(|| {
            let mut app = two_sessions();
            app.term = Some(crate::app::AttachedTerm::new(
                SessionRef::Agent(AgentId("a2".into())),
                40,
                10,
            ));
            app.focus = Focus::Terminal;
            app.term_locked = false;

            app.launcher_pane_hidden = true;
            app.flash = None;
            draw(&mut app);

            assert_eq!(app.focus, Focus::Sessions, "focus still settles");
            assert_eq!(app.flash, None, "nothing was being typed at");
        });
    }

    /// PR & ISSUE COUNTS: the header counts the selected project's open
    /// pull requests and issues beside the session count, so what is
    /// waiting on the repo reads off the header instead of `v` and `i`.
    #[test]
    fn the_header_counts_the_projects_open_prs_and_issues() {
        with_default_config(|| {
            let mut app = two_sessions();
            seed_open_prs(&mut app, &[(7, "Attach links"), (9, "Fix the nav")]);
            seed_issues(&mut app, &[(15, "Crash on boot")]);
            app.launcher_expanded = None;

            let text = buffer_text(&draw(&mut app));
            assert!(text.contains("2 sessions"), "{text}");
            assert!(text.contains("2 prs"), "{text}");
            assert!(text.contains("1 issue"), "the singular: {text}");
        });
    }

    /// INPUT PARITY: the header's PR & ISSUE COUNTS are buttons. A click
    /// on `2 prs` opens the PULL REQUESTS MODAL and one on `1 issue` the
    /// ISSUES MODAL, each for the project in front of you — the modals `v`
    /// and `i` open, through the same function. Each target sits on its
    /// own word and not on the `·` between them, and the one under the
    /// pointer is underlined, as the header's tabs are.
    #[test]
    fn clicking_a_header_count_opens_that_list() {
        with_default_config(|| {
            let mut app = two_sessions();
            seed_open_prs(&mut app, &[(7, "Attach links"), (9, "Fix the nav")]);
            seed_issues(&mut app, &[(15, "Crash on boot")]);
            let project = app.selected_project().expect("a project").id.clone();
            let terminal = draw(&mut app);
            let line: Vec<char> = head_line(&terminal).chars().collect();
            let word = |app: &App, hit: HitTarget| -> String {
                let rect = app.hit_rect(&hit).expect("the count was drawn");
                assert_eq!(rect.y, 1, "on the header row");
                line[rect.x as usize..(rect.x + rect.width) as usize]
                    .iter()
                    .collect()
            };
            assert_eq!(word(&app, HitTarget::LauncherPullRequests), "2 prs");
            assert_eq!(word(&app, HitTarget::LauncherIssues), "1 issue");

            // The pointer resting on a count underlines that word alone.
            let (x, y) = crumb_cell(&app, HitTarget::LauncherIssues);
            mouse(&mut app, MouseEventKind::Moved, x, y);
            assert_eq!(underlined_head(&draw(&mut app)), "1 issue");

            let (x, y) = crumb_cell(&app, HitTarget::LauncherPullRequests);
            mouse(&mut app, MouseEventKind::Down(MouseButton::Left), x, y);
            let Some(Overlay::PullRequests(view)) = &app.overlay else {
                panic!("`2 prs` opens the pull requests: {:?}", app.overlay);
            };
            assert_eq!(view.project, project);
            let clicked = app.overlay.take();
            key(&mut app, KeyCode::Char('v'), KeyModifiers::NONE);
            assert_eq!(
                format!("{:?}", app.overlay),
                format!("{clicked:?}"),
                "the click is `v`"
            );

            app.overlay = None;
            draw(&mut app);
            let (x, y) = crumb_cell(&app, HitTarget::LauncherIssues);
            mouse(&mut app, MouseEventKind::Down(MouseButton::Left), x, y);
            let Some(Overlay::Issues(view)) = &app.overlay else {
                panic!("`1 issue` opens the issues: {:?}", app.overlay);
            };
            assert_eq!(view.project, project);
            let clicked = app.overlay.take();
            key(&mut app, KeyCode::Char('i'), KeyModifiers::NONE);
            assert_eq!(
                format!("{:?}", app.overlay),
                format!("{clicked:?}"),
                "the click is `i`"
            );
        });
    }

    /// The grid names each session's place, the harness it runs on and
    /// its pull request, with the count and the needs-you tally in the
    /// header.
    #[test]
    fn the_grid_names_each_sessions_place_and_pull_request() {
        with_default_config(|| {
            let mut app = two_sessions();
            app.pull_requests.insert(
                WorktreeId("w2".into()),
                Some(crate::pull_request::PullRequest {
                    number: 42,
                    url: "https://github.com/o/web/pull/42".into(),
                    title: "Polish the nav".into(),
                    state: crate::pull_request::STATE_OPEN.into(),
                    is_draft: false,
                    health: Default::default(),
                    activity: Vec::new(),
                }),
            );
            draw(&mut app);
            key(&mut app, KeyCode::Esc, KeyModifiers::NONE);
            let text = buffer_text(&draw_at(&mut app, 130, 50));
            assert!(!text.contains("PROJECTS"), "{text}");
            assert!(!text.contains("WORKTREES"), "{text}");
            assert_eq!(tabs_drawn(&app), ["demo"], "the header's tabs: {text}");
            assert!(text.contains("2 sessions"), "{text}");
            assert!(text.contains("polish-nav"), "{text}");
            // Each band names the checkout its cards run in, in the SCOPE
            // COLOR's own glyph: `↳` for a worktree of its own, `⌂` for
            // the project's root branch — and not the project, which the
            // whole grid is scoped to and the header already names. The
            // cards under it say what runs there.
            assert!(text.contains("↳ feat"), "{text}");
            assert!(text.contains("claude"), "{text}");
            assert!(
                !text.contains("demo ▸ ↳") && !text.contains("demo ▸ ⌂"),
                "the project is the grid's scope, not a line on every card: {text}"
            );
            assert!(
                !text.contains("tidy-css"),
                "the project beside it is a level up, not in this grid: {text}"
            );
            assert!(text.contains("#42 Polish the nav"), "{text}");
            assert!(text.contains("⌂ main"), "{text}");
        });
    }

    const PR_42: &str = "https://github.com/o/demo/pull/42";

    /// An open pull request, `#<number> Polish the nav`, on `o/demo`.
    fn pull_request(number: u64) -> crate::pull_request::PullRequest {
        crate::pull_request::PullRequest {
            number,
            url: format!("https://github.com/o/demo/pull/{number}"),
            title: "Polish the nav".into(),
            state: crate::pull_request::STATE_OPEN.into(),
            is_draft: false,
            health: Default::default(),
            activity: Vec::new(),
        }
    }

    /// [`two_sessions`], drawn at the band level, with the checkout of the
    /// card under the cursor on pull request #42.
    fn card_on_a_pull_request() -> App {
        let mut app = two_sessions();
        draw(&mut app);
        let worktree = app
            .selected_session()
            .map(|a| a.worktree_id.clone())
            .expect("a card under the cursor");
        app.pull_requests.insert(worktree, Some(pull_request(42)));
        // The pull request is on the checkout's rule, over its card.
        draw(&mut app);
        app
    }

    /// The cell of the one pull request the grid drew on a band's rule,
    /// and the checkout it is on.
    fn pull_request_line(app: &App) -> (ratatui::layout::Rect, WorktreeId) {
        app.hits
            .iter()
            .find_map(|(rect, hit)| match hit {
                HitTarget::LauncherBandPr(wid) => Some((*rect, wid.clone())),
                _ => None,
            })
            .expect("a band's pull request was drawn")
    }

    /// A left click — press and release — at a cell, through the loop's
    /// own entry point, with what it sent.
    fn click_at(app: &mut App, column: u16, row: u16) -> Vec<ClientRequest> {
        let mut out = Vec::new();
        for kind in [
            MouseEventKind::Down(MouseButton::Left),
            MouseEventKind::Up(MouseButton::Left),
        ] {
            let event = MouseEvent {
                kind,
                column,
                row,
                modifiers: KeyModifiers::NONE,
            };
            handle_terminal_event(app, crossterm::event::Event::Mouse(event), &mut out);
        }
        out
    }

    /// Where **Open pull request** sits on the card's right-click menu, if
    /// it is there at all.
    fn pr_menu_row(app: &mut App) -> Option<usize> {
        right_click_card(app);
        match &app.overlay {
            Some(Overlay::Menu(menu)) => menu
                .items
                .iter()
                .position(|i| i.label == "Open pull request"),
            other => panic!("expected the card's menu, got {other:?}"),
        }
    }

    /// `⇧V` on a card opens the pull request its `#42 title` line names,
    /// marking it read on the way out as every other door to a PR does —
    /// and, INPUT PARITY, the card menu's **Open pull request** ends in
    /// the same state.
    #[test]
    fn shift_v_opens_the_cards_pull_request_as_its_menu_row_does() {
        with_default_config(|| {
            let mut by_key = card_on_a_pull_request();
            let sent = key(&mut by_key, KeyCode::Char('V'), KeyModifiers::SHIFT);
            assert!(by_key.overlay.is_none(), "{:?}", by_key.overlay);
            assert_eq!(
                by_key.flash.as_deref(),
                Some("opened github.com/o/demo/pull/42")
            );
            assert!(
                sent.iter()
                    .any(|r| matches!(r, ClientRequest::MarkPrSeen { url, .. } if url == PR_42)),
                "the pull request is marked read: {sent:?}"
            );

            let mut by_menu = card_on_a_pull_request();
            let at = pr_menu_row(&mut by_menu).expect("the row is on the card's menu");
            for _ in 0..at {
                key(&mut by_menu, KeyCode::Down, KeyModifiers::NONE);
            }
            let sent_by_menu = key(&mut by_menu, KeyCode::Enter, KeyModifiers::NONE);
            assert!(by_menu.overlay.is_none(), "{:?}", by_menu.overlay);
            assert_eq!(by_menu.flash, by_key.flash);
            assert_eq!(format!("{sent_by_menu:?}"), format!("{sent:?}"));
            assert_eq!(by_menu.pr_seen, by_key.pr_seen);
        });
    }

    /// `⇧R` on the grid reloads from GitHub: the pull requests on the
    /// loop's next turn, past every timer, and the project's issues with
    /// them — asked of a checkout that is not on disk here, so the ask is
    /// the miss it records without a process.
    #[test]
    fn shift_r_on_the_grid_reloads_pull_requests_and_issues() {
        with_default_config(|| {
            let mut app = two_sessions();
            draw(&mut app);
            let pid = app.selected_project().expect("a project").id.clone();
            for p in app.tree.projects.iter_mut() {
                p.repo_path = "/nonexistent/nebula-shift-r".into();
            }
            assert!(!app.issues_failed.contains(&pid));
            let sent = key(&mut app, KeyCode::Char('R'), KeyModifiers::SHIFT);
            assert!(sent.is_empty(), "gh runs client-side: {sent:?}");
            assert!(app.pr_refresh_requested, "the pull requests are re-asked");
            assert!(app.issues_failed.contains(&pid), "and the issues with them");
            assert_eq!(app.flash.as_deref(), Some(crate::event_loop::RELOAD_FLASH));
        });
    }

    /// A card whose checkout has no pull request yet says so, naming the
    /// branch, and its menu carries no row for one.
    #[test]
    fn shift_v_on_a_card_with_no_pull_request_says_so() {
        with_default_config(|| {
            let mut app = two_sessions();
            draw(&mut app);
            let branch = app
                .selected_session()
                .and_then(|a| crate::launcher::row(&app, &a.id))
                .map(|row| row.branch)
                .expect("a card under the cursor");
            let sent = key(&mut app, KeyCode::Char('V'), KeyModifiers::SHIFT);
            assert!(sent.is_empty(), "{sent:?}");
            assert_eq!(
                app.flash,
                Some(format!(
                    "no pull request on {branch} yet — ⇧R reloads from GitHub"
                ))
            );
            assert_eq!(pr_menu_row(&mut app), None);
        });
    }

    /// With the aim let go of (Esc), no card wears the cursor, so `⇧V`
    /// has none to read a pull request off — even though the session the
    /// cursor last rested on has one.
    #[test]
    fn shift_v_with_no_card_selected_opens_nothing() {
        with_default_config(|| {
            let mut app = card_on_a_pull_request();
            key(&mut app, KeyCode::Esc, KeyModifiers::NONE);
            assert!(app.launcher_unaimed);
            let sent = key(&mut app, KeyCode::Char('V'), KeyModifiers::SHIFT);
            assert!(sent.is_empty(), "{sent:?}");
            assert_eq!(app.flash.as_deref(), Some(super::NO_CARD_FOR_PR));
        });
    }

    /// `y` on a card comments on the pull request `⇧V` opens — its
    /// checkout's, the `#42 title` on its band's rule — with no PR row
    /// to stand on first.
    #[test]
    fn y_on_a_card_comments_on_its_pull_request() {
        with_default_config(|| {
            let mut app = card_on_a_pull_request();
            let sent = key(&mut app, KeyCode::Char('y'), KeyModifiers::NONE);
            assert!(sent.is_empty(), "{sent:?}");
            match &app.overlay {
                Some(Overlay::Prompt(prompt)) => match &prompt.kind {
                    crate::app::PromptKind::PrComment {
                        number, url, label, ..
                    } => {
                        assert_eq!(*number, 42);
                        assert_eq!(url, PR_42);
                        assert_eq!(label, "#42 Polish the nav");
                    }
                    other => panic!("expected the comment box, got {other:?}"),
                },
                other => panic!("expected the comment box, got {other:?}"),
            }
        });
    }

    /// `y` refuses as `⇧V` does: with no card selected, and on a card
    /// whose checkout has no pull request yet.
    #[test]
    fn y_without_a_cards_pull_request_says_why() {
        with_default_config(|| {
            let mut app = card_on_a_pull_request();
            key(&mut app, KeyCode::Esc, KeyModifiers::NONE);
            key(&mut app, KeyCode::Char('y'), KeyModifiers::NONE);
            assert!(app.overlay.is_none(), "{:?}", app.overlay);
            assert_eq!(app.flash.as_deref(), Some(super::NO_CARD_FOR_COMMENT));

            let mut app = two_sessions();
            draw(&mut app);
            key(&mut app, KeyCode::Char('y'), KeyModifiers::NONE);
            assert!(app.overlay.is_none(), "{:?}", app.overlay);
            assert!(
                app.flash
                    .as_deref()
                    .is_some_and(|f| f.starts_with("no pull request on ")),
                "{:?}",
                app.flash
            );
        });
    }

    /// The pull request on a band's rule is a link: a click on its
    /// `↗ #42 title` opens it in the browser exactly as `⇧V` does — the
    /// same URL, marked read the same way — and the target is only as
    /// wide as its text: the rest of the rule is the band's.
    #[test]
    fn clicking_the_bands_pull_request_opens_it_as_shift_v_does() {
        with_default_config(|| {
            let mut by_key = card_on_a_pull_request();
            key(&mut by_key, KeyCode::Char('V'), KeyModifiers::SHIFT);

            let mut by_click = card_on_a_pull_request();
            let (line, worktree) = pull_request_line(&by_click);
            let bands = crate::launcher::bands(&by_click);
            let band = bands
                .iter()
                .position(|b| b.worktree == worktree)
                .expect("the line's band");
            let rule = by_click
                .hit_rect(&HitTarget::LauncherBand(band))
                .expect("the band's rule");
            assert_eq!(line.y, rule.y, "on the band's rule");
            assert_eq!(
                line.width as usize,
                "↗ #42 Polish the nav ready".chars().count(),
                "the link is its text, not the rule's width"
            );
            assert_eq!(
                by_click.hit_at(line.x, line.y),
                Some(HitTarget::LauncherBandPr(worktree.clone()))
            );
            assert_eq!(
                by_click.hit_at(line.x + line.width, line.y),
                Some(HitTarget::LauncherBand(band)),
                "the rule after the badge is the band's"
            );

            let sent = click_at(&mut by_click, line.x + 3, line.y);
            assert!(by_click.overlay.is_none(), "{:?}", by_click.overlay);
            assert_eq!(
                by_click.flash.as_deref(),
                Some("opened github.com/o/demo/pull/42")
            );
            assert!(
                sent.iter()
                    .any(|r| matches!(r, ClientRequest::MarkPrSeen { url, .. } if url == PR_42)),
                "the pull request is marked read: {sent:?}"
            );
            assert_eq!(by_click.pr_seen, by_key.pr_seen);
            assert_eq!(by_click.flash, by_key.flash);
            assert_eq!(by_click.focus, Focus::Sessions, "the keys stay on the grid");
            assert_eq!(
                by_click.selected_session().map(|a| a.id.clone()),
                by_key.selected_session().map(|a| a.id.clone())
            );
        });
    }

    /// The pull request on a band the cursor is not on: the click lands
    /// the cursor on that band first — the checkout whose link was
    /// clicked is the one selected — and opens its pull request, not the
    /// old cursor's.
    #[test]
    fn clicking_another_bands_pull_request_lands_the_cursor_on_it_first() {
        with_default_config(|| {
            let mut app = two_sessions();
            draw(&mut app);
            let bands = crate::launcher::bands(&app);
            let cursor =
                crate::launcher::band_cursor(&app, &bands).expect("a band under the cursor");
            let other = (0..bands.len())
                .find(|&i| i != cursor)
                .expect("a second band");
            for (i, number) in [(cursor, 42), (other, 43)] {
                app.pull_requests
                    .insert(bands[i].worktree.clone(), Some(pull_request(number)));
            }
            key(&mut app, KeyCode::Esc, KeyModifiers::NONE);
            draw_at(&mut app, 130, 50);
            let line = app
                .hit_rect(&HitTarget::LauncherBandPr(bands[other].worktree.clone()))
                .expect("the other band's link");

            let sent = click_at(&mut app, line.x, line.y);
            assert_eq!(
                app.selected_worktree().map(|w| w.id.clone()),
                Some(bands[other].worktree.clone()),
                "the cursor is on the band whose link was clicked"
            );
            assert_eq!(
                app.flash.as_deref(),
                Some("opened github.com/o/demo/pull/43"),
                "and it is that band's pull request that opened"
            );
            assert!(
                sent.iter().any(|r| matches!(
                    r,
                    ClientRequest::MarkPrSeen { url, .. } if url == "https://github.com/o/demo/pull/43"
                )),
                "{sent:?}"
            );
        });
    }

    /// A right-click on the line is the card's right-click: the cursor
    /// lands on the card and its CONTEXT MENU opens, **Open pull
    /// request** on it.
    #[test]
    fn right_clicking_the_pull_request_line_opens_the_cards_menu() {
        with_default_config(|| {
            let mut app = card_on_a_pull_request();
            let (line, _) = pull_request_line(&app);
            mouse(
                &mut app,
                MouseEventKind::Down(MouseButton::Right),
                line.x,
                line.y,
            );
            let labels: Vec<String> = match &app.overlay {
                Some(Overlay::Menu(menu)) => menu.items.iter().map(|i| i.label.clone()).collect(),
                other => panic!("expected the card's menu, got {other:?}"),
            };
            assert!(
                labels.iter().any(|l| l == "Open pull request"),
                "{labels:?}"
            );
        });
    }

    /// The pointer resting on a band's pull request underlines its
    /// `#42 title` — nothing about a rule says part of it is a link — and
    /// only while it is there: off the link, the underline goes with it.
    #[test]
    fn the_pointer_marks_the_bands_pull_request() {
        with_default_config(|| {
            let mut app = card_on_a_pull_request();
            let (line, worktree) = pull_request_line(&app);
            let underlined = |terminal: &Terminal<TestBackend>| -> String {
                let buf = terminal.backend().buffer();
                (line.x..line.x + line.width)
                    .filter_map(|x| buf.cell((x, line.y)))
                    .filter(|c| c.modifier.contains(Modifier::UNDERLINED))
                    .map(|c| c.symbol().to_string())
                    .collect()
            };
            assert_eq!(underlined(&draw(&mut app)), "");

            mouse(&mut app, MouseEventKind::Moved, line.x + 2, line.y);
            assert_eq!(app.hover_crumb, Some(HitTarget::LauncherBandPr(worktree)));
            assert_eq!(underlined(&draw(&mut app)), "#42 Polish the nav");

            mouse(&mut app, MouseEventKind::Moved, line.x, line.y + 2);
            assert_eq!(app.hover_crumb, None, "the card under the rule is no link");
            assert_eq!(underlined(&draw(&mut app)), "");
        });
    }

    /// The air around a band's cards — beside the last one, in the rows
    /// under the rule — is the band's: a click there lands the cursor on
    /// that checkout as a click on its rule does, not on nothing.
    #[test]
    fn clicking_the_air_inside_a_band_selects_its_worktree() {
        with_default_config(|| {
            let mut app = two_sessions();
            draw(&mut app);
            let bands = crate::launcher::bands(&app);
            let cursor =
                crate::launcher::band_cursor(&app, &bands).expect("a band under the cursor");
            let other = (0..bands.len())
                .find(|&i| i != cursor)
                .expect("a second band");
            key(&mut app, KeyCode::Esc, KeyModifiers::NONE);
            draw_at(&mut app, 130, 50);
            let rule = app
                .hit_rect(&HitTarget::LauncherBand(other))
                .expect("the other band's rule");
            let whole = app
                .hits
                .iter()
                .filter(|(_, h)| *h == HitTarget::LauncherBand(other))
                .map(|(r, _)| *r)
                .find(|r| r.height > 1)
                .expect("the other band's whole rectangle");
            assert_eq!(whole.y, rule.y, "it starts at the band's rule");
            let (x, y) = (whole.y + 1..whole.y + whole.height)
                .flat_map(|y| (whole.x..whole.x + whole.width).map(move |x| (x, y)))
                .find(|&(x, y)| app.hit_at(x, y) == Some(HitTarget::LauncherBand(other)))
                .expect("air beside the band's cards");

            click_at(&mut app, x, y);
            assert_eq!(
                app.selected_worktree().map(|w| w.id.clone()),
                Some(bands[other].worktree.clone()),
                "the cursor is on the band whose air was clicked"
            );
        });
    }

    /// A checkout with no pull request draws no link on its rule and so
    /// has no target for one: a click where the link would be is a click
    /// on the band.
    #[test]
    fn a_band_without_a_pull_request_has_no_link_to_click() {
        with_default_config(|| {
            let mut app = two_sessions();
            draw(&mut app);
            assert!(
                !app.hits
                    .iter()
                    .any(|(_, h)| matches!(h, HitTarget::LauncherBandPr(_))),
                "no checkout has a pull request"
            );
        });
    }

    /// `⇧P` used to open the card's pull request; it is `⇧V` now, and
    /// `⇧P` opens the QUICK PROMPT on the card's settings instead — the
    /// pull request left alone.
    #[test]
    fn shift_p_no_longer_opens_the_cards_pull_request() {
        with_default_config(|| {
            let mut app = card_on_a_pull_request();
            let sent = key(&mut app, KeyCode::Char('P'), KeyModifiers::SHIFT);
            assert!(sent.is_empty(), "{sent:?}");
            quick_box(&app);
            assert_ne!(
                app.flash.as_deref(),
                Some("opened github.com/o/demo/pull/42")
            );
        });
    }

    /// [`two_sessions`], drawn, with the card under the cursor a CODEX
    /// session at an explicit model and effort, started from issue #15 —
    /// every setting the box has to come up on.
    fn card_with_settings() -> App {
        let mut app = two_sessions();
        draw(&mut app);
        let mut agent = app.selected_session().expect("a card under the cursor");
        agent.kind = AgentKind::Codex;
        agent.model = Some("gpt-5".into());
        agent.effort = Some("high".into());
        agent.issue_url = Some(ISSUE_15.into());
        hse(
            &mut app,
            ServerEvent::EntityUpserted {
                entity: Entity::Agent(agent),
            },
        );
        draw(&mut app);
        app
    }

    /// Where **Duplicate** sits on the card's right-click menu.
    fn duplicate_menu_row(app: &mut App) -> usize {
        right_click_card(app);
        match &app.overlay {
            Some(Overlay::Menu(menu)) => menu
                .items
                .iter()
                .position(|i| i.label == "Duplicate")
                .expect("the row is on the card's menu"),
            other => panic!("expected the card's menu, got {other:?}"),
        }
    }

    /// The QUICK PROMPT that is up, and what it is set to launch.
    fn quick_box(app: &App) -> QuickLaunch {
        match &app.overlay {
            Some(Overlay::Prompt(prompt)) => match &prompt.kind {
                PromptKind::QuickPrompt(launch) => launch.clone(),
                other => panic!("expected the quick prompt, got {other:?}"),
            },
            other => panic!("expected the quick prompt, got {other:?}"),
        }
    }

    /// `⇧P` on a card opens the QUICK PROMPT set to launch what the card
    /// runs — its harness, model, effort and checkout, and the issue it
    /// was started from — with nothing typed and nothing sent: the task
    /// is typed there, and Enter sends the create with those settings and
    /// the text as the first prompt. INPUT PARITY: the card menu's
    /// **Duplicate** puts up the same box.
    #[test]
    fn shift_p_opens_the_quick_prompt_on_the_cards_settings() {
        with_default_config(|| {
            let mut by_key = card_with_settings();
            let card = by_key.selected_session().expect("a card under the cursor");
            let sent = key(&mut by_key, KeyCode::Char('P'), KeyModifiers::SHIFT);
            assert!(sent.is_empty(), "nothing starts until Enter: {sent:?}");
            let launch = quick_box(&by_key);
            assert_eq!(
                launch.target,
                QuickTarget::Worktree(card.worktree_id.clone())
            );
            assert_eq!(launch.kind, AgentKind::Codex);
            assert_eq!(launch.custom, None);
            assert_eq!(launch.model.as_deref(), Some("gpt-5"));
            assert_eq!(launch.effort.as_deref(), Some("high"));
            assert_eq!(
                launch.issue.as_ref().map(|i| (i.url.as_str(), i.number)),
                Some((ISSUE_15, 15))
            );
            assert!(launch.preset.is_none());
            assert!(!launch.cloud);

            let mut by_menu = card_with_settings();
            let at = duplicate_menu_row(&mut by_menu);
            for _ in 0..at {
                key(&mut by_menu, KeyCode::Down, KeyModifiers::NONE);
            }
            key(&mut by_menu, KeyCode::Enter, KeyModifiers::NONE);
            assert_eq!(quick_box(&by_menu), launch);

            // And Enter in the box is the launch, on the card's settings.
            type_text(&mut by_key, "tidy the nav");
            let sent = key(&mut by_key, KeyCode::Enter, KeyModifiers::NONE);
            assert!(by_key.overlay.is_none(), "{:?}", by_key.overlay);
            match sent.as_slice() {
                [ClientRequest::CreateAgent {
                    worktree,
                    kind,
                    custom_harness,
                    model,
                    effort,
                    auto_title,
                    cloud_prompt,
                    starting_prompt,
                    issue_url,
                    ..
                }] => {
                    assert_eq!(worktree, &card.worktree_id);
                    assert_eq!(*kind, AgentKind::Codex);
                    assert_eq!(custom_harness, &None);
                    assert_eq!(model.as_deref(), Some("gpt-5"));
                    assert_eq!(effort.as_deref(), Some("high"));
                    assert!(auto_title, "the row titles itself on its first prompt");
                    assert_eq!(cloud_prompt, &None);
                    assert_eq!(starting_prompt.as_deref(), Some("tidy the nav"));
                    assert_eq!(issue_url.as_deref(), Some(ISSUE_15));
                }
                other => panic!("one CreateAgent: {other:?}"),
            }
        });
    }

    /// With the aim let go of (Esc), no card wears the cursor, so `⇧P`
    /// has no settings to open the box on, and opens nothing.
    #[test]
    fn shift_p_with_no_card_selected_opens_nothing() {
        with_default_config(|| {
            let mut app = card_with_settings();
            keys(&mut app, &[KeyCode::Esc, KeyCode::Esc]);
            assert!(app.launcher_unaimed);
            let sent = key(&mut app, KeyCode::Char('P'), KeyModifiers::SHIFT);
            assert!(sent.is_empty(), "{sent:?}");
            assert!(app.overlay.is_none(), "{:?}", app.overlay);
            assert_eq!(app.flash.as_deref(), Some(super::NO_CARD_TO_DUPLICATE));
        });
    }

    /// A CLOUD card has no local CLI to start again: `⇧P` on one opens
    /// the box as a CLOUD one on the card's checkout, model and effort,
    /// its Enter the cloud task.
    #[test]
    fn shift_p_on_a_cloud_card_opens_a_cloud_box_on_its_settings() {
        with_default_config(|| {
            let mut app = two_sessions();
            draw(&mut app);
            let mut agent = app.selected_session().expect("a card under the cursor");
            agent.cloud_session_id = Some("session_01".into());
            agent.model = Some("opus".into());
            agent.effort = Some("high".into());
            let (id, worktree) = (agent.id.clone(), agent.worktree_id.clone());
            hse(
                &mut app,
                ServerEvent::EntityUpserted {
                    entity: Entity::Agent(agent),
                },
            );
            draw(&mut app);
            assert_eq!(app.selected_session().map(|a| a.id), Some(id));
            let sent = key(&mut app, KeyCode::Char('P'), KeyModifiers::SHIFT);
            assert!(sent.is_empty(), "{sent:?}");
            let launch = quick_box(&app);
            assert!(launch.cloud, "{launch:?}");
            assert_eq!(launch.target, QuickTarget::Worktree(worktree));
            assert_eq!(launch.kind, AgentKind::Claude);
            assert_eq!(launch.model.as_deref(), Some("opus"));
            assert_eq!(launch.effort.as_deref(), Some("high"));
        });
    }

    const ISSUE_15: &str = "https://github.com/o/demo/issues/15";

    /// [`two_sessions`], drawn, with the card under the cursor an ISSUE
    /// SESSION started from issue #15 — the daemon's upsert carrying the
    /// URL, as it does for a session launched out of the ISSUES MODAL.
    fn card_from_an_issue() -> App {
        let mut app = two_sessions();
        draw(&mut app);
        let mut agent = app.selected_session().expect("a card under the cursor");
        agent.issue_url = Some(ISSUE_15.into());
        hse(
            &mut app,
            ServerEvent::EntityUpserted {
                entity: Entity::Agent(agent),
            },
        );
        draw(&mut app);
        app
    }

    /// Where **Open issue** sits on the card's right-click menu, if it is
    /// there.
    fn issue_menu_row(app: &mut App) -> Option<usize> {
        right_click_card(app);
        match &app.overlay {
            Some(Overlay::Menu(menu)) => menu.items.iter().position(|i| i.label == "Open issue"),
            other => panic!("expected the card's menu, got {other:?}"),
        }
    }

    /// `⇧I` on a card opens the issue its session was started from — and,
    /// INPUT PARITY, the card menu's **Open issue** ends in the same state.
    #[test]
    fn shift_i_opens_the_cards_issue_as_its_menu_row_does() {
        with_default_config(|| {
            let mut by_key = card_from_an_issue();
            let sent = key(&mut by_key, KeyCode::Char('I'), KeyModifiers::SHIFT);
            assert!(by_key.overlay.is_none(), "{:?}", by_key.overlay);
            assert_eq!(
                by_key.flash.as_deref(),
                Some("opened github.com/o/demo/issues/15")
            );

            let mut by_menu = card_from_an_issue();
            let at = issue_menu_row(&mut by_menu).expect("the row is on the card's menu");
            for _ in 0..at {
                key(&mut by_menu, KeyCode::Down, KeyModifiers::NONE);
            }
            let sent_by_menu = key(&mut by_menu, KeyCode::Enter, KeyModifiers::NONE);
            assert!(by_menu.overlay.is_none(), "{:?}", by_menu.overlay);
            assert_eq!(by_menu.flash, by_key.flash);
            assert_eq!(format!("{sent_by_menu:?}"), format!("{sent:?}"));
        });
    }

    /// With the CARD ISSUE NUMBER setting on, an ISSUE SESSION's card
    /// shows its `#15` as a link: a click on it lands the cursor on the
    /// card and opens the issue exactly as `⇧I` does — INPUT PARITY — and
    /// the target is only as wide as its text: the rest of the card is
    /// the card's.
    #[test]
    fn clicking_the_cards_issue_number_opens_it_as_shift_i_does() {
        with_default_config(|| {
            let mut by_key = card_from_an_issue();
            let id = by_key.selected_session().expect("a card").id;
            let sent_by_key = key(&mut by_key, KeyCode::Char('I'), KeyModifiers::SHIFT);

            let mut by_click = card_from_an_issue();
            by_click.card_issue_number = true;
            // The aim let go of, so the click is what puts it back.
            keys(&mut by_click, &[KeyCode::Esc, KeyCode::Esc]);
            assert!(by_click.launcher_unaimed);
            draw(&mut by_click);
            let chip = by_click
                .hit_rect(&HitTarget::LauncherCardIssue(id.clone()))
                .expect("the issue number is drawn on the card");
            assert_eq!(chip.width, 3, "the link is `#15`, not the card's width");
            assert!(
                matches!(
                    by_click.hit_at(chip.x - 1, chip.y),
                    Some(HitTarget::LauncherCard(_))
                ),
                "the card left of the number is the card's"
            );

            let sent = click_at(&mut by_click, chip.x, chip.y);
            assert!(by_click.overlay.is_none(), "{:?}", by_click.overlay);
            assert_eq!(by_click.flash, by_key.flash);
            assert_eq!(
                by_click.flash.as_deref(),
                Some("opened github.com/o/demo/issues/15")
            );
            assert!(!by_click.launcher_unaimed, "the cursor is back on the card");
            assert_eq!(by_click.selected_session().map(|a| a.id), Some(id));
            assert_eq!(by_click.focus, Focus::Sessions, "the keys stay on the grid");
            assert_eq!(
                sent.iter()
                    .filter(|r| format!("{r:?}").contains("issues/15"))
                    .count(),
                sent_by_key
                    .iter()
                    .filter(|r| format!("{r:?}").contains("issues/15"))
                    .count(),
            );
        });
    }

    /// The setting is off by default: no issue number on the card, and
    /// nothing on it to click but the card.
    #[test]
    fn the_cards_issue_number_is_off_by_default() {
        with_default_config(|| {
            let app = card_from_an_issue();
            assert!(!app.card_issue_number);
            assert!(
                !app.hits
                    .iter()
                    .any(|(_, h)| matches!(h, HitTarget::LauncherCardIssue(_))),
                "no issue link drawn"
            );
        });
    }

    /// A right-click on the issue number is a right-click on its card:
    /// the card's menu, **Open issue** on it.
    #[test]
    fn right_clicking_the_cards_issue_number_opens_the_cards_menu() {
        with_default_config(|| {
            let mut app = card_from_an_issue();
            app.card_issue_number = true;
            draw(&mut app);
            let id = app.selected_session().expect("a card").id;
            let chip = app
                .hit_rect(&HitTarget::LauncherCardIssue(id))
                .expect("the issue number is drawn on the card");
            mouse(
                &mut app,
                MouseEventKind::Down(MouseButton::Right),
                chip.x,
                chip.y,
            );
            match &app.overlay {
                Some(Overlay::Menu(menu)) => assert!(
                    menu.items.iter().any(|i| i.label == "Open issue"),
                    "{:?}",
                    menu.items.iter().map(|i| &i.label).collect::<Vec<_>>()
                ),
                other => panic!("expected the card's menu, got {other:?}"),
            }
        });
    }

    /// A card that was not started from an issue says so, and its menu
    /// carries no row for one.
    #[test]
    fn shift_i_on_a_card_with_no_issue_says_so() {
        with_default_config(|| {
            let mut app = two_sessions();
            draw(&mut app);
            let sent = key(&mut app, KeyCode::Char('I'), KeyModifiers::SHIFT);
            assert!(sent.is_empty(), "{sent:?}");
            assert_eq!(app.flash.as_deref(), Some(super::NO_ISSUE));
            assert_eq!(issue_menu_row(&mut app), None);
        });
    }

    /// With the aim let go of (Esc), `⇧I` has no card to read an issue off.
    #[test]
    fn shift_i_with_no_card_selected_opens_nothing() {
        with_default_config(|| {
            let mut app = card_from_an_issue();
            keys(&mut app, &[KeyCode::Esc, KeyCode::Esc]);
            assert!(app.launcher_unaimed);
            let sent = key(&mut app, KeyCode::Char('I'), KeyModifiers::SHIFT);
            assert!(sent.is_empty(), "{sent:?}");
            assert_eq!(app.flash.as_deref(), Some(super::NO_CARD_FOR_ISSUE));
        });
    }

    /// The PROJECT TABS' STATUS DOTS, on the header row: one dot per
    /// state a tab's sessions are in, carrying that state's count and no
    /// word — so what is waiting on a human is the red dot, and reading it
    /// means reading the color the cell is painted in.
    fn head_tally(terminal: &Terminal<TestBackend>) -> Vec<(String, Color)> {
        let buf = terminal.backend().buffer();
        let mut dots = Vec::new();
        for x in 0..buf.area.width {
            let Some(dot) = buf.cell((x, 1)) else {
                continue;
            };
            if dot.symbol() != "●" {
                continue;
            }
            // `●12`: the count starts on the next cell and runs as far as
            // the digits do, so the `2 sessions` off on the right edge is
            // never read as part of it.
            let mut count = String::new();
            let mut i = x + 1;
            while let Some(cell) = buf.cell((i, 1)) {
                if !cell.symbol().chars().all(|c| c.is_ascii_digit()) {
                    break;
                }
                count.push_str(cell.symbol());
                i += 1;
            }
            dots.push((count, dot.fg));
        }
        dots
    }

    /// The header counts what is waiting on a human in the project tab's
    /// red dot — the count, and not a word beside it. Nothing else on the
    /// row says it: the dot is the whole announcement.
    #[test]
    fn the_header_counts_the_sessions_waiting_on_you() {
        with_default_config(|| {
            let mut app = two_sessions();
            draw(&mut app);
            let red = app.theme.err;
            let term = draw(&mut app);
            let text = buffer_text(&term);
            assert!(
                !head_tally(&term).iter().any(|(_, c)| *c == red),
                "nobody is waiting yet: {text}"
            );

            for a in app.tree.agents.iter_mut() {
                if a.id.0 == "a2" {
                    a.status = AgentStatus::NeedsFeedback;
                }
            }
            let term = draw(&mut app);
            let text = buffer_text(&term);
            assert_eq!(
                head_tally(&term)
                    .into_iter()
                    .find(|(_, c)| *c == red)
                    .map(|(count, _)| count),
                Some("1".to_string()),
                "one red dot, carrying its count: {text}"
            );
            assert!(
                !text.contains("needs you"),
                "the dot says it; the header spends no words on it: {text}"
            );
        });
    }

    /// The fg of each cell a PROJECT TAB's name is painted in, on the
    /// header row — found by what it spells, since tabs move as others
    /// open.
    fn tab_name_colors(terminal: &Terminal<TestBackend>, name: &str) -> Vec<Color> {
        let buf = terminal.backend().buffer();
        let cells: Vec<_> = (0..buf.area.width)
            .filter_map(|x| buf.cell((x, 1)))
            .collect();
        let want: Vec<String> = name.chars().map(String::from).collect();
        let at = cells
            .windows(want.len())
            .position(|w| w.iter().zip(&want).all(|(c, s)| c.symbol() == s))
            .unwrap_or_else(|| panic!("no {name:?} tab on the header row"));
        cells[at..at + want.len()].iter().map(|c| c.fg).collect()
    }

    /// A PROJECT TAB's name sweeps on the loudest thing its sessions are
    /// doing, lit tab or not: yellow while one runs, red while one waits
    /// on you whatever else runs, and blue once none is live but a finish
    /// is left unread — for as long as it stays unread, so the sweep clock
    /// keeps running for it — then still once it is read. The animations
    /// setting stops all three.
    #[test]
    fn a_project_tabs_name_sweeps_the_loudest_status_under_it() {
        with_default_config(|| {
            let mut app = two_sessions();
            app.launcher_tabs.push(ProjectId("p2".into()));
            draw(&mut app);
            let th = app.theme;
            let sweeps = |term: &Terminal<TestBackend>, name: &str, ramp: [Color; 3]| {
                tab_name_colors(term, name).iter().all(|c| ramp.contains(c))
            };
            let set = |app: &mut App, id: &str, status: AgentStatus, unseen: bool| {
                let a = app.tree.agents.iter_mut().find(|a| a.id.0 == id).unwrap();
                a.status = status;
                a.unseen = unseen;
            };

            let term = draw(&mut app);
            assert!(sweeps(&term, "demo", th.warn_sweep), "polish-nav runs");
            assert!(sweeps(&term, "web", th.warn_sweep), "the tab not lit too");

            set(&mut app, "a1", AgentStatus::NeedsFeedback, false);
            assert!(
                sweeps(&draw(&mut app), "demo", th.err_sweep),
                "red outranks the one still running"
            );

            set(&mut app, "a1", AgentStatus::Finished, true);
            assert!(
                sweeps(&draw(&mut app), "demo", th.warn_sweep),
                "a finish is not the project done while another runs"
            );

            set(&mut app, "a2", AgentStatus::Finished, true);
            set(&mut app, "a3", AgentStatus::Finished, false);
            let term = draw(&mut app);
            assert!(sweeps(&term, "demo", th.done_sweep), "all done: blue");
            assert_eq!(
                tab_name_colors(&term, "web"),
                vec![th.muted; 3],
                "web is quiet"
            );
            assert!(
                app.status_anim_active(),
                "long-finished, but unread: the blue keeps the clock running"
            );

            set(&mut app, "a1", AgentStatus::Finished, false);
            set(&mut app, "a2", AgentStatus::Finished, false);
            let term = draw(&mut app);
            assert_eq!(
                tab_name_colors(&term, "demo"),
                vec![th.accent; 4],
                "read: still"
            );
            assert!(!app.status_anim_active(), "and nothing left to tick for");

            set(&mut app, "a2", AgentStatus::Running, false);
            app.animations = false;
            assert_eq!(
                tab_name_colors(&draw(&mut app), "demo"),
                vec![th.accent; 4],
                "animations off"
            );
        });
    }

    /// However narrow the terminal, the tabs and the count keep off each
    /// other: a tab is cut, then counted rather than drawn — never drawn
    /// over the count (the two are separate right/left-aligned paragraphs
    /// on one row, so an overlong tab would overprint it) — and the `+`
    /// is never pushed off.
    #[test]
    fn the_header_never_overprints_its_count() {
        with_default_config(|| {
            let mut app = two_sessions();
            app.launcher_expanded = None;
            // Below this the count itself no longer fits the row, and
            // nothing that could be drawn there would be readable.
            for width in 24..=130u16 {
                let text = buffer_text(&draw_at(&mut app, width, 50));
                let head = text.lines().nth(1).unwrap_or_default().to_string();
                assert!(head.contains("2 sessions"), "{width}: {head:?}");
                // The tabs end before the count begins: the gap between
                // the two is real air, not a letter eaten by one of them.
                let tabs = head.split("2 sessions").next().unwrap_or_default();
                assert!(
                    tabs.ends_with("  "),
                    "{width}: tabs run into the count: {head:?}"
                );
                assert!(tabs.contains('+'), "{width}: {head:?}");
            }
            // Wide enough for the tab whole.
            draw_at(&mut app, 130, 34);
            assert_eq!(tabs_drawn(&app), ["demo"]);
        });
    }

    /// The card's last line is the last thing the session was asked to
    /// do — the newest of the RECENT PROMPTS, on the prompt's own `›`.
    #[test]
    fn a_card_says_what_its_session_was_last_asked() {
        with_default_config(|| {
            let mut app = two_sessions();
            for a in app.tree.agents.iter_mut() {
                if a.id.0 == "a2" {
                    a.recent_prompts = vec![
                        nebula_core::PromptEntry {
                            text: "first pass at the nav".into(),
                            submitted_at: 1,
                        },
                        nebula_core::PromptEntry {
                            text: "now make it sticky".into(),
                            submitted_at: 2,
                        },
                    ];
                }
            }
            app.launcher_expanded = None;
            let text = buffer_text(&draw_at(&mut app, 130, 50));
            assert!(text.contains("› now make it sticky"), "the newest: {text}");
            assert!(!text.contains("first pass at the nav"), "{text}");
        });
    }

    /// A prompt too long for one row keeps going on the rows under it,
    /// indented to the `›`'s own column — four lines of what was asked,
    /// not a sentence clipped at the card's edge — and whatever still
    /// does not fit ends in an ellipsis rather than growing the card.
    #[test]
    fn a_long_prompt_runs_over_three_card_rows() {
        with_default_config(|| {
            let mut app = two_sessions();
            for a in app.tree.agents.iter_mut() {
                if a.id.0 == "a2" {
                    a.recent_prompts = vec![nebula_core::PromptEntry {
                        text: "show at least three lines of the original prompt inside each session card, so a long ask reads as a sentence instead of a fragment"
                            .into(),
                        submitted_at: 1,
                    }];
                }
            }
            // One card a row, so a buffer row is one card's and the
            // continuation cannot be a neighbour card's text.
            app.launcher_expanded = None;
            let text = buffer_text(&draw_at(&mut app, 44, 60));
            let rows: Vec<&str> = text.lines().collect();
            let head = rows
                .iter()
                .position(|r| r.contains("› show at least three lines of the"))
                .unwrap_or_else(|| panic!("no prompt row: {text}"));
            assert!(
                rows[head + 1].contains("original prompt inside each"),
                "the rest of it, on the row under: {text}"
            );
            assert!(
                rows[head + 2].contains("session card, so a long ask reads"),
                "and a third row: {text}"
            );
            assert!(
                rows[head + 3].contains("as a sentence instead of a…"),
                "and a fourth, ending in an ellipsis: {text}"
            );
            assert!(
                !rows[head + 1].contains('›') && !rows[head + 2].contains('›'),
                "only the first row is marked: {text}"
            );
        });
    }

    /// The box, in the view: each of its details on its first row beside
    /// the chord that changes it — the checkout the launch lands in among
    /// them, beside its `^T` — and the prompt header under them with the
    /// toggle that cuts a fresh checkout. None of those chords is repeated
    /// on the border — that repetition was the box's wall of text.
    #[test]
    fn the_box_names_its_details_and_its_keys() {
        with_default_config(|| {
            let mut app = two_sessions();
            draw(&mut app);
            key(&mut app, KeyCode::Char('p'), KeyModifiers::NONE);
            let text = buffer_text(&draw(&mut app));
            assert!(text.contains("project demo ^P"), "{text}");
            assert!(text.contains("harness claude Tab"), "{text}");
            assert!(text.contains("model default ^O"), "{text}");
            assert!(text.contains("new worktree ^N"), "{text}");
            assert!(
                text.contains("worktree main ^T"),
                "where the launch lands: {text}"
            );
            assert!(!text.contains("(demo / "), "not twice over: {text}");
            assert!(!text.contains("^P project"), "not twice over: {text}");
            assert!(!text.contains("^O model"), "not twice over: {text}");

            key(&mut app, KeyCode::Char('p'), KeyModifiers::CONTROL);
            let text = buffer_text(&draw(&mut app));
            assert!(text.contains("type a project name"), "{text}");
            assert!(text.contains("web"), "{text}");
        });
    }

    /// Out to the PROJECTS level the way the keys walk it: Esc lets the
    /// `k` on the grid's top row has nowhere left to go, and however many
    /// times it is pressed it goes nowhere: the grid is the top of the
    /// view, with no level above it to walk out to.
    #[test]
    fn k_on_the_top_row_stays_put() {
        with_default_config(|| {
            let mut app = two_sessions();
            draw(&mut app);
            let before = (selected(&app), app.selected_project().map(|p| p.id.clone()));
            key(&mut app, KeyCode::Char('k'), KeyModifiers::NONE);
            key(&mut app, KeyCode::Char('k'), KeyModifiers::NONE);
            assert_eq!(
                (selected(&app), app.selected_project().map(|p| p.id.clone())),
                before
            );
            assert!(app.overlay.is_none(), "{:?}", app.overlay);
        });
    }

    /// The PR sweep covers every checkout the SESSIONS level lists — not
    /// only the one under the worktree cursor, since the level names a
    /// pull request under every card — and follows the level when it is
    /// walked into another project, which is the only list it has.
    #[test]
    fn the_pr_sweep_reaches_every_listed_sessions_checkout() {
        let feat = tempfile::tempdir().unwrap();
        let mut app = App::new();
        seed_tree(&mut app);
        seed_feat(&mut app, feat.path().to_path_buf());
        seed_web(&mut app);
        app.sel_project = project_row(&app, "p1");
        let (id, _) = sweep_target(&mut app).expect("demo's other checkout is listed");
        assert_eq!(id, WorktreeId("w2".into()));

        // Walked into `web`, whose only checkout is its root: the sweep
        // went with the level and has nothing to spend the tick on.
        app.sel_project = project_row(&app, "p2");
        assert_eq!(sweep_target(&mut app), None, "the sweep followed the level");
    }

    /// The first Esc lets the card under the cursor go; a click on the
    /// air between the cards does NOT. A miss with the pointer — the
    /// gutter between two cards, the blank rows under a short last row —
    /// is not a request to close the session being read, so it takes
    /// FOCUS and changes nothing else. Letting the card go stays a key.
    #[test]
    fn the_first_esc_lets_the_card_go_and_a_click_on_the_air_does_not() {
        with_default_config(|| {
            let mut by_click = two_sessions();
            draw(&mut by_click);
            let (x, y) = air(&by_click);
            by_click.flash = None;
            let was = by_click.sel_session;
            mouse(&mut by_click, MouseEventKind::Down(MouseButton::Left), x, y);
            assert!(
                !by_click.launcher_unaimed,
                "the click on the air let the card go"
            );
            assert_eq!(by_click.flash, None, "and it said something about it");
            assert_eq!(by_click.sel_session, was, "it moved the cursor");
            assert_eq!(by_click.focus, Focus::Sessions, "the keys are the grid's");

            let mut by_key = two_sessions();
            draw(&mut by_key);
            by_key.flash = None;
            key(&mut by_key, KeyCode::Esc, KeyModifiers::NONE);
            assert!(by_key.launcher_unaimed, "Esc lets the card go");
            assert_eq!(by_key.flash.as_deref(), Some(super::UNAIMED));

            // With the band open, Esc closes it first, and only the
            // second press lets the card go.
            let mut by_key = two_sessions();
            draw(&mut by_key);
            key(&mut by_key, KeyCode::Tab, KeyModifiers::NONE);
            assert!(by_key.launcher_expanded.is_some(), "Tab opened the band");
            by_key.flash = None;
            key(&mut by_key, KeyCode::Esc, KeyModifiers::NONE);
            assert!(
                by_key.launcher_expanded.is_none(),
                "the first Esc closes the band"
            );
            assert!(!by_key.launcher_unaimed, "with the band still aimed at");
            assert_eq!(by_key.flash, None);
            key(&mut by_key, KeyCode::Esc, KeyModifiers::NONE);
            assert!(by_key.launcher_unaimed, "the second lets the card go");
            assert_eq!(by_key.flash.as_deref(), Some(super::UNAIMED));
        });
    }

    /// On the open band the cursor's card wears the accent border; closed
    /// again the band's card keeps it — it is what the pane reads and
    /// what `h`/`l` walk along the row — and only letting the aim go (a
    /// second Esc) takes it off. A click on the card aims the band at it
    /// again, border and all, and a second click is Enter: into the pane.
    #[test]
    fn the_unselected_card_stops_wearing_the_cursor() {
        with_default_config(|| {
            let mut app = two_sessions();
            draw(&mut app);
            key(&mut app, KeyCode::Tab, KeyModifiers::NONE);
            let terminal = draw(&mut app);
            let bands = crate::launcher::bands(&app);
            assert!(
                app.cursor_in_open_band(&bands).is_some(),
                "the band is open"
            );
            let at = crate::launcher::cursor(&app, &bands).expect("a card under the cursor");
            let (cell, _) = *app
                .hits
                .iter()
                .find(|(_, hit)| *hit == HitTarget::LauncherCard(at))
                .expect("the cursor's card was drawn");
            let accent = app.theme.accent;
            let edge = app.theme.edge;
            assert_eq!(
                corner(&terminal, cell),
                accent,
                "the cursor's card starts out wearing the accent border"
            );

            // Esc closes the band: its card keeps the border on the
            // collapsed row — the cursor is on the band, and the card is
            // the one it remembers.
            key(&mut app, KeyCode::Esc, KeyModifiers::NONE);
            assert!(app.launcher_expanded.is_none(), "closed");
            let terminal = draw(&mut app);
            let (cell, _) = *app
                .hits
                .iter()
                .find(|(_, hit)| *hit == HitTarget::LauncherCard(at))
                .expect("the card is still drawn, under its band's rule");
            assert_eq!(
                corner(&terminal, cell),
                accent,
                "the band's card wears the cursor at the band level"
            );

            // A second Esc lets the aim go: no card wears the cursor,
            // though the grid still knows where it was.
            key(&mut app, KeyCode::Esc, KeyModifiers::NONE);
            assert!(app.launcher_unaimed);
            let terminal = draw(&mut app);
            assert_eq!(
                corner(&terminal, cell),
                edge,
                "no card wears the cursor once the aim is let go of"
            );
            assert!(
                crate::launcher::cursor(&app, &bands).is_some(),
                "the grid still knows where the cursor was — it is left, not forgotten"
            );

            // A click on the card aims the band at it and no more: the
            // border comes back, the band stays collapsed. The second
            // click is Enter: into the pane, the card still marked.
            mouse(
                &mut app,
                MouseEventKind::Down(MouseButton::Left),
                cell.x + 2,
                cell.y + 1,
            );
            assert!(app.launcher_expanded.is_none(), "one click opens nothing");
            assert!(!app.launcher_unaimed, "aimed at the card's band");
            assert_eq!(app.focus, Focus::Sessions, "the keys stay on the grid");
            let terminal = draw(&mut app);
            assert_eq!(corner(&terminal, cell), accent);
            mouse(
                &mut app,
                MouseEventKind::Down(MouseButton::Left),
                cell.x + 2,
                cell.y + 1,
            );
            assert!(
                app.launcher_expanded.is_none(),
                "the second click opens nothing either"
            );
            assert_eq!(app.focus, Focus::Terminal, "it is Enter: into the pane");
            let terminal = draw(&mut app);
            let (cell, _) = *app
                .hits
                .iter()
                .find(|(_, hit)| *hit == HitTarget::LauncherCard(at))
                .expect("the cursor's card was drawn");
            assert_eq!(corner(&terminal, cell), accent);
        });
    }

    /// The PANE along the bottom IS the selected session, so it comes
    /// and goes with the card under the cursor: letting the card go — the
    /// first Esc — collapses the pane and gives the grid the whole body,
    /// and a click back on a card opens it again. A click OPENS the pane
    /// and no more: the keys stay on the cards, and only the second click
    /// crosses into it.
    ///
    /// A click on the AIR between the cards is none of that: the pane it
    /// was reading stays exactly where it is. Missing a card with the
    /// pointer — the gutter, the blank rows under a short last row — is
    /// the easiest click in the view to make by accident, and it used to
    /// shut the session under the grid every time.
    #[test]
    fn a_click_opens_the_pane_and_esc_collapses_it() {
        with_default_config(|| {
            let mut app = two_sessions();
            let draw = |app: &mut App| draw_at(app, 130, 50);
            draw(&mut app);
            let has_pane = |app: &App| {
                app.hits
                    .iter()
                    .any(|(_, hit)| *hit == HitTarget::LauncherPaneSplitter)
            };
            let cards_h = |app: &App| {
                app.hits
                    .iter()
                    .find(|(_, hit)| *hit == HitTarget::PanelBg(Focus::Sessions))
                    .map(|(area, _)| area.height)
                    .expect("the grid registered its background")
            };
            let pane_h = crate::launcher::pane_height(app.launcher_body, app.launcher_pane_h)
                .expect("34 rows fits a pane");
            let grid_with_pane = cards_h(&app);
            assert!(has_pane(&app) && !app.launcher_unaimed);

            // A click that misses every card leaves all of it alone:
            // same pane, same rows, same card under the cursor.
            let (ax, ay) = air(&app);
            mouse(&mut app, MouseEventKind::Down(MouseButton::Left), ax, ay);
            draw(&mut app);
            assert!(
                has_pane(&app) && !app.launcher_unaimed,
                "a click on the air shut the pane"
            );
            assert_eq!(cards_h(&app), grid_with_pane);

            // The card let go of — out of the worktree, then off the band:
            // the pane goes with it and the cards take the rows it was
            // drawn over.
            keys(&mut app, &[KeyCode::Esc, KeyCode::Esc]);
            draw(&mut app);
            assert!(app.launcher_unaimed);
            assert!(!has_pane(&app), "nothing selected, no pane");
            assert_eq!(cards_h(&app), grid_with_pane + pane_h);

            // A click on a card opens it again — and only opens it.
            let (x, y) = row_cell(&app, 0);
            mouse(&mut app, MouseEventKind::Down(MouseButton::Left), x, y);
            assert!(!app.launcher_unaimed, "the click aimed the grid again");
            assert_eq!(
                app.focus,
                Focus::Sessions,
                "one click opens the pane, it does not enter it"
            );
            assert!(!app.term_locked, "and nothing is being typed into");
            draw(&mut app);
            assert!(has_pane(&app), "the pane came back under the cards");
            assert_eq!(cards_h(&app), grid_with_pane);

            // The second click on the same card is Enter, into the pane.
            mouse(&mut app, MouseEventKind::Down(MouseButton::Left), x, y);
            assert_eq!(app.focus, Focus::Terminal, "the second click is Enter");
            assert!(!app.collapsed, "the pane under the grid, not full-screen");

            // And with the keys in the pane, letting the card go takes
            // them back out with it rather than leaving FOCUS on a pane
            // that is no longer drawn (`App::settle_launcher_focus`).
            // Straight through [`clear_aim`]: from inside a LOCKED PANE
            // Esc belongs to the child, so there is no key here to press.
            draw(&mut app);
            super::clear_aim(&mut app);
            draw(&mut app);
            assert!(
                !has_pane(&app),
                "the pane collapsed out from under the keys"
            );
            assert_eq!(app.focus, Focus::Sessions, "which handed them back");
            assert!(!app.term_locked);
        });
    }

    /// `^~` folds the PANE away and takes the selection with it: no pane
    /// is drawn under the cards, no card wears the cursor, and the edge
    /// the pane gave the pointer to drag is gone with it. The same key
    /// brings all three back. Ctrl and a bare backtick is the second
    /// chord on the same key, for the terminals that send it unshifted.
    #[test]
    fn folding_the_pane_away_lets_the_card_go_too() {
        with_default_config(|| {
            let mut app = two_sessions();
            draw(&mut app);
            // The pane's own edge, which `ui::draw` registers only while
            // there is a pane to drag, and the rows the cards are laid
            // out over: the fold hands one to the other.
            let has_pane = |app: &App| {
                app.hits
                    .iter()
                    .any(|(_, hit)| *hit == HitTarget::LauncherPaneSplitter)
            };
            let cards_h = |app: &App| {
                app.hits
                    .iter()
                    .find(|(_, hit)| *hit == HitTarget::PanelBg(Focus::Sessions))
                    .map(|(area, _)| area.height)
                    .expect("the grid registered its background")
            };
            let pane_h = crate::launcher::pane_height(app.launcher_body, app.launcher_pane_h)
                .expect("34 rows fits a pane");
            let (was_pane, grid_was) = (has_pane(&app), cards_h(&app));
            assert!(was_pane && !app.launcher_unaimed);

            key(&mut app, KeyCode::Char('~'), KeyModifiers::CONTROL);
            assert!(app.launcher_pane_hidden, "^~ folded the pane away");
            assert!(
                app.launcher_unaimed,
                "folding the pane away let the card under the cursor go"
            );
            draw(&mut app);
            assert!(
                !has_pane(&app),
                "a folded pane leaves no edge under the pointer to drag"
            );
            assert_eq!(
                cards_h(&app),
                grid_was + pane_h,
                "the cards took the rows the pane was drawn over"
            );

            // The same key back: the pane returns, reading the card it is
            // aimed at again.
            key(&mut app, KeyCode::Char('~'), KeyModifiers::CONTROL);
            assert!(!app.launcher_pane_hidden);
            assert!(
                !app.launcher_unaimed,
                "the pane came back without the card it reads"
            );
            draw(&mut app);
            assert!(has_pane(&app));
            assert_eq!(cards_h(&app), grid_was, "the pane took its rows back");

            // The two chords beside it: the bare `~` every terminal
            // delivers, and `^`` where ctrl reports the key unshifted.
            for chord in [
                (KeyCode::Char('~'), KeyModifiers::NONE),
                (KeyCode::Char('`'), KeyModifiers::CONTROL),
            ] {
                let folded = app.launcher_pane_hidden;
                key(&mut app, chord.0, chord.1);
                assert_ne!(
                    app.launcher_pane_hidden, folded,
                    "{chord:?} did not fold the pane"
                );
                draw(&mut app);
            }
        });
    }

    /// `^`` from inside the pane hands the keys back to the card first —
    /// the pane still up, the card still selected — and only a second
    /// press folds the pane; a third brings it back.
    #[test]
    fn ctrl_backtick_steps_out_to_the_card_then_folds_the_pane() {
        with_default_config(|| {
            let mut app = two_sessions();
            draw(&mut app);
            key(&mut app, KeyCode::Enter, KeyModifiers::NONE);
            key(&mut app, KeyCode::Enter, KeyModifiers::NONE);
            assert_eq!(app.focus, Focus::Terminal);
            assert!(app.term_locked && !app.collapsed);

            let typed = |out: &[ClientRequest]| {
                out.iter().any(|r| matches!(r, ClientRequest::Input { .. }))
            };
            let out = key(&mut app, KeyCode::Char('~'), KeyModifiers::NONE);
            assert!(typed(&out), "a bare ~ is typed into the session: {out:?}");
            assert!(!app.launcher_pane_hidden && app.term_locked);

            let card = crate::launcher::cursor(&app, &crate::launcher::bands(&app));
            let out = key(&mut app, KeyCode::Char('`'), KeyModifiers::CONTROL);
            assert!(!typed(&out), "^` never reaches the session: {out:?}");
            assert_eq!(app.focus, Focus::Sessions, "the keys are the grid's");
            assert!(!app.term_locked);
            assert!(!app.launcher_pane_hidden, "the first ^` left the pane up");
            assert!(!app.launcher_unaimed, "and the card it reads selected");
            assert_eq!(
                crate::launcher::cursor(&app, &crate::launcher::bands(&app)),
                card,
                "the cursor stayed on it"
            );
            draw(&mut app);
            assert_eq!(app.focus, Focus::Sessions);

            // Again, from the cards: the pane folds away.
            key(&mut app, KeyCode::Char('`'), KeyModifiers::CONTROL);
            assert!(app.launcher_pane_hidden, "the second ^` folded the pane");
            assert_eq!(app.focus, Focus::Sessions);
            draw(&mut app);

            // And once more brings it back.
            key(&mut app, KeyCode::Char('`'), KeyModifiers::CONTROL);
            assert!(!app.launcher_pane_hidden);
        });
    }

    /// Esc lets the card go and goes no further: the grid is the top of
    /// the view, so Esc pressed again changes nothing — not the project,
    /// not the tabs — and says nothing. A card walked onto takes the aim
    /// back.
    #[test]
    fn esc_lets_the_card_go_and_goes_no_further() {
        with_default_config(|| {
            let mut app = two_sessions();
            draw(&mut app);

            keys(&mut app, &[KeyCode::Esc, KeyCode::Esc]);
            assert!(app.launcher_unaimed);
            let project = app.selected_project().map(|p| p.id.clone());

            app.flash = None;
            key(&mut app, KeyCode::Esc, KeyModifiers::NONE);
            assert!(app.launcher_unaimed);
            assert_eq!(app.selected_project().map(|p| p.id.clone()), project);
            assert_eq!(app.flash, None, "nothing moved, nothing said");

            // A step along the bands takes the aim back.
            key(&mut app, KeyCode::Char('j'), KeyModifiers::NONE);
            assert!(!app.launcher_unaimed, "a step re-aimed the grid");
        });
    }

    /// `p` lands the box in the checkout under the grid's cursor — the
    /// worktree whose band the cursor is on, or the one the grid is
    /// inside, whichever of its cards was last selected — so a prompt
    /// sent with a worktree selected starts a new session beside the ones
    /// running in it. With the aim let go (Esc off the band) it lands on
    /// the project's ROOT BRANCH. Nothing selected asks nothing either: no
    /// PROJECT PICKER goes up, the box does, and `^P` in it is still the
    /// way to another project.
    #[test]
    fn p_opens_the_box_in_the_cursors_worktree_and_on_the_root_unaimed() {
        with_default_config(|| {
            let mut app = two_sessions();
            draw(&mut app);
            let root = QuickTarget::Worktree(WorktreeId("w1".into()));
            let feat = QuickTarget::Worktree(WorktreeId("w2".into()));

            // On polish-nav, whose card runs in the `feat` worktree: the
            // box is feat's.
            super::select(&mut app, AgentId("a2".into()), &mut Vec::new());
            assert_eq!(
                app.selected_worktree().map(|w| w.branch.as_str()),
                Some("feat")
            );
            key(&mut app, KeyCode::Char('p'), KeyModifiers::NONE);
            assert_eq!(
                launch(&app).0.target,
                feat,
                "the box is the card's checkout"
            );
            app.overlay = None;

            // With feat's band open as the ACCORDION, the same: the box
            // is the checkout under the cursor either way.
            key(&mut app, KeyCode::Tab, KeyModifiers::NONE);
            assert_eq!(app.launcher_expanded, Some(WorktreeId("w2".into())));
            key(&mut app, KeyCode::Char('p'), KeyModifiers::NONE);
            assert_eq!(launch(&app).0.target, feat, "the open band's checkout");
            app.overlay = None;

            // Closed again, the band still aimed at: still feat's box, and
            // `^N` flips it onto a fresh worktree and back onto feat, not
            // onto the root.
            key(&mut app, KeyCode::Esc, KeyModifiers::NONE);
            assert!(app.launcher_expanded.is_none() && !app.launcher_unaimed);
            key(&mut app, KeyCode::Char('p'), KeyModifiers::NONE);
            assert_eq!(launch(&app).0.target, feat, "the band's checkout");
            key(&mut app, KeyCode::Char('n'), KeyModifiers::CONTROL);
            assert!(launch(&app).0.is_new_worktree());
            key(&mut app, KeyCode::Char('n'), KeyModifiers::CONTROL);
            assert_eq!(launch(&app).0.target, feat, "^N came back off feat");
            app.overlay = None;

            // Let the aim go: off the band.
            key(&mut app, KeyCode::Esc, KeyModifiers::NONE);
            assert!(app.launcher_unaimed);

            key(&mut app, KeyCode::Char('p'), KeyModifiers::NONE);
            assert!(
                !matches!(&app.overlay, Some(Overlay::ProjectPicker(_))),
                "the picker went up instead of the box"
            );
            assert_eq!(
                launch(&app).0.target,
                root,
                "the box did not land on demo's root branch"
            );

            // And it is the box itself, drawn with its own chrome.
            let text = buffer_text(&draw(&mut app));
            assert!(text.contains("new worktree ^N"), "{text}");

            // `^N` flips it onto a fresh worktree and back onto the root:
            // nothing under the cursor to come back to.
            key(&mut app, KeyCode::Char('n'), KeyModifiers::CONTROL);
            assert!(launch(&app).0.is_new_worktree());
            key(&mut app, KeyCode::Char('n'), KeyModifiers::CONTROL);
            assert_eq!(launch(&app).0.target, root, "^N came back off the root");
        });
    }

    /// `n` is not `p`: it asks which harness first — the NEW SESSION
    /// PICKER, with no box drawn behind it — and Enter on a row opens the
    /// box set to that harness, in the checkout `p` would take (the
    /// worktree under the cursor). Esc on the picker opens nothing: no
    /// box was up to come back to. A DRAFT parked by an earlier box hands
    /// its text back into the picked box, and only its text: the harness
    /// was chosen a moment ago, on purpose.
    #[test]
    fn n_picks_the_harness_first_and_opens_the_box_on_it() {
        with_default_config(|| {
            let mut app = two_sessions();
            draw(&mut app);
            let feat = QuickTarget::Worktree(WorktreeId("w2".into()));
            super::select(&mut app, AgentId("a2".into()), &mut Vec::new());

            key(&mut app, KeyCode::Char('n'), KeyModifiers::NONE);
            let Some(Overlay::Menu(menu)) = &app.overlay else {
                panic!("expected the NEW SESSION PICKER, got {:?}", app.overlay);
            };
            assert_eq!(menu.title.as_deref(), Some("New session"));
            let back = super::super::menu_quick_return(menu).expect("the rows owe a box");
            assert!(!back.from_box, "no box is up under the picker");
            assert_eq!(back.launch.target, feat, "the checkout p would take");
            let default_kind = back.launch.kind;
            let text = buffer_text(&draw(&mut app));
            assert!(
                !text.contains("what should the agent do?"),
                "a box was drawn behind the picker:\n{text}"
            );

            // Esc: the picker closes and nothing goes up in its place.
            key(&mut app, KeyCode::Esc, KeyModifiers::NONE);
            assert!(
                app.overlay.is_none(),
                "Esc put up a box nobody asked for: {:?}",
                app.overlay
            );

            // Pick a harness other than the one the box would open on:
            // the box opens set to it, empty, aimed where p would aim.
            key(&mut app, KeyCode::Char('n'), KeyModifiers::NONE);
            key(&mut app, KeyCode::Down, KeyModifiers::NONE);
            let row_kind = match &app.overlay {
                Some(Overlay::Menu(menu)) => match &menu.items[menu.hover].action {
                    crate::app::MenuAction::NewAgentOfKind { kind, .. } => *kind,
                    other => panic!("{other:?}"),
                },
                other => panic!("{other:?}"),
            };
            assert_ne!(row_kind, default_kind, "Down stayed on the default row");
            key(&mut app, KeyCode::Enter, KeyModifiers::NONE);
            let (picked, typed) = launch(&app);
            assert_eq!(picked.kind, row_kind, "the box did not take the pick");
            assert_eq!(picked.target, feat);
            assert_eq!(typed, "");

            // A parked draft aimed the same way: its text comes back, the
            // pick's harness stays, and the slot is emptied.
            app.overlay = None;
            app.quick_draft = Some(crate::quick_prompt::QuickDraft {
                launch: QuickLaunch::from_config(feat.clone(), &crate::config::Config::default()),
                input: crate::text_input::TextInput::multiline_with_text("parked words"),
            });
            key(&mut app, KeyCode::Char('n'), KeyModifiers::NONE);
            key(&mut app, KeyCode::Down, KeyModifiers::NONE);
            key(&mut app, KeyCode::Enter, KeyModifiers::NONE);
            let (picked, typed) = launch(&app);
            assert_eq!(typed, "parked words");
            assert_eq!(picked.kind, row_kind, "the parked spec overrode the pick");
            assert!(app.quick_draft.is_none(), "the draft was not taken");
        });
    }

    /// With the `quick_prompt_new_worktree` SETTING on, every box starts
    /// on a fresh worktree in the selected project instead — the card
    /// under the cursor does not matter here either.
    #[test]
    fn the_new_worktree_setting_starts_every_box_on_a_fresh_worktree() {
        with_config_json(r#"{"quick_prompt_new_worktree": true}"#, || {
            let mut app = two_sessions();
            draw(&mut app);
            for agent in ["a1", "a2"] {
                super::select(&mut app, AgentId(agent.into()), &mut Vec::new());
                key(&mut app, KeyCode::Char('p'), KeyModifiers::NONE);
                assert!(
                    matches!(
                        &launch(&app).0.target,
                        QuickTarget::NewWorktree { project, .. } if project.0 == "p1"
                    ),
                    "{agent}: {:?}",
                    launch(&app).0.target
                );
                app.overlay = None;
            }
        });
    }

    /// The unaimed box is a box like any other: `^P` over it re-aims it
    /// at another project, and the pick hands the box back rather than
    /// opening a second one.
    #[test]
    fn the_unaimed_box_can_still_be_re_aimed_with_the_picker() {
        with_default_config(|| {
            let mut app = two_sessions();
            draw(&mut app);
            key(&mut app, KeyCode::Esc, KeyModifiers::NONE);
            key(&mut app, KeyCode::Char('p'), KeyModifiers::NONE);

            key(&mut app, KeyCode::Char('p'), KeyModifiers::CONTROL);
            let Some(Overlay::ProjectPicker(picker)) = &app.overlay else {
                panic!("expected the project picker, got {:?}", app.overlay);
            };
            assert!(picker.back.from_box, "the box under it was forgotten");
            let to = picker
                .matches
                .iter()
                .position(|(i, _)| picker.projects[*i].name == "web")
                .expect("web is on the list");
            for _ in 0..to {
                key(&mut app, KeyCode::Down, KeyModifiers::NONE);
            }
            key(&mut app, KeyCode::Enter, KeyModifiers::NONE);
            let (launch, _) = launch(&app);
            assert_eq!(
                crate::launcher::project_of(&app, &launch.target),
                Some(ProjectId("p2".into())),
                "the pick is what re-aimed the box"
            );
        });
    }

    /// A cell of the grid's background — the air between the cards, which
    /// falls through to the `PanelBg` the grid registers under them.
    fn air(app: &App) -> (u16, u16) {
        let (area, _) = *app
            .hits
            .iter()
            .find(|(_, hit)| *hit == HitTarget::PanelBg(Focus::Sessions))
            .expect("the grid registered its background");
        for y in area.y..area.y + area.height {
            for x in area.x..area.x + area.width {
                if app.hit_at(x, y) == Some(HitTarget::PanelBg(Focus::Sessions)) {
                    return (x, y);
                }
            }
        }
        panic!("the cards covered the whole grid");
    }

    /// The color of a card's top-left corner — its border, which is the
    /// accent while it wears the cursor and the frame's own edge when it
    /// does not.
    fn corner(terminal: &Terminal<TestBackend>, cell: ratatui::layout::Rect) -> Color {
        terminal
            .backend()
            .buffer()
            .cell((cell.x, cell.y))
            .expect("the card is on screen")
            .fg
    }

    /// A project's place in the PROJECTS PANEL's row order.
    fn project_row(app: &App, id: &str) -> usize {
        app.project_rows()
            .iter()
            .position(|i| app.tree.projects[*i].id.0 == id)
            .expect("the project has a row")
    }

    // ---- the compact LIST ----

    /// [`two_sessions`] in the compact LIST, with four more sessions in
    /// `demo`'s root: its band holds five, the `feat` band one.
    fn list_of_five() -> App {
        let mut app = two_sessions();
        app.launcher_list = true;
        for (id, name) in [
            ("a4", "second"),
            ("a5", "third"),
            ("a6", "fourth"),
            ("a7", "fifth"),
        ] {
            seed_running(&mut app, id, "w1", name);
        }
        app
    }

    /// The cards of `band` the frame just drew, as one-line entries, by
    /// their index in the band, top to bottom.
    fn drawn_entries(app: &App, band: usize) -> Vec<(usize, ratatui::layout::Rect)> {
        let mut out: Vec<(usize, ratatui::layout::Rect)> = app
            .hits
            .iter()
            .filter_map(|(r, h)| match h {
                HitTarget::LauncherCard(c) if c.band == band => Some((c.card, *r)),
                _ => None,
            })
            .collect();
        out.sort_by_key(|(_, r)| r.y);
        out
    }

    fn cursor_card(app: &App) -> Option<crate::launcher::CardRef> {
        crate::launcher::cursor(app, &crate::launcher::bands(app))
    }

    /// Settings → Appearance → **Worktree layout** reaches the app the way
    /// every setting does (`apply_config`): `list` is the LIST, anything
    /// else the cards.
    #[test]
    fn the_worktree_layout_setting_turns_the_list_on_and_off() {
        with_config_json(r#"{"worktree_layout": "list"}"#, || {
            let mut app = two_sessions();
            super::super::apply_config(&mut app, &crate::config::Config::load());
            assert!(app.launcher_list);
        });
        with_default_config(|| {
            let mut app = two_sessions();
            app.launcher_list = true;
            super::super::apply_config(&mut app, &crate::config::Config::load());
            assert!(!app.launcher_list, "the cards out of the box");
        });
    }

    /// In the LIST a worktree stacks its sessions a line apiece, and
    /// collapsed — as every band starts — shows only its three most
    /// recent, the line under them counting the rest: `▾ 2 more · Tab:
    /// see all 5`. Tab opens the band to every session, and again folds
    /// it back to three. A band with nothing left off has no such line.
    #[test]
    fn the_list_shows_three_recent_sessions_until_tab_opens_the_rest() {
        with_default_config(|| {
            let mut app = list_of_five();
            let bands = crate::launcher::bands(&app);
            assert_eq!(bands[0].cards.len(), 5);
            // The cursor on the newest: on the oldest, the band would list
            // it too (`the_cursors_session_stays_listed_on_a_collapsed_band`).
            super::select_card(&mut app, bands[0].cards[0].sref(), &mut Vec::new());
            let term = draw(&mut app);
            assert_eq!(app.launcher_expanded, None, "collapsed by default");
            let entries = drawn_entries(&app, 0);
            assert_eq!(
                entries.iter().map(|(i, _)| *i).collect::<Vec<_>>(),
                vec![0, 1, 2],
                "the three most recent, newest first"
            );
            for (_, r) in &entries {
                assert_eq!(r.height, 1, "one line an entry");
            }
            assert!(
                entries.windows(2).all(|w| w[1].1.y == w[0].1.y + 1),
                "stacked one under the other: {entries:?}"
            );
            let hint = app
                .hit_rect(&HitTarget::LauncherBandMore(0))
                .expect("a line counting the rest");
            assert_eq!(hint.y, entries[2].1.y + 1, "right under the last entry");
            let buf = term.backend().buffer();
            let text: String = (hint.x..hint.x + hint.width)
                .map(|x| buf[(x, hint.y)].symbol().to_string())
                .collect();
            assert!(text.contains("▾ 2 more"), "{text:?}");
            assert!(text.contains("see all 5"), "{text:?}");
            assert_eq!(drawn_entries(&app, 1).len(), 1, "feat's one session");
            assert!(app.hit_rect(&HitTarget::LauncherBandMore(1)).is_none());
            let screen = buffer_text(&term);
            for name in [
                bands[0].cards[0].name(),
                bands[0].cards[2].name(),
                "polish-nav",
            ] {
                assert!(screen.contains(name), "{name} is listed:\n{screen}");
            }
            assert!(
                !screen.contains(bands[0].cards[4].name()),
                "the oldest waits behind Tab:\n{screen}"
            );

            key(&mut app, KeyCode::Tab, KeyModifiers::NONE);
            assert_eq!(app.launcher_expanded.as_ref(), Some(&bands[0].worktree));
            let screen = buffer_text(&draw(&mut app));
            assert_eq!(drawn_entries(&app, 0).len(), 5, "every session");
            assert!(screen.contains(bands[0].cards[4].name()), "{screen}");
            assert!(app.hit_rect(&HitTarget::LauncherBandMore(0)).is_none());

            key(&mut app, KeyCode::Tab, KeyModifiers::NONE);
            assert_eq!(app.launcher_expanded, None, "Tab folds it back");
            draw(&mut app);
            assert_eq!(drawn_entries(&app, 0).len(), 3);
        });
    }

    /// `j` / `k` walk the LIST's lines as one column: down the band's
    /// three, past the ones it leaves off, onto the next band's first
    /// line — and `k` from there back onto the last line above it.
    #[test]
    fn j_and_k_walk_the_list_lines_across_worktrees() {
        with_default_config(|| {
            let mut app = list_of_five();
            let bands = crate::launcher::bands(&app);
            draw(&mut app);
            super::select_card(&mut app, bands[0].cards[0].sref(), &mut Vec::new());
            let at = |band, card| Some(crate::launcher::CardRef { band, card });
            for want in [at(0, 1), at(0, 2), at(1, 0), at(1, 0)] {
                key(&mut app, KeyCode::Char('j'), KeyModifiers::NONE);
                draw(&mut app);
                assert_eq!(cursor_card(&app), want);
            }
            key(&mut app, KeyCode::Char('k'), KeyModifiers::NONE);
            assert_eq!(cursor_card(&app), at(0, 2), "onto the last line above");
        });
    }

    /// The card the pane reads is always a line on screen: with the
    /// cursor on one the collapsed band leaves off — the palette jumped
    /// there, or the list reordered under it — the band lists it after
    /// its three, and counts one fewer left off.
    #[test]
    fn the_cursors_session_stays_listed_on_a_collapsed_band() {
        with_default_config(|| {
            let mut app = list_of_five();
            let bands = crate::launcher::bands(&app);
            super::select_card(&mut app, bands[0].cards[4].sref(), &mut Vec::new());
            let term = draw(&mut app);
            let entries = drawn_entries(&app, 0);
            assert_eq!(
                entries.iter().map(|(i, _)| *i).collect::<Vec<_>>(),
                vec![0, 1, 2, 4]
            );
            let hint = app.hit_rect(&HitTarget::LauncherBandMore(0)).unwrap();
            let buf = term.backend().buffer();
            let text: String = (hint.x..hint.x + hint.width)
                .map(|x| buf[(x, hint.y)].symbol().to_string())
                .collect();
            assert!(text.contains("▾ 1 more"), "{text:?}");
            let row: String = (0..130)
                .map(|x| buf[(x, entries[3].1.y)].symbol().to_string())
                .collect();
            assert!(
                row.contains("▌ "),
                "the cursor's line wears the mark: {row:?}"
            );
        });
    }

    /// Tab on a LIST band that already lists every session opens nothing
    /// — there is nothing more to see — and says so.
    #[test]
    fn tab_on_a_list_band_with_nothing_left_off_says_so() {
        with_default_config(|| {
            let mut app = list_of_five();
            let bands = crate::launcher::bands(&app);
            draw(&mut app);
            super::select_card(&mut app, bands[1].cards[0].sref(), &mut Vec::new());
            key(&mut app, KeyCode::Tab, KeyModifiers::NONE);
            assert_eq!(app.launcher_expanded, None);
            assert_eq!(app.flash.as_deref(), Some(super::ALL_LISTED));
        });
    }

    /// INPUT PARITY: a click on a LIST line lands the cursor on its card,
    /// as `j` walking onto it does, and a click on the `▾ 2 more` line
    /// opens the band as Tab does.
    #[test]
    fn a_click_on_a_list_line_is_the_walk_and_on_the_more_line_is_tab() {
        with_default_config(|| {
            let mut app = list_of_five();
            let bands = crate::launcher::bands(&app);
            draw(&mut app);
            let (card, line) = drawn_entries(&app, 0)[1];
            click_at(&mut app, line.x + 6, line.y);
            assert_eq!(
                cursor_card(&app),
                Some(crate::launcher::CardRef { band: 0, card })
            );
            let mut by_key = list_of_five();
            draw(&mut by_key);
            super::select_card(&mut by_key, bands[0].cards[0].sref(), &mut Vec::new());
            key(&mut by_key, KeyCode::Char('j'), KeyModifiers::NONE);
            assert_eq!(cursor_card(&by_key), cursor_card(&app));

            draw(&mut app);
            let hint = app.hit_rect(&HitTarget::LauncherBandMore(0)).unwrap();
            click_at(&mut app, hint.x + 1, hint.y);
            assert_eq!(app.launcher_expanded.as_ref(), Some(&bands[0].worktree));
        });
    }
}
