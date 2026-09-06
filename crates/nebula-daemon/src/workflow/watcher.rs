//! DAEMON-owned scheduling and recovery of durable workflow checkpoints.

use super::{now_ms, prompts};
use crate::{
    git,
    registry::{CreateAgentSpec, Daemon},
};
use anyhow::{bail, ensure, Context, Result};
use nebula_core::{workflow::*, AgentStatus, EntityId, SessionRef};
use std::sync::Arc;

impl Daemon {
    /// One watcher, owned by the DAEMON. A restart reads the same durable
    /// checkpoints; it never assumes that a lost create reply meant failure.
    pub fn start_workflow_watcher(self: &Arc<Self>) {
        let daemon = self.clone();
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(std::time::Duration::from_secs(2));
            loop {
                tokio::select! {
                    _ = daemon.shutdown.cancelled() => break,
                    _ = tick.tick() => {}
                }
                if let Err(err) = daemon.tick_workflows().await {
                    tracing::warn!(error = %err, "workflow watcher failed");
                }
            }
        });
    }

    pub(crate) async fn tick_workflows(self: &Arc<Self>) -> Result<()> {
        for id in self.store.active_workflow_ids()? {
            let _guard = self.workflow_ops.lock().await;
            let mut run = self.store.workflow(&id)?;
            if !run.status.active() {
                continue;
            }
            let before = serde_json::to_string(&run)?;
            if let Err(err) = self.advance_workflow(&mut run).await {
                run.status = WorkflowStatus::Blocked;
                run.message = format!("{err:#}");
            }
            if before != serde_json::to_string(&run)? {
                self.persist_workflow(&mut run)?;
            }
        }
        Ok(())
    }

    async fn advance_workflow(self: &Arc<Self>, run: &mut WorkflowRun) -> Result<()> {
        let worktree = run
            .worktree
            .as_ref()
            .context("WORKTREE creation was interrupted; inspect the recorded branch")?;
        let live_worktree = self
            .store
            .get_worktree(&worktree.id)?
            .context("managed WORKTREE was deleted")?;
        ensure!(
            live_worktree.path == worktree.path && worktree.path.is_dir(),
            "managed WORKTREE moved or disappeared"
        );
        if run.current == run.stages.len() {
            run.status = WorkflowStatus::Completed;
            run.message = "all stages completed".into();
            return Ok(());
        }
        if run.stages[run.current].status == StageStatus::Pending {
            ensure!(
                git::current_branch(&worktree.path).await? == run.branch,
                "managed WORKTREE changed branches"
            );
            self.launch_workflow_stage(run).await?;
            return Ok(());
        }
        if run.stages[run.current].status == StageStatus::Launching {
            // Only recovery reaches this: persist the intent before spawning,
            // then find the single matching SESSION if the reply was lost.
            let name = prompts::session_name(run);
            let (_, _, agents, _) = self.store.load_tree()?;
            let matches: Vec<_> = agents
                .into_iter()
                .filter(|a| a.worktree_id == worktree.id && a.name == name)
                .collect();
            ensure!(
                matches.len() == 1,
                "stage launch is uncertain; inspect SESSIONS before retrying"
            );
            run.stages[run.current].agent = Some(matches[0].id.clone());
            run.stages[run.current].status = StageStatus::Running;
        }
        let stage = &mut run.stages[run.current];
        let id = stage
            .agent
            .as_ref()
            .context("stage has no assigned SESSION")?;
        let agent = self
            .store
            .get_agent(id)?
            .context("assigned SESSION was deleted")?;
        ensure!(
            !agent.archived && agent.worktree_id == worktree.id,
            "assigned SESSION was archived or relocated"
        );
        if let Some(result) = &stage.result {
            if result.outcome == StageOutcome::Blocked {
                run.status = WorkflowStatus::Blocked;
                run.message = result.summary.clone();
                return Ok(());
            }
            if agent.status == AgentStatus::Finished {
                ensure!(
                    git::current_branch(&worktree.path).await? == run.branch,
                    "managed WORKTREE changed branches"
                );
                stage.status = StageStatus::Completed;
                run.current += 1;
                run.status = if run.current == run.stages.len() {
                    WorkflowStatus::Completed
                } else {
                    WorkflowStatus::Running
                };
                run.message = format!("{} of {} stages completed", run.current, run.stages.len());
                return Ok(());
            }
        }
        ensure!(self.is_alive(&SessionRef::Agent(id.clone())) &&
            !matches!(agent.status, AgentStatus::Terminated | AgentStatus::Disconnected),
            "assigned SESSION stopped before handoff; reopen it and continue, then resume the workflow");
        ensure!(
            now_ms() - stage.started_at < (run.definition.timeout_seconds * 1000) as i64,
            "stage timed out; inspect its SESSION, then resume the workflow"
        );
        run.status = if agent.status == AgentStatus::NeedsFeedback {
            WorkflowStatus::Waiting
        } else {
            WorkflowStatus::Running
        };
        run.message = match (agent.status, stage.result.is_some()) {
            (AgentStatus::Finished, false) => {
                "SESSION is FINISHED; waiting for an explicit stage result".into()
            }
            (_, true) => "result recorded; waiting for SESSION to reach FINISHED".into(),
            _ => format!(
                "{}: {}",
                run.definition.stages[run.current].id,
                agent.status.as_str()
            ),
        };
        Ok(())
    }

    async fn launch_workflow_stage(self: &Arc<Self>, run: &mut WorkflowRun) -> Result<()> {
        run.stages[run.current].status = StageStatus::Launching;
        run.stages[run.current].started_at = now_ms();
        self.persist_workflow(run)?;
        let definition = &run.definition.stages[run.current];
        let created = self
            .create_agent(CreateAgentSpec {
                worktree: run
                    .worktree
                    .as_ref()
                    .context("missing WORKTREE")?
                    .id
                    .clone(),
                name: prompts::session_name(run),
                kind: definition.kind,
                model: definition.model.clone(),
                effort: definition.effort.clone(),
                auto_title: false,
                cloud_prompt: None,
                starting_prompt: Some(prompts::starting_prompt(run)?),
                pr_url: None,
            })
            .await?;
        let EntityId::Agent(id) = created else {
            bail!("unexpected create AGENT result");
        };
        run.stages[run.current].agent = Some(id);
        run.stages[run.current].status = StageStatus::Running;
        run.status = WorkflowStatus::Running;
        run.message = format!("{} SESSION started", definition.id);
        Ok(())
    }
}
