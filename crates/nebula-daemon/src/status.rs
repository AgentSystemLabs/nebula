//! The agent status state machine. Pure logic — no I/O, injected clock — so
//! the tricky interleavings (Stop racing SubagentStart, post-turn helpers,
//! idle notifications, foreign sessions) are unit-testable.
//!
//! Semantics ported from mission-control's battle-tested implementation:
//! - `Stop` does NOT mean done while Task-tool subagents are still active;
//!   hold `running` and promote to `finished` only after a drain grace.
//! - A `SubagentStart` shortly *after* a finish heals back to `running`
//!   (the Stop raced the subagent's own hook POST) — but only within a
//!   window, because Claude runs post-turn helpers that fire subagent events
//!   with no following Stop.
//! - Hooks from a different Claude session in the same cwd are foreign and
//!   must not drive status.
//! - An `idle_prompt` notification is Claude reporting it is parked at the
//!   input box; it is what un-sticks a turn that ended without a `Stop`
//!   (rejected prompt, escape mid-turn). But since Claude Code 2.1 the Agent
//!   tool runs subagents in the *background*: the foreground turn ends, the
//!   input box comes back, and `idle_prompt` fires ~60 s later while the
//!   workers are still going. So with subagents tracked it is a hold, not a
//!   finish — the same hold a gated `Stop` gets — and only a quiet set
//!   (no subagent hook traffic for `SUBAGENT_QUIET_GRACE`) is presumed
//!   orphaned and finished anyway.
//! - `Progress` is the same end-of-turn news read straight off the PTY
//!   (OSC 9;4, see `pty::progress`). It is the only signal that survives a
//!   user cancel — no hook fires at all there, and Claude suppresses
//!   `idle_prompt` precisely because the user just touched the keyboard.
//! - Approving a permission prompt fires no hook of its own: the next news
//!   is the gated tool's `PostToolUse` (or the following call's
//!   `PreToolUse`). So a tool event from the same origin as the open
//!   dialog — the foreground turn, or the one subagent whose prompt it was
//!   — is the answer and moves the agent back to `running`. Another
//!   subagent's tool traffic says nothing about a dialog it did not raise.
//! - Claude's `permission_prompt` notification is *deferred*: a dialog
//!   sends it once it has sat 6 s with no keystroke, from a timer, with the
//!   hook detached from the turn, and its `AskUserQuestion` dialog sends
//!   the same type (it is a permission dialog in Claude's UI). One can
//!   therefore land just after the answer that closed the dialog, and
//!   would pin a row red for the rest of the turn. Inside
//!   `LATE_PROMPT_NOTIFICATION_GRACE` of leaving `needs_feedback` it is
//!   that echo and is ignored — a genuinely new dialog announces itself
//!   through `PermissionRequest` / `PreToolUse` first, and its own
//!   notification cannot arrive inside the grace.

use nebula_core::AgentStatus;
use std::collections::HashMap;
use std::time::{Duration, Instant};

pub const RECENT_FINISH_WINDOW: Duration = Duration::from_secs(30);
pub const DRAIN_GRACE: Duration = Duration::from_secs(180);
pub const SUBAGENT_TTL: Duration = Duration::from_secs(2 * 60 * 60);
/// How long a held Stop tolerates tracked subagents that show no sign of
/// life — no SubagentStart/SubagentStop, no subagent tool traffic — before
/// presuming they were killed without a SubagentStop and finishing anyway.
/// Generous on purpose: a fleet implementer's single `cargo test` can be
/// silent for many minutes, and a wrong green here is the bug this guards
/// against.
pub const SUBAGENT_QUIET_GRACE: Duration = Duration::from_secs(30 * 60);
pub const MAX_TRACKED_SUBAGENTS: usize = 512;
/// Claude Code sends a dialog's `permission_prompt` notification only once
/// the dialog has been open, and the keyboard untouched, for 6 s — from a
/// timer, with the Notification hook run detached from the turn (verified
/// against Claude Code 2.1.267, where a permission prompt and the
/// `AskUserQuestion` dialog both carry that type). One that lands within
/// this long of the status leaving `needs_feedback` is the echo of a dialog
/// the user already answered. Must stay under Claude's 6 s: a new dialog's
/// own notification never comes sooner than that after the
/// `PermissionRequest` / `PreToolUse` that opened it.
pub const LATE_PROMPT_NOTIFICATION_GRACE: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, PartialEq)]
pub enum HookEvent {
    UserPromptSubmit,
    Stop,
    SessionStart {
        source: Option<String>,
    },
    PermissionRequest {
        /// Set when the gated tool call is a subagent's: that subagent's
        /// next tool event, not the foreground's, is the approval.
        subagent_id: Option<String>,
    },
    Notification {
        notification_type: Option<String>,
    },
    PreToolUse {
        tool_name: Option<String>,
        /// Set when the call came from a subagent (claude stamps
        /// `agent_id` on subagent tool traffic): a sign of life for the
        /// STOP GATE's quiet clock, nothing more.
        subagent_id: Option<String>,
    },
    PostToolUse {
        tool_name: Option<String>,
        subagent_id: Option<String>,
    },
    SubagentStart {
        subagent_id: Option<String>,
    },
    SubagentStop {
        subagent_id: Option<String>,
    },
    /// Synthetic: the agent's PTY died.
    SessionEnded {
        exit_code: Option<i32>,
    },
    /// Synthetic: the CLI's OSC 9;4 progress state flipped. `busy: false` is
    /// end-of-turn — including the cancel that fires no hook.
    Progress {
        busy: bool,
    },
}

/// The tools whose call means the turn is waiting on you: Claude's
/// `AskUserQuestion` and pi's `ask_question` (its managed extension posts
/// the tool's own name).
fn asks_user(tool_name: Option<&str>) -> bool {
    matches!(tool_name, Some("AskUserQuestion" | "ask_question"))
}

impl HookEvent {
    /// Events that (re)establish which Claude session id owns this agent.
    pub fn captures_session(&self) -> bool {
        matches!(
            self,
            HookEvent::UserPromptSubmit | HookEvent::SessionStart { .. }
        )
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Effect {
    SetStatus(AgentStatus),
    SaveSessionId(String),
}

/// Who raised the dialog the agent is waiting on: the foreground turn, or
/// one Task-tool subagent (claude stamps `agent_id` on its hook traffic).
/// Only a tool event from the same origin counts as the dialog's answer.
#[derive(Debug, Clone, PartialEq)]
enum Origin {
    Foreground,
    Subagent(String),
}

impl Origin {
    fn of(subagent_id: Option<&str>) -> Self {
        match subagent_id {
            Some(id) => Origin::Subagent(id.to_string()),
            None => Origin::Foreground,
        }
    }
}

#[derive(Default)]
struct SubagentSet {
    keyed: HashMap<String, Instant>,
    anon: usize,
}

impl SubagentSet {
    fn start(&mut self, id: Option<String>, now: Instant) {
        match id {
            Some(id) if self.keyed.len() < MAX_TRACKED_SUBAGENTS => {
                self.keyed.insert(id, now);
            }
            Some(_) | None => self.anon = self.anon.saturating_add(1).min(MAX_TRACKED_SUBAGENTS),
        }
    }

    /// Cross-cancel bias toward finishing: a keyed stop with no matching start
    /// cancels an anon start; an anon stop cancels the oldest keyed start.
    fn stop(&mut self, id: Option<String>) {
        match id {
            Some(id) => {
                if self.keyed.remove(&id).is_none() {
                    self.anon = self.anon.saturating_sub(1);
                }
            }
            None => {
                if self.anon > 0 {
                    self.anon -= 1;
                } else if let Some(oldest) = self
                    .keyed
                    .iter()
                    .min_by_key(|(_, t)| **t)
                    .map(|(k, _)| k.clone())
                {
                    self.keyed.remove(&oldest);
                }
            }
        }
    }

    fn prune_expired(&mut self, now: Instant) {
        self.keyed
            .retain(|_, started| now.duration_since(*started) < SUBAGENT_TTL);
        // Anon starts can't be aged individually; they are cleared wholesale on
        // session change / clear / prompt.
    }

    fn is_empty(&self) -> bool {
        self.keyed.is_empty() && self.anon == 0
    }

    fn clear(&mut self) {
        self.keyed.clear();
        self.anon = 0;
    }
}

pub struct AgentStatusMachine {
    status: AgentStatus,
    session_id: Option<String>,
    subagents: SubagentSet,
    finished_at: Option<Instant>,
    /// Set while a Stop is being held open because subagents were active.
    stop_held: bool,
    /// When the subagent set last became empty during a held Stop.
    drain_idle_since: Option<Instant>,
    /// The last moment the tracked subagents proved they are alive — a
    /// SubagentStart/Stop or subagent tool traffic — or, failing that, when
    /// the hold began. `tick` gives up on the hold once this is
    /// `SUBAGENT_QUIET_GRACE` old.
    subagent_alive_at: Option<Instant>,
    /// While `needs_feedback`: who raised the open dialog. A tool event
    /// from that origin is its answer.
    waiting_on: Option<Origin>,
    /// When the status last left `needs_feedback` — the user answered, or
    /// the turn ended around the dialog. A `permission_prompt` notification
    /// inside `LATE_PROMPT_NOTIFICATION_GRACE` of it is a late echo.
    feedback_left_at: Option<Instant>,
}

impl AgentStatusMachine {
    pub fn new(status: AgentStatus, session_id: Option<String>) -> Self {
        Self {
            status,
            session_id,
            subagents: SubagentSet::default(),
            finished_at: None,
            stop_held: false,
            drain_idle_since: None,
            subagent_alive_at: None,
            waiting_on: None,
            feedback_left_at: None,
        }
    }

    pub fn status(&self) -> AgentStatus {
        self.status
    }

    pub fn handle(
        &mut self,
        event: HookEvent,
        payload_session_id: Option<&str>,
        now: Instant,
    ) -> Vec<Effect> {
        let mut effects = Vec::new();

        // ---- session reconciliation ----
        if event.captures_session() {
            if let Some(sid) = payload_session_id {
                if self.session_id.as_deref() != Some(sid) {
                    // New claude process (restart / manual relaunch): adopt it
                    // and forget the old session's subagents.
                    self.session_id = Some(sid.to_string());
                    self.subagents.clear();
                    self.stop_held = false;
                    self.drain_idle_since = None;
                    self.subagent_alive_at = None;
                    effects.push(Effect::SaveSessionId(sid.to_string()));
                }
            }
        } else if !matches!(event, HookEvent::SessionEnded { .. }) {
            if let (Some(mine), Some(theirs)) = (self.session_id.as_deref(), payload_session_id) {
                if mine != theirs {
                    return effects; // foreign session — ignore entirely
                }
            }
        }

        let was_waiting = self.status == AgentStatus::NeedsFeedback;
        match event {
            HookEvent::UserPromptSubmit => {
                self.stop_held = false;
                self.drain_idle_since = None;
                self.subagent_alive_at = None;
                self.finished_at = None;
                self.set_status(AgentStatus::Running, &mut effects);
            }
            HookEvent::Stop => self.end_turn(now, &mut effects),
            HookEvent::SessionStart { source } => {
                if source.as_deref() == Some("clear") {
                    // Same session id, but /clear killed any live subagents.
                    self.subagents.clear();
                    self.stop_held = false;
                    self.drain_idle_since = None;
                    self.subagent_alive_at = None;
                }
            }
            HookEvent::PermissionRequest { subagent_id } => {
                self.wait_on(Origin::of(subagent_id.as_deref()), &mut effects);
            }
            HookEvent::Notification { notification_type } => {
                match notification_type.as_deref() {
                    // Deferred by 6 s on Claude's side, so it is never the
                    // first word of a dialog nebula can see — and it can be
                    // the last word of one the user has already closed.
                    Some("permission_prompt") => {
                        if !self.late_prompt_echo(now) {
                            self.wait_on(Origin::Foreground, &mut effects);
                        }
                    }
                    // "Claude is waiting for your input". Claude fires this
                    // only with nothing in flight and no dialog open, so it
                    // means the turn really is over — see `mark_idle`.
                    Some("idle_prompt") => self.mark_idle(now, &mut effects),
                    // Every other notification type (auth, quota, nested
                    // fleet sessions) is none of our business.
                    _ => {}
                }
            }
            HookEvent::PreToolUse {
                tool_name,
                subagent_id,
            } => {
                if subagent_id.is_some() {
                    self.note_subagent_alive(now);
                }
                if asks_user(tool_name.as_deref()) {
                    self.wait_on(Origin::of(subagent_id.as_deref()), &mut effects);
                } else {
                    // The next call is being made: whatever this origin was
                    // waiting on has been answered.
                    self.dialog_closed(Origin::of(subagent_id.as_deref()), &mut effects);
                }
            }
            HookEvent::PostToolUse {
                tool_name,
                subagent_id,
            } => {
                if subagent_id.is_some() {
                    self.note_subagent_alive(now);
                }
                if asks_user(tool_name.as_deref()) {
                    // The question's answer: the turn has it and is running
                    // again, whoever asked.
                    self.set_status(AgentStatus::Running, &mut effects);
                } else {
                    // The gated tool ran: the permission prompt was
                    // approved. This is the only hook an approval fires.
                    self.dialog_closed(Origin::of(subagent_id.as_deref()), &mut effects);
                }
            }
            HookEvent::SubagentStart { subagent_id } => {
                self.subagents.start(subagent_id, now);
                self.note_subagent_alive(now);
                if self.status == AgentStatus::Finished {
                    match self.finished_at {
                        // The Stop raced this subagent's own POST — heal.
                        Some(finished) if now.duration_since(finished) < RECENT_FINISH_WINDOW => {
                            self.stop_held = true;
                            self.drain_idle_since = None;
                            self.set_status(AgentStatus::Running, &mut effects);
                        }
                        // Post-turn internal helper (away-summary etc.): a
                        // start with no Stop coming. Track it, don't heal —
                        // healing here wedges the agent on running forever.
                        _ => {}
                    }
                }
            }
            HookEvent::SubagentStop { subagent_id } => {
                self.subagents.stop(subagent_id);
                self.note_subagent_alive(now);
            }
            HookEvent::Progress { busy } => {
                if busy {
                    // A turn started. Normally `UserPromptSubmit` already
                    // said so; this also catches turns nebula never saw a
                    // prompt for (a resumed session, a scheduled wake-up).
                    // Deliberately narrow: it must not talk over a pending
                    // permission prompt (which holds progress at *busy*
                    // anyway, so no edge arrives) or revive a dead agent.
                    if matches!(self.status, AgentStatus::Fresh | AgentStatus::Finished) {
                        self.stop_held = false;
                        self.drain_idle_since = None;
                        self.subagent_alive_at = None;
                        self.finished_at = None;
                        self.set_status(AgentStatus::Running, &mut effects);
                    }
                } else if matches!(
                    self.status,
                    AgentStatus::Running | AgentStatus::NeedsFeedback
                ) {
                    // The CLI cleared its progress bar: the turn is over,
                    // however it ended. Same bookkeeping as `Stop`, so a
                    // real Stop arriving either side of this is a no-op and
                    // the subagent drain hold still applies.
                    self.end_turn(now, &mut effects);
                }
                // Fresh / Terminated / Disconnected are left alone: a CLI
                // clears its progress bar on startup and on exit too, and
                // neither is a finished turn.
            }
            HookEvent::SessionEnded { exit_code } => {
                // Dead process: laggard subagent POSTs must never resurrect it.
                self.subagents.clear();
                self.stop_held = false;
                self.drain_idle_since = None;
                self.subagent_alive_at = None;
                self.finished_at = None;
                if matches!(
                    self.status,
                    AgentStatus::Running | AgentStatus::NeedsFeedback
                ) {
                    let status = if exit_code == Some(0) {
                        AgentStatus::Finished
                    } else {
                        AgentStatus::Terminated
                    };
                    self.set_status(status, &mut effects);
                }
            }
        }
        if was_waiting && self.status != AgentStatus::NeedsFeedback {
            self.waiting_on = None;
            self.feedback_left_at = Some(now);
        }
        effects
    }

    /// Periodic tick (the deferred-finish recheck): while a Stop is held open,
    /// promote to finished once the subagent set has drained and stayed empty
    /// for the grace period — or once the set has gone quiet for
    /// `SUBAGENT_QUIET_GRACE`, which means its SubagentStops are never coming
    /// (killed tasks, a crashed worker) and holding longer only wedges the
    /// agent on yellow.
    pub fn tick(&mut self, now: Instant) -> Vec<Effect> {
        let mut effects = Vec::new();
        if !self.stop_held || self.status != AgentStatus::Running {
            return effects;
        }
        self.subagents.prune_expired(now);
        if self.subagents.is_empty() {
            match self.drain_idle_since {
                None => self.drain_idle_since = Some(now),
                Some(idle_since) if now.duration_since(idle_since) >= DRAIN_GRACE => {
                    self.stop_held = false;
                    self.drain_idle_since = None;
                    self.subagent_alive_at = None;
                    self.finished_at = Some(now);
                    self.set_status(AgentStatus::Finished, &mut effects);
                }
                Some(_) => {}
            }
        } else {
            self.drain_idle_since = None;
            let quiet_since = *self.subagent_alive_at.get_or_insert(now);
            if now.duration_since(quiet_since) >= SUBAGENT_QUIET_GRACE {
                // Orphaned: same exit as an idle notification with nothing
                // tracked, and no `finished_at` for the same reason.
                self.subagents.clear();
                self.stop_held = false;
                self.subagent_alive_at = None;
                self.finished_at = None;
                self.set_status(AgentStatus::Finished, &mut effects);
            }
        }
        effects
    }

    /// A tracked subagent just proved it is alive (its own hook traffic):
    /// restart the quiet clock the hold is measured against.
    fn note_subagent_alive(&mut self, now: Instant) {
        self.subagent_alive_at = Some(now);
    }

    /// A dialog opened (or is reported open): wait on its origin. A second
    /// report while already red keeps the first origin — one dialog shows
    /// at a time, and the notification never names one anyway.
    fn wait_on(&mut self, origin: Origin, effects: &mut Vec<Effect>) {
        if self.status != AgentStatus::NeedsFeedback {
            self.waiting_on = Some(origin);
        }
        self.set_status(AgentStatus::NeedsFeedback, effects);
    }

    /// A tool event from `origin` — its call completing, or its next call
    /// starting — means no dialog of that origin can be open. If that is
    /// the one being waited on, the user answered it: back to `running`.
    /// Only from red — a tool hook never starts a turn nebula did not see
    /// begin, and never revives a dead agent.
    fn dialog_closed(&mut self, origin: Origin, effects: &mut Vec<Effect>) {
        if self.status == AgentStatus::NeedsFeedback && self.waiting_on.as_ref() == Some(&origin) {
            self.set_status(AgentStatus::Running, effects);
        }
    }

    /// A `permission_prompt` notification this soon after the status left
    /// `needs_feedback` is Claude's deferred timer catching up with a
    /// dialog the user already closed — see `LATE_PROMPT_NOTIFICATION_GRACE`.
    fn late_prompt_echo(&self, now: Instant) -> bool {
        self.feedback_left_at
            .is_some_and(|left| now.duration_since(left) < LATE_PROMPT_NOTIFICATION_GRACE)
    }

    /// The foreground turn ended — a `Stop`, or the CLI clearing its
    /// progress bar. Finished outright when no subagent is still tracked;
    /// otherwise the stop is held at running and `tick` promotes it once
    /// the set has drained and stayed empty for the grace period.
    fn end_turn(&mut self, now: Instant, effects: &mut Vec<Effect>) {
        self.subagents.prune_expired(now);
        if self.subagents.is_empty() {
            self.stop_held = false;
            self.finished_at = Some(now);
            self.set_status(AgentStatus::Finished, effects);
        } else {
            // Foreground turn ended but subagents are still working.
            self.hold_for_subagents(now, effects);
        }
    }

    /// Keep (or put) the agent at running because subagents are still
    /// tracked. The quiet clock starts now unless a subagent has already
    /// shown life more recently than the hold.
    fn hold_for_subagents(&mut self, now: Instant, effects: &mut Vec<Effect>) {
        self.stop_held = true;
        self.drain_idle_since = None;
        self.subagent_alive_at.get_or_insert(now);
        self.set_status(AgentStatus::Running, effects);
    }

    /// Claude reports itself idle at the input box. This is the only end-of-
    /// turn signal that survives the paths where no `Stop` ever fires: the
    /// user rejecting a permission prompt or an `AskUserQuestion`, or hitting
    /// escape mid-turn. Without it those leave the agent pinned on red (or
    /// yellow) until the next prompt, long after the CLI went quiet.
    ///
    /// Safe to trust as "no dialog is open" because Claude gates the
    /// notification on an idle main loop AND an empty dialog stack — a
    /// permission prompt still waiting on the user suppresses it, so this
    /// can't green out an agent that genuinely needs feedback.
    ///
    /// It is *not* proof that the turn's work is over: the Agent tool runs
    /// its subagents in the background, the foreground loop parks at the
    /// input box while they work, and this fires ~60 s in. With subagents
    /// still tracked the agent stays at running exactly as a gated `Stop`
    /// would — `tick` finishes it once they drain, or once they have been
    /// quiet for `SUBAGENT_QUIET_GRACE`.
    fn mark_idle(&mut self, now: Instant, effects: &mut Vec<Effect>) {
        if !matches!(
            self.status,
            AgentStatus::Running | AgentStatus::NeedsFeedback
        ) {
            return;
        }
        self.subagents.prune_expired(now);
        if !self.subagents.is_empty() {
            self.hold_for_subagents(now, effects);
            return;
        }
        self.stop_held = false;
        self.drain_idle_since = None;
        self.subagent_alive_at = None;
        // Deliberately no `finished_at`: after this much idle time a
        // SubagentStart is a post-turn helper, never a Stop that raced its
        // own POST, so it must not heal back to running.
        self.finished_at = None;
        self.set_status(AgentStatus::Finished, effects);
    }

    fn set_status(&mut self, status: AgentStatus, effects: &mut Vec<Effect>) {
        if self.status != status {
            self.status = status;
            effects.push(Effect::SetStatus(status));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t0() -> Instant {
        Instant::now()
    }

    fn status_of(effects: &[Effect]) -> Option<AgentStatus> {
        effects.iter().rev().find_map(|e| match e {
            Effect::SetStatus(s) => Some(*s),
            _ => None,
        })
    }

    #[test]
    fn normal_turn_lifecycle() {
        let mut m = AgentStatusMachine::new(AgentStatus::Fresh, None);
        let now = t0();
        let fx = m.handle(HookEvent::UserPromptSubmit, Some("s1"), now);
        assert_eq!(status_of(&fx), Some(AgentStatus::Running));
        assert!(fx.contains(&Effect::SaveSessionId("s1".into())));
        let fx = m.handle(HookEvent::Stop, Some("s1"), now + Duration::from_secs(10));
        assert_eq!(status_of(&fx), Some(AgentStatus::Finished));
    }

    /// pi's question tool goes through the same red-then-back flow as
    /// Claude's AskUserQuestion; any other tool name is not a wait.
    #[test]
    fn pi_ask_question_reads_as_waiting_on_you() {
        let mut m = AgentStatusMachine::new(AgentStatus::Fresh, None);
        let now = t0();
        m.handle(HookEvent::UserPromptSubmit, Some("s1"), now);
        let fx = m.handle(
            HookEvent::PreToolUse {
                tool_name: Some("ask_question".into()),
                subagent_id: None,
            },
            Some("s1"),
            now,
        );
        assert_eq!(status_of(&fx), Some(AgentStatus::NeedsFeedback));
        let fx = m.handle(
            HookEvent::PostToolUse {
                tool_name: Some("ask_question".into()),
                subagent_id: None,
            },
            Some("s1"),
            now,
        );
        assert_eq!(status_of(&fx), Some(AgentStatus::Running));
        let fx = m.handle(
            HookEvent::PreToolUse {
                tool_name: Some("bash".into()),
                subagent_id: None,
            },
            Some("s1"),
            now,
        );
        assert_eq!(status_of(&fx), None, "an ordinary tool changes nothing");
        assert!(!asks_user(None));
    }

    fn tool(name: &str, subagent: Option<&str>, post: bool) -> HookEvent {
        let tool_name = Some(name.to_string());
        let subagent_id = subagent.map(str::to_string);
        if post {
            HookEvent::PostToolUse {
                tool_name,
                subagent_id,
            }
        } else {
            HookEvent::PreToolUse {
                tool_name,
                subagent_id,
            }
        }
    }

    fn prompt_notification(m: &mut AgentStatusMachine, now: Instant) -> Vec<Effect> {
        m.handle(
            HookEvent::Notification {
                notification_type: Some("permission_prompt".into()),
            },
            Some("s1"),
            now,
        )
    }

    #[test]
    fn permission_prompt_flow() {
        // The reported bug: approving a permission prompt fires no hook of
        // its own, and the row sat red until the turn ended. The gated
        // tool's PostToolUse is the approval.
        let mut m = AgentStatusMachine::new(AgentStatus::Fresh, None);
        let now = t0();
        m.handle(HookEvent::UserPromptSubmit, Some("s1"), now);
        let fx = m.handle(
            HookEvent::PermissionRequest { subagent_id: None },
            Some("s1"),
            now,
        );
        assert_eq!(status_of(&fx), Some(AgentStatus::NeedsFeedback));
        let fx = m.handle(
            tool("Bash", None, true),
            Some("s1"),
            now + Duration::from_secs(9),
        );
        assert_eq!(status_of(&fx), Some(AgentStatus::Running));
        // …and Edit, WebFetch, an MCP tool: whatever was gated.
        for name in ["Edit", "WebFetch", "mcp__github__create_issue"] {
            m.handle(
                HookEvent::PermissionRequest { subagent_id: None },
                Some("s1"),
                now,
            );
            assert_eq!(m.status(), AgentStatus::NeedsFeedback);
            let fx = m.handle(tool(name, None, true), Some("s1"), now);
            assert_eq!(status_of(&fx), Some(AgentStatus::Running), "{name}");
        }
    }

    #[test]
    fn the_next_calls_pre_tool_use_also_answers_a_prompt() {
        // A permission prompt on a call whose PostToolUse nebula never sees
        // (a harness with a narrower hook set): the following call's
        // PreToolUse still proves the dialog is gone.
        let mut m = AgentStatusMachine::new(AgentStatus::Fresh, None);
        let now = t0();
        m.handle(HookEvent::UserPromptSubmit, Some("s1"), now);
        m.handle(
            HookEvent::PermissionRequest { subagent_id: None },
            Some("s1"),
            now,
        );
        let fx = m.handle(tool("Bash", None, false), Some("s1"), now);
        assert_eq!(status_of(&fx), Some(AgentStatus::Running));
        // A question opening is not an answer, whichever hook carries it.
        let fx = m.handle(tool("AskUserQuestion", None, false), Some("s1"), now);
        assert_eq!(status_of(&fx), Some(AgentStatus::NeedsFeedback));
    }

    #[test]
    fn a_subagents_traffic_never_answers_the_foreground_prompt() {
        // Background workers keep calling tools while the foreground turn
        // waits on the user; none of that is the user answering.
        let mut m = AgentStatusMachine::new(AgentStatus::Fresh, None);
        let now = t0();
        m.handle(HookEvent::UserPromptSubmit, Some("s1"), now);
        m.handle(
            HookEvent::SubagentStart {
                subagent_id: Some("sub1".into()),
            },
            Some("s1"),
            now,
        );
        m.handle(
            HookEvent::PermissionRequest { subagent_id: None },
            Some("s1"),
            now,
        );
        for post in [false, true] {
            let fx = m.handle(tool("Bash", Some("sub1"), post), Some("s1"), now);
            assert!(fx.is_empty(), "{fx:?}");
            assert_eq!(m.status(), AgentStatus::NeedsFeedback);
        }
        let fx = m.handle(tool("Bash", None, true), Some("s1"), now);
        assert_eq!(status_of(&fx), Some(AgentStatus::Running));
    }

    #[test]
    fn a_subagents_own_prompt_is_answered_by_its_own_traffic() {
        // A worker's gated call prompts the user too; only that worker's
        // next tool event says it was approved — the foreground's traffic
        // (or another worker's) is not it.
        let mut m = AgentStatusMachine::new(AgentStatus::Fresh, None);
        let now = t0();
        m.handle(HookEvent::UserPromptSubmit, Some("s1"), now);
        let fx = m.handle(
            HookEvent::PermissionRequest {
                subagent_id: Some("sub1".into()),
            },
            Some("s1"),
            now,
        );
        assert_eq!(status_of(&fx), Some(AgentStatus::NeedsFeedback));
        for origin in [None, Some("sub2")] {
            let fx = m.handle(tool("Bash", origin, true), Some("s1"), now);
            assert!(fx.is_empty(), "{origin:?}: {fx:?}");
        }
        let fx = m.handle(tool("Bash", Some("sub1"), true), Some("s1"), now);
        assert_eq!(status_of(&fx), Some(AgentStatus::Running));
    }

    #[test]
    fn a_tool_hook_never_starts_a_turn_or_revives_the_dead() {
        for start in [
            AgentStatus::Fresh,
            AgentStatus::Finished,
            AgentStatus::Terminated,
            AgentStatus::Disconnected,
        ] {
            let mut m = AgentStatusMachine::new(start, Some("s1".into()));
            for post in [false, true] {
                let fx = m.handle(tool("Bash", None, post), Some("s1"), t0());
                assert!(fx.is_empty(), "{start:?} must not be touched: {fx:?}");
                assert_eq!(m.status(), start);
            }
        }
    }

    #[test]
    fn late_permission_prompt_notification_after_an_answer_is_ignored() {
        // The reported bug: Claude sends a dialog's `permission_prompt`
        // notification from a 6 s timer, detached from the turn, and its
        // AskUserQuestion dialog uses that type too. When the timer and the
        // user's answer coincide, the notification lands after the
        // PostToolUse and pinned the row red for the rest of the turn.
        let mut m = AgentStatusMachine::new(AgentStatus::Fresh, None);
        let now = t0();
        m.handle(HookEvent::UserPromptSubmit, Some("s1"), now);
        m.handle(tool("AskUserQuestion", None, false), Some("s1"), now);
        assert_eq!(m.status(), AgentStatus::NeedsFeedback);
        let answered = now + Duration::from_secs(6);
        let fx = m.handle(tool("AskUserQuestion", None, true), Some("s1"), answered);
        assert_eq!(status_of(&fx), Some(AgentStatus::Running));
        let fx = prompt_notification(&mut m, answered + Duration::from_millis(80));
        assert!(fx.is_empty(), "the echo must not re-redden: {fx:?}");
        assert_eq!(m.status(), AgentStatus::Running);
        // Same for a permission prompt approved as its timer fired.
        m.handle(
            HookEvent::PermissionRequest { subagent_id: None },
            Some("s1"),
            answered,
        );
        let approved = answered + Duration::from_secs(6);
        m.handle(tool("Bash", None, true), Some("s1"), approved);
        let fx = prompt_notification(&mut m, approved + Duration::from_millis(80));
        assert!(fx.is_empty(), "{fx:?}");
        assert_eq!(m.status(), AgentStatus::Running);
    }

    #[test]
    fn permission_prompt_notification_outside_the_grace_still_flags() {
        // Past the grace it is news again: a dialog whose opening nebula
        // missed (a lost hook) must still be able to go red on it.
        let mut m = AgentStatusMachine::new(AgentStatus::Fresh, None);
        let now = t0();
        m.handle(HookEvent::UserPromptSubmit, Some("s1"), now);
        m.handle(tool("AskUserQuestion", None, false), Some("s1"), now);
        let answered = now + Duration::from_secs(6);
        m.handle(tool("AskUserQuestion", None, true), Some("s1"), answered);
        let fx = prompt_notification(&mut m, answered + LATE_PROMPT_NOTIFICATION_GRACE);
        assert_eq!(status_of(&fx), Some(AgentStatus::NeedsFeedback));
        // A row that never waited has no echo to absorb.
        let mut m = AgentStatusMachine::new(AgentStatus::Fresh, None);
        m.handle(HookEvent::UserPromptSubmit, Some("s1"), now);
        let fx = prompt_notification(&mut m, now + Duration::from_secs(7));
        assert_eq!(status_of(&fx), Some(AgentStatus::NeedsFeedback));
    }

    #[test]
    fn a_new_dialog_right_after_an_answer_still_goes_red() {
        // The grace only mutes the deferred notification; the hooks that
        // open a dialog are never deferred, so a fresh prompt seconds after
        // an answer reads red at once, and its own (6 s later) notification
        // is a no-op on a row already red.
        let mut m = AgentStatusMachine::new(AgentStatus::Fresh, None);
        let now = t0();
        m.handle(HookEvent::UserPromptSubmit, Some("s1"), now);
        m.handle(tool("AskUserQuestion", None, false), Some("s1"), now);
        m.handle(
            tool("AskUserQuestion", None, true),
            Some("s1"),
            now + Duration::from_secs(6),
        );
        let fx = m.handle(
            HookEvent::PermissionRequest { subagent_id: None },
            Some("s1"),
            now + Duration::from_secs(7),
        );
        assert_eq!(status_of(&fx), Some(AgentStatus::NeedsFeedback));
        let fx = prompt_notification(&mut m, now + Duration::from_secs(13));
        assert!(fx.is_empty());
        assert_eq!(m.status(), AgentStatus::NeedsFeedback);
        let fx = m.handle(
            tool("Bash", None, true),
            Some("s1"),
            now + Duration::from_secs(20),
        );
        assert_eq!(status_of(&fx), Some(AgentStatus::Running));
    }

    #[test]
    fn late_permission_prompt_notification_after_a_rejected_prompt_is_ignored() {
        // Escape out of a prompt as its timer fires: the turn is over
        // (progress cleared), and the echo must not paint a finished row red.
        let mut m = AgentStatusMachine::new(AgentStatus::Fresh, None);
        let now = t0();
        m.handle(HookEvent::UserPromptSubmit, Some("s1"), now);
        m.handle(
            HookEvent::PermissionRequest { subagent_id: None },
            Some("s1"),
            now,
        );
        let rejected = now + Duration::from_secs(6);
        let fx = progress(&mut m, false, rejected);
        assert_eq!(status_of(&fx), Some(AgentStatus::Finished));
        let fx = prompt_notification(&mut m, rejected + Duration::from_millis(80));
        assert!(fx.is_empty(), "{fx:?}");
        assert_eq!(m.status(), AgentStatus::Finished);
    }

    fn idle(m: &mut AgentStatusMachine, now: Instant) -> Vec<Effect> {
        m.handle(
            HookEvent::Notification {
                notification_type: Some("idle_prompt".into()),
            },
            Some("s1"),
            now,
        )
    }

    #[test]
    fn idle_notification_is_a_noop_when_already_finished() {
        let mut m = AgentStatusMachine::new(AgentStatus::Fresh, None);
        let now = t0();
        m.handle(HookEvent::UserPromptSubmit, Some("s1"), now);
        m.handle(HookEvent::Stop, Some("s1"), now);
        let fx = idle(&mut m, now);
        assert!(fx.is_empty(), "already finished: {fx:?}");
        assert_eq!(m.status(), AgentStatus::Finished);
    }

    #[test]
    fn idle_notification_clears_a_rejected_question() {
        // The reported bug: AskUserQuestion goes red, the user rejects it,
        // and the interrupted turn fires neither PostToolUse nor Stop.
        let mut m = AgentStatusMachine::new(AgentStatus::Fresh, None);
        let now = t0();
        m.handle(HookEvent::UserPromptSubmit, Some("s1"), now);
        let fx = m.handle(
            HookEvent::PreToolUse {
                tool_name: Some("AskUserQuestion".into()),
                subagent_id: None,
            },
            Some("s1"),
            now,
        );
        assert_eq!(status_of(&fx), Some(AgentStatus::NeedsFeedback));
        let fx = idle(&mut m, now + Duration::from_secs(60));
        assert_eq!(status_of(&fx), Some(AgentStatus::Finished));
    }

    #[test]
    fn idle_notification_clears_a_stale_running_turn() {
        // Escape mid-turn: no Stop ever arrives, so running would stick.
        let mut m = AgentStatusMachine::new(AgentStatus::Fresh, None);
        let now = t0();
        m.handle(HookEvent::UserPromptSubmit, Some("s1"), now);
        let fx = idle(&mut m, now + Duration::from_secs(60));
        assert_eq!(status_of(&fx), Some(AgentStatus::Finished));
    }

    fn subagent(m: &mut AgentStatusMachine, start: bool, id: &str, now: Instant) -> Vec<Effect> {
        let ev = if start {
            HookEvent::SubagentStart {
                subagent_id: Some(id.into()),
            }
        } else {
            HookEvent::SubagentStop {
                subagent_id: Some(id.into()),
            }
        };
        m.handle(ev, Some("s1"), now)
    }

    #[test]
    fn idle_notification_holds_running_while_background_subagents_work() {
        // The reported bug: the Agent tool runs subagents in the background,
        // the foreground turn ends (Stop, held), and ~60 s later Claude's
        // idle_prompt fires with the workers still going — that must not
        // green the session out.
        let mut m = AgentStatusMachine::new(AgentStatus::Fresh, None);
        let now = t0();
        m.handle(HookEvent::UserPromptSubmit, Some("s1"), now);
        subagent(&mut m, true, "sub1", now);
        m.handle(HookEvent::Stop, Some("s1"), now);
        assert_eq!(m.status(), AgentStatus::Running, "stop held open");
        let fx = idle(&mut m, now + Duration::from_secs(60));
        assert!(fx.is_empty(), "idle with live subagents is a hold: {fx:?}");
        assert_eq!(m.status(), AgentStatus::Running);
        assert!(m.tick(now + Duration::from_secs(90)).is_empty());

        // The worker finishes and its completion re-invokes the foreground
        // turn (progress busy, then a real Stop): finished outright.
        subagent(&mut m, false, "sub1", now + Duration::from_secs(95));
        m.handle(
            HookEvent::Progress { busy: true },
            Some("s1"),
            now + Duration::from_secs(96),
        );
        assert_eq!(m.status(), AgentStatus::Running);
        let fx = m.handle(HookEvent::Stop, Some("s1"), now + Duration::from_secs(100));
        assert_eq!(status_of(&fx), Some(AgentStatus::Finished));
    }

    #[test]
    fn idle_hold_drains_through_the_grace_when_nothing_reinvokes_the_turn() {
        let mut m = AgentStatusMachine::new(AgentStatus::Fresh, None);
        let now = t0();
        m.handle(HookEvent::UserPromptSubmit, Some("s1"), now);
        subagent(&mut m, true, "sub1", now);
        m.handle(HookEvent::Stop, Some("s1"), now);
        idle(&mut m, now + Duration::from_secs(60));
        subagent(&mut m, false, "sub1", now + Duration::from_secs(120));
        let t = now + Duration::from_secs(121);
        assert!(m.tick(t).is_empty(), "drain grace starts");
        assert!(m.tick(t + DRAIN_GRACE - Duration::from_secs(1)).is_empty());
        let fx = m.tick(t + DRAIN_GRACE);
        assert_eq!(status_of(&fx), Some(AgentStatus::Finished));
    }

    #[test]
    fn idle_notification_with_a_rejected_prompt_still_holds_for_subagents() {
        // A permission prompt rejected while a background worker runs: the
        // dialog is gone (idle_prompt proves that) but the turn is not over.
        let mut m = AgentStatusMachine::new(AgentStatus::Fresh, None);
        let now = t0();
        m.handle(HookEvent::UserPromptSubmit, Some("s1"), now);
        subagent(&mut m, true, "sub1", now);
        m.handle(
            HookEvent::PermissionRequest { subagent_id: None },
            Some("s1"),
            now,
        );
        assert_eq!(m.status(), AgentStatus::NeedsFeedback);
        let fx = idle(&mut m, now + Duration::from_secs(60));
        assert_eq!(status_of(&fx), Some(AgentStatus::Running));
    }

    #[test]
    fn quiet_subagents_are_presumed_orphaned_after_the_quiet_grace() {
        // A worker killed without a SubagentStop: the hold must not wedge
        // the agent on yellow until SUBAGENT_TTL.
        let mut m = AgentStatusMachine::new(AgentStatus::Fresh, None);
        let now = t0();
        m.handle(HookEvent::UserPromptSubmit, Some("s1"), now);
        subagent(&mut m, true, "sub1", now);
        m.handle(HookEvent::Stop, Some("s1"), now + Duration::from_secs(5));
        idle(&mut m, now + Duration::from_secs(65));
        assert_eq!(m.status(), AgentStatus::Running);
        // The quiet clock runs from the last sign of life — the
        // SubagentStart at `now` — not from the Stop or the idle_prompt.
        assert!(m
            .tick(now + SUBAGENT_QUIET_GRACE - Duration::from_secs(1))
            .is_empty());
        let fx = m.tick(now + SUBAGENT_QUIET_GRACE);
        assert_eq!(status_of(&fx), Some(AgentStatus::Finished));
        // A helper subagent afterwards must not heal it back to running.
        let fx = subagent(
            &mut m,
            true,
            "sub2",
            now + SUBAGENT_QUIET_GRACE + Duration::from_secs(1),
        );
        assert!(fx.is_empty(), "post-orphan subagent must not heal: {fx:?}");
        assert_eq!(m.status(), AgentStatus::Finished);
    }

    #[test]
    fn subagent_traffic_resets_the_quiet_clock() {
        let mut m = AgentStatusMachine::new(AgentStatus::Fresh, None);
        let now = t0();
        m.handle(HookEvent::UserPromptSubmit, Some("s1"), now);
        subagent(&mut m, true, "sub1", now);
        m.handle(HookEvent::Stop, Some("s1"), now);
        idle(&mut m, now + Duration::from_secs(60));

        // Twenty minutes in, the worker runs a Bash call (agent_id stamped).
        let t1 = now + Duration::from_secs(20 * 60);
        m.handle(
            HookEvent::PostToolUse {
                tool_name: Some("Bash".into()),
                subagent_id: Some("sub1".into()),
            },
            Some("s1"),
            t1,
        );
        assert!(m
            .tick(now + SUBAGENT_QUIET_GRACE + Duration::from_secs(1))
            .is_empty());
        // A sibling starting resets it again; the main agent's own tool
        // traffic (no agent_id) does not count.
        let t2 = t1 + Duration::from_secs(20 * 60);
        subagent(&mut m, true, "sub2", t2);
        m.handle(
            HookEvent::PostToolUse {
                tool_name: Some("Bash".into()),
                subagent_id: None,
            },
            Some("s1"),
            t2 + Duration::from_secs(25 * 60),
        );
        assert!(m
            .tick(t1 + SUBAGENT_QUIET_GRACE + Duration::from_secs(1))
            .is_empty());
        let fx = m.tick(t2 + SUBAGENT_QUIET_GRACE);
        assert_eq!(status_of(&fx), Some(AgentStatus::Finished));
    }

    #[test]
    fn idle_notification_leaves_fresh_and_dead_agents_alone() {
        for start in [
            AgentStatus::Fresh,
            AgentStatus::Terminated,
            AgentStatus::Disconnected,
        ] {
            let mut m = AgentStatusMachine::new(start, Some("s1".into()));
            let fx = idle(&mut m, t0());
            assert!(fx.is_empty(), "{start:?} must not be touched: {fx:?}");
            assert_eq!(m.status(), start);
        }
    }

    #[test]
    fn foreign_idle_notification_is_ignored() {
        let mut m = AgentStatusMachine::new(AgentStatus::Fresh, None);
        let now = t0();
        m.handle(HookEvent::UserPromptSubmit, Some("s1"), now);
        m.handle(
            HookEvent::PermissionRequest { subagent_id: None },
            Some("s1"),
            now,
        );
        let fx = m.handle(
            HookEvent::Notification {
                notification_type: Some("idle_prompt".into()),
            },
            Some("someone-elses-claude"),
            now,
        );
        assert!(fx.is_empty(), "foreign session: {fx:?}");
        assert_eq!(m.status(), AgentStatus::NeedsFeedback);
    }

    #[test]
    fn unknown_notification_types_are_ignored() {
        let mut m = AgentStatusMachine::new(AgentStatus::Fresh, None);
        let now = t0();
        m.handle(HookEvent::UserPromptSubmit, Some("s1"), now);
        for ty in [
            "auth_success",
            "agent_needs_input",
            "quota_auto_resume_fired",
        ] {
            let fx = m.handle(
                HookEvent::Notification {
                    notification_type: Some(ty.into()),
                },
                Some("s1"),
                now,
            );
            assert!(fx.is_empty(), "{ty} must not flip status: {fx:?}");
        }
        assert_eq!(m.status(), AgentStatus::Running);
    }

    #[test]
    fn stop_with_active_subagents_holds_running_until_drained() {
        let mut m = AgentStatusMachine::new(AgentStatus::Fresh, None);
        let now = t0();
        m.handle(HookEvent::UserPromptSubmit, Some("s1"), now);
        m.handle(
            HookEvent::SubagentStart {
                subagent_id: Some("sub1".into()),
            },
            Some("s1"),
            now,
        );
        let fx = m.handle(HookEvent::Stop, Some("s1"), now + Duration::from_secs(5));
        assert_eq!(status_of(&fx), None, "stays running — no transition");
        assert_eq!(m.status(), AgentStatus::Running);

        // Subagent finishes; drain grace must elapse before finished.
        m.handle(
            HookEvent::SubagentStop {
                subagent_id: Some("sub1".into()),
            },
            Some("s1"),
            now + Duration::from_secs(60),
        );
        let fx = m.tick(now + Duration::from_secs(61));
        assert!(fx.is_empty(), "grace not elapsed yet");
        let fx = m.tick(now + Duration::from_secs(61) + DRAIN_GRACE);
        assert_eq!(status_of(&fx), Some(AgentStatus::Finished));
    }

    #[test]
    fn subagent_start_shortly_after_finish_heals_to_running() {
        let mut m = AgentStatusMachine::new(AgentStatus::Fresh, None);
        let now = t0();
        m.handle(HookEvent::UserPromptSubmit, Some("s1"), now);
        m.handle(HookEvent::Stop, Some("s1"), now + Duration::from_secs(10));
        assert_eq!(m.status(), AgentStatus::Finished);
        // The subagent's own POST arrives 2s later — race heal.
        let fx = m.handle(
            HookEvent::SubagentStart {
                subagent_id: Some("sub1".into()),
            },
            Some("s1"),
            now + Duration::from_secs(12),
        );
        assert_eq!(status_of(&fx), Some(AgentStatus::Running));
    }

    #[test]
    fn post_turn_helper_outside_window_does_not_heal() {
        let mut m = AgentStatusMachine::new(AgentStatus::Fresh, None);
        let now = t0();
        m.handle(HookEvent::UserPromptSubmit, Some("s1"), now);
        m.handle(HookEvent::Stop, Some("s1"), now + Duration::from_secs(10));
        // An away-summary helper fires SubagentStart minutes later.
        let fx = m.handle(
            HookEvent::SubagentStart {
                subagent_id: Some("helper".into()),
            },
            Some("s1"),
            now + Duration::from_secs(10) + RECENT_FINISH_WINDOW + Duration::from_secs(1),
        );
        assert!(fx.is_empty(), "must stay finished: {fx:?}");
        assert_eq!(m.status(), AgentStatus::Finished);
    }

    fn progress(m: &mut AgentStatusMachine, busy: bool, now: Instant) -> Vec<Effect> {
        m.handle(HookEvent::Progress { busy }, None, now)
    }

    #[test]
    fn progress_idle_finishes_a_cancelled_turn() {
        // The reported bug: the user hits escape. Claude Code fires no Stop
        // for an interrupted turn and never sends `idle_prompt` either (it
        // suppresses that when the user has just touched the keyboard), so
        // the only news is the progress bar clearing.
        let mut m = AgentStatusMachine::new(AgentStatus::Fresh, None);
        let now = t0();
        m.handle(HookEvent::UserPromptSubmit, Some("s1"), now);
        progress(&mut m, true, now);
        assert_eq!(m.status(), AgentStatus::Running);
        let fx = progress(&mut m, false, now + Duration::from_secs(8));
        assert_eq!(status_of(&fx), Some(AgentStatus::Finished));
    }

    #[test]
    fn progress_idle_clears_a_rejected_permission_prompt() {
        // Escaping out of a permission prompt: same story, from red.
        let mut m = AgentStatusMachine::new(AgentStatus::Fresh, None);
        let now = t0();
        m.handle(HookEvent::UserPromptSubmit, Some("s1"), now);
        m.handle(
            HookEvent::PermissionRequest { subagent_id: None },
            Some("s1"),
            now,
        );
        assert_eq!(m.status(), AgentStatus::NeedsFeedback);
        let fx = progress(&mut m, false, now + Duration::from_secs(5));
        assert_eq!(status_of(&fx), Some(AgentStatus::Finished));
    }

    #[test]
    fn progress_idle_leaves_fresh_and_dead_agents_alone() {
        // Every CLI clears its progress bar at startup and again on exit;
        // neither is a finished turn.
        for start in [
            AgentStatus::Fresh,
            AgentStatus::Finished,
            AgentStatus::Terminated,
            AgentStatus::Disconnected,
        ] {
            let mut m = AgentStatusMachine::new(start, Some("s1".into()));
            let fx = progress(&mut m, false, t0());
            assert!(fx.is_empty(), "{start:?} must not be touched: {fx:?}");
            assert_eq!(m.status(), start);
        }
    }

    #[test]
    fn progress_busy_starts_a_turn_but_never_talks_over_feedback() {
        // A turn nebula saw no prompt for (resumed session, scheduled wake).
        let mut m = AgentStatusMachine::new(AgentStatus::Fresh, Some("s1".into()));
        let now = t0();
        let fx = progress(&mut m, true, now);
        assert_eq!(status_of(&fx), Some(AgentStatus::Running));

        // …but a pending question outranks it, and the dead stay dead.
        for start in [
            AgentStatus::NeedsFeedback,
            AgentStatus::Terminated,
            AgentStatus::Disconnected,
        ] {
            let mut m = AgentStatusMachine::new(start, Some("s1".into()));
            let fx = progress(&mut m, true, now);
            assert!(fx.is_empty(), "{start:?} must not be touched: {fx:?}");
            assert_eq!(m.status(), start);
        }
    }

    #[test]
    fn progress_idle_respects_the_subagent_drain_hold() {
        // The progress bar clears when the *main loop* parks, which on a
        // normal turn end beats the Stop hook's HTTP round-trip. It must not
        // finish out from under still-running subagents.
        let mut m = AgentStatusMachine::new(AgentStatus::Fresh, None);
        let now = t0();
        m.handle(HookEvent::UserPromptSubmit, Some("s1"), now);
        m.handle(
            HookEvent::SubagentStart {
                subagent_id: Some("sub1".into()),
            },
            Some("s1"),
            now,
        );
        let fx = progress(&mut m, false, now + Duration::from_secs(5));
        assert_eq!(status_of(&fx), None, "stays running — no transition");
        assert_eq!(m.status(), AgentStatus::Running);
        // The Stop that follows a beat later agrees, and the drain still owns
        // the promotion.
        let fx = m.handle(HookEvent::Stop, Some("s1"), now + Duration::from_secs(5));
        assert!(fx.is_empty());
        m.handle(
            HookEvent::SubagentStop {
                subagent_id: Some("sub1".into()),
            },
            Some("s1"),
            now + Duration::from_secs(6),
        );
        let fx = m.tick(now + Duration::from_secs(7));
        assert!(fx.is_empty(), "grace not elapsed yet");
        let fx = m.tick(now + Duration::from_secs(7) + DRAIN_GRACE);
        assert_eq!(status_of(&fx), Some(AgentStatus::Finished));
    }

    #[test]
    fn progress_idle_preserves_the_subagent_race_heal() {
        // Progress clears just before the Stop hook lands. Both stamp
        // `finished_at`, so a subagent POST that raced the Stop still heals —
        // unlike `mark_idle`, which deliberately blocks healing.
        let mut m = AgentStatusMachine::new(AgentStatus::Fresh, None);
        let now = t0();
        m.handle(HookEvent::UserPromptSubmit, Some("s1"), now);
        progress(&mut m, false, now + Duration::from_secs(10));
        m.handle(HookEvent::Stop, Some("s1"), now + Duration::from_secs(10));
        assert_eq!(m.status(), AgentStatus::Finished);
        let fx = m.handle(
            HookEvent::SubagentStart {
                subagent_id: Some("sub1".into()),
            },
            Some("s1"),
            now + Duration::from_secs(12),
        );
        assert_eq!(status_of(&fx), Some(AgentStatus::Running));
    }

    #[test]
    fn progress_edges_across_a_whole_turn_settle_on_finished() {
        // The captured Claude Code 2.1.241 sequence, hooks and all.
        let mut m = AgentStatusMachine::new(AgentStatus::Fresh, None);
        let now = t0();
        progress(&mut m, false, now); // startup, parked at the input box
        assert_eq!(m.status(), AgentStatus::Fresh);
        m.handle(HookEvent::UserPromptSubmit, Some("s1"), now);
        progress(&mut m, true, now);
        m.handle(
            HookEvent::PermissionRequest { subagent_id: None },
            Some("s1"),
            now + Duration::from_secs(3),
        );
        assert_eq!(m.status(), AgentStatus::NeedsFeedback);
        // Approving does not move the progress bar — it never left "busy".
        // The gated tool's PostToolUse is the approval.
        let fx = m.handle(
            HookEvent::PostToolUse {
                tool_name: Some("Bash".into()),
                subagent_id: None,
            },
            Some("s1"),
            now + Duration::from_secs(19),
        );
        assert_eq!(status_of(&fx), Some(AgentStatus::Running));
        let fx = progress(&mut m, false, now + Duration::from_secs(20));
        assert_eq!(status_of(&fx), Some(AgentStatus::Finished));
    }

    #[test]
    fn foreign_session_events_are_ignored() {
        let mut m = AgentStatusMachine::new(AgentStatus::Fresh, None);
        let now = t0();
        m.handle(HookEvent::UserPromptSubmit, Some("s1"), now);
        // A manually-launched claude in the same cwd posts a Stop.
        let fx = m.handle(HookEvent::Stop, Some("other-session"), now);
        assert!(fx.is_empty());
        assert_eq!(m.status(), AgentStatus::Running);
    }

    #[test]
    fn new_session_id_on_capture_adopts_and_clears_subagents() {
        let mut m = AgentStatusMachine::new(AgentStatus::Fresh, None);
        let now = t0();
        m.handle(HookEvent::UserPromptSubmit, Some("s1"), now);
        m.handle(
            HookEvent::SubagentStart {
                subagent_id: Some("sub1".into()),
            },
            Some("s1"),
            now,
        );
        // claude restarted (new session id) and a fresh prompt arrives.
        let fx = m.handle(
            HookEvent::UserPromptSubmit,
            Some("s2"),
            now + Duration::from_secs(5),
        );
        assert!(fx.contains(&Effect::SaveSessionId("s2".into())));
        // Old subagents gone: a Stop finishes immediately.
        let fx = m.handle(HookEvent::Stop, Some("s2"), now + Duration::from_secs(6));
        assert_eq!(status_of(&fx), Some(AgentStatus::Finished));
    }

    #[test]
    fn pty_death_while_running_terminates_and_blocks_heal() {
        let mut m = AgentStatusMachine::new(AgentStatus::Fresh, None);
        let now = t0();
        m.handle(HookEvent::UserPromptSubmit, Some("s1"), now);
        let fx = m.handle(
            HookEvent::SessionEnded {
                exit_code: Some(137),
            },
            None,
            now,
        );
        assert_eq!(status_of(&fx), Some(AgentStatus::Terminated));
        // Laggard subagent POST must not resurrect the dead agent.
        let fx = m.handle(
            HookEvent::SubagentStart {
                subagent_id: Some("sub1".into()),
            },
            Some("s1"),
            now + Duration::from_secs(1),
        );
        assert!(fx.is_empty());
        assert_eq!(m.status(), AgentStatus::Terminated);
    }

    #[test]
    fn pty_clean_exit_while_running_is_finished() {
        let mut m = AgentStatusMachine::new(AgentStatus::Fresh, None);
        let now = t0();
        m.handle(HookEvent::UserPromptSubmit, Some("s1"), now);
        let fx = m.handle(HookEvent::SessionEnded { exit_code: Some(0) }, None, now);
        assert_eq!(status_of(&fx), Some(AgentStatus::Finished));
    }

    #[test]
    fn pty_exit_when_already_finished_keeps_finished() {
        let mut m = AgentStatusMachine::new(AgentStatus::Fresh, None);
        let now = t0();
        m.handle(HookEvent::UserPromptSubmit, Some("s1"), now);
        m.handle(HookEvent::Stop, Some("s1"), now);
        let fx = m.handle(HookEvent::SessionEnded { exit_code: Some(1) }, None, now);
        assert!(
            fx.is_empty(),
            "finished agent whose pty closes stays finished"
        );
        assert_eq!(m.status(), AgentStatus::Finished);
    }

    #[test]
    fn anon_and_keyed_subagent_cross_cancel() {
        let mut m = AgentStatusMachine::new(AgentStatus::Fresh, None);
        let now = t0();
        m.handle(HookEvent::UserPromptSubmit, Some("s1"), now);
        // Keyed start, anon stop → biases toward finishing.
        m.handle(
            HookEvent::SubagentStart {
                subagent_id: Some("sub1".into()),
            },
            Some("s1"),
            now,
        );
        m.handle(
            HookEvent::SubagentStop { subagent_id: None },
            Some("s1"),
            now,
        );
        let fx = m.handle(HookEvent::Stop, Some("s1"), now + Duration::from_secs(1));
        assert_eq!(
            status_of(&fx),
            Some(AgentStatus::Finished),
            "set drained via cross-cancel"
        );
    }

    #[test]
    fn clear_source_session_start_clears_subagents() {
        let mut m = AgentStatusMachine::new(AgentStatus::Fresh, None);
        let now = t0();
        m.handle(HookEvent::UserPromptSubmit, Some("s1"), now);
        m.handle(
            HookEvent::SubagentStart {
                subagent_id: Some("sub1".into()),
            },
            Some("s1"),
            now,
        );
        m.handle(
            HookEvent::SessionStart {
                source: Some("clear".into()),
            },
            Some("s1"),
            now,
        );
        let fx = m.handle(HookEvent::Stop, Some("s1"), now + Duration::from_secs(1));
        assert_eq!(status_of(&fx), Some(AgentStatus::Finished));
    }
}
