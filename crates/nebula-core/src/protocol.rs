use crate::entities::{Agent, AgentStatus, Entity, EntityId, Project, TerminalTab, Worktree};
use crate::ids::{AgentId, ProjectId, TerminalId, WorktreeId};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Bump on any breaking change to these enums. The daemon refuses mismatched
/// clients; the client then offers a kill-and-restart of the old daemon.
pub const PROTOCOL_VERSION: u32 = 2;

/// Max IPC frame size (length prefix sanity bound).
pub const MAX_FRAME_LEN: u32 = 4 * 1024 * 1024;

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
    AddProject { req_id: u64, path: PathBuf, name: Option<String> },
    RemoveProject { req_id: u64, id: ProjectId },
    CreateWorktree { req_id: u64, project: ProjectId, branch: String, base: Option<String> },
    DeleteWorktree { req_id: u64, id: WorktreeId, force: bool },
    CreateAgent { req_id: u64, worktree: WorktreeId, name: String },
    RenameAgent { req_id: u64, id: AgentId, name: String },
    /// Kills the PTY, sets archived=1.
    ArchiveAgent { req_id: u64, id: AgentId },
    UnarchiveAgent { req_id: u64, id: AgentId },
    DeleteAgent { req_id: u64, id: AgentId },
    /// Respawn; uses `claude --resume` when a session id is stored.
    RestartAgent { req_id: u64, id: AgentId },
    CreateTerminal { req_id: u64, worktree: WorktreeId, name: Option<String> },
    RenameTerminal { req_id: u64, id: TerminalId, name: String },
    CloseTerminal { req_id: u64, id: TerminalId },

    /// Fire-and-forget opaque TUI blob (last selection etc.).
    SaveUiState { json: String },

    Shutdown,
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
        projects: Vec<Project>,
        worktrees: Vec<Worktree>,
        agents: Vec<Agent>,
        terminals: Vec<TerminalTab>,
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
    EntityUpserted { entity: Entity },
    EntityRemoved { id: EntityId },
    StatusChanged { agent: AgentId, status: AgentStatus },

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
}
