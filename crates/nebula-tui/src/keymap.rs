//! Rebindable application hotkeys.
//!
//! Every key the panels react to is an [`Action`] here, with a default
//! chord list. The Hotkeys tab of the settings overlay writes overrides
//! into the shared config JSON (`keybindings: { "<action id>": "j, down" }`),
//! and the event loop dispatches through [`Keymap::lookup`] instead of
//! matching raw `KeyCode`s.
//!
//! Two things this module knows that a naive keymap wouldn't:
//!
//! * **Normalization.** Terminals disagree about how a key arrives —
//!   `Shift+j` may come through as `Char('J')` with or without the shift
//!   bit, `Ctrl+]` is spelled `Ctrl+5` in the legacy encoding, `BackTab`
//!   is really `Shift+Tab`. [`KeyChord::from_event`] folds all of that into
//!   one canonical form so a binding matches whatever the emulator sends.
//!
//! * **Host reachability.** nebula runs *inside* Terminal.app / Ghostty /
//!   tmux, which eat chords before we ever see them — every `⌘` combo, most
//!   `^⇧` ones, `^←`. [`host_warning`] flags those at bind time so the user
//!   finds out at the moment of choosing, not the next time the key does
//!   nothing.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::collections::BTreeMap;
use std::fmt;

/// Which mode a binding is live in. The same chord may mean different
/// things in each, so conflicts are only conflicts within one scope.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    /// Panel navigation: the three sidebars and an unlocked terminal pane.
    Global,
    /// Input-locked terminal pane, where every other key is forwarded to
    /// the child process.
    Terminal,
}

impl Scope {
    pub fn label(self) -> &'static str {
        match self {
            Scope::Global => "panels",
            Scope::Terminal => "locked terminal",
        }
    }
}

/// Everything a hotkey can do. Overlay-local keys (Esc to close, j/k inside
/// a picker, the line-editor bindings) are deliberately absent: they're the
/// modal grammar every overlay shares, not application hotkeys.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    // navigate
    FocusNext,
    FocusPrev,
    FocusLeft,
    FocusRight,
    FocusTerminal,
    MoveDown,
    MoveUp,
    Activate,
    Palette,
    // projects & worktrees
    AddProject,
    New,
    MoveProjectDown,
    MoveProjectUp,
    Pin,
    GitDiff,
    OpenRepo,
    Notes,
    // sessions
    NewTerminal,
    NewLink,
    Rename,
    Archive,
    Unarchive,
    ToggleArchived,
    ContextMenu,
    Delete,
    DeleteAll,
    // files
    FindFile,
    Grep,
    TreeBrowser,
    // terminal
    Zoom,
    UnlockTerminal,
    // general
    Workspaces,
    ToggleWorkspaces,
    Hosts,
    Settings,
    Metrics,
    Splash,
    Help,
    Quit,
}

/// One rebindable row: what it does, where it's live, and what it starts as.
pub struct ActionSpec {
    pub action: Action,
    /// Stable key in the config file's `keybindings` object.
    pub id: &'static str,
    pub label: &'static str,
    pub hint: &'static str,
    /// Section header in the Hotkeys tab.
    pub group: &'static str,
    pub scope: Scope,
    pub defaults: &'static [&'static str],
}

pub const ACTIONS: &[ActionSpec] = &[
    // ---- NAVIGATE ----
    ActionSpec {
        action: Action::FocusNext,
        id: "focus_next",
        label: "Next panel",
        hint: "Cycle focus forward: projects → worktrees → sessions → terminal",
        group: "NAVIGATE",
        scope: Scope::Global,
        defaults: &["tab"],
    },
    ActionSpec {
        action: Action::FocusPrev,
        id: "focus_prev",
        label: "Previous panel",
        hint: "Cycle focus backward through the panels",
        group: "NAVIGATE",
        scope: Scope::Global,
        defaults: &["shift+tab"],
    },
    ActionSpec {
        action: Action::FocusLeft,
        id: "focus_left",
        label: "Focus left",
        hint: "Move focus one panel left (stops at the leftmost column)",
        group: "NAVIGATE",
        scope: Scope::Global,
        defaults: &["h", "left"],
    },
    ActionSpec {
        action: Action::FocusRight,
        id: "focus_right",
        label: "Focus right",
        hint: "Move focus one panel right (stops at sessions — Enter enters the pane)",
        group: "NAVIGATE",
        scope: Scope::Global,
        defaults: &["l", "right"],
    },
    ActionSpec {
        action: Action::FocusTerminal,
        id: "focus_terminal",
        label: "Focus terminal pane",
        hint: "Cross into the terminal pane without locking input",
        group: "NAVIGATE",
        scope: Scope::Global,
        defaults: &["ctrl+right"],
    },
    ActionSpec {
        action: Action::MoveDown,
        id: "move_down",
        label: "Move down",
        hint: "Move the selection down in the focused panel",
        group: "NAVIGATE",
        scope: Scope::Global,
        defaults: &["j", "down"],
    },
    ActionSpec {
        action: Action::MoveUp,
        id: "move_up",
        label: "Move up",
        hint: "Move the selection up in the focused panel",
        group: "NAVIGATE",
        scope: Scope::Global,
        defaults: &["k", "up"],
    },
    ActionSpec {
        action: Action::Activate,
        id: "activate",
        label: "Open / attach",
        hint: "Drill into the next panel, attach a session, or lock terminal input",
        group: "NAVIGATE",
        scope: Scope::Global,
        defaults: &["enter"],
    },
    ActionSpec {
        action: Action::Palette,
        id: "palette",
        label: "Fuzzy jump",
        hint: "Search every workspace, project, worktree and session at once",
        group: "NAVIGATE",
        scope: Scope::Global,
        defaults: &["/"],
    },
    // ---- PROJECTS & WORKTREES ----
    ActionSpec {
        action: Action::AddProject,
        id: "add_project",
        label: "Add project",
        hint: "Add a project from any panel (unlike New, never changes meaning)",
        group: "PROJECTS & WORKTREES",
        scope: Scope::Global,
        defaults: &["o"],
    },
    ActionSpec {
        action: Action::New,
        id: "new",
        label: "New (in focused panel)",
        hint: "New project / worktree / agent, depending on which panel has focus",
        group: "PROJECTS & WORKTREES",
        scope: Scope::Global,
        defaults: &["n"],
    },
    ActionSpec {
        action: Action::MoveProjectDown,
        id: "move_project_down",
        label: "Reorder project down",
        hint: "Move the selected project down the list",
        group: "PROJECTS & WORKTREES",
        scope: Scope::Global,
        defaults: &["shift+j", "shift+down"],
    },
    ActionSpec {
        action: Action::MoveProjectUp,
        id: "move_project_up",
        label: "Reorder project up",
        hint: "Move the selected project up the list",
        group: "PROJECTS & WORKTREES",
        scope: Scope::Global,
        defaults: &["shift+k", "shift+up"],
    },
    ActionSpec {
        action: Action::Pin,
        id: "pin",
        label: "Pin / unpin",
        hint: "Pin the selected worktree or session to the top of its panel",
        group: "PROJECTS & WORKTREES",
        scope: Scope::Global,
        defaults: &["p"],
    },
    ActionSpec {
        action: Action::GitDiff,
        id: "git_diff",
        label: "Git diff",
        hint: "Open the diff viewer for the selected worktree — or, on an open-PR row, that pull request's diff",
        group: "PROJECTS & WORKTREES",
        scope: Scope::Global,
        defaults: &["g"],
    },
    ActionSpec {
        action: Action::OpenRepo,
        id: "open_repo",
        label: "Open repo in browser",
        hint: "Send the selected repo's git remote (GitHub, GitLab, …) to your browser",
        group: "PROJECTS & WORKTREES",
        scope: Scope::Global,
        defaults: &["shift+g"],
    },
    ActionSpec {
        action: Action::Notes,
        id: "notes",
        label: "Notes",
        hint: "Notes for the selected project or worktree",
        group: "PROJECTS & WORKTREES",
        scope: Scope::Global,
        defaults: &["e"],
    },
    // ---- SESSIONS ----
    ActionSpec {
        action: Action::NewTerminal,
        id: "new_terminal",
        label: "New shell terminal",
        hint: "Spawn a plain shell in the selected worktree's directory",
        group: "SESSIONS",
        scope: Scope::Global,
        defaults: &["t", "shift+t"],
    },
    ActionSpec {
        action: Action::NewLink,
        id: "new_link",
        label: "Attach a link",
        hint: "Pin a PR, doc or ticket URL alongside the sessions",
        group: "SESSIONS",
        scope: Scope::Global,
        defaults: &["shift+l"],
    },
    ActionSpec {
        action: Action::Rename,
        id: "rename",
        label: "Rename",
        hint: "Rename the selected session, or edit a link's URL",
        group: "SESSIONS",
        scope: Scope::Global,
        defaults: &["r"],
    },
    ActionSpec {
        action: Action::Archive,
        id: "archive",
        label: "Archive session",
        hint: "Archive the selected agent (its PTY is released)",
        group: "SESSIONS",
        scope: Scope::Global,
        defaults: &["a"],
    },
    ActionSpec {
        action: Action::Unarchive,
        id: "unarchive",
        label: "Unarchive session",
        hint: "Bring an archived agent back into the list",
        group: "SESSIONS",
        scope: Scope::Global,
        defaults: &["u"],
    },
    ActionSpec {
        action: Action::ToggleArchived,
        id: "toggle_archived",
        label: "Show / hide archived",
        hint: "Toggle archived sessions in the sessions panel",
        group: "SESSIONS",
        scope: Scope::Global,
        defaults: &["shift+a"],
    },
    ActionSpec {
        action: Action::ContextMenu,
        id: "context_menu",
        label: "Context menu",
        hint: "Open the menu for the selected row (same as right-click)",
        group: "SESSIONS",
        scope: Scope::Global,
        defaults: &["m"],
    },
    ActionSpec {
        action: Action::Delete,
        id: "delete",
        label: "Delete selected",
        hint: "Remove the selected row, behind a confirmation",
        group: "SESSIONS",
        scope: Scope::Global,
        defaults: &["d", "delete", "backspace"],
    },
    ActionSpec {
        action: Action::DeleteAll,
        id: "delete_all",
        label: "Delete all in panel",
        hint: "Remove every row of the focused panel, behind a confirmation",
        group: "SESSIONS",
        scope: Scope::Global,
        defaults: &["shift+d"],
    },
    // ---- FILES ----
    ActionSpec {
        action: Action::FindFile,
        id: "find_file",
        label: "Find file",
        hint: "Fuzzy file finder for the selected worktree",
        group: "FILES",
        scope: Scope::Global,
        defaults: &["f"],
    },
    ActionSpec {
        action: Action::Grep,
        id: "grep",
        label: "Find in files",
        hint: "git grep across the selected worktree",
        group: "FILES",
        scope: Scope::Global,
        defaults: &["shift+f"],
    },
    ActionSpec {
        action: Action::TreeBrowser,
        id: "tree_browser",
        label: "File tree browser",
        hint: "Browse the worktree's files with a preview pane",
        group: "FILES",
        scope: Scope::Global,
        defaults: &["b"],
    },
    // ---- TERMINAL ----
    ActionSpec {
        action: Action::Zoom,
        id: "zoom",
        label: "Full-screen terminal",
        hint: "Collapse the sidebars and lock input into the attached session",
        group: "TERMINAL",
        scope: Scope::Global,
        defaults: &["z"],
    },
    ActionSpec {
        action: Action::UnlockTerminal,
        id: "unlock_terminal",
        label: "Unlock terminal input",
        hint: "Leave the locked pane and go back to the panels (^q always works)",
        group: "TERMINAL",
        scope: Scope::Terminal,
        defaults: &["ctrl+q", "ctrl+]", "ctrl+esc", "ctrl+left"],
    },
    // ---- GENERAL ----
    ActionSpec {
        action: Action::Workspaces,
        id: "workspaces",
        label: "Workspaces",
        hint: "Switch workspace (n / r / d manage them)",
        group: "GENERAL",
        scope: Scope::Global,
        defaults: &["w"],
    },
    ActionSpec {
        action: Action::ToggleWorkspaces,
        id: "toggle_workspaces",
        label: "Workspaces column",
        hint: "Show or hide the Workspaces column at the left edge",
        group: "GENERAL",
        scope: Scope::Global,
        defaults: &["shift+w"],
    },
    ActionSpec {
        action: Action::Hosts,
        id: "hosts",
        label: "SSH hosts",
        hint: "Connect to a saved ssh host (restarts nebula over ssh)",
        group: "GENERAL",
        scope: Scope::Global,
        defaults: &["shift+h"],
    },
    ActionSpec {
        action: Action::Settings,
        id: "settings",
        label: "Settings",
        hint: "Open this settings overlay",
        group: "GENERAL",
        scope: Scope::Global,
        defaults: &["s"],
    },
    ActionSpec {
        action: Action::Metrics,
        id: "metrics",
        label: "Memory usage",
        hint: "RAM used by nebula and every live agent's process tree",
        group: "GENERAL",
        scope: Scope::Global,
        defaults: &["shift+m"],
    },
    ActionSpec {
        action: Action::Splash,
        id: "splash",
        label: "Nebula splash",
        hint: "Replay the startup splash (any key returns)",
        group: "GENERAL",
        scope: Scope::Global,
        defaults: &["shift+n"],
    },
    ActionSpec {
        action: Action::Help,
        id: "help",
        label: "Help",
        hint: "Toggle the keyboard help overlay",
        group: "GENERAL",
        scope: Scope::Global,
        defaults: &["?"],
    },
    ActionSpec {
        action: Action::Quit,
        id: "quit",
        label: "Quit nebula",
        hint: "Leave the TUI (sessions keep running in the daemon)",
        group: "GENERAL",
        scope: Scope::Global,
        defaults: &["q", "ctrl+c"],
    },
];

pub fn spec_at(index: usize) -> Option<&'static ActionSpec> {
    ACTIONS.get(index)
}

pub fn index_of(action: Action) -> Option<usize> {
    ACTIONS.iter().position(|s| s.action == action)
}

// ---- chords ----

/// A single key press: one key plus the modifiers held with it, in the one
/// canonical spelling [`KeyChord::from_event`] produces.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct KeyChord {
    pub code: KeyCode,
    pub mods: KeyModifiers,
}

/// The modifiers a binding can name. META/HYPER are dropped: no terminal
/// reports them for a key a user could reasonably choose.
const KEPT_MODS: KeyModifiers = KeyModifiers::CONTROL
    .union(KeyModifiers::ALT)
    .union(KeyModifiers::SHIFT)
    .union(KeyModifiers::SUPER);

impl KeyChord {
    /// Canonicalize a crossterm event into a chord, so the same physical
    /// press compares equal however the emulator spelled it:
    ///
    /// * `Char('J')` and `shift + Char('j')` both become `shift+j`;
    /// * shift is dropped from punctuation and digits, where the glyph
    ///   already carries it (`?` is `?`, never `shift+?`);
    /// * `BackTab` becomes `shift+tab`;
    /// * `ctrl+5` — the legacy encoding's name for byte 0x1D — becomes
    ///   `ctrl+]`, which is what the user actually pressed.
    pub fn from_event(key: &KeyEvent) -> Self {
        let mut mods = key.modifiers & KEPT_MODS;
        let mut code = key.code;
        match code {
            KeyCode::Char(c) => {
                if c.is_alphabetic() {
                    if c.is_uppercase() {
                        mods |= KeyModifiers::SHIFT;
                    }
                    code = KeyCode::Char(c.to_lowercase().next().unwrap_or(c));
                } else {
                    mods.remove(KeyModifiers::SHIFT);
                }
            }
            KeyCode::BackTab => {
                code = KeyCode::Tab;
                mods |= KeyModifiers::SHIFT;
            }
            _ => {}
        }
        if mods.contains(KeyModifiers::CONTROL) && code == KeyCode::Char('5') {
            code = KeyCode::Char(']');
        }
        Self { code, mods }
    }

    /// Parse a config-file spelling (`"ctrl+shift+f"`, `"shift+tab"`, `"/"`).
    /// Unknown key names return None, which the loader reports and skips.
    pub fn parse(spec: &str) -> Option<Self> {
        let spec = spec.trim();
        if spec.is_empty() {
            return None;
        }
        // '+' is both the separator and a bindable key, so a chord ending
        // in one — "+" alone, or "ctrl++" — names the plus key itself.
        let (mod_part, name) = match spec.strip_suffix('+') {
            Some(rest) => (rest.trim_end_matches('+'), "+".to_string()),
            None => match spec.rfind('+') {
                Some(i) => (&spec[..i], spec[i + 1..].trim().to_lowercase()),
                None => ("", spec.to_lowercase()),
            },
        };
        let mut mods = KeyModifiers::NONE;
        for part in mod_part.split('+').filter(|p| !p.trim().is_empty()) {
            mods |= match part.trim().to_lowercase().as_str() {
                "ctrl" | "control" | "^" => KeyModifiers::CONTROL,
                "alt" | "opt" | "option" | "meta" => KeyModifiers::ALT,
                "shift" => KeyModifiers::SHIFT,
                "cmd" | "super" | "win" => KeyModifiers::SUPER,
                _ => return None,
            };
        }
        let code = parse_key_name(&name)?;
        Some(Self::from_event(&KeyEvent::new(code, mods)))
    }

    /// Config-file spelling. Round-trips through [`KeyChord::parse`].
    pub fn spec(&self) -> String {
        let mut out = String::new();
        if self.mods.contains(KeyModifiers::CONTROL) {
            out.push_str("ctrl+");
        }
        if self.mods.contains(KeyModifiers::ALT) {
            out.push_str("alt+");
        }
        if self.mods.contains(KeyModifiers::SHIFT) {
            out.push_str("shift+");
        }
        if self.mods.contains(KeyModifiers::SUPER) {
            out.push_str("cmd+");
        }
        out.push_str(&key_name(self.code));
        out
    }

    /// Compact on-screen spelling: `^q`, `⇧Tab`, `⌥p`, `⌘k`, `↓`.
    pub fn display(&self) -> String {
        let mut out = String::new();
        if self.mods.contains(KeyModifiers::CONTROL) {
            out.push('^');
        }
        if self.mods.contains(KeyModifiers::ALT) {
            out.push('⌥');
        }
        if self.mods.contains(KeyModifiers::SUPER) {
            out.push('⌘');
        }
        let shift = self.mods.contains(KeyModifiers::SHIFT);
        match self.code {
            // A shifted letter reads best as the capital itself.
            KeyCode::Char(c) if shift && c.is_alphabetic() => {
                out.push('⇧');
                out.extend(c.to_uppercase());
            }
            _ => {
                if shift {
                    out.push('⇧');
                }
                out.push_str(&key_display(self.code));
            }
        }
        out
    }
}

impl fmt::Display for KeyChord {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.display())
    }
}

fn parse_key_name(name: &str) -> Option<KeyCode> {
    let code = match name {
        "up" => KeyCode::Up,
        "down" => KeyCode::Down,
        "left" => KeyCode::Left,
        "right" => KeyCode::Right,
        "enter" | "return" | "cr" => KeyCode::Enter,
        "tab" => KeyCode::Tab,
        "backtab" => KeyCode::BackTab,
        "esc" | "escape" => KeyCode::Esc,
        "space" => KeyCode::Char(' '),
        "bs" | "backspace" => KeyCode::Backspace,
        "del" | "delete" => KeyCode::Delete,
        "ins" | "insert" => KeyCode::Insert,
        "home" => KeyCode::Home,
        "end" => KeyCode::End,
        "pgup" | "pageup" => KeyCode::PageUp,
        "pgdn" | "pagedown" => KeyCode::PageDown,
        _ => {
            if let Some(n) = name.strip_prefix('f') {
                if let Ok(n) = n.parse::<u8>() {
                    if (1..=20).contains(&n) {
                        return Some(KeyCode::F(n));
                    }
                }
            }
            let mut chars = name.chars();
            let c = chars.next()?;
            if chars.next().is_some() {
                return None;
            }
            KeyCode::Char(c)
        }
    };
    Some(code)
}

fn key_name(code: KeyCode) -> String {
    match code {
        KeyCode::Up => "up".into(),
        KeyCode::Down => "down".into(),
        KeyCode::Left => "left".into(),
        KeyCode::Right => "right".into(),
        KeyCode::Enter => "enter".into(),
        KeyCode::Tab | KeyCode::BackTab => "tab".into(),
        KeyCode::Esc => "esc".into(),
        KeyCode::Backspace => "backspace".into(),
        KeyCode::Delete => "delete".into(),
        KeyCode::Insert => "insert".into(),
        KeyCode::Home => "home".into(),
        KeyCode::End => "end".into(),
        KeyCode::PageUp => "pgup".into(),
        KeyCode::PageDown => "pgdn".into(),
        KeyCode::Char(' ') => "space".into(),
        KeyCode::Char(c) => c.to_lowercase().to_string(),
        KeyCode::F(n) => format!("f{n}"),
        other => format!("{other:?}").to_lowercase(),
    }
}

fn key_display(code: KeyCode) -> String {
    match code {
        KeyCode::Up => "↑".into(),
        KeyCode::Down => "↓".into(),
        KeyCode::Left => "←".into(),
        KeyCode::Right => "→".into(),
        KeyCode::Enter => "Enter".into(),
        KeyCode::Tab | KeyCode::BackTab => "Tab".into(),
        KeyCode::Esc => "Esc".into(),
        KeyCode::Backspace => "⌫".into(),
        KeyCode::Delete => "Del".into(),
        KeyCode::Insert => "Ins".into(),
        KeyCode::Home => "Home".into(),
        KeyCode::End => "End".into(),
        KeyCode::PageUp => "PgUp".into(),
        KeyCode::PageDown => "PgDn".into(),
        KeyCode::Char(' ') => "Space".into(),
        KeyCode::Char(c) => c.to_string(),
        KeyCode::F(n) => format!("F{n}"),
        other => format!("{other:?}"),
    }
}

// ---- host-terminal reachability ----

/// How likely a chord is to actually reach nebula from inside the user's
/// terminal emulator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reach {
    /// Every emulator delivers it.
    Fine,
    /// Arrives in some setups only — worth saying so, still bindable.
    Risky,
    /// Effectively never arrives: the emulator or the OS takes it first.
    Blocked,
}

impl Reach {
    pub fn is_fine(self) -> bool {
        self == Reach::Fine
    }
}

/// Whether the host terminal is likely to swallow `chord` before nebula
/// sees it, and why. nebula is always a guest inside Terminal.app, Ghostty,
/// iTerm, tmux or an ssh session, and each of those claims keys for itself
/// — so the honest answer here is a probability, not a fact. Nothing is
/// rejected on the strength of it; the settings overlay just says so.
pub fn host_warning(chord: &KeyChord) -> (Reach, Option<&'static str>) {
    let m = chord.mods;
    let ctrl = m.contains(KeyModifiers::CONTROL);
    let shift = m.contains(KeyModifiers::SHIFT);
    let alt = m.contains(KeyModifiers::ALT);

    if m.contains(KeyModifiers::SUPER) {
        return (
            Reach::Blocked,
            Some("⌘ chords never reach a TUI — Terminal.app swallows them, Ghostty binds its own"),
        );
    }
    // Mission Control owns these on stock macOS (they switch Spaces).
    if ctrl && matches!(chord.code, KeyCode::Left | KeyCode::Right) && !shift && !alt {
        return (
            Reach::Risky,
            Some("macOS Mission Control takes ^← / ^→ unless you turn its Spaces shortcuts off"),
        );
    }
    if ctrl && shift {
        return (
            Reach::Risky,
            Some(
                "^⇧ needs the kitty keyboard protocol — Ghostty/kitty send it, Terminal.app won't",
            ),
        );
    }
    if ctrl {
        match chord.code {
            // Legacy control bytes that collide with a key of their own.
            KeyCode::Char('m') => {
                return (
                    Reach::Risky,
                    Some("^m is the same byte as Enter in terminals without the kitty protocol"),
                )
            }
            KeyCode::Char('i') => {
                return (
                    Reach::Risky,
                    Some("^i is the same byte as Tab in terminals without the kitty protocol"),
                )
            }
            KeyCode::Char('[') => {
                return (
                    Reach::Risky,
                    Some("^[ is the same byte as Esc in terminals without the kitty protocol"),
                )
            }
            KeyCode::Char('h') => {
                return (
                    Reach::Risky,
                    Some("^h is the same byte as ⌫ in terminals without the kitty protocol"),
                )
            }
            KeyCode::Enter | KeyCode::Tab | KeyCode::Backspace | KeyCode::Esc => {
                return (
                    Reach::Risky,
                    Some("ctrl + this key needs the kitty keyboard protocol to be distinguishable"),
                )
            }
            // Only a handful of punctuation has a control byte at all.
            KeyCode::Char(c)
                if !c.is_ascii_alphabetic()
                    && !matches!(c, ' ' | '@' | '[' | '\\' | ']' | '^' | '_' | '/') =>
            {
                return (
                    Reach::Risky,
                    Some("most terminals have no encoding for ctrl + this key"),
                )
            }
            _ => {}
        }
    }
    if alt {
        return (
            Reach::Risky,
            Some("⌥ only arrives if your terminal sends Option as Meta (Terminal.app: off by default)"),
        );
    }
    if let KeyCode::F(n) = chord.code {
        if n >= 13 {
            return (Reach::Risky, Some("few terminals emit F13 and above"));
        }
    }
    (Reach::Fine, None)
}

// ---- the map ----

/// Resolved bindings: one chord list per entry of [`ACTIONS`], in the same
/// order, so an index is a stable handle for both the UI and the config.
#[derive(Debug, Clone)]
pub struct Keymap {
    binds: Vec<Vec<KeyChord>>,
}

impl Default for Keymap {
    fn default() -> Self {
        Self {
            binds: ACTIONS.iter().map(|s| parse_list(s.defaults)).collect(),
        }
    }
}

impl Keymap {
    /// Defaults with the config's `keybindings` overrides applied. An
    /// override naming an unknown action or an unparseable chord is logged
    /// and skipped, so one bad line can't strand the user without a keymap.
    /// An empty string is a deliberate unbind.
    pub fn from_overrides(overrides: &BTreeMap<String, String>) -> Self {
        let mut map = Self::default();
        for (id, spec) in overrides {
            let Some(index) = ACTIONS.iter().position(|s| s.id == id) else {
                tracing::warn!("ignoring keybinding for unknown action {id:?}");
                continue;
            };
            if spec.trim().is_empty() {
                map.binds[index].clear();
                continue;
            }
            let mut chords = Vec::new();
            for part in spec.split(',') {
                match KeyChord::parse(part) {
                    Some(chord) if !chords.contains(&chord) => chords.push(chord),
                    Some(_) => {}
                    None => tracing::warn!("ignoring unparseable keybinding {part:?} for {id}"),
                }
            }
            map.binds[index] = chords;
        }
        map
    }

    /// Only what differs from the defaults, for writing back to the config.
    pub fn overrides(&self) -> BTreeMap<String, String> {
        let mut out = BTreeMap::new();
        for (index, spec) in ACTIONS.iter().enumerate() {
            if self.binds[index] != parse_list(spec.defaults) {
                out.insert(spec.id.to_string(), self.spec_list(index));
            }
        }
        out
    }

    /// The action `chord` triggers in `scope`, if any.
    pub fn lookup(&self, scope: Scope, chord: &KeyChord) -> Option<Action> {
        ACTIONS
            .iter()
            .enumerate()
            .find(|(i, s)| s.scope == scope && self.binds[*i].contains(chord))
            .map(|(_, s)| s.action)
    }

    pub fn chords(&self, action: Action) -> &[KeyChord] {
        match index_of(action) {
            Some(i) => &self.binds[i],
            None => &[],
        }
    }

    pub fn chords_at(&self, index: usize) -> &[KeyChord] {
        self.binds.get(index).map(|v| v.as_slice()).unwrap_or(&[])
    }

    /// On-screen chord list for a row: `j ↓`, or `—` when unbound.
    pub fn display_at(&self, index: usize) -> String {
        let chords = self.chords_at(index);
        if chords.is_empty() {
            return "—".into();
        }
        chords
            .iter()
            .map(|c| c.display())
            .collect::<Vec<_>>()
            .join(" ")
    }

    /// The first chord for an action, for help text and footer hints.
    pub fn first(&self, action: Action) -> Option<KeyChord> {
        self.chords(action).first().copied()
    }

    /// Help-style label for an action: every chord it answers to, or `—`.
    pub fn label(&self, action: Action) -> String {
        match index_of(action) {
            Some(i) => self.display_at(i),
            None => "—".into(),
        }
    }

    fn spec_list(&self, index: usize) -> String {
        self.binds[index]
            .iter()
            .map(|c| c.spec())
            .collect::<Vec<_>>()
            .join(", ")
    }

    /// Other rows that already answer to `chord` in the same scope as
    /// `index`. Cross-scope reuse is fine — the panels and a locked
    /// terminal never read the same keystroke.
    pub fn conflicts(&self, index: usize, chord: &KeyChord) -> Vec<usize> {
        let Some(spec) = ACTIONS.get(index) else {
            return Vec::new();
        };
        ACTIONS
            .iter()
            .enumerate()
            .filter(|(i, s)| *i != index && s.scope == spec.scope && self.binds[*i].contains(chord))
            .map(|(i, _)| i)
            .collect()
    }

    /// Bind `chord` to `index`, replacing its chords (or appending when
    /// `add`). Any conflicting row loses the chord — one keystroke can
    /// only mean one thing, and silently shadowing the loser would be
    /// worse than taking it away visibly.
    pub fn bind(&mut self, index: usize, chord: KeyChord, add: bool) {
        for other in self.conflicts(index, &chord) {
            self.binds[other].retain(|c| *c != chord);
        }
        let slot = &mut self.binds[index];
        if add {
            if !slot.contains(&chord) {
                slot.push(chord);
            }
        } else {
            slot.clear();
            slot.push(chord);
        }
    }

    pub fn reset(&mut self, index: usize) {
        if let Some(spec) = ACTIONS.get(index) {
            self.binds[index] = parse_list(spec.defaults);
        }
    }

    pub fn clear(&mut self, index: usize) {
        if let Some(slot) = self.binds.get_mut(index) {
            slot.clear();
        }
    }

    pub fn is_default(&self, index: usize) -> bool {
        ACTIONS
            .get(index)
            .is_some_and(|spec| self.binds[index] == parse_list(spec.defaults))
    }

    /// A row sharing a chord with another action in the same scope. The
    /// overlay warns before it lets you make one, so this only comes from a
    /// hand-edited config — but when it does, [`Keymap::lookup`] silently
    /// gives the key to whichever action is declared first, and a row that
    /// looks bound while doing nothing is exactly the confusing state the
    /// duplicate warning exists to prevent.
    pub fn is_ambiguous(&self, index: usize) -> bool {
        self.chords_at(index)
            .iter()
            .any(|c| !self.conflicts(index, c).is_empty())
    }

    /// Who else claims this row's chords, as a readable list.
    pub fn shadowed_by(&self, index: usize) -> Vec<&'static str> {
        let mut out: Vec<&'static str> = self
            .chords_at(index)
            .iter()
            .flat_map(|c| self.conflicts(index, c))
            .filter_map(|i| ACTIONS.get(i).map(|s| s.label))
            .collect();
        out.dedup();
        out
    }

    /// Rows whose chords the host terminal probably eats, with the worst
    /// verdict across the row's chords.
    pub fn reach_at(&self, index: usize) -> Reach {
        self.chords_at(index)
            .iter()
            .map(|c| host_warning(c).0)
            .fold(Reach::Fine, |worst, r| match (worst, r) {
                (Reach::Blocked, _) | (_, Reach::Blocked) => Reach::Blocked,
                (Reach::Risky, _) | (_, Reach::Risky) => Reach::Risky,
                _ => Reach::Fine,
            })
    }
}

fn parse_list(specs: &[&str]) -> Vec<KeyChord> {
    specs.iter().filter_map(|s| KeyChord::parse(s)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(code: KeyCode, mods: KeyModifiers) -> KeyChord {
        KeyChord::from_event(&KeyEvent::new(code, mods))
    }

    #[test]
    fn every_default_parses_and_round_trips() {
        for spec in ACTIONS {
            for raw in spec.defaults {
                let chord = KeyChord::parse(raw)
                    .unwrap_or_else(|| panic!("{}: {raw:?} does not parse", spec.id));
                let reparsed = KeyChord::parse(&chord.spec())
                    .unwrap_or_else(|| panic!("{}: {raw:?} does not round-trip", spec.id));
                assert_eq!(chord, reparsed, "{}: {raw:?}", spec.id);
            }
        }
    }

    #[test]
    fn action_ids_are_unique() {
        let mut ids: Vec<&str> = ACTIONS.iter().map(|s| s.id).collect();
        ids.sort_unstable();
        let before = ids.len();
        ids.dedup();
        assert_eq!(before, ids.len(), "duplicate action id");
    }

    #[test]
    fn defaults_do_not_collide_within_a_scope() {
        let map = Keymap::default();
        for (i, _) in ACTIONS.iter().enumerate() {
            for chord in map.chords_at(i) {
                assert!(
                    map.conflicts(i, chord).is_empty(),
                    "{} default {chord} collides with {:?}",
                    ACTIONS[i].id,
                    map.conflicts(i, chord)
                        .iter()
                        .map(|j| ACTIONS[*j].id)
                        .collect::<Vec<_>>()
                );
            }
        }
    }

    #[test]
    fn shift_letters_normalize_both_spellings() {
        let upper = ev(KeyCode::Char('J'), KeyModifiers::SHIFT);
        let bare_upper = ev(KeyCode::Char('J'), KeyModifiers::NONE);
        let lower_shift = ev(KeyCode::Char('j'), KeyModifiers::SHIFT);
        assert_eq!(upper, bare_upper);
        assert_eq!(upper, lower_shift);
        assert_eq!(upper.spec(), "shift+j");
        assert_eq!(upper.display(), "⇧J");
        assert_ne!(upper, ev(KeyCode::Char('j'), KeyModifiers::NONE));
    }

    #[test]
    fn punctuation_drops_the_shift_bit() {
        // '?' is shift+/ on a US layout; the glyph already says so.
        assert_eq!(
            ev(KeyCode::Char('?'), KeyModifiers::SHIFT),
            ev(KeyCode::Char('?'), KeyModifiers::NONE)
        );
        assert_eq!(KeyChord::parse("?").unwrap().spec(), "?");
    }

    #[test]
    fn backtab_is_shift_tab() {
        assert_eq!(
            ev(KeyCode::BackTab, KeyModifiers::NONE),
            KeyChord::parse("shift+tab").unwrap()
        );
    }

    #[test]
    fn legacy_ctrl_5_is_ctrl_bracket() {
        assert_eq!(
            ev(KeyCode::Char('5'), KeyModifiers::CONTROL),
            KeyChord::parse("ctrl+]").unwrap()
        );
    }

    #[test]
    fn parses_the_literal_plus_key() {
        let plus = KeyChord::parse("+").unwrap();
        assert_eq!(plus.code, KeyCode::Char('+'));
        assert_eq!(KeyChord::parse("ctrl++").unwrap().code, KeyCode::Char('+'));
    }

    #[test]
    fn lookup_respects_scope() {
        let map = Keymap::default();
        let ctrl_q = KeyChord::parse("ctrl+q").unwrap();
        assert_eq!(
            map.lookup(Scope::Terminal, &ctrl_q),
            Some(Action::UnlockTerminal)
        );
        // ^q in the panels is free — the scopes never read the same press.
        assert_eq!(map.lookup(Scope::Global, &ctrl_q), None);
        assert_eq!(
            map.lookup(Scope::Global, &KeyChord::parse("q").unwrap()),
            Some(Action::Quit)
        );
    }

    #[test]
    fn binding_steals_the_chord_from_its_previous_owner() {
        let mut map = Keymap::default();
        let g = KeyChord::parse("g").unwrap();
        let notes = index_of(Action::Notes).unwrap();
        assert_eq!(
            map.conflicts(notes, &g),
            vec![index_of(Action::GitDiff).unwrap()]
        );
        map.bind(notes, g, false);
        assert_eq!(map.lookup(Scope::Global, &g), Some(Action::Notes));
        assert!(!map.chords(Action::GitDiff).contains(&g));
    }

    #[test]
    fn overrides_round_trip_through_the_config_shape() {
        let mut map = Keymap::default();
        let quit = index_of(Action::Quit).unwrap();
        map.bind(quit, KeyChord::parse("ctrl+x").unwrap(), false);
        let saved = map.overrides();
        assert_eq!(saved.get("quit").map(String::as_str), Some("ctrl+x"));
        // Untouched actions stay out of the file.
        assert!(!saved.contains_key("help"));
        let loaded = Keymap::from_overrides(&saved);
        assert_eq!(
            loaded.lookup(Scope::Global, &KeyChord::parse("ctrl+x").unwrap()),
            Some(Action::Quit)
        );
        assert_eq!(
            loaded.lookup(Scope::Global, &KeyChord::parse("q").unwrap()),
            None
        );
    }

    #[test]
    fn an_empty_override_means_unbound() {
        let map = Keymap::from_overrides(&BTreeMap::from([("help".into(), String::new())]));
        assert!(map.chords(Action::Help).is_empty());
        assert_eq!(map.label(Action::Help), "—");
    }

    #[test]
    fn a_broken_override_falls_back_instead_of_stranding_the_user() {
        let map = Keymap::from_overrides(&BTreeMap::from([
            ("quit".into(), "nonsense+zz".into()),
            ("no_such_action".into(), "x".into()),
        ]));
        assert!(map.chords(Action::Quit).is_empty(), "the row is cleared");
        assert_eq!(
            map.lookup(Scope::Global, &KeyChord::parse("?").unwrap()),
            Some(Action::Help),
            "the rest of the map is untouched"
        );
    }

    #[test]
    fn a_hand_edited_duplicate_is_flagged_on_both_rows() {
        // Nothing in the overlay can produce this; a text editor can.
        let map = Keymap::from_overrides(&BTreeMap::from([("notes".into(), "g".into())]));
        let notes = index_of(Action::Notes).unwrap();
        let diff = index_of(Action::GitDiff).unwrap();
        assert!(map.is_ambiguous(notes));
        assert!(map.is_ambiguous(diff));
        assert_eq!(map.shadowed_by(notes), vec!["Git diff"]);
        // The defaults themselves are always unambiguous.
        assert!(Keymap::default()
            .binds
            .iter()
            .enumerate()
            .all(|(i, _)| !Keymap::default().is_ambiguous(i)));
    }

    #[test]
    fn cmd_chords_are_reported_unreachable() {
        let (reach, why) = host_warning(&KeyChord::parse("cmd+]").unwrap());
        assert_eq!(reach, Reach::Blocked);
        assert!(why.unwrap().contains('⌘'));
    }

    #[test]
    fn risky_chords_are_flagged_but_allowed() {
        for spec in ["ctrl+shift+f", "ctrl+left", "alt+p", "ctrl+m", "f13"] {
            let chord = KeyChord::parse(spec).unwrap();
            let (reach, why) = host_warning(&chord);
            assert_eq!(reach, Reach::Risky, "{spec}");
            assert!(why.is_some(), "{spec}");
        }
    }

    #[test]
    fn ordinary_chords_are_clean() {
        for spec in ["q", "shift+j", "/", "ctrl+q", "down", "enter", "f5"] {
            let chord = KeyChord::parse(spec).unwrap();
            assert!(host_warning(&chord).0.is_fine(), "{spec} should be fine");
        }
    }

    #[test]
    fn every_action_ships_with_a_reachable_chord() {
        // Some defaults are deliberately iffy (^→ fights Mission Control,
        // ^] is a fallback hatch), but no action may be *only* reachable
        // through a chord the host terminal is likely to eat.
        let map = Keymap::default();
        for (i, spec) in ACTIONS.iter().enumerate() {
            // focus_terminal is the one exception: ^→ is its only binding,
            // and Tab cycling round to the pane covers the same ground.
            if spec.action == Action::FocusTerminal {
                continue;
            }
            assert!(
                map.chords_at(i).iter().any(|c| host_warning(c).0.is_fine()),
                "{} has no chord a stock terminal delivers",
                spec.id
            );
        }
    }
}
