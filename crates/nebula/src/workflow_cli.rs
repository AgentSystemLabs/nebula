//! Prototype workflow CLI. The DAEMON owns all scheduling and durable state.

use anyhow::{bail, ensure, Context, Result};
use clap::{Args, Subcommand};
use nebula_core::{
    codec::{read_frame, write_frame},
    workflow::*,
    AgentId, ClientRequest, ServerEvent,
};
use std::{
    io::Read,
    path::{Path, PathBuf},
    time::Duration,
};

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
        /// Versioned workflow definition with stage order and MODEL / EFFORT.
        #[arg(long, default_value = ".nebula/workflow.json")]
        definition: PathBuf,
    },
    /// Inspect a workflow and its stage results.
    ///
    /// Without an id, finds the run assigned to this NEBULA AGENT. Add --json
    /// to read the task, frozen definition, and every stored stage artifact.
    #[command(
        after_help = "Examples:\n  nebula workflow status <run-id>\n  nebula workflow status --json"
    )]
    Status { id: Option<String> },
    /// List the 50 most recently updated runs.
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

fn read_file(path: &Path, limit: u64) -> Result<String> {
    let mut value = String::new();
    std::fs::File::open(path)
        .with_context(|| format!("open {}", path.display()))?
        .take(limit + 1)
        .read_to_string(&mut value)?;
    ensure!(
        value.len() as u64 <= limit,
        "{} exceeds {limit} bytes",
        path.display()
    );
    Ok(value)
}

impl WorkflowCli {
    pub(crate) fn run(self) -> Result<()> {
        let op = match self.command {
            WorkflowCommand::Start {
                task,
                task_file,
                definition,
            } => {
                let caller = caller()?;
                let task = match task_file {
                    Some(path) => read_file(&path, 8000)?,
                    None => task.context("task required")?,
                };
                let mut definition: WorkflowDefinition =
                    serde_json::from_str(&read_file(&definition, 64 * 1024)?)?;
                let config = nebula_tui::config::Config::load();
                for stage in &mut definition.stages {
                    ensure!(
                        config.kind_enabled(stage.kind),
                        "{} is disabled in NEBULA",
                        stage.kind.as_str()
                    );
                    if stage.model.is_none() {
                        stage.model = config.default_model(stage.kind);
                    }
                    if stage.effort.is_none() {
                        stage.effort = config.default_effort(stage.kind);
                    }
                }
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
                    .map(|p| read_file(&p, 64 * 1024))
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
        if self.json {
            println!("{}", serde_json::to_string_pretty(&reply)?);
        } else {
            match reply {
                WorkflowReply::Run(run) => {
                    println!(
                        "Workflow: {}\nStatus: {:?}\n{}",
                        run.id, run.status, run.message
                    );
                    if let Some(worktree) = &run.worktree {
                        println!("WORKTREE: {}", worktree.path.display());
                    }
                    for (definition, stage) in run.definition.stages.iter().zip(&run.stages) {
                        println!(
                            "  {}: {:?} ({}, model {}, effort {}){}",
                            definition.id,
                            stage.status,
                            definition.kind.as_str(),
                            definition.model.as_deref().unwrap_or("default"),
                            definition.effort.as_deref().unwrap_or("default"),
                            stage
                                .agent
                                .as_ref()
                                .map(|id| format!(" SESSION {id}"))
                                .unwrap_or_default()
                        );
                    }
                }
                WorkflowReply::List(runs) => {
                    if runs.is_empty() {
                        println!("No workflows.");
                    }
                    for run in runs {
                        println!(
                            "{}  {:?}  {}/{}  {}",
                            run.id, run.status, run.current, run.total, run.message
                        );
                    }
                }
            }
        }
        Ok(())
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
