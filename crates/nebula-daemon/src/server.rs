//! Unix-socket server: accept loop, per-client request handling, PTY
//! attach/forward plumbing.

use crate::pty::PtyEvent;
use crate::registry::Daemon;
use anyhow::Result;
use nebula_core::codec::{read_frame, write_frame};
use nebula_core::{ClientRequest, ServerEvent, SessionRef, PROTOCOL_VERSION};
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
                        ServerEvent::Incompatible { daemon_protocol_version: PROTOCOL_VERSION }
                    };
                    let closing = !handshaken;
                    let _ = out_tx.send(reply).await;
                    if closing {
                        break;
                    }
                }
                _ if !handshaken => {
                    let _ = out_tx
                        .send(ServerEvent::Error { req_id: None, message: "handshake required".into() })
                        .await;
                    break;
                }
                ClientRequest::Subscribe => {
                    let snapshot = daemon.snapshot().unwrap_or(ServerEvent::Snapshot {
                        projects: vec![],
                        worktrees: vec![],
                        agents: vec![],
                        terminals: vec![],
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
                                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                            }
                        }
                    });
                }
                ClientRequest::Attach { session: sref, from_seq, cols, rows } => {
                    match daemon.ensure_session(&sref, cols, rows) {
                        Ok(session) => {
                            // Subscribe BEFORE snapshotting so nothing falls in
                            // the gap; the forward task drops frames the
                            // snapshot already covers.
                            let events_rx = session.events.subscribe();
                            let (base_seq, data) = session.snapshot(from_seq);
                            let replay_end = base_seq + data.len() as u64;
                            let _ = out_tx
                                .send(ServerEvent::Scrollback { session: sref.clone(), base_seq, data })
                                .await;
                            let _ = out_tx
                                .send(ServerEvent::KittyFlags {
                                    session: sref.clone(),
                                    flags: session.kitty_flags(),
                                })
                                .await;
                            let _ = session.resize_with_jiggle(cols, rows);

                            if let Some(old) = attached.remove(&sref) {
                                old.abort();
                            }
                            let handle = tokio::spawn(forward_pty(
                                session.clone(),
                                sref.clone(),
                                events_rx,
                                out_tx.clone(),
                                replay_end,
                            ));
                            attached.insert(sref, handle);
                        }
                        Err(e) => {
                            let _ = out_tx
                                .send(ServerEvent::Error { req_id: None, message: format!("attach: {e:#}") })
                                .await;
                        }
                    }
                }
                ClientRequest::Detach { session } => {
                    if let Some(h) = attached.remove(&session) {
                        h.abort();
                    }
                }
                ClientRequest::Input { session, data } => {
                    if let Some(s) = daemon.session(&session) {
                        if let Err(e) = s.write_input(&data) {
                            tracing::warn!(error = %e, "pty write failed");
                        }
                    }
                }
                ClientRequest::Resize { session, cols, rows } => {
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
                // ---- entity CRUD: run the op, reply Ack/Error ----
                ClientRequest::AddProject { req_id, path, name } => {
                    reply(&out_tx, req_id, daemon.add_project(&path, name).await.map(Some)).await;
                }
                ClientRequest::RemoveProject { req_id, id } => {
                    reply(&out_tx, req_id, daemon.remove_project(&id).map(|_| None)).await;
                }
                ClientRequest::MoveProject { req_id, id, delta } => {
                    reply(&out_tx, req_id, daemon.move_project(&id, delta).map(|_| None)).await;
                }
                ClientRequest::SetProjectDivider { req_id, id, divider_after, label } => {
                    reply(&out_tx, req_id, daemon.set_project_divider(&id, divider_after, label).map(|_| None)).await;
                }
                ClientRequest::CreateWorktree { req_id, project, branch, base } => {
                    reply(
                        &out_tx,
                        req_id,
                        daemon.create_worktree(&project, &branch, base.as_deref()).await.map(Some),
                    )
                    .await;
                }
                ClientRequest::DeleteWorktree { req_id, id, force } => {
                    reply(&out_tx, req_id, daemon.delete_worktree(&id, force).await.map(|_| None)).await;
                }
                ClientRequest::CreateAgent { req_id, worktree, name } => {
                    reply(&out_tx, req_id, daemon.create_agent(&worktree, &name).map(Some)).await;
                }
                ClientRequest::RenameAgent { req_id, id, name } => {
                    reply(&out_tx, req_id, daemon.rename_agent(&id, &name).map(|_| None)).await;
                }
                ClientRequest::ArchiveAgent { req_id, id } => {
                    reply(&out_tx, req_id, daemon.archive_agent(&id).map(|_| None)).await;
                }
                ClientRequest::UnarchiveAgent { req_id, id } => {
                    reply(&out_tx, req_id, daemon.unarchive_agent(&id).map(|_| None)).await;
                }
                ClientRequest::DeleteAgent { req_id, id } => {
                    reply(&out_tx, req_id, daemon.delete_agent(&id).map(|_| None)).await;
                }
                ClientRequest::RestartAgent { req_id, id } => {
                    reply(&out_tx, req_id, daemon.restart_agent(&id).map(|_| None)).await;
                }
                ClientRequest::CreateTerminal { req_id, worktree, name } => {
                    reply(&out_tx, req_id, daemon.create_terminal(&worktree, name).map(Some)).await;
                }
                ClientRequest::RenameTerminal { req_id, id, name } => {
                    reply(&out_tx, req_id, daemon.rename_terminal(&id, &name).map(|_| None)).await;
                }
                ClientRequest::CloseTerminal { req_id, id } => {
                    reply(&out_tx, req_id, daemon.close_terminal(&id).map(|_| None)).await;
                }
            }
        }
        Ok(())
    }
    .await;

    for (_, h) in attached.drain() {
        h.abort();
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
                let payload = if skip > 0 { data[skip..].to_vec() } else { data };
                let send_seq = seq + skip as u64;
                min_seq = end;
                if out_tx
                    .send(ServerEvent::Output { session: sref.clone(), seq: send_seq, data: payload })
                    .await
                    .is_err()
                {
                    break;
                }
            }
            Ok(PtyEvent::Exited { exit_code }) => {
                let _ = out_tx.send(ServerEvent::SessionExited { session: sref.clone(), exit_code }).await;
                break;
            }
            Ok(PtyEvent::KittyFlags { flags }) => {
                if out_tx
                    .send(ServerEvent::KittyFlags { session: sref.clone(), flags })
                    .await
                    .is_err()
                {
                    break;
                }
            }
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
                    ServerEvent::Output { session: sref.clone(), seq: base_seq, data }
                } else {
                    ServerEvent::Scrollback { session: sref.clone(), base_seq, data }
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
        Err(e) => ServerEvent::Error { req_id: Some(req_id), message: format!("{e:#}") },
    };
    let _ = out_tx.send(ev).await;
}
