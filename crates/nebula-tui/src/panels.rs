//! The PANELS: the three-column layout nebula drew before the GRID, back
//! as Settings → Appearance → **Layout** `panels`. PROJECTS | WORKTREES |
//! SESSIONS down the left, each a list of rows with a STATUS DOT and the
//! cursor's selection bar, and the PANE beside them reading the session
//! under the SESSIONS cursor. Picking a project scopes the worktrees,
//! picking a worktree scopes the sessions; `h`/`l` walk FOCUS across the
//! columns, `j`/`k` the rows, Enter goes into the pane.
//!
//! Nothing here is a second copy of the tree or of the selection: every
//! row is a row of the lists the GRID's cursor already indexes —
//! `App::project_rows`, `App::worktree_rows`, `App::visible_session_rows`
//! — so `sel_project`, `sel_worktree` and `sel_session` are the cursors of
//! both layouts, and switching between them lands on the same rows with
//! the same session in the pane. The PROJECTS column stands in for the
//! PROJECT TABS: it lists every project on the machine, and the lit one
//! is whichever its cursor is on, which is also the tab the GRID lights.
//!
//! What lives here is what the layout adds: the columns' widths
//! ([`columns`]) and the drag of a column's BORDER that changes them
//! ([`drag_border`]), each column's FOLD — open, a RAIL folded by hand,
//! or the bare RULE SESSIONS folds to beside a pull request or an issue
//! ([`folds`]) — the rows each column lays its entries out on —
//! 3-row PROJECT buttons, 2-row PILLS under them ([`project_lines`],
//! [`worktree_lines`], [`session_lines`]), a session's pill growing by
//! its RECENT PROMPTS ([`prompt_lines`]) and its FOLLOW-UP COMPOSER
//! ([`follow_up_rows`]) — and the scroll that keeps the cursor's row on
//! screen until the wheel moves it ([`ColumnScroll`]).
//! The keys are `event_loop::panels`'s and the drawing
//! `ui::panels_view`'s.

use crate::app::{App, Focus, SessionRow, WorktreeRow};
use crate::keymap::KeyChord;
use crossterm::event::{KeyCode, KeyModifiers};
use nebula_core::{ProjectId, WorktreeId};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Color;

/// Widths of the PROJECTS, WORKTREES and SESSIONS columns on a body wide
/// enough for them and the pane, until a BORDER is dragged
/// (`App::panels_widths`).
pub const WIDTHS: [u16; 3] = [20, 22, 32];
/// Narrowest a column is squeezed to on a narrow body.
pub const MIN_W: u16 = 10;
/// Columns the PANE always keeps, whatever the columns would like.
pub const MIN_PANE_W: u16 = 20;
/// Columns a folded column keeps: its rule alone, a RAIL's expand
/// chevron drawn over it on the title row. The width the column was
/// dragged to is untouched while it is folded, so it opens back up to it.
pub const RAIL_W: u16 = 1;

/// How a column stands this frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fold {
    /// At its width, its rows listed under its title.
    Open,
    /// A RAIL: folded by hand — its title's `◀`, its `⇧` key, `^B` —
    /// down to its rule with a `▶` over it, which a click opens again.
    Rail,
    /// The bare rule, no chevron and no click target: SESSIONS beside a
    /// pull request or an issue under the WORKTREES cursor, which has no
    /// sessions to list ([`sessions_fold`]). It opens again of itself the
    /// moment the cursor steps onto a checkout.
    Rule,
}

/// `key` held with `mods`, as `KeyChord::from_event` spells it.
const fn chord(key: char, mods: KeyModifiers) -> KeyChord {
    KeyChord {
        code: KeyCode::Char(key),
        mods,
    }
}

/// The keys that fold PROJECTS, WORKTREES (`B` for branches) and SESSIONS
/// to their RAILS, or open them again: `⇧P`, `⇧B`, `⇧S`, the keys the
/// columns always folded by. Fixed rather than rebindable, and ahead of
/// whatever the keymap binds them to — `⇧P` is the GRID's **Duplicate
/// session**, which the panels reach from a row's right-click menu — but
/// only beside the columns: the GRID keeps its own meanings
/// (`event_loop::panels::fold_key`).
pub const FOLD_KEYS: [KeyChord; 3] = [
    chord('p', KeyModifiers::SHIFT),
    chord('b', KeyModifiers::SHIFT),
    chord('s', KeyModifiers::SHIFT),
];
/// The keys that fold every column at once, or open them all: `^B`, `⌘B`
/// where the emulator sends it, and `⇧Z` for a tmux whose prefix eats
/// `^B`.
pub const FOLD_ALL_KEYS: [KeyChord; 3] = [
    chord('b', KeyModifiers::CONTROL),
    chord('b', KeyModifiers::SUPER),
    chord('z', KeyModifiers::SHIFT),
];

/// What one of the PANELS' own keys ([`PANEL_KEYS`]) does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PanelKey {
    /// `⇧Tab` / `^⇧H`: FOCUS one open column left, stopping at the first
    /// — the backward twin of Tab.
    FocusPrev,
    /// `^⇧L`: Tab's twin, for an emulator with the kitty protocol.
    FocusNext,
    /// `^→`: FOCUS one step right without taking the input lock, so the
    /// pane is reached and read without typing into it.
    FocusTerminal,
    /// `z`: the pane FULL-SCREEN with the input lock on.
    Zoom,
    /// `m`: the CONTEXT MENU of the row under the focused column's cursor.
    ContextMenu,
    /// `⇧C`: a Ghostty tab in the selected checkout, as `⇧T` is.
    OpenGhosttyTab,
}

/// `code` held with `mods`, for a key that is no character.
const fn key(code: KeyCode, mods: KeyModifiers) -> KeyChord {
    KeyChord { code, mods }
}

/// The keys the columns always answered to that the GRID dropped or never
/// had: fixed rather than rebindable, ahead of whatever the keymap binds
/// them to, and only beside the columns, as [`FOLD_KEYS`] are
/// (`event_loop::panels::panel_key`). `^⇧H` is also a hatch out of a
/// LOCKED PANE; that one is the pane's, read before any of these.
pub const PANEL_KEYS: [(KeyChord, PanelKey); 7] = [
    (key(KeyCode::Tab, KeyModifiers::SHIFT), PanelKey::FocusPrev),
    (
        chord('h', KeyModifiers::CONTROL.union(KeyModifiers::SHIFT)),
        PanelKey::FocusPrev,
    ),
    (
        chord('l', KeyModifiers::CONTROL.union(KeyModifiers::SHIFT)),
        PanelKey::FocusNext,
    ),
    (
        key(KeyCode::Right, KeyModifiers::CONTROL),
        PanelKey::FocusTerminal,
    ),
    (chord('z', KeyModifiers::NONE), PanelKey::Zoom),
    (chord('m', KeyModifiers::NONE), PanelKey::ContextMenu),
    (chord('c', KeyModifiers::SHIFT), PanelKey::OpenGhosttyTab),
];

/// Whether SESSIONS has nothing to list: the WORKTREES cursor is on an
/// `OPEN PRS` or `ISSUES` row, neither of which has a checkout and so
/// sessions, and the pane beside it reads the pull request or the issue
/// — so the column folds to its RULE and gives the pane its width for as
/// long as the cursor rests there. `App::panels_hidden` is untouched
/// either way: a column folded by hand stays a RAIL on the checkout too,
/// and this fold is never written to CONFIG.JSON.
pub fn sessions_fold(app: &App) -> bool {
    app.selected_worktree_pr().is_some() || app.selected_worktree_issue().is_some()
}

/// The three columns' folds, PROJECTS first: a RAIL where
/// `App::panels_hidden` says so, SESSIONS' RULE where [`sessions_fold`]
/// does, the rest open.
pub fn folds(app: &App) -> [Fold; 3] {
    let by_hand = |hidden: bool| if hidden { Fold::Rail } else { Fold::Open };
    let [projects, worktrees, sessions] = app.panels_hidden;
    let sessions = if !sessions && sessions_fold(app) {
        Fold::Rule
    } else {
        by_hand(sessions)
    };
    [by_hand(projects), by_hand(worktrees), sessions]
}

/// The FOCUS TINT the PANELS paint for colour theme `theme` (a
/// CONFIG.JSON `theme`, read as `Theme::by_name` reads it): the faintly
/// lit gray each preset had while the columns were the only layout, where
/// the GRID's `Theme::focus_tint` is the accent taken down to near-black.
/// `event_loop::apply_config` puts it in the app's theme while the layout
/// is `panels`; every other colour is the preset's own.
pub fn focus_tint(theme: &str) -> Color {
    let (r, g, b) = match theme.trim().to_ascii_lowercase().as_str() {
        "ocean" => (21, 31, 38),
        "forest" => (26, 34, 27),
        "rose" => (37, 28, 32),
        "amber" => (37, 32, 22),
        "lavender" => (30, 28, 38),
        "coral" => (38, 28, 26),
        "slate" => (27, 30, 36),
        "sand" => (36, 32, 27),
        "mono" => (30, 30, 30),
        _ => (22, 33, 34),
    };
    Color::Rgb(r, g, b)
}

/// The column FOCUS `focus` names, PROJECTS first; None for the pane.
pub fn column_index(focus: Focus) -> Option<usize> {
    match focus {
        Focus::Projects => Some(0),
        Focus::Worktrees => Some(1),
        Focus::Sessions => Some(2),
        Focus::Terminal => None,
    }
}

/// Whether FOCUS can stand on `focus`: the pane always, a column while it
/// is open — the walk (`h`/`l`, Tab) steps over a folded one.
pub fn focus_open(app: &App, focus: Focus) -> bool {
    column_index(focus).is_none_or(|i| folds(app)[i] == Fold::Open)
}

/// FOCUS stepped off a column that has folded under it — by a key, a
/// click, a SETTING, or the SESSIONS fold a cursor's move brings on —
/// onto the nearest open column to the left, the way `⇧Tab` walks, or on
/// to the next one, the pane at the end, when none is open. Nothing to
/// do while it stands on an open column, or the pane.
pub fn settle_focus(app: &mut App) {
    if focus_open(app, app.focus) {
        return;
    }
    let back = app.previous_visible_focus(app.focus);
    app.focus = if back == app.focus {
        app.next_visible_focus(app.focus)
    } else {
        back
    };
}

/// Where a row — or a header that folds — sits in the PANELS, as a click
/// target (`HitTarget::PanelsRow`). The indices are the cursors': a
/// project's place in `App::project_rows`, a worktree row's in
/// `App::worktree_rows`, a session row's in `App::visible_session_rows`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Row {
    Project(usize),
    Worktree(usize),
    Session(usize),
    /// The WORKTREES column's `OPEN PRS` header: a click folds the group.
    OpenPrsHeader,
    /// Its `ISSUES` header, the same way.
    IssuesHeader,
    /// The SESSIONS column's `ARCHIVED` header: a click opens the group
    /// or folds it, as `⇧A` does.
    ArchivedHeader,
    /// Session row `i`'s FOLLOW-UP CHEVRON — the `▸` / `▾` at the end of
    /// its text row: a click puts the cursor on the row and expands its
    /// pill into the FOLLOW-UP COMPOSER, or folds it, as Space does.
    FollowUp(usize),
    /// Inside the open FOLLOW-UP COMPOSER: a click there gives SESSIONS
    /// FOCUS and stops, never a second click on the pill under it — which
    /// would attach the session and lock the pane out from under the
    /// typing.
    FollowUpBox,
}

impl Row {
    /// The column the row is in — the FOCUS a click on it takes.
    pub fn focus(self) -> Focus {
        match self {
            Row::Project(_) => Focus::Projects,
            Row::Worktree(_) | Row::OpenPrsHeader | Row::IssuesHeader => Focus::Worktrees,
            Row::Session(_) | Row::ArchivedHeader | Row::FollowUp(_) | Row::FollowUpBox => {
                Focus::Sessions
            }
        }
    }
}

/// Rows a PROJECT button is tall: its name on the middle one, a row of
/// air above and below for the selection fill to wrap. A renamed project
/// grows by the row its folder's name takes under the name.
pub const PROJECT_BTN_H: usize = 3;
/// The stride WORKTREES and SESSIONS rows stack on. Each is a PILL — a
/// 3-row cell, half-block pad, text, half-block pad — and a pill's bottom
/// pad is the next one's top pad, so the rows sit two apart with no air
/// of their own and the step down from the PROJECT buttons reads in text
/// weight rather than spacing.
pub const PILL_H: usize = 2;
/// Rows one PILL covers on its own: its two pads and its text.
pub const PILL_CELL: usize = PILL_H + 1;

/// What one entry of a column is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Line {
    /// A group's name over its rows — `TERMINALS`, `▾ OPEN PRS · 3`.
    /// `fold` is the header's own click target, for a group that folds.
    Header { text: String, fold: Option<Row> },
    /// One row, by its place in the column's list.
    Row(Row),
}

/// One entry of a column laid out in rows: where it starts down the
/// column, as though the column were tall enough for all of it, and how
/// many rows it covers. Two PILLS overlap by the pad they share, so an
/// entry's `top + height` can run into the next one's `top`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Placed {
    pub top: usize,
    pub height: usize,
    pub line: Line,
}

/// A column's entries in the order they stack, each at its row
/// ([`Placed`]): what the drawing walks, what the scroll measures.
pub type Column = Vec<Placed>;

/// Rows `column` runs down, the last entry's bottom pad and all.
pub fn content_height(column: &[Placed]) -> usize {
    column.iter().map(|p| p.top + p.height).max().unwrap_or(0)
}

/// Lays a column out top to bottom: the rows each entry takes, and the
/// line of air above every group after the first.
#[derive(Default)]
struct Stack {
    column: Column,
    next: usize,
}

impl Stack {
    /// A group's header, a line of air above it unless it opens the
    /// column. After a PILL the air is the pill's own bottom pad, so the
    /// header sits straight under it.
    fn header(&mut self, text: String, fold: Option<Row>) {
        if self.next > 0 {
            self.next += 1;
        }
        self.place(1, 1, Line::Header { text, fold });
    }

    /// A PILL, stacked on the [`PILL_H`] stride.
    fn pill(&mut self, row: Row) {
        self.pill_with(row, 0);
    }

    /// A PILL with `inside` rows of its own under its text — a session's
    /// RECENT PROMPTS and its FOLLOW-UP COMPOSER — which it grows by,
    /// keeping its bottom pad, so the next pill starts below that.
    fn pill_with(&mut self, row: Row, inside: usize) {
        if inside == 0 {
            self.place(PILL_CELL, PILL_H, Line::Row(row));
        } else {
            let height = PILL_CELL + inside;
            self.place(height, height, Line::Row(row));
        }
    }

    /// A button `height` rows tall, the next one straight under it.
    fn button(&mut self, row: Row, height: usize) {
        self.place(height, height, Line::Row(row));
    }

    /// A quiet row between two entries.
    fn air(&mut self) {
        self.next += 1;
    }

    fn place(&mut self, height: usize, stride: usize, line: Line) {
        self.column.push(Placed {
            top: self.next,
            height,
            line,
        });
        self.next += stride;
    }
}

/// The four rects of the PANELS: the three columns at the widths they were
/// dragged to — [`WIDTHS`] until then — a folded one at [`RAIL_W`], and
/// the PANE taking every column left over. A body too narrow for that
/// squeezes the open columns in proportion — down to [`MIN_W`] each — so
/// the pane keeps [`MIN_PANE_W`] for as long as the window allows. The
/// squeeze and the fold are the frame's alone: the widths remembered are
/// untouched, so a window that grows back, or a column opened again,
/// comes back to them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Columns {
    pub projects: Rect,
    pub worktrees: Rect,
    pub sessions: Rect,
    pub pane: Rect,
    /// Each column's [`Fold`] as laid out, PROJECTS first.
    pub folds: [Fold; 3],
}

impl Columns {
    /// The column FOCUS `focus` names, or the pane.
    pub fn of(&self, focus: Focus) -> Rect {
        match focus {
            Focus::Projects => self.projects,
            Focus::Worktrees => self.worktrees,
            Focus::Sessions => self.sessions,
            Focus::Terminal => self.pane,
        }
    }

    /// The three columns' widths as laid out, PROJECTS first.
    pub fn widths(&self) -> [u16; 3] {
        [self.projects, self.worktrees, self.sessions].map(|r| r.width)
    }

    /// Column `i`'s BORDER: the screen column just past its rule, where the
    /// next column — or, past SESSIONS, the pane — starts.
    pub fn border(&self, i: usize) -> u16 {
        let col = self.column(i);
        col.x + col.width
    }

    /// Column `i`, PROJECTS first.
    fn column(&self, i: usize) -> Rect {
        [self.projects, self.worktrees, self.sessions][i]
    }

    /// Column `i`'s BORDER as a drag target (`HitTarget::PanelsBorder`):
    /// its rule and the cell after it, down the whole body — the two
    /// touching cells every splitter grabs by. Beside a folded column the
    /// zone stops at the rule: the column after it is one cell wide, and
    /// that cell is a RAIL's expand chevron, which a click must reach
    /// rather than arm a drag on its neighbor. None for a folded column,
    /// which has no width to drag.
    pub fn grab_zone(&self, i: usize) -> Option<Rect> {
        if self.folds[i] != Fold::Open {
            return None;
        }
        let beside_fold = self.folds.get(i + 1).is_some_and(|f| *f != Fold::Open);
        Some(Rect {
            x: self.border(i).saturating_sub(1),
            width: if beside_fold { 1 } else { 2 },
            ..self.column(i)
        })
    }
}

/// Lay the PANELS out over `body` ([`Columns`]), the open columns at
/// `widths` — [`WIDTHS`] when None — and the folded ones at [`RAIL_W`].
pub fn columns(body: Rect, widths: Option<[u16; 3]>, folds: [Fold; 3]) -> Columns {
    let widths = widths.unwrap_or(WIDTHS);
    let open = |i: usize| folds[i] == Fold::Open;
    let rails = (0..3).filter(|i| !open(*i)).count() as u16 * RAIL_W;
    let budget = body.width.saturating_sub(MIN_PANE_W.saturating_add(rails));
    let requested: [u32; 3] = widths.map(|w| u32::from(w.max(MIN_W)));
    let requested_total: u32 = (0..3).filter(|i| open(*i)).map(|i| requested[i]).sum();
    let available = u32::from(budget);
    let mut fitted = [0u32; 3];
    if requested_total <= available {
        for i in 0..3 {
            fitted[i] = if open(i) {
                requested[i]
            } else {
                u32::from(RAIL_W)
            };
        }
    } else {
        let open_count = folds.iter().filter(|f| **f == Fold::Open).count() as u32;
        let floor_total = open_count * u32::from(MIN_W);
        if available >= floor_total {
            // Iteratively pin columns whose proportional share falls below
            // MIN_W, then divide the remainder among the other columns.
            let mut remaining = available;
            let mut active: [bool; 3] = std::array::from_fn(open);
            loop {
                let weight: u32 = (0..3).filter(|i| active[*i]).map(|i| requested[i]).sum();
                let below_floor: [bool; 3] = std::array::from_fn(|i| {
                    active[i] && requested[i] * remaining < u32::from(MIN_W) * weight
                });
                if !below_floor.iter().any(|below| *below) {
                    for i in 0..3 {
                        if active[i] && weight > 0 {
                            fitted[i] = requested[i] * remaining / weight;
                        }
                    }
                    break;
                }
                for i in 0..3 {
                    if below_floor[i] {
                        fitted[i] = u32::from(MIN_W);
                        remaining -= u32::from(MIN_W);
                        active[i] = false;
                    }
                }
            }
        }
        // Keep each open column's floor even when the whole layout is too
        // small; Ratatui clips the resulting areas to the body as before.
        for (i, width) in fitted.iter_mut().enumerate() {
            *width = if open(i) {
                (*width).max(u32::from(MIN_W))
            } else {
                u32::from(RAIL_W)
            };
        }
    }
    let widths = fitted.map(|w| w as u16);
    let [projects, worktrees, sessions, pane] = Layout::horizontal([
        Constraint::Length(widths[0]),
        Constraint::Length(widths[1]),
        Constraint::Length(widths[2]),
        Constraint::Min(0),
    ])
    .areas(body);
    Columns {
        projects,
        worktrees,
        sessions,
        pane,
        folds,
    }
}

/// The PANELS over `body` as `app` stands: its dragged widths and its
/// columns' folds ([`columns`]).
pub fn layout(app: &App, body: Rect) -> Columns {
    columns(body, app.panels_widths, folds(app))
}

/// The widths that leave column `i`'s BORDER at screen column `border`,
/// the other open column kept as `body` lays it out this frame and a
/// folded one at the width it opens back up to: the column held to
/// [`MIN_W`] at the narrow end and, at the wide one, to what leaves the
/// pane its [`MIN_PANE_W`] beside the others as drawn. None for a folded
/// column, and on a body with no room to widen or narrow it at all —
/// there is nothing a drag there could remember.
pub fn drag_border(
    body: Rect,
    widths: Option<[u16; 3]>,
    folds: [Fold; 3],
    i: usize,
    border: i32,
) -> Option<[u16; 3]> {
    if folds[i] != Fold::Open {
        return None;
    }
    let cols = columns(body, widths, folds);
    let drawn = cols.widths();
    let left = cols.column(i).x;
    let others: u16 = drawn.iter().sum::<u16>() - drawn[i];
    let max = body.width.saturating_sub(others + MIN_PANE_W);
    if max < MIN_W {
        return None;
    }
    let remembered = widths.unwrap_or(WIDTHS);
    let mut widths: [u16; 3] = std::array::from_fn(|j| {
        if folds[j] == Fold::Open {
            drawn[j]
        } else {
            remembered[j]
        }
    });
    let want = (border - i32::from(left)).clamp(0, i32::from(u16::MAX)) as u16;
    widths[i] = want.clamp(MIN_W, max);
    Some(widths)
}

/// The PROJECTS column: one button per project, most recently worked in
/// first — `App::project_rows`' own order, which is what `sel_project`
/// indexes — a row taller for a renamed project, whose folder's name
/// hangs under its own.
pub fn project_lines(app: &App) -> Column {
    let mut stack = Stack::default();
    for (i, at) in app.project_rows().into_iter().enumerate() {
        let renamed = app.tree.projects[at].folder_subtitle().is_some();
        stack.button(Row::Project(i), PROJECT_BTN_H + usize::from(renamed));
    }
    stack.column
}

/// The WORKTREES column: the project's checkouts — the ROOT WORKTREE
/// first, with a quiet row under it — then the `OPEN PRS` group (its pull
/// requests, each with the checkout on its head branch nested under it)
/// and the `ISSUES` group, each under a header that folds it. A folded
/// group is its header alone, still counting what it holds; so is an
/// OPEN PRS group whose every pull request is a draft kept out.
pub fn worktree_lines(app: &App) -> Column {
    let rows = app.worktree_rows();
    let mut stack = Stack::default();
    let plain = rows
        .iter()
        .take_while(|r| matches!(r, WorktreeRow::Checkout(_)))
        .count();
    for (i, row) in rows.iter().enumerate().take(plain) {
        stack.pill(Row::Worktree(i));
        if matches!(row, WorktreeRow::Checkout(w) if w.is_main) && plain > 1 {
            stack.air();
        }
    }
    // The issue rows close the list; everything between the plain
    // checkouts and them is the OPEN PRS group.
    let pr_end = rows
        .iter()
        .position(|r| matches!(r, WorktreeRow::Issue(_)))
        .unwrap_or(rows.len());
    let listed = app.listed_open_prs().len();
    let open = app.all_open_prs().len();
    if listed > 0 || open > listed {
        stack.header(open_prs_header(app, listed, open), Some(Row::OpenPrsHeader));
        // A checkout under its pull request stacks straight onto the pull
        // request's pill: the two are one thing.
        for i in plain..pr_end {
            stack.pill(Row::Worktree(i));
        }
    }
    let issues = app.listed_issues().len();
    if issues > 0 {
        // A hundred rows is not "a hundred issues": the answer hit the
        // fetch cap, and the count says so as the pull requests' does.
        let more = if issues >= crate::issues::LIST_LIMIT {
            "+"
        } else {
            ""
        };
        stack.header(
            format!(
                "{} ISSUES · {issues}{more}",
                fold_glyph(app.issues_collapsed)
            ),
            Some(Row::IssuesHeader),
        );
        for i in pr_end..rows.len() {
            stack.pill(Row::Worktree(i));
        }
    }
    stack.column
}

/// `▾ OPEN PRS · 3` over the rows, `▸` with them folded away. A list cut
/// off at the fetch cap says `100+` rather than passing itself off as
/// the whole set — the cap is on the answer, drafts and all, so it is
/// measured there — and with drafts kept out the count reads `9/12`:
/// nine rows listed of twelve open.
fn open_prs_header(app: &App, listed: usize, open: usize) -> String {
    let more = if open >= crate::pull_request::LIST_LIMIT {
        "+"
    } else {
        ""
    };
    let count = if open > listed {
        format!("{listed}/{open}{more}")
    } else {
        format!("{listed}{more}")
    };
    format!("{} OPEN PRS · {count}", fold_glyph(app.open_prs_collapsed))
}

/// The disclosure triangle: what a click on the header would do.
fn fold_glyph(folded: bool) -> &'static str {
    if folded {
        "▸"
    } else {
        "▾"
    }
}

/// The SESSIONS column: the checkout's live sessions, most recently
/// touched first, as one list with no header of its own — the headers
/// under it name what isn't a live agent: its `TERMINALS`, its `PULL
/// REQUESTS` and last the ARCHIVED group, open (`ARCHIVED · 2`, by `⇧A`
/// or a click on its header) or folded to the one line that counts it
/// (`… 2 archived`). The order is the one `App::visible_session_rows`
/// lists, which is what `sel_session` indexes. A session's pill grows by
/// the RECENT PROMPTS under its text and, on the one expanded, by its
/// FOLLOW-UP COMPOSER, wrapped at the list's `width` — so opening the box
/// pushes every pill below it down the column.
pub fn session_lines(app: &App, width: u16) -> Column {
    let rows = app.visible_session_rows();
    // The checkout's live and archived sessions, archived counted whether
    // the group is open or not: a folded group still says what it holds.
    let (live, archived) = app
        .selected_worktree()
        .map_or((0, 0), |w| app.group_counts_in(&w.id));
    let live = live.min(rows.len());
    let of = |want: fn(&SessionRow) -> bool| -> Vec<usize> {
        rows.iter()
            .enumerate()
            .filter(|(_, row)| want(row))
            .map(|(i, _)| i)
            .collect()
    };
    let terminals = of(|row| matches!(row, SessionRow::Terminal(_)));
    let links = of(|row| matches!(row, SessionRow::Link(_)));
    let gone = of(SessionRow::is_archived_agent);
    let mut stack = Stack::default();
    let pill = |stack: &mut Stack, i: usize| {
        let inside = rows.get(i).map_or(0, |row| {
            prompt_lines(app, row) + follow_up_rows(app, i, width)
        });
        stack.pill_with(Row::Session(i), inside);
    };
    for i in 0..live {
        pill(&mut stack, i);
    }
    for (title, group) in [("TERMINALS", &terminals), ("PULL REQUESTS", &links)] {
        if !group.is_empty() {
            stack.header(title.to_string(), None);
            for i in group {
                pill(&mut stack, *i);
            }
        }
    }
    if archived > 0 {
        let title = if app.show_archived {
            format!("ARCHIVED · {archived}")
        } else {
            format!("… {archived} archived")
        };
        stack.header(title, Some(Row::ArchivedHeader));
        for i in &gone {
            pill(&mut stack, *i);
        }
    }
    stack.column
}

/// How many RECENT PROMPTS lines a session row carries under its pill:
/// the `recent_prompts` setting (`App::recent_prompts`), capped at what the
/// session has. None for a terminal or a link (they take no prompts), an
/// archived session (its history is over, and the group is for scanning
/// names) or a QUICK PROMPT stand-in (its row has not been created yet).
pub fn prompt_lines(app: &App, row: &SessionRow) -> usize {
    match row {
        SessionRow::Agent(a) if !a.archived && !app.is_placeholder_agent(&a.id) => {
            app.recent_prompts.min(a.recent_prompts.len())
        }
        _ => 0,
    }
}

/// The most text rows the FOLLOW-UP COMPOSER grows to before it scrolls
/// under its own caret. Four is a paragraph of instruction in a 30-column
/// column; past that the box would own the column and push every pill
/// below it off the bottom for a prompt nobody reads back in full anyway.
pub const FOLLOW_UP_MAX_LINES: usize = 4;

/// Rows the FOLLOW-UP COMPOSER takes inside session row `index`'s pill, in
/// a list `width` columns wide: the framed box — title row, text, hint row
/// — or 0 for every pill but the expanded one. The layout and the draw
/// both ask, so the height they agree on is computed once here from the
/// text as it wraps at this width.
pub fn follow_up_rows(app: &App, index: usize, width: u16) -> usize {
    if app.follow_up_row() != Some(index) {
        return 0;
    }
    let Some(follow_up) = &app.follow_up else {
        return 0;
    };
    let lines = crate::ui::multiline_input_lines(
        &follow_up.input,
        follow_up_text_width(width),
        app.theme.accent,
        app.theme,
    )
    .0
    .len();
    2 + lines.clamp(1, FOLLOW_UP_MAX_LINES)
}

/// Columns of typing inside the composer's frame, in a list `width`
/// columns wide: the pill's rail column, the box's two borders and a space
/// either side of the text come off it first. Never 0 — a column dragged
/// down to [`MIN_W`] still has to wrap somewhere.
pub fn follow_up_text_width(width: u16) -> usize {
    usize::from(width).saturating_sub(5).max(1)
}

/// Where the column FOCUS names keeps its scroll in `App::panels_scroll`.
/// The pane has none and is never asked: it reads as the last column.
pub fn scroll_slot(focus: Focus) -> usize {
    match focus {
        Focus::Projects => 0,
        Focus::Worktrees => 1,
        Focus::Sessions | Focus::Terminal => 2,
    }
}

/// One column's scroll: the first row drawn, which the wheel moves under
/// a cursor that stays put and the cursor's own moves bring back on
/// screen. The columns lay out and clamp it each frame ([`settle`]); the
/// wheel (`event_loop::panels::wheel`) reads what that left in `max`.
///
/// [`settle`]: ColumnScroll::settle
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ColumnScroll {
    /// The first row drawn.
    pub top: usize,
    /// Furthest `top` goes: the column's last row on the bottom one, zero
    /// for a column that fits.
    pub max: usize,
    /// The cursor, the entry count and the rows the cursor's entry covers
    /// as the last frame drew them; a frame that finds any changed reveals
    /// the cursor again — a FOLLOW-UP COMPOSER opening, or growing by a
    /// line typed into it, among them. `None` is "reveal it".
    seen: Option<(Option<Row>, usize, usize)>,
}

impl ColumnScroll {
    /// The first row to draw of `column` in a list `height` rows tall:
    /// `top` held within the column, and — when the cursor, or what the
    /// column lists, is not what the last frame drew — slid only as far
    /// as brings the cursor's entry on screen, the header over it too
    /// when it is the first of its group, so a wheeled-away cursor stays
    /// away until it moves.
    pub fn settle(&mut self, column: &[Placed], cursor: Option<Row>, height: usize) -> usize {
        let height = height.max(1);
        self.max = content_height(column).saturating_sub(height);
        let at = cursor.and_then(|c| column.iter().position(|p| p.line == Line::Row(c)));
        let tall = at.map_or(0, |at| column[at].height);
        if self.seen != Some((cursor, column.len(), tall)) {
            self.seen = Some((cursor, column.len(), tall));
            if let Some(at) = at {
                let entry = &column[at];
                let up_to = match at.checked_sub(1).map(|i| &column[i]) {
                    Some(
                        header @ Placed {
                            line: Line::Header { .. },
                            ..
                        },
                    ) => header.top,
                    _ => entry.top,
                };
                let bottom = entry.top + entry.height;
                if up_to < self.top {
                    self.top = up_to;
                } else if bottom > self.top + height {
                    self.top = bottom - height;
                }
            }
        }
        self.top = self.top.min(self.max);
        self.top
    }

    /// A notch of the wheel: `delta` rows, held at the column's ends. False
    /// when the column fits, or is already at that end, and nothing moved.
    pub fn wheel(&mut self, delta: isize) -> bool {
        let next = self.top.saturating_add_signed(delta).min(self.max);
        std::mem::replace(&mut self.top, next) != next
    }

    /// The next frame reveals the cursor, whether or not it moved: what a
    /// key that walks the column asks for, so one pressed at the column's
    /// end still brings a wheeled-away cursor back.
    pub fn reveal_next(&mut self) {
        self.seen = None;
    }
}

/// The newest finish under `worktree` is still a ONE-SHOT SWEEP's worth
/// old (`app::fresh_done`): its row sweeps blue once, as the session's.
pub fn worktree_fresh_done(app: &App, worktree: &WorktreeId) -> bool {
    let now = crate::app::now_ms();
    app.tree
        .agents
        .iter()
        .any(|a| &a.worktree_id == worktree && crate::app::fresh_done(a, now))
}

/// The same over every checkout of `project`.
pub fn project_fresh_done(app: &App, project: &ProjectId) -> bool {
    app.tree
        .worktrees
        .iter()
        .filter(|w| &w.project_id == project)
        .any(|w| worktree_fresh_done(app, &w.id))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn body(width: u16) -> Rect {
        Rect::new(0, 0, width, 30)
    }

    const OPEN: [Fold; 3] = [Fold::Open; 3];

    /// A body with room for all three at their widths and the pane gets
    /// every column left over.
    #[test]
    fn the_columns_open_at_their_widths_and_the_pane_takes_the_rest() {
        let c = columns(body(190), None, OPEN);
        assert_eq!(
            [c.projects.width, c.worktrees.width, c.sessions.width],
            WIDTHS
        );
        assert_eq!(c.pane.x, 74);
        assert_eq!(c.pane.width, 190 - 74);
        assert_eq!(c.of(Focus::Worktrees), c.worktrees);
        assert_eq!(c.of(Focus::Terminal), c.pane);
    }

    /// A narrow body squeezes the columns, never under their floor, so the
    /// pane keeps its own.
    #[test]
    fn a_narrow_body_squeezes_the_columns_for_the_pane() {
        let c = columns(body(80), None, OPEN);
        assert!(c.pane.width >= MIN_PANE_W, "{c:?}");
        for w in [c.projects.width, c.worktrees.width, c.sessions.width] {
            assert!((MIN_W..32).contains(&w), "{c:?}");
        }
        // Widths dragged wide on a bigger window squeeze the same way.
        let c = columns(body(80), Some([60, 60, 60]), OPEN);
        assert!(c.pane.width >= MIN_PANE_W, "{c:?}");
        assert!(c.widths().iter().all(|w| *w >= MIN_W), "{c:?}");
    }

    #[test]
    fn uneven_saved_widths_fit_without_overspending_the_pane_floor() {
        let c = columns(body(80), Some([10, 10, 100]), OPEN);
        assert_eq!(c.widths(), [10, 10, 40]);
        assert_eq!(c.pane.width, MIN_PANE_W);
    }

    #[test]
    fn folded_rails_and_saved_open_widths_share_only_the_open_budget() {
        let c = columns(
            body(62),
            Some([400, 10, 100]),
            [Fold::Rail, Fold::Open, Fold::Open],
        );
        assert_eq!(c.widths(), [RAIL_W, 10, 31]);
        assert_eq!(c.pane.width, MIN_PANE_W);
    }

    #[test]
    fn widths_fit_tiny_bodies_and_keep_the_body_origin() {
        let tiny = columns(Rect::new(7, 4, 19, 8), Some([u16::MAX; 3]), OPEN);
        assert_eq!(tiny.projects.x, 7);
        assert_eq!(tiny.widths().iter().map(|w| u32::from(*w)).sum::<u32>(), 19);
        assert_eq!(tiny.pane.width, 0);

        let below_floor = columns(Rect::new(5, 2, 40, 8), None, OPEN);
        assert_eq!(below_floor.widths(), [10; 3]);
        assert_eq!(below_floor.pane.width, 10);

        let at_floor = columns(Rect::new(5, 2, 50, 8), None, OPEN);
        assert_eq!(at_floor.widths(), [10; 3]);
        assert_eq!(at_floor.pane.width, MIN_PANE_W);

        let above_floor = columns(Rect::new(5, 2, 79, 8), Some([10, 10, 100]), OPEN);
        assert_eq!(
            above_floor
                .widths()
                .iter()
                .map(|w| u32::from(*w))
                .sum::<u32>(),
            59
        );
        assert_eq!(above_floor.pane.width, MIN_PANE_W);
        assert_eq!(above_floor.projects.x, 5);
    }

    /// Widths a drag left are the ones laid out, and each BORDER sits
    /// just past its column's rule.
    #[test]
    fn the_columns_open_at_the_widths_they_were_dragged_to() {
        let c = columns(body(190), Some([30, 15, 40]), OPEN);
        assert_eq!(c.widths(), [30, 15, 40]);
        assert_eq!(c.pane.x, 85);
        assert_eq!([c.border(0), c.border(1), c.border(2)], [30, 45, 85]);
        assert_eq!(c.grab_zone(2), Some(Rect::new(84, 0, 2, 30)));
    }

    /// A drag moves the one column's BORDER, wider or narrower, and the
    /// other two keep their widths.
    #[test]
    fn a_drag_widens_and_narrows_the_one_column() {
        let wide = drag_border(body(190), None, OPEN, 1, 50);
        assert_eq!(wide, Some([20, 30, 32]));
        let narrow = drag_border(body(190), wide, OPEN, 2, 70);
        assert_eq!(narrow, Some([20, 30, 20]));
    }

    /// The column rests at its floor dragged off the left, and dragged
    /// off the right it stops where the pane keeps its own.
    #[test]
    fn a_drag_holds_the_column_and_the_pane_to_their_floors() {
        assert_eq!(
            drag_border(body(190), None, OPEN, 0, -40),
            Some([MIN_W, 22, 32])
        );
        let [p, w, s] = drag_border(body(190), None, OPEN, 1, 500).unwrap();
        assert_eq!((p, s), (20, 32), "only the dragged column moved");
        assert_eq!(190 - (p + w + s), MIN_PANE_W);
    }

    /// A body already squeezed to the floors has nothing to drag: the
    /// widths stay as they were remembered.
    #[test]
    fn a_body_with_no_room_remembers_no_drag() {
        assert_eq!(drag_border(body(40), None, OPEN, 0, 30), None);
    }

    /// A folded column takes its rail's one cell and the pane the rest;
    /// its width is the frame's, and the open columns keep theirs.
    #[test]
    fn a_folded_column_is_a_rail_and_the_pane_takes_its_width() {
        let c = columns(
            body(190),
            Some([30, 15, 40]),
            [Fold::Rail, Fold::Open, Fold::Rule],
        );
        assert_eq!(c.widths(), [RAIL_W, 15, RAIL_W]);
        assert_eq!(c.pane.x, 2 * RAIL_W + 15);
        assert_eq!(c.grab_zone(0), None, "a rail has no width to drag");
        assert_eq!(
            c.grab_zone(1),
            Some(Rect::new(c.border(1) - 1, 0, 1, 30)),
            "beside a fold the zone stops at the rule"
        );
        // Only the open columns are squeezed on a narrow body.
        let c = columns(body(60), None, [Fold::Rail, Fold::Rail, Fold::Open]);
        assert_eq!(c.widths(), [RAIL_W, RAIL_W, 32]);
    }

    /// A drag beside a folded column keeps the width the fold will open
    /// back up to, not the rail's one cell.
    #[test]
    fn a_drag_keeps_a_folded_columns_width() {
        let folds = [Fold::Rail, Fold::Open, Fold::Open];
        let widths = drag_border(body(190), Some([30, 15, 40]), folds, 1, 21);
        assert_eq!(widths, Some([30, 20, 40]));
        assert_eq!(drag_border(body(190), None, folds, 0, 40), None);
    }

    /// `n` session PILLS, one under the other.
    fn pills(n: usize) -> Column {
        let mut stack = Stack::default();
        for i in 0..n {
            stack.pill(Row::Session(i));
        }
        stack.column
    }

    /// Pills stack two rows apart, sharing their pads, and a header after
    /// them sits straight under the last one's bottom pad.
    #[test]
    fn pills_stack_on_their_stride() {
        let mut stack = Stack::default();
        stack.pill(Row::Session(0));
        stack.pill(Row::Session(1));
        stack.header("TERMINALS".into(), None);
        stack.pill(Row::Session(2));
        let tops: Vec<usize> = stack.column.iter().map(|p| p.top).collect();
        assert_eq!(tops, [0, 2, 5, 6]);
        assert_eq!(content_height(&stack.column), 9);
    }

    /// A pill with rows of its own under its text — RECENT PROMPTS, the
    /// FOLLOW-UP COMPOSER — grows by them and keeps its bottom pad, so the
    /// next one starts below it rather than sharing it.
    #[test]
    fn a_pill_with_rows_inside_keeps_its_bottom_pad() {
        let mut stack = Stack::default();
        stack.pill(Row::Session(0));
        stack.pill_with(Row::Session(1), 3);
        stack.pill(Row::Session(2));
        let at: Vec<(usize, usize)> = stack.column.iter().map(|p| (p.top, p.height)).collect();
        assert_eq!(at, [(0, 3), (2, 6), (8, 3)]);
    }

    /// The window follows the cursor down only as far as its pill needs,
    /// and back up to a cursor above it.
    #[test]
    fn the_scroll_keeps_the_cursor_on_screen() {
        let column = pills(20);
        let mut scroll = ColumnScroll::default();
        assert_eq!(scroll.settle(&column, Some(Row::Session(3)), 10), 0);
        // Pill 12 runs rows 24..27: the window ends on its bottom pad.
        assert_eq!(scroll.settle(&column, Some(Row::Session(12)), 10), 17);
        assert_eq!(scroll.settle(&column, Some(Row::Session(2)), 10), 4);
        assert_eq!(scroll.max, 41 - 10);
        let mut fresh = ColumnScroll::default();
        assert_eq!(fresh.settle(&column, None, 10), 0, "no cursor, no scroll");
    }

    /// Scrolled back up to the first row of a group, the header over it
    /// comes along.
    #[test]
    fn the_scroll_brings_the_header_along() {
        let mut stack = Stack::default();
        stack.header("TERMINALS".into(), None);
        stack.pill(Row::Session(0));
        stack.pill(Row::Session(1));
        let mut scroll = ColumnScroll {
            top: 3,
            ..Default::default()
        };
        assert_eq!(scroll.settle(&stack.column, Some(Row::Session(0)), 2), 0);
    }

    #[test]
    fn a_row_knows_its_column() {
        assert_eq!(Row::Project(2).focus(), Focus::Projects);
        assert_eq!(Row::IssuesHeader.focus(), Focus::Worktrees);
        assert_eq!(Row::ArchivedHeader.focus(), Focus::Sessions);
    }
}
