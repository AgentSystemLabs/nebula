//! Git's on-disk layout, read straight off the filesystem — no `git`
//! process — for the questions orbit asks often enough that a spawn per
//! call would show: which `.git` a checkout shares, and which checkout is
//! the repository's main one.

use std::path::{Path, PathBuf};

/// The `.git` holding the repo's shared HEAD and per-worktree HEADs.
///
/// `<checkout>/.git` is that directory in a normal checkout, but in a linked
/// worktree it is a `gitdir:` file pointing at `<repo>/.git/worktrees/<name>`,
/// whose own `commondir` points back at the shared `.git`. Following both is
/// what lets a project rooted at a worktree (one added with `orbit add` from
/// inside one) find the repository at all. None when `checkout` is not a git
/// checkout.
pub fn git_common_dir(checkout: &Path) -> Option<PathBuf> {
    let dot_git = checkout.join(".git");
    let git_dir = if std::fs::metadata(&dot_git).ok()?.is_dir() {
        dot_git
    } else {
        let text = std::fs::read_to_string(&dot_git).ok()?;
        rebase_on(checkout, text.trim().strip_prefix("gitdir:")?.trim())
    };
    // A worktree gitdir has no HEAD history of the repo; `commondir` is the
    // hop back to the `.git` that does. A normal checkout has no such file.
    let common = match std::fs::read_to_string(git_dir.join("commondir")) {
        // git writes that hop relatively (`../..`), so the join keeps the
        // parent components; canonicalizing folds them away, which is what
        // makes the two checkouts of one repo answer with the same path
        // rather than two spellings of it.
        Ok(common) => rebase_on(&git_dir, common.trim()),
        Err(_) => git_dir,
    };
    Some(std::fs::canonicalize(&common).unwrap_or(common))
}

/// The repository's main checkout for `checkout` — the directory whose
/// `.git` every linked worktree shares. `checkout` itself when it is the
/// main one, when it is not a git checkout at all, or when the repository's
/// `.git` is not a checkout's (a bare repository's worktrees).
pub fn main_checkout(checkout: &Path) -> PathBuf {
    git_common_dir(checkout)
        .filter(|common| common.file_name().is_some_and(|name| name == ".git"))
        .and_then(|common| common.parent().map(Path::to_path_buf))
        .unwrap_or_else(|| checkout.to_path_buf())
}

/// Git writes these pointers as either an absolute path or one relative to the
/// file that carries them.
pub fn rebase_on(base: &Path, target: &str) -> PathBuf {
    let target = PathBuf::from(target);
    if target.is_absolute() {
        target
    } else {
        base.join(target)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn git(dir: &Path, args: &[&str]) {
        let ok = std::process::Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@example.com")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@example.com")
            .output()
            .unwrap()
            .status
            .success();
        assert!(ok, "git {args:?}");
    }

    #[test]
    fn a_worktree_and_its_repo_share_one_main_checkout() {
        let root = std::env::temp_dir().join(format!("orbit-git-{}", ulid::Ulid::generate()));
        let repo = root.join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        git(&repo, &["init", "-b", "main"]);
        git(&repo, &["commit", "--allow-empty", "-m", "init"]);
        let feat = root.join("repo-worktrees").join("feat");
        git(
            &repo,
            &["worktree", "add", &feat.to_string_lossy(), "-b", "feat"],
        );
        let canonical = std::fs::canonicalize(&repo).unwrap();
        assert_eq!(main_checkout(&repo), canonical);
        assert_eq!(
            main_checkout(&feat),
            canonical,
            "a worktree's main is the repo"
        );
        assert_eq!(git_common_dir(&feat), git_common_dir(&repo));

        let plain = root.join("not-git");
        std::fs::create_dir_all(&plain).unwrap();
        assert_eq!(main_checkout(&plain), plain, "no git: the folder itself");
        let _ = std::fs::remove_dir_all(&root);
    }
}
