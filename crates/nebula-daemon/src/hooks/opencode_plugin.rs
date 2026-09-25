//! nebula's MANAGED HOOKS for OpenCode: OpenCode runs TypeScript plugins,
//! not shell hooks, so an OpenCode AGENT's status signals are one file —
//! `<opencode config dir>/plugins/nebula.ts` — that maps OpenCode's server
//! events onto the HOOK EVENTS the daemon already understands and POSTs
//! them to `/api/hooks/opencode` (the injectable route: the
//! `UserPromptSubmit` reply body carries the AUTO-TITLE INSTRUCTION, and
//! the plugin appends it to that turn's system prompt through OpenCode's
//! `experimental.chat.system.transform` hook).
//!
//! The file is wholly nebula-owned (namespaced name, rewritten on every
//! spawn when its content drifts) and env-guarded, so a bare `opencode`
//! outside nebula loads it and does nothing. It lives in OpenCode's
//! *global* config dir (`~/.config/opencode`, or `$XDG_CONFIG_HOME/opencode`),
//! never the worktree: OpenCode globs `{plugin,plugins}/*.{ts,js}` under
//! every config directory it knows at startup with no trust prompt, and the
//! global one is on that list whatever else is (`OPENCODE_CONFIG_DIR` adds
//! a directory, it never replaces this one), so one file serves every
//! worktree.

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

use super::installer::install_unless_unchanged;

/// The XDG base every OpenCode config path hangs off, and its default.
pub const XDG_CONFIG_HOME_ENV: &str = "XDG_CONFIG_HOME";
const XDG_CONFIG_DEFAULT: &str = ".config";
/// OpenCode's dir under the XDG config base.
const OPENCODE_DIR: &str = "opencode";
/// The plugin directory OpenCode globs (it accepts `plugin` too; the
/// docs name this one).
const PLUGINS_DIR: &str = "plugins";
/// Namespaced so it can never collide with a user's own plugin.
const PLUGIN_FILE: &str = "nebula.ts";

/// The plugin, verbatim. Kept as TypeScript beside this module so it reads
/// (and diffs) as the program it is.
pub const PLUGIN_SOURCE: &str = include_str!("nebula_opencode_plugin.ts");

/// OpenCode's global config dir: `$XDG_CONFIG_HOME/opencode`, else
/// `~/.config/opencode` — the one directory it always scans for plugins.
pub fn opencode_config_dir() -> PathBuf {
    let home = nebula_core::env::home_dir().unwrap_or_default();
    let base = match nebula_core::env::non_empty(XDG_CONFIG_HOME_ENV) {
        Some(xdg) => PathBuf::from(xdg),
        None => home.join(XDG_CONFIG_DEFAULT),
    };
    base.join(OPENCODE_DIR)
}

/// Where the plugin lands under `config_dir`.
pub fn plugin_path(config_dir: &Path) -> PathBuf {
    config_dir.join(PLUGINS_DIR).join(PLUGIN_FILE)
}

/// Write the managed plugin into `<config_dir>/plugins/nebula.ts`, unless
/// the file already holds exactly this build's source — an unchanged mtime
/// keeps OpenCode's own bookkeeping of the directory quiet.
pub fn install(config_dir: &Path) -> Result<()> {
    let dir = config_dir.join(PLUGINS_DIR);
    install_unless_unchanged(&dir, PLUGIN_FILE, PLUGIN_SOURCE).with_context(|| {
        format!(
            "install opencode plugin into {}",
            dir.join(PLUGIN_FILE).display()
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plugin_is_written_and_rewritten_when_it_drifts() {
        let tmp = tempfile::tempdir().unwrap();
        install(tmp.path()).unwrap();
        let path = plugin_path(tmp.path());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), PLUGIN_SOURCE);
        // Wholly nebula-owned: a scribbled-on file is simply replaced.
        std::fs::write(&path, "user scribbles").unwrap();
        install(tmp.path()).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), PLUGIN_SOURCE);
        // Already current: left alone (mtime included).
        let before = std::fs::metadata(&path).unwrap().modified().unwrap();
        install(tmp.path()).unwrap();
        let after = std::fs::metadata(&path).unwrap().modified().unwrap();
        assert_eq!(before, after, "an unchanged plugin is not rewritten");
    }

    /// The contract the daemon side relies on, pinned against the source:
    /// env-guarded, on the opencode route, and every hook event the status
    /// machine reads for an OpenCode session is posted by name.
    #[test]
    fn plugin_source_is_env_guarded_and_speaks_the_hook_dialect() {
        let src = PLUGIN_SOURCE;
        for var in nebula_core::env::AGENT_SESSION_VARS {
            assert!(src.contains(var), "reads {var}");
        }
        assert!(
            src.contains("if (!AGENT_ID || !API_URL) return {};"),
            "env guard"
        );
        assert!(
            src.contains("/api/hooks/opencode?agentId="),
            "opencode route"
        );
        for event in [
            "UserPromptSubmit",
            "Stop",
            "PreToolUse",
            "PostToolUse",
            "PermissionRequest",
        ] {
            assert!(src.contains(&format!("\"{event}\"")), "posts {event}");
        }
        // The OpenCode events it listens for: the turn's run state, the
        // permission and question dialogs, and the subagent sessions it
        // keeps apart from the foreground.
        for event in [
            "session.created",
            "session.status",
            "session.idle",
            "permission.asked",
            "permission.replied",
            "question.asked",
            "question.replied",
            "question.rejected",
        ] {
            assert!(src.contains(&format!("\"{event}\"")), "listens for {event}");
        }
        // OpenCode's question tool is the one `status.rs` treats as waiting
        // on you.
        assert!(src.contains("const ASK_TOOL = \"question\";"));
        // The auto-title reply is read out of the shared envelope and rides
        // the system prompt hook.
        assert!(src.contains("hookSpecificOutput?.additionalContext"));
        assert!(src.contains("\"experimental.chat.system.transform\""));
        // The prompt rides the UserPromptSubmit body under the key the
        // receiver reads (RECENT PROMPTS), off the chat.message hook.
        assert!(src.contains("\"chat.message\""));
        assert!(src.contains("{ prompt }"));
        // A named export, the shape OpenCode's plugin docs load.
        assert!(src.contains("export const NebulaPlugin = async ("));
    }

    #[test]
    fn config_dir_prefers_xdg_over_home() {
        // Serialised with the other env-reading tests by running in one test.
        let tmp = tempfile::tempdir().unwrap();
        let saved = std::env::var(XDG_CONFIG_HOME_ENV).ok();
        std::env::set_var(XDG_CONFIG_HOME_ENV, tmp.path());
        assert_eq!(opencode_config_dir(), tmp.path().join("opencode"));
        std::env::set_var(XDG_CONFIG_HOME_ENV, "");
        assert_eq!(
            opencode_config_dir(),
            nebula_core::env::home_dir()
                .unwrap_or_default()
                .join(".config")
                .join("opencode")
        );
        match saved {
            Some(v) => std::env::set_var(XDG_CONFIG_HOME_ENV, v),
            None => std::env::remove_var(XDG_CONFIG_HOME_ENV),
        }
    }
}
