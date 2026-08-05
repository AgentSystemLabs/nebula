//! The main TUI loop: terminal setup/teardown, message routing, update logic.

use crate::app::{
    App, AttachedTerm, ConfirmDialog, ConnState, ContextMenu, Focus, HitTarget, MenuAction,
    MenuItem, Overlay, PendingAction, PendingIntent, PromptDialog, PromptKind, SessionRow,
    TermSelection,
};
use crate::{ipc, keys, ui};
use anyhow::Result;
use crossterm::event::{
    Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use futures::StreamExt;
use nebula_core::{ClientRequest, EntityId, ServerEvent, SessionRef};
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
        let _ = execute!(std::io::stdout(), crossterm::event::PopKeyboardEnhancementFlags);
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
    let mut input = crossterm::event::EventStream::new();
    let mut out: Vec<ClientRequest> = Vec::new();
    let mut next_draw = tokio::time::Instant::now();

    loop {
        if app.dirty && tokio::time::Instant::now() >= next_draw {
            terminal.draw(|f| ui::draw(f, &mut app))?;
            app.dirty = false;
            next_draw = tokio::time::Instant::now() + FRAME_INTERVAL;
            sync_pty_size(&mut app, &mut out);
        }

        let focus_before = app.focus;
        tokio::select! {
            // Pending redraw: wake at the frame boundary even if no new
            // events arrive.
            _ = tokio::time::sleep_until(next_draw), if app.dirty => {}
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
            let _ = channels.tx.send(ClientRequest::SaveUiState { json: ui_state_json(&app) }).await;
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
        session_agent: match app.selected_session() {
            Some(SessionRow::Agent(a)) => Some(a.id.to_string()),
            _ => None,
        },
        session_terminal: match app.selected_session() {
            Some(SessionRow::Terminal(t)) => Some(t.id.to_string()),
            _ => None,
        },
        show_archived: app.show_archived,
        collapsed: app.collapsed,
    };
    serde_json::to_string(&state).unwrap_or_else(|_| "{}".into())
}

fn restore_ui_state(app: &mut App, json: &str) {
    use crate::app::UiState;
    let Ok(state) = serde_json::from_str::<UiState>(json) else { return };
    app.show_archived = state.show_archived;
    if let Some(pid) = &state.project {
        if let Some(i) = app.tree.projects.iter().position(|p| p.id.as_str() == pid) {
            app.sel_project = i;
        }
    }
    if let Some(wid) = &state.worktree {
        if let Some(i) = app.visible_worktrees().iter().position(|w| w.id.as_str() == wid) {
            app.sel_worktree = i;
        }
    }
    let target = state.session_agent.or(state.session_terminal);
    if let Some(sid) = target {
        if let Some(i) = app.visible_sessions().iter().position(|r| match r {
            SessionRow::Agent(a) => a.id.as_str() == sid,
            SessionRow::Terminal(t) => t.id.as_str() == sid,
        }) {
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
                    out.push(ClientRequest::Input { session: term.sref.clone(), data });
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
                // Typing exits scroll mode (tmux behavior).
                if term.scroll > 0 {
                    term.set_scroll(0);
                }
                if let Some(data) = keys::encode_key(&key, term.kitty_flags) {
                    out.push(ClientRequest::Input { session: term.sref.clone(), data });
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
        KeyCode::Char('j') | KeyCode::Down => move_selection(app, 1),
        KeyCode::Char('k') | KeyCode::Up => move_selection(app, -1),
        KeyCode::Enter => match app.focus {
            Focus::Projects => app.focus = Focus::Worktrees,
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
                    open_prompt(app, PromptKind::NewWorktree { project: p.id.clone() });
                }
            }
            Focus::Sessions => {
                if let Some(w) = app.selected_worktree() {
                    open_prompt(app, PromptKind::NewAgent { worktree: w.id.clone() });
                }
            }
            Focus::Terminal => {}
        },
        KeyCode::Char('t') => {
            if app.focus == Focus::Sessions {
                if let Some(w) = app.selected_worktree() {
                    open_prompt(app, PromptKind::NewTerminal { worktree: w.id.clone() });
                }
            }
        }
        KeyCode::Char('r') => {
            if app.focus == Focus::Sessions {
                match app.selected_session() {
                    Some(SessionRow::Agent(a)) => {
                        open_prompt(app, PromptKind::RenameAgent { id: a.id })
                    }
                    Some(SessionRow::Terminal(t)) => {
                        open_prompt(app, PromptKind::RenameTerminal { id: t.id })
                    }
                    None => {}
                }
            }
        }
        KeyCode::Char('a') => {
            if app.focus == Focus::Sessions {
                if let Some(SessionRow::Agent(a)) = app.selected_session() {
                    if !a.archived {
                        open_confirm_archive(app, &a);
                    }
                }
            }
        }
        KeyCode::Char('u') => {
            if app.focus == Focus::Sessions {
                if let Some(SessionRow::Agent(a)) = app.selected_session() {
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
        KeyCode::Char('d') | KeyCode::Delete => open_delete_confirm(app),
        KeyCode::Char('m') => open_context_menu_for_selection(app),
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
        PromptKind::NewWorktree { .. } => {
            ("New worktree".to_string(), "branch name".to_string(), String::new())
        }
        PromptKind::NewAgent { .. } => {
            ("New agent".to_string(), "name".to_string(), app.default_session_name("agent"))
        }
        PromptKind::NewTerminal { .. } => {
            ("New terminal".to_string(), "name".to_string(), app.default_session_name("term"))
        }
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
        PromptKind::RenameTerminal { id } => {
            let current = app
                .tree
                .terminals
                .iter()
                .find(|t| &t.id == id)
                .map(|t| t.name.clone())
                .unwrap_or_default();
            ("Rename terminal".to_string(), "name".to_string(), current)
        }
    };
    app.overlay =
        Some(Overlay::Prompt(PromptDialog { title, label, input, kind, candidates: vec![] }));
}

fn open_confirm_archive(app: &mut App, agent: &nebula_core::Agent) {
    app.overlay = Some(Overlay::Confirm(ConfirmDialog {
        title: "Archive agent".into(),
        message: format!("Archive '{}'? Its running session will be stopped.", agent.name),
        typed_guard: None,
        input: String::new(),
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
                    typed_guard: None,
                    input: String::new(),
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
                    .filter(|r| !r.is_archived())
                    .count();
                app.overlay = Some(Overlay::Confirm(ConfirmDialog {
                    title: "Delete worktree".into(),
                    message: format!(
                        "Delete worktree '{}' from disk? {live_here} session(s) will be killed.",
                        w.branch
                    ),
                    typed_guard: Some(w.branch.clone()),
                    input: String::new(),
                    action: PendingAction::DeleteWorktree(w.id.clone()),
                }));
            }
        }
        Focus::Sessions => match app.selected_session() {
            Some(SessionRow::Agent(a)) => {
                app.overlay = Some(Overlay::Confirm(ConfirmDialog {
                    title: "Delete agent".into(),
                    message: format!("Delete agent '{}'? Its session and history go away.", a.name),
                    typed_guard: None,
                    input: String::new(),
                    action: PendingAction::DeleteAgent(a.id),
                }));
            }
            Some(SessionRow::Terminal(t)) => {
                app.overlay = Some(Overlay::Confirm(ConfirmDialog {
                    title: "Close terminal".into(),
                    message: format!("Close terminal '{}'?", t.name),
                    typed_guard: None,
                    input: String::new(),
                    action: PendingAction::CloseTerminal(t.id),
                }));
            }
            None => {}
        },
        Focus::Terminal => {}
    }
}

fn menu_items_for_session(row: &SessionRow) -> Vec<MenuItem> {
    match row {
        SessionRow::Agent(a) if a.archived => vec![
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
        ],
        SessionRow::Agent(a) => vec![
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
                label: "Rename".into(),
                action: MenuAction::RenameAgent(a.id.clone()),
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
        ],
        SessionRow::Terminal(t) => vec![
            MenuItem {
                label: "Attach".into(),
                action: MenuAction::Attach(SessionRef::Terminal(t.id.clone())),
                destructive: false,
            },
            MenuItem {
                label: "Rename".into(),
                action: MenuAction::RenameTerminal(t.id.clone()),
                destructive: false,
            },
            MenuItem {
                label: "Close".into(),
                action: MenuAction::CloseTerminal(t.id.clone()),
                destructive: true,
            },
        ],
    }
}

fn open_menu(app: &mut App, items: Vec<MenuItem>, at: (u16, u16)) {
    if items.is_empty() {
        return;
    }
    app.overlay = Some(Overlay::Menu(ContextMenu {
        items,
        at,
        hover: 0,
        area: ratatui::layout::Rect::default(),
    }));
}

fn open_context_menu_for_selection(app: &mut App) {
    // Keyboard-invoked menu: anchor near the selected row's panel.
    let at = (30, 4);
    match app.focus {
        Focus::Projects => {
            let mut items = vec![MenuItem {
                label: "Add project".into(),
                action: MenuAction::AddProject,
                destructive: false,
            }];
            if let Some(p) = app.selected_project() {
                items.insert(0, MenuItem {
                    label: "New worktree".into(),
                    action: MenuAction::NewWorktree(p.id.clone()),
                    destructive: false,
                });
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
                let mut items = vec![
                    MenuItem {
                        label: "New agent".into(),
                        action: MenuAction::NewAgent(w.id.clone()),
                        destructive: false,
                    },
                    MenuItem {
                        label: "New terminal".into(),
                        action: MenuAction::NewTerminal(w.id.clone()),
                        destructive: false,
                    },
                ];
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
    let Some(overlay) = &mut app.overlay else { return };
    match overlay {
        Overlay::Help => {
            if matches!(key.code, KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('?')) {
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
            KeyCode::Esc | KeyCode::Char('n') if confirm.typed_guard.is_none() => app.overlay = None,
            KeyCode::Esc => app.overlay = None,
            KeyCode::Enter | KeyCode::Char('y') => {
                let ok = confirm.typed_guard.as_deref().is_none_or(|g| g == confirm.input)
                    && !(key.code == KeyCode::Char('y') && confirm.typed_guard.is_some());
                if ok {
                    let action = confirm.action.clone();
                    app.overlay = None;
                    run_pending_action(app, action, out);
                } else if key.code == KeyCode::Char('y') && confirm.typed_guard.is_some() {
                    // 'y' types into the guard input below.
                    confirm.input.push('y');
                }
            }
            KeyCode::Backspace => {
                confirm.input.pop();
            }
            KeyCode::Char(c) if confirm.typed_guard.is_some()
                && !key.modifiers.contains(KeyModifiers::CONTROL) =>
            {
                confirm.input.push(c)
            }
            _ => {}
        },
    }
}

fn submit_prompt(app: &mut App, prompt: PromptDialog, out: &mut Vec<ClientRequest>) {
    let value = prompt.input.trim().to_string();
    if value.is_empty() {
        app.flash = Some("cancelled: empty input".into());
        return;
    }
    match prompt.kind {
        PromptKind::AddProject => {
            let expanded = shellexpand_home(&value);
            let req_id = app.alloc_req_id(PendingIntent::None);
            out.push(ClientRequest::AddProject { req_id, path: expanded, name: None });
        }
        PromptKind::NewWorktree { project } => {
            let req_id = app.alloc_req_id(PendingIntent::None);
            out.push(ClientRequest::CreateWorktree { req_id, project, branch: value, base: None });
        }
        PromptKind::NewAgent { worktree } => {
            let req_id = app.alloc_req_id(PendingIntent::AttachCreated);
            out.push(ClientRequest::CreateAgent { req_id, worktree, name: value });
        }
        PromptKind::NewTerminal { worktree } => {
            let req_id = app.alloc_req_id(PendingIntent::AttachCreated);
            out.push(ClientRequest::CreateTerminal { req_id, worktree, name: Some(value) });
        }
        PromptKind::RenameAgent { id } => {
            let req_id = app.alloc_req_id(PendingIntent::None);
            out.push(ClientRequest::RenameAgent { req_id, id, name: value });
        }
        PromptKind::RenameTerminal { id } => {
            let req_id = app.alloc_req_id(PendingIntent::None);
            out.push(ClientRequest::RenameTerminal { req_id, id, name: value });
        }
    }
}

fn run_pending_action(app: &mut App, action: PendingAction, out: &mut Vec<ClientRequest>) {
    match action {
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
        PendingAction::CloseTerminal(id) => {
            detach_if_attached(app, &SessionRef::Terminal(id.clone()), out);
            let req_id = app.alloc_req_id(PendingIntent::None);
            out.push(ClientRequest::CloseTerminal { req_id, id });
        }
        PendingAction::DeleteWorktree(id) => {
            let req_id = app.alloc_req_id(PendingIntent::None);
            out.push(ClientRequest::DeleteWorktree { req_id, id, force: true });
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
        MenuAction::RenameTerminal(id) => open_prompt(app, PromptKind::RenameTerminal { id }),
        MenuAction::ArchiveAgent(id) => {
            if let Some(a) = app.tree.agents.iter().find(|a| a.id == id).cloned() {
                open_confirm_archive(app, &a);
            }
        }
        MenuAction::UnarchiveAgent(id) => {
            let req_id = app.alloc_req_id(PendingIntent::None);
            out.push(ClientRequest::UnarchiveAgent { req_id, id });
        }
        MenuAction::DeleteAgent(id) => {
            if let Some(a) = app.tree.agents.iter().find(|a| a.id == id).cloned() {
                app.overlay = Some(Overlay::Confirm(ConfirmDialog {
                    title: "Delete agent".into(),
                    message: format!("Delete agent '{}'? Its session and history go away.", a.name),
                    typed_guard: None,
                    input: String::new(),
                    action: PendingAction::DeleteAgent(id),
                }));
            }
        }
        MenuAction::CloseTerminal(id) => {
            if let Some(t) = app.tree.terminals.iter().find(|t| t.id == id).cloned() {
                app.overlay = Some(Overlay::Confirm(ConfirmDialog {
                    title: "Close terminal".into(),
                    message: format!("Close terminal '{}'?", t.name),
                    typed_guard: None,
                    input: String::new(),
                    action: PendingAction::CloseTerminal(id),
                }));
            }
        }
        MenuAction::NewAgent(worktree) => open_prompt(app, PromptKind::NewAgent { worktree }),
        MenuAction::NewTerminal(worktree) => open_prompt(app, PromptKind::NewTerminal { worktree }),
        MenuAction::NewWorktree(project) => open_prompt(app, PromptKind::NewWorktree { project }),
        MenuAction::DeleteWorktree(id) => {
            if let Some(w) = app.tree.worktrees.iter().find(|w| w.id == id).cloned() {
                app.overlay = Some(Overlay::Confirm(ConfirmDialog {
                    title: "Delete worktree".into(),
                    message: format!("Delete worktree '{}' from disk?", w.branch),
                    typed_guard: Some(w.branch),
                    input: String::new(),
                    action: PendingAction::DeleteWorktree(id),
                }));
            }
        }
        MenuAction::AddProject => open_prompt(app, PromptKind::AddProject),
        MenuAction::RemoveProject(id) => {
            if let Some(p) = app.tree.projects.iter().find(|p| p.id == id).cloned() {
                app.overlay = Some(Overlay::Confirm(ConfirmDialog {
                    title: "Remove project".into(),
                    message: format!("Remove '{}' from nebula? Nothing on disk is touched.", p.name),
                    typed_guard: None,
                    input: String::new(),
                    action: PendingAction::RemoveProject(id),
                }));
            }
        }
        MenuAction::ToggleArchived => app.show_archived = !app.show_archived,
    }
}

fn detach_if_attached(app: &mut App, sref: &SessionRef, out: &mut Vec<ClientRequest>) {
    if let Some(term) = &app.term {
        if &term.sref == sref {
            out.push(ClientRequest::Detach { session: sref.clone() });
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

fn move_selection(app: &mut App, delta: i64) {
    let (len, sel) = match app.focus {
        Focus::Projects => (app.tree.projects.len(), &mut app.sel_project),
        Focus::Worktrees => (app.visible_worktrees().len(), &mut app.sel_worktree),
        Focus::Sessions => (app.visible_sessions().len(), &mut app.sel_session),
        Focus::Terminal => return,
    };
    if len == 0 {
        return;
    }
    let new = (*sel as i64 + delta).clamp(0, len as i64 - 1) as usize;
    if new != *sel {
        *sel = new;
        // Selecting a different parent resets child selections.
        match app.focus {
            Focus::Projects => {
                app.sel_worktree = 0;
                app.sel_session = 0;
            }
            Focus::Worktrees => app.sel_session = 0,
            _ => {}
        }
    }
}

fn attach_selected(app: &mut App, out: &mut Vec<ClientRequest>) {
    let sessions = app.visible_sessions();
    let Some(row) = sessions.get(app.sel_session) else {
        return;
    };
    attach(app, row.sref(), out);
    app.focus = Focus::Terminal;
    app.term_locked = true;
}

fn attach(app: &mut App, sref: SessionRef, out: &mut Vec<ClientRequest>) {
    if let Some(existing) = &app.term {
        if existing.sref == sref && !existing.exited {
            return; // already attached
        }
        out.push(ClientRequest::Detach { session: existing.sref.clone() });
    }
    let area = app.term_area;
    let (cols, rows) = if area.width >= 2 && area.height >= 2 {
        (area.width, area.height)
    } else {
        (80, 24)
    };
    app.term = Some(AttachedTerm::new(sref.clone(), cols, rows));
    out.push(ClientRequest::Attach { session: sref, from_seq: None, cols, rows });
}

/// Mouse position → pane-relative cell, clamped into the terminal area (so a
/// drag that wanders outside the pane keeps selecting the nearest edge).
fn pane_cell(area: ratatui::layout::Rect, col: u16, row: u16) -> (u16, u16) {
    let max_x = area.x + area.width.saturating_sub(1);
    let max_y = area.y + area.height.saturating_sub(1);
    (col.clamp(area.x, max_x) - area.x, row.clamp(area.y, max_y) - area.y)
}

/// Text under the current drag-selection, from the screen's visible view
/// (respects scrollback offset and wrapped rows).
fn selection_text(app: &App) -> Option<String> {
    let sel = app.term_selection.as_ref()?;
    if sel.is_empty() {
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

/// Complete a drag-selection: copy the text to the system clipboard and drop
/// the highlight. A drag that never left its starting cell is just a click.
fn finish_selection(app: &mut App) {
    let text = selection_text(app);
    app.term_selection = None;
    app.dirty = true;
    if let Some(text) = text {
        app.flash = Some(if copy_to_clipboard(&text) {
            format!("copied {} chars", text.chars().count())
        } else {
            "copy failed (clipboard unavailable)".into()
        });
    }
}

/// Copy to the system clipboard via pbcopy (this tool targets macOS).
fn copy_to_clipboard(text: &str) -> bool {
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
    // Other overlays: keyboard only; ignore mouse.
    if app.overlay.is_some() {
        return;
    }

    match mouse.kind {
        MouseEventKind::Down(MouseButton::Left) => {
            // Any fresh click clears a stale selection highlight; a click on
            // the terminal pane below re-arms one.
            app.term_selection = None;
            match app.hit_at(mouse.column, mouse.row) {
                Some(HitTarget::Project(i)) => {
                    if app.sel_project != i {
                        app.sel_project = i;
                        app.sel_worktree = 0;
                        app.sel_session = 0;
                    }
                    app.focus = Focus::Projects;
                }
                Some(HitTarget::Worktree(i)) => {
                    if app.sel_worktree != i {
                        app.sel_worktree = i;
                        app.sel_session = 0;
                    }
                    app.focus = Focus::Worktrees;
                }
                Some(HitTarget::Session(i)) => {
                    app.sel_session = i;
                    if app.selected_session().is_some_and(|r| r.is_archived()) {
                        app.focus = Focus::Sessions;
                        app.flash = Some("agent is archived — unarchive first (u)".into());
                    } else {
                        attach_selected(app, out);
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
                        // Arm a drag-selection; it becomes visible (and
                        // copyable) once the drag leaves this cell.
                        let cell = pane_cell(app.term_area, mouse.column, mouse.row);
                        app.term_selection =
                            Some(TermSelection { anchor: cell, head: cell, dragging: true });
                    }
                }
                None => {}
            }
            app.dirty = true;
        }
        MouseEventKind::Drag(MouseButton::Left) => {
            if let Some(sel) = &mut app.term_selection {
                if sel.dragging {
                    sel.head = pane_cell(app.term_area, mouse.column, mouse.row);
                    app.dirty = true;
                }
            }
        }
        MouseEventKind::Up(MouseButton::Left) => {
            if app.term_selection.is_some_and(|s| s.dragging) {
                finish_selection(app);
            }
        }
        MouseEventKind::ScrollUp | MouseEventKind::ScrollDown => {
            let up = matches!(mouse.kind, MouseEventKind::ScrollUp);
            let in_term = matches!(app.hit_at(mouse.column, mouse.row), Some(HitTarget::TerminalPane))
                || app.collapsed;
            if in_term {
                if let Some(term) = &mut app.term {
                    if term.parser.screen().alternate_screen() {
                        // Full-screen apps (vim, htop, claude) expect arrows.
                        let arrow: &[u8] = if up { b"\x1b[A\x1b[A\x1b[A" } else { b"\x1b[B\x1b[B\x1b[B" };
                        out.push(ClientRequest::Input {
                            session: term.sref.clone(),
                            data: arrow.to_vec(),
                        });
                    } else {
                        let new_scroll =
                            if up { term.scroll.saturating_add(3) } else { term.scroll.saturating_sub(3) };
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
                    if let Some(p) = app.selected_project() {
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
                        let mut items = vec![
                            MenuItem {
                                label: "New agent".into(),
                                action: MenuAction::NewAgent(w.id.clone()),
                                destructive: false,
                            },
                            MenuItem {
                                label: "New terminal".into(),
                                action: MenuAction::NewTerminal(w.id.clone()),
                                destructive: false,
                            },
                        ];
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
                                        label: "New terminal".into(),
                                        action: MenuAction::NewTerminal(w.id.clone()),
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
        ServerEvent::Snapshot { projects, worktrees, agents, terminals, ui_state } => {
            app.tree.projects = projects;
            app.tree.worktrees = worktrees;
            app.tree.agents = agents;
            app.tree.terminals = terminals;
            if let Some(json) = ui_state {
                restore_ui_state(app, &json);
            }
            clamp_selections(app);
            app.dirty = true;
        }
        ServerEvent::Scrollback { session, data, .. } => {
            if let Some(term) = &mut app.term {
                if term.sref == session {
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
        ServerEvent::StatusChanged { agent, status } => {
            if let Some(a) = app.tree.agents.iter_mut().find(|a| a.id == agent) {
                a.status = status;
                app.dirty = true;
            }
        }
        ServerEvent::Ack { req_id, created } => {
            if let (Some(PendingIntent::AttachCreated), Some(id)) = (app.pending.remove(&req_id), created) {
                let sref = match id {
                    EntityId::Agent(id) => Some(SessionRef::Agent(id)),
                    EntityId::Terminal(id) => Some(SessionRef::Terminal(id)),
                    _ => None,
                };
                if let Some(sref) = sref {
                    app.select_when_seen = Some(sref.clone());
                    attach(app, sref, out);
                    app.focus = Focus::Terminal;
                    app.term_locked = true;
                }
            }
            app.dirty = true;
        }
        ServerEvent::EntityUpserted { entity } => {
            apply_upsert(app, entity);
            // Fix the selection onto a session we just created.
            if let Some(pending_sref) = app.select_when_seen.clone() {
                if let Some(index) =
                    app.visible_sessions().iter().position(|r| r.sref() == pending_sref)
                {
                    app.sel_session = index;
                    app.select_when_seen = None;
                }
            }
            app.dirty = true;
        }
        ServerEvent::EntityRemoved { id } => {
            apply_removal(app, &id);
            clamp_selections(app);
            app.dirty = true;
        }
        ServerEvent::Error { message, .. } => {
            app.flash = Some(message);
            app.dirty = true;
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nebula_core::{ServerEvent, SessionRef, TerminalId};
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
        use nebula_core::{Entity, Project, ProjectId, TerminalTab, Worktree, WorktreeId};
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
                entity: Entity::Terminal(TerminalTab {
                    id: TerminalId("scratch-1".into()),
                    worktree_id,
                    name: "scratch-1".into(),
                    sort_order: 0,
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

        let sref = SessionRef::Terminal(TerminalId("scratch-1".into()));
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
            ServerEvent::Output { session: sref, seq: 27, data: b"!\r\nline2".to_vec() },
        );

        let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
        terminal.draw(|f| ui::draw(f, &mut app)).unwrap();
        let text = buffer_text(&terminal);
        assert!(text.contains("hello from vt100 world!"), "terminal content rendered:\n{text}");
        assert!(text.contains("line2"), "second line rendered:\n{text}");
        assert!(text.contains("scratch-1"), "session row rendered:\n{text}");
        assert!(text.contains("AGENTS"), "agents group header rendered:\n{text}");
        assert!(text.contains("TERMINALS"), "terminals group header rendered:\n{text}");
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
        handle_key(&mut app, KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE), &mut out);
        let Some(Overlay::Prompt(p)) = &app.overlay else { panic!("prompt closed") };
        assert_eq!(p.input, format!("{}/workspace/", tmp.path().display()));
        assert!(p.candidates.is_empty());

        // Ambiguous at the directory root: list both candidates.
        handle_key(&mut app, KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE), &mut out);
        let Some(Overlay::Prompt(p)) = &app.overlay else { panic!("prompt closed") };
        assert_eq!(p.candidates, vec!["herdr/", "nebula/"]);

        // Typing narrows; next Tab completes fully and clears the list.
        handle_key(&mut app, KeyEvent::new(KeyCode::Char('n'), KeyModifiers::NONE), &mut out);
        let Some(Overlay::Prompt(p)) = &app.overlay else { panic!("prompt closed") };
        assert!(p.candidates.is_empty(), "editing clears candidates");
        handle_key(&mut app, KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE), &mut out);
        let Some(Overlay::Prompt(p)) = &app.overlay else { panic!("prompt closed") };
        assert_eq!(p.input, format!("{}/workspace/nebula/", tmp.path().display()));
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
            kind: PromptKind::RenameAgent { id: nebula_core::AgentId("a1".into()) },
            candidates: vec![],
        }));
        handle_key(&mut app, KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE), &mut out);
        let Some(Overlay::Prompt(p)) = &app.overlay else { panic!("prompt closed") };
        assert_eq!(p.input, "src", "name prompts ignore Tab");
    }

    #[test]
    fn keys_route_by_focus() {
        let mut app = App::new();
        seed_tree(&mut app);
        let mut out = Vec::new();

        // Panel focus: 'q' quits.
        handle_key(&mut app, KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE), &mut out);
        assert!(app.should_quit);
        app.should_quit = false;

        // Terminal input-locked: 'q' is forwarded, Ctrl+q escapes and unlocks.
        app.focus = Focus::Terminal;
        app.term_locked = true;
        let sref = SessionRef::Terminal(TerminalId("scratch-1".into()));
        app.term = Some(AttachedTerm::new(sref, 80, 24));
        handle_key(&mut app, KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE), &mut out);
        assert!(!app.should_quit, "q must forward to pty, not quit");
        assert!(matches!(out.last(), Some(ClientRequest::Input { data, .. }) if data == b"q"));
        handle_key(&mut app, KeyEvent::new(KeyCode::Char('q'), KeyModifiers::CONTROL), &mut out);
        assert_eq!(app.focus, Focus::Sessions, "Ctrl+q escapes to panels");
        assert!(!app.term_locked, "Ctrl+q clears the input lock");
    }

    #[test]
    fn escape_hatches_leave_terminal_lock() {
        let mut app = App::new();
        seed_tree(&mut app);
        let mut out = Vec::new();

        let sref = SessionRef::Terminal(TerminalId("scratch-1".into()));
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
            handle_key(&mut app, KeyEvent::new(code, KeyModifiers::CONTROL), &mut out);
            assert_eq!(app.focus, Focus::Sessions, "Ctrl+{code:?} leaves terminal input");
            assert!(!app.term_locked, "Ctrl+{code:?} clears the input lock");
            assert!(out.is_empty(), "Ctrl+{code:?} must not reach the pty");
        }

        // Bare Esc is NOT a hatch: it forwards to the pty untouched — Claude
        // Code owns Esc (interrupt) and double-Esc (clear input / jump back).
        app.focus = Focus::Terminal;
        app.term_locked = true;
        handle_key(&mut app, KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE), &mut out);
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
        handle_key(&mut app, KeyEvent::new(KeyCode::Left, KeyModifiers::SUPER), &mut out);
        assert_eq!(app.focus, Focus::Terminal, "Cmd+Left does not escape");
        assert!(app.term_locked, "Cmd+Left keeps the input lock");
        assert!(out.is_empty(), "Cmd+Left has no legacy pty encoding");
    }

    #[test]
    fn focus_without_lock_navigates_instead_of_forwarding() {
        let mut app = App::new();
        seed_tree(&mut app);
        let mut out = Vec::new();

        let sref = SessionRef::Terminal(TerminalId("scratch-1".into()));
        app.term = Some(AttachedTerm::new(sref, 80, 24));
        app.focus = Focus::Terminal; // focused via Tab/arrows — NOT locked

        // Arrows navigate panels instead of reaching the pty.
        handle_key(&mut app, KeyEvent::new(KeyCode::Left, KeyModifiers::NONE), &mut out);
        assert_eq!(app.focus, Focus::Sessions, "unlocked pane falls through to navigation");
        assert!(out.is_empty(), "no input to the pty while unlocked");

        // Enter from the sessions panel attaches AND locks.
        handle_key(&mut app, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), &mut out);
        assert_eq!(app.focus, Focus::Terminal);
        assert!(app.term_locked, "Enter on a session locks input into the terminal");

        // Ctrl+Left back out, Ctrl+Right to refocus the pane, Enter re-locks.
        handle_key(&mut app, KeyEvent::new(KeyCode::Left, KeyModifiers::CONTROL), &mut out);
        assert!(!app.term_locked);
        handle_key(&mut app, KeyEvent::new(KeyCode::Right, KeyModifiers::CONTROL), &mut out);
        assert_eq!(app.focus, Focus::Terminal);
        assert!(!app.term_locked, "focusing the pane does not lock it");
        handle_key(&mut app, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), &mut out);
        assert!(app.term_locked, "Enter on the focused pane locks input");
    }

    #[test]
    fn drag_selection_selects_and_extracts_text() {
        let mut app = App::new();
        seed_tree(&mut app);
        let mut out = Vec::new();

        let sref = SessionRef::Terminal(TerminalId("scratch-1".into()));
        let mut term = AttachedTerm::new(sref, 80, 24);
        term.parser.process(b"hello world");
        app.term = Some(term);
        app.term_area = ratatui::layout::Rect::new(0, 0, 80, 24);
        app.hits.push((app.term_area, HitTarget::TerminalPane));

        let ev = |kind, column, row| MouseEvent { kind, column, row, modifiers: KeyModifiers::NONE };

        // Mouse-down on the pane arms an (empty) selection and locks input.
        handle_mouse(&mut app, ev(MouseEventKind::Down(MouseButton::Left), 0, 0), &mut out);
        assert!(app.term_selection.is_some_and(|s| s.dragging && s.is_empty()));
        assert!(app.term_locked, "click into the pane still locks input");

        // Dragging extends the selection; the text under it is extractable.
        handle_mouse(&mut app, ev(MouseEventKind::Drag(MouseButton::Left), 10, 0), &mut out);
        let sel = app.term_selection.expect("drag keeps the selection");
        assert_eq!(sel.bounds(), ((0, 0), (10, 0)));
        assert_eq!(selection_text(&app).as_deref(), Some("hello world"));

        // A drag that wanders outside the pane clamps to the nearest edge.
        handle_mouse(&mut app, ev(MouseEventKind::Drag(MouseButton::Left), 200, 50), &mut out);
        assert_eq!(app.term_selection.expect("still selecting").head, (79, 23));

        // A fresh click outside the pane clears the highlight.
        app.hits.clear();
        handle_mouse(&mut app, ev(MouseEventKind::Down(MouseButton::Left), 0, 0), &mut out);
        assert!(app.term_selection.is_none(), "click elsewhere clears the selection");
    }

    #[test]
    fn exited_session_does_not_trap_keys() {
        let mut app = App::new();
        seed_tree(&mut app);
        let mut out = Vec::new();

        let sref = SessionRef::Terminal(TerminalId("scratch-1".into()));
        app.term = Some(AttachedTerm::new(sref, 80, 24));
        app.term.as_mut().unwrap().exited = true;
        app.focus = Focus::Terminal;
        app.term_locked = true;
        app.collapsed = true;

        // No input reaches a dead PTY.
        handle_key(&mut app, KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE), &mut out);
        assert!(out.is_empty(), "no input to a dead pty");

        // Esc leaves the pane and expands collapsed sidebars.
        handle_key(&mut app, KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE), &mut out);
        assert_eq!(app.focus, Focus::Sessions, "Esc leaves an exited pane");
        assert!(!app.collapsed, "escape expands sidebars");

        // Navigation keys fall through instead of being swallowed.
        app.focus = Focus::Terminal;
        handle_key(&mut app, KeyEvent::new(KeyCode::Left, KeyModifiers::NONE), &mut out);
        assert_eq!(app.focus, Focus::Sessions, "arrow navigation works from an exited pane");
    }
}

fn apply_upsert(app: &mut App, entity: nebula_core::Entity) {
    use nebula_core::Entity;
    match entity {
        Entity::Project(p) => {
            match app.tree.projects.iter_mut().find(|x| x.id == p.id) {
                Some(existing) => *existing = p,
                None => app.tree.projects.push(p),
            }
        }
        Entity::Worktree(w) => {
            match app.tree.worktrees.iter_mut().find(|x| x.id == w.id) {
                Some(existing) => *existing = w,
                None => app.tree.worktrees.push(w),
            }
        }
        Entity::Agent(a) => {
            match app.tree.agents.iter_mut().find(|x| x.id == a.id) {
                Some(existing) => *existing = a,
                None => app.tree.agents.push(a),
            }
        }
        Entity::Terminal(t) => {
            match app.tree.terminals.iter_mut().find(|x| x.id == t.id) {
                Some(existing) => *existing = t,
                None => app.tree.terminals.push(t),
            }
        }
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
            app.tree.terminals.retain(|t| !wt_ids.contains(&t.worktree_id));
            app.tree.worktrees.retain(|w| &w.project_id != id);
            app.tree.projects.retain(|p| &p.id != id);
        }
        EntityId::Worktree(id) => {
            app.tree.agents.retain(|a| &a.worktree_id != id);
            app.tree.terminals.retain(|t| &t.worktree_id != id);
            app.tree.worktrees.retain(|w| &w.id != id);
        }
        EntityId::Agent(id) => app.tree.agents.retain(|a| &a.id != id),
        EntityId::Terminal(id) => app.tree.terminals.retain(|t| &t.id != id),
    }
}

/// Keep selections valid after the tree shrinks.
fn clamp_selections(app: &mut App) {
    if app.sel_project >= app.tree.projects.len() {
        app.sel_project = app.tree.projects.len().saturating_sub(1);
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
