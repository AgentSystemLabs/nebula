//! The LAUNCHER VIEW's keys and clicks (`crate::launcher` is the view's
//! model, `ui::launcher_view` its drawing). Every arm here translates and
//! calls one function per intent, as the panels' do: a card chosen by key
//! or by pointer lands through [`select`], a session is stepped into
//! through [`enter_pane`] (full-screen through [`open_session`]), and the
//! box opens through [`open_box`] — so `j` and a click on the card below,
//! or Enter and a double-click, end in the same state.

use super::{
    build_submenu, double_tapped, enter_terminal_pane, is_double_click, jump_to_target,
    jump_to_target_inner, open_prompt, restore_project_cursors, restore_workspace_project,
    select_project_row_by_id, switch_workspace_quietly, Landing,
};
use crate::app::{App, Focus, MenuAction, MenuItem, Overlay, PromptKind};
use crate::keymap::Action;
use crate::launcher::{self as view, Level, ProjectPicker};
use crate::palette::PaletteTarget;
use crate::quick_prompt::{QuickLaunch, QuickReturn, QuickTarget};
use crate::text_input::TextInput;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use nebula_core::{AgentId, ClientRequest, ProjectId, WorkspaceId};

/// Rows of cards `Ctrl+d` / `Ctrl+u` jump.
const HALF_PAGE: i64 = 2;

/// What the grid says when there is nothing to step through yet.
const NO_SESSIONS: &str = "no sessions yet — p starts one";

/// What the panel toggles say here: there are no panels to show or hide.
const NO_PANELS: &str = "the launcher view has no panels — Settings › Experimental switches it off";

/// What the PROJECTS level says with nothing to step through.
const NO_PROJECTS: &str = "no projects in this workspace — Esc steps out to the workspaces";

/// What Esc says at the top of the tree: there is no level above the
/// workspaces to walk out to.
const AT_TOP: &str = "workspaces are the top of the tree — Enter opens the one under the cursor";

/// What the footer's workspace nameplate says here. Workspaces are not a
/// dialog in this view but the top LEVEL of it — workspaces / projects /
/// sessions, the hierarchy `Esc` walks up — so the switcher stays shut
/// (`event_loop::open_workspace_picker`) and `w` walks there instead.
pub(super) const WORKSPACES_LEVEL: &str =
    "workspaces are a level of this view, not a dialog — w walks up to it";

/// The first snapshot has landed: with the view on and a project to aim
/// at, the box goes up — nebula opens on it, focused, the way the view
/// promises. Once per run; a snapshot with no project yet (a first run)
/// leaves it owed until one arrives, and another modal already up (a
/// `nebula open` from a session) keeps it rather than covering that.
pub(super) fn boot(app: &mut App) {
    if !app.launcher_boot || !app.launcher_active() || app.overlay.is_some() {
        return;
    }
    app.launcher_boot = false;
    open_box(app);
}

/// The box: the QUICK PROMPT, aimed at the project under the list's cursor
/// (the selected project) and at a fresh worktree or that project's
/// checkout as `^N` last left it. `p`, `n`, and the boot.
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
    let target = view::target_for(app, &project, app.launcher_new_worktree);
    crate::quick_prompt::open_for(app, target);
}

/// A panel key while the GRID is up — true when the view took it. Which
/// grid it is depends on the LEVEL ([`view::Level`]): the keys that walk
/// cards, open one and walk the levels are the same three sets of arms,
/// reading a different list each time.
///
/// Nothing here fires while a session is full-screen: the keys are the
/// PTY's then, and `^q` (`leave_terminal_lock`) is the way back to the
/// grid.
pub(super) fn handle_action(
    app: &mut App,
    action: Action,
    armed: Option<(Action, std::time::Instant)>,
    chord: &crate::keymap::KeyChord,
    out: &mut Vec<ClientRequest>,
) -> bool {
    match app.launcher_level {
        Level::Sessions => sessions_action(app, action, armed, chord, out),
        Level::Projects => projects_action(app, action, armed, chord, out),
        Level::Workspaces => workspaces_action(app, action, armed, chord, out),
    }
}

/// The SESSIONS level's keys: `h` and `l` walk a row of cards, `j` and
/// `k` the column under the cursor, the ways into a session step down
/// into the PANE along the bottom (`z` full-screens it instead), `p` /
/// `n` open the box, the panel toggles say there are no panels, `w`
/// walks up to the workspaces and a digit opens the Nth one; every other
/// key falls through to its panel meaning, which reads the same
/// selection the grid's cursor is.
///
/// `k` on the grid's top row is the one key that is not a step: there is
/// nowhere above it to go, so a second press inside `DOUBLE_TAP` walks
/// out to the PROJECTS level, exactly as `k`,`k` on a panel's first row
/// steps up into the WORKSPACES BAR.
fn sessions_action(
    app: &mut App,
    action: Action,
    armed: Option<(Action, std::time::Instant)>,
    chord: &crate::keymap::KeyChord,
    out: &mut Vec<ClientRequest>,
) -> bool {
    match action {
        Action::MoveDown => step_grid(app, 0, 1, out),
        Action::MoveUp if at_top(app) => {
            if double_tapped(app, action, armed, chord, "projects") {
                go_up(app, out);
            }
        }
        Action::MoveUp => step_grid(app, 0, -1, out),
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
        Action::New | Action::QuickPrompt => open_box(app),
        Action::ToggleProjects
        | Action::ToggleWorktrees
        | Action::ToggleSessions
        | Action::ToggleSidebars => app.flash = Some(NO_PANELS.into()),
        _ => return level_walk(app, action, out),
    }
    true
}

/// The keys every level shares: `w` to the top of the tree, a digit to
/// the Nth workspace, ⇧W with nothing to fold. False for a key none of
/// them claims, which then keeps its panel meaning.
fn level_walk(app: &mut App, action: Action, out: &mut Vec<ClientRequest>) -> bool {
    match action {
        // Workspaces are a level here, not a dialog over the grid: `w`
        // walks to the top of the tree rather than opening the panels'
        // switcher (which `event_loop::open_workspace_picker` keeps shut).
        Action::Workspaces => go_to(app, Level::Workspaces),
        // A digit opens that workspace and lands on its projects — the
        // tab it names, without the bar the view never draws.
        Action::SelectWorkspace(n) => open_workspace_slot(app, n, out),
        // ⇧W folds a bar that is never drawn.
        Action::ToggleWorkspaces => app.flash = Some(WORKSPACES_LEVEL.into()),
        _ => return false,
    }
    true
}

/// The PROJECTS level's keys: the same walk over project cards, Enter
/// into the project under the cursor, `p` / `n` a box aimed at it, and
/// `k`,`k` off the top row out to the WORKSPACES level.
///
/// A key that acts on a session is swallowed here rather than falling
/// through: the cursor this level moves is the project's, and `a` must
/// not archive a session the user cannot see. [`reaches_here`] is the
/// list of keys that still mean what they always meant.
fn projects_action(
    app: &mut App,
    action: Action,
    armed: Option<(Action, std::time::Instant)>,
    chord: &crate::keymap::KeyChord,
    out: &mut Vec<ClientRequest>,
) -> bool {
    match action {
        Action::MoveDown => step_projects(app, 0, 1),
        Action::MoveUp
            if at_top_card(view::project_cursor(app, &view::project_cards(app)), app) =>
        {
            if double_tapped(app, action, armed, chord, "workspaces") {
                go_up(app, out);
            }
        }
        Action::MoveUp => step_projects(app, 0, -1),
        Action::FocusRight => step_projects(app, 1, 0),
        Action::FocusLeft => step_projects(app, -1, 0),
        Action::HalfPageDown => step_projects(app, 0, HALF_PAGE),
        Action::HalfPageUp => step_projects(app, 0, -HALF_PAGE),
        Action::Activate | Action::FocusNext | Action::FocusTerminal | Action::Zoom => {
            enter_project(app, out)
        }
        Action::FocusPrev => {}
        Action::New | Action::QuickPrompt => open_box(app),
        Action::ToggleProjects
        | Action::ToggleWorktrees
        | Action::ToggleSessions
        | Action::ToggleSidebars => app.flash = Some(NO_PANELS.into()),
        _ if level_walk(app, action, out) => {}
        _ if reaches_here(action) => return false,
        _ => app.flash = Some(swallowed(Level::Projects)),
    }
    true
}

/// The WORKSPACES level's keys: the walk over workspace cards — where
/// moving the cursor IS opening that workspace, as it is in the panels'
/// bar — and Enter down into its projects. `k` on the top row has nowhere
/// to go: this is the top of the tree.
fn workspaces_action(
    app: &mut App,
    action: Action,
    _armed: Option<(Action, std::time::Instant)>,
    _chord: &crate::keymap::KeyChord,
    out: &mut Vec<ClientRequest>,
) -> bool {
    match action {
        Action::MoveDown => step_workspaces(app, 0, 1, out),
        Action::MoveUp => step_workspaces(app, 0, -1, out),
        Action::FocusRight => step_workspaces(app, 1, 0, out),
        Action::FocusLeft => step_workspaces(app, -1, 0, out),
        Action::HalfPageDown => step_workspaces(app, 0, HALF_PAGE, out),
        Action::HalfPageUp => step_workspaces(app, 0, -HALF_PAGE, out),
        Action::Activate | Action::FocusNext | Action::FocusTerminal | Action::Zoom => {
            enter_workspace(app)
        }
        Action::FocusPrev => {}
        Action::ToggleProjects
        | Action::ToggleWorktrees
        | Action::ToggleSessions
        | Action::ToggleSidebars => app.flash = Some(NO_PANELS.into()),
        _ if level_walk(app, action, out) => {}
        _ if reaches_here(action) => return false,
        _ => app.flash = Some(swallowed(Level::Workspaces)),
    }
    true
}

/// Keys that keep their meaning above the SESSIONS level: they act on the
/// app, or on the project under the cursor — which these levels do have —
/// rather than on a session they are not drawing.
fn reaches_here(action: Action) -> bool {
    matches!(
        action,
        Action::Palette
            | Action::Settings
            | Action::Help
            | Action::Quit
            | Action::Metrics
            | Action::Hosts
            | Action::Splash
            | Action::AddProject
            | Action::FindFile
            | Action::Grep
            | Action::TreeBrowser
            | Action::Issues
            | Action::PullRequests
            | Action::GitDiff
            | Action::OpenRepo
            | Action::OpenGhosttyTab
            | Action::RefreshPullRequests
            | Action::SwitchBranch
    )
}

/// What a session key says on a level that is not drawing sessions.
fn swallowed(level: Level) -> String {
    format!(
        "that works on a session — Enter opens the {} under the cursor first",
        level.singular()
    )
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
        app.flash = Some(NO_SESSIONS.into());
        return;
    };
    if Some(next) == at {
        return;
    }
    select(app, rows[next].agent.id.clone(), out);
}

/// The SESSIONS cursor is on the grid's top row — where `k` has nowhere
/// left to go and a second press walks out a level. An empty grid counts:
/// there is nothing under the cursor to move down from.
fn at_top(app: &App) -> bool {
    let rows = view::rows(app);
    at_top_card(view::cursor(app, &rows), app)
}

/// The same for a cursor already in hand: card `at` sits on the grid's
/// first row. No cursor at all counts as the top.
fn at_top_card(at: Option<usize>, app: &App) -> bool {
    at.is_none_or(|at| at < cols(app))
}

// ---- walking the LEVELS ----

/// Esc, and the double tap off the grid's top row: one level back out of
/// the tree — a project's sessions to the projects beside it, those to
/// the workspaces. Already at the top, the footer says so rather than
/// leaving Esc looking broken.
///
/// Nothing is detached on the way out. The pane is not drawn above the
/// SESSIONS level (`launcher::split`), but the session it holds is the
/// one the level came from and the one Enter comes back to, so it keeps
/// running and the walk back in is instant.
pub(super) fn go_up(app: &mut App, out: &mut Vec<ClientRequest>) {
    let _ = out;
    let Some(up) = app.launcher_level.up() else {
        app.flash = Some(AT_TOP.into());
        return;
    };
    go_to(app, up);
}

/// Put the view on `level` — the one move, so a key, a click and Esc all
/// land in the same state. FOCUS comes back to the cards: above the
/// SESSIONS level there is no pane under them for it to sit in.
fn go_to(app: &mut App, level: Level) {
    if app.launcher_level == level {
        return;
    }
    app.launcher_level = level;
    app.focus = Focus::Sessions;
    app.term_locked = false;
    app.dirty = true;
}

/// A click on one of the header breadcrumb's crumbs: the crumb names a
/// tier of the tree and the click OPENS that tier — `nebula` the list of
/// workspaces, a workspace its projects, a project its sessions — so the
/// word clicked and the grid landed on say the same thing, the way a
/// path segment does. It walks through the same [`go_to`] that Esc,
/// `k`,`k` and `w` take, so the pointer and the keys land in the same
/// state, and a crumb naming the level already on screen simply stays
/// put. Never a dialog: the view puts none over its grid.
pub(super) fn click_crumb(app: &mut App, level: Level) {
    go_to(app, level);
}

/// `h` / `j` / `k` / `l` over the PROJECT cards. The cursor it moves is
/// `App::sel_project` — the PROJECTS PANEL's own — so the card under it
/// is what the level below will open on and what every verb that reads
/// the selection sees.
fn step_projects(app: &mut App, dx: i64, dy: i64) {
    let cards = view::project_cards(app);
    let at = view::project_cursor(app, &cards);
    let Some(next) = view::grid_stepped(at, dx, dy, cols(app), cards.len()) else {
        app.flash = Some(NO_PROJECTS.into());
        return;
    };
    if Some(next) == at {
        return;
    }
    let id = cards[next].id.clone();
    select_project(app, &id);
}

/// Put the cursor on project `id`. Nothing is attached: the level draws
/// no pane, so bringing up the project's last session here would boot a
/// PTY nobody is looking at — only Enter, which opens the level that does
/// draw one, brings a session up.
fn select_project(app: &mut App, id: &ProjectId) {
    if !select_project_row_by_id(app, id) {
        app.flash = Some("project no longer exists".into());
        return;
    }
    restore_project_cursors(app);
    app.dirty = true;
}

/// Enter on a project card: into the project — the SESSIONS level, scoped
/// to it, with the cursor on the session the card led with (the one
/// wanting a human, else the one that moved last) and the pane on that
/// session. A project with nothing in it yet opens the BOX on it: the
/// level below would otherwise be an empty grid with no way to fill it.
fn enter_project(app: &mut App, out: &mut Vec<ClientRequest>) {
    let cards = view::project_cards(app);
    let at = view::project_cursor(app, &cards).or((!cards.is_empty()).then_some(0));
    let Some(at) = at else {
        app.flash = Some(NO_PROJECTS.into());
        return;
    };
    let (id, first) = (
        cards[at].id.clone(),
        cards[at].sessions.first().map(|a| a.id.clone()),
    );
    select_project(app, &id);
    go_to(app, Level::Sessions);
    match first {
        Some(id) => select(app, id, out),
        None => open_box(app),
    }
}

/// `h` / `j` / `k` / `l` over the WORKSPACE cards. Moving the cursor here
/// IS opening that workspace — the cards are in tab order and the cursor
/// is `Tree::active_workspace`, exactly as it is in the panels' bar — but
/// quietly: nothing is restored and nothing attaches while the user is
/// still walking the row.
fn step_workspaces(app: &mut App, dx: i64, dy: i64, out: &mut Vec<ClientRequest>) {
    let cards = view::workspace_cards(app);
    let at = view::workspace_cursor(app);
    let Some(next) = view::grid_stepped(at, dx, dy, cols(app), cards.len()) else {
        return;
    };
    if Some(next) == at {
        return;
    }
    let id = cards[next].id.clone();
    select_workspace(app, id, out);
}

/// Put the cursor on workspace `id` — which is to open it, quietly.
fn select_workspace(app: &mut App, id: WorkspaceId, out: &mut Vec<ClientRequest>) {
    if switch_workspace_quietly(app, id, out) {
        app.dirty = true;
    }
}

/// Enter on a workspace card: down into its projects. The cursor is
/// already the open workspace, so there is nothing to switch — what Enter
/// adds is the project this workspace was last left on, which the quiet
/// switch deliberately did not restore while the row was still being
/// walked.
fn enter_workspace(app: &mut App) {
    restore_workspace_project(app);
    restore_project_cursors(app);
    go_to(app, Level::Projects);
}

/// A digit (or ⌘N): open the Nth workspace and land on its projects — the
/// tab it names, without the bar this view never draws.
fn open_workspace_slot(app: &mut App, slot: u8, out: &mut Vec<ClientRequest>) {
    let Some(id) = view::workspace_at(app, usize::from(slot).saturating_sub(1)) else {
        app.flash = Some(format!("no workspace {slot}"));
        return;
    };
    select_workspace(app, id, out);
    enter_workspace(app);
}

/// Put the cursor on session `id`, through the jump the `/` PALETTE takes
/// — so the panels' selection and every verb that reads it agree with the
/// card. FOCUS lands on the grid.
///
/// [`Landing::FocusOnly`]: the card the cursor lands on is the one the
/// PANE under the grid reads, so walking the cards swaps the pane exactly
/// as ↑/↓ down the SESSIONS PANEL previews a row — the same debounced
/// attach, and the same mark-as-seen, since that session is now on screen.
fn select(app: &mut App, id: AgentId, out: &mut Vec<ClientRequest>) {
    jump_to_target_inner(app, PaletteTarget::Session(id), Landing::FocusOnly, out);
    app.focus = Focus::Sessions;
}

/// A click on the card at `index`: the cursor lands there, and a second
/// click on the same card is Enter — down into the PANE along the bottom,
/// where that session is already running.
pub(super) fn click_row(app: &mut App, index: usize, out: &mut Vec<ClientRequest>) {
    let Some(id) = view::agent_at(app, index) else {
        return;
    };
    select(app, id.clone(), out);
    if is_double_click(
        &mut app.last_session_click,
        crate::app::RowKey::Session(nebula_core::SessionRef::Agent(id)),
    ) {
        enter_pane(app, out);
    }
}

/// A right-click's first half, `select_clicked_row`'s arm: the cursor on
/// the card at `index`, as a left click would put it. False off the grid.
pub(super) fn select_row(app: &mut App, index: usize, out: &mut Vec<ClientRequest>) -> bool {
    let Some(id) = view::agent_at(app, index) else {
        return false;
    };
    select(app, id, out);
    true
}

/// A click on the PROJECT card at `index`: the cursor lands there, and a
/// second click on the same card is Enter — into the project. Input is
/// not action: both ends call the same [`select_project`] and
/// [`enter_project`] the keys do.
pub(super) fn click_project_card(app: &mut App, index: usize, out: &mut Vec<ClientRequest>) {
    let Some(id) = view::project_at(app, index) else {
        return;
    };
    select_project(app, &id);
    if is_double_click(&mut app.last_project_click, id) {
        enter_project(app, out);
    }
}

/// The same click's first half, for a right-click's menu. False off the
/// grid.
pub(super) fn select_project_card(app: &mut App, index: usize) -> bool {
    let Some(id) = view::project_at(app, index) else {
        return false;
    };
    select_project(app, &id);
    true
}

/// A click on the WORKSPACE card at `index`: the cursor lands there —
/// which opens that workspace — and a second click is Enter, down into
/// its projects.
pub(super) fn click_workspace_card(app: &mut App, index: usize, out: &mut Vec<ClientRequest>) {
    let Some(id) = view::workspace_at(app, index) else {
        return;
    };
    select_workspace(app, id.clone(), out);
    if is_double_click(&mut app.last_workspace_click, id) {
        enter_workspace(app);
    }
}

/// The same click's first half. False off the grid.
pub(super) fn select_workspace_card(
    app: &mut App,
    index: usize,
    out: &mut Vec<ClientRequest>,
) -> bool {
    let Some(id) = view::workspace_at(app, index) else {
        return false;
    };
    select_workspace(app, id, out);
    true
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
    /// The SESSIONS level's project. A switch to another one takes every
    /// card off the list at once, which [`keep_cursor`] has to tell from
    /// this one card leaving it.
    project: Option<ProjectId>,
}

pub(super) fn cursor_entry(app: &App) -> Option<CursorCard> {
    if app.launcher_level != Level::Sessions {
        return None;
    }
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
pub(super) fn keep_cursor(app: &mut App, before: CursorCard, out: &mut Vec<ClientRequest>) {
    // Not this list any more: the level was walked out of, or the grid
    // has another project's cards up. Every card left it, this one with
    // them, and whatever put the cursor on one of the new ones meant to.
    if app.launcher_level != Level::Sessions
        || app.selected_project().map(|p| p.id.clone()) != before.project
        // A create is still being followed onto its own row (`n`, the
        // box): that landing is the one the user asked for.
        || app.select_when_seen.is_some()
    {
        return;
    }
    let rows = view::rows(app);
    if rows.is_empty() || rows.iter().any(|row| row.agent.id == before.id) {
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
/// `^q` (`leave_terminal_lock`) hands the keys back to the cards, exactly
/// as it does after a click into the pane. `z` ([`open_session`]) is the
/// way to the whole screen.
///
/// The jump attaches the card outright, so a card the pane's debounce had
/// not reached yet is the one the keys reach.
pub(super) fn enter_pane(app: &mut App, out: &mut Vec<ClientRequest>) {
    // A body too short for a pane worth the name draws none ([`has_pane`]):
    // there is nothing under the cards to cross into, so the session takes
    // the whole screen rather than the keys going somewhere off-screen.
    if !has_pane(app) {
        open_session(app, out);
        return;
    }
    let Some(id) = cursor_or_first(app) else {
        app.flash = Some(NO_SESSIONS.into());
        return;
    };
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
    let Some(id) = cursor_or_first(app) else {
        app.flash = Some(NO_SESSIONS.into());
        return;
    };
    jump_to_target(app, PaletteTarget::Session(id), Landing::Attach, out);
    // A Cloud row's Enter is its browser page, not a PTY: the jump has
    // already opened it and there is nothing to full-screen.
    if app.term.is_some() {
        super::zoom_pane(app, out);
    }
}

/// Is the PANE along the bottom on screen? False on a body too short to
/// hold the header, a row of cards and a pane worth the name, where a
/// session is only ever seen full-screen — and before the first draw,
/// which no key beats.
fn has_pane(app: &App) -> bool {
    view::split(app.launcher_body, app.launcher_level, app.launcher_pane_h)
        .1
        .is_some()
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
    if !app.launcher || !key.modifiers.contains(KeyModifiers::CONTROL) {
        return false;
    }
    let back = QuickReturn {
        launch: launch.clone(),
        text: input.as_str().to_string(),
        from_box: true,
    };
    match key.code {
        KeyCode::Char('p' | 'P') => open_project_picker(app, back),
        KeyCode::Char('o' | 'O') => open_model_picker(app, back),
        KeyCode::Char('n' | 'N') => toggle_new_worktree(app, launch.clone(), input.clone()),
        _ => return false,
    }
    true
}

/// `^P`: the PROJECT PICKER over the box. A box already bound to one
/// project's work — an issue's, a pull request's — keeps its project.
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
/// not always the one under the list's cursor (`^P` moves it). The choice
/// sticks for the next box. A PR SESSION's checkout is the DAEMON's to
/// pick, so it has nothing to flip.
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
        match &launch.issue {
            Some(issue) => QuickTarget::NewWorktree {
                branch: crate::branch_name::issue_name(
                    issue.number,
                    &issue.title,
                    &app.project_branches(&project),
                ),
                project,
            },
            None => view::target_for(app, &project, true),
        }
    } else {
        match view::checkout_for(app, &project) {
            Some(worktree) => QuickTarget::Worktree(worktree),
            None => {
                app.flash = Some("no checkout to reuse — keeping the new worktree".into());
                return;
            }
        }
    };
    app.launcher_new_worktree = fresh;
    reopen_with(app, QuickLaunch { target, ..launch }, input);
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
            crate::quick_prompt::reopen(app, back.launch, &back.text);
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
/// on the project you are working in, and the workspace stays open, even
/// when the pick lives in another one. The launch that follows is a
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
    crate::quick_prompt::reopen(app, launch, &back.text);
}

#[cfg(test)]
mod tests {
    use super::super::tests::{buffer_text, hse, seed_tree, with_default_config};
    use super::super::{handle_terminal_event, sweep_target};
    use crate::app::{App, Focus, HitTarget, Overlay, PromptKind};
    use crate::keymap::Action;
    use crate::quick_prompt::{QuickLaunch, QuickTarget};
    use crossterm::event::{KeyCode, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
    use nebula_core::{
        Agent, AgentId, AgentKind, AgentStatus, ClientRequest, Entity, Project, ProjectId,
        ServerEvent, SessionRef, WorkspaceId, Worktree, WorktreeId,
    };
    use ratatui::backend::TestBackend;
    use ratatui::style::Color;
    use ratatui::Terminal;

    /// `seed_tree`'s `demo` project (its root `main`, session `agent-1`),
    /// plus a second checkout of it — `feat`, running `polish-nav`, the
    /// newer session and so the grid's first card — and a second project,
    /// `web`, whose root runs `tidy-css`.
    ///
    /// The SESSIONS level is one project's, so the grid holds demo's two
    /// cards and `web` is a card beside it one level up — which is what
    /// makes both the walk and the walk out testable off one tree. The
    /// view is on; nothing is owed at boot.
    fn two_sessions() -> App {
        let mut app = App::new();
        seed_tree(&mut app);
        seed_feat(&mut app, "/tmp/demo-feat".into());
        seed_web(&mut app);
        app.launcher = true;
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
                    workspace_id: Default::default(),
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

    /// INPUT PARITY: the view has no workspaces, so nothing here reaches
    /// the WORKSPACE SWITCHER — `w`, `⇧W` and the digit tabs each say so,
    /// and so does a click on the footer's `◇ name` nameplate, which opens
    /// the switcher in every other view. Turning the view off gives the
    /// key its switcher back.
    #[test]
    fn the_workspace_keys_walk_the_level_instead_of_a_dialog() {
        with_default_config(|| {
            let mut app = two_sessions();
            hse(
                &mut app,
                ServerEvent::EntityUpserted {
                    entity: Entity::Workspace(nebula_core::Workspace {
                        id: Default::default(),
                        name: "default".into(),
                    }),
                },
            );
            draw(&mut app);
            // `w` walks to the top of the tree rather than dropping the
            // panels' switcher over the grid.
            key(&mut app, KeyCode::Char('w'), KeyModifiers::NONE);
            assert!(app.overlay.is_none(), "w opened {:?}", app.overlay);
            assert_eq!(app.launcher_level, crate::launcher::Level::Workspaces);

            // A digit opens that workspace and lands on its projects —
            // the tab it names, without the bar this view never draws.
            key(&mut app, KeyCode::Char('1'), KeyModifiers::NONE);
            assert!(app.overlay.is_none(), "1 opened {:?}", app.overlay);
            assert_eq!(app.launcher_level, crate::launcher::Level::Projects);

            // ⇧W still has a bar to fold that is never drawn.
            app.flash = None;
            key(&mut app, KeyCode::Char('W'), KeyModifiers::SHIFT);
            assert!(app.overlay.is_none());
            assert_eq!(app.flash.as_deref(), Some(super::WORKSPACES_LEVEL));
            app.launcher_level = crate::launcher::Level::Sessions;

            // The footer's nameplate belongs to the panels too, so it
            // lands on the same refusal the keys do. (The header's own
            // workspace crumb does not — it is the view's way in, and
            // `the_header_crumbs_open_their_switchers` is where it opens.)
            let (rect, _) = *app
                .hits
                .iter()
                .find(|(_, hit)| *hit == HitTarget::FooterWorkspace)
                .expect("the footer nameplate was drawn");
            app.flash = None;
            mouse(
                &mut app,
                MouseEventKind::Down(MouseButton::Left),
                rect.x,
                rect.y,
            );
            assert!(app.overlay.is_none(), "{:?}", app.overlay);
            assert_eq!(app.flash.as_deref(), Some(super::WORKSPACES_LEVEL));

            // Off, `w` is the switcher again.
            app.launcher = false;
            draw(&mut app);
            key(&mut app, KeyCode::Char('w'), KeyModifiers::NONE);
            assert!(
                matches!(&app.overlay, Some(Overlay::Menu(menu)) if menu.is_workspace_picker()),
                "the panels still switch workspaces: {:?}",
                app.overlay
            );
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

    /// With the view on, nebula opens on the box — up at the first
    /// snapshot that has a project, aimed at that project and at a fresh
    /// worktree, the harness the settings name — and only once: Esc
    /// leaves it for the list, and later snapshots do not bring it back.
    #[test]
    fn the_box_is_up_at_boot_aimed_at_a_fresh_worktree() {
        with_default_config(|| {
            let mut app = App::new();
            app.launcher = true;
            app.launcher_boot = true;
            seed_tree(&mut app);
            let (launch, text) = launch(&app);
            assert!(text.is_empty());
            assert!(matches!(
                &launch.target,
                QuickTarget::NewWorktree { project, .. } if project.0 == "p1"
            ));
            assert_eq!(launch.kind, AgentKind::Claude, "the settings' harness");
            assert!(!app.launcher_boot, "owed once");

            key(&mut app, KeyCode::Esc, KeyModifiers::NONE);
            assert!(app.overlay.is_none());
            seed_web(&mut app);
            assert!(app.overlay.is_none(), "not owed again");

            // Off, nothing is owed and nothing opens.
            let mut app = App::new();
            seed_tree(&mut app);
            assert!(app.overlay.is_none());
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
            assert!(
                text.contains("nebula / default / demo / sessions"),
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
            assert!(
                text.contains("nebula / default / demo / sessions"),
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
                !text.contains("/ sessions  "),
                "the grid's breadcrumb is gone: {text}"
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
            assert!(
                text.contains("nebula / default / demo / sessions"),
                "the grid, not the panels: {text}"
            );
            assert!(text.contains("2 sessions"), "{text}");
        });
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

    /// INPUT PARITY: the header's breadcrumb is the way back up the tree
    /// with the pointer — a click on a crumb lands in exactly the state
    /// the keys land in, since both go through `go_to`. A crumb opens
    /// what its word names: the workspace crumb its projects (`Esc`), the
    /// `nebula` crumb the workspaces (`Esc`,`Esc`, or `w`), and neither
    /// puts a dialog over the grid.
    #[test]
    fn the_header_crumbs_walk_back_up_the_tree() {
        with_default_config(|| {
            use crate::launcher::Level;
            let mut app = two_sessions();
            hse(
                &mut app,
                ServerEvent::EntityUpserted {
                    entity: Entity::Workspace(nebula_core::Workspace {
                        id: Default::default(),
                        name: "default".into(),
                    }),
                },
            );
            let text = buffer_text(&draw(&mut app));
            assert!(
                text.contains("nebula / default / demo / sessions"),
                "{text}"
            );

            // The project crumb names the project whose sessions are
            // already on screen, so the click is a no-op rather than a
            // step out to the list the project sits in.
            let (x, y) = crumb_cell(&app, HitTarget::LauncherProject);
            mouse(&mut app, MouseEventKind::Down(MouseButton::Left), x, y);
            assert_eq!(app.launcher_level, Level::Sessions);

            // The workspace crumb opens that workspace: its projects.
            let (x, y) = crumb_cell(&app, HitTarget::LauncherWorkspace);
            mouse(&mut app, MouseEventKind::Down(MouseButton::Left), x, y);
            assert_eq!(app.launcher_level, Level::Projects);
            assert!(app.overlay.is_none(), "no dialog: {:?}", app.overlay);
            let text = buffer_text(&draw(&mut app));
            assert!(text.contains("nebula / default / projects"), "{text}");

            // And `nebula` is the machine: every workspace it knows.
            let (x, y) = crumb_cell(&app, HitTarget::LauncherRoot);
            mouse(&mut app, MouseEventKind::Down(MouseButton::Left), x, y);
            assert_eq!(app.launcher_level, Level::Workspaces);
            assert!(app.overlay.is_none(), "no dialog: {:?}", app.overlay);
            let text = buffer_text(&draw(&mut app));
            assert!(text.contains("nebula / workspaces"), "{text}");

            // Two levels in one click on `nebula`, and it matches the two
            // Escs that walk the same path.
            let mut by_click = two_sessions();
            draw(&mut by_click);
            let (x, y) = crumb_cell(&by_click, HitTarget::LauncherRoot);
            mouse(&mut by_click, MouseEventKind::Down(MouseButton::Left), x, y);
            let mut by_key = two_sessions();
            draw(&mut by_key);
            key(&mut by_key, KeyCode::Esc, KeyModifiers::NONE);
            key(&mut by_key, KeyCode::Esc, KeyModifiers::NONE);
            assert_eq!(by_click.launcher_level, by_key.launcher_level);
            assert_eq!(by_click.focus, by_key.focus);
            assert_eq!(by_click.term_locked, by_key.term_locked);
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
                crate::launcher::split(app.launcher_body, app.launcher_level, app.launcher_pane_h)
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
        with_default_config(|| {
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
        with_default_config(|| {
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

    /// The same with `^N` off, where there is no checkout to cut and the
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
            // Into web's own checkout rather than a fresh worktree.
            key(&mut app, KeyCode::Char('n'), KeyModifiers::CONTROL);
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

    /// Picking a project that lives in another workspace aims the box
    /// there without opening that workspace: the grid behind the box goes
    /// on showing the work in front of the user.
    #[test]
    fn picking_a_project_in_another_workspace_leaves_the_workspace_open() {
        with_default_config(|| {
            let mut app = two_sessions();
            let elsewhere: WorkspaceId = "ws2".to_string().into();
            app.tree.workspaces.push(nebula_core::Workspace {
                id: elsewhere.clone(),
                name: "side".into(),
            });
            app.tree.projects.push(Project {
                workspace_id: elsewhere.clone(),
                id: ProjectId("p3".into()),
                name: "away".into(),
                repo_path: "/tmp/away".into(),
                sort_order: 2,
            });
            let open = app.tree.active_workspace.clone();
            draw(&mut app);

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
            assert_eq!(app.tree.active_workspace, open, "the workspace stayed open");
            assert!(
                !out.iter()
                    .any(|r| matches!(r, ClientRequest::OpenWorkspace { .. })),
                "and nothing asked the DAEMON to switch: {out:?}"
            );
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

    /// `^N` flips the box between a fresh worktree and the project's own
    /// checkout — the project the box is aimed at — and the next box
    /// starts the way this one was left.
    #[test]
    fn ctrl_n_flips_to_the_projects_checkout_and_is_remembered() {
        with_default_config(|| {
            let mut app = two_sessions();
            draw(&mut app);
            key(&mut app, KeyCode::Char('p'), KeyModifiers::NONE);
            let (launch, _) = launch(&app);
            assert!(launch.is_new_worktree());
            key(&mut app, KeyCode::Char('n'), KeyModifiers::CONTROL);
            let (launch, _) = self::launch(&app);
            let QuickTarget::Worktree(worktree) = &launch.target else {
                panic!("expected an existing checkout, got {:?}", launch.target);
            };
            let project = crate::launcher::project_of(&app, &launch.target);
            assert_eq!(project, app.selected_project().map(|p| p.id.clone()));
            assert!(app.tree.worktrees.iter().any(|w| &w.id == worktree));
            assert!(!app.launcher_new_worktree);

            key(&mut app, KeyCode::Esc, KeyModifiers::NONE);
            key(&mut app, KeyCode::Char('p'), KeyModifiers::NONE);
            let (launch, _) = self::launch(&app);
            assert!(!launch.is_new_worktree(), "the next box remembers");
        });
    }

    /// The panel toggles have no panels to fold here, and say so.
    #[test]
    fn the_panel_toggles_say_there_are_no_panels() {
        let mut app = two_sessions();
        let mut out = Vec::new();
        assert!(super::handle_action(
            &mut app,
            Action::ToggleProjects,
            None,
            &crate::keymap::KeyChord::from_event(&crossterm::event::KeyEvent::new(
                KeyCode::Char('P'),
                KeyModifiers::SHIFT
            )),
            &mut out
        ));
        assert_eq!(app.flash.as_deref(), Some(super::NO_PANELS));
    }

    /// The grid names each session's place, the harness it runs on and
    /// its pull request, with the count and the needs-you tally in the
    /// header — and no panels anywhere. Off, the panels are back.
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
            assert!(
                text.contains("nebula / default / demo / sessions"),
                "the header names the workspace and the project: {text}"
            );
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

            app.launcher = false;
            let text = buffer_text(&draw(&mut app));
            assert!(text.contains("PROJECTS"), "{text}");
        });
    }

    /// The header's STATUS TALLY, on the header row: one dot per state
    /// with a card in it, carrying that state's count and no word — so
    /// what is waiting on a human is the red dot, and reading it means
    /// reading the color the cell is painted in.
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
            // `  ● 12`: the count starts two cells along and runs as far
            // as the digits do, so the `2 sessions` off on the right edge
            // is never read as part of it.
            let mut count = String::new();
            let mut i = x + 2;
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

    /// The header counts what is waiting on a human in the STATUS TALLY's
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

    /// However narrow the terminal, the breadcrumb and the count keep
    /// off each other: the crumbs are squeezed, then dropped from the
    /// right — the project first, then the workspace — rather than drawn
    /// over the count (the two are separate right/left-aligned
    /// paragraphs on one row, so an overlong crumb would overprint it).
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
                // The crumbs end before the count begins: the gap between
                // the two is real air, not a letter eaten by one of them.
                let crumbs = head.split("2 sessions").next().unwrap_or_default();
                assert!(
                    crumbs.ends_with("  "),
                    "{width}: crumbs run into the count: {head:?}"
                );
                assert!(
                    crumbs.trim_start().starts_with("nebula"),
                    "{width}: {head:?}"
                );
            }
            // Wide enough for everything, narrow enough that the project
            // gives way first and the workspace survives alone.
            let wide = buffer_text(&draw_at(&mut app, 130, 34));
            assert!(
                wide.contains("nebula / default / demo / sessions"),
                "{wide}"
            );
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

    /// The box, in the view: each of the three chord-changed details on
    /// its first row beside the chord that changes it, and the prompt
    /// header under them naming where the launch lands and the toggle
    /// that cuts a fresh checkout. None of those chords is repeated on
    /// the border — that repetition was the box's wall of text.
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
            assert!(text.contains("(demo / "), "where the launch lands: {text}");
            assert!(!text.contains("^P project"), "not twice over: {text}");
            assert!(!text.contains("^O model"), "not twice over: {text}");

            key(&mut app, KeyCode::Char('p'), KeyModifiers::CONTROL);
            let text = buffer_text(&draw(&mut app));
            assert!(text.contains("type a project name"), "{text}");
            assert!(text.contains("web"), "{text}");
        });
    }

    /// A cell inside the PROJECT card at `index`, as the level drew it.
    fn project_cell(app: &App, index: usize) -> (u16, u16) {
        let (rect, _) = app
            .hits
            .iter()
            .find(|(_, hit)| *hit == HitTarget::LauncherProjectCard(index))
            .unwrap_or_else(|| panic!("project card {index} was not drawn"));
        (rect.x + 3, rect.y + 1)
    }

    /// `k` on the grid's top row has nowhere left to go, so a second press
    /// walks out to the PROJECTS level — the same double tap `k`,`k` on a
    /// panel's first row is, and the first press says so in the footer
    /// rather than moving anything.
    #[test]
    fn k_k_off_the_top_row_walks_out_to_the_projects() {
        with_default_config(|| {
            let mut app = two_sessions();
            draw(&mut app);
            // The cursor starts on the older card, on the same top row as
            // the newer one: both are on row 0 here.
            key(&mut app, KeyCode::Char('k'), KeyModifiers::NONE);
            assert_eq!(
                app.launcher_level,
                crate::launcher::Level::Sessions,
                "one press only arms"
            );
            assert_eq!(
                app.flash.as_deref(),
                Some("k again: projects"),
                "and says what the second one does"
            );

            key(&mut app, KeyCode::Char('k'), KeyModifiers::NONE);
            assert_eq!(app.launcher_level, crate::launcher::Level::Projects);

            // Slowly, it is two single presses: the arm is only good for
            // `DOUBLE_TAP`, and anything in between breaks it too.
            let mut app = two_sessions();
            draw(&mut app);
            key(&mut app, KeyCode::Char('k'), KeyModifiers::NONE);
            key(&mut app, KeyCode::Char('l'), KeyModifiers::NONE);
            key(&mut app, KeyCode::Char('k'), KeyModifiers::NONE);
            assert_eq!(
                app.launcher_level,
                crate::launcher::Level::Sessions,
                "a key in between breaks the tap"
            );
        });
    }

    /// Esc walks one level out of the tree each press — a project's
    /// sessions, the projects beside it, the workspaces — and stops at the
    /// top rather than looking broken. Enter walks back in along the same
    /// path.
    #[test]
    fn esc_walks_up_the_tree_and_enter_walks_back_in() {
        with_default_config(|| {
            use crate::launcher::Level;
            let mut app = two_sessions();
            draw(&mut app);
            assert_eq!(app.launcher_level, Level::Sessions);

            key(&mut app, KeyCode::Esc, KeyModifiers::NONE);
            assert_eq!(app.launcher_level, Level::Projects);
            key(&mut app, KeyCode::Esc, KeyModifiers::NONE);
            assert_eq!(app.launcher_level, Level::Workspaces);

            app.flash = None;
            key(&mut app, KeyCode::Esc, KeyModifiers::NONE);
            assert_eq!(app.launcher_level, Level::Workspaces, "the top of the tree");
            assert!(app.flash.is_some(), "and it says so");

            draw(&mut app);
            key(&mut app, KeyCode::Enter, KeyModifiers::NONE);
            assert_eq!(app.launcher_level, Level::Projects);
            draw(&mut app);
            key(&mut app, KeyCode::Enter, KeyModifiers::NONE);
            assert_eq!(app.launcher_level, Level::Sessions);
        });
    }

    /// The PROJECTS level draws a card per project, naming the sessions
    /// under it, and the header counts them. Walking the cards moves the
    /// project cursor; Enter goes into the project, and the SESSIONS level
    /// below is that project's alone.
    #[test]
    fn entering_a_project_scopes_the_grid_to_it() {
        with_default_config(|| {
            let mut app = two_sessions();
            draw(&mut app);
            key(&mut app, KeyCode::Esc, KeyModifiers::NONE);
            let text = buffer_text(&draw(&mut app));
            assert!(
                text.contains("nebula / default / projects"),
                "the crumb stops at the level: {text}"
            );
            assert!(text.contains("2 projects"), "{text}");
            // Each card names its project and the sessions under it.
            assert!(text.contains("demo"), "{text}");
            assert!(text.contains("polish-nav"), "{text}");
            assert!(text.contains("web"), "{text}");
            assert!(text.contains("tidy-css"), "{text}");

            // The cursor starts on the selected project and `l` walks to
            // the one beside it.
            assert_eq!(
                app.selected_project().map(|p| p.name.as_str()),
                Some("demo")
            );
            key(&mut app, KeyCode::Char('l'), KeyModifiers::NONE);
            assert_eq!(app.selected_project().map(|p| p.name.as_str()), Some("web"));

            // Enter goes in, and the grid below is web's one session.
            key(&mut app, KeyCode::Enter, KeyModifiers::NONE);
            assert_eq!(app.launcher_level, crate::launcher::Level::Sessions);
            assert_eq!(selected(&app).as_deref(), Some("a3"), "web's own session");
            let text = buffer_text(&draw(&mut app));
            assert!(text.contains("nebula / default / web / sessions"), "{text}");
            assert!(text.contains("1 session"), "{text}");
            assert!(text.contains("tidy-css"), "{text}");
            assert!(
                !text.contains("polish-nav"),
                "demo's sessions are a level away: {text}"
            );
        });
    }

    /// A click on a project card is the keys that walk to it, and a second
    /// click is its Enter — one function per intent, whichever end it
    /// comes from.
    #[test]
    fn a_click_on_a_project_card_is_the_keys_that_walk_to_it() {
        with_default_config(|| {
            let mut by_key = two_sessions();
            draw(&mut by_key);
            key(&mut by_key, KeyCode::Esc, KeyModifiers::NONE);
            draw(&mut by_key);
            key(&mut by_key, KeyCode::Char('l'), KeyModifiers::NONE);

            let mut by_click = two_sessions();
            draw(&mut by_click);
            key(&mut by_click, KeyCode::Esc, KeyModifiers::NONE);
            draw(&mut by_click);
            let (x, y) = project_cell(&by_click, 1);
            mouse(&mut by_click, MouseEventKind::Down(MouseButton::Left), x, y);

            assert_eq!(by_click.sel_project, by_key.sel_project);
            assert_eq!(by_click.launcher_level, by_key.launcher_level);
            assert_eq!(by_click.focus, by_key.focus);

            key(&mut by_key, KeyCode::Enter, KeyModifiers::NONE);
            mouse(&mut by_click, MouseEventKind::Down(MouseButton::Left), x, y);
            assert_eq!(
                by_click.launcher_level, by_key.launcher_level,
                "the second click is Enter"
            );
            assert_eq!(selected(&by_click), selected(&by_key));
        });
    }

    /// The WORKSPACES level draws a card per workspace naming its
    /// projects, and Enter on one opens it and lands on its projects.
    #[test]
    fn the_workspaces_level_names_each_workspaces_projects() {
        with_default_config(|| {
            let mut app = two_sessions();
            hse(
                &mut app,
                ServerEvent::EntityUpserted {
                    entity: Entity::Workspace(nebula_core::Workspace {
                        id: Default::default(),
                        name: "default".into(),
                    }),
                },
            );
            draw(&mut app);
            key(&mut app, KeyCode::Char('w'), KeyModifiers::NONE);
            let text = buffer_text(&draw(&mut app));
            assert!(
                text.contains("nebula / workspaces"),
                "the crumb is the whole path at the top: {text}"
            );
            assert!(text.contains("1 workspace"), "{text}");
            assert!(text.contains("default"), "{text}");
            assert!(
                text.contains("demo") && text.contains("web"),
                "the card names the projects under it: {text}"
            );
            // Above the sessions there is no pane to read.
            assert!(
                !text.contains("SESSION · polish-nav"),
                "no pane above the sessions: {text}"
            );
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
        app.launcher = true;
        app.sel_project = project_row(&app, "p1");
        let (id, _) = sweep_target(&mut app).expect("demo's other checkout is listed");
        assert_eq!(id, WorktreeId("w2".into()));

        // Walked into `web`, whose only checkout is its root: the sweep
        // went with the level and has nothing to spend the tick on.
        app.sel_project = project_row(&app, "p2");
        assert_eq!(sweep_target(&mut app), None, "the sweep followed the level");
    }

    /// A project's place in the PROJECTS PANEL's row order.
    fn project_row(app: &App, id: &str) -> usize {
        app.project_rows()
            .iter()
            .position(|i| app.tree.projects[*i].id.0 == id)
            .expect("the project has a row")
    }
}
