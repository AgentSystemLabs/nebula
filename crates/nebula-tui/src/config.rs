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
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Fallback for `recent_window` when the value is missing or malformed.
pub const DEFAULT_RECENT_WINDOW_MS: i64 = 30 * 60 * 1000;

/// Values the settings overlay cycles through for `recent_window`.
pub const RECENT_WINDOWS: &[&str] = &["off", "5m", "10m", "30m", "1h", "24h"];

/// Values the settings overlay cycles through for `session_idle_timeout`
/// (daemon-owned: how long unwatched idle sessions live before their PTY
/// is reaped).
pub const SESSION_IDLE_TIMEOUTS: &[&str] = &["off", "1m", "5m", "15m", "30m", "1h"];

/// Editor commands the settings overlay cycles through. Every entry
/// accepts `+<line> <file>`, which is how the overlays launch it. As with
/// models, hand-edited configs can name any command the list doesn't.
pub const EDITORS: &[&str] = &["vim", "nvim", "nano", "emacs", "hx"];

/// Model/effort choices for the new-session submenus and the settings
/// overlay. "default" everywhere means "don't pass the flag — let the CLI
/// pick" and is what the daemon sees as None.
pub const CLAUDE_MODELS: &[&str] = &["default", "fable", "opus", "sonnet", "haiku"];
pub const CLAUDE_EFFORTS: &[&str] = &["default", "low", "medium", "high", "xhigh", "max"];
pub const CODEX_MODELS: &[&str] = &[
    "default",
    "gpt-5.6-sol",
    "gpt-5.6-terra",
    "gpt-5.6-luna",
    "gpt-5.5",
];
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

/// One setting row in the overlay; rows live inside a [`SettingsTab`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SettingSpec {
    pub kind: SettingKind,
    pub label: &'static str,
    pub hint: &'static str,
}

/// What a tab shows. Ordinary tabs are a list of value settings; the
/// Hotkeys tab is generated from [`crate::keymap::ACTIONS`] instead, so a
/// new action shows up there without being declared twice.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TabBody {
    Values(&'static [SettingSpec]),
    Hotkeys,
}

/// One tab of the settings overlay. Selection indices are per-tab: within
/// a `Values` tab they index its settings, within `Hotkeys` they index
/// `keymap::ACTIONS`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SettingsTab {
    pub title: &'static str,
    pub body: TabBody,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingKind {
    PaletteEnterAttaches,
    GitInitOnCreate,
    Editor,
    SkipSessionNaming,
    RecentWindow,
    SessionIdleTimeout,
    Theme,
    Animations,
    FocusTint,
    ShowWorkspaces,
    ClaudeModel,
    ClaudeEffort,
    CodexModel,
    CodexEffort,
}

/// The tab strip, left to right. Ordered by how often a setting gets
/// touched, with Hotkeys last because it is the biggest and the least
/// casual.
pub const SETTINGS_TABS: &[SettingsTab] = &[
    SettingsTab {
        title: "General",
        body: TabBody::Values(&[
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
                kind: SettingKind::Editor,
                label: "File editor",
                hint: "Editor f/b/F and ⌥click launch (NEBULA_EDITOR overrides)",
            },
        ]),
    },
    SettingsTab {
        title: "Sessions",
        body: TabBody::Values(&[
            SettingSpec {
                kind: SettingKind::SkipSessionNaming,
                label: "Skip session naming",
                hint: "New agents skip the name prompt and take the auto-title the agent sets",
            },
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
        ]),
    },
    SettingsTab {
        title: "Appearance",
        body: TabBody::Values(&[
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
            SettingSpec {
                kind: SettingKind::ShowWorkspaces,
                label: "Workspaces column",
                hint: "Show the Workspaces column at the left edge (Shift+W toggles)",
            },
        ]),
    },
    SettingsTab {
        title: "Agents",
        body: TabBody::Values(&[
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
        ]),
    },
    SettingsTab {
        title: "Hotkeys",
        body: TabBody::Hotkeys,
    },
];

/// Index of the Hotkeys tab, which the overlay special-cases.
pub fn hotkeys_tab() -> usize {
    SETTINGS_TABS
        .iter()
        .position(|t| t.body == TabBody::Hotkeys)
        .expect("SETTINGS_TABS declares a Hotkeys tab")
}

pub fn tab_count() -> usize {
    SETTINGS_TABS.len()
}

/// The value settings of a tab; empty for the Hotkeys tab.
pub fn tab_settings(tab: usize) -> &'static [SettingSpec] {
    match SETTINGS_TABS.get(tab).map(|t| t.body) {
        Some(TabBody::Values(settings)) => settings,
        _ => &[],
    }
}

/// How many selectable rows a tab holds.
pub fn tab_len(tab: usize) -> usize {
    match SETTINGS_TABS.get(tab).map(|t| t.body) {
        Some(TabBody::Values(settings)) => settings.len(),
        Some(TabBody::Hotkeys) => crate::keymap::ACTIONS.len(),
        None => 0,
    }
}

/// The value setting at a tab-local index, if the tab has one there.
pub fn setting_at(tab: usize, index: usize) -> Option<&'static SettingSpec> {
    tab_settings(tab).get(index)
}

/// Where a setting lives, as `(tab, row)`. The overlay addresses settings
/// by position, so anything that wants to talk about one by name — tests,
/// and anything that ever jumps the cursor to a named setting — goes
/// through here rather than hardcoding an index.
pub fn locate(kind: SettingKind) -> Option<(usize, usize)> {
    SETTINGS_TABS.iter().enumerate().find_map(|(t, tab)| {
        match tab.body {
            TabBody::Values(settings) => settings.iter().position(|s| s.kind == kind),
            TabBody::Hotkeys => None,
        }
        .map(|i| (t, i))
    })
}

/// Every value setting, tab by tab, for coverage checks.
pub fn all_settings() -> impl Iterator<Item = (usize, usize, &'static SettingSpec)> {
    SETTINGS_TABS.iter().enumerate().flat_map(|(t, tab)| {
        tab_settings(t).iter().enumerate().map(move |(i, s)| {
            let _ = tab;
            (t, i, s)
        })
    })
}

/// The one-line hint under the selected row, whatever kind of row it is.
pub fn hint_at(tab: usize, index: usize) -> &'static str {
    match SETTINGS_TABS.get(tab).map(|t| t.body) {
        Some(TabBody::Values(settings)) => settings.get(index).map(|s| s.hint).unwrap_or(""),
        Some(TabBody::Hotkeys) => crate::keymap::spec_at(index).map(|s| s.hint).unwrap_or(""),
        None => "",
    }
}

/// One terminal row of the settings overlay body, in display order.
/// Shared by the renderer and mouse hit-testing so they can't drift.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingsRow {
    Blank,
    Header(&'static str),
    /// Label + value line for the value setting at this tab-local index.
    Setting(usize),
    /// Label + chord list for `keymap::ACTIONS[index]`.
    Hotkey(usize),
}

impl SettingsRow {
    /// The tab-local selection index this row stands for, if it's one the
    /// cursor can land on.
    pub fn index(self) -> Option<usize> {
        match self {
            SettingsRow::Setting(i) | SettingsRow::Hotkey(i) => Some(i),
            _ => None,
        }
    }
}

pub fn settings_rows(tab: usize) -> Vec<SettingsRow> {
    match SETTINGS_TABS.get(tab).map(|t| t.body) {
        Some(TabBody::Values(settings)) => (0..settings.len()).map(SettingsRow::Setting).collect(),
        Some(TabBody::Hotkeys) => {
            // The action table is already grouped; emit a header whenever
            // the group name changes.
            let mut rows = Vec::new();
            let mut group: Option<&'static str> = None;
            for (i, spec) in crate::keymap::ACTIONS.iter().enumerate() {
                if group != Some(spec.group) {
                    if group.is_some() {
                        rows.push(SettingsRow::Blank);
                    }
                    rows.push(SettingsRow::Header(spec.group));
                    group = Some(spec.group);
                }
                rows.push(SettingsRow::Hotkey(i));
            }
            rows
        }
        None => Vec::new(),
    }
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
    /// Editor command the file finder (`f`), tree browser (`b`),
    /// find-in-files (`F`), and ⌥click file links launch, invoked as
    /// `<editor> +<line> <file>`. Any command passes through verbatim, so
    /// hand-edited configs can name editors the picker doesn't list. The
    /// `NEBULA_EDITOR` env var overrides it for the process; see
    /// [`Config::editor_command`].
    pub editor: String,
    /// Create new agent sessions straight from the kind picker, with no
    /// name prompt: the session takes the generated default name and is
    /// opted into agent-driven auto-titling, exactly as accepting an empty
    /// prompt does. Off by default — naming a session is the deliberate
    /// choice, and skipping it is opting out of that.
    pub skip_session_naming: bool,
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
    /// Whether the Workspaces column is drawn at the left edge. This is
    /// the column's only home: `Shift+W` writes it here as it toggles, so
    /// a hidden column stays hidden across restarts — and a crash or a
    /// closed browser tab can't lose the choice the way the daemon's
    /// save-on-quit UI blob would.
    pub show_workspaces: bool,
    /// Default model/effort for new Claude / Codex sessions. "default"
    /// means "don't pass the flag" (the CLI picks); any other value is
    /// passed through verbatim, so hand-edited configs can name models the
    /// pickers don't list.
    pub claude_model: String,
    pub claude_effort: String,
    pub codex_model: String,
    pub codex_effort: String,
    /// Hotkey overrides, keyed by `keymap::ActionSpec::id`; the value is a
    /// comma-separated chord list (`"j, down"`), and an empty string means
    /// deliberately unbound. Only rows that differ from the defaults are
    /// written, so the file stays small and new defaults reach existing
    /// installs. See [`crate::keymap`].
    pub keybindings: BTreeMap<String, String>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            palette_enter_attaches: true,
            git_init_on_create: true,
            editor: "vim".into(),
            skip_session_naming: false,
            recent_window: "30m".into(),
            session_idle_timeout: "5m".into(),
            theme: "default".into(),
            animations: true,
            focus_tint: false,
            show_workspaces: true,
            claude_model: "default".into(),
            claude_effort: "default".into(),
            codex_model: "default".into(),
            codex_effort: "default".into(),
            keybindings: BTreeMap::new(),
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
        // A test that reaches a save without pinning the path would write
        // the dev's own settings file (and `NEBULA_DATA_DIR` only moves it
        // to their dev instance's, which is no better). Saves hang off
        // ordinary keystrokes now — `Shift+W` is one — so make the miss
        // loud instead of leaving it to be noticed in a diff later.
        #[cfg(test)]
        assert!(
            CONFIG_PATH_OVERRIDE.with(|p| p.borrow().is_some()),
            "Config::save() in a test without a path override — wrap the \
             test body in config::with_config_path (or with_default_config)"
        );
        self.save_to(&settings_path())
    }

    pub fn save_to(&self, path: &Path) -> std::io::Result<()> {
        let root = match std::fs::read_to_string(path) {
            Ok(raw) => serde_json::from_str::<serde_json::Value>(&raw)
                .ok()
                .filter(|v| v.is_object())
                .unwrap_or_else(|| serde_json::json!({})),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => serde_json::json!({}),
            Err(err) => return Err(err),
        };
        self.write_into(path, root)
    }

    /// Put every setting back to its default and return the result. The
    /// file is rewritten from scratch rather than patched like
    /// [`Config::save`], so keys the overlay doesn't own — the daemon's
    /// `prewarm_*`, anything hand-added — go too: a reset reads as if the
    /// file had never been edited.
    pub fn reset_to_defaults() -> std::io::Result<Self> {
        #[cfg(test)]
        assert!(
            CONFIG_PATH_OVERRIDE.with(|p| p.borrow().is_some()),
            "Config::reset_to_defaults() in a test without a path override — wrap \
             the test body in config::with_config_path (or with_default_config)"
        );
        let cfg = Self::default();
        cfg.write_into(&settings_path(), serde_json::json!({}))?;
        Ok(cfg)
    }

    /// Write this config's known keys over `root` (an object) and swap the
    /// result into place atomically.
    fn write_into(&self, path: &Path, mut root: serde_json::Value) -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
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
        obj.insert("editor".into(), serde_json::json!(self.editor));
        obj.insert(
            "skip_session_naming".into(),
            serde_json::json!(self.skip_session_naming),
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
        obj.insert(
            "show_workspaces".into(),
            serde_json::json!(self.show_workspaces),
        );
        obj.insert("claude_model".into(), serde_json::json!(self.claude_model));
        obj.insert(
            "claude_effort".into(),
            serde_json::json!(self.claude_effort),
        );
        obj.insert("codex_model".into(), serde_json::json!(self.codex_model));
        obj.insert("codex_effort".into(), serde_json::json!(self.codex_effort));
        obj.insert("keybindings".into(), serde_json::json!(self.keybindings));
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

    /// The editor the file overlays launch: `NEBULA_EDITOR` when set,
    /// otherwise the `editor` setting, otherwise vim.
    pub fn editor_command(&self) -> String {
        resolve_editor(std::env::var("NEBULA_EDITOR").ok().as_deref(), &self.editor)
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

    /// Hotkeys as the event loop dispatches them: defaults with this
    /// config's overrides applied.
    pub fn keymap(&self) -> crate::keymap::Keymap {
        crate::keymap::Keymap::from_overrides(&self.keybindings)
    }

    pub fn value_label(&self, kind: SettingKind) -> String {
        match kind {
            SettingKind::PaletteEnterAttaches => on_off(self.palette_enter_attaches).into(),
            SettingKind::GitInitOnCreate => on_off(self.git_init_on_create).into(),
            SettingKind::Editor => self.editor.clone(),
            SettingKind::SkipSessionNaming => on_off(self.skip_session_naming).into(),
            SettingKind::RecentWindow => self.recent_window.clone(),
            SettingKind::SessionIdleTimeout => self.session_idle_timeout.clone(),
            SettingKind::Theme => self.theme.clone(),
            SettingKind::Animations => on_off(self.animations).into(),
            SettingKind::FocusTint => on_off(self.focus_tint).into(),
            SettingKind::ShowWorkspaces => on_off(self.show_workspaces).into(),
            SettingKind::ClaudeModel => self.claude_model.clone(),
            SettingKind::ClaudeEffort => self.claude_effort.clone(),
            SettingKind::CodexModel => self.codex_model.clone(),
            SettingKind::CodexEffort => self.codex_effort.clone(),
        }
    }

    /// `delta == 0` means activate (toggle a bool, cycle a choice forward).
    /// Non-zero delta cycles a choice; bools still toggle. `index` is
    /// tab-local — the Hotkeys tab has no cyclable values and no-ops here.
    pub fn cycle(&mut self, tab: usize, index: usize, delta: i32) {
        let Some(spec) = setting_at(tab, index) else {
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
            SettingKind::Editor => {
                self.editor = cycle_choice(&self.editor, EDITORS, step).into();
            }
            SettingKind::SkipSessionNaming => {
                self.skip_session_naming = !self.skip_session_naming;
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
            SettingKind::ShowWorkspaces => {
                self.show_workspaces = !self.show_workspaces;
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

/// First non-blank of env override → configured value → vim.
fn resolve_editor(env: Option<&str>, configured: &str) -> String {
    for value in [env.unwrap_or(""), configured] {
        let value = value.trim();
        if !value.is_empty() {
            return value.to_string();
        }
    }
    "vim".into()
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
    use crate::keymap::Keymap;

    #[test]
    fn defaults_enter_attaches() {
        assert!(Config::default().palette_enter_attaches);
        let cfg: Config = serde_json::from_str("{}").unwrap();
        assert!(cfg.palette_enter_attaches);
        let cfg: Config = serde_json::from_str(r#"{"palette_enter_attaches": false}"#).unwrap();
        assert!(!cfg.palette_enter_attaches);
    }

    #[test]
    fn reset_rewrites_the_file_from_scratch() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        with_config_path(path.clone(), || {
            let mut cfg = Config {
                theme: "midnight".into(),
                animations: false,
                ..Config::default()
            };
            cfg.keybindings.insert("git_diff".into(), "f9".into());
            cfg.save().unwrap();
            // A key the overlay doesn't own survives an ordinary save…
            let mut root: serde_json::Value =
                serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
            root["prewarm_agents"] = serde_json::json!(false);
            std::fs::write(&path, serde_json::to_vec_pretty(&root).unwrap()).unwrap();
            Config::load().save().unwrap();
            let raw = std::fs::read_to_string(&path).unwrap();
            assert!(
                raw.contains("prewarm_agents"),
                "save() patches, keeping foreign keys:\n{raw}"
            );

            // …but not a reset: the file starts over from an empty object.
            let reset = Config::reset_to_defaults().unwrap();
            assert!(reset.animations);
            assert!(reset.keybindings.is_empty());
            let raw = std::fs::read_to_string(&path).unwrap();
            assert!(
                !raw.contains("prewarm_agents"),
                "foreign key survived:\n{raw}"
            );
            let loaded = Config::load();
            assert_eq!(loaded.theme, Config::default().theme);
            assert!(loaded.animations);
            assert!(loaded.keybindings.is_empty());
        });
    }

    #[test]
    fn daemon_fields_are_ignored() {
        let cfg: Config = serde_json::from_str(r#"{"git_init_on_create": false}"#).unwrap();
        assert!(cfg.palette_enter_attaches);
        assert!(!cfg.git_init_on_create);
    }

    #[test]
    fn skip_session_naming_defaults_off_toggles_and_persists() {
        assert!(
            !Config::default().skip_session_naming,
            "naming is the default; skipping it is opt-in"
        );
        let cfg: Config = serde_json::from_str("{}").unwrap();
        assert!(!cfg.skip_session_naming);

        let mut cfg = Config::default();
        let (tab, row) = locate(SettingKind::SkipSessionNaming).unwrap();
        assert_eq!(cfg.value_label(SettingKind::SkipSessionNaming), "off");
        cfg.cycle(tab, row, 0);
        assert!(cfg.skip_session_naming);
        assert_eq!(cfg.value_label(SettingKind::SkipSessionNaming), "on");

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        cfg.save_to(&path).unwrap();
        assert!(load_from(&path).skip_session_naming);
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
        let (t, r) = locate(SettingKind::PaletteEnterAttaches).unwrap();
        assert!(cfg.palette_enter_attaches);
        cfg.cycle(t, r, 0);
        assert!(!cfg.palette_enter_attaches);
        cfg.cycle(t, r, 1);
        assert!(cfg.palette_enter_attaches);

        assert_eq!(cfg.recent_window, "30m");
        let (t, r) = locate(SettingKind::RecentWindow).unwrap();
        cfg.cycle(t, r, 0);
        assert_eq!(cfg.recent_window, "1h");
        cfg.cycle(t, r, -1);
        assert_eq!(cfg.recent_window, "30m");
        cfg.cycle(t, r, -1);
        assert_eq!(cfg.recent_window, "10m");
    }

    #[test]
    fn editor_defaults_cycles_and_persists() {
        let mut cfg = Config::default();
        assert_eq!(cfg.editor, "vim");
        let (tab, row) = locate(SettingKind::Editor).unwrap();
        cfg.cycle(tab, row, 1);
        assert_eq!(cfg.editor, "nvim");
        cfg.cycle(tab, row, -1);
        assert_eq!(cfg.editor, "vim");
        // Hand-edited commands the picker doesn't list cycle from the start.
        cfg.editor = "kak".into();
        cfg.cycle(tab, row, 1);
        assert_eq!(cfg.editor, "nvim");

        cfg.editor = "nvim".into();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        cfg.save_to(&path).unwrap();
        assert_eq!(load_from(&path).editor, "nvim");
        // A config predating the key keeps vim.
        let cfg: Config = serde_json::from_str("{}").unwrap();
        assert_eq!(cfg.editor, "vim");
    }

    #[test]
    fn editor_resolution_prefers_env_then_setting_then_vim() {
        assert_eq!(resolve_editor(Some("hx"), "nvim"), "hx");
        assert_eq!(resolve_editor(Some("  "), "nvim"), "nvim");
        assert_eq!(resolve_editor(None, " nvim "), "nvim");
        assert_eq!(resolve_editor(None, ""), "vim");
    }

    #[test]
    fn session_idle_timeout_cycles_and_persists() {
        let mut cfg = Config::default();
        assert_eq!(cfg.session_idle_timeout, "5m");
        let (tab, row) = locate(SettingKind::SessionIdleTimeout).unwrap();
        cfg.cycle(tab, row, 1);
        assert_eq!(cfg.session_idle_timeout, "15m");
        cfg.cycle(tab, row, -2);
        assert_eq!(cfg.session_idle_timeout, "1m");
        cfg.cycle(tab, row, -1);
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
        let (tab, theme_row) = locate(SettingKind::Theme).unwrap();
        cfg.cycle(tab, theme_row, 1);
        assert_eq!(cfg.theme, "ocean");
        assert_ne!(cfg.theme(), crate::theme::Theme::default());
        cfg.cycle(tab, theme_row, -1);
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
        let (tab, row) = locate(SettingKind::Animations).unwrap();
        cfg.cycle(tab, row, 0);
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
    fn show_workspaces_default_on_toggle_and_persist() {
        let mut cfg = Config::default();
        assert!(cfg.show_workspaces);
        let (tab, row) = locate(SettingKind::ShowWorkspaces).unwrap();
        cfg.cycle(tab, row, 0);
        assert!(!cfg.show_workspaces);

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        cfg.save_to(&path).unwrap();
        assert!(!load_from(&path).show_workspaces);
        // A config predating the key leaves the column shown.
        let cfg: Config = serde_json::from_str("{}").unwrap();
        assert!(cfg.show_workspaces);
    }

    #[test]
    fn focus_tint_default_off_toggle_and_persist() {
        let mut cfg = Config::default();
        assert!(!cfg.focus_tint);
        let (tab, row) = locate(SettingKind::FocusTint).unwrap();
        cfg.cycle(tab, row, 0);
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
        let (tab, row) = locate(SettingKind::ClaudeModel).unwrap();
        cfg.claude_model = "default".into();
        cfg.cycle(tab, row, 1);
        assert_eq!(cfg.claude_model, "fable");
        cfg.cycle(tab, row, -1);
        assert_eq!(cfg.claude_model, "default");
        let (tab, row) = locate(SettingKind::CodexEffort).unwrap();
        cfg.cycle(tab, row, 0);
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
    fn tabs_cover_every_setting_once_and_rows_match() {
        // Every SettingKind appears exactly once across the tabs.
        let mut kinds: Vec<SettingKind> = all_settings().map(|(_, _, s)| s.kind).collect();
        let total = kinds.len();
        kinds.sort_by_key(|k| format!("{k:?}"));
        kinds.dedup();
        assert_eq!(kinds.len(), total, "a kind repeats across tabs");

        // Each tab's rows walk its own index space, in order.
        for (t, tab) in SETTINGS_TABS.iter().enumerate() {
            let indices: Vec<usize> = settings_rows(t)
                .into_iter()
                .filter_map(|row| row.index())
                .collect();
            assert_eq!(
                indices,
                (0..tab_len(t)).collect::<Vec<_>>(),
                "{} rows",
                tab.title
            );
        }

        // Value tabs are a bare list; only Hotkeys carries headers.
        for (t, tab) in SETTINGS_TABS.iter().enumerate() {
            let headers = settings_rows(t)
                .into_iter()
                .filter(|row| matches!(row, SettingsRow::Header(_)))
                .count();
            match tab.body {
                TabBody::Values(_) => assert_eq!(headers, 0, "{}", tab.title),
                TabBody::Hotkeys => assert!(headers > 0, "hotkeys tab groups its rows"),
            }
        }
    }

    #[test]
    fn every_tab_holds_something() {
        assert!(tab_count() >= 2);
        for (t, tab) in SETTINGS_TABS.iter().enumerate() {
            assert!(tab_len(t) > 0, "{} is empty", tab.title);
            assert!(!tab.title.is_empty());
        }
        assert_eq!(tab_len(hotkeys_tab()), crate::keymap::ACTIONS.len());
    }

    #[test]
    fn keybindings_round_trip_through_the_config_file() {
        let mut cfg = Config::default();
        assert!(cfg.keybindings.is_empty(), "no overrides out of the box");
        let mut keymap = cfg.keymap();
        let quit = crate::keymap::index_of(crate::keymap::Action::Quit).unwrap();
        keymap.bind(quit, crate::keymap::KeyChord::parse("f9").unwrap(), false);
        cfg.keybindings = keymap.overrides();

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        cfg.save_to(&path).unwrap();
        let reloaded = load_from(&path);
        assert_eq!(
            reloaded.keybindings.get("quit").map(String::as_str),
            Some("f9")
        );
        assert_eq!(
            reloaded.keymap().lookup(
                crate::keymap::Scope::Global,
                &crate::keymap::KeyChord::parse("f9").unwrap()
            ),
            Some(crate::keymap::Action::Quit)
        );
        // A config predating the key still gets the full default keymap.
        let old: Config = serde_json::from_str("{}").unwrap();
        assert_eq!(
            old.keymap().label(crate::keymap::Action::Quit),
            Keymap::default().label(crate::keymap::Action::Quit)
        );
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
