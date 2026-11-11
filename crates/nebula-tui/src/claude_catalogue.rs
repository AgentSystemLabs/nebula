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
//!
//! # MODEL NAMES
//!
//! The ids above are what `--model` takes; they are not what a person reads.
//! `fable` says nothing about which Fable, and Claude Code's own `/model`
//! picker lists `Fable 5.2`. Claude Code keeps the catalog that picker is
//! drawn from on disk, refreshed every few minutes while it runs:
//!
//! ```text
//! <config dir>/cache/model-catalog/<key>-cc.json
//! {"fetchedAt":…,"catalog":{"surface":"cc","config":{
//!   "models":[{"id":"claude-fable-5-2","name":"Fable 5.2",…},…],
//!   "provider_alias_targets":{"fable":{"default":"claude-fable-5-2",
//!                                      "per_provider":{"gateway":"claude-fable-5"}},…}}}}
//! ```
//!
//! (verified on 2.1.322; the signed `published-*.json` fallback carries the
//! same two tables under `document.surfaces.cc.model_selector_config`).
//! [`label`] maps a stored id through the newest of those files — alias to
//! id to name — so every surface that prints a Claude model prints the name
//! Claude Code would: `fable` → `Fable 5.2`, `opus[1m]` → `Opus 5.5 (1M
//! context)`. The stored id never changes; an id the catalog does not name
//! (a Bedrock ARN, a gateway path, any id on a machine with no catalog)
//! prints as it is.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{OnceLock, RwLock};

use crate::config::DEFAULT_CHOICE;

/// The built-in Claude model aliases, [`DEFAULT_CHOICE`] first: what the
/// pickers offer when neither CONFIG.JSON's `claude_models` nor Claude
/// Code's own `availableModels` allowlist names anything.
pub const CLAUDE_MODELS: &[&str] = &[DEFAULT_CHOICE, "fable", "opus", "sonnet", "haiku"];

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
    names: ModelNames,
}

static CURRENT: OnceLock<RwLock<State>> = OnceLock::new();

fn state() -> &'static RwLock<State> {
    CURRENT.get_or_init(|| {
        RwLock::new(State {
            configured: Vec::new(),
            available: Vec::new(),
            installed: CLAUDE_MODELS,
            names: ModelNames::default(),
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

// --- Claude Code's model names ---------------------------------------------

/// Where Claude Code caches its model catalog, under its config dir.
const MODEL_CATALOG_DIR: &str = "cache/model-catalog";

/// The catalog surface Claude Code's own `/model` picker is drawn from
/// (`ccd` is the desktop app's, `chat` claude.ai's).
const CATALOG_SURFACE: &str = "cc";

/// A file in the catalog dir larger than this is not read. The signed
/// fallback document, the largest seen, is under 300 KB.
const CATALOG_MAX_BYTES: u64 = 4 * 1024 * 1024;

/// How often a running TUI re-reads the catalog, so a model Claude Code
/// learns of mid-session is named without a restart.
const NAMES_REFRESH: std::time::Duration = std::time::Duration::from_secs(600);

/// The provider switches Claude Code reads, in the order it tests them,
/// with the key each selects in the catalog's `per_provider` tables.
const PROVIDER_SWITCHES: &[(&str, &str)] = &[
    ("CLAUDE_CODE_USE_GATEWAY", "gateway"),
    ("CLAUDE_CODE_USE_BEDROCK", "bedrock"),
    ("CLAUDE_CODE_USE_FOUNDRY", "foundry"),
    ("CLAUDE_CODE_USE_ANTHROPIC_AWS", "anthropic_aws"),
    (
        "CLAUDE_CODE_USE_ANTHROPIC_GOOGLE_CLOUD",
        "anthropic_google_cloud",
    ),
    ("CLAUDE_CODE_USE_MANTLE", "mantle"),
    ("CLAUDE_CODE_USE_VERTEX", "vertex"),
];

/// Claude Code's names for its models (MODEL NAMES in the module docs):
/// which id each alias means on this machine, and what each id is called.
/// Keys are lowercase.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ModelNames {
    /// `fable` → `claude-fable-5-2`.
    aliases: HashMap<String, String>,
    /// `claude-fable-5-2` → `Fable 5.2`.
    names: HashMap<String, String>,
}

impl ModelNames {
    /// The two tables out of one catalog cache document, in either shape
    /// Claude Code writes: the served catalog (`catalog.config`) or the
    /// signed fallback (`document.surfaces.cc.model_selector_config[]`).
    /// `provider` picks an alias's `per_provider` target over its default.
    /// None when the document is not the `cc` surface's or names no model.
    pub fn in_catalog(doc: &serde_json::Value, provider: Option<&str>) -> Option<Self> {
        let text = |v: &serde_json::Value, key: &str| -> Option<String> {
            Some(v.get(key)?.as_str()?.trim().to_string()).filter(|s| !s.is_empty())
        };
        let served = doc
            .get("catalog")
            .filter(|c| text(c, "surface").as_deref() == Some(CATALOG_SURFACE))
            .and_then(|c| c.get("config"));
        let published = || {
            doc.get("document")?
                .get("surfaces")?
                .get(CATALOG_SURFACE)?
                .get("model_selector_config")?
                .as_array()?
                .iter()
                .find(|c| text(c, "id").as_deref() == Some(CATALOG_SURFACE))
        };
        let config = served.or_else(published)?;

        let names: HashMap<String, String> = config
            .get("models")?
            .as_array()?
            .iter()
            .filter_map(|m| Some((text(m, "id")?.to_ascii_lowercase(), text(m, "name")?)))
            .collect();
        if names.is_empty() {
            return None;
        }
        let aliases = config
            .get("provider_alias_targets")
            .and_then(|t| t.as_object())
            .map(|targets| {
                targets
                    .iter()
                    .filter_map(|(alias, target)| {
                        let id = provider
                            .and_then(|p| text(target.get("per_provider")?, p))
                            .or_else(|| text(target, "default"))?;
                        Some((alias.trim().to_ascii_lowercase(), id.to_ascii_lowercase()))
                    })
                    .collect()
            })
            .unwrap_or_default();
        Some(Self { aliases, names })
    }

    /// Claude Code's name for `model` — an alias or a full id, with or
    /// without the `[1m]` context suffix, which reads `(1M context)` the
    /// way `/model` prints it. None for an id the catalog does not name.
    pub fn label(&self, model: &str) -> Option<String> {
        let lower = model.trim().to_ascii_lowercase();
        let (base, long) = match lower.strip_suffix("[1m]") {
            Some(base) => (base.trim_end(), true),
            None => (lower.as_str(), false),
        };
        let id = self.aliases.get(base).map_or(base, String::as_str);
        let name = self.names.get(id)?;
        Some(if long {
            format!("{name} (1M context)")
        } else {
            name.clone()
        })
    }
}

/// The provider Claude Code is pointed at, as the catalog's `per_provider`
/// tables key it; None on Anthropic's own API. A switch counts from the
/// environment or from the `env` block of the user's `settings.json`.
fn provider(cfg_dir: &Path) -> Option<&'static str> {
    let settings = read_json(&cfg_dir.join("settings.json"));
    let set = |key: &str| {
        nebula_core::env::non_empty(key).is_some()
            || settings
                .as_ref()
                .and_then(|s| s.get("env")?.get(key)?.as_str())
                .is_some_and(|v| !v.trim().is_empty())
    };
    PROVIDER_SWITCHES
        .iter()
        .find(|(key, _)| set(key))
        .map(|(_, provider)| *provider)
}

/// The names in the newest catalog Claude Code has cached under `cfg_dir`
/// (by the `fetchedAt` each file records); empty when it has cached none.
pub fn names_from(cfg_dir: &Path, provider: Option<&str>) -> ModelNames {
    let Ok(entries) = std::fs::read_dir(cfg_dir.join(MODEL_CATALOG_DIR)) else {
        return ModelNames::default();
    };
    entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|ext| ext == "json"))
        .filter(|p| std::fs::metadata(p).is_ok_and(|m| m.is_file() && m.len() <= CATALOG_MAX_BYTES))
        .filter_map(|path| {
            let doc = read_json(&path)?;
            let fetched = doc.get("fetchedAt").and_then(|v| v.as_u64()).unwrap_or(0);
            Some((fetched, ModelNames::in_catalog(&doc, provider)?))
        })
        .max_by_key(|(fetched, _)| *fetched)
        .map(|(_, names)| names)
        .unwrap_or_default()
}

/// Adopt Claude Code's names. Not called from tests: the view is
/// process-global, so a test's cards keep printing the ids they store.
pub fn install_names(names: ModelNames) {
    let mut state = state()
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if state.names != names {
        state.names = names;
    }
}

/// Claude Code's name for a Claude model id, when its catalog names it:
/// `fable` → `Fable 5.2`. Callers print the id itself on None; they reach
/// this through [`crate::config::model_label`].
pub fn label(model: &str) -> Option<String> {
    state()
        .read()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .names
        .label(model)
}

/// TUI startup: adopt Claude Code's allowlist when one is on disk, and its
/// model names, which a background thread then keeps current. Skipped
/// when Claude is switched off (HARNESS TOGGLE) and under `NEBULA_AGENT_CMD`
/// (tests, STUB AGENTS), so no test reads the developer's real settings.
pub fn bootstrap(claude_enabled: bool) {
    if !claude_enabled || nebula_core::env::non_empty(nebula_core::env::AGENT_CMD).is_some() {
        return;
    }
    let Some(cfg_dir) = nebula_core::paths::claude_config_dir() else {
        return;
    };
    if let Some(list) = available_models_from(&cfg_dir, &managed_dir()) {
        tracing::info!("claude models from Claude Code's availableModels: {list:?}");
        install_available(list);
    }
    install_names(names_from(&cfg_dir, provider(&cfg_dir)));
    // Off the event loop: the catalog is a directory of JSON files, and
    // nothing on the drawing side may wait on a disk read.
    std::thread::spawn(move || loop {
        std::thread::sleep(NAMES_REFRESH);
        install_names(names_from(&cfg_dir, provider(&cfg_dir)));
    });
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

    /// A served catalog cache file, cut down to the fields that are read.
    fn served(fetched_at: u64, surface: &str, fable: (&str, &str)) -> serde_json::Value {
        serde_json::json!({
            "version": 2,
            "fetchedAt": fetched_at,
            "catalog": {"surface": surface, "config": {
                "id": surface,
                "models": [
                    {"id": "claude-opus-5-5", "name": "Opus 5.5", "short_name": "Opus"},
                    {"id": fable.0, "name": fable.1, "short_name": "Fable"},
                    {"id": "claude-opus-4-6", "name": "Opus 4.6"},
                    {"id": "claude-nameless"},
                ],
                "provider_alias_targets": {
                    "fable": {"default": fable.0},
                    "opus": {
                        "default": "claude-opus-5-5",
                        "per_provider": {"foundry": "claude-opus-4-6", "mantle": "anthropic.opus"}
                    },
                    "haiku": {"default": "claude-haiku-5-5"},
                },
            }},
        })
    }

    #[test]
    fn a_model_reads_as_claude_code_lists_it() {
        let doc = served(1, "cc", ("claude-fable-5-2", "Fable 5.2"));
        let names = ModelNames::in_catalog(&doc, None).unwrap();
        // The alias a row stores, and the id it stands for.
        assert_eq!(names.label("fable").as_deref(), Some("Fable 5.2"));
        assert_eq!(names.label("opus").as_deref(), Some("Opus 5.5"));
        assert_eq!(
            names.label("claude-fable-5-2").as_deref(),
            Some("Fable 5.2")
        );
        assert_eq!(
            names.label(" Claude-Opus-5-5 ").as_deref(),
            Some("Opus 5.5")
        );
        // The 1M window reads the way `/model` prints it.
        assert_eq!(
            names.label("opus[1m]").as_deref(),
            Some("Opus 5.5 (1M context)")
        );
        assert_eq!(
            names.label("claude-fable-5-2[1M]").as_deref(),
            Some("Fable 5.2 (1M context)")
        );
        // Nothing the catalog does not name is renamed: "pass no flag", an
        // id from elsewhere, an alias whose target has no entry, a model
        // with no name.
        for unnamed in [
            "default",
            "us.anthropic.claude-sonnet-5-v1:0",
            "haiku",
            "sonnet",
            "claude-nameless",
            "",
        ] {
            assert_eq!(names.label(unnamed), None, "{unnamed:?}");
        }
    }

    #[test]
    fn an_alias_follows_the_provider_claude_code_is_pointed_at() {
        let doc = served(1, "cc", ("claude-fable-5-2", "Fable 5.2"));
        let on = |provider| ModelNames::in_catalog(&doc, provider).unwrap();
        assert_eq!(on(None).label("opus").as_deref(), Some("Opus 5.5"));
        assert_eq!(
            on(Some("foundry")).label("opus").as_deref(),
            Some("Opus 4.6")
        );
        // A provider the alias has no entry for takes the default.
        assert_eq!(
            on(Some("vertex")).label("opus").as_deref(),
            Some("Opus 5.5")
        );
        assert_eq!(
            on(Some("foundry")).label("fable").as_deref(),
            Some("Fable 5.2")
        );
        // One whose target the catalog does not name keeps the alias.
        assert_eq!(on(Some("mantle")).label("opus"), None);
    }

    #[test]
    fn the_signed_fallback_document_names_models_too() {
        let published = serde_json::json!({
            "fetchedAt": 5,
            "document": {"surfaces": {
                "ccd": {"model_selector_config": [{"id": "ccd", "models": [
                    {"id": "claude-fable-5-2", "name": "Desktop Fable"},
                ]}]},
                "cc": {"model_selector_config": [{"id": "cc",
                    "models": [{"id": "claude-fable-5-1", "name": "Fable 5.1"}],
                    "provider_alias_targets": {"fable": {"default": "claude-fable-5-1"}},
                }]},
            }},
        });
        let names = ModelNames::in_catalog(&published, None).unwrap();
        assert_eq!(names.label("fable").as_deref(), Some("Fable 5.1"));

        // Another surface's catalog, or a document with no models, is none.
        let desktop = served(1, "ccd", ("claude-fable-5-2", "Fable 5.2"));
        assert_eq!(ModelNames::in_catalog(&desktop, None), None);
        let empty = serde_json::json!({"catalog": {"surface": "cc", "config": {"models": []}}});
        assert_eq!(ModelNames::in_catalog(&empty, None), None);
        assert_eq!(ModelNames::in_catalog(&serde_json::json!({}), None), None);
    }

    #[test]
    fn names_come_from_the_newest_catalog_on_disk() {
        let root = tempfile::tempdir().unwrap();
        let cfg_dir = root.path().join("claude");
        // No cache dir at all: nothing is named.
        assert_eq!(names_from(&cfg_dir, None), ModelNames::default());

        let dir = cfg_dir.join(MODEL_CATALOG_DIR);
        std::fs::create_dir_all(&dir).unwrap();
        let write = |name: &str, doc: &serde_json::Value| {
            std::fs::write(dir.join(name), doc.to_string()).unwrap();
        };
        write(
            "tok-old-cc.json",
            &served(100, "cc", ("claude-fable-5-1", "Fable 5.1")),
        );
        assert_eq!(
            names_from(&cfg_dir, None).label("fable").as_deref(),
            Some("Fable 5.1")
        );

        // A later fetch wins whatever the file is called; the desktop
        // surface, a broken file and a stray non-JSON one are passed over.
        write(
            "org-new-cc.json",
            &served(300, "cc", ("claude-fable-5-2", "Fable 5.2")),
        );
        write(
            "tok-newest-ccd.json",
            &served(900, "ccd", ("claude-fable-9", "Fable 9")),
        );
        std::fs::write(dir.join("masked-ids.json"), r#"{"version":1,"ids":[]}"#).unwrap();
        std::fs::write(dir.join("torn-cc.json"), "{not json").unwrap();
        std::fs::write(
            dir.join("notes.txt"),
            served(999, "cc", ("claude-fable-9", "Fable 9")).to_string(),
        )
        .unwrap();
        let names = names_from(&cfg_dir, None);
        assert_eq!(names.label("fable").as_deref(), Some("Fable 5.2"));
        assert_eq!(names.label("claude-fable-5-1"), None);
        assert_eq!(
            names_from(&cfg_dir, Some("foundry"))
                .label("opus")
                .as_deref(),
            Some("Opus 4.6")
        );
    }
}
