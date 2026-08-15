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
    let mut head: Option<String> = None;
    for line in out.lines() {
        if let Some(p) = line.strip_prefix("worktree ") {
            if let Some(done_path) = path.take() {
                entries.push(WorktreeEntry {
                    path: done_path,
                    branch: branch.take().unwrap_or_else(|| detached_label(head.as_deref())),
                });
            }
            head = None;
            path = Some(PathBuf::from(p));
        } else if let Some(sha) = line.strip_prefix("HEAD ") {
            head = Some(sha.to_string());
        } else if let Some(b) = line.strip_prefix("branch ") {
            branch = Some(b.trim_start_matches("refs/heads/").to_string());
        }
    }
    if let Some(done_path) = path {
        entries.push(WorktreeEntry {
            path: done_path,
            branch: branch.unwrap_or_else(|| detached_label(head.as_deref())),
        });
    }
    Ok(entries)
}

/// Display name for a checkout with no branch (detached HEAD).
fn detached_label(head: Option<&str>) -> String {
    match head {
        Some(sha) => format!("detached @ {}", &sha[..sha.len().min(7)]),
        None => "(detached)".into(),
    }
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
    // Checkout already gone (manual rm -rf): `git worktree remove` would fail,
    // but the user's intent is already satisfied — just drop git's stale
    // bookkeeping so the entry leaves `git worktree list`.
    if !worktree_path.exists() {
        let _ = git(repo, &["worktree", "prune"]).await;
        return Ok(());
    }
    let path_str = worktree_path.to_string_lossy().into_owned();
    let mut args = vec!["worktree", "remove"];
    if force {
        args.push("--force");
    }
    args.push(&path_str);
    match git(repo, &args).await {
        Ok(_) => Ok(()),
        // Directory exists but git no longer tracks it as a worktree (already
        // pruned, or its .git link was destroyed). Nothing for git to remove;
        // prune any leftover metadata and let the caller drop its row. The
        // directory itself is left alone — deleting an untracked dir is not
        // ours to do.
        Err(e)
            if e.to_string().contains("is not a working tree")
                || e.to_string().contains("does not exist") =>
        {
            let _ = git(repo, &["worktree", "prune"]).await;
            Ok(())
        }
        Err(e) => Err(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn init_repo(dir: &Path) {
        git(dir, &["init", "-b", "main"]).await.unwrap();
        git(dir, &["config", "user.email", "t@t"]).await.unwrap();
        git(dir, &["config", "user.name", "t"]).await.unwrap();
        git(dir, &["commit", "--allow-empty", "-m", "init"]).await.unwrap();
    }

    #[tokio::test]
    async fn remove_worktree_survives_manual_rm_rf() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("repo");
        std::fs::create_dir(&repo).unwrap();
        init_repo(&repo).await;
        let wt = add_worktree(&repo, "feature", None).await.unwrap();

        // Simulate the user deleting the checkout by hand.
        std::fs::remove_dir_all(&wt).unwrap();

        remove_worktree(&repo, &wt, false).await.unwrap();
        // The stale registration should be pruned from git's list too.
        let entries = list_worktrees(&repo).await.unwrap();
        assert!(entries.iter().all(|e| e.path != wt));
    }

    #[tokio::test]
    async fn remove_worktree_ok_when_already_pruned() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("repo");
        std::fs::create_dir(&repo).unwrap();
        init_repo(&repo).await;
        let wt = add_worktree(&repo, "feature", None).await.unwrap();
        std::fs::remove_dir_all(&wt).unwrap();
        git(&repo, &["worktree", "prune"]).await.unwrap();

        // Path gone AND git no longer knows it — still not an error.
        remove_worktree(&repo, &wt, false).await.unwrap();
    }

    #[tokio::test]
    async fn remove_worktree_still_fails_on_dirty_checkout() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("repo");
        std::fs::create_dir(&repo).unwrap();
        init_repo(&repo).await;
        let wt = add_worktree(&repo, "feature", None).await.unwrap();
        std::fs::write(wt.join("untracked.txt"), "dirty").unwrap();

        assert!(remove_worktree(&repo, &wt, false).await.is_err());
        remove_worktree(&repo, &wt, true).await.unwrap();
    }
}
