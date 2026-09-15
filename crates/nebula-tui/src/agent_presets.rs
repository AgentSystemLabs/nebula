//! AGENT PRESETS: saved launch definitions — an AGENT KIND, a MODEL / EFFORT
//! choice, optional prefix / postfix text, and whether to ask for a task at
//! all — that the SESSIONS PANEL's `e` lists. Launching one asks for an
//! optional task (or, with `skip_task`, nothing) and hands the CLI
//! `prefix + task + postfix` as its positional starting prompt.
//!
//! A plain JSON list in the DATA DIR beside `config.json`, in list order.
//! A missing or malformed file reads as empty — like the SSH HOSTS FILE it
//! is a convenience store, never load-bearing. Writes go through a temp
//! file + rename so a crash mid-write cannot truncate the list. The list is
//! read entry by entry, and a save keeps what this build can't read — a
//! preset for a harness a newer nebula added, a field it has no name for —
//! so an older nebula sharing the file never deletes a newer one's presets.

use nebula_core::AgentKind;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentPreset {
    /// The row's label; unique (case-insensitively) within the list.
    pub name: String,
    /// The CLI the preset launches.
    #[serde(default)]
    pub kind: AgentKind,
    /// Registry id when `kind` is [`AgentKind::Custom`].
    #[serde(default)]
    pub custom_harness: Option<String>,
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
    /// Launch without asking for a task: Enter on the preset starts the CLI
    /// on prefix + postfix alone — a "commit and push" preset has nothing
    /// left to say. Off, the task is still asked for, but optional.
    #[serde(default)]
    pub skip_task: bool,
}

impl AgentPreset {
    /// `claude · opus · high` / `codex · gpt-5.5` / `cursor` — the kind plus
    /// whichever of model and effort the preset pins.
    pub fn spec_label(&self) -> String {
        let harness = self
            .custom_harness
            .as_deref()
            .unwrap_or_else(|| self.kind.as_str());
        let mut parts = vec![harness.to_string()];
        parts.extend(self.model.iter().cloned());
        parts.extend(self.effort.iter().cloned());
        parts.join(" · ")
    }

    /// True when the preset wraps the task in any text at all.
    pub fn has_wrapping(&self) -> bool {
        !self.prefix.trim().is_empty() || !self.postfix.trim().is_empty()
    }

    /// The starting prompt: prefix, task and postfix — each trimmed, empty
    /// parts skipped — joined by a blank line. Empty when all three are:
    /// the launch then has no starting prompt at all.
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

pub(crate) fn store_path() -> PathBuf {
    #[cfg(test)]
    {
        if let Some(path) = PRESETS_PATH_OVERRIDE.with(|p| p.borrow().clone()) {
            return path;
        }
    }
    nebula_core::paths::data_dir().join("agent_presets.json")
}

fn load_from(path: &Path) -> Vec<AgentPreset> {
    nebula_core::settings::read_list(path).0
}

/// The `name` a raw preset entry carries, as the file spells it.
pub(crate) fn entry_name(entry: &Value) -> Option<&str> {
    entry.get("name")?.as_str()
}

/// Write `presets` in order. Each keeps the fields its stored entry (same
/// name) had that this build doesn't know, and every stored entry this build
/// can't read goes after them unless a preset now holds its name.
fn save_to(store: &Path, presets: &[AgentPreset]) -> std::io::Result<()> {
    let stored = nebula_core::settings::read_array(store)
        .ok()
        .flatten()
        .unwrap_or_default();
    let named = |name: &str| {
        stored
            .iter()
            .find(|entry| entry_name(entry).is_some_and(|n| n.eq_ignore_ascii_case(name)))
    };
    let mut entries = Vec::with_capacity(presets.len());
    for preset in presets {
        let Value::Object(known) = serde_json::to_value(preset)
            .map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidData, err))?
        else {
            unreachable!("a preset serializes to a JSON object");
        };
        let mut entry = named(&preset.name)
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();
        entry.extend(known);
        entries.push(Value::Object(entry));
    }
    for entry in &stored {
        let unreadable = serde_json::from_value::<AgentPreset>(entry.clone()).is_err();
        let taken = entry_name(entry)
            .is_some_and(|n| presets.iter().any(|p| p.name.eq_ignore_ascii_case(n)));
        if unreadable && !taken {
            entries.push(entry.clone());
        }
    }
    nebula_core::settings::write_json(store, &Value::Array(entries))
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
            custom_harness: None,
            model: None,
            effort: None,
            prefix: String::new(),
            postfix: String::new(),
            skip_task: false,
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
            AgentPreset {
                skip_task: true,
                ..preset("scratch", AgentKind::Codex)
            },
        ];
        save_to(&path, &presets).unwrap();
        assert_eq!(load_from(&path), presets);
        assert!(!path.with_extension("json.tmp").exists());
        // A rewrite replaces, never appends.
        save_to(&path, &presets[1..]).unwrap();
        assert_eq!(load_from(&path), presets[1..].to_vec());
    }

    /// A preset for a harness this build has never heard of — a newer
    /// nebula's — survives a save, and so does a field this build doesn't know.
    #[test]
    fn a_save_keeps_what_a_newer_nebula_wrote() {
        let (_dir, path) = store();
        std::fs::write(
            &path,
            r#"[
                {"name": "reviewer", "kind": "codex", "pinned": true},
                {"name": "future", "kind": "antigravity", "model": "x"}
            ]"#,
        )
        .unwrap();
        let mut presets = load_from(&path);
        assert_eq!(presets.len(), 1, "only the readable preset lists");
        presets[0].prefix = "Be strict.".into();
        presets.push(preset("scratch", AgentKind::Claude));
        save_to(&path, &presets).unwrap();

        let saved: Vec<Value> =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        let names: Vec<&str> = saved.iter().filter_map(entry_name).collect();
        assert_eq!(names, ["reviewer", "scratch", "future"]);
        assert_eq!(saved[0]["pinned"], true);
        assert_eq!(saved[0]["prefix"], "Be strict.");
        assert_eq!(saved[2]["kind"], "antigravity");
        assert_eq!(load_from(&path).len(), 2);
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
        assert!(!presets[0].skip_task, "an old record still asks for a task");
    }

    #[test]
    fn compose_skips_empty_parts_and_trims() {
        let mut p = preset("p", AgentKind::Claude);
        assert_eq!(p.compose(""), "", "nothing at all composes to nothing");
        assert_eq!(p.compose("  do it \n"), "do it");
        p.prefix = "PRE\n".into();
        assert_eq!(p.compose("do it"), "PRE\n\ndo it");
        p.postfix = "  POST".into();
        assert_eq!(p.compose("do it"), "PRE\n\ndo it\n\nPOST");
        assert_eq!(p.compose(" \n"), "PRE\n\nPOST", "the task is optional");
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
