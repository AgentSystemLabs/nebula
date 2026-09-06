//! Prototype workflow definitions and durable run state. The DAEMON owns transitions.

use crate::{AgentId, AgentKind, ProjectId, Worktree};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkflowDefinition {
    pub version: u32,
    pub timeout_seconds: u64,
    pub stages: Vec<StageDefinition>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StageDefinition {
    pub id: String,
    pub kind: AgentKind,
    pub model: Option<String>,
    pub effort: Option<String>,
    pub instructions: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowStatus {
    Creating,
    Running,
    Waiting,
    Paused,
    Blocked,
    Completed,
}

impl WorkflowStatus {
    pub fn active(self) -> bool {
        matches!(self, Self::Creating | Self::Running | Self::Waiting)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StageStatus {
    Pending,
    Launching,
    Running,
    Completed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StageOutcome {
    Completed,
    Blocked,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StageResult {
    pub outcome: StageOutcome,
    pub summary: String,
    /// Markdown copied from the AGENT's result file; SQLite is authoritative.
    pub artifact: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StageRun {
    pub status: StageStatus,
    pub agent: Option<AgentId>,
    pub started_at: i64,
    pub result: Option<StageResult>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowRun {
    pub id: String,
    pub project: ProjectId,
    /// Retained as historical context even if the user deletes the WORKTREE.
    pub worktree: Option<Worktree>,
    pub branch: String,
    pub base: String,
    pub task: String,
    /// MODEL / EFFORT defaults are resolved and frozen at kickoff.
    pub definition: WorkflowDefinition,
    pub stages: Vec<StageRun>,
    pub current: usize,
    pub status: WorkflowStatus,
    pub message: String,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowSummary {
    pub id: String,
    pub branch: String,
    pub status: WorkflowStatus,
    pub current: usize,
    pub total: usize,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum WorkflowOp {
    Start {
        caller: AgentId,
        task: String,
        definition: WorkflowDefinition,
    },
    Status {
        id: Option<String>,
        caller: Option<AgentId>,
    },
    List,
    Report {
        caller: AgentId,
        stage: String,
        result: StageResult,
    },
    Pause {
        id: String,
    },
    Resume {
        id: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum WorkflowReply {
    Run(Box<WorkflowRun>),
    List(Vec<WorkflowSummary>),
}
