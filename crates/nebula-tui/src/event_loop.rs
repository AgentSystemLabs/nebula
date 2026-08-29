//! The main TUI loop: terminal setup/teardown, message routing, update logic.

use crate::app::{
    clamp_selection, App, AttachedTerm, ConfirmDialog, ConnState, ContextMenu, DiffView,
    FileFinder, Focus, GrepView, HelpView, HitTarget, LinkRow, MenuAction, MenuFilter, MenuItem,
    MetricsView, Overlay, Palette, PaletteTarget, PendingAction, PendingIntent, PointerShape,
    PromptDialog, PromptKind, RowKey, SessionRow, SettingsView, SplitterDrag, SubmenuKind,
    TermSelection, WorktreeRollback,
};
use crate::pull_request::PullRequest;
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
    AgentId, AgentKind, ClientRequest, EntityId, ProjectId, ServerEvent, SessionRef, TerminalId,
    WorkspaceId, WorktreeId, MAX_CLOUD_PROMPT_BYTES,
};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;
use std::io::{BufWriter, Stdout};
use std::time::Duration;

mod focus_walk;
use focus_walk::{
    at_top_row, bar_return_target, double_tapped, enter_terminal_pane, enter_workspaces_bar,
    leave_workspaces_bar, panel_name, walk_focus_back, walk_focus_forward,
};

/// Rows the Sessions column scrolls per wheel notch — one pill's stride,
/// so the list steps by whole rows instead of drifting half a pill.
const SESSIONS_WHEEL_STEP: usize = 2;

/// Wheel step for the pull-request reading pane, in lines. Prose wants a
/// bigger bite than a session list of two-row pills.
const PR_PREVIEW_WHEEL_STEP: u16 = 3;

/// Wheel step inside the diff and tree modals' reading panes, in lines.
const MODAL_WHEEL_LINES: i32 = 3;

/// Wheel step over the terminal pane, in lines: how far the scrollback
/// offset moves, and how many arrow keys an alt-screen app that ignores
/// the mouse is sent per notch.
const TERM_WHEEL_LINES: usize = 3;

/// Ceiling on a panel or file-list width read back from the persisted UI
/// state — a coarse sanity clamp; the draw re-fits it to the real screen.
const MAX_RESTORED_WIDTH: u16 = 300;

/// Smallest grid worth sizing a PTY or vt100 parser to: below this a pane
/// hasn't really been drawn yet.
const MIN_PANE_DIM: u16 = 2;

/// Terminal-pane grid assumed before the first draw, so a spawn or attach
/// requested that early never boots a 0×0 PTY.
const FALLBACK_PANE: (u16, u16) = (80, 24);

/// Where a keyboard-invoked context menu is anchored: near the panels,
/// where the selected row lives.
const KEYBOARD_MENU_ANCHOR: (u16, u16) = (30, 4);

/// Bracketed-paste markers around a pasted block, so the child (claude,
/// vim…) takes it as one paste rather than typing to auto-indent.
const PASTE_START: &[u8] = b"\x1b[200~";
const PASTE_END: &[u8] = b"\x1b[201~";

/// Flash for an action that needs a checkout to act on and has none.
const SELECT_CONTEXT_FIRST: &str = "select a project or worktree first";

/// Flash for a session pick that lost a race with its removal.
const SESSION_GONE: &str = "session no longer exists";

/// Flash for an action an archived agent refuses until it's unarchived.
const AGENT_ARCHIVED: &str = "agent is archived — unarchive first (u)";

/// Redraw cap (~60fps). Output bursts coalesce into one frame; input events
/// are still handled immediately between frames.
const FRAME_INTERVAL: Duration = Duration::from_millis(16);

/// How often the worktree panel's changed-file badge re-reads `git status`
/// for the selected checkout, so agent edits surface without a keypress.
const GIT_POLL: Duration = Duration::from_secs(2);

/// Repaint cadence for the sessions list's "23m ago" labels. They tick at
/// minute granularity, so half a minute keeps the worst-case staleness
/// under the resolution anyone can see.
const AGO_REFRESH: Duration = Duration::from_secs(30);

/// How long the worktree selection must rest before asking the daemon to
/// pre-spawn that worktree's dead sessions — long enough that walking the
/// list doesn't boot every CLI passed, short enough that the sessions are
/// booting well before the user picks one.
const PREWARM_DEBOUNCE: Duration = Duration::from_millis(250);
/// How long a selection-driven attach waits for the cursor to settle. Long
/// enough that walking a list (or the Workspaces column, where every step is
/// a whole workspace switch) attaches only where the cursor stops; short
/// enough to feel immediate when it does stop.
const ATTACH_DEBOUNCE: Duration = Duration::from_millis(180);

/// How often the standing keep-warm request for the selected worktree's
/// default-spec Claude session is re-sent. Must stay comfortably under the
/// daemon's reap window minus its recycle threshold, so the warm slot is
/// refreshed (a young session is a no-op, an aging one is recycled) before
/// the reaper can empty it.
const KEEPWARM_REFRESH: Duration = Duration::from_secs(4 * 60);

/// How soon a worktree that came back without a pull request is asked
/// again, and how far that gap may grow. Switching into a worktree resets
/// it to the floor, so a PR an agent opens while the user watches lands on
/// the row within seconds; resting on a checkout that will never have one
/// backs off to a cadence that costs nothing. Each answer costs a `gh`
/// process and a network round trip, so only the selected worktree is
/// asked, and a worktree whose PR has been found is never asked again.
const PR_RECHECK_MIN: Duration = Duration::from_secs(10);
const PR_RECHECK_MAX: Duration = Duration::from_secs(3 * 60);
/// How often the selected worktree's *known* pull request is re-asked. The
/// PR won't change, but its conversation and state will — this is the beat
/// the row's unread-comment badge and its `merged` / `closed` badge run at,
/// for the one checkout the cursor is resting on. Same cadence and the same
/// budget reasoning as `OPEN_PRS_REFRESH` below.
const PR_REFRESH: Duration = Duration::from_secs(15);

/// How often the selected *project's* open-pull-request list is re-asked
/// once a repo has proved it has any, and how a repo that answers empty (or
/// can't answer at all) backs off. One `gh pr list` is one GraphQL call —
/// one point, however many pull requests come back — and only the selected
/// project is ever asked.
///
/// The budget (docs.github.com, "Rate limits and node limits for the
/// GraphQL API", checked 2026-08-28): 5,000 points an hour per user token,
/// with a secondary cap of 2,000 points a minute. At fifteen seconds this
/// list and the selected worktree's `PR_REFRESH` together spend 480 an hour
/// — under a tenth of the quota — and a focus-driven re-ask (see
/// `schedule_pull_request_refresh`) adds at most one call per
/// `OPEN_PRS_MIN_AGE`. The rest is left for the user's own `gh` and for the
/// Claude sessions sharing the same token, which is why this isn't faster
/// still.
///
/// This beat is the whole pruning mechanism: `--state open` stops returning
/// a pull request the moment it is merged or closed, so a row that should
/// no longer be there is gone within one refresh — which is why it is
/// seconds rather than the minutes a pure "what's open?" readout could
/// afford. Arriving at a project, or focusing a sidebar panel or the
/// terminal window, pulls the next lookup forward, floored by
/// `OPEN_PRS_MIN_AGE` so walking the project list can't spend a call per
/// row.
const OPEN_PRS_REFRESH: Duration = Duration::from_secs(15);
const OPEN_PRS_RECHECK_MIN: Duration = Duration::from_secs(30);
const OPEN_PRS_RECHECK_MAX: Duration = Duration::from_secs(10 * 60);

/// How long the Worktrees cursor must rest on an open-PR row before its
/// description and conversation are fetched. Long enough that arrowing
/// through a hundred rows spends nothing, short enough that stopping to
/// read one feels immediate. Answers are cached for the session, so this is
/// paid at most once per pull request.
const PR_DETAIL_DEBOUNCE: Duration = Duration::from_millis(300);

/// While the metrics modal is open, how often a fresh memory reading is
/// requested from the daemon.
const METRICS_POLL: Duration = Duration::from_secs(2);

/// With the modal closed, how often the footer's memory/session readout is
/// refreshed.
const FOOTER_METRICS_POLL: Duration = Duration::from_secs(5);

/// The one hotkey that isn't only a hotkey. Whatever the user binds to
/// [`crate::keymap::Action::UnlockTerminal`], Ctrl+q also unlocks a locked
/// pane — the alternative is a config that silently traps you inside a
/// session with the keyboard going to the child process.
const HARDWIRED_UNLOCK: crate::keymap::KeyChord = crate::keymap::KeyChord {
    code: KeyCode::Char('q'),
    mods: KeyModifiers::CONTROL,
};

/// Repaint cadence for the first-run splash animation — the only thing
/// that marks the app dirty while it idles on an empty tree.
const SPLASH_FRAME: Duration = crate::splash::FRAME;

/// Repaint cadence for the status-sweep text animation on running /
/// needs-feedback rows.
const SWEEP_FRAME: Duration = crate::app::SWEEP_FRAME;

/// `Some(entry)` = quit via the hosts picker: the caller should exec
/// `nebula ssh` at it now that the terminal is restored.
///
/// `workspace` is `nebula --workspace <name>`: the workspace to open this
/// instance into, whatever the last one opened elsewhere was. Resolved
/// against the first snapshot, since names are the daemon's to map.
pub async fn run_app(workspace: Option<String>) -> Result<Option<crate::hosts::HostEntry>> {
    let conn = ipc::connect_or_spawn().await?;
    let mut channels = ipc::split_connection(conn);
    channels.tx.send(ClientRequest::Subscribe).await?;

    let mut terminal = setup_terminal()?;
    let result = main_loop(&mut terminal, &mut channels, workspace).await;
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
        // Focus reports (mode 1004): coming back from the browser is the
        // moment a pull request was most likely just closed there.
        crossterm::event::EnableFocusChange,
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
        crossterm::event::DisableFocusChange,
        crossterm::event::DisableBracketedPaste,
        crossterm::event::DisableMouseCapture,
        LeaveAlternateScreen,
    );
    let _ = disable_raw_mode();
}

async fn main_loop(
    terminal: &mut Terminal<CrosstermBackend<BufWriter<Stdout>>>,
    channels: &mut ipc::IpcChannels,
    startup_workspace: Option<String>,
) -> Result<Option<crate::hosts::HostEntry>> {
    let mut app = App::new();
    app.conn = ConnState::Connected;
    app.startup_workspace = startup_workspace;
    let cfg = crate::config::Config::load();
    apply_config(&mut app, &cfg);
    app.keymap = cfg.keymap();
    // The Cursor MODEL / EFFORT lists: cached `cursor-agent --list-models`
    // now, a background refresh when the cache is a day old.
    crate::cursor_catalogue::bootstrap(cfg.cursor_enabled);
    let mut input = crossterm::event::EventStream::new();
    let mut out: Vec<ClientRequest> = Vec::new();
    // Pointer shape last sent to the terminal (OSC 22), so hover over a
    // splitter swaps the cursor once instead of on every motion event.
    let mut pointer_sent = PointerShape::default();
    let mut next_draw = tokio::time::Instant::now();
    let mut next_git_poll = tokio::time::Instant::now();
    // Pull-request lookups run off the loop (they hit the network); answers
    // come back here and land in `app.pull_requests`.
    let (pr_tx, mut pr_rx) =
        tokio::sync::mpsc::unbounded_channel::<(WorktreeId, Option<PullRequest>)>();
    // The selected project's open-pull-request list, on the same off-loop
    // footing. `None` is "couldn't ask", which keeps the last good list.
    let (prs_tx, mut prs_rx) = tokio::sync::mpsc::unbounded_channel::<(
        nebula_core::ProjectId,
        Option<Vec<crate::pull_request::OpenPr>>,
    )>();
    // One pull request's body and conversation, for the preview pane.
    let (detail_tx, mut detail_rx) =
        tokio::sync::mpsc::unbounded_channel::<(String, Option<crate::pull_request::PrDetail>)>();
    // A whole `gh pr diff`, which opens the diff modal when it lands.
    let (prdiff_tx, mut prdiff_rx) =
        tokio::sync::mpsc::unbounded_channel::<(u64, String, Option<String>)>();
    app.pr_diff_tx = Some(prdiff_tx);
    let mut next_metrics_poll = tokio::time::Instant::now();
    let mut next_splash_frame = tokio::time::Instant::now();
    let mut next_sweep_frame = tokio::time::Instant::now();
    let mut next_ago_refresh = tokio::time::Instant::now() + AGO_REFRESH;
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
        }

        let focus_before = app.focus;
        let preview_before = app.previewed_pr().map(|pr| pr.url);
        tokio::select! {
            // Pending redraw: wake at the frame boundary even if no new
            // events arrive.
            _ = tokio::time::sleep_until(next_draw), if app.dirty => {}
            // Fixed deadline (not a fresh sleep per iteration) so heavy PTY
            // traffic can't starve the badge refresh.
            _ = tokio::time::sleep_until(next_git_poll) => {
                refresh_git_changes(&mut app);
                // Rides the git tick rather than the repaint, so walking the
                // worktree list with j/k can't spawn a `gh` per row passed —
                // only whatever the selection is resting on when it fires.
                lookup_pull_request(&mut app, &pr_tx);
                lookup_open_prs(&mut app, &prs_tx, &mut out);
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
            // "23m ago" labels age on their own with nothing else to
            // repaint an idle app. Only worth a frame once some row carries
            // one — a project or worktree row reads any session under it,
            // so the whole tree is the test, not just the visible sessions.
            _ = tokio::time::sleep_until(next_ago_refresh) => {
                if app.tree.agents.iter().any(|a| a.status_changed_at > 0) {
                    app.dirty = true;
                }
                next_ago_refresh = tokio::time::Instant::now() + AGO_REFRESH;
            }
            // The selection rested past the debounce: tell the daemon what
            // the pane has been showing since the cursor landed here.
            _ = tokio::time::sleep(app.attach_delay().unwrap_or_default()),
                if app.pending_attach.is_some() =>
            {
                fire_pending_attach(&mut app, &mut out);
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
            answer = pr_rx.recv() => {
                // Never None: `pr_tx` lives as long as the loop.
                if let Some((worktree, pr)) = answer {
                    app.pr_inflight.remove(&worktree);
                    note_pr_answer(&mut app, &worktree, pr.is_some());
                    app.dirty |= app.pull_requests.insert(worktree, pr.clone()) != Some(pr);
                }
            }
            answer = prs_rx.recv() => {
                // Never None: `prs_tx` lives as long as the loop.
                if let Some((project, list)) = answer {
                    note_open_prs_answer(&mut app, project, list, &mut out);
                    refresh_palette(&mut app);
                }
            }
            // The hover debounce: the cursor has rested on a pull request
            // long enough to mean it.
            _ = tokio::time::sleep(app.pr_detail_delay().unwrap_or(Duration::MAX)),
                if app.pr_detail_delay().is_some() => {
                lookup_pr_detail(&mut app, &detail_tx);
            }
            answer = detail_rx.recv() => {
                if let Some((url, detail)) = answer {
                    app.pr_detail_inflight.remove(&url);
                    match detail {
                        Some(detail) => {
                            // GitHub's answer about this one pull request is
                            // the authoritative one: if it has been merged or
                            // closed since the list was fetched, the row goes
                            // now rather than at the next refresh.
                            let retired = !detail.is_open();
                            app.pr_detail.insert(url.clone(), detail);
                            if retired {
                                drop_retired_pr(&mut app, &url, &mut out);
                            }
                        }
                        None => { app.pr_detail_failed.insert(url); }
                    }
                    app.dirty = true;
                }
            }
            answer = prdiff_rx.recv() => {
                if let Some((number, title, diff)) = answer {
                    open_pr_diff_view(&mut app, number, title, diff);
                }
            }
        }
        if app.focus != focus_before {
            tracing::debug!(from = ?focus_before, to = ?app.focus, "focus changed");
            note_focus_change(&mut app);
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
        note_preview_change(&mut app, preview_before);

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

        // A copy that had to be delegated to the attached terminal (OSC 52 —
        // the only clipboard reachable from a headless `nebula ssh` host).
        // BEL-terminated on purpose: it is the form every OSC 52 implementer
        // accepts, ST is not.
        if let Some(payload) = app.pending_clipboard.take() {
            use std::io::Write;
            let backend = terminal.backend_mut();
            let _ = write!(backend, "\x1b]52;c;{payload}\x07");
            let _ = backend.flush();
        }

        // A turn reached FINISHED: ring the DONE SOUND. The bell goes out
        // through the same terminal as the OSC writes above, so over ssh it
        // rings the terminal the user is sitting at.
        if std::mem::take(&mut app.pending_ding) {
            play_done_sound(terminal.backend_mut());
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

/// Ask `gh` for the selected worktree's pull request, off the loop. Skipped
/// while one is in flight — a repaint must never stack `gh` processes — and
/// until the timer the last answer armed expires. The reply arrives on
/// `pr_tx`.
fn lookup_pull_request(
    app: &mut App,
    pr_tx: &tokio::sync::mpsc::UnboundedSender<(WorktreeId, Option<PullRequest>)>,
) {
    let Some((id, path)) = app
        .selected_worktree()
        .map(|w| (w.id.clone(), w.path.clone()))
    else {
        return;
    };
    if !app.pr_lookup_due(&id) {
        return;
    }
    // A checkout that isn't on disk (deleted outside nebula) has no branch
    // for gh to resolve; don't spend a process finding that out — but let
    // the backoff run, since a worktree can be restored underneath us.
    if !path.is_dir() {
        note_pr_answer(app, &id, false);
        app.dirty |= app.pull_requests.insert(id, None) != Some(None);
        return;
    }
    app.pr_inflight.insert(id.clone());
    let pr_tx = pr_tx.clone();
    tokio::spawn(async move {
        let pr = crate::pull_request::lookup(&path).await;
        let _ = pr_tx.send((id, pr));
    });
}

/// Record what a lookup came back with, and arm the next one. A found PR
/// settles onto the steady `PR_REFRESH` beat — it keeps being asked because
/// its comment count has to keep up with GitHub — while an empty answer
/// arms the next attempt one backoff step further out, so a checkout that
/// never grows a PR settles at `PR_RECHECK_MAX` instead of asking every few
/// seconds forever.
fn note_pr_answer(app: &mut App, worktree: &WorktreeId, found: bool) {
    let step = if found {
        PR_REFRESH
    } else {
        match app.pr_recheck.get(worktree) {
            Some((_, prev)) => (*prev * 2).min(PR_RECHECK_MAX),
            None => PR_RECHECK_MIN,
        }
    };
    app.pr_recheck
        .insert(worktree.clone(), (std::time::Instant::now() + step, step));
}

/// Ask `gh` for every pull request open on the selected project's repo, off
/// the loop. Only the selected project is ever asked — the group only shows
/// for the project on screen, and a workspace of thirty repos must not cost
/// thirty API calls a beat. Skipped while one is in flight and until the
/// timer the last answer armed. The reply arrives on `prs_tx`.
fn lookup_open_prs(
    app: &mut App,
    prs_tx: &tokio::sync::mpsc::UnboundedSender<(
        nebula_core::ProjectId,
        Option<Vec<crate::pull_request::OpenPr>>,
    )>,
    out: &mut Vec<ClientRequest>,
) {
    let Some((id, path)) = app
        .selected_project()
        .map(|p| (p.id.clone(), p.repo_path.clone()))
    else {
        return;
    };
    if !app.open_prs_lookup_due(&id) {
        return;
    }
    // A repo that isn't on disk has nothing for `gh` to resolve against;
    // don't spend a process finding that out, but let the backoff run — the
    // checkout can come back (an unmounted volume, a restored directory).
    if !path.is_dir() {
        note_open_prs_answer(app, id, None, out);
        return;
    }
    app.open_prs_inflight.insert(id.clone());
    let prs_tx = prs_tx.clone();
    tokio::spawn(async move {
        let list = crate::pull_request::list(&path).await;
        let _ = prs_tx.send((id, list));
    });
}

/// Record what a list lookup came back with, and arm the next one. A repo
/// with pull requests open settles onto the steady `OPEN_PRS_REFRESH` beat;
/// an empty answer — or one `gh` couldn't give at all — arms the next
/// attempt a backoff step further out, so a repo with no PRs (or a machine
/// with no `gh`) settles at `OPEN_PRS_RECHECK_MAX` instead of asking all
/// day. A failed call keeps whatever list was already on screen: one flaky
/// network round trip is no reason to blank the group.
fn note_open_prs_answer(
    app: &mut App,
    project: nebula_core::ProjectId,
    list: Option<Vec<crate::pull_request::OpenPr>>,
    out: &mut Vec<ClientRequest>,
) {
    // Which pull request the cursor is resting on, before the list under it
    // changes. A refresh that retires a merged PR must not slide the
    // selection onto whatever row inherits its index.
    let cursor = app.selected_worktree_pr().cloned();
    app.open_prs_inflight.remove(&project);
    let previous = app.open_prs.get(&project);
    let found = list.as_ref().is_some_and(|l| !l.is_empty());
    let step = if found {
        OPEN_PRS_REFRESH
    } else {
        match previous {
            Some(open) => (open.step * 2).min(OPEN_PRS_RECHECK_MAX),
            None => OPEN_PRS_RECHECK_MIN,
        }
    };
    let now = std::time::Instant::now();
    let list = list.unwrap_or_else(|| previous.map(|o| o.list.clone()).unwrap_or_default());
    app.dirty |= previous.map(|o| &o.list) != Some(&list);
    app.open_prs.insert(
        project,
        crate::app::OpenPrs {
            list,
            at: now,
            due: now + step,
            step,
        },
    );
    forget_retired_prs(app);
    reconcile_open_pr_cursor(app, cursor, out);
}

/// Follow the Worktrees cursor across a change to the open-pull-request
/// list. Three cases, and the row count moved under all of them:
///
/// * the cursor wasn't on a pull request — nothing to do;
/// * its pull request is still open — keep the cursor on *it*, wherever the
///   new list put it, rather than on whatever now holds its old index;
/// * its pull request has been merged or closed — the row is gone, so the
///   cursor lands on the nearest surviving one, and if that is a checkout
///   the pane has to be given that checkout's session (the same
///   `restore_session` an arrow key would have run). Without it the PTY
///   underneath — deliberately left attached while the cursor is in the PR
///   group — would keep showing a session that belongs to a different
///   worktree.
fn reconcile_open_pr_cursor(
    app: &mut App,
    was: Option<crate::pull_request::OpenPr>,
    out: &mut Vec<ClientRequest>,
) {
    let Some(was) = was else {
        return;
    };
    let checkouts = app.visible_worktrees().len();
    match app
        .visible_open_prs()
        .iter()
        .position(|pr| pr.url == was.url)
    {
        // Same pull request, possibly at a new index. Nothing is re-armed:
        // `schedule_pr_detail` zeroes `pr_preview_scroll`, and a refresh
        // landing every minute must not yank a reader back to the top of a
        // conversation they're halfway down.
        Some(i) => app.sel_worktree = checkouts + i,
        None => {
            let rows = app.worktree_row_count();
            if app.sel_worktree >= rows {
                app.sel_worktree = rows.saturating_sub(1);
            }
            if app.selected_worktree().is_some() {
                restore_session(app, out);
            }
            // The pane is showing something else now: rewind it and fetch
            // whatever the cursor landed on. Say why, too — a row that
            // evaporates mid-read is otherwise just the cursor jumping.
            schedule_pr_detail(app);
            app.flash = Some(format!("#{} is no longer open", was.number));
            app.dirty = true;
        }
    }
}

/// Forget the description and conversation of every pull request that is no
/// longer open anywhere. `pr_detail` is a session cache — deliberately, a
/// PR's body doesn't change while you read it — so without this a workspace
/// left running for a week accumulates the full text of every pull request
/// that has since been merged, and `pr_detail_failed` keeps refusing to
/// re-ask about numbers that have long stopped being on screen.
fn forget_retired_prs(app: &mut App) {
    let live: std::collections::HashSet<String> = app
        .open_prs
        .values()
        .flat_map(|o| o.list.iter().map(|pr| pr.url.clone()))
        .collect();
    app.pr_detail.retain(|url, _| live.contains(url));
    app.pr_detail_failed.retain(|url| live.contains(url));
}

/// Retire one pull request from every project's list ahead of the next
/// `gh pr list`, because GitHub has just told us — in the detail fetched
/// for the row the cursor is resting on — that it is merged or closed.
/// The list refresh would catch it within the minute anyway; this is for
/// the case where the user is looking straight at it.
fn drop_retired_pr(app: &mut App, url: &str, out: &mut Vec<ClientRequest>) {
    let cursor = app.selected_worktree_pr().cloned();
    let mut removed = false;
    for open in app.open_prs.values_mut() {
        let before = open.list.len();
        open.list.retain(|pr| pr.url != url);
        removed |= open.list.len() != before;
    }
    if !removed {
        return;
    }
    reconcile_open_pr_cursor(app, cursor, out);
    refresh_palette(app);
    app.dirty = true;
}

/// Arm (or disarm) the debounced fetch of the pull request the pane is
/// reading (`App::previewed_pr`) — the Worktrees cursor's open-PR row or the
/// Sessions cursor's PR ROW. Called wherever the Worktrees cursor moves, and
/// from `note_preview_change` whenever the previewed URL changes for any
/// other reason. A PR already fetched, already in flight, or already known
/// to be unanswerable arms nothing — the pane has something to show either
/// way, and re-asking would spend an API call on a row the user is only
/// passing through.
fn schedule_pr_detail(app: &mut App) {
    let pending = app.previewed_pr().and_then(|pr| {
        let url = pr.url;
        if app.pr_detail.contains_key(&url)
            || app.pr_detail_inflight.contains(&url)
            || app.pr_detail_failed.contains(&url)
        {
            return None;
        }
        // Either row lives in the selected project's repo; `gh pr view`
        // resolves the number from any checkout of it.
        let dir = app.selected_project().map(|p| p.repo_path.clone())?;
        Some(crate::app::PendingPrDetail {
            url,
            number: pr.number,
            dir,
        })
    });
    // Landing on a different row resets the scroll: the pane is showing
    // something else now.
    app.pr_preview_scroll = 0;
    app.pending_pr_detail = pending.map(|p| (p, std::time::Instant::now() + PR_DETAIL_DEBOUNCE));
}

/// The one place "the pane is reading something else now" is noticed: the
/// loop takes `previewed_pr()`'s URL before handling an event and hands it
/// back here after. A different URL — the Sessions cursor stepped onto or
/// off the PR ROW, focus left the Sessions panel for the pane, a refresh
/// retired the row — re-arms the detail fetch and rewinds the scroll; the
/// same URL leaves a reader exactly where they were. Keyed on URL, not the
/// whole row, so a re-titled PR arriving on the GIT POLL is not a change.
fn note_preview_change(app: &mut App, before: Option<String>) {
    if app.previewed_pr().map(|pr| pr.url) != before {
        schedule_pr_detail(app);
    }
}

/// Fire the debounced fetch. Disarms first, so a `gh` that never answers
/// can't re-fire on every loop turn.
fn lookup_pr_detail(
    app: &mut App,
    detail_tx: &tokio::sync::mpsc::UnboundedSender<(String, Option<crate::pull_request::PrDetail>)>,
) {
    let Some((pending, _)) = app.pending_pr_detail.take() else {
        return;
    };
    if !pending.dir.is_dir() {
        app.pr_detail_failed.insert(pending.url);
        app.dirty = true;
        return;
    }
    app.pr_detail_inflight.insert(pending.url.clone());
    let detail_tx = detail_tx.clone();
    tokio::spawn(async move {
        let detail = crate::pull_request::detail(&pending.dir, pending.number).await;
        let _ = detail_tx.send((pending.url, detail));
    });
}

/// `g` on an open-PR row: fetch the whole pull request diff off the loop and
/// open the ordinary diff modal on it when it lands. One `gh pr diff` gets
/// every file at once, which is why this view carries its diffs with it
/// instead of shelling out per file the way the worktree view does.
fn request_pr_diff(app: &mut App) {
    let Some((number, title)) = app.previewed_pr().map(|pr| (pr.number, pr.label)) else {
        return;
    };
    if app.pr_diff_inflight == Some(number) {
        app.flash = Some(format!("still fetching the diff for #{number}…"));
        return;
    }
    let Some(dir) = app.selected_project().map(|p| p.repo_path.clone()) else {
        return;
    };
    if !dir.is_dir() {
        app.flash = Some(format!("repo path missing on disk: {}", dir.display()));
        return;
    }
    let Some(prdiff_tx) = app.pr_diff_tx.clone() else {
        return; // never: the loop installs it at startup
    };
    app.pr_diff_inflight = Some(number);
    app.flash = Some(format!("fetching the diff for #{number}…"));
    app.dirty = true;
    tokio::spawn(async move {
        let diff = crate::pull_request::diff(&dir, number).await;
        let _ = prdiff_tx.send((number, title, diff));
    });
}

/// Land a fetched pull-request diff in the diff modal. The files come from
/// splitting the unified diff rather than from `git status`, and every
/// entry is marked `M` — a pull request's own diff already renders the
/// add/delete headers, and porcelain codes would be an invention.
fn open_pr_diff_view(app: &mut App, number: u64, title: String, diff: Option<String>) {
    if app.pr_diff_inflight == Some(number) {
        app.pr_diff_inflight = None;
    }
    let Some(diff) = diff else {
        app.flash = Some(format!(
            "couldn't read the diff for #{number} — is `gh` set up?"
        ));
        return;
    };
    let chunks = crate::pull_request::split_unified_diff(&diff);
    if chunks.is_empty() {
        app.flash = Some(format!("#{number} changes no files"));
        return;
    }
    let files = chunks
        .iter()
        .map(|(path, _)| crate::git_diff::DiffFile {
            path: path.clone(),
            orig_path: None,
            xy: ['M', ' '],
        })
        .collect();
    // `root` is only ever used to shell out at git, which a prefetched view
    // never does — but the reviewed-mark store keys on it, so it stays the
    // repo path rather than something invented.
    let root = app
        .selected_project()
        .map(|p| p.repo_path.clone())
        .unwrap_or_default();
    let mut view = DiffView::new(root, title, files, true);
    view.prefetched = Some(chunks.into_iter().collect());
    view.files_width = app.diff_files_width;
    crate::git_diff::load_selected_diff(&mut view);
    app.overlay = Some(Overlay::Diff(view));
    app.flash = None;
    app.dirty = true;
}

/// Arm the selected project for a prompt open-pull-request lookup: arriving
/// at a project is exactly when the user wants to know what's still open on
/// it. Floored at `OPEN_PRS_MIN_AGE` past the last answer, so bouncing
/// between two projects re-reads the cache instead of spending an API call
/// per switch.
fn schedule_open_prs_lookup(app: &mut App) {
    let Some(id) = app.selected_project().map(|p| p.id.clone()) else {
        return;
    };
    if let Some(open) = app.open_prs.get_mut(&id) {
        open.due = open.due.min(open.at + crate::app::OPEN_PRS_MIN_AGE);
    }
}

/// Arm the selected worktree for a prompt pull-request lookup: switching
/// into a checkout is exactly when the user wants to see the PR a session
/// opened there — and, once it's known, whether anyone has commented since
/// — so drop whatever timer had accumulated and ask on the next tick.
fn schedule_pr_lookup(app: &mut App) {
    if let Some(id) = app.selected_worktree().map(|w| w.id.clone()) {
        app.pr_recheck.remove(&id);
    }
}

/// Pull both pull-request lookups forward — the project's open list and
/// the selected worktree's own PR — so the next `GIT_POLL` tick asks GitHub
/// again. Run on the gestures that mean "I want fresh data now": the
/// terminal window regaining focus and the cursor entering a sidebar panel.
/// The list keeps its `OPEN_PRS_MIN_AGE` floor, so a flurry of focus events
/// costs one call, not one per event. Deliberately does *not* touch the PR
/// preview: `schedule_pr_detail` resets its scroll, and a timer-shaped
/// caller must never yank a reader back to the top.
fn schedule_pull_request_refresh(app: &mut App) {
    schedule_open_prs_lookup(app);
    schedule_pr_lookup(app);
}

/// The panel focus just moved. Landing on the Worktrees or Sessions panel —
/// the two that show pull requests — is a reason to re-ask GitHub; walking
/// off them into the pane is not.
fn note_focus_change(app: &mut App) {
    if matches!(app.focus, Focus::Worktrees | Focus::Sessions) {
        schedule_pull_request_refresh(app);
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
    send(app, out, |req_id| ClientRequest::GetMetrics { req_id });
}

/// Queue a request that wants no follow-up when its Ack lands.
fn send(app: &mut App, out: &mut Vec<ClientRequest>, make: impl FnOnce(u64) -> ClientRequest) {
    send_with(app, out, PendingIntent::None, make);
}

/// Queue a request built around a fresh `req_id`, remembering `intent` to
/// run when the Ack (or Error) for it arrives.
fn send_with(
    app: &mut App,
    out: &mut Vec<ClientRequest>,
    intent: PendingIntent,
    make: impl FnOnce(u64) -> ClientRequest,
) {
    let req_id = app.alloc_req_id(intent);
    out.push(make(req_id));
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

/// Land `nebula --workspace <name>` on the first snapshot: names only mean
/// anything once the workspace list is here. An unknown name flashes and
/// leaves the instance on the daemon's default rather than booting into
/// nothing. Applies once — a later snapshot is not a fresh launch.
fn apply_startup_workspace(app: &mut App, out: &mut Vec<ClientRequest>) {
    let Some(name) = app.startup_workspace.take() else {
        return;
    };
    match app
        .tree
        .workspaces
        .iter()
        .find(|w| w.name == name)
        .map(|w| w.id.clone())
    {
        Some(id) => {
            // Not switch_workspace: there is no context to remember yet, and
            // the tree it would restore into is the one being installed.
            // Telling the daemon still matters — it scopes AddProject.
            if app.tree.active_workspace != id {
                app.tree.active_workspace = id.clone();
                send(app, out, |req_id| ClientRequest::OpenWorkspace {
                    req_id,
                    id,
                });
            }
        }
        None => {
            let names: Vec<&str> = app
                .tree
                .workspaces
                .iter()
                .map(|w| w.name.as_str())
                .collect();
            app.flash = Some(format!(
                "no workspace named '{name}' (have: {})",
                names.join(", ")
            ));
        }
    }
}

/// Re-seat the cursor from the persisted blob. Returns whether the
/// remembered session landed under it, so the caller can bring its pane
/// back too — the selection alone is a cursor on a blank screen.
fn restore_ui_state(app: &mut App, json: &str) -> bool {
    use crate::app::UiState;
    let Ok(state) = serde_json::from_str::<UiState>(json) else {
        return false;
    };
    app.show_archived = state.show_archived;
    if let Some(w) = state.panel_widths {
        // normalize_panel_widths re-fits to the actual screen on the next
        // draw.
        app.panel_widths = w.map(|v| v.clamp(crate::app::MIN_PANEL_W, MAX_RESTORED_WIDTH));
    }
    if let Some(w) = state.diff_files_width {
        // The draw re-caps it to the actual modal width.
        app.diff_files_width = w.clamp(crate::app::MIN_DIFF_FILES_W, MAX_RESTORED_WIDTH);
    }
    if let Some(pid) = &state.project {
        let row = app
            .project_rows()
            .iter()
            .position(|i| app.tree.projects[*i].id.as_str() == pid);
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
    let mut session_landed = false;
    if let Some(sid) = state.session_agent {
        if let Some(i) = app
            .visible_session_rows()
            .iter()
            .position(|r| matches!(r, SessionRow::Agent(a) if a.id.as_str() == sid))
        {
            app.sel_session = i;
            session_landed = true;
        }
    }
    session_landed
}

/// Keep the vt100 parser and the daemon PTY sized to the drawn pane.
fn sync_pty_size(app: &mut App, out: &mut Vec<ClientRequest>) {
    let area = app.term_area;
    if !pane_usable(area) {
        return;
    }
    if let Some(term) = &mut app.term {
        if (term.cols, term.rows) != (area.width, area.height) {
            // The grid reflows; a screen-anchored selection would drift.
            app.term_selection = None;
            term.cols = area.width;
            term.rows = area.height;
            term.parser.screen_mut().set_size(area.height, area.width);
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
        if pane_usable(vim.area) {
            vim.resize(vim.area.width, vim.area.height);
        }
    }
}

/// Whether a rect has been drawn large enough to size a grid to.
fn pane_usable(area: ratatui::layout::Rect) -> bool {
    area.width >= MIN_PANE_DIM && area.height >= MIN_PANE_DIM
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
    // With the pane holding input, whatever this event turns into is headed
    // for the PTY — and the daemon drops Input for a session it hasn't
    // spawned. A still-debounced attach has to land before the keystroke.
    if app.term_locked {
        fire_pending_attach(app, out);
    }
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
                vim.input(&bracketed(&text));
            }
        }
        // An overlay with a live text field takes the paste: ⌘V into a
        // filter or the ssh destination lands where the caret is.
        Event::Paste(text) if paste_into_overlay(app, &text) => {}
        Event::Paste(text) => {
            if app.focus == Focus::Terminal && app.term_locked {
                if let Some(term) = &app.term {
                    // Bracketed paste so the child (claude, vim…) knows.
                    out.push(ClientRequest::Input {
                        session: term.sref.clone(),
                        data: bracketed(&text),
                    });
                }
            }
        }
        Event::Resize(_, _) => app.dirty = true,
        // The terminal window took focus again — most often back from a
        // browser tab where a pull request was just merged or closed.
        Event::FocusGained => schedule_pull_request_refresh(app),
        _ => {}
    }
}

/// `text` wrapped in the bracketed-paste markers, ready for a PTY.
fn bracketed(text: &str) -> Vec<u8> {
    let mut data = PASTE_START.to_vec();
    data.extend_from_slice(text.as_bytes());
    data.extend_from_slice(PASTE_END);
    data
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
            if prompt.is_multiline() {
                prompt.input.insert_multiline_str(text);
            } else {
                prompt.input.insert_str(text);
            }
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
        // Only types while its add/edit input is open.
        Overlay::Hosts(view) => match &mut view.input {
            Some(input) => input.insert_str(text),
            None => return false,
        },
        // The name is one line; prefix and postfix keep their newlines.
        Overlay::AgentPresetEditor(editor) => {
            let multiline = matches!(
                editor.field,
                crate::preset_overlays::PresetField::Prefix
                    | crate::preset_overlays::PresetField::Postfix
            );
            match editor.text_field_mut() {
                Some(input) if multiline => input.insert_multiline_str(text),
                Some(input) => input.insert_str(text),
                None => return false,
            }
        }
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
    // the escape hatches. Enter and the forward panel walk (Tab / ^⇧L, or
    // l/→ double-tapped at Sessions) lock; Ctrl+→ is the way to stand in
    // the pane without locking, and an unlocked pane falls through to
    // panel navigation, so the user always has a way back that isn't a
    // hatch.
    if app.focus == Focus::Terminal && app.term.is_some() && app.term_locked {
        // Ctrl+q is the primary hatch: a plain control byte (0x11) that
        // every emulator delivers — Terminal.app included, no kitty protocol
        // needed — unbound in macOS and unused by Claude Code. The inner
        // session loses XON (unfreeze after an accidental Ctrl+S), which
        // nobody will miss.
        // Fallback hatches: Ctrl+Shift+H (the walk-back-a-panel key, which
        // doubles as the way out of a locked pane — needs the kitty
        // protocol, so Ghostty/kitty only, never Terminal.app), Ctrl+]
        // (telnet's escape char — byte 0x1D, which crossterm spells Ctrl+5
        // in legacy mode), Ctrl+Esc (kitty-only), and Ctrl+← (stolen by
        // Mission Control on stock macOS).
        // All five are rebindable in Settings → Hotkeys, but Ctrl+q stays
        // wired in on top of whatever is bound: unbinding your way out of
        // a locked session would trap you in it with no way back.
        let chord = crate::keymap::KeyChord::from_event(&key);
        let is_hatch = chord == HARDWIRED_UNLOCK
            || app.keymap.lookup(crate::keymap::Scope::Terminal, &chord)
                == Some(crate::keymap::Action::UnlockTerminal);
        if is_hatch {
            leave_terminal_lock(app);
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
            leave_terminal_lock(app);
            return;
        }
    }

    // Splash preview up: the next key just dismisses it, back to the
    // panels — even q, which quits on the press after.
    if app.splash_preview {
        app.splash_preview = false;
        return;
    }

    // Reading a pull request in the pane: the diff modal's scroll keys work
    // here too. Page/Home/End only — shift+↑/↓ already move a project, and
    // ↑/↓ have to keep walking the PR list itself. From either list that
    // can rest on one; a focused pane keeps its keys for the PTY.
    if app.previewed_pr().is_some() && matches!(app.focus, Focus::Worktrees | Focus::Sessions) {
        let page = app.term_area.height.max(1);
        let max = app.pr_preview_max_scroll();
        let scrolled = match key.code {
            KeyCode::PageDown => Some(app.pr_preview_scroll.saturating_add(page).min(max)),
            KeyCode::PageUp => Some(app.pr_preview_scroll.saturating_sub(page)),
            KeyCode::Home => Some(0),
            KeyCode::End => Some(max),
            _ => None,
        };
        if let Some(to) = scrolled {
            app.dirty |= app.pr_preview_scroll != to;
            app.pr_preview_scroll = to;
            return;
        }
    }

    // Panel focus: every key here is a rebindable action (see keymap.rs),
    // so the dispatch is a table lookup rather than a KeyCode match — an
    // unbound press simply falls through.
    let chord = crate::keymap::KeyChord::from_event(&key);
    // A double tap is two of the same key in a row: whatever else arrives
    // in between — bound or not — breaks it, so the arm is taken here and
    // only the edge arms below put one back.
    let armed = app.edge_tap.take();
    let Some(action) = app.keymap.lookup(crate::keymap::Scope::Global, &chord) else {
        return;
    };
    use crate::keymap::Action;
    match action {
        Action::Quit => app.should_quit = true,
        Action::Help => app.overlay = Some(Overlay::Help(HelpView::default())),
        Action::Settings => open_settings(app),
        // Request a reading right away — the main loop's poll may be up to
        // FOOTER_METRICS_POLL out.
        Action::Metrics => {
            app.overlay = Some(Overlay::Metrics(MetricsView::new()));
            request_metrics(app, out);
        }
        // Replay the first-run nebula splash, fade-in included.
        Action::Splash => {
            app.splash_epoch = std::time::Instant::now();
            app.splash_preview = true;
            app.collapsed = false;
        }
        // Tab / ^⇧L walk forward and stop dead at the terminal pane —
        // leaning on the key can't spill past the pane and back round to
        // the Workspaces bar. Landing on the pane takes the input lock:
        // walking that far means the user is going to type at the agent,
        // and the preview under the Sessions cursor is already the session
        // they picked.
        Action::FocusNext => walk_focus_forward(app, out),
        // ⇧Tab / ^⇧H walk back and stop dead at the first visible stop —
        // the Workspaces bar while shown, otherwise the first sidebar.
        // Neither wraps into the pane: ^⇧H is also the unlock hatch out of
        // a locked pane, so a wrap made the key cycle first column → pane
        // → Sessions → … → first column forever, with nothing to stop
        // against. Forward (Tab / ^⇧L) is the way into the pane, and
        // Ctrl+→ crosses into it without taking the input lock.
        Action::FocusPrev => walk_focus_back(app),
        // Inside the Workspaces bar, ←/→ walk the tabs rather than the
        // panels: the bar spans the top, so there is nothing horizontally
        // beside it to move to. j/↓ and Enter are the way out (below).
        Action::FocusLeft if app.focus == Focus::Workspaces => move_selection(app, -1, out),
        Action::FocusRight if app.focus == Focus::Workspaces => move_selection(app, 1, out),
        // h/← and l/→ are the vim twins of the ⇧Tab/Tab walk: one panel
        // at a time, stopping at the ends of the row. A single press at an
        // end stays put — leaning on the key can't spill over — and a
        // double tap jumps the boundary the way ^⇧H / ^⇧L would: h,h at the
        // leftmost column steps up into the Workspaces bar (only while it's
        // shown; hidden, there is nothing above to jump to), l,l at Sessions
        // crosses into the pane and takes its input.
        Action::FocusLeft => match app.focus {
            focus if focus == app.first_sidebar_focus() => {
                if app.show_workspaces && double_tapped(app, action, armed, &chord, "workspaces") {
                    walk_focus_back(app);
                }
            }
            _ => walk_focus_back(app),
        },
        // ⌘N / N: open the Nth tab in the Workspaces bar from any panel.
        // Focus stays where it is — the switch re-scopes the panels under
        // the cursor, and yanking focus up to the bar would undo that.
        Action::SelectWorkspace(n) => {
            match app
                .tree
                .workspaces
                .get(n as usize - 1)
                .map(|w| w.id.clone())
            {
                Some(id) => {
                    switch_workspace(app, id, out);
                }
                None => app.flash = Some(format!("no workspace {n}")),
            }
        }
        Action::Hosts => open_hosts_picker(app),
        Action::AgentPresets => crate::preset_overlays::open_agent_presets(app),
        // Ctrl+→ still reaches the terminal pane (the counterpart of the
        // Ctrl+← escape hatch).
        Action::FocusTerminal => {
            app.focus = app.next_visible_focus(app.focus);
        }
        Action::FocusRight => match app.focus {
            Focus::Sessions => {
                if double_tapped(app, action, armed, &chord, "enter pane") {
                    walk_focus_forward(app, out);
                }
            }
            // Standing in the pane unlocked (Ctrl+→): l,l takes the lock,
            // as ^⇧L does. A dead or empty pane has nothing to lock into.
            Focus::Terminal => {
                let live = app.term.as_ref().is_some_and(|t| !t.exited);
                if live && double_tapped(app, action, armed, &chord, "type into terminal") {
                    walk_focus_forward(app, out);
                }
            }
            _ => walk_focus_forward(app, out),
        },
        // Show/hide the Workspaces bar. Hiding it moves a cursor parked
        // there onto the first visible sidebar.
        Action::ToggleWorkspaces => {
            set_show_workspaces(app, !app.show_workspaces);
            // The hotkey and the settings row edit the same value, so the
            // choice survives a restart however it was made.
            let mut cfg = crate::config::Config::load();
            cfg.show_workspaces = app.show_workspaces;
            save_config(app, &cfg);
        }
        Action::ToggleProjects => {
            set_hide_projects(app, !app.hide_projects);
            save_panel_visibility(app);
        }
        Action::ToggleWorktrees => {
            set_hide_worktrees(app, !app.hide_worktrees);
            save_panel_visibility(app);
        }
        // The Workspaces bar is a horizontal strip: ↑ has nowhere left to
        // go and ←/→ walk the tabs. j/↓ is the way back down, and like h/l
        // at the ends of the row it is a double tap — one press stays put
        // and says so, j,j lands on the panel the cursor came up from (Enter
        // still steps into the first visible sidebar).
        Action::MoveDown if app.focus == Focus::Workspaces => {
            let does = format!("back to {}", panel_name(bar_return_target(app)));
            if double_tapped(app, action, armed, &chord, &does) {
                leave_workspaces_bar(app);
            }
        }
        Action::MoveUp if app.focus == Focus::Workspaces => {}
        Action::MoveDown => move_selection(app, 1, out),
        // A panel's first row is the top edge: k,k there steps up into the
        // Workspaces bar the way h,h at the leftmost column does (only
        // while it's shown — hidden, a single k stays a plain no-op).
        Action::MoveUp if app.show_workspaces && at_top_row(app) => {
            if double_tapped(app, action, armed, &chord, "workspaces") {
                enter_workspaces_bar(app);
            }
        }
        Action::MoveUp => move_selection(app, -1, out),
        Action::Activate => match app.focus {
            // The cursor already IS the open workspace; Enter steps into it.
            Focus::Workspaces => app.focus = app.first_sidebar_focus(),
            Focus::Projects => app.focus = app.next_visible_focus(Focus::Projects),
            // An open-PR row leads out of nebula, so Enter hands it to the
            // browser and stays put; a checkout hands focus one column right.
            Focus::Worktrees => match app.selected_worktree_pr().map(|pr| pr.url.clone()) {
                Some(url) => open_link(app, &url, out),
                None => app.focus = Focus::Sessions,
            },
            Focus::Sessions => attach_selected(app, out),
            // Lock input into an already-focused live pane.
            Focus::Terminal => enter_terminal_pane(app, out),
        },
        // First run (or an empty workspace): with no visible projects every
        // panel is empty and the splash is up — New creates a project no
        // matter which panel has focus.
        Action::New if !app.tree.has_visible_projects() => open_prompt(app, PromptKind::AddProject),
        Action::New => match app.focus {
            Focus::Workspaces => open_prompt(app, PromptKind::NewWorkspace),
            Focus::Projects => open_prompt(app, PromptKind::AddProject),
            Focus::Worktrees => {
                if app.selected_worktree_pr().is_some() {
                    open_pr_agent_picker(app);
                } else if let Some(p) = app.selected_project() {
                    let project = p.id.clone();
                    open_new_worktree_prompt(app, project);
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
        Action::Rename => match app.focus {
            Focus::Sessions => match app.selected_session_row() {
                Some(SessionRow::Agent(a)) => {
                    open_prompt(app, PromptKind::RenameAgent { id: a.id })
                }
                Some(SessionRow::Terminal(t)) => {
                    open_prompt(app, PromptKind::RenameTerminal { id: t.id })
                }
                Some(SessionRow::Link(l)) => edit_link(app, &l),
                None => {}
            },
            Focus::Projects => {
                if let Some(p) = app.selected_project() {
                    let id = p.id.clone();
                    open_prompt(app, PromptKind::RenameProject { id });
                }
            }
            Focus::Workspaces => {
                let id = app.tree.active_workspace.clone();
                open_prompt(app, PromptKind::RenameWorkspace { id });
            }
            _ => {}
        },
        Action::Archive => {
            if app.focus == Focus::Sessions {
                match app.selected_session_row() {
                    Some(SessionRow::Agent(a)) if !a.archived => {
                        archive_agent(app, a.id, out);
                    }
                    Some(SessionRow::Terminal(_)) => {
                        app.flash = Some("terminals can't be archived — d closes them".into());
                    }
                    Some(SessionRow::Link(_)) => {
                        app.flash = Some("links can't be archived — d deletes them".into());
                    }
                    _ => {}
                }
            }
        }
        Action::Unarchive => {
            if app.focus == Focus::Sessions {
                if let Some(a) = app.selected_session() {
                    if a.archived {
                        send(app, out, |req_id| ClientRequest::UnarchiveAgent {
                            req_id,
                            id: a.id,
                        });
                    }
                }
            }
        }
        Action::ToggleArchived => {
            if app.focus == Focus::Sessions {
                toggle_archived(app, out);
            }
        }
        // Workspace switcher: pick which workspace is open. The focus
        // guard keeps it out of an unlocked terminal pane — but under the
        // splash there is no pane on screen, so it always opens there.
        Action::Workspaces => {
            if app.focus != Focus::Terminal || app.splash_showing() {
                open_workspace_picker(app);
            }
        }
        // Fuzzy-search palette over every project / worktree / session.
        // The config read is per-open so edits apply without restarting.
        Action::Palette => {
            if app.focus != Focus::Terminal {
                app.overlay = Some(Overlay::Palette(Palette::new(
                    &app.tree,
                    app.show_archived,
                    crate::config::Config::load().palette_enter_attaches,
                    &app.open_prs,
                )));
            }
        }
        Action::Delete => open_delete_confirm(app),
        // Delete EVERY row of the focused panel (behind a confirm that
        // lists the casualties).
        Action::DeleteAll => open_delete_all_confirm(app),
        Action::ContextMenu => open_context_menu_for_selection(app),
        // On an open-PR row (either list) `g` reads that pull request's
        // diff off GitHub instead of the checkout's — same modal, different
        // source.
        Action::GitDiff if app.previewed_pr().is_some() => request_pr_diff(app),
        Action::GitDiff => open_diff_view(app),
        Action::OpenRepo => open_repo_in_browser(app),
        // AddProject adds a project from ANY panel — unlike New it never
        // changes meaning with focus, matching the "open a repo" instinct.
        Action::AddProject => open_prompt(app, PromptKind::AddProject),
        Action::FindFile => open_file_finder(app),
        Action::Grep => open_grep_view(app),
        Action::TreeBrowser => open_tree_browser(app),
        // New shell terminal, spawned in the worktree's directory.
        // (Cmd+T never reaches a TUI — the emulator opens its own tab.)
        Action::NewTerminal => create_terminal_for_context(app, out),
        Action::Zoom => {
            if app.term.is_some() {
                app.collapsed = true;
                app.focus = Focus::Terminal;
                app.term_locked = true;
                fire_pending_attach(app, out);
            } else {
                app.flash = Some("attach a session first".into());
            }
        }
        // Terminal-scope only; never resolved here.
        Action::UnlockTerminal => {}
    }
}

// ---- overlays ----

/// New-worktree prompt with a random branch name already picked out.
/// The project's existing branches are excluded, so Enter on an empty
/// input can't land on a name `git worktree add` would reject.
fn open_new_worktree_prompt(app: &mut App, project: nebula_core::ProjectId) {
    let taken: Vec<String> = app
        .tree
        .worktrees
        .iter()
        .filter(|w| w.project_id == project)
        .map(|w| w.branch.clone())
        .collect();
    let suggestion = crate::branch_name::random_name(&taken);
    open_prompt(
        app,
        PromptKind::NewWorktree {
            project,
            suggestion,
        },
    );
}

pub(crate) fn open_prompt(app: &mut App, kind: PromptKind) {
    use std::borrow::Cow;
    let (title, label, input): (Cow<'static, str>, Cow<'static, str>, String) = match &kind {
        // Starts at "~/" with the home listing already showing, so the
        // browser is one ↓ away; typing a leading '/' or '~' replaces the
        // prefill (see the Char arm), and Ctrl+u clears it.
        PromptKind::AddProject => (
            "Add project".into(),
            "path to a git repository".into(),
            if nebula_core::env::home_dir().is_some() {
                "~/".to_string()
            } else {
                String::new()
            },
        ),
        PromptKind::NewWorktree { suggestion, .. } => (
            "New worktree".into(),
            format!("branch name (empty = {suggestion})").into(),
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
                "New agent".into()
            } else {
                format!("New agent ({})", opts.join(" · ")).into()
            };
            (
                title,
                format!("name (empty = {})", app.default_session_name("agent")).into(),
                String::new(),
            )
        }
        PromptKind::ClaudeCloudTask { .. } => (
            "Claude Cloud task".into(),
            "what should Claude do?".into(),
            String::new(),
        ),
        PromptKind::AgentPresetTask { preset, .. } => (
            format!("Task for {}", preset.name).into(),
            if preset.has_wrapping() {
                format!("{} — prefix + your task + postfix", preset.spec_label()).into()
            } else {
                format!("{} — sent as the first prompt", preset.spec_label()).into()
            },
            String::new(),
        ),
        PromptKind::CloudMessage { .. } => (
            "Send to cloud session".into(),
            "message for the cloud agent".into(),
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
            ("Rename agent".into(), "name".into(), current)
        }
        PromptKind::RenameTerminal { id } => {
            let current = app
                .tree
                .terminals
                .iter()
                .find(|t| &t.id == id)
                .map(|t| t.name.clone())
                .unwrap_or_default();
            ("Rename terminal".into(), "name".into(), current)
        }
        PromptKind::RenameProject { id } => {
            let current = app
                .tree
                .projects
                .iter()
                .find(|p| &p.id == id)
                .map(|p| p.name.clone())
                .unwrap_or_default();
            (
                "Rename project".into(),
                "name (empty resets to the folder name)".into(),
                current,
            )
        }
        PromptKind::NewWorkspace => ("New workspace".into(), "name".into(), String::new()),
        PromptKind::RenameWorkspace { id } => {
            let current = app
                .tree
                .workspaces
                .iter()
                .find(|w| &w.id == id)
                .map(|w| w.name.clone())
                .unwrap_or_default();
            ("Rename workspace".into(), "name".into(), current)
        }

        PromptKind::EditLink { id } => {
            let current = app
                .tree
                .links
                .iter()
                .find(|l| &l.id == id)
                .map(|l| l.url.clone())
                .unwrap_or_default();
            ("Edit link".into(), "URL".into(), current)
        }
    };
    app.overlay = Some(Overlay::Prompt(PromptDialog::new(
        title, label, input, kind,
    )));
}

/// Open the selected repo's page on its git host (`G`). Any worktree
/// answers, since every checkout of a project shares one remote — so the
/// cursor's worktree decides, falling back to the project's own clone when
/// it has no worktrees yet or the one selected is gone from disk.
fn open_repo_in_browser(app: &mut App) {
    let root = app
        .selected_worktree()
        .map(|w| w.path.clone())
        .filter(|path| path.is_dir())
        .or_else(|| app.selected_project().map(|p| p.repo_path.clone()));
    let Some(root) = root else {
        app.flash = Some(SELECT_CONTEXT_FIRST.into());
        return;
    };
    match crate::remote::repo_url(&root) {
        // Not open_link: this is a repo page, never a PR row to mark read.
        Ok(url) if open_url(&url) => {
            app.flash = Some(format!("opened {}", crate::app::pretty_url(&url)))
        }
        Ok(url) => app.flash = Some(format!("couldn't open {url}")),
        Err(msg) => app.flash = Some(msg),
    }
}

/// The selected worktree's checkout — path and branch — for the modals that
/// read it. Flashes and returns None when no worktree is selected or its
/// path is gone from disk.
fn selected_checkout(app: &mut App) -> Option<(std::path::PathBuf, String)> {
    // Clone before touching app.overlay — selected_worktree borrows app.
    let Some((path, branch)) = app
        .selected_worktree()
        .map(|w| (w.path.clone(), w.branch.clone()))
    else {
        app.flash = Some("no worktree selected".into());
        return None;
    };
    if !path.is_dir() {
        app.flash = Some(format!("worktree path missing on disk: {}", path.display()));
        return None;
    }
    Some((path, branch))
}

/// Every tracked + untracked file of a checkout, plus the configured editor
/// command, for the finder and tree modals. Flashes and returns None when
/// git fails or the checkout has no files.
fn load_worktree_files(
    app: &mut App,
    path: &std::path::Path,
    branch: &str,
) -> Option<(Vec<String>, String)> {
    let files = match crate::git_diff::list_files(path) {
        Ok(files) => files,
        Err(msg) => {
            app.flash = Some(msg);
            return None;
        }
    };
    if files.is_empty() {
        app.flash = Some(format!("no files in {branch}"));
        return None;
    }
    let editor = crate::config::Config::load().editor_command();
    Some((files, editor))
}

fn open_diff_view(app: &mut App) {
    let Some((path, branch)) = selected_checkout(app) else {
        return;
    };
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
/// `Shift+H`: destinations remembered by `nebula ssh`, newest first. Opens even
/// when empty — the modal's hint is how the feature introduces itself.
fn open_hosts_picker(app: &mut App) {
    app.overlay = Some(Overlay::Hosts(crate::app::HostsView::new(
        crate::hosts::load(),
    )));
}

fn open_file_finder(app: &mut App) {
    let Some((path, branch)) = selected_checkout(app) else {
        return;
    };
    let Some((files, editor)) = load_worktree_files(app, &path, &branch) else {
        return;
    };
    app.overlay = Some(Overlay::Files(FileFinder::new(path, branch, editor, files)));
}

/// Tree browser (`b`): full file tree of the selected worktree with a
/// content preview, filterable by file name. Same shell as `open_diff_view`:
/// flash instead of opening when there's no worktree, the path is gone, or
/// git fails.
fn open_tree_browser(app: &mut App) {
    let Some((path, branch)) = selected_checkout(app) else {
        return;
    };
    let Some((files, editor)) = load_worktree_files(app, &path, &branch) else {
        return;
    };
    app.overlay = Some(Overlay::Tree(TreeBrowser::new(path, branch, editor, files)));
}

/// Find-in-files (`F`): live `git grep` over the selected worktree; Enter
/// on a hit opens it in the editor modal. Same shell as `open_diff_view`.
fn open_grep_view(app: &mut App) {
    let Some((path, branch)) = selected_checkout(app) else {
        return;
    };
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
    // Size guess from the last-drawn body; the post-draw sync corrects it.
    let size = vim_size_guess(app);
    spawn_editor_modal(app, &editor, &root, &path, line, size);
}

/// Boot `editor` on `file` at `line` (cwd `root`) into the editor modal at
/// grid `size`, replacing whatever it held; a spawn failure flashes. False
/// when nothing was spawned — the main loop isn't running (unit tests
/// without a channel) or the spawn failed.
fn spawn_editor_modal(
    app: &mut App,
    editor: &str,
    root: &std::path::Path,
    file: &str,
    line: u64,
    size: (u16, u16),
) -> bool {
    let Some(tx) = app.vim_tx.clone() else {
        return false;
    };
    let (cols, rows) = size;
    app.vim_generation += 1;
    match VimTerm::spawn_editor(editor, root, file, line, cols, rows, app.vim_generation, tx) {
        Ok(vim) => {
            app.vim = Some(vim);
            true
        }
        Err(msg) => {
            app.flash = Some(msg);
            false
        }
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
    // Size guess from the last-drawn body; the post-draw sync corrects it.
    let size = vim_size_guess(app);
    spawn_editor_modal(app, &editor, &root, &path, 1, size);
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
    let size = if pane_usable(preview) {
        (preview.width, preview.height)
    } else {
        vim_size_guess(app) // never drawn yet
    };
    if spawn_editor_modal(app, &editor, &root, &path, 1, size) {
        if let Some(vim) = &mut app.vim {
            vim.embedded = true;
        }
    }
}

/// Expected inner size of the editor modal before its first draw, derived
/// from the last-drawn body rect (`VIM_MODAL_PCT` of the frame, minus the
/// border). `sync_vim_size` trues it up after the real draw.
fn vim_size_guess(app: &App) -> (u16, u16) {
    let frame_w = app.body_area.width;
    let frame_h = app.body_area.height + 2; // + footer row and its padding
    let cols = (frame_w * ui::VIM_MODAL_PCT.0 / 100)
        .saturating_sub(2)
        .max(MIN_PANE_DIM);
    let rows = (frame_h * ui::VIM_MODAL_PCT.1 / 100)
        .saturating_sub(2)
        .max(MIN_PANE_DIM);
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
    let size = vim_size_guess(app);
    spawn_editor_modal(app, &editor, &root, &file, line.unwrap_or(1), size);
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
    let mut candidates = vec![path];
    for prefix in ["a/", "b/"] {
        if let Some(rest) = path.strip_prefix(prefix) {
            candidates.push(rest);
        }
    }
    for cand in candidates {
        let full = if let Some(rest) = cand.strip_prefix("~/") {
            nebula_core::env::home_dir()?.join(rest)
        } else {
            // join() with an absolute candidate yields the candidate.
            root.join(cand)
        };
        if full.is_file() {
            return Some(if cand.starts_with("~/") {
                full.to_string_lossy().into_owned()
            } else {
                cand.to_string()
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
    send(app, out, |req_id| ClientRequest::ArchiveAgent {
        req_id,
        id,
    });
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
    let Some(worktree) = worktree_in_context(app) else {
        app.flash = Some(SELECT_CONTEXT_FIRST.into());
        return;
    };
    create_terminal(app, worktree, out);
}

/// Ask the daemon for a shell terminal in `worktree`; the Ack attaches it.
fn create_terminal(app: &mut App, worktree: WorktreeId, out: &mut Vec<ClientRequest>) {
    send_with(app, out, PendingIntent::AttachCreated, |req_id| {
        ClientRequest::CreateTerminal {
            req_id,
            worktree,
            name: None,
        }
    });
}

/// The worktree the selection stands for: the selected one, or the selected
/// project's main checkout (root) when the Projects panel has focus.
fn worktree_in_context(app: &App) -> Option<WorktreeId> {
    match app.focus {
        Focus::Projects => app.selected_project().and_then(|p| {
            app.tree
                .worktrees
                .iter()
                .find(|w| w.project_id == p.id && w.is_main)
                .map(|w| w.id.clone())
        }),
        _ => app.selected_worktree().map(|w| w.id.clone()),
    }
}

fn open_delete_confirm(app: &mut App) {
    match app.focus {
        Focus::Projects => {
            if let Some(p) = app.selected_project() {
                app.overlay = Some(Overlay::Confirm(confirm_remove_project(
                    &p.name,
                    p.id.clone(),
                )));
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
                    area: ratatui::layout::Rect::default(),
                }));
            }
        }
        Focus::Sessions => match app.selected_session_row() {
            Some(SessionRow::Agent(a)) => {
                app.overlay = Some(Overlay::Confirm(confirm_delete_agent(&a.name, a.id)));
            }
            Some(SessionRow::Terminal(t)) => {
                app.overlay = Some(Overlay::Confirm(confirm_close_terminal(&t.name, t.id)));
            }
            Some(SessionRow::Link(l)) => delete_link(app, &l),
            None => {}
        },
        Focus::Workspaces => {
            let id = app.tree.active_workspace.clone();
            open_remove_workspace_confirm(app, id, None);
        }
        Focus::Terminal => {}
    }
}

/// The confirm before an agent is deleted — from the `d` key and the row
/// menu alike, so the two never drift apart in wording.
fn confirm_delete_agent(name: &str, id: AgentId) -> ConfirmDialog {
    ConfirmDialog {
        title: "Delete agent".into(),
        message: format!("Delete agent '{name}'? Its session and history go away."),
        action: PendingAction::DeleteAgent(id),
        area: ratatui::layout::Rect::default(),
    }
}

/// The confirm before a terminal tab is closed (key and menu).
fn confirm_close_terminal(name: &str, id: TerminalId) -> ConfirmDialog {
    ConfirmDialog {
        title: "Close terminal".into(),
        message: format!("Close terminal '{name}'? Its shell is killed."),
        action: PendingAction::CloseTerminal(id),
        area: ratatui::layout::Rect::default(),
    }
}

/// The confirm before a project is dropped from the list (key and menu).
fn confirm_remove_project(name: &str, id: ProjectId) -> ConfirmDialog {
    ConfirmDialog {
        title: "Remove project".into(),
        message: format!("Remove '{name}' from nebula? Nothing on disk is touched."),
        action: PendingAction::RemoveProject(id),
        area: ratatui::layout::Rect::default(),
    }
}

/// Edit the URL behind a link row. The detected pull request has no stored
/// row to rewrite — it comes back from git on every lookup.
fn edit_link(app: &mut App, row: &LinkRow) {
    match row.id() {
        Some(id) => open_prompt(app, PromptKind::EditLink { id: id.clone() }),
        None => app.flash = Some("the pull request comes from git and can't be edited".into()),
    }
}

/// Delete a link row, with the same confirm every other `d` gets. The
/// detected pull request isn't ours to delete: it would be back on the next
/// lookup.
fn delete_link(app: &mut App, row: &LinkRow) {
    let Some(id) = row.id() else {
        app.flash = Some("the pull request link can't be deleted — it comes from git".into());
        return;
    };
    app.overlay = Some(Overlay::Confirm(ConfirmDialog {
        title: "Delete link".into(),
        message: format!(
            "Delete link '{}'? Nothing it points at is touched.",
            row.label()
        ),
        action: PendingAction::DeleteLink(id.clone()),
        area: ratatui::layout::Rect::default(),
    }));
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
                area: ratatui::layout::Rect::default(),
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
            // Links are bookmarks, not sessions: `D` never touches them.
            let doomed: Vec<SessionRow> = doomed
                .into_iter()
                .filter(|r| r.as_link().is_none())
                .collect();
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
                    SessionRow::Link(_) => unreachable!("filtered out above"),
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
                area: ratatui::layout::Rect::default(),
            }));
        }
        Focus::Workspaces | Focus::Projects | Focus::Terminal => {}
    }
}

/// Row menu for a link: open it, read its diff when it is a pull request,
/// and — unless it's the pull request nebula found in git — edit or delete
/// it.
fn menu_items_for_link(row: &LinkRow) -> Vec<MenuItem> {
    let mut items = vec![MenuItem::new(
        "Open in browser",
        MenuAction::OpenLink(row.url().to_string()),
    )];
    if row.pull_request().is_some() {
        items.push(MenuItem::new("View diff", MenuAction::ViewPrDiff));
    }
    if let Some(id) = row.id() {
        items.push(MenuItem::new("Edit URL", MenuAction::EditLink(id.clone())));
        items.push(MenuItem::destructive(
            "Delete",
            MenuAction::DeleteLink(id.clone()),
        ));
    }
    items
}

fn menu_items_for_session(a: &nebula_core::Agent) -> Vec<MenuItem> {
    let mut items = if a.archived {
        vec![
            MenuItem::new("Unarchive", MenuAction::UnarchiveAgent(a.id.clone())),
            MenuItem::destructive("Delete", MenuAction::DeleteAgent(a.id.clone())),
        ]
    } else {
        vec![
            MenuItem::new(
                "Attach",
                MenuAction::Attach(SessionRef::Agent(a.id.clone())),
            ),
            MenuItem::new("Restart", MenuAction::RestartAgent(a.id.clone())),
            MenuItem::new("Rename", MenuAction::RenameAgent(a.id.clone())),
            MenuItem::new("Archive", MenuAction::ArchiveAgent(a.id.clone())),
            MenuItem::destructive("Delete", MenuAction::DeleteAgent(a.id.clone())),
        ]
    };
    // A Cloud row can always be re-entered explicitly — even after a
    // teleport made it a local session that Restart now resumes, a fresh
    // attach/teleport picks up whatever the cloud side did since.
    if !a.archived && a.cloud_session_id.is_some() {
        let after_restart = items
            .iter()
            .position(|i| matches!(i.action, MenuAction::RestartAgent(_)))
            .map_or(items.len(), |i| i + 1);
        items.insert(
            after_restart,
            MenuItem::new(
                "Attach cloud session",
                MenuAction::AttachCloudAgent(a.id.clone()),
            ),
        );
        items.insert(
            after_restart + 1,
            MenuItem::new(
                "Send to cloud session",
                MenuAction::SendCloudMessage(a.id.clone()),
            ),
        );
    }
    items
}

fn menu_items_for_terminal(t: &nebula_core::TerminalTab) -> Vec<MenuItem> {
    vec![
        MenuItem::new(
            "Attach",
            MenuAction::Attach(SessionRef::Terminal(t.id.clone())),
        ),
        MenuItem::new("Rename", MenuAction::RenameTerminal(t.id.clone())),
        MenuItem::destructive("Close", MenuAction::CloseTerminal(t.id.clone())),
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
        filter: None,
    }));
}

/// Step 1 of new-session creation: pick which CLI the session runs. The
/// kind chains into the name prompt via `MenuAction::NewAgentOfKind` —
/// unless `skip_session_naming` is on, which creates it right there.
/// Claude/Codex rows expand (→) into model and effort submenus; Enter
/// anywhere takes the configured defaults for whatever wasn't drilled into.
/// A plain TERMINAL SESSION is not offered here: NEW TERMINAL (`t`) and the
/// CONTEXT MENU's "New terminal" already cover it.
fn open_new_agent_picker(app: &mut App, worktree: WorktreeId) {
    // Only the AGENT KINDS still enabled in the SETTINGS OVERLAY's Agents
    // tab are offered; a disabled harness is absent, not greyed.
    let kinds = crate::config::Config::load().enabled_kinds();
    if kinds.is_empty() {
        // Only a hand-edited config gets here (the overlay refuses to turn
        // off the last harness). An empty ContextMenu would index past its
        // rows on Enter and `j`, so flash instead of opening one.
        app.flash = Some("every harness is disabled — enable one in Settings › Agents".into());
        return;
    }
    let items = kinds
        .into_iter()
        .map(|kind| {
            MenuItem::new(
                kind_label(kind),
                MenuAction::NewAgentOfKind {
                    worktree: worktree.clone(),
                    kind,
                    model: None,
                    effort: None,
                    cloud: false,
                    pr_url: None,
                },
            )
        })
        .collect();
    app.overlay = Some(Overlay::Menu(ContextMenu {
        title: Some("New session".into()),
        items,
        at: None,
        hover: 0,
        area: ratatui::layout::Rect::default(),
        parent: None,
        filter: None,
    }));
}

/// Whether Claude is still offered — the PR SESSION launch and the standing
/// PREWARM POOL slot are Claude-only, so both follow this toggle.
fn claude_enabled() -> bool {
    crate::config::Config::load().kind_enabled(AgentKind::Claude)
}

const CLAUDE_DISABLED_FLASH: &str = "Claude is disabled in Settings › Agents — PR sessions need it";

/// ROOT WORKTREE used by PROJECT-scoped actions. An OPEN PRS row has no
/// checkout of its own, so PR-created AGENTS follow the same established
/// fallback as PROJECT-scoped TERMINAL SESSION and LINK creation.
fn selected_project_main_worktree(app: &App) -> Option<WorktreeId> {
    let project = app.selected_project()?;
    app.tree
        .worktrees
        .iter()
        .find(|worktree| worktree.project_id == project.id && worktree.is_main)
        .map(|worktree| worktree.id.clone())
}

fn pr_agent_menu_item(
    worktree: WorktreeId,
    pr: &crate::pull_request::OpenPr,
    label: &str,
) -> MenuItem {
    MenuItem::new(
        label,
        MenuAction::NewAgentOfKind {
            worktree,
            kind: AgentKind::Claude,
            model: None,
            effort: None,
            cloud: false,
            pr_url: Some(pr.url.clone()),
        },
    )
}

/// An OPEN PRS row creates only a local Claude AGENT, while still reusing
/// the MODEL/EFFORT and optional naming steps of the NEW SESSION PICKER.
fn open_pr_agent_picker(app: &mut App) {
    let Some(pr) = app.selected_worktree_pr().cloned() else {
        return;
    };
    if !claude_enabled() {
        app.flash = Some(CLAUDE_DISABLED_FLASH.into());
        return;
    }
    let Some(worktree) = selected_project_main_worktree(app) else {
        app.flash = Some("the project has no ROOT WORKTREE for this PR session".into());
        return;
    };
    app.overlay = Some(Overlay::Menu(ContextMenu {
        title: Some(format!("New PR session · #{}", pr.number)),
        items: vec![pr_agent_menu_item(worktree, &pr, "Claude")],
        at: None,
        hover: 0,
        area: ratatui::layout::Rect::default(),
        parent: None,
        filter: None,
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
        cloud,
        pr_url,
        ..
    } = &item.action
    else {
        return None;
    };
    let cfg = crate::config::Config::load();
    let (title, choices, configured) = match sub {
        SubmenuKind::Models => (
            format!("{} model", kind_label(*kind)),
            crate::config::model_choices(*kind),
            cfg.default_model(*kind),
        ),
        SubmenuKind::Efforts => (
            format!("{} effort", kind_label(*kind)),
            crate::config::effort_choices(*kind, model.as_deref()),
            cfg.default_effort(*kind),
        ),
    };
    let configured = configured.unwrap_or_else(|| "default".into());
    let items: Vec<MenuItem> = choices
        .iter()
        .map(|choice| {
            MenuItem::new(
                if *choice == configured {
                    format!("{choice} ✓")
                } else {
                    (*choice).to_string()
                },
                MenuAction::NewAgentOfKind {
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
                    cloud: *cloud,
                    pr_url: pr_url.clone(),
                },
            )
        })
        .collect();
    let hover = choices.iter().position(|c| *c == configured).unwrap_or(0);
    // Both lists take type-ahead: letters narrow the rows, ↑/↓ move.
    let filter = Some(MenuFilter {
        query: String::new(),
        all: items.clone(),
    });
    Some(ContextMenu {
        title: Some(title),
        items,
        at: None,
        hover,
        area: ratatui::layout::Rect::default(),
        parent: None,
        filter,
    })
}

fn kind_label(kind: AgentKind) -> &'static str {
    match kind {
        AgentKind::Claude => "Claude",
        AgentKind::Codex => "Codex",
        AgentKind::Cursor => "Cursor",
    }
}

/// Workspace switcher (`w`): pick which workspace this instance shows. The
/// active one is checked and starts highlighted; Enter switches here and
/// here only — another nebula window on another workspace stays put.
/// Management verbs are keys with footer hints:
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
            MenuItem::new(
                format!(
                    "{}{}  ({projects})",
                    w.name,
                    if &w.id == active { " ✓" } else { "" }
                ),
                MenuAction::OpenWorkspace(w.id.clone()),
            )
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
        filter: None,
    }));
}

/// Rebuild an open workspace switcher after the workspace list (or the ✓
/// marker) changed under it, keeping the cursor row. The menu's rows are
/// snapshots, so refresh them here.
/// Show `id` in THIS instance. Everything visible re-filters; the selection
/// lands on the new workspace's first project with its remembered
/// worktree/session brought back.
///
/// The daemon is told, but only so it can scope this connection's future
/// `AddProject` and remember where a fresh instance should boot — it
/// broadcasts nothing, which is the whole point: another nebula window
/// stays on the workspace its user left it on.
fn switch_workspace(app: &mut App, id: WorkspaceId, out: &mut Vec<ClientRequest>) -> bool {
    switch_workspace_inner(app, id, true, out)
}

/// The scope change without the landing: same as [`switch_workspace`] but
/// it leaves every cursor at the top and the pane on the old session. Only
/// for callers that place the selection themselves — a `/` jump into
/// another workspace names the exact row it wants, and restoring that
/// workspace's remembered session first would attach it just to detach it
/// a request later.
fn switch_workspace_quietly(app: &mut App, id: WorkspaceId, out: &mut Vec<ClientRequest>) -> bool {
    switch_workspace_inner(app, id, false, out)
}

/// True when the switch happened (false means it was already the open one).
fn switch_workspace_inner(
    app: &mut App,
    id: WorkspaceId,
    restore: bool,
    out: &mut Vec<ClientRequest>,
) -> bool {
    if app.tree.active_workspace == id {
        return false;
    }
    remember_context(app);
    app.tree.active_workspace = id.clone();
    app.sel_project = 0;
    if restore {
        // Land on the project this workspace was left on before restoring
        // the worktree and session, which are remembered per project and
        // per worktree — restoring them against row 0 would bring back the
        // wrong project's context.
        restore_workspace_project(app);
        restore_context(app, out);
    }
    clamp_selections(app);
    refresh_palette(app);
    // An open switcher keeps its ✓ on the now-open workspace.
    refresh_workspace_picker(app);
    send(app, out, |req_id| ClientRequest::OpenWorkspace {
        req_id,
        id,
    });
    app.dirty = true;
    true
}

/// Re-scope after the OPEN WORKSPACE disappeared from under us — deleted
/// here (`d` in the WORKSPACES BAR or the WORKSPACE SWITCHER), from another
/// instance, or from `nebula workspace delete`. The daemon refuses to
/// delete a non-empty workspace, so there is nothing on screen to lose;
/// it's the scope itself that has to move somewhere real. It lands on the
/// WORKSPACE TAB that sat to the deleted one's right; the last tab falls
/// back to the one on its left. `removed_tab` is the position the deleted
/// workspace held in the bar before `apply_removal` dropped its row.
/// Deleting a workspace that is not the open one changes nothing.
fn reseat_deleted_workspace(
    app: &mut App,
    removed_tab: Option<usize>,
    out: &mut Vec<ClientRequest>,
) {
    if app
        .tree
        .workspaces
        .iter()
        .any(|w| w.id == app.tree.active_workspace)
    {
        return;
    }
    if app.tree.workspaces.is_empty() {
        return; // never expected: the daemon refuses to delete the last one
    }
    // With the row gone, its right-hand neighbor now sits at the deleted
    // tab's own index; an index past the end means it was the last tab,
    // so clamp onto the new last — its former left-hand neighbor.
    let land = removed_tab.unwrap_or(0).min(app.tree.workspaces.len() - 1);
    let fallback = app.tree.workspaces[land].id.clone();
    switch_workspace(app, fallback, out);
}

/// Put a confirm in front of deleting a workspace — every path to it (the
/// column's `d`, its menu, the `w` switcher's `d`) lands here. The daemon
/// still guards the delete itself (only an empty workspace goes, never the
/// last one), so a refusal after `y` just flashes. `reopen_picker` is the
/// switcher's hover row when the confirm came from there.
fn open_remove_workspace_confirm(app: &mut App, id: WorkspaceId, reopen_picker: Option<usize>) {
    let Some(name) = app
        .tree
        .workspaces
        .iter()
        .find(|w| w.id == id)
        .map(|w| w.name.clone())
    else {
        return;
    };
    app.overlay = Some(Overlay::Confirm(ConfirmDialog {
        title: "Delete workspace".into(),
        // Two lines: the dialog is sized to its longest line, and one
        // 85-column sentence would outgrow a narrow terminal.
        message: format!(
            "Delete workspace '{name}'?\nOnly an empty workspace can go — nothing on disk is touched."
        ),
        action: PendingAction::RemoveWorkspace { id, reopen_picker },
        area: ratatui::layout::Rect::default(),
    }));
}

/// Ask the daemon to delete a workspace (after the confirm): it only
/// deletes empty workspaces, so a refusal just flashes, and when the open
/// one does go the deletion delta reseats this instance
/// (`reseat_deleted_workspace`).
fn remove_workspace(app: &mut App, id: WorkspaceId, out: &mut Vec<ClientRequest>) {
    send(app, out, |req_id| ClientRequest::RemoveWorkspace {
        req_id,
        id,
    });
}

/// Bring the `w` switcher back after a confirm that replaced it, on the
/// row it was on (clamped — the row may have just been deleted).
fn reopen_workspace_picker(app: &mut App, hover: usize) {
    if app.tree.workspaces.is_empty() {
        return;
    }
    open_workspace_picker(app);
    if let Some(Overlay::Menu(menu)) = &mut app.overlay {
        menu.hover = hover.min(menu.items.len().saturating_sub(1));
    }
}

/// The Workspaces column's menu (`m`, right-click): the switcher's n / r / d
/// verbs as rows, for the open workspace.
fn workspace_menu(app: &App) -> Vec<MenuItem> {
    let id = app.tree.active_workspace.clone();
    vec![
        MenuItem::new("New workspace", MenuAction::NewWorkspace),
        MenuItem::new("Rename workspace", MenuAction::RenameWorkspace(id.clone())),
        MenuItem::destructive("Delete workspace", MenuAction::RemoveWorkspace(id)),
    ]
}

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
    reopen_workspace_picker(app, hover);
}

fn open_context_menu_for_selection(app: &mut App) {
    let at = KEYBOARD_MENU_ANCHOR;
    match app.focus {
        Focus::Workspaces => {
            let items = workspace_menu(app);
            open_menu(app, items, at);
        }
        Focus::Projects => {
            let mut items = vec![MenuItem::new("Add project", MenuAction::AddProject)];
            if let Some(p) = app.selected_project() {
                items.insert(
                    0,
                    MenuItem::new("New worktree", MenuAction::NewWorktree(p.id.clone())),
                );
                items.push(MenuItem::new(
                    "Rename",
                    MenuAction::RenameProject(p.id.clone()),
                ));
                items.push(MenuItem::destructive(
                    "Remove from list",
                    MenuAction::RemoveProject(p.id.clone()),
                ));
            }
            open_menu(app, items, at);
        }
        Focus::Worktrees => {
            if let Some(pr) = app.selected_worktree_pr().cloned() {
                let Some(worktree) = selected_project_main_worktree(app) else {
                    app.flash = Some("the project has no ROOT WORKTREE for this PR session".into());
                    return;
                };
                let mut items = Vec::new();
                if claude_enabled() {
                    items.push(pr_agent_menu_item(worktree, &pr, "New Claude session"));
                }
                items.extend([
                    MenuItem::new("Open in browser", MenuAction::OpenLink(pr.url)),
                    MenuItem::new("View diff", MenuAction::ViewPrDiff),
                ]);
                open_menu(app, items, at);
            } else if let Some(w) = app.selected_worktree() {
                let mut items = vec![
                    MenuItem::new("New agent", MenuAction::NewAgent(w.id.clone())),
                    MenuItem::new("New terminal", MenuAction::NewTerminal(w.id.clone())),
                ];
                if !w.is_main {
                    items.push(MenuItem::destructive(
                        "Delete worktree",
                        MenuAction::DeleteWorktree(w.id.clone()),
                    ));
                }
                open_menu(app, items, at);
            }
        }
        Focus::Sessions => match app.selected_session_row() {
            Some(SessionRow::Agent(a)) => open_menu(app, menu_items_for_session(&a), at),
            Some(SessionRow::Terminal(t)) => open_menu(app, menu_items_for_terminal(&t), at),
            Some(SessionRow::Link(l)) => open_menu(app, menu_items_for_link(&l), at),
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
        Overlay::Help(_) => {
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
                view.selected = clamp_selection(view.selected as i64 + (1), view.rows.len());
            }
            KeyCode::Char('k') | KeyCode::Up => {
                view.selected = clamp_selection(view.selected as i64 + (-1), view.rows.len());
            }
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
                    view.selected = clamp_selection(view.selected as i64 + (1), view.hosts.len());
                }
                KeyCode::Char('k') | KeyCode::Up => {
                    view.selected = clamp_selection(view.selected as i64 + (-1), view.hosts.len());
                }
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
                        view.selected = clamp_selection(view.selected as i64, view.hosts.len());
                        crate::hosts::remove(&entry);
                    }
                }
                _ => {}
            }
        }
        Overlay::AgentPresets(_) => crate::preset_overlays::handle_list_key(app, key),
        Overlay::AgentPresetEditor(_) => crate::preset_overlays::handle_editor_key(app, key),
        Overlay::Menu(menu) => match key.code {
            // Type-ahead in the MODEL / EFFORT submenus: letters narrow the
            // rows (so ↑/↓ move there, not j/k), Backspace widens, and Esc
            // clears the text before it backs out. A letter no row matches
            // is refused, so the list never empties.
            KeyCode::Char(c)
                if menu.filter.is_some()
                    && c != ' '
                    && !key.modifiers.contains(KeyModifiers::CONTROL) =>
            {
                let query = format!("{}{c}", menu.filter_query());
                if !menu.type_filter(c) {
                    app.flash = Some(format!("no row matches '{query}'"));
                }
            }
            KeyCode::Backspace if menu.filter.is_some() => menu.pop_filter(),
            KeyCode::Esc if menu.has_filter_text() => {
                menu.set_filter("");
            }
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
            // The root picker's Claude row owns Tab as a launch-mode
            // toggle. Submenus and every other menu leave it untouched.
            KeyCode::Tab if menu.toggle_hovered_claude_cloud() => {}
            // Workspace-switcher verbs (footer-hinted): n creates a
            // workspace (opened on Ack), r renames
            // the hovered one, d deletes it behind a confirm that hands
            // the switcher back either way.
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
                    let hover = menu.hover;
                    open_remove_workspace_confirm(app, id, Some(hover));
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
                        cloud: false,
                        ..
                    } => Some(worktree.clone()),
                    _ => None,
                };
                // Abandoning a preset's task goes back to the list it came
                // from, on the same row, rather than to the panels.
                let back_to_presets = match &prompt.kind {
                    PromptKind::AgentPresetTask { worktree, preset } => {
                        Some((worktree.clone(), preset.name.clone()))
                    }
                    _ => None,
                };
                app.overlay = None;
                if let Some(worktree) = restore {
                    out.extend(default_claude_prewarm(worktree));
                }
                if let Some((worktree, name)) = back_to_presets {
                    let index = crate::agent_presets::load()
                        .iter()
                        .position(|p| p.name == name)
                        .unwrap_or(0);
                    crate::preset_overlays::reopen_agent_presets(app, worktree, index);
                }
            }
            KeyCode::Char('j')
                if prompt.is_multiline() && key.modifiers.contains(KeyModifiers::CONTROL) =>
            {
                prompt.input.insert_char('\n');
            }
            KeyCode::Enter
                if prompt.is_multiline() && key.modifiers.contains(KeyModifiers::SHIFT) =>
            {
                prompt.input.insert_char('\n');
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
                let home = nebula_core::env::home_dir();
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
            KeyCode::Esc | KeyCode::Char('n') => {
                // Backing out lands where you were: a settings reset
                // reopens the overlay, a switcher delete reopens the
                // switcher — not the panels.
                let to_settings = matches!(confirm.action, PendingAction::ResetSettings);
                let to_picker = match &confirm.action {
                    PendingAction::RemoveWorkspace { reopen_picker, .. } => *reopen_picker,
                    _ => None,
                };
                let to_presets = match &confirm.action {
                    PendingAction::DeleteAgentPreset { index, worktree } => {
                        Some((*index, worktree.clone()))
                    }
                    _ => None,
                };
                app.overlay = None;
                if to_settings {
                    reopen_settings(app);
                } else if let Some(hover) = to_picker {
                    reopen_workspace_picker(app, hover);
                } else if let Some((index, worktree)) = to_presets {
                    crate::preset_overlays::reopen_agent_presets(app, worktree, index);
                }
            }
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
                    let landing = Landing::for_enter(palette.enter_attaches);
                    if let Some(target) = palette.selected_target().cloned() {
                        app.overlay = None;
                        jump_to_target(app, target, landing, out);
                    }
                }
                KeyCode::Char('o') if ctrl => {
                    if let Some(target) = palette.selected_target().cloned() {
                        app.overlay = None;
                        jump_to_target(app, target, Landing::Attach, out);
                    }
                }
                KeyCode::Char('f') if ctrl => {
                    if let Some(target) = palette.selected_target().cloned() {
                        app.overlay = None;
                        jump_to_target(app, target, Landing::FocusOnly, out);
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
                        let label = format!("copied {path}");
                        copy_and_flash(app, &path, &label);
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
                        let label = format!("copied {path}");
                        copy_and_flash(app, &path, &label);
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
    }
}

/// Settings overlay keys. Three modes share this handler, in priority
/// order: capturing a hotkey (every press is the binding), confirming a
/// duplicate (Enter takes it, anything else backs out), and ordinary
/// navigation.
///
/// The tab strip is a focusable row above the list — ↑ from the top row
/// steps onto it, where ←/→ walk the tabs and ↓ drops back in. That's what
/// keeps arrows working for tabs *and* for cycling a setting's value:
/// which one a press means is decided by where the cursor is, not by a
/// mode the user has to remember. Tab / ⇧Tab / `[` / `]` / 1-9 switch tabs
/// from anywhere and never mean anything else.
fn handle_settings_key(app: &mut App, key: KeyEvent) {
    let Some(view) = settings(app) else {
        return;
    };
    if view.capturing() {
        capture_hotkey(app, key);
        return;
    }
    if view.capture.is_some() {
        // Holding a captured chord that already belongs to someone else.
        if key.code == KeyCode::Enter {
            commit_pending_hotkey(app);
        } else if let Some(view) = settings_mut(app) {
            view.capture = None;
            view.info("kept the existing binding");
        }
        return;
    }

    let (tab, selected, on_tabs) = (view.tab, view.selected, view.on_tabs);
    let last = crate::config::tab_len(tab).saturating_sub(1);
    let tabs = crate::config::tab_count();
    let hotkeys = view.is_hotkeys();
    let shift = key.modifiers.contains(KeyModifiers::SHIFT);

    let cmd = match key.code {
        KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('s') => SettingsCmd::Close,
        KeyCode::BackTab => SettingsCmd::Tab((tab + tabs - 1) % tabs),
        KeyCode::Tab if shift => SettingsCmd::Tab((tab + tabs - 1) % tabs),
        KeyCode::Tab => SettingsCmd::Tab((tab + 1) % tabs),
        KeyCode::Char('[') => SettingsCmd::Tab((tab + tabs - 1) % tabs),
        KeyCode::Char(']') => SettingsCmd::Tab((tab + 1) % tabs),
        // 1-9 jump straight to a tab, the fastest route once you know the
        // strip; out-of-range digits are ignored rather than clamped.
        KeyCode::Char(c @ '1'..='9') => {
            let want = c as usize - '1' as usize;
            if want < tabs {
                SettingsCmd::Tab(want)
            } else {
                return;
            }
        }
        // Shift+R: back to the defaults, behind a confirmation. It isn't
        // about a row, so it works from the strip and the list alike.
        KeyCode::Char('R') => SettingsCmd::ResetAll,
        // ---- the tab strip has focus ----
        KeyCode::Left | KeyCode::Char('h') if on_tabs => SettingsCmd::Tab((tab + tabs - 1) % tabs),
        KeyCode::Right | KeyCode::Char('l') if on_tabs => SettingsCmd::Tab((tab + 1) % tabs),
        KeyCode::Down | KeyCode::Char('j') | KeyCode::Enter if on_tabs => SettingsCmd::EnterList,
        KeyCode::Up | KeyCode::Char('k') if on_tabs => return,
        // ---- the list has focus ----
        KeyCode::Char('j') | KeyCode::Down => SettingsCmd::Move((selected + 1).min(last)),
        // ↑ off the top row steps onto the tab strip.
        KeyCode::Char('k') | KeyCode::Up if selected == 0 => SettingsCmd::FocusTabs,
        KeyCode::Char('k') | KeyCode::Up => SettingsCmd::Move(selected - 1),
        KeyCode::Enter | KeyCode::Char(' ') if hotkeys => SettingsCmd::Capture { add: false },
        KeyCode::Char('a') | KeyCode::Char('+') if hotkeys => SettingsCmd::Capture { add: true },
        KeyCode::Backspace | KeyCode::Delete if hotkeys => SettingsCmd::ResetHotkey,
        KeyCode::Char('x') if hotkeys => SettingsCmd::ClearHotkey,
        // Nothing to cycle on a hotkey row — say so instead of no-op'ing.
        KeyCode::Char('h') | KeyCode::Left | KeyCode::Char('l') | KeyCode::Right if hotkeys => {
            SettingsCmd::Nudge
        }
        KeyCode::Enter | KeyCode::Char(' ') => SettingsCmd::Apply(selected, 0),
        KeyCode::Char('l') | KeyCode::Right => SettingsCmd::Apply(selected, 1),
        KeyCode::Char('h') | KeyCode::Left => SettingsCmd::Apply(selected, -1),
        _ => return,
    };

    match cmd {
        SettingsCmd::Close => close_settings(app),
        SettingsCmd::Tab(next) => {
            app.settings_tab = next;
            let row = app.settings_row(next);
            if let Some(view) = settings_mut(app) {
                view.tab = next;
                view.selected = row;
                view.notice = None;
                view.capture = None;
            }
        }
        SettingsCmd::FocusTabs => {
            app.remember_settings_focus(true);
            if let Some(view) = settings_mut(app) {
                view.on_tabs = true;
                view.notice = None;
            }
        }
        SettingsCmd::EnterList => {
            app.remember_settings_focus(false);
            if let Some(view) = settings_mut(app) {
                view.on_tabs = false;
            }
        }
        SettingsCmd::Move(i) => {
            app.remember_settings_row(tab, i);
            if let Some(view) = settings_mut(app) {
                view.selected = i;
                view.notice = None;
            }
        }
        SettingsCmd::Apply(i, delta) => apply_setting_at(app, tab, i, delta),
        SettingsCmd::Capture { add } => {
            if let Some(view) = settings_mut(app) {
                view.capture = Some(crate::app::HotkeyCapture {
                    action: selected,
                    add,
                    pending: None,
                });
                view.notice = None;
            }
        }
        SettingsCmd::ResetHotkey => {
            if edit_keymap(app, |keymap| keymap.reset(selected)) {
                let label = app.keymap.display_at(selected);
                if let Some(view) = settings_mut(app) {
                    view.info(format!("reset to the default binding: {label}"));
                }
            }
        }
        SettingsCmd::ClearHotkey => {
            if edit_keymap(app, |keymap| keymap.clear(selected)) {
                if let Some(view) = settings_mut(app) {
                    view.warn("unbound — ⌫ puts the default back");
                }
            }
        }
        SettingsCmd::Nudge => {
            if let Some(view) = settings_mut(app) {
                view.info("Enter: rebind   a: add another key   ⌫: default   x: unbind");
            }
        }
        SettingsCmd::ResetAll => {
            // The confirm replaces the overlay; both of its exits put the
            // settings back on screen (see `reset_settings` and the Esc
            // arm of the Confirm handler).
            app.overlay = Some(Overlay::Confirm(ConfirmDialog {
                title: "Reset settings".into(),
                message: "Every setting goes back to its default: theme, editor, agent \
                          defaults,\ntimeouts, panel visibility, and all hotkey \
                          bindings.\nYour config.json is rewritten; this can't be undone."
                    .into(),
                action: PendingAction::ResetSettings,
                area: ratatui::layout::Rect::default(),
            }));
        }
    }
}

/// The open settings overlay, for handlers that already know it's up.
fn settings(app: &App) -> Option<&SettingsView> {
    match &app.overlay {
        Some(Overlay::Settings(view)) => Some(view),
        _ => None,
    }
}

/// `settings`, mutably.
fn settings_mut(app: &mut App) -> Option<&mut SettingsView> {
    match &mut app.overlay {
        Some(Overlay::Settings(view)) => Some(view),
        _ => None,
    }
}

enum SettingsCmd {
    Close,
    Tab(usize),
    FocusTabs,
    EnterList,
    Move(usize),
    Apply(usize, i32),
    Capture { add: bool },
    ResetHotkey,
    ClearHotkey,
    Nudge,
    ResetAll,
}

/// The keystroke that lands while the Hotkeys tab is waiting for one.
/// Esc is the only key that can't be bound — it's the way out of here.
fn capture_hotkey(app: &mut App, key: KeyEvent) {
    // Bare modifier presses aren't chords; keep waiting for a real key.
    if matches!(
        key.code,
        KeyCode::Null | KeyCode::CapsLock | KeyCode::NumLock | KeyCode::ScrollLock
    ) || matches!(key.code, KeyCode::Modifier(_))
    {
        return;
    }
    let Some(view) = settings_mut(app) else {
        return;
    };
    let Some(capture) = view.capture.clone() else {
        return;
    };
    if key.code == KeyCode::Esc {
        view.capture = None;
        view.info("rebind cancelled");
        return;
    }
    let chord = crate::keymap::KeyChord::from_event(&key);
    let conflicts = app.keymap.conflicts(capture.action, &chord);
    if !conflicts.is_empty() {
        // Warn before stealing: the user gets to see who currently owns
        // the key and decide, instead of finding out when that action
        // stops responding.
        let owners = conflicts
            .iter()
            .filter_map(|i| crate::keymap::spec_at(*i))
            .map(|s| format!("\u{201c}{}\u{201d}", s.label))
            .collect::<Vec<_>>()
            .join(", ");
        if let Some(view) = settings_mut(app) {
            view.warn(format!(
                "{chord} is already {owners} — Enter to move it here, Esc to keep it there"
            ));
            if let Some(c) = &mut view.capture {
                c.pending = Some((chord, conflicts));
            }
        }
        return;
    }
    bind_hotkey(app, capture.action, chord, capture.add);
}

/// Enter on the duplicate warning: take the chord anyway.
fn commit_pending_hotkey(app: &mut App) {
    let Some(view) = settings(app) else {
        return;
    };
    let Some(capture) = view.capture.clone() else {
        return;
    };
    let Some((chord, losers)) = capture.pending else {
        return;
    };
    let stolen_from = losers
        .iter()
        .filter_map(|i| crate::keymap::spec_at(*i))
        .map(|s| s.label)
        .collect::<Vec<_>>()
        .join(", ");
    bind_hotkey(app, capture.action, chord, capture.add);
    if let Some(view) = settings_mut(app) {
        if !stolen_from.is_empty() {
            view.warn(format!(
                "{chord} taken from {stolen_from}, which is now unbound there"
            ));
        }
    }
}

/// Write one binding through to the config, then report how likely the
/// host terminal is to actually deliver it.
fn bind_hotkey(app: &mut App, action: usize, chord: crate::keymap::KeyChord, add: bool) {
    let saved = edit_keymap(app, |keymap| keymap.bind(action, chord, add));
    let Some(view) = settings_mut(app) else {
        return;
    };
    view.capture = None;
    if !saved {
        return;
    }
    match crate::keymap::host_warning(&chord) {
        (crate::keymap::Reach::Fine, _) => view.info(format!("bound to {chord}")),
        (_, Some(why)) => view.warn(format!("bound to {chord}, but {why}")),
        (_, None) => view.info(format!("bound to {chord}")),
    }
}

/// Persist a keymap and adopt it. False means the write failed and nothing
/// changed, so callers skip their success message.
fn save_keymap(app: &mut App, keymap: crate::keymap::Keymap) -> bool {
    let mut cfg = crate::config::Config::load();
    cfg.keybindings = keymap.overrides();
    if !save_config(app, &cfg) {
        return false;
    }
    app.keymap = keymap;
    true
}

/// Clone the live keymap, let `edit` change it, and persist the result.
/// False means the write failed and the live keymap is untouched.
fn edit_keymap(app: &mut App, edit: impl FnOnce(&mut crate::keymap::Keymap)) -> bool {
    let mut keymap = app.keymap.clone();
    edit(&mut keymap);
    save_keymap(app, keymap)
}

/// Write the config file, flashing the failure. False when it didn't land.
fn save_config(app: &mut App, cfg: &crate::config::Config) -> bool {
    match cfg.save() {
        Ok(()) => true,
        Err(err) => {
            app.flash = Some(format!("couldn't save settings: {err}"));
            false
        }
    }
}

fn apply_setting_at(app: &mut App, tab: usize, index: usize, delta: i32) {
    let mut cfg = crate::config::Config::load();
    cfg.cycle(tab, index, delta);
    if cfg.enabled_kinds().is_empty() {
        // Refuse the last harness here, where the user is looking, rather
        // than leave `n` with nothing to offer later.
        if let Some(view) = settings_mut(app) {
            view.warn("keep at least one harness enabled");
        }
        return;
    }
    if !save_config(app, &cfg) {
        return;
    }
    apply_config(app, &cfg);
}

/// Adopt every config value the running app mirrors. Shared by startup and
/// the settings overlay so a new setting can't reach one and miss the
/// other — the overlay is a live editor, not a restart-to-apply screen.
fn apply_config(app: &mut App, cfg: &crate::config::Config) {
    app.theme = cfg.theme();
    app.animations = cfg.animations;
    app.focus_tint = cfg.focus_tint;
    set_show_workspaces(app, cfg.show_workspaces);
    set_hide_projects(app, cfg.hide_projects);
    set_hide_worktrees(app, cfg.hide_worktrees);
}

/// `R` in the settings overlay, confirmed: rewrite config.json from the
/// defaults, adopt them live (values and hotkeys both), and put the
/// overlay back where it was so the reset values are the next thing on
/// screen.
fn reset_settings(app: &mut App) {
    let result = crate::config::Config::reset_to_defaults();
    reopen_settings(app);
    match result {
        Ok(cfg) => {
            apply_config(app, &cfg);
            app.keymap = cfg.keymap();
            if let Some(view) = settings_mut(app) {
                view.info("every setting is back to its default");
            }
        }
        Err(err) => app.flash = Some(format!("couldn't reset settings: {err}")),
    }
}

/// Open the settings overlay from the panels. The remembered tab / row /
/// strip-vs-list is restored only while it's fresh: closed more than
/// [`crate::app::SETTINGS_MEMORY_TTL`] ago, it's forgotten and the overlay
/// comes up like a first open — first tab, top row, cursor on the strip.
fn open_settings(app: &mut App) {
    if app.settings_memory_expired() {
        app.forget_settings_focus();
    }
    reopen_settings(app);
}

/// Put the settings overlay back up on its remembered tab and row, no
/// questions asked. `open_settings` is the from-the-panels entry that
/// checks the memory's age first; this one is for mid-visit round trips
/// (the reset confirmation) where the position can't have gone stale.
fn reopen_settings(app: &mut App) {
    let tab = app.settings_tab;
    app.overlay = Some(Overlay::Settings(SettingsView::new(
        tab,
        app.settings_row(tab),
        app.settings_on_tabs,
    )));
}

/// Take the settings overlay down and start the clock on its remembered
/// position (see `open_settings`). Both ways out — Esc/`q`/`s` and a click
/// outside the modal — go through here.
fn close_settings(app: &mut App) {
    app.overlay = None;
    app.note_settings_closed();
}

/// Show or hide the Workspaces bar, moving a cursor parked there onto the
/// first visible sidebar.
fn set_show_workspaces(app: &mut App, shown: bool) {
    app.show_workspaces = shown;
    if !shown && app.focus == Focus::Workspaces {
        app.focus = app.first_sidebar_focus();
    }
}

fn set_hide_projects(app: &mut App, hidden: bool) {
    app.hide_projects = hidden;
    if hidden && app.focus == Focus::Projects {
        app.focus = app.next_visible_focus(Focus::Projects);
    }
}

fn set_hide_worktrees(app: &mut App, hidden: bool) {
    app.hide_worktrees = hidden;
    if hidden && app.focus == Focus::Worktrees {
        app.focus = app.next_visible_focus(Focus::Worktrees);
    }
}

fn save_panel_visibility(app: &mut App) {
    let mut cfg = crate::config::Config::load();
    cfg.hide_projects = app.hide_projects;
    cfg.hide_worktrees = app.hide_worktrees;
    if let Err(err) = cfg.save() {
        app.flash = Some(format!("couldn't save settings: {err}"));
    }
}

fn submit_prompt(app: &mut App, prompt: PromptDialog, out: &mut Vec<ClientRequest>) {
    let value = prompt.input.trim().to_string();
    // A cloud session cannot start without its task. Keep the multiline
    // dialog open on validation so the user can correct it in place.
    if prompt.is_multiline() {
        // The cloud prompts and a preset's task share the bounds (the text
        // crosses the same argv), not the wording.
        let (needs, what) = match &prompt.kind {
            PromptKind::AgentPresetTask { .. } => ("the preset needs a task", "task"),
            _ => ("Claude Cloud needs a task", "Claude Cloud task"),
        };
        let error = if value.is_empty() {
            Some(needs.to_string())
        } else if value.contains('\0') {
            Some(format!("{what} cannot contain NUL bytes"))
        } else if value.len() > MAX_CLOUD_PROMPT_BYTES {
            Some(format!(
                "{what} is too long (max {} KiB)",
                MAX_CLOUD_PROMPT_BYTES / 1024
            ))
        } else if let PromptKind::AgentPresetTask { preset, .. } = &prompt.kind {
            // The wrapped text crosses the same argv as the task alone, so
            // it gets the same ceiling — a long prefix can push a valid
            // task over it.
            (preset.compose(&value).len() > MAX_CLOUD_PROMPT_BYTES).then(|| {
                format!(
                    "prefix + task + postfix is too long (max {} KiB)",
                    MAX_CLOUD_PROMPT_BYTES / 1024
                )
            })
        } else {
            None
        };
        if let Some(error) = error {
            app.flash = Some(error);
            app.overlay = Some(Overlay::Prompt(prompt));
            return;
        }
    }
    // An empty agent name falls back to the next free default (agent-1, …),
    // an empty worktree name to the random branch the prompt offered, and an
    // empty project name undoes the rename — the row goes back to the
    // folder's own name, which is the only way back from a rename. For every
    // other prompt an empty field is a cancel.
    if value.is_empty()
        && !matches!(
            prompt.kind,
            PromptKind::NewAgent { .. }
                | PromptKind::NewWorktree { .. }
                | PromptKind::RenameProject { .. }
        )
    {
        app.flash = Some("cancelled: empty input".into());
        return;
    }
    match prompt.kind {
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
                    area: ratatui::layout::Rect::default(),
                }));
                return;
            }
            send_with(app, out, PendingIntent::SelectCreatedProject, |req_id| {
                ClientRequest::AddProject {
                    req_id,
                    path: expanded,
                    name: None,
                    create_missing: false,
                }
            });
        }
        PromptKind::NewWorktree {
            project,
            suggestion,
        } => {
            // "fix login redirect" is how a branch gets described out
            // loud; git wants it hyphenated. Nothing typed at all takes
            // the random name the prompt was offering.
            let branch = crate::branch_name::slugify(&value);
            let branch = if branch.is_empty() {
                suggestion
            } else {
                branch
            };
            send_with(app, out, PendingIntent::SelectCreatedWorktree, |req_id| {
                ClientRequest::CreateWorktree {
                    req_id,
                    project,
                    branch,
                    base: None,
                }
            });
        }
        PromptKind::NewAgent {
            worktree,
            kind,
            model,
            effort,
            cloud,
            pr_url,
        } => {
            if cloud {
                open_prompt(
                    app,
                    PromptKind::ClaudeCloudTask {
                        worktree,
                        name: value,
                        model,
                        effort,
                    },
                );
            } else {
                create_agent(
                    app,
                    AgentLaunchDraft {
                        worktree,
                        kind,
                        model,
                        effort,
                        name: value,
                        cloud_prompt: None,
                        starting_prompt: None,
                        reopen_on_error: None,
                        pr_url,
                    },
                    out,
                );
            }
        }
        PromptKind::ClaudeCloudTask {
            worktree,
            name,
            model,
            effort,
        } => create_agent(
            app,
            AgentLaunchDraft {
                worktree,
                kind: AgentKind::Claude,
                model,
                effort,
                name,
                cloud_prompt: Some(value),
                starting_prompt: None,
                reopen_on_error: None,
                pr_url: None,
            },
            out,
        ),
        PromptKind::AgentPresetTask { worktree, preset } => {
            // Sized above, with the task — composing cannot fail here.
            let starting_prompt = preset.compose(&value);
            // A preset pins a model / effort or follows Settings → Agents,
            // exactly as the NEW SESSION PICKER's rows do.
            let cfg = crate::config::Config::load();
            let kind = preset.kind;
            let model = preset.model.clone().or_else(|| cfg.default_model(kind));
            let effort = crate::config::fit_effort(
                kind,
                model.as_deref(),
                preset.effort.clone().or_else(|| cfg.default_effort(kind)),
            );
            create_agent(
                app,
                AgentLaunchDraft {
                    worktree: worktree.clone(),
                    kind,
                    model,
                    effort,
                    name: String::new(),
                    cloud_prompt: None,
                    starting_prompt: Some(starting_prompt),
                    reopen_on_error: Some((
                        PromptKind::AgentPresetTask { worktree, preset },
                        value,
                    )),
                    pr_url: None,
                },
                out,
            );
        }
        PromptKind::CloudMessage { id } => {
            let intent = PendingIntent::ReopenPromptOnError {
                kind: PromptKind::CloudMessage { id: id.clone() },
                text: value.clone(),
                note: "Sent to the cloud session — pulling the transcript".into(),
            };
            send_with(app, out, intent, |req_id| ClientRequest::SendCloudMessage {
                req_id,
                id,
                message: value,
            });
        }
        PromptKind::RenameAgent { id } => {
            send(app, out, |req_id| ClientRequest::RenameAgent {
                req_id,
                id,
                name: value,
            });
        }
        PromptKind::RenameTerminal { id } => {
            send(app, out, |req_id| ClientRequest::RenameTerminal {
                req_id,
                id,
                name: value,
            });
        }
        PromptKind::RenameProject { id } => {
            let req_id = app.alloc_req_id(PendingIntent::None);
            out.push(ClientRequest::RenameProject {
                req_id,
                id,
                name: value,
            });
        }
        PromptKind::NewWorkspace => {
            // Created from the switcher: open it as soon as the Ack lands.
            send_with(app, out, PendingIntent::OpenCreatedWorkspace, |req_id| {
                ClientRequest::AddWorkspace {
                    req_id,
                    name: value,
                }
            });
        }
        PromptKind::RenameWorkspace { id } => {
            send(app, out, |req_id| ClientRequest::RenameWorkspace {
                req_id,
                id,
                name: value,
            });
        }

        PromptKind::EditLink { id } => {
            send(app, out, |req_id| ClientRequest::UpdateLink {
                req_id,
                id,
                url: value,
            });
        }
    }
}

fn run_pending_action(app: &mut App, action: PendingAction, out: &mut Vec<ClientRequest>) {
    match action {
        PendingAction::CreateProjectDir(path) => {
            send_with(app, out, PendingIntent::SelectCreatedProject, |req_id| {
                ClientRequest::AddProject {
                    req_id,
                    path,
                    name: None,
                    create_missing: true,
                }
            });
        }
        PendingAction::DeleteAgent(id) => delete_agent(app, id, out),
        PendingAction::CloseTerminal(id) => close_terminal(app, id, out),
        PendingAction::DeleteLink(id) => {
            send(app, out, |req_id| ClientRequest::DeleteLink { req_id, id });
        }
        PendingAction::DeleteWorktree(id) => {
            // Optimistic: drop the rows now (the daemon deletes in the
            // background — `git worktree remove` can take seconds). The
            // eventual EntityRemoved is a no-op; an Error for this req_id
            // restores the rows via the rollback stashed in the intent.
            let before = selection_snapshot(app);
            delete_worktree(app, id, out);
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
                delete_worktree(app, id, out);
            }
            reconcile_selection(app, before, out);
        }
        PendingAction::DeleteAllSessions { agents, terminals } => {
            for id in agents {
                delete_agent(app, id, out);
            }
            for id in terminals {
                close_terminal(app, id, out);
            }
        }
        PendingAction::RemoveProject(id) => {
            send(app, out, |req_id| ClientRequest::RemoveProject {
                req_id,
                id,
            });
        }
        PendingAction::RemoveWorkspace { id, reopen_picker } => {
            remove_workspace(app, id, out);
            // The switcher stays up across the delete, as it did before the
            // confirm: the EntityRemoved delta drops the row in place.
            if let Some(hover) = reopen_picker {
                reopen_workspace_picker(app, hover);
            }
        }
        PendingAction::DeleteAgentPreset { index, worktree } => {
            let mut presets = crate::agent_presets::load();
            if index < presets.len() {
                let removed = presets.remove(index);
                match crate::agent_presets::save(&presets) {
                    Ok(()) => app.flash = Some(format!("deleted preset '{}'", removed.name)),
                    Err(err) => app.flash = Some(format!("could not save agent presets: {err}")),
                }
            }
            crate::preset_overlays::reopen_agent_presets(app, worktree, index);
        }
        PendingAction::ResetSettings => reset_settings(app),
        PendingAction::Quit => app.should_quit = true,
    }
}

/// Delete an agent for good, detaching the pane first if it's showing it.
fn delete_agent(app: &mut App, id: AgentId, out: &mut Vec<ClientRequest>) {
    detach_if_attached(app, &SessionRef::Agent(id.clone()), out);
    send(app, out, |req_id| ClientRequest::DeleteAgent { req_id, id });
}

/// Close a terminal tab, detaching the pane first if it's showing it.
fn close_terminal(app: &mut App, id: TerminalId, out: &mut Vec<ClientRequest>) {
    detach_if_attached(app, &SessionRef::Terminal(id.clone()), out);
    send(app, out, |req_id| ClientRequest::CloseTerminal {
        req_id,
        id,
    });
}

/// Delete a worktree optimistically: drop its rows now (the daemon deletes
/// in the background — `git worktree remove` can take seconds). The
/// eventual EntityRemoved is a no-op; an Error for this req_id restores the
/// rows via the rollback stashed in the intent.
fn delete_worktree(app: &mut App, id: WorktreeId, out: &mut Vec<ClientRequest>) {
    let intent = match remove_worktree_rows(app, &id) {
        Some(rollback) => PendingIntent::DeleteWorktree(rollback),
        None => PendingIntent::None,
    };
    send_with(app, out, intent, |req_id| ClientRequest::DeleteWorktree {
        req_id,
        id,
        force: true,
    });
}

fn run_menu_action(app: &mut App, action: MenuAction, out: &mut Vec<ClientRequest>) {
    match action {
        MenuAction::Attach(sref) => {
            attach_now(app, sref, out);
            app.focus = Focus::Terminal;
            app.term_locked = true;
        }
        MenuAction::RestartAgent(id) => {
            send(app, out, |req_id| ClientRequest::RestartAgent {
                req_id,
                id,
            });
        }
        MenuAction::AttachCloudAgent(id) => {
            send(app, out, |req_id| ClientRequest::AttachCloudAgent {
                req_id,
                id,
            });
        }
        MenuAction::SendCloudMessage(id) => open_prompt(app, PromptKind::CloudMessage { id }),
        MenuAction::RenameAgent(id) => open_prompt(app, PromptKind::RenameAgent { id }),
        MenuAction::ArchiveAgent(id) => {
            archive_agent(app, id, out);
        }
        MenuAction::UnarchiveAgent(id) => {
            send(app, out, |req_id| ClientRequest::UnarchiveAgent {
                req_id,
                id,
            });
        }
        MenuAction::DeleteAgent(id) => {
            if let Some(a) = app.tree.agents.iter().find(|a| a.id == id).cloned() {
                app.overlay = Some(Overlay::Confirm(confirm_delete_agent(&a.name, id)));
            }
        }
        MenuAction::NewAgent(worktree) => open_new_agent_picker(app, worktree),
        MenuAction::NewTerminal(worktree) => create_terminal(app, worktree, out),
        MenuAction::RenameTerminal(id) => open_prompt(app, PromptKind::RenameTerminal { id }),
        MenuAction::CloseTerminal(id) => {
            if let Some(t) = app.tree.terminals.iter().find(|t| t.id == id).cloned() {
                app.overlay = Some(Overlay::Confirm(confirm_close_terminal(&t.name, id)));
            }
        }
        MenuAction::NewAgentOfKind {
            worktree,
            kind,
            model,
            effort,
            cloud,
            pr_url,
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
            // A Cursor effort only counts for the family it belongs to: a
            // stale setting behind a freshly picked model drops to none.
            let effort = crate::config::fit_effort(
                kind,
                model.as_deref(),
                resolve(effort, cfg.default_effort(kind)),
            );
            // No name prompt means no typing window to warm through, so
            // create straight from the picker: the standing default-spec
            // warm slot gets adopted where it matches, and the refill
            // behind the create re-warms it either way.
            if cfg.skip_session_naming {
                if cloud {
                    open_prompt(
                        app,
                        PromptKind::ClaudeCloudTask {
                            worktree,
                            name: String::new(),
                            model,
                            effort,
                        },
                    );
                } else {
                    create_agent(
                        app,
                        AgentLaunchDraft {
                            worktree,
                            kind,
                            model,
                            effort,
                            name: String::new(),
                            cloud_prompt: None,
                            starting_prompt: None,
                            reopen_on_error: None,
                            pr_url,
                        },
                        out,
                    );
                }
                return;
            }
            // Warm the CLI while the user types the name: the daemon
            // pre-spawns the session so CreateAgent adopts an already-booted
            // PTY. Fail-soft — a missing CLI just means a cold spawn later.
            if !cloud && pr_url.is_none() {
                out.push(ClientRequest::PrewarmAgent {
                    worktree: worktree.clone(),
                    kind,
                    model: model.clone(),
                    effort: effort.clone(),
                });
            }
            open_prompt(
                app,
                PromptKind::NewAgent {
                    worktree,
                    kind,
                    model,
                    effort,
                    cloud,
                    pr_url,
                },
            )
        }
        MenuAction::NewWorktree(project) => open_new_worktree_prompt(app, project),
        MenuAction::OpenLink(url) => open_link(app, &url, out),
        MenuAction::ViewPrDiff => request_pr_diff(app),
        MenuAction::EditLink(id) => open_prompt(app, PromptKind::EditLink { id }),
        MenuAction::DeleteLink(id) => {
            if let Some(row) = app
                .visible_links()
                .into_iter()
                .find(|l| l.id() == Some(&id))
            {
                delete_link(app, &row);
            }
        }
        MenuAction::DeleteWorktree(id) => {
            if let Some(w) = app.tree.worktrees.iter().find(|w| w.id == id).cloned() {
                app.overlay = Some(Overlay::Confirm(ConfirmDialog {
                    title: "Delete worktree".into(),
                    message: format!("Delete worktree '{}' from disk?", w.branch),
                    action: PendingAction::DeleteWorktree(id),
                    area: ratatui::layout::Rect::default(),
                }));
            }
        }
        MenuAction::AddProject => open_prompt(app, PromptKind::AddProject),
        MenuAction::RenameProject(id) => open_prompt(app, PromptKind::RenameProject { id }),
        MenuAction::OpenWorkspace(id) => {
            switch_workspace(app, id, out);
        }
        MenuAction::NewWorkspace => open_prompt(app, PromptKind::NewWorkspace),
        MenuAction::RenameWorkspace(id) => open_prompt(app, PromptKind::RenameWorkspace { id }),
        MenuAction::RemoveWorkspace(id) => open_remove_workspace_confirm(app, id, None),
        MenuAction::RemoveProject(id) => {
            if let Some(p) = app.tree.projects.iter().find(|p| p.id == id).cloned() {
                app.overlay = Some(Overlay::Confirm(confirm_remove_project(&p.name, id)));
            }
        }
        MenuAction::ToggleArchived => toggle_archived(app, out),
    }
}

fn detach_if_attached(app: &mut App, sref: &SessionRef, out: &mut Vec<ClientRequest>) {
    // The daemon may hold this session even when the pane has already moved
    // on to another one whose attach is still debounced — release it either
    // way, or the connection stays attached to a row that no longer exists.
    let showing = app.term.as_ref().is_some_and(|t| &t.sref == sref);
    if app.attached_sref.as_ref() == Some(sref) || showing {
        out.push(ClientRequest::Detach {
            session: sref.clone(),
        });
        if app.attached_sref.as_ref() == Some(sref) {
            app.attached_sref = None;
        }
    }
    if showing {
        app.pending_attach = None;
        app.term = None;
        app.term_locked = false;
        if app.focus == Focus::Terminal {
            app.focus = Focus::Sessions;
        }
    }
}

fn shellexpand_home(path: &str) -> std::path::PathBuf {
    if let Some(rest) = path.strip_prefix("~/") {
        if let Some(home) = nebula_core::env::home_dir() {
            return home.join(rest);
        }
    }
    std::path::PathBuf::from(path)
}

/// Snapshot the context being left — which project row this workspace was
/// on, which worktree row that project was on, and which session row that
/// worktree was on — so switching back restores all three. Call BEFORE
/// moving the selection away.
fn remember_context(app: &mut App) {
    // The workspace's project is recorded first and unconditionally: a
    // project with no worktree yet still has to come back under the cursor,
    // and the early return below would skip it.
    if let Some(pid) = app.selected_project().map(|p| p.id.clone()) {
        app.last_project_for_workspace
            .insert(app.tree.active_workspace.clone(), pid);
    }
    let Some(wid) = app.selected_worktree().map(|w| w.id.clone()) else {
        return;
    };
    if let Some(pid) = app.selected_project().map(|p| p.id.clone()) {
        app.last_worktree_for_project.insert(pid, wid.clone());
    }
    let row = app.selected_session_row();
    // A link row is not a session. Leaving the worktree with the cursor
    // parked on one must not forget which session it was last on — that
    // would blank the pane on the way back.
    if row.as_ref().is_some_and(|r| r.as_link().is_some()) {
        return;
    }
    match row.and_then(|r| r.sref()) {
        Some(sref) => {
            app.last_session_for_worktree.insert(wid, sref);
        }
        None => {
            app.last_session_for_worktree.remove(&wid);
        }
    }
}

/// After a workspace switch: put the cursor back on the project the user
/// left this workspace on. Silent when there is nothing remembered (first
/// visit) or the project is gone — the caller has already parked the
/// cursor on the first row.
fn restore_workspace_project(app: &mut App) {
    let Some(pid) = app
        .last_project_for_workspace
        .get(&app.tree.active_workspace)
        .cloned()
    else {
        return;
    };
    if let Some(i) = app
        .project_rows()
        .iter()
        .position(|i| app.tree.projects[*i].id == pid)
    {
        app.sel_project = i;
    }
}

/// After a project switch: land on the project's remembered worktree (its
/// main checkout otherwise), then re-show that worktree's session.
fn restore_context(app: &mut App, out: &mut Vec<ClientRequest>) {
    app.sel_worktree = 0;
    schedule_open_prs_lookup(app);
    schedule_pr_detail(app);
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
    schedule_pr_lookup(app);
    let remembered = app
        .selected_worktree()
        .and_then(|w| app.last_session_for_worktree.get(&w.id).cloned());
    let target = remembered.and_then(|sref| {
        app.visible_session_rows()
            .iter()
            .position(|r| r.sref().as_ref() == Some(&sref) && !r.is_archived_agent())
            .map(|i| (i, sref))
    });
    match target {
        Some((index, sref)) => {
            app.sel_session = index;
            attach(app, sref, out);
        }
        None => {
            if app.term.is_some() {
                detach_pane(app, out);
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
        .position(|r| r.sref().as_ref() == Some(&pending_sref))
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
                .position(|r| r.sref().as_ref() == Some(&pending_sref))
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

/// Land on a project we just added, the way a `/` palette pick of it
/// would: select its row, show its main checkout, and step into the next
/// visible child panel. False when its upsert hasn't arrived yet.
fn select_created_project(
    app: &mut App,
    id: &nebula_core::ProjectId,
    out: &mut Vec<ClientRequest>,
) -> bool {
    if !select_project_row_by_id(app, id) {
        return false;
    }
    restore_context(app, out);
    app.focus = app.next_visible_focus(Focus::Projects);
    true
}

/// Select the Projects-panel row for project `id`, with the manual-move
/// bookkeeping (drop pending selection-follows, remember the context being
/// left). Does NOT restore the target's remembered worktree/session — the
/// caller decides. False when the project is gone from the tree.
fn select_project_row_by_id(app: &mut App, id: &nebula_core::ProjectId) -> bool {
    let rows = app.project_rows();
    let Some(row) = rows.iter().position(|i| &app.tree.projects[*i].id == id) else {
        return false;
    };
    app.select_worktree_when_seen = None;
    remember_context(app);
    app.sel_project = row;
    true
}

/// The workspace a palette pick lives in, when that isn't the open one.
/// A pull request has no workspace of its own — it is opened in a browser
/// and moves no cursor — and a target whose row has vanished resolves to
/// None, leaving the jump's own re-validation to flash.
fn target_workspace(app: &App, target: &PaletteTarget) -> Option<WorkspaceId> {
    let project = |id: &ProjectId| {
        app.tree
            .projects
            .iter()
            .find(|p| &p.id == id)
            .map(|p| p.workspace_id.clone())
    };
    let worktree = |id: &WorktreeId| {
        app.tree
            .worktrees
            .iter()
            .find(|w| &w.id == id)
            .and_then(|w| project(&w.project_id))
    };
    let found = match target {
        PaletteTarget::Workspace(id) => Some(id.clone()),
        PaletteTarget::Project(id) => project(id),
        PaletteTarget::Worktree(id) => worktree(id),
        PaletteTarget::Session(id) => app
            .tree
            .agents
            .iter()
            .find(|a| &a.id == id)
            .and_then(|a| worktree(&a.worktree_id)),
        PaletteTarget::PullRequest(_) => None,
    };
    found.filter(|id| id != &app.tree.active_workspace)
}

/// Land the panel selections on a `/` palette pick. A project or worktree
/// pick moves the selection (restoring remembered child rows, like a manual
/// switch), then hands focus to the next visible child panel, since picking
/// either by name is a step towards one of its children, not an errand in
/// the column it names. A session pick with `attach` opens
/// it immediately, exactly like Enter on its row; without, it only lands
/// on the row in the Sessions panel, previewing like ↑/↓ there. Targets
/// are re-validated against the
/// tree — a pick can race a removal, in which case it flashes instead of
/// jumping.
/// Land the palette/finder on `target`, attaching without the debounce —
/// the user typed a query and picked a row, which is as explicit as it gets.
fn jump_to_target(
    app: &mut App,
    target: PaletteTarget,
    landing: Landing,
    out: &mut Vec<ClientRequest>,
) {
    jump_to_target_inner(app, target, landing, out);
    fire_pending_attach(app, out);
}

fn jump_to_target_inner(
    app: &mut App,
    target: PaletteTarget,
    landing: Landing,
    out: &mut Vec<ClientRequest>,
) {
    // Every panel cursor is scoped to the open workspace, so a target
    // living elsewhere has to move this instance there first — otherwise
    // the row it names simply isn't in `project_rows()` and the jump reads
    // as "no longer exists".
    let switched = match target_workspace(app, &target) {
        // A workspace row IS the jump — it wants the full switch, remembered
        // context and all, exactly like the `w` switcher's Enter.
        Some(id) if matches!(target, PaletteTarget::Workspace(_)) => switch_workspace(app, id, out),
        Some(id) => switch_workspace_quietly(app, id, out),
        None => false,
    };
    match target {
        PaletteTarget::Workspace(id) => {
            if !app.tree.workspaces.iter().any(|w| w.id == id) {
                app.flash = Some("workspace no longer exists".into());
                return;
            }
            app.focus = app.first_focus();
        }
        PaletteTarget::Project(id) => {
            // After a quiet switch the pane still shows the workspace we
            // left, so the landing has to run even when the cursor happens
            // to be sitting on the right row already.
            let changed = switched || app.selected_project().map(|p| p.id != id).unwrap_or(true);
            if !select_project_row_by_id(app, &id) {
                app.flash = Some("project no longer exists".into());
                return;
            }
            if changed {
                restore_context(app, out);
            }
            app.focus = app.next_visible_focus(Focus::Projects);
        }
        PaletteTarget::Worktree(id) => {
            if !switched && app.selected_worktree().is_some_and(|w| w.id == id) {
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
                app.flash = Some(SESSION_GONE.into());
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
                app.flash = Some(SESSION_GONE.into());
                return;
            };
            app.sel_session = index;
            match landing {
                Landing::Attach => attach_selected(app, out),
                Landing::FocusOnly => {
                    app.focus = Focus::Sessions;
                    preview_selected(app, out);
                }
            }
        }
        // A pull request isn't in any panel — picking it hands the URL to
        // the browser and leaves every cursor where it was. Enter opens it
        // whether or not the "Enter attaches" setting is on: there is no
        // second, quieter thing for it to do.
        PaletteTarget::PullRequest(url) => open_link(app, &url, out),
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
        app.flash = Some(SESSION_GONE.into());
        return;
    };
    app.sel_worktree = wt_index;
    let Some(index) = app
        .visible_session_rows()
        .iter()
        .position(|r| r.sref().as_ref() == Some(&sref))
    else {
        restore_session(app, out);
        app.focus = Focus::Sessions;
        app.flash = Some(SESSION_GONE.into());
        return;
    };
    app.sel_session = index;
    attach_selected(app, out);
}

/// What a palette pick does once it has found its session row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Landing {
    /// Open the session in the pane, exactly like Enter on its row.
    Attach,
    /// Only land the cursor on the row, previewing like ↑/↓ there.
    FocusOnly,
}

impl Landing {
    /// The palette's Enter: attaches when the setting says so.
    fn for_enter(attaches: bool) -> Self {
        if attaches {
            Landing::Attach
        } else {
            Landing::FocusOnly
        }
    }
}

fn move_selection(app: &mut App, delta: i64, out: &mut Vec<ClientRequest>) {
    // (row count, cursor) of the focused column.
    let (len, sel) = match app.focus {
        Focus::Workspaces => (
            app.tree.workspaces.len(),
            app.tree.active_workspace_index().unwrap_or(0),
        ),
        Focus::Projects => (app.project_rows().len(), app.sel_project),
        Focus::Worktrees => (app.worktree_row_count(), app.sel_worktree),
        Focus::Sessions => (app.visible_session_rows().len(), app.sel_session),
        Focus::Terminal => return,
    };
    if len == 0 {
        return;
    }
    let new = (sel as i64 + delta).clamp(0, len as i64 - 1) as usize;
    if new == sel {
        return;
    }
    // Selecting a different parent resets child selections.
    match app.focus {
        // The Workspaces cursor is the open workspace itself: stepping to
        // the next tab is a switch, with everything a switch entails
        // (remembered context, re-scoped panels, the daemon told).
        Focus::Workspaces => {
            let id = app.tree.workspaces[new].id.clone();
            switch_workspace(app, id, out);
        }
        Focus::Projects => select_project_row(app, new, out),
        Focus::Worktrees => select_worktree_row(app, new, out),
        Focus::Sessions => {
            app.sel_session = new;
            preview_selected(app, out);
        }
        Focus::Terminal => {}
    }
}

/// Move the Projects cursor to a *different* row `i`: the context being
/// left is remembered, and the new project's is restored.
fn select_project_row(app: &mut App, i: usize, out: &mut Vec<ClientRequest>) {
    // A manual move outranks any pending selection-follows.
    app.select_worktree_when_seen = None;
    remember_context(app);
    let owner_before = app.selected_project().map(|p| p.id.clone());
    app.sel_project = i;
    if app.selected_project().map(|p| p.id.clone()) != owner_before {
        restore_context(app, out);
    }
}

/// Move the Worktrees cursor to a *different* row `i` and bring up its
/// session. Stepping onto an open-PR row is not a worktree switch: it has
/// no sessions to restore and nothing to attach, so the pane is left
/// exactly as it was.
fn select_worktree_row(app: &mut App, i: usize, out: &mut Vec<ClientRequest>) {
    app.select_worktree_when_seen = None;
    remember_context(app);
    app.sel_worktree = i;
    if app.selected_worktree().is_some() {
        restore_session(app, out);
    }
    schedule_pr_detail(app);
}

/// Show the selected session in the terminal pane WITHOUT taking focus or
/// the input lock — walking the list with ↑/↓ (or single-clicking a row)
/// previews each session so it can be read; Enter (or a double-click) is
/// what commits: focus + lock. Archived rows don't preview.
fn preview_selected(app: &mut App, out: &mut Vec<ClientRequest>) {
    preview_inner(app, ATTACH_DEBOUNCE, out);
}

/// Preview with no debounce — a click points at exactly one row, so there is
/// no sweep to wait out.
fn preview_selected_now(app: &mut App, out: &mut Vec<ClientRequest>) {
    preview_inner(app, Duration::ZERO, out);
}

fn preview_inner(app: &mut App, delay: Duration, out: &mut Vec<ClientRequest>) {
    let Some(row) = app.selected_session_row() else {
        return;
    };
    if row.is_archived_agent() {
        return;
    }
    // A link row has no session behind it: leave whatever was in the pane
    // rather than blanking it while the cursor passes through the group.
    let Some(sref) = row.sref() else {
        return;
    };
    attach_inner(app, sref, delay, out);
}

/// Enter on the Sessions panel: attach the session under the cursor, or —
/// on a link row — hand its URL to the browser and stay put.
fn attach_selected(app: &mut App, out: &mut Vec<ClientRequest>) {
    let rows = app.visible_session_rows();
    let Some(row) = rows.get(app.sel_session) else {
        return;
    };
    let Some(sref) = row.sref() else {
        if let Some(link) = row.as_link() {
            open_link(app, link.url(), out);
        }
        return;
    };
    attach_now(app, sref, out);
    app.focus = Focus::Terminal;
    app.term_locked = true;
}

/// Leave a locked pane for the Sessions panel. Also expands collapsed
/// sidebars, so there is something on screen to land in.
fn leave_terminal_lock(app: &mut App) {
    app.collapsed = false;
    app.term_locked = false;
    app.focus = Focus::Sessions;
}

/// Open a saved link in the browser, reporting either way — the browser
/// comes up in front of the terminal, so a silent failure would read as
/// "nebula did nothing". A pull request is marked read on the way out: the
/// conversation is about to be on screen, so the row's unread count starts
/// again from here.
fn open_link(app: &mut App, url: &str, out: &mut Vec<ClientRequest>) {
    if open_url(url) {
        app.flash = Some(format!("opened {}", crate::app::pretty_url(url)));
        mark_pr_seen(app, url, out);
    } else {
        app.flash = Some(format!("couldn't open {url}"));
    }
}

/// Record that this pull request has been read up to whatever nebula knows
/// about it. Applied locally as well as sent, so the badge clears on this
/// frame instead of waiting for the daemon to say so — and skipped when the
/// URL isn't a PR, or when the mark wouldn't move.
fn mark_pr_seen(app: &mut App, url: &str, out: &mut Vec<ClientRequest>) {
    let Some(marker) = app
        .pull_requests
        .values()
        .flatten()
        .find(|pr| pr.url == url)
        .map(|pr| pr.seen_marker().to_string())
    else {
        return;
    };
    if app.pr_seen.get(url) == Some(&marker) {
        return;
    }
    app.pr_seen.insert(url.to_string(), marker.clone());
    app.dirty = true;
    out.push(ClientRequest::MarkPrSeen {
        url: url.to_string(),
        marker,
    });
}

/// Record that this agent's session is on screen, so a turn it finished
/// unwatched (`Agent::unseen`) stops counting on its worktree and project
/// rows. Applied locally as well as sent, so the counts drop on this frame
/// instead of waiting for the daemon's upsert — and skipped entirely when
/// there is nothing to clear.
fn mark_agent_seen(app: &mut App, id: &AgentId, out: &mut Vec<ClientRequest>) {
    let Some(a) = app.tree.agents.iter_mut().find(|a| &a.id == id && a.unseen) else {
        return;
    };
    a.unseen = false;
    app.dirty = true;
    out.push(ClientRequest::MarkAgentSeen { id: id.clone() });
}

/// Show `sref` in the pane, telling the daemon once the selection settles.
/// The pane swaps immediately — the header must never name a session other
/// than the selected one — but the Attach itself waits out
/// [`ATTACH_DEBOUNCE`], because attaching a reaped session makes the daemon
/// fork an agent CLI, and a cursor merely passing through a row has not
/// asked for that.
fn attach(app: &mut App, sref: SessionRef, out: &mut Vec<ClientRequest>) {
    attach_inner(app, sref, ATTACH_DEBOUNCE, out);
}

/// Attach with no debounce: the user named this row outright (Enter, a
/// click, the menu, a session they just created), so there is nothing to
/// wait to see whether they meant it.
fn attach_now(app: &mut App, sref: SessionRef, out: &mut Vec<ClientRequest>) {
    attach_inner(app, sref, Duration::ZERO, out);
}

fn attach_inner(app: &mut App, sref: SessionRef, delay: Duration, out: &mut Vec<ClientRequest>) {
    // Whatever lands in the pane has been looked at — walking the cursor
    // onto a row previews it here, so this is where the counts come down.
    // Keyed to the pane swap, not to the Attach: the user is reading the
    // screen during the debounce just the same.
    if let SessionRef::Agent(id) = &sref {
        mark_agent_seen(app, id, out);
    }
    let showing = app
        .term
        .as_ref()
        .is_some_and(|t| t.sref == sref && !t.exited);
    if !showing {
        let (cols, rows) = pane_size(app);
        // Fresh screen, so any persisted selection would point at stale cells.
        app.term_selection = None;
        app.term = Some(AttachedTerm::new(sref.clone(), cols, rows));
        app.dirty = true;
    }
    if delay.is_zero() {
        app.pending_attach = None;
        send_attach(app, sref, out);
    } else if app.attached_sref.as_ref() == Some(&sref) {
        // The daemon already holds it; nothing to send, nothing to wait for.
        app.pending_attach = None;
    } else {
        app.pending_attach = Some((sref, std::time::Instant::now() + delay));
    }
}

/// Move the daemon-side attachment to `sref`, releasing whatever it held.
/// Idempotent, so every caller can just ask for the session it wants.
fn send_attach(app: &mut App, sref: SessionRef, out: &mut Vec<ClientRequest>) {
    if app.attached_sref.as_ref() == Some(&sref) {
        return;
    }
    if let Some(old) = app.attached_sref.take() {
        out.push(ClientRequest::Detach { session: old });
    }
    let (cols, rows) = pane_size(app);
    app.attached_sref = Some(sref.clone());
    out.push(ClientRequest::Attach {
        session: sref,
        from_seq: None,
        cols,
        rows,
    });
}

/// Send the armed attach now — the selection settled, or something needs
/// the session live this instant (a keystroke about to be forwarded).
fn fire_pending_attach(app: &mut App, out: &mut Vec<ClientRequest>) {
    let Some((sref, _)) = app.pending_attach.take() else {
        return;
    };
    send_attach(app, sref, out);
}

/// Release the daemon-side attachment: whatever the daemon holds, or — when
/// an attach is still debounced — the session the pane is showing, so a
/// caller that only knows about the pane still lets go. A Detach the daemon
/// has no attachment for costs it a hash lookup and nothing else.
fn release_attachment(app: &mut App, out: &mut Vec<ClientRequest>) {
    app.pending_attach = None;
    let session = app
        .attached_sref
        .take()
        .or_else(|| app.term.as_ref().map(|t| t.sref.clone()));
    if let Some(session) = session {
        out.push(ClientRequest::Detach { session });
    }
}

/// Blank the pane and release the daemon-side attachment.
fn detach_pane(app: &mut App, out: &mut Vec<ClientRequest>) {
    release_attachment(app, out);
    app.term = None;
    app.term_locked = false;
}

/// Terminal-pane grid for spawn/attach requests; the fallback keeps
/// pre-first-draw requests from booting a 0×0 PTY.
fn pane_size(app: &App) -> (u16, u16) {
    let area = app.term_area;
    if pane_usable(area) {
        (area.width, area.height)
    } else {
        FALLBACK_PANE
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
    out.extend(default_claude_prewarm(worktree));
    app.next_keepwarm = Some(std::time::Instant::now() + KEEPWARM_REFRESH);
}

/// Ask the daemon for a new agent session and attach it once the Ack lands.
/// An empty `name` takes the generated default (agent-1, …) and opts the
/// session into agent-driven auto-titling (`nebula rename` on the first
/// prompt) — that's what accepting an empty name prompt means, and what
/// the `skip_session_naming` setting does without asking. A typed name is
/// the user's choice and stays.
struct AgentLaunchDraft {
    worktree: WorktreeId,
    kind: AgentKind,
    model: Option<String>,
    effort: Option<String>,
    name: String,
    cloud_prompt: Option<String>,
    /// An AGENT PRESET launch's composed first prompt (see
    /// `ClientRequest::CreateAgent::starting_prompt`).
    starting_prompt: Option<String>,
    /// The prompt (and its typed text) to bring back should the daemon
    /// refuse the create — so a rejected preset task is not lost.
    reopen_on_error: Option<(PromptKind, String)>,
    pr_url: Option<String>,
}

fn create_agent(app: &mut App, draft: AgentLaunchDraft, out: &mut Vec<ClientRequest>) {
    let AgentLaunchDraft {
        worktree,
        kind,
        model,
        effort,
        name,
        cloud_prompt,
        starting_prompt,
        reopen_on_error,
        pr_url,
    } = draft;
    let intent = match (reopen_on_error, &cloud_prompt) {
        (Some((kind, task)), _) => PendingIntent::AttachCreatedWithCloudRetry { kind, task },
        (None, Some(task)) => PendingIntent::AttachCreatedWithCloudRetry {
            kind: PromptKind::ClaudeCloudTask {
                worktree: worktree.clone(),
                name: name.clone(),
                model: model.clone(),
                effort: effort.clone(),
            },
            task: task.clone(),
        },
        (None, None) => PendingIntent::AttachCreated,
    };
    let auto_title = name.is_empty();
    let name = if auto_title {
        app.default_session_name("agent")
    } else {
        name
    };
    let cloud = cloud_prompt.is_some();
    let preset = starting_prompt.is_some();
    send_with(app, out, intent, |req_id| match pr_url {
        Some(pr_url) => {
            debug_assert_eq!(kind, AgentKind::Claude);
            debug_assert!(!cloud);
            ClientRequest::CreatePrAgent {
                req_id,
                worktree: worktree.clone(),
                name,
                model,
                effort,
                auto_title,
                pr_url,
            }
        }
        None => ClientRequest::CreateAgent {
            req_id,
            worktree: worktree.clone(),
            name,
            kind,
            model,
            effort,
            auto_title,
            cloud_prompt,
            starting_prompt,
        },
    });
    // The create consumes (or, off-spec, discards) the worktree's warm
    // Claude slot; refill it so the next create is instant too. A cloud or
    // preset launch never touches the slot, so there is nothing to refill.
    if kind == AgentKind::Claude && !cloud && !preset {
        out.extend(default_claude_prewarm(worktree));
    }
}

/// The one spec kept permanently warm: a Claude CLI at the configured
/// default model/effort. Creates matching it adopt the warm session
/// instantly; any other spec launches cold on purpose — off-default CLIs
/// would sit idle holding memory for a spec the user rarely repeats.
/// None while Claude is disabled in Settings: a harness the user turned
/// off should not keep a 150–300 MB WARM SPARE booted behind their back.
fn default_claude_prewarm(worktree: WorktreeId) -> Option<ClientRequest> {
    let cfg = crate::config::Config::load();
    if !cfg.kind_enabled(AgentKind::Claude) {
        return None;
    }
    Some(ClientRequest::PrewarmAgent {
        worktree,
        kind: AgentKind::Claude,
        model: cfg.default_model(AgentKind::Claude),
        effort: cfg.default_effort(AgentKind::Claude),
    })
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
    out.extend(default_claude_prewarm(worktree));
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
        let label = format!("copied {} chars", text.chars().count());
        copy_and_flash(app, &text, &label);
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

/// Copy `text` to the clipboard the user is actually looking at, flashing
/// `label` when it goes out.
///
/// Two routes, because "the clipboard" is not always on this machine. Run
/// locally, we shell out to the platform tool (`copy_to_clipboard`). Run over
/// `nebula ssh`, that tool would target the *remote* box — and a headless VM
/// has no clipboard at all, which is what used to surface as "copy failed
/// (clipboard unavailable)". There we ask the terminal on the near end of the
/// ssh connection instead, via OSC 52; the main loop writes the request.
///
/// OSC 52 is also the fallback for a local host with no display tool, and it
/// is silently dropped by terminals that do not implement it (Terminal.app),
/// so the flash names the route it took rather than claiming success.
fn copy_and_flash(app: &mut App, text: &str, label: &str) {
    // Unit tests exercise the copy flows; don't clobber the developer's real
    // clipboard, and don't depend on their terminal or their $SSH_TTY.
    if cfg!(test) {
        app.flash = Some(label.to_string());
        return;
    }
    if !app.is_remote && copy_to_clipboard(text) {
        app.flash = Some(label.to_string());
        return;
    }
    app.pending_clipboard = Some(base64_encode(text.as_bytes()));
    app.flash = Some(format!("{label} (via terminal)"));
}

/// Base64 (RFC 4648, padded) for OSC 52 payloads — one call site does not
/// justify a dependency.
fn base64_encode(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = u32::from(chunk[0]) << 16
            | u32::from(chunk.get(1).copied().unwrap_or(0)) << 8
            | u32::from(chunk.get(2).copied().unwrap_or(0));
        out.push(ALPHABET[(n >> 18 & 63) as usize] as char);
        out.push(ALPHABET[(n >> 12 & 63) as usize] as char);
        out.push(if chunk.len() > 1 {
            ALPHABET[(n >> 6 & 63) as usize] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            ALPHABET[(n & 63) as usize] as char
        } else {
            '='
        });
    }
    out
}

/// Ring the DONE SOUND once: a named system sound goes to `afplay`
/// (detached; a helper thread reaps it so no zombie lingers), anything
/// else — and an `afplay` that won't start — is the terminal BEL written
/// through `backend`. Reads CONFIG.JSON fresh, like every other setting.
fn play_done_sound<W: std::io::Write>(backend: &mut W) {
    let Some(sound) = crate::config::Config::load().done_sound() else {
        return;
    };
    if let crate::config::DoneSound::File(path) = &sound {
        use std::process::{Command, Stdio};
        if let Ok(mut child) = Command::new("afplay")
            .arg(path)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
        {
            std::thread::spawn(move || {
                let _ = child.wait();
            });
            return;
        }
    }
    let _ = backend.write_all(b"\x07");
    let _ = backend.flush();
}

/// Copy to *this machine's* system clipboard.
/// macOS: pbcopy. Linux: wl-copy on Wayland, xclip (or xsel) on X11.
fn copy_to_clipboard(text: &str) -> bool {
    // Unit tests exercise the selection flow; don't clobber the developer's
    // real clipboard from `cargo test`.
    if cfg!(test) {
        return true;
    }

    use std::io::Write as _;
    use std::process::{Command, Stdio};

    let copy_via = |cmd: &str, args: &[&str]| -> bool {
        let Ok(mut child) = Command::new(cmd)
            .args(args)
            .stdin(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
        else {
            return false;
        };
        let wrote = child
            .stdin
            .take()
            .is_some_and(|mut stdin| stdin.write_all(text.as_bytes()).is_ok());
        wrote && child.wait().is_ok_and(|status| status.success())
    };

    #[cfg(target_os = "macos")]
    {
        copy_via("pbcopy", &[])
    }

    #[cfg(not(target_os = "macos"))]
    {
        if std::env::var_os("WAYLAND_DISPLAY").is_some() {
            return copy_via("wl-copy", &[]);
        }
        // X11: prefer xclip, fall back to xsel
        if copy_via("xclip", &["-selection", "clipboard"]) {
            return true;
        }
        copy_via("xsel", &["--clipboard", "--input"])
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

/// Whether this click on `key` is the second of a double-click. The slot
/// is consumed either way: a double-click is spent, so a third click starts
/// over; a single click re-arms the slot with itself for the next one.
fn is_double_click<T: PartialEq>(slot: &mut Option<(std::time::Instant, T)>, key: T) -> bool {
    let now = std::time::Instant::now();
    let double = slot
        .take()
        .is_some_and(|(at, id)| id == key && now.duration_since(at) <= DOUBLE_CLICK);
    if !double {
        *slot = Some((now, key));
    }
    double
}

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
    let mouse_pos = ratatui::layout::Position::new(mouse.column, mouse.row);
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
    // Help, confirm, prompt, diff: a left-click outside the modal's box
    // dismisses it the way Esc does — a confirm cancels (and lands back in
    // the settings overlay / workspace switcher it came from), a prompt is
    // abandoned (restoring the warm slot's spec), help and the diff viewer
    // just close — and the click is swallowed rather than landing on the
    // panel underneath. The other modals hit-test the same way in their own
    // arms below. An undrawn box (zero width) can't be clicked outside of.
    if let MouseEventKind::Down(MouseButton::Left) = mouse.kind {
        let boxed = match &app.overlay {
            Some(Overlay::Help(v)) => Some(v.area),
            Some(Overlay::Confirm(c)) => Some(c.area),
            Some(Overlay::Prompt(p)) => Some(p.area),
            Some(Overlay::Diff(v)) => Some(v.area),
            // The preset editor backs out to its list, as its Esc does.
            Some(Overlay::AgentPresetEditor(e)) => Some(e.area),
            _ => None,
        };
        if let Some(area) = boxed {
            let inside = mouse.column >= area.x
                && mouse.column < area.x + area.width
                && mouse.row >= area.y
                && mouse.row < area.y + area.height;
            if area.width > 0 && !inside {
                match &app.overlay {
                    Some(Overlay::Help(_)) | Some(Overlay::Diff(_)) => app.overlay = None,
                    _ => handle_overlay_key(
                        app,
                        KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
                        out,
                    ),
                }
                app.dirty = true;
                return;
            }
        }
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
                if area.contains(mouse_pos) {
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
                view.scroll_by(-MODAL_WHEEL_LINES);
                app.dirty = true;
            }
            MouseEventKind::ScrollDown => {
                view.scroll_by(MODAL_WHEEL_LINES);
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
                if area.contains(mouse_pos) {
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
                let inside_list = list.contains(mouse_pos);
                let inside_modal = palette.area.contains(mouse_pos);
                if inside_list {
                    let start = palette.window_start(list.height as usize);
                    let index = start + (mouse.row - list.y) as usize;
                    if index < palette.matches.len() {
                        palette.select(index as i64);
                        let landing = Landing::for_enter(palette.enter_attaches);
                        if let Some(target) = palette.selected_target().cloned() {
                            app.overlay = None;
                            jump_to_target(app, target, landing, out);
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
                let inside_list = list.contains(mouse_pos);
                let inside_modal = finder.area.contains(mouse_pos);
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
                let inside_list = list.contains(mouse_pos);
                let inside_modal = view.area.contains(mouse_pos);
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
                view.scroll_by(-MODAL_WHEEL_LINES);
                app.dirty = true;
            }
            MouseEventKind::ScrollDown => {
                view.scroll_by(MODAL_WHEEL_LINES);
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
                let inside_list = list.contains(mouse_pos);
                let inside_modal = view.area.contains(mouse_pos);
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
    if matches!(&app.overlay, Some(Overlay::AgentPresets(_))) {
        crate::preset_overlays::handle_list_mouse(app, mouse, mouse_pos);
        return;
    }
    // Hosts picker: the wheel moves the selection, a click on a row connects
    // (the context-menu convention — rows are actions, not editable items),
    // a click outside the modal closes; everything else is swallowed.
    if let Some(Overlay::Hosts(view)) = &mut app.overlay {
        match mouse.kind {
            MouseEventKind::ScrollUp => {
                view.selected = clamp_selection(view.selected as i64 + (-1), view.hosts.len());
                app.dirty = true;
            }
            MouseEventKind::ScrollDown => {
                view.selected = clamp_selection(view.selected as i64 + (1), view.hosts.len());
                app.dirty = true;
            }
            MouseEventKind::Down(MouseButton::Left) => {
                let list = view.list_area;
                let inside_list = list.contains(mouse_pos);
                let inside_modal = view.area.contains(mouse_pos);
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
    // Settings: click a tab to switch, a row to select (or activate it if
    // it was already selected), outside to close; everything else is
    // swallowed. While a hotkey capture is live the mouse is inert — the
    // overlay is waiting for a key, and a stray click shouldn't answer it.
    if matches!(&app.overlay, Some(Overlay::Settings(_))) {
        if let MouseEventKind::Down(MouseButton::Left) = mouse.kind {
            let Some(view) = settings(app) else {
                return;
            };
            if view.capture.is_some() {
                return;
            }
            let (area, tab, selected, body, first_row) = (
                view.area,
                view.tab,
                view.selected,
                view.body_area,
                view.first_row,
            );
            let tab_hits = view.tab_hits.clone();
            let inside = area.contains(mouse_pos);
            if !inside {
                close_settings(app);
                app.dirty = true;
                return;
            }
            // The strip first: its labels are recorded during draw.
            if let Some(next) = tab_hits
                .iter()
                .position(|(x0, x1)| mouse.column >= *x0 && mouse.column < *x1)
            {
                if mouse.row == area.y.saturating_add(1) {
                    app.settings_tab = next;
                    let row = app.settings_row(next);
                    app.remember_settings_focus(false);
                    if let Some(view) = settings_mut(app) {
                        view.tab = next;
                        view.selected = row;
                        view.on_tabs = false;
                        view.notice = None;
                    }
                    app.dirty = true;
                    return;
                }
            }
            if body.height > 0 && mouse.row >= body.y && mouse.row < body.y + body.height {
                let row = first_row + (mouse.row - body.y) as usize;
                // Group headers and blanks aren't clickable; the shared
                // row map keeps this in step with the renderer.
                if let Some(index) = crate::config::settings_rows(tab)
                    .get(row)
                    .and_then(|r| r.index())
                {
                    if let Some(view) = settings_mut(app) {
                        view.selected = index;
                        view.on_tabs = false;
                        view.notice = None;
                    }
                    app.remember_settings_row(tab, index);
                    app.remember_settings_focus(false);
                    if selected == index {
                        if tab == crate::config::hotkeys_tab() {
                            // Second click on a hotkey row starts a rebind,
                            // the same as Enter would.
                            if let Some(view) = settings_mut(app) {
                                view.capture = Some(crate::app::HotkeyCapture {
                                    action: index,
                                    add: false,
                                    pending: None,
                                });
                            }
                        } else {
                            apply_setting_at(app, tab, index, 0);
                        }
                    }
                }
            }
            app.dirty = true;
        }
        return;
    }
    // Metrics: the wheel moves the selection, a click on a row selects it
    // (a click on the selected row opens it), a click outside closes;
    // everything else is swallowed.
    if let Some(Overlay::Metrics(view)) = &mut app.overlay {
        let mut open: Option<SessionRef> = None;
        match mouse.kind {
            MouseEventKind::ScrollUp => {
                view.selected = clamp_selection(view.selected as i64 + (-1), view.rows.len());
                app.dirty = true;
            }
            MouseEventKind::ScrollDown => {
                view.selected = clamp_selection(view.selected as i64 + (1), view.rows.len());
                app.dirty = true;
            }
            MouseEventKind::Down(MouseButton::Left) => {
                let list = view.list_area;
                let inside_list = list.contains(mouse_pos);
                let inside_modal = view.area.contains(mouse_pos);
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
                // A workspace row opens that workspace here, as ↑/↓ in the
                // column do; the footer's nameplate opens the switcher.
                Some(HitTarget::Workspace(i)) => {
                    if let Some(id) = app.tree.workspaces.get(i).map(|w| w.id.clone()) {
                        switch_workspace(app, id, out);
                    }
                    enter_workspaces_bar(app);
                }
                Some(HitTarget::FooterWorkspace) => open_workspace_picker(app),
                Some(HitTarget::Project(i)) => {
                    if app.sel_project != i {
                        select_project_row(app, i, out);
                    }
                    app.focus = app.first_sidebar_focus();
                }
                Some(HitTarget::Worktree(i)) => {
                    if app.sel_worktree != i {
                        select_worktree_row(app, i, out);
                    }
                    app.focus = Focus::Worktrees;
                    // A second click on a pull request opens it — the same
                    // double-click-to-activate the Sessions panel's link
                    // rows use, so one stray click never launches a browser.
                    // Landing anywhere else breaks the chain, or a click
                    // away and back would read as a double-click.
                    match app.selected_worktree_pr().map(|pr| pr.url.clone()) {
                        Some(url) => {
                            let key = RowKey::Link(url.clone());
                            if is_double_click(&mut app.last_session_click, key) {
                                open_link(app, &url, out);
                            }
                        }
                        None => app.last_session_click = None,
                    }
                }
                Some(HitTarget::Session(i)) => {
                    app.sel_session = i;
                    match app.selected_session_row() {
                        Some(row) if row.is_archived_agent() => {
                            app.focus = Focus::Sessions;
                            app.flash = Some(AGENT_ARCHIVED.into());
                        }
                        Some(row) => {
                            // Double-click attaches (a link row opens in the
                            // browser).
                            if is_double_click(&mut app.last_session_click, row.click_key()) {
                                attach_selected(app, out);
                            } else {
                                // Single click selects the row and previews its
                                // terminal (no focus/lock); Enter or a second
                                // click commits.
                                app.focus = Focus::Sessions;
                                preview_selected_now(app, out);
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
                        if is_double_click(&mut app.last_term_click, cell) {
                            // Double-click: select (and copy) the word under
                            // the cursor.
                            select_word_at(app, cell);
                        } else {
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
            // The Worktrees column scrolls the same way: a project with a
            // long open-PR list outgrows the panel just as readily.
            let over_worktrees = !app.collapsed
                && matches!(
                    over,
                    Some(HitTarget::Worktree(_) | HitTarget::PanelBg(Focus::Worktrees))
                );
            let in_term = matches!(over, Some(HitTarget::TerminalPane)) || app.collapsed;
            if over_worktrees {
                app.worktrees_scroll = if up {
                    app.worktrees_scroll.saturating_sub(SESSIONS_WHEEL_STEP)
                } else {
                    app.worktrees_scroll.saturating_add(SESSIONS_WHEEL_STEP)
                };
                app.dirty = true;
            } else if over_sessions {
                app.sessions_scroll = if up {
                    app.sessions_scroll.saturating_sub(SESSIONS_WHEEL_STEP)
                } else {
                    app.sessions_scroll.saturating_add(SESSIONS_WHEEL_STEP)
                };
                app.dirty = true;
            } else if in_term && app.previewed_pr().is_some() {
                // The pane is showing a pull request, not a session: the
                // wheel reads it rather than reaching the PTY underneath.
                let max = app.pr_preview_max_scroll();
                app.pr_preview_scroll = if up {
                    app.pr_preview_scroll.saturating_sub(PR_PREVIEW_WHEEL_STEP)
                } else {
                    app.pr_preview_scroll
                        .saturating_add(PR_PREVIEW_WHEEL_STEP)
                        .min(max)
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
                        // less, htop with mouse off) expect arrows, one per
                        // line the notch would have scrolled.
                        let arrow: &[u8] = if up { b"\x1b[A" } else { b"\x1b[B" };
                        out.push(ClientRequest::Input {
                            session: term.sref.clone(),
                            data: arrow.repeat(TERM_WHEEL_LINES),
                        });
                    } else {
                        let new_scroll = if up {
                            term.scroll.saturating_add(TERM_WHEEL_LINES)
                        } else {
                            term.scroll.saturating_sub(TERM_WHEEL_LINES)
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
                Some(HitTarget::Workspace(i)) => {
                    if let Some(id) = app.tree.workspaces.get(i).map(|w| w.id.clone()) {
                        switch_workspace(app, id, out);
                    }
                    enter_workspaces_bar(app);
                    let items = workspace_menu(app);
                    open_menu(app, items, at);
                }
                Some(HitTarget::Project(i)) => {
                    app.sel_project = i;
                    app.focus = Focus::Projects;
                    if let Some(p) = app.selected_project() {
                        let items = vec![
                            MenuItem::new("New worktree", MenuAction::NewWorktree(p.id.clone())),
                            MenuItem::new("Add project", MenuAction::AddProject),
                            MenuItem::new("Rename", MenuAction::RenameProject(p.id.clone())),
                            MenuItem::destructive(
                                "Remove from list",
                                MenuAction::RemoveProject(p.id.clone()),
                            ),
                        ];
                        open_menu(app, items, at);
                    }
                }
                Some(HitTarget::Worktree(i)) => {
                    app.sel_worktree = i;
                    app.sel_session = 0;
                    app.focus = Focus::Worktrees;
                    if let Some(pr) = app.selected_worktree_pr().cloned() {
                        let mut items = Vec::new();
                        if let Some(worktree) =
                            selected_project_main_worktree(app).filter(|_| claude_enabled())
                        {
                            items.push(pr_agent_menu_item(worktree, &pr, "New Claude session"));
                        }
                        items.extend([
                            MenuItem::new("Open in browser", MenuAction::OpenLink(pr.url)),
                            MenuItem::new("View diff", MenuAction::ViewPrDiff),
                        ]);
                        open_menu(app, items, at);
                    } else if let Some(w) = app.selected_worktree() {
                        let mut items = vec![
                            MenuItem::new("New agent", MenuAction::NewAgent(w.id.clone())),
                            MenuItem::new("New terminal", MenuAction::NewTerminal(w.id.clone())),
                        ];
                        if !w.is_main {
                            items.push(MenuItem::destructive(
                                "Delete worktree",
                                MenuAction::DeleteWorktree(w.id.clone()),
                            ));
                        }
                        open_menu(app, items, at);
                    }
                }
                Some(HitTarget::Session(i)) => {
                    app.sel_session = i;
                    app.focus = Focus::Sessions;
                    match app.selected_session_row() {
                        Some(SessionRow::Agent(a)) => {
                            open_menu(app, menu_items_for_session(&a), at)
                        }
                        Some(SessionRow::Terminal(t)) => {
                            open_menu(app, menu_items_for_terminal(&t), at)
                        }
                        Some(SessionRow::Link(l)) => open_menu(app, menu_items_for_link(&l), at),
                        None => {}
                    }
                }
                Some(HitTarget::PanelBg(focus)) => {
                    app.focus = focus;
                    let items = match focus {
                        Focus::Workspaces => {
                            vec![MenuItem::new("New workspace", MenuAction::NewWorkspace)]
                        }
                        Focus::Projects => {
                            vec![MenuItem::new("Add project", MenuAction::AddProject)]
                        }
                        Focus::Worktrees => app
                            .selected_project()
                            .map(|p| {
                                vec![MenuItem::new(
                                    "New worktree",
                                    MenuAction::NewWorktree(p.id.clone()),
                                )]
                            })
                            .unwrap_or_default(),
                        Focus::Sessions => app
                            .selected_worktree()
                            .map(|w| {
                                vec![
                                    MenuItem::new("New agent", MenuAction::NewAgent(w.id.clone())),
                                    MenuItem::new("Show/hide archived", MenuAction::ToggleArchived),
                                ]
                            })
                            .unwrap_or_default(),
                        Focus::Terminal => vec![],
                    };
                    open_menu(app, items, at);
                }
                _ => {}
            }
            app.dirty = true;
        }
        _ => {}
    }
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
            links,
            pr_seen,
            ui_state,
        } => {
            app.tree.workspaces = workspaces;
            app.tree.active_workspace = active_workspace;
            app.tree.projects = projects;
            app.tree.worktrees = worktrees;
            app.tree.agents = agents;
            app.tree.terminals = terminals;
            app.tree.links = links;
            app.pr_seen = pr_seen.into_iter().map(|s| (s.url, s.marker)).collect();
            // `--workspace <name>` overrides the daemon's last-opened one.
            // Before the UI-state restore, whose remembered project only
            // resolves against the workspace actually on screen.
            apply_startup_workspace(app, out);
            let session_restored = ui_state
                .as_deref()
                .is_some_and(|json| restore_ui_state(app, json));
            clamp_selections(app);
            refresh_palette(app);
            // Boot the restored worktree's sessions right away — the first
            // thing the user does after launch is walk into one of them.
            schedule_prewarm(app);
            // The cursor came back on the session the user left on; bring
            // its terminal back with it, exactly as landing on the row would.
            // No debounce: a boot restores one remembered session once, so
            // there is no cursor sweep to wait out — only the user waiting
            // to see the screen they left.
            if session_restored {
                preview_selected_now(app, out);
            }
            app.dirty = true;
        }
        ServerEvent::Scrollback { session, data, .. } => {
            if let Some(term) = &mut app.term {
                if term.sref == session {
                    // Full replay: the screen is rebuilt from scratch.
                    app.term_selection = None;
                    term.reset();
                    term.painted = !data.is_empty();
                    term.parser.process(&data);
                    app.dirty = true;
                }
            }
        }
        ServerEvent::Output { session, data, .. } => {
            if let Some(term) = &mut app.term {
                if term.sref == session {
                    term.painted |= !data.is_empty();
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
            unseen,
        } => {
            // A status flip reorders the sessions list — and, since
            // worktrees and projects sort on their sessions' stamps,
            // re-sorts those columns too.
            // Every cursor stays on the row it was on.
            let before = selection_snapshot(app);
            if let Some(a) = app.tree.agents.iter_mut().find(|a| a.id == agent) {
                // The RUNNING / NEEDS FEEDBACK → FINISHED edge — the one
                // that raises UNSEEN — rings the DONE SOUND, whether or not
                // the session is on screen. A re-stamp of a finished row
                // and the startup Snapshot never get here.
                if status == nebula_core::AgentStatus::Finished
                    && matches!(
                        a.status,
                        nebula_core::AgentStatus::Running | nebula_core::AgentStatus::NeedsFeedback
                    )
                {
                    app.pending_ding = true;
                }
                a.status = status;
                a.status_changed_at = changed_at;
                a.unseen = unseen;
                app.dirty = true;
            }
            // A turn that finished in the pane the user is looking at was
            // watched: clear it before it can count anywhere.
            let on_screen = app
                .term
                .as_ref()
                .is_some_and(|t| t.sref == SessionRef::Agent(agent.clone()));
            if unseen && on_screen {
                mark_agent_seen(app, &agent, out);
            }
            // Nothing left any list, so this only re-seats the cursors.
            reconcile_selection_inner(app, before, out);
        }
        ServerEvent::Ack { req_id, created } => {
            match (app.pending.remove(&req_id), created) {
                (
                    Some(
                        PendingIntent::AttachCreated
                        | PendingIntent::AttachCreatedWithCloudRetry { .. },
                    ),
                    Some(id),
                ) => {
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
                        attach_now(app, sref, out);
                        app.focus = Focus::Terminal;
                        app.term_locked = true;
                    }
                }
                (Some(PendingIntent::ReopenPromptOnError { note, .. }), _) => {
                    app.flash = Some(note);
                }
                (Some(PendingIntent::SelectCreatedProject), Some(EntityId::Project(id))) => {
                    // Its upsert usually lands just before this Ack; if not,
                    // stash the id and select once it does.
                    if !select_created_project(app, &id, out) {
                        app.select_project_when_seen = Some(id);
                    }
                }
                (Some(PendingIntent::SelectCreatedWorktree), Some(EntityId::Worktree(id))) => {
                    if !select_worktree_by_id(app, &id, out) {
                        app.select_worktree_when_seen = Some(id);
                    }
                }
                (Some(PendingIntent::OpenCreatedWorkspace), Some(EntityId::Workspace(id))) => {
                    // A workspace created from the WORKSPACE SWITCHER or the
                    // WORKSPACES BAR: show it right away, with the cursor on
                    // the first visible sidebar rather than leave focus on
                    // the bar (or wherever the switcher was opened from).
                    switch_workspace(app, id, out);
                    app.term_locked = false;
                    app.focus = app.first_sidebar_focus();
                }
                _ => {}
            }
            app.dirty = true;
        }
        ServerEvent::EntityUpserted { entity } => {
            let before = selection_snapshot(app);
            apply_upsert(app, entity);
            // Cursors follow the row they were on across re-sorts and
            // re-homes; a row that left its list (archived away,
            // moved elsewhere) hands the cursor — and the terminal pane —
            // to its neighbor.
            reconcile_selection(app, before, out);
            // Fix the selection onto a session we just created — or follow
            // one we just moved into another worktree of this project.
            land_pending_selection(app, out);
            // ...and onto a project we just added.
            if let Some(pid) = app.select_project_when_seen.clone() {
                if select_created_project(app, &pid, out) {
                    app.select_project_when_seen = None;
                }
            }
            // ...and onto a worktree we just created.
            if let Some(wt_id) = app.select_worktree_when_seen.clone() {
                if select_worktree_by_id(app, &wt_id, out) {
                    app.select_worktree_when_seen = None;
                }
            }
            refresh_palette(app);
            refresh_workspace_picker(app);
            app.dirty = true;
        }
        ServerEvent::EntityRemoved { id } => {
            let before = selection_snapshot(app);
            // Where a deleted workspace sat in the WORKSPACES BAR: the
            // reseat below lands on its neighbor, and the row is gone
            // once `apply_removal` runs.
            let removed_tab = match &id {
                nebula_core::EntityId::Workspace(ws) => {
                    app.tree.workspaces.iter().position(|w| &w.id == ws)
                }
                _ => None,
            };
            apply_removal(app, &id);
            // The cursor that was on the removed row now sits on its
            // neighbor — show that neighbor's session/context.
            reconcile_selection(app, before, out);
            // ...and if what went was the workspace we were scoped to, land
            // on the tab to its right (or, from the last tab, its left).
            reseat_deleted_workspace(app, removed_tab, out);
            refresh_palette(app);
            refresh_workspace_picker(app);
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
            // it was an optimistic worktree delete, put the rows back. A
            // failed Cloud launch reopens its populated task editor.
            match req_id.and_then(|id| app.pending.remove(&id)) {
                Some(PendingIntent::DeleteWorktree(rollback)) => {
                    restore_worktree_rows(app, rollback)
                }
                Some(
                    PendingIntent::AttachCreatedWithCloudRetry { kind, task: text }
                    | PendingIntent::ReopenPromptOnError { kind, text, .. },
                ) => {
                    open_prompt(app, kind);
                    if let Some(Overlay::Prompt(prompt)) = &mut app.overlay {
                        prompt.input.set_text(text);
                    }
                }
                _ => {}
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
        Entity::Workspace(w) => upsert_by(&mut app.tree.workspaces, w, |x, y| x.id == y.id),
        Entity::Project(p) => {
            let selected = app.selected_project().map(|p| p.id.clone());
            upsert_by(&mut app.tree.projects, p, |x, y| x.id == y.id);
            // Reorders arrive as plain upserts with new sort_orders; stable
            // sort keeps snapshot order for legacy all-zero ties. The
            // selection follows the project it was on, so children stay put.
            app.tree.projects.sort_by_key(|x| x.sort_order);
            if let Some(id) = selected {
                let found = app
                    .project_rows()
                    .iter()
                    .position(|i| app.tree.projects[*i].id == id);
                if let Some(i) = found {
                    app.sel_project = i;
                }
            }
        }
        Entity::Worktree(w) => upsert_by(&mut app.tree.worktrees, w, |x, y| x.id == y.id),
        Entity::Agent(a) => {
            // A row the daemon re-homed (`nebula worktree`, a hook cwd in
            // another checkout) takes the cursor with it when it was the
            // selected session — otherwise the selection would silently
            // land on whatever row slid into its place.
            let selected = app.selected_session().map(|s| s.id);
            let moved = app
                .tree
                .agents
                .iter()
                .any(|x| x.id == a.id && x.worktree_id != a.worktree_id);
            let id = a.id.clone();
            upsert_by(&mut app.tree.agents, a, |x, y| x.id == y.id);
            if moved && selected.as_ref() == Some(&id) {
                app.select_when_seen = Some(SessionRef::Agent(id));
            }
        }
        Entity::Terminal(t) => upsert_by(&mut app.tree.terminals, t, |x, y| x.id == y.id),
        Entity::Link(l) => upsert_by(&mut app.tree.links, l, |x, y| x.id == y.id),
    }
}

/// Replace the first entry of `list` that `same` pairs with `item`, or
/// append `item` when there is none.
fn upsert_by<T>(list: &mut Vec<T>, item: T, same: impl Fn(&T, &T) -> bool) {
    match list.iter_mut().find(|x| same(x, &item)) {
        Some(existing) => *existing = item,
        None => list.push(item),
    }
}

fn apply_removal(app: &mut App, id: &nebula_core::EntityId) {
    use nebula_core::EntityId;
    match id {
        EntityId::Workspace(id) => {
            // Only empty workspaces get deleted, so no project rows need
            // cleanup here — but this instance may have been scoped to it,
            // which `reseat_deleted_workspace` sorts out afterwards.
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
            app.tree.links.retain(|l| !wt_ids.contains(&l.worktree_id));
            app.pull_requests.retain(|w, _| !wt_ids.contains(w));
            app.pr_recheck.retain(|w, _| !wt_ids.contains(w));
            app.tree.worktrees.retain(|w| &w.project_id != id);
            app.tree.projects.retain(|p| &p.id != id);
        }
        EntityId::Worktree(id) => {
            app.tree.agents.retain(|a| &a.worktree_id != id);
            app.tree.terminals.retain(|t| &t.worktree_id != id);
            app.tree.links.retain(|l| &l.worktree_id != id);
            app.pull_requests.remove(id);
            app.pr_recheck.remove(id);
            app.tree.worktrees.retain(|w| &w.id != id);
        }
        EntityId::Agent(id) => app.tree.agents.retain(|a| &a.id != id),
        EntityId::Terminal(id) => app.tree.terminals.retain(|t| &t.id != id),
        EntityId::Link(id) => app.tree.links.retain(|l| &l.id != id),
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
        palette.rebuild(&app.tree, app.show_archived, &app.open_prs);
    }
}

/// What each panel cursor pointed at, captured with `selection_snapshot`
/// before a tree mutation so `reconcile_selection` can compare afterwards.
struct SelectionSnapshot {
    project: Option<nebula_core::ProjectId>,
    worktree: Option<WorktreeId>,
    session: Option<SessionRef>,
    /// Whether the selected session row was already in the archived group —
    /// following onto an archived row is only right when it was.
    session_archived: bool,
}

fn selection_snapshot(app: &App) -> SelectionSnapshot {
    let row = app.selected_session_row();
    SelectionSnapshot {
        project: app.selected_project().map(|p| p.id.clone()),
        worktree: app.selected_worktree().map(|w| w.id.clone()),
        session_archived: row.as_ref().is_some_and(|r| r.is_archived_agent()),
        session: row.and_then(|r| r.sref()),
    }
}

/// Re-point the panel cursors after the tree changed. Each cursor follows
/// the entity it was on when rows merely shifted; when that entity left its
/// list — deleted, archived away, re-homed — the cursor has landed on a
/// neighbor, and that neighbor gets shown exactly as if the user had moved
/// there (restore_context / restore_session / preview). The invariant: the
/// terminal pane always shows the highlighted session, never a stale or
/// blank one.
/// Re-seat the selection after the tree changed under it, then attach at
/// once. A delete, an archive or a move is an explicit act and the row the
/// cursor gets pushed onto is where it stays — there is no sweep to wait
/// out, unlike the key-walking that `attach`'s debounce exists for.
fn reconcile_selection(app: &mut App, before: SelectionSnapshot, out: &mut Vec<ClientRequest>) {
    reconcile_selection_inner(app, before, out);
    fire_pending_attach(app, out);
}

fn reconcile_selection_inner(
    app: &mut App,
    before: SelectionSnapshot,
    out: &mut Vec<ClientRequest>,
) {
    clamp_selections(app);
    if let Some(pid) = &before.project {
        if !app.tree.projects.iter().any(|p| &p.id == pid) {
            // The selected row's project is gone; the cursor landed on a
            // neighbor — bring up its remembered worktree + session.
            restore_context(app, out);
            return;
        }
        if app.selected_project().map(|p| p.id.clone()).as_ref() != Some(pid) {
            let rows = app.project_rows();
            let found = rows.iter().position(|i| &app.tree.projects[*i].id == pid);
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
        if rows.get(app.sel_session).and_then(|r| r.sref()).as_ref() != Some(sref) {
            let found = rows.iter().position(|r| {
                r.sref().as_ref() == Some(sref)
                    && (before.session_archived || !r.is_archived_agent())
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
    app.sel_project = clamp_selection(app.sel_project as i64, project_rows);
    let wt_len = app.worktree_row_count();
    app.sel_worktree = clamp_selection(app.sel_worktree as i64, wt_len);
    let sess_len = app.visible_session_rows().len();
    app.sel_session = clamp_selection(app.sel_session as i64, sess_len);
}

#[cfg(test)]
mod tests {
    use super::*;
    use nebula_core::{AgentId, LinkId, ServerEvent, SessionRef};
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    /// A second agent, `a2` / "agent-2", under the seeded worktree.
    fn seed_second_agent(app: &mut App, status: nebula_core::AgentStatus) {
        use nebula_core::{Agent, Entity, WorktreeId};
        hse(
            app,
            ServerEvent::EntityUpserted {
                entity: Entity::Agent(Agent {
                    id: AgentId("a2".into()),
                    worktree_id: WorktreeId("w1".into()),
                    name: "agent-2".into(),
                    status,
                    archived: false,
                    archived_at: 0,
                    unseen: false,
                    kind: nebula_core::AgentKind::Claude,
                    model: None,
                    effort: None,
                    session_id: None,
                    cloud_session_id: None,
                    sort_order: 1,
                    status_changed_at: 0,
                    alive: true,
                    cloud_mirroring: false,
                }),
            },
        );
    }

    /// Row index of an agent in the Sessions panel.
    fn row_of(app: &App, id: &str) -> usize {
        let sref = SessionRef::Agent(AgentId(id.into()));
        app.visible_session_rows()
            .iter()
            .position(|r| r.sref().as_ref() == Some(&sref))
            .unwrap_or_else(|| panic!("{id} has no row"))
    }

    /// A session that goes green while the pane shows something else counts
    /// on its worktree and project rows — the number of terminals to go
    /// read — and stays counted until the cursor walks onto it. Landing
    /// previews it, so that is the moment the counts come down: locally on
    /// the spot, and at the daemon for every other client.
    #[test]
    fn an_unwatched_finish_counts_until_the_cursor_lands_on_it() {
        use nebula_core::{AgentStatus, ProjectId, WorktreeId};
        let mut app = App::new();
        seed_tree(&mut app);
        seed_second_agent(&mut app, AgentStatus::Running);
        let a2 = AgentId("a2".into());
        let (w1, p1) = (WorktreeId("w1".into()), ProjectId("p1".into()));
        app.term = Some(AttachedTerm::new(
            SessionRef::Agent(AgentId("a1".into())),
            40,
            10,
        ));
        app.focus = Focus::Sessions;
        app.sel_session = row_of(&app, "a1");

        hse(
            &mut app,
            ServerEvent::StatusChanged {
                agent: a2.clone(),
                status: AgentStatus::Finished,
                changed_at: crate::app::now_ms(),
                unseen: true,
            },
        );
        assert_eq!(app.worktree_unseen(&w1), 1, "one terminal to go read");
        assert_eq!(app.project_unseen(&p1), 1);
        assert_eq!(
            app.sel_session,
            row_of(&app, "a1"),
            "the cursor stayed on the session it was on"
        );

        let mut out = Vec::new();
        let delta = row_of(&app, "a2") as i64 - app.sel_session as i64;
        move_selection(&mut app, delta, &mut out);
        assert_eq!(app.sel_session, row_of(&app, "a2"));
        assert_eq!(app.worktree_unseen(&w1), 0, "landing on the row reads it");
        assert_eq!(app.project_unseen(&p1), 0);
        assert!(
            out.iter()
                .any(|r| matches!(r, ClientRequest::MarkAgentSeen { id } if *id == a2)),
            "the daemon is told: {out:?}"
        );

        // Walking off and back sends nothing: there is nothing left to clear.
        out.clear();
        move_selection(&mut app, -delta, &mut out);
        move_selection(&mut app, delta, &mut out);
        assert!(
            !out.iter()
                .any(|r| matches!(r, ClientRequest::MarkAgentSeen { .. })),
            "{out:?}"
        );
    }

    /// The DONE SOUND rings on the RUNNING / NEEDS FEEDBACK → FINISHED
    /// edge only — not on the way into NEEDS FEEDBACK, not on a re-stamp
    /// of a row already finished — and the flag is one bool, so a frame
    /// with several finishes rings once.
    #[test]
    fn a_finish_rings_the_done_sound_once() {
        use nebula_core::AgentStatus;
        let mut app = App::new();
        seed_tree(&mut app);
        seed_second_agent(&mut app, AgentStatus::Running);
        assert!(!app.pending_ding, "the snapshot is silent");
        let a2 = AgentId("a2".into());
        let flip = |status: AgentStatus| ServerEvent::StatusChanged {
            agent: a2.clone(),
            status,
            changed_at: crate::app::now_ms(),
            unseen: status == AgentStatus::Finished,
        };

        hse(&mut app, flip(AgentStatus::NeedsFeedback));
        assert!(!app.pending_ding, "waiting on the user is not done");
        hse(&mut app, flip(AgentStatus::Finished));
        assert!(app.pending_ding, "NEEDS FEEDBACK -> FINISHED rings");

        app.pending_ding = false;
        hse(&mut app, flip(AgentStatus::Finished));
        assert!(!app.pending_ding, "a re-stamp of a finished row is silent");

        hse(&mut app, flip(AgentStatus::Running));
        assert!(!app.pending_ding);
        // On screen or not makes no difference to the sound.
        app.term = Some(AttachedTerm::new(SessionRef::Agent(a2.clone()), 40, 10));
        hse(&mut app, flip(AgentStatus::Finished));
        assert!(
            app.pending_ding,
            "RUNNING -> FINISHED rings, even on screen"
        );
        hse(&mut app, flip(AgentStatus::Running));
        hse(&mut app, flip(AgentStatus::Finished));
        assert!(
            app.pending_ding,
            "two finishes in a frame are still one ding"
        );
    }

    /// A turn that finishes in the pane the user is already looking at was
    /// watched: the flag is dropped the moment it arrives, so it never
    /// counts anywhere, and the daemon is told so every client agrees.
    #[test]
    fn a_finish_in_the_pane_on_screen_is_already_seen() {
        use nebula_core::{AgentStatus, WorktreeId};
        let mut app = App::new();
        seed_tree(&mut app);
        seed_second_agent(&mut app, AgentStatus::Running);
        let a2 = AgentId("a2".into());
        app.term = Some(AttachedTerm::new(SessionRef::Agent(a2.clone()), 40, 10));

        let mut out = Vec::new();
        handle_server_event(
            &mut app,
            ServerEvent::StatusChanged {
                agent: a2.clone(),
                status: AgentStatus::Finished,
                changed_at: crate::app::now_ms(),
                unseen: true,
            },
            &mut out,
        );
        assert_eq!(app.worktree_unseen(&WorktreeId("w1".into())), 0);
        assert!(
            out.iter()
                .any(|r| matches!(r, ClientRequest::MarkAgentSeen { id } if *id == a2)),
            "{out:?}"
        );
    }

    /// The badges: project and worktree rows count their unwatched finishes
    /// as ` n done`, and the session rows being counted say `done` in the
    /// harness slot — all of it gone once the session has been read. Dot
    /// and count share the `done` violet while the turn is unread; landing
    /// the cursor on the session drops the dot to the plain-success green,
    /// which is the whole distinction: violet is a job, green is a result.
    #[test]
    fn unwatched_finishes_badge_the_rows_until_read() {
        use nebula_core::AgentStatus;
        let mut app = App::new();
        seed_tree(&mut app);
        seed_second_agent(&mut app, AgentStatus::Running);
        let a2 = AgentId("a2".into());
        hse(
            &mut app,
            ServerEvent::StatusChanged {
                agent: a2.clone(),
                status: AgentStatus::Finished,
                changed_at: crate::app::now_ms(),
                unseen: true,
            },
        );
        let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
        terminal.draw(|f| ui::draw(f, &mut app)).unwrap();
        let text = buffer_text(&terminal);
        assert!(
            text.contains("demo 1 done"),
            "project row counts it: {text}"
        );
        // `main ⌂ 1 done`, not `main ⌂ root 1 done`: in a twenty-cell
        // column the root badge keeps its glyph and yields the word rather
        // than ellipsize the branch — and the "just now" label, which would
        // squeeze the name under MIN_NAME_W, drops out entirely.
        assert!(
            text.contains("main ⌂ 1 done"),
            "worktree row counts it: {text}"
        );
        let row = text.lines().find(|l| l.contains("agent-2")).unwrap();
        let tail = &row[row.find("agent-2").unwrap()..];
        assert!(tail.contains(" done"), "the session row says so: {row}");
        assert!(
            !tail.contains("claude"),
            "the harness slot is taken over: {row}"
        );
        {
            // Unread: the dot wears `done`, not the success green — and
            // the rows above it roll that up, so they're violet too.
            let (x, y) = find_cell(&terminal, "agent-2");
            let (px, py) = find_cell(&terminal, "demo");
            let buffer = terminal.backend().buffer();
            let dot = &buffer[(x - 2, y)];
            assert_eq!(dot.symbol(), "●", "{text}");
            assert_eq!(dot.fg, app.theme.done, "unread done dot:\n{text}");
            assert_ne!(
                app.theme.done, app.theme.ok,
                "and it isn't the success green"
            );
            assert_eq!(
                buffer[(px - 2, py)].fg,
                app.theme.done,
                "project dot:\n{text}"
            );
        }

        let mut out = Vec::new();
        mark_agent_seen(&mut app, &a2, &mut out);
        terminal.draw(|f| ui::draw(f, &mut app)).unwrap();
        let text = buffer_text(&terminal);
        assert!(
            !text.contains("1 done"),
            "read: the counts are gone: {text}"
        );
        let row = text.lines().find(|l| l.contains("agent-2")).unwrap();
        let tail = &row[row.find("agent-2").unwrap()..];
        assert!(tail.contains("claude"), "the harness is back: {row}");
        // …and the dots go green: still finished, no longer a job.
        let (x, y) = find_cell(&terminal, "agent-2");
        let (px, py) = find_cell(&terminal, "demo");
        let buffer = terminal.backend().buffer();
        assert_eq!(
            buffer[(x - 2, y)].fg,
            app.theme.ok,
            "read done dot:\n{text}"
        );
        assert_eq!(
            buffer[(px - 2, py)].fg,
            app.theme.ok,
            "project dot:\n{text}"
        );
    }

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
                    archived_at: 0,
                    unseen: false,
                    kind: nebula_core::AgentKind::Claude,
                    model: None,
                    effort: None,
                    session_id: None,
                    cloud_session_id: None,
                    sort_order: 0,
                    status_changed_at: 0,
                    alive: true,
                    cloud_mirroring: false,
                }),
            },
        );
    }

    /// Turn the seeded row into a Claude Cloud row, mirroring or not.
    fn make_cloud_row(app: &mut App, mirroring: bool) {
        let mut agent = app.tree.agents[0].clone();
        agent.cloud_session_id = Some("session_01SQugK2HDyk33coSrfqFJk4".into());
        agent.cloud_mirroring = mirroring;
        hse(
            app,
            ServerEvent::EntityUpserted {
                entity: nebula_core::Entity::Agent(agent),
            },
        );
    }

    /// A Cloud row can be steered from nebula: its menu offers a message to
    /// queue on the session, in the same multi-row editor the launch task
    /// uses, and a failed send hands the text back rather than eating it.
    #[test]
    fn cloud_row_can_send_a_message_to_its_session() {
        let mut app = App::new();
        seed_tree(&mut app);
        make_cloud_row(&mut app, true);
        app.focus = Focus::Sessions;
        let mut out = Vec::new();

        press(&mut app, KeyCode::Char('m'), KeyModifiers::NONE, &mut out);
        let Some(Overlay::Menu(menu)) = &app.overlay else {
            panic!("no menu: {:?}", app.overlay)
        };
        let labels: Vec<&str> = menu.items.iter().map(|i| i.label.as_str()).collect();
        assert!(
            labels.contains(&"Attach cloud session") && labels.contains(&"Send to cloud session"),
            "cloud rows get both cloud verbs: {labels:?}"
        );

        let idx = menu
            .items
            .iter()
            .position(|i| i.label == "Send to cloud session")
            .unwrap();
        let Some(Overlay::Menu(menu)) = &mut app.overlay else {
            unreachable!()
        };
        menu.hover = idx;
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE, &mut out);
        let Some(Overlay::Prompt(prompt)) = &mut app.overlay else {
            panic!("no message prompt: {:?}", app.overlay)
        };
        assert!(
            prompt.is_multiline(),
            "steering a cloud agent is rarely one line"
        );
        prompt.input.set_text("also update the README");
        out.clear();
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE, &mut out);
        let req_id = match &out[..] {
            [ClientRequest::SendCloudMessage {
                req_id,
                id,
                message,
            }] => {
                assert_eq!(id.0, "a1");
                assert_eq!(message, "also update the README");
                *req_id
            }
            other => panic!("expected a cloud send: {other:?}"),
        };
        assert!(app.overlay.is_none(), "the prompt closes on submit");

        // A failed send reopens the editor with the message intact.
        hse(
            &mut app,
            ServerEvent::Error {
                req_id: Some(req_id),
                message: "claude could not reach the cloud session".into(),
            },
        );
        let Some(Overlay::Prompt(prompt)) = &app.overlay else {
            panic!("a lost message should come back: {:?}", app.overlay)
        };
        assert_eq!(prompt.input.as_str(), "also update the README");
    }

    /// The badge says whether the pane is being kept current. A row that
    /// changes on its own with no explanation reads as a glitch.
    #[test]
    fn cloud_badge_says_when_the_row_is_following() {
        let mut app = App::new();
        seed_tree(&mut app);
        make_cloud_row(&mut app, true);
        let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
        terminal.draw(|f| ui::draw(f, &mut app)).unwrap();
        assert!(
            buffer_text(&terminal).contains("cloud ↻"),
            "a following row says so: {}",
            buffer_text(&terminal)
        );

        make_cloud_row(&mut app, false);
        terminal.draw(|f| ui::draw(f, &mut app)).unwrap();
        let text = buffer_text(&terminal);
        assert!(text.contains("cloud"), "still a cloud row: {text}");
        assert!(
            !text.contains("cloud ↻"),
            "the follow ended; stop promising refreshes: {text}"
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
        // Wide enough that the panel hints reach `?: help` unclipped —
        // the version nameplate on the far left costs ~18 columns.
        let mut terminal = Terminal::new(TestBackend::new(160, 30)).unwrap();
        terminal.draw(|f| ui::draw(f, &mut app)).unwrap();
        let text = buffer_text(&terminal);
        assert!(text.contains("n/o: add project"), "{text}");
        assert!(text.contains("w: workspaces"), "{text}");
        assert!(text.contains("q: quit"), "{text}");
        for dead in ["d: remove", "m: menu", "/: search"] {
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

    // ---- worktree links ----

    /// `seed_tree` plus one saved link on w1, cursor parked on it.
    fn seed_link(app: &mut App, url: &str) {
        hse(
            app,
            ServerEvent::EntityUpserted {
                entity: nebula_core::Entity::Link(nebula_core::Link {
                    id: LinkId("l1".into()),
                    worktree_id: nebula_core::WorktreeId("w1".into()),
                    url: url.into(),
                    sort_order: 0,
                }),
            },
        );
        app.focus = Focus::Sessions;
        app.sel_session = app
            .visible_session_rows()
            .iter()
            .position(|r| r.as_link().is_some())
            .expect("link row");
    }

    /// `h`/`l` are the vim twins of `←`/`→`: same focus walk, same stops
    /// at the ends — and a double tap at an end jumps the boundary the way
    /// ^⇧H / ^⇧L would: `l`,`l` at Sessions goes on into the pane, `h`,`h`
    /// at Projects steps up into the Workspaces bar. The plain letters used
    /// to open the hosts picker and the add-link prompt. The hosts picker
    /// moved to Shift+H; manual LINK creation is no longer exposed.
    #[test]
    fn h_and_l_walk_panel_focus_like_the_arrows() {
        let mut app = App::new();
        let mut out = Vec::new();
        let l = |app: &mut App, out: &mut Vec<ClientRequest>| {
            press(app, KeyCode::Char('l'), KeyModifiers::NONE, out)
        };
        let h = |app: &mut App, out: &mut Vec<ClientRequest>| {
            press(app, KeyCode::Char('h'), KeyModifiers::NONE, out)
        };

        app.focus = Focus::Projects;
        l(&mut app, &mut out);
        assert_eq!(app.focus, Focus::Worktrees);
        l(&mut app, &mut out);
        assert_eq!(app.focus, Focus::Sessions);
        l(&mut app, &mut out);
        assert_eq!(
            app.focus,
            Focus::Sessions,
            "a single l at sessions stays, as → does"
        );
        assert_eq!(
            app.flash.as_deref(),
            Some("l again: enter pane"),
            "and the footer says what a second one does"
        );
        l(&mut app, &mut out);
        assert_eq!(
            app.focus,
            Focus::Terminal,
            "l,l at sessions jumps on into the pane, as ^⇧L does"
        );
        assert!(!app.term_locked, "an empty pane is focused, never locked");
        l(&mut app, &mut out);
        l(&mut app, &mut out);
        assert_eq!(app.focus, Focus::Terminal, "and the walk stops there");
        assert!(
            app.overlay.is_none(),
            "l no longer opens the add-link prompt"
        );

        h(&mut app, &mut out);
        assert_eq!(app.focus, Focus::Sessions);
        h(&mut app, &mut out);
        assert_eq!(app.focus, Focus::Worktrees);
        h(&mut app, &mut out);
        assert_eq!(app.focus, Focus::Projects);
        h(&mut app, &mut out);
        assert_eq!(
            app.focus,
            Focus::Projects,
            "a single h at projects stays — the bar takes a double tap"
        );
        assert_eq!(app.flash.as_deref(), Some("h again: workspaces"));
        h(&mut app, &mut out);
        assert_eq!(
            app.focus,
            Focus::Workspaces,
            "h,h at projects steps up into the bar, as ^⇧H does"
        );
        h(&mut app, &mut out);
        assert_eq!(
            app.focus,
            Focus::Workspaces,
            "in the bar, h walks the tabs and stays put"
        );
        assert!(app.overlay.is_none(), "h no longer opens the hosts picker");

        app.show_workspaces = false;
        app.focus = Focus::Projects;
        app.flash = None;
        h(&mut app, &mut out);
        h(&mut app, &mut out);
        assert_eq!(
            app.focus,
            Focus::Projects,
            "bar hidden: projects is the first column, so h,h stops there too"
        );
        assert!(
            app.edge_tap.is_none(),
            "nothing above to jump to, so nothing arms"
        );
        assert!(app.flash.is_none(), "and no hint promises one");
    }

    /// The double tap is a gesture, not a state: a second press that comes
    /// too late, or after any other key, is a fresh single press.
    #[test]
    fn a_slow_or_interrupted_second_tap_at_the_edge_stays_put() {
        let mut app = App::new();
        let mut out = Vec::new();
        let l = |app: &mut App, out: &mut Vec<ClientRequest>| {
            press(app, KeyCode::Char('l'), KeyModifiers::NONE, out)
        };

        app.focus = Focus::Sessions;
        l(&mut app, &mut out);
        let (armed, _) = app.edge_tap.expect("the first press arms");
        assert_eq!(armed, crate::keymap::Action::FocusRight);
        app.edge_tap = Some((
            armed,
            std::time::Instant::now() - focus_walk::DOUBLE_TAP - Duration::from_millis(100),
        ));
        l(&mut app, &mut out);
        assert_eq!(app.focus, Focus::Sessions, "too slow: two single presses");
        assert!(app.edge_tap.is_some(), "but the late one arms again");

        press(&mut app, KeyCode::Char('j'), KeyModifiers::NONE, &mut out);
        assert!(app.edge_tap.is_none(), "any other key breaks the pair");
        l(&mut app, &mut out);
        assert_eq!(app.focus, Focus::Sessions, "so this is a first press again");
        l(&mut app, &mut out);
        assert_eq!(app.focus, Focus::Terminal, "and this one completes it");
    }

    /// k/↑ on a panel's first row is the top edge, and j/↓ in the bar the
    /// way back down: both are double taps, like h/l at the ends of the
    /// row. k,k steps up into the Workspaces bar (only while it's shown);
    /// j,j drops back onto the panel the cursor came up from — by k,k,
    /// h,h, ⇧Tab or a click — with its row untouched, and onto Projects
    /// when it never came up at all.
    #[test]
    fn k_k_at_the_top_row_steps_into_the_bar_and_j_j_drops_back_where_it_came_from() {
        use nebula_core::Entity;
        let mut app = App::new();
        seed_tree(&mut app);
        let mut second = app.tree.agents[0].clone();
        second.id = AgentId("a2".into());
        second.name = "agent-2".into();
        hse(
            &mut app,
            ServerEvent::EntityUpserted {
                entity: Entity::Agent(second),
            },
        );
        let mut out = Vec::new();
        let k = |app: &mut App, out: &mut Vec<ClientRequest>| {
            press(app, KeyCode::Char('k'), KeyModifiers::NONE, out)
        };
        let j = |app: &mut App, out: &mut Vec<ClientRequest>| {
            press(app, KeyCode::Char('j'), KeyModifiers::NONE, out)
        };

        // Never been up: j,j from the bar lands on Projects.
        app.focus = Focus::Workspaces;
        j(&mut app, &mut out);
        assert_eq!(
            app.focus,
            Focus::Workspaces,
            "a single j in the bar stays put"
        );
        assert_eq!(app.flash.as_deref(), Some("j again: back to projects"));
        j(&mut app, &mut out);
        assert_eq!(
            app.focus,
            Focus::Projects,
            "j,j drops out of the bar — onto Projects until it has come up from somewhere"
        );

        // Above the first row k is a plain move: no arm, no hint. (The
        // flash is cleared per terminal event, not per `handle_key`, so
        // the test clears the last hint itself.)
        app.focus = Focus::Sessions;
        app.sel_session = 1;
        app.flash = None;
        k(&mut app, &mut out);
        assert_eq!(app.sel_session, 0, "k above the top row moves the cursor");
        assert!(app.edge_tap.is_none(), "and arms nothing");
        assert!(app.flash.is_none(), "and promises nothing");

        k(&mut app, &mut out);
        assert_eq!(
            app.focus,
            Focus::Sessions,
            "a single k on the first row stays put"
        );
        assert_eq!(app.sel_session, 0);
        assert_eq!(app.flash.as_deref(), Some("k again: workspaces"));
        k(&mut app, &mut out);
        assert_eq!(app.focus, Focus::Workspaces, "k,k steps up into the bar");

        j(&mut app, &mut out);
        assert_eq!(app.focus, Focus::Workspaces);
        assert_eq!(
            app.flash.as_deref(),
            Some("j again: back to sessions"),
            "the hint names the panel it came up from"
        );
        j(&mut app, &mut out);
        assert_eq!(
            app.focus,
            Focus::Sessions,
            "j,j lands back on the panel k,k left"
        );
        assert_eq!(app.sel_session, 0, "with its cursor where it was");

        // The walk back and h,h remember where they stepped up from too.
        press(&mut app, KeyCode::Char('h'), KeyModifiers::NONE, &mut out);
        press(&mut app, KeyCode::Char('h'), KeyModifiers::NONE, &mut out);
        assert_eq!(app.focus, Focus::Projects);
        press(&mut app, KeyCode::BackTab, KeyModifiers::NONE, &mut out);
        assert_eq!(app.focus, Focus::Workspaces);
        j(&mut app, &mut out);
        j(&mut app, &mut out);
        assert_eq!(
            app.focus,
            Focus::Projects,
            "⇧Tab came up from Projects, so j,j returns there"
        );

        // The pane isn't a panel under the bar: from there, back lands on
        // Sessions, the column the pane previews.
        app.focus = Focus::Terminal;
        enter_workspaces_bar(&mut app);
        assert_eq!(app.focus, Focus::Workspaces);
        leave_workspaces_bar(&mut app);
        assert_eq!(app.focus, Focus::Sessions);

        // Bar hidden: nothing above to jump to, so k,k on the top row is
        // two plain no-ops — no arm, no hint.
        app.show_workspaces = false;
        app.focus = Focus::Sessions;
        app.sel_session = 0;
        app.flash = None;
        k(&mut app, &mut out);
        k(&mut app, &mut out);
        assert_eq!(
            app.focus,
            Focus::Sessions,
            "bar hidden: k,k stays on the top row"
        );
        assert!(
            app.edge_tap.is_none(),
            "nothing above to jump to, so nothing arms"
        );
        assert!(app.flash.is_none(), "and no hint promises one");
    }

    /// ^⇧L / ^⇧H are Tab / ⇧Tab under another name, in the one modifier
    /// space Ghostty leaves alone (it claims only ctrl+shift+tab) — unlike
    /// ⌘, which every macOS emulator either swallows or binds itself.
    /// Needs the kitty protocol, so Ghostty/kitty yes, Terminal.app never.
    /// Forward stops dead at the terminal pane; back stops dead at the
    /// first column — the workspaces bar when shown, Projects when hidden.
    /// Neither direction wraps. Landing on a live pane forward takes input.
    #[test]
    fn ctrl_shift_hl_walk_stops_at_the_pane_forward_and_the_first_column_back() {
        let mut app = App::new();
        seed_tree(&mut app);
        let mut out = Vec::new();
        let cs = KeyModifiers::CONTROL | KeyModifiers::SHIFT;
        let fwd = move |app: &mut App, out: &mut Vec<ClientRequest>| {
            press(app, KeyCode::Char('L'), cs, out)
        };
        let back = move |app: &mut App, out: &mut Vec<ClientRequest>| {
            press(app, KeyCode::Char('H'), cs, out)
        };

        // Nothing in the pane yet, so the far end is reachable twice over
        // without the input lock swallowing the second press.
        assert!(app.term.is_none());
        app.focus = Focus::Projects;
        fwd(&mut app, &mut out);
        assert_eq!(app.focus, Focus::Worktrees);
        fwd(&mut app, &mut out);
        assert_eq!(app.focus, Focus::Sessions);
        fwd(&mut app, &mut out);
        assert_eq!(app.focus, Focus::Terminal);
        fwd(&mut app, &mut out);
        assert_eq!(app.focus, Focus::Terminal, "stops at the pane, no wrap");
        assert!(!app.term_locked, "an empty pane is focused, never locked");

        back(&mut app, &mut out);
        assert_eq!(app.focus, Focus::Sessions);
        back(&mut app, &mut out);
        assert_eq!(app.focus, Focus::Worktrees);
        back(&mut app, &mut out);
        assert_eq!(app.focus, Focus::Projects);
        back(&mut app, &mut out);
        assert_eq!(app.focus, Focus::Workspaces);
        back(&mut app, &mut out);
        assert_eq!(app.focus, Focus::Workspaces, "stops at the bar, no wrap");
        assert!(app.overlay.is_none(), "no overlay claims ^⇧H / ^⇧L");

        // With a live session in the pane, arriving there — by either
        // walk — hands it the keyboard: the whole point is to end up
        // typing at the agent. ^⇧H is then the hatch back out.
        app.term = Some(AttachedTerm::new(
            SessionRef::Agent(AgentId("a1".into())),
            80,
            24,
        ));
        app.focus = Focus::Sessions;
        fwd(&mut app, &mut out);
        assert_eq!(app.focus, Focus::Terminal);
        assert!(app.term_locked, "landing on a live pane takes input");
        back(&mut app, &mut out);
        assert_eq!(app.focus, Focus::Sessions, "^⇧H is the hatch out");
        assert!(!app.term_locked);

        // The loop this stop exists to kill: with the pane live, wrapping
        // off the bar landed in it locked, where ^⇧H is the unlock hatch —
        // so the key walked bar → pane → Sessions → … → bar forever.
        // Leaning on it at the top must now sit still and keep the pane
        // untouched.
        app.focus = Focus::Workspaces;
        for _ in 0..4 {
            back(&mut app, &mut out);
            assert_eq!(app.focus, Focus::Workspaces, "^⇧H cannot cycle off the bar");
            assert!(!app.term_locked, "and never steals the pane's input lock");
        }

        // Bar toggled away: Projects is the first column, so the walk
        // back stops there too and the same lean can't fall into the pane.
        app.show_workspaces = false;
        app.focus = Focus::Projects;
        for _ in 0..4 {
            back(&mut app, &mut out);
            assert_eq!(
                app.focus,
                Focus::Projects,
                "bar hidden: ^⇧H stops at the projects list"
            );
            assert!(!app.term_locked, "and never grabs the pane's input lock");
        }
    }

    #[test]
    fn shift_l_no_longer_opens_manual_link_creation() {
        let mut app = App::new();
        seed_tree(&mut app);
        app.focus = Focus::Sessions;
        let mut out = Vec::new();

        press(&mut app, KeyCode::Char('L'), KeyModifiers::SHIFT, &mut out);

        assert!(app.overlay.is_none(), "manual LINK creation stays closed");
        assert!(
            out.is_empty(),
            "manual LINK creation sends no request: {out:?}"
        );
    }

    #[test]
    fn enter_on_a_link_opens_the_browser_instead_of_attaching() {
        let mut app = App::new();
        seed_tree(&mut app);
        seed_link(&mut app, "https://example.dev/spec");
        let mut out = Vec::new();
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE, &mut out);
        assert!(
            !out.iter()
                .any(|r| matches!(r, ClientRequest::Attach { .. })),
            "a link row has no session to attach: {out:?}"
        );
        assert_eq!(app.focus, Focus::Sessions, "focus stays in the panel");
        assert!(!app.term_locked);
        assert_eq!(app.flash.as_deref(), Some("opened example.dev/spec"));
    }

    #[test]
    fn r_edits_a_link_and_d_deletes_it() {
        let mut app = App::new();
        seed_tree(&mut app);
        seed_link(&mut app, "https://example.dev/spec");
        let mut out = Vec::new();

        press(&mut app, KeyCode::Char('r'), KeyModifiers::NONE, &mut out);
        let Some(Overlay::Prompt(p)) = &app.overlay else {
            panic!("expected the edit-link prompt, got {:?}", app.overlay);
        };
        assert_eq!(p.title, "Edit link");
        assert_eq!(p.input.trim(), "https://example.dev/spec", "prefilled");
        press(&mut app, KeyCode::Esc, KeyModifiers::NONE, &mut out);

        press(&mut app, KeyCode::Char('d'), KeyModifiers::NONE, &mut out);
        assert!(
            matches!(&app.overlay, Some(Overlay::Confirm(c)) if c.title == "Delete link"),
            "expected the delete confirm, got {:?}",
            app.overlay
        );
        press(&mut app, KeyCode::Char('y'), KeyModifiers::NONE, &mut out);
        assert!(
            out.iter()
                .any(|r| matches!(r, ClientRequest::DeleteLink { id, .. } if id.as_str() == "l1")),
            "expected DeleteLink, got {out:?}"
        );
    }

    /// Seed the selected project's open-pull-request list, as though a
    /// `gh pr list` had just answered.
    fn seed_open_prs(app: &mut App, prs: &[(u64, &str)]) {
        let id = app.selected_project().expect("a project").id.clone();
        let now = std::time::Instant::now();
        app.open_prs.insert(
            id,
            crate::app::OpenPrs {
                list: prs
                    .iter()
                    .map(|(number, title)| crate::pull_request::OpenPr {
                        number: *number,
                        title: (*title).into(),
                        url: format!("https://github.com/o/r/pull/{number}"),
                        is_draft: false,
                    })
                    .collect(),
                at: now,
                due: now + OPEN_PRS_REFRESH,
                step: OPEN_PRS_REFRESH,
            },
        );
    }

    /// The open pull requests take the rows *after* the checkouts, which is
    /// what lets every "index into visible_worktrees()" in the app stay
    /// correct: a cursor on a PR row simply has no selected worktree.
    #[test]
    fn open_prs_take_the_rows_below_the_worktrees() {
        let mut app = App::new();
        seed_tree(&mut app); // p1 / w1(main) / a1
        assert_eq!(app.worktree_row_count(), 1, "no list fetched yet");
        seed_open_prs(&mut app, &[(7, "Attach links"), (9, "Number the lines")]);
        assert_eq!(app.worktree_row_count(), 3);

        assert!(app.selected_worktree().is_some(), "row 0 is the checkout");
        assert!(app.selected_worktree_pr().is_none());

        app.sel_worktree = 1;
        assert!(app.selected_worktree().is_none(), "row 1 is a pull request");
        assert_eq!(app.selected_worktree_pr().map(|p| p.number), Some(7));
        app.sel_worktree = 2;
        assert_eq!(app.selected_worktree_pr().map(|p| p.number), Some(9));
    }

    /// Enter on an open-PR row hands it to the browser and stays put — and
    /// walking into the group must not detach the session on screen: a pull
    /// request has no sessions of its own, so the pane keeps the checkout's.
    #[test]
    fn enter_on_an_open_pr_row_opens_the_browser_without_disturbing_the_pane() {
        let mut app = App::new();
        seed_tree(&mut app);
        seed_open_prs(&mut app, &[(7, "Attach links")]);
        let a1 = SessionRef::Agent(AgentId("a1".into()));
        app.term = Some(AttachedTerm::new(a1.clone(), 40, 10));
        app.focus = Focus::Worktrees;
        let mut out = Vec::new();

        press(&mut app, KeyCode::Down, KeyModifiers::NONE, &mut out);
        assert_eq!(app.sel_worktree, 1, "walked into the OPEN PRS group");
        assert!(
            !out.iter()
                .any(|r| matches!(r, ClientRequest::Detach { .. })),
            "the pane is left alone: {out:?}"
        );
        assert!(app.term.is_some());

        press(&mut app, KeyCode::Enter, KeyModifiers::NONE, &mut out);
        assert_eq!(app.flash.as_deref(), Some("opened github.com/o/r/pull/7"));
        assert_eq!(app.focus, Focus::Worktrees, "Enter stays in the panel");

        // Back onto the checkout, Enter still hands focus one column right.
        press(&mut app, KeyCode::Up, KeyModifiers::NONE, &mut out);
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE, &mut out);
        assert_eq!(app.focus, Focus::Sessions);
    }

    /// A second click on a pull request opens it; one click only selects —
    /// the Sessions panel's link-row rule, so a stray click in the column
    /// never launches a browser.
    #[test]
    fn double_clicking_an_open_pr_row_opens_it() {
        let mut app = App::new();
        seed_tree(&mut app);
        seed_open_prs(&mut app, &[(7, "Attach links")]);
        let mut out = Vec::new();
        app.hits.push((
            ratatui::layout::Rect::new(0, 0, 20, 2),
            HitTarget::Worktree(1),
        ));
        let click = |app: &mut App, out: &mut Vec<ClientRequest>| {
            handle_mouse(
                app,
                MouseEvent {
                    kind: MouseEventKind::Down(MouseButton::Left),
                    column: 1,
                    row: 0,
                    modifiers: KeyModifiers::NONE,
                },
                out,
            )
        };

        click(&mut app, &mut out);
        assert_eq!(app.sel_worktree, 1);
        assert_eq!(app.focus, Focus::Worktrees);
        assert!(app.flash.is_none(), "one click only selects");

        click(&mut app, &mut out);
        assert_eq!(app.flash.as_deref(), Some("opened github.com/o/r/pull/7"));

        // A click on the checkout in between breaks the chain: clicking away
        // and back is two first clicks, not a double-click.
        app.flash = None;
        app.hits.push((
            ratatui::layout::Rect::new(0, 4, 20, 2),
            HitTarget::Worktree(0),
        ));
        let click_at = |app: &mut App, row: u16, out: &mut Vec<ClientRequest>| {
            handle_mouse(
                app,
                MouseEvent {
                    kind: MouseEventKind::Down(MouseButton::Left),
                    column: 1,
                    row,
                    modifiers: KeyModifiers::NONE,
                },
                out,
            )
        };
        click_at(&mut app, 0, &mut out);
        click_at(&mut app, 4, &mut out);
        click_at(&mut app, 0, &mut out);
        assert!(app.flash.is_none(), "got {:?}", app.flash);
    }

    /// `n` on an OPEN PRS row creates a Claude-only SESSION draft against
    /// the PROJECT's ROOT WORKTREE. The PR URL survives the normal naming
    /// flow and crosses IPC on the dedicated create request; no unscoped
    /// PREWARM POOL process can be adopted for it.
    #[test]
    fn new_on_an_open_pr_row_carries_its_url_into_a_claude_session() {
        with_default_config(|| {
            let mut app = App::new();
            seed_tree(&mut app);
            seed_open_prs(&mut app, &[(7, "Attach links")]);
            app.focus = Focus::Worktrees;
            app.sel_worktree = 1;
            let mut out = Vec::new();

            press(&mut app, KeyCode::Char('n'), KeyModifiers::NONE, &mut out);
            let Some(Overlay::Menu(menu)) = &app.overlay else {
                panic!("expected the PR SESSION picker, got {:?}", app.overlay);
            };
            assert_eq!(menu.title.as_deref(), Some("New PR session · #7"));
            assert_eq!(menu.items.len(), 1, "PR rows create Claude AGENTS only");
            assert!(matches!(
                &menu.items[0].action,
                MenuAction::NewAgentOfKind {
                    worktree,
                    kind: AgentKind::Claude,
                    pr_url: Some(url),
                    cloud: false,
                    ..
                } if worktree.as_str() == "w1" && url == "https://github.com/o/r/pull/7"
            ));

            press(&mut app, KeyCode::Enter, KeyModifiers::NONE, &mut out);
            assert!(matches!(
                &app.overlay,
                Some(Overlay::Prompt(p)) if matches!(
                    &p.kind,
                    PromptKind::NewAgent {
                        kind: AgentKind::Claude,
                        pr_url: Some(url),
                        cloud: false,
                        ..
                    } if url == "https://github.com/o/r/pull/7"
                )
            ));
            assert!(
                out.iter()
                    .all(|request| !matches!(request, ClientRequest::PrewarmAgent { .. })),
                "an unscoped warm Claude must not start before a PR SESSION: {out:?}"
            );

            for c in "pr-7".chars() {
                press(&mut app, KeyCode::Char(c), KeyModifiers::NONE, &mut out);
            }
            press(&mut app, KeyCode::Enter, KeyModifiers::NONE, &mut out);
            assert!(
                matches!(
                    out.first(),
                    Some(ClientRequest::CreatePrAgent {
                        worktree,
                        name,
                        pr_url,
                        ..
                    }) if worktree.as_str() == "w1"
                        && name == "pr-7"
                        && pr_url == "https://github.com/o/r/pull/7"
                ),
                "{out:?}"
            );
        });
    }

    #[test]
    fn context_menu_on_an_open_pr_row_offers_a_claude_session() {
        let mut app = App::new();
        seed_tree(&mut app);
        seed_open_prs(&mut app, &[(7, "Attach links")]);
        app.focus = Focus::Worktrees;
        app.sel_worktree = 1;

        open_context_menu_for_selection(&mut app);
        let Some(Overlay::Menu(menu)) = &app.overlay else {
            panic!("expected the OPEN PRS context menu, got {:?}", app.overlay);
        };
        assert!(menu.items.iter().any(|item| {
            item.label == "New Claude session"
                && matches!(
                    &item.action,
                    MenuAction::NewAgentOfKind {
                        pr_url: Some(url),
                        ..
                    } if url == "https://github.com/o/r/pull/7"
                )
        }));

        app.overlay = None;
        app.hits.push((
            ratatui::layout::Rect::new(0, 0, 20, 2),
            HitTarget::Worktree(1),
        ));
        handle_mouse(
            &mut app,
            MouseEvent {
                kind: MouseEventKind::Down(MouseButton::Right),
                column: 1,
                row: 0,
                modifiers: KeyModifiers::NONE,
            },
            &mut Vec::new(),
        );
        let Some(Overlay::Menu(menu)) = &app.overlay else {
            panic!(
                "expected the right-click OPEN PRS menu, got {:?}",
                app.overlay
            );
        };
        assert_eq!(menu.items[0].label, "New Claude session");
    }

    /// A repo with nothing open backs off instead of asking every beat, and
    /// a call `gh` couldn't answer keeps whatever list was already on screen
    /// — one flaky round trip is no reason to blank the group.
    #[test]
    fn the_open_pr_list_backs_off_when_empty_and_survives_a_failed_call() {
        let mut app = App::new();
        seed_tree(&mut app);
        let pid = app.selected_project().expect("a project").id.clone();

        note_open_prs_answer(&mut app, pid.clone(), Some(vec![]), &mut Vec::new());
        assert_eq!(app.open_prs[&pid].step, OPEN_PRS_RECHECK_MIN);
        note_open_prs_answer(&mut app, pid.clone(), Some(vec![]), &mut Vec::new());
        assert_eq!(app.open_prs[&pid].step, OPEN_PRS_RECHECK_MIN * 2);

        let found = vec![crate::pull_request::OpenPr {
            number: 7,
            title: "Attach links".into(),
            url: "https://github.com/o/r/pull/7".into(),
            is_draft: false,
        }];
        note_open_prs_answer(&mut app, pid.clone(), Some(found.clone()), &mut Vec::new());
        assert_eq!(
            app.open_prs[&pid].step, OPEN_PRS_REFRESH,
            "a repo with pull requests settles onto the steady beat"
        );
        assert_eq!(app.visible_open_prs().len(), 1);

        note_open_prs_answer(&mut app, pid.clone(), None, &mut Vec::new());
        assert_eq!(
            app.open_prs[&pid].list, found,
            "a failed call keeps the last good list"
        );
        assert!(app.open_prs[&pid].step > OPEN_PRS_REFRESH, "but backs off");
    }

    /// Arriving at a project asks again promptly — but never more often than
    /// `OPEN_PRS_MIN_AGE`, so bouncing between two projects re-reads the
    /// cache instead of spending an API call per switch.
    #[test]
    fn arriving_at_a_project_re_asks_but_not_faster_than_the_floor() {
        let mut app = App::new();
        seed_tree(&mut app);
        let pid = app.selected_project().expect("a project").id.clone();
        note_open_prs_answer(&mut app, pid.clone(), Some(vec![]), &mut Vec::new());

        schedule_open_prs_lookup(&mut app);
        assert!(
            !app.open_prs_lookup_due(&pid),
            "the answer is seconds old: the floor holds the next call off"
        );

        // An answer older than the floor is re-asked the moment we arrive.
        let stale = std::time::Instant::now() - crate::app::OPEN_PRS_MIN_AGE * 2;
        app.open_prs.get_mut(&pid).unwrap().at = stale;
        schedule_open_prs_lookup(&mut app);
        assert!(app.open_prs_lookup_due(&pid));

        // An in-flight call is never doubled up on.
        app.open_prs_inflight.insert(pid.clone());
        assert!(!app.open_prs_lookup_due(&pid));
    }

    /// Focusing the Worktrees or Sessions panel re-asks GitHub for the open
    /// list and the worktree's own PR; focusing the pane does not. The list
    /// keeps its floor, so a quick bounce between panels stays one call.
    #[test]
    fn focusing_a_sidebar_panel_re_asks_for_pull_requests() {
        let mut app = App::new();
        seed_tree(&mut app);
        let pid = app.selected_project().expect("a project").id.clone();
        let wid = app.selected_worktree().expect("a worktree").id.clone();
        let stale = std::time::Instant::now() - crate::app::OPEN_PRS_MIN_AGE * 2;
        let settle = |app: &mut App| {
            note_open_prs_answer(app, pid.clone(), Some(vec![]), &mut Vec::new());
            app.open_prs.get_mut(&pid).unwrap().at = stale;
            note_pr_answer(app, &wid, true);
        };

        settle(&mut app);
        app.focus = Focus::Terminal;
        note_focus_change(&mut app);
        assert!(
            !app.open_prs_lookup_due(&pid),
            "the pane isn't a PR surface"
        );
        assert!(!app.pr_lookup_due(&wid));

        app.focus = Focus::Worktrees;
        note_focus_change(&mut app);
        assert!(app.open_prs_lookup_due(&pid), "the Worktrees panel is");
        assert!(app.pr_lookup_due(&wid));

        settle(&mut app);
        app.focus = Focus::Sessions;
        note_focus_change(&mut app);
        assert!(app.open_prs_lookup_due(&pid), "so is the Sessions panel");
        assert!(app.pr_lookup_due(&wid));

        // Seconds-fresh answer: the floor holds the list off, the PR row
        // (one `gh pr view`, no floor of its own) is still re-asked.
        note_open_prs_answer(&mut app, pid.clone(), Some(vec![]), &mut Vec::new());
        note_pr_answer(&mut app, &wid, true);
        app.focus = Focus::Worktrees;
        note_focus_change(&mut app);
        assert!(!app.open_prs_lookup_due(&pid), "floored");
        assert!(app.pr_lookup_due(&wid));
    }

    /// The terminal window taking focus again — back from the browser where
    /// a pull request was just closed — re-asks on the next tick.
    #[test]
    fn terminal_window_focus_re_asks_for_pull_requests() {
        let mut app = App::new();
        seed_tree(&mut app);
        let pid = app.selected_project().expect("a project").id.clone();
        let wid = app.selected_worktree().expect("a worktree").id.clone();
        note_open_prs_answer(&mut app, pid.clone(), Some(vec![]), &mut Vec::new());
        app.open_prs.get_mut(&pid).unwrap().at =
            std::time::Instant::now() - crate::app::OPEN_PRS_MIN_AGE * 2;
        note_pr_answer(&mut app, &wid, true);
        assert!(!app.open_prs_lookup_due(&pid));
        assert!(!app.pr_lookup_due(&wid));

        let mut out = Vec::new();
        handle_terminal_event(&mut app, Event::FocusLost, &mut out);
        assert!(!app.open_prs_lookup_due(&pid), "losing focus asks nothing");
        handle_terminal_event(&mut app, Event::FocusGained, &mut out);
        assert!(app.open_prs_lookup_due(&pid));
        assert!(app.pr_lookup_due(&wid));
        assert!(out.is_empty(), "no daemon traffic — gh runs client-side");
    }

    /// The group renders under the checkouts, headed by its own count, and
    /// a list cut off at the fetch cap says so rather than passing itself
    /// off as the whole set.
    #[test]
    fn the_open_pr_group_renders_under_the_worktrees() {
        let mut app = App::new();
        seed_tree(&mut app);
        seed_open_prs(&mut app, &[(7, "Attach links"), (9, "Number lines")]);
        let mut terminal = Terminal::new(TestBackend::new(140, 30)).unwrap();
        terminal.draw(|f| ui::draw(f, &mut app)).unwrap();
        let text = buffer_text(&terminal);
        assert!(text.contains("OPEN PRS · 2"), "group header:\n{text}");
        assert!(text.contains("#7 Attach links"), "PR row:\n{text}");
        let (_, main_y) = find_cell(&terminal, "main");
        let (_, pr_y) = find_cell(&terminal, "#7");
        assert!(pr_y > main_y, "pull requests sit below the checkouts");

        // A full page is reported as "100+": the cap is ours, not GitHub's.
        let many: Vec<(u64, &str)> = (0..crate::pull_request::LIST_LIMIT as u64)
            .map(|n| (n + 1, "wide"))
            .collect();
        seed_open_prs(&mut app, &many);
        terminal.draw(|f| ui::draw(f, &mut app)).unwrap();
        assert!(
            buffer_text(&terminal).contains("OPEN PRS · 100+"),
            "a capped list says so:\n{}",
            buffer_text(&terminal)
        );
    }

    /// Drafts belong in the group. A draft is an open pull request — often
    /// the one a nebula worktree is still attached to — so it renders
    /// alongside the rest, told apart by a badge rather than left out.
    #[test]
    fn draft_pull_requests_render_in_the_group() {
        let mut app = App::new();
        seed_tree(&mut app);
        let pid = app.selected_project().expect("a project").id.clone();
        note_open_prs_answer(
            &mut app,
            pid,
            Some(vec![
                crate::pull_request::OpenPr {
                    number: 9,
                    title: "Still cooking".into(),
                    url: pr_url(9),
                    is_draft: true,
                },
                crate::pull_request::OpenPr {
                    number: 7,
                    title: "Attach links".into(),
                    url: pr_url(7),
                    is_draft: false,
                },
            ]),
            &mut Vec::new(),
        );

        let mut terminal = Terminal::new(TestBackend::new(140, 30)).unwrap();
        terminal.draw(|f| ui::draw(f, &mut app)).unwrap();
        let text = buffer_text(&terminal);
        assert!(text.contains("OPEN PRS · 2"), "both are counted:\n{text}");
        let row = |needle: &str| {
            text.lines()
                .find(|l| l.contains(needle))
                .unwrap_or_else(|| panic!("no {needle} row:\n{text}"))
        };
        assert!(
            row("#9").contains("draft"),
            "the draft is in the list, badged:\n{text}"
        );
        assert!(
            !row("#7").contains("draft"),
            "and the finished one is not:\n{text}"
        );
    }

    /// `/` searches pull requests by title alongside everything else, and
    /// Enter on one opens the browser instead of moving any panel cursor.
    #[test]
    fn the_palette_finds_open_prs_by_title_and_opens_them() {
        let mut app = App::new();
        seed_tree(&mut app);
        seed_open_prs(&mut app, &[(7, "Attach links to worktrees")]);
        app.focus = Focus::Worktrees;
        let mut out = Vec::new();

        press(&mut app, KeyCode::Char('/'), KeyModifiers::NONE, &mut out);
        for c in "attach".chars() {
            press(&mut app, KeyCode::Char(c), KeyModifiers::NONE, &mut out);
        }
        {
            let p = palette(&app);
            assert_eq!(
                p.items[p.matches[0].item].text, "demo/#7 Attach links to worktrees",
                "the project prefixes it, like every other row"
            );
        }
        // Enter opens it whether or not "Enter attaches" is on — there is no
        // second, quieter thing a pull request can do.
        set_enter_attaches(&mut app, false);
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE, &mut out);
        assert!(app.overlay.is_none(), "the palette closes");
        assert_eq!(app.flash.as_deref(), Some("opened github.com/o/r/pull/7"));
        assert_eq!(app.sel_worktree, 0, "no panel cursor moved");
        assert_eq!(app.focus, Focus::Worktrees);
    }

    /// A space in the query is an AND between terms, not a char to match: the
    /// PR row has no space between its project prefix and its `#7`, and
    /// "demo #7" still has to find it.
    #[test]
    fn palette_query_terms_match_independently_across_a_space() {
        let mut app = App::new();
        seed_tree(&mut app);
        seed_open_prs(&mut app, &[(7, "Attach links to worktrees")]);
        let mut out = Vec::new();

        press(&mut app, KeyCode::Char('/'), KeyModifiers::NONE, &mut out);
        for c in "demo #7".chars() {
            press(&mut app, KeyCode::Char(c), KeyModifiers::NONE, &mut out);
        }
        let p = palette(&app);
        assert_eq!(p.query, "demo #7", "the space reaches the query");
        let texts: Vec<&str> = p
            .matches
            .iter()
            .map(|m| p.items[m.item].text.as_str())
            .collect();
        assert_eq!(texts, vec!["demo/#7 Attach links to worktrees"]);
    }

    /// A pull request merged or closed on GitHub simply stops coming back
    /// from `gh pr list`, and that is the whole retirement mechanism: the
    /// next refresh drops the row, the cursor lands on a surviving one
    /// rather than on whatever inherited its index, and the body cached for
    /// the reading pane is forgotten with it. A draft is an open pull
    /// request and comes through the same pass untouched.
    #[test]
    fn a_merged_pull_request_leaves_the_list_on_the_next_refresh() {
        let mut app = App::new();
        seed_tree(&mut app); // p1 / w1(main) / a1
        let pid = app.selected_project().expect("a project").id.clone();
        let answer = |app: &mut App, prs: Vec<(u64, bool)>| {
            let list = prs
                .into_iter()
                .map(|(number, is_draft)| crate::pull_request::OpenPr {
                    number,
                    title: format!("pull {number}"),
                    url: format!("https://github.com/o/r/pull/{number}"),
                    is_draft,
                })
                .collect();
            note_open_prs_answer(app, pid.clone(), Some(list), &mut Vec::new());
        };

        answer(&mut app, vec![(7, false), (9, true)]);
        assert_eq!(app.worktree_row_count(), 3, "the checkout, then both PRs");
        app.pr_detail
            .insert(pr_url(7), a_detail(7, "read on a hover", vec![]));
        app.sel_worktree = 1;
        assert_eq!(app.selected_worktree_pr().map(|p| p.number), Some(7));

        // #7 is merged: the next list doesn't mention it.
        answer(&mut app, vec![(9, true)]);
        assert_eq!(
            open_pr_numbers(&app),
            vec![9],
            "the draft is still open and stays; the merged one goes"
        );
        assert_eq!(
            app.selected_worktree_pr().map(|p| p.number),
            Some(9),
            "the cursor lands on the row that survived"
        );
        assert!(
            !app.pr_detail.contains_key(&pr_url(7)),
            "and the body cached for the reading pane goes with it"
        );

        // The last one closes too: the cursor falls back to the checkout.
        answer(&mut app, vec![]);
        assert!(app.visible_open_prs().is_empty());
        assert!(
            app.selected_worktree().is_some(),
            "the cursor lands on the checkout, not past the end of the list"
        );
    }

    /// A refresh that merely reorders the list keeps the cursor on the pull
    /// request it was reading, not on whatever now holds that index — `gh`
    /// sorts newest first, so anyone opening a PR reshuffles everything
    /// below it.
    #[test]
    fn the_cursor_follows_its_pull_request_across_a_reorder() {
        let mut app = App::new();
        seed_tree(&mut app);
        seed_open_prs(&mut app, &[(9, "Number lines"), (7, "Attach links")]);
        app.sel_worktree = 2;
        assert_eq!(app.selected_worktree_pr().map(|p| p.number), Some(7));

        let pid = app.selected_project().expect("a project").id.clone();
        let list = vec![
            crate::pull_request::OpenPr {
                number: 11,
                title: "Brand new".into(),
                url: pr_url(11),
                is_draft: true,
            },
            crate::pull_request::OpenPr {
                number: 9,
                title: "Number lines".into(),
                url: pr_url(9),
                is_draft: false,
            },
            crate::pull_request::OpenPr {
                number: 7,
                title: "Attach links".into(),
                url: pr_url(7),
                is_draft: false,
            },
        ];
        // Halfway down #7's conversation when the refresh lands.
        app.pr_preview_scroll = 12;
        note_open_prs_answer(&mut app, pid, Some(list), &mut Vec::new());
        assert_eq!(open_pr_numbers(&app), vec![11, 9, 7]);
        assert_eq!(
            app.selected_worktree_pr().map(|p| p.number),
            Some(7),
            "still on #7, two rows further down"
        );
        assert_eq!(app.sel_worktree, 3);
        assert_eq!(
            app.pr_preview_scroll, 12,
            "and still where they were reading — a beat this quick must not \
             rewind the pane under them"
        );
    }

    /// The detail fetched for the row under the cursor is GitHub's answer
    /// about that one pull request, so a `MERGED` or `CLOSED` state retires
    /// the row on the spot instead of leaving the user reading something
    /// the next refresh is about to take away.
    #[test]
    fn a_detail_that_says_merged_retires_the_row_on_the_spot() {
        let mut app = App::new();
        seed_tree(&mut app);
        seed_open_prs(&mut app, &[(7, "Attach links"), (9, "Number lines")]);
        app.sel_worktree = 1;

        let mut merged = a_detail(7, "shipped", vec![]);
        merged.state = "MERGED".into();
        assert!(!merged.is_open());
        app.pr_detail.insert(pr_url(7), merged);
        drop_retired_pr(&mut app, &pr_url(7), &mut Vec::new());

        assert_eq!(open_pr_numbers(&app), vec![9]);
        assert_eq!(app.selected_worktree_pr().map(|p| p.number), Some(9));
        assert_eq!(
            app.flash.as_deref(),
            Some("#7 is no longer open"),
            "a row that evaporates mid-read says why"
        );

        // A draft is open, so nothing about it is retired.
        let mut draft = a_detail(9, "still cooking", vec![]);
        draft.is_draft = true;
        assert!(draft.is_open(), "a draft is an open pull request");
    }

    fn pr_url(number: u64) -> String {
        format!("https://github.com/o/r/pull/{number}")
    }

    fn open_pr_numbers(app: &App) -> Vec<u64> {
        app.visible_open_prs().iter().map(|p| p.number).collect()
    }

    fn a_detail(
        number: u64,
        body: &str,
        comments: Vec<crate::pull_request::PrComment>,
    ) -> crate::pull_request::PrDetail {
        crate::pull_request::PrDetail {
            number,
            url: format!("https://github.com/o/r/pull/{number}"),
            title: "Attach links".into(),
            state: "OPEN".into(),
            is_draft: false,
            author: "webdevcody".into(),
            base: "main".into(),
            head: "feat/links".into(),
            additions: 106,
            deletions: 4,
            changed_files: 2,
            body: body.into(),
            comments,
        }
    }

    /// Resting on a pull request arms a debounced fetch; one already known
    /// (cached, in flight, or already refused) arms nothing, so walking a
    /// long list costs no API calls at all.
    #[test]
    fn hovering_a_pr_row_arms_one_debounced_detail_fetch() {
        let mut app = App::new();
        seed_tree(&mut app);
        seed_open_prs(&mut app, &[(7, "Attach links"), (9, "Number lines")]);
        app.focus = Focus::Worktrees;
        let mut out = Vec::new();

        assert!(app.pending_pr_detail.is_none(), "the checkout arms nothing");
        press(&mut app, KeyCode::Down, KeyModifiers::NONE, &mut out);
        let (pending, _) = app.pending_pr_detail.clone().expect("armed on #7");
        assert_eq!(pending.number, 7);
        assert_eq!(pending.url, "https://github.com/o/r/pull/7");
        assert!(
            app.pr_detail_delay()
                .is_some_and(|d| d <= PR_DETAIL_DEBOUNCE),
            "and it is a delay, not an immediate fetch"
        );

        // Already read: nothing to fetch, and the pane has it in hand.
        press(&mut app, KeyCode::Down, KeyModifiers::NONE, &mut out);
        assert_eq!(
            app.pending_pr_detail.as_ref().map(|(p, _)| p.number),
            Some(9)
        );
        app.pr_detail.insert(
            "https://github.com/o/r/pull/9".into(),
            a_detail(9, "hi", vec![]),
        );
        schedule_pr_detail(&mut app);
        assert!(app.pending_pr_detail.is_none(), "cached: nothing to ask");

        // One `gh` already said no: don't keep asking on every pass.
        app.pr_detail_failed
            .insert("https://github.com/o/r/pull/7".into());
        app.sel_worktree = 1;
        schedule_pr_detail(&mut app);
        assert!(app.pending_pr_detail.is_none(), "refused: nothing to ask");

        // Back on a checkout, the debounce is disarmed entirely.
        app.pr_detail_failed.clear();
        app.sel_worktree = 0;
        schedule_pr_detail(&mut app);
        assert!(app.pending_pr_detail.is_none());
    }

    /// The pane reads the pull request while the cursor rests on it — and
    /// the session underneath stays attached, so stepping back is instant.
    #[test]
    fn the_pane_reads_the_pull_request_under_the_cursor() {
        let mut app = App::new();
        seed_tree(&mut app);
        seed_open_prs(&mut app, &[(7, "Attach links")]);
        let a1 = SessionRef::Agent(AgentId("a1".into()));
        app.term = Some(AttachedTerm::new(a1.clone(), 40, 10));
        app.focus = Focus::Worktrees;
        app.sel_worktree = 1;

        let mut terminal = Terminal::new(TestBackend::new(140, 30)).unwrap();
        terminal.draw(|f| ui::draw(f, &mut app)).unwrap();
        let text = buffer_text(&terminal);
        assert!(text.contains("PULL REQUEST"), "pane retitles:\n{text}");
        assert!(text.contains("reading it…"), "loading state:\n{text}");
        assert!(app.term.is_some(), "the session stays attached underneath");

        app.pr_detail.insert(
            "https://github.com/o/r/pull/7".into(),
            a_detail(
                7,
                "Pins a PR to the worktree.",
                vec![crate::pull_request::PrComment {
                    author: "kate".into(),
                    at: "2026-08-20T19:55:42Z".into(),
                    review_state: "APPROVED".into(),
                    body: "ship it".into(),
                }],
            ),
        );
        terminal.draw(|f| ui::draw(f, &mut app)).unwrap();
        let text = buffer_text(&terminal);
        assert!(text.contains("#7 Attach links"), "{text}");
        assert!(text.contains("+106 -4 · 2 files"), "{text}");
        assert!(text.contains("Pins a PR to the worktree."), "{text}");
        assert!(text.contains("kate approved"), "{text}");
        assert!(
            !text.contains("reading it…"),
            "loading state clears:\n{text}"
        );

        // Back on the checkout the pane is a terminal again — the PR row
        // stays in the Worktrees column, but its body leaves the pane.
        app.sel_worktree = 0;
        terminal.draw(|f| ui::draw(f, &mut app)).unwrap();
        let text = buffer_text(&terminal);
        assert!(text.contains("TERMINAL · agent-1"), "{text}");
        assert!(!text.contains("Pins a PR to the worktree."), "{text}");
        assert!(!text.contains("+106 -4"), "{text}");
    }

    /// A pull request `gh` couldn't read says so rather than sitting on
    /// "reading it…" forever.
    #[test]
    fn an_unreadable_pull_request_says_so_in_the_pane() {
        let mut app = App::new();
        seed_tree(&mut app);
        seed_open_prs(&mut app, &[(7, "Attach links")]);
        app.focus = Focus::Worktrees;
        app.sel_worktree = 1;
        app.pr_detail_failed
            .insert("https://github.com/o/r/pull/7".into());
        let mut terminal = Terminal::new(TestBackend::new(140, 30)).unwrap();
        terminal.draw(|f| ui::draw(f, &mut app)).unwrap();
        let text = buffer_text(&terminal);
        assert!(text.contains("unavailable"), "{text}");
        assert!(text.contains("couldn't read this pull request"), "{text}");
    }

    /// Seed the Sessions panel's PR ROW: the pull request `gh` found on the
    /// seeded worktree's branch.
    fn seed_branch_pr(app: &mut App, number: u64, title: &str) {
        app.pull_requests.insert(
            nebula_core::WorktreeId("w1".into()),
            Some(crate::pull_request::PullRequest {
                number,
                url: format!("https://github.com/o/r/pull/{number}"),
                title: title.into(),
                state: "OPEN".into(),
                is_draft: false,
                activity: Vec::new(),
            }),
        );
    }

    fn sessions_pr_row(app: &App) -> usize {
        app.visible_session_rows()
            .iter()
            .position(|r| r.as_link().is_some())
            .expect("the PR ROW")
    }

    /// The Sessions panel's PR ROW reads in the pane the same way the
    /// project-wide open-PR rows do — while that panel has focus. Stepping
    /// into the pane brings the attached session back: unlike a Worktrees
    /// PR row, there is still one underneath.
    #[test]
    fn the_sessions_panel_pr_row_reads_in_the_pane_too() {
        let mut app = App::new();
        seed_tree(&mut app);
        seed_branch_pr(&mut app, 7, "Attach links");
        let a1 = SessionRef::Agent(AgentId("a1".into()));
        app.term = Some(AttachedTerm::new(a1, 40, 10));
        app.focus = Focus::Sessions;
        app.sel_session = sessions_pr_row(&app);
        assert_eq!(app.previewed_pr().map(|pr| pr.number), Some(7));

        let mut terminal = Terminal::new(TestBackend::new(140, 30)).unwrap();
        terminal.draw(|f| ui::draw(f, &mut app)).unwrap();
        let text = buffer_text(&terminal);
        assert!(text.contains("PULL REQUEST"), "pane retitles:\n{text}");
        assert!(text.contains("reading it…"), "loading state:\n{text}");
        assert!(text.contains("PgUp/PgDn: scroll"), "footer:\n{text}");
        assert!(app.term.is_some(), "the session stays attached underneath");

        app.pr_detail.insert(
            "https://github.com/o/r/pull/7".into(),
            a_detail(7, "Pins a PR to the worktree.", vec![]),
        );
        terminal.draw(|f| ui::draw(f, &mut app)).unwrap();
        let text = buffer_text(&terminal);
        assert!(text.contains("#7 Attach links"), "{text}");
        assert!(text.contains("Pins a PR to the worktree."), "{text}");

        // Into the pane: the terminal has focus, so the terminal is what
        // shows — what you type is what you see.
        app.focus = Focus::Terminal;
        assert!(app.previewed_pr().is_none());
        terminal.draw(|f| ui::draw(f, &mut app)).unwrap();
        let text = buffer_text(&terminal);
        assert!(text.contains("TERMINAL · agent-1"), "{text}");
        assert!(!text.contains("Pins a PR to the worktree."), "{text}");

        // Back on a session row the pane is a terminal again.
        app.focus = Focus::Sessions;
        app.sel_session = 0;
        assert!(app.previewed_pr().is_none());
        terminal.draw(|f| ui::draw(f, &mut app)).unwrap();
        assert!(buffer_text(&terminal).contains("TERMINAL · agent-1"));
    }

    /// The loop notices the pane reading something else by URL: landing on
    /// the Sessions PR ROW arms one debounced fetch, a same-URL turn leaves
    /// a reader's scroll alone, and leaving it (for the pane here) disarms.
    #[test]
    fn stepping_onto_the_sessions_pr_row_arms_the_detail_fetch() {
        let mut app = App::new();
        seed_tree(&mut app);
        seed_branch_pr(&mut app, 7, "Attach links");
        app.focus = Focus::Sessions;
        let mut out = Vec::new();

        // Walk down from the agent row onto the PR ROW the way the loop
        // does: the URL before each key, the check after it.
        let target = sessions_pr_row(&app);
        assert!(
            app.pending_pr_detail.is_none(),
            "a session row arms nothing"
        );
        while app.sel_session < target {
            let before = app.previewed_pr().map(|pr| pr.url);
            press(&mut app, KeyCode::Down, KeyModifiers::NONE, &mut out);
            note_preview_change(&mut app, before);
        }
        let (pending, _) = app.pending_pr_detail.clone().expect("armed on #7");
        assert_eq!(pending.number, 7);
        assert_eq!(pending.url, "https://github.com/o/r/pull/7");
        assert_eq!(pending.dir, std::path::PathBuf::from("/tmp/demo"));
        assert!(
            app.pr_detail_delay()
                .is_some_and(|d| d <= PR_DETAIL_DEBOUNCE),
            "a delay, not an immediate fetch"
        );

        // A turn that changes nothing about the row keeps the reader's place.
        app.pr_preview_scroll = 9;
        let before = app.previewed_pr().map(|pr| pr.url);
        note_preview_change(&mut app, before);
        assert_eq!(app.pr_preview_scroll, 9);
        assert!(app.pending_pr_detail.is_some(), "still armed");

        // Focus into the pane: a terminal has nothing to fetch.
        let before = app.previewed_pr().map(|pr| pr.url);
        app.focus = Focus::Terminal;
        note_preview_change(&mut app, before);
        assert!(app.pending_pr_detail.is_none());
        assert_eq!(app.pr_preview_scroll, 0);
    }

    /// `g` on the Sessions PR ROW asks GitHub for that pull request's diff
    /// rather than opening the checkout's, and its row menu offers the same.
    #[test]
    fn g_on_the_sessions_pr_row_reads_its_diff() {
        let mut app = App::new();
        seed_tree(&mut app);
        seed_branch_pr(&mut app, 7, "Attach links");
        app.focus = Focus::Sessions;
        app.sel_session = sessions_pr_row(&app);
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        app.pr_diff_tx = Some(tx);
        app.pr_diff_inflight = Some(7);
        let mut out = Vec::new();

        press(&mut app, KeyCode::Char('g'), KeyModifiers::NONE, &mut out);
        assert!(app.overlay.is_none(), "not the worktree's diff modal");
        assert_eq!(
            app.flash.as_deref(),
            Some("still fetching the diff for #7…")
        );

        let items = menu_items_for_link(&app.selected_link().expect("the PR ROW"));
        assert!(items.iter().any(|i| i.label == "View diff"), "{items:?}");
        assert!(
            !items.iter().any(|i| i.label == "Delete"),
            "still not the user's row: {items:?}"
        );
    }

    /// PgDn/PgUp/Home/End page the preview, clamped to its real length —
    /// the pane writes the line count back on every draw.
    #[test]
    fn the_preview_pages_and_clamps_to_its_length() {
        let mut app = App::new();
        seed_tree(&mut app);
        seed_open_prs(&mut app, &[(7, "Attach links")]);
        app.focus = Focus::Worktrees;
        app.sel_worktree = 1;
        let body = (0..200)
            .map(|i| format!("line {i}"))
            .collect::<Vec<_>>()
            .join("\n");
        app.pr_detail.insert(
            "https://github.com/o/r/pull/7".into(),
            a_detail(7, &body, vec![]),
        );
        let mut terminal = Terminal::new(TestBackend::new(140, 30)).unwrap();
        terminal.draw(|f| ui::draw(f, &mut app)).unwrap();
        assert!(app.pr_preview_lines > 200, "the body wrapped long");

        let mut out = Vec::new();
        press(&mut app, KeyCode::PageDown, KeyModifiers::NONE, &mut out);
        let paged = app.pr_preview_scroll;
        assert!(paged > 0, "PgDn moved");
        press(&mut app, KeyCode::End, KeyModifiers::NONE, &mut out);
        assert_eq!(app.pr_preview_scroll, app.pr_preview_max_scroll());
        press(&mut app, KeyCode::PageDown, KeyModifiers::NONE, &mut out);
        assert_eq!(
            app.pr_preview_scroll,
            app.pr_preview_max_scroll(),
            "the end is the end"
        );
        press(&mut app, KeyCode::Home, KeyModifiers::NONE, &mut out);
        assert_eq!(app.pr_preview_scroll, 0);
        assert!(out.is_empty(), "reading a PR sends the daemon nothing");

        // Moving to another row starts its preview at the top.
        app.pr_preview_scroll = paged;
        app.sel_worktree = 0;
        schedule_pr_detail(&mut app);
        assert_eq!(app.pr_preview_scroll, 0);
    }

    /// `g` on a pull-request row opens the ordinary diff modal on the
    /// fetched diff — file list from the diff itself, and switching files
    /// reads the text already in hand rather than shelling out at git.
    #[test]
    fn the_fetched_pr_diff_opens_in_the_diff_modal() {
        let mut app = App::new();
        seed_tree(&mut app);
        seed_open_prs(&mut app, &[(7, "Attach links")]);
        app.sel_worktree = 1;
        let diff = "\
diff --git a/src/a.rs b/src/a.rs
--- a/src/a.rs
+++ b/src/a.rs
@@ -1 +1 @@
-old
+new
diff --git a/src/b.rs b/src/b.rs
--- a/src/b.rs
+++ b/src/b.rs
@@ -1 +1 @@
-x
+y
";
        app.pr_diff_inflight = Some(7);
        open_pr_diff_view(&mut app, 7, "#7 Attach links".into(), Some(diff.into()));
        assert!(app.pr_diff_inflight.is_none(), "the fetch is done");
        let Some(Overlay::Diff(view)) = &app.overlay else {
            panic!("expected the diff modal, got {:?}", app.overlay);
        };
        assert_eq!(view.branch, "#7 Attach links", "the PR titles the modal");
        assert_eq!(
            view.files
                .iter()
                .map(|f| f.path.as_str())
                .collect::<Vec<_>>(),
            ["src/a.rs", "src/b.rs"]
        );
        assert!(
            view.diff.contains("+new"),
            "first file's diff: {}",
            view.diff
        );

        // Selecting the second file reads the prefetched chunk — no repo is
        // touched (seed_tree's /tmp/demo isn't even a git checkout).
        let Some(Overlay::Diff(view)) = &mut app.overlay else {
            unreachable!()
        };
        view.select(1);
        crate::git_diff::load_selected_diff(view);
        assert!(
            view.diff.contains("+y"),
            "second file's diff: {}",
            view.diff
        );
        assert!(!view.diff.contains("+new"), "chunks don't bleed");
    }

    /// A diff `gh` couldn't fetch flashes and leaves the modal shut, and a
    /// second `g` while one is already in flight doesn't stack a request.
    #[test]
    fn a_failed_pr_diff_flashes_instead_of_opening() {
        let mut app = App::new();
        seed_tree(&mut app);
        seed_open_prs(&mut app, &[(7, "Attach links")]);
        app.sel_worktree = 1;

        app.pr_diff_inflight = Some(7);
        open_pr_diff_view(&mut app, 7, "#7 Attach links".into(), None);
        assert!(app.overlay.is_none());
        assert!(
            app.flash
                .as_deref()
                .is_some_and(|f| f.contains("couldn't read the diff for #7")),
            "got {:?}",
            app.flash
        );

        // An empty diff is not a modal with no rows in it.
        open_pr_diff_view(&mut app, 7, "#7".into(), Some(String::new()));
        assert!(app.overlay.is_none());
        assert_eq!(app.flash.as_deref(), Some("#7 changes no files"));

        // Mashing the key while one is in flight is a nudge, not a request.
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        app.pr_diff_tx = Some(tx);
        app.pr_diff_inflight = Some(7);
        request_pr_diff(&mut app);
        assert_eq!(
            app.flash.as_deref(),
            Some("still fetching the diff for #7…")
        );
    }

    /// The pull-request row comes back from git on every lookup, so editing
    /// or deleting it would be a lie. Both say so instead.
    #[test]
    fn the_pull_request_row_cannot_be_edited_or_deleted() {
        let mut app = App::new();
        seed_tree(&mut app);
        app.pull_requests.insert(
            nebula_core::WorktreeId("w1".into()),
            Some(crate::pull_request::PullRequest {
                number: 7,
                url: "https://github.com/o/r/pull/7".into(),
                title: "Attach links".into(),
                state: "OPEN".into(),
                is_draft: false,
                activity: Vec::new(),
            }),
        );
        app.focus = Focus::Sessions;
        app.sel_session = app
            .visible_session_rows()
            .iter()
            .position(|r| r.as_link().is_some())
            .expect("pull-request row");
        let mut out = Vec::new();

        press(&mut app, KeyCode::Char('d'), KeyModifiers::NONE, &mut out);
        assert!(app.overlay.is_none(), "no confirm for a row we don't own");
        assert!(
            app.flash
                .as_deref()
                .is_some_and(|f| f.contains("can't be deleted")),
            "got {:?}",
            app.flash
        );

        press(&mut app, KeyCode::Char('r'), KeyModifiers::NONE, &mut out);
        assert!(app.overlay.is_none(), "nothing stored to edit");
        assert_eq!(
            app.flash.as_deref(),
            Some("the pull request comes from git and can't be edited")
        );
        // Enter still opens it — reading the PR is the whole point.
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE, &mut out);
        assert_eq!(app.flash.as_deref(), Some("opened github.com/o/r/pull/7"));
        assert!(!out
            .iter()
            .any(|r| matches!(r, ClientRequest::DeleteLink { .. })));
    }

    /// Shift+D wipes the panel's sessions; links are bookmarks and survive.
    #[test]
    fn delete_all_sessions_leaves_links_alone() {
        let mut app = App::new();
        seed_tree(&mut app);
        seed_link(&mut app, "https://example.dev/spec");
        let mut out = Vec::new();
        press(&mut app, KeyCode::Char('D'), KeyModifiers::NONE, &mut out);
        let Some(Overlay::Confirm(c)) = &app.overlay else {
            panic!("expected the bulk confirm, got {:?}", app.overlay);
        };
        assert!(
            !c.message.contains("example.dev"),
            "links are not up for deletion: {}",
            c.message
        );
        let PendingAction::DeleteAllSessions { agents, terminals } = &c.action else {
            panic!("wrong action: {:?}", c.action);
        };
        assert_eq!(agents.len(), 1);
        assert!(terminals.is_empty());
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

    /// An empty `gh` answer doesn't retire the worktree: the next attempt
    /// is armed one backoff step out, growing to the cap so a checkout that
    /// never grows a PR stops costing a process every few seconds.
    #[test]
    fn empty_pr_answers_back_off_instead_of_settling() {
        use nebula_core::WorktreeId;
        let mut app = App::new();
        let wt = WorktreeId("w1".into());
        assert!(app.pr_lookup_due(&wt), "never asked: due immediately");

        note_pr_answer(&mut app, &wt, false);
        let (_, first) = *app.pr_recheck.get(&wt).expect("backoff armed");
        assert_eq!(first, PR_RECHECK_MIN);
        assert!(!app.pr_lookup_due(&wt), "not due until the backoff expires");

        note_pr_answer(&mut app, &wt, false);
        let (_, second) = *app.pr_recheck.get(&wt).expect("backoff armed");
        assert_eq!(second, PR_RECHECK_MIN * 2, "each miss doubles the gap");

        for _ in 0..12 {
            note_pr_answer(&mut app, &wt, false);
        }
        let (_, capped) = *app.pr_recheck.get(&wt).expect("backoff armed");
        assert_eq!(capped, PR_RECHECK_MAX, "growth stops at the cap");
    }

    /// A due backoff makes the worktree askable again — this is what lets a
    /// PR opened by a session after the first lookup still land on the row.
    #[test]
    fn an_expired_backoff_asks_again() {
        use nebula_core::WorktreeId;
        let mut app = App::new();
        let wt = WorktreeId("w1".into());
        app.pull_requests.insert(wt.clone(), None);
        app.pr_recheck.insert(
            wt.clone(),
            (
                std::time::Instant::now() - Duration::from_secs(1),
                PR_RECHECK_MIN,
            ),
        );
        assert!(app.pr_lookup_due(&wt), "a cached miss is not the last word");
    }

    /// Finding the PR settles the worktree onto a steady beat rather than
    /// retiring it: the PR won't change, but its conversation will, and the
    /// unread-comment badge is only as fresh as the last poll.
    #[test]
    fn a_found_pr_keeps_being_refreshed() {
        use nebula_core::WorktreeId;
        let mut app = App::new();
        let wt = WorktreeId("w1".into());
        note_pr_answer(&mut app, &wt, false);
        note_pr_answer(&mut app, &wt, true);
        let (_, step) = *app.pr_recheck.get(&wt).expect("still scheduled");
        assert_eq!(step, PR_REFRESH, "the miss backoff gives way to the beat");

        app.pull_requests.insert(
            wt.clone(),
            Some(crate::pull_request::PullRequest {
                number: 7,
                url: "https://github.com/o/r/pull/7".into(),
                title: "done".into(),
                state: "OPEN".into(),
                is_draft: false,
                activity: Vec::new(),
            }),
        );
        assert!(!app.pr_lookup_due(&wt), "not before the beat comes round");

        // Switching into the checkout is a reason to ask right now — that's
        // when the user wants to know whether anyone has commented.
        seed_tree(&mut app);
        schedule_pr_lookup(&mut app);
        assert!(app.pr_lookup_due(&wt), "arriving re-asks immediately");
    }

    /// Opening a pull request row banks everything nebula knows about its
    /// conversation, so the badge clears on the spot and the daemon is told
    /// to remember it. What lands afterwards is what counts as new.
    #[test]
    fn opening_a_pull_request_marks_it_read() {
        use nebula_core::WorktreeId;
        let mut app = App::new();
        let url = "https://github.com/o/r/pull/7";
        let wt = WorktreeId("w1".into());
        app.pull_requests.insert(
            wt.clone(),
            Some(crate::pull_request::PullRequest {
                number: 7,
                url: url.into(),
                title: "done".into(),
                state: "OPEN".into(),
                is_draft: false,
                activity: vec!["2024-04-25T19:55:42Z".into()],
            }),
        );
        let mut out = Vec::new();
        mark_pr_seen(&mut app, url, &mut out);
        assert_eq!(
            app.pr_seen.get(url).map(String::as_str),
            Some("2024-04-25T19:55:42Z"),
            "applied locally so the badge clears this frame"
        );
        assert!(matches!(
            out.as_slice(),
            [ClientRequest::MarkPrSeen { url: u, marker: m }]
                if u == url && m == "2024-04-25T19:55:42Z"
        ));

        // Opening it again with nothing new says nothing to the daemon.
        out.clear();
        mark_pr_seen(&mut app, url, &mut out);
        assert!(out.is_empty(), "an unmoved mark is not worth a round trip");

        // A URL that isn't a pull request has no conversation to bank.
        mark_pr_seen(&mut app, "https://example.dev/spec", &mut out);
        assert!(out.is_empty());
        assert_eq!(app.pr_seen.len(), 1);
    }

    /// The end-to-end shape the badge reads: a comment arrives after the
    /// last open, the row counts it, opening the row clears it again.
    #[test]
    fn the_link_row_counts_comments_that_landed_since_the_last_open() {
        use nebula_core::WorktreeId;
        let mut app = App::new();
        seed_tree(&mut app);
        let url = "https://github.com/o/r/pull/7";
        let wt = WorktreeId("w1".into());
        let pr = |activity: Vec<String>| crate::pull_request::PullRequest {
            number: 7,
            url: url.into(),
            title: "Attach links".into(),
            state: "OPEN".into(),
            is_draft: false,
            activity,
        };

        app.pull_requests
            .insert(wt.clone(), Some(pr(vec!["2024-04-25T19:55:42Z".into()])));
        fn unseen(app: &App) -> usize {
            app.visible_links()
                .into_iter()
                .next()
                .expect("the pull request row")
                .unseen_comments(&app.pr_seen)
        }
        assert_eq!(
            unseen(&app),
            1,
            "never opened: the whole conversation is unread"
        );

        let mut out = Vec::new();
        mark_pr_seen(&mut app, url, &mut out);
        assert_eq!(unseen(&app), 0, "opening clears it");

        // Somebody replies; the next poll brings it back.
        app.pull_requests.insert(
            wt,
            Some(pr(vec![
                "2024-04-25T19:55:42Z".into(),
                "2024-04-27T09:00:00Z".into(),
            ])),
        );
        assert_eq!(unseen(&app), 1, "one new comment");
    }

    /// A lookup in flight blocks a second one, so the 2s git tick can't
    /// stack `gh` processes on a slow network.
    #[test]
    fn an_inflight_lookup_blocks_a_second_one() {
        use nebula_core::WorktreeId;
        let mut app = App::new();
        let wt = WorktreeId("w1".into());
        app.pr_inflight.insert(wt.clone());
        assert!(!app.pr_lookup_due(&wt));
        app.pr_inflight.remove(&wt);
        assert!(app.pr_lookup_due(&wt));
    }

    /// Switching into a worktree drops its accumulated backoff, so arriving
    /// somewhere asks `gh` again on the next tick rather than up to three
    /// minutes later.
    #[test]
    fn a_worktree_switch_clears_the_backoff() {
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
                    sort_order: 1,
                }),
            },
        );
        let w2 = WorktreeId("w2".into());
        app.pull_requests.insert(w2.clone(), None);
        app.pr_recheck.insert(
            w2.clone(),
            (std::time::Instant::now() + PR_RECHECK_MAX, PR_RECHECK_MAX),
        );
        assert!(!app.pr_lookup_due(&w2), "backed off before the switch");

        app.focus = Focus::Worktrees;
        let mut out = Vec::new();
        move_selection(&mut app, 1, &mut out);
        assert_eq!(
            app.selected_worktree().map(|w| w.id.clone()),
            Some(w2.clone())
        );
        assert!(app.pr_lookup_due(&w2), "the switch re-arms the lookup");
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
                    cloud: false,
                    pr_url: None,
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
                links: tree.links,
                pr_seen: Vec::new(),
                ui_state: None,
            },
        );
        let (armed, _) = fresh.pending_prewarm.clone().expect("prewarm armed");
        assert_eq!(armed, nebula_core::WorktreeId("w1".into()));
    }

    /// The startup snapshot puts the cursor back on the session the user
    /// left on — and its terminal back in the pane. A restored selection
    /// over a blank pane reads as "nebula forgot", even though the row is
    /// highlighted.
    #[test]
    fn snapshot_reattaches_the_remembered_session() {
        let mut app = App::new();
        seed_tree(&mut app); // p1 / w1 / a1
        let tree = app.tree.clone();
        let snapshot = |ui_state: Option<String>| ServerEvent::Snapshot {
            workspaces: tree.workspaces.clone(),
            active_workspace: tree.active_workspace.clone(),
            projects: tree.projects.clone(),
            worktrees: tree.worktrees.clone(),
            agents: tree.agents.clone(),
            terminals: tree.terminals.clone(),
            links: tree.links.clone(),
            pr_seen: Vec::new(),
            ui_state,
        };
        let a1 = SessionRef::Agent(nebula_core::AgentId("a1".into()));

        // Remembered session present: the pane comes back with the cursor.
        let mut fresh = App::new();
        let mut out = Vec::new();
        handle_server_event(
            &mut fresh,
            snapshot(Some(
                r#"{"project":"p1","worktree":"w1","session_agent":"a1","show_archived":false,"collapsed":false}"#
                    .into(),
            )),
            &mut out,
        );
        assert_eq!(
            fresh.term.as_ref().map(|t| t.sref.clone()),
            Some(a1.clone())
        );
        assert!(
            out.iter()
                .any(|r| matches!(r, ClientRequest::Attach { session, .. } if *session == a1)),
            "expected an Attach for a1, got {out:?}"
        );
        // The cursor stays on the panels: this is a preview, not Enter.
        assert_eq!(fresh.focus, Focus::Projects);
        assert!(!fresh.term_locked);

        // No blob (first launch) or a blob whose session is gone: nothing
        // to bring back, so the pane stays blank rather than guessing.
        for blob in [
            None,
            Some(
                r#"{"project":"p1","worktree":"w1","session_agent":"gone","show_archived":false,"collapsed":false}"#
                    .to_string(),
            ),
        ] {
            let mut fresh = App::new();
            let mut out = Vec::new();
            handle_server_event(&mut fresh, snapshot(blob), &mut out);
            assert!(fresh.term.is_none(), "{out:?}");
            assert!(!out
                .iter()
                .any(|r| matches!(r, ClientRequest::Attach { .. })));
        }
    }

    /// The footer's far left is a nameplate: which nebula this is, ahead
    /// of the workspace and every cursor-driven crumb after it. It yields
    /// the columns back to a flash that would otherwise be cut off mid
    /// sentence — a clipped key list is still readable, a clipped message
    /// is not.
    #[test]
    fn footer_shows_the_nebula_version_but_never_truncates_a_flash() {
        let stamp = concat!("nebula v", env!("CARGO_PKG_VERSION"));
        let mut app = App::new();
        seed_tree(&mut app);
        let mut terminal = Terminal::new(TestBackend::new(120, 30)).unwrap();
        terminal.draw(|f| ui::draw(f, &mut app)).unwrap();
        let text = buffer_text(&terminal);
        assert!(text.contains(stamp), "{stamp} missing from:\n{text}");

        // A flash short enough to share the bar keeps the nameplate.
        app.flash = Some("saved".into());
        terminal.draw(|f| ui::draw(f, &mut app)).unwrap();
        let text = buffer_text(&terminal);
        assert!(text.contains(stamp), "{stamp} missing from:\n{text}");
        assert!(text.contains("saved"), "{text}");

        // One that isn't takes the whole left edge instead.
        let long = "the pull request link can't be deleted from here, close it on github";
        app.flash = Some(long.into());
        terminal.draw(|f| ui::draw(f, &mut app)).unwrap();
        let text = buffer_text(&terminal);
        assert!(
            !text.contains(stamp),
            "nameplate should have yielded:\n{text}"
        );
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
                    prewarm: None,
                },
                SessionMetrics {
                    session: SessionRef::Terminal(TerminalId("t1".into())),
                    pid: 11,
                    rss_bytes: 24 * 1024 * 1024,
                    procs: 2,
                    prewarm: None,
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
        // Sized for the three panels alone; the Workspaces column is its own test.
        app.show_workspaces = false;
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
            !text.contains("TERMINALS"),
            "terminals section is gone:\n{text}"
        );
    }

    /// Every session row names its harness in a dim badge after the title —
    /// claude included, so the column doesn't read as "codex/cursor are the
    /// odd ones out".
    #[test]
    fn session_rows_badge_their_harness() {
        use nebula_core::{Agent, AgentKind, AgentStatus, Entity};
        let mut app = App::new();
        seed_tree(&mut app); // agent-1, claude
        for (i, kind) in [(2, AgentKind::Codex), (3, AgentKind::Cursor)] {
            hse(
                &mut app,
                ServerEvent::EntityUpserted {
                    entity: Entity::Agent(Agent {
                        id: AgentId(format!("a{i}")),
                        worktree_id: WorktreeId("w1".into()),
                        name: format!("agent-{i}"),
                        status: AgentStatus::Fresh,
                        archived: false,
                        archived_at: 0,
                        unseen: false,
                        kind,
                        model: None,
                        effort: None,
                        session_id: None,
                        cloud_session_id: None,
                        sort_order: i,
                        status_changed_at: 0,
                        alive: true,
                        cloud_mirroring: false,
                    }),
                },
            );
        }

        let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
        terminal.draw(|f| ui::draw(f, &mut app)).unwrap();
        let text = buffer_text(&terminal);
        for (name, kind) in [
            ("agent-1", "claude"),
            ("agent-2", "codex"),
            ("agent-3", "cursor"),
        ] {
            assert!(
                text.contains(&format!("{name} {kind}")),
                "{name} badged {kind}:\n{text}"
            );
        }

        // The badge is dim, the name isn't — it has to read as secondary.
        // (Checked on an unselected row: the selection bar brightens dim
        // spans to muted.)
        let th = app.theme;
        let buffer = terminal.backend().buffer();
        let (x, y) = find_cell(&terminal, "agent-2 codex");
        assert_eq!(buffer[(x, y)].fg, th.muted, "name stays muted");
        let badge_x = x + "agent-2 ".chars().count() as u16;
        assert_eq!(buffer[(badge_x, y)].fg, th.dim, "badge is dim");
    }

    /// Running / needs-feedback sessions head the list and hold their
    /// place there however long they have been working — an old status
    /// timestamp doesn't drop them below a fresher finish.
    #[test]
    fn working_sessions_head_the_list_regardless_of_age() {
        use nebula_core::{Agent, AgentStatus, Entity};
        let mut app = App::new();
        seed_tree(&mut app);
        let now = crate::app::now_ms();
        let stale = now - 2 * 3_600_000;
        let mk = |id: &str, status: AgentStatus, changed_at: i64, sort: i64| {
            ServerEvent::EntityUpserted {
                entity: Entity::Agent(Agent {
                    id: AgentId(id.into()),
                    worktree_id: WorktreeId("w1".into()),
                    name: id.into(),
                    status,
                    archived: false,
                    archived_at: 0,
                    unseen: false,
                    kind: nebula_core::AgentKind::Claude,
                    model: None,
                    effort: None,
                    session_id: None,
                    cloud_session_id: None,
                    sort_order: sort,
                    status_changed_at: changed_at,
                    alive: true,
                    cloud_mirroring: false,
                }),
            }
        };
        // Finished a moment ago: near the top on the timestamp alone.
        hse(
            &mut app,
            mk("just-finished", AgentStatus::Finished, now - 1_000, 1),
        );
        // Working for hours: still above it.
        hse(&mut app, mk("long-running", AgentStatus::Running, stale, 2));
        hse(
            &mut app,
            mk("long-blocked", AgentStatus::NeedsFeedback, stale, 3),
        );

        let rows = app.visible_sessions();
        let names: Vec<&str> = rows.iter().map(|a| a.name.as_str()).collect();
        assert_eq!(
            names,
            vec!["long-running", "long-blocked", "just-finished", "agent-1"],
            "working sessions on top, then the freshly-changed row, then the rest"
        );
        assert_eq!(app.session_group_counts(), (4, 0));
    }

    /// The list is ordered by last interaction, newest first — the session
    /// you just ran surfaces at the top, and sessions that have never run
    /// sink to the bottom in tree order. No group headers split it.
    #[test]
    fn sessions_order_by_last_interaction() {
        use nebula_core::{Agent, AgentStatus, Entity};
        let mut app = App::new();
        seed_tree(&mut app); // agent-1: fresh, never run (stamp 0)
        let now = crate::app::now_ms();
        let mins = |n: i64| now - n * 60_000;
        let mk = |id: &str, status: AgentStatus, at: i64, sort: i64| ServerEvent::EntityUpserted {
            entity: Entity::Agent(Agent {
                id: AgentId(id.into()),
                worktree_id: WorktreeId("w1".into()),
                name: id.into(),
                status,
                archived: false,
                archived_at: 0,
                unseen: false,
                kind: nebula_core::AgentKind::Claude,
                model: None,
                effort: None,
                session_id: None,
                cloud_session_id: None,
                sort_order: sort,
                status_changed_at: at,
                alive: true,
                cloud_mirroring: false,
            }),
        };
        // A long-running turn outranks a more recent finish, because a
        // working session is interacting with you right now. Seeded out of
        // stamp order on purpose.
        hse(&mut app, mk("working", AgentStatus::Running, mins(25), 1));
        hse(&mut app, mk("done-1m", AgentStatus::Finished, mins(1), 2));
        hse(&mut app, mk("done-10m", AgentStatus::Finished, mins(10), 3));
        hse(&mut app, mk("cold-2h", AgentStatus::Finished, mins(120), 4));
        hse(&mut app, mk("cold-45m", AgentStatus::Finished, mins(45), 5));

        let rows = app.visible_sessions();
        let names: Vec<&str> = rows.iter().map(|a| a.name.as_str()).collect();
        assert_eq!(
            names,
            vec![
                "working", "done-1m", "done-10m", // working on top, then newest first
                "cold-45m", "cold-2h", "agent-1", // never-run last
            ],
        );
        assert_eq!(app.session_group_counts(), (6, 0));

        let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
        terminal.draw(|f| ui::draw(f, &mut app)).unwrap();
        let text = buffer_text(&terminal);
        for header in ["PINNED", "RECENT", "UNPINNED"] {
            assert!(!text.contains(header), "no {header} header:\n{text}");
        }

        // A status flip is an interaction: the coldest row jumps the queue.
        hse(
            &mut app,
            ServerEvent::StatusChanged {
                agent: AgentId("cold-2h".into()),
                status: AgentStatus::Finished,
                changed_at: now,
                unseen: false,
            },
        );
        let rows = app.visible_sessions();
        let names: Vec<&str> = rows.iter().map(|a| a.name.as_str()).collect();
        assert_eq!(
            names,
            vec![
                "working", "cold-2h", "done-1m", "done-10m", //
                "cold-45m", "agent-1",
            ],
            "the cold row jumped to the top, behind only the live turn it ties with"
        );
    }

    /// Session rows carry how long since they last did anything, sat
    /// between the name and the harness badge. Never-run sessions have
    /// nothing to say, and a narrow panel spends its columns on the name.
    #[test]
    fn session_rows_show_time_since_last_interaction() {
        use nebula_core::{Agent, AgentStatus, Entity};
        let mut app = App::new();
        seed_tree(&mut app); // agent-1: never run
        hse(
            &mut app,
            ServerEvent::EntityUpserted {
                entity: Entity::Agent(Agent {
                    id: AgentId("a2".into()),
                    worktree_id: WorktreeId("w1".into()),
                    name: "alpha".into(),
                    status: AgentStatus::Finished,
                    archived: false,
                    archived_at: 0,
                    unseen: false,
                    kind: nebula_core::AgentKind::Claude,
                    model: None,
                    effort: None,
                    session_id: None,
                    cloud_session_id: None,
                    sort_order: 1,
                    status_changed_at: crate::app::now_ms() - 23 * 60_000,
                    alive: true,
                    cloud_mirroring: false,
                }),
            },
        );

        let row_with = |app: &mut App, needle: &str| -> String {
            let mut terminal = Terminal::new(TestBackend::new(120, 30)).unwrap();
            terminal.draw(|f| ui::draw(f, app)).unwrap();
            // From the name onward: the worktree row shares the screen
            // line and carries its own "23m ago".
            buffer_text(&terminal)
                .lines()
                .find_map(|l| l.find(needle).map(|i| l[i..].to_string()))
                .unwrap_or_default()
        };

        let row = row_with(&mut app, "alpha");
        let name = row.find("alpha").expect("the session name");
        let ago = row
            .find("23m ago")
            .unwrap_or_else(|| panic!("no ago label:\n{row}"));
        let harness = row.find("claude").expect("the harness badge");
        assert!(
            name < ago && ago < harness,
            "name, then how long ago, then the harness:\n{row}"
        );

        // A session that has never run has no interaction to report.
        let row = row_with(&mut app, "agent-1");
        assert!(!row.contains("ago"), "never-run row stays bare:\n{row}");

        // Squeeze the panel: the label drops rather than eat the name.
        app.panel_widths[2] = 20;
        let row = row_with(&mut app, "alpha");
        assert!(row.contains("claude"), "harness badge survives:\n{row}");
        assert!(!row.contains("ago"), "ago label yields to the name:\n{row}");
    }

    /// A StatusChanged delta stamps the agent's timestamp, pulls it to the
    /// top, and the selection follows the session it was on.
    #[test]
    fn status_change_resorts_and_selection_follows() {
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
                    unseen: false,
                    kind: nebula_core::AgentKind::Claude,
                    model: None,
                    effort: None,
                    session_id: None,
                    cloud_session_id: None,
                    sort_order: 1,
                    status_changed_at: 0,
                    alive: true,
                    cloud_mirroring: false,
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
                unseen: false,
            },
        );
        let rows = app.visible_sessions();
        assert_eq!(
            rows[0].name, "agent-2",
            "the stamped agent bubbled to the top"
        );
        assert_eq!(app.session_group_counts(), (2, 0));
        assert_eq!(app.sel_session, 0, "selection followed agent-2");
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
                    archived_at: 0,
                    unseen: false,
                    kind: nebula_core::AgentKind::Claude,
                    model: None,
                    effort: None,
                    session_id: None,
                    cloud_session_id: None,
                    sort_order: 0,
                    status_changed_at: 0,
                    alive: true,
                    cloud_mirroring: false,
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

    /// Pin the config to a temp file holding `json` for the duration of `f`.
    fn with_config_json<T>(json: &str, f: impl FnOnce() -> T) -> T {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        std::fs::write(&path, json).unwrap();
        crate::config::with_config_path(path, f)
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
            assert_eq!(
                menu.items.len(),
                3,
                "no Terminal row: NEW TERMINAL (`t`) already covers it"
            );
            assert_eq!(menu.items[0].label, "Claude");
            assert_eq!(menu.items[1].label, "Codex");
            assert_eq!(menu.items[2].label, "Cursor");
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
    fn tab_on_claude_toggles_cloud_and_collects_a_multiline_task() {
        with_default_config(|| {
            let mut app = App::new();
            seed_tree(&mut app);
            app.focus = Focus::Sessions;
            let mut out = Vec::new();

            press(&mut app, KeyCode::Char('n'), KeyModifiers::NONE, &mut out);
            press(&mut app, KeyCode::Tab, KeyModifiers::NONE, &mut out);
            let Some(Overlay::Menu(menu)) = &app.overlay else {
                panic!("expected new-session picker");
            };
            assert_eq!(menu.items[0].label, "Claude · cloud");
            assert!(matches!(
                &menu.items[0].action,
                MenuAction::NewAgentOfKind {
                    kind: AgentKind::Claude,
                    cloud: true,
                    ..
                }
            ));

            // Cloud creation is cold on purpose: a bare-Claude warm PTY
            // cannot be adopted because it never received --cloud + task.
            press(&mut app, KeyCode::Enter, KeyModifiers::NONE, &mut out);
            assert!(out.is_empty(), "cloud name entry must not prewarm: {out:?}");
            assert!(matches!(
                &app.overlay,
                Some(Overlay::Prompt(p)) if matches!(
                    &p.kind,
                    PromptKind::NewAgent {
                        kind: AgentKind::Claude,
                        cloud: true,
                        ..
                    }
                )
            ));

            // The usual name prompt stays in the flow. Accepting its empty
            // default opens one additional, multiline task prompt.
            press(&mut app, KeyCode::Enter, KeyModifiers::NONE, &mut out);
            let Some(Overlay::Prompt(prompt)) = &app.overlay else {
                panic!("expected Claude Cloud task prompt");
            };
            assert_eq!(prompt.title, "Claude Cloud task");
            assert!(prompt.is_multiline());

            assert!(paste_into_overlay(&mut app, "Fix auth"));
            press(&mut app, KeyCode::Enter, KeyModifiers::SHIFT, &mut out);
            assert!(paste_into_overlay(&mut app, "Run the tests"));
            press(
                &mut app,
                KeyCode::Char('j'),
                KeyModifiers::CONTROL,
                &mut out,
            );
            assert!(paste_into_overlay(&mut app, "Ship it"));
            let Some(Overlay::Prompt(prompt)) = &app.overlay else {
                panic!("task prompt closed while editing");
            };
            assert_eq!(prompt.input.as_str(), "Fix auth\nRun the tests\nShip it");

            press(&mut app, KeyCode::Enter, KeyModifiers::NONE, &mut out);
            assert!(app.overlay.is_none());
            assert!(matches!(
                out.as_slice(),
                [ClientRequest::CreateAgent {
                    kind: AgentKind::Claude,
                    cloud_prompt: Some(task),
                    auto_title: true,
                    ..
                }] if task == "Fix auth\nRun the tests\nShip it"
            ));
            assert!(
                !out.iter()
                    .any(|request| matches!(request, ClientRequest::PrewarmAgent { .. })),
                "cloud launch must never consume/refill the local warm slot: {out:?}"
            );

            let req_id = match &out[0] {
                ClientRequest::CreateAgent { req_id, .. } => *req_id,
                other => panic!("expected create request, got {other:?}"),
            };
            handle_server_event(
                &mut app,
                ServerEvent::Error {
                    req_id: Some(req_id),
                    message: "cloud unavailable — retry".into(),
                },
                &mut out,
            );
            assert_eq!(app.flash.as_deref(), Some("cloud unavailable — retry"));
            assert!(matches!(
                &app.overlay,
                Some(Overlay::Prompt(prompt))
                    if prompt.is_multiline()
                        && prompt.input.as_str() == "Fix auth\nRun the tests\nShip it"
            ));
        })
    }

    #[test]
    fn cloud_task_is_still_required_when_session_naming_is_skipped() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        std::fs::write(&path, r#"{"skip_session_naming": true}"#).unwrap();
        crate::config::with_config_path(path, || {
            let mut app = App::new();
            seed_tree(&mut app);
            app.focus = Focus::Sessions;
            let mut out = Vec::new();

            press(&mut app, KeyCode::Char('n'), KeyModifiers::NONE, &mut out);
            press(&mut app, KeyCode::Tab, KeyModifiers::NONE, &mut out);
            press(&mut app, KeyCode::Enter, KeyModifiers::NONE, &mut out);

            assert!(out.is_empty(), "the task dialog comes before creation");
            assert!(matches!(
                &app.overlay,
                Some(Overlay::Prompt(p)) if p.is_multiline()
            ));

            // Empty is validation, not dismissal: keep the dialog open so
            // the user can correct it in place.
            press(&mut app, KeyCode::Enter, KeyModifiers::NONE, &mut out);
            assert!(out.is_empty());
            assert!(matches!(
                &app.overlay,
                Some(Overlay::Prompt(p)) if p.is_multiline()
            ));

            let Some(Overlay::Prompt(prompt)) = &mut app.overlay else {
                unreachable!()
            };
            prompt.input.set_text("fix\0auth");
            press(&mut app, KeyCode::Enter, KeyModifiers::NONE, &mut out);
            assert_eq!(
                app.flash.as_deref(),
                Some("Claude Cloud task cannot contain NUL bytes")
            );
            assert!(matches!(&app.overlay, Some(Overlay::Prompt(p)) if p.is_multiline()));

            let Some(Overlay::Prompt(prompt)) = &mut app.overlay else {
                unreachable!()
            };
            prompt
                .input
                .set_text("x".repeat(MAX_CLOUD_PROMPT_BYTES + 1));
            press(&mut app, KeyCode::Enter, KeyModifiers::NONE, &mut out);
            assert_eq!(
                app.flash.as_deref(),
                Some("Claude Cloud task is too long (max 16 KiB)")
            );
            assert!(matches!(&app.overlay, Some(Overlay::Prompt(p)) if p.is_multiline()));
        });
    }

    #[test]
    fn claude_cloud_task_prompt_soft_wraps_instead_of_horizontally_scrolling() {
        let mut app = App::new();
        seed_tree(&mut app);
        app.overlay = Some(Overlay::Prompt(PromptDialog::new(
            "Claude Cloud task",
            "what should Claude do?",
            format!("BEGIN {} END", "word ".repeat(20)),
            PromptKind::ClaudeCloudTask {
                worktree: WorktreeId("w1".into()),
                name: String::new(),
                model: None,
                effort: None,
            },
        )));
        let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
        terminal.draw(|f| crate::ui::draw(f, &mut app)).unwrap();

        let begin = find_cell(&terminal, "BEGIN");
        let end = find_cell(&terminal, "END");
        assert_ne!(begin.1, end.1, "long task should wrap across rows");
        assert!(buffer_text(&terminal).contains("Shift+Enter/^J: newline"));

        let Some(Overlay::Prompt(prompt)) = &mut app.overlay else {
            unreachable!()
        };
        prompt.input.set_text("VISIBLE");
        let mut small = Terminal::new(TestBackend::new(32, 6)).unwrap();
        small.draw(|f| crate::ui::draw(f, &mut app)).unwrap();
        let text = buffer_text(&small);
        assert!(
            text.contains("VISIBLE"),
            "small prompt lost its editor: {text}"
        );
        assert!(text.contains("Esc") && text.contains("^J") && text.contains("Enter"));
    }

    /// With `skip_session_naming` on, picking the kind is the whole flow:
    /// no name prompt, the generated default name, and the same auto-title
    /// opt-in that accepting an empty prompt gives.
    #[test]
    fn skip_session_naming_creates_straight_from_the_picker() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        std::fs::write(&path, r#"{"skip_session_naming": true}"#).unwrap();
        crate::config::with_config_path(path, || {
            let mut app = App::new();
            seed_tree(&mut app);
            app.focus = Focus::Sessions;
            let mut out = Vec::new();

            press(&mut app, KeyCode::Char('n'), KeyModifiers::NONE, &mut out);
            assert!(
                matches!(app.overlay, Some(Overlay::Menu(_))),
                "kind picker still opens: {:?}",
                app.overlay
            );
            assert!(out.is_empty(), "opening the picker sends nothing: {out:?}");

            press(&mut app, KeyCode::Enter, KeyModifiers::NONE, &mut out);
            assert!(app.overlay.is_none(), "no name prompt: {:?}", app.overlay);
            assert!(matches!(
                &out[0],
                ClientRequest::CreateAgent {
                    name,
                    kind: AgentKind::Claude,
                    model: None,
                    effort: None,
                    auto_title: true,
                    ..
                } if name == "agent-2"
            ));
            // Only the refill behind the create — the warm-while-typing
            // prewarm has no typing to cover, so it never fires.
            assert!(matches!(
                out.last(),
                Some(ClientRequest::PrewarmAgent {
                    kind: AgentKind::Claude,
                    model: None,
                    effort: None,
                    ..
                })
            ));
            assert_eq!(
                out.iter()
                    .filter(|r| matches!(r, ClientRequest::PrewarmAgent { .. }))
                    .count(),
                1,
                "one prewarm, the refill: {out:?}"
            );
        })
    }

    /// The submenu picks still apply when the prompt is skipped: the model
    /// row Enter lands on is what the create carries.
    #[test]
    fn skip_session_naming_keeps_the_submenu_model_pick() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        std::fs::write(&path, r#"{"skip_session_naming": true}"#).unwrap();
        crate::config::with_config_path(path, || {
            let mut app = App::new();
            seed_tree(&mut app);
            app.focus = Focus::Sessions;
            let mut out = Vec::new();

            press(&mut app, KeyCode::Char('n'), KeyModifiers::NONE, &mut out);
            // → into Claude's model list, down to "opus", Enter.
            press(&mut app, KeyCode::Right, KeyModifiers::NONE, &mut out);
            let Some(Overlay::Menu(menu)) = &app.overlay else {
                panic!("expected model submenu, got {:?}", app.overlay);
            };
            let opus = menu
                .items
                .iter()
                .position(|i| i.label.starts_with("opus"))
                .expect("opus row");
            // Letters type ahead in a model submenu, so move with ↓.
            for _ in 0..opus {
                press(&mut app, KeyCode::Down, KeyModifiers::NONE, &mut out);
            }
            press(&mut app, KeyCode::Enter, KeyModifiers::NONE, &mut out);
            assert!(app.overlay.is_none(), "no name prompt: {:?}", app.overlay);
            assert!(matches!(
                &out[0],
                ClientRequest::CreateAgent {
                    kind: AgentKind::Claude,
                    model: Some(m),
                    auto_title: true,
                    ..
                } if m == "opus"
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
            // Every row advertises a model submenu (the ▸ affordance) —
            // Cursor too, since `cursor-agent --model` grew a catalogue.
            assert_eq!(menu.items[0].action.submenu(), Some(SubmenuKind::Models));
            assert_eq!(menu.items[1].action.submenu(), Some(SubmenuKind::Models));
            assert_eq!(menu.items[2].action.submenu(), Some(SubmenuKind::Models));

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

            // n → Codex row → models → Luna → efforts → minimal → Enter.
            press(&mut app, KeyCode::Char('n'), KeyModifiers::NONE, &mut out);
            press(&mut app, KeyCode::Down, KeyModifiers::NONE, &mut out);
            press(&mut app, KeyCode::Right, KeyModifiers::NONE, &mut out);
            let Some(Overlay::Menu(menu)) = &app.overlay else {
                panic!("expected codex model submenu");
            };
            assert_eq!(menu.title.as_deref(), Some("Codex model"));
            assert!(menu.items.iter().any(|item| item.label == "gpt-5.6-terra"));
            let luna = menu
                .items
                .iter()
                .position(|item| item.label.starts_with("gpt-5.6-luna"))
                .expect("Luna row");
            for _ in 0..luna {
                press(&mut app, KeyCode::Down, KeyModifiers::NONE, &mut out);
            }
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
                }) if m == "gpt-5.6-luna" && e == "minimal"
            ));
            let Some(Overlay::Prompt(p)) = &app.overlay else {
                panic!("expected name prompt, got {:?}", app.overlay);
            };
            assert_eq!(p.title, "New agent (gpt-5.6-luna · minimal)");
            press(&mut app, KeyCode::Enter, KeyModifiers::NONE, &mut out);
            assert!(matches!(
                out.last(),
                Some(ClientRequest::CreateAgent {
                    kind: AgentKind::Codex,
                    model: Some(m),
                    effort: Some(e),
                    ..
                }) if m == "gpt-5.6-luna" && e == "minimal"
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
        // The picker reads the harness toggles, so pin the config: a dev
        // whose real config.json hides a kind would shift the rows.
        with_default_config(|| {
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
        });
    }

    #[test]
    fn picker_third_row_creates_cursor_agent() {
        with_default_config(|| {
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
        });
    }

    // ---- harnesses disabled in Settings leave the picker ----

    #[test]
    fn picker_omits_a_disabled_harness() {
        with_config_json(r#"{"codex_enabled": false}"#, || {
            let mut app = App::new();
            seed_tree(&mut app);
            app.focus = Focus::Sessions;
            let mut out = Vec::new();

            press(&mut app, KeyCode::Char('n'), KeyModifiers::NONE, &mut out);
            let Some(Overlay::Menu(menu)) = &app.overlay else {
                panic!("expected the NEW SESSION PICKER, got {:?}", app.overlay);
            };
            let labels: Vec<&str> = menu.items.iter().map(|item| item.label.as_str()).collect();
            assert_eq!(labels, ["Claude", "Cursor"], "Codex is absent, not greyed");

            // The second row is now Cursor: the rows shift, nothing is dead.
            press(&mut app, KeyCode::Char('j'), KeyModifiers::NONE, &mut out);
            press(&mut app, KeyCode::Enter, KeyModifiers::NONE, &mut out);
            assert!(matches!(
                &app.overlay,
                Some(Overlay::Prompt(p)) if matches!(&p.kind, PromptKind::NewAgent { kind: AgentKind::Cursor, .. })
            ));
        });
    }

    #[test]
    fn picker_with_every_harness_disabled_flashes_instead_of_opening() {
        with_config_json(
            r#"{"claude_enabled": false, "codex_enabled": false, "cursor_enabled": false}"#,
            || {
                let mut app = App::new();
                seed_tree(&mut app);
                app.focus = Focus::Sessions;
                let mut out = Vec::new();

                press(&mut app, KeyCode::Char('n'), KeyModifiers::NONE, &mut out);
                assert!(app.overlay.is_none(), "an empty picker must never open");
                let flash = app.flash.as_deref().expect("a flash says why");
                assert!(flash.contains("Settings"), "{flash}");

                // The CONTEXT MENU's "New agent" row lands on the same guard.
                app.flash = None;
                let worktree = app.selected_worktree().unwrap().id.clone();
                run_menu_action(&mut app, MenuAction::NewAgent(worktree), &mut out);
                assert!(app.overlay.is_none());
                assert!(app.flash.is_some());
            },
        );
    }

    #[test]
    fn disabled_claude_blocks_pr_session_and_hides_its_menu_row() {
        with_config_json(r#"{"claude_enabled": false}"#, || {
            let mut app = App::new();
            seed_tree(&mut app);
            seed_open_prs(&mut app, &[(7, "Attach links")]);
            app.focus = Focus::Worktrees;
            app.sel_worktree = 1;
            let mut out = Vec::new();

            press(&mut app, KeyCode::Char('n'), KeyModifiers::NONE, &mut out);
            assert!(app.overlay.is_none(), "no PR SESSION picker without Claude");
            let flash = app.flash.as_deref().expect("a flash says why");
            assert!(flash.contains("Claude is disabled"), "{flash}");

            open_context_menu_for_selection(&mut app);
            let Some(Overlay::Menu(menu)) = &app.overlay else {
                panic!("expected the OPEN PRS context menu, got {:?}", app.overlay);
            };
            let labels: Vec<&str> = menu.items.iter().map(|item| item.label.as_str()).collect();
            assert_eq!(labels, ["Open in browser", "View diff"]);

            app.overlay = None;
            app.hits.push((
                ratatui::layout::Rect::new(0, 0, 20, 2),
                HitTarget::Worktree(1),
            ));
            handle_mouse(
                &mut app,
                MouseEvent {
                    kind: MouseEventKind::Down(MouseButton::Right),
                    column: 1,
                    row: 1,
                    modifiers: KeyModifiers::NONE,
                },
                &mut out,
            );
            let Some(Overlay::Menu(menu)) = &app.overlay else {
                panic!("expected the right-click menu, got {:?}", app.overlay);
            };
            assert!(
                menu.items
                    .iter()
                    .all(|item| item.label != "New Claude session"),
                "{:?}",
                menu.items.iter().map(|i| &i.label).collect::<Vec<_>>()
            );
        });
    }

    #[test]
    fn disabled_claude_skips_the_standing_prewarm() {
        with_config_json(r#"{"claude_enabled": false}"#, || {
            let mut app = App::new();
            seed_tree(&mut app);
            let worktree = app.selected_worktree().unwrap().id.clone();
            app.pending_prewarm = Some((worktree.clone(), std::time::Instant::now()));
            let mut out = Vec::new();

            fire_pending_prewarm(&mut app, &mut out);
            assert!(
                matches!(
                    out.as_slice(),
                    [ClientRequest::PrewarmWorktreeSessions { .. }]
                ),
                "dead sessions still prewarm; no Claude WARM SPARE: {out:?}"
            );

            out.clear();
            fire_keepwarm(&mut app, &mut out);
            assert!(
                out.is_empty(),
                "keep-warm sends nothing for a disabled harness: {out:?}"
            );
            assert!(
                app.next_keepwarm.is_some(),
                "still re-armed, so re-enabling warms again"
            );
        });
    }

    #[test]
    fn agents_tab_toggles_a_harness_and_refuses_the_last_one() {
        use crate::config::{locate, SettingKind};
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        crate::config::with_config_path(path.clone(), || {
            let (tab, claude_row) = locate(SettingKind::ClaudeEnabled).unwrap();
            let (_, codex_row) = locate(SettingKind::CodexEnabled).unwrap();
            let (_, cursor_row) = locate(SettingKind::CursorEnabled).unwrap();
            let mut app = App::new();
            let mut out = Vec::new();
            open_settings_on(&mut app, tab, &mut out);
            for _ in 0..claude_row {
                press(&mut app, KeyCode::Down, KeyModifiers::NONE, &mut out);
            }
            press(&mut app, KeyCode::Enter, KeyModifiers::NONE, &mut out);
            let saved: serde_json::Value =
                serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
            assert_eq!(saved["claude_enabled"], false, "Enter toggles Claude off");
            assert_eq!(saved["codex_enabled"], true);

            for _ in claude_row..codex_row {
                press(&mut app, KeyCode::Down, KeyModifiers::NONE, &mut out);
            }
            press(&mut app, KeyCode::Enter, KeyModifiers::NONE, &mut out);
            assert!(!crate::config::Config::load().codex_enabled);

            for _ in codex_row..cursor_row {
                press(&mut app, KeyCode::Down, KeyModifiers::NONE, &mut out);
            }
            press(&mut app, KeyCode::Enter, KeyModifiers::NONE, &mut out);
            let cfg = crate::config::Config::load();
            assert!(
                cfg.cursor_enabled,
                "the last harness cannot be switched off"
            );
            assert_eq!(cfg.enabled_kinds(), vec![AgentKind::Cursor]);
            let (text, level) = settings_view(&app).notice.clone().expect("a warning");
            assert!(matches!(level, crate::app::NoticeLevel::Warn));
            assert!(text.contains("at least one harness"), "{text}");
            assert!(
                matches!(app.overlay, Some(Overlay::Settings(_))),
                "the refusal keeps the overlay open"
            );
        });
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

    /// Adding a project lands on it: its row, its main checkout, and the
    /// Worktrees panel — whether the upsert arrives before the Ack (the
    /// daemon's usual order) or after it.
    #[test]
    fn add_project_ack_selects_the_new_project() {
        use crate::app::{Overlay, PromptDialog, PromptKind};
        use nebula_core::{Entity, ProjectId, Worktree};
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join("herdr")).unwrap();

        for upsert_first in [true, false] {
            let mut app = App::new();
            seed_tree(&mut app); // p1 / w1(main) / a1, selected
            app.focus = Focus::Sessions;
            let mut out = Vec::new();
            app.overlay = Some(Overlay::Prompt(PromptDialog::new(
                "Add project",
                "path",
                format!("{}/herdr", tmp.path().display()),
                PromptKind::AddProject,
            )));
            handle_key(
                &mut app,
                KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
                &mut out,
            );
            let Some(ClientRequest::AddProject { req_id, .. }) = out.last() else {
                panic!("expected AddProject, got {:?}", out.last());
            };
            let req_id = *req_id;

            let upsert = |app: &mut App| {
                hse(
                    app,
                    ServerEvent::EntityUpserted {
                        entity: project("p2", "herdr", 1),
                    },
                );
                hse(
                    app,
                    ServerEvent::EntityUpserted {
                        entity: Entity::Worktree(Worktree {
                            id: WorktreeId("w2".into()),
                            project_id: ProjectId("p2".into()),
                            path: "/tmp/herdr".into(),
                            branch: "main".into(),
                            is_main: true,
                            sort_order: 0,
                        }),
                    },
                );
            };
            if upsert_first {
                upsert(&mut app);
            }
            hse(
                &mut app,
                ServerEvent::Ack {
                    req_id,
                    created: Some(EntityId::Project(ProjectId("p2".into()))),
                },
            );
            if !upsert_first {
                assert_eq!(
                    app.select_project_when_seen,
                    Some(ProjectId("p2".into())),
                    "an Ack ahead of the upsert waits for it"
                );
                upsert(&mut app);
            }

            assert_eq!(
                app.selected_project().map(|p| p.id.clone()),
                Some(ProjectId("p2".into())),
                "upsert_first={upsert_first}: the new project is selected"
            );
            assert_eq!(
                app.selected_worktree().map(|w| w.id.clone()),
                Some(WorktreeId("w2".into())),
                "upsert_first={upsert_first}: on its main checkout"
            );
            assert_eq!(app.focus, Focus::Worktrees);
            assert_eq!(app.select_project_when_seen, None);
        }

        let mut app = App::new();
        seed_tree(&mut app);
        app.hide_worktrees = true;
        let mut out = Vec::new();
        assert!(select_created_project(
            &mut app,
            &ProjectId("p1".into()),
            &mut out
        ));
        assert_eq!(
            app.focus,
            Focus::Sessions,
            "a hidden Worktrees panel is skipped after project creation"
        );
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
    fn pull_requests_and_legacy_links_render_under_open_prs() {
        let mut app = App::new();
        // Sized for the three panels alone; the Workspaces column is its own test.
        app.show_workspaces = false;
        seed_tree(&mut app);
        seed_link(&mut app, "https://example.dev/spec");
        app.pull_requests.insert(
            nebula_core::WorktreeId("w1".into()),
            Some(crate::pull_request::PullRequest {
                number: 7,
                url: "https://github.com/o/r/pull/7".into(),
                title: "Attach links".into(),
                state: "OPEN".into(),
                is_draft: false,
                activity: Vec::new(),
            }),
        );

        let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
        terminal.draw(|f| ui::draw(f, &mut app)).unwrap();
        let text = buffer_text(&terminal);
        assert!(text.contains("OPEN PRS"), "open-PR header:\n{text}");
        assert!(
            text.contains("#7 Attach links"),
            "pull request row:\n{text}"
        );
        assert!(
            text.contains("example.dev/spec"),
            "previously saved link row (scheme stripped):\n{text}"
        );
        // The panel's count is a session count; the two link rows don't
        // inflate it.
        assert!(text.contains("SESSIONS · 1"), "session count:\n{text}");
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

    /// The context menu's Delete / Close / Remove rows open the very confirm
    /// the `d` key does — same title, same message, same pending action —
    /// so the two routes can never drift apart in wording.
    #[test]
    fn menu_confirms_match_the_key_path_word_for_word() {
        use nebula_core::{ProjectId, TerminalId};
        let mut app = App::new();
        seed_tree(&mut app);
        seed_terminal(&mut app, "t1", "term-1");

        fn take_confirm(app: &mut App) -> ConfirmDialog {
            match app.overlay.take() {
                Some(Overlay::Confirm(c)) => c,
                other => panic!("expected a confirm, got {other:?}"),
            }
        }
        let via_key = |app: &mut App, focus: Focus, row: usize| {
            app.focus = focus;
            app.sel_session = row;
            press(app, KeyCode::Char('d'), KeyModifiers::NONE, &mut Vec::new());
            take_confirm(app)
        };
        let key_agent = via_key(&mut app, Focus::Sessions, 0);
        let key_term = via_key(&mut app, Focus::Sessions, 1);
        let key_project = via_key(&mut app, Focus::Projects, 0);

        let via_menu = |app: &mut App, action: MenuAction| {
            run_menu_action(app, action, &mut Vec::new());
            take_confirm(app)
        };
        let menu_agent = via_menu(&mut app, MenuAction::DeleteAgent(AgentId("a1".into())));
        let menu_term = via_menu(&mut app, MenuAction::CloseTerminal(TerminalId("t1".into())));
        let menu_project = via_menu(&mut app, MenuAction::RemoveProject(ProjectId("p1".into())));

        for (key, menu) in [
            (&key_agent, &menu_agent),
            (&key_term, &menu_term),
            (&key_project, &menu_project),
        ] {
            assert_eq!(key.title, menu.title);
            assert_eq!(key.message, menu.message);
        }
        assert_eq!(key_agent.title, "Delete agent");
        assert_eq!(
            key_agent.message,
            "Delete agent 'agent-1'? Its session and history go away."
        );
        assert!(matches!(menu_agent.action, PendingAction::DeleteAgent(_)));
        assert_eq!(key_term.title, "Close terminal");
        assert_eq!(
            key_term.message,
            "Close terminal 'term-1'? Its shell is killed."
        );
        assert!(matches!(menu_term.action, PendingAction::CloseTerminal(_)));
        assert_eq!(key_project.title, "Remove project");
        assert_eq!(
            key_project.message,
            "Remove 'demo' from nebula? Nothing on disk is touched."
        );
        assert!(matches!(
            menu_project.action,
            PendingAction::RemoveProject(_)
        ));
    }

    /// A paste into a locked pane reaches the PTY wrapped in the
    /// bracketed-paste markers, so the child (claude, vim…) takes it as one
    /// block instead of keystrokes to auto-indent. Unlocked, it goes nowhere.
    #[test]
    fn paste_into_a_locked_pane_is_bracketed() {
        let mut app = App::new();
        seed_tree(&mut app);
        let mut out = Vec::new();
        let sref = SessionRef::Agent(AgentId("a1".into()));
        app.term = Some(AttachedTerm::new(sref.clone(), 80, 24));
        app.focus = Focus::Terminal;
        app.term_locked = true;

        handle_terminal_event(&mut app, Event::Paste("fn main() {}\n".into()), &mut out);
        match out.as_slice() {
            [ClientRequest::Input { session, data }] => {
                assert_eq!(session, &sref);
                assert_eq!(data, b"\x1b[200~fn main() {}\n\x1b[201~");
            }
            other => panic!("expected one Input request, got {other:?}"),
        }

        out.clear();
        app.term_locked = false;
        handle_terminal_event(&mut app, Event::Paste("x".into()), &mut out);
        assert!(out.is_empty(), "an unlocked pane takes no paste: {out:?}");
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

        // Ctrl+Shift+H is the same key that walks back a panel when nothing
        // is locked, so inside a locked session it means the same thing:
        // leave. Kitty-protocol emulators only — crossterm may spell it
        // either as 'H' or as shift + 'h', and `from_event` folds both.
        for code in [KeyCode::Char('H'), KeyCode::Char('h')] {
            app.focus = Focus::Terminal;
            app.term_locked = true;
            handle_key(
                &mut app,
                KeyEvent::new(code, KeyModifiers::CONTROL | KeyModifiers::SHIFT),
                &mut out,
            );
            assert_eq!(
                app.focus,
                Focus::Sessions,
                "Ctrl+Shift+{code:?} leaves terminal input"
            );
            assert!(!app.term_locked, "Ctrl+Shift+{code:?} clears the lock");
            assert!(out.is_empty(), "Ctrl+Shift+{code:?} must not reach the pty");
        }

        // Ctrl+Shift+L is not a hatch — it walks forward, and forward from a
        // locked pane is nowhere. It stays in the session.
        app.focus = Focus::Terminal;
        app.term_locked = true;
        handle_key(
            &mut app,
            KeyEvent::new(
                KeyCode::Char('L'),
                KeyModifiers::CONTROL | KeyModifiers::SHIFT,
            ),
            &mut out,
        );
        assert_eq!(app.focus, Focus::Terminal, "Ctrl+Shift+L does not escape");
        assert!(app.term_locked, "Ctrl+Shift+L keeps the input lock");
        out.clear();

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

    /// Plain → stops at the Sessions panel; double-tapped there it jumps
    /// on into the terminal pane and takes its input, exactly as Tab / ^⇧L
    /// do — the preview under the cursor is already the session the user
    /// picked. Once in, an unlocked pane (Ctrl+→) locks on →,→ too.
    #[test]
    fn double_tapped_right_at_sessions_enters_the_pane_and_locks_it() {
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
        assert_eq!(
            app.focus,
            Focus::Sessions,
            "a single → must not enter the pane"
        );
        assert_eq!(app.flash.as_deref(), Some("→ again: enter pane"));
        press(&mut app, KeyCode::Right, KeyModifiers::NONE, &mut out);
        assert_eq!(
            app.focus,
            Focus::Terminal,
            "→,→ at sessions enters the pane"
        );
        assert!(app.term_locked, "and takes the input lock, like Tab");

        app.term_locked = false;
        press(&mut app, KeyCode::Right, KeyModifiers::NONE, &mut out);
        assert!(!app.term_locked, "one → on an unlocked pane stays unlocked");
        assert_eq!(app.flash.as_deref(), Some("→ again: type into terminal"));
        press(&mut app, KeyCode::Right, KeyModifiers::NONE, &mut out);
        assert!(
            app.term_locked,
            "→,→ on an unlocked pane takes the lock, like ^⇧L"
        );
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
                unseen: false,
                kind: nebula_core::AgentKind::Claude,
                model: None,
                effort: None,
                session_id: None,
                cloud_session_id: None,
                sort_order: sort,
                status_changed_at: 0,
                alive: true,
                cloud_mirroring: false,
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
        // The pane swaps at once, but the Attach waits for the cursor to
        // settle — walking a list must not boot a CLI per row passed.
        assert!(
            !out.iter()
                .any(|r| matches!(r, ClientRequest::Attach { .. })),
            "the walk itself attaches nothing: {out:?}"
        );
        assert_eq!(
            app.pending_attach.as_ref().map(|(s, _)| s.clone()),
            Some(a2.clone()),
            "the attach is armed for the walked-to session"
        );
        fire_pending_attach(&mut app, &mut out);
        assert!(
            matches!(out.last(), Some(ClientRequest::Attach { session, .. }) if *session == a2),
            "settling attaches so scrollback streams in: {out:?}"
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
            unseen: false,
            archived_at,
            kind: nebula_core::AgentKind::Claude,
            model: None,
            effort: None,
            session_id: None,
            cloud_session_id: None,
            sort_order: sort,
            status_changed_at: 0,
            alive: false,
            cloud_mirroring: false,
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
                SessionRow::Terminal(_) | SessionRow::Link(_) => unreachable!(),
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
        // Sized for the three panels alone; the Workspaces column is its own test.
        app.show_workspaces = false;
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
        assert!(
            text.contains("archived-00"),
            "list starts at the top: {text}"
        );
        assert!(!text.contains("archived-19"), "tail overflows: {text}");

        // Wheel over the Sessions column: the list moves, the cursor stays.
        for _ in 0..12 {
            handle_mouse(&mut app, mev(MouseEventKind::ScrollDown, 50, 10), &mut out);
            terminal.draw(|f| ui::draw(f, &mut app)).unwrap();
        }
        let text = buffer_text(&terminal);
        assert!(
            text.contains("archived-19"),
            "wheel reaches the tail: {text}"
        );
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
            crate::app::RowKey::Session(SessionRef::Agent(AgentId("a1".into()))),
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
    /// default panel widths: splitters 0..=2 at x = 20, 42, 68. The
    /// Workspaces bar runs across the top and owns no vertical boundary, so
    /// these x's hold whether or not it is shown.
    fn seed_splitters(app: &mut App) {
        app.body_area = ratatui::layout::Rect::new(0, 0, 120, 35);
        for i in app.splitter_indices() {
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
        assert_eq!(
            app.panel_widths,
            [30, 22, crate::app::DEFAULT_PANEL_WIDTHS[2]]
        );

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
        assert_eq!(
            app.panel_widths[1..],
            [22, crate::app::DEFAULT_PANEL_WIDTHS[2]],
            "only panel 0 moved"
        );
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
    fn base64_encodes_every_padding_case() {
        // RFC 4648 vectors — the OSC 52 payload is unusable if padding slips.
        assert_eq!(base64_encode(b""), "");
        assert_eq!(base64_encode(b"f"), "Zg==");
        assert_eq!(base64_encode(b"fo"), "Zm8=");
        assert_eq!(base64_encode(b"foo"), "Zm9v");
        assert_eq!(base64_encode(b"foob"), "Zm9vYg==");
        assert_eq!(base64_encode(b"fooba"), "Zm9vYmE=");
        assert_eq!(base64_encode(b"foobar"), "Zm9vYmFy");
        // Non-ASCII selections go over the wire as UTF-8 bytes.
        assert_eq!(base64_encode("→".as_bytes()), "4oaS");
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

        // Old blobs without the field keep the defaults — including ones
        // still carrying the retired `workspaces_w` of the column era,
        // which deserializes as an ignored extra key.
        let mut legacy = App::new();
        restore_ui_state(
            &mut legacy,
            r#"{"project":null,"worktree":null,"session_agent":null,"show_archived":false,"collapsed":false,"workspaces_w":26}"#,
        );
        assert_eq!(legacy.panel_widths, crate::app::DEFAULT_PANEL_WIDTHS);
    }

    fn project(id: &str, name: &str, sort_order: i64) -> nebula_core::Entity {
        use nebula_core::{Entity, Project, ProjectId};
        Entity::Project(Project {
            workspace_id: Default::default(),
            id: ProjectId(id.into()),
            name: name.into(),
            repo_path: format!("/tmp/{name}").into(),
            sort_order,
        })
    }

    /// The daemon re-homes rows on its own — a `nebula worktree` run
    /// inside the session, or a hook cwd that walked into another checkout.
    /// The selected session must not vanish from under the cursor when
    /// that happens: the selection follows it into its new worktree.
    #[test]
    fn selection_follows_the_selected_agent_when_the_daemon_rehomes_it() {
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
        let moved = |worktree: &str| Agent {
            id: AgentId("a1".into()),
            worktree_id: WorktreeId(worktree.into()),
            name: "agent-1".into(),
            status: AgentStatus::Fresh,
            archived: false,
            archived_at: 0,
            unseen: false,
            kind: nebula_core::AgentKind::Claude,
            model: None,
            effort: None,
            session_id: None,
            cloud_session_id: None,
            sort_order: 0,
            status_changed_at: 0,
            alive: true,
            cloud_mirroring: false,
        };

        // a1 is the selected session; its upsert lands under w2.
        assert_eq!(app.selected_session().map(|a| a.id.0), Some("a1".into()));
        hse(
            &mut app,
            ServerEvent::EntityUpserted {
                entity: Entity::Agent(moved("w2")),
            },
        );
        assert_eq!(
            app.selected_worktree().map(|w| w.branch.clone()),
            Some("feat".into()),
            "worktree selection followed the re-homed agent"
        );
        assert_eq!(app.selected_session().map(|a| a.id.0), Some("a1".into()));
        assert!(app.select_when_seen.is_none(), "follow intent consumed");

        // An agent that is NOT selected moving elsewhere leaves the cursor
        // where the user put it.
        app.sel_worktree = 0;
        hse(
            &mut app,
            ServerEvent::EntityUpserted {
                entity: Entity::Agent(moved("w1")),
            },
        );
        app.sel_session = 0;
        let before = app.selected_worktree().map(|w| w.id.clone());
        hse(
            &mut app,
            ServerEvent::EntityUpserted {
                entity: Entity::Agent(Agent {
                    id: AgentId("a9".into()),
                    ..moved("w1")
                }),
            },
        );
        hse(
            &mut app,
            ServerEvent::EntityUpserted {
                entity: Entity::Agent(Agent {
                    id: AgentId("a9".into()),
                    ..moved("w2")
                }),
            },
        );
        assert_eq!(app.selected_worktree().map(|w| w.id.clone()), before);
        assert!(app.select_when_seen.is_none());
    }

    /// `r` in the Projects panel retitles the row: the prompt opens on the
    /// current name and the request carries a name and nothing else —
    /// renaming a project never moves the folder it points at.
    #[test]
    fn r_renames_the_selected_project_row() {
        use nebula_core::ProjectId;
        let mut app = App::new();
        seed_tree(&mut app); // p1 "demo" at /tmp/demo
        let mut out = Vec::new();
        app.focus = Focus::Projects;

        press(&mut app, KeyCode::Char('r'), KeyModifiers::NONE, &mut out);
        match &app.overlay {
            Some(Overlay::Prompt(p)) => {
                assert_eq!(
                    p.kind,
                    PromptKind::RenameProject {
                        id: ProjectId("p1".into())
                    }
                );
                assert_eq!(p.input.as_str(), "demo", "prefilled with the current name");
            }
            other => panic!("r: {other:?}"),
        }

        // Retype it and submit.
        press(
            &mut app,
            KeyCode::Char('u'),
            KeyModifiers::CONTROL,
            &mut out,
        );
        for c in "Acme API".chars() {
            press(&mut app, KeyCode::Char(c), KeyModifiers::NONE, &mut out);
        }
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE, &mut out);
        assert!(
            matches!(
                out.last(),
                Some(ClientRequest::RenameProject { id, name, .. })
                    if id.as_str() == "p1" && name == "Acme API"
            ),
            "expected RenameProject, got {out:?}"
        );
    }

    /// Submitting an empty name undoes the rename: the row goes back to the
    /// folder's own name. `submit_prompt` cancels empty input for most
    /// prompts, so this is the arm that has to opt out of that — the daemon
    /// has always handled the reset, but the request never reached it.
    #[test]
    fn renaming_a_project_to_nothing_undoes_the_rename() {
        let mut app = App::new();
        seed_tree(&mut app);
        app.tree.projects[0].name = "Acme API".into();
        app.tree.projects[0].repo_path = "/tmp/acme-repo".into();
        let mut out = Vec::new();
        app.focus = Focus::Projects;

        press(&mut app, KeyCode::Char('r'), KeyModifiers::NONE, &mut out);
        press(
            &mut app,
            KeyCode::Char('u'),
            KeyModifiers::CONTROL,
            &mut out,
        );
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE, &mut out);

        assert!(
            matches!(
                out.last(),
                Some(ClientRequest::RenameProject { id, name, .. })
                    if id.as_str() == "p1" && name.is_empty()
            ),
            "an empty name is the undo, not a cancel: {out:?} flash={:?}",
            app.flash
        );
        assert!(
            app.overlay.is_none(),
            "the prompt closes: {:?}",
            app.overlay
        );
    }

    /// A renamed project keeps the folder it lives in visible: the folder
    /// name takes the row directly under the new one, and the row grows by
    /// exactly that line. An unrenamed row repeats nothing.
    ///
    /// The two lines are a hierarchy, not a pair — a terminal cell has one
    /// font size, so "smaller" is carried by weight, opacity and position:
    /// the chosen label is BOLD in full-strength text, the folder hangs off
    /// a `└ ` in `dim` *plus* DIM (SGR 2, faint). Assert the styles, because
    /// `buffer_text` can't see them and a silent loss of either flattens
    /// the row.
    #[test]
    fn a_renamed_project_shows_its_folder_name_underneath() {
        use ratatui::style::Modifier;
        let mut app = App::new();
        seed_tree(&mut app); // p1 "demo" at /tmp/demo
        seed_default_workspace(&mut app);
        app.show_workspaces = false;
        let th = app.theme;
        let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
        let draw = |app: &mut App, terminal: &mut Terminal<TestBackend>| {
            terminal.draw(|f| ui::draw(f, app)).unwrap();
            buffer_text(terminal)
        };

        // Unrenamed: the folder name is the row's name, so it appears once.
        let text = draw(&mut app, &mut terminal);
        let rows: Vec<&str> = text.lines().collect();
        let name_row = rows
            .iter()
            .position(|l| l.contains("demo"))
            .unwrap_or_else(|| panic!("no project row:\n{text}"));
        assert!(
            !rows[name_row + 1].contains("demo"),
            "nothing under an unrenamed row:\n{text}"
        );

        // Two renamed rows, so both the selected and the unselected style
        // are on screen in one draw. The cursor starts on the first.
        hse(
            &mut app,
            ServerEvent::EntityUpserted {
                entity: project("p1", "Acme API", 0),
            },
        );
        app.tree.projects[0].repo_path = "/tmp/acme-repo".into();
        hse(
            &mut app,
            ServerEvent::EntityUpserted {
                entity: project("p2", "Side Quest", 1),
            },
        );
        app.tree.projects[1].repo_path = "/tmp/side-repo".into();
        assert_eq!(app.sel_project, 0, "cursor on the first row");

        let text = draw(&mut app, &mut terminal);
        let rows: Vec<&str> = text.lines().collect();
        let name_row = rows
            .iter()
            .position(|l| l.contains("Acme API"))
            .unwrap_or_else(|| panic!("renamed row missing:\n{text}"));
        assert!(
            rows[name_row + 1].contains("└ acme-repo"),
            "the folder hangs off the label on the next row:\n{text}"
        );
        // The glyph lands under the name's first letter, not the dot.
        let (label_x, _) = find_cell(&terminal, "Acme API");
        let (glyph_x, _) = find_cell(&terminal, "└ acme-repo");
        assert_eq!(label_x, glyph_x, "`└` is flush with the label:\n{text}");

        // The label leads on weight and never goes faint.
        let (x, y) = find_cell(&terminal, "Acme API");
        let label = terminal.backend().buffer()[(x, y)].style();
        assert!(
            label.add_modifier.contains(Modifier::BOLD),
            "the chosen label is the bold one: {label:?}"
        );
        assert!(
            !label.add_modifier.contains(Modifier::DIM),
            "and never faint: {label:?}"
        );

        // Unselected: the dimmest color the theme has, plus faint.
        let (x, y) = find_cell(&terminal, "side-repo");
        let sub = terminal.backend().buffer()[(x, y)].style();
        assert_eq!(sub.fg, Some(th.dim), "folder takes the dimmest color");
        assert!(
            sub.add_modifier.contains(Modifier::DIM),
            "and the faint attribute on top of it: {sub:?}"
        );
        assert!(
            !sub.add_modifier.contains(Modifier::BOLD),
            "never bold: {sub:?}"
        );

        // Selected, the fill would swallow a `dim` foreground, so
        // `render_button` lifts it to muted — the faint attribute has to
        // survive that lift or the hierarchy flattens on the cursor row.
        let (x, y) = find_cell(&terminal, "acme-repo");
        let sub = terminal.backend().buffer()[(x, y)].style();
        assert_eq!(sub.fg, Some(th.muted), "lifted off the selection fill");
        assert!(
            sub.add_modifier.contains(Modifier::DIM),
            "still faint on the selected row: {sub:?}"
        );
    }

    /// The shifted keys used to reorder projects; the column orders itself
    /// now, so they neither send a request nor move anything.
    #[test]
    fn shifted_keys_no_longer_reorder_projects() {
        let mut app = App::new();
        seed_tree(&mut app);
        hse(
            &mut app,
            ServerEvent::EntityUpserted {
                entity: project("p2", "two", 1),
            },
        );
        let mut out = Vec::new();
        app.focus = Focus::Projects;
        for key in [
            KeyEvent::new(KeyCode::Down, KeyModifiers::SHIFT),
            KeyEvent::new(KeyCode::Char('J'), KeyModifiers::SHIFT),
            KeyEvent::new(KeyCode::Char('K'), KeyModifiers::SHIFT),
        ] {
            handle_key(&mut app, key, &mut out);
        }
        assert!(out.is_empty(), "no reorder request goes out: {out:?}");
        let order: Vec<&str> = app
            .project_rows()
            .into_iter()
            .map(|i| app.tree.projects[i].name.as_str())
            .collect();
        assert_eq!(order, ["demo", "two"], "nothing moved");
    }

    /// A finished-or-fresh agent under `wt`, stamped `at`.
    fn agent_stamped(id: &str, wt: &str, at: i64) -> nebula_core::Entity {
        use nebula_core::{Agent, AgentStatus, Entity};
        Entity::Agent(Agent {
            id: AgentId(id.into()),
            worktree_id: WorktreeId(wt.into()),
            name: id.into(),
            status: if at > 0 {
                AgentStatus::Finished
            } else {
                AgentStatus::Fresh
            },
            archived: false,
            archived_at: 0,
            unseen: false,
            kind: nebula_core::AgentKind::Claude,
            model: None,
            effort: None,
            session_id: None,
            cloud_session_id: None,
            sort_order: 0,
            status_changed_at: at,
            alive: true,
            cloud_mirroring: false,
        })
    }

    /// Projects list most-recently-interacted first — the newest stamp of
    /// any session under the project — and every cursor follows its row
    /// across the re-sort. Never-run projects keep tree order at the
    /// bottom.
    #[test]
    fn projects_sort_by_last_interaction_and_selection_follows() {
        use nebula_core::AgentStatus;
        let mut app = App::new();
        seed_tree(&mut app); // p1 "demo" / w1 / a1 (never run)
        hse(
            &mut app,
            ServerEvent::EntityUpserted {
                entity: project("p2", "two", 1),
            },
        );
        hse(
            &mut app,
            ServerEvent::EntityUpserted {
                entity: wt_entity("w2", "p2", "main", true),
            },
        );
        hse(
            &mut app,
            ServerEvent::EntityUpserted {
                entity: agent_stamped("a2", "w2", 0),
            },
        );
        let names = |app: &App| -> Vec<String> {
            app.project_rows()
                .into_iter()
                .map(|i| app.tree.projects[i].name.clone())
                .collect()
        };
        assert_eq!(
            names(&app),
            ["demo", "two"],
            "never-run rows keep tree order"
        );

        // Rest on "two", then its session finishes a turn: the project
        // heads the column and the cursor stays on it.
        app.focus = Focus::Projects;
        app.sel_project = 1;
        app.sel_worktree = 0;
        app.sel_session = 0;
        let now = crate::app::now_ms();
        hse(
            &mut app,
            ServerEvent::StatusChanged {
                agent: AgentId("a2".into()),
                status: AgentStatus::Finished,
                changed_at: now - 60_000,
                unseen: false,
            },
        );
        assert_eq!(names(&app), ["two", "demo"]);
        assert_eq!(
            app.sel_project, 0,
            "selection follows the project it was on"
        );
        assert_eq!(app.selected_worktree().map(|w| w.id.as_str()), Some("w2"));

        // Now "demo"'s session speaks: it overtakes, and "two" slides down
        // under the cursor.
        hse(
            &mut app,
            ServerEvent::StatusChanged {
                agent: AgentId("a1".into()),
                status: AgentStatus::Finished,
                changed_at: now,
                unseen: false,
            },
        );
        assert_eq!(names(&app), ["demo", "two"]);
        assert_eq!(app.sel_project, 1, "still on \"two\"");
    }

    /// Worktrees sort most-recently-interacted first below the root
    /// checkout, which stays the first row no matter what; the cursor
    /// follows its row across the re-sort.
    #[test]
    fn worktrees_sort_by_last_interaction() {
        use nebula_core::AgentStatus;
        let mut app = App::new();
        seed_tree(&mut app); // w1 "main" (root) / a1 never run
        let now = crate::app::now_ms();
        let mins = |n: i64| now - n * 60_000;
        hse(
            &mut app,
            ServerEvent::EntityUpserted {
                entity: wt_entity("w2", "p1", "feat", false),
            },
        );
        hse(
            &mut app,
            ServerEvent::EntityUpserted {
                entity: wt_entity("w3", "p1", "older", false),
            },
        );
        hse(
            &mut app,
            ServerEvent::EntityUpserted {
                entity: agent_stamped("a2", "w2", mins(5)),
            },
        );
        hse(
            &mut app,
            ServerEvent::EntityUpserted {
                entity: agent_stamped("a3", "w3", mins(50)),
            },
        );
        let branches = |app: &App| -> Vec<String> {
            app.visible_worktrees()
                .iter()
                .map(|w| w.branch.clone())
                .collect()
        };
        assert_eq!(
            branches(&app),
            ["main", "feat", "older"],
            "the never-run root stays on top; the rest sit newest first"
        );

        // Rest on "older", then its session finishes: it overtakes "feat"
        // but never the root, and the cursor follows it.
        app.focus = Focus::Worktrees;
        app.sel_worktree = 2;
        hse(
            &mut app,
            ServerEvent::StatusChanged {
                agent: AgentId("a3".into()),
                status: AgentStatus::Finished,
                changed_at: now,
                unseen: false,
            },
        );
        assert_eq!(branches(&app), ["main", "older", "feat"]);
        assert_eq!(app.sel_worktree, 1, "selection follows the \"older\" row");
    }

    /// Project and worktree rows carry the same "23m ago" label the session
    /// rows do — the newest stamp of the sessions under them — and stay
    /// bare while nothing under them has run.
    #[test]
    fn project_and_worktree_rows_show_time_since_last_interaction() {
        let mut app = App::new();
        seed_tree(&mut app); // demo / main / agent-1: never run
        let render = |app: &mut App| -> String {
            let mut terminal = Terminal::new(TestBackend::new(120, 30)).unwrap();
            terminal.draw(|f| ui::draw(f, app)).unwrap();
            buffer_text(&terminal)
        };
        let row_with = |app: &mut App, needle: &str| -> String {
            render(app)
                .lines()
                .find(|l| l.contains(needle))
                .unwrap_or_default()
                .to_string()
        };
        assert!(
            !row_with(&mut app, "demo").contains("ago"),
            "never-run project stays bare"
        );
        assert!(
            !row_with(&mut app, "main").contains("ago"),
            "never-run worktree stays bare"
        );

        hse(
            &mut app,
            ServerEvent::EntityUpserted {
                entity: agent_stamped("alpha", "w1", crate::app::now_ms() - 23 * 60_000),
            },
        );
        let row = row_with(&mut app, "demo");
        let name = row.find("demo").expect("the project name");
        let ago = row
            .find("23m ago")
            .unwrap_or_else(|| panic!("project row has no ago label:\n{row}"));
        assert!(name < ago, "name, then how long ago:\n{row}");

        // At the default 22-column width the root badge keeps its glyph
        // and gives up the word: `main ⌂ 23m ago`.
        let row = row_with(&mut app, "main");
        let name = row.find("main").expect("the branch");
        let root = row.find("⌂").expect("the root glyph");
        let ago = row
            .find("23m ago")
            .unwrap_or_else(|| panic!("worktree row has no ago label:\n{row}"));
        assert!(
            name < root && root < ago,
            "branch, root badge, then how long ago:\n{row}"
        );
        assert!(!row.contains("⌂ root"), "no room for the word:\n{row}");

        // A wider column gets the whole badge back.
        app.panel_widths[1] = 34;
        let row = row_with(&mut app, "main");
        assert!(
            row.contains("main ⌂ root 23m ago"),
            "full badge and the label:\n{row}"
        );
        app.panel_widths[1] = crate::app::DEFAULT_PANEL_WIDTHS[1];

        // Squeeze the columns: the labels drop rather than eat the names.
        app.panel_widths[0] = 14;
        app.panel_widths[1] = 14;
        let text = render(&mut app);
        let projects_and_worktrees: Vec<&str> = text
            .lines()
            .filter(|l| l.contains("demo") || l.contains("main"))
            .collect();
        assert!(
            projects_and_worktrees.iter().all(|l| !l
                .chars()
                .take(40)
                .collect::<String>()
                .contains("ago")),
            "ago labels yield to the names in narrow columns:\n{text}"
        );
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

    /// A branch is often described as a sentence ("fix login redirect");
    /// git wants it hyphenated, so the prompt does that conversion rather
    /// than handing git a ref it refuses.
    #[test]
    fn typed_worktree_name_hyphenates_spaces() {
        let mut app = App::new();
        seed_tree(&mut app); // p1/w1(main) + agent-1
        let mut out = Vec::new();
        app.focus = Focus::Worktrees;

        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('n'), KeyModifiers::NONE),
            &mut out,
        );
        for c in "  fix login  redirect ".chars() {
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
        let Some(ClientRequest::CreateWorktree { branch, .. }) = out.last() else {
            panic!("prompt submit requests a worktree: {out:?}");
        };
        assert_eq!(branch, "fix-login-redirect");
    }

    /// Enter on an empty prompt takes the random name the prompt was
    /// offering — the same one the label showed, not a fresh roll.
    #[test]
    fn empty_worktree_prompt_uses_the_offered_random_name() {
        let mut app = App::new();
        seed_tree(&mut app); // p1/w1(main) + agent-1
        let mut out = Vec::new();
        app.focus = Focus::Worktrees;

        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('n'), KeyModifiers::NONE),
            &mut out,
        );
        let Some(Overlay::Prompt(prompt)) = &app.overlay else {
            panic!("n opens the new-worktree prompt");
        };
        let PromptKind::NewWorktree { suggestion, .. } = &prompt.kind else {
            panic!("wrong prompt: {:?}", prompt.kind);
        };
        let offered = suggestion.clone();
        assert!(
            prompt.label.contains(&offered),
            "the offered name is not in the label: {}",
            prompt.label
        );
        assert_eq!(offered.split('-').count(), 3, "not three words: {offered}");

        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
            &mut out,
        );
        let Some(ClientRequest::CreateWorktree { branch, .. }) = out.last() else {
            panic!("empty submit still requests a worktree: {out:?}");
        };
        assert_eq!(branch, &offered);
    }

    /// Typing only spaces is the same as typing nothing: no empty ref, no
    /// "cancelled" flash — the offered name stands in.
    #[test]
    fn whitespace_only_worktree_name_falls_back_to_the_random_one() {
        let mut app = App::new();
        seed_tree(&mut app); // p1/w1(main) + agent-1
        let mut out = Vec::new();
        app.focus = Focus::Worktrees;

        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('n'), KeyModifiers::NONE),
            &mut out,
        );
        for _ in 0..3 {
            handle_key(
                &mut app,
                KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE),
                &mut out,
            );
        }
        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
            &mut out,
        );
        let Some(ClientRequest::CreateWorktree { branch, .. }) = out.last() else {
            panic!("whitespace submit still requests a worktree: {out:?}");
        };
        assert_eq!(branch.split('-').count(), 3, "not a random name: {branch}");
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
        // The pane comes back at once; the Attach waits out the debounce, so
        // sweeping through worktrees doesn't cold-boot each one in passing.
        fire_pending_attach(&mut app, &mut out);
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
                entity: project("p2", "two", 1),
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

    /// The selection rail on a worktree/session pill runs the pill's full
    /// visual height — the pad's own half-block on the pad rows, a solid
    /// block on the text row — and sessions share the worktrees' 2-row
    /// pill stride so the two lists read uniformly.
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
                    unseen: false,
                    kind: nebula_core::AgentKind::Claude,
                    model: None,
                    effort: None,
                    session_id: None,
                    cloud_session_id: None,
                    sort_order: 1,
                    status_changed_at: 0,
                    alive: true,
                    cloud_mirroring: false,
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
        // rail col █, then dot + name: the rail sits one cell left of the
        // dot, and on the pads it wears the same half-block as the fill so
        // no bare-background quarter is left beside it.
        let rail_check = |name: &str, text: &str, lines: &Vec<&str>| {
            let dot = format!("● {name}");
            let row = lines
                .iter()
                .position(|l| l.contains(&dot))
                .unwrap_or_else(|| panic!("row {name:?} not on screen:\n{text}"));
            let col = char_col(lines[row], &dot).unwrap() - 1;
            assert_eq!(
                at(row, col),
                Some('█'),
                "rail on {name}'s text row:\n{text}"
            );
            assert_eq!(
                at(row - 1, col),
                Some('▄'),
                "rail cap on {name}'s top pad:\n{text}"
            );
            assert_eq!(
                at(row + 1, col),
                Some('▀'),
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

    /// `G` turns the checkout's remote into a page and hands it to the
    /// browser (`open_url` is a no-op under test, so the flash is the
    /// observable half).
    #[test]
    fn shift_g_opens_the_repos_remote_in_the_browser() {
        let dir = tempfile::tempdir().unwrap();
        let repo = test_repo(&dir);
        run_git(
            &repo,
            &["remote", "add", "origin", "git@github.com:o/r.git"],
        );
        let mut app = App::new();
        seed_repo_tree(&mut app, &repo);
        let mut out = Vec::new();
        press(&mut app, KeyCode::Char('G'), KeyModifiers::SHIFT, &mut out);
        assert_eq!(app.flash.as_deref(), Some("opened github.com/o/r"));
        assert!(app.overlay.is_none(), "the browser is the whole feature");
        assert!(out.is_empty(), "nothing to tell the daemon about");
    }

    #[test]
    fn shift_g_without_a_remote_says_so() {
        let dir = tempfile::tempdir().unwrap();
        let repo = test_repo(&dir);
        let mut app = App::new();
        seed_repo_tree(&mut app, &repo);
        let mut out = Vec::new();
        press(&mut app, KeyCode::Char('G'), KeyModifiers::SHIFT, &mut out);
        assert_eq!(app.flash.as_deref(), Some("no git remote on this repo"));
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
                    archived_at: 0,
                    unseen: false,
                    kind: nebula_core::AgentKind::Codex,
                    model: None,
                    effort: None,
                    session_id: None,
                    cloud_session_id: None,
                    sort_order: 0,
                    status_changed_at: 0,
                    alive: true,
                    cloud_mirroring: false,
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
                    unseen: false,
                    kind: nebula_core::AgentKind::Claude,
                    model: None,
                    effort: None,
                    session_id: None,
                    cloud_session_id: None,
                    sort_order: 1,
                    status_changed_at: 0,
                    alive: false,
                    cloud_mirroring: false,
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
            // Same boundary match; the attention order breaks the tie, and
            // for two never-run rows that is build order — the worktree
            // before its session.
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
    fn palette_enter_on_project_lands_in_the_next_visible_panel() {
        let mut app = App::new();
        seed_tree(&mut app);
        seed_second_project(&mut app);
        app.focus = Focus::Projects;
        let mut out = Vec::new();
        press(&mut app, KeyCode::Char('/'), KeyModifiers::NONE, &mut out);
        for c in "nebula".chars() {
            press(&mut app, KeyCode::Char(c), KeyModifiers::NONE, &mut out);
        }
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE, &mut out);

        assert!(app.overlay.is_none());
        assert_eq!(app.selected_project().unwrap().name, "nebula");
        assert_eq!(
            app.focus,
            Focus::Worktrees,
            "a project pick lands in its Worktrees panel, not the Projects column"
        );

        app.hide_projects = true;
        app.hide_worktrees = true;
        app.focus = Focus::Sessions;
        press(&mut app, KeyCode::Char('/'), KeyModifiers::NONE, &mut out);
        for c in "nebula".chars() {
            press(&mut app, KeyCode::Char(c), KeyModifiers::NONE, &mut out);
        }
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE, &mut out);
        assert!(app.overlay.is_none());
        assert_eq!(app.selected_project().unwrap().name, "nebula");
        assert_eq!(app.focus, Focus::Sessions, "hidden panels stay hidden");
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
                    unseen: false,
                    kind: nebula_core::AgentKind::Claude,
                    model: None,
                    effort: None,
                    session_id: None,
                    cloud_session_id: None,
                    sort_order: 0,
                    status_changed_at: 0,
                    alive: true,
                    cloud_mirroring: false,
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
        open_settings_on(&mut app, 0, &mut out);
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
        open_settings_on(&mut app, 0, &mut out);
        press(&mut app, KeyCode::Char('j'), KeyModifiers::NONE, &mut out);
        press(&mut app, KeyCode::Char('j'), KeyModifiers::NONE, &mut out);
        press(&mut app, KeyCode::Esc, KeyModifiers::NONE, &mut out);
        press(&mut app, KeyCode::Char('s'), KeyModifiers::NONE, &mut out);
        let Some(Overlay::Settings(view)) = &app.overlay else {
            panic!("settings closed");
        };
        assert_eq!(view.selected, 2, "reopen lands on the last focused row");
        assert!(!view.on_tabs, "…in the list, where we left the cursor");
    }

    /// Nothing visited yet: the strip has the cursor, so ←/→ mean "walk the
    /// tabs" the moment the overlay is up. Once the cursor has been parked
    /// somewhere, a reopen restores that tab/row/focus instead.
    #[test]
    fn settings_first_open_lands_on_the_tab_strip() {
        let mut app = App::new();
        let mut out = Vec::new();
        press(&mut app, KeyCode::Char('s'), KeyModifiers::NONE, &mut out);
        assert!(settings_view(&app).on_tabs, "fresh open parks on the strip");
        // ←/→ steer the strip straight away, no ↑ needed first.
        press(&mut app, KeyCode::Right, KeyModifiers::NONE, &mut out);
        assert_eq!(settings_view(&app).tab, 1);
        press(&mut app, KeyCode::Esc, KeyModifiers::NONE, &mut out);
        press(&mut app, KeyCode::Char('s'), KeyModifiers::NONE, &mut out);
        assert_eq!(settings_view(&app).tab, 1, "reopen keeps the tab");
        assert!(settings_view(&app).on_tabs, "…and the strip focus");
        // Drop into the list, leave, come back: the list has it now.
        press(&mut app, KeyCode::Down, KeyModifiers::NONE, &mut out);
        press(&mut app, KeyCode::Esc, KeyModifiers::NONE, &mut out);
        press(&mut app, KeyCode::Char('s'), KeyModifiers::NONE, &mut out);
        assert!(!settings_view(&app).on_tabs, "reopen restores list focus");
    }

    /// The remembered position has a shelf life. Closed and reopened within
    /// `SETTINGS_MEMORY_TTL` it's restored as usual; reopened later than
    /// that it's forgotten and the overlay looks exactly like a first open.
    #[test]
    fn settings_memory_expires_a_minute_after_closing() {
        use crate::app::SETTINGS_MEMORY_TTL;
        let mut app = App::new();
        let mut out = Vec::new();
        assert!(
            app.settings_closed_at.is_none() && !app.settings_memory_expired(),
            "nothing to forget before the first close"
        );
        // Park the cursor somewhere distinctive: tab 1, second row, in the list.
        open_settings_on(&mut app, 1, &mut out);
        press(&mut app, KeyCode::Down, KeyModifiers::NONE, &mut out);
        let (tab, row) = (settings_view(&app).tab, settings_view(&app).selected);
        assert_eq!((tab, row), (1, 1));
        press(&mut app, KeyCode::Esc, KeyModifiers::NONE, &mut out);
        assert!(app.settings_closed_at.is_some(), "Esc stamps the close");

        // Straight back in: everything restored.
        press(&mut app, KeyCode::Char('s'), KeyModifiers::NONE, &mut out);
        assert_eq!(settings_view(&app).tab, 1, "within the minute, the tab");
        assert_eq!(settings_view(&app).selected, 1, "…the row");
        assert!(!settings_view(&app).on_tabs, "…and the list focus survive");
        press(&mut app, KeyCode::Esc, KeyModifiers::NONE, &mut out);

        // Pretend the close was a minute ago.
        app.settings_closed_at = std::time::Instant::now().checked_sub(SETTINGS_MEMORY_TTL);
        assert!(app.settings_memory_expired());
        press(&mut app, KeyCode::Char('s'), KeyModifiers::NONE, &mut out);
        assert_eq!(settings_view(&app).tab, 0, "stale memory: first tab");
        assert_eq!(settings_view(&app).selected, 0, "…top row");
        assert!(settings_view(&app).on_tabs, "…cursor on the strip");
        assert_eq!(
            app.settings_row(1),
            0,
            "the other tabs' rows are forgotten too"
        );
        assert!(
            app.settings_closed_at.is_none(),
            "the stale stamp is cleared, so the visit is a fresh one"
        );
    }

    /// The reset confirmation swaps the overlay out and back mid-visit;
    /// that round trip must not consult the clock, or a stale stamp from an
    /// earlier close would reset the cursor under the user's hands.
    #[test]
    fn settings_reset_round_trip_ignores_the_memory_clock() {
        use crate::app::SETTINGS_MEMORY_TTL;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        crate::config::with_config_path(path, || {
            let mut app = App::new();
            let mut out = Vec::new();
            open_settings_on(&mut app, 1, &mut out);
            // A stamp that would expire the memory if it were consulted.
            app.settings_closed_at = std::time::Instant::now().checked_sub(SETTINGS_MEMORY_TTL);
            press(&mut app, KeyCode::Char('R'), KeyModifiers::SHIFT, &mut out);
            assert!(matches!(app.overlay, Some(Overlay::Confirm(_))));
            press(&mut app, KeyCode::Esc, KeyModifiers::NONE, &mut out);
            assert_eq!(settings_view(&app).tab, 1, "back on the same tab");
            assert!(!settings_view(&app).on_tabs, "…still in the list");
        });
    }

    /// A click outside the modal is the other way out, and it starts the
    /// same clock as Esc does.
    #[test]
    fn clicking_outside_settings_stamps_the_close() {
        let mut app = App::new();
        let mut out = Vec::new();
        press(&mut app, KeyCode::Char('s'), KeyModifiers::NONE, &mut out);
        if let Some(Overlay::Settings(view)) = &mut app.overlay {
            view.area = ratatui::layout::Rect::new(10, 5, 40, 20);
        }
        handle_mouse(
            &mut app,
            MouseEvent {
                kind: MouseEventKind::Down(MouseButton::Left),
                column: 0,
                row: 0,
                modifiers: KeyModifiers::NONE,
            },
            &mut out,
        );
        assert!(app.overlay.is_none(), "click outside closes");
        assert!(app.settings_closed_at.is_some(), "…and stamps the close");
    }

    #[test]
    fn settings_enter_persists_toggle_to_config_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        crate::config::with_config_path(path.clone(), || {
            let mut app = App::new();
            let mut out = Vec::new();
            open_settings_on(&mut app, 0, &mut out);
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
    fn settings_hl_cycles_session_idle_timeout_and_persists() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        crate::config::with_config_path(path, || {
            let mut app = App::new();
            let mut out = Vec::new();
            let (tab, row) =
                crate::config::locate(crate::config::SettingKind::SessionIdleTimeout).unwrap();
            open_settings_on(&mut app, tab, &mut out);
            for _ in 0..row {
                press(&mut app, KeyCode::Char('j'), KeyModifiers::NONE, &mut out);
            }
            press(&mut app, KeyCode::Char('l'), KeyModifiers::NONE, &mut out);
            let cfg = crate::config::Config::load();
            assert_eq!(cfg.session_idle_timeout, "15m");
            press(&mut app, KeyCode::Char('h'), KeyModifiers::NONE, &mut out);
            let cfg = crate::config::Config::load();
            assert_eq!(cfg.session_idle_timeout, "5m");
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
        // Settings live on their own tab now, so a row from another tab
        // is only on screen once you switch to it.
        assert!(
            !text.contains("Idle session timeout"),
            "another tab's rows stay off screen:\n{text}"
        );
        press(&mut app, KeyCode::Tab, KeyModifiers::NONE, &mut out);
        terminal.draw(|f| ui::draw(f, &mut app)).unwrap();
        let sessions_text = buffer_text(&terminal);
        assert!(
            sessions_text.contains("Idle session timeout"),
            "Tab reaches the Sessions tab:\n{sessions_text}"
        );
        for tab in crate::config::SETTINGS_TABS {
            assert!(text.contains(tab.title), "tab strip rendered:\n{text}");
        }
        assert!(
            text.contains("Enter in / search opens the session"),
            "selected setting's hint shown in the footer:\n{text}"
        );
        let Some(Overlay::Settings(view)) = &app.overlay else {
            panic!("settings closed");
        };
        assert!(view.area.width > 0, "draw writes hit-test area");
        assert_eq!(
            view.tab_hits.len(),
            crate::config::tab_count(),
            "draw records a click target per tab"
        );
    }

    // ---- settings tabs & hotkeys ----

    /// Open the settings overlay parked on `tab`, cursor down in the list.
    /// A fresh overlay opens on the tab strip, so the ↓ is what these tests
    /// mean by "parked on the tab".
    fn open_settings_on(app: &mut App, tab: usize, out: &mut Vec<ClientRequest>) {
        press(app, KeyCode::Char('s'), KeyModifiers::NONE, out);
        for _ in 0..tab {
            press(app, KeyCode::Tab, KeyModifiers::NONE, out);
        }
        press(app, KeyCode::Down, KeyModifiers::NONE, out);
    }

    fn settings_view(app: &App) -> &crate::app::SettingsView {
        match &app.overlay {
            Some(Overlay::Settings(view)) => view,
            _ => panic!("settings closed"),
        }
    }

    #[test]
    fn tab_and_backtab_walk_the_strip_and_wrap() {
        let mut app = App::new();
        let mut out = Vec::new();
        let tabs = crate::config::tab_count();
        press(&mut app, KeyCode::Char('s'), KeyModifiers::NONE, &mut out);
        assert_eq!(settings_view(&app).tab, 0);
        for i in 1..tabs {
            press(&mut app, KeyCode::Tab, KeyModifiers::NONE, &mut out);
            assert_eq!(settings_view(&app).tab, i);
        }
        press(&mut app, KeyCode::Tab, KeyModifiers::NONE, &mut out);
        assert_eq!(settings_view(&app).tab, 0, "Tab wraps round the strip");
        press(&mut app, KeyCode::BackTab, KeyModifiers::SHIFT, &mut out);
        assert_eq!(settings_view(&app).tab, tabs - 1, "⇧Tab wraps back");
    }

    #[test]
    fn digits_jump_straight_to_a_tab() {
        let mut app = App::new();
        let mut out = Vec::new();
        press(&mut app, KeyCode::Char('s'), KeyModifiers::NONE, &mut out);
        press(&mut app, KeyCode::Char('3'), KeyModifiers::NONE, &mut out);
        assert_eq!(settings_view(&app).tab, 2);
        // A digit past the last tab is ignored rather than clamped.
        press(&mut app, KeyCode::Char('9'), KeyModifiers::NONE, &mut out);
        assert_eq!(settings_view(&app).tab, 2);
    }

    /// The arrows do double duty: cycling a value inside the list, walking
    /// the tabs once the cursor has stepped up onto the strip.
    #[test]
    fn up_from_the_top_row_parks_on_the_strip_where_arrows_move_tabs() {
        let dir = tempfile::tempdir().unwrap();
        crate::config::with_config_path(dir.path().join("config.json"), || {
            let mut app = App::new();
            let mut out = Vec::new();
            open_settings_on(&mut app, 0, &mut out);
            assert!(!settings_view(&app).on_tabs);
            // In the list, → cycles the selected setting's value.
            press(&mut app, KeyCode::Char('j'), KeyModifiers::NONE, &mut out);
            press(&mut app, KeyCode::Char('j'), KeyModifiers::NONE, &mut out);
            press(&mut app, KeyCode::Right, KeyModifiers::NONE, &mut out);
            assert_eq!(crate::config::Config::load().editor, "nvim");
            assert_eq!(settings_view(&app).tab, 0, "→ did not move the tab");

            // ↑ off the top row steps onto the strip; now → is the tab.
            press(&mut app, KeyCode::Up, KeyModifiers::NONE, &mut out);
            press(&mut app, KeyCode::Up, KeyModifiers::NONE, &mut out);
            press(&mut app, KeyCode::Up, KeyModifiers::NONE, &mut out);
            assert!(settings_view(&app).on_tabs, "↑ off the top row parks here");
            press(&mut app, KeyCode::Right, KeyModifiers::NONE, &mut out);
            assert_eq!(settings_view(&app).tab, 1);
            assert_eq!(
                crate::config::Config::load().editor,
                "nvim",
                "no value was cycled while the strip had focus"
            );
            // ↓ drops back into the list.
            press(&mut app, KeyCode::Down, KeyModifiers::NONE, &mut out);
            assert!(!settings_view(&app).on_tabs);
        });
    }

    #[test]
    fn each_tab_remembers_its_own_cursor_row() {
        let mut app = App::new();
        let mut out = Vec::new();
        open_settings_on(&mut app, 0, &mut out);
        press(&mut app, KeyCode::Char('j'), KeyModifiers::NONE, &mut out);
        press(&mut app, KeyCode::Char('j'), KeyModifiers::NONE, &mut out);
        assert_eq!(settings_view(&app).selected, 2);
        press(&mut app, KeyCode::Tab, KeyModifiers::NONE, &mut out);
        assert_eq!(settings_view(&app).selected, 0, "a fresh tab starts at 0");
        press(&mut app, KeyCode::Char('j'), KeyModifiers::NONE, &mut out);
        press(&mut app, KeyCode::BackTab, KeyModifiers::SHIFT, &mut out);
        assert_eq!(settings_view(&app).selected, 2, "back where we left it");
        press(&mut app, KeyCode::Esc, KeyModifiers::NONE, &mut out);
        press(&mut app, KeyCode::Char('s'), KeyModifiers::NONE, &mut out);
        assert_eq!(settings_view(&app).selected, 2, "and across a reopen");
    }

    #[test]
    fn hotkeys_tab_lists_every_action_with_its_chords() {
        let mut app = App::new();
        let mut out = Vec::new();
        open_settings_on(&mut app, crate::config::hotkeys_tab(), &mut out);
        let mut terminal = Terminal::new(TestBackend::new(100, 40)).unwrap();
        terminal.draw(|f| ui::draw(f, &mut app)).unwrap();
        let text = buffer_text(&terminal);
        assert!(text.contains("Hotkeys"), "tab strip:\n{text}");
        assert!(text.contains("NAVIGATE"), "group header:\n{text}");
        assert!(text.contains("Next panel"), "an action label:\n{text}");
        assert!(
            text.contains("Next panel                  Tab"),
            "its chord, in the value column:\n{text}"
        );
    }

    /// The headline of the whole tab: press Enter, press a key, and that
    /// key now drives the action — through the config file, not just in
    /// memory.
    #[test]
    fn rebinding_an_action_takes_effect_and_persists() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        crate::config::with_config_path(path.clone(), || {
            let mut app = App::new();
            let mut out = Vec::new();
            open_settings_on(&mut app, crate::config::hotkeys_tab(), &mut out);
            let row = crate::keymap::index_of(crate::keymap::Action::Help).unwrap();
            for _ in 0..row {
                press(&mut app, KeyCode::Char('j'), KeyModifiers::NONE, &mut out);
            }
            press(&mut app, KeyCode::Enter, KeyModifiers::NONE, &mut out);
            assert!(settings_view(&app).capturing(), "waiting for a key");
            press(&mut app, KeyCode::F(6), KeyModifiers::NONE, &mut out);
            assert!(
                !settings_view(&app).capturing(),
                "the press was the binding"
            );
            assert_eq!(app.keymap.label(crate::keymap::Action::Help), "F6");

            let saved: serde_json::Value =
                serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
            assert_eq!(saved["keybindings"]["help"], "f6");

            // And the new key actually opens help from the panels.
            press(&mut app, KeyCode::Esc, KeyModifiers::NONE, &mut out);
            assert!(app.overlay.is_none());
            press(&mut app, KeyCode::F(6), KeyModifiers::NONE, &mut out);
            assert!(matches!(app.overlay, Some(Overlay::Help(_))));
            // …and the old one no longer does.
            let mut fresh = App::new();
            fresh.keymap = crate::config::Config::load().keymap();
            press(&mut fresh, KeyCode::Char('?'), KeyModifiers::NONE, &mut out);
            assert!(fresh.overlay.is_none(), "? is unbound now");
        });
    }

    #[test]
    fn a_duplicate_chord_warns_before_it_is_taken() {
        let dir = tempfile::tempdir().unwrap();
        crate::config::with_config_path(dir.path().join("config.json"), || {
            let mut app = App::new();
            let mut out = Vec::new();
            open_settings_on(&mut app, crate::config::hotkeys_tab(), &mut out);
            let row = crate::keymap::index_of(crate::keymap::Action::Help).unwrap();
            for _ in 0..row {
                press(&mut app, KeyCode::Char('j'), KeyModifiers::NONE, &mut out);
            }
            press(&mut app, KeyCode::Enter, KeyModifiers::NONE, &mut out);
            // `g` is Git diff's — capturing it must not silently steal it.
            press(&mut app, KeyCode::Char('g'), KeyModifiers::NONE, &mut out);
            let view = settings_view(&app);
            let (text, level) = view.notice.clone().expect("a warning");
            assert_eq!(level, crate::app::NoticeLevel::Warn);
            assert!(text.contains("already"), "{text}");
            assert!(text.contains("Git diff"), "names the current owner: {text}");
            assert!(!view.capturing(), "the capture is paused on the warning");
            assert_eq!(
                app.keymap.label(crate::keymap::Action::Help),
                "?",
                "nothing changed yet"
            );

            // Esc leaves it where it was.
            press(&mut app, KeyCode::Esc, KeyModifiers::NONE, &mut out);
            assert_eq!(app.keymap.label(crate::keymap::Action::GitDiff), "g");
            assert_eq!(app.keymap.label(crate::keymap::Action::Help), "?");
        });
    }

    #[test]
    fn confirming_a_duplicate_moves_the_chord_off_its_old_action() {
        let dir = tempfile::tempdir().unwrap();
        crate::config::with_config_path(dir.path().join("config.json"), || {
            let mut app = App::new();
            let mut out = Vec::new();
            open_settings_on(&mut app, crate::config::hotkeys_tab(), &mut out);
            let row = crate::keymap::index_of(crate::keymap::Action::Help).unwrap();
            for _ in 0..row {
                press(&mut app, KeyCode::Char('j'), KeyModifiers::NONE, &mut out);
            }
            press(&mut app, KeyCode::Enter, KeyModifiers::NONE, &mut out);
            press(&mut app, KeyCode::Char('g'), KeyModifiers::NONE, &mut out);
            press(&mut app, KeyCode::Enter, KeyModifiers::NONE, &mut out);
            assert_eq!(app.keymap.label(crate::keymap::Action::Help), "g");
            assert_eq!(
                app.keymap.label(crate::keymap::Action::GitDiff),
                "—",
                "one keystroke can only mean one thing"
            );
            // The panels agree with the map.
            press(&mut app, KeyCode::Esc, KeyModifiers::NONE, &mut out);
            seed_tree(&mut app);
            app.focus = Focus::Worktrees;
            press(&mut app, KeyCode::Char('g'), KeyModifiers::NONE, &mut out);
            assert!(matches!(app.overlay, Some(Overlay::Help(_))));
        });
    }

    /// nebula is a guest inside Terminal.app / Ghostty, which take some
    /// chords before it ever sees them. Binding one is allowed — the user
    /// may be on a terminal that delivers it — but never silently.
    #[test]
    fn binding_a_chord_the_host_terminal_eats_says_so() {
        let dir = tempfile::tempdir().unwrap();
        crate::config::with_config_path(dir.path().join("config.json"), || {
            let mut app = App::new();
            let mut out = Vec::new();
            open_settings_on(&mut app, crate::config::hotkeys_tab(), &mut out);
            press(&mut app, KeyCode::Enter, KeyModifiers::NONE, &mut out);
            press(&mut app, KeyCode::Char(']'), KeyModifiers::SUPER, &mut out);
            let (text, level) = settings_view(&app).notice.clone().expect("a warning");
            assert_eq!(level, crate::app::NoticeLevel::Warn);
            assert!(text.contains('⌘'), "{text}");
            assert_eq!(
                app.keymap.label(crate::keymap::Action::FocusNext),
                "⌘]",
                "warned, not refused"
            );
        });
    }

    #[test]
    fn a_hotkey_row_resets_to_its_default_and_can_be_unbound() {
        let dir = tempfile::tempdir().unwrap();
        crate::config::with_config_path(dir.path().join("config.json"), || {
            let mut app = App::new();
            let mut out = Vec::new();
            open_settings_on(&mut app, crate::config::hotkeys_tab(), &mut out);
            // Row 0 is Next panel (Tab).
            press(&mut app, KeyCode::Enter, KeyModifiers::NONE, &mut out);
            press(&mut app, KeyCode::F(8), KeyModifiers::NONE, &mut out);
            assert_eq!(app.keymap.label(crate::keymap::Action::FocusNext), "F8");
            press(&mut app, KeyCode::Char('x'), KeyModifiers::NONE, &mut out);
            assert_eq!(app.keymap.label(crate::keymap::Action::FocusNext), "—");
            press(&mut app, KeyCode::Backspace, KeyModifiers::NONE, &mut out);
            assert_eq!(
                app.keymap.label(crate::keymap::Action::FocusNext),
                "Tab ^⇧L"
            );
            assert!(
                crate::config::Config::load().keybindings.is_empty(),
                "back to the default = nothing left to write down"
            );
        });
    }

    #[test]
    fn adding_an_alternate_keeps_the_original() {
        let dir = tempfile::tempdir().unwrap();
        crate::config::with_config_path(dir.path().join("config.json"), || {
            let mut app = App::new();
            let mut out = Vec::new();
            open_settings_on(&mut app, crate::config::hotkeys_tab(), &mut out);
            press(&mut app, KeyCode::Char('a'), KeyModifiers::NONE, &mut out);
            press(&mut app, KeyCode::F(7), KeyModifiers::NONE, &mut out);
            assert_eq!(
                app.keymap.label(crate::keymap::Action::FocusNext),
                "Tab ^⇧L F7"
            );
        });
    }

    #[test]
    fn esc_backs_out_of_a_capture_without_binding_it() {
        let mut app = App::new();
        let mut out = Vec::new();
        open_settings_on(&mut app, crate::config::hotkeys_tab(), &mut out);
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE, &mut out);
        press(&mut app, KeyCode::Esc, KeyModifiers::NONE, &mut out);
        assert!(!settings_view(&app).capturing());
        assert!(
            matches!(app.overlay, Some(Overlay::Settings(_))),
            "Esc left the capture, not the overlay"
        );
        assert_eq!(
            app.keymap.label(crate::keymap::Action::FocusNext),
            "Tab ^⇧L"
        );
    }

    /// A capture swallows the overlay's own keys — otherwise half the
    /// keyboard would be unbindable.
    #[test]
    fn a_capture_takes_keys_the_overlay_would_normally_use() {
        let dir = tempfile::tempdir().unwrap();
        crate::config::with_config_path(dir.path().join("config.json"), || {
            let mut app = App::new();
            let mut out = Vec::new();
            open_settings_on(&mut app, crate::config::hotkeys_tab(), &mut out);
            let row = crate::keymap::index_of(crate::keymap::Action::Splash).unwrap();
            for _ in 0..row {
                press(&mut app, KeyCode::Char('j'), KeyModifiers::NONE, &mut out);
            }
            press(&mut app, KeyCode::Enter, KeyModifiers::NONE, &mut out);
            // 'q' would close the overlay; here it is just a key.
            press(&mut app, KeyCode::Char('q'), KeyModifiers::NONE, &mut out);
            assert!(
                matches!(app.overlay, Some(Overlay::Settings(_))),
                "the overlay stayed open"
            );
            // It belongs to Quit, so this is the duplicate warning path.
            let (text, _) = settings_view(&app).notice.clone().expect("a warning");
            assert!(text.contains("Quit"), "{text}");
        });
    }

    #[test]
    fn ctrl_q_still_unlocks_a_terminal_after_the_hatch_is_rebound() {
        let dir = tempfile::tempdir().unwrap();
        crate::config::with_config_path(dir.path().join("config.json"), || {
            let mut app = App::new();
            seed_tree(&mut app);
            let mut out = Vec::new();
            // Rebind the unlock action to something else entirely.
            let mut keymap = app.keymap.clone();
            let idx = crate::keymap::index_of(crate::keymap::Action::UnlockTerminal).unwrap();
            keymap.bind(idx, crate::keymap::KeyChord::parse("f4").unwrap(), false);
            app.keymap = keymap;

            app.focus = Focus::Sessions;
            attach_selected(&mut app, &mut out);
            app.term_locked = true;
            assert!(app.term.is_some(), "a live pane to be locked into");
            press(
                &mut app,
                KeyCode::Char('q'),
                KeyModifiers::CONTROL,
                &mut out,
            );
            assert!(!app.term_locked, "^q is wired in, not merely bound");
            assert_eq!(app.focus, Focus::Sessions);

            // And the rebound key works too.
            app.term_locked = true;
            app.focus = Focus::Terminal;
            press(&mut app, KeyCode::F(4), KeyModifiers::NONE, &mut out);
            assert!(!app.term_locked);
        });
    }

    // ---- settings reset ----

    #[test]
    fn shift_r_in_settings_asks_first_and_n_goes_back_to_the_overlay() {
        with_default_config(|| {
            let mut app = App::new();
            let mut out = Vec::new();
            press(&mut app, KeyCode::Char('s'), KeyModifiers::NONE, &mut out);
            press(&mut app, KeyCode::Tab, KeyModifiers::NONE, &mut out);
            press(&mut app, KeyCode::Char('R'), KeyModifiers::SHIFT, &mut out);
            let Some(Overlay::Confirm(c)) = &app.overlay else {
                panic!("expected a confirmation, got {:?}", app.overlay);
            };
            assert_eq!(c.title, "Reset settings");
            assert!(matches!(c.action, PendingAction::ResetSettings));
            assert!(c.message.contains("hotkey"), "{}", c.message);

            press(&mut app, KeyCode::Char('n'), KeyModifiers::NONE, &mut out);
            assert!(
                matches!(app.overlay, Some(Overlay::Settings(_))),
                "n returns to the overlay, not the panels: {:?}",
                app.overlay
            );
            assert_eq!(settings_view(&app).tab, 1, "on the tab it was opened from");
            assert!(out.is_empty(), "nothing goes to the daemon");
        });
    }

    #[test]
    fn confirming_the_reset_rewrites_the_file_and_the_live_state() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        crate::config::with_config_path(path.clone(), || {
            // Dirty the file the way the overlay would, plus a key the
            // overlay doesn't own.
            let mut cfg = crate::config::Config {
                animations: false,
                show_workspaces: false,
                ..Default::default()
            };
            let mut keymap = cfg.keymap();
            let splash = crate::keymap::index_of(crate::keymap::Action::Splash).unwrap();
            keymap.bind(splash, crate::keymap::KeyChord::parse("f9").unwrap(), false);
            cfg.keybindings = keymap.overrides();
            cfg.save().unwrap();
            let mut root: serde_json::Value =
                serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
            root["prewarm_agents"] = serde_json::json!(false);
            std::fs::write(&path, serde_json::to_vec_pretty(&root).unwrap()).unwrap();

            let mut app = App::new();
            app.animations = false;
            app.show_workspaces = false;
            app.focus = Focus::Workspaces;
            app.keymap = keymap;
            let mut out = Vec::new();
            press(&mut app, KeyCode::Char('s'), KeyModifiers::NONE, &mut out);
            press(&mut app, KeyCode::Char('R'), KeyModifiers::SHIFT, &mut out);
            press(&mut app, KeyCode::Char('y'), KeyModifiers::NONE, &mut out);

            // Back in the overlay, saying what happened.
            let (text, level) = settings_view(&app).notice.clone().expect("a notice");
            assert!(text.contains("default"), "{text}");
            assert_eq!(level, crate::app::NoticeLevel::Info);
            assert!(app.flash.is_none(), "{:?}", app.flash);

            // Live state adopted the defaults…
            assert!(app.animations);
            assert!(app.show_workspaces);
            assert!(app.keymap.is_default(splash), "hotkeys reset too");

            // …and so did the file, foreign keys included.
            let saved = crate::config::Config::load();
            assert!(saved.animations);
            assert!(saved.show_workspaces);
            assert!(saved.keybindings.is_empty());
            let raw = std::fs::read_to_string(&path).unwrap();
            assert!(!raw.contains("prewarm_agents"), "{raw}");
        });
    }

    #[test]
    fn a_rebound_key_shows_up_in_help_and_the_footer() {
        let dir = tempfile::tempdir().unwrap();
        crate::config::with_config_path(dir.path().join("config.json"), || {
            let mut app = App::new();
            let mut keymap = app.keymap.clone();
            let idx = crate::keymap::index_of(crate::keymap::Action::Workspaces).unwrap();
            keymap.bind(idx, crate::keymap::KeyChord::parse("f9").unwrap(), false);
            app.keymap = keymap;

            let mut out = Vec::new();
            press(&mut app, KeyCode::Char('?'), KeyModifiers::NONE, &mut out);
            let mut terminal = Terminal::new(TestBackend::new(120, 40)).unwrap();
            terminal.draw(|f| ui::draw(f, &mut app)).unwrap();
            let text = buffer_text(&terminal);
            assert!(text.contains("F9"), "help follows the keymap:\n{text}");
            assert!(
                !text.contains("w             workspaces"),
                "and drops the old key:\n{text}"
            );

            // The first-run footer names the same keys; it follows too.
            press(&mut app, KeyCode::Esc, KeyModifiers::NONE, &mut out);
            terminal.draw(|f| ui::draw(f, &mut app)).unwrap();
            let footer = buffer_text(&terminal);
            assert!(
                footer.contains("F9: workspaces"),
                "footer follows too:\n{footer}"
            );
        });
    }

    /// The bind-time warning can't see a duplicate somebody typed into the
    /// config file by hand, so the row says it too.
    #[test]
    fn a_hand_edited_duplicate_is_called_out_on_the_row() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        std::fs::write(&path, r#"{"keybindings": {"help": "g"}}"#).unwrap();
        crate::config::with_config_path(path, || {
            let mut app = App::new();
            app.keymap = crate::config::Config::load().keymap();
            let mut out = Vec::new();
            open_settings_on(&mut app, crate::config::hotkeys_tab(), &mut out);
            let row = crate::keymap::index_of(crate::keymap::Action::GitDiff).unwrap();
            for _ in 0..row {
                press(&mut app, KeyCode::Char('j'), KeyModifiers::NONE, &mut out);
            }
            let mut terminal = Terminal::new(TestBackend::new(100, 40)).unwrap();
            terminal.draw(|f| ui::draw(f, &mut app)).unwrap();
            let text = buffer_text(&terminal);
            assert!(
                text.contains("also belongs to Help"),
                "the row names its rival:\n{text}"
            );
        });
    }

    #[test]
    fn clicking_a_tab_switches_to_it() {
        let mut app = App::new();
        let mut out = Vec::new();
        press(&mut app, KeyCode::Char('s'), KeyModifiers::NONE, &mut out);
        let mut terminal = Terminal::new(TestBackend::new(100, 40)).unwrap();
        terminal.draw(|f| ui::draw(f, &mut app)).unwrap();
        let (area, hits) = {
            let view = settings_view(&app);
            (view.area, view.tab_hits.clone())
        };
        let (x0, _) = hits[2];
        handle_mouse(
            &mut app,
            mev(MouseEventKind::Down(MouseButton::Left), x0, area.y + 1),
            &mut out,
        );
        assert_eq!(settings_view(&app).tab, 2, "clicked the third tab");
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
                        prewarm: None,
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

    /// Prewarm-pool spares have no agent row; without the home the daemon
    /// reports they'd render as "(unknown agent)". They group under one
    /// header as a small tree, named by kind/model and placed by worktree,
    /// with their own rollup line — and stay out of the live-agent counts.
    #[test]
    fn metrics_groups_prewarm_spares_under_their_own_header() {
        let mut app = App::new();
        seed_tree(&mut app);
        let mut out = Vec::new();
        press(&mut app, KeyCode::Char('M'), KeyModifiers::SHIFT, &mut out);
        let req_id = match out.last() {
            Some(ClientRequest::GetMetrics { req_id }) => *req_id,
            other => panic!("expected GetMetrics, got {other:?}"),
        };
        let spare =
            |id: &str, pid: u32, mb: u64, model: Option<&str>| nebula_core::SessionMetrics {
                session: SessionRef::Agent(AgentId(id.into())),
                pid,
                rss_bytes: mb * 1024 * 1024,
                procs: 3,
                prewarm: Some(nebula_core::PrewarmInfo {
                    worktree: nebula_core::WorktreeId("w1".into()),
                    kind: nebula_core::AgentKind::Claude,
                    model: model.map(str::to_string),
                }),
            };
        hse(
            &mut app,
            ServerEvent::Metrics {
                req_id,
                snapshot: nebula_core::MetricsSnapshot {
                    daemon_pid: 42,
                    daemon_rss_bytes: 40 * 1024 * 1024,
                    system_total_bytes: 0,
                    sessions: vec![
                        nebula_core::SessionMetrics {
                            session: SessionRef::Agent(AgentId("a1".into())),
                            pid: 4321,
                            rss_bytes: 500 * 1024 * 1024,
                            procs: 4,
                            prewarm: None,
                        },
                        spare("warm-1", 7001, 300, Some("opus")),
                        spare("warm-2", 7002, 250, None),
                    ],
                },
            },
        );
        let mut terminal = Terminal::new(TestBackend::new(100, 40)).unwrap();
        terminal.draw(|f| ui::draw(f, &mut app)).unwrap();
        let text = buffer_text(&terminal);
        assert!(
            !text.contains("(unknown agent)"),
            "spares are named, not unknown:\n{text}"
        );
        assert!(
            text.contains("1 session · 4 procs"),
            "claude rollup counts live sessions only:\n{text}"
        );
        assert!(
            text.contains("2 spares · 6 procs · pre-booted for new agents"),
            "spares get their own rollup line:\n{text}"
        );
        let pos = |s: &str| {
            text.find(s)
                .unwrap_or_else(|| panic!("{s} missing:\n{text}"))
        };
        assert!(
            pos("agent-1 (claude)") < pos("warm spares (2)")
                && pos("warm spares (2)") < pos("├ claude · opus")
                && pos("├ claude · opus") < pos("└ claude")
                && pos("└ claude") < pos("nebula daemon"),
            "live rows, then the spares tree, then nebula's own:\n{text}"
        );
        let leaf = text
            .lines()
            .find(|l| l.contains("├ claude · opus"))
            .expect("spare row");
        assert!(
            leaf.contains("demo/main") && leaf.contains("7001") && leaf.contains("300 MB"),
            "a spare is placed in its worktree with its own reading: {leaf}"
        );
        let header = text
            .lines()
            .find(|l| l.contains("warm spares (2)"))
            .expect("header row");
        assert!(
            header.contains("550 MB") && !header.contains("7001"),
            "the header sums its spares and points at no pid: {header}"
        );

        // Enter on a spare opens nothing: there's no agent row to attach
        // until a CreateAgent adopts it.
        let Some(Overlay::Metrics(view)) = &mut app.overlay else {
            panic!("metrics closed");
        };
        view.selected = 2;
        out.clear();
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE, &mut out);
        assert!(
            matches!(app.overlay, Some(Overlay::Metrics(_))) && out.is_empty(),
            "a spare row is inert: {out:?}"
        );
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
                prewarm: None,
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
                .and_then(|r| r.sref()),
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
    fn file_preview_gets_a_line_number_gutter_but_listings_dont() {
        let dir = tempfile::tempdir().unwrap();
        let repo = test_repo(&dir);
        std::fs::create_dir_all(repo.join("src")).unwrap();
        std::fs::write(repo.join("src/lib.rs"), "one\ntwo\nthree\n").unwrap();
        let mut app = App::new();
        seed_repo_tree(&mut app, &repo);
        let mut out = Vec::new();
        press(&mut app, KeyCode::Char('b'), KeyModifiers::NONE, &mut out);
        for c in ['l', 'i', 'b'] {
            press(&mut app, KeyCode::Char(c), KeyModifiers::NONE, &mut out);
        }
        assert_eq!(tree_rows(&app), vec!["src", "src/lib.rs"]);

        let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
        terminal.draw(|f| ui::draw(f, &mut app)).unwrap();
        let text = buffer_text(&terminal);
        for (n, line) in [(1, "one"), (2, "two"), (3, "three")] {
            assert!(
                text.contains(&format!(" {n} {line}")),
                "file preview numbers its lines:\n{text}"
            );
        }

        // A directory's child listing isn't file content — no gutter.
        press(&mut app, KeyCode::Up, KeyModifiers::NONE, &mut out);
        assert_eq!(tree_view(&app).selected_node().unwrap().path, "src");
        terminal.draw(|f| ui::draw(f, &mut app)).unwrap();
        let text = buffer_text(&terminal);
        assert!(text.contains("lib.rs"), "listing rendered:\n{text}");
        assert!(
            !text.contains(" 1 lib.rs"),
            "directory listings stay unnumbered:\n{text}"
        );
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
                        unseen: false,
                        kind: nebula_core::AgentKind::Claude,
                        model: None,
                        effort: None,
                        session_id: None,
                        cloud_session_id: None,
                        sort_order: 1,
                        status_changed_at: 0,
                        alive: true,
                        cloud_mirroring: false,
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
            unseen: false,
            kind: nebula_core::AgentKind::Claude,
            model: None,
            effort: None,
            session_id: None,
            cloud_session_id: None,
            sort_order: 1,
            status_changed_at: 0,
            alive: true,
            cloud_mirroring: false,
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
                entity: project("p2", "two", 1),
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
                    sort_order: 0,
                }),
            },
        );
    }

    /// Projects outside the open workspace get no panel row and don't count
    /// toward the header — but `/` reaches them anyway, pathed under their
    /// workspace, with the open workspace's rows still listed first.
    #[test]
    fn other_workspaces_are_off_the_panels_but_still_in_the_palette() {
        let mut app = App::new();
        seed_tree(&mut app);
        seed_default_workspace(&mut app);
        seed_other_workspace(&mut app);
        assert_eq!(app.project_rows().len(), 1, "only demo has a row");
        assert_eq!(app.tree.visible_project_count(), 1);

        let palette = Palette::new(&app.tree, true, false, &app.open_prs);
        let texts: Vec<&str> = palette.items.iter().map(|i| i.text.as_str()).collect();
        assert_eq!(
            texts,
            [
                "default",
                "default/demo",
                "default/demo/main",
                "default/demo/main/agent-1",
                "client",
                "client/secret",
                "client/secret/main",
            ],
            "open workspace first, every row pathed from its workspace down"
        );
        assert_eq!(
            palette.items[0].target,
            PaletteTarget::Workspace(nebula_core::WorkspaceId::default()),
            "a workspace is a jump target of its own"
        );
    }

    /// Picking a row in another workspace switches this instance to it
    /// first — otherwise the row it names isn't in any panel to land on.
    #[test]
    fn jumping_to_another_workspaces_worktree_switches_workspace_first() {
        let mut app = App::new();
        seed_tree(&mut app);
        seed_default_workspace(&mut app);
        seed_other_workspace(&mut app);
        let mut out = Vec::new();

        jump_to_target(
            &mut app,
            PaletteTarget::Worktree(WorktreeId("w9".into())),
            Landing::FocusOnly,
            &mut out,
        );
        assert_eq!(
            app.tree.active_workspace,
            nebula_core::WorkspaceId("ws2".into())
        );
        assert_eq!(
            app.selected_project().map(|p| p.name.clone()),
            Some("secret".into())
        );
        assert_eq!(
            app.selected_worktree().map(|w| w.branch.clone()),
            Some("main".into())
        );
        assert_eq!(app.focus, Focus::Sessions);
        assert!(app.flash.is_none(), "flash: {:?}", app.flash);
    }

    /// Crossing workspaces attaches exactly once. The switch deliberately
    /// skips the destination's remembered-session restore: doing it would
    /// attach that session and then detach it one request later, when the
    /// jump lands on the row it was actually asked for.
    #[test]
    fn a_cross_workspace_session_jump_attaches_only_the_session_picked() {
        let mut app = App::new();
        seed_tree(&mut app);
        seed_default_workspace(&mut app);
        seed_other_workspace(&mut app);
        seed_background_run(&mut app);
        // A second session over there, and it's the remembered one — so a
        // restoring switch would attach *it* before the jump attaches the
        // one actually picked.
        hse(
            &mut app,
            ServerEvent::EntityUpserted {
                entity: nebula_core::Entity::Agent(nebula_core::Agent {
                    id: AgentId("a8".into()),
                    worktree_id: WorktreeId("w9".into()),
                    name: "other".into(),
                    status: nebula_core::AgentStatus::Fresh,
                    archived: false,
                    archived_at: 0,
                    unseen: false,
                    kind: AgentKind::Claude,
                    model: None,
                    effort: None,
                    session_id: None,
                    cloud_session_id: None,
                    sort_order: 0,
                    status_changed_at: 0,
                    alive: true,
                    cloud_mirroring: false,
                }),
            },
        );
        // Park the pane on a session in the workspace we're leaving.
        let mut out = Vec::new();
        attach(&mut app, SessionRef::Agent(AgentId("a1".into())), &mut out);
        app.last_session_for_worktree.insert(
            WorktreeId("w9".into()),
            SessionRef::Agent(AgentId("a8".into())),
        );
        out.clear();

        jump_to_target(
            &mut app,
            PaletteTarget::Session(AgentId("a9".into())),
            Landing::Attach,
            &mut out,
        );
        let attaches: Vec<&ClientRequest> = out
            .iter()
            .filter(|r| matches!(r, ClientRequest::Attach { .. }))
            .collect();
        assert_eq!(
            attaches.len(),
            1,
            "one attach, not attach-detach-attach: {out:?}"
        );
        assert!(
            matches!(
                attaches[0],
                ClientRequest::Attach { session, .. } if session == &SessionRef::Agent(AgentId("a9".into()))
            ),
            "{:?}",
            attaches[0]
        );
        assert_eq!(app.focus, Focus::Terminal);
        assert_eq!(
            app.tree.active_workspace,
            nebula_core::WorkspaceId("ws2".into())
        );
    }

    /// A workspace row is a jump of its own: it switches and parks the
    /// cursor on the leftmost panel, the `w` switcher's Enter by another
    /// route.
    #[test]
    fn jumping_to_a_workspace_row_switches_and_focuses_the_column() {
        let mut app = App::new();
        seed_tree(&mut app);
        seed_default_workspace(&mut app);
        seed_other_workspace(&mut app);
        let mut out = Vec::new();

        jump_to_target(
            &mut app,
            PaletteTarget::Workspace(nebula_core::WorkspaceId("ws2".into())),
            Landing::FocusOnly,
            &mut out,
        );
        assert_eq!(
            app.tree.active_workspace,
            nebula_core::WorkspaceId("ws2".into())
        );
        assert_eq!(
            app.focus,
            Focus::Workspaces,
            "the column is shown by default"
        );
        assert!(
            out.iter()
                .any(|r| matches!(r, ClientRequest::OpenWorkspace { .. })),
            "the daemon is told which workspace this connection is on"
        );
    }

    /// Switching re-filters everything live: panel rows, an open palette,
    /// the selection, and the footer's workspace name.
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

        app.overlay = Some(Overlay::Palette(Palette::new(
            &app.tree,
            false,
            false,
            &app.open_prs,
        )));
        let mut out = Vec::new();
        switch_workspace(&mut app, nebula_core::WorkspaceId("ws2".into()), &mut out);
        assert_eq!(
            app.selected_project().map(|p| p.name.clone()),
            Some("secret".into()),
            "selection lands in the opened workspace"
        );
        match &app.overlay {
            Some(Overlay::Palette(palette)) => {
                assert!(
                    palette.items[0].text.starts_with("client"),
                    "the newly opened workspace's rows lead: {:?}",
                    palette.items.iter().map(|i| &i.text).collect::<Vec<_>>()
                );
                assert!(
                    palette.items.iter().any(|i| i.text.contains("demo")),
                    "`/` still reaches the workspace we left"
                );
            }
            other => panic!("palette should stay open, got {other:?}"),
        }

        // Close it before the panel assertions: `/` is deliberately not
        // workspace-scoped any more, so an open palette would put the
        // workspace we left back on the screen.
        app.overlay = None;
        terminal.draw(|f| ui::draw(f, &mut app)).unwrap();
        let text = buffer_text(&terminal);
        assert!(text.contains("◇ client"), "{text}");
        assert!(text.contains("secret"), "{text}");
        assert!(!text.contains("demo"), "{text}");
    }

    /// Switching away from a workspace and back brings the whole cursor
    /// home — project, worktree and session — not just the workspace. The
    /// project is the load-bearing part: the worktree and session are
    /// remembered per project and per worktree, so landing back on row 0
    /// would restore some other project's context.
    #[test]
    fn switching_back_to_a_workspace_restores_project_worktree_and_session() {
        use nebula_core::{
            Agent, AgentId, AgentStatus, Entity, Project, ProjectId, Worktree, WorktreeId,
        };
        let mut app = App::new();
        seed_tree(&mut app); // default: demo (p1) / main (w1) / a1
        seed_other_workspace(&mut app); // client: secret (p9) / main (w9)

        // A second project in the default workspace, on a non-main worktree
        // with its own session — so "the row we left on" is not row 0 at any
        // of the three levels.
        hse(
            &mut app,
            ServerEvent::EntityUpserted {
                entity: Entity::Project(Project {
                    workspace_id: Default::default(),
                    id: ProjectId("p2".into()),
                    name: "other".into(),
                    repo_path: "/tmp/other".into(),
                    sort_order: 1,
                }),
            },
        );
        for (id, branch, is_main, sort_order) in
            [("w2a", "main", true, 0), ("w2b", "feature", false, 1)]
        {
            hse(
                &mut app,
                ServerEvent::EntityUpserted {
                    entity: Entity::Worktree(Worktree {
                        id: WorktreeId(id.into()),
                        project_id: ProjectId("p2".into()),
                        path: format!("/tmp/other-{id}").into(),
                        branch: branch.into(),
                        is_main,
                        sort_order,
                    }),
                },
            );
        }
        hse(
            &mut app,
            ServerEvent::EntityUpserted {
                entity: Entity::Agent(Agent {
                    id: AgentId("a2".into()),
                    worktree_id: WorktreeId("w2b".into()),
                    name: "agent-2".into(),
                    status: AgentStatus::Fresh,
                    archived: false,
                    archived_at: 0,
                    unseen: false,
                    kind: nebula_core::AgentKind::Claude,
                    model: None,
                    effort: None,
                    session_id: None,
                    cloud_session_id: None,
                    sort_order: 0,
                    status_changed_at: 0,
                    alive: true,
                    cloud_mirroring: false,
                }),
            },
        );

        // Park the cursor on other / feature / agent-2.
        app.sel_project = 1;
        app.sel_worktree = 1;
        app.sel_session = 0;
        assert_eq!(
            app.selected_worktree().map(|w| w.branch.clone()),
            Some("feature".into())
        );

        let mut out = Vec::new();
        switch_workspace(&mut app, nebula_core::WorkspaceId("ws2".into()), &mut out);
        assert_eq!(
            app.selected_project().map(|p| p.name.clone()),
            Some("secret".into()),
            "a workspace with nothing remembered still opens on its first row"
        );

        out.clear();
        switch_workspace(&mut app, nebula_core::WorkspaceId::default(), &mut out);
        assert_eq!(
            app.selected_project().map(|p| p.name.clone()),
            Some("other".into()),
            "back on the project the workspace was left on, not row 0"
        );
        assert_eq!(
            app.selected_worktree().map(|w| w.branch.clone()),
            Some("feature".into()),
            "and that project's remembered worktree"
        );
        let a2 = SessionRef::Agent(AgentId("a2".into()));
        assert_eq!(
            app.selected_session_row().and_then(|r| r.sref()),
            Some(a2.clone()),
            "and that worktree's remembered session"
        );
        assert_eq!(
            app.term.as_ref().map(|t| t.sref.clone()),
            Some(a2.clone()),
            "the remembered session comes back in the pane too"
        );
        // The Attach itself waits out ATTACH_DEBOUNCE: walking the
        // Workspaces column runs a full switch per row, and a workspace
        // merely passed through must not cold-boot its agent CLI.
        assert!(
            !out.iter()
                .any(|r| matches!(r, ClientRequest::Attach { .. })),
            "the attach is debounced, not sent on the switch itself, got {out:?}"
        );
        fire_pending_attach(&mut app, &mut out);
        assert!(
            out.iter()
                .any(|r| matches!(r, ClientRequest::Attach { session, .. } if *session == a2)),
            "and lands once the cursor settles, got {out:?}"
        );
    }

    /// The 'default' workspace as an entity — `seed_tree`'s project points
    /// at it by id, but nothing lists it until the daemon's row arrives.
    fn seed_default_workspace(app: &mut App) {
        use nebula_core::{Entity, Workspace, WorkspaceId};
        hse(
            app,
            ServerEvent::EntityUpserted {
                entity: Entity::Workspace(Workspace {
                    id: WorkspaceId::default(),
                    name: "default".into(),
                }),
            },
        );
    }

    /// A running agent in the other workspace's checkout — the thing the
    /// Workspaces column exists to surface without opening it.
    fn seed_background_run(app: &mut App) {
        use nebula_core::{Agent, AgentId, AgentStatus, Entity, WorktreeId};
        hse(
            app,
            ServerEvent::EntityUpserted {
                entity: Entity::Agent(Agent {
                    id: AgentId("a9".into()),
                    worktree_id: WorktreeId("w9".into()),
                    name: "bg-run".into(),
                    status: AgentStatus::Running,
                    archived: false,
                    archived_at: 0,
                    unseen: false,
                    kind: nebula_core::AgentKind::Claude,
                    model: None,
                    effort: None,
                    session_id: None,
                    cloud_session_id: None,
                    sort_order: 0,
                    status_changed_at: 0,
                    alive: true,
                    cloud_mirroring: false,
                }),
            },
        );
    }

    /// A finished-and-unread session in the same background workspace as
    /// [`seed_background_run`], so the bar has something to count as done
    /// while the rollup still reads as running.
    fn seed_background_finished(app: &mut App) {
        use nebula_core::{Agent, AgentId, AgentStatus, Entity, WorktreeId};
        hse(
            app,
            ServerEvent::EntityUpserted {
                entity: Entity::Agent(Agent {
                    id: AgentId("a10".into()),
                    worktree_id: WorktreeId("w9".into()),
                    name: "bg-done".into(),
                    status: AgentStatus::Finished,
                    archived: false,
                    archived_at: 0,
                    unseen: true,
                    kind: nebula_core::AgentKind::Claude,
                    model: None,
                    effort: None,
                    session_id: None,
                    cloud_session_id: None,
                    sort_order: 1,
                    status_changed_at: 0,
                    alive: true,
                    cloud_mirroring: false,
                }),
            },
        );
    }
    /// Stepping through the Workspaces column runs a full `switch_workspace`
    /// per row, and each one restores that workspace's remembered session.
    /// Without the attach debounce every row merely passed through
    /// cold-boots an agent CLI nobody asked to see, and the boot the user IS
    /// waiting on queues behind them — the workspace-switch lag.
    #[test]
    fn walking_the_workspaces_column_attaches_only_where_it_stops() {
        use nebula_core::{
            Agent, AgentStatus, Entity, Project, ProjectId, Workspace, WorkspaceId, Worktree,
        };
        let mut app = App::new();
        seed_tree(&mut app);
        seed_default_workspace(&mut app);
        seed_other_workspace(&mut app); // ws2 → p9 → w9
        seed_background_run(&mut app); // a9 lives in w9

        // A third workspace, so the walk genuinely passes *through* ws2.
        hse(
            &mut app,
            ServerEvent::EntityUpserted {
                entity: Entity::Workspace(Workspace {
                    id: WorkspaceId("ws3".into()),
                    name: "third".into(),
                }),
            },
        );
        hse(
            &mut app,
            ServerEvent::EntityUpserted {
                entity: Entity::Project(Project {
                    workspace_id: WorkspaceId("ws3".into()),
                    id: ProjectId("p7".into()),
                    name: "third-proj".into(),
                    repo_path: "/tmp/third".into(),
                    sort_order: 7,
                }),
            },
        );
        hse(
            &mut app,
            ServerEvent::EntityUpserted {
                entity: Entity::Worktree(Worktree {
                    id: WorktreeId("w7".into()),
                    project_id: ProjectId("p7".into()),
                    path: "/tmp/third".into(),
                    branch: "main".into(),
                    is_main: true,
                    sort_order: 0,
                }),
            },
        );
        hse(
            &mut app,
            ServerEvent::EntityUpserted {
                entity: Entity::Agent(Agent {
                    id: AgentId("a7".into()),
                    worktree_id: WorktreeId("w7".into()),
                    name: "third-agent".into(),
                    status: AgentStatus::Fresh,
                    archived: false,
                    archived_at: 0,
                    unseen: false,
                    kind: AgentKind::Claude,
                    model: None,
                    effort: None,
                    session_id: None,
                    cloud_session_id: None,
                    sort_order: 0,
                    status_changed_at: 0,
                    alive: true,
                    cloud_mirroring: false,
                }),
            },
        );
        // Both destinations remember a session, so a restoring switch has
        // something to attach in each.
        app.last_session_for_worktree.insert(
            WorktreeId("w9".into()),
            SessionRef::Agent(AgentId("a9".into())),
        );
        app.last_session_for_worktree.insert(
            WorktreeId("w7".into()),
            SessionRef::Agent(AgentId("a7".into())),
        );

        app.focus = Focus::Workspaces;
        let mut out = Vec::new();
        move_selection(&mut app, 1, &mut out); // onto ws2…
        move_selection(&mut app, 1, &mut out); // …and straight through to ws3
        assert!(
            !out.iter()
                .any(|r| matches!(r, ClientRequest::Attach { .. })),
            "nothing attaches while the cursor is still moving: {out:?}"
        );
        assert!(
            !out.iter()
                .any(|r| matches!(r, ClientRequest::Attach { session, .. }
                if *session == SessionRef::Agent(AgentId("a9".into())))),
            "the workspace passed through never boots its agent: {out:?}"
        );

        fire_pending_attach(&mut app, &mut out);
        let attaches: Vec<_> = out
            .iter()
            .filter(|r| matches!(r, ClientRequest::Attach { .. }))
            .collect();
        assert_eq!(
            attaches.len(),
            1,
            "exactly one attach, for the row it stopped on: {out:?}"
        );
        assert!(
            matches!(out.last(), Some(ClientRequest::Attach { session, .. })
                if *session == SessionRef::Agent(AgentId("a7".into()))),
            "and it's the workspace the walk ended on: {out:?}"
        );
    }

    /// An attach whose session the daemon had reaped replays an empty ring,
    /// so the grid is blank for as long as the CLI takes to boot. The pane
    /// has to say that rather than look hung.
    #[test]
    fn a_booting_session_says_so_instead_of_showing_a_blank_pane() {
        let mut app = App::new();
        seed_tree(&mut app);
        let sref = SessionRef::Agent(AgentId("a1".into()));
        let mut out = Vec::new();
        attach_now(&mut app, sref.clone(), &mut out);
        // Wide, and without the Workspaces column: the terminal pane has to
        // be roomy enough that its text isn't truncated mid-assert.
        app.show_workspaces = false;
        let mut terminal = Terminal::new(TestBackend::new(140, 30)).unwrap();

        terminal.draw(|f| ui::draw(f, &mut app)).unwrap();
        let text = buffer_text(&terminal);
        assert!(
            text.contains("starting"),
            "a session with no output yet reads as starting:\n{text}"
        );

        // The empty replay on attach is not output — it must not clear the
        // notice, or the pane goes blank again with nothing to explain it.
        hse(
            &mut app,
            ServerEvent::Scrollback {
                session: sref.clone(),
                base_seq: 0,
                data: Vec::new(),
            },
        );
        terminal.draw(|f| ui::draw(f, &mut app)).unwrap();
        assert!(
            buffer_text(&terminal).contains("starting"),
            "an empty replay still means nothing has painted"
        );

        // First real bytes: the notice gives way to the PTY screen.
        hse(
            &mut app,
            ServerEvent::Output {
                session: sref,
                seq: 0,
                data: b"hello from the agent".to_vec(),
            },
        );
        terminal.draw(|f| ui::draw(f, &mut app)).unwrap();
        let text = buffer_text(&terminal);
        assert!(
            text.contains("hello from the agent"),
            "the real screen replaces it:\n{text}"
        );
        assert!(
            !text.contains("starting"),
            "and the notice is gone:\n{text}"
        );
    }

    // ---- the Workspaces column ----

    // ---- the Workspaces bar ----

    /// `Shift+W` shows and hides the Workspaces bar. Hiding it parks a
    /// cursor that was in it on Projects — there's nothing left to drive —
    /// and showing it again doesn't steal focus back.
    #[test]
    fn shift_w_toggles_the_workspaces_bar_and_parks_focus() {
        // The toggle writes the setting through, so pin the config to a
        // temp file — otherwise the suite edits the dev's real one.
        with_default_config(|| {
            let mut app = App::new();
            seed_tree(&mut app);
            seed_default_workspace(&mut app);
            let mut out = Vec::new();
            assert!(app.show_workspaces, "shown until hidden");

            app.focus = Focus::Workspaces;
            press(&mut app, KeyCode::Char('W'), KeyModifiers::SHIFT, &mut out);
            assert!(!app.show_workspaces);
            assert_eq!(app.focus, Focus::Projects, "focus leaves the hidden bar");
            press(&mut app, KeyCode::Char('W'), KeyModifiers::SHIFT, &mut out);
            assert!(app.show_workspaces);
            assert_eq!(app.focus, Focus::Projects, "showing it doesn't steal focus");

            // The draw follows: the bar tops the body with PROJECTS three
            // rows under it on the same column, then it's gone and the
            // panels take the top row back.
            let mut terminal = Terminal::new(TestBackend::new(140, 30)).unwrap();
            terminal.draw(|f| ui::draw(f, &mut app)).unwrap();
            let text = buffer_text(&terminal);
            let lines: Vec<&str> = text.lines().collect();
            assert!(
                lines[1].starts_with("   WORKSPACES"),
                "the bar tops the body:\n{text}"
            );
            assert!(
                lines[1 + crate::app::WORKSPACES_BAR_H as usize].starts_with("   PROJECTS"),
                "PROJECTS sits directly under it, same column:\n{text}"
            );
            press(&mut app, KeyCode::Char('W'), KeyModifiers::SHIFT, &mut out);
            terminal.draw(|f| ui::draw(f, &mut app)).unwrap();
            let text = buffer_text(&terminal);
            let lines: Vec<&str> = text.lines().collect();
            assert!(
                lines[1].starts_with("   DEFAULT"),
                "hidden: the projects column takes the top row, under the \
                 open workspace's name:\n{text}"
            );
            assert!(!text.contains("WORKSPACES"), "{text}");
        });
    }

    /// Hiding the bar leaves nothing on screen naming the open workspace,
    /// so the Projects header takes the job: `PROJECTS` becomes the
    /// workspace's own name, it retitles on a switch, and showing the bar
    /// again hands the header back.
    #[test]
    fn a_hidden_bar_moves_the_workspace_name_onto_the_projects_header() {
        let mut app = App::new();
        seed_tree(&mut app);
        seed_default_workspace(&mut app);
        seed_other_workspace(&mut app);
        let mut out = Vec::new();
        let mut terminal = Terminal::new(TestBackend::new(140, 30)).unwrap();
        let draw = |app: &mut App, terminal: &mut Terminal<TestBackend>| {
            terminal.draw(|f| ui::draw(f, app)).unwrap();
            buffer_text(terminal)
        };

        app.show_workspaces = false;
        let text = draw(&mut app, &mut terminal);
        assert!(
            text.contains("DEFAULT \u{b7} 1"),
            "the header names the open workspace, count intact:\n{text}"
        );
        assert!(!text.contains("PROJECTS"), "{text}");

        switch_workspace(&mut app, nebula_core::WorkspaceId("ws2".into()), &mut out);
        let text = draw(&mut app, &mut terminal);
        assert!(text.contains("CLIENT"), "a switch retitles it:\n{text}");
        assert!(!text.contains("DEFAULT"), "{text}");

        // Shown again, the bar names the workspace and the column goes back
        // to naming itself.
        app.show_workspaces = true;
        let text = draw(&mut app, &mut terminal);
        assert!(text.contains("PROJECTS"), "{text}");
        assert!(text.contains("WORKSPACES"), "{text}");
    }

    /// The bar lists every workspace with the rollup of the agents under
    /// it, so a run in a workspace you don't have open still shows: the
    /// shortcut digit, the rollup dot and a "done" count ride the name, and
    /// the open workspace is the selected tab. The dot and the badge answer
    /// different questions — the dot is what the workspace is doing (a run
    /// outranks a finished turn), the badge is how much finished unread —
    /// so a workspace mid-run still counts what's waiting to be read.
    #[test]
    fn workspaces_bar_rolls_up_every_workspace() {
        use nebula_core::{AgentStatus, WorkspaceId};
        let mut app = App::new();
        seed_tree(&mut app);
        seed_default_workspace(&mut app);
        seed_other_workspace(&mut app);
        seed_background_run(&mut app);
        seed_background_finished(&mut app);

        let other = WorkspaceId("ws2".into());
        assert_eq!(app.workspace_rollup(&other), Some(AgentStatus::Running));
        assert_eq!(app.workspace_unseen(&other), 1);
        assert_eq!(
            app.workspace_rollup(&WorkspaceId::default()),
            Some(AgentStatus::Fresh),
            "demo's never-run agent"
        );
        assert_eq!(app.workspace_unseen(&WorkspaceId::default()), 0);

        let mut terminal = Terminal::new(TestBackend::new(140, 30)).unwrap();
        terminal.draw(|f| ui::draw(f, &mut app)).unwrap();
        let text = buffer_text(&terminal);
        assert!(text.contains("WORKSPACES · 2"), "{text}");
        let buffer = terminal.backend().buffer();

        // "● " sits two cells ahead of the name, in the running color; the
        // done count follows the name.
        let (x, y) = find_cell(&terminal, "client");
        let dot = &buffer[(x - 2, y)];
        assert_eq!(dot.symbol(), "●", "{text}");
        assert_eq!(dot.fg, app.theme.warn, "running dot:\n{text}");
        let row: String = (0..buffer.area.width)
            .map(|x| buffer[(x, y)].symbol())
            .collect();
        assert!(row.contains("client 1 done"), "done count:\n{text}");
        // Its shortcut digit leads the tab, dim on an unopened one.
        assert_eq!(buffer[(x - 4, y)].symbol(), "2", "{text}");
        assert_eq!(buffer[(x - 4, y)].fg, app.theme.dim, "{text}");

        // The open workspace's tab carries the selection fill, and its
        // digit is accented.
        let (x, y) = find_cell(&terminal, "default");
        assert_eq!(buffer[(x, y)].bg, app.theme.sel_bg, "open tab:\n{text}");
        // The fill takes the bar's padding rows too, so the tab reads as
        // one block rather than a highlighted row.
        assert_eq!(buffer[(x, y - 1)].bg, app.theme.sel_bg, "top pad:\n{text}");
        assert_eq!(
            buffer[(x, y + 1)].bg,
            app.theme.sel_bg,
            "bottom pad:\n{text}"
        );
        assert_eq!(buffer[(x - 4, y)].symbol(), "1", "{text}");
        assert_eq!(buffer[(x - 4, y)].fg, app.theme.accent, "{text}");
        // A fresh dot is dim, lifted to muted on the selection fill.
        assert_eq!(buffer[(x - 2, y)].fg, app.theme.muted, "fresh dot:\n{text}");

        // The rule under the bar stays unbroken beneath the open tab — it
        // becomes that tab's underline, in the tab's rollup STATUS DOT
        // color (the fresh dot's gray, lifted to muted like the dot), so
        // the tab reads as attached to the panels below it.
        // A half block, not a line glyph: it paints from the cell's top
        // edge, flush against the tab's fill. A `━` draws at the midline
        // and leaves a strip of background above it — a visible gap
        // between the tab and its own underline.
        assert_eq!(buffer[(x, y + 2)].symbol(), "▀", "tab join:\n{text}");
        assert_eq!(buffer[(x, y + 2)].fg, app.theme.muted, "{text}");
        assert_eq!(buffer[(0, y + 2)].symbol(), "─", "{text}");
        assert_eq!(buffer[(0, y + 2)].fg, app.theme.edge, "{text}");
    }

    /// ←/→ in the bar switch workspaces outright — the cursor IS the open
    /// workspace — and a click on a tab does the same.
    #[test]
    fn walking_or_clicking_the_workspaces_bar_switches_workspace() {
        use nebula_core::WorkspaceId;
        let mut app = App::new();
        seed_tree(&mut app);
        seed_default_workspace(&mut app);
        seed_other_workspace(&mut app);
        let mut out = Vec::new();
        let other = WorkspaceId("ws2".into());

        app.focus = Focus::Workspaces;
        press(&mut app, KeyCode::Right, KeyModifiers::NONE, &mut out);
        assert_eq!(app.tree.active_workspace, other);
        assert_eq!(
            app.selected_project().map(|p| p.name.clone()),
            Some("secret".into()),
            "the panels re-scope"
        );
        assert!(
            out.iter()
                .any(|r| matches!(r, ClientRequest::OpenWorkspace { id, .. } if *id == other)),
            "the daemon is told: {out:?}"
        );
        assert_eq!(app.focus, Focus::Workspaces, "focus stays in the bar");
        press(&mut app, KeyCode::Right, KeyModifiers::NONE, &mut out);
        assert_eq!(app.tree.active_workspace, other, "clamps at the end");
        press(&mut app, KeyCode::Left, KeyModifiers::NONE, &mut out);
        assert_eq!(app.tree.active_workspace, WorkspaceId::default());

        // The positional shortcuts reach the same tabs from any panel,
        // without dragging focus up to the bar.
        app.focus = Focus::Sessions;
        press(&mut app, KeyCode::Char('2'), KeyModifiers::NONE, &mut out);
        assert_eq!(app.tree.active_workspace, other, "2 opens the second tab");
        assert_eq!(app.focus, Focus::Sessions, "focus stays put");
        press(&mut app, KeyCode::Char('1'), KeyModifiers::SUPER, &mut out);
        assert_eq!(
            app.tree.active_workspace,
            WorkspaceId::default(),
            "⌘1 opens the first — where the emulator delivers it"
        );
        press(&mut app, KeyCode::Char('9'), KeyModifiers::NONE, &mut out);
        assert_eq!(
            app.tree.active_workspace,
            WorkspaceId::default(),
            "a digit past the last tab is a no-op"
        );
        assert_eq!(app.flash.take(), Some("no workspace 9".into()));

        app.focus = Focus::Workspaces;

        // Enter steps into Projects, the way Enter on a project steps into
        // its worktrees.
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE, &mut out);
        assert_eq!(app.focus, Focus::Projects);

        // A click on a tab opens that workspace and focuses the bar.
        let mut terminal = Terminal::new(TestBackend::new(140, 30)).unwrap();
        terminal.draw(|f| ui::draw(f, &mut app)).unwrap();
        let (x, y) = find_cell(&terminal, "client");
        handle_mouse(
            &mut app,
            mev(MouseEventKind::Down(MouseButton::Left), x, y),
            &mut out,
        );
        assert_eq!(app.tree.active_workspace, other);
        assert_eq!(app.focus, Focus::Workspaces);
        assert_eq!(
            app.selected_project().map(|p| p.name.clone()),
            Some("secret".into())
        );
    }

    /// Seed `n` extra workspaces named ws-2..ws-(n+1), so the bar has more
    /// tabs than a narrow screen can hold. Each gets a project of its own,
    /// so opening one shows the panels rather than the first-run splash.
    fn seed_many_workspaces(app: &mut App, n: usize) {
        use nebula_core::{Entity, Project, ProjectId, Workspace, WorkspaceId};
        for i in 2..=n + 1 {
            hse(
                app,
                ServerEvent::EntityUpserted {
                    entity: Entity::Workspace(Workspace {
                        id: WorkspaceId(format!("ws{i}")),
                        name: format!("ws-{i}"),
                    }),
                },
            );
            hse(
                app,
                ServerEvent::EntityUpserted {
                    entity: Entity::Project(Project {
                        workspace_id: WorkspaceId(format!("ws{i}")),
                        id: ProjectId(format!("p{i}")),
                        name: format!("proj-{i}"),
                        repo_path: format!("/tmp/proj-{i}").into(),
                        sort_order: i as i64,
                    }),
                },
            );
        }
    }

    /// The bar's whole point of alignment: `WORKSPACES` sits on the same
    /// row-1 / x-3 grid the panel headers use, so it reads as the tier
    /// directly above `PROJECTS`, and its rule spans the body — broken only
    /// under the open tab.
    #[test]
    fn the_workspaces_bar_sits_directly_above_projects() {
        let mut app = App::new();
        seed_tree(&mut app);
        seed_default_workspace(&mut app);
        seed_other_workspace(&mut app);

        let mut terminal = Terminal::new(TestBackend::new(140, 30)).unwrap();
        terminal.draw(|f| ui::draw(f, &mut app)).unwrap();
        let text = buffer_text(&terminal);
        let lines: Vec<&str> = text.lines().collect();

        let bar_row = 1;
        let panel_row = bar_row + crate::app::WORKSPACES_BAR_H as usize;
        assert_eq!(
            lines[bar_row].find("WORKSPACES"),
            Some(3),
            "label indent:\n{text}"
        );
        assert_eq!(
            lines[panel_row].find("PROJECTS"),
            Some(3),
            "same column, the bar's height down:\n{text}"
        );
        // Tabs share that row, to the right of the label.
        let tabs_at = lines[bar_row].find("default").expect("first tab drawn");
        assert!(tabs_at > 3 + "WORKSPACES".len(), "tabs sit right:\n{text}");
        assert!(lines[bar_row].contains("client"), "{text}");

        // The rule closes the bar off, full width bar the open tab's gap.
        // It is the bar's last row, a blank pad row below the tabs.
        assert!(
            lines[bar_row + 1].trim().is_empty(),
            "a padding row under the tabs:\n{text}"
        );
        let rule = lines[bar_row + 2];
        assert!(rule.starts_with("───"), "rule leads the row:\n{text}");
        assert!(
            rule.trim_end().ends_with('─'),
            "and runs to the end:\n{text}"
        );
        assert!(
            rule.contains("▀▀"),
            "turning into an underline under the open tab:\n{text}"
        );
    }

    /// More tabs than fit: the bar scrolls to keep the open one on screen
    /// and marks what it dropped, rather than silently losing workspaces.
    #[test]
    fn workspace_tabs_scroll_to_keep_the_open_one_visible() {
        use nebula_core::WorkspaceId;
        let mut app = App::new();
        seed_tree(&mut app);
        seed_default_workspace(&mut app);
        seed_many_workspaces(&mut app, 12);
        let mut out = Vec::new();

        let mut terminal = Terminal::new(TestBackend::new(90, 20)).unwrap();
        terminal.draw(|f| ui::draw(f, &mut app)).unwrap();
        let text = buffer_text(&terminal);
        let bar = text.lines().nth(1).unwrap().to_string();
        assert!(bar.contains("default"), "open tab is first:\n{text}");
        assert!(bar.contains('›'), "and the rest overflow right:\n{text}");
        assert!(
            !bar.contains('‹'),
            "nothing dropped on the left yet:\n{text}"
        );

        // Open the last one: the window slides so it is on screen, and the
        // left overflow mark appears.
        switch_workspace(&mut app, WorkspaceId("ws13".into()), &mut out);
        terminal.draw(|f| ui::draw(f, &mut app)).unwrap();
        let text = buffer_text(&terminal);
        let bar = text.lines().nth(1).unwrap().to_string();
        assert!(
            bar.contains("ws-13"),
            "open tab scrolled into view:\n{text}"
        );
        assert!(bar.contains('‹'), "with the left overflow marked:\n{text}");
        assert!(!bar.contains("default"), "and the head dropped:\n{text}");
    }

    /// The footer's `◇ workspace` nameplate is a button: a click opens the
    /// switcher — under the splash too, where an empty workspace is exactly
    /// when you'd want to leave it. The rest of the bar stays inert.
    #[test]
    fn clicking_the_footer_workspace_opens_the_switcher() {
        let mut app = App::new();
        seed_default_workspace(&mut app);
        let mut out = Vec::new();
        let mut terminal = Terminal::new(TestBackend::new(140, 30)).unwrap();

        terminal.draw(|f| ui::draw(f, &mut app)).unwrap();
        assert!(app.splash_showing(), "no projects yet");
        let (x, y) = find_cell(&terminal, "◇ default");
        handle_mouse(
            &mut app,
            mev(MouseEventKind::Down(MouseButton::Left), x + 3, y),
            &mut out,
        );
        assert!(
            matches!(&app.overlay, Some(Overlay::Menu(m)) if m.is_workspace_picker()),
            "under the splash: {:?}",
            app.overlay
        );

        app.overlay = None;
        seed_tree(&mut app);
        terminal.draw(|f| ui::draw(f, &mut app)).unwrap();
        let (x, y) = find_cell(&terminal, "◇ default");
        handle_mouse(
            &mut app,
            mev(MouseEventKind::Down(MouseButton::Left), x, y),
            &mut out,
        );
        assert!(
            matches!(&app.overlay, Some(Overlay::Menu(m)) if m.is_workspace_picker()),
            "with the panels up: {:?}",
            app.overlay
        );

        app.overlay = None;
        handle_mouse(
            &mut app,
            mev(MouseEventKind::Down(MouseButton::Left), x + 40, y),
            &mut out,
        );
        assert!(app.overlay.is_none(), "the hints aren't a button");
    }

    /// The Workspaces bar spans the top, so it costs the panels rows, not
    /// columns: splitter x's and the width budget are the same whether it
    /// is shown or hidden, and there is no splitter for it to own.
    #[test]
    fn the_workspaces_bar_costs_rows_not_columns() {
        use crate::app::{DEFAULT_PANEL_WIDTHS, WORKSPACES_BAR_H};
        let mut app = App::new();
        assert!(app.show_workspaces);
        assert_eq!(app.splitter_indices(), vec![0, 1, 2]);
        assert_eq!(app.workspaces_bar_h(), WORKSPACES_BAR_H);
        assert_eq!(app.splitter_x(0), 20);
        assert_eq!(app.splitter_x(2), 74);

        // Dragging the projects|worktrees boundary to screen x=45 leaves
        // Projects 45 wide: nothing sits ahead of it any more.
        app.set_splitter(0, 45, 160);
        assert_eq!(app.panel_widths[0], 45);

        // A 100-wide body: only the terminal pane's minimum comes off the
        // budget, and Sessions gives up the rest.
        app.panel_widths = DEFAULT_PANEL_WIDTHS;
        app.normalize_panel_widths(100);
        assert_eq!(
            app.panel_widths, DEFAULT_PANEL_WIDTHS,
            "80 columns of budget fit them all"
        );

        // Hiding the bar changes nothing horizontal.
        app.show_workspaces = false;
        assert_eq!(app.workspaces_bar_h(), 0);
        assert_eq!(app.splitter_indices(), vec![0, 1, 2]);
        assert_eq!(app.splitter_x(0), 20);
    }

    /// Tab / Shift+Tab walk through the bar only while it's shown. The walk
    /// back stops dead at whichever column is first — the bar, or Projects
    /// when it's hidden. ← and → never reach the bar at
    /// all — it is above the panels, not beside them — so inside it they
    /// walk the tabs instead, and ↓ is the way out.
    #[test]
    fn focus_walk_includes_the_workspaces_bar_only_when_shown() {
        let mut app = App::new();
        seed_tree(&mut app);
        let mut out = Vec::new();
        let go = |app: &mut App, out: &mut Vec<ClientRequest>, code: KeyCode| {
            press(app, code, KeyModifiers::NONE, out);
            app.focus
        };

        app.focus = Focus::Projects;
        assert_eq!(
            go(&mut app, &mut out, KeyCode::Left),
            Focus::Projects,
            "a single ← at the first column stays: the bar takes ←,← or ⇧Tab"
        );
        assert_eq!(
            go(&mut app, &mut out, KeyCode::Left),
            Focus::Workspaces,
            "←,← jumps up into the bar, as ⇧Tab does"
        );
        assert_eq!(
            go(&mut app, &mut out, KeyCode::Down),
            Focus::Workspaces,
            "a single ↓ in the bar stays: the way down is ↓,↓"
        );
        assert_eq!(go(&mut app, &mut out, KeyCode::Down), Focus::Projects);
        assert_eq!(go(&mut app, &mut out, KeyCode::BackTab), Focus::Workspaces);
        assert_eq!(
            go(&mut app, &mut out, KeyCode::Left),
            Focus::Workspaces,
            "in the bar, ← walks tabs and stays put"
        );
        assert_eq!(
            go(&mut app, &mut out, KeyCode::Down),
            Focus::Workspaces,
            "← in between broke nothing: this ↓ is a first press"
        );
        assert_eq!(
            go(&mut app, &mut out, KeyCode::Down),
            Focus::Projects,
            "↓,↓ steps out of the bar"
        );
        assert_eq!(go(&mut app, &mut out, KeyCode::BackTab), Focus::Workspaces);
        assert_eq!(
            go(&mut app, &mut out, KeyCode::BackTab),
            Focus::Workspaces,
            "shown: the walk back stops dead on the bar"
        );
        assert_eq!(
            go(&mut app, &mut out, KeyCode::Tab),
            Focus::Projects,
            "and forward steps down out of it"
        );

        app.show_workspaces = false;
        app.focus = Focus::Projects;
        assert_eq!(
            go(&mut app, &mut out, KeyCode::Left),
            Focus::Projects,
            "hidden: ← stops"
        );
        assert_eq!(
            go(&mut app, &mut out, KeyCode::BackTab),
            Focus::Projects,
            "hidden: Projects is the first column, so the walk back stops there"
        );
    }

    /// n / r / d in the bar act on the open workspace the way the
    /// switcher's do, and m lists the same three verbs.
    #[test]
    fn workspaces_bar_verbs_act_on_the_open_workspace() {
        use nebula_core::WorkspaceId;
        let mut app = App::new();
        seed_tree(&mut app);
        seed_default_workspace(&mut app);
        let mut out = Vec::new();
        app.focus = Focus::Workspaces;

        press(&mut app, KeyCode::Char('n'), KeyModifiers::NONE, &mut out);
        assert!(
            matches!(&app.overlay, Some(Overlay::Prompt(p)) if p.kind == PromptKind::NewWorkspace),
            "n: {:?}",
            app.overlay
        );
        app.overlay = None;

        press(&mut app, KeyCode::Char('r'), KeyModifiers::NONE, &mut out);
        match &app.overlay {
            Some(Overlay::Prompt(p)) => {
                assert_eq!(
                    p.kind,
                    PromptKind::RenameWorkspace {
                        id: WorkspaceId::default()
                    }
                );
                assert_eq!(
                    p.input.as_str(),
                    "default",
                    "prefilled with the current name"
                );
            }
            other => panic!("r: {other:?}"),
        }
        app.overlay = None;

        // d: a confirm first — Esc backs out to the panels with nothing
        // sent, y sends the request (the daemon still refuses non-empty
        // ones).
        press(&mut app, KeyCode::Char('d'), KeyModifiers::NONE, &mut out);
        match &app.overlay {
            Some(Overlay::Confirm(c)) => {
                assert_eq!(c.title, "Delete workspace");
                assert!(c.message.contains("'default'"), "{}", c.message);
                assert!(matches!(
                    &c.action,
                    PendingAction::RemoveWorkspace { id, reopen_picker: None }
                        if *id == WorkspaceId::default()
                ));
            }
            other => panic!("d should confirm, got {other:?}"),
        }
        press(&mut app, KeyCode::Esc, KeyModifiers::NONE, &mut out);
        assert!(app.overlay.is_none(), "Esc lands on the panels");
        assert!(
            !out.iter()
                .any(|r| matches!(r, ClientRequest::RemoveWorkspace { .. })),
            "nothing sent on Esc: {out:?}"
        );
        press(&mut app, KeyCode::Char('d'), KeyModifiers::NONE, &mut out);
        press(&mut app, KeyCode::Char('y'), KeyModifiers::NONE, &mut out);
        assert!(app.overlay.is_none());
        assert!(
            out.iter().any(|r| matches!(
                r,
                ClientRequest::RemoveWorkspace { id, .. } if *id == WorkspaceId::default()
            )),
            "{out:?}"
        );

        press(&mut app, KeyCode::Char('m'), KeyModifiers::NONE, &mut out);
        match &app.overlay {
            Some(Overlay::Menu(m)) => {
                let labels: Vec<&str> = m.items.iter().map(|i| i.label.as_str()).collect();
                assert_eq!(
                    labels,
                    ["New workspace", "Rename workspace", "Delete workspace"]
                );
                assert!(!m.is_workspace_picker(), "verbs, not the switcher");
            }
            other => panic!("m: {other:?}"),
        }
    }

    /// Hiding the column writes the setting there and then, so the next
    /// launch starts hidden — no clean quit required.
    #[test]
    fn hiding_the_workspaces_column_persists_to_the_config() {
        let dir = tempfile::tempdir().unwrap();
        crate::config::with_config_path(dir.path().join("config.json"), || {
            let mut app = App::new();
            let mut out = Vec::new();
            assert!(app.show_workspaces, "shown until hidden");

            press(&mut app, KeyCode::Char('W'), KeyModifiers::SHIFT, &mut out);
            assert!(!app.show_workspaces);
            assert!(
                !crate::config::Config::load().show_workspaces,
                "the hotkey saved it"
            );

            // A fresh launch reads the file back.
            let mut next = App::new();
            apply_config(&mut next, &crate::config::Config::load());
            assert!(!next.show_workspaces);

            press(&mut app, KeyCode::Char('W'), KeyModifiers::SHIFT, &mut out);
            assert!(crate::config::Config::load().show_workspaces);
        });
    }

    /// The Appearance tab edits the same value the hotkey does, live — and
    /// hiding the column out from under the cursor moves it to Projects.
    #[test]
    fn the_appearance_tab_toggles_the_workspaces_bar() {
        let dir = tempfile::tempdir().unwrap();
        crate::config::with_config_path(dir.path().join("config.json"), || {
            let mut app = App::new();
            app.focus = Focus::Workspaces;
            let (tab, row) = crate::config::locate(crate::config::SettingKind::ShowWorkspaces)
                .expect("the setting has a row");

            apply_setting_at(&mut app, tab, row, 0);
            assert!(!app.show_workspaces);
            assert_eq!(app.focus, Focus::Projects, "no bar left to drive");
            assert!(!crate::config::Config::load().show_workspaces);

            apply_setting_at(&mut app, tab, row, 0);
            assert!(app.show_workspaces);

            // And the row is actually reachable and readable on that tab.
            let mut out = Vec::new();
            press(&mut app, KeyCode::Char('s'), KeyModifiers::NONE, &mut out);
            for _ in 0..tab {
                press(&mut app, KeyCode::Tab, KeyModifiers::NONE, &mut out);
            }
            let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
            terminal.draw(|f| ui::draw(f, &mut app)).unwrap();
            let text = buffer_text(&terminal);
            assert!(text.contains("Workspaces bar"), "{text}");
            assert!(text.contains("Appearance"), "{text}");
        });
    }

    #[test]
    fn shift_p_and_shift_b_hide_panels_independently_and_persist() {
        with_default_config(|| {
            let mut app = App::new();
            seed_tree(&mut app);
            let mut out = Vec::new();

            app.focus = Focus::Projects;
            press(&mut app, KeyCode::Char('P'), KeyModifiers::SHIFT, &mut out);
            assert!(app.hide_projects);
            assert!(!app.hide_worktrees);
            assert_eq!(app.focus, Focus::Worktrees);

            press(&mut app, KeyCode::Char('B'), KeyModifiers::SHIFT, &mut out);
            assert!(app.hide_projects);
            assert!(app.hide_worktrees);
            assert_eq!(app.focus, Focus::Sessions);
            let saved = crate::config::Config::load();
            assert!(saved.hide_projects);
            assert!(saved.hide_worktrees);

            let mut next = App::new();
            apply_config(&mut next, &saved);
            assert!(next.hide_projects);
            assert!(next.hide_worktrees);
            assert_eq!(next.focus, Focus::Sessions);

            press(&mut app, KeyCode::Char('P'), KeyModifiers::SHIFT, &mut out);
            assert!(!app.hide_projects);
            assert!(app.hide_worktrees);
            assert_eq!(app.focus, Focus::Sessions, "showing does not steal focus");
            press(&mut app, KeyCode::Char('B'), KeyModifiers::SHIFT, &mut out);
            assert!(!app.hide_projects);
            assert!(!app.hide_worktrees);
            assert_eq!(app.focus, Focus::Sessions, "showing does not steal focus");
        });
    }

    #[test]
    fn focus_walk_skips_hidden_panels() {
        let mut app = App::new();
        let mut out = Vec::new();
        app.hide_projects = true;
        app.hide_worktrees = true;
        app.focus = Focus::Workspaces;

        press(&mut app, KeyCode::Tab, KeyModifiers::NONE, &mut out);
        assert_eq!(app.focus, Focus::Sessions);
        press(&mut app, KeyCode::BackTab, KeyModifiers::SHIFT, &mut out);
        assert_eq!(app.focus, Focus::Workspaces);

        app.show_workspaces = false;
        app.focus = Focus::Sessions;
        press(&mut app, KeyCode::BackTab, KeyModifiers::SHIFT, &mut out);
        assert_eq!(app.focus, Focus::Sessions, "no hidden stop to walk into");

        app.hide_worktrees = false;
        press(&mut app, KeyCode::BackTab, KeyModifiers::SHIFT, &mut out);
        assert_eq!(app.focus, Focus::Worktrees);
        app.hide_projects = false;
        press(&mut app, KeyCode::BackTab, KeyModifiers::SHIFT, &mut out);
        assert_eq!(app.focus, Focus::Projects);
    }

    #[test]
    fn hidden_panels_give_their_width_to_the_terminal_and_restore() {
        let mut app = App::new();
        seed_tree(&mut app);
        seed_default_workspace(&mut app);
        let widths = app.panel_widths;
        let mut terminal = Terminal::new(TestBackend::new(160, 30)).unwrap();

        terminal.draw(|f| ui::draw(f, &mut app)).unwrap();
        assert_eq!(app.term_area.x, widths.iter().sum::<u16>() + 1);
        let all_text = buffer_text(&terminal);
        assert!(all_text.contains("PROJECTS"), "{all_text}");
        assert!(all_text.contains("WORKTREES"), "{all_text}");

        app.focus = Focus::Projects;
        set_hide_projects(&mut app, true);
        terminal.draw(|f| ui::draw(f, &mut app)).unwrap();
        let projects_hidden = buffer_text(&terminal);
        assert!(!projects_hidden.contains("PROJECTS"), "{projects_hidden}");
        assert!(projects_hidden.contains("WORKTREES"), "{projects_hidden}");
        assert_eq!(app.term_area.x, widths[1] + widths[2] + 1);
        assert_eq!(app.splitter_indices(), vec![1, 2]);
        assert_eq!(app.focus, Focus::Worktrees);

        set_hide_worktrees(&mut app, true);
        terminal.draw(|f| ui::draw(f, &mut app)).unwrap();
        let both_hidden = buffer_text(&terminal);
        assert!(!both_hidden.contains("PROJECTS"), "{both_hidden}");
        assert!(!both_hidden.contains("WORKTREES"), "{both_hidden}");
        assert!(both_hidden.contains("SESSIONS"), "{both_hidden}");
        assert!(both_hidden.contains("⇧P: show projects"), "{both_hidden}");
        assert!(both_hidden.contains("⇧B: show worktrees"), "{both_hidden}");
        assert_eq!(app.term_area.x, widths[2] + 1);
        assert_eq!(app.splitter_indices(), vec![2]);
        assert_eq!(app.panel_widths, widths, "hidden widths stay remembered");
        assert_eq!(app.focus, Focus::Sessions);

        set_hide_projects(&mut app, false);
        set_hide_worktrees(&mut app, false);
        terminal.draw(|f| ui::draw(f, &mut app)).unwrap();
        let restored = buffer_text(&terminal);
        assert!(restored.contains("PROJECTS"), "{restored}");
        assert!(restored.contains("WORKTREES"), "{restored}");
        assert_eq!(app.term_area.x, widths.iter().sum::<u16>() + 1);
        assert_eq!(app.panel_widths, widths);
        assert_eq!(app.focus, Focus::Sessions, "restoring does not steal focus");
    }

    #[test]
    fn appearance_rows_toggle_project_and_worktree_panels_live() {
        with_default_config(|| {
            let mut app = App::new();
            app.focus = Focus::Projects;
            let (tab, projects_row) =
                crate::config::locate(crate::config::SettingKind::HideProjects).unwrap();
            let (_, worktrees_row) =
                crate::config::locate(crate::config::SettingKind::HideWorktrees).unwrap();

            apply_setting_at(&mut app, tab, projects_row, 0);
            assert!(app.hide_projects);
            assert_eq!(app.focus, Focus::Worktrees);
            apply_setting_at(&mut app, tab, projects_row, 0);
            assert!(!app.hide_projects);
            assert_eq!(app.focus, Focus::Worktrees);

            apply_setting_at(&mut app, tab, worktrees_row, 0);
            assert!(app.hide_worktrees);
            assert_eq!(app.focus, Focus::Sessions);
            assert!(crate::config::Config::load().hide_worktrees);

            press(
                &mut app,
                KeyCode::Char('s'),
                KeyModifiers::NONE,
                &mut Vec::new(),
            );
            for _ in 0..tab {
                press(&mut app, KeyCode::Tab, KeyModifiers::NONE, &mut Vec::new());
            }
            let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
            terminal.draw(|f| ui::draw(f, &mut app)).unwrap();
            let text = buffer_text(&terminal);
            assert!(text.contains("Projects panel"), "{text}");
            assert!(text.contains("Worktrees panel"), "{text}");
            assert!(text.contains("hidden"), "{text}");
        });
    }

    /// Opening an empty workspace clears the child panels and the terminal
    /// pane instead of keeping the previous workspace's session on screen —
    /// and it is NOT a first run, so the splash stays down and the panels
    /// (Workspaces column included) stay on screen.
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
        switch_workspace(&mut app, WorkspaceId("ws-empty".into()), &mut out);
        assert!(app.project_rows().is_empty(), "no visible projects");
        assert!(app.term.is_none(), "pane blanked");
        assert!(
            out.iter()
                .any(|r| matches!(r, ClientRequest::Detach { .. })),
            "old session detached: {out:?}"
        );
        assert!(
            !app.splash_showing(),
            "an empty non-default workspace is not a first run"
        );
    }

    /// The splash is scoped to the default workspace: stepping the
    /// Workspaces column onto an empty workspace the user created keeps the
    /// column and the "no projects yet" panels on screen, and stepping
    /// back to an (empty) default brings the splash back.
    #[test]
    fn empty_non_default_workspace_keeps_the_panels_not_the_splash() {
        use nebula_core::{Entity, Workspace, WorkspaceId};
        let mut app = App::new();
        seed_default_workspace(&mut app);
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
        let mut terminal = Terminal::new(TestBackend::new(140, 30)).unwrap();

        terminal.draw(|f| ui::draw(f, &mut app)).unwrap();
        assert!(
            app.splash_showing(),
            "empty default workspace is a first run"
        );
        assert!(
            !buffer_text(&terminal).contains("no projects yet"),
            "the splash replaces the panels"
        );

        app.focus = Focus::Workspaces;
        move_selection(&mut app, 1, &mut out);
        assert_eq!(app.tree.active_workspace_name(), "fresh");
        assert!(!app.splash_showing(), "not a first run");
        assert!(!app.splash_active());
        terminal.draw(|f| ui::draw(f, &mut app)).unwrap();
        let text = buffer_text(&terminal);
        assert!(
            text.contains("WORKSPACES"),
            "column stays on screen:\n{text}"
        );
        assert!(
            text.contains("no projects yet"),
            "panels stay on screen:\n{text}"
        );

        move_selection(&mut app, -1, &mut out);
        assert_eq!(app.tree.active_workspace_name(), "default");
        assert!(
            app.splash_showing(),
            "back on the empty default: splash again"
        );
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

        // The key verbs ride the modal's bottom border.
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
        app.focus = Focus::Sessions;

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
        assert_eq!(
            app.focus,
            Focus::Projects,
            "the new workspace lands focus on the PROJECTS PANEL, not back on Sessions"
        );
    }

    /// `n` in the WORKSPACES BAR creates a workspace too; when the Ack opens
    /// it, focus moves down off the bar onto the PROJECTS PANEL — the
    /// workspace is empty, and `n` there adds its first project.
    #[test]
    fn a_workspace_created_from_the_bar_lands_focus_on_projects() {
        use nebula_core::{EntityId, WorkspaceId};
        let mut app = App::new();
        seed_tree(&mut app);
        seed_default_workspace(&mut app);
        app.focus = Focus::Workspaces;

        let mut out = Vec::new();
        press(&mut app, KeyCode::Char('n'), KeyModifiers::NONE, &mut out);
        assert!(
            matches!(&app.overlay, Some(Overlay::Prompt(p)) if p.kind == PromptKind::NewWorkspace),
            "{:?}",
            app.overlay
        );
        for c in "acme".chars() {
            press(&mut app, KeyCode::Char(c), KeyModifiers::NONE, &mut out);
        }
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE, &mut out);
        let req_id = match out.last() {
            Some(ClientRequest::AddWorkspace { req_id, .. }) => *req_id,
            other => panic!("expected AddWorkspace, got {other:?}"),
        };
        assert_eq!(
            app.focus,
            Focus::Workspaces,
            "still on the bar until the Ack"
        );

        let mut out = Vec::new();
        handle_server_event(
            &mut app,
            ServerEvent::Ack {
                req_id,
                created: Some(EntityId::Workspace(WorkspaceId("ws-new".into()))),
            },
            &mut out,
        );
        assert_eq!(app.tree.active_workspace.as_str(), "ws-new");
        assert!(
            matches!(
                out.last(),
                Some(ClientRequest::OpenWorkspace { id, .. }) if id.as_str() == "ws-new"
            ),
            "created workspace gets opened: {out:?}"
        );
        assert_eq!(app.focus, Focus::Projects);
        assert!(!app.term_locked);
    }

    /// `r` and `d` in the switcher act on the hovered workspace (footer
    /// hints, no submenus); after a delete the
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

        // d: a confirm replaces the switcher; Esc hands the switcher back
        // on the same row with nothing sent.
        let mut out = Vec::new();
        press(&mut app, KeyCode::Char('w'), &mut out);
        press(&mut app, KeyCode::Char('j'), &mut out);
        press(&mut app, KeyCode::Char('d'), &mut out);
        match &app.overlay {
            Some(Overlay::Confirm(c)) => {
                assert_eq!(c.title, "Delete workspace");
                assert!(c.message.contains("'client'"), "{}", c.message);
                assert!(matches!(
                    &c.action,
                    PendingAction::RemoveWorkspace { id, reopen_picker: Some(1) }
                        if id.as_str() == "ws2"
                ));
            }
            other => panic!("d should confirm, got {other:?}"),
        }
        press(&mut app, KeyCode::Esc, &mut out);
        match &app.overlay {
            Some(Overlay::Menu(menu)) => {
                assert!(menu.is_workspace_picker(), "Esc reopens the switcher");
                assert_eq!(menu.hover, 1, "on the row it was on");
            }
            other => panic!("Esc should reopen the switcher, got {other:?}"),
        }
        assert!(
            !out.iter()
                .any(|r| matches!(r, ClientRequest::RemoveWorkspace { .. })),
            "nothing sent on Esc: {out:?}"
        );

        // y: the request goes out (the daemon guards misuse) and the
        // switcher stays up, dropping the row when the removal delta lands.
        press(&mut app, KeyCode::Char('d'), &mut out);
        press(&mut app, KeyCode::Char('y'), &mut out);
        assert!(
            matches!(
                out.last(),
                Some(ClientRequest::RemoveWorkspace { id, .. }) if id.as_str() == "ws2"
            ),
            "delete request sent: {out:?}"
        );
        assert!(
            matches!(&app.overlay, Some(Overlay::Menu(m)) if m.is_workspace_picker() && m.hover == 1),
            "switcher stays open on its row"
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

    /// When the OPEN WORKSPACE is deleted — from here, another instance, or
    /// `nebula workspace delete` — the reseat lands on the WORKSPACE TAB to
    /// its right; from the last tab it falls back to the one on its left.
    /// It never jumps to the first tab, and deleting a workspace that is
    /// not the open one leaves the OPEN WORKSPACE alone.
    #[test]
    fn deleting_the_open_workspace_lands_on_its_right_neighbor_then_its_left() {
        use nebula_core::{Entity, EntityId, Workspace, WorkspaceId};
        // Three tabs, in bar order: default, ws2, ws3.
        let seed = || {
            let mut app = App::new();
            for (id, name) in [("default", "default"), ("ws2", "client"), ("ws3", "lab")] {
                hse(
                    &mut app,
                    ServerEvent::EntityUpserted {
                        entity: Entity::Workspace(Workspace {
                            id: WorkspaceId(id.into()),
                            name: name.into(),
                        }),
                    },
                );
            }
            app
        };
        // Open `open`, delete `gone`, expect to land on `want`.
        let check = |open: &str, gone: &str, want: &str| {
            let mut app = seed();
            let mut out = Vec::new();
            switch_workspace(&mut app, WorkspaceId(open.into()), &mut out);
            assert_eq!(app.tree.active_workspace.as_str(), open);
            let mut out = Vec::new();
            handle_server_event(
                &mut app,
                ServerEvent::EntityRemoved {
                    id: EntityId::Workspace(WorkspaceId(gone.into())),
                },
                &mut out,
            );
            assert_eq!(
                app.tree.active_workspace.as_str(),
                want,
                "open {open}, delete {gone}"
            );
            assert!(
                app.tree.workspaces.iter().all(|w| w.id.as_str() != gone),
                "row dropped"
            );
            out
        };
        // First tab: the one to its right.
        let out = check("default", "default", "ws2");
        assert!(
            matches!(out.last(), Some(ClientRequest::OpenWorkspace { id, .. }) if id.as_str() == "ws2"),
            "{out:?}"
        );
        // Middle tab: the one to its right, not back to the first.
        check("ws2", "ws2", "ws3");
        // Last tab: the one to its left.
        check("ws3", "ws3", "ws2");
        // Not the open one: nothing moves and nothing is sent.
        let out = check("default", "ws3", "default");
        assert!(
            !out.iter()
                .any(|r| matches!(r, ClientRequest::OpenWorkspace { .. })),
            "no switch for a workspace that wasn't open: {out:?}"
        );
    }

    // ---- agent presets ----

    /// Route the preset store at a temp file (and the config at a default
    /// one) and pre-seed two presets: "reviewer" (claude · opus · high,
    /// wrapped) then "scratch" (codex · gpt-5.5, bare).
    fn with_seeded_presets(f: impl FnOnce()) {
        use crate::agent_presets::AgentPreset;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("agent_presets.json");
        with_default_config(|| {
            crate::agent_presets::with_presets_path(path, || {
                crate::agent_presets::save(&[
                    AgentPreset {
                        name: "reviewer".into(),
                        kind: AgentKind::Claude,
                        model: Some("opus".into()),
                        effort: Some("high".into()),
                        prefix: "Be strict.".into(),
                        postfix: "Run the tests.".into(),
                    },
                    AgentPreset {
                        name: "scratch".into(),
                        kind: AgentKind::Codex,
                        model: Some("gpt-5.5".into()),
                        effort: None,
                        prefix: String::new(),
                        postfix: String::new(),
                    },
                ])
                .unwrap();
                f();
            })
        });
    }

    /// A seeded app with FOCUS on the SESSIONS PANEL and the list open.
    fn open_presets(app: &mut App, out: &mut Vec<ClientRequest>) {
        seed_tree(app);
        app.focus = Focus::Sessions;
        press(app, KeyCode::Char('e'), KeyModifiers::NONE, out);
        assert!(
            matches!(&app.overlay, Some(Overlay::AgentPresets(_))),
            "e in Sessions should open the presets list, got {:?}",
            app.overlay
        );
    }

    fn type_text(app: &mut App, text: &str, out: &mut Vec<ClientRequest>) {
        for c in text.chars() {
            press(app, KeyCode::Char(c), KeyModifiers::NONE, out);
        }
    }

    #[test]
    fn e_in_sessions_opens_agent_presets_and_flashes_elsewhere() {
        with_seeded_presets(|| {
            let mut app = App::new();
            let mut out = Vec::new();
            open_presets(&mut app, &mut out);
            let worktree = app.selected_worktree().unwrap().id.clone();
            let Some(Overlay::AgentPresets(view)) = &app.overlay else {
                unreachable!()
            };
            assert_eq!(view.presets.len(), 2);
            assert_eq!(view.presets[0].name, "reviewer", "file order");
            assert_eq!(
                view.worktree, worktree,
                "launches land in the selected worktree"
            );
            assert_eq!(view.selected, 0);

            let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
            terminal.draw(|f| ui::draw(f, &mut app)).unwrap();
            let text = buffer_text(&terminal);
            assert!(text.contains("Agent presets"), "title rendered:\n{text}");
            assert!(text.contains("reviewer"), "rows rendered:\n{text}");
            assert!(
                text.contains("claude · opus · high"),
                "spec rendered:\n{text}"
            );
            assert!(
                text.contains("+prefix +postfix"),
                "wrapping marked:\n{text}"
            );
            assert!(
                text.contains("scratch  codex · gpt-5.5"),
                "bare preset:\n{text}"
            );
            assert!(
                text.contains("Enter: launch  a: new  e: edit  d: delete"),
                "modal hint:\n{text}"
            );

            press(&mut app, KeyCode::Esc, KeyModifiers::NONE, &mut out);
            assert!(app.overlay.is_none(), "Esc closes the list");
            assert!(out.is_empty(), "browsing presets sends nothing: {out:?}");
            // The Sessions hints need a wide footer to show in full.
            let mut wide = Terminal::new(TestBackend::new(180, 30)).unwrap();
            wide.draw(|f| ui::draw(f, &mut app)).unwrap();
            let text = buffer_text(&wide);
            assert!(text.contains("e: presets"), "Sessions footer hint:\n{text}");

            // Anywhere but the Sessions panel, `e` only says where it works.
            app.focus = Focus::Projects;
            press(&mut app, KeyCode::Char('e'), KeyModifiers::NONE, &mut out);
            assert!(app.overlay.is_none());
            assert!(
                app.flash
                    .as_deref()
                    .unwrap_or("")
                    .contains("Sessions panel"),
                "flash: {:?}",
                app.flash
            );
        });
    }

    #[test]
    fn presets_a_fills_the_editor_and_enter_persists() {
        use crate::preset_overlays::PresetField;
        with_seeded_presets(|| {
            let mut app = App::new();
            let mut out = Vec::new();
            open_presets(&mut app, &mut out);
            press(&mut app, KeyCode::Char('a'), KeyModifiers::NONE, &mut out);
            let Some(Overlay::AgentPresetEditor(editor)) = &app.overlay else {
                panic!("a should open the editor, got {:?}", app.overlay);
            };
            assert_eq!(editor.editing, None);
            assert_eq!(editor.field, PresetField::Name);

            // List verbs are just characters while a text field has the
            // caret — `e`, `d`, `a` type rather than act.
            type_text(&mut app, "tested", &mut out);
            press(&mut app, KeyCode::Tab, KeyModifiers::NONE, &mut out);
            press(&mut app, KeyCode::Right, KeyModifiers::NONE, &mut out);
            press(&mut app, KeyCode::Tab, KeyModifiers::NONE, &mut out);
            press(&mut app, KeyCode::Right, KeyModifiers::NONE, &mut out);
            press(&mut app, KeyCode::Tab, KeyModifiers::NONE, &mut out);
            press(&mut app, KeyCode::Right, KeyModifiers::NONE, &mut out);
            press(&mut app, KeyCode::Tab, KeyModifiers::NONE, &mut out);
            assert!(paste_into_overlay(&mut app, "Line one"));
            press(&mut app, KeyCode::Enter, KeyModifiers::SHIFT, &mut out);
            assert!(paste_into_overlay(&mut app, "Line two"));
            press(&mut app, KeyCode::Tab, KeyModifiers::NONE, &mut out);
            type_text(&mut app, "END", &mut out);
            let Some(Overlay::AgentPresetEditor(editor)) = &app.overlay else {
                panic!("editor should stay open, got {:?}", app.overlay);
            };
            assert_eq!(editor.name.as_str(), "tested");
            assert_eq!(editor.kind, AgentKind::Codex);
            assert_eq!(editor.model, "gpt-5.6-sol", "→ steps off the default");
            assert_eq!(editor.effort, "minimal");
            assert_eq!(editor.prefix.as_str(), "Line one\nLine two");
            assert_eq!(editor.field, PresetField::Postfix);

            let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
            terminal.draw(|f| ui::draw(f, &mut app)).unwrap();
            let text = buffer_text(&terminal);
            assert!(text.contains("New preset"), "title:\n{text}");
            assert!(text.contains("Harness  codex"), "kind row:\n{text}");
            assert!(text.contains("Line two"), "prefix box:\n{text}");
            assert!(
                text.contains("◂ END ▸") || text.contains("END"),
                "postfix caret:\n{text}"
            );

            press(&mut app, KeyCode::Enter, KeyModifiers::NONE, &mut out);
            let Some(Overlay::AgentPresets(view)) = &app.overlay else {
                panic!("save should land back in the list, got {:?}", app.overlay);
            };
            assert_eq!(view.presets.len(), 3);
            assert_eq!(view.selected, 2, "cursor on the saved row");
            let saved = &crate::agent_presets::load()[2];
            assert_eq!(saved.name, "tested");
            assert_eq!(saved.kind, AgentKind::Codex);
            assert_eq!(saved.model.as_deref(), Some("gpt-5.6-sol"));
            assert_eq!(saved.effort.as_deref(), Some("minimal"));
            assert_eq!(saved.prefix, "Line one\nLine two");
            assert_eq!(saved.postfix, "END");
            assert!(
                out.is_empty(),
                "editing sends nothing to the daemon: {out:?}"
            );
        });
    }

    #[test]
    fn presets_editor_refuses_blank_and_duplicate_names() {
        with_seeded_presets(|| {
            let mut app = App::new();
            let mut out = Vec::new();
            open_presets(&mut app, &mut out);
            press(&mut app, KeyCode::Char('a'), KeyModifiers::NONE, &mut out);
            press(&mut app, KeyCode::Enter, KeyModifiers::NONE, &mut out);
            assert!(matches!(&app.overlay, Some(Overlay::AgentPresetEditor(_))));
            assert_eq!(app.flash.as_deref(), Some("the preset needs a name"));

            type_text(&mut app, "Reviewer", &mut out);
            press(&mut app, KeyCode::Enter, KeyModifiers::NONE, &mut out);
            assert!(matches!(&app.overlay, Some(Overlay::AgentPresetEditor(_))));
            assert!(
                app.flash
                    .as_deref()
                    .unwrap_or("")
                    .contains("already exists"),
                "case-insensitive duplicate: {:?}",
                app.flash
            );

            press(&mut app, KeyCode::Esc, KeyModifiers::NONE, &mut out);
            let Some(Overlay::AgentPresets(view)) = &app.overlay else {
                panic!("Esc goes back to the list, got {:?}", app.overlay);
            };
            assert_eq!(view.presets.len(), 2, "nothing saved");
            assert_eq!(crate::agent_presets::load().len(), 2);
        });
    }

    #[test]
    fn presets_e_edits_in_place() {
        with_seeded_presets(|| {
            let mut app = App::new();
            let mut out = Vec::new();
            open_presets(&mut app, &mut out);
            press(&mut app, KeyCode::Char('j'), KeyModifiers::NONE, &mut out);
            press(&mut app, KeyCode::Char('e'), KeyModifiers::NONE, &mut out);
            let Some(Overlay::AgentPresetEditor(editor)) = &app.overlay else {
                panic!("e should open the editor on the row, got {:?}", app.overlay);
            };
            assert_eq!(editor.editing, Some(1));
            assert_eq!(editor.name.as_str(), "scratch");
            assert_eq!(editor.kind, AgentKind::Codex);
            assert_eq!(editor.model, "gpt-5.5");
            assert_eq!(editor.effort, crate::config::DEFAULT_CHOICE);

            // Tab to Model; the last choice wraps to the default.
            press(&mut app, KeyCode::Tab, KeyModifiers::NONE, &mut out);
            press(&mut app, KeyCode::Tab, KeyModifiers::NONE, &mut out);
            press(&mut app, KeyCode::Right, KeyModifiers::NONE, &mut out);
            let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
            terminal.draw(|f| ui::draw(f, &mut app)).unwrap();
            let text = buffer_text(&terminal);
            assert!(text.contains("Edit preset — scratch"), "title:\n{text}");
            assert!(text.contains("◂ default ▸"), "focused choice:\n{text}");

            press(&mut app, KeyCode::Enter, KeyModifiers::NONE, &mut out);
            let Some(Overlay::AgentPresets(view)) = &app.overlay else {
                panic!("save should land back in the list, got {:?}", app.overlay);
            };
            assert_eq!(view.presets.len(), 2, "edited in place, not appended");
            assert_eq!(view.selected, 1);
            let saved = crate::agent_presets::load();
            assert_eq!(saved[1].name, "scratch");
            assert_eq!(saved[1].model, None, "default folds to None");
            assert_eq!(saved[0].name, "reviewer", "order kept");
        });
    }

    #[test]
    fn presets_d_confirms_deletes_and_reopens_and_cancel_keeps() {
        with_seeded_presets(|| {
            let mut app = App::new();
            let mut out = Vec::new();
            open_presets(&mut app, &mut out);
            press(&mut app, KeyCode::Char('d'), KeyModifiers::NONE, &mut out);
            match &app.overlay {
                Some(Overlay::Confirm(c)) => {
                    assert!(c.message.contains("'reviewer'"), "{}", c.message);
                    assert!(matches!(
                        c.action,
                        PendingAction::DeleteAgentPreset { index: 0, .. }
                    ));
                }
                other => panic!("d should ask first, got {other:?}"),
            }
            // Backing out puts the list back, untouched.
            press(&mut app, KeyCode::Esc, KeyModifiers::NONE, &mut out);
            match &app.overlay {
                Some(Overlay::AgentPresets(view)) => assert_eq!(view.presets.len(), 2),
                other => panic!("cancel should reopen the list, got {other:?}"),
            }
            press(&mut app, KeyCode::Char('d'), KeyModifiers::NONE, &mut out);
            press(&mut app, KeyCode::Char('y'), KeyModifiers::NONE, &mut out);
            match &app.overlay {
                Some(Overlay::AgentPresets(view)) => {
                    assert_eq!(view.presets.len(), 1, "row dropped");
                    assert_eq!(view.presets[0].name, "scratch");
                    assert_eq!(view.selected, 0, "cursor clamped");
                }
                other => panic!("delete should reopen the list, got {other:?}"),
            }
            assert_eq!(app.flash.as_deref(), Some("deleted preset 'reviewer'"));
            let left = crate::agent_presets::load();
            assert_eq!(left.len(), 1, "removal reached the store");
            assert_eq!(left[0].name, "scratch");
            assert!(out.is_empty(), "{out:?}");
        });
    }

    #[test]
    fn preset_enter_asks_for_a_task_and_launches_with_the_composed_prompt() {
        with_seeded_presets(|| {
            let mut app = App::new();
            let mut out = Vec::new();
            open_presets(&mut app, &mut out);
            let worktree = app.selected_worktree().unwrap().id.clone();
            press(&mut app, KeyCode::Enter, KeyModifiers::NONE, &mut out);
            let Some(Overlay::Prompt(prompt)) = &app.overlay else {
                panic!("Enter should ask for the task, got {:?}", app.overlay);
            };
            assert_eq!(prompt.title, "Task for reviewer");
            assert!(
                prompt.label.contains("claude · opus · high"),
                "{}",
                prompt.label
            );
            assert!(
                prompt.label.contains("prefix + your task + postfix"),
                "{}",
                prompt.label
            );
            assert!(prompt.is_multiline());
            assert!(out.is_empty(), "no prewarm for a preset launch: {out:?}");

            assert!(paste_into_overlay(&mut app, "Fix auth"));
            press(&mut app, KeyCode::Enter, KeyModifiers::SHIFT, &mut out);
            assert!(paste_into_overlay(&mut app, "Ship it"));
            press(&mut app, KeyCode::Enter, KeyModifiers::NONE, &mut out);
            assert!(app.overlay.is_none(), "launch closes the prompt");
            assert!(
                matches!(
                    out.as_slice(),
                    [ClientRequest::CreateAgent {
                        worktree: w,
                        kind: AgentKind::Claude,
                        model: Some(model),
                        effort: Some(effort),
                        auto_title: true,
                        cloud_prompt: None,
                        starting_prompt: Some(text),
                        ..
                    }] if *w == worktree
                        && model == "opus"
                        && effort == "high"
                        && text == "Be strict.\n\nFix auth\nShip it\n\nRun the tests."
                ),
                "one create with the sandwiched prompt, no warm-slot refill: {out:?}"
            );

            // The daemon refusing it brings the task back to be fixed.
            let req_id = match &out[0] {
                ClientRequest::CreateAgent { req_id, .. } => *req_id,
                other => panic!("expected create request, got {other:?}"),
            };
            handle_server_event(
                &mut app,
                ServerEvent::Error {
                    req_id: Some(req_id),
                    message: "claude is not installed".into(),
                },
                &mut out,
            );
            assert_eq!(app.flash.as_deref(), Some("claude is not installed"));
            assert!(matches!(
                &app.overlay,
                Some(Overlay::Prompt(prompt))
                    if matches!(&prompt.kind, PromptKind::AgentPresetTask { preset, .. } if preset.name == "reviewer")
                        && prompt.input.as_str() == "Fix auth\nShip it"
            ));
        });
    }

    #[test]
    fn preset_task_cannot_be_empty_and_esc_returns_to_the_list() {
        with_seeded_presets(|| {
            let mut app = App::new();
            let mut out = Vec::new();
            open_presets(&mut app, &mut out);
            press(&mut app, KeyCode::Char('j'), KeyModifiers::NONE, &mut out);
            press(&mut app, KeyCode::Enter, KeyModifiers::NONE, &mut out);
            let Some(Overlay::Prompt(prompt)) = &app.overlay else {
                panic!("Enter should ask for the task, got {:?}", app.overlay);
            };
            assert_eq!(prompt.title, "Task for scratch");
            assert!(
                prompt.label.contains("sent as the first prompt"),
                "{}",
                prompt.label
            );

            press(&mut app, KeyCode::Enter, KeyModifiers::NONE, &mut out);
            assert!(
                matches!(&app.overlay, Some(Overlay::Prompt(_))),
                "empty task stays open"
            );
            assert_eq!(app.flash.as_deref(), Some("the preset needs a task"));
            assert!(out.is_empty(), "{out:?}");

            press(&mut app, KeyCode::Esc, KeyModifiers::NONE, &mut out);
            match &app.overlay {
                Some(Overlay::AgentPresets(view)) => assert_eq!(view.selected, 1, "same row"),
                other => panic!("Esc goes back to the list, got {other:?}"),
            }
        });
    }

    #[test]
    fn cursor_preset_effort_follows_the_model_family() {
        use crate::preset_overlays::PresetField;
        with_seeded_presets(|| {
            let mut app = App::new();
            let mut out = Vec::new();
            open_presets(&mut app, &mut out);
            press(&mut app, KeyCode::Char('a'), KeyModifiers::NONE, &mut out);
            type_text(&mut app, "cur", &mut out);
            press(&mut app, KeyCode::Tab, KeyModifiers::NONE, &mut out);
            press(&mut app, KeyCode::Right, KeyModifiers::NONE, &mut out);
            press(&mut app, KeyCode::Right, KeyModifiers::NONE, &mut out);
            press(&mut app, KeyCode::Tab, KeyModifiers::NONE, &mut out);
            let Some(Overlay::AgentPresetEditor(editor)) = &app.overlay else {
                panic!("editor should stay open, got {:?}", app.overlay);
            };
            assert_eq!(editor.kind, AgentKind::Cursor);
            assert_eq!(editor.field, PresetField::Model, "cursor has a model row");
            // With the model still default there is no suffix to pick:
            // Effort reads n/a and Tab skips it.
            let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
            terminal.draw(|f| ui::draw(f, &mut app)).unwrap();
            let text = buffer_text(&terminal);
            assert!(text.contains("Effort  n/a"), "no family yet:\n{text}");
            press(&mut app, KeyCode::Tab, KeyModifiers::NONE, &mut out);
            let Some(Overlay::AgentPresetEditor(editor)) = &app.overlay else {
                unreachable!()
            };
            assert_eq!(
                editor.field,
                PresetField::Prefix,
                "Tab skips the n/a effort"
            );
            press(&mut app, KeyCode::BackTab, KeyModifiers::SHIFT, &mut out);

            // → → : default → auto → claude-fable-5, whose efforts exist.
            press(&mut app, KeyCode::Right, KeyModifiers::NONE, &mut out);
            press(&mut app, KeyCode::Right, KeyModifiers::NONE, &mut out);
            press(&mut app, KeyCode::Tab, KeyModifiers::NONE, &mut out);
            let Some(Overlay::AgentPresetEditor(editor)) = &app.overlay else {
                unreachable!()
            };
            assert_eq!(editor.model, "claude-fable-5");
            assert_eq!(
                editor.field,
                PresetField::Effort,
                "a family with efforts gets the row"
            );
            // No bare `claude-fable-5` id exists, so the effort lands on the
            // family's fallback at once; → → walks up to max.
            assert_eq!(editor.effort, "high");
            press(&mut app, KeyCode::Right, KeyModifiers::NONE, &mut out);
            press(&mut app, KeyCode::Right, KeyModifiers::NONE, &mut out);
            let Some(Overlay::AgentPresetEditor(editor)) = &app.overlay else {
                unreachable!()
            };
            assert_eq!(editor.effort, "max");
            // Stepping the model past fable-5-thinking (has max) onto Opus 5
            // (no max variant) drops it back to that family's fallback.
            press(&mut app, KeyCode::BackTab, KeyModifiers::SHIFT, &mut out);
            press(&mut app, KeyCode::Right, KeyModifiers::NONE, &mut out);
            press(&mut app, KeyCode::Right, KeyModifiers::NONE, &mut out);
            let Some(Overlay::AgentPresetEditor(editor)) = &app.overlay else {
                unreachable!()
            };
            assert_eq!(editor.model, "claude-opus-5");
            assert_eq!(editor.effort, "high", "max dropped with the family");
            press(&mut app, KeyCode::Tab, KeyModifiers::NONE, &mut out);
            press(&mut app, KeyCode::Right, KeyModifiers::NONE, &mut out);
            let Some(Overlay::AgentPresetEditor(editor)) = &app.overlay else {
                unreachable!()
            };
            assert_eq!(editor.effort, "high-fast", "-fast is the next effort row");
            press(&mut app, KeyCode::Left, KeyModifiers::NONE, &mut out);

            press(&mut app, KeyCode::Enter, KeyModifiers::NONE, &mut out);
            let saved = crate::agent_presets::load();
            assert_eq!(saved[2].kind, AgentKind::Cursor);
            assert_eq!(saved[2].model.as_deref(), Some("claude-opus-5"));
            assert_eq!(saved[2].effort.as_deref(), Some("high"));
            terminal.draw(|f| ui::draw(f, &mut app)).unwrap();
            let text = buffer_text(&terminal);
            assert!(
                text.contains("cur  cursor · claude-opus-5 · high"),
                "spec label names family and effort:\n{text}"
            );
        });
    }

    #[test]
    fn picker_model_submenu_types_to_filter() {
        with_default_config(|| {
            let mut app = App::new();
            seed_tree(&mut app);
            app.focus = Focus::Sessions;
            let mut out = Vec::new();
            press(&mut app, KeyCode::Char('n'), KeyModifiers::NONE, &mut out);
            press(&mut app, KeyCode::Down, KeyModifiers::NONE, &mut out);
            press(&mut app, KeyCode::Down, KeyModifiers::NONE, &mut out);
            press(&mut app, KeyCode::Right, KeyModifiers::NONE, &mut out);
            let Some(Overlay::Menu(menu)) = &app.overlay else {
                panic!("expected cursor model submenu, got {:?}", app.overlay);
            };
            let all = menu.items.len();
            assert!(menu.filter.is_some(), "model submenus take type-ahead");

            // The title shows the affordance before anything is typed.
            let mut terminal = Terminal::new(TestBackend::new(120, 40)).unwrap();
            terminal.draw(|f| ui::draw(f, &mut app)).unwrap();
            let text = buffer_text(&terminal);
            assert!(
                text.contains("Cursor model ⌕"),
                "bare ⌕ in the title:\n{text}"
            );
            assert!(
                text.contains("type to filter"),
                "hint on the border:\n{text}"
            );

            // "opus" narrows to the two Opus families, best first, hover
            // on the first; the letters did not move the hover as j/k.
            type_text(&mut app, "opus", &mut out);
            let Some(Overlay::Menu(menu)) = &app.overlay else {
                unreachable!()
            };
            assert_eq!(
                menu.items
                    .iter()
                    .map(|i| i.label.as_str())
                    .collect::<Vec<_>>(),
                vec!["claude-opus-5", "claude-opus-5-thinking"]
            );
            assert_eq!(menu.hover, 0);
            assert_eq!(menu.filter_query(), "opus");
            terminal.draw(|f| ui::draw(f, &mut app)).unwrap();
            let text = buffer_text(&terminal);
            assert!(
                text.contains("Cursor model ⌕ opus"),
                "query in the title:\n{text}"
            );

            // A letter no row matches is refused and flashed.
            type_text(&mut app, "z", &mut out);
            let Some(Overlay::Menu(menu)) = &app.overlay else {
                unreachable!()
            };
            assert_eq!(menu.filter_query(), "opus");
            assert_eq!(menu.items.len(), 2);
            assert!(app.flash.as_deref().is_some_and(|f| f.contains("opusz")));

            // ↓ moves within the matches; → drills the hovered family into
            // its efforts, which start with an empty filter of their own.
            press(&mut app, KeyCode::Down, KeyModifiers::NONE, &mut out);
            press(&mut app, KeyCode::Right, KeyModifiers::NONE, &mut out);
            let Some(Overlay::Menu(menu)) = &app.overlay else {
                unreachable!()
            };
            assert_eq!(menu.title.as_deref(), Some("Cursor effort"));
            assert_eq!(menu.filter_query(), "");
            type_text(&mut app, "hf", &mut out);
            let Some(Overlay::Menu(menu)) = &app.overlay else {
                unreachable!()
            };
            assert!(
                menu.items.iter().all(|i| i.label.ends_with("-fast")),
                "hf keeps only fast rows: {:?}",
                menu.items.iter().map(|i| &i.label).collect::<Vec<_>>()
            );
            assert_eq!(menu.items[0].label, "high-fast");
            assert!(matches!(
                &menu.items[0].action,
                MenuAction::NewAgentOfKind { kind: AgentKind::Cursor, model: Some(m), effort: Some(e), .. }
                    if m == "claude-opus-5-thinking" && e == "high-fast"
            ));

            // Backspace widens; Esc clears the text first, then backs out
            // to the (still filtered) model list.
            press(&mut app, KeyCode::Backspace, KeyModifiers::NONE, &mut out);
            let Some(Overlay::Menu(menu)) = &app.overlay else {
                unreachable!()
            };
            assert_eq!(menu.filter_query(), "h");
            press(&mut app, KeyCode::Esc, KeyModifiers::NONE, &mut out);
            let Some(Overlay::Menu(menu)) = &app.overlay else {
                unreachable!()
            };
            assert_eq!(menu.title.as_deref(), Some("Cursor effort"));
            assert_eq!(menu.filter_query(), "");
            press(&mut app, KeyCode::Esc, KeyModifiers::NONE, &mut out);
            let Some(Overlay::Menu(menu)) = &app.overlay else {
                unreachable!()
            };
            assert_eq!(menu.title.as_deref(), Some("Cursor model"));
            assert_eq!(menu.filter_query(), "opus");
            assert_eq!(menu.hover, 1, "← restores the parent's hover");
            // Clearing the filter brings every row back, hovering the ✓.
            press(&mut app, KeyCode::Esc, KeyModifiers::NONE, &mut out);
            let Some(Overlay::Menu(menu)) = &app.overlay else {
                unreachable!()
            };
            assert_eq!(menu.items.len(), all);
            assert_eq!(menu.items[menu.hover].label, "default ✓");
        });
    }

    #[test]
    fn preset_editor_types_to_filter_choice_rows() {
        use crate::preset_overlays::PresetField;
        with_seeded_presets(|| {
            let mut app = App::new();
            let mut out = Vec::new();
            open_presets(&mut app, &mut out);
            press(&mut app, KeyCode::Char('a'), KeyModifiers::NONE, &mut out);
            type_text(&mut app, "typed", &mut out);
            press(&mut app, KeyCode::Tab, KeyModifiers::NONE, &mut out);
            // Harness: "cu" jumps to cursor; h/l no longer cycle.
            type_text(&mut app, "cu", &mut out);
            let Some(Overlay::AgentPresetEditor(editor)) = &app.overlay else {
                panic!("editor should stay open, got {:?}", app.overlay);
            };
            assert_eq!(editor.field, PresetField::Kind);
            assert_eq!(editor.kind, AgentKind::Cursor);
            assert_eq!(editor.filter, "cu");
            // Tab drops the filter and lands on Model.
            press(&mut app, KeyCode::Tab, KeyModifiers::NONE, &mut out);
            let Some(Overlay::AgentPresetEditor(editor)) = &app.overlay else {
                unreachable!()
            };
            assert_eq!(editor.field, PresetField::Model);
            assert_eq!(editor.filter, "");
            // "sol" → gpt-5.6-sol at once, effort on its fallback; → cycles
            // only the matches (one), so it stays.
            type_text(&mut app, "sol", &mut out);
            let Some(Overlay::AgentPresetEditor(editor)) = &app.overlay else {
                unreachable!()
            };
            assert_eq!(editor.model, "gpt-5.6-sol");
            assert_eq!(editor.effort, "high");
            assert_eq!(editor.filtered_choices(), vec!["gpt-5.6-sol"]);
            press(&mut app, KeyCode::Right, KeyModifiers::NONE, &mut out);
            let Some(Overlay::AgentPresetEditor(editor)) = &app.overlay else {
                unreachable!()
            };
            assert_eq!(editor.model, "gpt-5.6-sol");
            let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
            terminal.draw(|f| ui::draw(f, &mut app)).unwrap();
            let text = buffer_text(&terminal);
            assert!(
                text.contains("gpt-5.6-sol ▸  ⌕ sol (1)"),
                "filter beside the value:\n{text}"
            );

            // A refused letter leaves everything as it was and flashes.
            type_text(&mut app, "q", &mut out);
            let Some(Overlay::AgentPresetEditor(editor)) = &app.overlay else {
                unreachable!()
            };
            assert_eq!(editor.filter, "sol");
            assert!(app.flash.as_deref().is_some_and(|f| f.contains("solq")));
            // Backspace to "so": sol still matches, so the value stays;
            // Esc clears the filter but keeps the value; a second Esc
            // backs out to the list as before.
            press(&mut app, KeyCode::Backspace, KeyModifiers::NONE, &mut out);
            let Some(Overlay::AgentPresetEditor(editor)) = &app.overlay else {
                unreachable!()
            };
            assert_eq!(editor.filter, "so");
            assert_eq!(editor.model, "gpt-5.6-sol");
            press(&mut app, KeyCode::Esc, KeyModifiers::NONE, &mut out);
            let Some(Overlay::AgentPresetEditor(editor)) = &app.overlay else {
                panic!("first Esc only clears the filter, got {:?}", app.overlay);
            };
            assert_eq!(editor.filter, "");
            assert_eq!(editor.model, "gpt-5.6-sol");
            // On the Effort row, "xf" lands on xhigh-fast.
            press(&mut app, KeyCode::Tab, KeyModifiers::NONE, &mut out);
            type_text(&mut app, "xf", &mut out);
            let Some(Overlay::AgentPresetEditor(editor)) = &app.overlay else {
                unreachable!()
            };
            assert_eq!(editor.field, PresetField::Effort);
            assert_eq!(editor.effort, "xhigh-fast");
            press(&mut app, KeyCode::Esc, KeyModifiers::NONE, &mut out);
            press(&mut app, KeyCode::Esc, KeyModifiers::NONE, &mut out);
            assert!(
                matches!(app.overlay, Some(Overlay::AgentPresets(_))),
                "second Esc backs out: {:?}",
                app.overlay
            );
        });
    }

    #[test]
    fn picker_cursor_drills_into_family_then_effort_and_auto_is_a_leaf() {
        with_default_config(|| {
            let mut app = App::new();
            seed_tree(&mut app);
            app.focus = Focus::Sessions;
            let mut out = Vec::new();

            press(&mut app, KeyCode::Char('n'), KeyModifiers::NONE, &mut out);
            press(&mut app, KeyCode::Down, KeyModifiers::NONE, &mut out);
            press(&mut app, KeyCode::Down, KeyModifiers::NONE, &mut out);
            press(&mut app, KeyCode::Right, KeyModifiers::NONE, &mut out);
            let Some(Overlay::Menu(menu)) = &app.overlay else {
                panic!("expected cursor model submenu, got {:?}", app.overlay);
            };
            assert_eq!(menu.title.as_deref(), Some("Cursor model"));
            assert_eq!(
                menu.items.len(),
                crate::config::model_choices(AgentKind::Cursor).len()
            );
            assert_eq!(menu.items[0].label, "default ✓");
            assert_eq!(menu.items[1].label, "auto");
            // `auto` has no effort variants: a leaf. Opus 5 thinking drills.
            assert_eq!(menu.items[1].action.submenu(), None);
            assert_eq!(menu.items[5].label, "claude-opus-5-thinking");
            assert_eq!(menu.items[5].action.submenu(), Some(SubmenuKind::Efforts));

            for _ in 0..5 {
                press(&mut app, KeyCode::Down, KeyModifiers::NONE, &mut out);
            }
            press(&mut app, KeyCode::Right, KeyModifiers::NONE, &mut out);
            let Some(Overlay::Menu(menu)) = &app.overlay else {
                panic!("expected cursor effort submenu, got {:?}", app.overlay);
            };
            assert_eq!(menu.title.as_deref(), Some("Cursor effort"));
            // No bare `claude-opus-5-thinking` id, so no default row; the
            // -fast twins follow their base.
            assert_eq!(
                menu.items
                    .iter()
                    .map(|i| i.label.as_str())
                    .collect::<Vec<_>>(),
                vec![
                    "low",
                    "low-fast",
                    "medium",
                    "medium-fast",
                    "high",
                    "high-fast",
                    "xhigh",
                    "xhigh-fast",
                    "max",
                    "max-fast"
                ]
            );
            assert!(matches!(
                &menu.items[9].action,
                MenuAction::NewAgentOfKind { kind: AgentKind::Cursor, model: Some(m), effort: Some(e), .. }
                    if m == "claude-opus-5-thinking" && e == "max-fast"
            ));
            assert_eq!(menu.items[9].action.submenu(), None);
        });
    }

    #[test]
    fn presets_click_row_launches_and_outside_closes() {
        with_seeded_presets(|| {
            let mut app = App::new();
            let mut out = Vec::new();
            open_presets(&mut app, &mut out);
            let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
            terminal.draw(|f| ui::draw(f, &mut app)).unwrap();
            let list = match &app.overlay {
                Some(Overlay::AgentPresets(view)) => view.list_area,
                other => panic!("{other:?}"),
            };
            assert!(list.width > 0, "drawn list rect recorded");
            click(&mut app, list.x + 1, list.y + 1, &mut out);
            match &app.overlay {
                Some(Overlay::Prompt(prompt)) => assert_eq!(prompt.title, "Task for scratch"),
                other => panic!("a click on the second row launches it, got {other:?}"),
            }
            press(&mut app, KeyCode::Esc, KeyModifiers::NONE, &mut out);
            terminal.draw(|f| ui::draw(f, &mut app)).unwrap();
            assert!(matches!(&app.overlay, Some(Overlay::AgentPresets(_))));
            click(&mut app, 0, 0, &mut out);
            assert!(app.overlay.is_none(), "a click outside closes the list");
            assert!(out.is_empty(), "{out:?}");
        });
    }

    #[test]
    fn empty_presets_list_shows_the_hint() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("agent_presets.json");
        with_default_config(|| {
            crate::agent_presets::with_presets_path(path, || {
                let mut app = App::new();
                let mut out = Vec::new();
                open_presets(&mut app, &mut out);
                let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
                terminal.draw(|f| ui::draw(f, &mut app)).unwrap();
                let text = buffer_text(&terminal);
                assert!(text.contains("no presets yet — a creates one"), "{text}");
                // Enter and e on nothing only nudge toward `a`.
                press(&mut app, KeyCode::Enter, KeyModifiers::NONE, &mut out);
                assert!(matches!(&app.overlay, Some(Overlay::AgentPresets(_))));
                assert_eq!(
                    app.flash.as_deref(),
                    Some("no preset selected — a creates one")
                );
                press(&mut app, KeyCode::Char('e'), KeyModifiers::NONE, &mut out);
                assert!(matches!(&app.overlay, Some(Overlay::AgentPresets(_))));
            })
        });
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
    fn shift_h_opens_hosts_picker_newest_first() {
        with_seeded_hosts(|| {
            let mut app = App::new();
            let mut out = Vec::new();
            press(&mut app, KeyCode::Char('H'), KeyModifiers::SHIFT, &mut out);
            let Some(Overlay::Hosts(view)) = &app.overlay else {
                panic!(
                    "shift+h should open the hosts picker, got {:?}",
                    app.overlay
                );
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
            press(&mut app, KeyCode::Char('H'), KeyModifiers::SHIFT, &mut out);
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
            press(&mut app, KeyCode::Char('H'), KeyModifiers::SHIFT, &mut out);
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
            press(&mut app, KeyCode::Char('H'), KeyModifiers::SHIFT, &mut out);
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
            press(&mut app, KeyCode::Char('H'), KeyModifiers::SHIFT, &mut out);
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

    // ---- a click outside any modal dismisses it ----

    /// Draw once so the modal writes back its hit-test rect, and return it.
    fn drawn_modal_area(app: &mut App) -> ratatui::layout::Rect {
        let mut terminal = Terminal::new(TestBackend::new(140, 40)).unwrap();
        terminal.draw(|f| ui::draw(f, app)).unwrap();
        let area = match &app.overlay {
            Some(Overlay::Help(v)) => v.area,
            Some(Overlay::Confirm(c)) => c.area,
            Some(Overlay::Prompt(p)) => p.area,
            Some(Overlay::Diff(v)) => v.area,
            other => panic!("no boxed modal open: {other:?}"),
        };
        assert!(area.width > 0 && area.x > 0 && area.y > 0, "{area:?}");
        area
    }

    fn click(app: &mut App, column: u16, row: u16, out: &mut Vec<ClientRequest>) {
        handle_mouse(
            app,
            mev(MouseEventKind::Down(MouseButton::Left), column, row),
            out,
        );
    }

    #[test]
    fn help_click_outside_closes_and_inside_stays() {
        let mut app = App::new();
        let mut out = Vec::new();
        press(&mut app, KeyCode::Char('?'), KeyModifiers::NONE, &mut out);
        let area = drawn_modal_area(&mut app);
        click(&mut app, area.x + 2, area.y + 2, &mut out);
        assert!(
            matches!(app.overlay, Some(Overlay::Help(_))),
            "inside click keeps it"
        );
        click(&mut app, 0, 0, &mut out);
        assert!(app.overlay.is_none(), "outside click closes");
        assert!(app.dirty);
    }

    #[test]
    fn confirm_click_outside_cancels_without_confirming() {
        let mut app = App::new();
        seed_tree(&mut app);
        seed_link(&mut app, "https://example.dev/spec");
        let mut out = Vec::new();
        press(&mut app, KeyCode::Char('D'), KeyModifiers::NONE, &mut out);
        assert!(
            matches!(app.overlay, Some(Overlay::Confirm(_))),
            "{:?}",
            app.overlay
        );
        let area = drawn_modal_area(&mut app);
        out.clear();
        click(&mut app, area.x + 2, area.y + 1, &mut out);
        assert!(
            matches!(app.overlay, Some(Overlay::Confirm(_))),
            "inside click keeps it"
        );
        click(&mut app, 0, 0, &mut out);
        assert!(app.overlay.is_none(), "outside click cancels");
        assert!(out.is_empty(), "nothing was confirmed: {out:?}");
    }

    /// The outside click is Esc, not a bare close: backing out of the
    /// settings-reset confirm lands back in the settings overlay.
    #[test]
    fn confirm_click_outside_lands_where_esc_would() {
        with_default_config(|| {
            let mut app = App::new();
            let mut out = Vec::new();
            press(&mut app, KeyCode::Char('s'), KeyModifiers::NONE, &mut out);
            press(&mut app, KeyCode::Tab, KeyModifiers::NONE, &mut out);
            press(&mut app, KeyCode::Char('R'), KeyModifiers::SHIFT, &mut out);
            assert!(
                matches!(app.overlay, Some(Overlay::Confirm(_))),
                "{:?}",
                app.overlay
            );
            drawn_modal_area(&mut app);
            click(&mut app, 0, 0, &mut out);
            assert!(
                matches!(app.overlay, Some(Overlay::Settings(_))),
                "back to the overlay, not the panels: {:?}",
                app.overlay
            );
        });
    }

    #[test]
    fn prompt_click_outside_abandons_it() {
        let mut app = App::new();
        let mut out = Vec::new();
        press(&mut app, KeyCode::Char('n'), KeyModifiers::NONE, &mut out);
        assert!(
            matches!(app.overlay, Some(Overlay::Prompt(_))),
            "{:?}",
            app.overlay
        );
        let area = drawn_modal_area(&mut app);
        click(&mut app, area.x + 2, area.y + 1, &mut out);
        assert!(
            matches!(app.overlay, Some(Overlay::Prompt(_))),
            "inside click keeps it"
        );
        click(&mut app, 0, 0, &mut out);
        assert!(app.overlay.is_none(), "outside click abandons the prompt");
    }

    #[test]
    fn diff_click_outside_closes_and_inside_stays() {
        let mut app = App::new();
        seed_tree(&mut app);
        app.overlay = Some(Overlay::Diff(fake_diff_view(10)));
        let mut out = Vec::new();
        let area = drawn_modal_area(&mut app);
        click(
            &mut app,
            area.x + area.width / 2,
            area.y + area.height / 2,
            &mut out,
        );
        assert!(
            matches!(app.overlay, Some(Overlay::Diff(_))),
            "inside click keeps it"
        );
        click(&mut app, 0, 0, &mut out);
        assert!(app.overlay.is_none(), "outside click closes");
    }

    #[test]
    fn hosts_a_types_a_new_destination_and_enter_connects() {
        with_seeded_hosts(|| {
            let mut app = App::new();
            let mut out = Vec::new();
            press(&mut app, KeyCode::Char('H'), KeyModifiers::SHIFT, &mut out);
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
            press(&mut app, KeyCode::Char('H'), KeyModifiers::SHIFT, &mut out);
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
            press(&mut app, KeyCode::Char('H'), KeyModifiers::SHIFT, &mut out);
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
