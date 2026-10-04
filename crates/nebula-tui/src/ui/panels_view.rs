//! The PANELS' drawing (`crate::panels` is their model,
//! `event_loop::panels` their keys): PROJECTS | WORKTREES | SESSIONS down
//! the left of the body, each column a header over a list — PROJECTS as
//! 3-row buttons, WORKTREES and SESSIONS as 2-row PILLS, each with a
//! STATUS DOT, the name, how long since it moved and what it runs on —
//! the cursor's row raised on the selection fill with its rail in the
//! row's own color, and the PANE beside them reading the session under
//! the SESSIONS cursor (`draw_terminal`, the very pane the GRID's
//! full-screen session is drawn into). The focused column wears the FOCUS
//! TINT, as the pane does when it has the keys.
//!
//! A session's pill carries its RECENT PROMPTS under its text and, once
//! Space or its FOLLOW-UP CHEVRON expands it, the FOLLOW-UP COMPOSER under
//! those: the pill grows around them, and the pills below move down.
//!
//! Every row registers a `HitTarget::PanelsRow` ahead of its column's
//! `PanelBg`, so a click lands on the row and a click on the air under the
//! rows only takes FOCUS; a session's chevron and its open composer
//! register ahead of their row. Each column's BORDER — its rule and the cell
//! after it — registers a `HitTarget::PanelsBorder` ahead of them all, and
//! wears a grip that lights while it is hovered or dragged.

use super::{
    ago_badge, draw_focus_tint, draw_terminal, fit_ago, key_hint, multiline_input_lines,
    open_counts_badge, render_button, row_bar, row_rect, selection_mark, status_color, status_dot,
    status_name_spans, sweep_ramp, truncate, HelpSection, MIN_NAME_W, PENDING_SESSION_BADGE,
};
use crate::app::{App, Focus, HitTarget, SessionRow, WorktreeRow};
use crate::keymap::Action;
use crate::panels::{Fold, Line as PanelLine, Placed, Row, PILL_CELL, PILL_H, PROJECT_BTN_H};
use crate::theme::Theme;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;

/// Left gutter every row gets from its one-column selection marker plus
/// its two-column STATUS DOT: a column's title takes the same indent, so
/// it lines up with the names under it.
const ROW_GUTTER: &str = "   ";
/// What a checkout's row says while git is still cutting it.
const PENDING_WORKTREE_BADGE: &str = " creating";
/// The ROOT WORKTREE's badge, and the glyph alone on a narrow column.
const ROOT_BADGE: &str = " ⌂ root";
const ROOT_GLYPH: &str = " ⌂";
/// The RUNNING badge of a checkout whose RUN COMMAND is up (`r`), and the
/// bare glyph where the word would cut the branch. The glyph never
/// yields: it is the one thing on the row that says a process is serving
/// from this checkout.
const RUN_BADGE: &str = " ▶ running";
const RUN_GLYPH: &str = " ▶";
/// What hangs a checkout under the pull request on its head branch.
const NESTED_INDENT: &str = "└";
/// A selected PILL's pads, top and bottom: half blocks in the fill's
/// color, so the fill reads as a rounded slab about two rows tall.
const PILL_HALF: (char, char) = ('▄', '▀');
/// The selection rail owns the pill's first column outright: a solid `█`
/// on the text row, the pad's own `PILL_HALF` glyph on the pads. A
/// half-width `▌` can't run the pill's full height — a cell holds one
/// glyph and two colors, so a quadrant cap on a pad row strands the fill
/// quarter beside it on bare background, which the FOCUS TINT turns into
/// a dark notch at each of the pill's left corners.
const PILL_RAIL: &str = "█";
/// Rows a BORDER's grip runs down the middle of its rule: the LAUNCHER
/// VIEW's pane grip beside the cards is as tall.
const GRIP_H: u16 = 4;
/// Narrowest a column's title row carries its fold chevron at: any
/// narrower and the `◀` would sit on the title.
const FOLD_MIN_W: u16 = 16;
/// A session pill's FOLLOW-UP CHEVRON, at the end of its text row: folded,
/// and expanded into its FOLLOW-UP COMPOSER.
const FOLLOW_UP_FOLDED: &str = " ▸";
const FOLLOW_UP_OPEN: &str = " ▾";
/// What a RECENT PROMPTS line opens with, after the pill's rail column: a
/// column to land under the name (past the STATUS DOT), and a bullet so
/// the lines read as a list hanging off the row rather than as more rows.
const PROMPT_INDENT: &str = " · ";

/// The PANELS over `body`: the three columns and the pane beside them, a
/// folded column drawn as its RAIL or its bare RULE (`panels::Fold`).
pub(super) fn draw(f: &mut Frame, app: &mut App, body: Rect) {
    app.body_area = body;
    // A FOCUS the cursor's own move has folded SESSIONS out from under —
    // onto a pull request, say — steps off it before the columns are laid
    // out, so the tint and the keys land on an open column.
    crate::panels::settle_focus(app);
    let cols = crate::panels::layout(app, body);
    // The BORDERS first, so they win `hit_at`'s first-match scan against
    // the row, or the pane, a grab cell lands on.
    for i in 0..3 {
        if let Some(zone) = cols.grab_zone(i) {
            app.hits.push((zone, HitTarget::PanelsBorder(i)));
        }
    }
    let [projects, worktrees, sessions] = cols.folds;
    match projects {
        Fold::Open => draw_projects(f, app, cols.projects),
        fold => draw_fold(f, app, cols.projects, Focus::Projects, fold),
    }
    match worktrees {
        Fold::Open => draw_worktrees(f, app, cols.worktrees),
        fold => draw_fold(f, app, cols.worktrees, Focus::Worktrees, fold),
    }
    match sessions {
        Fold::Open => draw_sessions(f, app, cols.sessions),
        fold => draw_fold(f, app, cols.sessions, Focus::Sessions, fold),
    }
    draw_terminal(f, app, cols.pane);
    // The FOCUS TINT on the column with the keys — inside its rule, so the
    // rule stays the boundary between two columns rather than part of one.
    let tinted = match app.focus {
        Focus::Terminal => cols.pane,
        column => {
            let area = cols.of(column);
            Rect {
                width: area.width.saturating_sub(1),
                ..area
            }
        }
    };
    draw_focus_tint(f.buffer_mut(), tinted, app.theme);
    draw_grips(f.buffer_mut(), app, &cols, body);
}

/// The grip on each column's rule: a short heavy stretch down its middle,
/// the one visible sign that the BORDER can be dragged — muted at rest,
/// the accent while the pointer rests on it or while it is being dragged,
/// as the LAUNCHER VIEW's pane grip is.
fn draw_grips(
    buf: &mut ratatui::buffer::Buffer,
    app: &App,
    cols: &crate::panels::Columns,
    body: Rect,
) {
    let th = app.theme;
    if body.height < GRIP_H + 2 {
        return; // no room for the grip and rule either side of it
    }
    let top = body.y + (body.height - GRIP_H) / 2;
    // A folded column has no BORDER to drag, and so no grip.
    for i in (0..3).filter(|i| cols.folds[*i] == Fold::Open) {
        let x = cols.border(i).saturating_sub(1);
        let active =
            app.panels_drag.map(|(at, _)| at) == Some(i) || app.hover_panels_border == Some(i);
        let fg = if active { th.accent } else { th.muted };
        for y in top..top + GRIP_H {
            if let Some(cell) = buf.cell_mut((x, y)) {
                cell.set_symbol("┃");
                cell.set_style(Style::default().fg(fg));
            }
        }
    }
}

/// A folded column: its rule alone, and for a RAIL the `▶` over it on
/// the title row, the whole strip a click that opens the column again
/// at the width it was left at. The bare RULE SESSIONS folds to of its own
/// accord has no chevron and no target: it opens the same way it folded,
/// and a click there would only write a fold the user never chose.
fn draw_fold(f: &mut Frame, app: &mut App, area: Rect, focus: Focus, fold: Fold) {
    let th = app.theme;
    f.render_widget(
        Block::default()
            .borders(Borders::RIGHT)
            .border_style(Style::default().fg(th.edge)),
        area,
    );
    if fold != Fold::Rail {
        return;
    }
    if area.height > 1 {
        f.render_widget(
            Paragraph::new(Span::styled("▶", Style::default().fg(th.dim))),
            Rect {
                y: area.y + 1,
                width: 1,
                height: 1,
                ..area
            },
        );
    }
    app.hits.push((area, HitTarget::PanelsFold(focus)));
}

/// A column's frame: its rule down the right, a blank row, the title with
/// its count and, at the row's right end, the `◀` that folds the column
/// to its RAIL, then a blank row. Returns the rect its list fills, one
/// column short of the rule so a row's text never touches it.
#[allow(clippy::too_many_arguments)]
fn column(
    f: &mut Frame,
    area: Rect,
    title: &str,
    count: usize,
    focused: bool,
    th: Theme,
    focus: Focus,
    hits: &mut Vec<(Rect, HitTarget)>,
) -> Rect {
    let block = Block::default()
        .borders(Borders::RIGHT)
        .border_style(Style::default().fg(th.edge));
    let inner = block.inner(area);
    f.render_widget(block, area);
    let title_style = if focused {
        Style::default().fg(th.accent).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(th.muted).add_modifier(Modifier::BOLD)
    };
    if let Some(r) = row_rect(inner, 1) {
        let mut spans = vec![Span::styled(format!("{ROW_GUTTER}{title}"), title_style)];
        if count > 0 {
            spans.push(Span::styled(
                format!(" · {count}"),
                Style::default().fg(th.dim),
            ));
        }
        f.render_widget(Paragraph::new(Line::from(spans)), r);
        // A title row too narrow for it keeps the title alone: the
        // chevron would land on its text.
        if r.width >= FOLD_MIN_W {
            let glyph = Rect {
                x: r.x + r.width - 3,
                width: 2,
                ..r
            };
            f.render_widget(
                Paragraph::new(Span::styled("◀", Style::default().fg(th.dim))),
                glyph,
            );
            hits.push((glyph, HitTarget::PanelsFold(focus)));
        }
    }
    Rect {
        y: inner.y + 3,
        height: inner.height.saturating_sub(3),
        width: inner.width.saturating_sub(1),
        ..inner
    }
}

/// An empty column's one-line nudge: accent keys and dim prose
/// alternating, the first key in the row gutter so it lines up with row
/// text. Every key is the live keymap's chord for its action.
fn hint(f: &mut Frame, app: &App, list: Rect, pairs: &[(Action, &str)]) {
    let th = app.theme;
    let mut spans = vec![Span::raw(ROW_GUTTER)];
    for (action, prose) in pairs {
        spans.push(Span::styled(
            key_hint(app, *action),
            Style::default().fg(th.accent),
        ));
        spans.push(Span::styled(*prose, Style::default().fg(th.dim)));
    }
    f.render_widget(Paragraph::new(Line::from(spans)), list);
}

/// How a column's rows are drawn: PROJECTS as buttons, the name on the
/// middle of their rows; WORKTREES and SESSIONS as PILLS.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Shape {
    Button,
    Pill,
}

/// What a column's `row` closure draws one row as: its lines of spans — a
/// pill takes the first, a button one per text row — the color its
/// selection rail takes, and whether the first line's last span is a
/// session's FOLLOW-UP CHEVRON, a click target of its own.
type RowText = (Vec<Vec<Span<'static>>>, Color, bool);

/// A column's list: `column`'s entries from the row it is scrolled to
/// (`panels::ColumnScroll`, which brings the `reveal` row back on screen
/// when it moves — the `cursor`'s, or the pill whose FOLLOW-UP COMPOSER
/// has the keys), each row drawn by `row` ([`RowText`]), a session pill's
/// RECENT PROMPTS and composer under it ([`draw_pill_inside`]). Registers
/// each row, and each header that folds, as a `PanelsRow` — a session's
/// chevron and open composer ahead of it — then the whole column as its
/// `PanelBg`.
#[allow(clippy::too_many_arguments)]
fn draw_list(
    f: &mut Frame,
    app: &mut App,
    area: Rect,
    list: Rect,
    column: &[Placed],
    cursor: Option<Row>,
    reveal: Option<Row>,
    focus: Focus,
    shape: Shape,
    mut row: impl FnMut(&App, Row, usize) -> RowText,
) {
    let th = app.theme;
    let focused = app.focus == focus;
    let height = usize::from(list.height);
    let top = app.panels_scroll[crate::panels::scroll_slot(focus)].settle(column, reveal, height);
    let width = usize::from(list.width);
    // SESSIONS' rows, read once a pill has something hanging in it.
    let mut sessions: Option<Vec<SessionRow>> = None;
    for (at, placed) in column.iter().enumerate() {
        let y = placed.top as isize - top as isize;
        if y >= height as isize {
            break;
        }
        match &placed.line {
            PanelLine::Header { text, fold } => {
                let Some(r) = rows_at(list, y, 1) else {
                    continue;
                };
                f.render_widget(
                    Paragraph::new(Span::styled(
                        format!(" {text}"),
                        Style::default().fg(th.dim),
                    )),
                    r,
                );
                if let Some(fold) = fold {
                    app.hits.push((r, HitTarget::PanelsRow(*fold)));
                }
            }
            PanelLine::Row(at_row) => {
                if y + placed.height as isize <= 0 {
                    continue;
                }
                let (text, mark, chevron) = row(app, *at_row, width);
                let selected = Some(*at_row) == cursor;
                let hit = match shape {
                    Shape::Button => {
                        let Some(r) = rows_at(list, y, placed.height) else {
                            continue;
                        };
                        // A button scrolled part-way off the top keeps the
                        // rows still on screen: its text row moves up with
                        // it, and lines of text above the edge go.
                        let skip = (-y).max(0) as usize;
                        let middle = PROJECT_BTN_H / 2;
                        let (text, text_row) = if skip <= middle {
                            (text, middle - skip)
                        } else {
                            (text.into_iter().skip(skip - middle).collect(), 0)
                        };
                        render_button(f, r, text, selected, focused, th, text_row as u16, mark);
                        Some(r)
                    }
                    Shape::Pill => {
                        let spans = text.into_iter().next().unwrap_or_default();
                        // The chevron's cells: past the rail column, after
                        // every span ahead of it.
                        let chevron =
                            spans
                                .split_last()
                                .filter(|_| chevron)
                                .map(|(glyph, ahead)| {
                                    let x = 1 + ahead.iter().map(Span::width).sum::<usize>();
                                    (x, glyph.width())
                                });
                        let inside = placed.height.saturating_sub(PILL_CELL);
                        render_pill(f, list, y, spans, selected, focused, th, mark, inside);
                        if let Row::Session(i) = *at_row {
                            let bar = selected.then(|| pill_bar(focused, mark, th));
                            if inside > 0 {
                                let rows =
                                    sessions.get_or_insert_with(|| app.visible_session_rows());
                                draw_pill_inside(f, app, list, y, rows.get(i), inside, bar);
                            }
                            if let (Some((x, w)), Some(r)) = (chevron, rows_at(list, y + 1, 1)) {
                                let x = (x as u16).min(r.width);
                                let cell = Rect {
                                    x: r.x + x,
                                    width: (w as u16).min(r.width - x),
                                    ..r
                                };
                                app.hits
                                    .push((cell, HitTarget::PanelsRow(Row::FollowUp(i))));
                            }
                        }
                        // The pad two pills share is the lower one's: its
                        // fill owns that cell's bottom half. A pill with
                        // nothing stacked onto it — or rows of its own
                        // under its text — keeps its bottom pad.
                        let next = column.get(at + 1).map(|p| p.top);
                        let cell = placed.height;
                        let rows = next.map_or(cell, |n| n.saturating_sub(placed.top).min(cell));
                        rows_at(list, y, rows)
                    }
                };
                if let Some(r) = hit {
                    app.hits.push((r, HitTarget::PanelsRow(*at_row)));
                }
            }
        }
    }
    app.hits.push((area, HitTarget::PanelBg(focus)));
}

/// `rows` rows of `list` from row `y`, which may sit above the list's top
/// once it has scrolled: clipped to what is still on screen, None when
/// none of it is.
fn rows_at(list: Rect, y: isize, rows: usize) -> Option<Rect> {
    let visible = rows as isize + y.min(0);
    if visible <= 0 || y >= list.height as isize {
        return None;
    }
    let y = list.y + y.max(0) as u16;
    Some(Rect {
        y,
        height: (visible as u16).min(list.bottom() - y),
        ..list
    })
}

/// The fill and rail of a selected PILL, `(fill, rail)`: in the focused
/// column the raised `sel_bg` with the row's `mark` on the rail (see
/// `selection_mark`); elsewhere the barely-raised `sel_bg_dim` under a
/// dim rail, so an unfocused cursor reads as a place, not a signal.
fn pill_bar(focused: bool, mark: Color, th: Theme) -> (Color, Color) {
    if focused {
        (th.sel_bg, selection_mark(mark, th))
    } else {
        (th.sel_bg_dim, th.dim)
    }
}

/// One PILL from row `top` of `list`: half-block pad, `spans`, `inside`
/// rows left for what hangs under the text (a session's RECENT PROMPTS and
/// FOLLOW-UP COMPOSER, [`draw_pill_inside`]), half-block pad — the pads
/// drawn only under the cursor, running the full width so the fill has no
/// dark notch beside the STATUS DOT, the rail column carrying the pad's own
/// half block in the rail's color so the rail spans the pill's whole
/// height. Dim spans are lifted to muted on the fill, as `render_button`
/// lifts them.
#[allow(clippy::too_many_arguments)]
fn render_pill(
    f: &mut Frame,
    list: Rect,
    top: isize,
    mut spans: Vec<Span>,
    selected: bool,
    focused: bool,
    th: Theme,
    mark: Color,
    inside: usize,
) {
    let (fill, rail) = pill_bar(focused, mark, th);
    if selected {
        let mut pad = |glyph: char, row: isize| {
            if let Some(r) = rows_at(list, row, 1) {
                f.render_widget(
                    Paragraph::new(Span::styled(
                        glyph.to_string().repeat(usize::from(list.width)),
                        Style::default().fg(fill),
                    )),
                    r,
                );
                // Same half block, rail-colored: the rail's cap and the
                // fill quarter beside it are one cell, so they have to be
                // one color, and the rail is the one worth keeping.
                f.render_widget(
                    Paragraph::new(Span::styled(glyph.to_string(), Style::default().fg(rail))),
                    Rect { width: 1, ..r },
                );
            }
        };
        pad(PILL_HALF.0, top);
        pad(PILL_HALF.1, top + (PILL_H + inside) as isize);
    }
    let Some(text_row) = rows_at(list, top + 1, 1) else {
        return;
    };
    let marker = if selected {
        for s in &mut spans {
            if s.style.fg == Some(th.dim) {
                s.style.fg = Some(th.muted);
            }
        }
        Span::styled(PILL_RAIL, Style::default().fg(rail))
    } else {
        Span::raw(" ")
    };
    spans.insert(0, marker);
    f.render_widget(
        Paragraph::new(Line::from(spans)).style(row_bar(selected, focused, th)),
        text_row,
    );
}

/// What hangs inside session `row`'s pill from row `top` of `list`,
/// in the `inside` rows it grew by: its RECENT PROMPTS straight under its
/// text, then — on the pill that is expanded — its FOLLOW-UP COMPOSER,
/// which registers ahead of the row so a click in what is being typed
/// never reads as a second click on the pill. `bar` is the pill's
/// `(fill, rail)` from [`pill_bar`] when it is the cursor's: the lines then
/// sit on its fill and carry the rail down their first column, so the
/// pill and what hangs in it are one slab.
fn draw_pill_inside(
    f: &mut Frame,
    app: &mut App,
    list: Rect,
    top: isize,
    row: Option<&SessionRow>,
    inside: usize,
    bar: Option<(Color, Color)>,
) {
    let th = app.theme;
    let Some(row @ SessionRow::Agent(a)) = row else {
        return;
    };
    let prompts = crate::panels::prompt_lines(app, row);
    let follow_up = inside.saturating_sub(prompts);
    let first_row = top + PILL_H as isize;
    if prompts > 0 {
        draw_prompt_lines(f, list, first_row, &a.recent_prompts, prompts, bar, th);
    }
    if follow_up > 0 {
        let first_row = first_row + prompts as isize;
        draw_follow_up_box(f, app, list, first_row, follow_up, bar, th);
        if let Some(r) = rows_at(list, first_row, follow_up) {
            app.hits.push((r, HitTarget::PanelsRow(Row::FollowUpBox)));
        }
    }
}

/// The RECENT PROMPTS under a session's name, from `first_row`: the
/// newest `count` of `prompts`, oldest first so the bottom line is the
/// latest thing asked, each clipped to fit with its ago label pinned
/// right. Dim, with the newest lifted to muted so the eye lands on it —
/// these are context for the row, not rows of their own. On the cursor's
/// pill they take `bar` as [`draw_pill_inside`] says, dim lifting to muted
/// on the fill the way the pill's own dim spans do.
fn draw_prompt_lines(
    f: &mut Frame,
    list: Rect,
    first_row: isize,
    prompts: &[nebula_core::PromptEntry],
    count: usize,
    bar: Option<(Color, Color)>,
    th: Theme,
) {
    let skip = prompts.len().saturating_sub(count);
    let free = usize::from(list.width).saturating_sub(1 + PROMPT_INDENT.chars().count());
    let lift = |color: Color| match bar {
        Some(_) if color == th.dim => th.muted,
        _ => color,
    };
    let base = bar.map_or_else(Style::default, |(fill, _)| Style::default().bg(fill));
    let marker = match bar {
        Some((_, rail)) => Span::styled(PILL_RAIL, Style::default().fg(rail)),
        None => Span::raw(" "),
    };
    for (i, entry) in prompts.iter().skip(skip).enumerate() {
        let Some(area) = rows_at(list, first_row + i as isize, 1) else {
            continue;
        };
        let newest = skip + i + 1 == prompts.len();
        let text_color = lift(if newest { th.muted } else { th.dim });
        let (ago, text_max) = fit_ago(ago_badge(entry.submitted_at), free);
        let text = truncate(&entry.text, text_max);
        let mut spans = vec![
            marker.clone(),
            Span::styled(PROMPT_INDENT, Style::default().fg(lift(th.dim))),
            Span::styled(text.clone(), Style::default().fg(text_color)),
        ];
        if !ago.is_empty() {
            let gap = text_max.saturating_sub(text.chars().count());
            spans.push(Span::raw(" ".repeat(gap)));
            spans.push(Span::styled(ago, Style::default().fg(lift(th.dim))));
        }
        f.render_widget(Paragraph::new(Line::from(spans)).style(base), area);
    }
}

/// The FOLLOW-UP COMPOSER, `rows` tall from `first_row` inside its pill: a
/// framed box with `follow-up` on its top border, the turn being typed
/// inside it, and the keys that send it on the bottom one — drawn a row at
/// a time so a pill straddling the top of the column loses only the rows
/// that scrolled off. On the cursor's pill it takes `bar` as
/// [`draw_pill_inside`] says; the frame is the accent — this is where the
/// keyboard is pointed.
fn draw_follow_up_box(
    f: &mut Frame,
    app: &App,
    list: Rect,
    first_row: isize,
    rows: usize,
    bar: Option<(Color, Color)>,
    th: Theme,
) {
    let Some(follow_up) = &app.follow_up else {
        return;
    };
    let base = bar.map_or_else(Style::default, |(fill, _)| Style::default().bg(fill));
    let marker = match bar {
        Some((_, rail)) => Span::styled(PILL_RAIL, Style::default().fg(rail)),
        None => Span::raw(" "),
    };
    let frame = Style::default().fg(th.accent);
    // Everything but the rail column belongs to the box.
    let box_w = usize::from(list.width).saturating_sub(1).max(2);
    let text_w = crate::panels::follow_up_text_width(list.width);
    let put = |f: &mut Frame, row: isize, mut spans: Vec<Span<'static>>| {
        if let Some(r) = rows_at(list, row, 1) {
            spans.insert(0, marker.clone());
            f.render_widget(Paragraph::new(Line::from(spans)).style(base), r);
        }
    };

    // Top border: ╭─ follow-up ──────╮
    let title = truncate(" follow-up ", box_w.saturating_sub(3));
    let fill = box_w.saturating_sub(3 + title.chars().count());
    put(
        f,
        first_row,
        vec![Span::styled(
            format!("╭─{title}{}╮", "─".repeat(fill)),
            frame,
        )],
    );

    // The text, windowed on the caret the way every other box windows it.
    let visible = rows.saturating_sub(2).max(1);
    let (lines, caret_row) = multiline_input_lines(&follow_up.input, text_w, th.accent, th);
    let max_start = lines.len().saturating_sub(visible);
    let start = caret_row.saturating_sub(visible / 2).min(max_start);
    for i in 0..visible {
        let mut spans = vec![Span::styled("│ ", frame)];
        let mut used = 0usize;
        if let Some(line) = lines.get(start + i) {
            for span in &line.spans {
                used += span.content.chars().count();
                spans.push(Span::styled(span.content.to_string(), span.style));
            }
        }
        spans.push(Span::raw(" ".repeat(text_w.saturating_sub(used))));
        spans.push(Span::styled(" │", frame));
        put(f, first_row + 1 + i as isize, spans);
    }

    // Bottom border, carrying the keys: ╰─ ↵ send · ^J nl · Esc ─╯
    let hint = follow_up_hint(box_w);
    let fill = box_w.saturating_sub(3 + hint.chars().count());
    put(
        f,
        first_row + rows as isize - 1,
        vec![Span::styled(
            format!("╰─{hint}{}╯", "─".repeat(fill)),
            frame,
        )],
    );
}

/// The keys on the composer's bottom border, widest that fits `width` (the
/// box's own, borders included). The column is narrow and a hint wider
/// than its border is silently chopped, so this steps down.
fn follow_up_hint(width: usize) -> &'static str {
    if width >= 32 {
        " ↵ send · ⇧↵ newline · Esc close "
    } else if width >= 24 {
        " ↵ send · ^J nl · Esc "
    } else if width >= 14 {
        " ↵ · ^J · Esc "
    } else {
        ""
    }
}

/// `⇧P: show projects  ⇧B: show worktrees`: the key that opens each
/// column folded to its RAIL by hand, for the footer to lead its hints
/// with. None off the PANELS, and with no column folded — SESSIONS'
/// own RULE beside a pull request opens of itself and needs no key.
pub(super) fn restore_hints(app: &App) -> Option<String> {
    if !app.panels_active() {
        return None;
    }
    let hints: Vec<String> = ["projects", "worktrees", "sessions"]
        .iter()
        .zip(crate::panels::FOLD_KEYS)
        .zip(app.panels_hidden)
        .filter(|(_, hidden)| *hidden)
        .map(|((column, key), _)| format!("{}: show {column}", key.display()))
        .collect();
    (!hints.is_empty()).then(|| hints.join("  "))
}

/// `m: menu  ` for the footer's hints on a PANELS column, before its
/// `?: help`, as they always ended there: `m` opens the cursor row's menu
/// (`panels::PanelKey::ContextMenu`). Empty off the PANELS, where no key
/// does.
pub(super) fn menu_hint(app: &App) -> &'static str {
    if app.panels_active() {
        "m: menu  "
    } else {
        ""
    }
}

/// The `?` overlay's two columns while the PANELS are up: the sections
/// and the wording they always had there, a row per key the columns
/// answer to. Every chord the keymap carries is the live keymap's, as the
/// grid's help is; the PANELS' own keys (`panels::PANEL_KEYS`,
/// `panels::FOLD_KEYS`) are fixed, and spelled as such.
pub(super) fn help_sections() -> (&'static [HelpSection], &'static [HelpSection]) {
    use super::HelpKeys::{Act, Also, Lit};
    use Action::*;
    const LEFT: &[HelpSection] = &[
        (
            "NAVIGATE & SEARCH",
            &[
                (Also(&[FocusNext], "^⇧L"), "walk panels (fwd locks input)"),
                (Lit("⇧Tab ^⇧H"), "walk panels back"),
                (
                    Act(&[FocusLeft, FocusRight]),
                    "focus left / right (2×: jump)",
                ),
                (Act(&[MoveDown, MoveUp]), "move selection"),
                (Act(&[Activate]), "drill in / attach session"),
                (Act(&[Palette]), "fuzzy jump to anything"),
                (Lit("^o / ^f"), "jump pick: open / focus row"),
                (
                    Act(&[NextProjectTab, PrevProjectTab]),
                    "next/prev session needing you",
                ),
                (Act(&[FindFile]), "find file (^y copies path)"),
                (Act(&[Grep]), "find in files (git grep)"),
                (Act(&[TreeBrowser]), "file tree browser"),
            ],
        ),
        (
            "PROJECTS",
            &[
                (Act(&[New, AddProject]), "add project (2nd: from anywhere)"),
                (Act(&[Rename]), "rename row (folder keeps its name)"),
                (Act(&[Delete]), "remove from list"),
            ],
        ),
        (
            "WORKTREES",
            &[
                (Act(&[New]), "new worktree (PR row: Claude)"),
                (Act(&[Rename]), "run / stop the project's run command"),
                (Act(&[OpenWorktree]), "fire its open command"),
                (Act(&[HalfPageDown, HalfPageUp]), "half a panel down / up"),
                (Act(&[GitDiff]), "git diff (^r: mark reviewed ✓, ^t: tree)"),
                (
                    Also(&[OpenRepo, OpenGhosttyTab], "⇧C"),
                    "repo on GitHub / Ghostty tab",
                ),
                (Act(&[RefreshPullRequests]), "refresh pull requests now"),
                (
                    Act(&[CommentPullRequest]),
                    "comment on the pull request row",
                ),
                (Act(&[Issues]), "github issues: prompt / preset / edit one"),
                (
                    Act(&[PullRequests]),
                    "pull requests: read one, launch a PR session on it",
                ),
                (Act(&[SwitchBranch]), "switch the ⌂ root checkout's branch"),
                (Act(&[Delete, DeleteAll]), "delete one / delete all"),
            ],
        ),
        (
            // Every typed field — names, filters, queries — is the same
            // line editor (text_input.rs).
            "TYPING IN A FIELD",
            &[
                (Lit("←→ / ⌥←→"), "move by character / by word"),
                (Lit("^a^e ⌥⌫ ^u^k"), "ends · del word · kill line"),
            ],
        ),
    ];
    const RIGHT: &[HelpSection] = &[
        (
            "SESSIONS",
            &[
                (Act(&[New]), "new agent (pick CLI kind)"),
                (Act(&[AgentPresets]), "agent presets: saved launches"),
                (Act(&[NewTerminal]), "new shell terminal"),
                (Act(&[Activate]), "attach session / open link"),
                (Act(&[HalfPageDown, HalfPageUp]), "half a panel down / up"),
                (Act(&[Rename]), "rename agent / edit link URL"),
                (
                    Act(&[Archive, Unarchive, ToggleArchived]),
                    "archive / unarchive / show",
                ),
                (Lit("m"), "context menu (right-click)"),
                (
                    Act(&[OpenPullRequest, OpenIssue]),
                    "its PR / issue on GitHub",
                ),
                (Act(&[Delete, DeleteAll]), "delete one / delete all"),
            ],
        ),
        (
            "TERMINAL & MOUSE",
            &[
                (Also(&[Activate], "/ z"), "lock input (2nd: full-screen)"),
                (Act(&[UnlockTerminal]), "unlock, back to panels"),
                (Act(&[ToggleFullScreen]), "full-screen / back to the pane"),
                (Lit("drag"), "select + copy (2×click: word)"),
                (Lit("click / drag"), "an app that took the mouse gets it"),
                (Lit("⌥click"), "open URL / file under cursor"),
                (Lit("⇧drag"), "select via your terminal"),
                (Lit("drag border"), "resize panels"),
                (Lit("click outside"), "dismiss any modal (= Esc)"),
            ],
        ),
        (
            "GENERAL",
            &[
                (
                    Lit("⇧P / ⇧B / ⇧S"),
                    "collapse / expand Projects / Worktrees / Sessions",
                ),
                (Lit("^b ⇧Z"), "collapse every panel / bring them back"),
                (Act(&[Hosts]), "ssh hosts: connect (a: new, d: del)"),
                (Act(&[Settings]), "settings (Hotkeys tab rebinds these)"),
                (Act(&[Metrics]), "memory usage (nebula + agents)"),
                (Act(&[Quit, Help]), "quit / toggle this help"),
            ],
        ),
    ];
    (LEFT, RIGHT)
}

/// PROJECTS: every project on the machine, the one last worked in first —
/// what the PROJECT TABS are in the GRID, with the cursor's button the lit
/// tab.
fn draw_projects(f: &mut Frame, app: &mut App, area: Rect) {
    let th = app.theme;
    let count = app.project_rows().len();
    let focused = app.focus == Focus::Projects;
    let list = column(
        f,
        area,
        "PROJECTS",
        count,
        focused,
        th,
        Focus::Projects,
        &mut app.hits,
    );
    let lines = crate::panels::project_lines(app);
    let cursor = Some(Row::Project(app.sel_project));
    let rows = app.project_rows();
    let now = crate::app::now_ms();
    draw_list(
        f,
        app,
        area,
        list,
        &lines,
        cursor,
        cursor,
        Focus::Projects,
        Shape::Button,
        |app, at, width| {
            let Row::Project(i) = at else {
                return (Vec::new(), th.accent, false);
            };
            let Some(p) = rows.get(i).and_then(|i| app.tree.projects.get(*i)) else {
                return (Vec::new(), th.accent, false);
            };
            let (text, mark) = project_row(app, p, width, now);
            (text, mark, false)
        },
    );
}

/// One project's button: its rolled-up STATUS DOT, its name in bold — the
/// top of the tree reads biggest — how long since anything under it
/// moved, its PR & ISSUE COUNTS and the finishes under it nobody has
/// read; then, for a renamed project, the folder it still lives in on
/// the row under its name.
fn project_row(
    app: &App,
    p: &nebula_core::Project,
    width: usize,
    now: i64,
) -> (Vec<Vec<Span<'static>>>, Color) {
    let th = app.theme;
    let roll = crate::app::project_rollup(&app.tree, &p.id);
    let unseen = crate::app::project_unseen(&app.tree, &p.id);
    let fresh = crate::panels::project_fresh_done(app, &p.id);
    let stamped = crate::app::project_recency(&app.tree, &p.id, now).stamped;
    let badge = unseen_badge(unseen);
    let mut free = width.saturating_sub(3 + badge.chars().count());
    // The counts take their columns ahead of the ago label, and drop out
    // whole, as the label does, before the name would be squeezed under
    // `MIN_NAME_W`.
    let counts = open_counts_badge(app.project_open_counts(&p.id), th).filter(|(_, len)| {
        free.checked_sub(*len)
            .is_some_and(|rest| rest >= MIN_NAME_W)
    });
    if let Some((_, len)) = &counts {
        free -= len;
    }
    // With counts after it the label ends in ` -`, so `3m ago - 4 prs`
    // reads as two facts rather than one run of words; the dash is the
    // label's and goes when the label goes.
    let mut ago = ago_badge(stamped);
    if counts.is_some() && !ago.is_empty() {
        ago.push_str(" -");
    }
    let (ago, name_max) = fit_ago(ago, free);
    let mut spans = vec![status_dot(roll, unseen > 0, th)];
    spans.extend(status_name_spans(
        truncate(&p.name, name_max),
        Style::default().add_modifier(Modifier::BOLD),
        sweep_ramp(roll, fresh, th, app.animations),
        app.sweep_phase(),
    ));
    push_dim(&mut spans, ago, th);
    if let Some((counts, _)) = counts {
        spans.extend(
            counts
                .into_iter()
                .map(|(text, style, _)| Span::styled(text, style)),
        );
    }
    spans.push(Span::styled(badge, Style::default().fg(th.done)));
    let mut text = vec![spans];
    // Renaming a project is a label change, never a move on disk, so the
    // folder keeps its name on the row under the label — as its child,
    // not a second label: the dimmest color plus faint, hung off the name
    // by a `└`.
    if let Some(folder) = p.folder_subtitle() {
        text.push(vec![
            Span::raw("  "),
            Span::styled(
                format!("└ {}", truncate(&folder, width.saturating_sub(5))),
                Style::default().fg(th.dim).add_modifier(Modifier::DIM),
            ),
        ]);
    }
    (text, status_color(roll, unseen > 0, th))
}

/// WORKTREES: the selected project's checkouts, then its open pull
/// requests and issues under the headers that fold them
/// (`panels::worktree_lines`). A project with none of them says which key
/// starts one.
fn draw_worktrees(f: &mut Frame, app: &mut App, area: Rect) {
    let th = app.theme;
    let count = app.visible_worktrees().len();
    let focused = app.focus == Focus::Worktrees;
    let list = column(
        f,
        area,
        "WORKTREES",
        count,
        focused,
        th,
        Focus::Worktrees,
        &mut app.hits,
    );
    // The page Ctrl+d / Ctrl+u halve: the pills the column has room for.
    app.worktrees_view_rows = usize::from(list.height) / PILL_H;
    let lines = crate::panels::worktree_lines(app);
    if lines.is_empty() && app.selected_project().is_some() {
        hint(f, app, list, &[(Action::New, " starts a worktree")]);
    }
    let cursor = Some(Row::Worktree(app.sel_worktree));
    let now = crate::app::now_ms();
    draw_list(
        f,
        app,
        area,
        list,
        &lines,
        cursor,
        cursor,
        Focus::Worktrees,
        Shape::Pill,
        |app, at, width| {
            let Row::Worktree(i) = at else {
                return (Vec::new(), th.accent, false);
            };
            let (spans, mark) = match app.worktree_rows().get(i).copied() {
                Some(WorktreeRow::Checkout(w)) => checkout_row(app, w, false, width, now),
                Some(WorktreeRow::PrCheckout(w)) => checkout_row(app, w, true, width, now),
                Some(WorktreeRow::Pr(pr)) => {
                    let trouble = pr.trouble();
                    let look = crate::pr_row::look(pr.standing(), trouble, th);
                    let badge = match trouble {
                        Some(trouble) => Some((format!(" {}", trouble.badge()), look.badge)),
                        None => pr
                            .is_draft
                            .then(|| (format!(" {}", pr.badge()), look.badge)),
                    };
                    let spans = crate::pr_row::spans(look, &pr.label(), width, badge);
                    (spans, look.rail)
                }
                Some(WorktreeRow::Issue(issue)) => {
                    // The green the ISSUES MODAL paints `open` in.
                    let look = crate::pr_row::Look {
                        glyph: th.ok,
                        label: th.muted,
                        rail: th.ok,
                        badge: th.dim,
                    };
                    (
                        crate::pr_row::spans(look, &issue.label(), width, None),
                        look.rail,
                    )
                }
                None => (Vec::new(), th.accent),
            };
            (vec![spans], mark, false)
        },
    );
}

/// One checkout's row: the rolled-up STATUS DOT of its sessions, its
/// branch — under a `└` when it is a pull request's — the RUNNING badge
/// while its RUN COMMAND is up, `⌂ root` on the ROOT WORKTREE, how long
/// since anything in it moved, and the finishes in it nobody has read.
///
/// A checkout whose pull request has merged wears the merge instead
/// (`App::worktree_wears_merge`, where a live session still wins): purple
/// dot, purple rail, purple branch — swept on the merged ramp for the
/// few seconds after the merge is seen to land, solid from then on.
fn checkout_row(
    app: &App,
    w: &nebula_core::Worktree,
    nested: bool,
    width: usize,
    now: i64,
) -> (Vec<Span<'static>>, Color) {
    let th = app.theme;
    let dim = Style::default().fg(th.dim);
    let pending = app.is_placeholder_worktree(&w.id);
    let merged = !pending && app.worktree_wears_merge(&w.id);
    let roll = if pending {
        None
    } else {
        crate::app::worktree_rollup(&app.tree, &w.id)
    };
    let unseen = crate::app::worktree_unseen(&app.tree, &w.id);
    let (dot, mark, ramp, name_style) = if merged {
        let fresh = app.animations && app.merge_is_fresh(&w.id);
        (
            Span::styled("● ", Style::default().fg(th.merged)),
            th.merged,
            fresh.then_some(th.merged_sweep),
            Style::default().fg(th.merged),
        )
    } else {
        let fresh = crate::panels::worktree_fresh_done(app, &w.id);
        (
            status_dot(roll, unseen > 0, th),
            status_color(roll, unseen > 0, th),
            sweep_ramp(roll, fresh, th, app.animations),
            Style::default(),
        )
    };
    let badge = unseen_badge(unseen);
    let indent = if nested { NESTED_INDENT } else { "" };
    let free = width.saturating_sub(3 + badge.chars().count() + indent.chars().count());
    let run = app.worktree_running(&w.id).then(|| {
        if w.branch.chars().count() + RUN_BADGE.chars().count() <= free {
            RUN_BADGE
        } else {
            RUN_GLYPH
        }
    });
    let free = free.saturating_sub(run.map_or(0, |r| r.chars().count()));
    let ago = if pending {
        PENDING_WORKTREE_BADGE.to_string()
    } else {
        ago_badge(crate::app::worktree_recency(&app.tree, &w.id, now).stamped)
    };
    let (ago, free) = fit_ago(ago, free);
    // The root badge yields to a branch it would push into an ellipsis —
    // the branch is the row's identity, the ⌂ the least of it — and
    // shrinks to the bare glyph before it goes.
    let fits = |b: &str| w.branch.chars().count() + b.chars().count() <= free;
    let root = if !w.is_main {
        None
    } else if fits(ROOT_BADGE) {
        Some(ROOT_BADGE)
    } else if fits(ROOT_GLYPH) {
        Some(ROOT_GLYPH)
    } else {
        None
    };
    let max = free.saturating_sub(root.map_or(0, |r| r.chars().count()));
    let mut spans = Vec::new();
    if nested {
        spans.push(Span::styled(indent, dim));
    }
    spans.push(dot);
    spans.extend(status_name_spans(
        truncate(&w.branch, max),
        name_style,
        ramp,
        app.sweep_phase(),
    ));
    if let Some(run) = run {
        spans.push(Span::styled(
            run,
            Style::default().fg(th.ok).add_modifier(Modifier::BOLD),
        ));
    }
    if let Some(root) = root {
        spans.push(Span::styled(root, dim));
    }
    push_dim(&mut spans, ago, th);
    spans.push(Span::styled(badge, Style::default().fg(th.done)));
    (spans, mark)
}

/// SESSIONS: the selected checkout's live sessions, then its terminals,
/// pull request and archived sessions under their headers
/// (`panels::session_lines`). A checkout with nothing in it says which
/// keys start something. A pull request or an issue under the WORKTREES
/// cursor has no sessions, and the column is never drawn then: it folds
/// to its RULE and the pane reading the row takes its width
/// (`panels::sessions_fold`).
fn draw_sessions(f: &mut Frame, app: &mut App, area: Rect) {
    let th = app.theme;
    let rows = app.visible_session_rows();
    let count = rows.iter().filter(|r| r.as_link().is_none()).count();
    let focused = app.focus == Focus::Sessions;
    let list = column(
        f,
        area,
        "SESSIONS",
        count,
        focused,
        th,
        Focus::Sessions,
        &mut app.hits,
    );
    app.sessions_view_rows = usize::from(list.height) / PILL_H;
    let lines = crate::panels::session_lines(app, list.width);
    if lines.is_empty() && app.selected_worktree().is_some() {
        hint(
            f,
            app,
            list,
            &[
                (Action::New, " agent · "),
                (Action::NewTerminal, " terminal"),
            ],
        );
    }
    let cursor = Some(Row::Session(app.sel_session));
    // The expanded pill pulls the window ahead of the cursor's own row:
    // the box is where the keyboard is pointed, and a cursor parked
    // elsewhere must not scroll the box being typed into off the screen.
    let reveal = app.follow_up_row().map(Row::Session).or(cursor);
    let mut cfg: Option<crate::config::Config> = None;
    draw_list(
        f,
        app,
        area,
        list,
        &lines,
        cursor,
        reveal,
        Focus::Sessions,
        Shape::Pill,
        |app, at, width| {
            let Row::Session(i) = at else {
                return (Vec::new(), th.accent, false);
            };
            let row = rows.get(i);
            let chevron = row.is_some_and(|row| app.takes_follow_up(row));
            let (spans, mark) = match row {
                Some(SessionRow::Agent(a)) => agent_row(app, a, width, chevron, &mut cfg),
                Some(SessionRow::Terminal(t)) => terminal_row(t, width, th),
                Some(SessionRow::Link(l)) => link_row(app, l, width),
                None => (Vec::new(), th.accent),
            };
            (vec![spans], mark, chevron)
        },
    );
}

/// A session's row: its STATUS DOT — gray while no PTY is behind it (the
/// IDLE REAPER took it), hollow while it is a stand-in still being
/// created, `⊘` once archived — its name, muted, so the bottom of the
/// tree reads smallest beside the bold projects, how long since it moved,
/// and the harness it runs on. The name is what gives way on a narrow
/// column; an unread finish takes the harness's slot as ` done`, a Claude
/// Cloud row says ` cloud`. A row that can grow a FOLLOW-UP COMPOSER
/// (`chevron`, `App::takes_follow_up`) ends in its FOLLOW-UP CHEVRON —
/// `▸` folded, `▾` in the accent expanded — taken out of the name's
/// budget before anything else is measured.
fn agent_row(
    app: &App,
    a: &nebula_core::Agent,
    width: usize,
    chevron: bool,
    cfg: &mut Option<crate::config::Config>,
) -> (Vec<Span<'static>>, Color) {
    let th = app.theme;
    let chevron = chevron.then(|| {
        if app.follow_up.as_ref().is_some_and(|f| f.agent == a.id) {
            (FOLLOW_UP_OPEN, th.accent)
        } else {
            (FOLLOW_UP_FOLDED, th.dim)
        }
    });
    let chevron_w = chevron.map_or(0, |(glyph, _)| glyph.chars().count());
    let pending = app.is_placeholder_agent(&a.id);
    let cold = !a.alive && a.cloud_session_id.is_none();
    let dot = if a.archived {
        Span::styled("⊘ ", Style::default().fg(th.dim))
    } else if pending {
        status_dot(None, false, th)
    } else if cold {
        Span {
            style: Style::default().fg(th.dim),
            ..status_dot(Some(a.status), false, th)
        }
    } else {
        status_dot(Some(a.status), a.unseen, th)
    };
    let ago = if pending {
        String::new()
    } else {
        ago_badge(a.status_changed_at)
    };
    let (badge, badge_color) = if pending {
        (PENDING_SESSION_BADGE.to_string(), th.dim)
    } else if a.unseen && !a.archived {
        (" done".to_string(), th.done)
    } else if a.cloud_session_id.is_some() {
        (" cloud".to_string(), th.dim)
    } else {
        let harness = if a.kind == nebula_core::AgentKind::Custom {
            let cfg = cfg.get_or_insert_with(crate::config::Config::load);
            crate::agent_picker::session_harness_badge_in(a, cfg)
        } else {
            a.kind.as_str().to_string()
        };
        (format!(" {harness}"), th.dim)
    };
    let free = width.saturating_sub(3 + badge.chars().count() + chevron_w);
    let (ago, name_max) = fit_ago(ago, free);
    let quiet = a.archived || pending || cold;
    let ramp = if quiet {
        None
    } else {
        sweep_ramp(Some(a.status), app.agent_fresh_done(a), th, app.animations)
    };
    let name_style = Style::default().fg(if a.archived { th.dim } else { th.muted });
    let mut spans = vec![dot];
    spans.extend(status_name_spans(
        truncate(&a.name, name_max),
        name_style,
        ramp,
        app.sweep_phase(),
    ));
    push_dim(&mut spans, ago, th);
    spans.push(Span::styled(badge, Style::default().fg(badge_color)));
    if let Some((glyph, color)) = chevron {
        spans.push(Span::styled(glyph, Style::default().fg(color)));
    }
    let mark = if quiet {
        th.dim
    } else {
        status_color(Some(a.status), a.unseen, th)
    };
    (spans, mark)
}

/// A terminal's row: the shell's `❯` — a RUN TERMINAL's `▶` and its
/// command after the name — green while the shell is up, dim once it has
/// exited.
fn terminal_row(
    t: &nebula_core::TerminalTab,
    width: usize,
    th: Theme,
) -> (Vec<Span<'static>>, Color) {
    let glyph = if t.run_command.is_some() {
        "▶ "
    } else {
        "❯ "
    };
    let color = if t.alive { th.ok } else { th.dim };
    let name = truncate(&t.name, width.saturating_sub(3));
    let room = width.saturating_sub(4 + name.chars().count());
    let mut spans = vec![
        Span::styled(glyph, Style::default().fg(color)),
        Span::styled(name, Style::default().fg(th.muted)),
    ];
    if let Some(command) = t.run_command.as_deref().filter(|_| room > 1) {
        spans.push(Span::styled(
            format!(" {}", truncate(command, room)),
            Style::default().fg(th.dim),
        ));
    }
    (spans, th.accent)
}

/// The checkout's pull request — or a saved link — as a row: the arrow
/// that says it leaves nebula, its `#42 title`, and its state word, or the
/// trouble GitHub says it cannot merge for. Comments other people left
/// since it was last opened from nebula take the badge over, loud — ` 2
/// new` — as the one thing on the row worth walking over for.
fn link_row(app: &App, l: &crate::app::LinkRow, width: usize) -> (Vec<Span<'static>>, Color) {
    let th = app.theme;
    let pr = l.pull_request();
    let look = match pr {
        Some(pr) => crate::pr_row::look(pr.standing(), pr.trouble(), th),
        None => crate::pr_row::Look {
            glyph: th.muted,
            label: th.muted,
            rail: th.accent,
            badge: th.dim,
        },
    };
    let badge = pr.map(|pr| match (unseen_comments(app, pr), pr.trouble()) {
        (n, _) if n > 0 => (format!(" {n} new"), th.warn),
        (_, Some(trouble)) => (format!(" {}", trouble.badge()), look.badge),
        (_, None) => (format!(" {}", pr.standing().badge()), look.badge),
    });
    (
        crate::pr_row::spans(look, &l.label(), width, badge),
        look.rail,
    )
}

/// Comments and reviews on `pr` newer than the last one seen when it was
/// opened from nebula (`App::pr_seen`); a pull request never opened has
/// the whole conversation unread.
fn unseen_comments(app: &App, pr: &crate::pull_request::PullRequest) -> usize {
    match app.pr_seen.get(&pr.url) {
        Some(mark) => pr
            .activity
            .iter()
            .filter(|at| at.as_str() > mark.as_str())
            .count(),
        None => pr.activity.len(),
    }
}

/// ` 2 done`: the finishes under a project or checkout nobody has read,
/// in the color the session rows' own ` done` wears. Empty with none.
fn unseen_badge(unseen: usize) -> String {
    if unseen == 0 {
        String::new()
    } else {
        format!(" {unseen} done")
    }
}

/// `text` as a dim span, when there is any.
fn push_dim(spans: &mut Vec<Span<'static>>, text: String, th: Theme) {
    if !text.is_empty() {
        spans.push(Span::styled(text, Style::default().fg(th.dim)));
    }
}
