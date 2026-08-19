//! The main TUI loop: terminal setup/teardown, message routing, update logic.

use crate::app::{
    agent_sref, App, AttachedTerm, ConfirmDialog, ConnState, ContextMenu, DiffView, Focus,
    HitTarget, MenuAction, MenuItem, Overlay, Palette, PaletteTarget, PendingAction, PendingIntent,
    ProjectRow, PromptDialog, PromptKind, SettingsView, SplitterDrag, TermSelection,
    WorktreeRollback,
};
use crate::{ipc, keys, ui};
use anyhow::Result;
use crossterm::event::{
    Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use futures::StreamExt;
use nebula_core::{
    AgentId, AgentKind, ClientRequest, EntityId, ServerEvent, SessionRef, WorktreeId,
};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;
use std::io::{BufWriter, Stdout};
use std::time::Duration;

/// Redraw cap (~60fps). Output bursts coalesce into one frame; input events
/// are still handled immediately between frames.
const FRAME_INTERVAL: Duration = Duration::from_millis(16);

pub async fn run_app() -> Result<()> {
    let conn = ipc::connect_or_spawn().await?;
    let mut channels = ipc::split_connection(conn);
    channels.tx.send(ClientRequest::Subscribe).await?;

    let mut terminal = setup_terminal()?;
    let result = main_loop(&mut terminal, &mut channels).await;
    restore_terminal();
    result
}

/// Whether we pushed kitty keyboard flags on the outer terminal (so restore —
/// including the panic hook — knows to pop them).
static KITTY_PUSHED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

fn setup_terminal() -> Result<Terminal<CrosstermBackend<BufWriter<Stdout>>>> {
    use crossterm::{execute, terminal::*};
    enable_raw_mode()?;
    let mut stdout = std::io::stdout();
    execute!(
        stdout,
        EnterAlternateScreen,
        crossterm::event::EnableMouseCapture,
        crossterm::event::EnableBracketedPaste,
    )?;
    // Kitty keyboard protocol on the outer terminal: without it, Cmd-combos
    // never reach us and Option/Esc combos arrive ambiguous. Probe first —
    // Terminal.app and friends don't speak it (must happen before the
    // EventStream exists; the probe reads stdin).
    if matches!(supports_keyboard_enhancement(), Ok(true)) {
        use crossterm::event::{KeyboardEnhancementFlags, PushKeyboardEnhancementFlags};
        execute!(
            stdout,
            PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES)
        )?;
        KITTY_PUSHED.store(true, std::sync::atomic::Ordering::Relaxed);
    }
    // Panic hook: restore the user's terminal before the panic message prints.
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        restore_terminal();
        default_hook(info);
    }));
    // Buffered so a full-frame redraw reaches the terminal in a few large
    // writes instead of one syscall per line (Stdout is line-buffered).
    let writer = BufWriter::with_capacity(64 * 1024, std::io::stdout());
    Ok(Terminal::new(CrosstermBackend::new(writer))?)
}

pub fn restore_terminal() {
    use crossterm::{execute, terminal::*};
    // Pop while still on the alternate screen — kitty keeps a keyboard-flag
    // stack per screen, so the pop must land on the screen that pushed.
    if KITTY_PUSHED.swap(false, std::sync::atomic::Ordering::Relaxed) {
        let _ = execute!(
            std::io::stdout(),
            crossterm::event::PopKeyboardEnhancementFlags
        );
    }
    let _ = execute!(
        std::io::stdout(),
        crossterm::event::DisableBracketedPaste,
        crossterm::event::DisableMouseCapture,
        LeaveAlternateScreen,
    );
    let _ = disable_raw_mode();
}

async fn main_loop(
    terminal: &mut Terminal<CrosstermBackend<BufWriter<Stdout>>>,
    channels: &mut ipc::IpcChannels,
) -> Result<()> {
    let mut app = App::new();
    app.conn = ConnState::Connected;
    let cfg = crate::config::Config::load();
    app.recent_window_ms = cfg.recent_window_ms();
    app.theme = cfg.theme();
    let mut input = crossterm::event::EventStream::new();
    let mut out: Vec<ClientRequest> = Vec::new();
    let mut next_draw = tokio::time::Instant::now();

    loop {
        if app.dirty && tokio::time::Instant::now() >= next_draw {
            terminal.draw(|f| ui::draw(f, &mut app))?;
            app.dirty = false;
            next_draw = tokio::time::Instant::now() + FRAME_INTERVAL;
            sync_pty_size(&mut app, &mut out);
            // What the user sees selected, for RECENT-expiry re-anchoring.
            app.drawn_session = app.selected_session().map(|a| a.id.clone());
        }

        // Wake when the next RECENT session ages out so the list regroups
        // (slack so the wakeup lands past the boundary).
        let recent_expiry = app.next_recent_expiry();

        let focus_before = app.focus;
        tokio::select! {
            // Pending redraw: wake at the frame boundary even if no new
            // events arrive.
            _ = tokio::time::sleep_until(next_draw), if app.dirty => {}
            _ = tokio::time::sleep(recent_expiry.unwrap_or_default() + Duration::from_millis(250)),
                if recent_expiry.is_some() =>
            {
                // Re-read the config so window edits apply without restart.
                app.recent_window_ms = crate::config::Config::load().recent_window_ms();
                // The expired session dropped down the list; keep the
                // selection on whatever row the user had selected.
                if let Some(keep) = app.drawn_session.clone() {
                    if let Some(i) = app.visible_sessions().iter().position(|a| a.id == keep) {
                        app.sel_session = i;
                    }
                }
                app.dirty = true;
            }
            ev = input.next() => match ev {
                Some(Ok(event)) => {
                    tracing::debug!(?event, "terminal event");
                    handle_terminal_event(&mut app, event, &mut out);
                }
                Some(Err(_)) | None => app.should_quit = true,
            },
            ev = channels.rx.recv() => match ev {
                Some(server_event) => {
                    log_server_event(&server_event);
                    handle_server_event(&mut app, server_event, &mut out);
                }
                None => {
                    app.conn = ConnState::Disconnected;
                    app.flash = Some("daemon connection lost".into());
                    app.dirty = true;
                }
            },
        }
        if app.focus != focus_before {
            tracing::debug!(from = ?focus_before, to = ?app.focus, "focus changed");
        }

        // Drain whatever else is immediately ready before redrawing once
        // (burst coalescing for PTY output).
        while let Ok(ev) = channels.rx.try_recv() {
            log_server_event(&ev);
            handle_server_event(&mut app, ev, &mut out);
        }

        for req in out.drain(..) {
            if channels.tx.send(req).await.is_err() {
                app.conn = ConnState::Disconnected;
                app.dirty = true;
            }
        }

        if app.should_quit {
            // Persist selection so the next launch restores it.
            let _ = channels
                .tx
                .send(ClientRequest::SaveUiState {
                    json: ui_state_json(&app),
                })
                .await;
            return Ok(());
        }
    }
}

fn log_server_event(ev: &ServerEvent) {
    match ev {
        ServerEvent::Output { .. } | ServerEvent::Scrollback { .. } => {}
        other => tracing::debug!(event = ?other, "server event"),
    }
}

fn ui_state_json(app: &App) -> String {
    use crate::app::UiState;
    let state = UiState {
        project: app.selected_project().map(|p| p.id.to_string()),
        worktree: app.selected_worktree().map(|w| w.id.to_string()),
        session_agent: app.selected_session().map(|a| a.id.to_string()),
        show_archived: app.show_archived,
        collapsed: app.collapsed,
        panel_widths: Some(app.panel_widths),
        diff_files_width: Some(app.diff_files_width),
    };
    serde_json::to_string(&state).unwrap_or_else(|_| "{}".into())
}

fn restore_ui_state(app: &mut App, json: &str) {
    use crate::app::UiState;
    let Ok(state) = serde_json::from_str::<UiState>(json) else {
        return;
    };
    app.show_archived = state.show_archived;
    if let Some(w) = state.panel_widths {
        // Coarse sanity clamp; normalize_panel_widths re-fits to the actual
        // screen on the next draw.
        app.panel_widths = w.map(|v| v.clamp(crate::app::MIN_PANEL_W, 300));
    }
    if let Some(w) = state.diff_files_width {
        // Coarse sanity clamp; the draw re-caps it to the actual modal width.
        app.diff_files_width = w.clamp(crate::app::MIN_DIFF_FILES_W, 300);
    }
    if let Some(pid) = &state.project {
        let row = app.project_rows().iter().position(
            |r| matches!(r, ProjectRow::Project(i) if app.tree.projects[*i].id.as_str() == pid),
        );
        if let Some(i) = row {
            app.sel_project = i;
        }
    }
    if let Some(wid) = &state.worktree {
        if let Some(i) = app
            .visible_worktrees()
            .iter()
            .position(|w| w.id.as_str() == wid)
        {
            app.sel_worktree = i;
        }
    }
    if let Some(sid) = state.session_agent {
        if let Some(i) = app
            .visible_sessions()
            .iter()
            .position(|a| a.id.as_str() == sid)
        {
            app.sel_session = i;
        }
    }
}

/// Keep the vt100 parser and the daemon PTY sized to the drawn pane.
fn sync_pty_size(app: &mut App, out: &mut Vec<ClientRequest>) {
    let area = app.term_area;
    if area.width < 2 || area.height < 2 {
        return;
    }
    if let Some(term) = &mut app.term {
        if (term.cols, term.rows) != (area.width, area.height) {
            // The grid reflows; a screen-anchored selection would drift.
            app.term_selection = None;
            term.cols = area.width;
            term.rows = area.height;
            term.parser.set_size(area.height, area.width);
            out.push(ClientRequest::Resize {
                session: term.sref.clone(),
                cols: area.width,
                rows: area.height,
            });
        }
    }
}

fn handle_terminal_event(app: &mut App, event: Event, out: &mut Vec<ClientRequest>) {
    match event {
        Event::Key(key) if key.kind != KeyEventKind::Release => {
            app.flash = None;
            handle_key(app, key, out);
            app.dirty = true;
        }
        Event::Mouse(mouse) => handle_mouse(app, mouse, out),
        Event::Paste(text) => {
            if app.focus == Focus::Terminal && app.term_locked {
                if let Some(term) = &app.term {
                    // Bracketed paste so the child (claude, vim…) knows.
                    let mut data = b"\x1b[200~".to_vec();
                    data.extend_from_slice(text.as_bytes());
                    data.extend_from_slice(b"\x1b[201~");
                    out.push(ClientRequest::Input {
                        session: term.sref.clone(),
                        data,
                    });
                }
            }
        }
        Event::Resize(_, _) => app.dirty = true,
        _ => {}
    }
}

fn handle_key(app: &mut App, key: KeyEvent, out: &mut Vec<ClientRequest>) {
    // Modal overlays swallow all keys.
    if app.overlay.is_some() {
        handle_overlay_key(app, key, out);
        return;
    }

    // Terminal input-locked with a live session: forward everything except
    // the escape hatches. Merely focusing the pane (Tab / Ctrl+arrows) does
    // not lock — Enter does — so an unlocked pane falls through to panel
    // navigation and the user is never trapped.
    if app.focus == Focus::Terminal && app.term.is_some() && app.term_locked {
        // Ctrl+q is the primary hatch: a plain control byte (0x11) that
        // every emulator delivers — Terminal.app included, no kitty protocol
        // needed — unbound in macOS and unused by Claude Code. The inner
        // session loses XON (unfreeze after an accidental Ctrl+S), which
        // nobody will miss.
        // Fallback hatches: Ctrl+] (telnet's escape char — byte 0x1D, which
        // crossterm spells Ctrl+5 in legacy mode), Ctrl+Esc (kitty-only),
        // and Ctrl+← (stolen by Mission Control on stock macOS).
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let ctrl_q = ctrl && key.code == KeyCode::Char('q');
        let ctrl_bracket = ctrl && matches!(key.code, KeyCode::Char(']') | KeyCode::Char('5'));
        let ctrl_esc = ctrl && key.code == KeyCode::Esc;
        let ctrl_left = ctrl && key.code == KeyCode::Left;
        if ctrl_q || ctrl_bracket || ctrl_esc || ctrl_left {
            // Escape hatch: also expands collapsed sidebars.
            app.collapsed = false;
            app.term_locked = false;
            app.focus = Focus::Sessions;
            return;
        }
        let exited = app.term.as_ref().is_some_and(|t| t.exited);
        if !exited {
            if let Some(term) = &mut app.term {
                // Typing changes the content under a persisted selection
                // highlight — drop it.
                app.term_selection = None;
                // Typing exits scroll mode (tmux behavior).
                if term.scroll > 0 {
                    term.set_scroll(0);
                }
                if let Some(data) = keys::encode_key(&key, term.kitty_flags) {
                    out.push(ClientRequest::Input {
                        session: term.sref.clone(),
                        data,
                    });
                }
            }
            return;
        }
        // Exited session: there is nothing to type into, so don't swallow
        // keys. Esc/Enter/q go back to the session list; everything else
        // falls through to panel navigation.
        if matches!(key.code, KeyCode::Esc | KeyCode::Enter | KeyCode::Char('q')) {
            app.collapsed = false;
            app.term_locked = false;
            app.focus = Focus::Sessions;
            return;
        }
    }

    // Panel focus: navigation + action keymap.
    match key.code {
        KeyCode::Char('q') => app.should_quit = true,
        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            app.should_quit = true
        }
        KeyCode::Char('?') => app.overlay = Some(Overlay::Help),
        KeyCode::Char('s') => app.overlay = Some(Overlay::Settings(SettingsView::new())),
        KeyCode::Tab => {
            app.focus = match app.focus {
                Focus::Projects => Focus::Worktrees,
                Focus::Worktrees => Focus::Sessions,
                Focus::Sessions => Focus::Terminal,
                Focus::Terminal => Focus::Projects,
            }
        }
        KeyCode::BackTab => {
            app.focus = match app.focus {
                Focus::Projects => Focus::Terminal,
                Focus::Worktrees => Focus::Projects,
                Focus::Sessions => Focus::Worktrees,
                Focus::Terminal => Focus::Sessions,
            }
        }
        KeyCode::Char('h') | KeyCode::Left => {
            app.focus = match app.focus {
                Focus::Projects => Focus::Projects,
                Focus::Worktrees => Focus::Projects,
                Focus::Sessions => Focus::Worktrees,
                Focus::Terminal => Focus::Sessions,
            }
        }
        KeyCode::Char('l') | KeyCode::Right => {
            app.focus = match app.focus {
                Focus::Projects => Focus::Worktrees,
                Focus::Worktrees => Focus::Sessions,
                Focus::Sessions => Focus::Terminal,
                Focus::Terminal => Focus::Terminal,
            }
        }
        // Shift+↑/↓ (or Shift+j/k) reorders projects instead of moving the
        // selection; guarded arms must precede the plain ones.
        KeyCode::Down | KeyCode::Char('J')
            if app.focus == Focus::Projects && key.modifiers.contains(KeyModifiers::SHIFT) =>
        {
            move_project(app, 1, out)
        }
        KeyCode::Up | KeyCode::Char('K')
            if app.focus == Focus::Projects && key.modifiers.contains(KeyModifiers::SHIFT) =>
        {
            move_project(app, -1, out)
        }
        KeyCode::Char('j') | KeyCode::Down => move_selection(app, 1, out),
        KeyCode::Char('k') | KeyCode::Up => move_selection(app, -1, out),
        KeyCode::Enter => match app.focus {
            Focus::Projects => match app.selected_project_row() {
                Some(ProjectRow::Divider(i)) => {
                    let id = app.tree.projects[i].id.clone();
                    open_prompt(app, PromptKind::DividerLabel { id });
                }
                _ => app.focus = Focus::Worktrees,
            },
            Focus::Worktrees => app.focus = Focus::Sessions,
            Focus::Sessions => attach_selected(app, out),
            Focus::Terminal => {
                // Lock input into an already-focused live pane.
                if app.term.as_ref().is_some_and(|t| !t.exited) {
                    app.term_locked = true;
                }
            }
        },
        KeyCode::Char('n') => match app.focus {
            Focus::Projects => open_prompt(app, PromptKind::AddProject),
            Focus::Worktrees => {
                if let Some(p) = app.selected_project() {
                    open_prompt(
                        app,
                        PromptKind::NewWorktree {
                            project: p.id.clone(),
                        },
                    );
                }
            }
            Focus::Sessions => {
                if let Some(w) = app.selected_worktree() {
                    let worktree = w.id.clone();
                    open_new_agent_picker(app, worktree);
                }
            }
            Focus::Terminal => {}
        },
        KeyCode::Char('r') => match app.focus {
            Focus::Sessions => {
                if let Some(a) = app.selected_session() {
                    open_prompt(app, PromptKind::RenameAgent { id: a.id })
                }
            }
            Focus::Projects => {
                if let Some(ProjectRow::Divider(i)) = app.selected_project_row() {
                    let id = app.tree.projects[i].id.clone();
                    open_prompt(app, PromptKind::DividerLabel { id });
                }
            }
            _ => {}
        },
        KeyCode::Char('a') => {
            if app.focus == Focus::Sessions {
                if let Some(a) = app.selected_session() {
                    if !a.archived {
                        open_confirm_archive(app, &a);
                    }
                }
            }
        }
        KeyCode::Char('p') => {
            if app.focus == Focus::Sessions {
                if let Some(a) = app.selected_session() {
                    if a.archived {
                        app.flash = Some("agent is archived — unarchive first (u)".into());
                    } else {
                        // Keep the selection on this agent after it jumps
                        // between the PINNED/UNPINNED groups.
                        app.select_when_seen = Some(SessionRef::Agent(a.id.clone()));
                        let req_id = app.alloc_req_id(PendingIntent::None);
                        out.push(ClientRequest::SetAgentPinned {
                            req_id,
                            id: a.id,
                            pinned: !a.pinned,
                        });
                    }
                }
            }
        }
        KeyCode::Char('u') => {
            if app.focus == Focus::Sessions {
                if let Some(a) = app.selected_session() {
                    if a.archived {
                        let req_id = app.alloc_req_id(PendingIntent::None);
                        out.push(ClientRequest::UnarchiveAgent { req_id, id: a.id });
                    }
                }
            }
        }
        KeyCode::Char('A') => {
            if app.focus == Focus::Sessions {
                app.show_archived = !app.show_archived;
            }
        }
        // Fuzzy-search palette over every project / worktree / session.
        // The config read is per-open so edits apply without restarting.
        KeyCode::Char('/') => {
            if app.focus != Focus::Terminal {
                app.overlay = Some(Overlay::Palette(Palette::new(
                    &app.tree,
                    app.show_archived,
                    crate::config::Config::load().palette_enter_attaches,
                )));
            }
        }
        KeyCode::Char('-') => {
            if app.focus == Focus::Projects {
                match app.selected_project_row() {
                    Some(ProjectRow::Project(i)) => {
                        let p = &app.tree.projects[i];
                        let (id, divider_after) = (p.id.clone(), !p.divider_after);
                        let req_id = app.alloc_req_id(PendingIntent::None);
                        out.push(ClientRequest::SetProjectDivider {
                            req_id,
                            id,
                            divider_after,
                            label: None,
                        });
                    }
                    Some(ProjectRow::Divider(i)) => remove_divider(app, i, out),
                    None => {}
                }
            }
        }
        // Backspace is what a Mac "delete" key actually sends.
        KeyCode::Char('d') | KeyCode::Delete | KeyCode::Backspace => {
            match (app.focus, app.selected_project_row()) {
                // Dividers are cheap to recreate — no confirmation dance.
                (Focus::Projects, Some(ProjectRow::Divider(i))) => remove_divider(app, i, out),
                _ => open_delete_confirm(app),
            }
        }
        KeyCode::Char('m') => open_context_menu_for_selection(app),
        KeyCode::Char('g') => open_diff_view(app),
        KeyCode::Char('z') => {
            if app.term.is_some() {
                app.collapsed = true;
                app.focus = Focus::Terminal;
                app.term_locked = true;
            } else {
                app.flash = Some("attach a session first".into());
            }
        }
        _ => {}
    }
}

// ---- overlays ----

fn open_prompt(app: &mut App, kind: PromptKind) {
    let (title, label, input) = match &kind {
        PromptKind::AddProject => (
            "Add project".to_string(),
            "path to a git repository".to_string(),
            String::new(),
        ),
        PromptKind::DividerLabel { id } => {
            let current = app
                .tree
                .projects
                .iter()
                .find(|p| &p.id == id)
                .and_then(|p| p.divider_label.clone())
                .unwrap_or_default();
            (
                "Divider label".to_string(),
                "label (empty clears it)".to_string(),
                current,
            )
        }
        PromptKind::NewWorktree { .. } => (
            "New worktree".to_string(),
            "branch name".to_string(),
            String::new(),
        ),
        PromptKind::NewAgent { .. } => (
            "New agent".to_string(),
            format!("name (empty = {})", app.default_session_name("agent")),
            String::new(),
        ),
        PromptKind::RenameAgent { id } => {
            let current = app
                .tree
                .agents
                .iter()
                .find(|a| &a.id == id)
                .map(|a| a.name.clone())
                .unwrap_or_default();
            ("Rename agent".to_string(), "name".to_string(), current)
        }
    };
    app.overlay = Some(Overlay::Prompt(PromptDialog {
        title,
        label,
        input,
        kind,
        candidates: vec![],
    }));
}

fn open_diff_view(app: &mut App) {
    // Clone before touching app.overlay — selected_worktree borrows app.
    let Some((path, branch)) = app
        .selected_worktree()
        .map(|w| (w.path.clone(), w.branch.clone()))
    else {
        app.flash = Some("no worktree selected".into());
        return;
    };
    if !path.is_dir() {
        app.flash = Some(format!("worktree path missing on disk: {}", path.display()));
        return;
    }
    let files = match crate::git_diff::changed_files(&path) {
        Ok(files) => files,
        Err(msg) => {
            app.flash = Some(msg);
            return;
        }
    };
    if files.is_empty() {
        app.flash = Some(format!("no changes in {branch}"));
        return;
    }
    let head_ok = crate::git_diff::has_head(&path);
    let mut view = DiffView::new(path, branch, files, head_ok);
    view.files_width = app.diff_files_width;
    crate::git_diff::load_selected_diff(&mut view);
    app.overlay = Some(Overlay::Diff(view));
}

fn open_confirm_archive(app: &mut App, agent: &nebula_core::Agent) {
    app.overlay = Some(Overlay::Confirm(ConfirmDialog {
        title: "Archive agent".into(),
        message: format!(
            "Archive '{}'? Its running session will be stopped.",
            agent.name
        ),
        action: PendingAction::ArchiveAgent(agent.id.clone()),
    }));
}

fn open_delete_confirm(app: &mut App) {
    match app.focus {
        Focus::Projects => {
            if let Some(p) = app.selected_project() {
                app.overlay = Some(Overlay::Confirm(ConfirmDialog {
                    title: "Remove project".into(),
                    message: format!(
                        "Remove '{}' from nebula? Nothing on disk is touched.",
                        p.name
                    ),
                    action: PendingAction::RemoveProject(p.id.clone()),
                }));
            }
        }
        Focus::Worktrees => {
            if let Some(w) = app.selected_worktree() {
                if w.is_main {
                    app.flash = Some("cannot delete the main checkout".into());
                    return;
                }
                let live_here = app
                    .visible_sessions()
                    .iter()
                    .filter(|a| !a.archived)
                    .count();
                app.overlay = Some(Overlay::Confirm(ConfirmDialog {
                    title: "Delete worktree".into(),
                    message: format!(
                        "Delete worktree '{}' from disk? {live_here} session(s) will be killed.",
                        w.branch
                    ),
                    action: PendingAction::DeleteWorktree(w.id.clone()),
                }));
            }
        }
        Focus::Sessions => {
            if let Some(a) = app.selected_session() {
                app.overlay = Some(Overlay::Confirm(ConfirmDialog {
                    title: "Delete agent".into(),
                    message: format!(
                        "Delete agent '{}'? Its session and history go away.",
                        a.name
                    ),
                    action: PendingAction::DeleteAgent(a.id),
                }));
            }
        }
        Focus::Terminal => {}
    }
}

fn menu_items_for_session(a: &nebula_core::Agent) -> Vec<MenuItem> {
    if a.archived {
        vec![
            MenuItem {
                label: "Unarchive".into(),
                action: MenuAction::UnarchiveAgent(a.id.clone()),
                destructive: false,
            },
            MenuItem {
                label: "Delete".into(),
                action: MenuAction::DeleteAgent(a.id.clone()),
                destructive: true,
            },
        ]
    } else {
        vec![
            MenuItem {
                label: "Attach".into(),
                action: MenuAction::Attach(SessionRef::Agent(a.id.clone())),
                destructive: false,
            },
            MenuItem {
                label: "Restart".into(),
                action: MenuAction::RestartAgent(a.id.clone()),
                destructive: false,
            },
            MenuItem {
                label: if a.pinned { "Unpin" } else { "Pin" }.into(),
                action: MenuAction::SetAgentPinned(a.id.clone(), !a.pinned),
                destructive: false,
            },
            MenuItem {
                label: "Rename".into(),
                action: MenuAction::RenameAgent(a.id.clone()),
                destructive: false,
            },
            MenuItem {
                label: "Move to worktree".into(),
                action: MenuAction::MoveAgent(a.id.clone()),
                destructive: false,
            },
            MenuItem {
                label: "Archive".into(),
                action: MenuAction::ArchiveAgent(a.id.clone()),
                destructive: false,
            },
            MenuItem {
                label: "Delete".into(),
                action: MenuAction::DeleteAgent(a.id.clone()),
                destructive: true,
            },
        ]
    }
}

fn divider_menu_item(p: &nebula_core::Project) -> MenuItem {
    MenuItem {
        label: if p.divider_after {
            "Remove divider below"
        } else {
            "Add divider below"
        }
        .into(),
        action: MenuAction::SetProjectDivider(p.id.clone(), !p.divider_after),
        destructive: false,
    }
}

/// Menu for a selected divider row.
fn divider_row_menu(id: nebula_core::ProjectId) -> Vec<MenuItem> {
    vec![
        MenuItem {
            label: "Edit label".into(),
            action: MenuAction::LabelDivider(id.clone()),
            destructive: false,
        },
        MenuItem {
            label: "Remove divider".into(),
            action: MenuAction::SetProjectDivider(id, false),
            destructive: false,
        },
    ]
}

fn open_menu(app: &mut App, items: Vec<MenuItem>, at: (u16, u16)) {
    if items.is_empty() {
        return;
    }
    app.overlay = Some(Overlay::Menu(ContextMenu {
        title: None,
        items,
        at: Some(at),
        hover: 0,
        area: ratatui::layout::Rect::default(),
    }));
}

/// Step 1 of new-agent creation: pick which CLI the session runs. The
/// chosen kind chains into the name prompt via `MenuAction::NewAgentOfKind`.
fn open_new_agent_picker(app: &mut App, worktree: WorktreeId) {
    app.overlay = Some(Overlay::Menu(ContextMenu {
        title: Some("Agent type".into()),
        items: vec![
            MenuItem {
                label: "Claude".into(),
                action: MenuAction::NewAgentOfKind(worktree.clone(), AgentKind::Claude),
                destructive: false,
            },
            MenuItem {
                label: "Codex".into(),
                action: MenuAction::NewAgentOfKind(worktree.clone(), AgentKind::Codex),
                destructive: false,
            },
            MenuItem {
                label: "Cursor".into(),
                action: MenuAction::NewAgentOfKind(worktree, AgentKind::Cursor),
                destructive: false,
            },
        ],
        at: None,
        hover: 0,
        area: ratatui::layout::Rect::default(),
    }));
}

/// Step 1 of moving an agent: pick the destination — any other worktree of
/// the selected project. Chains into `MenuAction::MoveAgentToWorktree`.
fn open_move_agent_picker(app: &mut App, agent: AgentId) {
    let current = app
        .tree
        .agents
        .iter()
        .find(|a| a.id == agent)
        .map(|a| a.worktree_id.clone());
    let items: Vec<MenuItem> = app
        .visible_worktrees()
        .iter()
        .filter(|w| Some(&w.id) != current.as_ref())
        .map(|w| MenuItem {
            label: if w.is_main {
                format!("{} ⌂ root", w.branch)
            } else {
                w.branch.clone()
            },
            action: MenuAction::MoveAgentToWorktree(agent.clone(), w.id.clone()),
            destructive: false,
        })
        .collect();
    if items.is_empty() {
        app.flash = Some("no other worktree to move to".into());
        return;
    }
    app.overlay = Some(Overlay::Menu(ContextMenu {
        title: Some("Move to worktree".into()),
        items,
        at: None,
        hover: 0,
        area: ratatui::layout::Rect::default(),
    }));
}

fn open_context_menu_for_selection(app: &mut App) {
    // Keyboard-invoked menu: anchor near the selected row's panel.
    let at = (30, 4);
    match app.focus {
        Focus::Projects => {
            if let Some(ProjectRow::Divider(i)) = app.selected_project_row() {
                let id = app.tree.projects[i].id.clone();
                open_menu(app, divider_row_menu(id), at);
                return;
            }
            let mut items = vec![MenuItem {
                label: "Add project".into(),
                action: MenuAction::AddProject,
                destructive: false,
            }];
            if let Some(p) = app.selected_project() {
                items.insert(
                    0,
                    MenuItem {
                        label: "New worktree".into(),
                        action: MenuAction::NewWorktree(p.id.clone()),
                        destructive: false,
                    },
                );
                items.push(divider_menu_item(p));
                items.push(MenuItem {
                    label: "Remove from list".into(),
                    action: MenuAction::RemoveProject(p.id.clone()),
                    destructive: true,
                });
            }
            open_menu(app, items, at);
        }
        Focus::Worktrees => {
            if let Some(w) = app.selected_worktree() {
                let mut items = vec![MenuItem {
                    label: "New agent".into(),
                    action: MenuAction::NewAgent(w.id.clone()),
                    destructive: false,
                }];
                if !w.is_main {
                    items.push(MenuItem {
                        label: "Delete worktree".into(),
                        action: MenuAction::DeleteWorktree(w.id.clone()),
                        destructive: true,
                    });
                }
                open_menu(app, items, at);
            }
        }
        Focus::Sessions => {
            if let Some(row) = app.selected_session() {
                open_menu(app, menu_items_for_session(&row), at);
            }
        }
        Focus::Terminal => {}
    }
}

fn handle_overlay_key(app: &mut App, key: KeyEvent, out: &mut Vec<ClientRequest>) {
    if matches!(&app.overlay, Some(Overlay::Settings(_))) {
        handle_settings_key(app, key);
        return;
    }
    let Some(overlay) = &mut app.overlay else {
        return;
    };
    match overlay {
        Overlay::Settings(_) => {}
        Overlay::Help => {
            if matches!(
                key.code,
                KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('?')
            ) {
                app.overlay = None;
            }
        }
        Overlay::Menu(menu) => match key.code {
            KeyCode::Esc => app.overlay = None,
            KeyCode::Char('j') | KeyCode::Down => {
                menu.hover = (menu.hover + 1).min(menu.items.len() - 1)
            }
            KeyCode::Char('k') | KeyCode::Up => menu.hover = menu.hover.saturating_sub(1),
            KeyCode::Enter => {
                let action = menu.items[menu.hover].action.clone();
                app.overlay = None;
                run_menu_action(app, action, out);
            }
            _ => {}
        },
        Overlay::Prompt(prompt) => match key.code {
            KeyCode::Esc => app.overlay = None,
            KeyCode::Enter => {
                let prompt = prompt.clone();
                app.overlay = None;
                submit_prompt(app, prompt, out);
            }
            KeyCode::Tab if prompt.completes_paths() => {
                let home = std::env::var_os("HOME").map(std::path::PathBuf::from);
                let result = crate::completion::complete_path(&prompt.input, home.as_deref());
                if let Some(completed) = result.completed {
                    prompt.input = completed;
                }
                prompt.candidates = result.candidates;
            }
            KeyCode::Backspace => {
                prompt.input.pop();
                prompt.candidates.clear();
            }
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                prompt.input.clear();
                prompt.candidates.clear();
            }
            KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                prompt.input.push(c);
                prompt.candidates.clear();
            }
            _ => {}
        },
        Overlay::Confirm(confirm) => match key.code {
            KeyCode::Esc | KeyCode::Char('n') => app.overlay = None,
            KeyCode::Enter | KeyCode::Char('y') => {
                let action = confirm.action.clone();
                app.overlay = None;
                run_pending_action(app, action, out);
            }
            _ => {}
        },
        Overlay::Diff(view) => {
            let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
            let shift = key.modifiers.contains(KeyModifiers::SHIFT);
            let half = (view.view_height / 2).max(1) as i32;
            let page = view.view_height.max(1) as i32;
            match key.code {
                // Two-stage escape: an active filter is cleared before the
                // second Esc closes the modal.
                KeyCode::Esc if !view.filter.is_empty() => {
                    view.filter.clear();
                    if view.apply_filter() {
                        crate::git_diff::load_selected_diff(view);
                    }
                }
                KeyCode::Esc => app.overlay = None,
                KeyCode::Char('d') if ctrl => view.scroll_by(half),
                KeyCode::Char('u') if ctrl => view.scroll_by(-half),
                KeyCode::Down if shift => view.scroll_by(1),
                KeyCode::Up if shift => view.scroll_by(-1),
                KeyCode::Down => {
                    if view.select(view.selected as i64 + 1) {
                        crate::git_diff::load_selected_diff(view);
                    }
                }
                KeyCode::Up => {
                    if view.select(view.selected as i64 - 1) {
                        crate::git_diff::load_selected_diff(view);
                    }
                }
                KeyCode::PageDown => view.scroll_by(page),
                KeyCode::PageUp => view.scroll_by(-page),
                KeyCode::Home => view.scroll = 0,
                KeyCode::End => view.scroll = view.max_scroll(),
                KeyCode::Backspace => {
                    if view.filter.pop().is_some() && view.apply_filter() {
                        crate::git_diff::load_selected_diff(view);
                    }
                }
                // Everything printable feeds the always-on fuzzy filter.
                KeyCode::Char(c) if !ctrl => {
                    view.filter.push(c);
                    if view.apply_filter() {
                        crate::git_diff::load_selected_diff(view);
                    }
                }
                _ => {}
            }
        }
        Overlay::Palette(palette) => {
            let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
            match key.code {
                // Two-stage escape: an active query is cleared before the
                // second Esc closes the palette.
                KeyCode::Esc if !palette.query.is_empty() => {
                    palette.query.clear();
                    palette.apply_filter();
                }
                KeyCode::Esc => app.overlay = None,
                // j/k stay typeable in the query; Ctrl+n/p mirror ↑/↓.
                KeyCode::Down => palette.select(palette.selected as i64 + 1),
                KeyCode::Up => palette.select(palette.selected as i64 - 1),
                KeyCode::Char('n') if ctrl => palette.select(palette.selected as i64 + 1),
                KeyCode::Char('p') if ctrl => palette.select(palette.selected as i64 - 1),
                // Enter picks per the config setting; Ctrl+O always opens
                // (attach + terminal focus), Ctrl+F only focuses the row.
                KeyCode::Enter => {
                    let attach = palette.enter_attaches;
                    if let Some(target) = palette.selected_target().cloned() {
                        app.overlay = None;
                        jump_to_target(app, target, attach, out);
                    }
                }
                KeyCode::Char('o') if ctrl => {
                    if let Some(target) = palette.selected_target().cloned() {
                        app.overlay = None;
                        jump_to_target(app, target, true, out);
                    }
                }
                KeyCode::Char('f') if ctrl => {
                    if let Some(target) = palette.selected_target().cloned() {
                        app.overlay = None;
                        jump_to_target(app, target, false, out);
                    }
                }
                KeyCode::Backspace => {
                    if palette.query.pop().is_some() {
                        palette.apply_filter();
                    }
                }
                KeyCode::Char('u') if ctrl => {
                    palette.query.clear();
                    palette.apply_filter();
                }
                KeyCode::Char(c) if !ctrl => {
                    palette.query.push(c);
                    palette.apply_filter();
                }
                _ => {}
            }
        }
    }
}

fn handle_settings_key(app: &mut App, key: KeyEvent) {
    let Some(Overlay::Settings(view)) = &app.overlay else {
        return;
    };
    let last = crate::config::SETTINGS.len().saturating_sub(1);
    let cmd = match key.code {
        KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('s') => SettingsCmd::Close,
        KeyCode::Char('j') | KeyCode::Down => SettingsCmd::Move((view.selected + 1).min(last)),
        KeyCode::Char('k') | KeyCode::Up => SettingsCmd::Move(view.selected.saturating_sub(1)),
        KeyCode::Enter | KeyCode::Char(' ') => SettingsCmd::Apply(view.selected, 0),
        KeyCode::Char('l') | KeyCode::Right => SettingsCmd::Apply(view.selected, 1),
        KeyCode::Char('h') | KeyCode::Left => SettingsCmd::Apply(view.selected, -1),
        _ => return,
    };
    match cmd {
        SettingsCmd::Close => app.overlay = None,
        SettingsCmd::Move(i) => {
            if let Some(Overlay::Settings(view)) = &mut app.overlay {
                view.selected = i;
            }
        }
        SettingsCmd::Apply(i, delta) => apply_setting_at(app, i, delta),
    }
}

enum SettingsCmd {
    Close,
    Move(usize),
    Apply(usize, i32),
}

fn apply_setting_at(app: &mut App, index: usize, delta: i32) {
    let mut cfg = crate::config::Config::load();
    cfg.cycle(index, delta);
    if let Err(err) = cfg.save() {
        app.flash = Some(format!("couldn't save settings: {err}"));
        return;
    }
    app.recent_window_ms = cfg.recent_window_ms();
    app.theme = cfg.theme();
}

fn submit_prompt(app: &mut App, prompt: PromptDialog, out: &mut Vec<ClientRequest>) {
    let value = prompt.input.trim().to_string();
    // An empty divider label is meaningful: it clears the label.
    if let PromptKind::DividerLabel { id } = &prompt.kind {
        let req_id = app.alloc_req_id(PendingIntent::None);
        out.push(ClientRequest::SetProjectDivider {
            req_id,
            id: id.clone(),
            divider_after: true,
            label: (!value.is_empty()).then_some(value),
        });
        return;
    }
    // An empty agent name falls back to the next free default (agent-1, …).
    if value.is_empty() && !matches!(prompt.kind, PromptKind::NewAgent { .. }) {
        app.flash = Some("cancelled: empty input".into());
        return;
    }
    match prompt.kind {
        PromptKind::DividerLabel { .. } => unreachable!("handled above (empty input allowed)"),
        PromptKind::AddProject => {
            let expanded = shellexpand_home(&value);
            if !expanded.exists() {
                app.overlay = Some(Overlay::Confirm(ConfirmDialog {
                    title: "Create directory".into(),
                    message: format!(
                        "{} doesn't exist, would you like to create it?",
                        expanded.display()
                    ),
                    action: PendingAction::CreateProjectDir(expanded),
                }));
                return;
            }
            let req_id = app.alloc_req_id(PendingIntent::None);
            out.push(ClientRequest::AddProject {
                req_id,
                path: expanded,
                name: None,
                create_missing: false,
            });
        }
        PromptKind::NewWorktree { project } => {
            let req_id = app.alloc_req_id(PendingIntent::SelectCreatedWorktree);
            out.push(ClientRequest::CreateWorktree {
                req_id,
                project,
                branch: value,
                base: None,
            });
        }
        PromptKind::NewAgent { worktree, kind } => {
            let name = if value.is_empty() {
                app.default_session_name("agent")
            } else {
                value
            };
            let req_id = app.alloc_req_id(PendingIntent::AttachCreated);
            out.push(ClientRequest::CreateAgent {
                req_id,
                worktree,
                name,
                kind,
            });
        }
        PromptKind::RenameAgent { id } => {
            let req_id = app.alloc_req_id(PendingIntent::None);
            out.push(ClientRequest::RenameAgent {
                req_id,
                id,
                name: value,
            });
        }
    }
}

fn run_pending_action(app: &mut App, action: PendingAction, out: &mut Vec<ClientRequest>) {
    match action {
        PendingAction::CreateProjectDir(path) => {
            let req_id = app.alloc_req_id(PendingIntent::None);
            out.push(ClientRequest::AddProject {
                req_id,
                path,
                name: None,
                create_missing: true,
            });
        }
        PendingAction::ArchiveAgent(id) => {
            detach_if_attached(app, &SessionRef::Agent(id.clone()), out);
            let req_id = app.alloc_req_id(PendingIntent::None);
            out.push(ClientRequest::ArchiveAgent { req_id, id });
        }
        PendingAction::DeleteAgent(id) => {
            detach_if_attached(app, &SessionRef::Agent(id.clone()), out);
            let req_id = app.alloc_req_id(PendingIntent::None);
            out.push(ClientRequest::DeleteAgent { req_id, id });
        }
        PendingAction::DeleteWorktree(id) => {
            // Optimistic: drop the rows now (the daemon deletes in the
            // background — `git worktree remove` can take seconds). The
            // eventual EntityRemoved is a no-op; an Error for this req_id
            // restores the rows via the rollback stashed in the intent.
            let intent = match remove_worktree_rows(app, &id) {
                Some(rollback) => PendingIntent::DeleteWorktree(rollback),
                None => PendingIntent::None,
            };
            let req_id = app.alloc_req_id(intent);
            out.push(ClientRequest::DeleteWorktree {
                req_id,
                id,
                force: true,
            });
        }
        PendingAction::RemoveProject(id) => {
            let req_id = app.alloc_req_id(PendingIntent::None);
            out.push(ClientRequest::RemoveProject { req_id, id });
        }
        PendingAction::Quit => app.should_quit = true,
    }
}

fn run_menu_action(app: &mut App, action: MenuAction, out: &mut Vec<ClientRequest>) {
    match action {
        MenuAction::Attach(sref) => {
            attach(app, sref, out);
            app.focus = Focus::Terminal;
            app.term_locked = true;
        }
        MenuAction::RestartAgent(id) => {
            let req_id = app.alloc_req_id(PendingIntent::None);
            out.push(ClientRequest::RestartAgent { req_id, id });
        }
        MenuAction::RenameAgent(id) => open_prompt(app, PromptKind::RenameAgent { id }),
        MenuAction::MoveAgent(id) => open_move_agent_picker(app, id),
        MenuAction::MoveAgentToWorktree(id, worktree) => {
            // Follow the agent to its new home when the upsert lands.
            app.select_when_seen = Some(SessionRef::Agent(id.clone()));
            let req_id = app.alloc_req_id(PendingIntent::None);
            out.push(ClientRequest::MoveAgent {
                req_id,
                id,
                worktree,
            });
        }
        MenuAction::ArchiveAgent(id) => {
            if let Some(a) = app.tree.agents.iter().find(|a| a.id == id).cloned() {
                open_confirm_archive(app, &a);
            }
        }
        MenuAction::UnarchiveAgent(id) => {
            let req_id = app.alloc_req_id(PendingIntent::None);
            out.push(ClientRequest::UnarchiveAgent { req_id, id });
        }
        MenuAction::SetAgentPinned(id, pinned) => {
            app.select_when_seen = Some(SessionRef::Agent(id.clone()));
            let req_id = app.alloc_req_id(PendingIntent::None);
            out.push(ClientRequest::SetAgentPinned { req_id, id, pinned });
        }
        MenuAction::DeleteAgent(id) => {
            if let Some(a) = app.tree.agents.iter().find(|a| a.id == id).cloned() {
                app.overlay = Some(Overlay::Confirm(ConfirmDialog {
                    title: "Delete agent".into(),
                    message: format!(
                        "Delete agent '{}'? Its session and history go away.",
                        a.name
                    ),
                    action: PendingAction::DeleteAgent(id),
                }));
            }
        }
        MenuAction::NewAgent(worktree) => open_new_agent_picker(app, worktree),
        MenuAction::NewAgentOfKind(worktree, kind) => {
            // Warm the CLI while the user types the name: the daemon
            // pre-spawns the session so CreateAgent adopts an already-booted
            // PTY. Fail-soft — a missing CLI just means a cold spawn later.
            out.push(ClientRequest::PrewarmAgent {
                worktree: worktree.clone(),
                kind,
            });
            open_prompt(app, PromptKind::NewAgent { worktree, kind })
        }
        MenuAction::NewWorktree(project) => open_prompt(app, PromptKind::NewWorktree { project }),
        MenuAction::DeleteWorktree(id) => {
            if let Some(w) = app.tree.worktrees.iter().find(|w| w.id == id).cloned() {
                app.overlay = Some(Overlay::Confirm(ConfirmDialog {
                    title: "Delete worktree".into(),
                    message: format!("Delete worktree '{}' from disk?", w.branch),
                    action: PendingAction::DeleteWorktree(id),
                }));
            }
        }
        MenuAction::AddProject => open_prompt(app, PromptKind::AddProject),
        MenuAction::RemoveProject(id) => {
            if let Some(p) = app.tree.projects.iter().find(|p| p.id == id).cloned() {
                app.overlay = Some(Overlay::Confirm(ConfirmDialog {
                    title: "Remove project".into(),
                    message: format!(
                        "Remove '{}' from nebula? Nothing on disk is touched.",
                        p.name
                    ),
                    action: PendingAction::RemoveProject(id),
                }));
            }
        }
        MenuAction::SetProjectDivider(id, divider_after) => {
            let req_id = app.alloc_req_id(PendingIntent::None);
            out.push(ClientRequest::SetProjectDivider {
                req_id,
                id,
                divider_after,
                label: None,
            });
        }
        MenuAction::LabelDivider(id) => open_prompt(app, PromptKind::DividerLabel { id }),
        MenuAction::ToggleArchived => app.show_archived = !app.show_archived,
    }
}

fn detach_if_attached(app: &mut App, sref: &SessionRef, out: &mut Vec<ClientRequest>) {
    if let Some(term) = &app.term {
        if &term.sref == sref {
            out.push(ClientRequest::Detach {
                session: sref.clone(),
            });
            app.term = None;
            app.term_locked = false;
            if app.focus == Focus::Terminal {
                app.focus = Focus::Sessions;
            }
        }
    }
}

fn shellexpand_home(path: &str) -> std::path::PathBuf {
    if let Some(rest) = path.strip_prefix("~/") {
        if let Ok(home) = std::env::var("HOME") {
            return std::path::PathBuf::from(home).join(rest);
        }
    }
    std::path::PathBuf::from(path)
}

/// Ask the daemon to shift the selected row — a project, or the divider
/// itself when one is selected; the selection follows the moved row when the
/// reordered rows come back (see `apply_upsert`).
fn move_project(app: &mut App, delta: i64, out: &mut Vec<ClientRequest>) {
    match app.selected_project_row() {
        Some(ProjectRow::Project(index)) => {
            let target = index as i64 + delta;
            if target < 0 || target >= app.tree.projects.len() as i64 {
                return; // already at the edge
            }
            let id = app.tree.projects[index].id.clone();
            let req_id = app.alloc_req_id(PendingIntent::None);
            out.push(ClientRequest::MoveProject { req_id, id, delta });
        }
        Some(ProjectRow::Divider(index)) => move_divider(app, index, delta, out),
        None => {}
    }
}

/// Ask the daemon to hop the selected divider under the previous/next
/// project. Mirrors the daemon's own rules so a blocked move flashes
/// immediately instead of arming a selection-follow that never fires.
fn move_divider(app: &mut App, index: usize, delta: i64, out: &mut Vec<ClientRequest>) {
    let neighbor = index as i64 + delta.signum();
    let Some(neighbor) = usize::try_from(neighbor)
        .ok()
        .and_then(|i| app.tree.projects.get(i))
    else {
        return; // no project on that side: the divider is at an edge
    };
    if neighbor.divider_after {
        app.flash = Some("that gap already has a divider".into());
        return;
    }
    app.select_divider_when_seen = Some(neighbor.id.clone());
    let id = app.tree.projects[index].id.clone();
    let req_id = app.alloc_req_id(PendingIntent::None);
    out.push(ClientRequest::MoveDivider { req_id, id, delta });
}

fn remove_divider(app: &mut App, project_index: usize, out: &mut Vec<ClientRequest>) {
    let id = app.tree.projects[project_index].id.clone();
    let req_id = app.alloc_req_id(PendingIntent::None);
    out.push(ClientRequest::SetProjectDivider {
        req_id,
        id,
        divider_after: false,
        label: None,
    });
}

/// Snapshot the context being left — which worktree row this project was
/// on, and which session row that worktree was on — so switching back
/// restores both. Call BEFORE moving the selection away.
fn remember_context(app: &mut App) {
    let Some(wid) = app.selected_worktree().map(|w| w.id.clone()) else {
        return;
    };
    if let Some(pid) = app.selected_project().map(|p| p.id.clone()) {
        app.last_worktree_for_project.insert(pid, wid.clone());
    }
    match app.selected_session().map(|a| agent_sref(&a)) {
        Some(sref) => {
            app.last_session_for_worktree.insert(wid, sref);
        }
        None => {
            app.last_session_for_worktree.remove(&wid);
        }
    }
}

/// After a project switch: land on the project's remembered worktree (its
/// main checkout otherwise), then re-show that worktree's session.
fn restore_context(app: &mut App, out: &mut Vec<ClientRequest>) {
    app.sel_worktree = 0;
    if let Some(pid) = app.selected_project().map(|p| p.id.clone()) {
        if let Some(wid) = app.last_worktree_for_project.get(&pid).cloned() {
            if let Some(i) = app.visible_worktrees().iter().position(|w| w.id == wid) {
                app.sel_worktree = i;
            }
        }
    }
    restore_session(app, out);
}

/// After a worktree switch: select and re-attach the worktree's remembered
/// session; with nothing to restore (or it's gone/archived), blank the pane
/// rather than keep showing the previous context's session.
fn restore_session(app: &mut App, out: &mut Vec<ClientRequest>) {
    app.sel_session = 0;
    let remembered = app
        .selected_worktree()
        .and_then(|w| app.last_session_for_worktree.get(&w.id).cloned());
    let target = remembered.and_then(|sref| {
        app.visible_sessions()
            .iter()
            .position(|a| agent_sref(a) == sref && !a.archived)
            .map(|i| (i, sref))
    });
    match target {
        Some((index, sref)) => {
            app.sel_session = index;
            attach(app, sref, out);
        }
        None => {
            if let Some(term) = &app.term {
                let session = term.sref.clone();
                out.push(ClientRequest::Detach { session });
                app.term = None;
                app.term_locked = false;
            }
        }
    }
}

/// Select the worktree row for `id` within the selected project; returns
/// false when it isn't in the tree yet (its upsert hasn't arrived).
fn select_worktree_by_id(
    app: &mut App,
    id: &nebula_core::WorktreeId,
    out: &mut Vec<ClientRequest>,
) -> bool {
    let Some(index) = app.visible_worktrees().iter().position(|w| &w.id == id) else {
        return false;
    };
    if app.sel_worktree != index {
        remember_context(app);
        app.sel_worktree = index;
        restore_session(app, out);
    }
    // Land on the sessions panel so `n` immediately creates a session here.
    app.focus = Focus::Sessions;
    true
}

/// Select the Projects-panel row for project `id`, with the manual-move
/// bookkeeping (drop pending selection-follows, remember the context being
/// left). Does NOT restore the target's remembered worktree/session — the
/// caller decides. False when the project is gone from the tree.
fn select_project_row_by_id(app: &mut App, id: &nebula_core::ProjectId) -> bool {
    let rows = app.project_rows();
    let Some(row) = rows
        .iter()
        .position(|r| matches!(r, ProjectRow::Project(i) if &app.tree.projects[*i].id == id))
    else {
        return false;
    };
    app.select_divider_when_seen = None;
    app.select_worktree_when_seen = None;
    remember_context(app);
    app.sel_project = row;
    true
}

/// Land the panel selections on a `/` palette pick. A project or worktree
/// pick moves the selection (restoring remembered child rows, like a manual
/// switch). A session pick with `attach` opens it immediately, exactly like
/// Enter on its row; without, it only lands on the row in the Sessions
/// panel, previewing like ↑/↓ there. Targets are re-validated against the
/// tree — a pick can race a removal, in which case it flashes instead of
/// jumping.
fn jump_to_target(
    app: &mut App,
    target: PaletteTarget,
    attach: bool,
    out: &mut Vec<ClientRequest>,
) {
    match target {
        PaletteTarget::Project(id) => {
            let changed = app.selected_project().map(|p| p.id != id).unwrap_or(true);
            if !select_project_row_by_id(app, &id) {
                app.flash = Some("project no longer exists".into());
                return;
            }
            if changed {
                restore_context(app, out);
            }
            app.focus = Focus::Projects;
        }
        PaletteTarget::Worktree(id) => {
            if app.selected_worktree().is_some_and(|w| w.id == id) {
                app.focus = Focus::Worktrees;
                return;
            }
            let found = app
                .tree
                .worktrees
                .iter()
                .find(|w| w.id == id)
                .map(|w| w.project_id.clone())
                .is_some_and(|pid| select_project_row_by_id(app, &pid));
            let index = found
                .then(|| app.visible_worktrees().iter().position(|w| w.id == id))
                .flatten();
            let Some(index) = index else {
                app.flash = Some("worktree no longer exists".into());
                return;
            };
            app.sel_worktree = index;
            restore_session(app, out);
            app.focus = Focus::Worktrees;
        }
        PaletteTarget::Session(id) => {
            let worktree = app
                .tree
                .agents
                .iter()
                .find(|a| a.id == id)
                .map(|a| a.worktree_id.clone());
            let found = worktree.as_ref().is_some_and(|wid| {
                app.tree
                    .worktrees
                    .iter()
                    .find(|w| &w.id == wid)
                    .map(|w| w.project_id.clone())
                    .is_some_and(|pid| select_project_row_by_id(app, &pid))
            });
            let wt_index = found
                .then(|| {
                    app.visible_worktrees()
                        .iter()
                        .position(|w| Some(&w.id) == worktree.as_ref())
                })
                .flatten();
            let Some(wt_index) = wt_index else {
                app.flash = Some("session no longer exists".into());
                return;
            };
            app.sel_worktree = wt_index;
            let Some(index) = app.visible_sessions().iter().position(|a| a.id == id) else {
                // Vanished (or got archived out of view) mid-pick: land on
                // its worktree instead of attaching.
                restore_session(app, out);
                app.focus = Focus::Sessions;
                app.flash = Some("session no longer exists".into());
                return;
            };
            app.sel_session = index;
            if attach {
                attach_selected(app, out);
            } else {
                app.focus = Focus::Sessions;
                preview_selected(app, out);
            }
        }
    }
}

fn move_selection(app: &mut App, delta: i64, out: &mut Vec<ClientRequest>) {
    let len = match app.focus {
        Focus::Projects => app.project_rows().len(),
        Focus::Worktrees => app.visible_worktrees().len(),
        Focus::Sessions => app.visible_sessions().len(),
        Focus::Terminal => return,
    };
    if len == 0 {
        return;
    }
    let sel = match app.focus {
        Focus::Projects => app.sel_project,
        Focus::Worktrees => app.sel_worktree,
        Focus::Sessions => app.sel_session,
        Focus::Terminal => return,
    };
    let new = (sel as i64 + delta).clamp(0, len as i64 - 1) as usize;
    if new == sel {
        return;
    }
    // Selecting a different parent resets child selections.
    match app.focus {
        Focus::Projects => {
            // Walking onto a divider keeps its project's context, so the
            // child panels only change when the actual project does.
            // A manual move also outranks any pending selection-follows.
            app.select_divider_when_seen = None;
            app.select_worktree_when_seen = None;
            remember_context(app);
            let owner_before = app.selected_project().map(|p| p.id.clone());
            app.sel_project = new;
            if app.selected_project().map(|p| p.id.clone()) != owner_before {
                restore_context(app, out);
            }
        }
        Focus::Worktrees => {
            app.select_worktree_when_seen = None;
            remember_context(app);
            app.sel_worktree = new;
            restore_session(app, out);
        }
        Focus::Sessions => {
            app.sel_session = new;
            preview_selected(app, out);
        }
        Focus::Terminal => {}
    }
}

/// Show the selected session in the terminal pane WITHOUT taking focus or
/// the input lock — walking the list with ↑/↓ previews each session so it
/// can be read; Enter (or a click) is what commits: focus + lock. Archived
/// rows don't preview (same rule as clicking one).
fn preview_selected(app: &mut App, out: &mut Vec<ClientRequest>) {
    let Some(row) = app.selected_session() else {
        return;
    };
    if row.archived {
        return;
    }
    attach(app, agent_sref(&row), out);
}

fn attach_selected(app: &mut App, out: &mut Vec<ClientRequest>) {
    let sessions = app.visible_sessions();
    let Some(row) = sessions.get(app.sel_session) else {
        return;
    };
    attach(app, agent_sref(row), out);
    app.focus = Focus::Terminal;
    app.term_locked = true;
}

fn attach(app: &mut App, sref: SessionRef, out: &mut Vec<ClientRequest>) {
    if let Some(existing) = &app.term {
        if existing.sref == sref && !existing.exited {
            return; // already attached
        }
        out.push(ClientRequest::Detach {
            session: existing.sref.clone(),
        });
    }
    let area = app.term_area;
    let (cols, rows) = if area.width >= 2 && area.height >= 2 {
        (area.width, area.height)
    } else {
        (80, 24)
    };
    // Fresh screen, so any persisted selection would point at stale cells.
    app.term_selection = None;
    app.term = Some(AttachedTerm::new(sref.clone(), cols, rows));
    out.push(ClientRequest::Attach {
        session: sref,
        from_seq: None,
        cols,
        rows,
    });
}

/// Mouse position → pane-relative cell, clamped into the terminal area (so a
/// drag that wanders outside the pane keeps selecting the nearest edge).
fn pane_cell(area: ratatui::layout::Rect, col: u16, row: u16) -> (u16, u16) {
    let max_x = area.x + area.width.saturating_sub(1);
    let max_y = area.y + area.height.saturating_sub(1);
    (
        col.clamp(area.x, max_x) - area.x,
        row.clamp(area.y, max_y) - area.y,
    )
}

/// Text under the current selection, from the screen's visible view
/// (respects scrollback offset and wrapped rows).
fn selection_text(app: &App) -> Option<String> {
    let sel = app.term_selection.as_ref()?;
    if !sel.active {
        return None;
    }
    let screen = app.term.as_ref()?.parser.screen();
    let (rows, cols) = screen.size();
    if rows == 0 || cols == 0 {
        return None;
    }
    let ((start_col, start_row), (end_col, end_row)) = sel.bounds();
    let text = screen.contents_between(
        start_row.min(rows - 1),
        start_col.min(cols - 1),
        end_row.min(rows - 1),
        // contents_between's end column is exclusive; the selection's head
        // cell is inclusive.
        (end_col + 1).min(cols),
    );
    (!text.is_empty()).then_some(text)
}

/// Complete a drag-selection: copy the text to the system clipboard and keep
/// the highlight (it clears on the next click / scroll / keypress). A drag
/// that never left its starting cell is just a click — drop it.
fn finish_selection(app: &mut App) {
    app.dirty = true;
    let Some(sel) = &mut app.term_selection else {
        return;
    };
    if !sel.active {
        app.term_selection = None;
        return;
    }
    sel.dragging = false;
    copy_selection(app);
}

/// Copy the current selection's text to the clipboard, flashing the result.
fn copy_selection(app: &mut App) {
    if let Some(text) = selection_text(app) {
        app.flash = Some(if copy_to_clipboard(&text) {
            format!("copied {} chars", text.chars().count())
        } else {
            "copy failed (clipboard unavailable)".into()
        });
    }
}

/// Select the maximal run of non-blank cells around `cell` on its row (a
/// double-click "word": handles identifiers, paths, and URLs alike).
fn select_word_at(app: &mut App, cell: (u16, u16)) {
    let Some(term) = &app.term else {
        return;
    };
    let screen = term.parser.screen();
    let (rows, cols) = screen.size();
    let (col, row) = cell;
    if row >= rows || col >= cols {
        return;
    }
    let is_word = |c: u16| {
        screen
            .cell(row, c)
            .is_some_and(|cell| !cell.contents().trim().is_empty())
    };
    if !is_word(col) {
        return;
    }
    let mut start = col;
    while start > 0 && is_word(start - 1) {
        start -= 1;
    }
    let mut end = col;
    while end + 1 < cols && is_word(end + 1) {
        end += 1;
    }
    app.term_selection = Some(TermSelection {
        anchor: (start, row),
        head: (end, row),
        dragging: false,
        active: true,
    });
    copy_selection(app);
}

/// Copy to the system clipboard via pbcopy (this tool targets macOS).
fn copy_to_clipboard(text: &str) -> bool {
    // Unit tests exercise the selection flow; don't clobber the developer's
    // real clipboard from `cargo test`.
    if cfg!(test) {
        return true;
    }
    #[cfg(target_os = "macos")]
    {
        use std::io::Write as _;
        use std::process::{Command, Stdio};
        let Ok(mut child) = Command::new("pbcopy").stdin(Stdio::piped()).spawn() else {
            return false;
        };
        let wrote = child
            .stdin
            .take()
            .is_some_and(|mut stdin| stdin.write_all(text.as_bytes()).is_ok());
        wrote && child.wait().is_ok_and(|status| status.success())
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = text;
        false
    }
}

/// Open a URL in the default browser via open(1) (this tool targets macOS).
/// The scheme allowlist is defense in depth — the link scanner only ever
/// produces http(s) URLs, but the text originates from untrusted PTY output.
fn open_url(url: &str) -> bool {
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        return false;
    }
    if cfg!(test) {
        return true;
    }
    #[cfg(target_os = "macos")]
    {
        use std::process::{Command, Stdio};
        Command::new("open")
            .arg(url)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|status| status.success())
    }
    #[cfg(not(target_os = "macos"))]
    {
        false
    }
}

/// Two clicks on the same cell within this window make a double-click.
const DOUBLE_CLICK: Duration = Duration::from_millis(400);

fn handle_mouse(app: &mut App, mouse: MouseEvent, out: &mut Vec<ClientRequest>) {
    // An open context menu owns the mouse: click inside activates, outside
    // closes (and swallows the click).
    if let Some(Overlay::Menu(menu)) = &app.overlay {
        if let MouseEventKind::Down(_) = mouse.kind {
            let area = menu.area;
            let inside = mouse.column > area.x
                && mouse.column < area.x + area.width
                && mouse.row > area.y
                && mouse.row < area.y + area.height.saturating_sub(1);
            if inside {
                let index = (mouse.row - area.y - 1) as usize;
                if let Some(item) = menu.items.get(index) {
                    let action = item.action.clone();
                    app.overlay = None;
                    run_menu_action(app, action, out);
                }
            } else {
                app.overlay = None;
            }
            app.dirty = true;
        }
        return;
    }
    // Diff modal: the wheel scrolls the diff, a click on a file-list row
    // selects that file, a drag on the files/diff border resizes the file
    // list; everything else is swallowed.
    if let Some(Overlay::Diff(view)) = &mut app.overlay {
        match mouse.kind {
            MouseEventKind::ScrollUp => {
                view.scroll_by(-3);
                app.dirty = true;
            }
            MouseEventKind::ScrollDown => {
                view.scroll_by(3);
                app.dirty = true;
            }
            MouseEventKind::Down(MouseButton::Left) => {
                // Border grab zone: the two touching border cells at the
                // files/diff boundary (the panel `Splitter` pattern).
                let bx = view.splitter_x();
                let on_border = view.area.width > 0
                    && mouse.row >= view.area.y
                    && mouse.row < view.area.y + view.area.height
                    && mouse.column.saturating_add(1) >= bx
                    && mouse.column <= bx;
                if on_border {
                    view.files_drag = Some(bx as i32 - mouse.column as i32);
                    return;
                }
                let area = view.list_area;
                if area.width > 0
                    && mouse.column >= area.x
                    && mouse.column < area.x + area.width
                    && mouse.row >= area.y
                    && mouse.row < area.y + area.height
                {
                    let start = view.window_start(area.height as usize);
                    let index = start + (mouse.row - area.y) as usize;
                    if index < view.matches.len() && view.select(index as i64) {
                        crate::git_diff::load_selected_diff(view);
                        app.dirty = true;
                    }
                }
            }
            MouseEventKind::Drag(MouseButton::Left) => {
                if let Some(offset) = view.files_drag {
                    view.set_files_width(mouse.column as i32 + offset);
                    app.diff_files_width = view.files_width;
                    app.dirty = true;
                }
            }
            MouseEventKind::Up(MouseButton::Left) => {
                if view.files_drag.take().is_some() {
                    app.dirty = true;
                }
            }
            _ => {}
        }
        return;
    }
    // Palette: the wheel moves the selection, a click on a result row jumps
    // there, a click outside the modal closes; everything else is swallowed.
    if let Some(Overlay::Palette(palette)) = &mut app.overlay {
        match mouse.kind {
            MouseEventKind::ScrollUp => {
                palette.select(palette.selected as i64 - 1);
                app.dirty = true;
            }
            MouseEventKind::ScrollDown => {
                palette.select(palette.selected as i64 + 1);
                app.dirty = true;
            }
            MouseEventKind::Down(MouseButton::Left) => {
                let list = palette.list_area;
                let inside_list = list.width > 0
                    && mouse.column >= list.x
                    && mouse.column < list.x + list.width
                    && mouse.row >= list.y
                    && mouse.row < list.y + list.height;
                let area = palette.area;
                let inside_modal = mouse.column >= area.x
                    && mouse.column < area.x + area.width
                    && mouse.row >= area.y
                    && mouse.row < area.y + area.height;
                if inside_list {
                    let start = palette.window_start(list.height as usize);
                    let index = start + (mouse.row - list.y) as usize;
                    if index < palette.matches.len() {
                        palette.select(index as i64);
                        let attach = palette.enter_attaches;
                        if let Some(target) = palette.selected_target().cloned() {
                            app.overlay = None;
                            jump_to_target(app, target, attach, out);
                        }
                    }
                } else if !inside_modal {
                    app.overlay = None;
                }
                app.dirty = true;
            }
            _ => {}
        }
        return;
    }
    // Settings: click a row to select (or toggle if already selected),
    // click outside to close; everything else is swallowed.
    if matches!(&app.overlay, Some(Overlay::Settings(_))) {
        if let MouseEventKind::Down(MouseButton::Left) = mouse.kind {
            let (area, selected) = match &app.overlay {
                Some(Overlay::Settings(view)) => (view.area, view.selected),
                _ => return,
            };
            let inside = area.width > 0
                && mouse.column >= area.x
                && mouse.column < area.x + area.width
                && mouse.row >= area.y
                && mouse.row < area.y + area.height;
            if inside {
                let inner_y = area.y.saturating_add(1);
                if mouse.row >= inner_y {
                    let row = (mouse.row - inner_y) as usize;
                    let n = crate::config::SETTINGS.len();
                    let index = row / 2;
                    if index < n && row < n * 2 {
                        if let Some(Overlay::Settings(view)) = &mut app.overlay {
                            view.selected = index;
                        }
                        if selected == index {
                            apply_setting_at(app, index, 0);
                        }
                    }
                }
            } else {
                app.overlay = None;
            }
            app.dirty = true;
        }
        return;
    }
    // Other overlays: keyboard only; ignore mouse.
    if app.overlay.is_some() {
        return;
    }

    match mouse.kind {
        MouseEventKind::Down(MouseButton::Left) => {
            // ⌥click on a detected URL opens it in the browser; the click is
            // swallowed so it doesn't move focus or disturb the selection.
            // (Cmd never reaches us — the SGR mouse protocol has no such
            // bit — so Option is the "open link" modifier.)
            if mouse.modifiers.contains(KeyModifiers::ALT)
                && matches!(
                    app.hit_at(mouse.column, mouse.row),
                    Some(HitTarget::TerminalPane)
                )
            {
                let cell = pane_cell(app.term_area, mouse.column, mouse.row);
                if let Some(url) = app
                    .term_links
                    .iter()
                    .find(|link| link.contains(cell))
                    .map(|link| link.url.clone())
                {
                    app.flash = Some(if open_url(&url) {
                        format!("opened {url}")
                    } else {
                        format!("open failed: {url}")
                    });
                    app.dirty = true;
                    return;
                }
            }
            // Any fresh click clears a stale selection highlight; a click on
            // the terminal pane below re-arms one.
            app.term_selection = None;
            match app.hit_at(mouse.column, mouse.row) {
                Some(HitTarget::Splitter(i)) => {
                    // Arm a resize drag; focus and selections stay put.
                    app.splitter_drag = Some(SplitterDrag {
                        idx: i,
                        grab_offset: app.splitter_x(i) as i32 - mouse.column as i32,
                    });
                }
                Some(HitTarget::Project(i)) => {
                    if app.sel_project != i {
                        app.select_divider_when_seen = None;
                        app.select_worktree_when_seen = None;
                        remember_context(app);
                        let owner_before = app.selected_project().map(|p| p.id.clone());
                        app.sel_project = i;
                        if app.selected_project().map(|p| p.id.clone()) != owner_before {
                            restore_context(app, out);
                        }
                    }
                    app.focus = Focus::Projects;
                }
                Some(HitTarget::Worktree(i)) => {
                    if app.sel_worktree != i {
                        app.select_worktree_when_seen = None;
                        remember_context(app);
                        app.sel_worktree = i;
                        restore_session(app, out);
                    }
                    app.focus = Focus::Worktrees;
                }
                Some(HitTarget::Session(i)) => {
                    app.sel_session = i;
                    if app.selected_session().is_some_and(|a| a.archived) {
                        app.focus = Focus::Sessions;
                        app.flash = Some("agent is archived — unarchive first (u)".into());
                    } else if let Some(a) = app.selected_session() {
                        let now = std::time::Instant::now();
                        // Double-click toggles pin; `last_session_click` was
                        // consumed, so a third click starts over.
                        let double = app.last_session_click.take().is_some_and(|(at, id)| {
                            id == a.id && now.duration_since(at) <= DOUBLE_CLICK
                        });
                        if double {
                            // Keep the selection on this agent after it jumps
                            // between the PINNED/UNPINNED groups.
                            app.select_when_seen = Some(SessionRef::Agent(a.id.clone()));
                            let req_id = app.alloc_req_id(PendingIntent::None);
                            out.push(ClientRequest::SetAgentPinned {
                                req_id,
                                id: a.id,
                                pinned: !a.pinned,
                            });
                        } else {
                            app.last_session_click = Some((now, a.id));
                            attach_selected(app, out);
                        }
                    }
                }
                Some(HitTarget::PanelBg(focus)) => {
                    // Empty projects list: left click opens the obvious
                    // creation prompt. Other panels just take focus.
                    app.focus = focus;
                    if focus == Focus::Projects && app.tree.projects.is_empty() {
                        open_prompt(app, PromptKind::AddProject);
                    }
                }
                Some(HitTarget::TerminalPane) => {
                    // A click into the pane is deliberate — lock input too.
                    if let Some(t) = &app.term {
                        app.focus = Focus::Terminal;
                        if !t.exited {
                            app.term_locked = true;
                        }
                        let cell = pane_cell(app.term_area, mouse.column, mouse.row);
                        let now = std::time::Instant::now();
                        let double = app.last_term_click.take().is_some_and(|(at, c)| {
                            c == cell && now.duration_since(at) <= DOUBLE_CLICK
                        });
                        if double {
                            // Double-click: select (and copy) the word under
                            // the cursor. `last_term_click` was consumed, so
                            // a third click starts over.
                            select_word_at(app, cell);
                        } else {
                            app.last_term_click = Some((now, cell));
                            // Arm a drag-selection; it becomes visible (and
                            // copyable) once the drag leaves this cell.
                            app.term_selection = Some(TermSelection {
                                anchor: cell,
                                head: cell,
                                dragging: true,
                                active: false,
                            });
                        }
                    }
                }
                None => {}
            }
            app.dirty = true;
        }
        MouseEventKind::Drag(MouseButton::Left) => {
            if let Some(drag) = app.splitter_drag {
                app.set_splitter(
                    drag.idx,
                    mouse.column as i32 + drag.grab_offset,
                    app.body_area.width,
                );
                app.dirty = true;
            } else if let Some(sel) = &mut app.term_selection {
                if sel.dragging {
                    sel.head = pane_cell(app.term_area, mouse.column, mouse.row);
                    // A real drag; stays active even if it returns to the
                    // anchor cell (a 1-cell selection is still a selection).
                    if sel.head != sel.anchor {
                        sel.active = true;
                    }
                    app.dirty = true;
                }
            }
        }
        MouseEventKind::Up(MouseButton::Left) => {
            if app.splitter_drag.take().is_some() {
                app.dirty = true;
            } else if app.term_selection.is_some_and(|s| s.dragging) {
                finish_selection(app);
            }
        }
        MouseEventKind::ScrollUp | MouseEventKind::ScrollDown => {
            let up = matches!(mouse.kind, MouseEventKind::ScrollUp);
            let in_term = matches!(
                app.hit_at(mouse.column, mouse.row),
                Some(HitTarget::TerminalPane)
            ) || app.collapsed;
            if in_term {
                if let Some(term) = &mut app.term {
                    // Scrolling shifts the content under a (screen-anchored)
                    // selection highlight — drop it.
                    app.term_selection = None;
                    if term.parser.screen().alternate_screen() {
                        // Full-screen apps (vim, htop, claude) expect arrows.
                        let arrow: &[u8] = if up {
                            b"\x1b[A\x1b[A\x1b[A"
                        } else {
                            b"\x1b[B\x1b[B\x1b[B"
                        };
                        out.push(ClientRequest::Input {
                            session: term.sref.clone(),
                            data: arrow.to_vec(),
                        });
                    } else {
                        let new_scroll = if up {
                            term.scroll.saturating_add(3)
                        } else {
                            term.scroll.saturating_sub(3)
                        };
                        term.set_scroll(new_scroll);
                    }
                    app.dirty = true;
                }
            }
        }
        MouseEventKind::Down(MouseButton::Right) => {
            let at = (mouse.column, mouse.row);
            match app.hit_at(mouse.column, mouse.row) {
                Some(HitTarget::Project(i)) => {
                    app.sel_project = i;
                    app.focus = Focus::Projects;
                    if let Some(ProjectRow::Divider(pi)) = app.selected_project_row() {
                        let id = app.tree.projects[pi].id.clone();
                        open_menu_at(app, divider_row_menu(id), at);
                    } else if let Some(p) = app.selected_project() {
                        let items = vec![
                            MenuItem {
                                label: "New worktree".into(),
                                action: MenuAction::NewWorktree(p.id.clone()),
                                destructive: false,
                            },
                            MenuItem {
                                label: "Add project".into(),
                                action: MenuAction::AddProject,
                                destructive: false,
                            },
                            divider_menu_item(p),
                            MenuItem {
                                label: "Remove from list".into(),
                                action: MenuAction::RemoveProject(p.id.clone()),
                                destructive: true,
                            },
                        ];
                        open_menu_at(app, items, at);
                    }
                }
                Some(HitTarget::Worktree(i)) => {
                    app.sel_worktree = i;
                    app.sel_session = 0;
                    app.focus = Focus::Worktrees;
                    if let Some(w) = app.selected_worktree() {
                        let mut items = vec![MenuItem {
                            label: "New agent".into(),
                            action: MenuAction::NewAgent(w.id.clone()),
                            destructive: false,
                        }];
                        if !w.is_main {
                            items.push(MenuItem {
                                label: "Delete worktree".into(),
                                action: MenuAction::DeleteWorktree(w.id.clone()),
                                destructive: true,
                            });
                        }
                        open_menu_at(app, items, at);
                    }
                }
                Some(HitTarget::Session(i)) => {
                    app.sel_session = i;
                    app.focus = Focus::Sessions;
                    if let Some(row) = app.selected_session() {
                        open_menu_at(app, menu_items_for_session(&row), at);
                    }
                }
                Some(HitTarget::PanelBg(focus)) => {
                    app.focus = focus;
                    let items = match focus {
                        Focus::Projects => vec![MenuItem {
                            label: "Add project".into(),
                            action: MenuAction::AddProject,
                            destructive: false,
                        }],
                        Focus::Worktrees => app
                            .selected_project()
                            .map(|p| {
                                vec![MenuItem {
                                    label: "New worktree".into(),
                                    action: MenuAction::NewWorktree(p.id.clone()),
                                    destructive: false,
                                }]
                            })
                            .unwrap_or_default(),
                        Focus::Sessions => app
                            .selected_worktree()
                            .map(|w| {
                                vec![
                                    MenuItem {
                                        label: "New agent".into(),
                                        action: MenuAction::NewAgent(w.id.clone()),
                                        destructive: false,
                                    },
                                    MenuItem {
                                        label: "Show/hide archived".into(),
                                        action: MenuAction::ToggleArchived,
                                        destructive: false,
                                    },
                                ]
                            })
                            .unwrap_or_default(),
                        Focus::Terminal => vec![],
                    };
                    open_menu_at(app, items, at);
                }
                _ => {}
            }
            app.dirty = true;
        }
        _ => {}
    }
}

fn open_menu_at(app: &mut App, items: Vec<MenuItem>, at: (u16, u16)) {
    open_menu(app, items, at);
}

fn handle_server_event(app: &mut App, event: ServerEvent, out: &mut Vec<ClientRequest>) {
    match event {
        ServerEvent::Snapshot {
            projects,
            worktrees,
            agents,
            // Scratch terminals (`nebula raw-attach`) aren't shown in the TUI.
            terminals: _,
            ui_state,
        } => {
            app.tree.projects = projects;
            app.tree.worktrees = worktrees;
            app.tree.agents = agents;
            if let Some(json) = ui_state {
                restore_ui_state(app, &json);
            }
            clamp_selections(app);
            refresh_palette(app);
            app.dirty = true;
        }
        ServerEvent::Scrollback { session, data, .. } => {
            if let Some(term) = &mut app.term {
                if term.sref == session {
                    // Full replay: the screen is rebuilt from scratch.
                    app.term_selection = None;
                    term.reset();
                    term.parser.process(&data);
                    app.dirty = true;
                }
            }
        }
        ServerEvent::Output { session, data, .. } => {
            if let Some(term) = &mut app.term {
                if term.sref == session {
                    term.parser.process(&data);
                    app.dirty = true;
                }
            }
        }
        ServerEvent::SessionExited { session, .. } => {
            if let Some(term) = &mut app.term {
                if term.sref == session {
                    term.exited = true;
                    app.dirty = true;
                }
            }
        }
        ServerEvent::KittyFlags { session, flags } => {
            if let Some(term) = &mut app.term {
                if term.sref == session {
                    term.kitty_flags = flags;
                }
            }
        }
        ServerEvent::StatusChanged {
            agent,
            status,
            changed_at,
        } => {
            // A status flip can pull the agent into the RECENT group and
            // reorder the list; keep the selection on the same session.
            let keep = app.selected_session().map(|a| a.id.clone());
            if let Some(a) = app.tree.agents.iter_mut().find(|a| a.id == agent) {
                a.status = status;
                a.status_changed_at = changed_at;
                app.dirty = true;
            }
            if let Some(keep) = keep {
                if let Some(i) = app.visible_sessions().iter().position(|a| a.id == keep) {
                    app.sel_session = i;
                }
            }
        }
        ServerEvent::Ack { req_id, created } => {
            match (app.pending.remove(&req_id), created) {
                (Some(PendingIntent::AttachCreated), Some(id)) => {
                    let sref = match id {
                        EntityId::Agent(id) => Some(SessionRef::Agent(id)),
                        _ => None,
                    };
                    if let Some(sref) = sref {
                        app.select_when_seen = Some(sref.clone());
                        attach(app, sref, out);
                        app.focus = Focus::Terminal;
                        app.term_locked = true;
                    }
                }
                (Some(PendingIntent::SelectCreatedWorktree), Some(EntityId::Worktree(id))) => {
                    // Its upsert usually lands just before this Ack; if not,
                    // stash the id and select once it does.
                    if !select_worktree_by_id(app, &id, out) {
                        app.select_worktree_when_seen = Some(id);
                    }
                }
                _ => {}
            }
            app.dirty = true;
        }
        ServerEvent::EntityUpserted { entity } => {
            apply_upsert(app, entity);
            // Fix the selection onto a session we just created — or follow
            // one we just moved into another worktree of this project.
            if let Some(pending_sref) = app.select_when_seen.clone() {
                if let Some(index) = app
                    .visible_sessions()
                    .iter()
                    .position(|a| agent_sref(a) == pending_sref)
                {
                    app.sel_session = index;
                    app.select_when_seen = None;
                } else if let SessionRef::Agent(id) = &pending_sref {
                    let landed_worktree = app
                        .tree
                        .agents
                        .iter()
                        .find(|a| &a.id == id)
                        .map(|a| a.worktree_id.clone());
                    if let Some(wt_id) = landed_worktree {
                        if select_worktree_by_id(app, &wt_id, out) {
                            if let Some(index) = app
                                .visible_sessions()
                                .iter()
                                .position(|a| agent_sref(a) == pending_sref)
                            {
                                app.sel_session = index;
                            }
                            app.select_when_seen = None;
                        }
                    }
                }
            }
            // ...and onto a worktree we just created.
            if let Some(wt_id) = app.select_worktree_when_seen.clone() {
                if select_worktree_by_id(app, &wt_id, out) {
                    app.select_worktree_when_seen = None;
                }
            }
            // An agent upsert can shrink the visible session list (the
            // daemon re-homed it to another worktree) — keep selections in
            // bounds.
            clamp_selections(app);
            refresh_palette(app);
            app.dirty = true;
        }
        ServerEvent::EntityRemoved { id } => {
            apply_removal(app, &id);
            clamp_selections(app);
            refresh_palette(app);
            app.dirty = true;
        }
        ServerEvent::Error { req_id, message } => {
            // A failed request's intent never gets an Ack; clear it — and if
            // it was an optimistic worktree delete, put the rows back.
            if let Some(PendingIntent::DeleteWorktree(rollback)) =
                req_id.and_then(|id| app.pending.remove(&id))
            {
                restore_worktree_rows(app, rollback);
            }
            app.flash = Some(message);
            app.dirty = true;
        }
        _ => {}
    }
}

fn apply_upsert(app: &mut App, entity: nebula_core::Entity) {
    use nebula_core::Entity;
    match entity {
        Entity::Project(p) => {
            let selected = app.selected_project_row().map(|row| {
                let is_divider = matches!(row, ProjectRow::Divider(_));
                (
                    is_divider,
                    app.tree.projects[row.project_index()].id.clone(),
                )
            });
            match app.tree.projects.iter_mut().find(|x| x.id == p.id) {
                Some(existing) => *existing = p,
                None => app.tree.projects.push(p),
            }
            // Reorders arrive as plain upserts with new sort_orders; stable
            // sort keeps snapshot order for legacy all-zero ties. The
            // selection follows the row it was on, so children stay put; a
            // selected divider that just vanished falls back to its project.
            app.tree.projects.sort_by_key(|x| x.sort_order);
            if let Some((was_divider, id)) = selected {
                let rows = app.project_rows();
                let same_kind = rows.iter().position(|row| {
                    matches!(row, ProjectRow::Divider(_)) == was_divider
                        && app.tree.projects[row.project_index()].id == id
                });
                let found = same_kind.or_else(|| {
                    rows.iter()
                        .position(|row| app.tree.projects[row.project_index()].id == id)
                });
                if let Some(i) = found {
                    app.sel_project = i;
                }
            }
            // A divider we moved re-homes under another project; chase it
            // there once the destination's upsert lands.
            if let Some(target) = app.select_divider_when_seen.clone() {
                let rows = app.project_rows();
                let landed = rows.iter().position(|row| {
                    matches!(row, ProjectRow::Divider(_))
                        && app.tree.projects[row.project_index()].id == target
                });
                if let Some(i) = landed {
                    app.sel_project = i;
                    app.select_divider_when_seen = None;
                }
            }
        }
        Entity::Worktree(w) => match app.tree.worktrees.iter_mut().find(|x| x.id == w.id) {
            Some(existing) => *existing = w,
            None => app.tree.worktrees.push(w),
        },
        Entity::Agent(a) => match app.tree.agents.iter_mut().find(|x| x.id == a.id) {
            Some(existing) => *existing = a,
            None => app.tree.agents.push(a),
        },
        // Scratch terminals (`nebula raw-attach`) aren't shown in the TUI.
        Entity::Terminal(_) => {}
    }
}

fn apply_removal(app: &mut App, id: &nebula_core::EntityId) {
    use nebula_core::EntityId;
    match id {
        EntityId::Project(id) => {
            // Children cascade server-side; mirror that here.
            let wt_ids: Vec<_> = app
                .tree
                .worktrees
                .iter()
                .filter(|w| &w.project_id == id)
                .map(|w| w.id.clone())
                .collect();
            app.tree.agents.retain(|a| !wt_ids.contains(&a.worktree_id));
            app.tree.worktrees.retain(|w| &w.project_id != id);
            app.tree.projects.retain(|p| &p.id != id);
        }
        EntityId::Worktree(id) => {
            app.tree.agents.retain(|a| &a.worktree_id != id);
            app.tree.worktrees.retain(|w| &w.id != id);
        }
        EntityId::Agent(id) => app.tree.agents.retain(|a| &a.id != id),
        EntityId::Terminal(_) => {}
    }
}

/// Optimistically remove a worktree row and its agent rows, returning a
/// snapshot that `restore_worktree_rows` can reinsert if the daemon-side
/// delete fails. None when the worktree isn't in the tree.
fn remove_worktree_rows(app: &mut App, id: &WorktreeId) -> Option<WorktreeRollback> {
    let index = app.tree.worktrees.iter().position(|w| &w.id == id)?;
    let worktree = app.tree.worktrees.remove(index);
    let mut agents = Vec::new();
    let mut kept = Vec::with_capacity(app.tree.agents.len());
    for (i, a) in std::mem::take(&mut app.tree.agents).into_iter().enumerate() {
        if &a.worktree_id == id {
            agents.push((i, a));
        } else {
            kept.push(a);
        }
    }
    app.tree.agents = kept;
    clamp_selections(app);
    Some(WorktreeRollback {
        index,
        worktree,
        agents,
    })
}

/// Rollback of `remove_worktree_rows`: reinsert the rows at (or near) their
/// old positions. Skips anything the daemon re-upserted in the meantime.
fn restore_worktree_rows(app: &mut App, rollback: WorktreeRollback) {
    let WorktreeRollback {
        index,
        worktree,
        agents,
    } = rollback;
    if !app.tree.worktrees.iter().any(|w| w.id == worktree.id) {
        let at = index.min(app.tree.worktrees.len());
        app.tree.worktrees.insert(at, worktree);
    }
    for (i, a) in agents {
        if !app.tree.agents.iter().any(|x| x.id == a.id) {
            let at = i.min(app.tree.agents.len());
            app.tree.agents.insert(at, a);
        }
    }
    clamp_selections(app);
    app.dirty = true;
}

/// Keep an open `/` palette in sync with tree changes (renames, removals,
/// new entities) so its rows never go stale under the user's cursor.
fn refresh_palette(app: &mut App) {
    if let Some(Overlay::Palette(palette)) = &mut app.overlay {
        palette.rebuild(&app.tree, app.show_archived);
    }
}

/// Keep selections valid after the tree shrinks.
fn clamp_selections(app: &mut App) {
    let project_rows = app.project_rows().len();
    if app.sel_project >= project_rows {
        app.sel_project = project_rows.saturating_sub(1);
    }
    let wt_len = app.visible_worktrees().len();
    if app.sel_worktree >= wt_len {
        app.sel_worktree = wt_len.saturating_sub(1);
    }
    let sess_len = app.visible_sessions().len();
    if app.sel_session >= sess_len {
        app.sel_session = sess_len.saturating_sub(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nebula_core::{AgentId, ServerEvent, SessionRef};
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    fn hse(app: &mut App, ev: ServerEvent) {
        let mut out = Vec::new();
        handle_server_event(app, ev, &mut out);
    }

    fn buffer_text(terminal: &Terminal<TestBackend>) -> String {
        let buffer = terminal.backend().buffer();
        let mut out = String::new();
        for y in 0..buffer.area.height {
            for x in 0..buffer.area.width {
                out.push_str(buffer[(x, y)].symbol());
            }
            out.push('\n');
        }
        out
    }

    fn seed_tree(app: &mut App) {
        use nebula_core::{Agent, AgentStatus, Entity, Project, ProjectId, Worktree, WorktreeId};
        let project_id = ProjectId("p1".into());
        let worktree_id = WorktreeId("w1".into());
        hse(
            app,
            ServerEvent::EntityUpserted {
                entity: Entity::Project(Project {
                    id: project_id.clone(),
                    name: "demo".into(),
                    repo_path: "/tmp/demo".into(),
                    sort_order: 0,
                    divider_after: false,
                    divider_label: None,
                }),
            },
        );
        hse(
            app,
            ServerEvent::EntityUpserted {
                entity: Entity::Worktree(Worktree {
                    id: worktree_id.clone(),
                    project_id,
                    path: "/tmp/demo".into(),
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
                    id: AgentId("a1".into()),
                    worktree_id,
                    name: "agent-1".into(),
                    status: AgentStatus::Fresh,
                    archived: false,
                    pinned: false,
                    kind: nebula_core::AgentKind::Claude,
                    session_id: None,
                    sort_order: 0,
                    status_changed_at: 0,
                    alive: true,
                }),
            },
        );
    }

    #[test]
    fn embedded_terminal_renders_pty_output() {
        let mut app = App::new();
        seed_tree(&mut app);
        assert_eq!(app.tree.projects.len(), 1);

        let sref = SessionRef::Agent(AgentId("a1".into()));
        app.term = Some(AttachedTerm::new(sref.clone(), 40, 10));
        hse(
            &mut app,
            ServerEvent::Scrollback {
                session: sref.clone(),
                base_seq: 0,
                data: b"hello from \x1b[31mvt100\x1b[m world".to_vec(),
            },
        );
        hse(
            &mut app,
            ServerEvent::Output {
                session: sref,
                seq: 27,
                data: b"!\r\nline2".to_vec(),
            },
        );

        let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
        terminal.draw(|f| ui::draw(f, &mut app)).unwrap();
        let text = buffer_text(&terminal);
        assert!(
            text.contains("hello from vt100 world!"),
            "terminal content rendered:\n{text}"
        );
        assert!(text.contains("line2"), "second line rendered:\n{text}");
        assert!(text.contains("agent-1"), "session row rendered:\n{text}");
        assert!(
            !text.contains("PINNED"),
            "no group headers with nothing pinned:\n{text}"
        );
        assert!(
            !text.contains("TERMINALS"),
            "terminals section is gone:\n{text}"
        );
    }

    /// Pinning an agent splits the sessions panel into PINNED and UNPINNED
    /// groups; pinned rows sort first.
    #[test]
    fn pinned_agents_render_in_their_own_group() {
        use nebula_core::{Agent, AgentStatus, Entity};
        let mut app = App::new();
        seed_tree(&mut app);
        hse(
            &mut app,
            ServerEvent::EntityUpserted {
                entity: Entity::Agent(Agent {
                    id: AgentId("a2".into()),
                    worktree_id: WorktreeId("w1".into()),
                    name: "agent-2".into(),
                    status: AgentStatus::Fresh,
                    archived: false,
                    pinned: true,
                    kind: nebula_core::AgentKind::Claude,
                    session_id: None,
                    sort_order: 1,
                    status_changed_at: 0,
                    alive: true,
                }),
            },
        );

        let rows = app.visible_sessions();
        assert_eq!(rows[0].name, "agent-2", "pinned agent sorts first");
        assert_eq!(app.session_group_counts(), (1, 0, 1, 0));

        let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
        terminal.draw(|f| ui::draw(f, &mut app)).unwrap();
        let text = buffer_text(&terminal);
        assert!(text.contains("PINNED"), "pinned header rendered:\n{text}");
        assert!(
            text.contains("UNPINNED"),
            "unpinned header rendered:\n{text}"
        );
    }

    /// Agents whose status changed within the window sort into a RECENT
    /// group: below PINNED (always), above the remaining unpinned rows.
    /// Pinned agents stay in PINNED even with a fresh status change, and an
    /// expired timestamp lands back in UNPINNED.
    #[test]
    fn recent_status_changes_group_below_pinned() {
        use nebula_core::{Agent, AgentStatus, Entity};
        let mut app = App::new();
        seed_tree(&mut app);
        let now = crate::app::now_ms();
        let mk = |id: &str, pinned: bool, changed_at: i64, sort: i64| ServerEvent::EntityUpserted {
            entity: Entity::Agent(Agent {
                id: AgentId(id.into()),
                worktree_id: WorktreeId("w1".into()),
                name: id.into(),
                status: AgentStatus::Finished,
                archived: false,
                pinned,
                kind: nebula_core::AgentKind::Claude,
                session_id: None,
                sort_order: sort,
                status_changed_at: changed_at,
                alive: true,
            }),
        };
        // Pinned with a fresh change: must stay in PINNED, not RECENT.
        hse(&mut app, mk("pinned-fresh", true, now, 1));
        // Unpinned, changed just now: RECENT.
        hse(&mut app, mk("recent-1", false, now - 1_000, 2));
        // Unpinned, changed outside the window: plain UNPINNED.
        let stale = now - app.recent_window_ms - 60_000;
        hse(&mut app, mk("stale-1", false, stale, 3));

        let rows = app.visible_sessions();
        let names: Vec<&str> = rows.iter().map(|a| a.name.as_str()).collect();
        assert_eq!(
            names,
            vec!["pinned-fresh", "recent-1", "agent-1", "stale-1"],
            "pinned, then recent, then the rest"
        );
        assert_eq!(app.session_group_counts(), (1, 1, 2, 0));
        assert!(app.next_recent_expiry().is_some());

        let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
        terminal.draw(|f| ui::draw(f, &mut app)).unwrap();
        let text = buffer_text(&terminal);
        assert!(text.contains("PINNED"), "pinned header rendered:\n{text}");
        assert!(text.contains("RECENT"), "recent header rendered:\n{text}");
        assert!(
            text.contains("UNPINNED"),
            "unpinned header rendered:\n{text}"
        );
    }

    /// A StatusChanged delta stamps the agent's timestamp, pulls it into
    /// RECENT, and the selection follows the session it was on.
    #[test]
    fn status_change_regroups_and_selection_follows() {
        use nebula_core::{Agent, AgentStatus, Entity};
        let mut app = App::new();
        seed_tree(&mut app);
        hse(
            &mut app,
            ServerEvent::EntityUpserted {
                entity: Entity::Agent(Agent {
                    id: AgentId("a2".into()),
                    worktree_id: WorktreeId("w1".into()),
                    name: "agent-2".into(),
                    status: AgentStatus::Fresh,
                    archived: false,
                    pinned: false,
                    kind: nebula_core::AgentKind::Claude,
                    session_id: None,
                    sort_order: 1,
                    status_changed_at: 0,
                    alive: true,
                }),
            },
        );
        app.focus = Focus::Sessions;
        app.sel_session = 1; // agent-2

        hse(
            &mut app,
            ServerEvent::StatusChanged {
                agent: AgentId("a2".into()),
                status: AgentStatus::Finished,
                changed_at: crate::app::now_ms(),
            },
        );
        let rows = app.visible_sessions();
        assert_eq!(rows[0].name, "agent-2", "recent agent bubbled to the top");
        assert_eq!(app.session_group_counts(), (0, 1, 1, 0));
        assert_eq!(app.sel_session, 0, "selection followed agent-2");

        // recent_window "off" collapses the group back to a flat list.
        app.recent_window_ms = 0;
        assert_eq!(app.session_group_counts(), (0, 0, 2, 0));
        assert_eq!(app.visible_sessions()[0].name, "agent-1");
        assert!(app.next_recent_expiry().is_none());
    }

    /// Confirming a worktree delete drops the row (and its agents)
    /// immediately — the daemon deletes in the background — and an Error
    /// reply for that request restores them where they were.
    #[test]
    fn worktree_delete_is_optimistic_and_rolls_back_on_error() {
        use nebula_core::{Agent, AgentStatus, Entity, Worktree, WorktreeId};
        let mut app = App::new();
        seed_tree(&mut app);
        let wt_id = WorktreeId("w2".into());
        hse(
            &mut app,
            ServerEvent::EntityUpserted {
                entity: Entity::Worktree(Worktree {
                    id: wt_id.clone(),
                    project_id: nebula_core::ProjectId("p1".into()),
                    path: "/tmp/demo-feature".into(),
                    branch: "feature".into(),
                    is_main: false,
                    sort_order: 0,
                }),
            },
        );
        hse(
            &mut app,
            ServerEvent::EntityUpserted {
                entity: Entity::Agent(Agent {
                    id: AgentId("a2".into()),
                    worktree_id: wt_id.clone(),
                    name: "agent-2".into(),
                    status: AgentStatus::Fresh,
                    archived: false,
                    pinned: false,
                    kind: nebula_core::AgentKind::Claude,
                    session_id: None,
                    sort_order: 0,
                    status_changed_at: 0,
                    alive: true,
                }),
            },
        );

        // Confirmed delete: rows vanish before any daemon reply.
        let mut out = Vec::new();
        run_pending_action(
            &mut app,
            PendingAction::DeleteWorktree(wt_id.clone()),
            &mut out,
        );
        let req_id = match out.as_slice() {
            [ClientRequest::DeleteWorktree { req_id, id, .. }] if *id == wt_id => *req_id,
            other => panic!("expected DeleteWorktree request, got {other:?}"),
        };
        assert!(!app.tree.worktrees.iter().any(|w| w.id == wt_id));
        assert!(!app.tree.agents.iter().any(|a| a.worktree_id == wt_id));

        // Daemon says the delete failed: rows come back, error flashes.
        hse(
            &mut app,
            ServerEvent::Error {
                req_id: Some(req_id),
                message: "worktree dirty".into(),
            },
        );
        assert_eq!(
            app.tree.worktrees.iter().position(|w| w.id == wt_id),
            Some(1),
            "worktree restored at its old index"
        );
        assert!(app.tree.agents.iter().any(|a| a.worktree_id == wt_id));
        assert_eq!(app.flash.as_deref(), Some("worktree dirty"));
        assert!(
            app.pending.is_empty(),
            "failed request leaves no pending intent"
        );
    }

    #[test]
    fn tab_in_add_project_prompt_completes_paths() {
        use crate::app::{Overlay, PromptDialog, PromptKind};
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join("workspace/nebula")).unwrap();
        std::fs::create_dir_all(tmp.path().join("workspace/herdr")).unwrap();

        let mut app = App::new();
        let mut out = Vec::new();
        app.overlay = Some(Overlay::Prompt(PromptDialog {
            title: "Add project".into(),
            label: "path".into(),
            input: format!("{}/work", tmp.path().display()),
            kind: PromptKind::AddProject,
            candidates: vec![],
        }));

        // Unambiguous: work → workspace/
        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE),
            &mut out,
        );
        let Some(Overlay::Prompt(p)) = &app.overlay else {
            panic!("prompt closed")
        };
        assert_eq!(p.input, format!("{}/workspace/", tmp.path().display()));
        assert!(p.candidates.is_empty());

        // Ambiguous at the directory root: list both candidates.
        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE),
            &mut out,
        );
        let Some(Overlay::Prompt(p)) = &app.overlay else {
            panic!("prompt closed")
        };
        assert_eq!(p.candidates, vec!["herdr/", "nebula/"]);

        // Typing narrows; next Tab completes fully and clears the list.
        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('n'), KeyModifiers::NONE),
            &mut out,
        );
        let Some(Overlay::Prompt(p)) = &app.overlay else {
            panic!("prompt closed")
        };
        assert!(p.candidates.is_empty(), "editing clears candidates");
        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE),
            &mut out,
        );
        let Some(Overlay::Prompt(p)) = &app.overlay else {
            panic!("prompt closed")
        };
        assert_eq!(
            p.input,
            format!("{}/workspace/nebula/", tmp.path().display())
        );
    }

    #[test]
    fn tab_in_name_prompt_does_not_complete() {
        use crate::app::{Overlay, PromptDialog, PromptKind};
        let mut app = App::new();
        seed_tree(&mut app);
        let mut out = Vec::new();
        app.overlay = Some(Overlay::Prompt(PromptDialog {
            title: "Rename agent".into(),
            label: "name".into(),
            input: "src".into(), // a dir that exists in cwd — must NOT complete
            kind: PromptKind::RenameAgent {
                id: nebula_core::AgentId("a1".into()),
            },
            candidates: vec![],
        }));
        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE),
            &mut out,
        );
        let Some(Overlay::Prompt(p)) = &app.overlay else {
            panic!("prompt closed")
        };
        assert_eq!(p.input, "src", "name prompts ignore Tab");
    }

    #[test]
    fn keys_route_by_focus() {
        let mut app = App::new();
        seed_tree(&mut app);
        let mut out = Vec::new();

        // Panel focus: 'q' quits.
        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE),
            &mut out,
        );
        assert!(app.should_quit);
        app.should_quit = false;

        // Terminal input-locked: 'q' is forwarded, Ctrl+q escapes and unlocks.
        app.focus = Focus::Terminal;
        app.term_locked = true;
        let sref = SessionRef::Agent(AgentId("a1".into()));
        app.term = Some(AttachedTerm::new(sref, 80, 24));
        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE),
            &mut out,
        );
        assert!(!app.should_quit, "q must forward to pty, not quit");
        assert!(matches!(out.last(), Some(ClientRequest::Input { data, .. }) if data == b"q"));
        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('q'), KeyModifiers::CONTROL),
            &mut out,
        );
        assert_eq!(app.focus, Focus::Sessions, "Ctrl+q escapes to panels");
        assert!(!app.term_locked, "Ctrl+q clears the input lock");
    }

    #[test]
    fn n_in_sessions_opens_agent_type_picker_then_prompt() {
        let mut app = App::new();
        seed_tree(&mut app);
        app.focus = Focus::Sessions;
        let mut out = Vec::new();

        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('n'), KeyModifiers::NONE),
            &mut out,
        );
        let Some(Overlay::Menu(menu)) = &app.overlay else {
            panic!("expected agent-type picker, got {:?}", app.overlay);
        };
        assert_eq!(menu.title.as_deref(), Some("Agent type"));
        assert_eq!(menu.items.len(), 3);
        assert_eq!(menu.items[0].label, "Claude");
        assert_eq!(menu.items[1].label, "Codex");
        assert_eq!(menu.items[2].label, "Cursor");
        assert_eq!(menu.hover, 0, "Claude is the default");

        // Enter on the default chains into the name prompt with kind=Claude,
        // and fires the prewarm so the CLI boots while the user types.
        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
            &mut out,
        );
        assert!(matches!(
            out.last(),
            Some(ClientRequest::PrewarmAgent {
                kind: AgentKind::Claude,
                ..
            })
        ));
        let Some(Overlay::Prompt(p)) = &app.overlay else {
            panic!("expected name prompt, got {:?}", app.overlay);
        };
        assert_eq!(p.title, "New agent");
        assert_eq!(p.input, "", "name starts blank; the default is only a hint");
        assert_eq!(p.label, "name (empty = agent-2)");
        assert!(matches!(
            &p.kind,
            PromptKind::NewAgent {
                kind: AgentKind::Claude,
                ..
            }
        ));

        // Accepting the empty prompt falls back to the next free default name.
        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
            &mut out,
        );
        assert!(app.overlay.is_none());
        assert!(matches!(
            out.last(),
            Some(ClientRequest::CreateAgent { name, kind: AgentKind::Claude, .. }) if name == "agent-2"
        ));
    }

    #[test]
    fn picker_second_row_creates_codex_agent() {
        let mut app = App::new();
        seed_tree(&mut app);
        app.focus = Focus::Sessions;
        let mut out = Vec::new();

        for code in [KeyCode::Char('n'), KeyCode::Char('j'), KeyCode::Enter] {
            handle_key(&mut app, KeyEvent::new(code, KeyModifiers::NONE), &mut out);
        }
        assert!(matches!(
            &app.overlay,
            Some(Overlay::Prompt(p)) if matches!(&p.kind, PromptKind::NewAgent { kind: AgentKind::Codex, .. })
        ));
        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
            &mut out,
        );
        assert!(matches!(
            out.last(),
            Some(ClientRequest::CreateAgent {
                kind: AgentKind::Codex,
                ..
            })
        ));
    }

    #[test]
    fn picker_third_row_creates_cursor_agent() {
        let mut app = App::new();
        seed_tree(&mut app);
        app.focus = Focus::Sessions;
        let mut out = Vec::new();

        for code in [
            KeyCode::Char('n'),
            KeyCode::Char('j'),
            KeyCode::Char('j'),
            KeyCode::Enter,
        ] {
            handle_key(&mut app, KeyEvent::new(code, KeyModifiers::NONE), &mut out);
        }
        assert!(matches!(
            &app.overlay,
            Some(Overlay::Prompt(p)) if matches!(&p.kind, PromptKind::NewAgent { kind: AgentKind::Cursor, .. })
        ));
        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
            &mut out,
        );
        assert!(matches!(
            out.last(),
            Some(ClientRequest::CreateAgent {
                kind: AgentKind::Cursor,
                ..
            })
        ));
    }

    #[test]
    fn esc_cancels_agent_type_picker() {
        let mut app = App::new();
        seed_tree(&mut app);
        app.focus = Focus::Sessions;
        let mut out = Vec::new();

        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('n'), KeyModifiers::NONE),
            &mut out,
        );
        assert!(matches!(&app.overlay, Some(Overlay::Menu(_))));
        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
            &mut out,
        );
        assert!(app.overlay.is_none());
        assert!(
            !out.iter()
                .any(|r| matches!(r, ClientRequest::CreateAgent { .. })),
            "cancelled picker must not create anything"
        );
    }

    #[test]
    fn menu_new_agent_action_routes_through_picker() {
        use nebula_core::WorktreeId;
        let mut app = App::new();
        seed_tree(&mut app);
        let mut out = Vec::new();

        run_menu_action(
            &mut app,
            MenuAction::NewAgent(WorktreeId("w1".into())),
            &mut out,
        );
        assert!(matches!(
            &app.overlay,
            Some(Overlay::Menu(m)) if m.title.as_deref() == Some("Agent type")
        ));
    }

    #[test]
    fn escape_hatches_leave_terminal_lock() {
        let mut app = App::new();
        seed_tree(&mut app);
        let mut out = Vec::new();

        let sref = SessionRef::Agent(AgentId("a1".into()));
        app.term = Some(AttachedTerm::new(sref, 80, 24));

        // Ctrl+q plus the fallbacks: Ctrl+] in both spellings (kitty reports
        // ']', legacy 0x1D parses as Ctrl+5), Ctrl+Esc, and Ctrl+←.
        let hatches = [
            KeyCode::Char('q'),
            KeyCode::Char(']'),
            KeyCode::Char('5'),
            KeyCode::Esc,
            KeyCode::Left,
        ];
        for code in hatches {
            app.focus = Focus::Terminal;
            app.term_locked = true;
            handle_key(
                &mut app,
                KeyEvent::new(code, KeyModifiers::CONTROL),
                &mut out,
            );
            assert_eq!(
                app.focus,
                Focus::Sessions,
                "Ctrl+{code:?} leaves terminal input"
            );
            assert!(!app.term_locked, "Ctrl+{code:?} clears the input lock");
            assert!(out.is_empty(), "Ctrl+{code:?} must not reach the pty");
        }

        // Bare Esc is NOT a hatch: it forwards to the pty untouched — Claude
        // Code owns Esc (interrupt) and double-Esc (clear input / jump back).
        app.focus = Focus::Terminal;
        app.term_locked = true;
        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
            &mut out,
        );
        assert_eq!(app.focus, Focus::Terminal, "Esc stays in the terminal");
        assert!(app.term_locked, "Esc keeps the input lock");
        assert!(
            matches!(out.last(), Some(ClientRequest::Input { data, .. }) if data == b"\x1b"),
            "Esc forwards to the pty immediately"
        );
        out.clear();

        // Cmd+Left is not a hatch: it stays in the terminal (and is
        // swallowed rather than forwarded — no legacy encoding for Super).
        app.focus = Focus::Terminal;
        app.term_locked = true;
        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Left, KeyModifiers::SUPER),
            &mut out,
        );
        assert_eq!(app.focus, Focus::Terminal, "Cmd+Left does not escape");
        assert!(app.term_locked, "Cmd+Left keeps the input lock");
        assert!(out.is_empty(), "Cmd+Left has no legacy pty encoding");
    }

    #[test]
    fn focus_without_lock_navigates_instead_of_forwarding() {
        let mut app = App::new();
        seed_tree(&mut app);
        let mut out = Vec::new();

        let sref = SessionRef::Agent(AgentId("a1".into()));
        app.term = Some(AttachedTerm::new(sref, 80, 24));
        app.focus = Focus::Terminal; // focused via Tab/arrows — NOT locked

        // Arrows navigate panels instead of reaching the pty.
        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Left, KeyModifiers::NONE),
            &mut out,
        );
        assert_eq!(
            app.focus,
            Focus::Sessions,
            "unlocked pane falls through to navigation"
        );
        assert!(out.is_empty(), "no input to the pty while unlocked");

        // Enter from the sessions panel attaches AND locks.
        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
            &mut out,
        );
        assert_eq!(app.focus, Focus::Terminal);
        assert!(
            app.term_locked,
            "Enter on a session locks input into the terminal"
        );

        // Ctrl+Left back out, Ctrl+Right to refocus the pane, Enter re-locks.
        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Left, KeyModifiers::CONTROL),
            &mut out,
        );
        assert!(!app.term_locked);
        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Right, KeyModifiers::CONTROL),
            &mut out,
        );
        assert_eq!(app.focus, Focus::Terminal);
        assert!(!app.term_locked, "focusing the pane does not lock it");
        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
            &mut out,
        );
        assert!(app.term_locked, "Enter on the focused pane locks input");
    }

    /// ↑/↓ in the Sessions panel previews the selected session in the
    /// terminal pane (attach, so it can be read) but does NOT move focus or
    /// lock input — that's Enter's job. Archived rows are skipped.
    #[test]
    fn session_arrows_preview_without_focusing() {
        use nebula_core::{Agent, AgentStatus, Entity, WorktreeId};
        let mut app = App::new();
        seed_tree(&mut app);
        let agent = |id: &str, name: &str, archived: bool, sort: i64| {
            Entity::Agent(Agent {
                id: AgentId(id.into()),
                worktree_id: WorktreeId("w1".into()),
                name: name.into(),
                status: AgentStatus::Fresh,
                archived,
                pinned: false,
                kind: nebula_core::AgentKind::Claude,
                session_id: None,
                sort_order: sort,
                status_changed_at: 0,
                alive: true,
            })
        };
        hse(
            &mut app,
            ServerEvent::EntityUpserted {
                entity: agent("a2", "agent-2", false, 1),
            },
        );
        hse(
            &mut app,
            ServerEvent::EntityUpserted {
                entity: agent("a3", "agent-3", true, 2),
            },
        );
        app.show_archived = true;
        app.focus = Focus::Sessions;
        let mut out = Vec::new();

        press(&mut app, KeyCode::Down, KeyModifiers::NONE, &mut out);
        assert_eq!(app.sel_session, 1);
        assert_eq!(app.focus, Focus::Sessions, "preview must not steal focus");
        assert!(!app.term_locked, "preview must not lock input");
        let a2 = SessionRef::Agent(AgentId("a2".into()));
        assert_eq!(
            app.term.as_ref().map(|t| t.sref.clone()),
            Some(a2.clone()),
            "the walked-to session shows in the pane"
        );
        assert!(
            matches!(out.last(), Some(ClientRequest::Attach { session, .. }) if *session == a2),
            "preview attaches so scrollback streams in"
        );

        // Walking onto an archived row keeps the previous preview.
        out.clear();
        press(&mut app, KeyCode::Down, KeyModifiers::NONE, &mut out);
        assert_eq!(app.sel_session, 2);
        assert_eq!(app.term.as_ref().map(|t| t.sref.clone()), Some(a2.clone()));
        assert!(out.is_empty(), "archived rows don't attach");

        // Enter on a previewed live row commits: focus + lock, no re-attach.
        press(&mut app, KeyCode::Up, KeyModifiers::NONE, &mut out);
        out.clear();
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE, &mut out);
        assert_eq!(app.focus, Focus::Terminal);
        assert!(app.term_locked, "Enter locks input into the preview");
        assert!(
            !out.iter()
                .any(|r| matches!(r, ClientRequest::Attach { .. })),
            "already-previewed session isn't re-attached"
        );
    }

    #[test]
    fn drag_selection_selects_and_extracts_text() {
        let mut app = App::new();
        seed_tree(&mut app);
        let mut out = Vec::new();

        let sref = SessionRef::Agent(AgentId("a1".into()));
        let mut term = AttachedTerm::new(sref, 80, 24);
        term.parser.process(b"hello world");
        app.term = Some(term);
        app.term_area = ratatui::layout::Rect::new(0, 0, 80, 24);
        app.hits.push((app.term_area, HitTarget::TerminalPane));

        let ev = |kind, column, row| MouseEvent {
            kind,
            column,
            row,
            modifiers: KeyModifiers::NONE,
        };

        // Mouse-down on the pane arms an (inactive) selection and locks input.
        handle_mouse(
            &mut app,
            ev(MouseEventKind::Down(MouseButton::Left), 0, 0),
            &mut out,
        );
        assert!(app.term_selection.is_some_and(|s| s.dragging && !s.active));
        assert!(app.term_locked, "click into the pane still locks input");

        // Dragging extends the selection; the text under it is extractable.
        handle_mouse(
            &mut app,
            ev(MouseEventKind::Drag(MouseButton::Left), 10, 0),
            &mut out,
        );
        let sel = app.term_selection.expect("drag keeps the selection");
        assert!(
            sel.active,
            "leaving the anchor cell activates the selection"
        );
        assert_eq!(sel.bounds(), ((0, 0), (10, 0)));
        assert_eq!(selection_text(&app).as_deref(), Some("hello world"));

        // A drag that wanders outside the pane clamps to the nearest edge.
        handle_mouse(
            &mut app,
            ev(MouseEventKind::Drag(MouseButton::Left), 200, 50),
            &mut out,
        );
        assert_eq!(app.term_selection.expect("still selecting").head, (79, 23));

        // Mouse-up copies AND keeps the highlight (dragging over).
        handle_mouse(
            &mut app,
            ev(MouseEventKind::Up(MouseButton::Left), 200, 50),
            &mut out,
        );
        let sel = app
            .term_selection
            .expect("highlight persists after release");
        assert!(!sel.dragging && sel.active);
        assert!(
            app.flash
                .as_deref()
                .is_some_and(|f| f.starts_with("copied")),
            "release copies the selection"
        );
        assert!(
            selection_text(&app).is_some(),
            "persisted selection is still extractable"
        );

        // A fresh click outside the pane clears the highlight.
        app.hits.clear();
        handle_mouse(
            &mut app,
            ev(MouseEventKind::Down(MouseButton::Left), 0, 0),
            &mut out,
        );
        assert!(
            app.term_selection.is_none(),
            "click elsewhere clears the selection"
        );
    }

    #[test]
    fn plain_click_without_drag_leaves_no_selection() {
        let mut app = App::new();
        seed_tree(&mut app);
        let mut out = Vec::new();

        let sref = SessionRef::Agent(AgentId("a1".into()));
        let mut term = AttachedTerm::new(sref, 80, 24);
        term.parser.process(b"hello world");
        app.term = Some(term);
        app.term_area = ratatui::layout::Rect::new(0, 0, 80, 24);
        app.hits.push((app.term_area, HitTarget::TerminalPane));

        handle_mouse(
            &mut app,
            mev(MouseEventKind::Down(MouseButton::Left), 3, 0),
            &mut out,
        );
        handle_mouse(
            &mut app,
            mev(MouseEventKind::Up(MouseButton::Left), 3, 0),
            &mut out,
        );
        assert!(
            app.term_selection.is_none(),
            "a click that never dragged is not a selection"
        );
        assert!(app.flash.is_none(), "nothing was copied");
    }

    #[test]
    fn double_click_selects_word_and_persists() {
        let mut app = App::new();
        seed_tree(&mut app);
        let mut out = Vec::new();

        let sref = SessionRef::Agent(AgentId("a1".into()));
        let mut term = AttachedTerm::new(sref, 80, 24);
        term.parser.process(b"hello world");
        app.term = Some(term);
        app.term_area = ratatui::layout::Rect::new(0, 0, 80, 24);
        app.hits.push((app.term_area, HitTarget::TerminalPane));

        // Click, release, click again on the same cell (a fast double-click).
        handle_mouse(
            &mut app,
            mev(MouseEventKind::Down(MouseButton::Left), 2, 0),
            &mut out,
        );
        handle_mouse(
            &mut app,
            mev(MouseEventKind::Up(MouseButton::Left), 2, 0),
            &mut out,
        );
        handle_mouse(
            &mut app,
            mev(MouseEventKind::Down(MouseButton::Left), 2, 0),
            &mut out,
        );
        let sel = app.term_selection.expect("double-click selects the word");
        assert!(sel.active && !sel.dragging);
        assert_eq!(sel.bounds(), ((0, 0), (4, 0)));
        assert_eq!(selection_text(&app).as_deref(), Some("hello"));
        assert!(
            app.flash
                .as_deref()
                .is_some_and(|f| f.starts_with("copied")),
            "double-click copies the word"
        );

        // The release after the second click must not disturb the selection.
        handle_mouse(
            &mut app,
            mev(MouseEventKind::Up(MouseButton::Left), 2, 0),
            &mut out,
        );
        assert!(app.term_selection.is_some_and(|s| s.active));
    }

    #[test]
    fn double_click_selects_single_char_word() {
        let mut app = App::new();
        seed_tree(&mut app);
        let mut out = Vec::new();

        let sref = SessionRef::Agent(AgentId("a1".into()));
        let mut term = AttachedTerm::new(sref, 80, 24);
        term.parser.process(b"a bc");
        app.term = Some(term);
        app.term_area = ratatui::layout::Rect::new(0, 0, 80, 24);
        app.hits.push((app.term_area, HitTarget::TerminalPane));

        handle_mouse(
            &mut app,
            mev(MouseEventKind::Down(MouseButton::Left), 0, 0),
            &mut out,
        );
        handle_mouse(
            &mut app,
            mev(MouseEventKind::Up(MouseButton::Left), 0, 0),
            &mut out,
        );
        handle_mouse(
            &mut app,
            mev(MouseEventKind::Down(MouseButton::Left), 0, 0),
            &mut out,
        );
        // A one-cell word: anchor == head but the selection is real.
        let sel = app.term_selection.expect("single-char word selected");
        assert!(sel.active);
        assert_eq!(sel.bounds(), ((0, 0), (0, 0)));
        assert_eq!(selection_text(&app).as_deref(), Some("a"));
    }

    #[test]
    fn slow_second_click_arms_a_plain_drag() {
        let mut app = App::new();
        seed_tree(&mut app);
        let mut out = Vec::new();

        let sref = SessionRef::Agent(AgentId("a1".into()));
        let mut term = AttachedTerm::new(sref, 80, 24);
        term.parser.process(b"hello world");
        app.term = Some(term);
        app.term_area = ratatui::layout::Rect::new(0, 0, 80, 24);
        app.hits.push((app.term_area, HitTarget::TerminalPane));

        // A stale first click, well outside the double-click window.
        app.last_term_click = Some((
            std::time::Instant::now() - Duration::from_millis(500),
            (2, 0),
        ));
        handle_mouse(
            &mut app,
            mev(MouseEventKind::Down(MouseButton::Left), 2, 0),
            &mut out,
        );
        assert!(
            app.term_selection.is_some_and(|s| s.dragging && !s.active),
            "slow second click starts a fresh drag, not a word selection"
        );
    }

    #[test]
    fn double_click_on_session_row_toggles_pin() {
        let mut app = App::new();
        seed_tree(&mut app);
        let mut out = Vec::new();
        app.hits.push((
            ratatui::layout::Rect::new(0, 0, 20, 1),
            HitTarget::Session(0),
        ));

        handle_mouse(
            &mut app,
            mev(MouseEventKind::Down(MouseButton::Left), 1, 0),
            &mut out,
        );
        assert!(
            !out.iter()
                .any(|r| matches!(r, ClientRequest::SetAgentPinned { .. })),
            "single click attaches, never pins"
        );
        handle_mouse(
            &mut app,
            mev(MouseEventKind::Up(MouseButton::Left), 1, 0),
            &mut out,
        );
        handle_mouse(
            &mut app,
            mev(MouseEventKind::Down(MouseButton::Left), 1, 0),
            &mut out,
        );
        assert!(
            out.iter().any(|r| matches!(
                r,
                ClientRequest::SetAgentPinned { id, pinned: true, .. } if id.0 == "a1"
            )),
            "fast second click pins the agent"
        );
        assert_eq!(
            app.select_when_seen,
            Some(SessionRef::Agent(AgentId("a1".into()))),
            "selection follows the agent across the pin regroup"
        );
        assert!(
            app.last_session_click.is_none(),
            "double-click consumed the click state, a third click starts over"
        );
    }

    #[test]
    fn slow_second_click_on_session_row_does_not_pin() {
        let mut app = App::new();
        seed_tree(&mut app);
        let mut out = Vec::new();
        app.hits.push((
            ratatui::layout::Rect::new(0, 0, 20, 1),
            HitTarget::Session(0),
        ));

        // A stale first click, well outside the double-click window.
        app.last_session_click = Some((
            std::time::Instant::now() - Duration::from_millis(500),
            AgentId("a1".into()),
        ));
        handle_mouse(
            &mut app,
            mev(MouseEventKind::Down(MouseButton::Left), 1, 0),
            &mut out,
        );
        assert!(
            !out.iter()
                .any(|r| matches!(r, ClientRequest::SetAgentPinned { .. })),
            "slow second click is a plain attach, not a pin toggle"
        );
    }

    #[test]
    fn alt_click_opens_link_under_cursor() {
        let mut app = App::new();
        seed_tree(&mut app);
        let mut out = Vec::new();

        let sref = SessionRef::Agent(AgentId("a1".into()));
        let mut term = AttachedTerm::new(sref, 80, 24);
        term.parser.process(b"see https://example.com ok");
        app.term = Some(term);
        app.term_area = ratatui::layout::Rect::new(0, 0, 80, 24);
        app.hits.push((app.term_area, HitTarget::TerminalPane));
        app.term_links = crate::links::visible_links(app.term.as_ref().unwrap().parser.screen());
        assert_eq!(app.term_links.len(), 1);

        let alt = |kind, column, row| MouseEvent {
            kind,
            column,
            row,
            modifiers: KeyModifiers::ALT,
        };

        // ⌥click on the link opens it and swallows the click entirely.
        app.focus = Focus::Projects;
        handle_mouse(
            &mut app,
            alt(MouseEventKind::Down(MouseButton::Left), 6, 0),
            &mut out,
        );
        assert_eq!(
            app.flash.as_deref(),
            Some("opened https://example.com"),
            "the URL under the cursor is opened"
        );
        assert_eq!(app.focus, Focus::Projects, "focus is untouched");
        assert!(!app.term_locked, "input stays unlocked");
        assert!(app.term_selection.is_none(), "no selection armed");

        // ⌥click on a non-link cell falls through to a normal click.
        app.flash = None;
        handle_mouse(
            &mut app,
            alt(MouseEventKind::Down(MouseButton::Left), 0, 0),
            &mut out,
        );
        assert!(app.flash.is_none());
        assert_eq!(app.focus, Focus::Terminal);
        assert!(app.term_selection.is_some_and(|s| s.dragging));
    }

    /// Mirror ui::draw's splitter registration for a 120x35 body with the
    /// default panel widths (splitters at x = 20, 42, 68).
    fn seed_splitters(app: &mut App) {
        app.body_area = ratatui::layout::Rect::new(0, 0, 120, 35);
        for i in 0..3 {
            let x = app.splitter_x(i);
            app.hits.push((
                ratatui::layout::Rect::new(x - 1, 0, 2, 35),
                HitTarget::Splitter(i),
            ));
        }
    }

    fn mev(kind: MouseEventKind, column: u16, row: u16) -> MouseEvent {
        MouseEvent {
            kind,
            column,
            row,
            modifiers: KeyModifiers::NONE,
        }
    }

    #[test]
    fn splitter_drag_resizes_panel() {
        let mut app = App::new();
        seed_splitters(&mut app);
        let mut out = Vec::new();

        // Grab the projects|worktrees boundary (x = 20) and pull it right.
        handle_mouse(
            &mut app,
            mev(MouseEventKind::Down(MouseButton::Left), 20, 5),
            &mut out,
        );
        assert!(app
            .splitter_drag
            .is_some_and(|d| d.idx == 0 && d.grab_offset == 0));
        assert!(
            app.term_selection.is_none(),
            "splitter grab must not arm a terminal selection"
        );

        handle_mouse(
            &mut app,
            mev(MouseEventKind::Drag(MouseButton::Left), 30, 5),
            &mut out,
        );
        assert_eq!(app.panel_widths, [30, 22, 26]);

        handle_mouse(
            &mut app,
            mev(MouseEventKind::Up(MouseButton::Left), 30, 5),
            &mut out,
        );
        assert!(app.splitter_drag.is_none(), "mouse-up ends the drag");
    }

    #[test]
    fn splitter_drag_clamps() {
        use crate::app::{MIN_PANEL_W, MIN_TERM_W};
        let mut app = App::new();
        seed_splitters(&mut app);
        let mut out = Vec::new();

        handle_mouse(
            &mut app,
            mev(MouseEventKind::Down(MouseButton::Left), 20, 5),
            &mut out,
        );

        // Far left: floors at the panel minimum.
        handle_mouse(
            &mut app,
            mev(MouseEventKind::Drag(MouseButton::Left), 2, 5),
            &mut out,
        );
        assert_eq!(app.panel_widths[0], MIN_PANEL_W);

        // Far right: the terminal pane keeps its minimum width.
        handle_mouse(
            &mut app,
            mev(MouseEventKind::Drag(MouseButton::Left), 200, 5),
            &mut out,
        );
        let total: u16 = app.panel_widths.iter().sum();
        assert_eq!(app.body_area.width - total, MIN_TERM_W);
        assert_eq!(app.panel_widths[1..], [22, 26], "only panel 0 moved");
    }

    #[test]
    fn splitter_grab_offset_tracks_grabbed_cell() {
        let mut app = App::new();
        seed_splitters(&mut app);
        let mut out = Vec::new();

        // Grab the LEFT border cell of the boundary (x = 19, boundary at 20).
        handle_mouse(
            &mut app,
            mev(MouseEventKind::Down(MouseButton::Left), 19, 5),
            &mut out,
        );
        assert!(app.splitter_drag.is_some_and(|d| d.grab_offset == 1));

        // Dragging +5 columns grows the panel by exactly 5 — no cell jump.
        handle_mouse(
            &mut app,
            mev(MouseEventKind::Drag(MouseButton::Left), 24, 5),
            &mut out,
        );
        assert_eq!(app.panel_widths[0], 25);
    }

    #[test]
    fn splitter_down_keeps_focus_and_selection() {
        let mut app = App::new();
        seed_tree(&mut app);
        seed_splitters(&mut app);
        let mut out = Vec::new();

        app.focus = Focus::Sessions;
        handle_mouse(
            &mut app,
            mev(MouseEventKind::Down(MouseButton::Left), 42, 5),
            &mut out,
        );
        assert_eq!(app.focus, Focus::Sessions, "grab must not steal focus");
        assert_eq!(
            (app.sel_project, app.sel_worktree, app.sel_session),
            (0, 0, 0)
        );
        assert!(out.is_empty(), "no requests from a splitter grab");
    }

    #[test]
    fn normalize_panel_widths_shrinks_rightmost_first() {
        let mut app = App::new();
        app.panel_widths = [40, 40, 40];
        app.normalize_panel_widths(100);
        assert_eq!(
            app.panel_widths,
            [40, 30, 10],
            "sessions floors first, then worktrees gives way"
        );
        let total: u16 = app.panel_widths.iter().sum();
        assert_eq!(100 - total, crate::app::MIN_TERM_W);
    }

    #[test]
    fn ui_state_roundtrip_includes_panel_widths() {
        let mut app = App::new();
        app.panel_widths = [33, 44, 55];
        let json = ui_state_json(&app);

        let mut restored = App::new();
        restore_ui_state(&mut restored, &json);
        assert_eq!(restored.panel_widths, [33, 44, 55]);

        // Old blobs without the field keep the defaults.
        let mut legacy = App::new();
        restore_ui_state(
            &mut legacy,
            r#"{"project":null,"worktree":null,"session_agent":null,"show_archived":false,"collapsed":false}"#,
        );
        assert_eq!(legacy.panel_widths, crate::app::DEFAULT_PANEL_WIDTHS);
    }

    fn project(
        id: &str,
        name: &str,
        sort_order: i64,
        divider_after: bool,
        divider_label: Option<&str>,
    ) -> nebula_core::Entity {
        use nebula_core::{Entity, Project, ProjectId};
        Entity::Project(Project {
            id: ProjectId(id.into()),
            name: name.into(),
            repo_path: format!("/tmp/{name}").into(),
            sort_order,
            divider_after,
            divider_label: divider_label.map(String::from),
        })
    }

    #[test]
    fn move_agent_menu_requests_move_and_selection_follows_the_upsert() {
        use nebula_core::{Agent, AgentStatus, Entity, Worktree};
        let mut app = App::new();
        seed_tree(&mut app); // p1 / w1(main) / a1
        hse(
            &mut app,
            ServerEvent::EntityUpserted {
                entity: Entity::Worktree(Worktree {
                    id: WorktreeId("w2".into()),
                    project_id: nebula_core::ProjectId("p1".into()),
                    path: "/tmp/demo-feat".into(),
                    branch: "feat".into(),
                    is_main: false,
                    sort_order: 0,
                }),
            },
        );
        app.focus = Focus::Sessions;
        let mut out = Vec::new();

        // The picker offers only the OTHER worktree.
        open_move_agent_picker(&mut app, AgentId("a1".into()));
        let Some(Overlay::Menu(menu)) = &app.overlay else {
            panic!("picker did not open");
        };
        assert_eq!(menu.items.len(), 1);
        assert_eq!(menu.items[0].label, "feat");

        run_menu_action(
            &mut app,
            MenuAction::MoveAgentToWorktree(AgentId("a1".into()), WorktreeId("w2".into())),
            &mut out,
        );
        assert!(
            matches!(out.last(), Some(ClientRequest::MoveAgent { .. })),
            "menu action sends MoveAgent: {out:?}"
        );

        // The daemon's upsert lands with the new worktree_id — the selection
        // follows the agent into its new worktree.
        hse(
            &mut app,
            ServerEvent::EntityUpserted {
                entity: Entity::Agent(Agent {
                    id: AgentId("a1".into()),
                    worktree_id: WorktreeId("w2".into()),
                    name: "agent-1".into(),
                    status: AgentStatus::Fresh,
                    archived: false,
                    pinned: false,
                    kind: nebula_core::AgentKind::Claude,
                    session_id: None,
                    sort_order: 0,
                    status_changed_at: 0,
                    alive: true,
                }),
            },
        );
        assert_eq!(
            app.selected_worktree().map(|w| w.branch.clone()),
            Some("feat".into()),
            "worktree selection followed the moved agent"
        );
        assert_eq!(app.sel_session, 0);
        assert!(app.select_when_seen.is_none(), "follow intent consumed");
    }

    #[test]
    fn shift_arrows_reorder_projects_and_dash_toggles_divider() {
        let mut app = App::new();
        seed_tree(&mut app);
        let mut out = Vec::new();
        app.focus = Focus::Projects;

        // A single project is already at both edges — nothing to send.
        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Down, KeyModifiers::SHIFT),
            &mut out,
        );
        assert!(out.is_empty(), "edge move sends nothing");

        hse(
            &mut app,
            ServerEvent::EntityUpserted {
                entity: project("p2", "two", 1, false, None),
            },
        );
        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Down, KeyModifiers::SHIFT),
            &mut out,
        );
        assert!(
            matches!(
                out.last(),
                Some(ClientRequest::MoveProject { delta: 1, .. })
            ),
            "Shift+Down requests a move: {out:?}"
        );

        // Plain arrows still just move the selection.
        let sent = out.len();
        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Down, KeyModifiers::NONE),
            &mut out,
        );
        assert_eq!(out.len(), sent, "plain Down only moves the selection");
        assert_eq!(app.sel_project, 1);

        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('K'), KeyModifiers::SHIFT),
            &mut out,
        );
        assert!(
            matches!(
                out.last(),
                Some(ClientRequest::MoveProject { delta: -1, .. })
            ),
            "Shift+K requests a move up: {out:?}"
        );

        // '-' toggles the divider below the selected project.
        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('-'), KeyModifiers::NONE),
            &mut out,
        );
        assert!(
            matches!(
                out.last(),
                Some(ClientRequest::SetProjectDivider {
                    divider_after: true,
                    ..
                })
            ),
            "dash toggles the divider on: {out:?}"
        );
    }

    #[test]
    fn reorder_upserts_resort_projects_and_selection_follows() {
        let mut app = App::new();
        seed_tree(&mut app); // p1 "demo" at sort 0, selected
        hse(
            &mut app,
            ServerEvent::EntityUpserted {
                entity: project("p2", "two", 1, false, None),
            },
        );
        app.focus = Focus::Projects;
        assert_eq!(app.sel_project, 0);

        // The daemon swapped them; upserts arrive one by one.
        hse(
            &mut app,
            ServerEvent::EntityUpserted {
                entity: project("p1", "demo", 1, false, None),
            },
        );
        hse(
            &mut app,
            ServerEvent::EntityUpserted {
                entity: project("p2", "two", 0, false, None),
            },
        );

        let order: Vec<&str> = app.tree.projects.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(order, ["two", "demo"], "projects re-sort by sort_order");
        assert_eq!(
            app.sel_project, 1,
            "selection follows the project it was on"
        );
    }

    #[test]
    fn divider_moves_with_shift_and_selection_follows() {
        let mut app = App::new();
        seed_tree(&mut app); // p1 "demo" at sort 0, selected
        let mut out = Vec::new();
        app.focus = Focus::Projects;
        hse(
            &mut app,
            ServerEvent::EntityUpserted {
                entity: project("p1", "demo", 0, true, Some("work")),
            },
        );
        hse(
            &mut app,
            ServerEvent::EntityUpserted {
                entity: project("p2", "two", 1, false, None),
            },
        );

        // j walks onto the divider under p1; Shift+J asks the daemon to hop
        // it under p2.
        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE),
            &mut out,
        );
        assert_eq!(app.selected_project_row(), Some(ProjectRow::Divider(0)));
        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('J'), KeyModifiers::SHIFT),
            &mut out,
        );
        assert!(
            matches!(
                out.last(),
                Some(ClientRequest::MoveDivider { delta: 1, .. })
            ),
            "Shift+J on a divider requests a divider move: {out:?}"
        );

        // The daemon answers with both upserts; the selection chases the
        // divider to its new home under p2, label and all.
        hse(
            &mut app,
            ServerEvent::EntityUpserted {
                entity: project("p1", "demo", 0, false, None),
            },
        );
        hse(
            &mut app,
            ServerEvent::EntityUpserted {
                entity: project("p2", "two", 1, true, Some("work")),
            },
        );
        assert_eq!(app.selected_project_row(), Some(ProjectRow::Divider(1)));
        assert_eq!(app.selected_project().unwrap().name, "two");

        // Under the last project it sits at the bottom edge: nothing to send.
        let sent = out.len();
        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('J'), KeyModifiers::SHIFT),
            &mut out,
        );
        assert_eq!(out.len(), sent, "edge divider move sends nothing");

        // A divider in the neighboring gap blocks the hop with a flash.
        hse(
            &mut app,
            ServerEvent::EntityUpserted {
                entity: project("p1", "demo", 0, true, None),
            },
        );
        app.flash = None;
        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('K'), KeyModifiers::SHIFT),
            &mut out,
        );
        assert_eq!(out.len(), sent, "blocked divider move sends nothing");
        assert!(app.flash.is_some(), "blocked divider move explains itself");
    }

    #[test]
    fn created_worktree_gets_selected() {
        use nebula_core::{Entity, Worktree, WorktreeId};
        let mut app = App::new();
        seed_tree(&mut app); // p1/w1(main) + agent-1
        let mut out = Vec::new();
        app.focus = Focus::Worktrees;

        // n opens the branch prompt; submitting requests the worktree.
        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('n'), KeyModifiers::NONE),
            &mut out,
        );
        for c in "feat".chars() {
            handle_key(
                &mut app,
                KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE),
                &mut out,
            );
        }
        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
            &mut out,
        );
        let Some(ClientRequest::CreateWorktree { req_id, .. }) = out.last() else {
            panic!("prompt submit requests a worktree: {out:?}");
        };
        let req_id = *req_id;

        // The daemon broadcasts the upsert, then acks — selection lands on
        // the new worktree, children reset, sessions panel focused so `n`
        // creates a session right away.
        let w2 = Worktree {
            id: WorktreeId("w2".into()),
            project_id: nebula_core::ProjectId("p1".into()),
            path: "/tmp/demo-worktrees/feat".into(),
            branch: "feat".into(),
            is_main: false,
            sort_order: 0,
        };
        hse(
            &mut app,
            ServerEvent::EntityUpserted {
                entity: Entity::Worktree(w2.clone()),
            },
        );
        hse(
            &mut app,
            ServerEvent::Ack {
                req_id,
                created: Some(EntityId::Worktree(w2.id.clone())),
            },
        );
        assert_eq!(app.focus, Focus::Sessions);
        assert_eq!(app.selected_worktree().map(|w| w.id.clone()), Some(w2.id));
        assert_eq!(app.sel_session, 0);
    }

    #[test]
    fn switching_contexts_restores_the_remembered_session() {
        use nebula_core::{Entity, Worktree, WorktreeId};
        let mut app = App::new();
        seed_tree(&mut app); // p1/w1(main) + agent-1
        let sref = SessionRef::Agent(AgentId("a1".into()));
        app.term = Some(AttachedTerm::new(sref.clone(), 40, 10));
        let mut out = Vec::new();

        // Moving within the session's own context keeps the pane: the
        // worktree list clamps at its single row.
        app.focus = Focus::Worktrees;
        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE),
            &mut out,
        );
        assert!(app.term.is_some(), "clamped move keeps the pane");

        // Walking onto a sibling worktree with no history blanks the pane.
        hse(
            &mut app,
            ServerEvent::EntityUpserted {
                entity: Entity::Worktree(Worktree {
                    id: WorktreeId("w2".into()),
                    project_id: nebula_core::ProjectId("p1".into()),
                    path: "/tmp/demo-worktrees/other".into(),
                    branch: "other".into(),
                    is_main: false,
                    sort_order: 1,
                }),
            },
        );
        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE),
            &mut out,
        );
        assert!(
            matches!(out.last(), Some(ClientRequest::Detach { session }) if *session == sref),
            "leaving the worktree detaches: {out:?}"
        );
        assert!(app.term.is_none(), "no history on w2 — pane blanks");

        // Walking back restores the remembered session, re-attached.
        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('k'), KeyModifiers::NONE),
            &mut out,
        );
        assert!(
            matches!(out.last(), Some(ClientRequest::Attach { session, .. }) if *session == sref),
            "returning to w1 re-attaches its session: {out:?}"
        );
        assert_eq!(
            app.term.as_ref().map(|t| t.sref.clone()),
            Some(sref.clone())
        );
        assert_eq!(app.sel_session, 0);

        // Project switches remember the whole context: leaving p1 blanks
        // (p2 has no history), returning restores worktree AND session.
        hse(
            &mut app,
            ServerEvent::EntityUpserted {
                entity: project("p2", "two", 1, false, None),
            },
        );
        app.focus = Focus::Projects;
        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE),
            &mut out,
        );
        assert_eq!(
            app.selected_project().map(|p| p.name.clone()),
            Some("two".into())
        );
        assert!(app.term.is_none(), "no history on p2 — pane blanks");

        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('k'), KeyModifiers::NONE),
            &mut out,
        );
        assert_eq!(
            app.selected_project().map(|p| p.name.clone()),
            Some("demo".into())
        );
        assert_eq!(app.sel_worktree, 0, "p1 remembered its worktree row");
        assert_eq!(
            app.term.as_ref().map(|t| t.sref.clone()),
            Some(sref),
            "returning to p1 re-shows its session"
        );
    }

    #[test]
    fn divider_renders_under_project_row() {
        let mut app = App::new();
        seed_tree(&mut app);
        hse(
            &mut app,
            ServerEvent::EntityUpserted {
                entity: project("p1", "demo", 0, true, None),
            },
        );

        let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
        terminal.draw(|f| ui::draw(f, &mut app)).unwrap();
        let text = buffer_text(&terminal);
        // Projects panel is 20 wide: row 1 is the project, row 2 the divider
        // spanning the 18 inner columns between the borders (thick ┃ — the
        // Projects panel starts focused).
        let lines: Vec<&str> = text.lines().collect();
        assert!(
            lines[1].starts_with("┃● demo"),
            "project row first:\n{text}"
        );
        assert!(
            lines[2].starts_with(&format!("┃{}┃", "─".repeat(18))),
            "divider row under the project:\n{text}"
        );

        // A labeled divider weaves the label into the line.
        hse(
            &mut app,
            ServerEvent::EntityUpserted {
                entity: project("p1", "demo", 0, true, Some("work")),
            },
        );
        terminal.draw(|f| ui::draw(f, &mut app)).unwrap();
        let text = buffer_text(&terminal);
        assert!(
            text.lines().nth(2).unwrap().starts_with("┃─ work ──"),
            "labeled divider row:\n{text}"
        );
    }

    #[test]
    fn divider_rows_select_label_and_delete() {
        let mut app = App::new();
        seed_tree(&mut app);
        let mut out = Vec::new();
        app.focus = Focus::Projects;
        hse(
            &mut app,
            ServerEvent::EntityUpserted {
                entity: project("p1", "demo", 0, true, None),
            },
        );

        // j walks onto the divider; the project's context sticks.
        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE),
            &mut out,
        );
        assert_eq!(app.selected_project_row(), Some(ProjectRow::Divider(0)));
        assert_eq!(app.selected_project().unwrap().name, "demo");
        assert!(
            !app.visible_worktrees().is_empty(),
            "divider keeps its project's context"
        );

        // Enter opens the label prompt; submitting sends the label.
        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
            &mut out,
        );
        assert!(
            matches!(&app.overlay, Some(Overlay::Prompt(p)) if p.title == "Divider label"),
            "Enter on a divider prompts for its label"
        );
        for c in "work".chars() {
            handle_key(
                &mut app,
                KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE),
                &mut out,
            );
        }
        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
            &mut out,
        );
        assert!(
            matches!(
                out.last(),
                Some(ClientRequest::SetProjectDivider { divider_after: true, label: Some(l), .. })
                    if l == "work"
            ),
            "label submit: {out:?}"
        );

        // With no project below, the divider is at the bottom edge — the
        // Shift move has nowhere to go and sends nothing.
        let sent = out.len();
        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('J'), KeyModifiers::SHIFT),
            &mut out,
        );
        assert_eq!(out.len(), sent, "edge divider move sends nothing");

        // d deletes the divider without a confirm dialog.
        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE),
            &mut out,
        );
        assert!(app.overlay.is_none(), "divider delete needs no confirm");
        assert!(
            matches!(
                out.last(),
                Some(ClientRequest::SetProjectDivider {
                    divider_after: false,
                    label: None,
                    ..
                })
            ),
            "divider delete: {out:?}"
        );
    }

    #[test]
    fn backspace_opens_delete_confirm_per_panel() {
        let mut app = App::new();
        seed_tree(&mut app);
        let mut out = Vec::new();

        app.focus = Focus::Projects;
        press(&mut app, KeyCode::Backspace, KeyModifiers::NONE, &mut out);
        assert!(
            matches!(
                &app.overlay,
                Some(Overlay::Confirm(c)) if matches!(c.action, PendingAction::RemoveProject(_))
            ),
            "backspace on a project confirms removal: {:?}",
            app.overlay
        );
        press(&mut app, KeyCode::Esc, KeyModifiers::NONE, &mut out);

        // The seeded worktree is the main checkout — deletion is refused.
        app.focus = Focus::Worktrees;
        press(&mut app, KeyCode::Backspace, KeyModifiers::NONE, &mut out);
        assert!(app.overlay.is_none(), "main checkout never gets a confirm");
        assert!(app.flash.is_some(), "main checkout delete flashes instead");

        app.focus = Focus::Sessions;
        press(&mut app, KeyCode::Backspace, KeyModifiers::NONE, &mut out);
        assert!(
            matches!(
                &app.overlay,
                Some(Overlay::Confirm(c)) if matches!(c.action, PendingAction::DeleteAgent(_))
            ),
            "backspace on a session confirms agent delete: {:?}",
            app.overlay
        );
    }

    #[test]
    fn exited_session_does_not_trap_keys() {
        let mut app = App::new();
        seed_tree(&mut app);
        let mut out = Vec::new();

        let sref = SessionRef::Agent(AgentId("a1".into()));
        app.term = Some(AttachedTerm::new(sref, 80, 24));
        app.term.as_mut().unwrap().exited = true;
        app.focus = Focus::Terminal;
        app.term_locked = true;
        app.collapsed = true;

        // No input reaches a dead PTY.
        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE),
            &mut out,
        );
        assert!(out.is_empty(), "no input to a dead pty");

        // Esc leaves the pane and expands collapsed sidebars.
        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
            &mut out,
        );
        assert_eq!(app.focus, Focus::Sessions, "Esc leaves an exited pane");
        assert!(!app.collapsed, "escape expands sidebars");

        // Navigation keys fall through instead of being swallowed.
        app.focus = Focus::Terminal;
        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Left, KeyModifiers::NONE),
            &mut out,
        );
        assert_eq!(
            app.focus,
            Focus::Sessions,
            "arrow navigation works from an exited pane"
        );
    }

    // ---- git-diff modal ----

    fn press(app: &mut App, code: KeyCode, mods: KeyModifiers, out: &mut Vec<ClientRequest>) {
        handle_key(app, KeyEvent::new(code, mods), out);
    }

    fn run_git(repo: &std::path::Path, args: &[&str]) {
        let out = std::process::Command::new("git")
            .arg("-C")
            .arg(repo)
            .args(args)
            .output()
            .expect("run git");
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    /// `git init` + one commit containing a.txt.
    fn test_repo(dir: &tempfile::TempDir) -> std::path::PathBuf {
        let repo = dir.path().join("repo");
        std::fs::create_dir(&repo).unwrap();
        run_git(&repo, &["init", "-b", "main"]);
        run_git(&repo, &["config", "user.email", "t@t"]);
        run_git(&repo, &["config", "user.name", "t"]);
        std::fs::write(repo.join("a.txt"), "orig\n").unwrap();
        run_git(&repo, &["add", "."]);
        run_git(&repo, &["commit", "-m", "init"]);
        repo
    }

    /// Like `seed_tree`, but the worktree points at a real checkout.
    fn seed_repo_tree(app: &mut App, path: &std::path::Path) {
        use nebula_core::{Entity, Project, ProjectId, Worktree, WorktreeId};
        hse(
            app,
            ServerEvent::EntityUpserted {
                entity: Entity::Project(Project {
                    id: ProjectId("p1".into()),
                    name: "demo".into(),
                    repo_path: path.to_path_buf(),
                    sort_order: 0,
                    divider_after: false,
                    divider_label: None,
                }),
            },
        );
        hse(
            app,
            ServerEvent::EntityUpserted {
                entity: Entity::Worktree(Worktree {
                    id: WorktreeId("w1".into()),
                    project_id: ProjectId("p1".into()),
                    path: path.to_path_buf(),
                    branch: "main".into(),
                    is_main: true,
                    sort_order: 0,
                }),
            },
        );
    }

    /// Hand-built modal state — no git involved.
    fn fake_diff_view(lines: usize) -> crate::app::DiffView {
        use crate::git_diff::DiffFile;
        let mut view = DiffView::new(
            "/nonexistent-nebula-diff-test".into(),
            "main".into(),
            vec![
                DiffFile {
                    path: "alpha.rs".into(),
                    orig_path: None,
                    xy: ['M', ' '],
                },
                DiffFile {
                    path: "beta.rs".into(),
                    orig_path: None,
                    xy: ['?', '?'],
                },
            ],
            true,
        );
        view.diff = (0..lines)
            .map(|i| format!("line {i}"))
            .collect::<Vec<_>>()
            .join("\n");
        view.diff_line_count = lines;
        view.view_height = 20;
        view
    }

    #[test]
    fn g_opens_diff_modal_and_esc_closes() {
        let dir = tempfile::tempdir().unwrap();
        let repo = test_repo(&dir);
        std::fs::write(repo.join("a.txt"), "changed\n").unwrap();
        std::fs::write(repo.join("z.txt"), "fresh\n").unwrap();

        let mut app = App::new();
        seed_repo_tree(&mut app, &repo);
        let mut out = Vec::new();
        press(&mut app, KeyCode::Char('g'), KeyModifiers::NONE, &mut out);
        match &app.overlay {
            Some(Overlay::Diff(v)) => {
                assert_eq!(v.files.len(), 2, "{:?}", v.files);
                assert_eq!(v.branch, "main");
                assert!(v.head_ok);
                // Status is path-ordered, so a.txt is selected first.
                assert!(v.diff.contains("-orig"), "{}", v.diff);
                assert!(v.diff.contains("+changed"), "{}", v.diff);
            }
            other => panic!("expected diff overlay, got {other:?}"),
        }
        press(&mut app, KeyCode::Esc, KeyModifiers::NONE, &mut out);
        assert!(app.overlay.is_none(), "Esc closes the modal");
        assert!(out.is_empty(), "the diff modal never talks to the daemon");
    }

    #[test]
    fn g_with_clean_repo_flashes_no_changes() {
        let dir = tempfile::tempdir().unwrap();
        let repo = test_repo(&dir);
        let mut app = App::new();
        seed_repo_tree(&mut app, &repo);
        let mut out = Vec::new();
        press(&mut app, KeyCode::Char('g'), KeyModifiers::NONE, &mut out);
        assert!(app.overlay.is_none(), "clean tree opens no modal");
        assert!(
            app.flash
                .as_deref()
                .unwrap_or("")
                .contains("no changes in main"),
            "{:?}",
            app.flash
        );
    }

    #[test]
    fn g_with_missing_path_flashes() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = App::new();
        seed_repo_tree(&mut app, &dir.path().join("nope"));
        let mut out = Vec::new();
        press(&mut app, KeyCode::Char('g'), KeyModifiers::NONE, &mut out);
        assert!(app.overlay.is_none());
        assert!(
            app.flash.as_deref().unwrap_or("").contains("missing"),
            "{:?}",
            app.flash
        );
    }

    #[test]
    fn diff_modal_keys_switch_files_and_scroll() {
        let mut app = App::new();
        seed_tree(&mut app);
        app.overlay = Some(Overlay::Diff(fake_diff_view(100)));
        let mut out = Vec::new();
        let scroll = |app: &App| match &app.overlay {
            Some(Overlay::Diff(v)) => (v.selected, v.scroll),
            _ => panic!("diff overlay gone"),
        };

        press(&mut app, KeyCode::Down, KeyModifiers::SHIFT, &mut out);
        assert_eq!(scroll(&app), (0, 1), "Shift+Down scrolls down one line");
        press(&mut app, KeyCode::Up, KeyModifiers::SHIFT, &mut out);
        press(&mut app, KeyCode::Up, KeyModifiers::SHIFT, &mut out);
        assert_eq!(scroll(&app), (0, 0), "Shift+Up clamps at the top");
        press(
            &mut app,
            KeyCode::Char('d'),
            KeyModifiers::CONTROL,
            &mut out,
        );
        assert_eq!(scroll(&app), (0, 10), "Ctrl+d scrolls half a page");
        press(&mut app, KeyCode::End, KeyModifiers::NONE, &mut out);
        assert_eq!(scroll(&app), (0, 80), "End jumps to max scroll");
        press(&mut app, KeyCode::PageDown, KeyModifiers::NONE, &mut out);
        assert_eq!(scroll(&app), (0, 80), "paging clamps at the bottom");
        press(&mut app, KeyCode::Home, KeyModifiers::NONE, &mut out);
        assert_eq!(scroll(&app), (0, 0), "Home jumps back to the top");

        // File switch resets the scroll; the fake root makes the reload an
        // error body, which must not panic.
        press(&mut app, KeyCode::End, KeyModifiers::NONE, &mut out);
        press(&mut app, KeyCode::Down, KeyModifiers::NONE, &mut out);
        assert_eq!(scroll(&app).0, 1, "Down selects the next file");
        assert_eq!(scroll(&app).1, 0, "file switch resets scroll");
        press(&mut app, KeyCode::Down, KeyModifiers::NONE, &mut out);
        assert_eq!(scroll(&app).0, 1, "selection clamps at the last file");
        press(&mut app, KeyCode::Up, KeyModifiers::NONE, &mut out);
        assert_eq!(scroll(&app).0, 0, "Up selects the previous file");

        press(&mut app, KeyCode::Esc, KeyModifiers::NONE, &mut out);
        assert!(app.overlay.is_none(), "Esc closes the modal");
        assert!(out.is_empty());
    }

    #[test]
    fn diff_modal_type_to_filter() {
        let mut app = App::new();
        seed_tree(&mut app);
        app.overlay = Some(Overlay::Diff(fake_diff_view(10)));
        let mut out = Vec::new();
        let view = |app: &App| match &app.overlay {
            Some(Overlay::Diff(v)) => v.clone(),
            _ => panic!("diff overlay gone"),
        };

        // Typing narrows to the fuzzy matches; the diff reload against the
        // fake root yields an error body, which must not panic.
        press(&mut app, KeyCode::Char('b'), KeyModifiers::NONE, &mut out);
        let v = view(&app);
        assert_eq!(v.filter, "b");
        assert_eq!(v.matches.len(), 1, "only beta.rs matches");
        assert_eq!(v.selected_file().unwrap().path, "beta.rs");

        // Uppercase (SHIFT-modified) chars land in the filter too, and the
        // match is case-insensitive.
        press(&mut app, KeyCode::Char('T'), KeyModifiers::SHIFT, &mut out);
        let v = view(&app);
        assert_eq!(v.filter, "bT");
        assert_eq!(v.matches.len(), 1, "bT still fuzzy-matches beta.rs");

        // A dead-end query empties the list without panicking.
        press(&mut app, KeyCode::Char('z'), KeyModifiers::NONE, &mut out);
        let v = view(&app);
        assert!(v.matches.is_empty(), "no file matches bTz");
        assert!(v.selected_file().is_none());
        assert_eq!(v.diff, "", "no selection clears the diff pane");

        // Backspace restores the previous narrowing.
        press(&mut app, KeyCode::Backspace, KeyModifiers::NONE, &mut out);
        assert_eq!(view(&app).matches.len(), 1);

        // First Esc clears the filter, second closes.
        press(&mut app, KeyCode::Esc, KeyModifiers::NONE, &mut out);
        let v = view(&app);
        assert_eq!(v.filter, "", "Esc clears the filter first");
        assert_eq!(v.matches.len(), 2, "full list restored in git order");
        assert_eq!(v.selected_file().unwrap().path, "alpha.rs");
        press(&mut app, KeyCode::Esc, KeyModifiers::NONE, &mut out);
        assert!(app.overlay.is_none(), "second Esc closes the modal");
        assert!(out.is_empty(), "filtering never talks to the daemon");
    }

    #[test]
    fn diff_filter_sorts_best_match_first() {
        use crate::git_diff::DiffFile;
        let file = |path: &str| DiffFile {
            path: path.into(),
            orig_path: None,
            xy: ['M', ' '],
        };
        let mut view = DiffView::new(
            "/nonexistent-nebula-diff-test".into(),
            "main".into(),
            vec![file("build.rs"), file("src/ui.rs")],
            true,
        );
        view.filter = "ui".into();
        view.apply_filter();
        assert_eq!(view.matches.len(), 2);
        // Segment-start match on src/ui.rs outranks the mid-word one in
        // build.rs despite git order listing build.rs first.
        assert_eq!(view.selected_file().unwrap().path, "src/ui.rs");
    }

    #[test]
    fn diff_modal_renders_two_panes() {
        let mut app = App::new();
        seed_tree(&mut app);
        let mut view = fake_diff_view(4);
        view.diff = "diff --git a/a.rs b/a.rs\n@@ -1,2 +1,2 @@\n-old line\n+new line".into();
        view.diff_line_count = 4;
        app.overlay = Some(Overlay::Diff(view));

        let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
        terminal.draw(|f| ui::draw(f, &mut app)).unwrap();
        let text = buffer_text(&terminal);
        assert!(text.contains("Files (2)"), "file pane title:\n{text}");
        assert!(text.contains("alpha.rs"), "file row:\n{text}");
        assert!(text.contains("type to filter"), "filter row:\n{text}");
        assert!(text.contains("+new line"), "diff body:\n{text}");
        assert!(text.contains("type: filter"), "footer hint:\n{text}");
        match &app.overlay {
            Some(Overlay::Diff(v)) => {
                assert!(v.view_height > 0, "view_height written back during draw")
            }
            _ => panic!("diff overlay gone"),
        }
    }

    #[test]
    fn diff_modal_swallows_mouse_and_wheel_scrolls() {
        let mut app = App::new();
        seed_tree(&mut app);
        app.focus = Focus::Projects;
        app.overlay = Some(Overlay::Diff(fake_diff_view(100)));
        let mut out = Vec::new();

        let wheel = MouseEvent {
            kind: MouseEventKind::ScrollDown,
            column: 50,
            row: 10,
            modifiers: KeyModifiers::NONE,
        };
        handle_mouse(&mut app, wheel, &mut out);
        match &app.overlay {
            Some(Overlay::Diff(v)) => assert_eq!(v.scroll, 3, "wheel scrolls the diff"),
            _ => panic!("diff overlay gone"),
        }

        let (focus_before, sel_before) = (app.focus, app.sel_project);
        let click = MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 2,
            row: 1,
            modifiers: KeyModifiers::NONE,
        };
        handle_mouse(&mut app, click, &mut out);
        assert!(
            matches!(app.overlay, Some(Overlay::Diff(_))),
            "clicks do not close the modal"
        );
        assert_eq!(app.focus, focus_before, "clicks do not change focus");
        assert_eq!(app.sel_project, sel_before);
        assert!(out.is_empty(), "mouse in the modal sends nothing");
    }

    #[test]
    fn diff_modal_click_selects_file_row() {
        let mut app = App::new();
        seed_tree(&mut app);
        app.overlay = Some(Overlay::Diff(fake_diff_view(4)));
        let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
        terminal.draw(|f| ui::draw(f, &mut app)).unwrap();
        let area = match &app.overlay {
            Some(Overlay::Diff(v)) => v.list_area,
            _ => panic!("diff overlay gone"),
        };
        assert!(
            area.height >= 2,
            "list area written back during draw: {area:?}"
        );

        let mut out = Vec::new();
        // Click the second row: beta.rs becomes the selection and its diff
        // loads (the fake root makes that an error string, still a reload).
        handle_mouse(
            &mut app,
            mev(
                MouseEventKind::Down(MouseButton::Left),
                area.x + 2,
                area.y + 1,
            ),
            &mut out,
        );
        match &app.overlay {
            Some(Overlay::Diff(v)) => {
                assert_eq!(v.selected, 1);
                assert_eq!(v.selected_file().unwrap().path, "beta.rs");
                assert_eq!(v.scroll, 0, "reload resets the scroll");
            }
            _ => panic!("diff overlay gone"),
        }

        // A click below the last populated row is a no-op.
        handle_mouse(
            &mut app,
            mev(
                MouseEventKind::Down(MouseButton::Left),
                area.x + 2,
                area.y + area.height - 1,
            ),
            &mut out,
        );
        match &app.overlay {
            Some(Overlay::Diff(v)) => assert_eq!(v.selected, 1, "empty-row click ignored"),
            _ => panic!("diff overlay gone"),
        }
        assert!(out.is_empty(), "clicks in the modal send nothing");
    }

    #[test]
    fn diff_modal_border_drag_resizes_file_list() {
        let mut app = App::new();
        seed_tree(&mut app);
        app.overlay = Some(Overlay::Diff(fake_diff_view(4)));
        let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
        terminal.draw(|f| ui::draw(f, &mut app)).unwrap();
        let (area, width_before) = match &app.overlay {
            Some(Overlay::Diff(v)) => (v.area, v.files_width),
            _ => panic!("diff overlay gone"),
        };
        assert!(area.width > 0, "modal area written back during draw");
        assert_eq!(width_before, crate::app::DEFAULT_DIFF_FILES_W);

        let bx = area.x + width_before;
        let mut out = Vec::new();
        // Grab the boundary's left border cell and drag 10 columns right.
        handle_mouse(
            &mut app,
            mev(MouseEventKind::Down(MouseButton::Left), bx - 1, area.y + 5),
            &mut out,
        );
        match &app.overlay {
            Some(Overlay::Diff(v)) => {
                assert!(v.files_drag.is_some(), "border click arms the drag");
                assert_eq!(v.selected, 0, "border click selects no row");
            }
            _ => panic!("diff overlay gone"),
        }
        handle_mouse(
            &mut app,
            mev(MouseEventKind::Drag(MouseButton::Left), bx + 9, area.y + 5),
            &mut out,
        );
        match &app.overlay {
            Some(Overlay::Diff(v)) => assert_eq!(v.files_width, width_before + 10),
            _ => panic!("diff overlay gone"),
        }
        assert_eq!(
            app.diff_files_width,
            width_before + 10,
            "width remembered for the next open"
        );

        // A drag far past the right edge clamps so the diff pane keeps its
        // minimum; far left clamps to the file-list minimum.
        handle_mouse(
            &mut app,
            mev(MouseEventKind::Drag(MouseButton::Left), area.x + 200, 5),
            &mut out,
        );
        match &app.overlay {
            Some(Overlay::Diff(v)) => {
                assert_eq!(v.files_width, area.width - crate::app::MIN_DIFF_PANE_W)
            }
            _ => panic!("diff overlay gone"),
        }
        handle_mouse(
            &mut app,
            mev(MouseEventKind::Drag(MouseButton::Left), area.x, 5),
            &mut out,
        );
        match &app.overlay {
            Some(Overlay::Diff(v)) => {
                assert_eq!(v.files_width, crate::app::MIN_DIFF_FILES_W)
            }
            _ => panic!("diff overlay gone"),
        }

        handle_mouse(
            &mut app,
            mev(MouseEventKind::Up(MouseButton::Left), area.x, 5),
            &mut out,
        );
        match &app.overlay {
            Some(Overlay::Diff(v)) => assert!(v.files_drag.is_none(), "mouse-up ends the drag"),
            _ => panic!("diff overlay gone"),
        }
        assert!(out.is_empty(), "resizing never talks to the daemon");
    }

    // ---- `/` fuzzy-search palette ----

    /// A second project ("nebula", branch feat-x, session codex-1) next to
    /// `seed_tree`'s demo/main/agent-1, plus an archived session on demo.
    fn seed_second_project(app: &mut App) {
        use nebula_core::{Agent, AgentStatus, Entity, Project, ProjectId, Worktree, WorktreeId};
        hse(
            app,
            ServerEvent::EntityUpserted {
                entity: Entity::Project(Project {
                    id: ProjectId("p2".into()),
                    name: "nebula".into(),
                    repo_path: "/tmp/nebula".into(),
                    sort_order: 1,
                    divider_after: false,
                    divider_label: None,
                }),
            },
        );
        hse(
            app,
            ServerEvent::EntityUpserted {
                entity: Entity::Worktree(Worktree {
                    id: WorktreeId("w2".into()),
                    project_id: ProjectId("p2".into()),
                    path: "/tmp/nebula".into(),
                    branch: "feat-x".into(),
                    is_main: true,
                    sort_order: 0,
                }),
            },
        );
        hse(
            app,
            ServerEvent::EntityUpserted {
                entity: Entity::Agent(Agent {
                    id: AgentId("a2".into()),
                    worktree_id: WorktreeId("w2".into()),
                    name: "codex-1".into(),
                    status: AgentStatus::Fresh,
                    archived: false,
                    pinned: false,
                    kind: nebula_core::AgentKind::Codex,
                    session_id: None,
                    sort_order: 0,
                    status_changed_at: 0,
                    alive: true,
                }),
            },
        );
        hse(
            app,
            ServerEvent::EntityUpserted {
                entity: Entity::Agent(Agent {
                    id: AgentId("a3".into()),
                    worktree_id: WorktreeId("w1".into()),
                    name: "old-1".into(),
                    status: AgentStatus::Terminated,
                    archived: true,
                    pinned: false,
                    kind: nebula_core::AgentKind::Claude,
                    session_id: None,
                    sort_order: 1,
                    status_changed_at: 0,
                    alive: false,
                }),
            },
        );
    }

    fn palette(app: &App) -> &crate::app::Palette {
        match &app.overlay {
            Some(Overlay::Palette(p)) => p,
            other => panic!("expected palette overlay, got {other:?}"),
        }
    }

    /// Pin the open palette's Enter behavior: `/` snapshots it from the
    /// machine's real config.json, which tests must not depend on.
    fn set_enter_attaches(app: &mut App, v: bool) {
        match &mut app.overlay {
            Some(Overlay::Palette(p)) => p.enter_attaches = v,
            other => panic!("expected palette overlay, got {other:?}"),
        }
    }

    #[test]
    fn slash_opens_palette_listing_projects_then_worktrees_then_sessions() {
        let mut app = App::new();
        seed_tree(&mut app);
        seed_second_project(&mut app);
        let mut out = Vec::new();
        press(&mut app, KeyCode::Char('/'), KeyModifiers::NONE, &mut out);
        let texts: Vec<&str> = palette(&app)
            .items
            .iter()
            .map(|i| i.text.as_str())
            .collect();
        assert_eq!(
            texts,
            vec![
                "demo",
                "nebula",
                "demo/main",
                "nebula/feat-x",
                "demo/main/agent-1",
                "nebula/feat-x/codex-1",
            ],
            "grouped build order, archived hidden by default"
        );
        // The empty query shows everything.
        assert_eq!(palette(&app).matches.len(), texts.len());
        assert!(out.is_empty(), "opening the palette sends nothing");
    }

    #[test]
    fn palette_follows_the_archived_toggle() {
        let mut app = App::new();
        seed_tree(&mut app);
        seed_second_project(&mut app);
        app.show_archived = true;
        let mut out = Vec::new();
        press(&mut app, KeyCode::Char('/'), KeyModifiers::NONE, &mut out);
        let archived: Vec<&str> = palette(&app)
            .items
            .iter()
            .filter(|i| i.archived)
            .map(|i| i.text.as_str())
            .collect();
        assert_eq!(archived, vec!["demo/main/old-1"]);
    }

    #[test]
    fn palette_typing_filters_best_match_first_and_esc_is_two_stage() {
        let mut app = App::new();
        seed_tree(&mut app);
        seed_second_project(&mut app);
        let mut out = Vec::new();
        press(&mut app, KeyCode::Char('/'), KeyModifiers::NONE, &mut out);
        for c in "main".chars() {
            press(&mut app, KeyCode::Char(c), KeyModifiers::NONE, &mut out);
        }
        {
            let p = palette(&app);
            assert_eq!(p.query, "main");
            let top = &p.items[p.matches[0].item];
            // Same boundary match, but the worktree text is shorter than its
            // session's — the tighter candidate wins the tie.
            assert_eq!(top.text, "demo/main");
            assert!(p
                .matches
                .iter()
                .all(|m| p.items[m.item].text.contains("main")));
        }
        // First Esc clears the query, second closes.
        press(&mut app, KeyCode::Esc, KeyModifiers::NONE, &mut out);
        assert_eq!(palette(&app).query, "");
        assert!(!palette(&app).matches.is_empty());
        press(&mut app, KeyCode::Esc, KeyModifiers::NONE, &mut out);
        assert!(app.overlay.is_none());
        assert!(out.is_empty(), "browsing the palette sends nothing");
    }

    #[test]
    fn palette_enter_on_session_selects_the_chain_and_attaches() {
        let mut app = App::new();
        seed_tree(&mut app);
        seed_second_project(&mut app);
        let mut out = Vec::new();
        press(&mut app, KeyCode::Char('/'), KeyModifiers::NONE, &mut out);
        set_enter_attaches(&mut app, true);
        for c in "codex".chars() {
            press(&mut app, KeyCode::Char(c), KeyModifiers::NONE, &mut out);
        }
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE, &mut out);

        assert!(app.overlay.is_none());
        assert_eq!(app.selected_project().unwrap().name, "nebula");
        assert_eq!(app.selected_worktree().unwrap().branch, "feat-x");
        assert_eq!(app.selected_session().unwrap().name, "codex-1");
        assert_eq!(app.focus, Focus::Terminal);
        assert!(app.term_locked, "a session pick locks input immediately");
        assert!(
            out.iter()
                .any(|r| matches!(r, ClientRequest::Attach { session, .. }
                    if *session == SessionRef::Agent(AgentId("a2".into())))),
            "a session pick attaches: {out:?}"
        );
    }

    #[test]
    fn palette_enter_only_focuses_the_row_when_auto_attach_is_off() {
        let mut app = App::new();
        seed_tree(&mut app);
        seed_second_project(&mut app);
        let mut out = Vec::new();
        press(&mut app, KeyCode::Char('/'), KeyModifiers::NONE, &mut out);
        set_enter_attaches(&mut app, false);
        for c in "codex".chars() {
            press(&mut app, KeyCode::Char(c), KeyModifiers::NONE, &mut out);
        }
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE, &mut out);

        assert!(app.overlay.is_none());
        assert_eq!(app.selected_session().unwrap().name, "codex-1");
        assert_eq!(
            app.focus,
            Focus::Sessions,
            "lands on the list, not the terminal"
        );
        assert!(!app.term_locked, "no input lock — Enter on the row commits");
        assert!(
            out.iter()
                .any(|r| matches!(r, ClientRequest::Attach { session, .. }
                    if *session == SessionRef::Agent(AgentId("a2".into())))),
            "the pane still previews the picked session: {out:?}"
        );
    }

    #[test]
    fn palette_ctrl_o_opens_the_session_regardless_of_the_setting() {
        let mut app = App::new();
        seed_tree(&mut app);
        seed_second_project(&mut app);
        let mut out = Vec::new();
        press(&mut app, KeyCode::Char('/'), KeyModifiers::NONE, &mut out);
        set_enter_attaches(&mut app, false);
        for c in "codex".chars() {
            press(&mut app, KeyCode::Char(c), KeyModifiers::NONE, &mut out);
        }
        press(
            &mut app,
            KeyCode::Char('o'),
            KeyModifiers::CONTROL,
            &mut out,
        );

        assert!(app.overlay.is_none());
        assert_eq!(app.selected_session().unwrap().name, "codex-1");
        assert_eq!(app.focus, Focus::Terminal);
        assert!(app.term_locked);
    }

    #[test]
    fn palette_ctrl_f_focuses_the_row_regardless_of_the_setting() {
        let mut app = App::new();
        seed_tree(&mut app);
        seed_second_project(&mut app);
        let mut out = Vec::new();
        press(&mut app, KeyCode::Char('/'), KeyModifiers::NONE, &mut out);
        set_enter_attaches(&mut app, true);
        for c in "codex".chars() {
            press(&mut app, KeyCode::Char(c), KeyModifiers::NONE, &mut out);
        }
        press(
            &mut app,
            KeyCode::Char('f'),
            KeyModifiers::CONTROL,
            &mut out,
        );

        assert!(app.overlay.is_none());
        assert_eq!(app.selected_session().unwrap().name, "codex-1");
        assert_eq!(app.focus, Focus::Sessions);
        assert!(!app.term_locked);
    }

    #[test]
    fn palette_enter_on_worktree_navigates_without_attaching() {
        let mut app = App::new();
        seed_tree(&mut app);
        seed_second_project(&mut app);
        let mut out = Vec::new();
        press(&mut app, KeyCode::Char('/'), KeyModifiers::NONE, &mut out);
        for c in "featx".chars() {
            press(&mut app, KeyCode::Char(c), KeyModifiers::NONE, &mut out);
        }
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE, &mut out);

        assert!(app.overlay.is_none());
        assert_eq!(app.selected_project().unwrap().name, "nebula");
        assert_eq!(app.selected_worktree().unwrap().branch, "feat-x");
        assert_eq!(app.focus, Focus::Worktrees);
        assert!(!app.term_locked);
        assert!(
            !out.iter()
                .any(|r| matches!(r, ClientRequest::Attach { .. })),
            "no remembered session on the target worktree, so nothing attaches: {out:?}"
        );
    }

    #[test]
    fn palette_rebuilds_when_the_tree_changes_under_it() {
        use nebula_core::{Entity, EntityId, Project, ProjectId};
        let mut app = App::new();
        seed_tree(&mut app);
        let mut out = Vec::new();
        press(&mut app, KeyCode::Char('/'), KeyModifiers::NONE, &mut out);
        assert_eq!(palette(&app).items.len(), 3);
        // Park the cursor on the session row before the tree churns.
        press(&mut app, KeyCode::Down, KeyModifiers::NONE, &mut out);
        press(&mut app, KeyCode::Down, KeyModifiers::NONE, &mut out);

        hse(
            &mut app,
            ServerEvent::EntityUpserted {
                entity: Entity::Project(Project {
                    id: ProjectId("p9".into()),
                    name: "fresh".into(),
                    repo_path: "/tmp/fresh".into(),
                    sort_order: 9,
                    divider_after: false,
                    divider_label: None,
                }),
            },
        );
        assert!(
            palette(&app).items.iter().any(|i| i.text == "fresh"),
            "an upsert lands in the open palette"
        );
        assert_eq!(
            palette(&app).selected_target(),
            Some(&crate::app::PaletteTarget::Session(AgentId("a1".into()))),
            "a rebuild keeps the cursor on its target"
        );
        hse(
            &mut app,
            ServerEvent::EntityRemoved {
                id: EntityId::Project(ProjectId("p9".into())),
            },
        );
        assert!(
            !palette(&app).items.iter().any(|i| i.text == "fresh"),
            "a removal drops out of the open palette"
        );
    }

    #[test]
    fn palette_renders_with_kind_badges_and_emoji_panel_titles() {
        let mut app = App::new();
        seed_tree(&mut app);
        let mut out = Vec::new();
        press(&mut app, KeyCode::Char('/'), KeyModifiers::NONE, &mut out);
        let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
        terminal.draw(|f| ui::draw(f, &mut app)).unwrap();
        let text = buffer_text(&terminal);
        assert!(text.contains("Jump to"), "palette title rendered:\n{text}");
        assert!(
            text.contains("type to search"),
            "query placeholder rendered:\n{text}"
        );
        // The TestBackend pads a double-width emoji with a placeholder
        // cell, so match badge and label separately rather than adjacent.
        assert!(text.contains("📁") && text.contains("Projects"), "{text}");
        assert!(text.contains("🌳") && text.contains("Worktrees"), "{text}");
        assert!(text.contains("🤖") && text.contains("Sessions"), "{text}");
        assert!(
            text.contains("demo/main/agent-1"),
            "session row rendered in the palette:\n{text}"
        );
        // Rects for mouse hit-testing were written back during the draw.
        assert!(palette(&app).list_area.width > 0);
    }

    #[test]
    fn s_opens_settings_and_esc_closes() {
        let mut app = App::new();
        let mut out = Vec::new();
        press(&mut app, KeyCode::Char('s'), KeyModifiers::NONE, &mut out);
        assert!(matches!(app.overlay, Some(Overlay::Settings(_))));
        press(&mut app, KeyCode::Esc, KeyModifiers::NONE, &mut out);
        assert!(app.overlay.is_none());
    }

    #[test]
    fn s_toggles_settings_closed_like_help() {
        let mut app = App::new();
        let mut out = Vec::new();
        press(&mut app, KeyCode::Char('s'), KeyModifiers::NONE, &mut out);
        press(&mut app, KeyCode::Char('s'), KeyModifiers::NONE, &mut out);
        assert!(app.overlay.is_none());
    }

    #[test]
    fn settings_j_k_move_selection() {
        let mut app = App::new();
        let mut out = Vec::new();
        press(&mut app, KeyCode::Char('s'), KeyModifiers::NONE, &mut out);
        press(&mut app, KeyCode::Char('j'), KeyModifiers::NONE, &mut out);
        let Some(Overlay::Settings(view)) = &app.overlay else {
            panic!("settings closed");
        };
        assert_eq!(view.selected, 1);
        press(&mut app, KeyCode::Char('k'), KeyModifiers::NONE, &mut out);
        let Some(Overlay::Settings(view)) = &app.overlay else {
            panic!("settings closed");
        };
        assert_eq!(view.selected, 0);
        press(&mut app, KeyCode::Char('k'), KeyModifiers::NONE, &mut out);
        let Some(Overlay::Settings(view)) = &app.overlay else {
            panic!("settings closed");
        };
        assert_eq!(view.selected, 0, "selection does not wrap");
    }

    #[test]
    fn settings_enter_persists_toggle_to_config_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        crate::config::with_config_path(path.clone(), || {
            let mut app = App::new();
            let mut out = Vec::new();
            press(&mut app, KeyCode::Char('s'), KeyModifiers::NONE, &mut out);
            press(&mut app, KeyCode::Enter, KeyModifiers::NONE, &mut out);
            let cfg = crate::config::Config::load();
            assert!(
                !cfg.palette_enter_attaches,
                "Enter toggles the first setting off"
            );
            let saved: serde_json::Value =
                serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
            assert_eq!(saved["palette_enter_attaches"], false);
            assert!(
                matches!(app.overlay, Some(Overlay::Settings(_))),
                "toggle keeps the overlay open"
            );
        });
    }

    #[test]
    fn settings_hl_cycles_recent_window_and_applies() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        crate::config::with_config_path(path, || {
            let mut app = App::new();
            let mut out = Vec::new();
            press(&mut app, KeyCode::Char('s'), KeyModifiers::NONE, &mut out);
            press(&mut app, KeyCode::Char('j'), KeyModifiers::NONE, &mut out);
            press(&mut app, KeyCode::Char('j'), KeyModifiers::NONE, &mut out);
            press(&mut app, KeyCode::Char('l'), KeyModifiers::NONE, &mut out);
            let cfg = crate::config::Config::load();
            assert_eq!(cfg.recent_window, "1h");
            assert_eq!(app.recent_window_ms, 3_600_000);
            press(&mut app, KeyCode::Char('h'), KeyModifiers::NONE, &mut out);
            let cfg = crate::config::Config::load();
            assert_eq!(cfg.recent_window, "30m");
            assert_eq!(
                app.recent_window_ms,
                crate::config::DEFAULT_RECENT_WINDOW_MS
            );
        });
    }

    #[test]
    fn settings_overlay_renders_labels() {
        let mut app = App::new();
        let mut out = Vec::new();
        press(&mut app, KeyCode::Char('s'), KeyModifiers::NONE, &mut out);
        let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
        terminal.draw(|f| ui::draw(f, &mut app)).unwrap();
        let text = buffer_text(&terminal);
        assert!(text.contains("Settings"), "title rendered:\n{text}");
        assert!(
            text.contains("Search Enter attaches"),
            "bool setting rendered:\n{text}"
        );
        assert!(
            text.contains("Recent window"),
            "cycle setting rendered:\n{text}"
        );
        let Some(Overlay::Settings(view)) = &app.overlay else {
            panic!("settings closed");
        };
        assert!(view.area.width > 0, "draw writes hit-test area");
    }
}
