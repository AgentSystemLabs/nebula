//! AGENT PRESETS: saved launch definitions — an AGENT KIND, a MODEL / EFFORT
//! choice, and optional prefix / postfix text — that the SESSIONS PANEL's
//! `e` lists. Launching one asks for a task and hands the CLI
//! `prefix + task + postfix` as its positional starting prompt.
//!
//! A plain JSON list in the DATA DIR beside `config.json`, in list order.
//! A missing or malformed file reads as empty — like the SSH HOSTS FILE it
//! is a convenience store, never load-bearing. Writes go through a temp
//! file + rename so a crash mid-write cannot truncate the list.

use nebula_core::AgentKind;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentPreset {
    /// The row's label; unique (case-insensitively) within the list.
    pub name: String,
    /// The CLI the preset launches.
    #[serde(default)]
    pub kind: AgentKind,
    /// Launch model; None = follow the Settings → Agents default.
    #[serde(default)]
    pub model: Option<String>,
    /// Launch effort; None = follow the Settings → Agents default.
    #[serde(default)]
    pub effort: Option<String>,
    /// Text sent before the task (may be empty).
    #[serde(default)]
    pub prefix: String,
    /// Text sent after the task (may be empty).
    #[serde(default)]
    pub postfix: String,
}

impl AgentPreset {
    /// `claude · opus · high` / `codex · gpt-5.5` / `cursor` — the kind plus
    /// whichever of model and effort the preset pins.
    pub fn spec_label(&self) -> String {
        let mut parts = vec![self.kind.as_str().to_string()];
        parts.extend(self.model.iter().cloned());
        parts.extend(self.effort.iter().cloned());
        parts.join(" · ")
    }

    /// True when the preset wraps the task in any text at all.
    pub fn has_wrapping(&self) -> bool {
        !self.prefix.trim().is_empty() || !self.postfix.trim().is_empty()
    }

    /// The starting prompt: prefix, task and postfix — each trimmed, empty
    /// parts skipped — joined by a blank line.
    pub fn compose(&self, task: &str) -> String {
        [self.prefix.as_str(), task, self.postfix.as_str()]
            .iter()
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join("\n\n")
    }
}

pub fn load() -> Vec<AgentPreset> {
    load_from(&store_path())
}

/// Persist the whole list, in order.
pub fn save(presets: &[AgentPreset]) -> std::io::Result<()> {
    save_to(&store_path(), presets)
}

fn store_path() -> PathBuf {
    #[cfg(test)]
    {
        if let Some(path) = PRESETS_PATH_OVERRIDE.with(|p| p.borrow().clone()) {
            return path;
        }
    }
    nebula_core::paths::data_dir().join("agent_presets.json")
}

fn load_from(path: &Path) -> Vec<AgentPreset> {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_default()
}

fn save_to(store: &Path, presets: &[AgentPreset]) -> std::io::Result<()> {
    if let Some(parent) = store.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut bytes = serde_json::to_vec_pretty(presets)
        .map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidData, err))?;
    bytes.push(b'\n');
    let tmp = store.with_extension("json.tmp");
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, store)
}

#[cfg(test)]
thread_local! {
    static PRESETS_PATH_OVERRIDE: std::cell::RefCell<Option<PathBuf>> =
        const { std::cell::RefCell::new(None) };
}

/// Test hook (the `with_config_path` pattern): route this thread's preset
/// store at `path` for the duration of `f`.
#[cfg(test)]
pub fn with_presets_path<T>(path: PathBuf, f: impl FnOnce() -> T) -> T {
    PRESETS_PATH_OVERRIDE.with(|slot| {
        let prev = slot.replace(Some(path));
        let out = f();
        slot.replace(prev);
        out
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("agent_presets.json");
        (dir, path)
    }

    fn preset(name: &str, kind: AgentKind) -> AgentPreset {
        AgentPreset {
            name: name.into(),
            kind,
            model: None,
            effort: None,
            prefix: String::new(),
            postfix: String::new(),
        }
    }

    #[test]
    fn missing_or_malformed_file_reads_empty() {
        let (_dir, path) = store();
        assert!(load_from(&path).is_empty());
        std::fs::write(&path, "not json").unwrap();
        assert!(load_from(&path).is_empty());
    }

    #[test]
    fn save_round_trips_in_order_and_leaves_no_temp_file() {
        let (_dir, path) = store();
        let presets = vec![
            AgentPreset {
                model: Some("opus".into()),
                effort: Some("high".into()),
                prefix: "Be strict.".into(),
                postfix: "Run the tests.".into(),
                ..preset("reviewer", AgentKind::Claude)
            },
            preset("scratch", AgentKind::Codex),
        ];
        save_to(&path, &presets).unwrap();
        assert_eq!(load_from(&path), presets);
        assert!(!path.with_extension("json.tmp").exists());
        // A rewrite replaces, never appends.
        save_to(&path, &presets[1..]).unwrap();
        assert_eq!(load_from(&path), presets[1..].to_vec());
    }

    #[test]
    fn a_name_only_record_deserializes_with_defaults() {
        let (_dir, path) = store();
        std::fs::write(&path, r#"[{"name": "old"}]"#).unwrap();
        let presets = load_from(&path);
        assert_eq!(presets.len(), 1);
        assert_eq!(presets[0].name, "old");
        assert_eq!(presets[0].kind, AgentKind::Claude);
        assert_eq!(presets[0].model, None);
        assert!(presets[0].prefix.is_empty() && presets[0].postfix.is_empty());
    }

    #[test]
    fn compose_skips_empty_parts_and_trims() {
        let mut p = preset("p", AgentKind::Claude);
        assert_eq!(p.compose("  do it \n"), "do it");
        p.prefix = "PRE\n".into();
        assert_eq!(p.compose("do it"), "PRE\n\ndo it");
        p.postfix = "  POST".into();
        assert_eq!(p.compose("do it"), "PRE\n\ndo it\n\nPOST");
        p.prefix = "   ".into();
        assert_eq!(p.compose("line1\nline2"), "line1\nline2\n\nPOST");
        assert!(p.has_wrapping());
        p.postfix.clear();
        assert!(!p.has_wrapping());
    }

    #[test]
    fn spec_label_names_only_what_is_pinned() {
        assert_eq!(preset("p", AgentKind::Cursor).spec_label(), "cursor");
        let full = AgentPreset {
            model: Some("opus".into()),
            effort: Some("high".into()),
            ..preset("p", AgentKind::Claude)
        };
        assert_eq!(full.spec_label(), "claude · opus · high");
        let model_only = AgentPreset {
            model: Some("gpt-5.5".into()),
            ..preset("p", AgentKind::Codex)
        };
        assert_eq!(model_only.spec_label(), "codex · gpt-5.5");
    }
}
