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
use nebula_core::{Agent, AgentId, AgentStatus, ProjectId, SessionRef, TerminalTab, WorktreeId};
use ratatui::layout::Rect;

/// One session in the launcher's list.
#[derive(Debug, Clone)]
pub struct LauncherRow {
    pub agent: Agent,
    /// The PROJECT's display name.
    pub project: String,
    /// The checkout's branch — what the panels call the worktree.
    pub branch: String,
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
/// takes over ([`crate::app::App::just_launched`]).
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
    Some(LauncherRow {
        agent: agent.clone(),
        project: project.name.clone(),
        branch: worktree.branch.clone(),
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

// ---- the BANDS ----

/// One card on the grid: a session, or a shell TERMINAL — the two things
/// that run in a checkout, drawn side by side in its BAND — a session
/// the grid's column wide, a terminal two ([`terminal_w`]), so the lines
/// its shell printed have room. A session card carries its whole row and a
/// terminal's only its tab: the difference in what they hold is what it
/// is, and the cards live for one frame, so boxing every session to even
/// the two out would buy nothing.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone)]
pub enum Card {
    Session(LauncherRow),
    Terminal(TerminalTab),
}

impl Card {
    /// The attachable session behind the card — what the PANE reads when
    /// the cursor is on it.
    pub fn sref(&self) -> SessionRef {
        match self {
            Card::Session(row) => SessionRef::Agent(row.agent.id.clone()),
            Card::Terminal(t) => SessionRef::Terminal(t.id.clone()),
        }
    }

    pub fn name(&self) -> &str {
        match self {
            Card::Session(row) => &row.agent.name,
            Card::Terminal(t) => &t.name,
        }
    }

    pub fn is_terminal(&self) -> bool {
        matches!(self, Card::Terminal(_))
    }
}

/// One worktree's BAND: the checkout — its branch, its uncommitted
/// changes, its pull request, all on the rule over the cards — and every
/// session and terminal running in it, sessions first.
#[derive(Debug, Clone)]
pub struct Band {
    pub worktree: WorktreeId,
    pub branch: String,
    /// The checkout is the project's ROOT WORKTREE (`⌂`).
    pub is_main: bool,
    pub pr: Option<RowPr>,
    /// The sessions in `rows` order, then the terminals in tree order.
    pub cards: Vec<Card>,
}

impl Band {
    pub fn sessions(&self) -> usize {
        self.cards.iter().filter(|c| !c.is_terminal()).count()
    }

    pub fn terminals(&self) -> usize {
        self.cards.iter().filter(|c| c.is_terminal()).count()
    }

    /// Where `sref`'s card sits in the band.
    pub fn position(&self, sref: &SessionRef) -> Option<usize> {
        self.cards.iter().position(|c| &c.sref() == sref)
    }
}

/// The grid's BANDS: one per checkout of the SELECTED PROJECT that has
/// something running in it — a session on the list `rows` builds, or a
/// terminal — the root first, then the rest most recently worked in
/// first (`App::visible_worktrees`). A checkout with nothing running has
/// no band: the grid is what is running, not what is checked out — unless
/// the **Show all worktrees** SETTING is on (`App::show_all_worktrees`),
/// when every checkout gets one, an empty band with no cards on it. The
/// ARCHIVED VIEW's bands hold the archived sessions alone; a terminal is
/// never archived, so none is listed there, and no empty band is either.
pub fn bands(app: &App) -> Vec<Band> {
    let Some(project) = app.selected_project() else {
        return Vec::new();
    };
    let rows = rows(app);
    let mut out = Vec::new();
    for w in app.visible_worktrees() {
        let mut cards: Vec<Card> = rows
            .iter()
            .filter(|r| r.agent.worktree_id == w.id)
            .cloned()
            .map(Card::Session)
            .collect();
        if !app.show_archived {
            cards.extend(
                app.tree
                    .terminals
                    .iter()
                    .filter(|t| t.worktree_id == w.id)
                    .cloned()
                    .map(Card::Terminal),
            );
        }
        if cards.is_empty() && (app.show_archived || !app.show_all_worktrees) {
            continue;
        }
        out.push(Band {
            worktree: w.id.clone(),
            branch: w.branch.clone(),
            is_main: w.is_main,
            pr: row_pr(app, &w.id, &project.id, &w.branch),
            cards,
        });
    }
    out
}

/// A card's place on the grid: which band, and which card along it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CardRef {
    pub band: usize,
    pub card: usize,
}

/// The card at `at`, when the grid has one there.
pub fn card_at(bands: &[Band], at: CardRef) -> Option<&Card> {
    bands.get(at.band)?.cards.get(at.card)
}

/// The band the cursor is on: the SELECTED WORKTREE's, when it has one.
/// None while the selection rests on a pull request, an issue, or a
/// checkout with nothing running.
pub fn band_cursor(app: &App, bands: &[Band]) -> Option<usize> {
    let w = app.selected_worktree()?;
    bands.iter().position(|b| b.worktree == w.id)
}

/// The card the cursor is on inside `band`: the selected row's session
/// or terminal, when it is one of the band's. None on a link row, an
/// archived session on the live grid, or nothing.
pub fn card_cursor(app: &App, band: &Band) -> Option<usize> {
    let sref = app.selected_session_row()?.sref()?;
    band.position(&sref)
}

/// The cursor as a card: the band it is on and, inside it, the card. At
/// the band level the card is the one the pane reads — the band's
/// remembered card — which is what a click or Enter lands on.
pub fn cursor(app: &App, bands: &[Band]) -> Option<CardRef> {
    let band = band_cursor(app, bands)?;
    let card = card_cursor(app, &bands[band])?;
    Some(CardRef { band, card })
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
pub const PROMPT_LINES: usize = 4;
/// The rows above the prompt: the name, and what it runs on. Where it
/// runs — the checkout, its changes, its pull request — is the BAND's
/// rule over the card, said once for every card in it.
pub const CARD_HEAD_H: u16 = 2;
/// A card's text rows — name, harness, then the last prompt wrapped over
/// [`PROMPT_LINES`] — inside its border.
pub const CARD_TEXT_H: u16 = CARD_HEAD_H + PROMPT_LINES as u16;
pub const CARD_H: u16 = CARD_TEXT_H + 2;
/// How many of the grid's columns a TERMINAL's card spans: two, gap
/// included, so the last lines its shell printed have room to read as
/// lines — a session card's width shows a prompt, a shell's output wants
/// twice that. A one-column grid gives it the one.
pub const TERM_SPAN: usize = 2;

/// How wide a TERMINAL's card is on a grid of `cols` columns `card_w`
/// wide: [`TERM_SPAN`] columns and the gaps between them.
pub fn terminal_w(cols: usize, card_w: u16) -> u16 {
    let span = TERM_SPAN.min(cols.max(1)) as u16;
    card_w * span + GAP_X * (span - 1)
}
/// The row over a BAND's cards: the checkout's rule.
pub const BAND_RULE_H: u16 = 1;
/// A BAND on the grid: its rule and one row of cards under it.
pub const BAND_H: u16 = BAND_RULE_H + CARD_H;
/// The row under a collapsed BAND's cards that says how many its row
/// left off (`▾ 2 more · Tab: see all 8`), on a band whose cards do not
/// all fit across ([`BandsLayout::row_overflows`]). Part of the band, so
/// the gap to the next band's rule stands under it rather than being
/// painted over.
pub const MORE_H: u16 = 1;
/// The compact LIST layout (Settings → Appearance → **Worktree layout**):
/// how many of a collapsed band's entries it shows — its most recent, since
/// the sessions come newest first ([`rows`]) — before the `▾ N more` row;
/// Tab (the ACCORDION) opens the rest.
pub const LIST_RECENT: usize = 3;
/// One entry of the LIST: a session or a terminal on a single line.
pub const LIST_ROW_H: u16 = 1;
/// The line under an EMPTY BAND's rule — a checkout with nothing running
/// in it, on the grid only with **Show all worktrees** on — saying what
/// can be done there, in place of a row of cards it has none of.
pub const EMPTY_BAND_ROW_H: u16 = 1;
/// An EMPTY BAND on the grid: its rule and the line under it.
pub const EMPTY_BAND_H: u16 = BAND_RULE_H + EMPTY_BAND_ROW_H;
/// Gaps between cards: a column of air either side, a blank row under.
pub const GAP_X: u16 = 2;
pub const GAP_Y: u16 = 1;
/// The margin the grid keeps off the body's edges.
pub const PAD_X: u16 = 2;
/// Rows above the grid: a blank, the PROJECT TABS, the rule under them,
/// and a row of air under that — so the first band's rule, or the
/// checkout's rule inside a worktree, never sits hard against the
/// header's.
pub const HEAD_H: u16 = 4;
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

/// Test-only: where a card slot lands, for the column tests.
#[cfg(test)]
impl Grid {
    pub fn cell(&self, slot: usize) -> Rect {
        let (row, col) = (slot / self.cols, slot % self.cols);
        Rect {
            x: self.area.x + col as u16 * (self.card_w + GAP_X),
            y: self.area.y + row as u16 * (CARD_H + GAP_Y),
            width: self.card_w,
            height: CARD_H,
        }
    }
}

impl Grid {}

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

// ---- the BANDS' layout ----

/// Where one card is drawn: its place on the grid and its cell.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Slot {
    pub at: CardRef,
    pub rect: Rect,
}

/// A collapsed BAND's row: its rule over one row of cards. The cards run
/// along the row; the ones past its edges are counted on the rule rather
/// than wrapped, so a step down is always a step onto the next checkout,
/// and the row scrolls under `h` / `l` to keep the cursor's card on it
/// ([`BandsLayout::strip_at`]). Where each band's row sits on the grid,
/// and how the grid scrolls, is [`PanelLayout`]'s.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BandsLayout {
    /// Where the bands go: the body under the header, inset by [`PAD_X`].
    pub area: Rect,
    /// Width of one session card — what [`grid`] would give a card in
    /// this area, so a band's cards are the size the open band's cards
    /// are.
    pub card_w: u16,
    /// Cards per row, as [`grid`] gives it: what a terminal's card spans
    /// two of ([`terminal_w`]).
    pub cols: usize,
}

/// The bands `body` lays out.
pub fn bands_layout(body: Rect) -> BandsLayout {
    let g = grid(body);
    BandsLayout {
        area: g.area,
        card_w: g.card_w,
        cols: g.cols,
    }
}

impl BandsLayout {
    /// How wide `card` is on this grid: a session's card one column, a
    /// terminal's two ([`terminal_w`]).
    pub fn card_width(&self, card: &Card) -> u16 {
        if card.is_terminal() {
            terminal_w(self.cols, self.card_w)
        } else {
            self.card_w
        }
    }

    /// `band`'s cards are wider together, gaps included, than the row:
    /// its STRIP leaves some off wherever the cursor is, and the band
    /// takes the [`MORE_H`] row that says so.
    pub fn row_overflows(&self, band: &Band) -> bool {
        let cards: u32 = band
            .cards
            .iter()
            .map(|c| u32::from(self.card_width(c)))
            .sum();
        let gaps = u32::from(GAP_X) * band.cards.len().saturating_sub(1) as u32;
        cards + gaps > u32::from(self.area.width)
    }

    /// The first card `band`'s row draws with the cursor on `cursor`: the
    /// stateless follow-window every list scrolls by
    /// (`app::window_start`) — the row slides only as far as it must to
    /// keep the cursor's card on it, as its last — counted in cards'
    /// widths rather than rows, since a terminal's card is two columns
    /// wide. With no cursor on the band the row starts at its first card.
    pub fn strip_start(&self, band: &Band, cursor: Option<usize>) -> usize {
        let Some(last) = band.cards.len().checked_sub(1) else {
            return 0;
        };
        let Some(cursor) = cursor else {
            return 0;
        };
        let cursor = cursor.min(last);
        let mut start = cursor;
        let mut used = self.card_width(&band.cards[cursor]);
        while start > 0 {
            let width = self.card_width(&band.cards[start - 1]);
            if used + GAP_X + width > self.area.width {
                break;
            }
            used += GAP_X + width;
            start -= 1;
        }
        start
    }

    /// The cards of `band` along `row` — its rule on the row's first
    /// line, wherever [`panel_layout`] put the band — with the cursor on
    /// `cursor`: from the first the follow-window keeps
    /// ([`BandsLayout::strip_start`]), left to right, as many as fit — a
    /// card is drawn whole or not at all — and how many the row left off
    /// at either end.
    pub fn strip_at(
        &self,
        row: Rect,
        band_index: usize,
        band: &Band,
        cursor: Option<usize>,
    ) -> Strip {
        let y = row.y + BAND_RULE_H;
        let right = row.x + row.width;
        let start = self.strip_start(band, cursor);
        let mut x = row.x;
        let mut cards = Vec::new();
        for (i, card) in band.cards.iter().enumerate().skip(start) {
            let width = self.card_width(card);
            if x + width > right {
                break;
            }
            cards.push(Slot {
                at: CardRef {
                    band: band_index,
                    card: i,
                },
                rect: Rect {
                    x,
                    y,
                    width,
                    height: CARD_H,
                },
            });
            x += width + GAP_X;
        }
        let after = band.cards.len() - start - cards.len();
        Strip {
            cards,
            before: start,
            after,
        }
    }
}

/// One band's row of cards as the grid draws it: the cards that fit, and
/// how many the row left off at either end — scrolled past its left edge
/// to keep the cursor's card on the row, or past its right edge for want
/// of room. Each count is what the `❮` / `❯` beside the row stands for
/// (`ui::launcher_view::draw_strip_arrows`), and `h` / `l` walk the
/// cursor onto them one card at a time (`event_loop::launcher::walk_band`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Strip {
    pub cards: Vec<Slot>,
    /// Cards before the first drawn.
    pub before: usize,
    /// Cards after the last drawn.
    pub after: usize,
}

impl Strip {
    /// Cards the row left off, either side: what the rule counts as
    /// `▸ n more`.
    pub fn hidden(&self) -> usize {
        self.before + self.after
    }
}

// ---- the PANEL: every band, at most one of them open ----

/// One band's place on the GRID: the row its rule stands on, counted
/// from the top of the whole panel rather than the screen, and — on the
/// one band open as the ACCORDION — its cards' own layout, the same one
/// the keys walk it with (`event_loop::launcher::step_grid`), so the
/// rows the grid draws are exactly the rows `j`/`k` step through.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PanelBand {
    pub rule_y: u16,
    /// The band's cards wrapped into rows under its rule, on the open
    /// band; None collapsed, where the STRIP draws its one row. In the
    /// LIST layout every band has one — its entries stacked a line apiece
    /// ([`list_layout`]) — and `open` says which is showing them all.
    pub content: Option<ExpandedLayout>,
    /// The band is the one open as the ACCORDION.
    pub open: bool,
    /// Rows the band takes, rule included: [`BAND_H`] collapsed — and
    /// the [`MORE_H`] row under the cards when they do not all fit — or
    /// its content's height open.
    pub height: u16,
}

impl PanelBand {
    /// `card`'s cell on the open band, in panel rows. None collapsed,
    /// where the cards are the STRIP's ([`BandsLayout::strip_at`]).
    pub fn cell(&self, card: usize) -> Option<Rect> {
        let local = self.content.as_ref()?.cell(card)?;
        Some(Rect {
            y: self.rule_y + local.y,
            ..local
        })
    }

    /// The open band's section rules in panel rows: the sessions' on the
    /// band's own rule, the terminals' where the terminals begin. None
    /// collapsed.
    pub fn rules(&self) -> impl Iterator<Item = (u16, &'static str)> + '_ {
        self.content
            .iter()
            .flat_map(|c| c.rules().iter().map(|&(y, label)| (self.rule_y + y, label)))
    }
}

/// The whole GRID laid out: every band's rule top to bottom, a collapsed
/// band's one row of cards under it, and the one band open as the
/// ACCORDION (`App::launcher_expanded`) every card of it wrapped into
/// rows — which pushes every band after it down, so the panel can run
/// taller than the screen. It scrolls as one list, by rows, the way a
/// terminal's screen scrolls through its history: a card or a band the
/// window's edge cuts is drawn cut ([`ui::launcher_view::draw_cut`]),
/// never left out, so the window is always full to its edges and no
/// card-sized hole opens where one a row too far up would have been.
/// Where it is scrolled to is the app's
/// ([`crate::app::App::launcher_scroll`]): the wheel moves it, a cursor
/// move pulls it just far enough to bring the cursor's card whole on
/// screen ([`PanelLayout::reveal`]), and the draw holds it within the
/// panel ([`PanelLayout::clamp`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PanelLayout {
    /// Where the bands go: the body under the header, inset by [`PAD_X`].
    pub area: Rect,
    pub bands: Vec<PanelBand>,
    /// Rows the whole panel takes.
    height: u16,
}

/// `bands` laid out in `body`, with `expanded`'s cards open under its
/// rule when it names one of them.
pub fn panel_layout(body: Rect, bands: &[Band], expanded: Option<&WorktreeId>) -> PanelLayout {
    let mut y = 0u16;
    let mut out = Vec::with_capacity(bands.len());
    let strips = bands_layout(body);
    for band in bands {
        let content = (expanded == Some(&band.worktree)).then(|| expanded_layout(body, band));
        let collapsed = if band.cards.is_empty() {
            EMPTY_BAND_H
        } else if strips.row_overflows(band) {
            BAND_H + MORE_H
        } else {
            BAND_H
        };
        let height = content.as_ref().map_or(collapsed, ExpandedLayout::height);
        out.push(PanelBand {
            rule_y: y,
            open: content.is_some(),
            content,
            height,
        });
        y += height + GAP_Y;
    }
    PanelLayout {
        area: grid(body).area,
        bands: out,
        height: y.saturating_sub(GAP_Y),
    }
}

/// [`panel_layout`] for the compact LIST: every band its rule over its
/// entries stacked a line apiece ([`list_layout`]) — all of them on the
/// band `expanded` names, the [`LIST_RECENT`] most recent on the rest, and
/// on the band the cursor is on (`pin`) its card too wherever it sits, so
/// the card the pane reads is always one of the lines on screen.
pub fn list_panel_layout(
    body: Rect,
    bands: &[Band],
    expanded: Option<&WorktreeId>,
    pin: Option<CardRef>,
) -> PanelLayout {
    let mut y = 0u16;
    let mut out = Vec::with_capacity(bands.len());
    for (index, band) in bands.iter().enumerate() {
        let open = expanded == Some(&band.worktree);
        let pin = pin.filter(|p| p.band == index).map(|p| p.card);
        let content = list_layout(body, band, open, pin);
        let height = content.height();
        out.push(PanelBand {
            rule_y: y,
            content: Some(content),
            open,
            height,
        });
        y += height + GAP_Y;
    }
    PanelLayout {
        area: grid(body).area,
        bands: out,
        height: y.saturating_sub(GAP_Y),
    }
}

impl PanelLayout {
    /// The band open as the ACCORDION, if one of these is.
    pub fn open(&self) -> Option<usize> {
        self.bands.iter().position(|b| b.open)
    }

    /// The panel is taller than its area: it scrolls, and a marker row
    /// stands under the window ([`BELOW_MARK_H`]).
    pub fn overflows(&self) -> bool {
        self.height > self.area.height
    }

    /// The rows of the area the panel scrolls through: all of it when it
    /// fits, else all but its last row, kept for the `↓ N more below`
    /// marker ([`BELOW_MARK_H`]) — so the marker never paints over a
    /// card's row, and a panel scrolled to its end leaves that row as
    /// air, as the header's row of air stands over the top.
    pub fn window(&self) -> Rect {
        if self.overflows() {
            Rect {
                height: self.area.height.saturating_sub(BELOW_MARK_H),
                ..self.area
            }
        } else {
            self.area
        }
    }

    /// The furthest the panel scrolls: the last row on the window's last
    /// row. Zero when it fits.
    pub fn max_scroll(&self) -> u16 {
        self.height.saturating_sub(self.window().height)
    }

    /// `scroll` held within the panel: one kept from a taller panel, or
    /// a taller window, comes back to the last one this has.
    pub fn clamp(&self, scroll: u16) -> u16 {
        scroll.min(self.max_scroll())
    }

    /// `scroll` moved just far enough that the cursor is whole on screen:
    /// on the open band, `card` with the row over it — the band's rule on
    /// a first row, the air between rows elsewhere — so a walk onto the
    /// first row brings the rule back too; on a collapsed band, or the
    /// open one with no card under the cursor, the whole band, rule and
    /// row. Something already whole moves nothing: a wheel that left the
    /// cursor's card on screen is left alone. Something taller than the
    /// window shows its top. Held within the panel.
    pub fn reveal(&self, scroll: u16, band: usize, card: Option<usize>) -> u16 {
        let pb = &self.bands[band];
        let (top, bottom) = match card.and_then(|c| pb.cell(c)) {
            Some(cell) => (cell.y.saturating_sub(BAND_RULE_H), cell.y + cell.height),
            None => (pb.rule_y, pb.rule_y + pb.height),
        };
        let rows = self.window().height;
        let mut scroll = scroll;
        if bottom > scroll + rows {
            scroll = bottom - rows;
        }
        if scroll > top {
            scroll = top;
        }
        self.clamp(scroll)
    }

    /// What the window's edges cut, either way — what the header's `↑↓ N
    /// hidden` and the edge markers count: a collapsed band not whole on
    /// screen is one, the open band's cards one each, and a thing drawn
    /// cut counts, since the rest of it is what the marker says there is
    /// more of. `above` has rows over the window's top edge, `below` rows
    /// past its bottom.
    pub fn hidden(&self, scroll: u16) -> Hidden {
        let bottom = scroll + self.window().height;
        let mut hidden = Hidden::default();
        let mut count = |top: u16, end: u16| {
            if top < scroll {
                hidden.above += 1;
            } else if end > bottom {
                hidden.below += 1;
            }
        };
        for band in &self.bands {
            match band.content.as_ref().filter(|_| band.open) {
                None => count(band.rule_y, band.rule_y + band.height),
                Some(content) => {
                    for cell in content.cells() {
                        let y = band.rule_y + cell.y;
                        count(y, y + cell.height);
                    }
                }
            }
        }
        hidden
    }
}

// ---- the open band's cards ----

/// The band open as the ACCORDION, as its cards lay out under its rule:
/// the sessions wrapped into rows — the band's own rule over the first,
/// where a collapsed band has its one row — then the terminals, each two
/// columns wide ([`terminal_w`]), wrapped into rows of their own under a
/// `terminals` rule that stands only once there is one, and `j`/`k`
/// walking the rows of both as one column — off the last row of sessions
/// is onto the first of terminals. Rows are counted from the band's rule
/// (row 0), not the screen: where the band sits on the grid, and how far
/// the grid is scrolled, is [`PanelLayout`]'s.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExpandedLayout {
    /// Every card's cell, by its index in the band — None for one a
    /// collapsed LIST leaves off ([`list_layout`]).
    cells: Vec<Option<Rect>>,
    /// Cards the collapsed LIST leaves off, counted on the `▾ N more` row
    /// under its last entry ([`ExpandedLayout::more_y`]). Always 0 on the
    /// open band and in the cards layout.
    pub more: usize,
    /// The rules over each section: their row and what they say. The
    /// sessions' is the band's own rule, on row 0; the terminals' stands
    /// only with a terminal under it.
    rules: Vec<(u16, &'static str)>,
    /// The cards on each visual row, top to bottom and left to right:
    /// what the cursor keys walk.
    pub rows: Vec<Vec<usize>>,
    /// Rows the whole layout takes, the band's rule included.
    height: u16,
}

/// What the two sections are called on their rules.
pub const SESSIONS_RULE: &str = "sessions";
pub const TERMINALS_RULE: &str = "terminals";

/// `band`'s cards laid out on the columns `body` gives the grid.
pub fn expanded_layout(body: Rect, band: &Band) -> ExpandedLayout {
    let g = grid(body);
    let mut cells = vec![None; band.cards.len()];
    let mut rules = Vec::new();
    let mut rows: Vec<Vec<usize>> = Vec::new();
    let mut y = 0u16;
    // One section at a time: the sessions on the grid's columns, then the
    // terminals on their wider ones (`terminal_w`) — fewer to a row, and
    // no row at all, rule included, when the checkout has none.
    for (terminal, label, cols, w) in [
        (false, SESSIONS_RULE, g.cols, g.card_w),
        (
            true,
            TERMINALS_RULE,
            (g.cols / TERM_SPAN).max(1),
            terminal_w(g.cols, g.card_w),
        ),
    ] {
        let slots: Vec<usize> = band
            .cards
            .iter()
            .enumerate()
            .filter(|(_, c)| c.is_terminal() == terminal)
            .map(|(i, _)| i)
            .collect();
        if slots.is_empty() {
            // The sessions' rule is the band's own, and heads the cards
            // even with no session under it. The terminals' says nothing
            // without a terminal to say it of.
            if !terminal {
                rules.push((y, label));
                y += BAND_RULE_H;
            }
            continue;
        }
        rules.push((y, label));
        y += BAND_RULE_H;
        for chunk in slots.chunks(cols.max(1)) {
            let mut row = Vec::new();
            for (col, i) in chunk.iter().enumerate() {
                cells[*i] = Some(Rect {
                    x: g.area.x + col as u16 * (w + GAP_X),
                    y,
                    width: w,
                    height: CARD_H,
                });
                row.push(*i);
            }
            rows.push(row);
            y += CARD_H + GAP_Y;
        }
    }
    ExpandedLayout {
        cells,
        more: 0,
        rules,
        rows,
        height: y.saturating_sub(GAP_Y),
    }
}

/// `band` as the compact LIST lays it out under its rule: one line per
/// card, the band's full width, in the band's own order — its sessions
/// newest first, then its terminals — one column `j`/`k` walk. `open`
/// lists every card; collapsed it lists the first [`LIST_RECENT`], then
/// `pin` (the cursor's card) when that is further down, and says how
/// many it left off on a row of its own under them (`more`).
pub fn list_layout(body: Rect, band: &Band, open: bool, pin: Option<usize>) -> ExpandedLayout {
    let area = grid(body).area;
    let total = band.cards.len();
    let mut shown: Vec<usize> = (0..if open { total } else { total.min(LIST_RECENT) }).collect();
    if let Some(p) = pin.filter(|&p| p < total && !shown.contains(&p)) {
        shown.push(p);
    }
    let mut cells = vec![None; total];
    let mut y = BAND_RULE_H;
    for &i in &shown {
        cells[i] = Some(Rect {
            x: area.x,
            y,
            width: area.width,
            height: LIST_ROW_H,
        });
        y += LIST_ROW_H;
    }
    let more = total - shown.len();
    if more > 0 {
        y += MORE_H;
    }
    if total == 0 {
        y += EMPTY_BAND_ROW_H;
    }
    ExpandedLayout {
        cells,
        more,
        rules: vec![(0, SESSIONS_RULE)],
        rows: shown.into_iter().map(|i| vec![i]).collect(),
        height: y,
    }
}

/// A card, or a rule, as the scrolled window lands it on screen: the
/// part of it inside the window, and how many of its rows the window's
/// edges cut off — none for one drawn whole.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Placed {
    /// The rows of it on screen, in screen coordinates.
    pub rect: Rect,
    /// Rows of it above the window's top edge.
    pub cut_top: u16,
    /// Rows of it past the window's bottom edge.
    pub cut_bottom: u16,
}

impl Placed {
    /// Every row of it is on screen.
    pub fn whole(&self) -> bool {
        self.cut_top == 0 && self.cut_bottom == 0
    }
}

/// The row kept under the window when the cards outrun it: where the
/// `↓ N more below` marker stands, the twin of the header's row of air
/// that the `↑` marker rides ([`HEAD_H`]).
pub const BELOW_MARK_H: u16 = 1;

/// Where `rect` — a card or a rule in panel rows, `y` counted from the
/// top of the panel rather than the screen — lands with the panel
/// scrolled by `scroll` rows through `window` ([`PanelLayout::window`]):
/// the rows of it inside the window, and what the window's edges cut
/// off. None when none of it is on screen.
pub fn place(window: Rect, scroll: u16, rect: Rect) -> Option<Placed> {
    let (top, bottom) = (scroll, scroll + window.height);
    let (y0, y1) = (rect.y.max(top), (rect.y + rect.height).min(bottom));
    (y1 > y0).then(|| Placed {
        rect: Rect {
            y: window.y + (y0 - top),
            height: y1 - y0,
            ..rect
        },
        cut_top: y0 - rect.y,
        cut_bottom: rect.y + rect.height - y1,
    })
}

impl ExpandedLayout {
    /// The row and column of `card` in the walk.
    pub fn row_of(&self, card: usize) -> Option<(usize, usize)> {
        self.rows
            .iter()
            .enumerate()
            .find_map(|(r, row)| row.iter().position(|&i| i == card).map(|c| (r, c)))
    }

    /// Rows the whole layout takes, the band's rule included.
    pub fn height(&self) -> u16 {
        self.height
    }

    /// `card`'s cell, `y` counted from the band's rule. None for one a
    /// collapsed LIST leaves off.
    pub fn cell(&self, card: usize) -> Option<Rect> {
        self.cells.get(card).copied().flatten()
    }

    /// Every card's cell there is, in the band's own card order.
    pub fn cells(&self) -> impl Iterator<Item = Rect> + '_ {
        self.cells.iter().flatten().copied()
    }

    /// The row of the collapsed LIST's `▾ N more` line, counted from the
    /// band's rule: under its last entry. None with nothing left off.
    pub fn more_y(&self) -> Option<u16> {
        (self.more > 0).then(|| self.height - MORE_H)
    }

    /// The section rules: each one's row, counted from the band's rule,
    /// and its word.
    pub fn rules(&self) -> &[(u16, &'static str)] {
        &self.rules
    }

    /// The card `dx` along its row and `dy` rows down from `at`, clamped
    /// as [`grid_stepped`] clamps: a step along a row stops at its ends,
    /// a step down the rows keeps the column and lands on a short row's
    /// last card. From no cursor a forward step takes the first card and
    /// a backward one the last. None only with no cards.
    pub fn stepped(&self, at: Option<usize>, dx: i64, dy: i64) -> Option<usize> {
        let last_row = self.rows.len().checked_sub(1)?;
        let Some(at) = at else {
            return if dx + dy >= 0 {
                self.rows.first()?.first().copied()
            } else {
                self.rows.last()?.last().copied()
            };
        };
        let (row, col) = self.row_of(at)?;
        if dx != 0 {
            let cells = &self.rows[row];
            let col = (col as i64 + dx).clamp(0, cells.len() as i64 - 1) as usize;
            return Some(cells[col]);
        }
        let row = (row as i64 + dy).clamp(0, last_row as i64) as usize;
        let cells = &self.rows[row];
        Some(cells[col.min(cells.len() - 1)])
    }

    /// Is `card` on the top row — where `k` walks up into the header?
    pub fn on_top_row(&self, card: Option<usize>) -> bool {
        match card {
            Some(i) => self.row_of(i).is_some_and(|(r, _)| r == 0),
            None => self.rows.is_empty(),
        }
    }
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
/// `recency_key`, so the first is the session the grid opens on.
fn project_sessions(app: &App, project: &ProjectId) -> Vec<Agent> {
    // The project's checkouts once, not once per session: the tabs count
    // every open project's sessions on every frame, and a scan per agent
    // turned that into the tree squared.
    let checkouts: std::collections::HashSet<&WorktreeId> = app
        .tree
        .worktrees
        .iter()
        .filter(|w| &w.project_id == project)
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
/// Never a stand-in git is still cutting. None for a project with no
/// usable checkout.
pub fn checkout_for(app: &App, project: &ProjectId) -> Option<WorktreeId> {
    let usable = |id: &WorktreeId| {
        app.tree
            .worktrees
            .iter()
            .any(|w| &w.id == id && &w.project_id == project)
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
/// `quick_prompt_new_worktree` SETTING, or `^N` in the box), or an
/// existing checkout ([`launch_checkout`]): the worktree under the grid's
/// cursor, else the project's ROOT BRANCH. A project with no usable
/// checkout — still being cut, or none at all — gets a fresh worktree
/// either way.
pub fn target_for(app: &App, project: &ProjectId, new_worktree: bool) -> QuickTarget {
    let existing = (!new_worktree)
        .then(|| launch_checkout(app, project))
        .flatten();
    match existing {
        Some(worktree) => QuickTarget::Worktree(worktree),
        None => QuickTarget::NewWorktree {
            project: project.clone(),
            branch: crate::branch_name::random_name(&app.project_branches(project)),
        },
    }
}

/// The existing checkout a launch from the box lands in for `project`:
/// the one under the grid's cursor when it is the project's own
/// ([`cursor_checkout`]) — a prompt sent with a worktree's band under
/// the cursor, or from inside the worktree, runs in that worktree, as
/// `t`'s terminal does — else the project's ROOT BRANCH
/// ([`root_checkout`]): the aim let go (Esc off the band), the cursor on
/// a pull request or an issue, or a box re-aimed at another project with
/// `^P`. A checkout still being cut is stepped over to the root. None
/// for a project with no usable checkout at all.
pub fn launch_checkout(app: &App, project: &ProjectId) -> Option<WorktreeId> {
    cursor_checkout(app)
        .filter(|id| {
            app.tree
                .worktrees
                .iter()
                .any(|w| &w.id == id && &w.project_id == project)
                && !app.is_placeholder_worktree(id)
        })
        .or_else(|| root_checkout(app, project))
}

/// The checkout the GRID's cursor is on: the band it rests on, or the
/// worktree the grid is inside — the SELECTED WORKTREE, while the aim is
/// on it ([`App::launcher_aimed`]) and it has a band to be on. A
/// selection resting on a checkout with nothing running has no row on
/// screen, so there is nothing under the cursor to read; nor with the
/// aim let go.
pub fn cursor_checkout(app: &App) -> Option<WorktreeId> {
    if !app.launcher_aimed() {
        return None;
    }
    let bands = bands(app);
    let band = band_cursor(app, &bands)?;
    Some(bands[band].worktree.clone())
}

/// `project`'s ROOT WORKTREE — the checkout its ROOT BRANCH lives in,
/// which is where a launch from the box lands with nothing under the
/// grid's cursor ([`launch_checkout`]). None for a project whose root git
/// is still cutting, or that has no root checkout of its own.
pub fn root_checkout(app: &App, project: &ProjectId) -> Option<WorktreeId> {
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

/// A fresh worktree for `launch` in `project`, on a branch nobody has
/// yet: named after the issue for an ISSUE SESSION, the random name `n`
/// would offer otherwise.
pub fn fresh_worktree(
    app: &App,
    project: ProjectId,
    launch: &crate::quick_prompt::QuickLaunch,
) -> QuickTarget {
    let taken = app.project_branches(&project);
    let branch = match &launch.issue {
        Some(issue) => crate::branch_name::issue_name(issue.number, &issue.title, &taken),
        None => crate::branch_name::random_name(&taken),
    };
    QuickTarget::NewWorktree { project, branch }
}

/// Where `launch` lands with its NEW WORKTREE toggle flipped: a fresh
/// worktree of the project it is aimed at, or — flipping off — an existing
/// checkout of it, the one under the grid's cursor else the ROOT BRANCH
/// ([`launch_checkout`]). The box's `^N` and the AGENT PRESETS list's
/// `Tab` both flip through here. A PR SESSION's checkout is the DAEMON's
/// to pick, so it has nothing to flip; the error says why for the footer.
pub fn flipped_target(
    app: &App,
    launch: &crate::quick_prompt::QuickLaunch,
) -> Result<QuickTarget, &'static str> {
    if launch.pr.is_some() {
        return Err("a PR session runs in the pull request's own checkout");
    }
    let project = project_of(app, &launch.target).ok_or("project no longer exists")?;
    if !launch.is_new_worktree() {
        return Ok(fresh_worktree(app, project, launch));
    }
    launch_checkout(app, &project)
        .map(QuickTarget::Worktree)
        .ok_or("no checkout to launch on — keeping the new worktree")
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
/// Each is drawn with the chord that changes it beside it
/// (`ui::launcher_view::detail_line`), and each is a button — a click on
/// one opens the very picker its chord does, through the one
/// `event_loop::launcher::open_box_field` both ways in call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BoxField {
    /// `^P` — the PROJECT the launch is aimed at.
    Project,
    /// `^T` — the checkout in it the launch runs in: the WORKTREE PICKER.
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

/// Test-only accessors: nothing in the app reads these any more.
#[cfg(test)]
impl PanelLayout {
    /// Rows the whole panel takes, scrolled or not.
    pub fn height(&self) -> u16 {
        self.height
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

    /// A checkout with nothing running in it has no band — until **Show
    /// all worktrees** is on, when it gets an EMPTY BAND: no cards, and
    /// only a rule and a line of height. The ARCHIVED VIEW never lists
    /// one, whatever the setting says.
    #[test]
    fn an_empty_checkout_gets_a_band_only_with_show_all_worktrees() {
        let mut app = app();
        app.tree.worktrees.push(worktree("w5", "p1", "idle", false));
        let branches = |app: &App| -> Vec<String> {
            super::bands(app).into_iter().map(|b| b.branch).collect()
        };
        assert!(
            !branches(&app).contains(&"idle".to_string()),
            "off: what runs"
        );

        app.show_all_worktrees = true;
        let bands = super::bands(&app);
        let idle = bands
            .iter()
            .position(|b| b.branch == "idle")
            .expect("on: every checkout");
        assert!(bands[idle].cards.is_empty());
        assert_eq!(branches(&app).len(), 3, "main, feat and idle");
        let body = Rect::new(0, 0, 120, 60);
        assert_eq!(
            super::panel_layout(body, &bands, None).bands[idle].height,
            EMPTY_BAND_H
        );
        assert_eq!(
            super::list_panel_layout(body, &bands, None, None).bands[idle].height,
            EMPTY_BAND_H
        );

        app.show_archived = true;
        assert!(
            !branches(&app).contains(&"idle".to_string()),
            "archived view"
        );
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
            (rows[0].project.as_str(), rows[0].branch.as_str()),
            ("api", "feat")
        );
        assert_eq!(
            (rows[1].project.as_str(), rows[1].branch.as_str()),
            ("api", "main")
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

    /// With `^N` off the box lands in the checkout under the grid's
    /// cursor — the band it is on, or the worktree it is inside — and,
    /// with the aim let go, on the project's ROOT BRANCH, whatever
    /// checkout the project was last left on; another project's box
    /// (`^P`) lands on that project's root, not this grid's cursor. With
    /// `^N` on it cuts a fresh worktree.
    #[test]
    fn a_launch_lands_in_the_cursors_checkout_or_on_the_root_branch() {
        let mut app = app();
        let api = ProjectId("p1".into());
        let root = QuickTarget::Worktree(WorktreeId("w1".into()));
        let feat = QuickTarget::Worktree(WorktreeId("w2".into()));
        app.last_worktree_for_project
            .insert(api.clone(), WorktreeId("w2".into()));
        assert_eq!(
            target_for(&app, &api, false),
            root,
            "the cursor opens on the root's band, not the remembered one"
        );
        // The cursor on api's other checkout: the box goes there.
        app.sel_worktree = app
            .worktree_rows()
            .iter()
            .position(|r| r.checkout().is_some_and(|w| w.id.0 == "w2"))
            .expect("api has a second checkout to park the cursor on");
        assert_eq!(app.selected_worktree().map(|w| w.id.0.as_str()), Some("w2"));
        assert_eq!(
            target_for(&app, &api, false),
            feat,
            "the checkout under the cursor"
        );
        // Collapsed or open, the band the cursor is on reads the same.
        app.launcher_expanded = None;
        assert_eq!(target_for(&app, &api, false), feat, "on the band");
        app.launcher_expanded = Some(WorktreeId("w2".into()));
        // Another project's box reads its own root, not this cursor.
        assert_eq!(
            target_for(&app, &ProjectId("p2".into()), false),
            QuickTarget::Worktree(WorktreeId("w3".into())),
            "web's root"
        );
        // The aim let go: nothing under the cursor, so the root.
        app.launcher_unaimed = true;
        assert_eq!(target_for(&app, &api, false), root, "unaimed");
        app.launcher_unaimed = false;
        assert!(matches!(
            target_for(&app, &api, true),
            QuickTarget::NewWorktree { ref project, .. } if *project == api
        ));
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

    fn terminal(id: &str, worktree: &str, name: &str) -> nebula_core::TerminalTab {
        nebula_core::TerminalTab {
            id: nebula_core::TerminalId(id.into()),
            worktree_id: WorktreeId(worktree.into()),
            name: name.into(),
            sort_order: 0,
            alive: true,
            run_command: None,
        }
    }

    /// A terminal's card spans two of the grid's columns, gap included —
    /// on a band's strip and on the open band alike — so the lines its
    /// shell printed have room to read; a one-column grid gives it the
    /// one. Open, the terminals wrap by rows of their own width, and the
    /// walk still runs down through them.
    #[test]
    fn terminal_cards_span_two_columns() {
        let mut app = app();
        app.tree.terminals = vec![
            terminal("t1", "w1", "shell-1"),
            terminal("t2", "w1", "shell-2"),
            terminal("t3", "w1", "shell-3"),
        ];
        let all = super::bands(&app);
        // Four columns: the session, then the first terminal beside it.
        let body = Rect::new(0, 0, 150, 60);
        let g = bands_layout(body);
        assert_eq!(g.cols, 4, "150 columns hold four cards");
        let wide = terminal_w(g.cols, g.card_w);
        assert_eq!(wide, g.card_w * 2 + GAP_X);
        let strip = g.strip_at(g.area, 0, &all[0], None);
        let (more, cards) = (strip.hidden(), strip.cards);
        assert_eq!(cards.len(), 2, "the session and one terminal fit");
        assert_eq!(more, 2, "the rest are counted on the rule");
        assert_eq!(cards[0].rect.width, g.card_w);
        assert_eq!(cards[1].rect.width, wide, "two columns wide");
        assert_eq!(cards[1].rect.x, cards[0].rect.x + g.card_w + GAP_X);

        let layout = expanded_layout(body, &all[0]);
        let cell = |i: usize| layout.cell(i).expect("the card's cell");
        let (session, first, second, third) = (cell(0), cell(1), cell(2), cell(3));
        assert_eq!(first.width, wide, "on the open band too");
        assert_eq!(first.x, session.x, "under the terminals rule, first column");
        assert_eq!(
            (second.x, second.y),
            (first.x + wide + GAP_X, first.y),
            "two to a row of four columns"
        );
        assert_eq!(
            (third.x, third.y),
            (first.x, first.y + CARD_H + GAP_Y),
            "the third wraps"
        );
        assert_eq!(layout.rows, vec![vec![0], vec![1, 2], vec![3]]);
        assert_eq!(
            layout.stepped(Some(0), 0, 1),
            Some(1),
            "j goes down into them"
        );

        // One column: a terminal's card is the column's width.
        let narrow = grid(Rect::new(0, 0, 50, 60));
        assert_eq!(narrow.cols, 1);
        assert_eq!(terminal_w(narrow.cols, narrow.card_w), narrow.card_w);
    }

    /// A band's row follows the cursor along it: it starts at the first
    /// card until the cursor's card would fall off the right edge, then
    /// slides just far enough to keep that card on the row as its last —
    /// the follow-window every list scrolls by — and counts what it left
    /// off either side. A terminal's card is two columns of it.
    #[test]
    fn the_bands_row_follows_the_cursor() {
        let mut app = app();
        app.tree.agents = (0..5)
            .map(|i| agent(&format!("s{i}"), "w1", &format!("session-{i}")))
            .collect();
        app.tree.terminals = vec![terminal("t1", "w1", "shell-1")];
        let all = super::bands(&app);
        assert_eq!(all[0].cards.len(), 6, "five sessions and the terminal");
        // 80 columns: two cards to a row, a terminal's card the whole row.
        let g = bands_layout(Rect::new(0, 0, 80, 60));
        assert_eq!(g.cols, 2);
        let at = |cursor: Option<usize>| {
            let s = g.strip_at(g.area, 0, &all[0], cursor);
            (
                s.cards.iter().map(|c| c.at.card).collect::<Vec<_>>(),
                s.before,
                s.after,
            )
        };
        assert_eq!(at(None), (vec![0, 1], 0, 4), "no cursor: from the first");
        assert_eq!(at(Some(0)), (vec![0, 1], 0, 4));
        assert_eq!(at(Some(1)), (vec![0, 1], 0, 4), "still on the row");
        assert_eq!(
            at(Some(2)),
            (vec![1, 2], 1, 3),
            "one step past the edge: the row slides one"
        );
        assert_eq!(at(Some(4)), (vec![3, 4], 3, 1));
        assert_eq!(
            at(Some(5)),
            (vec![5], 5, 0),
            "the terminal's card is the whole row: two columns wide"
        );
        // Wherever the window starts, the first card drawn sits in the
        // row's first column, and the rule counts both sides as hidden.
        let s = g.strip_at(g.area, 0, &all[0], Some(4));
        assert_eq!(s.cards[0].rect.x, g.area.x);
        assert_eq!(s.hidden(), 4);
    }

    /// The ACCORDION in the panel: a collapsed band is its rule and one
    /// row of cards, one after another down the grid; the open band takes
    /// the rows its cards wrap into, its first row right under its rule
    /// where a collapsed band's row is, and every band after it moves
    /// down by the difference. Past the window's edge a collapsed band
    /// counts as one thing, the open band's cards one each.
    #[test]
    fn the_open_band_pushes_the_bands_under_it_down() {
        let mut app = app();
        app.tree
            .agents
            .extend((0..5).map(|i| agent(&format!("s{i}"), "w1", &format!("session-{i}"))));
        let all = super::bands(&app);
        assert_eq!(all.len(), 2, "the root band and feat's");
        assert_eq!(all[0].cards.len(), 6);
        // 80 columns: two cards to a row.
        let body = Rect::new(0, 0, 80, 60);
        let collapsed = panel_layout(body, &all, None);
        assert_eq!(collapsed.open(), None);
        assert_eq!(
            collapsed.bands[0].height,
            BAND_H + MORE_H,
            "six cards on a two-card row: the row under them says so"
        );
        assert_eq!(
            collapsed.bands[1].rule_y,
            BAND_H + MORE_H + GAP_Y,
            "one band's row and its more row, then the next"
        );
        assert_eq!(
            collapsed.bands[1].height, BAND_H,
            "one card fits: no more row"
        );
        assert_eq!(collapsed.height(), BAND_H * 2 + MORE_H + GAP_Y);
        assert!(
            collapsed.bands[0].cell(0).is_none(),
            "a collapsed band's cards are the strip's"
        );

        let open = panel_layout(body, &all, Some(&all[0].worktree));
        assert_eq!(open.open(), Some(0));
        let first = &open.bands[0];
        let layout = first.content.as_ref().expect("the root band is open");
        assert_eq!(
            layout.rows,
            vec![vec![0, 1], vec![2, 3], vec![4, 5]],
            "six sessions over two columns"
        );
        assert_eq!(first.height, BAND_RULE_H + CARD_H * 3 + GAP_Y * 2);
        assert_eq!(
            first.cell(0).unwrap().y,
            BAND_RULE_H,
            "the first row right under the rule"
        );
        assert_eq!(first.cell(2).unwrap().y, BAND_RULE_H + CARD_H + GAP_Y);
        assert_eq!(
            open.bands[1].rule_y,
            first.height + GAP_Y,
            "feat's band moved down"
        );
        assert_eq!(open.bands[1].height, BAND_H, "and stays collapsed");

        // A window two rows of cards tall: the open band's third row and
        // feat's band are past its edge — two cards and one band.
        let short = Rect::new(
            0,
            0,
            80,
            HEAD_H + BAND_RULE_H + CARD_H * 2 + GAP_Y + BELOW_MARK_H,
        );
        let open = panel_layout(short, &all, Some(&all[0].worktree));
        assert!(open.overflows());
        assert_eq!(open.hidden(0), Hidden { above: 0, below: 3 });
        // Scrolled to the end: the first two rows are off the top.
        let end = open.max_scroll();
        assert_eq!(open.hidden(end), Hidden { above: 4, below: 0 });
        // Revealing feat's band brings the whole band on, rule and row.
        let scroll = open.reveal(0, 1, None);
        assert_eq!(scroll, end, "feat's row is the panel's last");
        assert!(place(
            open.window(),
            scroll,
            open.bands[1].cell(0).unwrap_or(Rect {
                y: open.bands[1].rule_y,
                height: BAND_H,
                ..open.area
            })
        )
        .unwrap()
        .whole());
    }

    /// The screenshot's grid: nine sessions in two columns and three
    /// terminals one to a row, in a body two rows short of the lot.
    fn crowded() -> (App, Rect) {
        let mut app = app();
        app.tree.agents = (0..9)
            .map(|i| agent(&format!("s{i}"), "w1", &format!("session {i}")))
            .collect();
        app.tree.terminals = (1..=3)
            .map(|i| terminal(&format!("t{i}"), "w1", &format!("term-{i}")))
            .collect();
        (app, Rect::new(0, 0, 100, 58))
    }

    /// A grid taller than the body scrolls by rows through a window that
    /// keeps its last row for the `↓` marker, and a card the window's top
    /// edge cuts is placed cut rather than left out. The cursor walked
    /// onto the first terminal used to draw the second row of sessions at
    /// the top and nothing at all where the first row was — one row too
    /// far up to be drawn whole, so a card-sized hole stood in for it.
    #[test]
    fn a_card_the_top_edge_cuts_is_placed_cut_not_dropped() {
        let (app, body) = crowded();
        let all = super::bands(&app);
        let at = all.iter().position(|b| b.worktree.0 == "w1").unwrap();
        assert_eq!(all[at].cards.len(), 12);
        let panel = panel_layout(body, &all, Some(&all[at].worktree));
        let pb = &panel.bands[at];
        let layout = pb.content.as_ref().expect("the band is open");
        assert_eq!(
            layout.rows.len(),
            8,
            "five rows of sessions, three of terminals"
        );
        assert_eq!(
            pb.height,
            layout.height(),
            "the band is as tall as its cards"
        );
        assert!(panel.overflows());
        let window = panel.window();
        assert_eq!(
            window.height,
            panel.area.height - BELOW_MARK_H,
            "the marker's row is kept back"
        );
        assert_eq!(panel.max_scroll(), panel.height() - window.height);

        // The first terminal is card 9: bringing it whole on screen
        // scrolls two rows, and the first row of sessions loses one row
        // to the top edge — and is still placed, one row short.
        let scroll = panel.reveal(0, at, Some(9));
        assert_eq!(scroll, 2);
        let first =
            place(window, scroll, pb.cell(0).unwrap()).expect("the first card is still placed");
        assert_eq!((first.cut_top, first.cut_bottom), (1, 0));
        assert!(!first.whole());
        assert_eq!(
            first.rect.y, window.y,
            "its first row on screen is the window's first"
        );
        assert_eq!(first.rect.height, CARD_H - 1);
        let term = place(window, scroll, pb.cell(9).unwrap()).expect("the cursor's card");
        assert!(term.whole(), "the cursor's card is whole");
        assert_eq!(
            term.rect.y + term.rect.height,
            window.y + window.height,
            "on the window's last row"
        );
        // Both cards on the cut row count as above; the two terminals
        // past the window count as below.
        assert_eq!(panel.hidden(scroll), Hidden { above: 2, below: 2 });
        // The band's own rule is off the top; the terminals' is on.
        let on_screen: Vec<&str> = pb
            .rules()
            .filter(|&(y, _)| {
                let rule = Rect {
                    y,
                    height: BAND_RULE_H,
                    ..panel.area
                };
                place(window, scroll, rule).is_some()
            })
            .map(|(_, label)| label)
            .collect();
        assert_eq!(on_screen, [TERMINALS_RULE]);
    }

    /// The scroll follows the cursor only as far as it must: a walk to
    /// the last card scrolls to the panel's end, a walk back up onto the
    /// first row scrolls all the way back — the rule over it comes too —
    /// and a card already whole on screen moves nothing, so a wheel that
    /// left the cursor's card in view is left alone. A scroll past the
    /// end comes back to it.
    #[test]
    fn reveal_scrolls_just_far_enough_either_way() {
        let (app, body) = crowded();
        let all = super::bands(&app);
        let at = all.iter().position(|b| b.worktree.0 == "w1").unwrap();
        let panel = panel_layout(body, &all, Some(&all[at].worktree));
        let pb = &panel.bands[at];
        let end = panel.reveal(2, at, Some(11));
        assert_eq!(end, panel.max_scroll(), "the last terminal is at the end");
        assert!(place(panel.window(), end, pb.cell(11).unwrap())
            .unwrap()
            .whole());
        assert_eq!(
            panel.reveal(end, at, Some(0)),
            pb.rule_y,
            "the first row brings the rule back"
        );
        assert_eq!(
            panel.reveal(end, at, Some(2)),
            pb.cell(2).unwrap().y - GAP_Y,
            "the second row brings the air over it"
        );
        assert_eq!(
            panel.reveal(5, at, Some(4)),
            5,
            "a card already whole moves nothing"
        );
        assert_eq!(panel.clamp(500), panel.max_scroll());
        assert_eq!(panel.reveal(500, at, Some(11)), panel.max_scroll());
    }

    /// A panel that fits has nothing to scroll: no marker row kept back,
    /// no scroll at all whatever is asked for, nothing hidden.
    #[test]
    fn a_panel_that_fits_never_scrolls() {
        let app = app();
        let all = super::bands(&app);
        let panel = panel_layout(Rect::new(0, 0, 80, 60), &all, Some(&all[0].worktree));
        assert!(!panel.overflows());
        assert_eq!(panel.window(), panel.area);
        assert_eq!(panel.max_scroll(), 0);
        assert_eq!(panel.reveal(7, 0, Some(0)), 0);
        assert_eq!(panel.clamp(7), 0);
        assert_eq!(panel.hidden(0), Hidden::default());
        assert!(place(panel.window(), 0, panel.bands[0].cell(0).unwrap())
            .unwrap()
            .whole());
    }

    /// A checkout with no terminal has no `terminals` section on its open
    /// band: the band's rule heads the cards alone, the layout ends with
    /// the last session and the walk on it. The rule comes with the first
    /// terminal.
    #[test]
    fn no_terminals_no_terminals_section() {
        let body = Rect::new(0, 0, 80, 60);
        let mut app = app();
        let all = super::bands(&app);
        let layout = expanded_layout(body, &all[0]);
        assert_eq!(
            layout.rules().iter().map(|(_, l)| *l).collect::<Vec<_>>(),
            [SESSIONS_RULE],
            "no rule over nothing"
        );
        assert_eq!(layout.rows, vec![vec![0]]);
        assert_eq!(
            layout.stepped(Some(0), 0, 1),
            Some(0),
            "j has nowhere to go"
        );
        assert_eq!(
            layout.height,
            BAND_RULE_H + CARD_H,
            "the layout ends with the session"
        );

        app.tree.terminals = vec![terminal("t1", "w1", "shell-1")];
        let all = super::bands(&app);
        let layout = expanded_layout(body, &all[0]);
        assert_eq!(
            layout.rules().iter().map(|(_, l)| *l).collect::<Vec<_>>(),
            [SESSIONS_RULE, TERMINALS_RULE]
        );
        assert_eq!(layout.rows, vec![vec![0], vec![1]]);
    }
}
