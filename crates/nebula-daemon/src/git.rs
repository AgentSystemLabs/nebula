//! Git worktree operations — shelled out to the `git` CLI on purpose:
//! libgit2's worktree support lags git's, these are rare user-initiated ops,
//! and git's stderr is the best error message we could show.

use anyhow::{anyhow, bail, Context, Result};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::Duration;
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

/// [`git`] for a background read that must not take the index lock: a
/// `git status` that refreshes the index writes it back, racing whatever
/// the user or an agent runs in that checkout at the same moment
/// (`GIT_OPTIONAL_LOCKS=0`, as editors and prompts set it).
async fn git_read(repo: &Path, args: &[&str]) -> Result<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .env("GIT_OPTIONAL_LOCKS", "0")
        .output()
        .await
        .map_err(spawn_err)?;
    if !output.status.success() {
        bail!("{}", String::from_utf8_lossy(&output.stderr).trim());
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// [`git_read`] with `input` on its stdin.
async fn git_read_input(repo: &Path, args: &[&str], input: &str) -> Result<String> {
    use tokio::io::AsyncWriteExt;
    let mut child = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .env("GIT_OPTIONAL_LOCKS", "0")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(spawn_err)?;
    let mut stdin = child.stdin.take().context("git's stdin")?;
    let input = input.to_string();
    let writer = tokio::spawn(async move {
        let _ = stdin.write_all(input.as_bytes()).await;
    });
    let output = child.wait_with_output().await?;
    let _ = writer.await;
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

/// `git worktree repair` from a repo whose main checkout moved, naming the
/// new paths of linked checkouts that moved with it. git then rewrites the
/// links both ways: each checkout's `.git` file back to the repo, and the
/// repo's record of where each checkout lives. A checkout that stayed put
/// (a sibling `<repo>-worktrees/` one) is repaired without being named.
pub async fn repair_worktrees(repo: &Path, moved: &[PathBuf]) -> Result<()> {
    let moved: Vec<String> = moved
        .iter()
        .map(|p| p.to_string_lossy().into_owned())
        .collect();
    let mut args = vec!["worktree", "repair"];
    args.extend(moved.iter().map(String::as_str));
    git(repo, &args).await?;
    Ok(())
}

#[derive(Debug, Clone)]
pub struct WorktreeEntry {
    pub path: PathBuf,
    pub branch: String,
}

/// Parse `git worktree list --porcelain`. The first entry is the main
/// checkout.
///
/// Only checkouts that are really there are returned. The parse drops the
/// ones git marks `prunable`; a *locked* checkout is never marked prunable
/// however gone it is (Claude Code locks its worktrees and a killed session
/// never unlocks), so a linked checkout whose `.git` file is missing is
/// dropped here too. Git's registry is left exactly as it was: nebula never
/// prunes or unlocks for the user, and the checkout is listed again the
/// moment its directory comes back (a remounted volume, say).
pub async fn list_worktrees(repo: &Path) -> Result<Vec<WorktreeEntry>> {
    let out = git(repo, &["worktree", "list", "--porcelain"]).await?;
    let mut entries = parse_worktree_list(&out);
    let mut index = 0;
    entries.retain(|entry| {
        index += 1;
        index == 1 || entry.path.join(".git").exists()
    });
    for entry in &mut entries {
        if entry.branch != "(detached)" && !entry.branch.starts_with("detached @ ") {
            continue;
        }
        if let Some(branch) = rebasing_branch(&entry.path).await {
            entry.branch = branch;
        }
    }
    Ok(entries)
}

/// The parse behind `list_worktrees`, kept free of git so it can be pinned
/// against captured porcelain output: one stanza per checkout, `worktree
/// <path>` first, then `HEAD <sha>` and either `branch refs/heads/<name>`
/// or `detached`, separated by blank lines.
///
/// A stanza marked `prunable` is left out: its directory was deleted without
/// `git worktree remove` (`rm -rf`, Finder, a disk sweep), and git keeps
/// listing it only until the next `git worktree prune`. It is not a checkout
/// anyone can work in, so it must not be adopted or keep a row alive. The
/// main checkout is never prunable, so it stays first.
fn parse_worktree_list(out: &str) -> Vec<WorktreeEntry> {
    /// Close out the stanza in progress, if one is open: a `branch` line
    /// named it, otherwise it is a detached HEAD. The next `worktree` line
    /// closes one stanza and the end of the output closes the last.
    fn close(
        entries: &mut Vec<WorktreeEntry>,
        path: Option<PathBuf>,
        branch: &mut Option<String>,
        head: Option<&str>,
        prunable: bool,
    ) {
        let branch_name = branch.take();
        if prunable {
            return;
        }
        if let Some(path) = path {
            entries.push(WorktreeEntry {
                path,
                branch: branch_name.unwrap_or_else(|| detached_label(head)),
            });
        }
    }

    let mut entries = Vec::new();
    let mut path: Option<PathBuf> = None;
    let mut branch: Option<String> = None;
    let mut head: Option<String> = None;
    let mut prunable = false;
    for line in out.lines() {
        if let Some(p) = line.strip_prefix("worktree ") {
            close(
                &mut entries,
                path.take(),
                &mut branch,
                head.as_deref(),
                prunable,
            );
            head = None;
            prunable = false;
            path = Some(PathBuf::from(p));
        } else if let Some(sha) = line.strip_prefix("HEAD ") {
            head = Some(sha.to_string());
        } else if let Some(b) = line.strip_prefix("branch ") {
            branch = Some(b.trim_start_matches("refs/heads/").to_string());
        } else if line == "prunable" || line.starts_with("prunable ") {
            prunable = true;
        }
    }
    close(&mut entries, path, &mut branch, head.as_deref(), prunable);
    entries
}

/// The lock git keeps on `worktree`, a checkout of `repo`: the reason it
/// was locked with (empty when none was given), or None when it is not
/// locked. Anyone can lock a checkout — `git worktree lock --reason …` by
/// hand, an agent marking the one it works in, a tool keeping one it
/// reuses — and git's own `worktree remove` and `prune` then leave it be.
pub async fn worktree_lock(repo: &Path, worktree: &Path) -> Result<Option<String>> {
    let out = git(repo, &["worktree", "list", "--porcelain"]).await?;
    let real = std::fs::canonicalize(worktree).ok();
    Ok(lock_in_list(&out, |listed| {
        listed == worktree || real.is_some() && std::fs::canonicalize(listed).ok() == real
    }))
}

/// The pure half of [`worktree_lock`]: in `git worktree list --porcelain`
/// output, the `locked` line of the stanza whose path `is_target` picks.
/// Git writes `locked` alone when no reason was given, and C-quotes a
/// reason it cannot print as is (a newline, a quote).
fn lock_in_list(out: &str, is_target: impl Fn(&Path) -> bool) -> Option<String> {
    let mut in_target = false;
    for line in out.lines() {
        if let Some(listed) = line.strip_prefix("worktree ") {
            in_target = is_target(Path::new(listed));
        } else if in_target && line == "locked" {
            return Some(String::new());
        } else if let Some(reason) = line.strip_prefix("locked ").filter(|_| in_target) {
            return Some(unquote_c_style(reason));
        }
    }
    None
}

/// Undo git's C-style quoting (`"a \"b\"\n\303\251"`); text git left
/// unquoted comes back as it is.
fn unquote_c_style(text: &str) -> String {
    let Some(inner) = text.strip_prefix('"').and_then(|t| t.strip_suffix('"')) else {
        return text.to_string();
    };
    let mut bytes = Vec::with_capacity(inner.len());
    let mut chars = inner.bytes().peekable();
    while let Some(b) = chars.next() {
        if b != b'\\' {
            bytes.push(b);
            continue;
        }
        match chars.next() {
            Some(b'n') => bytes.push(b'\n'),
            Some(b't') => bytes.push(b'\t'),
            Some(b'r') => bytes.push(b'\r'),
            Some(b'a') => bytes.push(0x07),
            Some(b'b') => bytes.push(0x08),
            Some(b'f') => bytes.push(0x0c),
            Some(b'v') => bytes.push(0x0b),
            Some(d @ b'0'..=b'7') => {
                let mut value = u32::from(d - b'0');
                for _ in 0..2 {
                    match chars.peek() {
                        Some(&o @ b'0'..=b'7') => {
                            value = value * 8 + u32::from(o - b'0');
                            chars.next();
                        }
                        _ => break,
                    }
                }
                bytes.push(u8::try_from(value).unwrap_or(b'?'));
            }
            Some(other) => bytes.push(other),
            None => bytes.push(b'\\'),
        }
    }
    String::from_utf8_lossy(&bytes).into_owned()
}

/// The branch a paused rebase in `checkout` is replaying, if there is one —
/// read from the same state file `git status` uses to say "rebasing branch
/// X" while `git worktree list` calls the checkout detached. `None` for a
/// checkout that is not rebasing, is rebasing a detached HEAD, or is gone.
async fn rebasing_branch(checkout: &Path) -> Option<String> {
    // Per-worktree git dir: the rebase state lives under
    // `<repo>/.git/worktrees/<name>/` for a linked checkout, not in the
    // shared `.git`.
    let git_dir = git(checkout, &["rev-parse", "--absolute-git-dir"])
        .await
        .ok()?;
    let git_dir = Path::new(git_dir.trim());
    // `rebase-merge` is the default backend, `rebase-apply` the `--apply` one.
    ["rebase-merge", "rebase-apply"]
        .into_iter()
        .find_map(|state| {
            let name = std::fs::read_to_string(git_dir.join(state).join("head-name")).ok()?;
            // Rebasing with HEAD already detached writes "detached HEAD" here.
            Some(name.trim().strip_prefix("refs/heads/")?.to_string())
        })
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
/// existing branch when `-b` fails because it already exists. `None` is
/// git's own default — the checkout's HEAD; a base that is a
/// remote-tracking branch becomes the new branch's upstream, as git does.
pub async fn add_worktree(repo: &Path, branch: &str, base: Option<&str>) -> Result<PathBuf> {
    add_worktree_inner(repo, branch, base, true).await
}

/// `add_worktree` for a branch nobody named a base for: it starts at the
/// fetched `origin/HEAD` (see `default_base`) — what everyone else sees as
/// main — instead of the ROOT WORKTREE's HEAD, which is routinely commits
/// behind or on some other branch. Cut with `--no-track`: the branch is
/// new work, not a copy of main, and a branch tracking `origin/main`
/// aims its first `git push` at main (`push.default=simple` refuses it,
/// `upstream` sends it). Falls back to HEAD when there is no `origin` or
/// the fetch fails (offline).
pub async fn add_worktree_off_default(repo: &Path, branch: &str) -> Result<PathBuf> {
    let base = default_base(repo).await;
    add_worktree_inner(repo, branch, base.as_deref(), false).await
}

/// `add_worktree` for a base the caller named (`nebula worktree --base
/// <ref>`). `origin` is fetched first, and a branch origin has — `main`,
/// `release` — means origin's copy of it, `origin/main`, never the
/// checkout's local branch of that name: that one is whatever this
/// checkout last pulled, routinely commits behind. Cut with `--no-track`
/// like a default-base worktree, for the same reason. A ref origin has no
/// branch for — a tag, a SHA, a local-only branch, an explicit `origin/x`,
/// or `HEAD` (always this checkout's) — is used as named and tracks as git
/// decides. The rewrite does not wait on the fetch succeeding: offline,
/// `origin/main` as last fetched is still never behind the local branch's
/// last pull, and the daemon log says the fetch failed.
///
pub async fn add_worktree_off_ref(repo: &Path, branch: &str, base: &str) -> Result<PathBuf> {
    fetch_origin_if_any(repo).await;
    match origin_branch(repo, base).await {
        Some(remote) => add_worktree_inner(repo, branch, Some(&remote), false).await,
        None => add_worktree_inner(repo, branch, Some(base), true).await,
    }
}

/// `add_worktree` for a branch nobody named a base for when the
/// `worktree_base_branch` SETTING names one (`master`, `develop`): the
/// stand-in for `origin/HEAD` in repos whose default branch is not what
/// origin says, or that have no origin at all. Resolved like `--base`:
/// `origin` is fetched first, and origin's copy of the branch
/// (`origin/master`) wins over the checkout's local one, which is whatever
/// it last pulled — cut with `--no-track`, for the reason
/// `add_worktree_off_default` gives. A branch origin lacks that the
/// checkout has locally is used as named. The setting is one name for
/// every project, so a repo with no branch of that name at all does not
/// fail the `n`: it falls back to the fetched `origin/HEAD` exactly as if
/// the setting were empty, and the daemon log says which repo ignored it.
pub async fn add_worktree_off_configured(
    repo: &Path,
    branch: &str,
    configured: &str,
) -> Result<PathBuf> {
    let fetched = fetch_origin_if_any(repo).await;
    if let Some(remote) = origin_branch(repo, configured).await {
        return add_worktree_inner(repo, branch, Some(&remote), false).await;
    }
    if local_branch(repo, configured).await {
        return add_worktree_inner(repo, branch, Some(configured), true).await;
    }
    tracing::warn!(
        repo = %repo.display(),
        base = configured,
        "configured worktree base branch is not in this repo; branching from origin's default"
    );
    let base = if fetched {
        origin_head(repo).await
    } else {
        None
    };
    add_worktree_inner(repo, branch, base.as_deref(), false).await
}

async fn add_worktree_inner(
    repo: &Path,
    branch: &str,
    base: Option<&str>,
    track: bool,
) -> Result<PathBuf> {
    let path = worktree_dir(repo, branch);
    if path.exists() {
        bail!("worktree path already exists: {}", path.display());
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let path_str = path.to_string_lossy().into_owned();
    let mut args = vec!["worktree", "add", &path_str];
    if !track {
        args.push("--no-track");
    }
    args.extend(["-b", branch]);
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

/// How long `default_base` waits for a call that talks to the remote —
/// the fetch, and `remote set-head --auto` when `origin/HEAD` is unset —
/// before branching from local HEAD instead. They run under the DAEMON's
/// worktree lock, so a stalled connection must not hold every worktree op
/// with it.
const REMOTE_TIMEOUT: Duration = Duration::from_secs(30);

/// The start point for a new branch when the caller named none: the
/// remote's default branch, fetched first, so it is what `origin` has
/// right now (`origin/main` for most repos) and not what the ROOT WORKTREE
/// happens to have pulled. `None` — git's own default, the checkout's
/// HEAD — when the repo has no `origin` or the fetch fails, which the
/// daemon log says; a worktree cut offline is better than none.
pub async fn default_base(repo: &Path) -> Option<String> {
    if !fetch_origin_if_any(repo).await {
        return None;
    }
    origin_head(repo).await
}

/// `git fetch origin` when the repo has an `origin`: true when origin's
/// refs are what origin has right now. False — the reason in the daemon
/// log — when there is no origin or the fetch fails (offline), and the
/// caller makes do with what the checkout has.
async fn fetch_origin_if_any(repo: &Path) -> bool {
    if git(repo, &["remote", "get-url", "origin"]).await.is_err() {
        return false;
    }
    match fetch_origin(repo).await {
        Ok(()) => true,
        Err(e) => {
            tracing::warn!(
                repo = %repo.display(),
                error = %e,
                "fetch before worktree add failed; branching from what the checkout has"
            );
            false
        }
    }
}

/// `origin/<name>` when origin has a branch called `name`. None for a
/// name that is no origin branch — a tag, a SHA, a local-only branch, an
/// already-qualified `origin/x` — and for `HEAD`, which always means this
/// checkout's: `refs/remotes/origin/HEAD` exists too, and is not what
/// anyone naming HEAD means.
async fn origin_branch(repo: &Path, name: &str) -> Option<String> {
    if name == "HEAD" {
        return None;
    }
    let full = format!("refs/remotes/origin/{name}");
    git(repo, &["rev-parse", "--verify", "--quiet", &full])
        .await
        .ok()?;
    Some(format!("origin/{name}"))
}

/// Whether the checkout has a local branch called `name`
/// (`refs/heads/<name>`). Only branches: the setting is a *branch* name,
/// and a tag or SHA that happened to share it is not what anyone meant.
pub(crate) async fn local_branch(repo: &Path, name: &str) -> bool {
    let full = format!("refs/heads/{name}");
    git(repo, &["rev-parse", "--verify", "--quiet", &full])
        .await
        .is_ok()
}

/// `git fetch origin`, killed and reported as an error past `REMOTE_TIMEOUT`.
async fn fetch_origin(repo: &Path) -> Result<()> {
    git_remote(repo, &["fetch", "--quiet", "origin"]).await?;
    Ok(())
}

/// `git` for a call that talks to the remote: the child is killed and the
/// call reported as an error past `REMOTE_TIMEOUT`, so a dropped
/// connection or a credential prompt with no tty degrades to the local
/// fallback instead of wedging the worktree lock.
async fn git_remote(repo: &Path, args: &[&str]) -> Result<String> {
    let run = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .kill_on_drop(true)
        .output();
    let output = match tokio::time::timeout(REMOTE_TIMEOUT, run).await {
        Ok(output) => output.map_err(spawn_err)?,
        Err(_) => bail!(
            "git {} did not finish within {}s",
            args.join(" "),
            REMOTE_TIMEOUT.as_secs()
        ),
    };
    if !output.status.success() {
        bail!("{}", String::from_utf8_lossy(&output.stderr).trim());
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// `origin/HEAD` as a short ref (`origin/main`). A repo whose remote was
/// added by hand (`git remote add`, a fresh push) has no such symref, so
/// one `git remote set-head origin --auto` — a remote round-trip, timeboxed
/// like the fetch — asks the remote which branch it means and records the
/// answer for next time. None when the remote has no HEAD or does not
/// answer in time.
async fn origin_head(repo: &Path) -> Option<String> {
    for attempt in 0..2 {
        if let Ok(out) = git(
            repo,
            &["symbolic-ref", "-q", "--short", "refs/remotes/origin/HEAD"],
        )
        .await
        {
            let short = out.trim();
            if !short.is_empty() {
                return Some(short.to_string());
            }
        }
        if attempt == 0
            && git_remote(repo, &["remote", "set-head", "origin", "--auto"])
                .await
                .is_err()
        {
            return None;
        }
    }
    None
}

/// Check pull request `number`'s head branch `head` out into a new worktree
/// in the WORKTREE DIR layout — where every PR SESSION for it runs. Pure
/// git, two routes: a same-repo PR's branch is fetched from `origin` and
/// checked out tracking it (so a plain `git push` lands on the PR); a fork's
/// branch is not on `origin`, so the PR ref itself (`refs/pull/N/head`)
/// seeds a local branch of that name. A branch that already exists locally
/// is checked out as is — `add_worktree`'s own fallback — even offline.
///
/// A fork's `head` arrives under its owner's name (`givemeurhats/main` —
/// the client's `checkout_branch`), so it is never a branch `origin` has,
/// nor one of ours: the first fetch misses and the PR ref seeds it. That
/// branch has no upstream of its own, so it is given the one `gh pr
/// checkout` gives a fork's — `origin`, `refs/pull/N/head`
/// (`track_pr_ref`): `git pull` in the checkout then brings the
/// contributor's new commits in, and `gh pr view` there finds the pull
/// request, which a bare branch name never did for a fork — the
/// checkout's own PR ROW, its merge and its unread count hang on that.
pub async fn add_pr_worktree(repo: &Path, number: u64, head: &str) -> Result<PathBuf> {
    let local = git(
        repo,
        &[
            "rev-parse",
            "--verify",
            "--quiet",
            &format!("refs/heads/{head}"),
        ],
    )
    .await
    .is_ok();
    // Both fetches talk to the remote under the DAEMON's worktree lock, so
    // they are bounded (`git_remote`): a stalled connection must not hold
    // every worktree op — and this launch's stand-in rows — with it.
    let pr_ref = format!("refs/pull/{number}/head");
    let mut from_pr_ref = false;
    let base = match git_remote(repo, &["fetch", "origin", head]).await {
        // The remote-tracking ref when the clone keeps one, so the new
        // branch tracks it; a single-branch clone's fetch writes only
        // FETCH_HEAD, and the tip it names is the base then.
        Ok(_) => match origin_branch(repo, head).await {
            Some(remote) => Some(remote),
            None => fetched_tip(repo).await,
        },
        Err(branch_err) => match git_remote(repo, &["fetch", "origin", &pr_ref]).await {
            Ok(_) => {
                from_pr_ref = true;
                fetched_tip(repo).await
            }
            Err(_) if local => None,
            Err(pr_err) => bail!(
                "could not fetch pull request #{number} ({head}) from origin: {pr_err} \
                 (branch: {branch_err})"
            ),
        },
    };
    let path = add_worktree(repo, head, base.as_deref()).await?;
    if from_pr_ref {
        track_pr_ref(repo, head, &pr_ref).await;
    }
    // A branch that was already here — this pull request reviewed before,
    // its worktree deleted since (a delete keeps the branch) — came up as
    // it was left, which is behind whatever the author pushed meanwhile:
    // the session would review commits `gh pr diff` no longer shows. The
    // checkout is seconds old and clean, so it fast-forwards to the
    // fetched tip; a branch with commits of its own is left as it is.
    if let (true, Some(tip)) = (local, base.as_deref()) {
        if let Err(error) = git(&path, &["merge", "--ff-only", "--quiet", tip]).await {
            tracing::info!(
                branch = head,
                error = %error,
                "a pull request's existing branch was not fast-forwarded to its fetched tip"
            );
        }
    }
    Ok(path)
}

/// The commit the fetch that just ran brought in, by SHA rather than as
/// `FETCH_HEAD` — which the next fetch anyone runs in this repo (an agent's,
/// the default-base fetch of another worktree op's tail) would repoint
/// before `git worktree add` read it.
async fn fetched_tip(repo: &Path) -> Option<String> {
    git(repo, &["rev-parse", "--verify", "--quiet", "FETCH_HEAD"])
        .await
        .ok()
        .map(|sha| sha.trim().to_string())
        .filter(|sha| !sha.is_empty())
}

/// Point `branch` — a fork pull request's local branch, seeded from
/// `pr_ref` — at that ref on `origin`, the upstream `gh pr checkout` gives
/// a fork it cannot push to. Left alone when the branch already has an
/// upstream (a checkout `gh` made, pushing to the fork itself). A failure
/// costs the checkout its `git pull` and its PR ROW, not the launch, so it
/// is logged rather than returned.
async fn track_pr_ref(repo: &Path, branch: &str, pr_ref: &str) {
    let merge = format!("branch.{branch}.merge");
    if config_get(repo, &merge).await.is_some() {
        return;
    }
    let remote = format!("branch.{branch}.remote");
    for (key, value) in [(remote.as_str(), "origin"), (merge.as_str(), pr_ref)] {
        if let Err(error) = git(repo, &["config", key, value]).await {
            tracing::warn!(
                repo = %repo.display(),
                branch,
                error = %error,
                "could not point a fork pull request's branch at its PR ref"
            );
            return;
        }
    }
}

/// One git config value for `repo`, resolved the way git resolves it —
/// the repo's own `.git/config`, then the user's global file, then the
/// system's — or None when the key is unset (git exits 1 with nothing on
/// stderr), empty, or git itself is missing.
pub async fn config_get(repo: &Path, key: &str) -> Option<String> {
    git(repo, &["config", "--get", key])
        .await
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

pub async fn head_sha(repo: &Path) -> Result<String> {
    Ok(git(repo, &["rev-parse", "HEAD"]).await?.trim().to_string())
}

pub async fn default_branch_name(repo: &Path) -> Option<String> {
    git(
        repo,
        &[
            "symbolic-ref",
            "--quiet",
            "--short",
            "refs/remotes/origin/HEAD",
        ],
    )
    .await
    .ok()
    .and_then(|out| out.trim().strip_prefix("origin/").map(str::to_string))
    .filter(|branch| !branch.is_empty())
}

pub async fn status_porcelain(repo: &Path) -> Result<String> {
    git(
        repo,
        &["status", "--porcelain=v1", "--untracked-files=normal"],
    )
    .await
}

/// One path `git status` reports in a checkout.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChangedPath {
    /// Relative to the checkout.
    pub path: PathBuf,
    /// A file git does not track (`??`), rather than an edit to one it does.
    pub untracked: bool,
}

/// The paths `git status` reports in a checkout — uncommitted edits,
/// untracked files, and submodules with changes or new commits of their
/// own — relative to it. NUL-separated porcelain, so a name with a space
/// or a newline survives; a rename reports its new name only.
pub async fn changed_paths(worktree: &Path) -> Result<Vec<ChangedPath>> {
    let out = git_read(
        worktree,
        &["status", "--porcelain=v1", "-z", "--untracked-files=normal"],
    )
    .await?;
    Ok(parse_status_z(&out))
}

/// Pure core of [`changed_paths`].
fn parse_status_z(out: &str) -> Vec<ChangedPath> {
    let mut paths = Vec::new();
    let mut entries = out.split('\0').filter(|e| !e.is_empty());
    while let Some(entry) = entries.next() {
        let Some((xy, path)) = entry.split_at_checked(3) else {
            continue;
        };
        paths.push(ChangedPath {
            path: PathBuf::from(path),
            untracked: xy.starts_with("??"),
        });
        // A rename or copy is followed by the name it came from.
        if xy.starts_with(['R', 'C']) {
            entries.next();
        }
    }
    paths
}

/// What a checkout's commits are measured against when the DELETE CHECK
/// asks what it holds that its base lacks: the WORKTREE BASE BRANCH
/// setting when the repo has it (origin's copy first, as a cut takes it),
/// else origin's default branch, else the ROOT WORKTREE's branch. Local
/// refs only — nothing is fetched to answer a confirm.
pub async fn comparison_base(repo: &Path, configured: Option<&str>) -> Option<String> {
    if let Some(name) = configured {
        if let Some(remote) = origin_branch(repo, name).await {
            return Some(remote);
        }
        if local_branch(repo, name).await {
            return Some(name.to_string());
        }
    }
    if let Ok(out) = git(
        repo,
        &["symbolic-ref", "-q", "--short", "refs/remotes/origin/HEAD"],
    )
    .await
    {
        let short = out.trim();
        if !short.is_empty() {
            return Some(short.to_string());
        }
    }
    git(repo, &["symbolic-ref", "-q", "--short", "HEAD"])
        .await
        .ok()
        .map(|out| out.trim().to_string())
        .filter(|branch| !branch.is_empty())
}

/// How many of `head`'s commits have not landed on `base`, counted by
/// content, not ancestry: a branch landed by a squash merge, a
/// rebase-and-merge or a cherry-pick has commits `base..head` still lists
/// forever, though nothing in them is missing from `base`. `git cherry`
/// drops each commit whose patch `base` already has (patch-id equality);
/// when some remain, one more test catches the squash merge, whose single
/// commit matches none of them: merging `head` into `base` would change
/// nothing (`git merge-tree --write-tree` gives `base`'s own tree). That
/// test runs only when `cherry` found something, and any trouble with it —
/// a conflict, a git too old for `--write-tree` — counts the commits as
/// unlanded, never as landed. `dir` is any checkout of the repo. The
/// merge writes its tree's objects to the store, as any merge would; it
/// moves no ref and touches no checkout.
pub async fn unlanded_commits(dir: &Path, base: &str, head: &str) -> Result<u32> {
    let out = git(dir, &["cherry", base, head]).await?;
    let unlanded = out.lines().filter(|l| l.starts_with('+')).count();
    if unlanded == 0 || merges_to_nothing(dir, base, head).await {
        return Ok(0);
    }
    Ok(u32::try_from(unlanded).unwrap_or(u32::MAX))
}

/// Would merging `head` into `base` leave `base`'s tree exactly as it is?
async fn merges_to_nothing(dir: &Path, base: &str, head: &str) -> bool {
    let Ok(merged) = git(dir, &["merge-tree", "--write-tree", base, head]).await else {
        return false;
    };
    let base_tree = format!("{base}^{{tree}}");
    let Ok(tree) = git(dir, &["rev-parse", "--verify", &base_tree]).await else {
        return false;
    };
    let merged = merged.lines().next().unwrap_or("").trim();
    !merged.is_empty() && merged == tree.trim()
}

/// The branch a checkout is on; None on a detached HEAD, or when git
/// cannot say.
pub async fn current_branch(worktree: &Path) -> Option<String> {
    git(worktree, &["symbolic-ref", "-q", "--short", "HEAD"])
        .await
        .ok()
        .map(|out| out.trim().to_string())
        .filter(|branch| !branch.is_empty())
}

/// Is `branch` one a delete must never take with a worktree: the base its
/// commits are measured against (or the local branch of the same name),
/// origin's default branch, or the branch the ROOT WORKTREE has out?
pub async fn protected_branch(repo: &Path, branch: &str, base: Option<&str>) -> bool {
    base.is_some_and(|base| base == branch || base.strip_prefix("origin/") == Some(branch))
        || default_branch_name(repo).await.as_deref() == Some(branch)
        || current_branch(repo).await.as_deref() == Some(branch)
}

/// The branch a checkout is on and the commit at its tip; None on a
/// detached HEAD, or when git cannot say.
pub async fn branch_tip(worktree: &Path) -> Option<nebula_core::BranchTip> {
    let name = current_branch(worktree).await?;
    let full = format!("refs/heads/{name}");
    let oid = git(worktree, &["rev-parse", "--verify", "--quiet", &full])
        .await
        .ok()?;
    Some(nebula_core::BranchTip {
        name,
        oid: oid.trim().to_string(),
    })
}

/// Delete local branch `tip.name` only while its tip is still `tip.oid`:
/// `git update-ref -d` compares and deletes in one step, so a commit that
/// lands on the branch between the check and the delete keeps it. Unlike
/// `git branch -d`, it does not refuse a branch some checkout has out —
/// callers check that first. The branch's config section goes with it,
/// as `git branch -D` would take it. `--no-deref`: a branch that is a
/// symbolic ref (`alias -> main`) goes itself, never the branch it
/// points at.
pub async fn delete_branch_at(repo: &Path, tip: &nebula_core::BranchTip) -> Result<()> {
    let full = format!("refs/heads/{}", tip.name);
    git(repo, &["update-ref", "--no-deref", "-d", &full, &tip.oid])
        .await
        .context("its tip moved since the confirm")?;
    let section = format!("branch.{}", tip.name);
    let _ = git(repo, &["config", "--remove-section", &section]).await;
    Ok(())
}

/// How many commits in `worktree`'s submodule stores nothing else keeps:
/// commits reachable from a store's HEAD, local branches, tags or stash
/// that no remote-tracking ref of it holds and the ROOT WORKTREE's store
/// for the same submodule (`root`'s) lacks too. A linked worktree's
/// submodules live in its own git dir (`.git/worktrees/<name>/modules/…`),
/// which `git worktree remove` deletes — so a superproject commit can pin
/// a submodule commit that exists nowhere else. The stores counted are
/// the ones the delete takes: every store under the worktree's own git
/// dir — a submodule deinitialised there, or absent at this commit, keeps
/// its store, commits and all — and the store of every submodule checked
/// out in it (nested ones too), when that lives inside the worktree or its
/// git dir. Never the main repository's `.git/modules`, which a worktree
/// delete leaves alone. The stores are asked [`SUBMODULE_JOBS`] at a time:
/// a repo with two dozen submodules is most of what a confirm waits for.
pub async fn submodule_unpushed(worktree: &Path, root: &Path) -> Result<u32> {
    let own_git = PathBuf::from(
        git_read(worktree, &["rev-parse", "--absolute-git-dir"])
            .await?
            .trim(),
    );
    let own_git = std::fs::canonicalize(&own_git).unwrap_or(own_git);
    let checkout = std::fs::canonicalize(worktree).unwrap_or_else(|_| worktree.to_path_buf());
    // Each store once, by the name a band names it by when it fails: its
    // path in the checkout when one is checked out there, else its name
    // under `modules/`.
    let mut stores: std::collections::BTreeMap<PathBuf, String> = Default::default();
    let modules = own_git.join("modules");
    let mut found = Vec::new();
    collect_stores(&modules, &mut found)?;
    for store in found {
        let name = store
            .strip_prefix(&modules)
            .map(module_name)
            .unwrap_or_else(|_| store.display().to_string());
        stores.insert(store, name);
    }
    if worktree.join(".gitmodules").exists() {
        let mut dirs: Vec<PathBuf> = checked_out_submodules(worktree)
            .await?
            .into_iter()
            .collect();
        while let Some(rel) = dirs.pop() {
            let dir = worktree.join(&rel);
            let label = rel.display().to_string();
            if let Some(store) = store_of(&dir) {
                stores.insert(store, label.clone());
            }
            if dir.join(".gitmodules").exists() {
                let nested = checked_out_submodules(&dir)
                    .await
                    .map_err(|e| SubmoduleError::wrap(&label, e))?;
                dirs.extend(nested.into_iter().map(|p| rel.join(p)));
            }
        }
    }
    // Only what the delete takes.
    stores.retain(|s, _| s.starts_with(&own_git) || s.starts_with(&checkout));
    let mut stores: Vec<(PathBuf, String)> = stores.into_iter().collect();
    let root_git = if root == worktree {
        None
    } else {
        let out = git_read(root, &["rev-parse", "--absolute-git-dir"]).await?;
        let dir = PathBuf::from(out.trim());
        Some(std::fs::canonicalize(&dir).unwrap_or(dir))
    };
    let mut jobs = tokio::task::JoinSet::new();
    let mut total: u32 = 0;
    loop {
        while jobs.len() < SUBMODULE_JOBS {
            let Some((store, label)) = stores.pop() else {
                break;
            };
            let kept = root_git
                .as_deref()
                .and_then(|root_git| kept_store(&store, &own_git, &checkout, root, root_git));
            let cwd = checkout.clone();
            jobs.spawn(async move {
                lost_in_store(cwd, store, kept)
                    .await
                    .map_err(|e| SubmoduleError::wrap(&label, e))
            });
        }
        let Some(done) = jobs.join_next().await else {
            break;
        };
        let lost = done.context("submodule check")??;
        total = total.saturating_add(lost);
    }
    Ok(total)
}

/// A store's name under a git dir's `modules/` (`lib/modules/inner` is
/// `lib/inner`): what a band calls a submodule that is not checked out.
fn module_name(rel: &Path) -> String {
    let mut parts = Vec::new();
    let mut skip = false;
    for part in rel.components() {
        let part = part.as_os_str().to_string_lossy();
        // Every other `modules` level is a store's own `modules/`.
        if skip && part == "modules" {
            skip = false;
            continue;
        }
        skip = true;
        parts.push(part.into_owned());
    }
    parts.join("/")
}

/// [`submodule_unpushed`] could not read one submodule: which one, by its
/// path in the checkout (or its name), for a band to say — a checkout in
/// the middle of `git submodule update` reads this way for a moment.
#[derive(Debug)]
pub struct SubmoduleError {
    pub path: String,
    message: String,
}

impl SubmoduleError {
    fn wrap(path: &str, e: anyhow::Error) -> anyhow::Error {
        anyhow::Error::new(Self {
            path: path.to_string(),
            message: format!("{e:#}"),
        })
    }
}

impl std::fmt::Display for SubmoduleError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "submodule {}: {}", self.path, self.message)
    }
}

impl std::error::Error for SubmoduleError {}

/// Stores asked about at once by [`submodule_unpushed`].
const SUBMODULE_JOBS: usize = 8;

/// Every submodule store under `modules` (a git dir's `modules/`), nested
/// ones too: a directory with a `HEAD` and an `objects/` is a store, and
/// its own `modules/` holds its submodules'; any other directory is a
/// level of a submodule name with a `/` in it. Symbolic links are not
/// followed: a store is a real directory, and a link could loop. A
/// `modules/` that is not there holds nothing; one that cannot be read is
/// an error, never "nothing to lose".
fn collect_stores(modules: &Path, stores: &mut Vec<PathBuf>) -> Result<()> {
    let entries = match std::fs::read_dir(modules) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e).with_context(|| format!("read {}", modules.display())),
    };
    for entry in entries {
        let entry = entry.with_context(|| format!("read {}", modules.display()))?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let dir = entry.path();
        if dir.join("HEAD").is_file() && dir.join("objects").is_dir() {
            collect_stores(&dir.join("modules"), stores)?;
            stores.push(std::fs::canonicalize(&dir).unwrap_or(dir));
        } else {
            collect_stores(&dir, stores)?;
        }
    }
    Ok(())
}

/// The store of the submodule checked out at `dir`: its `.git` directory,
/// or where its `.git` file (`gitdir: …`) points.
pub(crate) fn store_of(dir: &Path) -> Option<PathBuf> {
    let dot_git = dir.join(".git");
    let store = if dot_git.is_dir() {
        dot_git
    } else {
        let text = std::fs::read_to_string(&dot_git).ok()?;
        dir.join(text.trim().strip_prefix("gitdir:")?.trim())
    };
    std::fs::canonicalize(store).ok()
}

/// The ROOT WORKTREE's store for the submodule whose store in the worktree
/// is `store`: the same place under the root's git dir, or — for a store
/// inside the worktree's own folder — the store of the root's checkout at
/// the same path. None when the root has none.
fn kept_store(
    store: &Path,
    own_git: &Path,
    checkout: &Path,
    root: &Path,
    root_git: &Path,
) -> Option<PathBuf> {
    if let Ok(rel) = store.strip_prefix(own_git) {
        let kept = root_git.join(rel);
        return kept.join("HEAD").is_file().then_some(kept);
    }
    let rel = store.strip_prefix(checkout).ok()?;
    store_of(&root.join(rel.parent()?))
}

/// What one store holds that no remote does and `kept` (the root's store
/// for the same submodule) lacks: its HEAD, its local branches and tags (a
/// commit on a branch, then HEAD detached back to the pin, is still lost
/// with the store) and its stash. `--glob` matches nothing when there is
/// no stash; the bracket keeps git from reading it as `refs/stash/*`.
async fn lost_in_store(cwd: PathBuf, store: PathBuf, kept: Option<PathBuf>) -> Result<u32> {
    let git_dir = format!("--git-dir={}", store.display());
    let refs = [
        "--branches",
        "--tags",
        "--glob=refs/stas[h]",
        "--not",
        "--remotes",
    ];
    let with_head: Vec<&str> = [git_dir.as_str(), "rev-list", "HEAD"]
        .into_iter()
        .chain(refs)
        .collect();
    let lost = match git_read(&cwd, &with_head).await {
        Ok(lost) => lost,
        // A store whose HEAD names a branch with no commit yet — a clone
        // `git submodule update` is still writing — holds nothing at HEAD
        // to lose: ask again about its refs alone. Any other failure, and
        // a failure of that, stands: a store git cannot read is never
        // read as empty.
        Err(e) if !head_resolves(&cwd, &git_dir).await => {
            let refs_only: Vec<&str> = [git_dir.as_str(), "rev-list"]
                .into_iter()
                .chain(refs)
                .collect();
            git_read(&cwd, &refs_only).await.map_err(|_| e)?
        }
        Err(e) => return Err(e),
    };
    let n = if lost.trim().is_empty() {
        0
    } else if let Some(kept) = kept {
        unkept_in(&cwd, &kept, &lost).await?
    } else {
        lost.lines().count()
    };
    Ok(u32::try_from(n).unwrap_or(u32::MAX))
}

/// Does the store at `git_dir` (a `--git-dir=` argument) have a HEAD
/// that names a commit? No on an unborn branch, as in a clone not yet
/// fetched.
async fn head_resolves(cwd: &Path, git_dir: &str) -> bool {
    git_read(
        cwd,
        &[git_dir, "rev-parse", "--verify", "--quiet", "HEAD^{commit}"],
    )
    .await
    .is_ok()
}

/// How many of `commits` (one id per line) the root's store `kept` does
/// not keep: commits it lacks, and commits it has but no ref of it
/// reaches — a commit fetched by id and left dangling goes at its next
/// gc, so it is no keeper.
async fn unkept_in(cwd: &Path, kept: &Path, commits: &str) -> Result<usize> {
    let kept_dir = format!("--git-dir={}", kept.display());
    let answers = git_read_input(cwd, &[&kept_dir, "cat-file", "--batch-check"], commits).await?;
    let mut missing = 0;
    let mut present = String::new();
    for line in answers.lines() {
        match line.split_once(' ') {
            Some((_, "missing")) | None => missing += 1,
            Some((id, _)) => {
                present.push_str(id);
                present.push('\n');
            }
        }
    }
    if present.is_empty() {
        return Ok(missing);
    }
    // Of those it has, the ones none of its refs reach. The walk also
    // lists their unreached ancestors; only the asked-about ones count.
    let unreached = git_read_input(
        cwd,
        &[&kept_dir, "rev-list", "--stdin", "--not", "--all"],
        &present,
    )
    .await?;
    let asked: HashSet<&str> = present.lines().collect();
    let dangling = unreached.lines().filter(|id| asked.contains(id)).count();
    Ok(missing + dangling)
}

/// The submodules checked out in `dir`, relative to it: the gitlinks its
/// index records whose folder has a `.git` of its own — read straight off
/// `git ls-files --stage`, much quicker than `git submodule status`.
async fn checked_out_submodules(dir: &Path) -> Result<Vec<PathBuf>> {
    let out = git_read(dir, &["ls-files", "-z", "--stage"]).await?;
    Ok(gitlinks(&out)
        .into_iter()
        .filter(|p| dir.join(p).join(".git").exists())
        .collect())
}

/// Pure core of [`checked_out_submodules`]: the gitlinks (mode `160000`)
/// among `ls-files -z --stage` entries (`<mode> <oid> <stage>\t<path>`).
fn gitlinks(out: &str) -> Vec<PathBuf> {
    out.split('\0')
        .filter_map(|entry| {
            let (meta, path) = entry.split_once('\t')?;
            meta.starts_with("160000 ").then(|| PathBuf::from(path))
        })
        .collect()
}

/// Gitignored output a delete costs nothing to lose: what a build, an
/// install or the OS regenerates. Matched against an entry's last path
/// component, so a nested `crates/x/target/` counts too. Anything else
/// ignored — `.env`, an editor's local settings — is someone's, and the
/// delete takes it.
const REGENERABLE: &[&str] = &[
    "target",
    "node_modules",
    "dist",
    "build",
    "out",
    ".next",
    "__pycache__",
    ".venv",
    "venv",
    ".gradle",
    ".cache",
    ".DS_Store",
];

/// The gitignored files and folders in a checkout that are not build
/// output ([`REGENERABLE`]), relative to it: `git ls-files --others
/// --ignored --directory`, so a whole ignored folder is one entry.
pub async fn ignored_keepsakes(worktree: &Path) -> Result<Vec<String>> {
    let out = git_read(
        worktree,
        &[
            "ls-files",
            "-z",
            "--others",
            "--ignored",
            "--exclude-standard",
            "--directory",
        ],
    )
    .await?;
    Ok(keepsakes(&out))
}

/// Pure core of [`ignored_keepsakes`].
fn keepsakes(out: &str) -> Vec<String> {
    out.split('\0')
        .filter(|entry| !entry.is_empty())
        .filter(|entry| {
            let name = entry.trim_end_matches('/').rsplit('/').next().unwrap_or("");
            !REGENERABLE.contains(&name)
        })
        .map(str::to_string)
        .collect()
}

/// Is the checkout's HEAD detached — on no branch at all?
pub async fn head_detached(worktree: &Path) -> bool {
    git(worktree, &["symbolic-ref", "-q", "HEAD"])
        .await
        .is_err()
}

/// How many of HEAD's commits no branch and no remote-tracking ref holds:
/// on a detached HEAD, what only the reflog would keep once the checkout
/// is gone.
pub async fn unreferenced_commits(worktree: &Path) -> Result<u32> {
    count(
        git(
            worktree,
            &[
                "rev-list",
                "--count",
                "HEAD",
                "--not",
                "--branches",
                "--remotes",
            ],
        )
        .await?,
    )
}

fn count(out: String) -> Result<u32> {
    out.trim()
        .parse()
        .map_err(|_| anyhow!("git printed no count: {}", out.trim()))
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
        // Locked (`git worktree lock`): often by an agent that locks the
        // checkout it works in and never unlocks when killed, but anyone
        // can lock one to keep it. A delete the person asked for was
        // confirmed with the lock and its reason in front of them (the
        // DELETE CHECK names it, to a client that reads it); auto-cleanup
        // never takes a locked checkout. So unlock and retry rather than
        // surfacing git's refusal.
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
    fn parse_worktree_list_leaves_out_checkouts_deleted_without_git() {
        // The `gone` stanza is git's porcelain after `rm -rf` on a linked
        // worktree (git 2.31+ prints `prunable`, with a reason). The
        // `bare-mark` one is hand-written: it guards against a git that
        // prints the mark with no reason.
        let porcelain = "worktree /repo\n\
                         HEAD 0123456789abcdef0123456789abcdef01234567\n\
                         branch refs/heads/main\n\
                         \n\
                         worktree /repo-worktrees/gone\n\
                         HEAD 0123456789abcdef0123456789abcdef01234567\n\
                         branch refs/heads/gone\n\
                         prunable gitdir file points to non-existent location\n\
                         \n\
                         worktree /repo-worktrees/bare-mark\n\
                         HEAD 0123456789abcdef0123456789abcdef01234567\n\
                         detached\n\
                         prunable\n\
                         \n\
                         worktree /repo-worktrees/kept\n\
                         HEAD fedcba9876543210fedcba9876543210fedcba98\n\
                         branch refs/heads/kept\n";
        let entries = parse_worktree_list(porcelain);
        let got: Vec<(&Path, &str)> = entries
            .iter()
            .map(|e| (e.path.as_path(), e.branch.as_str()))
            .collect();
        assert_eq!(
            got,
            vec![
                (Path::new("/repo"), "main"),
                (Path::new("/repo-worktrees/kept"), "kept"),
            ]
        );
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

    /// A rebase parks HEAD on the commits it replays, so for as long as it
    /// sits on a conflict `git worktree list` calls the checkout detached.
    /// The row must keep its branch name through that: the branch is coming
    /// back, and the worktree sync would otherwise rename the row twice per
    /// rebase and hide it from every lookup keyed on the name meanwhile.
    #[tokio::test]
    async fn a_paused_rebase_keeps_the_worktree_on_its_branch() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("repo");
        std::fs::create_dir(&repo).unwrap();
        init_repo(&repo).await;
        std::fs::write(repo.join("f"), "base\n").unwrap();
        git(&repo, &["add", "f"]).await.unwrap();
        git(&repo, &["commit", "-m", "base"]).await.unwrap();
        let wt = add_worktree(&repo, "topic", None).await.unwrap();
        // Both sides rewrite the same line, so the rebase has to stop.
        std::fs::write(wt.join("f"), "topic\n").unwrap();
        git(&wt, &["commit", "-am", "topic"]).await.unwrap();
        std::fs::write(repo.join("f"), "main\n").unwrap();
        git(&repo, &["commit", "-am", "main"]).await.unwrap();
        assert!(git(&wt, &["rebase", "main"]).await.is_err());
        // Precondition: git itself now reports no current branch there.
        let current = git(&wt, &["branch", "--show-current"]).await.unwrap();
        assert!(
            current.trim().is_empty(),
            "expected a detached HEAD mid-rebase"
        );

        // Paths come back canonical from git; the tempdir may be a symlink.
        let wt_canon = wt.canonicalize().unwrap();
        let branch_of = |entries: &[WorktreeEntry]| {
            entries
                .iter()
                .find(|e| e.path.canonicalize().ok() == Some(wt_canon.clone()))
                .map(|e| e.branch.clone())
                .expect("the worktree is listed")
        };

        let entries = list_worktrees(&repo).await.unwrap();
        assert_eq!(
            branch_of(&entries),
            "topic",
            "mid-rebase: still the branch's row"
        );

        git(&wt, &["rebase", "--abort"]).await.unwrap();
        let entries = list_worktrees(&repo).await.unwrap();
        assert_eq!(
            branch_of(&entries),
            "topic",
            "after: the ordinary branch line"
        );

        // A checkout that genuinely detached still says so.
        git(&wt, &["checkout", "--detach"]).await.unwrap();
        let entries = list_worktrees(&repo).await.unwrap();
        assert!(
            branch_of(&entries).starts_with("detached @ "),
            "a real detached HEAD is still labelled as one, got {:?}",
            branch_of(&entries)
        );
    }

    /// A bare `origin` beside `repo`, with `repo`'s `main` pushed and a
    /// `feat-x` branch that exists only there — the shape of a same-repo
    /// pull request whose branch nobody has fetched yet.
    /// Someone else lands a commit on origin's `main` from their own
    /// clone — nothing here fetches. The landed commit's sha.
    async fn land_on_origin(tmp: &Path, origin: &Path) -> String {
        let other = tmp.join("other");
        git(
            tmp,
            &[
                "clone",
                "-q",
                origin.to_str().unwrap(),
                other.to_str().unwrap(),
            ],
        )
        .await
        .unwrap();
        git(&other, &["config", "user.email", "t@t"]).await.unwrap();
        git(&other, &["config", "user.name", "t"]).await.unwrap();
        git(
            &other,
            &["commit", "--allow-empty", "-m", "landed elsewhere"],
        )
        .await
        .unwrap();
        git(&other, &["push", "-q", "origin", "main"])
            .await
            .unwrap();
        git(&other, &["rev-parse", "HEAD"]).await.unwrap()
    }

    async fn add_bare_origin(repo: &Path, tmp: &Path) -> PathBuf {
        let origin = tmp.join("origin.git");
        std::fs::create_dir(&origin).unwrap();
        git(&origin, &["init", "--bare", "-b", "main"])
            .await
            .unwrap();
        let origin_str = origin.to_string_lossy().into_owned();
        git(repo, &["remote", "add", "origin", &origin_str])
            .await
            .unwrap();
        git(repo, &["push", "-u", "origin", "main"]).await.unwrap();
        git(repo, &["branch", "feat-x", "main"]).await.unwrap();
        git(repo, &["push", "origin", "feat-x"]).await.unwrap();
        git(repo, &["branch", "-D", "feat-x"]).await.unwrap();
        origin
    }

    /// A same-repo PR: the branch comes from `origin` and the new checkout
    /// tracks it, so a `git push` from a PR SESSION lands on the PR.
    #[tokio::test]
    async fn add_pr_worktree_tracks_the_branch_on_origin() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("repo");
        std::fs::create_dir(&repo).unwrap();
        init_repo(&repo).await;
        add_bare_origin(&repo, tmp.path()).await;

        let wt = add_pr_worktree(&repo, 7, "feat-x").await.unwrap();
        assert_eq!(wt, worktree_dir(&repo, "feat-x"));
        let branch = git(&wt, &["branch", "--show-current"]).await.unwrap();
        assert_eq!(branch.trim(), "feat-x");
        let upstream = git(&wt, &["rev-parse", "--abbrev-ref", "feat-x@{upstream}"])
            .await
            .unwrap();
        assert_eq!(upstream.trim(), "origin/feat-x");
    }

    /// A fork PR: `origin` has no branch of that name, only the PR ref, so
    /// the checkout is seeded from `refs/pull/N/head` under the head's name.
    #[tokio::test]
    async fn add_pr_worktree_seeds_a_fork_branch_from_the_pr_ref() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("repo");
        std::fs::create_dir(&repo).unwrap();
        init_repo(&repo).await;
        let origin = add_bare_origin(&repo, tmp.path()).await;
        // The fork's commit reaches origin only as the PR ref.
        git(&repo, &["commit", "--allow-empty", "-m", "fork work"])
            .await
            .unwrap();
        git(&repo, &["push", "origin", "HEAD:refs/pull/9/head"])
            .await
            .unwrap();
        git(&repo, &["reset", "--hard", "origin/main"])
            .await
            .unwrap();
        let expected = git(&origin, &["rev-parse", "refs/pull/9/head"])
            .await
            .unwrap();

        let wt = add_pr_worktree(&repo, 9, "their-fix").await.unwrap();
        let branch = git(&wt, &["branch", "--show-current"]).await.unwrap();
        assert_eq!(branch.trim(), "their-fix");
        let head = git(&wt, &["rev-parse", "HEAD"]).await.unwrap();
        assert_eq!(head.trim(), expected.trim());
        // Its upstream is the PR ref, as `gh pr checkout` leaves a fork's:
        // what `git pull` and `gh pr view` in the checkout go by.
        assert_eq!(
            config_get(&repo, "branch.their-fix.remote")
                .await
                .as_deref(),
            Some("origin")
        );
        assert_eq!(
            config_get(&repo, "branch.their-fix.merge").await.as_deref(),
            Some("refs/pull/9/head")
        );

        // Neither route: no such PR, no such branch anywhere.
        let err = add_pr_worktree(&repo, 10, "nowhere").await.unwrap_err();
        assert!(err.to_string().contains("#10"), "{err}");
    }

    /// A pull request reviewed before: its worktree was deleted, its branch
    /// kept, and the author has pushed since. The new checkout comes up on
    /// what the pull request holds now, not on the commits of the last
    /// review — fast-forwarded, since a delete-and-recut leaves nothing of
    /// the user's on the branch.
    #[tokio::test]
    async fn add_pr_worktree_brings_a_kept_branch_up_to_the_fetched_tip() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("repo");
        std::fs::create_dir(&repo).unwrap();
        init_repo(&repo).await;
        let origin = add_bare_origin(&repo, tmp.path()).await;

        let wt = add_pr_worktree(&repo, 7, "feat-x").await.unwrap();
        let reviewed = git(&wt, &["rev-parse", "HEAD"]).await.unwrap();
        remove_worktree(&repo, &wt, false).await.unwrap();
        assert!(local_branch(&repo, "feat-x").await, "the delete keeps it");

        // The author pushes another commit to the pull request's branch.
        git(&repo, &["checkout", "-q", "-b", "author", "origin/feat-x"])
            .await
            .unwrap();
        git(&repo, &["commit", "--allow-empty", "-m", "review feedback"])
            .await
            .unwrap();
        git(&repo, &["push", "-q", "origin", "author:feat-x"])
            .await
            .unwrap();
        git(&repo, &["checkout", "-q", "main"]).await.unwrap();
        git(&repo, &["branch", "-D", "author"]).await.unwrap();
        let pushed = git(&origin, &["rev-parse", "refs/heads/feat-x"])
            .await
            .unwrap();
        assert_ne!(reviewed.trim(), pushed.trim());

        let wt = add_pr_worktree(&repo, 7, "feat-x").await.unwrap();
        let head = git(&wt, &["rev-parse", "HEAD"]).await.unwrap();
        assert_eq!(
            head.trim(),
            pushed.trim(),
            "what the pull request holds now"
        );
    }

    /// A fork's pull request from its own `main`: the client names the
    /// checkout's branch for its owner (`someone/main`), so it is neither
    /// the ROOT WORKTREE's `main` nor `origin/main` — the checkout holds
    /// the contributor's commit, on its own branch, in its own directory,
    /// and the root is left exactly where it was. A later `git pull` there
    /// follows the pull request.
    #[tokio::test]
    async fn add_pr_worktree_keeps_a_forks_main_apart_from_ours() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("repo");
        std::fs::create_dir(&repo).unwrap();
        init_repo(&repo).await;
        let origin = add_bare_origin(&repo, tmp.path()).await;
        git(&repo, &["commit", "--allow-empty", "-m", "fork work"])
            .await
            .unwrap();
        git(&repo, &["push", "origin", "HEAD:refs/pull/129/head"])
            .await
            .unwrap();
        git(&repo, &["reset", "--hard", "origin/main"])
            .await
            .unwrap();
        let ours = git(&repo, &["rev-parse", "HEAD"]).await.unwrap();
        let theirs = git(&origin, &["rev-parse", "refs/pull/129/head"])
            .await
            .unwrap();
        assert_ne!(ours.trim(), theirs.trim());

        let wt = add_pr_worktree(&repo, 129, "someone/main").await.unwrap();
        assert_eq!(wt, worktree_dir(&repo, "someone/main"));
        assert!(wt.ends_with("someone-main"), "{}", wt.display());
        let branch = git(&wt, &["branch", "--show-current"]).await.unwrap();
        assert_eq!(branch.trim(), "someone/main");
        let head = git(&wt, &["rev-parse", "HEAD"]).await.unwrap();
        assert_eq!(head.trim(), theirs.trim(), "the contributor's commit");
        let root = git(&repo, &["rev-parse", "HEAD"]).await.unwrap();
        assert_eq!(root.trim(), ours.trim(), "the root is where it was");
        let root_branch = git(&repo, &["branch", "--show-current"]).await.unwrap();
        assert_eq!(root_branch.trim(), "main");

        // The contributor pushes again, on top of their first commit:
        // `git pull` in the checkout follows.
        git(&repo, &["reset", "--hard", theirs.trim()])
            .await
            .unwrap();
        git(&repo, &["commit", "--allow-empty", "-m", "more fork work"])
            .await
            .unwrap();
        git(&repo, &["push", "-f", "origin", "HEAD:refs/pull/129/head"])
            .await
            .unwrap();
        git(&repo, &["reset", "--hard", "origin/main"])
            .await
            .unwrap();
        let newer = git(&origin, &["rev-parse", "refs/pull/129/head"])
            .await
            .unwrap();
        git(&wt, &["pull", "--ff-only"]).await.unwrap();
        let head = git(&wt, &["rev-parse", "HEAD"]).await.unwrap();
        assert_eq!(head.trim(), newer.trim());
    }

    /// No `origin`, no default base: the branch starts at HEAD, as before.
    #[tokio::test]
    async fn default_base_is_none_without_an_origin() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("repo");
        std::fs::create_dir(&repo).unwrap();
        init_repo(&repo).await;
        assert_eq!(default_base(&repo).await, None);
        let wt = add_worktree_off_default(&repo, "feat").await.unwrap();
        let head = git(&wt, &["rev-parse", "HEAD"]).await.unwrap();
        let root = git(&repo, &["rev-parse", "HEAD"]).await.unwrap();
        assert_eq!(head, root);
    }

    /// Offline (here: an origin that does not exist) the fetch fails and
    /// the branch starts at HEAD rather than not at all.
    #[tokio::test]
    async fn default_base_falls_back_to_head_when_the_fetch_fails() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("repo");
        std::fs::create_dir(&repo).unwrap();
        init_repo(&repo).await;
        let gone = tmp.path().join("gone.git");
        git(&repo, &["remote", "add", "origin", gone.to_str().unwrap()])
            .await
            .unwrap();
        assert_eq!(default_base(&repo).await, None);
        let wt = add_worktree_off_default(&repo, "feat").await.unwrap();
        let head = git(&wt, &["rev-parse", "HEAD"]).await.unwrap();
        let root = git(&repo, &["rev-parse", "HEAD"]).await.unwrap();
        assert_eq!(head, root);
    }

    /// The checkout is a commit behind `origin/main` and has not fetched
    /// since: a worktree nobody named a base for still starts at what
    /// origin has right now, and its branch does not track `origin/main`.
    #[tokio::test]
    async fn a_worktree_off_the_default_base_starts_at_the_fetched_origin_head() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("repo");
        std::fs::create_dir(&repo).unwrap();
        init_repo(&repo).await;
        let origin = add_bare_origin(&repo, tmp.path()).await;
        let landed = land_on_origin(tmp.path(), &origin).await;
        let local = git(&repo, &["rev-parse", "HEAD"]).await.unwrap();
        assert_ne!(landed, local, "the checkout is behind origin");
        // `git remote add` + a push leave no origin/HEAD symref behind;
        // resolving it is default_base's job, not the user's.
        assert!(
            git(&repo, &["symbolic-ref", "-q", "refs/remotes/origin/HEAD"])
                .await
                .is_err()
        );

        assert_eq!(default_base(&repo).await.as_deref(), Some("origin/main"));
        let wt = add_worktree_off_default(&repo, "feat").await.unwrap();
        let head = git(&wt, &["rev-parse", "HEAD"]).await.unwrap();
        assert_eq!(head, landed, "starts at origin's main, not the checkout's");
        assert!(
            git(&wt, &["rev-parse", "--abbrev-ref", "feat@{upstream}"])
                .await
                .is_err(),
            "a new branch must not track origin/main"
        );
    }

    /// `--base main` with the checkout's `main` a commit behind origin's:
    /// the branch starts at origin's main, never the local branch of that
    /// name, and does not track it.
    #[tokio::test]
    async fn a_named_branch_base_means_origins_copy_not_the_local_branch() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("repo");
        std::fs::create_dir(&repo).unwrap();
        init_repo(&repo).await;
        let origin = add_bare_origin(&repo, tmp.path()).await;
        let landed = land_on_origin(tmp.path(), &origin).await;
        let local_main = git(&repo, &["rev-parse", "main"]).await.unwrap();
        assert_ne!(landed, local_main, "the local main is behind origin's");

        let wt = add_worktree_off_ref(&repo, "feat", "main").await.unwrap();
        let head = git(&wt, &["rev-parse", "HEAD"]).await.unwrap();
        assert_eq!(
            head, landed,
            "starts at origin's main, not the local branch"
        );
        assert!(
            git(&wt, &["rev-parse", "--abbrev-ref", "feat@{upstream}"])
                .await
                .is_err(),
            "a new branch must not track origin/main"
        );
        // The checkout's own main is untouched.
        let after = git(&repo, &["rev-parse", "main"]).await.unwrap();
        assert_eq!(after, local_main);
    }

    /// A base origin has no branch for is used as named: a tag here, and
    /// `HEAD`, which means this checkout's HEAD even though the fetched
    /// `origin/HEAD` exists.
    #[tokio::test]
    async fn a_named_base_origin_has_no_branch_for_is_used_as_named() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("repo");
        std::fs::create_dir(&repo).unwrap();
        init_repo(&repo).await;
        let origin = add_bare_origin(&repo, tmp.path()).await;
        git(&repo, &["remote", "set-head", "origin", "--auto"])
            .await
            .unwrap();
        let tagged = git(&repo, &["rev-parse", "HEAD"]).await.unwrap();
        git(&repo, &["tag", "v1"]).await.unwrap();
        // The checkout moves on locally; origin moves on separately.
        git(&repo, &["commit", "--allow-empty", "-m", "local only"])
            .await
            .unwrap();
        let local_head = git(&repo, &["rev-parse", "HEAD"]).await.unwrap();
        let landed = land_on_origin(tmp.path(), &origin).await;

        let wt = add_worktree_off_ref(&repo, "hotfix", "v1").await.unwrap();
        let head = git(&wt, &["rev-parse", "HEAD"]).await.unwrap();
        assert_eq!(head, tagged, "a tag is used as named");

        let wt = add_worktree_off_ref(&repo, "spike", "HEAD").await.unwrap();
        let head = git(&wt, &["rev-parse", "HEAD"]).await.unwrap();
        assert_eq!(head, local_head, "HEAD is this checkout's, not origin's");
        assert_ne!(head, landed);
    }

    /// A named base on a branch that already exists without a worktree — a
    /// kept branch of a deleted checkout — keeps the long-standing fallback:
    /// check out the existing branch and ignore the base.
    #[tokio::test]
    async fn a_named_base_on_an_existing_branch_checks_out_that_branch() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("repo");
        std::fs::create_dir(&repo).unwrap();
        init_repo(&repo).await;
        git(&repo, &["tag", "v1"]).await.unwrap();
        git(&repo, &["commit", "--allow-empty", "-m", "hotfix"])
            .await
            .unwrap();
        let hotfix_head = git(&repo, &["rev-parse", "HEAD"]).await.unwrap();
        git(&repo, &["branch", "hotfix"]).await.unwrap();

        let wt = add_worktree_off_ref(&repo, "hotfix", "v1").await.unwrap();
        let head = git(&wt, &["rev-parse", "HEAD"]).await.unwrap();
        let base = git(&repo, &["rev-parse", "v1"]).await.unwrap();
        assert_eq!(head, hotfix_head, "the existing branch is checked out");
        assert_ne!(head, base, "the named base is ignored");
    }

    /// The `worktree_base_branch` SETTING says `main` while the checkout's
    /// `main` is a commit behind origin's: the branch starts at origin's
    /// main, untracked — the same answer `--base main` gives.
    #[tokio::test]
    async fn a_configured_base_means_origins_copy_not_the_local_branch() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("repo");
        std::fs::create_dir(&repo).unwrap();
        init_repo(&repo).await;
        let origin = add_bare_origin(&repo, tmp.path()).await;
        let landed = land_on_origin(tmp.path(), &origin).await;
        let local_main = git(&repo, &["rev-parse", "main"]).await.unwrap();
        assert_ne!(landed, local_main, "the local main is behind origin's");

        let wt = add_worktree_off_configured(&repo, "feat", "main")
            .await
            .unwrap();
        let head = git(&wt, &["rev-parse", "HEAD"]).await.unwrap();
        assert_eq!(head, landed, "starts at origin's main");
        assert!(
            git(&wt, &["rev-parse", "--abbrev-ref", "feat@{upstream}"])
                .await
                .is_err(),
            "a new branch must not track origin/main"
        );
    }

    /// A configured branch origin lacks but the checkout has — a repo with
    /// no origin at all, here — is used as named, from wherever the local
    /// branch points, not from HEAD.
    #[tokio::test]
    async fn a_configured_base_origin_lacks_uses_the_local_branch() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("repo");
        std::fs::create_dir(&repo).unwrap();
        init_repo(&repo).await;
        let master = git(&repo, &["rev-parse", "HEAD"]).await.unwrap();
        git(&repo, &["branch", "master"]).await.unwrap();
        // HEAD moves on along main; master stays where it was.
        git(&repo, &["commit", "--allow-empty", "-m", "later on main"])
            .await
            .unwrap();
        let head_now = git(&repo, &["rev-parse", "HEAD"]).await.unwrap();
        assert_ne!(master, head_now);

        let wt = add_worktree_off_configured(&repo, "feat", "master")
            .await
            .unwrap();
        let head = git(&wt, &["rev-parse", "HEAD"]).await.unwrap();
        assert_eq!(head, master, "starts at the local master, not HEAD");
    }

    /// A configured branch this repo has nowhere — the setting is global,
    /// the repo uses `main` — falls back to the fetched `origin/HEAD`
    /// rather than failing the worktree.
    #[tokio::test]
    async fn a_configured_base_the_repo_lacks_falls_back_to_origin_head() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("repo");
        std::fs::create_dir(&repo).unwrap();
        init_repo(&repo).await;
        let origin = add_bare_origin(&repo, tmp.path()).await;
        let landed = land_on_origin(tmp.path(), &origin).await;
        let local = git(&repo, &["rev-parse", "HEAD"]).await.unwrap();
        assert_ne!(landed, local, "the checkout is behind origin");

        let wt = add_worktree_off_configured(&repo, "feat", "master")
            .await
            .unwrap();
        let head = git(&wt, &["rev-parse", "HEAD"]).await.unwrap();
        assert_eq!(head, landed, "origin's default branch, freshly fetched");
        assert!(
            git(&wt, &["rev-parse", "--abbrev-ref", "feat@{upstream}"])
                .await
                .is_err(),
            "the fallback is untracked like every default-base worktree"
        );
        // And a tag of that name is not a branch: still the fallback.
        git(&repo, &["tag", "release"]).await.unwrap();
        let wt = add_worktree_off_configured(&repo, "feat2", "release")
            .await
            .unwrap();
        let head = git(&wt, &["rev-parse", "HEAD"]).await.unwrap();
        assert_eq!(head, landed, "a tag sharing the name is not the branch");
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
        // The stale registration should be pruned from git's registry too.
        // `list_worktrees` hides a gone checkout whether or not it was
        // pruned, so ask git's raw listing instead.
        let raw = git(&repo, &["worktree", "list", "--porcelain"])
            .await
            .unwrap();
        assert!(!raw.contains("prunable"), "{raw}");
        assert_eq!(raw.matches("worktree ").count(), 1, "{raw}");
    }

    /// A locked checkout is never marked `prunable`, however gone it is, so
    /// the parse alone would keep it. It is still not a checkout anyone can
    /// work in, and the lock is git's business: it is left in place.
    #[tokio::test]
    async fn list_worktrees_leaves_out_a_locked_checkout_that_was_deleted() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("repo");
        std::fs::create_dir(&repo).unwrap();
        init_repo(&repo).await;
        let wt = add_worktree(&repo, "feature", None).await.unwrap();
        git(&repo, &["worktree", "lock", &wt.to_string_lossy()])
            .await
            .unwrap();
        std::fs::remove_dir_all(&wt).unwrap();

        let raw = git(&repo, &["worktree", "list", "--porcelain"])
            .await
            .unwrap();
        assert!(raw.contains("locked") && !raw.contains("prunable"), "{raw}");
        let entries = list_worktrees(&repo).await.unwrap();
        assert_eq!(entries.len(), 1, "only the main checkout: {entries:?}");
        assert!(git(&repo, &["worktree", "list", "--porcelain"])
            .await
            .unwrap()
            .contains("locked"));
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

    #[test]
    fn lock_in_list_reads_the_target_stanzas_lock_and_reason() {
        let out = "worktree /r\nHEAD 1\nbranch refs/heads/main\n\n\
                   worktree /r/a\nHEAD 2\nbranch refs/heads/a\nlocked\n\n\
                   worktree /r/b\nHEAD 3\ndetached\nlocked merge queue: reused by every push\n\n\
                   worktree /r/c\nHEAD 4\nbranch refs/heads/c\nlocked \"two\\nlines \\\"q\\\" caf\\303\\251\"\n\n\
                   worktree /r/d\nHEAD 5\nbranch refs/heads/d\n";
        let lock = |p: &str| lock_in_list(out, |listed| listed == Path::new(p));
        assert_eq!(lock("/r"), None, "the main checkout is not locked");
        assert_eq!(lock("/r/a").as_deref(), Some(""), "locked, no reason");
        assert_eq!(
            lock("/r/b").as_deref(),
            Some("merge queue: reused by every push")
        );
        assert_eq!(lock("/r/c").as_deref(), Some("two\nlines \"q\" café"));
        assert_eq!(lock("/r/d"), None, "another stanza's lock is not its");
        assert_eq!(lock("/r/x"), None, "not listed");
    }

    #[tokio::test]
    async fn worktree_lock_reads_what_git_worktree_lock_wrote() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("repo");
        std::fs::create_dir(&repo).unwrap();
        init_repo(&repo).await;
        let wt = add_worktree(&repo, "feature", None).await.unwrap();
        let wt_str = wt.to_string_lossy().into_owned();
        assert_eq!(worktree_lock(&repo, &wt).await.unwrap(), None);
        git(&repo, &["worktree", "lock", &wt_str]).await.unwrap();
        assert_eq!(
            worktree_lock(&repo, &wt).await.unwrap().as_deref(),
            Some("")
        );
        git(&repo, &["worktree", "unlock", &wt_str]).await.unwrap();
        git(
            &repo,
            &[
                "worktree",
                "lock",
                "--reason",
                "kept for the nightly build",
                &wt_str,
            ],
        )
        .await
        .unwrap();
        assert_eq!(
            worktree_lock(&repo, &wt).await.unwrap().as_deref(),
            Some("kept for the nightly build")
        );
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

    #[test]
    fn status_z_keeps_odd_names_and_drops_rename_sources() {
        let out = " M src/a b.rs\0R  new.rs\0old.rs\0?? notes/\0 M sub\0";
        let parsed = parse_status_z(out);
        let paths: Vec<&Path> = parsed.iter().map(|c| c.path.as_path()).collect();
        assert_eq!(
            paths,
            ["src/a b.rs", "new.rs", "notes/", "sub"].map(Path::new)
        );
        let untracked: Vec<bool> = parsed.iter().map(|c| c.untracked).collect();
        assert_eq!(untracked, [false, false, true, false]);
        assert!(parse_status_z("").is_empty());
    }

    #[test]
    fn ignored_build_output_is_not_a_keepsake() {
        let out = ".env\0target/\0crates/x/target/\0node_modules/\0.claude/settings.local.json\0notes/\0.DS_Store\0";
        assert_eq!(
            keepsakes(out),
            [".env", ".claude/settings.local.json", "notes/"]
        );
    }

    /// On a real checkout: an ignored `.env` is listed, an ignored
    /// `target/` is not.
    #[tokio::test]
    async fn ignored_keepsakes_lists_dot_env_but_not_target() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("repo");
        std::fs::create_dir(&repo).unwrap();
        init_repo(&repo).await;
        std::fs::write(repo.join(".gitignore"), ".env\ntarget/\n").unwrap();
        git(&repo, &["add", ".gitignore"]).await.unwrap();
        git(&repo, &["commit", "-q", "-m", "ignore"]).await.unwrap();
        std::fs::write(repo.join(".env"), "SECRET=1").unwrap();
        std::fs::create_dir_all(repo.join("target/debug")).unwrap();
        std::fs::write(repo.join("target/debug/app"), "bin").unwrap();
        assert_eq!(ignored_keepsakes(&repo).await.unwrap(), [".env"]);
    }

    #[test]
    fn gitlinks_are_the_160000_entries() {
        let out = [
            "100644 0123 0\tsrc/a.rs",
            "160000 4567 0\tlib/a",
            "160000 89ab 0\tlib/with space",
        ]
        .join("\0");
        let out = out.as_str();
        assert_eq!(
            gitlinks(out),
            ["lib/a", "lib/with space"].map(PathBuf::from)
        );
    }

    /// The DELETE CHECK's git half on a real checkout: edits and untracked
    /// files are counted, commits are measured against the base, and a
    /// detached HEAD's commits are the ones nothing else holds.
    #[tokio::test]
    async fn delete_check_git_reads_changes_commits_and_detached_heads() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("repo");
        std::fs::create_dir(&repo).unwrap();
        init_repo(&repo).await;
        let wt = add_worktree(&repo, "feature", None).await.unwrap();

        assert!(changed_paths(&wt).await.unwrap().is_empty());
        let base = comparison_base(&repo, None).await;
        assert_eq!(
            base.as_deref(),
            Some("main"),
            "no origin: the root's branch"
        );
        assert_eq!(unlanded_commits(&wt, "main", "HEAD").await.unwrap(), 0);

        std::fs::write(wt.join("new.txt"), "x").unwrap();
        assert_eq!(
            changed_paths(&wt).await.unwrap(),
            [ChangedPath {
                path: PathBuf::from("new.txt"),
                untracked: true,
            }]
        );
        git(&wt, &["add", "."]).await.unwrap();
        git(&wt, &["commit", "-m", "work"]).await.unwrap();
        assert_eq!(unlanded_commits(&wt, "main", "HEAD").await.unwrap(), 1);
        assert!(!head_detached(&wt).await);
        assert_eq!(
            unreferenced_commits(&wt).await.unwrap(),
            0,
            "the branch holds it"
        );

        git(&wt, &["checkout", "--detach"]).await.unwrap();
        git(&wt, &["commit", "--allow-empty", "-m", "loose"])
            .await
            .unwrap();
        assert!(head_detached(&wt).await);
        assert_eq!(unreferenced_commits(&wt).await.unwrap(), 1);

        assert_eq!(
            comparison_base(&repo, Some("feature")).await.as_deref(),
            Some("feature"),
            "a configured base the repo has wins"
        );
        assert_eq!(
            comparison_base(&repo, Some("nope")).await.as_deref(),
            Some("main")
        );
    }

    /// Commit `file` with `body` in `dir`.
    async fn commit_file(dir: &Path, file: &str, body: &str) {
        std::fs::write(dir.join(file), body).unwrap();
        git(dir, &["add", "--", file]).await.unwrap();
        git(dir, &["commit", "-q", "-m", file]).await.unwrap();
    }

    /// A repo on `main` and a worktree on `feat` holding two commits of
    /// its own, for the landing tests to land one way or another.
    async fn repo_with_feature() -> (tempfile::TempDir, PathBuf, PathBuf) {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("repo");
        std::fs::create_dir(&repo).unwrap();
        init_repo(&repo).await;
        let wt = add_worktree(&repo, "feat", None).await.unwrap();
        commit_file(&wt, "a.txt", "a").await;
        commit_file(&wt, "b.txt", "b").await;
        (tmp, repo, wt)
    }

    /// A branch merged into its base, by a merge commit, has landed.
    #[tokio::test]
    async fn a_branch_landed_by_merge_has_nothing_unlanded() {
        let (_tmp, repo, wt) = repo_with_feature().await;
        assert_eq!(unlanded_commits(&wt, "main", "HEAD").await.unwrap(), 2);
        commit_file(&repo, "other.txt", "o").await;
        git(&repo, &["merge", "--no-ff", "-q", "-m", "merge", "feat"])
            .await
            .unwrap();
        assert_eq!(unlanded_commits(&wt, "main", "HEAD").await.unwrap(), 0);
        assert_eq!(unlanded_commits(&repo, "main", "feat").await.unwrap(), 0);
    }

    /// A squash merge leaves `main..feat` at two commits forever; by
    /// content nothing in them is missing.
    #[tokio::test]
    async fn a_branch_landed_by_squash_has_nothing_unlanded() {
        let (_tmp, repo, wt) = repo_with_feature().await;
        commit_file(&repo, "other.txt", "o").await;
        git(&repo, &["merge", "--squash", "-q", "feat"])
            .await
            .unwrap();
        git(&repo, &["commit", "-q", "-m", "squashed"])
            .await
            .unwrap();
        let ancestry = git(&wt, &["rev-list", "--count", "main..HEAD"])
            .await
            .unwrap();
        assert_eq!(ancestry.trim(), "2", "ancestry still counts both");
        assert_eq!(unlanded_commits(&wt, "main", "HEAD").await.unwrap(), 0);

        // One more commit on the branch is unlanded again — the whole
        // branch is, since the squash test is all or nothing.
        commit_file(&wt, "c.txt", "c").await;
        assert!(unlanded_commits(&wt, "main", "HEAD").await.unwrap() > 0);
    }

    /// Commits cherry-picked onto the base (a rebase-and-merge, a merge
    /// queue) are on it by patch, not by ancestry.
    #[tokio::test]
    async fn a_branch_landed_by_cherry_pick_has_nothing_unlanded() {
        let (_tmp, repo, wt) = repo_with_feature().await;
        commit_file(&repo, "other.txt", "o").await;
        git(&repo, &["cherry-pick", "main..feat"]).await.unwrap();
        assert_eq!(unlanded_commits(&wt, "main", "HEAD").await.unwrap(), 0);
    }

    /// One commit the base has no patch for is counted, alone.
    #[tokio::test]
    async fn one_unlanded_commit_is_counted() {
        let (_tmp, repo, wt) = repo_with_feature().await;
        git(&repo, &["cherry-pick", "feat~1"]).await.unwrap();
        assert_eq!(unlanded_commits(&wt, "main", "HEAD").await.unwrap(), 1);
    }

    /// The branch a worktree delete may take never includes the base,
    /// the default branch or the root's own.
    #[tokio::test]
    async fn base_and_root_branches_are_protected() {
        let (_tmp, repo, _wt) = repo_with_feature().await;
        assert!(protected_branch(&repo, "main", Some("main")).await);
        assert!(protected_branch(&repo, "main", Some("origin/main")).await);
        assert!(
            protected_branch(&repo, "main", Some("develop")).await,
            "root's"
        );
        assert!(!protected_branch(&repo, "feat", Some("main")).await);
        assert!(
            protected_branch(&repo, "main", None).await,
            "no base: still the root's"
        );
        assert!(!protected_branch(&repo, "feat", None).await);
    }

    /// `delete_branch_at` deletes a branch only at the tip it was given:
    /// one a commit has moved since is kept.
    #[tokio::test]
    async fn delete_branch_at_compares_the_tip_before_it_deletes() {
        let (_tmp, repo, wt) = repo_with_feature().await;
        let judged = branch_tip(&wt).await.expect("on feat");
        assert_eq!(judged.name, "feat");
        git(&repo, &["config", "branch.feat.remote", "origin"])
            .await
            .unwrap();
        commit_file(&wt, "late.txt", "late").await;
        remove_worktree(&repo, &wt, true).await.unwrap();
        let moved = delete_branch_at(&repo, &judged).await;
        assert!(moved.is_err(), "a commit since the check keeps it");
        assert!(local_branch(&repo, "feat").await);

        let now = nebula_core::BranchTip {
            name: "feat".into(),
            oid: git(&repo, &["rev-parse", "feat"])
                .await
                .unwrap()
                .trim()
                .into(),
        };
        delete_branch_at(&repo, &now).await.unwrap();
        assert!(!local_branch(&repo, "feat").await);
        assert_eq!(config_get(&repo, "branch.feat.remote").await, None);
    }

    /// A submodule deinitialised in the worktree leaves its store in the
    /// worktree's git dir, which the delete takes, commits and all; the
    /// main repository's own stores, which it leaves, never count.
    #[tokio::test]
    async fn a_deinitialised_submodules_store_still_counts() {
        let tmp = tempfile::tempdir().unwrap();
        let allow = ["-c", "protocol.file.allow=always"];
        let upstream = tmp.path().join("lib");
        let repo = tmp.path().join("repo");
        for dir in [&upstream, &repo] {
            std::fs::create_dir(dir).unwrap();
            init_repo(dir).await;
        }
        let url = upstream.to_string_lossy().into_owned();
        let mut args = allow.to_vec();
        args.extend(["submodule", "add", "-q", url.as_str(), "lib"]);
        git(&repo, &args).await.unwrap();
        git(&repo, &["commit", "-q", "-m", "add lib"])
            .await
            .unwrap();
        // A commit only the main repository's store has: never this
        // worktree's to lose.
        let root_sub = repo.join("lib");
        git(&root_sub, &["config", "user.email", "t@t"])
            .await
            .unwrap();
        git(&root_sub, &["config", "user.name", "t"]).await.unwrap();
        commit_file(&root_sub, "root.txt", "r").await;

        let wt = add_worktree(&repo, "feat", None).await.unwrap();
        let mut args = allow.to_vec();
        args.extend(["submodule", "update", "--init", "-q"]);
        git(&wt, &args).await.unwrap();
        assert_eq!(submodule_unpushed(&wt, &repo).await.unwrap(), 0);
        let sub = wt.join("lib");
        git(&sub, &["config", "user.email", "t@t"]).await.unwrap();
        git(&sub, &["config", "user.name", "t"]).await.unwrap();
        commit_file(&sub, "z.txt", "z").await;
        git(&wt, &["submodule", "deinit", "-q", "-f", "lib"])
            .await
            .unwrap();
        assert!(!sub.join(".git").exists(), "deinitialised");
        assert_eq!(submodule_unpushed(&wt, &repo).await.unwrap(), 1);
    }

    #[test]
    fn a_module_name_drops_the_stores_own_modules_levels() {
        assert_eq!(module_name(Path::new("lib")), "lib");
        assert_eq!(module_name(Path::new("lib/modules/inner")), "lib/inner");
    }

    /// A store mid-clone — HEAD naming a branch with no commit yet, the
    /// state `git submodule update` leaves for a moment — holds nothing to
    /// lose: 0, not an error.
    #[tokio::test]
    async fn a_store_still_being_cloned_counts_nothing() {
        let (_tmp, repo, wt) = repo_with_feature().await;
        let own = git(&wt, &["rev-parse", "--absolute-git-dir"])
            .await
            .unwrap();
        let store = Path::new(own.trim()).join("modules").join("cloning");
        git(&wt, &["init", "-q", "--bare", &store.to_string_lossy()])
            .await
            .unwrap();
        assert_eq!(submodule_unpushed(&wt, &repo).await.unwrap(), 0);
    }

    /// A store git cannot read names its submodule in the error, for the
    /// band to say which.
    #[tokio::test]
    async fn an_unreadable_store_names_its_submodule() {
        let (_tmp, repo, wt) = repo_with_feature().await;
        let own = git(&wt, &["rev-parse", "--absolute-git-dir"])
            .await
            .unwrap();
        let broken = Path::new(own.trim()).join("modules").join("half");
        std::fs::create_dir_all(broken.join("objects")).unwrap();
        std::fs::write(broken.join("HEAD"), "not a ref\n").unwrap();
        let err = submodule_unpushed(&wt, &repo).await.unwrap_err();
        let failed = err
            .downcast_ref::<SubmoduleError>()
            .unwrap_or_else(|| panic!("{err:#}"));
        assert_eq!(failed.path, "half");
    }

    /// Only a directory with `HEAD` and `objects/` is a store, and a
    /// symbolic link under `modules/` is never followed.
    #[test]
    fn collect_stores_takes_real_stores_and_follows_no_link() {
        let tmp = tempfile::tempdir().unwrap();
        let modules = tmp.path().join("modules");
        let store = modules.join("lib");
        std::fs::create_dir_all(store.join("objects")).unwrap();
        std::fs::write(store.join("HEAD"), "ref: refs/heads/main\n").unwrap();
        let headless = modules.join("stray");
        std::fs::create_dir_all(&headless).unwrap();
        std::fs::write(headless.join("HEAD"), "x").unwrap();
        std::os::unix::fs::symlink(&modules, modules.join("loop")).unwrap();
        let mut found = Vec::new();
        collect_stores(&modules, &mut found).unwrap();
        assert_eq!(found, [std::fs::canonicalize(&store).unwrap()]);
        let mut none = Vec::new();
        collect_stores(&tmp.path().join("absent"), &mut none).unwrap();
        assert!(none.is_empty());
    }

    /// A nested submodule's commits count like a top-level one's.
    #[tokio::test]
    async fn nested_submodule_commits_on_no_remote_are_counted() {
        let tmp = tempfile::tempdir().unwrap();
        let allow = ["-c", "protocol.file.allow=always"];
        let make = |name: &str| tmp.path().join(name);
        for name in ["inner", "lib", "repo"] {
            std::fs::create_dir(make(name)).unwrap();
            init_repo(&make(name)).await;
        }
        let url = |name: &str| make(name).to_string_lossy().into_owned();
        let inner = url("inner");
        let mut args = allow.to_vec();
        args.extend(["submodule", "add", "-q", inner.as_str(), "inner"]);
        git(&make("lib"), &args).await.unwrap();
        git(&make("lib"), &["commit", "-q", "-m", "add inner"])
            .await
            .unwrap();
        let lib = url("lib");
        let mut args = allow.to_vec();
        args.extend(["submodule", "add", "-q", lib.as_str(), "lib"]);
        git(&make("repo"), &args).await.unwrap();
        git(&make("repo"), &["commit", "-q", "-m", "add lib"])
            .await
            .unwrap();
        let repo = make("repo");
        let wt = add_worktree(&repo, "feat", None).await.unwrap();
        let mut args = allow.to_vec();
        args.extend(["submodule", "update", "--init", "--recursive", "-q"]);
        git(&wt, &args).await.unwrap();
        assert_eq!(submodule_unpushed(&wt, &repo).await.unwrap(), 0);

        let nested = wt.join("lib").join("inner");
        git(&nested, &["config", "user.email", "t@t"])
            .await
            .unwrap();
        git(&nested, &["config", "user.name", "t"]).await.unwrap();
        commit_file(&nested, "n.txt", "n").await;
        assert_eq!(submodule_unpushed(&wt, &repo).await.unwrap(), 1);
    }

    /// A symbolic-ref branch is deleted itself, never what it points at.
    #[tokio::test]
    async fn delete_branch_at_never_follows_a_symbolic_ref() {
        let (_tmp, repo, _wt) = repo_with_feature().await;
        git(
            &repo,
            &["symbolic-ref", "refs/heads/alias", "refs/heads/main"],
        )
        .await
        .unwrap();
        let main = git(&repo, &["rev-parse", "main"]).await.unwrap();
        let alias = nebula_core::BranchTip {
            name: "alias".into(),
            oid: main.trim().into(),
        };
        delete_branch_at(&repo, &alias).await.unwrap();
        assert!(local_branch(&repo, "main").await, "main survives");
        assert_eq!(
            git(&repo, &["rev-parse", "main"]).await.unwrap(),
            main,
            "untouched"
        );
        assert!(git(&repo, &["symbolic-ref", "-q", "refs/heads/alias"])
            .await
            .is_err());
    }

    /// A submodule commit made in a linked worktree lives only in that
    /// worktree's submodule store until it is pushed; one the root's
    /// submodule has, or a remote holds, is safe.
    #[tokio::test]
    async fn submodule_commits_on_no_remote_are_counted() {
        let tmp = tempfile::tempdir().unwrap();
        let upstream = tmp.path().join("lib");
        std::fs::create_dir(&upstream).unwrap();
        init_repo(&upstream).await;
        let repo = tmp.path().join("repo");
        std::fs::create_dir(&repo).unwrap();
        init_repo(&repo).await;
        let url = upstream.to_string_lossy().into_owned();
        git(
            &repo,
            &[
                "-c",
                "protocol.file.allow=always",
                "submodule",
                "add",
                "-q",
                &url,
                "lib",
            ],
        )
        .await
        .unwrap();
        git(&repo, &["commit", "-q", "-m", "add lib"])
            .await
            .unwrap();
        assert_eq!(submodule_unpushed(&repo, &repo).await.unwrap(), 0);

        let wt = add_worktree(&repo, "feat", None).await.unwrap();
        assert_eq!(
            submodule_unpushed(&wt, &repo).await.unwrap(),
            0,
            "not checked out in the worktree: nothing in it"
        );
        git(
            &wt,
            &[
                "-c",
                "protocol.file.allow=always",
                "submodule",
                "update",
                "--init",
                "-q",
            ],
        )
        .await
        .unwrap();
        assert_eq!(submodule_unpushed(&wt, &repo).await.unwrap(), 0);
        let sub = wt.join("lib");
        git(&sub, &["config", "user.email", "t@t"]).await.unwrap();
        git(&sub, &["config", "user.name", "t"]).await.unwrap();
        commit_file(&sub, "x.txt", "x").await;
        commit_file(&sub, "y.txt", "y").await;
        git(&wt, &["commit", "-q", "-am", "bump lib"])
            .await
            .unwrap();
        assert!(changed_paths(&wt).await.unwrap().is_empty());
        assert_eq!(submodule_unpushed(&wt, &repo).await.unwrap(), 2);

        // Fetched into the root's copy but on none of its refs, they are
        // dangling there — gone at its next gc — so still lost.
        let wt_sub = sub.to_string_lossy().into_owned();
        git(&repo.join("lib"), &["fetch", "-q", &wt_sub, "HEAD"])
            .await
            .unwrap();
        assert_eq!(submodule_unpushed(&wt, &repo).await.unwrap(), 2);
        // A branch there keeps them: they survive the delete.
        git(&repo.join("lib"), &["branch", "kept", "FETCH_HEAD"])
            .await
            .unwrap();
        assert_eq!(submodule_unpushed(&wt, &repo).await.unwrap(), 0);

        // A commit on a local branch of the worktree's submodule, HEAD
        // then detached back to the pin: on no remote, in no other store.
        let pin = git(&sub, &["rev-parse", "HEAD"]).await.unwrap();
        git(&sub, &["switch", "-q", "-c", "side"]).await.unwrap();
        commit_file(&sub, "z.txt", "z").await;
        git(&sub, &["checkout", "-q", "--detach", pin.trim()])
            .await
            .unwrap();
        assert!(changed_paths(&wt).await.unwrap().is_empty());
        assert_eq!(submodule_unpushed(&wt, &repo).await.unwrap(), 1);

        // A stash there is lost with the store too.
        git(&sub, &["branch", "-q", "-D", "side"]).await.unwrap();
        assert_eq!(submodule_unpushed(&wt, &repo).await.unwrap(), 0);
        std::fs::write(sub.join("x.txt"), "edited").unwrap();
        git(&sub, &["stash", "-q"]).await.unwrap();
        assert!(submodule_unpushed(&wt, &repo).await.unwrap() > 0);
    }
}
