//! The Claude MODEL list.
//!
//! `claude --model` takes an alias (`sonnet`) or a full model id
//! (`claude-sonnet-5`, a Bedrock ARN, a gateway path). The built-in
//! [`CLAUDE_MODELS`] aliases are right for a personal Anthropic account and
//! wrong for two other set-ups:
//!
//! - An organization that restricts models with Claude Code's
//!   `availableModels` allowlist, delivered as managed settings. When the
//!   allowlist names full ids, `--model sonnet` is refused at startup —
//!   `Model "sonnet" is restricted by your organization's settings. Using
//!   claude-sonnet-5 instead.` — and on Bedrock, Vertex and Foundry the
//!   alias even resolves to an older generation than the allowlist wants.
//! - A third-party provider whose ids look nothing like the aliases.
//!
//! So the list is resolved from three sources, first non-empty wins:
//!
//! 1. `claude_models` in CONFIG.JSON — the user's own list, verbatim,
//!    synced on every [`crate::config::Config::load`] so a hand edit applies
//!    without a restart.
//! 2. `availableModels` from Claude Code's own settings, read once at TUI
//!    startup ([`bootstrap`]) in Claude Code's precedence: the
//!    server-managed cache, the macOS MDM profile, the managed settings
//!    file and its drop-in directory, then the user's `settings.json`.
//! 3. [`CLAUDE_MODELS`], the aliases.
//!
//! [`DEFAULT_CHOICE`] always heads the list — "pass no flag" is right under
//! any policy. Lists are handed out as `&'static` slices, the shape every
//! MODEL / EFFORT surface already takes, by leaking a new one only when the
//! resolved list actually changes.

use std::path::{Path, PathBuf};
use std::sync::{OnceLock, RwLock};

use crate::config::{CLAUDE_MODELS, DEFAULT_CHOICE};

/// The settings key Claude Code reads its allowlist from.
const AVAILABLE_MODELS_KEY: &str = "availableModels";

/// Where Claude Code caches server-managed settings, under its config dir.
const REMOTE_SETTINGS_FILE: &str = "remote-settings.json";

/// The macOS managed-preferences domain an MDM profile writes.
#[cfg(target_os = "macos")]
const MDM_DOMAIN: &str = "com.anthropic.claudecode";

// --- resolution ------------------------------------------------------------

/// The choice list for the given sources: [`DEFAULT_CHOICE`] first, then the
/// first non-empty of `configured` (CONFIG.JSON) and `available` (Claude
/// Code's allowlist), trimmed and deduplicated case-insensitively; the
/// built-in aliases when both are empty.
pub fn resolve(configured: &[String], available: &[String]) -> Vec<String> {
    let entries = |list: &[String]| -> Vec<String> {
        let mut out = vec![DEFAULT_CHOICE.to_string()];
        for model in list {
            let model = model.trim();
            if model.is_empty() || out.iter().any(|m| m.eq_ignore_ascii_case(model)) {
                continue;
            }
            out.push(model.to_string());
        }
        out
    };
    // A list of nothing but blanks (or only "default") is no list.
    [configured, available]
        .into_iter()
        .map(entries)
        .find(|list| list.len() > 1)
        .unwrap_or_else(|| CLAUDE_MODELS.iter().map(|m| m.to_string()).collect())
}

struct State {
    configured: Vec<String>,
    available: Vec<String>,
    installed: &'static [&'static str],
}

static CURRENT: OnceLock<RwLock<State>> = OnceLock::new();

fn state() -> &'static RwLock<State> {
    CURRENT.get_or_init(|| {
        RwLock::new(State {
            configured: Vec::new(),
            available: Vec::new(),
            installed: CLAUDE_MODELS,
        })
    })
}

/// The MODEL choices for Claude, as the pickers, the AGENTS TAB and the
/// PRESET EDITOR list them.
pub fn models() -> &'static [&'static str] {
    state()
        .read()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .installed
}

fn leak(list: &[String]) -> &'static [&'static str] {
    if list
        .iter()
        .map(String::as_str)
        .eq(CLAUDE_MODELS.iter().copied())
    {
        return CLAUDE_MODELS;
    }
    let leaked: Vec<&'static str> = list
        .iter()
        .map(|m| {
            if m == DEFAULT_CHOICE {
                DEFAULT_CHOICE
            } else {
                Box::leak(m.clone().into_boxed_str()) as &'static str
            }
        })
        .collect();
    Box::leak(leaked.into_boxed_slice())
}

fn reinstall(state: &mut State) {
    let list = resolve(&state.configured, &state.available);
    if list
        .iter()
        .map(String::as_str)
        .eq(state.installed.iter().copied())
    {
        return;
    }
    state.installed = leak(&list);
}

/// Adopt CONFIG.JSON's `claude_models`. Called from every `Config::load`
/// outside tests, so it is cheap when nothing changed and leaks only when
/// the list did. Not called from tests: the view is process-global.
pub fn sync_config(configured: &[String]) {
    let mut state = state()
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if state.configured == configured {
        return;
    }
    state.configured = configured.to_vec();
    reinstall(&mut state);
}

/// Adopt Claude Code's `availableModels`.
pub fn install_available(available: Vec<String>) {
    let mut state = state()
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if state.available == available {
        return;
    }
    state.available = available;
    reinstall(&mut state);
}

// --- Claude Code's settings ------------------------------------------------

/// Claude Code's config dir: `$CLAUDE_CONFIG_DIR`, else `~/.claude`.
fn claude_config_dir() -> Option<PathBuf> {
    nebula_core::env::non_empty("CLAUDE_CONFIG_DIR")
        .map(PathBuf::from)
        .or_else(|| nebula_core::env::home_dir().map(|home| home.join(".claude")))
}

/// The system directory managed settings files live in.
fn managed_dir() -> PathBuf {
    if cfg!(target_os = "macos") {
        PathBuf::from("/Library/Application Support/ClaudeCode")
    } else if cfg!(windows) {
        PathBuf::from(r"C:\Program Files\ClaudeCode")
    } else {
        PathBuf::from("/etc/claude-code")
    }
}

/// `availableModels` out of one settings document: an array of strings at
/// the top level, or — the server-managed cache wraps its payload — under a
/// `settings` object. Entries that are not strings are skipped.
pub fn available_models_in(doc: &serde_json::Value) -> Option<Vec<String>> {
    let list = doc
        .get(AVAILABLE_MODELS_KEY)
        .or_else(|| doc.get("settings")?.get(AVAILABLE_MODELS_KEY))?
        .as_array()?;
    Some(
        list.iter()
            .filter_map(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(String::from)
            .collect(),
    )
}

fn read_json(path: &Path) -> Option<serde_json::Value> {
    let raw = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&raw).ok()
}

fn available_models_in_file(path: &Path) -> Option<Vec<String>> {
    available_models_in(&read_json(path)?)
}

/// The managed settings file plus every `*.json` in its drop-in directory
/// in name order, the way Claude Code merges them: a later file's
/// `availableModels` replaces an earlier one's.
fn available_models_in_managed_files(dir: &Path) -> Option<Vec<String>> {
    let mut found = available_models_in_file(&dir.join("managed-settings.json"));
    let mut drop_ins: Vec<PathBuf> = std::fs::read_dir(dir.join("managed-settings.d"))
        .map(|entries| {
            entries
                .flatten()
                .map(|e| e.path())
                .filter(|p| {
                    p.extension().is_some_and(|ext| ext == "json")
                        && !p
                            .file_name()
                            .and_then(|n| n.to_str())
                            .is_some_and(|n| n.starts_with('.'))
                })
                .collect()
        })
        .unwrap_or_default();
    drop_ins.sort();
    for path in drop_ins {
        if let Some(list) = available_models_in_file(&path) {
            found = Some(list);
        }
    }
    found
}

/// The MDM profile, per-user then machine-wide, converted with `plutil`.
/// Only shells out when the profile file exists.
#[cfg(target_os = "macos")]
fn available_models_in_mdm() -> Option<Vec<String>> {
    let file = format!("{MDM_DOMAIN}.plist");
    let mut paths = Vec::new();
    if let Some(user) = nebula_core::env::non_empty("USER") {
        paths.push(
            PathBuf::from("/Library/Managed Preferences")
                .join(user)
                .join(&file),
        );
    }
    paths.push(PathBuf::from("/Library/Managed Preferences").join(&file));
    paths.into_iter().filter(|p| p.is_file()).find_map(|path| {
        let output = std::process::Command::new("plutil")
            .args(["-convert", "json", "-o", "-"])
            .arg(&path)
            .stdin(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .output()
            .ok()?;
        if !output.status.success() {
            return None;
        }
        available_models_in(&serde_json::from_slice(&output.stdout).ok()?)
    })
}

#[cfg(not(target_os = "macos"))]
fn available_models_in_mdm() -> Option<Vec<String>> {
    None
}

/// The first `availableModels` Claude Code would apply from on-disk
/// sources, in its own precedence: the server-managed cache, the MDM
/// profile, the managed files, then the user's own `settings.json`.
/// `cfg_dir` is Claude Code's config dir, `sys_dir` the managed one.
pub fn available_models_from(cfg_dir: &Path, sys_dir: &Path) -> Option<Vec<String>> {
    available_models_in_file(&cfg_dir.join(REMOTE_SETTINGS_FILE))
        .or_else(available_models_in_mdm)
        .or_else(|| available_models_in_managed_files(sys_dir))
        .or_else(|| available_models_in_file(&cfg_dir.join("settings.json")))
}

/// TUI startup: adopt Claude Code's allowlist when one is on disk. Skipped
/// when Claude is switched off (HARNESS TOGGLE) and under `NEBULA_AGENT_CMD`
/// (tests, STUB AGENTS), so no test reads the developer's real settings.
pub fn bootstrap(claude_enabled: bool) {
    if !claude_enabled || nebula_core::env::non_empty(nebula_core::env::AGENT_CMD).is_some() {
        return;
    }
    let Some(cfg_dir) = claude_config_dir() else {
        return;
    };
    if let Some(list) = available_models_from(&cfg_dir, &managed_dir()) {
        tracing::info!("claude models from Claude Code's availableModels: {list:?}");
        install_available(list);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strings(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn resolve_prefers_config_then_allowlist_then_builtins() {
        assert_eq!(resolve(&[], &[]), strings(CLAUDE_MODELS));
        assert_eq!(
            resolve(&[], &strings(&["claude-sonnet-5", "claude-opus-5"])),
            strings(&["default", "claude-sonnet-5", "claude-opus-5"])
        );
        assert_eq!(
            resolve(
                &strings(&["us.anthropic.claude-sonnet-5-v1:0"]),
                &strings(&["claude-sonnet-5"])
            ),
            strings(&["default", "us.anthropic.claude-sonnet-5-v1:0"]),
            "the user's own list wins over the allowlist"
        );
    }

    #[test]
    fn resolve_keeps_default_first_and_dedups() {
        assert_eq!(
            resolve(
                &strings(&[" sonnet ", "", "default", "SONNET", "opus[1m]"]),
                &[]
            ),
            strings(&["default", "sonnet", "opus[1m]"])
        );
        // A list of nothing but blanks is not a list.
        assert_eq!(
            resolve(&strings(&["", "  "]), &strings(&["haiku"])),
            strings(&["default", "haiku"]),
        );
    }

    #[test]
    fn available_models_reads_top_level_and_wrapped_shapes() {
        let top: serde_json::Value = serde_json::json!({
            "availableModels": ["sonnet", 3, {"value": "x"}, " claude-opus-5 ", ""]
        });
        assert_eq!(
            available_models_in(&top),
            Some(strings(&["sonnet", "claude-opus-5"]))
        );
        let wrapped = serde_json::json!({"settings": {"availableModels": ["haiku"]}});
        assert_eq!(available_models_in(&wrapped), Some(strings(&["haiku"])));
        assert_eq!(
            available_models_in(&serde_json::json!({"model": "opus"})),
            None
        );
        assert_eq!(
            available_models_in(&serde_json::json!({"availableModels": "sonnet"})),
            None,
            "not an array"
        );
    }

    #[test]
    fn sources_follow_claude_code_precedence() {
        let root = tempfile::tempdir().unwrap();
        let cfg_dir = root.path().join("claude");
        let sys_dir = root.path().join("managed");
        std::fs::create_dir_all(&cfg_dir).unwrap();
        std::fs::create_dir_all(sys_dir.join("managed-settings.d")).unwrap();

        // Nothing on disk: no allowlist.
        assert_eq!(available_models_from(&cfg_dir, &sys_dir), None);

        // The user's own settings.json is the last resort.
        std::fs::write(
            cfg_dir.join("settings.json"),
            r#"{"availableModels": ["from-user"]}"#,
        )
        .unwrap();
        assert_eq!(
            available_models_from(&cfg_dir, &sys_dir),
            Some(strings(&["from-user"]))
        );

        // Managed files beat it; drop-ins in name order, the last one wins.
        std::fs::write(
            sys_dir.join("managed-settings.json"),
            r#"{"availableModels": ["from-file"]}"#,
        )
        .unwrap();
        assert_eq!(
            available_models_from(&cfg_dir, &sys_dir),
            Some(strings(&["from-file"]))
        );
        std::fs::write(
            sys_dir.join("managed-settings.d/20-models.json"),
            r#"{"availableModels": ["from-20"]}"#,
        )
        .unwrap();
        std::fs::write(
            sys_dir.join("managed-settings.d/10-models.json"),
            r#"{"availableModels": ["from-10"]}"#,
        )
        .unwrap();
        std::fs::write(
            sys_dir.join("managed-settings.d/30-other.json"),
            r#"{"permissions": {}}"#,
        )
        .unwrap();
        std::fs::write(
            sys_dir.join("managed-settings.d/.hidden.json"),
            r#"{"availableModels": ["hidden"]}"#,
        )
        .unwrap();
        std::fs::write(
            sys_dir.join("managed-settings.d/40-models.txt"),
            r#"{"availableModels": ["txt"]}"#,
        )
        .unwrap();
        assert_eq!(
            available_models_from(&cfg_dir, &sys_dir),
            Some(strings(&["from-20"]))
        );

        // The server-managed cache outranks every file.
        std::fs::write(
            cfg_dir.join(REMOTE_SETTINGS_FILE),
            r#"{"settings": {"availableModels": ["from-remote"]}}"#,
        )
        .unwrap();
        assert_eq!(
            available_models_from(&cfg_dir, &sys_dir),
            Some(strings(&["from-remote"]))
        );

        // A malformed file is skipped, not fatal.
        std::fs::write(cfg_dir.join(REMOTE_SETTINGS_FILE), "{not json").unwrap();
        assert_eq!(
            available_models_from(&cfg_dir, &sys_dir),
            Some(strings(&["from-20"]))
        );
    }
}
