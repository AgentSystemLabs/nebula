//! Git worktree operations — shelled out to the `git` CLI on purpose:
//! libgit2's worktree support lags git's, these are rare user-initiated ops,
//! and git's stderr is the best error message we could show.

use anyhow::{bail, Context, Result};
use std::path::{Path, PathBuf};
use tokio::process::Command;

async fn git(repo: &Path, args: &[&str]) -> Result<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .output()
        .await
        .context("run git")?;
    if !output.status.success() {
        bail!("{}", String::from_utf8_lossy(&output.stderr).trim());
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Verify `path` is inside a git repo and return its toplevel.
pub async fn repo_toplevel(path: &Path) -> Result<PathBuf> {
    let out = git(path, &["rev-parse", "--show-toplevel"]).await?;
    Ok(PathBuf::from(out.trim()))
}

pub async fn current_branch(repo: &Path) -> Result<String> {
    let out = git(repo, &["branch", "--show-current"]).await?;
    let branch = out.trim();
    if branch.is_empty() {
        // Detached HEAD — fall back to the short hash.
        let hash = git(repo, &["rev-parse", "--short", "HEAD"]).await?;
        return Ok(format!("detached@{}", hash.trim()));
    }
    Ok(branch.to_string())
}

#[derive(Debug, Clone)]
pub struct WorktreeEntry {
    pub path: PathBuf,
    pub branch: String,
}

/// Parse `git worktree list --porcelain`. The first entry is the main
/// checkout.
pub async fn list_worktrees(repo: &Path) -> Result<Vec<WorktreeEntry>> {
    let out = git(repo, &["worktree", "list", "--porcelain"]).await?;
    let mut entries = Vec::new();
    let mut path: Option<PathBuf> = None;
    let mut branch: Option<String> = None;
    for line in out.lines() {
        if let Some(p) = line.strip_prefix("worktree ") {
            if let Some(done_path) = path.take() {
                entries.push(WorktreeEntry {
                    path: done_path,
                    branch: branch.take().unwrap_or_else(|| "(detached)".into()),
                });
            }
            path = Some(PathBuf::from(p));
        } else if let Some(b) = line.strip_prefix("branch ") {
            branch = Some(b.trim_start_matches("refs/heads/").to_string());
        } else if line == "detached" {
            branch = Some("(detached)".into());
        }
    }
    if let Some(done_path) = path {
        entries.push(WorktreeEntry {
            path: done_path,
            branch: branch.unwrap_or_else(|| "(detached)".into()),
        });
    }
    Ok(entries)
}

/// Directory a new worktree for `branch` should live in:
/// `<repo>/../<repo-name>-worktrees/<branch>` (slashes in branch → dashes).
pub fn worktree_dir(repo: &Path, branch: &str) -> PathBuf {
    let repo_name = repo.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "repo".into());
    let safe_branch = branch.replace('/', "-");
    repo.parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."))
        .join(format!("{repo_name}-worktrees"))
        .join(safe_branch)
}

/// `git worktree add <path> -b <branch> [base]`. Falls back to checking out an
/// existing branch when `-b` fails because it already exists.
pub async fn add_worktree(repo: &Path, branch: &str, base: Option<&str>) -> Result<PathBuf> {
    let path = worktree_dir(repo, branch);
    if path.exists() {
        bail!("worktree path already exists: {}", path.display());
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let path_str = path.to_string_lossy().into_owned();
    let mut args = vec!["worktree", "add", &path_str, "-b", branch];
    if let Some(base) = base {
        args.push(base);
    }
    match git(repo, &args).await {
        Ok(_) => Ok(path),
        Err(e) if e.to_string().contains("already exists") => {
            // Branch exists: check it out instead of creating.
            git(repo, &["worktree", "add", &path_str, branch]).await?;
            Ok(path)
        }
        Err(e) => Err(e),
    }
}

pub async fn remove_worktree(repo: &Path, worktree_path: &Path, force: bool) -> Result<()> {
    let path_str = worktree_path.to_string_lossy().into_owned();
    let mut args = vec!["worktree", "remove"];
    if force {
        args.push("--force");
    }
    args.push(&path_str);
    git(repo, &args).await?;
    Ok(())
}
