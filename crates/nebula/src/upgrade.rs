//! `nebula upgrade`: re-run the published install script.
//!
//! install.sh already knows how to pick the right prebuilt binary, fall back
//! to cargo, and report where it landed — this subcommand just saves the user
//! from remembering the curl one-liner.

use anyhow::{bail, Context, Result};
use nebula_core::PROTOCOL_VERSION;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::Command;

const INSTALL_URL: &str =
    "https://raw.githubusercontent.com/AgentSystemLabs/nebula/main/install.sh";

/// The published install script, with `NEBULA_INSTALL_URL` as the override
/// hook (tests point it at a file:// URL). Shared with `nebula ssh`.
pub(crate) fn install_url() -> String {
    nebula_core::env::non_empty(nebula_core::env::INSTALL_URL)
        .unwrap_or_else(|| INSTALL_URL.to_string())
}

/// Printed whenever a daemon from an older binary is left running: the only
/// way onto the new code is a restart, and a restart takes the sessions.
pub(crate) const KILL_HINT: &str =
    "      run 'nebula kill' to restart onto the new binary (stops all sessions).";

pub fn run_upgrade(force: bool) -> Result<()> {
    let url = install_url();
    // The runtime dir is already the 0700 auth boundary; staging the script
    // there keeps it out of a world-writable /tmp before we execute it.
    nebula_daemon::lifecycle::ensure_runtime_dir()?;
    upgrade_with(&url, &nebula_core::paths::runtime_dir(), force)?;
    finish_daemon_handoff();
    Ok(())
}

/// Swapping the binary on disk doesn't touch the running daemon — it keeps
/// executing the old code. An idle daemon (no live PTYs) is shut down here so
/// the next launch spawns the new binary; live sessions would die with the
/// daemon, so that restart stays the user's call. Never fails the upgrade:
/// the install already succeeded.
///
/// This process is still the old binary, so it speaks the daemon's protocol
/// whatever the new one does. That makes now the moment to say when the new
/// build can't attach — afterwards the only binary that could reach the
/// daemon is gone (#68).
fn finish_daemon_handoff() {
    use nebula_tui::ipc::IdleShutdown;
    match nebula_tui::shutdown_daemon_if_idle() {
        Ok(IdleShutdown::NoDaemon) => {}
        Ok(IdleShutdown::ShutDown) => {
            println!(
                "old daemon had no live sessions — shut it down; \
                 the new binary starts on the next launch"
            );
        }
        Ok(IdleShutdown::SessionsLive { count }) => {
            let plural = if count == 1 { "" } else { "s" };
            println!("note: the old daemon is still running with {count} live session{plural}.");
            let installed = std::env::var_os("PATH").and_then(|path| {
                protocol_version_on_path(&path, &nebula_core::paths::runtime_dir())
            });
            match installed.filter(|v| *v != PROTOCOL_VERSION) {
                Some(new) => {
                    println!("{}", protocol_change_note(new));
                    if !offer_restart(count) {
                        println!("{KILL_HINT}");
                    }
                }
                None => println!("{KILL_HINT}"),
            }
        }
        Ok(IdleShutdown::Skewed) | Err(_) => {
            println!("note: a daemon from a previous version may still be running.");
            println!("{KILL_HINT}");
        }
    }
}

/// Why the restart can't wait, when the new build speaks another protocol.
fn protocol_change_note(installed: u32) -> String {
    format!(
        "      the new build speaks protocol v{installed} and the daemon v{PROTOCOL_VERSION}, \
         so `nebula` won't open until the daemon restarts."
    )
}

/// Offer that restart when someone is at the terminal to answer. No is still
/// the default — a restart takes every session with it. True once the daemon
/// is down.
fn offer_restart(count: usize) -> bool {
    use std::io::{BufRead, IsTerminal, Write};
    if !(std::io::stdin().is_terminal() && std::io::stdout().is_terminal()) {
        return false;
    }
    let plural = if count == 1 { "" } else { "s" };
    print!("restart it now? that stops the {count} live session{plural} [y/N] ");
    let _ = std::io::stdout().flush();
    let mut answer = String::new();
    if std::io::stdin().lock().read_line(&mut answer).is_err() || !is_yes(&answer) {
        return false;
    }
    match nebula_tui::run_kill() {
        Ok(()) => {
            println!("the new binary starts on the next launch");
            true
        }
        Err(err) => {
            println!("could not stop the daemon: {err:#}");
            false
        }
    }
}

fn is_yes(answer: &str) -> bool {
    matches!(answer.trim().to_ascii_lowercase().as_str(), "y" | "yes")
}

/// The protocol version of the first `nebula` on `path` — the one the user
/// runs next, which is where install.sh just put the new build (it warns when
/// that dir isn't on PATH). None when there is none, or it predates
/// `_protocol-version`: such a build reads the word as `nebula <dir>`, so it
/// runs from `cwd` — the runtime dir, where no directory by that name will
/// ever sit to be registered as a project.
fn protocol_version_on_path(path: &OsStr, cwd: &Path) -> Option<u32> {
    use std::os::unix::fs::PermissionsExt;
    let exe = std::env::split_paths(path)
        .map(|dir| dir.join("nebula"))
        .find(|p| {
            p.metadata()
                .is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        })?;
    let out = Command::new(exe)
        .arg("_protocol-version")
        .current_dir(cwd)
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    String::from_utf8_lossy(&out.stdout).trim().parse().ok()
}

fn upgrade_with(url: &str, staging_dir: &Path, force: bool) -> Result<()> {
    if !force {
        if let Some(exe) = dev_build() {
            bail!(
                "{} is a local cargo build.\n\n\
                 Upgrading installs the published binary and would overwrite a \
                 ~/.cargo/bin symlink pointing at this build. Rebuild instead:\n\
                 \x20   cargo build --release\n\n\
                 Re-run with --force to install the published binary anyway.",
                exe.display()
            );
        }
    }

    let script = stage_script(url, staging_dir)?;
    // Inherited stdio: the script's own progress lines are the UI here.
    // NEBULA_UPGRADE_HANDOFF tells install.sh to skip its "daemon still
    // running" note — finish_daemon_handoff owns that messaging here.
    let result = Command::new("sh")
        .arg(&script)
        .env("NEBULA_UPGRADE_HANDOFF", "1")
        .status()
        .with_context(|| format!("run {}", script.display()));
    let _ = std::fs::remove_file(&script);

    let status = result?;
    if !status.success() {
        bail!("install script failed ({status})");
    }
    Ok(())
}

/// Download the installer to a file before running it. `curl … | sh` executes
/// whatever arrived when a connection drops mid-transfer; a staged file either
/// passes the shebang check below or never runs at all.
fn stage_script(url: &str, dir: &Path) -> Result<PathBuf> {
    let path = dir.join(format!("install-{}.sh", std::process::id()));
    let output = Command::new("curl")
        .args(["-fsSL", url, "-o"])
        .arg(&path)
        .output()
        .context("run curl — is it installed?")?;
    if !output.status.success() {
        let _ = std::fs::remove_file(&path);
        let stderr = String::from_utf8_lossy(&output.stderr);
        let detail = stderr.trim();
        bail!(
            "downloading {url} failed{}{}",
            if detail.is_empty() { "" } else { ": " },
            detail
        );
    }
    // A 200 that isn't the installer (captive portal, error page) would
    // otherwise get executed as a shell script.
    if !std::fs::read_to_string(&path)
        .unwrap_or_default()
        .starts_with("#!")
    {
        let _ = std::fs::remove_file(&path);
        bail!("{url} did not return a shell script");
    }
    Ok(path)
}

/// The running binary when it lives in a cargo build dir (`target/release`,
/// `target/debug`, or the `deps/` dir test binaries run from). Symlinks
/// resolve first: the usual dev setup is a `~/.cargo/bin/nebula` symlink
/// pointing into `target/release`, and it's that symlink an upgrade replaces.
fn dev_build() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let exe = std::fs::canonicalize(&exe).unwrap_or(exe);
    let dir = exe.parent()?;
    let dir = if dir.file_name()? == "deps" {
        dir.parent()?
    } else {
        dir
    };
    let is_target_dir = matches!(dir.file_name()?.to_str()?, "debug" | "release");
    is_target_dir.then_some(exe)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// curl speaks file://, so these exercise the real download → stage →
    /// execute path without a network.
    fn url_of(path: &Path) -> String {
        format!("file://{}", path.display())
    }

    fn write_script(dir: &Path, body: &str) -> PathBuf {
        let path = dir.join("source.sh");
        std::fs::write(&path, body).unwrap();
        path
    }

    fn staged_files(dir: &Path) -> Vec<String> {
        std::fs::read_dir(dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.starts_with("install-"))
            .collect()
    }

    #[test]
    fn runs_the_downloaded_script_then_cleans_up() {
        let tmp = tempfile::tempdir().unwrap();
        let marker = tmp.path().join("ran");
        let script = write_script(
            tmp.path(),
            &format!("#!/bin/sh\necho upgrading\n: > '{}'\n", marker.display()),
        );

        upgrade_with(&url_of(&script), tmp.path(), true).unwrap();

        assert!(marker.exists(), "the installer actually ran");
        assert!(
            staged_files(tmp.path()).is_empty(),
            "staged copy is removed"
        );
    }

    #[test]
    fn a_failing_installer_is_an_error() {
        let tmp = tempfile::tempdir().unwrap();
        let script = write_script(tmp.path(), "#!/bin/sh\nexit 3\n");

        let err = upgrade_with(&url_of(&script), tmp.path(), true).unwrap_err();
        assert!(err.to_string().contains("install script failed"), "{err}");
        assert!(
            staged_files(tmp.path()).is_empty(),
            "staged copy is removed on failure"
        );
    }

    #[test]
    fn a_missing_url_never_reaches_sh() {
        let tmp = tempfile::tempdir().unwrap();
        let missing = tmp.path().join("nope.sh");

        let err = upgrade_with(&url_of(&missing), tmp.path(), true).unwrap_err();
        assert!(err.to_string().contains("downloading"), "{err}");
        assert!(staged_files(tmp.path()).is_empty());
    }

    #[test]
    fn a_non_script_payload_is_rejected() {
        let tmp = tempfile::tempdir().unwrap();
        // What a captive portal or a 200-with-error-page looks like.
        let page = write_script(tmp.path(), "<html>sign in to continue</html>");

        let err = upgrade_with(&url_of(&page), tmp.path(), true).unwrap_err();
        assert!(
            err.to_string().contains("did not return a shell script"),
            "{err}"
        );
        assert!(staged_files(tmp.path()).is_empty());
    }

    fn fake_nebula(dir: &Path, body: &str) {
        use std::os::unix::fs::PermissionsExt;
        let exe = dir.join("nebula");
        std::fs::write(&exe, body).unwrap();
        std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    #[test]
    fn reads_the_protocol_of_the_first_nebula_on_path() {
        let empty = tempfile::tempdir().unwrap();
        let newer = tempfile::tempdir().unwrap();
        let shadowed = tempfile::tempdir().unwrap();
        fake_nebula(
            newer.path(),
            "#!/bin/sh\n[ \"$1\" = _protocol-version ] && echo 44\n",
        );
        fake_nebula(shadowed.path(), "#!/bin/sh\necho 12\n");
        let path = std::env::join_paths([empty.path(), newer.path(), shadowed.path()]).unwrap();

        assert_eq!(protocol_version_on_path(&path, empty.path()), Some(44));
    }

    // Every build released before `_protocol-version` reads the word as
    // `nebula <dir>`: an error when no such directory exists, a registered
    // project when one does. Either way it must come back "unknown", and the
    // probe must run where the caller says, not wherever `upgrade` was run.
    #[test]
    fn a_nebula_without_the_hook_has_no_known_protocol() {
        let old = tempfile::tempdir().unwrap();
        let added = old.path().join("added");
        fake_nebula(
            old.path(),
            &format!(
                "#!/bin/sh\nif [ -d \"$1\" ]; then : > '{}'; echo \"added project $1\"; \
                 else echo \"Error: $1 does not exist\" >&2; exit 1; fi\n",
                added.display()
            ),
        );
        let clean = tempfile::tempdir().unwrap();
        assert_eq!(
            protocol_version_on_path(old.path().as_os_str(), clean.path()),
            None
        );
        assert!(!added.exists());

        let trap = tempfile::tempdir().unwrap();
        std::fs::create_dir(trap.path().join("_protocol-version")).unwrap();
        assert_eq!(
            protocol_version_on_path(old.path().as_os_str(), trap.path()),
            None
        );
        assert!(added.exists(), "the probe ran in the cwd it was given");

        let none = tempfile::tempdir().unwrap();
        assert_eq!(
            protocol_version_on_path(none.path().as_os_str(), clean.path()),
            None
        );
    }

    #[test]
    fn the_protocol_note_names_both_versions() {
        let note = protocol_change_note(PROTOCOL_VERSION + 1);
        assert!(
            note.contains(&format!("v{}", PROTOCOL_VERSION + 1)),
            "{note}"
        );
        assert!(
            note.contains(&format!("daemon v{PROTOCOL_VERSION}")),
            "{note}"
        );
    }

    #[test]
    fn only_an_explicit_yes_restarts() {
        for yes in ["y\n", "Y\n", " yes \n"] {
            assert!(is_yes(yes), "{yes:?}");
        }
        for no in ["\n", "n\n", "no\n", "sure\n", ""] {
            assert!(!is_yes(no), "{no:?}");
        }
    }

    #[test]
    fn dev_builds_are_refused_without_force() {
        // Cargo runs this binary from <target>/debug/deps, so the guard sees
        // exactly what it would see for a symlinked target/release build.
        assert!(
            dev_build().is_some(),
            "test binary should look like a cargo build"
        );

        let tmp = tempfile::tempdir().unwrap();
        let marker = tmp.path().join("ran");
        let script = write_script(
            tmp.path(),
            &format!("#!/bin/sh\n: > '{}'\n", marker.display()),
        );

        let err = upgrade_with(&url_of(&script), tmp.path(), false).unwrap_err();
        assert!(err.to_string().contains("local cargo build"), "{err}");
        assert!(
            !marker.exists(),
            "refusing must happen before the installer runs"
        );
    }
}
