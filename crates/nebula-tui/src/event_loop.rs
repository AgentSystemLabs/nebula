//! The main TUI loop: terminal setup/teardown, message routing, update logic.

use crate::app::{
    App, AttachedTerm, ConfirmDialog, ConnState, ContextMenu, DiffView, FileFinder, Focus,
    GrepView, HitTarget, MenuAction, MenuItem, MetricsView, NoteInput, NoteView, Overlay, Palette,
    PaletteTarget, PendingAction, PendingIntent, PointerShape, ProjectRow, PromptDialog,
    PromptKind, SessionRow, SettingsView, SplitterDrag, SubmenuKind, TermSelection,
    WorktreeRollback,
};
use crate::text_input::TextInput;
use crate::tree_browser::TreeBrowser;
use crate::vim_term::{VimEvent, VimTerm};
use crate::{ipc, keys, ui};
use anyhow::Result;
use crossterm::event::{
    Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use futures::StreamExt;
use nebula_core::{
    AgentId, AgentKind, ClientRequest, EntityId, NoteId, NoteOwner, ServerEvent, SessionRef,
    WorktreeId,
};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;
use std::io::{BufWriter, Stdout};
use std::time::Duration;

/// Rows the Sessions column scrolls per wheel notch — one pill's stride,
/// so the list steps by whole rows instead of drifting half a pill.
const SESSIONS_WHEEL_STEP: usize = 2;

/// Redraw cap (~60fps). Output bursts coalesce into one frame; input events
/// are still handled immediately between frames.
const FRAME_INTERVAL: Duration = Duration::from_millis(16);

/// How often the worktree panel's changed-file badge re-reads `git status`
/// for the selected checkout, so agent edits surface without a keypress.
const GIT_POLL: Duration = Duration::from_secs(2);

/// How long the worktree selection must rest before asking the daemon to
/// pre-spawn that worktree's dead sessions — long enough that walking the
/// list doesn't boot every CLI passed, short enough that the sessions are
/// booting well before the user picks one.
const PREWARM_DEBOUNCE: Duration = Duration::from_millis(250);

/// How often the standing keep-warm request for the selected worktree's
/// default-spec Claude session is re-sent. Must stay comfortably under the
/// daemon's reap window minus its recycle threshold, so the warm slot is
/// refreshed (a young session is a no-op, an aging one is recycled) before
/// the reaper can empty it.
const KEEPWARM_REFRESH: Duration = Duration::from_secs(4 * 60);

/// While the metrics modal is open, how often a fresh memory reading is
/// requested from the daemon.
const METRICS_POLL: Duration = Duration::from_secs(2);

/// With the modal closed, how often the footer's memory/session readout is
/// refreshed.
const FOOTER_METRICS_POLL: Duration = Duration::from_secs(5);

/// Repaint cadence for the first-run splash animation — the only thing
/// that marks the app dirty while it idles on an empty tree.
const SPLASH_FRAME: Duration = crate::splash::FRAME;

/// Repaint cadence for the status-sweep text animation on running /
/// needs-feedback rows.
const SWEEP_FRAME: Duration = crate::app::SWEEP_FRAME;

/// `Some(entry)` = quit via the hosts picker: the caller should exec
/// `nebula ssh` at it now that the terminal is restored.
pub async fn run_app() -> Result<Option<crate::hosts::HostEntry>> {
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
        // Hand back the default pointer in case we left it col-resize
        // (OSC 22; terminals without pointer-shape support drop it).
        crossterm::style::Print("\x1b]22;default\x1b\\"),
        crossterm::event::DisableBracketedPaste,
        crossterm::event::DisableMouseCapture,
        LeaveAlternateScreen,
    );
    let _ = disable_raw_mode();
}

async fn main_loop(
    terminal: &mut Terminal<CrosstermBackend<BufWriter<Stdout>>>,
    channels: &mut ipc::IpcChannels,
) -> Result<Option<crate::hosts::HostEntry>> {
    let mut app = App::new();
    app.conn = ConnState::Connected;
    let cfg = crate::config::Config::load();
    app.recent_window_ms = cfg.recent_window_ms();
    app.theme = cfg.theme();
    app.animations = cfg.animations;
    app.focus_tint = cfg.focus_tint;
    let mut input = crossterm::event::EventStream::new();
    let mut out: Vec<ClientRequest> = Vec::new();
    // Pointer shape last sent to the terminal (OSC 22), so hover over a
    // splitter swaps the cursor once instead of on every motion event.
    let mut pointer_sent = PointerShape::default();
    let mut next_draw = tokio::time::Instant::now();
    let mut next_git_poll = tokio::time::Instant::now();
    let mut next_metrics_poll = tokio::time::Instant::now();
    let mut next_splash_frame = tokio::time::Instant::now();
    let mut next_sweep_frame = tokio::time::Instant::now();
    // Editor-modal PTY output; the channel outlives individual editor
    // spawns (VimEvent generations keep them apart).
    let (vim_tx, mut vim_rx) = tokio::sync::mpsc::unbounded_channel::<VimEvent>();
    app.vim_tx = Some(vim_tx);

    loop {
        if app.dirty && tokio::time::Instant::now() >= next_draw {
            // A selection change must never paint another checkout's badge;
            // between selections the slow poll keeps the count fresh.
            if app.git_changes_stale() {
                refresh_git_changes(&mut app);
            }
            terminal.draw(|f| ui::draw(f, &mut app))?;
            app.dirty = false;
            next_draw = tokio::time::Instant::now() + FRAME_INTERVAL;
            sync_pty_size(&mut app, &mut out);
            sync_vim_size(&mut app);
            // What the user sees selected, for RECENT-expiry re-anchoring.
            app.drawn_session = app.selected_session_row().map(|r| r.sref());
        }

        // Wake when the next RECENT session ages out so the list regroups
        // (slack so the wakeup lands past the boundary).
        let recent_expiry = app.next_recent_expiry();

        let focus_before = app.focus;
        tokio::select! {
            // Pending redraw: wake at the frame boundary even if no new
            // events arrive.
            _ = tokio::time::sleep_until(next_draw), if app.dirty => {}
            // Fixed deadline (not a fresh sleep per iteration) so heavy PTY
            // traffic can't starve the badge refresh.
            _ = tokio::time::sleep_until(next_git_poll) => {
                refresh_git_changes(&mut app);
                next_git_poll = tokio::time::Instant::now() + GIT_POLL;
            }
            // Metrics poll: always on for the footer's memory/session
            // readout, tightened while the metrics modal is open (its
            // initial reading is requested by the M keypress itself).
            _ = tokio::time::sleep_until(next_metrics_poll) => {
                request_metrics(&mut app, &mut out);
                let period = if matches!(app.overlay, Some(Overlay::Metrics(_))) {
                    METRICS_POLL
                } else {
                    FOOTER_METRICS_POLL
                };
                next_metrics_poll = tokio::time::Instant::now() + period;
            }
            // First-run splash: while it's on screen nothing else repaints
            // an idle app, so tick the animation on a fixed cadence.
            _ = tokio::time::sleep_until(next_splash_frame), if app.splash_active() => {
                app.dirty = true;
                next_splash_frame = tokio::time::Instant::now() + SPLASH_FRAME;
            }
            // Status sweep: running / needs-feedback rows shimmer, so keep
            // repainting while any are visible (same pure-function-of-time
            // model as the splash — a missed tick skips ahead cleanly).
            _ = tokio::time::sleep_until(next_sweep_frame), if app.status_anim_active() => {
                app.dirty = true;
                next_sweep_frame = tokio::time::Instant::now() + SWEEP_FRAME;
            }
            _ = tokio::time::sleep(recent_expiry.unwrap_or_default() + Duration::from_millis(250)),
                if recent_expiry.is_some() =>
            {
                // Re-read the config so window edits apply without restart.
                app.recent_window_ms = crate::config::Config::load().recent_window_ms();
                // The expired session dropped down the list; keep the
                // selection on whatever row the user had selected.
                if let Some(keep) = app.drawn_session.clone() {
                    if let Some(i) = app
                        .visible_session_rows()
                        .iter()
                        .position(|r| r.sref() == keep)
                    {
                        app.sel_session = i;
                    }
                }
                app.dirty = true;
            }
            // The worktree selection rested past the debounce: ask the
            // daemon to boot its dead sessions in the background so
            // attaching one replays a live screen instead of a cold boot.
            _ = tokio::time::sleep(app.prewarm_delay().unwrap_or_default()),
                if app.pending_prewarm.is_some() =>
            {
                fire_pending_prewarm(&mut app, &mut out);
            }
            // Standing keep-warm: periodically re-assert the selected
            // worktree's warm default-spec Claude session so the daemon's
            // reaper never leaves the next create cold.
            _ = tokio::time::sleep(app.keepwarm_delay().unwrap_or_default()),
                if app.next_keepwarm.is_some() =>
            {
                fire_keepwarm(&mut app, &mut out);
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
            ev = vim_rx.recv() => {
                // Never None: app.vim_tx keeps a sender alive.
                if let Some(ev) = ev {
                    handle_vim_event(&mut app, ev);
                }
            }
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
        while let Ok(ev) = vim_rx.try_recv() {
            handle_vim_event(&mut app, ev);
        }

        // Mouse handlers only record the pointer shape they want; emit the
        // OSC 22 request when it changes. Terminals without pointer-shape
        // support (Terminal.app) parse and drop the sequence.
        if app.pointer_shape != pointer_sent {
            pointer_sent = app.pointer_shape;
            use std::io::Write;
            let backend = terminal.backend_mut();
            let _ = write!(backend, "\x1b]22;{}\x1b\\", pointer_sent.osc_name());
            let _ = backend.flush();
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
            return Ok(app.pending_ssh.take());
        }
    }
}

/// Recompute the changed-file count behind the worktree panel's badge.
/// Synchronous `git status` on purpose (the git_diff.rs precedent): it runs
/// once per `GIT_POLL` plus on selection changes, off the input hot path.
fn refresh_git_changes(app: &mut App) {
    let next = app
        .selected_worktree()
        .map(|w| (w.id.clone(), w.path.clone()))
        .map(|(id, path)| {
            let count = crate::git_diff::changed_files(&path).ok().map(|f| f.len());
            (id, count)
        });
    if app.git_changes != next {
        app.git_changes = next;
        app.dirty = true;
    }
}

/// Fire one memory reading for the metrics modal: sample this client's own
/// RSS now (the daemon can't see us), ask the daemon for itself plus every
/// session's process tree. The reply arrives as `ServerEvent::Metrics`.
fn request_metrics(app: &mut App, out: &mut Vec<ClientRequest>) {
    app.client_rss_bytes = nebula_core::mem::process_rss_bytes(std::process::id()).unwrap_or(0);
    if let Some(Overlay::Metrics(view)) = &mut app.overlay {
        view.client_rss_bytes = app.client_rss_bytes;
    }
    let req_id = app.alloc_req_id(PendingIntent::None);
    out.push(ClientRequest::GetMetrics { req_id });
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
            .visible_session_rows()
            .iter()
            .position(|r| matches!(r, SessionRow::Agent(a) if a.id.as_str() == sid))
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

/// Keep the editor modal's PTY and parser sized to the drawn inner rect
/// (the `sync_pty_size` pattern, minus the daemon round-trip).
fn sync_vim_size(app: &mut App) {
    if let Some(vim) = &mut app.vim {
        if vim.area.width >= 2 && vim.area.height >= 2 {
            vim.resize(vim.area.width, vim.area.height);
        }
    }
}

/// Editor reader-thread events. A stale generation (bytes buffered from an
/// editor that was already closed) is dropped on the floor.
fn handle_vim_event(app: &mut App, ev: VimEvent) {
    match ev {
        VimEvent::Output { generation, data } => {
            if let Some(vim) = &mut app.vim {
                if vim.generation == generation {
                    vim.process(&data);
                    app.dirty = true;
                }
            }
        }
        VimEvent::Exited { generation } => {
            if app.vim.as_ref().is_some_and(|v| v.generation == generation) {
                close_vim(app);
                app.dirty = true;
            }
        }
    }
}

/// Drop the editor; an embedded one hands its preview pane back to the tree
/// browser with the (possibly just-edited) file reloaded.
fn close_vim(app: &mut App) {
    let embedded = app.vim.as_ref().is_some_and(|v| v.embedded);
    app.vim = None;
    if embedded {
        if let Some(Overlay::Tree(view)) = &mut app.overlay {
            view.load_preview();
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
        Event::Paste(text) if app.vim.is_some() => {
            if let Some(vim) = &mut app.vim {
                // Bracketed paste so vim doesn't auto-indent it to mush.
                let mut data = b"\x1b[200~".to_vec();
                data.extend_from_slice(text.as_bytes());
                data.extend_from_slice(b"\x1b[201~");
                vim.input(&data);
            }
        }
        // An overlay with a live text field takes the paste: ⌘V into a note,
        // a filter, or the ssh destination lands where the caret is.
        Event::Paste(text) if paste_into_overlay(app, &text) => {}
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

/// Route a bracketed paste into whatever text field the open overlay has
/// live. Returns false when nothing is typing, so the paste falls through to
/// the terminal pane.
fn paste_into_overlay(app: &mut App, text: &str) -> bool {
    let Some(overlay) = &mut app.overlay else {
        return false;
    };
    match overlay {
        Overlay::Prompt(prompt) => {
            prompt.input.insert_str(text);
            prompt.refresh_dirs();
        }
        Overlay::Palette(palette) => {
            palette.query.insert_str(text);
            palette.apply_filter();
        }
        Overlay::Files(finder) => {
            finder.query.insert_str(text);
            finder.apply_filter();
        }
        Overlay::Grep(view) => {
            view.query.insert_str(text);
            view.run_search();
        }
        Overlay::Tree(view) => {
            view.filter.insert_str(text);
            view.apply_filter();
        }
        Overlay::Diff(view) => {
            view.filter.insert_str(text);
            if view.apply_filter() {
                crate::git_diff::load_selected_diff(view);
            }
        }
        // These two only type while their add/edit input is open.
        Overlay::Notes(view) => match &mut view.input {
            Some(input) => input.text.insert_str(text),
            None => return false,
        },
        Overlay::Hosts(view) => match &mut view.input {
            Some(input) => input.insert_str(text),
            None => return false,
        },
        _ => return false,
    }
    app.dirty = true;
    true
}

fn handle_key(app: &mut App, key: KeyEvent, out: &mut Vec<ClientRequest>) {
    // The editor modal sits above every overlay: all keys forward to it —
    // vim needs Esc — except Ctrl+Q, the same hatch the terminal lock uses.
    if app.vim.is_some() {
        handle_vim_key(app, key);
        return;
    }

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

    // Splash preview up: the next key just dismisses it, back to the
    // panels — even q, which quits on the press after.
    if app.splash_preview {
        app.splash_preview = false;
        return;
    }

    // Panel focus: navigation + action keymap.
    match key.code {
        KeyCode::Char('q') => app.should_quit = true,
        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            app.should_quit = true
        }
        KeyCode::Char('?') => app.overlay = Some(Overlay::Help),
        KeyCode::Char('s') => {
            app.overlay = Some(Overlay::Settings(SettingsView::new(app.settings_selected)))
        }
        // Request a reading right away — the main loop's poll may be up to
        // FOOTER_METRICS_POLL out.
        KeyCode::Char('M') => {
            app.overlay = Some(Overlay::Metrics(MetricsView::new()));
            request_metrics(app, out);
        }
        // Replay the first-run nebula splash, fade-in included.
        KeyCode::Char('N') => {
            app.splash_epoch = std::time::Instant::now();
            app.splash_preview = true;
            app.collapsed = false;
        }
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
        // `h` belongs to the hosts picker (below), so ← alone moves left —
        // the one asymmetry in the hjkl set.
        KeyCode::Left => {
            app.focus = match app.focus {
                Focus::Projects => Focus::Projects,
                Focus::Worktrees => Focus::Projects,
                Focus::Sessions => Focus::Worktrees,
                Focus::Terminal => Focus::Sessions,
            }
        }
        KeyCode::Char('h') => open_hosts_picker(app),
        // Ctrl+→ still reaches the terminal pane (the counterpart of the
        // Ctrl+← escape hatch); must precede the unguarded arm below.
        KeyCode::Right if key.modifiers.contains(KeyModifiers::CONTROL) => {
            app.focus = match app.focus {
                Focus::Projects => Focus::Worktrees,
                Focus::Worktrees => Focus::Sessions,
                Focus::Sessions => Focus::Terminal,
                Focus::Terminal => Focus::Terminal,
            }
        }
        KeyCode::Char('l') | KeyCode::Right => {
            // Stops at Sessions: entering the terminal pane means the user
            // chose a session, which is Enter's job — plain focus movement
            // never crosses into the pane (Tab/Ctrl+→ do).
            app.focus = match app.focus {
                Focus::Projects => Focus::Worktrees,
                Focus::Worktrees => Focus::Sessions,
                Focus::Sessions => Focus::Sessions,
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
                Some(ProjectRow::Divider { project, before }) => {
                    let id = app.tree.projects[project].id.clone();
                    open_prompt(app, PromptKind::DividerLabel { id, before });
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
        // First run (or an empty workspace): with no visible projects every
        // panel is empty and the splash is up — `n` creates a project no
        // matter which panel has focus.
        KeyCode::Char('n') if !app.tree.has_visible_projects() => {
            open_prompt(app, PromptKind::AddProject)
        }
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
            Focus::Sessions => match app.selected_session_row() {
                Some(SessionRow::Agent(a)) => {
                    open_prompt(app, PromptKind::RenameAgent { id: a.id })
                }
                Some(SessionRow::Terminal(t)) => {
                    open_prompt(app, PromptKind::RenameTerminal { id: t.id })
                }
                None => {}
            },
            Focus::Projects => {
                if let Some(ProjectRow::Divider { project, before }) = app.selected_project_row() {
                    let id = app.tree.projects[project].id.clone();
                    open_prompt(app, PromptKind::DividerLabel { id, before });
                }
            }
            _ => {}
        },
        KeyCode::Char('a') => {
            if app.focus == Focus::Sessions {
                match app.selected_session_row() {
                    Some(SessionRow::Agent(a)) if !a.archived => {
                        archive_agent(app, a.id, out);
                    }
                    Some(SessionRow::Terminal(_)) => {
                        app.flash = Some("terminals can't be archived — d closes them".into());
                    }
                    _ => {}
                }
            }
        }
        KeyCode::Char('p') => match app.focus {
            Focus::Sessions => {
                if let Some(SessionRow::Terminal(_)) = app.selected_session_row() {
                    app.flash = Some("terminals can't be pinned".into());
                } else if let Some(a) = app.selected_session() {
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
            Focus::Worktrees => {
                // Selection follow across the regroup happens on the upsert
                // (the handler re-finds the selected worktree by id).
                if let Some(w) = app.selected_worktree() {
                    let (id, pinned) = (w.id.clone(), !w.pinned);
                    let req_id = app.alloc_req_id(PendingIntent::None);
                    out.push(ClientRequest::SetWorktreePinned { req_id, id, pinned });
                }
            }
            _ => {}
        },
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
                toggle_archived(app, out);
            }
        }
        // Workspace switcher: pick which workspace is open. The focus
        // guard keeps `w` out of an unlocked terminal pane — but under the
        // splash there is no pane on screen, so it always opens there.
        KeyCode::Char('w') => {
            if app.focus != Focus::Terminal || app.splash_showing() {
                open_workspace_picker(app);
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
                        let (id, present) = (p.id.clone(), !p.divider_after);
                        let req_id = app.alloc_req_id(PendingIntent::None);
                        out.push(ClientRequest::SetProjectDivider {
                            req_id,
                            id,
                            before: false,
                            present,
                            label: None,
                        });
                    }
                    Some(ProjectRow::Divider { project, before }) => {
                        remove_divider(app, project, before, out)
                    }
                    None => {}
                }
            }
        }
        // Backspace is what a Mac "delete" key actually sends.
        KeyCode::Char('d') | KeyCode::Delete | KeyCode::Backspace => {
            match (app.focus, app.selected_project_row()) {
                // Dividers are cheap to recreate — no confirmation dance.
                (Focus::Projects, Some(ProjectRow::Divider { project, before })) => {
                    remove_divider(app, project, before, out)
                }
                _ => open_delete_confirm(app),
            }
        }
        // Shift+D: delete EVERY row of the focused panel (behind a confirm
        // that lists the casualties).
        KeyCode::Char('D') => open_delete_all_confirm(app),
        KeyCode::Char('m') => open_context_menu_for_selection(app),
        KeyCode::Char('g') => open_diff_view(app),
        // `o` opens (adds) a project from ANY panel — unlike `n` it never
        // changes meaning with focus, matching the "open a repo" instinct.
        KeyCode::Char('o') => open_prompt(app, PromptKind::AddProject),
        KeyCode::Char('f') => open_file_finder(app),
        KeyCode::Char('F') => open_grep_view(app),
        // `e` for not(e)s.
        KeyCode::Char('e') => open_note_view(app),
        KeyCode::Char('b') => open_tree_browser(app),
        // t/T: new shell terminal, spawned in the worktree's directory.
        // (Cmd+T never reaches a TUI — the emulator opens its own tab.)
        KeyCode::Char('t') | KeyCode::Char('T') => create_terminal_for_context(app, out),
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
        // Starts at "~/" with the home listing already showing, so the
        // browser is one ↓ away; typing a leading '/' or '~' replaces the
        // prefill (see the Char arm), and Ctrl+u clears it.
        PromptKind::AddProject => (
            "Add project".to_string(),
            "path to a git repository".to_string(),
            if std::env::var_os("HOME").is_some() {
                "~/".to_string()
            } else {
                String::new()
            },
        ),
        PromptKind::DividerLabel { id, before } => {
            let current = app
                .tree
                .projects
                .iter()
                .find(|p| &p.id == id)
                .and_then(|p| {
                    if *before {
                        p.divider_before_label.clone()
                    } else {
                        p.divider_label.clone()
                    }
                })
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
        PromptKind::NewAgent { model, effort, .. } => {
            // Surface the resolved launch options so Enter-with-defaults is
            // visibly what it is; plain "New agent" means CLI defaults.
            let opts: Vec<&str> = model
                .as_deref()
                .into_iter()
                .chain(effort.as_deref())
                .collect();
            let title = if opts.is_empty() {
                "New agent".to_string()
            } else {
                format!("New agent ({})", opts.join(" · "))
            };
            (
                title,
                format!("name (empty = {})", app.default_session_name("agent")),
                String::new(),
            )
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
        PromptKind::NewWorkspace => (
            "New workspace".to_string(),
            "name".to_string(),
            String::new(),
        ),
        PromptKind::RenameWorkspace { id } => {
            let current = app
                .tree
                .workspaces
                .iter()
                .find(|w| &w.id == id)
                .map(|w| w.name.clone())
                .unwrap_or_default();
            ("Rename workspace".to_string(), "name".to_string(), current)
        }
    };
    app.overlay = Some(Overlay::Prompt(PromptDialog::new(
        title, label, input, kind,
    )));
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
    let head = crate::git_diff::head_oid(&path);
    let head_ok = head.is_some();
    let mut view = DiffView::new(path, branch, files, head_ok);
    view.head_key = head.unwrap_or_default();
    view.files_width = app.diff_files_width;
    restore_reviewed_marks(&mut view);
    crate::git_diff::load_selected_diff(&mut view);
    app.overlay = Some(Overlay::Diff(view));
}

/// Restore the worktree's reviewed ✓ marks into `view.reviewed`, dropping
/// any that no longer apply: `load_marks` already returns nothing when HEAD
/// moved (a commit resets the whole worktree), and a mark whose file left
/// the change list or whose diff text changed since it was approved is
/// pruned here — then the pruned set is written back. Restored marks sink
/// to the bottom, so the modal opens on the first unreviewed file.
fn restore_reviewed_marks(view: &mut DiffView) {
    let stored = crate::review::load_marks(&view.root, &view.head_key);
    if stored.is_empty() {
        return;
    }
    view.reviewed = view
        .files
        .iter()
        .filter_map(|file| {
            let mark = *stored.get(&file.path)?;
            let diff = crate::git_diff::diff_for(&view.root, file, view.head_ok);
            (crate::review::fingerprint(&diff) == mark).then(|| (file.path.clone(), mark))
        })
        .collect();
    if view.reviewed.len() != stored.len() {
        crate::review::store_marks(&view.root, &view.head_key, &view.reviewed);
    }
    view.recompute_matches();
}

/// Fuzzy file finder over every tracked + untracked file of the selected
/// worktree (`f`). Same shell as `open_diff_view`: flash instead of opening
/// when there's no worktree, the path is gone, or git fails.
/// Open the note modal (`o`): the project's own notes from the Projects
/// panel, the selected worktree's notes elsewhere (falling back to the
/// project when it has no worktrees yet).
fn open_note_view(app: &mut App) {
    let owner = if app.focus == Focus::Projects {
        app.selected_project()
            .map(|p| NoteOwner::Project(p.id.clone()))
    } else {
        app.selected_worktree()
            .map(|w| NoteOwner::Worktree(w.id.clone()))
            .or_else(|| {
                app.selected_project()
                    .map(|p| NoteOwner::Project(p.id.clone()))
            })
    };
    let Some(owner) = owner else {
        app.flash = Some("select a project or worktree first".into());
        return;
    };
    open_notes_for_owner(app, owner);
}

fn open_notes_for_owner(app: &mut App, owner: NoteOwner) {
    let context = match &owner {
        NoteOwner::Project(id) => {
            let Some(project) = app.tree.projects.iter().find(|p| &p.id == id) else {
                return;
            };
            project.name.clone()
        }
        NoteOwner::Worktree(id) => {
            let Some(worktree) = app.tree.worktrees.iter().find(|w| &w.id == id) else {
                return;
            };
            let project = app
                .tree
                .projects
                .iter()
                .find(|p| p.id == worktree.project_id);
            format!(
                "{}/{}",
                project.map(|p| p.name.as_str()).unwrap_or("?"),
                worktree.branch
            )
        }
    };
    app.overlay = Some(Overlay::Notes(NoteView::new(owner, context)));
}

/// `h`: destinations remembered by `nebula ssh`, newest first. Opens even
/// when empty — the modal's hint is how the feature introduces itself.
fn open_hosts_picker(app: &mut App) {
    app.overlay = Some(Overlay::Hosts(crate::app::HostsView::new(
        crate::hosts::load(),
    )));
}

fn open_file_finder(app: &mut App) {
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
    let files = match crate::git_diff::list_files(&path) {
        Ok(files) => files,
        Err(msg) => {
            app.flash = Some(msg);
            return;
        }
    };
    if files.is_empty() {
        app.flash = Some(format!("no files in {branch}"));
        return;
    }
    let editor = crate::config::Config::load().editor_command();
    app.overlay = Some(Overlay::Files(FileFinder::new(path, branch, editor, files)));
}

/// Tree browser (`b`): full file tree of the selected worktree with a
/// content preview, filterable by file name. Same shell as `open_diff_view`:
/// flash instead of opening when there's no worktree, the path is gone, or
/// git fails.
fn open_tree_browser(app: &mut App) {
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
    let files = match crate::git_diff::list_files(&path) {
        Ok(files) => files,
        Err(msg) => {
            app.flash = Some(msg);
            return;
        }
    };
    if files.is_empty() {
        app.flash = Some(format!("no files in {branch}"));
        return;
    }
    let editor = crate::config::Config::load().editor_command();
    app.overlay = Some(Overlay::Tree(TreeBrowser::new(path, branch, editor, files)));
}

/// Find-in-files (`F`): live `git grep` over the selected worktree; Enter
/// on a hit opens it in the editor modal. Same shell as `open_diff_view`.
fn open_grep_view(app: &mut App) {
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
    let editor = crate::config::Config::load().editor_command();
    app.overlay = Some(Overlay::Grep(GrepView::new(path, branch, editor)));
}

/// Enter on a grep hit: spawn the editor at `path:line` inside the modal
/// terminal. The grep overlay stays open underneath, so quitting the editor
/// lands back on the results.
fn open_selected_hit_in_editor(app: &mut App) {
    let Some(Overlay::Grep(view)) = &app.overlay else {
        return;
    };
    let Some(hit) = view.selected_hit() else {
        return;
    };
    let (root, editor) = (view.root.clone(), view.editor.clone());
    let (path, line) = (hit.path.clone(), hit.line);
    let Some(tx) = app.vim_tx.clone() else {
        return; // main loop not running (unit tests without a channel)
    };
    // Size guess from the last-drawn body; the post-draw sync corrects it.
    let (cols, rows) = vim_size_guess(app);
    app.vim_generation += 1;
    match VimTerm::spawn_editor(
        &editor,
        &root,
        &path,
        line,
        cols,
        rows,
        app.vim_generation,
        tx,
    ) {
        Ok(vim) => app.vim = Some(vim),
        Err(msg) => app.flash = Some(msg),
    }
}

/// Enter on a file-finder row: spawn the editor at the file's first line
/// inside the modal terminal. The finder stays open underneath, so quitting
/// the editor lands back on the results.
fn open_selected_file_in_editor(app: &mut App) {
    let Some(Overlay::Files(finder)) = &app.overlay else {
        return;
    };
    let Some(path) = finder.selected_path().map(str::to_string) else {
        return;
    };
    let (root, editor) = (finder.root.clone(), finder.editor.clone());
    let Some(tx) = app.vim_tx.clone() else {
        return; // main loop not running (unit tests without a channel)
    };
    // Size guess from the last-drawn body; the post-draw sync corrects it.
    let (cols, rows) = vim_size_guess(app);
    app.vim_generation += 1;
    match VimTerm::spawn_editor(&editor, &root, &path, 1, cols, rows, app.vim_generation, tx) {
        Ok(vim) => app.vim = Some(vim),
        Err(msg) => app.flash = Some(msg),
    }
}

/// Enter on a tree-browser file row: spawn the editor embedded in the
/// preview pane — the pane becomes vim, keys flow to it, and quitting lands
/// back on the tree with the preview reloaded.
fn open_selected_tree_file_in_editor(app: &mut App) {
    let Some(Overlay::Tree(view)) = &app.overlay else {
        return;
    };
    let Some(path) = view
        .selected_node()
        .filter(|n| !n.is_dir)
        .map(|n| n.path.clone())
    else {
        return;
    };
    let (root, editor) = (view.root.clone(), view.editor.clone());
    // Size from the last-drawn preview pane; the post-draw sync corrects it.
    let preview = view.preview_area;
    let Some(tx) = app.vim_tx.clone() else {
        return; // main loop not running (unit tests without a channel)
    };
    let (cols, rows) = if preview.width >= 2 && preview.height >= 2 {
        (preview.width, preview.height)
    } else {
        vim_size_guess(app) // never drawn yet
    };
    app.vim_generation += 1;
    match VimTerm::spawn_editor(&editor, &root, &path, 1, cols, rows, app.vim_generation, tx) {
        Ok(mut vim) => {
            vim.embedded = true;
            app.vim = Some(vim);
        }
        Err(msg) => app.flash = Some(msg),
    }
}

/// Expected inner size of the editor modal before its first draw, derived
/// from the last-drawn body rect (`VIM_MODAL_PCT` of the frame, minus the
/// border). `sync_vim_size` trues it up after the real draw.
fn vim_size_guess(app: &App) -> (u16, u16) {
    let frame_w = app.body_area.width;
    let frame_h = app.body_area.height + 1; // + footer row
    let cols = (frame_w * ui::VIM_MODAL_PCT.0 / 100)
        .saturating_sub(2)
        .max(2);
    let rows = (frame_h * ui::VIM_MODAL_PCT.1 / 100)
        .saturating_sub(2)
        .max(2);
    (cols, rows)
}

/// ⌥click on a file path in the terminal pane: resolve it against the
/// attached session's worktree and open it in the editor modal at the
/// referenced line.
fn open_file_link(app: &mut App, path: &str, line: Option<u64>) {
    let Some(root) = attached_worktree_root(app) else {
        app.flash = Some("no worktree for this session".into());
        return;
    };
    let Some(file) = resolve_file_link(&root, path) else {
        app.flash = Some(format!("file not found: {path}"));
        return;
    };
    let editor = crate::config::Config::load().editor_command();
    let Some(tx) = app.vim_tx.clone() else {
        return; // main loop not running (unit tests without a channel)
    };
    let (cols, rows) = vim_size_guess(app);
    app.vim_generation += 1;
    match VimTerm::spawn_editor(
        &editor,
        &root,
        &file,
        line.unwrap_or(1),
        cols,
        rows,
        app.vim_generation,
        tx,
    ) {
        Ok(vim) => app.vim = Some(vim),
        Err(msg) => app.flash = Some(msg),
    }
}

/// Worktree root of the attached session; falls back to the selected
/// worktree when the attachment isn't an agent (or isn't in the tree yet).
fn attached_worktree_root(app: &App) -> Option<std::path::PathBuf> {
    if let Some(SessionRef::Agent(id)) = app.term.as_ref().map(|t| &t.sref) {
        let root = app
            .tree
            .agents
            .iter()
            .find(|a| &a.id == id)
            .and_then(|a| app.tree.worktrees.iter().find(|w| w.id == a.worktree_id))
            .map(|w| w.path.clone());
        if root.is_some() {
            return root;
        }
    }
    app.selected_worktree().map(|w| w.path.clone())
}

/// Resolve a clicked path against the worktree: expand `~/`, try it as
/// printed, then with the git-diff `a/`/`b/` prefix stripped. Returns the
/// argument to hand the editor — relative paths stay relative, since the
/// editor runs with the worktree as cwd.
fn resolve_file_link(root: &std::path::Path, path: &str) -> Option<String> {
    let mut candidates = vec![path.to_string()];
    for prefix in ["a/", "b/"] {
        if let Some(rest) = path.strip_prefix(prefix) {
            candidates.push(rest.to_string());
        }
    }
    for cand in candidates {
        let full = if let Some(rest) = cand.strip_prefix("~/") {
            let home = std::env::var_os("HOME")?;
            std::path::PathBuf::from(home).join(rest)
        } else {
            // join() with an absolute candidate yields the candidate.
            root.join(&cand)
        };
        if full.is_file() {
            return Some(if cand.starts_with("~/") {
                full.to_string_lossy().into_owned()
            } else {
                cand
            });
        }
    }
    None
}

/// Keys while the editor modal is open: Ctrl+Q force-closes (the terminal
/// lock's hatch — vim owns Esc), everything else forwards in the legacy
/// dialect (vim never pushes kitty flags).
fn handle_vim_key(app: &mut App, key: KeyEvent) {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    if ctrl && key.code == KeyCode::Char('q') {
        if let Some(vim) = &mut app.vim {
            vim.kill();
        }
        close_vim(app);
        return;
    }
    if let Some(vim) = &mut app.vim {
        if let Some(data) = keys::encode_key(&key, 0) {
            vim.input(&data);
        }
    }
}

/// Archive is cheap to undo (u), so it skips the confirm dialog.
fn archive_agent(app: &mut App, id: AgentId, out: &mut Vec<ClientRequest>) {
    detach_if_attached(app, &SessionRef::Agent(id.clone()), out);
    let req_id = app.alloc_req_id(PendingIntent::None);
    out.push(ClientRequest::ArchiveAgent { req_id, id });
}

/// Expand/collapse the ARCHIVED group (A key, header click, context menu).
/// Collapsing while the cursor sits on an archived row re-lands it on a
/// surviving row and previews it, same as any other regroup.
fn toggle_archived(app: &mut App, out: &mut Vec<ClientRequest>) {
    let before = selection_snapshot(app);
    app.show_archived = !app.show_archived;
    reconcile_selection(app, before, out);
}

/// Shift+T: create a shell terminal whose pwd is the selection's checkout —
/// the selected worktree, or the project's main checkout (root) when the
/// Projects panel has focus. The daemon names it (`term-N`) and the Ack
/// attaches it, so one keypress lands in a ready shell.
fn create_terminal_for_context(app: &mut App, out: &mut Vec<ClientRequest>) {
    let worktree = match app.focus {
        Focus::Projects => app.selected_project().and_then(|p| {
            app.tree
                .worktrees
                .iter()
                .find(|w| w.project_id == p.id && w.is_main)
                .map(|w| w.id.clone())
        }),
        _ => app.selected_worktree().map(|w| w.id.clone()),
    };
    let Some(worktree) = worktree else {
        app.flash = Some("select a project or worktree first".into());
        return;
    };
    let req_id = app.alloc_req_id(PendingIntent::AttachCreated);
    out.push(ClientRequest::CreateTerminal {
        req_id,
        worktree,
        name: None,
    });
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
                    .count()
                    + app.visible_terminals().len();
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
        Focus::Sessions => match app.selected_session_row() {
            Some(SessionRow::Agent(a)) => {
                app.overlay = Some(Overlay::Confirm(ConfirmDialog {
                    title: "Delete agent".into(),
                    message: format!(
                        "Delete agent '{}'? Its session and history go away.",
                        a.name
                    ),
                    action: PendingAction::DeleteAgent(a.id),
                }));
            }
            Some(SessionRow::Terminal(t)) => {
                app.overlay = Some(Overlay::Confirm(ConfirmDialog {
                    title: "Close terminal".into(),
                    message: format!("Close terminal '{}'? Its shell is killed.", t.name),
                    action: PendingAction::CloseTerminal(t.id),
                }));
            }
            None => {}
        },
        Focus::Terminal => {}
    }
}

/// Cap on itemized rows in the bulk-delete confirm; the rest collapse into
/// an "and N more" line so the dialog always fits on screen.
const BULK_CONFIRM_MAX_LISTED: usize = 8;

/// The itemized body of a bulk-delete confirm: one bullet per doomed row.
fn bulk_confirm_listing(names: &[String]) -> String {
    let mut lines: Vec<String> = names
        .iter()
        .take(BULK_CONFIRM_MAX_LISTED)
        .map(|n| format!("  • {n}"))
        .collect();
    if names.len() > BULK_CONFIRM_MAX_LISTED {
        lines.push(format!(
            "  … and {} more",
            names.len() - BULK_CONFIRM_MAX_LISTED
        ));
    }
    lines.join("\n")
}

/// Shift+D: confirm deleting EVERY row of the focused panel — all worktrees
/// of the selected project, or all sessions the panel shows. The dialog
/// itemizes the casualties so the blast radius is unmistakable.
fn open_delete_all_confirm(app: &mut App) {
    match app.focus {
        Focus::Worktrees => {
            let doomed: Vec<&nebula_core::Worktree> = app
                .visible_worktrees()
                .into_iter()
                .filter(|w| !w.is_main)
                .collect();
            if doomed.is_empty() {
                app.flash = Some("no deletable worktrees (the main checkout stays)".into());
                return;
            }
            let killed = app
                .tree
                .agents
                .iter()
                .filter(|a| !a.archived && doomed.iter().any(|w| w.id == a.worktree_id))
                .count()
                + app
                    .tree
                    .terminals
                    .iter()
                    .filter(|t| doomed.iter().any(|w| w.id == t.worktree_id))
                    .count();
            let names: Vec<String> = doomed.iter().map(|w| w.branch.clone()).collect();
            let ids: Vec<WorktreeId> = doomed.iter().map(|w| w.id.clone()).collect();
            app.overlay = Some(Overlay::Confirm(ConfirmDialog {
                title: format!("Delete ALL {} worktree(s)", ids.len()),
                message: format!(
                    "Delete these {} worktree(s) from disk? {killed} session(s) will be killed.\n{}\nThe main checkout stays.",
                    ids.len(),
                    bulk_confirm_listing(&names),
                ),
                action: PendingAction::DeleteAllWorktrees(ids),
            }));
        }
        Focus::Sessions => {
            // What the panel shows is what dies — terminals too, archived
            // rows only when the archived toggle has them visible.
            let doomed = app.visible_session_rows();
            if doomed.is_empty() {
                app.flash = Some("no sessions to delete".into());
                return;
            }
            let names: Vec<String> = doomed.iter().map(|r| r.name().to_string()).collect();
            let mut agents = Vec::new();
            let mut terminals = Vec::new();
            for row in doomed {
                match row {
                    SessionRow::Agent(a) => agents.push(a.id),
                    SessionRow::Terminal(t) => terminals.push(t.id),
                }
            }
            app.overlay = Some(Overlay::Confirm(ConfirmDialog {
                title: format!("Delete ALL {} session(s)", names.len()),
                message: format!(
                    "Delete these {} session(s)? Their history goes away.\n{}",
                    names.len(),
                    bulk_confirm_listing(&names),
                ),
                action: PendingAction::DeleteAllSessions { agents, terminals },
            }));
        }
        Focus::Projects | Focus::Terminal => {}
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

fn menu_items_for_terminal(t: &nebula_core::TerminalTab) -> Vec<MenuItem> {
    vec![
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
    ]
}

fn divider_menu_item(p: &nebula_core::Project) -> MenuItem {
    MenuItem {
        label: if p.divider_after {
            "Remove divider below"
        } else {
            "Add divider below"
        }
        .into(),
        action: MenuAction::SetProjectDivider {
            id: p.id.clone(),
            before: false,
            present: !p.divider_after,
        },
        destructive: false,
    }
}

/// Menu for a selected divider row.
fn divider_row_menu(id: nebula_core::ProjectId, before: bool) -> Vec<MenuItem> {
    vec![
        MenuItem {
            label: "Edit label".into(),
            action: MenuAction::LabelDivider(id.clone(), before),
            destructive: false,
        },
        MenuItem {
            label: "Remove divider".into(),
            action: MenuAction::SetProjectDivider {
                id,
                before,
                present: false,
            },
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
        parent: None,
    }));
}

/// Step 1 of new-session creation: pick which CLI the session runs — or a
/// plain shell terminal. An agent kind chains into the name prompt via
/// `MenuAction::NewAgentOfKind`; the terminal is created immediately.
/// Claude/Codex rows expand (→) into model and effort submenus; Enter
/// anywhere takes the configured defaults for whatever wasn't drilled into.
fn open_new_agent_picker(app: &mut App, worktree: WorktreeId) {
    let kind_row = |label: &str, kind: AgentKind| MenuItem {
        label: label.into(),
        action: MenuAction::NewAgentOfKind {
            worktree: worktree.clone(),
            kind,
            model: None,
            effort: None,
        },
        destructive: false,
    };
    app.overlay = Some(Overlay::Menu(ContextMenu {
        title: Some("New session".into()),
        items: vec![
            kind_row("Claude", AgentKind::Claude),
            kind_row("Codex", AgentKind::Codex),
            kind_row("Cursor", AgentKind::Cursor),
            MenuItem {
                label: "Terminal (shell)".into(),
                action: MenuAction::NewTerminal(worktree),
                destructive: false,
            },
        ],
        at: None,
        hover: 0,
        area: ratatui::layout::Rect::default(),
        parent: None,
    }));
}

/// Build the submenu a menu row expands into: the model list for a
/// new-session kind row, or the effort list for a model row. Rows carry the
/// full choice so Enter works the same at any depth; the row matching the
/// configured default starts highlighted.
fn build_submenu(item: &MenuItem) -> Option<ContextMenu> {
    let sub = item.action.submenu()?;
    let MenuAction::NewAgentOfKind {
        worktree,
        kind,
        model,
        ..
    } = &item.action
    else {
        return None;
    };
    let cfg = crate::config::Config::load();
    let (title, choices, configured) = match sub {
        SubmenuKind::Models => (
            format!("{} model", item.label),
            crate::config::model_choices(*kind),
            cfg.default_model(*kind),
        ),
        SubmenuKind::Efforts => (
            format!("{} effort", kind_label(*kind)),
            crate::config::effort_choices(*kind),
            cfg.default_effort(*kind),
        ),
    };
    let configured = configured.unwrap_or_else(|| "default".into());
    let items: Vec<MenuItem> = choices
        .iter()
        .map(|choice| MenuItem {
            label: if *choice == configured {
                format!("{choice} ✓")
            } else {
                (*choice).to_string()
            },
            action: MenuAction::NewAgentOfKind {
                worktree: worktree.clone(),
                kind: *kind,
                model: match sub {
                    SubmenuKind::Models => Some((*choice).to_string()),
                    SubmenuKind::Efforts => model.clone(),
                },
                effort: match sub {
                    SubmenuKind::Models => None,
                    SubmenuKind::Efforts => Some((*choice).to_string()),
                },
            },
            destructive: false,
        })
        .collect();
    let hover = choices.iter().position(|c| *c == configured).unwrap_or(0);
    Some(ContextMenu {
        title: Some(title),
        items,
        at: None,
        hover,
        area: ratatui::layout::Rect::default(),
        parent: None,
    })
}

fn kind_label(kind: AgentKind) -> &'static str {
    match kind {
        AgentKind::Claude => "Claude",
        AgentKind::Codex => "Codex",
        AgentKind::Cursor => "Cursor",
    }
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
        parent: None,
    }));
}

/// Workspace switcher (`w`): pick which workspace is open. The active one is
/// checked and starts highlighted; Enter asks the daemon to open the pick
/// (the switch lands via ActiveWorkspaceChanged, so every client follows).
/// Management verbs are keys with footer hints, the notes-modal pattern:
/// n creates (and opens) a workspace, r renames the hovered one, d deletes
/// it. The list refreshes in place as workspace deltas arrive.
fn open_workspace_picker(app: &mut App) {
    let active = &app.tree.active_workspace;
    let items: Vec<MenuItem> = app
        .tree
        .workspaces
        .iter()
        .map(|w| {
            let projects = app
                .tree
                .projects
                .iter()
                .filter(|p| p.workspace_id == w.id)
                .count();
            MenuItem {
                label: format!(
                    "{}{}  ({projects})",
                    w.name,
                    if &w.id == active { " ✓" } else { "" }
                ),
                action: MenuAction::OpenWorkspace(w.id.clone()),
                destructive: false,
            }
        })
        .collect();
    if items.is_empty() {
        // Never expected — every install has the 'default' workspace — but
        // an empty menu would render as a dead overlay.
        app.flash = Some("no workspaces — `nebula workspace add <name>` creates one".into());
        return;
    }
    let hover = app
        .tree
        .workspaces
        .iter()
        .position(|w| &w.id == active)
        .unwrap_or(0);
    app.overlay = Some(Overlay::Menu(ContextMenu {
        title: Some("Workspace".into()),
        items,
        at: None,
        hover,
        area: ratatui::layout::Rect::default(),
        parent: None,
    }));
}

/// Rebuild an open workspace switcher after the workspace list (or the ✓
/// marker) changed under it, keeping the cursor row. The notes modal gets
/// this for free by reading the tree at draw time; the menu's rows are
/// snapshots, so refresh them here.
fn refresh_workspace_picker(app: &mut App) {
    let Some(Overlay::Menu(menu)) = &app.overlay else {
        return;
    };
    if !menu.is_workspace_picker() {
        return;
    }
    let hover = menu.hover;
    if app.tree.workspaces.is_empty() {
        app.overlay = None; // nothing left to list
        return;
    }
    open_workspace_picker(app);
    if let Some(Overlay::Menu(menu)) = &mut app.overlay {
        menu.hover = hover.min(menu.items.len().saturating_sub(1));
    }
}

fn open_context_menu_for_selection(app: &mut App) {
    // Keyboard-invoked menu: anchor near the selected row's panel.
    let at = (30, 4);
    match app.focus {
        Focus::Projects => {
            if let Some(ProjectRow::Divider { project, before }) = app.selected_project_row() {
                let id = app.tree.projects[project].id.clone();
                open_menu(app, divider_row_menu(id, before), at);
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
                items.push(MenuItem {
                    label: "Notes".into(),
                    action: MenuAction::OpenNotes(NoteOwner::Project(p.id.clone())),
                    destructive: false,
                });
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
                    MenuItem {
                        label: "Notes".into(),
                        action: MenuAction::OpenNotes(NoteOwner::Worktree(w.id.clone())),
                        destructive: false,
                    },
                    MenuItem {
                        label: if w.pinned { "Unpin" } else { "Pin" }.into(),
                        action: MenuAction::SetWorktreePinned(w.id.clone(), !w.pinned),
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
        Focus::Sessions => match app.selected_session_row() {
            Some(SessionRow::Agent(a)) => open_menu(app, menu_items_for_session(&a), at),
            Some(SessionRow::Terminal(t)) => open_menu(app, menu_items_for_terminal(&t), at),
            None => {}
        },
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
        Overlay::Metrics(view) => match key.code {
            KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('M') => app.overlay = None,
            KeyCode::Char('j') | KeyCode::Down => {
                view.selected = (view.selected + 1).min(view.rows.len().saturating_sub(1));
            }
            KeyCode::Char('k') | KeyCode::Up => view.selected = view.selected.saturating_sub(1),
            KeyCode::Enter => {
                // Nebula's own rows (daemon / this UI) carry no session.
                if let Some(Some(sref)) = view.rows.get(view.selected).cloned() {
                    app.overlay = None;
                    open_session(app, sref, out);
                }
            }
            _ => {}
        },
        Overlay::Hosts(view) => {
            // Typing a new destination (`a`): the input owns printable keys.
            if let Some(input) = &mut view.input {
                match key.code {
                    KeyCode::Esc => view.input = None,
                    KeyCode::Enter => {
                        let entry = crate::hosts::parse_destination(input);
                        view.input = None;
                        // Nothing typed = cancel; otherwise connect exactly
                        // like `nebula ssh host [dir]` would.
                        if let Some(entry) = entry {
                            app.overlay = None;
                            app.pending_ssh = Some(entry);
                            app.should_quit = true;
                        }
                    }
                    // Everything else is the line editor's: arrows,
                    // ⌥←/⌥→ by word, the readline chords (text_input).
                    _ => {
                        input.handle_key(&key);
                    }
                }
                return;
            }
            match key.code {
                KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('h') => app.overlay = None,
                KeyCode::Char('j') | KeyCode::Down => {
                    view.selected = (view.selected + 1).min(view.hosts.len().saturating_sub(1));
                }
                KeyCode::Char('k') | KeyCode::Up => view.selected = view.selected.saturating_sub(1),
                // A destination the list doesn't have yet — typed here so an
                // open nebula never needs a shell for `nebula ssh`.
                KeyCode::Char('a') | KeyCode::Char('n') => view.input = Some(TextInput::new()),
                // Enter hands off: quit the TUI, then the binary execs a
                // fresh `nebula ssh` at the entry (the daemon and its
                // sessions stay up).
                KeyCode::Enter => {
                    if let Some(entry) = view.hosts.get(view.selected).cloned() {
                        app.overlay = None;
                        app.pending_ssh = Some(entry);
                        app.should_quit = true;
                    }
                }
                // Forget the entry — no confirm, the next `nebula ssh` to it
                // just re-adds it.
                KeyCode::Char('d') | KeyCode::Char('x') | KeyCode::Backspace | KeyCode::Delete => {
                    if view.selected < view.hosts.len() {
                        let entry = view.hosts.remove(view.selected);
                        view.selected = view.selected.min(view.hosts.len().saturating_sub(1));
                        crate::hosts::remove(&entry);
                    }
                }
                _ => {}
            }
        }
        Overlay::Menu(menu) => match key.code {
            // Esc in a submenu backs out one level; at the top it closes.
            KeyCode::Esc => match menu.parent.take() {
                Some(parent) => *menu = *parent,
                None => app.overlay = None,
            },
            KeyCode::Char('j') | KeyCode::Down => {
                menu.hover = (menu.hover + 1).min(menu.items.len() - 1)
            }
            KeyCode::Char('k') | KeyCode::Up => menu.hover = menu.hover.saturating_sub(1),
            // → expands a row marked ▸ into its submenu; ← returns.
            KeyCode::Char('l') | KeyCode::Right => {
                if let Some(mut sub) = build_submenu(&menu.items[menu.hover]) {
                    sub.parent = Some(Box::new(menu.clone()));
                    *menu = sub;
                }
            }
            KeyCode::Char('h') | KeyCode::Left => {
                if let Some(parent) = menu.parent.take() {
                    *menu = *parent;
                }
            }
            // Workspace-switcher verbs (footer-hinted, the notes-modal
            // pattern): n creates a workspace (opened on Ack), r renames
            // the hovered one, d deletes it — no confirm, the daemon only
            // deletes empty workspaces so a refusal just flashes.
            KeyCode::Char('n') if menu.is_workspace_picker() => {
                open_prompt(app, PromptKind::NewWorkspace);
            }
            KeyCode::Char('r') if menu.is_workspace_picker() => {
                if let Some(id) = menu.hovered_workspace() {
                    open_prompt(app, PromptKind::RenameWorkspace { id });
                }
            }
            KeyCode::Char('d') if menu.is_workspace_picker() => {
                if let Some(id) = menu.hovered_workspace() {
                    let req_id = app.alloc_req_id(PendingIntent::None);
                    out.push(ClientRequest::RemoveWorkspace { req_id, id });
                }
            }
            KeyCode::Enter => {
                let action = menu.items[menu.hover].action.clone();
                app.overlay = None;
                run_menu_action(app, action, out);
            }
            _ => {}
        },
        Overlay::Prompt(prompt) => match key.code {
            KeyCode::Esc => {
                // Abandoning a Claude name prompt can leave the warm slot
                // holding the submenu's off-default spec (its prewarm fired
                // on kind-pick); put the standing default spec back. Same
                // spec = daemon-side no-op.
                let restore = match &prompt.kind {
                    PromptKind::NewAgent {
                        worktree,
                        kind: AgentKind::Claude,
                        ..
                    } => Some(worktree.clone()),
                    _ => None,
                };
                app.overlay = None;
                if let Some(worktree) = restore {
                    out.push(default_claude_prewarm(worktree));
                }
            }
            KeyCode::Enter => {
                // Enter on a highlighted listing row adds that directory;
                // on the input row it submits the typed path as before.
                let mut prompt = prompt.clone();
                if let Some(path) = prompt.hovered_path() {
                    prompt.input.set_text(path);
                }
                app.overlay = None;
                submit_prompt(app, prompt, out);
            }
            KeyCode::Tab if prompt.completes_paths() => {
                let home = std::env::var_os("HOME").map(std::path::PathBuf::from);
                let result = crate::completion::complete_path(&prompt.input, home.as_deref());
                if let Some(completed) = result.completed {
                    prompt.input.set_text(completed);
                    prompt.refresh_dirs();
                }
            }
            KeyCode::Down if prompt.completes_paths() => prompt.move_hover(1),
            KeyCode::Up if prompt.completes_paths() => prompt.move_hover(-1),
            // ←/→ stay the path browser's dive/ascend here — the one
            // prompt where they are already spoken for. Caret motion in a
            // path is ⌥←/⌥→ (by segment), Ctrl+B/F, Home/End.
            KeyCode::Right if prompt.completes_paths() => {
                if let Some(i) = prompt.hover {
                    prompt.dive(i);
                }
            }
            KeyCode::Left if prompt.completes_paths() => prompt.ascend(),
            // The untouched "~/" prefill yields to an absolute (or
            // re-typed tilde) path — no clearing required first.
            KeyCode::Char(c)
                if prompt.completes_paths()
                    && prompt.input == "~/"
                    && (c == '/' || c == '~')
                    && !key
                        .modifiers
                        .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                prompt.input.set_text(c.to_string());
                prompt.refresh_dirs();
            }
            // Everything else is the line editor's (see text_input).
            _ => {
                if prompt.input.handle_key(&key).changed() {
                    prompt.refresh_dirs();
                }
            }
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
                // Ctrl+u is the line editor's kill-to-start while something
                // is typed; only with an empty filter does it scroll.
                KeyCode::Char('u') if ctrl && view.filter.is_empty() => view.scroll_by(-half),
                // Ctrl+r toggles the reviewed ✓ on the selected file —
                // nebula-side bookkeeping only, no git state is touched.
                // Reviewed files sink to the bottom; marking advances to the
                // next file and unmarking to the next still-marked file, so
                // held Ctrl+r sweeps either way (see
                // `DiffView::toggle_reviewed`).
                KeyCode::Char('r') if ctrl => {
                    if let Some(changed) = view.toggle_reviewed() {
                        crate::review::store_marks(&view.root, &view.head_key, &view.reviewed);
                        if changed {
                            crate::git_diff::load_selected_diff(view);
                        }
                    }
                }
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
                // Everything else feeds the always-on fuzzy filter, which
                // edits like a terminal line (see text_input).
                _ => {
                    if view.filter.handle_key(&key).changed() && view.apply_filter() {
                        crate::git_diff::load_selected_diff(view);
                    }
                }
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
                // Everything else edits the query like a terminal line
                // (see text_input).
                _ => {
                    if palette.query.handle_key(&key).changed() {
                        palette.apply_filter();
                    }
                }
            }
        }
        Overlay::Files(finder) => {
            let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
            match key.code {
                // Two-stage escape: an active query is cleared before the
                // second Esc closes the finder.
                KeyCode::Esc if !finder.query.is_empty() => {
                    finder.query.clear();
                    finder.apply_filter();
                }
                KeyCode::Esc => app.overlay = None,
                // j/k stay typeable in the query; Ctrl+n/p mirror ↑/↓.
                KeyCode::Down => finder.select(finder.selected as i64 + 1),
                KeyCode::Up => finder.select(finder.selected as i64 - 1),
                KeyCode::Char('n') if ctrl => finder.select(finder.selected as i64 + 1),
                KeyCode::Char('p') if ctrl => finder.select(finder.selected as i64 - 1),
                // Enter opens the selected file in the editor modal; the
                // finder stays open underneath so quitting the editor
                // returns here.
                KeyCode::Enter => open_selected_file_in_editor(app),
                // Ctrl+y copies the selected path (relative to the worktree
                // root) to the clipboard — ready to paste into an agent.
                KeyCode::Char('y') if ctrl => {
                    if let Some(path) = finder.selected_path().map(str::to_string) {
                        app.overlay = None;
                        app.flash = Some(if copy_to_clipboard(&path) {
                            format!("copied {path}")
                        } else {
                            "copy failed (clipboard unavailable)".into()
                        });
                    }
                }
                // Everything else edits the query like a terminal line
                // (see text_input).
                _ => {
                    if finder.query.handle_key(&key).changed() {
                        finder.apply_filter();
                    }
                }
            }
        }
        Overlay::Grep(view) => {
            let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
            match key.code {
                // Two-stage escape: an active query is cleared before the
                // second Esc closes the overlay.
                KeyCode::Esc if !view.query.is_empty() => {
                    view.query.clear();
                    view.run_search();
                }
                KeyCode::Esc => app.overlay = None,
                // j/k stay typeable in the query; Ctrl+n/p mirror ↑/↓.
                KeyCode::Down => view.select(view.selected as i64 + 1),
                KeyCode::Up => view.select(view.selected as i64 - 1),
                KeyCode::Char('n') if ctrl => view.select(view.selected as i64 + 1),
                KeyCode::Char('p') if ctrl => view.select(view.selected as i64 - 1),
                // Enter opens the hit in the editor modal; the overlay stays
                // open underneath so quitting the editor returns here.
                KeyCode::Enter => open_selected_hit_in_editor(app),
                // Everything else edits the query like a terminal line
                // (see text_input).
                _ => {
                    if view.query.handle_key(&key).changed() {
                        view.run_search();
                    }
                }
            }
        }
        Overlay::Tree(view) => {
            let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
            let shift = key.modifiers.contains(KeyModifiers::SHIFT);
            let half = (view.view_height / 2).max(1) as i32;
            let page = view.view_height.max(1) as i32;
            match key.code {
                // Two-stage escape: an active filter is cleared before the
                // second Esc closes the modal.
                KeyCode::Esc if !view.filter.is_empty() => {
                    view.filter.clear();
                    view.apply_filter();
                }
                KeyCode::Esc => app.overlay = None,
                // The preview scrolls on the diff-modal keys: ⇧↑/↓ lines,
                // Ctrl+d/u half pages, PageUp/Down, Home/End.
                KeyCode::Char('d') if ctrl => view.scroll_by(half),
                // Ctrl+u is the line editor's kill-to-start while something
                // is typed; only with an empty filter does it scroll.
                KeyCode::Char('u') if ctrl && view.filter.is_empty() => view.scroll_by(-half),
                KeyCode::Down if shift => view.scroll_by(1),
                KeyCode::Up if shift => view.scroll_by(-1),
                KeyCode::PageDown => view.scroll_by(page),
                KeyCode::PageUp => view.scroll_by(-page),
                KeyCode::Home => view.scroll = 0,
                KeyCode::End => view.scroll = view.max_scroll(),
                // j/k stay typeable in the filter; Ctrl+n/p mirror ↑/↓.
                KeyCode::Down => view.select(view.selected as i64 + 1),
                KeyCode::Up => view.select(view.selected as i64 - 1),
                KeyCode::Char('n') if ctrl => view.select(view.selected as i64 + 1),
                KeyCode::Char('p') if ctrl => view.select(view.selected as i64 - 1),
                KeyCode::Right => view.expand_selected(),
                KeyCode::Left => view.collapse_selected(),
                // Enter folds/unfolds a directory; on a file it opens the
                // editor modal, with the browser staying open underneath.
                KeyCode::Enter => {
                    if view.selected_is_dir() {
                        view.toggle_row(view.selected);
                    } else {
                        open_selected_tree_file_in_editor(app);
                    }
                }
                // Ctrl+y copies the selected path (relative to the worktree
                // root) to the clipboard — ready to paste into an agent.
                KeyCode::Char('y') if ctrl => {
                    if let Some(path) = view.selected_node().map(|n| n.path.clone()) {
                        app.overlay = None;
                        app.flash = Some(if copy_to_clipboard(&path) {
                            format!("copied {path}")
                        } else {
                            "copy failed (clipboard unavailable)".into()
                        });
                    }
                }
                // Everything else feeds the always-on fuzzy filter, which
                // edits like a terminal line (see text_input).
                _ => {
                    if view.filter.handle_key(&key).changed() {
                        view.apply_filter();
                    }
                }
            }
        }
        Overlay::Notes(view) => {
            // The CRUD keys need both the view (cursor/input) and the tree
            // (rows) — resolve the key to a command first, then act, so the
            // overlay borrow never overlaps the request plumbing.
            enum NoteCmd {
                Nothing,
                Close,
                Move(i64),
                StartCreate,
                StartEdit(NoteId, String),
                CancelInput,
                Create(String),
                Update(NoteId, String),
                Toggle(NoteId, bool),
                Delete(NoteId),
            }
            // Row order matches the draw: tree order for this owner.
            let rows: Vec<(NoteId, String, bool)> = app
                .tree
                .notes
                .iter()
                .filter(|t| t.owner == view.owner)
                .map(|t| (t.id.clone(), t.text.clone(), t.done))
                .collect();
            let selected = view.selected.min(rows.len().saturating_sub(1));
            view.selected = selected;
            let cmd = if let Some(input) = &mut view.input {
                match key.code {
                    KeyCode::Esc => NoteCmd::CancelInput,
                    // Nothing typed = cancel; edits and adds both no-op.
                    KeyCode::Enter => match (&input.editing, input.text.trim()) {
                        (_, "") => NoteCmd::CancelInput,
                        (Some(id), text) => NoteCmd::Update(id.clone(), text.to_string()),
                        (None, text) => NoteCmd::Create(text.to_string()),
                    },
                    // Everything else is the line editor's: arrows, ⌥←/⌥→
                    // by word, the readline chords (see text_input).
                    _ => {
                        input.text.handle_key(&key);
                        NoteCmd::Nothing
                    }
                }
            } else {
                match key.code {
                    KeyCode::Esc | KeyCode::Char('q') => NoteCmd::Close,
                    KeyCode::Char('j') | KeyCode::Down => NoteCmd::Move(1),
                    KeyCode::Char('k') | KeyCode::Up => NoteCmd::Move(-1),
                    // `e` mirrors the key that opened the modal; a/n too.
                    // (That costs `e` as an edit key — Enter/r cover it.)
                    KeyCode::Char('e') | KeyCode::Char('a') | KeyCode::Char('n') => {
                        NoteCmd::StartCreate
                    }
                    KeyCode::Enter | KeyCode::Char('r') => {
                        match rows.get(selected) {
                            Some((id, text, _)) => NoteCmd::StartEdit(id.clone(), text.clone()),
                            // Empty list: Enter starts the first note.
                            None => NoteCmd::StartCreate,
                        }
                    }
                    KeyCode::Char(' ') | KeyCode::Char('x') => match rows.get(selected) {
                        Some((id, _, done)) => NoteCmd::Toggle(id.clone(), !done),
                        None => NoteCmd::Nothing,
                    },
                    KeyCode::Char('d') | KeyCode::Backspace | KeyCode::Delete => {
                        match rows.get(selected) {
                            Some((id, _, _)) => NoteCmd::Delete(id.clone()),
                            None => NoteCmd::Nothing,
                        }
                    }
                    _ => NoteCmd::Nothing,
                }
            };
            match cmd {
                NoteCmd::Nothing => {}
                NoteCmd::Close => app.overlay = None,
                NoteCmd::Move(delta) => {
                    let max = rows.len().saturating_sub(1) as i64;
                    view.selected = (selected as i64 + delta).clamp(0, max) as usize;
                }
                NoteCmd::StartCreate => {
                    view.input = Some(NoteInput {
                        editing: None,
                        text: TextInput::new(),
                    });
                }
                NoteCmd::StartEdit(id, text) => {
                    view.input = Some(NoteInput {
                        editing: Some(id),
                        // Cursor lands at the end, ready for ⌥← to step back
                        // through the existing name.
                        text: TextInput::with_text(text),
                    });
                }
                NoteCmd::CancelInput => view.input = None,
                NoteCmd::Create(text) => {
                    view.input = None;
                    let owner = view.owner.clone();
                    let req_id = app.alloc_req_id(PendingIntent::SelectCreatedNote);
                    out.push(ClientRequest::CreateNote {
                        req_id,
                        owner,
                        text,
                    });
                }
                NoteCmd::Update(id, text) => {
                    view.input = None;
                    let req_id = app.alloc_req_id(PendingIntent::None);
                    out.push(ClientRequest::UpdateNote { req_id, id, text });
                }
                NoteCmd::Toggle(id, done) => {
                    let req_id = app.alloc_req_id(PendingIntent::None);
                    out.push(ClientRequest::SetNoteDone { req_id, id, done });
                }
                NoteCmd::Delete(id) => {
                    let req_id = app.alloc_req_id(PendingIntent::None);
                    out.push(ClientRequest::DeleteNote { req_id, id });
                }
            }
        }
    }
}

fn handle_settings_key(app: &mut App, key: KeyEvent) {
    let Some(Overlay::Settings(view)) = &app.overlay else {
        return;
    };
    let last = crate::config::settings_len().saturating_sub(1);
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
            app.settings_selected = i;
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
    app.animations = cfg.animations;
    app.focus_tint = cfg.focus_tint;
}

fn submit_prompt(app: &mut App, prompt: PromptDialog, out: &mut Vec<ClientRequest>) {
    let value = prompt.input.trim().to_string();
    // An empty divider label is meaningful: it clears the label.
    if let PromptKind::DividerLabel { id, before } = &prompt.kind {
        let req_id = app.alloc_req_id(PendingIntent::None);
        out.push(ClientRequest::SetProjectDivider {
            req_id,
            id: id.clone(),
            before: *before,
            present: true,
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
        PromptKind::NewAgent {
            worktree,
            kind,
            model,
            effort,
        } => {
            // Accepting the generated default name opts the session into
            // agent-driven auto-titling (`nebula rename` on first prompt);
            // a typed name is the user's choice and stays.
            let auto_title = value.is_empty();
            let name = if value.is_empty() {
                app.default_session_name("agent")
            } else {
                value
            };
            let req_id = app.alloc_req_id(PendingIntent::AttachCreated);
            out.push(ClientRequest::CreateAgent {
                req_id,
                worktree: worktree.clone(),
                name,
                kind,
                model,
                effort,
                auto_title,
            });
            // The create consumes (or, off-spec, discards) the worktree's
            // warm Claude slot; refill it so the next create is instant too.
            if kind == AgentKind::Claude {
                out.push(default_claude_prewarm(worktree));
            }
        }
        PromptKind::RenameAgent { id } => {
            let req_id = app.alloc_req_id(PendingIntent::None);
            out.push(ClientRequest::RenameAgent {
                req_id,
                id,
                name: value,
            });
        }
        PromptKind::RenameTerminal { id } => {
            let req_id = app.alloc_req_id(PendingIntent::None);
            out.push(ClientRequest::RenameTerminal {
                req_id,
                id,
                name: value,
            });
        }
        PromptKind::NewWorkspace => {
            // Created from the switcher: open it as soon as the Ack lands.
            let req_id = app.alloc_req_id(PendingIntent::OpenCreatedWorkspace);
            out.push(ClientRequest::AddWorkspace {
                req_id,
                name: value,
            });
        }
        PromptKind::RenameWorkspace { id } => {
            let req_id = app.alloc_req_id(PendingIntent::None);
            out.push(ClientRequest::RenameWorkspace {
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
            // Optimistic: drop the rows now (the daemon deletes in the
            // background — `git worktree remove` can take seconds). The
            // eventual EntityRemoved is a no-op; an Error for this req_id
            // restores the rows via the rollback stashed in the intent.
            let before = selection_snapshot(app);
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
            // Deleting the selected worktree lands the cursor on a neighbor
            // — bring up that neighbor's session like a manual switch would.
            reconcile_selection(app, before, out);
        }
        PendingAction::DeleteAllWorktrees(ids) => {
            // Each delete is its own request with its own optimistic
            // removal + rollback, so one failure restores only its rows.
            // One reconcile at the end: the cursor settles on a survivor.
            let before = selection_snapshot(app);
            for id in ids {
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
            reconcile_selection(app, before, out);
        }
        PendingAction::DeleteAllSessions { agents, terminals } => {
            for id in agents {
                detach_if_attached(app, &SessionRef::Agent(id.clone()), out);
                let req_id = app.alloc_req_id(PendingIntent::None);
                out.push(ClientRequest::DeleteAgent { req_id, id });
            }
            for id in terminals {
                detach_if_attached(app, &SessionRef::Terminal(id.clone()), out);
                let req_id = app.alloc_req_id(PendingIntent::None);
                out.push(ClientRequest::CloseTerminal { req_id, id });
            }
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
            archive_agent(app, id, out);
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
        MenuAction::NewTerminal(worktree) => {
            let req_id = app.alloc_req_id(PendingIntent::AttachCreated);
            out.push(ClientRequest::CreateTerminal {
                req_id,
                worktree,
                name: None,
            });
        }
        MenuAction::RenameTerminal(id) => open_prompt(app, PromptKind::RenameTerminal { id }),
        MenuAction::CloseTerminal(id) => {
            if let Some(t) = app.tree.terminals.iter().find(|t| t.id == id).cloned() {
                app.overlay = Some(Overlay::Confirm(ConfirmDialog {
                    title: "Close terminal".into(),
                    message: format!("Close terminal '{}'? Its shell is killed.", t.name),
                    action: PendingAction::CloseTerminal(id),
                }));
            }
        }
        MenuAction::NewAgentOfKind {
            worktree,
            kind,
            model,
            effort,
        } => {
            // Resolve the picker's choice against the configured defaults:
            // an unexpanded submenu (None) and the explicit "default" row
            // both take the setting; the setting's own "default" means
            // "no flag" and reaches the daemon as None.
            let cfg = crate::config::Config::load();
            let resolve = |choice: Option<String>, configured: Option<String>| match choice {
                None => configured,
                Some(c) if c == "default" => configured,
                some => some,
            };
            let model = resolve(model, cfg.default_model(kind));
            let effort = resolve(effort, cfg.default_effort(kind));
            // Warm the CLI while the user types the name: the daemon
            // pre-spawns the session so CreateAgent adopts an already-booted
            // PTY. Fail-soft — a missing CLI just means a cold spawn later.
            out.push(ClientRequest::PrewarmAgent {
                worktree: worktree.clone(),
                kind,
                model: model.clone(),
                effort: effort.clone(),
            });
            open_prompt(
                app,
                PromptKind::NewAgent {
                    worktree,
                    kind,
                    model,
                    effort,
                },
            )
        }
        MenuAction::NewWorktree(project) => open_prompt(app, PromptKind::NewWorktree { project }),
        MenuAction::OpenNotes(owner) => open_notes_for_owner(app, owner),
        MenuAction::SetWorktreePinned(id, pinned) => {
            let req_id = app.alloc_req_id(PendingIntent::None);
            out.push(ClientRequest::SetWorktreePinned { req_id, id, pinned });
        }
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
        MenuAction::OpenWorkspace(id) => {
            // The switch itself lands when ActiveWorkspaceChanged arrives.
            if id != app.tree.active_workspace {
                let req_id = app.alloc_req_id(PendingIntent::None);
                out.push(ClientRequest::OpenWorkspace { req_id, id });
            }
        }
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
        MenuAction::SetProjectDivider {
            id,
            before,
            present,
        } => {
            let req_id = app.alloc_req_id(PendingIntent::None);
            out.push(ClientRequest::SetProjectDivider {
                req_id,
                id,
                before,
                present,
                label: None,
            });
        }
        MenuAction::LabelDivider(id, before) => {
            open_prompt(app, PromptKind::DividerLabel { id, before })
        }
        MenuAction::ToggleArchived => toggle_archived(app, out),
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
            // Edge check in row terms: crossing a divider at the edge is
            // still a real move (the divider ends up leading / trailing).
            let last = app.project_rows().len() - 1;
            let at_edge = match delta.signum() {
                -1 => app.sel_project == 0,
                1 => app.sel_project == last,
                _ => true,
            };
            if at_edge {
                return;
            }
            let id = app.tree.projects[index].id.clone();
            let req_id = app.alloc_req_id(PendingIntent::None);
            out.push(ClientRequest::MoveProject { req_id, id, delta });
        }
        Some(ProjectRow::Divider { project, before }) => {
            move_divider(app, project, before, delta, out)
        }
        None => {}
    }
}

/// Ask the daemon to hop the selected divider into the previous/next gap —
/// including the slot above the whole list (the leading divider). Mirrors
/// the daemon's own rules so a blocked move flashes immediately instead of
/// arming a selection-follow that never fires.
fn move_divider(
    app: &mut App,
    project: usize,
    before: bool,
    delta: i64,
    out: &mut Vec<ClientRequest>,
) {
    // Neighbors live in the open workspace's visible sequence — `project`
    // indexes the full tree, where another workspace's rows may interleave.
    let visible: Vec<usize> = app
        .tree
        .projects
        .iter()
        .enumerate()
        .filter(|(_, p)| app.tree.in_active_workspace(p))
        .map(|(i, _)| i)
        .collect();
    let Some(vpos) = visible.iter().position(|&i| i == project) else {
        return;
    };
    let down = delta.signum() > 0;
    if before {
        // The leading divider: up is already the top; down hops below the
        // first project when that gap is free.
        if !down {
            return;
        }
        if app.tree.projects[project].divider_after {
            app.flash = Some("that gap already has a divider".into());
            return;
        }
        app.select_divider_when_seen = Some((app.tree.projects[project].id.clone(), false));
    } else if vpos == 0 && !down {
        // The divider under the first project hops above the whole list.
        if app.tree.projects[project].divider_before {
            app.flash = Some("that gap already has a divider".into());
            return;
        }
        app.select_divider_when_seen = Some((app.tree.projects[project].id.clone(), true));
    } else {
        let neighbor = vpos as i64 + delta.signum();
        let Some(neighbor) = usize::try_from(neighbor)
            .ok()
            .and_then(|i| visible.get(i))
            .and_then(|&i| app.tree.projects.get(i))
        else {
            return; // no project below: the divider is at the bottom edge
        };
        if neighbor.divider_after {
            app.flash = Some("that gap already has a divider".into());
            return;
        }
        app.select_divider_when_seen = Some((neighbor.id.clone(), false));
    }
    let id = app.tree.projects[project].id.clone();
    let req_id = app.alloc_req_id(PendingIntent::None);
    out.push(ClientRequest::MoveDivider {
        req_id,
        id,
        before,
        delta,
    });
}

fn remove_divider(app: &mut App, project_index: usize, before: bool, out: &mut Vec<ClientRequest>) {
    let id = app.tree.projects[project_index].id.clone();
    let req_id = app.alloc_req_id(PendingIntent::None);
    out.push(ClientRequest::SetProjectDivider {
        req_id,
        id,
        before,
        present: false,
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
    match app.selected_session_row().map(|r| r.sref()) {
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
    schedule_prewarm(app);
    let remembered = app
        .selected_worktree()
        .and_then(|w| app.last_session_for_worktree.get(&w.id).cloned());
    let target = remembered.and_then(|sref| {
        app.visible_session_rows()
            .iter()
            .position(|r| r.sref() == sref && !r.is_archived_agent())
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

/// Land the selection on `select_when_seen` — a session just created, or
/// moved into another worktree of this project: directly when its row is
/// visible under the selected worktree, else by switching to the worktree it
/// landed under first. Clears the pending follow once it lands; a no-op
/// until the session's upsert has arrived.
fn land_pending_selection(app: &mut App, out: &mut Vec<ClientRequest>) {
    let Some(pending_sref) = app.select_when_seen.clone() else {
        return;
    };
    if let Some(index) = app
        .visible_session_rows()
        .iter()
        .position(|r| r.sref() == pending_sref)
    {
        app.sel_session = index;
        app.select_when_seen = None;
        // The pane follows the cursor; a session about to be attached
        // outright (the create flow's Ack) dedupes in attach().
        preview_selected(app, out);
        return;
    }
    let landed_worktree = match &pending_sref {
        SessionRef::Agent(id) => app
            .tree
            .agents
            .iter()
            .find(|a| &a.id == id)
            .map(|a| a.worktree_id.clone()),
        SessionRef::Terminal(id) => app
            .tree
            .terminals
            .iter()
            .find(|t| &t.id == id)
            .map(|t| t.worktree_id.clone()),
    };
    if let Some(wt_id) = landed_worktree {
        if select_worktree_by_id(app, &wt_id, out) {
            if let Some(index) = app
                .visible_session_rows()
                .iter()
                .position(|r| r.sref() == pending_sref)
            {
                app.sel_session = index;
                preview_selected(app, out);
            }
            app.select_when_seen = None;
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
/// switch); a worktree pick then hands focus to its Sessions panel, since
/// picking a worktree by name is a step towards one of its sessions, not
/// an errand in the Worktrees column. A session pick with `attach` opens
/// it immediately, exactly like Enter on its row; without, it only lands
/// on the row in the Sessions panel, previewing like ↑/↓ there. Targets
/// are re-validated against the
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
                app.focus = Focus::Sessions;
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
            app.focus = Focus::Sessions;
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
            let Some(index) = app
                .visible_session_rows()
                .iter()
                .position(|r| matches!(r, SessionRow::Agent(a) if a.id == id))
            else {
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

/// Land the panel selection on `sref`'s session and attach it — the metrics
/// modal's Enter. The same walk as the palette's session jump, generalized
/// to terminal tabs.
fn open_session(app: &mut App, sref: SessionRef, out: &mut Vec<ClientRequest>) {
    let worktree = match &sref {
        SessionRef::Agent(id) => app
            .tree
            .agents
            .iter()
            .find(|a| &a.id == id)
            .map(|a| a.worktree_id.clone()),
        SessionRef::Terminal(id) => app
            .tree
            .terminals
            .iter()
            .find(|t| &t.id == id)
            .map(|t| t.worktree_id.clone()),
    };
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
    let Some(index) = app
        .visible_session_rows()
        .iter()
        .position(|r| r.sref() == sref)
    else {
        restore_session(app, out);
        app.focus = Focus::Sessions;
        app.flash = Some("session no longer exists".into());
        return;
    };
    app.sel_session = index;
    attach_selected(app, out);
}

fn move_selection(app: &mut App, delta: i64, out: &mut Vec<ClientRequest>) {
    let len = match app.focus {
        Focus::Projects => app.project_rows().len(),
        Focus::Worktrees => app.visible_worktrees().len(),
        Focus::Sessions => app.visible_session_rows().len(),
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
/// the input lock — walking the list with ↑/↓ (or single-clicking a row)
/// previews each session so it can be read; Enter (or a double-click) is
/// what commits: focus + lock. Archived rows don't preview.
fn preview_selected(app: &mut App, out: &mut Vec<ClientRequest>) {
    let Some(row) = app.selected_session_row() else {
        return;
    };
    if row.is_archived_agent() {
        return;
    }
    attach(app, row.sref(), out);
}

fn attach_selected(app: &mut App, out: &mut Vec<ClientRequest>) {
    let rows = app.visible_session_rows();
    let Some(row) = rows.get(app.sel_session) else {
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
        out.push(ClientRequest::Detach {
            session: existing.sref.clone(),
        });
    }
    let (cols, rows) = pane_size(app);
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

/// Terminal-pane grid for spawn/attach requests; the fallback keeps
/// pre-first-draw requests from booting a 0×0 PTY.
fn pane_size(app: &App) -> (u16, u16) {
    let area = app.term_area;
    if area.width >= 2 && area.height >= 2 {
        (area.width, area.height)
    } else {
        (80, 24)
    }
}

/// Arm the debounced session prewarm for the selected worktree; the main
/// loop fires it once the selection has rested there (PREWARM_DEBOUNCE).
fn schedule_prewarm(app: &mut App) {
    app.pending_prewarm = app
        .selected_worktree()
        .map(|w| (w.id.clone(), std::time::Instant::now() + PREWARM_DEBOUNCE));
}

/// Send the armed worktree-sessions prewarm. Re-firing for an already-warm
/// worktree is a cheap daemon-side no-op, so staleness needs no handling
/// beyond the daemon skipping rows that no longer exist.
fn fire_pending_prewarm(app: &mut App, out: &mut Vec<ClientRequest>) {
    let Some((worktree, _)) = app.pending_prewarm.take() else {
        return;
    };
    let (cols, rows) = pane_size(app);
    out.push(ClientRequest::PrewarmWorktreeSessions {
        worktree: worktree.clone(),
        cols,
        rows,
    });
    // The selected worktree also keeps one Claude session standing by, so
    // creating a session there adopts an already-booted CLI.
    out.push(default_claude_prewarm(worktree));
    app.next_keepwarm = Some(std::time::Instant::now() + KEEPWARM_REFRESH);
}

/// The one spec kept permanently warm: a Claude CLI at the configured
/// default model/effort. Creates matching it adopt the warm session
/// instantly; any other spec launches cold on purpose — off-default CLIs
/// would sit idle holding memory for a spec the user rarely repeats.
fn default_claude_prewarm(worktree: WorktreeId) -> ClientRequest {
    let cfg = crate::config::Config::load();
    ClientRequest::PrewarmAgent {
        worktree,
        kind: AgentKind::Claude,
        model: cfg.default_model(AgentKind::Claude),
        effort: cfg.default_effort(AgentKind::Claude),
    }
}

/// Periodic re-assert of the standing warm Claude session for the selected
/// worktree. A young same-spec session makes this a daemon-side no-op and an
/// aging one is recycled in place, so without this tick the daemon's reaper
/// would empty the slot at its max age and the next create would boot cold.
fn fire_keepwarm(app: &mut App, out: &mut Vec<ClientRequest>) {
    let Some(worktree) = app.selected_worktree().map(|w| w.id.clone()) else {
        app.next_keepwarm = None;
        return;
    };
    out.push(default_claude_prewarm(worktree));
    app.next_keepwarm = Some(std::time::Instant::now() + KEEPWARM_REFRESH);
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

/// The two touching border cells at a vertical panel boundary `bx`, bounded
/// by `area` — the shared grab-zone rule for every splitter.
fn on_vsplit(bx: u16, area: ratatui::layout::Rect, column: u16, row: u16) -> bool {
    area.width > 0
        && row >= area.y
        && row < area.y + area.height
        && column.saturating_add(1) >= bx
        && column <= bx
}

/// Whether the mouse is somewhere a horizontal resize could start (or one is
/// already in progress): a main-screen splitter, or the file-list border of
/// the diff / tree modals.
fn pointer_wants_resize(app: &App, column: u16, row: u16) -> bool {
    if app.vim.is_some() {
        return false;
    }
    match &app.overlay {
        Some(Overlay::Diff(view)) => {
            view.files_drag.is_some() || on_vsplit(view.splitter_x(), view.area, column, row)
        }
        Some(Overlay::Tree(view)) => {
            view.files_drag.is_some() || on_vsplit(view.splitter_x(), view.area, column, row)
        }
        Some(_) => false,
        None => {
            app.splitter_drag.is_some()
                || matches!(app.hit_at(column, row), Some(HitTarget::Splitter(_)))
        }
    }
}

/// Track the mouse for the resize affordances: the pointer shape the outer
/// terminal should show (col-resize over any draggable boundary) and the
/// main-screen grip highlight. Runs on every mouse event — including plain
/// motion, the only kind that arrives with nothing pressed. Terminals that
/// don't report motion (Terminal.app) still pass through here on clicks and
/// drags, so drag state keeps the shape honest where hover can't.
fn update_pointer(app: &mut App, mouse: &MouseEvent) {
    app.pointer_shape = if pointer_wants_resize(app, mouse.column, mouse.row) {
        PointerShape::ColResize
    } else {
        PointerShape::Default
    };
    let hover = if app.vim.is_none() && app.overlay.is_none() {
        match (app.splitter_drag, app.hit_at(mouse.column, mouse.row)) {
            (Some(drag), _) => Some(drag.idx),
            (None, Some(HitTarget::Splitter(i))) => Some(i),
            _ => None,
        }
    } else {
        None
    };
    if app.hover_splitter != hover {
        app.hover_splitter = hover;
        app.dirty = true;
    }
}

fn handle_mouse(app: &mut App, mouse: MouseEvent, out: &mut Vec<ClientRequest>) {
    update_pointer(app, &mouse);
    // The editor modal swallows the mouse entirely — its selection/scroll
    // story is vim's, not ours.
    if app.vim.is_some() {
        return;
    }
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
    // A prompt dialog is modal too: the wheel and clicks drive the
    // Add-project directory listing (click highlights, a second click on
    // the highlighted row steps in); everything else is swallowed.
    if let Some(Overlay::Prompt(prompt)) = &mut app.overlay {
        match mouse.kind {
            MouseEventKind::ScrollDown => {
                prompt.move_hover(1);
                app.dirty = true;
            }
            MouseEventKind::ScrollUp => {
                prompt.move_hover(-1);
                app.dirty = true;
            }
            MouseEventKind::Down(MouseButton::Left) => {
                let area = prompt.list_area;
                if area.width > 0
                    && mouse.column >= area.x
                    && mouse.column < area.x + area.width
                    && mouse.row >= area.y
                    && mouse.row < area.y + area.height
                {
                    let i =
                        prompt.window_start(area.height as usize) + (mouse.row - area.y) as usize;
                    if i < prompt.dirs.len() {
                        if prompt.hover == Some(i) {
                            prompt.dive(i);
                        } else {
                            prompt.hover = Some(i);
                        }
                    }
                }
                app.dirty = true;
            }
            _ => {}
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
                if on_vsplit(bx, view.area, mouse.column, mouse.row) {
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
    // File finder: the wheel moves the selection, a click on a result row
    // opens it in the editor, a click outside the modal closes; everything
    // else is swallowed.
    if let Some(Overlay::Files(finder)) = &mut app.overlay {
        match mouse.kind {
            MouseEventKind::ScrollUp => {
                finder.select(finder.selected as i64 - 1);
                app.dirty = true;
            }
            MouseEventKind::ScrollDown => {
                finder.select(finder.selected as i64 + 1);
                app.dirty = true;
            }
            MouseEventKind::Down(MouseButton::Left) => {
                let list = finder.list_area;
                let inside_list = list.width > 0
                    && mouse.column >= list.x
                    && mouse.column < list.x + list.width
                    && mouse.row >= list.y
                    && mouse.row < list.y + list.height;
                let area = finder.area;
                let inside_modal = mouse.column >= area.x
                    && mouse.column < area.x + area.width
                    && mouse.row >= area.y
                    && mouse.row < area.y + area.height;
                if inside_list {
                    let start = finder.window_start(list.height as usize);
                    let index = start + (mouse.row - list.y) as usize;
                    if index < finder.matches.len() {
                        finder.select(index as i64);
                        open_selected_file_in_editor(app);
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
    // Find-in-files: the wheel moves the selection, a click on a result row
    // opens it in the editor, a click outside the modal closes; everything
    // else is swallowed.
    if let Some(Overlay::Grep(view)) = &mut app.overlay {
        match mouse.kind {
            MouseEventKind::ScrollUp => {
                view.select(view.selected as i64 - 1);
                app.dirty = true;
            }
            MouseEventKind::ScrollDown => {
                view.select(view.selected as i64 + 1);
                app.dirty = true;
            }
            MouseEventKind::Down(MouseButton::Left) => {
                let list = view.list_area;
                let inside_list = list.width > 0
                    && mouse.column >= list.x
                    && mouse.column < list.x + list.width
                    && mouse.row >= list.y
                    && mouse.row < list.y + list.height;
                let area = view.area;
                let inside_modal = mouse.column >= area.x
                    && mouse.column < area.x + area.width
                    && mouse.row >= area.y
                    && mouse.row < area.y + area.height;
                if inside_list {
                    let start = view.window_start(list.height as usize);
                    let index = start + (mouse.row - list.y) as usize;
                    if index < view.hits.len() {
                        view.select(index as i64);
                        open_selected_hit_in_editor(app);
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
    // Tree browser: the wheel scrolls the preview, a click selects a row
    // (folding/unfolding directories), a drag on the tree/preview border
    // resizes the tree panel, a click outside the modal closes; everything
    // else is swallowed.
    if let Some(Overlay::Tree(view)) = &mut app.overlay {
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
                // tree/preview boundary (the panel `Splitter` pattern).
                let bx = view.splitter_x();
                if on_vsplit(bx, view.area, mouse.column, mouse.row) {
                    view.files_drag = Some(bx as i32 - mouse.column as i32);
                    return;
                }
                let list = view.list_area;
                let inside_list = list.width > 0
                    && mouse.column >= list.x
                    && mouse.column < list.x + list.width
                    && mouse.row >= list.y
                    && mouse.row < list.y + list.height;
                let area = view.area;
                let inside_modal = mouse.column >= area.x
                    && mouse.column < area.x + area.width
                    && mouse.row >= area.y
                    && mouse.row < area.y + area.height;
                if inside_list {
                    let start = view.window_start(list.height as usize);
                    let index = start + (mouse.row - list.y) as usize;
                    if index < view.rows.len() {
                        view.select(index as i64);
                        view.toggle_row(index); // no-op on files / under a filter
                    }
                } else if !inside_modal {
                    app.overlay = None;
                }
                app.dirty = true;
            }
            MouseEventKind::Drag(MouseButton::Left) => {
                if let Some(offset) = view.files_drag {
                    view.set_files_width(mouse.column as i32 + offset);
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
    // Hosts picker: the wheel moves the selection, a click on a row connects
    // (the context-menu convention — rows are actions, not editable items),
    // a click outside the modal closes; everything else is swallowed.
    if let Some(Overlay::Hosts(view)) = &mut app.overlay {
        let max = view.hosts.len().saturating_sub(1) as i64;
        match mouse.kind {
            MouseEventKind::ScrollUp => {
                view.selected = (view.selected as i64 - 1).clamp(0, max) as usize;
                app.dirty = true;
            }
            MouseEventKind::ScrollDown => {
                view.selected = (view.selected as i64 + 1).clamp(0, max) as usize;
                app.dirty = true;
            }
            MouseEventKind::Down(MouseButton::Left) => {
                let list = view.list_area;
                let area = view.area;
                let inside_list = list.width > 0
                    && mouse.column >= list.x
                    && mouse.column < list.x + list.width
                    && mouse.row >= list.y
                    && mouse.row < list.y + list.height;
                let inside_modal = mouse.column >= area.x
                    && mouse.column < area.x + area.width
                    && mouse.row >= area.y
                    && mouse.row < area.y + area.height;
                if inside_list {
                    let start = view.window_start(list.height as usize);
                    let index = start + (mouse.row - list.y) as usize;
                    if let Some(entry) = view.hosts.get(index).cloned() {
                        view.selected = index;
                        app.overlay = None;
                        app.pending_ssh = Some(entry);
                        app.should_quit = true;
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
    // Note modal: the wheel moves the selection, a click on a row selects
    // it, a click outside the modal closes; everything else is swallowed.
    if let Some(Overlay::Notes(view)) = &mut app.overlay {
        let count = app
            .tree
            .notes
            .iter()
            .filter(|t| t.owner == view.owner)
            .count();
        let max = count.saturating_sub(1) as i64;
        match mouse.kind {
            MouseEventKind::ScrollUp => {
                view.selected = (view.selected as i64 - 1).clamp(0, max) as usize;
                app.dirty = true;
            }
            MouseEventKind::ScrollDown => {
                view.selected = (view.selected as i64 + 1).clamp(0, max) as usize;
                app.dirty = true;
            }
            MouseEventKind::Down(MouseButton::Left) => {
                let list = view.list_area;
                let inside_list = list.width > 0
                    && mouse.column >= list.x
                    && mouse.column < list.x + list.width
                    && mouse.row >= list.y
                    && mouse.row < list.y + list.height;
                let area = view.area;
                let inside_modal = mouse.column >= area.x
                    && mouse.column < area.x + area.width
                    && mouse.row >= area.y
                    && mouse.row < area.y + area.height;
                if inside_list {
                    let start = view.window_start(list.height as usize);
                    let index = start + (mouse.row - list.y) as usize;
                    if index < count {
                        view.selected = index;
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
                    // Group headers and blanks aren't clickable; the shared
                    // row map keeps this in step with the renderer.
                    if let Some(crate::config::SettingsRow::Setting(index)) =
                        crate::config::settings_rows().get(row).copied()
                    {
                        if let Some(Overlay::Settings(view)) = &mut app.overlay {
                            view.selected = index;
                        }
                        app.settings_selected = index;
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
    // Metrics: the wheel moves the selection, a click on a row selects it
    // (a click on the selected row opens it), a click outside closes;
    // everything else is swallowed.
    if let Some(Overlay::Metrics(view)) = &mut app.overlay {
        let max = view.rows.len().saturating_sub(1);
        let mut open: Option<SessionRef> = None;
        match mouse.kind {
            MouseEventKind::ScrollUp => {
                view.selected = view.selected.saturating_sub(1);
                app.dirty = true;
            }
            MouseEventKind::ScrollDown => {
                view.selected = (view.selected + 1).min(max);
                app.dirty = true;
            }
            MouseEventKind::Down(MouseButton::Left) => {
                let list = view.list_area;
                let inside_list = list.width > 0
                    && mouse.column >= list.x
                    && mouse.column < list.x + list.width
                    && mouse.row >= list.y
                    && mouse.row < list.y + list.height;
                let area = view.area;
                let inside_modal = mouse.column >= area.x
                    && mouse.column < area.x + area.width
                    && mouse.row >= area.y
                    && mouse.row < area.y + area.height;
                if inside_list {
                    let index = view.scroll + (mouse.row - list.y) as usize;
                    if index < view.rows.len() {
                        if view.selected == index {
                            open = view.rows[index].clone();
                        }
                        view.selected = index;
                    }
                } else if !inside_modal {
                    app.overlay = None;
                }
                app.dirty = true;
            }
            _ => {}
        }
        if let Some(sref) = open {
            app.overlay = None;
            open_session(app, sref, out);
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
                // Not a URL — a detected file path opens in the editor
                // modal instead (claude/cursor/codex print `path:line`).
                if let Some((path, line)) = app
                    .term_file_links
                    .iter()
                    .find(|link| link.contains(cell))
                    .map(|link| (link.path.clone(), link.line))
                {
                    open_file_link(app, &path, line);
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
                    match app.selected_session_row() {
                        Some(row) if row.is_archived_agent() => {
                            app.focus = Focus::Sessions;
                            app.flash = Some("agent is archived — unarchive first (u)".into());
                        }
                        Some(row) => {
                            let sref = row.sref();
                            let now = std::time::Instant::now();
                            // Double-click attaches; `last_session_click` was
                            // consumed, so a third click starts over.
                            let double = app.last_session_click.take().is_some_and(|(at, id)| {
                                id == sref && now.duration_since(at) <= DOUBLE_CLICK
                            });
                            if double {
                                attach_selected(app, out);
                            } else {
                                // Single click selects the row and previews its
                                // terminal (no focus/lock); Enter or a second
                                // click commits.
                                app.last_session_click = Some((now, sref));
                                app.focus = Focus::Sessions;
                                preview_selected(app, out);
                            }
                        }
                        None => {}
                    }
                }
                Some(HitTarget::ArchivedHeader) => {
                    app.focus = Focus::Sessions;
                    toggle_archived(app, out);
                }
                Some(HitTarget::PanelBg(focus)) => {
                    // Empty projects list: left click opens the obvious
                    // creation prompt. Other panels just take focus.
                    app.focus = focus;
                    if focus == Focus::Projects && !app.tree.has_visible_projects() {
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
            let over = app.hit_at(mouse.column, mouse.row);
            // The Sessions column scrolls under the wheel/trackpad — with
            // the ARCHIVED group expanded its list routinely outgrows the
            // panel. The offset moves without touching the selection; the
            // draw clamps it to the content.
            let over_sessions = !app.collapsed
                && matches!(
                    over,
                    Some(
                        HitTarget::Session(_)
                            | HitTarget::ArchivedHeader
                            | HitTarget::PanelBg(Focus::Sessions)
                    )
                );
            let in_term = matches!(over, Some(HitTarget::TerminalPane)) || app.collapsed;
            if over_sessions {
                app.sessions_scroll = if up {
                    app.sessions_scroll.saturating_sub(SESSIONS_WHEEL_STEP)
                } else {
                    app.sessions_scroll.saturating_add(SESSIONS_WHEEL_STEP)
                };
                app.dirty = true;
            } else if in_term {
                if let Some(term) = &mut app.term {
                    // Scrolling shifts the content under a (screen-anchored)
                    // selection highlight — drop it.
                    app.term_selection = None;
                    let screen = term.parser.screen();
                    let mouse_mode = screen.mouse_protocol_mode();
                    let sgr = screen.mouse_protocol_encoding() == vt100::MouseProtocolEncoding::Sgr;
                    let alternate = screen.alternate_screen();
                    if mouse_mode != vt100::MouseProtocolMode::None {
                        // The child asked for the mouse (claude's alt-screen
                        // UI, vim `mouse=a`, htop): forward the wheel event
                        // itself. Synthesized arrows would land in claude's
                        // input box — cycling prompt history and tripping its
                        // "Scroll wheel is sending arrow keys" warning.
                        let (col, row) = pane_cell(app.term_area, mouse.column, mouse.row);
                        let button: u16 = if up { 64 } else { 65 };
                        let data = if sgr {
                            format!("\x1b[<{button};{};{}M", col + 1, row + 1).into_bytes()
                        } else {
                            // Legacy X10 bytes: 32 + button/coord, 1-based
                            // coords capped at the encoding's 223 limit.
                            vec![
                                0x1b,
                                b'[',
                                b'M',
                                32 + button as u8,
                                32 + (col + 1).min(223) as u8,
                                32 + (row + 1).min(223) as u8,
                            ]
                        };
                        out.push(ClientRequest::Input {
                            session: term.sref.clone(),
                            data,
                        });
                    } else if alternate {
                        // Full-screen apps that ignore the mouse (plain vim,
                        // less, htop with mouse off) expect arrows.
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
                    if let Some(ProjectRow::Divider { project, before }) =
                        app.selected_project_row()
                    {
                        let id = app.tree.projects[project].id.clone();
                        open_menu_at(app, divider_row_menu(id, before), at);
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
                    match app.selected_session_row() {
                        Some(SessionRow::Agent(a)) => {
                            open_menu_at(app, menu_items_for_session(&a), at)
                        }
                        Some(SessionRow::Terminal(t)) => {
                            open_menu_at(app, menu_items_for_terminal(&t), at)
                        }
                        None => {}
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
            workspaces,
            active_workspace,
            projects,
            worktrees,
            agents,
            terminals,
            notes,
            ui_state,
        } => {
            app.tree.workspaces = workspaces;
            app.tree.active_workspace = active_workspace;
            app.tree.projects = projects;
            app.tree.worktrees = worktrees;
            app.tree.agents = agents;
            app.tree.terminals = terminals;
            app.tree.notes = notes;
            if let Some(json) = ui_state {
                restore_ui_state(app, &json);
            }
            clamp_selections(app);
            refresh_palette(app);
            // Boot the restored worktree's sessions right away — the first
            // thing the user does after launch is walk into one of them.
            schedule_prewarm(app);
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
            let keep = app.selected_session_row().map(|r| r.sref());
            if let Some(a) = app.tree.agents.iter_mut().find(|a| a.id == agent) {
                a.status = status;
                a.status_changed_at = changed_at;
                app.dirty = true;
            }
            if let Some(keep) = keep {
                if let Some(i) = app
                    .visible_session_rows()
                    .iter()
                    .position(|r| r.sref() == keep)
                {
                    app.sel_session = i;
                }
            }
        }
        ServerEvent::Ack { req_id, created } => {
            match (app.pending.remove(&req_id), created) {
                (Some(PendingIntent::AttachCreated), Some(id)) => {
                    let sref = match id {
                        EntityId::Agent(id) => Some(SessionRef::Agent(id)),
                        EntityId::Terminal(id) => Some(SessionRef::Terminal(id)),
                        _ => None,
                    };
                    if let Some(sref) = sref {
                        app.select_when_seen = Some(sref.clone());
                        // Its upsert usually lands just before this Ack; land
                        // the selection now, or on the upsert otherwise.
                        land_pending_selection(app, out);
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
                (Some(PendingIntent::SelectCreatedNote), Some(EntityId::Note(id))) => {
                    // Same idiom: land the modal's cursor now, or when the
                    // upsert arrives.
                    if !select_note_by_id(app, &id) {
                        app.select_note_when_seen = Some(id);
                    }
                }
                (Some(PendingIntent::OpenCreatedWorkspace), Some(EntityId::Workspace(id))) => {
                    // Switcher-created workspace: open it right away (the
                    // switch lands via ActiveWorkspaceChanged as usual).
                    let req_id = app.alloc_req_id(PendingIntent::None);
                    out.push(ClientRequest::OpenWorkspace { req_id, id });
                }
                _ => {}
            }
            app.dirty = true;
        }
        ServerEvent::EntityUpserted { entity } => {
            let before = selection_snapshot(app);
            apply_upsert(app, entity);
            // Cursors follow the row they were on across regroups (pin
            // toggles, re-homes); a row that left its list (archived away,
            // moved elsewhere) hands the cursor — and the terminal pane —
            // to its neighbor.
            reconcile_selection(app, before, out);
            // Fix the selection onto a session we just created — or follow
            // one we just moved into another worktree of this project.
            land_pending_selection(app, out);
            // ...and onto a worktree we just created.
            if let Some(wt_id) = app.select_worktree_when_seen.clone() {
                if select_worktree_by_id(app, &wt_id, out) {
                    app.select_worktree_when_seen = None;
                }
            }
            // ...and the note modal's cursor onto a note we just created.
            if let Some(note_id) = app.select_note_when_seen.clone() {
                if select_note_by_id(app, &note_id) {
                    app.select_note_when_seen = None;
                }
            }
            refresh_palette(app);
            refresh_workspace_picker(app);
            app.dirty = true;
        }
        ServerEvent::EntityRemoved { id } => {
            let before = selection_snapshot(app);
            apply_removal(app, &id);
            // The cursor that was on the removed row now sits on its
            // neighbor — show that neighbor's session/context.
            reconcile_selection(app, before, out);
            refresh_palette(app);
            refresh_workspace_picker(app);
            app.dirty = true;
        }
        ServerEvent::ActiveWorkspaceChanged { id } => {
            // A different workspace was opened — here, via the CLI, or by
            // another client; daemon-global either way. Everything visible
            // re-filters; selections land on the new workspace's first
            // project with its remembered worktree/session brought back.
            if app.tree.active_workspace != id {
                remember_context(app);
                app.tree.active_workspace = id;
                app.sel_project = 0;
                restore_context(app, out);
                clamp_selections(app);
                refresh_palette(app);
                // An open switcher keeps its ✓ on the now-open workspace.
                refresh_workspace_picker(app);
            }
            app.dirty = true;
        }
        ServerEvent::Metrics { req_id, snapshot } => {
            // Answered with Metrics, not Ack — clear the pending slot by hand.
            app.pending.remove(&req_id);
            if let Some(Overlay::Metrics(view)) = &mut app.overlay {
                view.snapshot = Some(snapshot.clone());
            }
            // The footer's readout keeps the latest reading either way.
            app.last_metrics = Some(snapshot);
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
        Entity::Workspace(w) => match app.tree.workspaces.iter_mut().find(|x| x.id == w.id) {
            Some(existing) => *existing = w,
            None => app.tree.workspaces.push(w),
        },
        Entity::Project(p) => {
            // A row's kind: None = the project itself, Some(before) = one
            // of its dividers.
            let kind = |row: &ProjectRow| match row {
                ProjectRow::Divider { before, .. } => Some(*before),
                ProjectRow::Project(_) => None,
            };
            let selected = app.selected_project_row().map(|row| {
                (
                    kind(&row),
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
            if let Some((was_kind, id)) = selected {
                let rows = app.project_rows();
                let same_kind = rows.iter().position(|row| {
                    kind(row) == was_kind && app.tree.projects[row.project_index()].id == id
                });
                let found = same_kind.or_else(|| {
                    rows.iter()
                        .position(|row| app.tree.projects[row.project_index()].id == id)
                });
                if let Some(i) = found {
                    app.sel_project = i;
                }
            }
            // A divider we moved re-homes onto another slot; chase it
            // there once the destination's upsert lands.
            if let Some((target, before)) = app.select_divider_when_seen.clone() {
                let rows = app.project_rows();
                let landed = rows.iter().position(|row| {
                    kind(row) == Some(before) && app.tree.projects[row.project_index()].id == target
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
        Entity::Terminal(t) => match app.tree.terminals.iter_mut().find(|x| x.id == t.id) {
            Some(existing) => *existing = t,
            None => app.tree.terminals.push(t),
        },
        Entity::Note(t) => match app.tree.notes.iter_mut().find(|x| x.id == t.id) {
            Some(existing) => *existing = t,
            None => app.tree.notes.push(t),
        },
    }
}

/// Land the note modal's cursor on `id`; false when the modal isn't open on
/// that note's owner or the note hasn't arrived in the tree yet.
fn select_note_by_id(app: &mut App, id: &NoteId) -> bool {
    let pos = match &app.overlay {
        Some(Overlay::Notes(view)) => app
            .tree
            .notes
            .iter()
            .filter(|t| t.owner == view.owner)
            .position(|t| &t.id == id),
        _ => return false,
    };
    match (pos, &mut app.overlay) {
        (Some(i), Some(Overlay::Notes(view))) => {
            view.selected = i;
            true
        }
        _ => false,
    }
}

fn apply_removal(app: &mut App, id: &nebula_core::EntityId) {
    use nebula_core::EntityId;
    match id {
        EntityId::Workspace(id) => {
            // Only empty, non-open workspaces get deleted (and an open one is
            // switched away from first, via ActiveWorkspaceChanged), so no
            // project rows need cleanup here.
            app.tree.workspaces.retain(|w| &w.id != id);
        }
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
            app.tree
                .terminals
                .retain(|t| !wt_ids.contains(&t.worktree_id));
            app.tree.notes.retain(|t| match &t.owner {
                NoteOwner::Project(p) => p != id,
                NoteOwner::Worktree(w) => !wt_ids.contains(w),
            });
            app.tree.worktrees.retain(|w| &w.project_id != id);
            app.tree.projects.retain(|p| &p.id != id);
        }
        EntityId::Worktree(id) => {
            app.tree.agents.retain(|a| &a.worktree_id != id);
            app.tree.terminals.retain(|t| &t.worktree_id != id);
            app.tree
                .notes
                .retain(|t| t.owner != NoteOwner::Worktree(id.clone()));
            app.tree.worktrees.retain(|w| &w.id != id);
        }
        EntityId::Agent(id) => app.tree.agents.retain(|a| &a.id != id),
        EntityId::Terminal(id) => app.tree.terminals.retain(|t| &t.id != id),
        EntityId::Note(id) => app.tree.notes.retain(|t| &t.id != id),
    }
    // A note modal aimed at a vanished owner has nothing left to show.
    if let Some(Overlay::Notes(view)) = &app.overlay {
        let gone = match &view.owner {
            NoteOwner::Project(id) => !app.tree.projects.iter().any(|p| &p.id == id),
            NoteOwner::Worktree(id) => !app.tree.worktrees.iter().any(|w| &w.id == id),
        };
        if gone {
            app.overlay = None;
        }
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

/// What each panel cursor pointed at, captured with `selection_snapshot`
/// before a tree mutation so `reconcile_selection` can compare afterwards.
struct SelectionSnapshot {
    project: Option<nebula_core::ProjectId>,
    /// The selected Projects-panel row's kind: None = the project itself,
    /// Some(before) = one of its dividers (apply_upsert's convention).
    project_kind: Option<bool>,
    /// A divider move was in flight — its landing is apply_upsert's to
    /// chase, so the project cursor is left alone.
    divider_chase: bool,
    worktree: Option<WorktreeId>,
    session: Option<SessionRef>,
    /// Whether the selected session row was already in the archived group —
    /// following onto an archived row is only right when it was.
    session_archived: bool,
}

fn project_row_kind(row: &ProjectRow) -> Option<bool> {
    match row {
        ProjectRow::Divider { before, .. } => Some(*before),
        ProjectRow::Project(_) => None,
    }
}

fn selection_snapshot(app: &App) -> SelectionSnapshot {
    let row = app.selected_session_row();
    SelectionSnapshot {
        project: app.selected_project().map(|p| p.id.clone()),
        project_kind: app
            .selected_project_row()
            .as_ref()
            .and_then(project_row_kind),
        divider_chase: app.select_divider_when_seen.is_some(),
        worktree: app.selected_worktree().map(|w| w.id.clone()),
        session_archived: row.as_ref().is_some_and(|r| r.is_archived_agent()),
        session: row.map(|r| r.sref()),
    }
}

/// Re-point the panel cursors after the tree changed. Each cursor follows
/// the entity it was on when rows merely shifted; when that entity left its
/// list — deleted, archived away, re-homed — the cursor has landed on a
/// neighbor, and that neighbor gets shown exactly as if the user had moved
/// there (restore_context / restore_session / preview). The invariant: the
/// terminal pane always shows the highlighted session, never a stale or
/// blank one.
fn reconcile_selection(app: &mut App, before: SelectionSnapshot, out: &mut Vec<ClientRequest>) {
    clamp_selections(app);
    if let Some(pid) = &before.project {
        if !app.tree.projects.iter().any(|p| &p.id == pid) {
            // The selected row's project is gone; the cursor landed on a
            // neighbor — bring up its remembered worktree + session.
            restore_context(app, out);
            return;
        }
        // A divider move lands via apply_upsert's own chase; don't fight it.
        if !before.divider_chase
            && app.selected_project().map(|p| p.id.clone()).as_ref() != Some(pid)
        {
            let rows = app.project_rows();
            let same_kind = rows.iter().position(|r| {
                project_row_kind(r) == before.project_kind
                    && &app.tree.projects[r.project_index()].id == pid
            });
            let found = same_kind.or_else(|| {
                rows.iter().position(
                    |r| matches!(r, ProjectRow::Project(i) if &app.tree.projects[*i].id == pid),
                )
            });
            if let Some(i) = found {
                app.sel_project = i;
            }
        }
    }
    if let Some(wid) = &before.worktree {
        if app.selected_worktree().map(|w| w.id.clone()).as_ref() != Some(wid) {
            match app.visible_worktrees().iter().position(|w| &w.id == wid) {
                Some(i) => app.sel_worktree = i,
                None => {
                    restore_session(app, out);
                    return;
                }
            }
        }
    }
    if let Some(sref) = &before.session {
        let rows = app.visible_session_rows();
        if rows.get(app.sel_session).map(|r| r.sref()).as_ref() != Some(sref) {
            let found = rows.iter().position(|r| {
                r.sref() == *sref && (before.session_archived || !r.is_archived_agent())
            });
            match found {
                Some(i) => app.sel_session = i,
                None => {
                    preview_selected(app, out);
                    // Nothing previewable left (empty list, or only archived
                    // rows): don't keep showing a session that's gone.
                    if let Some(tref) = app.term.as_ref().map(|t| t.sref.clone()) {
                        let alive = match &tref {
                            SessionRef::Agent(id) => app.tree.agents.iter().any(|a| &a.id == id),
                            SessionRef::Terminal(id) => {
                                app.tree.terminals.iter().any(|t| &t.id == id)
                            }
                        };
                        if !alive {
                            detach_if_attached(app, &tref, out);
                        }
                    }
                }
            }
        }
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
    let sess_len = app.visible_session_rows().len();
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

    /// (x, y) of the first cell of `needle` in the rendered buffer.
    fn find_cell(terminal: &Terminal<TestBackend>, needle: &str) -> (u16, u16) {
        let buffer = terminal.backend().buffer();
        for y in 0..buffer.area.height {
            let line: String = (0..buffer.area.width)
                .map(|x| buffer[(x, y)].symbol())
                .collect();
            if let Some(byte) = line.find(needle) {
                return (line[..byte].chars().count() as u16, y);
            }
        }
        panic!("{needle:?} is not on screen");
    }

    fn seed_tree(app: &mut App) {
        use nebula_core::{Agent, AgentStatus, Entity, Project, ProjectId, Worktree, WorktreeId};
        let project_id = ProjectId("p1".into());
        let worktree_id = WorktreeId("w1".into());
        hse(
            app,
            ServerEvent::EntityUpserted {
                entity: Entity::Project(Project {
                    workspace_id: Default::default(),
                    id: project_id.clone(),
                    name: "demo".into(),
                    repo_path: "/tmp/demo".into(),
                    sort_order: 0,
                    divider_after: false,
                    divider_label: None,
                    divider_before: false,
                    divider_before_label: None,
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
                    pinned: false,
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
                    archived_at: 0,
                    pinned: false,
                    kind: nebula_core::AgentKind::Claude,
                    model: None,
                    effort: None,
                    session_id: None,
                    sort_order: 0,
                    status_changed_at: 0,
                    alive: true,
                }),
            },
        );
    }

    /// An empty tree replaces the panel columns with the animated splash
    /// (wordmark + create hint); the first project upsert swaps the normal
    /// columns back in.
    #[test]
    fn empty_tree_draws_splash_until_first_project() {
        let mut app = App::new();
        let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
        terminal.draw(|f| ui::draw(f, &mut app)).unwrap();
        let text = buffer_text(&terminal);
        assert!(text.contains("create your first project"), "{text}");
        assert!(
            text.contains("your agents keep running"),
            "tagline on the splash: {text}"
        );
        assert!(!text.contains("PROJECTS"), "no panel chrome: {text}");
        assert!(app.splash_active());

        seed_tree(&mut app);
        assert!(!app.splash_active());
        terminal.draw(|f| ui::draw(f, &mut app)).unwrap();
        let text = buffer_text(&terminal);
        assert!(text.contains("PROJECTS"), "columns back: {text}");
    }

    /// The animations setting is a master off-switch for both repaint
    /// tickers: the status sweep (running/red rows) and the splash.
    #[test]
    fn animations_off_stops_sweep_and_splash_ticking() {
        let mut app = App::new();
        assert!(app.splash_active(), "empty tree splash ticks by default");
        app.animations = false;
        assert!(!app.splash_active(), "still splash: drawn but not ticked");

        app.animations = true;
        seed_tree(&mut app);
        assert!(!app.status_anim_active(), "fresh agent doesn't animate");
        app.tree.agents[0].status = nebula_core::AgentStatus::Running;
        assert!(app.status_anim_active());
        app.animations = false;
        assert!(!app.status_anim_active());
    }

    /// N summons the splash as a preview over a populated tree — full-body
    /// nebula with the "any key" hint instead of panel columns — and the
    /// next keypress (even q) only dismisses it.
    #[test]
    fn shift_n_previews_splash_and_any_key_dismisses() {
        let mut app = App::new();
        seed_tree(&mut app);
        let mut out = Vec::new();
        press(&mut app, KeyCode::Char('N'), KeyModifiers::SHIFT, &mut out);
        assert!(app.splash_preview && app.splash_active());

        let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
        terminal.draw(|f| ui::draw(f, &mut app)).unwrap();
        let text = buffer_text(&terminal);
        assert!(text.contains("any key returns"), "{text}");
        assert!(!text.contains("PROJECTS"), "panels hidden: {text}");

        press(&mut app, KeyCode::Char('q'), KeyModifiers::NONE, &mut out);
        assert!(!app.splash_preview, "any key dismisses");
        assert!(!app.should_quit, "the dismissing key is swallowed");
        terminal.draw(|f| ui::draw(f, &mut app)).unwrap();
        assert!(buffer_text(&terminal).contains("PROJECTS"));
    }

    /// While the tree is empty, `n` opens the add-project prompt from any
    /// focus — the splash hides the panels, so the per-panel meanings of
    /// `n` would just dead-end.
    #[test]
    fn n_adds_project_from_any_focus_while_tree_empty() {
        let mut app = App::new();
        app.focus = Focus::Sessions;
        let mut out = Vec::new();
        press(&mut app, KeyCode::Char('n'), KeyModifiers::NONE, &mut out);
        let Some(Overlay::Prompt(p)) = &app.overlay else {
            panic!("expected add-project prompt, got {:?}", app.overlay);
        };
        assert_eq!(p.kind, crate::app::PromptKind::AddProject);
    }

    /// The splash hides the panels, so the footer drops the panel keymap
    /// for the handful of keys that still fire under it — and in preview,
    /// for the only one there is.
    #[test]
    fn splash_footer_lists_only_keys_that_work() {
        let mut app = App::new();
        let mut terminal = Terminal::new(TestBackend::new(140, 30)).unwrap();
        terminal.draw(|f| ui::draw(f, &mut app)).unwrap();
        let text = buffer_text(&terminal);
        assert!(text.contains("n/o: add project"), "{text}");
        assert!(text.contains("w: workspaces"), "{text}");
        assert!(text.contains("q: quit"), "{text}");
        for dead in [
            "e: notes",
            "d: remove",
            "-: divider",
            "m: menu",
            "/: search",
        ] {
            assert!(
                !text.contains(dead),
                "{dead} does nothing on the splash: {text}"
            );
        }

        // Preview over a populated tree: the next key only dismisses.
        seed_tree(&mut app);
        let mut out = Vec::new();
        press(&mut app, KeyCode::Char('N'), KeyModifiers::SHIFT, &mut out);
        terminal.draw(|f| ui::draw(f, &mut app)).unwrap();
        let text = buffer_text(&terminal);
        assert!(text.contains("any key: back to panels"), "{text}");
        assert!(!text.contains("n/o: add project"), "{text}");

        // Panels back, panel keymap back.
        press(&mut app, KeyCode::Esc, KeyModifiers::NONE, &mut out);
        terminal.draw(|f| ui::draw(f, &mut app)).unwrap();
        assert!(buffer_text(&terminal).contains("m: menu"));
    }

    /// `w` is one of the splash's advertised keys, so it opens the
    /// workspace picker from any focus while the splash is up — including
    /// the terminal focus its guard normally excludes.
    #[test]
    fn w_opens_workspace_picker_from_any_focus_under_splash() {
        let mut app = App::new();
        app.tree.workspaces.push(nebula_core::Workspace {
            id: "default".to_string().into(),
            name: "default".into(),
        });
        app.focus = Focus::Terminal;
        let mut out = Vec::new();
        press(&mut app, KeyCode::Char('w'), KeyModifiers::NONE, &mut out);
        assert!(
            matches!(&app.overlay, Some(Overlay::Menu(m)) if m.is_workspace_picker()),
            "expected the workspace picker, got {:?}",
            app.overlay
        );
    }

    /// `o` opens the add-project prompt regardless of focus or tree state —
    /// unlike `n` it never takes on a per-panel meaning.
    #[test]
    fn o_adds_project_from_any_focus() {
        for focus in [Focus::Projects, Focus::Worktrees, Focus::Sessions] {
            let mut app = App::new();
            seed_tree(&mut app);
            app.focus = focus;
            let mut out = Vec::new();
            press(&mut app, KeyCode::Char('o'), KeyModifiers::NONE, &mut out);
            let Some(Overlay::Prompt(p)) = &app.overlay else {
                panic!(
                    "expected add-project prompt at {focus:?}, got {:?}",
                    app.overlay
                );
            };
            assert_eq!(p.kind, crate::app::PromptKind::AddProject);
        }
    }

    /// `e` opens the notes modal for the current selection.
    #[test]
    fn e_opens_notes_for_selection() {
        let mut app = App::new();
        seed_tree(&mut app);
        app.focus = Focus::Worktrees;
        let mut out = Vec::new();
        press(&mut app, KeyCode::Char('e'), KeyModifiers::NONE, &mut out);
        assert!(
            matches!(app.overlay, Some(Overlay::Notes(_))),
            "expected notes overlay, got {:?}",
            app.overlay
        );
    }

    /// Editing a note's text behaves like a terminal line: the caret starts
    /// at the end, ⌥← (which macOS terminals send as ESC b) walks back a
    /// word at a time, and typing lands at the caret instead of the tail.
    #[test]
    fn note_edit_takes_word_motion_and_inserts_at_the_caret() {
        use crate::app::NoteView;
        use nebula_core::{Note, NoteId, NoteOwner, ProjectId};
        let mut app = App::new();
        seed_tree(&mut app);
        let owner = NoteOwner::Project(ProjectId("p1".into()));
        app.tree.notes.push(Note {
            id: NoteId("t1".into()),
            owner: owner.clone(),
            text: "fix login redirect".into(),
            done: false,
            sort_order: 0,
        });
        app.overlay = Some(Overlay::Notes(NoteView::new(owner, "demo".into())));
        let mut out = Vec::new();

        press(&mut app, KeyCode::Enter, KeyModifiers::NONE, &mut out);
        for _ in 0..2 {
            press(&mut app, KeyCode::Char('b'), KeyModifiers::ALT, &mut out);
        }
        for c in "the ".chars() {
            press(&mut app, KeyCode::Char(c), KeyModifiers::NONE, &mut out);
        }
        let Some(Overlay::Notes(view)) = &app.overlay else {
            panic!("notes closed")
        };
        let input = view.input.as_ref().expect("still editing");
        assert_eq!(input.text.as_str(), "fix the login redirect");

        out.clear();
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE, &mut out);
        assert!(
            matches!(out.as_slice(), [ClientRequest::UpdateNote { text, .. }] if text == "fix the login redirect"),
            "Enter saves the edited text, got {out:?}"
        );
    }

    /// Backspace on a note being edited deletes a character; it must not
    /// reach the list's "delete the selected note" binding, and at column 0
    /// it does nothing at all.
    #[test]
    fn note_edit_swallows_backspace_instead_of_deleting_the_note() {
        use crate::app::NoteView;
        use nebula_core::{Note, NoteId, NoteOwner, ProjectId};
        let mut app = App::new();
        seed_tree(&mut app);
        let owner = NoteOwner::Project(ProjectId("p1".into()));
        app.tree.notes.push(Note {
            id: NoteId("t1".into()),
            owner: owner.clone(),
            text: "ab".into(),
            done: false,
            sort_order: 0,
        });
        app.overlay = Some(Overlay::Notes(NoteView::new(owner, "demo".into())));
        let mut out = Vec::new();
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE, &mut out);
        out.clear();
        for _ in 0..4 {
            press(&mut app, KeyCode::Backspace, KeyModifiers::NONE, &mut out);
        }
        let Some(Overlay::Notes(view)) = &app.overlay else {
            panic!("notes closed")
        };
        assert_eq!(
            view.input.as_ref().expect("still editing").text.as_str(),
            ""
        );
        assert!(out.is_empty(), "no DeleteNote leaked out, got {out:?}");
    }

    /// The always-live search fields edit the same way — and ⌥←/⌥→ move the
    /// caret rather than typing a literal "b"/"f" into the query.
    #[test]
    fn palette_query_edits_like_a_line_and_refilters() {
        let mut app = App::new();
        seed_tree(&mut app);
        let mut out = Vec::new();
        press(&mut app, KeyCode::Char('/'), KeyModifiers::NONE, &mut out);
        for c in "demo".chars() {
            press(&mut app, KeyCode::Char(c), KeyModifiers::NONE, &mut out);
        }
        press(&mut app, KeyCode::Char('b'), KeyModifiers::ALT, &mut out);
        press(&mut app, KeyCode::Char('x'), KeyModifiers::NONE, &mut out);
        let matched = |app: &App| match &app.overlay {
            Some(Overlay::Palette(p)) => p.matches.len(),
            other => panic!("expected palette, got {other:?}"),
        };
        let Some(Overlay::Palette(p)) = &app.overlay else {
            panic!("palette closed")
        };
        assert_eq!(p.query.as_str(), "xdemo", "⌥← moves, it does not type 'b'");
        assert_eq!(matched(&app), 0, "the edit re-ran the filter");

        // Ctrl+W kills the word back to an empty query, which matches all.
        press(
            &mut app,
            KeyCode::Char('e'),
            KeyModifiers::CONTROL,
            &mut out,
        );
        press(
            &mut app,
            KeyCode::Char('w'),
            KeyModifiers::CONTROL,
            &mut out,
        );
        let Some(Overlay::Palette(p)) = &app.overlay else {
            panic!("palette closed")
        };
        assert_eq!(p.query.as_str(), "");
        assert!(matched(&app) > 0, "clearing the query restores every row");
    }

    /// Resting the worktree selection arms the debounced prewarm; firing it
    /// sends one PrewarmWorktreeSessions plus the standing default-spec
    /// Claude keep-warm for that worktree, then disarms.
    #[test]
    fn worktree_move_arms_prewarm_and_fire_sends_request() {
        use nebula_core::{Entity, ProjectId, Worktree, WorktreeId};
        let mut app = App::new();
        seed_tree(&mut app);
        hse(
            &mut app,
            ServerEvent::EntityUpserted {
                entity: Entity::Worktree(Worktree {
                    id: WorktreeId("w2".into()),
                    project_id: ProjectId("p1".into()),
                    path: "/tmp/demo-w2".into(),
                    branch: "feature".into(),
                    is_main: false,
                    pinned: false,
                    sort_order: 1,
                }),
            },
        );
        app.pending_prewarm = None;
        app.focus = Focus::Worktrees;
        let mut out = Vec::new();
        move_selection(&mut app, 1, &mut out);
        let (armed, _) = app.pending_prewarm.clone().expect("prewarm armed");
        assert_eq!(armed, WorktreeId("w2".into()));

        out.clear();
        with_default_config(|| fire_pending_prewarm(&mut app, &mut out));
        assert!(app.pending_prewarm.is_none(), "fires once, then disarms");
        assert!(matches!(
            out.as_slice(),
            [
                ClientRequest::PrewarmWorktreeSessions { worktree, .. },
                ClientRequest::PrewarmAgent {
                    worktree: agent_wt,
                    kind: AgentKind::Claude,
                    model: None,
                    effort: None,
                },
            ] if worktree == &WorktreeId("w2".into()) && agent_wt == &WorktreeId("w2".into())
        ));
        assert!(app.next_keepwarm.is_some(), "keep-warm re-send is armed");
    }

    /// The keep-warm tick re-sends the default-spec Claude prewarm for the
    /// selected worktree and re-arms itself; with nothing selected it
    /// disarms until the next worktree rest re-arms it.
    #[test]
    fn keepwarm_refires_for_selected_worktree_and_rearms() {
        with_default_config(|| {
            let mut app = App::new();
            seed_tree(&mut app);
            app.next_keepwarm = Some(std::time::Instant::now());
            let mut out = Vec::new();
            fire_keepwarm(&mut app, &mut out);
            assert!(matches!(
                out.as_slice(),
                [ClientRequest::PrewarmAgent {
                    worktree,
                    kind: AgentKind::Claude,
                    model: None,
                    effort: None,
                }] if worktree == &nebula_core::WorktreeId("w1".into())
            ));
            assert!(app.next_keepwarm.is_some(), "re-arms after sending");

            let mut empty = App::new();
            empty.next_keepwarm = Some(std::time::Instant::now());
            out.clear();
            fire_keepwarm(&mut empty, &mut out);
            assert!(out.is_empty(), "nothing selected, nothing to keep warm");
            assert!(empty.next_keepwarm.is_none(), "disarms without a worktree");
        })
    }

    /// Esc on a Claude name prompt restores the standing default-spec warm
    /// session — the submenu's off-default pick had already replaced it the
    /// moment the kind was chosen.
    #[test]
    fn esc_on_claude_name_prompt_restores_default_prewarm() {
        with_default_config(|| {
            let mut app = App::new();
            seed_tree(&mut app);
            app.overlay = Some(Overlay::Prompt(PromptDialog::new(
                "New agent (opus · high)",
                "name",
                "",
                PromptKind::NewAgent {
                    worktree: nebula_core::WorktreeId("w1".into()),
                    kind: AgentKind::Claude,
                    model: Some("opus".into()),
                    effort: Some("high".into()),
                },
            )));
            let mut out = Vec::new();
            press(&mut app, KeyCode::Esc, KeyModifiers::NONE, &mut out);
            assert!(app.overlay.is_none());
            assert!(matches!(
                out.as_slice(),
                [ClientRequest::PrewarmAgent {
                    worktree,
                    kind: AgentKind::Claude,
                    model: None,
                    effort: None,
                }] if worktree == &nebula_core::WorktreeId("w1".into())
            ));
        })
    }

    /// The startup snapshot arms the prewarm for the restored worktree, so
    /// its sessions boot before the user presses anything.
    #[test]
    fn snapshot_arms_prewarm_for_selected_worktree() {
        let mut app = App::new();
        seed_tree(&mut app);
        let tree = app.tree.clone();
        let mut fresh = App::new();
        assert!(fresh.pending_prewarm.is_none());
        hse(
            &mut fresh,
            ServerEvent::Snapshot {
                workspaces: tree.workspaces,
                active_workspace: tree.active_workspace,
                projects: tree.projects,
                worktrees: tree.worktrees,
                agents: tree.agents,
                terminals: tree.terminals,
                notes: tree.notes,
                ui_state: None,
            },
        );
        let (armed, _) = fresh.pending_prewarm.clone().expect("prewarm armed");
        assert_eq!(armed, nebula_core::WorktreeId("w1".into()));
    }

    /// The footer's right edge shows live session counts and nebula's
    /// total memory once a metrics reading arrives.
    #[test]
    fn footer_shows_session_counts_and_memory() {
        use nebula_core::{MetricsSnapshot, SessionMetrics, TerminalId};
        let mut app = App::new();
        seed_tree(&mut app);
        let mut terminal = Terminal::new(TestBackend::new(120, 30)).unwrap();
        terminal.draw(|f| ui::draw(f, &mut app)).unwrap();
        assert!(
            !buffer_text(&terminal).contains("agent ·"),
            "no readout before the first reading"
        );

        app.client_rss_bytes = 100 * 1024 * 1024;
        app.last_metrics = Some(MetricsSnapshot {
            daemon_pid: 1,
            daemon_rss_bytes: 200 * 1024 * 1024,
            system_total_bytes: 0,
            sessions: vec![
                SessionMetrics {
                    session: SessionRef::Agent(AgentId("a1".into())),
                    pid: 10,
                    rss_bytes: 700 * 1024 * 1024,
                    procs: 3,
                },
                SessionMetrics {
                    session: SessionRef::Terminal(TerminalId("t1".into())),
                    pid: 11,
                    rss_bytes: 24 * 1024 * 1024,
                    procs: 2,
                },
            ],
        });
        terminal.draw(|f| ui::draw(f, &mut app)).unwrap();
        let text = buffer_text(&terminal);
        assert!(
            text.contains("1 agent · 1 term · 1.0 GB"),
            "footer readout rendered:\n{text}"
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
                    archived_at: 0,
                    pinned: true,
                    kind: nebula_core::AgentKind::Claude,
                    model: None,
                    effort: None,
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

    /// `p` on a worktree row asks the daemon to pin it; the upsert splits
    /// the worktrees panel into PINNED and UNPINNED groups with pinned rows
    /// first, and the selection follows the row across the regroup.
    #[test]
    fn pinned_worktrees_render_in_their_own_group() {
        use nebula_core::{Entity, Worktree, WorktreeId};
        let mk_feat = |pinned: bool| ServerEvent::EntityUpserted {
            entity: Entity::Worktree(Worktree {
                id: WorktreeId("w2".into()),
                project_id: nebula_core::ProjectId("p1".into()),
                path: "/tmp/demo-worktrees/feat".into(),
                branch: "feat".into(),
                is_main: false,
                pinned,
                sort_order: 0,
            }),
        };
        let mut app = App::new();
        seed_tree(&mut app); // p1/w1(main) + agent-1
        hse(&mut app, mk_feat(false));

        app.focus = Focus::Worktrees;
        app.sel_worktree = 1; // "feat"
        let mut out = Vec::new();
        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('p'), KeyModifiers::NONE),
            &mut out,
        );
        assert!(
            matches!(
                out.last(),
                Some(ClientRequest::SetWorktreePinned { id, pinned: true, .. })
                    if id.as_str() == "w2"
            ),
            "p requests the pin: {out:?}"
        );

        hse(&mut app, mk_feat(true));
        let rows = app.visible_worktrees();
        assert_eq!(rows[0].branch, "feat", "pinned worktree sorts first");
        assert_eq!(app.worktree_group_counts(), (1, 1));
        assert_eq!(app.sel_worktree, 0, "selection follows the pinned row");
        assert_eq!(app.focus, Focus::Worktrees, "focus stays on the panel");

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
                archived_at: 0,
                pinned,
                kind: nebula_core::AgentKind::Claude,
                model: None,
                effort: None,
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
                    archived_at: 0,
                    pinned: false,
                    kind: nebula_core::AgentKind::Claude,
                    model: None,
                    effort: None,
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
                    pinned: false,
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
                    archived_at: 0,
                    pinned: false,
                    kind: nebula_core::AgentKind::Claude,
                    model: None,
                    effort: None,
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

    fn dir_names(p: &crate::app::PromptDialog) -> Vec<&str> {
        p.dirs.iter().map(|d| d.name.as_str()).collect()
    }

    #[test]
    fn tab_in_add_project_prompt_completes_paths() {
        use crate::app::{Overlay, PromptDialog, PromptKind};
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join("workspace/nebula")).unwrap();
        std::fs::create_dir_all(tmp.path().join("workspace/herdr")).unwrap();

        let mut app = App::new();
        let mut out = Vec::new();
        app.overlay = Some(Overlay::Prompt(PromptDialog::new(
            "Add project",
            "path",
            format!("{}/work", tmp.path().display()),
            PromptKind::AddProject,
        )));

        // Unambiguous: work → workspace/, and the listing follows it in.
        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE),
            &mut out,
        );
        let Some(Overlay::Prompt(p)) = &app.overlay else {
            panic!("prompt closed")
        };
        assert_eq!(p.input, format!("{}/workspace/", tmp.path().display()));
        assert_eq!(dir_names(p), vec!["herdr", "nebula"]);

        // Ambiguous: Tab makes no progress, the listing already shows both.
        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE),
            &mut out,
        );
        let Some(Overlay::Prompt(p)) = &app.overlay else {
            panic!("prompt closed")
        };
        assert_eq!(p.input, format!("{}/workspace/", tmp.path().display()));
        assert_eq!(dir_names(p), vec!["herdr", "nebula"]);

        // Typing narrows the listing; the next Tab completes fully.
        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('n'), KeyModifiers::NONE),
            &mut out,
        );
        let Some(Overlay::Prompt(p)) = &app.overlay else {
            panic!("prompt closed")
        };
        assert_eq!(dir_names(p), vec!["nebula"]);
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
    fn add_project_prompt_browses_with_arrows_and_submits_hovered() {
        use crate::app::{Overlay, PromptDialog, PromptKind};
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join("ws/beta/inner")).unwrap();
        std::fs::create_dir_all(tmp.path().join("ws/alpha/.git")).unwrap();

        let mut app = App::new();
        let mut out = Vec::new();
        app.overlay = Some(Overlay::Prompt(PromptDialog::new(
            "Add project",
            "path",
            format!("{}/ws/", tmp.path().display()),
            PromptKind::AddProject,
        )));
        let Some(Overlay::Prompt(p)) = &app.overlay else {
            panic!("prompt closed")
        };
        assert_eq!(dir_names(p), vec!["alpha", "beta"]);
        assert!(p.dirs[0].is_repo && !p.dirs[1].is_repo);
        assert_eq!(p.hover, None, "opens on the input row");

        // ↓↓ highlights beta; → dives into it and lists its children.
        for _ in 0..2 {
            handle_key(
                &mut app,
                KeyEvent::new(KeyCode::Down, KeyModifiers::NONE),
                &mut out,
            );
        }
        let Some(Overlay::Prompt(p)) = &app.overlay else {
            panic!("prompt closed")
        };
        assert_eq!(
            p.hovered_path(),
            Some(format!("{}/ws/beta", tmp.path().display()))
        );
        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Right, KeyModifiers::NONE),
            &mut out,
        );
        let Some(Overlay::Prompt(p)) = &app.overlay else {
            panic!("prompt closed")
        };
        assert_eq!(p.input, format!("{}/ws/beta/", tmp.path().display()));
        assert_eq!(dir_names(p), vec!["inner"]);
        assert_eq!(p.hover, None, "diving resets the highlight");

        // ← steps back up to ws/.
        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Left, KeyModifiers::NONE),
            &mut out,
        );
        let Some(Overlay::Prompt(p)) = &app.overlay else {
            panic!("prompt closed")
        };
        assert_eq!(p.input, format!("{}/ws/", tmp.path().display()));

        // ↓ + Enter adds the highlighted directory, not the typed parent.
        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Down, KeyModifiers::NONE),
            &mut out,
        );
        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
            &mut out,
        );
        assert!(app.overlay.is_none());
        assert!(matches!(
            out.as_slice(),
            [ClientRequest::AddProject { path, create_missing: false, .. }]
                if path == &tmp.path().join("ws/alpha")
        ));
    }

    #[test]
    fn add_project_prefill_yields_to_absolute_paths() {
        use crate::app::{Overlay, PromptDialog, PromptKind};
        let mut app = App::new();
        let mut out = Vec::new();
        app.overlay = Some(Overlay::Prompt(PromptDialog::new(
            "Add project",
            "path",
            "~/",
            PromptKind::AddProject,
        )));
        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('/'), KeyModifiers::NONE),
            &mut out,
        );
        let Some(Overlay::Prompt(p)) = &app.overlay else {
            panic!("prompt closed")
        };
        assert_eq!(p.input, "/", "leading '/' replaces the untouched prefill");
    }

    #[test]
    fn tab_in_name_prompt_does_not_complete() {
        use crate::app::{Overlay, PromptDialog, PromptKind};
        let mut app = App::new();
        seed_tree(&mut app);
        let mut out = Vec::new();
        app.overlay = Some(Overlay::Prompt(PromptDialog::new(
            "Rename agent",
            "name",
            "src", // a dir that exists in cwd — must NOT complete
            PromptKind::RenameAgent {
                id: nebula_core::AgentId("a1".into()),
            },
        )));
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

    /// Picker/submenu tests resolve model/effort through `Config::load`, so
    /// pin the config to an empty temp file to stay off the dev's real one.
    fn with_default_config<T>(f: impl FnOnce() -> T) -> T {
        let dir = tempfile::tempdir().unwrap();
        crate::config::with_config_path(dir.path().join("config.json"), f)
    }

    #[test]
    fn n_in_sessions_opens_agent_type_picker_then_prompt() {
        with_default_config(|| {
            let mut app = App::new();
            seed_tree(&mut app);
            app.focus = Focus::Sessions;
            let mut out = Vec::new();

            press(&mut app, KeyCode::Char('n'), KeyModifiers::NONE, &mut out);
            let Some(Overlay::Menu(menu)) = &app.overlay else {
                panic!("expected agent-type picker, got {:?}", app.overlay);
            };
            assert_eq!(menu.title.as_deref(), Some("New session"));
            assert_eq!(menu.items.len(), 4);
            assert_eq!(menu.items[0].label, "Claude");
            assert_eq!(menu.items[1].label, "Codex");
            assert_eq!(menu.items[2].label, "Cursor");
            assert_eq!(menu.items[3].label, "Terminal (shell)");
            assert_eq!(menu.hover, 0, "Claude is the default");

            // Enter on the default chains into the name prompt with
            // kind=Claude, and fires the prewarm so the CLI boots while the
            // user types. Nothing configured → no model/effort flags.
            press(&mut app, KeyCode::Enter, KeyModifiers::NONE, &mut out);
            assert!(matches!(
                out.last(),
                Some(ClientRequest::PrewarmAgent {
                    kind: AgentKind::Claude,
                    model: None,
                    effort: None,
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
                    model: None,
                    effort: None,
                    ..
                }
            ));

            // Accepting the empty prompt falls back to the next free default
            // name, and the consumed warm slot is refilled right behind the
            // create so the next one adopts a booted CLI too.
            press(&mut app, KeyCode::Enter, KeyModifiers::NONE, &mut out);
            assert!(app.overlay.is_none());
            assert!(matches!(
                &out[out.len() - 2],
                ClientRequest::CreateAgent { name, kind: AgentKind::Claude, model: None, effort: None, .. } if name == "agent-2"
            ));
            assert!(matches!(
                out.last(),
                Some(ClientRequest::PrewarmAgent {
                    kind: AgentKind::Claude,
                    model: None,
                    effort: None,
                    ..
                })
            ));
        })
    }

    #[test]
    fn picker_right_drills_into_model_then_effort_submenus() {
        with_default_config(|| {
            let mut app = App::new();
            seed_tree(&mut app);
            app.focus = Focus::Sessions;
            let mut out = Vec::new();

            press(&mut app, KeyCode::Char('n'), KeyModifiers::NONE, &mut out);
            let Some(Overlay::Menu(menu)) = &app.overlay else {
                panic!("expected picker, got {:?}", app.overlay);
            };
            // Claude/Codex rows advertise a submenu (the ▸ affordance);
            // Cursor and Terminal don't.
            assert_eq!(menu.items[0].action.submenu(), Some(SubmenuKind::Models));
            assert_eq!(menu.items[1].action.submenu(), Some(SubmenuKind::Models));
            assert_eq!(menu.items[2].action.submenu(), None);
            assert_eq!(menu.items[3].action.submenu(), None);

            // → opens the model list; nothing configured, so the "default"
            // row is checked and highlighted, and the parent is kept for ←.
            press(&mut app, KeyCode::Right, KeyModifiers::NONE, &mut out);
            let Some(Overlay::Menu(menu)) = &app.overlay else {
                panic!("expected model submenu, got {:?}", app.overlay);
            };
            assert_eq!(menu.title.as_deref(), Some("Claude model"));
            assert_eq!(menu.items.len(), crate::config::CLAUDE_MODELS.len());
            assert_eq!(menu.items[0].label, "default ✓");
            assert_eq!(menu.items[2].label, "opus");
            assert_eq!(menu.hover, 0);
            assert!(menu.parent.is_some());
            // Model rows drill further into the effort list…
            assert_eq!(menu.items[2].action.submenu(), Some(SubmenuKind::Efforts));

            // …so ↓↓ to opus, → again: efforts for that model.
            press(&mut app, KeyCode::Down, KeyModifiers::NONE, &mut out);
            press(&mut app, KeyCode::Down, KeyModifiers::NONE, &mut out);
            press(&mut app, KeyCode::Right, KeyModifiers::NONE, &mut out);
            let Some(Overlay::Menu(menu)) = &app.overlay else {
                panic!("expected effort submenu, got {:?}", app.overlay);
            };
            assert_eq!(menu.title.as_deref(), Some("Claude effort"));
            assert_eq!(menu.items.len(), crate::config::CLAUDE_EFFORTS.len());
            assert!(matches!(
                &menu.items[3].action,
                MenuAction::NewAgentOfKind { kind: AgentKind::Claude, model: Some(m), effort: Some(e), .. }
                    if m == "opus" && e == "high"
            ));
            // Effort rows are leaves.
            assert_eq!(menu.items[3].action.submenu(), None);

            // ← backs out to the models; Esc also backs out one level, and
            // only closes from the top.
            press(&mut app, KeyCode::Left, KeyModifiers::NONE, &mut out);
            let Some(Overlay::Menu(menu)) = &app.overlay else {
                panic!("expected model submenu after ←");
            };
            assert_eq!(menu.title.as_deref(), Some("Claude model"));
            assert_eq!(menu.hover, 2, "← restores the parent's hover");
            press(&mut app, KeyCode::Esc, KeyModifiers::NONE, &mut out);
            let Some(Overlay::Menu(menu)) = &app.overlay else {
                panic!("expected root picker after Esc");
            };
            assert_eq!(menu.title.as_deref(), Some("New session"));
            press(&mut app, KeyCode::Esc, KeyModifiers::NONE, &mut out);
            assert!(app.overlay.is_none());
            assert!(
                !out.iter()
                    .any(|r| matches!(r, ClientRequest::CreateAgent { .. })),
                "browsing submenus must not create anything"
            );
        })
    }

    #[test]
    fn picker_enter_on_effort_row_carries_model_and_effort() {
        with_default_config(|| {
            let mut app = App::new();
            seed_tree(&mut app);
            app.focus = Focus::Sessions;
            let mut out = Vec::new();

            // n → Codex row → models → gpt-5.5 → efforts → minimal → Enter.
            press(&mut app, KeyCode::Char('n'), KeyModifiers::NONE, &mut out);
            press(&mut app, KeyCode::Down, KeyModifiers::NONE, &mut out);
            press(&mut app, KeyCode::Right, KeyModifiers::NONE, &mut out);
            let Some(Overlay::Menu(menu)) = &app.overlay else {
                panic!("expected codex model submenu");
            };
            assert_eq!(menu.title.as_deref(), Some("Codex model"));
            press(&mut app, KeyCode::Down, KeyModifiers::NONE, &mut out);
            press(&mut app, KeyCode::Down, KeyModifiers::NONE, &mut out);
            press(&mut app, KeyCode::Right, KeyModifiers::NONE, &mut out);
            press(&mut app, KeyCode::Down, KeyModifiers::NONE, &mut out);
            press(&mut app, KeyCode::Enter, KeyModifiers::NONE, &mut out);

            assert!(matches!(
                out.last(),
                Some(ClientRequest::PrewarmAgent {
                    kind: AgentKind::Codex,
                    model: Some(m),
                    effort: Some(e),
                    ..
                }) if m == "gpt-5.5" && e == "minimal"
            ));
            let Some(Overlay::Prompt(p)) = &app.overlay else {
                panic!("expected name prompt, got {:?}", app.overlay);
            };
            assert_eq!(p.title, "New agent (gpt-5.5 · minimal)");
            press(&mut app, KeyCode::Enter, KeyModifiers::NONE, &mut out);
            assert!(matches!(
                out.last(),
                Some(ClientRequest::CreateAgent {
                    kind: AgentKind::Codex,
                    model: Some(m),
                    effort: Some(e),
                    ..
                }) if m == "gpt-5.5" && e == "minimal"
            ));
        })
    }

    #[test]
    fn picker_resolves_configured_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        std::fs::write(
            &path,
            r#"{"claude_model": "sonnet", "claude_effort": "max"}"#,
        )
        .unwrap();
        crate::config::with_config_path(path, || {
            let mut app = App::new();
            seed_tree(&mut app);
            app.focus = Focus::Sessions;
            let mut out = Vec::new();

            // Enter straight on the Claude row: both settings apply.
            press(&mut app, KeyCode::Char('n'), KeyModifiers::NONE, &mut out);
            press(&mut app, KeyCode::Enter, KeyModifiers::NONE, &mut out);
            assert!(matches!(
                out.last(),
                Some(ClientRequest::PrewarmAgent {
                    kind: AgentKind::Claude,
                    model: Some(m),
                    effort: Some(e),
                    ..
                }) if m == "sonnet" && e == "max"
            ));
            let Some(Overlay::Prompt(p)) = &app.overlay else {
                panic!("expected name prompt");
            };
            assert_eq!(p.title, "New agent (sonnet · max)");
            press(&mut app, KeyCode::Esc, KeyModifiers::NONE, &mut out);

            // The model submenu highlights and checks the configured model,
            // and its explicit "default" row resolves to the same setting.
            press(&mut app, KeyCode::Char('n'), KeyModifiers::NONE, &mut out);
            press(&mut app, KeyCode::Right, KeyModifiers::NONE, &mut out);
            let Some(Overlay::Menu(menu)) = &app.overlay else {
                panic!("expected model submenu");
            };
            assert_eq!(menu.items[3].label, "sonnet ✓");
            assert_eq!(menu.hover, 3, "hover starts on the configured model");
            press(&mut app, KeyCode::Up, KeyModifiers::NONE, &mut out);
            press(&mut app, KeyCode::Up, KeyModifiers::NONE, &mut out);
            press(&mut app, KeyCode::Up, KeyModifiers::NONE, &mut out);
            press(&mut app, KeyCode::Enter, KeyModifiers::NONE, &mut out);
            assert!(matches!(
                out.last(),
                Some(ClientRequest::PrewarmAgent {
                    model: Some(m),
                    effort: Some(e),
                    ..
                }) if m == "sonnet" && e == "max"
            ));
        })
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
            Some(Overlay::Menu(m)) if m.title.as_deref() == Some("New session")
        ));
    }

    fn seed_terminal(app: &mut App, id: &str, name: &str) {
        use nebula_core::{Entity, TerminalId, TerminalTab, WorktreeId};
        hse(
            app,
            ServerEvent::EntityUpserted {
                entity: Entity::Terminal(TerminalTab {
                    id: TerminalId(id.into()),
                    worktree_id: WorktreeId("w1".into()),
                    name: name.into(),
                    sort_order: 0,
                    alive: true,
                }),
            },
        );
    }

    #[test]
    fn shift_t_creates_terminal_in_selected_worktree() {
        use nebula_core::WorktreeId;
        let mut app = App::new();
        seed_tree(&mut app);
        app.focus = Focus::Sessions;
        let mut out = Vec::new();

        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('T'), KeyModifiers::SHIFT),
            &mut out,
        );
        assert!(matches!(
            out.last(),
            Some(ClientRequest::CreateTerminal { worktree, name: None, .. })
                if worktree == &WorktreeId("w1".into())
        ));
    }

    /// From the Projects panel, Shift+T targets the project's main checkout
    /// (root), not whatever worktree row happens to be selected.
    #[test]
    fn shift_t_from_projects_targets_the_root_checkout() {
        use nebula_core::{Entity, Worktree, WorktreeId};
        let mut app = App::new();
        seed_tree(&mut app);
        hse(
            &mut app,
            ServerEvent::EntityUpserted {
                entity: Entity::Worktree(Worktree {
                    id: WorktreeId("w2".into()),
                    project_id: nebula_core::ProjectId("p1".into()),
                    path: "/tmp/demo-worktrees/feat".into(),
                    branch: "feat".into(),
                    is_main: false,
                    pinned: false,
                    sort_order: 1,
                }),
            },
        );
        app.sel_worktree = 1; // the feat worktree
        app.focus = Focus::Projects;
        let mut out = Vec::new();

        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('T'), KeyModifiers::SHIFT),
            &mut out,
        );
        assert!(matches!(
            out.last(),
            Some(ClientRequest::CreateTerminal { worktree, .. })
                if worktree == &WorktreeId("w1".into())
        ));
    }

    /// The CreateTerminal Ack attaches the new terminal, and its upsert
    /// lands the selection on the new row.
    #[test]
    fn create_terminal_ack_attaches_and_selects_it() {
        use nebula_core::TerminalId;
        let mut app = App::new();
        seed_tree(&mut app);
        app.focus = Focus::Sessions;
        let mut out = Vec::new();

        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('T'), KeyModifiers::SHIFT),
            &mut out,
        );
        let Some(ClientRequest::CreateTerminal { req_id, .. }) = out.last() else {
            panic!("expected CreateTerminal, got {:?}", out.last());
        };
        let req_id = *req_id;

        // The daemon broadcasts the upsert before it replies with the Ack.
        seed_terminal(&mut app, "t1", "term-1");
        hse(
            &mut app,
            ServerEvent::Ack {
                req_id,
                created: Some(EntityId::Terminal(TerminalId("t1".into()))),
            },
        );
        assert_eq!(
            app.term.as_ref().map(|t| t.sref.clone()),
            Some(SessionRef::Terminal(TerminalId("t1".into())))
        );
        assert_eq!(app.focus, Focus::Terminal);
        assert!(app.term_locked, "a created terminal takes the input lock");
        assert_eq!(app.sel_session, 1, "selection follows the new terminal row");
    }

    #[test]
    fn terminal_rows_render_under_terminals_header() {
        let mut app = App::new();
        seed_tree(&mut app);
        seed_terminal(&mut app, "t1", "term-1");

        let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
        terminal.draw(|f| ui::draw(f, &mut app)).unwrap();
        let text = buffer_text(&terminal);
        assert!(text.contains("TERMINALS"), "terminals header:\n{text}");
        assert!(text.contains("term-1"), "terminal row rendered:\n{text}");
    }

    #[test]
    fn enter_on_terminal_row_attaches_it() {
        use nebula_core::TerminalId;
        let mut app = App::new();
        seed_tree(&mut app);
        seed_terminal(&mut app, "t1", "term-1");
        app.focus = Focus::Sessions;
        app.sel_session = 1; // agent-1 first, then the terminal
        let mut out = Vec::new();

        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
            &mut out,
        );
        assert!(out.iter().any(|r| matches!(
            r,
            ClientRequest::Attach { session: SessionRef::Terminal(id), .. }
                if id == &TerminalId("t1".into())
        )));
        assert_eq!(app.focus, Focus::Terminal);
        assert!(app.term_locked);
    }

    #[test]
    fn d_on_terminal_row_confirms_then_closes() {
        use nebula_core::TerminalId;
        let mut app = App::new();
        seed_tree(&mut app);
        seed_terminal(&mut app, "t1", "term-1");
        app.focus = Focus::Sessions;
        app.sel_session = 1;
        let mut out = Vec::new();

        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE),
            &mut out,
        );
        assert!(matches!(
            &app.overlay,
            Some(Overlay::Confirm(c)) if matches!(c.action, PendingAction::CloseTerminal(_))
        ));

        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('y'), KeyModifiers::NONE),
            &mut out,
        );
        assert!(matches!(
            out.last(),
            Some(ClientRequest::CloseTerminal { id, .. }) if id == &TerminalId("t1".into())
        ));
    }

    #[test]
    fn r_on_terminal_row_renames_it() {
        use nebula_core::TerminalId;
        let mut app = App::new();
        seed_tree(&mut app);
        seed_terminal(&mut app, "t1", "term-1");
        app.focus = Focus::Sessions;
        app.sel_session = 1;
        let mut out = Vec::new();

        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('r'), KeyModifiers::NONE),
            &mut out,
        );
        let Some(Overlay::Prompt(p)) = &app.overlay else {
            panic!("expected rename prompt, got {:?}", app.overlay);
        };
        assert_eq!(p.title, "Rename terminal");
        assert_eq!(p.input, "term-1", "prompt starts from the current name");

        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
            &mut out,
        );
        assert!(matches!(
            out.last(),
            Some(ClientRequest::RenameTerminal { id, name, .. })
                if id == &TerminalId("t1".into()) && name == "term-1"
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

    /// Plain →/l stops at the Sessions panel: crossing into the terminal
    /// pane means the user chose a session, which is Enter's job (Tab and
    /// Ctrl+→ still reach the pane deliberately).
    #[test]
    fn plain_right_stops_at_sessions() {
        let mut app = App::new();
        seed_tree(&mut app);
        app.term = Some(AttachedTerm::new(
            SessionRef::Agent(AgentId("a1".into())),
            80,
            24,
        ));
        app.focus = Focus::Sessions;
        let mut out = Vec::new();

        press(&mut app, KeyCode::Right, KeyModifiers::NONE, &mut out);
        assert_eq!(app.focus, Focus::Sessions, "→ must not enter the pane");
        press(&mut app, KeyCode::Char('l'), KeyModifiers::NONE, &mut out);
        assert_eq!(app.focus, Focus::Sessions, "l must not enter the pane");
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
                archived_at: 0,
                pinned: false,
                kind: nebula_core::AgentKind::Claude,
                model: None,
                effort: None,
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

    fn archived_agent(id: &str, name: &str, archived_at: i64, sort: i64) -> nebula_core::Entity {
        use nebula_core::{Agent, AgentStatus, Entity, WorktreeId};
        Entity::Agent(Agent {
            id: AgentId(id.into()),
            worktree_id: WorktreeId("w1".into()),
            name: name.into(),
            status: AgentStatus::Fresh,
            archived: true,
            archived_at,
            pinned: false,
            kind: nebula_core::AgentKind::Claude,
            model: None,
            effort: None,
            session_id: None,
            sort_order: sort,
            status_changed_at: 0,
            alive: false,
        })
    }

    /// The ARCHIVED group lists the most recently archived session first;
    /// never-stamped legacy rows (archived_at == 0) sink to the bottom.
    #[test]
    fn archived_group_orders_newest_first() {
        let mut app = App::new();
        seed_tree(&mut app);
        for ev in [
            archived_agent("old", "old", 100, 1),
            archived_agent("newest", "newest", 300, 2),
            archived_agent("mid", "mid", 200, 3),
            archived_agent("legacy", "legacy", 0, 4),
        ] {
            hse(&mut app, ServerEvent::EntityUpserted { entity: ev });
        }
        app.show_archived = true;
        let names: Vec<String> = app
            .visible_session_rows()
            .iter()
            .filter(|r| r.is_archived_agent())
            .map(|r| match r {
                SessionRow::Agent(a) => a.name.clone(),
                SessionRow::Terminal(_) => unreachable!(),
            })
            .collect();
        assert_eq!(names, ["newest", "mid", "old", "legacy"]);
    }

    /// Collapsing the ARCHIVED group (A) while the cursor sits on an
    /// archived row re-lands it on a surviving row instead of leaving it
    /// dangling past the end of the list.
    #[test]
    fn collapsing_archived_relands_the_cursor() {
        let mut app = App::new();
        seed_tree(&mut app);
        hse(
            &mut app,
            ServerEvent::EntityUpserted {
                entity: archived_agent("a9", "old-agent", 100, 9),
            },
        );
        app.show_archived = true;
        app.focus = Focus::Sessions;
        let mut out = Vec::new();
        press(&mut app, KeyCode::Down, KeyModifiers::NONE, &mut out);
        assert!(
            app.selected_session_row()
                .is_some_and(|r| r.is_archived_agent()),
            "cursor sits on the archived row"
        );

        press(&mut app, KeyCode::Char('A'), KeyModifiers::SHIFT, &mut out);
        assert!(!app.show_archived, "A collapses the group");
        assert_eq!(
            app.selected_session().map(|a| a.name),
            Some("agent-1".into()),
            "cursor lands on a surviving row"
        );
    }

    /// An ARCHIVED group taller than the panel scrolls: the wheel moves the
    /// viewport without touching the cursor, and walking the cursor down
    /// drags the viewport along so the selected row never falls off the
    /// bottom edge.
    #[test]
    fn archived_list_scrolls_by_wheel_and_follows_the_cursor() {
        let mut app = App::new();
        seed_tree(&mut app);
        for i in 0..20i64 {
            hse(
                &mut app,
                ServerEvent::EntityUpserted {
                    entity: archived_agent(
                        &format!("z{i}"),
                        &format!("archived-{i:02}"),
                        1000 - i,
                        i + 1,
                    ),
                },
            );
        }
        app.show_archived = true;
        app.focus = Focus::Sessions;
        let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
        let mut out = Vec::new();

        terminal.draw(|f| ui::draw(f, &mut app)).unwrap();
        let text = buffer_text(&terminal);
        assert!(text.contains("archived-00"), "list starts at the top: {text}");
        assert!(!text.contains("archived-19"), "tail overflows: {text}");

        // Wheel over the Sessions column: the list moves, the cursor stays.
        for _ in 0..12 {
            handle_mouse(&mut app, mev(MouseEventKind::ScrollDown, 50, 10), &mut out);
            terminal.draw(|f| ui::draw(f, &mut app)).unwrap();
        }
        let text = buffer_text(&terminal);
        assert!(text.contains("archived-19"), "wheel reaches the tail: {text}");
        assert!(!text.contains("archived-00"), "top scrolled away: {text}");
        assert_eq!(app.sel_session, 0, "the wheel never moves the cursor");

        // Scrolling back up stops at the top instead of running away.
        for _ in 0..40 {
            handle_mouse(&mut app, mev(MouseEventKind::ScrollUp, 50, 10), &mut out);
            terminal.draw(|f| ui::draw(f, &mut app)).unwrap();
        }
        assert_eq!(app.sessions_scroll, 0, "wheel-up clamps at the top");

        // ↓ to the last archived row pulls the viewport down with it.
        for _ in 0..20 {
            press(&mut app, KeyCode::Down, KeyModifiers::NONE, &mut out);
        }
        assert_eq!(app.sel_session, 20, "cursor walks onto the last row");
        terminal.draw(|f| ui::draw(f, &mut app)).unwrap();
        let text = buffer_text(&terminal);
        assert!(
            text.contains("archived-19"),
            "the selected row is on screen: {text}"
        );

        // …and ↑ back to the first row pulls it back.
        for _ in 0..20 {
            press(&mut app, KeyCode::Up, KeyModifiers::NONE, &mut out);
        }
        assert_eq!(app.sel_session, 0);
        terminal.draw(|f| ui::draw(f, &mut app)).unwrap();
        let text = buffer_text(&terminal);
        assert!(text.contains("agent-1"), "back at the top: {text}");
        assert_eq!(app.sessions_scroll, 0);
    }

    /// Clicking the ARCHIVED header toggles the group open/closed, same as
    /// the A key.
    #[test]
    fn clicking_the_archived_header_toggles_the_group() {
        let mut app = App::new();
        seed_tree(&mut app);
        let mut out = Vec::new();
        app.hits.push((
            ratatui::layout::Rect::new(0, 5, 20, 1),
            HitTarget::ArchivedHeader,
        ));
        handle_mouse(
            &mut app,
            mev(MouseEventKind::Down(MouseButton::Left), 1, 5),
            &mut out,
        );
        assert!(app.show_archived, "click on the header expands");
        assert_eq!(app.focus, Focus::Sessions);
        handle_mouse(
            &mut app,
            mev(MouseEventKind::Down(MouseButton::Left), 1, 5),
            &mut out,
        );
        assert!(!app.show_archived, "second click collapses");
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
    fn single_click_previews_double_click_focuses() {
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
            out.iter()
                .any(|r| matches!(r, ClientRequest::Attach { .. })),
            "single click previews the session's terminal"
        );
        assert_eq!(app.focus, Focus::Sessions, "single click keeps list focus");
        assert!(!app.term_locked, "preview never takes the input lock");
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
        assert_eq!(app.focus, Focus::Terminal, "double-click focuses terminal");
        assert!(app.term_locked, "double-click locks input");
        assert!(
            app.last_session_click.is_none(),
            "double-click consumed the click state, a third click starts over"
        );
    }

    #[test]
    fn slow_second_click_on_session_row_previews_without_focusing() {
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
            SessionRef::Agent(AgentId("a1".into())),
        ));
        handle_mouse(
            &mut app,
            mev(MouseEventKind::Down(MouseButton::Left), 1, 0),
            &mut out,
        );
        assert_eq!(app.focus, Focus::Sessions, "slow click keeps list focus");
        assert!(!app.term_locked, "slow click never takes the input lock");
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

    #[test]
    fn alt_click_on_file_path_resolves_against_attached_worktree() {
        let mut app = App::new();
        seed_tree(&mut app);
        let mut out = Vec::new();

        // Attach a1 (worktree /tmp/demo); the printed path doesn't exist
        // there, so the click reports it instead of spawning an editor.
        let sref = SessionRef::Agent(AgentId("a1".into()));
        let mut term = AttachedTerm::new(sref, 80, 24);
        term.parser.process(b"edited src/nope.rs:12 just now");
        app.term = Some(term);
        app.term_area = ratatui::layout::Rect::new(0, 0, 80, 24);
        app.hits.push((app.term_area, HitTarget::TerminalPane));
        app.term_file_links =
            crate::links::visible_file_links(app.term.as_ref().unwrap().parser.screen());
        assert_eq!(app.term_file_links.len(), 1);

        let alt = |column| MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column,
            row: 0,
            modifiers: KeyModifiers::ALT,
        };

        app.focus = Focus::Projects;
        handle_mouse(&mut app, alt(9), &mut out);
        assert_eq!(app.flash.as_deref(), Some("file not found: src/nope.rs"));
        assert!(app.vim.is_none());
        assert_eq!(app.focus, Focus::Projects, "the click is swallowed");
        assert!(app.term_selection.is_none(), "no selection armed");
    }

    #[test]
    fn resolve_file_link_handles_diff_prefixes_and_absolutes() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("src/app.rs"), "").unwrap();

        assert_eq!(
            resolve_file_link(root, "src/app.rs").as_deref(),
            Some("src/app.rs"),
            "relative paths stay relative (editor cwd is the worktree)"
        );
        assert_eq!(
            resolve_file_link(root, "a/src/app.rs").as_deref(),
            Some("src/app.rs"),
            "git-diff a/ prefix is stripped when the raw path is missing"
        );
        let abs = root.join("src/app.rs");
        assert_eq!(
            resolve_file_link(root, abs.to_str().unwrap()).as_deref(),
            abs.to_str(),
            "absolute paths pass through"
        );
        assert_eq!(resolve_file_link(root, "src/nope.rs"), None);
        assert_eq!(
            resolve_file_link(root, "src"),
            None,
            "directories don't open"
        );
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

    /// A wheel tick over an app that enabled mouse reporting (claude's
    /// alt-screen UI, vim `mouse=a`, htop) forwards the wheel event itself.
    /// Synthesized arrows would land in claude's input box, cycling prompt
    /// history and tripping its "Scroll wheel is sending arrow keys" hint.
    #[test]
    fn wheel_forwards_mouse_report_when_child_wants_mouse() {
        let mut app = App::new();
        let mut out = Vec::new();

        let sref = SessionRef::Agent(AgentId("a1".into()));
        let mut term = AttachedTerm::new(sref, 80, 24);
        // Claude's alt-screen entry: 1049 + tracking modes + SGR encoding.
        term.parser
            .process(b"\x1b[?1049h\x1b[?1000h\x1b[?1002h\x1b[?1003h\x1b[?1006h");
        app.term = Some(term);
        app.term_area = ratatui::layout::Rect::new(0, 0, 80, 24);
        app.hits.push((app.term_area, HitTarget::TerminalPane));

        handle_mouse(&mut app, mev(MouseEventKind::ScrollUp, 10, 5), &mut out);
        match out.as_slice() {
            [ClientRequest::Input { data, .. }] => assert_eq!(
                data, b"\x1b[<64;11;6M",
                "wheel-up becomes an SGR report at the 1-based pane cell"
            ),
            other => panic!("expected one Input request, got {other:?}"),
        }

        out.clear();
        handle_mouse(&mut app, mev(MouseEventKind::ScrollDown, 0, 0), &mut out);
        match out.as_slice() {
            [ClientRequest::Input { data, .. }] => assert_eq!(data, b"\x1b[<65;1;1M"),
            other => panic!("expected one Input request, got {other:?}"),
        }
    }

    /// Alt-screen apps that never asked for the mouse (plain vim, less) keep
    /// the arrow-key emulation.
    #[test]
    fn wheel_sends_arrows_to_mouseless_alt_screen_apps() {
        let mut app = App::new();
        let mut out = Vec::new();

        let sref = SessionRef::Agent(AgentId("a1".into()));
        let mut term = AttachedTerm::new(sref, 80, 24);
        term.parser.process(b"\x1b[?1049h");
        app.term = Some(term);
        app.term_area = ratatui::layout::Rect::new(0, 0, 80, 24);
        app.hits.push((app.term_area, HitTarget::TerminalPane));

        handle_mouse(&mut app, mev(MouseEventKind::ScrollUp, 10, 5), &mut out);
        match out.as_slice() {
            [ClientRequest::Input { data, .. }] => assert_eq!(data, b"\x1b[A\x1b[A\x1b[A"),
            other => panic!("expected one Input request, got {other:?}"),
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
    fn pointer_shape_tracks_splitter_hover() {
        let mut app = App::new();
        seed_splitters(&mut app);
        let mut out = Vec::new();

        // Hover onto the projects|worktrees boundary: col-resize + grip lit.
        app.dirty = false;
        handle_mouse(&mut app, mev(MouseEventKind::Moved, 20, 5), &mut out);
        assert_eq!(app.pointer_shape, PointerShape::ColResize);
        assert_eq!(app.hover_splitter, Some(0));
        assert!(app.dirty, "hover change repaints the grip");

        // Hover away: back to default, grip resting.
        app.dirty = false;
        handle_mouse(&mut app, mev(MouseEventKind::Moved, 5, 5), &mut out);
        assert_eq!(app.pointer_shape, PointerShape::Default);
        assert_eq!(app.hover_splitter, None);
        assert!(app.dirty);

        // Motion with nothing to change must not schedule repaints.
        app.dirty = false;
        handle_mouse(&mut app, mev(MouseEventKind::Moved, 6, 5), &mut out);
        assert!(!app.dirty);
    }

    #[test]
    fn pointer_shape_holds_while_dragging_past_the_boundary() {
        let mut app = App::new();
        seed_splitters(&mut app);
        let mut out = Vec::new();

        handle_mouse(
            &mut app,
            mev(MouseEventKind::Down(MouseButton::Left), 20, 5),
            &mut out,
        );
        assert_eq!(app.pointer_shape, PointerShape::ColResize);

        // Mid-drag the cursor outruns the grab zone; the drag keeps the
        // resize shape (and the grip highlight) anyway.
        handle_mouse(
            &mut app,
            mev(MouseEventKind::Drag(MouseButton::Left), 60, 5),
            &mut out,
        );
        assert_eq!(app.pointer_shape, PointerShape::ColResize);
        assert_eq!(app.hover_splitter, Some(0));
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
            workspace_id: Default::default(),
            id: ProjectId(id.into()),
            name: name.into(),
            repo_path: format!("/tmp/{name}").into(),
            sort_order,
            divider_after,
            divider_label: divider_label.map(String::from),
            divider_before: false,
            divider_before_label: None,
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
                    pinned: false,
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
                    archived_at: 0,
                    pinned: false,
                    kind: nebula_core::AgentKind::Claude,
                    model: None,
                    effort: None,
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
                    before: false,
                    present: true,
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
        assert_eq!(
            app.selected_project_row(),
            Some(ProjectRow::Divider {
                project: 0,
                before: false
            })
        );
        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('J'), KeyModifiers::SHIFT),
            &mut out,
        );
        assert!(
            matches!(
                out.last(),
                Some(ClientRequest::MoveDivider {
                    before: false,
                    delta: 1,
                    ..
                })
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
        assert_eq!(
            app.selected_project_row(),
            Some(ProjectRow::Divider {
                project: 1,
                before: false
            })
        );
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
            pinned: false,
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
                    pinned: false,
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
        // Borderless column: row 0 a top-padding spacer, row 1 the header,
        // row 2 a spacer, rows 3-5 the 3-tall project button (selected in
        // the focused panel → accent ▌ rail down its edge, name on the
        // middle row), row 6 the divider behind a 1-cell gutter.
        let lines: Vec<&str> = text.lines().collect();
        assert!(
            lines[1].starts_with("   PROJECTS"),
            "column header first:\n{text}"
        );
        assert!(
            lines[3].starts_with("▌ ") && lines[5].starts_with("▌ "),
            "selection rail spans the project button:\n{text}"
        );
        assert!(
            lines[4].starts_with("▌● demo"),
            "project name centered in the button:\n{text}"
        );
        assert!(
            lines[6].starts_with(&format!(" {}", "─".repeat(10))),
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
            text.lines().nth(6).unwrap().starts_with(" ─ work ──"),
            "labeled divider row:\n{text}"
        );
    }

    /// The selection rail on a worktree/session pill runs the pill's full
    /// visual height — quadrant caps on the half-block pad rows, `▌` on
    /// the text row — and sessions share the worktrees' 2-row pill stride
    /// so the two lists read uniformly.
    #[test]
    fn pill_rail_spans_pads_and_sessions_match_worktree_stride() {
        use nebula_core::{Agent, AgentStatus, Entity, WorktreeId};
        let mut app = App::new();
        seed_tree(&mut app);
        // A second session proves the stride between session rows.
        hse(
            &mut app,
            ServerEvent::EntityUpserted {
                entity: Entity::Agent(Agent {
                    id: AgentId("a2".into()),
                    worktree_id: WorktreeId("w1".into()),
                    name: "agent-2".into(),
                    status: AgentStatus::Fresh,
                    archived: false,
                    archived_at: 0,
                    pinned: false,
                    kind: nebula_core::AgentKind::Claude,
                    model: None,
                    effort: None,
                    session_id: None,
                    sort_order: 1,
                    status_changed_at: 0,
                    alive: true,
                }),
            },
        );
        app.focus = Focus::Worktrees;
        let mut terminal = Terminal::new(TestBackend::new(120, 30)).unwrap();
        terminal.draw(|f| ui::draw(f, &mut app)).unwrap();
        let text = buffer_text(&terminal);
        let lines: Vec<&str> = text.lines().collect();

        // Char column of `needle` in `line` (buffer glyphs are multi-byte,
        // so byte offsets from find() need converting).
        let char_col =
            |line: &str, needle: &str| line.find(needle).map(|b| line[..b].chars().count());
        let at = |row: usize, col: usize| lines[row].chars().nth(col);
        // rail col ▌, then dot + name: the rail sits one cell left of the dot.
        let rail_check = |name: &str, text: &str, lines: &Vec<&str>| {
            let dot = format!("● {name}");
            let row = lines
                .iter()
                .position(|l| l.contains(&dot))
                .unwrap_or_else(|| panic!("row {name:?} not on screen:\n{text}"));
            let col = char_col(lines[row], &dot).unwrap() - 1;
            assert_eq!(
                at(row, col),
                Some('▌'),
                "rail on {name}'s text row:\n{text}"
            );
            assert_eq!(
                at(row - 1, col),
                Some('▖'),
                "rail cap on {name}'s top pad:\n{text}"
            );
            assert_eq!(
                at(row + 1, col),
                Some('▘'),
                "rail cap on {name}'s bottom pad:\n{text}"
            );
            row
        };
        rail_check("main", &text, &lines);

        // Sessions panel (unfocused, still selected → dim rail, same caps),
        // and the second row sits exactly one pill stride below the first.
        let a1_row = rail_check("agent-1", &text, &lines);
        let a2_row = lines
            .iter()
            .position(|l| l.contains("● agent-2"))
            .unwrap_or_else(|| panic!("agent-2 row not on screen:\n{text}"));
        assert_eq!(
            a2_row,
            a1_row + 2,
            "session rows stack on the 2-row pill stride:\n{text}"
        );
    }

    /// A project with the leading divider (divider above the whole list).
    fn leading_project(
        id: &str,
        name: &str,
        sort_order: i64,
        label: Option<&str>,
    ) -> nebula_core::Entity {
        use nebula_core::{Entity, Project, ProjectId};
        Entity::Project(Project {
            workspace_id: Default::default(),
            id: ProjectId(id.into()),
            name: name.into(),
            repo_path: format!("/tmp/{name}").into(),
            sort_order,
            divider_after: false,
            divider_label: None,
            divider_before: true,
            divider_before_label: label.map(String::from),
        })
    }

    #[test]
    fn divider_under_the_top_project_hops_above_the_list() {
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

        // j onto the divider under p1; Shift+K asks for the hop above the
        // whole list.
        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE),
            &mut out,
        );
        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('K'), KeyModifiers::SHIFT),
            &mut out,
        );
        assert!(
            matches!(
                out.last(),
                Some(ClientRequest::MoveDivider {
                    before: false,
                    delta: -1,
                    ..
                })
            ),
            "Shift+K under the first project requests the top hop: {out:?}"
        );

        // The daemon answers with the re-owned divider; the selection
        // chases it onto the leading row, drawn above its project.
        hse(
            &mut app,
            ServerEvent::EntityUpserted {
                entity: leading_project("p1", "demo", 0, Some("work")),
            },
        );
        assert_eq!(
            app.selected_project_row(),
            Some(ProjectRow::Divider {
                project: 0,
                before: true
            })
        );
        assert_eq!(app.sel_project, 0, "the leading divider is the first row");

        // Shift+K again clamps — it is already above everything.
        let sent = out.len();
        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('K'), KeyModifiers::SHIFT),
            &mut out,
        );
        assert_eq!(out.len(), sent, "top divider move up sends nothing");
    }

    #[test]
    fn selected_divider_blanks_the_panes_with_a_hint() {
        let mut app = App::new();
        seed_tree(&mut app); // p1/w1(main) + agent-1
        let mut out = Vec::new();
        app.focus = Focus::Projects;
        hse(
            &mut app,
            ServerEvent::EntityUpserted {
                entity: project("p1", "demo", 0, true, Some("work")),
            },
        );

        // On the project row the side panels show its content.
        let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
        terminal.draw(|f| ui::draw(f, &mut app)).unwrap();
        let text = buffer_text(&terminal);
        assert!(text.contains("⌂ root"), "project shows worktrees:\n{text}");
        assert!(text.contains("agent-1"), "project shows sessions:\n{text}");

        // On the divider row the panels stay but their content blanks, and
        // the terminal pane explains why.
        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE),
            &mut out,
        );
        terminal.draw(|f| ui::draw(f, &mut app)).unwrap();
        let text = buffer_text(&terminal);
        assert!(
            !text.contains("⌂ root"),
            "divider hides worktree rows:\n{text}"
        );
        assert!(
            !text.contains("agent-1"),
            "divider hides session rows:\n{text}"
        );
        assert!(
            text.contains("you're focused on a separator"),
            "the pane hints at what to do:\n{text}"
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
        assert_eq!(
            app.selected_project_row(),
            Some(ProjectRow::Divider {
                project: 0,
                before: false
            })
        );
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
                Some(ClientRequest::SetProjectDivider { present: true, label: Some(l), .. })
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
                    present: false,
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
                    workspace_id: Default::default(),
                    id: ProjectId("p1".into()),
                    name: "demo".into(),
                    repo_path: path.to_path_buf(),
                    sort_order: 0,
                    divider_after: false,
                    divider_label: None,
                    divider_before: false,
                    divider_before_label: None,
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
                    pinned: false,
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

    /// The badge cache follows the checkout: dirty counts (staged, unstaged
    /// and untracked alike), clean clears, an unreadable path shows nothing,
    /// and every value change marks the app dirty so a frame gets drawn.
    #[test]
    fn refresh_git_changes_tracks_the_checkout() {
        let dir = tempfile::tempdir().unwrap();
        let repo = test_repo(&dir);
        let mut app = App::new();
        seed_repo_tree(&mut app, &repo);

        app.dirty = false;
        refresh_git_changes(&mut app);
        assert_eq!(app.selected_worktree_changes(), Some(0), "clean tree");
        assert!(app.dirty, "first computation redraws");
        assert!(!app.git_changes_stale(), "cache matches the selection");

        std::fs::write(repo.join("a.txt"), "changed\n").unwrap();
        std::fs::write(repo.join("z.txt"), "fresh\n").unwrap();
        app.dirty = false;
        refresh_git_changes(&mut app);
        assert_eq!(app.selected_worktree_changes(), Some(2), "dirty tree");
        assert!(app.dirty, "count change redraws");

        app.dirty = false;
        refresh_git_changes(&mut app);
        assert!(!app.dirty, "an unchanged count skips the redraw");

        run_git(&repo, &["add", "."]);
        run_git(&repo, &["commit", "-m", "wip"]);
        refresh_git_changes(&mut app);
        assert_eq!(app.selected_worktree_changes(), Some(0), "commit clears");
    }

    #[test]
    fn refresh_git_changes_survives_a_missing_repo() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = App::new();
        seed_repo_tree(&mut app, &dir.path().join("nope"));
        refresh_git_changes(&mut app);
        assert_eq!(app.selected_worktree_changes(), None);
        assert!(!app.git_changes_stale(), "the failed read is still cached");
    }

    /// The worktree panel badge renders only for a dirty selected checkout.
    #[test]
    fn worktree_panel_badge_shows_change_count() {
        let mut app = App::new();
        seed_tree(&mut app);
        let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();

        terminal.draw(|f| ui::draw(f, &mut app)).unwrap();
        assert!(
            !buffer_text(&terminal).contains("+1 file"),
            "no badge before a count exists"
        );

        app.git_changes = Some((WorktreeId("w1".into()), Some(2)));
        terminal.draw(|f| ui::draw(f, &mut app)).unwrap();
        let text = buffer_text(&terminal);
        assert!(text.contains("+2 files"), "badge rendered:\n{text}");

        app.git_changes = Some((WorktreeId("w1".into()), Some(1)));
        terminal.draw(|f| ui::draw(f, &mut app)).unwrap();
        let text = buffer_text(&terminal);
        assert!(text.contains("+1 file "), "singular form:\n{text}");

        app.git_changes = Some((WorktreeId("w1".into()), Some(0)));
        terminal.draw(|f| ui::draw(f, &mut app)).unwrap();
        assert!(
            !buffer_text(&terminal).contains("+0 file"),
            "clean checkout stays quiet"
        );

        // A count cached for some other worktree must not leak onto the
        // selected row's footer crumb.
        app.git_changes = Some((WorktreeId("w2".into()), Some(5)));
        terminal.draw(|f| ui::draw(f, &mut app)).unwrap();
        assert!(
            !buffer_text(&terminal).contains("+5 file"),
            "stale cache renders nothing"
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

    /// The current diff view, or panic.
    fn diff_view(app: &App) -> &crate::app::DiffView {
        match &app.overlay {
            Some(Overlay::Diff(v)) => v,
            other => panic!("expected diff overlay, got {other:?}"),
        }
    }

    /// The visible file list in display order.
    fn diff_order(app: &App) -> Vec<String> {
        let v = diff_view(app);
        v.matches
            .iter()
            .map(|m| v.files[m.file].path.clone())
            .collect()
    }

    #[test]
    fn ctrl_r_toggles_reviewed_and_marks_survive_reopen() {
        let dir = tempfile::tempdir().unwrap();
        crate::review::with_store_path(dir.path().join("reviewed.json"), || {
            let repo = test_repo(&dir);
            std::fs::write(repo.join("a.txt"), "changed\n").unwrap();
            std::fs::write(repo.join("z.txt"), "fresh\n").unwrap();

            let mut app = App::new();
            seed_repo_tree(&mut app, &repo);
            let mut out = Vec::new();
            press(&mut app, KeyCode::Char('g'), KeyModifiers::NONE, &mut out);
            // Status is path-ordered, so a.txt is the selected file. Marking
            // sinks it below z.txt and advances to the next file.
            press(
                &mut app,
                KeyCode::Char('r'),
                KeyModifiers::CONTROL,
                &mut out,
            );
            let v = diff_view(&app);
            assert!(v.reviewed.contains_key("a.txt"), "{:?}", v.reviewed);
            assert!(!v.head_key.is_empty(), "head OID captured for scoping");
            assert_eq!(diff_order(&app), ["z.txt", "a.txt"], "reviewed sinks");
            let v = diff_view(&app);
            assert_eq!(v.selected_file().unwrap().path, "z.txt", "auto-advance");
            assert!(
                v.diff.contains("+fresh"),
                "next file's diff loaded: {}",
                v.diff
            );

            // Reopen: the mark comes back from the store, already sunk, and
            // the first unreviewed file starts selected.
            press(&mut app, KeyCode::Esc, KeyModifiers::NONE, &mut out);
            press(&mut app, KeyCode::Char('g'), KeyModifiers::NONE, &mut out);
            assert_eq!(diff_order(&app), ["z.txt", "a.txt"], "restored + sunk");
            assert_eq!(diff_view(&app).selected_file().unwrap().path, "z.txt");

            // Ctrl+r on the reviewed row unmarks it; the file pops back up
            // to git order, stays selected, and the store forgets the mark.
            press(&mut app, KeyCode::Down, KeyModifiers::NONE, &mut out);
            press(
                &mut app,
                KeyCode::Char('r'),
                KeyModifiers::CONTROL,
                &mut out,
            );
            let v = diff_view(&app);
            assert!(v.reviewed.is_empty());
            assert_eq!(diff_order(&app), ["a.txt", "z.txt"], "git order back");
            let v = diff_view(&app);
            assert_eq!(
                v.selected_file().unwrap().path,
                "a.txt",
                "selection follows the unmarked file"
            );
            press(&mut app, KeyCode::Esc, KeyModifiers::NONE, &mut out);
            press(&mut app, KeyCode::Char('g'), KeyModifiers::NONE, &mut out);
            assert!(diff_view(&app).reviewed.is_empty(), "unmark persisted");
            assert!(out.is_empty(), "reviewed marks never talk to the daemon");
        });
    }

    #[test]
    fn editing_a_reviewed_file_drops_its_mark_on_reopen() {
        let dir = tempfile::tempdir().unwrap();
        crate::review::with_store_path(dir.path().join("reviewed.json"), || {
            let repo = test_repo(&dir);
            std::fs::write(repo.join("a.txt"), "changed\n").unwrap();

            let mut app = App::new();
            seed_repo_tree(&mut app, &repo);
            let mut out = Vec::new();
            press(&mut app, KeyCode::Char('g'), KeyModifiers::NONE, &mut out);
            press(
                &mut app,
                KeyCode::Char('r'),
                KeyModifiers::CONTROL,
                &mut out,
            );
            press(&mut app, KeyCode::Esc, KeyModifiers::NONE, &mut out);

            // The approved diff no longer matches what's on disk.
            std::fs::write(repo.join("a.txt"), "changed again\n").unwrap();
            press(&mut app, KeyCode::Char('g'), KeyModifiers::NONE, &mut out);
            assert!(
                diff_view(&app).reviewed.is_empty(),
                "an edited file comes back unreviewed"
            );
        });
    }

    #[test]
    fn a_commit_resets_reviewed_marks() {
        let dir = tempfile::tempdir().unwrap();
        crate::review::with_store_path(dir.path().join("reviewed.json"), || {
            let repo = test_repo(&dir);
            std::fs::write(repo.join("a.txt"), "changed\n").unwrap();

            let mut app = App::new();
            seed_repo_tree(&mut app, &repo);
            let mut out = Vec::new();
            press(&mut app, KeyCode::Char('g'), KeyModifiers::NONE, &mut out);
            press(
                &mut app,
                KeyCode::Char('r'),
                KeyModifiers::CONTROL,
                &mut out,
            );
            press(&mut app, KeyCode::Esc, KeyModifiers::NONE, &mut out);

            // Commit moves HEAD; the next round of changes starts unreviewed.
            run_git(&repo, &["add", "."]);
            run_git(&repo, &["commit", "-m", "wip"]);
            std::fs::write(repo.join("a.txt"), "post-commit\n").unwrap();
            press(&mut app, KeyCode::Char('g'), KeyModifiers::NONE, &mut out);
            assert!(
                diff_view(&app).reviewed.is_empty(),
                "a commit resets the worktree's marks"
            );
        });
    }

    #[test]
    fn diff_modal_ctrl_u_clears_filter_before_scrolling() {
        let mut app = App::new();
        seed_tree(&mut app);
        app.overlay = Some(Overlay::Diff(fake_diff_view(100)));
        let mut out = Vec::new();
        let view = |app: &App| match &app.overlay {
            Some(Overlay::Diff(v)) => v.clone(),
            _ => panic!("diff overlay gone"),
        };

        // With nothing typed, Ctrl+u keeps its half-page-up scroll role.
        press(
            &mut app,
            KeyCode::Char('d'),
            KeyModifiers::CONTROL,
            &mut out,
        );
        assert_eq!(view(&app).scroll, 10, "Ctrl+d scrolls half a page down");
        press(
            &mut app,
            KeyCode::Char('u'),
            KeyModifiers::CONTROL,
            &mut out,
        );
        assert_eq!(view(&app).scroll, 0, "empty filter: Ctrl+u scrolls up");

        // With a filter typed, Ctrl+u clears it instead of scrolling.
        press(&mut app, KeyCode::Char('b'), KeyModifiers::NONE, &mut out);
        assert_eq!(view(&app).matches.len(), 1, "filter narrows to beta.rs");
        press(
            &mut app,
            KeyCode::Char('u'),
            KeyModifiers::CONTROL,
            &mut out,
        );
        let v = view(&app);
        assert_eq!(v.filter, "", "Ctrl+u clears the filter");
        assert_eq!(v.matches.len(), 2, "full list restored");
        assert!(
            matches!(app.overlay, Some(Overlay::Diff(_))),
            "the modal stays open"
        );
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
                    workspace_id: Default::default(),
                    id: ProjectId("p2".into()),
                    name: "nebula".into(),
                    repo_path: "/tmp/nebula".into(),
                    sort_order: 1,
                    divider_after: false,
                    divider_label: None,
                    divider_before: false,
                    divider_before_label: None,
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
                    pinned: false,
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
                    archived_at: 0,
                    pinned: false,
                    kind: nebula_core::AgentKind::Codex,
                    model: None,
                    effort: None,
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
                    archived_at: 0,
                    pinned: false,
                    kind: nebula_core::AgentKind::Claude,
                    model: None,
                    effort: None,
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
        assert_eq!(
            app.focus,
            Focus::Sessions,
            "a worktree pick lands in its Sessions panel, not the Worktrees column"
        );
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
                    workspace_id: Default::default(),
                    id: ProjectId("p9".into()),
                    name: "fresh".into(),
                    repo_path: "/tmp/fresh".into(),
                    sort_order: 9,
                    divider_after: false,
                    divider_label: None,
                    divider_before: false,
                    divider_before_label: None,
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
    fn palette_renders_with_kind_glyphs_and_column_headers() {
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
        // Sidebar headers are plain uppercase text (no emoji).
        assert!(text.contains("PROJECTS"), "{text}");
        assert!(text.contains("WORKTREES"), "{text}");
        assert!(text.contains("SESSIONS"), "{text}");
        // Palette rows carry per-kind glyphs: ▪ project, ▸ worktree,
        // ● session.
        assert!(text.contains("▪ demo"), "project glyph row:\n{text}");
        assert!(text.contains("▸ demo/main"), "worktree glyph row:\n{text}");
        assert!(
            text.contains("● demo/main/agent-1"),
            "session row rendered in the palette:\n{text}"
        );
        // Rects for mouse hit-testing were written back during the draw.
        assert!(palette(&app).list_area.width > 0);
    }

    /// A palette row wears the status its panel row wears: the session's
    /// own, rolled up for its worktree and project. The status arrives
    /// while the palette is open, so the rebuild must carry it through.
    #[test]
    fn palette_rows_take_their_status_color_and_sweep() {
        use nebula_core::{Agent, AgentStatus, Entity};
        let th = crate::theme::Theme::default();
        let mut app = App::new();
        seed_tree(&mut app);
        let mut out = Vec::new();
        press(&mut app, KeyCode::Char('/'), KeyModifiers::NONE, &mut out);
        hse(
            &mut app,
            ServerEvent::EntityUpserted {
                entity: Entity::Agent(Agent {
                    id: AgentId("a1".into()),
                    worktree_id: nebula_core::WorktreeId("w1".into()),
                    name: "agent-1".into(),
                    status: AgentStatus::Running,
                    archived: false,
                    archived_at: 0,
                    pinned: false,
                    kind: nebula_core::AgentKind::Claude,
                    model: None,
                    effort: None,
                    session_id: None,
                    sort_order: 0,
                    status_changed_at: 0,
                    alive: true,
                }),
            },
        );
        let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
        terminal.draw(|f| ui::draw(f, &mut app)).unwrap();

        // The one running agent lights its own row and both rollups.
        for row in ["▪ demo", "▸ demo/main", "● demo/main/agent-1"] {
            let (x, y) = find_cell(&terminal, row);
            assert_eq!(
                terminal.backend().buffer()[(x, y)].fg,
                th.warn,
                "{row:?} glyph reads running"
            );
        }
        // ...and the leaf segment rides the running sweep, not plain text.
        let (x, y) = find_cell(&terminal, "● demo/main/agent-1");
        let buffer = terminal.backend().buffer();
        let leaf_x = x + "● demo/main/".chars().count() as u16;
        for i in 0.."agent-1".chars().count() as u16 {
            let fg = buffer[(leaf_x + i, y)].fg;
            assert!(
                th.warn_sweep.contains(&fg),
                "leaf cell {i} is on the sweep ramp, got {fg:?}"
            );
        }
        // The dim parent path stays out of the sweep.
        assert_eq!(buffer[(x + 2, y)].fg, th.dim, "parent path stays quiet");
    }

    /// Nothing live under a row: the glyph goes hollow and dim, mirroring
    /// the panels' `○`.
    #[test]
    fn palette_rows_with_no_live_status_render_hollow() {
        let th = crate::theme::Theme::default();
        let mut app = App::new();
        seed_tree(&mut app);
        app.tree.agents.clear();
        let mut out = Vec::new();
        press(&mut app, KeyCode::Char('/'), KeyModifiers::NONE, &mut out);
        let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
        terminal.draw(|f| ui::draw(f, &mut app)).unwrap();
        let text = buffer_text(&terminal);
        assert!(text.contains("▫ demo"), "hollow project glyph:\n{text}");
        assert!(
            text.contains("▹ demo/main"),
            "hollow worktree glyph:\n{text}"
        );
        // Row 0 sits under the selection fill, which lifts dim to muted;
        // read the unselected worktree row for the resting shade.
        let (x, y) = find_cell(&terminal, "▹ demo/main");
        assert_eq!(terminal.backend().buffer()[(x, y)].fg, th.dim);
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
    fn settings_reopens_on_last_focused_row() {
        let mut app = App::new();
        let mut out = Vec::new();
        press(&mut app, KeyCode::Char('s'), KeyModifiers::NONE, &mut out);
        press(&mut app, KeyCode::Char('j'), KeyModifiers::NONE, &mut out);
        press(&mut app, KeyCode::Char('j'), KeyModifiers::NONE, &mut out);
        press(&mut app, KeyCode::Esc, KeyModifiers::NONE, &mut out);
        press(&mut app, KeyCode::Char('s'), KeyModifiers::NONE, &mut out);
        let Some(Overlay::Settings(view)) = &app.overlay else {
            panic!("settings closed");
        };
        assert_eq!(view.selected, 2, "reopen lands on the last focused row");
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
            let row = crate::config::settings()
                .position(|s| s.kind == crate::config::SettingKind::RecentWindow)
                .unwrap();
            for _ in 0..row {
                press(&mut app, KeyCode::Char('j'), KeyModifiers::NONE, &mut out);
            }
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
        for group in crate::config::SETTING_GROUPS {
            assert!(text.contains(group.title), "group header rendered:\n{text}");
        }
        assert!(
            text.contains("Enter in / search opens the session"),
            "selected setting's hint shown in the footer:\n{text}"
        );
        let Some(Overlay::Settings(view)) = &app.overlay else {
            panic!("settings closed");
        };
        assert!(view.area.width > 0, "draw writes hit-test area");
    }

    // ---- `M` metrics modal ----

    #[test]
    fn metrics_modal_opens_requests_and_renders() {
        let mut app = App::new();
        seed_tree(&mut app);
        let mut out = Vec::new();
        press(&mut app, KeyCode::Char('M'), KeyModifiers::SHIFT, &mut out);
        assert!(matches!(app.overlay, Some(Overlay::Metrics(_))));

        // The keypress itself fires the initial reading's request.
        let req_id = match out.last() {
            Some(ClientRequest::GetMetrics { req_id }) => *req_id,
            other => panic!("expected GetMetrics, got {other:?}"),
        };
        hse(
            &mut app,
            ServerEvent::Metrics {
                req_id,
                snapshot: nebula_core::MetricsSnapshot {
                    daemon_pid: 42,
                    daemon_rss_bytes: 40 * 1024 * 1024,
                    system_total_bytes: 32 * 1024 * 1024 * 1024,
                    sessions: vec![nebula_core::SessionMetrics {
                        session: SessionRef::Agent(AgentId("a1".into())),
                        pid: 4321,
                        rss_bytes: 1_610_612_736, // 1.5 GB
                        procs: 3,
                    }],
                },
            },
        );
        assert!(
            app.pending.is_empty(),
            "the Metrics reply must clear its pending slot"
        );
        let Some(Overlay::Metrics(view)) = &app.overlay else {
            panic!("metrics closed");
        };
        assert!(view.snapshot.is_some());

        let mut terminal = Terminal::new(TestBackend::new(100, 40)).unwrap();
        terminal.draw(|f| ui::draw(f, &mut app)).unwrap();
        let text = buffer_text(&terminal);
        assert!(text.contains("Memory"), "title rendered:\n{text}");
        assert!(
            text.contains("1 session · 3 procs"),
            "claude rollup rendered:\n{text}"
        );
        assert!(
            text.contains("agent-1 (claude)") && text.contains("demo/main"),
            "session row joined with the tree:\n{text}"
        );
        assert!(text.contains("1.5 GB"), "subtree memory rendered:\n{text}");
        assert!(
            text.contains("nebula daemon") && text.contains("40 MB"),
            "daemon row rendered:\n{text}"
        );
        assert!(
            text.contains("% of 32 GB installed"),
            "system share rendered:\n{text}"
        );
        let Some(Overlay::Metrics(view)) = &app.overlay else {
            panic!("metrics closed");
        };
        assert!(view.area.width > 0, "draw writes the hit-test area back");

        press(&mut app, KeyCode::Esc, KeyModifiers::NONE, &mut out);
        assert!(app.overlay.is_none());
    }

    #[test]
    fn metrics_enter_opens_selected_session() {
        let mut app = App::new();
        seed_tree(&mut app);
        let mut out = Vec::new();
        press(&mut app, KeyCode::Char('M'), KeyModifiers::SHIFT, &mut out);
        request_metrics(&mut app, &mut out);
        let req_id = match out.last() {
            Some(ClientRequest::GetMetrics { req_id }) => *req_id,
            other => panic!("expected GetMetrics, got {other:?}"),
        };
        let snapshot = nebula_core::MetricsSnapshot {
            daemon_pid: 42,
            daemon_rss_bytes: 1024,
            system_total_bytes: 0,
            sessions: vec![nebula_core::SessionMetrics {
                session: SessionRef::Agent(AgentId("a1".into())),
                pid: 4321,
                rss_bytes: 2048,
                procs: 1,
            }],
        };
        hse(
            &mut app,
            ServerEvent::Metrics {
                req_id,
                snapshot: snapshot.clone(),
            },
        );

        // A draw writes the row order back into the view; Enter reads it.
        let mut terminal = Terminal::new(TestBackend::new(100, 40)).unwrap();
        terminal.draw(|f| ui::draw(f, &mut app)).unwrap();
        let Some(Overlay::Metrics(view)) = &app.overlay else {
            panic!("metrics closed");
        };
        assert_eq!(view.rows.len(), 3, "session + daemon + ui rows");
        assert_eq!(view.selected, 0, "cursor starts on the biggest session");

        out.clear();
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE, &mut out);
        assert!(app.overlay.is_none(), "Enter closes the modal");
        assert_eq!(app.focus, Focus::Terminal);
        assert!(app.term_locked, "opened session locks input like an attach");
        let sref = SessionRef::Agent(AgentId("a1".into()));
        assert!(
            out.iter()
                .any(|r| matches!(r, ClientRequest::Attach { session, .. } if *session == sref)),
            "Enter attaches the selected session: {out:?}"
        );
        assert_eq!(
            app.visible_session_rows()
                .get(app.sel_session)
                .map(|r| r.sref()),
            Some(sref),
            "the panel selection landed on the opened session"
        );

        // Reopen (Ctrl+q first — the attach locked input to the terminal);
        // Enter on one of nebula's own rows (no session) is inert.
        press(
            &mut app,
            KeyCode::Char('q'),
            KeyModifiers::CONTROL,
            &mut out,
        );
        press(&mut app, KeyCode::Char('M'), KeyModifiers::SHIFT, &mut out);
        request_metrics(&mut app, &mut out);
        let req_id = match out.last() {
            Some(ClientRequest::GetMetrics { req_id }) => *req_id,
            other => panic!("expected GetMetrics, got {other:?}"),
        };
        hse(&mut app, ServerEvent::Metrics { req_id, snapshot });
        terminal.draw(|f| ui::draw(f, &mut app)).unwrap();
        press(&mut app, KeyCode::Char('j'), KeyModifiers::NONE, &mut out);
        press(&mut app, KeyCode::Char('j'), KeyModifiers::NONE, &mut out);
        let Some(Overlay::Metrics(view)) = &app.overlay else {
            panic!("metrics closed");
        };
        assert_eq!(view.selected, 2, "j walks down to the ui row");
        press(&mut app, KeyCode::Char('j'), KeyModifiers::NONE, &mut out);
        let Some(Overlay::Metrics(view)) = &app.overlay else {
            panic!("metrics closed");
        };
        assert_eq!(view.selected, 2, "selection does not run past the last row");
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE, &mut out);
        assert!(
            matches!(app.overlay, Some(Overlay::Metrics(_))),
            "Enter on a nebula row keeps the modal open"
        );
    }

    #[test]
    fn metrics_reply_after_close_is_dropped() {
        let mut app = App::new();
        let mut out = Vec::new();
        press(&mut app, KeyCode::Char('M'), KeyModifiers::SHIFT, &mut out);
        let req_id = match out.last() {
            Some(ClientRequest::GetMetrics { req_id }) => *req_id,
            other => panic!("expected GetMetrics, got {other:?}"),
        };
        press(&mut app, KeyCode::Char('q'), KeyModifiers::NONE, &mut out);
        assert!(app.overlay.is_none(), "q closes the modal");
        hse(
            &mut app,
            ServerEvent::Metrics {
                req_id,
                snapshot: nebula_core::MetricsSnapshot {
                    daemon_pid: 42,
                    daemon_rss_bytes: 0,
                    system_total_bytes: 0,
                    sessions: vec![],
                },
            },
        );
        assert!(
            app.overlay.is_none(),
            "late reply must not reopen the modal"
        );
        assert!(app.pending.is_empty(), "late reply still clears its slot");
    }

    // ---- `f` fuzzy file finder ----

    fn finder(app: &App) -> &crate::app::FileFinder {
        match &app.overlay {
            Some(Overlay::Files(f)) => f,
            other => panic!("expected file finder overlay, got {other:?}"),
        }
    }

    #[test]
    fn f_opens_file_finder_listing_tracked_and_untracked() {
        let dir = tempfile::tempdir().unwrap();
        let repo = test_repo(&dir);
        std::fs::write(repo.join("fresh.txt"), "hello\n").unwrap();
        let mut app = App::new();
        seed_repo_tree(&mut app, &repo);
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        app.vim_tx = Some(tx);
        let mut out = Vec::new();

        press(&mut app, KeyCode::Char('f'), KeyModifiers::NONE, &mut out);
        let files = &finder(&app).files;
        assert!(files.contains(&"a.txt".to_string()), "{files:?}");
        assert!(files.contains(&"fresh.txt".to_string()), "{files:?}");
        // The empty query shows everything.
        assert_eq!(finder(&app).matches.len(), files.len());
        assert!(out.is_empty(), "opening the finder sends nothing");

        // Typing narrows to the fuzzy matches.
        for c in ['f', 'r'] {
            press(&mut app, KeyCode::Char(c), KeyModifiers::NONE, &mut out);
        }
        assert_eq!(finder(&app).matches.len(), 1, "fr matches only fresh.txt");
        assert_eq!(finder(&app).selected_path(), Some("fresh.txt"));

        // Enter opens the selection in the editor modal; the finder stays
        // open underneath. A shell stands in for vim (`sh +1 fresh.txt`
        // still spawns fine).
        if let Some(Overlay::Files(f)) = &mut app.overlay {
            f.editor = "/bin/sh".into();
        }
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE, &mut out);
        let vim = app.vim.as_ref().expect("enter spawns the editor modal");
        assert_eq!(vim.title, "fresh.txt:1");
        assert!(
            matches!(&app.overlay, Some(Overlay::Files(_))),
            "the finder stays open under the editor"
        );

        // Ctrl+Q closes the editor, landing back on the finder; Ctrl+y
        // copies the selected path and closes.
        press(
            &mut app,
            KeyCode::Char('q'),
            KeyModifiers::CONTROL,
            &mut out,
        );
        assert!(app.vim.is_none(), "Ctrl+Q force-closes the editor");
        press(
            &mut app,
            KeyCode::Char('y'),
            KeyModifiers::CONTROL,
            &mut out,
        );
        assert!(app.overlay.is_none(), "ctrl+y closes the finder");
        assert_eq!(app.flash.as_deref(), Some("copied fresh.txt"));
    }

    #[test]
    fn file_finder_escape_clears_query_then_closes() {
        let mut app = App::new();
        app.overlay = Some(Overlay::Files(FileFinder::new(
            "/nonexistent-nebula-finder-test".into(),
            "main".into(),
            "vim".into(),
            vec!["src/alpha.rs".into(), "src/beta.rs".into()],
        )));
        let mut out = Vec::new();
        press(&mut app, KeyCode::Char('b'), KeyModifiers::NONE, &mut out);
        assert_eq!(finder(&app).matches.len(), 1, "b matches only beta.rs");
        press(&mut app, KeyCode::Esc, KeyModifiers::NONE, &mut out);
        assert_eq!(finder(&app).query, "", "first Esc clears the query");
        assert_eq!(finder(&app).matches.len(), 2, "cleared query shows all");
        press(&mut app, KeyCode::Esc, KeyModifiers::NONE, &mut out);
        assert!(app.overlay.is_none(), "second Esc closes the finder");
    }

    #[test]
    fn file_overlays_launch_the_configured_editor() {
        let dir = tempfile::tempdir().unwrap();
        let repo = test_repo(&dir);
        let cfg_dir = tempfile::tempdir().unwrap();
        crate::config::with_config_path(cfg_dir.path().join("config.json"), || {
            let mut cfg = crate::config::Config::load();
            cfg.editor = "nvim".into();
            cfg.save().unwrap();
            // What the overlays should capture: the setting, unless the
            // test environment carries a NEBULA_EDITOR override.
            let expect = crate::config::Config::load().editor_command();

            let mut app = App::new();
            seed_repo_tree(&mut app, &repo);
            let mut out = Vec::new();
            press(&mut app, KeyCode::Char('f'), KeyModifiers::NONE, &mut out);
            assert_eq!(finder(&app).editor, expect);
            app.overlay = None;
            press(&mut app, KeyCode::Char('b'), KeyModifiers::NONE, &mut out);
            assert_eq!(tree_view(&app).editor, expect);
            app.overlay = None;
            press(&mut app, KeyCode::Char('F'), KeyModifiers::SHIFT, &mut out);
            let Some(Overlay::Grep(view)) = &app.overlay else {
                panic!("F opens the grep overlay");
            };
            assert_eq!(view.editor, expect);
        });
    }

    #[test]
    fn f_without_worktree_flashes() {
        let mut app = App::new();
        let mut out = Vec::new();
        press(&mut app, KeyCode::Char('f'), KeyModifiers::NONE, &mut out);
        assert!(app.overlay.is_none());
        assert_eq!(app.flash.as_deref(), Some("no worktree selected"));
    }

    #[test]
    fn file_finder_renders_query_row_and_matches() {
        let mut app = App::new();
        app.overlay = Some(Overlay::Files(FileFinder::new(
            "/nonexistent-nebula-finder-test".into(),
            "main".into(),
            "vim".into(),
            vec!["src/alpha.rs".into(), "src/beta.rs".into()],
        )));
        let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
        terminal.draw(|f| ui::draw(f, &mut app)).unwrap();
        let text = buffer_text(&terminal);
        assert!(text.contains("Find file — main (2)"), "title:\n{text}");
        assert!(text.contains("type to filter…"), "query hint:\n{text}");
        assert!(text.contains("src/alpha.rs"), "rows rendered:\n{text}");
        let fin = finder(&app);
        assert!(fin.area.width > 0, "draw writes hit-test area");
        assert!(fin.list_area.height > 0, "draw writes list area");
    }

    // ---- `b` tree browser ----

    fn tree_view(app: &App) -> &crate::tree_browser::TreeBrowser {
        match &app.overlay {
            Some(Overlay::Tree(v)) => v,
            other => panic!("expected tree overlay, got {other:?}"),
        }
    }

    fn tree_rows(app: &App) -> Vec<String> {
        let v = tree_view(app);
        v.rows
            .iter()
            .map(|r| v.nodes[r.node].path.clone())
            .collect()
    }

    #[test]
    fn t_opens_tree_browser_folds_dirs_and_filters_hierarchies() {
        let dir = tempfile::tempdir().unwrap();
        let repo = test_repo(&dir);
        std::fs::create_dir_all(repo.join("src/sub")).unwrap();
        std::fs::write(repo.join("src/lib.rs"), "hello tree\n").unwrap();
        std::fs::write(repo.join("src/sub/deep.rs"), "deep\n").unwrap();
        let mut app = App::new();
        seed_repo_tree(&mut app, &repo);
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        app.vim_tx = Some(tx);
        let mut out = Vec::new();

        press(&mut app, KeyCode::Char('b'), KeyModifiers::NONE, &mut out);
        assert_eq!(tree_view(&app).file_count, 3);
        // Collapsed by default: dirs first, then top-level files; the
        // selected dir previews its children.
        assert_eq!(tree_rows(&app), vec!["src", "a.txt"]);
        assert_eq!(tree_view(&app).preview, "sub/\nlib.rs");
        assert!(out.is_empty(), "opening the browser sends nothing");

        // Enter on a directory unfolds it, and folds it again.
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE, &mut out);
        assert_eq!(
            tree_rows(&app),
            vec!["src", "src/sub", "src/lib.rs", "a.txt"]
        );
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE, &mut out);
        assert_eq!(tree_rows(&app), vec!["src", "a.txt"]);

        // Typing narrows the tree to matching files plus the hierarchies
        // containing them, forced open, with the selection on the match.
        for c in ['d', 'e', 'e', 'p'] {
            press(&mut app, KeyCode::Char(c), KeyModifiers::NONE, &mut out);
        }
        assert_eq!(tree_rows(&app), vec!["src", "src/sub", "src/sub/deep.rs"]);
        assert_eq!(tree_view(&app).match_count, 1);
        assert_eq!(
            tree_view(&app).selected_node().unwrap().path,
            "src/sub/deep.rs"
        );
        assert_eq!(tree_view(&app).preview, "deep");

        // Enter opens the selected file in an editor embedded in the
        // preview pane; the browser stays open. A shell stands in for vim.
        if let Some(Overlay::Tree(v)) = &mut app.overlay {
            v.editor = "/bin/sh".into();
        }
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE, &mut out);
        let vim = app.vim.as_ref().expect("enter spawns the editor");
        assert_eq!(vim.title, "src/sub/deep.rs:1");
        assert!(vim.embedded, "tree editor renders in the preview pane");
        assert!(
            matches!(&app.overlay, Some(Overlay::Tree(_))),
            "the browser stays open around the editor"
        );

        // Closing the editor reloads the preview — the file may have been
        // edited under it.
        std::fs::write(repo.join("src/sub/deep.rs"), "deeper\n").unwrap();
        press(
            &mut app,
            KeyCode::Char('q'),
            KeyModifiers::CONTROL,
            &mut out,
        );
        assert!(app.vim.is_none(), "Ctrl+Q force-closes the editor");
        assert_eq!(tree_view(&app).preview, "deeper");

        // Two-stage escape: clear the filter (restoring the folded tree),
        // then close.
        press(&mut app, KeyCode::Esc, KeyModifiers::NONE, &mut out);
        assert_eq!(tree_view(&app).filter, "", "first Esc clears the filter");
        assert_eq!(tree_rows(&app), vec!["src", "a.txt"]);
        press(&mut app, KeyCode::Esc, KeyModifiers::NONE, &mut out);
        assert!(app.overlay.is_none(), "second Esc closes the browser");
    }

    #[test]
    fn tree_browser_ctrl_u_clears_filter() {
        let dir = tempfile::tempdir().unwrap();
        let repo = test_repo(&dir);
        std::fs::create_dir_all(repo.join("src")).unwrap();
        std::fs::write(repo.join("src/lib.rs"), "hello tree\n").unwrap();
        let mut app = App::new();
        seed_repo_tree(&mut app, &repo);
        let mut out = Vec::new();

        press(&mut app, KeyCode::Char('b'), KeyModifiers::NONE, &mut out);
        for c in ['l', 'i', 'b'] {
            press(&mut app, KeyCode::Char(c), KeyModifiers::NONE, &mut out);
        }
        assert_eq!(tree_rows(&app), vec!["src", "src/lib.rs"]);

        press(
            &mut app,
            KeyCode::Char('u'),
            KeyModifiers::CONTROL,
            &mut out,
        );
        assert_eq!(tree_view(&app).filter, "", "Ctrl+u clears the filter");
        assert_eq!(
            tree_rows(&app),
            vec!["src", "a.txt"],
            "folded tree restored"
        );

        // With nothing typed, Ctrl+u falls back to scrolling: the browser
        // stays open and the filter stays empty.
        press(
            &mut app,
            KeyCode::Char('u'),
            KeyModifiers::CONTROL,
            &mut out,
        );
        assert_eq!(tree_view(&app).filter, "");
        assert!(
            matches!(app.overlay, Some(Overlay::Tree(_))),
            "the browser stays open"
        );
    }

    #[test]
    fn b_without_worktree_flashes() {
        let mut app = App::new();
        let mut out = Vec::new();
        press(&mut app, KeyCode::Char('b'), KeyModifiers::NONE, &mut out);
        assert!(app.overlay.is_none());
        assert_eq!(app.flash.as_deref(), Some("no worktree selected"));
    }

    #[test]
    fn tree_browser_renders_tree_and_preview_panes() {
        let dir = tempfile::tempdir().unwrap();
        let repo = test_repo(&dir); // commits a.txt containing "orig"
        let mut app = App::new();
        seed_repo_tree(&mut app, &repo);
        let mut out = Vec::new();
        press(&mut app, KeyCode::Char('b'), KeyModifiers::NONE, &mut out);

        let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
        terminal.draw(|f| ui::draw(f, &mut app)).unwrap();
        let text = buffer_text(&terminal);
        assert!(text.contains("Tree — main (1)"), "tree title:\n{text}");
        assert!(text.contains("type to filter…"), "filter hint:\n{text}");
        assert!(text.contains("a.txt"), "tree rows rendered:\n{text}");
        assert!(text.contains("orig"), "preview rendered:\n{text}");
        let v = tree_view(&app);
        assert!(v.area.width > 0, "draw writes hit-test area");
        assert!(v.list_area.height > 0, "draw writes list area");
        assert!(v.view_height > 0, "draw writes preview page size");
    }

    #[test]
    fn embedded_editor_takes_over_the_preview_pane() {
        let dir = tempfile::tempdir().unwrap();
        let repo = test_repo(&dir);
        let mut app = App::new();
        seed_repo_tree(&mut app, &repo);
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        app.vim_tx = Some(tx);
        let mut out = Vec::new();
        press(&mut app, KeyCode::Char('b'), KeyModifiers::NONE, &mut out);

        // A draw teaches the browser its preview rect, so the editor can
        // spawn at the pane's size. Row 0 is a.txt (the only file).
        let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
        terminal.draw(|f| ui::draw(f, &mut app)).unwrap();
        if let Some(Overlay::Tree(v)) = &mut app.overlay {
            v.editor = "/bin/sh".into();
        }
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE, &mut out);
        let pane = tree_view(&app).preview_area;
        let vim = app.vim.as_ref().expect("enter spawns the editor");
        assert!(vim.embedded);
        assert_eq!(
            (vim.cols, vim.rows),
            (pane.width, pane.height),
            "editor spawns at the pane size"
        );

        terminal.draw(|f| ui::draw(f, &mut app)).unwrap();
        let text = buffer_text(&terminal);
        assert!(
            text.contains("— editing"),
            "title shows edit state:\n{text}"
        );
        assert_eq!(
            app.vim.as_ref().unwrap().area,
            tree_view(&app).preview_area,
            "editor renders into the preview pane, not the modal"
        );
    }

    // ---- `F` find in files + editor modal ----

    fn grep_view(app: &App) -> &crate::app::GrepView {
        match &app.overlay {
            Some(Overlay::Grep(v)) => v,
            other => panic!("expected grep overlay, got {other:?}"),
        }
    }

    fn fake_grep_view(hits: Vec<crate::grep_search::GrepHit>) -> GrepView {
        let mut view = GrepView::new(
            "/nonexistent-nebula-grep-test".into(),
            "main".into(),
            "vim".into(),
        );
        view.query = "zz".into();
        view.hits = hits;
        view
    }

    #[test]
    fn shift_f_opens_grep_and_typing_searches() {
        let dir = tempfile::tempdir().unwrap();
        let repo = test_repo(&dir);
        std::fs::write(repo.join("hay.txt"), "one\nneedle here\n").unwrap();
        let mut app = App::new();
        seed_repo_tree(&mut app, &repo);
        let mut out = Vec::new();

        press(&mut app, KeyCode::Char('F'), KeyModifiers::SHIFT, &mut out);
        assert!(grep_view(&app).hits.is_empty(), "opens with no results");
        assert!(out.is_empty(), "opening the overlay sends nothing");

        for c in "needle".chars() {
            press(&mut app, KeyCode::Char(c), KeyModifiers::NONE, &mut out);
        }
        let view = grep_view(&app);
        assert_eq!(view.hits.len(), 1, "{:?}", view.hits);
        assert_eq!(view.hits[0].path, "hay.txt");
        assert_eq!(view.hits[0].line, 2);
        assert_eq!(view.hits[0].text, "needle here");

        // Two-stage escape: clear the query, then close.
        press(&mut app, KeyCode::Esc, KeyModifiers::NONE, &mut out);
        assert_eq!(grep_view(&app).query, "", "first Esc clears the query");
        assert!(
            grep_view(&app).hits.is_empty(),
            "cleared query shows no hits"
        );
        press(&mut app, KeyCode::Esc, KeyModifiers::NONE, &mut out);
        assert!(app.overlay.is_none(), "second Esc closes the overlay");
    }

    #[test]
    fn shift_f_without_worktree_flashes() {
        let mut app = App::new();
        let mut out = Vec::new();
        press(&mut app, KeyCode::Char('F'), KeyModifiers::SHIFT, &mut out);
        assert!(app.overlay.is_none());
        assert_eq!(app.flash.as_deref(), Some("no worktree selected"));
    }

    #[test]
    fn grep_enter_spawns_editor_and_ctrl_q_closes_it() {
        let dir = tempfile::tempdir().unwrap();
        let repo = test_repo(&dir);
        let mut app = App::new();
        seed_repo_tree(&mut app, &repo);
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        app.vim_tx = Some(tx);
        let mut out = Vec::new();

        press(&mut app, KeyCode::Char('F'), KeyModifiers::SHIFT, &mut out);
        for c in "orig".chars() {
            press(&mut app, KeyCode::Char(c), KeyModifiers::NONE, &mut out);
        }
        assert_eq!(grep_view(&app).selected_hit().unwrap().path, "a.txt");
        // A shell stands in for vim (`sh +1 a.txt` still spawns fine).
        if let Some(Overlay::Grep(v)) = &mut app.overlay {
            v.editor = "/bin/sh".into();
        }

        press(&mut app, KeyCode::Enter, KeyModifiers::NONE, &mut out);
        let vim = app.vim.as_ref().expect("enter spawns the editor modal");
        assert_eq!(vim.title, "a.txt:1");
        assert_eq!(vim.generation, 1);
        assert!(
            matches!(&app.overlay, Some(Overlay::Grep(_))),
            "the grep overlay stays open under the editor"
        );

        // With the modal open, keys forward to the editor — q must not quit.
        press(&mut app, KeyCode::Char('q'), KeyModifiers::NONE, &mut out);
        assert!(!app.should_quit, "q goes to the editor, not the app");
        assert!(app.vim.is_some());

        // Ctrl+Q is the hatch.
        press(
            &mut app,
            KeyCode::Char('q'),
            KeyModifiers::CONTROL,
            &mut out,
        );
        assert!(app.vim.is_none(), "Ctrl+Q force-closes the editor");
        assert!(
            matches!(&app.overlay, Some(Overlay::Grep(_))),
            "closing the editor lands back on the results"
        );
    }

    #[test]
    fn stale_generation_editor_events_are_dropped() {
        let mut app = App::new();
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let dir = tempfile::tempdir().unwrap();
        let mut vim = crate::vim_term::VimTerm::spawn_cmd(
            "/bin/sh",
            &["-c".into(), "sleep 30".into()],
            dir.path(),
            "a.txt:1".into(),
            80,
            24,
            2,
            tx,
        )
        .unwrap();
        vim.kill();
        app.vim = Some(vim);

        // Output and exit stamped with a previous spawn's generation: ignored.
        handle_vim_event(
            &mut app,
            VimEvent::Output {
                generation: 1,
                data: b"stale".to_vec(),
            },
        );
        handle_vim_event(&mut app, VimEvent::Exited { generation: 1 });
        assert!(app.vim.is_some(), "stale exit must not close a new editor");
        assert!(
            !app.vim
                .as_ref()
                .unwrap()
                .parser
                .screen()
                .contents()
                .contains("stale"),
            "stale output must not reach the new editor's screen"
        );

        // The current generation's exit closes the modal.
        handle_vim_event(&mut app, VimEvent::Exited { generation: 2 });
        assert!(app.vim.is_none());
    }

    #[test]
    fn grep_overlay_renders_hits_and_editor_modal_renders_on_top() {
        let mut app = App::new();
        app.overlay = Some(Overlay::Grep(fake_grep_view(vec![
            crate::grep_search::GrepHit {
                path: "src/alpha.rs".into(),
                line: 3,
                text: "let zz = 1;".into(),
            },
            crate::grep_search::GrepHit {
                path: "src/beta.rs".into(),
                line: 14,
                text: "zz += 1;".into(),
            },
        ])));
        let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
        terminal.draw(|f| ui::draw(f, &mut app)).unwrap();
        let text = buffer_text(&terminal);
        assert!(
            text.contains("Find in files — main (2 hits)"),
            "title:\n{text}"
        );
        assert!(text.contains("src/alpha.rs:3"), "hit location:\n{text}");
        assert!(text.contains("let zz = 1;"), "hit text:\n{text}");
        let view = grep_view(&app);
        assert!(view.area.width > 0, "draw writes hit-test area");
        assert!(view.list_area.height > 0, "draw writes list area");

        // Spawn an editor modal: it draws on top and gets its rect written
        // back for the PTY resize sync.
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let dir = tempfile::tempdir().unwrap();
        let mut vim = crate::vim_term::VimTerm::spawn_cmd(
            "/bin/sh",
            &["-c".into(), "sleep 30".into()],
            dir.path(),
            "src/alpha.rs:3".into(),
            80,
            24,
            1,
            tx,
        )
        .unwrap();
        vim.kill();
        app.vim = Some(vim);
        terminal.draw(|f| ui::draw(f, &mut app)).unwrap();
        let text = buffer_text(&terminal);
        assert!(text.contains("src/alpha.rs:3"), "modal title:\n{text}");
        assert!(text.contains("Ctrl+Q: force close"), "hatch hint:\n{text}");
        let vim = app.vim.as_ref().unwrap();
        assert!(vim.area.width > 0, "draw writes the editor rect");
        sync_vim_size(&mut app);
        let vim = app.vim.as_ref().unwrap();
        assert_eq!(
            (vim.cols, vim.rows),
            (vim.area.width, vim.area.height),
            "sync resizes the PTY to the drawn rect"
        );
    }

    // ---- Shift+D bulk delete ----

    /// Shift+D in the worktrees panel confirms deleting EVERY non-main
    /// worktree of the project — itemized in the dialog — and confirming
    /// fires one delete per worktree, dropping the rows optimistically.
    #[test]
    fn shift_d_bulk_deletes_worktrees_behind_an_itemized_confirm() {
        use nebula_core::{Entity, Worktree, WorktreeId};
        let mut app = App::new();
        seed_tree(&mut app); // p1/w1(main) + agent-1
        let mut out = Vec::new();
        for (id, branch) in [("w2", "feat"), ("w3", "fix")] {
            hse(
                &mut app,
                ServerEvent::EntityUpserted {
                    entity: Entity::Worktree(Worktree {
                        id: WorktreeId(id.into()),
                        project_id: nebula_core::ProjectId("p1".into()),
                        path: format!("/tmp/demo-worktrees/{branch}").into(),
                        branch: branch.into(),
                        is_main: false,
                        pinned: false,
                        sort_order: 0,
                    }),
                },
            );
        }
        app.focus = Focus::Worktrees;

        press(&mut app, KeyCode::Char('D'), KeyModifiers::SHIFT, &mut out);
        let Some(Overlay::Confirm(c)) = &app.overlay else {
            panic!("Shift+D confirms first: {:?}", app.overlay);
        };
        assert!(
            c.message.contains("• feat") && c.message.contains("• fix"),
            "casualties are itemized: {}",
            c.message
        );
        assert!(
            !c.message.contains("• main"),
            "main checkout is not on the kill list: {}",
            c.message
        );
        assert!(
            matches!(&c.action, PendingAction::DeleteAllWorktrees(ids) if ids.len() == 2),
            "main checkout excluded from the action: {:?}",
            c.action
        );

        // The dialog really shows the list (multi-line confirm rendering).
        let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
        terminal.draw(|f| ui::draw(f, &mut app)).unwrap();
        let text = buffer_text(&terminal);
        assert!(text.contains("• feat"), "dialog lists feat:\n{text}");
        assert!(text.contains("• fix"), "dialog lists fix:\n{text}");

        press(&mut app, KeyCode::Char('y'), KeyModifiers::NONE, &mut out);
        let deleted: Vec<&str> = out
            .iter()
            .filter_map(|r| match r {
                ClientRequest::DeleteWorktree { id, .. } => Some(id.0.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(deleted, ["w2", "w3"], "one request per worktree: {out:?}");
        assert!(app.overlay.is_none());
        let left: Vec<&str> = app.tree.worktrees.iter().map(|w| w.id.0.as_str()).collect();
        assert_eq!(left, ["w1"], "only the main checkout survives");
    }

    /// With only the main checkout, Shift+D has nothing to offer — flash,
    /// no dialog.
    #[test]
    fn shift_d_with_only_the_main_checkout_flashes() {
        let mut app = App::new();
        seed_tree(&mut app); // p1/w1(main) + agent-1
        let mut out = Vec::new();
        app.focus = Focus::Worktrees;
        press(&mut app, KeyCode::Char('D'), KeyModifiers::SHIFT, &mut out);
        assert!(app.overlay.is_none(), "nothing to confirm");
        assert!(app.flash.is_some(), "the refusal explains itself");
        assert!(out.is_empty(), "nothing is requested");
    }

    /// Shift+D in the sessions panel confirms deleting every LISTED session
    /// — hidden archived rows are spared — and an attached doomed session
    /// detaches before its delete.
    #[test]
    fn shift_d_bulk_deletes_the_visible_sessions() {
        use nebula_core::{Agent, AgentStatus, Entity};
        let mut app = App::new();
        seed_tree(&mut app); // p1/w1(main) + agent-1
        for (id, name, archived) in [("a2", "agent-2", false), ("a3", "agent-3", true)] {
            hse(
                &mut app,
                ServerEvent::EntityUpserted {
                    entity: Entity::Agent(Agent {
                        id: AgentId(id.into()),
                        worktree_id: WorktreeId("w1".into()),
                        name: name.into(),
                        status: AgentStatus::Fresh,
                        archived,
                        archived_at: 0,
                        pinned: false,
                        kind: nebula_core::AgentKind::Claude,
                        model: None,
                        effort: None,
                        session_id: None,
                        sort_order: 1,
                        status_changed_at: 0,
                        alive: true,
                    }),
                },
            );
        }
        app.focus = Focus::Sessions;
        let sref = SessionRef::Agent(AgentId("a1".into()));
        app.term = Some(AttachedTerm::new(sref.clone(), 40, 10));
        let mut out = Vec::new();

        press(&mut app, KeyCode::Char('D'), KeyModifiers::SHIFT, &mut out);
        let Some(Overlay::Confirm(c)) = &app.overlay else {
            panic!("Shift+D confirms first: {:?}", app.overlay);
        };
        assert!(
            c.message.contains("• agent-1") && c.message.contains("• agent-2"),
            "listed sessions are itemized: {}",
            c.message
        );
        assert!(
            !c.message.contains("agent-3"),
            "hidden archived rows are spared: {}",
            c.message
        );

        press(&mut app, KeyCode::Enter, KeyModifiers::NONE, &mut out);
        assert!(
            matches!(out.first(), Some(ClientRequest::Detach { session }) if *session == sref),
            "attached doomed session detaches first: {out:?}"
        );
        assert!(app.term.is_none(), "the pane blanks with the detach");
        let deleted: Vec<&str> = out
            .iter()
            .filter_map(|r| match r {
                ClientRequest::DeleteAgent { id, .. } => Some(id.0.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(deleted, ["a1", "a2"], "one request per session: {out:?}");
    }

    fn wt_entity(id: &str, project: &str, branch: &str, is_main: bool) -> nebula_core::Entity {
        use nebula_core::{Entity, Worktree};
        Entity::Worktree(Worktree {
            id: WorktreeId(id.into()),
            project_id: nebula_core::ProjectId(project.into()),
            path: format!("/tmp/{branch}").into(),
            branch: branch.into(),
            is_main,
            pinned: false,
            sort_order: 0,
        })
    }

    fn agent_entity(id: &str, wt: &str, name: &str, archived: bool) -> nebula_core::Entity {
        use nebula_core::{Agent, AgentStatus, Entity};
        Entity::Agent(Agent {
            id: AgentId(id.into()),
            worktree_id: WorktreeId(wt.into()),
            name: name.into(),
            status: AgentStatus::Fresh,
            archived,
            archived_at: 0,
            pinned: false,
            kind: nebula_core::AgentKind::Claude,
            model: None,
            effort: None,
            session_id: None,
            sort_order: 1,
            status_changed_at: 0,
            alive: true,
        })
    }

    /// Archiving the selected session lands the cursor on the next row AND
    /// attaches it — the pane must show the newly highlighted session, not
    /// stay blank after the archive's detach.
    #[test]
    fn archiving_selected_agent_previews_the_next_row() {
        let mut app = App::new();
        seed_tree(&mut app); // p1 / w1(main) / a1
        hse(
            &mut app,
            ServerEvent::EntityUpserted {
                entity: agent_entity("a2", "w1", "agent-2", false),
            },
        );
        app.focus = Focus::Sessions;
        app.sel_session = 0; // a1
        let a1 = SessionRef::Agent(AgentId("a1".into()));
        app.term = Some(AttachedTerm::new(a1.clone(), 40, 10));

        let mut out = Vec::new();
        press(&mut app, KeyCode::Char('a'), KeyModifiers::NONE, &mut out);
        assert!(
            out.iter()
                .any(|r| matches!(r, ClientRequest::ArchiveAgent { .. })),
            "a requests the archive: {out:?}"
        );

        // The daemon's upsert flips the archived flag; the row leaves the
        // list, the cursor lands on agent-2, and agent-2 gets shown.
        out.clear();
        handle_server_event(
            &mut app,
            ServerEvent::EntityUpserted {
                entity: agent_entity("a1", "w1", "agent-1", true),
            },
            &mut out,
        );
        let a2 = SessionRef::Agent(AgentId("a2".into()));
        assert_eq!(
            app.selected_session().map(|a| a.name),
            Some("agent-2".into()),
            "cursor landed on the next row"
        );
        assert!(
            out.iter()
                .any(|r| matches!(r, ClientRequest::Attach { session, .. } if *session == a2)),
            "the next row's session attaches: {out:?}"
        );
        assert_eq!(
            app.term.as_ref().map(|t| t.sref.clone()),
            Some(a2),
            "the pane shows the newly highlighted session"
        );
    }

    /// Archiving a row ABOVE the cursor must not drag the highlight onto a
    /// different session — the cursor follows the session it was on.
    #[test]
    fn archiving_a_row_above_keeps_the_cursor_on_its_session() {
        let mut app = App::new();
        seed_tree(&mut app); // p1 / w1(main) / a1
        hse(
            &mut app,
            ServerEvent::EntityUpserted {
                entity: agent_entity("a2", "w1", "agent-2", false),
            },
        );
        app.focus = Focus::Sessions;
        app.sel_session = 1; // a2
        let a2 = SessionRef::Agent(AgentId("a2".into()));
        app.term = Some(AttachedTerm::new(a2.clone(), 40, 10));

        let mut out = Vec::new();
        handle_server_event(
            &mut app,
            ServerEvent::EntityUpserted {
                entity: agent_entity("a1", "w1", "agent-1", true),
            },
            &mut out,
        );
        assert_eq!(
            app.selected_session().map(|a| a.name),
            Some("agent-2".into()),
            "cursor followed its session up the list"
        );
        assert_eq!(
            app.term.as_ref().map(|t| t.sref.clone()),
            Some(a2),
            "the attached pane is untouched"
        );
        assert!(
            !out.iter()
                .any(|r| matches!(r, ClientRequest::Attach { .. })),
            "no re-attach when the highlighted session didn't change: {out:?}"
        );
    }

    /// Deleting the selected session lands the cursor on the next row and
    /// shows it in the pane.
    #[test]
    fn deleting_selected_agent_previews_the_next_row() {
        use nebula_core::EntityId;
        let mut app = App::new();
        seed_tree(&mut app); // p1 / w1(main) / a1
        hse(
            &mut app,
            ServerEvent::EntityUpserted {
                entity: agent_entity("a2", "w1", "agent-2", false),
            },
        );
        app.focus = Focus::Sessions;
        app.sel_session = 0; // a1
        let a1 = SessionRef::Agent(AgentId("a1".into()));
        app.term = Some(AttachedTerm::new(a1.clone(), 40, 10));

        let mut out = Vec::new();
        handle_server_event(
            &mut app,
            ServerEvent::EntityRemoved {
                id: EntityId::Agent(AgentId("a1".into())),
            },
            &mut out,
        );
        let a2 = SessionRef::Agent(AgentId("a2".into()));
        assert_eq!(
            app.selected_session().map(|a| a.name),
            Some("agent-2".into()),
            "cursor landed on the next row"
        );
        assert!(
            out.iter()
                .any(|r| matches!(r, ClientRequest::Attach { session, .. } if *session == a2)),
            "the next row's session attaches: {out:?}"
        );
        assert_eq!(app.term.as_ref().map(|t| t.sref.clone()), Some(a2));
    }

    /// Removing the only session leaves nothing to preview: the pane blanks
    /// instead of keeping the dead session's screen.
    #[test]
    fn deleting_the_last_session_blanks_the_pane() {
        use nebula_core::EntityId;
        let mut app = App::new();
        seed_tree(&mut app); // p1 / w1(main) / a1
        let a1 = SessionRef::Agent(AgentId("a1".into()));
        app.term = Some(AttachedTerm::new(a1.clone(), 40, 10));
        app.focus = Focus::Terminal;
        app.term_locked = true;

        let mut out = Vec::new();
        handle_server_event(
            &mut app,
            ServerEvent::EntityRemoved {
                id: EntityId::Agent(AgentId("a1".into())),
            },
            &mut out,
        );
        assert!(app.term.is_none(), "the pane blanks");
        assert_eq!(app.focus, Focus::Sessions, "focus hands back to the list");
        assert!(
            out.iter()
                .any(|r| matches!(r, ClientRequest::Detach { session } if *session == a1)),
            "the dead session detaches: {out:?}"
        );
    }

    /// Deleting the selected worktree lands the cursor on a neighbor and
    /// brings up that neighbor's remembered session, like a manual switch.
    #[test]
    fn deleting_selected_worktree_shows_the_neighbor_worktrees_session() {
        let mut app = App::new();
        seed_tree(&mut app); // p1 / w1(main) / a1
        hse(
            &mut app,
            ServerEvent::EntityUpserted {
                entity: wt_entity("w2", "p1", "feat", false),
            },
        );
        hse(
            &mut app,
            ServerEvent::EntityUpserted {
                entity: agent_entity("a2", "w2", "agent-2", false),
            },
        );
        let w2_index = app
            .visible_worktrees()
            .iter()
            .position(|w| w.id.0 == "w2")
            .unwrap();
        app.sel_worktree = w2_index;
        app.sel_session = 0; // a2
        app.focus = Focus::Worktrees;
        let a1 = SessionRef::Agent(AgentId("a1".into()));
        let a2 = SessionRef::Agent(AgentId("a2".into()));
        app.term = Some(AttachedTerm::new(a2.clone(), 40, 10));
        app.last_session_for_worktree
            .insert(WorktreeId("w1".into()), a1.clone());

        let mut out = Vec::new();
        run_pending_action(
            &mut app,
            PendingAction::DeleteWorktree(WorktreeId("w2".into())),
            &mut out,
        );
        assert_eq!(
            app.selected_worktree().map(|w| w.id.0.clone()),
            Some("w1".into()),
            "cursor landed on the surviving worktree"
        );
        assert!(
            out.iter()
                .any(|r| matches!(r, ClientRequest::Attach { session, .. } if *session == a1)),
            "the survivor's remembered session attaches: {out:?}"
        );
        assert_eq!(
            app.term.as_ref().map(|t| t.sref.clone()),
            Some(a1),
            "the pane shows the survivor's session, not the deleted one"
        );
    }

    /// Removing the selected project restores the neighbor project's
    /// remembered worktree + session, like switching to it manually.
    #[test]
    fn removing_selected_project_restores_the_neighbor_projects_context() {
        use nebula_core::EntityId;
        let mut app = App::new();
        seed_tree(&mut app); // p1 / w1(main) / a1
        hse(
            &mut app,
            ServerEvent::EntityUpserted {
                entity: project("p2", "two", 1, false, None),
            },
        );
        hse(
            &mut app,
            ServerEvent::EntityUpserted {
                entity: wt_entity("w2", "p2", "main2", true),
            },
        );
        hse(
            &mut app,
            ServerEvent::EntityUpserted {
                entity: agent_entity("a2", "w2", "agent-2", false),
            },
        );
        app.sel_project = 1; // p2
        app.sel_worktree = 0; // w2
        app.sel_session = 0; // a2
        app.focus = Focus::Projects;
        let a1 = SessionRef::Agent(AgentId("a1".into()));
        let a2 = SessionRef::Agent(AgentId("a2".into()));
        app.term = Some(AttachedTerm::new(a2.clone(), 40, 10));
        app.last_worktree_for_project
            .insert(nebula_core::ProjectId("p1".into()), WorktreeId("w1".into()));
        app.last_session_for_worktree
            .insert(WorktreeId("w1".into()), a1.clone());

        let mut out = Vec::new();
        handle_server_event(
            &mut app,
            ServerEvent::EntityRemoved {
                id: EntityId::Project(nebula_core::ProjectId("p2".into())),
            },
            &mut out,
        );
        assert_eq!(
            app.selected_project().map(|p| p.name.clone()),
            Some("demo".into()),
            "cursor landed on the surviving project"
        );
        assert_eq!(
            app.selected_worktree().map(|w| w.id.0.clone()),
            Some("w1".into()),
            "its remembered worktree is selected"
        );
        assert_eq!(
            app.term.as_ref().map(|t| t.sref.clone()),
            Some(a1),
            "the pane shows the survivor's remembered session"
        );
    }

    // ---- workspaces ----

    /// A second workspace ("client") holding project "secret", next to
    /// `seed_tree`'s demo project in the default workspace.
    fn seed_other_workspace(app: &mut App) {
        use nebula_core::{
            Entity, Project, ProjectId, Workspace, WorkspaceId, Worktree, WorktreeId,
        };
        hse(
            app,
            ServerEvent::EntityUpserted {
                entity: Entity::Workspace(Workspace {
                    id: WorkspaceId("ws2".into()),
                    name: "client".into(),
                }),
            },
        );
        hse(
            app,
            ServerEvent::EntityUpserted {
                entity: Entity::Project(Project {
                    workspace_id: WorkspaceId("ws2".into()),
                    id: ProjectId("p9".into()),
                    name: "secret".into(),
                    repo_path: "/tmp/secret".into(),
                    sort_order: 9,
                    divider_after: false,
                    divider_label: None,
                    divider_before: false,
                    divider_before_label: None,
                }),
            },
        );
        hse(
            app,
            ServerEvent::EntityUpserted {
                entity: Entity::Worktree(Worktree {
                    id: WorktreeId("w9".into()),
                    project_id: ProjectId("p9".into()),
                    path: "/tmp/secret".into(),
                    branch: "main".into(),
                    is_main: true,
                    pinned: false,
                    sort_order: 0,
                }),
            },
        );
    }

    /// Projects outside the open workspace get no panel row, don't count
    /// toward the header, and never surface in the `/` palette.
    #[test]
    fn other_workspaces_projects_are_hidden_and_unsearchable() {
        let mut app = App::new();
        seed_tree(&mut app);
        seed_other_workspace(&mut app);
        assert_eq!(app.project_rows().len(), 1, "only demo has a row");
        assert_eq!(app.tree.visible_project_count(), 1);

        let palette = Palette::new(&app.tree, true, false);
        assert!(
            !palette.items.is_empty() && palette.items.iter().all(|i| !i.text.contains("secret")),
            "palette must not search other workspaces: {:?}",
            palette.items.iter().map(|i| &i.text).collect::<Vec<_>>()
        );
    }

    /// ActiveWorkspaceChanged re-filters everything live: panel rows, an
    /// open palette, the selection, and the footer's workspace name.
    #[test]
    fn switching_workspace_refilters_rows_palette_and_footer() {
        let mut app = App::new();
        seed_tree(&mut app);
        seed_other_workspace(&mut app);

        let mut terminal = Terminal::new(TestBackend::new(120, 30)).unwrap();
        terminal.draw(|f| ui::draw(f, &mut app)).unwrap();
        let text = buffer_text(&terminal);
        assert!(text.contains("◇ default"), "{text}");
        assert!(text.contains("demo"), "{text}");
        assert!(!text.contains("secret"), "{text}");

        app.overlay = Some(Overlay::Palette(Palette::new(&app.tree, false, false)));
        let mut out = Vec::new();
        handle_server_event(
            &mut app,
            ServerEvent::ActiveWorkspaceChanged {
                id: nebula_core::WorkspaceId("ws2".into()),
            },
            &mut out,
        );
        assert_eq!(
            app.selected_project().map(|p| p.name.clone()),
            Some("secret".into()),
            "selection lands in the opened workspace"
        );
        match &app.overlay {
            Some(Overlay::Palette(palette)) => assert!(
                palette.items.iter().all(|i| !i.text.contains("demo")),
                "open palette re-scopes to the new workspace"
            ),
            other => panic!("palette should stay open, got {other:?}"),
        }

        terminal.draw(|f| ui::draw(f, &mut app)).unwrap();
        let text = buffer_text(&terminal);
        assert!(text.contains("◇ client"), "{text}");
        assert!(text.contains("secret"), "{text}");
        assert!(!text.contains("demo"), "{text}");
    }

    /// Opening an empty workspace clears the child panels and the terminal
    /// pane instead of keeping the previous workspace's session on screen.
    #[test]
    fn switching_to_empty_workspace_blanks_the_pane() {
        use nebula_core::{Entity, Workspace, WorkspaceId};
        let mut app = App::new();
        seed_tree(&mut app);
        hse(
            &mut app,
            ServerEvent::EntityUpserted {
                entity: Entity::Workspace(Workspace {
                    id: WorkspaceId("ws-empty".into()),
                    name: "fresh".into(),
                }),
            },
        );
        let mut out = Vec::new();
        attach(&mut app, SessionRef::Agent(AgentId("a1".into())), &mut out);
        assert!(app.term.is_some());
        handle_server_event(
            &mut app,
            ServerEvent::ActiveWorkspaceChanged {
                id: WorkspaceId("ws-empty".into()),
            },
            &mut out,
        );
        assert!(app.project_rows().is_empty(), "no visible projects");
        assert!(app.term.is_none(), "pane blanked");
        assert!(
            out.iter()
                .any(|r| matches!(r, ClientRequest::Detach { .. })),
            "old session detached: {out:?}"
        );
        assert!(app.splash_active(), "empty workspace shows the splash");
    }

    /// `w` opens the workspace switcher with the open workspace checked and
    /// highlighted; Enter on another row asks the daemon to open it.
    #[test]
    fn w_key_opens_workspace_switcher_and_enter_switches() {
        use nebula_core::{Entity, Workspace, WorkspaceId};
        let mut app = App::new();
        hse(
            &mut app,
            ServerEvent::EntityUpserted {
                entity: Entity::Workspace(Workspace {
                    id: WorkspaceId::default(),
                    name: "default".into(),
                }),
            },
        );
        seed_tree(&mut app);
        seed_other_workspace(&mut app);

        let mut out = Vec::new();
        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('w'), KeyModifiers::NONE),
            &mut out,
        );
        let Some(Overlay::Menu(menu)) = &app.overlay else {
            panic!("workspace switcher should open");
        };
        assert_eq!(menu.title.as_deref(), Some("Workspace"));
        assert_eq!(menu.items.len(), 2);
        assert!(
            menu.items[0].label.contains("default ✓"),
            "active workspace checked: {}",
            menu.items[0].label
        );
        assert_eq!(menu.hover, 0, "active row starts highlighted");

        // The key verbs ride the modal's bottom border (notes-modal style).
        let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
        terminal.draw(|f| ui::draw(f, &mut app)).unwrap();
        let text = buffer_text(&terminal);
        assert!(
            text.contains("n: new  r: rename  d: delete"),
            "hints at the bottom of the modal: {text}"
        );

        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE),
            &mut out,
        );
        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
            &mut out,
        );
        assert!(
            matches!(
                out.last(),
                Some(ClientRequest::OpenWorkspace { id, .. }) if id.as_str() == "ws2"
            ),
            "Enter requests the switch: {out:?}"
        );
        assert!(app.overlay.is_none(), "menu closed");

        // Picking the already-open workspace sends nothing.
        let mut out = Vec::new();
        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('w'), KeyModifiers::NONE),
            &mut out,
        );
        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
            &mut out,
        );
        assert!(out.is_empty(), "re-picking the open workspace is a no-op");
    }

    /// `n` in the switcher prompts for a name, creates the workspace, and
    /// opens it as soon as the daemon acks the create.
    #[test]
    fn switcher_creates_a_workspace_and_opens_it_on_ack() {
        use nebula_core::{Entity, EntityId, Workspace, WorkspaceId};
        let mut app = App::new();
        hse(
            &mut app,
            ServerEvent::EntityUpserted {
                entity: Entity::Workspace(Workspace {
                    id: WorkspaceId::default(),
                    name: "default".into(),
                }),
            },
        );
        seed_tree(&mut app);

        let mut out = Vec::new();
        let press = |app: &mut App, code, out: &mut Vec<ClientRequest>| {
            handle_key(app, KeyEvent::new(code, KeyModifiers::NONE), out);
        };
        press(&mut app, KeyCode::Char('w'), &mut out);
        press(&mut app, KeyCode::Char('n'), &mut out);
        match &app.overlay {
            Some(Overlay::Prompt(p)) => assert_eq!(p.title, "New workspace"),
            other => panic!("name prompt should open, got {other:?}"),
        }
        for c in "acme".chars() {
            press(&mut app, KeyCode::Char(c), &mut out);
        }
        press(&mut app, KeyCode::Enter, &mut out);
        let req_id = match out.last() {
            Some(ClientRequest::AddWorkspace { req_id, name }) => {
                assert_eq!(name, "acme");
                *req_id
            }
            other => panic!("expected AddWorkspace, got {other:?}"),
        };

        // The Ack carries the created id; the switch request follows.
        let mut out = Vec::new();
        handle_server_event(
            &mut app,
            ServerEvent::Ack {
                req_id,
                created: Some(EntityId::Workspace(nebula_core::WorkspaceId(
                    "ws-new".into(),
                ))),
            },
            &mut out,
        );
        assert!(
            matches!(
                out.last(),
                Some(ClientRequest::OpenWorkspace { id, .. }) if id.as_str() == "ws-new"
            ),
            "created workspace gets opened: {out:?}"
        );
    }

    /// `r` and `d` in the switcher act on the hovered workspace (the
    /// notes-modal pattern — footer hints, no submenus); after a delete the
    /// open switcher refreshes its rows in place.
    #[test]
    fn switcher_r_and_d_act_on_the_hovered_workspace() {
        use nebula_core::{Entity, EntityId, Workspace, WorkspaceId};
        let mut app = App::new();
        hse(
            &mut app,
            ServerEvent::EntityUpserted {
                entity: Entity::Workspace(Workspace {
                    id: WorkspaceId::default(),
                    name: "default".into(),
                }),
            },
        );
        seed_tree(&mut app);
        seed_other_workspace(&mut app); // "client" (ws2)

        let mut out = Vec::new();
        let press = |app: &mut App, code, out: &mut Vec<ClientRequest>| {
            handle_key(app, KeyEvent::new(code, KeyModifiers::NONE), out);
        };

        // r: rename prompt prefilled with the hovered workspace's name.
        press(&mut app, KeyCode::Char('w'), &mut out);
        press(&mut app, KeyCode::Char('j'), &mut out); // onto "client"
        press(&mut app, KeyCode::Char('r'), &mut out);
        match &app.overlay {
            Some(Overlay::Prompt(p)) => {
                assert_eq!(p.title, "Rename workspace");
                assert_eq!(p.input, "client");
            }
            other => panic!("rename prompt should open, got {other:?}"),
        }
        press(&mut app, KeyCode::Enter, &mut out);
        assert!(
            matches!(
                out.last(),
                Some(ClientRequest::RenameWorkspace { id, name, .. })
                    if id.as_str() == "ws2" && name == "client"
            ),
            "rename request sent: {out:?}"
        );

        // d: straight to the request (the daemon guards misuse); the menu
        // stays up and drops the row when the removal delta lands.
        let mut out = Vec::new();
        press(&mut app, KeyCode::Char('w'), &mut out);
        press(&mut app, KeyCode::Char('j'), &mut out);
        press(&mut app, KeyCode::Char('d'), &mut out);
        assert!(
            matches!(
                out.last(),
                Some(ClientRequest::RemoveWorkspace { id, .. }) if id.as_str() == "ws2"
            ),
            "delete request sent: {out:?}"
        );
        assert!(
            matches!(&app.overlay, Some(Overlay::Menu(_))),
            "switcher stays open"
        );
        hse(
            &mut app,
            ServerEvent::EntityRemoved {
                id: EntityId::Workspace(WorkspaceId("ws2".into())),
            },
        );
        match &app.overlay {
            Some(Overlay::Menu(menu)) => {
                assert_eq!(menu.items.len(), 1, "deleted row dropped in place");
                assert!(menu.items[0].label.contains("default"));
                assert_eq!(menu.hover, 0, "cursor clamped onto a live row");
            }
            other => panic!("switcher should stay open, got {other:?}"),
        }
    }

    // ---- ssh hosts picker ----

    /// Route the host store at a temp file and pre-seed it with two
    /// destinations, "old@one" first, then "new@two /srv/app" (so the list
    /// reads newest-first: new@two, old@one).
    fn with_seeded_hosts(f: impl FnOnce()) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ssh_hosts.json");
        crate::hosts::with_hosts_path(path, || {
            crate::hosts::record("old@one", None);
            crate::hosts::record("new@two", Some("/srv/app"));
            f();
        });
    }

    #[test]
    fn h_opens_hosts_picker_newest_first() {
        with_seeded_hosts(|| {
            let mut app = App::new();
            let mut out = Vec::new();
            press(&mut app, KeyCode::Char('h'), KeyModifiers::NONE, &mut out);
            let Some(Overlay::Hosts(view)) = &app.overlay else {
                panic!("h should open the hosts picker, got {:?}", app.overlay);
            };
            assert_eq!(view.hosts.len(), 2);
            assert_eq!(view.hosts[0].host, "new@two", "most recent first");
            assert_eq!(view.hosts[0].path.as_deref(), Some("/srv/app"));
            assert_eq!(view.hosts[1].host, "old@one");
            assert_eq!(view.selected, 0);

            let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
            terminal.draw(|f| ui::draw(f, &mut app)).unwrap();
            let text = buffer_text(&terminal);
            assert!(text.contains("SSH Hosts"), "title rendered:\n{text}");
            assert!(text.contains("new@two"), "hosts rendered:\n{text}");
            assert!(text.contains("/srv/app"), "start dir rendered:\n{text}");
            assert!(text.contains("just now"), "ago label rendered:\n{text}");

            press(&mut app, KeyCode::Esc, KeyModifiers::NONE, &mut out);
            assert!(app.overlay.is_none(), "Esc closes the picker");
            assert!(!app.should_quit);
        });
    }

    #[test]
    fn hosts_enter_quits_with_the_selected_destination() {
        with_seeded_hosts(|| {
            let mut app = App::new();
            let mut out = Vec::new();
            press(&mut app, KeyCode::Char('h'), KeyModifiers::NONE, &mut out);
            press(&mut app, KeyCode::Char('j'), KeyModifiers::NONE, &mut out);
            press(&mut app, KeyCode::Enter, KeyModifiers::NONE, &mut out);
            assert!(app.should_quit, "Enter hands off by quitting");
            assert!(app.overlay.is_none());
            let entry = app.pending_ssh.as_ref().expect("handoff target set");
            assert_eq!(entry.host, "old@one");
            assert_eq!(entry.path, None);
        });
    }

    #[test]
    fn hosts_d_removes_the_entry_and_persists() {
        with_seeded_hosts(|| {
            let mut app = App::new();
            let mut out = Vec::new();
            press(&mut app, KeyCode::Char('h'), KeyModifiers::NONE, &mut out);
            press(&mut app, KeyCode::Char('d'), KeyModifiers::NONE, &mut out);
            match &app.overlay {
                Some(Overlay::Hosts(view)) => {
                    assert_eq!(view.hosts.len(), 1, "row dropped in place");
                    assert_eq!(view.hosts[0].host, "old@one");
                    assert_eq!(view.selected, 0, "cursor clamped");
                }
                other => panic!("picker should stay open, got {other:?}"),
            }
            let left = crate::hosts::load();
            assert_eq!(left.len(), 1, "removal reached the store");
            assert_eq!(left[0].host, "old@one");
        });
    }

    #[test]
    fn hosts_click_on_a_row_connects_and_outside_closes() {
        with_seeded_hosts(|| {
            let mut app = App::new();
            let mut out = Vec::new();
            press(&mut app, KeyCode::Char('h'), KeyModifiers::NONE, &mut out);
            // Draw once so the modal writes back its hit-test rects.
            let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
            terminal.draw(|f| ui::draw(f, &mut app)).unwrap();
            let list = match &app.overlay {
                Some(Overlay::Hosts(view)) => view.list_area,
                other => panic!("picker open, got {other:?}"),
            };
            // Click the second row: connect to it.
            handle_mouse(
                &mut app,
                mev(
                    MouseEventKind::Down(MouseButton::Left),
                    list.x + 1,
                    list.y + 1,
                ),
                &mut out,
            );
            assert!(app.should_quit, "click connects");
            assert_eq!(app.pending_ssh.as_ref().unwrap().host, "old@one");

            // Reopened, a click outside the modal closes it.
            app.should_quit = false;
            app.pending_ssh = None;
            press(&mut app, KeyCode::Char('h'), KeyModifiers::NONE, &mut out);
            terminal.draw(|f| ui::draw(f, &mut app)).unwrap();
            handle_mouse(
                &mut app,
                mev(MouseEventKind::Down(MouseButton::Left), 0, 0),
                &mut out,
            );
            assert!(app.overlay.is_none(), "outside click closes");
            assert!(!app.should_quit);
        });
    }

    #[test]
    fn hosts_a_types_a_new_destination_and_enter_connects() {
        with_seeded_hosts(|| {
            let mut app = App::new();
            let mut out = Vec::new();
            press(&mut app, KeyCode::Char('h'), KeyModifiers::NONE, &mut out);
            press(&mut app, KeyCode::Char('a'), KeyModifiers::NONE, &mut out);
            // While typing, list verbs are just characters — q must not
            // close, d must not delete.
            for c in "qd@db /var".chars() {
                press(&mut app, KeyCode::Char(c), KeyModifiers::NONE, &mut out);
            }
            match &app.overlay {
                Some(Overlay::Hosts(view)) => {
                    assert_eq!(view.input.as_deref(), Some("qd@db /var"));
                    assert_eq!(view.hosts.len(), 2, "d typed, not deleted");
                }
                other => panic!("picker should stay open, got {other:?}"),
            }
            // Draw shows the input row.
            let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
            terminal.draw(|f| ui::draw(f, &mut app)).unwrap();
            let text = buffer_text(&terminal);
            assert!(text.contains("+ qd@db /var"), "input row rendered:\n{text}");

            press(&mut app, KeyCode::Enter, KeyModifiers::NONE, &mut out);
            assert!(app.should_quit, "Enter connects to the typed host");
            let entry = app.pending_ssh.as_ref().expect("handoff target set");
            assert_eq!(entry.host, "qd@db");
            assert_eq!(entry.path.as_deref(), Some("/var"));
        });
    }

    #[test]
    fn hosts_input_esc_cancels_and_empty_enter_is_a_noop() {
        with_seeded_hosts(|| {
            let mut app = App::new();
            let mut out = Vec::new();
            press(&mut app, KeyCode::Char('h'), KeyModifiers::NONE, &mut out);
            press(&mut app, KeyCode::Char('a'), KeyModifiers::NONE, &mut out);
            press(&mut app, KeyCode::Enter, KeyModifiers::NONE, &mut out);
            match &app.overlay {
                Some(Overlay::Hosts(view)) => {
                    assert!(view.input.is_none(), "empty Enter cancels the input");
                }
                other => panic!("picker should stay open, got {other:?}"),
            }
            assert!(!app.should_quit);
            press(&mut app, KeyCode::Char('a'), KeyModifiers::NONE, &mut out);
            press(&mut app, KeyCode::Char('x'), KeyModifiers::NONE, &mut out);
            press(&mut app, KeyCode::Esc, KeyModifiers::NONE, &mut out);
            match &app.overlay {
                Some(Overlay::Hosts(view)) => assert!(view.input.is_none()),
                other => panic!("Esc only cancels the input, got {other:?}"),
            }
        });
    }

    #[test]
    fn empty_hosts_picker_shows_the_hint() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ssh_hosts.json");
        crate::hosts::with_hosts_path(path, || {
            let mut app = App::new();
            let mut out = Vec::new();
            press(&mut app, KeyCode::Char('h'), KeyModifiers::NONE, &mut out);
            assert!(matches!(app.overlay, Some(Overlay::Hosts(_))));
            let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
            terminal.draw(|f| ui::draw(f, &mut app)).unwrap();
            let text = buffer_text(&terminal);
            assert!(
                text.contains("no hosts yet"),
                "empty state introduces the feature:\n{text}"
            );
            // d on the empty list must not panic or write.
            press(&mut app, KeyCode::Char('d'), KeyModifiers::NONE, &mut out);
            press(&mut app, KeyCode::Enter, KeyModifiers::NONE, &mut out);
            assert!(!app.should_quit, "Enter on an empty list is a no-op");
        });
    }
}
