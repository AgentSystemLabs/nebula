//! The LAUNCHER VIEW (Settings → Experimental, `launcher_view`): nebula
//! built around the prompt instead of the tree. It opens on the QUICK
//! PROMPT, already focused, on the launch the AGENTS TAB defaults describe
//! — `^P` picks the PROJECT with type-ahead over every one this machine
//! knows, `^O` the MODEL, `Tab` the harness, `^N` flips between a fresh
//! worktree and the project's checkout — and once something has been sent
//! the three panels are gone: a GRID of cards, one per session in the
//! project on screen, most recent first, each card the session's name with the
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
use nebula_core::{Agent, AgentId, AgentStatus, ProjectId, WorktreeId};
use ratatui::layout::Rect;

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
/// unarchived AGENTS of the project on screen, ordered on the
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
/// The list is the SELECTED PROJECT's alone — the one whose PROJECT TAB is
/// lit in the header. Every jump moves `sel_project` with it, so the
/// cursor only ever rests on a session the list holds, and the tabs (or
/// the `+` in front of them) are the way to the projects beside it.
pub fn rows(app: &App) -> Vec<LauncherRow> {
    let scope = app.selected_project().map(|p| p.id.clone());
    // The ARCHIVED VIEW (`⇧A`) is the grid, swapped: the same cards for
    // the project's archived sessions instead of its live ones, so
    // `u` unarchives one where it stands and `⇧A` again comes back. The
    // two lists never mix — a grid of cards has no room for a group
    // header to fold, and an archived card answers to none of the keys a
    // live one does.
    let want_archived = app.show_archived;
    let mut rows: Vec<LauncherRow> = app
        .tree
        .agents
        .iter()
        .filter_map(|agent| row_of(app, agent, scope.as_ref(), Some(want_archived)))
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
    // full-screen — not the grid's list, and it must not go blank
    // because the project cursor has moved off it.
    row_of(
        app,
        app.tree.agents.iter().find(|a| &a.id == id)?,
        None,
        None,
    )
}

/// `agent`'s row: its project and checkout looked up, or None for one the
/// list leaves out — on the wrong side of `archived`, outside `scope`,
/// in a hidden root. `scope` is the project the grid is showing; None
/// reads the row whatever project it is in. `archived` is which of the two grids is on (`App::show_archived`);
/// None reads the row whether or not it has been archived, for the one
/// caller that names a session rather than listing a grid.
fn row_of(
    app: &App,
    agent: &Agent,
    scope: Option<&ProjectId>,
    archived: Option<bool>,
) -> Option<LauncherRow> {
    if archived.is_some_and(|want| agent.archived != want) {
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
    if worktree.is_main && app.root_hidden(project) {
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

/// The body in two: the view's own area — the PROJECT TABS and the GRID
/// under them — and the PANE along the bottom that reads whichever card
/// the cursor is on, [`pane_height`] tall. None for a body too short to
/// hold the header, a row of cards and a pane worth the name: the grid
/// takes every row of it and a session is only ever seen full-screen there.
///
/// This is the geometry alone. Whether a pane is wanted at all — the fold
/// (`^~`) and whether any card is wearing the cursor — is
/// [`crate::app::App::launcher_split`]'s, the one place both are read.
pub fn split(body: Rect, want: Option<u16>) -> (Rect, Option<Rect>) {
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

/// Where the PANE sits against the GRID: down the right side of the
/// cards — the default — or along the bottom, under them, where it sat
/// before there was a choice. Settings → Appearance → **Session pane**
/// (`session_pane`, read through `Config::pane_side`), or the SIDE BUTTON
/// on the pane's own TAB STRIP, which writes that same setting.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PaneSide {
    Bottom,
    #[default]
    Right,
}

impl PaneSide {
    /// The word `config.json` stores, and the value the settings row shows.
    pub const fn as_str(self) -> &'static str {
        match self {
            PaneSide::Bottom => "bottom",
            PaneSide::Right => "right",
        }
    }

    /// A stored word read back. Anything else — a typo in a hand edit, a
    /// side a newer build added, the `left` older builds offered — is the
    /// default, down the right.
    pub fn parse(word: &str) -> Self {
        match word.trim() {
            "bottom" => PaneSide::Bottom,
            _ => PaneSide::default(),
        }
    }

    /// The other side: where the pane's SIDE BUTTON moves it.
    pub const fn other(self) -> Self {
        match self {
            PaneSide::Bottom => PaneSide::Right,
            PaneSide::Right => PaneSide::Bottom,
        }
    }

    /// The pane stands beside the cards rather than under them: its edge
    /// runs down the body, and is dragged sideways.
    pub fn beside(self) -> bool {
        self != PaneSide::Bottom
    }

    /// The coordinate a drag of the pane's edge reads off the pointer: its
    /// row for an edge that runs across the body, its column for one that
    /// runs down it. [`pane_boundary`] is measured the same way.
    pub fn along(self, column: u16, row: u16) -> i32 {
        i32::from(if self.beside() { column } else { row })
    }
}

/// Narrowest the PANE beside the cards is worth drawing: room for an
/// agent's own screen to lay out without folding every line it prints.
pub const PANE_MIN_W: u16 = 40;
/// What the GRID keeps beside a pane: one card and the margins either
/// side of it, so a drag to that end rests against a column of cards
/// instead of folding them away.
const GRID_MIN_W: u16 = CARD_MIN_W + PAD_X * 2;

/// How wide the PANE stands beside the cards: `want` — the width its edge
/// was last dragged to — or half the body when it has never been dragged,
/// held to [`PANE_MIN_W`] at one end and to one column of cards at the
/// other, as [`pane_height`] holds a pane under them. None for a body
/// with no room for both side by side, or too short for the pane's own
/// header and a reply under it.
///
/// Half rather than [`pane_height`]'s third: a session read down the side
/// is read in columns, and an agent's screen squeezed to a third of the
/// width wraps every line it draws.
pub fn pane_width(body: Rect, want: Option<u16>) -> Option<u16> {
    if body.width < GRID_MIN_W + PANE_MIN_W || body.height < PANE_MIN_H {
        return None;
    }
    Some(
        want.unwrap_or(body.width / 2)
            .max(PANE_MIN_W)
            .min(body.width - GRID_MIN_W),
    )
}

/// The side the pane is laid out on in `body`: the one `side` asks for,
/// or the bottom when the body is too narrow to stand the pane beside a
/// column of cards — a pane under the grid beats no pane at all on a
/// narrow window, and the setting is picked up again once there is room.
pub fn fitted_side(body: Rect, side: PaneSide) -> PaneSide {
    if side.beside() && pane_width(body, None).is_none() {
        PaneSide::Bottom
    } else {
        side
    }
}

/// [`split`] for a pane on any side: `want` is the size the pane was last
/// dragged to along the axis `side` splits the body on — rows under the
/// cards, columns beside them. The side is taken as given; falling back
/// to the bottom on a narrow body is [`fitted_side`]'s.
pub fn split_at(body: Rect, side: PaneSide, want: Option<u16>) -> (Rect, Option<Rect>) {
    if side == PaneSide::Bottom {
        return split(body, want);
    }
    let Some(pane_w) = pane_width(body, want) else {
        return (body, None);
    };
    let grid_w = body.width - pane_w;
    let view = Rect {
        width: grid_w,
        ..body
    };
    let pane = Rect {
        x: body.x + grid_w,
        width: pane_w,
        ..body
    };
    (view, Some(pane))
}

/// Where the edge between the cards and `pane` sits, by the measure
/// [`PaneSide::along`] reads a pointer with: the pane's first row under
/// the cards, or its first column right of them.
pub fn pane_boundary(side: PaneSide, pane: Rect) -> i32 {
    match side {
        PaneSide::Bottom => i32::from(pane.y),
        PaneSide::Right => i32::from(pane.x),
    }
}

/// The pane's edge facing the cards, one cell deep: the blank row a pane
/// under them opens with, or the column a pane beside them keeps clear
/// on that side ([`pane_content`]). The GRIP is drawn along it.
pub fn pane_edge(side: PaneSide, pane: Rect) -> Rect {
    match side {
        PaneSide::Bottom => Rect {
            height: pane.height.min(1),
            ..pane
        },
        PaneSide::Right => Rect {
            width: pane.width.min(1),
            ..pane
        },
    }
}

/// What a press on the pane's edge is caught by: [`pane_edge`] and the
/// grid's row or column next to it, so the pointer has two cells to find
/// rather than one.
pub fn pane_grab_zone(side: PaneSide, pane: Rect) -> Rect {
    let edge = pane_edge(side, pane);
    match side {
        PaneSide::Bottom => Rect {
            y: edge.y.saturating_sub(1),
            height: 2,
            ..edge
        },
        PaneSide::Right => Rect {
            x: edge.x.saturating_sub(1),
            width: 2,
            ..edge
        },
    }
}

/// The part of the pane its header and the session's screen draw in: all
/// of it under the cards, whose frame opens on the blank row the grip
/// stands in; beside them, all but [`pane_edge`]'s column, so the screen
/// never runs under the grip.
pub fn pane_content(side: PaneSide, pane: Rect) -> Rect {
    match side {
        PaneSide::Bottom => pane,
        PaneSide::Right => Rect {
            x: pane.x + pane.width.min(1),
            width: pane.width.saturating_sub(1),
            ..pane
        },
    }
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

    /// The slice of `total` cards this window actually draws, with the
    /// cursor on `cursor`: where it starts, and how many cards follow it
    /// on screen. The count stops at the last WHOLE row the area has room
    /// for — a card is drawn entire or not at all — so a body too short
    /// for even one reports none rather than a clipped one.
    ///
    /// The grids and the header both read this, so the number the header
    /// says is hidden is exactly the number the grid left off.
    pub fn window(&self, cursor: Option<usize>, total: usize) -> (usize, usize) {
        let start =
            crate::app::window_start(cursor.unwrap_or(0) / self.cols, self.rows_fit) * self.cols;
        let bottom = self.area.y + self.area.height;
        let fits = (0..self.page())
            .take_while(|&slot| {
                let cell = self.cell(slot);
                cell.y + cell.height <= bottom
            })
            .count();
        (start, total.saturating_sub(start).min(fits))
    }

    /// How many of `total` cards the window leaves off screen, split by
    /// which way they went: scrolled off the top, or past the bottom edge.
    pub fn hidden(&self, cursor: Option<usize>, total: usize) -> Hidden {
        let (start, shown) = self.window(cursor, total);
        Hidden {
            above: start.min(total),
            below: total.saturating_sub(start + shown),
        }
    }
}

/// Cards a grid holds but does not draw — what the PANE along the bottom
/// took the room for, or what a screenful of sessions simply outruns.
/// Counted both ways round so the header can point at them: `above` are
/// scrolled off the top, `below` are past the bottom edge.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Hidden {
    pub above: usize,
    pub below: usize,
}

impl Hidden {
    /// Cards off screen either way. Zero when the grid holds the lot.
    pub fn total(self) -> usize {
        self.above + self.below
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

// ---- the PROJECT DROPDOWN's list ----

/// One project as the PROJECT DROPDOWN lists it.
#[derive(Debug, Clone)]
pub struct ProjectCard {
    pub id: ProjectId,
    pub name: String,
    /// Its unarchived sessions, the ones wanting a human first — what the
    /// row's count reads.
    pub sessions: Vec<Agent>,
    /// The loudest status under it; None for a project with no session.
    pub status: Option<AgentStatus>,
    /// When it was last worked in: the order two projects with the same
    /// standing come in.
    pub recency: crate::app::Recency,
}

/// How far up a card its status lifts it: the rollup's own priority, and
/// nothing at all for a card with no session under it.
fn attention(status: Option<AgentStatus>) -> u8 {
    status.map_or(0, crate::app::status_rank)
}

/// Every project on this machine — what the PROJECT DROPDOWN lists: the
/// ones with a session waiting on a human first, then the ones with one
/// running, then the rest most recently worked in first, so the project
/// to look at is the one the eye lands on at the top.
pub fn project_cards(app: &App) -> Vec<ProjectCard> {
    let now = crate::app::now_ms();
    let mut cards: Vec<ProjectCard> = app
        .tree
        .projects
        .iter()
        .map(|p| {
            let sessions = project_sessions(app, &p.id);
            ProjectCard {
                id: p.id.clone(),
                name: p.name.clone(),
                status: crate::app::rollup(sessions.iter().map(|a| a.status)),
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

/// A project's sessions as its tab and its dropdown row count them: the
/// unarchived ones in its checkouts, the ones wanting a human first, then
/// the ones most recently interacted with — the grid's own
/// `recency_key`, so the first is the session the grid opens on. A
/// session in a hidden ROOT WORKTREE is left out, as the grid leaves it
/// out.
fn project_sessions(app: &App, project: &ProjectId) -> Vec<Agent> {
    let hide_root = app
        .tree
        .projects
        .iter()
        .find(|p| &p.id == project)
        .is_some_and(|p| app.root_hidden(p));
    // The project's checkouts once, not once per session: the tabs count
    // every open project's sessions on every frame, and a scan per agent
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

// ---- the header's PROJECT TABS ----

/// What a PROJECT TAB counts in dots beside its name: that project's
/// sessions, each under its own status — waiting on a human, finished
/// unread, mid-turn. A session at rest counts nowhere, so a quiet project
/// is a bare name.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Tally {
    /// Waiting on a human: the red dot.
    pub needs_you: usize,
    /// Finished, and the finish still unread: the blue dot.
    pub done: usize,
    /// Mid-turn: the yellow dot.
    pub running: usize,
}

/// `project`'s tally, over the sessions its grid lists — the unarchived
/// ones, less any in a ROOT WORKTREE it hides — so a tab never counts a
/// session its own grid would not show.
pub fn project_tally(app: &App, project: &ProjectId) -> Tally {
    let mut tally = Tally::default();
    for a in project_sessions(app, project) {
        match a.status {
            AgentStatus::NeedsFeedback => tally.needs_you += 1,
            AgentStatus::Running => tally.running += 1,
            AgentStatus::Finished if a.unseen => tally.done += 1,
            _ => {}
        }
    }
    tally
}

/// One tab in the header's PROJECT TABS.
#[derive(Debug, Clone, PartialEq)]
pub struct ProjectTab {
    pub id: ProjectId,
    pub name: String,
    pub tally: Tally,
    /// The project the grid is on — the selected one.
    pub active: bool,
    /// The header's own cursor is on it: the PROJECT TABS have the keys
    /// ([`App::launcher_tab_cursor`]) and the grid is showing this one.
    pub focused: bool,
}

/// The PROJECT TABS across the LAUNCHER VIEW's header: every project
/// opened since it was last closed ([`App::launcher_tabs`]), the one last
/// worked in at the far left. Opening a project that has no tab yet puts
/// one there ([`App::settle_project_tabs`]), and so does working in one —
/// a session launched, a turn sent, a key typed at a session
/// ([`App::bring_tab_forward`]). Switching between tabs only looks and
/// moves none of them, so `[` / `]` and the header's cursor walk a row
/// that holds still under them.
///
/// A project gone from the tree drops out here, before the settle next
/// prunes it.
pub fn project_tabs(app: &App) -> Vec<ProjectTab> {
    let active = app.selected_project().map(|p| p.id.clone());
    app.launcher_tabs
        .iter()
        .filter_map(|id| {
            let p = app.tree.projects.iter().find(|p| &p.id == id)?;
            Some(ProjectTab {
                id: id.clone(),
                name: p.name.clone(),
                tally: project_tally(app, id),
                active: active.as_ref() == Some(id),
                focused: app.launcher_tab_cursor.as_ref() == Some(id),
            })
        })
        .collect()
}

/// The checkout `project`'s own menu runs and opens — the one the panels
/// would restore for it (the selected worktree when the cursor is in that
/// project, else the one it was last left on), else its ROOT WORKTREE,
/// else any checkout it has. Not where the box launches: that is the
/// root or a fresh worktree ([`target_for`]).
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
/// the project's default base (`new_worktree` — the
/// `quick_prompt_new_worktree` SETTING, or `^N` in the box), or the
/// project's ROOT BRANCH ([`root_checkout`]). Never the checkout of the
/// card under the cursor: more work in a session's own worktree is a
/// FOLLOW-UP (Space on its card), not a new session. A project with no
/// usable root — hidden, or still being cut — gets a fresh worktree
/// either way.
pub fn target_for(app: &App, project: &ProjectId, new_worktree: bool) -> QuickTarget {
    let root = (!new_worktree)
        .then(|| root_checkout(app, project))
        .flatten();
    match root {
        Some(worktree) => QuickTarget::Worktree(worktree),
        None => QuickTarget::NewWorktree {
            project: project.clone(),
            branch: crate::branch_name::random_name(&app.project_branches(project)),
        },
    }
}

/// `project`'s ROOT WORKTREE — the checkout its ROOT BRANCH lives in,
/// which is where a launch from the box lands unless it cuts a fresh
/// worktree ([`target_for`]). None for a project that hides its root,
/// whose root git is still cutting, or that has no root checkout of its
/// own.
pub fn root_checkout(app: &App, project: &ProjectId) -> Option<WorktreeId> {
    let p = app.tree.projects.iter().find(|p| &p.id == project)?;
    if app.root_hidden(p) {
        return None;
    }
    app.tree
        .worktrees
        .iter()
        .find(|w| &w.project_id == project && w.is_main && !app.is_placeholder_worktree(&w.id))
        .map(|w| w.id.clone())
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
/// outside what the screen is showing? The grid is one project's, so a box re-aimed with `^P` starts its session in a list
/// nobody is looking at, and that is the point: a prompt fired into
/// another project while you keep working in this one. Such a launch
/// moves nothing here — not the cursor, not the tabs, not the pane —
/// where a launch into the project under the cursor still lands
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

/// One of the four details on the view's box: where the session runs —
/// the project and the checkout in it — what runs there, on which model.
/// Each is drawn with the chord that changes it beside it, the checkout
/// with a `▾` (`ui::launcher_view::detail_line`), and each is a button — a
/// click on one opens the very picker its chord does, through the one
/// `event_loop::launcher::open_box_field` both ways in call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BoxField {
    /// `^P` — the PROJECT the launch is aimed at.
    Project,
    /// `▾` — the checkout in it the launch runs in: the WORKTREE PICKER,
    /// which only a click opens.
    Worktree,
    /// `Tab` — the harness that runs there.
    Agent,
    /// `^O` — that harness's MODEL, and its effort.
    Model,
}

/// One project the PROJECT PICKER offers.
#[derive(Debug, Clone, PartialEq)]
pub struct PickerProject {
    pub id: ProjectId,
    pub name: String,
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
/// the box back on that project with the typed text kept. The projects
/// come in the Projects panel's order, the most recently worked in on top.
///
/// A pick only aims the box. The grid behind it goes on showing the
/// project being worked in — the launch that follows is a BACKGROUND
/// LAUNCH ([`is_background`]), which starts the session over there and
/// leaves the screen here.
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
        let projects: Vec<PickerProject> = app
            .project_rows()
            .into_iter()
            .filter_map(|i| app.tree.projects.get(i))
            .map(|p| PickerProject {
                id: p.id.clone(),
                name: p.name.clone(),
                path: home_relative(&p.repo_path),
            })
            .collect();
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
    use nebula_core::{AgentKind, AgentStatus, Project, Worktree};

    fn project(id: &str, name: &str) -> Project {
        Project {
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
            issue_url: None,
            recent_prompts: Vec::new(),
        }
    }

    /// Three projects: `api` with its root and a `feat` checkout, `web`
    /// and `ops` with their roots. Sessions a1 (api root), a2 (api feat),
    /// a3 (web root), a4 (ops root), created in that order.
    fn app() -> App {
        let mut app = App::new();
        app.tree.projects = vec![
            project("p1", "api"),
            project("p2", "web"),
            project("p3", "ops"),
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
    /// project and worktree its row names under it. The projects beside it
    /// are not in the list — their tabs are how to get to them — nor is an
    /// archived one. (The order is
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

        // Switched to `web`, and the list is its one session instead —
        // the same cursor, one tab over.
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
        let (view, pane) = split(body, None);
        let pane = pane.expect("40 rows has room for a pane");
        assert_eq!(view.height + pane.height, body.height, "the whole body");
        assert_eq!(pane.y, view.y + view.height, "the pane is under the grid");
        assert_eq!((pane.x, pane.width), (body.x, body.width), "full width");
        assert!(pane.height >= PANE_MIN_H, "and worth drawing: {pane:?}");
        assert!(grid(view).rows_fit >= 1, "a row of cards still fits");

        // Too short for a header, a row of cards and a pane worth the name:
        // all grid, and a session is only seen full-screen.
        let short = Rect::new(0, 0, 80, HEAD_H + CARD_H + PANE_MIN_H - 1);
        assert_eq!(split(short, None), (short, None));
    }

    /// The window counts what it left off, and which way it went: cards
    /// past the bottom edge when the cursor is at the top, cards behind
    /// the cursor once it has walked down. A grid with room for the lot
    /// hides nothing — that is the case the header stays quiet for.
    #[test]
    fn the_window_counts_the_cards_it_could_not_draw() {
        // Two columns, two rows of cards on screen: four at a time.
        let body = Rect::new(0, 0, 100, HEAD_H + CARD_H * 2 + GAP_Y);
        let g = grid(body);
        assert_eq!((g.cols, g.rows_fit), (2, 2));

        assert_eq!(g.hidden(Some(0), 4), Hidden::default(), "the lot fits");
        assert_eq!(g.hidden(None, 4), Hidden::default(), "and with no cursor");
        assert_eq!(
            g.hidden(Some(0), 9),
            Hidden { above: 0, below: 5 },
            "from the top, the rest are under the fold"
        );
        // The cursor on the last card: the window has scrolled to it, so
        // what is missing is behind it rather than ahead. Nine cards over
        // two columns is five rows; the last two of them are on screen.
        assert_eq!(g.hidden(Some(8), 9), Hidden { above: 6, below: 0 });
        // And in the middle, both ways at once.
        assert_eq!(g.hidden(Some(5), 12), Hidden { above: 2, below: 6 });
        assert_eq!(g.hidden(Some(5), 12).total(), 8);

        // A body with room for one row of cards hides everything under it
        // — the PANE dragged up to its stop.
        let squeezed = grid(Rect::new(0, 0, 100, HEAD_H + CARD_H));
        assert_eq!(squeezed.rows_fit, 1);
        assert_eq!(squeezed.hidden(Some(0), 9), Hidden { above: 0, below: 7 });
    }

    #[test]
    fn a_tiny_body_still_has_one_cell() {
        let g = grid(Rect::new(0, 0, 10, 4));
        assert_eq!((g.cols, g.rows_fit), (1, 1));
        assert_eq!(g.page(), 1);
    }

    /// With `^N` off the box lands on the project's ROOT BRANCH, whatever
    /// card the cursor is on and whatever checkout the project was last
    /// left on; a hidden root is never it, and a project with no usable
    /// root gets a fresh worktree after all.
    #[test]
    fn a_launch_into_existing_work_lands_on_the_root_branch() {
        let mut app = app();
        let api = ProjectId("p1".into());
        let root = QuickTarget::Worktree(WorktreeId("w1".into()));
        app.last_worktree_for_project
            .insert(api.clone(), WorktreeId("w2".into()));
        assert_eq!(
            target_for(&app, &api, false),
            root,
            "not the remembered one"
        );
        // The cursor on api's other checkout: still the root.
        app.sel_worktree = app
            .worktree_rows()
            .iter()
            .position(|r| r.checkout().is_some_and(|w| w.id.0 == "w2"))
            .expect("api has a second checkout to park the cursor on");
        assert_eq!(app.selected_worktree().map(|w| w.id.0.as_str()), Some("w2"));
        assert_eq!(
            target_for(&app, &api, false),
            root,
            "not the checkout under the cursor"
        );
        assert!(matches!(
            target_for(&app, &api, true),
            QuickTarget::NewWorktree { ref project, .. } if *project == api
        ));

        app.project_fallback.hide_root_worktree = true;
        assert!(
            matches!(
                target_for(&app, &api, false),
                QuickTarget::NewWorktree { .. }
            ),
            "a hidden root is never the target"
        );
    }

    /// The PROJECT DROPDOWN's list puts what is waiting on a human first,
    /// then what is running, then the rest in the PROJECTS PANEL's own
    /// order — every project on the machine — and each project's sessions
    /// are sorted the same way, so the one it leads with is the one to
    /// look at.
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
            ["api", "web", "ops"],
            "every project"
        );
        assert_eq!(cards[0].sessions.len(), 2, "api's two");

        // web's session blocks on a human and its card comes first, with
        // the status to match.
        app.tree.agents[2].status = AgentStatus::NeedsFeedback;
        let cards = project_cards(&app);
        assert_eq!(
            cards.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(),
            ["web", "api", "ops"]
        );
        assert_eq!(cards[0].status, Some(AgentStatus::NeedsFeedback));

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
        // to count, as its grid has none to list.
        app.project_fallback.hide_root_worktree = true;
        let cards = project_cards(&app);
        let web = cards.iter().find(|c| c.name == "web").expect("web");
        assert!(web.sessions.is_empty());
        assert_eq!(web.status, None, "and no dot to wear");
    }

    /// A PROJECT TAB's tally counts every session its grid lists, each
    /// under its own status — asking, mid-turn, finished unread — and a
    /// session at rest, or one archived out of the grid, nowhere.
    #[test]
    fn a_project_tally_counts_each_session_under_its_status() {
        let mut app = app();
        let api = ProjectId("p1".into());
        // Both api sessions are mid-turn (the fixture's default).
        assert_eq!(
            project_tally(&app, &api),
            Tally {
                running: 2,
                ..Tally::default()
            }
        );

        app.tree.agents[1].status = AgentStatus::NeedsFeedback;
        app.tree.agents[0].status = AgentStatus::Finished;
        app.tree.agents[0].unseen = true;
        assert_eq!(
            project_tally(&app, &api),
            Tally {
                needs_you: 1,
                done: 1,
                running: 0,
            }
        );

        // Read, the finish is at rest; archived, the question is off the
        // grid — and so off the tab.
        app.tree.agents[0].unseen = false;
        app.tree.agents[1].archived = true;
        assert_eq!(project_tally(&app, &api), Tally::default());

        // Another project's sessions are its own tab's business.
        assert_eq!(
            project_tally(&app, &ProjectId("p2".into())),
            Tally {
                running: 1,
                ..Tally::default()
            }
        );
    }

    /// The tabs are the projects opened on the grid, the newest opened at
    /// the far left. Coming back to one already open moves nothing, and a
    /// project gone from the tree takes its tab with it.
    #[test]
    fn a_project_gets_a_tab_when_it_is_opened() {
        let mut app = app();
        let ids =
            |app: &App| -> Vec<String> { project_tabs(app).into_iter().map(|t| t.name).collect() };
        app.settle_project_tabs();
        assert_eq!(ids(&app), ["api"], "the project the view opened on");

        app.sel_project = web_row(&app);
        app.settle_project_tabs();
        assert_eq!(ids(&app), ["web", "api"], "the newest opened leads");

        // Back to `api`: it is already open, so nothing moves — only which
        // tab is lit.
        app.sel_project = app
            .project_rows()
            .iter()
            .position(|i| app.tree.projects[*i].id.0 == "p1")
            .expect("api has a row");
        app.settle_project_tabs();
        let tabs = project_tabs(&app);
        assert_eq!(ids(&app), ["web", "api"]);
        assert_eq!(
            tabs.iter().map(|t| t.active).collect::<Vec<_>>(),
            [false, true]
        );

        // Removed from the tree, `web` is removed from the header.
        app.tree.projects.retain(|p| p.id.0 != "p2");
        app.settle_project_tabs();
        assert_eq!(ids(&app), ["api"]);
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

        let (view, pane) = split(body, Some(20));
        let pane = pane.expect("40 rows has room for a pane");
        assert_eq!(pane.height, 20, "the dragged height, laid out");
        assert_eq!(view.height + pane.height, body.height, "the whole body");
        assert_eq!(pane.y, view.y + view.height, "the pane is under the grid");
        assert!(grid(view).rows_fit >= 1, "a row of cards still fits");

        // A body with no room for a pane has no height to drag it to.
        let short = Rect::new(0, 0, 80, HEAD_H + CARD_H + PANE_MIN_H - 1);
        assert_eq!(pane_height(short, Some(20)), None);
        assert_eq!(split(short, Some(20)), (short, None));
    }

    /// The **Session pane** words: both sides round-trip and are each
    /// other's other, and a word off the list — `left` included, which
    /// older builds offered — is the right side the pane has out of the
    /// box.
    #[test]
    fn pane_sides_read_back_and_default_to_the_right() {
        for side in [PaneSide::Bottom, PaneSide::Right] {
            assert_eq!(PaneSide::parse(side.as_str()), side);
            assert_eq!(side.other().other(), side);
            assert_ne!(side.other(), side);
        }
        assert_eq!(PaneSide::parse(" bottom "), PaneSide::Bottom);
        assert_eq!(PaneSide::parse("left"), PaneSide::Right);
        assert_eq!(PaneSide::parse("top"), PaneSide::Right);
        assert_eq!(PaneSide::parse(""), PaneSide::Right);
        assert_eq!(PaneSide::default(), PaneSide::Right);
    }

    /// A pane beside the cards splits the body's columns rather than its
    /// rows: half of them by default, all of the height, the grid on the
    /// left — and its edge, grip column and grab zone face the cards.
    #[test]
    fn a_side_pane_splits_the_columns() {
        let body = Rect::new(0, 3, 160, 40);

        let (view, pane) = split_at(body, PaneSide::Right, None);
        let pane = pane.expect("160 columns has room beside the cards");
        assert_eq!((pane.y, pane.height), (body.y, body.height), "full height");
        assert_eq!(pane.width, 80, "half the body by default");
        assert_eq!((view.x, view.width), (0, 80), "the grid on the left");
        assert_eq!(pane.x, view.x + view.width, "the pane right of it");
        assert_eq!(pane_boundary(PaneSide::Right, pane), 80);
        assert_eq!(pane_edge(PaneSide::Right, pane).x, 80);
        assert_eq!(
            pane_grab_zone(PaneSide::Right, pane),
            Rect::new(79, 3, 2, 40),
            "the grid's last column and the pane's first"
        );
        let content = pane_content(PaneSide::Right, pane);
        assert_eq!((content.x, content.width), (81, 79), "clear of the grip");

        let (view, pane) = split_at(body, PaneSide::Right, Some(50));
        let pane = pane.expect("room for a dragged width too");
        assert_eq!((pane.x, pane.width), (110, 50), "as dragged");
        assert_eq!((view.x, view.width), (0, 110), "the grid takes the rest");

        // The bottom is `split` itself, and its edge the pane's first row.
        let (_, pane) = split_at(body, PaneSide::Bottom, None);
        let pane = pane.expect("40 rows has room under the cards");
        assert_eq!(Some(pane), split(body, None).1);
        assert_eq!(pane_boundary(PaneSide::Bottom, pane), i32::from(pane.y));
        assert_eq!(pane_content(PaneSide::Bottom, pane), pane);
    }

    /// A side pane's width rests against its own minimum at one end and
    /// against one column of cards at the other; a body too narrow for
    /// both side by side lays the pane out along the bottom instead.
    #[test]
    fn a_side_pane_is_clamped_and_falls_back_to_the_bottom() {
        let body = Rect::new(0, 0, 160, 40);
        assert_eq!(pane_width(body, Some(1)), Some(PANE_MIN_W), "its floor");
        assert_eq!(
            pane_width(body, Some(999)),
            Some(160 - CARD_MIN_W - PAD_X * 2),
            "one column of cards kept"
        );
        assert_eq!(fitted_side(body, PaneSide::Right), PaneSide::Right);

        let narrow = Rect::new(0, 0, CARD_MIN_W + PAD_X * 2 + PANE_MIN_W - 1, 40);
        assert_eq!(pane_width(narrow, None), None);
        assert_eq!(fitted_side(narrow, PaneSide::Right), PaneSide::Bottom);
        assert_eq!(fitted_side(narrow, PaneSide::Bottom), PaneSide::Bottom);
    }

    /// The picker lists every project, each with its path; typing narrows
    /// by name, best match first, and the cursor starts on the project the
    /// box is aimed at.
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
        let listed: Vec<(&str, &str)> = picker
            .matches
            .iter()
            .map(|(i, _)| {
                let p = &picker.projects[*i];
                (p.name.as_str(), p.path.as_str())
            })
            .collect();
        assert_eq!(listed.len(), 3);
        assert_eq!(listed[2], ("ops", "/tmp/ops"));
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
