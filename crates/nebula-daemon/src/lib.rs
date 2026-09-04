pub mod config;
pub mod git;
pub mod hooks;
pub mod lifecycle;
pub mod metrics;
pub mod pty;
pub mod registry;
pub mod server;
pub mod session_title;
pub mod sibling;
pub mod status;
pub mod store;

use anyhow::{bail, Context, Result};
use nebula_core::{env, paths};

/// Floor on any env-tunable loop period: the overrides exist to make tests
/// fast, and a zero or near-zero tick would just spin the daemon.
const MIN_TICK_MS: u64 = 50;

/// A loop period from an env override's value: unset or unparseable falls
/// back to `default_ms`, and nothing goes below [`MIN_TICK_MS`].
fn env_period_ms(value: Option<&str>, default_ms: u64) -> u64 {
    value
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(default_ms)
        .max(MIN_TICK_MS)
}

/// The ticker for a background loop whose period the env var `var` may
/// override (see [`env_period_ms`]).
fn env_interval(var: &str, default_ms: u64) -> tokio::time::Interval {
    let period = env_period_ms(std::env::var(var).ok().as_deref(), default_ms);
    tokio::time::interval(std::time::Duration::from_millis(period))
}

/// Entry point for the daemon process (already detached by the launcher,
/// or running with --foreground).
pub fn run_daemon() -> Result<()> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()?;
    runtime.block_on(serve())
}

async fn serve() -> Result<()> {
    let Some(_lock) = lifecycle::PidfileLock::try_acquire()? else {
        bail!("another nebula daemon is already running");
    };
    // Record which build this daemon runs so installers can tell an
    // up-to-date daemon from a stale one (`nebula _stale-daemon-note`).
    lifecycle::write_buildstamp();

    let sock = paths::socket_path();
    lifecycle::unlink_stale_socket(&sock);
    let listener = tokio::net::UnixListener::bind(&sock)
        .with_context(|| format!("bind {}", sock.display()))?;
    tracing::info!(pid = std::process::id(), socket = %sock.display(), "nebula daemon listening");

    let store = std::sync::Arc::new(store::Store::open(&paths::db_path())?);
    // Agents persisted as live had their PTYs die with the previous daemon.
    match store.sweep_disconnected() {
        Ok(swept) if !swept.is_empty() => {
            tracing::info!(
                count = swept.len(),
                "boot sweep: marked orphaned agents disconnected"
            )
        }
        Ok(_) => {}
        Err(e) => tracing::warn!(error = %e, "boot sweep failed"),
    }

    // Hook receiver: loopback HTTP endpoint the claude hook one-liners hit.
    // It shares the store to answer UserPromptSubmit hooks with the
    // auto-title instruction while a session is still untitled.
    let (hook_env, mut hook_rx) = hooks::start_hook_server(store.clone()).await?;
    tracing::info!(port = hook_env.port, "hook receiver listening");

    let daemon = registry::Daemon::new(store, hook_env);

    // Drain hook events into the status machines; a payload that reports a
    // cwd inside another worktree of the same project re-homes the agent row.
    {
        let daemon = daemon.clone();
        tokio::spawn(async move {
            while let Some(hooks::HookDelivery {
                agent_id,
                event,
                session_id,
                cwd,
                transcript,
            }) = hook_rx.recv().await
            {
                let captures_session = event.captures_session();
                daemon.apply_hook_event(&agent_id, event.clone(), session_id.clone());
                // Claude's own session name (`/rename`) rides no hook, but
                // every payload says where it is persisted: remember that
                // for the window-title path and read it now.
                if let Some(transcript) = transcript {
                    daemon.note_transcript(&agent_id, transcript);
                    daemon.sync_claude_title(&agent_id);
                }
                if let Some(cwd) = &cwd {
                    daemon.reparent_agent_by_cwd(
                        &agent_id,
                        cwd,
                        session_id.as_deref(),
                        captures_session,
                    );
                }
                // A `nebula worktree` relocation waits for the turn to end.
                // After the reparent on purpose: this turn's final payload
                // still carries the old checkout's cwd, which must be seen
                // (and ignored) while the relocation is still pending.
                daemon.complete_pending_move(&agent_id, &event);
            }
        });
    }

    // Learn which agent CLIs are installed before anyone asks, so a create
    // that has to refuse ("codex was not found on your PATH") answers at once
    // instead of stalling on a login-shell probe.
    {
        let daemon = daemon.clone();
        tokio::spawn(async move { daemon.warm_cli_probes().await });
    }

    // Deferred-finish recheck (held Stops drain to finished after grace).
    {
        let daemon = daemon.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(30));
            loop {
                tokio::select! {
                    _ = daemon.shutdown.cancelled() => break,
                    _ = interval.tick() => {
                        daemon.tick_status_machines();
                        daemon.reap_prewarmed();
                    }
                }
            }
        });
    }

    // Idle-session reaper: kill PTYs in worktrees no client is looking at
    // once they age past `session_idle_timeout` (bounds what prewarmed and
    // walked-away-from sessions cost). Its own loop so tests can speed the
    // sweep up without touching the status-machine cadence.
    {
        let daemon = daemon.clone();
        tokio::spawn(async move {
            let mut interval = env_interval(env::IDLE_REAP_MS, 15_000);
            loop {
                tokio::select! {
                    _ = daemon.shutdown.cancelled() => break,
                    _ = interval.tick() => daemon.reap_idle_sessions(),
                }
            }
        });
    }

    // Worktrees created, removed, or re-checked-out outside nebula (an agent
    // running `git worktree add`, a manual `git checkout` on the root) should
    // show up without a restart. A cheap mtime probe over the git files those
    // operations touch gates the full `git worktree list` reconcile; the
    // first tick also runs one boot-time sync per project.
    {
        let daemon = daemon.clone();
        tokio::spawn(async move {
            let mut interval = env_interval(env::WORKTREE_SYNC_MS, 2_000);
            let mut seen: std::collections::HashMap<nebula_core::ProjectId, std::time::SystemTime> =
                std::collections::HashMap::new();
            loop {
                tokio::select! {
                    _ = daemon.shutdown.cancelled() => break,
                    _ = interval.tick() => {}
                }
                let Ok((projects, _, _, _)) = daemon.store.load_tree() else {
                    continue;
                };
                seen.retain(|id, _| projects.iter().any(|p| &p.id == id));
                for project in projects {
                    // An unreadable probe is not a fingerprint: caching it
                    // would compare equal on every later tick and retire the
                    // project from syncing for the life of the daemon. Only a
                    // stamp we actually read can gate the skip.
                    let stamp = worktree_probe_stamp(&project.repo_path);
                    if stamp.is_some() && seen.get(&project.id) == stamp.as_ref() {
                        continue;
                    }
                    // The stamp is only recorded on success, so a failed
                    // sync (repo briefly locked, git missing) retries.
                    match daemon.sync_project_worktrees(&project).await {
                        Ok(()) => match stamp {
                            Some(stamp) => {
                                seen.insert(project.id.clone(), stamp);
                            }
                            None => {
                                seen.remove(&project.id);
                            }
                        },
                        Err(e) => tracing::warn!(
                            project = %project.name, error = %e, "worktree sync failed"
                        ),
                    }
                }
            }
        });
    }

    // SIGTERM/SIGINT → clean shutdown.
    {
        let daemon = daemon.clone();
        tokio::spawn(async move {
            let mut term =
                tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                    .expect("install SIGTERM handler");
            let mut int = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt())
                .expect("install SIGINT handler");
            tokio::select! {
                _ = term.recv() => {}
                _ = int.recv() => {}
            }
            tracing::info!("signal received; shutting down");
            daemon.shutdown.cancel();
        });
    }

    server::accept_loop(daemon.clone(), listener).await;

    // Cleanup: kill PTYs, remove the socket. (Status persistence joins in
    // phase 4/5 when the store exists.)
    daemon.kill_all();
    let _ = std::fs::remove_file(&sock);
    tracing::info!("daemon exited cleanly");
    Ok(())
}

/// Latest mtime across the git files a worktree change touches: the
/// `.git/worktrees` registry (add/remove/prune), each linked checkout's
/// `HEAD` (branch switch inside it), and the root `.git/HEAD` (branch
/// switch on the main checkout). Any of these moving forward means the
/// stored rows may be stale.
fn worktree_probe_stamp(repo_path: &std::path::Path) -> Option<std::time::SystemTime> {
    let git_dir = git_common_dir(repo_path)?;
    let mtime = |p: std::path::PathBuf| std::fs::metadata(p).and_then(|m| m.modified()).ok();
    let mut stamps: Vec<std::time::SystemTime> = Vec::new();
    stamps.extend(mtime(git_dir.join("HEAD")));
    stamps.extend(mtime(git_dir.join("worktrees")));
    if let Ok(dir) = std::fs::read_dir(git_dir.join("worktrees")) {
        for entry in dir.flatten() {
            stamps.extend(mtime(entry.path().join("HEAD")));
        }
    }
    stamps.into_iter().max()
}

/// The `.git` holding the repo's shared HEAD and per-worktree HEADs — the
/// files the probe above watches.
///
/// `<checkout>/.git` is that directory in a normal checkout, but in a linked
/// worktree it is a `gitdir:` file pointing at `<repo>/.git/worktrees/<name>`,
/// whose own `commondir` points back at the shared `.git`. Following both is
/// what lets a project rooted at a worktree (one added with `nebula add` from
/// inside one) probe anything at all: joining `HEAD` onto a *file* reads
/// nothing, and a stamp that is always `None` compares equal to itself on
/// every tick, so the sync would never run again after the first.
fn git_common_dir(repo_path: &std::path::Path) -> Option<std::path::PathBuf> {
    let dot_git = repo_path.join(".git");
    let git_dir = if std::fs::metadata(&dot_git).ok()?.is_dir() {
        dot_git
    } else {
        let text = std::fs::read_to_string(&dot_git).ok()?;
        rebase_on(repo_path, text.trim().strip_prefix("gitdir:")?.trim())
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

/// Git writes these pointers as either an absolute path or one relative to the
/// file that carries them.
fn rebase_on(base: &std::path::Path, target: &str) -> std::path::PathBuf {
    let target = std::path::PathBuf::from(target);
    if target.is_absolute() {
        target
    } else {
        base.join(target)
    }
}

#[cfg(test)]
mod period_tests {
    use super::*;

    #[test]
    fn env_period_ms_defaults_parses_and_floors() {
        assert_eq!(env_period_ms(None, 15_000), 15_000, "unset → default");
        assert_eq!(
            env_period_ms(Some("soon"), 15_000),
            15_000,
            "garbage → default"
        );
        assert_eq!(
            env_period_ms(Some("-5"), 2_000),
            2_000,
            "negative → default"
        );
        assert_eq!(
            env_period_ms(Some("10"), 15_000),
            MIN_TICK_MS,
            "too fast → floor"
        );
        assert_eq!(env_period_ms(Some("200"), 15_000), 200);
        // The floor applies to the default too — a caller can't dodge it.
        assert_eq!(env_period_ms(None, 1), MIN_TICK_MS);
    }
}

#[cfg(test)]
mod probe_tests {
    use super::*;

    fn git_in(repo: &std::path::Path, args: &[&str]) {
        let out = std::process::Command::new("git")
            .arg("-C")
            .arg(repo)
            .args(args)
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@example.com")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@example.com")
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    /// The sync only runs when this probe reports a change. A linked worktree
    /// keeps a `gitdir:` *file* where a checkout keeps a directory, so the
    /// probe used to read nothing there and answer `None` on every tick —
    /// equal to itself, so a project rooted at a worktree synced once at boot
    /// and never again. Both shapes must land on the same shared `.git`.
    #[test]
    fn probe_follows_a_linked_worktrees_gitdir_file() {
        let tmp = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(tmp.path()).unwrap();
        let repo = root.join("repo");
        std::fs::create_dir(&repo).unwrap();
        git_in(&repo, &["init", "-b", "main"]);
        git_in(&repo, &["commit", "--allow-empty", "-m", "init"]);
        let feat = root.join("repo-worktrees").join("feat");
        git_in(
            &repo,
            &["worktree", "add", &feat.to_string_lossy(), "-b", "feat"],
        );

        assert_eq!(
            git_common_dir(&feat),
            git_common_dir(&repo),
            "both checkouts probe the repo's shared .git"
        );
        let stamp = worktree_probe_stamp(&feat);
        assert!(stamp.is_some(), "a worktree-rooted project has a stamp");
        assert_eq!(
            stamp,
            worktree_probe_stamp(&repo),
            "and it is the same fingerprint the repo's own checkout reports"
        );
    }

    /// A directory that is not a checkout at all has no fingerprint. The sync
    /// loop must never cache that: `None` compares equal to itself forever.
    #[test]
    fn probe_has_no_stamp_outside_a_repo() {
        let tmp = tempfile::tempdir().unwrap();
        assert_eq!(worktree_probe_stamp(tmp.path()), None);
    }
}
