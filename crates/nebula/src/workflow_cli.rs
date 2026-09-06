//! Prototype workflow CLI. The DAEMON owns all scheduling and durable state.

mod output;

use crate::workflow_config::{self, read_text};
use anyhow::{bail, ensure, Context, Result};
use clap::{Args, Subcommand};
use nebula_core::{
    codec::{read_frame, write_frame},
    workflow::*,
    AgentId, ClientRequest, ServerEvent,
};
use std::{path::PathBuf, time::Duration};

#[derive(Args)]
#[command(
    after_help = "Examples:\n  nebula workflow start \"fix the login redirect\"\n  nebula workflow list\n  nebula workflow status <run-id> --json"
)]
pub(crate) struct WorkflowCli {
    /// Print the full structured result, including stored stage artifacts.
    #[arg(long, global = true)]
    json: bool,
    #[command(subcommand)]
    command: WorkflowCommand,
}

#[derive(Subcommand)]
enum WorkflowCommand {
    /// List this checkout's workflow definitions.
    ///
    /// Reads .nebula/workflows locally, including invalid definitions and
    /// their errors. Does not start a DAEMON or an AGENT.
    #[command(after_help = "Example:\n  nebula workflow catalog --json")]
    Catalog,
    /// Preview a workflow's resolved AGENTS and instructions.
    ///
    /// Resolves AGENT references and MODEL / EFFORT defaults locally. Without
    /// a selector, uses default.toml, the sole definition, or legacy JSON.
    #[command(
        after_help = "Examples:\n  nebula workflow inspect default\n  nebula workflow inspect --definition task.toml"
    )]
    Inspect {
        /// Filename selector from .nebula/workflows (without .toml).
        workflow: Option<String>,
        /// Read an explicit TOML or legacy JSON workflow.
        #[arg(long, conflicts_with = "workflow")]
        definition: Option<PathBuf>,
    },
    /// Start an ordered workflow from main.
    ///
    /// Run inside a NEBULA AGENT in the ROOT WORKTREE on main. Creates a new
    /// WORKTREE and queues the first stage without moving the caller.
    #[command(after_help = "Example:\n  nebula workflow start \"fix the login redirect\"")]
    Start {
        /// Task to execute; use quotes for multiple words.
        #[arg(required_unless_present = "task_file", conflicts_with = "task_file")]
        task: Option<String>,
        /// Read the task from a UTF-8 file.
        #[arg(long)]
        task_file: Option<PathBuf>,
        /// Filename selector from .nebula/workflows (without .toml).
        #[arg(long, conflicts_with = "definition")]
        workflow: Option<String>,
        /// Read an explicit TOML or legacy JSON workflow.
        #[arg(long)]
        definition: Option<PathBuf>,
    },
    /// Inspect a workflow and its stage results.
    ///
    /// Without an id, finds the run assigned to this NEBULA AGENT. Add --json
    /// to read the task, frozen definition, and every stored stage artifact.
    #[command(
        after_help = "Examples:\n  nebula workflow status <run-id>\n  nebula workflow status --json"
    )]
    Status { id: Option<String> },
    /// List workflow runs, most recently updated first.
    ///
    /// Shows summaries across this DAEMON; status --json reads one run in full.
    #[command(after_help = "Example:\n  nebula workflow list")]
    List,
    /// Record the current AGENT's stage result.
    ///
    /// The assigned AGENT runs this as its last tool call. A completed result
    /// requires a Markdown artifact; the DAEMON also waits for FINISHED.
    #[command(
        override_usage = "nebula workflow report [OPTIONS]\n       --stage <STAGE> --outcome <OUTCOME>\n       --summary <SUMMARY>",
        after_help = "Example:\n  nebula workflow report --stage planner \\\n    --outcome completed --file plan.md \\\n    --summary \"Plan ready\""
    )]
    Report {
        /// Stage id from the frozen definition.
        #[arg(long)]
        stage: String,
        /// Whether this stage completed or needs intervention.
        #[arg(long, value_parser = ["completed", "blocked"])]
        outcome: String,
        /// Short result or the exact reason work is blocked.
        #[arg(long)]
        summary: String,
        /// Result file, copied into the SQLITE STORE (required for completed).
        #[arg(long)]
        file: Option<PathBuf>,
    },
    /// Pause stage scheduling.
    ///
    /// Leaves the active SESSION running. No subsequent stage starts until resume.
    #[command(after_help = "Example:\n  nebula workflow pause <run-id>")]
    Pause { id: String },
    /// Resume a paused or blocked workflow.
    ///
    /// Renews the current stage's timeout. Resolve any reported blocker first;
    /// reopen a stopped SESSION in NEBULA. Never creates a replacement SESSION.
    #[command(after_help = "Example:\n  nebula workflow resume <run-id>")]
    Resume { id: String },
}

fn caller() -> Result<AgentId> {
    nebula_core::env::non_empty(nebula_core::env::AGENT_ID)
        .map(AgentId)
        .context("run this command inside a NEBULA AGENT SESSION")
}

impl WorkflowCli {
    pub(crate) fn run(self) -> Result<()> {
        let op = match self.command {
            WorkflowCommand::Catalog => {
                return output::catalog(
                    workflow_config::catalog(
                        &std::env::current_dir()?,
                        &nebula_tui::config::Config::load(),
                    )?,
                    self.json,
                );
            }
            WorkflowCommand::Inspect {
                workflow,
                definition,
            } => {
                return output::inspect(
                    workflow_config::load(
                        &std::env::current_dir()?,
                        workflow.as_deref(),
                        definition.as_deref(),
                        &nebula_tui::config::Config::load(),
                    )?,
                    self.json,
                );
            }
            WorkflowCommand::Start {
                task,
                task_file,
                workflow,
                definition,
            } => {
                let caller = caller()?;
                let task = match task_file {
                    Some(path) => read_text(&path, 8000)?,
                    None => task.context("task required")?,
                };
                let definition = workflow_config::load(
                    &std::env::current_dir()?,
                    workflow.as_deref(),
                    definition.as_deref(),
                    &nebula_tui::config::Config::load(),
                )?
                .definition;
                WorkflowOp::Start {
                    caller,
                    task,
                    definition,
                }
            }
            WorkflowCommand::Status { id } => WorkflowOp::Status {
                id,
                caller: caller().ok(),
            },
            WorkflowCommand::List => WorkflowOp::List,
            WorkflowCommand::Pause { id } => WorkflowOp::Pause { id },
            WorkflowCommand::Resume { id } => WorkflowOp::Resume { id },
            WorkflowCommand::Report {
                stage,
                outcome,
                summary,
                file,
            } => {
                let outcome = if outcome == "completed" {
                    StageOutcome::Completed
                } else {
                    StageOutcome::Blocked
                };
                ensure!(
                    outcome != StageOutcome::Completed || file.is_some(),
                    "completed requires --file"
                );
                let artifact = file
                    .map(|p| read_text(&p, 64 * 1024))
                    .transpose()?
                    .unwrap_or_default();
                WorkflowOp::Report {
                    caller: caller()?,
                    stage,
                    result: StageResult {
                        outcome,
                        summary,
                        artifact,
                    },
                }
            }
        };
        let runtime = tokio::runtime::Runtime::new()?;
        let reply = runtime.block_on(async {
            tokio::time::timeout(Duration::from_secs(60), exchange(op))
                .await
                .context(
                    "workflow request timed out; inspect `nebula workflow list` before retrying",
                )?
        })?;
        output::reply(reply, self.json)
    }
}

async fn exchange(op: WorkflowOp) -> Result<WorkflowReply> {
    let mut connection = nebula_tui::ipc::connect_or_spawn().await?;
    write_frame(
        &mut connection.stream,
        &ClientRequest::Workflow { req_id: 1, op },
    )
    .await?;
    loop {
        match read_frame::<ServerEvent, _>(&mut connection.stream).await? {
            Some(ServerEvent::Workflow { req_id: 1, reply }) => return Ok(reply),
            Some(ServerEvent::Error { message, .. }) => bail!("{message}"),
            None => bail!("DAEMON closed before replying; inspect workflow list before retrying"),
            _ => {}
        }
    }
}
