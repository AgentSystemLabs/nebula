//! nebula's MANAGED HOOKS for pi (pi.dev): pi runs TypeScript extensions,
//! not shell hooks, so a pi AGENT's status signals are one file —
//! `<pi agent dir>/extensions/nebula.ts` — that maps pi's lifecycle events
//! onto the HOOK EVENTS the daemon already understands and POSTs them to
//! `/api/hooks/pi` (the injectable route: the `UserPromptSubmit` reply body
//! carries the AUTO-TITLE INSTRUCTION, and the extension appends it to that
//! run's system prompt).
//!
//! The file is wholly nebula-owned (namespaced name, rewritten on every
//! spawn when its content drifts) and env-guarded, so a bare `pi` outside
//! nebula loads it and does nothing. It lives in pi's *global* agent dir
//! (`~/.pi/agent`, or `$PI_CODING_AGENT_DIR`), never the worktree: pi loads
//! global extensions without a trust prompt, while a project-local
//! `.pi/extensions/` asks in every fresh checkout — the same trap codex's
//! per-worktree hooks fell into (see `installer.rs`).

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

use super::installer::write_text_atomic;

/// pi's config dir under `$HOME`, and the agent dir inside it.
const PI_DIR: &str = ".pi";
const PI_AGENT_SUBDIR: &str = "agent";
const EXTENSIONS_DIR: &str = "extensions";
/// Namespaced so it can never collide with a user's own extension.
const EXTENSION_FILE: &str = "nebula.ts";
/// pi's own override for its agent dir; honoured so an isolated pi (a test,
/// a second profile) finds the extension where it looks for the rest.
pub const AGENT_DIR_ENV: &str = "PI_CODING_AGENT_DIR";

/// The extension, verbatim. Kept as TypeScript beside this module so it
/// reads (and diffs) as the program it is.
pub const EXTENSION_SOURCE: &str = include_str!("nebula_pi_extension.ts");

/// pi's agent dir: `$PI_CODING_AGENT_DIR` (a leading `~` expanded, as pi
/// does), else `~/.pi/agent`.
pub fn pi_agent_dir() -> PathBuf {
    let home = nebula_core::env::home_dir().unwrap_or_default();
    match nebula_core::env::non_empty(AGENT_DIR_ENV) {
        Some(dir) => match dir.strip_prefix("~/") {
            Some(rest) => home.join(rest),
            None if dir == "~" => home,
            None => PathBuf::from(dir),
        },
        None => home.join(PI_DIR).join(PI_AGENT_SUBDIR),
    }
}

/// Where the extension lands under `agent_dir`.
pub fn extension_path(agent_dir: &Path) -> PathBuf {
    agent_dir.join(EXTENSIONS_DIR).join(EXTENSION_FILE)
}

/// Write the managed extension into `<agent_dir>/extensions/nebula.ts`,
/// unless the file already holds exactly this build's source — pi is not
/// told to reload, and an unchanged mtime keeps its extension cache warm.
pub fn install(agent_dir: &Path) -> Result<()> {
    let dir = agent_dir.join(EXTENSIONS_DIR);
    let path = dir.join(EXTENSION_FILE);
    if let Ok(existing) = std::fs::read_to_string(&path) {
        if existing == EXTENSION_SOURCE {
            return Ok(());
        }
    }
    write_text_atomic(&dir, EXTENSION_FILE, EXTENSION_SOURCE)
        .with_context(|| format!("install pi extension into {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extension_is_written_and_rewritten_when_it_drifts() {
        let tmp = tempfile::tempdir().unwrap();
        install(tmp.path()).unwrap();
        let path = extension_path(tmp.path());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), EXTENSION_SOURCE);
        // Wholly nebula-owned: a scribbled-on file is simply replaced.
        std::fs::write(&path, "user scribbles").unwrap();
        install(tmp.path()).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), EXTENSION_SOURCE);
        // Already current: left alone (mtime included).
        let before = std::fs::metadata(&path).unwrap().modified().unwrap();
        install(tmp.path()).unwrap();
        let after = std::fs::metadata(&path).unwrap().modified().unwrap();
        assert_eq!(before, after, "an unchanged extension is not rewritten");
    }

    /// The contract the daemon side relies on, pinned against the source:
    /// env-guarded, on the pi route, and every hook event the status
    /// machine reads for a pi session is posted by name.
    #[test]
    fn extension_source_is_env_guarded_and_speaks_the_hook_dialect() {
        let src = EXTENSION_SOURCE;
        for var in nebula_core::env::AGENT_SESSION_VARS {
            assert!(src.contains(var), "reads {var}");
        }
        assert!(
            src.contains("if (!AGENT_ID || !API_URL) return;"),
            "env guard"
        );
        assert!(src.contains("/api/hooks/pi?agentId="), "pi route");
        for event in [
            "SessionStart",
            "UserPromptSubmit",
            "Stop",
            "PreToolUse",
            "PostToolUse",
            "PermissionRequest",
        ] {
            assert!(src.contains(&format!("\"{event}\"")), "posts {event}");
        }
        // pi's question tool is the one `status.rs` treats as waiting on you.
        assert!(src.contains("const ASK_TOOL = \"ask_question\";"));
        // The auto-title reply is read out of the shared envelope.
        assert!(src.contains("hookSpecificOutput?.additionalContext"));
    }

    #[test]
    fn agent_dir_prefers_env_over_home() {
        // Serialised with the other env-reading tests by running in one test.
        let tmp = tempfile::tempdir().unwrap();
        let saved = std::env::var(AGENT_DIR_ENV).ok();
        std::env::set_var(AGENT_DIR_ENV, tmp.path());
        assert_eq!(pi_agent_dir(), tmp.path());
        std::env::set_var(AGENT_DIR_ENV, "~/pi-profile");
        assert_eq!(
            pi_agent_dir(),
            nebula_core::env::home_dir()
                .unwrap_or_default()
                .join("pi-profile")
        );
        std::env::set_var(AGENT_DIR_ENV, "");
        assert_eq!(
            pi_agent_dir(),
            nebula_core::env::home_dir()
                .unwrap_or_default()
                .join(".pi")
                .join("agent")
        );
        match saved {
            Some(v) => std::env::set_var(AGENT_DIR_ENV, v),
            None => std::env::remove_var(AGENT_DIR_ENV),
        }
    }
}
