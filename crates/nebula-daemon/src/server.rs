//! Unix-socket server: accept loop, per-client request handling, PTY
//! attach/forward plumbing.

use crate::pty::PtyEvent;
use crate::registry::{CreateAgentSpec, Daemon};
use anyhow::Result;
use nebula_core::codec::{read_frame, write_frame};
use nebula_core::{ClientRequest, ServerEvent, SessionRef, WorkspaceId, PROTOCOL_VERSION};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::io::{AsyncWriteExt, BufWriter};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::mpsc;

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
    // Which workspace THIS client is scoped to. Per-connection on purpose:
    // two nebula instances are two independent views, so one switching
    // workspaces must not move the other. Pinned at Subscribe to whatever
    // the client was handed to boot into, because "the current default" is
    // not a stable answer — another instance switching moves it, and a
    // client that read it once must not silently follow. `None` outlives
    // Subscribe only for connections that never subscribe: the one-shot
    // `nebula add`, whose workspace genuinely is the current default.
    let mut workspace: Option<WorkspaceId> = None;

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
                        workspaces: vec![],
                        active_workspace: Default::default(),
                        projects: vec![],
                        worktrees: vec![],
                        agents: vec![],
                        terminals: vec![],
                        links: vec![],
                        pr_seen: vec![],
                        ui_state: None,
                    });
                    // Scope this client to the workspace it is being shown.
                    // First Subscribe only — a re-subscribe must not undo a
                    // switch the client made in between.
                    if workspace.is_none() {
                        if let ServerEvent::Snapshot {
                            active_workspace, ..
                        } = &snapshot
                        {
                            workspace = Some(active_workspace.clone());
                        }
                    }
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
                            // Subscribe BEFORE snapshotting so nothing falls in
                            // the gap; the forward task drops frames the
                            // snapshot already covers.
                            let events_rx = session.events.subscribe();
                            let (base_seq, data) = session.snapshot(from_seq);
                            let replay_end = base_seq + data.len() as u64;
                            let _ = out_tx
                                .send(ServerEvent::Scrollback {
                                    session: sref.clone(),
                                    base_seq,
                                    data,
                                })
                                .await;
                            let _ = out_tx
                                .send(ServerEvent::KittyFlags {
                                    session: sref.clone(),
                                    flags: session.kitty_flags(),
                                })
                                .await;
                            let _ = session.resize_with_jiggle(cols, rows);

                            let rebind = attached.remove(&sref);
                            if let Some(old) = &rebind {
                                old.abort();
                            }
                            let handle = tokio::spawn(forward_pty(
                                session.clone(),
                                sref.clone(),
                                events_rx,
                                out_tx.clone(),
                                replay_end,
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
                // ---- entity CRUD: run the op, reply Ack/Error ----
                ClientRequest::AddWorkspace { req_id, name } => {
                    reply(&out_tx, req_id, daemon.add_workspace(&name).map(Some)).await;
                }
                ClientRequest::RemoveWorkspace { req_id, id } => {
                    reply(&out_tx, req_id, daemon.remove_workspace(&id).map(|_| None)).await;
                }
                ClientRequest::RenameWorkspace { req_id, id, name } => {
                    reply(
                        &out_tx,
                        req_id,
                        daemon.rename_workspace(&id, &name).map(|_| None),
                    )
                    .await;
                }
                ClientRequest::OpenWorkspace { req_id, id } => {
                    // Scope this connection, and leave the pick behind as the
                    // default a fresh client boots into. A workspace that
                    // doesn't exist scopes nothing.
                    let result = daemon.set_default_workspace(&id);
                    if result.is_ok() {
                        workspace = Some(id);
                    }
                    reply(&out_tx, req_id, result.map(|_| None)).await;
                }
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
                            .add_project(&path, name, create_missing, workspace.clone())
                            .await
                            .map(Some),
                    )
                    .await;
                }
                ClientRequest::RemoveProject { req_id, id } => {
                    reply(&out_tx, req_id, daemon.remove_project(&id).map(|_| None)).await;
                }
                ClientRequest::MoveProject { req_id, id, delta } => {
                    reply(
                        &out_tx,
                        req_id,
                        daemon.move_project(&id, delta).map(|_| None),
                    )
                    .await;
                }
                ClientRequest::CreateWorktree {
                    req_id,
                    project,
                    branch,
                    base,
                } => {
                    reply(
                        &out_tx,
                        req_id,
                        daemon
                            .create_worktree(&project, &branch, base.as_deref())
                            .await
                            .map(Some),
                    )
                    .await;
                }
                ClientRequest::DeleteWorktree { req_id, id, force } => {
                    // `git worktree remove` can take seconds on a large
                    // checkout; run it off the request loop so Input/Attach
                    // frames keep flowing while it grinds. `worktree_ops`
                    // still serializes it against create/sync.
                    let daemon = daemon.clone();
                    let out_tx = out_tx.clone();
                    tokio::spawn(async move {
                        reply(
                            &out_tx,
                            req_id,
                            daemon.delete_worktree(&id, force).await.map(|_| None),
                        )
                        .await;
                    });
                }
                ClientRequest::SetWorktreePinned { req_id, id, pinned } => {
                    reply(
                        &out_tx,
                        req_id,
                        daemon.set_worktree_pinned(&id, pinned).map(|_| None),
                    )
                    .await;
                }
                ClientRequest::CreateAgent {
                    req_id,
                    worktree,
                    name,
                    kind,
                    model,
                    effort,
                    auto_title,
                    cloud_prompt,
                } => {
                    let is_cloud = cloud_prompt.is_some();
                    let result = daemon
                        .create_agent(CreateAgentSpec {
                            worktree: worktree.clone(),
                            name,
                            kind,
                            model,
                            effort,
                            auto_title,
                            cloud_prompt,
                        })
                        .await;
                    if is_cloud {
                        match &result {
                            Ok(nebula_core::EntityId::Agent(agent)) => tracing::info!(
                                req_id,
                                agent = %agent,
                                kind = kind.as_str(),
                                worktree = %worktree,
                                launch_mode = "cloud",
                                "agent session spawned"
                            ),
                            Err(error) => tracing::warn!(
                                req_id,
                                error = %error,
                                kind = kind.as_str(),
                                worktree = %worktree,
                                launch_mode = "cloud",
                                "agent session spawn failed"
                            ),
                            Ok(_) => unreachable!("CreateAgent returned a non-agent id"),
                        }
                    }
                    reply(&out_tx, req_id, result.map(Some)).await;
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
                    // Deliberately inline: an Attach for one of these
                    // sessions is then ordered after it instead of racing it
                    // (two concurrent ensure_session calls for the same sref
                    // would double-spawn). Spawns are forkpty-fast; the
                    // children boot in the background.
                    daemon.prewarm_worktree_sessions(&worktree, cols, rows);
                }
                ClientRequest::RenameAgent { req_id, id, name } => {
                    reply(
                        &out_tx,
                        req_id,
                        daemon.rename_agent(&id, &name).map(|_| None),
                    )
                    .await;
                }
                ClientRequest::AutoRenameAgent { req_id, id, name } => {
                    reply(
                        &out_tx,
                        req_id,
                        daemon.auto_rename_agent(&id, &name).map(|_| None),
                    )
                    .await;
                }
                ClientRequest::MoveAgent {
                    req_id,
                    id,
                    worktree,
                } => {
                    reply(
                        &out_tx,
                        req_id,
                        daemon.move_agent(&id, &worktree).map(|_| None),
                    )
                    .await;
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
                    reply(&out_tx, req_id, daemon.archive_agent(&id).map(|_| None)).await;
                }
                ClientRequest::UnarchiveAgent { req_id, id } => {
                    reply(&out_tx, req_id, daemon.unarchive_agent(&id).map(|_| None)).await;
                }
                ClientRequest::SetAgentPinned { req_id, id, pinned } => {
                    reply(
                        &out_tx,
                        req_id,
                        daemon.set_agent_pinned(&id, pinned).map(|_| None),
                    )
                    .await;
                }
                ClientRequest::DeleteAgent { req_id, id } => {
                    reply(&out_tx, req_id, daemon.delete_agent(&id).map(|_| None)).await;
                }
                ClientRequest::RestartAgent { req_id, id } => {
                    reply(
                        &out_tx,
                        req_id,
                        daemon.restart_agent(&id).await.map(|_| None),
                    )
                    .await;
                }
                ClientRequest::AttachCloudAgent { req_id, id } => {
                    reply(
                        &out_tx,
                        req_id,
                        daemon.attach_cloud_agent(&id).await.map(|_| None),
                    )
                    .await;
                }
                ClientRequest::SendCloudMessage {
                    req_id,
                    id,
                    message,
                } => {
                    tracing::info!(agent = %id, bytes = message.len(), "send to cloud session");
                    reply(
                        &out_tx,
                        req_id,
                        daemon.send_cloud_message(&id, &message).await.map(|_| None),
                    )
                    .await;
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
                    reply(&out_tx, req_id, daemon.update_link(&id, &url).map(|_| None)).await;
                }
                ClientRequest::DeleteLink { req_id, id } => {
                    reply(&out_tx, req_id, daemon.delete_link(&id).map(|_| None)).await;
                }
                ClientRequest::RenameTerminal { req_id, id, name } => {
                    reply(
                        &out_tx,
                        req_id,
                        daemon.rename_terminal(&id, &name).map(|_| None),
                    )
                    .await;
                }
                ClientRequest::CloseTerminal { req_id, id } => {
                    reply(&out_tx, req_id, daemon.close_terminal(&id).map(|_| None)).await;
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

/// Forward live PTY output/exit to one client, skipping bytes the attach
/// replay already delivered. On broadcast lag, resync with a fresh
/// Scrollback (the client resets its parser on every Scrollback frame).
async fn forward_pty(
    session: Arc<crate::pty::PtySession>,
    sref: SessionRef,
    mut rx: tokio::sync::broadcast::Receiver<PtyEvent>,
    out_tx: mpsc::Sender<ServerEvent>,
    mut min_seq: u64,
) {
    loop {
        match rx.recv().await {
            Ok(PtyEvent::Output { seq, data }) => {
                let end = seq + data.len() as u64;
                if end <= min_seq {
                    continue; // fully covered by the replay
                }
                let skip = min_seq.saturating_sub(seq) as usize;
                let payload = if skip > 0 {
                    data[skip..].to_vec()
                } else {
                    data
                };
                let send_seq = seq + skip as u64;
                min_seq = end;
                if out_tx
                    .send(ServerEvent::Output {
                        session: sref.clone(),
                        seq: send_seq,
                        data: payload,
                    })
                    .await
                    .is_err()
                {
                    break;
                }
            }
            Ok(PtyEvent::Exited { exit_code }) => {
                let _ = out_tx
                    .send(ServerEvent::SessionExited {
                        session: sref.clone(),
                        exit_code,
                    })
                    .await;
                break;
            }
            Ok(PtyEvent::KittyFlags { flags }) => {
                if out_tx
                    .send(ServerEvent::KittyFlags {
                        session: sref.clone(),
                        flags,
                    })
                    .await
                    .is_err()
                {
                    break;
                }
            }
            // Daemon-side only: the progress edge drives the status machine
            // and reaches clients as a StatusChanged, not as session output;
            // the cloud sightings reach them as the row's own upsert.
            Ok(
                PtyEvent::Progress { .. }
                | PtyEvent::CloudSession { .. }
                | PtyEvent::CloudAttachRejected,
            ) => {}
            Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                // Catch up from the ring. If the missed bytes are still
                // retained, send them as a plain Output continuation so the
                // client keeps its parser state; only when the gap has fallen
                // off the ring do we force a full replay (parser reset —
                // expensive on the client, so avoid it when possible).
                let wanted = min_seq;
                let (base_seq, data) = session.snapshot(Some(wanted));
                min_seq = base_seq + data.len() as u64;
                let ev = if base_seq == wanted {
                    ServerEvent::Output {
                        session: sref.clone(),
                        seq: base_seq,
                        data,
                    }
                } else {
                    ServerEvent::Scrollback {
                        session: sref.clone(),
                        base_seq,
                        data,
                    }
                };
                if out_tx.send(ev).await.is_err() {
                    break;
                }
            }
            Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
        }
    }
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
