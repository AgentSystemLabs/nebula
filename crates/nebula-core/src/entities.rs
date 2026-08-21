use crate::ids::{AgentId, ProjectId, TerminalId, TodoId, WorktreeId};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Project {
    pub id: ProjectId,
    pub name: String,
    pub repo_path: PathBuf,
    pub sort_order: i64,
    /// Draw a group divider under this row. Dividers belong to list
    /// positions, not projects: reordering keeps them in place.
    pub divider_after: bool,
    /// Optional group label rendered inside the divider line.
    pub divider_label: Option<String>,
    /// Draw a group divider above this row. Only ever set on the first
    /// project — it is the list's leading divider, re-owned by whichever
    /// project is on top after a reorder.
    pub divider_before: bool,
    /// Optional group label for the leading divider.
    pub divider_before_label: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Worktree {
    pub id: WorktreeId,
    pub project_id: ProjectId,
    pub path: PathBuf,
    pub branch: String,
    pub is_main: bool,
    /// Pinned worktrees sort into their own PINNED group in the worktrees list.
    #[serde(default)]
    pub pinned: bool,
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
    /// Pinned agents sort into their own PINNED group in the sessions list.
    #[serde(default)]
    pub pinned: bool,
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
    pub sort_order: i64,
    /// True when the daemon currently holds a live PTY for this agent.
    pub alive: bool,
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

/// Who a todo list hangs off: a project (high-level notes spanning its
/// worktrees) or one worktree (notes for that checkout). The two are
/// separate lists — a project's notes never mix into its worktrees'.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum TodoOwner {
    Project(ProjectId),
    Worktree(WorktreeId),
}

/// One todo note.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Todo {
    pub id: TodoId,
    pub owner: TodoOwner,
    pub text: String,
    pub done: bool,
    pub sort_order: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Entity {
    Project(Project),
    Worktree(Worktree),
    Agent(Agent),
    Terminal(TerminalTab),
    Todo(Todo),
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum EntityId {
    Project(ProjectId),
    Worktree(WorktreeId),
    Agent(AgentId),
    Terminal(TerminalId),
    Todo(TodoId),
}
