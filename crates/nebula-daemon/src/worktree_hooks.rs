//! WORKTREE HOOKS: a user-provided executable the DAEMON runs after it
//! creates or deletes a worktree, so a project can provision and release
//! what a checkout owns outside its own directory — a dev-server port, a
//! Caddy route, a docker compose project, a database. Configured per
//! repository in git config, read fresh at each use:
//!
//! ```sh
//! git config nebula.worktreeCreateHook /absolute/path/to/script
//! git config nebula.worktreeDeleteHook /absolute/path/to/script
//! ```
//!
//! Never a file inside the checkout — a committed hook would run whatever
//! a clone brought with it, which is why git itself refuses hooks from the
//! working tree. Git resolves the key the usual way, so a `--global` value
//! serves every project and a repo's own `.git/config` overrides it.
//!
//! The value is an executable path, spawned directly with no shell so
//! spaces in either path survive, from the main checkout (the deleted
//! directory is gone), with the main repository path and the worktree
//! path as its two arguments and `NEBULA_HOOK`, `NEBULA_WORKTREE_BRANCH`
//! and `NEBULA_WORKTREE_ID` in its environment. A hook only reports: it
//! runs after the git operation and the row change have gone through,
//! and a failure, a timeout, or a program that will not start becomes a
//! warning in every client — never a rolled-back create or delete. The
//! DAEMON's environment is not a login shell (launchd's PATH is thin), so
//! a script sets its own PATH.

use anyhow::{anyhow, bail, Context, Result};
use nebula_core::{env, WorktreeId};
use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

/// Which lifecycle moment a hook answers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorktreeHook {
    /// After nebula created and registered a checkout (`n`, `nebula
    /// worktree`, the QUICK PROMPT's fresh worktree, a PR SESSION's).
    Create,
    /// After nebula removed a checkout and dropped its row.
    Delete,
}

impl WorktreeHook {
    /// The git config key naming the executable.
    pub fn config_key(self) -> &'static str {
        match self {
            Self::Create => "nebula.worktreeCreateHook",
            Self::Delete => "nebula.worktreeDeleteHook",
        }
    }

    /// The `NEBULA_HOOK` value the script sees, so one script can serve
    /// both keys and branch on it.
    pub fn name(self) -> &'static str {
        match self {
            Self::Create => "worktree-create",
            Self::Delete => "worktree-delete",
        }
    }
}

/// What a hook is told about the worktree it runs for.
pub struct HookContext<'a> {
    /// The main checkout: the hook's cwd and its first argument.
    pub repo: &'a Path,
    /// The created or deleted checkout: the second argument.
    pub worktree: &'a Path,
    pub branch: &'a str,
    pub id: &'a WorktreeId,
}

/// How long a hook may run before it is killed, unless
/// `NEBULA_HOOK_TIMEOUT_MS` says otherwise.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

/// The hook timeout from the env override: a positive number of ms, else
/// the default.
fn timeout() -> Duration {
    parse_timeout_ms(env::non_empty(env::HOOK_TIMEOUT_MS).as_deref())
}

fn parse_timeout_ms(raw: Option<&str>) -> Duration {
    raw.and_then(|v| v.trim().parse::<u64>().ok())
        .filter(|ms| *ms > 0)
        .map(Duration::from_millis)
        .unwrap_or(DEFAULT_TIMEOUT)
}

/// Run `hook` when the repository configures one. `Ok(false)`: none is
/// configured, nothing happened. `Ok(true)`: it ran and exited 0. `Err`:
/// a condition the caller should show the user — the hook failed, timed
/// out, could not start, or (delete only) was skipped because the
/// directory is still on disk, so the resources tied to that path are
/// not nebula's to release.
pub async fn run(hook: WorktreeHook, ctx: HookContext<'_>) -> Result<bool> {
    let Some(program) = crate::git::config_get(ctx.repo, hook.config_key()).await else {
        return Ok(false);
    };
    if hook == WorktreeHook::Delete && ctx.worktree.exists() {
        bail!(
            "{} hook skipped: {} is still on disk",
            hook.name(),
            ctx.worktree.display()
        );
    }
    run_program(&program, hook, &ctx, timeout())
        .await
        .map(|()| true)
}

async fn run_program(
    program: &str,
    hook: WorktreeHook,
    ctx: &HookContext<'_>,
    timeout: Duration,
) -> Result<()> {
    let label = hook.name();
    tracing::info!(
        hook = label,
        program,
        worktree = %ctx.worktree.display(),
        "running worktree hook"
    );
    // Output lands in unlinked temp files, not pipes. A hook that starts a
    // dev server in the background and exits 0 leaves that server holding
    // its stdout; a pipe would keep the wait open until the timeout and
    // then report a success as "timed out". A file is inherited harmlessly
    // and the wait below is on the process alone.
    let stdout = tempfile::tempfile().context("hook output file")?;
    let stderr = tempfile::tempfile().context("hook output file")?;
    let mut child = tokio::process::Command::new(program)
        .arg(ctx.repo)
        .arg(ctx.worktree)
        .env("NEBULA_HOOK", label)
        .env("NEBULA_WORKTREE_BRANCH", ctx.branch)
        .env("NEBULA_WORKTREE_ID", ctx.id.as_str())
        .current_dir(ctx.repo)
        .stdin(Stdio::null())
        .stdout(Stdio::from(stdout.try_clone()?))
        .stderr(Stdio::from(stderr.try_clone()?))
        // Its own process group, so a timeout takes everything it started
        // down with it, not just the script.
        .process_group(0)
        .kill_on_drop(true)
        .spawn()
        .with_context(|| format!("{label} hook `{program}` could not start"))?;
    let status = match tokio::time::timeout(timeout, child.wait()).await {
        Ok(waited) => waited.with_context(|| format!("{label} hook `{program}`"))?,
        Err(_) => {
            kill_group(&child);
            let _ = child.wait().await;
            bail!(
                "{label} hook `{program}` timed out after {} and was killed with everything it started",
                describe(timeout)
            );
        }
    };
    let stdout = read_tail(stdout);
    let stderr = read_tail(stderr);
    if !stdout.trim().is_empty() {
        tracing::info!(hook = label, "hook stdout:\n{}", stdout.trim_end());
    }
    if !stderr.trim().is_empty() {
        tracing::info!(hook = label, "hook stderr:\n{}", stderr.trim_end());
    }
    if status.success() {
        return Ok(());
    }
    let status = match status.code() {
        Some(code) => format!("exited {code}"),
        None => "was killed by a signal".to_string(),
    };
    // The last line of stderr (or stdout) is the script's own summary,
    // and all a one-line warning has room for.
    let detail = stderr
        .trim()
        .lines()
        .last()
        .or_else(|| stdout.trim().lines().last())
        .unwrap_or("")
        .to_string();
    Err(anyhow!(
        "{label} hook `{program}` {status}{}",
        if detail.is_empty() {
            String::new()
        } else {
            format!(": {detail}")
        }
    ))
}

/// SIGKILL the hook's whole process group — the script and whatever it
/// spawned without `exec`. The group id is the hook's own pid
/// (`process_group(0)` above).
fn kill_group(child: &tokio::process::Child) {
    use nix::sys::signal::{killpg, Signal};
    use nix::unistd::Pid;
    if let Some(pid) = child.id() {
        let _ = killpg(Pid::from_raw(pid as i32), Signal::SIGKILL);
    }
}

/// How much of a hook's output is kept: the tail, since the last line is
/// what the warning quotes and a chatty script must not fill memory.
const OUTPUT_TAIL: u64 = 64 * 1024;

fn read_tail(mut file: std::fs::File) -> String {
    use std::io::{Read, Seek, SeekFrom};
    let len = file.metadata().map(|m| m.len()).unwrap_or(0);
    let mut buf = Vec::new();
    if file
        .seek(SeekFrom::Start(len.saturating_sub(OUTPUT_TAIL)))
        .is_ok()
    {
        let _ = file.by_ref().take(OUTPUT_TAIL).read_to_end(&mut buf);
    }
    String::from_utf8_lossy(&buf).into_owned()
}

fn describe(d: Duration) -> String {
    if d.as_millis().is_multiple_of(1000) {
        format!("{}s", d.as_secs())
    } else {
        format!("{}ms", d.as_millis())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use std::path::PathBuf;

    fn git(repo: &Path, args: &[&str]) {
        let out = std::process::Command::new("git")
            .arg("-C")
            .arg(repo)
            .args(args)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    /// A repo whose path has a space in it, so a hook that mangles its
    /// arguments shows up as the wrong path, not a passing test.
    fn repo_with_space(root: &Path) -> PathBuf {
        let repo = root.join("my repo");
        std::fs::create_dir_all(&repo).unwrap();
        git(&repo, &["init", "-b", "main"]);
        repo
    }

    fn script(dir: &Path, body: &str) -> PathBuf {
        let path = dir.join("hook.sh");
        std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    fn ctx<'a>(repo: &'a Path, worktree: &'a Path, id: &'a WorktreeId) -> HookContext<'a> {
        HookContext {
            repo,
            worktree,
            branch: "feat/x",
            id,
        }
    }

    #[tokio::test]
    async fn unset_hook_is_a_no_op() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = repo_with_space(tmp.path());
        let id = WorktreeId("w".into());
        let wt = tmp.path().join("gone");
        assert!(!run(WorktreeHook::Delete, ctx(&repo, &wt, &id))
            .await
            .unwrap());
        assert!(!run(WorktreeHook::Create, ctx(&repo, &wt, &id))
            .await
            .unwrap());
    }

    /// Both paths arrive verbatim as `$1`/`$2` (spaces and all), the
    /// branch, id and hook name ride the environment, and the script runs
    /// from the main checkout.
    #[tokio::test]
    async fn hook_gets_literal_paths_env_and_the_repo_as_cwd() {
        let tmp = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(tmp.path()).unwrap();
        let repo = repo_with_space(&root);
        let out = root.join("out.txt");
        let hook = script(
            &root,
            &format!(
                "printf '%s\\n%s\\n%s\\n%s\\n%s\\n%s\\n' \"$1\" \"$2\" \"$NEBULA_HOOK\" \
                 \"$NEBULA_WORKTREE_BRANCH\" \"$NEBULA_WORKTREE_ID\" \"$(pwd)\" > '{}'",
                out.display()
            ),
        );
        git(
            &repo,
            &[
                "config",
                "nebula.worktreeDeleteHook",
                &hook.to_string_lossy(),
            ],
        );
        let wt = root.join("my repo-worktrees").join("feat x");
        let id = WorktreeId("w1".into());

        assert!(run(WorktreeHook::Delete, ctx(&repo, &wt, &id))
            .await
            .unwrap());

        let got = std::fs::read_to_string(&out).unwrap();
        let lines: Vec<&str> = got.lines().collect();
        assert_eq!(
            lines,
            vec![
                repo.to_str().unwrap(),
                wt.to_str().unwrap(),
                "worktree-delete",
                "feat/x",
                "w1",
                repo.to_str().unwrap(),
            ]
        );
    }

    /// The create key is its own setting: a repo with only a delete hook
    /// runs nothing on create, and the create hook sees its own name.
    #[tokio::test]
    async fn create_and_delete_keys_are_independent() {
        let tmp = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(tmp.path()).unwrap();
        let repo = repo_with_space(&root);
        let out = root.join("out.txt");
        let hook = script(
            &root,
            &format!("echo \"$NEBULA_HOOK\" >> '{}'", out.display()),
        );
        git(
            &repo,
            &[
                "config",
                "nebula.worktreeCreateHook",
                &hook.to_string_lossy(),
            ],
        );
        let wt = root.join("wt");
        std::fs::create_dir(&wt).unwrap();
        let id = WorktreeId("w".into());

        assert!(run(WorktreeHook::Create, ctx(&repo, &wt, &id))
            .await
            .unwrap());
        assert!(!run(WorktreeHook::Delete, ctx(&repo, &wt, &id))
            .await
            .unwrap());
        assert_eq!(std::fs::read_to_string(&out).unwrap(), "worktree-create\n");
    }

    /// A delete hook is for a checkout that is gone; while the directory
    /// is still there (git stopped tracking it and nebula left it alone)
    /// the hook is skipped and the user told, not run against live files.
    #[tokio::test]
    async fn delete_hook_is_skipped_while_the_directory_exists() {
        let tmp = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(tmp.path()).unwrap();
        let repo = repo_with_space(&root);
        let out = root.join("out.txt");
        let hook = script(&root, &format!("touch '{}'", out.display()));
        git(
            &repo,
            &[
                "config",
                "nebula.worktreeDeleteHook",
                &hook.to_string_lossy(),
            ],
        );
        let wt = root.join("still-here");
        std::fs::create_dir(&wt).unwrap();
        let id = WorktreeId("w".into());

        let err = run(WorktreeHook::Delete, ctx(&repo, &wt, &id))
            .await
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("skipped") && err.contains("still on disk"),
            "{err}"
        );
        assert!(!out.exists(), "the hook did not run");
    }

    #[tokio::test]
    async fn failing_hook_reports_its_status_and_last_stderr_line() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = repo_with_space(tmp.path());
        let hook = script(
            tmp.path(),
            "echo 'first line' >&2\necho 'slot 3 not found' >&2\nexit 3",
        );
        let id = WorktreeId("w".into());
        let wt = tmp.path().join("gone");

        let err = run_program(
            &hook.to_string_lossy(),
            WorktreeHook::Delete,
            &ctx(&repo, &wt, &id),
            DEFAULT_TIMEOUT,
        )
        .await
        .unwrap_err()
        .to_string();
        assert!(err.starts_with("worktree-delete hook `"), "{err}");
        assert!(err.contains("exited 3: slot 3 not found"), "{err}");
    }

    #[tokio::test]
    async fn hook_that_cannot_start_is_a_clear_warning() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = repo_with_space(tmp.path());
        let id = WorktreeId("w".into());
        let wt = tmp.path().join("gone");
        let missing = tmp.path().join("no-such-hook");

        let err = run_program(
            &missing.to_string_lossy(),
            WorktreeHook::Create,
            &ctx(&repo, &wt, &id),
            DEFAULT_TIMEOUT,
        )
        .await
        .unwrap_err();
        assert!(
            format!("{err:#}").contains("worktree-create hook `")
                && format!("{err:#}").contains("could not start"),
            "{err:#}"
        );
    }

    /// The pid a script wrote to `file`, waiting for it to appear: under a
    /// loaded test runner a shell can take a while to start.
    async fn pid_in(file: &Path) -> i32 {
        for _ in 0..100 {
            if let Ok(s) = std::fs::read_to_string(file) {
                if let Ok(pid) = s.trim().parse() {
                    return pid;
                }
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        panic!("{} was never written", file.display());
    }

    fn alive(pid: i32) -> bool {
        nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid), None).is_ok()
    }

    async fn wait_dead(pid: i32) -> bool {
        for _ in 0..40 {
            if !alive(pid) {
                return true;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        false
    }

    /// A script that hangs on a child it did not `exec` — the shape of a
    /// real hook — is killed with that child: the group goes, not just the
    /// script, so "was killed" is the truth.
    #[tokio::test]
    async fn hook_past_the_timeout_is_killed_with_what_it_started() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = repo_with_space(tmp.path());
        let pidfile = tmp.path().join("sleeper.pid");
        let hook = script(
            tmp.path(),
            &format!("sleep 30 &\necho $! > '{}'\nwait", pidfile.display()),
        );
        let id = WorktreeId("w".into());
        let wt = tmp.path().join("gone");

        let started = std::time::Instant::now();
        let err = run_program(
            &hook.to_string_lossy(),
            WorktreeHook::Delete,
            &ctx(&repo, &wt, &id),
            Duration::from_secs(1),
        )
        .await
        .unwrap_err()
        .to_string();
        assert!(
            err.contains("timed out after 1s") && err.contains("everything it started"),
            "{err}"
        );
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "the wait ended with the timeout, not the sleep"
        );
        let sleeper = pid_in(&pidfile).await;
        assert!(
            wait_dead(sleeper).await,
            "the sleeper {sleeper} outlived the kill"
        );
    }

    /// A hook that starts something long-lived — a dev server — and exits
    /// 0 is a success the moment it exits, even though its child still
    /// holds the output it inherited; and that child is left running.
    #[tokio::test]
    async fn backgrounded_child_neither_holds_the_hook_open_nor_dies_with_it() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = repo_with_space(tmp.path());
        let pidfile = tmp.path().join("server.pid");
        // No output redirection on purpose: the child keeps stdout/stderr.
        let hook = script(
            tmp.path(),
            &format!(
                "sleep 30 &\necho $! > '{}'\necho 'server up'\nexit 0",
                pidfile.display()
            ),
        );
        let id = WorktreeId("w".into());
        let wt = tmp.path().join("gone");

        let started = std::time::Instant::now();
        run_program(
            &hook.to_string_lossy(),
            WorktreeHook::Create,
            &ctx(&repo, &wt, &id),
            Duration::from_secs(5),
        )
        .await
        .unwrap();
        assert!(
            started.elapsed() < Duration::from_secs(4),
            "the wait ended with the script's exit, not its child's"
        );
        let server = pid_in(&pidfile).await;
        assert!(alive(server), "a successful hook's child is left running");
        let _ = nix::sys::signal::kill(
            nix::unistd::Pid::from_raw(server),
            nix::sys::signal::Signal::SIGKILL,
        );
    }

    #[test]
    fn timeout_override_parses_positive_ms_and_falls_back() {
        assert_eq!(parse_timeout_ms(None), DEFAULT_TIMEOUT);
        assert_eq!(parse_timeout_ms(Some("250")), Duration::from_millis(250));
        assert_eq!(parse_timeout_ms(Some(" 1000 ")), Duration::from_secs(1));
        assert_eq!(parse_timeout_ms(Some("0")), DEFAULT_TIMEOUT);
        assert_eq!(parse_timeout_ms(Some("soon")), DEFAULT_TIMEOUT);
        assert_eq!(describe(Duration::from_secs(30)), "30s");
        assert_eq!(describe(Duration::from_millis(1500)), "1500ms");
    }
}
