pub mod git;
pub mod hooks;
pub mod lifecycle;
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

    let sock = paths::socket_path();
    lifecycle::unlink_stale_socket(&sock);
    let listener = tokio::net::UnixListener::bind(&sock)
        .with_context(|| format!("bind {}", sock.display()))?;
    tracing::info!(pid = std::process::id(), socket = %sock.display(), "nebula daemon listening");

    let store = store::Store::open(&paths::db_path())?;
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
    let (hook_env, mut hook_rx) = hooks::start_hook_server().await?;
    tracing::info!(port = hook_env.port, "hook receiver listening");

    let daemon = registry::Daemon::new(store, hook_env);

    // Drain hook events into the status machines.
    {
        let daemon = daemon.clone();
        tokio::spawn(async move {
            while let Some((agent_id, event, session_id)) = hook_rx.recv().await {
                daemon.apply_hook_event(&agent_id, event, session_id);
            }
        });
    }

    // Deferred-finish recheck (held Stops drain to finished after grace).
    {
        let daemon = daemon.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(30));
            loop {
                tokio::select! {
                    _ = daemon.shutdown.cancelled() => break,
                    _ = interval.tick() => daemon.tick_status_machines(),
                }
            }
        });
    }

    // Worktrees created or removed outside nebula (an agent running
    // `git worktree add`, manual CLI use) should show up without a restart.
    // Git registers every worktree under `<repo>/.git/worktrees`, so a cheap
    // mtime probe of that dir gates the full `git worktree list` reconcile;
    // the first tick also runs one boot-time sync per project.
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
                    let stamp = std::fs::metadata(project.repo_path.join(".git/worktrees"))
                        .and_then(|m| m.modified())
                        .ok();
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
