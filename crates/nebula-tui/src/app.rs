//! TUI state: the Elm-ish Model.

use nebula_core::{
    Agent, AgentId, AgentStatus, Project, ProjectId, SessionRef, TerminalId, TerminalTab,
    Worktree, WorktreeId,
};
use ratatui::layout::Rect;
use std::collections::HashMap;

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
    Project(usize),
    Worktree(usize),
    Session(usize),
    /// Panel background (registered after rows, so rows win).
    PanelBg(Focus),
    TerminalPane,
}

// ---- overlays ----

#[derive(Debug, Clone, PartialEq)]
pub enum MenuAction {
    Attach(SessionRef),
    RestartAgent(AgentId),
    RenameAgent(AgentId),
    RenameTerminal(TerminalId),
    ArchiveAgent(AgentId),
    UnarchiveAgent(AgentId),
    DeleteAgent(AgentId),
    CloseTerminal(TerminalId),
    NewAgent(WorktreeId),
    NewTerminal(WorktreeId),
    NewWorktree(ProjectId),
    DeleteWorktree(WorktreeId),
    AddProject,
    RemoveProject(ProjectId),
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
    CloseTerminal(TerminalId),
    DeleteWorktree(WorktreeId),
    RemoveProject(ProjectId),
    Quit,
}

#[derive(Debug, Clone)]
pub struct ConfirmDialog {
    pub title: String,
    pub message: String,
    /// When set, the user must type this exact string to enable Yes.
    pub typed_guard: Option<String>,
    pub input: String,
    pub action: PendingAction,
}

#[derive(Debug, Clone, PartialEq)]
pub enum PromptKind {
    AddProject,
    NewWorktree { project: ProjectId },
    NewAgent { worktree: WorktreeId },
    NewTerminal { worktree: WorktreeId },
    RenameAgent { id: AgentId },
    RenameTerminal { id: TerminalId },
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

#[derive(Debug, Clone)]
pub enum Overlay {
    Menu(ContextMenu),
    Confirm(ConfirmDialog),
    Prompt(PromptDialog),
    Help,
}

/// What to do when an Ack for this req_id arrives.
#[derive(Debug, Clone, PartialEq)]
pub enum PendingIntent {
    /// Attach the created session and focus the terminal.
    AttachCreated,
    None,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnState {
    Connected,
    Disconnected,
}

/// One row in the Sessions panel: agents on top, terminals below.
#[derive(Debug, Clone)]
pub enum SessionRow {
    Agent(Agent),
    Terminal(TerminalTab),
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

impl SessionRow {
    pub fn name(&self) -> &str {
        match self {
            SessionRow::Agent(a) => &a.name,
            SessionRow::Terminal(t) => &t.name,
        }
    }

    pub fn sref(&self) -> SessionRef {
        match self {
            SessionRow::Agent(a) => SessionRef::Agent(a.id.clone()),
            SessionRow::Terminal(t) => SessionRef::Terminal(t.id.clone()),
        }
    }

    pub fn status(&self) -> Option<AgentStatus> {
        match self {
            SessionRow::Agent(a) => Some(a.status),
            SessionRow::Terminal(_) => None,
        }
    }

    pub fn is_archived(&self) -> bool {
        matches!(self, SessionRow::Agent(a) if a.archived)
    }
}

/// Client-side mirror of the entity tree.
#[derive(Debug, Clone, Default)]
pub struct Tree {
    pub projects: Vec<Project>,
    pub worktrees: Vec<Worktree>,
    pub agents: Vec<Agent>,
    pub terminals: Vec<TerminalTab>,
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
    pub session_terminal: Option<String>,
    pub show_archived: bool,
    pub collapsed: bool,
}

/// A mouse drag-selection over the terminal pane, in pane-relative cell
/// coordinates `(col, row)` with inclusive endpoints. Nebula owns the mouse
/// (the emulator's native shift+drag never reaches us reliably — Terminal.app
/// has no such bypass at all), so selection is implemented app-side and
/// copied to the system clipboard on mouse-up.
#[derive(Debug, Clone, Copy)]
pub struct TermSelection {
    pub anchor: (u16, u16),
    pub head: (u16, u16),
    /// Still being dragged (button down). Cleared on mouse-up.
    pub dragging: bool,
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

    /// A drag that never left its starting cell — just a click.
    pub fn is_empty(&self) -> bool {
        self.anchor == self.head
    }
}

pub struct App {
    pub tree: Tree,
    pub focus: Focus,
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
    /// Mouse drag-selection over the terminal pane, if any.
    pub term_selection: Option<TermSelection>,
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
            term_selection: None,
        }
    }

    pub fn alloc_req_id(&mut self, intent: PendingIntent) -> u64 {
        let id = self.next_req_id;
        self.next_req_id += 1;
        self.pending.insert(id, intent);
        id
    }

    pub fn selected_project(&self) -> Option<&Project> {
        self.tree.projects.get(self.sel_project)
    }

    pub fn selected_worktree(&self) -> Option<&Worktree> {
        let worktrees = self.visible_worktrees();
        worktrees.get(self.sel_worktree).copied()
    }

    pub fn selected_session(&self) -> Option<SessionRow> {
        self.visible_sessions().into_iter().nth(self.sel_session)
    }

    /// First free `prefix-N` name within the selected worktree.
    pub fn default_session_name(&self, prefix: &str) -> String {
        let taken: Vec<String> = self
            .visible_sessions()
            .iter()
            .map(|r| r.name().to_string())
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
        let Some(project) = self.tree.projects.get(self.sel_project) else {
            return vec![];
        };
        self.tree.worktrees.iter().filter(|w| w.project_id == project.id).collect()
    }

    /// Session rows for the selected worktree: active agents, then terminals,
    /// then (when shown) archived agents.
    pub fn visible_sessions(&self) -> Vec<SessionRow> {
        let worktrees = self.visible_worktrees();
        let Some(wt) = worktrees.get(self.sel_worktree) else {
            return vec![];
        };
        let mut rows: Vec<SessionRow> = self
            .tree
            .agents
            .iter()
            .filter(|a| a.worktree_id == wt.id && !a.archived)
            .cloned()
            .map(SessionRow::Agent)
            .collect();
        rows.extend(
            self.tree
                .terminals
                .iter()
                .filter(|t| t.worktree_id == wt.id)
                .cloned()
                .map(SessionRow::Terminal),
        );
        if self.show_archived {
            rows.extend(
                self.tree
                    .agents
                    .iter()
                    .filter(|a| a.worktree_id == wt.id && a.archived)
                    .cloned()
                    .map(SessionRow::Agent),
            );
        }
        rows
    }

    /// (active agents, terminals, archived-total) for the selected worktree.
    pub fn session_group_counts(&self) -> (usize, usize, usize) {
        let worktrees = self.visible_worktrees();
        let Some(wt) = worktrees.get(self.sel_worktree) else {
            return (0, 0, 0);
        };
        let agents =
            self.tree.agents.iter().filter(|a| a.worktree_id == wt.id && !a.archived).count();
        let terminals = self.tree.terminals.iter().filter(|t| t.worktree_id == wt.id).count();
        let archived =
            self.tree.agents.iter().filter(|a| a.worktree_id == wt.id && a.archived).count();
        (agents, terminals, archived)
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
