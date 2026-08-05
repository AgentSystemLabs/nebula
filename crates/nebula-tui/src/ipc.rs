//! Client-side IPC: connect to the daemon (auto-spawning it when absent) and
//! perform the version handshake.

use anyhow::{bail, Context, Result};
use nebula_core::codec::{read_frame, write_frame};
use nebula_core::{paths, ClientRequest, ServerEvent, PROTOCOL_VERSION};
use std::time::Duration;
use tokio::net::UnixStream;

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
                tokio::time::sleep(Duration::from_millis(50)).await;
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
    std::process::Command::new(exe)
        .arg("daemon")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        // New process group so the daemon outlives this client and never
        // receives the TUI's terminal signals (Ctrl+C etc.).
        .process_group(0)
        .spawn()
        .context("spawn nebula daemon")?;
    Ok(())
}

async fn handshake(mut stream: UnixStream) -> Result<Connection> {
    write_frame(&mut stream, &ClientRequest::Hello { protocol_version: PROTOCOL_VERSION }).await?;
    match read_frame::<ServerEvent, _>(&mut stream).await? {
        Some(ServerEvent::HelloOk { daemon_pid, .. }) => Ok(Connection { stream, daemon_pid }),
        Some(ServerEvent::Incompatible { daemon_protocol_version }) => bail!(
            "daemon speaks protocol v{daemon_protocol_version}, this client v{PROTOCOL_VERSION} — \
             run `nebula kill-server` and relaunch"
        ),
        other => bail!("unexpected handshake reply: {other:?}"),
    }
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

    IpcChannels { tx: req_tx, rx: event_rx }
}

/// Ask a running daemon to shut down. Ok(false) when none is running.
///
/// A daemon on a different protocol version closes the socket right after
/// the handshake, so `Shutdown` can never reach it — exactly the situation
/// kill-server exists to fix. Fall back to SIGTERM via the pidfile, guarded
/// by the daemon's flock so a stale pid is never signalled.
pub async fn kill_server() -> Result<bool> {
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
    let Ok(file) = std::fs::OpenOptions::new().read(true).write(true).open(path) else {
        return false;
    };
    flock_try_exclusive(file.as_raw_fd()) != 0
}

/// Poll until the daemon releases its pidfile lock, so a relaunch right after
/// kill-server can't race the old daemon's teardown.
async fn wait_for_daemon_exit() {
    let path = paths::pidfile_path();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while daemon_holds_pidfile_lock(&path) && tokio::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(50)).await;
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
