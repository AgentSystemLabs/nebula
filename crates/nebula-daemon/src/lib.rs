pub mod config;
pub mod git;
pub mod hooks;
pub mod lifecycle;
pub mod metrics;
pub mod pty;
pub mod registry;
pub mod server;
pub mod status;
pub mod store;

use anyhow::{bail, Context, Result};
use nebula_core::paths;

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
            }) = hook_rx.recv().await
            {
                let captures_session = event.captures_session();
                daemon.apply_hook_event(&agent_id, event, session_id.clone());
                if let Some(cwd) = &cwd {
                    daemon.reparent_agent_by_cwd(
                        &agent_id,
                        cwd,
                        session_id.as_deref(),
                        captures_session,
                    );
                }
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
            let period = std::env::var("NEBULA_IDLE_REAP_MS")
                .ok()
                .and_then(|v| v.parse::<u64>().ok())
                .unwrap_or(15_000)
                .max(50);
            let mut interval = tokio::time::interval(std::time::Duration::from_millis(period));
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
            let period = std::env::var("NEBULA_WORKTREE_SYNC_MS")
                .ok()
                .and_then(|v| v.parse::<u64>().ok())
                .unwrap_or(2_000)
                .max(50);
            let mut interval = tokio::time::interval(std::time::Duration::from_millis(period));
            let mut seen: std::collections::HashMap<
                nebula_core::ProjectId,
                Option<std::time::SystemTime>,
            > = std::collections::HashMap::new();
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
                    let stamp = worktree_probe_stamp(&project.repo_path);
                    if seen.get(&project.id) == Some(&stamp) {
                        continue;
                    }
                    // The stamp is only recorded on success, so a failed
                    // sync (repo briefly locked, git missing) retries.
                    match daemon.sync_project_worktrees(&project).await {
                        Ok(()) => {
                            seen.insert(project.id.clone(), stamp);
                        }
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
    let git_dir = repo_path.join(".git");
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
