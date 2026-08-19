//! TUI user settings, read from the same `paths::config_path()` JSON the
//! daemon reads (each side deserializes only its own fields; serde ignores
//! the rest). Loaded fresh at each use so edits apply without restarting
//! the TUI. A missing file or unknown fields fall back to defaults; a
//! malformed file is logged and ignored.
//!
//! The settings overlay is the writer: it patches known keys and leaves
//! any other JSON fields (including future daemon keys) untouched.

use serde::Deserialize;
use std::path::{Path, PathBuf};

/// Fallback for `recent_window` when the value is missing or malformed.
pub const DEFAULT_RECENT_WINDOW_MS: i64 = 30 * 60 * 1000;

/// Values the settings overlay cycles through for `recent_window`.
pub const RECENT_WINDOWS: &[&str] = &["off", "5m", "10m", "30m", "1h", "24h"];

/// One row in the settings overlay, in display order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SettingSpec {
    pub kind: SettingKind,
    pub label: &'static str,
    pub hint: &'static str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingKind {
    PaletteEnterAttaches,
    GitInitOnCreate,
    RecentWindow,
    Theme,
}

pub const SETTINGS: &[SettingSpec] = &[
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
    SettingSpec {
        kind: SettingKind::RecentWindow,
        label: "Recent window",
        hint: "How long unpinned sessions stay in the RECENT group",
    },
    SettingSpec {
        kind: SettingKind::Theme,
        label: "Color theme",
        hint: "Accent colors used across the panels and overlays",
    },
];

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
    /// Color theme name (see `theme::THEMES`). Unknown names fall back to
    /// the default theme.
    pub theme: String,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            palette_enter_attaches: true,
            git_init_on_create: true,
            recent_window: "30m".into(),
            theme: "default".into(),
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
        obj.insert("theme".into(), serde_json::json!(self.theme));
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

    pub fn value_label(&self, kind: SettingKind) -> String {
        match kind {
            SettingKind::PaletteEnterAttaches => on_off(self.palette_enter_attaches).into(),
            SettingKind::GitInitOnCreate => on_off(self.git_init_on_create).into(),
            SettingKind::RecentWindow => self.recent_window.clone(),
            SettingKind::Theme => self.theme.clone(),
        }
    }

    /// `delta == 0` means activate (toggle a bool, cycle a choice forward).
    /// Non-zero delta cycles a choice; bools still toggle.
    pub fn cycle(&mut self, index: usize, delta: i32) {
        let Some(spec) = SETTINGS.get(index) else {
            return;
        };
        match spec.kind {
            SettingKind::PaletteEnterAttaches => {
                self.palette_enter_attaches = !self.palette_enter_attaches;
            }
            SettingKind::GitInitOnCreate => {
                self.git_init_on_create = !self.git_init_on_create;
            }
            SettingKind::RecentWindow => {
                let step = if delta == 0 { 1 } else { delta };
                self.recent_window = cycle_choice(&self.recent_window, RECENT_WINDOWS, step).into();
            }
            SettingKind::Theme => {
                let step = if delta == 0 { 1 } else { delta };
                self.theme = cycle_choice(&self.theme, crate::theme::THEMES, step).into();
            }
        }
    }
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
    fn theme_cycles_through_presets_and_resolves() {
        let mut cfg = Config::default();
        assert_eq!(cfg.theme, "default");
        assert_eq!(cfg.theme(), crate::theme::Theme::default());
        let theme_row = SETTINGS
            .iter()
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
