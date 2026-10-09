//! Unix-socket server: accept loop and per-client request handling. The
//! PTY plane of a connection (attach replay, forwarding) lives in `attach`.

use crate::attach::{self, PaneSize};
use crate::pr_scope::CreatePrAgentSpec;
use crate::registry::{CreateAgentSpec, Daemon};
use anyhow::{Context, Result};
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

    let mut client = ClientConnection::new(daemon, out_tx);
    let result: Result<()> = async {
        while let Some(req) = read_frame::<ClientRequest, _>(&mut reader).await? {
            if !client.handle_request(req).await? {
                break;
            }
        }
        Ok(())
    }
    .await;

    client.cleanup().await;
    drop(client);
    let _ = writer_task.await;
    result
}

struct ClientConnection {
    daemon: Arc<Daemon>,
    out_tx: mpsc::Sender<ServerEvent>,
    attached: HashMap<SessionRef, tokio::task::JoinHandle<()>>,
    subscription: Option<tokio::task::JoinHandle<()>>,
    handshaken: bool,
}

impl ClientConnection {
    fn new(daemon: Arc<Daemon>, out_tx: mpsc::Sender<ServerEvent>) -> Self {
        Self {
            daemon,
            out_tx,
            attached: HashMap::new(),
            subscription: None,
            handshaken: false,
        }
    }

    async fn handle_request(&mut self, req: ClientRequest) -> Result<bool> {
        match req {
            ClientRequest::Hello { protocol_version } => {
                Ok(self.handle_hello(protocol_version).await)
            }
            _ if !self.handshaken => {
                self.send(ServerEvent::Error {
                    req_id: None,
                    message: "handshake required".into(),
                })
                .await;
                Ok(false)
            }
            ClientRequest::Subscribe => self.handle_subscribe().await,
            ClientRequest::Attach {
                session,
                from_seq,
                cols,
                rows,
            } => self.handle_attach(session, from_seq, cols, rows).await,
            ClientRequest::Detach { session } => {
                self.handle_detach(session);
                Ok(true)
            }
            ClientRequest::Input { session, data } => {
                self.handle_input(session, data).await;
                Ok(true)
            }
            ClientRequest::Resize {
                session,
                cols,
                rows,
            } => {
                self.handle_resize(session, cols, rows).await;
                Ok(true)
            }
            ClientRequest::Shutdown => {
                tracing::info!("shutdown requested by client");
                self.daemon.shutdown.cancel();
                Ok(false)
            }
            ClientRequest::SaveUiState { json } => {
                let _ = self
                    .daemon
                    .store_blocking(move |store| store.save_ui_state(&json))
                    .await;
                Ok(true)
            }
            ClientRequest::MarkPrSeen { url, marker } => {
                let _ = self
                    .daemon
                    .store_blocking(move |store| store.mark_pr_seen(&url, &marker))
                    .await;
                Ok(true)
            }
            ClientRequest::MarkAgentSeen { id } => {
                let result = self
                    .blocking_daemon(move |daemon| daemon.mark_agent_seen(&id))
                    .await;
                if let Err(e) = result {
                    tracing::warn!(error = %e, "mark agent seen failed");
                }
                Ok(true)
            }
            ClientRequest::GetMetrics { req_id } => {
                self.handle_get_metrics(req_id);
                Ok(true)
            }
            ClientRequest::TailOutput {
                req_id,
                session,
                max_bytes,
                after_seq,
            } => {
                self.handle_tail_output(req_id, session, max_bytes, after_seq)
                    .await
            }
            ClientRequest::AddProject {
                req_id,
                path,
                name,
                create_missing,
            } => {
                let result = self
                    .daemon
                    .add_project(&path, name, create_missing)
                    .await
                    .map(Some);
                reply(&self.out_tx, req_id, result).await;
                Ok(true)
            }
            ClientRequest::RemoveProject { req_id, id } => {
                let result = self
                    .blocking_daemon(move |daemon| daemon.remove_project(&id))
                    .await;
                reply_done(&self.out_tx, req_id, result).await;
                Ok(true)
            }
            ClientRequest::RenameProject { req_id, id, name } => {
                let result = self
                    .blocking_daemon(move |daemon| daemon.rename_project(&id, &name))
                    .await;
                reply(&self.out_tx, req_id, result.map(|_| None)).await;
                Ok(true)
            }
            ClientRequest::SetProjectPath { req_id, id, path } => {
                let result = self.daemon.set_project_path(&id, &path).await.map(|_| None);
                reply(&self.out_tx, req_id, result).await;
                Ok(true)
            }
            ClientRequest::CreateWorktree {
                req_id,
                project,
                branch,
                base,
            } => {
                self.spawn_create_worktree(req_id, project, branch, base);
                Ok(true)
            }
            ClientRequest::DeleteWorktree { req_id, id, force } => {
                self.spawn_delete_worktree(req_id, id, force);
                Ok(true)
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
                self.handle_create_agent(
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
                )
                .await;
                Ok(true)
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
                self.spawn_create_pr_agent(
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
                );
                Ok(true)
            }
            ClientRequest::PrewarmAgent {
                worktree,
                kind,
                model,
                effort,
            } => {
                let daemon = self.daemon.clone();
                tokio::spawn(async move {
                    if let Err(e) = daemon.prewarm_agent(&worktree, kind, model, effort).await {
                        tracing::debug!(error = %e, "prewarm failed");
                    }
                });
                Ok(true)
            }
            ClientRequest::PrewarmWorktreeSessions {
                worktree,
                cols,
                rows,
            } => {
                self.daemon.prewarm_worktree_sessions(&worktree, cols, rows);
                Ok(true)
            }
            ClientRequest::RenameAgent { req_id, id, name } => {
                let result = self
                    .blocking_daemon(move |daemon| daemon.rename_agent(&id, &name))
                    .await;
                reply_done(&self.out_tx, req_id, result).await;
                Ok(true)
            }
            ClientRequest::AutoRenameAgent { req_id, id, name } => {
                let result = self
                    .blocking_daemon(move |daemon| daemon.auto_rename_agent(&id, &name))
                    .await;
                reply_done(&self.out_tx, req_id, result).await;
                Ok(true)
            }
            ClientRequest::SpawnSiblingAgent {
                req_id,
                id,
                kind,
                starting_prompt,
                worktree,
                base,
            } => {
                self.handle_spawn_sibling(req_id, id, kind, starting_prompt, worktree, base)
                    .await;
                Ok(true)
            }
            ClientRequest::OpenFiles { req_id, id, paths } => {
                let result = self
                    .blocking_daemon(move |daemon| daemon.open_files(&id, paths))
                    .await;
                reply_done(&self.out_tx, req_id, result).await;
                Ok(true)
            }
            ClientRequest::EnterWorktree {
                req_id,
                id,
                branch,
                base,
            } => self.handle_enter_worktree(req_id, id, branch, base).await,
            ClientRequest::ArchiveAgent { req_id, id } => {
                let result = self
                    .blocking_daemon(move |daemon| daemon.archive_agent(&id))
                    .await;
                reply_done(&self.out_tx, req_id, result).await;
                Ok(true)
            }
            ClientRequest::UnarchiveAgent { req_id, id } => {
                let result = self
                    .blocking_daemon(move |daemon| daemon.unarchive_agent(&id))
                    .await;
                reply_done(&self.out_tx, req_id, result).await;
                Ok(true)
            }
            ClientRequest::DeleteAgent { req_id, id } => {
                let result = self
                    .blocking_daemon(move |daemon| daemon.delete_agent(&id))
                    .await;
                reply_done(&self.out_tx, req_id, result).await;
                Ok(true)
            }
            ClientRequest::RestartAgent { req_id, id } => {
                let result = self.daemon.restart_agent(&id).await;
                reply_done(&self.out_tx, req_id, result).await;
                Ok(true)
            }
            ClientRequest::MoveAgent {
                req_id,
                id,
                worktree,
            } => {
                let result = self
                    .blocking_daemon(move |daemon| daemon.move_agent(&id, &worktree))
                    .await;
                reply_done(&self.out_tx, req_id, result).await;
                Ok(true)
            }
            ClientRequest::SendCloudMessage {
                req_id,
                id,
                message,
            } => {
                self.spawn_send_cloud_message(req_id, id, message);
                Ok(true)
            }
            ClientRequest::CreateTerminal {
                req_id,
                worktree,
                name,
            } => {
                let result = self
                    .blocking_daemon(move |daemon| {
                        daemon.create_terminal(&worktree, name).map(Some)
                    })
                    .await;
                reply(&self.out_tx, req_id, result).await;
                Ok(true)
            }
            ClientRequest::UpdateLink { req_id, id, url } => {
                let result = self
                    .blocking_daemon(move |daemon| daemon.update_link(&id, &url))
                    .await;
                reply_done(&self.out_tx, req_id, result).await;
                Ok(true)
            }
            ClientRequest::DeleteLink { req_id, id } => {
                let result = self
                    .blocking_daemon(move |daemon| daemon.delete_link(&id))
                    .await;
                reply_done(&self.out_tx, req_id, result).await;
                Ok(true)
            }
            ClientRequest::RenameTerminal { req_id, id, name } => {
                let result = self
                    .blocking_daemon(move |daemon| daemon.rename_terminal(&id, &name))
                    .await;
                reply_done(&self.out_tx, req_id, result).await;
                Ok(true)
            }
            ClientRequest::CloseTerminal { req_id, id } => {
                let result = self
                    .blocking_daemon(move |daemon| daemon.close_terminal(&id))
                    .await;
                reply_done(&self.out_tx, req_id, result).await;
                Ok(true)
            }
            ClientRequest::StartRun { req_id, worktree } => {
                let result = self
                    .blocking_daemon(move |daemon| daemon.start_run(&worktree).map(Some))
                    .await;
                reply(&self.out_tx, req_id, result).await;
                Ok(true)
            }
            ClientRequest::StopRun { req_id, worktree } => {
                let result = self
                    .blocking_daemon(move |daemon| daemon.stop_run(&worktree))
                    .await;
                reply_done(&self.out_tx, req_id, result).await;
                Ok(true)
            }
        }
    }

    async fn handle_hello(&mut self, protocol_version: u32) -> bool {
        self.handshaken = protocol_version == PROTOCOL_VERSION;
        let reply = if self.handshaken {
            ServerEvent::HelloOk {
                protocol_version: PROTOCOL_VERSION,
                daemon_pid: std::process::id(),
            }
        } else {
            ServerEvent::Incompatible {
                daemon_protocol_version: PROTOCOL_VERSION,
            }
        };
        self.send(reply).await;
        self.handshaken
    }

    async fn handle_subscribe(&mut self) -> Result<bool> {
        // Subscribed before the snapshot is taken, so a change made between
        // the two is not lost: it arrives after the Snapshot, where a client
        // folds it in by id.
        let mut rx = self.daemon.events.subscribe();
        let snapshot = self
            .blocking_daemon(|daemon| daemon.snapshot())
            .await
            .unwrap_or(ServerEvent::Snapshot {
                projects: vec![],
                worktrees: vec![],
                agents: vec![],
                terminals: vec![],
                links: vec![],
                pr_seen: vec![],
                ui_state: None,
            });
        self.send(snapshot).await;
        let tx = self.out_tx.clone();
        let forward = tokio::spawn(async move {
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
        if let Some(old) = self.subscription.replace(forward) {
            old.abort();
        }
        Ok(true)
    }

    async fn handle_attach(
        &mut self,
        sref: SessionRef,
        from_seq: Option<u64>,
        cols: u16,
        rows: u16,
    ) -> Result<bool> {
        let target = sref.clone();
        match self
            .blocking_daemon(move |daemon| daemon.ensure_session(&target, cols, rows))
            .await
        {
            Ok(session) => {
                // Replay inline, before the loop takes the next request, so
                // the Scrollback precedes any later reply; the forward task
                // follows from there.
                let size = PaneSize { cols, rows };
                let (events_rx, replay_end) =
                    attach::bind(&session, &sref, &self.out_tx, size, from_seq).await;
                // A RUN TERMINAL whose command already exited replays how it
                // ended. Say it is over, or the pane would offer to type into
                // a process that is gone.
                if let Some(exit_code) = self.daemon.finished_run_exit(&sref) {
                    self.send(ServerEvent::SessionExited {
                        session: sref.clone(),
                        exit_code,
                    })
                    .await;
                }

                let rebind = self.attached.remove(&sref);
                if let Some(old) = &rebind {
                    old.abort();
                }
                let handle = tokio::spawn(attach::forward(
                    self.daemon.clone(),
                    session,
                    sref.clone(),
                    events_rx,
                    self.out_tx.clone(),
                    replay_end,
                    size,
                ));
                // Count this connection once even across re-attaches to the
                // same session.
                if rebind.is_none() {
                    self.daemon.note_attached(&sref);
                }
                self.attached.insert(sref, handle);
            }
            Err(e) => {
                self.send(ServerEvent::AttachRefused {
                    session: sref,
                    message: format!("{e:#}"),
                })
                .await;
            }
        }
        Ok(true)
    }

    fn handle_detach(&mut self, session: SessionRef) {
        if let Some(h) = self.attached.remove(&session) {
            h.abort();
            self.daemon.note_detached(&session);
        }
    }

    async fn handle_input(&self, session: SessionRef, data: Vec<u8>) {
        let result = self
            .blocking_daemon(move |daemon| {
                if let Some(s) = daemon.session(&session) {
                    if let Err(e) = s.write_input(&data) {
                        tracing::warn!(error = %e, "pty write failed");
                    }
                }
                Ok(())
            })
            .await;
        if let Err(e) = result {
            tracing::warn!(error = %e, "input handling failed");
        }
    }

    async fn handle_resize(&self, session: SessionRef, cols: u16, rows: u16) {
        let result = self
            .blocking_daemon(move |daemon| {
                if let Some(s) = daemon.session(&session) {
                    let _ = s.resize(cols, rows);
                }
                Ok(())
            })
            .await;
        if let Err(e) = result {
            tracing::warn!(error = %e, "resize handling failed");
        }
    }

    fn handle_get_metrics(&self, req_id: u64) {
        // A machine-wide `ps` sweep takes tens of ms; keep it off the request
        // loop so Input/Attach frames keep flowing.
        let pids = self.daemon.session_pids();
        let out_tx = self.out_tx.clone();
        tokio::spawn(async move {
            let snapshot = tokio::task::spawn_blocking(move || crate::metrics::collect(pids)).await;
            if let Ok(snapshot) = snapshot {
                let _ = out_tx.send(ServerEvent::Metrics { req_id, snapshot }).await;
            }
        });
    }

    async fn handle_tail_output(
        &self,
        req_id: u64,
        session: SessionRef,
        max_bytes: u32,
        after_seq: Option<u64>,
    ) -> Result<bool> {
        // A few KB copied out of a ring: cheap enough inline.
        let max = max_bytes.min(TAIL_MAX_BYTES) as usize;
        let tail = self
            .daemon
            .session(&session)
            .map(|s| s.tail(max, after_seq));
        self.send(ServerEvent::OutputTail {
            req_id,
            session,
            tail,
        })
        .await;
        Ok(true)
    }

    #[allow(clippy::too_many_arguments)]
    async fn handle_create_agent(
        &self,
        req_id: u64,
        worktree: nebula_core::WorktreeId,
        name: String,
        kind: nebula_core::AgentKind,
        custom_harness: Option<String>,
        model: Option<String>,
        effort: Option<String>,
        auto_title: bool,
        cloud_prompt: Option<String>,
        starting_prompt: Option<String>,
        issue_url: Option<String>,
    ) {
        // Logged by mode only — never the task, prompt text or issue URL.
        let launch_mode = match (&cloud_prompt, &starting_prompt, &issue_url) {
            (Some(_), _, _) => Some("cloud"),
            (None, _, Some(_)) => Some("issue"),
            (None, Some(_), None) => Some("preset"),
            (None, None, None) => None,
        };
        let result = self
            .daemon
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
            log_spawn_result(
                req_id,
                &result,
                kind,
                launch_mode,
                "agent session",
                |fields| {
                    fields.field("worktree", worktree.to_string());
                },
            );
        }
        reply(&self.out_tx, req_id, result.map(Some)).await;
    }

    fn spawn_create_worktree(
        &self,
        req_id: u64,
        project: nebula_core::ProjectId,
        branch: String,
        base: Option<String>,
    ) {
        // A create fetches `origin` and then runs the WORKTREE HOOK, each
        // bounded by a 30 s timeout; off the request loop, like the delete
        // below, so Input/Attach frames on this connection never wait.
        let daemon = self.daemon.clone();
        let out_tx = self.out_tx.clone();
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

    fn spawn_delete_worktree(&self, req_id: u64, id: nebula_core::WorktreeId, force: bool) {
        // `git worktree remove` can take seconds on a large checkout; run it
        // off the request loop so Input/Attach frames keep flowing while it
        // grinds. `worktree_ops` still serializes it against create/sync.
        let daemon = self.daemon.clone();
        let out_tx = self.out_tx.clone();
        tokio::spawn(async move {
            reply_done(&out_tx, req_id, daemon.delete_worktree(&id, force).await).await;
        });
    }

    #[allow(clippy::too_many_arguments)]
    fn spawn_create_pr_agent(
        &self,
        req_id: u64,
        project: nebula_core::ProjectId,
        name: String,
        kind: nebula_core::AgentKind,
        custom_harness: Option<String>,
        model: Option<String>,
        effort: Option<String>,
        auto_title: bool,
        pr_url: String,
        head: String,
        starting_prompt: Option<String>,
    ) {
        // A PR SESSION whose checkout does not exist yet is a fetch, a `git
        // worktree add` and the WORKTREE HOOK before the CLI spawns. Off the
        // request loop like `CreateWorktree`; `worktree_ops` still serializes
        // the checkout against every other worktree op.
        let daemon = self.daemon.clone();
        let out_tx = self.out_tx.clone();
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

    async fn handle_spawn_sibling(
        &self,
        req_id: u64,
        id: nebula_core::AgentId,
        kind: Option<nebula_core::AgentKind>,
        starting_prompt: String,
        worktree: Option<String>,
        base: Option<String>,
    ) {
        // Logged by mode only — never the prompt text.
        let worktree = worktree
            .as_deref()
            .map(|branch| crate::sibling::SiblingWorktree {
                branch,
                base: base.as_deref(),
            });
        let result = self
            .daemon
            .spawn_sibling_agent(&id, kind, worktree, &starting_prompt)
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
        reply(&self.out_tx, req_id, result.map(Some)).await;
    }

    async fn handle_enter_worktree(
        &self,
        req_id: u64,
        id: nebula_core::AgentId,
        branch: String,
        base: Option<String>,
    ) -> Result<bool> {
        let ev = match self
            .daemon
            .enter_worktree(&id, &branch, base.as_deref())
            .await
        {
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
        self.send(ev).await;
        Ok(true)
    }

    fn spawn_send_cloud_message(&self, req_id: u64, id: nebula_core::AgentId, message: String) {
        tracing::info!(agent = %id, bytes = message.len(), "send to cloud session");
        // `claude -p … --cloud` is a login shell and a network round trip —
        // seconds. Off the request loop: inline, every keystroke and every
        // session switch on this connection waited for it.
        let daemon = self.daemon.clone();
        let out_tx = self.out_tx.clone();
        tokio::spawn(async move {
            reply_done(
                &out_tx,
                req_id,
                daemon.send_cloud_message(&id, &message).await,
            )
            .await;
        });
    }

    async fn blocking_daemon<T, F>(&self, f: F) -> Result<T>
    where
        T: Send + 'static,
        F: FnOnce(Arc<Daemon>) -> Result<T> + Send + 'static,
    {
        let daemon = self.daemon.clone();
        tokio::task::spawn_blocking(move || f(daemon))
            .await
            .context("daemon blocking task panicked")?
    }

    async fn send(&self, ev: ServerEvent) {
        let _ = self.out_tx.send(ev).await;
    }

    async fn cleanup(&mut self) {
        for (sref, h) in self.attached.drain() {
            h.abort();
            self.daemon.note_detached(&sref);
        }
        // The forwarder holds a sender and waits on the next broadcast: left
        // running, it keeps the writer, and this client's socket, open until
        // the daemon next has something to say.
        if let Some(forward) = self.subscription.take() {
            forward.abort();
        }
    }
}

struct SpawnLogFields(Vec<(&'static str, String)>);

impl SpawnLogFields {
    fn field(&mut self, name: &'static str, value: String) {
        self.0.push((name, value));
    }
}

fn log_spawn_result<F>(
    req_id: u64,
    result: &anyhow::Result<nebula_core::EntityId>,
    kind: nebula_core::AgentKind,
    launch_mode: &'static str,
    label: &'static str,
    fields: F,
) where
    F: FnOnce(&mut SpawnLogFields),
{
    let mut extra = SpawnLogFields(Vec::new());
    fields(&mut extra);
    match result {
        Ok(nebula_core::EntityId::Agent(agent)) => {
            tracing::info!(
                req_id,
                agent = %agent,
                kind = kind.as_str(),
                launch_mode,
                extra = ?extra.0,
                "{label} spawned"
            );
        }
        Err(error) => {
            tracing::warn!(
                req_id,
                error = %error,
                kind = kind.as_str(),
                launch_mode,
                extra = ?extra.0,
                "{label} spawn failed"
            );
        }
        Ok(_) => unreachable!("CreateAgent returned a non-agent id"),
    }
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
