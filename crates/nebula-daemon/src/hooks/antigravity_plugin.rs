//! nebula's MANAGED HOOKS for Antigravity (`agy`): agy runs shell hooks
//! out of a `hooks.json`, but only from a customization root — a
//! worktree's `.agents/` (checked in, shared with the team) or the global
//! `~/.gemini/config/` — so an Antigravity AGENT's status signals are one
//! nebula-owned plugin, `<gemini config dir>/plugins/nebula/`, holding a
//! `plugin.json` marker and the `hooks.json` agy loads from a plugin root.
//! Global plugins load with no trust prompt and serve every worktree.
//!
//! Each hook POSTs agy's own payload (camelCase: `conversationId`,
//! `workspacePaths`, `invocationNum`) to `/api/hooks/antigravity` under
//! agy's event name, and prints the daemon's reply: agy reads every hook's
//! stdout as JSON, so the route answers each event with an agy-shaped
//! object — `{}`, or `injectSteps` carrying nebula's guidance (at
//! SessionStart) and the AUTO-TITLE INSTRUCTION (on a turn's first
//! PreInvocation). Verified on agy 1.4.2: hooks inherit the CLI's env, a
//! resume (`--conversation`) fires no SessionStart, and `invocationNum`
//! restarts at 0 on every turn.
//!
//! No tool hooks: a PreToolUse reply must carry a permission decision,
//! and any answer nebula gave would override agy's own policy.
//!
//! Env-guarded like every managed hook, so a bare `agy` outside nebula
//! loads the plugin and gets `{}` back without a request being made.

use anyhow::{Context, Result};
use serde_json::json;
use std::path::{Path, PathBuf};

use super::installer::{env_guard, hook_curl, install_unless_unchanged};

/// agy's global customization root under `$HOME`.
const GEMINI_DIR: &str = ".gemini";
const CONFIG_SUBDIR: &str = "config";
const PLUGINS_DIR: &str = "plugins";
/// Namespaced so it can never collide with a user's own plugin.
const PLUGIN_DIR: &str = "nebula";
const MANIFEST_FILE: &str = "plugin.json";
const HOOKS_FILE: &str = "hooks.json";
/// The route the hooks POST to (`/api/hooks/<endpoint>`).
pub const ENDPOINT: &str = "antigravity";
/// The agy events nebula listens to. All three are flat lists of handlers
/// (no tool matcher).
pub const EVENTS: &[&str] = &["SessionStart", "PreInvocation", "Stop"];
/// Seconds agy waits on a hook before giving up on it (its default is 30,
/// and hooks block the agent loop). curl's own cap is shorter.
const HOOK_TIMEOUT_SECS: u32 = 5;

/// agy's global config dir: `~/.gemini/config`.
pub fn gemini_config_dir() -> PathBuf {
    nebula_core::env::home_dir()
        .unwrap_or_default()
        .join(GEMINI_DIR)
        .join(CONFIG_SUBDIR)
}

/// The plugin directory under `config_dir`.
pub fn plugin_dir(config_dir: &Path) -> PathBuf {
    config_dir.join(PLUGINS_DIR).join(PLUGIN_DIR)
}

/// One hook's shell command: inert `{}` outside nebula, else the POST,
/// whose reply is printed only when it is a JSON object — a failed or
/// empty request still hands agy valid JSON.
fn hook_command(event: &str) -> String {
    format!(
        "if {}; then printf '{{}}'; exit 0; fi; out=$({} 2>/dev/null); \
         case \"$out\" in '{{'*) printf '%s' \"$out\" ;; *) printf '{{}}' ;; esac",
        env_guard(),
        hook_curl(ENDPOINT, event)
    )
}

/// The managed `hooks.json`: one named hook, one handler per event.
pub fn hooks_source() -> String {
    let mut spec = serde_json::Map::new();
    for event in EVENTS {
        spec.insert(
            (*event).to_string(),
            json!([{
                "type": "command",
                "command": hook_command(event),
                "timeout": HOOK_TIMEOUT_SECS,
            }]),
        );
    }
    let hooks = json!({ "nebula": spec });
    format!(
        "{}\n",
        serde_json::to_string_pretty(&hooks).expect("static JSON")
    )
}

fn manifest_source() -> String {
    format!(
        "{}\n",
        serde_json::to_string_pretty(&json!({ "name": PLUGIN_DIR })).expect("static JSON")
    )
}

/// Write the managed plugin into `<config_dir>/plugins/nebula/`, leaving
/// each file alone when it already holds exactly this build's content.
pub fn install(config_dir: &Path) -> Result<()> {
    let dir = plugin_dir(config_dir);
    install_unless_unchanged(&dir, MANIFEST_FILE, &manifest_source())
        .and_then(|()| install_unless_unchanged(&dir, HOOKS_FILE, &hooks_source()))
        .with_context(|| format!("install antigravity plugin into {}", dir.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plugin_is_written_and_rewritten_when_it_drifts() {
        let tmp = tempfile::tempdir().unwrap();
        install(tmp.path()).unwrap();
        let dir = plugin_dir(tmp.path());
        let hooks = dir.join(HOOKS_FILE);
        assert_eq!(std::fs::read_to_string(&hooks).unwrap(), hooks_source());
        let manifest: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(dir.join(MANIFEST_FILE)).unwrap())
                .unwrap();
        assert_eq!(manifest["name"], "nebula");
        // Wholly nebula-owned: a scribbled-on file is simply replaced.
        std::fs::write(&hooks, "user scribbles").unwrap();
        install(tmp.path()).unwrap();
        assert_eq!(std::fs::read_to_string(&hooks).unwrap(), hooks_source());
        // Already current: left alone (mtime included).
        let before = std::fs::metadata(&hooks).unwrap().modified().unwrap();
        install(tmp.path()).unwrap();
        let after = std::fs::metadata(&hooks).unwrap().modified().unwrap();
        assert_eq!(before, after, "an unchanged plugin is not rewritten");
    }

    /// The contract the daemon side relies on: agy's flat event lists,
    /// env-guarded, on the antigravity route, always printing JSON.
    #[test]
    fn hooks_are_env_guarded_and_post_every_event_to_the_route() {
        let hooks: serde_json::Value = serde_json::from_str(&hooks_source()).unwrap();
        let spec = hooks["nebula"].as_object().unwrap();
        assert_eq!(spec.len(), EVENTS.len());
        for event in EVENTS {
            let handlers = spec[*event].as_array().unwrap();
            assert_eq!(handlers.len(), 1, "{event} is a flat list");
            let command = handlers[0]["command"].as_str().unwrap();
            for var in nebula_core::env::AGENT_SESSION_VARS {
                assert!(command.contains(var), "{event} reads {var}");
            }
            assert!(command.starts_with("if [ -z "), "{event} env guard first");
            assert!(command.contains(&format!(
                "/api/hooks/antigravity?agentId=$NEBULA_AGENT_ID&hookEvent={event}"
            )));
            assert_eq!(handlers[0]["timeout"], HOOK_TIMEOUT_SECS);
        }
    }

    /// The command really runs under `sh -c` as agy does: outside nebula
    /// it prints `{}` and never reaches curl; with a dead daemon it still
    /// prints `{}`.
    #[test]
    fn hook_command_always_prints_a_json_object() {
        let run = |env: &[(&str, &str)]| {
            let mut cmd = std::process::Command::new("sh");
            cmd.arg("-c").arg(hook_command("Stop"));
            for var in nebula_core::env::AGENT_SESSION_VARS {
                cmd.env_remove(var);
            }
            cmd.envs(env.iter().copied());
            cmd.stdin(std::process::Stdio::null());
            let out = cmd.output().unwrap();
            String::from_utf8(out.stdout).unwrap()
        };
        assert_eq!(run(&[]), "{}");
        assert_eq!(
            run(&[
                ("NEBULA_AGENT_ID", "a1"),
                // Port 9 (discard) on loopback: refused at once.
                ("NEBULA_API_URL", "http://127.0.0.1:9"),
                ("NEBULA_API_TOKEN", "t"),
            ]),
            "{}"
        );
    }
}
