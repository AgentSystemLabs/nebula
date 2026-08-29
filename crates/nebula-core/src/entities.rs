use crate::ids::{AgentId, LinkId, ProjectId, TerminalId, WorkspaceId, WorktreeId};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentStatus {
    /// Never run yet (gray).
    Fresh,
    /// Actively working (yellow).
    Running,
    /// Turn complete (green).
    Finished,
    /// Waiting on the user: permission prompt or question (red).
    NeedsFeedback,
    /// Process died with a nonzero exit while working.
    Terminated,
    /// Daemon restarted while the agent was live; PTY is gone.
    Disconnected,
}

impl AgentStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            AgentStatus::Fresh => "fresh",
            AgentStatus::Running => "running",
            AgentStatus::Finished => "finished",
            AgentStatus::NeedsFeedback => "needs_feedback",
            AgentStatus::Terminated => "terminated",
            AgentStatus::Disconnected => "disconnected",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "fresh" => AgentStatus::Fresh,
            "running" => AgentStatus::Running,
            "finished" => AgentStatus::Finished,
            "needs_feedback" => AgentStatus::NeedsFeedback,
            "terminated" => AgentStatus::Terminated,
            "disconnected" => AgentStatus::Disconnected,
            _ => return None,
        })
    }
}

/// Which agent CLI a session runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum AgentKind {
    #[default]
    Claude,
    Codex,
    Cursor,
}

impl AgentKind {
    /// Every kind, for callers that must cover all of them (menus, the
    /// boot-time CLI probe warm) and should fail to compile if one is added.
    pub const ALL: [AgentKind; 3] = [AgentKind::Claude, AgentKind::Codex, AgentKind::Cursor];

    pub fn as_str(&self) -> &'static str {
        match self {
            AgentKind::Claude => "claude",
            AgentKind::Codex => "codex",
            AgentKind::Cursor => "cursor",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "claude" => AgentKind::Claude,
            "codex" => AgentKind::Codex,
            "cursor" => AgentKind::Cursor,
            _ => return None,
        })
    }

    /// Binary the kind launches. Differs from `as_str` only for Cursor,
    /// whose agent CLI ships as `cursor-agent` (`cursor` opens the editor).
    pub fn cli_program(&self) -> &'static str {
        match self {
            AgentKind::Claude => "claude",
            AgentKind::Codex => "codex",
            AgentKind::Cursor => "cursor-agent",
        }
    }
}

/// A named group of projects. Each nebula instance has exactly one
/// workspace open and shows only that workspace's projects; the daemon
/// remembers the last one opened as the workspace a fresh instance boots
/// into, not as a scope every client shares.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Workspace {
    pub id: WorkspaceId,
    pub name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Project {
    pub id: ProjectId,
    pub name: String,
    /// The workspace this project lives in. Defaults to the built-in
    /// `default` workspace for rows that predate workspaces.
    #[serde(default)]
    pub workspace_id: WorkspaceId,
    pub repo_path: PathBuf,
    pub sort_order: i64,
}

impl Project {
    /// The name a project takes from disk: the last component of its repo
    /// path. This is the default `name`, and it stays the truth about where
    /// the project lives no matter what the row is later renamed to.
    pub fn folder_name(repo_path: &Path) -> String {
        repo_path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "project".into())
    }

    /// The folder name to show beneath a renamed row, or None while the row
    /// still carries the folder's own name and repeating it would be noise.
    pub fn folder_subtitle(&self) -> Option<String> {
        let folder = Self::folder_name(&self.repo_path);
        (folder != self.name).then_some(folder)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Worktree {
    pub id: WorktreeId,
    pub project_id: ProjectId,
    pub path: PathBuf,
    pub branch: String,
    pub is_main: bool,
    pub sort_order: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Agent {
    pub id: AgentId,
    pub worktree_id: WorktreeId,
    pub name: String,
    pub status: AgentStatus,
    pub archived: bool,
    /// Epoch ms of the last archive; 0 = never archived (or archived before
    /// this field existed). Orders the ARCHIVED group newest-first.
    #[serde(default)]
    pub archived_at: i64,
    /// Finished a turn (running or needs-feedback → finished) that no client
    /// has looked at since. The Projects and Worktrees rows count these so
    /// the user knows how many terminals to go read; the pane landing on
    /// the session clears it (`ClientRequest::MarkAgentSeen`). Only ever
    /// true on a finished, unarchived row — leaving `finished` clears it.
    #[serde(default)]
    pub unseen: bool,
    /// Epoch ms of the last status change; 0 = unknown (pre-upgrade rows or
    /// never-run agents). Drives the TUI's RECENT session group.
    #[serde(default)]
    pub status_changed_at: i64,
    #[serde(default)]
    pub kind: AgentKind,
    /// Model the CLI is launched with (claude `--model` / codex `-m`);
    /// None = the CLI's own default. Persisted so respawns keep it.
    #[serde(default)]
    pub model: Option<String>,
    /// Reasoning effort the CLI is launched with (claude `--effort` /
    /// codex `model_reasoning_effort`); None = the CLI's own default.
    #[serde(default)]
    pub effort: Option<String>,
    /// CLI session id used for resume (claude, codex, or cursor, per `kind`).
    pub session_id: Option<String>,
    /// The Claude Cloud session this row launched (`claude --cloud <task>`
    /// prints the id as it creates one). Only cloud rows have it. Restarting
    /// such a row while it has no local `session_id` re-enters the cloud
    /// session — `claude --cloud <id>`, or `claude --teleport <id>` when the
    /// account cannot attach — instead of booting a bare local CLI.
    #[serde(default)]
    pub cloud_session_id: Option<String>,
    pub sort_order: i64,
    /// True when the daemon currently holds a live PTY for this agent.
    pub alive: bool,
    /// True while the daemon is following this row's Claude Cloud session —
    /// re-teleporting the pane on a timer so turns taken in the cloud show
    /// up here. Runtime state like `alive`, never persisted: it ends the
    /// moment the pane is typed into (the session is then the user's) and
    /// does not survive a daemon restart.
    #[serde(default)]
    pub cloud_mirroring: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TerminalTab {
    pub id: TerminalId,
    pub worktree_id: WorktreeId,
    pub name: String,
    pub sort_order: i64,
    /// True when the daemon currently holds a live PTY for this terminal.
    pub alive: bool,
}

/// A URL pinned to a worktree — the pull request, the ticket, the design
/// doc for whatever that checkout is for. Nebula never fetches these; they
/// are bookmarks the user opens in a browser from the Sessions panel. The
/// open pull request shown above them is discovered from git, not stored
/// here (see the TUI's `PullRequest`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Link {
    pub id: LinkId,
    pub worktree_id: WorktreeId,
    /// Always http(s) — normalized on the way in, so opening one can never
    /// hand the OS a scheme the user didn't intend.
    pub url: String,
    pub sort_order: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Entity {
    Workspace(Workspace),
    Project(Project),
    Worktree(Worktree),
    Agent(Agent),
    Terminal(TerminalTab),
    Link(Link),
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum EntityId {
    Workspace(WorkspaceId),
    Project(ProjectId),
    Worktree(WorktreeId),
    Agent(AgentId),
    Terminal(TerminalId),
    Link(LinkId),
}
