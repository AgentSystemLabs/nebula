//! Finding the external CLIs orbit integrates with (graphify, agent-browser).
//!
//! Users are only expected to have `uv` and `npm`: a tool that isn't
//! installed is run through `uvx` (Python packages) or `npx` (npm packages),
//! which fetch and cache it on first use. An installed copy wins when there
//! is one. [`Invocation`] says which, and how a shell types it, so the
//! guidance and permission rules handed to Claude match what will run.
//!
//! The DAEMON may run with a thin PATH (launchd on macOS), so after PATH the
//! usual install locations are tried: Homebrew's, `/usr/local/bin` and
//! `~/.local/bin` (where `uv tool` and `pipx` put things). A script whose
//! `#!` interpreter no longer exists — a pip shim stranded by a Homebrew
//! Python upgrade — is skipped for the next copy that can actually run.
//! Looked up fresh each call, so installing a tool later just works.

use std::path::{Path, PathBuf};

/// The PyPI package that provides the `graphify` command.
pub const GRAPHIFY_PACKAGE: &str = "graphifyy";
/// The npm package that provides the `agent-browser` command.
pub const AGENT_BROWSER_PACKAGE: &str = "agent-browser";

/// Where an integrated tool comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Via {
    /// A copy on PATH or in a usual install dir.
    Installed,
    /// Fetched and cached by `uvx` on first use.
    Uvx,
    /// Fetched and cached by `npx` on first use.
    Npx,
}

/// How to run one integrated tool.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Invocation {
    /// The executable to spawn: the tool itself, or `uvx` / `npx`.
    pub program: PathBuf,
    /// Arguments that come before the tool's own (`--from graphifyy graphify`).
    pub prefix: Vec<String>,
    /// How a shell types it — what an agent is told to run.
    pub shell: String,
    pub via: Via,
}

impl Invocation {
    /// A ready-to-extend command: the program and its prefix.
    pub fn command(&self) -> std::process::Command {
        let mut command = std::process::Command::new(&self.program);
        command.args(&self.prefix);
        command
    }

    /// Where it comes from, for people: `installed at …`, `via uvx`.
    pub fn describe(&self) -> String {
        match self.via {
            Via::Installed => format!("installed at {}", self.program.display()),
            Via::Uvx => "via uvx (fetched and cached on first use)".into(),
            Via::Npx => "via npx (fetched and cached on first use)".into(),
        }
    }
}

/// graphify: an installed copy, else `uvx --from graphifyy graphify`.
pub fn graphify() -> Option<Invocation> {
    if let Some(program) = find("graphify") {
        return Some(Invocation {
            program,
            prefix: Vec::new(),
            shell: "graphify".into(),
            via: Via::Installed,
        });
    }
    find("uvx").map(|program| Invocation {
        program,
        prefix: vec!["--from".into(), GRAPHIFY_PACKAGE.into(), "graphify".into()],
        shell: format!("uvx --from {GRAPHIFY_PACKAGE} graphify"),
        via: Via::Uvx,
    })
}

/// agent-browser: an installed copy, else `npx -y agent-browser`.
pub fn agent_browser() -> Option<Invocation> {
    if let Some(program) = find("agent-browser") {
        return Some(Invocation {
            program,
            prefix: Vec::new(),
            shell: "agent-browser".into(),
            via: Via::Installed,
        });
    }
    find("npx").map(|program| Invocation {
        program,
        prefix: vec!["-y".into(), AGENT_BROWSER_PACKAGE.into()],
        shell: format!("npx -y {AGENT_BROWSER_PACKAGE}"),
        via: Via::Npx,
    })
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
}
