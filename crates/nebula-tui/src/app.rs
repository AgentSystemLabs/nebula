//! TUI state: the Elm-ish Model.

use crate::git_diff::DiffFile;
use crate::pull_request::{OpenPr, PrDetail, PullRequest};
use crate::text_input::TextInput;
use nebula_core::{
    Agent, AgentId, AgentKind, AgentStatus, Link, LinkId, Note, NoteId, NoteOwner, Project,
    ProjectId, SessionRef, TerminalId, TerminalTab, Workspace, WorkspaceId, Worktree, WorktreeId,
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
    /// The optional leftmost column (`Shift+W` shows/hides it). Its cursor
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
    /// Row index into `App::project_rows()` (projects and dividers both).
    Project(usize),
    Worktree(usize),
    Session(usize),
    /// The ARCHIVED group header (either form); a click toggles the group
    /// open/closed, same as the A key.
    ArchivedHeader,
    /// Panel background (registered after rows, so rows win).
    PanelBg(Focus),
    TerminalPane,
    /// Draggable vertical boundary between panels, left to right:
    /// 0 = workspaces|projects (only while the column is shown),
    /// 1 = projects|worktrees, 2 = worktrees|sessions, 3 = sessions|terminal.
    Splitter(usize),
}

/// Default widths of the Projects / Worktrees / Sessions panels. Sessions
/// is the widest because its rows carry the most: name, "23m ago", harness.
pub const DEFAULT_PANEL_WIDTHS: [u16; 3] = [20, 22, 32];
/// Starting width of the Workspaces column. It's draggable like the three
/// panels to its right, but persisted separately (`UiState::workspaces_w`)
/// rather than as a fourth slot in the `[u16; 3]` `panel_widths` blob, so
/// older saved layouts keep loading. Names longer than the column truncate,
/// as project names do.
pub const DEFAULT_WORKSPACES_PANEL_W: u16 = 18;
/// A panel can't be dragged narrower than this.
pub const MIN_PANEL_W: u16 = 10;
/// The terminal pane always keeps at least this much width.
pub const MIN_TERM_W: u16 = 20;

/// Default outer width of the diff modal's file-list panel.
pub const DEFAULT_DIFF_FILES_W: u16 = 34;
/// The diff modal's file list can't be dragged narrower than this.
pub const MIN_DIFF_FILES_W: u16 = 16;
/// The diff pane always keeps at least this much width.
pub const MIN_DIFF_PANE_W: u16 = 24;

// ---- overlays ----

#[derive(Debug, Clone, PartialEq)]
pub enum MenuAction {
    Attach(SessionRef),
    RestartAgent(AgentId),
    RenameAgent(AgentId),
    /// Opens the destination-worktree picker for this agent.
    MoveAgent(AgentId),
    /// Picker result: re-home the agent under this worktree.
    MoveAgentToWorktree(AgentId, WorktreeId),
    ArchiveAgent(AgentId),
    UnarchiveAgent(AgentId),
    SetAgentPinned(AgentId, bool),
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
    },
    /// Shell terminal in the worktree's directory; created immediately with
    /// a default name (no prompt), renameable later.
    NewTerminal(WorktreeId),
    RenameTerminal(TerminalId),
    CloseTerminal(TerminalId),
    NewWorktree(ProjectId),
    /// Open the note modal for this owner (project or worktree).
    OpenNotes(NoteOwner),
    /// Attach a URL to this worktree (prompts for it).
    NewLink(WorktreeId),
    /// Hand a link row's URL to the browser.
    OpenLink(String),
    /// Read the selected open pull request's diff in the diff modal. Carries
    /// no id: the row is the selection, and the fetch reads it back off the
    /// cursor the same way `g` does.
    ViewPrDiff,
    EditLink(LinkId),
    DeleteLink(LinkId),
    SetWorktreePinned(WorktreeId, bool),
    DeleteWorktree(WorktreeId),
    AddProject,
    RemoveProject(ProjectId),
    /// Workspace-switcher row: open this workspace. The switcher's other
    /// verbs are keys, not rows — n: new, r: rename, d: delete (footer
    /// hints, the notes-modal pattern).
    OpenWorkspace(WorkspaceId),
    /// The Workspaces column's menu rows: the switcher's n / r / d verbs,
    /// for a mouse that never learned the keys.
    NewWorkspace,
    RenameWorkspace(WorkspaceId),
    RemoveWorkspace(WorkspaceId),
    SetProjectDivider {
        id: ProjectId,
        before: bool,
        present: bool,
    },
    LabelDivider(ProjectId, bool),
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
    /// effort; a row that already carries an effort is a leaf.
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
                    (Some(_), None) => Some(SubmenuKind::Efforts),
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
}

impl ContextMenu {
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
    DeleteLink(LinkId),
    Quit,
}

#[derive(Debug, Clone)]
pub struct ConfirmDialog {
    pub title: String,
    pub message: String,
    pub action: PendingAction,
}

#[derive(Debug, Clone, PartialEq)]
pub enum PromptKind {
    AddProject,
    /// Label for one of this project's dividers (`before` = the leading
    /// divider above the list).
    DividerLabel {
        id: ProjectId,
        before: bool,
    },
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
    RenameAgent {
        id: AgentId,
    },
    RenameTerminal {
        id: TerminalId,
    },
    /// Name for a workspace created from the switcher; opened on Ack.
    NewWorkspace,
    RenameWorkspace {
        id: WorkspaceId,
    },
    /// Pin a URL to this worktree.
    NewLink {
        worktree: WorktreeId,
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
        };
        prompt.refresh_dirs();
        prompt
    }

    /// Does Tab complete filesystem paths in this prompt?
    pub fn completes_paths(&self) -> bool {
        matches!(self.kind, PromptKind::AddProject)
    }

    /// The Claude Cloud task is the only prompt with a multi-row editor.
    pub fn is_multiline(&self) -> bool {
        matches!(self.kind, PromptKind::ClaudeCloudTask { .. })
    }

    fn home() -> Option<std::path::PathBuf> {
        std::env::var_os("HOME").map(std::path::PathBuf::from)
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
        (self.diff_line_count as u16).saturating_sub(self.view_height.max(1))
    }

    /// Screen x of the files/diff boundary — the column where the diff panel
    /// starts.
    pub fn splitter_x(&self) -> u16 {
        self.area.x + self.files_width
    }

    /// Move the files/diff boundary to `boundary_x`, clamped so the file list
    /// keeps `MIN_DIFF_FILES_W` and the diff pane keeps `MIN_DIFF_PANE_W`.
    pub fn set_files_width(&mut self, boundary_x: i32) {
        let max = self.area.width.saturating_sub(MIN_DIFF_PANE_W);
        if max < MIN_DIFF_FILES_W {
            return; // modal too small to honor the minimums
        }
        let want = (boundary_x - self.area.x as i32).max(0) as u16;
        self.files_width = want.clamp(MIN_DIFF_FILES_W, max);
    }

    /// Clamped relative scroll.
    pub fn scroll_by(&mut self, delta: i32) {
        self.scroll = (self.scroll as i32 + delta).clamp(0, self.max_scroll() as i32) as u16;
    }

    /// Clamped absolute selection in the filtered list; true when it changed
    /// (the caller reloads the diff).
    pub fn select(&mut self, index: i64) -> bool {
        let max = self.matches.len().saturating_sub(1) as i64;
        let clamped = index.clamp(0, max) as usize;
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
        (self.selected + 1).saturating_sub(height)
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

/// What a `/` palette row jumps to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PaletteTarget {
    Project(ProjectId),
    Worktree(WorktreeId),
    Session(AgentId),
    /// An open pull request on some project's repo, addressed by URL — the
    /// only identity it has, since nothing about a PR is stored. Picking it
    /// opens a browser instead of moving any panel cursor.
    PullRequest(String),
}

/// One searchable row of the `/` palette. `text` is both the string the
/// fuzzy filter runs over and the string rendered after the kind badge, so
/// match highlighting always lines up: `name` for projects,
/// `project/branch` for worktrees, `project/branch/name` for sessions —
/// letting a query narrow by parent context.
#[derive(Debug, Clone)]
pub struct PaletteItem {
    pub target: PaletteTarget,
    pub text: String,
    pub archived: bool,
    /// The status this row's panel row would show: a rollup for projects
    /// and worktrees, its own status for a session. Drives the glyph color
    /// and the text sweep, so a running session reads as running in the
    /// palette too. Refreshed by [`Palette::rebuild`] as upserts land.
    pub status: Option<AgentStatus>,
}

/// One visible palette row: an index into `items` plus the char positions of
/// `text` the query matched, for highlighting.
#[derive(Debug, Clone)]
pub struct PaletteMatch {
    pub item: usize,
    pub positions: Vec<usize>,
}

/// Fuzzy-search palette over every project, worktree, and session (`/`).
#[derive(Debug, Clone)]
pub struct Palette {
    pub items: Vec<PaletteItem>,
    /// Type-to-filter query over `items` texts; always live.
    pub query: TextInput,
    /// Visible rows: `items` narrowed by `query`, best matches first (build
    /// order when the query is empty).
    pub matches: Vec<PaletteMatch>,
    /// Index into `matches` (not `items`).
    pub selected: usize,
    /// Whether Enter (and a click) on a session row attaches to it, or only
    /// lands on its Sessions-panel row. Snapshot of the config setting at
    /// open time; Ctrl+O / Ctrl+F pick explicitly either way.
    pub enter_attaches: bool,
    /// Whole modal rect, written back during draw so clicks outside close.
    pub area: Rect,
    /// Screen rect of the result rows (query row excluded), written back
    /// during draw so clicks can hit-test rows.
    pub list_area: Rect,
}

impl Palette {
    pub fn new(
        tree: &Tree,
        show_archived: bool,
        enter_attaches: bool,
        open_prs: &HashMap<ProjectId, OpenPrs>,
    ) -> Self {
        let mut palette = Self {
            items: build_palette_items(tree, show_archived, open_prs),
            query: TextInput::new(),
            matches: Vec::new(),
            selected: 0,
            enter_attaches,
            area: Rect::default(),
            list_area: Rect::default(),
        };
        palette.apply_filter();
        palette
    }

    /// Re-derive `items` after the tree changed under an open palette,
    /// keeping the query — and the cursor: agent status flips arrive as
    /// upserts every few seconds, and a rebuild must not yank the user's
    /// ↑/↓ position to the top. The selection follows its target's row;
    /// only a vanished target falls back to the best match.
    pub fn rebuild(
        &mut self,
        tree: &Tree,
        show_archived: bool,
        open_prs: &HashMap<ProjectId, OpenPrs>,
    ) {
        let keep = self.selected_target().cloned();
        self.items = build_palette_items(tree, show_archived, open_prs);
        self.apply_filter();
        if let Some(target) = keep {
            if let Some(row) = self
                .matches
                .iter()
                .position(|m| self.items[m.item].target == target)
            {
                self.selected = row;
            }
        }
    }

    /// First visible row of the result list's stateless follow-window for a
    /// list of `height` rows.
    pub fn window_start(&self, height: usize) -> usize {
        (self.selected + 1).saturating_sub(height)
    }

    /// Clamped absolute selection in the filtered list.
    pub fn select(&mut self, index: i64) {
        let max = self.matches.len().saturating_sub(1) as i64;
        self.selected = index.clamp(0, max) as usize;
    }

    /// The jump target behind the current selection, if any row is visible.
    pub fn selected_target(&self) -> Option<&PaletteTarget> {
        Some(
            &self
                .items
                .get(self.matches.get(self.selected)?.item)?
                .target,
        )
    }

    /// Recompute `matches` from `query` and reset the selection to the top
    /// row. Best matches first, build order when the query is empty.
    pub fn apply_filter(&mut self) {
        self.matches = crate::fuzzy::rank(&self.query, self.items.iter().map(|i| i.text.as_str()))
            .into_iter()
            .map(|(item, positions)| PaletteMatch { item, positions })
            .collect();
        self.selected = 0;
    }
}

/// Every jumpable entity: projects in tree order, then each project's
/// worktrees, then each worktree's sessions, then the open pull requests
/// nebula has fetched for each project. Archived sessions appear only when
/// the archived toggle is on (the Sessions panel rule). Scoped to the open
/// workspace — `/` never searches across other workspaces.
fn build_palette_items(
    tree: &Tree,
    show_archived: bool,
    open_prs: &HashMap<ProjectId, OpenPrs>,
) -> Vec<PaletteItem> {
    let projects: Vec<&Project> = tree
        .projects
        .iter()
        .filter(|p| tree.in_active_workspace(p))
        .collect();
    let mut items = Vec::new();
    for p in &projects {
        items.push(PaletteItem {
            target: PaletteTarget::Project(p.id.clone()),
            text: p.name.clone(),
            archived: false,
            status: project_rollup(tree, &p.id),
        });
    }
    for p in &projects {
        for w in tree.worktrees.iter().filter(|w| w.project_id == p.id) {
            items.push(PaletteItem {
                target: PaletteTarget::Worktree(w.id.clone()),
                text: format!("{}/{}", p.name, w.branch),
                archived: false,
                status: worktree_rollup(tree, &w.id),
            });
        }
    }
    for p in &projects {
        for w in tree.worktrees.iter().filter(|w| w.project_id == p.id) {
            for a in tree.agents.iter().filter(|a| a.worktree_id == w.id) {
                if a.archived && !show_archived {
                    continue;
                }
                items.push(PaletteItem {
                    target: PaletteTarget::Session(a.id.clone()),
                    text: format!("{}/{}/{}", p.name, w.branch, a.name),
                    archived: a.archived,
                    status: Some(a.status),
                });
            }
        }
    }
    // Pull requests go last so a query that also matches a session still
    // lands on the session first — the panels are what `/` is mostly for.
    // Only projects whose list has actually been fetched contribute; the
    // rest simply have nothing to offer yet.
    for p in &projects {
        let Some(open) = open_prs.get(&p.id) else {
            continue;
        };
        for pr in &open.list {
            items.push(PaletteItem {
                target: PaletteTarget::PullRequest(pr.url.clone()),
                text: format!("{}/{}", p.name, pr.label()),
                archived: false,
                status: None,
            });
        }
    }
    items
}

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
        (self.selected + 1).saturating_sub(height)
    }

    /// Clamped absolute selection in the filtered list.
    pub fn select(&mut self, index: i64) {
        let max = self.matches.len().saturating_sub(1) as i64;
        self.selected = index.clamp(0, max) as usize;
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
        (self.selected + 1).saturating_sub(height)
    }

    /// Clamped absolute selection.
    pub fn select(&mut self, index: i64) {
        let max = self.hits.len().saturating_sub(1) as i64;
        self.selected = index.clamp(0, max) as usize;
    }

    /// The hit behind the current selection, if any row is visible.
    pub fn selected_hit(&self) -> Option<&crate::grep_search::GrepHit> {
        self.hits.get(self.selected)
    }
}

/// In-progress add/edit inside the note modal; keys feed `text` while set.
#[derive(Debug, Clone)]
pub struct NoteInput {
    /// None = creating a new note; Some = rewriting that note's text.
    pub editing: Option<NoteId>,
    pub text: TextInput,
}

/// Note notes modal (`o`) for one owner — a project (high-level notes) or
/// a worktree. The rows themselves live in `App::tree.notes` (kept fresh
/// by upserts) — the view only holds the owner plus cursor/input state.
#[derive(Debug, Clone)]
pub struct NoteView {
    pub owner: NoteOwner,
    /// `project` or `project/branch`, for the modal title.
    pub context: String,
    /// Index into the owner's note rows.
    pub selected: usize,
    /// Active add/edit input, if any.
    pub input: Option<NoteInput>,
    /// Whole modal rect, written back during draw so clicks outside close.
    pub area: Rect,
    /// Screen rect of the note rows, written back during draw so clicks can
    /// hit-test rows.
    pub list_area: Rect,
}

impl NoteView {
    pub fn new(owner: NoteOwner, context: String) -> Self {
        Self {
            owner,
            context,
            selected: 0,
            input: None,
            area: Rect::default(),
            list_area: Rect::default(),
        }
    }

    /// First visible row of the list's stateless follow-window for a list of
    /// `height` rows.
    pub fn window_start(&self, height: usize) -> usize {
        (self.selected + 1).saturating_sub(height)
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
        (self.selected + 1).saturating_sub(height)
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
    /// cycles a setting's value.
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
    /// `tab`/`selected` are the remembered cursor position
    /// (`App::settings_tab` / `App::settings_selected`), clamped in case
    /// the lists shrank between builds.
    pub fn new(tab: usize, selected: usize) -> Self {
        let tab = tab.min(crate::config::tab_count().saturating_sub(1));
        Self {
            tab,
            selected: selected.min(crate::config::tab_len(tab).saturating_sub(1)),
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
    Help,
    Settings(SettingsView),
    Diff(DiffView),
    Palette(Palette),
    Files(FileFinder),
    Grep(GrepView),
    Tree(crate::tree_browser::TreeBrowser),
    Notes(NoteView),
    Metrics(MetricsView),
    Hosts(HostsView),
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
    /// Select the added project and step into its Worktrees panel.
    SelectCreatedProject,
    /// Select the created worktree in the Worktrees panel.
    SelectCreatedWorktree,
    /// Move the note modal's cursor onto the created note.
    SelectCreatedNote,
    /// Move the Sessions panel's cursor onto the link just created.
    SelectCreatedLink,
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

/// One row of the Sessions panel's LINKS group: a URL the user pinned to
/// the worktree, or the pull request nebula found on its branch.
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

/// One row in the Sessions panel: agents (pinned / recent / unpinned), then
/// shell terminals, then the worktree's links, then archived agents.
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

    /// Raw last-interaction stamp, or 0 for rows that carry no "23m ago"
    /// label (terminals, links, sessions that have never run).
    pub fn last_interaction_at(&self) -> i64 {
        match self {
            SessionRow::Agent(a) => a.status_changed_at,
            SessionRow::Terminal(_) | SessionRow::Link(_) => 0,
        }
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

/// How many agents are running right now across a workspace — the
/// Workspaces column's count badge, next to the rollup dot.
pub fn workspace_running(tree: &Tree, workspace_id: &WorkspaceId) -> usize {
    workspace_agents(tree, workspace_id)
        .filter(|a| a.status == AgentStatus::Running)
        .count()
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

/// One selectable row in the Projects panel. The payload indexes
/// `tree.projects`; a `Divider` is the separator hanging below that project
/// — or, with `before`, the leading divider drawn above the whole list
/// (always owned by project 0).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProjectRow {
    Project(usize),
    Divider { project: usize, before: bool },
}

impl ProjectRow {
    /// Index of the project this row belongs to (a divider belongs to the
    /// project that owns it).
    pub fn project_index(&self) -> usize {
        match self {
            ProjectRow::Project(i) | ProjectRow::Divider { project: i, .. } => *i,
        }
    }
}

/// A session that is mid-turn or blocked on the user. These head the
/// RECENT group and never age out of it — the whole point of the group is
/// to keep what needs attention in view.
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
/// projects; the view layer scopes to `active_workspace` (see
/// [`App::project_rows`] and [`build_palette_items`]), so a workspace switch
/// is a pure re-filter — no refetch, and background workspaces keep
/// receiving status updates.
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
    pub notes: Vec<Note>,
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
        }
    }

    /// Reset the parser (fresh replay is about to arrive).
    pub fn reset(&mut self) {
        self.parser = vt100::Parser::new(self.rows, self.cols, 10_000);
        self.exited = false;
        self.scroll = 0;
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
    /// Whether the Workspaces column is shown (`Shift+W`); absent in older
    /// blobs, which means shown.
    #[serde(default)]
    pub show_workspaces: Option<bool>,
    /// Dragged width of the Workspaces column; absent in older blobs.
    #[serde(default)]
    pub workspaces_w: Option<u16>,
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

/// How recently a project's open-PR list may have been fetched and still be
/// refetched on arrival. Walking the project list must not turn into one
/// API call per row visited.
pub const OPEN_PRS_MIN_AGE: std::time::Duration = std::time::Duration::from_secs(30);

pub struct App {
    pub tree: Tree,
    pub focus: Focus,
    /// Selected row in the Projects panel — indexes `project_rows()`, which
    /// interleaves projects and their dividers.
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
    pub overlay: Option<Overlay>,
    pub show_archived: bool,
    /// Sidebars collapsed (z) — terminal takes the full width.
    pub collapsed: bool,
    /// Workspaces column shown at the left edge (`Shift+W` toggles). The
    /// three panels sit to its right; see `splitter_x`.
    pub show_workspaces: bool,
    pub next_req_id: u64,
    pub pending: HashMap<u64, PendingIntent>,
    /// `nebula --workspace <name>`: the workspace this instance was asked
    /// to open into, held until the first snapshot arrives with the names
    /// to resolve it against. Taken there — it applies once, at boot.
    pub startup_workspace: Option<String>,
    /// Session created by us, awaiting its upsert to fix the selection.
    pub select_when_seen: Option<SessionRef>,
    /// A divider we asked to move, awaiting the upsert that lands it on
    /// this project (`true` = its leading divider) so the selection can
    /// follow it there.
    pub select_divider_when_seen: Option<(ProjectId, bool)>,
    /// Project added by us, awaiting its upsert to fix the selection.
    pub select_project_when_seen: Option<ProjectId>,
    /// Worktree created by us, awaiting its upsert to fix the selection.
    pub select_worktree_when_seen: Option<WorktreeId>,
    /// Note created by us, awaiting its upsert to land the modal's cursor.
    pub select_note_when_seen: Option<NoteId>,
    /// Link created by us, awaiting its upsert to land the panel cursor on
    /// the new row.
    pub select_link_when_seen: Option<LinkId>,
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
    pub workspaces_w: u16,
    /// File-list width of the diff modal, remembered across opens.
    pub diff_files_width: u16,
    /// Selected tab of the settings modal, remembered across opens.
    pub settings_tab: usize,
    /// Cursor row of the settings modal, one per tab, remembered across
    /// opens so switching tabs and coming back lands where you left.
    pub settings_selected: Vec<usize>,
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
    /// Body rect (everything above the footer) from the last draw; bounds
    /// splitter drags.
    pub body_area: Rect,
    /// Short machine hostname, shown at the far left of the footer.
    pub hostname: String,
    /// Running inside an ssh session (SSH_CONNECTION/SSH_TTY) — the footer
    /// colors the hostname as a remote warning.
    pub is_remote: bool,
    /// Unpinned sessions whose status changed within this window sort into
    /// a RECENT group (below PINNED). 0 disables the group. From config
    /// (`recent_window`); the event loop refreshes it.
    pub recent_window_ms: i64,
    /// Session under the cursor as of the last draw, so the RECENT-expiry
    /// tick can re-anchor `sel_session` after rows regroup underneath it.
    pub drawn_session: Option<SessionRef>,
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
    /// The `focus_tint` setting: paints the focused panel's background
    /// with a faint accent tint. Off by default; mirrors the config,
    /// refreshed at startup and when the settings overlay applies a change.
    pub focus_tint: bool,
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
            overlay: None,
            show_archived: false,
            collapsed: false,
            show_workspaces: true,
            next_req_id: 1,
            pending: HashMap::new(),
            startup_workspace: None,
            select_when_seen: None,
            select_divider_when_seen: None,
            select_project_when_seen: None,
            select_worktree_when_seen: None,
            select_note_when_seen: None,
            select_link_when_seen: None,
            last_worktree_for_project: HashMap::new(),
            last_session_for_worktree: HashMap::new(),
            pending_prewarm: None,
            next_keepwarm: None,
            term_selection: None,
            last_term_click: None,
            last_session_click: None,
            term_links: Vec::new(),
            term_file_links: Vec::new(),
            panel_widths: DEFAULT_PANEL_WIDTHS,
            workspaces_w: DEFAULT_WORKSPACES_PANEL_W,
            diff_files_width: DEFAULT_DIFF_FILES_W,
            settings_tab: 0,
            settings_selected: vec![0; crate::config::tab_count()],
            keymap: crate::keymap::Keymap::default(),
            splitter_drag: None,
            hover_splitter: None,
            pointer_shape: PointerShape::default(),
            body_area: Rect::default(),
            hostname: nebula_core::host::hostname(),
            is_remote: nebula_core::host::is_remote_session(),
            recent_window_ms: crate::config::DEFAULT_RECENT_WINDOW_MS,
            drawn_session: None,
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
            focus_tint: false,
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
    pub fn splash_showing(&self) -> bool {
        !self.collapsed && (!self.tree.has_visible_projects() || self.splash_preview)
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

    /// Columns the Workspaces panel takes at the left edge: its dragged
    /// width when shown, nothing when hidden. Every screen-x computation for
    /// the three panels to its right starts here.
    pub fn workspaces_panel_w(&self) -> u16 {
        if self.show_workspaces {
            self.workspaces_w
        } else {
            0
        }
    }

    /// The splitters that exist right now, left to right. The Workspaces
    /// boundary (0) only exists while its column is shown; the three panel
    /// boundaries (1..=3) always do.
    pub fn splitter_indices(&self) -> std::ops::Range<usize> {
        if self.show_workspaces {
            0..4
        } else {
            1..4
        }
    }

    /// Screen x of splitter `idx` — the column where the panel to its right
    /// starts. 0 is the Workspaces column's right edge; 1..=3 add a prefix
    /// sum of the panel widths on top of it.
    pub fn splitter_x(&self, idx: usize) -> u16 {
        self.workspaces_panel_w() + self.panel_widths[..idx].iter().sum::<u16>()
    }

    /// Move splitter `idx` so its boundary lands at `boundary_x`, clamped so
    /// the panel keeps `MIN_PANEL_W` and the terminal pane keeps `MIN_TERM_W`.
    pub fn set_splitter(&mut self, idx: usize, boundary_x: i32, body_w: u16) {
        let want = boundary_x.max(0) as u16;
        // Splitter 0 is the Workspaces column's own right edge: it starts at
        // x=0, so the boundary IS its width, and everything to its right
        // keeps the width it has.
        if idx == 0 {
            let fixed_right: u16 = self.panel_widths.iter().sum();
            let max = body_w.saturating_sub(fixed_right + MIN_TERM_W);
            if max >= MIN_PANEL_W {
                self.workspaces_w = want.clamp(MIN_PANEL_W, max);
            }
            return;
        }
        // The Workspaces column is spoken for before any panel is sized.
        let idx = idx - 1;
        let offset = self.workspaces_panel_w();
        let body_w = body_w.saturating_sub(offset);
        let left: u16 = self.panel_widths[..idx].iter().sum();
        let fixed_right: u16 = self.panel_widths[idx + 1..].iter().sum();
        let max = body_w.saturating_sub(left + fixed_right + MIN_TERM_W);
        if max < MIN_PANEL_W {
            return; // terminal too small to honor the minimums
        }
        self.panel_widths[idx] = want.saturating_sub(offset + left).clamp(MIN_PANEL_W, max);
    }

    /// Re-fit panel widths to the current body width, shrinking the rightmost
    /// panel first, each floored at `MIN_PANEL_W`. Keeps the terminal pane at
    /// `MIN_TERM_W` whenever the screen allows it at all. The Workspaces
    /// column, when shown, comes off the top of the budget — and gives way
    /// first, since a width dragged out on a wide screen must not squeeze
    /// three panels plus the pane off a narrow one.
    pub fn normalize_panel_widths(&mut self, body_w: u16) {
        if self.show_workspaces {
            let max = body_w.saturating_sub(3 * MIN_PANEL_W + MIN_TERM_W);
            self.workspaces_w = self.workspaces_w.clamp(MIN_PANEL_W, max.max(MIN_PANEL_W));
        }
        let budget = body_w
            .saturating_sub(self.workspaces_panel_w())
            .saturating_sub(MIN_TERM_W);
        for i in (0..3).rev() {
            let others: u16 = self
                .panel_widths
                .iter()
                .enumerate()
                .filter(|(j, _)| *j != i)
                .map(|(_, w)| *w)
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

    /// Projects panel rows in display order: the leading divider (when
    /// present), then each project with its divider directly below it.
    /// Scoped to the open workspace — rows index the FULL `tree.projects`
    /// list but other workspaces' projects get no row.
    pub fn project_rows(&self) -> Vec<ProjectRow> {
        let mut rows = Vec::with_capacity(self.tree.projects.len() + 1);
        let mut first = true;
        for (i, p) in self.tree.projects.iter().enumerate() {
            if !self.tree.in_active_workspace(p) {
                continue;
            }
            if first && p.divider_before {
                rows.push(ProjectRow::Divider {
                    project: i,
                    before: true,
                });
            }
            first = false;
            rows.push(ProjectRow::Project(i));
            if p.divider_after {
                rows.push(ProjectRow::Divider {
                    project: i,
                    before: false,
                });
            }
        }
        rows
    }

    /// Is the Projects-panel selection sitting on a divider row? The
    /// Worktrees/Sessions panels and the terminal pane blank their content
    /// while it is — a separator has nothing underneath it to show.
    pub fn divider_focused(&self) -> bool {
        matches!(
            self.selected_project_row(),
            Some(ProjectRow::Divider { .. })
        )
    }

    pub fn selected_project_row(&self) -> Option<ProjectRow> {
        self.project_rows().get(self.sel_project).copied()
    }

    /// The project giving the current selection its context. Selecting a
    /// divider keeps the context of the project it hangs below, so the
    /// Worktrees/Sessions panels stay put while walking the list.
    pub fn selected_project(&self) -> Option<&Project> {
        let row = self.selected_project_row()?;
        self.tree.projects.get(row.project_index())
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
        let (pinned, recent, unpinned, _) = self.session_group_counts();
        let active = (pinned + recent + unpinned).min(agents.len());
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

    /// The selected worktree's LINKS group: the pull request on its branch
    /// first (however it got there), then the saved links in list order.
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

    /// Worktrees of the selected project: pinned first, then the rest,
    /// each group in tree order (mirrors the sessions list).
    pub fn visible_worktrees(&self) -> Vec<&Worktree> {
        let Some(project) = self.selected_project() else {
            return vec![];
        };
        let mut rows: Vec<&Worktree> = self
            .tree
            .worktrees
            .iter()
            .filter(|w| w.project_id == project.id && w.pinned)
            .collect();
        rows.extend(
            self.tree
                .worktrees
                .iter()
                .filter(|w| w.project_id == project.id && !w.pinned),
        );
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

    /// (pinned, unpinned) worktree counts for the selected project.
    pub fn worktree_group_counts(&self) -> (usize, usize) {
        let Some(project) = self.selected_project() else {
            return (0, 0);
        };
        let pinned = self
            .tree
            .worktrees
            .iter()
            .filter(|w| w.project_id == project.id && w.pinned)
            .count();
        let unpinned = self
            .tree
            .worktrees
            .iter()
            .filter(|w| w.project_id == project.id && !w.pinned)
            .count();
        (pinned, unpinned)
    }

    /// An unpinned, unarchived agent sorts into the RECENT group when it is
    /// still working (running / needs-feedback, however long that has taken)
    /// or when its status changed within the configured window. Pinned
    /// agents never join it — PINNED always stays on top, and `off` still
    /// collapses the group entirely.
    pub fn is_recent(&self, a: &Agent) -> bool {
        !a.pinned
            && !a.archived
            && self.recent_window_ms > 0
            && (is_active_status(a.status)
                || (a.status_changed_at > 0
                    && now_ms().saturating_sub(a.status_changed_at) < self.recent_window_ms))
    }

    /// Session rows for the selected worktree: pinned agents, then RECENT
    /// (whatever is working or changed status within the window), then the
    /// remaining unpinned, then (when shown) archived agents.
    ///
    /// Every group is ordered by last interaction, newest first, so the
    /// session you just ran surfaces at the top of its group and the list
    /// reads as a history of what you have been doing. Working sessions
    /// count as interacting now, which keeps them heading RECENT however
    /// long the turn has taken.
    pub fn visible_sessions(&self) -> Vec<Agent> {
        let worktrees = self.visible_worktrees();
        let Some(wt) = worktrees.get(self.sel_worktree) else {
            return vec![];
        };
        let now = now_ms();
        // Stable throughout, so ties — never-run rows especially, which all
        // stamp 0 — keep tree order instead of shuffling between frames.
        let collect = |keep: &dyn Fn(&Agent) -> bool| {
            let mut group: Vec<Agent> = self
                .tree
                .agents
                .iter()
                .filter(|a| a.worktree_id == wt.id && keep(a))
                .cloned()
                .collect();
            group.sort_by_key(|a| recency_key(a, now));
            group
        };
        let mut rows = collect(&|a| !a.archived && a.pinned);
        rows.extend(collect(&|a| self.is_recent(a)));
        rows.extend(collect(&|a| !a.archived && !a.pinned && !self.is_recent(a)));
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

    /// (pinned, recent, unpinned, archived-total) for the selected worktree.
    pub fn session_group_counts(&self) -> (usize, usize, usize, usize) {
        let worktrees = self.visible_worktrees();
        let Some(wt) = worktrees.get(self.sel_worktree) else {
            return (0, 0, 0, 0);
        };
        let pinned = self
            .tree
            .agents
            .iter()
            .filter(|a| a.worktree_id == wt.id && !a.archived && a.pinned)
            .count();
        let recent = self
            .tree
            .agents
            .iter()
            .filter(|a| a.worktree_id == wt.id && self.is_recent(a))
            .count();
        let unpinned = self
            .tree
            .agents
            .iter()
            .filter(|a| a.worktree_id == wt.id && !a.archived && !a.pinned && !self.is_recent(a))
            .count();
        let archived = self
            .tree
            .agents
            .iter()
            .filter(|a| a.worktree_id == wt.id && a.archived)
            .count();
        (pinned, recent, unpinned, archived)
    }

    /// Delay until the next visible RECENT session ages out of the window,
    /// so the event loop can wake up and regroup. None when nothing is
    /// pending expiry. Working sessions are excluded — they hold their place
    /// in RECENT past the window, so their deadline is already behind us and
    /// would otherwise wake the loop on every pass.
    pub fn next_recent_expiry(&self) -> Option<std::time::Duration> {
        let worktrees = self.visible_worktrees();
        let wt = worktrees.get(self.sel_worktree)?;
        let now = now_ms();
        self.tree
            .agents
            .iter()
            .filter(|a| a.worktree_id == wt.id && self.is_recent(a) && !is_active_status(a.status))
            .map(|a| (a.status_changed_at + self.recent_window_ms - now).max(0) as u64)
            .min()
            .map(std::time::Duration::from_millis)
    }

    /// Delay until the pending worktree-sessions prewarm is due, so the
    /// event loop can wake up and fire it. None when nothing is armed.
    pub fn prewarm_delay(&self) -> Option<std::time::Duration> {
        let (_, at) = self.pending_prewarm.as_ref()?;
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

    /// The body and conversation behind the open-PR row under the cursor:
    /// `Some(Some(_))` once fetched, `Some(None)` while it's still coming
    /// (or came back empty), `None` when the cursor isn't on one at all.
    pub fn selected_pr_detail(&self) -> Option<Option<&PrDetail>> {
        let pr = self.selected_worktree_pr()?;
        Some(self.pr_detail.get(&pr.url))
    }

    /// Delay until the standing keep-warm re-send is due. None when disarmed.
    pub fn keepwarm_delay(&self) -> Option<std::time::Duration> {
        let at = self.next_keepwarm.as_ref()?;
        Some(at.saturating_duration_since(std::time::Instant::now()))
    }

    /// An owner's notes, in tree order (snapshot order; new ones append).
    pub fn notes_for(&self, owner: &NoteOwner) -> Vec<&Note> {
        self.tree
            .notes
            .iter()
            .filter(|t| &t.owner == owner)
            .collect()
    }

    /// (open, total) note counts for an owner — the row badges.
    pub fn note_stats(&self, owner: &NoteOwner) -> (usize, usize) {
        let total = self.tree.notes.iter().filter(|t| &t.owner == owner).count();
        let open = self
            .tree
            .notes
            .iter()
            .filter(|t| &t.owner == owner && !t.done)
            .count();
        (open, total)
    }

    /// Aggregate status for a worktree row: red > yellow > green > gray,
    /// archived agents excluded.
    pub fn worktree_rollup(&self, worktree_id: &WorktreeId) -> Option<AgentStatus> {
        worktree_rollup(&self.tree, worktree_id)
    }

    pub fn project_rollup(&self, project_id: &ProjectId) -> Option<AgentStatus> {
        project_rollup(&self.tree, project_id)
    }

    pub fn workspace_rollup(&self, workspace_id: &WorkspaceId) -> Option<AgentStatus> {
        workspace_rollup(&self.tree, workspace_id)
    }

    pub fn workspace_running(&self, workspace_id: &WorkspaceId) -> usize {
        workspace_running(&self.tree, workspace_id)
    }

    /// The panel focus lands on walking off the left edge (or wrapping
    /// around from the terminal pane): the Workspaces column when it's
    /// shown, Projects otherwise.
    pub fn leftmost_focus(&self) -> Focus {
        if self.show_workspaces {
            Focus::Workspaces
        } else {
            Focus::Projects
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
            divider_after: false,
            divider_label: None,
            divider_before: false,
            divider_before_label: None,
        });
        app.tree.worktrees.push(Worktree {
            id: worktree_id.clone(),
            project_id,
            path: "/tmp/demo".into(),
            branch: "main".into(),
            is_main: true,
            pinned: false,
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
            pinned: false,
            status_changed_at: 0,
            kind: AgentKind::Claude,
            model: None,
            effort: None,
            session_id: None,
            sort_order: 0,
            alive: true,
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
