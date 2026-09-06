//! File discovery and resolution. The DAEMON receives a complete, frozen definition.

mod format;
#[cfg(test)]
mod tests;

use anyhow::{bail, ensure, Context, Result};
use nebula_core::workflow::WorkflowDefinition;
use nebula_tui::config::Config;
use serde::Serialize;
use std::{
    fs,
    io::Read,
    path::{Path, PathBuf},
};

#[derive(Debug, Serialize)]
pub(crate) struct ResolvedWorkflow {
    pub path: PathBuf,
    pub description: String,
    /// One source per stage: a canonical AGENT file path, or None for inline.
    pub agent_sources: Vec<Option<PathBuf>>,
    pub definition: WorkflowDefinition,
}

#[derive(Serialize)]
pub(crate) struct CatalogEntry {
    pub id: String,
    pub path: PathBuf,
    pub name: Option<String>,
    pub description: Option<String>,
    pub stages: Vec<String>,
    pub error: Option<String>,
}

pub(crate) fn read_text(path: &Path, limit: u64) -> Result<String> {
    let mut value = String::new();
    fs::File::open(path)
        .with_context(|| format!("open {}", path.display()))?
        .take(limit + 1)
        .read_to_string(&mut value)
        .with_context(|| format!("read UTF-8 from {}", path.display()))?;
    ensure!(
        value.len() as u64 <= limit,
        "{} exceeds {limit} bytes",
        path.display()
    );
    Ok(value)
}

fn config_dir(cwd: &Path) -> Result<PathBuf> {
    for dir in cwd.ancestors() {
        let nebula = dir.join(".nebula");
        if nebula.is_dir() {
            return Ok(nebula);
        }
        if dir.join(".git").exists() {
            break;
        }
    }
    bail!("no .nebula directory in this checkout; use --definition for an explicit file")
}

fn slug(id: &str) -> Result<()> {
    ensure!(
        id.len() <= 64
            && id.starts_with(|c: char| c.is_ascii_lowercase())
            && id
                .bytes()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || b"_-".contains(&c)),
        "{id:?} must be a lowercase filename slug (letters, digits, '_' or '-')"
    );
    Ok(())
}

fn named_file(dir: &Path, id: &str) -> Result<PathBuf> {
    slug(id)?;
    let path = dir.join(format!("{id}.toml"));
    let canonical = path
        .canonicalize()
        .with_context(|| format!("find {}", path.display()))?;
    let root = dir.canonicalize()?;
    ensure!(
        canonical.starts_with(root),
        "{} points outside its configuration directory",
        path.display()
    );
    ensure!(
        canonical.is_file(),
        "{} must be a regular file",
        path.display()
    );
    // Keep the selected filename as the identity even for an internal symlink.
    Ok(path)
}

fn workflow_ids(root: &Path) -> Result<Vec<String>> {
    let dir = root.join("workflows");
    if !dir.exists() {
        return Ok(Vec::new());
    }
    let mut ids = Vec::new();
    for entry in fs::read_dir(&dir).with_context(|| format!("read {}", dir.display()))? {
        let path = entry?.path();
        if path.extension().is_some_and(|ext| ext == "toml") {
            ids.push(
                path.file_stem()
                    .and_then(|s| s.to_str())
                    .context("workflow filename is not UTF-8")?
                    .to_string(),
            );
        }
    }
    ids.sort();
    Ok(ids)
}

pub(crate) fn load(
    cwd: &Path,
    workflow: Option<&str>,
    definition: Option<&Path>,
    config: &Config,
) -> Result<ResolvedWorkflow> {
    ensure!(
        workflow.is_none() || definition.is_none(),
        "choose --workflow or --definition"
    );
    let path = if let Some(path) = definition {
        if path.is_absolute() {
            path.to_path_buf()
        } else {
            cwd.join(path)
        }
    } else {
        let root = config_dir(cwd)?;
        if let Some(id) = workflow {
            named_file(&root.join("workflows"), id)?
        } else {
            let ids = workflow_ids(&root)?;
            if ids.iter().any(|id| id == "default") {
                named_file(&root.join("workflows"), "default")?
            } else if ids.len() == 1 {
                named_file(&root.join("workflows"), &ids[0])?
            } else if ids.is_empty() && root.join("workflow.json").is_file() {
                root.join("workflow.json")
            } else if ids.is_empty() {
                bail!("no workflow definitions; add .nebula/workflows/default.toml");
            } else {
                bail!(
                    "choose --workflow from: {} (or name the default file default.toml)",
                    ids.join(", ")
                );
            }
        }
    };
    load_path(&path, config)
}

fn load_path(path: &Path, config: &Config) -> Result<ResolvedWorkflow> {
    let text = read_text(path, 64 * 1024)?;
    let mut resolved = match path.extension().and_then(|ext| ext.to_str()) {
        Some("toml") => format::resolve(path, &text)?,
        Some("json") => {
            let definition: WorkflowDefinition = serde_json::from_str(&text)
                .with_context(|| format!("parse legacy workflow {}", path.display()))?;
            ResolvedWorkflow {
                path: path.to_path_buf(),
                description: "Legacy JSON workflow".into(),
                agent_sources: vec![None; definition.stages.len()],
                definition,
            }
        }
        _ => bail!(
            "{} must be a .toml or legacy .json definition",
            path.display()
        ),
    };
    for stage in &mut resolved.definition.stages {
        ensure!(
            config.kind_enabled(stage.kind),
            "{}: {} is disabled in NEBULA",
            path.display(),
            stage.kind.as_str()
        );
        if stage.model.is_none() {
            stage.model = config.default_model(stage.kind);
        }
        if stage.effort.is_none() {
            stage.effort = config.default_effort(stage.kind);
        }
    }
    nebula_daemon::workflow::validate_definition(&resolved.definition)
        .with_context(|| format!("validate {}", path.display()))?;
    Ok(resolved)
}

pub(crate) fn catalog(cwd: &Path, config: &Config) -> Result<Vec<CatalogEntry>> {
    let root = config_dir(cwd)?;
    let ids = workflow_ids(&root)?;
    let legacy = ids.is_empty() && root.join("workflow.json").is_file();
    let ids = if legacy { vec!["legacy".into()] } else { ids };
    Ok(ids
        .into_iter()
        .map(|id| {
            let path = if legacy {
                root.join("workflow.json")
            } else {
                root.join("workflows").join(format!("{id}.toml"))
            };
            let loaded = if legacy {
                load_path(&path, config)
            } else {
                named_file(&root.join("workflows"), &id).and_then(|p| load_path(&p, config))
            };
            match loaded {
                Ok(resolved) => CatalogEntry {
                    id,
                    path,
                    name: resolved.definition.name,
                    description: Some(resolved.description),
                    stages: resolved
                        .definition
                        .stages
                        .into_iter()
                        .map(|s| s.id)
                        .collect(),
                    error: None,
                },
                Err(err) => CatalogEntry {
                    id,
                    path,
                    name: None,
                    description: None,
                    stages: Vec::new(),
                    error: Some(format!("{err:#}")),
                },
            }
        })
        .collect())
}
