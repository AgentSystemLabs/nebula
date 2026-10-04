//! The PANELS' keys and clicks (`crate::panels` is the layout's model,
//! `ui::panels_view` its drawing). Most of what the panels answer to is
//! the panel walk `event_loop::handle_key` has always kept under the GRID —
//! `h`/`l` across the columns (`focus_walk`), `j`/`k` down them
//! (`move_selection`), Enter into the pane, Space expanding a session's
//! pill into its FOLLOW-UP COMPOSER (`activate::follow_up`, whose box then
//! takes the keys, `follow_up_key`), and every verb that reads the
//! selection — so this module only takes the keys the GRID owns and gives
//! them their panel meaning, or a word saying they have none here
//! ([`handle_action`]), folds a column to its RAIL and opens it again
//! ([`fold_key`], [`click_fold`]), answers the keys the columns always
//! had that the GRID has not ([`panel_key`]), and translates a click on a row
//! ([`click_row`]), a drag of a column's BORDER ([`grab_border`],
//! [`move_border`]) or a notch of the wheel over a column ([`wheel`]).

use super::{
    activate, attach_selected, context_menu_items, is_double_click, jump_attention,
    open_ghostty_tab, open_menu, select_project_row, select_session_row, select_worktree_row,
    toggle_issues, toggle_open_prs, walk_focus_back, walk_focus_forward, zoom_pane,
    KEYBOARD_MENU_ANCHOR,
};
use crate::app::{App, Focus, HitTarget, RowKey};
use crate::keymap::{Action, KeyChord};
use crate::panels::{Fold, PanelKey, Row};
use nebula_core::ClientRequest;
use std::time::Duration;

/// What the GRID's PROJECT TAB keys (`x`, the digits, `+`) say beside the
/// columns: the PROJECTS column is where the projects are.
pub(super) const NO_TABS_IN_PANELS: &str =
    "no project tabs in the panels — the PROJECTS column lists every project";
/// What the pane fold (`^``) says: the panels' pane is not one that folds.
const NO_FOLD_IN_PANELS: &str = "the panels' pane doesn't fold — ^F full-screens it";
/// What `` ` `` says: a checkout's terminals are rows of the SESSIONS
/// column here, not chips over the pane.
const NO_PANE_TABS_IN_PANELS: &str = "terminals are rows under TERMINALS in the SESSIONS column";
/// What `^F` says with nothing in the pane to full-screen.
const NOTHING_TO_FULL_SCREEN: &str = "no session in the pane — j/k onto one, then ^F";

/// A panel key while the PANELS are up — true when it was taken here. Only
/// the keys the GRID's own handler owns (`launcher::handle_action`) and
/// that mean something else beside the columns come through here; every
/// other key falls through to its panel meaning, which reads the same
/// selection the columns' cursors are.
///
/// * `^F`: the session under the cursor full-screen ([`toggle_full_screen`]).
/// * `]` / `[`: the attention walk, the meaning they had in the panels —
///   the same ring `.` / `,` walk.
/// * `⇧A`: the SESSIONS column's ARCHIVED group, opened or folded in
///   place ([`toggle_archived`]) — not the grid's swap to a list of every
///   archived session in the project.
/// * The PROJECT TAB keys, the pane fold and the pane's terminal strip
///   have nothing to act on here, and say so.
pub(super) fn handle_action(app: &mut App, action: Action, out: &mut Vec<ClientRequest>) -> bool {
    // Whatever the key does, the focused column reveals its cursor again:
    // the walk that follows a wheel brings the cursor back on screen even
    // from the column's end, where it has nowhere to move to.
    if app.focus != Focus::Terminal {
        app.panels_scroll[crate::panels::scroll_slot(app.focus)].reveal_next();
    }
    match action {
        Action::ToggleFullScreen => toggle_full_screen(app, out),
        Action::NextProjectTab | Action::PrevProjectTab => {
            let step = if action == Action::NextProjectTab {
                1
            } else {
                -1
            };
            let attaches = crate::config::Config::load().palette_enter_attaches;
            jump_attention(app, step, attaches, out);
        }
        Action::ToggleArchived => toggle_archived(app, out),
        Action::CloseProjectTab | Action::SelectProjectTab(_) | Action::ProjectDropdown => {
            app.flash = Some(NO_TABS_IN_PANELS.into());
        }
        Action::ToggleLauncherPane => app.flash = Some(NO_FOLD_IN_PANELS.into()),
        Action::PaneTabs => app.flash = Some(NO_PANE_TABS_IN_PANELS.into()),
        _ => return false,
    }
    true
}

/// A fold key while the PANELS are up — true when `chord` was one and was
/// taken here, ahead of the keymap: `⇧P` / `⇧B` / `⇧S` fold PROJECTS /
/// WORKTREES / SESSIONS to their RAILS or open them again ([`toggle`]),
/// `^B` / `⌘B` / `⇧Z` every column at once ([`toggle_all`]). The GRID never
/// asks, so it keeps whatever the keymap binds the chords to.
pub(super) fn fold_key(app: &mut App, chord: &KeyChord) -> bool {
    if let Some(i) = crate::panels::FOLD_KEYS.iter().position(|k| k == chord) {
        toggle(app, i);
    } else if crate::panels::FOLD_ALL_KEYS.contains(chord) {
        toggle_all(app);
    } else {
        return false;
    }
    true
}

/// One of the PANELS' own keys while they are up — true when `chord` was
/// one and was taken here, ahead of the keymap, as [`fold_key`] takes its
/// own (`panels::PANEL_KEYS`): the backward walk (`⇧Tab` / `^⇧H`) and
/// the forward one's `^⇧L`, `^→` onto the next column or into the pane
/// without the input lock, `z` the pane FULL-SCREEN, `m` the cursor row's
/// CONTEXT MENU ([`open_row_menu`]) and `⇧C` a Ghostty tab. The GRID never
/// asks, so it keeps whatever the keymap binds the chords to.
pub(super) fn panel_key(app: &mut App, chord: &KeyChord, out: &mut Vec<ClientRequest>) -> bool {
    let Some(&(_, key)) = crate::panels::PANEL_KEYS.iter().find(|(k, _)| k == chord) else {
        return false;
    };
    // The focused column reveals its cursor again, as every key that
    // reaches `handle_action` makes it.
    if app.focus != Focus::Terminal {
        app.panels_scroll[crate::panels::scroll_slot(app.focus)].reveal_next();
    }
    match key {
        PanelKey::FocusPrev => walk_focus_back(app),
        PanelKey::FocusNext => walk_focus_forward(app, out),
        PanelKey::FocusTerminal => app.focus = app.next_visible_focus(app.focus),
        PanelKey::Zoom => {
            if app.term.is_some() {
                zoom_pane(app, out);
            } else {
                app.flash = Some(ATTACH_FIRST.into());
            }
        }
        PanelKey::ContextMenu => open_row_menu(app),
        PanelKey::OpenGhosttyTab => open_ghostty_tab(app),
    }
    app.dirty = true;
    true
}

/// What `z` says with nothing in the pane to full-screen.
const ATTACH_FIRST: &str = "attach a session first";

/// `m`: the CONTEXT MENU of the row under the focused column's cursor —
/// the one a right-click on that row opens (`context_menu_items`) — at
/// the fixed spot near the columns' top left a keyboard menu always hung
/// at (`KEYBOARD_MENU_ANCHOR`). A column with no row under its cursor has
/// no menu, and neither has the pane; the column's own verbs are its
/// empty background's right-click.
fn open_row_menu(app: &mut App) {
    if let Some(items) = context_menu_items(app, app.focus) {
        open_menu(app, items, KEYBOARD_MENU_ANCHOR);
    }
}

/// Column `i` folded to its RAIL, or opened back up to the width it was
/// dragged to: its `⇧` key, the `◀` on its title and the rail itself all
/// come here, and so the choice is written to CONFIG.JSON however it was
/// made ([`save_folds`]), surviving a restart.
fn toggle(app: &mut App, i: usize) {
    set_hidden(app, i, !app.panels_hidden[i]);
    save_folds(app);
}

/// `^B`: every column folded to its RAIL, the pane taking the whole body
/// — FOCUS left on a column goes into the pane — or, with none open,
/// every one opened again, FOCUS left where it is.
fn toggle_all(app: &mut App) {
    if crate::panels::folds(app).contains(&Fold::Open) {
        for i in 0..3 {
            set_hidden(app, i, true);
        }
        if !app.focus_visible(app.focus) {
            app.focus = Focus::Terminal;
        }
        app.flash = Some("panels collapsed".into());
    } else {
        for i in 0..3 {
            set_hidden(app, i, false);
        }
        app.flash = Some("panels expanded".into());
    }
    save_folds(app);
}

/// Column `i` folded by hand, or opened: a FOCUS on a column folding
/// under it steps on to the next open one, the pane past the last.
pub(super) fn set_hidden(app: &mut App, i: usize, hidden: bool) {
    app.panels_hidden[i] = hidden;
    let focus = [Focus::Projects, Focus::Worktrees, Focus::Sessions][i];
    if hidden && app.focus == focus && app.panels_active() {
        app.focus = app.next_visible_focus(focus);
    }
    app.dirty = true;
}

/// CONFIG.JSON's `hide_projects`, `hide_worktrees` and `hide_sessions`
/// written from the columns' folds, so the next start opens them as they
/// were left.
fn save_folds(app: &mut App) {
    let mut cfg = crate::config::Config::load();
    [cfg.hide_projects, cfg.hide_worktrees, cfg.hide_sessions] = app.panels_hidden;
    if let Err(err) = cfg.save() {
        app.flash = Some(format!("couldn't save settings: {err}"));
    }
}

/// A click on a column's fold button (`HitTarget::PanelsFold`): the `◀` on
/// an open column's title folds it — FOCUS taken first, so a cursor there
/// steps off it as the key would move it — and a RAIL opens again, FOCUS
/// left where it was.
pub(super) fn click_fold(app: &mut App, focus: Focus) {
    let Some(i) = crate::panels::column_index(focus) else {
        return;
    };
    if app.focus_visible(focus) {
        app.focus = focus;
    }
    toggle(app, i);
}

/// `^F` beside the columns: the session in the pane full-screen with the
/// input lock on — from the SESSIONS column the row under the cursor,
/// attached first, exactly as Enter on it would; from the pane, or the
/// columns above, whatever the pane is showing. `^F` from the full-screen
/// session comes back down to the pane with the keys still in it
/// (`launcher::toggle_full_screen`, which the locked pane runs); `^q`
/// leaves it for the columns, as it leaves `z`'s.
fn toggle_full_screen(app: &mut App, out: &mut Vec<ClientRequest>) {
    if app.focus == Focus::Sessions {
        let Some(row) = app.selected_session_row() else {
            app.flash = Some(NOTHING_TO_FULL_SCREEN.into());
            return;
        };
        if row.is_archived_agent() {
            app.flash = Some(super::AGENT_ARCHIVED.into());
            return;
        }
        // A link or a Cloud row leads out to the browser and leaves no
        // PTY in the pane: there is nothing to full-screen then.
        attach_selected(app, out);
        if app.focus != Focus::Terminal {
            return;
        }
    }
    if app.term.is_none() {
        app.flash = Some(NOTHING_TO_FULL_SCREEN.into());
        return;
    }
    zoom_pane(app, out);
    app.dirty = true;
}

/// `⇧A`, and a click on the `ARCHIVED` header: the SESSIONS column's
/// ARCHIVED group opened under the live rows, or folded back to its one
/// line. The cursor keeps its row; one that was on an archived row the
/// fold took away lands on the last row left, with its session in the
/// pane.
pub(super) fn toggle_archived(app: &mut App, out: &mut Vec<ClientRequest>) {
    app.show_archived = !app.show_archived;
    let len = app.visible_session_rows().len();
    if len > 0 && app.sel_session >= len {
        select_session_row(app, len - 1, Duration::ZERO, out);
    }
    app.dirty = true;
}

/// The cursor onto the PANELS row a click — either button — landed on, its
/// column taking FOCUS: the move the arrow keys make onto it
/// (`select_project_row`, `select_worktree_row`, `select_session_row`),
/// with the context it brings — the project's checkouts, the checkout's
/// session in the pane. False for a header, a session's FOLLOW-UP CHEVRON
/// and its open composer, which have no cursor to move and no menu of
/// their own.
pub(super) fn select_row(app: &mut App, row: Row, out: &mut Vec<ClientRequest>) -> bool {
    match row {
        Row::Project(i) => {
            if i != app.sel_project {
                select_project_row(app, i, out);
            }
        }
        Row::Worktree(i) => {
            if i != app.sel_worktree {
                select_worktree_row(app, i, out);
            }
        }
        Row::Session(i) => {
            // A pointer moved onto another pill is the one way the cursor
            // leaves an open FOLLOW-UP COMPOSER while it holds the
            // keyboard, so it folds: a box on one pill with the cursor on
            // another would leave `j` typing a letter instead of moving.
            if app.follow_up_row().is_some_and(|open| open != i) {
                app.follow_up = None;
            }
            select_session_row(app, i, Duration::ZERO, out)
        }
        Row::OpenPrsHeader
        | Row::IssuesHeader
        | Row::ArchivedHeader
        | Row::FollowUp(_)
        | Row::FollowUpBox => return false,
    }
    app.focus = row.focus();
    app.dirty = true;
    true
}

/// A left click on the PANELS: a row takes the cursor ([`select_row`]),
/// and a second click on the same one is Enter on it — a checkout hands
/// FOCUS to its sessions, a pull request or an issue opens in the
/// browser, a session is attached and takes the keys, a link opens; an
/// archived session says why neither happens. Landing on any other row
/// breaks the chain, so a click away and back never reads as a
/// double-click. A group header folds its group, or opens it; a session's
/// FOLLOW-UP CHEVRON puts the cursor on its row and expands the pill or
/// folds it — `activate::follow_up`, exactly what Space on it does — and
/// a click inside the open composer only gives SESSIONS FOCUS.
pub(super) fn click_row(app: &mut App, row: Row, out: &mut Vec<ClientRequest>) {
    match row {
        Row::OpenPrsHeader => toggle_open_prs(app, out),
        Row::IssuesHeader => toggle_issues(app, out),
        Row::ArchivedHeader => toggle_archived(app, out),
        Row::FollowUp(i) => {
            select_row(app, Row::Session(i), out);
            activate::follow_up(app);
        }
        Row::FollowUpBox => {
            app.focus = Focus::Sessions;
            app.dirty = true;
        }
        Row::Project(_) => {
            select_row(app, row, out);
        }
        Row::Worktree(_) => {
            select_row(app, row, out);
            let key = match app.selected_worktree_pr() {
                Some(pr) => Some(RowKey::Link(pr.url.clone())),
                None => match app.selected_worktree_issue() {
                    Some(issue) => Some(RowKey::Link(issue.url.clone())),
                    None => app
                        .selected_worktree()
                        .map(|w| RowKey::Worktree(w.id.clone())),
                },
            };
            match key {
                Some(key) => {
                    if is_double_click(&mut app.last_session_click, key) {
                        activate::worktrees_row(app, out);
                    }
                }
                None => app.last_session_click = None,
            }
        }
        Row::Session(_) => {
            select_row(app, row, out);
            match app.selected_session_row() {
                Some(row) if row.is_archived_agent() => {
                    app.flash = Some(super::AGENT_ARCHIVED.into());
                }
                Some(row) => {
                    let key = match row.sref() {
                        Some(sref) => RowKey::Session(sref),
                        None => RowKey::Link(row.name().to_string()),
                    };
                    if is_double_click(&mut app.last_session_click, key) {
                        attach_selected(app, out);
                    }
                }
                None => {}
            }
        }
    }
}

/// A press on a PANELS column's BORDER (`HitTarget::PanelsBorder`) at
/// screen column `x`: a resize drag armed, as quietly as the LAUNCHER
/// VIEW's pane edge arms one — no row selected, no FOCUS taken. The offset
/// from the grabbed cell to the border is kept so the border does not jump
/// by one depending on which of its two grab cells was caught; the border
/// is measured by the arithmetic the draw laid it out with.
pub(super) fn grab_border(app: &mut App, column: usize, x: u16) {
    let border = crate::panels::layout(app, app.body_area).border(column);
    app.panels_drag = Some((column, i32::from(border) - i32::from(x)));
}

/// The pointer at screen column `x` with a BORDER held: the border follows
/// it (`panels::drag_border`, which holds the column and the pane to their
/// floors) and the width is remembered. The PANE takes up what the columns
/// leave, and its PTY is resized to that once the frame has drawn it
/// (`sync_pty_size`), as it is when the window itself resizes.
pub(super) fn move_border(app: &mut App, x: u16) {
    let Some((column, grab)) = app.panels_drag else {
        return;
    };
    let to = i32::from(x) + grab;
    let folds = crate::panels::folds(app);
    if let Some(widths) =
        crate::panels::drag_border(app.body_area, app.panels_widths, folds, column, to)
    {
        app.panels_widths = Some(widths);
    }
    app.dirty = true;
}

/// Rows a notch of the wheel scrolls a PANELS column: a third of the
/// GRID's card, as `launcher::GRID_WHEEL_ROWS`.
const WHEEL_ROWS: isize = 3;

/// A notch of the wheel over a PANELS column — a row, a group header or
/// the air under them — scrolls that column a few rows under a cursor
/// that stays put (`ColumnScroll::wheel`): the pane keeps reading the
/// session it was on, so a trackpad never swaps it out from under you,
/// and nothing here moves a cursor, FOCUS or sends a request. The scroll
/// is held at the column's ends, and a column that fits moves nothing.
/// The next key that walks the column brings its cursor back on screen
/// (`handle_action`). True when the pointer was over a column; over the
/// pane it is not, and falls through to the pane's own.
pub(super) fn wheel(app: &mut App, over: Option<&HitTarget>, up: bool) -> bool {
    let focus = match over {
        Some(HitTarget::PanelsRow(row)) => row.focus(),
        Some(HitTarget::PanelBg(focus)) => *focus,
        _ => return false,
    };
    let delta = if up { -WHEEL_ROWS } else { WHEEL_ROWS };
    if app.panels_scroll[crate::panels::scroll_slot(focus)].wheel(delta) {
        app.dirty = true;
    }
    true
}

#[cfg(test)]
mod tests {
    use super::super::tests::{buffer_text, hse, press, seed_tree, with_config_json};
    use super::super::{
        apply_config, handle_mouse, restore_ui_state, sync_pty_size, ui_state_json,
    };
    use crate::app::{App, AttachedTerm, Focus, HitTarget, Overlay, PointerShape, PromptKind};
    use crate::panels::Row;
    use crossterm::event::{KeyCode, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
    use nebula_core::{
        Agent, AgentId, AgentKind, AgentStatus, ClientRequest, Entity, Project, ProjectId,
        ServerEvent, Worktree, WorktreeId,
    };
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    /// `seed_tree`'s `demo` (root `main`, session `agent-1`) with a second
    /// checkout `feat` running `polish-nav`, and a second project `web`
    /// whose root runs `tidy-css` — the PANELS on.
    fn panels_app() -> App {
        let mut app = App::new();
        seed_tree(&mut app);
        let worktree = |id: &str, project: &str, branch: &str, is_main: bool| {
            Entity::Worktree(Worktree {
                id: WorktreeId(id.into()),
                project_id: ProjectId(project.into()),
                path: format!("/tmp/{id}").into(),
                branch: branch.into(),
                is_main,
                sort_order: 1,
            })
        };
        let agent = |id: &str, worktree: &str, name: &str, at: i64| {
            Entity::Agent(Agent {
                id: AgentId(id.into()),
                worktree_id: WorktreeId(worktree.into()),
                name: name.into(),
                status: AgentStatus::Finished,
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
                status_changed_at: at,
                alive: true,
                issue_url: None,
                recent_prompts: Vec::new(),
            })
        };
        for entity in [
            worktree("w2", "p1", "feat", false),
            agent("a2", "w2", "polish-nav", 1),
            Entity::Project(Project {
                id: ProjectId("p2".into()),
                name: "web".into(),
                repo_path: "/tmp/web".into(),
                sort_order: 1,
            }),
            worktree("w3", "p2", "main", true),
            agent("a3", "w3", "tidy-css", 0),
        ] {
            hse(&mut app, ServerEvent::EntityUpserted { entity });
        }
        app.panels = true;
        app
    }

    fn draw(app: &mut App) -> (Terminal<TestBackend>, String) {
        let mut terminal = Terminal::new(TestBackend::new(140, 32)).unwrap();
        terminal.draw(|f| crate::ui::draw(f, app)).unwrap();
        let text = buffer_text(&terminal);
        (terminal, text)
    }

    /// [`draw`] on a window tall enough for the whole `?` overlay.
    fn draw_tall(app: &mut App) -> String {
        let mut terminal = Terminal::new(TestBackend::new(140, 44)).unwrap();
        terminal.draw(|f| crate::ui::draw(f, app)).unwrap();
        buffer_text(&terminal)
    }

    fn key(app: &mut App, c: char, out: &mut Vec<ClientRequest>) {
        press(app, KeyCode::Char(c), KeyModifiers::NONE, out);
    }

    fn selected_project(app: &App) -> String {
        app.selected_project().map(|p| p.name.clone()).unwrap()
    }

    /// The PANELS draw the three columns and the pane in place of the
    /// PROJECT TABS and the GRID, and nothing of the grid's chrome.
    #[test]
    fn the_panels_draw_three_columns_and_the_pane() {
        let mut app = panels_app();
        assert!(app.panels_active() && !app.launcher_grid());
        let (_, text) = draw(&mut app);
        for word in ["PROJECTS", "WORKTREES", "SESSIONS", "TERMINAL"] {
            assert!(text.contains(word), "{word}: {text}");
        }
        for row in ["demo", "web", "main ⌂", "feat", "agent-1"] {
            assert!(text.contains(row), "{row}: {text}");
        }
        assert!(
            !text.contains("RECENT"),
            "the live list has no header: {text}"
        );
        assert!(
            !app.hits
                .iter()
                .any(|(_, h)| matches!(h, HitTarget::LauncherTabAdd | HitTarget::LauncherCard(_))),
            "no grid chrome: {text}"
        );
        for focus in [Focus::Projects, Focus::Worktrees, Focus::Sessions] {
            assert!(
                app.hits
                    .iter()
                    .any(|(_, h)| *h == HitTarget::PanelBg(focus)),
                "{focus:?} has its background"
            );
        }
    }

    /// The setting is live and only a switch: flipped back and forth
    /// mid-session the same project, checkout and session stay selected,
    /// and the pane keeps whatever it had.
    #[test]
    fn switching_the_layout_keeps_the_selection() {
        with_config_json(r#"{"layout": "panels"}"#, || {
            let mut app = panels_app();
            app.panels = false;
            let mut out = Vec::new();
            app.focus = Focus::Projects;
            key(&mut app, 'j', &mut out);
            let before = (app.sel_project, app.sel_worktree, app.sel_session);
            let cfg = crate::config::Config::load();
            apply_config(&mut app, &cfg);
            assert!(app.panels_active());
            draw(&mut app);
            assert_eq!(before, (app.sel_project, app.sel_worktree, app.sel_session));
            apply_config(&mut app, &crate::config::Config::default());
            assert!(app.launcher_grid());
            assert_eq!(app.focus, Focus::Sessions, "no column to hold the keys");
            let (_, text) = draw(&mut app);
            assert_eq!(before, (app.sel_project, app.sel_worktree, app.sel_session));
            assert!(!text.contains("WORKTREES"), "the grid again: {text}");
        });
    }

    /// `h` / `l` walk FOCUS across the columns and stop at the first one;
    /// `j` / `k` in PROJECTS move the project, which scopes WORKTREES.
    #[test]
    fn h_and_l_walk_the_columns_and_j_walks_the_projects() {
        let mut app = panels_app();
        let mut out = Vec::new();
        app.focus = Focus::Sessions;
        key(&mut app, 'h', &mut out);
        assert_eq!(app.focus, Focus::Worktrees);
        key(&mut app, 'h', &mut out);
        assert_eq!(app.focus, Focus::Projects);
        key(&mut app, 'h', &mut out);
        assert_eq!(app.focus, Focus::Projects, "the walk stops at the first");
        let first = selected_project(&app);
        key(&mut app, 'j', &mut out);
        let second = selected_project(&app);
        assert_ne!(first, second);
        let branches: Vec<String> = app
            .visible_worktrees()
            .iter()
            .map(|w| w.branch.clone())
            .collect();
        let want: &[&str] = if second == "web" {
            &["main"]
        } else {
            &["main", "feat"]
        };
        assert_eq!(branches, want, "WORKTREES follows the project");
        key(&mut app, 'l', &mut out);
        assert_eq!(app.focus, Focus::Worktrees);
    }

    /// Tab walks forward and lands in the pane; Enter on a session
    /// attaches it and takes the keys.
    #[test]
    fn tab_and_enter_go_into_the_pane() {
        let mut app = panels_app();
        let mut out = Vec::new();
        app.focus = Focus::Projects;
        press(&mut app, KeyCode::Tab, KeyModifiers::NONE, &mut out);
        assert_eq!(app.focus, Focus::Worktrees);
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE, &mut out);
        assert_eq!(
            app.focus,
            Focus::Sessions,
            "Enter on a checkout: its sessions"
        );
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE, &mut out);
        assert_eq!(app.focus, Focus::Terminal);
        assert!(app.term_locked, "Enter on a session takes the keys");
        assert!(
            out.iter()
                .any(|r| matches!(r, ClientRequest::Attach { .. })),
            "{out:?}"
        );
    }

    /// The grid's own keys never strand the panels: Space expands the
    /// session's pill into its FOLLOW-UP COMPOSER — no modal over the
    /// screen — and the PROJECT TAB keys, the pane fold and its strip only
    /// say they have nothing to act on here.
    #[test]
    fn the_grids_own_keys_are_harmless_beside_the_columns() {
        let mut app = panels_app();
        let mut out = Vec::new();
        app.focus = Focus::Sessions;
        let projects = app.tree.projects.len();
        for c in ['x', '2', '`'] {
            key(&mut app, c, &mut out);
            assert!(app.overlay.is_none(), "{c}: {:?}", app.overlay);
            assert!(app.flash.take().is_some(), "{c} says why not");
        }
        assert_eq!(app.tree.projects.len(), projects);
        assert!(app.panels_active(), "no tab was closed");
        key(&mut app, ' ', &mut out);
        assert!(app.overlay.is_none(), "no modal: {:?}", app.overlay);
        assert!(app.follow_up_live(), "the pill expanded");
    }

    /// `^F` on a session row full-screens it with the keys in it; `^F`
    /// again comes back down to the panels' pane, keys still there.
    #[test]
    fn ctrl_f_full_screens_the_session_and_back() {
        let mut app = panels_app();
        let mut out = Vec::new();
        app.focus = Focus::Sessions;
        press(
            &mut app,
            KeyCode::Char('f'),
            KeyModifiers::CONTROL,
            &mut out,
        );
        assert!(app.collapsed && app.term_locked, "full screen, typing");
        draw(&mut app);
        press(
            &mut app,
            KeyCode::Char('f'),
            KeyModifiers::CONTROL,
            &mut out,
        );
        assert!(!app.collapsed, "back down");
        assert_eq!(app.focus, Focus::Terminal, "into the panels' pane");
        let (_, text) = draw(&mut app);
        assert!(text.contains("WORKTREES"), "{text}");
    }

    fn click_at(app: &mut App, target: HitTarget, out: &mut Vec<ClientRequest>) {
        let rect = app
            .hits
            .iter()
            .find(|(_, h)| *h == target)
            .map(|(r, _)| *r)
            .unwrap_or_else(|| panic!("{target:?} is not on screen"));
        handle_mouse(
            app,
            MouseEvent {
                kind: MouseEventKind::Down(MouseButton::Left),
                column: rect.x + 2,
                row: rect.y,
                modifiers: KeyModifiers::NONE,
            },
            out,
        );
    }

    /// A click on a row puts the cursor there and its column takes FOCUS;
    /// a click on the pane steps into it.
    #[test]
    fn a_click_selects_a_row_and_the_pane_takes_the_keys() {
        let mut app = panels_app();
        let mut out = Vec::new();
        app.focus = Focus::Sessions;
        draw(&mut app);
        let other = if app.sel_project == 0 { 1 } else { 0 };
        click_at(
            &mut app,
            HitTarget::PanelsRow(Row::Project(other)),
            &mut out,
        );
        assert_eq!(app.focus, Focus::Projects);
        assert_eq!(app.sel_project, other);
        draw(&mut app);
        click_at(&mut app, HitTarget::PanelsRow(Row::Session(0)), &mut out);
        assert_eq!(app.focus, Focus::Sessions);
        draw(&mut app);
        click_at(&mut app, HitTarget::TerminalPane, &mut out);
        assert_eq!(app.focus, Focus::Terminal);
    }

    /// `⇧A` opens the ARCHIVED group under the live rows and folds it
    /// again, in place, to the one line that counts it: the cursor stays
    /// in the checkout it was in.
    #[test]
    fn shift_a_folds_the_archived_group_in_place() {
        let mut app = panels_app();
        let mut out = Vec::new();
        app.focus = Focus::Sessions;
        let worktree = app.selected_worktree().map(|w| w.id.clone());
        let old = Agent {
            archived: true,
            id: AgentId("old".into()),
            name: "old-run".into(),
            worktree_id: worktree.clone().unwrap(),
            ..app.tree.agents[0].clone()
        };
        hse(
            &mut app,
            ServerEvent::EntityUpserted {
                entity: Entity::Agent(old),
            },
        );
        let (_, text) = draw(&mut app);
        assert!(
            text.contains("… 1 archived") && !text.contains("old-run"),
            "{text}"
        );
        press(&mut app, KeyCode::Char('A'), KeyModifiers::SHIFT, &mut out);
        let (_, text) = draw(&mut app);
        assert!(
            text.contains("ARCHIVED · 1") && text.contains("old-run"),
            "{text}"
        );
        assert_eq!(app.selected_worktree().map(|w| w.id.clone()), worktree);
    }

    /// One wheel notch with the pointer on the middle of `target`'s rect.
    fn wheel_at(
        app: &mut App,
        target: HitTarget,
        kind: MouseEventKind,
        out: &mut Vec<ClientRequest>,
    ) {
        let rect = app
            .hits
            .iter()
            .find(|(_, h)| *h == target)
            .map(|(r, _)| *r)
            .unwrap_or_else(|| panic!("{target:?} is not on screen"));
        handle_mouse(
            app,
            MouseEvent {
                kind,
                column: rect.x + 2,
                row: rect.y,
                modifiers: KeyModifiers::NONE,
            },
            out,
        );
    }

    /// `demo`'s root with `n` more sessions, so SESSIONS outgrows its
    /// column; the cursor on the first row.
    fn long_sessions(app: &mut App, n: usize) {
        let root = app.selected_worktree().map(|w| w.id.clone()).unwrap();
        for i in 0..n {
            let more = Agent {
                id: AgentId(format!("long{i}")),
                name: format!("long-{i}"),
                worktree_id: root.clone(),
                ..app.tree.agents[0].clone()
            };
            hse(
                app,
                ServerEvent::EntityUpserted {
                    entity: Entity::Agent(more),
                },
            );
        }
        app.sel_project = 0;
        app.sel_worktree = 0;
        app.sel_session = 0;
    }

    fn on_screen(app: &App, row: Row) -> bool {
        app.hits
            .iter()
            .any(|(_, h)| *h == HitTarget::PanelsRow(row))
    }

    /// A notch over a column taller than its height scrolls it under the
    /// cursor: no cursor, FOCUS or request moves.
    #[test]
    fn the_wheel_scrolls_a_long_column_under_its_cursor() {
        let mut app = panels_app();
        let mut out = Vec::new();
        long_sessions(&mut app, 40);
        app.focus = Focus::Terminal;
        draw(&mut app);
        assert!(on_screen(&app, Row::Session(0)));
        let cursors = (app.sel_project, app.sel_worktree, app.sel_session);
        wheel_at(
            &mut app,
            HitTarget::PanelsRow(Row::Session(0)),
            MouseEventKind::ScrollDown,
            &mut out,
        );
        assert_eq!(app.panels_scroll[2].top, 3, "three rows a notch");
        assert_eq!(
            cursors,
            (app.sel_project, app.sel_worktree, app.sel_session)
        );
        assert_eq!(app.focus, Focus::Terminal, "FOCUS stays");
        assert!(out.is_empty(), "no attach: {out:?}");
        draw(&mut app);
        assert!(!on_screen(&app, Row::Session(0)), "the window moved");
        assert!(on_screen(&app, Row::Session(5)));
        wheel_at(
            &mut app,
            HitTarget::PanelBg(Focus::Sessions),
            MouseEventKind::ScrollUp,
            &mut out,
        );
        assert_eq!(app.panels_scroll[2].top, 0, "a notch up scrolls back");
        assert_eq!(app.panels_scroll[0].top + app.panels_scroll[1].top, 0);
    }

    /// A column that fits its height moves nothing.
    #[test]
    fn the_wheel_moves_nothing_in_a_column_that_fits() {
        let mut app = panels_app();
        let mut out = Vec::new();
        draw(&mut app);
        let before = app.panels_scroll;
        for focus in [Focus::Projects, Focus::Worktrees, Focus::Sessions] {
            wheel_at(
                &mut app,
                HitTarget::PanelBg(focus),
                MouseEventKind::ScrollDown,
                &mut out,
            );
        }
        assert_eq!(before, app.panels_scroll);
        assert!(out.is_empty());
    }

    /// The scroll holds at the column's first row and at the one that
    /// puts the last row on the bottom one.
    #[test]
    fn the_wheel_holds_at_both_ends() {
        let mut app = panels_app();
        let mut out = Vec::new();
        long_sessions(&mut app, 40);
        draw(&mut app);
        let at = HitTarget::PanelBg(Focus::Sessions);
        wheel_at(&mut app, at.clone(), MouseEventKind::ScrollUp, &mut out);
        assert_eq!(app.panels_scroll[2].top, 0, "held at the top");
        for _ in 0..100 {
            wheel_at(&mut app, at.clone(), MouseEventKind::ScrollDown, &mut out);
        }
        let max = app.panels_scroll[2].max;
        assert!(max > 0, "the column is longer than it is tall");
        assert_eq!(app.panels_scroll[2].top, max, "held at the bottom");
        draw(&mut app);
        assert!(on_screen(&app, Row::Session(40)), "the last row shows");
    }

    /// With the cursor's row wheeled off screen, the next `j` or `k` in the
    /// column brings it back — even a `k` on the first row, which has
    /// nowhere to go.
    #[test]
    fn a_walk_after_the_wheel_brings_the_cursor_back() {
        let mut app = panels_app();
        let mut out = Vec::new();
        long_sessions(&mut app, 40);
        app.focus = Focus::Sessions;
        draw(&mut app);
        let at = HitTarget::PanelBg(Focus::Sessions);
        for _ in 0..4 {
            wheel_at(&mut app, at.clone(), MouseEventKind::ScrollDown, &mut out);
        }
        draw(&mut app);
        assert!(!on_screen(&app, Row::Session(0)), "wheeled away");
        key(&mut app, 'k', &mut out);
        assert_eq!(app.sel_session, 0);
        draw(&mut app);
        assert!(on_screen(&app, Row::Session(0)), "k at the top: back");
        for _ in 0..100 {
            wheel_at(&mut app, at.clone(), MouseEventKind::ScrollDown, &mut out);
        }
        draw(&mut app);
        assert!(!on_screen(&app, Row::Session(0)));
        key(&mut app, 'j', &mut out);
        assert_eq!(app.sel_session, 1);
        draw(&mut app);
        assert!(on_screen(&app, Row::Session(1)), "j: back on the cursor");
    }

    /// Over the pane the wheel is the pane's: no column's cursor moves and
    /// FOCUS stays where it was.
    #[test]
    fn the_wheel_over_the_pane_leaves_the_columns_alone() {
        let mut app = panels_app();
        let mut out = Vec::new();
        app.focus = Focus::Terminal;
        let before = (app.sel_project, app.sel_worktree, app.sel_session);
        draw(&mut app);
        wheel_at(
            &mut app,
            HitTarget::TerminalPane,
            MouseEventKind::ScrollDown,
            &mut out,
        );
        assert_eq!(before, (app.sel_project, app.sel_worktree, app.sel_session));
        assert_eq!(app.focus, Focus::Terminal);
    }

    /// One mouse event at a cell, unmodified.
    fn mouse_at(
        app: &mut App,
        kind: MouseEventKind,
        (column, row): (u16, u16),
        out: &mut Vec<ClientRequest>,
    ) {
        let modifiers = KeyModifiers::NONE;
        handle_mouse(
            app,
            MouseEvent {
                kind,
                column,
                row,
                modifiers,
            },
            out,
        );
    }

    /// Column `i`'s rule, a few rows down: the left of its BORDER's two
    /// grab cells.
    fn rule_cell(app: &App, i: usize) -> (u16, u16) {
        let zone = app
            .hits
            .iter()
            .find(|(_, h)| *h == HitTarget::PanelsBorder(i))
            .map(|(r, _)| *r)
            .unwrap_or_else(|| panic!("border {i} is not on screen"));
        (zone.x, zone.y + 5)
    }

    /// A press on a BORDER arms a drag that keeps its grab offset, moves no
    /// cursor or FOCUS and selects nothing; the motion resizes the one
    /// column, the release ends it, and the pane — and the PTY in it —
    /// take up what the column gave or took.
    #[test]
    fn dragging_a_border_resizes_its_column_and_the_pane_follows() {
        let mut app = panels_app();
        let mut out = Vec::new();
        app.focus = Focus::Projects;
        draw(&mut app);
        let sref = app.selected_session_row().and_then(|r| r.sref()).unwrap();
        app.term = Some(AttachedTerm::new(sref, 40, 10));
        draw(&mut app);
        sync_pty_size(&mut app, &mut out);
        out.clear();
        let pane = app.term_area;
        let cursors = (app.sel_project, app.sel_worktree, app.sel_session);
        let (x, y) = rule_cell(&app, 1);
        let down = MouseEventKind::Down(MouseButton::Left);
        mouse_at(&mut app, down, (x, y), &mut out);
        assert_eq!(app.panels_drag, Some((1, 1)), "one short of the border");
        assert_eq!(app.pointer_shape, PointerShape::ColResize);
        assert_eq!(app.hover_panels_border, Some(1));
        let drag = MouseEventKind::Drag(MouseButton::Left);
        mouse_at(&mut app, drag, (x + 8, y), &mut out);
        assert_eq!(app.panels_widths, Some([20, 30, 32]));
        mouse_at(&mut app, drag, (x + 3, y), &mut out);
        assert_eq!(app.panels_widths, Some([20, 25, 32]), "and back");
        assert_eq!(
            cursors,
            (app.sel_project, app.sel_worktree, app.sel_session)
        );
        assert_eq!(app.focus, Focus::Projects, "FOCUS stays");
        assert!(app.term_selection.is_none(), "nothing selected");
        assert!(out.is_empty(), "{out:?}");
        let up = MouseEventKind::Up(MouseButton::Left);
        mouse_at(&mut app, up, (x + 3, y), &mut out);
        assert!(app.panels_drag.is_none(), "mouse-up ends the drag");
        draw(&mut app);
        assert_eq!(
            (app.term_area.x, app.term_area.width),
            (pane.x + 3, pane.width - 3)
        );
        sync_pty_size(&mut app, &mut out);
        assert!(
            out.iter().any(
                |r| matches!(r, ClientRequest::Resize { cols, .. } if *cols == pane.width - 3)
            ),
            "{out:?}"
        );
    }

    /// Dragged off either end, the column rests at its floor and the pane
    /// at its own; the drag holds the resize arrows past the grab zone.
    #[test]
    fn a_border_drag_stops_at_both_floors() {
        use crate::panels::{MIN_PANE_W, MIN_W};
        let mut app = panels_app();
        let mut out = Vec::new();
        draw(&mut app);
        let body = app.body_area;
        let (x, y) = rule_cell(&app, 0);
        let down = MouseEventKind::Down(MouseButton::Left);
        mouse_at(&mut app, down, (x, y), &mut out);
        let drag = MouseEventKind::Drag(MouseButton::Left);
        mouse_at(&mut app, drag, (0, y), &mut out);
        assert_eq!(app.panels_widths, Some([MIN_W, 22, 32]));
        mouse_at(&mut app, drag, (body.right() - 1, y), &mut out);
        let [p, w, s] = app.panels_widths.unwrap();
        assert_eq!((w, s), (22, 32), "only the dragged column moved");
        assert_eq!(body.width - (p + w + s), MIN_PANE_W);
        assert_eq!(app.pointer_shape, PointerShape::ColResize);
        assert_eq!(app.hover_panels_border, Some(0), "the grip stays lit");
    }

    /// The pointer resting on a BORDER asks for the resize arrows and
    /// lights that border's grip; moved off, both rest again.
    #[test]
    fn hovering_a_border_lights_its_grip() {
        let mut app = panels_app();
        let mut out = Vec::new();
        draw(&mut app);
        let (x, y) = rule_cell(&app, 2);
        mouse_at(&mut app, MouseEventKind::Moved, (x + 1, y), &mut out);
        assert_eq!(app.pointer_shape, PointerShape::ColResize);
        assert_eq!(app.hover_panels_border, Some(2));
        let (terminal, _) = draw(&mut app);
        let lit = |i: usize| {
            let x = rule_cell(&app, i).0;
            let buf = terminal.backend().buffer();
            (0..buf.area.height).any(|y| {
                let cell = &buf[(x, y)];
                cell.symbol() == "┃" && cell.fg == app.theme.accent
            })
        };
        assert!(lit(2), "the hovered grip is lit");
        assert!(!lit(0) && !lit(1), "the others rest");
        mouse_at(&mut app, MouseEventKind::Moved, (x - 5, y), &mut out);
        assert_eq!(app.pointer_shape, PointerShape::Default);
        assert_eq!(app.hover_panels_border, None);
    }

    /// The widths a drag left outlive a restart; a blob from before there
    /// were any opens the columns at their defaults, and a nonsense one is
    /// held to sane widths.
    #[test]
    fn the_dragged_widths_outlive_a_restart() {
        let mut app = panels_app();
        app.panels_widths = Some([30, 15, 40]);
        let json = ui_state_json(&app);
        let mut next = panels_app();
        restore_ui_state(&mut next, &json);
        assert_eq!(next.panels_widths, Some([30, 15, 40]));
        restore_ui_state(&mut next, r#"{"show_archived":false,"collapsed":false}"#);
        assert_eq!(next.panels_widths, None, "an older blob: the defaults");
        restore_ui_state(
            &mut next,
            r#"{"show_archived":false,"collapsed":false,"panels_widths":[0,9999,40]}"#,
        );
        assert_eq!(
            next.panels_widths,
            Some([crate::panels::MIN_W, super::super::MAX_RESTORED_WIDTH, 40])
        );
    }

    /// The rect `target` registered, panicking when it is not on screen.
    fn hit_rect(app: &App, target: HitTarget) -> ratatui::layout::Rect {
        app.hits
            .iter()
            .find(|(_, h)| *h == target)
            .map(|(r, _)| *r)
            .unwrap_or_else(|| panic!("{target:?} is not on screen"))
    }

    /// PROJECTS are 3-row buttons and the checkouts 2-row PILLS on a
    /// 2-row stride: a click anywhere on a button lands on its project,
    /// and the cursor's pill wears its half-block pads.
    #[test]
    fn projects_are_buttons_and_checkouts_are_pills() {
        let mut app = panels_app();
        let mut out = Vec::new();
        app.focus = Focus::Worktrees;
        let (terminal, _) = draw(&mut app);
        let first = hit_rect(&app, HitTarget::PanelsRow(Row::Project(0)));
        let second = hit_rect(&app, HitTarget::PanelsRow(Row::Project(1)));
        assert_eq!((first.height, second.y), (3, first.y + 3));
        let root = hit_rect(&app, HitTarget::PanelsRow(Row::Worktree(0)));
        let feat = hit_rect(&app, HitTarget::PanelsRow(Row::Worktree(1)));
        assert_eq!(feat.y, root.y + 3, "a quiet row under the root");
        let buf = terminal.backend().buffer();
        assert_eq!(buf[(root.x + 3, root.y)].symbol(), "▄", "top pad");
        assert_eq!(buf[(root.x + 3, root.y + 2)].symbol(), "▀", "bottom pad");
        assert_eq!(buf[(root.x, root.y + 1)].symbol(), "█", "the rail");
        mouse_at(
            &mut app,
            MouseEventKind::Down(MouseButton::Left),
            (second.x + 2, second.y + 2),
            &mut out,
        );
        assert_eq!((app.focus, app.sel_project), (Focus::Projects, 1));
    }

    /// A project button counts its open pull requests and issues after
    /// the ago label, and a renamed one names the folder it lives in on
    /// the row under it.
    #[test]
    fn a_project_button_counts_prs_and_issues_and_names_its_folder() {
        use super::super::tests::{seed_issues, seed_open_prs};
        let mut app = panels_app();
        app.panels_widths = Some([44, 22, 32]);
        seed_open_prs(&mut app, &[(1, "a"), (2, "b"), (3, "c")]);
        seed_issues(&mut app, &[(7, "x"), (8, "y")]);
        let at = app.project_rows()[app.sel_project];
        app.tree.projects[at].name = "api".into();
        let (_, text) = draw(&mut app);
        assert!(text.contains("3 prs · 2 issues"), "{text}");
        assert!(text.contains("└ demo"), "{text}");
    }

    /// The OPEN PRS header owns up to drafts kept out — `0/3`, kept on
    /// screen when every pull request is one — and a list at the fetch cap
    /// says `100+`, as the ISSUES header says `+`.
    #[test]
    fn the_group_headers_own_up_to_hidden_drafts_and_the_cap() {
        use super::super::tests::{seed_issues, seed_open_prs};
        let mut app = panels_app();
        app.sel_project = app
            .project_rows()
            .iter()
            .position(|i| app.tree.projects[*i].name == "demo")
            .unwrap();
        seed_open_prs(&mut app, &[(1, "a"), (2, "b"), (3, "c")]);
        let id = app.selected_project().unwrap().id.clone();
        for pr in &mut app.open_prs.get_mut(&id).unwrap().list {
            pr.is_draft = true;
        }
        app.hide_draft_prs = true;
        let (_, text) = draw(&mut app);
        assert!(text.contains("▾ OPEN PRS · 0/3"), "{text}");
        let many: Vec<(u64, String)> = (0..100).map(|n| (n, format!("t{n}"))).collect();
        let many: Vec<(u64, &str)> = many.iter().map(|(n, t)| (*n, t.as_str())).collect();
        seed_open_prs(&mut app, &many);
        seed_issues(&mut app, &many);
        app.open_prs_collapsed = true;
        let (_, text) = draw(&mut app);
        assert!(text.contains("OPEN PRS · 100+"), "{text}");
        assert!(text.contains("ISSUES · 100+"), "{text}");
    }

    /// A checkout whose pull request merged wears the purple — dot and
    /// branch — and one with its RUN COMMAND up says `▶ running`, or the
    /// bare `▶` where the word would cut the branch.
    #[test]
    fn a_merged_checkout_is_purple_and_a_running_one_says_so() {
        let mut app = panels_app();
        app.panels_widths = Some([20, 40, 32]);
        let feat = WorktreeId("w2".into());
        app.pull_requests.insert(
            feat.clone(),
            Some(crate::pull_request::PullRequest {
                number: 5,
                url: "https://github.com/o/r/pull/5".into(),
                title: "feat".into(),
                state: crate::pull_request::STATE_MERGED.into(),
                is_draft: false,
                health: Default::default(),
                activity: Vec::new(),
            }),
        );
        app.tree.terminals.push(nebula_core::TerminalTab {
            id: nebula_core::TerminalId("t1".into()),
            worktree_id: WorktreeId("w1".into()),
            name: "run".into(),
            sort_order: 0,
            alive: true,
            run_command: Some("make dev".into()),
        });
        app.sel_project = app
            .project_rows()
            .iter()
            .position(|i| app.tree.projects[*i].name == "demo")
            .unwrap();
        let (terminal, text) = draw(&mut app);
        assert!(text.contains("main ▶ running"), "{text}");
        let rect = hit_rect(&app, HitTarget::PanelsRow(Row::Worktree(1)));
        let buf = terminal.backend().buffer();
        let row = rect.y + 1;
        let x = (rect.x..rect.right())
            .find(|x| buf[(*x, row)].symbol() == "f")
            .expect("the branch");
        assert_eq!(buf[(x, row)].fg, app.theme.merged, "purple branch");
        assert_eq!(buf[(x - 2, row)].fg, app.theme.merged, "purple dot");
        app.panels_widths = Some([20, 14, 32]);
        let (_, text) = draw(&mut app);
        assert!(
            text.contains("main ▶") && !text.contains("▶ running"),
            "{text}"
        );
    }

    /// The checkout's pull request counts the comments since it was last
    /// opened from nebula, in place of its state word.
    #[test]
    fn the_pull_request_row_counts_unread_comments() {
        let mut app = panels_app();
        app.sel_project = app
            .project_rows()
            .iter()
            .position(|i| app.tree.projects[*i].name == "demo")
            .unwrap();
        app.sel_worktree = 0;
        let url = "https://github.com/o/r/pull/9".to_string();
        app.pull_requests.insert(
            WorktreeId("w1".into()),
            Some(crate::pull_request::PullRequest {
                number: 9,
                url: url.clone(),
                title: "fix".into(),
                state: crate::pull_request::STATE_OPEN.into(),
                is_draft: false,
                health: Default::default(),
                activity: vec!["2026-09-01T00:00:00Z".into(), "2026-09-02T00:00:00Z".into()],
            }),
        );
        let (_, text) = draw(&mut app);
        assert!(text.contains("#9 fix 2 new"), "{text}");
        app.pr_seen.insert(url, "2026-09-01T00:00:00Z".into());
        let (_, text) = draw(&mut app);
        assert!(text.contains("#9 fix 1 new"), "{text}");
    }

    /// A project with no checkout, pull request or issue says which key
    /// starts a worktree.
    #[test]
    fn an_empty_project_says_how_to_start_a_worktree() {
        let mut app = panels_app();
        app.panels_widths = Some([20, 26, 32]);
        hse(
            &mut app,
            ServerEvent::EntityUpserted {
                entity: Entity::Project(Project {
                    id: ProjectId("p3".into()),
                    name: "bare".into(),
                    repo_path: "/tmp/bare".into(),
                    sort_order: 2,
                }),
            },
        );
        app.sel_project = app
            .project_rows()
            .iter()
            .position(|i| app.tree.projects[*i].name == "bare")
            .unwrap();
        let (_, text) = draw(&mut app);
        assert!(text.contains("n starts a worktree"), "{text}");
    }

    /// The footer carries the checkout's changed files after the
    /// breadcrumb beside the PANELS, and no KEY COMBO DISPLAY over it; the
    /// GRID's footer is what it was.
    #[test]
    fn the_footer_counts_changed_files_and_echoes_no_keys() {
        let mut app = panels_app();
        let w = app.selected_worktree().unwrap().id.clone();
        app.git_changes = Some((w, Some(3)));
        let j = crate::keymap::KeyChord {
            code: KeyCode::Char('j'),
            mods: KeyModifiers::NONE,
        };
        crate::key_combo::note(&mut app, &[j], Some("Move down"));
        let (_, text) = draw(&mut app);
        assert!(text.contains("+3 files"), "{text}");
        assert!(!text.contains("Move down"), "{text}");
        app.panels = false;
        let (_, text) = draw(&mut app);
        assert!(!text.contains("+3 files"), "{text}");
        assert!(text.contains("Move down"), "{text}");
    }

    /// `?` beside the columns teaches the columns' keys, not the grid's
    /// cards and project tabs.
    #[test]
    fn question_mark_in_the_panels_describes_the_panels() {
        let mut app = panels_app();
        let mut out = Vec::new();
        app.focus = Focus::Sessions;
        draw(&mut app);
        key(&mut app, '?', &mut out);
        assert!(
            matches!(app.overlay, Some(Overlay::Help(_))),
            "{:?}",
            app.overlay
        );
        let text = draw_tall(&mut app);
        for want in [
            "NAVIGATE & SEARCH",
            "walk panels (fwd locks input)",
            "PROJECTS",
            "WORKTREES",
            "SESSIONS",
            "TERMINAL & MOUSE",
            "GENERAL",
            "TYPING IN A FIELD",
            "context menu (right-click)",
            "lock input (2nd: full-screen)",
            "Enter / z",
            "resize panels",
        ] {
            assert!(text.contains(want), "{want}: {text}");
        }
        for gone in ["walk the cards", "fold / unfold the pane", "workspace"] {
            assert!(!text.contains(gone), "{gone}: {text}");
        }
    }

    /// With the grid up `?` is what it always was.
    #[test]
    fn question_mark_in_the_grid_is_unchanged() {
        let mut app = panels_app();
        app.panels = false;
        let mut out = Vec::new();
        draw(&mut app);
        key(&mut app, '?', &mut out);
        assert!(
            matches!(app.overlay, Some(Overlay::Help(_))),
            "{:?}",
            app.overlay
        );
        let (_, text) = draw(&mut app);
        for want in [
            "NAVIGATE & SEARCH",
            "walk the cards",
            "fold / unfold the pane",
        ] {
            assert!(text.contains(want), "{want}: {text}");
        }
        assert!(!text.contains("walk panels"), "{text}");
    }

    /// A chord as the terminal sends it with modifiers held.
    fn chord(app: &mut App, code: KeyCode, mods: KeyModifiers, out: &mut Vec<ClientRequest>) {
        press(app, code, mods, out);
    }

    /// `⇧Tab` and `^⇧H` walk FOCUS back a column, `^⇧L` forward as Tab
    /// does, and `^→` onto the next column and on into the pane without
    /// taking the input lock — the columns' walk as it always was.
    #[test]
    fn shift_tab_and_the_ctrl_chords_walk_the_columns() {
        let mut app = panels_app();
        let mut out = Vec::new();
        app.focus = Focus::Sessions;
        draw(&mut app);
        chord(&mut app, KeyCode::BackTab, KeyModifiers::SHIFT, &mut out);
        assert_eq!(app.focus, Focus::Worktrees);
        let ctrl_shift = KeyModifiers::CONTROL | KeyModifiers::SHIFT;
        chord(&mut app, KeyCode::Char('H'), ctrl_shift, &mut out);
        assert_eq!(app.focus, Focus::Projects);
        chord(&mut app, KeyCode::BackTab, KeyModifiers::SHIFT, &mut out);
        assert_eq!(app.focus, Focus::Projects, "the walk stops at the first");
        chord(&mut app, KeyCode::Char('L'), ctrl_shift, &mut out);
        assert_eq!(app.focus, Focus::Worktrees);
        for want in [Focus::Sessions, Focus::Terminal, Focus::Terminal] {
            chord(&mut app, KeyCode::Right, KeyModifiers::CONTROL, &mut out);
            assert_eq!(app.focus, want);
        }
        assert!(!app.term_locked, "^→ reads the pane without typing");
    }

    /// `z` full-screens the pane with the keys in it, and the hatch out
    /// of it (`^q`) goes straight back to the columns, as it always did;
    /// with nothing in the pane `z` says so.
    #[test]
    fn z_full_screens_the_pane_and_the_hatch_leaves_for_the_columns() {
        let mut app = panels_app();
        let mut out = Vec::new();
        app.focus = Focus::Sessions;
        draw(&mut app);
        key(&mut app, 'z', &mut out);
        assert_eq!(app.flash.as_deref(), Some(super::ATTACH_FIRST));
        assert!(!app.collapsed);
        let sref = app.selected_session_row().and_then(|r| r.sref()).unwrap();
        app.term = Some(AttachedTerm::new(sref, 40, 10));
        key(&mut app, 'z', &mut out);
        assert!(app.collapsed && app.term_locked, "full screen, typing");
        assert_eq!(app.focus, Focus::Terminal);
        draw(&mut app);
        chord(
            &mut app,
            KeyCode::Char('q'),
            KeyModifiers::CONTROL,
            &mut out,
        );
        assert!(!app.collapsed && !app.term_locked);
        assert_eq!(app.focus, Focus::Sessions, "back on the columns");
        let (_, text) = draw(&mut app);
        assert!(text.contains("WORKTREES"), "{text}");
    }

    /// The labels of the menu `m` opened, and where it hangs.
    fn menu_of(app: &App) -> (Vec<String>, Option<(u16, u16)>) {
        match &app.overlay {
            Some(Overlay::Menu(m)) => (m.items.iter().map(|i| i.label.clone()).collect(), m.at),
            other => panic!("no menu: {other:?}"),
        }
    }

    /// `m` opens the cursor row's menu — a right-click's, and only that —
    /// at the fixed spot a keyboard menu always hung at; a session row's
    /// menu is the session's alone, the checkout's verbs being its
    /// WORKTREES row's. A column with no row under its cursor has none,
    /// and neither has the pane.
    #[test]
    fn m_opens_the_cursor_rows_menu_alone() {
        let mut app = panels_app();
        let mut out = Vec::new();
        app.focus = Focus::Worktrees;
        draw(&mut app);
        key(&mut app, 'm', &mut out);
        let (labels, at) = menu_of(&app);
        for want in ["New agent", "New terminal", "Open"] {
            assert!(labels.iter().any(|l| l == want), "{want}: {labels:?}");
        }
        for gone in ["New worktree", "Show/hide open PRs", "Show/hide issues"] {
            assert!(!labels.iter().any(|l| l == gone), "{gone}: {labels:?}");
        }
        assert_eq!(at, Some(super::KEYBOARD_MENU_ANCHOR));
        app.overlay = None;
        app.focus = Focus::Sessions;
        draw(&mut app);
        key(&mut app, 'm', &mut out);
        let (labels, at) = menu_of(&app);
        for want in ["Attach", "Follow-up prompt", "Duplicate", "Archive"] {
            assert!(labels.iter().any(|l| l == want), "{want}: {labels:?}");
        }
        for gone in ["Run", "Delete worktree", "Show/hide archived"] {
            assert!(!labels.iter().any(|l| l == gone), "{gone}: {labels:?}");
        }
        assert_eq!(at, Some(super::KEYBOARD_MENU_ANCHOR));
        app.overlay = None;
        app.focus = Focus::Terminal;
        key(&mut app, 'm', &mut out);
        assert!(app.overlay.is_none(), "{:?}", app.overlay);
        // A checkout with no session: SESSIONS has no row, and no menu.
        app.tree.agents.clear();
        app.focus = Focus::Sessions;
        draw(&mut app);
        key(&mut app, 'm', &mut out);
        assert!(app.overlay.is_none(), "{:?}", app.overlay);
    }

    /// The PANELS' own keys are no default binding of the GRID's, so
    /// taking them beside the columns takes nothing from it — and on the
    /// grid they still do nothing at all.
    #[test]
    fn the_panels_own_keys_leave_the_grid_alone() {
        let keymap = crate::keymap::Keymap::default();
        for (k, _) in crate::panels::PANEL_KEYS {
            let bound = keymap.lookup(crate::keymap::Scope::Global, &k);
            assert_eq!(bound, None, "{} is the grid's", k.display());
        }
        let mut app = panels_app();
        app.panels = false;
        let mut out = Vec::new();
        draw(&mut app);
        let focus = app.focus;
        key(&mut app, 'z', &mut out);
        key(&mut app, 'm', &mut out);
        chord(&mut app, KeyCode::BackTab, KeyModifiers::SHIFT, &mut out);
        chord(&mut app, KeyCode::Right, KeyModifiers::CONTROL, &mut out);
        assert!(app.overlay.is_none() && !app.collapsed, "{:?}", app.overlay);
        assert_eq!(app.focus, focus);
    }

    /// The footer's hints on a column end `m: menu  ?: help` beside the
    /// columns, as they always did, and the grid's never name `m`.
    #[test]
    fn the_footer_names_m_beside_the_columns_only() {
        let mut app = panels_app();
        app.focus = Focus::Projects;
        let (_, text) = draw(&mut app);
        assert!(text.contains("m: menu  ?: help"), "{text}");
        app.panels = false;
        let (_, text) = draw(&mut app);
        assert!(!text.contains("m: menu"), "{text}");
    }

    /// The PANELS paint the FOCUS TINT each preset had while they were
    /// the only layout; the GRID keeps the preset's own.
    #[test]
    fn the_panels_keep_their_old_focus_tint() {
        use ratatui::style::Color;
        let mut app = App::new();
        let mut cfg = crate::config::Config {
            layout: "panels".into(),
            ..Default::default()
        };
        apply_config(&mut app, &cfg);
        assert_eq!(app.theme.focus_tint, Color::Rgb(22, 33, 34));
        cfg.theme = "Ocean".into();
        apply_config(&mut app, &cfg);
        assert_eq!(app.theme.focus_tint, Color::Rgb(21, 31, 38));
        cfg.layout = "grid".into();
        apply_config(&mut app, &cfg);
        assert_eq!(app.theme, crate::theme::Theme::by_name("ocean"));
    }

    /// The Appearance tab's column rows and RECENT PROMPTS sit under their
    /// own header, never marked `(new)`: they came back with the layout.
    #[test]
    fn the_column_rows_sit_under_a_panels_header() {
        use crate::config::{settings_rows, today_days, SettingKind, SettingsRow};
        let (tab, _) = crate::config::locate(SettingKind::HideProjects).unwrap();
        let rows = settings_rows(tab);
        let header = rows
            .iter()
            .position(|r| *r == SettingsRow::Header("PANELS LAYOUT".into()))
            .expect("the header");
        assert_eq!(rows.len() - header, 6, "the five rows close the tab");
        for kind in [
            SettingKind::HideProjects,
            SettingKind::HideWorktrees,
            SettingKind::HideSessions,
            SettingKind::RecentPrompts,
            SettingKind::RecentPromptsCount,
        ] {
            assert!(!kind.is_new(today_days()), "{kind:?}");
        }
    }

    /// `⇧P` / `⇧B` / `⇧S` as the keys arrive from a terminal.
    fn shift(app: &mut App, c: char, out: &mut Vec<ClientRequest>) {
        press(app, KeyCode::Char(c), KeyModifiers::SHIFT, out);
    }

    /// `⇧P` folds PROJECTS to its RAIL — the pane takes the width, a FOCUS
    /// there steps on to WORKTREES, the footer names the key that opens
    /// it — and `⇧P` again opens it at the width it was dragged to. The
    /// fold is written to CONFIG.JSON as it happens.
    #[test]
    fn shift_p_folds_projects_to_a_rail_and_back() {
        with_config_json("{}", || {
            let mut app = panels_app();
            let mut out = Vec::new();
            app.panels_widths = Some([30, 22, 32]);
            app.focus = Focus::Projects;
            draw(&mut app);
            let pane = app.term_area;
            shift(&mut app, 'P', &mut out);
            assert_eq!(app.panels_hidden, [true, false, false]);
            assert_eq!(app.focus, Focus::Worktrees, "FOCUS steps off the rail");
            assert!(crate::config::Config::load().hide_projects);
            let (terminal, text) = draw(&mut app);
            assert!(!text.contains("PROJECTS"), "{text}");
            assert!(text.contains("⇧P: show projects"), "{text}");
            assert_eq!(terminal.backend().buffer()[(0, 1)].symbol(), "▶");
            assert_eq!(app.term_area.x, pane.x - 30 + crate::panels::RAIL_W);
            assert_eq!(app.panels_widths, Some([30, 22, 32]), "remembered");
            shift(&mut app, 'P', &mut out);
            assert_eq!(app.panels_hidden, [false; 3]);
            assert_eq!(app.focus, Focus::Worktrees, "opening takes no FOCUS");
            assert!(!crate::config::Config::load().hide_projects);
            let (_, text) = draw(&mut app);
            assert!(text.contains("PROJECTS") && !text.contains("show projects"));
            assert_eq!(app.term_area, pane, "back at its width");
        });
    }

    /// `⇧B` folds WORKTREES and `⇧S` SESSIONS the same way, each on its
    /// own; the GRID keeps what the keymap binds the chords to.
    #[test]
    fn shift_b_and_shift_s_fold_their_columns_only_beside_them() {
        with_config_json("{}", || {
            let mut app = panels_app();
            let mut out = Vec::new();
            app.focus = Focus::Projects;
            draw(&mut app);
            shift(&mut app, 'B', &mut out);
            shift(&mut app, 'S', &mut out);
            assert_eq!(app.panels_hidden, [false, true, true]);
            let (_, text) = draw(&mut app);
            assert!(text.contains("⇧B: show worktrees"), "{text}");
            assert!(text.contains("⇧S: show sessions"), "{text}");
            assert!(!text.contains("WORKTREES") && !text.contains("SESSIONS"));
            let saved = crate::config::Config::load();
            assert!(saved.hide_worktrees && saved.hide_sessions);
            app.panels = false;
            app.panels_hidden = [false; 3];
            app.focus = Focus::Sessions;
            draw(&mut app);
            for c in ['P', 'B', 'S'] {
                shift(&mut app, c, &mut out);
                app.overlay = None;
            }
            assert_eq!(app.panels_hidden, [false; 3], "the grid has no columns");
        });
    }

    /// `^B` folds every column and puts FOCUS in the pane; again, with
    /// none open, it brings them all back. `⇧Z` is the same key.
    #[test]
    fn ctrl_b_folds_every_column_and_brings_them_back() {
        with_config_json("{}", || {
            let mut app = panels_app();
            let mut out = Vec::new();
            app.focus = Focus::Sessions;
            draw(&mut app);
            let pane = app.term_area;
            press(
                &mut app,
                KeyCode::Char('b'),
                KeyModifiers::CONTROL,
                &mut out,
            );
            assert_eq!(app.panels_hidden, [true; 3]);
            assert_eq!(app.focus, Focus::Terminal);
            assert_eq!(app.flash.as_deref(), Some("panels collapsed"));
            let (_, text) = draw(&mut app);
            let rails = 3 * crate::panels::RAIL_W;
            assert_eq!(app.term_area.x, pane.x - (20 + 22 + 32) + rails, "{text}");
            shift(&mut app, 'Z', &mut out);
            assert_eq!(app.panels_hidden, [false; 3]);
            assert_eq!(app.flash.as_deref(), Some("panels expanded"));
            assert_eq!(app.focus, Focus::Terminal, "FOCUS stays");
            // One column still open is enough to fold them all.
            shift(&mut app, 'P', &mut out);
            shift(&mut app, 'Z', &mut out);
            assert_eq!(app.panels_hidden, [true; 3]);
        });
    }

    /// `h` / `l` and Tab step over a folded column.
    #[test]
    fn the_walk_skips_a_folded_column() {
        with_config_json("{}", || {
            let mut app = panels_app();
            let mut out = Vec::new();
            app.focus = Focus::Projects;
            shift(&mut app, 'B', &mut out);
            key(&mut app, 'l', &mut out);
            assert_eq!(app.focus, Focus::Sessions);
            key(&mut app, 'h', &mut out);
            assert_eq!(app.focus, Focus::Projects);
            press(&mut app, KeyCode::Tab, KeyModifiers::NONE, &mut out);
            assert_eq!(app.focus, Focus::Sessions);
            // PROJECTS folded too: `h` from SESSIONS has nowhere to go.
            shift(&mut app, 'P', &mut out);
            key(&mut app, 'h', &mut out);
            assert_eq!(app.focus, Focus::Sessions);
        });
    }

    /// The folds outlive a restart through CONFIG.JSON, PROJECTS folded
    /// opening with FOCUS on the first open column; the Settings rows read
    /// and flip the same keys.
    #[test]
    fn the_folds_outlive_a_restart() {
        use crate::config::SettingKind;
        with_config_json(r#"{"layout": "panels", "hide_projects": true}"#, || {
            let mut app = panels_app();
            app.focus = Focus::Projects;
            let mut cfg = crate::config::Config::load();
            apply_config(&mut app, &cfg);
            assert_eq!(app.panels_hidden, [true, false, false]);
            assert_eq!(app.focus, Focus::Worktrees);
            assert_eq!(cfg.value_label(SettingKind::HideProjects), "hidden");
            let (tab, row) = crate::config::locate(SettingKind::HideSessions).unwrap();
            cfg.cycle(tab, row, 1);
            apply_config(&mut app, &cfg);
            assert_eq!(app.panels_hidden, [true, false, true]);
            let (_, text) = draw(&mut app);
            assert!(text.contains("⇧S: show sessions"), "{text}");
        });
    }

    /// The `◀` on a column's title folds it, and a click on its RAIL — the
    /// one cell its neighbor's BORDER zone stops short of — opens it at
    /// the width it had, arming no drag.
    #[test]
    fn the_chevron_folds_a_column_and_its_rail_opens_it() {
        with_config_json("{}", || {
            let mut app = panels_app();
            let mut out = Vec::new();
            app.panels_widths = Some([24, 26, 32]);
            app.focus = Focus::Sessions;
            draw(&mut app);
            let down = MouseEventKind::Down(MouseButton::Left);
            let chevron = hit_rect(&app, HitTarget::PanelsFold(Focus::Worktrees));
            mouse_at(&mut app, down, (chevron.x, chevron.y), &mut out);
            assert_eq!(app.panels_hidden, [false, true, false]);
            assert_eq!(app.focus, Focus::Sessions, "FOCUS was elsewhere");
            assert!(crate::config::Config::load().hide_worktrees);
            draw(&mut app);
            let rail = hit_rect(&app, HitTarget::PanelsFold(Focus::Worktrees));
            assert_eq!((rail.x, rail.width), (24, crate::panels::RAIL_W));
            let zone = hit_rect(&app, HitTarget::PanelsBorder(0));
            assert_eq!((zone.x, zone.width), (23, 1), "stops at the rule");
            assert!(!app
                .hits
                .iter()
                .any(|(_, h)| *h == HitTarget::PanelsBorder(1)));
            mouse_at(&mut app, down, (rail.x, rail.y + 1), &mut out);
            assert_eq!(app.panels_hidden, [false; 3]);
            assert!(app.panels_drag.is_none(), "no drag armed");
            draw(&mut app);
            assert_eq!(
                hit_rect(&app, HitTarget::PanelBg(Focus::Worktrees)).width,
                26
            );
            // A click on the chevron of the column FOCUS is on steps it on.
            app.focus = Focus::Projects;
            let chevron = hit_rect(&app, HitTarget::PanelsFold(Focus::Projects));
            mouse_at(&mut app, down, (chevron.x, chevron.y), &mut out);
            assert_eq!(app.focus, Focus::Worktrees);
        });
    }

    /// The WORKTREES cursor on a pull request or an issue folds SESSIONS to
    /// a bare rule — no chevron, no target, no restore hint — and the pane
    /// reading the row takes its width; a FOCUS left on SESSIONS steps
    /// back onto WORKTREES. The cursor back on a checkout opens it again,
    /// and `hide_sessions` is never written by the fold.
    #[test]
    fn sessions_folds_to_a_rule_beside_a_pull_request_or_an_issue() {
        use super::super::tests::{seed_issues, seed_open_prs};
        use crate::app::WorktreeRow;
        with_config_json("{}", || {
            let mut app = panels_app();
            let mut out = Vec::new();
            app.sel_project = app
                .project_rows()
                .iter()
                .position(|i| app.tree.projects[*i].name == "demo")
                .unwrap();
            seed_open_prs(&mut app, &[(7, "a fix")]);
            seed_issues(&mut app, &[(9, "a bug")]);
            draw(&mut app);
            let pane = app.term_area;
            let rows = app.worktree_rows();
            let pr = rows.iter().position(|r| matches!(r, WorktreeRow::Pr(_)));
            let issue = rows.iter().position(|r| matches!(r, WorktreeRow::Issue(_)));
            for at in [pr.unwrap(), issue.unwrap()] {
                app.sel_worktree = at;
                app.focus = Focus::Sessions;
                let (_, text) = draw(&mut app);
                assert_eq!(crate::panels::folds(&app)[2], crate::panels::Fold::Rule);
                assert_eq!(app.focus, Focus::Worktrees, "FOCUS steps back");
                assert!(!text.contains("SESSIONS"), "{text}");
                assert!(!text.contains("show sessions"), "{text}");
                assert!(!app.hits.iter().any(|(_, h)| matches!(
                    h,
                    HitTarget::PanelsFold(Focus::Sessions) | HitTarget::PanelBg(Focus::Sessions)
                )));
                assert_eq!(app.term_area.x, pane.x - 32 + crate::panels::RAIL_W);
                assert!(app.term_area.width > pane.width);
                key(&mut app, 'l', &mut out);
                assert_eq!(app.focus, Focus::Terminal, "l steps over the rule");
                app.focus = Focus::Worktrees;
            }
            assert!(!app.panels_hidden[2]);
            assert!(!crate::config::Config::load().hide_sessions);
            app.sel_worktree = 0;
            let (_, text) = draw(&mut app);
            assert!(text.contains("SESSIONS"), "{text}");
            assert_eq!(app.term_area, pane);
        });
    }

    /// `?` beside the columns lists the keys that fold them.
    #[test]
    fn question_mark_lists_the_fold_keys() {
        let mut app = panels_app();
        let mut out = Vec::new();
        draw(&mut app);
        key(&mut app, '?', &mut out);
        let text = draw_tall(&mut app);
        for want in [
            "⇧P / ⇧B / ⇧S",
            "collapse / expand Projects",
            "^b ⇧Z",
            "collapse every panel",
        ] {
            assert!(text.contains(want), "{want}: {text}");
        }
    }

    /// `demo`'s root checkout under the cursors, `agent-1` with `beta`
    /// beside it (newer, so first), the SESSIONS cursor on the first row.
    fn demo_root(app: &mut App) {
        app.sel_project = app
            .project_rows()
            .iter()
            .position(|i| app.tree.projects[*i].name == "demo")
            .unwrap();
        app.sel_worktree = 0;
        let beta = Agent {
            id: AgentId("b1".into()),
            name: "beta".into(),
            status_changed_at: 5,
            ..app.tree.agents[0].clone()
        };
        hse(
            app,
            ServerEvent::EntityUpserted {
                entity: Entity::Agent(beta),
            },
        );
        app.sel_session = 0;
        app.focus = Focus::Sessions;
    }

    /// The agent under the SESSIONS cursor.
    fn selected_agent(app: &App) -> AgentId {
        match app.selected_session_row() {
            Some(crate::app::SessionRow::Agent(a)) => a.id,
            other => panic!("no agent row: {other:?}"),
        }
    }

    fn follow_up_text(app: &App) -> Option<String> {
        app.follow_up.as_ref().map(|f| f.input.as_str().to_string())
    }

    /// Space expands the session's pill in place into its FOLLOW-UP
    /// COMPOSER — no modal — its chevron flipping to `▾`, the box straight
    /// under the name and the pill below pushed down by the box and the
    /// bottom pad the pill keeps. The box owns the keys: `a` is a letter,
    /// not an archive, and Enter sends the turn down the PTY and folds the
    /// pill back up.
    #[test]
    fn space_expands_the_session_pill_into_its_composer() {
        let mut app = panels_app();
        let mut out = Vec::new();
        demo_root(&mut app);
        let (_, text) = draw(&mut app);
        assert!(text.contains('▸') && !text.contains("follow-up"), "{text}");
        let name = hit_rect(&app, HitTarget::PanelsRow(Row::Session(0)));
        let below = hit_rect(&app, HitTarget::PanelsRow(Row::Session(1)));
        let id = selected_agent(&app);
        key(&mut app, ' ', &mut out);
        assert!(app.overlay.is_none(), "{:?}", app.overlay);
        assert_eq!(app.follow_up.as_ref().map(|f| f.agent.clone()), Some(id));
        let (terminal, text) = draw(&mut app);
        assert!(text.contains('▾'), "{text}");
        let buf = terminal.backend().buffer();
        let line = |y: u16| -> String {
            (name.x..name.right())
                .map(|x| buf[(x, y)].symbol().to_string())
                .collect()
        };
        assert!(
            line(name.y + 2).contains("╭─ follow-up"),
            "{}",
            line(name.y + 2)
        );
        assert!(line(name.y + 4).contains("Esc"), "{}", line(name.y + 4));
        let moved = hit_rect(&app, HitTarget::PanelsRow(Row::Session(1)));
        assert_eq!(moved.y, below.y + 4, "three rows of box and the bottom pad");
        for c in "a fix".chars() {
            key(&mut app, c, &mut out);
        }
        assert_eq!(follow_up_text(&app).as_deref(), Some("a fix"));
        assert!(
            app.overlay.is_none(),
            "`a` archived nothing: {:?}",
            app.overlay
        );
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE, &mut out);
        assert!(app.follow_up.is_none(), "sent, and folded back up");
        let sent: Vec<&[u8]> = out
            .iter()
            .filter_map(|r| match r {
                ClientRequest::Input { data, .. } => Some(data.as_slice()),
                _ => None,
            })
            .collect();
        assert_eq!(sent, [b"a fix".as_slice(), b"\r".as_slice()]);
    }

    /// Esc folds the pill without sending; a paste lands in the box,
    /// newlines and all; Tab walks on and leaves the box open behind it.
    #[test]
    fn the_composer_folds_on_esc_and_takes_a_paste() {
        let mut app = panels_app();
        let mut out = Vec::new();
        demo_root(&mut app);
        draw(&mut app);
        key(&mut app, ' ', &mut out);
        assert!(super::super::paste_into_follow_up(&mut app, "one\ntwo"));
        assert_eq!(follow_up_text(&app).as_deref(), Some("one\ntwo"));
        let (_, text) = draw(&mut app);
        assert!(text.contains("one") && text.contains("two"), "{text}");
        press(&mut app, KeyCode::Tab, KeyModifiers::NONE, &mut out);
        assert_eq!(app.focus, Focus::Terminal, "Tab walks on");
        assert!(app.follow_up.is_some(), "the box stays behind");
        app.focus = Focus::Sessions;
        press(&mut app, KeyCode::Esc, KeyModifiers::NONE, &mut out);
        assert!(app.follow_up.is_none());
        assert!(
            !out.iter().any(|r| matches!(r, ClientRequest::Input { .. })),
            "{out:?}"
        );
    }

    /// The chevron is its own click target: a click expands that pill —
    /// the cursor landing on it first — and a second folds it. A click in
    /// the open box only gives SESSIONS FOCUS, never attaching; a click on
    /// another pill folds the box.
    #[test]
    fn the_chevron_toggles_the_composer_and_the_box_is_its_own_target() {
        let mut app = panels_app();
        let mut out = Vec::new();
        demo_root(&mut app);
        draw(&mut app);
        let down = MouseEventKind::Down(MouseButton::Left);
        let chevron = hit_rect(&app, HitTarget::PanelsRow(Row::FollowUp(1)));
        assert_eq!(chevron.width, 2);
        mouse_at(&mut app, down, (chevron.x + 1, chevron.y), &mut out);
        assert_eq!(app.sel_session, 1, "the cursor came along");
        assert_eq!(app.follow_up_row(), Some(1));
        draw(&mut app);
        app.focus = Focus::Worktrees;
        let inside = hit_rect(&app, HitTarget::PanelsRow(Row::FollowUpBox));
        assert_eq!(inside.height, 3);
        out.clear();
        mouse_at(&mut app, down, (inside.x + 3, inside.y + 1), &mut out);
        mouse_at(&mut app, down, (inside.x + 3, inside.y + 1), &mut out);
        assert_eq!(app.focus, Focus::Sessions);
        assert!(app.follow_up.is_some() && !app.term_locked);
        assert!(
            !out.iter()
                .any(|r| matches!(r, ClientRequest::Attach { .. })),
            "{out:?}"
        );
        let chevron = hit_rect(&app, HitTarget::PanelsRow(Row::FollowUp(1)));
        mouse_at(&mut app, down, (chevron.x, chevron.y), &mut out);
        assert!(app.follow_up.is_none(), "a second click folds it");
        key(&mut app, ' ', &mut out);
        draw(&mut app);
        let other = hit_rect(&app, HitTarget::PanelsRow(Row::Session(0)));
        mouse_at(&mut app, down, (other.x + 2, other.y + 1), &mut out);
        assert!(app.follow_up.is_none(), "another pill folds the box");
        assert_eq!(app.sel_session, 0);
    }

    /// Only a live local agent wears a chevron or takes Space: an archived
    /// row says why not, and so does a terminal.
    #[test]
    fn only_a_live_agent_takes_a_follow_up() {
        let mut app = panels_app();
        let mut out = Vec::new();
        demo_root(&mut app);
        app.tree.terminals.push(nebula_core::TerminalTab {
            id: nebula_core::TerminalId("t1".into()),
            worktree_id: WorktreeId("w1".into()),
            name: "shell".into(),
            sort_order: 0,
            alive: true,
            run_command: None,
        });
        draw(&mut app);
        let rows = app.visible_session_rows();
        let terminal = rows
            .iter()
            .position(|r| matches!(r, crate::app::SessionRow::Terminal(_)))
            .unwrap();
        assert!(!app
            .hits
            .iter()
            .any(|(_, h)| *h == HitTarget::PanelsRow(Row::FollowUp(terminal))));
        app.sel_session = terminal;
        key(&mut app, ' ', &mut out);
        assert!(app.follow_up.is_none());
        assert!(app.flash.take().is_some(), "says what a terminal takes");
    }

    /// The menu's **Follow-up prompt** beside the columns expands the pill,
    /// as Space does, rather than opening the grid's modal.
    #[test]
    fn the_menus_follow_up_expands_the_pill() {
        let mut app = panels_app();
        let mut out = Vec::new();
        demo_root(&mut app);
        draw(&mut app);
        super::super::run_menu_action(&mut app, crate::app::MenuAction::FollowUp, &mut out);
        assert!(
            !matches!(
                &app.overlay,
                Some(Overlay::Prompt(p)) if matches!(p.kind, PromptKind::FollowUp { .. })
            ),
            "{:?}",
            app.overlay
        );
        assert_eq!(app.follow_up_row(), Some(0));
    }

    /// In a column taller than the screen the window follows the expanded
    /// pill, and keeps its box on screen as it grows.
    #[test]
    fn the_scroll_keeps_the_open_box_on_screen() {
        let mut app = panels_app();
        let mut out = Vec::new();
        long_sessions(&mut app, 40);
        app.focus = Focus::Sessions;
        draw(&mut app);
        for _ in 0..11 {
            key(&mut app, 'j', &mut out);
        }
        draw(&mut app);
        key(&mut app, ' ', &mut out);
        for _ in 0..3 {
            press(
                &mut app,
                KeyCode::Char('j'),
                KeyModifiers::CONTROL,
                &mut out,
            );
            key(&mut app, 'x', &mut out);
        }
        draw(&mut app);
        let list = hit_rect(&app, HitTarget::PanelBg(Focus::Sessions));
        let inside = hit_rect(&app, HitTarget::PanelsRow(Row::FollowUpBox));
        assert_eq!(inside.height, 2 + 4, "four lines of typing");
        assert!(inside.bottom() <= list.bottom(), "{inside:?} in {list:?}");
    }

    /// Three prompts in agent `id`'s history, oldest first.
    fn with_prompts(app: &mut App, id: &str) {
        let at = app.tree.agents.iter().position(|a| a.id.0 == id).unwrap();
        let now = crate::app::now_ms();
        app.tree.agents[at].recent_prompts = ["fix the redirect", "add a test", "why is CI slow"]
            .iter()
            .enumerate()
            .map(|(i, text)| nebula_core::PromptEntry {
                text: (*text).into(),
                submitted_at: now - (3 - i as i64) * 60_000,
            })
            .collect();
    }

    /// With RECENT PROMPTS on, a session's pill lists its newest prompts
    /// under its name — oldest first, the newest at the bottom — and grows
    /// by them, keeping its bottom pad; off, the pill is two rows again.
    /// The setting is read through CONFIG.JSON.
    #[test]
    fn recent_prompts_hang_under_a_session_pill() {
        with_config_json(
            r#"{"layout": "panels", "recent_prompts": true, "recent_prompts_count": 2}"#,
            || {
                let mut app = panels_app();
                apply_config(&mut app, &crate::config::Config::load());
                assert_eq!(app.recent_prompts, 2);
                demo_root(&mut app);
                with_prompts(&mut app, "b1");
                let (terminal, text) = draw(&mut app);
                assert!(!text.contains("fix the redirect"), "{text}");
                let pill = hit_rect(&app, HitTarget::PanelsRow(Row::Session(0)));
                assert_eq!(pill.height, 3 + 2, "the pill grew by its two lines");
                let below = hit_rect(&app, HitTarget::PanelsRow(Row::Session(1)));
                assert_eq!(below.y, pill.y + 5);
                let buf = terminal.backend().buffer();
                let line = |y: u16| -> String {
                    (pill.x..pill.right())
                        .map(|x| buf[(pill.x.max(x), y)].symbol().to_string())
                        .collect()
                };
                assert!(
                    line(pill.y + 2).contains("· add a test"),
                    "{}",
                    line(pill.y + 2)
                );
                assert!(
                    line(pill.y + 3).contains("· why is CI slow"),
                    "{}",
                    line(pill.y + 3)
                );
                assert!(line(pill.y + 3).contains("ago"), "{}", line(pill.y + 3));
                // The FOLLOW-UP COMPOSER opens under the prompts.
                key(&mut app, ' ', &mut Vec::new());
                draw(&mut app);
                let inside = hit_rect(&app, HitTarget::PanelsRow(Row::FollowUpBox));
                assert_eq!(inside.y, pill.y + 4);
                apply_config(&mut app, &crate::config::Config::default());
                app.panels = true;
                app.follow_up = None;
                draw(&mut app);
                let pill = hit_rect(&app, HitTarget::PanelsRow(Row::Session(0)));
                assert_eq!(pill.height, 2, "off: the pill shares its pad again");
            },
        );
    }

    /// A click on an archived session selects it and says why it neither
    /// previews nor attaches; a second click attaches nothing.
    #[test]
    fn a_click_on_an_archived_session_says_why() {
        let mut app = panels_app();
        let mut out = Vec::new();
        demo_root(&mut app);
        app.show_archived = true;
        let at = app.tree.agents.iter().position(|a| a.id.0 == "b1").unwrap();
        app.tree.agents[at].archived = true;
        draw(&mut app);
        let rows = app.visible_session_rows();
        let gone = rows.iter().position(|r| r.is_archived_agent()).unwrap();
        let rect = hit_rect(&app, HitTarget::PanelsRow(Row::Session(gone)));
        let down = MouseEventKind::Down(MouseButton::Left);
        mouse_at(&mut app, down, (rect.x + 2, rect.y + 1), &mut out);
        mouse_at(&mut app, down, (rect.x + 2, rect.y + 1), &mut out);
        assert_eq!(app.flash.as_deref(), Some(super::super::AGENT_ARCHIVED));
        assert!(!app.term_locked);
    }
}
