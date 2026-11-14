//! Managed-hook installation into the agent CLI's config:
//! `<worktree>/.claude/settings.local.json` (Claude Code),
//! `~/.codex/hooks.json` (Codex CLI), or `<worktree>/.cursor/hooks.json`
//! (Cursor CLI). Pi and OpenCode have no hook file — their managed
//! TypeScript extension and plugin are `pi_extension.rs` and
//! `opencode_plugin.rs`, which share this module's atomic writer.
//!
//! Claude and Codex share one hooks dialect (PascalCase event names, groups
//! of `{"hooks": [{"type": "command", ...}]}`). Cursor speaks its own
//! (verified against cursor-agent 2026.08 + cursor.com/docs/agent/hooks):
//! camelCase event names (`beforeSubmitPrompt`, `stop`, ...), flat
//! `{"command": ...}` entries, a required top-level `"version": 1`, and each
//! hook must print a JSON response (`{"continue": true}`) to stdout or
//! gating events fall back to fail-open error handling.
//!
//! Claude's settings file also gets a managed `statusLine` — not a hook,
//! but the command Claude Code runs after every assistant message with the
//! session's context window and plan limits as JSON on stdin (USAGE AND
//! LIMITS, `crate::usage`). It POSTs that JSON to the daemon and prints the
//! daemon's one-line answer; a status line the user already had (in that
//! file, the checkout's `.claude/settings.json` or their user settings) is
//! CHAINED: the POST goes in the background and the user's own command gets
//! the same stdin and draws the bar, as it always did.
//!
//! Rules (learned the hard way in mission-control):
//! - MERGE, never replace: user hooks are preserved untouched.
//! - Our groups carry `_orbitManaged: true` and are stripped + rebuilt on
//!   every spawn, so upgrades never accumulate duplicates. A legacy-signature
//!   check (command contains our endpoint + env var) catches untagged strays.
//! - A corrupt file ABORTS the install — never clobber user data.
//! - Commands are env-guarded, so the hooks are inert when the user runs
//!   `claude`/`codex`/`cursor-agent` outside orbit (no ORBIT_* in env →
//!   exit 0).
//!
//! Codex caveat, and the reason its hooks are the one set that does NOT go
//! in the worktree: codex gates every hook behind a startup "Hooks need
//! review" prompt and records the approval in `~/.codex/config.toml
//! [hooks.state]` under the hook FILE'S PATH. Project-local
//! `.codex/hooks.json` therefore re-prompts in every fresh worktree — and
//! an unanswered prompt means no hooks at all, so status stayed grey and no
//! title was ever injected. Installing into `$CODEX_HOME/hooks.json`
//! (default `~/.codex`) makes that path stable: the user trusts orbit's
//! hooks once and every later worktree is silent. The commands stay
//! env-guarded, so they remain inert in codex sessions outside orbit.

use anyhow::{bail, Context, Result};
use orbit_core::env::{AGENT_ID, API_TOKEN, API_URL};
use serde_json::{json, Map, Value};
use std::path::{Path, PathBuf};

/// The hooks file's name under codex's home and cursor's per-worktree dir.
/// Claude keeps its hooks in its settings file instead.
const HOOKS_FILE: &str = "hooks.json";
/// Claude Code's per-checkout settings: hooks and permission rules both.
const CLAUDE_SETTINGS_FILE: &str = "settings.local.json";
/// The CLIs' config directories: `.claude` and `.cursor` are per-worktree;
/// `.codex` is both the worktree-local one an older orbit wrote (pruned
/// now) and the name under `$HOME` when `$CODEX_HOME` is unset.
const CLAUDE_DIR: &str = ".claude";
const CURSOR_DIR: &str = ".cursor";
const CODEX_DIR: &str = ".codex";
/// How long a hook waits on the daemon. Hooks run inline in the agent's
/// turn, so a hung daemon has to cost seconds, not a stuck session.
const HOOK_CURL_TIMEOUT_SECS: u32 = 3;
/// The status line's POST runs inline with drawing Claude's status bar: a
/// hung daemon may blank it for a second, never longer.
const STATUS_LINE_CURL_TIMEOUT_SECS: u32 = 1;
/// Claude Code's checked-in project settings, beside the local file, and
/// the name of the user-level one in its config dir.
const CLAUDE_SHARED_SETTINGS_FILE: &str = "settings.json";
const STATUS_LINE_KEY: &str = "statusLine";
/// Inside orbit's managed status line: the user's own status line object
/// it displaced from the same file, kept so every reinstall chains it
/// again (and nothing of it is lost).
const CHAINED_KEY: &str = "_orbitChained";

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
    // Every tool's end, unmatched. Three signals ride it: the answer to an
    // `AskUserQuestion` (the row leaves red); the approval of a permission
    // prompt, which fires no hook of its own — the gated tool simply runs,
    // and its PostToolUse is the first word that the user said yes, for
    // Edit and WebFetch as much as for Bash (see status.rs); and the
    // session's position — Claude reports its working directory on every
    // payload, and EnterWorktree/ExitWorktree (what "do this in a
    // worktree" actually runs) or a Bash `cd` move it, so the row re-homes
    // seconds after the move instead of at the turn's Stop, which can be
    // many minutes later (see registry::reparent_agent_by_cwd).
    ("PostToolUse", None),
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

/// (cursor hook event, orbit hookEvent query value). Cursor has no
/// PermissionRequest hook and orbit always runs cursor-agent with
/// `--force`; its `AskQuestion` dialog fires no tool hook at all, so that
/// wait is read off the screen instead (`pty::question`).
/// `sessionEnd` is skipped: PTY-exit synthetics already cover agent death.
const CURSOR_EVENTS: &[(&str, &str)] = &[
    ("sessionStart", "SessionStart"),
    ("beforeSubmitPrompt", "UserPromptSubmit"),
    ("stop", "Stop"),
    ("subagentStart", "SubagentStart"),
    ("subagentStop", "SubagentStop"),
];

/// The shell test every hook opens with. Outside orbit (a bare `claude`
/// in the same checkout) the session env is absent and the hook must be
/// inert — exit before curl ever runs.
fn env_guard() -> String {
    format!("[ -z \"${AGENT_ID}\" ] || [ -z \"${API_URL}\" ]")
}

/// The POST itself, identical for every dialect: bearer auth, the hook's
/// stdin payload passed straight through, and the agent id plus event
/// name on the query string so the daemon needs nothing else to route it.
fn hook_curl(endpoint: &str, event: &str) -> String {
    daemon_post(
        HOOK_CURL_TIMEOUT_SECS,
        &format!("/api/hooks/{endpoint}?agentId=${AGENT_ID}&hookEvent={event}"),
    )
}

/// curl POSTing stdin to the daemon's `route` (path and query), bearer
/// authed, giving up after `timeout` seconds.
fn daemon_post(timeout: u32, route: &str) -> String {
    format!(
        "curl -sS -m {timeout} -X POST -H \"Authorization: Bearer ${API_TOKEN}\" \
         -H \"Content-Type: application/json\" --data-binary @- \"${API_URL}{route}\""
    )
}

/// The managed `statusLine` command. On its own it POSTs Claude's status
/// JSON and prints the daemon's answer — the status bar text. Chained to
/// the user's own `user` command, it reads stdin once, POSTs it in the
/// background (output discarded, so it never holds the bar), and hands the
/// same bytes to `user` through `eval`, in whatever shell Claude runs
/// status lines with, so that command behaves exactly as it did. Outside
/// orbit (no ORBIT_* env) only the user's command runs, or nothing.
pub(crate) fn status_line_command(user: Option<&str>) -> String {
    let post = daemon_post(
        STATUS_LINE_CURL_TIMEOUT_SECS,
        &format!("/api/statusline?agentId=${AGENT_ID}"),
    );
    match user {
        None => format!(
            "if {}; then exit 0; fi; {post} 2>/dev/null || true",
            env_guard()
        ),
        Some(user) => format!(
            "orbit_in=$(cat); if {}; then :; else printf '%s' \"$orbit_in\" | \
             {post} >/dev/null 2>&1 & fi; printf '%s' \"$orbit_in\" | eval {}",
            env_guard(),
            orbit_core::shell::single_quote(user)
        ),
    }
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

/// Permission rules letting Claude Code run orbit's own commands without a
/// permission prompt: the auto-title `orbit rename`, and the `orbit
/// worktree` relocation, `orbit spawn` sibling and `orbit open` file tabs
/// its appended system prompt tells it to use (codex/cursor run with their
/// skip-permissions flags).
const CLAUDE_ALLOW_RULES: &[&str] = &[
    "Bash(orbit rename:*)",
    "Bash(orbit worktree:*)",
    "Bash(orbit spawn:*)",
    "Bash(orbit open:*)",
    "Bash(orbit sessions:*)",
    "Bash(orbit read:*)",
    "Bash(orbit ask:*)",
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

fn is_orbit_command(cmd: Option<&Value>) -> bool {
    cmd.and_then(Value::as_str)
        .map(|c| c.contains("/api/hooks/") && c.contains(AGENT_ID))
        .unwrap_or(false)
}

fn is_orbit_group(group: &Value) -> bool {
    if group.get("_orbitManaged").and_then(Value::as_bool) == Some(true) {
        return true;
    }
    // Legacy/untagged detection by command signature — nested Claude/Codex
    // shape and flat Cursor shape both.
    if is_orbit_command(group.get("command")) {
        return true;
    }
    group
        .get("hooks")
        .and_then(Value::as_array)
        .map(|hooks| hooks.iter().any(|h| is_orbit_command(h.get("command"))))
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
    group.insert("_orbitManaged".into(), json!(true));
    Value::Object(group)
}

/// Merge orbit's managed hooks for Claude Code into
/// `<cwd>/.claude/settings.local.json`, plus the permission rule that lets
/// the auto-title `orbit rename` run unprompted.
pub fn install_claude_hooks(cwd: &Path) -> Result<()> {
    install_claude_hooks_with(cwd, orbit_core::paths::claude_config_dir().as_deref())
}

/// [`install_claude_hooks`], reading the user's own settings (for a status
/// line to chain) from `user_config` — Claude's config dir, injectable so
/// a test never reads the real one.
fn install_claude_hooks_with(cwd: &Path, user_config: Option<&Path>) -> Result<()> {
    // orbit's own commands, plus graphify's read-only graph queries spelled
    // the way this machine runs graphify (crate::graphify::allow_rules).
    let graphify = crate::graphify::allow_rules();
    let rules: Vec<&str> = CLAUDE_ALLOW_RULES
        .iter()
        .copied()
        .chain(graphify.iter().map(String::as_str))
        .collect();
    let dir = cwd.join(CLAUDE_DIR);
    let path = dir.join(CLAUDE_SETTINGS_FILE);
    let mut root = load_hooks_root(&path)?;
    let root_obj = root_object_mut(&mut root, &path)?;
    merge_managed_hooks(root_obj, "claude", CLAUDE_EVENTS, &path)?;
    for rule in &rules {
        ensure_permission_allow(root_obj, rule, &path)?;
    }
    let shared = [
        Some(dir.join(CLAUDE_SHARED_SETTINGS_FILE)),
        user_config.map(|d| d.join(CLAUDE_SHARED_SETTINGS_FILE)),
    ];
    merge_status_line(root_obj, shared.iter().flatten().map(PathBuf::as_path));
    write_hooks_root(&dir, CLAUDE_SETTINGS_FILE, &root)
}

/// Orbit's own status line: tagged, or (untagged) by its endpoint.
fn is_orbit_status_line(v: &Value) -> bool {
    v.get("_orbitManaged").and_then(Value::as_bool) == Some(true)
        || v.get("command")
            .and_then(Value::as_str)
            .is_some_and(|c| c.contains("/api/statusline") && c.contains(AGENT_ID))
}

/// `v` as a status line of the user's own — an object with a command,
/// and not orbit's.
fn user_status_line(v: &Value) -> Option<&str> {
    if is_orbit_status_line(v) {
        return None;
    }
    v.get("command")
        .and_then(Value::as_str)
        .filter(|c| !c.trim().is_empty())
}

/// The `statusLine` a settings file holds, read leniently: a file that is
/// missing or does not parse holds none (it is only read, never written).
fn settings_status_line(path: &Path) -> Option<Value> {
    let text = std::fs::read_to_string(path).ok()?;
    let root: Value = serde_json::from_str(&text).ok()?;
    root.get(STATUS_LINE_KEY).cloned()
}

/// Put orbit's managed `statusLine` into a loaded `settings.local.json`,
/// chaining the status line Claude would otherwise show. That is, in
/// Claude's own precedence: the user's line in this very file (as found,
/// or as orbit chained it last time — kept under [`CHAINED_KEY`]), else
/// the first of `shared` — the checkout's `settings.json`, then the user's
/// — that has one. The user's other settings for it (`padding`, …) are
/// kept. A `statusLine` here that is neither orbit's nor a status line at
/// all is left exactly as found, and so is nothing installed.
fn merge_status_line<'a>(
    root_obj: &mut Map<String, Value>,
    shared: impl IntoIterator<Item = &'a Path>,
) {
    let local = match root_obj.get(STATUS_LINE_KEY) {
        None => None,
        Some(v) if is_orbit_status_line(v) => v
            .get(CHAINED_KEY)
            .filter(|c| user_status_line(c).is_some())
            .cloned(),
        Some(v) if user_status_line(v).is_some() => Some(v.clone()),
        Some(_) => return,
    };
    let user = local.clone().or_else(|| {
        shared
            .into_iter()
            .filter_map(settings_status_line)
            .find(|v| user_status_line(v).is_some())
    });
    let mut line = Map::new();
    if let Some(Value::Object(user)) = &user {
        for (key, value) in user {
            if !matches!(key.as_str(), "type" | "command") && !key.starts_with("_orbit") {
                line.insert(key.clone(), value.clone());
            }
        }
    }
    let command = status_line_command(user.as_ref().and_then(user_status_line));
    line.insert("type".into(), json!("command"));
    line.insert("command".into(), json!(command));
    line.insert("_orbitManaged".into(), json!(true));
    if let Some(local) = local {
        line.insert(CHAINED_KEY.into(), local);
    }
    root_obj.insert(STATUS_LINE_KEY.into(), Value::Object(line));
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
/// object is not a config orbit understands, so it is left exactly as
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
    orbit_core::env::non_empty(orbit_core::env::CODEX_HOME)
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            orbit_core::env::home_dir()
                .unwrap_or_default()
                .join(CODEX_DIR)
        })
}

/// Merge orbit's managed hooks for Codex into `<codex_home>/hooks.json`.
pub fn install_codex_hooks(codex_home: &Path) -> Result<()> {
    install_managed_hooks(codex_home, HOOKS_FILE, "codex", CODEX_EVENTS)
}

/// Drop the per-worktree `.codex/hooks.json` groups an older orbit wrote:
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
    purge_orbit_groups(hooks_obj);
    let emptied = hooks_obj.is_empty();
    if emptied && before > 0 && root.as_object().map(|o| o.len()) == Some(1) {
        std::fs::remove_file(&path).with_context(|| format!("remove {}", path.display()))?;
        // Only ours to remove, and only while it holds nothing else.
        let _ = std::fs::remove_dir(&dir);
        return Ok(());
    }
    write_hooks_root(&dir, HOOKS_FILE, &root)
}

/// Strip orbit's groups from under EVERY event key of a loaded hooks
/// object (not just the ones about to be reinstalled: stale keys from an
/// older install shape must go, and events orbit may drop in the future
/// must not linger), then drop the keys that emptied. User groups are left
/// exactly as found.
fn purge_orbit_groups(hooks_obj: &mut Map<String, Value>) {
    for (_, groups) in hooks_obj.iter_mut() {
        if let Some(arr) = groups.as_array_mut() {
            arr.retain(|g| !is_orbit_group(g));
        }
    }
    hooks_obj.retain(|_, groups| groups.as_array().map(|a| !a.is_empty()).unwrap_or(true));
}

/// Merge orbit's managed hooks for Cursor into `<cwd>/.cursor/hooks.json`,
/// in Cursor's own dialect. Also migrates away the Claude-shaped groups an
/// older orbit wrote there (events Cursor never fires — the original
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
    purge_orbit_groups(hooks_obj);

    for (cursor_event, orbit_event) in CURSOR_EVENTS {
        let groups = hooks_obj
            .entry(cursor_event.to_string())
            .or_insert_with(|| json!([]));
        let groups_arr = array_mut(groups, &format!("hooks.{cursor_event}"), &path)?;
        groups_arr.push(json!({
            "command": cursor_hook_command(orbit_event),
            "_orbitManaged": true,
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

/// Write the managed file `name` holding `source` into `dir`, unless it
/// already holds exactly that — an unchanged mtime keeps the CLI's own
/// bookkeeping of the directory (pi's extension cache, OpenCode's plugin
/// scan) quiet.
pub(super) fn install_unless_unchanged(dir: &Path, name: &str, source: &str) -> Result<()> {
    if let Ok(existing) = std::fs::read_to_string(dir.join(name)) {
        if existing == source {
            return Ok(());
        }
    }
    write_text_atomic(dir, name, source)
}

pub(super) fn write_text_atomic(dir: &Path, file_name: &str, text: &str) -> Result<()> {
    std::fs::create_dir_all(dir)?;
    // Atomic write: tmp + rename.
    let tmp = dir.join(format!(".{file_name}.orbit-tmp"));
    let path = dir.join(file_name);
    std::fs::write(&tmp, text).with_context(|| format!("write {}", tmp.display()))?;
    std::fs::rename(&tmp, &path).with_context(|| format!("rename into {}", path.display()))?;
    Ok(())
}

/// Load `<dir>/<file_name>`, strip-and-rebuild orbit's groups for `events`
/// in the Claude/Codex dialect, and write it back — codex's hooks-only
/// file. (Claude's settings file carries permissions and a status line
/// too: `install_claude_hooks_with`.)
fn install_managed_hooks(
    dir: &Path,
    file_name: &str,
    endpoint: &str,
    events: &[(&str, Option<&str>)],
) -> Result<()> {
    let path = dir.join(file_name);
    let mut root = load_hooks_root(&path)?;

    let root_obj = root_object_mut(&mut root, &path)?;
    merge_managed_hooks(root_obj, endpoint, events, &path)?;
    write_hooks_root(dir, file_name, &root)
}

/// Strip-and-rebuild orbit's groups under each event key of a loaded
/// Claude/Codex-dialect config, leaving user groups untouched.
fn merge_managed_hooks(
    root_obj: &mut Map<String, Value>,
    endpoint: &str,
    events: &[(&str, Option<&str>)],
    path: &Path,
) -> Result<()> {
    let hooks = root_obj.entry("hooks").or_insert_with(|| json!({}));
    let hooks_obj = object_mut(hooks, "\"hooks\"", path)?;

    // One event may carry several managed groups (one per matcher), so the
    // strip-and-rebuild happens once per event name — stripping again for
    // a second matcher would delete the group the first one just added.
    let mut stripped: std::collections::HashSet<&str> = std::collections::HashSet::new();
    for (event, matcher) in events {
        let groups = hooks_obj
            .entry(event.to_string())
            .or_insert_with(|| json!([]));
        let groups_arr = array_mut(groups, &format!("hooks.{event}"), path)?;
        if stripped.insert(event) {
            groups_arr.retain(|g| !is_orbit_group(g));
        }
        groups_arr.push(managed_group(endpoint, event, *matcher));
    }
    Ok(())
}

/// Cursor can't receive daemon-injected context (its hooks are gating-only,
/// answering with their own JSON), so the auto-title instruction ships as a
/// managed always-on project rule instead. The file is wholly orbit-owned
/// (namespaced filename) and rewritten on every spawn. Firing outside
/// orbit or on later prompts is harmless: the rule env-guards itself, and
/// the daemon accepts at most one auto-title per session.
pub fn install_cursor_title_rule(cwd: &Path) -> Result<()> {
    let dir = cwd.join(CURSOR_DIR).join("rules");
    write_text_atomic(&dir, "orbit-title.mdc", &cursor_title_rule())
}

/// Same instruction the injectable CLIs get, wrapped in cursor's rule
/// frontmatter and an env guard (a project rule can't be switched off from
/// the daemon's side the way an injection can).
fn cursor_title_rule() -> String {
    format!(
        "---
description: Orbit session auto-title (managed by orbit — edits are overwritten)
alwaysApply: true
---

This rule applies only when the environment variable ORBIT_AGENT_ID is set
(the session runs inside orbit). If it is unset, ignore this rule entirely.

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
            "if [ -z \"$ORBIT_AGENT_ID\" ] || [ -z \"$ORBIT_API_URL\" ]; then exit 0; fi; \
             curl -sS -m 3 -X POST -H \"Authorization: Bearer $ORBIT_API_TOKEN\" \
             -H \"Content-Type: application/json\" --data-binary @- \
             \"$ORBIT_API_URL/api/hooks/claude?agentId=$ORBIT_AGENT_ID&hookEvent=UserPromptSubmit\" \
             2>/dev/null || true"
        );
        assert_eq!(
            hook_command("codex", "Stop"),
            "if [ -z \"$ORBIT_AGENT_ID\" ] || [ -z \"$ORBIT_API_URL\" ]; then exit 0; fi; \
             curl -sS -m 3 -X POST -H \"Authorization: Bearer $ORBIT_API_TOKEN\" \
             -H \"Content-Type: application/json\" --data-binary @- \
             \"$ORBIT_API_URL/api/hooks/codex?agentId=$ORBIT_AGENT_ID&hookEvent=Stop\" \
             >/dev/null 2>&1 || true"
        );
        assert_eq!(
            cursor_hook_command("Stop"),
            "if [ -z \"$ORBIT_AGENT_ID\" ] || [ -z \"$ORBIT_API_URL\" ]; then \
             printf '{\"continue\": true}\\n'; exit 0; fi; \
             curl -sS -m 3 -X POST -H \"Authorization: Bearer $ORBIT_API_TOKEN\" \
             -H \"Content-Type: application/json\" --data-binary @- \
             \"$ORBIT_API_URL/api/hooks/cursor?agentId=$ORBIT_AGENT_ID&hookEvent=Stop\" \
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
        assert_eq!(stop[0]["_orbitManaged"], json!(true));
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
    fn post_tool_use_is_one_unmatched_group_across_reinstalls() {
        // PostToolUse carries no matcher: a permission prompt's approval is
        // only ever visible as the gated tool's completion, whatever the
        // tool, and the AskUserQuestion answer and the cwd probe ride the
        // same group. A reinstall (every spawn) must leave exactly one —
        // and must replace the two matched groups an older orbit wrote.
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join(".claude");
        std::fs::create_dir_all(&dir).unwrap();
        let old = |matcher: &str| {
            let mut g = managed_group("claude", "PostToolUse", Some(matcher));
            g["_orbitManaged"] = json!(true);
            g
        };
        std::fs::write(
            dir.join("settings.local.json"),
            serde_json::to_string(&json!({
                "hooks": {
                    "PostToolUse": [old("AskUserQuestion"), old("Bash|EnterWorktree|ExitWorktree")]
                }
            }))
            .unwrap(),
        )
        .unwrap();
        install_claude_hooks(tmp.path()).unwrap();
        install_claude_hooks(tmp.path()).unwrap();
        let settings = read_settings(tmp.path());
        let groups = settings["hooks"]["PostToolUse"].as_array().unwrap();
        assert_eq!(groups.len(), 1, "{groups:?}");
        assert!(groups[0].get("matcher").is_none());
        assert_eq!(groups[0]["_orbitManaged"], json!(true));
        assert!(groups[0]["hooks"][0]["command"]
            .as_str()
            .unwrap()
            .contains("hookEvent=PostToolUse"));
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
        assert_eq!(stop[0]["_orbitManaged"], json!(true));
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
            orbit_core::env::home_dir()
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
                        { "_orbitManaged": true,
                          "hooks": [{ "type": "command",
                            "command": "curl $ORBIT_API_URL/api/hooks/codex?agentId=$ORBIT_AGENT_ID" }] },
                        { "_mcManaged": true,
                          "hooks": [{ "type": "command", "command": "curl $MC_API_URL/api/hooks/codex" }] }
                    ],
                    "UserPromptSubmit": [
                        { "_orbitManaged": true,
                          "hooks": [{ "type": "command",
                            "command": "curl $ORBIT_API_URL/api/hooks/codex?agentId=$ORBIT_AGENT_ID" }] }
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

        // Second worktree: the file was orbit's alone, so nothing is left
        // behind — not the file, not the directory it needed.
        let solo = tempfile::tempdir().unwrap();
        let solo_dir = solo.path().join(".codex");
        std::fs::create_dir_all(&solo_dir).unwrap();
        std::fs::write(
            solo_dir.join("hooks.json"),
            serde_json::to_string(&json!({
                "hooks": { "Stop": [
                    { "_orbitManaged": true,
                      "hooks": [{ "type": "command",
                        "command": "curl $ORBIT_API_URL/api/hooks/codex?agentId=$ORBIT_AGENT_ID" }] }
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
        let path = tmp.path().join(".cursor/rules/orbit-title.mdc");
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("alwaysApply: true"));
        assert!(text.contains("orbit rename"));
        assert!(text.contains("ORBIT_AGENT_ID"), "must be env-guarded");
        // Wholly orbit-owned: a scribbled-on file is simply replaced.
        std::fs::write(&path, "user scribbles").unwrap();
        install_cursor_title_rule(tmp.path()).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("orbit rename"));
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
        assert_eq!(stop.len(), 2, "user group + orbit group");
        assert_eq!(stop[0]["hooks"][0]["command"], json!("say done"));
        assert_eq!(stop[1]["_orbitManaged"], json!(true));
    }

    #[test]
    fn reinstall_does_not_accumulate_duplicates() {
        let tmp = tempfile::tempdir().unwrap();
        install_claude_hooks(tmp.path()).unwrap();
        install_claude_hooks(tmp.path()).unwrap();
        install_claude_hooks(tmp.path()).unwrap();
        let settings = read_settings(tmp.path());
        for (event, _) in CLAUDE_EVENTS {
            // One group per (event, matcher) pair.
            let expected = CLAUDE_EVENTS.iter().filter(|(e, _)| e == event).count();
            assert_eq!(
                settings["hooks"][*event].as_array().unwrap().len(),
                expected,
                "{event} accumulated duplicates"
            );
        }
    }

    #[test]
    fn strips_legacy_untagged_orbit_groups() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join(".claude");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("settings.local.json"),
            serde_json::to_string(&json!({
                "hooks": {
                    "Stop": [
                        // Old orbit install without the marker.
                        { "hooks": [{ "type": "command",
                            "command": "curl $ORBIT_API_URL/api/hooks/claude?agentId=$ORBIT_AGENT_ID" }] }
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
        assert_eq!(stop.len(), 2, "foreign managed group + orbit group");
        assert_eq!(stop[0]["_mcManaged"], json!(true));
        assert_eq!(stop[1]["_orbitManaged"], json!(true));
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
        assert_eq!(stop[0]["_orbitManaged"], json!(true));
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
        // An older orbit wrote Claude-dialect groups (PascalCase events,
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
                        { "_orbitManaged": true,
                          "hooks": [{ "type": "command",
                            "command": "curl $ORBIT_API_URL/api/hooks/cursor?agentId=$ORBIT_AGENT_ID&hookEvent=Stop" }] }
                    ],
                    "UserPromptSubmit": [
                        { "_orbitManaged": true,
                          "hooks": [{ "type": "command",
                            "command": "curl $ORBIT_API_URL/api/hooks/cursor?agentId=$ORBIT_AGENT_ID&hookEvent=UserPromptSubmit" }] }
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
        assert_eq!(stop.len(), 2, "mc group + orbit group");
        assert_eq!(stop[0]["_mcManaged"], json!(true));
        assert_eq!(stop[1]["_orbitManaged"], json!(true));
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

    /// Install with an empty user config dir, so no test reads the real
    /// `~/.claude`.
    fn install_claude_isolated(cwd: &Path, user_config: &Path) {
        install_claude_hooks_with(cwd, Some(user_config)).unwrap();
    }

    fn write_settings(dir: &Path, rel: &str, value: Value) {
        let path = dir.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, serde_json::to_string(&value).unwrap()).unwrap();
    }

    #[test]
    fn status_line_commands_are_spelled_exactly() {
        assert_eq!(
            status_line_command(None),
            "if [ -z \"$ORBIT_AGENT_ID\" ] || [ -z \"$ORBIT_API_URL\" ]; then exit 0; fi; \
             curl -sS -m 1 -X POST -H \"Authorization: Bearer $ORBIT_API_TOKEN\" \
             -H \"Content-Type: application/json\" --data-binary @- \
             \"$ORBIT_API_URL/api/statusline?agentId=$ORBIT_AGENT_ID\" 2>/dev/null || true"
        );
        assert_eq!(
            status_line_command(Some("~/bin/line.sh 'x'")),
            "orbit_in=$(cat); if [ -z \"$ORBIT_AGENT_ID\" ] || [ -z \"$ORBIT_API_URL\" ]; \
             then :; else printf '%s' \"$orbit_in\" | \
             curl -sS -m 1 -X POST -H \"Authorization: Bearer $ORBIT_API_TOKEN\" \
             -H \"Content-Type: application/json\" --data-binary @- \
             \"$ORBIT_API_URL/api/statusline?agentId=$ORBIT_AGENT_ID\" >/dev/null 2>&1 & fi; \
             printf '%s' \"$orbit_in\" | eval '~/bin/line.sh '\\''x'\\'''"
        );
    }

    #[test]
    fn status_line_is_installed_into_a_fresh_file_and_reinstalled_unchanged() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tempfile::tempdir().unwrap();
        install_claude_isolated(tmp.path(), home.path());
        let first = read_settings(tmp.path())["statusLine"].clone();
        assert_eq!(first["_orbitManaged"], json!(true));
        assert_eq!(first["type"], json!("command"));
        assert_eq!(first["command"], json!(status_line_command(None)));
        assert!(first.get(CHAINED_KEY).is_none());
        install_claude_isolated(tmp.path(), home.path());
        install_claude_isolated(tmp.path(), home.path());
        assert_eq!(read_settings(tmp.path())["statusLine"], first, "idempotent");
    }

    /// The user's own status line in settings.local.json is displaced, not
    /// lost: orbit's chains it (padding kept), carries the original, and
    /// every reinstall chains the original again rather than itself.
    #[test]
    fn a_local_user_status_line_is_chained_across_reinstalls() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tempfile::tempdir().unwrap();
        let theirs = json!({ "type": "command", "command": "~/bin/line.sh 'x'", "padding": 2 });
        write_settings(
            tmp.path(),
            ".claude/settings.local.json",
            json!({ "statusLine": theirs.clone(), "model": "opus" }),
        );
        // A user-level one loses to the checkout's own, as in Claude.
        write_settings(
            home.path(),
            "settings.json",
            json!({ "statusLine": { "type": "command", "command": "home-line" } }),
        );
        for _ in 0..3 {
            install_claude_isolated(tmp.path(), home.path());
            let settings = read_settings(tmp.path());
            let line = &settings["statusLine"];
            assert_eq!(line["_orbitManaged"], json!(true));
            assert_eq!(
                line["command"],
                json!(status_line_command(Some("~/bin/line.sh 'x'")))
            );
            assert_eq!(line["padding"], json!(2));
            assert_eq!(line[CHAINED_KEY], theirs);
            assert_eq!(settings["model"], json!("opus"), "other settings kept");
        }
    }

    /// No status line in the checkout: the checkout's checked-in one is
    /// chained, else the user's own from their Claude config dir.
    #[test]
    fn a_shared_or_user_status_line_is_chained() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tempfile::tempdir().unwrap();
        write_settings(
            home.path(),
            "settings.json",
            json!({ "statusLine": { "type": "command", "command": "home-line", "padding": 0 } }),
        );
        install_claude_isolated(tmp.path(), home.path());
        let line = read_settings(tmp.path())["statusLine"].clone();
        assert_eq!(
            line["command"],
            json!(status_line_command(Some("home-line")))
        );
        assert_eq!(line["padding"], json!(0));
        assert!(line.get(CHAINED_KEY).is_none(), "it still lives at home");

        write_settings(
            tmp.path(),
            ".claude/settings.json",
            json!({ "statusLine": { "type": "command", "command": "repo-line" } }),
        );
        install_claude_isolated(tmp.path(), home.path());
        let line = read_settings(tmp.path())["statusLine"].clone();
        assert_eq!(
            line["command"],
            json!(status_line_command(Some("repo-line")))
        );
        assert!(line.get("padding").is_none());

        // Gone from both: orbit's own, unchained.
        std::fs::remove_file(tmp.path().join(".claude/settings.json")).unwrap();
        std::fs::remove_file(home.path().join("settings.json")).unwrap();
        install_claude_isolated(tmp.path(), home.path());
        let line = read_settings(tmp.path())["statusLine"].clone();
        assert_eq!(line["command"], json!(status_line_command(None)));
    }

    /// A `statusLine` orbit can't read as one is left exactly as found.
    #[test]
    fn an_unreadable_status_line_is_left_alone() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tempfile::tempdir().unwrap();
        write_settings(
            tmp.path(),
            ".claude/settings.local.json",
            json!({ "statusLine": "weird" }),
        );
        install_claude_isolated(tmp.path(), home.path());
        let settings = read_settings(tmp.path());
        assert_eq!(settings["statusLine"], json!("weird"));
        assert!(
            settings["hooks"]["Stop"].is_array(),
            "hooks still installed"
        );
    }

    /// The commands under a real shell: outside orbit the bare one prints
    /// nothing and the chained one runs the user's command on the same
    /// stdin; inside orbit with the daemon unreachable, the user's line
    /// still draws.
    #[test]
    fn status_line_commands_run_under_sh() {
        use std::io::Write;
        use std::process::{Command, Stdio};
        let run = |cmd: &str, api_url: Option<&str>| -> (String, std::time::Duration) {
            let mut c = Command::new("sh");
            c.arg("-c")
                .arg(cmd)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .env_remove(AGENT_ID)
                .env_remove(API_URL)
                .env_remove(API_TOKEN);
            if let Some(url) = api_url {
                c.env(AGENT_ID, "a1").env(API_URL, url).env(API_TOKEN, "t");
            }
            let started = std::time::Instant::now();
            let mut child = c.spawn().unwrap();
            child
                .stdin
                .take()
                .unwrap()
                .write_all(br#"{"model":{"display_name":"Opus"}}"#)
                .unwrap();
            let out = child.wait_with_output().unwrap();
            assert!(out.status.success(), "{cmd}: {out:?}");
            (
                String::from_utf8_lossy(&out.stdout).into_owned(),
                started.elapsed(),
            )
        };
        let user = "sed 's/^/user: /'";
        assert_eq!(run(&status_line_command(None), None).0, "");
        assert_eq!(
            run(&status_line_command(Some(user)), None).0,
            "user: {\"model\":{\"display_name\":\"Opus\"}}"
        );
        // Port 9 (discard) on loopback: nothing listens, curl fails fast.
        let dead = Some("http://127.0.0.1:9");
        let (out, took) = run(&status_line_command(Some(user)), dead);
        assert!(out.starts_with("user: {"), "{out}");
        assert!(took < std::time::Duration::from_secs(3), "{took:?}");
        assert_eq!(run(&status_line_command(None), dead).0, "");
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
