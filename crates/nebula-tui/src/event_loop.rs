//! The main TUI loop: terminal setup/teardown, message routing, update logic.

use crate::agent_picker::{self, kind_label, KindPicker};
use crate::app::{
    clamp_selection, AgentLaunchDraft, App, AttachedTerm, ConfirmDialog, ConnState, ContextMenu,
    DiffView, FileFinder, Focus, GrepView, HelpView, HitTarget, LinkRow, MenuAction, MenuFilter,
    MenuItem, MetricsView, Overlay, Palette, PaletteTarget, PendingAction, PendingIntent,
    PointerShape, PromptDialog, PromptKind, SessionRow, SettingsView, SubmenuKind, TermSelection,
    WorktreeRollback,
};
use crate::app::{PrCommentAnswer, PrDiffAnswer};
use crate::pull_request::Lookup;
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
    WorktreeId, MAX_CLOUD_PROMPT_BYTES,
};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;
use std::io::{BufWriter, Stdout};
use std::time::Duration;

mod actions;
mod activate;
mod alerts;
mod background;
mod commands;
mod focus_walk;
mod host_terminal;
mod key_modes;
mod launcher;
mod loop_run;
mod mouse_modes;
mod optimistic;
mod overlays;
mod pacing;
mod placeholder;
mod prompts;
mod quick_launch;
mod release_watch;
mod selection;
mod server_events;
mod server_state;
mod settings_handlers;
mod state_io;
mod terminal_events;
mod terminal_mouse;
mod text_entry;
mod worktree_views;
pub(crate) use actions::*;
pub(crate) use background::*;
pub(crate) use commands::*;
use focus_walk::{
    double_tapped, enter_terminal_pane, land_click_focus, walk_focus_back, walk_focus_forward,
};
pub use host_terminal::restore_terminal;
use host_terminal::{
    on_host_resize, reassert_modes, repaint, report_working_directory, setup_terminal,
    take_worker_panic, watch_held_key, MODE_REASSERT,
};
use key_modes::handle_key;
pub(crate) use loop_run::*;
use mouse_modes::handle_mouse;
pub(crate) use overlays::handle_overlay_key;
pub(crate) use prompts::*;
pub use release_watch::ReleaseWatch;
pub(crate) use selection::*;
pub(crate) use server_events::handle_server_event;
pub(crate) use server_state::*;
pub(crate) use settings_handlers::*;
pub(crate) use state_io::*;
pub(crate) use terminal_events::*;
pub(crate) use terminal_mouse::*;
pub(crate) use text_entry::*;
pub(crate) use worktree_views::*;

/// Wheel step for the pull-request reading pane, in lines. Prose wants a
/// bigger bite than a session list of two-row pills.
const PR_PREVIEW_WHEEL_STEP: u16 = 3;

/// Wheel step inside the diff and tree modals' reading panes, in lines.
const MODAL_WHEEL_LINES: i32 = 3;

/// Wheel step over the terminal pane, in lines: how far the scrollback
/// offset moves, and how many arrow keys an alt-screen app that ignores
/// the mouse is sent per notch.
const TERM_WHEEL_LINES: usize = 1;

/// The EDGE AUTO-SCROLL beat: a drag-selection whose pointer rests past
/// the pane's top or bottom edge scrolls the history under it this often.
const DRAG_AUTOSCROLL_TICK: Duration = Duration::from_millis(50);

/// Lines per EDGE AUTO-SCROLL tick, at most. One per row the pointer is
/// past the edge — the rule line above the pane is a gentle crawl, the
/// footer below it a faster one — so the pointer steers the speed rather
/// than waiting on one fixed crawl for a long copy.
const DRAG_AUTOSCROLL_MAX_LINES: usize = 6;

/// Ceiling on a panel or file-list width read back from the persisted UI
/// state — a coarse sanity clamp; the draw re-fits it to the real screen.
const MAX_RESTORED_WIDTH: u16 = 300;

/// Smallest grid worth sizing a PTY or vt100 parser to: below this a pane
/// hasn't really been drawn yet.
const MIN_PANE_DIM: u16 = 2;

/// Terminal-pane grid assumed before the first draw, so a spawn or attach
/// requested that early never boots a 0×0 PTY.
const FALLBACK_PANE: (u16, u16) = (80, 24);

/// Where a menu hangs when there is no drawn word to hang it under: near
/// the top left, where the selected row lives.
pub(super) const KEYBOARD_MENU_ANCHOR: (u16, u16) = (30, 4);

/// Bracketed-paste markers around a pasted block, so the child (claude,
/// vim…) takes it as one paste rather than typing to auto-indent.
const PASTE_START: &[u8] = b"\x1b[200~";
const PASTE_END: &[u8] = b"\x1b[201~";

/// Flash for an action that needs a checkout to act on and has none.
const SELECT_CONTEXT_FIRST: &str = "select a project or worktree first";

/// Flash for a session pick that lost a race with its removal.
const SESSION_GONE: &str = "session no longer exists";
/// Flash for a `/` pull-request pick whose row a refresh retired meanwhile.
const PR_GONE: &str = "pull request is no longer open";
/// `.` / `,` with no session anywhere to land on.
const NO_SESSIONS_TO_JUMP: &str = "no sessions to jump to";

/// Flash for an action an archived agent refuses until it's unarchived.
pub(crate) const AGENT_ARCHIVED: &str = "agent is archived — unarchive first (u)";

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
pub(crate) const PREWARM_DEBOUNCE: Duration = Duration::from_millis(250);
/// How long a selection-driven attach to a *reaped* session waits for the
/// cursor to settle: attaching one makes the daemon fork an agent CLI, and a
/// cursor merely passing through a row — walking a list, or the project
/// tabs, where every step is a whole project switch — has not asked for
/// that. Long enough that a walk boots only where it stops; short enough to
/// feel immediate when it does. A session the daemon still holds never
/// waits: attaching it only replays its ring, so every path (`attach_inner`)
/// attaches it on the keypress.
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
/// process and a network round trip, so the selected worktree is the only
/// one asked on every tick; the rest of the project's checkouts take turns
/// (`sweep_pull_request`), one per tick, on the same backoff.
const PR_RECHECK_MIN: Duration = Duration::from_secs(10);
const PR_RECHECK_MAX: Duration = Duration::from_secs(3 * 60);
/// How often the selected worktree's *known* pull request is re-asked. The
/// PR won't change, but its conversation and state will — this is the beat
/// the row's unread-comment badge and its `merged` / `closed` badge run at,
/// for the one checkout the cursor is resting on. Same cadence and the same
/// budget reasoning as `OPEN_PRS_REFRESH` below.
const PR_REFRESH: Duration = Duration::from_secs(15);
/// The same beat for every *other* checkout of the selected project: how
/// often a known pull request on a worktree the cursor is not resting on
/// is re-asked. Slow, because it multiplies by the number of checkouts —
/// thirty worktrees with pull requests is thirty calls a sweep, so at five
/// minutes that is 360 an hour — and because nothing on those rows needs
/// to be fresher: what they show is whether the branch has *merged*, a
/// change the project's open list catches within `OPEN_PRS_REFRESH`
/// anyway (`note_open_prs_answer` pulls a checkout's lookup forward the
/// moment its pull request leaves that list). The sweep is what lets a
/// checkout turn purple without ever being focused, and what has every
/// PR ROW badged when the cursor does arrive.
const PR_SWEEP_REFRESH: Duration = Duration::from_secs(5 * 60);

/// How often the selected *project's* open-pull-request list is re-asked
/// once a repo has proved it has any, and how a repo that answers empty (or
/// can't answer at all) backs off. One list lookup is one GraphQL call —
/// one point, however many pull requests come back. Every other project's
/// list is on the slower `OPEN_PRS_SWEEP_REFRESH`.
///
/// The budget (docs.github.com, "Rate limits and node limits for the
/// GraphQL API", checked 2026-08-28): 5,000 points an hour per user token,
/// with a secondary cap of 2,000 points a minute. At fifteen seconds this
/// list and the selected worktree's `PR_REFRESH` together spend 480 an hour
/// — under a tenth of the quota — the sweep of the project's other
/// checkouts (`PR_SWEEP_REFRESH`) adds twelve an hour per checkout with a
/// pull request, and a focus-driven re-ask (see
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
/// row. `Shift+R` (`refresh_pull_requests`) is the one gesture that skips
/// the floor: a deliberate keypress may spend the call.
pub(crate) const OPEN_PRS_REFRESH: Duration = Duration::from_secs(15);
pub(crate) const OPEN_PRS_RECHECK_MIN: Duration = Duration::from_secs(30);
pub(crate) const OPEN_PRS_RECHECK_MAX: Duration = Duration::from_secs(10 * 60);
/// How often the open list of a project the cursor is *not* on is re-asked
/// — the background pass that keeps every project's group warm, so
/// switching to one shows a list minutes old at worst (and the cache the
/// next launch hydrates from is as fresh as that). One list lookup per
/// project per beat, one project per tick (`sweep_open_prs`): twelve
/// calls an hour per project against the budget above, and a project that
/// answers empty keeps its own backoff on top. Same reasoning and cadence
/// as `PR_SWEEP_REFRESH`; the selected project's list stays on
/// `OPEN_PRS_REFRESH`.
pub(crate) const OPEN_PRS_SWEEP_REFRESH: Duration = Duration::from_secs(5 * 60);

/// How long the Worktrees cursor must rest on an open-PR row before its
/// description and conversation are fetched. Long enough that arrowing
/// through a hundred rows spends nothing, short enough that stopping to
/// read one feels immediate. Answers are cached for the session, so this is
/// paid at most once per pull request.
pub(crate) const PR_DETAIL_DEBOUNCE: Duration = Duration::from_millis(300);

/// While the metrics modal is open, how often a fresh memory reading is
/// requested from the daemon.
const METRICS_POLL: Duration = Duration::from_secs(2);

/// With the modal closed, how often the footer's memory/session readout is
/// refreshed.
const FOOTER_METRICS_POLL: Duration = Duration::from_secs(5);

/// How often the grid asks the daemon what each TERMINAL on it last
/// printed, for the lines on its card (`request_terminal_tails`).
const TAIL_POLL: Duration = Duration::from_secs(1);

/// How much of a terminal's ring one such ask brings back: enough to lay
/// its last screenful out, little enough that a grid of shells asking once
/// a second is nothing on the socket — and an idle one answers with none.
const TAIL_BYTES: u32 = 16 * 1024;

/// The one hotkey that isn't only a hotkey. Whatever the user binds to
/// [`crate::keymap::Action::UnlockTerminal`], Ctrl+q also unlocks a locked
/// pane, force-closes the VIM MODAL and closes any OVERLAY outright — the
/// alternative is a config, a nested modal or a half-typed field that
/// silently traps you with no way back to the panels.
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
pub async fn run_app() -> Result<Option<crate::hosts::HostEntry>> {
    let conn = ipc::connect_or_spawn().await?;
    let mut channels = ipc::split_connection(conn);
    channels.tx.send(ClientRequest::Subscribe).await?;

    let mut terminal = setup_terminal()?;
    let result = main_loop(&mut terminal, &mut channels).await;
    restore_terminal();
    result
}

#[cfg(test)]
mod tests;
