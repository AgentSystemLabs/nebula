//! The LAUNCHER VIEW's keys and clicks (`crate::launcher` is the view's
//! model, `ui::launcher_view` its drawing). Every arm here translates and
//! calls one function per intent, as the panels' do: a card chosen by key
//! or by pointer lands through [`select`], a session is stepped into
//! through [`enter_pane`] (full-screen through [`open_session`]), and the
//! box opens through [`open_box`] — so `j` and a click on the card below,
//! or Enter and a double-click, end in the same state.

use super::{
    build_submenu, enter_terminal_pane, is_double_click, jump_to_target, jump_to_target_inner,
    open_prompt, restore_project_cursors, select_project_row_by_id, Landing,
};
use crate::app::{
    App, ContextMenu, Focus, HitTarget, MenuAction, MenuFilter, MenuItem, Overlay, PromptKind,
};
use crate::keymap::{Action, KeyChord};
use crate::launcher::{self as view, BoxField, ProjectPicker};
use crate::palette::PaletteTarget;
use crate::quick_prompt::{QuickLaunch, QuickReturn, QuickTarget};
use crate::text_input::TextInput;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use nebula_core::{AgentId, ClientRequest, ProjectId, SessionRef, TerminalId};

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
/// too, so the same `p` that takes the root branch after an Esc takes it
/// after this.
const PANE_HIDDEN: &str = "pane hidden, nothing selected — ^` brings it back";
const PANE_SHOWN: &str = "pane back under the cards";

/// What the fold key did as the first of its two presses from inside the
/// PANE ([`fold_key`]), for the KEY COMBO DISPLAY.
const BACK_TO_CARD: &str = "Back to the card";

/// What letting the card under the cursor go says: nothing in the GRID is
/// selected any more, so the box `p` opens has no checkout to read off a
/// card and lands on the project's ROOT BRANCH instead.
pub(super) const UNAIMED: &str = "nothing selected — p starts on the project's root branch";

/// The box: the QUICK PROMPT, aimed at the project under the list's cursor
/// (the selected project). The `quick_prompt_new_worktree` SETTING alone
/// picks a fresh worktree or an existing checkout; `^N` flips only the box
/// that is up. `p`, `n`, and the boot.
///
/// On an existing checkout with the aim let go of ([`clear_aim`]) there is
/// no card to read one off, so the box lands on the project's ROOT BRANCH
/// (`view::root_target_for`).
pub(super) fn open_box(app: &mut App) {
    let project = app.selected_project().map(|p| p.id.clone()).or_else(|| {
        app.project_rows()
            .first()
            .and_then(|i| app.tree.projects.get(*i))
            .map(|p| p.id.clone())
    });
    let Some(project) = project else {
        app.flash = Some("add a project first".into());
        return;
    };
    let target = if crate::config::Config::load().quick_prompt_new_worktree {
        view::target_for(app, &project, true)
    } else if app.launcher_unaimed {
        view::root_target_for(app, &project)
    } else {
        view::target_for(app, &project, false)
    };
    crate::quick_prompt::open_for(app, target);
}

/// Put the cursor back on a card: anything that lands on one — a key that
/// walks the grid, a click on a card, a project opened — takes the aim
/// back, so the box reads a checkout again and the PANE along the bottom
/// opens on that card ([`App::launcher_split`]). Only the aim: the keys
/// stay on the cards, and crossing into the pane is a second click or
/// Enter ([`enter_pane`]).
pub(super) fn take_aim(app: &mut App) {
    if app.launcher_unaimed {
        app.launcher_unaimed = false;
        app.dirty = true;
    }
}

/// Let the card under the cursor go: no card is drawn wearing the cursor,
/// the box has no checkout to read off one, and the PANE along the bottom
/// collapses — it is the selected session, so with nothing selected there
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
pub(super) fn toggle_pane(app: &mut App) {
    app.launcher_pane_hidden = !app.launcher_pane_hidden;
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

// ---- the PANE's TAB STRIP ----

/// What `` ` `` says with nothing to walk to: the checkout has no
/// terminals, so SESSION is the only tab on the strip — which says as
/// much itself, naming the key that opens one.
const NO_TERMINALS: &str = "no terminals in this checkout — t opens one";

/// What it says over a full-screen session, which has no strip to walk.
pub(super) const NO_PANE_HERE: &str = "the pane's terminals are under the grid — ^q back to it";

/// Read `tab` in the PANE along the bottom: None is the SESSION tab —
/// the card under the cursor — and Some names one of the checkout's
/// TERMINALS (`ui::launcher_view::pane_frame` draws the strip).
///
/// INPUT PARITY: the one function behind every way the strip is walked —
/// a click on a tab (`event_loop`'s `LauncherPaneSession` and
/// `LauncherPaneTerminal` arms) and the `` ` `` key ([`cycle_pane_tab`])
/// — so the pointer and the key end in the same state.
///
/// The attach is immediate: a tab was named outright, so there is
/// nothing to wait to see whether it was meant.
pub(super) fn show_pane_tab(app: &mut App, tab: Option<TerminalId>, out: &mut Vec<ClientRequest>) {
    // A folded pane has nothing to read ([`toggle_pane`]): bring it back,
    // or the tab would be swapped behind a pane that is not drawn. A
    // pane collapsed for want of a card under the cursor ([`clear_aim`])
    // comes back the same way.
    if app.launcher_pane_hidden {
        toggle_pane(app);
    }
    take_aim(app);
    app.launcher_terminal = tab.clone();
    app.dirty = true;
    match tab {
        Some(id) => super::attach_now(app, SessionRef::Terminal(id), out),
        // Back onto the card under the cursor, found the same way every
        // other walk of the grid finds it — the pin is already off, so
        // the preview reads the session rather than the terminal.
        None => super::preview_selected_now(app, out),
    }
}

/// `` ` ``: walk the strip — SESSION, then each TERMINAL of the checkout
/// in the order the header lists them, then back to SESSION. The key
/// walks exactly the tabs the pointer can click, so it never lands on
/// something that is not on screen.
pub(super) fn cycle_pane_tab(app: &mut App, out: &mut Vec<ClientRequest>) {
    let terminals = app.visible_terminals();
    if terminals.is_empty() {
        app.flash = Some(NO_TERMINALS.into());
        return;
    }
    let at = app
        .pinned_terminal()
        .and_then(|id| terminals.iter().position(|t| t.id == id));
    let next = match at {
        None => Some(terminals[0].id.clone()),
        // Past the last tab is back to SESSION, so the walk is a loop
        // rather than a dead end.
        Some(i) => terminals.get(i + 1).map(|t| t.id.clone()),
    };
    show_pane_tab(app, next, out);
}

/// A click on the TERMINAL tab at `index` — its place in
/// `App::visible_terminals`, which is the order the strip draws. A stale
/// index (the terminal closed between the draw and the click) is left
/// alone rather than reading whatever slid into the slot.
pub(super) fn click_pane_tab(app: &mut App, index: usize, out: &mut Vec<ClientRequest>) {
    let Some(id) = app.visible_terminals().get(index).map(|t| t.id.clone()) else {
        return;
    };
    show_pane_tab(app, Some(id), out);
}

/// Close the TERMINAL at `index` on the strip — the `×` on its tab, and
/// `d` with the pane on it ([`close_pinned_terminal`]). Both put up the
/// CONFIRM the SESSIONS PANEL's own `d` puts up on a terminal row, so
/// one wording covers every way a shell is killed.
pub(super) fn close_pane_tab(app: &mut App, index: usize) {
    let Some(t) = app.pane_terminals().into_iter().nth(index) else {
        return;
    };
    app.overlay = Some(Overlay::Confirm(super::confirm_close_terminal(
        &t.name,
        t.id.clone(),
    )));
}

/// `d` while the pane is on a TERMINAL: close that terminal rather than
/// delete the card under the cursor. The strip is what the keys aimed at
/// what the pane holds mean while it is on one — as Enter and `z` are
/// ([`enter_pane`], [`open_session`]) — and the card is still there to
/// delete once the strip is back on SESSION.
pub(super) fn close_pinned_terminal(app: &mut App) {
    let Some(id) = app.pinned_terminal() else {
        return;
    };
    let at = app.pane_terminals().iter().position(|t| t.id == id);
    if let Some(at) = at {
        close_pane_tab(app, at);
    }
}

/// Which tab the strip lands on when `closing` goes: the tab after it,
/// else the one before it, else SESSION (the inner None). The outer None
/// means the strip was not on that terminal at all and must be left
/// alone — which is every close made from the panels, where there is no
/// strip.
///
/// Read BEFORE the row is dropped, since it is the row's neighbours that
/// answer it.
pub(super) fn tab_after(app: &App, closing: &TerminalId) -> Option<Option<TerminalId>> {
    if app.pinned_terminal().as_ref() != Some(closing) {
        return None;
    }
    let terminals = app.pane_terminals();
    let at = terminals.iter().position(|t| &t.id == closing)?;
    Some(
        terminals
            .get(at + 1)
            .or_else(|| at.checked_sub(1).and_then(|before| terminals.get(before)))
            .map(|t| t.id.clone()),
    )
}

/// Esc in the GRID: let the card under the cursor go ([`clear_aim`]) —
/// the selection the box reads, so the next `p` starts its session on the
/// project's root branch instead of in whichever checkout the cursor was
/// parked in. With nothing selected already there is nothing left to let
/// go of, and Esc does nothing: the grid is the top of the view.
///
/// With the PROJECT TABS holding the keys ([`focus_tabs`]) Esc is only
/// the way back down: the cards get the keys again, on the project the
/// header's cursor walked to and the card it shows.
pub(super) fn escape(app: &mut App) {
    if app.launcher_tab_cursor.is_some() {
        leave_tabs(app);
    } else if !app.launcher_unaimed {
        clear_aim(app);
    }
}

/// A panel key while the GRID is up — true when the view took it: `h` and
/// `l` walk a row of cards, `j` and `k` the column under the cursor, the
/// ways into a session step down into the PANE along the bottom (`z`
/// full-screens it instead), `p` / `n` open the box, the PROJECT TABS
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
        // Enter, Tab and ^→ cross into the PANE along the bottom, where
        // the card's session is already running and reading it only takes
        // the keys — the walk into the pane Tab is out of the panels, with
        // the grid left up over it. `z` is the one that gives that session
        // the whole screen, exactly as it full-screens the pane there.
        Action::Activate | Action::FocusNext | Action::FocusTerminal => enter_pane(app, out),
        Action::Zoom => open_session(app, out),
        // There is nothing to the left of the grid to walk back to.
        Action::FocusPrev => {}
        // The pane's TAB STRIP: what the pane READS, where `^~` is
        // whether it is drawn at all.
        Action::PaneTabs => cycle_pane_tab(app, out),
        // And `d` on a strip that is on a terminal closes that terminal,
        // not the card under the cursor.
        Action::Delete if app.pinned_terminal().is_some() => close_pinned_terminal(app),
        Action::New | Action::QuickPrompt => open_box(app),
        // `⇧A` swaps the grid for the project's archived sessions, and
        // back. It is the grid's own key here rather than the panels'
        // group fold: there is no ARCHIVED group to open, only the other
        // list of cards.
        Action::ToggleArchived => toggle_archived(app, out),
        // Space on a card: its FOLLOW-UP MODAL, a box over the grid —
        // the card itself has no room to grow one, and the pane stays
        // exactly as it is.
        Action::FollowUp => follow_up(app),
        // `m` with no card selected — the aim let go of, or a project with
        // no sessions yet — is the PROJECT's menu: there is no session
        // under the cursor for it to be the menu of.
        Action::ContextMenu if app.launcher_unaimed || view::rows(app).is_empty() => {
            project_menu(app, super::KEYBOARD_MENU_ANCHOR)
        }
        // The fold is a preference the view keeps, and its key is the way
        // out of the pane first ([`fold_key`]). `⇧Z` / `^B` is the same
        // fold under its old "give it the full width" name, since the pane
        // is the only thing left to fold.
        Action::ToggleLauncherPane => {
            fold_key(app);
        }
        Action::ToggleSidebars => toggle_pane(app),
        // The PROJECT TABS across the header.
        Action::NextProjectTab => step_tab(app, 1, out),
        Action::PrevProjectTab => step_tab(app, -1, out),
        Action::CloseProjectTab => close_active_tab(app, out),
        Action::SelectProjectTab(n) => open_tab_slot(app, n, out),
        // `⌘P`: the list the header's `+` drops, the click's own
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

/// The PROJECT's own menu — `m` with no card selected, and a right-click
/// on its PROJECT TAB ([`tab_menu`]): what the PROJECTS and WORKTREES
/// panels' rows carried between them — a checkout cut in it, the project
/// renamed or dropped, and the RUN COMMAND of the checkout the grid would
/// launch into started or stopped. These have no key of their own on the
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

/// Cards per row as the last frame drew them — the geometry the keys and
/// the drawing share, so `j` moves by exactly one row of cards. One before
/// the first draw, which walks the grid as a list until the body is known.
fn cols(app: &App) -> usize {
    crate::launcher::grid(app.body_area).cols
}

/// `h` / `j` / `k` / `l` (and the half-page jumps): the cursor `dx` cards
/// along its row and `dy` rows down the grid.
pub(super) fn step_grid(app: &mut App, dx: i64, dy: i64, out: &mut Vec<ClientRequest>) {
    let rows = view::rows(app);
    let at = view::cursor(app, &rows);
    let Some(next) = view::grid_stepped(at, dx, dy, cols(app), rows.len()) else {
        app.flash = Some(nothing_here(app).into());
        return;
    };
    if Some(next) == at {
        return;
    }
    select(app, rows[next].agent.id.clone(), out);
}

/// `k` (↑): a row up the grid — and on the top row, where there is no
/// row above, the edge of a DOUBLE TAP: the first press stays put and
/// says what a second one does, the second walks up into the PROJECT
/// TABS ([`focus_tabs`]). A grid with no cards on it is all top row.
/// Only `k` itself: `^u`'s half page stops against the top like any
/// other edge, so leaning on it never lands in the header.
fn step_up(
    app: &mut App,
    armed: Option<(Action, std::time::Instant)>,
    chord: &KeyChord,
    out: &mut Vec<ClientRequest>,
) {
    let rows = view::rows(app);
    let top = match view::cursor(app, &rows) {
        Some(at) => at < cols(app),
        None => rows.is_empty(),
    };
    if !top {
        step_grid(app, 0, -1, out);
    } else if super::double_tapped(app, Action::MoveUp, armed, chord, "project tabs") {
        focus_tabs(app);
    }
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
/// What closing the only tab says: it is the project on screen, and there
/// is no tab beside it to hand the grid to.
const LAST_TAB: &str = "the only project open stays open — + opens another first";

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
/// tap back down the way `k`,`k` came up — and `x` closes it. `k` has nowhere higher to go. Any other key hands the keys
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

/// `x` with the PROJECT TABS holding the keys: close the tab the header's
/// cursor is on, through the same [`close_tab`] the `×` takes, and put
/// the cursor on the tab that slid into its place. The last tab is
/// refused there, and the cursor stays on it.
fn close_cursor_tab(app: &mut App, on: &ProjectId, out: &mut Vec<ClientRequest>) {
    let at = open_tabs(app).iter().position(|t| t == on);
    close_tab(app, on, out);
    let tabs = open_tabs(app);
    if tabs.contains(on) {
        return;
    }
    let next = at
        .and_then(|at| tabs.get(at.min(tabs.len().saturating_sub(1))))
        .cloned();
    app.launcher_tab_cursor = next;
    app.dirty = true;
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
/// other tab moves nothing. The only tab left is refused: it is the
/// project on screen, and there is no tab to hand the grid to — the `+`
/// opens another first.
pub(super) fn close_tab(app: &mut App, id: &ProjectId, out: &mut Vec<ClientRequest>) {
    let mut tabs = open_tabs(app);
    let Some(at) = tabs.iter().position(|t| t == id) else {
        return;
    };
    if tabs.len() == 1 {
        app.flash = Some(LAST_TAB.into());
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

/// The PROJECT DROPDOWN: a click on the `+` after the PROJECT TABS drops
/// every project on this machine under it ([`view::project_cards`]) —
/// the ones wanting a human first, then the most recently worked in —
/// the one in front of you ticked, each with how many sessions it holds.
/// Enter on a row opens that project's sessions ([`open_project`]) and,
/// if it had none, a tab for it at the far left. The last row opens a
/// folder that is not a project yet — the same prompt `o` opens — so the
/// `+` is the one place to reach for any project, known or not.
///
/// `⌘P` ([`Action::ProjectDropdown`]) drops the same list from the
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
    let open = app.selected_project().map(|p| p.id.clone());
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
    let at = crumb_anchor(app, &HitTarget::LauncherTabAdd);
    app.overlay = Some(Overlay::Menu(ContextMenu {
        title: Some("Project".into()),
        filter: Some(MenuFilter {
            query: String::new(),
            all: items.clone(),
        }),
        items,
        at: Some(at),
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
        None => fold_empty_grid(app),
    }
}

/// Nothing to aim at: with no card the PANE along the bottom has no
/// session to be, so it folds and the grid takes the body — the hero, and
/// the word for how to fill it.
///
/// INPUT PARITY: the one landing for a grid with no cards, whether a
/// project with none was opened ([`open_project`]) or the last card was
/// archived or deleted out of it ([`keep_cursor`]).
fn fold_empty_grid(app: &mut App) {
    let word = nothing_here(app);
    clear_aim(app);
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

/// A click on the card at `index`: the cursor lands there, which opens the
/// PANE along the bottom on that session ([`point_at`]) without taking the
/// keys off the cards — the session is there to read, not yet to type
/// into. A second click on the same card is Enter — down into the pane,
/// where that session is already running.
pub(super) fn click_row(app: &mut App, index: usize, out: &mut Vec<ClientRequest>) {
    let Some(id) = point_at(app, index, out) else {
        return;
    };
    if is_double_click(
        &mut app.last_session_click,
        crate::app::RowKey::Session(nebula_core::SessionRef::Agent(id)),
    ) {
        enter_pane(app, out);
    }
}

/// A right-click's first half, `select_clicked_row`'s arm: the cursor on
/// the card at `index` and the pane open on it, as a left click leaves
/// them. False off the grid.
pub(super) fn select_row(app: &mut App, index: usize, out: &mut Vec<ClientRequest>) -> bool {
    point_at(app, index, out).is_some()
}

/// The pointer landing on the card at `index`, either button: the cursor
/// goes there and the PANE along the bottom opens on it — ALWAYS, folded
/// away (`^~`) or not. A card clicked is a card to read, so the fold only
/// lasts while the grid is walked with the keys; a click unfolds it the
/// way Enter and naming a TAB do ([`enter_pane`], [`show_pane_tab`]).
/// None off the grid.
fn point_at(app: &mut App, index: usize, out: &mut Vec<ClientRequest>) -> Option<AgentId> {
    let id = view::agent_at(app, index)?;
    if app.launcher_pane_hidden {
        toggle_pane(app);
    }
    select(app, id.clone(), out);
    Some(id)
}

/// The card under the cursor ahead of an input event, for [`keep_cursor`]:
/// where it sat in the grid, its session, and the project whose cards the
/// grid was listing.
pub(super) struct CursorCard {
    index: usize,
    id: AgentId,
    /// Its neighbours by id — the card before it in the grid and the one
    /// after — so [`keep_cursor`] lands on the card the eye is on rather
    /// than on whatever the arithmetic `index - 1` points at once the
    /// list has moved. None at the ends of the list.
    before: Option<AgentId>,
    after: Option<AgentId>,
    /// The grid's project. A switch to another one takes every card off
    /// the list at once, which [`keep_cursor`] has to tell from this one
    /// card leaving it.
    project: Option<ProjectId>,
}

pub(super) fn cursor_entry(app: &App) -> Option<CursorCard> {
    let rows = view::rows(app);
    let at = view::cursor(app, &rows)?;
    let id_at = |i: usize| rows.get(i).map(|row| row.agent.id.clone());
    Some(CursorCard {
        index: at,
        id: rows[at].agent.id.clone(),
        before: at.checked_sub(1).and_then(id_at),
        after: id_at(at + 1),
        project: app.selected_project().map(|p| p.id.clone()),
    })
}

/// After an input event: the session the cursor was on left the list —
/// archived (`a`), deleted (`d`, its confirm), from the list or a menu.
/// The cursor takes the card BEFORE it, the one the archive was walked
/// onto from, with the pane on it; the card that slid into the slot when
/// the one that left was the first, since there is nothing before that.
///
/// The grid settles this itself rather than leaving it to the PANELS'
/// own reseat (`reconcile_selection`), which runs on the same archive:
/// their list is one checkout's, in tree order, while this one is the
/// whole project's, ordered by recency — so the neighbor they hand the
/// cursor to is some card elsewhere in the grid, and taking it threw the
/// cursor across the screen.
///
/// The LAST card leaving has no neighbor to take: the pane folds away with
/// it ([`fold_empty_grid`]), as it does on a project opened with none.
pub(super) fn keep_cursor(app: &mut App, before: CursorCard, out: &mut Vec<ClientRequest>) {
    // Not this list any more: the grid has another project's cards up.
    // Every card left it, this one with them, and whatever put the cursor
    // on one of the new ones meant to.
    if app.selected_project().map(|p| p.id.clone()) != before.project
        // A create is still being followed onto its own row (`n`, the
        // box): that landing is the one the user asked for.
        || app.select_when_seen.is_some()
    {
        return;
    }
    let rows = view::rows(app);
    if rows.is_empty() {
        fold_empty_grid(app);
        return;
    }
    if rows.iter().any(|row| row.agent.id == before.id) {
        return;
    }
    // Named neighbours, not arithmetic: the card that WAS before this one
    // is found wherever the list now holds it. `index - 1` was a card
    // wide of it whenever anything else moved in the same breath — a row
    // arriving, a turn starting and re-sorting the grid, the launch pin
    // dropping — and the cursor then landed a card past the one the eye
    // was on. Nothing before it (the first card): the card that slid up
    // into the slot, since there is nothing before that. Neither still
    // listed: the slot the card left, clamped to the list.
    let at = |id: &Option<AgentId>| {
        let id = id.as_ref()?;
        rows.iter().position(|row| &row.agent.id == id)
    };
    let next = at(&before.before)
        .or_else(|| at(&before.after))
        .unwrap_or_else(|| before.index.saturating_sub(1).min(rows.len() - 1));
    tracing::debug!(
        left = %before.id.0,
        was_at = before.index,
        lands_on = %rows[next].agent.id.0,
        "launcher grid: the cursor's card left the list"
    );
    if view::cursor(app, &rows) == Some(next) {
        return;
    }
    select(app, rows[next].agent.id.clone(), out);
}

/// Enter (or Tab, or `^→`, or a double-click): the session under the
/// cursor in the PANE along the bottom, with the input lock on — focus
/// crosses into the pane where it stands, the grid still up over it, and
/// `^`` ([`fold_key`]) hands the keys back to the cards, exactly as it
/// does after a click into the pane. `z` ([`open_session`]) is the
/// way to the whole screen.
///
/// The jump attaches the card outright, so a card the pane's debounce had
/// not reached yet is the one the keys reach.
pub(super) fn enter_pane(app: &mut App, out: &mut Vec<ClientRequest>) {
    // A pane folded away (`^~`) is brought back rather than stepped over,
    // the way naming a TAB brings it back ([`show_pane_tab`]): opening a
    // session here means the pane under the cards, so asking for one
    // unfolds it instead of taking the whole screen out from under the
    // grid — `z` ([`open_session`]) is the only way to that. With the
    // pane already drawn this changes nothing and focus lands in it
    // below, so a second ask, with the keys already there, is a no-op.
    if app.launcher_pane_hidden {
        toggle_pane(app);
    }
    // Nor is a pane collapsed for want of a card under the cursor
    // ([`clear_aim`] — Esc, a click on the air) stepped over: the aim
    // comes back first, so the pane opens on the card Enter is about to
    // cross into instead of the keys landing on nothing.
    take_aim(app);
    // A body too short for a pane worth the name draws none ([`has_pane`]):
    // there is nothing under the cards to cross into, so the session takes
    // the whole screen rather than the keys going somewhere off-screen.
    if !has_pane(app) {
        open_session(app, out);
        return;
    }
    // The pane may be reading a TERMINAL rather than the card under the
    // cursor (its TAB STRIP): Enter steps into what is on screen, not
    // into something the strip would have to be walked off first.
    if let Some(id) = app.pinned_terminal() {
        super::attach_now(app, SessionRef::Terminal(id), out);
        enter_terminal_pane(app, out);
        return;
    }
    let Some(id) = cursor_or_first(app) else {
        app.flash = Some(nothing_here(app).into());
        return;
    };
    // An ARCHIVED card has no session to read: the daemon reaped it when
    // it was archived. Say what to press rather than handing the keys to
    // an empty pane.
    if is_archived(app, &id) {
        app.flash = Some(super::AGENT_ARCHIVED.into());
        return;
    }
    take_aim(app);
    jump_to_target(app, PaletteTarget::Session(id), Landing::Attach, out);
    // A Cloud row's Enter is its browser page, not a PTY: the jump has
    // already opened it and there is nothing to type into.
    if app.term.is_some() {
        enter_terminal_pane(app, out);
    }
}

/// `z`: the session under the cursor full-screen — the grid and its pane
/// give way to the PTY with the input lock on, exactly as `z`
/// (`Action::Zoom`) full-screens the pane out of the panels, and `^q`
/// comes back to the grid. The jump attaches the card outright, so a card
/// the pane's debounce had not reached yet is the one that comes up.
pub(super) fn open_session(app: &mut App, out: &mut Vec<ClientRequest>) {
    // As with Enter ([`enter_pane`]): `z` full-screens whatever the pane
    // is reading, which is the pinned TERMINAL when the strip is on one.
    if let Some(id) = app.pinned_terminal() {
        super::attach_now(app, SessionRef::Terminal(id), out);
        super::zoom_pane(app, out);
        return;
    }
    let Some(id) = cursor_or_first(app) else {
        app.flash = Some(nothing_here(app).into());
        return;
    };
    if is_archived(app, &id) {
        app.flash = Some(super::AGENT_ARCHIVED.into());
        return;
    }
    take_aim(app);
    jump_to_target(app, PaletteTarget::Session(id), Landing::Attach, out);
    // A Cloud row's Enter is its browser page, not a PTY: the jump has
    // already opened it and there is nothing to full-screen.
    if app.term.is_some() {
        super::zoom_pane(app, out);
    }
}

/// Is the PANE along the bottom on screen? False with the pane folded
/// away (`^~`), on a body too short to hold the header, a row of cards
/// and a pane worth the name, where a session is only ever seen
/// full-screen — and before the first draw, which no key beats.
fn has_pane(app: &App) -> bool {
    app.launcher_split(app.launcher_body).1.is_some()
}

/// The card under the cursor, or the first one when it is on none — the
/// pane shows no session until a card is walked onto, so a way in from a
/// fresh launch takes the newest card rather than saying there is nothing
/// to enter. None with no session anywhere, which [`NO_SESSIONS`] answers.
fn cursor_or_first(app: &App) -> Option<AgentId> {
    let rows = view::rows(app);
    let at = view::cursor(app, &rows).or((!rows.is_empty()).then_some(0))?;
    Some(rows[at].agent.id.clone())
}

// ---- the box's own keys ----

/// Is `prompt`'s key one of the view's box chords? `^P` (project), `^O`
/// (model) and `^N` (fresh worktree or not) — the rest of the box's keys
/// are the QUICK PROMPT's own. True when the key was taken.
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
            cloud: false,
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
/// the project's own checkout — the project the box is aimed at, which is
/// not always the one under the list's cursor (`^P` moves it). Only this
/// box: the next one starts from the `quick_prompt_new_worktree` SETTING.
/// A PR SESSION's checkout is the DAEMON's to pick, so it has nothing to
/// flip.
fn toggle_new_worktree(app: &mut App, launch: QuickLaunch, input: TextInput) {
    if launch.pr.is_some() {
        app.flash =
            Some("quick prompt: a PR session runs in the pull request's own checkout".into());
        return;
    }
    let Some(project) = view::project_of(app, &launch.target) else {
        app.flash = Some("project no longer exists".into());
        return;
    };
    let fresh = !launch.is_new_worktree();
    let target = if fresh {
        fresh_worktree(app, project, &launch)
    } else {
        match view::checkout_for(app, &project) {
            Some(worktree) => QuickTarget::Worktree(worktree),
            None => {
                app.flash = Some("no checkout to reuse — keeping the new worktree".into());
                return;
            }
        }
    };
    reopen_with(app, QuickLaunch { target, ..launch }, input);
}

/// A fresh worktree for `launch` in `project`, on a branch nobody has
/// yet: named after the issue for an ISSUE SESSION, the random name `n`
/// would offer otherwise.
fn fresh_worktree(app: &App, project: ProjectId, launch: &QuickLaunch) -> QuickTarget {
    let taken = app.project_branches(&project);
    let branch = match &launch.issue {
        Some(issue) => crate::branch_name::issue_name(issue.number, &issue.title, &taken),
        None => crate::branch_name::random_name(&taken),
    };
    QuickTarget::NewWorktree { project, branch }
}

/// The WORKTREE PICKER for the box `back` owes — a click on the branch in
/// the box's details row (`worktree main ▾`), dropped down from that
/// branch over the box, which stays on screen under it as it does under
/// `Tab` and `^O` (`ui::draw_overlay`). It is the manual pick of where
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
    let hide_root = app
        .tree
        .projects
        .iter()
        .find(|p| p.id == project)
        .is_some_and(|p| app.root_hidden(p));
    let mut checkouts: Vec<_> = app
        .tree
        .worktrees
        .iter()
        .filter(|w| {
            w.project_id == project
                && !(hide_root && w.is_main)
                && !app.is_placeholder_worktree(&w.id)
        })
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
        QuickTarget::Worktree(_) => fresh_worktree(app, project.clone(), &back.launch),
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
/// under the cursor, text kept, a fresh worktree or its checkout as the
/// box had it. Aiming the box is not navigation: the grid behind it stays
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

    /// INPUT PARITY for the PANE's top edge: the grab zone the draw
    /// registers is where the pane actually starts, a drag through the
    /// loop's own entry point moves that boundary, and the next frame lays
    /// the grid and the pane out at the height it was left at — grip and
    /// grab zone moving with it. The draw and the handler measure the edge
    /// by the same arithmetic, so neither can drift from the other.
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
    /// **Session pane**): the draw lays it down the right or the left side,
    /// registers the grab zone on the edge facing the cards, and a drag
    /// through the loop's own entry point moves that edge sideways — the
    /// next frame laying the pane out at the width it was left at, and the
    /// height it had under the cards left alone.
    #[test]
    fn a_side_panes_edge_drags_sideways() {
        use crate::launcher::PaneSide;
        with_default_config(|| {
            for side in [PaneSide::Right, PaneSide::Left] {
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
                assert_eq!((pane.y, pane.height), (body.y, body.height), "{side:?}");
                let zone = edge(&app).expect("the pane's edge was registered");
                let grip_x = crate::launcher::pane_edge(side, pane).x;
                assert!(zone.x <= grip_x && grip_x < zone.x + zone.width);
                assert_eq!(zone.height, pane.height, "the whole edge is grabbable");
                assert!(col(&terminal, grip_x).contains('┃'), "the grip marks it");

                // Four columns toward the cards: the pane takes them.
                let toward = if side == PaneSide::Right { -4 } else { 4 };
                let to = (i32::from(grip_x) + toward) as u16;
                let row = pane.y + pane.height / 2;
                mouse(
                    &mut app,
                    MouseEventKind::Down(MouseButton::Left),
                    grip_x,
                    row,
                );
                mouse(&mut app, MouseEventKind::Drag(MouseButton::Left), to, row);
                mouse(&mut app, MouseEventKind::Up(MouseButton::Left), to, row);
                assert_eq!(app.launcher_pane_w, Some(pane.width + 4), "{side:?}");
                assert_eq!(app.launcher_pane_h, None, "the bottom's height untouched");

                let terminal = draw(&mut app);
                let (_, moved) = app.launcher_split(app.launcher_body);
                let moved = moved.expect("still a pane");
                assert_eq!(moved.width, pane.width + 4);
                assert_eq!(crate::launcher::pane_edge(side, moved).x, to);
                assert!(col(&terminal, to).contains('┃'), "the grip moved with it");
            }
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
        let mut out = Vec::new();
        let event = crossterm::event::Event::Key(crossterm::event::KeyEvent::new(code, mods));
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

    /// INPUT PARITY: the box's three details — `project ^P`, `agent Tab`,
    /// `model ^O` — are buttons as much as the toggle under them. A click
    /// on one opens exactly what its chord opens, down to the rows in it,
    /// and Esc hands the box back with what was typed either way. The air
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
                            .map(|i| i.label.as_str())
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

    /// The PANE's header is a TAB STRIP: the `SESSION` tab, the checkout
    /// the tabs after it belong to, a divider, then one tab per TERMINAL
    /// open in that checkout — in that order, because the branch is what
    /// scopes everything drawn after it. Each tab is a click target the
    /// draw registered where the word actually is.
    #[test]
    fn the_pane_header_lists_the_checkouts_terminals_after_the_session_tab() {
        with_default_config(|| {
            let mut app = two_sessions();
            seed_terminal(&mut app, "t1", "w2", "shell-1");
            seed_terminal(&mut app, "t2", "w2", "shell-2");
            draw(&mut app);
            // Onto the `feat` card, so `feat` is the strip's checkout and
            // its two terminals are the tabs.
            key(&mut app, KeyCode::Char('h'), KeyModifiers::NONE);
            let terminal = draw(&mut app);
            let head = head_row(&app, &terminal);

            // Columns, not bytes: the strip is full of `·`, `│` and `❯`,
            // and a click lands on a cell.
            let at = |needle: &str| {
                let byte = head
                    .find(needle)
                    .unwrap_or_else(|| panic!("{head:?} is missing {needle:?}"));
                head[..byte].chars().count()
            };
            assert!(
                at("SESSION") < at("feat"),
                "the checkout comes after the tab it scopes: {head:?}"
            );
            assert!(
                at("feat") < at('│'.to_string().as_str()),
                "the divider closes the session side off: {head:?}"
            );
            assert!(
                at("│") < at("shell-1") && at("shell-1") < at("shell-2"),
                "the terminals follow the divider, in tree order: {head:?}"
            );

            // And every tab is a button, on the row it was drawn on.
            let session = tab_at(&app, HitTarget::LauncherPaneSession);
            let second = tab_at(&app, HitTarget::LauncherPaneTerminal(1));
            assert_eq!(session.y, second.y, "one row, all of it");
            assert_eq!(
                usize::from(session.x - app.launcher_body.x),
                at("SESSION"),
                "the SESSION button is where the word is"
            );
            assert_eq!(
                usize::from(second.x - app.launcher_body.x),
                at("shell-2") - 2,
                "a terminal's button takes its glyph as well as its name"
            );
        });
    }

    /// INPUT PARITY: `` ` `` and a click on a tab walk the same strip
    /// through the one [`show_pane_tab`], and what the pane READS follows
    /// — SESSION, each terminal in turn, then round to SESSION again.
    #[test]
    fn the_key_and_a_click_walk_the_same_tab_strip() {
        with_default_config(|| {
            let mut app = two_sessions();
            seed_terminal(&mut app, "t1", "w2", "shell-1");
            seed_terminal(&mut app, "t2", "w2", "shell-2");
            draw(&mut app);
            // Onto the `feat` card, whose checkout holds both terminals.
            key(&mut app, KeyCode::Char('h'), KeyModifiers::NONE);
            draw(&mut app);
            let term = |id: &str| Some(SessionRef::Terminal(TerminalId(id.into())));

            key(&mut app, KeyCode::Char('`'), KeyModifiers::NONE);
            assert_eq!(reading(&app), term("t1"), "the first tab past SESSION");
            key(&mut app, KeyCode::Char('`'), KeyModifiers::NONE);
            assert_eq!(reading(&app), term("t2"));
            key(&mut app, KeyCode::Char('`'), KeyModifiers::NONE);
            assert!(
                matches!(reading(&app), Some(SessionRef::Agent(_))),
                "past the last tab is back to the card: {:?}",
                reading(&app)
            );

            // The pointer lands in exactly the states the key does.
            draw(&mut app);
            let tab = tab_at(&app, HitTarget::LauncherPaneTerminal(1));
            mouse(
                &mut app,
                MouseEventKind::Down(MouseButton::Left),
                tab.x,
                tab.y,
            );
            assert_eq!(reading(&app), term("t2"), "a click reads that terminal");

            draw(&mut app);
            let tab = tab_at(&app, HitTarget::LauncherPaneSession);
            mouse(
                &mut app,
                MouseEventKind::Down(MouseButton::Left),
                tab.x,
                tab.y,
            );
            assert!(
                matches!(reading(&app), Some(SessionRef::Agent(_))),
                "and SESSION hands the pane back to the card: {:?}",
                reading(&app)
            );
        });
    }

    /// The strip is one checkout's, so the terminal on it stays up while
    /// the cursor walks the cards of that checkout — and is let go of the
    /// moment the cursor lands in another one, where the strip lists
    /// different terminals and the pin names nothing on it.
    #[test]
    fn a_terminal_stays_up_inside_its_checkout_and_is_dropped_leaving_it() {
        with_default_config(|| {
            let mut app = two_sessions();
            seed_running(&mut app, "a4", "w2", "more-feat");
            seed_terminal(&mut app, "t1", "w2", "shell-1");
            draw(&mut app);
            let card = |app: &App, name: &str| {
                crate::launcher::rows(app)
                    .iter()
                    .position(|r| r.agent.name == name)
                    .unwrap_or_else(|| panic!("no card for {name}"))
            };

            let mut out = Vec::new();
            let feat = card(&app, "polish-nav");
            super::select_row(&mut app, feat, &mut out);
            key(&mut app, KeyCode::Char('`'), KeyModifiers::NONE);
            assert_eq!(
                reading(&app),
                Some(SessionRef::Terminal(TerminalId("t1".into())))
            );

            // Another card in the same checkout: the terminal is still up.
            let sibling = card(&app, "more-feat");
            super::select_row(&mut app, sibling, &mut out);
            draw(&mut app);
            assert_eq!(
                reading(&app),
                Some(SessionRef::Terminal(TerminalId("t1".into()))),
                "walking inside the checkout must not swap the terminal out"
            );

            // A card in the root checkout: different terminals, so the pin
            // goes and the pane reads the card again.
            let root = card(&app, "agent-1");
            super::select_row(&mut app, root, &mut out);
            draw(&mut app);
            assert_eq!(app.launcher_terminal, None, "the pin was let go of");
            assert!(
                matches!(reading(&app), Some(SessionRef::Agent(_))),
                "the pane is the card's again: {:?}",
                reading(&app)
            );
        });
    }

    /// The `×` on a tab is how a terminal is done with: a click on it puts
    /// up the same confirm the SESSIONS PANEL's `d` does, `y` kills the
    /// shell, and the strip lands on the tab beside it rather than on a
    /// pane left blank by the detach.
    #[test]
    fn the_cross_on_a_tab_closes_that_terminal_and_the_strip_moves_on() {
        with_default_config(|| {
            let mut app = two_sessions();
            seed_terminal(&mut app, "t1", "w2", "shell-1");
            seed_terminal(&mut app, "t2", "w2", "shell-2");
            draw(&mut app);
            // Onto the `feat` card, then onto its first terminal.
            key(&mut app, KeyCode::Char('h'), KeyModifiers::NONE);
            key(&mut app, KeyCode::Char('`'), KeyModifiers::NONE);
            let term = |id: &str| Some(SessionRef::Terminal(TerminalId(id.into())));
            assert_eq!(reading(&app), term("t1"));

            draw(&mut app);
            let cross = tab_at(&app, HitTarget::LauncherPaneCloseTerminal(0));
            mouse(
                &mut app,
                MouseEventKind::Down(MouseButton::Left),
                cross.x,
                cross.y,
            );
            assert!(
                matches!(&app.overlay, Some(Overlay::Confirm(c))
                    if matches!(c.action, PendingAction::CloseTerminal(_))),
                "the cross asks first: {:?}",
                app.overlay
            );

            let out = key(&mut app, KeyCode::Char('y'), KeyModifiers::NONE);
            assert!(
                out.iter()
                    .any(|r| matches!(r, ClientRequest::CloseTerminal { id, .. }
                    if id == &TerminalId("t1".into()))),
                "the daemon is told to close it: {out:?}"
            );
            assert_eq!(
                reading(&app),
                term("t2"),
                "and the strip lands on the tab beside it"
            );

            // Closing the last one hands the pane back to the card.
            draw(&mut app);
            let cross = tab_at(&app, HitTarget::LauncherPaneCloseTerminal(0));
            mouse(
                &mut app,
                MouseEventKind::Down(MouseButton::Left),
                cross.x,
                cross.y,
            );
            key(&mut app, KeyCode::Char('y'), KeyModifiers::NONE);
            assert_eq!(app.launcher_terminal, None, "no tab is pinned any more");
            assert!(
                matches!(reading(&app), Some(SessionRef::Agent(_))),
                "the pane is the card's again: {:?}",
                reading(&app)
            );
        });
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

    /// `d` follows the strip: on a TERMINAL it closes that terminal, and
    /// back on SESSION it is the card's delete again — the card is never
    /// deleted by a key aimed at the pane.
    #[test]
    fn d_closes_the_terminal_the_strip_is_on_and_the_card_otherwise() {
        with_default_config(|| {
            let mut app = two_sessions();
            seed_terminal(&mut app, "t1", "w2", "shell-1");
            draw(&mut app);
            key(&mut app, KeyCode::Char('h'), KeyModifiers::NONE);

            // On SESSION, `d` is still the card's.
            key(&mut app, KeyCode::Char('d'), KeyModifiers::NONE);
            assert!(
                matches!(&app.overlay, Some(Overlay::Confirm(c))
                    if matches!(c.action, PendingAction::DeleteAgent(_))),
                "the card's delete: {:?}",
                app.overlay
            );
            key(&mut app, KeyCode::Esc, KeyModifiers::NONE);

            // On the terminal it is the terminal's.
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
            for turn in ["first", "second"] {
                // Along the row to the next card (which takes the aim
                // back, the fold having let it go), then prompt it.
                key(&mut app, KeyCode::Char('h'), KeyModifiers::NONE);
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

    /// A pane already open is left exactly where it is — including on a
    /// TERMINAL tab, which prompting the card does not swap away from.
    #[test]
    fn an_open_pane_is_left_on_the_tab_it_was_reading() {
        with_default_config(|| {
            let mut app = two_sessions();
            seed_terminal(&mut app, "t1", "w2", "shell-1");
            draw(&mut app);
            key(&mut app, KeyCode::Char('h'), KeyModifiers::NONE);
            key(&mut app, KeyCode::Char('`'), KeyModifiers::NONE);
            let reading = pane(&app);
            assert!(app.pinned_terminal().is_some(), "the pane is on a terminal");

            key(&mut app, KeyCode::Char(' '), KeyModifiers::NONE);
            type_text(&mut app, "status?");
            key(&mut app, KeyCode::Enter, KeyModifiers::NONE);

            assert!(app.pinned_terminal().is_some(), "still on the terminal tab");
            assert_eq!(pane(&app), reading, "and still reading it");
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

    /// INPUT PARITY: **Follow-up prompt** in the card's `m` menu opens the
    /// same modal Space does.
    #[test]
    fn the_menu_row_opens_the_same_modal_space_does() {
        with_default_config(|| {
            let mut app = two_sessions();
            draw(&mut app);
            let id = app.selected_session().map(|a| a.id.clone()).unwrap();

            key(&mut app, KeyCode::Char('m'), KeyModifiers::NONE);
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
                Some("cloud sessions take a queued message — m, then Send to cloud session"),
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

    /// A click on the branch in the box's header opens the WORKTREE PICKER
    /// over the box, hung right under the branch: a fresh worktree first,
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
                text.contains("worktree feat ▾"),
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

    /// A cell inside the card whose hit region names `index`, as drawn.
    fn row_cell(app: &App, index: usize) -> (u16, u16) {
        let (rect, _) = app
            .hits
            .iter()
            .find(|(_, hit)| *hit == HitTarget::LauncherRow(index))
            .unwrap_or_else(|| panic!("card {index} was not drawn"));
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

    /// `h` / `l` walk a row of cards — the selected project's, newest
    /// session first — and `j` / `k` walk the column. The PANE under the grid
    /// follows the cursor: whichever card it lands on is the session the
    /// pane reads, as ↑/↓ down the SESSIONS PANEL previews a row. The
    /// grid's cursor is the panels' selection, so the verbs that read it
    /// name the same session.
    #[test]
    fn hjkl_walk_the_grid_and_the_pane_follows() {
        with_default_config(|| {
            let mut app = two_sessions();
            draw(&mut app);
            assert_eq!(app.focus, Focus::Sessions, "the grid has the keys");
            assert_eq!(
                selected(&app).as_deref(),
                Some("a1"),
                "the cursor starts on the selected session — the older, card 2"
            );

            key(&mut app, KeyCode::Char('h'), KeyModifiers::NONE);
            assert_eq!(selected(&app).as_deref(), Some("a2"), "the newest first");
            assert_eq!(
                pane(&app),
                Some(SessionRef::Agent(AgentId("a2".into()))),
                "the card walked onto is what the pane reads"
            );
            assert_eq!(
                app.selected_project().map(|p| p.name.as_str()),
                Some("demo"),
                "the walk stays inside the project the level is scoped to"
            );
            key(&mut app, KeyCode::Char('h'), KeyModifiers::NONE);
            assert_eq!(
                selected(&app).as_deref(),
                Some("a2"),
                "the first card stays"
            );

            key(&mut app, KeyCode::Char('l'), KeyModifiers::NONE);
            assert_eq!(selected(&app).as_deref(), Some("a1"));
            assert_eq!(
                pane(&app),
                Some(SessionRef::Agent(AgentId("a1".into()))),
                "and swaps with the cursor"
            );
            assert_eq!(
                app.selected_project().map(|p| p.name.as_str()),
                Some("demo")
            );
            key(&mut app, KeyCode::Char('l'), KeyModifiers::NONE);
            assert_eq!(selected(&app).as_deref(), Some("a1"), "the last card stays");
            // Both cards are on one row here, so the column keys have
            // nowhere to go.
            key(&mut app, KeyCode::Char('k'), KeyModifiers::NONE);
            assert_eq!(selected(&app).as_deref(), Some("a1"));
            assert_eq!(app.focus, Focus::Sessions);

            // One card a row, and `j` / `k` are the walk instead.
            let mut app = two_sessions();
            draw_narrow(&mut app);
            key(&mut app, KeyCode::Char('k'), KeyModifiers::NONE);
            assert_eq!(selected(&app).as_deref(), Some("a2"));
            key(&mut app, KeyCode::Char('j'), KeyModifiers::NONE);
            assert_eq!(selected(&app).as_deref(), Some("a1"));
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
            let out = key(&mut app, KeyCode::Char('h'), KeyModifiers::NONE);
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

    /// The card under the cursor is live in the PANE along the BOTTOM: the
    /// pane names it, and sits under every card the grid drew rather than
    /// beside them. Its own frame, not the full-screen breadcrumb — the
    /// grid is still up.
    #[test]
    fn the_pane_along_the_bottom_reads_the_card_under_the_cursor() {
        with_default_config(|| {
            let mut app = two_sessions();
            draw(&mut app);
            key(&mut app, KeyCode::Char('h'), KeyModifiers::NONE);
            let text = buffer_text(&draw(&mut app));
            assert!(
                text.contains("SESSION · polish-nav"),
                "the pane names the cursor's card: {text}"
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
                    matches!(hit, HitTarget::LauncherRow(_)).then_some(r.y + r.height)
                })
                .max()
                .expect("the grid drew cards");
            assert!(
                app.term_area.y >= below,
                "the pane is under the cards, not beside them: {:?} vs {below}",
                app.term_area
            );

            // And the walk keeps swapping it.
            key(&mut app, KeyCode::Char('l'), KeyModifiers::NONE);
            let text = buffer_text(&draw(&mut app));
            assert!(
                text.contains("SESSION · agent-1"),
                "the next card takes the pane: {text}"
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
            key(&mut app, KeyCode::Char('h'), KeyModifiers::NONE);
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
            key(&mut app, KeyCode::Char('l'), KeyModifiers::NONE);
            assert_eq!(
                selected(&app).as_deref(),
                Some("a1"),
                "and the cards walk again"
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
            key(&mut app, KeyCode::Char('h'), KeyModifiers::NONE);
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

    /// `z` full-screens the session under the cursor and takes its
    /// input — the same state `z` leaves the panels in; the hatch hands
    /// the keys back to the grid, the cursor where it was.
    #[test]
    fn z_full_screens_the_session_and_the_hatch_returns_to_the_grid() {
        with_default_config(|| {
            let mut app = two_sessions();
            draw(&mut app);
            key(&mut app, KeyCode::Char('h'), KeyModifiers::NONE);
            key(&mut app, KeyCode::Char('z'), KeyModifiers::NONE);
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
            assert!(text.contains("2 sessions"), "{text}");
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
    /// the view opened on — no `nebula`, no trail. A lone tab
    /// has no `×`: it is the project on screen.
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
                vec![HitTarget::LauncherTabAdd, HitTarget::LauncherTab(demo)],
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
            key(&mut app, KeyCode::Char('z'), KeyModifiers::NONE);
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

    /// `m` with no card selected is the PROJECT's menu — there is no
    /// session under the cursor for it to be the menu of — and with a
    /// card selected it is still that card's.
    #[test]
    fn m_with_nothing_selected_is_the_projects_menu() {
        with_default_config(|| {
            let mut app = two_sessions();
            draw(&mut app);
            key(&mut app, KeyCode::Char('m'), KeyModifiers::NONE);
            let Some(Overlay::Menu(menu)) = &app.overlay else {
                panic!("the card's menu: {:?}", app.overlay);
            };
            assert!(
                !menu.items.iter().any(|i| i.label == "Remove from list"),
                "a card's menu is the card's"
            );
            app.overlay = None;

            key(&mut app, KeyCode::Esc, KeyModifiers::NONE);
            assert!(app.launcher_unaimed);
            key(&mut app, KeyCode::Char('m'), KeyModifiers::NONE);
            let Some(Overlay::Menu(menu)) = &app.overlay else {
                panic!("the project's menu: {:?}", app.overlay);
            };
            let labels: Vec<&str> = menu.items.iter().map(|i| i.label.as_str()).collect();
            assert!(labels.contains(&"Rename"), "{labels:?}");
            assert!(labels.contains(&"Remove from list"), "{labels:?}");
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
    /// A switch moves no tab: the order is when each was opened.
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
    /// tab is the project on screen, with nothing to hand the grid to, so
    /// it stays — and says why.
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

            let before = tab_state(&by_key);
            key(&mut by_key, KeyCode::Char('x'), KeyModifiers::NONE);
            assert_eq!(tab_state(&by_key), before, "the last tab stays");
            assert_eq!(by_key.flash.as_deref(), Some(super::LAST_TAB));
            draw(&mut by_key);
            assert_eq!(tabs_drawn(&by_key), ["web"]);
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
            keys(&mut app, &[KeyCode::Char('h'), KeyCode::Char('h')]);
            assert_eq!(selected(&app).as_deref(), Some("a2"), "the first card");
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

    /// Only the top row walks up: from a card with a row over it `k` is
    /// the step onto that row, and the double tap starts there. `↑` is the
    /// same key.
    #[test]
    fn k_below_the_top_row_is_a_step_up_the_grid() {
        with_default_config(|| {
            let mut app = two_tabs();
            draw_narrow(&mut app);
            keys(&mut app, &[KeyCode::Char('k'), KeyCode::Char('k')]);
            assert_eq!(app.launcher_tab_cursor, None);
            keys(&mut app, &[KeyCode::Char('j')]);
            assert_eq!(selected(&app).as_deref(), Some("a1"), "the second row");

            key(&mut app, KeyCode::Up, KeyModifiers::NONE);
            assert_eq!(selected(&app).as_deref(), Some("a2"), "a row up");
            assert_eq!(app.launcher_tab_cursor, None, "and not armed by the step");
            key(&mut app, KeyCode::Up, KeyModifiers::NONE);
            assert_eq!(app.launcher_tab_cursor, None, "the top row's first press");
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
            let (x, y) = crumb_cell(&app, HitTarget::LauncherRow(1));
            mouse(
                &mut app,
                MouseEventKind::Down(MouseButton::Left),
                x + 2,
                y + 1,
            );
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
            key(&mut full, KeyCode::Char('z'), KeyModifiers::NONE);
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
            key(&mut by_key, KeyCode::Char('z'), KeyModifiers::NONE);
            draw(&mut by_key);
            key(&mut by_key, KeyCode::Char('q'), KeyModifiers::CONTROL);

            let mut by_click = two_sessions();
            draw(&mut by_click);
            key(&mut by_click, KeyCode::Char('z'), KeyModifiers::NONE);
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

    /// `a` on the grid archives the session under the cursor. The first
    /// card has no card before it to fall back on, so the cursor takes
    /// the one that slides up into its place rather than resting on
    /// nothing.
    #[test]
    fn archiving_the_first_card_takes_the_card_that_slides_up() {
        with_default_config(|| {
            let mut app = two_sessions();
            draw(&mut app);
            key(&mut app, KeyCode::Char('h'), KeyModifiers::NONE);
            assert_eq!(selected(&app).as_deref(), Some("a2"));
            key(&mut app, KeyCode::Char('a'), KeyModifiers::NONE);
            assert!(
                app.tree.agents.iter().any(|a| a.id.0 == "a2" && a.archived),
                "archived at once"
            );
            assert_eq!(selected(&app).as_deref(), Some("a1"));
            assert_eq!(app.focus, Focus::Sessions);
        });
    }

    /// Anywhere else in the grid, `a` hands the cursor to the card BEFORE
    /// the one archived — the card the walk came in from, where the eye
    /// already is — and the pane under the grid comes with it. Archiving
    /// down a row of cards therefore walks backwards through them instead
    /// of pulling the rest of the row up under a cursor that stayed put.
    #[test]
    fn archiving_a_card_lands_on_the_card_before_it() {
        with_default_config(|| {
            let mut app = three_sessions();
            draw(&mut app);
            assert_eq!(cards(&app), ["a9", "a2", "a1"], "newest first");
            let (x, y) = row_cell(&app, 1);
            mouse(&mut app, MouseEventKind::Down(MouseButton::Left), x, y);
            assert_eq!(selected(&app).as_deref(), Some("a2"), "the middle card");

            key(&mut app, KeyCode::Char('a'), KeyModifiers::NONE);
            assert_eq!(cards(&app), ["a9", "a1"]);
            assert_eq!(
                selected(&app).as_deref(),
                Some("a9"),
                "the card before it, not the one that slid up"
            );
            assert_eq!(
                pane(&app),
                Some(SessionRef::Agent(AgentId("a9".into()))),
                "and the pane reads it"
            );
            assert_eq!(app.focus, Focus::Sessions);
        });
    }

    /// The PANELS reseat their own cursor on the same archive, onto the
    /// next row of the CHECKOUT the session sat in — and that row is
    /// somewhere else entirely in a grid ordered by recency across the
    /// project's checkouts. Taking it threw the cursor across the screen:
    /// here archiving the first card landed on the last. The grid settles
    /// its own landing instead of letting that stand.
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
            assert_eq!(cards(&app), ["a2", "a1"]);
            assert_eq!(
                selected(&app).as_deref(),
                Some("a2"),
                "the slot the archived card left — not w1's own next row, a1"
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
            super::select(&mut app, AgentId("a1".into()), &mut Vec::new());
            assert_eq!(selected(&app).as_deref(), Some("a1"), "the last card");
            let out = key(&mut app, KeyCode::Char('a'), KeyModifiers::NONE);
            assert_eq!(selected(&app).as_deref(), Some("a2"), "the card before it");

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
                .find(|a| a.id.0 == "a1")
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
                Some("a2"),
                "the daemon's own events leave the landing alone"
            );
        });
    }

    /// The list moving in the same breath as the archive — a row arriving
    /// from the DAEMON, a turn starting and re-sorting the grid, the launch
    /// pin dropping — must not drag the landing along with it. The cursor
    /// takes the card it was walked in from BY NAME; counting `index - 1`
    /// landed it a card past that one, skipping the card the eye was on.
    #[test]
    fn a_list_that_moves_under_the_archive_still_lands_on_the_card_before() {
        with_default_config(|| {
            let mut app = three_sessions();
            draw(&mut app);
            assert_eq!(cards(&app), ["a9", "a2", "a1"], "newest first");
            super::select(&mut app, AgentId("a1".into()), &mut Vec::new());

            // What the input event opens with: the cursor on the last card.
            let before = super::cursor_entry(&app).expect("a card under the cursor");
            // ...and in the same breath a newer session leads the grid, so
            // every index below it has moved by one.
            seed_running(&mut app, "az", "w1", "fresh-one");
            assert_eq!(cards(&app), ["az", "a9", "a2", "a1"], "the newcomer leads");

            let mut out = Vec::new();
            super::super::archive_agent_now(&mut app, AgentId("a1".into()), &mut out);
            super::keep_cursor(&mut app, before, &mut out);
            assert_eq!(cards(&app), ["az", "a9", "a2"]);
            assert_eq!(
                selected(&app).as_deref(),
                Some("a2"),
                "the card that was before it, not the one a shifted index points at"
            );
        });
    }

    /// The LAST card — the oldest, the end of the list, the one an
    /// archiving sweep reaches last — has no card after it to fall back
    /// on, and still lands on the card before it.
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
            assert_eq!(cards(&app), ["a9", "a2"]);
            assert_eq!(selected(&app).as_deref(), Some("a2"), "the card before it");
        });
    }

    /// The ONLY card leaving — archived with `a`, or deleted with `d` and
    /// its confirm — leaves nothing for the PANE to read, so it folds away
    /// and the empty grid takes the body, the way a project opened with no
    /// sessions lands. INPUT PARITY: both verbs end in the same state.
    #[test]
    fn the_last_card_leaving_folds_the_pane_away() {
        with_default_config(|| {
            for keys in [&['a'][..], &['d', 'y'][..]] {
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

    /// `confirm_on_archive` on: the archive runs on the dialog's Enter,
    /// a second input event, and that is the one the landing is kept
    /// across. The bare key's landing and this one are the same card.
    #[test]
    fn archiving_with_the_confirm_on_lands_on_the_card_before_it() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        std::fs::write(&path, r#"{"confirm_on_archive": true}"#).unwrap();
        crate::config::with_config_path(path, || {
            let mut app = three_sessions();
            draw(&mut app);
            let (x, y) = row_cell(&app, 2);
            mouse(&mut app, MouseEventKind::Down(MouseButton::Left), x, y);
            assert_eq!(selected(&app).as_deref(), Some("a1"), "the last card");

            key(&mut app, KeyCode::Char('a'), KeyModifiers::NONE);
            key(&mut app, KeyCode::Enter, KeyModifiers::NONE);
            assert_eq!(cards(&app), ["a9", "a2"], "archived on the confirm");
            assert_eq!(selected(&app).as_deref(), Some("a2"), "the card before it");
        });
    }

    /// INPUT PARITY: a click on a card lands where the keys that walk to
    /// it land — cursor, FOCUS, project — and a second click is Enter.
    #[test]
    fn a_click_on_a_card_is_the_keys_that_walk_to_it() {
        with_default_config(|| {
            let mut by_key = two_sessions();
            draw(&mut by_key);
            key(&mut by_key, KeyCode::Char('h'), KeyModifiers::NONE);

            let mut by_click = two_sessions();
            draw(&mut by_click);
            let (x, y) = row_cell(&by_click, 0);
            mouse(&mut by_click, MouseEventKind::Down(MouseButton::Left), x, y);

            assert_eq!(selected(&by_click), selected(&by_key));
            assert_eq!(pane(&by_click), pane(&by_key));
            assert_eq!(by_click.focus, by_key.focus);
            assert_eq!(by_click.sel_project, by_key.sel_project);

            key(&mut by_key, KeyCode::Enter, KeyModifiers::NONE);
            mouse(&mut by_click, MouseEventKind::Down(MouseButton::Left), x, y);
            assert_eq!(by_click.focus, Focus::Terminal, "the second click is Enter");
            assert_eq!(by_click.term_locked, by_key.term_locked);
            assert_eq!(
                by_click.collapsed, by_key.collapsed,
                "the pane under the grid either way"
            );
            assert!(!by_click.collapsed, "not full-screen: that is `z`");
        });
    }

    /// INPUT PARITY: with the pane folded away (`^~`), a click on a card
    /// brings the pane back under the grid, and the second click — like
    /// Enter, the key behind it — puts the keys in it. Neither
    /// full-screens the session: that is `z` alone. A third click, with
    /// the keys already in the pane, leaves it exactly where it is.
    #[test]
    fn a_click_on_a_card_unfolds_the_pane_rather_than_full_screening() {
        with_default_config(|| {
            let mut by_key = two_sessions();
            draw(&mut by_key);
            key(&mut by_key, KeyCode::Char('~'), KeyModifiers::NONE);
            assert!(by_key.launcher_pane_hidden, "^~ folded the pane away");
            key(&mut by_key, KeyCode::Char('h'), KeyModifiers::NONE);
            key(&mut by_key, KeyCode::Enter, KeyModifiers::NONE);
            assert!(!by_key.launcher_pane_hidden, "Enter brought the pane back");
            assert!(
                !by_key.collapsed,
                "the pane under the grid, not full-screen"
            );
            assert_eq!(by_key.focus, Focus::Terminal, "and the keys are in it");

            let mut by_click = two_sessions();
            draw(&mut by_click);
            key(&mut by_click, KeyCode::Char('~'), KeyModifiers::NONE);
            assert!(by_click.launcher_pane_hidden);
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
                "the pane under the grid either way"
            );
            assert!(!by_click.collapsed, "not full-screen: that is `z`");
            assert_eq!(by_click.focus, by_key.focus, "and the keys are in it");
            assert_eq!(pane(&by_click), pane(&by_key));
            let text = buffer_text(&draw(&mut by_click));
            assert_eq!(
                tabs_drawn(&by_click),
                ["demo"],
                "the grid is still up over the pane: {text}"
            );

            // Again, with the keys already there: nothing moves.
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
                key(app, KeyCode::Char('h'), KeyModifiers::NONE);
                assert!(
                    app.launcher_pane_hidden,
                    "a key walking the cards leaves the fold be"
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
    /// the card you were on. Only the keys walk the grid now.
    #[test]
    fn the_wheel_over_the_grid_leaves_the_cursor_alone() {
        with_default_config(|| {
            let mut app = two_sessions();
            draw_narrow(&mut app);
            let (x, y) = row_cell(&app, 0);
            let before = selected(&app);
            assert_eq!(before.as_deref(), Some("a1"), "the cursor starts here");
            mouse(&mut app, MouseEventKind::ScrollUp, x, y);
            assert_eq!(selected(&app), before, "the wheel up moves nothing");
            mouse(&mut app, MouseEventKind::ScrollDown, x, y);
            assert_eq!(selected(&app), before, "and neither does the wheel down");
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
                Some(crate::event_loop::AGENT_ARCHIVED)
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

            // And `⇧A` again is the live grid.
            key(&mut app, KeyCode::Char('A'), KeyModifiers::SHIFT);
            assert!(!app.show_archived);
            let text = buffer_text(&draw(&mut app));
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
            key(&mut app, KeyCode::Char('h'), KeyModifiers::NONE);
            let text = buffer_text(&draw(&mut app));
            assert!(!text.contains("PROJECTS"), "{text}");
            assert!(!text.contains("WORKTREES"), "{text}");
            assert_eq!(tabs_drawn(&app), ["demo"], "the header's tabs: {text}");
            assert!(text.contains("2 sessions"), "{text}");
            assert!(text.contains("polish-nav"), "{text}");
            // Each card names the checkout its session runs in, in the
            // SCOPE COLOR's own glyph: `↳` for a worktree of its own, `⌂`
            // for the project's root branch — and not the project, which
            // the whole grid is scoped to and the crumb already names.
            assert!(text.contains("↳ feat · claude"), "{text}");
            assert!(
                !text.contains("demo ▸ ↳") && !text.contains("demo ▸ ⌂"),
                "the project is the grid's scope, not a line on every card: {text}"
            );
            assert!(
                !text.contains("tidy-css"),
                "the project beside it is a level up, not in this grid: {text}"
            );
            assert!(text.contains("#42 Polish the nav"), "{text}");
            assert!(text.contains("⌂ main · claude"), "{text}");
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
            // Below this the count itself no longer fits the row, and
            // nothing that could be drawn there would be readable.
            for width in 24..=130u16 {
                let text = buffer_text(&draw_at(&mut app, width, 34));
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
            let text = buffer_text(&draw(&mut app));
            assert!(text.contains("› now make it sticky"), "the newest: {text}");
            assert!(!text.contains("first pass at the nav"), "{text}");
        });
    }

    /// A prompt too long for one row keeps going on the rows under it,
    /// indented to the `›`'s own column — three lines of what was asked,
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
            let text = buffer_text(&draw_narrow(&mut app));
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
                rows[head + 2].contains("session card, so a long ask reads…"),
                "and a third row, ending in an ellipsis: {text}"
            );
            assert!(
                !rows[head + 1].contains('›') && !rows[head + 2].contains('›'),
                "only the first row is marked: {text}"
            );
        });
    }

    /// The box, in the view: each of its details on its first row beside
    /// the chord that changes it — the checkout the launch lands in among
    /// them, beside its `▾` — and the prompt header under them with the
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
            assert!(text.contains("agent claude Tab"), "{text}");
            assert!(text.contains("model default ^O"), "{text}");
            assert!(text.contains("new worktree ^N"), "{text}");
            assert!(
                text.contains("worktree main ▾"),
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
            assert!(by_key.launcher_unaimed, "Esc left the grid aimed");
            assert_eq!(by_key.flash.as_deref(), Some(super::UNAIMED));
        });
    }

    /// The card really stops being drawn as the cursor's — the border
    /// goes back to the frame's own edge color — and the grid does not
    /// scroll or forget where the cursor is: clicking a card takes the
    /// aim straight back.
    #[test]
    fn the_unselected_card_stops_wearing_the_cursor() {
        with_default_config(|| {
            let mut app = two_sessions();
            let terminal = draw(&mut app);
            let at = crate::launcher::cursor(&app, &crate::launcher::rows(&app))
                .expect("the grid opens with a card under the cursor");
            let (cell, _) = *app
                .hits
                .iter()
                .find(|(_, hit)| *hit == HitTarget::LauncherRow(at))
                .expect("the cursor's card was drawn");
            let accent = app.theme.accent;
            let edge = app.theme.edge;
            assert_eq!(
                corner(&terminal, cell),
                accent,
                "the cursor's card starts out wearing the accent border"
            );

            key(&mut app, KeyCode::Esc, KeyModifiers::NONE);
            let terminal = draw(&mut app);
            assert_eq!(
                corner(&terminal, cell),
                edge,
                "no card wears the cursor once the aim is let go of"
            );
            assert!(
                crate::launcher::cursor(&app, &crate::launcher::rows(&app)).is_some(),
                "the grid still knows where the cursor was — it is let go of, not forgotten"
            );

            // A click on a card is the aim back, and the border with it.
            mouse(
                &mut app,
                MouseEventKind::Down(MouseButton::Left),
                cell.x + 2,
                cell.y + 1,
            );
            assert!(!app.launcher_unaimed, "the click re-aimed the grid");
            let terminal = draw(&mut app);
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

            // The card let go of: the pane goes with it and the cards
            // take the rows it was drawn over.
            key(&mut app, KeyCode::Esc, KeyModifiers::NONE);
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
            assert!(
                !app.collapsed,
                "the pane under the grid: full-screen is `z`"
            );

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

    /// `^`` is the way out of the PANE, in two presses: typing into the
    /// session under the cards, the chord is not forwarded to it but
    /// hands the keys back to the card the pane reads — pane still up,
    /// card still under the cursor — and the same chord from the cards
    /// then folds the pane away. The bare `~` bound beside it is still
    /// the agent's to type.
    #[test]
    fn ctrl_backtick_steps_out_to_the_card_then_folds_the_pane() {
        with_default_config(|| {
            let mut app = two_sessions();
            draw(&mut app);
            key(&mut app, KeyCode::Char('h'), KeyModifiers::NONE);
            key(&mut app, KeyCode::Enter, KeyModifiers::NONE);
            assert_eq!(app.focus, Focus::Terminal);
            assert!(app.term_locked && !app.collapsed);

            let typed = |out: &[ClientRequest]| {
                out.iter().any(|r| matches!(r, ClientRequest::Input { .. }))
            };
            let out = key(&mut app, KeyCode::Char('~'), KeyModifiers::NONE);
            assert!(typed(&out), "a bare ~ is typed into the session: {out:?}");
            assert!(!app.launcher_pane_hidden && app.term_locked);

            let card = crate::launcher::cursor(&app, &crate::launcher::rows(&app));
            let out = key(&mut app, KeyCode::Char('`'), KeyModifiers::CONTROL);
            assert!(!typed(&out), "^` never reaches the session: {out:?}");
            assert_eq!(app.focus, Focus::Sessions, "the keys are the grid's");
            assert!(!app.term_locked);
            assert!(!app.launcher_pane_hidden, "the first ^` left the pane up");
            assert!(!app.launcher_unaimed, "and the card it reads selected");
            assert_eq!(
                crate::launcher::cursor(&app, &crate::launcher::rows(&app)),
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

            key(&mut app, KeyCode::Esc, KeyModifiers::NONE);
            assert!(app.launcher_unaimed);
            let project = app.selected_project().map(|p| p.id.clone());

            app.flash = None;
            key(&mut app, KeyCode::Esc, KeyModifiers::NONE);
            assert!(app.launcher_unaimed);
            assert_eq!(app.selected_project().map(|p| p.id.clone()), project);
            assert_eq!(app.flash, None, "nothing moved, nothing said");

            // The cursor sits on the older card, the right-hand one of
            // the row, so `h` has a card to walk onto.
            key(&mut app, KeyCode::Char('h'), KeyModifiers::NONE);
            assert!(!app.launcher_unaimed, "a step re-aimed the grid");
        });
    }

    /// With nothing selected, `p` asks nothing: no PROJECT PICKER goes
    /// up, the box does — aimed at the project's ROOT BRANCH, the one
    /// place every project has, rather than at the checkout the cursor
    /// was parked in. `^P` in the
    /// box is still the way to another project.
    #[test]
    fn p_with_nothing_selected_opens_the_box_on_the_root_branch() {
        with_default_config(|| {
            let mut app = two_sessions();
            draw(&mut app);

            // Aimed at a card, the box reuses a checkout — the new-worktree
            // SETTING is off by default.
            key(&mut app, KeyCode::Char('p'), KeyModifiers::NONE);
            assert!(
                !launch(&app).0.is_new_worktree(),
                "an aimed p cut a worktree with the setting off"
            );
            app.overlay = None;

            // Let the aim go with the first Esc.
            key(&mut app, KeyCode::Esc, KeyModifiers::NONE);
            assert!(app.launcher_unaimed);

            key(&mut app, KeyCode::Char('p'), KeyModifiers::NONE);
            assert!(
                !matches!(&app.overlay, Some(Overlay::ProjectPicker(_))),
                "the picker went up instead of the box"
            );
            let (launch, _) = launch(&app);
            assert_eq!(
                launch.target,
                QuickTarget::Worktree(WorktreeId("w1".into())),
                "the box did not land on demo's root branch"
            );

            // And it is the box itself, drawn with its own chrome.
            let text = buffer_text(&draw(&mut app));
            assert!(text.contains("new worktree ^N"), "{text}");
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
}
