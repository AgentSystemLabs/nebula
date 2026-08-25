use crate::entities::{
    Agent, AgentKind, AgentStatus, Entity, EntityId, Link, Note, NoteOwner, Project, TerminalTab,
    Workspace, Worktree,
};
use crate::ids::{AgentId, LinkId, NoteId, ProjectId, TerminalId, WorkspaceId, WorktreeId};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Bump on any breaking change to these enums. The daemon refuses mismatched
/// clients; the client then offers a kill-and-restart of the old daemon.
pub const PROTOCOL_VERSION: u32 = 24;

/// Max IPC frame size (length prefix sanity bound).
pub const MAX_FRAME_LEN: u32 = 4 * 1024 * 1024;

/// Cloud tasks ultimately cross an OS argv boundary (twice: the login
/// shell's `-c` string and Claude's own argv). Leave ample room for shell
/// quoting expansion and the rest of the environment on every platform.
pub const MAX_CLOUD_PROMPT_BYTES: usize = 16 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SessionRef {
    Agent(AgentId),
    Terminal(TerminalId),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ClientRequest {
    Hello {
        protocol_version: u32,
    },
    /// Reply is one Snapshot, then deltas stream on this connection forever.
    Subscribe,

    // -- PTY plane --
    Attach {
        session: SessionRef,
        /// Resume point for gap-free re-attach; None = replay whole ring.
        from_seq: Option<u64>,
        cols: u16,
        rows: u16,
    },
    Detach {
        session: SessionRef,
    },
    Input {
        session: SessionRef,
        #[serde(with = "serde_bytes")]
        data: Vec<u8>,
    },
    Resize {
        session: SessionRef,
        cols: u16,
        rows: u16,
    },

    // -- entity CRUD (RPC-style; answered by Ack/Error with matching req_id) --
    /// Create a workspace. Does not open it — that stays a separate step.
    AddWorkspace {
        req_id: u64,
        name: String,
    },
    /// Delete a workspace. Refused while it still holds projects, or when it
    /// is the last workspace. Clients still scoped to it fall back to a
    /// surviving one when its EntityRemoved lands.
    RemoveWorkspace {
        req_id: u64,
        id: WorkspaceId,
    },
    RenameWorkspace {
        req_id: u64,
        id: WorkspaceId,
        name: String,
    },
    /// Scope THIS connection to `id`, and remember it as the workspace a
    /// fresh client opens into. Deliberately not broadcast: two nebula
    /// instances are two views, and switching in one must not drag the
    /// other along with it.
    OpenWorkspace {
        req_id: u64,
        id: WorkspaceId,
    },
    /// Add a project to the workspace this connection is scoped to (see
    /// OpenWorkspace) — the remembered default for a connection that never
    /// scoped itself, which is every one-shot `nebula add`.
    AddProject {
        req_id: u64,
        path: PathBuf,
        name: Option<String>,
        /// Create `path` (and `git init` it, per config) when it doesn't
        /// exist on disk. Set only after the user confirmed in the client.
        create_missing: bool,
    },
    RemoveProject {
        req_id: u64,
        id: ProjectId,
    },
    /// Move a project `delta` slots in the list (clamped at the edges).
    MoveProject {
        req_id: u64,
        id: ProjectId,
        delta: i64,
    },
    /// Set a group divider on a project row: presence and label.
    /// `before` targets the leading divider drawn above the row (only
    /// valid on the first project) instead of the one hanging below it.
    /// `present: false` removes it (label is dropped too).
    SetProjectDivider {
        req_id: u64,
        id: ProjectId,
        before: bool,
        present: bool,
        label: Option<String>,
    },
    /// Move the divider on project `id` (`before` picks which one) to the
    /// neighboring gap (sign of `delta`). No-op past the list's edges or
    /// when the destination gap already has a divider.
    MoveDivider {
        req_id: u64,
        id: ProjectId,
        before: bool,
        delta: i64,
    },
    CreateWorktree {
        req_id: u64,
        project: ProjectId,
        branch: String,
        base: Option<String>,
    },
    DeleteWorktree {
        req_id: u64,
        id: WorktreeId,
        force: bool,
    },
    /// Pin/unpin the worktree in the worktrees list (pure metadata).
    SetWorktreePinned {
        req_id: u64,
        id: WorktreeId,
        pinned: bool,
    },
    CreateAgent {
        req_id: u64,
        worktree: WorktreeId,
        name: String,
        kind: AgentKind,
        /// Model the CLI launches with; None = the CLI's own default.
        model: Option<String>,
        /// Reasoning effort the CLI launches with; None = the CLI's own default.
        effort: Option<String>,
        /// True when the user accepted the generated default name, marking
        /// the session eligible for one agent-driven auto-title (the CLI
        /// runs `nebula rename` on its first prompt).
        auto_title: bool,
        /// One-shot task for a fresh `claude --cloud <task>` launch. This is
        /// deliberately request-only: prompts are not persisted with Agent.
        #[serde(default)]
        cloud_prompt: Option<String>,
    },
    /// Fire-and-forget: pre-spawn an agent CLI for this (worktree, kind) so
    /// the next CreateAgent adopts an already-booted session. Sent the
    /// moment the user picks the kind, before they type the name. No reply;
    /// a missing CLI or failed spawn silently degrades to a cold spawn.
    PrewarmAgent {
        worktree: WorktreeId,
        kind: AgentKind,
        /// Must match the CreateAgent that follows or the warm session is
        /// discarded (a CLI booted with the wrong model can't be adopted).
        model: Option<String>,
        effort: Option<String>,
    },
    /// Fire-and-forget: pre-spawn every dead (non-archived) session under a
    /// worktree so attaching later replays an already-booted screen instead
    /// of watching a login shell + CLI boot. Sent once the worktree
    /// selection has rested (debounced client-side); already-alive sessions
    /// are untouched. No reply; a failed spawn degrades to today's lazy
    /// spawn-on-attach.
    PrewarmWorktreeSessions {
        worktree: WorktreeId,
        /// Pane size the sessions boot at, so the later Attach resizes to
        /// the same grid and full-screen apps need no reflow.
        cols: u16,
        rows: u16,
    },
    RenameAgent {
        req_id: u64,
        id: AgentId,
        name: String,
    },
    /// Agent-initiated one-shot title (`nebula rename` inside the session's
    /// CLI). Applies only while the session still awaits its auto-title;
    /// answered with Error (informational, not a fault) once a title —
    /// user- or agent-set — already sticks, so a user rename is never
    /// clobbered by a late or repeated agent attempt.
    AutoRenameAgent {
        req_id: u64,
        id: AgentId,
        name: String,
    },
    /// Re-home the agent row under another worktree of the same project.
    /// A live PTY is killed and respawned (resumed) in the new path — left
    /// running, its hooks would keep reporting the old checkout's cwd and
    /// the daemon would re-home the row right back.
    MoveAgent {
        req_id: u64,
        id: AgentId,
        worktree: WorktreeId,
    },
    /// Kills the PTY, sets archived=1.
    ArchiveAgent {
        req_id: u64,
        id: AgentId,
    },
    UnarchiveAgent {
        req_id: u64,
        id: AgentId,
    },
    /// Pin/unpin the agent in the sessions list (pure metadata; PTY untouched).
    SetAgentPinned {
        req_id: u64,
        id: AgentId,
        pinned: bool,
    },
    DeleteAgent {
        req_id: u64,
        id: AgentId,
    },
    /// Respawn; resumes the stored session id (`claude --resume` /
    /// `codex resume` / `cursor-agent --resume`) when one is stored.
    RestartAgent {
        req_id: u64,
        id: AgentId,
    },
    CreateTerminal {
        req_id: u64,
        worktree: WorktreeId,
        name: Option<String>,
    },
    CreateNote {
        req_id: u64,
        owner: NoteOwner,
        text: String,
    },
    /// Rewrite a note's text.
    UpdateNote {
        req_id: u64,
        id: NoteId,
        text: String,
    },
    SetNoteDone {
        req_id: u64,
        id: NoteId,
        done: bool,
    },
    DeleteNote {
        req_id: u64,
        id: NoteId,
    },
    /// Pin a URL to a worktree. `url` is normalized daemon-side (a bare
    /// `github.com/...` gains an https:// scheme) and refused if it can't be
    /// made into an http(s) URL.
    CreateLink {
        req_id: u64,
        worktree: WorktreeId,
        url: String,
    },
    /// Rewrite a link's URL (same normalization as CreateLink).
    UpdateLink {
        req_id: u64,
        id: LinkId,
        url: String,
    },
    DeleteLink {
        req_id: u64,
        id: LinkId,
    },
    RenameTerminal {
        req_id: u64,
        id: TerminalId,
        name: String,
    },
    CloseTerminal {
        req_id: u64,
        id: TerminalId,
    },

    /// Fire-and-forget opaque TUI blob (last selection etc.).
    SaveUiState {
        json: String,
    },

    /// Fire-and-forget: the user just opened this pull request, so
    /// everything up to `marker` has now been read.
    MarkPrSeen {
        url: String,
        marker: String,
    },

    /// One point-in-time memory reading — the daemon plus every live
    /// session's process subtree. Answered by `ServerEvent::Metrics` with
    /// the same req_id (not an Ack).
    GetMetrics {
        req_id: u64,
    },

    Shutdown,
}

/// How much of a pull request's conversation the user had already seen the
/// last time they opened it. `marker` is the newest thing anyone else had
/// posted at that moment, as GitHub's RFC 3339 stamp — those sort
/// lexicographically, so "arrived since" is a string compare and nebula
/// never has to consult a clock. Empty means the PR was opened while its
/// conversation was still empty.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PrSeen {
    pub url: String,
    pub marker: String,
}

/// Memory usage of one live session: the PTY child plus every descendant
/// (an agent CLI typically fans out into node workers, shells, MCP servers).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionMetrics {
    pub session: SessionRef,
    /// OS pid of the PTY child (the subtree's root).
    pub pid: u32,
    /// Resident set size summed over the whole subtree, bytes.
    pub rss_bytes: u64,
    /// Live processes in the subtree, the root included.
    pub procs: u32,
}

/// Daemon-side half of the metrics modal's data; the client stacks its own
/// RSS on top. Session subtrees are daemon descendants, so `daemon_rss_bytes`
/// counts the daemon process alone — the total stays double-count-free.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetricsSnapshot {
    pub daemon_pid: u32,
    pub daemon_rss_bytes: u64,
    /// Physical memory installed on the machine, bytes; 0 = unknown.
    pub system_total_bytes: u64,
    pub sessions: Vec<SessionMetrics>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ServerEvent {
    HelloOk {
        protocol_version: u32,
        daemon_pid: u32,
    },
    Incompatible {
        daemon_protocol_version: u32,
    },
    Snapshot {
        workspaces: Vec<Workspace>,
        /// The workspace to scope this client's project lists to: the
        /// last one opened anywhere, which is only ever a starting point —
        /// each client owns its scope from here (see OpenWorkspace).
        active_workspace: WorkspaceId,
        projects: Vec<Project>,
        worktrees: Vec<Worktree>,
        agents: Vec<Agent>,
        terminals: Vec<TerminalTab>,
        notes: Vec<Note>,
        links: Vec<Link>,
        /// How far the user has read into each pull request they've opened.
        pr_seen: Vec<PrSeen>,
        ui_state: Option<String>,
    },

    Ack {
        req_id: u64,
        created: Option<EntityId>,
    },
    Error {
        req_id: Option<u64>,
        message: String,
    },

    // -- deltas (pushed to all subscribers) --
    EntityUpserted {
        entity: Entity,
    },
    EntityRemoved {
        id: EntityId,
    },
    StatusChanged {
        agent: AgentId,
        status: AgentStatus,
        /// Epoch ms the change was stamped with (matches the persisted
        /// `status_changed_at`, so clients regroup consistently).
        changed_at: i64,
    },

    // -- PTY plane (only to clients attached to that session) --
    /// Ring replay on attach; client resets its parser before applying.
    Scrollback {
        session: SessionRef,
        base_seq: u64,
        #[serde(with = "serde_bytes")]
        data: Vec<u8>,
    },
    /// Live coalesced output. `seq` = byte offset of the first byte.
    Output {
        session: SessionRef,
        seq: u64,
        #[serde(with = "serde_bytes")]
        data: Vec<u8>,
    },
    SessionExited {
        session: SessionRef,
        exit_code: Option<i32>,
    },
    /// The child's kitty-keyboard-protocol flags changed (or, right after
    /// Scrollback on attach, the current value). 0 = legacy encoding.
    KittyFlags {
        session: SessionRef,
        flags: u8,
    },

    /// Reply to `ClientRequest::GetMetrics`.
    Metrics {
        req_id: u64,
        snapshot: MetricsSnapshot,
    },
}
