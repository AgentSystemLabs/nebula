//! Managed-hook installation into a worktree's agent-CLI config:
//! `.claude/settings.local.json` (Claude Code), `.codex/hooks.json`
//! (Codex CLI), or `.cursor/hooks.json` (Cursor CLI).
//!
//! Claude and Codex share one hooks dialect (PascalCase event names, groups
//! of `{"hooks": [{"type": "command", ...}]}`). Cursor speaks its own
//! (verified against cursor-agent 2026.08 + cursor.com/docs/agent/hooks):
//! camelCase event names (`beforeSubmitPrompt`, `stop`, ...), flat
//! `{"command": ...}` entries, a required top-level `"version": 1`, and each
//! hook must print a JSON response (`{"continue": true}`) to stdout or
//! gating events fall back to fail-open error handling.
//!
//! Rules (learned the hard way in mission-control):
//! - MERGE, never replace: user hooks are preserved untouched.
//! - Our groups carry `_nebulaManaged: true` and are stripped + rebuilt on
//!   every spawn, so upgrades never accumulate duplicates. A legacy-signature
//!   check (command contains our endpoint + env var) catches untagged strays.
//! - A corrupt file ABORTS the install — never clobber user data.
//! - Commands are env-guarded, so the hooks are inert when the user runs
//!   `claude`/`codex`/`cursor-agent` outside nebula (no NEBULA_* in env →
//!   exit 0).
//!
//! Codex caveat: project-local `.codex/hooks.json` only loads when the
//! project is trusted, and codex asks once per hook on first run (approval
//! recorded in `~/.codex/config.toml [hooks.state]`). Until approved, the
//! agent simply reports no status — same degradation as a failed install.

use anyhow::{bail, Context, Result};
use serde_json::{json, Map, Value};
use std::path::Path;

/// (hook event, optional matcher)
const CLAUDE_EVENTS: &[(&str, Option<&str>)] = &[
    ("UserPromptSubmit", None),
    ("Stop", None),
    ("SessionStart", None),
    ("PermissionRequest", None),
    ("Notification", Some("permission_prompt")),
    ("PreToolUse", Some("AskUserQuestion")),
    ("PostToolUse", Some("AskUserQuestion")),
    ("SubagentStart", None),
    ("SubagentStop", None),
];

/// Codex has no Notification hook and no AskUserQuestion tool; its native
/// PermissionRequest covers the waiting-on-user state.
const CODEX_EVENTS: &[(&str, Option<&str>)] = &[
    ("UserPromptSubmit", None),
    ("Stop", None),
    ("SessionStart", None),
    ("PermissionRequest", None),
    ("SubagentStart", None),
    ("SubagentStop", None),
];

/// (cursor hook event, nebula hookEvent query value). Cursor has no
/// PermissionRequest hook and nebula always runs cursor-agent with
/// `--force`, so waiting-on-user is simply not detectable — busy/idle is.
/// `sessionEnd` is skipped: PTY-exit synthetics already cover agent death.
const CURSOR_EVENTS: &[(&str, &str)] = &[
    ("sessionStart", "SessionStart"),
    ("beforeSubmitPrompt", "UserPromptSubmit"),
    ("stop", "Stop"),
    ("subagentStart", "SubagentStart"),
    ("subagentStop", "SubagentStop"),
];

fn hook_command(endpoint: &str, event: &str) -> String {
    format!(
        "if [ -z \"$NEBULA_AGENT_ID\" ] || [ -z \"$NEBULA_API_URL\" ]; then exit 0; fi; \
         curl -sS -m 3 -X POST -H \"Authorization: Bearer $NEBULA_API_TOKEN\" \
         -H \"Content-Type: application/json\" --data-binary @- \
         \"$NEBULA_API_URL/api/hooks/{endpoint}?agentId=$NEBULA_AGENT_ID&hookEvent={event}\" \
         >/dev/null 2>&1 || true"
    )
}

/// Cursor variant: the payload arrives on stdin like Claude's, but cursor
/// expects a JSON response on stdout — `{"continue": true}` keeps gating
/// events (beforeSubmitPrompt) flowing and is ignored by the rest.
fn cursor_hook_command(event: &str) -> String {
    format!(
        "if [ -z \"$NEBULA_AGENT_ID\" ] || [ -z \"$NEBULA_API_URL\" ]; then \
         printf '{{\"continue\": true}}\\n'; exit 0; fi; \
         curl -sS -m 3 -X POST -H \"Authorization: Bearer $NEBULA_API_TOKEN\" \
         -H \"Content-Type: application/json\" --data-binary @- \
         \"$NEBULA_API_URL/api/hooks/cursor?agentId=$NEBULA_AGENT_ID&hookEvent={event}\" \
         >/dev/null 2>&1 || true; printf '{{\"continue\": true}}\\n'"
    )
}

fn is_nebula_command(cmd: Option<&Value>) -> bool {
    cmd.and_then(Value::as_str)
        .map(|c| c.contains("/api/hooks/") && c.contains("NEBULA_AGENT_ID"))
        .unwrap_or(false)
}

fn is_nebula_group(group: &Value) -> bool {
    if group.get("_nebulaManaged").and_then(Value::as_bool) == Some(true) {
        return true;
    }
    // Legacy/untagged detection by command signature — nested Claude/Codex
    // shape and flat Cursor shape both.
    if is_nebula_command(group.get("command")) {
        return true;
    }
    group
        .get("hooks")
        .and_then(Value::as_array)
        .map(|hooks| hooks.iter().any(|h| is_nebula_command(h.get("command"))))
        .unwrap_or(false)
}

fn managed_group(endpoint: &str, event: &str, matcher: Option<&str>) -> Value {
    let mut group = Map::new();
    if let Some(m) = matcher {
        group.insert("matcher".into(), json!(m));
    }
    group.insert(
        "hooks".into(),
        json!([{ "type": "command", "command": hook_command(endpoint, event) }]),
    );
    group.insert("_nebulaManaged".into(), json!(true));
    Value::Object(group)
}

/// Merge nebula's managed hooks for Claude Code into
/// `<cwd>/.claude/settings.local.json`.
pub fn install_claude_hooks(cwd: &Path) -> Result<()> {
    install_managed_hooks(
        cwd,
        ".claude",
        "settings.local.json",
        "claude",
        CLAUDE_EVENTS,
    )
}

/// Merge nebula's managed hooks for Codex into `<cwd>/.codex/hooks.json`.
pub fn install_codex_hooks(cwd: &Path) -> Result<()> {
    install_managed_hooks(cwd, ".codex", "hooks.json", "codex", CODEX_EVENTS)
}

/// Merge nebula's managed hooks for Cursor into `<cwd>/.cursor/hooks.json`,
/// in Cursor's own dialect. Also migrates away the Claude-shaped groups an
/// older nebula wrote there (events Cursor never fires — the original
/// "cursor status never updates" bug).
pub fn install_cursor_hooks(cwd: &Path) -> Result<()> {
    let dir = cwd.join(".cursor");
    let path = dir.join("hooks.json");
    let mut root = load_hooks_root(&path)?;

    let Some(root_obj) = root.as_object_mut() else {
        bail!(
            "{} is not a JSON object — refusing to modify it",
            path.display()
        );
    };
    // Cursor requires a top-level version; never overwrite an existing one.
    root_obj.entry("version").or_insert(json!(1));
    let hooks = root_obj.entry("hooks").or_insert_with(|| json!({}));
    let Some(hooks_obj) = hooks.as_object_mut() else {
        bail!(
            "\"hooks\" in {} is not an object — refusing to modify it",
            path.display()
        );
    };

    // Purge nebula groups under EVERY key (not just the ones we reinstall):
    // stale PascalCase keys from the old Claude-shaped install must go, and
    // event keys we may drop in the future must not linger.
    for (_, groups) in hooks_obj.iter_mut() {
        if let Some(arr) = groups.as_array_mut() {
            arr.retain(|g| !is_nebula_group(g));
        }
    }
    hooks_obj.retain(|_, groups| groups.as_array().map(|a| !a.is_empty()).unwrap_or(true));

    for (cursor_event, nebula_event) in CURSOR_EVENTS {
        let groups = hooks_obj
            .entry(cursor_event.to_string())
            .or_insert_with(|| json!([]));
        let Some(groups_arr) = groups.as_array_mut() else {
            bail!(
                "hooks.{cursor_event} in {} is not an array — refusing to modify it",
                path.display()
            );
        };
        groups_arr.push(json!({
            "command": cursor_hook_command(nebula_event),
            "_nebulaManaged": true,
        }));
    }

    write_hooks_root(&dir, "hooks.json", &root)
}

fn load_hooks_root(path: &Path) -> Result<Value> {
    if !path.exists() {
        return Ok(json!({}));
    }
    let text =
        std::fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    if text.trim().is_empty() {
        return Ok(json!({}));
    }
    match serde_json::from_str(&text) {
        Ok(v) => Ok(v),
        Err(e) => bail!(
            "{} is not valid JSON ({e}) — refusing to modify it; fix or remove the file",
            path.display()
        ),
    }
}

fn write_hooks_root(dir: &Path, file_name: &str, root: &Value) -> Result<()> {
    std::fs::create_dir_all(dir)?;
    // Atomic write: tmp + rename.
    let tmp = dir.join(format!(".{file_name}.nebula-tmp"));
    let path = dir.join(file_name);
    std::fs::write(&tmp, serde_json::to_string_pretty(root)?)
        .with_context(|| format!("write {}", tmp.display()))?;
    std::fs::rename(&tmp, &path).with_context(|| format!("rename into {}", path.display()))?;
    Ok(())
}

fn install_managed_hooks(
    cwd: &Path,
    dir_name: &str,
    file_name: &str,
    endpoint: &str,
    events: &[(&str, Option<&str>)],
) -> Result<()> {
    let dir = cwd.join(dir_name);
    let path = dir.join(file_name);
    let mut root = load_hooks_root(&path)?;

    let Some(root_obj) = root.as_object_mut() else {
        bail!(
            "{} is not a JSON object — refusing to modify it",
            path.display()
        );
    };
    let hooks = root_obj.entry("hooks").or_insert_with(|| json!({}));
    let Some(hooks_obj) = hooks.as_object_mut() else {
        bail!(
            "\"hooks\" in {} is not an object — refusing to modify it",
            path.display()
        );
    };

    for (event, matcher) in events {
        let groups = hooks_obj
            .entry(event.to_string())
            .or_insert_with(|| json!([]));
        let Some(groups_arr) = groups.as_array_mut() else {
            bail!(
                "hooks.{event} in {} is not an array — refusing to modify it",
                path.display()
            );
        };
        groups_arr.retain(|g| !is_nebula_group(g));
        groups_arr.push(managed_group(endpoint, event, *matcher));
    }

    write_hooks_root(&dir, file_name, &root)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read_json(dir: &Path, rel: &str) -> Value {
        let text = std::fs::read_to_string(dir.join(rel)).unwrap();
        serde_json::from_str(&text).unwrap()
    }

    fn read_settings(dir: &Path) -> Value {
        read_json(dir, ".claude/settings.local.json")
    }

    #[test]
    fn installs_into_missing_file() {
        let tmp = tempfile::tempdir().unwrap();
        install_claude_hooks(tmp.path()).unwrap();
        let settings = read_settings(tmp.path());
        let stop = &settings["hooks"]["Stop"];
        assert_eq!(stop.as_array().unwrap().len(), 1);
        assert_eq!(stop[0]["_nebulaManaged"], json!(true));
        assert!(stop[0]["hooks"][0]["command"]
            .as_str()
            .unwrap()
            .contains("hookEvent=Stop"));
        let notification = &settings["hooks"]["Notification"];
        assert_eq!(notification[0]["matcher"], json!("permission_prompt"));
        let pre = &settings["hooks"]["PreToolUse"];
        assert_eq!(pre[0]["matcher"], json!("AskUserQuestion"));
    }

    #[test]
    fn preserves_user_hooks_and_settings() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join(".claude");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("settings.local.json"),
            serde_json::to_string(&json!({
                "permissions": { "allow": ["Bash(ls:*)"] },
                "hooks": {
                    "Stop": [
                        { "hooks": [{ "type": "command", "command": "say done" }] }
                    ]
                }
            }))
            .unwrap(),
        )
        .unwrap();

        install_claude_hooks(tmp.path()).unwrap();
        let settings = read_settings(tmp.path());
        assert_eq!(settings["permissions"]["allow"][0], json!("Bash(ls:*)"));
        let stop = settings["hooks"]["Stop"].as_array().unwrap();
        assert_eq!(stop.len(), 2, "user group + nebula group");
        assert_eq!(stop[0]["hooks"][0]["command"], json!("say done"));
        assert_eq!(stop[1]["_nebulaManaged"], json!(true));
    }

    #[test]
    fn reinstall_does_not_accumulate_duplicates() {
        let tmp = tempfile::tempdir().unwrap();
        install_claude_hooks(tmp.path()).unwrap();
        install_claude_hooks(tmp.path()).unwrap();
        install_claude_hooks(tmp.path()).unwrap();
        let settings = read_settings(tmp.path());
        for (event, _) in CLAUDE_EVENTS {
            assert_eq!(
                settings["hooks"][*event].as_array().unwrap().len(),
                1,
                "{event} accumulated duplicates"
            );
        }
    }

    #[test]
    fn strips_legacy_untagged_nebula_groups() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join(".claude");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("settings.local.json"),
            serde_json::to_string(&json!({
                "hooks": {
                    "Stop": [
                        // Old nebula install without the marker.
                        { "hooks": [{ "type": "command",
                            "command": "curl $NEBULA_API_URL/api/hooks/claude?agentId=$NEBULA_AGENT_ID" }] }
                    ]
                }
            }))
            .unwrap(),
        )
        .unwrap();
        install_claude_hooks(tmp.path()).unwrap();
        let settings = read_settings(tmp.path());
        assert_eq!(settings["hooks"]["Stop"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn corrupt_file_aborts_without_clobbering() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join(".claude");
        std::fs::create_dir_all(&dir).unwrap();
        let original = "{ this is not json";
        std::fs::write(dir.join("settings.local.json"), original).unwrap();
        assert!(install_claude_hooks(tmp.path()).is_err());
        let after = std::fs::read_to_string(dir.join("settings.local.json")).unwrap();
        assert_eq!(after, original, "corrupt file must be left untouched");
    }

    #[test]
    fn codex_installs_into_missing_file() {
        let tmp = tempfile::tempdir().unwrap();
        install_codex_hooks(tmp.path()).unwrap();
        let hooks = read_json(tmp.path(), ".codex/hooks.json");
        let stop = &hooks["hooks"]["Stop"];
        assert_eq!(stop.as_array().unwrap().len(), 1);
        assert_eq!(stop[0]["_nebulaManaged"], json!(true));
        let cmd = stop[0]["hooks"][0]["command"].as_str().unwrap();
        assert!(cmd.contains("/api/hooks/codex?"), "codex endpoint: {cmd}");
        assert!(cmd.contains("hookEvent=Stop"));
        // Claude-only events must not leak into the codex file.
        assert!(hooks["hooks"].get("Notification").is_none());
        assert!(hooks["hooks"].get("PreToolUse").is_none());
        assert!(hooks["hooks"].get("PostToolUse").is_none());
        assert_eq!(
            hooks["hooks"]["PermissionRequest"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    fn codex_preserves_foreign_managed_groups() {
        // Mission Control also writes _mcManaged groups into the same file.
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join(".codex");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("hooks.json"),
            serde_json::to_string(&json!({
                "hooks": {
                    "Stop": [
                        { "hooks": [{ "type": "command", "command": "curl $MC_API_URL/api/hooks/codex" }],
                          "_mcManaged": true }
                    ]
                }
            }))
            .unwrap(),
        )
        .unwrap();

        install_codex_hooks(tmp.path()).unwrap();
        install_codex_hooks(tmp.path()).unwrap();
        let hooks = read_json(tmp.path(), ".codex/hooks.json");
        let stop = hooks["hooks"]["Stop"].as_array().unwrap();
        assert_eq!(stop.len(), 2, "foreign managed group + nebula group");
        assert_eq!(stop[0]["_mcManaged"], json!(true));
        assert_eq!(stop[1]["_nebulaManaged"], json!(true));
    }

    #[test]
    fn codex_corrupt_file_aborts_without_clobbering() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join(".codex");
        std::fs::create_dir_all(&dir).unwrap();
        let original = "not json at all";
        std::fs::write(dir.join("hooks.json"), original).unwrap();
        assert!(install_codex_hooks(tmp.path()).is_err());
        let after = std::fs::read_to_string(dir.join("hooks.json")).unwrap();
        assert_eq!(after, original, "corrupt file must be left untouched");
    }

    #[test]
    fn cursor_installs_native_dialect_into_missing_file() {
        let tmp = tempfile::tempdir().unwrap();
        install_cursor_hooks(tmp.path()).unwrap();
        let hooks = read_json(tmp.path(), ".cursor/hooks.json");
        // Cursor requires the top-level version marker.
        assert_eq!(hooks["version"], json!(1));
        // camelCase cursor events, flat command entries.
        let stop = &hooks["hooks"]["stop"];
        assert_eq!(stop.as_array().unwrap().len(), 1);
        assert_eq!(stop[0]["_nebulaManaged"], json!(true));
        let cmd = stop[0]["command"].as_str().unwrap();
        assert!(cmd.contains("/api/hooks/cursor?"), "cursor endpoint: {cmd}");
        assert!(cmd.contains("hookEvent=Stop"));
        // Gating hooks must answer cursor with a JSON response.
        assert!(cmd.contains("{\"continue\": true}"), "stdout response: {cmd}");
        let submit = &hooks["hooks"]["beforeSubmitPrompt"][0];
        assert!(submit["command"]
            .as_str()
            .unwrap()
            .contains("hookEvent=UserPromptSubmit"));
        assert!(hooks["hooks"]["sessionStart"][0]["command"]
            .as_str()
            .unwrap()
            .contains("hookEvent=SessionStart"));
        assert!(hooks["hooks"]["subagentStop"][0]["command"]
            .as_str()
            .unwrap()
            .contains("hookEvent=SubagentStop"));
        // Claude-dialect events must not appear — cursor never fires them.
        for key in ["Stop", "UserPromptSubmit", "SessionStart", "PermissionRequest"] {
            assert!(hooks["hooks"].get(key).is_none(), "{key} leaked");
        }
    }

    #[test]
    fn cursor_reinstall_does_not_accumulate_duplicates() {
        let tmp = tempfile::tempdir().unwrap();
        install_cursor_hooks(tmp.path()).unwrap();
        install_cursor_hooks(tmp.path()).unwrap();
        let hooks = read_json(tmp.path(), ".cursor/hooks.json");
        for (event, _) in CURSOR_EVENTS {
            assert_eq!(
                hooks["hooks"][*event].as_array().unwrap().len(),
                1,
                "{event} accumulated duplicates"
            );
        }
    }

    #[test]
    fn cursor_migrates_legacy_claude_shaped_groups_and_keeps_foreign() {
        // An older nebula wrote Claude-dialect groups (PascalCase events,
        // nested hooks arrays) that cursor never fires; mission-control
        // writes flat _mcManaged groups into the same file. Migration must
        // remove the former and preserve the latter.
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join(".cursor");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("hooks.json"),
            serde_json::to_string(&json!({
                "version": 1,
                "hooks": {
                    "Stop": [
                        { "_nebulaManaged": true,
                          "hooks": [{ "type": "command",
                            "command": "curl $NEBULA_API_URL/api/hooks/cursor?agentId=$NEBULA_AGENT_ID&hookEvent=Stop" }] }
                    ],
                    "UserPromptSubmit": [
                        { "_nebulaManaged": true,
                          "hooks": [{ "type": "command",
                            "command": "curl $NEBULA_API_URL/api/hooks/cursor?agentId=$NEBULA_AGENT_ID&hookEvent=UserPromptSubmit" }] }
                    ],
                    "stop": [
                        { "command": "curl $MC_API_URL/api/hooks/cursor", "_mcManaged": true }
                    ]
                }
            }))
            .unwrap(),
        )
        .unwrap();

        install_cursor_hooks(tmp.path()).unwrap();
        let hooks = read_json(tmp.path(), ".cursor/hooks.json");
        // Stale Claude-dialect keys are gone entirely (empty arrays pruned).
        assert!(hooks["hooks"].get("Stop").is_none());
        assert!(hooks["hooks"].get("UserPromptSubmit").is_none());
        // Foreign managed group survives ahead of ours.
        let stop = hooks["hooks"]["stop"].as_array().unwrap();
        assert_eq!(stop.len(), 2, "mc group + nebula group");
        assert_eq!(stop[0]["_mcManaged"], json!(true));
        assert_eq!(stop[1]["_nebulaManaged"], json!(true));
    }

    #[test]
    fn cursor_corrupt_file_aborts_without_clobbering() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join(".cursor");
        std::fs::create_dir_all(&dir).unwrap();
        let original = "{ not json";
        std::fs::write(dir.join("hooks.json"), original).unwrap();
        assert!(install_cursor_hooks(tmp.path()).is_err());
        let after = std::fs::read_to_string(dir.join("hooks.json")).unwrap();
        assert_eq!(after, original, "corrupt file must be left untouched");
    }

    #[test]
    fn per_kind_installs_do_not_interfere() {
        let tmp = tempfile::tempdir().unwrap();
        install_claude_hooks(tmp.path()).unwrap();
        install_codex_hooks(tmp.path()).unwrap();
        install_cursor_hooks(tmp.path()).unwrap();
        let claude = read_settings(tmp.path());
        let codex = read_json(tmp.path(), ".codex/hooks.json");
        let cursor = read_json(tmp.path(), ".cursor/hooks.json");
        assert!(claude["hooks"]["Stop"][0]["hooks"][0]["command"]
            .as_str()
            .unwrap()
            .contains("/api/hooks/claude?"));
        assert!(codex["hooks"]["Stop"][0]["hooks"][0]["command"]
            .as_str()
            .unwrap()
            .contains("/api/hooks/codex?"));
        assert!(cursor["hooks"]["stop"][0]["command"]
            .as_str()
            .unwrap()
            .contains("/api/hooks/cursor?"));
    }
}
