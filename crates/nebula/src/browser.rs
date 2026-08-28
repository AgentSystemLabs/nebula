//! `nebula browser [--port N] [--bind ADDR | --public] [--credential U:P]
//! [--no-open]`: open this TUI in a web browser.
//!
//! The port is chosen rather than fixed, because several checkouts each
//! serving their own build is the normal case here, not a mistake — see
//! `resolve_port`.
//!
//! The HTTP/WebSocket half is ttyd's job — it runs a command in a PTY and
//! bridges that PTY to xterm.js in the page. We spawn `ttyd … nebula`, wait
//! for the port to accept a connection, hand the URL to the desktop browser,
//! then block on ttyd until it exits. Ctrl+C takes both down: they share a
//! process group, so the signal reaches ttyd directly.
//!
//! Loopback is the default because what this serves is a live, writable
//! terminal on this machine and ttyd ships no auth of its own. `--bind` /
//! `--public` widen it anyway, for the case the default cannot cover: nebula
//! running on a box you reach over the network (an EC2 instance behind a
//! security group, say), where the access control lives in front of the port
//! rather than in ttyd. A wider bind without `--credential` says so loudly —
//! see `warn_if_exposed`.

use anyhow::{anyhow, bail, Context, Result};
use std::ffi::OsString;
use std::io::ErrorKind;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, TcpListener, TcpStream};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

/// ttyd's own default, and where a bare `nebula browser` starts looking so
/// it lines up with every ttyd doc the user might read next. It is a
/// preference, not a reservation: `resolve_port` steps off it when it's busy.
pub const DEFAULT_PORT: u16 = 7681;

/// Where `nebula browser` listens unless told otherwise.
pub const DEFAULT_BIND: IpAddr = IpAddr::V4(Ipv4Addr::LOCALHOST);

/// What `--public` means: every interface, so whatever address the host is
/// reachable on works without naming it.
pub const PUBLIC_BIND: IpAddr = IpAddr::V4(Ipv4Addr::UNSPECIFIED);

/// How long ttyd gets to bind before we stop waiting to open the browser.
const STARTUP_TIMEOUT: Duration = Duration::from_secs(5);
const POLL_INTERVAL: Duration = Duration::from_millis(50);

/// ttyd's own default font size, passed back to it explicitly to buy a second
/// fit. See `ttyd_args` — the value is deliberately the default, so the tab
/// looks exactly as it always did; only the column count changes.
const FONT_SIZE: u16 = 13;

const MISSING_TTYD: &str = "\
nebula browser needs ttyd, and it is not on your PATH.

ttyd serves a command's terminal over HTTP; `nebula browser` points it at this
binary so the TUI renders in a browser tab. Install it, then try again:

  macOS          brew install ttyd
  Debian/Ubuntu  sudo apt install ttyd
  Arch           sudo pacman -S ttyd
  elsewhere      https://github.com/tsl0922/ttyd#installation";

/// Everything `nebula browser` was asked for, straight off the CLI.
#[derive(Debug, Clone)]
pub struct BrowserOpts {
    /// `--port`; see [`resolve_port`].
    pub port: Option<u16>,
    /// `--bind`, or [`PUBLIC_BIND`] for `--public`.
    pub bind: IpAddr,
    /// `--credential USER:PASSWORD`, passed to ttyd as HTTP basic auth.
    pub credential: Option<String>,
    /// Whether to hand the URL to a desktop browser once ttyd is serving.
    /// `--no-open` clears it, for a machine with no desktop to open it on —
    /// `nebula tunnel` runs this on the remote box, where an `xdg-open`
    /// would at best fail and at worst block forever on a text browser.
    pub open: bool,
}

impl Default for BrowserOpts {
    fn default() -> Self {
        Self {
            port: None,
            bind: DEFAULT_BIND,
            credential: None,
            open: true,
        }
    }
}

pub fn run_browser(opts: BrowserOpts) -> Result<()> {
    let port = resolve_port(opts.port, opts.bind)?;
    warn_if_exposed(&opts);
    let mut child = spawn_ttyd(&nebula_exe(), port, &opts)?;
    wait_until_serving(&mut child, SocketAddr::new(reachable_addr(opts.bind), port))?;

    let url = url_for(reachable_addr(opts.bind), port);
    if opts.open && open_url(&url) {
        println!("nebula browser: serving on {url}");
    } else {
        println!("nebula browser: serving on {url} (open it yourself — no browser launched)");
    }
    if opts.bind.is_unspecified() {
        println!("nebula browser: reachable on every interface of this host at port {port}.");
    }
    println!("Ctrl+C to stop.");

    let status = child.wait().context("failed to wait on ttyd")?;
    // A signal exit is the user's Ctrl+C reaching ttyd through the shared
    // process group, not a failure worth reporting.
    match status.code() {
        Some(0) | None => Ok(()),
        Some(code) => bail!("ttyd exited with status {code}"),
    }
}

/// A bind address is not always an address you can *connect* to: `0.0.0.0`
/// and `::` name every interface rather than one, so both the readiness poll
/// and the URL we open use loopback of the same family instead.
fn reachable_addr(bind: IpAddr) -> IpAddr {
    match bind {
        IpAddr::V4(a) if a.is_unspecified() => IpAddr::V4(Ipv4Addr::LOCALHOST),
        IpAddr::V6(a) if a.is_unspecified() => IpAddr::V6(Ipv6Addr::LOCALHOST),
        other => other,
    }
}

/// `SocketAddr`'s own formatting, which brackets IPv6 exactly as a URL needs.
fn url_for(addr: IpAddr, port: u16) -> String {
    format!("http://{}", SocketAddr::new(addr, port))
}

/// Say out loud when this run is reachable from another host with nothing in
/// front of it. What ttyd serves is a writable terminal, so the difference
/// between "behind a security group" and "open to the internet" is invisible
/// from here — the warning names the risk and leaves the call to the user.
fn warn_if_exposed(opts: &BrowserOpts) {
    if opts.bind.is_loopback() {
        return;
    }
    eprintln!("nebula browser: WARNING — binding {} serves a live, writable terminal on this machine to the network.", opts.bind);
    if opts.credential.is_none() {
        eprintln!("nebula browser: there is no password on it. Restrict the port (firewall, security group, VPN) or pass --credential USER:PASSWORD.");
    }
}

/// Settle on a port before ttyd is spawned, so the URL we print and the
/// port ttyd binds are the same number. (`ttyd -p 0` picks its own and only
/// mentions it in its log, which is why we never pass 0 through.)
///
/// * `--port N` — that port or nothing. The user named it, so a clash is an
///   error they want to hear rather than a silent move.
/// * `--port 0` — any free port, no preference.
/// * nothing — [`DEFAULT_PORT`] when it's free, otherwise a free one, said
///   out loud. Running a `nebula browser` per checkout is routine, and the
///   second one failing on a port collision would be a papercut with no
///   upside.
///
/// Free is asked of `bind` and not of loopback: a port can be taken on one
/// interface and free on another, and the only answer that matters is the
/// one for the address ttyd is about to bind.
fn resolve_port(requested: Option<u16>, bind: IpAddr) -> Result<u16> {
    match requested {
        Some(0) => free_port(bind),
        Some(n) => {
            probe(n, bind).with_context(|| format!("port {n} is not free"))?;
            Ok(n)
        }
        None if probe(DEFAULT_PORT, bind).is_ok() => Ok(DEFAULT_PORT),
        None => {
            let port = free_port(bind)?;
            println!("nebula browser: {DEFAULT_PORT} is busy — serving on {port} instead");
            Ok(port)
        }
    }
}

/// Whether this port can be bound on `bind` right now. The listener is
/// dropped immediately, so this reserves nothing — it only answers the
/// question, and ttyd binds for real a moment later. A listener that never
/// accepted anything doesn't go to TIME_WAIT, so the rebind is clean. The
/// gap in between is a race in principle; ttyd's own bind failure (which
/// `wait_until_serving` surfaces) is the backstop.
pub(crate) fn probe(port: u16, bind: IpAddr) -> std::io::Result<()> {
    TcpListener::bind(SocketAddr::new(bind, port)).map(drop)
}

/// A port on `bind` the kernel says is free right now.
pub(crate) fn free_port(bind: IpAddr) -> Result<u16> {
    let sock = TcpListener::bind(SocketAddr::new(bind, 0))
        .with_context(|| format!("could not get a free port on {bind} from the kernel"))?;
    let port = sock
        .local_addr()
        .context("could not read back the port the kernel chose")?
        .port();
    Ok(port)
}

fn spawn_ttyd(exe: &OsString, port: u16, opts: &BrowserOpts) -> Result<Child> {
    Command::new("ttyd")
        .args(ttyd_args(port, opts))
        .arg(exe)
        // ttyd never reads stdin, and inheriting it would put a second
        // reader on the terminal nebula was launched from.
        .stdin(Stdio::null())
        .spawn()
        .map_err(|e| match e.kind() {
            ErrorKind::NotFound => anyhow!(MISSING_TTYD),
            _ => anyhow::Error::new(e).context("failed to start ttyd"),
        })
}

fn ttyd_args(port: u16, opts: &BrowserOpts) -> Vec<String> {
    let mut args: Vec<String> = vec![
        // ttyd is read-only by default, and a TUI you cannot type into is
        // not worth serving.
        "-W".into(),
        "-i".into(),
        opts.bind.to_string(),
        "-p".into(),
        port.to_string(),
    ];
    // ttyd's listener is v4 unless this says otherwise, whatever `-i` holds.
    if opts.bind.is_ipv6() {
        args.push("-6".into());
    }
    // HTTP basic auth, ttyd's only access control. Absent by default: on
    // loopback there is nobody to keep out who isn't already on the machine.
    if let Some(cred) = &opts.credential {
        args.push("-c".into());
        args.push(cred.clone());
    }
    args.extend([
        // Makes the grid reach the right edge of the window. ttyd's page
        // fits the terminal to the window immediately after `Terminal.open`,
        // while xterm is still on its DOM renderer, whose cell width is the
        // measured character advance (7.83px at this size). It then swaps in
        // the WebGL renderer, which floors that to a whole pixel (7px) and
        // does *not* re-fit — so the grid keeps the ~10% narrower column
        // count and paints ~24 columns short of the edge. ttyd re-runs the
        // fit whenever it applies a client option whose name starts with
        // `font`, and by then the real renderer is in place, so naming the
        // font size — even at ttyd's own default — is what closes the gap.
        // Keep this ahead of `--`, and keep a `font*` option in the list.
        "-t".into(),
        format!("fontSize={FONT_SIZE}"),
        // Stop option parsing: everything after this is the command to run.
        "--".into(),
    ]);
    args
}

/// Serve *this* binary rather than whatever `nebula` resolves to on PATH — a
/// cargo build and an installed release are routinely different builds.
fn nebula_exe() -> OsString {
    std::env::current_exe()
        .map(OsString::from)
        .unwrap_or_else(|_| "nebula".into())
}

/// Block until the address accepts a connection, so the browser never opens
/// on a refused one.
fn wait_until_serving(child: &mut Child, addr: SocketAddr) -> Result<()> {
    let deadline = Instant::now() + STARTUP_TIMEOUT;
    loop {
        // A ttyd that already exited will never bind. The usual cause is the
        // port being taken, and it has printed the reason itself.
        if let Some(status) = child.try_wait().context("failed to poll ttyd")? {
            bail!("ttyd exited before it started serving ({status}) — see its output above");
        }
        if TcpStream::connect_timeout(&addr, POLL_INTERVAL).is_ok() {
            return Ok(());
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            bail!(
                "ttyd did not start serving on {addr} within {}s",
                STARTUP_TIMEOUT.as_secs()
            );
        }
        std::thread::sleep(POLL_INTERVAL);
    }
}

/// Hand the URL to the desktop browser, mirroring the TUI's opener: `open` on
/// macOS, `xdg-open` on Linux.
pub(crate) fn open_url(url: &str) -> bool {
    if cfg!(test) {
        return true;
    }
    let opener = if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    Command::new(opener)
        .arg(url)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The shape every other test starts from: loopback, no auth.
    fn opts() -> BrowserOpts {
        BrowserOpts::default()
    }

    fn value_after(args: &[String], flag: &str) -> String {
        let at = args
            .iter()
            .position(|a| a == flag)
            .unwrap_or_else(|| panic!("no {flag} in {args:?}"));
        args[at + 1].clone()
    }

    #[test]
    fn args_serve_a_writable_loopback_port() {
        let args = ttyd_args(9000, &opts());
        assert!(args.contains(&"-W".to_string()), "must be writable");
        assert_eq!(value_after(&args, "-i"), "127.0.0.1");
        assert_eq!(value_after(&args, "-p"), "9000");
    }

    #[test]
    fn command_is_separated_from_the_options() {
        // Without the terminator, getopt permutation could read the served
        // binary's path as a ttyd flag.
        assert_eq!(ttyd_args(DEFAULT_PORT, &opts()).last().unwrap(), "--");
    }

    #[test]
    fn a_font_client_option_is_passed_so_ttyd_refits_after_the_renderer_swap() {
        // Load-bearing, and it looks like a no-op: the value is ttyd's own
        // default. Dropping it costs ~24 columns off the right of the window.
        let args = ttyd_args(DEFAULT_PORT, &opts());
        let opt = args.iter().position(|a| a == "-t").expect("passes -t");
        assert_eq!(args[opt + 1], format!("fontSize={FONT_SIZE}"));
        // ttyd only re-fits for options *named* `font…`, and only if the
        // option reaches it as an option rather than as the served command.
        assert!(args[opt + 1].starts_with("font"));
        assert!(opt + 1 < args.iter().position(|a| a == "--").unwrap());
    }

    /// The whole point: a second checkout serving at the same time gets a
    /// port of its own instead of an error.
    #[test]
    fn a_busy_default_port_steps_aside_instead_of_failing() {
        // Stand on the default the way another checkout's ttyd would.
        let held = TcpListener::bind(SocketAddr::new(DEFAULT_BIND, DEFAULT_PORT));
        let Ok(held) = held else {
            // Something outside the test already owns 7681 — which is the
            // condition under test, so the assertion below still holds.
            let port = resolve_port(None, DEFAULT_BIND).expect("still resolves");
            assert_ne!(port, DEFAULT_PORT);
            return;
        };
        let port = resolve_port(None, DEFAULT_BIND).expect("falls back");
        assert_ne!(port, DEFAULT_PORT, "must not hand ttyd a taken port");
        assert_ne!(port, 0, "must be a real port we can print");
        drop(held);

        // Free again: back to the default, so the usual case is unchanged.
        assert_eq!(resolve_port(None, DEFAULT_BIND).unwrap(), DEFAULT_PORT);
    }

    /// `--port 0` used to be refused outright. It now means "any free one",
    /// which is what the Makefile's per-worktree dev instances ask for.
    #[test]
    fn port_zero_means_any_free_port() {
        let port = resolve_port(Some(0), DEFAULT_BIND).expect("picks one");
        assert_ne!(port, 0);
        // And it is genuinely free — we can take it ourselves right after.
        TcpListener::bind(SocketAddr::new(DEFAULT_BIND, port)).expect("free");
    }

    /// A port the user named is theirs or nothing: silently moving would
    /// break `ssh -L 9000:localhost:9000` set up against that number.
    #[test]
    fn an_explicit_port_that_is_taken_is_an_error() {
        let held = free_port(DEFAULT_BIND).unwrap();
        let _guard = TcpListener::bind(SocketAddr::new(DEFAULT_BIND, held)).unwrap();
        let err = resolve_port(Some(held), DEFAULT_BIND)
            .unwrap_err()
            .to_string();
        assert!(err.contains(&format!("port {held} is not free")), "{err}");
    }

    #[test]
    fn the_missing_ttyd_message_says_how_to_install_it() {
        assert!(MISSING_TTYD.contains("brew install ttyd"));
        assert!(MISSING_TTYD.contains("apt install ttyd"));
    }

    /// `--public` is the case this exists for: a nebula on a remote box whose
    /// access control is the security group in front of it.
    #[test]
    fn a_public_bind_reaches_ttyd_as_the_listen_address() {
        let args = ttyd_args(
            8080,
            &BrowserOpts {
                bind: PUBLIC_BIND,
                ..opts()
            },
        );
        assert_eq!(value_after(&args, "-i"), "0.0.0.0");
        assert!(!args.contains(&"-6".to_string()), "0.0.0.0 is v4");
    }

    #[test]
    fn an_explicit_bind_address_is_passed_through_verbatim() {
        let bind: IpAddr = "10.0.1.7".parse().unwrap();
        let args = ttyd_args(DEFAULT_PORT, &BrowserOpts { bind, ..opts() });
        assert_eq!(value_after(&args, "-i"), "10.0.1.7");
    }

    /// ttyd listens on v4 unless `-6` says otherwise, no matter what `-i`
    /// holds — so a v6 bind address alone would silently not take.
    #[test]
    fn an_ipv6_bind_also_turns_on_ttyds_ipv6_listener() {
        let bind: IpAddr = "::".parse().unwrap();
        let args = ttyd_args(DEFAULT_PORT, &BrowserOpts { bind, ..opts() });
        assert_eq!(value_after(&args, "-i"), "::");
        assert!(args.contains(&"-6".to_string()));
    }

    #[test]
    fn a_credential_becomes_ttyds_basic_auth_and_is_absent_otherwise() {
        assert!(!ttyd_args(DEFAULT_PORT, &opts()).contains(&"-c".to_string()));
        let args = ttyd_args(
            DEFAULT_PORT,
            &BrowserOpts {
                credential: Some("me:hunter2".into()),
                ..opts()
            },
        );
        assert_eq!(value_after(&args, "-c"), "me:hunter2");
        // Auth has to be an option, not part of the served command line.
        let at = args.iter().position(|a| a == "-c").unwrap();
        assert!(at < args.iter().position(|a| a == "--").unwrap());
    }

    /// `0.0.0.0` is a bind address, not a destination: polling it for
    /// readiness and opening it in a browser both need a real one.
    #[test]
    fn an_unspecified_bind_is_polled_and_opened_on_loopback() {
        assert_eq!(reachable_addr(PUBLIC_BIND), DEFAULT_BIND);
        assert_eq!(
            reachable_addr("::".parse().unwrap()),
            IpAddr::V6(Ipv6Addr::LOCALHOST)
        );
        // Anything else is already somewhere you can connect to.
        let lan: IpAddr = "10.0.1.7".parse().unwrap();
        assert_eq!(reachable_addr(lan), lan);
    }

    #[test]
    fn ipv6_urls_are_bracketed() {
        assert_eq!(url_for(DEFAULT_BIND, 7681), "http://127.0.0.1:7681");
        assert_eq!(
            url_for(IpAddr::V6(Ipv6Addr::LOCALHOST), 7681),
            "http://[::1]:7681"
        );
    }

    /// Free is asked of the address ttyd will bind. A port held on all
    /// interfaces is not free for a public bind even though the default port
    /// logic would otherwise still be looking at loopback.
    #[test]
    fn a_port_taken_on_every_interface_is_not_free_for_a_public_bind() {
        let port = free_port(PUBLIC_BIND).unwrap();
        let _guard = TcpListener::bind(SocketAddr::new(PUBLIC_BIND, port)).unwrap();
        let err = resolve_port(Some(port), PUBLIC_BIND)
            .unwrap_err()
            .to_string();
        assert!(err.contains(&format!("port {port} is not free")), "{err}");
    }
}
