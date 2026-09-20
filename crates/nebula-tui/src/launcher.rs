//! The LAUNCHER VIEW (Settings → Experimental, `launcher_view`): nebula
//! built around the prompt instead of the tree. It opens on the QUICK
//! PROMPT, already focused, on the launch the AGENTS TAB defaults describe
//! — `^P` picks the PROJECT with type-ahead over every one this machine
//! knows, `^O` the MODEL, `Tab` the harness, `^N` flips between a fresh
//! worktree and the project's checkout — and once something has been sent
//! the three panels are gone: a GRID of cards, one per session in the open
//! workspace, most recent first, each card the session's name with the
//! worktree under it and its pull request under that, and the session
//! under the cursor live in the PANE along the BOTTOM ([`split`]). Walking
//! the cards walks the pane, so stepping through the grid reads each
//! session's progress in turn.
//!
//! Nothing here is a second copy of the tree: the list's cursor IS the
//! panels' selection (`App::selected_session`), moved through the same
//! jump the `/` PALETTE uses, so every verb that reads the selection —
//! archive, delete, rename, the diff, the context menu — keeps working on
//! the row under the cursor. What lives here is what the view adds: the
//! rows ([`rows`]), where a launch from the box lands ([`target_for`]) and
//! the PROJECT PICKER behind `^P` ([`ProjectPicker`]). The keys are
//! `event_loop::launcher`'s and the drawing `ui::launcher_view`'s.

use crate::app::App;
use crate::pull_request::{Standing, Trouble};
use crate::quick_prompt::{QuickReturn, QuickTarget};
use crate::text_input::TextInput;
use nebula_core::{Agent, AgentId, AgentStatus, ProjectId, WorkspaceId, WorktreeId};
use ratatui::layout::Rect;

/// Which tier of the workspace tree the GRID is showing. The view is one
/// path down it — `workspaces / projects / sessions` — walked into with
/// Enter and back out with Esc (or `k`,`k` off the grid's top row). It
/// opens on [`Level::Sessions`], scoped to the selected project, and a
/// launch always lands back there.
///
/// Each level's cards are the same [`CARD_H`] tall, so the grid keeps one
/// rhythm however deep the view is walked and `j` always moves by a row.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Level {
    /// Every workspace this machine knows, each card naming its projects.
    Workspaces,
    /// The open workspace's projects, each card naming its sessions.
    Projects,
    /// The selected project's sessions — the view's home.
    #[default]
    Sessions,
}

impl Level {
    /// One level out — what Esc takes. None at the top of the tree.
    pub fn up(self) -> Option<Level> {
        match self {
            Level::Sessions => Some(Level::Projects),
            Level::Projects => Some(Level::Workspaces),
            Level::Workspaces => None,
        }
    }

    /// The word this level takes as the header's last crumb, and as the
    /// noun its count reads.
    pub fn crumb(self) -> &'static str {
        match self {
            Level::Workspaces => "workspaces",
            Level::Projects => "projects",
            Level::Sessions => "sessions",
        }
    }

    /// The same word for one of them, as a flash names it.
    pub fn singular(self) -> &'static str {
        match self {
            Level::Workspaces => "workspace",
            Level::Projects => "project",
            Level::Sessions => "session",
        }
    }
}

/// One session in the launcher's list.
#[derive(Debug, Clone)]
pub struct LauncherRow {
    pub agent: Agent,
    /// The PROJECT's display name.
    pub project: String,
    /// The checkout's branch — what the panels call the worktree.
    pub branch: String,
    /// The checkout is the project's ROOT WORKTREE (drawn with the `⌂`
    /// the WORKTREES PANEL gives it).
    pub is_main: bool,
    /// The pull request on that branch, when one is known.
    pub pr: Option<RowPr>,
}

/// What a row says about its pull request: the number and title, and the
/// standing and trouble that color it the way the PR rows in the panels
/// are colored (`pr_row::look`).
#[derive(Debug, Clone, PartialEq)]
pub struct RowPr {
    pub number: u64,
    pub title: String,
    pub url: String,
    pub standing: Standing,
    pub trouble: Option<Trouble>,
}

impl RowPr {
    /// The word the row's badge slot takes: the trouble while there is one
    /// (`conflicts`, `failing`), else the state (`ready`, `draft`,
    /// `merged`, `closed`) — the sidebar's words.
    pub fn badge(&self) -> &'static str {
        self.trouble.map_or(self.standing.badge(), |t| t.badge())
    }
}

/// Every session the list shows, most recently touched first: the
/// unarchived AGENTS of the open workspace's projects, ordered on the
/// SESSIONS panel's own `recency_key` so the grid reads the way that panel
/// reads — newest at the top left, along the row and wrapping — instead of
/// the creation order, which left a card that had sat for half an hour
/// above one that moved a minute ago. Working and blocked sessions count
/// as interacting *now*, so they hold the first row however long the turn
/// has taken (the `23m ago` on such a card is how long the turn has run,
/// not how stale it is). Among those, and among never-run sessions, which
/// all stamp 0, the newest by id — ULIDs sort by creation — comes first,
/// so ties never shuffle between frames. A launch is not one of those
/// ties, though: one carrying a task is created `running`, stamped as it
/// is created, so it leads the working rows on the stamp alone; one with
/// nothing to submit arrives `fresh`, and a `fresh` row loses to every
/// session mid-turn — so the session this client just launched leads the
/// list outright until its own first turn starts, which is when its stamp
/// takes over ([`crate::app::App::just_launched`]). A
/// session in a ROOT WORKTREE its project hides (**Hide root worktree**)
/// is left out, as the panels leave it out: the cursor cannot be put on
/// it.
///
/// The list is the SELECTED PROJECT's alone — the project this level was
/// walked into, and the one the header's crumb names. Scoping to one
/// project is what makes the level a level: every jump moves
/// `sel_project` with it, so the cursor only ever rests on a session the
/// list holds, and Esc is the way out to the projects beside it.
pub fn rows(app: &App) -> Vec<LauncherRow> {
    let scope = app.selected_project().map(|p| p.id.clone());
    let mut rows: Vec<LauncherRow> = app
        .tree
        .agents
        .iter()
        .filter_map(|agent| row_of(app, agent, scope.as_ref()))
        .collect();
    let now = crate::app::now_ms();
    rows.sort_by(|a, b| {
        crate::app::recency_key(&a.agent, now)
            .cmp(&crate::app::recency_key(&b.agent, now))
            .then_with(|| b.agent.id.cmp(&a.agent.id))
    });
    // A stable pass over the top of it, so the launch just fired is the
    // top left card from the moment its row arrives — see
    // `App::just_launched`.
    if let Some(id) = &app.just_launched {
        rows.sort_by_key(|r| &r.agent.id != id);
    }
    rows
}

/// The list's row for session `id`, when it has one — what the pane's
/// header reads for the session it shows, without building the list.
pub fn row(app: &App, id: &AgentId) -> Option<LauncherRow> {
    // Unscoped: this reads one named session — the one already on screen
    // full-screen — not the level's list, and it must not go blank
    // because the project cursor has moved off it.
    row_of(app, app.tree.agents.iter().find(|a| &a.id == id)?, None)
}

/// `agent`'s row: its project and checkout looked up, or None for one the
/// list leaves out — archived, outside `scope`, another workspace's, in a
/// hidden root. `scope` is the project the SESSIONS level is showing;
/// None reads the row whatever project it is in.
fn row_of(app: &App, agent: &Agent, scope: Option<&ProjectId>) -> Option<LauncherRow> {
    if agent.archived {
        return None;
    }
    let worktree = app
        .tree
        .worktrees
        .iter()
        .find(|w| w.id == agent.worktree_id)?;
    let project = app
        .tree
        .projects
        .iter()
        .find(|p| p.id == worktree.project_id)?;
    if scope.is_some_and(|id| id != &project.id) {
        return None;
    }
    if !app.tree.in_active_workspace(project) || (worktree.is_main && app.root_hidden(project)) {
        return None;
    }
    Some(LauncherRow {
        agent: agent.clone(),
        project: project.name.clone(),
        branch: worktree.branch.clone(),
        is_main: worktree.is_main,
        pr: row_pr(app, &worktree.id, &project.id, &worktree.branch),
    })
}

/// The pull request a checkout is on: what `gh pr view` said about its
/// branch (`App::pull_requests`, kept warm for these rows by the sweep —
/// see `event_loop::sweep_target`), else the project's OPEN PRS list's
/// row on the same head branch, which the selected project has before
/// its own lookup lands.
fn row_pr(app: &App, worktree: &WorktreeId, project: &ProjectId, branch: &str) -> Option<RowPr> {
    if let Some(Some(pr)) = app.pull_requests.get(worktree) {
        return Some(RowPr {
            number: pr.number,
            title: pr.title.clone(),
            url: pr.url.clone(),
            standing: pr.standing(),
            trouble: pr.trouble(),
        });
    }
    let listed = app.open_prs.get(project)?;
    let pr = listed.list.iter().find(|pr| pr.head == branch)?;
    Some(RowPr {
        number: pr.number,
        title: pr.title.clone(),
        url: pr.url.clone(),
        standing: pr.standing(),
        trouble: pr.trouble(),
    })
}

/// The row the cursor is on: the selected session, when it is one of the
/// list's. None while the selection rests on a terminal, a pull request,
/// an archived session or nothing.
pub fn cursor(app: &App, rows: &[LauncherRow]) -> Option<usize> {
    let selected = app.selected_session()?;
    rows.iter().position(|row| row.agent.id == selected.id)
}

// ---- the GRID ----

/// Narrowest a card is still worth drawing: the dot, a few words of name
/// and an ago label. The column count is chosen so no card goes under it.
pub const CARD_MIN_W: u16 = 34;
/// Most cards one row holds, however wide the terminal is: past four the
/// eye stops reading a row as a row, and each card loses the width its
/// name needs.
pub const MAX_COLS: usize = 4;
/// How many rows of a card the last prompt gets: enough that a sentence
/// reads as a sentence instead of being clipped at the card's edge.
pub const PROMPT_LINES: usize = 3;
/// The rows above the prompt: the name, where it runs, its pull request.
pub const CARD_HEAD_H: u16 = 3;
/// A card's text rows — name, place, pull request, then the last prompt
/// wrapped over [`PROMPT_LINES`] — inside its border.
pub const CARD_TEXT_H: u16 = CARD_HEAD_H + PROMPT_LINES as u16;
pub const CARD_H: u16 = CARD_TEXT_H + 2;
/// Gaps between cards: a column of air either side, a blank row under.
pub const GAP_X: u16 = 2;
pub const GAP_Y: u16 = 1;
/// The margin the grid keeps off the body's edges.
pub const PAD_X: u16 = 2;
/// Rows above the grid: the breadcrumb header and a blank under it.
pub const HEAD_H: u16 = 3;
/// Shortest the PANE under the grid is worth drawing: its own three header
/// rows and enough of the PTY under them that a reply reads as a reply.
pub const PANE_MIN_H: u16 = 12;
/// Share of the body the pane takes once there is room for more than its
/// minimum — a third, so the cards keep the screen and the pane keeps
/// enough of it to follow what the session is saying.
const PANE_SHARE: u16 = 3;

/// How tall the PANE stands in `body`: `want` — the height its top edge
/// was last dragged to — or the default third when it has never been
/// dragged, held to [`PANE_MIN_H`] at the bottom and to what the header
/// and one row of cards need at the top, so a drag to either end rests
/// against that stop instead of folding one side away. None for a body
/// with no room for both.
///
/// Every frame runs the remembered height back through here, so one kept
/// from a taller window — or restored from the UI-state blob — is capped
/// by the screen actually in front of the user rather than squeezing the
/// cards out.
pub fn pane_height(body: Rect, want: Option<u16>) -> Option<u16> {
    let keep = HEAD_H + CARD_H;
    if body.height < keep + PANE_MIN_H {
        return None;
    }
    Some(
        want.unwrap_or(body.height / PANE_SHARE)
            .max(PANE_MIN_H)
            .min(body.height - keep),
    )
}

/// The body in two: the view's own area — the breadcrumb header and the
/// GRID under it — and the PANE along the bottom that reads whichever card
/// the cursor is on, [`pane_height`] tall. None for a body too short to
/// hold the header, a row of cards and a pane worth the name: the grid
/// takes every row of it and a session is only ever seen full-screen there.
pub fn split(body: Rect, level: Level, want: Option<u16>) -> (Rect, Option<Rect>) {
    // Only the SESSIONS level has a session to read: a project or a
    // workspace card is not something the pane can show, and previewing
    // one would boot a PTY nobody is looking at. Those levels take the
    // whole body for their cards.
    if level != Level::Sessions {
        return (body, None);
    }
    let Some(pane_h) = pane_height(body, want) else {
        return (body, None);
    };
    let view = Rect {
        height: body.height - pane_h,
        ..body
    };
    let pane = Rect {
        y: view.y + view.height,
        height: pane_h,
        ..body
    };
    (view, Some(pane))
}

/// The grid as one frame draws it — the geometry the keys and the drawing
/// both read, so `j` moves by exactly the number of cards a row holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Grid {
    /// Where the cards go: the body under the header, inset by [`PAD_X`].
    pub area: Rect,
    /// Cards per row, 1 to [`MAX_COLS`].
    pub cols: usize,
    /// Width of one card, the leftover split evenly.
    pub card_w: u16,
    /// Rows of cards the area has room for, at least 1.
    pub rows_fit: usize,
}

/// The grid `body` lays out. A body too small for one card still reports
/// one column and one row: the cards clip rather than vanish, and the
/// cursor keeps moving.
pub fn grid(body: Rect) -> Grid {
    let area = Rect {
        x: body.x + PAD_X,
        y: body.y + HEAD_H,
        width: body.width.saturating_sub(PAD_X * 2),
        height: body.height.saturating_sub(HEAD_H),
    };
    let cols = usize::from((area.width + GAP_X) / (CARD_MIN_W + GAP_X)).clamp(1, MAX_COLS);
    let gaps = GAP_X * (cols as u16 - 1);
    let card_w = area.width.saturating_sub(gaps) / cols as u16;
    let rows_fit = usize::from((area.height + GAP_Y) / (CARD_H + GAP_Y)).max(1);
    Grid {
        area,
        cols,
        card_w,
        rows_fit,
    }
}

impl Grid {
    /// Where the card in `slot` goes, counting from the first card drawn
    /// (slot 0 is the top-left of the scrolled window, not of the list).
    pub fn cell(&self, slot: usize) -> Rect {
        let (row, col) = (slot / self.cols, slot % self.cols);
        Rect {
            x: self.area.x + col as u16 * (self.card_w + GAP_X),
            y: self.area.y + row as u16 * (CARD_H + GAP_Y),
            width: self.card_w,
            height: CARD_H,
        }
    }

    /// Cards the window holds at once.
    pub fn page(&self) -> usize {
        self.cols * self.rows_fit
    }
}

/// The card `dx` columns and `dy` rows from `at`, clamped to the grid.
///
/// Each step stays in its own axis: `h`/`l` walk the cursor's own row and
/// stop at its ends rather than wrapping onto the next, and `j`/`k` walk
/// the column and stop at the top and bottom rows rather than sliding
/// along one. A downward step into a short last row lands on its last
/// card, so `j` off the bottom of a column never falls through the grid.
/// From no cursor, a forward step takes the first card and a backward one
/// the last. None only for an empty grid.
pub fn grid_stepped(at: Option<usize>, dx: i64, dy: i64, cols: usize, len: usize) -> Option<usize> {
    if len == 0 {
        return None;
    }
    let cols = cols.max(1) as i64;
    let last = len as i64 - 1;
    let Some(at) = at else {
        return Some(if dx + dy >= 0 { 0 } else { len - 1 });
    };
    let (row, col) = (at as i64 / cols, at as i64 % cols);
    if dx != 0 {
        // The last card of this row — the row's own right-hand end, which
        // on a short last row is before the column count.
        let row_end = (last - row * cols).min(cols - 1);
        return Some((row * cols + (col + dx).clamp(0, row_end)) as usize);
    }
    let rows = last / cols;
    Some((((row + dy).clamp(0, rows) * cols) + col).min(last) as usize)
}

/// How many of `rows` are waiting on a human — the count the grid's
/// header puts in red, the same status the red dot marks.
pub fn needs_you(rows: &[LauncherRow]) -> usize {
    rows.iter()
        .filter(|row| row.agent.status == nebula_core::AgentStatus::NeedsFeedback)
        .count()
}

/// The last thing this session was asked to do — the newest of the
/// RECENT PROMPTS the daemon captures off the `UserPromptSubmit` hook,
/// already one line. None for a session that predates the capture, or one
/// whose only prompt nebula composed itself.
pub fn last_prompt(agent: &Agent) -> Option<&str> {
    agent
        .recent_prompts
        .last()
        .map(|p| p.text.as_str())
        .filter(|t| !t.is_empty())
}

/// The id of the session on row `index`, if the list has one there.
pub fn agent_at(app: &App, index: usize) -> Option<AgentId> {
    rows(app).get(index).map(|row| row.agent.id.clone())
}

// ---- the PROJECTS and WORKSPACES levels ----

/// Rows a PROJECT or WORKSPACE card gives to what is under it — the
/// sessions in the project, the projects in the workspace. One less than
/// a card's text rows, the first being its own name, so every level's
/// card is exactly [`CARD_H`] tall and the grid keeps one rhythm however
/// deep the view is walked.
pub const CARD_LIST: usize = CARD_TEXT_H as usize - 1;

/// One project as the PROJECTS level draws it.
#[derive(Debug, Clone)]
pub struct ProjectCard {
    pub id: ProjectId,
    pub name: String,
    /// Its unarchived sessions, the ones wanting a human first — the
    /// names the card lists under its own, and what its counts read.
    pub sessions: Vec<Agent>,
    /// How many of those are waiting on a human.
    pub needs_you: usize,
    /// The loudest status under it, the one its dot takes; None for a
    /// project with no session at all.
    pub status: Option<AgentStatus>,
    /// One of its sessions finished unread — the `done` blue its dot
    /// takes, as an unread session row takes it.
    pub unseen: bool,
    /// When it was last worked in: the ago badge, and the order two
    /// projects with the same standing come in.
    pub recency: crate::app::Recency,
}

/// One workspace as the WORKSPACES level draws it.
#[derive(Debug, Clone)]
pub struct WorkspaceCard {
    pub id: WorkspaceId,
    pub name: String,
    /// Its projects in the PROJECTS level's own order — the names the
    /// card lists under its own.
    pub projects: Vec<ProjectCard>,
    pub sessions: usize,
    pub needs_you: usize,
    pub status: Option<AgentStatus>,
    pub unseen: bool,
    pub recency: crate::app::Recency,
    /// This is the open workspace — the card the cursor is on, since the
    /// cursor here IS `Tree::active_workspace` (see [`workspace_cursor`]).
    pub open: bool,
}

/// How far up a card its status lifts it: the rollup's own priority, and
/// nothing at all for a card with no session under it.
fn attention(status: Option<AgentStatus>) -> u8 {
    status.map_or(0, crate::app::status_rank)
}

/// The PROJECTS level's cards: every project of the open workspace, the
/// ones with a session waiting on a human first, then the ones with one
/// running, then the rest most recently worked in first — the PROJECTS
/// PANEL's own order with whatever wants an answer lifted over it, so the
/// card to look at is the one the eye lands on top left.
pub fn project_cards(app: &App) -> Vec<ProjectCard> {
    cards_in(app, &app.tree.active_workspace)
}

/// The same over one named workspace — what a WORKSPACE card lists, and
/// what [`project_cards`] is for the open one.
fn cards_in(app: &App, workspace: &WorkspaceId) -> Vec<ProjectCard> {
    let now = crate::app::now_ms();
    let mut cards: Vec<ProjectCard> = app
        .tree
        .projects
        .iter()
        .filter(|p| &p.workspace_id == workspace)
        .map(|p| {
            let sessions = project_sessions(app, &p.id);
            ProjectCard {
                id: p.id.clone(),
                name: p.name.clone(),
                needs_you: sessions
                    .iter()
                    .filter(|a| a.status == AgentStatus::NeedsFeedback)
                    .count(),
                status: crate::app::rollup(sessions.iter().map(|a| a.status)),
                unseen: sessions.iter().any(|a| a.unseen),
                recency: crate::app::project_recency(&app.tree, &p.id, now),
                sessions,
            }
        })
        .collect();
    // Two stable passes: recency first, then the standing over it, so two
    // cards with the same standing keep the panel's order between them.
    // The raw stamp breaks the tie every project with a session mid-turn
    // shares, as it does in the panels (`crate::app::recency_key`).
    cards.sort_by_key(|c| {
        (
            std::cmp::Reverse(c.recency.interacted),
            std::cmp::Reverse(c.recency.stamped),
        )
    });
    cards.sort_by_key(|c| std::cmp::Reverse(attention(c.status)));
    cards
}

/// A project's sessions as its card lists them: the unarchived ones in
/// its checkouts, the ones wanting a human first, then the ones most
/// recently interacted with — the SESSIONS level's own `recency_key`, so
/// a card names the same session that level opens on. A session in a
/// hidden ROOT WORKTREE is left out, as that level leaves it out.
fn project_sessions(app: &App, project: &ProjectId) -> Vec<Agent> {
    let hide_root = app
        .tree
        .projects
        .iter()
        .find(|p| &p.id == project)
        .is_some_and(|p| app.root_hidden(p));
    // The project's checkouts once, not once per session: these levels
    // build every project's card on every frame, and a scan per agent
    // turned that into the tree squared.
    let checkouts: std::collections::HashSet<&WorktreeId> = app
        .tree
        .worktrees
        .iter()
        .filter(|w| &w.project_id == project && !(hide_root && w.is_main))
        .map(|w| &w.id)
        .collect();
    let mut out: Vec<Agent> = app
        .tree
        .agents
        .iter()
        .filter(|a| !a.archived && checkouts.contains(&a.worktree_id))
        .cloned()
        .collect();
    let now = crate::app::now_ms();
    out.sort_by(|a, b| {
        crate::app::recency_key(a, now)
            .cmp(&crate::app::recency_key(b, now))
            .then_with(|| b.id.cmp(&a.id))
    });
    out.sort_by_key(|a| std::cmp::Reverse(attention(Some(a.status))));
    out
}

/// The WORKSPACES level's cards: every workspace, in the tab order the
/// panels' bar gives them. Never reordered by what is running in them —
/// the cursor here IS the open workspace ([`workspace_cursor`]), so a
/// card that moved under it would take the cursor with it.
pub fn workspace_cards(app: &App) -> Vec<WorkspaceCard> {
    let now = crate::app::now_ms();
    app.tree
        .workspaces
        .iter()
        .map(|w| {
            let projects = cards_in(app, &w.id);
            WorkspaceCard {
                id: w.id.clone(),
                name: w.name.clone(),
                sessions: projects.iter().map(|p| p.sessions.len()).sum(),
                needs_you: projects.iter().map(|p| p.needs_you).sum(),
                status: crate::app::rollup(projects.iter().filter_map(|p| p.status)),
                unseen: projects.iter().any(|p| p.unseen),
                recency: crate::app::workspace_recency(&app.tree, &w.id, now),
                open: w.id == app.tree.active_workspace,
                projects,
            }
        })
        .collect()
}

/// Where the cursor is on the PROJECTS level: the selected project's
/// card. The cursor IS `App::sel_project` — the PROJECTS PANEL's own — so
/// the card under it, the grid it opens and every verb that reads the
/// selection all agree, exactly as the SESSIONS level's cursor is
/// `App::selected_session`. None while the selection rests on nothing.
pub fn project_cursor(app: &App, cards: &[ProjectCard]) -> Option<usize> {
    let selected = app.selected_project()?;
    cards.iter().position(|c| c.id == selected.id)
}

/// And on the WORKSPACES level: the open workspace's own tab index, since
/// the cards are in tab order and moving the cursor is opening one.
pub fn workspace_cursor(app: &App) -> Option<usize> {
    app.tree.active_workspace_index()
}

/// The project on card `index`, for the mouse.
pub fn project_at(app: &App, index: usize) -> Option<ProjectId> {
    project_cards(app).get(index).map(|c| c.id.clone())
}

/// The workspace on card `index`, for the mouse — tab order, so straight
/// off the tree.
pub fn workspace_at(app: &App, index: usize) -> Option<WorkspaceId> {
    app.tree.workspaces.get(index).map(|w| w.id.clone())
}

/// How many of `cards` have something waiting on a human — the count the
/// PROJECTS level's header puts in red, as [`needs_you`] is the SESSIONS
/// level's.
pub fn project_cards_needing_you(cards: &[ProjectCard]) -> usize {
    cards.iter().filter(|c| c.needs_you > 0).count()
}

/// The same for the WORKSPACES level.
pub fn workspace_cards_needing_you(cards: &[WorkspaceCard]) -> usize {
    cards.iter().filter(|c| c.needs_you > 0).count()
}

// ---- the header's STATUS TALLY ----

/// What the LAUNCHER VIEW's header counts in dots: the cards in front of
/// you, one apiece, under the loudest state each is in — so the counts can
/// never add up to more than the grid holds, and every card the eye can
/// find is in exactly one of them.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Tally {
    /// Waiting on a human: the red dot.
    pub needs_you: usize,
    /// Finished, and the finish still unread: the blue dot.
    pub done: usize,
    /// Mid-turn: the yellow dot.
    pub running: usize,
    /// Nothing live on it, and the pull request on its branch has landed:
    /// the purple dot.
    pub merged: usize,
}

/// Which dot a card counts under, or None for one at rest: the ladder every
/// rollup in nebula reads — needs-you over running over finished-unread —
/// with the merge under all of it, since a landed branch is a thing to file
/// away rather than a thing happening. A live session on a merged checkout
/// therefore counts as live, exactly as
/// [`crate::app::App::worktree_wears_merge`] lets one outrank the merge in
/// the WORKTREES panel.
fn counted(tally: &mut Tally, status: Option<AgentStatus>, unseen: bool, merged: bool) {
    match status {
        Some(AgentStatus::NeedsFeedback) => tally.needs_you += 1,
        Some(AgentStatus::Running) => tally.running += 1,
        Some(AgentStatus::Finished) if unseen => tally.done += 1,
        _ if merged => tally.merged += 1,
        _ => {}
    }
}

/// The SESSIONS level's tally, over the cards the grid holds: each card's
/// own status, and the standing of the pull request on its checkout.
pub fn session_tally(rows: &[LauncherRow]) -> Tally {
    let mut tally = Tally::default();
    for row in rows {
        let merged = row
            .pr
            .as_ref()
            .is_some_and(|pr| pr.standing == Standing::Merged);
        counted(&mut tally, Some(row.agent.status), row.agent.unseen, merged);
    }
    tally
}

/// The PROJECTS level's, over each card's rolled-up status — and over the
/// project's checkouts for the merge, which belongs to a branch rather than
/// to any session on it.
pub fn project_tally(app: &App, cards: &[ProjectCard]) -> Tally {
    let mut tally = Tally::default();
    for card in cards {
        counted(
            &mut tally,
            card.status,
            card.unseen,
            project_merged(app, &card.id),
        );
    }
    tally
}

/// The WORKSPACES level's, the same way one tier further out.
pub fn workspace_tally(app: &App, cards: &[WorkspaceCard]) -> Tally {
    let mut tally = Tally::default();
    for card in cards {
        let merged = card.projects.iter().any(|p| project_merged(app, &p.id));
        counted(&mut tally, card.status, card.unseen, merged);
    }
    tally
}

/// Whether any of `project`'s checkouts has landed — the purple a WORKTREES
/// row wears (`App::worktree_wears_merge`, which lets a live session on the
/// branch outrank the merge), rolled up to the card over it.
fn project_merged(app: &App, project: &ProjectId) -> bool {
    app.tree
        .worktrees
        .iter()
        .any(|w| &w.project_id == project && app.worktree_wears_merge(&w.id))
}

/// The checkout a launch into `project`'s existing work lands in — the
/// box with `^N` off: the one the panels would restore for it (the
/// selected worktree when the cursor is in that project, else the one it
/// was last left on), else its ROOT WORKTREE, else any checkout it has.
/// Never a stand-in git is still cutting, nor a root the project hides.
/// None for a project with no usable checkout.
pub fn checkout_for(app: &App, project: &ProjectId) -> Option<WorktreeId> {
    let p = app.tree.projects.iter().find(|p| &p.id == project)?;
    let hide_root = app.root_hidden(p);
    let usable = |id: &WorktreeId| {
        app.tree
            .worktrees
            .iter()
            .any(|w| &w.id == id && &w.project_id == project && !(hide_root && w.is_main))
            && !app.is_placeholder_worktree(id)
    };
    let selected = app
        .selected_project()
        .filter(|p| &p.id == project)
        .and_then(|_| app.selected_worktree())
        .map(|w| w.id.clone());
    let remembered = app.last_worktree_for_project.get(project).cloned();
    let mut checkouts = app
        .tree
        .worktrees
        .iter()
        .filter(|w| &w.project_id == project)
        .map(|w| (w.is_main, w.id.clone()))
        .collect::<Vec<_>>();
    // The root first, then the rest in tree order.
    checkouts.sort_by_key(|(is_main, _)| !is_main);
    selected
        .into_iter()
        .chain(remembered)
        .chain(checkouts.into_iter().map(|(_, id)| id))
        .find(|id| usable(id))
}

/// Where a launch from the box lands for `project`: a fresh worktree off
/// the project's default base (`new_worktree`, the view's default — a
/// session of its own per task), or the project's existing checkout
/// ([`checkout_for`]). A project with no checkout to reuse gets a fresh
/// one either way.
pub fn target_for(app: &App, project: &ProjectId, new_worktree: bool) -> QuickTarget {
    let fresh = || QuickTarget::NewWorktree {
        project: project.clone(),
        branch: crate::branch_name::random_name(&app.project_branches(project)),
    };
    if new_worktree {
        return fresh();
    }
    match checkout_for(app, project) {
        Some(worktree) => QuickTarget::Worktree(worktree),
        None => fresh(),
    }
}

/// The PROJECT a launch target is in.
pub fn project_of(app: &App, target: &QuickTarget) -> Option<ProjectId> {
    match target {
        QuickTarget::NewWorktree { project, .. } => Some(project.clone()),
        QuickTarget::Worktree(id) => app
            .tree
            .worktrees
            .iter()
            .find(|w| &w.id == id)
            .map(|w| w.project_id.clone()),
    }
}

/// Is a launch into `project` a BACKGROUND LAUNCH — one that lands
/// outside what the screen is showing? The SESSIONS level is one
/// project's, so a box re-aimed with `^P` starts its session in a list
/// nobody is looking at, and that is the point: a prompt fired into
/// another project while you keep working in this one. Such a launch
/// moves nothing here — not the cursor, not the open workspace, not the
/// pane — where a launch into the project under the cursor still lands
/// on its new session.
pub fn is_background(app: &App, project: &ProjectId) -> bool {
    app.launcher_active() && app.selected_project().is_some_and(|p| &p.id != project)
}

/// The PROJECT's display name, for the box's target row.
pub fn project_name(app: &App, project: &ProjectId) -> Option<String> {
    app.tree
        .projects
        .iter()
        .find(|p| &p.id == project)
        .map(|p| p.name.clone())
}

/// One project the PROJECT PICKER offers.
#[derive(Debug, Clone, PartialEq)]
pub struct PickerProject {
    pub id: ProjectId,
    pub name: String,
    pub workspace: WorkspaceId,
    /// The workspace's name when it is not the open one — drawn dim after
    /// the project's, since picking it switches this instance there.
    pub elsewhere: Option<String>,
    /// The repo path, `~/…` under the home directory, drawn dim to tell
    /// two same-named projects apart.
    pub path: String,
}

/// `path` with the home directory spelled `~`, as a shell prompt spells it.
fn home_relative(path: &std::path::Path) -> String {
    match nebula_core::env::home_dir()
        .and_then(|home| path.strip_prefix(home).ok().map(|rest| rest.to_path_buf()))
    {
        Some(rest) if rest.as_os_str().is_empty() => "~".into(),
        Some(rest) => format!("~/{}", rest.display()),
        None => path.display().to_string(),
    }
}

/// `^P` in the launcher's box: every project on this machine, filtered as
/// you type (fzf-style, `fuzzy::rank`) and picked with Enter, which puts
/// the box back on that project with the typed text kept. The open
/// workspace's projects come first, in the Projects panel's order (the
/// most recently worked in on top); the rest follow under their
/// workspace's name.
///
/// A pick only aims the box. The grid behind it goes on showing the
/// project being worked in, and a project from another workspace does not
/// open that workspace — the launch that follows is a BACKGROUND LAUNCH
/// ([`is_background`]), which starts the session over there and leaves
/// the screen here.
#[derive(Debug, Clone)]
pub struct ProjectPicker {
    /// The box to put back — with its text — on a pick or on Esc.
    pub back: QuickReturn,
    pub query: TextInput,
    pub projects: Vec<PickerProject>,
    /// Indices into `projects`, best match first, with the matched chars
    /// of the name for the highlight.
    pub matches: Vec<(usize, Vec<usize>)>,
    /// Cursor, into `matches`.
    pub selected: usize,
    /// Drawn rects, for the mouse.
    pub area: Rect,
    pub list_area: Rect,
}

impl ProjectPicker {
    /// The picker over every project in `app`'s tree, the cursor on the
    /// project the box is aimed at.
    pub fn new(app: &App, back: QuickReturn) -> Self {
        let current = project_of(app, &back.launch.target);
        let open = &app.tree.active_workspace;
        let mut projects: Vec<PickerProject> = app
            .project_rows()
            .into_iter()
            .filter_map(|i| app.tree.projects.get(i))
            .map(|p| PickerProject {
                id: p.id.clone(),
                name: p.name.clone(),
                workspace: p.workspace_id.clone(),
                elsewhere: None,
                path: home_relative(&p.repo_path),
            })
            .collect();
        // Then every other workspace's, in tab order.
        for workspace in app.tree.workspaces.iter().filter(|w| &w.id != open) {
            projects.extend(
                app.tree
                    .projects
                    .iter()
                    .filter(|p| p.workspace_id == workspace.id)
                    .map(|p| PickerProject {
                        id: p.id.clone(),
                        name: p.name.clone(),
                        workspace: p.workspace_id.clone(),
                        elsewhere: Some(workspace.name.clone()),
                        path: home_relative(&p.repo_path),
                    }),
            );
        }
        let mut picker = Self {
            back,
            query: TextInput::new(),
            projects,
            matches: Vec::new(),
            selected: 0,
            area: Rect::default(),
            list_area: Rect::default(),
        };
        picker.apply_filter();
        if let Some(current) = current {
            if let Some(i) = picker
                .matches
                .iter()
                .position(|(p, _)| picker.projects[*p].id == current)
            {
                picker.selected = i;
            }
        }
        picker
    }

    /// Re-rank against the query; the cursor goes back to the best match.
    /// An empty query lists every project in its own order.
    pub fn apply_filter(&mut self) {
        self.matches = crate::fuzzy::rank(
            self.query.as_str(),
            self.projects.iter().map(|p| p.name.as_str()),
        );
        self.selected = 0;
    }

    /// Move the cursor by `delta`, clamped.
    pub fn select(&mut self, delta: i64) {
        self.selected =
            crate::app::clamp_selection(self.selected as i64 + delta, self.matches.len());
    }

    /// The project under the cursor.
    pub fn selected_project(&self) -> Option<&PickerProject> {
        self.matches
            .get(self.selected)
            .and_then(|(i, _)| self.projects.get(*i))
    }

    /// First visible row of a list `height` rows tall.
    pub fn window_start(&self, height: usize) -> usize {
        crate::app::window_start(self.selected, height)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pull_request::PullRequest;
    use nebula_core::{AgentKind, AgentStatus, Project, Workspace, Worktree};

    fn project(id: &str, name: &str, workspace: &str) -> Project {
        Project {
            workspace_id: WorkspaceId(workspace.into()),
            id: ProjectId(id.into()),
            name: name.into(),
            repo_path: format!("/tmp/{name}").into(),
            sort_order: 0,
        }
    }

    fn worktree(id: &str, project: &str, branch: &str, is_main: bool) -> Worktree {
        Worktree {
            id: WorktreeId(id.into()),
            project_id: ProjectId(project.into()),
            path: format!("/tmp/{id}").into(),
            branch: branch.into(),
            is_main,
            sort_order: 0,
        }
    }

    fn agent(id: &str, worktree: &str, name: &str) -> Agent {
        Agent {
            id: AgentId(id.into()),
            worktree_id: WorktreeId(worktree.into()),
            name: name.into(),
            status: AgentStatus::Running,
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
            status_changed_at: 0,
            alive: true,
            recent_prompts: Vec::new(),
        }
    }

    /// Two projects in the default workspace, one in `side`: `api` with
    /// its root and a `feat` checkout, `web` with its root, `ops` over in
    /// the other workspace. Sessions a1 (api root), a2 (api feat), a3
    /// (web root), a4 (ops root), created in that order.
    fn app() -> App {
        let mut app = App::new();
        app.tree.workspaces = vec![
            Workspace {
                id: WorkspaceId::default(),
                name: "default".into(),
            },
            Workspace {
                id: WorkspaceId("side".into()),
                name: "side".into(),
            },
        ];
        app.tree.projects = vec![
            project("p1", "api", "default"),
            project("p2", "web", "default"),
            project("p3", "ops", "side"),
        ];
        app.tree.worktrees = vec![
            worktree("w1", "p1", "main", true),
            worktree("w2", "p1", "feat", false),
            worktree("w3", "p2", "main", true),
            worktree("w4", "p3", "main", true),
        ];
        app.tree.agents = vec![
            agent("a1", "w1", "fix-login"),
            agent("a2", "w2", "add-search"),
            agent("a3", "w3", "tidy-css"),
            agent("a4", "w4", "rotate-keys"),
        ];
        app
    }

    fn names(rows: &[LauncherRow]) -> Vec<&str> {
        rows.iter().map(|r| r.agent.name.as_str()).collect()
    }

    /// The list is the SELECTED PROJECT's sessions, each carrying the
    /// project and worktree its row names under it. The project beside it
    /// is not in the list — walking out to it is what Esc and the
    /// PROJECTS level are for — nor is the other workspace's session, nor
    /// an archived one. (The order is
    /// `rows_are_ordered_the_way_the_sessions_panel_orders_them`'s
    /// subject; here every session is working, so they tie and fall back
    /// to newest created.)
    #[test]
    fn rows_list_the_selected_projects_sessions() {
        let mut app = app();
        assert_eq!(
            app.selected_project().map(|p| p.name.as_str()),
            Some("api"),
            "the cursor opens on the first project"
        );
        let rows = super::rows(&app);
        assert_eq!(names(&rows), ["add-search", "fix-login"]);
        assert_eq!(
            (
                rows[0].project.as_str(),
                rows[0].branch.as_str(),
                rows[0].is_main
            ),
            ("api", "feat", false)
        );
        assert_eq!(
            (
                rows[1].project.as_str(),
                rows[1].branch.as_str(),
                rows[1].is_main
            ),
            ("api", "main", true)
        );

        // Walked into `web`, and the list is its one session instead —
        // the same cursor, a level's worth of scope away.
        app.sel_project = web_row(&app);
        assert_eq!(names(&super::rows(&app)), ["tidy-css"]);

        app.tree.agents[2].archived = true;
        assert!(
            names(&super::rows(&app)).is_empty(),
            "and an archived one is left out"
        );
    }

    /// `web`'s place in the PROJECTS PANEL's row order.
    fn web_row(app: &App) -> usize {
        app.project_rows()
            .iter()
            .position(|i| app.tree.projects[*i].id.0 == "p2")
            .expect("web has a row")
    }

    /// The order is the one the SESSIONS panel uses: a session that last
    /// moved a minute ago comes before one that has been sitting for half
    /// an hour, however long ago either was created. Working and blocked
    /// sessions count as moving now, so they head the grid — among those
    /// the raw stamp decides, newest turn first — and rows that are equally
    /// recent fall back to the newest by id, so a launch still lands top
    /// left.
    #[test]
    fn rows_are_ordered_the_way_the_sessions_panel_orders_them() {
        let mut app = app();
        // A third session in the selected project: the level lists one
        // project's, so the order needs three of them to have something
        // to say.
        app.tree.agents.push(agent("a5", "w2", "poll-ci"));
        let now = crate::app::now_ms();
        // Nothing is running: each session is only as recent as its stamp,
        // and the oldest-created one moved most recently.
        for a in &mut app.tree.agents {
            a.status = AgentStatus::Finished;
        }
        app.tree.agents[0].status_changed_at = now - 60_000; // fix-login: 1m
        app.tree.agents[1].status_changed_at = now - 1_800_000; // add-search: 30m
        app.tree.agents[4].status_changed_at = now - 600_000; // poll-ci: 10m
        assert_eq!(names(&rows(&app)), ["fix-login", "poll-ci", "add-search"]);

        // A turn starting in the stalest session puts it on top: it is
        // producing output as you look at it.
        app.tree.agents[1].status = AgentStatus::Running;
        assert_eq!(names(&rows(&app)), ["add-search", "fix-login", "poll-ci"]);

        // Every one working means every one interacting *now*, so the raw
        // stamp decides among them: the newest turn leads
        // (`crate::app::recency_key`).
        for a in &mut app.tree.agents {
            a.status = AgentStatus::Running;
        }
        assert_eq!(names(&rows(&app)), ["fix-login", "poll-ci", "add-search"]);

        // Rows that are *equally* recent — never run, all stamping 0 —
        // fall back to newest created, so the order never shuffles between
        // frames and a launch lands first.
        for a in &mut app.tree.agents {
            a.status = AgentStatus::Fresh;
            a.status_changed_at = 0;
        }
        assert_eq!(names(&rows(&app)), ["poll-ci", "add-search", "fix-login"]);
    }

    /// A session just launched is the top left card from the moment its
    /// row arrives — while it is still `fresh`, stamped a moment ago, and
    /// every other session is mid-turn and so counts as interacting now.
    #[test]
    fn a_just_launched_session_leads_the_grid() {
        let mut app = app();
        let now = crate::app::now_ms();
        for a in &mut app.tree.agents {
            a.status = AgentStatus::Running;
            a.status_changed_at = now - 600_000;
        }
        // The row as the DAEMON's create makes it: `fresh`, stamped now.
        app.tree.agents.push(agent("a5", "w2", "poll-ci"));
        let new_row = app.tree.agents.len() - 1;
        app.tree.agents[new_row].status = AgentStatus::Fresh;
        app.tree.agents[new_row].status_changed_at = now - 5;
        assert_eq!(
            names(&rows(&app)),
            ["add-search", "fix-login", "poll-ci"],
            "its own stamp puts it under every working session"
        );

        app.just_launched = Some(AgentId("a5".into()));
        assert_eq!(names(&rows(&app))[0], "poll-ci", "the launch leads");

        // Its first turn starts: the stamp holds it there on its own, so
        // the card does not move as the flag is dropped.
        app.just_launched = None;
        app.tree.agents[new_row].status = AgentStatus::Running;
        app.tree.agents[new_row].status_changed_at = now;
        assert_eq!(names(&rows(&app))[0], "poll-ci", "and keeps leading");
    }

    /// The cards fill the grid the way the rows are ordered: the first
    /// along the top row left to right, then wrapping onto the next — never
    /// down a column.
    #[test]
    fn the_grid_fills_along_the_row_before_it_wraps() {
        let g = grid(Rect::new(0, 0, 130, 40));
        assert_eq!(g.cols, 3);
        let (first, second, third, fourth) = (g.cell(0), g.cell(1), g.cell(2), g.cell(3));
        assert_eq!(
            (first.x, first.y),
            (g.area.x, g.area.y),
            "the first card is the top left"
        );
        assert!(second.x > first.x && second.y == first.y, "then rightwards");
        assert!(third.x > second.x && third.y == second.y);
        assert!(
            fourth.x == first.x && fourth.y > first.y,
            "and the row wraps back to the left: {fourth:?}"
        );
    }

    /// A project that hides its ROOT WORKTREE hides its sessions from the
    /// list too — the cursor could not land on them.
    #[test]
    fn a_hidden_root_keeps_its_sessions_off_the_list() {
        let mut app = app();
        app.project_fallback.hide_root_worktree = true;
        assert_eq!(names(&rows(&app)), ["add-search"]);
    }

    /// A row's pull request is what `gh pr view` said about its branch,
    /// else the project's OPEN PRS row on the same head branch; with
    /// neither, it has none.
    #[test]
    fn a_rows_pull_request_comes_from_its_branch() {
        let mut app = app();
        assert!(rows(&app).iter().all(|r| r.pr.is_none()));

        app.pull_requests.insert(
            WorktreeId("w2".into()),
            Some(PullRequest {
                number: 42,
                url: "https://github.com/o/api/pull/42".into(),
                title: "Add search".into(),
                state: crate::pull_request::STATE_MERGED.into(),
                is_draft: false,
                health: Default::default(),
                activity: Vec::new(),
            }),
        );
        let rows = rows(&app);
        let pr = rows[0].pr.as_ref().expect("a2's checkout has a PR");
        assert_eq!((pr.number, pr.standing), (42, Standing::Merged));
        assert_eq!(pr.badge(), "merged");

        let mut app = self::app();
        // The open list is `web`'s, so the level has to be in `web` to
        // hold a row that reads it.
        app.sel_project = web_row(&app);
        app.open_prs.insert(
            ProjectId("p2".into()),
            crate::app::OpenPrs {
                list: vec![crate::pull_request::OpenPr {
                    number: 7,
                    title: "Tidy".into(),
                    url: "https://github.com/o/web/pull/7".into(),
                    is_draft: true,
                    health: Default::default(),
                    head: "main".into(),
                }],
                at: std::time::Instant::now(),
                due: std::time::Instant::now(),
                step: std::time::Duration::from_secs(60),
            },
        );
        let rows = super::rows(&app);
        let pr = rows[0]
            .pr
            .as_ref()
            .expect("the open list names web's branch");
        assert_eq!((pr.number, pr.standing), (7, Standing::Draft));
    }

    /// A step from no cursor starts at an end; a step past an end stays.
    /// Each axis keeps to itself: `h`/`l` never leave their own row, and
    /// `j`/`k` never slide along one — except into a short last row,
    /// where `j` lands on its last card rather than falling through.
    #[test]
    fn grid_steps_clamp_per_axis_and_start_at_an_end() {
        // 3 columns over 7 cards: rows [0 1 2] [3 4 5] [6].
        let step = |at, dx, dy| grid_stepped(at, dx, dy, 3, 7);
        assert_eq!(step(None, 0, 1), Some(0));
        assert_eq!(step(None, 0, -1), Some(6));
        assert_eq!(grid_stepped(None, 0, 1, 3, 0), None);

        // Along a row, stopping at its ends.
        assert_eq!(step(Some(1), 1, 0), Some(2));
        assert_eq!(step(Some(2), 1, 0), Some(2), "no wrap onto the next row");
        assert_eq!(step(Some(3), -1, 0), Some(3), "nor back onto the last");
        assert_eq!(step(Some(6), 1, 0), Some(6), "a short row ends early");

        // Down a column, stopping at the top and bottom rows.
        assert_eq!(step(Some(1), 0, 1), Some(4));
        assert_eq!(step(Some(1), 0, -1), Some(1), "already on the top row");
        assert_eq!(step(Some(4), 0, 1), Some(6), "the short last row's end");
        assert_eq!(step(Some(6), 0, 1), Some(6), "already on the last row");
        assert_eq!(step(Some(4), 0, 5), Some(6), "a half page clamps");
        assert_eq!(step(Some(4), 0, -5), Some(1), "and so does one back");

        // One column is a plain list.
        assert_eq!(grid_stepped(Some(0), 0, 1, 1, 3), Some(1));
        assert_eq!(grid_stepped(Some(0), 1, 0, 1, 3), Some(0), "nowhere right");
    }

    /// The grid takes as many cards a row as fit at [`CARD_MIN_W`], never
    /// more than [`MAX_COLS`], and always at least one — a terminal too
    /// narrow for a card clips it rather than dropping the cursor.
    #[test]
    fn the_column_count_follows_the_width_and_stops_at_four() {
        let cols = |w| grid(Rect::new(0, 0, w, 40)).cols;
        assert_eq!(cols(20), 1, "narrower than one card");
        assert_eq!(cols(60), 1);
        assert_eq!(cols(100), 2);
        assert_eq!(cols(130), 3);
        assert_eq!(cols(200), 4);
        assert_eq!(cols(400), MAX_COLS, "and no more, however wide");

        // The cards share the width the gaps leave, and the last one ends
        // inside the grid.
        let g = grid(Rect::new(0, 0, 130, 40));
        let last = g.cell(g.cols - 1);
        assert!(
            last.x + last.width <= g.area.x + g.area.width,
            "{last:?} outside {:?}",
            g.area
        );
        assert!(g.card_w >= CARD_MIN_W, "{}", g.card_w);
    }

    /// A body with no room for a whole card still reports a row, so the
    /// cursor keeps moving and the card clips instead of vanishing.
    /// The PANE takes the bottom of the body and the header and cards keep
    /// the rest, with a row of cards still fitting over it; a body too
    /// short for both keeps every row for the grid.
    #[test]
    fn the_pane_takes_the_bottom_of_a_body_with_room_for_it() {
        let body = Rect::new(0, 0, 80, 40);
        let (view, pane) = split(body, Level::Sessions, None);
        let pane = pane.expect("40 rows has room for a pane");
        assert_eq!(view.height + pane.height, body.height, "the whole body");
        assert_eq!(pane.y, view.y + view.height, "the pane is under the grid");
        assert_eq!((pane.x, pane.width), (body.x, body.width), "full width");
        assert!(pane.height >= PANE_MIN_H, "and worth drawing: {pane:?}");
        assert!(grid(view).rows_fit >= 1, "a row of cards still fits");

        // Too short for a header, a row of cards and a pane worth the name:
        // all grid, and a session is only seen full-screen.
        let short = Rect::new(0, 0, 80, HEAD_H + CARD_H + PANE_MIN_H - 1);
        assert_eq!(split(short, Level::Sessions, None), (short, None));
    }

    #[test]
    fn a_tiny_body_still_has_one_cell() {
        let g = grid(Rect::new(0, 0, 10, 4));
        assert_eq!((g.cols, g.rows_fit), (1, 1));
        assert_eq!(g.page(), 1);
    }

    /// With `^N` off the box reuses the project's checkout: the one the
    /// cursor is on in that project, else the one it was last left on,
    /// else its root; a hidden root is never it, and a project with no
    /// usable checkout gets a fresh worktree after all.
    #[test]
    fn a_launch_into_existing_work_picks_the_projects_checkout() {
        let mut app = app();
        let api = ProjectId("p1".into());
        assert_eq!(
            app.selected_worktree().map(|w| w.id.0.as_str()),
            Some("w1"),
            "the cursor is on api's root"
        );
        app.last_worktree_for_project
            .insert(api.clone(), WorktreeId("w2".into()));
        assert_eq!(
            target_for(&app, &api, false),
            QuickTarget::Worktree(WorktreeId("w1".into())),
            "the checkout under the cursor wins"
        );
        // The cursor over in web: api's is the one it was last left on.
        app.sel_project = app
            .project_rows()
            .iter()
            .position(|i| app.tree.projects[*i].id.0 == "p2")
            .unwrap();
        assert_eq!(
            target_for(&app, &api, false),
            QuickTarget::Worktree(WorktreeId("w2".into()))
        );
        app.last_worktree_for_project.clear();
        assert_eq!(
            target_for(&app, &api, false),
            QuickTarget::Worktree(WorktreeId("w1".into())),
            "the root, with nothing remembered"
        );
        assert!(matches!(
            target_for(&app, &api, true),
            QuickTarget::NewWorktree { ref project, .. } if *project == api
        ));

        app.project_fallback.hide_root_worktree = true;
        let web = ProjectId("p2".into());
        assert!(
            matches!(
                target_for(&app, &web, false),
                QuickTarget::NewWorktree { .. }
            ),
            "web's only checkout is a hidden root"
        );
    }

    /// The levels walk one tier at a time and stop at either end: Esc off
    /// the sessions reaches the workspaces in two presses and no further,
    /// and Enter comes back the same way.
    #[test]
    fn the_levels_walk_one_tier_at_a_time() {
        assert_eq!(Level::Sessions.up(), Some(Level::Projects));
        assert_eq!(Level::Projects.up(), Some(Level::Workspaces));
        assert_eq!(Level::Workspaces.up(), None, "the top of the tree");

        assert_eq!(Level::default(), Level::Sessions, "the view's home");
        assert_eq!(Level::Projects.crumb(), "projects");
        assert_eq!(Level::Projects.singular(), "project");
    }

    /// The PROJECTS level puts what is waiting on a human first, then what
    /// is running, then the rest in the PROJECTS PANEL's own order — and
    /// each card's sessions are sorted the same way, so the name the card
    /// leads with is the one to look at.
    #[test]
    fn project_cards_put_what_wants_a_human_first() {
        let mut app = app();
        // Everything idle: the cards keep the panel's order, api first.
        for a in &mut app.tree.agents {
            a.status = AgentStatus::Finished;
        }
        let cards = project_cards(&app);
        assert_eq!(
            cards.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(),
            ["api", "web"],
            "the open workspace's projects, and not the other one's"
        );
        assert_eq!(cards[0].sessions.len(), 2, "api's two");
        assert_eq!(cards[0].needs_you, 0);

        // web's session blocks on a human and its card comes first, with
        // the dot and the count to match.
        app.tree.agents[2].status = AgentStatus::NeedsFeedback;
        let cards = project_cards(&app);
        assert_eq!(
            cards.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(),
            ["web", "api"]
        );
        assert_eq!(cards[0].needs_you, 1);
        assert_eq!(cards[0].status, Some(AgentStatus::NeedsFeedback));
        assert_eq!(project_cards_needing_you(&cards), 1);

        // Inside a card the same order holds: api's blocked session leads
        // its running one, whatever their stamps say.
        app.tree.agents[0].status = AgentStatus::NeedsFeedback; // fix-login
        app.tree.agents[1].status = AgentStatus::Running; // add-search
        let cards = project_cards(&app);
        let api = cards.iter().find(|c| c.name == "api").expect("api");
        assert_eq!(
            api.sessions
                .iter()
                .map(|a| a.name.as_str())
                .collect::<Vec<_>>(),
            ["fix-login", "add-search"]
        );

        // A project whose only checkout is a hidden root has no sessions
        // to name, as the SESSIONS level has none to list.
        app.project_fallback.hide_root_worktree = true;
        let cards = project_cards(&app);
        let web = cards.iter().find(|c| c.name == "web").expect("web");
        assert!(web.sessions.is_empty());
        assert_eq!(web.status, None, "and no dot to wear");
    }

    /// The WORKSPACES level stays in tab order however loud a workspace
    /// gets — the cursor there IS the open workspace, so a card that moved
    /// would take the cursor with it — and each card carries what is under
    /// it, its projects in the PROJECTS level's own order.
    #[test]
    fn workspace_cards_keep_tab_order_and_carry_their_projects() {
        let mut app = app();
        app.tree.agents[3].status = AgentStatus::NeedsFeedback; // ops, in `side`
        let cards = workspace_cards(&app);
        assert_eq!(
            cards.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(),
            ["default", "side"],
            "tab order, not attention order"
        );
        assert!(cards[0].open, "the cursor's card");
        assert_eq!(
            cards[0]
                .projects
                .iter()
                .map(|p| p.name.as_str())
                .collect::<Vec<_>>(),
            ["api", "web"]
        );
        assert_eq!(cards[0].sessions, 3, "api's two and web's one");
        assert_eq!(cards[1].needs_you, 1);
        assert_eq!(cards[1].status, Some(AgentStatus::NeedsFeedback));
        assert_eq!(workspace_cards_needing_you(&cards), 1);
        assert_eq!(workspace_at(&app, 1).map(|w| w.0), Some("side".into()));
    }

    /// The header's TALLY counts every card the grid holds exactly once,
    /// under the loudest state it is in: needs-you over running over an
    /// unread finish, and the merge only for a card with nothing live on
    /// it at all.
    #[test]
    fn a_tally_counts_each_card_once_under_its_loudest_state() {
        let mut app = app();
        // The SESSIONS level is `api`'s: its root session and its `feat`
        // one, which is the checkout with the merged pull request.
        app.pull_requests.insert(
            WorktreeId("w2".into()),
            Some(PullRequest {
                number: 42,
                url: "https://github.com/o/api/pull/42".into(),
                title: "Add search".into(),
                state: crate::pull_request::STATE_MERGED.into(),
                is_draft: false,
                health: Default::default(),
                activity: Vec::new(),
            }),
        );
        // Both api sessions are mid-turn (the fixture's default), so the
        // merge is outranked on its own checkout.
        assert_eq!(
            session_tally(&rows(&app)),
            Tally {
                running: 2,
                ..Tally::default()
            }
        );

        // The merged checkout's session finishes and is read: nothing is
        // live on it any more, so the card counts as landed rather than as
        // one more result filed away.
        app.tree.agents[1].status = AgentStatus::Finished;
        assert_eq!(
            session_tally(&rows(&app)),
            Tally {
                running: 1,
                merged: 1,
                ..Tally::default()
            }
        );

        // Unread, it is a finish first — a result nobody has looked at
        // outranks a branch that has landed.
        app.tree.agents[1].unseen = true;
        assert_eq!(
            session_tally(&rows(&app)),
            Tally {
                running: 1,
                done: 1,
                ..Tally::default()
            }
        );

        // And a question beats everything.
        app.tree.agents[1].unseen = false;
        app.tree.agents[1].status = AgentStatus::NeedsFeedback;
        assert_eq!(
            session_tally(&rows(&app)),
            Tally {
                running: 1,
                needs_you: 1,
                ..Tally::default()
            }
        );
    }

    /// The levels above the sessions tally their own cards: one dot per
    /// project, one per workspace, off the rollup each card already wears.
    #[test]
    fn the_levels_above_tally_their_cards() {
        let mut app = app();
        app.tree.agents[0].status = AgentStatus::NeedsFeedback; // api root
        app.tree.agents[2].status = AgentStatus::Finished; // web root
        app.tree.agents[2].unseen = true;

        let cards = project_cards(&app);
        assert_eq!(
            project_tally(&app, &cards),
            Tally {
                needs_you: 1,
                done: 1,
                ..Tally::default()
            },
            "api asks, web has an unread finish"
        );

        // The workspace over them wears the loudest of what it holds, and
        // `side`'s own project is still mid-turn.
        let cards = workspace_cards(&app);
        assert_eq!(
            workspace_tally(&app, &cards),
            Tally {
                needs_you: 1,
                running: 1,
                ..Tally::default()
            }
        );
    }

    /// Only the SESSIONS level has a pane: a project or a workspace card
    /// is not something the pane can read, and the cards take the body.
    #[test]
    fn the_levels_above_the_sessions_have_no_pane() {
        let body = Rect::new(0, 0, 80, 40);
        assert!(split(body, Level::Sessions, None).1.is_some());
        assert_eq!(split(body, Level::Projects, None), (body, None));
        assert_eq!(split(body, Level::Workspaces, None), (body, None));
    }

    /// The PANE stands at the height its edge was dragged to, held to its
    /// own minimum at one end and to the header plus a row of cards at the
    /// other — the two stops a drag rests against — and `split` lays that
    /// height out along the bottom of the body.
    #[test]
    fn a_dragged_pane_keeps_its_height_between_the_two_stops() {
        let body = Rect::new(0, 0, 80, 40);
        assert_eq!(
            pane_height(body, None),
            Some(body.height / PANE_SHARE),
            "never dragged: the default share"
        );
        assert_eq!(pane_height(body, Some(20)), Some(20), "as dragged");
        assert_eq!(
            pane_height(body, Some(1)),
            Some(PANE_MIN_H),
            "the pane's floor"
        );
        assert_eq!(
            pane_height(body, Some(99)),
            Some(body.height - (HEAD_H + CARD_H)),
            "the header and a row of cards are kept"
        );

        let (view, pane) = split(body, Level::Sessions, Some(20));
        let pane = pane.expect("40 rows has room for a pane");
        assert_eq!(pane.height, 20, "the dragged height, laid out");
        assert_eq!(view.height + pane.height, body.height, "the whole body");
        assert_eq!(pane.y, view.y + view.height, "the pane is under the grid");
        assert!(grid(view).rows_fit >= 1, "a row of cards still fits");

        // A body with no room for a pane has no height to drag it to.
        let short = Rect::new(0, 0, 80, HEAD_H + CARD_H + PANE_MIN_H - 1);
        assert_eq!(pane_height(short, Some(20)), None);
        assert_eq!(split(short, Level::Sessions, Some(20)), (short, None));
    }

    /// The picker lists the open workspace's projects, then the rest under
    /// their workspace's name; typing narrows by name, best match first,
    /// and the cursor starts on the project the box is aimed at.
    #[test]
    fn the_project_picker_lists_every_project_and_filters_by_name() {
        let app = app();
        let back = QuickReturn {
            launch: crate::quick_prompt::QuickLaunch::from_config(
                QuickTarget::Worktree(WorktreeId("w3".into())),
                &crate::config::Config::default(),
            ),
            text: "hello".into(),
            from_box: true,
        };
        let mut picker = ProjectPicker::new(&app, back);
        let listed: Vec<(&str, Option<&str>)> = picker
            .matches
            .iter()
            .map(|(i, _)| {
                let p = &picker.projects[*i];
                (p.name.as_str(), p.elsewhere.as_deref())
            })
            .collect();
        assert_eq!(listed.len(), 3);
        assert_eq!(listed[2], ("ops", Some("side")));
        assert_eq!(
            picker.selected_project().map(|p| p.name.as_str()),
            Some("web"),
            "the box's own project"
        );

        picker.query.insert_str("op");
        picker.apply_filter();
        assert_eq!(
            picker.selected_project().map(|p| p.name.as_str()),
            Some("ops")
        );
        picker.select(5);
        assert_eq!(picker.selected, picker.matches.len() - 1);
    }
}
