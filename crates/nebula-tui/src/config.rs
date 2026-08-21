//! TUI user settings, read from the same `paths::config_path()` JSON the
//! daemon reads (each side deserializes only its own fields; serde ignores
//! the rest). Loaded fresh at each use so edits apply without restarting
//! the TUI. A missing file or unknown fields fall back to defaults; a
//! malformed file is logged and ignored.
//!
//! The settings overlay is the writer: it patches known keys and leaves
//! any other JSON fields (including future daemon keys) untouched.

use nebula_core::AgentKind;
use serde::Deserialize;
use std::path::{Path, PathBuf};

/// Fallback for `recent_window` when the value is missing or malformed.
pub const DEFAULT_RECENT_WINDOW_MS: i64 = 30 * 60 * 1000;

/// Values the settings overlay cycles through for `recent_window`.
pub const RECENT_WINDOWS: &[&str] = &["off", "5m", "10m", "30m", "1h", "24h"];

/// Values the settings overlay cycles through for `session_idle_timeout`
/// (daemon-owned: how long unwatched idle sessions live before their PTY
/// is reaped).
pub const SESSION_IDLE_TIMEOUTS: &[&str] = &["off", "1m", "5m", "15m", "30m", "1h"];

/// Model/effort choices for the new-session submenus and the settings
/// overlay. "default" everywhere means "don't pass the flag — let the CLI
/// pick" and is what the daemon sees as None.
pub const CLAUDE_MODELS: &[&str] = &["default", "fable", "opus", "sonnet", "haiku"];
pub const CLAUDE_EFFORTS: &[&str] = &["default", "low", "medium", "high", "xhigh", "max"];
pub const CODEX_MODELS: &[&str] = &["default", "gpt-5.6-sol", "gpt-5.5"];
pub const CODEX_EFFORTS: &[&str] = &["default", "minimal", "low", "medium", "high", "xhigh"];

/// Model choices for a session kind; empty = no model submenu (Cursor).
pub fn model_choices(kind: AgentKind) -> &'static [&'static str] {
    match kind {
        AgentKind::Claude => CLAUDE_MODELS,
        AgentKind::Codex => CODEX_MODELS,
        AgentKind::Cursor => &[],
    }
}

/// Effort choices for a session kind; empty = no effort submenu (Cursor).
pub fn effort_choices(kind: AgentKind) -> &'static [&'static str] {
    match kind {
        AgentKind::Claude => CLAUDE_EFFORTS,
        AgentKind::Codex => CODEX_EFFORTS,
        AgentKind::Cursor => &[],
    }
}

/// One setting row in the overlay; rows live inside a [`SettingGroup`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SettingSpec {
    pub kind: SettingKind,
    pub label: &'static str,
    pub hint: &'static str,
}

/// A titled section of the settings overlay, Help-menu style. Flat
/// setting indices (selection, `Config::cycle`) run through the groups
/// in declaration order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SettingGroup {
    pub title: &'static str,
    pub settings: &'static [SettingSpec],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingKind {
    PaletteEnterAttaches,
    GitInitOnCreate,
    RecentWindow,
    SessionIdleTimeout,
    Theme,
    Animations,
    FocusTint,
    ClaudeModel,
    ClaudeEffort,
    CodexModel,
    CodexEffort,
}

pub const SETTING_GROUPS: &[SettingGroup] = &[
    SettingGroup {
        title: "GENERAL",
        settings: &[
            SettingSpec {
                kind: SettingKind::PaletteEnterAttaches,
                label: "Search Enter attaches",
                hint: "Enter in / search opens the session in the terminal",
            },
            SettingSpec {
                kind: SettingKind::GitInitOnCreate,
                label: "git init new projects",
                hint: "When adding a missing directory, run git init in it",
            },
        ],
    },
    SettingGroup {
        title: "SESSIONS",
        settings: &[
            SettingSpec {
                kind: SettingKind::RecentWindow,
                label: "Recent window",
                hint: "How long unpinned sessions stay in the RECENT group",
            },
            SettingSpec {
                kind: SettingKind::SessionIdleTimeout,
                label: "Idle session timeout",
                hint: "Kill idle sessions in unviewed worktrees (pinned/busy spared; off disables)",
            },
        ],
    },
    SettingGroup {
        title: "APPEARANCE",
        settings: &[
            SettingSpec {
                kind: SettingKind::Theme,
                label: "Color theme",
                hint: "Accent colors used across the panels and overlays",
            },
            SettingSpec {
                kind: SettingKind::Animations,
                label: "Animations",
                hint: "Status text sweep and splash motion (off = fewer repaints)",
            },
            SettingSpec {
                kind: SettingKind::FocusTint,
                label: "Focused panel tint",
                hint: "Faint accent-colored background on the focused panel",
            },
        ],
    },
    SettingGroup {
        title: "AGENT DEFAULTS",
        settings: &[
            SettingSpec {
                kind: SettingKind::ClaudeModel,
                label: "Claude model",
                hint: "Default model for new Claude sessions (default = CLI's pick)",
            },
            SettingSpec {
                kind: SettingKind::ClaudeEffort,
                label: "Claude effort",
                hint: "Default reasoning effort for new Claude sessions",
            },
            SettingSpec {
                kind: SettingKind::CodexModel,
                label: "Codex model",
                hint: "Default model for new Codex sessions (default = CLI's pick)",
            },
            SettingSpec {
                kind: SettingKind::CodexEffort,
                label: "Codex effort",
                hint: "Default reasoning effort for new Codex sessions",
            },
        ],
    },
];

/// All settings in flat display order (the order selection indices use).
pub fn settings() -> impl Iterator<Item = &'static SettingSpec> {
    SETTING_GROUPS.iter().flat_map(|g| g.settings.iter())
}

pub fn settings_len() -> usize {
    settings().count()
}

/// The setting at a flat display index, if any.
pub fn setting_at(index: usize) -> Option<&'static SettingSpec> {
    settings().nth(index)
}

/// One terminal row of the settings overlay body, in display order.
/// Shared by the renderer and mouse hit-testing so they can't drift.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingsRow {
    Blank,
    Header(&'static str),
    /// Label + value line for the setting at this flat index.
    Setting(usize),
}

pub fn settings_rows() -> Vec<SettingsRow> {
    let mut rows = Vec::new();
    let mut index = 0;
    for (gi, group) in SETTING_GROUPS.iter().enumerate() {
        if gi > 0 {
            rows.push(SettingsRow::Blank);
        }
        rows.push(SettingsRow::Header(group.title));
        for _ in group.settings {
            rows.push(SettingsRow::Setting(index));
            index += 1;
        }
    }
    rows
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct Config {
    /// `/` palette: Enter on a session attaches and focuses the terminal.
    /// When false, Enter only lands on the session's row in the Sessions
    /// panel (previewing it in the pane). Ctrl+O / Ctrl+F always pick
    /// open / focus explicitly, regardless of this setting.
    pub palette_enter_attaches: bool,
    /// Run `git init` after AddProject creates a missing directory.
    /// Owned by the daemon; the TUI writes it so the settings overlay can
    /// toggle every key in the shared file.
    pub git_init_on_create: bool,
    /// How long a session stays in the RECENT group after its status last
    /// changed: "5m", "10m", "30m", "1h", "24h" (any `<n>m`/`<n>h` works).
    /// "off" disables the group. Malformed values fall back to 30m.
    pub recent_window: String,
    /// How long an idle session in an unviewed worktree lives before the
    /// daemon reaps its PTY: "1m", "5m", "15m", "30m", "1h"; "off"
    /// disables. Owned by the daemon (which does the parsing and reaping);
    /// the TUI writes it so the settings overlay can cycle it.
    pub session_idle_timeout: String,
    /// Color theme name (see `theme::THEMES`). Unknown names fall back to
    /// the default theme.
    pub theme: String,
    /// Master switch for the TUI's animations (the running/needs-feedback
    /// status-text sweep and the splash's motion). Off trades them for
    /// fewer repaints on constrained machines.
    pub animations: bool,
    /// Faint accent-tinted background fill on the focused panel. Off by
    /// default — it's a taste call, not everyone wants the extra color.
    pub focus_tint: bool,
    /// Default model/effort for new Claude / Codex sessions. "default"
    /// means "don't pass the flag" (the CLI picks); any other value is
    /// passed through verbatim, so hand-edited configs can name models the
    /// pickers don't list.
    pub claude_model: String,
    pub claude_effort: String,
    pub codex_model: String,
    pub codex_effort: String,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            palette_enter_attaches: true,
            git_init_on_create: true,
            recent_window: "30m".into(),
            session_idle_timeout: "5m".into(),
            theme: "default".into(),
            animations: true,
            focus_tint: false,
            claude_model: "default".into(),
            claude_effort: "default".into(),
            codex_model: "default".into(),
            codex_effort: "default".into(),
        }
    }
}

impl Config {
    pub fn load() -> Self {
        load_from(&settings_path())
    }

    /// Patch this config's known keys into the JSON file, preserving any
    /// other fields already there.
    pub fn save(&self) -> std::io::Result<()> {
        self.save_to(&settings_path())
    }

    pub fn save_to(&self, path: &Path) -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut root = match std::fs::read_to_string(path) {
            Ok(raw) => serde_json::from_str::<serde_json::Value>(&raw)
                .ok()
                .filter(|v| v.is_object())
                .unwrap_or_else(|| serde_json::json!({})),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => serde_json::json!({}),
            Err(err) => return Err(err),
        };
        let obj = root
            .as_object_mut()
            .expect("root filtered to object or empty object");
        obj.insert(
            "palette_enter_attaches".into(),
            serde_json::json!(self.palette_enter_attaches),
        );
        obj.insert(
            "git_init_on_create".into(),
            serde_json::json!(self.git_init_on_create),
        );
        obj.insert(
            "recent_window".into(),
            serde_json::json!(self.recent_window),
        );
        obj.insert(
            "session_idle_timeout".into(),
            serde_json::json!(self.session_idle_timeout),
        );
        obj.insert("theme".into(), serde_json::json!(self.theme));
        obj.insert("animations".into(), serde_json::json!(self.animations));
        obj.insert("focus_tint".into(), serde_json::json!(self.focus_tint));
        obj.insert("claude_model".into(), serde_json::json!(self.claude_model));
        obj.insert(
            "claude_effort".into(),
            serde_json::json!(self.claude_effort),
        );
        obj.insert("codex_model".into(), serde_json::json!(self.codex_model));
        obj.insert("codex_effort".into(), serde_json::json!(self.codex_effort));
        let mut bytes = serde_json::to_vec_pretty(&root)
            .map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidData, err))?;
        if !bytes.ends_with(b"\n") {
            bytes.push(b'\n');
        }
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, &bytes)?;
        std::fs::rename(&tmp, path)?;
        Ok(())
    }

    /// `recent_window` parsed to ms; 0 disables the RECENT group.
    pub fn recent_window_ms(&self) -> i64 {
        parse_window_ms(&self.recent_window).unwrap_or(DEFAULT_RECENT_WINDOW_MS)
    }

    /// `theme` resolved to the palette the UI draws with.
    pub fn theme(&self) -> crate::theme::Theme {
        crate::theme::Theme::by_name(&self.theme)
    }

    /// The configured default model for new sessions of `kind`, as the
    /// daemon wants it: None = "default" = don't pass the flag.
    pub fn default_model(&self, kind: AgentKind) -> Option<String> {
        let value = match kind {
            AgentKind::Claude => &self.claude_model,
            AgentKind::Codex => &self.codex_model,
            AgentKind::Cursor => return None,
        };
        non_default(value)
    }

    /// The configured default effort for new sessions of `kind`;
    /// None = "default" = don't pass the flag.
    pub fn default_effort(&self, kind: AgentKind) -> Option<String> {
        let value = match kind {
            AgentKind::Claude => &self.claude_effort,
            AgentKind::Codex => &self.codex_effort,
            AgentKind::Cursor => return None,
        };
        non_default(value)
    }

    pub fn value_label(&self, kind: SettingKind) -> String {
        match kind {
            SettingKind::PaletteEnterAttaches => on_off(self.palette_enter_attaches).into(),
            SettingKind::GitInitOnCreate => on_off(self.git_init_on_create).into(),
            SettingKind::RecentWindow => self.recent_window.clone(),
            SettingKind::SessionIdleTimeout => self.session_idle_timeout.clone(),
            SettingKind::Theme => self.theme.clone(),
            SettingKind::Animations => on_off(self.animations).into(),
            SettingKind::FocusTint => on_off(self.focus_tint).into(),
            SettingKind::ClaudeModel => self.claude_model.clone(),
            SettingKind::ClaudeEffort => self.claude_effort.clone(),
            SettingKind::CodexModel => self.codex_model.clone(),
            SettingKind::CodexEffort => self.codex_effort.clone(),
        }
    }

    /// `delta == 0` means activate (toggle a bool, cycle a choice forward).
    /// Non-zero delta cycles a choice; bools still toggle.
    pub fn cycle(&mut self, index: usize, delta: i32) {
        let Some(spec) = setting_at(index) else {
            return;
        };
        let step = if delta == 0 { 1 } else { delta };
        match spec.kind {
            SettingKind::PaletteEnterAttaches => {
                self.palette_enter_attaches = !self.palette_enter_attaches;
            }
            SettingKind::GitInitOnCreate => {
                self.git_init_on_create = !self.git_init_on_create;
            }
            SettingKind::RecentWindow => {
                self.recent_window = cycle_choice(&self.recent_window, RECENT_WINDOWS, step).into();
            }
            SettingKind::SessionIdleTimeout => {
                self.session_idle_timeout =
                    cycle_choice(&self.session_idle_timeout, SESSION_IDLE_TIMEOUTS, step).into();
            }
            SettingKind::Theme => {
                self.theme = cycle_choice(&self.theme, crate::theme::THEMES, step).into();
            }
            SettingKind::Animations => {
                self.animations = !self.animations;
            }
            SettingKind::FocusTint => {
                self.focus_tint = !self.focus_tint;
            }
            SettingKind::ClaudeModel => {
                self.claude_model = cycle_choice(&self.claude_model, CLAUDE_MODELS, step).into();
            }
            SettingKind::ClaudeEffort => {
                self.claude_effort = cycle_choice(&self.claude_effort, CLAUDE_EFFORTS, step).into();
            }
            SettingKind::CodexModel => {
                self.codex_model = cycle_choice(&self.codex_model, CODEX_MODELS, step).into();
            }
            SettingKind::CodexEffort => {
                self.codex_effort = cycle_choice(&self.codex_effort, CODEX_EFFORTS, step).into();
            }
        }
    }
}

/// "default" (or blank) → None; anything else passes through.
fn non_default(value: &str) -> Option<String> {
    let value = value.trim();
    (!value.is_empty() && !value.eq_ignore_ascii_case("default")).then(|| value.to_string())
}

fn on_off(v: bool) -> &'static str {
    if v {
        "on"
    } else {
        "off"
    }
}

fn cycle_choice<'a>(current: &str, choices: &[&'a str], delta: i32) -> &'a str {
    let n = choices.len() as i32;
    let pos = choices
        .iter()
        .position(|c| c.eq_ignore_ascii_case(current.trim()))
        .unwrap_or(0) as i32;
    choices[(pos + delta).rem_euclid(n) as usize]
}

fn load_from(path: &Path) -> Config {
    let Ok(raw) = std::fs::read_to_string(path) else {
        return Config::default();
    };
    serde_json::from_str(&raw).unwrap_or_else(|err| {
        tracing::warn!("ignoring malformed {}: {err}", path.display());
        Config::default()
    })
}

fn settings_path() -> PathBuf {
    #[cfg(test)]
    {
        if let Some(path) = CONFIG_PATH_OVERRIDE.with(|p| p.borrow().clone()) {
            return path;
        }
    }
    nebula_core::paths::config_path()
}

fn parse_window_ms(s: &str) -> Option<i64> {
    let s = s.trim();
    if s.eq_ignore_ascii_case("off") || s == "0" {
        return Some(0);
    }
    let (digits, unit_ms) = match s.strip_suffix(['m', 'M']) {
        Some(d) => (d, 60_000),
        None => (s.strip_suffix(['h', 'H'])?, 3_600_000),
    };
    let n: i64 = digits.trim().parse().ok()?;
    (n >= 0).then(|| n.saturating_mul(unit_ms))
}

#[cfg(test)]
thread_local! {
    static CONFIG_PATH_OVERRIDE: std::cell::RefCell<Option<PathBuf>> =
        const { std::cell::RefCell::new(None) };
}

#[cfg(test)]
pub fn with_config_path<T>(path: PathBuf, f: impl FnOnce() -> T) -> T {
    CONFIG_PATH_OVERRIDE.with(|slot| {
        let prev = slot.replace(Some(path));
        let out = f();
        slot.replace(prev);
        out
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_enter_attaches() {
        assert!(Config::default().palette_enter_attaches);
        let cfg: Config = serde_json::from_str("{}").unwrap();
        assert!(cfg.palette_enter_attaches);
        let cfg: Config = serde_json::from_str(r#"{"palette_enter_attaches": false}"#).unwrap();
        assert!(!cfg.palette_enter_attaches);
    }

    #[test]
    fn daemon_fields_are_ignored() {
        let cfg: Config = serde_json::from_str(r#"{"git_init_on_create": false}"#).unwrap();
        assert!(cfg.palette_enter_attaches);
        assert!(!cfg.git_init_on_create);
    }

    #[test]
    fn recent_window_parses_minutes_hours_and_off() {
        let ms = |v: &str| {
            let cfg: Config =
                serde_json::from_str(&format!(r#"{{"recent_window": "{v}"}}"#)).unwrap();
            cfg.recent_window_ms()
        };
        assert_eq!(ms("5m"), 5 * 60_000);
        assert_eq!(ms("10m"), 10 * 60_000);
        assert_eq!(ms("30m"), 30 * 60_000);
        assert_eq!(ms("1h"), 3_600_000);
        assert_eq!(ms("24h"), 24 * 3_600_000);
        assert_eq!(ms("off"), 0);
        assert_eq!(ms("0"), 0);
        // Malformed values fall back to the default.
        assert_eq!(ms("soon"), DEFAULT_RECENT_WINDOW_MS);
        assert_eq!(ms("-5m"), DEFAULT_RECENT_WINDOW_MS);
        assert_eq!(
            Config::default().recent_window_ms(),
            DEFAULT_RECENT_WINDOW_MS
        );
    }

    #[test]
    fn cycle_toggles_bools_and_walks_recent_window() {
        let mut cfg = Config::default();
        assert!(cfg.palette_enter_attaches);
        cfg.cycle(0, 0);
        assert!(!cfg.palette_enter_attaches);
        cfg.cycle(0, 1);
        assert!(cfg.palette_enter_attaches);

        assert_eq!(cfg.recent_window, "30m");
        cfg.cycle(2, 0);
        assert_eq!(cfg.recent_window, "1h");
        cfg.cycle(2, -1);
        assert_eq!(cfg.recent_window, "30m");
        cfg.cycle(2, -1);
        assert_eq!(cfg.recent_window, "10m");
    }

    #[test]
    fn session_idle_timeout_cycles_and_persists() {
        let mut cfg = Config::default();
        assert_eq!(cfg.session_idle_timeout, "5m");
        let row = settings()
            .position(|s| s.kind == SettingKind::SessionIdleTimeout)
            .unwrap();
        cfg.cycle(row, 1);
        assert_eq!(cfg.session_idle_timeout, "15m");
        cfg.cycle(row, -2);
        assert_eq!(cfg.session_idle_timeout, "1m");
        cfg.cycle(row, -1);
        assert_eq!(cfg.session_idle_timeout, "off");

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        cfg.save_to(&path).unwrap();
        assert_eq!(load_from(&path).session_idle_timeout, "off");
    }

    #[test]
    fn theme_cycles_through_presets_and_resolves() {
        let mut cfg = Config::default();
        assert_eq!(cfg.theme, "default");
        assert_eq!(cfg.theme(), crate::theme::Theme::default());
        let theme_row = settings()
            .position(|s| s.kind == SettingKind::Theme)
            .unwrap();
        cfg.cycle(theme_row, 1);
        assert_eq!(cfg.theme, "ocean");
        assert_ne!(cfg.theme(), crate::theme::Theme::default());
        cfg.cycle(theme_row, -1);
        assert_eq!(cfg.theme, "default");
        // Unknown names (hand-edited config) cycle from the start and
        // resolve to the default palette rather than erroring.
        cfg.theme = "sparkle".into();
        assert_eq!(cfg.theme(), crate::theme::Theme::default());
    }

    #[test]
    fn animations_default_on_toggle_and_persist() {
        let mut cfg = Config::default();
        assert!(cfg.animations);
        let row = settings()
            .position(|s| s.kind == SettingKind::Animations)
            .unwrap();
        cfg.cycle(row, 0);
        assert!(!cfg.animations);

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        cfg.save_to(&path).unwrap();
        assert!(!load_from(&path).animations);
        // A config predating the key keeps animations on.
        let cfg: Config = serde_json::from_str("{}").unwrap();
        assert!(cfg.animations);
    }

    #[test]
    fn focus_tint_default_off_toggle_and_persist() {
        let mut cfg = Config::default();
        assert!(!cfg.focus_tint);
        let row = settings()
            .position(|s| s.kind == SettingKind::FocusTint)
            .unwrap();
        cfg.cycle(row, 0);
        assert!(cfg.focus_tint);

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        cfg.save_to(&path).unwrap();
        assert!(load_from(&path).focus_tint);
        // A config predating the key keeps the tint off.
        let cfg: Config = serde_json::from_str("{}").unwrap();
        assert!(!cfg.focus_tint);
    }

    #[test]
    fn model_effort_defaults_resolve_and_cycle() {
        let mut cfg = Config::default();
        // "default" everywhere → no flags for any kind.
        assert_eq!(cfg.default_model(AgentKind::Claude), None);
        assert_eq!(cfg.default_effort(AgentKind::Claude), None);
        assert_eq!(cfg.default_model(AgentKind::Codex), None);
        assert_eq!(cfg.default_effort(AgentKind::Codex), None);

        cfg.claude_model = "opus".into();
        cfg.codex_effort = "high".into();
        assert_eq!(
            cfg.default_model(AgentKind::Claude).as_deref(),
            Some("opus")
        );
        assert_eq!(cfg.default_effort(AgentKind::Claude), None);
        assert_eq!(cfg.default_model(AgentKind::Codex), None);
        assert_eq!(
            cfg.default_effort(AgentKind::Codex).as_deref(),
            Some("high")
        );
        // Cursor has no model/effort knobs regardless of settings.
        assert_eq!(cfg.default_model(AgentKind::Cursor), None);
        assert_eq!(cfg.default_effort(AgentKind::Cursor), None);

        // The settings rows walk the same choice lists the submenus show.
        let row = settings()
            .position(|s| s.kind == SettingKind::ClaudeModel)
            .unwrap();
        cfg.claude_model = "default".into();
        cfg.cycle(row, 1);
        assert_eq!(cfg.claude_model, "fable");
        cfg.cycle(row, -1);
        assert_eq!(cfg.claude_model, "default");
        let row = settings()
            .position(|s| s.kind == SettingKind::CodexEffort)
            .unwrap();
        cfg.cycle(row, 0);
        assert_eq!(
            cfg.codex_effort, "xhigh",
            "activate steps forward from high"
        );
    }

    #[test]
    fn save_persists_model_effort_keys() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        let mut cfg = Config::default();
        cfg.claude_model = "sonnet".into();
        cfg.codex_effort = "xhigh".into();
        cfg.save_to(&path).unwrap();
        let reread = load_from(&path);
        assert_eq!(reread.claude_model, "sonnet");
        assert_eq!(reread.claude_effort, "default");
        assert_eq!(reread.codex_model, "default");
        assert_eq!(reread.codex_effort, "xhigh");
    }

    #[test]
    fn save_patches_known_keys_and_keeps_unknown_fields() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        std::fs::write(
            &path,
            r#"{
  "git_init_on_create": false,
  "future_daemon_flag": true,
  "recent_window": "5m"
}
"#,
        )
        .unwrap();

        let mut cfg = load_from(&path);
        assert!(!cfg.git_init_on_create);
        assert_eq!(cfg.recent_window, "5m");
        cfg.palette_enter_attaches = false;
        cfg.git_init_on_create = true;
        cfg.recent_window = "1h".into();
        cfg.save_to(&path).unwrap();

        let saved: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(saved["palette_enter_attaches"], false);
        assert_eq!(saved["git_init_on_create"], true);
        assert_eq!(saved["recent_window"], "1h");
        assert_eq!(saved["future_daemon_flag"], true);
    }

    #[test]
    fn groups_cover_every_setting_once_and_rows_match() {
        // Every SettingKind appears exactly once across the groups.
        let mut kinds: Vec<SettingKind> = settings().map(|s| s.kind).collect();
        assert_eq!(kinds.len(), settings_len());
        kinds.dedup();
        assert_eq!(kinds.len(), settings_len(), "a kind repeats across groups");

        // The overlay rows walk the same flat order: one Setting(i) per
        // index, in order, with a header starting each group.
        let indices: Vec<usize> = settings_rows()
            .into_iter()
            .filter_map(|row| match row {
                SettingsRow::Setting(i) => Some(i),
                _ => None,
            })
            .collect();
        assert_eq!(indices, (0..settings_len()).collect::<Vec<_>>());
        let headers = settings_rows()
            .into_iter()
            .filter(|row| matches!(row, SettingsRow::Header(_)))
            .count();
        assert_eq!(headers, SETTING_GROUPS.len());
    }

    #[test]
    fn save_creates_file_when_missing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("config.json");
        Config::default().save_to(&path).unwrap();
        let saved: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(saved["palette_enter_attaches"], true);
        assert_eq!(saved["git_init_on_create"], true);
        assert_eq!(saved["recent_window"], "30m");
    }
}
