//! `nebula upgrade`: re-run the published install script.
//!
//! install.sh already knows how to pick the right prebuilt binary, fall back
//! to cargo, and report where it landed — this subcommand just saves the user
//! from remembering the curl one-liner.

use anyhow::{bail, Context, Result};
use std::path::{Path, PathBuf};
use std::process::Command;

const INSTALL_URL: &str =
    "https://raw.githubusercontent.com/AgentSystemLabs/nebula/main/install.sh";

pub fn run_upgrade(force: bool) -> Result<()> {
    let url = std::env::var("NEBULA_INSTALL_URL")
        .ok()
        .filter(|u| !u.is_empty())
        .unwrap_or_else(|| INSTALL_URL.to_string());
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
            println!("      run 'nebula kill-server' to restart onto the new binary (stops all sessions).");
        }
        Ok(IdleShutdown::Skewed) | Err(_) => {
            println!("note: a daemon from a previous version may still be running.");
            println!("      run 'nebula kill-server' to restart onto the new binary (stops all sessions).");
        }
    }
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
    matches!(dir.file_name()?.to_str()?, "debug" | "release").then(|| exe.clone())
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
