//! The external CLIs orbit integrates with — graphify and agent-browser —
//! found, and installed once per machine when they are missing.
//!
//! Users are only expected to have `uv` and `npm`. The first time a tool is
//! needed and isn't there, orbit installs it for good — `uv tool install
//! graphifyy`, `npm install -g agent-browser` (then that tool's one-time
//! browser download) — and from then on finds the installed copy like any
//! other command. Nothing is reinstalled on later runs. A failed install
//! (offline, no permission) is not retried for a while, so a turn ending
//! every minute never turns into an install attempt every minute.
//!
//! Lookup searches PATH, then the usual install dirs — the DAEMON may run
//! with a thin PATH (launchd on macOS): Homebrew's, `/usr/local/bin` and
//! `~/.local/bin` (where `uv tool` puts commands). A script whose `#!`
//! interpreter no longer exists — a pip shim stranded by a Homebrew Python
//! upgrade — is skipped for the next copy that can actually run.

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// How long a failed install waits before it may be tried again.
const RETRY_AFTER: Duration = Duration::from_secs(10 * 60);

/// Which package manager installs a tool.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Installer {
    /// `uv tool install <package>`.
    Uv,
    /// `npm install -g <package>`.
    Npm,
}

/// One integrated tool.
#[derive(Debug)]
pub struct Tool {
    /// The command it provides.
    pub name: &'static str,
    /// The package that provides it.
    pub package: &'static str,
    pub installer: Installer,
    /// Arguments run once right after a fresh install (agent-browser's
    /// browser download), if any.
    pub post_install: &'static [&'static str],
    /// The last failed install, so it isn't retried on every call.
    failed_at: Mutex<Option<Instant>>,
}

/// graphify: the code knowledge graph.
pub static GRAPHIFY: Tool = Tool {
    name: "graphify",
    package: "graphifyy",
    installer: Installer::Uv,
    post_install: &[],
    failed_at: Mutex::new(None),
};

/// agent-browser: browser automation for agents.
pub static AGENT_BROWSER: Tool = Tool {
    name: "agent-browser",
    package: "agent-browser",
    installer: Installer::Npm,
    post_install: &["install"],
    failed_at: Mutex::new(None),
};

/// An installed tool: where it is, and how a shell types it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Invocation {
    pub program: PathBuf,
    /// What an agent is told to run: the bare name when its directory is on
    /// PATH, else the full path, so the command works either way.
    pub shell: String,
}

impl Invocation {
    fn at(program: PathBuf) -> Self {
        let on_path = program.parent().is_some_and(|dir| {
            std::env::var_os("PATH")
                .is_some_and(|p| std::env::split_paths(&p).any(|entry| entry == dir))
        });
        let shell = match program.file_name() {
            Some(name) if on_path => name.to_string_lossy().into_owned(),
            _ => program.display().to_string(),
        };
        Self { program, shell }
    }

    pub fn command(&self) -> std::process::Command {
        std::process::Command::new(&self.program)
    }
}

impl Tool {
    /// The installed copy, if there is one.
    pub fn installed(&self) -> Option<Invocation> {
        find(self.name).map(Invocation::at)
    }

    /// The package manager that would install it, when this machine has it.
    pub fn installer_program(&self) -> Option<PathBuf> {
        find(match self.installer {
            Installer::Uv => "uv",
            Installer::Npm => "npm",
        })
    }

    /// True when it is installed or can be installed here.
    pub fn available(&self) -> bool {
        self.installed().is_some() || self.installer_program().is_some()
    }

    /// The install command, for people: `uv tool install graphifyy`.
    pub fn install_hint(&self) -> String {
        match self.installer {
            Installer::Uv => format!("uv tool install {}", self.package),
            Installer::Npm => format!("npm install -g {}", self.package),
        }
    }

    /// The installed copy — installing it first, once, when it is missing.
    /// Blocking: call it off any loop that must stay responsive.
    pub fn ensure(&self) -> Result<Invocation, String> {
        if let Some(found) = self.installed() {
            return Ok(found);
        }
        // One install at a time in this process; a second caller waits and
        // then finds the copy the first one installed.
        let mut failed_at = self.failed_at.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(found) = self.installed() {
            return Ok(found);
        }
        if failed_at.is_some_and(|at| at.elapsed() < RETRY_AFTER) {
            return Err(format!(
                "{} isn't installed and the last install failed — try `{}`",
                self.name,
                self.install_hint()
            ));
        }
        let result = self.install();
        *failed_at = result.is_err().then(Instant::now);
        result
    }

    fn install(&self) -> Result<Invocation, String> {
        let Some(installer) = self.installer_program() else {
            return Err(match self.installer {
                Installer::Uv => format!("{} needs uv — install uv (docs.astral.sh/uv)", self.name),
                Installer::Npm => format!("{} needs npm — install Node.js", self.name),
            });
        };
        let mut command = std::process::Command::new(installer);
        match self.installer {
            Installer::Uv => command.args(["tool", "install", self.package]),
            Installer::Npm => command.args(["install", "-g", self.package]),
        };
        let status = command
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map_err(|e| format!("couldn't run `{}`: {e}", self.install_hint()))?;
        if !status.success() {
            return Err(format!("`{}` failed ({status})", self.install_hint()));
        }
        let found = self.installed().ok_or_else(|| {
            format!(
                "`{}` finished but `{}` isn't on PATH or in ~/.local/bin",
                self.install_hint(),
                self.name
            )
        })?;
        if !self.post_install.is_empty() {
            // Best effort: the tool works for what doesn't need it, and its
            // own error names the step if a later command does.
            let _ = found
                .command()
                .args(self.post_install)
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status();
        }
        Ok(found)
    }
}

/// The first runnable `name` on PATH or in the usual install locations.
pub fn find(name: &str) -> Option<PathBuf> {
    let mut dirs: Vec<PathBuf> = std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).collect())
        .unwrap_or_default();
    dirs.extend(["/opt/homebrew/bin", "/usr/local/bin"].map(PathBuf::from));
    if let Some(home) = std::env::var_os("HOME") {
        dirs.push(PathBuf::from(home).join(".local/bin"));
    }
    dirs.into_iter()
        .map(|dir| dir.join(name))
        .find(|candidate| candidate.is_file() && interpreter_present(candidate))
}

/// False only for a script whose `#!` names an absolute interpreter that no
/// longer exists; binaries and `#!/usr/bin/env …` scripts pass.
pub fn interpreter_present(script: &Path) -> bool {
    use std::io::Read;
    let mut head = [0u8; 256];
    let n = std::fs::File::open(script)
        .and_then(|mut f| f.read(&mut head))
        .unwrap_or(0);
    let Some(rest) = head[..n].strip_prefix(b"#!") else {
        return true;
    };
    let line = rest.split(|&b| b == b'\n').next().unwrap_or_default();
    match String::from_utf8_lossy(line).split_whitespace().next() {
        Some(interp) if interp.starts_with('/') => Path::new(interp).exists(),
        _ => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_script_whose_interpreter_is_gone_is_skipped() {
        let tmp = tempfile::tempdir().unwrap();
        let stale = tmp.path().join("stale");
        std::fs::write(&stale, "#!/no/such/python3.14\nimport sys\n").unwrap();
        let env = tmp.path().join("env");
        std::fs::write(&env, "#!/usr/bin/env python3\n").unwrap();
        let binary = tmp.path().join("bin");
        std::fs::write(&binary, [0x7f, b'E', b'L', b'F']).unwrap();
        assert!(!interpreter_present(&stale));
        assert!(interpreter_present(&env));
        assert!(interpreter_present(&binary));
    }

    #[test]
    fn install_hints_name_the_package_manager() {
        assert_eq!(GRAPHIFY.install_hint(), "uv tool install graphifyy");
        assert_eq!(AGENT_BROWSER.install_hint(), "npm install -g agent-browser");
    }

    #[test]
    fn a_tool_off_path_is_typed_by_its_full_path() {
        let off = Invocation::at(PathBuf::from("/definitely/not/on/path/graphify"));
        assert_eq!(off.shell, "/definitely/not/on/path/graphify");
    }
}
