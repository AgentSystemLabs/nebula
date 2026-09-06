use anyhow::{ensure, Result};
use nebula_core::{workflow::*, AgentKind, MAX_CLOUD_PROMPT_BYTES};
use std::collections::HashSet;

fn checked(value: &str, label: &str, max: usize) -> Result<()> {
    ensure!(
        !value.trim().is_empty() && !value.contains('\0') && value.len() <= max,
        "{label} must be nonempty, NUL-free text of at most {max} bytes"
    );
    Ok(())
}

pub(super) fn validate(task: &str, definition: &WorkflowDefinition) -> Result<()> {
    checked(task, "task", 8000)?;
    validate_definition(definition)
}

/// Shared by configuration previews and the DAEMON's IPC trust boundary.
pub fn validate_definition(definition: &WorkflowDefinition) -> Result<()> {
    ensure!(definition.version == 1, "workflow version must be 1");
    if let Some(id) = &definition.id {
        checked(id, "workflow id", 64)?;
        ensure!(
            id.starts_with(|c: char| c.is_ascii_lowercase())
                && id
                    .bytes()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || b"_-".contains(&c)),
            "workflow id must be a lowercase slug"
        );
    }
    if let Some(name) = &definition.name {
        checked(name, "workflow name", 120)?;
        ensure!(!name.chars().any(char::is_control), "invalid workflow name");
    }
    ensure!(
        (10..=86400).contains(&definition.timeout_seconds),
        "timeout_seconds must be 10..86400"
    );
    ensure!(
        (1..=10).contains(&definition.stages.len()),
        "workflow needs 1..10 stages"
    );
    let mut names = HashSet::new();
    for stage in &definition.stages {
        checked(&stage.id, "stage id", 40)?;
        ensure!(
            stage.id.starts_with(|c: char| c.is_ascii_lowercase())
                && stage
                    .id
                    .bytes()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || b"_-".contains(&c))
                && names.insert(&stage.id),
            "stage ids must be unique lowercase slugs"
        );
        ensure!(
            matches!(stage.kind, AgentKind::Claude | AgentKind::Codex),
            "prototype supports claude and codex"
        );
        checked(&stage.instructions, "stage instructions", 4000)?;
        for value in [&stage.model, &stage.effort].into_iter().flatten() {
            checked(value, "MODEL / EFFORT", 128)?;
            ensure!(
                !value.starts_with('-') && !value.chars().any(char::is_control),
                "invalid MODEL / EFFORT"
            );
        }
    }
    Ok(())
}

pub(super) fn validate_result(result: &StageResult) -> Result<()> {
    checked(&result.summary, "summary", 4000)?;
    if result.outcome == StageOutcome::Completed {
        checked(&result.artifact, "completed stage artifact", 64 * 1024)?;
    } else {
        ensure!(
            result.artifact.len() <= 64 * 1024 && !result.artifact.contains('\0'),
            "invalid artifact"
        );
    }
    Ok(())
}

pub(super) fn session_name(run: &WorkflowRun) -> String {
    format!(
        "workflow {} {}",
        run.id, run.definition.stages[run.current].id
    )
}

pub(super) fn starting_prompt(run: &WorkflowRun) -> Result<String> {
    let stage = &run.definition.stages[run.current];
    let worktree = run.worktree.as_ref().expect("assigned before launch");
    // A development DAEMON may be newer than the installed NEBULA on PATH.
    let executable = std::env::current_exe()?;
    let command = format!(
        "'{}' workflow",
        executable.to_string_lossy().replace('\'', "'\\''")
    );
    let prompt = format!(
        "You are the {} AGENT in workflow {}.\n\
        This is an already-created managed WORKTREE at {} on branch {}. Stay here.\n\
        Runtime state and previous stage artifacts are in NEBULA's SQLITE STORE.\n\
        First run `{command} status {} --json` and read the task and previous stage results.\n\n\
        Task: {}\n\nYour stage: {}\n\n\
        Work autonomously within this task and the repository rules. The DAEMON owns stage ordering.\n\
        Do not start another workflow, launch other SESSIONS, or relocate this WORKTREE.\n\
        Do not commit, push, merge, publish, or deploy. Leave changes for the user to inspect.\n\
        After all stage work, checks, and repository bookkeeping, write a Markdown result to\n\
        a temporary file outside the checkout. Replace RESULT_FILE below with its quoted absolute path.\n\
        As your LAST tool call run:\n\
          {command} report --stage {} --outcome completed --file RESULT_FILE \
        --summary \"Stage completed\"\n\
        Then end your turn. The next stage waits for both this report and FINISHED status.\n\
        If blocked, run `{command} report --stage {} --outcome blocked --summary \"<exact reason>\"`\n\
        and end your turn. Never report completed for unfinished work.",
        stage.id, run.id, worktree.path.display(), run.branch, run.id, run.task,
        stage.instructions, stage.id, stage.id,
    );
    ensure!(
        prompt.len() <= MAX_CLOUD_PROMPT_BYTES,
        "composed STARTING PROMPT exceeds 16 KiB"
    );
    Ok(prompt)
}
