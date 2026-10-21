//! `nebula sbx [NAME]`: open nebula inside a local Docker container.
//!
//! The container is the sandbox: agents run inside it and never see this
//! machine's files. It is created on first use, started again when stopped,
//! and kept afterwards, so its daemon, sessions, clones and agent logins are
//! still there on the next `nebula sbx`. `docker rm -f nebula-sbx` throws it
//! away.
//!
//! Getting in is `nebula ssh` with `docker exec` as the transport: the same
//! [`crate::ssh::REMOTE_SCRIPT`] installs nebula in the container when it is
//! missing, forwards this machine's settings, and execs it. A container has
//! no login shell in the way, so the script and its parameters go over as
//! plain argv with no quoting.

use anyhow::{bail, Context, Result};
use std::ffi::OsString;
use std::os::unix::process::CommandExt;
use std::process::Command;

/// Debian with curl and git and little else: curl is what the install script
/// needs, git is what nebula needs. Only read when the container is created.
pub(crate) const DEFAULT_IMAGE: &str = "buildpack-deps:bookworm-scm";

/// Every sandbox container carries this label, so `docker ps --filter
/// label=nebula.sbx` lists them all.
const LABEL: &str = "nebula.sbx";

pub fn run_sbx(name: Option<&str>, image: Option<&str>) -> Result<()> {
    let container = container_name(name);
    match container_state(&container)? {
        State::Running => note_image_ignored(&container, image),
        State::Stopped => {
            note_image_ignored(&container, image);
            eprintln!("nebula: starting sandbox {container}…");
            docker(&["start", &container])?;
        }
        State::Missing => {
            let image = image.unwrap_or(DEFAULT_IMAGE);
            eprintln!("nebula: creating sandbox {container} from {image}…");
            docker(&create_args(&container, image))?;
        }
    }
    let bundle = nebula_tui::bundle::for_remote();
    let args = exec_args(
        &container,
        &crate::upgrade::install_url(),
        bundle.as_deref(),
    );
    // exec, as `nebula ssh` does: docker owns the tty from here and the exit
    // status propagates natively. Only returns on failure.
    let err = Command::new("docker").args(args).exec();
    Err(err).context("failed to exec docker")
}

/// `nebula-sbx`, or `nebula-sbx-NAME` for a sandbox of its own.
fn container_name(name: Option<&str>) -> String {
    match name {
        Some(name) => format!("nebula-sbx-{name}"),
        None => "nebula-sbx".to_string(),
    }
}

#[derive(Debug, PartialEq)]
enum State {
    Running,
    Stopped,
    Missing,
}

fn container_state(container: &str) -> Result<State> {
    let out = match Command::new("docker")
        .args([
            "container",
            "inspect",
            "-f",
            "{{.State.Running}}",
            container,
        ])
        .output()
    {
        Ok(out) => out,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            bail!("docker not found on PATH — nebula sbx runs the sandbox in Docker")
        }
        Err(e) => return Err(e).context("failed to run docker"),
    };
    if out.status.success() {
        return Ok(match String::from_utf8_lossy(&out.stdout).trim() {
            "true" => State::Running,
            _ => State::Stopped,
        });
    }
    let stderr = String::from_utf8_lossy(&out.stderr);
    if stderr.contains("No such container") {
        return Ok(State::Missing);
    }
    // Most often the Docker daemon isn't running; its own words say so.
    bail!("docker: {}", stderr.trim())
}

fn note_image_ignored(container: &str, image: Option<&str>) {
    if image.is_some() {
        eprintln!(
            "nebula: {container} already exists, so --image is ignored \
             (`docker rm -f {container}` to recreate it)"
        );
    }
}

fn docker(args: &[impl AsRef<std::ffi::OsStr>]) -> Result<()> {
    let status = Command::new("docker")
        .args(args)
        .stdout(std::process::Stdio::null())
        .status()
        .context("failed to run docker")?;
    if !status.success() {
        bail!("docker {} failed", args[0].as_ref().to_string_lossy());
    }
    Ok(())
}

/// A long-lived container doing nothing until `docker exec` brings nebula in.
/// `--init` puts a real PID 1 under the daemon nebula leaves running, so the
/// agents it spawns are reaped; `LANG` because the TUI draws Unicode and a
/// bare image's locale is POSIX.
fn create_args(container: &str, image: &str) -> Vec<String> {
    [
        "run",
        "-d",
        "--init",
        "--name",
        container,
        "--hostname",
        container,
        "--label",
        LABEL,
        "-e",
        "LANG=C.UTF-8",
        image,
        "sleep",
        "infinity",
    ]
    .map(String::from)
    .to_vec()
}

/// `docker exec -it` running `nebula ssh`'s remote script: `$1` the install
/// URL, `$2` empty for the container's `$HOME`, `$3` the settings bundle.
/// `-e TERM` and `-e COLORTERM` (values from here, skipped when unset) so the
/// TUI gets this terminal's colours, as ssh would send them.
fn exec_args(container: &str, install_url: &str, bundle: Option<&str>) -> Vec<OsString> {
    let mut args: Vec<OsString> = [
        "exec",
        "-it",
        "-e",
        "TERM",
        "-e",
        "COLORTERM",
        container,
        "sh",
        "-c",
        crate::ssh::REMOTE_SCRIPT,
        "nebula-sbx",
        install_url,
        "",
    ]
    .map(OsString::from)
    .to_vec();
    args.extend(bundle.map(OsString::from));
    args
}

#[cfg(test)]
mod tests {
    use super::*;

    const URL: &str = "https://example.com/install.sh";

    #[test]
    fn names_share_a_prefix() {
        assert_eq!(container_name(None), "nebula-sbx");
        assert_eq!(container_name(Some("client-a")), "nebula-sbx-client-a");
    }

    #[test]
    fn the_container_idles_until_nebula_execs_in() {
        let args = create_args("nebula-sbx", DEFAULT_IMAGE);
        assert!(args.ends_with(&[DEFAULT_IMAGE.into(), "sleep".into(), "infinity".into()]));
        assert!(args.contains(&"--init".to_string()));
    }

    #[test]
    fn exec_runs_the_ssh_script_with_its_parameters() {
        let args = exec_args("nebula-sbx", URL, None);
        let tail: Vec<OsString> = ["sh", "-c", crate::ssh::REMOTE_SCRIPT, "nebula-sbx", URL, ""]
            .map(OsString::from)
            .to_vec();
        assert!(args.ends_with(&tail), "{args:?}");

        let args = exec_args("nebula-sbx", URL, Some("eyJ4IjoxfQ=="));
        assert_eq!(args.last().unwrap(), "eyJ4IjoxfQ==");
    }
}
