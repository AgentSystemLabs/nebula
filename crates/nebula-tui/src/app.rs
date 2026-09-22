//! TUI state: the Elm-ish Model.

use crate::git_diff::DiffFile;
use crate::pull_request::{OpenPr, PrDetail, PullRequest};
use crate::text_input::TextInput;
use nebula_core::{
    Agent, AgentId, AgentKind, AgentStatus, Link, LinkId, Project, ProjectId, SessionRef,
    TerminalId, TerminalTab, Worktree, WorktreeId,
};
use ratatui::layout::{Position, Rect};
use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;

/// Frame duration of the status-sweep text animation: the event loop's
/// repaint cadence while [`App::status_anim_active`] holds, and the step
/// size of [`App::sweep_phase`] (one text cell per frame).
pub const SWEEP_FRAME: std::time::Duration = std::time::Duration::from_millis(100);

/// How long a ONE-SHOT SWEEP runs: the sweep a row takes for a change that
/// happened while nobody was looking — a turn finishing unread (blue), a
/// checkout's pull request merging (purple) — before it settles into its
/// still color. Two or three passes of the band: long enough to catch the
/// eye from another panel, short enough that motion keeps meaning *live*.
/// Only running and needs-feedback rows sweep for as long as they last.
pub const ONE_SHOT_SWEEP: std::time::Duration = std::time::Duration::from_secs(5);

/// How many recently shown sessions keep their screen ([`App::term_cache`]):
/// enough for a rotation through the sessions of a couple of worktrees.
/// What bounds the memory is [`TERM_CACHE_CELLS`], not this.
pub const TERM_CACHE_MAX: usize = 6;
/// The most the kept screens may hold between them, in grid cells (32 bytes
/// each, so about 12 MB — half of what two entries of that size each used to
/// be allowed). An alt-screen CLI is a screen's worth, 200 KB; a shell whose
/// 10 000-line scrollback has filled is tens of megabytes on its own, and
/// used never to be kept at all — every return to it re-parsed the whole
/// ring, 33 ms of blank pane under the INPUT LATENCY PROBE. Now a screen
/// that does not fit is kept WITHOUT its history ([`AttachedTerm::
/// drop_history`]): the return paints on the keypress like any other, and
/// the history is replayed if the user scrolls up into it.
pub const TERM_CACHE_CELLS: usize = 400_000;

/// The git repository nebula was started in: the directory it was launched
/// from, or the nearest one above it holding a `.git` — what the first
/// run's SPLASH offers to open with Enter, and what the open-project prompt
/// starts on. None when started outside a repository, or at the home
/// directory itself: a dotfiles repo at `~` is not the project anyone
/// launching from there means.
pub fn launch_repo() -> Option<PathBuf> {
    let cwd = std::env::current_dir().ok()?;
    let home = nebula_core::env::home_dir();
    cwd.ancestors()
        .take_while(|dir| Some(*dir) != home.as_deref())
        .find(|dir| dir.join(".git").exists())
        .map(std::path::Path::to_path_buf)
}

/// Wall-clock epoch ms, comparable to the daemon's `status_changed_at`.
pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    Projects,
    Worktrees,
    Sessions,
    Terminal,
}

/// What a screen cell maps to; rebuilt on every draw for hit-testing.
#[derive(Debug, Clone, PartialEq)]
pub enum HitTarget {
    /// The GRID's background (registered after the cards, so they win).
    PanelBg(Focus),
    TerminalPane,
    /// The session URL on the CLOUD SESSION PANEL; a click opens it in the
    /// browser. Registered ahead of the pane it sits on, so it wins.
    CloudSessionLink,
    /// A card in the LAUNCHER VIEW's GRID, by its place in
    /// `launcher::rows`.
    LauncherRow(usize),
    /// The `‹ sessions` crumb in a full-screen session's header
    /// (LAUNCHER VIEW): a click leaves the session for the grid, as `^q`
    /// does.
    LauncherCrumb,
    /// A PROJECT TAB in the LAUNCHER VIEW's header, by the project it
    /// names: a click opens that project's sessions, as `[` and `]`
    /// walking onto it do, and a right-click opens it with the project's
    /// own menu over it — new worktree, run, rename, remove.
    LauncherTab(ProjectId),
    /// The `×` on that tab. Its own target rather than a corner of the
    /// tab's, so a click on the cross can never read as a click on the
    /// tab it closes.
    LauncherTabClose(ProjectId),
    /// The `+` before the first tab: a click drops the PROJECT DROPDOWN
    /// under it — every project, the one in front of you ticked, narrowed
    /// by type-ahead, with a row for opening a folder that is not one yet
    /// — and the pick opens a tab.
    LauncherTabAdd,
    /// Draggable top edge of the LAUNCHER VIEW's PANE: the blank row the
    /// pane opens with, plus the grid row above it. Registered ahead of
    /// the cards so a card ending on that row never swallows the grab.
    LauncherPaneSplitter,
    /// The `SESSION` tab at the head of the LAUNCHER VIEW's PANE: a click
    /// takes the pane off whichever TERMINAL it was reading and back onto
    /// the card under the cursor.
    LauncherPaneSession,
    /// A TERMINAL tab in that same header, by its place in
    /// `App::pane_terminals` — the current checkout's terminals, in tree
    /// order. A click reads that terminal in the pane.
    LauncherPaneTerminal(usize),
    /// The `×` on that tab. Its own target rather than a corner of the
    /// tab's, so a click on the cross can never read as a click on the
    /// tab it closes.
    LauncherPaneCloseTerminal(usize),
    /// The CLOSE BUTTON at the right end of that header: a click folds
    /// the pane away, the same `event_loop::launcher::toggle_pane` `^~`
    /// runs.
    LauncherPaneClose,
    /// The PR COUNT on the right of the LAUNCHER VIEW's header (`2 prs`):
    /// a click opens the open pull requests of the project in front of
    /// you — the modal `v` opens.
    LauncherPullRequests,
    /// The ISSUE COUNT beside it (`1 issue`): a click opens that
    /// project's open issues — the modal `i` opens.
    LauncherIssues,
    /// The key cap in the empty GRID's welcome (`press p to prompt`): a
    /// click opens the QUICK PROMPT, through the very
    /// `event_loop::launcher::open_box` the key runs.
    LauncherWelcomePrompt,
    /// The footer's right-edge readout (`2 agents · 1 term · 412 MB`): a
    /// click opens the memory modal — the one `⇧M` opens.
    FooterUsage,
}

/// Default widths of the Projects / Worktrees / Sessions panels. Sessions
/// is the widest because its rows carry the most: name, "23m ago", harness.
pub const DEFAULT_PANEL_WIDTHS: [u16; 3] = [20, 22, 32];
/// A panel can't be dragged narrower than this.
pub const MIN_PANEL_W: u16 = 10;
/// Width of a collapsed sidebar panel's rail: the column rule itself,
/// with the expand chevron drawn over it on the header row. Clickable
/// along its whole height. The panel's remembered width is untouched
/// while it is a rail, so expanding restores it.
pub const COLLAPSED_RAIL_W: u16 = 1;
/// The terminal pane always keeps at least this much width.
pub const MIN_TERM_W: u16 = 20;

/// Default outer width of the diff modal's file-list panel.
pub const DEFAULT_DIFF_FILES_W: u16 = 34;
/// The diff modal's file list can't be dragged narrower than this.
pub const MIN_DIFF_FILES_W: u16 = 16;
/// How long the settings overlay remembers its tab / row / strip-vs-list
/// after closing. Reopened within this, it lands where you left it; later
/// than this the memory is stale and it opens fresh on the tab strip.
pub const SETTINGS_MEMORY_TTL: std::time::Duration = std::time::Duration::from_secs(60);
/// The diff pane always keeps at least this much width.
pub const MIN_DIFF_PANE_W: u16 = 24;

/// What a PANE losing the keyboard says ([`App::release_terminal`]). The
/// input lock is what decides where a keystroke lands, so a drop the user
/// did not ask for silently changes what every key they type next MEANS —
/// `x` closes the project's tab, `⇧M` opens the memory modal — and the
/// one thing it must not be is quiet.
pub const TERMINAL_RELEASED: &str =
    "the keys are the grid's again — Enter steps back into the session";

// ---- list-view arithmetic shared by every overlay with a cursor ----

/// First visible row of a stateless follow-window over a list: the window
/// slides only as far as it must to keep `selected` on its last row. One
/// definition so every overlay list scrolls the same way.
pub fn window_start(selected: usize, height: usize) -> usize {
    (selected + 1).saturating_sub(height)
}

/// Clamp an absolute cursor request onto a list of `len` rows: negative
/// lands on the first row, past-the-end on the last, and an empty list on
/// 0 — the same rule every overlay applies before indexing.
pub fn clamp_selection(index: i64, len: usize) -> usize {
    let max = len.saturating_sub(1) as i64;
    index.clamp(0, max) as usize
}

/// Furthest a pane of `view_height` rows can scroll into `lines` lines —
/// the scroll that puts the last line on the bottom row. Zero when it all
/// fits. Shared by the diff pane and the tree preview.
pub fn max_scroll(lines: usize, view_height: u16) -> u16 {
    (lines as u16).saturating_sub(view_height.max(1))
}

/// `scroll` moved by `delta` and held within `0..=max`.
pub fn scrolled_by(scroll: u16, delta: i32, max: u16) -> u16 {
    (scroll as i32 + delta).clamp(0, max as i32) as u16
}

/// Width of a split modal's left list when its boundary is dragged to
/// screen column `boundary_x`, clamped so the list keeps `MIN_DIFF_FILES_W`
/// and the right pane keeps `MIN_DIFF_PANE_W`. `None` when `area` is too
/// narrow to honor both minimums — the caller leaves the width alone.
pub fn clamp_files_width(area: Rect, boundary_x: i32) -> Option<u16> {
    let max = area.width.saturating_sub(MIN_DIFF_PANE_W);
    if max < MIN_DIFF_FILES_W {
        return None; // modal too small to honor the minimums
    }
    let want = (boundary_x - area.x as i32).max(0) as u16;
    Some(want.clamp(MIN_DIFF_FILES_W, max))
}

// ---- overlays ----

#[derive(Debug, Clone, PartialEq)]
pub enum MenuAction {
    Attach(SessionRef),
    RestartAgent(AgentId),
    /// Queue a message on the row's Claude Cloud session
    /// (`ClientRequest::SendCloudMessage`), via a prompt.
    SendCloudMessage(AgentId),
    RenameAgent(AgentId),
    ArchiveAgent(AgentId),
    UnarchiveAgent(AgentId),
    DeleteAgent(AgentId),
    NewAgent(WorktreeId),
    /// Picker result: create an agent of this kind (chains into the NEW
    /// SESSION box — a PR SESSION into its name prompt). `model`/`effort`
    /// are submenu choices: None means the row
    /// hasn't drilled into that submenu (its configured default applies);
    /// "default" is the submenu row that picks the default explicitly.
    NewAgentOfKind {
        worktree: WorktreeId,
        kind: AgentKind,
        /// Registry id when `kind` is [`AgentKind::Custom`]; None for
        /// built-ins. Carried through the MODEL submenu, the QUICK PROMPT
        /// box and the launch draft into `CreateAgent::custom_harness`.
        custom: Option<String>,
        model: Option<String>,
        effort: Option<String>,
        /// One-shot launch modifier for Claude. The task itself is collected
        /// in the CLOUD TASK box and crosses IPC only on create.
        cloud: bool,
        /// OPEN PRS launch context (a PR SESSION): the pull request and
        /// the head branch its worktree is checked out on. Valid for every
        /// local kind, never with `cloud`, and preserved through
        /// model/effort submenus and the name prompt.
        pr: Option<crate::pull_request::PrLaunch>,
        /// Set when the picker was opened from the QUICK PROMPT's `Tab`:
        /// the box to put back (with its typed text) instead of creating a
        /// session. Rides through the MODEL / EFFORT submenus like
        /// `pr`, and is what an abandoned picker restores.
        quick: Option<Box<crate::quick_prompt::QuickReturn>>,
    },
    /// Shell terminal in the worktree's directory; created immediately with
    /// a default name (no prompt), renameable later.
    NewTerminal(WorktreeId),
    RenameTerminal(TerminalId),
    CloseTerminal(TerminalId),
    NewWorktree(ProjectId),
    /// Hand a link row's URL to the browser.
    OpenLink(String),
    /// Read the selected open pull request's diff in the diff modal. Carries
    /// no id: the row is the selection, and the fetch reads it back off the
    /// cursor the same way `g` does.
    ViewPrDiff,
    /// Open the COMMENT BOX on the selected pull request. Like
    /// `ViewPrDiff`, carries no id: the row is the selection, and `y`
    /// reads it off the cursor the same way.
    CommentPullRequest,
    /// Expand the selected session card into its FOLLOW-UP COMPOSER, or
    /// fold it back up. Carries no id for the same reason `ViewPrDiff`
    /// doesn't: the card is the selection, and Space reads it off the
    /// cursor the same way.
    FollowUp,
    EditLink(LinkId),
    DeleteLink(LinkId),
    DeleteWorktree(WorktreeId),
    /// The ROOT WORKTREE row's menu: open the BRANCH SWITCHER on it.
    SwitchBranch(WorktreeId),
    /// Start the worktree's RUN COMMAND, or stop it while it runs (`r`).
    ToggleRun(WorktreeId),
    /// Fire the worktree's OPEN COMMAND (`Shift+Enter`).
    OpenWorktree(WorktreeId),
    AddProject,
    RemoveProject(ProjectId),
    /// Retitle a project's row. Display only — the folder keeps its name and
    /// stays visible under the new one.
    RenameProject(ProjectId),
    /// PROJECT DROPDOWN row (the `+` in front of the LAUNCHER VIEW's
    /// PROJECT TABS): open this project — its sessions, and a tab for it
    /// first, next to the `+`, if it had none.
    OpenProject(ProjectId),
    ToggleArchived,
    /// Fold / unfold the PROJECT OPEN PRS GROUP (Worktrees panel menu).
    ToggleOpenPrs,
    /// Fold / unfold the PROJECT ISSUES GROUP (Worktrees panel menu).
    ToggleIssues,
    /// Flip the `hide_draft_prs` SETTING from the Worktrees panel menu:
    /// drafts out of the group and `/`, or back in.
    ToggleDraftPrs,
    /// A row of the WORKTREE PICKER a click on the NEW SESSION box's branch
    /// opens: aim this one launch at `target` — one of the project's
    /// checkouts, or a fresh worktree — and hand the box back with its
    /// text. It picks where the session runs, never what branch a checkout
    /// is on: that is the BRANCH SWITCHER's (`SwitchBranch`).
    PickLaunchWorktree {
        target: crate::quick_prompt::QuickTarget,
        back: Box<crate::quick_prompt::QuickReturn>,
    },
}

/// Which submenu → (right arrow) opens from a menu row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubmenuKind {
    /// Model list for a Claude/Codex session (new-session picker rows).
    Models,
    /// Effort list, offered once a model row is highlighted.
    Efforts,
}

impl MenuAction {
    /// The submenu this action's row expands into, if any. Drives both the
    /// `▸` indicator and the → key. New-session rows drill kind → model →
    /// effort; a row that already carries an effort is a leaf, and so is
    /// a model row whose effort list is empty (a Cursor family with no
    /// effort variants, like `auto`).
    pub fn submenu(&self) -> Option<SubmenuKind> {
        match self {
            MenuAction::NewAgentOfKind {
                kind,
                custom,
                model,
                effort,
                ..
            } => {
                if crate::config::model_choices(*kind, custom.as_deref()).is_empty() {
                    return None;
                }
                match (model, effort) {
                    (None, None) => Some(SubmenuKind::Models),
                    (Some(m), None)
                        if !crate::config::effort_choices(*kind, Some(m), custom.as_deref())
                            .is_empty() =>
                    {
                        Some(SubmenuKind::Efforts)
                    }
                    _ => None,
                }
            }
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct MenuItem {
    pub label: String,
    pub action: MenuAction,
    pub destructive: bool,
}

impl MenuItem {
    /// A plain menu row.
    pub fn new(label: impl Into<String>, action: MenuAction) -> Self {
        Self {
            label: label.into(),
            action,
            destructive: false,
        }
    }

    /// A row drawn in the warning color: it deletes, closes, or removes.
    pub fn destructive(label: impl Into<String>, action: MenuAction) -> Self {
        Self {
            label: label.into(),
            action,
            destructive: true,
        }
    }
}

#[derive(Debug, Clone)]
pub struct ContextMenu {
    /// Optional title rendered in the border (used by picker-style menus).
    pub title: Option<String>,
    pub items: Vec<MenuItem>,
    /// Anchor position for context menus; `None` centers the menu in the
    /// frame (used by picker-style menus opened from the keyboard).
    pub at: Option<(u16, u16)>,
    pub hover: usize,
    /// Set during draw for click hit-testing.
    pub area: Rect,
    /// The menu ← returns to when this one is a submenu.
    pub parent: Option<Box<ContextMenu>>,
    /// Type-ahead over the rows — set on the MODEL / EFFORT submenus, where
    /// letters filter instead of moving (↑/↓ move there); None elsewhere.
    pub filter: Option<MenuFilter>,
}

/// A picker submenu's type-ahead: the query typed so far and the full row
/// set it narrows (`items` holds the visible subset, best match first).
#[derive(Debug, Clone)]
pub struct MenuFilter {
    pub query: String,
    pub all: Vec<MenuItem>,
}

impl ContextMenu {
    /// Narrow the rows to `query` (fuzzy, best match first; the full list
    /// in its own order when empty, hovering the ✓ row). Returns false —
    /// and changes nothing — when no row matches, so the list never
    /// empties; a no-op on a menu without a filter.
    pub fn set_filter(&mut self, query: &str) -> bool {
        let Some(filter) = &self.filter else {
            return false;
        };
        let labels: Vec<&str> = filter.all.iter().map(|i| i.label.as_str()).collect();
        let ranked = crate::fuzzy::rank(query, labels.iter().copied());
        if ranked.is_empty() {
            return false;
        }
        let items: Vec<MenuItem> = ranked.iter().map(|(i, _)| filter.all[*i].clone()).collect();
        self.hover = if query.trim().is_empty() {
            items
                .iter()
                .position(|i| i.label.ends_with(" ✓"))
                .unwrap_or(0)
        } else {
            0
        };
        self.items = items;
        if let Some(filter) = &mut self.filter {
            filter.query = query.to_string();
        }
        true
    }

    /// Append one typed character to the filter; false when it would leave
    /// no rows (nothing changes).
    pub fn type_filter(&mut self, c: char) -> bool {
        let query = format!("{}{c}", self.filter_query());
        self.set_filter(&query)
    }

    /// Backspace: drop the last character (widens, so it always succeeds).
    pub fn pop_filter(&mut self) {
        let mut query = self.filter_query().to_string();
        query.pop();
        self.set_filter(&query);
    }

    pub fn filter_query(&self) -> &str {
        self.filter.as_ref().map_or("", |f| f.query.as_str())
    }

    pub fn has_filter_text(&self) -> bool {
        !self.filter_query().is_empty()
    }

    /// Is this the LAUNCHER VIEW's PROJECT DROPDOWN? Its rows are
    /// OpenProject, and it gates its footer hint.
    pub fn is_project_picker(&self) -> bool {
        self.items
            .iter()
            .any(|i| matches!(i.action, MenuAction::OpenProject(_)))
    }

    /// Is this the WORKTREE PICKER a click on the NEW SESSION box's branch
    /// opens? It is drawn hanging from that branch.
    pub fn is_launch_worktree_picker(&self) -> bool {
        self.items
            .iter()
            .any(|i| matches!(i.action, MenuAction::PickLaunchWorktree { .. }))
    }

    /// Cloud mode is a root-picker modifier, not another agent kind.
    /// Returning Some only while the Claude row itself is highlighted
    /// keeps Tab free everywhere else (including model/effort submenus).
    /// Two pickers offer it: the NEW SESSION PICKER (its `"New session"`
    /// title is the gate — the PR SESSION picker and a PR row's menu share
    /// these rows but never launch cloud, the daemon refusing a PR launch
    /// with a cloud task) and the QUICK PROMPT's `Tab` picker, whose pick
    /// makes the box a cloud one — unless the box is for an issue or a PR
    /// (`QuickLaunch::takes_cloud`).
    pub fn hovered_claude_cloud(&self) -> Option<bool> {
        if self.parent.is_some() {
            return None;
        }
        match &self.items.get(self.hover)?.action {
            MenuAction::NewAgentOfKind {
                kind: AgentKind::Claude,
                custom: None,
                cloud,
                pr: None,
                quick,
                ..
            } => {
                let offered = match quick {
                    Some(back) => back.launch.takes_cloud(),
                    None => self.title.as_deref() == Some("New session"),
                };
                offered.then_some(*cloud)
            }
            _ => None,
        }
    }

    /// The harnessed launch under the cursor, if the hovered row starts
    /// one: the New session picker, its PR sibling, the quick prompt
    /// picker, and their model/effort submenus all carry it. Gates the
    /// `?` jump to agent settings.
    pub fn hovered_agent_kind(&self) -> Option<(AgentKind, Option<String>)> {
        match &self.items.get(self.hover)?.action {
            MenuAction::NewAgentOfKind { kind, custom, .. } => Some((*kind, custom.clone())),
            _ => None,
        }
    }

    /// Toggle the highlighted Claude row and keep the state visible in the
    /// label. False means Tab did not belong to this menu/row.
    pub fn toggle_hovered_claude_cloud(&mut self) -> bool {
        if self.hovered_claude_cloud().is_none() {
            return false;
        }
        let item = &mut self.items[self.hover];
        let MenuAction::NewAgentOfKind { cloud, .. } = &mut item.action else {
            unreachable!("hovered_claude_cloud checked the action")
        };
        *cloud = !*cloud;
        item.label = if *cloud {
            "Claude · cloud".into()
        } else {
            "Claude".into()
        };
        true
    }
}

/// Destructive action waiting behind a confirmation.
#[derive(Debug, Clone, PartialEq)]
pub enum PendingAction {
    /// AddProject aimed at a path that doesn't exist yet: create the
    /// directory (daemon-side, `git init` per its config) and add it.
    CreateProjectDir(std::path::PathBuf),
    /// `a` (or the row menu's Archive) with the `confirm_on_archive`
    /// SETTING on: archive the agent once the dialog is answered.
    ArchiveAgent(AgentId),
    DeleteAgent(AgentId),
    CloseTerminal(TerminalId),
    DeleteWorktree(WorktreeId),
    /// Shift+D: every deletable worktree of the selected project.
    DeleteAllWorktrees(Vec<WorktreeId>),
    /// Shift+D: every session row the panel currently shows — agents and
    /// terminals both.
    DeleteAllSessions {
        agents: Vec<AgentId>,
        terminals: Vec<TerminalId>,
    },
    RemoveProject(ProjectId),
    DeleteLink(LinkId),
    /// `d` in the AGENT PRESETS list: drop the preset at `index` from the
    /// store. Both answers reopen the list for `worktree` — as the QUICK
    /// PROMPT picker for `quick`'s box when it was one — so the modal the
    /// confirm evicted comes back where the user left it.
    DeleteAgentPreset {
        index: usize,
        worktree: WorktreeId,
        quick: Option<Box<crate::quick_prompt::QuickReturn>>,
    },
    /// `R` in the settings overlay: rewrite config.json from the defaults
    /// (every setting and every hotkey), then reopen the overlay on them.
    ResetSettings,
    Quit,
}

#[derive(Debug, Clone)]
pub struct ConfirmDialog {
    pub title: String,
    pub message: String,
    pub action: PendingAction,
    /// Full dialog rect, written back during draw (the `ContextMenu::area`
    /// pattern) so a click outside it can cancel like Esc.
    pub area: Rect,
}

/// The `?` keymap overlay. Carries nothing but its drawn rect, so a click
/// outside the box can close it.
#[derive(Debug, Clone, Default)]
pub struct HelpView {
    pub area: Rect,
}

#[derive(Debug, Clone, PartialEq)]
pub enum PromptKind {
    AddProject,
    NewWorktree {
        project: ProjectId,
        /// Random `<adj>-<noun>-<verb>` name minted when the prompt
        /// opened; shown in the label and used when Enter arrives on an
        /// empty input, so the name offered is the name created.
        suggestion: String,
    },
    /// Final task input for a one-shot `claude --cloud <task>` launch,
    /// opened straight from the picker's `Claude · cloud` row. Multi-row:
    /// Shift+Enter inserts task newlines where the one-line prompts submit.
    ClaudeCloudTask {
        worktree: WorktreeId,
        name: String,
        model: Option<String>,
        effort: Option<String>,
    },
    /// The task for an AGENT PRESET launch: Enter composes
    /// `prefix + task + postfix` into the CLI's starting prompt. Multi-row
    /// like the cloud task — a framed request is rarely one line.
    AgentPresetTask {
        worktree: WorktreeId,
        preset: crate::agent_presets::AgentPreset,
    },
    /// The QUICK PROMPT's task: one multi-row box, opened by its hotkey
    /// from anywhere, that launches an AGENT in the selected WORKTREE with
    /// the typed text as its STARTING PROMPT. It carries the whole launch
    /// spec, resolved when the dialog opens so the title can show what
    /// Enter is about to start — and rewritten in place by the box's `Tab`
    /// / `Shift+Tab` pickers. The NEW SESSION PICKER never ends here: its
    /// pick creates the session outright.
    QuickPrompt(crate::quick_prompt::QuickLaunch),
    /// A message to queue on a row's Claude Cloud session
    /// (`claude -p <message> --cloud <id>`). Multi-row like the launch task:
    /// steering a cloud agent is rarely one line.
    CloudMessage {
        id: AgentId,
    },
    /// The next turn for a session already running, typed into a small
    /// modal and sent straight down that session's PTY — the LAUNCHER
    /// VIEW's follow-up, where the SESSIONS PANEL expands the card itself
    /// ([`App::follow_up`]). A grid of fixed-height cards has nowhere to
    /// grow a box, and a modal is what lets one card after another be
    /// prompted without ever stepping into a session. Multi-row: a turn
    /// is usually one line, but never only one line.
    FollowUp {
        id: AgentId,
    },
    /// A comment to post on the pull request under the cursor — a
    /// PROJECT OPEN PRS GROUP row or the Sessions panel's PR ROW — with
    /// `gh pr comment` (`y`, or **Comment…** from the row's menu).
    /// Multi-row like the cloud message: a review note is rarely one
    /// line, and it is markdown. Carries the pull request the box was
    /// opened on, so Enter posts where the title said it would.
    PrComment {
        number: u64,
        url: String,
        /// Row text, `#42 title` — what the box is titled with.
        label: String,
        /// The PULL REQUESTS MODAL the box stood in for (`c` there): Enter,
        /// Esc and an empty box all put it back on its row. None from the
        /// panels, where the box closes onto them. Boxed: the view is
        /// several times the size of the other variants.
        back: Option<Box<crate::pr_modal::PullRequestsView>>,
    },
    RenameAgent {
        id: AgentId,
    },
    RenameTerminal {
        id: TerminalId,
    },
    /// Retitle a project's row. The folder on disk is untouched; an empty
    /// name puts the row back on the folder's own name.
    RenameProject {
        id: ProjectId,
    },
    /// A typed SETTINGS OVERLAY row (`SettingKind::is_text`), opened by
    /// Enter on it: the one-line prompt stands in for the overlay while the
    /// value is typed, and both Enter and Esc put the overlay back on the
    /// row it left. An empty value is the row's default, not a cancel.
    /// `project` is the repo path of the project a PROJECT TAB row
    /// (`SettingKind::is_project`) was opened on, so the value lands in
    /// that project's entry even if the selection moves meanwhile; None
    /// for a top-level row.
    SettingText {
        kind: crate::config::SettingKind,
        project: Option<std::path::PathBuf>,
    },
    /// Rewrite a pinned link's URL.
    EditLink {
        id: LinkId,
    },
    /// `c` in the ISSUES MODAL: a comment to post on the issue under the
    /// cursor, as you, with `gh issue comment`. Multi-row like the tasks —
    /// a comment is rarely one line. Enter posts it off the loop and puts
    /// the modal back on its row while the answer lands; Esc, or an empty
    /// box, puts the modal back without posting.
    IssueComment {
        view: crate::issues::IssuesView,
        issue: crate::issues::IssueRef,
    },
}

impl PromptKind {
    /// Does this box's text become a turn for an agent on this machine —
    /// one that can open a file path written into it? A cloud session
    /// can't, and the rest aren't prompts at all.
    pub fn reaches_local_agent(&self) -> bool {
        matches!(
            self,
            PromptKind::QuickPrompt(_)
                | PromptKind::AgentPresetTask { .. }
                | PromptKind::FollowUp { .. }
        )
    }

    /// The checkout this box is addressed to — the one its Enter launches
    /// into — to rewrite when a stand-in becomes the real row. None for a
    /// box that names no checkout, and for a QUICK PROMPT about to cut
    /// one of its own.
    pub fn worktree_mut(&mut self) -> Option<&mut WorktreeId> {
        match self {
            PromptKind::ClaudeCloudTask { worktree, .. }
            | PromptKind::AgentPresetTask { worktree, .. } => Some(worktree),
            PromptKind::QuickPrompt(launch) => launch.worktree_mut(),
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct PromptDialog {
    pub title: String,
    pub label: String,
    pub input: TextInput,
    pub kind: PromptKind,
    /// Live directory listing under the input (path prompts only): the
    /// typed parent's subdirectories narrowed by the partial segment.
    pub dirs: Vec<crate::completion::DirEntry>,
    /// Listing row highlighted by ↓↑; None = the typed path itself.
    pub hover: Option<usize>,
    /// Screen rect of the listing rows, written during draw for click
    /// hit-testing.
    pub list_area: Rect,
    /// Full dialog rect, written during draw so a click outside it can
    /// abandon the prompt like Esc.
    pub area: Rect,
    /// The text inside a task box's editor (multi-row prompts only),
    /// written during draw: a click there puts the caret where it points,
    /// and the wheel over it scrolls the text.
    pub editor_area: Rect,
    /// The `[ ] new worktree ^N` toggle on the LAUNCHER VIEW box's prompt
    /// header, written during draw: a click there flips the launch, the
    /// same as `^N`. Empty on every other box, and an empty rect contains
    /// no point — so no other box has to know about it.
    pub toggle_area: Rect,
    /// The box's four details — `project ^P`, `worktree main ▾`, `agent
    /// Tab`, `model ^O` — and the columns each was drawn in, written
    /// during draw: a click on one opens the picker its chord opens, the
    /// branch the WORKTREE PICKER. Empty on every other box, and on a box
    /// too narrow to draw a field at all.
    pub detail_areas: Vec<(crate::launcher::BoxField, Rect)>,
    /// The branch alone in that row's `worktree main ▾`, written during
    /// draw: the WORKTREE PICKER hangs from it. Empty wherever the branch
    /// is not drawn or names nothing to pick — a PR SESSION's checkout is
    /// the DAEMON's.
    pub branch_area: Rect,
}

impl PromptDialog {
    pub fn new(
        title: impl Into<String>,
        label: impl Into<String>,
        input: impl Into<String>,
        kind: PromptKind,
    ) -> Self {
        let mut prompt = Self {
            title: title.into(),
            label: label.into(),
            input: TextInput::with_text(input),
            kind,
            dirs: Vec::new(),
            hover: None,
            list_area: Rect::default(),
            area: Rect::default(),
            editor_area: Rect::default(),
            toggle_area: Rect::default(),
            detail_areas: Vec::new(),
            branch_area: Rect::default(),
        };
        // The task and comment boxes hold line breaks; the rest are one
        // line. The field itself then knows which keys break a line and
        // whether a paste keeps its newlines.
        let multiline = prompt.is_multiline();
        prompt.input.set_multiline(multiline);
        prompt.refresh_dirs();
        prompt
    }

    /// Does Tab complete filesystem paths in this prompt?
    pub fn completes_paths(&self) -> bool {
        matches!(self.kind, PromptKind::AddProject)
    }

    /// The task prompts — the Claude Cloud launch task, a message to a live
    /// cloud session, an AGENT PRESET's task and the QUICK PROMPT — and an
    /// issue comment are the ones with a multi-row editor.
    pub fn is_multiline(&self) -> bool {
        matches!(
            self.kind,
            PromptKind::ClaudeCloudTask { .. }
                | PromptKind::CloudMessage { .. }
                | PromptKind::FollowUp { .. }
                | PromptKind::PrComment { .. }
                | PromptKind::AgentPresetTask { .. }
                | PromptKind::QuickPrompt { .. }
                | PromptKind::IssueComment { .. }
        )
    }

    fn home() -> Option<std::path::PathBuf> {
        nebula_core::env::home_dir()
    }

    /// Recompute `dirs` from `input` after any edit; the hover returns to
    /// the input row. Non-path prompts keep an empty listing.
    pub fn refresh_dirs(&mut self) {
        self.hover = None;
        self.dirs = if self.completes_paths() {
            crate::completion::list_dirs(&self.input, Self::home().as_deref())
        } else {
            Vec::new()
        };
    }

    /// Full path of the hovered listing row (typed parent + entry name).
    pub fn hovered_path(&self) -> Option<String> {
        let entry = self.dirs.get(self.hover?)?;
        let (parent, _) = crate::completion::split_input(&self.input);
        Some(format!("{parent}{}", entry.name))
    }

    /// ↓↑ over the listing; Up from the first row returns to the input.
    pub fn move_hover(&mut self, delta: i32) {
        if self.dirs.is_empty() {
            return;
        }
        let next = self.hover.map_or(-1, |h| h as i32) + delta;
        self.hover = (next >= 0).then(|| (next as usize).min(self.dirs.len() - 1));
    }

    /// → (or a click) on listing row `i`: step into that directory.
    pub fn dive(&mut self, i: usize) {
        let Some(entry) = self.dirs.get(i) else {
            return;
        };
        let (parent, _) = crate::completion::split_input(&self.input);
        self.input.set_text(format!("{parent}{}/", entry.name));
        self.refresh_dirs();
    }

    /// ← steps up: a typed partial segment is cleared first; from a bare
    /// "dir/" the last segment is dropped. "~/" expands so browsing keeps
    /// working above the home directory.
    pub fn ascend(&mut self) {
        let (parent, partial) = crate::completion::split_input(&self.input);
        if !partial.is_empty() {
            let parent = parent.to_string();
            self.input.set_text(parent);
            self.refresh_dirs();
            return;
        }
        let mut path = self.input.to_string();
        if path == "~/" {
            match Self::home() {
                Some(home) => path = format!("{}/", home.display()),
                None => return,
            }
        }
        if path.len() <= 1 {
            return; // "" or "/" — nowhere further up
        }
        path.pop(); // the trailing '/'
        let cut = path.rfind('/').map(|i| i + 1).unwrap_or(0);
        path.truncate(cut);
        self.input.set_text(path);
        self.refresh_dirs();
    }

    /// First visible listing row of the stateless follow-window for a list
    /// `height` rows tall.
    pub fn window_start(&self, height: usize) -> usize {
        self.hover.map_or(0, |h| h + 1).saturating_sub(height)
    }
}

/// One visible row of the diff-view file list: an index into `files` plus
/// the char positions of `path` the filter matched, for highlighting.
#[derive(Debug, Clone)]
pub struct DiffMatch {
    pub file: usize,
    pub positions: Vec<usize>,
}

/// Full-screen git-diff viewer: file list left, scrollable diff right.
#[derive(Debug, Clone)]
pub struct DiffView {
    /// Checkout dir the diffs are read from.
    pub root: PathBuf,
    /// Branch name for the pane title.
    pub branch: String,
    pub files: Vec<DiffFile>,
    /// Type-to-filter query over `files` paths; always live.
    pub filter: TextInput,
    /// Visible rows: `files` narrowed by `filter`, best matches first
    /// (git order when the filter is empty); reviewed ✓ files always sink
    /// to the bottom.
    pub matches: Vec<DiffMatch>,
    /// Index into `matches` (not `files`).
    pub selected: usize,
    /// Diff text of the selected file (reloaded on selection change).
    pub diff: String,
    /// Cached line count of `diff`, for scroll clamping.
    pub diff_line_count: usize,
    /// Top visible diff line.
    pub scroll: u16,
    /// Inner height of the diff pane, written back during draw (the
    /// `ContextMenu::area` pattern) so paging and clamping track resizes.
    pub view_height: u16,
    /// Screen rect of the file-list rows (filter row excluded), written back
    /// during draw so clicks can hit-test rows.
    pub list_area: Rect,
    /// Full modal rect, written back during draw; bounds the file-panel
    /// splitter drag and hit-tests its border.
    pub area: Rect,
    /// Outer width of the file-list panel; drag the panel border to resize.
    pub files_width: u16,
    /// In-progress drag of the files/diff border: `boundary_x - grab column`
    /// at mouse-down (the `SplitterDrag::grab_offset` pattern).
    pub files_drag: Option<i32>,
    /// Whether the repo has a commit; picks the diff command.
    pub head_ok: bool,
    /// Per-file diff text when this view is showing something git can't be
    /// asked for file by file — a pull request, whose whole diff arrives in
    /// one `gh pr diff`. `None` is the ordinary worktree view, which shells
    /// out per file. Its presence also turns OFF reviewed-mark persistence:
    /// marks are stored under the worktree path and pruned when that path
    /// isn't a directory, and a pull request has no path of its own.
    pub prefetched: Option<HashMap<String, String>>,
    /// The pull request a prefetched view is showing, by URL — what a
    /// fresh `gh pr diff` landing later checks before replacing the
    /// contents of a modal opened on the cached copy. `None` for the
    /// worktree view.
    pub pr_url: Option<String>,
    /// Reviewed ✓ marks: file path → fingerprint of the approved diff text.
    /// Nebula-side bookkeeping only (persisted via `review::store_marks`);
    /// never stages or otherwise touches git state.
    pub reviewed: HashMap<String, u64>,
    /// HEAD OID the marks are scoped to (empty on an unborn HEAD). A moved
    /// HEAD — commit, checkout — resets the worktree's marks on next open.
    pub head_key: String,
    /// BACKGROUND READS: with it, the file list and every file's diff are
    /// read off the loop (`git_diff::load_selected_diff`); without (a view
    /// built by a test, a pull request's prefetched view), inline.
    pub jobs: Option<crate::view_jobs::Jobs>,
    /// This view's own ticket, carried by every diff it asks for, so text
    /// read for an earlier modal never lands in this one's cache.
    pub id: u64,
    /// The `git status` this view opened ahead of, by ticket: the list is
    /// empty and says `reading changes…` until the listing lands.
    pub listing: Option<u64>,
    /// The selected file's diff in flight, by ticket. `diff` keeps the text
    /// it had meanwhile (`view_jobs::STALE_GRACE`) — see `shown`.
    pub waiting: Option<u64>,
    /// The file `diff` is the diff of. Differs from the selected file while
    /// that one's read is in flight, which is when a reviewed ✓ — a
    /// fingerprint of the text on screen — must not be taken.
    pub shown: Option<String>,
    /// Diffs read while this modal has been open, newest last: walking
    /// back onto a file shows it on the keypress (and re-reads it behind,
    /// so an agent's edit meanwhile still shows up), and the row after the
    /// cursor is read ahead. Bounded by [`DIFF_CACHE_BYTES`]; gone with
    /// the modal.
    pub cache: Vec<(String, std::sync::Arc<str>)>,
    /// The file list folded into a directory tree (`Ctrl+t`, see
    /// `diff_tree`); `None` is the flat list. While it is up the cursor is
    /// the tree's — `selected` and `matches` stay current underneath, so
    /// toggling back lands on a list that is already right.
    pub tree: Option<crate::diff_tree::DiffTree>,
}

/// The most diff text a DIFF VIEWER keeps beyond the one on screen. Two
/// megabytes is a few hundred ordinary files' worth, and an entry over
/// [`DIFF_CACHE_ENTRY_MAX`] is never kept: re-reading one huge diff is
/// cheaper than holding it.
pub const DIFF_CACHE_BYTES: usize = 2 * 1024 * 1024;
pub const DIFF_CACHE_ENTRY_MAX: usize = 512 * 1024;

impl DiffView {
    /// A view up before its file list is: `g` opens this at once and
    /// `event_loop::land_view_answer` fills it when `git status` answers.
    pub fn opening(
        root: PathBuf,
        branch: String,
        jobs: crate::view_jobs::Jobs,
        listing: u64,
    ) -> Self {
        let mut view = Self::new(root, branch, Vec::new(), true);
        view.jobs = Some(jobs);
        view.listing = Some(listing);
        view
    }

    /// The cached diff of `path`, if this modal has read it.
    pub fn cached(&self, path: &str) -> Option<std::sync::Arc<str>> {
        self.cache
            .iter()
            .find(|(p, _)| p == path)
            .map(|(_, text)| text.clone())
    }

    /// Keep `diff` as the diff of `path`, dropping the oldest entries to
    /// stay inside the budget.
    pub fn cache_put(&mut self, path: &str, diff: &str) {
        self.cache.retain(|(p, _)| p != path);
        if diff.len() > DIFF_CACHE_ENTRY_MAX {
            return;
        }
        self.cache.push((path.to_string(), diff.into()));
        let mut held: usize = self.cache.iter().map(|(_, text)| text.len()).sum();
        while held > DIFF_CACHE_BYTES && self.cache.len() > 1 {
            held -= self.cache.remove(0).1.len();
        }
    }

    /// Put `diff` on screen as the diff of `path`. `keep_scroll` is a
    /// re-read of the file already showing: the reader's place is kept.
    pub fn show_diff(&mut self, path: Option<&str>, diff: String, keep_scroll: bool) {
        self.diff_line_count = diff.lines().count();
        self.diff = diff;
        self.shown = path.map(str::to_string);
        if !keep_scroll {
            self.scroll = 0;
        }
    }

    pub fn new(root: PathBuf, branch: String, files: Vec<DiffFile>, head_ok: bool) -> Self {
        let mut view = Self {
            root,
            branch,
            files,
            filter: TextInput::new(),
            matches: Vec::new(),
            selected: 0,
            diff: String::new(),
            diff_line_count: 0,
            scroll: 0,
            view_height: 0,
            list_area: Rect::default(),
            area: Rect::default(),
            files_width: DEFAULT_DIFF_FILES_W,
            files_drag: None,
            head_ok,
            prefetched: None,
            pr_url: None,
            reviewed: HashMap::new(),
            head_key: String::new(),
            jobs: None,
            id: crate::view_jobs::ticket(),
            listing: None,
            waiting: None,
            shown: None,
            cache: Vec::new(),
            tree: None,
        };
        view.apply_filter();
        view
    }

    pub fn max_scroll(&self) -> u16 {
        max_scroll(self.diff_line_count, self.view_height)
    }

    /// Screen x of the files/diff boundary — the column where the diff panel
    /// starts.
    pub fn splitter_x(&self) -> u16 {
        self.area.x + self.files_width
    }

    /// Move the files/diff boundary to `boundary_x`, clamped so the file list
    /// keeps `MIN_DIFF_FILES_W` and the diff pane keeps `MIN_DIFF_PANE_W`.
    pub fn set_files_width(&mut self, boundary_x: i32) {
        if let Some(width) = clamp_files_width(self.area, boundary_x) {
            self.files_width = width;
        }
    }

    /// Clamped relative scroll.
    pub fn scroll_by(&mut self, delta: i32) {
        self.scroll = scrolled_by(self.scroll, delta, self.max_scroll());
    }

    /// Clamped absolute selection in the list on screen — the filtered
    /// files, or the tree's rows; true when it changed (the caller reloads
    /// the diff).
    pub fn select(&mut self, index: i64) -> bool {
        if let Some(tree) = &mut self.tree {
            return tree.select(index);
        }
        let clamped = clamp_selection(index, self.matches.len());
        let changed = clamped != self.selected;
        self.selected = clamped;
        changed
    }

    /// The cursor's row in the list on screen: an index into `matches`, or
    /// into the tree's rows.
    pub fn cursor(&self) -> usize {
        self.tree.as_ref().map_or(self.selected, |t| t.selected)
    }

    /// How many rows the list on screen has.
    pub fn row_count(&self) -> usize {
        self.tree
            .as_ref()
            .map_or(self.matches.len(), |t| t.rows.len())
    }

    /// The file behind the current selection, if any row is visible — and,
    /// in the tree, if that row is a file's.
    pub fn selected_file(&self) -> Option<&DiffFile> {
        match &self.tree {
            Some(tree) => self.files.get(tree.selected_file()?),
            None => self.files.get(self.matches.get(self.selected)?.file),
        }
    }

    /// The directory the tree's cursor rests on, as its path.
    pub fn selected_dir(&self) -> Option<&str> {
        let node = self.tree.as_ref()?.selected_node()?;
        node.is_dir.then_some(node.path.as_str())
    }

    /// The selected row's path: the file's, or a tree directory's.
    pub fn selected_path(&self) -> Option<&str> {
        self.selected_file()
            .map(|f| f.path.as_str())
            .or_else(|| self.selected_dir())
    }

    /// Put the cursor on the row for `path`, in whichever list is showing.
    /// False, and the cursor left alone, when no row has it.
    pub fn select_path(&mut self, path: &str) -> bool {
        if let Some(tree) = &mut self.tree {
            return tree.select_path(path, &self.filter);
        }
        let row = self
            .matches
            .iter()
            .position(|m| self.files[m.file].path == path);
        if let Some(row) = row {
            self.selected = row;
        }
        row.is_some()
    }

    /// First visible row of the file list's stateless follow-window for a
    /// list of `height` rows.
    pub fn window_start(&self, height: usize) -> usize {
        window_start(self.cursor(), height)
    }

    /// Whether the cursor is still where opening the modal put it: the top
    /// of the flat list, the tree's home row. A reader who has moved keeps
    /// the file they are on when a fresh listing lands (`git_diff::
    /// fill_view`); one who has not gets what a fresh open gives.
    pub fn at_home(&self) -> bool {
        match &self.tree {
            Some(tree) => tree.selected == tree.home_row(),
            None => self.selected == 0,
        }
    }

    /// The next file down from the cursor in whichever list is showing —
    /// what the DIFF VIEWER reads ahead, `↓` being the key it is walked
    /// with. Tree directories are stepped over: they have no diff to read.
    pub fn file_after_cursor(&self) -> Option<&DiffFile> {
        match &self.tree {
            Some(tree) => tree
                .rows
                .iter()
                .skip(tree.selected + 1)
                .find_map(|row| tree.file_of[row.node])
                .and_then(|file| self.files.get(file)),
            None => self.files.get(self.matches.get(self.selected + 1)?.file),
        }
    }

    /// Recompute the rows from `filter` and send the cursor home — the top
    /// row, or in the tree the best match (its first file when the filter
    /// is empty); true when that moved it off the row it was on (the caller
    /// reloads the diff).
    pub fn apply_filter(&mut self) -> bool {
        let before = self.matches.get(self.selected).map(|m| m.file);
        self.recompute_matches();
        self.selected = 0;
        match &mut self.tree {
            Some(tree) => tree.apply_filter(&self.filter),
            None => before != self.matches.first().map(|m| m.file),
        }
    }

    /// Swap in a new file list (a pull request's diff refreshed under the
    /// modal): both lists are rebuilt — the tree keeping what the reader
    /// folded — and the cursor goes home; the caller moves it back
    /// (`select_path`) and reloads the diff.
    pub fn replace_files(&mut self, files: Vec<DiffFile>) {
        self.files = files;
        self.recompute_matches();
        self.selected = 0;
        if let Some(tree) = &self.tree {
            self.tree = Some(tree.rebuilt(&self.files, &self.filter));
        }
    }

    /// `Ctrl+t`: the other shape of the file list — flat paths, or the
    /// directory tree (`diff_tree`). The cursor stays on the file it was on;
    /// leaving the tree from a directory row, that directory's first file
    /// stands in, since the flat list has no row for a directory. True when
    /// the diff pane has something else to show (the caller reloads it).
    pub fn toggle_tree(&mut self) -> bool {
        let before = self.selected_file().map(|f| f.path.clone());
        let target = match self.tree.take() {
            Some(tree) => before.clone().or_else(|| {
                let under = tree.files_under(tree.rows.get(tree.selected)?.node);
                Some(self.files[*under.first()?].path.clone())
            }),
            None => {
                self.tree = Some(crate::diff_tree::DiffTree::new(&self.files, &self.filter));
                before.clone()
            }
        };
        if let Some(path) = target {
            self.select_path(&path);
        }
        before.is_none() || before != self.selected_file().map(|f| f.path.clone())
    }

    /// `Enter` on a tree directory's row, or a click on it: fold or unfold
    /// it. True when that moved the cursor onto another row.
    pub fn toggle_dir(&mut self, row: usize) -> bool {
        match &mut self.tree {
            Some(tree) => tree.toggle_row(row, &self.filter),
            None => false,
        }
    }

    /// `→` in the tree: open the directory, or step into an open one. True
    /// when the cursor moved onto another row.
    pub fn expand_selected(&mut self) -> bool {
        match &mut self.tree {
            Some(tree) => tree.expand_selected(&self.filter),
            None => false,
        }
    }

    /// `←` in the tree: fold the directory, or jump to the parent's row.
    /// True when the cursor moved onto another row.
    pub fn collapse_selected(&mut self) -> bool {
        match &mut self.tree {
            Some(tree) => tree.collapse_selected(&self.filter),
            None => false,
        }
    }

    /// What the diff pane shows for a tree directory's row: the files that
    /// changed under it, each with its status code and its ✓.
    pub fn dir_summary(&self) -> Option<String> {
        let tree = self.tree.as_ref()?;
        let row = tree.rows.get(tree.selected)?;
        let dir = &tree.nodes[row.node];
        if !dir.is_dir {
            return None;
        }
        let under = tree.files_under(row.node);
        let plural = if under.len() == 1 { "" } else { "s" };
        let mut out = format!("{}/ — {} changed file{plural}\n", dir.path, under.len());
        for file in under.into_iter().map(|f| &self.files[f]) {
            let name = file
                .path
                .strip_prefix(dir.path.as_str())
                .map_or(file.path.as_str(), |rest| rest.trim_start_matches('/'));
            let mark = if self.reviewed.contains_key(&file.path) {
                "✓"
            } else {
                " "
            };
            out.push_str(&format!("\n{} {mark} {name}", file.status_str()));
        }
        Some(out)
    }

    /// Rebuild the visible rows from `filter` and the reviewed marks: best
    /// matches first (git order when the filter is empty), reviewed ✓ files
    /// stably sunk to the bottom. The selection index is left alone —
    /// callers reset or fix it up.
    pub fn recompute_matches(&mut self) {
        self.matches = crate::fuzzy::rank(&self.filter, self.files.iter().map(|f| f.path.as_str()))
            .into_iter()
            .map(|(file, positions)| DiffMatch { file, positions })
            .collect();
        let files = &self.files;
        let reviewed = &self.reviewed;
        self.matches
            .sort_by_key(|m| reviewed.contains_key(&files[m.file].path));
    }

    /// Toggle the reviewed ✓ on the selected file and re-sink reviewed
    /// files. Marking keeps the selection row — with the marked file sunk,
    /// that lands on the next file in the list; unmarking advances to the
    /// next still-reviewed file so repeated presses clear a batch of marks.
    /// Only when the last visible mark is cleared does the selection follow
    /// the file back to its natural spot. `None` when no row is selected,
    /// otherwise whether the selected file changed (the caller reloads the
    /// diff; it persists `reviewed` either way).
    ///
    /// The tree keeps its order — nothing sinks there — so the same sweep
    /// moves the cursor instead: down to the next file still in the state
    /// this one just left, staying put when there is none. A directory's
    /// row has no mark of its own to toggle.
    pub fn toggle_reviewed(&mut self) -> Option<bool> {
        let path = self.selected_file()?.path.clone();
        // The mark is a fingerprint of the diff that was read — not of
        // whatever the pane still holds while this file's is in flight.
        if self.waiting.is_some() && self.shown.as_deref() != Some(path.as_str()) {
            return None;
        }
        let before = self.matches.get(self.selected).map(|m| m.file);
        let unmarked = self.reviewed.remove(&path).is_some();
        if !unmarked {
            let mark = crate::review::fingerprint(&self.diff);
            self.reviewed.insert(path.clone(), mark);
        }
        self.recompute_matches();
        if let Some(tree) = &mut self.tree {
            let (files, reviewed) = (&self.files, &self.reviewed);
            let next = (tree.selected + 1..tree.rows.len()).find(|&row| {
                tree.file_of[tree.rows[row].node]
                    .is_some_and(|f| reviewed.contains_key(&files[f].path) == unmarked)
            });
            tree.selected = next.unwrap_or(tree.selected);
            return Some(next.is_some());
        }
        let marks_visible = self
            .matches
            .iter()
            .any(|m| self.reviewed.contains_key(&self.files[m.file].path));
        if unmarked && marks_visible {
            // The reviewed zone is contiguous at the bottom and the selection
            // sat inside it, so one row down is the next still-marked file.
            self.selected = (self.selected + 1).min(self.matches.len().saturating_sub(1));
        } else if unmarked {
            if let Some(pos) = self
                .matches
                .iter()
                .position(|m| self.files[m.file].path == path)
            {
                self.selected = pos;
            }
        } else {
            self.selected = self.selected.min(self.matches.len().saturating_sub(1));
        }
        Some(before != self.matches.get(self.selected).map(|m| m.file))
    }
}

// The `/` PALETTE lives in `palette.rs`; re-exported so the callers and
// tests that always reached it through `app::` keep working.
pub use crate::palette::{Palette, PaletteItem, PaletteMatch, PaletteTarget};

/// One visible row of the file finder: an index into `files` plus the char
/// positions of the path the query matched, for highlighting.
#[derive(Debug, Clone)]
pub struct FinderMatch {
    pub file: usize,
    pub positions: Vec<usize>,
}

/// Fuzzy file finder over every file of the selected worktree (`f`).
#[derive(Debug, Clone)]
pub struct FileFinder {
    /// Checkout dir the listing was read from.
    pub root: PathBuf,
    /// Branch name for the modal title.
    pub branch: String,
    /// Editor command Enter launches (NEBULA_EDITOR, then the `editor`
    /// setting, default vim), captured at open time.
    pub editor: String,
    /// Paths relative to `root`, in git listing order.
    pub files: Vec<String>,
    /// Type-to-filter query over `files`; always live.
    pub query: TextInput,
    /// Visible rows: `files` narrowed by `query`, best matches first
    /// (listing order when the query is empty).
    pub matches: Vec<FinderMatch>,
    /// Index into `matches` (not `files`).
    pub selected: usize,
    /// Whole modal rect, written back during draw so clicks outside close.
    pub area: Rect,
    /// Screen rect of the result rows (query row excluded), written back
    /// during draw so clicks can hit-test rows.
    pub list_area: Rect,
    /// The `git ls-files` this finder opened ahead of, by ticket
    /// (`view_jobs`): the list is empty and says `listing files…` until
    /// [`FileFinder::set_files`]. None once the listing is in hand — and
    /// always, for a finder built with its files.
    pub listing: Option<u64>,
}

impl FileFinder {
    pub fn new(root: PathBuf, branch: String, editor: String, files: Vec<String>) -> Self {
        let mut finder = Self {
            root,
            branch,
            editor,
            files,
            query: TextInput::new(),
            matches: Vec::new(),
            selected: 0,
            area: Rect::default(),
            list_area: Rect::default(),
            listing: None,
        };
        finder.apply_filter();
        finder
    }

    /// A finder up before its listing is: `f` opens this at once, and
    /// `set_files` fills it when `git ls-files` answers. What is typed
    /// meanwhile is kept, and narrows the list the moment there is one.
    pub fn opening(root: PathBuf, branch: String, editor: String, listing: u64) -> Self {
        let mut finder = Self::new(root, branch, editor, Vec::new());
        finder.listing = Some(listing);
        finder
    }

    /// The listing landed: rank it by whatever the query holds by now.
    pub fn set_files(&mut self, files: Vec<String>) {
        self.files = files;
        self.listing = None;
        self.apply_filter();
    }

    /// First visible row of the result list's stateless follow-window for a
    /// list of `height` rows.
    pub fn window_start(&self, height: usize) -> usize {
        window_start(self.selected, height)
    }

    /// Clamped absolute selection in the filtered list.
    pub fn select(&mut self, index: i64) {
        self.selected = clamp_selection(index, self.matches.len());
    }

    /// The path behind the current selection, if any row is visible.
    pub fn selected_path(&self) -> Option<&str> {
        self.files
            .get(self.matches.get(self.selected)?.file)
            .map(String::as_str)
    }

    /// Recompute `matches` from `query` and reset the selection to the top
    /// row. Best matches first, listing order when the query is empty.
    pub fn apply_filter(&mut self) {
        self.matches = crate::fuzzy::rank(&self.query, self.files.iter().map(String::as_str))
            .into_iter()
            .map(|(file, positions)| FinderMatch { file, positions })
            .collect();
        self.selected = 0;
    }
}

/// Find-in-files overlay (`F`): live `git grep` over the selected worktree.
#[derive(Debug, Clone)]
pub struct GrepView {
    /// Checkout dir the search runs in.
    pub root: PathBuf,
    /// Branch name for the modal title.
    pub branch: String,
    /// Editor command Enter launches (NEBULA_EDITOR, then the `editor`
    /// setting, default vim), captured at open time.
    pub editor: String,
    /// The search text; every edit re-runs the grep.
    pub query: TextInput,
    /// Current results, best-first in git grep order (path, then line).
    pub hits: Vec<crate::grep_search::GrepHit>,
    /// The search stopped at the result cap — the title says so.
    pub truncated: bool,
    /// A failed grep's message, shown in the list area until the next edit.
    pub error: Option<String>,
    /// Index into `hits`.
    pub selected: usize,
    /// Whole modal rect, written back during draw so clicks outside close.
    pub area: Rect,
    /// Screen rect of the result rows (query row excluded), written back
    /// during draw so clicks can hit-test rows.
    pub list_area: Rect,
    /// BACKGROUND READS: with it, a search runs off the loop and lands in
    /// [`GrepView::land`]; without (a view built by a test), inline.
    pub jobs: Option<crate::view_jobs::Jobs>,
    /// The search in flight, by ticket. The hits on screen meanwhile are the
    /// previous query's — the title says `searching…`.
    pub waiting: Option<u64>,
    /// Stops the search in flight when the query moves on.
    pub cancel: crate::view_jobs::Cancel,
}

impl GrepView {
    pub fn new(root: PathBuf, branch: String, editor: String) -> Self {
        Self {
            root,
            branch,
            editor,
            query: TextInput::new(),
            hits: Vec::new(),
            truncated: false,
            error: None,
            selected: 0,
            area: Rect::default(),
            list_area: Rect::default(),
            jobs: None,
            waiting: None,
            cancel: crate::view_jobs::Cancel::default(),
        }
    }

    /// Re-run the grep for the current query and reset the selection to the
    /// top row. Queries under `MIN_QUERY_LEN` just clear the results.
    pub fn run_search(&mut self) {
        self.selected = 0;
        self.error = None;
        // Whatever was being searched for is no longer the query.
        self.cancel.cancel();
        self.waiting = None;
        if self.query.chars().count() < crate::grep_search::MIN_QUERY_LEN {
            self.hits.clear();
            self.truncated = false;
            return;
        }
        let Some(jobs) = &self.jobs else {
            let result = crate::grep_search::search(&self.root, &self.query);
            self.show(result);
            return;
        };
        let ticket = crate::view_jobs::ticket();
        self.waiting = Some(ticket);
        self.cancel = crate::view_jobs::Cancel::default();
        let (root, query, cancel) = (
            self.root.clone(),
            self.query.to_string(),
            self.cancel.clone(),
        );
        jobs.run(move || {
            // The next character, typed at speed, cancels this before git
            // is ever started.
            std::thread::sleep(crate::view_jobs::GREP_DEBOUNCE);
            if cancel.is_cancelled() {
                return None;
            }
            let result = crate::grep_search::search_streaming(&root, &query, &cancel)?;
            Some(crate::view_jobs::Answer::Grep { ticket, result })
        });
    }

    /// A background search's answer: shown when it is the one being waited
    /// on, dropped when the query has moved on since.
    pub fn land(
        &mut self,
        ticket: u64,
        result: Result<(Vec<crate::grep_search::GrepHit>, bool), String>,
    ) {
        if self.waiting != Some(ticket) {
            return;
        }
        self.waiting = None;
        self.show(result);
    }

    fn show(&mut self, result: Result<(Vec<crate::grep_search::GrepHit>, bool), String>) {
        self.selected = 0;
        match result {
            Ok((hits, truncated)) => {
                self.hits = hits;
                self.truncated = truncated;
            }
            Err(msg) => {
                self.hits.clear();
                self.truncated = false;
                self.error = Some(msg);
            }
        }
    }

    /// First visible row of the result list's stateless follow-window for a
    /// list of `height` rows.
    pub fn window_start(&self, height: usize) -> usize {
        window_start(self.selected, height)
    }

    /// Clamped absolute selection.
    pub fn select(&mut self, index: i64) {
        self.selected = clamp_selection(index, self.hits.len());
    }

    /// The hit behind the current selection, if any row is visible.
    pub fn selected_hit(&self) -> Option<&crate::grep_search::GrepHit> {
        self.hits.get(self.selected)
    }
}

/// Recent-hosts modal (`h`): destinations remembered by `nebula ssh`.
/// Enter (or a click) quits the TUI and execs a fresh `nebula ssh` at the
/// selected entry; `a` types a new destination, `d` forgets one. The rows
/// are a snapshot loaded when the modal opens — nothing else writes the
/// list while the TUI is up.
#[derive(Debug, Clone)]
pub struct HostsView {
    pub hosts: Vec<crate::hosts::HostEntry>,
    /// Cursor into `hosts`.
    pub selected: usize,
    /// Active "connect to a new destination" input (`a`), if any — typed as
    /// `user@host [dir]`, Enter connects like a `nebula ssh` invocation.
    pub input: Option<TextInput>,
    /// Whole modal rect, written back during draw so clicks outside close.
    pub area: Rect,
    /// Screen rect of the host rows, written back during draw so clicks can
    /// hit-test rows.
    pub list_area: Rect,
}

impl HostsView {
    pub fn new(hosts: Vec<crate::hosts::HostEntry>) -> Self {
        Self {
            hosts,
            selected: 0,
            input: None,
            area: Rect::default(),
            list_area: Rect::default(),
        }
    }

    /// First visible row of the list's stateless follow-window for a list of
    /// `height` rows.
    pub fn window_start(&self, height: usize) -> usize {
        window_start(self.selected, height)
    }
}

/// A live rebind in the Hotkeys tab: the overlay is holding still, waiting
/// for the user to press the key they want.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HotkeyCapture {
    /// Index into [`crate::keymap::ACTIONS`].
    pub action: usize,
    /// Add the chord as an alternate instead of replacing the row's list.
    pub add: bool,
    /// A chord captured but held back because another action in the same
    /// scope already answers to it: Enter takes it anyway (and the other
    /// action loses it), Esc backs out. The `Vec` is the losers, for the
    /// warning text.
    pub pending: Option<(crate::keymap::KeyChord, Vec<usize>)>,
}

/// How loudly the line under the settings body is speaking.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoticeLevel {
    Info,
    Warn,
}

#[derive(Debug, Clone, Default)]
pub struct SettingsView {
    /// Index into [`crate::config::SETTINGS_TABS`].
    pub tab: usize,
    /// Cursor row *within the current tab*.
    pub selected: usize,
    /// The tab strip itself has the cursor: ←/→ walk the tabs, ↓ drops
    /// back into the list. Stepping up off the top row is what puts it
    /// here, so arrows can steer tabs without ever fighting the ←/→ that
    /// cycles a setting's value. An overlay opened for the first time
    /// starts here (see [`App::settings_on_tabs`]).
    pub on_tabs: bool,
    /// Set during draw for click hit-testing.
    pub area: Rect,
    /// Screen x-range of each tab label, written during draw so clicks on
    /// the strip land on the right tab.
    pub tab_hits: Vec<(u16, u16)>,
    /// Body rect and the first body row visible in it, written during draw
    /// so click hit-testing agrees with what's actually on screen.
    pub body_area: Rect,
    pub first_row: usize,
    /// Set while the Hotkeys tab is waiting for a key press.
    pub capture: Option<HotkeyCapture>,
    /// Transient line under the body: duplicate warnings, host-terminal
    /// warnings, "reset to default".
    pub notice: Option<(String, NoticeLevel)>,
}

impl SettingsView {
    /// `tab`/`selected`/`on_tabs` are the remembered cursor position
    /// (`App::settings_tab` / `App::settings_selected` /
    /// `App::settings_on_tabs`), clamped in case the lists shrank between
    /// builds.
    pub fn new(tab: usize, selected: usize, on_tabs: bool) -> Self {
        let tab = tab.min(crate::config::tab_count().saturating_sub(1));
        Self {
            tab,
            selected: selected.min(crate::config::tab_len(tab).saturating_sub(1)),
            on_tabs,
            ..Self::default()
        }
    }

    /// True while a key press should be captured as a binding rather than
    /// steering the overlay.
    pub fn capturing(&self) -> bool {
        self.capture.as_ref().is_some_and(|c| c.pending.is_none())
    }

    pub fn is_hotkeys(&self) -> bool {
        self.tab == crate::config::hotkeys_tab()
    }

    pub fn warn(&mut self, text: impl Into<String>) {
        self.notice = Some((text.into(), NoticeLevel::Warn));
    }

    pub fn info(&mut self, text: impl Into<String>) {
        self.notice = Some((text.into(), NoticeLevel::Info));
    }
}

/// Memory-usage modal (`M`): how much RAM nebula and every live session's
/// process tree (claude, codex, shells and their children) are using. The
/// daemon's half arrives async as `ServerEvent::Metrics`; the event loop
/// re-requests on a slow poll while the modal is open.
#[derive(Debug, Clone, Default)]
pub struct MetricsView {
    /// Last daemon reading; None until the first reply lands.
    pub snapshot: Option<nebula_core::MetricsSnapshot>,
    /// This TUI process's own RSS, sampled client-side with each request
    /// (the daemon can't see us — we're not its child).
    pub client_rss_bytes: u64,
    /// Cursor into `rows`; Enter opens the session under it.
    pub selected: usize,
    /// Scroll offset into the per-session rows, clamped during draw.
    pub scroll: usize,
    /// Display order of the rows, written back during draw so the key and
    /// mouse handlers agree with what's on screen. `None` = one of nebula's
    /// own processes (daemon / this UI) — selectable but not openable.
    pub rows: Vec<Option<SessionRef>>,
    /// Whole modal rect, written back during draw so clicks outside close.
    pub area: Rect,
    /// Screen rect of the session rows, written back during draw so clicks
    /// can hit-test rows.
    pub list_area: Rect,
}

impl MetricsView {
    pub fn new() -> Self {
        Self::default()
    }
}

#[derive(Debug, Clone)]
pub enum Overlay {
    Menu(ContextMenu),
    Confirm(ConfirmDialog),
    Prompt(PromptDialog),
    Help(HelpView),
    Settings(SettingsView),
    Diff(DiffView),
    Palette(Palette),
    Files(FileFinder),
    Grep(GrepView),
    Tree(crate::tree_browser::TreeBrowser),
    /// `nebula open <file>…` from a session: the FILE TABS.
    FileTabs(crate::file_tabs::FileTabsView),
    Metrics(MetricsView),
    Hosts(HostsView),
    /// `e` in the SESSIONS PANEL: the AGENT PRESETS list.
    AgentPresets(crate::preset_overlays::AgentPresetsView),
    /// The PRESET EDITOR form behind the list's `Ctrl+a` / `Ctrl+e`.
    AgentPresetEditor(crate::preset_overlays::AgentPresetEditor),
    /// `i`: the ISSUES MODAL — the project's open GitHub issues.
    Issues(crate::issues::IssuesView),
    /// `v`: the PULL REQUESTS MODAL — the project's open pull requests.
    PullRequests(crate::pr_modal::PullRequestsView),
    /// `c`: the BRANCH SWITCHER — the ROOT WORKTREE onto another branch.
    BranchSwitch(crate::branch_switch::BranchSwitchView),
    /// `^P` in the LAUNCHER VIEW's box: the PROJECT PICKER.
    ProjectPicker(crate::launcher::ProjectPicker),
}

/// Rows optimistically removed for an in-flight DeleteWorktree, kept so an
/// Error reply can put them back exactly where they were.
#[derive(Debug, Clone)]
pub struct WorktreeRollback {
    /// Index the worktree held in `tree.worktrees`.
    pub index: usize,
    pub worktree: Worktree,
    /// Its agents, each with the index it held in `tree.agents`.
    pub agents: Vec<(usize, Agent)>,
}

/// The rows a launch into a WORKTREE that does not exist yet puts up the
/// moment Enter is pressed — a checkout row and its one session row,
/// under ids this client made up — so the panels never wait on the
/// DAEMON's `git worktree add` and CLI spawn. A QUICK PROMPT's ride the
/// PENDING INTENTs of its two creates, a PR SESSION's the one
/// `CreatePrAgent` that cuts the checkout and spawns in it: the Acks turn
/// them into the real rows, an Error takes them down
/// (`event_loop::placeholder`). The NEW WORKTREE modal puts up the
/// checkout row alone, under `PendingIntent::SelectCreatedWorktree`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlaceholderRows {
    pub worktree: WorktreeId,
    pub agent: AgentId,
}

/// A `CreateAgent` (or `CreatePrAgent`) as a launch surface drafts it —
/// the NEW SESSION PICKER, the QUICK PROMPT, an AGENT PRESET's task box,
/// the ISSUES MODAL. `event_loop::create_agent` turns it into the request
/// and the PENDING INTENT that attaches the row; a draft aimed at a
/// stand-in checkout waits on that checkout's own intent instead
/// (`PendingIntent::SelectCreatedWorktree::launch`).
///
/// An empty `name` takes the generated default (agent-1, …) and opts the
/// session into agent-driven auto-titling (`nebula rename` on the first
/// prompt) — what every launch from the NEW SESSION PICKER and the QUICK
/// PROMPT does. A name a surface does set is the user's choice and stays.
#[derive(Debug, Clone)]
pub struct AgentLaunchDraft {
    pub worktree: WorktreeId,
    pub kind: AgentKind,
    /// Registry id when `kind` is [`AgentKind::Custom`].
    pub custom: Option<String>,
    pub model: Option<String>,
    pub effort: Option<String>,
    pub name: String,
    pub cloud_prompt: Option<String>,
    /// An AGENT PRESET launch's composed first prompt (see
    /// `ClientRequest::CreateAgent::starting_prompt`).
    pub starting_prompt: Option<String>,
    /// The prompt (and its typed text) to bring back should the daemon
    /// refuse the create — so a rejected preset task is not lost.
    pub reopen_on_error: Option<(PromptKind, String)>,
    /// OPEN PRS launch context: this launch is a PR SESSION, scoped to that
    /// pull request and run in a checkout of its head branch.
    pub pr: Option<crate::pull_request::PrLaunch>,
    /// ISSUES MODAL launch context: this launch is an ISSUE SESSION, and
    /// the DAEMON persists the URL as the row's context (see
    /// `ClientRequest::CreateAgent::issue_url`).
    pub issue_url: Option<String>,
    /// Enter and lock the TERMINAL PANE once the create is acked. True for
    /// every launch the user walked a picker to reach; the QUICK PROMPT
    /// passes the `quick_prompt_focus` SETTING, which is off by default.
    pub focus_pane: bool,
    /// The stand-in session row already on screen for this create (a
    /// QUICK PROMPT into a worktree that did not exist, a launch that
    /// waited on the NEW WORKTREE modal's checkout): the Ack turns it
    /// into the created row, an Error drops it.
    pub placeholder: Option<AgentId>,
    /// Land the cursors, the pane and FOCUS on the created session when
    /// its Ack arrives. False only for the second half of a two-request
    /// launch (a checkout cut first, then the session) whose first half
    /// the user navigated away from: the create is born in
    /// `App::left_behind`, so the manual move still outranks the follow.
    pub follow: bool,
}

impl AgentLaunchDraft {
    /// A launch of `kind` at `model` / `effort` into `worktree` with
    /// nothing else said: no name (the row titles itself), no first
    /// prompt, no pull request or issue behind it, no box to bring back,
    /// no stand-in row, and the pane taken — what a launch the user walked
    /// a picker to reach looks like. Every launch surface starts from this
    /// and names only what is its own (`..AgentLaunchDraft::new(…)`), so a
    /// field added here has one default instead of one per surface that
    /// would otherwise have to remember it.
    pub fn new(
        worktree: WorktreeId,
        kind: AgentKind,
        model: Option<String>,
        effort: Option<String>,
    ) -> Self {
        Self {
            worktree,
            kind,
            custom: None,
            model,
            effort,
            name: String::new(),
            cloud_prompt: None,
            starting_prompt: None,
            reopen_on_error: None,
            pr: None,
            issue_url: None,
            focus_pane: true,
            placeholder: None,
            follow: true,
        }
    }
}

/// What to do when an Ack (or Error) for this req_id arrives.
#[derive(Debug, Clone)]
pub enum PendingIntent {
    /// Attach the created session; `focus` also enters and locks the
    /// terminal pane. Off (the QUICK PROMPT's default) stops at selecting
    /// the row, so the pane previews the new session without taking the
    /// cursor off whatever panel the create was fired from.
    AttachCreated {
        focus: bool,
        /// The stand-in session row up for this create — a launch that
        /// waited on the NEW WORKTREE modal's checkout: the Ack turns it
        /// into the created row, an Error drops it.
        placeholder: Option<AgentId>,
    },
    /// Attach on success; on failure, reopen the exact Cloud task so a
    /// transient daemon/CLI error never makes the user retype it.
    AttachCreatedWithCloudRetry {
        kind: PromptKind,
        task: String,
        focus: bool,
        /// The stand-in session row a QUICK PROMPT put up for this create:
        /// the Ack turns it into the created row, an Error drops it.
        placeholder: Option<AgentId>,
    },
    /// Flash `note` on success; on failure, reopen this prompt with `text`
    /// restored. Same bargain as the Cloud task: a message worth typing into
    /// a multi-row editor is worth not losing to a transient error.
    ReopenPromptOnError {
        kind: PromptKind,
        text: String,
        note: String,
    },
    /// `r` on a worktree (`StartRun` / `StopRun`): once the DAEMON has done
    /// it, flash what happened in `branch`.
    RunToggled {
        branch: String,
        started: bool,
    },
    /// Select the added project and step into its Worktrees panel.
    SelectCreatedProject,
    /// The NEW WORKTREE modal's create. The stand-in row `placeholder`
    /// went up and took the cursor when Enter was pressed, exactly as the
    /// Ack used to leave the real row: the Ack only swaps the real id in.
    /// An Error takes the row down, puts the cursor and `focus` back where
    /// they were, and reopens `prompt` with `text` for a retry.
    SelectCreatedWorktree {
        placeholder: WorktreeId,
        focus: Focus,
        prompt: PromptKind,
        text: String,
        /// A launch fired into the stand-in while the DAEMON was still
        /// cutting the checkout (`e` on the new row, an AGENT PRESET
        /// run): its session row is up under the stand-in, and the Ack
        /// sends the create into the real checkout
        /// (`placeholder::defer_launch` / `replay_launch`). An Error
        /// takes that row down with the checkout's.
        launch: Option<Box<AgentLaunchDraft>>,
    },
    /// A QUICK PROMPT whose target was a worktree that did not exist yet:
    /// the Ack names the checkout the DAEMON cut, the cursor moves onto
    /// its row, and `launch` fires there with `text` as the task. On
    /// Error the box comes back with the text, like every other launch.
    LaunchInCreatedWorktree {
        launch: crate::quick_prompt::QuickLaunch,
        text: String,
        /// The stand-in rows on screen meanwhile.
        placeholder: PlaceholderRows,
        /// The session's Ack takes the pane — the `quick_prompt_focus`
        /// SETTING, as the box's Enter found it.
        focus: bool,
    },
    /// A PR SESSION whose head branch had no checkout yet: the DAEMON
    /// fetches the branch, cuts the worktree and spawns the CLI in it
    /// behind this one request, and the Ack names only the session. The
    /// stand-in checkout row is adopted by the worktree's upsert as it
    /// lands (`placeholder::adopt_worktree`), the session row by the Ack,
    /// which then attaches it like `AttachCreated` — `focus` enters and
    /// locks the pane. An Error takes down whatever is still a stand-in
    /// and, like every other launch out of a box, brings `reopen`'s box
    /// back with what was typed in it — this create is the one most
    /// likely to be refused (a fetch offline, a fork since deleted, a
    /// branch git will not cut), and a review prompt is not retyped
    /// gladly. None for the `n` picker's name box, which has no task.
    AttachCreatedPrSession {
        focus: bool,
        placeholder: PlaceholderRows,
        reopen: Option<(PromptKind, String)>,
        /// The pull request the launch was fired from: where a cursor
        /// still on the refused stand-in goes back to. The row it left had
        /// no worktree, so `remember_context` recorded nothing to return
        /// to, and `restore_context` alone would drop the cursor on the
        /// ROOT WORKTREE — one `p` away from a session in the main
        /// checkout.
        pr_url: String,
    },
    /// Worktree removed optimistically; restore these rows on Error.
    DeleteWorktree(WorktreeRollback),
    /// A row renamed, archived, unarchived or deleted on the keypress
    /// (`event_loop::optimistic`): put it back on Error.
    Undo(Undo),
    None,
}

/// A row as it was before an OPTIMISTIC UPDATE changed it.
#[derive(Debug, Clone)]
pub enum Undo {
    /// Renamed or (un)archived in place: this is the row to show again.
    Restore(Box<nebula_core::Entity>),
    /// Deleted: the row, and the index it held in its `tree` list.
    Reinsert {
        index: usize,
        entity: Box<nebula_core::Entity>,
    },
}

impl PendingIntent {
    /// Will this request's Ack move a cursor, the pane or FOCUS onto what
    /// it created? Those are the follows a manual move cancels
    /// (`App::left_behind`). The NEW WORKTREE modal's own Ack moves
    /// nothing — its select happened at Enter — until a launch is waiting
    /// on it.
    pub fn follows(&self) -> bool {
        match self {
            PendingIntent::AttachCreated { .. }
            | PendingIntent::AttachCreatedWithCloudRetry { .. }
            | PendingIntent::AttachCreatedPrSession { .. }
            | PendingIntent::LaunchInCreatedWorktree { .. }
            | PendingIntent::SelectCreatedProject => true,
            PendingIntent::SelectCreatedWorktree { launch, .. } => launch.is_some(),
            _ => false,
        }
    }

    /// The stand-in worktree row this in-flight request is holding up.
    pub fn placeholder_worktree(&self) -> Option<&WorktreeId> {
        match self {
            PendingIntent::LaunchInCreatedWorktree { placeholder, .. }
            | PendingIntent::AttachCreatedPrSession { placeholder, .. } => {
                Some(&placeholder.worktree)
            }
            PendingIntent::SelectCreatedWorktree { placeholder, .. } => Some(placeholder),
            _ => None,
        }
    }

    /// The stand-in session row this in-flight request is holding up.
    pub fn placeholder_agent(&self) -> Option<&AgentId> {
        match self {
            PendingIntent::LaunchInCreatedWorktree { placeholder, .. }
            | PendingIntent::AttachCreatedPrSession { placeholder, .. } => Some(&placeholder.agent),
            PendingIntent::AttachCreatedWithCloudRetry { placeholder, .. }
            | PendingIntent::AttachCreated { placeholder, .. } => placeholder.as_ref(),
            PendingIntent::SelectCreatedWorktree { launch, .. } => {
                launch.as_ref().and_then(|draft| draft.placeholder.as_ref())
            }
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnState {
    Connected,
    Disconnected,
}

/// One row of the Sessions panel's PULL REQUESTS group: a previously saved
/// URL, or the pull request nebula found on the worktree's branch — open,
/// draft, merged or closed alike.
#[derive(Debug, Clone)]
pub enum LinkRow {
    /// Discovered by `gh pr view`, backed by nothing in the store — so it
    /// can be opened but not edited or deleted.
    PullRequest(PullRequest),
    /// A saved link. `pr` is set when this link's URL *is* the detected
    /// pull request: the row then shows the PR's title and badge instead of
    /// a bare URL, and stays editable — it is still the user's own row.
    Saved { link: Link, pr: Option<PullRequest> },
}

impl LinkRow {
    pub fn url(&self) -> &str {
        match self {
            LinkRow::PullRequest(pr) => &pr.url,
            LinkRow::Saved { link, .. } => &link.url,
        }
    }

    /// The stored link behind the row; None for the detected pull request,
    /// which is what makes it un-editable and un-deletable.
    pub fn id(&self) -> Option<&LinkId> {
        match self {
            LinkRow::PullRequest(_) => None,
            LinkRow::Saved { link, .. } => Some(&link.id),
        }
    }

    /// The pull request this row stands for, however it got here.
    pub fn pull_request(&self) -> Option<&PullRequest> {
        match self {
            LinkRow::PullRequest(pr) => Some(pr),
            LinkRow::Saved { pr, .. } => pr.as_ref(),
        }
    }

    /// Comments and reviews other people left on this row's pull request
    /// since it was last opened from nebula. Zero for a row that isn't a
    /// pull request — nothing else has a conversation to fall behind on.
    pub fn unseen_comments(&self, seen: &HashMap<String, String>) -> usize {
        match self.pull_request() {
            Some(pr) => pr.unseen(seen.get(&pr.url).map(String::as_str)),
            None => 0,
        }
    }

    /// Row text: a pull request reads as `#42 title`, anything else as its
    /// URL with the noise (scheme, `www.`, trailing slash) stripped.
    pub fn label(&self) -> String {
        match self.pull_request() {
            Some(pr) if !pr.title.is_empty() => format!("#{} {}", pr.number, pr.title),
            Some(pr) => format!("#{}", pr.number),
            None => pretty_url(self.url()),
        }
    }
}

/// A URL as a person reads it: no scheme, no `www.`, no trailing slash.
/// Purely cosmetic — the full URL is what gets opened.
pub fn pretty_url(url: &str) -> String {
    let bare = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))
        .unwrap_or(url);
    let bare = bare.strip_prefix("www.").unwrap_or(bare);
    bare.strip_suffix('/').unwrap_or(bare).to_string()
}

/// One row in the Sessions panel: agents, then shell terminals, then the
/// worktree's links, then archived agents.
#[derive(Debug, Clone)]
pub enum SessionRow {
    Agent(Agent),
    Terminal(TerminalTab),
    Link(LinkRow),
}

impl SessionRow {
    pub fn name(&self) -> &str {
        match self {
            SessionRow::Agent(a) => &a.name,
            SessionRow::Terminal(t) => &t.name,
            SessionRow::Link(l) => l.url(),
        }
    }

    /// The attachable session behind the row. Link rows have none — they
    /// open a browser, not a PTY.
    pub fn sref(&self) -> Option<SessionRef> {
        match self {
            SessionRow::Agent(a) => Some(SessionRef::Agent(a.id.clone())),
            SessionRow::Terminal(t) => Some(SessionRef::Terminal(t.id.clone())),
            SessionRow::Link(_) => None,
        }
    }

    pub fn is_archived_agent(&self) -> bool {
        matches!(self, SessionRow::Agent(a) if a.archived)
    }

    pub fn as_link(&self) -> Option<&LinkRow> {
        match self {
            SessionRow::Link(l) => Some(l),
            _ => None,
        }
    }

    /// Identity for double-click tracking: distinct per row, and stable
    /// across the repaints between the two clicks.
    pub fn click_key(&self) -> RowKey {
        match self.sref() {
            Some(sref) => RowKey::Session(sref),
            None => RowKey::Link(self.name().to_string()),
        }
    }
}

/// One row of the WORKTREES PANEL, in cursor order — what `sel_worktree`
/// indexes (see [`App::worktree_rows`]).
#[derive(Debug, Clone, Copy)]
pub enum WorktreeRow<'a> {
    /// A checkout listed on its own: the ROOT WORKTREE, or a branch no
    /// open pull request on screen is from.
    Checkout(&'a Worktree),
    /// A PROJECT OPEN PRS GROUP row.
    Pr(&'a OpenPr),
    /// A checkout on an open pull request's head branch — the one a PR
    /// SESSION launched off that row works in (`CreatePrAgent` cuts or
    /// reuses it), or one `n` cut on a branch a pull request was later
    /// opened from — listed under that pull request's row rather than
    /// among the plain checkouts, so the checkout and the pull request it
    /// is for read as one thing.
    PrCheckout {
        worktree: &'a Worktree,
        pr: &'a OpenPr,
    },
    /// A PROJECT ISSUES GROUP row: an issue open on the repo, listed under
    /// the pull requests. No checkout and no sessions — the pane reads it,
    /// as it reads a pull request row.
    Issue(&'a crate::issues::Issue),
}

impl<'a> WorktreeRow<'a> {
    /// The checkout the row is, wherever it sits: a plain one, or one
    /// nested under its pull request. None on a pull request or issue row.
    pub fn checkout(self) -> Option<&'a Worktree> {
        match self {
            WorktreeRow::Checkout(w) | WorktreeRow::PrCheckout { worktree: w, .. } => Some(w),
            WorktreeRow::Pr(_) | WorktreeRow::Issue(_) => None,
        }
    }

    /// The issue the row *is* — a PROJECT ISSUES GROUP row.
    pub fn open_issue(self) -> Option<&'a crate::issues::Issue> {
        match self {
            WorktreeRow::Issue(issue) => Some(issue),
            WorktreeRow::Checkout(_) | WorktreeRow::Pr(_) | WorktreeRow::PrCheckout { .. } => None,
        }
    }

    /// The pull request the row *is* — the OPEN PRS row itself, not the
    /// checkout nested under one: that row is a worktree, with a
    /// worktree's keys and sessions, and its pull request is one row up.
    pub fn open_pr(self) -> Option<&'a OpenPr> {
        match self {
            WorktreeRow::Pr(pr) => Some(pr),
            WorktreeRow::Checkout(_) | WorktreeRow::PrCheckout { .. } | WorktreeRow::Issue(_) => {
                None
            }
        }
    }

    /// The pull request a nested checkout sits under.
    pub fn nested_under(self) -> Option<&'a OpenPr> {
        match self {
            WorktreeRow::PrCheckout { pr, .. } => Some(pr),
            WorktreeRow::Checkout(_) | WorktreeRow::Pr(_) | WorktreeRow::Issue(_) => None,
        }
    }
}

/// What a click landed on, for the double-click window. Sessions are their
/// own reference; a link has none (the pull-request row isn't even stored),
/// so its URL is the identity; a checkout is its id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RowKey {
    Session(SessionRef),
    Link(String),
    Worktree(WorktreeId),
}

/// Aggregate status for a worktree row: red > yellow > green > gray,
/// archived agents excluded. Free-standing so the `/` palette can roll a
/// row up straight from the tree, with no `App` in hand.
pub fn worktree_rollup(tree: &Tree, worktree_id: &WorktreeId) -> Option<AgentStatus> {
    rollup(
        tree.agents
            .iter()
            .filter(|a| &a.worktree_id == worktree_id && !a.archived)
            .map(|a| a.status),
    )
}

/// The same aggregate over every worktree of a project.
pub fn project_rollup(tree: &Tree, project_id: &ProjectId) -> Option<AgentStatus> {
    let wt_ids: Vec<&WorktreeId> = tree
        .worktrees
        .iter()
        .filter(|w| &w.project_id == project_id)
        .map(|w| &w.id)
        .collect();
    rollup(
        tree.agents
            .iter()
            .filter(|a| wt_ids.contains(&&a.worktree_id) && !a.archived)
            .map(|a| a.status),
    )
}

/// How many sessions under a worktree finished a turn nobody has looked at
/// yet (`Agent::unseen`) — the row's count badge, the number of terminals
/// to go read. Archived rows are out of sight, so they don't count.
pub fn worktree_unseen(tree: &Tree, worktree_id: &WorktreeId) -> usize {
    tree.agents
        .iter()
        .filter(|a| &a.worktree_id == worktree_id && !a.archived && a.unseen)
        .count()
}

/// The same count over every worktree of a project.
pub fn project_unseen(tree: &Tree, project_id: &ProjectId) -> usize {
    tree.worktrees
        .iter()
        .filter(|w| &w.project_id == project_id)
        .map(|w| worktree_unseen(tree, &w.id))
        .sum()
}

/// Whether the agent's unread finish is recent enough (`ONE_SHOT_SWEEP`) to
/// still be sweeping at `now` epoch ms. `unseen` is only ever true on a
/// finished row, so `status_changed_at` is the finish itself. The stamp is
/// the DAEMON's clock and `now` this client's, so the window is taken either
/// side of it: a skewed pair sweeps a little off-time, never for an hour.
pub fn fresh_done(agent: &Agent, now: i64) -> bool {
    let window = ONE_SHOT_SWEEP.as_millis() as i64;
    agent.unseen
        && !agent.archived
        && agent.status_changed_at > 0
        && (now - agent.status_changed_at).abs() < window
}

/// [`fresh_done`] rolled up to a worktree row, the way [`worktree_unseen`]
/// rolls the count up: some session under it just finished unread.
pub fn worktree_fresh_done(tree: &Tree, worktree_id: &WorktreeId, now: i64) -> bool {
    tree.agents
        .iter()
        .any(|a| &a.worktree_id == worktree_id && fresh_done(a, now))
}

/// The same over every worktree of a project.
pub fn project_fresh_done(tree: &Tree, project_id: &ProjectId, now: i64) -> bool {
    tree.worktrees
        .iter()
        .filter(|w| &w.project_id == project_id)
        .any(|w| worktree_fresh_done(tree, &w.id, now))
}

/// A session that is mid-turn or blocked on the user. These count as
/// interacting *now*, so they head the sessions list however long the turn
/// has taken — the point is to keep what needs attention in view.
pub fn is_active_status(s: AgentStatus) -> bool {
    matches!(s, AgentStatus::Running | AgentStatus::NeedsFeedback)
}

/// Epoch ms of the last interaction with a session — the stamp the sessions
/// list sorts on and renders as "23m ago". A working session counts as
/// interacting *now*: it is producing output as you look at it, so it holds
/// the top of the list however long the turn has taken. 0 = never run.
pub fn last_interaction_ms(a: &Agent, now: i64) -> i64 {
    if is_active_status(a.status) {
        now
    } else {
        a.status_changed_at
    }
}

/// Sort key for "most recently interacted with, first". Applied with a
/// stable sort, so never-run sessions (stamp 0) fall to the bottom of their
/// group in tree order. The SESSIONS panel and the LAUNCHER's grid both
/// order on it, so a session sits in the same place in either.
///
/// Working and blocked sessions all count as interacting *now*, so the raw
/// stamp breaks that tie: among them the newest turn leads — which is what
/// puts the session just launched at the top of the list, rather than under
/// every session that has been mid-turn for an hour (a launch handed a
/// first prompt is created `running`, stamped as it is created). Only live
/// turns are ordered that way; for every other row the last key is the
/// first one again, so nothing else moves.
pub fn recency_key(
    a: &Agent,
    now: i64,
) -> (
    std::cmp::Reverse<i64>,
    std::cmp::Reverse<bool>,
    std::cmp::Reverse<i64>,
) {
    (
        std::cmp::Reverse(last_interaction_ms(a, now)),
        // A live turn holds the tie it shares with a row stamped in that
        // same millisecond: its own stamp is older by design (the clock
        // above stands in for it), so the stamps below cannot decide it.
        std::cmp::Reverse(is_active_status(a.status)),
        std::cmp::Reverse(a.status_changed_at),
    )
}

/// The two stamps a worktree or project row derives from the sessions
/// under it. `interacted` is the newest [`last_interaction_ms`] — the sort
/// key, where a working session counts as now — and `stamped` the newest
/// raw `status_changed_at`, which the row's "23m ago" label reads so a
/// checkout with an hour-long turn in it says "1h ago" exactly like the
/// session does. Both 0 when nothing under the row has ever run.
///
/// Archived sessions count: the stamp records when work last happened
/// there, and archiving a row is housekeeping, not activity.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Recency {
    pub interacted: i64,
    pub stamped: i64,
}

impl Recency {
    fn of<'a>(agents: impl Iterator<Item = &'a Agent>, now: i64) -> Recency {
        agents.fold(Recency::default(), |r, a| Recency {
            interacted: r.interacted.max(last_interaction_ms(a, now)),
            stamped: r.stamped.max(a.status_changed_at),
        })
    }

    /// Fold one more session in, for the one-pass rollups below.
    fn absorb(&mut self, a: &Agent, now: i64) {
        self.interacted = self.interacted.max(last_interaction_ms(a, now));
        self.stamped = self.stamped.max(a.status_changed_at);
    }
}

/// EVERY checkout's [`Recency`], rolled up in ONE pass over the sessions.
///
/// The row sorts want all of them at once. Asking [`worktree_recency`]
/// per row walks the whole session list again for each — and `sort_by_key`
/// repeats the key on every COMPARISON, not once per row, so a project
/// with a couple of hundred sessions under one checkout pays tens of
/// thousands of id comparisons per sort. The sorts run several times per
/// frame and twice per turn of the event loop, which is enough to starve
/// the keyboard. One pass here, one hash lookup per comparison there.
pub fn worktree_recencies(tree: &Tree, now: i64) -> HashMap<&WorktreeId, Recency> {
    let mut by_worktree: HashMap<&WorktreeId, Recency> = HashMap::new();
    for a in &tree.agents {
        by_worktree
            .entry(&a.worktree_id)
            .or_default()
            .absorb(a, now);
    }
    by_worktree
}

/// The same for every project, on the same one-pass footing — and with
/// the checkout→project map hashed too, where [`project_recency`] walks a
/// `Vec` of the project's checkouts once per session.
pub fn project_recencies(tree: &Tree, now: i64) -> HashMap<&ProjectId, Recency> {
    let owner: HashMap<&WorktreeId, &ProjectId> = tree
        .worktrees
        .iter()
        .map(|w| (&w.id, &w.project_id))
        .collect();
    let mut by_project: HashMap<&ProjectId, Recency> = HashMap::new();
    for a in &tree.agents {
        let Some(project) = owner.get(&a.worktree_id) else {
            continue;
        };
        by_project.entry(project).or_default().absorb(a, now);
    }
    by_project
}

/// When a worktree last saw a turn: the newest stamp of any session in it.
pub fn worktree_recency(tree: &Tree, worktree_id: &WorktreeId, now: i64) -> Recency {
    Recency::of(
        tree.agents.iter().filter(|a| &a.worktree_id == worktree_id),
        now,
    )
}

/// The same over every worktree of a project.
pub fn project_recency(tree: &Tree, project_id: &ProjectId, now: i64) -> Recency {
    let wt_ids: Vec<&WorktreeId> = tree
        .worktrees
        .iter()
        .filter(|w| &w.project_id == project_id)
        .map(|w| &w.id)
        .collect();
    Recency::of(
        tree.agents
            .iter()
            .filter(|a| wt_ids.contains(&&a.worktree_id)),
        now,
    )
}

/// How loudly one status asks for a human: needs-feedback > running >
/// finished > gone > fresh. The order every rollup reads, and the one the
/// LAUNCHER VIEW's cards sort by, so a project card, a panel row and the
/// `/` PALETTE all agree on which session is the one to look at.
pub fn status_rank(s: AgentStatus) -> u8 {
    match s {
        AgentStatus::NeedsFeedback => 4,
        AgentStatus::Running => 3,
        AgentStatus::Finished => 2,
        AgentStatus::Terminated | AgentStatus::Disconnected => 1,
        AgentStatus::Fresh => 0,
    }
}

/// Priority-ordered aggregate: needs-feedback > running > finished > fresh.
pub fn rollup(statuses: impl Iterator<Item = AgentStatus>) -> Option<AgentStatus> {
    let mut best: Option<AgentStatus> = None;
    for s in statuses {
        best = Some(match best {
            Some(b) if status_rank(b) >= status_rank(s) => b,
            _ => s,
        });
    }
    best
}

/// Client-side mirror of the entity tree: every project on this machine,
/// with the checkouts, sessions, terminals and links under them.
#[derive(Debug, Clone, Default)]
pub struct Tree {
    pub projects: Vec<Project>,
    pub worktrees: Vec<Worktree>,
    pub agents: Vec<Agent>,
    pub terminals: Vec<TerminalTab>,
    pub links: Vec<Link>,
}

impl Tree {
    /// Any project at all? None is a first run: the splash, with the way
    /// to open one, is all there is to draw.
    pub fn has_projects(&self) -> bool {
        !self.projects.is_empty()
    }

    /// The project registered for the repo at `path`, or for any checkout
    /// of it — the folder the user named when a project already stands
    /// for it. An exact path match, so it is run on canonical paths
    /// ([`crate::event_loop`]'s add prompt canonicalizes first).
    pub fn project_at_path(&self, path: &std::path::Path) -> Option<&Project> {
        self.projects.iter().find(|p| {
            p.repo_path == path
                || self
                    .worktrees
                    .iter()
                    .any(|w| w.project_id == p.id && w.path == path)
        })
    }
}

/// What the pane's terminal emulation reports back beyond the screen
/// itself. One thing today: an OSC 52 clipboard write from the program in
/// the pane — Claude Code's fullscreen renderer copying its own selection
/// over ssh, vim's OSC 52 clipboard, tmux with `set-clipboard` on. In a
/// plain terminal that write lands on the clipboard; here it lands in the
/// emulation, so the parser parks it and the event loop passes it on to
/// the terminal the user is sitting at (`App::pending_clipboard`, the route
/// nebula's own copy takes on a remote host).
#[derive(Debug, Default)]
pub struct TermCallbacks {
    /// Base64 payload of the latest OSC 52 write, until drained.
    pub clipboard: Option<String>,
}

impl vt100::Callbacks for TermCallbacks {
    fn copy_to_clipboard(&mut self, _: &mut vt100::Screen, _ty: &[u8], data: &[u8]) {
        // vt100 only gets here with a base64-clean payload.
        self.clipboard = Some(String::from_utf8_lossy(data).into_owned());
    }
}

pub struct AttachedTerm {
    pub sref: SessionRef,
    pub parser: vt100::Parser<TermCallbacks>,
    pub exited: bool,
    /// Size the parser (and daemon PTY) currently uses.
    pub cols: u16,
    pub rows: u16,
    /// Scrollback offset; 0 = live tail.
    pub scroll: usize,
    /// The child's kitty keyboard flags (daemon-tracked); picks the key
    /// encoding dialect. 0 = legacy.
    pub kitty_flags: u8,
    /// Whether any PTY bytes have reached this parser yet. False means the
    /// grid is blank because the session is still booting — attaching to a
    /// reaped session replays an empty ring, and an agent CLI takes seconds
    /// to paint its first frame. The pane says so instead of showing an
    /// unexplained void.
    pub painted: bool,
    /// The grid is blank because the session has no screen yet — the
    /// daemon replayed an empty ring, or the session was reaped and this
    /// attach is what boots it — as opposed to a replay that hasn't landed.
    /// The pane's "starting session…" notice shows only in the first case:
    /// flashing it for the frame a live session's replay takes reads as a
    /// hiccup on every switch.
    pub booting: bool,
    /// Byte offset the next PTY byte should carry, in the daemon's ring
    /// numbering: the end of everything this parser has processed. A
    /// re-attach asks for output from here, so a session shown a moment ago
    /// comes back as a gap-free delta onto the screen it left rather than a
    /// megabyte replay into a fresh parser (see [`App::term_cache`]).
    pub next_seq: u64,
    /// The scrollback was let go while this screen sat in
    /// [`App::term_cache`]: what is on screen is exact, what is above it is
    /// gone. Scrolling up asks the DAEMON for the whole ring again
    /// (`event_loop::rehydrate_history`), which rebuilds both.
    pub history_dropped: bool,
    /// The scroll offset to land on once that replay has rebuilt the
    /// history: the notch that asked for it.
    pub pending_scroll: Option<usize>,
}

impl AttachedTerm {
    pub fn new(sref: SessionRef, cols: u16, rows: u16) -> Self {
        Self {
            sref,
            parser: vt100::Parser::new_with_callbacks(rows, cols, 10_000, TermCallbacks::default()),
            exited: false,
            cols,
            rows,
            scroll: 0,
            kitty_flags: 0,
            painted: false,
            booting: false,
            next_seq: 0,
            history_dropped: false,
            pending_scroll: None,
        }
    }

    /// Reset the parser (fresh replay is about to arrive).
    pub fn reset(&mut self) {
        self.parser = vt100::Parser::new_with_callbacks(
            self.rows,
            self.cols,
            10_000,
            TermCallbacks::default(),
        );
        self.exited = false;
        self.scroll = 0;
        self.painted = false;
        self.booting = false;
        self.next_seq = 0;
        self.history_dropped = false;
    }

    /// Apply a ring replay. One that continues exactly where this parser
    /// left off (`base_seq == next_seq`, the answer to an attach that asked
    /// `from_seq`) is appended and the screen stays; anything else — a
    /// first attach, a ring that wrapped past what was seen, a new process
    /// under the same session — rebuilds the screen from scratch. Returns
    /// whether it was rebuilt, so a selection anchored to the old cells can
    /// be dropped. An empty replay is the daemon saying nothing has painted
    /// yet, which is what `booting` reports.
    pub fn apply_scrollback(&mut self, base_seq: u64, data: &[u8]) -> bool {
        // A parser that watched its process exit has nothing worth
        // continuing: the only replay that can follow is the next process's.
        let rebuilt = base_seq != self.next_seq || self.exited;
        if rebuilt {
            self.reset();
        }
        self.next_seq = base_seq + data.len() as u64;
        self.feed(data);
        if !self.painted {
            self.booting = true;
        }
        // A replay asked for to get the history back lands the reader
        // where the scroll that asked for it was headed.
        if let Some(scroll) = self.pending_scroll.take() {
            if rebuilt {
                self.set_scroll(scroll);
            }
        }
        rebuilt
    }

    /// Apply live output that follows what the parser holds. Bytes a replay
    /// already covered are skipped: a second Attach of the session on
    /// screen (the history replay) can cross a frame the first one's
    /// forwarder had already queued, and parsing those bytes twice would
    /// garble the screen the replay just rebuilt.
    pub fn apply_output(&mut self, seq: u64, data: &[u8]) {
        let end = seq + data.len() as u64;
        if end <= self.next_seq {
            return;
        }
        let covered = self.next_seq.saturating_sub(seq) as usize;
        self.next_seq = end;
        self.feed(&data[covered.min(data.len())..]);
    }

    fn feed(&mut self, data: &[u8]) {
        if !data.is_empty() {
            self.painted = true;
            self.booting = false;
        }
        self.parser.process(data);
    }

    /// Regrid to `cols`×`rows` when that differs from the current size.
    pub fn resize(&mut self, cols: u16, rows: u16) {
        if (self.cols, self.rows) != (cols, rows) {
            self.cols = cols;
            self.rows = rows;
            self.parser.screen_mut().set_size(rows, cols);
        }
    }

    /// Footprint in grid cells: the visible grid plus every scrollback row
    /// the primary screen holds — also while a full-screen program is up
    /// over it, which the offset-clamping trick this used to be could not
    /// see (the vendored `vt100` now says, `Screen::scrollback_rows`).
    pub fn estimated_cells(&self) -> usize {
        let lines = self.parser.screen().scrollback_rows();
        (lines + self.rows as usize) * self.cols as usize
    }

    /// Let the scrollback go, keeping the screen: what a kept screen that
    /// is over budget does instead of not being kept.
    pub fn drop_history(&mut self) {
        if self.parser.screen().scrollback_rows() == 0 {
            return;
        }
        self.set_scroll(0);
        self.parser.screen_mut().clear_scrollback();
        self.history_dropped = true;
    }

    pub fn set_scroll(&mut self, scroll: usize) {
        self.scroll = scroll;
        self.parser.screen_mut().set_scrollback(scroll);
    }

    /// The program's latest OSC 52 clipboard write, if one arrived since the
    /// last call (see `TermCallbacks`).
    pub fn take_clipboard(&mut self) -> Option<String> {
        self.parser.callbacks_mut().clipboard.take()
    }
}

/// Opaque UI state persisted in the daemon's DB for session restore.
#[derive(Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct UiState {
    pub project: Option<String>,
    pub worktree: Option<String>,
    pub session_agent: Option<String>,
    pub show_archived: bool,
    pub collapsed: bool,
    /// The PROJECT OPEN PRS GROUP folded down to its header; absent in
    /// older blobs, which keep it open.
    #[serde(default)]
    pub open_prs_collapsed: bool,
    /// The PROJECT ISSUES GROUP folded down to its header; absent in
    /// older blobs, which keep it open.
    #[serde(default)]
    pub issues_collapsed: bool,
    /// Diff modal file-list width; absent in older blobs.
    #[serde(default)]
    pub diff_files_width: Option<u16>,
    /// The diff modal's file list is the directory tree (`Ctrl+t`); absent
    /// in older blobs, which keep the flat list.
    #[serde(default)]
    pub diff_tree: bool,
    /// Height the LAUNCHER VIEW's pane was dragged to; absent in older
    /// blobs, and None in ones written before the edge was ever dragged,
    /// both of which open the pane on its default share.
    #[serde(default)]
    pub launcher_pane_h: Option<u16>,
    /// Width the pane was dragged to beside the cards; absent in older
    /// blobs, which open a side pane on its default half.
    #[serde(default)]
    pub launcher_pane_w: Option<u16>,
    /// The LAUNCHER VIEW's pane was folded away (`^~`); absent in older
    /// blobs, which open with it showing.
    #[serde(default)]
    pub launcher_pane_hidden: bool,
    /// The LAUNCHER VIEW's PROJECT TABS, by project id, far left first;
    /// absent in older blobs, which open with the one project restored.
    #[serde(default)]
    pub launcher_tabs: Vec<String>,
}

/// A mouse selection over the terminal pane (drag or double-click word),
/// with inclusive `(col, line)` endpoints: a pane-relative column and the
/// screen's HISTORY LINE (`vt100::Screen::history_base`) — a number every
/// row keeps as the view scrolls and as new output pushes it up into the
/// scrollback. Anchored that way the highlight stays on its text while
/// the pane scrolls under a drag (the EDGE AUTO-SCROLL, the wheel) and
/// while the agent keeps printing, and the copy at release reads rows
/// that have since left the screen.
/// Nebula owns the mouse (the emulator's native shift+drag never reaches us
/// reliably — Terminal.app has no such bypass at all), so selection is
/// implemented app-side and copied to the system clipboard when it completes.
/// The highlight persists after mouse-up; it's cleared by the next click,
/// the wheel, typing into the PTY, a resize, or a replay that rebuilds the
/// screen (its numbering starts over). A drag still in progress survives
/// every one of those but the click: the button is the user's, and nothing
/// here lets go of the selection until it comes up.
#[derive(Debug, Clone, Copy)]
pub struct TermSelection {
    pub anchor: (u16, u64),
    pub head: (u16, u64),
    /// Still being dragged (button down). Cleared on mouse-up.
    pub dragging: bool,
    /// A real selection, not just an armed click. Set once a drag leaves its
    /// starting cell (and kept if it returns), or immediately for a
    /// double-click word selection — which may be a single cell, so
    /// `anchor == head` can't be the "just a click" test.
    pub active: bool,
    /// Where the pointer last was, in host cells, while dragging. The EDGE
    /// AUTO-SCROLL re-reads the head from here on every tick, so a pointer
    /// resting past the pane's top or bottom edge keeps selecting as the
    /// history scrolls under it.
    pub pointer: (u16, u16),
}

impl TermSelection {
    /// Endpoints normalized to line-major order: (start, end).
    pub fn bounds(&self) -> ((u16, u64), (u16, u64)) {
        let anchor_key = (self.anchor.1, self.anchor.0);
        let head_key = (self.head.1, self.head.0);
        if anchor_key <= head_key {
            (self.anchor, self.head)
        } else {
            (self.head, self.anchor)
        }
    }
}

/// Mouse pointer shape the outer terminal should show, requested via the
/// xterm OSC 22 pointer-shape escape (CSS cursor names, per the kitty
/// pointer-shapes protocol). Mouse handlers record the want here; the event
/// loop emits the escape when it changes. Terminals that don't support the
/// sequence (Terminal.app) parse and drop it, so requesting is always safe.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PointerShape {
    #[default]
    Default,
    /// Horizontal-resize arrows over a draggable panel boundary.
    ColResize,
    /// Vertical-resize arrows over the LAUNCHER VIEW's pane boundary.
    RowResize,
}

impl PointerShape {
    /// The shape's name inside the OSC 22 escape.
    pub fn osc_name(self) -> &'static str {
        match self {
            PointerShape::Default => "default",
            PointerShape::ColResize => "col-resize",
            PointerShape::RowResize => "row-resize",
        }
    }
}

/// What `gh pr list` last said about one project's open pull requests, and
/// the timer deciding when to ask again. Held per project rather than
/// refetched per repaint because every answer is a `gh` process and a
/// GitHub API call, and the list changes on the order of minutes.
#[derive(Debug, Clone)]
pub struct OpenPrs {
    /// Open pull requests: newest first, with the drafts sunk below every
    /// finished one (`pull_request::drafts_last`, applied as the answer
    /// lands).
    pub list: Vec<OpenPr>,
    /// When this answer landed. Switching projects pulls the next lookup
    /// forward, but never past this plus [`OPEN_PRS_MIN_AGE`] — otherwise
    /// bouncing between two projects would spend an API call per keystroke.
    pub at: std::time::Instant,
    /// When the next lookup is due, and the step that produced that
    /// deadline: a steady beat once a repo has proved it has pull requests,
    /// a doubling backoff while it hasn't.
    pub due: std::time::Instant,
    pub step: std::time::Duration,
}

/// What a debounced pull-request detail fetch needs: which PR, and the
/// checkout to run `gh` from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingPrDetail {
    pub url: String,
    pub number: u64,
    pub dir: PathBuf,
}

/// A finished `gh pr diff`, back on the loop: which pull request, what to
/// title the modal, and the diff — `None` when `gh` couldn't answer. The
/// URL is what the cache files it under and what says whether a modal
/// already open on a cached copy is this one's.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrDiffAnswer {
    pub number: u64,
    pub url: String,
    pub title: String,
    pub diff: Option<String>,
}

/// A finished `gh pr comment`, back on the loop: which pull request (and
/// the row text that titled its box), the text that was sent — handed
/// back to the box when the post failed — and what `gh` said: the new
/// comment's URL, or why it refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrCommentAnswer {
    pub number: u64,
    pub url: String,
    pub label: String,
    pub body: String,
    pub result: Result<String, String>,
}

/// The pull request the TERMINAL PANE is reading instead of a session,
/// wherever the cursor found it — a PROJECT OPEN PRS GROUP row or the
/// SESSIONS PANEL's PR ROW. Just enough to fetch, title and scroll it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreviewedPr {
    pub number: u64,
    pub url: String,
    /// Row text, `#42 title` — what the pane says while the body loads.
    pub label: String,
}

/// The Claude Cloud row the pane is describing (`App::previewed_cloud`):
/// enough to title the CLOUD SESSION PANEL and open the session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CloudPreview {
    pub id: AgentId,
    pub name: String,
    /// The `session_…` id the create printed.
    pub cloud_session_id: String,
    /// The session's page on claude.ai.
    pub url: String,
}

/// How recently a project's open-PR list may have been fetched and still be
/// refetched on arrival — at a project, at a sidebar panel, or back at the
/// terminal window. Walking the project list, or a flurry of focus events,
/// must not turn into one API call per gesture; a few seconds is enough to
/// coalesce those while still beating the steady beat by a wide margin.
pub const OPEN_PRS_MIN_AGE: std::time::Duration = std::time::Duration::from_secs(5);

/// One session that stopped to ask the user, as the desktop notification
/// names it: the row's name and where it runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FeedbackAlert {
    /// The session row's name.
    pub session: String,
    /// `<project> · <branch>`, the worktree it runs in; empty when the tree
    /// no longer holds them.
    pub place: String,
}

/// The FOLLOW-UP COMPOSER: the box a session card grows when it is
/// expanded, and the next turn being typed into it.
///
/// One at a time, because it owns the keyboard while it is open: the
/// SESSIONS PANEL's own keys (`j`, `a`, `d`…) are letters, so a card with a
/// live box takes every key the panel would otherwise act on. It is bound
/// to the AGENT rather than to a row index — the list re-sorts on every
/// status change, and the box has to stay on the card it was opened on.
pub struct FollowUp {
    pub agent: AgentId,
    /// Multi-line, like the QUICK PROMPT's box: Enter sends, Shift+Enter /
    /// ⌥Enter / `^J` break the line.
    pub input: TextInput,
}

/// The ROWS MEMO: where the cursor is, worked out once for a stretch that
/// asks it over and over and changes none of what the answer is built
/// from — one frame (`ui::draw`), one [`App::reading_url`].
///
/// A frame asks a dozen times: the grid and the pane's strip for the card
/// under the cursor, the pane for whether it is reading a pull request,
/// an issue or a cloud row, the footer for its breadcrumb. Every asking
/// re-sorted the projects and the selected project's checkouts from a
/// roll-up of every session on the machine, and cloned the checkout's
/// session rows to hand back one of them — on a machine with a few
/// hundred sessions, most of a debug build's frame, and a wheel notch
/// over the pane waited behind it.
///
/// Off outside a stretch, where everything is built fresh as it always
/// was: a handler that moves the cursor and then asks where it is must
/// get the new answer. Within one, the kept rows also answer only for the
/// cursor and the tree's shape they were built for ([`RowsKey`]), so a
/// stretch that moved either would rebuild rather than read stale rows.
#[derive(Default)]
pub struct RowsMemo {
    armed: std::cell::Cell<bool>,
    key: std::cell::Cell<Option<RowsKey>>,
    /// [`App::project_rows`].
    projects: std::cell::RefCell<Option<Vec<usize>>>,
    /// [`App::visible_worktrees`], as indices into `tree.worktrees`.
    worktrees: std::cell::RefCell<Option<Vec<usize>>>,
    /// [`App::visible_session_rows`].
    sessions: std::cell::RefCell<Option<Vec<SessionRow>>>,
}

/// What the kept rows were built for: the three cursors and how many of
/// each thing the tree holds.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct RowsKey {
    cursor: (usize, usize, usize),
    shape: [usize; 5],
    show_archived: bool,
}

impl RowsMemo {
    /// Start a stretch: nothing kept yet, everything asked from here on
    /// is kept until [`RowsMemo::disarm`].
    pub fn arm(&self) {
        self.clear();
        self.armed.set(true);
    }

    /// End the stretch and let go of what it kept.
    pub fn disarm(&self) {
        self.armed.set(false);
        self.clear();
    }

    /// Run `f` as a stretch of its own, or as part of the one already
    /// running.
    pub fn hold<R>(&self, f: impl FnOnce() -> R) -> R {
        if self.armed.get() {
            return f();
        }
        self.arm();
        let out = f();
        self.disarm();
        out
    }

    fn clear(&self) {
        self.key.set(None);
        self.projects.take();
        self.worktrees.take();
        self.sessions.take();
    }

    /// `read` what `slot` keeps for `key`, building it the first time the
    /// stretch asks; outside a stretch, `read` a fresh build.
    fn with<T, R>(
        &self,
        slot: fn(&Self) -> &std::cell::RefCell<Option<T>>,
        key: RowsKey,
        build: impl FnOnce() -> T,
        read: impl FnOnce(&T) -> R,
    ) -> R {
        if !self.armed.get() {
            return read(&build());
        }
        if self.key.get() != Some(key) {
            self.clear();
            self.key.set(Some(key));
        }
        if slot(self).borrow().is_none() {
            // Built before it is stored: a build asks the slots below it
            // (the session rows ask for the checkout, which asks for the
            // project), never its own.
            let built = build();
            *slot(self).borrow_mut() = Some(built);
        }
        read(slot(self).borrow().as_ref().expect("built above"))
    }
}

pub struct App {
    pub tree: Tree,
    pub focus: Focus,
    /// Selected row in the Projects panel — indexes `project_rows()`,
    /// every project in display order.
    pub sel_project: usize,
    pub sel_worktree: usize,
    pub sel_session: usize,
    /// First visible row of the Sessions panel, in panel rows (not list
    /// indices — group headers and pill pads take rows too). The wheel
    /// moves it freely; the draw clamps it to the content height and
    /// re-anchors it on the selected row whenever `sessions_anchor` shows
    /// the selection moved (so arrows follow the cursor but the wheel
    /// doesn't fight it).
    pub sessions_scroll: usize,
    /// `(sel_worktree, sel_session, follow-up rows)` as of the last draw —
    /// the draw re-anchors `sessions_scroll` only when this changes. The
    /// third member is how tall the FOLLOW-UP COMPOSER drew: expanding a
    /// card, and every line typed into it, scrolls the column after the box
    /// the way a moved cursor scrolls it after the selection.
    pub sessions_anchor: Option<(usize, usize, usize)>,
    /// First visible row of the Worktrees panel, in panel rows. Same
    /// contract as `sessions_scroll`: the wheel moves it freely, the draw
    /// clamps it and re-anchors on the cursor when `worktrees_anchor` shows
    /// the selection moved. A project with a long open-PR list routinely
    /// outgrows the column.
    pub worktrees_scroll: usize,
    /// `(sel_project, sel_worktree)` as of the last draw.
    pub worktrees_anchor: Option<(usize, usize)>,
    /// How many pill rows the Worktrees column had room for as of the
    /// last draw — the page Ctrl+d / Ctrl+u jump by half of. Zero before
    /// the first frame, when a half page is a single row.
    pub worktrees_view_rows: usize,
    /// The same for the Sessions column: pill rows it had room for as of
    /// the last draw, the page its Ctrl+d / Ctrl+u halve. A worktree
    /// with a long ARCHIVED group outgrows the column the way a project
    /// with many open pull requests outgrows Worktrees.
    pub sessions_view_rows: usize,
    pub term: Option<AttachedTerm>,
    /// Screens of the sessions the pane showed most recently, most recent
    /// first — at most [`TERM_CACHE_MAX`], each under [`TERM_CACHE_CELLS`].
    /// Coming back to one puts its last screen up on the frame the cursor
    /// moves and asks the daemon only for the bytes it missed
    /// ([`AttachedTerm::next_seq`]), instead of replaying the whole ring
    /// into a fresh parser. Only live, painted, real sessions are kept: a
    /// reaped or exited one comes back as a new process whose ring starts
    /// over, and a QUICK PROMPT stand-in never had a PTY.
    pub term_cache: Vec<AttachedTerm>,
    /// Input lock: keys forward to the attached PTY. Focusing the terminal
    /// pane alone (Tab / arrows) does NOT lock — Enter, a click, or `z` does.
    pub term_locked: bool,
    pub conn: ConnState,
    pub hits: Vec<(Rect, HitTarget)>,
    /// Inner rect of the terminal pane from the last draw.
    pub term_area: Rect,
    /// Where the host terminal's own cursor is parked once a frame is
    /// flushed: the cell under the cursor of the PTY the keyboard is
    /// headed for — the editor modal's, else the attached session's — or
    /// None to leave it wherever the frame's last diff run ended. The host
    /// cursor stays hidden either way (the pane paints its own), but a
    /// CJK input method anchors its composition — the preedit text and
    /// the candidate window — to the hardware cursor's cell, hidden or
    /// not, so an unparked cursor put Japanese preedit at the edge of the
    /// window instead of at the prompt (#53). Set by `ui::draw`, applied
    /// by the event loop after the frame.
    pub host_cursor: Option<Position>,
    pub dirty: bool,
    pub should_quit: bool,
    /// Set with `should_quit` when the hosts picker chose a destination:
    /// after teardown the binary execs `nebula ssh` at it, replacing this
    /// process with a fresh connection.
    pub pending_ssh: Option<crate::hosts::HostEntry>,
    pub flash: Option<String>,
    /// The newest release published on GitHub (`0.22.0`) when it is newer
    /// than this build — the footer's `⇡ v0.22.0` beside the version
    /// nameplate. `None` until the update check finds one; a check that
    /// can't ask leaves it as it was.
    pub update_available: Option<String>,
    /// The last `h`/`l` (or ←/→) that landed on the end of the panel row,
    /// or `k`/`j` (↑/↓) on a panel's first row, and stayed put, with when
    /// it arrived: a second press of the same action inside `DOUBLE_TAP`
    /// jumps the boundary the way ⇧Tab / Tab would. Any other key in
    /// between clears it.
    pub edge_tap: Option<(crate::keymap::Action, std::time::Instant)>,
    pub overlay: Option<Overlay>,
    /// The expanded session card's FOLLOW-UP COMPOSER, or None with every
    /// card folded. Not an `Overlay`: it draws inside the SESSIONS PANEL
    /// and the panels stay live around it — what it takes is the keyboard,
    /// not the screen.
    pub follow_up: Option<FollowUp>,
    pub show_archived: bool,
    /// The Worktrees panel's OPEN PRS group folded down to its header (a
    /// click on it). Like `show_archived`, it rides the UI-state blob so a
    /// restart brings it back folded.
    pub open_prs_collapsed: bool,
    /// The ISSUES group under it, folded and remembered the same way.
    pub issues_collapsed: bool,
    /// Sidebars collapsed (z) — terminal takes the full width.
    pub collapsed: bool,
    /// Draft pull requests left out of the PROJECT OPEN PRS GROUP and the
    /// `/` PALETTE; mirrors CONFIG.JSON's `hide_draft_prs` (Settings →
    /// Appearance, or the Worktrees panel menu). Read on every look at
    /// the list (`listed_open_prs`), never applied to what is stored.
    pub hide_draft_prs: bool,
    /// PROJECT SETTINGS as the panels read them: each project's own entry
    /// keyed by repo path, and beside it what a project without one gets;
    /// mirror CONFIG.JSON's `projects` map and `Config::project_fallback`
    /// (Settings → Project). Resolved by `project_settings`; the one row
    /// so far is read through `root_hidden`.
    pub projects_config: BTreeMap<PathBuf, crate::config::ProjectSettings>,
    pub project_fallback: crate::config::ProjectSettings,
    /// How many RECENT PROMPTS the SESSIONS PANEL lists under each
    /// session, newest at the bottom; 0 draws none. Mirrors CONFIG.JSON's
    /// `recent_prompts` switch and `recent_prompts_count` (Settings →
    /// Experimental), resolved through `Config::recent_prompts_shown`.
    pub recent_prompts: usize,
    /// The KEY COMBO DISPLAY is on; mirrors CONFIG.JSON's `show_key_combos`
    /// (Settings → Experimental). `key_combo` is what it is showing.
    pub show_key_combos: bool,
    /// PR & ISSUE COUNTS are on: each PROJECTS PANEL row counts its repo's
    /// open pull requests and issues after its name; mirrors CONFIG.JSON's
    /// `pr_issue_counts` (Settings → Experimental). Read through
    /// `project_open_counts`, and what lets `issues::sweep_others` ask
    /// about the projects the cursor is not on.
    pub pr_issue_counts: bool,
    /// Nothing in the LAUNCHER VIEW's GRID is selected: the aim has been
    /// let go of — by a click on the air between the cards, or by the
    /// first Esc (`event_loop::launcher::clear_aim`). No card is drawn
    /// wearing the cursor while it is set, and the box `p` opens has no
    /// card to read a checkout off, so it is aimed at the project's ROOT
    /// BRANCH (`event_loop::launcher::open_box`) — the way out of a box
    /// locked onto whichever worktree the cursor was last parked in.
    /// Anything that puts the cursor back on a card takes the aim back
    /// (`event_loop::launcher::take_aim`).
    pub launcher_unaimed: bool,
    /// Height the LAUNCHER VIEW's PANE was dragged to, in rows; None until
    /// its top edge is dragged, which leaves the pane on its default share
    /// of the body. Re-clamped to the body on every draw
    /// (`launcher::pane_height`), so a height kept from a taller window
    /// never squeezes the cards out.
    pub launcher_pane_h: Option<u16>,
    /// Width the PANE was dragged to while it stands beside the cards, in
    /// columns: [`App::launcher_pane_h`]'s twin for a pane on the right or
    /// the left, kept apart from it so switching sides never reads a
    /// height as a width. Re-clamped the same way (`launcher::pane_width`).
    pub launcher_pane_w: Option<u16>,
    /// Where the PANE sits against the GRID — along the bottom, or down
    /// the right or the left side — as Settings → Appearance → **Session
    /// pane** has it (`event_loop::apply_config`). What a frame lays out
    /// is [`App::launcher_pane_side`], which falls back to the bottom on a
    /// window too narrow to stand the pane beside the cards.
    pub launcher_pane_at: crate::launcher::PaneSide,
    /// The LAUNCHER VIEW's PANE is folded away (`^~`): the GRID takes the
    /// whole body and no session is read under it. Hiding it also lets
    /// the card under the cursor go (`event_loop::launcher::toggle_pane`
    /// runs the same `clear_aim` the first Esc does), so the cards stand
    /// on their own — nothing selected, nothing being read. It is
    /// remembered across restarts with the pane's height.
    pub launcher_pane_hidden: bool,
    /// Which TERMINAL the LAUNCHER VIEW's PANE is reading instead of the
    /// card under the cursor — the tab its header's strip is on, None for
    /// SESSION (`event_loop::launcher::show_pane_tab`).
    ///
    /// Scoped to the checkout, because the strip is: it lists that
    /// worktree's terminals and nobody else's, so a cursor walked into
    /// another checkout is reading a different strip and the pin no
    /// longer names anything on it. [`App::pinned_terminal`] is the pin
    /// read through that rule; [`App::settle_pane_tab`] drops one the
    /// cursor has walked out from under. Not remembered across restarts:
    /// a terminal's PTY doesn't outlive the daemon's hold on it, and
    /// nebula opens on the session.
    pub launcher_terminal: Option<TerminalId>,
    /// In-progress drag of that edge: `boundary row - grab row` at
    /// mouse-down, so the edge tracks the pointer instead of jumping by
    /// one depending on which of the two grab rows was caught (the
    /// [`SplitterDrag::grab_offset`] pattern).
    pub launcher_pane_drag: Option<i32>,
    /// That edge is under the mouse, or being dragged: its grip lights up.
    /// Only ever set in terminals that report plain mouse motion;
    /// elsewhere the grip rests until a drag takes hold.
    pub hover_launcher_pane: bool,
    /// The header button under the pointer, from the last mouse report:
    /// a PROJECT TAB, its `×`, the `+` in front of them, a full-screen
    /// session's `‹ sessions`, or the footer's memory readout (the one
    /// button off the header). The header marks that one for as long as
    /// it is there, so a word reads as the button it is before anyone
    /// clicks to find out. Carries `hover_launcher_pane`'s caveat — only
    /// terminals that report plain motion ever set it, and elsewhere the
    /// header rests plain.
    pub hover_crumb: Option<HitTarget>,
    /// The LAUNCHER VIEW's PROJECT TABS: every project opened on the
    /// SESSIONS level since its tab was last closed, the most recently
    /// opened first — the far left of the header. Kept up by
    /// [`App::settle_project_tabs`], closed one at a time by
    /// `event_loop::launcher::close_tab`, and remembered across restarts.
    pub launcher_tabs: Vec<ProjectId>,
    /// The PROJECT TABS have the keyboard, and this is the tab their
    /// cursor is on: `k`,`k` (↑,↑) on the GRID's top row walks up into the
    /// header (`event_loop::launcher::focus_tabs`), `h` / `l` move this
    /// cursor along the tabs and switch the grid to each project as they
    /// pass, on the card it was last left on, and Enter — or `j`,`j` back
    /// down — hands the keys back to that card
    /// (`event_loop::launcher::choose_tab`). None with the keys on the
    /// cards, which is every other moment: Esc, a click anywhere, and any
    /// other key hand them back. Never remembered across a restart.
    pub launcher_tab_cursor: Option<ProjectId>,
    /// The git repository this instance was started in
    /// ([`launch_repo`]), read once at launch: the folder the first run's
    /// SPLASH opens on Enter and the open-project prompt starts on.
    pub launch_repo: Option<PathBuf>,
    /// The whole body the LAUNCHER VIEW splits, from the last draw.
    /// `body_area` there is the grid's half alone, so the pane drag takes
    /// its bounds from here.
    pub launcher_body: Rect,
    /// The last key press, spelled for the bottom-left of the screen with
    /// what it did, while the display is on and the press is fresh; the
    /// loop clears it after `key_combo::LINGER`. See `key_combo.rs`.
    pub key_combo: Option<crate::key_combo::KeyCombo>,
    pub next_req_id: u64,
    pub pending: HashMap<u64, PendingIntent>,
    /// In-flight creates — keys of `pending` — the user has navigated away
    /// from since firing them: a key or a click moved a cursor or FOCUS
    /// while the DAEMON worked (a PR SESSION's fetch and `git worktree
    /// add` are seconds). Their Ack still turns the stand-ins into the
    /// real rows, but leaves the cursors, the pane and FOCUS where the
    /// user put them — a manual move outranks a selection-follow. Filled
    /// by `event_loop::handle_terminal_event`, emptied by the Ack or Error.
    pub left_behind: std::collections::HashSet<u64>,
    /// Session created by us, awaiting its upsert to fix the selection.
    pub select_when_seen: Option<SessionRef>,
    /// The session this client just launched, held first in the sessions
    /// lists until its own first turn starts. It is the launch with nothing
    /// to submit that needs this — one carrying a task is created `running`
    /// and leads on its stamp alone ([`recency_key`]). Without it such a row
    /// arrives `fresh`, stamped a moment ago, while every session mid-turn
    /// counts as interacting *now* ([`last_interaction_ms`]) — so the new
    /// card landed *below* the working ones and only jumped to the top left
    /// a second later, when its first turn began. Cleared by that first
    /// status change (recency holds the card there from then on), and
    /// replaced by the next launch; a row that has gone away just stops
    /// matching, so nothing has to clear it.
    pub just_launched: Option<AgentId>,
    /// Project added by us, awaiting its upsert to fix the selection.
    pub select_project_when_seen: Option<ProjectId>,
    /// Worktree created by us, awaiting its upsert to fix the selection.
    pub select_worktree_when_seen: Option<WorktreeId>,
    /// A RUN TERMINAL `r` started whose upsert had not landed when its Ack
    /// did, and the branch it runs in: the flash names the command once
    /// the row arrives.
    pub run_flash_when_seen: Option<(TerminalId, String)>,
    /// Last selected worktree per project — switching back to a project
    /// returns to the worktree the user left it on.
    pub last_worktree_for_project: HashMap<ProjectId, WorktreeId>,
    /// Last selected session per worktree — switching back to a worktree
    /// re-shows the session the user left it on.
    pub last_session_for_worktree: HashMap<WorktreeId, SessionRef>,
    /// Debounced session prewarm: the worktree whose dead sessions the
    /// daemon should pre-spawn once the selection has rested on it past the
    /// deadline — armed on every worktree context switch, so walking the
    /// list doesn't boot every CLI it passes.
    pub pending_prewarm: Option<(WorktreeId, std::time::Instant)>,
    /// A refused PR SESSION's box that could not come back because another
    /// modal was up when the refusal landed — the DAEMON fetches before it
    /// refuses, seconds after Enter, so the user has usually moved on. It
    /// is never put over that modal (whose own text would go); the next
    /// quick prompt opened on the same pull request starts from it
    /// instead, as a refused pull request comment does.
    pub parked_pr_prompt: Option<(String, String)>,
    /// The QUICK PROMPT box last abandoned with something typed in it
    /// (`quick_prompt::QuickDraft`) — Esc, a click outside, the HARDWIRED
    /// UNLOCK. The next box opened takes it back, so a press that closes
    /// the box costs nothing typed; one slot, never written to disk.
    pub quick_draft: Option<crate::quick_prompt::QuickDraft>,
    /// Debounced attach: the session the pane is showing but the daemon has
    /// not been told about yet. Stepping a selection is not a decision to
    /// boot a CLI — walking the grid past four cards must not cold-spawn
    /// four agents and abandon three of them.
    pub pending_attach: Option<(SessionRef, std::time::Instant)>,
    /// What this connection is attached to daemon-side. Lags `term.sref`
    /// while an attach waits out its debounce, so the Detach that precedes
    /// the next Attach names the session the daemon actually holds.
    pub attached_sref: Option<SessionRef>,
    /// Standing keep-warm: when to next re-assert the selected worktree's
    /// warm default-spec Claude session, so one is always ready to adopt.
    /// Re-armed after every send; disarmed when nothing is selected.
    pub next_keepwarm: Option<std::time::Instant>,
    /// Mouse drag-selection over the terminal pane, if any.
    pub term_selection: Option<TermSelection>,
    /// The next EDGE AUTO-SCROLL tick: set while a drag-selection's pointer
    /// rests past the pane's top or bottom edge, so the history keeps
    /// scrolling under it on a fixed beat with no further mouse report;
    /// None once the pointer is back inside or the button is up.
    pub next_drag_autoscroll: Option<std::time::Instant>,
    /// The session whose program holds the left button: it asked for the
    /// mouse (Claude Code's fullscreen renderer, vim `mouse=a`, htop), the
    /// press on the pane went to it, and the drag and release that follow
    /// go to it too — wherever the pointer has wandered by then. Cleared by
    /// the release; a press on anything else starts over.
    pub term_mouse_grab: Option<SessionRef>,
    /// Last left-click on the terminal pane (time + pane-relative cell), for
    /// double-click detection.
    pub last_term_click: Option<(std::time::Instant, (u16, u16))>,
    /// Last left-click on a session row (time + session), for double-click
    /// attach detection (a single click only selects the row).
    pub last_session_click: Option<(std::time::Instant, RowKey)>,
    /// URLs detected on the visible screen during the last draw; hit-tested
    /// on ⌥click and underlined by the renderer.
    pub term_links: Vec<crate::links::TermLink>,
    /// File paths detected on the visible screen during the last draw;
    /// ⌥click opens them in the editor modal.
    pub term_file_links: Vec<crate::links::FileLink>,
    /// File-list width of the diff modal, remembered across opens.
    pub diff_files_width: u16,
    /// The diff modal lists its files as a directory tree (`Ctrl+t` inside
    /// it), remembered across opens and launches like the width.
    pub diff_tree: bool,
    /// Selected tab of the settings modal, remembered across opens.
    pub settings_tab: usize,
    /// Cursor row of the settings modal, one per tab, remembered across
    /// opens so switching tabs and coming back lands where you left.
    pub settings_selected: Vec<usize>,
    /// Where the settings cursor was parked when the overlay last closed:
    /// on the tab strip, or down in the list. True until the first visit
    /// puts it somewhere, so a fresh overlay opens with the strip focused
    /// and ←/→ immediately mean "walk the tabs".
    pub settings_on_tabs: bool,
    /// When the settings overlay was last closed. The remembered position
    /// above is only worth restoring while it's still fresh in the user's
    /// head: a reopen more than [`SETTINGS_MEMORY_TTL`] after this forgets
    /// it and starts over like a first open. `None` until the first close.
    pub settings_closed_at: Option<std::time::Instant>,
    /// Hotkeys as the panels dispatch them: `config.keymap()`, cached here
    /// because a keymap lookup happens on every single key press. The
    /// event loop refreshes it at startup and whenever a binding changes.
    pub keymap: crate::keymap::Keymap,
    /// Pointer shape the outer terminal should currently show (OSC 22).
    pub pointer_shape: PointerShape,
    /// Base64 payload waiting to go out as an OSC 52 clipboard request, set
    /// when the copy has to be delegated to the attached terminal (see
    /// `copy_and_flash`). The main loop writes and clears it.
    pub pending_clipboard: Option<String>,
    /// A turn reached FINISHED since the last frame: the main loop rings
    /// the DONE SOUND (`Config::done_sound`) once and clears it — once per
    /// frame however many rows finished together.
    pub pending_ding: bool,
    /// Sessions that reached NEEDS FEEDBACK since the last frame, one entry
    /// each: the main loop rings the FEEDBACK SOUND (`Config::feedback_sound`)
    /// once for the lot, posts a desktop notification per entry while the
    /// terminal window is in the background, and clears it. A session whose
    /// pane the user is locked into typing at, window focused, is never
    /// queued — that prompt is already in front of them.
    pub pending_feedback: Vec<FeedbackAlert>,
    /// Whether the terminal window has focus, from the focus reports
    /// (mode 1004) `setup_terminal` asks for. True until the terminal says
    /// otherwise, so one that never reports (tmux without `focus-events`)
    /// keeps every desktop notification off rather than posting them while
    /// the user is looking.
    pub window_focused: bool,
    /// Body rect (everything above the footer) from the last draw; bounds
    /// splitter drags.
    pub body_area: Rect,
    /// Short machine hostname, shown at the far left of the footer.
    pub hostname: String,
    /// Running inside an ssh session (SSH_CONNECTION/SSH_TTY) — the footer
    /// colors the hostname as a remote warning.
    pub is_remote: bool,
    /// Active color theme. From config (`theme`); the event loop refreshes
    /// it when the setting changes.
    pub theme: crate::theme::Theme,
    /// Embedded editor modal (find-in-files Enter), above every overlay.
    pub vim: Option<crate::vim_term::VimTerm>,
    /// Where editor reader threads send output; the main loop installs it.
    pub vim_tx: Option<tokio::sync::mpsc::UnboundedSender<crate::vim_term::VimEvent>>,
    /// Stamp for the current editor spawn, so a closed editor's buffered
    /// events can't touch its successor.
    pub vim_generation: u64,
    /// Changed-file count of the selected worktree's checkout (staged +
    /// unstaged + untracked), the worktree panel's bottom badge. Keyed by
    /// worktree so a selection change can't show another checkout's count;
    /// the inner `None` means the checkout wasn't readable. The event loop
    /// refreshes it on a slow poll and on a changed selection — off the
    /// loop, since a `git status` is tens of milliseconds on a big checkout
    /// and the badge is not worth a late frame; the selection guard means a
    /// pending answer shows nothing rather than another checkout's count.
    pub git_changes: Option<(WorktreeId, Option<usize>)>,
    /// The checkout whose count is being read right now, so a repaint can't
    /// stack `git status` processes; the answer clears it.
    pub git_changes_inflight: Option<WorktreeId>,
    /// The last changed-file count read in each checkout, and when: what
    /// the LAUNCHER VIEW's cards print beside their branch. Fed by the
    /// selected checkout's own reads (`git_changes`) and by a sweep that
    /// spends each poll tick on one other checkout the grid lists, the
    /// least recently read first. The count is None when git couldn't say.
    pub worktree_changes: HashMap<WorktreeId, (Option<usize>, std::time::Instant)>,
    /// The checkout the sweep is reading right now; its answer clears it.
    pub worktree_changes_inflight: Option<WorktreeId>,
    /// The lines added and removed in each checkout, read beside its
    /// changed-file count while CARD LINE COUNTS is on: what its cards
    /// print after `+3 files`. Only a checkout with changed lines has an
    /// entry.
    pub worktree_lines: HashMap<WorktreeId, crate::git_diff::LineChanges>,
    /// Mirrors CONFIG.JSON's `card_line_changes` (Settings → Appearance):
    /// the reads behind `worktree_lines` run, and the cards print them,
    /// only while it is on.
    pub card_line_changes: bool,
    /// What `gh pr view` last said about each worktree's branch: `Some(pr)`
    /// when one exists, `None` when the lookup came back empty (no PR, no
    /// `gh`, no remote). A missing key means "not looked up yet" — briefly,
    /// for the selected project: its selected checkout is asked on every
    /// tick, and the others take turns on a sweep, so every row learns its
    /// merge without being visited. An empty answer is re-asked on a
    /// backing-off timer (`pr_recheck`), since the PR a session opens
    /// appears well after the first lookup; a found one keeps being
    /// re-asked on a beat, for its conversation and its state. At startup
    /// the found ones come back from the last run's cache (`pr_cache`), so
    /// the rows are painted before the first lookup answers.
    pub pull_requests: HashMap<WorktreeId, Option<PullRequest>>,
    /// When this client saw a checkout's pull request turn merged
    /// (`note_merge_landed`) — what times the row's ONE-SHOT SWEEP. Only a
    /// merge seen to happen is stamped: a row the cache hydrated as merged,
    /// or whose first answer ever says merged, landed some other day and
    /// paints solid purple from the first frame.
    pub merge_landed: HashMap<WorktreeId, std::time::Instant>,
    /// How far the user has read into each pull request's conversation,
    /// keyed by PR URL — the daemon's `pr_seen` rows, plus whatever this
    /// session has marked since. What's newer than the mark is what the
    /// row's unread badge counts.
    pub pr_seen: HashMap<String, String>,
    /// Worktrees with a lookup in flight, so a repaint can't stack a second
    /// `gh` process on the first.
    pub pr_inflight: std::collections::HashSet<WorktreeId>,
    /// When to ask `gh` about a worktree again, and the step that produced
    /// that deadline: a steady beat once its pull request is known (a quick
    /// one for the selected checkout, so the unread-comment count keeps up;
    /// a slow one for the rest, which only have to keep up with a merge), a
    /// doubling backoff while it isn't. Switching into a worktree drops its
    /// entry, so arriving somewhere always asks again promptly; so does its
    /// pull request leaving the project's open list.
    pub pr_recheck: HashMap<WorktreeId, (std::time::Instant, std::time::Duration)>,
    /// What `gh pr list` last said about each project's open pull requests
    /// — the group at the bottom of the Worktrees panel. A missing key
    /// means "never asked"; only the selected project is ever asked, so a
    /// machine with thirty projects still costs one call per refresh.
    pub open_prs: HashMap<ProjectId, OpenPrs>,
    /// Projects with a list lookup in flight, so a repaint can't stack a
    /// second `gh` on the first.
    pub open_prs_inflight: std::collections::HashSet<ProjectId>,
    /// Bodies and conversations of the pull requests the cursor has rested
    /// on, keyed by URL. A second API call on top of the list, so it is
    /// fetched only for the row actually being read and kept for the whole
    /// session — a pull request's description doesn't change while you read
    /// it, and its comments ride the list's own refresh.
    pub pr_detail: HashMap<String, PrDetail>,
    /// Pull requests whose detail is in flight, and ones `gh` couldn't
    /// answer for — the pane says "couldn't reach gh" rather than spinning
    /// on a request that already came back empty.
    pub pr_detail_inflight: std::collections::HashSet<String>,
    pub pr_detail_failed: std::collections::HashSet<String>,
    /// Debounced detail fetch: the pull request under the cursor and when
    /// its lookup is due. Re-armed on every move, so walking a list of a
    /// hundred rows fetches only the ones actually paused on.
    pub pending_pr_detail: Option<(PendingPrDetail, std::time::Instant)>,
    /// `r` on a worktree or pull-request row asked for the pull requests
    /// *now*: the event loop runs the two list lookups on its next turn
    /// instead of waiting for the git tick, then clears this.
    pub pr_refresh_requested: bool,
    /// Top visible line of the reading pane — the pull request or the
    /// issue under a cursor (`App::reading_url`) — and the pane's total
    /// line count as of the last draw (for clamping).
    pub pr_preview_scroll: u16,
    pub pr_preview_lines: usize,
    /// The pull request whose full diff is being fetched, if any — one at a
    /// time, so mashing the key can't spawn a `gh pr diff` per press.
    pub pr_diff_inflight: Option<u64>,
    /// Pull requests whose *cached* diff the modal opened on while a fresh
    /// `gh pr diff` runs underneath. When that lands it replaces the modal's
    /// contents in place if the modal is still on the same pull request,
    /// and is only cached otherwise — never a second modal popping up over
    /// whatever the user moved on to.
    pub pr_diff_refreshing: std::collections::HashSet<String>,
    /// Where a finished `gh pr diff` is sent back to the loop; the main loop
    /// installs it at startup (the `vim_tx` precedent). Key handlers can
    /// therefore start a network fetch without the loop's channels in hand.
    pub pr_diff_tx: Option<tokio::sync::mpsc::UnboundedSender<PrDiffAnswer>>,
    /// Pull requests (by URL) with a `gh pr comment` still running. A
    /// second box sent on the same pull request waits for the first to
    /// land rather than racing it.
    pub pr_comment_inflight: std::collections::HashSet<String>,
    /// Comment text a refused post handed back while another modal was
    /// up, by PR URL: the next COMMENT BOX opened on that pull request
    /// starts from it, so the refusal cost nothing typed.
    pub pr_comment_drafts: HashMap<String, String>,
    /// Where a finished `gh pr comment` lands (`PrCommentAnswer`);
    /// installed by the loop at startup like `pr_diff_tx`.
    pub pr_comment_tx: Option<tokio::sync::mpsc::UnboundedSender<PrCommentAnswer>>,
    /// The on-disk memory of every pull-request answer (`pr_cache`), when
    /// this instance has one: the main loop installs the real one at
    /// startup and hydrates from it; the unit tests leave it `None`, so no
    /// test touches the real user's cache. Written whenever
    /// `pr_cache_dirty` says something in it changed — at most once per
    /// GIT POLL, plus once on quit.
    pub pr_cache: Option<crate::pr_cache::PrCache>,
    pub pr_cache_dirty: bool,
    /// Where a file dropped onto a prompt box bound for an agent is copied
    /// before macOS deletes it (`dropped_files`): the main loop installs
    /// the DATA DIR's `attachments/` at startup; the unit tests leave it
    /// `None`, so a paste there is never staged into the real user's dir.
    pub attachments_dir: Option<std::path::PathBuf>,
    /// Bodies in `pr_detail` that came from the cache rather than from
    /// `gh`. The pane shows them at once; resting the cursor on their row
    /// fetches a fresh copy over the top, as it would fetch a missing one,
    /// and the answer takes the URL out of here.
    pub pr_detail_stale: std::collections::HashSet<String>,
    /// What `gh issue list` last said about each project's open issues —
    /// the ISSUES MODAL's rows, kept for the session. Prefetched in the
    /// background once the cursor rests on a project and kept fresh on a
    /// slow beat while it stays selected (`issues::schedule_prefetch`,
    /// `issues::refresh_selected`), so `i` paints rows at once; the modal
    /// re-asks on open only past `issues::FRESH`, and on its `r`.
    pub issues: HashMap<ProjectId, crate::issues::IssueList>,
    /// Projects with a list lookup in flight, and ones whose first ask
    /// `gh` couldn't answer (the modal says so rather than spinning).
    pub issues_inflight: std::collections::HashSet<ProjectId>,
    pub issues_failed: std::collections::HashSet<ProjectId>,
    /// When each project's next background list ask is owed — the steady
    /// beat once it has issues, a backoff while it hasn't — so the git
    /// tick spends a `gh` only when one is due.
    pub issues_due: HashMap<ProjectId, crate::issues::IssuesBeat>,
    /// The debounced prefetch: the project the cursor landed on and when
    /// its list is asked for, re-armed on every project switch.
    pub pending_issues_prefetch: Option<(ProjectId, std::time::Instant)>,
    /// The conversations of the issues the cursor has rested on, keyed by
    /// URL; in flight and failed like the pull requests'.
    pub issue_detail: HashMap<String, crate::issues::IssueDetail>,
    pub issue_detail_inflight: std::collections::HashSet<String>,
    pub issue_detail_failed: std::collections::HashSet<String>,
    /// Issues with a comment on its way to GitHub (`gh issue comment`),
    /// by URL, so the reading pane says so until the answer lands.
    pub issue_comment_inflight: std::collections::HashSet<String>,
    /// Debounced comments fetch: the issue under the cursor and when its
    /// lookup is due, re-armed on every move.
    pub pending_issue_detail: Option<(crate::issues::PendingIssueDetail, std::time::Instant)>,
    /// Where a finished `gh issue …` is sent back to the loop; installed at
    /// startup like `pr_diff_tx`, so the modal's own handlers can start a
    /// fetch. `None` in the unit tests, which then never spawn one.
    pub issues_tx: Option<tokio::sync::mpsc::UnboundedSender<crate::issues::IssuesAnswer>>,
    /// BACKGROUND READS for the worktree views (`view_jobs`): the DIFF
    /// VIEWER, the FILE FINDER, its grep view and the TREE BROWSER are
    /// handed a clone when they open, and their git and disk reads land
    /// through the main loop instead of holding it. None with no loop
    /// running (unit tests), where those views read inline.
    pub view_jobs: Option<crate::view_jobs::Jobs>,
    /// A `g` on a checkout the changed-files badge called clean: the ticket
    /// of the `git status` checking that, and the checkout (path, branch)
    /// to open the DIFF VIEWER on if git disagrees.
    pub diff_probe: Option<(u64, PathBuf, String)>,
    /// The changed files the badge's last `git status` listed, and the
    /// checkout they are in (`event_loop::keep_changed_files`): what `g`
    /// opens the DIFF VIEWER on while its own `git status` runs. One
    /// checkout's worth, capped, replaced by every poll.
    pub changed_files: Option<(WorktreeId, Vec<crate::git_diff::DiffFile>)>,
    /// Rows deleted here ahead of the DAEMON's answer
    /// (`event_loop::optimistic`): an upsert of one of them is a straggler
    /// from before the delete, and is ignored rather than shown.
    pub deleting: std::collections::HashSet<nebula_core::EntityId>,
    /// The BRANCH SWITCHER's answer channel, listing cache and fetch
    /// throttle — what outlives the modal.
    pub branch_switch: crate::branch_switch::Shared,
    /// Latest daemon metrics reading (daemon + per-session process trees),
    /// for the footer's memory/session readout. Refreshed on a slow poll;
    /// the metrics modal shares the same replies at a faster cadence.
    pub last_metrics: Option<nebula_core::MetricsSnapshot>,
    /// This TUI process's own RSS, sampled alongside each metrics request
    /// (the daemon can't see us).
    pub client_rss_bytes: u64,
    /// Launch instant; the first-run splash animation and the status-sweep
    /// text animation are pure functions of time elapsed since this. (The
    /// N-key splash preview resets it to restart the fade-in — the sweep
    /// isn't visible under the splash, so the phase jump never shows.)
    pub splash_epoch: std::time::Instant,
    /// Splash summoned on demand (N) with a populated tree; any key
    /// dismisses it.
    pub splash_preview: bool,
    /// The last frame drew the empty GRID's welcome, nebula and all
    /// (`ui::launcher_view`). Cleared at the top of every `ui::draw` and
    /// set again by the welcome itself, so between frames it says what is
    /// on screen — which is what keeps the sky ticking
    /// ([`App::welcome_active`]).
    pub welcome_on_screen: bool,
    /// The `animations` setting: master switch for the status-text sweep
    /// and the splash's motion (off = fewer repaints). Mirrors the config,
    /// refreshed at startup and when the settings overlay applies a change.
    pub animations: bool,
    /// The `focus_tint` setting: paints the focused panel's background
    /// with a faint accent wash. On by default; off leaves the terminal's
    /// own background (transparency included) showing through. Mirrors
    /// the config, refreshed at startup and when the settings overlay
    /// applies a change.
    pub focus_tint: bool,
    /// The `black_background` setting: every cell still on the terminal's
    /// default background is painted pure black at the end of a frame
    /// (`ui::draw`). On in the config by default; off here until startup
    /// applies it. Mirrors the config, refreshed at startup and when the
    /// settings overlay applies a change.
    pub black_background: bool,
    /// The ROWS MEMO, armed by the frame and by [`App::reading_url`].
    pub rows_memo: RowsMemo,
}

impl Default for App {
    fn default() -> Self {
        Self::new()
    }
}

impl App {
    pub fn new() -> Self {
        Self {
            tree: Tree::default(),
            // The GRID is where FOCUS lives; the PANE under it is the only
            // other place it can go.
            focus: Focus::Sessions,
            sel_project: 0,
            sel_worktree: 0,
            sel_session: 0,
            sessions_scroll: 0,
            sessions_anchor: None,
            follow_up: None,
            worktrees_scroll: 0,
            worktrees_anchor: None,
            worktrees_view_rows: 0,
            sessions_view_rows: 0,
            term: None,
            term_cache: Vec::new(),
            term_locked: false,
            conn: ConnState::Disconnected,
            hits: Vec::new(),
            term_area: Rect::default(),
            host_cursor: None,
            dirty: true,
            should_quit: false,
            pending_ssh: None,
            flash: None,
            update_available: None,
            edge_tap: None,
            overlay: None,
            show_archived: false,
            open_prs_collapsed: false,
            issues_collapsed: false,
            collapsed: false,
            hide_draft_prs: false,
            projects_config: BTreeMap::new(),
            project_fallback: Default::default(),
            recent_prompts: 0,
            show_key_combos: false,
            pr_issue_counts: true,
            launcher_unaimed: false,
            launcher_pane_h: None,
            launcher_pane_w: None,
            launcher_pane_at: crate::launcher::PaneSide::Bottom,
            launcher_pane_hidden: false,
            launcher_terminal: None,
            launcher_pane_drag: None,
            hover_launcher_pane: false,
            hover_crumb: None,
            launcher_tabs: Vec::new(),
            launcher_tab_cursor: None,
            launch_repo: None,
            launcher_body: Rect::default(),
            key_combo: None,
            next_req_id: 1,
            pending: HashMap::new(),
            left_behind: std::collections::HashSet::new(),
            select_when_seen: None,
            just_launched: None,
            select_project_when_seen: None,
            select_worktree_when_seen: None,
            run_flash_when_seen: None,
            last_worktree_for_project: HashMap::new(),
            last_session_for_worktree: HashMap::new(),
            pending_prewarm: None,
            parked_pr_prompt: None,
            quick_draft: None,
            pending_attach: None,
            attached_sref: None,
            next_keepwarm: None,
            term_selection: None,
            next_drag_autoscroll: None,
            term_mouse_grab: None,
            last_term_click: None,
            last_session_click: None,
            term_links: Vec::new(),
            term_file_links: Vec::new(),
            diff_files_width: DEFAULT_DIFF_FILES_W,
            diff_tree: false,
            settings_tab: 0,
            settings_selected: vec![0; crate::config::tab_count()],
            settings_on_tabs: true,
            settings_closed_at: None,
            keymap: crate::keymap::Keymap::default(),
            pointer_shape: PointerShape::default(),
            pending_clipboard: None,
            pending_ding: false,
            pending_feedback: Vec::new(),
            window_focused: true,
            body_area: Rect::default(),
            hostname: nebula_core::host::hostname(),
            is_remote: nebula_core::host::is_remote_session(),
            theme: crate::theme::Theme::default(),
            vim: None,
            vim_tx: None,
            vim_generation: 0,
            git_changes: None,
            git_changes_inflight: None,
            worktree_changes: HashMap::new(),
            worktree_changes_inflight: None,
            worktree_lines: HashMap::new(),
            card_line_changes: false,
            pull_requests: HashMap::new(),
            merge_landed: HashMap::new(),
            pr_seen: HashMap::new(),
            pr_inflight: std::collections::HashSet::new(),
            pr_recheck: HashMap::new(),
            open_prs: HashMap::new(),
            open_prs_inflight: std::collections::HashSet::new(),
            pr_detail: HashMap::new(),
            pr_detail_inflight: std::collections::HashSet::new(),
            pr_detail_failed: std::collections::HashSet::new(),
            pending_pr_detail: None,
            pr_refresh_requested: false,
            pr_preview_scroll: 0,
            pr_preview_lines: 0,
            pr_diff_inflight: None,
            pr_diff_refreshing: std::collections::HashSet::new(),
            pr_diff_tx: None,
            pr_comment_inflight: std::collections::HashSet::new(),
            pr_comment_drafts: HashMap::new(),
            pr_comment_tx: None,
            pr_cache: None,
            pr_cache_dirty: false,
            attachments_dir: None,
            pr_detail_stale: std::collections::HashSet::new(),
            issues: HashMap::new(),
            issues_inflight: std::collections::HashSet::new(),
            issues_failed: std::collections::HashSet::new(),
            issues_due: HashMap::new(),
            pending_issues_prefetch: None,
            issue_detail: HashMap::new(),
            issue_detail_inflight: std::collections::HashSet::new(),
            issue_detail_failed: std::collections::HashSet::new(),
            issue_comment_inflight: std::collections::HashSet::new(),
            pending_issue_detail: None,
            issues_tx: None,
            view_jobs: None,
            diff_probe: None,
            changed_files: None,
            deleting: std::collections::HashSet::new(),
            branch_switch: Default::default(),
            last_metrics: None,
            client_rss_bytes: 0,
            splash_epoch: std::time::Instant::now(),
            splash_preview: false,
            welcome_on_screen: false,
            animations: true,
            focus_tint: true,
            black_background: false,
            rows_memo: RowsMemo::default(),
        }
    }

    /// Remembered cursor row for a settings tab, clamped to what that tab
    /// currently holds.
    pub fn settings_row(&self, tab: usize) -> usize {
        self.settings_selected
            .get(tab)
            .copied()
            .unwrap_or(0)
            .min(crate::config::tab_len(tab).saturating_sub(1))
    }

    /// Record where the settings cursor is parked, so the next open lands
    /// in the same place.
    pub fn remember_settings_focus(&mut self, on_tabs: bool) {
        self.settings_on_tabs = on_tabs;
    }

    /// Stamp the moment the settings overlay went away, starting the
    /// [`SETTINGS_MEMORY_TTL`] clock on the remembered position.
    pub fn note_settings_closed(&mut self) {
        self.settings_closed_at = Some(std::time::Instant::now());
    }

    /// The remembered settings position has gone stale: the overlay was
    /// closed more than [`SETTINGS_MEMORY_TTL`] ago. Never true before the
    /// first close — there's nothing to forget yet.
    pub fn settings_memory_expired(&self) -> bool {
        self.settings_closed_at
            .is_some_and(|closed| closed.elapsed() >= SETTINGS_MEMORY_TTL)
    }

    /// Drop the remembered settings position so the next open looks like
    /// the very first one: first tab, top row, cursor on the tab strip.
    pub fn forget_settings_focus(&mut self) {
        self.settings_tab = 0;
        self.settings_selected = vec![0; crate::config::tab_count()];
        self.settings_on_tabs = true;
        self.settings_closed_at = None;
    }

    pub fn remember_settings_row(&mut self, tab: usize, row: usize) {
        if self.settings_selected.len() < crate::config::tab_count() {
            self.settings_selected.resize(crate::config::tab_count(), 0);
        }
        if let Some(slot) = self.settings_selected.get_mut(tab) {
            *slot = row;
        }
    }

    /// The folder name of [`App::launch_repo`] while it is not a project
    /// yet — what the SPLASH's "Enter: open …" names. None once it is one,
    /// or when nebula was started outside a repository.
    pub fn launch_repo_name(&self) -> Option<String> {
        let repo = self.launch_repo.as_ref()?;
        if self.tree.project_at_path(repo).is_some() {
            return None;
        }
        repo.file_name().map(|n| n.to_string_lossy().into_owned())
    }

    /// The LAUNCHER VIEW is what the body draws — which is nebula's only
    /// view — once this machine knows a project. With none at all (a
    /// first run) the splash's "open a project" comes first.
    pub fn launcher_active(&self) -> bool {
        self.tree.has_projects()
    }

    /// The checkout the LAUNCHER PANE's TAB STRIP is scoped to: the one
    /// the card under the GRID's cursor runs in, so the terminals on the
    /// strip and the session the pane reads beside them always belong to
    /// the same checkout — which is the whole claim the strip makes by
    /// naming the branch between them.
    ///
    /// Not [`App::selected_worktree`], which is the PANELS' cursor: a
    /// walk of the grid drags it along (`event_loop::launcher::select`),
    /// but before the first walk it is still parked on whatever the
    /// panels were left on, and the strip would name one checkout while
    /// listing another's terminals. It is the fallback all the same, for
    /// a grid with no card under the cursor at all.
    pub fn pane_worktree(&self) -> Option<WorktreeId> {
        let rows = crate::launcher::rows(self);
        crate::launcher::cursor(self, &rows)
            .and_then(|at| rows.get(at))
            .map(|row| row.agent.worktree_id.clone())
            .or_else(|| self.selected_worktree().map(|w| w.id.clone()))
    }

    /// The TERMINALS the strip lists: that checkout's, in tree order.
    /// [`App::visible_terminals`] is the same list read off the panels'
    /// cursor, for the panels' own rows.
    pub fn pane_terminals(&self) -> Vec<TerminalTab> {
        let Some(wt) = self.pane_worktree() else {
            return Vec::new();
        };
        self.tree
            .terminals
            .iter()
            .filter(|t| t.worktree_id == wt)
            .cloned()
            .collect()
    }

    /// The TERMINAL the LAUNCHER VIEW's PANE is reading, when that pin is
    /// still good: the view is on and the terminal is one of the
    /// checkout's ([`App::pane_terminals`] — what the header's TAB STRIP
    /// lists). None means the pane reads the card under the cursor, which
    /// is what every other tenant of the pane assumes.
    ///
    /// Read rather than trusted, so a pin that has gone stale — the
    /// cursor walked into another checkout, the terminal was closed —
    /// simply stops answering instead of leaving the pane on a session
    /// nobody selected.
    pub fn pinned_terminal(&self) -> Option<TerminalId> {
        if !self.launcher_active() {
            return None;
        }
        let id = self.launcher_terminal.as_ref()?;
        self.pane_terminals()
            .iter()
            .any(|t| &t.id == id)
            .then(|| id.clone())
    }

    /// Drop a pin that no longer names a tab on the strip. Run by the
    /// view's draw, as [`App::settle_launcher_focus`] is: the pin goes
    /// stale by things that happen elsewhere — the cursor walking into
    /// another checkout, a terminal closing — rather than by anything the
    /// pane itself does, so there is no one place to clear it from.
    pub fn settle_pane_tab(&mut self) {
        if self.launcher_terminal.is_some() && self.pinned_terminal().is_none() {
            self.launcher_terminal = None;
        }
    }

    /// Keep the PROJECT TABS true to the tree: a tab whose project is gone
    /// goes, and the project the grid is on gets one at the far left if it
    /// has none — however it got there, whether a tab, the `+` dropdown, a
    /// `/` jump, a folder just opened or the restore at boot. A project
    /// already open keeps its place. Run by the view's draw, as
    /// [`App::settle_pane_tab`] is, and by the tab keys before they read
    /// the list.
    pub fn settle_project_tabs(&mut self) {
        let projects = &self.tree.projects;
        self.launcher_tabs
            .retain(|id| projects.iter().any(|p| &p.id == id));
        // The header's cursor needs a tab to be on and the keys to be on
        // the grid: a tab closed or a project dropped under it, or the
        // pane taking the keys, hands them back to the cards.
        if self.launcher_tab_cursor.as_ref().is_some_and(|id| {
            !self.launcher_tabs.contains(id) || self.focus == Focus::Terminal || self.collapsed
        }) {
            self.launcher_tab_cursor = None;
            self.dirty = true;
        }
        let Some(id) = self.selected_project().map(|p| p.id.clone()) else {
            return;
        };
        if !self.launcher_tabs.contains(&id) {
            self.launcher_tabs.insert(0, id);
            self.dirty = true;
        }
    }

    /// The GRID is what the body is showing: the LAUNCHER VIEW is on and
    /// no session has been opened full-screen over it (`collapsed`, which
    /// `ui::draw` hands to the pane before it ever reaches the view).
    pub fn launcher_grid(&self) -> bool {
        self.launcher_active() && !self.collapsed
    }

    /// Take the keyboard back from the session in the PANE, and say so
    /// when it was really being typed into.
    ///
    /// Every UNASKED drop goes through here: a redraw that finds the pane
    /// gone ([`App::settle_launcher_focus`]), a row archived, deleted or
    /// reaped out from under the pane showing it, a stand-in replaced by
    /// the session it stood for. None of those is a key the user pressed,
    /// and each one leaves the next thing they type meaning something
    /// else — so each one flashes [`TERMINAL_RELEASED`].
    ///
    /// The guard lives here rather than at the call sites: they run from
    /// draws and from daemon events and cannot know whether the lock was
    /// held, and a flash on every frame would be noise. Only a lock that
    /// was actually HELD says anything.
    ///
    /// The deliberate ways out of a pane — `^q`, `^z`, Esc up a level —
    /// name what they did themselves and do not come through here.
    pub fn release_terminal(&mut self) {
        if !self.term_locked {
            return;
        }
        self.term_locked = false;
        self.flash = Some(TERMINAL_RELEASED.into());
        self.dirty = true;
    }

    /// FOCUS as the LAUNCHER VIEW's GRID has it: the cards, or the PANE
    /// under them while something is in it — a click into the pane types
    /// into that session where it stands, and the hatch (`^q`) comes back
    /// out to the cards. Every other focus — a restored UI state parked on
    /// a panel this view doesn't draw — lands on the cards. Run by the
    /// view's draw, as `settle_focus` is by the panels'.
    pub fn settle_launcher_focus(&mut self) {
        // Only the SESSIONS level draws a pane (`launcher::split`), only
        // while it is unfolded (`^~`) and only with a card wearing the
        // cursor for it to read: with nothing under the grid there is
        // nowhere for focus to rest, so it comes back to the cards
        // rather than sitting on a pane that is no longer on screen.
        if self.focus == Focus::Terminal
            && self.term.is_some()
            && !self.launcher_pane_hidden
            && self.launcher_aimed()
        {
            return;
        }
        self.focus = Focus::Sessions;
        // The lock goes with the pane however FOCUS got off it: a click
        // on the air between the cards sets FOCUS itself
        // (`event_loop`'s `PanelBg` arm) before letting the card go, and
        // the input lock left behind would have gone on eating keys with
        // no pane on screen to type into. `release_terminal` is a no-op
        // on a lock that was never held, so only a real one says so.
        self.release_terminal();
    }

    /// The splash is what the body is showing: no project on this machine
    /// yet (first run) or summoned with N, and no session full-screen over
    /// it. True whether it's animating or drawn as a still frame, so the
    /// footer can key its hints off it.
    pub fn splash_showing(&self) -> bool {
        !self.collapsed && (!self.tree.has_projects() || self.splash_preview)
    }

    /// The animated splash is on screen and should be ticking: nothing in
    /// the tree yet (first run) or summoned with N, panels not collapsed,
    /// no editor modal covering the body, animations enabled (off, the
    /// splash still draws — as a still frame).
    pub fn splash_active(&self) -> bool {
        self.animations && self.splash_showing() && self.vim.is_none()
    }

    /// The empty GRID's welcome is on screen and its nebula should be
    /// ticking — the same cadence and the same switch as the splash's.
    /// Off, the welcome still draws, as a still frame.
    pub fn welcome_active(&self) -> bool {
        self.animations && self.welcome_on_screen && self.vim.is_none()
    }

    /// Some sidebar row is showing a running (yellow) or needs-feedback
    /// (red) status, or is inside a ONE-SHOT SWEEP — a turn that just
    /// finished unread (blue), a checkout whose pull request was just seen
    /// to merge (purple) — so its text sweep should be ticking. Any agent
    /// in one of those states surfaces somewhere — its own row, or a
    /// worktree / project rollup — unless the panels are hidden (collapsed,
    /// editor modal, splash) or animations are switched off. A merged
    /// checkout only shows while its project is selected, so only those
    /// keep the clock running. The one-shots run out on the clock, so an
    /// idle app with a week-old merged checkout on screen repaints nothing.
    /// The exception is a PROJECT TAB: its name sweeps blue for as long as
    /// its project has a finish left unread and nothing live, so the clock
    /// runs while one does (the cheap scan for any unread finish first,
    /// since this is asked on every turn of the event loop).
    pub fn status_anim_active(&self) -> bool {
        let now = now_ms();
        self.animations
            && !self.collapsed
            && self.vim.is_none()
            && !self.splash_active()
            && (self.tree.agents.iter().any(|a| {
                !a.archived
                    && (matches!(a.status, AgentStatus::Running | AgentStatus::NeedsFeedback)
                        || fresh_done(a, now))
            }) || self
                .visible_worktrees()
                .iter()
                .any(|w| self.worktree_wears_merge(&w.id) && self.merge_is_fresh(&w.id))
                || self.tab_sweeps_done())
    }

    /// Some PROJECT TAB sweeps blue: its project has an unread finish on
    /// the grid ([`crate::launcher::project_tally`]'s `done`).
    fn tab_sweeps_done(&self) -> bool {
        self.launcher_active()
            && self
                .tree
                .agents
                .iter()
                .any(|a| !a.archived && a.unseen && a.status == AgentStatus::Finished)
            && self
                .launcher_tabs
                .iter()
                .any(|id| crate::launcher::project_tally(self, id).done > 0)
    }

    /// This client just saw `worktree`'s pull request turn merged: start
    /// the row's ONE-SHOT SWEEP. Stamps that have run out go as new ones
    /// arrive, so the map holds a few seconds' worth at most.
    pub fn note_merge_landed(&mut self, worktree: WorktreeId) {
        self.merge_landed
            .retain(|_, at| at.elapsed() < ONE_SHOT_SWEEP);
        self.merge_landed
            .insert(worktree, std::time::Instant::now());
    }

    /// Whether the checkout's merge was seen to land within the last
    /// `ONE_SHOT_SWEEP` — the purple row still sweeps; after, it is solid.
    pub fn merge_is_fresh(&self, worktree_id: &WorktreeId) -> bool {
        self.merge_landed
            .get(worktree_id)
            .is_some_and(|at| at.elapsed() < ONE_SHOT_SWEEP)
    }

    /// Whether the session's unread finish still sweeps ([`fresh_done`]).
    pub fn agent_fresh_done(&self, agent: &Agent) -> bool {
        fresh_done(agent, now_ms())
    }

    /// The same for a worktree row's rollup — or, on a row that wears its
    /// merged pull request, whether the merge still sweeps: the one flag a
    /// checkout's row needs, whichever story it is telling.
    pub fn worktree_fresh(&self, worktree_id: &WorktreeId) -> bool {
        if self.worktree_wears_merge(worktree_id) {
            self.merge_is_fresh(worktree_id)
        } else {
            worktree_fresh_done(&self.tree, worktree_id, now_ms())
        }
    }

    pub fn project_fresh_done(&self, project_id: &ProjectId) -> bool {
        project_fresh_done(&self.tree, project_id, now_ms())
    }

    /// Frame counter for the status-sweep text animation — a pure function
    /// of elapsed time (same model as the splash), so a missed tick just
    /// skips ahead instead of stuttering.
    pub fn sweep_phase(&self) -> usize {
        (self.splash_epoch.elapsed().as_millis() / SWEEP_FRAME.as_millis()) as usize
    }

    /// Is the GRID aimed at a card — is there something for the PANE
    /// along the bottom to read? The selection itself is never let go of
    /// (see [`App::launcher_unaimed`]); this is whether the view is
    /// pointed at it, which `event_loop::launcher::take_aim` sets and
    /// `clear_aim` drops, and which `ui::launcher_view`'s `wearing`
    /// draws the cursor's card by.
    pub fn launcher_aimed(&self) -> bool {
        !self.launcher_unaimed
    }

    /// `body` in two the way the LAUNCHER VIEW draws it: the grid's half
    /// and the PANE — along the bottom, or beside the cards on the side
    /// [`App::launcher_pane_side`] names — or the whole body and no pane
    /// at all. `launcher::split_at` is the geometry — whether there is
    /// room for a pane, and how the two share the rows or the columns;
    /// this is the one place the fold (`^~`) and the aim are read, so the
    /// draw, the drag grip and the keys all agree about whether a pane is
    /// on screen.
    ///
    /// No card wearing the cursor, no pane: the pane is the selected
    /// session, so with nothing selected there is nothing for it to be
    /// and the grid takes the whole body back. Clicking a card selects
    /// it and the pane opens under the cards; letting the card go —
    /// Esc, a click on the air between them — collapses it again.
    pub fn launcher_split(&self, body: Rect) -> (Rect, Option<Rect>) {
        if self.launcher_pane_hidden || !self.launcher_aimed() {
            return (body, None);
        }
        let side = crate::launcher::fitted_side(body, self.launcher_pane_at);
        let want = if side.beside() {
            self.launcher_pane_w
        } else {
            self.launcher_pane_h
        };
        crate::launcher::split_at(body, side, want)
    }

    /// The side the PANE is laid out on this frame: the one Settings asks
    /// for, or the bottom on a body too narrow to stand it beside the cards
    /// (`launcher::fitted_side`). The drag, the grip and the pointer's
    /// arrows all read this one, so none of them can disagree with the
    /// draw about which way the pane's edge runs.
    pub fn launcher_pane_side(&self) -> crate::launcher::PaneSide {
        crate::launcher::fitted_side(self.launcher_body, self.launcher_pane_at)
    }

    /// Where the edge between the cards and the PANE sits on this frame,
    /// by `launcher::pane_boundary`'s measure — a row under the cards, a
    /// column beside them. None with no pane on screen.
    pub fn launcher_pane_boundary(&self) -> Option<i32> {
        let pane = self.launcher_split(self.launcher_body).1?;
        Some(crate::launcher::pane_boundary(
            self.launcher_pane_side(),
            pane,
        ))
    }

    /// Move the LAUNCHER VIEW's pane boundary to `boundary` — the screen
    /// row the pane starts on under the cards, or beside them the column
    /// `launcher::pane_boundary` names — and remember the height, or the
    /// width, that leaves it. `launcher::pane_height` and `pane_width` do
    /// the clamping, so a drag off either end rests against the pane's own
    /// minimum or against what the grid keeps: the header plus one row of
    /// cards, or one column of them. A body with no room for a pane at all
    /// remembers nothing: there is no edge on screen to have grabbed.
    pub fn set_launcher_pane(&mut self, boundary: i32) {
        use crate::launcher::PaneSide;
        let body = self.launcher_body;
        let side = self.launcher_pane_side();
        let want = match side {
            PaneSide::Bottom => i32::from(body.y) + i32::from(body.height) - boundary,
            PaneSide::Right => i32::from(body.x) + i32::from(body.width) - boundary,
            PaneSide::Left => boundary - i32::from(body.x),
        };
        let want = want.clamp(0, i32::from(u16::MAX)) as u16;
        if side.beside() {
            if let Some(w) = crate::launcher::pane_width(body, Some(want)) {
                self.launcher_pane_w = Some(w);
            }
        } else if let Some(h) = crate::launcher::pane_height(body, Some(want)) {
            self.launcher_pane_h = Some(h);
        }
    }

    pub fn alloc_req_id(&mut self, intent: PendingIntent) -> u64 {
        let id = self.next_req_id;
        self.next_req_id += 1;
        self.pending.insert(id, intent);
        id
    }

    /// Is the left button down, as far as nebula knows — a press came and
    /// its release has not: a panel splitter being dragged, a program in
    /// the pane holding the button, or a drag-selection under way? While
    /// it is, the host terminal is left exactly as it is (re-asking it for
    /// its modes mid-drag is a change under a gesture in progress), and a
    /// motion report with no button named is still the drag.
    pub fn mouse_held(&self) -> bool {
        self.term_mouse_grab.is_some() || self.term_selection.is_some_and(|s| s.dragging)
    }

    /// Is this worktree row a stand-in (a QUICK PROMPT's, or the NEW
    /// WORKTREE modal's) the DAEMON has not answered for yet? The
    /// in-flight intent is the one record of it —
    /// once the Ack or Error takes the intent, the row is real or gone.
    pub fn is_placeholder_worktree(&self, id: &WorktreeId) -> bool {
        self.pending
            .values()
            .any(|intent| intent.placeholder_worktree() == Some(id))
    }

    /// Is this session row a QUICK PROMPT stand-in the DAEMON has not
    /// answered for yet?
    pub fn is_placeholder_agent(&self, id: &AgentId) -> bool {
        self.pending
            .values()
            .any(|intent| intent.placeholder_agent() == Some(id))
    }

    pub fn is_placeholder_session(&self, sref: &SessionRef) -> bool {
        match sref {
            SessionRef::Agent(id) => self.is_placeholder_agent(id),
            SessionRef::Terminal(_) => false,
        }
    }

    /// The pane is showing a stand-in: there is no PTY behind it to type
    /// into, and nothing to attach.
    pub fn pane_shows_placeholder(&self) -> bool {
        self.term
            .as_ref()
            .is_some_and(|t| self.is_placeholder_session(&t.sref))
    }

    /// The mouse protocol the program in the pane has asked for, and
    /// whether it wants SGR coordinates. `None` when nothing there can take
    /// a report: no session, one whose process has exited (its last screen
    /// is still worth selecting from), a stand-in pane with no PTY behind
    /// it, or a pull request or a Cloud session's panel showing in the
    /// pane instead of a terminal.
    pub fn child_mouse_mode(&self) -> (vt100::MouseProtocolMode, bool) {
        let mouseless = (vt100::MouseProtocolMode::None, false);
        let Some(term) = &self.term else {
            return mouseless;
        };
        // Asked on every wheel notch over the pane: one reading of the
        // cursor for all four (`RowsMemo`).
        let reading_something_else = self.rows_memo.hold(|| {
            self.pane_shows_placeholder()
                || self.previewed_pr().is_some()
                || self.previewed_issue().is_some()
                || self.previewed_cloud().is_some()
        });
        if term.exited || reading_something_else {
            return mouseless;
        }
        let screen = term.parser.screen();
        (
            screen.mouse_protocol_mode(),
            screen.mouse_protocol_encoding() == vt100::MouseProtocolEncoding::Sgr,
        )
    }

    /// Projects panel rows in display order, each an index into
    /// `tree.projects` — every project gets one.
    ///
    /// Most recently interacted with first — the newest stamp under any of
    /// the project's worktrees, so the project you just worked in heads the
    /// column (mirrors the sessions list; there is no manual reorder). The
    /// sort is stable, so never-run projects keep tree order at the bottom
    /// instead of shuffling between frames.
    pub fn project_rows(&self) -> Vec<usize> {
        self.rows_memo.with(
            |m| &m.projects,
            self.rows_key(),
            || self.build_project_rows(),
            Vec::clone,
        )
    }

    /// What [`RowsMemo::with`] keys the kept rows on.
    fn rows_key(&self) -> RowsKey {
        RowsKey {
            cursor: (self.sel_project, self.sel_worktree, self.sel_session),
            shape: [
                self.tree.projects.len(),
                self.tree.worktrees.len(),
                self.tree.agents.len(),
                self.tree.terminals.len(),
                self.tree.links.len(),
            ],
            show_archived: self.show_archived,
        }
    }

    fn build_project_rows(&self) -> Vec<usize> {
        let now = now_ms();
        let mut rows: Vec<usize> = self
            .tree
            .projects
            .iter()
            .enumerate()
            .map(|(i, _)| i)
            .collect();
        // One pass for every project rather than one per comparison the
        // sort makes (`project_recencies`).
        let recencies = project_recencies(&self.tree, now);
        rows.sort_by_key(|i| {
            // The raw stamp breaks the tie every project with a session
            // mid-turn shares, so the project just launched into leads —
            // the sessions list's own rule (`recency_key`).
            let r = recencies
                .get(&self.tree.projects[*i].id)
                .copied()
                .unwrap_or_default();
            (
                std::cmp::Reverse(r.interacted),
                std::cmp::Reverse(r.stamped),
            )
        });
        rows
    }

    /// Index into `tree.projects` of the selected Projects-panel row.
    pub fn selected_project_index(&self) -> Option<usize> {
        self.project_rows().get(self.sel_project).copied()
    }

    /// The project giving the current selection its context.
    pub fn selected_project(&self) -> Option<&Project> {
        self.tree.projects.get(self.selected_project_index()?)
    }

    /// `project`'s own settings (Settings → Project): its entry in
    /// `projects_config`, else the fallback every project without one gets.
    pub fn project_settings(&self, project: &Project) -> &crate::config::ProjectSettings {
        self.projects_config
            .get(&project.repo_path)
            .unwrap_or(&self.project_fallback)
    }

    /// Whether `project`'s ROOT WORKTREE row is left out of the WORKTREES
    /// PANEL — its **Hide root worktree** project setting.
    pub fn root_hidden(&self, project: &Project) -> bool {
        self.project_settings(project).hide_root_worktree
    }

    /// `root_hidden` for the selected project; false while there is none.
    pub fn selected_root_hidden(&self) -> bool {
        self.selected_project().is_some_and(|p| self.root_hidden(p))
    }

    /// The checkout under the Worktrees cursor — a plain row or one nested
    /// under its pull request. None while the cursor is on a pull request.
    pub fn selected_worktree(&self) -> Option<&Worktree> {
        self.worktree_rows()
            .get(self.sel_worktree)
            .and_then(|row| row.checkout())
    }

    /// The cached changed-file count when it belongs to the selected
    /// worktree; `None` while unknown or the checkout is unreadable.
    pub fn selected_worktree_changes(&self) -> Option<usize> {
        let wt = self.selected_worktree()?;
        match &self.git_changes {
            Some((id, count)) if *id == wt.id => *count,
            _ => None,
        }
    }

    /// A checkout's last-read changed-file count, for its cards: None until
    /// one has been read there, or when git couldn't say.
    pub fn worktree_changes(&self, id: &WorktreeId) -> Option<usize> {
        self.worktree_changes.get(id).and_then(|(count, _)| *count)
    }

    /// A checkout's last-read line counts, for its cards: None while CARD
    /// LINE COUNTS is off, until one has been read there, or when nothing
    /// changed by the line.
    pub fn worktree_lines(&self, id: &WorktreeId) -> Option<crate::git_diff::LineChanges> {
        if !self.card_line_changes {
            return None;
        }
        self.worktree_lines.get(id).copied()
    }

    /// Does the cache describe a different worktree than the selection?
    /// The event loop refreshes before drawing when it does, so the badge
    /// never lags a j/k by a poll interval.
    pub fn git_changes_stale(&self) -> bool {
        self.git_changes.as_ref().map(|(id, _)| id) != self.selected_worktree().map(|w| &w.id)
    }

    /// Whether the daemon currently holds a live PTY for `sref`. Attaching
    /// to one only replays its ring; attaching to a session the IDLE REAPER
    /// took cold-spawns an agent CLI.
    pub fn session_is_live(&self, sref: &SessionRef) -> bool {
        match sref {
            SessionRef::Agent(id) => self.tree.agents.iter().any(|a| &a.id == id && a.alive),
            SessionRef::Terminal(id) => self.tree.terminals.iter().any(|t| &t.id == id && t.alive),
        }
    }

    /// Put a screen the pane is leaving aside for a quick return, when it
    /// is worth keeping: a real session that has painted and is still live
    /// (see [`App::term_cache`]). The cache is most-recent-first and
    /// bounded twice — [`TERM_CACHE_MAX`] screens, [`TERM_CACHE_CELLS`]
    /// between them; whatever it already held for this session is
    /// replaced. Over the cell budget, histories go before screens do,
    /// oldest first: a screen without its scrollback is a fiftieth of the
    /// size and still paints the return on the keypress.
    pub fn stash_term(&mut self, term: AttachedTerm) {
        self.term_cache.retain(|t| t.sref != term.sref);
        let keep = term.painted
            && !term.exited
            && !self.is_placeholder_session(&term.sref)
            && self.session_is_live(&term.sref);
        if !keep {
            return;
        }
        let mut term = term;
        // Left before the history it asked back had landed: still without
        // it, and no longer asking.
        if term.pending_scroll.take().is_some() {
            term.history_dropped = true;
        }
        self.term_cache.insert(0, term);
        self.term_cache.truncate(TERM_CACHE_MAX);
        self.prune_term_cache();
        let held = |cache: &[AttachedTerm]| -> usize {
            cache.iter().map(AttachedTerm::estimated_cells).sum()
        };
        while held(&self.term_cache) > TERM_CACHE_CELLS {
            let oldest_with_history = self
                .term_cache
                .iter()
                .rposition(|t| t.parser.screen().scrollback_rows() > 0);
            match oldest_with_history {
                Some(i) => self.term_cache[i].drop_history(),
                None => {
                    self.term_cache.pop();
                }
            }
        }
    }

    /// The kept screen for `sref`, if there is one and its session is
    /// still live — a session that died meanwhile comes back as a new
    /// process, and its old screen would only mislead for a frame.
    pub fn take_cached_term(&mut self, sref: &SessionRef) -> Option<AttachedTerm> {
        let i = self.term_cache.iter().position(|t| &t.sref == sref)?;
        let term = self.term_cache.remove(i);
        self.session_is_live(sref).then_some(term)
    }

    /// Drop kept screens whose session is gone or no longer live.
    pub fn prune_term_cache(&mut self) {
        let live: Vec<bool> = self
            .term_cache
            .iter()
            .map(|t| self.session_is_live(&t.sref))
            .collect();
        let mut i = 0;
        self.term_cache.retain(|_| {
            let keep = live[i];
            i += 1;
            keep
        });
    }

    /// The full row list the panel shows — `sel_session` indexes this.
    pub fn visible_session_rows(&self) -> Vec<SessionRow> {
        self.with_session_rows(<[SessionRow]>::to_vec)
    }

    /// `read` the rows [`App::visible_session_rows`] lists.
    fn with_session_rows<R>(&self, read: impl FnOnce(&[SessionRow]) -> R) -> R {
        self.rows_memo.hold(|| {
            self.rows_memo.with(
                |m| &m.sessions,
                self.rows_key(),
                || self.build_session_rows(),
                |rows| read(rows),
            )
        })
    }

    fn build_session_rows(&self) -> Vec<SessionRow> {
        // All four groups below hang off the SAME checkout, and each one
        // asking `selected_worktree` for it re-sorts every checkout of the
        // project — four sorts to build one list of rows. Asked once here
        // and handed down.
        let Some(wt) = self.selected_worktree() else {
            return vec![];
        };
        let wt = &wt.id;
        let agents = self.sessions_in(wt);
        let (active, _) = self.group_counts_in(wt);
        let active = active.min(agents.len());
        let mut rows: Vec<SessionRow> = agents[..active]
            .iter()
            .cloned()
            .map(SessionRow::Agent)
            .collect();
        rows.extend(self.terminals_in(wt).into_iter().map(SessionRow::Terminal));
        rows.extend(self.links_in(wt).into_iter().map(SessionRow::Link));
        rows.extend(agents[active..].iter().cloned().map(SessionRow::Agent));
        rows
    }

    /// The selected worktree's PULL REQUESTS group: the pull request on its branch
    /// first (however it got there), then any previously saved links in list
    /// order. New saved links are no longer exposed through the TUI.
    /// A saved link that *is* the pull request is shown once, as the
    /// pull-request row — a duplicate would just be the same destination
    /// twice.
    pub fn visible_links(&self) -> Vec<LinkRow> {
        let Some(wt) = self.selected_worktree() else {
            return vec![];
        };
        self.links_in(&wt.id)
    }

    /// The same for a checkout already in hand ([`App::sessions_in`]).
    fn links_in(&self, wt: &WorktreeId) -> Vec<LinkRow> {
        let saved: Vec<&Link> = self
            .tree
            .links
            .iter()
            .filter(|l| &l.worktree_id == wt)
            .collect();
        let pr = self.pull_requests.get(wt).cloned().flatten();
        let matched = pr
            .as_ref()
            .and_then(|p| saved.iter().position(|l| l.url == p.url));
        let mut rows: Vec<LinkRow> = Vec::new();
        match (&pr, matched) {
            (Some(pr), Some(i)) => rows.push(LinkRow::Saved {
                link: saved[i].clone(),
                pr: Some(pr.clone()),
            }),
            (Some(pr), None) => rows.push(LinkRow::PullRequest(pr.clone())),
            (None, _) => {}
        }
        for (i, link) in saved.iter().enumerate() {
            if Some(i) != matched {
                rows.push(LinkRow::Saved {
                    link: (*link).clone(),
                    pr: None,
                });
            }
        }
        rows
    }

    /// The link row under the cursor, when the cursor is on one.
    pub fn selected_link(&self) -> Option<LinkRow> {
        match self.selected_session_row() {
            Some(SessionRow::Link(l)) => Some(l),
            _ => None,
        }
    }

    pub fn selected_session_row(&self) -> Option<SessionRow> {
        self.with_session_rows(|rows| rows.get(self.sel_session).cloned())
    }

    /// The selected row's agent, when it is one (terminal rows return None).
    pub fn selected_session(&self) -> Option<Agent> {
        match self.selected_session_row() {
            Some(SessionRow::Agent(a)) => Some(a),
            _ => None,
        }
    }

    // ---- the FOLLOW-UP COMPOSER ----

    /// Can this row grow a FOLLOW-UP COMPOSER? An agent with a local PTY
    /// behind it, and only that: an ARCHIVED row's turn is over, a CLOUD
    /// row's agent is in a sandbox with a message queue of its own (its
    /// menu's **Send to cloud session**), a QUICK PROMPT stand-in has no
    /// session yet, and a TERMINAL or a PULL REQUEST row was never a
    /// conversation to follow up on.
    pub fn takes_follow_up(&self, row: &SessionRow) -> bool {
        matches!(
            row,
            SessionRow::Agent(a)
                if !a.archived
                    && a.cloud_session_id.is_none()
                    && !self.is_placeholder_agent(&a.id)
        )
    }

    /// The row the open FOLLOW-UP COMPOSER belongs to — an index into
    /// [`App::visible_session_rows`], found by AGENT so a re-sorted list
    /// keeps the box on its own card. None with nothing expanded, or when
    /// the agent it was opened on has left the list (archived, deleted, or
    /// the panel moved to another checkout).
    pub fn follow_up_row(&self) -> Option<usize> {
        let id = &self.follow_up.as_ref()?.agent;
        self.visible_session_rows()
            .iter()
            .position(|row| matches!(row, SessionRow::Agent(a) if &a.id == id))
    }

    /// Is the composer live — open, and on a card still in the list? What
    /// decides whether the SESSIONS PANEL's keys are the box's.
    pub fn follow_up_live(&self) -> bool {
        self.follow_up_row().is_some()
    }

    /// Shell terminals of the selected worktree, in tree order.
    pub fn visible_terminals(&self) -> Vec<TerminalTab> {
        let Some(wt) = self.selected_worktree() else {
            return vec![];
        };
        self.terminals_in(&wt.id)
    }

    /// The same for a checkout already in hand ([`App::sessions_in`]).
    fn terminals_in(&self, wt: &WorktreeId) -> Vec<TerminalTab> {
        self.tree
            .terminals
            .iter()
            .filter(|t| &t.worktree_id == wt)
            .cloned()
            .collect()
    }

    /// First free `prefix-N` name within the selected worktree. A QUICK
    /// PROMPT stand-in counts as taken: its create is in flight and will
    /// land under that name (the create it stands for takes the name off
    /// the stand-in itself — see `create_agent`).
    pub fn default_session_name(&self, prefix: &str) -> String {
        let taken: Vec<String> = self
            .visible_sessions()
            .iter()
            .map(|a| a.name.clone())
            .collect();
        let mut n = 1;
        loop {
            let candidate = format!("{prefix}-{n}");
            if !taken.contains(&candidate) {
                return candidate;
            }
            n += 1;
        }
    }

    /// Worktrees of the selected project: the root checkout first, always,
    /// then the rest most recently interacted with first (mirrors the
    /// sessions list). The stamp is the newest of the checkout's sessions;
    /// a stable sort keeps never-run worktrees in tree order at the bottom.
    pub fn visible_worktrees(&self) -> Vec<&Worktree> {
        self.rows_memo
            .with(
                |m| &m.worktrees,
                self.rows_key(),
                || self.build_visible_worktrees(),
                Vec::clone,
            )
            .into_iter()
            .map(|i| &self.tree.worktrees[i])
            .collect()
    }

    /// [`App::visible_worktrees`], as indices into `tree.worktrees`.
    fn build_visible_worktrees(&self) -> Vec<usize> {
        let Some(project) = self.selected_project() else {
            return vec![];
        };
        let now = now_ms();
        // With the project's **Hide root worktree** on, the ROOT WORKTREE
        // is not a row: it is still in the tree (its sessions keep running,
        // the PALETTE still knows them), it just cannot be selected here,
        // so nothing launched from this panel ever lands in the shared
        // checkout.
        let hide_root = self.root_hidden(project);
        let worktrees = &self.tree.worktrees;
        let mut rows: Vec<usize> = (0..worktrees.len())
            .filter(|&i| {
                let w = &worktrees[i];
                w.project_id == project.id && !(hide_root && w.is_main)
            })
            .collect();
        // Rolled up once for every checkout rather than re-walked per
        // comparison the sort makes (`worktree_recencies`).
        let recencies = worktree_recencies(&self.tree, now);
        rows.sort_by_key(|&i| {
            let w = &worktrees[i];
            // The raw stamp breaks the tie every checkout with a session
            // mid-turn shares — see `recency_key`.
            let r = recencies.get(&w.id).copied().unwrap_or_default();
            (
                std::cmp::Reverse(w.is_main),
                std::cmp::Reverse(r.interacted),
                std::cmp::Reverse(r.stamped),
            )
        });
        rows
    }

    /// Every branch `project` already has a checkout for — hidden ROOT
    /// WORKTREE included — so a generated branch name never collides.
    pub fn project_branches(&self, project: &ProjectId) -> Vec<String> {
        self.tree
            .worktrees
            .iter()
            .filter(|w| &w.project_id == project)
            .map(|w| w.branch.clone())
            .collect()
    }

    /// The selected project's open pull requests, every one `gh pr list`
    /// answered with — drafts included whatever `hide_draft_prs` says, and
    /// whether or not the group under the checkouts is showing them. The
    /// list the fetch cap (`pull_request::LIST_LIMIT`) is measured
    /// against. Empty until the first answer (or when the repo genuinely
    /// has none).
    pub fn all_open_prs(&self) -> &[OpenPr] {
        self.selected_project()
            .and_then(|p| self.open_prs.get(&p.id))
            .map(|o| o.list.as_slice())
            .unwrap_or_default()
    }

    /// The open pull requests the PROJECT OPEN PRS GROUP lists once it is
    /// open: the whole answer, or — with `hide_draft_prs` on — only the
    /// rows asking for a reviewer. What its header counts while it is
    /// folded. A view over [`App::all_open_prs`], filtered on every read
    /// rather than once on arrival, so switching the setting off shows
    /// every draft at once with no refetch, and a draft marked ready on
    /// GitHub joins the rows on the refresh that says so.
    pub fn listed_open_prs(&self) -> Vec<&OpenPr> {
        self.all_open_prs()
            .iter()
            .filter(|pr| !(self.hide_draft_prs && pr.is_draft))
            .collect()
    }

    /// How many drafts `hide_draft_prs` is keeping out of the group right
    /// now — what its header owns up to (`9/12`), so a pull request that
    /// is not where it was reads as a setting, not a loss. Zero while
    /// drafts show.
    pub fn hidden_draft_prs(&self) -> usize {
        if !self.hide_draft_prs {
            return 0;
        }
        self.all_open_prs().iter().filter(|pr| pr.is_draft).count()
    }

    /// The open pull requests with rows under the checkouts: the listed
    /// ones, or none while the group is folded — a folded group has no
    /// rows for the cursor to walk into, the way a collapsed ARCHIVED
    /// group has none.
    pub fn visible_open_prs(&self) -> Vec<&OpenPr> {
        if self.open_prs_collapsed {
            return Vec::new();
        }
        self.listed_open_prs()
    }

    /// The selected project's open issues, as `gh issue list` last
    /// answered (`issues::request_list`): the ISSUES MODAL's rows, and
    /// what the PROJECT ISSUES GROUP under the pull requests lists and its
    /// header counts while it is folded. Empty until the first answer, or
    /// when the repo has none.
    pub fn listed_issues(&self) -> &[crate::issues::Issue] {
        self.selected_project()
            .and_then(|p| self.issues.get(&p.id))
            .map(|l| l.list.as_slice())
            .unwrap_or_default()
    }

    /// The issues with rows under the pull requests: the listed ones, or
    /// none while the group is folded — a folded group has no rows for
    /// the cursor to walk into, like the folded OPEN PRS group.
    pub fn visible_issues(&self) -> &[crate::issues::Issue] {
        if self.issues_collapsed {
            return &[];
        }
        self.listed_issues()
    }

    /// The Worktrees panel's rows in cursor order — what `sel_worktree`
    /// indexes: the project's checkouts, then the pull requests still open
    /// on its repo, each followed by the checkout on its head branch when
    /// the project has one. A PR SESSION works in a checkout of the pull
    /// request's head branch (`CreatePrAgent` cuts it, or reuses the one
    /// already there), and that checkout listed among the plain ones left
    /// no way to tell which pull request it was for — so it sits under
    /// the pull request's row instead, indented, and the plain rows above
    /// are the checkouts that are nobody's PR. The ROOT WORKTREE never
    /// nests, whatever branch it is on; a branch two open pull requests
    /// share nests under the first listed. Only a pull request *on
    /// screen* takes its checkout: with the group folded, or the pull
    /// request a draft `hide_draft_prs` keeps out, the checkout is a plain
    /// row again — hiding pull requests must never hide work you have.
    ///
    /// A cursor parked on a pull request has no selected worktree, which
    /// is the truth about it; one on a nested checkout has that worktree
    /// and no pull request — the row is a worktree, its pull request is
    /// the row above.
    ///
    /// The issues open on the repo close the list (`visible_issues`): one
    /// row each under the pull requests, none while their group is
    /// folded. An issue nests nothing — it has no branch to check out.
    pub fn worktree_rows(&self) -> Vec<WorktreeRow<'_>> {
        // The checkouts, the pull requests and the issues each ask for
        // the selected project: one sort of the projects between them.
        self.rows_memo.hold(|| self.build_worktree_rows())
    }

    fn build_worktree_rows(&self) -> Vec<WorktreeRow<'_>> {
        let checkouts = self.visible_worktrees();
        let prs = self.visible_open_prs();
        // Which listed pull request each checkout nests under, if any.
        let under: Vec<Option<usize>> = checkouts
            .iter()
            .map(|w| {
                if w.is_main {
                    return None;
                }
                prs.iter().position(|pr| pr.head == w.branch)
            })
            .collect();
        let mut rows: Vec<WorktreeRow<'_>> = checkouts
            .iter()
            .zip(&under)
            .filter(|(_, under)| under.is_none())
            .map(|(w, _)| WorktreeRow::Checkout(w))
            .collect();
        for (i, pr) in prs.iter().enumerate() {
            rows.push(WorktreeRow::Pr(pr));
            rows.extend(
                checkouts
                    .iter()
                    .zip(&under)
                    .filter(|(_, under)| **under == Some(i))
                    .map(|(w, _)| WorktreeRow::PrCheckout { worktree: w, pr }),
            );
        }
        rows.extend(self.visible_issues().iter().map(WorktreeRow::Issue));
        rows
    }

    /// How many rows the Worktrees panel has — see [`App::worktree_rows`].
    pub fn worktree_row_count(&self) -> usize {
        self.worktree_rows().len()
    }

    /// The Worktrees row of checkout `id`, wherever it sits — among the
    /// plain checkouts or under its pull request. None when it is not a
    /// row of the selected project (or is the hidden ROOT WORKTREE).
    pub fn worktree_row_of(&self, id: &WorktreeId) -> Option<usize> {
        self.worktree_rows()
            .iter()
            .position(|row| row.checkout().is_some_and(|w| &w.id == id))
    }

    /// The Worktrees row of the open pull request at `url`: its own row,
    /// not the checkout nested under it. None while the group is folded
    /// or the pull request is not listed.
    pub fn open_pr_row_of(&self, url: &str) -> Option<usize> {
        self.worktree_rows()
            .iter()
            .position(|row| row.open_pr().is_some_and(|pr| pr.url == url))
    }

    /// The Worktrees row of the open issue at `url`, while the ISSUES
    /// group is open and lists it.
    pub fn issue_row_of(&self, url: &str) -> Option<usize> {
        self.worktree_rows()
            .iter()
            .position(|row| row.open_issue().is_some_and(|i| i.url == url))
    }

    /// Rows a half-page jump (Ctrl+d / Ctrl+u) moves the Worktrees
    /// cursor: half of what the column showed room for on the last
    /// frame, never less than one so the keys still move before the
    /// first draw or in a column squeezed down to a row or two.
    pub fn worktrees_half_page(&self) -> usize {
        (self.worktrees_view_rows / 2).max(1)
    }

    /// Rows a half-page jump moves the Sessions cursor: the same rule as
    /// [`App::worktrees_half_page`], on the Sessions column's last frame.
    pub fn sessions_half_page(&self) -> usize {
        (self.sessions_view_rows / 2).max(1)
    }

    /// The open pull request under the Worktrees cursor, when it's on one.
    /// Mutually exclusive with [`App::selected_worktree`]: a pull request
    /// row has no checkout, and a checkout nested under a pull request is
    /// still a checkout, not the pull request.
    pub fn selected_worktree_pr(&self) -> Option<&OpenPr> {
        self.worktree_rows()
            .get(self.sel_worktree)
            .and_then(|row| row.open_pr())
    }

    /// The issue under the Worktrees cursor — a PROJECT ISSUES GROUP row.
    /// None on a checkout or a pull request.
    pub fn selected_worktree_issue(&self) -> Option<&crate::issues::Issue> {
        self.worktree_rows()
            .get(self.sel_worktree)
            .and_then(|row| row.open_issue())
    }

    /// The pull request the pane should be reading: the PROJECT OPEN PRS
    /// GROUP row under the Worktrees cursor, or — while the SESSIONS PANEL
    /// has focus — the PR ROW under its cursor (a saved LINK that *is* the
    /// branch's pull request counts; a bare URL does not).
    ///
    /// The Sessions half is keyed on focus on purpose: a session is still
    /// attached behind that pane, and stepping into it (`l`, a click) has
    /// to bring the terminal back so what you type is what you see. The
    /// Worktrees half never needs that — a PR row there has no checkout, so
    /// there is nothing behind the pane to return to.
    pub fn previewed_pr(&self) -> Option<PreviewedPr> {
        if let Some(pr) = self.selected_worktree_pr() {
            return Some(PreviewedPr {
                number: pr.number,
                url: pr.url.clone(),
                label: pr.label(),
            });
        }
        if self.focus != Focus::Sessions {
            return None;
        }
        let row = self.selected_link()?;
        let pr = row.pull_request()?;
        Some(PreviewedPr {
            number: pr.number,
            url: pr.url.clone(),
            label: row.label(),
        })
    }

    /// The issue the pane should be reading: the PROJECT ISSUES GROUP row
    /// under the Worktrees cursor. `draw_terminal` asks for the pull
    /// request first; the two never share a cursor.
    pub fn previewed_issue(&self) -> Option<&crate::issues::Issue> {
        self.selected_worktree_issue()
    }

    /// What the pane is reading, by URL — the pull request or the issue
    /// under a cursor — so the loop can tell a turn that changed it
    /// (`note_preview_change`) from one that left the reader in place.
    pub fn reading_url(&self) -> Option<String> {
        // Twice a turn of the event loop, and three walks to the same
        // cursor each time (`RowsMemo`).
        self.rows_memo.hold(|| {
            self.previewed_pr()
                .map(|pr| pr.url)
                .or_else(|| self.previewed_issue().map(|i| i.url.clone()))
        })
    }

    /// The Claude Cloud row the pane should be describing: the SESSIONS
    /// PANEL's cursor on an agent that launched a cloud session. The agent
    /// runs in the cloud sandbox, so the pane shows the CLOUD SESSION
    /// PANEL — where it is, and the link to it — instead of a terminal.
    /// Unlike the PR ROW this is not keyed on focus: there is no terminal
    /// behind the panel to step back into, so wherever focus goes the pane
    /// keeps pointing at the session. A pull request under the Worktrees
    /// cursor still wins (`draw_terminal` asks for it first).
    pub fn previewed_cloud(&self) -> Option<CloudPreview> {
        let SessionRow::Agent(agent) = self.selected_session_row()? else {
            return None;
        };
        let url = agent.cloud_session_url()?;
        Some(CloudPreview {
            id: agent.id,
            name: agent.name,
            cloud_session_id: agent.cloud_session_id.unwrap_or_default(),
            url,
        })
    }

    /// Session rows for the selected worktree: the live agents, then (when
    /// shown) the archived ones.
    ///
    /// The live rows are ordered by last interaction, newest first, so the
    /// session you just ran surfaces at the top and the list reads as a
    /// history of what you have been doing. Working sessions count as
    /// interacting now, which keeps them on top however long the turn has
    /// taken.
    pub fn visible_sessions(&self) -> Vec<Agent> {
        let Some(wt) = self.selected_worktree() else {
            return vec![];
        };
        self.sessions_in(&wt.id)
    }

    /// The same for a checkout the caller has already found, so the rows
    /// builder does not pay for `selected_worktree` to find it again
    /// ([`App::visible_session_rows`]).
    fn sessions_in(&self, wt: &WorktreeId) -> Vec<Agent> {
        let now = now_ms();
        // Stable throughout, so ties — never-run rows especially, which all
        // stamp 0 — keep tree order instead of shuffling between frames.
        let mut rows: Vec<Agent> = self
            .tree
            .agents
            .iter()
            .filter(|a| &a.worktree_id == wt && !a.archived)
            .cloned()
            .collect();
        rows.sort_by_key(|a| recency_key(a, now));
        // A stable pass over the top of it: the session just launched
        // leads the list from the moment its row arrives, rather than
        // sitting under the working ones until its own turn starts.
        if let Some(id) = &self.just_launched {
            rows.sort_by_key(|a| &a.id != id);
        }
        if self.show_archived {
            let mut archived: Vec<Agent> = self
                .tree
                .agents
                .iter()
                .filter(|a| &a.worktree_id == wt && a.archived)
                .cloned()
                .collect();
            // Most recently archived first; pre-`archived_at` rows (stamp 0)
            // keep tree order at the bottom (stable sort).
            archived.sort_by_key(|a| std::cmp::Reverse(a.archived_at));
            rows.extend(archived);
        }
        rows
    }

    /// (live, archived) agent counts for the selected worktree.
    pub fn session_group_counts(&self) -> (usize, usize) {
        let Some(wt) = self.selected_worktree() else {
            return (0, 0);
        };
        self.group_counts_in(&wt.id)
    }

    /// The same for a checkout already in hand ([`App::sessions_in`]).
    fn group_counts_in(&self, wt: &WorktreeId) -> (usize, usize) {
        let live = self
            .tree
            .agents
            .iter()
            .filter(|a| &a.worktree_id == wt && !a.archived)
            .count();
        let archived = self
            .tree
            .agents
            .iter()
            .filter(|a| &a.worktree_id == wt && a.archived)
            .count();
        (live, archived)
    }

    /// Delay until the pending worktree-sessions prewarm is due, so the
    /// event loop can wake up and fire it. None when nothing is armed.
    pub fn prewarm_delay(&self) -> Option<std::time::Duration> {
        let (_, at) = self.pending_prewarm.as_ref()?;
        Some(at.saturating_duration_since(std::time::Instant::now()))
    }

    /// How long until the debounced attach should be sent, if one is armed.
    pub fn attach_delay(&self) -> Option<std::time::Duration> {
        let (_, at) = self.pending_attach.as_ref()?;
        Some(at.saturating_duration_since(std::time::Instant::now()))
    }

    /// Whether `gh` should be asked about this worktree now: not while an
    /// answer is in flight, and not before the timer the last answer armed.
    /// A found PR no longer retires the worktree — the PR doesn't change,
    /// but its conversation does, and the unread badge is only as fresh as
    /// the last poll.
    pub fn pr_lookup_due(&self, worktree: &WorktreeId) -> bool {
        if self.pr_inflight.contains(worktree) {
            return false;
        }
        match self.pr_recheck.get(worktree) {
            Some((due, _)) => std::time::Instant::now() >= *due,
            None => true,
        }
    }

    /// Whether `gh pr list` should be run for this project now: not while
    /// an answer is in flight, and not before the timer the last answer
    /// armed. A project nebula has never asked about is always due.
    pub fn open_prs_lookup_due(&self, project: &ProjectId) -> bool {
        if self.open_prs_inflight.contains(project) {
            return false;
        }
        match self.open_prs.get(project) {
            Some(open) => std::time::Instant::now() >= open.due,
            None => true,
        }
    }

    /// Furthest the pull-request preview may scroll: its last line pinned
    /// to the bottom of the pane. Zero while the preview fits, so a wheel
    /// flick on a short PR does nothing instead of scrolling it off screen.
    pub fn pr_preview_max_scroll(&self) -> u16 {
        (self.pr_preview_lines as u16).saturating_sub(self.term_area.height.max(1))
    }

    /// Delay until the debounced pull-request detail fetch is due. None when
    /// the cursor isn't resting on a pull request that still needs one.
    pub fn pr_detail_delay(&self) -> Option<std::time::Duration> {
        let (_, at) = self.pending_pr_detail.as_ref()?;
        Some(at.saturating_duration_since(std::time::Instant::now()))
    }

    /// The same for the ISSUES MODAL's debounced comments fetch.
    pub fn issue_detail_delay(&self) -> Option<std::time::Duration> {
        let (_, at) = self.pending_issue_detail.as_ref()?;
        Some(at.saturating_duration_since(std::time::Instant::now()))
    }

    /// Delay until the debounced issues prefetch is due, so the loop can
    /// wake up and fire it. None when nothing is armed.
    pub fn issues_prefetch_delay(&self) -> Option<std::time::Duration> {
        let (_, at) = self.pending_issues_prefetch.as_ref()?;
        Some(at.saturating_duration_since(std::time::Instant::now()))
    }

    /// The body and conversation behind the pull request the pane is
    /// reading: `Some(Some(_))` once fetched, `Some(None)` while it's still
    /// coming (or came back empty), `None` when the pane isn't reading one.
    pub fn selected_pr_detail(&self) -> Option<Option<&PrDetail>> {
        let pr = self.previewed_pr()?;
        Some(self.pr_detail.get(&pr.url))
    }

    /// Every pull request still on some row, by URL: each project's open
    /// list, and each checkout's own PR ROW whatever its state — a merged
    /// pull request stays on that row (see `pull_request::PullRequest`).
    /// What the per-URL caches — bodies in memory, diffs on disk — are
    /// pruned to.
    pub fn live_pr_urls(&self) -> std::collections::HashSet<String> {
        self.open_prs
            .values()
            .flat_map(|o| o.list.iter().map(|pr| pr.url.clone()))
            .chain(
                self.pull_requests
                    .values()
                    .flatten()
                    .map(|pr| pr.url.clone()),
            )
            .collect()
    }

    /// Delay until the standing keep-warm re-send is due. None when disarmed.
    pub fn keepwarm_delay(&self) -> Option<std::time::Duration> {
        let at = self.next_keepwarm.as_ref()?;
        Some(at.saturating_duration_since(std::time::Instant::now()))
    }

    /// PR & ISSUE COUNTS for a project row, while the switch is on: how
    /// many open pull requests its OPEN PRS GROUP lists (drafts left out
    /// with `hide_draft_prs`, so the number is the group header's), and
    /// how many issues are open on the repo. Each is `None` until its list
    /// has landed — a repo nobody has asked about yet says nothing rather
    /// than `0` — and both are `None` with the switch off.
    pub fn project_open_counts(&self, project_id: &ProjectId) -> (Option<usize>, Option<usize>) {
        if !self.pr_issue_counts {
            return (None, None);
        }
        let prs = self.open_prs.get(project_id).map(|open| {
            open.list
                .iter()
                .filter(|pr| !(self.hide_draft_prs && pr.is_draft))
                .count()
        });
        let issues = self.issues.get(project_id).map(|l| l.list.len());
        (prs, issues)
    }

    /// Aggregate status for a worktree row: red > yellow > green > gray,
    /// archived agents excluded.
    pub fn worktree_rollup(&self, worktree_id: &WorktreeId) -> Option<AgentStatus> {
        worktree_rollup(&self.tree, worktree_id)
    }

    pub fn project_rollup(&self, project_id: &ProjectId) -> Option<AgentStatus> {
        project_rollup(&self.tree, project_id)
    }

    /// Whether the checkout's row wears its pull request's merge instead of
    /// its sessions' status. The PR ROW keeps a merged pull request
    /// (`pull_requests` holds it, state and all), and a checkout whose
    /// branch has landed is the one to archive or delete — so its row says
    /// so in purple, from across the room. A live session still wins: a
    /// running or asking agent is exactly the thing not to delete a
    /// checkout out from under, and its yellow or red is the warning.
    pub fn worktree_wears_merge(&self, worktree_id: &WorktreeId) -> bool {
        let merged = self
            .pull_requests
            .get(worktree_id)
            .and_then(Option::as_ref)
            .is_some_and(|pr| pr.standing() == crate::pull_request::Standing::Merged);
        merged
            && !matches!(
                self.worktree_rollup(worktree_id),
                Some(AgentStatus::Running | AgentStatus::NeedsFeedback)
            )
    }

    /// The worktree's RUN TERMINAL — the terminal `r` started there, still
    /// running or exited on its own — when it has one.
    pub fn run_terminal(&self, worktree_id: &WorktreeId) -> Option<&TerminalTab> {
        self.tree
            .terminals
            .iter()
            .find(|t| &t.worktree_id == worktree_id && t.run_command.is_some())
    }

    /// Whether the worktree's RUN COMMAND is up right now: the RUNNING
    /// badge its row wears, and what `r` stops.
    pub fn worktree_running(&self, worktree_id: &WorktreeId) -> bool {
        self.run_terminal(worktree_id).is_some_and(|t| t.alive)
    }

    /// When the worktree last saw a turn — what its row sorts and labels on.
    pub fn worktree_recency(&self, worktree_id: &WorktreeId) -> Recency {
        worktree_recency(&self.tree, worktree_id, now_ms())
    }

    pub fn project_recency(&self, project_id: &ProjectId) -> Recency {
        project_recency(&self.tree, project_id, now_ms())
    }

    /// Sessions under a worktree that went green with nobody looking.
    pub fn worktree_unseen(&self, worktree_id: &WorktreeId) -> usize {
        worktree_unseen(&self.tree, worktree_id)
    }

    pub fn project_unseen(&self, project_id: &ProjectId) -> usize {
        project_unseen(&self.tree, project_id)
    }

    /// Where a walk out of the pane lands: the GRID, which is the only
    /// thing beside it.
    pub fn first_sidebar_focus(&self) -> Focus {
        Focus::Sessions
    }

    /// The two places FOCUS can rest: the LAUNCHER VIEW's GRID of cards
    /// and the PANE under them. The three columns the other variants name
    /// are no longer drawn.
    pub fn focus_visible(&self, focus: Focus) -> bool {
        matches!(focus, Focus::Sessions | Focus::Terminal)
    }

    fn focus_rank(focus: Focus) -> u8 {
        match focus {
            Focus::Projects => 1,
            Focus::Worktrees => 2,
            Focus::Sessions => 3,
            Focus::Terminal => 4,
        }
    }

    pub fn next_visible_focus(&self, focus: Focus) -> Focus {
        let rank = Self::focus_rank(focus);
        [
            Focus::Projects,
            Focus::Worktrees,
            Focus::Sessions,
            Focus::Terminal,
        ]
        .into_iter()
        .find(|candidate| Self::focus_rank(*candidate) > rank && self.focus_visible(*candidate))
        .unwrap_or(focus)
    }

    pub fn previous_visible_focus(&self, focus: Focus) -> Focus {
        let rank = Self::focus_rank(focus);
        [
            Focus::Terminal,
            Focus::Sessions,
            Focus::Worktrees,
            Focus::Projects,
        ]
        .into_iter()
        .find(|candidate| Self::focus_rank(*candidate) < rank && self.focus_visible(*candidate))
        .unwrap_or(focus)
    }

    /// First stop in the Tab walk, and where a jump into another project
    /// lands: the GRID.
    pub fn first_focus(&self) -> Focus {
        Focus::Sessions
    }

    /// Where `target` was drawn on the last frame — the rect the hit was
    /// registered with. A dropdown hangs off the word it belongs to, so
    /// it needs the word's own cell and not the pointer's.
    pub fn hit_rect(&self, target: &HitTarget) -> Option<Rect> {
        self.hits
            .iter()
            .find(|(_, t)| t == target)
            .map(|(rect, _)| *rect)
    }

    pub fn hit_at(&self, x: u16, y: u16) -> Option<HitTarget> {
        self.hits
            .iter()
            .find(|(rect, _)| {
                x >= rect.x && x < rect.x + rect.width && y >= rect.y && y < rect.y + rect.height
            })
            .map(|(_, t)| t.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- the ROWS MEMO ----

    /// Two projects: `api` with its root `w0` and a `feat` checkout `w1`,
    /// `web` with its root `w2`; one session in each checkout, stamped in
    /// that order so `web` is the project used most recently.
    fn a_memo_tree() -> App {
        let mut app = App::new();
        app.tree.projects = ["api", "web"]
            .iter()
            .enumerate()
            .map(|(i, name)| Project {
                id: ProjectId(format!("p{i}")),
                name: (*name).into(),
                repo_path: format!("/tmp/{name}").into(),
                sort_order: 0,
            })
            .collect();
        app.tree.worktrees = [("w0", "p0", true), ("w1", "p0", false), ("w2", "p1", true)]
            .iter()
            .map(|(id, project, is_main)| Worktree {
                id: WorktreeId((*id).into()),
                project_id: ProjectId((*project).into()),
                path: format!("/tmp/{id}").into(),
                branch: if *is_main {
                    "main".into()
                } else {
                    "feat".into()
                },
                is_main: *is_main,
                sort_order: 0,
            })
            .collect();
        app.tree.agents = (0..3)
            .map(|i| Agent {
                id: AgentId(format!("a{i}")),
                worktree_id: WorktreeId(format!("w{i}")),
                name: format!("s{i}"),
                status: AgentStatus::Finished,
                archived: false,
                archived_at: 0,
                unseen: false,
                kind: AgentKind::Claude,
                custom_harness: None,
                model: None,
                effort: None,
                session_id: None,
                cloud_session_id: None,
                sort_order: 0,
                status_changed_at: 1_000 * (i as i64 + 1),
                alive: true,
                recent_prompts: Vec::new(),
            })
            .collect();
        app
    }

    fn project_names(app: &App) -> Vec<String> {
        app.project_rows()
            .iter()
            .map(|&i| app.tree.projects[i].name.clone())
            .collect()
    }

    /// Outside a stretch nothing is kept: a handler that changes the tree
    /// and then asks gets the answer for the tree it just changed.
    #[test]
    fn outside_a_stretch_every_answer_is_fresh() {
        let mut app = a_memo_tree();
        assert_eq!(project_names(&app), ["web", "api"]);
        app.tree.agents[0].status_changed_at = 9_000;
        assert_eq!(project_names(&app), ["api", "web"], "api just saw a turn");
    }

    /// A stretch keeps its rows only for the cursor and the tree's shape
    /// they were built for: moving a cursor or adding a row inside one
    /// is answered anew, never from the rows kept before it.
    #[test]
    fn a_stretch_rebuilds_for_a_moved_cursor_or_a_new_row() {
        let mut app = a_memo_tree();
        app.sel_project = 1; // `api`, behind `web`
        app.rows_memo.arm();
        let ids = |app: &App| -> Vec<String> {
            app.visible_worktrees()
                .iter()
                .map(|w| w.id.0.clone())
                .collect()
        };
        assert_eq!(ids(&app), ["w0", "w1"]);
        let session = |app: &App| match app.selected_session_row() {
            Some(SessionRow::Agent(a)) => Some(a.id.0),
            _ => None,
        };
        assert_eq!(session(&app).as_deref(), Some("a0"));
        app.sel_worktree = 1;
        assert_eq!(
            session(&app).as_deref(),
            Some("a1"),
            "the cursor moved to `feat` inside the stretch: its session, not the root's"
        );
        app.tree.worktrees.push(Worktree {
            id: WorktreeId("w3".into()),
            project_id: ProjectId("p0".into()),
            path: "/tmp/w3".into(),
            branch: "fix".into(),
            is_main: false,
            sort_order: 0,
        });
        assert_eq!(
            ids(&app),
            ["w0", "w1", "w3"],
            "a checkout arrived inside it"
        );
        app.rows_memo.disarm();
    }

    /// A frame is a stretch that ends with the frame: once it is drawn,
    /// the next question is answered fresh.
    #[test]
    fn a_frame_keeps_nothing_past_itself() {
        let mut app = a_memo_tree();
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(120, 40)).unwrap();
        terminal.draw(|f| crate::ui::draw(f, &mut app)).unwrap();
        assert_eq!(project_names(&app), ["web", "api"]);
        app.tree.agents[0].status_changed_at = 9_000;
        assert_eq!(project_names(&app), ["api", "web"]);
    }

    // ---- the pane's screen ----

    /// A screen that watched its process exit is rebuilt by whatever replay
    /// comes next, even one numbered as a continuation: the only bytes
    /// that can follow an exit are the next process's, from a ring of its
    /// own.
    #[test]
    fn an_exited_screen_is_rebuilt_by_the_next_replay() {
        let mut term = AttachedTerm::new(SessionRef::Agent(AgentId("a".into())), 20, 4);
        assert!(
            !term.apply_scrollback(0, b"old"),
            "a first replay onto an empty parser continues it"
        );
        term.exited = true;
        assert!(
            term.apply_scrollback(3, b"new"),
            "after an exit the screen starts over"
        );
        assert!(!term.exited, "the exit mark clears with it");
        let contents = term.parser.screen().contents();
        assert!(
            contents.contains("new") && !contents.contains("old"),
            "{contents:?}"
        );
        assert_eq!(term.next_seq, 6);
    }

    /// The "starting session…" notice is the daemon's empty replay, not the
    /// wait for the replay: unknown before it lands, booting after an empty
    /// one, gone with the first byte.
    #[test]
    fn an_empty_replay_means_booting_until_bytes_arrive() {
        let mut term = AttachedTerm::new(SessionRef::Agent(AgentId("a".into())), 20, 4);
        assert!(!term.booting, "before the daemon answers, nothing is known");
        term.apply_scrollback(0, b"");
        assert!(term.booting && !term.painted, "an empty replay: booting");
        term.apply_output(0, b"hi");
        assert!(term.painted && !term.booting, "the first bytes end it");
        assert_eq!(term.next_seq, 2);
    }

    // ---- shared list arithmetic ----

    #[test]
    fn window_start_slides_only_to_keep_the_cursor_visible() {
        assert_eq!(window_start(0, 5), 0);
        assert_eq!(window_start(4, 5), 0, "last row still fits");
        assert_eq!(window_start(5, 5), 1);
        assert_eq!(window_start(12, 5), 8);
        assert_eq!(window_start(3, 0), 4, "a zero-height list still computes");
    }

    #[test]
    fn clamp_selection_pins_to_the_list() {
        assert_eq!(clamp_selection(3, 0), 0, "empty list");
        assert_eq!(clamp_selection(-2, 4), 0);
        assert_eq!(clamp_selection(2, 4), 2);
        assert_eq!(clamp_selection(9, 4), 3);
    }

    #[test]
    fn max_scroll_and_scrolled_by_pin_the_pane() {
        assert_eq!(max_scroll(10, 4), 6);
        assert_eq!(max_scroll(3, 4), 0, "everything fits");
        assert_eq!(max_scroll(3, 0), 2, "a zero-height pane counts as one row");
        assert_eq!(scrolled_by(2, 3, 6), 5);
        assert_eq!(scrolled_by(2, 10, 6), 6);
        assert_eq!(scrolled_by(2, -10, 6), 0);
    }

    #[test]
    fn clamp_files_width_honors_both_minimums() {
        let area = Rect::new(10, 0, 100, 20);
        // Boundary at column 50 → 40 columns of list.
        assert_eq!(clamp_files_width(area, 50), Some(40));
        assert_eq!(
            clamp_files_width(area, 0),
            Some(MIN_DIFF_FILES_W),
            "left of the modal"
        );
        assert_eq!(
            clamp_files_width(area, 200),
            Some(100 - MIN_DIFF_PANE_W),
            "the right pane keeps its minimum"
        );
        let tiny = Rect::new(0, 0, MIN_DIFF_FILES_W + MIN_DIFF_PANE_W - 1, 20);
        assert_eq!(clamp_files_width(tiny, 5), None, "too small to honor both");
    }

    // ---- worktree links ----

    fn link_app() -> (App, WorktreeId) {
        let mut app = App::new();
        let project_id = ProjectId("p1".into());
        let worktree_id = WorktreeId("w1".into());
        app.tree.projects.push(Project {
            id: project_id.clone(),
            name: "demo".into(),
            repo_path: "/tmp/demo".into(),
            sort_order: 0,
        });
        app.tree.worktrees.push(Worktree {
            id: worktree_id.clone(),
            project_id,
            path: "/tmp/demo".into(),
            branch: "main".into(),
            is_main: true,
            sort_order: 0,
        });
        (app, worktree_id)
    }

    fn link(id: &str, worktree: &WorktreeId, url: &str, sort_order: i64) -> Link {
        Link {
            id: LinkId(id.into()),
            worktree_id: worktree.clone(),
            url: url.into(),
            sort_order,
        }
    }

    fn pr(url: &str) -> PullRequest {
        PullRequest {
            number: 7,
            url: url.into(),
            title: "Attach links".into(),
            state: crate::pull_request::STATE_OPEN.into(),
            is_draft: false,
            health: Default::default(),
            activity: Vec::new(),
        }
    }

    #[test]
    fn saved_links_list_in_order_under_the_pull_request() {
        let (mut app, wt) = link_app();
        app.tree
            .links
            .push(link("l1", &wt, "https://a.dev/spec", 0));
        app.tree
            .links
            .push(link("l2", &wt, "https://b.dev/issue", 1));
        // Another worktree's links never leak into this list.
        app.tree
            .links
            .push(link("l3", &WorktreeId("other".into()), "https://c.dev", 0));

        let rows = app.visible_links();
        let urls: Vec<&str> = rows.iter().map(|l| l.url()).collect();
        assert_eq!(urls, ["https://a.dev/spec", "https://b.dev/issue"]);

        app.pull_requests
            .insert(wt, Some(pr("https://github.com/o/r/pull/7")));
        let rows = app.visible_links();
        assert_eq!(
            rows[0].url(),
            "https://github.com/o/r/pull/7",
            "the pull request leads the list"
        );
        assert_eq!(rows[0].label(), "#7 Attach links");
        assert!(rows[0].id().is_none(), "it is not a stored row");
        assert_eq!(rows.len(), 3);
    }

    /// A link the user pasted before nebula found the PR is the same
    /// destination: it shows once, as the pull-request row, and stays
    /// deletable because it is still the user's own row.
    #[test]
    fn a_saved_link_matching_the_pull_request_is_shown_once() {
        let (mut app, wt) = link_app();
        let url = "https://github.com/o/r/pull/7";
        app.tree
            .links
            .push(link("l1", &wt, "https://a.dev/spec", 0));
        app.tree.links.push(link("l2", &wt, url, 1));
        app.pull_requests.insert(wt, Some(pr(url)));

        let rows = app.visible_links();
        assert_eq!(rows.len(), 2, "no duplicate row for the same URL");
        assert_eq!(rows[0].url(), url);
        assert_eq!(rows[0].label(), "#7 Attach links", "shown as the PR");
        assert_eq!(
            rows[0].id().map(|id| id.as_str()),
            Some("l2"),
            "still the stored row, so it can be edited and deleted"
        );
        assert_eq!(rows[1].url(), "https://a.dev/spec");
    }

    #[test]
    fn links_sit_between_terminals_and_archived_sessions() {
        let (mut app, wt) = link_app();
        app.tree.agents.push(Agent {
            id: AgentId("a1".into()),
            worktree_id: wt.clone(),
            name: "live".into(),
            status: AgentStatus::Fresh,
            archived: false,
            archived_at: 0,
            unseen: false,
            status_changed_at: 0,
            kind: AgentKind::Claude,
            custom_harness: None,
            model: None,
            effort: None,
            session_id: None,
            cloud_session_id: None,
            sort_order: 0,
            alive: true,
            recent_prompts: Vec::new(),
        });
        app.tree.agents.push(Agent {
            id: AgentId("a2".into()),
            name: "old".into(),
            archived: true,
            ..app.tree.agents[0].clone()
        });
        app.tree.terminals.push(TerminalTab {
            id: TerminalId("t1".into()),
            worktree_id: wt.clone(),
            name: "shell".into(),
            sort_order: 0,
            alive: true,
            run_command: None,
        });
        app.tree
            .links
            .push(link("l1", &wt, "https://a.dev/spec", 0));
        app.show_archived = true;

        let rows = app.visible_session_rows();
        let names: Vec<&str> = rows.iter().map(|r| r.name()).collect();
        assert_eq!(names, ["live", "shell", "https://a.dev/spec", "old"]);
    }

    #[test]
    fn link_rows_have_no_session_to_attach() {
        let (mut app, wt) = link_app();
        app.tree
            .links
            .push(link("l1", &wt, "https://a.dev/spec", 0));
        let rows = app.visible_session_rows();
        assert!(rows[0].sref().is_none(), "a link is not attachable");
        assert!(!rows[0].is_archived_agent());
        assert_eq!(
            rows[0].click_key(),
            RowKey::Link("https://a.dev/spec".into())
        );
    }

    #[test]
    fn pretty_url_strips_the_parts_nobody_reads() {
        assert_eq!(pretty_url("https://www.example.com/a/b"), "example.com/a/b");
        assert_eq!(pretty_url("http://x.dev/"), "x.dev");
        assert_eq!(pretty_url("https://x.dev"), "x.dev");
        // Not a URL shape we produce, but the function must not panic.
        assert_eq!(pretty_url(""), "");
    }

    /// Codex (ratatui inline viewport) inserts chat history by scrolling a
    /// TOP-ANCHORED DECSTBM region, which stock vt100 discards instead of
    /// saving — leaving nothing to scroll back to. This exercises the
    /// vendored vt100 patch through the real dependency, so it also fails if
    /// the `[patch.crates-io]` wiring is ever dropped.
    #[test]
    fn top_anchored_region_scroll_lands_in_scrollback() {
        let sref = SessionRef::Agent(AgentId::from("test-agent".to_string()));
        let mut term = AttachedTerm::new(sref, 80, 24);

        // Codex-style history insert: region rows 1..=10 (viewport below),
        // cursor at region bottom, newlines scroll history off the top.
        term.parser.process(b"\x1b[1;10r\x1b[10;1H");
        for i in 0..20 {
            term.parser
                .process(format!("history line {i}\r\n").as_bytes());
        }
        term.parser.process(b"\x1b[r");

        term.set_scroll(5);
        assert_eq!(
            term.parser.screen().scrollback(),
            5,
            "rows scrolled out of a top-anchored region must be recallable"
        );
        let top_row = term.parser.screen().contents();
        let top_row = top_row.lines().next().unwrap_or("");
        assert!(
            top_row.starts_with("history line"),
            "scrolled-back view should show an evicted history line, got {top_row:?}"
        );
    }

    /// The alternate screen (vim, htop) has no scrollback buffer, so region
    /// scrolls there must stay discarded even with the vendored patch.
    #[test]
    fn alternate_screen_region_scroll_stays_unscrollable() {
        let sref = SessionRef::Agent(AgentId::from("test-agent".to_string()));
        let mut term = AttachedTerm::new(sref, 80, 24);

        term.parser.process(b"\x1b[?1049h\x1b[1;10r\x1b[10;1H");
        for i in 0..20 {
            term.parser.process(format!("alt line {i}\r\n").as_bytes());
        }

        term.set_scroll(5);
        assert_eq!(
            term.parser.screen().scrollback(),
            0,
            "alternate screen must not accumulate scrollback"
        );
    }

    #[test]
    fn toggle_reviewed_sinks_marks_and_moves_the_selection() {
        let files = ["a", "b", "c"]
            .map(|p| DiffFile {
                path: p.into(),
                orig_path: None,
                xy: ['M', ' '],
            })
            .to_vec();
        let mut v = DiffView::new("/nonexistent-review".into(), "main".into(), files, true);
        let order = |v: &DiffView| -> Vec<String> {
            v.matches
                .iter()
                .map(|m| v.files[m.file].path.clone())
                .collect()
        };

        // Mark the middle file: it sinks and the next file takes its row.
        v.select(1);
        assert_eq!(v.toggle_reviewed(), Some(true), "moved on to c");
        assert_eq!(order(&v), ["a", "c", "b"]);
        assert_eq!(v.selected_file().unwrap().path, "c");

        // Mark c too: the reviewed zone keeps git order and the selection
        // row lands in it (nothing unreviewed is left below c).
        assert_eq!(v.toggle_reviewed(), Some(true));
        assert_eq!(order(&v), ["a", "b", "c"]);
        assert_eq!(v.selected_file().unwrap().path, "b");

        // Unmark b: it pops back to its natural spot but the selection
        // advances to the next still-marked file (c), so repeated presses
        // clear a batch of marks.
        assert_eq!(v.toggle_reviewed(), Some(true), "advanced to c");
        assert_eq!(order(&v), ["a", "b", "c"]);
        assert_eq!(v.selected_file().unwrap().path, "c");
        assert_eq!(v.reviewed.len(), 1, "only c is still marked");

        // Unmark c — the last mark: nothing left to batch through, so the
        // selection follows the file back to its natural spot — same file,
        // no diff reload.
        assert_eq!(v.toggle_reviewed(), Some(false), "c stays selected");
        assert!(v.reviewed.is_empty());
        assert_eq!(order(&v), ["a", "b", "c"]);
        assert_eq!(v.selected_file().unwrap().path, "c");

        // With every other file reviewed, marking keeps the file selected —
        // there is nowhere further to advance.
        assert_eq!(v.toggle_reviewed(), Some(false), "c stays selected");
        v.select(1);
        assert_eq!(v.toggle_reviewed(), Some(false), "b stays selected");
        v.select(0);
        assert_eq!(v.toggle_reviewed(), Some(false), "a stays selected");
        assert_eq!(order(&v), ["a", "b", "c"]);
        assert_eq!(v.reviewed.len(), 3);

        // Batch unmark from the top of the reviewed zone: each press clears
        // the selected mark and lands on the next one down.
        assert_eq!(v.toggle_reviewed(), Some(true), "a cleared, on to b");
        assert_eq!(v.selected_file().unwrap().path, "b");
        assert_eq!(v.toggle_reviewed(), Some(true), "b cleared, on to c");
        assert_eq!(v.selected_file().unwrap().path, "c");
        assert_eq!(v.toggle_reviewed(), Some(false), "last mark, c stays");
        assert!(v.reviewed.is_empty());
        assert_eq!(order(&v), ["a", "b", "c"]);

        // No visible row (dead-end filter): toggling is a no-op.
        v.filter = "zzz".into();
        v.apply_filter();
        assert_eq!(v.toggle_reviewed(), None);
    }

    // ---- the recency rollups ----

    /// The one-pass rollups must answer exactly what the per-row functions
    /// answer, for every checkout and every project — including the ones
    /// with no sessions under them at all, which the maps have no entry for
    /// and the sorts read as a default stamp.
    ///
    /// They exist only to stop the row sorts re-walking every session per
    /// COMPARISON (`worktree_recencies`); the moment they disagree with
    /// `worktree_recency` the launcher silently reorders itself.
    #[test]
    fn the_one_pass_rollups_agree_with_the_per_row_stamps() {
        use nebula_core::AgentKind;

        let now = 10_000;
        let mut tree = Tree::default();
        for p in 0..3 {
            tree.projects.push(Project {
                id: ProjectId(format!("p{p}")),
                name: format!("p{p}"),
                repo_path: format!("/tmp/p{p}").into(),
                sort_order: p,
            });
            for w in 0..3 {
                tree.worktrees.push(Worktree {
                    id: WorktreeId(format!("p{p}w{w}")),
                    project_id: ProjectId(format!("p{p}")),
                    path: format!("/tmp/p{p}w{w}").into(),
                    branch: if w == 0 {
                        "main".into()
                    } else {
                        format!("b{w}")
                    },
                    is_main: w == 0,
                    sort_order: w,
                });
            }
        }
        // Sessions on some checkouts and not others, live turns among them
        // (which count as interacting NOW, the tie the raw stamp breaks).
        for (i, wt) in ["p0w0", "p0w0", "p0w1", "p1w2", "p2w0"].iter().enumerate() {
            tree.agents.push(Agent {
                id: AgentId(format!("a{i}")),
                worktree_id: WorktreeId((*wt).into()),
                name: format!("a{i}"),
                status: if i % 2 == 0 {
                    AgentStatus::Finished
                } else {
                    AgentStatus::Running
                },
                archived: i == 4,
                archived_at: 0,
                unseen: false,
                kind: AgentKind::Claude,
                custom_harness: None,
                model: None,
                effort: None,
                session_id: None,
                cloud_session_id: None,
                sort_order: 0,
                status_changed_at: 100 * (i as i64 + 1),
                alive: true,
                recent_prompts: Vec::new(),
            });
        }

        let by_worktree = worktree_recencies(&tree, now);
        for w in &tree.worktrees {
            assert_eq!(
                by_worktree.get(&w.id).copied().unwrap_or_default(),
                worktree_recency(&tree, &w.id, now),
                "checkout {:?}",
                w.id
            );
        }

        let by_project = project_recencies(&tree, now);
        for p in &tree.projects {
            assert_eq!(
                by_project.get(&p.id).copied().unwrap_or_default(),
                project_recency(&tree, &p.id, now),
                "project {:?}",
                p.id
            );
        }
    }
}
