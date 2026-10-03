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
//! ([`columns`]), the lines each column lays out ([`project_lines`],
//! [`worktree_lines`], [`session_lines`]) and the scroll that keeps the
//! cursor's line on screen ([`scroll_to`]). The keys are
//! `event_loop::panels`'s and the drawing `ui::panels_view`'s.

use crate::app::{App, Focus, SessionRow, WorktreeRow};
use nebula_core::{ProjectId, WorktreeId};
use ratatui::layout::{Constraint, Layout, Rect};

/// Widths of the PROJECTS, WORKTREES and SESSIONS columns on a body wide
/// enough for them and the pane: the widths the three panels opened at
/// before they were dragged.
pub const WIDTHS: [u16; 3] = [20, 22, 32];
/// Narrowest a column is squeezed to on a narrow body.
pub const MIN_W: u16 = 10;
/// Columns the PANE always keeps, whatever the columns would like.
pub const MIN_PANE_W: u16 = 20;

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
}

impl Row {
    /// The column the row is in — the FOCUS a click on it takes.
    pub fn focus(self) -> Focus {
        match self {
            Row::Project(_) => Focus::Projects,
            Row::Worktree(_) | Row::OpenPrsHeader | Row::IssuesHeader => Focus::Worktrees,
            Row::Session(_) | Row::ArchivedHeader => Focus::Sessions,
        }
    }
}

/// One line a column lays out, top to bottom.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Line {
    /// Air between two groups.
    Blank,
    /// A group's name over its rows — `TERMINALS`, `▾ OPEN PRS · 3`.
    /// `fold` is the header's own click target, for a group that folds.
    Header { text: String, fold: Option<Row> },
    /// One row, by its place in the column's list.
    Row(Row),
}

/// The four rects of the PANELS: the three columns at [`WIDTHS`], and the
/// PANE taking every column left over. A body too narrow for that squeezes
/// the columns in proportion — down to [`MIN_W`] each — so the pane keeps
/// [`MIN_PANE_W`] for as long as the window allows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Columns {
    pub projects: Rect,
    pub worktrees: Rect,
    pub sessions: Rect,
    pub pane: Rect,
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
}

/// Lay the PANELS out over `body` ([`Columns`]).
pub fn columns(body: Rect) -> Columns {
    let want: u16 = WIDTHS.iter().sum();
    let budget = body.width.saturating_sub(MIN_PANE_W);
    let widths = WIDTHS.map(|w| {
        if budget >= want {
            w
        } else {
            (u32::from(w) * u32::from(budget) / u32::from(want)) as u16
        }
        .max(MIN_W)
    });
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
    }
}

/// The PROJECTS column: one row per project, most recently worked in
/// first — `App::project_rows`' own order, which is what `sel_project`
/// indexes.
pub fn project_lines(app: &App) -> Vec<Line> {
    (0..app.project_rows().len())
        .map(|i| Line::Row(Row::Project(i)))
        .collect()
}

/// The WORKTREES column: the project's checkouts — the ROOT WORKTREE
/// first, with a line of air under it — then the `OPEN PRS` group (its
/// pull requests, each with the checkout on its head branch nested under
/// it) and the `ISSUES` group, each under a header that folds it. A
/// folded group is its header alone, still counting what it holds.
pub fn worktree_lines(app: &App) -> Vec<Line> {
    let rows = app.worktree_rows();
    let mut lines = Vec::new();
    let mut prs_headed = false;
    let mut issues_headed = false;
    let gap = |lines: &mut Vec<Line>| {
        if !lines.is_empty() {
            lines.push(Line::Blank);
        }
    };
    for (i, row) in rows.iter().enumerate() {
        match row {
            WorktreeRow::Checkout(w) => {
                lines.push(Line::Row(Row::Worktree(i)));
                let more = rows
                    .get(i + 1)
                    .is_some_and(|r| matches!(r, WorktreeRow::Checkout(_)));
                if w.is_main && more {
                    lines.push(Line::Blank);
                }
            }
            WorktreeRow::Pr(_) | WorktreeRow::PrCheckout(_) => {
                if !prs_headed {
                    prs_headed = true;
                    gap(&mut lines);
                    lines.push(open_prs_header(app));
                }
                lines.push(Line::Row(Row::Worktree(i)));
            }
            WorktreeRow::Issue(_) => {
                if !prs_headed && app.open_prs_collapsed && !app.listed_open_prs().is_empty() {
                    prs_headed = true;
                    gap(&mut lines);
                    lines.push(open_prs_header(app));
                }
                if !issues_headed {
                    issues_headed = true;
                    gap(&mut lines);
                    lines.push(issues_header(app));
                }
                lines.push(Line::Row(Row::Worktree(i)));
            }
        }
    }
    // A folded group has no rows to have put its header up on the way.
    if !prs_headed && !app.listed_open_prs().is_empty() {
        gap(&mut lines);
        lines.push(open_prs_header(app));
    }
    if !issues_headed && !app.listed_issues().is_empty() {
        gap(&mut lines);
        lines.push(issues_header(app));
    }
    lines
}

/// `▾ OPEN PRS · 3` over the rows, `▸` with them folded away.
fn open_prs_header(app: &App) -> Line {
    Line::Header {
        text: format!(
            "{} OPEN PRS · {}",
            fold_glyph(app.open_prs_collapsed),
            app.listed_open_prs().len()
        ),
        fold: Some(Row::OpenPrsHeader),
    }
}

/// `▾ ISSUES · 2`, the same way.
fn issues_header(app: &App) -> Line {
    Line::Header {
        text: format!(
            "{} ISSUES · {}",
            fold_glyph(app.issues_collapsed),
            app.listed_issues().len()
        ),
        fold: Some(Row::IssuesHeader),
    }
}

/// The disclosure triangle: what a click on the header would do.
fn fold_glyph(folded: bool) -> &'static str {
    if folded {
        "▸"
    } else {
        "▾"
    }
}

/// The SESSIONS column: the checkout's live sessions under `RECENT`, most
/// recently touched first, then its `TERMINALS`, its `PULL REQUESTS` and
/// last the `ARCHIVED` group — open (`⇧A`, or a click on its header) or
/// folded to the one line that counts it. The order is the one
/// `App::visible_session_rows` lists, which is what `sel_session` indexes.
pub fn session_lines(app: &App) -> Vec<Line> {
    let rows = app.visible_session_rows();
    // The checkout's live and archived sessions, archived counted whether
    // the group is open or not: a folded group still says what it holds.
    let checkout = app.selected_worktree().map(|w| w.id.clone());
    let (live, archived) = app
        .tree
        .agents
        .iter()
        .filter(|a| Some(&a.worktree_id) == checkout.as_ref())
        .fold((0, 0), |(live, gone), a| {
            if a.archived {
                (live, gone + 1)
            } else {
                (live + 1, gone)
            }
        });
    let mut lines = Vec::new();
    let group = |lines: &mut Vec<Line>, title: &str, fold: Option<Row>, rows: &[usize]| {
        if !lines.is_empty() {
            lines.push(Line::Blank);
        }
        lines.push(Line::Header {
            text: title.to_string(),
            fold,
        });
        lines.extend(rows.iter().map(|i| Line::Row(Row::Session(*i))));
    };
    let of = |want: fn(&SessionRow) -> bool| -> Vec<usize> {
        rows.iter()
            .enumerate()
            .filter(|(_, row)| want(row))
            .map(|(i, _)| i)
            .collect()
    };
    let recent: Vec<usize> = (0..live.min(rows.len())).collect();
    let terminals = of(|row| matches!(row, SessionRow::Terminal(_)));
    let links = of(|row| matches!(row, SessionRow::Link(_)));
    let gone = of(SessionRow::is_archived_agent);
    if !recent.is_empty() {
        group(&mut lines, "RECENT", None, &recent);
    }
    if !terminals.is_empty() {
        group(&mut lines, "TERMINALS", None, &terminals);
    }
    if !links.is_empty() {
        group(&mut lines, "PULL REQUESTS", None, &links);
    }
    if archived > 0 {
        let title = if app.show_archived {
            format!("▾ ARCHIVED · {archived}")
        } else {
            format!("▸ ARCHIVED · {archived}")
        };
        group(&mut lines, &title, Some(Row::ArchivedHeader), &gone);
    }
    lines
}

/// The first line of `lines` to draw in a column `height` rows tall, so
/// the line holding `cursor` is on screen: the stateless follow-window
/// every overlay list scrolls by (`app::window_start`), sliding only as
/// far as the cursor's line needs. The group header over a row is the
/// line above it, so it is on screen with the row whenever there is room.
pub fn scroll_to(lines: &[Line], cursor: Option<Row>, height: usize) -> usize {
    cursor
        .and_then(|c| lines.iter().position(|l| *l == Line::Row(c)))
        .map_or(0, |at| crate::app::window_start(at, height.max(1)))
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

    /// A body with room for all three at their widths and the pane gets
    /// every column left over.
    #[test]
    fn the_columns_open_at_their_widths_and_the_pane_takes_the_rest() {
        let c = columns(body(190));
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
        let c = columns(body(80));
        assert!(c.pane.width >= MIN_PANE_W, "{c:?}");
        for w in [c.projects.width, c.worktrees.width, c.sessions.width] {
            assert!((MIN_W..32).contains(&w), "{c:?}");
        }
    }

    fn rows(n: usize) -> Vec<Line> {
        (0..n).map(|i| Line::Row(Row::Session(i))).collect()
    }

    /// The window follows the cursor down and is back at the top for a
    /// cursor that fits there.
    #[test]
    fn the_scroll_keeps_the_cursor_on_screen() {
        let lines = rows(20);
        assert_eq!(scroll_to(&lines, Some(Row::Session(3)), 5), 0);
        assert_eq!(scroll_to(&lines, Some(Row::Session(12)), 5), 8);
        assert_eq!(scroll_to(&lines, None, 5), 0, "no cursor, no scroll");
    }

    #[test]
    fn a_row_knows_its_column() {
        assert_eq!(Row::Project(2).focus(), Focus::Projects);
        assert_eq!(Row::IssuesHeader.focus(), Focus::Worktrees);
        assert_eq!(Row::ArchivedHeader.focus(), Focus::Sessions);
    }
}
