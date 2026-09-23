//! Unix-socket server: accept loop and per-client request handling. The
//! PTY plane of a connection (attach replay, forwarding) lives in `attach`.

use crate::attach::{self, PaneSize};
use crate::pr_scope::CreatePrAgentSpec;
use crate::registry::{CreateAgentSpec, Daemon};
use anyhow::Result;
use nebula_core::codec::{read_frame, write_frame};
use nebula_core::{ClientRequest, ServerEvent, SessionRef, PROTOCOL_VERSION};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::io::{AsyncWriteExt, BufWriter};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::mpsc;

/// The most of a ring one `TailOutput` copies, whatever the client asked:
/// the answer is built on the request loop, ahead of the next Input frame.
const TAIL_MAX_BYTES: u32 = 64 * 1024;

pub async fn accept_loop(daemon: Arc<Daemon>, listener: UnixListener) {
    loop {
        tokio::select! {
            _ = daemon.shutdown.cancelled() => break,
            accepted = listener.accept() => match accepted {
                Ok((stream, _)) => {
                    let daemon = daemon.clone();
                    tokio::spawn(async move {
                        if let Err(e) = handle_client(daemon, stream).await {
                            tracing::debug!(error = %e, "client connection ended with error");
                        }
                    });
                }
                Err(e) => {
                    tracing::warn!(error = %e, "accept failed");
                    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                }
            },
        }
    }
}

async fn handle_client(daemon: Arc<Daemon>, stream: UnixStream) -> Result<()> {
    let (read_half, write_half) = stream.into_split();
    let mut reader = tokio::io::BufReader::new(read_half);

    // Single writer task; everything else sends frames through this channel
    // so PTY forwards and RPC replies never interleave mid-frame.
    let (out_tx, mut out_rx) = mpsc::channel::<ServerEvent>(256);
    let writer_task = tokio::spawn(async move {
        let mut w = BufWriter::new(write_half);
        while let Some(ev) = out_rx.recv().await {
            if write_frame(&mut w, &ev).await.is_err() {
                break;
            }
        }
        let _ = w.shutdown().await;
    });

    // Per-connection attach state: forward-task handles keyed by session.
    let mut attached: HashMap<SessionRef, tokio::task::JoinHandle<()>> = HashMap::new();
    let mut handshaken = false;

    let result: Result<()> = async {
        while let Some(req) = read_frame::<ClientRequest, _>(&mut reader).await? {
            match req {
                ClientRequest::Hello { protocol_version } => {
                    handshaken = protocol_version == PROTOCOL_VERSION;
                    let reply = if handshaken {
                        ServerEvent::HelloOk {
                            protocol_version: PROTOCOL_VERSION,
                            daemon_pid: std::process::id(),
                        }
                    } else {
                        ServerEvent::Incompatible {
                            daemon_protocol_version: PROTOCOL_VERSION,
                        }
                    };
                    let closing = !handshaken;
                    let _ = out_tx.send(reply).await;
                    if closing {
                        break;
                    }
                }
                _ if !handshaken => {
                    let _ = out_tx
                        .send(ServerEvent::Error {
                            req_id: None,
                            message: "handshake required".into(),
                        })
                        .await;
                    break;
                }
                ClientRequest::Subscribe => {
                    let snapshot = daemon.snapshot().unwrap_or(ServerEvent::Snapshot {
                        projects: vec![],
                        worktrees: vec![],
                        agents: vec![],
                        terminals: vec![],
                        links: vec![],
                        pr_seen: vec![],
                        ui_state: None,
                    });
                    let _ = out_tx.send(snapshot).await;
                    let mut rx = daemon.events.subscribe();
                    let tx = out_tx.clone();
                    tokio::spawn(async move {
                        loop {
                            match rx.recv().await {
                                Ok(ev) => {
                                    if tx.send(ev).await.is_err() {
                                        break;
                                    }
                                }
                                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                                    continue
                                }
                                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                            }
                        }
                    });
                }
                ClientRequest::Attach {
                    session: sref,
                    from_seq,
                    cols,
                    rows,
                } => {
                    match daemon.ensure_session(&sref, cols, rows) {
                        Ok(session) => {
                            // Replay inline, before the loop takes the next
                            // request, so the Scrollback precedes any later
                            // reply; the forward task follows from there.
                            let size = PaneSize { cols, rows };
                            let (events_rx, replay_end) =
                                attach::bind(&session, &sref, &out_tx, size, from_seq).await;
                            // A RUN TERMINAL whose command already exited
                            // replays how it ended. Say it is over, or the
                            // pane would offer to type into a process that
                            // is gone.
                            if let Some(exit_code) = daemon.finished_run_exit(&sref) {
                                let _ = out_tx
                                    .send(ServerEvent::SessionExited {
                                        session: sref.clone(),
                                        exit_code,
                                    })
                                    .await;
                            }

                            let rebind = attached.remove(&sref);
                            if let Some(old) = &rebind {
                                old.abort();
                            }
                            let handle = tokio::spawn(attach::forward(
                                daemon.clone(),
                                session,
                                sref.clone(),
                                events_rx,
                                out_tx.clone(),
                                replay_end,
                                size,
                            ));
                            // Count this connection once even across
                            // re-attaches to the same session.
                            if rebind.is_none() {
                                daemon.note_attached(&sref);
                            }
                            attached.insert(sref, handle);
                        }
                        Err(e) => {
                            let _ = out_tx
                                .send(ServerEvent::Error {
                                    req_id: None,
                                    message: format!("attach: {e:#}"),
                                })
                                .await;
                        }
                    }
                }
                ClientRequest::Detach { session } => {
                    if let Some(h) = attached.remove(&session) {
                        h.abort();
                        daemon.note_detached(&session);
                    }
                }
                ClientRequest::Input { session, data } => {
                    if let Some(s) = daemon.session(&session) {
                        if let Err(e) = s.write_input(&data) {
                            tracing::warn!(error = %e, "pty write failed");
                        }
                    }
                }
                ClientRequest::Resize {
                    session,
                    cols,
                    rows,
                } => {
                    if let Some(s) = daemon.session(&session) {
                        let _ = s.resize(cols, rows);
                    }
                }
                ClientRequest::Shutdown => {
                    tracing::info!("shutdown requested by client");
                    daemon.shutdown.cancel();
                    break;
                }
                ClientRequest::SaveUiState { json } => {
                    let _ = daemon.store.save_ui_state(&json);
                }
                ClientRequest::MarkPrSeen { url, marker } => {
                    let _ = daemon.store.mark_pr_seen(&url, &marker);
                }
                ClientRequest::MarkAgentSeen { id } => {
                    if let Err(e) = daemon.mark_agent_seen(&id) {
                        tracing::warn!(error = %e, "mark agent seen failed");
                    }
                }
                ClientRequest::GetMetrics { req_id } => {
                    // A machine-wide `ps` sweep takes tens of ms; keep it off
                    // the request loop so Input/Attach frames keep flowing.
                    let pids = daemon.session_pids();
                    let out_tx = out_tx.clone();
                    tokio::spawn(async move {
                        let snapshot =
                            tokio::task::spawn_blocking(move || crate::metrics::collect(pids))
                                .await;
                        if let Ok(snapshot) = snapshot {
                            let _ = out_tx.send(ServerEvent::Metrics { req_id, snapshot }).await;
                        }
                    });
                }
                ClientRequest::TailOutput {
                    req_id,
                    session,
                    max_bytes,
                    after_seq,
                } => {
                    // A few KB copied out of a ring: cheap enough inline.
                    let max = max_bytes.min(TAIL_MAX_BYTES) as usize;
                    let tail = daemon.session(&session).map(|s| s.tail(max, after_seq));
                    let _ = out_tx
                        .send(ServerEvent::OutputTail {
                            req_id,
                            session,
                            tail,
                        })
                        .await;
                }
                // ---- entity CRUD: run the op, reply Ack/Error ----
                ClientRequest::AddProject {
                    req_id,
                    path,
                    name,
                    create_missing,
                } => {
                    reply(
                        &out_tx,
                        req_id,
                        daemon
                            .add_project(&path, name, create_missing)
                            .await
                            .map(Some),
                    )
                    .await;
                }
                ClientRequest::RemoveProject { req_id, id } => {
                    reply_done(&out_tx, req_id, daemon.remove_project(&id)).await;
                }
                ClientRequest::RenameProject { req_id, id, name } => {
                    reply(
                        &out_tx,
                        req_id,
                        daemon.rename_project(&id, &name).map(|_| None),
                    )
                    .await;
                }
                ClientRequest::CreateWorktree {
                    req_id,
                    project,
                    branch,
                    base,
                } => {
                    // A create fetches `origin` and then runs the WORKTREE
                    // HOOK, each bounded by a 30 s timeout; off the request
                    // loop, like the delete below, so Input/Attach frames on
                    // this connection never wait on either.
                    let daemon = daemon.clone();
                    let out_tx = out_tx.clone();
                    tokio::spawn(async move {
                        reply(
                            &out_tx,
                            req_id,
                            daemon
                                .create_worktree(&project, &branch, base.as_deref())
                                .await
                                .map(Some),
                        )
                        .await;
                    });
                }
                ClientRequest::DeleteWorktree { req_id, id, force } => {
                    // `git worktree remove` can take seconds on a large
                    // checkout; run it off the request loop so Input/Attach
                    // frames keep flowing while it grinds. `worktree_ops`
                    // still serializes it against create/sync.
                    let daemon = daemon.clone();
                    let out_tx = out_tx.clone();
                    tokio::spawn(async move {
                        reply_done(&out_tx, req_id, daemon.delete_worktree(&id, force).await).await;
                    });
                }
                ClientRequest::CreateAgent {
                    req_id,
                    worktree,
                    name,
                    kind,
                    custom_harness,
                    model,
                    effort,
                    auto_title,
                    cloud_prompt,
                    starting_prompt,
                    issue_url,
                } => {
                    // Logged by mode only — never the task, prompt text or
                    // issue URL.
                    let launch_mode = match (&cloud_prompt, &starting_prompt, &issue_url) {
                        (Some(_), _, _) => Some("cloud"),
                        (None, _, Some(_)) => Some("issue"),
                        (None, Some(_), None) => Some("preset"),
                        (None, None, None) => None,
                    };
                    let result = daemon
                        .create_agent(CreateAgentSpec {
                            worktree: worktree.clone(),
                            name,
                            kind,
                            custom_harness,
                            model,
                            effort,
                            auto_title,
                            cloud_prompt,
                            starting_prompt,
                            pr_url: None,
                            issue_url,
                        })
                        .await;
                    if let Some(launch_mode) = launch_mode {
                        match &result {
                            Ok(nebula_core::EntityId::Agent(agent)) => tracing::info!(
                                req_id,
                                agent = %agent,
                                kind = kind.as_str(),
                                worktree = %worktree,
                                launch_mode,
                                "agent session spawned"
                            ),
                            Err(error) => tracing::warn!(
                                req_id,
                                error = %error,
                                kind = kind.as_str(),
                                worktree = %worktree,
                                launch_mode,
                                "agent session spawn failed"
                            ),
                            Ok(_) => unreachable!("CreateAgent returned a non-agent id"),
                        }
                    }
                    reply(&out_tx, req_id, result.map(Some)).await;
                }
                ClientRequest::CreatePrAgent {
                    req_id,
                    project,
                    name,
                    kind,
                    custom_harness,
                    model,
                    effort,
                    auto_title,
                    pr_url,
                    head,
                    starting_prompt,
                } => {
                    // A PR SESSION whose checkout does not exist yet is a
                    // fetch, a `git worktree add` and the WORKTREE HOOK
                    // before the CLI spawns — seconds, each step bounded by
                    // its own timeout. Off the request loop, like
                    // `CreateWorktree` above: the client put stand-in rows
                    // up precisely so the user keeps working meanwhile, and
                    // run inline every Attach, Input and Resize on this
                    // connection queued behind it — moving to another
                    // session showed nothing until the Ack. `worktree_ops`
                    // still serializes the checkout against every other
                    // worktree op.
                    let daemon = daemon.clone();
                    let out_tx = out_tx.clone();
                    tokio::spawn(async move {
                        let result = daemon
                            .create_pr_agent(CreatePrAgentSpec {
                                project: project.clone(),
                                name,
                                kind,
                                custom_harness,
                                model,
                                effort,
                                auto_title,
                                pr_url: pr_url.clone(),
                                head,
                                starting_prompt,
                            })
                            .await;
                        match &result {
                            Ok(nebula_core::EntityId::Agent(agent)) => tracing::info!(
                                req_id,
                                agent = %agent,
                                kind = kind.as_str(),
                                project = %project,
                                pr_url = %pr_url,
                                launch_mode = "pull_request",
                                "agent session spawned"
                            ),
                            Err(error) => tracing::warn!(
                                req_id,
                                error = %error,
                                kind = kind.as_str(),
                                project = %project,
                                pr_url = %pr_url,
                                launch_mode = "pull_request",
                                "agent session spawn failed"
                            ),
                            Ok(_) => unreachable!("CreatePrAgent returned a non-agent id"),
                        }
                        reply(&out_tx, req_id, result.map(Some)).await;
                    });
                }
                ClientRequest::PrewarmAgent {
                    worktree,
                    kind,
                    model,
                    effort,
                } => {
                    // Fire-and-forget: boot the CLI while the user is still
                    // typing the session name; CreateAgent adopts it. Runs
                    // off the request loop (the CLI probe can take a bit).
                    let daemon = daemon.clone();
                    tokio::spawn(async move {
                        if let Err(e) = daemon.prewarm_agent(&worktree, kind, model, effort).await {
                            tracing::debug!(error = %e, "prewarm failed");
                        }
                    });
                }
                ClientRequest::PrewarmWorktreeSessions {
                    worktree,
                    cols,
                    rows,
                } => {
                    // Returns at once: the sweep boots the worktree's dead
                    // sessions on its own task, staggered, skipping the one
                    // the Attach above already spawned. Racing that Attach is
                    // safe — ensure_session's spawn gate makes the
                    // check-and-spawn atomic, so neither can double-fork.
                    daemon.prewarm_worktree_sessions(&worktree, cols, rows);
                }
                ClientRequest::RenameAgent { req_id, id, name } => {
                    reply_done(&out_tx, req_id, daemon.rename_agent(&id, &name)).await;
                }
                ClientRequest::AutoRenameAgent { req_id, id, name } => {
                    reply_done(&out_tx, req_id, daemon.auto_rename_agent(&id, &name)).await;
                }
                ClientRequest::MoveAgent {
                    req_id,
                    id,
                    worktree,
                } => {
                    reply_done(&out_tx, req_id, daemon.move_agent(&id, &worktree)).await;
                }
                ClientRequest::SpawnSiblingAgent {
                    req_id,
                    id,
                    kind,
                    starting_prompt,
                } => {
                    // Logged by mode only — never the prompt text.
                    let result = daemon
                        .spawn_sibling_agent(&id, kind, &starting_prompt)
                        .await;
                    match &result {
                        Ok(nebula_core::EntityId::Agent(agent)) => tracing::info!(
                            req_id,
                            agent = %agent,
                            spawned_by = %id,
                            launch_mode = "sibling",
                            "agent session spawned"
                        ),
                        Err(error) => tracing::warn!(
                            req_id,
                            error = %error,
                            spawned_by = %id,
                            launch_mode = "sibling",
                            "agent session spawn failed"
                        ),
                        Ok(_) => unreachable!("SpawnSiblingAgent returned a non-agent id"),
                    }
                    reply(&out_tx, req_id, result.map(Some)).await;
                }
                ClientRequest::OpenFiles { req_id, id, paths } => {
                    reply_done(&out_tx, req_id, daemon.open_files(&id, paths)).await;
                }
                ClientRequest::EnterWorktree {
                    req_id,
                    id,
                    branch,
                    base,
                } => {
                    let ev = match daemon.enter_worktree(&id, &branch, base.as_deref()).await {
                        Ok((worktree, outcome)) => ServerEvent::WorktreeEntered {
                            req_id,
                            worktree,
                            outcome,
                        },
                        Err(e) => ServerEvent::Error {
                            req_id: Some(req_id),
                            message: format!("{e:#}"),
                        },
                    };
                    let _ = out_tx.send(ev).await;
                }
                ClientRequest::ArchiveAgent { req_id, id } => {
                    reply_done(&out_tx, req_id, daemon.archive_agent(&id)).await;
                }
                ClientRequest::UnarchiveAgent { req_id, id } => {
                    reply_done(&out_tx, req_id, daemon.unarchive_agent(&id)).await;
                }
                ClientRequest::DeleteAgent { req_id, id } => {
                    reply_done(&out_tx, req_id, daemon.delete_agent(&id)).await;
                }
                ClientRequest::RestartAgent { req_id, id } => {
                    reply_done(&out_tx, req_id, daemon.restart_agent(&id).await).await;
                }
                ClientRequest::SendCloudMessage {
                    req_id,
                    id,
                    message,
                } => {
                    tracing::info!(agent = %id, bytes = message.len(), "send to cloud session");
                    // `claude -p … --cloud` is a login shell and a network
                    // round trip — seconds. Off the request loop, like the
                    // worktree ops above: run inline, every keystroke and
                    // every session switch on this connection waited for
                    // it, and the pane the user went back to typing in
                    // looked hung until the message was sent.
                    let daemon = daemon.clone();
                    let out_tx = out_tx.clone();
                    tokio::spawn(async move {
                        reply_done(
                            &out_tx,
                            req_id,
                            daemon.send_cloud_message(&id, &message).await,
                        )
                        .await;
                    });
                }
                ClientRequest::CreateTerminal {
                    req_id,
                    worktree,
                    name,
                } => {
                    reply(
                        &out_tx,
                        req_id,
                        daemon.create_terminal(&worktree, name).map(Some),
                    )
                    .await;
                }
                ClientRequest::CreateLink {
                    req_id,
                    worktree,
                    url,
                } => {
                    reply(
                        &out_tx,
                        req_id,
                        daemon.create_link(&worktree, &url).map(Some),
                    )
                    .await;
                }
                ClientRequest::UpdateLink { req_id, id, url } => {
                    reply_done(&out_tx, req_id, daemon.update_link(&id, &url)).await;
                }
                ClientRequest::DeleteLink { req_id, id } => {
                    reply_done(&out_tx, req_id, daemon.delete_link(&id)).await;
                }
                ClientRequest::RenameTerminal { req_id, id, name } => {
                    reply_done(&out_tx, req_id, daemon.rename_terminal(&id, &name)).await;
                }
                ClientRequest::CloseTerminal { req_id, id } => {
                    reply_done(&out_tx, req_id, daemon.close_terminal(&id)).await;
                }
                ClientRequest::StartRun { req_id, worktree } => {
                    reply(&out_tx, req_id, daemon.start_run(&worktree).map(Some)).await;
                }
                ClientRequest::StopRun { req_id, worktree } => {
                    reply_done(&out_tx, req_id, daemon.stop_run(&worktree)).await;
                }
            }
        }
        Ok(())
    }
    .await;

    for (sref, h) in attached.drain() {
        h.abort();
        daemon.note_detached(&sref);
    }
    drop(out_tx);
    let _ = writer_task.await;
    result
}

/// [`reply`] for the requests that create nothing: success is a bare Ack.
async fn reply_done(out_tx: &mpsc::Sender<ServerEvent>, req_id: u64, result: anyhow::Result<()>) {
    reply(out_tx, req_id, result.map(|_| None)).await
}

async fn reply(
    out_tx: &mpsc::Sender<ServerEvent>,
    req_id: u64,
    result: anyhow::Result<Option<nebula_core::EntityId>>,
) {
    let ev = match result {
        Ok(created) => ServerEvent::Ack { req_id, created },
        Err(e) => ServerEvent::Error {
            req_id: Some(req_id),
            message: format!("{e:#}"),
        },
    };
    let _ = out_tx.send(ev).await;
}
