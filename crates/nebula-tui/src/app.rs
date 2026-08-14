//! TUI state: the Elm-ish Model.

use crate::git_diff::DiffFile;
use nebula_core::{
    Agent, AgentId, AgentKind, AgentStatus, Project, ProjectId, SessionRef, Worktree, WorktreeId,
};
use ratatui::layout::Rect;
use std::collections::HashMap;
use std::path::PathBuf;

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

// ---- overlays ----

#[derive(Debug, Clone, PartialEq)]
pub enum MenuAction {
    Attach(SessionRef),
    RestartAgent(AgentId),
    RenameAgent(AgentId),
    ArchiveAgent(AgentId),
    UnarchiveAgent(AgentId),
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
    pub at: (u16, u16),
    pub hover: usize,
    /// Set during draw for click hit-testing.
    pub area: Rect,
}

/// Destructive action waiting behind a confirmation.
#[derive(Debug, Clone, PartialEq)]
pub enum PendingAction {
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

/// Full-screen git-diff viewer: file list left, scrollable diff right.
#[derive(Debug, Clone)]
pub struct DiffView {
    /// Checkout dir the diffs are read from.
    pub root: PathBuf,
    /// Branch name for the pane title.
    pub branch: String,
    pub files: Vec<DiffFile>,
    /// Index into `files`.
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
    /// Whether the repo has a commit; picks the diff command.
    pub head_ok: bool,
}

impl DiffView {
    pub fn max_scroll(&self) -> u16 {
        (self.diff_line_count as u16).saturating_sub(self.view_height.max(1))
    }

    /// Clamped relative scroll.
    pub fn scroll_by(&mut self, delta: i32) {
        self.scroll = (self.scroll as i32 + delta).clamp(0, self.max_scroll() as i32) as u16;
    }

    /// Clamped absolute file selection; true when it changed (the caller
    /// reloads the diff).
    pub fn select(&mut self, index: i64) -> bool {
        let max = self.files.len().saturating_sub(1) as i64;
        let clamped = index.clamp(0, max) as usize;
        let changed = clamped != self.selected;
        self.selected = clamped;
        changed
    }
}

#[derive(Debug, Clone)]
pub enum Overlay {
    Menu(ContextMenu),
    Confirm(ConfirmDialog),
    Prompt(PromptDialog),
    Help,
    Diff(DiffView),
}

/// What to do when an Ack for this req_id arrives.
#[derive(Debug, Clone, PartialEq)]
pub enum PendingIntent {
    /// Attach the created session and focus the terminal.
    AttachCreated,
    /// Select the created worktree in the Worktrees panel.
    SelectCreatedWorktree,
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
    /// URLs detected on the visible screen during the last draw; hit-tested
    /// on ⌥click and underlined by the renderer.
    pub term_links: Vec<crate::links::TermLink>,
    /// Widths of the Projects / Worktrees / Sessions panels; the terminal
    /// pane takes the remainder.
    pub panel_widths: [u16; 3],
    /// In-progress splitter drag, if any.
    pub splitter_drag: Option<SplitterDrag>,
    /// Body rect (everything above the footer) from the last draw; bounds
    /// splitter drags.
    pub body_area: Rect,
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
            term_links: Vec::new(),
            panel_widths: DEFAULT_PANEL_WIDTHS,
            splitter_drag: None,
            body_area: Rect::default(),
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

    /// Session rows for the selected worktree: active agents, then (when
    /// shown) archived agents.
    pub fn visible_sessions(&self) -> Vec<Agent> {
        let worktrees = self.visible_worktrees();
        let Some(wt) = worktrees.get(self.sel_worktree) else {
            return vec![];
        };
        let mut rows: Vec<Agent> = self
            .tree
            .agents
            .iter()
            .filter(|a| a.worktree_id == wt.id && !a.archived)
            .cloned()
            .collect();
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

    /// (active agents, archived-total) for the selected worktree.
    pub fn session_group_counts(&self) -> (usize, usize) {
        let worktrees = self.visible_worktrees();
        let Some(wt) = worktrees.get(self.sel_worktree) else {
            return (0, 0);
        };
        let agents = self
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
        (agents, archived)
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
