use crate::workflow_config::{CatalogEntry, ResolvedWorkflow};
use anyhow::Result;
use nebula_core::workflow::WorkflowReply;

pub(super) fn catalog(entries: Vec<CatalogEntry>, json: bool) -> Result<()> {
    if json {
        println!("{}", serde_json::to_string_pretty(&entries)?);
    } else if entries.is_empty() {
        println!("No definitions. Add .nebula/workflows/default.toml.");
    } else {
        for entry in entries {
            if let Some(error) = entry.error {
                println!("{}: INVALID\n  {error}", entry.id);
            } else {
                println!(
                    "{}: {}\n  {}\n  {}",
                    entry.id,
                    entry.name.as_deref().unwrap_or("Legacy JSON"),
                    entry.description.as_deref().unwrap_or_default(),
                    entry.stages.join(" -> ")
                );
            }
        }
    }
    Ok(())
}

pub(super) fn inspect(resolved: ResolvedWorkflow, json: bool) -> Result<()> {
    if json {
        println!("{}", serde_json::to_string_pretty(&resolved)?);
        return Ok(());
    }
    let definition = &resolved.definition;
    println!(
        "Definition: {} ({})\nFile: {}\n{}\nTimeout: {} seconds per stage",
        definition.id.as_deref().unwrap_or("legacy"),
        definition.name.as_deref().unwrap_or("Legacy JSON"),
        resolved.path.display(),
        resolved.description,
        definition.timeout_seconds
    );
    for (index, stage) in definition.stages.iter().enumerate() {
        println!(
            "\n{}. {}: {} / {} / {}",
            index + 1,
            stage.id,
            stage.kind.as_str(),
            stage.model.as_deref().unwrap_or("provider default"),
            stage.effort.as_deref().unwrap_or("provider default")
        );
        match &resolved.agent_sources[index] {
            Some(path) => println!("   AGENT: {}", path.display()),
            None => println!("   AGENT: inline"),
        }
        for line in stage.instructions.lines() {
            println!("   {line}");
        }
    }
    Ok(())
}

pub(super) fn reply(reply: WorkflowReply, json: bool) -> Result<()> {
    if json {
        println!("{}", serde_json::to_string_pretty(&reply)?);
        return Ok(());
    }
    match reply {
        WorkflowReply::Run(run) => {
            println!(
                "Workflow: {}\nStatus: {:?}\n{}",
                run.id, run.status, run.message
            );
            if let Some(id) = &run.definition.id {
                println!(
                    "Definition: {id} ({})",
                    run.definition.name.as_deref().unwrap_or(id)
                );
            }
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
                    "{}  {}  {:?}  {}/{}  {}",
                    run.id,
                    run.workflow.as_deref().unwrap_or("legacy"),
                    run.status,
                    run.current,
                    run.total,
                    run.message
                );
            }
        }
    }
    Ok(())
}
