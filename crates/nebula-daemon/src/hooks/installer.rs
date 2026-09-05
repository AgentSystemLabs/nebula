//! Managed-hook installation into the agent CLI's config:
//! `<worktree>/.claude/settings.local.json` (Claude Code),
//! `~/.codex/hooks.json` (Codex CLI), or `<worktree>/.cursor/hooks.json`
//! (Cursor CLI). Pi has no hook file — its managed TypeScript extension is
//! `pi_extension.rs`, which shares this module's atomic writer.
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
//! Codex caveat, and the reason its hooks are the one set that does NOT go
//! in the worktree: codex gates every hook behind a startup "Hooks need
//! review" prompt and records the approval in `~/.codex/config.toml
//! [hooks.state]` under the hook FILE'S PATH. Project-local
//! `.codex/hooks.json` therefore re-prompts in every fresh worktree — and
//! an unanswered prompt means no hooks at all, so status stayed grey and no
//! title was ever injected. Installing into `$CODEX_HOME/hooks.json`
//! (default `~/.codex`) makes that path stable: the user trusts nebula's
//! hooks once and every later worktree is silent. The commands stay
//! env-guarded, so they remain inert in codex sessions outside nebula.

use anyhow::{bail, Context, Result};
use nebula_core::env::{AGENT_ID, API_TOKEN, API_URL};
use serde_json::{json, Map, Value};
use std::path::{Path, PathBuf};

/// The hooks file's name under codex's home and cursor's per-worktree dir.
/// Claude keeps its hooks in its settings file instead.
const HOOKS_FILE: &str = "hooks.json";
/// Claude Code's per-checkout settings: hooks and permission rules both.
const CLAUDE_SETTINGS_FILE: &str = "settings.local.json";
/// The CLIs' config directories: `.claude` and `.cursor` are per-worktree;
/// `.codex` is both the worktree-local one an older nebula wrote (pruned
/// now) and the name under `$HOME` when `$CODEX_HOME` is unset.
const CLAUDE_DIR: &str = ".claude";
const CURSOR_DIR: &str = ".cursor";
const CODEX_DIR: &str = ".codex";
/// How long a hook waits on the daemon. Hooks run inline in the agent's
/// turn, so a hung daemon has to cost seconds, not a stuck session.
const HOOK_CURL_TIMEOUT_SECS: u32 = 3;

/// (hook event, optional matcher)
const CLAUDE_EVENTS: &[(&str, Option<&str>)] = &[
    ("UserPromptSubmit", None),
    ("Stop", None),
    ("SessionStart", None),
    ("PermissionRequest", None),
    // No matcher: Claude filters Notification hooks by notification_type
    // before dispatch, and we need `idle_prompt` ("Claude is waiting for
    // your input") as well as `permission_prompt` — it is the only signal
    // that arrives when a turn ends without a Stop. See status.rs.
    ("Notification", None),
    ("PreToolUse", Some("AskUserQuestion")),
    ("PostToolUse", Some("AskUserQuestion")),
    // Not a status signal — a position one. Claude reports the session's
    // working directory on every hook payload, and these are the tools that
    // change it: EnterWorktree/ExitWorktree relocate the whole session (what
    // "do this in a worktree" actually runs), and a Bash `cd` moves it too.
    // Hooking them re-homes the row seconds after the session moves instead
    // of at the turn's Stop, which can be many minutes later. Matchers are
    // regexes, so one group covers all three.
    // See registry::reparent_agent_by_cwd.
    ("PostToolUse", Some("Bash|EnterWorktree|ExitWorktree")),
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

/// The shell test every hook opens with. Outside nebula (a bare `claude`
/// in the same checkout) the session env is absent and the hook must be
/// inert — exit before curl ever runs.
fn env_guard() -> String {
    format!("[ -z \"${AGENT_ID}\" ] || [ -z \"${API_URL}\" ]")
}

/// The POST itself, identical for every dialect: bearer auth, the hook's
/// stdin payload passed straight through, and the agent id plus event
/// name on the query string so the daemon needs nothing else to route it.
fn hook_curl(endpoint: &str, event: &str) -> String {
    format!(
        "curl -sS -m {HOOK_CURL_TIMEOUT_SECS} -X POST -H \"Authorization: Bearer ${API_TOKEN}\" \
         -H \"Content-Type: application/json\" --data-binary @- \
         \"${API_URL}/api/hooks/{endpoint}?agentId=${AGENT_ID}&hookEvent={event}\""
    )
}

fn hook_command(endpoint: &str, event: &str) -> String {
    // UserPromptSubmit passes the daemon's response body through to stdout:
    // Claude Code (and Codex, same dialect) add a hook's stdout to the
    // model's context, which is how the session auto-title instruction
    // reaches the agent. The daemon keeps that body empty except when an
    // instruction is due. Every other event stays fully silent.
    let silence = if event == "UserPromptSubmit" {
        "2>/dev/null"
    } else {
        ">/dev/null 2>&1"
    };
    format!(
        "if {}; then exit 0; fi; {} {silence} || true",
        env_guard(),
        hook_curl(endpoint, event)
    )
}

/// Permission rules letting Claude Code run nebula's own commands without a
/// permission prompt: the auto-title `nebula rename`, and the `nebula
/// worktree` relocation, `nebula spawn` sibling and `nebula open` file tabs
/// its appended system prompt tells it to use (codex/cursor run with their
/// skip-permissions flags).
const CLAUDE_ALLOW_RULES: &[&str] = &[
    "Bash(nebula rename:*)",
    "Bash(nebula worktree:*)",
    "Bash(nebula spawn:*)",
    "Bash(nebula open:*)",
];

/// Cursor variant: the payload arrives on stdin like Claude's, but cursor
/// expects a JSON response on stdout — `{"continue": true}` keeps gating
/// events (beforeSubmitPrompt) flowing and is ignored by the rest.
fn cursor_hook_command(event: &str) -> String {
    format!(
        "if {}; then printf '{{\"continue\": true}}\\n'; exit 0; fi; \
         {} >/dev/null 2>&1 || true; printf '{{\"continue\": true}}\\n'",
        env_guard(),
        hook_curl("cursor", event)
    )
}

fn is_nebula_command(cmd: Option<&Value>) -> bool {
    cmd.and_then(Value::as_str)
        .map(|c| c.contains("/api/hooks/") && c.contains(AGENT_ID))
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
/// `<cwd>/.claude/settings.local.json`, plus the permission rule that lets
/// the auto-title `nebula rename` run unprompted.
pub fn install_claude_hooks(cwd: &Path) -> Result<()> {
    install_managed_hooks(
        &cwd.join(CLAUDE_DIR),
        CLAUDE_SETTINGS_FILE,
        "claude",
        CLAUDE_EVENTS,
        CLAUDE_ALLOW_RULES,
    )
}

/// Idempotently add one entry to `permissions.allow`, preserving everything
/// the user put there. Same abort-don't-clobber policy as the hook merge.
fn ensure_permission_allow(
    root_obj: &mut Map<String, Value>,
    entry: &str,
    path: &Path,
) -> Result<()> {
    let perms = root_obj.entry("permissions").or_insert_with(|| json!({}));
    let perms_obj = object_mut(perms, "\"permissions\"", path)?;
    let allow = perms_obj.entry("allow").or_insert_with(|| json!([]));
    let allow_arr = array_mut(allow, "permissions.allow", path)?;
    if !allow_arr.iter().any(|v| v.as_str() == Some(entry)) {
        allow_arr.push(json!(entry));
    }
    Ok(())
}

/// The abort-don't-clobber gate on a config file's root: anything but an
/// object is not a config nebula understands, so it is left exactly as
/// found and the install fails instead.
fn root_object_mut<'a>(root: &'a mut Value, path: &Path) -> Result<&'a mut Map<String, Value>> {
    match root.as_object_mut() {
        Some(obj) => Ok(obj),
        None => bail!(
            "{} is not a JSON object — refusing to modify it",
            path.display()
        ),
    }
}

/// `v` as a mutable object, or the same refusal naming the field (`what`,
/// spelled as the message should show it) inside `path`.
fn object_mut<'a>(v: &'a mut Value, what: &str, path: &Path) -> Result<&'a mut Map<String, Value>> {
    match v.as_object_mut() {
        Some(obj) => Ok(obj),
        None => bail!(
            "{what} in {} is not an object — refusing to modify it",
            path.display()
        ),
    }
}

/// Array twin of [`object_mut`].
fn array_mut<'a>(v: &'a mut Value, what: &str, path: &Path) -> Result<&'a mut Vec<Value>> {
    match v.as_array_mut() {
        Some(arr) => Ok(arr),
        None => bail!(
            "{what} in {} is not an array — refusing to modify it",
            path.display()
        ),
    }
}

/// Codex's home (`$CODEX_HOME`, else `~/.codex`) — where its hooks live so
/// one trust approval covers every worktree. See the module header.
pub fn codex_home() -> PathBuf {
    nebula_core::env::non_empty("CODEX_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            nebula_core::env::home_dir()
                .unwrap_or_default()
                .join(CODEX_DIR)
        })
}

/// Merge nebula's managed hooks for Codex into `<codex_home>/hooks.json`.
pub fn install_codex_hooks(codex_home: &Path) -> Result<()> {
    install_managed_hooks(codex_home, HOOKS_FILE, "codex", CODEX_EVENTS, &[])
}

/// Drop the per-worktree `.codex/hooks.json` groups an older nebula wrote:
/// left in place they are a second, never-trusted copy of the same hooks
/// that codex would re-prompt for. Foreign groups (mission-control's) stay;
/// a file left holding nothing at all is removed, and so is a `.codex`
/// directory that existed only for it.
pub fn prune_codex_worktree_hooks(cwd: &Path) -> Result<()> {
    let dir = cwd.join(CODEX_DIR);
    let path = dir.join(HOOKS_FILE);
    if !path.exists() {
        return Ok(());
    }
    let mut root = load_hooks_root(&path)?;
    let Some(hooks_obj) = root
        .as_object_mut()
        .and_then(|o| o.get_mut("hooks"))
        .and_then(Value::as_object_mut)
    else {
        return Ok(());
    };
    let before = hooks_obj.len();
    purge_nebula_groups(hooks_obj);
    let emptied = hooks_obj.is_empty();
    if emptied && before > 0 && root.as_object().map(|o| o.len()) == Some(1) {
        std::fs::remove_file(&path).with_context(|| format!("remove {}", path.display()))?;
        // Only ours to remove, and only while it holds nothing else.
        let _ = std::fs::remove_dir(&dir);
        return Ok(());
    }
    write_hooks_root(&dir, HOOKS_FILE, &root)
}

/// Strip nebula's groups from under EVERY event key of a loaded hooks
/// object (not just the ones about to be reinstalled: stale keys from an
/// older install shape must go, and events nebula may drop in the future
/// must not linger), then drop the keys that emptied. User groups are left
/// exactly as found.
fn purge_nebula_groups(hooks_obj: &mut Map<String, Value>) {
    for (_, groups) in hooks_obj.iter_mut() {
        if let Some(arr) = groups.as_array_mut() {
            arr.retain(|g| !is_nebula_group(g));
        }
    }
    hooks_obj.retain(|_, groups| groups.as_array().map(|a| !a.is_empty()).unwrap_or(true));
}

/// Merge nebula's managed hooks for Cursor into `<cwd>/.cursor/hooks.json`,
/// in Cursor's own dialect. Also migrates away the Claude-shaped groups an
/// older nebula wrote there (events Cursor never fires — the original
/// "cursor status never updates" bug).
pub fn install_cursor_hooks(cwd: &Path) -> Result<()> {
    let dir = cwd.join(CURSOR_DIR);
    let path = dir.join(HOOKS_FILE);
    let mut root = load_hooks_root(&path)?;

    let root_obj = root_object_mut(&mut root, &path)?;
    // Cursor requires a top-level version; never overwrite an existing one.
    root_obj.entry("version").or_insert(json!(1));
    let hooks = root_obj.entry("hooks").or_insert_with(|| json!({}));
    let hooks_obj = object_mut(hooks, "\"hooks\"", &path)?;

    // The stale PascalCase keys from the old Claude-shaped install go here.
    purge_nebula_groups(hooks_obj);

    for (cursor_event, nebula_event) in CURSOR_EVENTS {
        let groups = hooks_obj
            .entry(cursor_event.to_string())
            .or_insert_with(|| json!([]));
        let groups_arr = array_mut(groups, &format!("hooks.{cursor_event}"), &path)?;
        groups_arr.push(json!({
            "command": cursor_hook_command(nebula_event),
            "_nebulaManaged": true,
        }));
    }

    write_hooks_root(&dir, HOOKS_FILE, &root)
}

fn load_hooks_root(path: &Path) -> Result<Value> {
    if !path.exists() {
        return Ok(json!({}));
    }
    let text = std::fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
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
    write_text_atomic(dir, file_name, &serde_json::to_string_pretty(root)?)
}

pub(super) fn write_text_atomic(dir: &Path, file_name: &str, text: &str) -> Result<()> {
    std::fs::create_dir_all(dir)?;
    // Atomic write: tmp + rename.
    let tmp = dir.join(format!(".{file_name}.nebula-tmp"));
    let path = dir.join(file_name);
    std::fs::write(&tmp, text).with_context(|| format!("write {}", tmp.display()))?;
    std::fs::rename(&tmp, &path).with_context(|| format!("rename into {}", path.display()))?;
    Ok(())
}

/// Load `<dir>/<file_name>`, strip-and-rebuild nebula's groups for `events`
/// in the Claude/Codex dialect, make sure each of `allow_rules` is in
/// `permissions.allow`, and write it back. Claude's settings file carries
/// hooks and permissions both, so the one pass serves it and codex's
/// hooks-only file alike (codex passes no rules).
fn install_managed_hooks(
    dir: &Path,
    file_name: &str,
    endpoint: &str,
    events: &[(&str, Option<&str>)],
    allow_rules: &[&str],
) -> Result<()> {
    let path = dir.join(file_name);
    let mut root = load_hooks_root(&path)?;

    let root_obj = root_object_mut(&mut root, &path)?;
    merge_managed_hooks(root_obj, endpoint, events, &path)?;
    for rule in allow_rules {
        ensure_permission_allow(root_obj, rule, &path)?;
    }
    write_hooks_root(dir, file_name, &root)
}

/// Strip-and-rebuild nebula's groups under each event key of a loaded
/// Claude/Codex-dialect config, leaving user groups untouched.
fn merge_managed_hooks(
    root_obj: &mut Map<String, Value>,
    endpoint: &str,
    events: &[(&str, Option<&str>)],
    path: &Path,
) -> Result<()> {
    let hooks = root_obj.entry("hooks").or_insert_with(|| json!({}));
    let hooks_obj = object_mut(hooks, "\"hooks\"", path)?;

    // One event can carry several managed groups (PostToolUse has one per
    // matcher), so the strip-and-rebuild happens once per event name —
    // stripping again for the second matcher would delete the group the
    // first one just added.
    let mut stripped: std::collections::HashSet<&str> = std::collections::HashSet::new();
    for (event, matcher) in events {
        let groups = hooks_obj
            .entry(event.to_string())
            .or_insert_with(|| json!([]));
        let groups_arr = array_mut(groups, &format!("hooks.{event}"), path)?;
        if stripped.insert(event) {
            groups_arr.retain(|g| !is_nebula_group(g));
        }
        groups_arr.push(managed_group(endpoint, event, *matcher));
    }
    Ok(())
}

/// Cursor can't receive daemon-injected context (its hooks are gating-only,
/// answering with their own JSON), so the auto-title instruction ships as a
/// managed always-on project rule instead. The file is wholly nebula-owned
/// (namespaced filename) and rewritten on every spawn. Firing outside
/// nebula or on later prompts is harmless: the rule env-guards itself, and
/// the daemon accepts at most one auto-title per session.
pub fn install_cursor_title_rule(cwd: &Path) -> Result<()> {
    let dir = cwd.join(CURSOR_DIR).join("rules");
    write_text_atomic(&dir, "nebula-title.mdc", &cursor_title_rule())
}

/// Same instruction the injectable CLIs get, wrapped in cursor's rule
/// frontmatter and an env guard (a project rule can't be switched off from
/// the daemon's side the way an injection can).
fn cursor_title_rule() -> String {
    format!(
        "---
description: Nebula session auto-title (managed by nebula — edits are overwritten)
alwaysApply: true
---

This rule applies only when the environment variable NEBULA_AGENT_ID is set
(the session runs inside nebula). If it is unset, ignore this rule entirely.

On the first user message of a new conversation:

{}
",
        super::AUTO_TITLE_INSTRUCTION
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The exact one-liners that land in users' config files. Pinned
    /// verbatim so the shared curl prelude can be factored without a byte
    /// of the installed command drifting.
    #[test]
    fn hook_commands_are_spelled_exactly() {
        assert_eq!(
            hook_command("claude", "UserPromptSubmit"),
            "if [ -z \"$NEBULA_AGENT_ID\" ] || [ -z \"$NEBULA_API_URL\" ]; then exit 0; fi; \
             curl -sS -m 3 -X POST -H \"Authorization: Bearer $NEBULA_API_TOKEN\" \
             -H \"Content-Type: application/json\" --data-binary @- \
             \"$NEBULA_API_URL/api/hooks/claude?agentId=$NEBULA_AGENT_ID&hookEvent=UserPromptSubmit\" \
             2>/dev/null || true"
        );
        assert_eq!(
            hook_command("codex", "Stop"),
            "if [ -z \"$NEBULA_AGENT_ID\" ] || [ -z \"$NEBULA_API_URL\" ]; then exit 0; fi; \
             curl -sS -m 3 -X POST -H \"Authorization: Bearer $NEBULA_API_TOKEN\" \
             -H \"Content-Type: application/json\" --data-binary @- \
             \"$NEBULA_API_URL/api/hooks/codex?agentId=$NEBULA_AGENT_ID&hookEvent=Stop\" \
             >/dev/null 2>&1 || true"
        );
        assert_eq!(
            cursor_hook_command("Stop"),
            "if [ -z \"$NEBULA_AGENT_ID\" ] || [ -z \"$NEBULA_API_URL\" ]; then \
             printf '{\"continue\": true}\\n'; exit 0; fi; \
             curl -sS -m 3 -X POST -H \"Authorization: Bearer $NEBULA_API_TOKEN\" \
             -H \"Content-Type: application/json\" --data-binary @- \
             \"$NEBULA_API_URL/api/hooks/cursor?agentId=$NEBULA_AGENT_ID&hookEvent=Stop\" \
             >/dev/null 2>&1 || true; printf '{\"continue\": true}\\n'"
        );
    }

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
        // Notification carries no matcher: idle_prompt has to reach us too.
        let notification = &settings["hooks"]["Notification"];
        assert!(notification[0].get("matcher").is_none());
        let pre = &settings["hooks"]["PreToolUse"];
        assert_eq!(pre[0]["matcher"], json!("AskUserQuestion"));
    }

    #[test]
    fn post_tool_use_keeps_both_matchers_across_reinstalls() {
        // Two managed groups share the PostToolUse event: AskUserQuestion is
        // the waiting-on-user signal, the other is the cwd probe that
        // re-homes a session into a worktree it just entered. A reinstall
        // (every spawn) must leave exactly one of each.
        let tmp = tempfile::tempdir().unwrap();
        install_claude_hooks(tmp.path()).unwrap();
        install_claude_hooks(tmp.path()).unwrap();
        let settings = read_settings(tmp.path());
        let groups = settings["hooks"]["PostToolUse"].as_array().unwrap();
        let matchers: Vec<&str> = groups
            .iter()
            .map(|g| g["matcher"].as_str().unwrap())
            .collect();
        assert_eq!(
            matchers,
            vec!["AskUserQuestion", "Bash|EnterWorktree|ExitWorktree"]
        );
        assert!(groups.iter().all(|g| g["_nebulaManaged"] == json!(true)));
    }

    #[test]
    fn user_prompt_submit_command_pipes_response_to_stdout() {
        // The daemon's UserPromptSubmit response body is the auto-title
        // context injection — that one command must let stdout through
        // (stderr still silenced); every other event stays fully silent.
        let tmp = tempfile::tempdir().unwrap();
        install_claude_hooks(tmp.path()).unwrap();
        let settings = read_settings(tmp.path());
        let submit = settings["hooks"]["UserPromptSubmit"][0]["hooks"][0]["command"]
            .as_str()
            .unwrap();
        assert!(submit.contains("2>/dev/null"), "stderr silenced: {submit}");
        assert!(
            !submit.contains(">/dev/null 2>&1"),
            "stdout must pass through: {submit}"
        );
        let stop = settings["hooks"]["Stop"][0]["hooks"][0]["command"]
            .as_str()
            .unwrap();
        assert!(
            stop.contains(">/dev/null 2>&1"),
            "stop stays silent: {stop}"
        );
    }

    #[test]
    fn claude_install_adds_rename_permission_idempotently() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join(".claude");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("settings.local.json"),
            serde_json::to_string(&json!({
                "permissions": { "allow": ["Bash(ls:*)"], "deny": ["WebFetch"] }
            }))
            .unwrap(),
        )
        .unwrap();

        install_claude_hooks(tmp.path()).unwrap();
        install_claude_hooks(tmp.path()).unwrap();
        let settings = read_settings(tmp.path());
        let allow = settings["permissions"]["allow"].as_array().unwrap();
        assert_eq!(allow[0], json!("Bash(ls:*)"), "user entry preserved first");
        for rule in CLAUDE_ALLOW_RULES {
            assert_eq!(
                allow.iter().filter(|v| v.as_str() == Some(rule)).count(),
                1,
                "exactly one {rule} entry after reinstalls: {allow:?}"
            );
        }
        assert_eq!(settings["permissions"]["deny"][0], json!("WebFetch"));
    }

    /// Codex hooks land in its home dir, not the worktree — that stable
    /// path is what keeps its trust prompt to a single approval.
    #[test]
    fn codex_installs_into_codex_home() {
        let tmp = tempfile::tempdir().unwrap();
        install_codex_hooks(tmp.path()).unwrap();
        let hooks = read_json(tmp.path(), "hooks.json");
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
        // UserPromptSubmit still lets the daemon's response through: that
        // body is the auto-title injection.
        let submit = hooks["hooks"]["UserPromptSubmit"][0]["hooks"][0]["command"]
            .as_str()
            .unwrap();
        assert!(!submit.contains(">/dev/null 2>&1"), "stdout: {submit}");
    }

    #[test]
    fn codex_home_prefers_env_over_home() {
        // Serialised with the other env-reading test by running in one test.
        let tmp = tempfile::tempdir().unwrap();
        let saved = std::env::var("CODEX_HOME").ok();
        std::env::set_var("CODEX_HOME", tmp.path());
        assert_eq!(codex_home(), tmp.path());
        std::env::set_var("CODEX_HOME", "");
        assert_eq!(
            codex_home(),
            nebula_core::env::home_dir()
                .unwrap_or_default()
                .join(".codex")
        );
        match saved {
            Some(v) => std::env::set_var("CODEX_HOME", v),
            None => std::env::remove_var("CODEX_HOME"),
        }
    }

    /// Migration off the old per-worktree install: our groups go, foreign
    /// ones stay, and a file left with nothing in it is removed outright.
    #[test]
    fn codex_worktree_hooks_are_pruned() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join(".codex");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("hooks.json"),
            serde_json::to_string(&json!({
                "hooks": {
                    "Stop": [
                        { "_nebulaManaged": true,
                          "hooks": [{ "type": "command",
                            "command": "curl $NEBULA_API_URL/api/hooks/codex?agentId=$NEBULA_AGENT_ID" }] },
                        { "_mcManaged": true,
                          "hooks": [{ "type": "command", "command": "curl $MC_API_URL/api/hooks/codex" }] }
                    ],
                    "UserPromptSubmit": [
                        { "_nebulaManaged": true,
                          "hooks": [{ "type": "command",
                            "command": "curl $NEBULA_API_URL/api/hooks/codex?agentId=$NEBULA_AGENT_ID" }] }
                    ]
                }
            }))
            .unwrap(),
        )
        .unwrap();

        prune_codex_worktree_hooks(tmp.path()).unwrap();
        let hooks = read_json(tmp.path(), ".codex/hooks.json");
        // Ours gone from both keys; the emptied key pruned; foreign kept.
        assert!(hooks["hooks"].get("UserPromptSubmit").is_none());
        let stop = hooks["hooks"]["Stop"].as_array().unwrap();
        assert_eq!(stop.len(), 1);
        assert_eq!(stop[0]["_mcManaged"], json!(true));

        // Second worktree: the file was nebula's alone, so nothing is left
        // behind — not the file, not the directory it needed.
        let solo = tempfile::tempdir().unwrap();
        let solo_dir = solo.path().join(".codex");
        std::fs::create_dir_all(&solo_dir).unwrap();
        std::fs::write(
            solo_dir.join("hooks.json"),
            serde_json::to_string(&json!({
                "hooks": { "Stop": [
                    { "_nebulaManaged": true,
                      "hooks": [{ "type": "command",
                        "command": "curl $NEBULA_API_URL/api/hooks/codex?agentId=$NEBULA_AGENT_ID" }] }
                ] }
            }))
            .unwrap(),
        )
        .unwrap();
        prune_codex_worktree_hooks(solo.path()).unwrap();
        assert!(!solo_dir.join("hooks.json").exists());
        assert!(!solo_dir.exists());

        // Nothing to prune is not an error.
        let bare = tempfile::tempdir().unwrap();
        prune_codex_worktree_hooks(bare.path()).unwrap();
    }

    #[test]
    fn cursor_title_rule_is_written_and_rewritten() {
        let tmp = tempfile::tempdir().unwrap();
        install_cursor_title_rule(tmp.path()).unwrap();
        let path = tmp.path().join(".cursor/rules/nebula-title.mdc");
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("alwaysApply: true"));
        assert!(text.contains("nebula rename"));
        assert!(text.contains("NEBULA_AGENT_ID"), "must be env-guarded");
        // Wholly nebula-owned: a scribbled-on file is simply replaced.
        std::fs::write(&path, "user scribbles").unwrap();
        install_cursor_title_rule(tmp.path()).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("nebula rename"));
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
            // One group per (event, matcher) pair — PostToolUse carries two.
            let expected = CLAUDE_EVENTS.iter().filter(|(e, _)| e == event).count();
            assert_eq!(
                settings["hooks"][*event].as_array().unwrap().len(),
                expected,
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
    fn codex_preserves_foreign_managed_groups() {
        // Mission Control also writes _mcManaged groups into hook files.
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        std::fs::create_dir_all(dir).unwrap();
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
        let hooks = read_json(tmp.path(), "hooks.json");
        let stop = hooks["hooks"]["Stop"].as_array().unwrap();
        assert_eq!(stop.len(), 2, "foreign managed group + nebula group");
        assert_eq!(stop[0]["_mcManaged"], json!(true));
        assert_eq!(stop[1]["_nebulaManaged"], json!(true));
    }

    #[test]
    fn codex_corrupt_file_aborts_without_clobbering() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        let original = "not json at all";
        std::fs::write(dir.join("hooks.json"), original).unwrap();
        assert!(install_codex_hooks(dir).is_err());
        // A worktree copy in the same shape is left alone too.
        assert!(prune_codex_worktree_hooks(dir).is_ok());
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
        assert!(
            cmd.contains("{\"continue\": true}"),
            "stdout response: {cmd}"
        );
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
        for key in [
            "Stop",
            "UserPromptSubmit",
            "SessionStart",
            "PermissionRequest",
        ] {
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
        let codex_dir = tempfile::tempdir().unwrap();
        install_claude_hooks(tmp.path()).unwrap();
        install_codex_hooks(codex_dir.path()).unwrap();
        install_cursor_hooks(tmp.path()).unwrap();
        let claude = read_settings(tmp.path());
        let codex = read_json(codex_dir.path(), "hooks.json");
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
