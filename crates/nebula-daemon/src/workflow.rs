//! Prototype sequential workflows. Only the DAEMON writes run state or launches stages.

mod prompts;
#[cfg(test)]
mod tests;
mod watcher;

use anyhow::{bail, ensure, Context, Result};
use nebula_core::workflow::*;
use nebula_core::{AgentId, AgentStatus, EntityId, SessionRef};
use std::sync::Arc;

use crate::{git, registry::Daemon};

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

impl Daemon {
    pub async fn workflow_op(self: &Arc<Self>, op: WorkflowOp) -> Result<WorkflowReply> {
        let _guard = self.workflow_ops.lock().await;
        let run = match op {
            WorkflowOp::Start {
                caller,
                task,
                definition,
            } => self.start_workflow(&caller, task, definition).await?,
            WorkflowOp::Status { id, caller } => match (id, caller) {
                (Some(id), _) => self.store.workflow(&id)?,
                (None, Some(caller)) => self
                    .store
                    .workflow_for_agent(&caller)?
                    .context("this SESSION does not belong to a workflow")?,
                _ => bail!("name a workflow id, or run status inside a managed SESSION"),
            },
            WorkflowOp::List => return Ok(WorkflowReply::List(self.store.workflow_summaries()?)),
            WorkflowOp::Report {
                caller,
                stage,
                result,
            } => self.report_workflow(&caller, &stage, result)?,
            WorkflowOp::Pause { id } => {
                let mut run = self.store.workflow(&id)?;
                ensure!(
                    run.status != WorkflowStatus::Completed,
                    "workflow already completed"
                );
                run.status = WorkflowStatus::Paused;
                run.message = "scheduling paused; the current SESSION is left running".into();
                self.persist_workflow(&mut run)?;
                run
            }
            WorkflowOp::Resume { id } => {
                let mut run = self.store.workflow(&id)?;
                ensure!(
                    matches!(run.status, WorkflowStatus::Blocked | WorkflowStatus::Paused),
                    "only a blocked or paused workflow can resume"
                );
                ensure!(
                    run.worktree.is_some(),
                    "WORKTREE creation was interrupted; inspect it before starting another run"
                );
                let stage = &mut run.stages[run.current];
                if let Some(agent) = &stage.agent {
                    let row = self
                        .store
                        .get_agent(agent)?
                        .context("assigned SESSION was deleted")?;
                    ensure!(self.is_alive(&SessionRef::Agent(agent.clone())) ||
                        (row.status == AgentStatus::Finished && stage.result.as_ref()
                            .is_some_and(|r| r.outcome == StageOutcome::Completed)),
                        "assigned SESSION stopped; reopen it in NEBULA and continue its stage before resuming");
                }
                ensure!(!stage.result.as_ref().is_some_and(|r| r.outcome == StageOutcome::Blocked),
                    "the AGENT reported a blocker; resolve it in that SESSION and report completed first");
                stage.started_at = now_ms();
                run.status = WorkflowStatus::Running;
                run.message = "watcher resumed".into();
                self.persist_workflow(&mut run)?;
                run
            }
        };
        Ok(WorkflowReply::Run(Box::new(run)))
    }

    async fn start_workflow(
        self: &Arc<Self>,
        caller: &AgentId,
        task: String,
        definition: WorkflowDefinition,
    ) -> Result<WorkflowRun> {
        prompts::validate(&task, &definition)?;
        ensure!(
            self.store.workflow_for_agent(caller)?.is_none(),
            "a workflow AGENT cannot start a nested workflow"
        );
        let agent = self
            .store
            .get_agent(caller)?
            .context("caller SESSION not found")?;
        ensure!(!agent.archived, "caller SESSION is archived");
        let root = self
            .store
            .get_worktree(&agent.worktree_id)?
            .context("caller WORKTREE not found")?;
        ensure!(
            root.is_main && git::current_branch(&root.path).await? == "main",
            "start a workflow from the ROOT WORKTREE on main"
        );
        let id = ulid::Ulid::generate().to_string();
        let mut run = WorkflowRun {
            branch: format!("workflow-{}", id.to_lowercase()),
            id,
            project: root.project_id.clone(),
            worktree: None,
            base: git::main_commit(&root.path).await?,
            task,
            stages: definition
                .stages
                .iter()
                .map(|_| StageRun {
                    status: StageStatus::Pending,
                    agent: None,
                    started_at: 0,
                    result: None,
                })
                .collect(),
            definition,
            current: 0,
            status: WorkflowStatus::Creating,
            message: "creating WORKTREE".into(),
            created_at: now_ms(),
            updated_at: now_ms(),
        };
        self.store.save_workflow(&run)?;
        let created = self
            .create_worktree(&run.project, &run.branch, Some(&run.base))
            .await;
        match created {
            Ok(EntityId::Worktree(id)) => {
                run.worktree = self.store.get_worktree(&id)?;
                run.status = WorkflowStatus::Running;
                run.message = "WORKTREE created; first stage queued".into();
            }
            Ok(_) => bail!("unexpected create WORKTREE result"),
            Err(err) => {
                run.status = WorkflowStatus::Blocked;
                run.message = format!("WORKTREE creation failed: {err:#}");
            }
        }
        self.persist_workflow(&mut run)?;
        Ok(run)
    }

    fn persist_workflow(&self, run: &mut WorkflowRun) -> Result<()> {
        run.updated_at = now_ms();
        self.store.save_workflow(run)
    }

    fn report_workflow(
        &self,
        caller: &AgentId,
        stage_id: &str,
        result: StageResult,
    ) -> Result<WorkflowRun> {
        prompts::validate_result(&result)?;
        let mut run = self
            .store
            .workflow_for_agent(caller)?
            .context("SESSION is not managed by a workflow")?;
        ensure!(run.current < run.stages.len(), "workflow already completed");
        ensure!(
            run.definition.stages[run.current].id == stage_id,
            "report is for a different stage"
        );
        let stage = &mut run.stages[run.current];
        ensure!(
            stage.agent.as_ref() == Some(caller) && stage.status == StageStatus::Running,
            "only the assigned AGENT may report the current stage"
        );
        let agent = self
            .store
            .get_agent(caller)?
            .context("SESSION was deleted")?;
        ensure!(
            !agent.archived
                && run
                    .worktree
                    .as_ref()
                    .is_some_and(|w| w.id == agent.worktree_id),
            "SESSION was archived or relocated"
        );
        if let Some(previous) = &stage.result {
            if previous == &result {
                return Ok(run);
            }
            ensure!(
                previous.outcome != StageOutcome::Completed,
                "completed report cannot be overwritten"
            );
        }
        stage.result = Some(result);
        self.persist_workflow(&mut run)?;
        Ok(run)
    }
}
