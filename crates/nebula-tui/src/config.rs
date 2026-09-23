//! TUI user settings, read from the same two layers the daemon reads —
//! `paths::config_path()` with `paths::config_local_path()` over it, see
//! `nebula_core::settings` (each side deserializes only its own fields;
//! serde ignores the rest). Loaded fresh at each use so edits apply without
//! restarting the TUI. A missing file or unknown fields fall back to
//! defaults; a value this build can't read costs only its own key, which is
//! logged and left as stored.
//!
//! The settings overlay is the writer: it patches known keys and leaves
//! any other JSON fields (including future daemon keys) untouched, and a
//! key `config.local.json` holds is written back there, never into the
//! portable file.

use crate::agent_presets::PresetText;
use nebula_core::harness::{CustomHarness, HarnessDescriptor};
use nebula_core::AgentKind;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

/// Values the settings overlay cycles through for `session_idle_timeout`
/// (daemon-owned: how long unwatched idle sessions live before their PTY
/// is reaped).
pub const SESSION_IDLE_TIMEOUTS: &[&str] = &["off", "1m", "5m", "15m", "30m", "1h"];
/// How many RECENT PROMPTS the SESSIONS PANEL draws under a session while
/// the feature is on: the Experimental tab's choices. A hand edit may go
/// as high as the daemon keeps (`RECENT_PROMPTS_KEPT`).
pub const RECENT_PROMPT_COUNTS: &[&str] = &["1", "2", "3", "4", "5"];
pub const DEFAULT_RECENT_PROMPTS_COUNT: usize = 3;

/// Editor commands the settings overlay cycles through. Every entry
/// accepts `+<line> <file>`, which is how the overlays launch it. As with
/// models, hand-edited configs can name any command the list doesn't.
pub const EDITORS: &[&str] = &["vim", "nvim", "nano", "emacs", "hx"];

/// The **Session pane** choices (Settings → Appearance), in the order the
/// row cycles them: the [`crate::launcher::PaneSide`] sides by name, the
/// default first.
pub const PANE_SIDES: &[&str] = &[
    crate::launcher::PaneSide::Right.as_str(),
    crate::launcher::PaneSide::Bottom.as_str(),
];

/// The **Preset text** choices (Settings → Sessions), in the order the row
/// cycles them: the [`PresetText`] sides by label.
pub const PRESET_TEXTS: &[&str] = &[
    PresetText::Prefix.as_str(),
    PresetText::Postfix.as_str(),
    PresetText::Both.as_str(),
];

/// Values the settings overlay cycles through for `done_sound` (what rings
/// when a turn reaches FINISHED) and `feedback_sound` (what rings when one
/// stops at NEEDS FEEDBACK). `off` is silence, `bell` the terminal BEL
/// (the one sound that reaches the local terminal over `nebula ssh` — but
/// silent in Ghostty out of the box, whose `bell-features` default to
/// `no-audio`), the rest are macOS system sounds in `/System/Library/Sounds`,
/// played with `afplay`; see [`Config::done_sound`] for where a name falls
/// back to the bell. Hand-edited configs can name any sound in that folder.
pub const SOUNDS: &[&str] = &[
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

/// What the overlay shows for an empty `worktree_base_branch`: the daemon
/// picks origin's default branch itself. Display only — the file holds
/// `""`, never this word.
pub const AUTO_CHOICE: &str = "auto";
/// What the Project tab's **Run command** row shows while it is empty:
/// the checkout's `.nebula.json` is what `r` reads then.
pub const PROJECT_FILE_CHOICE: &str = nebula_core::project_file::FILE_NAME;

/// The static model/effort lists live in the core registry table now
/// ([`nebula_core::harness::builtin`]); what the pickers show is built
/// below from the effective descriptor, so a `harnesses` override renames
/// the rows everywhere at once. Claude's models still come from
/// `claude_catalogue.rs` at runtime — CONFIG.JSON's `claude_models`, else
/// Claude Code's own `availableModels`, else the aliases — and Cursor's
/// from its catalogue (a seed plus a cached `cursor-agent --list-models`).

/// The `quick_prompt_kind` choices: every built-in harness id, by the name
/// the config file stores. Derived from the registry table rather than
/// spelled out, so a new built-in joins the cycle without a second edit.
pub fn agent_kind_names() -> Vec<String> {
    nebula_core::harness::builtins()
        .iter()
        .map(|descriptor| descriptor.id.clone())
        .collect()
}

/// The model rows for a harness: [`DEFAULT_CHOICE`] first ("don't pass the
/// flag — let the CLI pick", what the daemon sees as None), then the
/// runtime catalogue (Claude/Cursor, already headed) or the descriptor's
/// static list. Pi's `--model` takes a fuzzy pattern across providers, so
/// its list is families, not ids; a hand-edited `provider/id` passes
/// through verbatim.
pub fn model_choices(kind: AgentKind, custom: Option<&str>) -> Vec<String> {
    model_choices_in(&describe(kind, custom))
}

/// [`model_choices`] against an explicit descriptor, for callers that
/// already resolved one (the Agents tab, the launch sites).
pub fn model_choices_in(descriptor: &nebula_core::harness::HarnessDescriptor) -> Vec<String> {
    match descriptor.model.catalog {
        Some(nebula_core::harness::HarnessCatalog::Claude) => crate::claude_catalogue::models()
            .iter()
            .map(|s| s.to_string())
            .collect(),
        Some(nebula_core::harness::HarnessCatalog::Cursor) => crate::cursor_catalogue::models()
            .iter()
            .map(|s| s.to_string())
            .collect(),
        None => headed(
            descriptor
                .model
                .models
                .iter()
                .map(|entry| entry.id.clone())
                .collect(),
        ),
    }
}

/// Effort rows for a harness given its chosen model (None or "default" =
/// the CLI's pick). Empty — no Effort row, no effort submenu — while the
/// harness offers no effort. Cursor's list follows the family (`-fast`
/// variants ride in the effort, `high-fast`); any other harness takes its
/// static list with any model.
pub fn effort_choices(kind: AgentKind, model: Option<&str>, custom: Option<&str>) -> Vec<String> {
    effort_choices_in(&describe(kind, custom), model)
}

/// [`effort_choices`] against an explicit descriptor, for callers that
/// already resolved one (the Agents tab, the launch sites).
pub fn effort_choices_in(
    descriptor: &nebula_core::harness::HarnessDescriptor,
    model: Option<&str>,
) -> Vec<String> {
    if !descriptor.effort.offered {
        return Vec::new();
    }
    if descriptor.model.catalog == Some(nebula_core::harness::HarnessCatalog::Cursor) {
        return crate::cursor_catalogue::efforts(model)
            .iter()
            .map(|s| s.to_string())
            .collect();
    }
    headed(descriptor.effort.efforts.clone())
}

/// [`DEFAULT_CHOICE`] heading a choice list, without doubling a default
/// the source already carries.
fn headed(mut rest: Vec<String>) -> Vec<String> {
    rest.retain(|choice| !choice.eq_ignore_ascii_case(DEFAULT_CHOICE));
    let mut out = vec![DEFAULT_CHOICE.to_string()];
    out.append(&mut rest);
    out
}

/// Whether `value` is one of `choices`, case-insensitively and trimmed —
/// how a form decides a saved or cycled choice still has a row.
pub(crate) fn fits(value: &str, choices: &[impl AsRef<str>]) -> bool {
    choices
        .iter()
        .any(|c| c.as_ref().eq_ignore_ascii_case(value.trim()))
}

/// Step `current` through an owned choice list, wrapping around; a value
/// off the list steps onto it. The owned twin of [`cycle_choice`], for
/// rows the registry builds at runtime.
pub(crate) fn cycle_owned(current: &str, choices: &[String], delta: i32) -> String {
    if choices.is_empty() {
        return current.to_string();
    }
    let n = choices.len() as i32;
    let pos = choices
        .iter()
        .position(|c| c.eq_ignore_ascii_case(current.trim()))
        .unwrap_or(0) as i32;
    choices[(pos + delta).rem_euclid(n) as usize].clone()
}

/// The effort to launch with, given the harness, its model and the picked
/// effort. Most harnesses pass through. A composing harness (Cursor's
/// family-suffix shape: `--model <family>-<effort>`) fits instead: no
/// family → None; an effort the family ships → itself; anything else
/// ("default", unset, a suffix the family lacks) → None when the bare
/// family id exists, otherwise the family's fallback — most families have
/// no bare id, and a bare `--model claude-fable-5` is refused at spawn.
pub fn fit_effort(
    kind: AgentKind,
    model: Option<&str>,
    effort: Option<String>,
    custom: Option<&str>,
) -> Option<String> {
    fit_effort_in(&describe(kind, custom), model, effort)
}

/// [`fit_effort`] against an explicit descriptor, for callers that
/// already resolved one (the Agents tab, the launch sites).
pub fn fit_effort_in(
    descriptor: &nebula_core::harness::HarnessDescriptor,
    model: Option<&str>,
    effort: Option<String>,
) -> Option<String> {
    if !descriptor.compose_model_effort {
        return effort;
    }
    let family = model
        .map(str::trim)
        .filter(|m| !m.eq_ignore_ascii_case(DEFAULT_CHOICE))?;
    if descriptor.model.catalog == Some(nebula_core::harness::HarnessCatalog::Cursor) {
        let choices = crate::cursor_catalogue::efforts(Some(family));
        if choices.is_empty() {
            return None;
        }
        let picked = effort
            .map(|e| e.trim().to_ascii_lowercase())
            .filter(|e| e != DEFAULT_CHOICE);
        return match picked {
            Some(e) if fits(&e, choices) => Some(e),
            _ if choices[0] == DEFAULT_CHOICE => None,
            _ => crate::cursor_catalogue::fallback_effort(family).map(String::from),
        };
    }
    if descriptor.effort.efforts.is_empty() {
        return None;
    }
    let picked = effort
        .map(|e| e.trim().to_ascii_lowercase())
        .filter(|e| e != DEFAULT_CHOICE);
    match picked {
        Some(e) if fits(&e, &descriptor.effort.efforts) => Some(e),
        _ => static_fallback_effort(&descriptor.effort.efforts).map(String::from),
    }
}

/// The fallback effort for a static list: `high`, else `medium`, else the
/// first the harness ships.
fn static_fallback_effort(efforts: &[String]) -> Option<&str> {
    efforts
        .iter()
        .find(|e| e.as_str() == "high")
        .or_else(|| efforts.iter().find(|e| e.as_str() == "medium"))
        .or_else(|| efforts.first())
        .map(String::as_str)
}

/// The effective descriptor `(kind, custom)` reads as: the registry row
/// with the `harnesses` map, the legacy list entry, and the legacy
/// per-harness keys folded in. Loads the config fresh, like every other
/// reader here. A broken entry still resolves (the picker, not the read,
/// hides it); launches refuse it with its reason. An id the registry no
/// longer names degrades to a placeholder under its own name, so rows
/// outliving their entry still render.
fn describe(kind: AgentKind, custom: Option<&str>) -> nebula_core::harness::HarnessDescriptor {
    let id = match kind {
        AgentKind::Custom => custom.unwrap_or_default().trim(),
        _ => kind.as_str(),
    };
    let cfg = Config::load();
    if let Some(descriptor) = cfg
        .harness_registry()
        .into_iter()
        .find(|entry| entry.id == id)
    {
        return descriptor;
    }
    nebula_core::harness::CustomHarness {
        id: id.to_string(),
        label: String::new(),
        program: id.to_string(),
        enabled: true,
        model: "default".into(),
        model_flag: "--model".into(),
        hooks: None,
    }
    .as_descriptor()
}

/// One setting row in the overlay; rows live inside a [`SettingsTab`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SettingSpec {
    pub kind: SettingKind,
    pub label: &'static str,
    pub hint: &'static str,
    /// Section header the row sits under, as `keymap::ActionSpec::group`
    /// is for the Hotkeys tab. Empty means the tab lists the row bare;
    /// a tab whose rows all say so stays a flat list.
    pub group: &'static str,
}

/// What a tab shows. Ordinary tabs are a list of value settings. The
/// Project tab is a list too, but its rows are one project's — the one
/// selected in the PROJECTS PANEL, named on the tab's first line — and
/// read and write that project's entry instead of a top-level key. The
/// Hotkeys tab is generated from [`crate::keymap::ACTIONS`] instead, so a
/// new action shows up there without being declared twice — and the Agents
/// tab is generated from the harness registry, so a new CLI shows up
/// there without being declared twice.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TabBody {
    Values(&'static [SettingSpec]),
    Project(&'static [SettingSpec]),
    Hotkeys,
    Agents,
}

/// The Agents tab's static head: the cross-harness quick-prompt rows. The
/// per-harness sections below them are generated from the registry (see
/// [`Config::agent_rows`]).
pub const AGENTS_HEAD: &[SettingSpec] = &[
    SettingSpec {
        kind: SettingKind::QuickPromptKind,
        label: "Agent",
        hint: "Harness the quick prompt hotkey launches, with that kind's model/effort",
        group: "Quick prompt",
    },
    SettingSpec {
        kind: SettingKind::QuickPromptFocus,
        label: "Focus",
        hint: "Enter the new session's terminal on launch (off = just select its row)",
        group: "Quick prompt",
    },
    SettingSpec {
        kind: SettingKind::QuickPromptNewWorktree,
        label: "New worktree",
        hint: "Each new session's box starts on a fresh worktree (off = the project's root branch; ^N flips one box)",
        group: "Quick prompt",
    },
    SettingSpec {
        kind: SettingKind::HideUninstalledHarnesses,
        label: "Hide missing CLIs",
        hint: "List only harnesses found on PATH in the New session picker (daemon still checks at launch)",
        group: "Quick prompt",
    },
];

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
    WorktreeBaseBranch,
    Editor,
    CloseFinderOnOpen,
    SshSyncConfig,
    SessionIdleTimeout,
    PrewarmAgents,
    PrewarmSessions,
    DoneSound,
    FeedbackSound,
    PresetText,
    Theme,
    Animations,
    BlackBackground,
    SessionPane,
    HideProjects,
    HideWorktrees,
    HideSessions,
    HideDraftPrs,
    CardLineChanges,
    QuickPromptKind,
    QuickPromptFocus,
    QuickPromptNewWorktree,
    RunCommand,
    OpenCommand,
    RecentPrompts,
    RecentPromptsCount,
    ShowKeyCombos,
    RememberHarness,
    PrIssueCounts,
    HideUninstalledHarnesses,
}

/// One harness field row in the Agents tab. The tab renders one section
/// per registry entry — built-ins, legacy customs and `harnesses` map ids
/// alike — with an Enabled row, a Model row, and an Effort row while the
/// harness offers effort.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HarnessField {
    Enabled,
    Model,
    Effort,
}

impl HarnessField {
    pub fn label(self) -> &'static str {
        match self {
            HarnessField::Enabled => "Enabled",
            HarnessField::Model => "Model",
            HarnessField::Effort => "Effort",
        }
    }
}

impl SettingKind {
    /// A row whose value is typed, not toggled or cycled: Enter on it opens
    /// a one-line prompt pre-filled with the current value, and ←/→ have
    /// nothing to step through. [`Config::cycle`] leaves such a row alone;
    /// [`Config::set_text`] is what writes it.
    pub fn is_text(self) -> bool {
        matches!(
            self,
            SettingKind::WorktreeBaseBranch | SettingKind::RunCommand | SettingKind::OpenCommand
        )
    }

    /// A row on the PROJECT TAB: its value is the focused project's, kept
    /// in that project's `projects` entry rather than at the top level of
    /// the file. Every one of them is typed ([`SettingKind::is_text`]):
    /// [`Config::cycle`] leaves such a row alone,
    /// [`Config::set_project_text`] is what writes it, and
    /// [`ProjectSettings::value_label`] what reads it.
    pub fn is_project(self) -> bool {
        matches!(self, SettingKind::RunCommand | SettingKind::OpenCommand)
    }
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
                hint: "Enter in / search opens the session in the terminal (a red one always does)",
                group: "",
            },
            SettingSpec {
                kind: SettingKind::GitInitOnCreate,
                label: "git init new projects",
                hint: "When adding a missing directory, run git init in it",
                group: "",
            },
            SettingSpec {
                kind: SettingKind::WorktreeBaseBranch,
                label: "Worktree base branch",
                hint: "Branch new worktrees start from; Enter types one (empty = origin's default)",
                group: "",
            },
            SettingSpec {
                kind: SettingKind::Editor,
                label: "File editor",
                hint: "Editor f/b/F and ⌥click launch (NEBULA_EDITOR overrides)",
                group: "",
            },
            SettingSpec {
                kind: SettingKind::CloseFinderOnOpen,
                label: "Finder closes on open",
                hint: "Opening a file closes f/F, so quitting the editor is one Esc",
                group: "",
            },
            SettingSpec {
                kind: SettingKind::SshSyncConfig,
                label: "Sync settings over ssh",
                hint: "nebula ssh / tunnel carry config.json and presets to the remote (config.local.json stays)",
                group: "",
            },
        ]),
    },
    SettingsTab {
        title: "Sessions",
        body: TabBody::Values(&[
            SettingSpec {
                kind: SettingKind::SessionIdleTimeout,
                label: "Idle session timeout",
                hint: "Kill idle sessions in unviewed worktrees (busy ones spared; off disables)",
                group: "",
            },
            SettingSpec {
                kind: SettingKind::PrewarmAgents,
                label: "Warm spare agent",
                hint: "Boot a spare CLI in the selected worktree for instant creates (a peer in /list-agents)",
                group: "",
            },
            SettingSpec {
                kind: SettingKind::PrewarmSessions,
                label: "Prewarm dead sessions",
                hint: "Boot a worktree's dead sessions while the cursor rests on it, so attaching is instant",
                group: "",
            },
            SettingSpec {
                kind: SettingKind::DoneSound,
                label: "Done sound",
                hint: "Ding when a turn finishes: off, the terminal bell, or a macOS system sound",
                group: "",
            },
            SettingSpec {
                kind: SettingKind::FeedbackSound,
                label: "Feedback sound",
                hint: "Ring, and notify an unfocused window, when a turn stops to ask you (off silences both)",
                group: "",
            },
            SettingSpec {
                kind: SettingKind::PresetText,
                label: "Preset text",
                hint: "Where a new agent preset's text goes: a prefix before the task, a postfix after it, or both (its Text row can change one)",
                group: "",
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
                group: "",
            },
            SettingSpec {
                kind: SettingKind::Animations,
                label: "Animations",
                hint: "Status text sweep and splash motion (off = fewer repaints)",
                group: "",
            },
            SettingSpec {
                kind: SettingKind::BlackBackground,
                label: "Black background",
                hint: "Paint the window pure black instead of the terminal's own background (off keeps the terminal's, transparency included)",
                group: "",
            },
            SettingSpec {
                kind: SettingKind::SessionPane,
                label: "Session pane",
                hint: "Where the session under the cursor is read: beside the cards on the right, or under them",
                group: "",
            },
            SettingSpec {
                kind: SettingKind::HideProjects,
                label: "Projects panel",
                hint: "Collapse or expand the Projects panel (Shift+P toggles)",
                group: "",
            },
            SettingSpec {
                kind: SettingKind::HideWorktrees,
                label: "Worktrees panel",
                hint: "Collapse or expand the Worktrees panel (Shift+B toggles)",
                group: "",
            },
            SettingSpec {
                kind: SettingKind::HideSessions,
                label: "Sessions panel",
                hint: "Collapse or expand the Sessions panel (Shift+S toggles)",
                group: "",
            },
            SettingSpec {
                kind: SettingKind::HideDraftPrs,
                label: "Draft pull requests",
                hint: "Show or hide drafts in the OPEN PRS group and / search; checkouts always stay",
                group: "",
            },
            SettingSpec {
                kind: SettingKind::CardLineChanges,
                label: "Card line counts",
                hint: "Follow each card's changed-file count with its lines, +3 files +120 -45 in green and red",
                group: "",
            },
        ]),
    },
    // Generated from the harness registry: the static quick-prompt head
    // first, then one section per entry holding its enabled toggle and
    // its model / effort defaults, in registry order. A new CLI in the
    // `harnesses` map grows its own section with no code change.
    SettingsTab {
        title: "Agents",
        body: TabBody::Agents,
    },
    // Settings that belong to one project rather than to nebula. The tab
    // edits the selected project's entry in `projects` and names that
    // project on its first line, so a row here never reads as a switch
    // for every project at once.
    SettingsTab {
        title: "Project",
        body: TabBody::Project(&[
            SettingSpec {
                kind: SettingKind::RunCommand,
                label: "Run command",
                hint: "Shell line r runs in this project's worktrees (empty = its .nebula.json \"run\")",
                group: "",
            },
            SettingSpec {
                kind: SettingKind::OpenCommand,
                label: "Open command",
                hint: "Shell line ⇧Enter / ⇧O runs to open a worktree of this project, e.g. open http://localhost:3000 (empty = its .nebula.json \"open\")",
                group: "",
            },
        ]),
    },
    // Behaviors that change how the tree is worked, off by default (PR &
    // ISSUE COUNTS excepted) until they have earned a tab of their own. Before Hotkeys, which stays
    // last for the reason above.
    SettingsTab {
        title: "Experimental",
        body: TabBody::Values(&[
            SettingSpec {
                kind: SettingKind::RecentPrompts,
                label: "Recent prompts",
                hint: "List a session's last prompts under its row, newest at the bottom, each with how long ago",
                group: "",
            },
            SettingSpec {
                kind: SettingKind::RecentPromptsCount,
                label: "Recent prompts shown",
                hint: "How many of a session's recent prompts the Sessions panel lists",
                group: "",
            },
            SettingSpec {
                kind: SettingKind::ShowKeyCombos,
                label: "Key combo display",
                hint: "Spell each key you press bottom-left with what it did, for anyone watching",
                group: "",
            },
            SettingSpec {
                kind: SettingKind::RememberHarness,
                label: "Remember harness",
                hint: "A harness (and model) picked for a session becomes the Agents tab default the next launch starts on",
                group: "",
            },
            SettingSpec {
                kind: SettingKind::PrIssueCounts,
                label: "PR & issue counts",
                hint: "Count each project's open pull requests and issues after its name, 3 prs · 2 issues",
                group: "",
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

/// Index of the Agents tab, generated from the harness registry.
pub fn agents_tab() -> usize {
    SETTINGS_TABS
        .iter()
        .position(|t| t.body == TabBody::Agents)
        .expect("SETTINGS_TABS declares an Agents tab")
}

/// Index of the Project tab, whose rows are the selected project's.
pub fn project_tab() -> usize {
    SETTINGS_TABS
        .iter()
        .position(|t| matches!(t.body, TabBody::Project(_)))
        .expect("SETTINGS_TABS declares a Project tab")
}

pub fn tab_count() -> usize {
    SETTINGS_TABS.len()
}

/// The static value settings of a tab: the declared list, or the Agents
/// head (its per-harness sections are generated — see
/// [`Config::agent_rows`]). Empty for the Hotkeys tab.
pub fn tab_settings(tab: usize) -> &'static [SettingSpec] {
    match SETTINGS_TABS.get(tab).map(|t| t.body) {
        Some(TabBody::Values(settings) | TabBody::Project(settings)) => settings,
        Some(TabBody::Agents) => AGENTS_HEAD,
        _ => &[],
    }
}

/// How many selectable rows a tab holds. The Agents tab reads the
/// registry, so a new CLI grows it without a code change.
pub fn tab_len(tab: usize) -> usize {
    match SETTINGS_TABS.get(tab).map(|t| t.body) {
        Some(TabBody::Values(settings) | TabBody::Project(settings)) => settings.len(),
        Some(TabBody::Hotkeys) => crate::keymap::ACTIONS.len(),
        Some(TabBody::Agents) => AGENTS_HEAD.len() + Config::load().agent_rows().len(),
        None => 0,
    }
}

/// The static value setting at a tab-local index, if the tab declares one
/// there: the full list on ordinary tabs, the head on the Agents tab
/// (its harness rows resolve through [`Config::agent_row`]), never on
/// Hotkeys.
pub fn setting_at(tab: usize, index: usize) -> Option<&'static SettingSpec> {
    tab_settings(tab).get(index)
}

/// Where a static setting lives, as `(tab, row)`. The overlay addresses
/// settings by position, so anything that wants to talk about one by name
/// — tests, and anything that ever jumps the cursor to a named setting —
/// goes through here rather than hardcoding an index. Harness rows locate
/// through [`locate_agent`].
pub fn locate(kind: SettingKind) -> Option<(usize, usize)> {
    SETTINGS_TABS.iter().enumerate().find_map(|(t, tab)| {
        match tab.body {
            TabBody::Values(settings) | TabBody::Project(settings) => {
                settings.iter().position(|s| s.kind == kind)
            }
            TabBody::Agents => AGENTS_HEAD.iter().position(|s| s.kind == kind),
            TabBody::Hotkeys => None,
        }
        .map(|i| (t, i))
    })
}

/// Where an Agents tab harness row lives, as `(tab, row)`. Reads the
/// registry, like the tab itself.
pub fn locate_agent(id: &str, field: HarnessField) -> Option<(usize, usize)> {
    let tab = agents_tab();
    let rows = Config::load().agent_rows();
    rows.iter()
        .position(|(row_id, row_field)| row_id == id && *row_field == field)
        .map(|i| (tab, AGENTS_HEAD.len() + i))
}

/// The row declared for `kind`, wherever it sits — for anything that
/// wants its label or hint by name (the typed-row prompt's title).
pub fn spec_for(kind: SettingKind) -> Option<&'static SettingSpec> {
    all_settings()
        .map(|(_, _, spec)| spec)
        .find(|spec| spec.kind == kind)
}

/// Every static value setting, tab by tab, for coverage checks. The
/// Agents tab contributes its head; its harness rows are covered through
/// [`Config::agent_rows`].
pub fn all_settings() -> impl Iterator<Item = (usize, usize, &'static SettingSpec)> {
    SETTINGS_TABS.iter().enumerate().flat_map(|(t, tab)| {
        tab_settings(t).iter().enumerate().map(move |(i, s)| {
            let _ = tab;
            (t, i, s)
        })
    })
}

/// The one-line hint under the selected row, whatever kind of row it is.
/// The Agents tab reads the registry for its harness rows.
pub fn hint_at(tab: usize, index: usize) -> String {
    match SETTINGS_TABS.get(tab).map(|t| t.body) {
        Some(TabBody::Values(settings) | TabBody::Project(settings)) => settings
            .get(index)
            .map(|s| s.hint)
            .unwrap_or("")
            .to_string(),
        Some(TabBody::Hotkeys) => crate::keymap::spec_at(index)
            .map(|s| s.hint)
            .unwrap_or("")
            .to_string(),
        Some(TabBody::Agents) => Config::load().agent_hint_by_index(index),
        None => String::new(),
    }
}

/// One terminal row of the settings overlay body, in display order.
/// Shared by the renderer and mouse hit-testing so they can't drift.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SettingsRow {
    Blank,
    Header(String),
    /// The Project tab's first line: the selected project's name and repo
    /// path, which the renderer reads off the app — the row map is static
    /// and only knows there is such a line. Not selectable.
    Project,
    /// Label + value line for the value setting at this tab-local index.
    Setting(usize),
    /// Label + chord list for `keymap::ACTIONS[index]`.
    Hotkey(usize),
}

impl SettingsRow {
    /// The tab-local selection index this row stands for, if it's one the
    /// cursor can land on.
    pub fn index(&self) -> Option<usize> {
        match self {
            SettingsRow::Setting(i) | SettingsRow::Hotkey(i) => Some(*i),
            _ => None,
        }
    }
}

pub fn settings_rows(tab: usize) -> Vec<SettingsRow> {
    match SETTINGS_TABS.get(tab).map(|t| t.body) {
        Some(TabBody::Values(settings)) => grouped(
            settings.iter().map(|s| s.group.to_string()),
            SettingsRow::Setting,
        ),
        Some(TabBody::Project(settings)) => {
            let mut rows = vec![SettingsRow::Project];
            rows.extend(grouped(
                settings.iter().map(|s| s.group.to_string()),
                SettingsRow::Setting,
            ));
            rows
        }
        Some(TabBody::Hotkeys) => grouped(
            crate::keymap::ACTIONS.iter().map(|s| s.group.to_string()),
            SettingsRow::Hotkey,
        ),
        Some(TabBody::Agents) => {
            let cfg = Config::load();
            let head = AGENTS_HEAD.iter().map(|s| s.group.to_string());
            let rows = cfg.agent_rows();
            let groups = rows
                .iter()
                .map(|(id, _)| cfg.effective_harness_by_id(id).display_label().to_string());
            grouped(head.chain(groups), SettingsRow::Setting)
        }
        None => Vec::new(),
    }
}

/// Lays a table that is already in group order out under its section
/// headers: a header whenever the group name changes, a blank line
/// before every header but the first, and no header at all for a row
/// whose group is empty — so a tab with no groups is the bare list it
/// always was.
fn grouped(
    groups: impl Iterator<Item = String>,
    row: fn(usize) -> SettingsRow,
) -> Vec<SettingsRow> {
    let mut rows = Vec::new();
    let mut current: Option<String> = None;
    for (i, group) in groups.enumerate() {
        if !group.is_empty() && current.as_deref() != Some(&group) {
            if !rows.is_empty() {
                rows.push(SettingsRow::Blank);
            }
            rows.push(SettingsRow::Header(group.clone()));
            current = Some(group);
        }
        rows.push(row(i));
    }
    rows
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct Config {
    /// `/` palette: Enter on a session attaches and focuses the terminal.
    /// When false, Enter only lands on the session's row in the Sessions
    /// panel (previewing it in the pane) — except on a session that NEEDS
    /// FEEDBACK, the red row, which always attaches. Ctrl+O / Ctrl+F
    /// always pick open / focus explicitly, regardless of this setting.
    pub palette_enter_attaches: bool,
    /// Run `git init` after AddProject creates a missing directory.
    /// Owned by the daemon; the TUI writes it so the settings overlay can
    /// toggle every key in the shared file.
    pub git_init_on_create: bool,
    /// The branch every new WORKTREE nobody named a base for starts from
    /// (`n` in the WORKTREES PANEL, a bare `nebula worktree`, the QUICK
    /// PROMPT's auto-created one). Empty — the default, shown as `auto` —
    /// is origin's own default branch, `origin/HEAD` freshly fetched; a
    /// name (`master`, `develop`) is origin's fetched copy of that branch
    /// when origin has one, else the checkout's local branch of that name,
    /// else the default again. Owned by the daemon, which does the
    /// resolving (`git::add_worktree_off_configured`); the TUI writes it so
    /// the settings overlay can edit every key in the shared file.
    pub worktree_base_branch: String,
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
    /// `nebula ssh` and `nebula tunnel` carry this machine's `config.json`
    /// and AGENT PRESETS to the remote nebula, which merges them into its
    /// own on every connect — its `config.local.json` still wins there. On
    /// by default; `--no-sync-config` leaves them behind for one connection.
    pub ssh_sync_config: bool,
    /// The key of the **Skip starting prompt** SETTING (Settings →
    /// Sessions, through 0.30): on, `n` created the session straight from
    /// the NEW SESSION PICKER instead of putting a task box up first.
    /// Every `n` does that now — a launch that starts from a typed task is
    /// the QUICK PROMPT's — so this build never reads it and no tab edits
    /// it any more. Still loaded and written back as stored, so an older
    /// build sharing the file keeps the behavior its user chose.
    pub skip_session_naming: bool,
    /// RETIRED with the archive confirm made unconditional. Through 0.33,
    /// on, it put a CONFIRM DIALOG in front of archiving a session — the
    /// `a` key and the row menu's Archive alike — and off (the default)
    /// archived at once. Every archive asks now, so no tab shows the row
    /// and nothing reads it. Still loaded and written back as stored, so
    /// an older build sharing the file keeps the behavior its user chose.
    pub confirm_on_archive: bool,
    /// How long an idle session in an unviewed worktree lives before the
    /// daemon reaps its PTY: "1m", "5m", "15m", "30m", "1h"; "off"
    /// disables. Owned by the daemon (which does the parsing and reaping);
    /// the TUI writes it so the settings overlay can cycle it.
    pub session_idle_timeout: String,
    /// PREWARM POOL: keep one booted agent CLI standing by in the selected
    /// worktree, so creating a session there adopts it instead of waiting
    /// on a cold start. Owned by the daemon (which spawns, adopts and reaps
    /// the spare); the TUI writes it so the settings overlay can toggle it.
    /// A spare is a real CLI process — Claude's own `/list-agents` lists
    /// it beside the sessions you made, named after the directory — and
    /// switching this off drains the pool on the daemon's next sweep.
    pub prewarm_agents: bool,
    /// SESSION PREWARM: boot a worktree's dead sessions while the selection
    /// rests on it, so attaching lands on a booted screen. Daemon-owned and
    /// TUI-written, same as above.
    pub prewarm_sessions: bool,
    /// What rings when a turn reaches FINISHED: "off", "bell" (terminal
    /// BEL) or the name of a macOS system sound (`Glass` by default,
    /// `Ping`, …; see [`SOUNDS`]). Resolved by [`Config::done_sound`],
    /// which falls back to the bell wherever `afplay` can't reach the
    /// user's speakers.
    pub done_sound: String,
    /// What rings when a turn stops at NEEDS FEEDBACK — a permission
    /// prompt or a question the agent is parked on. Same values and
    /// resolution as `done_sound`; `Sosumi` by default so red and green
    /// sound different from the next room. The one knob for both the
    /// FEEDBACK SOUND and the desktop notification an unfocused terminal
    /// window gets: "off" silences the pair.
    pub feedback_sound: String,
    /// PRESET TEXT: which side of the task a new AGENT PRESET's text goes
    /// — `prefix` (one box, sent before the task), `postfix` (one box,
    /// sent after it) or `prefix & postfix` (both). The PRESET EDITOR
    /// opens a new preset on the box(es) named here, and its Text row
    /// changes one preset; a stored side with text always shows. Resolved
    /// by [`Config::preset_text`]; `prefix` by default, the framing most
    /// people reach for and one box to fill.
    pub preset_text: String,
    /// Color theme name (see `theme::THEMES`). Unknown names fall back to
    /// the default theme.
    pub theme: String,
    /// Master switch for the TUI's animations (the running/needs-feedback
    /// status-text sweep and the splash's motion). Off trades them for
    /// fewer repaints on constrained machines.
    pub animations: bool,
    /// The key of the **Focused panel tint** SETTING (Settings →
    /// Appearance, through 0.33): whether the faint accent wash behind
    /// whatever keys land in — the card under the cursor, or the session
    /// pane — was painted at all. It always is now: the wash is the one
    /// cue that says which surface keys land in, so this build never
    /// reads the key and no tab edits it. Still loaded and written back
    /// as stored, so an older build sharing the file keeps the choice its
    /// user made.
    pub focus_tint: bool,
    /// BLACK BACKGROUND: paint every cell nothing else colored pure black
    /// — the grid, the cards, the session pane, the overlays — instead of
    /// leaving it on the terminal's own background, which in a stock
    /// Ghostty is a dark gray. On by default; off lets a transparency or
    /// image configured in the terminal show through.
    pub black_background: bool,
    /// Where the LAUNCHER VIEW's PANE — the session under the cursor, live
    /// — sits against the GRID of cards: `right` (down that side of them,
    /// the default) or `bottom` (under them). Also written by the SIDE
    /// BUTTON on the pane's own TAB STRIP. Read through
    /// [`Config::pane_side`], so a word off the list — the `left` older
    /// builds offered too — is the right; a window too narrow for the
    /// pane beside the cards lays it out along the bottom until there is
    /// room (`launcher::fitted_side`).
    pub session_pane: String,
    /// The key of the **Workspaces bar** SETTING (Settings → Appearance,
    /// through 0.33): whether the bar of WORKSPACE tabs was drawn across
    /// the top. Workspaces are gone — every project is in the one list the
    /// PROJECT TABS open from — so this build never reads it and no tab
    /// edits it. Still loaded and written back as stored, so an older
    /// build sharing the file keeps the bar its user chose.
    pub show_workspaces: bool,
    /// Collapse the Projects panel to a rail and give its width to the
    /// terminal pane. False by default so configs written before this key
    /// keep the current three-panel layout.
    pub hide_projects: bool,
    /// Collapse the Worktrees panel to a rail and give its width to the
    /// terminal pane. Independent from `hide_projects` and `hide_sessions`.
    pub hide_worktrees: bool,
    /// Collapse the Sessions panel to a rail and give its width to the
    /// terminal pane. Independent from `hide_projects` and `hide_worktrees`.
    /// False by default so configs written before this key keep the
    /// current three-panel layout.
    pub hide_sessions: bool,
    /// Leave draft pull requests out of the PROJECT OPEN PRS GROUP and the
    /// `/` PALETTE's pull-request rows, so browsing what's open shows only
    /// the rows asking for a reviewer. A view filter, not a fetch filter:
    /// `gh pr list` still returns the drafts and the cache still holds
    /// them, so switching this off shows them again at once, and a draft
    /// marked ready on GitHub joins the rows on the next refresh. Never
    /// touches a checkout, its sessions, or the checkout's own PR ROW in
    /// the SESSIONS PANEL — those describe work you have, not work you are
    /// browsing. Off by default: a config predating the key hides nothing.
    pub hide_draft_prs: bool,
    /// CARD LINE COUNTS: each LAUNCHER VIEW card follows its checkout's
    /// changed-file count with the lines behind it — `+3 files +120 -45`,
    /// the added in the DIFF VIEWER's green and the removed in its red —
    /// read by a `git diff --numstat` beside every `git status` the count
    /// already runs, which only happens while this is on. Off by default:
    /// the file count alone is what a card has always said.
    pub card_line_changes: bool,
    /// RETIRED with the root always listed. Through 0.27 one switch for
    /// every project (Settings → Experimental), then through 0.33 the
    /// fallback for a project whose `projects` entry had no **Hide root
    /// worktree** row of its own: on, a project's ROOT WORKTREE was left
    /// out of everything the grid launched into. Nothing hides the root
    /// now — a launch that must not land in the shared checkout cuts a
    /// fresh worktree instead — so no tab shows the row and nothing reads
    /// this key, nor the one an entry may still carry (which rides through
    /// [`ProjectSettings::other`]). Still loaded and written back as
    /// stored, so an older build sharing the file keeps the choice its
    /// user made.
    pub hide_root_worktree: bool,
    /// PROJECT SETTINGS: one [`ProjectSettings`] per project set up
    /// differently from the rest, keyed by the project's repo path as the
    /// DAEMON stores it — what the Settings → Project tab edits for the
    /// selected project. A project with no entry reads as the defaults
    /// ([`ProjectSettings::default`]), and an entry that says nothing they
    /// don't is dropped on save ([`Config::set_project`]), so
    /// the map names only the projects that differ. One key to the file's
    /// rules: a value in here this build can't read costs the whole map,
    /// not one project.
    pub projects: BTreeMap<PathBuf, ProjectSettings>,
    /// Experimental: list each session's RECENT PROMPTS — the last few
    /// things typed into it, as the daemon captured them off the
    /// `UserPromptSubmit` hook — under its row in the SESSIONS PANEL,
    /// newest at the bottom, each with an ago label. Off by default: the
    /// rows are three lines taller with it on.
    pub recent_prompts: bool,
    /// How many of those prompts to list while `recent_prompts` is on.
    /// The overlay cycles [`RECENT_PROMPT_COUNTS`]; a hand edit is clamped
    /// to what the daemon keeps. Read through
    /// [`Config::recent_prompts_shown`].
    pub recent_prompts_count: usize,
    /// Experimental: the KEY COMBO DISPLAY — each key pressed in the
    /// panels spelled at the bottom left of the screen with what it did
    /// (`j - Move down`), vim's `showcmd` for people watching a screen
    /// share learn the shortcuts. Keys typed into a LOCKED PANE or an
    /// overlay's text field never show. Off by default: it is a teaching
    /// aid, and a row of chrome nobody asked for otherwise.
    pub show_key_combos: bool,
    /// Experimental: REMEMBER HARNESS — a launch walked through the NEW
    /// SESSION PICKER, the PR SESSION picker or the QUICK PROMPT's `Tab`
    /// picker writes its harness into `quick_prompt_kind`, and a model or
    /// effort a submenu chose into that harness's own rows, so the next
    /// picker starts on it and the next `p` launches it
    /// ([`Config::remember_launch`]). Off by default: a pick is one
    /// session's, and the AGENTS TAB is where the defaults are set.
    pub remember_harness: bool,
    /// Experimental: PR & ISSUE COUNTS — each PROJECTS PANEL row counts
    /// the repo's open pull requests and issues after its name (`3 prs ·
    /// 2 issues`), so what is waiting on a repo reads off the column
    /// without visiting it. The pull requests are the lists the OPEN PRS
    /// sweep already keeps warm for every project; the issues take a
    /// sweep of their own (`issues::sweep_others`), one project per tick,
    /// that only runs while this is on. On by default — the sweep is one
    /// `gh issue list` per project every five minutes, well inside the
    /// budget — and the one Experimental switch that is; off, the rows
    /// are what they were and no project but the selected one is asked.
    pub pr_issue_counts: bool,
    /// Default model/effort for new Claude / Codex / Cursor sessions.
    /// "default" means "don't pass the flag" (the CLI picks); any other
    /// value is passed through verbatim, so hand-edited configs can name
    /// models the pickers don't list. Cursor's pair is a family plus the
    /// effort suffix the daemon joins onto it (see `cursor_catalogue.rs`).
    pub claude_model: String,
    /// The Claude model rows the pickers, the AGENTS TAB and the PRESET
    /// EDITOR offer, verbatim, in place of the built-in aliases — for an
    /// organization allowlist or a provider (Bedrock, Vertex, a gateway)
    /// whose ids the aliases don't reach: `["claude-sonnet-5",
    /// "us.anthropic.claude-opus-5-v1:0"]`. Empty (the default) means the
    /// list follows Claude Code's own `availableModels` when one is on
    /// disk, else the aliases; see `claude_catalogue.rs`. Hand-edited only.
    pub claude_models: Vec<String>,
    pub claude_effort: String,
    pub codex_model: String,
    pub codex_effort: String,
    pub cursor_model: String,
    pub cursor_effort: String,
    /// Pi's pair: a `--model` pattern and a `--thinking` level.
    pub pi_model: String,
    pub pi_effort: String,
    /// Muse's `--model` id. `muse_effort` is reserved until the CLI
    /// documents a reasoning flag; it stores but sends nothing.
    pub muse_model: String,
    pub muse_effort: String,
    /// OpenCode's `--model` id (`provider/model`, passed verbatim). No
    /// effort key: OpenCode has no effort flag — reasoning is a per-model
    /// variant picked inside its own TUI — so its Agents section has no
    /// Effort row and nothing to store for one.
    pub opencode_model: String,
    /// Which AGENT KINDS the NEW SESSION PICKER offers. Off leaves that
    /// harness out of the picker and the PR SESSION picker (and, for
    /// Claude, out of the standing PREWARM POOL slot); sessions that already
    /// exist keep attaching, resuming and restarting as before. All on by
    /// default, so a config predating the keys hides nothing.
    pub claude_enabled: bool,
    pub codex_enabled: bool,
    pub cursor_enabled: bool,
    pub pi_enabled: bool,
    pub muse_enabled: bool,
    pub opencode_enabled: bool,
    /// When on, the New session picker lists only enabled harnesses whose
    /// CLI is found on this machine's PATH. Off by default: a login shell
    /// (mise, brew shims) can see CLIs a plain PATH lookup misses, and the
    /// daemon re-checks through the login shell at launch anyway.
    pub hide_uninstalled_harnesses: bool,
    /// User-defined harnesses (`custom_harnesses` in config.json): offered
    /// in the New session picker after the built-ins when enabled, launched
    /// with the entry's program and model flag, with process-based status
    /// unless the entry names a hook dialect. Empty by default. Legacy:
    /// new harnesses belong in `harnesses` as full descriptors, where
    /// they also gain resume, effort, system-prompt and hook-dialect rows.
    pub custom_harnesses: Vec<CustomHarness>,
    /// Per-harness deltas over the compiled-in registry (`harnesses` in
    /// config.json): disable a built-in, repoint a program, rename a
    /// flag, or define a whole new CLI. Merged by
    /// [`Config::harness_registry`]; the Agents tab, the `n` picker, the
    /// `e` presets, spawn and hooks all read the merged rows. A hand edit
    /// that breaks one entry refuses its launches with the reason, never
    /// the whole file (see `nebula_core::settings`).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub harnesses: BTreeMap<String, nebula_core::harness::HarnessOverride>,
    /// Which AGENT KIND the QUICK PROMPT hotkey launches. Its model and
    /// effort come from that kind's own defaults above, so the setting is
    /// one name, not a third model/effort pair. Read through
    /// [`Config::quick_prompt_kind`], which steps around a harness that has
    /// since been switched off.
    pub quick_prompt_kind: String,
    /// Whether a QUICK PROMPT launch takes FOCUS into the TERMINAL PANE and
    /// locks it. Off by default: the new SESSION's row is selected (so the
    /// pane previews it and it is marked seen) but FOCUS stays on the panel
    /// the prompt was fired from, so firing one off does not interrupt what
    /// you were doing. Only the QUICK PROMPT reads this — every other launch
    /// (the NEW SESSION PICKER, an AGENT PRESET, a PR SESSION, a Cloud task)
    /// still enters the pane.
    pub quick_prompt_focus: bool,
    /// Whether each new QUICK PROMPT starts aimed at a fresh worktree
    /// rather than the project's ROOT BRANCH — never the checkout of the
    /// card under the cursor, whose session takes more work as a
    /// FOLLOW-UP. `^N` flips the one box that is up; the next box starts
    /// from this again. Off by default.
    pub quick_prompt_new_worktree: bool,
    /// Hotkey overrides, keyed by `keymap::ActionSpec::id`; the value is a
    /// comma-separated chord list (`"j, down"`), and an empty string means
    /// deliberately unbound. Only rows that differ from the defaults are
    /// written, so the file stays small and new defaults reach existing
    /// installs. See [`crate::keymap`].
    pub keybindings: BTreeMap<String, String>,
    /// Keys the files held that this build could not read; they loaded as
    /// defaults. Never written: [`Config::save`] reads it to leave those
    /// stored values alone — most likely a newer nebula's — unless the
    /// setting has been changed since. See `nebula_core::settings`.
    #[serde(skip)]
    pub skipped: BTreeSet<String>,
}

/// One project's own settings — the PROJECT TAB's rows — kept under the
/// project's repo path in [`Config::projects`]. Read through
/// [`Config::project`], which supplies the defaults for a project with no
/// entry; written through [`Config::set_project`].
#[derive(Debug, Clone, Default, PartialEq, Deserialize, Serialize)]
#[serde(default)]
pub struct ProjectSettings {
    /// The RUN COMMAND `r` starts in this project's worktrees, typed on
    /// the Project tab. Empty — the default, shown as `.nebula.json` — is
    /// the checkout's PROJECT FILE `run`, where the command lived before
    /// the row existed; set, it wins over the file. The DAEMON reads it
    /// (`nebula-daemon/src/config.rs`); the TUI only edits it. Left out
    /// of the file while empty, so an entry written before the row reads
    /// the same after a save.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub run_command: String,
    /// The OPEN COMMAND `Shift+Enter` / `Shift+O` fires on this project's
    /// worktrees, typed on the Project tab — `open http://localhost:3000`,
    /// say. Empty (shown as `.nebula.json`) is the checkout's PROJECT FILE
    /// `open`; set, it wins over the file. The TUI both edits and runs it
    /// (`event_loop::open_worktree`): what it opens belongs on the machine
    /// the user sits at, never the DAEMON's. Left out of the file while
    /// empty, like `run_command`.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub open_command: String,
    /// Keys in the entry this build doesn't know — a newer nebula's, most
    /// likely, or the retired `hide_root_worktree` an older one wrote —
    /// carried through a save untouched, as the file's top-level keys
    /// are. An entry holding one is never dropped as "all default".
    #[serde(flatten)]
    pub other: BTreeMap<String, serde_json::Value>,
}

impl ProjectSettings {
    /// The overlay's label for a PROJECT TAB row ([`SettingKind::is_project`]);
    /// empty for a row that is not one.
    pub fn value_label(&self, kind: SettingKind) -> String {
        match kind {
            SettingKind::RunCommand => match self.run_command.trim() {
                "" => PROJECT_FILE_CHOICE.into(),
                command => command.to_string(),
            },
            SettingKind::OpenCommand => match self.open_command.trim() {
                "" => PROJECT_FILE_CHOICE.into(),
                command => command.to_string(),
            },
            _ => String::new(),
        }
    }

    /// The stored text of a typed PROJECT TAB row as its prompt pre-fills
    /// it — `""` for an unset row, never the `.nebula.json` the overlay
    /// shows in its place. Empty for a row that is not typed.
    pub fn text_value(&self, kind: SettingKind) -> String {
        match kind {
            SettingKind::RunCommand => self.run_command.clone(),
            SettingKind::OpenCommand => self.open_command.clone(),
            _ => String::new(),
        }
    }

    /// Write a typed value into a typed PROJECT TAB row, trimmed. Empty
    /// puts the row back on its default. False for a row that is not a
    /// typed project row — nothing changes.
    pub fn set_text(&mut self, kind: SettingKind, value: &str) -> bool {
        match kind {
            SettingKind::RunCommand => {
                self.run_command = value.trim().to_string();
                true
            }
            SettingKind::OpenCommand => {
                self.open_command = value.trim().to_string();
                true
            }
            _ => false,
        }
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            palette_enter_attaches: true,
            git_init_on_create: true,
            worktree_base_branch: String::new(),
            editor: "vim".into(),
            close_finder_on_open: true,
            ssh_sync_config: true,
            skip_session_naming: false,
            confirm_on_archive: false,
            session_idle_timeout: "5m".into(),
            prewarm_agents: true,
            prewarm_sessions: true,
            done_sound: "Glass".into(),
            feedback_sound: "Sosumi".into(),
            preset_text: PresetText::DEFAULT.as_str().into(),
            theme: "default".into(),
            animations: true,
            focus_tint: true,
            black_background: true,
            session_pane: crate::launcher::PaneSide::default().as_str().into(),
            show_workspaces: true,
            hide_projects: false,
            hide_worktrees: false,
            hide_sessions: false,
            hide_draft_prs: false,
            card_line_changes: false,
            hide_root_worktree: false,
            projects: BTreeMap::new(),
            recent_prompts: false,
            recent_prompts_count: DEFAULT_RECENT_PROMPTS_COUNT,
            show_key_combos: false,
            remember_harness: false,
            pr_issue_counts: true,
            claude_model: DEFAULT_CHOICE.into(),
            claude_models: Vec::new(),
            claude_effort: DEFAULT_CHOICE.into(),
            codex_model: DEFAULT_CHOICE.into(),
            codex_effort: DEFAULT_CHOICE.into(),
            cursor_model: DEFAULT_CHOICE.into(),
            cursor_effort: DEFAULT_CHOICE.into(),
            pi_model: DEFAULT_CHOICE.into(),
            pi_effort: DEFAULT_CHOICE.into(),
            muse_model: DEFAULT_CHOICE.into(),
            muse_effort: DEFAULT_CHOICE.into(),
            opencode_model: DEFAULT_CHOICE.into(),
            claude_enabled: true,
            codex_enabled: true,
            cursor_enabled: true,
            pi_enabled: true,
            muse_enabled: true,
            opencode_enabled: true,
            hide_uninstalled_harnesses: false,
            custom_harnesses: Vec::new(),
            harnesses: BTreeMap::new(),
            quick_prompt_kind: AgentKind::Claude.as_str().into(),
            quick_prompt_focus: false,
            quick_prompt_new_worktree: false,
            keybindings: BTreeMap::new(),
            skipped: BTreeSet::new(),
        }
    }
}

impl Config {
    pub fn load() -> Self {
        let cfg = load_layers(&settings_path(), &local_settings_path());
        // The Claude model rows follow `claude_models` live, as every
        // other hand edit does. Not under test: the list is process-global
        // and a test that never pinned the path would install the dev's.
        #[cfg(not(test))]
        crate::claude_catalogue::sync_config(&cfg.claude_models);
        cfg
    }

    /// Patch this config's known keys into the settings files, preserving
    /// any other fields already there: a key `config.local.json` holds goes
    /// back into it, every other key into `config.json`.
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
        self.write_layers(&settings_path(), &local_settings_path(), false)
    }

    /// [`Config::save`] into `path`, with its local layer beside it.
    pub fn save_to(&self, path: &Path) -> std::io::Result<()> {
        self.write_layers(path, &sibling_local_path(path), false)
    }

    /// Put every setting back to its default and return the result.
    /// `config.json` is rewritten from scratch rather than patched like
    /// [`Config::save`] and `config.local.json` is removed, so keys the
    /// overlay doesn't own — anything hand-added — go too: a reset reads as
    /// if neither file had ever been edited.
    pub fn reset_to_defaults() -> std::io::Result<Self> {
        #[cfg(test)]
        assert!(
            CONFIG_PATH_OVERRIDE.with(|p| p.borrow().is_some()),
            "Config::reset_to_defaults() in a test without a path override — wrap \
             the test body in config::with_config_path (or with_default_config)"
        );
        let local = local_settings_path();
        match std::fs::remove_file(&local) {
            Err(err) if err.kind() != std::io::ErrorKind::NotFound => return Err(err),
            _ => {}
        }
        let cfg = Self::default();
        cfg.write_layers(&settings_path(), &local, true)?;
        Ok(cfg)
    }

    /// Write this config's known keys into the two layers and swap each
    /// file that changed into place atomically. `fresh` starts `config.json`
    /// from an empty object instead of patching what is there.
    fn write_layers(&self, path: &Path, local: &Path, fresh: bool) -> std::io::Result<()> {
        use nebula_core::settings;
        let mut root = if fresh {
            settings::Object::new()
        } else {
            patchable_object(path)?
        };
        // A local layer that isn't a readable object was ignored on load,
        // so it holds no keys — and is never rewritten from a partial view.
        let mut local_root = settings::read_object(local).ok().flatten();
        let serde_json::Value::Object(known) = serde_json::to_value(self).map_err(invalid_data)?
        else {
            unreachable!("Config serializes to a JSON object");
        };
        let defaults = serde_json::to_value(Self::default()).map_err(invalid_data)?;
        let mut local_changed = false;
        for (key, value) in known {
            // A stored value this build couldn't read loaded as its default.
            // Unless it has been changed since, leave the stored one alone.
            if self.skipped.contains(&key) && defaults.get(&key) == Some(&value) {
                continue;
            }
            match local_root.as_mut() {
                Some(held) if held.contains_key(&key) => {
                    if held.get(&key) != Some(&value) {
                        held.insert(key, value);
                        local_changed = true;
                    }
                }
                _ => {
                    root.insert(key, value);
                }
            }
        }
        settings::write_json(path, &serde_json::Value::Object(root))?;
        if let (true, Some(held)) = (local_changed, local_root) {
            settings::write_json(local, &serde_json::Value::Object(held))?;
        }
        Ok(())
    }

    /// `theme` resolved to the palette the UI draws with.
    pub fn theme(&self) -> crate::theme::Theme {
        crate::theme::Theme::by_name(&self.theme)
    }

    /// `session_pane` resolved to the side the LAUNCHER VIEW lays its pane
    /// out on.
    pub fn pane_side(&self) -> crate::launcher::PaneSide {
        crate::launcher::PaneSide::parse(&self.session_pane)
    }

    /// The editor the file overlays launch: `NEBULA_EDITOR` when set,
    /// otherwise the `editor` setting, otherwise vim.
    pub fn editor_command(&self) -> String {
        resolve_editor(
            nebula_core::env::non_empty(nebula_core::env::EDITOR).as_deref(),
            &self.editor,
        )
    }

    /// The effective registry this config reads: the compiled-in known
    /// harnesses with the `harnesses` map applied, then the legacy
    /// `custom_harnesses` list, then map-only new ids — plus, for
    /// built-ins, the legacy per-harness keys (`claude_model`,
    /// `codex_enabled`, …) wherever the map stays silent on that field.
    /// The map wins where both speak; the Agents tab writes the legacy
    /// keys for built-ins, so its edits apply without a migration.
    pub fn harness_registry(&self) -> Vec<HarnessDescriptor> {
        let mut all = nebula_core::harness::registry(&self.harnesses, &self.custom_harnesses);
        for entry in &mut all {
            if nebula_core::harness::builtin(&entry.id).is_none() {
                continue;
            }
            let over = self.harnesses.get(&entry.id);
            let (legacy_enabled, legacy_model, legacy_effort) =
                self.legacy_harness_fields(&entry.id);
            if over.and_then(|o| o.enabled).is_none() {
                if let Some(enabled) = legacy_enabled {
                    entry.enabled = enabled;
                }
            }
            if over.and_then(|o| o.model_default.clone()).is_none() {
                if let Some(default) = legacy_model {
                    entry.model.default = default;
                }
            }
            if over.and_then(|o| o.effort_default.clone()).is_none() {
                if let Some(default) = legacy_effort {
                    entry.effort.default = default;
                }
            }
        }
        all
    }

    /// The legacy per-harness keys for a built-in id, as
    /// `(enabled, model, effort)` — `Some` only where the file differs
    /// from the default, i.e. where the user said something. Newer than
    /// the table, older than the `harnesses` map.
    fn legacy_harness_fields(&self, id: &str) -> (Option<bool>, Option<String>, Option<String>) {
        let (enabled, model, effort) = match id {
            "claude" => (
                &self.claude_enabled,
                &self.claude_model,
                &self.claude_effort,
            ),
            "codex" => (&self.codex_enabled, &self.codex_model, &self.codex_effort),
            "cursor" => (
                &self.cursor_enabled,
                &self.cursor_model,
                &self.cursor_effort,
            ),
            "pi" => (&self.pi_enabled, &self.pi_model, &self.pi_effort),
            "muse" => (&self.muse_enabled, &self.muse_model, &self.muse_effort),
            // OpenCode has no effort key: nothing to fall back to there.
            "opencode" => {
                return (
                    (!self.opencode_enabled).then_some(false),
                    non_default(&self.opencode_model),
                    None,
                )
            }
            _ => return (None, None, None),
        };
        (
            (!enabled).then_some(false),
            non_default(model),
            non_default(effort),
        )
    }

    /// The effective built-in descriptor `kind` reads as, regardless of
    /// whether the entry is enabled or valid (the picker hides broken
    /// rows; reads degrade gracefully).
    fn builtin_descriptor(&self, kind: AgentKind) -> HarnessDescriptor {
        let id = kind.as_str();
        self.harness_registry()
            .into_iter()
            .find(|entry| entry.id == id)
            .or_else(|| nebula_core::harness::builtin(id))
            .expect("every built-in AgentKind describes")
    }

    /// The effective descriptor `(kind, custom)` reads as: the registry
    /// row, or a placeholder under its own name when the registry no
    /// longer names the id, so rows outliving their entry still render.
    pub fn effective_harness(&self, kind: AgentKind, custom: Option<&str>) -> HarnessDescriptor {
        let id = match kind {
            AgentKind::Custom => custom.unwrap_or_default().trim(),
            _ => kind.as_str(),
        };
        if let Some(descriptor) = self
            .harness_registry()
            .into_iter()
            .find(|entry| entry.id == id)
        {
            return descriptor;
        }
        CustomHarness {
            id: id.to_string(),
            label: String::new(),
            program: id.to_string(),
            enabled: true,
            model: "default".into(),
            model_flag: "--model".into(),
            hooks: None,
        }
        .as_descriptor()
    }

    /// The configured default model for new sessions of `kind`, as the
    /// daemon wants it: None = "default" = don't pass the flag.
    pub fn default_model(&self, kind: AgentKind) -> Option<String> {
        // Custom defaults resolve from the entry at the launch site,
        // where the id is known — never through this kind-only helper.
        if kind == AgentKind::Custom {
            return None;
        }
        self.builtin_descriptor(kind)
            .default_model()
            .map(str::to_string)
    }

    /// The configured default effort for new sessions of `kind`;
    /// None = "default" = don't pass the flag. A composing harness fits
    /// the effort against its configured family ([`fit_effort`]). A
    /// reserved effort (stored, like Muse's, but with no flag mapped yet)
    /// is None whatever the file holds — spawn would drop it anyway.
    pub fn default_effort(&self, kind: AgentKind) -> Option<String> {
        if kind == AgentKind::Custom {
            return None;
        }
        let descriptor = self.builtin_descriptor(kind);
        if descriptor.effort.flag.is_none()
            && descriptor.effort.config_key.is_none()
            && !descriptor.compose_model_effort
        {
            return None;
        }
        let model = descriptor.default_model().map(str::to_string);
        let effort = descriptor.default_effort().map(str::to_string);
        fit_effort_in(&descriptor, model.as_deref(), effort)
    }

    /// Whether the NEW SESSION PICKER offers `kind` at all.
    pub fn kind_enabled(&self, kind: AgentKind) -> bool {
        if kind == AgentKind::Custom {
            // A bare Custom kind is never enabled: entries gate themselves.
            return false;
        }
        self.builtin_descriptor(kind).enabled
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

    /// The kinds the picker shows: enabled, and when
    /// `hide_uninstalled_harnesses` is on, only those whose CLI is found
    /// on PATH right now. The daemon stays authoritative at launch (it
    /// probes through the login shell, which sees more than PATH).
    pub fn visible_kinds(&self) -> Vec<AgentKind> {
        let all = self.harness_registry();
        let kinds = self.enabled_kinds();
        if !self.hide_uninstalled_harnesses {
            return kinds;
        }
        kinds
            .into_iter()
            .filter(|kind| {
                all.iter()
                    .find(|entry| entry.id == kind.as_str())
                    .is_some_and(|entry| program_installed(&entry.program))
            })
            .collect()
    }

    /// Every harness the picker and presets offer, in registry order:
    /// `(kind, None)` for built-ins, `(Custom, Some(id))` for customs.
    /// Disabled and broken entries are out (broken ones surface in the
    /// Agents tab with their reason); under `hide_uninstalled_harnesses`
    /// so is anything whose program is missing from PATH. The daemon
    /// stays authoritative at launch.
    pub fn offered_harnesses(&self) -> Vec<(AgentKind, Option<String>)> {
        let all = self.harness_registry();
        nebula_core::harness::usable(&all)
            .into_iter()
            .filter(|entry| !self.hide_uninstalled_harnesses || program_installed(&entry.program))
            .map(|entry| match AgentKind::parse(&entry.id) {
                Some(kind) => (kind, None),
                None => (AgentKind::Custom, Some(entry.id.clone())),
            })
            .collect()
    }

    /// Whether a preset may launch: its harness's Agents tab switch,
    /// enabled and valid.
    pub fn preset_harness_usable(&self, preset: &crate::agent_presets::AgentPreset) -> bool {
        let id = match preset.kind {
            AgentKind::Custom => preset.custom_harness.clone().unwrap_or_default(),
            _ => preset.kind.as_str().to_string(),
        };
        self.harness_registry()
            .iter()
            .find(|entry| entry.id == id)
            .is_some_and(|entry| entry.enabled && entry.problem().is_none())
    }

    /// PRESET TEXT resolved: the side(s) of the task a new AGENT PRESET's
    /// text goes. `prefix` for a config that never set it or a hand edit
    /// off the list; `both` is taken for `prefix & postfix`.
    pub fn preset_text(&self) -> PresetText {
        PresetText::parse(&self.preset_text).unwrap_or(PresetText::DEFAULT)
    }

    /// The effective descriptor for a registry id, or a placeholder under
    /// its own name when the registry no longer names it, so rows
    /// outliving their entry still render.
    pub fn effective_harness_by_id(&self, id: &str) -> HarnessDescriptor {
        if let Some(descriptor) = self
            .harness_registry()
            .into_iter()
            .find(|entry| entry.id == id)
        {
            return descriptor;
        }
        CustomHarness {
            id: id.to_string(),
            label: String::new(),
            program: id.to_string(),
            enabled: true,
            model: "default".into(),
            model_flag: "--model".into(),
            hooks: None,
        }
        .as_descriptor()
    }

    /// `(id, field)` rows below the Agents head, in registry order: every
    /// entry, enabled or not, valid or not (broken rows show their reason
    /// so they can be fixed); Effort only while the harness offers effort.
    pub fn agent_rows(&self) -> Vec<(String, HarnessField)> {
        let mut rows = Vec::new();
        for entry in self.harness_registry() {
            rows.push((entry.id.clone(), HarnessField::Enabled));
            rows.push((entry.id.clone(), HarnessField::Model));
            if entry.effort.offered {
                rows.push((entry.id.clone(), HarnessField::Effort));
            }
        }
        rows
    }

    /// The harness row at a tab-local Agents index, or None while the
    /// index lands on the static head.
    pub fn agent_row(&self, index: usize) -> Option<(String, HarnessField)> {
        self.agent_rows()
            .into_iter()
            .nth(index.checked_sub(AGENTS_HEAD.len())?)
    }

    /// The value an Agents harness row shows.
    pub fn agent_value(&self, id: &str, field: HarnessField) -> String {
        let descriptor = self.effective_harness_by_id(id);
        match field {
            HarnessField::Enabled => on_off(descriptor.enabled).into(),
            HarnessField::Model => descriptor.model.default.clone(),
            HarnessField::Effort => {
                let choices = effort_choices_in(
                    &descriptor,
                    descriptor.default_model().map(str::to_string).as_deref(),
                );
                if choices.is_empty() {
                    "n/a".into()
                } else {
                    descriptor.effort.default.clone()
                }
            }
        }
    }

    /// The hint an Agents harness row shows: what the row edits, with the
    /// entry's problem appended while it is broken.
    pub fn agent_hint(&self, id: &str, field: HarnessField) -> String {
        let descriptor = self.effective_harness_by_id(id);
        let label = descriptor.display_label();
        let mut hint = match field {
            HarnessField::Enabled => format!(
                "Offer {label} in the New session picker (off hides it; existing sessions keep running)"
            ),
            HarnessField::Model => match descriptor.model.catalog {
                Some(nebula_core::harness::HarnessCatalog::Claude) => format!(
                    "Default model for new {label} sessions; rows follow Claude's availableModels or config.json claude_models"
                ),
                Some(nebula_core::harness::HarnessCatalog::Cursor) => format!(
                    "Default model family for new {label} sessions; rows follow cursor-agent --list-models"
                ),
                None => format!("Default model for new {label} sessions (default = CLI's pick)"),
            },
            HarnessField::Effort => {
                if descriptor.compose_model_effort {
                    format!(
                        "Effort (and -fast) variant of the chosen {label} model; n/a while it is default or auto"
                    )
                } else if let Some(flag) = descriptor.effort.flag.as_deref() {
                    format!("Default reasoning effort ({flag}) for new {label} sessions")
                } else if let (Some(flag), Some(key)) = (
                    descriptor.effort.config_flag.as_deref(),
                    descriptor.effort.config_key.as_deref(),
                ) {
                    format!("Default reasoning effort ({flag} {key}=) for new {label} sessions")
                } else {
                    format!("Reserved until the {label} CLI documents a reasoning flag (default = unset)")
                }
            }
        };
        if let Some(problem) = descriptor.problem() {
            hint.push_str(&format!(" — broken: {problem}"));
        }
        hint
    }

    /// The hint for a tab-local Agents index: the head row's own hint, or
    /// the harness row's.
    pub fn agent_hint_by_index(&self, index: usize) -> String {
        if let Some(spec) = AGENTS_HEAD.get(index) {
            return spec.hint.to_string();
        }
        match self.agent_row(index) {
            Some((id, field)) => self.agent_hint(&id, field),
            None => String::new(),
        }
    }

    /// Cycle an Agents harness row: toggle Enabled, step Model / Effort
    /// through the rows the pickers offer. A composing harness refits its
    /// effort against the new model, so the row never holds an id the CLI
    /// would refuse.
    pub fn cycle_agent_row(&mut self, id: &str, field: HarnessField, delta: i32) {
        let Some(descriptor) = self
            .harness_registry()
            .into_iter()
            .find(|entry| entry.id == id)
        else {
            return;
        };
        let step = if delta == 0 { 1 } else { delta };
        match field {
            HarnessField::Enabled => self.set_harness_enabled(id, !descriptor.enabled),
            HarnessField::Model => {
                let choices = model_choices_in(&descriptor);
                let next = cycle_owned(&descriptor.model.default, &choices, step);
                self.set_harness_model(id, next.clone());
                let descriptor = self.effective_harness_by_id(id);
                if descriptor.compose_model_effort {
                    let choices = effort_choices_in(&descriptor, Some(&next));
                    if !fits(&descriptor.effort.default, &choices) {
                        let fitted = fit_effort_in(&descriptor, Some(&next), None)
                            .unwrap_or_else(|| DEFAULT_CHOICE.into());
                        self.set_harness_effort(id, fitted);
                    }
                }
            }
            HarnessField::Effort => {
                let choices = effort_choices_in(
                    &descriptor,
                    descriptor.default_model().map(str::to_string).as_deref(),
                );
                if choices.is_empty() {
                    return;
                }
                let next = cycle_owned(&descriptor.effort.default, &choices, step);
                self.set_harness_effort(id, next);
            }
        }
    }

    /// Write an Enabled toggle: the `harnesses` map where it speaks, else
    /// the legacy layer (the built-in switch, the list entry).
    fn set_harness_enabled(&mut self, id: &str, enabled: bool) {
        if self.harnesses.get(id).and_then(|o| o.enabled).is_some() {
            self.harness_override_mut(id).enabled = Some(enabled);
            return;
        }
        if nebula_core::harness::builtin(id).is_some() {
            self.set_legacy_enabled(id, enabled);
            return;
        }
        if let Some(entry) = self
            .custom_harnesses
            .iter_mut()
            .find(|entry| entry.id == id)
        {
            entry.enabled = enabled;
            return;
        }
        self.harness_override_mut(id).enabled = Some(enabled);
    }

    /// Write a Model default: the map where it speaks, else the legacy
    /// layer (the built-in model key, the list entry's model).
    fn set_harness_model(&mut self, id: &str, model: String) {
        if self
            .harnesses
            .get(id)
            .and_then(|o| o.model_default.clone())
            .is_some()
        {
            self.harness_override_mut(id).model_default = Some(model);
            return;
        }
        if nebula_core::harness::builtin(id).is_some() {
            self.set_legacy_model(id, model);
            return;
        }
        if let Some(entry) = self
            .custom_harnesses
            .iter_mut()
            .find(|entry| entry.id == id)
        {
            entry.model = model;
            return;
        }
        self.harness_override_mut(id).model_default = Some(model);
    }

    /// Write an Effort default: the map where it speaks, else the legacy
    /// built-in effort key (legacy list entries hold no effort; the map
    /// owns theirs).
    fn set_harness_effort(&mut self, id: &str, effort: String) {
        if nebula_core::harness::builtin(id).is_some()
            && self
                .harnesses
                .get(id)
                .and_then(|o| o.effort_default.clone())
                .is_none()
        {
            self.set_legacy_effort(id, effort);
            return;
        }
        self.harness_override_mut(id).effort_default = Some(effort);
    }

    /// The `harnesses` map entry for `id`, created when absent.
    fn harness_override_mut(&mut self, id: &str) -> &mut nebula_core::harness::HarnessOverride {
        self.harnesses.entry(id.to_string()).or_default()
    }

    fn set_legacy_enabled(&mut self, id: &str, enabled: bool) {
        match id {
            "claude" => self.claude_enabled = enabled,
            "codex" => self.codex_enabled = enabled,
            "cursor" => self.cursor_enabled = enabled,
            "pi" => self.pi_enabled = enabled,
            "muse" => self.muse_enabled = enabled,
            "opencode" => self.opencode_enabled = enabled,
            _ => self.harness_override_mut(id).enabled = Some(enabled),
        }
    }

    fn set_legacy_model(&mut self, id: &str, model: String) {
        match id {
            "claude" => self.claude_model = model,
            "codex" => self.codex_model = model,
            "cursor" => self.cursor_model = model,
            "pi" => self.pi_model = model,
            "muse" => self.muse_model = model,
            "opencode" => self.opencode_model = model,
            _ => self.harness_override_mut(id).model_default = Some(model),
        }
    }

    fn set_legacy_effort(&mut self, id: &str, effort: String) {
        match id {
            "claude" => self.claude_effort = effort,
            "codex" => self.codex_effort = effort,
            "cursor" => self.cursor_effort = effort,
            "pi" => self.pi_effort = effort,
            "muse" => self.muse_effort = effort,
            _ => self.harness_override_mut(id).effort_default = Some(effort),
        }
    }

    /// The AGENT KIND the QUICK PROMPT launches: the `quick_prompt_kind`
    /// setting, stepped on to the first enabled kind when that harness has
    /// been switched off on the AGENTS TAB since it was chosen (an
    /// unreadable name, or a hand-edited config with every harness off,
    /// reads as Claude — the same default the picker starts from).
    pub fn quick_prompt_kind(&self) -> AgentKind {
        let configured = AgentKind::parse(&self.quick_prompt_kind).unwrap_or_default();
        if self.kind_enabled(configured) {
            return configured;
        }
        self.enabled_kinds().first().copied().unwrap_or(configured)
    }

    /// The harness the NEW SESSION PICKER (and the PR SESSION picker)
    /// starts on: the last launch's while REMEMBER HARNESS is on — read
    /// through [`Config::quick_prompt_kind`], so one switched off since
    /// steps aside — and None, the first row, while it is off.
    pub fn remembered_kind(&self) -> Option<AgentKind> {
        self.remember_harness.then(|| self.quick_prompt_kind())
    }

    /// REMEMBER HARNESS (Settings → Experimental): make `kind` — and a
    /// model or effort a picker chose for it, `None` for one it did not —
    /// the defaults the next launch starts from, by writing the AGENTS
    /// TAB's own rows: `quick_prompt_kind`, and that harness's Model /
    /// Effort (the registry entry's, keyed by the kind name or the
    /// custom id). An explicit `"default"` pick lands as the row's own
    /// `default`. The QUICK PROMPT names only built-in kinds, so a custom
    /// entry is remembered by its Model / Effort rows alone. Returns
    /// whether anything changed, so the caller saves only then; nothing
    /// moves while the switch is off.
    pub fn remember_launch(
        &mut self,
        kind: AgentKind,
        custom: Option<&str>,
        model: Option<&str>,
        effort: Option<&str>,
    ) -> bool {
        if !self.remember_harness {
            return false;
        }
        let id = match (kind, custom) {
            (AgentKind::Custom, Some(id)) => id.to_string(),
            (AgentKind::Custom, None) => return false,
            (kind, _) => kind.as_str().to_string(),
        };
        let mut changed = false;
        if kind != AgentKind::Custom && self.quick_prompt_kind != kind.as_str() {
            self.quick_prompt_kind = kind.as_str().into();
            changed = true;
        }
        let before = self.effective_harness_by_id(&id);
        if let Some(model) = model.map(str::trim).filter(|m| !m.is_empty()) {
            if before.model.default != model {
                self.set_harness_model(&id, model.into());
                changed = true;
            }
        }
        if let Some(effort) = effort.map(str::trim).filter(|e| !e.is_empty()) {
            if before.effort.default != effort {
                self.set_harness_effort(&id, effort.into());
                changed = true;
            }
        }
        // A composing harness (Cursor's family-suffix shape) keeps the
        // stored effort only if the family it now names ships it, as the
        // AGENTS TAB's own Model cycle does — never an id the CLI would
        // refuse.
        if changed {
            let descriptor = self.effective_harness_by_id(&id);
            if descriptor.compose_model_effort {
                let family = descriptor.default_model().map(str::to_string);
                let choices = effort_choices_in(&descriptor, family.as_deref());
                if !fits(&descriptor.effort.default, &choices) {
                    let fitted = fit_effort_in(&descriptor, family.as_deref(), None)
                        .unwrap_or_else(|| DEFAULT_CHOICE.into());
                    self.set_harness_effort(&id, fitted);
                }
            }
        }
        changed
    }

    /// How many RECENT PROMPTS the SESSIONS PANEL lists under a session:
    /// zero while the feature is off, else the count clamped to what the
    /// daemon keeps (a hand-edited `0` or `50` reads as `1` or the cap,
    /// never as nothing while the switch says on).
    pub fn recent_prompts_shown(&self) -> usize {
        if !self.recent_prompts {
            return 0;
        }
        self.recent_prompts_count
            .clamp(1, nebula_core::RECENT_PROMPTS_KEPT)
    }

    /// Hotkeys as the event loop dispatches them: defaults with this
    /// config's overrides applied.
    pub fn keymap(&self) -> crate::keymap::Keymap {
        crate::keymap::Keymap::from_overrides(&self.keybindings)
    }

    /// The settings of the project checked out at `repo_path`: its
    /// `projects` entry, else the defaults every project without one gets.
    pub fn project(&self, repo_path: &Path) -> ProjectSettings {
        self.projects.get(repo_path).cloned().unwrap_or_default()
    }

    /// Store `settings` as the entry of the project at `repo_path`. An
    /// entry that says nothing the defaults don't is dropped rather than
    /// written, so `projects` names only the projects set up differently
    /// — and a project turned back to match the rest leaves no trace.
    pub fn set_project(&mut self, repo_path: &Path, settings: ProjectSettings) {
        if settings == ProjectSettings::default() {
            self.projects.remove(repo_path);
        } else {
            self.projects.insert(repo_path.to_path_buf(), settings);
        }
    }

    /// The stored text of a typed PROJECT TAB row for the project at
    /// `repo_path` — what [`Config::text_value`] is for a top-level row.
    pub fn project_text_value(&self, repo_path: &Path, kind: SettingKind) -> String {
        self.project(repo_path).text_value(kind)
    }

    /// Write a typed PROJECT TAB row for the project at `repo_path` — what
    /// [`Config::set_text`] is for a top-level row. False for a row that
    /// is not a typed project row.
    pub fn set_project_text(&mut self, repo_path: &Path, kind: SettingKind, value: &str) -> bool {
        let mut settings = self.project(repo_path);
        if !settings.set_text(kind, value) {
            return false;
        }
        self.set_project(repo_path, settings);
        true
    }

    /// The overlay's label for `kind`. A PROJECT TAB row read here shows
    /// the fallback — the overlay reads the selected project's through
    /// [`Config::project`] instead.
    pub fn value_label(&self, kind: SettingKind) -> String {
        match kind {
            SettingKind::PaletteEnterAttaches => on_off(self.palette_enter_attaches).into(),
            SettingKind::GitInitOnCreate => on_off(self.git_init_on_create).into(),
            SettingKind::WorktreeBaseBranch => match self.worktree_base_branch.trim() {
                "" => AUTO_CHOICE.into(),
                name => name.to_string(),
            },
            SettingKind::Editor => self.editor.clone(),
            SettingKind::CloseFinderOnOpen => on_off(self.close_finder_on_open).into(),
            SettingKind::SshSyncConfig => on_off(self.ssh_sync_config).into(),
            SettingKind::SessionIdleTimeout => self.session_idle_timeout.clone(),
            SettingKind::PrewarmAgents => on_off(self.prewarm_agents).into(),
            SettingKind::PrewarmSessions => on_off(self.prewarm_sessions).into(),
            SettingKind::DoneSound => self.done_sound.clone(),
            SettingKind::FeedbackSound => self.feedback_sound.clone(),
            SettingKind::PresetText => self.preset_text().as_str().into(),
            SettingKind::Theme => self.theme.clone(),
            SettingKind::Animations => on_off(self.animations).into(),
            SettingKind::BlackBackground => on_off(self.black_background).into(),
            SettingKind::SessionPane => self.pane_side().as_str().into(),
            SettingKind::HideProjects => shown_hidden(self.hide_projects).into(),
            SettingKind::HideWorktrees => shown_hidden(self.hide_worktrees).into(),
            SettingKind::HideSessions => shown_hidden(self.hide_sessions).into(),
            SettingKind::HideDraftPrs => shown_hidden(self.hide_draft_prs).into(),
            SettingKind::CardLineChanges => on_off(self.card_line_changes).into(),
            // A project row with no project to speak of: what one without
            // an entry would show.
            SettingKind::RunCommand | SettingKind::OpenCommand => {
                ProjectSettings::default().value_label(kind)
            }
            SettingKind::RecentPrompts => on_off(self.recent_prompts).into(),
            SettingKind::ShowKeyCombos => on_off(self.show_key_combos).into(),
            SettingKind::RememberHarness => on_off(self.remember_harness).into(),
            SettingKind::PrIssueCounts => on_off(self.pr_issue_counts).into(),
            SettingKind::RecentPromptsCount => self
                .recent_prompts_count
                .clamp(1, nebula_core::RECENT_PROMPTS_KEPT)
                .to_string(),
            SettingKind::HideUninstalledHarnesses => on_off(self.hide_uninstalled_harnesses).into(),
            SettingKind::QuickPromptKind => self.quick_prompt_kind.clone(),
            SettingKind::QuickPromptFocus => on_off(self.quick_prompt_focus).into(),
            SettingKind::QuickPromptNewWorktree => on_off(self.quick_prompt_new_worktree).into(),
        }
    }

    /// `delta == 0` means activate (toggle a bool, cycle a choice forward).
    /// Non-zero delta cycles a choice; bools still toggle. `index` is
    /// tab-local — the Hotkeys tab has no cyclable values and no-ops here,
    /// and so does a PROJECT TAB row, which is typed for one project
    /// ([`Config::set_project_text`]). The Agents tab resolves its head rows
    /// statically and its harness rows through the registry.
    pub fn cycle(&mut self, tab: usize, index: usize, delta: i32) {
        if tab == agents_tab() {
            if let Some(spec) = AGENTS_HEAD.get(index) {
                self.cycle_kind(spec.kind, delta);
            } else if let Some((id, field)) = self.agent_row(index) {
                self.cycle_agent_row(&id, field, delta);
            }
            return;
        }
        let Some(spec) = setting_at(tab, index) else {
            return;
        };
        self.cycle_kind(spec.kind, delta);
    }

    /// Cycle one static setting row. The Agents tab's harness rows cycle
    /// through [`Config::cycle_agent_row`] instead.
    fn cycle_kind(&mut self, kind: SettingKind, delta: i32) {
        let step = if delta == 0 { 1 } else { delta };
        match kind {
            SettingKind::PaletteEnterAttaches => {
                self.palette_enter_attaches = !self.palette_enter_attaches;
            }
            SettingKind::GitInitOnCreate => {
                self.git_init_on_create = !self.git_init_on_create;
            }
            // Typed, not cycled: see `SettingKind::is_text` / `set_text`.
            SettingKind::WorktreeBaseBranch => {}
            SettingKind::Editor => {
                self.editor = cycle_choice(&self.editor, EDITORS, step).into();
            }
            SettingKind::CloseFinderOnOpen => {
                self.close_finder_on_open = !self.close_finder_on_open;
            }
            SettingKind::SshSyncConfig => {
                self.ssh_sync_config = !self.ssh_sync_config;
            }
            SettingKind::SessionIdleTimeout => {
                self.session_idle_timeout =
                    cycle_choice(&self.session_idle_timeout, SESSION_IDLE_TIMEOUTS, step).into();
            }
            SettingKind::PrewarmAgents => {
                self.prewarm_agents = !self.prewarm_agents;
            }
            SettingKind::PrewarmSessions => {
                self.prewarm_sessions = !self.prewarm_sessions;
            }
            SettingKind::DoneSound => {
                self.done_sound = cycle_choice(&self.done_sound, SOUNDS, step).into();
            }
            SettingKind::FeedbackSound => {
                self.feedback_sound = cycle_choice(&self.feedback_sound, SOUNDS, step).into();
            }
            SettingKind::PresetText => {
                // Cycled from the resolved side, so a hand edit off the
                // list steps on from the default it reads as.
                self.preset_text =
                    cycle_choice(self.preset_text().as_str(), PRESET_TEXTS, step).into();
            }
            SettingKind::Theme => {
                self.theme = cycle_choice(&self.theme, crate::theme::THEMES, step).into();
            }
            SettingKind::Animations => {
                self.animations = !self.animations;
            }
            SettingKind::BlackBackground => {
                self.black_background = !self.black_background;
            }
            SettingKind::SessionPane => {
                // Cycled from the resolved side, so a hand edit off the
                // list steps on from the right it reads as.
                self.session_pane =
                    cycle_choice(self.pane_side().as_str(), PANE_SIDES, step).into();
            }
            SettingKind::HideProjects => {
                self.hide_projects = !self.hide_projects;
            }
            SettingKind::HideWorktrees => {
                self.hide_worktrees = !self.hide_worktrees;
            }
            SettingKind::HideSessions => {
                self.hide_sessions = !self.hide_sessions;
            }
            SettingKind::HideDraftPrs => {
                self.hide_draft_prs = !self.hide_draft_prs;
            }
            SettingKind::CardLineChanges => {
                self.card_line_changes = !self.card_line_changes;
            }
            // One project's, not the file's, and typed: see `set_project_text`.
            SettingKind::RunCommand | SettingKind::OpenCommand => {}
            SettingKind::RecentPrompts => {
                self.recent_prompts = !self.recent_prompts;
            }
            SettingKind::RecentPromptsCount => {
                // A hand-edited count off the list steps onto it.
                let current = self.recent_prompts_count.to_string();
                self.recent_prompts_count = cycle_choice(&current, RECENT_PROMPT_COUNTS, step)
                    .parse()
                    .unwrap_or(DEFAULT_RECENT_PROMPTS_COUNT);
            }
            SettingKind::ShowKeyCombos => {
                self.show_key_combos = !self.show_key_combos;
            }
            SettingKind::RememberHarness => {
                self.remember_harness = !self.remember_harness;
            }
            SettingKind::PrIssueCounts => {
                self.pr_issue_counts = !self.pr_issue_counts;
            }
            SettingKind::HideUninstalledHarnesses => {
                self.hide_uninstalled_harnesses = !self.hide_uninstalled_harnesses;
            }
            SettingKind::QuickPromptKind => {
                self.quick_prompt_kind =
                    cycle_owned(&self.quick_prompt_kind, &agent_kind_names(), step);
            }
            SettingKind::QuickPromptFocus => {
                self.quick_prompt_focus = !self.quick_prompt_focus;
            }
            SettingKind::QuickPromptNewWorktree => {
                self.quick_prompt_new_worktree = !self.quick_prompt_new_worktree;
            }
        }
    }

    /// The stored text of a typed row ([`SettingKind::is_text`]) as the
    /// prompt should pre-fill it — `""` for an unset row, never the `auto`
    /// the overlay shows in its place. Empty for a row that is not typed.
    pub fn text_value(&self, kind: SettingKind) -> String {
        match kind {
            SettingKind::WorktreeBaseBranch => self.worktree_base_branch.clone(),
            _ => String::new(),
        }
    }

    /// Write a typed value into a text row ([`SettingKind::is_text`]),
    /// trimmed. Empty puts the row back on its default (`auto`). False for
    /// a row that is not typed — nothing changes.
    pub fn set_text(&mut self, kind: SettingKind, value: &str) -> bool {
        match kind {
            SettingKind::WorktreeBaseBranch => {
                self.worktree_base_branch = value.trim().to_string();
                true
            }
            _ => false,
        }
    }
}

/// What the TUI plays for a status edge — the `done_sound` or
/// `feedback_sound` SETTING resolved against where the TUI is running.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Sound {
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
    pub fn done_sound(&self) -> Option<Sound> {
        resolve_sound(
            &self.done_sound,
            nebula_core::host::is_remote_session(),
            cfg!(target_os = "macos"),
        )
    }

    /// The sound to play when a turn stops to ask the user, or `None` for
    /// silence — which also stands down the desktop notification, since
    /// `feedback_sound` is the one switch for both. Same fallbacks as
    /// [`Config::done_sound`].
    pub fn feedback_sound(&self) -> Option<Sound> {
        resolve_sound(
            &self.feedback_sound,
            nebula_core::host::is_remote_session(),
            cfg!(target_os = "macos"),
        )
    }
}

fn resolve_sound(configured: &str, remote: bool, macos: bool) -> Option<Sound> {
    let name = configured.trim();
    if name.is_empty() || name.eq_ignore_ascii_case("off") {
        return None;
    }
    if name.eq_ignore_ascii_case("bell") || remote || !macos {
        return Some(Sound::Bell);
    }
    // A sound name is a bare file stem; anything else (a path, a dot) is
    // not one, and the bell covers the typo.
    if !name.chars().all(|c| c.is_ascii_alphanumeric()) {
        return Some(Sound::Bell);
    }
    let path = Path::new(MACOS_SOUNDS_DIR).join(format!("{name}.aiff"));
    if path.is_file() {
        Some(Sound::File(path))
    } else {
        Some(Sound::Bell)
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

/// Whether `kind`'s CLI resolves on this process's PATH right now. A fast
/// synchronous check for picker filtering only; the daemon re-probes
/// through the login shell at launch, which can see shims PATH misses.
/// Whether `program` resolves on this process's PATH right now — the
/// fast check behind `hide_uninstalled_harnesses`, for built-ins and
/// customs alike.
pub fn program_installed(program: &str) -> bool {
    let program = program.trim();
    if program.is_empty() {
        return false;
    }
    let Some(paths) = std::env::var_os("PATH") else {
        return false;
    };
    for dir in std::env::split_paths(&paths) {
        let candidate = dir.join(program);
        if candidate.is_file() {
            return true;
        }
    }
    false
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

/// The two settings layers merged, `local` over `path`. What this build
/// can't read is logged and remembered in [`Config::skipped`], so a save
/// leaves it as stored.
fn load_layers(path: &Path, local: &Path) -> Config {
    let loaded = nebula_core::settings::load::<Config>(path, local);
    for problem in &loaded.problems {
        tracing::warn!("{problem}");
    }
    if !loaded.skipped.is_empty() {
        tracing::warn!(keys = ?loaded.skipped, "settings this build can't read keep their defaults");
    }
    Config {
        skipped: loaded.skipped,
        ..loaded.value
    }
}

/// Test shorthand: the settings at `path`, local layer beside it.
#[cfg(test)]
fn load_from(path: &Path) -> Config {
    load_layers(path, &sibling_local_path(path))
}

/// The object a settings file holds, to patch keys into: empty when the
/// file is missing or isn't a JSON object (the save replaces it, as it
/// always has), an error only when the file is there and can't be read.
fn patchable_object(path: &Path) -> std::io::Result<nebula_core::settings::Object> {
    match std::fs::read_to_string(path) {
        Ok(raw) => Ok(match serde_json::from_str(&raw) {
            Ok(serde_json::Value::Object(obj)) => obj,
            _ => nebula_core::settings::Object::new(),
        }),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(Default::default()),
        Err(err) => Err(err),
    }
}

fn invalid_data(err: serde_json::Error) -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::InvalidData, err)
}

/// `config.local.json` beside `path`: the local layer of a settings file
/// that is not this nebula's own — a test's, or [`Config::save_to`]'s.
fn sibling_local_path(path: &Path) -> PathBuf {
    path.with_file_name("config.local.json")
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

/// The local layer. A test's path override moves it too, beside the
/// overriding file, so no test reads the dev's own.
fn local_settings_path() -> PathBuf {
    #[cfg(test)]
    {
        if let Some(path) = CONFIG_PATH_OVERRIDE.with(|p| p.borrow().clone()) {
            return sibling_local_path(&path);
        }
    }
    nebula_core::paths::config_local_path()
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

    /// Config files earlier releases wrote, every value off its default.
    /// Add one per release; see [`config_files_from_earlier_releases_still_load_every_key`].
    const CONFIG_FIXTURES: &[(&str, &str)] = &[
        (
            "0.26.0",
            include_str!("../../nebula-core/fixtures/config-0.26.0.json"),
        ),
        (
            "0.27.0",
            include_str!("../../nebula-core/fixtures/config-0.27.0.json"),
        ),
        (
            "0.28.0",
            include_str!("../../nebula-core/fixtures/config-0.28.0.json"),
        ),
        (
            "0.29.0",
            include_str!("../../nebula-core/fixtures/config-0.29.0.json"),
        ),
        (
            "0.30.0",
            include_str!("../../nebula-core/fixtures/config-0.30.0.json"),
        ),
        (
            "0.31.0",
            include_str!("../../nebula-core/fixtures/config-0.31.0.json"),
        ),
        (
            "0.32.0",
            include_str!("../../nebula-core/fixtures/config-0.32.0.json"),
        ),
        (
            "0.33.0",
            include_str!("../../nebula-core/fixtures/config-0.33.0.json"),
        ),
        (
            "0.34.0",
            include_str!("../../nebula-core/fixtures/config-0.34.0.json"),
        ),
        (
            "0.35.0",
            include_str!("../../nebula-core/fixtures/config-0.35.0.json"),
        ),
        (
            "0.36.0",
            include_str!("../../nebula-core/fixtures/config-0.36.0.json"),
        ),
    ];

    fn read_json_file(path: &Path) -> serde_json::Value {
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
    }

    /// Run `f` with the config pinned to an empty temp file, so every
    /// registry read (Agents rows, choice lists, tab lengths) sees a
    /// fresh install — never the dev's own file.
    fn with_empty_config<T>(f: impl FnOnce() -> T) -> T {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        std::fs::write(&path, "{}").unwrap();
        with_config_path(path, f)
    }

    /// Where an Agents harness row lives in `cfg`'s own rows, without
    /// loading anything: [`locate_agent`] reads the live file, which a
    /// test's in-memory config may have left behind.
    fn locate_in(cfg: &Config, id: &str, field: HarnessField) -> Option<(usize, usize)> {
        let tab = agents_tab();
        cfg.agent_rows()
            .iter()
            .position(|(row_id, row_field)| row_id == id && *row_field == field)
            .map(|i| (tab, AGENTS_HEAD.len() + i))
    }

    /// The first compatibility rule in docs/configuration.md: a key, once
    /// shipped, keeps its name, its type and its meaning. Every key a
    /// release wrote must still load to exactly the value it wrote — a
    /// rename leaves the key unknown, a type change leaves it unreadable,
    /// and either reads back as something else here.
    #[test]
    fn config_files_from_earlier_releases_still_load_every_key() {
        for (release, raw) in CONFIG_FIXTURES {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("config.json");
            std::fs::write(&path, raw).unwrap();
            let loaded = load_from(&path);
            assert!(
                loaded.skipped.is_empty(),
                "{release}: keys this build can't read: {:?}",
                loaded.skipped
            );
            let known = serde_json::to_value(&loaded).unwrap();
            let fixture: serde_json::Value = serde_json::from_str(raw).unwrap();
            for (key, value) in fixture.as_object().unwrap() {
                assert_eq!(
                    known.get(key),
                    Some(value),
                    "{release}: `{key}` no longer loads as that release wrote it"
                );
            }
            // A save writes every one of them back unchanged.
            loaded.save_to(&path).unwrap();
            let saved = read_json_file(&path);
            for (key, value) in fixture.as_object().unwrap() {
                assert_eq!(
                    saved.get(key),
                    Some(value),
                    "{release}: `{key}` after a save"
                );
            }
        }
    }

    /// A value this build can't read — a newer nebula's, most likely — costs
    /// only its own key, and a save leaves it as stored until the setting is
    /// changed here.
    #[test]
    fn an_unreadable_key_keeps_the_rest_and_outlives_a_save() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        std::fs::write(
            &path,
            r#"{"theme": "ocean", "recent_prompts_count": "auto", "animations": false}"#,
        )
        .unwrap();
        let mut cfg = load_from(&path);
        assert_eq!(cfg.theme, "ocean");
        assert!(!cfg.animations);
        assert_eq!(cfg.recent_prompts_count, DEFAULT_RECENT_PROMPTS_COUNT);
        assert_eq!(
            cfg.skipped,
            BTreeSet::from(["recent_prompts_count".to_string()])
        );

        cfg.black_background = false;
        cfg.save_to(&path).unwrap();
        let saved = read_json_file(&path);
        assert_eq!(saved["recent_prompts_count"], "auto", "left as stored");
        assert_eq!(saved["black_background"], false);
        assert_eq!(saved["theme"], "ocean");

        let (t, r) = locate(SettingKind::RecentPromptsCount).unwrap();
        cfg.cycle(t, r, 1);
        cfg.save_to(&path).unwrap();
        assert_eq!(
            read_json_file(&path)["recent_prompts_count"],
            4,
            "changing it here is a real edit"
        );
    }

    /// `config.local.json` wins key by key, and a key it holds is saved back
    /// into it — never copied into the portable `config.json`.
    #[test]
    fn the_local_layer_overrides_and_keeps_its_own_keys() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        let local = dir.path().join("config.local.json");
        std::fs::write(&path, r#"{"editor": "nvim", "theme": "ocean"}"#).unwrap();
        std::fs::write(&local, r#"{"editor": "nano"}"#).unwrap();
        let mut cfg = load_from(&path);
        assert_eq!(cfg.editor, "nano");
        assert_eq!(cfg.theme, "ocean");

        cfg.theme = "forest".into();
        cfg.save_to(&path).unwrap();
        assert_eq!(read_json_file(&path)["theme"], "forest");
        assert_eq!(
            read_json_file(&path)["editor"],
            "nvim",
            "the local value stays local"
        );
        assert_eq!(
            read_json_file(&local),
            serde_json::json!({"editor": "nano"})
        );

        cfg.editor = "hx".into();
        cfg.save_to(&path).unwrap();
        assert_eq!(read_json_file(&local)["editor"], "hx");
        assert_eq!(read_json_file(&path)["editor"], "nvim");
    }

    #[test]
    fn reset_removes_the_local_layer_too() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        let local = dir.path().join("config.local.json");
        with_config_path(path.clone(), || {
            std::fs::write(&local, r#"{"theme": "rose"}"#).unwrap();
            assert_eq!(Config::load().theme, "rose");
            Config::reset_to_defaults().unwrap();
            assert!(!local.exists());
            assert_eq!(Config::load().theme, Config::default().theme);
        });
    }

    #[test]
    fn ssh_sync_defaults_on_and_toggles_from_the_general_tab() {
        assert!(Config::default().ssh_sync_config);
        let cfg: Config = serde_json::from_str("{}").unwrap();
        assert!(cfg.ssh_sync_config);
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        let mut cfg = Config::default();
        let (t, r) = locate(SettingKind::SshSyncConfig).unwrap();
        assert_eq!(SETTINGS_TABS[t].title, "General");
        cfg.cycle(t, r, 0);
        assert_eq!(cfg.value_label(SettingKind::SshSyncConfig), "off");
        cfg.save_to(&path).unwrap();
        assert_eq!(read_json_file(&path)["ssh_sync_config"], false);
        assert!(!load_from(&path).ssh_sync_config);
    }

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

    /// The two prewarm keys are daemon-owned but overlay-toggled, like
    /// `git_init_on_create`: on by default, a missing key reads as on, and
    /// the Sessions-tab rows round-trip through the saved file.
    #[test]
    fn prewarm_toggles_default_on_and_round_trip() {
        let cfg = Config::default();
        assert!(cfg.prewarm_agents && cfg.prewarm_sessions);
        let cfg: Config = serde_json::from_str("{}").unwrap();
        assert!(cfg.prewarm_agents && cfg.prewarm_sessions);
        let cfg: Config =
            serde_json::from_str(r#"{"prewarm_agents": false, "prewarm_sessions": false}"#)
                .unwrap();
        assert!(!cfg.prewarm_agents && !cfg.prewarm_sessions);

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        let mut cfg = Config::default();
        let (t, r) = locate(SettingKind::PrewarmAgents).unwrap();
        assert_eq!(SETTINGS_TABS[t].title, "Sessions");
        cfg.cycle(t, r, 0);
        let (t, r) = locate(SettingKind::PrewarmSessions).unwrap();
        assert_eq!(SETTINGS_TABS[t].title, "Sessions");
        cfg.cycle(t, r, 0);
        assert_eq!(cfg.value_label(SettingKind::PrewarmAgents), "off");
        assert_eq!(cfg.value_label(SettingKind::PrewarmSessions), "off");
        cfg.save_to(&path).unwrap();
        let saved: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        // Written under the daemon's own key names, since it is the reader.
        assert_eq!(saved["prewarm_agents"], false);
        assert_eq!(saved["prewarm_sessions"], false);
        let loaded = load_from(&path);
        assert!(!loaded.prewarm_agents && !loaded.prewarm_sessions);
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
            root["hand_added_key"] = serde_json::json!(false);
            std::fs::write(&path, serde_json::to_vec_pretty(&root).unwrap()).unwrap();
            Config::load().save().unwrap();
            let raw = std::fs::read_to_string(&path).unwrap();
            assert!(
                raw.contains("hand_added_key"),
                "save() patches, keeping foreign keys:\n{raw}"
            );

            // …but not a reset: the file starts over from an empty object.
            let reset = Config::reset_to_defaults().unwrap();
            assert!(reset.animations);
            assert!(reset.keybindings.is_empty());
            let raw = std::fs::read_to_string(&path).unwrap();
            assert!(
                !raw.contains("hand_added_key"),
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

    /// `n` always launches straight from the picker now, so **Skip
    /// starting prompt** has no row to be edited on — but the key an
    /// earlier release wrote still loads, and is written back unchanged
    /// for the older builds that read it.
    #[test]
    fn skip_session_naming_has_no_row_and_is_written_back_for_older_builds() {
        assert!(SETTINGS_TABS.iter().all(|tab| match &tab.body {
            TabBody::Values(rows) | TabBody::Project(rows) => {
                rows.iter().all(|row| row.label != "Skip starting prompt")
            }
            TabBody::Hotkeys | TabBody::Agents => true,
        }));

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        std::fs::write(&path, r#"{"skip_session_naming": true}"#).unwrap();
        let mut cfg = load_from(&path);
        assert!(cfg.skipped.is_empty(), "{:?}", cfg.skipped);
        assert!(cfg.skip_session_naming);

        cfg.black_background = false;
        cfg.save_to(&path).unwrap();
        assert_eq!(read_json_file(&path)["skip_session_naming"], true);
    }

    /// Retired with the archive confirm made unconditional: no tab shows
    /// the row, and a `true` an earlier release wrote still loads and is
    /// written back unchanged for the older builds that read it.
    #[test]
    fn confirm_on_archive_has_no_row_and_is_written_back_for_older_builds() {
        assert!(SETTINGS_TABS.iter().all(|tab| match &tab.body {
            TabBody::Values(rows) | TabBody::Project(rows) => {
                rows.iter().all(|row| row.label != "Confirm on archive")
            }
            TabBody::Hotkeys | TabBody::Agents => true,
        }));
        assert!(!Config::default().confirm_on_archive);
        let cfg: Config = serde_json::from_str("{}").unwrap();
        assert!(!cfg.confirm_on_archive);

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        std::fs::write(&path, r#"{"confirm_on_archive": true}"#).unwrap();
        let cfg = load_from(&path);
        assert!(cfg.skipped.is_empty(), "{:?}", cfg.skipped);
        assert!(cfg.confirm_on_archive);

        cfg.save_to(&path).unwrap();
        assert_eq!(read_json_file(&path)["confirm_on_archive"], true);
    }

    #[test]
    fn worktree_base_branch_is_a_typed_row_that_defaults_to_auto() {
        assert_eq!(Config::default().worktree_base_branch, "");
        let cfg: Config = serde_json::from_str("{}").unwrap();
        assert_eq!(cfg.worktree_base_branch, "");
        assert_eq!(
            cfg.value_label(SettingKind::WorktreeBaseBranch),
            AUTO_CHOICE
        );
        assert!(SettingKind::WorktreeBaseBranch.is_text());

        let (tab, row) = locate(SettingKind::WorktreeBaseBranch).unwrap();
        assert_eq!(
            SETTINGS_TABS[tab].title, "General",
            "sits beside git init new projects"
        );
        // Enter / ←/→ on a typed row change nothing; the prompt does.
        let mut cfg = Config::default();
        for delta in [0, 1, -1] {
            cfg.cycle(tab, row, delta);
            assert_eq!(cfg.worktree_base_branch, "");
        }

        assert_eq!(cfg.text_value(SettingKind::WorktreeBaseBranch), "");
        assert!(cfg.set_text(SettingKind::WorktreeBaseBranch, "  master "));
        assert_eq!(cfg.worktree_base_branch, "master", "trimmed");
        assert_eq!(cfg.value_label(SettingKind::WorktreeBaseBranch), "master");
        assert_eq!(cfg.text_value(SettingKind::WorktreeBaseBranch), "master");
        assert_eq!(
            spec_for(SettingKind::WorktreeBaseBranch).map(|s| s.label),
            Some("Worktree base branch")
        );
        assert!(
            !cfg.set_text(SettingKind::Editor, "nvim"),
            "a cycled row is not a typed one"
        );
        assert_eq!(cfg.editor, "vim");

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        cfg.save_to(&path).unwrap();
        let saved: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(saved["worktree_base_branch"], "master");
        assert_eq!(load_from(&path).worktree_base_branch, "master");

        // Empty is the way back to auto, and is stored as "", not "auto".
        assert!(cfg.set_text(SettingKind::WorktreeBaseBranch, "   "));
        cfg.save_to(&path).unwrap();
        let saved: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(saved["worktree_base_branch"], "");
        assert_eq!(
            load_from(&path).value_label(SettingKind::WorktreeBaseBranch),
            AUTO_CHOICE
        );
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
        assert_eq!(resolve_sound("off", false, true), None);
        assert_eq!(resolve_sound("OFF", false, true), None);
        assert_eq!(resolve_sound("", false, true), None);
        assert_eq!(resolve_sound("bell", false, true), Some(Sound::Bell));
        assert_eq!(
            resolve_sound("Glass", true, true),
            Some(Sound::Bell),
            "over ssh afplay would ring the remote box"
        );
        assert_eq!(
            resolve_sound("Glass", false, false),
            Some(Sound::Bell),
            "no system sounds off macOS"
        );
        assert_eq!(resolve_sound("NoSuchSound", false, true), Some(Sound::Bell));
        assert_eq!(
            resolve_sound("../etc/passwd", false, true),
            Some(Sound::Bell)
        );
        #[cfg(target_os = "macos")]
        assert_eq!(
            resolve_sound("Glass", false, true),
            Some(Sound::File(Path::new(MACOS_SOUNDS_DIR).join("Glass.aiff")))
        );
    }

    #[test]
    fn feedback_sound_defaults_to_sosumi_cycles_persists_and_resolves() {
        let mut cfg = Config::default();
        assert_eq!(cfg.feedback_sound, "Sosumi");
        assert_ne!(
            cfg.feedback_sound, cfg.done_sound,
            "red and green must sound different"
        );
        // A config predating the key rings too.
        let old: Config = serde_json::from_str("{}").unwrap();
        assert_eq!(old.feedback_sound, "Sosumi");
        // …and one that only ever set the done sound keeps it.
        let old: Config = serde_json::from_str(r#"{"done_sound": "Ping"}"#).unwrap();
        assert_eq!(old.done_sound, "Ping");
        assert_eq!(old.feedback_sound, "Sosumi");

        // Its row sits right after the done sound on the Sessions tab.
        let (tab, row) = locate(SettingKind::FeedbackSound).unwrap();
        assert_eq!(locate(SettingKind::DoneSound).unwrap(), (tab, row - 1));
        assert_eq!(SETTINGS_TABS[tab].title, "Sessions");
        cfg.cycle(tab, row, 1);
        assert_eq!(cfg.feedback_sound, "Basso");
        cfg.cycle(tab, row, 1);
        assert_eq!(cfg.feedback_sound, "off", "the list wraps");
        cfg.cycle(tab, row, -1);
        cfg.cycle(tab, row, -1);
        assert_eq!(cfg.feedback_sound, "Sosumi");
        assert_eq!(cfg.value_label(SettingKind::FeedbackSound), "Sosumi");
        assert_eq!(cfg.done_sound, "Glass", "the done sound is its own row");

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        cfg.feedback_sound = "off".into();
        cfg.save_to(&path).unwrap();
        let loaded = load_from(&path);
        assert_eq!(loaded.feedback_sound, "off");
        assert_eq!(loaded.feedback_sound(), None, "off is silence for both");
        assert_eq!(loaded.done_sound, "Glass");

        // The same resolution as the done sound: the bell over ssh and off
        // macOS, silence for off.
        assert_eq!(resolve_sound("Sosumi", true, true), Some(Sound::Bell));
        assert_eq!(resolve_sound("Sosumi", false, false), Some(Sound::Bell));
        #[cfg(target_os = "macos")]
        assert_eq!(
            resolve_sound("Sosumi", false, true),
            Some(Sound::File(Path::new(MACOS_SOUNDS_DIR).join("Sosumi.aiff")))
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

    /// Workspaces are gone, so **Workspaces bar** has no row to be edited
    /// on — but the key an earlier release wrote still loads, and is
    /// written back unchanged for the older builds that read it.
    #[test]
    fn show_workspaces_has_no_row_and_is_written_back_for_older_builds() {
        assert!(SETTINGS_TABS.iter().all(|tab| match &tab.body {
            TabBody::Values(rows) | TabBody::Project(rows) => {
                rows.iter().all(|row| row.label != "Workspaces bar")
            }
            TabBody::Hotkeys | TabBody::Agents => true,
        }));

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        std::fs::write(&path, r#"{"show_workspaces": false}"#).unwrap();
        let mut cfg = load_from(&path);
        assert!(cfg.skipped.is_empty(), "{:?}", cfg.skipped);
        assert!(!cfg.show_workspaces);

        cfg.black_background = false;
        cfg.save_to(&path).unwrap();
        assert_eq!(read_json_file(&path)["show_workspaces"], false);
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

    /// DRAFT PULL REQUESTS: an Appearance row that reads `shown` / `hidden`
    /// like the panel rows beside it, shown by default so a config that
    /// predates the key keeps every draft on screen, and persisted under
    /// `hide_draft_prs`.
    #[test]
    fn draft_pull_requests_default_shown_toggle_on_the_appearance_tab_and_persist() {
        let mut cfg = Config::default();
        assert!(!cfg.hide_draft_prs, "drafts stay on screen until asked");
        assert_eq!(cfg.value_label(SettingKind::HideDraftPrs), "shown");

        let (tab, row) = locate(SettingKind::HideDraftPrs).unwrap();
        assert_eq!(SETTINGS_TABS[tab].title, "Appearance");
        cfg.cycle(tab, row, 0);
        assert!(cfg.hide_draft_prs);
        assert_eq!(cfg.value_label(SettingKind::HideDraftPrs), "hidden");
        cfg.cycle(tab, row, 1);
        assert!(!cfg.hide_draft_prs, "←/→ toggle a bool like Enter does");
        cfg.cycle(tab, row, 0);

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        cfg.save_to(&path).unwrap();
        let raw = std::fs::read_to_string(&path).unwrap();
        assert!(raw.contains(r#""hide_draft_prs": true"#), "{raw}");
        assert!(load_from(&path).hide_draft_prs);

        let legacy: Config = serde_json::from_str("{}").unwrap();
        assert!(!legacy.hide_draft_prs);
    }

    /// CARD LINE COUNTS: an Appearance row, off by default so a config that
    /// predates the key keeps its cards as they were, persisted under
    /// `card_line_changes`.
    #[test]
    fn card_line_counts_default_off_toggle_on_the_appearance_tab_and_persist() {
        let mut cfg = Config::default();
        assert!(
            !cfg.card_line_changes,
            "cards count files alone until asked"
        );
        assert_eq!(cfg.value_label(SettingKind::CardLineChanges), "off");

        let (tab, row) = locate(SettingKind::CardLineChanges).unwrap();
        assert_eq!(SETTINGS_TABS[tab].title, "Appearance");
        cfg.cycle(tab, row, 0);
        assert!(cfg.card_line_changes);
        assert_eq!(cfg.value_label(SettingKind::CardLineChanges), "on");

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        cfg.save_to(&path).unwrap();
        let raw = std::fs::read_to_string(&path).unwrap();
        assert!(raw.contains(r#""card_line_changes": true"#), "{raw}");
        assert!(load_from(&path).card_line_changes);

        let legacy: Config = serde_json::from_str("{}").unwrap();
        assert!(!legacy.card_line_changes);
    }

    /// The tint is always on now, so **Focused panel tint** has no row to
    /// be edited on — but the key an earlier release wrote still loads,
    /// and is written back unchanged for the older builds that read it.
    #[test]
    fn focus_tint_has_no_row_and_is_written_back_for_older_builds() {
        assert!(SETTINGS_TABS.iter().all(|tab| match &tab.body {
            TabBody::Values(rows) | TabBody::Project(rows) => {
                rows.iter().all(|row| row.label != "Focused panel tint")
            }
            TabBody::Hotkeys | TabBody::Agents => true,
        }));

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        std::fs::write(&path, r#"{"focus_tint": false}"#).unwrap();
        let mut cfg = load_from(&path);
        assert!(cfg.skipped.is_empty(), "{:?}", cfg.skipped);
        assert!(!cfg.focus_tint);

        cfg.black_background = false;
        cfg.save_to(&path).unwrap();
        assert_eq!(read_json_file(&path)["focus_tint"], false);
    }

    /// The BLACK BACKGROUND: on out of the box, toggled off from its
    /// Appearance row (the terminal's own background shows), persisted
    /// under `black_background`; a config predating the key turns it on,
    /// and a saved `false` is honoured.
    #[test]
    fn black_background_default_on_toggle_and_persist() {
        let mut cfg = Config::default();
        assert!(cfg.black_background);
        let (tab, row) = locate(SettingKind::BlackBackground).unwrap();
        assert_eq!(SETTINGS_TABS[tab].title, "Appearance");
        cfg.cycle(tab, row, 0);
        assert!(!cfg.black_background);

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        cfg.save_to(&path).unwrap();
        assert!(!load_from(&path).black_background);
        let raw: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(raw.get("black_background"), Some(&serde_json::json!(false)));

        let older: Config = serde_json::from_str("{}").unwrap();
        assert!(
            older.black_background,
            "a config predating the key turns it on"
        );
    }

    /// The **Session pane**: down the right out of the box, cycled from
    /// its Appearance row to the bottom and back, persisted under
    /// `session_pane`. A config predating the key, or holding a word off
    /// the list — `left`, which older builds offered, included — reads as
    /// the right.
    #[test]
    fn session_pane_defaults_to_the_right_cycles_and_persists() {
        use crate::launcher::PaneSide;
        let mut cfg = Config::default();
        assert_eq!(cfg.pane_side(), PaneSide::Right);
        let (tab, row) = locate(SettingKind::SessionPane).unwrap();
        assert_eq!(SETTINGS_TABS[tab].title, "Appearance");
        assert_eq!(cfg.value_label(SettingKind::SessionPane), "right");
        cfg.cycle(tab, row, 0);
        assert_eq!(cfg.pane_side(), PaneSide::Bottom);
        assert_eq!(cfg.value_label(SettingKind::SessionPane), "bottom");
        cfg.cycle(tab, row, 1);
        assert_eq!(cfg.pane_side(), PaneSide::Right, "and round again");
        cfg.cycle(tab, row, -1);
        assert_eq!(cfg.pane_side(), PaneSide::Bottom, "either arrow walks it");

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        cfg.save_to(&path).unwrap();
        assert_eq!(load_from(&path).pane_side(), PaneSide::Bottom);
        let raw: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(raw.get("session_pane"), Some(&serde_json::json!("bottom")));

        let older: Config = serde_json::from_str("{}").unwrap();
        assert_eq!(older.pane_side(), PaneSide::Right, "predating the key");
        let mut odd: Config = serde_json::from_str(r#"{"session_pane": "top"}"#).unwrap();
        assert_eq!(odd.pane_side(), PaneSide::Right, "a word off the list");
        odd.cycle(tab, row, 0);
        assert_eq!(odd.pane_side(), PaneSide::Bottom, "steps on from the right");
        let left: Config = serde_json::from_str(r#"{"session_pane": "left"}"#).unwrap();
        assert_eq!(left.pane_side(), PaneSide::Right, "the retired left side");
    }

    /// The QUICK PROMPT's focus toggle: off unless the user turns it on,
    /// and persisted under its own key (a missed `obj.insert` would let the
    /// row toggle on screen and read back off on the next launch).
    #[test]
    fn quick_prompt_focus_toggles_off_by_default_and_persists() {
        let mut cfg = Config::default();
        assert!(
            !cfg.quick_prompt_focus,
            "a quick prompt stays out of the way"
        );
        assert_eq!(cfg.value_label(SettingKind::QuickPromptFocus), "off");

        let (tab, row) = locate(SettingKind::QuickPromptFocus).unwrap();
        cfg.cycle(tab, row, 0);
        assert!(cfg.quick_prompt_focus);
        assert_eq!(cfg.value_label(SettingKind::QuickPromptFocus), "on");

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        cfg.save_to(&path).unwrap();
        assert!(load_from(&path).quick_prompt_focus);

        // A config predating the key reads as off.
        let cfg: Config = serde_json::from_str("{}").unwrap();
        assert!(!cfg.quick_prompt_focus);
    }

    /// **Run command** on the Project tab: a typed row (Enter prompts,
    /// ←/→ and cycle change nothing) kept per repo path as `run_command`,
    /// shown as `.nebula.json` while empty — the file decides then — and
    /// left out of the file while empty, so an entry with nothing else in
    /// it is dropped rather than written.
    #[test]
    fn run_command_is_a_typed_project_row_kept_per_repo_path() {
        let demo = Path::new("/tmp/demo");
        let other = Path::new("/tmp/other");
        let mut cfg = Config::default();
        assert!(SettingKind::RunCommand.is_text());
        assert!(SettingKind::RunCommand.is_project());
        let (tab, row) = locate(SettingKind::RunCommand).unwrap();
        assert_eq!(tab, project_tab());
        assert_eq!(row, 0, "the first row of the tab");
        assert_eq!(cfg.project(demo).run_command, "");
        assert_eq!(
            cfg.project(demo).value_label(SettingKind::RunCommand),
            PROJECT_FILE_CHOICE
        );
        assert_eq!(cfg.value_label(SettingKind::RunCommand), ".nebula.json");
        assert_eq!(cfg.project_text_value(demo, SettingKind::RunCommand), "");

        // The tab's cycle never touches a typed row.
        for delta in [0, 1, -1] {
            cfg.cycle(tab, row, delta);
        }
        assert!(cfg.projects.is_empty());
        assert!(
            !cfg.set_text(SettingKind::RunCommand, "npm run dev"),
            "not a top-level row"
        );
        assert!(
            !cfg.set_project_text(demo, SettingKind::Theme, "dark"),
            "not a project row"
        );
        assert!(cfg.projects.is_empty());

        assert!(cfg.set_project_text(demo, SettingKind::RunCommand, "  npm run dev "));
        assert_eq!(cfg.project(demo).run_command, "npm run dev", "trimmed");
        assert_eq!(
            cfg.project(demo).value_label(SettingKind::RunCommand),
            "npm run dev"
        );
        assert_eq!(
            cfg.project_text_value(demo, SettingKind::RunCommand),
            "npm run dev"
        );
        assert_eq!(
            cfg.project(other).value_label(SettingKind::RunCommand),
            PROJECT_FILE_CHOICE,
            "one project's, not every project's"
        );
        assert_eq!(
            cfg.project(demo).open_command,
            "",
            "open's key is untouched"
        );

        // Persisted in the project's entry, under its path, beside no key
        // it did not set.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        assert!(cfg.set_project_text(other, SettingKind::OpenCommand, "open x"));
        cfg.save_to(&path).unwrap();
        assert_eq!(
            read_json_file(&path)["projects"],
            serde_json::json!({
                "/tmp/demo": { "run_command": "npm run dev" },
                "/tmp/other": { "open_command": "open x" },
            })
        );
        let loaded = load_from(&path);
        assert_eq!(loaded.project(demo).run_command, "npm run dev");
        assert_eq!(loaded.project(other).run_command, "");

        // Empty is the way back to the file — and drops the entry.
        assert!(cfg.set_project_text(demo, SettingKind::RunCommand, "   "));
        assert_eq!(cfg.projects.keys().collect::<Vec<_>>(), [other]);
        cfg.save_to(&path).unwrap();
        assert_eq!(
            read_json_file(&path)["projects"],
            serde_json::json!({ "/tmp/other": { "open_command": "open x" } })
        );
    }

    /// **Open command** on the Project tab: the same typed per-project row
    /// as Run command, right under it, kept as `open_command` — what
    /// `Shift+Enter` / `Shift+O` runs before looking at `.nebula.json`.
    /// Its own key, so setting it leaves `run_command` alone; empty drops
    /// it from the entry.
    #[test]
    fn open_command_is_a_typed_project_row_under_run_command() {
        let demo = Path::new("/tmp/demo");
        let mut cfg = Config::default();
        assert!(SettingKind::OpenCommand.is_text());
        assert!(SettingKind::OpenCommand.is_project());
        let (tab, row) = locate(SettingKind::OpenCommand).unwrap();
        assert_eq!(tab, project_tab());
        assert_eq!(
            row,
            locate(SettingKind::RunCommand).unwrap().1 + 1,
            "right under Run command"
        );
        assert_eq!(cfg.project(demo).open_command, "");
        assert_eq!(
            cfg.project(demo).value_label(SettingKind::OpenCommand),
            PROJECT_FILE_CHOICE
        );
        assert_eq!(cfg.value_label(SettingKind::OpenCommand), ".nebula.json");

        // A typed row: cycling its tab row changes nothing, and the
        // top-level setter is not its.
        cfg.cycle(tab, row, 0);
        assert!(cfg.projects.is_empty());
        assert!(!cfg.set_text(SettingKind::OpenCommand, "open x"));

        assert!(cfg.set_project_text(
            demo,
            SettingKind::OpenCommand,
            "  open http://localhost:3000 "
        ));
        assert_eq!(
            cfg.project(demo).open_command,
            "open http://localhost:3000",
            "trimmed"
        );
        assert_eq!(cfg.project(demo).run_command, "", "run's key is untouched");
        assert_eq!(
            cfg.project_text_value(demo, SettingKind::OpenCommand),
            "open http://localhost:3000"
        );

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        cfg.save_to(&path).unwrap();
        assert_eq!(
            read_json_file(&path)["projects"],
            serde_json::json!({
                "/tmp/demo": { "open_command": "open http://localhost:3000" },
            })
        );
        assert_eq!(
            load_from(&path).project(demo).open_command,
            "open http://localhost:3000"
        );

        // Empty is the way back to the file — and drops the entry.
        assert!(cfg.set_project_text(demo, SettingKind::OpenCommand, " "));
        assert!(cfg.projects.is_empty());
    }

    /// The root is always listed now, so **Hide root worktree** has no row
    /// to be edited on — but the keys earlier releases wrote still load
    /// and are written back unchanged for the older builds that read
    /// them: the top-level `hide_root_worktree` (one switch for every
    /// project, through 0.27) as a retired field of its own, and the one
    /// in a project's entry (the Project tab's row, through 0.33) as a key
    /// this build doesn't know — which also keeps that entry from being
    /// dropped as all-default. Neither hides anything.
    #[test]
    fn hide_root_worktree_has_no_row_and_is_written_back_for_older_builds() {
        assert!(SETTINGS_TABS.iter().all(|tab| match &tab.body {
            TabBody::Values(rows) | TabBody::Project(rows) => {
                rows.iter().all(|row| row.label != "Hide root worktree")
            }
            TabBody::Hotkeys | TabBody::Agents => true,
        }));

        let demo = Path::new("/tmp/demo");
        let other = Path::new("/tmp/other");
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        std::fs::write(
            &path,
            r#"{
              "hide_root_worktree": true,
              "projects": {
                "/tmp/other": { "hide_root_worktree": true, "future_row": "x" }
              }
            }"#,
        )
        .unwrap();
        let mut cfg = load_from(&path);
        assert!(cfg.skipped.is_empty(), "{:?}", cfg.skipped);
        assert!(cfg.hide_root_worktree);
        assert_eq!(
            cfg.project(demo),
            ProjectSettings::default(),
            "no entry: the defaults, whatever the old global key says"
        );
        assert_eq!(
            cfg.project(other).other.get("hide_root_worktree"),
            Some(&serde_json::json!(true)),
            "an entry's key rides along unread"
        );

        // A save writes both back; the entry that only carries the old
        // key is kept for it, not dropped as all-default.
        let kept = cfg.project(other);
        cfg.set_project(other, kept);
        cfg.save_to(&path).unwrap();
        let saved = read_json_file(&path);
        assert_eq!(
            saved["hide_root_worktree"], true,
            "written back for older builds"
        );
        assert_eq!(
            saved["projects"],
            serde_json::json!({
                "/tmp/other": { "hide_root_worktree": true, "future_row": "x" }
            })
        );
    }

    /// The Project tab: one line naming the project, then its rows, every
    /// one of them a project row — and no project row anywhere else.
    #[test]
    fn the_project_tab_names_the_project_then_lists_its_rows() {
        let tab = project_tab();
        assert_eq!(SETTINGS_TABS[tab].title, "Project");
        assert!(tab < hotkeys_tab());
        let rows = settings_rows(tab);
        assert_eq!(rows[0], SettingsRow::Project);
        assert_eq!(
            rows[1..],
            (0..tab_len(tab))
                .map(SettingsRow::Setting)
                .collect::<Vec<_>>()[..]
        );
        for (t, _, spec) in all_settings() {
            assert_eq!(
                spec.kind.is_project(),
                t == tab,
                "{:?} sits on {}",
                spec.kind,
                SETTINGS_TABS[t].title
            );
        }
    }

    /// The KEY COMBO DISPLAY: an Experimental switch, off by default, a
    /// plain toggle persisted under `show_key_combos`, unknown to a config
    /// written before it (which reads as off).
    #[test]
    fn key_combo_display_is_off_by_default_on_the_experimental_tab_and_persists() {
        let mut cfg = Config::default();
        assert!(!cfg.show_key_combos, "a teaching aid nobody asked for yet");
        assert_eq!(cfg.value_label(SettingKind::ShowKeyCombos), "off");

        let (tab, row) = locate(SettingKind::ShowKeyCombos).unwrap();
        assert_eq!(SETTINGS_TABS[tab].title, "Experimental");
        assert_eq!(tab + 1, hotkeys_tab(), "Hotkeys stays last");
        cfg.cycle(tab, row, 0);
        assert!(cfg.show_key_combos);
        assert_eq!(cfg.value_label(SettingKind::ShowKeyCombos), "on");
        cfg.cycle(tab, row, 1);
        assert!(!cfg.show_key_combos, "either arrow toggles it back");
        cfg.cycle(tab, row, -1);
        assert!(cfg.show_key_combos);

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        cfg.save_to(&path).unwrap();
        assert!(load_from(&path).show_key_combos);

        let cfg: Config = serde_json::from_str("{}").unwrap();
        assert!(!cfg.show_key_combos);
    }

    /// REMEMBER HARNESS: an Experimental switch, off by default, a plain
    /// toggle persisted under `remember_harness`, unknown to a config
    /// written before it (which reads as off).
    #[test]
    fn remember_harness_is_off_by_default_on_the_experimental_tab_and_persists() {
        let mut cfg = Config::default();
        assert!(!cfg.remember_harness, "a pick is one session's by default");
        assert_eq!(cfg.value_label(SettingKind::RememberHarness), "off");
        assert_eq!(
            cfg.remembered_kind(),
            None,
            "off: the picker starts on its first row"
        );

        let (tab, row) = locate(SettingKind::RememberHarness).unwrap();
        assert_eq!(SETTINGS_TABS[tab].title, "Experimental");
        assert_eq!(tab + 1, hotkeys_tab(), "Hotkeys stays last");
        let (combo_tab, combo_row) = locate(SettingKind::ShowKeyCombos).unwrap();
        assert_eq!(
            (combo_tab, combo_row + 1),
            (tab, row),
            "the newest switch sits last"
        );
        cfg.cycle(tab, row, 0);
        assert!(cfg.remember_harness);
        assert_eq!(cfg.value_label(SettingKind::RememberHarness), "on");
        assert_eq!(cfg.remembered_kind(), Some(AgentKind::Claude));
        cfg.cycle(tab, row, 1);
        assert!(!cfg.remember_harness, "either arrow toggles it back");
        cfg.cycle(tab, row, -1);
        assert!(cfg.remember_harness);

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        cfg.save_to(&path).unwrap();
        assert!(load_from(&path).remember_harness);

        let cfg: Config = serde_json::from_str("{}").unwrap();
        assert!(!cfg.remember_harness);
    }

    /// PRESET TEXT: a Sessions row after the sounds, `prefix` by default,
    /// cycling the three sides and persisted under `preset_text`; a hand
    /// edit off the list reads as the default, and `both` as the long
    /// label.
    #[test]
    fn preset_text_is_prefix_by_default_on_the_sessions_tab_and_cycles_the_sides() {
        let mut cfg = Config::default();
        assert_eq!(
            cfg.preset_text(),
            PresetText::Prefix,
            "one box, before the task"
        );
        assert_eq!(cfg.value_label(SettingKind::PresetText), "prefix");
        assert_eq!(
            PRESET_TEXTS.to_vec(),
            PresetText::ALL.map(PresetText::as_str).to_vec(),
            "the row cycles every side, in the enum's order"
        );

        let (tab, row) = locate(SettingKind::PresetText).unwrap();
        assert_eq!(SETTINGS_TABS[tab].title, "Sessions");
        let (sound_tab, sound_row) = locate(SettingKind::FeedbackSound).unwrap();
        assert_eq!((sound_tab, sound_row + 1), (tab, row), "after the sounds");
        cfg.cycle(tab, row, 1);
        assert_eq!(cfg.preset_text(), PresetText::Postfix);
        cfg.cycle(tab, row, 1);
        assert_eq!(cfg.preset_text(), PresetText::Both);
        assert_eq!(cfg.value_label(SettingKind::PresetText), "prefix & postfix");
        cfg.cycle(tab, row, 1);
        assert_eq!(cfg.preset_text(), PresetText::Prefix, "wraps");
        cfg.cycle(tab, row, -1);
        assert_eq!(cfg.preset_text(), PresetText::Both, "← steps back");
        cfg.cycle(tab, row, 0);
        assert_eq!(cfg.preset_text(), PresetText::Prefix, "Enter steps on");

        cfg.cycle(tab, row, -1);
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        cfg.save_to(&path).unwrap();
        assert_eq!(load_from(&path).preset_text, "prefix & postfix");
        assert_eq!(load_from(&path).preset_text(), PresetText::Both);

        let cfg: Config = serde_json::from_str("{}").unwrap();
        assert_eq!(
            cfg.preset_text(),
            PresetText::Prefix,
            "unknown to an older file"
        );
        let cfg: Config = serde_json::from_str(r#"{"preset_text": "both"}"#).unwrap();
        assert_eq!(cfg.preset_text(), PresetText::Both);
        assert_eq!(cfg.value_label(SettingKind::PresetText), "prefix & postfix");
        let mut cfg: Config = serde_json::from_str(r#"{"preset_text": "suffix"}"#).unwrap();
        assert_eq!(
            cfg.preset_text(),
            PresetText::Prefix,
            "an unknown value is the default"
        );
        cfg.cycle(tab, row, 1);
        assert_eq!(
            cfg.preset_text(),
            PresetText::Postfix,
            "and cycles on from it"
        );
    }

    /// PR & ISSUE COUNTS: an Experimental switch, on by default — the one
    /// on the tab that is — a plain toggle persisted under
    /// `pr_issue_counts`, unknown to a config written before it (which
    /// reads as on). The newest switch, so it sits last on the tab, under
    /// REMEMBER HARNESS.
    #[test]
    fn pr_issue_counts_is_on_by_default_on_the_experimental_tab_and_persists() {
        let mut cfg = Config::default();
        assert!(cfg.pr_issue_counts, "the rows count out of the box");
        assert_eq!(cfg.value_label(SettingKind::PrIssueCounts), "on");

        let (tab, row) = locate(SettingKind::PrIssueCounts).unwrap();
        assert_eq!(SETTINGS_TABS[tab].title, "Experimental");
        assert_eq!(tab + 1, hotkeys_tab(), "Hotkeys stays last");
        let (harness_tab, harness_row) = locate(SettingKind::RememberHarness).unwrap();
        assert_eq!(
            (harness_tab, harness_row + 1),
            (tab, row),
            "the newest switch sits last"
        );
        cfg.cycle(tab, row, 0);
        assert!(!cfg.pr_issue_counts);
        assert_eq!(cfg.value_label(SettingKind::PrIssueCounts), "off");
        cfg.cycle(tab, row, 1);
        assert!(cfg.pr_issue_counts, "either arrow toggles it back");
        cfg.cycle(tab, row, -1);
        assert!(!cfg.pr_issue_counts);

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        cfg.save_to(&path).unwrap();
        assert!(!load_from(&path).pr_issue_counts, "off survives a save");

        let cfg: Config = serde_json::from_str("{}").unwrap();
        assert!(
            cfg.pr_issue_counts,
            "a config from before the key reads as on"
        );
    }

    /// What a remembered launch writes: the harness into the QUICK
    /// PROMPT's `quick_prompt_kind`, a picked model or effort into that
    /// harness's own rows — and nothing at all while the switch is off,
    /// or when the pick already is the default.
    #[test]
    fn remember_launch_writes_the_agents_tab_rows_only_while_on() {
        let mut cfg = Config::default();
        assert!(
            !cfg.remember_launch(AgentKind::Codex, None, Some("gpt-5.5"), Some("high")),
            "off: nothing moves"
        );
        assert_eq!(cfg.quick_prompt_kind(), AgentKind::Claude);
        assert_eq!(cfg.codex_model, DEFAULT_CHOICE);
        assert_eq!(cfg.codex_effort, DEFAULT_CHOICE);

        cfg.remember_harness = true;
        // The harness alone: the rows it did not drill into stay put.
        assert!(cfg.remember_launch(AgentKind::Codex, None, None, None));
        assert_eq!(cfg.quick_prompt_kind(), AgentKind::Codex);
        assert_eq!(cfg.remembered_kind(), Some(AgentKind::Codex));
        assert_eq!(cfg.codex_model, DEFAULT_CHOICE, "no model was picked");
        assert_eq!(cfg.codex_effort, DEFAULT_CHOICE);
        assert!(
            !cfg.remember_launch(AgentKind::Codex, None, None, None),
            "the same pick again changes nothing, so nothing is saved"
        );

        // A model and effort drilled into land on that harness's rows.
        assert!(cfg.remember_launch(AgentKind::Claude, None, Some("opus"), Some("high")));
        assert_eq!(cfg.quick_prompt_kind(), AgentKind::Claude);
        assert_eq!(
            cfg.default_model(AgentKind::Claude).as_deref(),
            Some("opus")
        );
        assert_eq!(
            cfg.default_effort(AgentKind::Claude).as_deref(),
            Some("high")
        );
        assert_eq!(
            cfg.codex_model, DEFAULT_CHOICE,
            "another harness's rows are its own"
        );
        // The explicit "default" row is a pick too: back to no flag.
        assert!(cfg.remember_launch(AgentKind::Claude, None, Some("default"), None));
        assert_eq!(cfg.claude_model, DEFAULT_CHOICE);
        assert_eq!(
            cfg.default_effort(AgentKind::Claude).as_deref(),
            Some("high")
        );
        // A blank pick is no pick.
        assert!(!cfg.remember_launch(AgentKind::Claude, None, Some("  "), Some("")));

        // Cursor: a family picked without an effort refits the stored one
        // to what the family ships, as the AGENTS TAB's own cycle does.
        cfg.cursor_effort = "high".into();
        let family_without_efforts = crate::cursor_catalogue::models()
            .iter()
            .find(|m| {
                !m.eq_ignore_ascii_case(DEFAULT_CHOICE)
                    && effort_choices(AgentKind::Cursor, Some(m), None).is_empty()
            })
            .copied()
            .expect("the seed catalogue ships a family with no effort variants");
        assert!(cfg.remember_launch(AgentKind::Cursor, None, Some(family_without_efforts), None));
        assert_eq!(cfg.cursor_model, family_without_efforts);
        assert_eq!(
            cfg.default_effort(AgentKind::Cursor),
            None,
            "refitted, never refused"
        );

        // Round trip: what was remembered is what loads.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        cfg.save_to(&path).unwrap();
        let loaded = load_from(&path);
        assert_eq!(loaded.quick_prompt_kind(), AgentKind::Cursor);
        assert_eq!(loaded.claude_effort, "high");
        assert_eq!(loaded.cursor_model, family_without_efforts);
    }

    /// RECENT PROMPTS: an Experimental switch that is off by default and
    /// a count beside it, read together through `recent_prompts_shown`
    /// — zero while off, the count while on, a hand edit clamped to what
    /// the daemon keeps — and both persisted under their own keys.
    #[test]
    fn recent_prompts_are_off_by_default_and_the_count_cycles_and_persists() {
        let mut cfg = Config::default();
        assert!(!cfg.recent_prompts, "rows stay short until asked");
        assert_eq!(cfg.recent_prompts_count, DEFAULT_RECENT_PROMPTS_COUNT);
        assert_eq!(cfg.recent_prompts_shown(), 0, "off means none drawn");
        assert_eq!(cfg.value_label(SettingKind::RecentPrompts), "off");
        assert_eq!(cfg.value_label(SettingKind::RecentPromptsCount), "3");

        let (tab, row) = locate(SettingKind::RecentPrompts).unwrap();
        assert_eq!(SETTINGS_TABS[tab].title, "Experimental");
        let (count_tab, count_row) = locate(SettingKind::RecentPromptsCount).unwrap();
        assert_eq!(count_tab, tab);
        assert_eq!(count_row, row + 1, "the count sits under its switch");

        cfg.cycle(tab, row, 0);
        assert!(cfg.recent_prompts);
        assert_eq!(cfg.recent_prompts_shown(), 3);

        // The count walks the list both ways and wraps.
        cfg.cycle(count_tab, count_row, 1);
        assert_eq!(cfg.recent_prompts_count, 4);
        cfg.cycle(count_tab, count_row, 1);
        cfg.cycle(count_tab, count_row, 1);
        assert_eq!(cfg.recent_prompts_count, 1, "wraps past 5");
        cfg.cycle(count_tab, count_row, -1);
        assert_eq!(cfg.recent_prompts_count, 5);
        assert_eq!(cfg.value_label(SettingKind::RecentPromptsCount), "5");
        let most: usize = RECENT_PROMPT_COUNTS.last().unwrap().parse().unwrap();
        assert!(
            most <= nebula_core::RECENT_PROMPTS_KEPT,
            "the overlay never asks for more than the daemon keeps"
        );

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        cfg.save_to(&path).unwrap();
        let loaded = load_from(&path);
        assert!(loaded.recent_prompts);
        assert_eq!(loaded.recent_prompts_count, 5);
        assert_eq!(loaded.recent_prompts_shown(), 5);

        // A hand edit past the list is clamped, not refused; a count that
        // is off the list steps back onto it when cycled.
        let mut cfg: Config =
            serde_json::from_str(r#"{"recent_prompts": true, "recent_prompts_count": 50}"#)
                .unwrap();
        assert_eq!(cfg.recent_prompts_shown(), nebula_core::RECENT_PROMPTS_KEPT);
        assert_eq!(
            cfg.value_label(SettingKind::RecentPromptsCount),
            nebula_core::RECENT_PROMPTS_KEPT.to_string()
        );
        cfg.cycle(count_tab, count_row, 1);
        assert_eq!(
            cfg.recent_prompts_count, 2,
            "off-list steps from the first choice"
        );
        let cfg: Config =
            serde_json::from_str(r#"{"recent_prompts": true, "recent_prompts_count": 0}"#).unwrap();
        assert_eq!(cfg.recent_prompts_shown(), 1);

        // A config predating the keys reads as off, with the default count.
        let cfg: Config = serde_json::from_str("{}").unwrap();
        assert!(!cfg.recent_prompts);
        assert_eq!(cfg.recent_prompts_count, DEFAULT_RECENT_PROMPTS_COUNT);
    }

    /// The QUICK PROMPT's harness: one name, cycled over every AGENT KIND,
    /// read back through the fallback that steps around a harness switched
    /// off since it was chosen.
    #[test]
    fn quick_prompt_kind_cycles_every_harness_and_persists() {
        let names: Vec<String> = AgentKind::ALL
            .iter()
            .filter(|k| **k != AgentKind::Custom)
            .map(|k| k.as_str().to_string())
            .collect();
        assert_eq!(agent_kind_names(), names, "one choice per kind");

        let mut cfg = Config::default();
        assert_eq!(cfg.quick_prompt_kind(), AgentKind::Claude);
        let (tab, row) = locate(SettingKind::QuickPromptKind).unwrap();
        cfg.cycle(tab, row, 1);
        assert_eq!(cfg.value_label(SettingKind::QuickPromptKind), "codex");
        assert_eq!(cfg.quick_prompt_kind(), AgentKind::Codex);
        cfg.cycle(tab, row, -1);
        assert_eq!(cfg.quick_prompt_kind(), AgentKind::Claude);

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        cfg.cycle(tab, row, 2);
        cfg.save_to(&path).unwrap();
        assert_eq!(load_from(&path).quick_prompt_kind(), AgentKind::Cursor);

        // A config predating the key, and a name nothing parses, both read
        // as Claude rather than refusing to launch.
        let cfg: Config = serde_json::from_str("{}").unwrap();
        assert_eq!(cfg.quick_prompt_kind(), AgentKind::Claude);
        let cfg: Config = serde_json::from_str(r#"{"quick_prompt_kind":"gemini"}"#).unwrap();
        assert_eq!(cfg.quick_prompt_kind(), AgentKind::Claude);

        // The chosen harness switched off on the AGENTS TAB steps on to the
        // first one still enabled.
        let cfg: Config = serde_json::from_str(
            r#"{"quick_prompt_kind":"codex","codex_enabled":false,"claude_enabled":false}"#,
        )
        .unwrap();
        assert_eq!(cfg.quick_prompt_kind(), AgentKind::Cursor);
    }

    #[test]
    fn harness_toggles_default_on_and_persist() {
        let mut cfg = Config::default();
        assert!(cfg.claude_enabled && cfg.codex_enabled && cfg.cursor_enabled);
        assert!(cfg.pi_enabled && cfg.muse_enabled && cfg.opencode_enabled);
        let builtin: Vec<AgentKind> = AgentKind::ALL
            .into_iter()
            .filter(|kind| *kind != AgentKind::Custom)
            .collect();
        assert_eq!(cfg.enabled_kinds(), builtin);
        assert!(
            !cfg.kind_enabled(AgentKind::Custom),
            "a bare Custom kind is never enabled: entries gate themselves"
        );

        let (tab, row) = locate_in(&cfg, "codex", HarnessField::Enabled).unwrap();
        cfg.cycle(tab, row, 0);
        assert!(!cfg.codex_enabled);
        assert!(!cfg.kind_enabled(AgentKind::Codex));
        assert_eq!(
            cfg.enabled_kinds(),
            vec![
                AgentKind::Claude,
                AgentKind::Cursor,
                AgentKind::Pi,
                AgentKind::Muse,
                AgentKind::Grok,
                AgentKind::OpenCode
            ],
            "the disabled kind drops out, order kept"
        );
        assert!(
            !cfg.enabled_kinds().contains(&AgentKind::Custom),
            "a bare Custom kind never lists"
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
        // A config predating the keys offers every built-in harness (a
        // bare Custom kind never lists — entries come from the registry).
        let cfg: Config = serde_json::from_str("{}").unwrap();
        assert_eq!(cfg.enabled_kinds().len(), AgentKind::ALL.len() - 1);

        // Every kind off is representable (a hand edit), and reads as empty.
        let cfg: Config = serde_json::from_str(
            r#"{"claude_enabled":false,"codex_enabled":false,"cursor_enabled":false,"pi_enabled":false,"muse_enabled":false,"opencode_enabled":false,"harnesses":{"grok":{"enabled":false}}}"#,
        )
        .unwrap();
        assert!(cfg.enabled_kinds().is_empty());
    }

    #[test]
    fn grok_is_builtin_and_settings_persist_without_legacy_keys() {
        let mut cfg: Config = serde_json::from_str("{}").unwrap();
        assert!(cfg.offered_harnesses().contains(&(AgentKind::Grok, None)));
        assert_eq!(AgentKind::parse("grok"), Some(AgentKind::Grok));
        assert_eq!(AgentKind::Grok.cli_program(), "grok");
        cfg.set_harness_enabled("grok", false);
        cfg.set_harness_model("grok", "model-id".into());
        cfg.set_harness_effort("grok", "high".into());
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        cfg.save_to(&path).unwrap();
        let loaded = load_from(&path);
        assert!(!loaded.kind_enabled(AgentKind::Grok));
        assert_eq!(
            loaded.default_model(AgentKind::Grok).as_deref(),
            Some("model-id")
        );
        assert_eq!(
            loaded.default_effort(AgentKind::Grok).as_deref(),
            Some("high")
        );
        let saved: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        assert_eq!(saved["harnesses"]["grok"]["enabled"], false);
        assert_eq!(saved["harnesses"]["grok"]["model_default"], "model-id");
        assert_eq!(saved["harnesses"]["grok"]["effort_default"], "high");
    }

    #[test]
    fn visible_kinds_hides_only_when_asked_and_only_missing_clis() {
        // Off by default: the picker lists everything enabled, even when
        // no CLI is on PATH (the daemon checks through the login shell).
        let cfg = Config::default();
        assert!(!cfg.hide_uninstalled_harnesses);
        assert_eq!(cfg.visible_kinds(), cfg.enabled_kinds());
        assert!(cfg.visible_kinds().contains(&AgentKind::Muse));

        // On: only CLIs found on PATH survive. Point PATH at a dir
        // holding just a fake `muse` binary.
        let dir = tempfile::tempdir().unwrap();
        let muse_bin = dir.path().join("muse");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::write(&muse_bin, "#!/bin/sh\nexit 0\n").unwrap();
            let mut perms = std::fs::metadata(&muse_bin).unwrap().permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(&muse_bin, perms).unwrap();
        }
        #[cfg(not(unix))]
        std::fs::write(&muse_bin, "").unwrap();
        let prior = std::env::var_os("PATH");
        std::env::set_var("PATH", dir.path());
        let filtered = Config {
            hide_uninstalled_harnesses: true,
            ..Config::default()
        }
        .visible_kinds();
        if let Some(prior) = prior {
            std::env::set_var("PATH", prior);
        } else {
            std::env::remove_var("PATH");
        }
        assert_eq!(filtered, vec![AgentKind::Muse]);
    }

    #[test]
    fn offered_harnesses_list_usable_entries_in_registry_order() {
        let cfg = Config::default();
        assert_eq!(
            cfg.offered_harnesses(),
            vec![
                (AgentKind::Claude, None),
                (AgentKind::Codex, None),
                (AgentKind::Cursor, None),
                (AgentKind::Pi, None),
                (AgentKind::Muse, None),
                (AgentKind::Grok, None),
                (AgentKind::OpenCode, None),
            ]
        );

        // Disabled and broken entries are out; the tab still lists them
        // (with their reason) so they can be fixed.
        let cfg: Config = serde_json::from_str(
            r#"{"custom_harnesses": [
                {"id": "agy", "program": "agy"},
                {"id": "off", "program": "off", "enabled": false},
                {"id": "broken", "program": ""}
            ]}"#,
        )
        .unwrap();
        let offered = cfg.offered_harnesses();
        assert_eq!(offered.len(), 8);
        assert_eq!(offered[7], (AgentKind::Custom, Some("agy".into())));
        let rows = cfg.agent_rows();
        assert!(rows.contains(&("off".to_string(), HarnessField::Enabled)));
        assert!(rows.contains(&("broken".to_string(), HarnessField::Enabled)));
        assert!(
            cfg.agent_hint("broken", HarnessField::Enabled)
                .contains("broken:"),
            "the tab names the reason"
        );

        // Under the hide switch an entry survives only when its program
        // is on PATH, built-ins and customs alike.
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("agy"), "").unwrap();
        let prior = std::env::var_os("PATH");
        std::env::set_var("PATH", dir.path());
        let hiding = Config {
            hide_uninstalled_harnesses: true,
            ..serde_json::from_str::<Config>(
                r#"{"custom_harnesses": [
                    {"id": "agy", "program": "agy"},
                    {"id": "missing", "program": "definitely-not-on-path"}
                ]}"#,
            )
            .unwrap()
        };
        let offered = hiding.offered_harnesses();
        if let Some(prior) = prior {
            std::env::set_var("PATH", prior);
        } else {
            std::env::remove_var("PATH");
        }
        assert_eq!(offered.len(), 1);
        assert_eq!(offered[0], (AgentKind::Custom, Some("agy".into())));
    }

    /// Safety: one broken `harnesses` entry never takes the rest down.
    /// The picker hides it, the tab shows its reason, and every other
    /// harness launches exactly as before.
    #[test]
    fn a_broken_harness_entry_isolates_itself() {
        let cfg: Config = serde_json::from_str(
            r#"{"harnesses": {
                "codex": {"program": ""},
                "agy": {"program": "agy", "resume_flag": "--resume"}
            }}"#,
        )
        .unwrap();
        let offered = cfg.offered_harnesses();
        assert!(
            !offered.iter().any(|(kind, _)| *kind == AgentKind::Codex),
            "the broken built-in hides"
        );
        assert!(
            offered.contains(&(AgentKind::Custom, Some("agy".into()))),
            "the valid newcomer offers"
        );
        assert_eq!(cfg.default_model(AgentKind::Claude), None);
        assert_eq!(cfg.default_model(AgentKind::Codex), None);
        assert!(cfg
            .agent_hint("codex", HarnessField::Enabled)
            .contains("broken:"));
        // The entry still resolves for reads (placeholder-free), while
        // launches refuse it with the reason.
        let codex = cfg.effective_harness_by_id("codex");
        assert!(codex.problem().is_some());
        assert_eq!(
            nebula_core::harness::resolve(&cfg.harness_registry(), AgentKind::Codex, None)
                .unwrap_err(),
            "harness `codex` has no program"
        );
    }

    /// Safety: a `harnesses` map that fails to parse costs only its own
    /// key — every other setting keeps its value, and the registry reads
    /// as a fresh install.
    #[test]
    fn an_unreadable_harnesses_map_costs_only_that_key() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        std::fs::write(
            &path,
            r#"{"harnesses": {"claude": {"enabled": "yes"}}, "theme": "ocean"}"#,
        )
        .unwrap();
        let cfg = load_from(&path);
        assert_eq!(cfg.theme, "ocean");
        assert_eq!(cfg.skipped, BTreeSet::from(["harnesses".to_string()]));
        assert!(cfg.harness_registry().iter().all(|entry| entry.enabled));
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
        // Pi passes both through like Claude: a pattern and a thinking level.
        assert_eq!(cfg.default_model(AgentKind::Pi), None);
        assert_eq!(cfg.default_effort(AgentKind::Pi), None);
        cfg.pi_model = "sonnet".into();
        cfg.pi_effort = "xhigh".into();
        assert_eq!(cfg.default_model(AgentKind::Pi).as_deref(), Some("sonnet"));
        assert_eq!(cfg.default_effort(AgentKind::Pi).as_deref(), Some("xhigh"));
        // Muse passes --model through verbatim; effort is reserved and
        // never sent, whatever the file holds.
        assert_eq!(cfg.default_model(AgentKind::Muse), None);
        assert_eq!(cfg.default_effort(AgentKind::Muse), None);
        cfg.muse_model = "spark".into();
        cfg.muse_effort = "high".into();
        assert_eq!(cfg.default_model(AgentKind::Muse).as_deref(), Some("spark"));
        assert_eq!(cfg.default_effort(AgentKind::Muse), None);
        assert_eq!(AgentKind::parse("muse"), Some(AgentKind::Muse));
        assert_eq!(AgentKind::Muse.cli_program(), "muse");
        // OpenCode passes a `provider/model` id through verbatim and has
        // no effort key at all: no Effort row to locate.
        assert_eq!(cfg.default_model(AgentKind::OpenCode), None);
        assert_eq!(cfg.default_effort(AgentKind::OpenCode), None);
        cfg.opencode_model = "anthropic/claude-sonnet-5".into();
        assert_eq!(
            cfg.default_model(AgentKind::OpenCode).as_deref(),
            Some("anthropic/claude-sonnet-5")
        );
        assert_eq!(cfg.default_effort(AgentKind::OpenCode), None);
        assert_eq!(AgentKind::parse("opencode"), Some(AgentKind::OpenCode));
        assert_eq!(AgentKind::OpenCode.cli_program(), "opencode");
        assert!(locate_in(&cfg, "opencode", HarnessField::Model).is_some());
        assert!(
            locate_in(&cfg, "opencode", HarnessField::Effort).is_none(),
            "no Effort row"
        );
        let (tab, row) = locate_in(&cfg, "pi", HarnessField::Effort).unwrap();
        cfg.cycle(tab, row, 1);
        assert_eq!(cfg.agent_value("pi", HarnessField::Effort), "max");
        cfg.cycle(tab, row, 1);
        assert_eq!(
            cfg.agent_value("pi", HarnessField::Effort),
            DEFAULT_CHOICE,
            "wraps"
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
        let (tab, row) = locate_in(&cfg, "claude", HarnessField::Model).unwrap();
        cfg.claude_model = "default".into();
        cfg.cycle(tab, row, 1);
        assert_eq!(cfg.claude_model, "fable");
        cfg.cycle(tab, row, -1);
        assert_eq!(cfg.claude_model, "default");
        let (tab, row) = locate_in(&cfg, "codex", HarnessField::Effort).unwrap();
        cfg.cycle(tab, row, 0);
        assert_eq!(
            cfg.codex_effort, "xhigh",
            "activate steps forward from high"
        );
    }

    #[test]
    fn cursor_settings_rows_follow_the_family() {
        let mut cfg = Config::default();
        let (tab, model_row) = locate_in(&cfg, "cursor", HarnessField::Model).unwrap();
        let (_, effort_row) = locate_in(&cfg, "cursor", HarnessField::Effort).unwrap();
        // No family: the effort row is n/a and does not cycle.
        assert_eq!(cfg.agent_value("cursor", HarnessField::Effort), "n/a");
        cfg.cycle(tab, effort_row, 1);
        assert_eq!(cfg.cursor_effort, "default");
        // default → auto (still no efforts) → claude-fable-5, which has no
        // bare id, so the effort lands on its fallback at once.
        cfg.cycle(tab, model_row, 1);
        assert_eq!(cfg.cursor_model, "auto");
        assert_eq!(cfg.agent_value("cursor", HarnessField::Effort), "n/a");
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
        let cursor = nebula_core::harness::builtin("cursor").unwrap();
        let fit = |m: Option<&str>, e: Option<&str>| fit_effort_in(&cursor, m, e.map(String::from));
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
        let codex = nebula_core::harness::builtin("codex").unwrap();
        assert_eq!(
            fit_effort_in(&codex, None, Some("high".into())).as_deref(),
            Some("high"),
            "claude/codex pass through"
        );
    }

    /// `claude_models` is hand-edited only: empty by default, written back
    /// as `[]` so the key is discoverable, and read back verbatim — a
    /// Bedrock id or an org's full model name survives the round trip.
    #[test]
    fn claude_models_key_round_trips_and_defaults_empty() {
        assert!(Config::default().claude_models.is_empty());
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        Config::default().save_to(&path).unwrap();
        let raw: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(raw["claude_models"], serde_json::json!([]));

        std::fs::write(
            &path,
            r#"{"claude_models": ["claude-sonnet-5", "us.anthropic.claude-opus-5-v1:0"]}"#,
        )
        .unwrap();
        let cfg = load_from(&path);
        assert_eq!(
            cfg.claude_models,
            vec![
                "claude-sonnet-5".to_string(),
                "us.anthropic.claude-opus-5-v1:0".to_string()
            ]
        );
        cfg.save_to(&path).unwrap();
        assert_eq!(load_from(&path).claude_models, cfg.claude_models);
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
        with_empty_config(|| {
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

            // A value tab carries headers exactly when its rows name
            // groups; Hotkeys and Agents always do.
            for (t, tab) in SETTINGS_TABS.iter().enumerate() {
                let headers = settings_rows(t)
                    .into_iter()
                    .filter(|row| matches!(row, SettingsRow::Header(_)))
                    .count();
                match tab.body {
                    TabBody::Values(settings) | TabBody::Project(settings) => {
                        let grouped = settings.iter().any(|s| !s.group.is_empty());
                        assert_eq!(headers > 0, grouped, "{}", tab.title);
                    }
                    TabBody::Hotkeys => assert!(headers > 0, "hotkeys tab groups its rows"),
                    TabBody::Agents => assert!(headers > 0, "agents tab groups its rows"),
                }
            }
        });
    }

    #[test]
    fn agents_tab_groups_its_rows_per_harness() {
        with_empty_config(|| {
            let tab = agents_tab();
            assert_eq!(SETTINGS_TABS[tab].title, "Agents");
            let cfg = Config::load();

            // Read the rows back the way the screen shows them: a header,
            // then the labels under it, with a blank between sections.
            let mut sections: Vec<(String, Vec<String>)> = Vec::new();
            for row in settings_rows(tab) {
                match row {
                    SettingsRow::Header(title) => sections.push((title, Vec::new())),
                    SettingsRow::Setting(i) => {
                        let label = match AGENTS_HEAD.get(i) {
                            Some(spec) => spec.label.to_string(),
                            None => cfg
                                .agent_row(i)
                                .map(|(_, field)| field.label().to_string())
                                .expect("every Agents row resolves"),
                        };
                        sections
                            .last_mut()
                            .expect("every Agents row sits under a header")
                            .1
                            .push(label);
                    }
                    SettingsRow::Blank => assert!(!sections.is_empty(), "no leading blank"),
                    SettingsRow::Project | SettingsRow::Hotkey(_) => unreachable!(),
                }
            }
            assert_eq!(
                sections,
                vec![
                    (
                        "Quick prompt".to_string(),
                        vec![
                            "Agent".to_string(),
                            "Focus".to_string(),
                            "New worktree".to_string(),
                            "Hide missing CLIs".to_string()
                        ]
                    ),
                    (
                        "Claude".to_string(),
                        vec![
                            "Enabled".to_string(),
                            "Model".to_string(),
                            "Effort".to_string()
                        ]
                    ),
                    (
                        "Codex".to_string(),
                        vec![
                            "Enabled".to_string(),
                            "Model".to_string(),
                            "Effort".to_string()
                        ]
                    ),
                    (
                        "Cursor".to_string(),
                        vec![
                            "Enabled".to_string(),
                            "Model".to_string(),
                            "Effort".to_string()
                        ]
                    ),
                    (
                        "Pi".to_string(),
                        vec![
                            "Enabled".to_string(),
                            "Model".to_string(),
                            "Effort".to_string()
                        ]
                    ),
                    (
                        "Muse".to_string(),
                        vec![
                            "Enabled".to_string(),
                            "Model".to_string(),
                            "Effort".to_string()
                        ]
                    ),
                    (
                        "Grok Build".to_string(),
                        vec![
                            "Enabled".to_string(),
                            "Model".to_string(),
                            "Effort".to_string()
                        ]
                    ),
                    (
                        "OpenCode".to_string(),
                        vec!["Enabled".to_string(), "Model".to_string()]
                    ),
                ]
            );

            // One blank line separates the sections and nothing else does.
            let blanks = settings_rows(tab)
                .into_iter()
                .filter(|row| *row == SettingsRow::Blank)
                .count();
            assert_eq!(blanks, sections.len() - 1);
        });
    }

    /// A new CLI in the registry grows its own Agents section — model and
    /// effort rows included — with no code change, and the picker offers
    /// it in the same order.
    #[test]
    fn agents_tab_grows_a_section_per_registry_entry() {
        with_empty_config(|| {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("config.json");
            std::fs::write(
                &path,
                r#"{"harnesses": {
                    "agy": {
                        "program": "agy",
                        "model_default": "big-1",
                        "effort_flag": "--effort",
                        "efforts": ["low", "high"],
                        "resume_flag": "--resume",
                        "hooks": "claude"
                    }
                }}"#,
            )
            .unwrap();
            with_config_path(path, || {
                let tab = agents_tab();
                let cfg = Config::load();
                let sections: Vec<String> = settings_rows(tab)
                    .into_iter()
                    .filter_map(|row| match row {
                        SettingsRow::Header(title) => Some(title),
                        _ => None,
                    })
                    .collect();
                assert_eq!(
                    sections,
                    vec![
                        "Quick prompt",
                        "Claude",
                        "Codex",
                        "Cursor",
                        "Pi",
                        "Muse",
                        "Grok Build",
                        "OpenCode",
                        "agy"
                    ]
                );
                let (_, model_row) =
                    locate_agent("agy", HarnessField::Model).expect("the newcomer locates");
                assert_eq!(cfg.agent_value("agy", HarnessField::Model), "big-1");
                let (_, effort_row) =
                    locate_agent("agy", HarnessField::Effort).expect("its effort row shows");
                assert_eq!(cfg.agent_value("agy", HarnessField::Effort), "default");
                assert_eq!(tab_len(tab), AGENTS_HEAD.len() + cfg.agent_rows().len());
                // ... and the picker offers it after the built-ins.
                let offered = cfg.offered_harnesses();
                assert_eq!(
                    offered.last(),
                    Some(&(AgentKind::Custom, Some("agy".to_string())))
                );
                // Cycling its rows edits the map entry a save persists:
                // an off-list hand edit steps onto the list, effort
                // steps forward through its rows.
                let mut cfg = cfg;
                cfg.cycle(tab, model_row, 0);
                assert_eq!(
                    cfg.harnesses["agy"].model_default.as_deref(),
                    Some("default"),
                    "an off-list hand edit steps onto the offered rows"
                );
                cfg.cycle(tab, effort_row, 1);
                assert_eq!(cfg.harnesses["agy"].effort_default.as_deref(), Some("low"));
            });
        });
    }

    #[test]
    fn every_tab_holds_something() {
        with_empty_config(|| {
            assert!(tab_count() >= 2);
            for (t, tab) in SETTINGS_TABS.iter().enumerate() {
                assert!(tab_len(t) > 0, "{} is empty", tab.title);
                assert!(!tab.title.is_empty());
            }
            assert_eq!(tab_len(hotkeys_tab()), crate::keymap::ACTIONS.len());
        });
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
