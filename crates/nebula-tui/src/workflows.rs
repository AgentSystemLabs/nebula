//! MANAGED WORKFLOW progress and display labels for the TUI.

use crate::app::App;
use nebula_core::{
    workflow::{StageStatus, WorkflowStatus, WorkflowSummary},
    Agent, Worktree, WorktreeId,
};

pub const ICON: &str = "◆";

pub struct Workflows {
    pub runs: Vec<WorkflowSummary>,
    pub show: bool,
    pub selected: Option<String>,
    pub width: u16,
    pub scroll: usize,
    pub anchor: Option<(String, usize)>,
}

impl Default for Workflows {
    fn default() -> Self {
        Self {
            runs: Vec::new(),
            show: false,
            selected: None,
            width: 34,
            scroll: 0,
            anchor: None,
        }
    }
}

impl App {
    pub fn workflow_rows(&self) -> Vec<&WorkflowSummary> {
        let mut rows: Vec<_> = self
            .workflows
            .runs
            .iter()
            .filter(|run| {
                self.tree
                    .projects
                    .iter()
                    .any(|p| p.id == run.project && self.tree.in_active_workspace(p))
            })
            .collect();
        rows.sort_by(|a, b| {
            (a.status == WorkflowStatus::Completed)
                .cmp(&(b.status == WorkflowStatus::Completed))
                .then_with(|| b.created_at.cmp(&a.created_at))
                .then_with(|| a.id.cmp(&b.id))
        });
        let unfinished = rows
            .iter()
            .take_while(|r| r.status != WorkflowStatus::Completed)
            .count();
        rows.truncate(unfinished + 10);
        rows
    }

    pub fn workflow_selection(&self) -> usize {
        self.workflow_rows()
            .iter()
            .position(|r| Some(&r.id) == self.workflows.selected.as_ref())
            .unwrap_or(0)
    }

    pub fn workflow_for_worktree(&self, id: &WorktreeId) -> Option<&WorkflowSummary> {
        self.workflows
            .runs
            .iter()
            .find(|run| run.worktree.as_ref() == Some(id))
    }

    pub fn worktree_label<'a>(&'a self, worktree: &'a Worktree) -> &'a str {
        self.workflow_for_worktree(&worktree.id)
            .map_or(&worktree.branch, |run| &run.title)
    }

    pub fn agent_label<'a>(&'a self, agent: &'a Agent) -> &'a str {
        if let Some(run) = self.workflow_for_worktree(&agent.worktree_id) {
            if let Some(stage) = run
                .stages
                .iter()
                .find(|s| s.agent.as_ref() == Some(&agent.id))
            {
                if agent.name == format!("workflow {} {}", run.id, stage.id) {
                    return &stage.id;
                }
            }
        }
        &agent.name
    }
}

pub fn status_label(status: WorkflowStatus) -> &'static str {
    match status {
        WorkflowStatus::Creating => "Creating",
        WorkflowStatus::Running => "Running",
        WorkflowStatus::Waiting => "Waiting",
        WorkflowStatus::Paused => "Paused",
        WorkflowStatus::Blocked => "Blocked",
        WorkflowStatus::Completed => "Completed",
    }
}

pub fn progress(run: &WorkflowSummary) -> String {
    let done = run
        .stages
        .iter()
        .filter(|s| s.status == StageStatus::Completed)
        .count();
    format!(
        "{done}/{} done · {} left",
        run.total,
        run.total.saturating_sub(done)
    )
}

pub fn current_step(run: &WorkflowSummary) -> String {
    if run.status == WorkflowStatus::Completed {
        return "All steps complete".into();
    }
    run.stages.get(run.current).map_or_else(
        || "Preparing".into(),
        |s| {
            format!(
                "{} {}/{} · {}",
                if s.status == StageStatus::Pending {
                    "Queued"
                } else {
                    "Step"
                },
                run.current + 1,
                run.total,
                s.id
            )
        },
    )
}

pub fn next_step(run: &WorkflowSummary) -> String {
    run.stages
        .get(run.current.saturating_add(1))
        .map_or_else(|| "Next: none".into(), |s| format!("Next: {}", s.id))
}
