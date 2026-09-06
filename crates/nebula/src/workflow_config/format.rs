//! TOML supports an AGENT filename or inline table; references resolve once at kickoff.

use super::{named_file, read_text, slug, ResolvedWorkflow};
use anyhow::{ensure, Context, Result};
use nebula_core::{
    workflow::{StageDefinition, WorkflowDefinition},
    AgentKind,
};
use serde::Deserialize;
use std::path::Path;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WorkflowFile {
    version: u32,
    name: Option<String>,
    #[serde(default)]
    description: String,
    timeout_seconds: u64,
    stages: Vec<StageFile>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StageFile {
    id: String,
    agent: AgentSource,
    model: Option<String>,
    effort: Option<String>,
    #[serde(default)]
    instructions: String,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum AgentSource {
    Reference(String),
    Inline(AgentFile),
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AgentFile {
    kind: AgentKind,
    model: Option<String>,
    effort: Option<String>,
    #[serde(default)]
    instructions: String,
}

pub(super) fn resolve(path: &Path, text: &str) -> Result<ResolvedWorkflow> {
    let file: WorkflowFile =
        toml::from_str(text).with_context(|| format!("parse {}", path.display()))?;
    let id = path
        .file_stem()
        .and_then(|s| s.to_str())
        .context("workflow filename is not UTF-8")?;
    slug(id)?;
    ensure!(
        file.description.len() <= 2000
            && !file
                .description
                .chars()
                .any(|c| c.is_control() && !matches!(c, '\n' | '\r' | '\t')),
        "workflow description must be plain text of at most 2000 bytes"
    );
    ensure!(
        (1..=10).contains(&file.stages.len()),
        "workflow needs 1..10 stages"
    );
    let mut stages = Vec::new();
    let mut sources = Vec::new();
    for stage in file.stages {
        let (agent, source) = match stage.agent {
            AgentSource::Inline(agent) => (agent, None),
            AgentSource::Reference(reference) => {
                let root = path.ancestors().find(|p| p.file_name().is_some_and(|n| n == ".nebula"))
                    .context("referenced AGENTS require the workflow file to live under .nebula; use an inline agent for standalone files")?;
                let agent_path =
                    named_file(&root.join("agents"), &reference).with_context(|| {
                        format!("stage {} references AGENT {reference:?}", stage.id)
                    })?;
                let text = read_text(&agent_path, 16 * 1024)?;
                let agent: AgentFile = toml::from_str(&text).with_context(|| {
                    format!(
                        "parse AGENT {} for stage {}",
                        agent_path.display(),
                        stage.id
                    )
                })?;
                (agent, Some(agent_path.canonicalize()?))
            }
        };
        // AGENT instructions describe the reusable role; stage text adds task context.
        let instructions = [agent.instructions.trim(), stage.instructions.trim()]
            .into_iter()
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join("\n\n");
        stages.push(StageDefinition {
            id: stage.id,
            kind: agent.kind,
            model: stage.model.or(agent.model),
            effort: stage.effort.or(agent.effort),
            instructions,
        });
        sources.push(source);
    }
    Ok(ResolvedWorkflow {
        path: path.canonicalize()?,
        description: file.description,
        agent_sources: sources,
        definition: WorkflowDefinition {
            version: file.version,
            id: Some(id.to_string()),
            name: Some(file.name.unwrap_or_else(|| id.to_string())),
            timeout_seconds: file.timeout_seconds,
            stages,
        },
    })
}
