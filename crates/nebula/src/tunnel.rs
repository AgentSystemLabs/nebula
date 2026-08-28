//! `nebula tunnel HOST [PATH] [--port N] [--remote-port N]`: run the remote
//! machine's nebula in a browser tab here, with nothing exposed on the
//! remote's network.
//!
//! One `ssh -t -L local:127.0.0.1:remote HOST` does all of it. The remote
//! command is [`crate::ssh`]'s self-installing prelude with a different tail:
//! `nebula browser --no-open --port <remote>`, which stays on the remote's
//! loopback — the ssh channel is the only way in, so there is no port on the
//! box for anyone else to find and no ttyd password to set. `--no-open`
//! because the desktop that should open the tab is this one.
//!
//! If the remote already has a ttyd on that port — a `nebula browser` the
//! user left running there, say — the remote command reuses it instead of
//! failing on the port clash: it holds the session open so the forward has
//! something to reach, and starts nothing. See [`REMOTE_SCRIPT`].
//!
//! We spawn ssh rather than exec it (unlike `nebula ssh`) because there is
//! work left after the connection: wait for the far end to answer through the
//! forward, then open the local URL. The pty keeps the lifetime honest in
//! both directions — Ctrl+C here reaches the remote nebula and ttyd, and
//! losing the connection hangs up the remote side.

use anyhow::{anyhow, bail, Context, Result};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpStream};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use crate::browser;
use crate::ssh::{install_prelude, shell_single_quote};

/// Both ends of the tunnel are loopback: the local listener ssh binds, and
/// the address on the remote that ssh connects the other end to.
const LOOPBACK: IpAddr = IpAddr::V4(Ipv4Addr::LOCALHOST);

/// Generous, because a first connection to a bare box downloads and installs
/// a nebula release before ttyd is even spawned.
const STARTUP_TIMEOUT: Duration = Duration::from_secs(120);
/// Slower than `nebula browser`'s poll on purpose: until the far end is up,
/// every probe is a forwarded channel the remote refuses, and OpenSSH logs a
/// line for each one. See [`forward_ssh_stderr`].
const POLL_INTERVAL: Duration = Duration::from_millis(500);
const PROBE_TIMEOUT: Duration = Duration::from_secs(2);

/// The reuse branch of [`REMOTE_SCRIPT`]: `$2` is the port. A macro for the
/// same reason as [`install_prelude!`] — `concat!` takes only literals.
/// Same quoting rules as the rest of the script.
macro_rules! reuse_existing_ttyd {
    () => {
        concat!(
            "if curl -sI --max-time 2 \"http://127.0.0.1:$2/\" 2>/dev/null | grep -qi \"^server: ttyd\"; then ",
            "echo \"nebula tunnel: a nebula browser is already serving on this host at port $2; reusing it ",
            "(if it was started with --credential, the tab will ask for that)\" >&2; ",
            "exec sleep 2147483647; ",
            "fi; "
        )
    };
}

/// Runs under `sh -c` on the remote: $1 = install URL, $2 = port to serve on,
/// $3 = start dir (optional; defaults to the remote $HOME). Same quoting
/// rules as [`crate::ssh`] — no single quotes, backslashes, or newlines.
///
/// stdout is dropped because the remote `nebula browser` addresses a user
/// sitting at *its* machine — its "serving on http://127.0.0.1:<remote>" and
/// "Ctrl+C to stop." would land under ours, naming a port that means nothing
/// here. stderr is kept: the install progress, a missing ttyd, and a port
/// clash are the whole diagnosis when this goes wrong.
///
/// Before anything is started, the port is asked whether a ttyd is already
/// on it: `curl -I` against the remote's own loopback, matched on the
/// `server: ttyd/…` header ttyd sends with every response (a 401 from one
/// behind `--credential` included). If so, this is a `nebula browser` the
/// user already has serving there — most often one launched by hand with
/// `--public` — and starting a second would only fail on the port clash. So
/// the script says so and `exec`s a long sleep instead: the session stays
/// open for the forward to reach the existing server, and Ctrl+C or a
/// hang-up ends the sleep exactly as it would have ended `nebula browser`.
/// The probe is shell-only, so a remote whose nebula predates this still
/// reuses; a remote with no `curl` skips the probe and behaves as before.
///
/// The `--help` grep is the version check. `nebula ssh` and this only install
/// nebula when the remote has *none*, so a box last touched a few releases
/// ago keeps a `nebula browser` that predates `--no-open` and would fail on
/// an unknown argument. Naming the fix beats a clap usage dump.
const REMOTE_SCRIPT: &str = concat!(
    install_prelude!(),
    reuse_existing_ttyd!(),
    "nebula browser --help 2>/dev/null | grep -q -- --no-open || { ",
    "echo \"nebula tunnel: the nebula on this host is too old to tunnel into; ",
    "reach it with nebula ssh and run nebula upgrade there\" >&2; exit 1; }; ",
    "cd -- \"${3:-$HOME}\" || exit 1; ",
    "exec nebula browser --no-open --port \"$2\" >/dev/null"
);

/// Everything `nebula tunnel` was asked for, straight off the CLI.
#[derive(Debug, Clone)]
pub struct TunnelOpts {
    /// ssh destination, passed to ssh verbatim (e.g. `user@10.0.1.7`).
    pub host: String,
    /// Remote directory the served nebula starts in.
    pub path: Option<String>,
    /// `--port`: the local end of the forward, and the port the browser
    /// opens. See [`resolve_local_port`].
    pub port: Option<u16>,
    /// `--remote-port`: what the remote's ttyd binds. Defaults to the local
    /// port, so the two numbers match unless something on the remote is
    /// already sitting on it.
    pub remote_port: Option<u16>,
}

pub fn run_tunnel(opts: TunnelOpts) -> Result<()> {
    let local = resolve_local_port(opts.port)?;
    let remote = opts.remote_port.unwrap_or(local);
    // The same list `nebula ssh` writes and the TUI's `h` picker reads — a
    // host worth tunnelling into is a host worth reconnecting to. Recorded
    // before the connection, as there, so a failed attempt still lists.
    nebula_tui::hosts::record(&opts.host, opts.path.as_deref());

    println!(
        "nebula tunnel: connecting to {} (localhost:{local} → its 127.0.0.1:{remote})",
        opts.host
    );
    let mut child = spawn_ssh(&opts, local, remote)?;
    forward_ssh_stderr(&mut child);

    let addr = SocketAddr::new(LOOPBACK, local);
    wait_until_forwarded(&mut child, addr, &opts.host)?;

    let url = format!("http://{addr}");
    if browser::open_url(&url) {
        println!("nebula tunnel: {} is serving on {url}", opts.host);
    } else {
        println!(
            "nebula tunnel: {} is serving on {url} (open it yourself — no browser launched)",
            opts.host
        );
    }
    println!("Ctrl+C to stop.");

    let status = child.wait().context("failed to wait on ssh")?;
    match status.code() {
        // 130 is the remote nebula taking this terminal's Ctrl+C through the
        // pty, which is how the user stops a tunnel — not a failure.
        Some(0) | Some(130) | None => Ok(()),
        Some(code) => bail!("ssh exited with status {code}"),
    }
}

/// The local end of the forward, settled before ssh runs so the number we
/// print, the number ssh binds, and the URL we open are all the same one.
///
/// Mirrors `nebula browser`'s rules — [`browser::DEFAULT_PORT`] when free, a
/// named port or an error, `0` for any — because from the browser's side this
/// *is* a `nebula browser`, just one whose terminal lives on another machine.
fn resolve_local_port(requested: Option<u16>) -> Result<u16> {
    match requested {
        Some(0) => browser::free_port(LOOPBACK),
        Some(n) => {
            browser::probe(n, LOOPBACK).with_context(|| format!("local port {n} is not free"))?;
            Ok(n)
        }
        None if browser::probe(browser::DEFAULT_PORT, LOOPBACK).is_ok() => {
            Ok(browser::DEFAULT_PORT)
        }
        None => {
            let port = browser::free_port(LOOPBACK)?;
            println!(
                "nebula tunnel: {} is busy here — forwarding {port} instead",
                browser::DEFAULT_PORT
            );
            Ok(port)
        }
    }
}

fn spawn_ssh(opts: &TunnelOpts, local: u16, remote: u16) -> Result<Child> {
    let cmd = remote_command(&crate::upgrade::install_url(), remote, opts.path.as_deref());
    Command::new("ssh")
        // Force a pty (`-tt`, not `-t`) so Ctrl+C here becomes SIGINT on the
        // remote nebula and its ttyd, and hanging up kills them rather than
        // leaving a ttyd serving to nobody. Forced, because `-t` allocates
        // nothing when our own stdin is not a terminal — and that is exactly
        // when nobody is watching for the orphans it leaves.
        .arg("-tt")
        // Without this, ssh logs the failed forward and connects anyway —
        // leaving a session whose local port belongs to something else.
        .args(["-o", "ExitOnForwardFailure=yes"])
        .args(["-L", &forward_spec(local, remote)])
        .args(["--", &opts.host, &cmd])
        // Read by `forward_ssh_stderr`; the remote's own output arrives on
        // stdout through the pty and is left inherited.
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => {
                anyhow!("ssh not found on PATH — nebula tunnel requires the OpenSSH client")
            }
            _ => anyhow::Error::new(e).context("failed to start ssh"),
        })
}

/// `-L` argument: listen on loopback here, connect to loopback *there*. Both
/// ends are named explicitly — a bare `local:host:remote` would have ssh
/// resolve the remote end from the remote's perspective, and the whole point
/// is that the remote ttyd is only reachable through this channel.
fn forward_spec(local: u16, remote: u16) -> String {
    format!("127.0.0.1:{local}:127.0.0.1:{remote}")
}

fn remote_command(install_url: &str, port: u16, path: Option<&str>) -> String {
    let mut cmd = format!(
        "sh -c '{}' nebula-tunnel {} {}",
        REMOTE_SCRIPT,
        shell_single_quote(install_url),
        port
    );
    if let Some(path) = path {
        cmd.push(' ');
        cmd.push_str(&shell_single_quote(path));
    }
    cmd
}

/// Print ssh's diagnostics, minus the ones the readiness poll causes.
///
/// Every probe before the remote is up opens a forwarded channel that the
/// remote refuses, and OpenSSH says so on stderr. Over a cold start that
/// installs nebula first, that is hundreds of lines burying the messages that
/// matter — so drop exactly that one, and keep the rest: a permission denial
/// or an unresolvable host is the only explanation the user gets.
fn forward_ssh_stderr(child: &mut Child) {
    let Some(stderr) = child.stderr.take() else {
        return;
    };
    std::thread::spawn(move || {
        for line in BufReader::new(stderr).lines().map_while(Result::ok) {
            if is_refused_channel(&line) {
                continue;
            }
            eprintln!("{line}");
        }
    });
}

/// `channel 3: open failed: connect failed: Connection refused` — the far end
/// of the forward isn't listening *yet*.
fn is_refused_channel(line: &str) -> bool {
    line.contains("open failed: connect failed")
}

fn wait_until_forwarded(child: &mut Child, addr: SocketAddr, host: &str) -> Result<()> {
    let deadline = Instant::now() + STARTUP_TIMEOUT;
    loop {
        // ssh is gone: auth failed, the host is unreachable, or the remote
        // command exited (no ttyd there, say). It has said why on its way out.
        if let Some(status) = child.try_wait().context("failed to poll ssh")? {
            bail!("ssh exited before the tunnel came up ({status}) — see its output above");
        }
        if answers_http(addr) {
            return Ok(());
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            bail!(
                "{host} did not start serving within {}s — connect with `nebula ssh {host}` and check that ttyd is installed there",
                STARTUP_TIMEOUT.as_secs()
            );
        }
        std::thread::sleep(POLL_INTERVAL);
    }
}

/// Whether the far end of the forward is really serving.
///
/// A connect proves nothing here: ssh accepts on the local port from the
/// moment the session is up, and only discovers the remote end is refusing
/// once it tries to open the channel. So ask a question and require an
/// answer. Any byte counts — ttyd replies 200 to this, or 401 behind a
/// credential, and both mean something is alive on the other side.
fn answers_http(addr: SocketAddr) -> bool {
    let Ok(mut sock) = TcpStream::connect_timeout(&addr, PROBE_TIMEOUT) else {
        return false;
    };
    if sock.set_read_timeout(Some(PROBE_TIMEOUT)).is_err() {
        return false;
    }
    // HTTP/1.0: no keep-alive to unwind, and the connection drops on its own.
    if sock.write_all(b"GET / HTTP/1.0\r\n\r\n").is_err() {
        return false;
    }
    let mut byte = [0u8; 1];
    matches!(sock.read(&mut byte), Ok(1))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;

    const URL: &str = "https://example.com/install.sh";

    fn opts() -> TunnelOpts {
        TunnelOpts {
            host: "user@example.com".into(),
            path: None,
            port: None,
            remote_port: None,
        }
    }

    /// The same constraint `nebula ssh`'s script lives under: it is wrapped
    /// once in '...' and handed to whatever login shell the remote uses.
    #[test]
    fn script_survives_single_quoting() {
        assert!(!REMOTE_SCRIPT.contains('\''));
        assert!(!REMOTE_SCRIPT.contains('\\'));
        assert!(!REMOTE_SCRIPT.contains('\n'));
    }

    /// The remote must not open a browser of its own — there is no desktop
    /// there, and an `xdg-open` that blocks would hang the tunnel.
    #[test]
    fn the_remote_serves_without_opening_anything() {
        let cmd = remote_command(URL, 7681, None);
        assert!(cmd.contains("nebula browser --no-open --port"), "{cmd}");
        assert!(cmd.ends_with("7681"), "{cmd}");
    }

    /// …and without narrating it: those lines are addressed to someone at the
    /// remote machine and name a port that is not the one to open here.
    /// stderr stays, because that is where the failures are.
    #[test]
    fn the_remotes_stdout_is_dropped_but_not_its_stderr() {
        // The served command ends the script, so `>/dev/null` with no `2>`
        // after it is the whole redirection: stdout gone, stderr inherited.
        assert!(
            REMOTE_SCRIPT.ends_with("--port \"$2\" >/dev/null"),
            "{REMOTE_SCRIPT}"
        );
    }

    /// The port is what ties the two halves together: ssh forwards to it and
    /// the remote ttyd must bind exactly it, so it is passed explicitly
    /// rather than left to `nebula browser`'s free-port fallback.
    #[test]
    fn the_port_reaches_the_script_as_a_parameter() {
        let cmd = remote_command(URL, 9123, None);
        assert!(cmd.contains("nebula-tunnel 'https://example.com/install.sh' 9123"));
        assert!(REMOTE_SCRIPT.contains("--port \"$2\""));
    }

    #[test]
    fn no_path_defaults_to_remote_home() {
        assert!(REMOTE_SCRIPT.contains("${3:-$HOME}"));
        assert!(remote_command(URL, 7681, None).ends_with("7681"));
    }

    #[test]
    fn a_path_is_quoted_after_the_port() {
        let cmd = remote_command(URL, 7681, Some("/srv/my repo"));
        assert!(cmd.ends_with("7681 '/srv/my repo'"), "{cmd}");
        let cmd = remote_command(URL, 7681, Some("/tmp/it's here"));
        assert!(cmd.ends_with("7681 '/tmp/it'\\''s here'"), "{cmd}");
    }

    /// A ttyd already on the remote port is a `nebula browser` the user has
    /// running there; the script keeps the session open for it rather than
    /// starting a second one into a port clash — and decides that before the
    /// version gate, since a reused server needs nothing from the remote's
    /// own nebula.
    #[test]
    fn an_existing_ttyd_on_the_remote_port_is_reused_before_anything_starts() {
        let probe = REMOTE_SCRIPT.find("curl -sI").expect("probes the port");
        let gate = REMOTE_SCRIPT.find("grep -q -- --no-open").unwrap();
        let start = REMOTE_SCRIPT.find("exec nebula browser").unwrap();
        assert!(probe < gate && gate < start, "{REMOTE_SCRIPT}");
        assert!(REMOTE_SCRIPT.contains("http://127.0.0.1:$2/"));
        assert!(REMOTE_SCRIPT.contains("grep -qi \"^server: ttyd\""));
        assert!(REMOTE_SCRIPT.contains("exec sleep "));
    }

    /// A listener that answers every request the way ttyd does — with its
    /// `server:` header — on a port of the kernel's choosing.
    fn fake_ttyd(status: &'static str) -> u16 {
        let listener = TcpListener::bind(SocketAddr::new(LOOPBACK, 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            for sock in listener.incoming().map_while(Result::ok) {
                let mut sock = sock;
                let mut buf = [0u8; 1024];
                let _ = sock.read(&mut buf);
                let _ = sock.write_all(
                    format!(
                        "HTTP/1.1 {status}\r\nserver: ttyd/1.7.7 (libwebsockets/5.0.0)\r\ncontent-length: 0\r\n\r\n"
                    )
                    .as_bytes(),
                );
            }
        });
        port
    }

    /// Run [`REMOTE_SCRIPT`] here the way sshd would there: under `sh -c`,
    /// with `$HOME` pointed at an empty dir (the prelude prepends
    /// `$HOME/.local/bin`, and the real nebula must not be found) and a
    /// stub `nebula` first on PATH that answers nothing — so the version
    /// gate, if reached, fails with the "too old" message rather than
    /// launching anything.
    fn run_remote_script(port: u16) -> (std::process::Child, tempfile::TempDir) {
        let home = tempfile::tempdir().unwrap();
        let stub = home.path().join("stub");
        std::fs::create_dir(&stub).unwrap();
        let nebula = stub.join("nebula");
        std::fs::write(&nebula, "#!/bin/sh\nexit 0\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&nebula, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let path = format!(
            "{}:/usr/bin:/bin:/usr/local/bin:/opt/homebrew/bin",
            stub.display()
        );
        let child = Command::new("sh")
            .args(["-c", REMOTE_SCRIPT, "nebula-tunnel", "file:///nonexistent"])
            .arg(port.to_string())
            .env("HOME", home.path())
            .env("PATH", path)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .expect("sh");
        (child, home)
    }

    fn first_stderr_line(child: &mut Child) -> String {
        let stderr = child.stderr.take().unwrap();
        let mut line = String::new();
        BufReader::new(stderr).read_line(&mut line).unwrap();
        line
    }

    /// The whole point: a server already on the port means "reuse it", said
    /// out loud, with the session held open — not a port clash.
    #[test]
    fn the_script_reuses_a_ttyd_that_answers_on_the_port() {
        for status in ["200 OK", "401 Unauthorized"] {
            let port = fake_ttyd(status);
            let (mut child, _home) = run_remote_script(port);
            let line = first_stderr_line(&mut child);
            assert!(
                line.contains(&format!("already serving on this host at port {port}")),
                "{status}: {line:?}"
            );
            assert!(
                child.try_wait().unwrap().is_none(),
                "{status}: the session should stay open for the forward"
            );
            let _ = child.kill();
            let _ = child.wait();
        }
    }

    /// Nothing on the port: the probe stays quiet and the script goes on to
    /// start its own — here, into the version gate the stub nebula fails.
    #[test]
    fn the_script_starts_its_own_when_nothing_answers() {
        let port = browser::free_port(LOOPBACK).unwrap();
        let (mut child, _home) = run_remote_script(port);
        let line = first_stderr_line(&mut child);
        let status = child.wait().unwrap();
        assert!(!status.success(), "{line:?}");
        assert!(line.contains("too old to tunnel into"), "{line:?}");
    }

    /// An old remote fails on an unknown `--no-open` with a clap usage dump;
    /// the script checks first and says what to do instead.
    #[test]
    fn a_remote_too_old_to_tunnel_is_named_as_such() {
        assert!(REMOTE_SCRIPT.contains("grep -q -- --no-open"));
        assert!(REMOTE_SCRIPT.contains("nebula upgrade"));
    }

    /// Both ends of the forward are pinned to loopback: the remote ttyd is
    /// reachable through this channel and nothing else.
    #[test]
    fn the_forward_is_loopback_to_loopback() {
        assert_eq!(forward_spec(7681, 9000), "127.0.0.1:7681:127.0.0.1:9000");
    }

    /// The noise floor of the readiness poll, and the messages that must
    /// survive it.
    #[test]
    fn only_the_not_yet_listening_channel_errors_are_dropped() {
        assert!(is_refused_channel(
            "channel 3: open failed: connect failed: Connection refused"
        ));
        assert!(!is_refused_channel("Permission denied (publickey)."));
        assert!(!is_refused_channel(
            "ssh: Could not resolve hostname nope: nodename nor servname provided"
        ));
        assert!(!is_refused_channel(
            "bind [127.0.0.1]:7681: Address already in use"
        ));
    }

    /// The remote port follows the local one so the two numbers match, which
    /// is what makes the printed URL readable as "that host's nebula".
    #[test]
    fn the_remote_port_defaults_to_the_local_one() {
        let o = TunnelOpts {
            remote_port: None,
            ..opts()
        };
        assert_eq!(o.remote_port.unwrap_or(4242), 4242);
        let o = TunnelOpts {
            remote_port: Some(9000),
            ..opts()
        };
        assert_eq!(o.remote_port.unwrap_or(4242), 9000);
    }

    /// A local port that is taken is an error rather than a silent move: the
    /// user named it, likely because something else already points at it.
    #[test]
    fn an_explicit_local_port_that_is_taken_is_an_error() {
        let port = browser::free_port(LOOPBACK).unwrap();
        let _guard = TcpListener::bind(SocketAddr::new(LOOPBACK, port)).unwrap();
        let err = resolve_local_port(Some(port)).unwrap_err().to_string();
        assert!(
            err.contains(&format!("local port {port} is not free")),
            "{err}"
        );
    }

    #[test]
    fn port_zero_means_any_free_local_port() {
        let port = resolve_local_port(Some(0)).expect("picks one");
        assert_ne!(port, 0);
        TcpListener::bind(SocketAddr::new(LOOPBACK, port)).expect("free");
    }

    /// A tunnel per remote host at once is routine, so no `--port` always
    /// lands on a port that is actually free — the default when it is, one
    /// the kernel picked when something else (another tunnel, a local
    /// `nebula browser`) already holds it. Deliberately does not stand on
    /// 7681 itself: `browser`'s equivalent test does, in this same binary.
    #[test]
    fn no_port_resolves_to_one_that_is_free() {
        let port = resolve_local_port(None).expect("resolves");
        assert_ne!(port, 0, "must be a real port we can print");
        TcpListener::bind(SocketAddr::new(LOOPBACK, port)).expect("free");
    }

    /// Nothing is listening on a port we just released, so the probe must
    /// come back false rather than mistaking a connect for a service.
    #[test]
    fn an_unserved_port_does_not_answer() {
        let port = browser::free_port(LOOPBACK).unwrap();
        assert!(!answers_http(SocketAddr::new(LOOPBACK, port)));
    }
}
