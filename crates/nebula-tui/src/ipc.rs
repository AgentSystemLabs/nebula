//! Client-side IPC: connect to the daemon (auto-spawning it when absent) and
//! perform the version handshake.

use anyhow::{bail, Context, Result};
use nebula_core::codec::{read_frame, write_frame};
use nebula_core::{
    env, paths, AgentId, ClientRequest, EnterOutcome, ServerEvent, PROTOCOL_VERSION,
};
use std::time::Duration;
use tokio::net::UnixStream;

/// Request id for the one-shot CLIs: each opens a fresh connection, sends a
/// single request and waits for its reply, so there is never a second id.
const ONE_SHOT_REQ_ID: u64 = 1;
/// The daemon hung up mid-request — the message every one-shot client shows.
const CLOSED_BEFORE_REPLY: &str = "daemon closed the connection before replying";
/// How often the connect and shutdown waits re-check the daemon.
const POLL_STEP: Duration = Duration::from_millis(50);

pub struct Connection {
    pub stream: UnixStream,
    pub daemon_pid: u32,
}

/// Connect, auto-spawning `current_exe() daemon` when nothing is listening.
pub async fn connect_or_spawn() -> Result<Connection> {
    let sock = paths::socket_path();

    if let Ok(conn) = try_connect(&sock).await {
        return handshake(conn).await;
    }

    spawn_daemon()?;

    // Poll-connect while the daemon boots.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
    loop {
        match try_connect(&sock).await {
            Ok(conn) => return handshake(conn).await,
            Err(_) if tokio::time::Instant::now() < deadline => {
                tokio::time::sleep(POLL_STEP).await;
            }
            Err(e) => {
                return Err(e).with_context(|| {
                    format!(
                        "daemon did not come up on {} — check {}",
                        sock.display(),
                        paths::daemon_log_path().display()
                    )
                })
            }
        }
    }
}

async fn try_connect(sock: &std::path::Path) -> Result<UnixStream> {
    Ok(UnixStream::connect(sock).await?)
}

fn spawn_daemon() -> Result<()> {
    use std::os::unix::process::CommandExt;
    let exe = std::env::current_exe().context("resolve current_exe")?;
    let mut cmd = std::process::Command::new(exe);
    cmd.arg("daemon")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    // New *session*, not just a new process group: besides outliving this
    // client and skipping its terminal signals (Ctrl+C etc.), the daemon must
    // hold no controlling terminal. It shells out to the user's interactive
    // shell (CLI probes, login-shell agent wrap), and an interactive zsh that
    // can reach a tty via /dev/tty grabs its foreground process group —
    // SIGTTIN-stopping the TUI running on this terminal mid-frame.
    unsafe {
        cmd.pre_exec(|| {
            if libc_setsid() < 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    cmd.spawn().context("spawn nebula daemon")?;
    Ok(())
}

// Avoid a libc dependency for one call (same pattern as nebula-core's geteuid).
fn libc_setsid() -> i32 {
    extern "C" {
        fn setsid() -> i32;
    }
    unsafe { setsid() }
}

async fn handshake(mut stream: UnixStream) -> Result<Connection> {
    write_frame(
        &mut stream,
        &ClientRequest::Hello {
            protocol_version: PROTOCOL_VERSION,
        },
    )
    .await?;
    match read_frame::<ServerEvent, _>(&mut stream).await? {
        Some(ServerEvent::HelloOk { daemon_pid, .. }) => Ok(Connection { stream, daemon_pid }),
        Some(ServerEvent::Incompatible {
            daemon_protocol_version,
        }) => bail!(version_skew_message(daemon_protocol_version)),
        other => bail!("unexpected handshake reply: {other:?}"),
    }
}

/// Explain a failed version handshake in terms of the fix.
///
/// Which side is stale decides the remedy, and getting it backwards costs a
/// debugging session: when the *daemon* is ahead, the `nebula` that just ran
/// is an older build than the one the daemon was launched from, and `nebula
/// kill` cannot help — the live instance immediately respawns its daemon
/// from its own binary (`spawn_daemon` above uses `current_exe`), so the
/// skew survives every restart. That is the common shape in a checkout,
/// where `make dev` runs `target/debug` while PATH still finds an older
/// `nebula` from the last `make install`.
fn version_skew_message(daemon_protocol_version: u32) -> String {
    let client = std::env::current_exe()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| "unknown".into());
    let daemon = daemon_exe_path().unwrap_or_else(|| "unknown".into());
    let header = format!(
        "protocol mismatch: the daemon speaks v{daemon_protocol_version}, this client \
         v{PROTOCOL_VERSION}\n  this client: {client}\n  the daemon:  {daemon}\n"
    );
    if daemon_protocol_version > PROTOCOL_VERSION {
        format!(
            "{header}This client is the older build, so `nebula kill` will not fix it — the \
             running instance respawns its daemon from its own binary. Install the daemon's \
             build over this one instead (`make install` from that checkout)."
        )
    } else {
        format!(
            "{header}The daemon is the older build — run `nebula kill` and relaunch. That \
             stops every live session."
        )
    }
}

/// Best-effort path of the binary the running daemon was launched from, so
/// the mismatch message can name it. Read from the pidfile rather than the
/// handshake: `Incompatible` is what a *newer* daemon sends an older client,
/// so adding a field to it would only break decoding on the clients that
/// need this message most. The buildstamp beside the pidfile is no help
/// either — it is a content hash, not a path.
fn daemon_exe_path() -> Option<String> {
    let pid = std::fs::read_to_string(paths::pidfile_path()).ok()?;
    let pid = pid.trim();
    if pid.is_empty() || !pid.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    if let Ok(path) = std::fs::read_link(format!("/proc/{pid}/exe")) {
        return Some(path.display().to_string());
    }
    let out = std::process::Command::new("ps")
        .args(["-p", pid, "-o", "comm="])
        .output()
        .ok()?;
    let path = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (!path.is_empty()).then_some(path)
}

/// Channel-based IPC handle for the TUI event loop: outbound requests go
/// through `tx`; inbound events arrive on `rx`. Reader/writer tasks own the
/// socket halves.
pub struct IpcChannels {
    pub tx: tokio::sync::mpsc::Sender<ClientRequest>,
    pub rx: tokio::sync::mpsc::Receiver<ServerEvent>,
}

pub fn split_connection(conn: Connection) -> IpcChannels {
    let (read_half, mut write_half) = conn.stream.into_split();
    let (event_tx, event_rx) = tokio::sync::mpsc::channel::<ServerEvent>(1024);
    let (req_tx, mut req_rx) = tokio::sync::mpsc::channel::<ClientRequest>(256);

    tokio::spawn(async move {
        let mut reader = tokio::io::BufReader::new(read_half);
        while let Ok(Some(ev)) = read_frame::<ServerEvent, _>(&mut reader).await {
            if event_tx.send(ev).await.is_err() {
                break;
            }
        }
        // Dropping event_tx closes the channel, signalling disconnect.
    });

    tokio::spawn(async move {
        while let Some(req) = req_rx.recv().await {
            if write_frame(&mut write_half, &req).await.is_err() {
                break;
            }
        }
    });

    IpcChannels {
        tx: req_tx,
        rx: event_rx,
    }
}

/// The agent id a one-shot CLI runs as, from the raw `NEBULA_AGENT_ID`
/// value. Unset and empty are the same miss, and the error names the
/// `nebula <verb>` that needs it. Pure so the message is testable.
fn agent_id_from(value: Option<String>, verb: &str) -> Result<String> {
    value.filter(|v| !v.is_empty()).with_context(|| {
        format!(
            "{} is not set — `nebula {verb}` only works from inside a \
             nebula agent session",
            env::AGENT_ID
        )
    })
}

/// [`agent_id_from`] over the live environment.
fn current_agent_id(verb: &str) -> Result<String> {
    agent_id_from(env::non_empty(env::AGENT_ID), verb)
}

/// What the daemon said back to a one-shot request.
enum Reply {
    Ack,
    /// The daemon's own message for a request it declined.
    Error(String),
}

/// Read events until the daemon answers `req_id` — an Ack or an Error —
/// skipping anything else it broadcasts meanwhile. Only the hang-up is an
/// `Err`; the daemon's refusal comes back as a [`Reply::Error`] so each
/// caller can decide whether that is a failure.
async fn await_reply(conn: &mut Connection, req_id: u64) -> Result<Reply> {
    loop {
        match read_frame::<ServerEvent, _>(&mut conn.stream).await? {
            Some(ServerEvent::Ack { req_id: r, .. }) if r == req_id => return Ok(Reply::Ack),
            Some(ServerEvent::Error {
                req_id: Some(r),
                message,
            }) if r == req_id => return Ok(Reply::Error(message)),
            Some(_) => continue,
            None => bail!("{CLOSED_BEFORE_REPLY}"),
        }
    }
}

/// [`await_reply`] for the callers where the daemon declining *is* the
/// failure: its message becomes the error.
async fn await_ack(conn: &mut Connection, req_id: u64) -> Result<()> {
    match await_reply(conn, req_id).await? {
        Reply::Ack => Ok(()),
        Reply::Error(message) => bail!("{message}"),
    }
}

/// How `nebula rename` treats a session that already carries a title.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RenameMode {
    /// Title only an untitled session; the daemon declines otherwise.
    Auto,
    /// Overwrite whatever title the session has (`--force`).
    Force,
}

/// One-shot client for `nebula rename`, run from inside an agent session's
/// CLI: resolve the agent from NEBULA_AGENT_ID and ask the daemon to title
/// it. Never spawns a daemon — no daemon means no session worth titling.
///
/// Daemon-reported outcomes (renamed, or "already titled" on the non-force
/// path) both print and exit 0: for the model running this, a declined
/// auto-title is a settled answer, not a failure to retry.
pub async fn rename_current_agent(title: &str, mode: RenameMode) -> Result<()> {
    let agent_id = current_agent_id("rename")?;
    let sock = paths::socket_path();
    let Ok(stream) = try_connect(&sock).await else {
        bail!("no nebula daemon is running — title unchanged");
    };
    let mut conn = handshake(stream).await?;
    let req_id = ONE_SHOT_REQ_ID;
    let id = AgentId(agent_id);
    let name = title.to_string();
    let request = match mode {
        RenameMode::Force => ClientRequest::RenameAgent { req_id, id, name },
        RenameMode::Auto => ClientRequest::AutoRenameAgent { req_id, id, name },
    };
    write_frame(&mut conn.stream, &request).await?;
    match await_reply(&mut conn, req_id).await? {
        Reply::Ack => println!("session renamed to \"{title}\""),
        Reply::Error(message) => println!("nebula: {message}"),
    }
    Ok(())
}

/// CLI: `nebula worktree [name] [--base <ref>]` from inside an agent
/// session — take (or create) the named worktree of this session's project
/// and have the daemon move the session into it. A blank name is invented
/// the way the TUI's new-worktree prompt invents one, and spaces slugify to
/// hyphens the same way. Never spawns a daemon: no daemon means no session
/// to move.
///
/// What this prints is read by the model that ran it, so the relocating
/// case spells out what happens next: the daemon kills and resumes this
/// very process once the turn ends, and the answer has to be finished by
/// then. A daemon-side failure is a nonzero exit — the model reports it and
/// stays put.
pub async fn enter_worktree_for_current_agent(name: &str, base: Option<String>) -> Result<()> {
    let agent_id = current_agent_id("worktree")?;
    let branch = match crate::branch_name::slugify(name) {
        slug if slug.is_empty() => crate::branch_name::random_name(&[]),
        slug => slug,
    };
    let sock = paths::socket_path();
    let Ok(stream) = try_connect(&sock).await else {
        bail!("no nebula daemon is running — nothing to move");
    };
    let mut conn = handshake(stream).await?;
    let req_id = ONE_SHOT_REQ_ID;
    write_frame(
        &mut conn.stream,
        &ClientRequest::EnterWorktree {
            req_id,
            id: AgentId(agent_id),
            branch,
            base,
        },
    )
    .await?;
    loop {
        match read_frame::<ServerEvent, _>(&mut conn.stream).await? {
            Some(ServerEvent::WorktreeEntered {
                req_id: r,
                worktree,
                outcome,
            }) if r == req_id => {
                println!(
                    "worktree \"{}\" is ready at {}",
                    worktree.branch,
                    worktree.path.display()
                );
                match outcome {
                    EnterOutcome::AlreadyThere => {
                        println!("this session already runs inside it — nothing to move.");
                    }
                    EnterOutcome::Relocating => {
                        println!(
                            "this session is now associated with it; nebula will relocate the \
                             session into it the moment this turn ends."
                        );
                        println!(
                            "Finish now: tell the user in one line that the session is moving \
                             into the worktree, and make no further tool calls or edits — you \
                             will be resumed inside the worktree with a prompt to continue."
                        );
                    }
                    EnterOutcome::NextLaunch => {
                        println!(
                            "this session is now associated with it and runs there from its \
                             next launch."
                        );
                    }
                }
                return Ok(());
            }
            Some(ServerEvent::Error {
                req_id: Some(r),
                message,
            }) if r == req_id => bail!("{message}"),
            Some(_) => continue,
            None => bail!("{CLOSED_BEFORE_REPLY}"),
        }
    }
}

/// One-shot client for `nebula add <dir>` (and bare `nebula <dir>`): resolve
/// the path locally — the daemon's cwd is not ours, so relative paths must be
/// absolutized here — and ask the daemon to register it as a project. The
/// daemon owns the rest: normalizing to the repo toplevel, naming the project
/// after the directory, rejecting non-repos and duplicates. Spawns a daemon
/// when none is running, same as launching the TUI would.
pub async fn add_project(path: &str) -> Result<()> {
    let expanded = match (path.strip_prefix("~/"), env::home_dir()) {
        (Some(rest), Some(home)) => home.join(rest),
        _ => std::path::PathBuf::from(path),
    };
    let dir = std::fs::canonicalize(&expanded)
        .with_context(|| format!("{} does not exist", expanded.display()))?;
    if !dir.is_dir() {
        bail!("{} is not a directory", dir.display());
    }
    let mut conn = connect_or_spawn().await?;
    let req_id = ONE_SHOT_REQ_ID;
    write_frame(
        &mut conn.stream,
        &ClientRequest::AddProject {
            req_id,
            path: dir.clone(),
            name: None,
            create_missing: false,
        },
    )
    .await?;
    await_ack(&mut conn, req_id).await?;
    println!("added project {}", dir.display());
    Ok(())
}

/// One `nebula workspace <op>` invocation, resolved and executed against the
/// daemon (spawned when absent, same as `nebula add`).
#[derive(Debug, Clone)]
pub enum WorkspaceOp {
    Add { name: String },
    Open { name: String },
    List,
    Delete { name: String },
    Rename { name: String, new_name: String },
}

/// One-shot client for `nebula workspace …`. Name→id resolution runs off a
/// snapshot (Subscribe's first reply), so the daemon's RPC surface stays
/// id-based for the TUI's picker.
pub async fn run_workspace_op(op: WorkspaceOp) -> Result<()> {
    use nebula_core::{Workspace, WorkspaceId};
    let mut conn = connect_or_spawn().await?;
    write_frame(&mut conn.stream, &ClientRequest::Subscribe).await?;
    let (workspaces, active, projects) = loop {
        match read_frame::<ServerEvent, _>(&mut conn.stream).await? {
            Some(ServerEvent::Snapshot {
                workspaces,
                active_workspace,
                projects,
                ..
            }) => break (workspaces, active_workspace, projects),
            Some(_) => continue,
            None => bail!("daemon closed the connection before sending a snapshot"),
        }
    };
    let resolve = |name: &str| -> Result<WorkspaceId> {
        workspaces
            .iter()
            .find(|w: &&Workspace| w.name == name)
            .map(|w| w.id.clone())
            .with_context(|| {
                let names: Vec<&str> = workspaces.iter().map(|w| w.name.as_str()).collect();
                format!(
                    "no workspace named '{name}' (available: {})",
                    names.join(", ")
                )
            })
    };
    let req_id = ONE_SHOT_REQ_ID;
    let (request, done): (ClientRequest, String) = match op {
        WorkspaceOp::List => {
            for w in &workspaces {
                let marker = if w.id == active { "*" } else { " " };
                let count = projects.iter().filter(|p| p.workspace_id == w.id).count();
                println!(
                    "{marker} {}  ({count} project{})",
                    w.name,
                    if count == 1 { "" } else { "s" }
                );
            }
            return Ok(());
        }
        WorkspaceOp::Add { name } => (
            ClientRequest::AddWorkspace {
                req_id,
                name: name.clone(),
            },
            format!("workspace '{name}' added — open it with `nebula workspace open {name}`"),
        ),
        WorkspaceOp::Open { name } => (
            ClientRequest::OpenWorkspace {
                req_id,
                id: resolve(&name)?,
            },
            // Running instances keep the workspace their user put them on;
            // this sets where the next one starts. `nebula --workspace
            // <name>` is the way to aim one instance without moving this.
            format!("workspace '{name}' will open in new nebula instances"),
        ),
        WorkspaceOp::Delete { name } => (
            ClientRequest::RemoveWorkspace {
                req_id,
                id: resolve(&name)?,
            },
            format!("workspace '{name}' deleted"),
        ),
        WorkspaceOp::Rename { name, new_name } => (
            ClientRequest::RenameWorkspace {
                req_id,
                id: resolve(&name)?,
                name: new_name.clone(),
            },
            format!("workspace '{name}' renamed to '{new_name}'"),
        ),
    };
    write_frame(&mut conn.stream, &request).await?;
    await_ack(&mut conn, req_id).await?;
    println!("{done}");
    Ok(())
}

/// Ask a running daemon to shut down. Ok(false) when none is running.
///
/// A daemon on a different protocol version closes the socket right after
/// the handshake, so `Shutdown` can never reach it — exactly the situation
/// `nebula kill` exists to fix. Fall back to SIGTERM via the pidfile, guarded
/// by the daemon's flock so a stale pid is never signalled.
pub async fn kill_daemon() -> Result<bool> {
    let sock = paths::socket_path();
    if let Ok(stream) = try_connect(&sock).await {
        if let Ok(mut conn) = handshake(stream).await {
            write_frame(&mut conn.stream, &ClientRequest::Shutdown).await?;
            wait_for_daemon_exit().await;
            return Ok(true);
        }
        return kill_by_pidfile().await;
    }
    // Nothing listening — but a wedged or mid-boot daemon may still hold the
    // pidfile lock; fall through to the same check.
    kill_by_pidfile().await
}

/// Outcome of `shutdown_if_idle`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdleShutdown {
    /// Nothing listening on the socket.
    NoDaemon,
    /// The daemon held no live PTYs and was shut down cleanly.
    ShutDown,
    /// Live sessions exist; the daemon was left running.
    SessionsLive { count: usize },
    /// A daemon is listening but its protocol version differs, so its
    /// session state can't be inspected.
    Skewed,
}

/// Shut the daemon down only when it holds no live PTYs — the post-upgrade
/// handoff. An idle daemon can die safely (the next client launch spawns one
/// from the new binary on disk); live sessions would be killed with it, so
/// their daemon is left alone and the restart stays the user's call.
pub async fn shutdown_if_idle() -> Result<IdleShutdown> {
    let sock = paths::socket_path();
    let Ok(stream) = try_connect(&sock).await else {
        return Ok(IdleShutdown::NoDaemon);
    };
    let Ok(mut conn) = handshake(stream).await else {
        return Ok(IdleShutdown::Skewed);
    };
    write_frame(&mut conn.stream, &ClientRequest::Subscribe).await?;
    loop {
        match read_frame::<ServerEvent, _>(&mut conn.stream).await? {
            Some(ServerEvent::Snapshot {
                agents, terminals, ..
            }) => {
                let live = agents.iter().filter(|a| a.alive).count()
                    + terminals.iter().filter(|t| t.alive).count();
                if live > 0 {
                    return Ok(IdleShutdown::SessionsLive { count: live });
                }
                write_frame(&mut conn.stream, &ClientRequest::Shutdown).await?;
                wait_for_daemon_exit().await;
                return Ok(IdleShutdown::ShutDown);
            }
            Some(_) => continue,
            None => bail!("daemon closed the connection before sending a snapshot"),
        }
    }
}

/// SIGTERM the daemon recorded in the pidfile (its SIGTERM handler runs the
/// same clean shutdown as `Shutdown`). Ok(false) when no daemon is alive.
async fn kill_by_pidfile() -> Result<bool> {
    let path = paths::pidfile_path();
    if !daemon_holds_pidfile_lock(&path) {
        return Ok(false);
    }
    let pid: i32 = std::fs::read_to_string(&path)
        .ok()
        .and_then(|s| s.trim().parse().ok())
        .filter(|pid| *pid > 0)
        .context("daemon is running but its pidfile is unreadable — kill it manually")?;
    if send_sigterm(pid) != 0 {
        bail!("failed to signal daemon pid {pid} — kill it manually");
    }
    wait_for_daemon_exit().await;
    Ok(true)
}

/// Liveness = flock possession (mirrors the daemon's PidfileLock): if we can
/// take the lock ourselves, nobody holds it. Released on drop.
fn daemon_holds_pidfile_lock(path: &std::path::Path) -> bool {
    use std::os::fd::AsRawFd;
    let Ok(file) = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)
    else {
        return false;
    };
    flock_try_exclusive(file.as_raw_fd()) != 0
}

/// Poll until the daemon releases its pidfile lock, so a relaunch right after
/// `nebula kill` can't race the old daemon's teardown.
async fn wait_for_daemon_exit() {
    let path = paths::pidfile_path();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while daemon_holds_pidfile_lock(&path) && tokio::time::Instant::now() < deadline {
        tokio::time::sleep(POLL_STEP).await;
    }
}

// Tiny extern shims, same dep-light idiom as nebula_core::paths.
fn flock_try_exclusive(fd: i32) -> i32 {
    extern "C" {
        fn flock(fd: i32, operation: i32) -> i32;
    }
    const LOCK_EX: i32 = 2;
    const LOCK_NB: i32 = 4;
    unsafe { flock(fd, LOCK_EX | LOCK_NB) }
}

fn send_sigterm(pid: i32) -> i32 {
    extern "C" {
        fn kill(pid: i32, sig: i32) -> i32;
    }
    const SIGTERM: i32 = 15;
    unsafe { kill(pid, SIGTERM) }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Unset and empty are the same miss, and the error has to name the
    // command the model just ran so it knows why it can't work here.
    #[test]
    fn agent_id_requires_a_non_empty_value_and_names_the_verb() {
        let err = agent_id_from(None, "rename").unwrap_err().to_string();
        assert!(err.contains("`nebula rename`"), "{err}");
        assert!(err.contains(env::AGENT_ID), "{err}");
        let err = agent_id_from(Some(String::new()), "worktree")
            .unwrap_err()
            .to_string();
        assert!(err.contains("`nebula worktree`"), "{err}");
        assert_eq!(agent_id_from(Some("a1".into()), "rename").unwrap(), "a1");
    }

    // The whole point of the message: `nebula kill` is the fix for exactly
    // one of the two skews, and recommending it for the other sends the user
    // in a circle (kill the daemon, the live TUI respawns the same one).
    #[test]
    fn skew_message_blames_the_older_side() {
        let daemon_ahead = version_skew_message(PROTOCOL_VERSION + 2);
        assert!(daemon_ahead.contains("This client is the older build"));
        assert!(
            !daemon_ahead.contains("run `nebula kill` and relaunch"),
            "must not send the user to kill a daemon that is not the stale side: {daemon_ahead}"
        );
        assert!(daemon_ahead.contains("make install"));

        let daemon_behind = version_skew_message(PROTOCOL_VERSION - 1);
        assert!(daemon_behind.contains("The daemon is the older build"));
        assert!(daemon_behind.contains("run `nebula kill` and relaunch"));
    }

    #[test]
    fn skew_message_names_both_binaries() {
        let msg = version_skew_message(PROTOCOL_VERSION + 1);
        assert!(msg.contains("this client: "), "{msg}");
        assert!(msg.contains("the daemon:  "), "{msg}");
        // current_exe resolves under a test binary, so this half is never
        // the "unknown" fallback.
        assert!(msg.contains(&format!("v{PROTOCOL_VERSION}")), "{msg}");
    }
}
