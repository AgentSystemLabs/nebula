//! Finding the external CLIs orbit integrates with (graphify, agent-browser).
//!
//! The DAEMON may run with a thin PATH (launchd on macOS), so after PATH the
//! usual install locations are tried: Homebrew's, `/usr/local/bin` and
//! `~/.local/bin` (where `uv tool` and `pipx` put things). A script whose
//! `#!` interpreter no longer exists — a pip shim stranded by a Homebrew
//! Python upgrade — is skipped for the next copy that can actually run.
//! Looked up fresh each call, so installing a tool later just works.

use std::path::{Path, PathBuf};

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
