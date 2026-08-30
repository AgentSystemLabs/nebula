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

/// Values the settings overlay cycles through for `session_idle_timeout`
/// (daemon-owned: how long unwatched idle sessions live before their PTY
/// is reaped).
pub const SESSION_IDLE_TIMEOUTS: &[&str] = &["off", "1m", "5m", "15m", "30m", "1h"];

/// Editor commands the settings overlay cycles through. Every entry
/// accepts `+<line> <file>`, which is how the overlays launch it. As with
/// models, hand-edited configs can name any command the list doesn't.
pub const EDITORS: &[&str] = &["vim", "nvim", "nano", "emacs", "hx"];

/// Values the settings overlay cycles through for `done_sound` — what rings
/// when a turn reaches FINISHED. `off` is silence, `bell` the terminal BEL
/// (the one sound that reaches the local terminal over `nebula ssh` — but
/// silent in Ghostty out of the box, whose `bell-features` default to
/// `no-audio`), the rest are macOS system sounds in `/System/Library/Sounds`,
/// played with `afplay`; see [`Config::done_sound`] for where a name falls
/// back to the bell. Hand-edited configs can name any sound in that folder.
pub const DONE_SOUNDS: &[&str] = &[
    "off",
    "bell",
    "Glass",
    "Ping",
    "Pop",
    "Hero",
    "Purr",
    "Tink",
    "Submarine",
    "Funk",
    "Blow",
    "Bottle",
    "Frog",
    "Morse",
    "Sosumi",
    "Basso",
];

/// Where the macOS system sounds live; `<name>.aiff` inside it.
const MACOS_SOUNDS_DIR: &str = "/System/Library/Sounds";

/// The model/effort sentinel meaning "don't pass the flag — let the CLI
/// pick"; it heads every choice list and is what the daemon sees as None.
pub const DEFAULT_CHOICE: &str = "default";

/// Model/effort choices for the new-session submenus and the settings
/// overlay. [`DEFAULT_CHOICE`] everywhere means "don't pass the flag — let
/// the CLI pick" and is what the daemon sees as None.
pub const CLAUDE_MODELS: &[&str] = &[DEFAULT_CHOICE, "fable", "opus", "sonnet", "haiku"];
pub const CLAUDE_EFFORTS: &[&str] = &[DEFAULT_CHOICE, "low", "medium", "high", "xhigh", "max"];
pub const CODEX_MODELS: &[&str] = &[
    DEFAULT_CHOICE,
    "gpt-5.6-sol",
    "gpt-5.6-terra",
    "gpt-5.6-luna",
    "gpt-5.5",
];
pub const CODEX_EFFORTS: &[&str] = &[DEFAULT_CHOICE, "minimal", "low", "medium", "high", "xhigh"];

/// Model choices for a session kind. Cursor's come from the CURSOR
/// CATALOGUE (`cursor_catalogue.rs`): a seed plus a cached
/// `cursor-agent --list-models`.
pub fn model_choices(kind: AgentKind) -> &'static [&'static str] {
    match kind {
        AgentKind::Claude => CLAUDE_MODELS,
        AgentKind::Codex => CODEX_MODELS,
        AgentKind::Cursor => crate::cursor_catalogue::models(),
    }
}

/// Effort choices for a session kind given its chosen model (None or
/// "default" = the CLI's pick). Claude and Codex take any effort with any
/// model; Cursor's list follows the family (`-fast` variants ride in the
/// effort, `high-fast`), leads with "default" only when the bare family id
/// exists, and is empty — no Effort row, no effort submenu — when the model
/// is unset or the family has no effort variants (`auto`).
pub fn effort_choices(kind: AgentKind, model: Option<&str>) -> &'static [&'static str] {
    match kind {
        AgentKind::Claude => CLAUDE_EFFORTS,
        AgentKind::Codex => CODEX_EFFORTS,
        AgentKind::Cursor => crate::cursor_catalogue::efforts(model),
    }
}

/// Whether `value` is one of `choices`, case-insensitively and trimmed —
/// how a form decides a saved or cycled choice still has a row.
pub(crate) fn fits(value: &str, choices: &[&str]) -> bool {
    choices.iter().any(|c| c.eq_ignore_ascii_case(value.trim()))
}

/// The effort to launch `kind` with, given its model and the picked effort.
/// Claude and Codex pass through. For Cursor: no family → None; an effort
/// the family ships → itself; anything else ("default", unset, a suffix the
/// family lacks) → None when the bare family id exists, otherwise the
/// family's fallback (`high` > `medium` > first) — most families have no
/// bare id, and `--model claude-fable-5` alone is refused at spawn.
pub fn fit_effort(kind: AgentKind, model: Option<&str>, effort: Option<String>) -> Option<String> {
    match kind {
        AgentKind::Claude | AgentKind::Codex => effort,
        AgentKind::Cursor => {
            let family = model
                .map(str::trim)
                .filter(|m| !m.eq_ignore_ascii_case(DEFAULT_CHOICE))?;
            let choices = crate::cursor_catalogue::efforts(Some(family));
            if choices.is_empty() {
                return None;
            }
            let picked = effort
                .map(|e| e.trim().to_ascii_lowercase())
                .filter(|e| e != DEFAULT_CHOICE);
            match picked {
                Some(e) if fits(&e, choices) => Some(e),
                _ if choices[0] == DEFAULT_CHOICE => None,
                _ => crate::cursor_catalogue::fallback_effort(family).map(String::from),
            }
        }
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
    CloseFinderOnOpen,
    SkipSessionNaming,
    SessionIdleTimeout,
    DoneSound,
    Theme,
    Animations,
    ShowWorkspaces,
    HideProjects,
    HideWorktrees,
    ClaudeEnabled,
    ClaudeModel,
    ClaudeEffort,
    CodexEnabled,
    CodexModel,
    CodexEffort,
    CursorEnabled,
    CursorModel,
    CursorEffort,
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
            SettingSpec {
                kind: SettingKind::CloseFinderOnOpen,
                label: "Finder closes on open",
                hint: "Opening a file closes f/F, so quitting the editor is one Esc",
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
                kind: SettingKind::SessionIdleTimeout,
                label: "Idle session timeout",
                hint: "Kill idle sessions in unviewed worktrees (busy ones spared; off disables)",
            },
            SettingSpec {
                kind: SettingKind::DoneSound,
                label: "Done sound",
                hint: "Ding when a turn finishes: off, the terminal bell, or a macOS system sound",
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
                kind: SettingKind::ShowWorkspaces,
                label: "Workspaces bar",
                hint: "Show the Workspaces tab bar across the top (Shift+W toggles)",
            },
            SettingSpec {
                kind: SettingKind::HideProjects,
                label: "Projects panel",
                hint: "Show or hide the Projects panel (Shift+P toggles)",
            },
            SettingSpec {
                kind: SettingKind::HideWorktrees,
                label: "Worktrees panel",
                hint: "Show or hide the Worktrees panel (Shift+B toggles)",
            },
        ]),
    },
    SettingsTab {
        title: "Agents",
        body: TabBody::Values(&[
            SettingSpec {
                kind: SettingKind::ClaudeEnabled,
                label: "Claude enabled",
                hint: "Offer Claude in the New session picker (off hides it; existing sessions keep running)",
            },
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
                kind: SettingKind::CodexEnabled,
                label: "Codex enabled",
                hint: "Offer Codex in the New session picker (off hides it; existing sessions keep running)",
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
            SettingSpec {
                kind: SettingKind::CursorEnabled,
                label: "Cursor enabled",
                hint: "Offer Cursor in the New session picker (off hides it; existing sessions keep running)",
            },
            SettingSpec {
                kind: SettingKind::CursorModel,
                label: "Cursor model",
                hint: "Default model family for new Cursor sessions (default = CLI's pick)",
            },
            SettingSpec {
                kind: SettingKind::CursorEffort,
                label: "Cursor effort",
                hint: "Effort (and -fast) variant of the chosen Cursor model; n/a while it is default or auto",
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
    /// Opening a file from the file finder (`f`) or find-in-files (`F`)
    /// closes that overlay as the editor modal opens, so quitting the
    /// editor lands back on the panels instead of on the finder the user
    /// then has to Esc a second time. When false the finder stays open
    /// underneath and quitting the editor returns to the results. Does not
    /// touch the tree browser (`b`), whose editor is embedded in its own
    /// preview pane, or ⌥click, which has no overlay to close.
    pub close_finder_on_open: bool,
    /// Create new agent sessions straight from the kind picker, with no
    /// name prompt: the session takes the generated default name and is
    /// opted into agent-driven auto-titling, exactly as accepting an empty
    /// prompt does. Off by default — naming a session is the deliberate
    /// choice, and skipping it is opting out of that.
    pub skip_session_naming: bool,
    /// How long an idle session in an unviewed worktree lives before the
    /// daemon reaps its PTY: "1m", "5m", "15m", "30m", "1h"; "off"
    /// disables. Owned by the daemon (which does the parsing and reaping);
    /// the TUI writes it so the settings overlay can cycle it.
    pub session_idle_timeout: String,
    /// What rings when a turn reaches FINISHED: "off", "bell" (terminal
    /// BEL) or the name of a macOS system sound (`Glass` by default,
    /// `Ping`, …; see [`DONE_SOUNDS`]). Resolved by [`Config::done_sound`],
    /// which falls back to the bell wherever `afplay` can't reach the
    /// user's speakers.
    pub done_sound: String,
    /// Color theme name (see `theme::THEMES`). Unknown names fall back to
    /// the default theme.
    pub theme: String,
    /// Master switch for the TUI's animations (the running/needs-feedback
    /// status-text sweep and the splash's motion). Off trades them for
    /// fewer repaints on constrained machines.
    pub animations: bool,
    /// Whether the Workspaces bar is drawn across the top. This is the
    /// bar's only home: `Shift+W` writes it here as it toggles, so a hidden
    /// bar stays hidden across restarts, and a crash or a
    /// closed browser tab can't lose the choice the way the daemon's
    /// save-on-quit UI blob would.
    pub show_workspaces: bool,
    /// Hide the Projects panel and give its width to the terminal pane.
    /// False by default so configs written before this key keep the current
    /// three-panel layout.
    pub hide_projects: bool,
    /// Hide the Worktrees panel and give its width to the terminal pane.
    /// Independent from `hide_projects`; Sessions always remains visible.
    pub hide_worktrees: bool,
    /// Default model/effort for new Claude / Codex / Cursor sessions.
    /// "default" means "don't pass the flag" (the CLI picks); any other
    /// value is passed through verbatim, so hand-edited configs can name
    /// models the pickers don't list. Cursor's pair is a family plus the
    /// effort suffix the daemon joins onto it (see `cursor_catalogue.rs`).
    pub claude_model: String,
    pub claude_effort: String,
    pub codex_model: String,
    pub codex_effort: String,
    pub cursor_model: String,
    pub cursor_effort: String,
    /// Which AGENT KINDS the NEW SESSION PICKER offers. Off leaves that
    /// harness out of the picker (and, for Claude, out of the PR SESSION
    /// launch and the standing PREWARM POOL slot); sessions that already
    /// exist keep attaching, resuming and restarting as before. All on by
    /// default, so a config predating the keys hides nothing.
    pub claude_enabled: bool,
    pub codex_enabled: bool,
    pub cursor_enabled: bool,
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
            close_finder_on_open: true,
            skip_session_naming: false,
            session_idle_timeout: "5m".into(),
            done_sound: "Glass".into(),
            theme: "default".into(),
            animations: true,
            show_workspaces: true,
            hide_projects: false,
            hide_worktrees: false,
            claude_model: DEFAULT_CHOICE.into(),
            claude_effort: DEFAULT_CHOICE.into(),
            codex_model: DEFAULT_CHOICE.into(),
            codex_effort: DEFAULT_CHOICE.into(),
            cursor_model: DEFAULT_CHOICE.into(),
            cursor_effort: DEFAULT_CHOICE.into(),
            claude_enabled: true,
            codex_enabled: true,
            cursor_enabled: true,
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
            "close_finder_on_open".into(),
            serde_json::json!(self.close_finder_on_open),
        );
        obj.insert(
            "skip_session_naming".into(),
            serde_json::json!(self.skip_session_naming),
        );
        obj.insert(
            "session_idle_timeout".into(),
            serde_json::json!(self.session_idle_timeout),
        );
        obj.insert("done_sound".into(), serde_json::json!(self.done_sound));
        obj.insert("theme".into(), serde_json::json!(self.theme));
        obj.insert("animations".into(), serde_json::json!(self.animations));
        obj.insert(
            "show_workspaces".into(),
            serde_json::json!(self.show_workspaces),
        );
        obj.insert(
            "hide_projects".into(),
            serde_json::json!(self.hide_projects),
        );
        obj.insert(
            "hide_worktrees".into(),
            serde_json::json!(self.hide_worktrees),
        );
        obj.insert("claude_model".into(), serde_json::json!(self.claude_model));
        obj.insert(
            "claude_effort".into(),
            serde_json::json!(self.claude_effort),
        );
        obj.insert("codex_model".into(), serde_json::json!(self.codex_model));
        obj.insert("codex_effort".into(), serde_json::json!(self.codex_effort));
        obj.insert("cursor_model".into(), serde_json::json!(self.cursor_model));
        obj.insert(
            "cursor_effort".into(),
            serde_json::json!(self.cursor_effort),
        );
        obj.insert(
            "claude_enabled".into(),
            serde_json::json!(self.claude_enabled),
        );
        obj.insert(
            "codex_enabled".into(),
            serde_json::json!(self.codex_enabled),
        );
        obj.insert(
            "cursor_enabled".into(),
            serde_json::json!(self.cursor_enabled),
        );
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

    /// `theme` resolved to the palette the UI draws with.
    pub fn theme(&self) -> crate::theme::Theme {
        crate::theme::Theme::by_name(&self.theme)
    }

    /// The editor the file overlays launch: `NEBULA_EDITOR` when set,
    /// otherwise the `editor` setting, otherwise vim.
    pub fn editor_command(&self) -> String {
        resolve_editor(
            nebula_core::env::non_empty(nebula_core::env::EDITOR).as_deref(),
            &self.editor,
        )
    }

    /// The configured default model for new sessions of `kind`, as the
    /// daemon wants it: None = "default" = don't pass the flag.
    pub fn default_model(&self, kind: AgentKind) -> Option<String> {
        let value = match kind {
            AgentKind::Claude => &self.claude_model,
            AgentKind::Codex => &self.codex_model,
            AgentKind::Cursor => &self.cursor_model,
        };
        non_default(value)
    }

    /// The configured default effort for new sessions of `kind`;
    /// None = "default" = don't pass the flag. For Cursor an effort the
    /// configured family does not ship is None too ([`fit_effort`]).
    pub fn default_effort(&self, kind: AgentKind) -> Option<String> {
        let value = match kind {
            AgentKind::Claude => &self.claude_effort,
            AgentKind::Codex => &self.codex_effort,
            AgentKind::Cursor => &self.cursor_effort,
        };
        fit_effort(kind, Some(&self.cursor_model), non_default(value))
    }

    /// Whether the NEW SESSION PICKER offers `kind` at all.
    pub fn kind_enabled(&self, kind: AgentKind) -> bool {
        match kind {
            AgentKind::Claude => self.claude_enabled,
            AgentKind::Codex => self.codex_enabled,
            AgentKind::Cursor => self.cursor_enabled,
        }
    }

    /// The AGENT KINDS the picker lists, in `AgentKind::ALL` order. Empty
    /// only from a hand-edited config: the overlay refuses to switch off
    /// the last one.
    pub fn enabled_kinds(&self) -> Vec<AgentKind> {
        AgentKind::ALL
            .into_iter()
            .filter(|kind| self.kind_enabled(*kind))
            .collect()
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
            SettingKind::CloseFinderOnOpen => on_off(self.close_finder_on_open).into(),
            SettingKind::SkipSessionNaming => on_off(self.skip_session_naming).into(),
            SettingKind::SessionIdleTimeout => self.session_idle_timeout.clone(),
            SettingKind::DoneSound => self.done_sound.clone(),
            SettingKind::Theme => self.theme.clone(),
            SettingKind::Animations => on_off(self.animations).into(),
            SettingKind::ShowWorkspaces => on_off(self.show_workspaces).into(),
            SettingKind::HideProjects => shown_hidden(self.hide_projects).into(),
            SettingKind::HideWorktrees => shown_hidden(self.hide_worktrees).into(),
            SettingKind::ClaudeModel => self.claude_model.clone(),
            SettingKind::ClaudeEffort => self.claude_effort.clone(),
            SettingKind::CodexModel => self.codex_model.clone(),
            SettingKind::CodexEffort => self.codex_effort.clone(),
            SettingKind::CursorModel => self.cursor_model.clone(),
            SettingKind::CursorEffort => {
                if effort_choices(AgentKind::Cursor, Some(&self.cursor_model)).is_empty() {
                    "n/a".into()
                } else {
                    self.cursor_effort.clone()
                }
            }
            SettingKind::ClaudeEnabled => on_off(self.claude_enabled).into(),
            SettingKind::CodexEnabled => on_off(self.codex_enabled).into(),
            SettingKind::CursorEnabled => on_off(self.cursor_enabled).into(),
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
            SettingKind::CloseFinderOnOpen => {
                self.close_finder_on_open = !self.close_finder_on_open;
            }
            SettingKind::SkipSessionNaming => {
                self.skip_session_naming = !self.skip_session_naming;
            }
            SettingKind::SessionIdleTimeout => {
                self.session_idle_timeout =
                    cycle_choice(&self.session_idle_timeout, SESSION_IDLE_TIMEOUTS, step).into();
            }
            SettingKind::DoneSound => {
                self.done_sound = cycle_choice(&self.done_sound, DONE_SOUNDS, step).into();
            }
            SettingKind::Theme => {
                self.theme = cycle_choice(&self.theme, crate::theme::THEMES, step).into();
            }
            SettingKind::Animations => {
                self.animations = !self.animations;
            }
            SettingKind::ShowWorkspaces => {
                self.show_workspaces = !self.show_workspaces;
            }
            SettingKind::HideProjects => {
                self.hide_projects = !self.hide_projects;
            }
            SettingKind::HideWorktrees => {
                self.hide_worktrees = !self.hide_worktrees;
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
            SettingKind::ClaudeEnabled => {
                self.claude_enabled = !self.claude_enabled;
            }
            SettingKind::CodexEnabled => {
                self.codex_enabled = !self.codex_enabled;
            }
            SettingKind::CursorEnabled => {
                self.cursor_enabled = !self.cursor_enabled;
            }
            SettingKind::CursorModel => {
                self.cursor_model =
                    cycle_choice(&self.cursor_model, crate::cursor_catalogue::models(), step)
                        .into();
                // The effort list follows the family: an effort the new
                // family lacks becomes its fallback (or default), never an
                // id the CLI would refuse.
                let choices = effort_choices(AgentKind::Cursor, Some(&self.cursor_model));
                if !fits(&self.cursor_effort, choices) {
                    self.cursor_effort =
                        fit_effort(AgentKind::Cursor, Some(&self.cursor_model), None)
                            .unwrap_or_else(|| DEFAULT_CHOICE.into());
                }
            }
            SettingKind::CursorEffort => {
                let choices = effort_choices(AgentKind::Cursor, Some(&self.cursor_model));
                if !choices.is_empty() {
                    self.cursor_effort = cycle_choice(&self.cursor_effort, choices, step).into();
                }
            }
        }
    }
}

/// What the TUI plays when a turn reaches FINISHED — the `done_sound`
/// SETTING resolved against where the TUI is running.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DoneSound {
    /// The terminal BEL (`\x07`), written through the attached terminal,
    /// which decides whether that is a sound, a flash, or a dock bounce.
    Bell,
    /// A sound file to hand to `afplay`.
    File(PathBuf),
}

impl Config {
    /// The sound to play for a finish, or `None` for silence. A named
    /// system sound only resolves to its file on macOS, on a local
    /// terminal, and when the file exists — over ssh `afplay` would ring
    /// the *remote* box, so the bell stands in there, as it does off
    /// macOS and for a name the sound folder doesn't hold.
    pub fn done_sound(&self) -> Option<DoneSound> {
        resolve_done_sound(
            &self.done_sound,
            nebula_core::host::is_remote_session(),
            cfg!(target_os = "macos"),
        )
    }
}

fn resolve_done_sound(configured: &str, remote: bool, macos: bool) -> Option<DoneSound> {
    let name = configured.trim();
    if name.is_empty() || name.eq_ignore_ascii_case("off") {
        return None;
    }
    if name.eq_ignore_ascii_case("bell") || remote || !macos {
        return Some(DoneSound::Bell);
    }
    // A sound name is a bare file stem; anything else (a path, a dot) is
    // not one, and the bell covers the typo.
    if !name.chars().all(|c| c.is_ascii_alphanumeric()) {
        return Some(DoneSound::Bell);
    }
    let path = Path::new(MACOS_SOUNDS_DIR).join(format!("{name}.aiff"));
    if path.is_file() {
        Some(DoneSound::File(path))
    } else {
        Some(DoneSound::Bell)
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

/// [`DEFAULT_CHOICE`] (or blank) → None; anything else passes through.
pub(crate) fn non_default(value: &str) -> Option<String> {
    let value = value.trim();
    (!value.is_empty() && !value.eq_ignore_ascii_case(DEFAULT_CHOICE)).then(|| value.to_string())
}

fn on_off(v: bool) -> &'static str {
    if v {
        "on"
    } else {
        "off"
    }
}

fn shown_hidden(hidden: bool) -> &'static str {
    if hidden {
        "hidden"
    } else {
        "shown"
    }
}

pub(crate) fn cycle_choice<'a>(current: &str, choices: &[&'a str], delta: i32) -> &'a str {
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
    fn defaults_close_the_finder_on_open() {
        assert!(Config::default().close_finder_on_open);
        let cfg: Config = serde_json::from_str("{}").unwrap();
        assert!(cfg.close_finder_on_open);
        let cfg: Config = serde_json::from_str(r#"{"close_finder_on_open": false}"#).unwrap();
        assert!(!cfg.close_finder_on_open);
        // The overlay toggle round-trips through the saved file.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        let mut cfg = Config::default();
        let (t, r) = locate(SettingKind::CloseFinderOnOpen).unwrap();
        cfg.cycle(t, r, 1);
        cfg.save_to(&path).unwrap();
        assert!(!load_from(&path).close_finder_on_open);
    }

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
    fn done_sound_defaults_to_bell_cycles_persists_and_resolves() {
        let mut cfg = Config::default();
        assert_eq!(cfg.done_sound, "Glass");
        // A config predating the key dings too.
        let old: Config = serde_json::from_str("{}").unwrap();
        assert_eq!(old.done_sound, "Glass");

        let (tab, row) = locate(SettingKind::DoneSound).unwrap();
        cfg.cycle(tab, row, -1);
        assert_eq!(cfg.done_sound, "bell");
        cfg.cycle(tab, row, -1);
        assert_eq!(cfg.done_sound, "off");
        cfg.cycle(tab, row, -1);
        assert_eq!(cfg.done_sound, "Basso", "the list wraps");
        cfg.cycle(tab, row, 0);
        assert_eq!(cfg.done_sound, "off");
        cfg.cycle(tab, row, 1);
        cfg.cycle(tab, row, 1);
        assert_eq!(cfg.done_sound, "Glass");
        assert_eq!(cfg.value_label(SettingKind::DoneSound), "Glass");

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        cfg.save_to(&path).unwrap();
        assert_eq!(load_from(&path).done_sound, "Glass");

        // Silence, the bell, and every reason a name falls back to it.
        assert_eq!(resolve_done_sound("off", false, true), None);
        assert_eq!(resolve_done_sound("OFF", false, true), None);
        assert_eq!(resolve_done_sound("", false, true), None);
        assert_eq!(
            resolve_done_sound("bell", false, true),
            Some(DoneSound::Bell)
        );
        assert_eq!(
            resolve_done_sound("Glass", true, true),
            Some(DoneSound::Bell),
            "over ssh afplay would ring the remote box"
        );
        assert_eq!(
            resolve_done_sound("Glass", false, false),
            Some(DoneSound::Bell),
            "no system sounds off macOS"
        );
        assert_eq!(
            resolve_done_sound("NoSuchSound", false, true),
            Some(DoneSound::Bell)
        );
        assert_eq!(
            resolve_done_sound("../etc/passwd", false, true),
            Some(DoneSound::Bell)
        );
        #[cfg(target_os = "macos")]
        assert_eq!(
            resolve_done_sound("Glass", false, true),
            Some(DoneSound::File(
                Path::new(MACOS_SOUNDS_DIR).join("Glass.aiff")
            ))
        );
    }

    #[test]
    fn cycle_toggles_bools_and_walks_session_idle_timeout() {
        let mut cfg = Config::default();
        let (t, r) = locate(SettingKind::PaletteEnterAttaches).unwrap();
        assert!(cfg.palette_enter_attaches);
        cfg.cycle(t, r, 0);
        assert!(!cfg.palette_enter_attaches);
        cfg.cycle(t, r, 1);
        assert!(cfg.palette_enter_attaches);

        assert_eq!(cfg.session_idle_timeout, "5m");
        let (t, r) = locate(SettingKind::SessionIdleTimeout).unwrap();
        cfg.cycle(t, r, 0);
        assert_eq!(cfg.session_idle_timeout, "15m");
        cfg.cycle(t, r, -1);
        assert_eq!(cfg.session_idle_timeout, "5m");
        cfg.cycle(t, r, -1);
        assert_eq!(cfg.session_idle_timeout, "1m");
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
    fn project_and_worktree_panels_default_shown_toggle_and_persist() {
        let mut cfg = Config::default();
        assert!(!cfg.hide_projects);
        assert!(!cfg.hide_worktrees);
        assert_eq!(cfg.value_label(SettingKind::HideProjects), "shown");
        assert_eq!(cfg.value_label(SettingKind::HideWorktrees), "shown");

        let (projects_tab, projects_row) = locate(SettingKind::HideProjects).unwrap();
        cfg.cycle(projects_tab, projects_row, 0);
        let (worktrees_tab, worktrees_row) = locate(SettingKind::HideWorktrees).unwrap();
        cfg.cycle(worktrees_tab, worktrees_row, 0);
        assert!(cfg.hide_projects);
        assert!(cfg.hide_worktrees);
        assert_eq!(cfg.value_label(SettingKind::HideProjects), "hidden");
        assert_eq!(cfg.value_label(SettingKind::HideWorktrees), "hidden");

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        cfg.save_to(&path).unwrap();
        let loaded = load_from(&path);
        assert!(loaded.hide_projects);
        assert!(loaded.hide_worktrees);

        // A CONFIG.JSON predating these keys keeps both panels shown.
        let legacy: Config = serde_json::from_str("{}").unwrap();
        assert!(!legacy.hide_projects);
        assert!(!legacy.hide_worktrees);
    }

    /// The FOCUS TINT is always on since 2026-08-29: a `focus_tint` key
    /// left behind in an older config.json is ignored, never an error.
    #[test]
    fn stale_focus_tint_key_is_ignored() {
        let cfg: Config = serde_json::from_str(r#"{"focus_tint": true}"#).unwrap();
        assert!(cfg.animations);
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        cfg.save_to(&path).unwrap();
        let raw: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert!(raw.get("focus_tint").is_none());
    }

    #[test]
    fn harness_toggles_default_on_and_persist() {
        let mut cfg = Config::default();
        assert!(cfg.claude_enabled && cfg.codex_enabled && cfg.cursor_enabled);
        assert_eq!(cfg.enabled_kinds(), AgentKind::ALL.to_vec());

        let (tab, row) = locate(SettingKind::CodexEnabled).unwrap();
        cfg.cycle(tab, row, 0);
        assert!(!cfg.codex_enabled);
        assert!(!cfg.kind_enabled(AgentKind::Codex));
        assert_eq!(
            cfg.enabled_kinds(),
            vec![AgentKind::Claude, AgentKind::Cursor],
            "the disabled kind drops out, order kept"
        );
        // ←/→ toggle a bool just like Enter does.
        cfg.cycle(tab, row, -1);
        assert!(cfg.codex_enabled);
        cfg.cycle(tab, row, 1);
        assert!(!cfg.codex_enabled);

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        cfg.save_to(&path).unwrap();
        let loaded = load_from(&path);
        assert!(loaded.claude_enabled);
        assert!(!loaded.codex_enabled);
        assert!(loaded.cursor_enabled);
        // A config predating the keys offers every harness.
        let cfg: Config = serde_json::from_str("{}").unwrap();
        assert_eq!(cfg.enabled_kinds().len(), 3);

        // Every kind off is representable (a hand edit), and reads as empty.
        let cfg: Config = serde_json::from_str(
            r#"{"claude_enabled":false,"codex_enabled":false,"cursor_enabled":false}"#,
        )
        .unwrap();
        assert!(cfg.enabled_kinds().is_empty());
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
        // Cursor: the family is the model; the effort only counts when
        // that family ships it.
        assert_eq!(cfg.default_model(AgentKind::Cursor), None);
        assert_eq!(cfg.default_effort(AgentKind::Cursor), None);
        cfg.cursor_effort = "high".into();
        assert_eq!(
            cfg.default_effort(AgentKind::Cursor),
            None,
            "no family, no suffix to join"
        );
        cfg.cursor_model = "claude-opus-5".into();
        assert_eq!(
            cfg.default_model(AgentKind::Cursor).as_deref(),
            Some("claude-opus-5")
        );
        assert_eq!(
            cfg.default_effort(AgentKind::Cursor).as_deref(),
            Some("high")
        );
        cfg.cursor_effort = "max".into();
        assert_eq!(
            cfg.default_effort(AgentKind::Cursor).as_deref(),
            Some("high"),
            "Opus 5 has no max variant and no bare id: its fallback launches"
        );
        cfg.cursor_model = "gpt-5.3-codex".into();
        assert_eq!(
            cfg.default_effort(AgentKind::Cursor),
            None,
            "a family with a bare id: default really is no suffix"
        );
        cfg.cursor_effort = "high-fast".into();
        assert_eq!(
            cfg.default_effort(AgentKind::Cursor).as_deref(),
            Some("high-fast")
        );

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
    fn cursor_settings_rows_follow_the_family() {
        let mut cfg = Config::default();
        let (tab, model_row) = locate(SettingKind::CursorModel).unwrap();
        let (_, effort_row) = locate(SettingKind::CursorEffort).unwrap();
        // No family: the effort row is n/a and does not cycle.
        assert_eq!(cfg.value_label(SettingKind::CursorEffort), "n/a");
        cfg.cycle(tab, effort_row, 1);
        assert_eq!(cfg.cursor_effort, "default");
        // default → auto (still no efforts) → claude-fable-5, which has no
        // bare id, so the effort lands on its fallback at once.
        cfg.cycle(tab, model_row, 1);
        assert_eq!(cfg.cursor_model, "auto");
        assert_eq!(cfg.value_label(SettingKind::CursorEffort), "n/a");
        cfg.cycle(tab, model_row, 1);
        assert_eq!(cfg.cursor_model, "claude-fable-5");
        assert_eq!(cfg.cursor_effort, "high");
        cfg.cycle(tab, effort_row, 1);
        assert_eq!(cfg.cursor_effort, "xhigh");
        cfg.cycle(tab, effort_row, 1);
        assert_eq!(cfg.cursor_effort, "max");
        // fable-5-thinking has max; Opus 5 doesn't → back to its fallback.
        cfg.cycle(tab, model_row, 1);
        assert_eq!(cfg.cursor_model, "claude-fable-5-thinking");
        assert_eq!(cfg.cursor_effort, "max", "a shared effort survives");
        cfg.cycle(tab, model_row, 1);
        assert_eq!(cfg.cursor_model, "claude-opus-5");
        assert_eq!(cfg.cursor_effort, "high");
        cfg.cycle(tab, effort_row, 1);
        assert_eq!(cfg.cursor_effort, "high-fast", "-fast rides in the effort");
        // A family with a bare id offers default again, and ← wraps onto
        // its last fast variant.
        cfg.cursor_model = "gpt-5.3-codex".into();
        cfg.cursor_effort = "default".into();
        cfg.cycle(tab, effort_row, -1);
        assert_eq!(cfg.cursor_effort, "xhigh-fast");
    }

    #[test]
    fn fit_effort_resolves_cursor_pairs() {
        let fit = |m: Option<&str>, e: Option<&str>| {
            fit_effort(AgentKind::Cursor, m, e.map(String::from))
        };
        assert_eq!(fit(None, Some("high")), None, "no family, nothing to join");
        assert_eq!(fit(Some("default"), Some("high")), None);
        assert_eq!(
            fit(Some("auto"), Some("high")),
            None,
            "auto has no variants"
        );
        assert_eq!(fit(Some("nope"), Some("high")), None);
        assert_eq!(fit(Some("gpt-5.3-codex"), None), None, "bare id exists");
        assert_eq!(fit(Some("gpt-5.3-codex"), Some("bogus")), None);
        assert_eq!(
            fit(Some("gpt-5.3-codex"), Some("fast")).as_deref(),
            Some("fast")
        );
        assert_eq!(fit(Some("claude-fable-5"), None).as_deref(), Some("high"));
        assert_eq!(
            fit(Some("claude-fable-5"), Some("default")).as_deref(),
            Some("high")
        );
        assert_eq!(
            fit(Some("claude-fable-5"), Some("MAX ")).as_deref(),
            Some("max")
        );
        assert_eq!(
            fit(Some("gpt-5.5"), Some("xhigh")).as_deref(),
            Some("high"),
            "spelled extra-high there"
        );
        assert_eq!(
            fit(Some("gpt-5.5"), Some("extra-high-fast")).as_deref(),
            Some("extra-high-fast")
        );
        assert_eq!(
            fit_effort(AgentKind::Codex, None, Some("high".into())).as_deref(),
            Some("high"),
            "claude/codex pass through"
        );
    }

    #[test]
    fn save_persists_model_effort_keys() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        let cfg = Config {
            claude_model: "sonnet".into(),
            codex_effort: "xhigh".into(),
            cursor_model: "gpt-5.6-sol".into(),
            cursor_effort: "none".into(),
            ..Config::default()
        };
        cfg.save_to(&path).unwrap();
        let reread = load_from(&path);
        assert_eq!(reread.claude_model, "sonnet");
        assert_eq!(reread.claude_effort, "default");
        assert_eq!(reread.codex_model, "default");
        assert_eq!(reread.codex_effort, "xhigh");
        assert_eq!(reread.cursor_model, "gpt-5.6-sol");
        assert_eq!(reread.cursor_effort, "none");
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
  "session_idle_timeout": "15m"
}
"#,
        )
        .unwrap();

        let mut cfg = load_from(&path);
        assert!(!cfg.git_init_on_create);
        assert_eq!(cfg.session_idle_timeout, "15m");
        cfg.palette_enter_attaches = false;
        cfg.git_init_on_create = true;
        cfg.session_idle_timeout = "1h".into();
        cfg.save_to(&path).unwrap();

        let saved: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(saved["palette_enter_attaches"], false);
        assert_eq!(saved["git_init_on_create"], true);
        assert_eq!(saved["session_idle_timeout"], "1h");
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
        assert_eq!(saved["session_idle_timeout"], "5m");
    }
}
