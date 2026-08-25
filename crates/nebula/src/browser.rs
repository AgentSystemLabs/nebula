//! `nebula browser [--port N]`: open this TUI in a web browser.
//!
//! The HTTP/WebSocket half is ttyd's job — it runs a command in a PTY and
//! bridges that PTY to xterm.js in the page. We spawn `ttyd … nebula`, wait
//! for the port to accept a connection, hand the URL to the desktop browser,
//! then block on ttyd until it exits. Ctrl+C takes both down: they share a
//! process group, so the signal reaches ttyd directly.
//!
//! Bound to loopback and left unauthenticated on purpose. ttyd ships no auth
//! by default and what it serves here is a live terminal on this machine, so
//! reaching it from another host is a job for `ssh -L` or a tunnel — not for
//! swapping the bind address.

use anyhow::{anyhow, bail, Context, Result};
use std::ffi::OsString;
use std::io::ErrorKind;
use std::net::{SocketAddr, TcpStream};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

/// ttyd's own default, kept so a bare `nebula browser` lines up with every
/// ttyd doc the user might read next.
pub const DEFAULT_PORT: u16 = 7681;

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

pub fn run_browser(port: u16) -> Result<()> {
    // `-p 0` is ttyd's "pick any free port", but it only reports the choice
    // in its own log — we would have no URL to open.
    if port == 0 {
        bail!(
            "nebula browser needs a fixed port; 0 asks ttyd to choose one and never tells us which"
        );
    }
    let mut child = spawn_ttyd(&nebula_exe(), port)?;
    wait_until_serving(&mut child, port)?;

    let url = format!("http://127.0.0.1:{port}");
    if open_url(&url) {
        println!("nebula browser: serving on {url}");
    } else {
        println!("nebula browser: serving on {url} (open it yourself — no browser launched)");
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

fn spawn_ttyd(exe: &OsString, port: u16) -> Result<Child> {
    Command::new("ttyd")
        .args(ttyd_args(port))
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

fn ttyd_args(port: u16) -> Vec<String> {
    vec![
        // ttyd is read-only by default, and a TUI you cannot type into is
        // not worth serving.
        "-W".into(),
        "-i".into(),
        "127.0.0.1".into(),
        "-p".into(),
        port.to_string(),
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
    ]
}

/// Serve *this* binary rather than whatever `nebula` resolves to on PATH — a
/// cargo build and an installed release are routinely different builds.
fn nebula_exe() -> OsString {
    std::env::current_exe()
        .map(OsString::from)
        .unwrap_or_else(|_| "nebula".into())
}

/// Block until the port accepts a connection, so the browser never opens on
/// a refused one.
fn wait_until_serving(child: &mut Child, port: u16) -> Result<()> {
    let addr = SocketAddr::from(([127, 0, 0, 1], port));
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
                "ttyd did not start serving on port {port} within {}s",
                STARTUP_TIMEOUT.as_secs()
            );
        }
        std::thread::sleep(POLL_INTERVAL);
    }
}

/// Hand the URL to the desktop browser, mirroring the TUI's opener: `open` on
/// macOS, `xdg-open` on Linux.
fn open_url(url: &str) -> bool {
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

    #[test]
    fn args_serve_a_writable_loopback_port() {
        let args = ttyd_args(9000);
        assert!(args.contains(&"-W".to_string()), "must be writable");
        let iface = args.iter().position(|a| a == "-i").expect("binds -i");
        assert_eq!(args[iface + 1], "127.0.0.1");
        let port = args.iter().position(|a| a == "-p").expect("takes -p");
        assert_eq!(args[port + 1], "9000");
    }

    #[test]
    fn command_is_separated_from_the_options() {
        // Without the terminator, getopt permutation could read the served
        // binary's path as a ttyd flag.
        assert_eq!(ttyd_args(DEFAULT_PORT).last().unwrap(), "--");
    }

    #[test]
    fn a_font_client_option_is_passed_so_ttyd_refits_after_the_renderer_swap() {
        // Load-bearing, and it looks like a no-op: the value is ttyd's own
        // default. Dropping it costs ~24 columns off the right of the window.
        let args = ttyd_args(DEFAULT_PORT);
        let opt = args.iter().position(|a| a == "-t").expect("passes -t");
        assert_eq!(args[opt + 1], format!("fontSize={FONT_SIZE}"));
        // ttyd only re-fits for options *named* `font…`, and only if the
        // option reaches it as an option rather than as the served command.
        assert!(args[opt + 1].starts_with("font"));
        assert!(opt + 1 < args.iter().position(|a| a == "--").unwrap());
    }

    #[test]
    fn port_zero_is_refused_before_anything_spawns() {
        let err = run_browser(0).unwrap_err().to_string();
        assert!(err.contains("fixed port"), "{err}");
    }

    #[test]
    fn the_missing_ttyd_message_says_how_to_install_it() {
        assert!(MISSING_TTYD.contains("brew install ttyd"));
        assert!(MISSING_TTYD.contains("apt install ttyd"));
    }
}
