//! IN-PLACE RESTART: move a running daemon onto a new binary without
//! stopping a single session.
//!
//! The daemon `exec`s the new binary over itself. The pid stays the same,
//! so every agent CLI is still this process's child and is reaped as
//! before, and the fds it chose to keep open survive the exec: each PTY
//! master, the client socket, the hook receiver's socket (every live agent
//! has its port and token in its environment) and the pidfile with its
//! flock. What lives only in memory — each session's scrollback ring and
//! the terminal modes read off it — is written to [`paths::restart_state_path`]
//! for the new image to read back as it boots.
//!
//! A client asks by writing [`Request`] to [`paths::restart_request_path`]
//! and sending the daemon SIGWINCH, then waits for an [`Outcome`] at
//! [`paths::restart_result_path`]: the old image writes one when it refuses,
//! the new image when it has taken the sessions over. The request travels
//! outside the socket protocol on purpose: the client is usually the newer
//! build, which a daemon on an older protocol would refuse to talk to.

use crate::lifecycle::PidfileLock;
use crate::pty::Carried;
use crate::registry::Daemon;
use anyhow::{bail, Context, Result};
use nebula_core::{paths, AgentId, Worktree};
use serde::{Deserialize, Serialize};
use std::os::fd::{FromRawFd, RawFd};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// The state-file format this build writes, and the newest it reads. A
/// daemon only restarts onto a binary whose `_restart-version` is at least
/// its own, so a change that an older reader could misread bumps this.
pub const VERSION: u32 = 1;

/// A request older than this is a leftover, not a client still waiting.
const REQUEST_MAX_AGE_MS: i64 = 60_000;
/// How long a client waits for the restart to be done.
const OUTCOME_TIMEOUT: Duration = Duration::from_secs(20);
const POLL_STEP: Duration = Duration::from_millis(100);

#[derive(Debug, Serialize, Deserialize)]
pub struct Request {
    pub nonce: String,
    /// The binary to restart onto; absolute.
    pub exe: PathBuf,
    pub requested_at_ms: i64,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Outcome {
    pub nonce: String,
    pub ok: bool,
    /// Sessions running in the new image.
    #[serde(default)]
    pub sessions: usize,
    #[serde(default)]
    pub message: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct CarriedSession {
    pub pty: Carried,
    /// A PREWARM POOL spare: reaped by the new image, not kept.
    #[serde(default)]
    pub spare: bool,
    /// An agent whose status machine still held its launch reprieve
    /// (`AgentStatusMachine::launching`).
    #[serde(default)]
    pub launching: bool,
}

#[derive(Serialize, Deserialize)]
struct State {
    version: u32,
    nonce: String,
    pidfile_fd: RawFd,
    socket_fd: RawFd,
    hook_fd: RawFd,
    hook_token: String,
    #[serde(default)]
    sessions: Vec<CarriedSession>,
    #[serde(default)]
    pending_moves: Vec<(AgentId, Worktree)>,
}

/// The running image's handles a restart passes on.
pub struct Handles {
    pub lock: Arc<Mutex<PidfileLock>>,
    pub socket_fd: RawFd,
    pub hook_fd: RawFd,
    /// `nebula daemon --foreground`: the new image stays in the foreground.
    pub foreground: bool,
}

/// What the new image takes over from the old one as it boots.
pub struct Inherited {
    pub lock: PidfileLock,
    pub listener: std::os::unix::net::UnixListener,
    pub hooks: crate::hooks::InheritedHooks,
    pub carry: Carry,
}

/// The sessions and relocations to re-install once the daemon is built.
pub struct Carry {
    pub nonce: String,
    pub sessions: Vec<CarriedSession>,
    pub pending_moves: Vec<(AgentId, Worktree)>,
}

impl Carry {
    /// The agents whose CLIs are still running, for the boot sweep to spare.
    pub fn agents(&self) -> std::collections::HashSet<AgentId> {
        self.sessions
            .iter()
            .filter(|s| !s.spare)
            .filter_map(|s| match &s.pty.sref {
                nebula_core::SessionRef::Agent(id) => Some(id.clone()),
                nebula_core::SessionRef::Terminal(_) => None,
            })
            .collect()
    }
}

// ---- daemon side ----

/// Tell clients this daemon takes restart requests. Rewritten on the
/// pidfile's refresh tick, for the same /tmp cleaner.
pub fn advertise() {
    let _ = std::fs::write(
        paths::restart_capability_path(),
        format!("{} {VERSION}\n", std::process::id()),
    );
}

/// The pending request, removed so it is answered once. None when there is
/// none, it can't be read, or it is too old to have a client waiting.
pub fn take_request() -> Option<Request> {
    let path = paths::restart_request_path();
    let text = std::fs::read_to_string(&path).ok()?;
    let _ = std::fs::remove_file(&path);
    let request: Request = serde_json::from_str(&text).ok()?;
    let age = nebula_core::clock::now_ms() - request.requested_at_ms;
    (age <= REQUEST_MAX_AGE_MS).then_some(request)
}

/// Exec `request.exe` over this process, handing it every live session.
/// Only returns on failure, with this image still running and every
/// session still its own.
pub fn restart(daemon: &Arc<Daemon>, handles: &Handles, request: &Request) -> Result<()> {
    use std::os::unix::process::CommandExt;
    let exe = &request.exe;
    let meta = std::fs::metadata(exe).with_context(|| format!("{}", exe.display()))?;
    if !exe.is_absolute() || !meta.is_file() {
        bail!("{} is not a nebula binary", exe.display());
    }
    match probe_version(exe) {
        Some(v) if v >= VERSION => {}
        _ => bail!(
            "{} can't take over running sessions — it predates in-place restarts",
            exe.display()
        ),
    }

    // Nothing spawns, and nothing writes the database, from here to the
    // exec: a child forked now would inherit the masters left open for it.
    let _spawns = daemon.hold_spawns();
    let _db = daemon.store.quiesce();
    let lock = handles.lock.lock().unwrap();
    let sessions = daemon.carry_sessions()?;
    let live = sessions.iter().filter(|s| !s.spare).count();
    let state = State {
        version: VERSION,
        nonce: request.nonce.clone(),
        pidfile_fd: lock.raw_fd(),
        socket_fd: handles.socket_fd,
        hook_fd: handles.hook_fd,
        hook_token: daemon.hook_env.token.clone(),
        sessions,
        pending_moves: daemon.pending_moves(),
    };
    let state_path = paths::restart_state_path();
    let kept = [state.pidfile_fd, state.socket_fd, state.hook_fd];
    let prepared = rmp_serde::to_vec_named(&state)
        .context("encode restart state")
        .and_then(|bytes| {
            std::fs::write(&state_path, bytes)
                .with_context(|| format!("write {}", state_path.display()))
        })
        .and_then(|()| {
            kept.iter()
                .try_for_each(|&fd| crate::pty::set_cloexec(fd, false))
        });
    let err = match prepared {
        Ok(()) => {
            tracing::info!(exe = %exe.display(), sessions = live, "restarting in place");
            let mut args = vec!["daemon".to_string()];
            if handles.foreground {
                args.push("--foreground".into());
            }
            args.push("--adopt".into());
            args.push(state_path.display().to_string());
            anyhow::Error::new(std::process::Command::new(exe).args(&args).exec())
                .context(format!("exec {}", exe.display()))
        }
        Err(e) => e,
    };
    for fd in kept {
        let _ = crate::pty::set_cloexec(fd, true);
    }
    daemon.uncarry_sessions();
    let _ = std::fs::remove_file(&state_path);
    Err(err)
}

/// The newest restart-state version `exe` reads. Run from the runtime dir:
/// a build from before the hook reads the word as `nebula <dir>`, and no
/// directory by that name will ever sit there to be added as a project.
fn probe_version(exe: &Path) -> Option<u32> {
    let out = std::process::Command::new(exe)
        .arg("_restart-version")
        .current_dir(paths::runtime_dir())
        .stdin(std::process::Stdio::null())
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    String::from_utf8_lossy(&out.stdout).trim().parse().ok()
}

/// Read back what the old image wrote, and take ownership of the fds it
/// kept open. Every fd taken here is close-on-exec again, ready for the
/// next restart to choose what it keeps.
pub fn inherit(state_path: &Path) -> Result<Inherited> {
    let bytes =
        std::fs::read(state_path).with_context(|| format!("read {}", state_path.display()))?;
    let _ = std::fs::remove_file(state_path);
    let state: State = rmp_serde::from_slice(&bytes).context("decode restart state")?;
    if state.version > VERSION {
        bail!(
            "restart state v{} is newer than this build reads (v{VERSION})",
            state.version
        );
    }
    for fd in [state.pidfile_fd, state.socket_fd, state.hook_fd] {
        crate::pty::set_cloexec(fd, true)?;
    }
    // SAFETY: the old image named these fds and kept them open across the
    // exec for exactly this; nothing else in this process owns them.
    let (lock, listener, hook_listener) = unsafe {
        (
            PidfileLock::inherit(state.pidfile_fd),
            std::os::unix::net::UnixListener::from_raw_fd(state.socket_fd),
            std::net::TcpListener::from_raw_fd(state.hook_fd),
        )
    };
    Ok(Inherited {
        lock,
        listener,
        hooks: crate::hooks::InheritedHooks {
            listener: hook_listener,
            token: state.hook_token,
        },
        carry: Carry {
            nonce: state.nonce,
            sessions: state.sessions,
            pending_moves: state.pending_moves,
        },
    })
}

/// Answer the client waiting on `nonce`.
pub fn report(nonce: &str, result: std::result::Result<usize, String>) {
    let outcome = match result {
        Ok(sessions) => Outcome {
            nonce: nonce.to_string(),
            ok: true,
            sessions,
            message: String::new(),
        },
        Err(message) => Outcome {
            nonce: nonce.to_string(),
            ok: false,
            sessions: 0,
            message,
        },
    };
    if let Ok(json) = serde_json::to_string(&outcome) {
        let _ = write_atomic(&paths::restart_result_path(), json.as_bytes());
    }
}

fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let tmp = path.with_extension(format!("tmp-{}", std::process::id()));
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path)
}

// ---- client side ----

/// How a restart request went, short of an error.
#[derive(Debug, PartialEq, Eq)]
pub enum Restart {
    /// No daemon is running; the next launch starts the new binary anyway.
    NoDaemon,
    /// The running daemon predates in-place restarts.
    Unsupported,
    /// The daemon now runs `exe`, with this many sessions still live.
    Restarted { sessions: usize },
}

/// Ask the running daemon to restart in place onto `exe`, and wait until
/// it has (or has said why not).
pub fn request_restart(exe: &Path) -> Result<Restart> {
    if !PidfileLock::is_daemon_alive() {
        return Ok(Restart::NoDaemon);
    }
    let Some(pid) = restartable_daemon_pid() else {
        return Ok(Restart::Unsupported);
    };
    let exe = std::fs::canonicalize(exe).with_context(|| format!("{}", exe.display()))?;
    let request = Request {
        nonce: nonce(),
        exe,
        requested_at_ms: nebula_core::clock::now_ms(),
    };
    let result_path = paths::restart_result_path();
    let _ = std::fs::remove_file(&result_path);
    write_atomic(
        &paths::restart_request_path(),
        serde_json::to_string(&request)?.as_bytes(),
    )
    .context("write restart request")?;
    nix::sys::signal::kill(
        nix::unistd::Pid::from_raw(pid),
        nix::sys::signal::Signal::SIGWINCH,
    )
    .with_context(|| format!("signal daemon pid {pid}"))?;

    let deadline = std::time::Instant::now() + OUTCOME_TIMEOUT;
    while std::time::Instant::now() < deadline {
        std::thread::sleep(POLL_STEP);
        let Some(outcome) = std::fs::read_to_string(&result_path)
            .ok()
            .and_then(|text| serde_json::from_str::<Outcome>(&text).ok())
            .filter(|o| o.nonce == request.nonce)
        else {
            continue;
        };
        let _ = std::fs::remove_file(&result_path);
        if outcome.ok {
            return Ok(Restart::Restarted {
                sessions: outcome.sessions,
            });
        }
        bail!("{}", outcome.message);
    }
    let _ = std::fs::remove_file(paths::restart_request_path());
    bail!(
        "the daemon did not finish restarting within {}s — see {}",
        OUTCOME_TIMEOUT.as_secs(),
        paths::daemon_log_path().display()
    )
}

/// Whether the running daemon takes IN-PLACE RESTART requests.
pub fn daemon_can_restart() -> bool {
    PidfileLock::is_daemon_alive() && restartable_daemon_pid().is_some()
}

/// The pid of the running daemon when it advertises restarts, and the
/// advertisement is its own — not one a dead daemon left behind for an
/// older one now holding the pidfile.
fn restartable_daemon_pid() -> Option<i32> {
    let advertised = std::fs::read_to_string(paths::restart_capability_path()).ok()?;
    let mut words = advertised.split_whitespace();
    let pid: i32 = words.next()?.parse().ok()?;
    let version: u32 = words.next()?.parse().ok()?;
    let holder: i32 = std::fs::read_to_string(paths::pidfile_path())
        .ok()?
        .trim()
        .parse()
        .ok()?;
    (pid > 1 && pid == holder && version >= 1).then_some(pid)
}

fn nonce() -> String {
    use rand::Rng;
    let mut bytes = [0u8; 16];
    rand::rng().fill_bytes(&mut bytes);
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_round_trips_with_its_rings() {
        let state = State {
            version: VERSION,
            nonce: "n".into(),
            pidfile_fd: 3,
            socket_fd: 4,
            hook_fd: 5,
            hook_token: "tok".into(),
            sessions: vec![CarriedSession {
                pty: Carried {
                    sref: nebula_core::SessionRef::Agent(AgentId("a".into())),
                    master_fd: 9,
                    pid: 1234,
                    size: (120, 40),
                    ring_start_seq: 77,
                    ring: vec![0, 1, 2, 255],
                    kitty_stack: vec![1],
                    bracketed_paste: true,
                    progress_busy: Some(true),
                    title: Some("✳ Fix Login".into()),
                    cloud_scan: true,
                },
                spare: false,
                launching: true,
            }],
            pending_moves: vec![],
        };
        let bytes = rmp_serde::to_vec_named(&state).unwrap();
        let back: State = rmp_serde::from_slice(&bytes).unwrap();
        assert_eq!(back.hook_token, "tok");
        let pty = &back.sessions[0].pty;
        assert_eq!((pty.master_fd, pty.pid, pty.size), (9, 1234, (120, 40)));
        assert_eq!(
            (pty.ring_start_seq, pty.ring.as_slice()),
            (77, &[0, 1, 2, 255][..])
        );
        assert_eq!(pty.title.as_deref(), Some("✳ Fix Login"));
        assert!(pty.bracketed_paste);
        assert!(pty.cloud_scan && back.sessions[0].launching);
    }

    #[test]
    fn carry_names_only_the_agents_still_running() {
        let pty = |sref| Carried {
            sref,
            master_fd: 0,
            pid: 1,
            size: (80, 24),
            ring_start_seq: 0,
            ring: vec![],
            kitty_stack: vec![],
            bracketed_paste: false,
            progress_busy: None,
            title: None,
            cloud_scan: false,
        };
        let carry = Carry {
            nonce: String::new(),
            sessions: vec![
                CarriedSession {
                    pty: pty(nebula_core::SessionRef::Agent(AgentId("kept".into()))),
                    spare: false,
                    launching: false,
                },
                CarriedSession {
                    pty: pty(nebula_core::SessionRef::Agent(AgentId("spare".into()))),
                    spare: true,
                    launching: false,
                },
                CarriedSession {
                    pty: pty(nebula_core::SessionRef::Terminal(nebula_core::TerminalId(
                        "t".into(),
                    ))),
                    spare: false,
                    launching: false,
                },
            ],
            pending_moves: vec![],
        };
        assert_eq!(
            carry.agents(),
            [AgentId("kept".into())].into_iter().collect()
        );
    }
}
