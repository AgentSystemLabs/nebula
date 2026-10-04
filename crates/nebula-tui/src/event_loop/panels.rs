//! The PANELS' keys and clicks (`crate::panels` is the layout's model,
//! `ui::panels_view` its drawing). Most of what the panels answer to is
//! the panel walk `event_loop::handle_key` has always kept under the GRID —
//! `h`/`l` across the columns (`focus_walk`), `j`/`k` down them
//! (`move_selection`), Enter into the pane, and every verb that reads the
//! selection — so this module only takes the keys the GRID owns and gives
//! them their panel meaning, or a word saying they have none here
//! ([`handle_action`]), and translates a click on a row ([`click_row`]), a
//! drag of a column's BORDER ([`grab_border`], [`move_border`]) or a
//! notch of the wheel over a column ([`wheel`]).

use super::{
    activate, attach_selected, is_double_click, jump_attention, launcher, select_project_row,
    select_session_row, select_worktree_row, toggle_issues, toggle_open_prs, zoom_pane,
};
use crate::app::{App, Focus, HitTarget, RowKey};
use crate::keymap::Action;
use crate::panels::Row;
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
/// * `Space` on a session: the FOLLOW-UP MODAL the grid's card opens
///   (`launcher::follow_up`). Off the SESSIONS column there is no session
///   under the cursor to prompt, and it does nothing.
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
        Action::FollowUp => {
            if app.focus == Focus::Sessions {
                launcher::follow_up(app);
            }
        }
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

/// `^F` beside the columns: the session in the pane full-screen with the
/// input lock on — from the SESSIONS column the row under the cursor,
/// attached first, exactly as Enter on it would; from the pane, or the
/// columns above, whatever the pane is showing. `^F` (or `^q`) from the
/// full-screen session comes back down to the pane with the keys still in
/// it (`launcher::toggle_full_screen`, which the locked pane runs).
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
/// session in the pane. False for a header, which has no cursor to move
/// and no menu of its own.
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
        Row::Session(i) => select_session_row(app, i, Duration::ZERO, out),
        Row::OpenPrsHeader | Row::IssuesHeader | Row::ArchivedHeader => return false,
    }
    app.focus = row.focus();
    app.dirty = true;
    true
}

/// A left click on the PANELS: a row takes the cursor ([`select_row`]),
/// and a second click on the same one is Enter on it — a checkout hands
/// FOCUS to its sessions, a session is attached and takes the keys. A
/// group header folds its group, or opens it.
pub(super) fn click_row(app: &mut App, row: Row, out: &mut Vec<ClientRequest>) {
    match row {
        Row::OpenPrsHeader => toggle_open_prs(app, out),
        Row::IssuesHeader => toggle_issues(app, out),
        Row::ArchivedHeader => toggle_archived(app, out),
        Row::Project(_) => {
            select_row(app, row, out);
        }
        Row::Worktree(_) => {
            select_row(app, row, out);
            let Some(id) = app.selected_worktree().map(|w| w.id.clone()) else {
                return;
            };
            if is_double_click(&mut app.last_session_click, RowKey::Worktree(id)) {
                activate::worktrees_row(app, out);
            }
        }
        Row::Session(_) => {
            select_row(app, row, out);
            let Some(sref) = app.selected_session_row().and_then(|r| r.sref()) else {
                return;
            };
            if is_double_click(&mut app.last_session_click, RowKey::Session(sref)) {
                attach_selected(app, out);
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
    let border = crate::panels::columns(app.body_area, app.panels_widths).border(column);
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
    if let Some(widths) = crate::panels::drag_border(app.body_area, app.panels_widths, column, to) {
        app.panels_widths = Some(widths);
    }
    app.dirty = true;
}

/// Lines a notch of the wheel scrolls a PANELS column: a third of the
/// GRID's card, as `launcher::GRID_WHEEL_ROWS`.
const WHEEL_LINES: isize = 3;

/// A notch of the wheel over a PANELS column — a row, a group header or
/// the air under them — scrolls that column a few lines under a cursor
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
    let delta = if up { -WHEEL_LINES } else { WHEEL_LINES };
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
        for row in ["demo", "web", "main ⌂ root", "feat", "RECENT"] {
            assert!(text.contains(row), "{row}: {text}");
        }
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

    /// The grid's own keys never strand the panels: Space is the
    /// FOLLOW-UP MODAL on a session, and the PROJECT TAB keys, the pane
    /// fold and its strip only say they have nothing to act on here.
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
        assert!(
            matches!(
                &app.overlay,
                Some(Overlay::Prompt(p)) if matches!(p.kind, PromptKind::FollowUp { .. })
            ),
            "{:?}",
            app.overlay
        );
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
    /// again, in place: the cursor stays in the checkout it was in.
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
            text.contains("▸ ARCHIVED · 1") && !text.contains("old-run"),
            "{text}"
        );
        press(&mut app, KeyCode::Char('A'), KeyModifiers::SHIFT, &mut out);
        let (_, text) = draw(&mut app);
        assert!(
            text.contains("▾ ARCHIVED · 1") && text.contains("old-run"),
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
        assert_eq!(app.panels_scroll[2].top, 3, "three lines a notch");
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

    /// The scroll holds at the column's first line and at the one that
    /// puts the last line on the bottom row.
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
        let (_, text) = draw(&mut app);
        for want in [
            "THE COLUMNS",
            "walk the three columns",
            "session: follow-up modal",
            "fold the ARCHIVED group",
            "project tabs: none here",
            "resize the column",
        ] {
            assert!(text.contains(want), "{want}: {text}");
        }
        for gone in [
            "NAVIGATE & SEARCH",
            "walk the cards",
            "fold / unfold the pane",
        ] {
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
        assert!(!text.contains("THE COLUMNS"), "{text}");
    }
}
