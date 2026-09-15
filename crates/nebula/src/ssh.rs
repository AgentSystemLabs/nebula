//! `nebula ssh HOST [PATH]`: open nebula on a remote machine.
//!
//! Execs `ssh -t HOST <cmd>` so the remote TUI renders in this terminal. The
//! remote command installs nebula via the published install script when it
//! isn't on the remote PATH, then launches it.
//!
//! Quoting: sshd hands the command string to the user's login shell, which
//! may be bash, zsh, or fish. The script below is a fixed constant with no
//! single quotes, backslashes, or newlines, wrapped once in '...'; user input
//! (install URL, start dir) is passed only as positional parameters, each
//! POSIX-single-quoted. csh/tcsh login shells are the one unsupported case.

use anyhow::{bail, Context, Result};
use std::os::unix::process::CommandExt;
use std::process::Command;

/// The opening half of every remote script: leave a usable `nebula` on the
/// remote PATH, installing it first when there is none. `$1` is the install
/// URL. A macro rather than a const because `concat!` only takes literals,
/// and [`crate::tunnel`] builds a different tail onto the same head.
macro_rules! install_prelude {
    () => {
        concat!(
            // sshd hands a remote command a bare PATH — no login shell runs,
            // so nothing the user configured applies. Prepend install.sh's
            // default NEBULA_INSTALL_DIR, and append both Homebrew prefixes:
            // on a macOS remote that is the only place ttyd (which
            // `nebula browser` needs) or a brew-installed nebula lives.
            "export PATH=\"$HOME/.local/bin:$PATH:/opt/homebrew/bin:/usr/local/bin\"; ",
            "if ! command -v nebula >/dev/null 2>&1; then ",
            "command -v curl >/dev/null 2>&1 || { ",
            "echo \"nebula: curl is required on the remote to install nebula\" >&2; exit 127; }; ",
            "echo \"nebula not found on remote; installing...\" >&2; ",
            "curl -fsSL \"$1\" | sh || exit 1; ",
            "fi; "
        )
    };
}
pub(crate) use install_prelude;

/// Hand the nebula the script starts the SETTINGS BUNDLE in positional
/// parameter `$n`, when the command carries one (see `nebula_tui::bundle`).
/// An environment variable rather than a `nebula config import` step: a
/// remote nebula too old to know the variable ignores it, where it would fail
/// on the unknown command — and the remote nebula works out its own data dir,
/// which a script writing the files would have to guess. A macro for the same
/// reason as [`install_prelude!`].
macro_rules! export_settings_bundle {
    ($n:literal) => {
        concat!(
            "[ -z \"$",
            $n,
            "\" ] || export NEBULA_IMPORT_BUNDLE=\"$",
            $n,
            "\"; "
        )
    };
}
pub(crate) use export_settings_bundle;

/// Runs under `sh -c` on the remote: $1 = install URL, $2 = start dir
/// (optional, may be empty; defaults to the remote $HOME), $3 = the settings
/// bundle (optional).
const REMOTE_SCRIPT: &str = concat!(
    install_prelude!(),
    "cd -- \"${2:-$HOME}\" || exit 1; ",
    export_settings_bundle!("3"),
    "exec nebula"
);

pub fn run_ssh(host: &str, path: Option<&str>, sync_config: bool) -> Result<()> {
    // Remember the destination for the TUI's `Shift+H` picker. Before the exec on
    // purpose (there is no after); a host that fails to connect still lists,
    // and `d` can drop it.
    nebula_tui::hosts::record(host, path);
    let bundle = sync_config.then(nebula_tui::bundle::for_remote).flatten();
    let cmd = remote_command(&crate::upgrade::install_url(), path, bundle.as_deref());
    // exec: ssh owns the tty from here and its exit status propagates
    // natively. Only returns on failure.
    let err = Command::new("ssh").args(["-t", "--", host, &cmd]).exec();
    if err.kind() == std::io::ErrorKind::NotFound {
        bail!("ssh not found on PATH — nebula ssh requires the OpenSSH client");
    }
    Err(err).context("failed to exec ssh")
}

fn remote_command(install_url: &str, path: Option<&str>, bundle: Option<&str>) -> String {
    let mut cmd = format!(
        "sh -c '{}' nebula-ssh {}",
        REMOTE_SCRIPT,
        shell_single_quote(install_url)
    );
    // A bundle is `$3`, so an absent start dir still takes its place, empty.
    if path.is_some() || bundle.is_some() {
        cmd.push(' ');
        cmd.push_str(&shell_single_quote(path.unwrap_or("")));
    }
    if let Some(bundle) = bundle {
        cmd.push(' ');
        cmd.push_str(&shell_single_quote(bundle));
    }
    cmd
}

/// POSIX-quote for a remote shell: `it's` -> `'it'\''s'`.
pub(crate) fn shell_single_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

#[cfg(test)]
mod tests {
    use super::*;

    const URL: &str = "https://example.com/install.sh";

    #[test]
    fn script_survives_single_quoting() {
        // The whole scheme rests on the script needing no escaping inside
        // '...' under any login shell.
        assert!(!REMOTE_SCRIPT.contains('\''));
        assert!(!REMOTE_SCRIPT.contains('\\'));
        assert!(!REMOTE_SCRIPT.contains('\n'));
    }

    #[test]
    fn no_path_defaults_to_remote_home() {
        let cmd = remote_command(URL, None, None);
        assert!(cmd.ends_with("nebula-ssh 'https://example.com/install.sh'"));
        assert!(cmd.contains("${2:-$HOME}"));
    }

    #[test]
    fn path_is_quoted() {
        let cmd = remote_command(URL, Some("/srv/my repo"), None);
        assert!(cmd.ends_with("'/srv/my repo'"));
    }

    #[test]
    fn path_with_single_quote_is_escaped() {
        let cmd = remote_command(URL, Some("/tmp/it's here"), None);
        assert!(cmd.ends_with("'/tmp/it'\\''s here'"));
    }

    #[test]
    fn a_bundle_rides_after_the_start_dir() {
        let cmd = remote_command(URL, None, Some("eyJ4IjoxfQ=="));
        assert!(
            cmd.ends_with("nebula-ssh 'https://example.com/install.sh' '' 'eyJ4IjoxfQ=='"),
            "{cmd}"
        );
        let cmd = remote_command(URL, Some("/srv/app"), Some("eyJ4IjoxfQ=="));
        assert!(cmd.ends_with("'/srv/app' 'eyJ4IjoxfQ=='"), "{cmd}");
    }

    /// The script spells the variable inside a string literal, so hold it to
    /// the constant the remote nebula reads.
    #[test]
    fn the_script_exports_the_variable_the_remote_nebula_reads() {
        let export = format!("export {}=\"$3\"", nebula_core::env::IMPORT_BUNDLE);
        assert!(REMOTE_SCRIPT.contains(&export), "{REMOTE_SCRIPT}");
    }

    /// The script run the way sshd runs it, with a stub `nebula` that
    /// records what it was handed: the bundle arrives as the variable, and a
    /// command without one leaves the variable unset.
    #[test]
    fn the_bundle_reaches_the_remote_nebula_in_its_environment() {
        use std::os::unix::fs::PermissionsExt;
        for bundle in [Some("eyJ4IjoxfQ=="), None] {
            let home = tempfile::tempdir().unwrap();
            let stub = home.path().join("stub");
            std::fs::create_dir(&stub).unwrap();
            let nebula = stub.join("nebula");
            std::fs::write(
                &nebula,
                "#!/bin/sh\nprintf '%s' \"${NEBULA_IMPORT_BUNDLE-unset}\" > \"$SEEN\"\n",
            )
            .unwrap();
            std::fs::set_permissions(&nebula, std::fs::Permissions::from_mode(0o755)).unwrap();
            let seen = home.path().join("seen");
            let mut args = vec!["-c", REMOTE_SCRIPT, "nebula-ssh", "file:///nonexistent", ""];
            args.extend(bundle);
            let status = Command::new("sh")
                .args(&args)
                .env("HOME", home.path())
                .env("PATH", format!("{}:/usr/bin:/bin", stub.display()))
                .env("SEEN", &seen)
                .env_remove(nebula_core::env::IMPORT_BUNDLE)
                .status()
                .expect("sh");
            assert!(status.success(), "{bundle:?}");
            assert_eq!(
                std::fs::read_to_string(&seen).unwrap(),
                bundle.unwrap_or("unset")
            );
        }
    }

    #[test]
    fn single_quote_edge_cases() {
        assert_eq!(shell_single_quote(""), "''");
        assert_eq!(shell_single_quote("plain"), "'plain'");
        assert_eq!(shell_single_quote("'''"), "''\\'''\\'''\\'''");
    }
}
