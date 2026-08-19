//! TUI state: the Elm-ish Model.

use crate::git_diff::DiffFile;
use nebula_core::{
    Agent, AgentId, AgentKind, AgentStatus, Project, ProjectId, SessionRef, Worktree, WorktreeId,
};
use ratatui::layout::Rect;
use std::collections::HashMap;
use std::path::PathBuf;

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
    /// Row index into `App::project_rows()` (projects and dividers both).
    Project(usize),
    Worktree(usize),
    Session(usize),
    /// Panel background (registered after rows, so rows win).
    PanelBg(Focus),
    TerminalPane,
    /// Draggable vertical boundary between panels, left to right:
    /// 0 = projects|worktrees, 1 = worktrees|sessions, 2 = sessions|terminal.
    Splitter(usize),
}

/// Default widths of the Projects / Worktrees / Sessions panels.
pub const DEFAULT_PANEL_WIDTHS: [u16; 3] = [20, 22, 26];
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
    /// Picker result: create an agent of this kind (chains into the name prompt).
    NewAgentOfKind(WorktreeId, AgentKind),
    NewWorktree(ProjectId),
    DeleteWorktree(WorktreeId),
    AddProject,
    RemoveProject(ProjectId),
    SetProjectDivider(ProjectId, bool),
    LabelDivider(ProjectId),
    ToggleArchived,
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
}

/// Destructive action waiting behind a confirmation.
#[derive(Debug, Clone, PartialEq)]
pub enum PendingAction {
    /// AddProject aimed at a path that doesn't exist yet: create the
    /// directory (daemon-side, `git init` per its config) and add it.
    CreateProjectDir(std::path::PathBuf),
    ArchiveAgent(AgentId),
    DeleteAgent(AgentId),
    DeleteWorktree(WorktreeId),
    RemoveProject(ProjectId),
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
    /// Label for the divider hanging below this project.
    DividerLabel {
        id: ProjectId,
    },
    NewWorktree {
        project: ProjectId,
    },
    NewAgent {
        worktree: WorktreeId,
        kind: AgentKind,
    },
    RenameAgent {
        id: AgentId,
    },
}

#[derive(Debug, Clone)]
pub struct PromptDialog {
    pub title: String,
    pub label: String,
    pub input: String,
    pub kind: PromptKind,
    /// Tab-completion candidates to display (path prompts only).
    pub candidates: Vec<String>,
}

impl PromptDialog {
    /// Does Tab complete filesystem paths in this prompt?
    pub fn completes_paths(&self) -> bool {
        matches!(self.kind, PromptKind::AddProject)
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
    pub filter: String,
    /// Visible rows: `files` narrowed by `filter`, best matches first
    /// (git order when the filter is empty).
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
}

impl DiffView {
    pub fn new(root: PathBuf, branch: String, files: Vec<DiffFile>, head_ok: bool) -> Self {
        let mut view = Self {
            root,
            branch,
            files,
            filter: String::new(),
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
    /// diff). Best matches first, git order when the filter is empty.
    pub fn apply_filter(&mut self) -> bool {
        let before = self.matches.get(self.selected).map(|m| m.file);
        self.matches = crate::fuzzy::rank(&self.filter, self.files.iter().map(|f| f.path.as_str()))
            .into_iter()
            .map(|(file, positions)| DiffMatch { file, positions })
            .collect();
        self.selected = 0;
        before != self.matches.first().map(|m| m.file)
    }
}

/// What a `/` palette row jumps to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PaletteTarget {
    Project(ProjectId),
    Worktree(WorktreeId),
    Session(AgentId),
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
    pub query: String,
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
    pub fn new(tree: &Tree, show_archived: bool, enter_attaches: bool) -> Self {
        let mut palette = Self {
            items: build_palette_items(tree, show_archived),
            query: String::new(),
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
    pub fn rebuild(&mut self, tree: &Tree, show_archived: bool) {
        let keep = self.selected_target().cloned();
        self.items = build_palette_items(tree, show_archived);
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
/// worktrees, then each worktree's sessions. Archived sessions appear only
/// when the archived toggle is on (the Sessions panel rule).
fn build_palette_items(tree: &Tree, show_archived: bool) -> Vec<PaletteItem> {
    let mut items = Vec::new();
    for p in &tree.projects {
        items.push(PaletteItem {
            target: PaletteTarget::Project(p.id.clone()),
            text: p.name.clone(),
            archived: false,
        });
    }
    for p in &tree.projects {
        for w in tree.worktrees.iter().filter(|w| w.project_id == p.id) {
            items.push(PaletteItem {
                target: PaletteTarget::Worktree(w.id.clone()),
                text: format!("{}/{}", p.name, w.branch),
                archived: false,
            });
        }
    }
    for p in &tree.projects {
        for w in tree.worktrees.iter().filter(|w| w.project_id == p.id) {
            for a in tree.agents.iter().filter(|a| a.worktree_id == w.id) {
                if a.archived && !show_archived {
                    continue;
                }
                items.push(PaletteItem {
                    target: PaletteTarget::Session(a.id.clone()),
                    text: format!("{}/{}/{}", p.name, w.branch, a.name),
                    archived: a.archived,
                });
            }
        }
    }
    items
}

#[derive(Debug, Clone, Default)]
pub struct SettingsView {
    pub selected: usize,
    /// Set during draw for click hit-testing.
    pub area: Rect,
}

impl SettingsView {
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
    /// Select the created worktree in the Worktrees panel.
    SelectCreatedWorktree,
    /// Worktree removed optimistically; restore these rows on Error.
    DeleteWorktree(WorktreeRollback),
    None,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnState {
    Connected,
    Disconnected,
}

/// SessionRef for an agent row (the only session kind the panel shows).
pub fn agent_sref(a: &Agent) -> SessionRef {
    SessionRef::Agent(a.id.clone())
}

/// One selectable row in the Projects panel. The payload indexes
/// `tree.projects`; a `Divider` is the separator hanging below that project.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProjectRow {
    Project(usize),
    Divider(usize),
}

impl ProjectRow {
    /// Index of the project this row belongs to (a divider belongs to the
    /// project it hangs below).
    pub fn project_index(&self) -> usize {
        match self {
            ProjectRow::Project(i) | ProjectRow::Divider(i) => *i,
        }
    }
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

/// Client-side mirror of the entity tree.
#[derive(Debug, Clone, Default)]
pub struct Tree {
    pub projects: Vec<Project>,
    pub worktrees: Vec<Worktree>,
    pub agents: Vec<Agent>,
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
        self.parser.set_scrollback(scroll);
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

pub struct App {
    pub tree: Tree,
    pub focus: Focus,
    /// Selected row in the Projects panel — indexes `project_rows()`, which
    /// interleaves projects and their dividers.
    pub sel_project: usize,
    pub sel_worktree: usize,
    pub sel_session: usize,
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
    pub flash: Option<String>,
    pub overlay: Option<Overlay>,
    pub show_archived: bool,
    /// Sidebars collapsed (z) — terminal takes the full width.
    pub collapsed: bool,
    pub next_req_id: u64,
    pub pending: HashMap<u64, PendingIntent>,
    /// Session created by us, awaiting its upsert to fix the selection.
    pub select_when_seen: Option<SessionRef>,
    /// A divider we asked to move, awaiting the upsert that lands it under
    /// this project so the selection can follow it there.
    pub select_divider_when_seen: Option<ProjectId>,
    /// Worktree created by us, awaiting its upsert to fix the selection.
    pub select_worktree_when_seen: Option<WorktreeId>,
    /// Last selected worktree per project — switching back to a project
    /// returns to the worktree the user left it on.
    pub last_worktree_for_project: HashMap<ProjectId, WorktreeId>,
    /// Last selected session per worktree — switching back to a worktree
    /// re-shows the session the user left it on.
    pub last_session_for_worktree: HashMap<WorktreeId, SessionRef>,
    /// Mouse drag-selection over the terminal pane, if any.
    pub term_selection: Option<TermSelection>,
    /// Last left-click on the terminal pane (time + pane-relative cell), for
    /// double-click detection.
    pub last_term_click: Option<(std::time::Instant, (u16, u16))>,
    /// Last left-click on a session row (time + agent), for double-click
    /// pin/unpin detection.
    pub last_session_click: Option<(std::time::Instant, AgentId)>,
    /// URLs detected on the visible screen during the last draw; hit-tested
    /// on ⌥click and underlined by the renderer.
    pub term_links: Vec<crate::links::TermLink>,
    /// Widths of the Projects / Worktrees / Sessions panels; the terminal
    /// pane takes the remainder.
    pub panel_widths: [u16; 3],
    /// File-list width of the diff modal, remembered across opens.
    pub diff_files_width: u16,
    /// In-progress splitter drag, if any.
    pub splitter_drag: Option<SplitterDrag>,
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
    pub drawn_session: Option<AgentId>,
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
            term: None,
            term_locked: false,
            conn: ConnState::Disconnected,
            hits: Vec::new(),
            term_area: Rect::default(),
            dirty: true,
            should_quit: false,
            flash: None,
            overlay: None,
            show_archived: false,
            collapsed: false,
            next_req_id: 1,
            pending: HashMap::new(),
            select_when_seen: None,
            select_divider_when_seen: None,
            select_worktree_when_seen: None,
            last_worktree_for_project: HashMap::new(),
            last_session_for_worktree: HashMap::new(),
            term_selection: None,
            last_term_click: None,
            last_session_click: None,
            term_links: Vec::new(),
            panel_widths: DEFAULT_PANEL_WIDTHS,
            diff_files_width: DEFAULT_DIFF_FILES_W,
            splitter_drag: None,
            body_area: Rect::default(),
            hostname: nebula_core::host::hostname(),
            is_remote: nebula_core::host::is_remote_session(),
            recent_window_ms: crate::config::DEFAULT_RECENT_WINDOW_MS,
            drawn_session: None,
        }
    }

    /// Screen x of splitter `idx` — the column where the panel to its right
    /// starts (prefix sum of panel widths).
    pub fn splitter_x(&self, idx: usize) -> u16 {
        self.panel_widths[..=idx].iter().sum()
    }

    /// Move splitter `idx` so its boundary lands at `boundary_x`, clamped so
    /// the panel keeps `MIN_PANEL_W` and the terminal pane keeps `MIN_TERM_W`.
    pub fn set_splitter(&mut self, idx: usize, boundary_x: i32, body_w: u16) {
        let left: u16 = self.panel_widths[..idx].iter().sum();
        let fixed_right: u16 = self.panel_widths[idx + 1..].iter().sum();
        let max = body_w.saturating_sub(left + fixed_right + MIN_TERM_W);
        if max < MIN_PANEL_W {
            return; // terminal too small to honor the minimums
        }
        let want = boundary_x.max(0) as u16;
        self.panel_widths[idx] = want.saturating_sub(left).clamp(MIN_PANEL_W, max);
    }

    /// Re-fit panel widths to the current body width, shrinking the rightmost
    /// panel first, each floored at `MIN_PANEL_W`. Keeps the terminal pane at
    /// `MIN_TERM_W` whenever the screen allows it at all.
    pub fn normalize_panel_widths(&mut self, body_w: u16) {
        let budget = body_w.saturating_sub(MIN_TERM_W);
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

    /// Projects panel rows in display order: each project, then its divider
    /// (when present) directly below it.
    pub fn project_rows(&self) -> Vec<ProjectRow> {
        let mut rows = Vec::with_capacity(self.tree.projects.len());
        for (i, p) in self.tree.projects.iter().enumerate() {
            rows.push(ProjectRow::Project(i));
            if p.divider_after {
                rows.push(ProjectRow::Divider(i));
            }
        }
        rows
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

    pub fn selected_session(&self) -> Option<Agent> {
        self.visible_sessions().into_iter().nth(self.sel_session)
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

    /// Worktrees of the selected project, in tree order.
    pub fn visible_worktrees(&self) -> Vec<&Worktree> {
        let Some(project) = self.selected_project() else {
            return vec![];
        };
        self.tree
            .worktrees
            .iter()
            .filter(|w| w.project_id == project.id)
            .collect()
    }

    /// An unpinned, unarchived agent whose status changed within the
    /// configured window sorts into the RECENT group. Pinned agents never
    /// join it — PINNED always stays on top.
    pub fn is_recent(&self, a: &Agent) -> bool {
        !a.pinned
            && !a.archived
            && self.recent_window_ms > 0
            && a.status_changed_at > 0
            && now_ms().saturating_sub(a.status_changed_at) < self.recent_window_ms
    }

    /// Session rows for the selected worktree: pinned agents, then RECENT
    /// (status changed within the window), then the remaining unpinned,
    /// then (when shown) archived agents.
    pub fn visible_sessions(&self) -> Vec<Agent> {
        let worktrees = self.visible_worktrees();
        let Some(wt) = worktrees.get(self.sel_worktree) else {
            return vec![];
        };
        let mut rows: Vec<Agent> = self
            .tree
            .agents
            .iter()
            .filter(|a| a.worktree_id == wt.id && !a.archived && a.pinned)
            .cloned()
            .collect();
        rows.extend(
            self.tree
                .agents
                .iter()
                .filter(|a| a.worktree_id == wt.id && self.is_recent(a))
                .cloned(),
        );
        rows.extend(
            self.tree
                .agents
                .iter()
                .filter(|a| {
                    a.worktree_id == wt.id && !a.archived && !a.pinned && !self.is_recent(a)
                })
                .cloned(),
        );
        if self.show_archived {
            rows.extend(
                self.tree
                    .agents
                    .iter()
                    .filter(|a| a.worktree_id == wt.id && a.archived)
                    .cloned(),
            );
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
    /// pending expiry.
    pub fn next_recent_expiry(&self) -> Option<std::time::Duration> {
        let worktrees = self.visible_worktrees();
        let wt = worktrees.get(self.sel_worktree)?;
        let now = now_ms();
        self.tree
            .agents
            .iter()
            .filter(|a| a.worktree_id == wt.id && self.is_recent(a))
            .map(|a| (a.status_changed_at + self.recent_window_ms - now).max(0) as u64)
            .min()
            .map(std::time::Duration::from_millis)
    }

    /// Aggregate status for a worktree row: red > yellow > green > gray,
    /// archived agents excluded.
    pub fn worktree_rollup(&self, worktree_id: &WorktreeId) -> Option<AgentStatus> {
        rollup(
            self.tree
                .agents
                .iter()
                .filter(|a| &a.worktree_id == worktree_id && !a.archived)
                .map(|a| a.status),
        )
    }

    pub fn project_rollup(&self, project_id: &ProjectId) -> Option<AgentStatus> {
        let wt_ids: Vec<&WorktreeId> = self
            .tree
            .worktrees
            .iter()
            .filter(|w| &w.project_id == project_id)
            .map(|w| &w.id)
            .collect();
        rollup(
            self.tree
                .agents
                .iter()
                .filter(|a| wt_ids.contains(&&a.worktree_id) && !a.archived)
                .map(|a| a.status),
        )
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
}
