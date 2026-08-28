//! End-to-end daemon tests over the real IPC surface: entity CRUD, PTY
//! attach/detach with scrollback replay, git worktree ops, and persistence
//! across a daemon restart.

use nebula_core::codec::{read_frame, write_frame};
use nebula_core::env;
use nebula_core::{
    AgentKind, ClientRequest, Entity, EntityId, ServerEvent, SessionRef, PROTOCOL_VERSION,
};
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio::net::UnixStream;

/// How long a daemon reply or broadcast may take to arrive.
const EVENT_TIMEOUT: Duration = Duration::from_secs(5);
/// Same, for events that wait on a PTY child (spawn, exit, hook round-trip).
const SLOW_TIMEOUT: Duration = Duration::from_secs(10);
/// Same, for chains of several respawns or a cloud-mirror follow.
const SPAWN_CHAIN_TIMEOUT: Duration = Duration::from_secs(20);
/// Sleep between polls of the filesystem or a counter.
const POLL_STEP: Duration = Duration::from_millis(50);

struct TestEnv {
    tmp: tempfile::TempDir,
    runtime_dir: PathBuf,
}

impl TestEnv {
    fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let runtime_dir = tmp.path().join("rt");
        Self { tmp, runtime_dir }
    }

    fn sock(&self) -> PathBuf {
        self.runtime_dir.join("daemon.sock")
    }

    /// The `nebula` binary under test, pointed at this env's runtime and
    /// data dirs — the base every daemon spawn and one-shot CLI run shares.
    fn cli(&self) -> std::process::Command {
        let mut cmd = std::process::Command::new(env!("CARGO_BIN_EXE_nebula"));
        cmd.env(env::RUNTIME_DIR, &self.runtime_dir)
            .env(env::DATA_DIR, self.tmp.path().join("data"));
        cmd
    }

    fn spawn_daemon(&self) -> DaemonProc {
        self.spawn_daemon_with_agent_cmd("/bin/sh") // no real claude in tests
    }

    fn spawn_daemon_with_agent_cmd(&self, agent_cmd: &str) -> DaemonProc {
        self.spawn_daemon_with(agent_cmd, &[])
    }

    fn spawn_daemon_with(&self, agent_cmd: &str, envs: &[(&str, &str)]) -> DaemonProc {
        self.spawn_daemon_in(Path::new("/bin/sh"), Some(agent_cmd), envs)
    }

    /// Daemon with no `NEBULA_AGENT_CMD` override, so agent spawns take the
    /// real login-shell path, and `$SHELL` set to `shell`. Lets a test decide
    /// what the daemon can find on PATH.
    fn spawn_daemon_with_shell(&self, shell: &Path) -> DaemonProc {
        self.spawn_daemon_in(shell, None, &[])
    }

    fn spawn_daemon_in(
        &self,
        shell: &Path,
        agent_cmd: Option<&str>,
        envs: &[(&str, &str)],
    ) -> DaemonProc {
        let mut cmd = self.cli();
        cmd.args(["daemon", "--foreground"])
            .env("SHELL", shell)
            .env(env::WORKTREE_SYNC_MS, "100") // fast external-worktree pickup
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        match agent_cmd {
            Some(agent_cmd) => cmd.env(env::AGENT_CMD, agent_cmd),
            None => cmd.env_remove(env::AGENT_CMD),
        };
        for (k, v) in envs {
            cmd.env(k, v);
        }
        DaemonProc(cmd.spawn().unwrap())
    }

    /// A `$SHELL` that answers `-l -i -c` but sees no agent CLI on PATH.
    fn blind_shell(&self) -> PathBuf {
        let path = self.tmp.path().join("blind-shell.sh");
        std::fs::write(
            &path,
            "#!/bin/sh\nPATH=/usr/bin:/bin\nexport PATH\nexec /bin/sh -c \"$4\"\n",
        )
        .unwrap();
        make_executable(&path);
        path
    }

    /// Write the daemon's `config.json` (read from `NEBULA_DATA_DIR`)
    /// before boot.
    fn write_config(&self, json: &str) {
        let data = self.tmp.path().join("data");
        std::fs::create_dir_all(&data).unwrap();
        std::fs::write(data.join("config.json"), json).unwrap();
    }

    /// A committed git repo to act as the project.
    fn make_repo(&self) -> PathBuf {
        let repo = self.tmp.path().join("repo");
        make_repo_at(&repo);
        repo
    }
}

/// A daemon spawned for one test, killed when the test's scope ends.
///
/// Without this, a test that panics before its closing `Shutdown` — a failed
/// assertion, a timeout — leaks its `nebula daemon --foreground`. Nothing
/// reaps it: it detaches from the test binary and outlives the whole `cargo
/// test` run. They pile up across days of development, and dozens of them
/// holding watchers and fds starve *later* runs' daemons, which surfaces as
/// every test in the file failing with "daemon socket never appeared" —
/// an error that points nowhere near the actual cause.
struct DaemonProc(std::process::Child);

impl std::ops::Deref for DaemonProc {
    type Target = std::process::Child;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl std::ops::DerefMut for DaemonProc {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl Drop for DaemonProc {
    fn drop(&mut self) {
        // Already exited (the clean path: `Shutdown` + `wait_for_exit`).
        // Checking matters — `Child::kill` on a reaped child is an error
        // rather than a signal to whatever now owns that recycled pid, but
        // there is nothing to do here either way.
        if matches!(self.0.try_wait(), Ok(Some(_))) {
            return;
        }
        // SIGTERM, not SIGKILL: the daemon's handler runs the same clean
        // shutdown as `Shutdown`, taking its PTY children down with it.
        // SIGKILL would leave those orphaned instead.
        let _ = std::process::Command::new("kill")
            .args(["-TERM", &self.0.id().to_string()])
            .stderr(std::process::Stdio::null())
            .status();
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        while std::time::Instant::now() < deadline {
            if matches!(self.0.try_wait(), Ok(Some(_))) {
                return;
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

async fn connect(sock: &Path) -> UnixStream {
    let deadline = tokio::time::Instant::now() + EVENT_TIMEOUT;
    loop {
        match UnixStream::connect(sock).await {
            Ok(s) => return s,
            Err(_) if tokio::time::Instant::now() < deadline => tokio::time::sleep(POLL_STEP).await,
            Err(e) => panic!("daemon socket never appeared: {e}"),
        }
    }
}

async fn handshake(stream: &mut UnixStream) {
    write_frame(
        stream,
        &ClientRequest::Hello {
            protocol_version: PROTOCOL_VERSION,
        },
    )
    .await
    .unwrap();
    match read_frame::<ServerEvent, _>(stream).await.unwrap() {
        Some(ServerEvent::HelloOk { .. }) => {}
        other => panic!("bad handshake reply: {other:?}"),
    }
}

/// Collect events until `pred` says done (returns all seen events).
async fn read_events_until(
    stream: &mut UnixStream,
    timeout: Duration,
    mut pred: impl FnMut(&[ServerEvent]) -> bool,
) -> Vec<ServerEvent> {
    let mut seen = Vec::new();
    let ok = tokio::time::timeout(timeout, async {
        loop {
            match read_frame::<ServerEvent, _>(stream).await.unwrap() {
                Some(ev) => {
                    seen.push(ev);
                    if pred(&seen) {
                        return;
                    }
                }
                None => panic!("daemon closed connection early"),
            }
        }
    })
    .await;
    assert!(ok.is_ok(), "timed out waiting for events; saw: {seen:#?}");
    seen
}

fn find_ack(events: &[ServerEvent], want_req: u64) -> Option<&ServerEvent> {
    events.iter().find(|e| {
        matches!(e, ServerEvent::Ack { req_id, .. } if *req_id == want_req)
            || matches!(e, ServerEvent::Error { req_id: Some(r), .. } if *r == want_req)
    })
}

fn collected_output(events: &[ServerEvent]) -> Vec<u8> {
    let mut out = Vec::new();
    for e in events {
        match e {
            ServerEvent::Scrollback { data, .. } | ServerEvent::Output { data, .. } => {
                out.extend_from_slice(data)
            }
            _ => {}
        }
    }
    out
}

#[tokio::test]
async fn full_crud_attach_and_restart_persistence() {
    let env = TestEnv::new();
    let repo = env.make_repo();
    let mut daemon = env.spawn_daemon();

    let mut c = connect(&env.sock()).await;
    handshake(&mut c).await;
    let events = subscribe(&mut c).await;
    match &events[0] {
        ServerEvent::Snapshot { projects, .. } => assert!(projects.is_empty()),
        other => panic!("expected snapshot first, got {other:?}"),
    }

    // ---- AddProject: creates project + main worktree row ----
    write_frame(
        &mut c,
        &ClientRequest::AddProject {
            req_id: 1,
            path: repo.clone(),
            name: None,
            create_missing: false,
        },
    )
    .await
    .unwrap();
    let events = read_events_until(&mut c, EVENT_TIMEOUT, |evs| {
        find_ack(evs, 1).is_some()
            && evs.iter().any(|e| {
                matches!(
                    e,
                    ServerEvent::EntityUpserted {
                        entity: Entity::Worktree(_)
                    }
                )
            })
    })
    .await;
    let ServerEvent::Ack {
        created: Some(EntityId::Project(project_id)),
        ..
    } = find_ack(&events, 1).unwrap()
    else {
        panic!("AddProject failed: {events:#?}");
    };
    let project_id = project_id.clone();
    let main_worktree = events
        .iter()
        .find_map(|e| match e {
            ServerEvent::EntityUpserted {
                entity: Entity::Worktree(w),
            } if w.is_main => Some(w.clone()),
            _ => None,
        })
        .expect("main worktree upsert");
    assert_eq!(main_worktree.branch, "main");

    // ---- CreateTerminal + attach + echo through the PTY ----
    write_frame(
        &mut c,
        &ClientRequest::CreateTerminal {
            req_id: 2,
            worktree: main_worktree.id.clone(),
            name: None,
        },
    )
    .await
    .unwrap();
    let events = read_events_until(&mut c, EVENT_TIMEOUT, |evs| find_ack(evs, 2).is_some()).await;
    let ServerEvent::Ack {
        created: Some(EntityId::Terminal(term_id)),
        ..
    } = find_ack(&events, 2).unwrap()
    else {
        panic!("CreateTerminal failed: {events:#?}");
    };
    let term_id = term_id.clone();

    let sref = SessionRef::Terminal(term_id);
    write_frame(
        &mut c,
        &ClientRequest::Attach {
            session: sref.clone(),
            from_seq: None,
            cols: 80,
            rows: 24,
        },
    )
    .await
    .unwrap();
    let marker = "nebula_e2e_marker_4519";
    write_frame(
        &mut c,
        &ClientRequest::Input {
            session: sref.clone(),
            data: format!("echo {marker}; pwd\n").into_bytes(),
        },
    )
    .await
    .unwrap();
    let events = read_events_until(&mut c, EVENT_TIMEOUT, |evs| {
        let text = String::from_utf8_lossy(&collected_output(evs)).into_owned();
        text.matches(marker).count() >= 2
    })
    .await;
    // The shell runs in the worktree directory.
    let text = String::from_utf8_lossy(&collected_output(&events)).into_owned();
    assert!(
        text.contains("repo"),
        "terminal cwd should be the worktree: {text}"
    );

    // ---- CreateAgent (NEBULA_AGENT_CMD=/bin/sh stands in for claude) ----
    write_frame(
        &mut c,
        &ClientRequest::CreateAgent {
            req_id: 3,
            worktree: main_worktree.id.clone(),
            name: "agent-1".into(),
            kind: AgentKind::Claude,
            model: None,
            effort: None,
            auto_title: false,
            cloud_prompt: None,
        },
    )
    .await
    .unwrap();
    let events = read_events_until(&mut c, EVENT_TIMEOUT, |evs| find_ack(evs, 3).is_some()).await;
    assert!(
        matches!(
            find_ack(&events, 3),
            Some(ServerEvent::Ack {
                created: Some(EntityId::Agent(_)),
                ..
            })
        ),
        "CreateAgent failed: {events:#?}"
    );

    // ---- CreateWorktree: real `git worktree add` on disk ----
    write_frame(
        &mut c,
        &ClientRequest::CreateWorktree {
            req_id: 4,
            project: project_id.clone(),
            branch: "feature-x".into(),
            base: None,
        },
    )
    .await
    .unwrap();
    let events = read_events_until(&mut c, SLOW_TIMEOUT, |evs| {
        find_ack(evs, 4).is_some()
            && evs.iter().any(|e| {
                matches!(e, ServerEvent::EntityUpserted { entity: Entity::Worktree(w) } if w.branch == "feature-x")
            })
    })
    .await;
    let ServerEvent::Ack {
        created: Some(EntityId::Worktree(feature_wt_id)),
        ..
    } = find_ack(&events, 4).unwrap()
    else {
        panic!("CreateWorktree failed: {events:#?}");
    };
    let feature_wt_id = feature_wt_id.clone();
    let feature_wt_path = events
        .iter()
        .find_map(|e| match e {
            ServerEvent::EntityUpserted {
                entity: Entity::Worktree(w),
            } if w.id == feature_wt_id => Some(w.path.clone()),
            _ => None,
        })
        .expect("worktree upsert carries its path");
    assert!(feature_wt_path.exists(), "worktree dir created on disk");

    // ---- DeleteWorktree removes it from disk ----
    write_frame(
        &mut c,
        &ClientRequest::DeleteWorktree {
            req_id: 5,
            id: feature_wt_id.clone(),
            force: true,
        },
    )
    .await
    .unwrap();
    let events = read_events_until(&mut c, SLOW_TIMEOUT, |evs| find_ack(evs, 5).is_some()).await;
    assert!(
        matches!(find_ack(&events, 5), Some(ServerEvent::Ack { .. })),
        "DeleteWorktree failed: {events:#?}"
    );
    assert!(!feature_wt_path.exists(), "worktree dir removed from disk");

    // ---- restart: tree persists, boot sweep marks nothing (agents fresh) ----
    write_frame(&mut c, &ClientRequest::Shutdown).await.unwrap();
    wait_for_exit(&mut daemon);

    let mut daemon2 = env.spawn_daemon();
    let mut c2 = connect(&env.sock()).await;
    handshake(&mut c2).await;
    write_frame(&mut c2, &ClientRequest::Subscribe)
        .await
        .unwrap();
    let events = read_events_until(&mut c2, EVENT_TIMEOUT, |evs| {
        evs.iter()
            .any(|e| matches!(e, ServerEvent::Snapshot { .. }))
    })
    .await;
    let ServerEvent::Snapshot {
        projects,
        worktrees,
        agents,
        terminals,
        ..
    } = &events[0]
    else {
        panic!("expected snapshot");
    };
    assert_eq!(projects.len(), 1, "project persisted");
    assert_eq!(worktrees.len(), 1, "only main worktree remains");
    assert_eq!(agents.len(), 1, "agent persisted");
    assert_eq!(agents[0].name, "agent-1");
    assert_eq!(terminals.len(), 1, "terminal persisted");
    assert!(!agents[0].alive, "no PTY after restart until reattach");

    // Reattach the persisted terminal: lazy respawn, cwd still the worktree.
    let sref2 = SessionRef::Terminal(terminals[0].id.clone());
    write_frame(
        &mut c2,
        &ClientRequest::Attach {
            session: sref2.clone(),
            from_seq: None,
            cols: 80,
            rows: 24,
        },
    )
    .await
    .unwrap();
    let marker2 = "nebula_e2e_after_restart_8846";
    write_frame(
        &mut c2,
        &ClientRequest::Input {
            session: sref2,
            data: format!("echo {marker2}\n").into_bytes(),
        },
    )
    .await
    .unwrap();
    read_events_until(&mut c2, EVENT_TIMEOUT, |evs| {
        String::from_utf8_lossy(&collected_output(evs))
            .matches(marker2)
            .count()
            >= 2
    })
    .await;

    write_frame(&mut c2, &ClientRequest::Shutdown)
        .await
        .unwrap();
    wait_for_exit(&mut daemon2);
}

/// The daemon must answer a child's kitty-keyboard support query (nothing
/// else ever would — the child talks to a virtual terminal), track pushed
/// flags, and tell attached clients so they switch key encodings. This is
/// what makes Cmd/Option combos reach Claude Code.
#[tokio::test]
async fn kitty_keyboard_negotiation_passthrough() {
    let env = TestEnv::new();
    let repo = env.make_repo();
    let mut daemon = env.spawn_daemon();

    let mut c = connect(&env.sock()).await;
    handshake(&mut c).await;
    write_frame(
        &mut c,
        &ClientRequest::AddProject {
            req_id: 1,
            path: repo.clone(),
            name: None,
            create_missing: false,
        },
    )
    .await
    .unwrap();
    let events = read_events_until(&mut c, EVENT_TIMEOUT, |evs| find_ack(evs, 1).is_some()).await;
    let ServerEvent::Ack {
        created: Some(EntityId::Project(_)),
        ..
    } = find_ack(&events, 1).unwrap()
    else {
        panic!("AddProject failed: {events:#?}");
    };
    // AddProject's worktree upsert goes to subscribers only; fetch it via the DB
    // snapshot path instead: create the terminal against the main worktree id
    // that Subscribe would report. Simplest: subscribe now.
    let events = subscribe(&mut c).await;
    let worktree_id = events
        .iter()
        .find_map(|e| match e {
            ServerEvent::Snapshot { worktrees, .. } => worktrees.first().map(|w| w.id.clone()),
            _ => None,
        })
        .expect("main worktree in snapshot");

    write_frame(
        &mut c,
        &ClientRequest::CreateTerminal {
            req_id: 2,
            worktree: worktree_id,
            name: None,
        },
    )
    .await
    .unwrap();
    let events = read_events_until(&mut c, EVENT_TIMEOUT, |evs| find_ack(evs, 2).is_some()).await;
    let ServerEvent::Ack {
        created: Some(EntityId::Terminal(term_id)),
        ..
    } = find_ack(&events, 2).unwrap()
    else {
        panic!("CreateTerminal failed: {events:#?}");
    };
    let sref = SessionRef::Terminal(term_id.clone());
    write_frame(
        &mut c,
        &ClientRequest::Attach {
            session: sref.clone(),
            from_seq: None,
            cols: 100,
            rows: 30,
        },
    )
    .await
    .unwrap();
    // Attach reports the child's current (legacy) flags right away.
    read_events_until(&mut c, EVENT_TIMEOUT, |evs| {
        evs.iter()
            .any(|e| matches!(e, ServerEvent::KittyFlags { flags: 0, .. }))
    })
    .await;

    // The child queries support and reads the daemon's reply off its own
    // stdin — the same detection recipe Claude Code uses. `tr` makes the
    // reply greppable in plain text.
    let probe = "stty -icanon -echo min 0 time 20; printf '\\033[?u'; sleep 1; \
                 printf 'REPLY:'; dd bs=64 count=1 2>/dev/null | tr '\\033' 'E'; echo; stty sane\n";
    write_frame(
        &mut c,
        &ClientRequest::Input {
            session: sref.clone(),
            data: probe.into(),
        },
    )
    .await
    .unwrap();
    read_events_until(&mut c, SLOW_TIMEOUT, |evs| {
        String::from_utf8_lossy(&collected_output(evs)).contains("REPLY:E[?0u")
    })
    .await;

    // Pushing flags reaches the attached client…
    write_frame(
        &mut c,
        &ClientRequest::Input {
            session: sref.clone(),
            data: b"printf '\\033[>1u'\n".to_vec(),
        },
    )
    .await
    .unwrap();
    read_events_until(&mut c, SLOW_TIMEOUT, |evs| {
        evs.iter()
            .any(|e| matches!(e, ServerEvent::KittyFlags { flags: 1, .. }))
    })
    .await;

    // …survives a re-attach (fresh client learns the current mode)…
    write_frame(
        &mut c,
        &ClientRequest::Attach {
            session: sref.clone(),
            from_seq: None,
            cols: 100,
            rows: 30,
        },
    )
    .await
    .unwrap();
    let events = read_events_until(&mut c, EVENT_TIMEOUT, |evs| {
        evs.iter()
            .any(|e| matches!(e, ServerEvent::KittyFlags { .. }))
    })
    .await;
    assert!(
        events
            .iter()
            .any(|e| matches!(e, ServerEvent::KittyFlags { flags: 1, .. })),
        "re-attach must report the pushed flags: {events:#?}"
    );

    // …and popping restores legacy.
    write_frame(
        &mut c,
        &ClientRequest::Input {
            session: sref.clone(),
            data: b"printf '\\033[<u'\n".to_vec(),
        },
    )
    .await
    .unwrap();
    read_events_until(&mut c, SLOW_TIMEOUT, |evs| {
        evs.iter()
            .any(|e| matches!(e, ServerEvent::KittyFlags { flags: 0, .. }))
    })
    .await;

    write_frame(&mut c, &ClientRequest::Shutdown).await.unwrap();
    wait_for_exit(&mut daemon);
}

/// True end-to-end status detection: the agent PTY (a /bin/sh stand-in for
/// claude) uses its *injected* NEBULA_* env to curl the daemon's hook
/// endpoint, exactly like the installed claude hooks would — and the
/// subscribed client sees StatusChanged.
#[tokio::test]
async fn hook_post_from_agent_pty_drives_status() {
    let env = TestEnv::new();
    let repo = env.make_repo();
    let mut daemon = env.spawn_daemon();

    let mut c = connect(&env.sock()).await;
    handshake(&mut c).await;
    subscribe(&mut c).await;

    write_frame(
        &mut c,
        &ClientRequest::AddProject {
            req_id: 1,
            path: repo.clone(),
            name: None,
            create_missing: false,
        },
    )
    .await
    .unwrap();
    let events = read_events_until(&mut c, EVENT_TIMEOUT, |evs| {
        evs.iter().any(|e| {
            matches!(
                e,
                ServerEvent::EntityUpserted {
                    entity: Entity::Worktree(_)
                }
            )
        })
    })
    .await;
    let worktree = events
        .iter()
        .find_map(|e| match e {
            ServerEvent::EntityUpserted {
                entity: Entity::Worktree(w),
            } => Some(w.clone()),
            _ => None,
        })
        .unwrap();

    write_frame(
        &mut c,
        &ClientRequest::CreateAgent {
            req_id: 2,
            worktree: worktree.id.clone(),
            name: "hooked".into(),
            kind: AgentKind::Claude,
            model: None,
            effort: None,
            auto_title: false,
            cloud_prompt: None,
        },
    )
    .await
    .unwrap();
    let events = read_events_until(&mut c, EVENT_TIMEOUT, |evs| find_ack(evs, 2).is_some()).await;
    let ServerEvent::Ack {
        created: Some(EntityId::Agent(agent_id)),
        ..
    } = find_ack(&events, 2).unwrap()
    else {
        panic!("CreateAgent failed: {events:#?}");
    };
    let agent_id = agent_id.clone();

    // Hook install happened at spawn: managed hooks exist in the worktree.
    let settings_path = repo.join(".claude/settings.local.json");
    assert!(settings_path.exists(), "hooks installed into worktree");
    let settings: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&settings_path).unwrap()).unwrap();
    assert!(settings["hooks"]["Stop"][0]["_nebulaManaged"]
        .as_bool()
        .unwrap());

    // Drive the shell inside the agent PTY to POST hooks with its own env.
    let sref = SessionRef::Agent(agent_id.clone());
    write_frame(
        &mut c,
        &ClientRequest::Attach {
            session: sref.clone(),
            from_seq: None,
            cols: 120,
            rows: 30,
        },
    )
    .await
    .unwrap();
    let curl = |event: &str, body: &str| {
        format!(
            "curl -sS -m 3 -X POST -H \"Authorization: Bearer $NEBULA_API_TOKEN\" \
             -H 'Content-Type: application/json' -d '{body}' \
             \"$NEBULA_API_URL/api/hooks/claude?agentId=$NEBULA_AGENT_ID&hookEvent={event}\"\n"
        )
    };

    // UserPromptSubmit → running
    write_frame(
        &mut c,
        &ClientRequest::Input {
            session: sref.clone(),
            data: curl("UserPromptSubmit", r#"{"session_id":"sess-1"}"#).into_bytes(),
        },
    )
    .await
    .unwrap();
    read_events_until(&mut c, SLOW_TIMEOUT, |evs| {
        evs.iter().any(|e| {
            matches!(e, ServerEvent::StatusChanged { agent, status: nebula_core::AgentStatus::Running, .. }
                if *agent == agent_id)
        })
    })
    .await;

    // Notification(permission_prompt) → needs_feedback
    write_frame(
        &mut c,
        &ClientRequest::Input {
            session: sref.clone(),
            data: curl(
                "Notification",
                r#"{"session_id":"sess-1","notification_type":"permission_prompt"}"#,
            )
            .into_bytes(),
        },
    )
    .await
    .unwrap();
    read_events_until(&mut c, SLOW_TIMEOUT, |evs| {
        evs.iter().any(|e| {
            matches!(e, ServerEvent::StatusChanged { agent, status: nebula_core::AgentStatus::NeedsFeedback, .. }
                if *agent == agent_id)
        })
    })
    .await;

    // A foreign session's Stop is ignored…
    write_frame(
        &mut c,
        &ClientRequest::Input {
            session: sref.clone(),
            data: curl("Stop", r#"{"session_id":"someone-elses-claude"}"#).into_bytes(),
        },
    )
    .await
    .unwrap();
    // …while the owning session's Stop finishes the agent.
    write_frame(
        &mut c,
        &ClientRequest::Input {
            session: sref.clone(),
            data: curl("Stop", r#"{"session_id":"sess-1"}"#).into_bytes(),
        },
    )
    .await
    .unwrap();
    let events = read_events_until(&mut c, SLOW_TIMEOUT, |evs| {
        evs.iter().any(|e| {
            matches!(e, ServerEvent::StatusChanged { agent, status: nebula_core::AgentStatus::Finished, .. }
                if *agent == agent_id)
        })
    })
    .await;
    // The foreign Stop must not have produced its own StatusChanged→Finished
    // before the NeedsFeedback→Finished one (i.e. exactly one Finished).
    let finished_count = events
        .iter()
        .filter(|e| {
            matches!(
                e,
                ServerEvent::StatusChanged {
                    status: nebula_core::AgentStatus::Finished,
                    ..
                }
            )
        })
        .count();
    assert_eq!(finished_count, 1, "foreign-session Stop must be ignored");

    // Session id was captured for --resume.
    write_frame(&mut c, &ClientRequest::Shutdown).await.unwrap();
    wait_for_exit(&mut daemon);
    let mut daemon2 = env.spawn_daemon();
    let mut c2 = connect(&env.sock()).await;
    handshake(&mut c2).await;
    write_frame(&mut c2, &ClientRequest::Subscribe)
        .await
        .unwrap();
    let events = read_events_until(&mut c2, EVENT_TIMEOUT, |evs| {
        evs.iter()
            .any(|e| matches!(e, ServerEvent::Snapshot { .. }))
    })
    .await;
    let ServerEvent::Snapshot { agents, .. } = &events[0] else {
        panic!()
    };
    assert_eq!(
        agents[0].session_id.as_deref(),
        Some("sess-1"),
        "session id persisted"
    );
    write_frame(&mut c2, &ClientRequest::Shutdown)
        .await
        .unwrap();
    wait_for_exit(&mut daemon2);
}

/// cwd-based re-homing end to end: an agent created in the main checkout
/// posts a hook whose payload reports a cwd inside another worktree of the
/// same project (claude entered a worktree it created mid-conversation) —
/// the daemon re-homes the agent row there and broadcasts the upsert.
/// The cancel path. Claude Code fires NO hook when the user interrupts a
/// turn — `Stop` is documented not to run on a user interrupt, and the
/// `idle_prompt` notification that normally rescues a hookless turn end is
/// suppressed precisely because the user just pressed a key. What it does
/// still do is clear its OSC 9;4 progress bar, so nebula reads busy/idle
/// straight off the PTY. This drives the whole path — raw bytes out of the
/// child, through the pump's scanner, into the status machine — with no HTTP
/// hook involved at all.
#[tokio::test]
async fn pty_progress_sequence_drives_status_without_any_hook() {
    let env = TestEnv::new();
    let repo = env.make_repo();
    let mut daemon = env.spawn_daemon();

    let mut c = connect(&env.sock()).await;
    handshake(&mut c).await;
    let worktree = add_project_get_main_worktree(&mut c, &repo).await;
    let agent_id = create_agent_get_id(&mut c, &worktree.id, "cancelled", 2).await;

    let sref = SessionRef::Agent(agent_id.clone());
    write_frame(
        &mut c,
        &ClientRequest::Attach {
            session: sref.clone(),
            from_seq: None,
            cols: 120,
            rows: 30,
        },
    )
    .await
    .unwrap();

    // Have the shell in the agent PTY emit exactly what Claude Code emits.
    // The command text is echoed back by the tty, which is the point: only
    // the real escape bytes may move the status, never a mention of them.
    // `\ddd` octal, not `\e` — POSIX printf, so this works under dash too.
    let emit = |state: &str| format!("printf '\\033]9;4;{state};\\007'\n").into_bytes();

    // Turn starts: progress goes indeterminate → running.
    write_frame(
        &mut c,
        &ClientRequest::Input {
            session: sref.clone(),
            data: emit("3"),
        },
    )
    .await
    .unwrap();
    read_events_until(&mut c, SLOW_TIMEOUT, |evs| {
        evs.iter().any(|e| {
            matches!(e, ServerEvent::StatusChanged { agent, status: nebula_core::AgentStatus::Running, .. }
                if *agent == agent_id)
        })
    })
    .await;

    // User hits escape: the progress bar clears, and nothing else happens.
    write_frame(
        &mut c,
        &ClientRequest::Input {
            session: sref.clone(),
            data: emit("0"),
        },
    )
    .await
    .unwrap();
    read_events_until(&mut c, SLOW_TIMEOUT, |evs| {
        evs.iter().any(|e| {
            matches!(e, ServerEvent::StatusChanged { agent, status: nebula_core::AgentStatus::Finished, .. }
                if *agent == agent_id)
        })
    })
    .await;

    write_frame(&mut c, &ClientRequest::Shutdown).await.unwrap();
    wait_for_exit(&mut daemon);
}

#[tokio::test]
async fn hook_cwd_rehomes_agent_to_other_worktree() {
    let env = TestEnv::new();
    let repo = env.make_repo();
    let mut daemon = env.spawn_daemon();

    let mut c = connect(&env.sock()).await;
    handshake(&mut c).await;
    subscribe(&mut c).await;

    write_frame(
        &mut c,
        &ClientRequest::AddProject {
            req_id: 1,
            path: repo.clone(),
            name: None,
            create_missing: false,
        },
    )
    .await
    .unwrap();
    let events = read_events_until(&mut c, EVENT_TIMEOUT, |evs| {
        evs.iter().any(|e| {
            matches!(
                e,
                ServerEvent::EntityUpserted {
                    entity: Entity::Worktree(_)
                }
            )
        })
    })
    .await;
    let main_worktree = events
        .iter()
        .find_map(|e| match e {
            ServerEvent::EntityUpserted {
                entity: Entity::Worktree(w),
            } => Some(w.clone()),
            _ => None,
        })
        .unwrap();

    // A second worktree — the one the agent will "enter".
    write_frame(
        &mut c,
        &ClientRequest::CreateWorktree {
            req_id: 2,
            project: main_worktree.project_id.clone(),
            branch: "feat".into(),
            base: None,
        },
    )
    .await
    .unwrap();
    // The upsert broadcast and the Ack ride different channels — wait for
    // the upsert itself.
    let events = read_events_until(&mut c, SLOW_TIMEOUT, |evs| {
        evs.iter().any(|e| {
            matches!(e, ServerEvent::EntityUpserted { entity: Entity::Worktree(w) }
                if w.branch == "feat")
        })
    })
    .await;
    let feat_worktree = events
        .iter()
        .find_map(|e| match e {
            ServerEvent::EntityUpserted {
                entity: Entity::Worktree(w),
            } if w.branch == "feat" => Some(w.clone()),
            _ => None,
        })
        .expect("feat worktree upsert");

    // Agent lives in the main checkout.
    write_frame(
        &mut c,
        &ClientRequest::CreateAgent {
            req_id: 3,
            worktree: main_worktree.id.clone(),
            name: "mover".into(),
            kind: AgentKind::Claude,
            model: None,
            effort: None,
            auto_title: false,
            cloud_prompt: None,
        },
    )
    .await
    .unwrap();
    let events = read_events_until(&mut c, EVENT_TIMEOUT, |evs| find_ack(evs, 3).is_some()).await;
    let ServerEvent::Ack {
        created: Some(EntityId::Agent(agent_id)),
        ..
    } = find_ack(&events, 3).unwrap()
    else {
        panic!("CreateAgent failed: {events:#?}");
    };
    let agent_id = agent_id.clone();

    // POST a hook from inside the agent PTY whose payload reports the feat
    // worktree as cwd — exactly what claude sends after entering it.
    let sref = SessionRef::Agent(agent_id.clone());
    write_frame(
        &mut c,
        &ClientRequest::Attach {
            session: sref.clone(),
            from_seq: None,
            cols: 120,
            rows: 30,
        },
    )
    .await
    .unwrap();
    let body = format!(
        r#"{{"session_id":"sess-1","cwd":"{}"}}"#,
        feat_worktree.path.display()
    );
    let curl = format!(
        "curl -sS -m 3 -X POST -H \"Authorization: Bearer $NEBULA_API_TOKEN\" \
         -H 'Content-Type: application/json' -d '{body}' \
         \"$NEBULA_API_URL/api/hooks/claude?agentId=$NEBULA_AGENT_ID&hookEvent=UserPromptSubmit\"\n"
    );
    write_frame(
        &mut c,
        &ClientRequest::Input {
            session: sref.clone(),
            data: curl.into_bytes(),
        },
    )
    .await
    .unwrap();

    let events = read_events_until(&mut c, SLOW_TIMEOUT, |evs| {
        evs.iter().any(|e| {
            matches!(e, ServerEvent::EntityUpserted { entity: Entity::Agent(a) }
                if a.id == agent_id && a.worktree_id == feat_worktree.id)
        })
    })
    .await;
    assert!(
        events.iter().any(|e| {
            matches!(e, ServerEvent::EntityUpserted { entity: Entity::Agent(a) }
                if a.id == agent_id && a.worktree_id == feat_worktree.id)
        }),
        "agent re-homed to the worktree its hook cwd reported: {events:#?}"
    );

    write_frame(&mut c, &ClientRequest::Shutdown).await.unwrap();
    wait_for_exit(&mut daemon);
}

/// The inverse of the cwd-rehome test: a *user* move of a live agent must
/// relocate the process too. The PTY is killed and respawned in the target
/// checkout — left running in the old one, its hooks would keep reporting
/// the old cwd and the daemon would snap the row right back.
#[tokio::test]
async fn move_agent_respawns_live_session_in_target_worktree() {
    let env = TestEnv::new();
    let repo = env.make_repo();
    let mut daemon = env.spawn_daemon();

    let mut c = connect(&env.sock()).await;
    handshake(&mut c).await;
    subscribe(&mut c).await;

    write_frame(
        &mut c,
        &ClientRequest::AddProject {
            req_id: 1,
            path: repo.clone(),
            name: None,
            create_missing: false,
        },
    )
    .await
    .unwrap();
    let events = read_events_until(&mut c, EVENT_TIMEOUT, |evs| {
        evs.iter().any(|e| {
            matches!(
                e,
                ServerEvent::EntityUpserted {
                    entity: Entity::Worktree(_)
                }
            )
        })
    })
    .await;
    let main_worktree = events
        .iter()
        .find_map(|e| match e {
            ServerEvent::EntityUpserted {
                entity: Entity::Worktree(w),
            } => Some(w.clone()),
            _ => None,
        })
        .unwrap();

    write_frame(
        &mut c,
        &ClientRequest::CreateWorktree {
            req_id: 2,
            project: main_worktree.project_id.clone(),
            branch: "feat".into(),
            base: None,
        },
    )
    .await
    .unwrap();
    let events = read_events_until(&mut c, SLOW_TIMEOUT, |evs| {
        evs.iter().any(|e| {
            matches!(e, ServerEvent::EntityUpserted { entity: Entity::Worktree(w) }
                if w.branch == "feat")
        })
    })
    .await;
    let feat_worktree = events
        .iter()
        .find_map(|e| match e {
            ServerEvent::EntityUpserted {
                entity: Entity::Worktree(w),
            } if w.branch == "feat" => Some(w.clone()),
            _ => None,
        })
        .expect("feat worktree upsert");

    write_frame(
        &mut c,
        &ClientRequest::CreateAgent {
            req_id: 3,
            worktree: main_worktree.id.clone(),
            name: "mover".into(),
            kind: AgentKind::Claude,
            model: None,
            effort: None,
            auto_title: false,
            cloud_prompt: None,
        },
    )
    .await
    .unwrap();
    let events = read_events_until(&mut c, EVENT_TIMEOUT, |evs| find_ack(evs, 3).is_some()).await;
    let ServerEvent::Ack {
        created: Some(EntityId::Agent(agent_id)),
        ..
    } = find_ack(&events, 3).unwrap()
    else {
        panic!("CreateAgent failed: {events:#?}");
    };
    let agent_id = agent_id.clone();
    let sref = SessionRef::Agent(agent_id.clone());

    // Sanity: the stand-in sh really runs in the main checkout ("…/repo",
    // never "repo-worktrees"). Suffix match sidesteps macOS /private
    // symlink canonicalization in pwd's output.
    write_frame(
        &mut c,
        &ClientRequest::Attach {
            session: sref.clone(),
            from_seq: None,
            cols: 120,
            rows: 30,
        },
    )
    .await
    .unwrap();
    write_frame(
        &mut c,
        &ClientRequest::Input {
            session: sref.clone(),
            data: b"pwd\n".to_vec(),
        },
    )
    .await
    .unwrap();
    read_events_until(&mut c, SLOW_TIMEOUT, |evs| {
        String::from_utf8_lossy(&collected_output(evs)).contains("/repo\r")
    })
    .await;

    // The move: row re-homes AND the PTY comes back alive in the target.
    write_frame(
        &mut c,
        &ClientRequest::MoveAgent {
            req_id: 4,
            id: agent_id.clone(),
            worktree: feat_worktree.id.clone(),
        },
    )
    .await
    .unwrap();
    let events = read_events_until(&mut c, SLOW_TIMEOUT, |evs| {
        evs.iter().any(|e| {
            matches!(e, ServerEvent::EntityUpserted { entity: Entity::Agent(a) }
                if a.id == agent_id && a.worktree_id == feat_worktree.id && a.alive)
        })
    })
    .await;
    assert!(
        events.iter().any(|e| {
            matches!(e, ServerEvent::EntityUpserted { entity: Entity::Agent(a) }
                if a.id == agent_id && a.worktree_id == feat_worktree.id && a.alive)
        }),
        "moved agent re-homed and respawned alive: {events:#?}"
    );

    // The respawned process must actually sit in the feat checkout — that
    // is what keeps its hook cwds from re-homing the row back.
    write_frame(
        &mut c,
        &ClientRequest::Attach {
            session: sref.clone(),
            from_seq: None,
            cols: 120,
            rows: 30,
        },
    )
    .await
    .unwrap();
    write_frame(
        &mut c,
        &ClientRequest::Input {
            session: sref.clone(),
            data: b"pwd\n".to_vec(),
        },
    )
    .await
    .unwrap();
    read_events_until(&mut c, SLOW_TIMEOUT, |evs| {
        String::from_utf8_lossy(&collected_output(evs)).contains("repo-worktrees/feat")
    })
    .await;

    write_frame(&mut c, &ClientRequest::Shutdown).await.unwrap();
    wait_for_exit(&mut daemon);
}

/// Codex mirror of the claude hook test: a codex-kind agent gets its hooks
/// installed into codex's home (not the worktree — one trust approval has
/// to cover every worktree), and posts to `/api/hooks/codex` drive the same
/// status machine (PermissionRequest is codex's native waiting signal).
#[tokio::test]
async fn codex_hooks_install_and_drive_status() {
    let env = TestEnv::new();
    let repo = env.make_repo();
    let codex_home = env.tmp.path().join("codex-home");
    // A worktree copy from an older nebula, alongside a foreign managed
    // group: the spawn must prune ours and leave theirs alone.
    let stale = repo.join(".codex");
    std::fs::create_dir_all(&stale).unwrap();
    std::fs::write(
        stale.join("hooks.json"),
        r#"{"hooks":{"Stop":[
            {"_nebulaManaged":true,"hooks":[{"type":"command",
              "command":"curl $NEBULA_API_URL/api/hooks/codex?agentId=$NEBULA_AGENT_ID"}]},
            {"_mcManaged":true,"hooks":[{"type":"command","command":"curl $MC_API_URL/x"}]}]}}"#,
    )
    .unwrap();
    let mut daemon =
        env.spawn_daemon_with("/bin/sh", &[("CODEX_HOME", codex_home.to_str().unwrap())]);

    let mut c = connect(&env.sock()).await;
    handshake(&mut c).await;
    subscribe(&mut c).await;

    write_frame(
        &mut c,
        &ClientRequest::AddProject {
            req_id: 1,
            path: repo.clone(),
            name: None,
            create_missing: false,
        },
    )
    .await
    .unwrap();
    let events = read_events_until(&mut c, EVENT_TIMEOUT, |evs| {
        evs.iter().any(|e| {
            matches!(
                e,
                ServerEvent::EntityUpserted {
                    entity: Entity::Worktree(_)
                }
            )
        })
    })
    .await;
    let worktree = events
        .iter()
        .find_map(|e| match e {
            ServerEvent::EntityUpserted {
                entity: Entity::Worktree(w),
            } => Some(w.clone()),
            _ => None,
        })
        .unwrap();

    write_frame(
        &mut c,
        &ClientRequest::CreateAgent {
            req_id: 2,
            worktree: worktree.id.clone(),
            name: "codexed".into(),
            kind: AgentKind::Codex,
            model: None,
            effort: None,
            auto_title: false,
            cloud_prompt: None,
        },
    )
    .await
    .unwrap();
    let events = read_events_until(&mut c, EVENT_TIMEOUT, |evs| find_ack(evs, 2).is_some()).await;
    let ServerEvent::Ack {
        created: Some(EntityId::Agent(agent_id)),
        ..
    } = find_ack(&events, 2).unwrap()
    else {
        panic!("CreateAgent failed: {events:#?}");
    };
    let agent_id = agent_id.clone();

    // Codex hooks were installed into codex's home — and only codex-shaped
    // ones (no claude-specific Notification/AskUserQuestion groups).
    let hooks_path = codex_home.join("hooks.json");
    assert!(hooks_path.exists(), "codex hooks installed into codex home");
    let hooks: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&hooks_path).unwrap()).unwrap();
    assert!(hooks["hooks"]["Stop"][0]["_nebulaManaged"]
        .as_bool()
        .unwrap());
    assert!(hooks["hooks"]["Stop"][0]["hooks"][0]["command"]
        .as_str()
        .unwrap()
        .contains("/api/hooks/codex?"));
    assert!(hooks["hooks"].get("Notification").is_none());
    assert!(hooks["hooks"].get("PreToolUse").is_none());

    // The stale worktree copy lost our group and kept the foreign one.
    let stale_hooks: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(stale.join("hooks.json")).unwrap()).unwrap();
    let stop = stale_hooks["hooks"]["Stop"].as_array().unwrap();
    assert_eq!(stop.len(), 1, "only the foreign group survives: {stop:#?}");
    assert!(stop[0]["_mcManaged"].as_bool().unwrap());

    // Drive the shell inside the agent PTY to POST codex hooks with its env.
    let sref = SessionRef::Agent(agent_id.clone());
    write_frame(
        &mut c,
        &ClientRequest::Attach {
            session: sref.clone(),
            from_seq: None,
            cols: 120,
            rows: 30,
        },
    )
    .await
    .unwrap();
    let curl = |event: &str, body: &str| {
        format!(
            "curl -sS -m 3 -X POST -H \"Authorization: Bearer $NEBULA_API_TOKEN\" \
             -H 'Content-Type: application/json' -d '{body}' \
             \"$NEBULA_API_URL/api/hooks/codex?agentId=$NEBULA_AGENT_ID&hookEvent={event}\"\n"
        )
    };

    // UserPromptSubmit → running
    write_frame(
        &mut c,
        &ClientRequest::Input {
            session: sref.clone(),
            data: curl("UserPromptSubmit", r#"{"session_id":"codex-sess-1"}"#).into_bytes(),
        },
    )
    .await
    .unwrap();
    read_events_until(&mut c, SLOW_TIMEOUT, |evs| {
        evs.iter().any(|e| {
            matches!(e, ServerEvent::StatusChanged { agent, status: nebula_core::AgentStatus::Running, .. }
                if *agent == agent_id)
        })
    })
    .await;

    // PermissionRequest (codex's native waiting signal) → needs_feedback
    write_frame(
        &mut c,
        &ClientRequest::Input {
            session: sref.clone(),
            data: curl("PermissionRequest", r#"{"session_id":"codex-sess-1"}"#).into_bytes(),
        },
    )
    .await
    .unwrap();
    read_events_until(&mut c, SLOW_TIMEOUT, |evs| {
        evs.iter().any(|e| {
            matches!(e, ServerEvent::StatusChanged { agent, status: nebula_core::AgentStatus::NeedsFeedback, .. }
                if *agent == agent_id)
        })
    })
    .await;

    // Stop → finished
    write_frame(
        &mut c,
        &ClientRequest::Input {
            session: sref.clone(),
            data: curl("Stop", r#"{"session_id":"codex-sess-1"}"#).into_bytes(),
        },
    )
    .await
    .unwrap();
    read_events_until(&mut c, SLOW_TIMEOUT, |evs| {
        evs.iter().any(|e| {
            matches!(e, ServerEvent::StatusChanged { agent, status: nebula_core::AgentStatus::Finished, .. }
                if *agent == agent_id)
        })
    })
    .await;

    // Kind and session id survive a daemon restart (feeds `codex resume`).
    write_frame(&mut c, &ClientRequest::Shutdown).await.unwrap();
    wait_for_exit(&mut daemon);
    let mut daemon2 = env.spawn_daemon();
    let mut c2 = connect(&env.sock()).await;
    handshake(&mut c2).await;
    write_frame(&mut c2, &ClientRequest::Subscribe)
        .await
        .unwrap();
    let events = read_events_until(&mut c2, EVENT_TIMEOUT, |evs| {
        evs.iter()
            .any(|e| matches!(e, ServerEvent::Snapshot { .. }))
    })
    .await;
    let ServerEvent::Snapshot { agents, .. } = &events[0] else {
        panic!()
    };
    assert_eq!(agents[0].kind, AgentKind::Codex, "kind persisted");
    assert_eq!(
        agents[0].session_id.as_deref(),
        Some("codex-sess-1"),
        "codex session id persisted"
    );
    write_frame(&mut c2, &ClientRequest::Shutdown)
        .await
        .unwrap();
    wait_for_exit(&mut daemon2);
}

#[tokio::test]
async fn external_worktrees_are_adopted_and_dropped() {
    let env = TestEnv::new();
    let repo = env.make_repo();
    let mut daemon = env.spawn_daemon();

    let mut c = connect(&env.sock()).await;
    handshake(&mut c).await;
    write_frame(&mut c, &ClientRequest::Subscribe)
        .await
        .unwrap();
    write_frame(
        &mut c,
        &ClientRequest::AddProject {
            req_id: 1,
            path: repo.clone(),
            name: None,
            create_missing: false,
        },
    )
    .await
    .unwrap();
    let events = read_events_until(&mut c, EVENT_TIMEOUT, |evs| find_ack(evs, 1).is_some()).await;
    assert!(
        matches!(find_ack(&events, 1), Some(ServerEvent::Ack { .. })),
        "AddProject failed: {events:#?}"
    );

    // A worktree created behind nebula's back — exactly what an agent (or a
    // human in another shell) does.
    let wt_path = env.tmp.path().join("repo-worktrees").join("agent-branch");
    let git_worktree = |args: &[&str]| {
        std::process::Command::new("git")
            .arg("-C")
            .arg(&repo)
            .arg("worktree")
            .args(args)
            .arg(&wt_path)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .unwrap()
            .success()
    };
    assert!(
        git_worktree(&["add", "-b", "agent-branch"]),
        "external git worktree add failed"
    );

    // The auto-sync adopts it without any client request.
    let events = read_events_until(&mut c, EVENT_TIMEOUT, |evs| {
        evs.iter().any(|e| matches!(
            e,
            ServerEvent::EntityUpserted { entity: Entity::Worktree(w) } if w.branch == "agent-branch"
        ))
    })
    .await;
    let adopted = events
        .iter()
        .find_map(|e| match e {
            ServerEvent::EntityUpserted {
                entity: Entity::Worktree(w),
            } if w.branch == "agent-branch" => Some(w.clone()),
            _ => None,
        })
        .unwrap();
    assert!(!adopted.is_main, "adopted checkout is not the main row");
    assert!(
        adopted.path.exists(),
        "adopted row points at the real checkout"
    );

    // Removing it externally drops the row too (nothing lives there).
    assert!(
        git_worktree(&["remove", "--force"]),
        "external git worktree remove failed"
    );
    read_events_until(&mut c, EVENT_TIMEOUT, |evs| {
        evs.iter().any(|e| {
            matches!(
                e,
                ServerEvent::EntityRemoved { id: EntityId::Worktree(id) } if *id == adopted.id
            )
        })
    })
    .await;

    // Switching branches on the root checkout renames the main row in place
    // (the probe watches .git/HEAD, not just the worktrees registry).
    assert!(
        std::process::Command::new("git")
            .arg("-C")
            .arg(&repo)
            .args(["checkout", "-b", "renamed-root"])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .unwrap()
            .success(),
        "git checkout -b renamed-root failed"
    );
    let events = read_events_until(&mut c, EVENT_TIMEOUT, |evs| {
        evs.iter().any(|e| {
            matches!(
                e,
                ServerEvent::EntityUpserted { entity: Entity::Worktree(w) }
                    if w.is_main && w.branch == "renamed-root"
            )
        })
    })
    .await;
    assert!(
        events.iter().any(|e| matches!(
            e,
            ServerEvent::EntityUpserted { entity: Entity::Worktree(w) }
                if w.is_main && w.branch == "renamed-root"
        )),
        "main row should refresh to the new branch: {events:#?}"
    );

    write_frame(&mut c, &ClientRequest::Shutdown).await.unwrap();
    wait_for_exit(&mut daemon);
}

/// `nebula upgrade` daemon handoff: with a live session the old daemon is
/// left running (restart is the user's call); once idle, the upgrade shuts
/// it down so the next launch spawns the new binary.
#[tokio::test]
async fn upgrade_shuts_down_idle_daemon_but_spares_live_sessions() {
    let env = TestEnv::new();
    let repo = env.make_repo();
    let mut daemon = env.spawn_daemon();

    let mut c = connect(&env.sock()).await;
    handshake(&mut c).await;
    write_frame(&mut c, &ClientRequest::Subscribe)
        .await
        .unwrap();
    write_frame(
        &mut c,
        &ClientRequest::AddProject {
            req_id: 1,
            path: repo.clone(),
            name: None,
            create_missing: false,
        },
    )
    .await
    .unwrap();
    let events = read_events_until(&mut c, EVENT_TIMEOUT, |evs| {
        find_ack(evs, 1).is_some()
            && evs.iter().any(|e| {
                matches!(
                    e,
                    ServerEvent::EntityUpserted {
                        entity: Entity::Worktree(_)
                    }
                )
            })
    })
    .await;
    let main_worktree = events
        .iter()
        .find_map(|e| match e {
            ServerEvent::EntityUpserted {
                entity: Entity::Worktree(w),
            } if w.is_main => Some(w.clone()),
            _ => None,
        })
        .expect("main worktree upsert");

    write_frame(
        &mut c,
        &ClientRequest::CreateTerminal {
            req_id: 2,
            worktree: main_worktree.id.clone(),
            name: None,
        },
    )
    .await
    .unwrap();
    let events = read_events_until(&mut c, EVENT_TIMEOUT, |evs| find_ack(evs, 2).is_some()).await;
    let ServerEvent::Ack {
        created: Some(EntityId::Terminal(term_id)),
        ..
    } = find_ack(&events, 2).unwrap()
    else {
        panic!("CreateTerminal failed: {events:#?}");
    };
    let term_id = term_id.clone();

    // Stub installer: the upgrade command runs it, then handles the daemon.
    let script = env.tmp.path().join("stub-install.sh");
    std::fs::write(&script, "#!/bin/sh\nexit 0\n").unwrap();
    let run_upgrade = || {
        env.cli()
            .args(["upgrade", "--force"])
            .env(env::INSTALL_URL, format!("file://{}", script.display()))
            .output()
            .unwrap()
    };

    // A live terminal PTY keeps the daemon alive through the upgrade.
    let out = run_upgrade();
    assert!(
        out.status.success(),
        "upgrade failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("1 live session"),
        "expected live-session note, got: {stdout}"
    );
    assert!(
        daemon.try_wait().unwrap().is_none(),
        "daemon must keep running while sessions are live"
    );

    // Exit the shell; the daemon marks the terminal dead and is now idle.
    let sref = SessionRef::Terminal(term_id.clone());
    write_frame(
        &mut c,
        &ClientRequest::Attach {
            session: sref.clone(),
            from_seq: None,
            cols: 80,
            rows: 24,
        },
    )
    .await
    .unwrap();
    write_frame(
        &mut c,
        &ClientRequest::Input {
            session: sref,
            data: b"exit\n".to_vec(),
        },
    )
    .await
    .unwrap();
    read_events_until(&mut c, EVENT_TIMEOUT, |evs| {
        evs.iter().any(|e| {
            matches!(
                e,
                ServerEvent::EntityUpserted { entity: Entity::Terminal(t) }
                    if t.id == term_id && !t.alive
            )
        })
    })
    .await;

    let out = run_upgrade();
    assert!(
        out.status.success(),
        "idle upgrade failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("no live sessions"),
        "expected idle-shutdown note, got: {stdout}"
    );
    wait_for_exit(&mut daemon);
}

/// AddProject with `create_missing` makes the directory and `git init`s it;
/// with `git_init_on_create: false` in config.json the directory is still
/// created but adding fails (not a git repository).
#[tokio::test]
async fn add_project_creates_missing_dir_and_inits() {
    let env = TestEnv::new();
    let mut daemon = env.spawn_daemon();

    let mut c = connect(&env.sock()).await;
    handshake(&mut c).await;

    let new_dir = env.tmp.path().join("brand-new-project");
    assert!(!new_dir.exists());
    write_frame(
        &mut c,
        &ClientRequest::AddProject {
            req_id: 1,
            path: new_dir.clone(),
            name: None,
            create_missing: true,
        },
    )
    .await
    .unwrap();
    let events = read_events_until(&mut c, EVENT_TIMEOUT, |evs| find_ack(evs, 1).is_some()).await;
    assert!(
        matches!(
            find_ack(&events, 1),
            Some(ServerEvent::Ack {
                created: Some(EntityId::Project(_)),
                ..
            })
        ),
        "AddProject with create_missing failed: {events:#?}"
    );
    assert!(new_dir.join(".git").is_dir(), "git init ran in the new dir");

    // Opt out of git init via config: the dir is created, the add errors.
    std::fs::write(
        env.tmp.path().join("data").join("config.json"),
        r#"{"git_init_on_create": false}"#,
    )
    .unwrap();
    let bare_dir = env.tmp.path().join("bare-new-project");
    write_frame(
        &mut c,
        &ClientRequest::AddProject {
            req_id: 2,
            path: bare_dir.clone(),
            name: None,
            create_missing: true,
        },
    )
    .await
    .unwrap();
    let events = read_events_until(&mut c, EVENT_TIMEOUT, |evs| find_ack(evs, 2).is_some()).await;
    assert!(
        matches!(find_ack(&events, 2), Some(ServerEvent::Error { .. })),
        "expected not-a-git-repo error: {events:#?}"
    );
    assert!(bare_dir.is_dir(), "dir created even without git init");
    assert!(!bare_dir.join(".git").exists(), "git init skipped");

    write_frame(&mut c, &ClientRequest::Shutdown).await.unwrap();
    wait_for_exit(&mut daemon);
}

/// Subscribe + AddProject boilerplate; returns the main worktree row.
async fn add_project_get_main_worktree(c: &mut UnixStream, repo: &Path) -> nebula_core::Worktree {
    write_frame(c, &ClientRequest::Subscribe).await.unwrap();
    read_events_until(c, EVENT_TIMEOUT, |evs| {
        evs.iter()
            .any(|e| matches!(e, ServerEvent::Snapshot { .. }))
    })
    .await;
    write_frame(
        c,
        &ClientRequest::AddProject {
            req_id: 1,
            path: repo.to_path_buf(),
            name: None,
            create_missing: false,
        },
    )
    .await
    .unwrap();
    let events = read_events_until(c, EVENT_TIMEOUT, |evs| {
        find_ack(evs, 1).is_some()
            && evs.iter().any(|e| {
                matches!(
                    e,
                    ServerEvent::EntityUpserted {
                        entity: Entity::Worktree(w)
                    } if w.is_main
                )
            })
    })
    .await;
    events
        .iter()
        .find_map(|e| match e {
            ServerEvent::EntityUpserted {
                entity: Entity::Worktree(w),
            } if w.is_main => Some(w.clone()),
            _ => None,
        })
        .expect("main worktree upsert")
}

/// PrewarmAgent boots the CLI while the user is "typing the name"; the
/// following CreateAgent must adopt that already-running PTY (its slow boot
/// output is already in scrollback) and replay the hooks it fired before the
/// row existed (SessionStart → stored resume session id).
#[tokio::test]
async fn prewarmed_session_is_adopted_by_create_agent() {
    let env = TestEnv::new();
    let repo = env.make_repo();
    // Stand-in agent CLI with a deliberately slow boot: posts SessionStart
    // (like claude does), sleeps, prints a marker, then becomes a shell. If
    // adoption works, the marker is in scrollback the moment we attach; a
    // cold spawn at CreateAgent time couldn't print it for another 3s.
    let script = env.tmp.path().join("slow-agent.sh");
    std::fs::write(
        &script,
        concat!(
            "#!/bin/sh\n",
            "curl -sS -m 3 -X POST -H \"Authorization: Bearer $NEBULA_API_TOKEN\" \\\n",
            "  -H 'Content-Type: application/json' -d '{\"session_id\":\"warm-sid-99\"}' \\\n",
            "  \"$NEBULA_API_URL/api/hooks/claude?agentId=$NEBULA_AGENT_ID&hookEvent=SessionStart\" \\\n",
            "  >/dev/null 2>&1\n",
            "sleep 3\n",
            "echo PREWARM_READY\n",
            "exec /bin/sh\n",
        ),
    )
    .unwrap();
    make_executable(&script);
    let mut daemon = env.spawn_daemon_with_agent_cmd(script.to_str().unwrap());

    let mut c = connect(&env.sock()).await;
    handshake(&mut c).await;
    let worktree = add_project_get_main_worktree(&mut c, &repo).await;

    // Kind picked — warm the CLI. No reply expected.
    write_frame(
        &mut c,
        &ClientRequest::PrewarmAgent {
            worktree: worktree.id.clone(),
            kind: AgentKind::Claude,
            model: None,
            effort: None,
        },
    )
    .await
    .unwrap();
    // "User types the name": long enough for the warm boot to finish.
    tokio::time::sleep(Duration::from_millis(4500)).await;

    write_frame(
        &mut c,
        &ClientRequest::CreateAgent {
            req_id: 2,
            worktree: worktree.id.clone(),
            name: "warm-agent".into(),
            kind: AgentKind::Claude,
            model: None,
            effort: None,
            auto_title: false,
            cloud_prompt: None,
        },
    )
    .await
    .unwrap();
    let events = read_events_until(&mut c, EVENT_TIMEOUT, |evs| find_ack(evs, 2).is_some()).await;
    let ServerEvent::Ack {
        created: Some(EntityId::Agent(agent_id)),
        ..
    } = find_ack(&events, 2).unwrap()
    else {
        panic!("CreateAgent failed: {events:#?}");
    };
    let agent_id = agent_id.clone();

    // Attach: the boot marker must already be there (window far shorter
    // than the script's 3s boot, so a cold spawn cannot pass).
    let sref = SessionRef::Agent(agent_id.clone());
    write_frame(
        &mut c,
        &ClientRequest::Attach {
            session: sref.clone(),
            from_seq: None,
            cols: 100,
            rows: 30,
        },
    )
    .await
    .unwrap();
    let events = read_events_until(&mut c, Duration::from_secs(2), |evs| {
        String::from_utf8_lossy(&collected_output(evs)).contains("PREWARM_READY")
    })
    .await;
    drop(events);

    // The adopted PTY is interactive.
    let marker = "adopted_marker_7731";
    write_frame(
        &mut c,
        &ClientRequest::Input {
            session: sref.clone(),
            data: format!("echo {marker}\n").into_bytes(),
        },
    )
    .await
    .unwrap();
    read_events_until(&mut c, EVENT_TIMEOUT, |evs| {
        String::from_utf8_lossy(&collected_output(evs))
            .matches(marker)
            .count()
            >= 2
    })
    .await;

    // The SessionStart the warm CLI posted before the row existed was
    // buffered and replayed: the agent row carries the resume session id.
    let mut c2 = connect(&env.sock()).await;
    handshake(&mut c2).await;
    write_frame(&mut c2, &ClientRequest::Subscribe)
        .await
        .unwrap();
    let events = read_events_until(&mut c2, EVENT_TIMEOUT, |evs| {
        evs.iter()
            .any(|e| matches!(e, ServerEvent::Snapshot { .. }))
    })
    .await;
    let ServerEvent::Snapshot { agents, .. } = &events[0] else {
        panic!("expected snapshot");
    };
    let agent = agents.iter().find(|a| a.id == agent_id).expect("agent row");
    assert_eq!(agent.name, "warm-agent");
    assert!(agent.alive, "adopted session is live");
    assert_eq!(
        agent.session_id.as_deref(),
        Some("warm-sid-99"),
        "buffered SessionStart replayed at adoption"
    );

    write_frame(&mut c, &ClientRequest::Shutdown).await.unwrap();
    wait_for_exit(&mut daemon);
}

/// A prewarmed CLI that dies immediately (the "claude/codex not installed"
/// shape) must not poison creation: CreateAgent quietly falls back to a
/// fresh spawn and still succeeds.
#[tokio::test]
async fn dead_prewarm_falls_back_to_cold_spawn() {
    let env = TestEnv::new();
    let repo = env.make_repo();
    let script = env.tmp.path().join("dying-agent.sh");
    std::fs::write(&script, "#!/bin/sh\nexit 127\n").unwrap();
    make_executable(&script);
    let mut daemon = env.spawn_daemon_with_agent_cmd(script.to_str().unwrap());

    let mut c = connect(&env.sock()).await;
    handshake(&mut c).await;
    let worktree = add_project_get_main_worktree(&mut c, &repo).await;

    write_frame(
        &mut c,
        &ClientRequest::PrewarmAgent {
            worktree: worktree.id.clone(),
            kind: AgentKind::Claude,
            model: None,
            effort: None,
        },
    )
    .await
    .unwrap();
    // Give the warm spawn time to die.
    tokio::time::sleep(Duration::from_millis(1000)).await;

    write_frame(
        &mut c,
        &ClientRequest::CreateAgent {
            req_id: 2,
            worktree: worktree.id.clone(),
            name: "fallback-agent".into(),
            kind: AgentKind::Claude,
            model: None,
            effort: None,
            auto_title: false,
            cloud_prompt: None,
        },
    )
    .await
    .unwrap();
    let events = read_events_until(&mut c, EVENT_TIMEOUT, |evs| find_ack(evs, 2).is_some()).await;
    assert!(
        matches!(
            find_ack(&events, 2),
            Some(ServerEvent::Ack {
                created: Some(EntityId::Agent(_)),
                ..
            })
        ),
        "CreateAgent must survive a dead prewarm: {events:#?}"
    );

    write_frame(&mut c, &ClientRequest::Shutdown).await.unwrap();
    wait_for_exit(&mut daemon);
}

/// `kill -0` liveness probe (also true for an unreaped zombie).
/// An agent CLI that isn't installed must be refused up front. Before this
/// check the create "succeeded": the login shell printed `command not found`
/// into a PTY that died at once, leaving a dead session row indistinguishable
/// from a fresh one.
#[tokio::test]
async fn create_agent_refuses_when_the_cli_is_not_installed() {
    let env = TestEnv::new();
    let repo = env.make_repo();
    let shell = env.blind_shell();
    let mut daemon = env.spawn_daemon_with_shell(&shell);

    let mut c = connect(&env.sock()).await;
    handshake(&mut c).await;
    let worktree = add_project_get_main_worktree(&mut c, &repo).await;

    for (req_id, kind, want) in [
        (10u64, AgentKind::Claude, "claude"),
        (11, AgentKind::Codex, "codex"),
        // Cursor's binary is `cursor-agent`; the message must name that, not
        // the kind, or the user goes looking for the wrong thing to install.
        (12, AgentKind::Cursor, "cursor-agent"),
    ] {
        write_frame(
            &mut c,
            &ClientRequest::CreateAgent {
                req_id,
                worktree: worktree.id.clone(),
                name: format!("agent-{req_id}"),
                kind,
                model: None,
                effort: None,
                auto_title: false,
                cloud_prompt: None,
            },
        )
        .await
        .unwrap();
        let events = read_events_until(&mut c, SPAWN_CHAIN_TIMEOUT, |evs| {
            evs.iter().any(|e| {
                matches!(e, ServerEvent::Error { req_id: Some(r), .. } if *r == req_id)
                    || matches!(e, ServerEvent::Ack { req_id: r, .. } if *r == req_id)
            })
        })
        .await;
        let message = events
            .iter()
            .find_map(|e| match e {
                ServerEvent::Error {
                    req_id: Some(r),
                    message,
                } if *r == req_id => Some(message.clone()),
                _ => None,
            })
            .unwrap_or_else(|| panic!("{kind:?} create must be refused, got: {events:#?}"));
        assert!(
            message.starts_with(&format!("{want} was not found on your PATH")),
            "{kind:?}: {message}"
        );
    }

    // And no half-created rows left behind in the Sessions column.
    let events = subscribe(&mut c).await;
    let agents = events
        .iter()
        .find_map(|e| match e {
            ServerEvent::Snapshot { agents, .. } => Some(agents.clone()),
            _ => None,
        })
        .expect("snapshot");
    assert!(agents.is_empty(), "refused creates left rows: {agents:#?}");

    write_frame(&mut c, &ClientRequest::Shutdown).await.unwrap();
    wait_for_exit(&mut daemon);
}

/// The mirror of the refusal: when the CLI *is* on the login shell's PATH the
/// check must stay out of the way — guards against the probe being so strict
/// (or so slow) that it blocks legitimate creates.
#[tokio::test]
async fn create_agent_succeeds_when_the_cli_is_on_the_login_shell_path() {
    let env = TestEnv::new();
    let repo = env.make_repo();

    // A stub `claude` that just sits there, reachable only via this shell.
    let bin = env.tmp.path().join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let stub = bin.join("claude");
    std::fs::write(&stub, "#!/bin/sh\nsleep 60\n").unwrap();
    make_executable(&stub);

    let shell = env.tmp.path().join("seeing-shell.sh");
    std::fs::write(
        &shell,
        format!(
            "#!/bin/sh\nPATH={}:/usr/bin:/bin\nexport PATH\nexec /bin/sh -c \"$4\"\n",
            bin.display()
        ),
    )
    .unwrap();
    make_executable(&shell);

    let mut daemon = env.spawn_daemon_with_shell(&shell);
    let mut c = connect(&env.sock()).await;
    handshake(&mut c).await;
    let worktree = add_project_get_main_worktree(&mut c, &repo).await;

    write_frame(
        &mut c,
        &ClientRequest::CreateAgent {
            req_id: 7,
            worktree: worktree.id.clone(),
            name: "real-agent".into(),
            kind: AgentKind::Claude,
            model: None,
            effort: None,
            auto_title: false,
            cloud_prompt: None,
        },
    )
    .await
    .unwrap();
    let events = read_events_until(&mut c, SPAWN_CHAIN_TIMEOUT, |evs| {
        find_ack(evs, 7).is_some()
            || evs.iter().any(|e| {
                matches!(
                    e,
                    ServerEvent::Error {
                        req_id: Some(7),
                        ..
                    }
                )
            })
    })
    .await;
    assert!(
        matches!(
            find_ack(&events, 7),
            Some(ServerEvent::Ack {
                created: Some(EntityId::Agent(_)),
                ..
            })
        ),
        "an installed CLI must still create: {events:#?}"
    );

    write_frame(&mut c, &ClientRequest::Shutdown).await.unwrap();
    wait_for_exit(&mut daemon);
}

fn pid_alive(pid: i32) -> bool {
    std::process::Command::new("kill")
        .args(["-0", &pid.to_string()])
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

async fn wait_pid_dead(pid: i32, timeout: Duration, what: &str) {
    let deadline = tokio::time::Instant::now() + timeout;
    while pid_alive(pid) {
        assert!(
            tokio::time::Instant::now() < deadline,
            "{what} (pid {pid}) still running after {timeout:?}"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

async fn create_agent_get_id(
    c: &mut UnixStream,
    worktree: &nebula_core::WorktreeId,
    name: &str,
    req_id: u64,
) -> nebula_core::AgentId {
    write_frame(
        c,
        &ClientRequest::CreateAgent {
            req_id,
            worktree: worktree.clone(),
            name: name.into(),
            kind: AgentKind::Claude,
            model: None,
            effort: None,
            auto_title: false,
            cloud_prompt: None,
        },
    )
    .await
    .unwrap();
    let events = read_events_until(c, EVENT_TIMEOUT, |evs| find_ack(evs, req_id).is_some()).await;
    let ServerEvent::Ack {
        created: Some(EntityId::Agent(id)),
        ..
    } = find_ack(&events, req_id).unwrap()
    else {
        panic!("CreateAgent failed: {events:#?}");
    };
    id.clone()
}

/// Poll a pidfile the fake agent writes on boot.
async fn read_pidfile(path: &Path) -> i32 {
    let deadline = tokio::time::Instant::now() + EVENT_TIMEOUT;
    loop {
        if let Ok(s) = std::fs::read_to_string(path) {
            if let Ok(pid) = s.trim().parse() {
                return pid;
            }
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "pidfile {path:?} never appeared"
        );
        tokio::time::sleep(POLL_STEP).await;
    }
}

/// Archiving or deleting an agent must kill its CLI process — sessions must
/// not keep burning memory/CPU once the user has put them away.
#[tokio::test]
async fn archive_and_delete_kill_the_agent_process() {
    let env = TestEnv::new();
    let repo = env.make_repo();
    let pid_dir = env.tmp.path().join("pids");
    std::fs::create_dir_all(&pid_dir).unwrap();
    // Stand-in CLI: record the pid, then exec into a long sleep (same pid).
    let script = env.tmp.path().join("agent.sh");
    std::fs::write(
        &script,
        format!(
            "#!/bin/sh\necho $$ > '{}'/$NEBULA_AGENT_ID.pid\nexec sleep 600\n",
            pid_dir.display()
        ),
    )
    .unwrap();
    make_executable(&script);
    let mut daemon = env.spawn_daemon_with_agent_cmd(script.to_str().unwrap());

    let mut c = connect(&env.sock()).await;
    handshake(&mut c).await;
    let worktree = add_project_get_main_worktree(&mut c, &repo).await;

    let a1 = create_agent_get_id(&mut c, &worktree.id, "to-archive", 2).await;
    let a2 = create_agent_get_id(&mut c, &worktree.id, "to-delete", 3).await;
    let pid1 = read_pidfile(&pid_dir.join(format!("{}.pid", a1.0))).await;
    let pid2 = read_pidfile(&pid_dir.join(format!("{}.pid", a2.0))).await;
    assert!(pid_alive(pid1) && pid_alive(pid2), "fake CLIs should be up");

    // ---- archive kills the CLI and broadcasts archived + not-alive ----
    write_frame(
        &mut c,
        &ClientRequest::ArchiveAgent {
            req_id: 4,
            id: a1.clone(),
        },
    )
    .await
    .unwrap();
    // The Ack and the EntityUpserted broadcast race on the client stream —
    // wait for both.
    let events = read_events_until(&mut c, EVENT_TIMEOUT, |evs| {
        find_ack(evs, 4).is_some()
            && evs.iter().any(|e| {
                matches!(e, ServerEvent::EntityUpserted { entity: Entity::Agent(a) }
                    if a.id == a1 && a.archived)
            })
    })
    .await;
    let archived = events
        .iter()
        .find_map(|e| match e {
            ServerEvent::EntityUpserted {
                entity: Entity::Agent(a),
            } if a.id == a1 && a.archived => Some(a.clone()),
            _ => None,
        })
        .expect("archive upsert");
    assert!(
        !archived.alive,
        "archived agent should not be alive: {archived:?}"
    );
    wait_pid_dead(pid1, EVENT_TIMEOUT, "archived agent CLI").await;
    assert!(pid_alive(pid2), "the other agent must be untouched");

    // ---- delete kills the CLI too ----
    write_frame(
        &mut c,
        &ClientRequest::DeleteAgent {
            req_id: 5,
            id: a2.clone(),
        },
    )
    .await
    .unwrap();
    read_events_until(&mut c, EVENT_TIMEOUT, |evs| {
        find_ack(evs, 5).is_some()
            && evs
                .iter()
                .any(|e| matches!(e, ServerEvent::EntityRemoved { id: EntityId::Agent(id) } if *id == a2))
    })
    .await;
    wait_pid_dead(pid2, EVENT_TIMEOUT, "deleted agent CLI").await;

    write_frame(&mut c, &ClientRequest::Shutdown).await.unwrap();
    wait_for_exit(&mut daemon);
}

/// A wedged CLI that ignores SIGHUP still gets cleared on archive: the kill
/// watchdog SIGKILLs its whole process group (grandchildren included) after
/// the grace period.
#[tokio::test]
async fn archive_sigkills_an_agent_that_ignores_sighup() {
    let env = TestEnv::new();
    let repo = env.make_repo();
    let dir = env.tmp.path().to_path_buf();
    // HUP-immune stand-in with a background child; `trap '' HUP` is inherited
    // by `sleep`, so neither dies from the polite signal alone.
    let script = dir.join("stubborn-agent.sh");
    std::fs::write(
        &script,
        format!(
            concat!(
                "#!/bin/sh\n",
                "trap '' HUP\n",
                "echo $$ > '{d}/agent.pid'\n",
                "sleep 600 &\n",
                "echo $! > '{d}/child.pid'\n",
                "wait\n",
            ),
            d = dir.display()
        ),
    )
    .unwrap();
    make_executable(&script);
    let mut daemon = env.spawn_daemon_with_agent_cmd(script.to_str().unwrap());

    let mut c = connect(&env.sock()).await;
    handshake(&mut c).await;
    let worktree = add_project_get_main_worktree(&mut c, &repo).await;

    write_frame(
        &mut c,
        &ClientRequest::CreateAgent {
            req_id: 2,
            worktree: worktree.id.clone(),
            name: "stubborn".into(),
            kind: AgentKind::Claude,
            model: None,
            effort: None,
            auto_title: false,
            cloud_prompt: None,
        },
    )
    .await
    .unwrap();
    let events = read_events_until(&mut c, EVENT_TIMEOUT, |evs| find_ack(evs, 2).is_some()).await;
    let ServerEvent::Ack {
        created: Some(EntityId::Agent(agent_id)),
        ..
    } = find_ack(&events, 2).unwrap()
    else {
        panic!("CreateAgent failed: {events:#?}");
    };
    let agent_id = agent_id.clone();
    let shell_pid = read_pidfile(&dir.join("agent.pid")).await;
    let child_pid = read_pidfile(&dir.join("child.pid")).await;

    write_frame(
        &mut c,
        &ClientRequest::ArchiveAgent {
            req_id: 3,
            id: agent_id,
        },
    )
    .await
    .unwrap();
    read_events_until(&mut c, EVENT_TIMEOUT, |evs| find_ack(evs, 3).is_some()).await;
    // SIGHUP alone can't clear these; the ~3s watchdog escalation must.
    wait_pid_dead(shell_pid, SLOW_TIMEOUT, "HUP-immune agent CLI").await;
    wait_pid_dead(child_pid, SLOW_TIMEOUT, "agent CLI's grandchild").await;

    write_frame(&mut c, &ClientRequest::Shutdown).await.unwrap();
    wait_for_exit(&mut daemon);
}

/// One PrewarmWorktreeSessions must revive every dead session under the
/// worktree — no Attach involved — so the TUI can boot a worktree's
/// sessions the moment the user's selection rests on it. Archived agents
/// stay dead.
#[tokio::test]
async fn prewarm_worktree_sessions_boots_dead_sessions() {
    let env = TestEnv::new();
    let repo = env.make_repo();
    let mut daemon = env.spawn_daemon();
    let mut c = connect(&env.sock()).await;
    handshake(&mut c).await;
    let worktree = add_project_get_main_worktree(&mut c, &repo).await;

    // One agent, one terminal, one archived agent — every PTY dies with the
    // daemon below.
    write_frame(
        &mut c,
        &ClientRequest::CreateAgent {
            req_id: 2,
            worktree: worktree.id.clone(),
            name: "warmed".into(),
            kind: AgentKind::Claude,
            model: None,
            effort: None,
            auto_title: false,
            cloud_prompt: None,
        },
    )
    .await
    .unwrap();
    let events = read_events_until(&mut c, EVENT_TIMEOUT, |evs| find_ack(evs, 2).is_some()).await;
    let ServerEvent::Ack {
        created: Some(EntityId::Agent(agent_id)),
        ..
    } = find_ack(&events, 2).unwrap()
    else {
        panic!("CreateAgent failed: {events:#?}");
    };
    let agent_id = agent_id.clone();

    write_frame(
        &mut c,
        &ClientRequest::CreateTerminal {
            req_id: 3,
            worktree: worktree.id.clone(),
            name: None,
        },
    )
    .await
    .unwrap();
    let events = read_events_until(&mut c, EVENT_TIMEOUT, |evs| find_ack(evs, 3).is_some()).await;
    let ServerEvent::Ack {
        created: Some(EntityId::Terminal(term_id)),
        ..
    } = find_ack(&events, 3).unwrap()
    else {
        panic!("CreateTerminal failed: {events:#?}");
    };
    let term_id = term_id.clone();

    write_frame(
        &mut c,
        &ClientRequest::CreateAgent {
            req_id: 4,
            worktree: worktree.id.clone(),
            name: "shelved".into(),
            kind: AgentKind::Claude,
            model: None,
            effort: None,
            auto_title: false,
            cloud_prompt: None,
        },
    )
    .await
    .unwrap();
    let events = read_events_until(&mut c, EVENT_TIMEOUT, |evs| find_ack(evs, 4).is_some()).await;
    let ServerEvent::Ack {
        created: Some(EntityId::Agent(archived_id)),
        ..
    } = find_ack(&events, 4).unwrap()
    else {
        panic!("CreateAgent failed: {events:#?}");
    };
    let archived_id = archived_id.clone();
    write_frame(
        &mut c,
        &ClientRequest::ArchiveAgent {
            req_id: 5,
            id: archived_id.clone(),
        },
    )
    .await
    .unwrap();
    read_events_until(&mut c, EVENT_TIMEOUT, |evs| find_ack(evs, 5).is_some()).await;

    // Restart: rows persist, every PTY is dead.
    write_frame(&mut c, &ClientRequest::Shutdown).await.unwrap();
    wait_for_exit(&mut daemon);
    let mut daemon2 = env.spawn_daemon();
    let mut c2 = connect(&env.sock()).await;
    handshake(&mut c2).await;
    write_frame(&mut c2, &ClientRequest::Subscribe)
        .await
        .unwrap();
    let events = read_events_until(&mut c2, EVENT_TIMEOUT, |evs| {
        evs.iter()
            .any(|e| matches!(e, ServerEvent::Snapshot { .. }))
    })
    .await;
    let ServerEvent::Snapshot {
        agents, terminals, ..
    } = &events[0]
    else {
        panic!("expected snapshot");
    };
    assert!(agents.iter().all(|a| !a.alive), "agents dead after restart");
    assert!(
        terminals.iter().all(|t| !t.alive),
        "terminals dead after restart"
    );

    // One prewarm revives the agent and the terminal (upserts flip alive)…
    write_frame(
        &mut c2,
        &ClientRequest::PrewarmWorktreeSessions {
            worktree: worktree.id.clone(),
            cols: 80,
            rows: 24,
        },
    )
    .await
    .unwrap();
    let events = read_events_until(&mut c2, SLOW_TIMEOUT, |evs| {
        let agent_alive = evs.iter().any(|e| {
            matches!(e, ServerEvent::EntityUpserted { entity: Entity::Agent(a) }
                if a.id == agent_id && a.alive)
        });
        let term_alive = evs.iter().any(|e| {
            matches!(e, ServerEvent::EntityUpserted { entity: Entity::Terminal(t) }
                if t.id == term_id && t.alive)
        });
        agent_alive && term_alive
    })
    .await;
    // …and never touches the archived agent.
    assert!(
        !events.iter().any(|e| {
            matches!(e, ServerEvent::EntityUpserted { entity: Entity::Agent(a) }
                if a.id == archived_id && a.alive)
        }),
        "archived agent stayed dead: {events:#?}"
    );

    write_frame(&mut c2, &ClientRequest::Shutdown)
        .await
        .unwrap();
    wait_for_exit(&mut daemon2);
}

/// The idle reaper kills sessions in worktrees no client is looking at once
/// they age past `session_idle_timeout` — but spares terminals with a
/// command still running, and never touches an attached session no matter
/// how long it idles. A reaped agent revives on the next attach.
#[tokio::test]
async fn idle_sessions_reap_unwatched_but_spare_busy_and_attached() {
    let env = TestEnv::new();
    let repo = env.make_repo();
    env.write_config(r#"{"session_idle_timeout": "2s"}"#);
    let mut daemon = env.spawn_daemon_with("/bin/sh", &[(env::IDLE_REAP_MS, "200")]);
    let mut c = connect(&env.sock()).await;
    handshake(&mut c).await;
    let worktree = add_project_get_main_worktree(&mut c, &repo).await;

    write_frame(
        &mut c,
        &ClientRequest::CreateAgent {
            req_id: 2,
            worktree: worktree.id.clone(),
            name: "idler".into(),
            kind: AgentKind::Claude,
            model: None,
            effort: None,
            auto_title: false,
            cloud_prompt: None,
        },
    )
    .await
    .unwrap();
    let events = read_events_until(&mut c, EVENT_TIMEOUT, |evs| find_ack(evs, 2).is_some()).await;
    let ServerEvent::Ack {
        created: Some(EntityId::Agent(agent_id)),
        ..
    } = find_ack(&events, 2).unwrap()
    else {
        panic!("CreateAgent failed: {events:#?}");
    };
    let agent_id = agent_id.clone();

    write_frame(
        &mut c,
        &ClientRequest::CreateTerminal {
            req_id: 3,
            worktree: worktree.id.clone(),
            name: None,
        },
    )
    .await
    .unwrap();
    let events = read_events_until(&mut c, EVENT_TIMEOUT, |evs| find_ack(evs, 3).is_some()).await;
    let ServerEvent::Ack {
        created: Some(EntityId::Terminal(term_id)),
        ..
    } = find_ack(&events, 3).unwrap()
    else {
        panic!("CreateTerminal failed: {events:#?}");
    };
    let term_id = term_id.clone();

    // A pinned agent: idle and unwatched like the idler, but marked "never
    // kill" (schedules and background jobs are invisible to the status
    // machine, so pinning is the user's protection).
    write_frame(
        &mut c,
        &ClientRequest::CreateAgent {
            req_id: 4,
            worktree: worktree.id.clone(),
            name: "keeper".into(),
            kind: AgentKind::Claude,
            model: None,
            effort: None,
            auto_title: false,
            cloud_prompt: None,
        },
    )
    .await
    .unwrap();
    let events = read_events_until(&mut c, EVENT_TIMEOUT, |evs| find_ack(evs, 4).is_some()).await;
    let ServerEvent::Ack {
        created: Some(EntityId::Agent(pinned_id)),
        ..
    } = find_ack(&events, 4).unwrap()
    else {
        panic!("CreateAgent failed: {events:#?}");
    };
    let pinned_id = pinned_id.clone();
    write_frame(
        &mut c,
        &ClientRequest::SetAgentPinned {
            req_id: 5,
            id: pinned_id.clone(),
            pinned: true,
        },
    )
    .await
    .unwrap();
    read_events_until(&mut c, EVENT_TIMEOUT, |evs| find_ack(evs, 5).is_some()).await;

    // Give the terminal a running command, then stop looking at anything.
    let term_sref = SessionRef::Terminal(term_id.clone());
    write_frame(
        &mut c,
        &ClientRequest::Attach {
            session: term_sref.clone(),
            from_seq: None,
            cols: 80,
            rows: 24,
        },
    )
    .await
    .unwrap();
    write_frame(
        &mut c,
        &ClientRequest::Input {
            session: term_sref.clone(),
            data: b"sleep 30\n".to_vec(),
        },
    )
    .await
    .unwrap();
    write_frame(
        &mut c,
        &ClientRequest::Detach {
            session: term_sref.clone(),
        },
    )
    .await
    .unwrap();

    // Unwatched: the idle agent is reaped after ~2s…
    let events = read_events_until(&mut c, SLOW_TIMEOUT, |evs| {
        evs.iter().any(|e| {
            matches!(e, ServerEvent::EntityUpserted { entity: Entity::Agent(a) }
                if a.id == agent_id && !a.alive)
        })
    })
    .await;
    // …while the terminal's sleep and the keeper's pin keep them alive.
    let term_reaped = |evs: &[ServerEvent]| {
        evs.iter().any(|e| {
            matches!(e, ServerEvent::EntityUpserted { entity: Entity::Terminal(t) }
                if t.id == term_id && !t.alive)
        })
    };
    let pinned_reaped = |evs: &[ServerEvent]| {
        evs.iter().any(|e| {
            matches!(e, ServerEvent::EntityUpserted { entity: Entity::Agent(a) }
                if a.id == pinned_id && !a.alive)
        })
    };
    assert!(!term_reaped(&events), "busy terminal spared: {events:#?}");
    assert!(!pinned_reaped(&events), "pinned agent spared: {events:#?}");

    // Attaching revives the agent; an attached session then idles forever.
    let agent_sref = SessionRef::Agent(agent_id.clone());
    write_frame(
        &mut c,
        &ClientRequest::Attach {
            session: agent_sref.clone(),
            from_seq: None,
            cols: 80,
            rows: 24,
        },
    )
    .await
    .unwrap();
    read_events_until(&mut c, EVENT_TIMEOUT, |evs| {
        evs.iter().any(|e| {
            matches!(e, ServerEvent::EntityUpserted { entity: Entity::Agent(a) }
                if a.id == agent_id && a.alive)
        })
    })
    .await;
    // Well past the 2s timeout with sweeps every 200ms.
    tokio::time::sleep(Duration::from_secs(4)).await;
    write_frame(
        &mut c,
        &ClientRequest::RenameAgent {
            req_id: 6,
            id: agent_id.clone(),
            name: "still-here".into(),
        },
    )
    .await
    .unwrap();
    let events = read_events_until(&mut c, EVENT_TIMEOUT, |evs| find_ack(evs, 6).is_some()).await;
    assert!(
        !events.iter().any(|e| {
            matches!(e, ServerEvent::EntityUpserted { entity: Entity::Agent(a) }
                if a.id == agent_id && !a.alive)
        }),
        "attached agent never reaped: {events:#?}"
    );
    assert!(
        !term_reaped(&events),
        "in-view terminal spared: {events:#?}"
    );
    assert!(
        !pinned_reaped(&events),
        "pinned agent still spared: {events:#?}"
    );

    write_frame(&mut c, &ClientRequest::Shutdown).await.unwrap();
    wait_for_exit(&mut daemon);
}

fn wait_for_exit(daemon: &mut DaemonProc) {
    let deadline = std::time::Instant::now() + EVENT_TIMEOUT;
    loop {
        match daemon.try_wait().unwrap() {
            Some(status) => {
                assert!(status.success(), "daemon exited with {status:?}");
                return;
            }
            None if std::time::Instant::now() < deadline => std::thread::sleep(POLL_STEP),
            None => {
                let _ = daemon.kill();
                panic!("daemon did not exit after Shutdown");
            }
        }
    }
}

/// Poll the env dump the fake agent CLI writes on boot, returning the
/// NEBULA_* variables the real CLI's hooks (and `nebula rename`) would see.
async fn read_env_file(path: &Path) -> std::collections::HashMap<String, String> {
    let deadline = tokio::time::Instant::now() + EVENT_TIMEOUT;
    loop {
        if let Ok(s) = std::fs::read_to_string(path) {
            let map: std::collections::HashMap<String, String> = s
                .lines()
                .filter_map(|l| l.split_once('='))
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect();
            if map.contains_key(env::API_URL) && map.contains_key(env::API_TOKEN) {
                return map;
            }
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "agent env dump {path:?} never appeared"
        );
        tokio::time::sleep(POLL_STEP).await;
    }
}

/// Raw HTTP POST to the daemon's hook receiver, standing in for the curl
/// one-liner the installed hook runs; returns (status, body) — the body is
/// what the hook would pipe into the CLI's stdout.
async fn hook_post(port: u16, path_query: &str, token: &str) -> (u16, String) {
    hook_post_json(port, path_query, token, r#"{"session_id":"s1"}"#).await
}

/// `hook_post` with the payload the CLI would have piped in.
async fn hook_post_json(port: u16, path_query: &str, token: &str, payload: &str) -> (u16, String) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let mut s = tokio::net::TcpStream::connect(("127.0.0.1", port))
        .await
        .unwrap();
    let req = format!(
        "POST {path_query} HTTP/1.1\r\nHost: 127.0.0.1\r\nAuthorization: Bearer {token}\r\n\
         Content-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{payload}",
        payload.len()
    );
    s.write_all(req.as_bytes()).await.unwrap();
    let mut buf = Vec::new();
    s.read_to_end(&mut buf).await.unwrap();
    let text = String::from_utf8_lossy(&buf).to_string();
    let status: u16 = text.split_whitespace().nth(1).unwrap().parse().unwrap();
    let body = text
        .split_once("\r\n\r\n")
        .map(|(_, b)| b.to_string())
        .unwrap_or_default();
    (status, body)
}

/// The whole auto-title loop over real processes: a default-named agent's
/// UserPromptSubmit hook response carries the titling instruction, the
/// `nebula rename` CLI (what the model runs) applies it exactly once and
/// broadcasts the new name, and afterwards the instruction stops and a
/// retitle attempt is declined without failing.
#[tokio::test]
async fn auto_title_instruction_and_rename_flow() {
    let env = TestEnv::new();
    let repo = env.make_repo();
    let env_dir = env.tmp.path().join("agent-env");
    std::fs::create_dir_all(&env_dir).unwrap();
    // Stand-in CLI: capture the NEBULA_* env its hooks would use, then park.
    let script = env.tmp.path().join("agent.sh");
    std::fs::write(
        &script,
        format!(
            "#!/bin/sh\nenv | grep '^NEBULA_' > '{}'/$NEBULA_AGENT_ID.env\nexec sleep 600\n",
            env_dir.display()
        ),
    )
    .unwrap();
    make_executable(&script);
    let mut daemon = env.spawn_daemon_with_agent_cmd(script.to_str().unwrap());

    let mut c = connect(&env.sock()).await;
    handshake(&mut c).await;
    let worktree = add_project_get_main_worktree(&mut c, &repo).await;

    // Created with the accepted default name → auto-title pending.
    write_frame(
        &mut c,
        &ClientRequest::CreateAgent {
            req_id: 2,
            worktree: worktree.id.clone(),
            name: "agent-1".into(),
            kind: AgentKind::Claude,
            model: None,
            effort: None,
            auto_title: true,
            cloud_prompt: None,
        },
    )
    .await
    .unwrap();
    let events = read_events_until(&mut c, EVENT_TIMEOUT, |evs| find_ack(evs, 2).is_some()).await;
    let ServerEvent::Ack {
        created: Some(EntityId::Agent(agent_id)),
        ..
    } = find_ack(&events, 2).unwrap()
    else {
        panic!("CreateAgent failed: {events:#?}");
    };
    let agent_id = agent_id.clone();

    let agent_env = read_env_file(&env_dir.join(format!("{}.env", agent_id.0))).await;
    let port: u16 = agent_env[env::API_URL]
        .rsplit(':')
        .next()
        .unwrap()
        .parse()
        .unwrap();
    let token = agent_env[env::API_TOKEN].clone();
    let submit_path = format!(
        "/api/hooks/claude?agentId={}&hookEvent=UserPromptSubmit",
        agent_id.0
    );

    // First prompt on the untitled session: instruction rides the response.
    let (status, body) = hook_post(port, &submit_path, &token).await;
    assert_eq!(status, 200);
    assert_eq!(body, nebula_daemon::hooks::auto_title_injection());

    // The model obeys — `nebula rename` runs with the session's env.
    let out = agent_cli(&env, &agent_id, &["rename", "Fix", "Login", "Redirect"]);
    assert!(out.status.success(), "rename failed: {out:?}");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("Fix Login Redirect"), "stdout: {stdout}");
    read_events_until(&mut c, EVENT_TIMEOUT, |evs| {
        evs.iter().any(|e| {
            matches!(e, ServerEvent::EntityUpserted { entity: Entity::Agent(a) }
                if a.id == agent_id && a.name == "Fix Login Redirect")
        })
    })
    .await;

    // Titled now: the next prompt injects nothing.
    let (status, body) = hook_post(port, &submit_path, &token).await;
    assert_eq!((status, body.as_str()), (200, ""));

    // A repeat attempt is declined as a settled answer (exit 0), not a fault.
    let out = agent_cli(&env, &agent_id, &["rename", "Another", "Title"]);
    assert!(out.status.success(), "declined rename must exit 0: {out:?}");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("already has a title"), "stdout: {stdout}");

    write_frame(&mut c, &ClientRequest::Shutdown).await.unwrap();
    wait_for_exit(&mut daemon);
}

/// `nebula worktree <name>` from inside a session, end to end over real
/// processes: the CLI (what the model runs) creates the checkout in nebula's
/// sibling layout and re-homes the row at once; the live PTY is left alone
/// until the turn's Stop hook — a tool hook still reporting the old
/// checkout's cwd in between must not drag the row back — and then respawns
/// inside the worktree.
#[tokio::test]
async fn nebula_worktree_cli_relocates_the_session_when_the_turn_ends() {
    let env = TestEnv::new();
    let repo = env.make_repo();
    let env_dir = env.tmp.path().join("agent-env");
    std::fs::create_dir_all(&env_dir).unwrap();
    // Stand-in CLI: dump the NEBULA_* env its hooks would use, log where
    // each boot runs, then park.
    let script = env.tmp.path().join("agent.sh");
    std::fs::write(
        &script,
        format!(
            "#!/bin/sh\nenv | grep '^NEBULA_' > '{d}'/$NEBULA_AGENT_ID.env\n\
             pwd >> '{d}'/$NEBULA_AGENT_ID.pwd\nexec sleep 600\n",
            d = env_dir.display()
        ),
    )
    .unwrap();
    make_executable(&script);
    let mut daemon = env.spawn_daemon_with_agent_cmd(script.to_str().unwrap());

    let mut c = connect(&env.sock()).await;
    handshake(&mut c).await;
    let main_worktree = add_project_get_main_worktree(&mut c, &repo).await;

    write_frame(
        &mut c,
        &ClientRequest::CreateAgent {
            req_id: 2,
            worktree: main_worktree.id.clone(),
            name: "agent-1".into(),
            kind: AgentKind::Claude,
            model: None,
            effort: None,
            auto_title: false,
            cloud_prompt: None,
        },
    )
    .await
    .unwrap();
    let events = read_events_until(&mut c, EVENT_TIMEOUT, |evs| find_ack(evs, 2).is_some()).await;
    let ServerEvent::Ack {
        created: Some(EntityId::Agent(agent_id)),
        ..
    } = find_ack(&events, 2).unwrap()
    else {
        panic!("CreateAgent failed: {events:#?}");
    };
    let agent_id = agent_id.clone();
    let agent_env = read_env_file(&env_dir.join(format!("{}.env", agent_id.0))).await;
    let port: u16 = agent_env[env::API_URL]
        .rsplit(':')
        .next()
        .unwrap()
        .parse()
        .unwrap();
    let token = agent_env[env::API_TOKEN].clone();
    let pwd_log = env_dir.join(format!("{}.pwd", agent_id.0));
    let boots = |path: &Path| -> Vec<String> {
        std::fs::read_to_string(path)
            .unwrap_or_default()
            .lines()
            .map(str::to_string)
            .collect()
    };
    let deadline = tokio::time::Instant::now() + EVENT_TIMEOUT;
    while boots(&pwd_log).is_empty() {
        assert!(
            tokio::time::Instant::now() < deadline,
            "first boot never logged"
        );
        tokio::time::sleep(POLL_STEP).await;
    }

    // The model obeys the guidance — `nebula worktree feat x` (the space
    // slugifies) with the session's env.
    let out = agent_cli(&env, &agent_id, &["worktree", "feat", "x"]);
    assert!(out.status.success(), "nebula worktree failed: {out:?}");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("feat-x") && stdout.contains("this turn ends"),
        "stdout: {stdout}"
    );

    // The row re-homes under the new checkout at once…
    let events = read_events_until(&mut c, SLOW_TIMEOUT, |evs| {
        evs.iter().any(|e| {
            matches!(e, ServerEvent::EntityUpserted { entity: Entity::Agent(a) }
                if a.id == agent_id && a.worktree_id != main_worktree.id)
        })
    })
    .await;
    let feat = events
        .iter()
        .find_map(|e| match e {
            ServerEvent::EntityUpserted {
                entity: Entity::Worktree(w),
            } if w.branch == "feat-x" => Some(w.clone()),
            _ => None,
        })
        .expect("feat-x worktree upsert");
    assert!(
        feat.path.ends_with("repo-worktrees/feat-x"),
        "nebula's sibling layout: {:?}",
        feat.path
    );
    assert!(feat.path.join(".git").exists(), "a real checkout");
    // …while the process is untouched: still the one boot, in the old checkout.
    assert_eq!(boots(&pwd_log).len(), 1, "no respawn before the turn ends");

    // Mid-turn the CLI's hooks keep reporting the old checkout's cwd; that
    // must not drag the row back. Then the Stop — same old cwd — ends the
    // turn and triggers the relocation.
    let payload = format!(
        r#"{{"session_id":"s1","cwd":"{}","tool_name":"Bash"}}"#,
        repo.display()
    );
    for event in ["PostToolUse", "Stop"] {
        let path = format!("/api/hooks/claude?agentId={}&hookEvent={event}", agent_id.0);
        let (status, _) = hook_post_json(port, &path, &token, &payload).await;
        assert_eq!(status, 200, "{event}");
    }

    // The respawn: alive again under feat-x, and booted inside it.
    read_events_until(&mut c, SLOW_TIMEOUT, |evs| {
        evs.iter().any(|e| {
            matches!(e, ServerEvent::EntityUpserted { entity: Entity::Agent(a) }
                if a.id == agent_id && a.worktree_id == feat.id && a.alive)
        })
    })
    .await;
    let deadline = tokio::time::Instant::now() + SLOW_TIMEOUT;
    loop {
        let b = boots(&pwd_log);
        if b.len() >= 2 {
            assert_eq!(b.len(), 2, "one respawn, not several: {b:?}");
            assert!(
                b[1].ends_with("repo-worktrees/feat-x"),
                "respawned inside the worktree: {b:?}"
            );
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "respawn never booted in the worktree: {b:?}"
        );
        tokio::time::sleep(POLL_STEP).await;
    }

    // Already there now: a settled answer, and no second relocation.
    let out = agent_cli(&env, &agent_id, &["worktree", "feat-x"]);
    assert!(out.status.success(), "repeat must exit 0: {out:?}");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("already runs inside it"),
        "stdout: {stdout}"
    );

    write_frame(&mut c, &ClientRequest::Shutdown).await.unwrap();
    wait_for_exit(&mut daemon);
}

/// `nebula add <dir>` and the bare `nebula <dir>` shorthand: the one-shot CLI
/// resolves the path against its own cwd (the daemon's differs), registers
/// the repo over IPC, and surfaces daemon rejections as nonzero exits.
/// Two nebula instances are two independent views: one switching workspaces
/// leaves the other where its user put it, and each one's `AddProject`
/// lands in the workspace *it* is looking at. The daemon still remembers
/// the last pick as where a fresh instance boots.
#[tokio::test]
async fn workspace_scope_is_per_connection() {
    let env = TestEnv::new();
    let mut daemon = env.spawn_daemon();

    // Boot one client and report the workspace its snapshot lands it in.
    let boot_client = |sock: PathBuf| async move {
        let mut c = connect(&sock).await;
        handshake(&mut c).await;
        let events = subscribe(&mut c).await;
        let active = events
            .iter()
            .find_map(|e| match e {
                ServerEvent::Snapshot {
                    active_workspace, ..
                } => Some(active_workspace.clone()),
                _ => None,
            })
            .expect("snapshot carries the workspace to boot into");
        (c, active)
    };

    // Two instances, both booted into the default workspace.
    let (mut a, a_boot) = boot_client(env.sock()).await;
    let (mut b, b_boot) = boot_client(env.sock()).await;
    assert_eq!(a_boot.as_str(), "default");
    assert_eq!(b_boot.as_str(), "default");

    // Instance A creates a workspace and switches into it.
    write_frame(
        &mut a,
        &ClientRequest::AddWorkspace {
            req_id: 1,
            name: "client".into(),
        },
    )
    .await
    .unwrap();
    let events = read_events_until(&mut a, EVENT_TIMEOUT, |evs| {
        evs.iter()
            .any(|e| matches!(e, ServerEvent::Ack { req_id: 1, .. }))
    })
    .await;
    let ws = events
        .iter()
        .find_map(|e| match e {
            ServerEvent::Ack {
                req_id: 1,
                created: Some(EntityId::Workspace(id)),
            } => Some(id.clone()),
            _ => None,
        })
        .expect("AddWorkspace acks with the new id");
    write_frame(
        &mut a,
        &ClientRequest::OpenWorkspace {
            req_id: 2,
            id: ws.clone(),
        },
    )
    .await
    .unwrap();
    read_events_until(&mut a, EVENT_TIMEOUT, |evs| {
        evs.iter()
            .any(|e| matches!(e, ServerEvent::Ack { req_id: 2, .. }))
    })
    .await;

    // A project added from A lands in A's workspace...
    let repo_a = env.make_repo();
    write_frame(
        &mut a,
        &ClientRequest::AddProject {
            req_id: 3,
            path: repo_a.clone(),
            name: None,
            create_missing: false,
        },
    )
    .await
    .unwrap();
    let events = read_events_until(&mut a, EVENT_TIMEOUT, |evs| {
        evs.iter()
            .any(|e| matches!(e, ServerEvent::Ack { req_id: 3, .. }))
    })
    .await;
    let project_a = events
        .iter()
        .find_map(|e| match e {
            ServerEvent::EntityUpserted {
                entity: Entity::Project(p),
            } => Some(p.clone()),
            _ => None,
        })
        .expect("AddProject upserts the project");
    assert_eq!(
        project_a.workspace_id, ws,
        "A's project lands in the workspace A switched to"
    );

    // ...while B, which never switched, is still adding to the default.
    // This is the whole bug: B must not have been dragged along.
    let repo_b = env.tmp.path().join("repo-b");
    make_repo_at(&repo_b);
    write_frame(
        &mut b,
        &ClientRequest::AddProject {
            req_id: 4,
            path: repo_b.clone(),
            name: None,
            create_missing: false,
        },
    )
    .await
    .unwrap();
    let events = read_events_until(&mut b, EVENT_TIMEOUT, |evs| {
        evs.iter()
            .any(|e| matches!(e, ServerEvent::Ack { req_id: 4, .. }))
    })
    .await;
    let project_b = events
        .iter()
        .find_map(|e| match e {
            ServerEvent::EntityUpserted {
                entity: Entity::Project(p),
            } if p.repo_path == repo_b.canonicalize().unwrap() => Some(p.clone()),
            _ => None,
        })
        .expect("AddProject upserts the project");
    assert_eq!(
        project_b.workspace_id.as_str(),
        "default",
        "B never switched, so B still adds to the workspace it booted into"
    );

    // A third instance launched now boots into A's pick — the switch is
    // remembered as a default for new clients, just not pushed onto live ones.
    let (_c, c_boot) = boot_client(env.sock()).await;
    assert_eq!(
        c_boot, ws,
        "a fresh instance opens the last workspace opened"
    );

    write_frame(&mut a, &ClientRequest::Shutdown).await.unwrap();
    wait_for_exit(&mut daemon);
}

/// `TestEnv::make_repo` at a caller-chosen path, for tests that need two.
fn make_repo_at(repo: &Path) {
    std::fs::create_dir_all(repo).unwrap();
    let git = |args: &[&str]| {
        let ok = std::process::Command::new("git")
            .arg("-C")
            .arg(repo)
            .args(args)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .unwrap()
            .success();
        assert!(ok, "git {args:?} failed");
    };
    git(&["init", "-b", "main"]);
    git(&["config", "user.email", "test@nebula.dev"]);
    git(&["config", "user.name", "nebula-test"]);
    std::fs::write(repo.join("README.md"), "# test\n").unwrap();
    git(&["add", "."]);
    git(&["commit", "-m", "init"]);
}

/// Mark a freshly written stub script runnable.
fn make_executable(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

/// Subscribe a handshaken client and wait for its first `Snapshot`; returns
/// everything received up to and including it.
async fn subscribe(c: &mut UnixStream) -> Vec<ServerEvent> {
    write_frame(c, &ClientRequest::Subscribe).await.unwrap();
    read_events_until(c, EVENT_TIMEOUT, |evs| {
        evs.iter()
            .any(|e| matches!(e, ServerEvent::Snapshot { .. }))
    })
    .await
}

/// Run the `nebula` CLI the way a hook would inside an agent session: the
/// test daemon's runtime dir plus the session's `NEBULA_AGENT_ID`.
fn agent_cli(
    env: &TestEnv,
    agent_id: &nebula_core::AgentId,
    args: &[&str],
) -> std::process::Output {
    std::process::Command::new(env!("CARGO_BIN_EXE_nebula"))
        .args(args)
        .env(env::RUNTIME_DIR, &env.runtime_dir)
        .env(env::AGENT_ID, &agent_id.0)
        .output()
        .unwrap()
}

#[tokio::test]
async fn cli_add_project() {
    let env = TestEnv::new();
    let repo = env.make_repo();
    let mut daemon = env.spawn_daemon();
    let mut c = connect(&env.sock()).await;
    handshake(&mut c).await;
    subscribe(&mut c).await;

    let run_cli =
        |args: &[&str], cwd: &Path| env.cli().args(args).current_dir(cwd).output().unwrap();

    // `nebula add .` from inside the repo: cwd-relative resolution, project
    // named after the directory.
    let out = run_cli(&["add", "."], &repo);
    assert!(out.status.success(), "add . failed: {out:?}");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("added project"), "stdout: {stdout}");
    let canon = repo.canonicalize().unwrap();
    read_events_until(&mut c, EVENT_TIMEOUT, |evs| {
        evs.iter().any(|e| {
            matches!(e, ServerEvent::EntityUpserted { entity: Entity::Project(p) }
                if p.repo_path == canon && p.name == "repo")
        })
    })
    .await;

    // The same repo again: the daemon's dedupe comes back as a failure.
    let out = run_cli(&["add", repo.to_str().unwrap()], env.tmp.path());
    assert!(!out.status.success(), "duplicate add must fail: {out:?}");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("already added"), "stderr: {stderr}");

    // Dedupe is per-workspace: the same repo is welcome in a second
    // workspace (workspaces are free-form groupings the user curates).
    write_frame(
        &mut c,
        &ClientRequest::AddWorkspace {
            req_id: 91,
            name: "second".into(),
        },
    )
    .await
    .unwrap();
    let events = read_events_until(&mut c, EVENT_TIMEOUT, |evs| {
        evs.iter()
            .any(|e| matches!(e, ServerEvent::Ack { req_id: 91, .. }))
    })
    .await;
    let ws_id = events
        .iter()
        .find_map(|e| match e {
            ServerEvent::Ack {
                req_id: 91,
                created: Some(EntityId::Workspace(id)),
            } => Some(id.clone()),
            _ => None,
        })
        .expect("AddWorkspace ack carries the new workspace id");
    write_frame(
        &mut c,
        &ClientRequest::OpenWorkspace {
            req_id: 92,
            id: ws_id,
        },
    )
    .await
    .unwrap();
    read_events_until(&mut c, EVENT_TIMEOUT, |evs| {
        evs.iter()
            .any(|e| matches!(e, ServerEvent::Ack { req_id: 92, .. }))
    })
    .await;
    let out = run_cli(&["add", repo.to_str().unwrap()], env.tmp.path());
    assert!(
        out.status.success(),
        "same repo in another workspace must succeed: {out:?}"
    );
    // …but only once per workspace.
    let out = run_cli(&["add", repo.to_str().unwrap()], env.tmp.path());
    assert!(
        !out.status.success(),
        "duplicate within the second workspace must fail: {out:?}"
    );

    // Bare `nebula <dir>` shorthand on a second repo.
    let repo2 = env.tmp.path().join("repo2");
    std::fs::create_dir_all(&repo2).unwrap();
    for args in [
        vec!["init", "-b", "main"],
        vec!["commit", "--allow-empty", "-m", "init"],
    ] {
        let ok = std::process::Command::new("git")
            .arg("-C")
            .arg(&repo2)
            .args(&args)
            .env("GIT_AUTHOR_NAME", "nebula-test")
            .env("GIT_AUTHOR_EMAIL", "test@nebula.dev")
            .env("GIT_COMMITTER_NAME", "nebula-test")
            .env("GIT_COMMITTER_EMAIL", "test@nebula.dev")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .unwrap()
            .success();
        assert!(ok, "git {args:?} failed");
    }
    let out = run_cli(&[repo2.to_str().unwrap()], env.tmp.path());
    assert!(out.status.success(), "bare add failed: {out:?}");
    let canon2 = repo2.canonicalize().unwrap();
    read_events_until(&mut c, EVENT_TIMEOUT, |evs| {
        evs.iter().any(|e| {
            matches!(e, ServerEvent::EntityUpserted { entity: Entity::Project(p) }
                if p.repo_path == canon2 && p.name == "repo2")
        })
    })
    .await;

    // A directory that isn't a git repo is rejected by the daemon.
    let plain = env.tmp.path().join("plain");
    std::fs::create_dir_all(&plain).unwrap();
    let out = run_cli(&["add", plain.to_str().unwrap()], env.tmp.path());
    assert!(!out.status.success(), "non-repo add must fail: {out:?}");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("not a git repository"), "stderr: {stderr}");

    // A path that doesn't exist fails client-side, before any IPC.
    let out = run_cli(&["add", "does-not-exist"], env.tmp.path());
    assert!(!out.status.success(), "missing dir must fail: {out:?}");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("does not exist"), "stderr: {stderr}");

    write_frame(&mut c, &ClientRequest::Shutdown).await.unwrap();
    wait_for_exit(&mut daemon);
}

/// A teleport is a snapshot of the cloud session, not a live link, so the
/// row keeps re-teleporting to stay current — that is what makes a cloud
/// agent's work show up in nebula at all. The follow ends the moment the
/// pane is typed into: from then on it is the user's local session, and
/// respawning it under them would eat their turn.
#[tokio::test]
async fn cloud_mirror_refreshes_until_the_pane_is_typed_into() {
    let env = TestEnv::new();
    let repo = env.make_repo();
    let state = env.tmp.path().join("mirror-stub");
    std::fs::create_dir_all(&state).unwrap();
    let stub = env.tmp.path().join("mirror-stub.sh");
    std::fs::write(
        &stub,
        format!(
            r#"#!/bin/sh
n=$(cat "{state}/runs" 2>/dev/null || echo 0)
n=$((n + 1))
echo "$n" > "{state}/runs"
case "$n" in
  1)
    printf 'Created cloud session: Follow me\r\n'
    printf 'Resume with: claude --teleport session_01SQugK2HDyk33coSrfqFJk4\r\n'
    exit 0
    ;;
  2)
    printf 'Error: Attaching to an existing cloud session is not enabled for your account.\r\n'
    exit 1
    ;;
  *)
    exec sleep 300
    ;;
esac
"#,
            state = state.display()
        ),
    )
    .unwrap();
    make_executable(&stub);
    let runs = || {
        std::fs::read_to_string(state.join("runs"))
            .ok()
            .and_then(|s| s.trim().parse::<u32>().ok())
            .unwrap_or(0)
    };
    let mut daemon =
        env.spawn_daemon_with(stub.to_str().unwrap(), &[(env::CLOUD_MIRROR_SECS, "2")]);

    let mut c = connect(&env.sock()).await;
    handshake(&mut c).await;
    let main_worktree = add_project_get_main_worktree(&mut c, &repo).await;

    write_frame(
        &mut c,
        &ClientRequest::CreateAgent {
            req_id: 10,
            worktree: main_worktree.id.clone(),
            name: "cloud".into(),
            kind: AgentKind::Claude,
            model: None,
            effort: None,
            auto_title: false,
            cloud_prompt: Some("Follow me".into()),
        },
    )
    .await
    .unwrap();
    let events = read_events_until(&mut c, SLOW_TIMEOUT, |evs| find_ack(evs, 10).is_some()).await;
    let ServerEvent::Ack {
        created: Some(EntityId::Agent(agent_id)),
        ..
    } = find_ack(&events, 10).unwrap()
    else {
        panic!("CreateAgent failed: {events:#?}");
    };
    let agent_id = agent_id.clone();

    // create, refused attach, teleport — then the follow keeps going: each
    // tick kills the pane and teleports it again, pulling whatever the
    // cloud session has done since.
    let wait_for_runs = |target: u32| async move {
        let deadline = tokio::time::Instant::now() + SPAWN_CHAIN_TIMEOUT;
        while runs() < target && tokio::time::Instant::now() < deadline {
            tokio::time::sleep(POLL_STEP).await;
        }
        runs()
    };
    assert!(
        wait_for_runs(5).await >= 5,
        "the mirror re-teleports on its own; runs stalled at {}",
        runs()
    );

    // Attach and type: the pane is the user's from here.
    write_frame(
        &mut c,
        &ClientRequest::Attach {
            session: SessionRef::Agent(agent_id.clone()),
            from_seq: None,
            cols: 80,
            rows: 24,
        },
    )
    .await
    .unwrap();
    write_frame(
        &mut c,
        &ClientRequest::Input {
            session: SessionRef::Agent(agent_id.clone()),
            data: b"hello".to_vec(),
        },
    )
    .await
    .unwrap();

    // The badge clears when the follow gives up, so wait on that rather
    // than on a sleep — then hold still and confirm the runs stop climbing.
    let events = read_events_until(&mut c, SPAWN_CHAIN_TIMEOUT, |evs| {
        evs.iter().any(|e| {
            matches!(
                e,
                ServerEvent::EntityUpserted {
                    entity: Entity::Agent(a)
                } if a.id == agent_id && !a.cloud_mirroring
            )
        })
    })
    .await;
    assert!(
        events.iter().any(|e| matches!(
            e,
            ServerEvent::EntityUpserted { entity: Entity::Agent(a) }
                if a.id == agent_id && !a.cloud_mirroring
        )),
        "the row should stop advertising a follow it has given up: {events:#?}"
    );
    let settled = runs();
    tokio::time::sleep(EVENT_TIMEOUT).await;
    assert_eq!(
        runs(),
        settled,
        "an adopted pane must not be teleported over"
    );

    write_frame(&mut c, &ClientRequest::Shutdown).await.unwrap();
    wait_for_exit(&mut daemon);
}

/// The mirror must not be able to loop forever. If the pane it last
/// spawned is gone — the idle reaper took it because nobody has looked at
/// this row in a long time, or the teleport itself died — following stops
/// instead of respawning a session every tick, which would make cloud rows
/// the one kind nebula can never reap.
#[tokio::test]
async fn cloud_mirror_gives_up_when_its_pane_stops_coming_back() {
    let env = TestEnv::new();
    let repo = env.make_repo();
    let state = env.tmp.path().join("dying-stub");
    std::fs::create_dir_all(&state).unwrap();
    let stub = env.tmp.path().join("dying-stub.sh");
    std::fs::write(
        &stub,
        format!(
            r#"#!/bin/sh
n=$(cat "{state}/runs" 2>/dev/null || echo 0)
n=$((n + 1))
echo "$n" > "{state}/runs"
case "$n" in
  1)
    printf 'Created cloud session: Follow me\r\n'
    printf 'Resume with: claude --teleport session_01SQugK2HDyk33coSrfqFJk4\r\n'
    exit 0
    ;;
  2)
    printf 'Error: Attaching to an existing cloud session is not enabled for your account.\r\n'
    exit 1
    ;;
  3)
    exec sleep 300
    ;;
  *)
    exit 0
    ;;
esac
"#,
            state = state.display()
        ),
    )
    .unwrap();
    make_executable(&stub);
    let runs = || {
        std::fs::read_to_string(state.join("runs"))
            .ok()
            .and_then(|s| s.trim().parse::<u32>().ok())
            .unwrap_or(0)
    };
    let mut daemon =
        env.spawn_daemon_with(stub.to_str().unwrap(), &[(env::CLOUD_MIRROR_SECS, "2")]);

    let mut c = connect(&env.sock()).await;
    handshake(&mut c).await;
    let main_worktree = add_project_get_main_worktree(&mut c, &repo).await;
    write_frame(
        &mut c,
        &ClientRequest::CreateAgent {
            req_id: 10,
            worktree: main_worktree.id.clone(),
            name: "cloud".into(),
            kind: AgentKind::Claude,
            model: None,
            effort: None,
            auto_title: false,
            cloud_prompt: Some("Follow me".into()),
        },
    )
    .await
    .unwrap();
    let events = read_events_until(&mut c, SLOW_TIMEOUT, |evs| find_ack(evs, 10).is_some()).await;
    let ServerEvent::Ack {
        created: Some(EntityId::Agent(agent_id)),
        ..
    } = find_ack(&events, 10).unwrap()
    else {
        panic!("CreateAgent failed: {events:#?}");
    };
    let agent_id = agent_id.clone();

    // create, refused attach, teleport — then one tick teleports again
    // (run 4) and that child dies at once.
    let deadline = tokio::time::Instant::now() + SPAWN_CHAIN_TIMEOUT;
    while runs() < 4 && tokio::time::Instant::now() < deadline {
        tokio::time::sleep(POLL_STEP).await;
    }
    assert!(runs() >= 4, "the mirror never got a tick in: {}", runs());

    // The tick after that finds no pane and gives up. Watch for the badge
    // going quiet *after* it was lit — a row's upserts start out unmirrored,
    // and a spawn's upsert reaches the client before its child runs a line.
    let events = read_events_until(&mut c, SPAWN_CHAIN_TIMEOUT, |evs| {
        let is = |e: &ServerEvent, want: bool| {
            matches!(
                e,
                ServerEvent::EntityUpserted {
                    entity: Entity::Agent(a)
                } if a.id == agent_id && a.cloud_mirroring == want
            )
        };
        let lit = evs.iter().position(|e| is(e, true));
        let quiet = evs.iter().rposition(|e| is(e, false));
        matches!((lit, quiet), (Some(lit), Some(quiet)) if quiet > lit)
    })
    .await;
    let settled = runs();
    assert!(
        !events.is_empty(),
        "the mirror should have stopped advertising itself"
    );
    tokio::time::sleep(Duration::from_secs(7)).await;
    assert_eq!(runs(), settled, "no endless respawn loop");

    write_frame(&mut c, &ClientRequest::Shutdown).await.unwrap();
    wait_for_exit(&mut daemon);
}

/// A Claude Cloud row on an account without the live-attach rollout:
/// `claude --cloud <task>` prints the session id and exits, and the daemon
/// captures the id off the PTY and re-enters the session *on its own* —
/// nobody has to ask, because the alternative is a dead pane whose last
/// line tells the user to go watch their agent somewhere else. The attach
/// is refused (read off the output, not inferred from the exit), so the row
/// is teleported instead, inside a `cloud-<id>` worktree of its own rather
/// than on top of the user's main checkout. The stub stands in for all
/// three CLI invocations in turn.
#[tokio::test]
async fn cloud_row_captures_its_session_id_and_reenters_it() {
    let env = TestEnv::new();
    let repo = env.make_repo();
    let state = env.tmp.path().join("cloud-stub");
    std::fs::create_dir_all(&state).unwrap();
    let stub = env.tmp.path().join("cloud-stub.sh");
    std::fs::write(
        &stub,
        format!(
            r#"#!/bin/sh
n=$(cat "{state}/runs" 2>/dev/null || echo 0)
n=$((n + 1))
echo "$n" > "{state}/runs"
pwd >> "{state}/cwds"
case "$n" in
  1)
    printf 'Created cloud session: Hello world\r\n'
    printf 'View: https://claude.ai/code/session_016SiQW5Lem2LbnUf1A3undt?from=cli&m=0\r\n'
    printf 'Resume with: claude --teleport session_016SiQW5Lem2LbnUf1A3undt\r\n'
    exit 0
    ;;
  2)
    printf 'Error: Attaching to an existing cloud session is not enabled for your account.\r\n'
    exit 1
    ;;
  *)
    exec sleep 300
    ;;
esac
"#,
            state = state.display()
        ),
    )
    .unwrap();
    make_executable(&stub);
    let runs = || {
        std::fs::read_to_string(state.join("runs"))
            .map(|s| s.trim().to_string())
            .unwrap_or_default()
    };
    // A cadence long enough that no mirror tick lands inside the test: the
    // run counts here are about the re-entry chain, not the refresh loop
    // (which `cloud_mirror_refreshes_until_the_pane_is_typed_into` covers).
    let mut daemon =
        env.spawn_daemon_with(stub.to_str().unwrap(), &[(env::CLOUD_MIRROR_SECS, "600")]);

    let mut c = connect(&env.sock()).await;
    handshake(&mut c).await;
    let main_worktree = add_project_get_main_worktree(&mut c, &repo).await;

    // The create: the stub prints the session lines and exits at once.
    const CLOUD_ID: &str = "session_016SiQW5Lem2LbnUf1A3undt";
    write_frame(
        &mut c,
        &ClientRequest::CreateAgent {
            req_id: 10,
            worktree: main_worktree.id.clone(),
            name: "cloud".into(),
            kind: AgentKind::Claude,
            model: None,
            effort: None,
            auto_title: false,
            cloud_prompt: Some("Hello world".into()),
        },
    )
    .await
    .unwrap();
    let events = read_events_until(&mut c, SLOW_TIMEOUT, |evs| {
        find_ack(evs, 10).is_some()
            && evs.iter().any(|e| {
                matches!(
                    e,
                    ServerEvent::EntityUpserted {
                        entity: Entity::Agent(a)
                    } if a.cloud_session_id.as_deref() == Some(CLOUD_ID)
                )
            })
    })
    .await;
    let ServerEvent::Ack {
        created: Some(EntityId::Agent(agent_id)),
        ..
    } = find_ack(&events, 10).unwrap()
    else {
        panic!("CreateAgent failed: {events:#?}");
    };
    let agent_id = agent_id.clone();

    // Nothing more is asked of the daemon: capturing the id is what starts
    // the re-entry. Two further respawns of the row follow — the attach,
    // refused, and then the teleport.
    let events = read_events_until(&mut c, SPAWN_CHAIN_TIMEOUT, |evs| {
        let live_spawns = evs
            .iter()
            .filter(|e| {
                matches!(
                    e,
                    ServerEvent::EntityUpserted {
                        entity: Entity::Agent(a)
                    } if a.id == agent_id && a.alive
                )
            })
            .count();
        live_spawns >= 2
    })
    .await;
    // The spawn upsert goes out before the child has run a line; give the
    // stub a moment to record itself.
    let deadline = tokio::time::Instant::now() + EVENT_TIMEOUT;
    while runs() != "3" && tokio::time::Instant::now() < deadline {
        tokio::time::sleep(POLL_STEP).await;
    }
    assert_eq!(runs(), "3", "create, refused attach, teleport");
    let cloud_worktree = events
        .iter()
        .find_map(|e| match e {
            ServerEvent::EntityUpserted {
                entity: Entity::Worktree(w),
            } if w.branch == "cloud-f1A3undt" => Some(w.clone()),
            _ => None,
        })
        .expect("a cloud-<id> worktree was created for the attach");
    assert_eq!(cloud_worktree.project_id, main_worktree.project_id);
    assert!(!cloud_worktree.is_main);
    let row = events
        .iter()
        .rev()
        .find_map(|e| match e {
            ServerEvent::EntityUpserted {
                entity: Entity::Agent(a),
            } if a.id == agent_id => Some(a.clone()),
            _ => None,
        })
        .unwrap();
    assert_eq!(
        row.worktree_id, cloud_worktree.id,
        "row re-homed before attaching"
    );
    assert_eq!(row.cloud_session_id.as_deref(), Some(CLOUD_ID));
    assert!(row.alive);
    assert!(
        row.cloud_mirroring,
        "the teleported pane follows the cloud session from here"
    );

    // The create ran in the main checkout; the attach and the teleport both
    // ran in the new worktree — the user's checkout never switched branch.
    let cwds = std::fs::read_to_string(state.join("cwds")).unwrap();
    let cwds: Vec<PathBuf> = cwds
        .lines()
        .map(|l| std::fs::canonicalize(l).unwrap())
        .collect();
    assert_eq!(
        cwds,
        vec![
            std::fs::canonicalize(&main_worktree.path).unwrap(),
            std::fs::canonicalize(&cloud_worktree.path).unwrap(),
            std::fs::canonicalize(&cloud_worktree.path).unwrap(),
        ]
    );
    let main_branch = std::process::Command::new("git")
        .args(["-C", repo.to_str().unwrap(), "branch", "--show-current"])
        .output()
        .unwrap();
    assert_eq!(String::from_utf8_lossy(&main_branch.stdout).trim(), "main");

    // Restarting a row that is mirroring re-enters the cloud session rather
    // than resuming the local session the teleport left behind — and it
    // goes straight to the teleport, because this daemon has already seen
    // the attach refused once. One new run, not two.
    write_frame(
        &mut c,
        &ClientRequest::RestartAgent {
            req_id: 11,
            id: agent_id.clone(),
        },
    )
    .await
    .unwrap();
    let events = read_events_until(&mut c, SPAWN_CHAIN_TIMEOUT, |evs| {
        find_ack(evs, 11).is_some()
    })
    .await;
    assert!(
        matches!(find_ack(&events, 11), Some(ServerEvent::Ack { .. })),
        "RestartAgent failed: {events:#?}"
    );
    let deadline = tokio::time::Instant::now() + EVENT_TIMEOUT;
    while runs() != "4" && tokio::time::Instant::now() < deadline {
        tokio::time::sleep(POLL_STEP).await;
    }
    assert_eq!(
        runs(),
        "4",
        "one re-entry, and it skipped the refused attach"
    );

    // Shutting down must not spawn anything further.
    write_frame(&mut c, &ClientRequest::Shutdown).await.unwrap();
    wait_for_exit(&mut daemon);
    assert_eq!(runs(), "4");
}
