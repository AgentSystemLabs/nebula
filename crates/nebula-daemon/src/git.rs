//! Git worktree operations — shelled out to the `git` CLI on purpose:
//! libgit2's worktree support lags git's, these are rare user-initiated ops,
//! and git's stderr is the best error message we could show.

use anyhow::{anyhow, bail, Result};
use std::path::{Path, PathBuf};
use tokio::process::Command;

/// Shown when the `git` binary itself is missing. Every other git failure
/// carries git's own stderr; this one git never gets to print, so spelling out
/// the fix is on us — otherwise the user sees "No such file or directory" and
/// blames the directory they just picked. Kept to one line: the TUI shows it
/// in the footer flash, which truncates.
pub const GIT_MISSING: &str =
    "git was not found on your PATH — nebula needs it. Install git (https://git-scm.com/downloads), then restart nebula.";

/// True when `err` came from `git` being absent, so callers can pass the
/// message through instead of layering their own (wrong) explanation on top.
pub fn is_missing(err: &anyhow::Error) -> bool {
    err.chain().any(|c| c.to_string() == GIT_MISSING)
}

/// `git` never even started. NotFound means the binary isn't installed — the
/// one git failure with no stderr to quote, so the explanation has to be ours.
fn spawn_err(e: std::io::Error) -> anyhow::Error {
    if e.kind() == std::io::ErrorKind::NotFound {
        anyhow!(GIT_MISSING)
    } else {
        anyhow::Error::new(e).context("run git")
    }
}

async fn git(repo: &Path, args: &[&str]) -> Result<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .output()
        .await
        .map_err(spawn_err)?;
    if !output.status.success() {
        bail!("{}", String::from_utf8_lossy(&output.stderr).trim());
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// `git init` an existing directory.
pub async fn init(path: &Path) -> Result<()> {
    git(path, &["init"]).await?;
    Ok(())
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
    Ok(parse_worktree_list(&out))
}

/// The parse behind `list_worktrees`, kept free of git so it can be pinned
/// against captured porcelain output: one stanza per checkout, `worktree
/// <path>` first, then `HEAD <sha>` and either `branch refs/heads/<name>`
/// or `detached`, separated by blank lines.
fn parse_worktree_list(out: &str) -> Vec<WorktreeEntry> {
    /// Close out the stanza in progress, if one is open: a `branch` line
    /// named it, otherwise it is a detached HEAD. The next `worktree` line
    /// closes one stanza and the end of the output closes the last.
    fn close(
        entries: &mut Vec<WorktreeEntry>,
        path: Option<PathBuf>,
        branch: &mut Option<String>,
        head: Option<&str>,
    ) {
        if let Some(path) = path {
            entries.push(WorktreeEntry {
                path,
                branch: branch.take().unwrap_or_else(|| detached_label(head)),
            });
        }
    }

    let mut entries = Vec::new();
    let mut path: Option<PathBuf> = None;
    let mut branch: Option<String> = None;
    let mut head: Option<String> = None;
    for line in out.lines() {
        if let Some(p) = line.strip_prefix("worktree ") {
            close(&mut entries, path.take(), &mut branch, head.as_deref());
            head = None;
            path = Some(PathBuf::from(p));
        } else if let Some(sha) = line.strip_prefix("HEAD ") {
            head = Some(sha.to_string());
        } else if let Some(b) = line.strip_prefix("branch ") {
            branch = Some(b.trim_start_matches("refs/heads/").to_string());
        }
    }
    close(&mut entries, path, &mut branch, head.as_deref());
    entries
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
    let repo_name = repo
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "repo".into());
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
        // Locked by a session that ran `git worktree lock` (Claude Code locks
        // its worktree and a killed session never unlocks). The caller has
        // already killed this worktree's sessions, so the lock is stale —
        // unlock and retry rather than surfacing git's refusal.
        Err(e) if e.to_string().contains("locked working tree") => {
            git(repo, &["worktree", "unlock", &path_str]).await?;
            git(repo, &args).await?;
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
        git(dir, &["commit", "--allow-empty", "-m", "init"])
            .await
            .unwrap();
    }

    /// Captured `git worktree list --porcelain` shape: the main checkout
    /// leads, a linked worktree on a branch follows, and a detached one
    /// gets the short-sha label instead of a branch name.
    #[test]
    fn parse_worktree_list_reads_branches_and_detached_heads() {
        let porcelain = "worktree /repo\n\
                         HEAD 0123456789abcdef0123456789abcdef01234567\n\
                         branch refs/heads/main\n\
                         \n\
                         worktree /repo-worktrees/feat\n\
                         HEAD fedcba9876543210fedcba9876543210fedcba98\n\
                         branch refs/heads/feat/x\n\
                         \n\
                         worktree /repo-worktrees/pinned\n\
                         HEAD abcdef0123456789abcdef0123456789abcdef01\n\
                         detached\n\
                         \n";
        let entries = parse_worktree_list(porcelain);
        let got: Vec<(&Path, &str)> = entries
            .iter()
            .map(|e| (e.path.as_path(), e.branch.as_str()))
            .collect();
        assert_eq!(
            got,
            vec![
                (Path::new("/repo"), "main"),
                (Path::new("/repo-worktrees/feat"), "feat/x"),
                (Path::new("/repo-worktrees/pinned"), "detached @ abcdef0"),
            ]
        );
        // A trailing stanza with no blank line after it still closes.
        let entries = parse_worktree_list("worktree /only\nHEAD 1234567890\nbranch refs/heads/b");
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].branch, "b");
        assert!(parse_worktree_list("").is_empty());
    }

    #[test]
    fn missing_git_binary_explains_the_install() {
        let err = spawn_err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "No such file or directory (os error 2)",
        ));
        assert!(is_missing(&err), "{err:#}");
        assert!(err.to_string().contains("Install git"));
        // Still recognized once a caller layers its own context on top.
        assert!(is_missing(&err.context("open /some/dir")));
    }

    #[test]
    fn other_spawn_failures_are_not_reported_as_missing_git() {
        let err = spawn_err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "Permission denied (os error 13)",
        ));
        assert!(!is_missing(&err), "{err:#}");
    }

    #[tokio::test]
    async fn git_errors_are_not_reported_as_missing_git() {
        let tmp = tempfile::tempdir().unwrap();
        // A real git that says "not a repository" must keep saying so.
        let err = repo_toplevel(tmp.path()).await.unwrap_err();
        assert!(!is_missing(&err), "{err:#}");
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
    async fn remove_worktree_unlocks_session_locked_checkout() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("repo");
        std::fs::create_dir(&repo).unwrap();
        init_repo(&repo).await;
        let wt = add_worktree(&repo, "feature", None).await.unwrap();
        let wt_str = wt.to_string_lossy().into_owned();
        git(
            &repo,
            &[
                "worktree",
                "lock",
                "--reason",
                "claude session menu-enable-level",
                &wt_str,
            ],
        )
        .await
        .unwrap();

        remove_worktree(&repo, &wt, false).await.unwrap();
        let entries = list_worktrees(&repo).await.unwrap();
        assert!(entries.iter().all(|e| e.path != wt));
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
