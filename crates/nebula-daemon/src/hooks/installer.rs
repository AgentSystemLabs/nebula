//! Managed-hook installation into a worktree's `.claude/settings.local.json`.
//!
//! Rules (learned the hard way in mission-control):
//! - MERGE, never replace: user hooks are preserved untouched.
//! - Our groups carry `_nebulaManaged: true` and are stripped + rebuilt on
//!   every spawn, so upgrades never accumulate duplicates. A legacy-signature
//!   check (command contains our endpoint + env var) catches untagged strays.
//! - A corrupt file ABORTS the install — never clobber user data.
//! - Commands are env-guarded, so the hooks are inert when the user runs
//!   `claude` outside nebula (no NEBULA_* in env → exit 0).

use anyhow::{bail, Context, Result};
use serde_json::{json, Map, Value};
use std::path::Path;

/// (hook event, optional matcher)
const MANAGED_EVENTS: &[(&str, Option<&str>)] = &[
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

fn hook_command(event: &str) -> String {
    format!(
        "if [ -z \"$NEBULA_AGENT_ID\" ] || [ -z \"$NEBULA_API_URL\" ]; then exit 0; fi; \
         curl -sS -m 3 -X POST -H \"Authorization: Bearer $NEBULA_API_TOKEN\" \
         -H \"Content-Type: application/json\" --data-binary @- \
         \"$NEBULA_API_URL/api/hooks/claude?agentId=$NEBULA_AGENT_ID&hookEvent={event}\" \
         >/dev/null 2>&1 || true"
    )
}

fn is_nebula_group(group: &Value) -> bool {
    if group.get("_nebulaManaged").and_then(Value::as_bool) == Some(true) {
        return true;
    }
    // Legacy/untagged detection by command signature.
    group
        .get("hooks")
        .and_then(Value::as_array)
        .map(|hooks| {
            hooks.iter().any(|h| {
                h.get("command")
                    .and_then(Value::as_str)
                    .map(|c| c.contains("/api/hooks/") && c.contains("NEBULA_AGENT_ID"))
                    .unwrap_or(false)
            })
        })
        .unwrap_or(false)
}

fn managed_group(event: &str, matcher: Option<&str>) -> Value {
    let mut group = Map::new();
    if let Some(m) = matcher {
        group.insert("matcher".into(), json!(m));
    }
    group.insert(
        "hooks".into(),
        json!([{ "type": "command", "command": hook_command(event) }]),
    );
    group.insert("_nebulaManaged".into(), json!(true));
    Value::Object(group)
}

/// Merge nebula's managed hooks into `<cwd>/.claude/settings.local.json`.
pub fn install_hooks(cwd: &Path) -> Result<()> {
    let dir = cwd.join(".claude");
    let path = dir.join("settings.local.json");

    let mut root: Value = if path.exists() {
        let text = std::fs::read_to_string(&path)
            .with_context(|| format!("read {}", path.display()))?;
        if text.trim().is_empty() {
            json!({})
        } else {
            match serde_json::from_str(&text) {
                Ok(v) => v,
                Err(e) => bail!(
                    "{} is not valid JSON ({e}) — refusing to modify it; fix or remove the file",
                    path.display()
                ),
            }
        }
    } else {
        json!({})
    };

    let Some(root_obj) = root.as_object_mut() else {
        bail!("{} is not a JSON object — refusing to modify it", path.display());
    };
    let hooks = root_obj
        .entry("hooks")
        .or_insert_with(|| json!({}));
    let Some(hooks_obj) = hooks.as_object_mut() else {
        bail!("\"hooks\" in {} is not an object — refusing to modify it", path.display());
    };

    for (event, matcher) in MANAGED_EVENTS {
        let groups = hooks_obj.entry(event.to_string()).or_insert_with(|| json!([]));
        let Some(groups_arr) = groups.as_array_mut() else {
            bail!("hooks.{event} in {} is not an array — refusing to modify it", path.display());
        };
        groups_arr.retain(|g| !is_nebula_group(g));
        groups_arr.push(managed_group(event, *matcher));
    }

    std::fs::create_dir_all(&dir)?;
    // Atomic write: tmp + rename.
    let tmp = dir.join(".settings.local.json.nebula-tmp");
    std::fs::write(&tmp, serde_json::to_string_pretty(&root)?)
        .with_context(|| format!("write {}", tmp.display()))?;
    std::fs::rename(&tmp, &path).with_context(|| format!("rename into {}", path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read_settings(dir: &Path) -> Value {
        let text =
            std::fs::read_to_string(dir.join(".claude/settings.local.json")).unwrap();
        serde_json::from_str(&text).unwrap()
    }

    #[test]
    fn installs_into_missing_file() {
        let tmp = tempfile::tempdir().unwrap();
        install_hooks(tmp.path()).unwrap();
        let settings = read_settings(tmp.path());
        let stop = &settings["hooks"]["Stop"];
        assert_eq!(stop.as_array().unwrap().len(), 1);
        assert_eq!(stop[0]["_nebulaManaged"], json!(true));
        assert!(stop[0]["hooks"][0]["command"].as_str().unwrap().contains("hookEvent=Stop"));
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

        install_hooks(tmp.path()).unwrap();
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
        install_hooks(tmp.path()).unwrap();
        install_hooks(tmp.path()).unwrap();
        install_hooks(tmp.path()).unwrap();
        let settings = read_settings(tmp.path());
        for (event, _) in MANAGED_EVENTS {
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
        install_hooks(tmp.path()).unwrap();
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
        assert!(install_hooks(tmp.path()).is_err());
        let after = std::fs::read_to_string(dir.join("settings.local.json")).unwrap();
        assert_eq!(after, original, "corrupt file must be left untouched");
    }
}
