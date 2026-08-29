//! TUI state: the Elm-ish Model.

use crate::git_diff::DiffFile;
use crate::pull_request::{OpenPr, PrDetail, PullRequest};
use crate::text_input::TextInput;
use nebula_core::{
    Agent, AgentId, AgentKind, AgentStatus, Link, LinkId, Project, ProjectId, SessionRef,
    TerminalId, TerminalTab, Workspace, WorkspaceId, Worktree, WorktreeId,
};
use ratatui::layout::Rect;
use std::collections::HashMap;
use std::path::PathBuf;

/// Frame duration of the status-sweep text animation: the event loop's
/// repaint cadence while [`App::status_anim_active`] holds, and the step
/// size of [`App::sweep_phase`] (one text cell per frame).
pub const SWEEP_FRAME: std::time::Duration = std::time::Duration::from_millis(100);

/// Wall-clock epoch ms, comparable to the daemon's `status_changed_at`.
pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    /// The optional top bar (`Shift+W` shows/hides it). Its cursor
    /// IS the open workspace — moving it switches, the way moving in the
    /// Projects column re-scopes the worktrees.
    Workspaces,
    Projects,
    Worktrees,
    Sessions,
    Terminal,
}

/// What a screen cell maps to; rebuilt on every draw for hit-testing.
#[derive(Debug, Clone, PartialEq)]
pub enum HitTarget {
    /// Row index into `tree.workspaces`; a click opens that workspace.
    Workspace(usize),
    /// The `◇ workspace` nameplate on the footer; a click opens the
    /// workspace switcher.
    FooterWorkspace,
    /// Row index into `App::project_rows()`.
    Project(usize),
    Worktree(usize),
    Session(usize),
    /// The ARCHIVED group header (either form); a click toggles the group
    /// open/closed, same as the A key.
    ArchivedHeader,
    /// Panel background (registered after rows, so rows win).
    PanelBg(Focus),
    TerminalPane,
    /// Draggable right boundary of a visible sidebar panel. The index is
    /// logical (0 Projects, 1 Worktrees, 2 Sessions), so hidden panels keep
    /// their remembered widths without owning a boundary.
    Splitter(usize),
}

/// Default widths of the Projects / Worktrees / Sessions panels. Sessions
/// is the widest because its rows carry the most: name, "23m ago", harness.
pub const DEFAULT_PANEL_WIDTHS: [u16; 3] = [20, 22, 32];
/// Height of the Workspaces bar that spans the top of the body: a blank
/// spacer, the label-plus-tabs row, a second blank spacer, and the rule
/// that closes it off from the panels below. The label lands on the same
/// row-1 / x-3 grid the panel headers use, and the tabs get a row of air on
/// either side so the bar reads as its own tier rather than as a header
/// crowded against the rule.
pub const WORKSPACES_BAR_H: u16 = 4;
/// A panel can't be dragged narrower than this.
pub const MIN_PANEL_W: u16 = 10;
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
    /// Re-enter the Claude Cloud session a row launched (see
    /// `ClientRequest::AttachCloudAgent`).
    AttachCloudAgent(AgentId),
    /// Queue a message on the row's Claude Cloud session
    /// (`ClientRequest::SendCloudMessage`), via a prompt.
    SendCloudMessage(AgentId),
    RenameAgent(AgentId),
    ArchiveAgent(AgentId),
    UnarchiveAgent(AgentId),
    DeleteAgent(AgentId),
    NewAgent(WorktreeId),
    /// Picker result: create an agent of this kind (chains into the name
    /// prompt). `model`/`effort` are submenu choices: None means the row
    /// hasn't drilled into that submenu (its configured default applies);
    /// "default" is the submenu row that picks the default explicitly.
    NewAgentOfKind {
        worktree: WorktreeId,
        kind: AgentKind,
        model: Option<String>,
        effort: Option<String>,
        /// One-shot launch modifier for Claude. The task itself is collected
        /// after the optional name prompt and crosses IPC only on create.
        cloud: bool,
        /// OPEN PRS launch context. Some is valid only for local Claude and
        /// is preserved through model/effort submenus and the name prompt.
        pr_url: Option<String>,
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
    EditLink(LinkId),
    DeleteLink(LinkId),
    DeleteWorktree(WorktreeId),
    AddProject,
    RemoveProject(ProjectId),
    /// Retitle a project's row. Display only — the folder keeps its name and
    /// stays visible under the new one.
    RenameProject(ProjectId),
    /// Workspace-switcher row: open this workspace. The switcher's other
    /// verbs are keys, not rows — n: new, r: rename, d: delete (footer
    /// hints).
    OpenWorkspace(WorkspaceId),
    /// The Workspaces column's menu rows: the switcher's n / r / d verbs,
    /// for a mouse that never learned the keys.
    NewWorkspace,
    RenameWorkspace(WorkspaceId),
    RemoveWorkspace(WorkspaceId),
    ToggleArchived,
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
                model,
                effort,
                ..
            } => {
                if crate::config::model_choices(*kind).is_empty() {
                    return None;
                }
                match (model, effort) {
                    (None, None) => Some(SubmenuKind::Models),
                    (Some(m), None)
                        if !crate::config::effort_choices(*kind, Some(m)).is_empty() =>
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

    /// Is this the `w` workspace switcher? Its rows are all OpenWorkspace,
    /// which gates the switcher-only keys (n/r/d) and its footer hint.
    pub fn is_workspace_picker(&self) -> bool {
        self.items
            .iter()
            .any(|i| matches!(i.action, MenuAction::OpenWorkspace(_)))
    }

    /// The workspace under the switcher's cursor, if this is the switcher.
    pub fn hovered_workspace(&self) -> Option<WorkspaceId> {
        match &self.items.get(self.hover)?.action {
            MenuAction::OpenWorkspace(id) => Some(id.clone()),
            _ => None,
        }
    }

    /// Cloud mode is a root new-session-picker modifier, not another agent
    /// kind. Returning Some only while the Claude row itself is highlighted
    /// keeps Tab free everywhere else (including model/effort submenus).
    pub fn hovered_claude_cloud(&self) -> Option<bool> {
        if self.parent.is_some() || self.title.as_deref() != Some("New session") {
            return None;
        }
        match &self.items.get(self.hover)?.action {
            MenuAction::NewAgentOfKind {
                kind: AgentKind::Claude,
                cloud,
                ..
            } => Some(*cloud),
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
    /// `d` on a workspace — the column, its menu, or the `w` switcher.
    /// `reopen_picker` is the switcher's hover row when the confirm came
    /// from there: both answers put the switcher back, hover intact, so
    /// the EntityRemoved delta drops the row in place as before.
    RemoveWorkspace {
        id: WorkspaceId,
        reopen_picker: Option<usize>,
    },
    DeleteLink(LinkId),
    /// `d` in the AGENT PRESETS list: drop the preset at `index` from the
    /// store. Both answers reopen the list for `worktree`, so the modal the
    /// confirm evicted comes back where the user left it.
    DeleteAgentPreset {
        index: usize,
        worktree: WorktreeId,
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
    NewAgent {
        worktree: WorktreeId,
        kind: AgentKind,
        /// Resolved launch options (picker choice or configured default);
        /// None = the CLI's own default.
        model: Option<String>,
        effort: Option<String>,
        cloud: bool,
        pr_url: Option<String>,
    },
    /// Final task input for a one-shot `claude --cloud <task>` launch. Kept
    /// separate from the name prompt so Enter still submits names normally,
    /// while Shift+Enter can insert task newlines here.
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
    /// A message to queue on a row's Claude Cloud session
    /// (`claude -p <message> --cloud <id>`). Multi-row like the launch task:
    /// steering a cloud agent is rarely one line.
    CloudMessage {
        id: AgentId,
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
    /// Name for a workspace created from the switcher; opened on Ack.
    NewWorkspace,
    RenameWorkspace {
        id: WorkspaceId,
    },
    /// Rewrite a pinned link's URL.
    EditLink {
        id: LinkId,
    },
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
        };
        prompt.refresh_dirs();
        prompt
    }

    /// Does Tab complete filesystem paths in this prompt?
    pub fn completes_paths(&self) -> bool {
        matches!(self.kind, PromptKind::AddProject)
    }

    /// The task prompts — the Claude Cloud launch task, a message to a live
    /// cloud session, and an AGENT PRESET's task — are the ones with a
    /// multi-row editor.
    pub fn is_multiline(&self) -> bool {
        matches!(
            self.kind,
            PromptKind::ClaudeCloudTask { .. }
                | PromptKind::CloudMessage { .. }
                | PromptKind::AgentPresetTask { .. }
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
    /// Reviewed ✓ marks: file path → fingerprint of the approved diff text.
    /// Nebula-side bookkeeping only (persisted via `review::store_marks`);
    /// never stages or otherwise touches git state.
    pub reviewed: HashMap<String, u64>,
    /// HEAD OID the marks are scoped to (empty on an unborn HEAD). A moved
    /// HEAD — commit, checkout — resets the worktree's marks on next open.
    pub head_key: String,
}

impl DiffView {
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
            reviewed: HashMap::new(),
            head_key: String::new(),
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

    /// Clamped absolute selection in the filtered list; true when it changed
    /// (the caller reloads the diff).
    pub fn select(&mut self, index: i64) -> bool {
        let clamped = clamp_selection(index, self.matches.len());
        let changed = clamped != self.selected;
        self.selected = clamped;
        changed
    }

    /// The file behind the current selection, if any row is visible.
    pub fn selected_file(&self) -> Option<&DiffFile> {
        self.files.get(self.matches.get(self.selected)?.file)
    }

    /// First visible row of the file list's stateless follow-window for a
    /// list of `height` rows.
    pub fn window_start(&self, height: usize) -> usize {
        window_start(self.selected, height)
    }

    /// Recompute `matches` from `filter` and reset the selection to the top
    /// row; true when the selected file changed (the caller reloads the
    /// diff).
    pub fn apply_filter(&mut self) -> bool {
        let before = self.matches.get(self.selected).map(|m| m.file);
        self.recompute_matches();
        self.selected = 0;
        before != self.matches.first().map(|m| m.file)
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
    pub fn toggle_reviewed(&mut self) -> Option<bool> {
        let path = self.selected_file()?.path.clone();
        let before = self.matches.get(self.selected).map(|m| m.file);
        let unmarked = self.reviewed.remove(&path).is_some();
        if !unmarked {
            let mark = crate::review::fingerprint(&self.diff);
            self.reviewed.insert(path.clone(), mark);
        }
        self.recompute_matches();
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
        };
        finder.apply_filter();
        finder
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
        }
    }

    /// Re-run the grep for the current query and reset the selection to the
    /// top row. Queries under `MIN_QUERY_LEN` just clear the results.
    pub fn run_search(&mut self) {
        self.selected = 0;
        self.error = None;
        if self.query.chars().count() < crate::grep_search::MIN_QUERY_LEN {
            self.hits.clear();
            self.truncated = false;
            return;
        }
        match crate::grep_search::search(&self.root, &self.query) {
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
    Metrics(MetricsView),
    Hosts(HostsView),
    /// `e` in the SESSIONS PANEL: the AGENT PRESETS list.
    AgentPresets(crate::preset_overlays::AgentPresetsView),
    /// The PRESET EDITOR form behind the list's `a` / `e`.
    AgentPresetEditor(crate::preset_overlays::AgentPresetEditor),
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

/// What to do when an Ack (or Error) for this req_id arrives.
#[derive(Debug, Clone)]
pub enum PendingIntent {
    /// Attach the created session and focus the terminal.
    AttachCreated,
    /// Attach on success; on failure, reopen the exact Cloud task so a
    /// transient daemon/CLI error never makes the user retype it.
    AttachCreatedWithCloudRetry {
        kind: PromptKind,
        task: String,
    },
    /// Flash `note` on success; on failure, reopen this prompt with `text`
    /// restored. Same bargain as the Cloud task: a message worth typing into
    /// a multi-row editor is worth not losing to a transient error.
    ReopenPromptOnError {
        kind: PromptKind,
        text: String,
        note: String,
    },
    /// Select the added project and step into its Worktrees panel.
    SelectCreatedProject,
    /// Select the created worktree in the Worktrees panel.
    SelectCreatedWorktree,
    /// Open the workspace this Ack just created (switcher's "New workspace…"
    /// flow: creating from there means you want to be in it).
    OpenCreatedWorkspace,
    /// Worktree removed optimistically; restore these rows on Error.
    DeleteWorktree(WorktreeRollback),
    None,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnState {
    Connected,
    Disconnected,
}

/// One row of the Sessions panel's OPEN PRS group: a previously saved URL,
/// or the pull request nebula found on the worktree's branch.
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

/// What a click landed on, for the double-click window. Sessions are their
/// own reference; a link has none (the pull-request row isn't even stored),
/// so its URL is the identity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RowKey {
    Session(SessionRef),
    Link(String),
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

/// One status for a whole workspace: every live agent under every project
/// in it, folded the same way a project folds its worktrees. This is what
/// lets the Workspaces column say "something is running over there"
/// without opening it — background workspaces keep receiving status
/// deltas, so the reading is live.
pub fn workspace_rollup(tree: &Tree, workspace_id: &WorkspaceId) -> Option<AgentStatus> {
    rollup(workspace_agents(tree, workspace_id).map(|a| a.status))
}

/// The `n done` count for a workspace tab: sessions under it that finished
/// a turn nobody has read yet. Same thing [`worktree_unseen`] and
/// [`project_unseen`] count, one tier up — a count means the same at every
/// tier, and it counts down to nothing as the cursor visits each session.
pub fn workspace_unseen(tree: &Tree, workspace_id: &WorkspaceId) -> usize {
    workspace_agents(tree, workspace_id)
        .filter(|a| a.unseen)
        .count()
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

/// Every unarchived agent under every project in a workspace.
fn workspace_agents<'a>(
    tree: &'a Tree,
    workspace_id: &WorkspaceId,
) -> impl Iterator<Item = &'a Agent> + 'a {
    let project_ids: Vec<&ProjectId> = tree
        .projects
        .iter()
        .filter(|p| &p.workspace_id == workspace_id)
        .map(|p| &p.id)
        .collect();
    let wt_ids: Vec<WorktreeId> = tree
        .worktrees
        .iter()
        .filter(|w| project_ids.contains(&&w.project_id))
        .map(|w| w.id.clone())
        .collect();
    tree.agents
        .iter()
        .filter(move |a| !a.archived && wt_ids.contains(&a.worktree_id))
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
/// group in tree order.
fn recency_key(a: &Agent, now: i64) -> std::cmp::Reverse<i64> {
    std::cmp::Reverse(last_interaction_ms(a, now))
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

/// The same over every project of a workspace — what lets the `/` PALETTE
/// put the workspace you were just in back within reach.
pub fn workspace_recency(tree: &Tree, workspace_id: &WorkspaceId, now: i64) -> Recency {
    tree.projects
        .iter()
        .filter(|p| &p.workspace_id == workspace_id)
        .map(|p| project_recency(tree, &p.id, now))
        .fold(Recency::default(), |r, p| Recency {
            interacted: r.interacted.max(p.interacted),
            stamped: r.stamped.max(p.stamped),
        })
}

/// Priority-ordered aggregate: needs-feedback > running > finished > fresh.
fn rollup(statuses: impl Iterator<Item = AgentStatus>) -> Option<AgentStatus> {
    let mut best: Option<AgentStatus> = None;
    fn rank(s: AgentStatus) -> u8 {
        match s {
            AgentStatus::NeedsFeedback => 4,
            AgentStatus::Running => 3,
            AgentStatus::Finished => 2,
            AgentStatus::Terminated | AgentStatus::Disconnected => 1,
            AgentStatus::Fresh => 0,
        }
    }
    for s in statuses {
        best = Some(match best {
            Some(b) if rank(b) >= rank(s) => b,
            _ => s,
        });
    }
    best
}

/// Client-side mirror of the entity tree. `projects` holds EVERY workspace's
/// projects; the panels scope to `active_workspace` (see
/// [`App::project_rows`]), so a workspace switch is a pure re-filter — no
/// refetch, and background workspaces keep receiving status updates. The
/// `/` palette deliberately doesn't scope: it searches the whole tree (see
/// [`crate::palette::Palette`]), which is the same data either way.
#[derive(Debug, Clone, Default)]
pub struct Tree {
    pub workspaces: Vec<Workspace>,
    /// The workspace THIS instance is showing. Client-local: the daemon
    /// hands over the last-opened one at boot, and after that every nebula
    /// window scopes itself — switching here moves no one else.
    pub active_workspace: WorkspaceId,
    pub projects: Vec<Project>,
    pub worktrees: Vec<Worktree>,
    pub agents: Vec<Agent>,
    pub terminals: Vec<TerminalTab>,
    pub links: Vec<Link>,
}

impl Tree {
    /// Is this project in the open workspace (i.e. visible)?
    pub fn in_active_workspace(&self, p: &Project) -> bool {
        p.workspace_id == self.active_workspace
    }

    /// Row of the open workspace in `workspaces` — the Workspaces column's
    /// cursor. None only before the first snapshot lands.
    pub fn active_workspace_index(&self) -> Option<usize> {
        self.workspaces
            .iter()
            .position(|w| w.id == self.active_workspace)
    }

    /// Display name of the open workspace, for the footer and switcher.
    pub fn active_workspace_name(&self) -> &str {
        self.workspaces
            .iter()
            .find(|w| w.id == self.active_workspace)
            .map(|w| w.name.as_str())
            .unwrap_or("default")
    }

    /// Is the open workspace the built-in `default` one? The first-run
    /// splash is scoped to it: an empty workspace the user created (or
    /// stepped onto in the Workspaces column) is an ordinary empty tree,
    /// not a first run.
    pub fn in_default_workspace(&self) -> bool {
        self.active_workspace.as_str() == nebula_core::DEFAULT_WORKSPACE_ID
    }

    /// Any project visible in the open workspace? (The splash and the
    /// empty-panel hints key off this, not the raw project list — other
    /// workspaces' projects don't count.)
    pub fn has_visible_projects(&self) -> bool {
        self.projects.iter().any(|p| self.in_active_workspace(p))
    }

    /// Visible-project count for the PROJECTS panel header.
    pub fn visible_project_count(&self) -> usize {
        self.projects
            .iter()
            .filter(|p| self.in_active_workspace(p))
            .count()
    }
}

pub struct AttachedTerm {
    pub sref: SessionRef,
    pub parser: vt100::Parser,
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
}

impl AttachedTerm {
    pub fn new(sref: SessionRef, cols: u16, rows: u16) -> Self {
        Self {
            sref,
            parser: vt100::Parser::new(rows, cols, 10_000),
            exited: false,
            cols,
            rows,
            scroll: 0,
            kitty_flags: 0,
            painted: false,
        }
    }

    /// Reset the parser (fresh replay is about to arrive).
    pub fn reset(&mut self) {
        self.parser = vt100::Parser::new(self.rows, self.cols, 10_000);
        self.exited = false;
        self.scroll = 0;
        self.painted = false;
    }

    pub fn set_scroll(&mut self, scroll: usize) {
        self.scroll = scroll;
        self.parser.screen_mut().set_scrollback(scroll);
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
    /// Panel widths (projects, worktrees, sessions); absent in older blobs.
    #[serde(default)]
    pub panel_widths: Option<[u16; 3]>,
    /// Diff modal file-list width; absent in older blobs.
    #[serde(default)]
    pub diff_files_width: Option<u16>,
}

/// A mouse selection over the terminal pane (drag or double-click word), in
/// pane-relative cell coordinates `(col, row)` with inclusive endpoints.
/// Nebula owns the mouse (the emulator's native shift+drag never reaches us
/// reliably — Terminal.app has no such bypass at all), so selection is
/// implemented app-side and copied to the system clipboard when it completes.
/// The highlight persists after mouse-up; it's cleared by the next click,
/// scrolling, typing into the PTY, or a resize/reattach (anything that moves
/// the content under it — the selection is in screen coordinates).
#[derive(Debug, Clone, Copy)]
pub struct TermSelection {
    pub anchor: (u16, u16),
    pub head: (u16, u16),
    /// Still being dragged (button down). Cleared on mouse-up.
    pub dragging: bool,
    /// A real selection, not just an armed click. Set once a drag leaves its
    /// starting cell (and kept if it returns), or immediately for a
    /// double-click word selection — which may be a single cell, so
    /// `anchor == head` can't be the "just a click" test.
    pub active: bool,
}

impl TermSelection {
    /// Endpoints normalized to row-major order: (start, end).
    pub fn bounds(&self) -> ((u16, u16), (u16, u16)) {
        let anchor_key = (self.anchor.1, self.anchor.0);
        let head_key = (self.head.1, self.head.0);
        if anchor_key <= head_key {
            (self.anchor, self.head)
        } else {
            (self.head, self.anchor)
        }
    }
}

/// An in-progress drag of a panel splitter.
#[derive(Debug, Clone, Copy)]
pub struct SplitterDrag {
    /// Which boundary (see `HitTarget::Splitter`).
    pub idx: usize,
    /// `boundary_x - grab column` at mouse-down, so the boundary tracks the
    /// cursor without jumping a cell depending on which border cell was
    /// grabbed.
    pub grab_offset: i32,
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
}

impl PointerShape {
    /// The shape's name inside the OSC 22 escape.
    pub fn osc_name(self) -> &'static str {
        match self {
            PointerShape::Default => "default",
            PointerShape::ColResize => "col-resize",
        }
    }
}

/// What `gh pr list` last said about one project's open pull requests, and
/// the timer deciding when to ask again. Held per project rather than
/// refetched per repaint because every answer is a `gh` process and a
/// GitHub API call, and the list changes on the order of minutes.
#[derive(Debug, Clone)]
pub struct OpenPrs {
    /// Open pull requests, in `gh`'s order (newest first).
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

/// How recently a project's open-PR list may have been fetched and still be
/// refetched on arrival — at a project, at a sidebar panel, or back at the
/// terminal window. Walking the project list, or a flurry of focus events,
/// must not turn into one API call per gesture; a few seconds is enough to
/// coalesce those while still beating the steady beat by a wide margin.
pub const OPEN_PRS_MIN_AGE: std::time::Duration = std::time::Duration::from_secs(5);

pub struct App {
    pub tree: Tree,
    pub focus: Focus,
    /// Selected row in the Projects panel — indexes `project_rows()`, the
    /// open workspace's projects in display order.
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
    /// `(sel_worktree, sel_session)` as of the last draw — the draw
    /// re-anchors `sessions_scroll` only when this changes.
    pub sessions_anchor: Option<(usize, usize)>,
    /// First visible row of the Worktrees panel, in panel rows. Same
    /// contract as `sessions_scroll`: the wheel moves it freely, the draw
    /// clamps it and re-anchors on the cursor when `worktrees_anchor` shows
    /// the selection moved. A project with a long open-PR list routinely
    /// outgrows the column.
    pub worktrees_scroll: usize,
    /// `(sel_project, sel_worktree)` as of the last draw.
    pub worktrees_anchor: Option<(usize, usize)>,
    pub term: Option<AttachedTerm>,
    /// Input lock: keys forward to the attached PTY. Focusing the terminal
    /// pane alone (Tab / arrows) does NOT lock — Enter, a click, or `z` does.
    pub term_locked: bool,
    pub conn: ConnState,
    pub hits: Vec<(Rect, HitTarget)>,
    /// Inner rect of the terminal pane from the last draw.
    pub term_area: Rect,
    pub dirty: bool,
    pub should_quit: bool,
    /// Set with `should_quit` when the hosts picker chose a destination:
    /// after teardown the binary execs `nebula ssh` at it, replacing this
    /// process with a fresh connection.
    pub pending_ssh: Option<crate::hosts::HostEntry>,
    pub flash: Option<String>,
    /// The last `h`/`l` (or ←/→) that landed on the end of the panel row,
    /// or `k`/`j` (↑/↓) on a panel's first row / in the Workspaces bar, and
    /// stayed put, with when it arrived: a second press of the same action
    /// inside `DOUBLE_TAP` jumps the boundary the way ⇧Tab / Tab would.
    /// Any other key in between clears it.
    pub edge_tap: Option<(crate::keymap::Action, std::time::Instant)>,
    /// The panel focus came up from when it last stepped into the
    /// Workspaces bar — by k,k, h,h, ⇧Tab or a click — so j,j in the bar
    /// drops back onto it. Projects until the bar has been entered.
    pub bar_return: Focus,
    pub overlay: Option<Overlay>,
    pub show_archived: bool,
    /// Sidebars collapsed (z) — terminal takes the full width.
    pub collapsed: bool,
    /// Workspaces bar shown across the top of the body, with the panels
    /// below it; see `workspaces_bar_h`. Mirrors the `show_workspaces`
    /// setting, which both `Shift+W` and the Appearance tab write — this
    /// field is the live copy, the config file is where it persists.
    pub show_workspaces: bool,
    /// Projects panel hidden; mirrors CONFIG.JSON's `hide_projects`.
    pub hide_projects: bool,
    /// Worktrees panel hidden; mirrors CONFIG.JSON's `hide_worktrees`.
    pub hide_worktrees: bool,
    pub next_req_id: u64,
    pub pending: HashMap<u64, PendingIntent>,
    /// `nebula --workspace <name>`: the workspace this instance was asked
    /// to open into, held until the first snapshot arrives with the names
    /// to resolve it against. Taken there — it applies once, at boot.
    pub startup_workspace: Option<String>,
    /// Session created by us, awaiting its upsert to fix the selection.
    pub select_when_seen: Option<SessionRef>,
    /// Project added by us, awaiting its upsert to fix the selection.
    pub select_project_when_seen: Option<ProjectId>,
    /// Worktree created by us, awaiting its upsert to fix the selection.
    pub select_worktree_when_seen: Option<WorktreeId>,
    /// Last selected worktree per project — switching back to a project
    /// returns to the worktree the user left it on.
    pub last_worktree_for_project: HashMap<ProjectId, WorktreeId>,
    /// Last selected session per worktree — switching back to a worktree
    /// re-shows the session the user left it on.
    pub last_session_for_worktree: HashMap<WorktreeId, SessionRef>,
    /// Last selected project per workspace — switching back to a workspace
    /// returns to the project the user left it on, which is what makes the
    /// worktree and session memory above reachable across a workspace
    /// switch (they key off the project the cursor lands on).
    pub last_project_for_workspace: HashMap<WorkspaceId, ProjectId>,
    /// Debounced session prewarm: the worktree whose dead sessions the
    /// daemon should pre-spawn once the selection has rested on it past the
    /// deadline — armed on every worktree context switch, so walking the
    /// list doesn't boot every CLI it passes.
    pub pending_prewarm: Option<(WorktreeId, std::time::Instant)>,
    /// Debounced attach: the session the pane is showing but the daemon has
    /// not been told about yet. Stepping a selection is not a decision to
    /// boot a CLI — and in the Workspaces column every step is a full
    /// workspace switch, so without this, walking past four workspaces
    /// cold-spawns four agents and abandons three of them.
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
    /// Widths of the Projects / Worktrees / Sessions panels; the terminal
    /// pane takes the remainder.
    pub panel_widths: [u16; 3],
    /// Width of the Workspaces column when it's shown. Kept out of
    /// `panel_widths` so old persisted layouts still deserialize.
    /// File-list width of the diff modal, remembered across opens.
    pub diff_files_width: u16,
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
    /// In-progress splitter drag, if any.
    pub splitter_drag: Option<SplitterDrag>,
    /// Main-screen splitter under the mouse (a drag counts), highlighting
    /// that boundary's grip. Only ever set in terminals that report plain
    /// mouse motion; elsewhere the grip just stays in its resting shade.
    pub hover_splitter: Option<usize>,
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
    /// refreshes it on a slow poll and before drawing a changed selection.
    pub git_changes: Option<(WorktreeId, Option<usize>)>,
    /// What `gh pr view` last said about each worktree's branch: `Some(pr)`
    /// when one exists, `None` when the lookup came back empty (no PR, no
    /// `gh`, no remote). A missing key means "not looked up yet". An empty
    /// answer is re-asked on a backing-off timer (`pr_recheck`), since the
    /// PR a session opens appears well after the first lookup; a found one
    /// is final.
    pub pull_requests: HashMap<WorktreeId, Option<PullRequest>>,
    /// How far the user has read into each pull request's conversation,
    /// keyed by PR URL — the daemon's `pr_seen` rows, plus whatever this
    /// session has marked since. What's newer than the mark is what the
    /// row's unread badge counts.
    pub pr_seen: HashMap<String, String>,
    /// Worktrees with a lookup in flight, so a repaint can't stack a second
    /// `gh` process on the first.
    pub pr_inflight: std::collections::HashSet<WorktreeId>,
    /// When to ask `gh` about a worktree again, and the step that produced
    /// that deadline: a steady beat once its pull request is known (so the
    /// unread-comment count keeps up), a doubling backoff while it isn't.
    /// Switching into a worktree drops its entry, so arriving somewhere
    /// always asks again promptly.
    pub pr_recheck: HashMap<WorktreeId, (std::time::Instant, std::time::Duration)>,
    /// What `gh pr list` last said about each project's open pull requests
    /// — the group at the bottom of the Worktrees panel. A missing key
    /// means "never asked"; only the selected project is ever asked, so a
    /// workspace of thirty projects still costs one call per refresh.
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
    /// Top visible line of the pull-request preview pane, and the pane's
    /// total line count as of the last draw (for clamping).
    pub pr_preview_scroll: u16,
    pub pr_preview_lines: usize,
    /// The pull request whose full diff is being fetched, if any — one at a
    /// time, so mashing the key can't spawn a `gh pr diff` per press.
    pub pr_diff_inflight: Option<u64>,
    /// Where a finished `gh pr diff` is sent back to the loop; the main loop
    /// installs it at startup (the `vim_tx` precedent). Key handlers can
    /// therefore start a network fetch without the loop's channels in hand.
    pub pr_diff_tx: Option<tokio::sync::mpsc::UnboundedSender<(u64, String, Option<String>)>>,
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
    /// The `animations` setting: master switch for the status-text sweep
    /// and the splash's motion (off = fewer repaints). Mirrors the config,
    /// refreshed at startup and when the settings overlay applies a change.
    pub animations: bool,
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
            focus: Focus::Projects,
            sel_project: 0,
            sel_worktree: 0,
            sel_session: 0,
            sessions_scroll: 0,
            sessions_anchor: None,
            worktrees_scroll: 0,
            worktrees_anchor: None,
            term: None,
            term_locked: false,
            conn: ConnState::Disconnected,
            hits: Vec::new(),
            term_area: Rect::default(),
            dirty: true,
            should_quit: false,
            pending_ssh: None,
            flash: None,
            edge_tap: None,
            bar_return: Focus::Projects,
            overlay: None,
            show_archived: false,
            collapsed: false,
            show_workspaces: true,
            hide_projects: false,
            hide_worktrees: false,
            next_req_id: 1,
            pending: HashMap::new(),
            startup_workspace: None,
            select_when_seen: None,
            select_project_when_seen: None,
            select_worktree_when_seen: None,
            last_worktree_for_project: HashMap::new(),
            last_session_for_worktree: HashMap::new(),
            last_project_for_workspace: HashMap::new(),
            pending_prewarm: None,
            pending_attach: None,
            attached_sref: None,
            next_keepwarm: None,
            term_selection: None,
            last_term_click: None,
            last_session_click: None,
            term_links: Vec::new(),
            term_file_links: Vec::new(),
            panel_widths: DEFAULT_PANEL_WIDTHS,
            diff_files_width: DEFAULT_DIFF_FILES_W,
            settings_tab: 0,
            settings_selected: vec![0; crate::config::tab_count()],
            settings_on_tabs: true,
            settings_closed_at: None,
            keymap: crate::keymap::Keymap::default(),
            splitter_drag: None,
            hover_splitter: None,
            pointer_shape: PointerShape::default(),
            pending_clipboard: None,
            pending_ding: false,
            body_area: Rect::default(),
            hostname: nebula_core::host::hostname(),
            is_remote: nebula_core::host::is_remote_session(),
            theme: crate::theme::Theme::default(),
            vim: None,
            vim_tx: None,
            vim_generation: 0,
            git_changes: None,
            pull_requests: HashMap::new(),
            pr_seen: HashMap::new(),
            pr_inflight: std::collections::HashSet::new(),
            pr_recheck: HashMap::new(),
            open_prs: HashMap::new(),
            open_prs_inflight: std::collections::HashSet::new(),
            pr_detail: HashMap::new(),
            pr_detail_inflight: std::collections::HashSet::new(),
            pr_detail_failed: std::collections::HashSet::new(),
            pending_pr_detail: None,
            pr_preview_scroll: 0,
            pr_preview_lines: 0,
            pr_diff_inflight: None,
            pr_diff_tx: None,
            last_metrics: None,
            client_rss_bytes: 0,
            splash_epoch: std::time::Instant::now(),
            splash_preview: false,
            animations: true,
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

    /// The splash is what the body is showing: nothing in the tree yet
    /// (first run) or summoned with N, and the panels aren't collapsed
    /// away. True whether it's animating or drawn as a still frame, so the
    /// footer can key its hints off it.
    ///
    /// "First run" means the **default** workspace is empty. Any other
    /// empty workspace keeps the panels (with their "no projects yet"
    /// hints) — otherwise stepping the Workspaces column onto a fresh
    /// workspace would hide the very column being stepped through.
    pub fn splash_showing(&self) -> bool {
        !self.collapsed
            && ((self.tree.in_default_workspace() && !self.tree.has_visible_projects())
                || self.splash_preview)
    }

    /// The animated splash is on screen and should be ticking: nothing in
    /// the tree yet (first run) or summoned with N, panels not collapsed,
    /// no editor modal covering the body, animations enabled (off, the
    /// splash still draws — as a still frame).
    pub fn splash_active(&self) -> bool {
        self.animations && self.splash_showing() && self.vim.is_none()
    }

    /// Some sidebar row is showing a running (yellow) or needs-feedback
    /// (red) status, so its text sweep should be ticking. Any live agent in
    /// one of those states surfaces somewhere — its own row, or a worktree /
    /// project rollup — unless the panels are hidden (collapsed, editor
    /// modal, splash) or animations are switched off.
    pub fn status_anim_active(&self) -> bool {
        self.animations
            && !self.collapsed
            && self.vim.is_none()
            && !self.splash_active()
            && self.tree.agents.iter().any(|a| {
                !a.archived && matches!(a.status, AgentStatus::Running | AgentStatus::NeedsFeedback)
            })
    }

    /// Frame counter for the status-sweep text animation — a pure function
    /// of elapsed time (same model as the splash), so a missed tick just
    /// skips ahead instead of stuttering.
    pub fn sweep_phase(&self) -> usize {
        (self.splash_epoch.elapsed().as_millis() / SWEEP_FRAME.as_millis()) as usize
    }

    /// Rows the Workspaces bar takes off the top of the body when it's
    /// shown, nothing when hidden. Every screen-y computation for the
    /// panels below it starts here.
    pub fn workspaces_bar_h(&self) -> u16 {
        if self.show_workspaces {
            WORKSPACES_BAR_H
        } else {
            0
        }
    }

    /// Visible sidebar indices, left to right. Sessions is always present.
    pub fn visible_panel_indices(&self) -> Vec<usize> {
        (0..3).filter(|idx| self.panel_visible(*idx)).collect()
    }

    pub fn panel_visible(&self, idx: usize) -> bool {
        match idx {
            0 => !self.hide_projects,
            1 => !self.hide_worktrees,
            2 => true,
            _ => false,
        }
    }

    /// Every visible sidebar owns the draggable boundary on its right.
    pub fn splitter_indices(&self) -> Vec<usize> {
        self.visible_panel_indices()
    }

    /// Screen x of splitter `idx` — the column where the panel to its right
    /// starts, i.e. the right edge of panel `idx`.
    pub fn splitter_x(&self, idx: usize) -> u16 {
        self.visible_panel_indices()
            .into_iter()
            .filter(|visible| *visible <= idx)
            .map(|visible| self.panel_widths[visible])
            .sum()
    }

    /// Move splitter `idx` so its boundary lands at `boundary_x`, clamped so
    /// the panel keeps `MIN_PANEL_W` and the terminal pane keeps `MIN_TERM_W`.
    pub fn set_splitter(&mut self, idx: usize, boundary_x: i32, body_w: u16) {
        let want = boundary_x.max(0) as u16;
        if !self.panel_visible(idx) {
            return;
        }
        let visible = self.visible_panel_indices();
        let left: u16 = visible
            .iter()
            .copied()
            .filter(|visible| *visible < idx)
            .map(|visible| self.panel_widths[visible])
            .sum();
        let fixed_right: u16 = visible
            .iter()
            .copied()
            .filter(|visible| *visible > idx)
            .map(|visible| self.panel_widths[visible])
            .sum();
        let max = body_w.saturating_sub(left + fixed_right + MIN_TERM_W);
        if max < MIN_PANEL_W {
            return; // terminal too small to honor the minimums
        }
        self.panel_widths[idx] = want.saturating_sub(left).clamp(MIN_PANEL_W, max);
    }

    /// Re-fit panel widths to the current body width, shrinking the rightmost
    /// panel first, each floored at `MIN_PANEL_W`. Keeps the terminal pane at
    /// `MIN_TERM_W` whenever the screen allows it at all. The Workspaces bar
    /// spans the full width above them, so it costs the panels nothing here.
    pub fn normalize_panel_widths(&mut self, body_w: u16) {
        let budget = body_w.saturating_sub(MIN_TERM_W);
        let visible = self.visible_panel_indices();
        for i in visible.iter().rev().copied() {
            let others: u16 = visible
                .iter()
                .copied()
                .filter(|j| *j != i)
                .map(|j| self.panel_widths[j])
                .sum();
            let max = budget.saturating_sub(others);
            self.panel_widths[i] = self.panel_widths[i].clamp(MIN_PANEL_W, max.max(MIN_PANEL_W));
        }
    }

    pub fn alloc_req_id(&mut self, intent: PendingIntent) -> u64 {
        let id = self.next_req_id;
        self.next_req_id += 1;
        self.pending.insert(id, intent);
        id
    }

    /// Projects panel rows in display order, each an index into the FULL
    /// `tree.projects` list. Scoped to the open workspace — other
    /// workspaces' projects get no row.
    ///
    /// Most recently interacted with first — the newest stamp under any of
    /// the project's worktrees, so the project you just worked in heads the
    /// column (mirrors the sessions list; there is no manual reorder). The
    /// sort is stable, so never-run projects keep tree order at the bottom
    /// instead of shuffling between frames.
    pub fn project_rows(&self) -> Vec<usize> {
        let now = now_ms();
        let mut rows: Vec<usize> = self
            .tree
            .projects
            .iter()
            .enumerate()
            .filter(|(_, p)| self.tree.in_active_workspace(p))
            .map(|(i, _)| i)
            .collect();
        rows.sort_by_key(|i| {
            std::cmp::Reverse(
                project_recency(&self.tree, &self.tree.projects[*i].id, now).interacted,
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

    pub fn selected_worktree(&self) -> Option<&Worktree> {
        let worktrees = self.visible_worktrees();
        worktrees.get(self.sel_worktree).copied()
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

    /// Does the cache describe a different worktree than the selection?
    /// The event loop refreshes before drawing when it does, so the badge
    /// never lags a j/k by a poll interval.
    pub fn git_changes_stale(&self) -> bool {
        self.git_changes.as_ref().map(|(id, _)| id) != self.selected_worktree().map(|w| &w.id)
    }

    /// The full row list the panel shows — `sel_session` indexes this.
    pub fn visible_session_rows(&self) -> Vec<SessionRow> {
        let agents = self.visible_sessions();
        let (active, _) = self.session_group_counts();
        let active = active.min(agents.len());
        let mut rows: Vec<SessionRow> = agents[..active]
            .iter()
            .cloned()
            .map(SessionRow::Agent)
            .collect();
        rows.extend(
            self.visible_terminals()
                .into_iter()
                .map(SessionRow::Terminal),
        );
        rows.extend(self.visible_links().into_iter().map(SessionRow::Link));
        rows.extend(agents[active..].iter().cloned().map(SessionRow::Agent));
        rows
    }

    /// The selected worktree's OPEN PRS group: the pull request on its branch
    /// first (however it got there), then any previously saved links in list
    /// order. New saved links are no longer exposed through the TUI.
    /// A saved link that *is* the pull request is shown once, as the
    /// pull-request row — a duplicate would just be the same destination
    /// twice.
    pub fn visible_links(&self) -> Vec<LinkRow> {
        let worktrees = self.visible_worktrees();
        let Some(wt) = worktrees.get(self.sel_worktree) else {
            return vec![];
        };
        let saved: Vec<&Link> = self
            .tree
            .links
            .iter()
            .filter(|l| l.worktree_id == wt.id)
            .collect();
        let pr = self.pull_requests.get(&wt.id).cloned().flatten();
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
        self.visible_session_rows()
            .into_iter()
            .nth(self.sel_session)
    }

    /// The selected row's agent, when it is one (terminal rows return None).
    pub fn selected_session(&self) -> Option<Agent> {
        match self.selected_session_row() {
            Some(SessionRow::Agent(a)) => Some(a),
            _ => None,
        }
    }

    /// Shell terminals of the selected worktree, in tree order.
    pub fn visible_terminals(&self) -> Vec<TerminalTab> {
        let worktrees = self.visible_worktrees();
        let Some(wt) = worktrees.get(self.sel_worktree) else {
            return vec![];
        };
        self.tree
            .terminals
            .iter()
            .filter(|t| t.worktree_id == wt.id)
            .cloned()
            .collect()
    }

    /// First free `prefix-N` name within the selected worktree.
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
        let Some(project) = self.selected_project() else {
            return vec![];
        };
        let now = now_ms();
        let mut rows: Vec<&Worktree> = self
            .tree
            .worktrees
            .iter()
            .filter(|w| w.project_id == project.id)
            .collect();
        rows.sort_by_key(|w| {
            (
                std::cmp::Reverse(w.is_main),
                std::cmp::Reverse(worktree_recency(&self.tree, &w.id, now).interacted),
            )
        });
        rows
    }

    /// The selected project's open pull requests — the group under the
    /// checkouts. Empty until the first `gh pr list` answers (or when the
    /// repo genuinely has none).
    pub fn visible_open_prs(&self) -> &[OpenPr] {
        self.selected_project()
            .and_then(|p| self.open_prs.get(&p.id))
            .map(|o| o.list.as_slice())
            .unwrap_or_default()
    }

    /// How many rows the Worktrees panel has: the project's checkouts, then
    /// the pull requests still open on its repo. `sel_worktree` indexes that
    /// combined list, and because the checkouts come first every existing
    /// "index into `visible_worktrees()`" stays exactly right — a cursor
    /// parked on a pull request simply has no selected worktree, which is
    /// the truth about it.
    pub fn worktree_row_count(&self) -> usize {
        self.visible_worktrees().len() + self.visible_open_prs().len()
    }

    /// The open pull request under the Worktrees cursor, when it's on one.
    /// Mutually exclusive with [`App::selected_worktree`] — the checkouts
    /// occupy the rows below the pull requests.
    pub fn selected_worktree_pr(&self) -> Option<&OpenPr> {
        let i = self
            .sel_worktree
            .checked_sub(self.visible_worktrees().len())?;
        self.visible_open_prs().get(i)
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

    /// Session rows for the selected worktree: the live agents, then (when
    /// shown) the archived ones.
    ///
    /// The live rows are ordered by last interaction, newest first, so the
    /// session you just ran surfaces at the top and the list reads as a
    /// history of what you have been doing. Working sessions count as
    /// interacting now, which keeps them on top however long the turn has
    /// taken.
    pub fn visible_sessions(&self) -> Vec<Agent> {
        let worktrees = self.visible_worktrees();
        let Some(wt) = worktrees.get(self.sel_worktree) else {
            return vec![];
        };
        let now = now_ms();
        // Stable throughout, so ties — never-run rows especially, which all
        // stamp 0 — keep tree order instead of shuffling between frames.
        let mut rows: Vec<Agent> = self
            .tree
            .agents
            .iter()
            .filter(|a| a.worktree_id == wt.id && !a.archived)
            .cloned()
            .collect();
        rows.sort_by_key(|a| recency_key(a, now));
        if self.show_archived {
            let mut archived: Vec<Agent> = self
                .tree
                .agents
                .iter()
                .filter(|a| a.worktree_id == wt.id && a.archived)
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
        let worktrees = self.visible_worktrees();
        let Some(wt) = worktrees.get(self.sel_worktree) else {
            return (0, 0);
        };
        let live = self
            .tree
            .agents
            .iter()
            .filter(|a| a.worktree_id == wt.id && !a.archived)
            .count();
        let archived = self
            .tree
            .agents
            .iter()
            .filter(|a| a.worktree_id == wt.id && a.archived)
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

    /// The body and conversation behind the pull request the pane is
    /// reading: `Some(Some(_))` once fetched, `Some(None)` while it's still
    /// coming (or came back empty), `None` when the pane isn't reading one.
    pub fn selected_pr_detail(&self) -> Option<Option<&PrDetail>> {
        let pr = self.previewed_pr()?;
        Some(self.pr_detail.get(&pr.url))
    }

    /// Delay until the standing keep-warm re-send is due. None when disarmed.
    pub fn keepwarm_delay(&self) -> Option<std::time::Duration> {
        let at = self.next_keepwarm.as_ref()?;
        Some(at.saturating_duration_since(std::time::Instant::now()))
    }

    /// Aggregate status for a worktree row: red > yellow > green > gray,
    /// archived agents excluded.
    pub fn worktree_rollup(&self, worktree_id: &WorktreeId) -> Option<AgentStatus> {
        worktree_rollup(&self.tree, worktree_id)
    }

    pub fn project_rollup(&self, project_id: &ProjectId) -> Option<AgentStatus> {
        project_rollup(&self.tree, project_id)
    }

    /// When the worktree last saw a turn — what its row sorts and labels on.
    pub fn worktree_recency(&self, worktree_id: &WorktreeId) -> Recency {
        worktree_recency(&self.tree, worktree_id, now_ms())
    }

    pub fn project_recency(&self, project_id: &ProjectId) -> Recency {
        project_recency(&self.tree, project_id, now_ms())
    }

    pub fn workspace_rollup(&self, workspace_id: &WorkspaceId) -> Option<AgentStatus> {
        workspace_rollup(&self.tree, workspace_id)
    }

    pub fn workspace_unseen(&self, workspace_id: &WorkspaceId) -> usize {
        workspace_unseen(&self.tree, workspace_id)
    }

    /// Sessions under a worktree that went green with nobody looking.
    pub fn worktree_unseen(&self, worktree_id: &WorktreeId) -> usize {
        worktree_unseen(&self.tree, worktree_id)
    }

    pub fn project_unseen(&self, project_id: &ProjectId) -> usize {
        project_unseen(&self.tree, project_id)
    }

    /// First visible sidebar under the Workspaces bar.
    pub fn first_sidebar_focus(&self) -> Focus {
        if !self.hide_projects {
            Focus::Projects
        } else if !self.hide_worktrees {
            Focus::Worktrees
        } else {
            Focus::Sessions
        }
    }

    pub fn focus_visible(&self, focus: Focus) -> bool {
        match focus {
            Focus::Workspaces => self.show_workspaces,
            Focus::Projects => !self.hide_projects,
            Focus::Worktrees => !self.hide_worktrees,
            Focus::Sessions | Focus::Terminal => true,
        }
    }

    fn focus_rank(focus: Focus) -> u8 {
        match focus {
            Focus::Workspaces => 0,
            Focus::Projects => 1,
            Focus::Worktrees => 2,
            Focus::Sessions => 3,
            Focus::Terminal => 4,
        }
    }

    pub fn next_visible_focus(&self, focus: Focus) -> Focus {
        let rank = Self::focus_rank(focus);
        [
            Focus::Workspaces,
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
            Focus::Workspaces,
        ]
        .into_iter()
        .find(|candidate| Self::focus_rank(*candidate) < rank && self.focus_visible(*candidate))
        .unwrap_or(focus)
    }

    /// First stop in the Tab walk (and where a cross-workspace jump lands):
    /// the Workspaces bar when shown, otherwise the first visible sidebar.
    pub fn first_focus(&self) -> Focus {
        if self.show_workspaces {
            Focus::Workspaces
        } else {
            self.first_sidebar_focus()
        }
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
            workspace_id: Default::default(),
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
            state: "OPEN".into(),
            is_draft: false,
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
            model: None,
            effort: None,
            session_id: None,
            cloud_session_id: None,
            sort_order: 0,
            alive: true,
            cloud_mirroring: false,
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
}
