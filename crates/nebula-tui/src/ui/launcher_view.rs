//! The LAUNCHER VIEW's drawing (`crate::launcher` is its model,
//! `event_loop::launcher` its keys): the GRID of session cards — each card
//! the session's name and status, the worktree it runs in under it, its
//! pull request under that — over the PANE along the bottom that reads the
//! card under the cursor (`ui::draw` splits the body and fills that pane);
//! plus the view's own pieces of the QUICK PROMPT (the project on its
//! target row, its key hints) and the PROJECT PICKER.

use super::{
    ago_badge, below_first_row, centered_rect, empty_list_row, fit_ago, fuzzy_highlight_spans,
    over_box_rect, render_modal_frame, render_row, row_rect, search_line, status_dot,
    status_name_spans, sweep_ramp, truncate, visible_positions, NO_MATCHES, OVER_BOX_INSET,
    PENDING_SESSION_BADGE,
};
use crate::app::{App, Focus, HitTarget, Overlay};
use crate::launcher::{LauncherRow, Level, ProjectCard, ProjectPicker, Tally, WorkspaceCard};
use crate::quick_prompt::QuickLaunch;
use crate::theme::Theme;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Paragraph};
use ratatui::Frame;

/// Width of the view's QUICK PROMPT, and its height: wider and taller than
/// the panels' box, since here it is the front door. The extra row over
/// the panels' box pays for the blank one between the details and the
/// question, so the editor keeps its full height.
pub(super) const BOX_SIZE: (u16, u16) = (92, 18);
/// The PROJECT PICKER's width, and the most rows it lists before scrolling.
const PICKER_W: u16 = 64;
const PICKER_ROWS: u16 = 14;
/// What the breadcrumb's first crumb says: the whole machine, above
/// every workspace.
const ROOT: &str = "nebula";
/// What the breadcrumb's last crumb says, in both views — the word a
/// click on it goes back to.
const CRUMB: &str = "sessions";
/// What separates two crumbs.
const SEP: &str = " / ";
/// A name in the breadcrumb is never squeezed under this, however narrow
/// the row: at that point the count on the right gives way instead.
const CRUMB_MIN: usize = 6;

/// The view's area, with the view on: the breadcrumb header, then the GRID
/// under it — session, project or workspace cards, whichever LEVEL the
/// view is on ([`crate::launcher::Level`]). `body` is what
/// `crate::launcher::split` left over the PANE along the bottom, which
/// `ui::draw` fills with the card under the cursor on the SESSIONS level
/// and which the levels above it do not have at all; Enter on a session
/// card steps down into that pane (`event_loop::launcher::enter_pane`)
/// and `z` full-screens it over the lot
/// (`event_loop::launcher::open_session`, the `collapsed` arm of
/// `ui::draw`).
///
/// Every level is the same three pieces — a head, a grid of equal-height
/// cards, an empty state — so the screen never jumps as the tree is
/// walked; only what a card says changes.
pub(super) fn draw(f: &mut Frame, app: &mut App, body: Rect) {
    app.body_area = body;
    app.settle_launcher_focus();
    let g = crate::launcher::grid(body);
    match app.launcher_level {
        Level::Sessions => {
            let rows = crate::launcher::rows(app);
            let tally = crate::launcher::session_tally(&rows);
            draw_head(f, app, body, rows.len(), tally);
            if rows.is_empty() {
                draw_empty(f, app, g.area);
                return;
            }
            draw_grid(f, app, g, &rows);
        }
        Level::Projects => {
            let cards = crate::launcher::project_cards(app);
            let tally = crate::launcher::project_tally(app, &cards);
            draw_head(f, app, body, cards.len(), tally);
            if cards.is_empty() {
                draw_level_empty(f, app, g.area, NO_PROJECTS);
                return;
            }
            draw_project_grid(f, app, g, &cards);
        }
        Level::Workspaces => {
            let cards = crate::launcher::workspace_cards(app);
            let tally = crate::launcher::workspace_tally(app, &cards);
            draw_head(f, app, body, cards.len(), tally);
            if cards.is_empty() {
                draw_level_empty(f, app, g.area, NO_WORKSPACES);
                return;
            }
            draw_workspace_grid(f, app, g, &cards);
        }
    }
}

/// What the PROJECTS level says with nothing to show.
const NO_PROJECTS: &str = "no projects in this workspace";
/// And the WORKSPACES level, which a tree always has at least one of.
const NO_WORKSPACES: &str = "no workspaces";

/// The header over the grid: the breadcrumb down to this LEVEL on the
/// left with the STATUS TALLY's dots beside it, how many cards the grid
/// holds on the right, a rule under both — the same three-row head the
/// panels' columns sit on.
fn draw_head(f: &mut Frame, app: &mut App, body: Rect, count: usize, tally: Tally) {
    let th = app.theme;
    if let Some(r) = row_rect(body, 1) {
        let r = pad_x(r);
        let right = head_count(app.launcher_level, count, th);
        let used: usize = right.iter().map(|s| s.width()).sum();
        f.render_widget(
            Paragraph::new(Line::from(head_crumbs(app, r, used, tally))),
            r,
        );
        f.render_widget(
            Paragraph::new(Line::from(right)).alignment(ratatui::layout::Alignment::Right),
            r,
        );
    }
    draw_rule(f, body, 2, th.edge);
}

/// The header's breadcrumb — the path down to the LEVEL the view is on:
/// `nebula / workspace / project / sessions` at the bottom of the tree,
/// `nebula / workspace / projects` a level out, `nebula / workspaces` at
/// the top. A crumb is only ever drawn for a tier the view has already
/// walked past, so the trail reads as where this is and Esc is the way
/// back along it.
///
/// Every crumb is a button to what its word names, never to the list the
/// word sits in: `nebula` is the whole machine, so it opens the
/// WORKSPACES level; a workspace opens its projects; a project opens its
/// sessions — which, since the project crumb is only ever drawn there, is
/// where the view already is, so that click stays put. The last word is
/// the list in front of you and is no button at all. The hit rects are
/// laid down as the spans are measured, so a click lands on the word
/// itself and not on the separators around it, and
/// `event_loop::launcher::click_crumb` walks there the way Esc does. A
/// tree with no project yet simply has no project crumb.
///
/// The trail itself sits still. What is happening under it is the STATUS
/// TALLY's job — [`head_dots`], which follows the last crumb on the same
/// row — so the header says its piece in four colors instead of animating
/// the path you navigate by.
///
/// `taken` is what the count on the right of the same row has already
/// spent: the crumbs get the rest, less the tally's own columns and one of
/// air, so the three are never drawn over each other ([`head_room`] is
/// what shares out what is left).
fn head_crumbs(app: &mut App, r: Rect, taken: usize, tally: Tally) -> Vec<Span<'static>> {
    let th = app.theme;
    let level = app.launcher_level;
    let project = app.selected_project().map(|p| p.name.clone());
    let mut middles = Vec::new();
    if level != Level::Workspaces {
        middles.push(Crumb {
            name: app.tree.active_workspace_name().to_string(),
            hit: HitTarget::LauncherWorkspace,
        });
    }
    if level == Level::Sessions {
        if let Some(name) = project {
            middles.push(Crumb {
                name,
                hit: HitTarget::LauncherProject,
            });
        }
    }
    // The tally is reserved before the names are: it is four dots at most,
    // and the only thing on this row that says whether anything wants you,
    // so the crumbs shrink around it rather than print over it. On a row
    // with no columns to spare for it, it goes the way the crumbs go —
    // half a trail under a dot reads as a bug.
    let mut dots = head_dots(tally, th);
    let mut dots_w: usize = dots.iter().map(|s| s.width()).sum();
    if taken + dots_w + ROOT.len() + 2 > r.width as usize {
        dots.clear();
        dots_w = 0;
    }
    let room = (r.width as usize).saturating_sub(taken + dots_w + 2);
    let last = level.crumb();
    let cap = head_room(&mut middles, room, last);
    for crumb in middles.iter_mut() {
        crumb.name = truncate(&crumb.name, cap);
    }

    let sep = Span::styled(SEP, Style::default().fg(th.dim));
    let mut spans = vec![Span::styled(
        ROOT.to_string(),
        Style::default().fg(th.accent).add_modifier(Modifier::BOLD),
    )];
    // The head of the trail is a button like the rest of them: it names
    // the machine, and the machine's own list is the WORKSPACES level.
    let root_w = ROOT.chars().count() as u16;
    app.hits
        .push((Rect { width: root_w, ..r }, HitTarget::LauncherRoot));
    let mut x = r.x + root_w;
    // A crumb that leads somewhere: the word, and the rect a click on it
    // hits. `x` walks the row as the spans are pushed, by the word's own
    // width — the separators around it are not part of the target.
    let mut button = |spans: &mut Vec<Span<'static>>, crumb: Crumb| {
        spans.push(sep.clone());
        x += SEP.chars().count() as u16;
        let width = crumb.name.chars().count() as u16;
        spans.push(Span::styled(crumb.name, Style::default().fg(th.muted)));
        let rect = Rect { x, width, ..r };
        x += width;
        (rect, crumb.hit)
    };
    let hits: Vec<_> = middles
        .into_iter()
        .map(|crumb| button(&mut spans, crumb))
        .collect();
    app.hits.extend(hits);
    // The level's own word goes the way the middles went on a row too
    // narrow even for it: half a word under the count reads as a typo,
    // and with the middles already dropped `nebula` alone still says
    // where this is.
    if ROOT.len() + SEP.len() + last.len() <= room {
        spans.push(sep);
        spans.push(Span::styled(
            last.to_string(),
            Style::default().fg(th.text).add_modifier(Modifier::BOLD),
        ));
    }
    spans.extend(dots);
    spans
}

/// One crumb of the breadcrumb: the word, and where a click on it walks to.
struct Crumb {
    name: String,
    hit: HitTarget,
}

/// The header's STATUS TALLY: one dot per state the grid has a card in,
/// carrying that state's count and no word at all, so the header is read at
/// a glance rather than parsed. Waiting on you leads (red), then finished
/// unread (blue), then working (yellow) — the three a row's own STATUS DOT
/// wears — and then landed (purple), the color a merged checkout takes in
/// the WORKTREES panel.
/// A state with nothing in it is left out, so a quiet grid keeps a bare
/// breadcrumb — and because the order is fixed, the dots that are there
/// never move as the work under them does.
fn head_dots(tally: Tally, th: Theme) -> Vec<Span<'static>> {
    [
        (tally.needs_you, th.err),
        (tally.done, th.done),
        (tally.running, th.warn),
        (tally.merged, th.merged),
    ]
    .into_iter()
    .filter(|(n, _)| *n > 0)
    .map(|(n, color)| Span::styled(format!("  ● {n}"), Style::default().fg(color)))
    .collect()
}

/// How many cells each middle crumb may take, and which of them survive
/// at all: `middles` is trimmed from the right — the project first, then
/// the workspace — until what `room` leaves after the fixed words and the
/// separators can still give every remaining name something to read. A
/// name shorter than its even split hands what it does not use to the
/// other, so `web` beside a long workspace costs only three cells.
fn head_room(middles: &mut Vec<Crumb>, room: usize, last: &str) -> usize {
    let fixed = ROOT.len() + last.len();
    loop {
        let n = middles.len();
        if n == 0 {
            return 0;
        }
        let budget = room.saturating_sub(fixed + SEP.len() * (n + 1));
        let even = budget / n;
        let spare: usize = middles
            .iter()
            .map(|c| even.saturating_sub(c.name.chars().count()))
            .sum();
        let cap = even + spare;
        // Readable, or short enough that the squeeze never bites.
        if cap >= CRUMB_MIN || middles.iter().all(|c| c.name.chars().count() <= cap) {
            return cap;
        }
        middles.pop();
    }
}

/// The header's right side: how many cards the grid holds, and nothing
/// else. Which of them want something is the STATUS TALLY's business over
/// beside the breadcrumb, told in dots rather than in a second sentence.
fn head_count(level: Level, count: usize, th: Theme) -> Vec<Span<'static>> {
    vec![Span::styled(
        format!("{count} {}{}", level.singular(), plural(count)),
        Style::default().fg(th.dim),
    )]
}

fn plural(n: usize) -> &'static str {
    if n == 1 {
        ""
    } else {
        "s"
    }
}

/// The rule under a header row.
fn draw_rule(f: &mut Frame, area: Rect, row: usize, color: Color) {
    if let Some(r) = row_rect(area, row) {
        f.render_widget(
            Paragraph::new(Span::styled(
                "─".repeat(r.width as usize),
                Style::default().fg(color),
            )),
            r,
        );
    }
}

/// The header's own margin, so its text lines up with the cards' left
/// edge rather than hugging the screen.
fn pad_x(r: Rect) -> Rect {
    let pad = crate::launcher::PAD_X;
    Rect {
        x: r.x + pad,
        width: r.width.saturating_sub(pad * 2),
        ..r
    }
}

/// The GRID: one card per session, most recent first, left to right and top to
/// bottom, scrolled by whole rows so the cursor's card is always drawn.
fn draw_grid(f: &mut Frame, app: &mut App, g: crate::launcher::Grid, rows: &[LauncherRow]) {
    let th = app.theme;
    let focused = app.focus != Focus::Terminal;
    let cursor = crate::launcher::cursor(app, rows);
    // Scrolling is by row: the window starts on the first card of
    // whichever row keeps the cursor's on screen.
    let cursor_row = cursor.unwrap_or(0) / g.cols;
    let start = crate::app::window_start(cursor_row, g.rows_fit) * g.cols;
    // One CONFIG.JSON read for the whole frame, and only if some card on
    // it runs a CUSTOM harness whose label lives in there — a screenful
    // of cards must not reload the file once per card.
    let mut cfg = None;
    for (slot, (index, row)) in rows
        .iter()
        .enumerate()
        .skip(start)
        .enumerate()
        .take(g.page())
    {
        let cell = g.cell(slot);
        if cell.y + cell.height > g.area.y + g.area.height {
            break;
        }
        draw_card(
            f,
            app,
            cell,
            row,
            cursor == Some(index),
            focused,
            th,
            &mut cfg,
        );
        app.hits.push((cell, HitTarget::LauncherRow(index)));
    }
    // Last, so the cards themselves win `hit_at`'s first-match scan and
    // only the air between them falls through to the grid.
    app.hits.push((g.area, HitTarget::PanelBg(Focus::Sessions)));
}

/// One session's card: its name and how long since it last moved, where
/// it runs and with what, its pull request, and the last thing it was
/// asked to do. The cursor's card takes the selection fill and an accent
/// border; every other card is drawn on the frame's own edge color.
#[allow(clippy::too_many_arguments)]
fn draw_card(
    f: &mut Frame,
    app: &App,
    area: Rect,
    row: &LauncherRow,
    selected: bool,
    focused: bool,
    th: Theme,
    cfg: &mut Option<crate::config::Config>,
) {
    let a = &row.agent;
    let pending = app.is_placeholder_agent(&a.id);
    let cold = !a.alive && a.cloud_session_id.is_none();
    // The dot and the sweep read the status the panels' session rows read,
    // cold and pending alike (see `draw_session_row`).
    let dot = if pending {
        status_dot(None, false, th)
    } else if cold {
        Span {
            style: Style::default().fg(th.dim),
            ..status_dot(Some(a.status), false, th)
        }
    } else {
        status_dot(Some(a.status), a.unseen, th)
    };
    let border = if selected && focused {
        th.accent
    } else if selected {
        th.muted
    } else {
        th.edge
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(border))
        .style(super::row_bar(selected, focused, th));
    let inner = block.inner(area);
    f.render_widget(block, area);
    // One cell of air inside the border, so the text never touches it.
    let inner = Rect {
        x: inner.x + 1,
        width: inner.width.saturating_sub(2),
        ..inner
    };
    let width = inner.width as usize;
    if width == 0 {
        return;
    }

    let ago = if pending {
        PENDING_SESSION_BADGE.to_string()
    } else if a.unseen {
        " done".to_string()
    } else {
        ago_badge(a.status_changed_at)
    };
    let (ago, name_max) = fit_ago(ago, width);
    let ramp = if pending || cold {
        None
    } else {
        sweep_ramp(Some(a.status), app.agent_fresh_done(a), th, app.animations)
    };
    let name = truncate(&a.name, name_max.saturating_sub(2));
    let mut first = vec![dot];
    let used = name.chars().count() + 2;
    first.extend(status_name_spans(
        name,
        Style::default().fg(th.text).add_modifier(Modifier::BOLD),
        ramp,
        app.sweep_phase(),
    ));
    if !ago.is_empty() {
        let ago = ago.trim_start().to_string();
        let pad = width.saturating_sub(used + ago.chars().count());
        first.push(Span::raw(" ".repeat(pad)));
        first.push(Span::styled(
            ago,
            if a.unseen && !pending {
                Style::default().fg(th.done)
            } else {
                Style::default().fg(th.dim)
            },
        ));
    }

    // Where it runs and with what: the checkout — then the harness and
    // the model the session was launched on. The project is not on the
    // card: the grid is one project's sessions and its name is already in
    // the header's crumb, so repeating it on every card is noise.
    //
    // The checkout carries the SCOPE COLOR, glyph and branch both: `⌂` in
    // `th.root` on the project's root branch, as the WORKTREES PANEL marks
    // that row, and `↳` in `th.worktree` on a checkout of its own. It is
    // the one thing on a grid of cards worth sorting by before any of them
    // is read — an agent on the root branch is editing what everything
    // else is cut from — so it is the one thing on this row that is not
    // dim. The glyph is its own span and never yields: a long branch
    // truncates around it rather than through it, so the scope survives
    // the narrowest card the grid will draw.
    let (glyph, scope) = if row.is_main {
        ("⌂ ", th.root)
    } else {
        ("↳ ", th.worktree)
    };
    let harness = harness_line(a, cfg);
    let room = width.saturating_sub(harness.chars().count() + 3);
    let branch = truncate(&row.branch, room.saturating_sub(glyph.chars().count()));
    let mut second = vec![
        Span::styled(glyph, Style::default().fg(scope)),
        Span::styled(branch, Style::default().fg(scope)),
    ];
    if !harness.is_empty() {
        second.push(Span::styled(" · ", Style::default().fg(th.dim)));
        second.push(Span::styled(harness, Style::default().fg(th.dim)));
    }

    // Its pull request, in the PR rows' own colors; a session with none
    // leaves the row blank rather than saying so four times over a screen
    // of cards.
    let third = match &row.pr {
        Some(pr) => {
            let look = crate::pr_row::look(pr.standing, pr.trouble, th);
            let label = if pr.title.is_empty() {
                format!("#{}", pr.number)
            } else {
                format!("#{} {}", pr.number, pr.title)
            };
            crate::pr_row::spans(
                look,
                &label,
                width,
                Some((format!(" {}", pr.badge()), look.badge)),
            )
        }
        None => Vec::new(),
    };

    // The last thing it was asked to do, on the prompt's own `›`, over
    // the card's last rows rather than clipped at the first.
    let mut lines = vec![first, second, third];
    lines.resize(crate::launcher::CARD_HEAD_H as usize, Vec::new());
    lines.extend(prompt_lines(
        crate::launcher::last_prompt(a).unwrap_or_default(),
        width,
        th,
    ));

    for (i, spans) in lines.into_iter().enumerate() {
        if spans.is_empty() {
            continue;
        }
        let Some(r) = row_rect(inner, i) else { break };
        f.render_widget(Paragraph::new(Line::from(spans)), r);
    }
}

// ---- the PROJECTS and WORKSPACES levels ----

/// The PROJECTS level's grid: one card per project, the ones wanting a
/// human first, scrolled by whole rows so the cursor's card is always
/// drawn — the session grid's own geometry, reading a different list.
fn draw_project_grid(
    f: &mut Frame,
    app: &mut App,
    g: crate::launcher::Grid,
    cards: &[ProjectCard],
) {
    let th = app.theme;
    let focused = app.focus != Focus::Terminal;
    let cursor = crate::launcher::project_cursor(app, cards);
    let start = window_row(cursor, g) * g.cols;
    for (slot, (index, card)) in cards
        .iter()
        .enumerate()
        .skip(start)
        .enumerate()
        .take(g.page())
    {
        let Some(cell) = fitting_cell(&g, slot) else {
            break;
        };
        draw_project_card(f, cell, card, cursor == Some(index), focused, th);
        app.hits.push((cell, HitTarget::LauncherProjectCard(index)));
    }
    app.hits.push((g.area, HitTarget::PanelBg(Focus::Sessions)));
}

/// The WORKSPACES level's grid, the same way over workspace cards.
fn draw_workspace_grid(
    f: &mut Frame,
    app: &mut App,
    g: crate::launcher::Grid,
    cards: &[WorkspaceCard],
) {
    let th = app.theme;
    let focused = app.focus != Focus::Terminal;
    let cursor = crate::launcher::workspace_cursor(app);
    let start = window_row(cursor, g) * g.cols;
    for (slot, (index, card)) in cards
        .iter()
        .enumerate()
        .skip(start)
        .enumerate()
        .take(g.page())
    {
        let Some(cell) = fitting_cell(&g, slot) else {
            break;
        };
        draw_workspace_card(f, cell, card, cursor == Some(index), focused, th);
        app.hits
            .push((cell, HitTarget::LauncherWorkspaceCard(index)));
    }
    app.hits.push((g.area, HitTarget::PanelBg(Focus::Sessions)));
}

/// The first row of cards the window shows: the one that keeps the
/// cursor's card on screen. Scrolling is by whole rows, so a card is
/// never half-drawn at the top.
fn window_row(cursor: Option<usize>, g: crate::launcher::Grid) -> usize {
    crate::app::window_start(cursor.unwrap_or(0) / g.cols, g.rows_fit)
}

/// Where the card in `slot` goes, or None once the grid has run out of
/// rows for it — a card is drawn whole or not at all.
fn fitting_cell(g: &crate::launcher::Grid, slot: usize) -> Option<Rect> {
    let cell = g.cell(slot);
    (cell.y + cell.height <= g.area.y + g.area.height).then_some(cell)
}

/// The frame every card on these levels shares: the cursor's takes the
/// selection fill and an accent border, every other one the frame's own
/// edge color — the session card's look, so the grid reads the same at
/// any depth. Returns the row of cells inside it that the text goes on.
fn card_frame(f: &mut Frame, area: Rect, selected: bool, focused: bool, th: Theme) -> Rect {
    let border = if selected && focused {
        th.accent
    } else if selected {
        th.muted
    } else {
        th.edge
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(border))
        .style(super::row_bar(selected, focused, th));
    let inner = block.inner(area);
    f.render_widget(block, area);
    // One cell of air inside the border, so the text never touches it.
    Rect {
        x: inner.x + 1,
        width: inner.width.saturating_sub(2),
        ..inner
    }
}

/// A card's own name row: the rolled-up dot, the name in bold, and a
/// right-aligned tail — the ago badge, or the count of what is under it.
fn name_row(
    dot: Span<'static>,
    name: &str,
    tail: String,
    tail_style: Style,
    width: usize,
    th: Theme,
) -> Vec<Span<'static>> {
    let (tail, name_max) = fit_ago(tail, width);
    let name = truncate(name, name_max.saturating_sub(2));
    let used = name.chars().count() + 2;
    let mut row = vec![
        dot,
        Span::styled(
            name,
            Style::default().fg(th.text).add_modifier(Modifier::BOLD),
        ),
    ];
    if !tail.is_empty() {
        let tail = tail.trim_start().to_string();
        let pad = width.saturating_sub(used + tail.chars().count());
        row.push(Span::raw(" ".repeat(pad)));
        row.push(Span::styled(tail, tail_style));
    }
    row
}

/// One line for something under a card: its dot, its name, and how long
/// ago it moved on the right.
fn child_row(
    dot: Span<'static>,
    name: &str,
    ago: String,
    width: usize,
    th: Theme,
) -> Vec<Span<'static>> {
    let (ago, name_max) = fit_ago(ago, width);
    let name = truncate(name, name_max.saturating_sub(2));
    let used = name.chars().count() + 2;
    let mut row = vec![dot, Span::styled(name, Style::default().fg(th.muted))];
    if !ago.is_empty() {
        let ago = ago.trim_start().to_string();
        let pad = width.saturating_sub(used + ago.chars().count());
        row.push(Span::raw(" ".repeat(pad)));
        row.push(Span::styled(ago, Style::default().fg(th.dim)));
    }
    row
}

/// The `+ 3 more` line a card ends on when what is under it outruns the
/// rows it has: the list is trimmed to [`crate::launcher::CARD_LIST`]
/// rows, the last of them saying how many were left off, so a card is a
/// fixed height however much is running in it.
fn more_row(left: usize, th: Theme) -> Vec<Span<'static>> {
    vec![Span::styled(
        format!("  + {left} more"),
        Style::default().fg(th.dim),
    )]
}

/// How a card's list of children is shared out: the rows to draw in full,
/// and how many are left over for a `+ N more` line. All of them fit, or
/// one row is given up to say what did not.
fn listed(total: usize) -> (usize, usize) {
    let keep = crate::launcher::CARD_LIST;
    if total <= keep {
        (total, 0)
    } else {
        (keep - 1, total - (keep - 1))
    }
}

/// One project's card: its name with the rolled-up dot and how long since
/// anything in it moved, then its sessions under it — the ones wanting a
/// human first, each with its own dot and age. A project with nothing in
/// it says so rather than leaving the card blank.
fn draw_project_card(
    f: &mut Frame,
    area: Rect,
    card: &ProjectCard,
    selected: bool,
    focused: bool,
    th: Theme,
) {
    let inner = card_frame(f, area, selected, focused, th);
    let width = inner.width as usize;
    if width == 0 {
        return;
    }
    let mut lines = vec![name_row(
        status_dot(card.status, card.unseen, th),
        &card.name,
        if card.sessions.is_empty() {
            String::new()
        } else {
            ago_badge(card.recency.stamped)
        },
        Style::default().fg(th.dim),
        width,
        th,
    )];
    if card.sessions.is_empty() {
        lines.push(vec![Span::styled(
            "  no sessions yet",
            Style::default().fg(th.dim),
        )]);
    }
    let (shown, left) = listed(card.sessions.len());
    for a in card.sessions.iter().take(shown) {
        lines.push(child_row(
            status_dot(Some(a.status), a.unseen, th),
            &a.name,
            ago_badge(a.status_changed_at),
            width,
            th,
        ));
    }
    if left > 0 {
        lines.push(more_row(left, th));
    }
    draw_card_lines(f, inner, lines);
}

/// One workspace's card: its name with the rolled-up dot and the count of
/// what is running in it, then its projects under it in the PROJECTS
/// level's own order — so the card is a preview of the level Enter opens.
fn draw_workspace_card(
    f: &mut Frame,
    area: Rect,
    card: &WorkspaceCard,
    selected: bool,
    focused: bool,
    th: Theme,
) {
    let inner = card_frame(f, area, selected, focused, th);
    let width = inner.width as usize;
    if width == 0 {
        return;
    }
    let mut lines = vec![name_row(
        status_dot(card.status, card.unseen, th),
        &card.name,
        if card.sessions == 0 {
            String::new()
        } else {
            format!(" {} session{}", card.sessions, plural(card.sessions))
        },
        Style::default().fg(th.dim),
        width,
        th,
    )];
    if card.projects.is_empty() {
        lines.push(vec![Span::styled(
            "  no projects yet",
            Style::default().fg(th.dim),
        )]);
    }
    let (shown, left) = listed(card.projects.len());
    for p in card.projects.iter().take(shown) {
        lines.push(child_row(
            status_dot(p.status, p.unseen, th),
            &p.name,
            if p.sessions.is_empty() {
                String::new()
            } else {
                ago_badge(p.recency.stamped)
            },
            width,
            th,
        ));
    }
    if left > 0 {
        lines.push(more_row(left, th));
    }
    draw_card_lines(f, inner, lines);
}

/// Put a card's rows down its inside, skipping the blank ones.
fn draw_card_lines(f: &mut Frame, inner: Rect, lines: Vec<Vec<Span<'static>>>) {
    for (i, spans) in lines.into_iter().enumerate() {
        if spans.is_empty() {
            continue;
        }
        let Some(r) = row_rect(inner, i) else { break };
        f.render_widget(Paragraph::new(Line::from(spans)), r);
    }
}

/// What a level above the sessions shows with no cards at all — one line
/// where the grid would be, rather than the SESSIONS level's hero, which
/// is about starting a session and has nothing to say here.
fn draw_level_empty(f: &mut Frame, app: &mut App, area: Rect, what: &str) {
    app.hits.push((area, HitTarget::PanelBg(Focus::Sessions)));
    if app.overlay.is_some() {
        return;
    }
    let th = app.theme;
    if let Some(r) = row_rect(area, 1) {
        f.render_widget(
            Paragraph::new(Line::from(Span::styled(
                what.to_string(),
                Style::default().fg(th.dim),
            )))
            .alignment(ratatui::layout::Alignment::Center),
            r,
        );
    }
}

/// The prompt block at the foot of a card: the `›` on its first row and
/// the sentence wrapped under it, indented to the same column, over at
/// most [`crate::launcher::PROMPT_LINES`] rows — so a card is a fixed
/// height whatever it was asked to do. A prompt longer than that is cut
/// on the last of them with an ellipsis; an empty one draws nothing.
fn prompt_lines(prompt: &str, width: usize, th: Theme) -> Vec<Vec<Span<'static>>> {
    const MARK: &str = "› ";
    let indent = MARK.chars().count();
    let body = width.saturating_sub(indent);
    if prompt.is_empty() || body == 0 {
        return Vec::new();
    }
    let wrapped = crate::pr_preview::wrap(prompt, body);
    let keep = crate::launcher::PROMPT_LINES;
    let cut = wrapped.len() > keep;
    wrapped
        .into_iter()
        .take(keep)
        .enumerate()
        .map(|(i, line)| {
            let last = i + 1 == keep;
            let text = if last && cut {
                truncate(&format!("{line}…"), body)
            } else {
                line
            };
            vec![
                if i == 0 {
                    Span::styled(MARK, Style::default().fg(th.dim))
                } else {
                    Span::raw(" ".repeat(indent))
                },
                Span::styled(text, Style::default().fg(th.muted)),
            ]
        })
        .collect()
}

/// The harness (and model) a session runs on, as its card names it:
/// `claude opus`. A Claude Cloud row says `cloud` — the sandbox is the
/// harness that matters there.
///
/// `cfg` is the frame's CONFIG.JSON slot, filled on the first card that
/// needs it: only a CUSTOM harness's label comes out of the file, so a
/// grid of built-ins never opens it at all.
fn harness_line(a: &nebula_core::Agent, cfg: &mut Option<crate::config::Config>) -> String {
    if a.cloud_session_id.is_some() {
        return "cloud".into();
    }
    let mut out = if a.kind == nebula_core::AgentKind::Custom {
        let cfg = cfg.get_or_insert_with(crate::config::Config::load);
        crate::agent_picker::session_harness_badge_in(a, cfg)
    } else {
        a.kind.as_str().to_string()
    };
    if let Some(model) = a.model.as_deref().filter(|m| !m.is_empty()) {
        out.push(' ');
        out.push_str(model);
    }
    out
}

/// The grid with nothing in it: the wordmark and the keys that get a
/// session going, where the cards would be.
fn draw_empty(f: &mut Frame, app: &mut App, area: Rect) {
    app.hits.push((area, HitTarget::PanelBg(Focus::Sessions)));
    // Under the box (or any modal) the hero would only peek out around
    // its edges in fragments; the box is saying the same thing.
    if app.overlay.is_some() {
        return;
    }
    let th = app.theme;
    let key = |k: String, label: &str| {
        vec![
            Span::styled(
                k,
                Style::default().fg(th.accent).add_modifier(Modifier::BOLD),
            ),
            Span::styled(format!(" {label}"), Style::default().fg(th.dim)),
        ]
    };
    let sep = || Span::styled("   ·   ", Style::default().fg(th.dim));
    let mut hint = key(
        super::key_hint(app, crate::keymap::Action::QuickPrompt),
        "new session",
    );
    hint.push(sep());
    hint.extend(key("^P".into(), "project (in the box)"));
    hint.push(sep());
    hint.extend(key(
        super::key_hint(app, crate::keymap::Action::Help),
        "help",
    ));
    let mut lines = Vec::new();
    for _ in 0..area.height.saturating_sub(5) / 2 {
        lines.push(Line::from(""));
    }
    lines.push(Line::from(vec![
        Span::styled("◆ ", Style::default().fg(th.accent)),
        Span::styled(
            "nebula",
            Style::default().fg(th.text).add_modifier(Modifier::BOLD),
        ),
    ]));
    lines.push(Line::from(Span::styled(
        "type a task — each one gets a session of its own",
        Style::default().fg(th.dim),
    )));
    lines.push(Line::from(""));
    lines.push(Line::from(hint));
    f.render_widget(Paragraph::new(lines).centered(), area);
}

/// The full-screen session's own header, in place of the pane's
/// `TERMINAL · name`: a breadcrumb back to the grid — `‹ sessions` is a
/// button, the session's name the crumb it leads out of — with the
/// harness it runs on and where, right-aligned. Returns the rect the PTY
/// draws in, as `terminal_frame` does.
pub(super) fn crumb_frame(f: &mut Frame, app: &mut App, area: Rect) -> Rect {
    let th = app.theme;
    if let Some(r) = row_rect(area, 1) {
        let r = pad_x(r);
        let back = format!("‹ {CRUMB}");
        let mut spans = vec![Span::styled(
            back.clone(),
            Style::default().fg(th.accent).add_modifier(Modifier::BOLD),
        )];
        // Only the crumb itself is the button — a click anywhere else on
        // the header row belongs to the pane.
        app.hits.push((
            Rect {
                width: back.chars().count() as u16,
                height: 1,
                ..r
            },
            HitTarget::LauncherCrumb,
        ));
        let row = app
            .term
            .as_ref()
            .map(|t| t.sref.clone())
            .and_then(|sref| match sref {
                nebula_core::SessionRef::Agent(id) => crate::launcher::row(app, &id),
                _ => None,
            });
        if let Some(row) = &row {
            let a = &row.agent;
            spans.push(Span::styled(" / ", Style::default().fg(th.dim)));
            spans.push(status_dot(Some(a.status), a.unseen, th));
            spans.push(Span::styled(
                truncate(&a.name, usize::from(r.width / 2).max(8)),
                Style::default().fg(th.text).add_modifier(Modifier::BOLD),
            ));
        }
        f.render_widget(Paragraph::new(Line::from(spans)), r);
        if let Some(row) = &row {
            let a = &row.agent;
            let mut right = vec![Span::styled(
                harness_line(a, &mut None),
                Style::default().fg(th.muted),
            )];
            if let Some(effort) = a.effort.as_deref().filter(|e| !e.is_empty()) {
                right.push(Span::styled(
                    format!(" {effort}"),
                    Style::default().fg(th.dim),
                ));
            }
            right.push(Span::styled(" · ", Style::default().fg(th.dim)));
            right.push(Span::styled(
                format!(
                    "{} / {}",
                    truncate(&row.project, 20),
                    truncate(&row.branch, 24)
                ),
                Style::default().fg(th.dim),
            ));
            f.render_widget(
                Paragraph::new(Line::from(right)).alignment(ratatui::layout::Alignment::Right),
                r,
            );
        }
    }
    let focused = app.focus == Focus::Terminal;
    draw_rule(f, area, 2, if focused { th.accent } else { th.edge });
    Rect {
        y: area.y + 3,
        height: area.height.saturating_sub(3),
        ..area
    }
}

/// The view's box, row 0: the three things a chord changes — where the
/// session runs, what runs there and on which model — each named with the
/// chord that changes it, set far enough apart that no two read as one
/// phrase.
///
/// Widest form that fits, in order: labelled and airy; the values and
/// their chords alone; then the same without the model, and without the
/// agent. The project is never dropped, only cut — where a session lands
/// is the one thing worth a whole row on its own.
pub(super) fn detail_line(app: &App, launch: &QuickLaunch, width: u16, th: Theme) -> Line<'static> {
    let harness = launch
        .custom
        .as_deref()
        .unwrap_or_else(|| launch.kind.as_str())
        .to_string();
    let mut model = launch.model.clone().unwrap_or_else(|| "default".into());
    if let Some(effort) = launch.effort.as_deref().filter(|e| !e.is_empty()) {
        model.push(' ');
        model.push_str(effort);
    }
    let project = launch_project(app, launch);
    let width = usize::from(width);
    for (labels, gap, fields) in [
        (true, DETAIL_GAP, 3),
        (false, DETAIL_TIGHT, 3),
        (false, DETAIL_TIGHT, 2),
        (false, DETAIL_TIGHT, 1),
    ] {
        let spans = detail_spans(&project, &harness, &model, fields, labels, gap, th);
        let w: usize = spans.iter().map(|s| s.width()).sum();
        if w <= width {
            return Line::from(spans);
        }
        // This tier with the project cut to what is left over — but not
        // past the point where the name stops saying which project it is.
        let keep = project.chars().count().saturating_sub(w - width);
        if keep >= MIN_PROJECT {
            let cut = truncate(&project, keep);
            return Line::from(detail_spans(
                &cut, &harness, &model, fields, labels, gap, th,
            ));
        }
    }
    // Narrower than the project and its chord together: cut it to the row.
    let cut = truncate(&project, width.saturating_sub(3));
    Line::from(detail_spans(
        &cut,
        &harness,
        &model,
        1,
        false,
        DETAIL_TIGHT,
        th,
    ))
}

/// Shortest a project name is cut to before [`detail_line`] gives up a
/// whole field instead.
const MIN_PROJECT: usize = 8;

/// The gap between two of [`detail_line`]'s fields — wide enough that
/// `api-server ^P` and `agent claude` never read as one phrase — and the
/// one a box too narrow for that falls back to.
const DETAIL_GAP: &str = "   ·   ";
const DETAIL_TIGHT: &str = " · ";

/// [`detail_line`]'s row at one tier: `fields` of them, each a `value
/// ^key` with the word for it ahead when `labels`, held apart by `gap`.
fn detail_spans(
    project: &str,
    harness: &str,
    model: &str,
    fields: usize,
    labels: bool,
    gap: &str,
    th: Theme,
) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    for (i, (label, value, key)) in [
        ("project", project, "^P"),
        ("agent", harness, "Tab"),
        ("model", model, "^O"),
    ]
    .into_iter()
    .take(fields)
    .enumerate()
    {
        if i > 0 {
            spans.push(Span::styled(gap.to_string(), Style::default().fg(th.edge)));
        }
        if labels {
            spans.push(Span::styled(
                format!("{label} "),
                Style::default().fg(th.dim),
            ));
        }
        spans.push(Span::styled(
            value.to_string(),
            Style::default().fg(th.text).add_modifier(Modifier::BOLD),
        ));
        spans.push(Span::styled(
            format!(" {key}"),
            Style::default().fg(th.accent),
        ));
    }
    spans
}

/// The PROJECT a launch is aimed at, by name.
fn launch_project(app: &App, launch: &QuickLaunch) -> String {
    crate::launcher::project_of(app, &launch.target)
        .and_then(|id| crate::launcher::project_name(app, &id))
        .unwrap_or_else(|| "(project gone)".into())
}

/// Narrowest the prompt header cuts the question down to before it drops
/// the crumb instead.
const LABEL_FLOOR: usize = 22;

/// Air between the question and the crumb that says where it lands.
const CRUMB_GAP: usize = 2;

/// The view's box prompt header: what Enter sends on the left, the
/// checkout it lands in right after it as `(project / worktree)` — beside
/// the question rather than a row away from it — and the toggle that cuts
/// a fresh one, right. A PR SESSION says what the DAEMON will do with the
/// head branch instead, its checkout not being ours to flip.
///
/// Returns the line and the toggle's own columns within it, so a click
/// there can be given the same flip `^N` has
/// ([`crate::app::PromptDialog::toggle_area`]).
pub(super) fn target_line(
    app: &App,
    launch: &QuickLaunch,
    label: &str,
    width: u16,
    th: Theme,
) -> (Line<'static>, Option<(u16, u16)>) {
    let dim = Style::default().fg(th.dim);
    let fresh = launch.is_new_worktree();
    let branch = match &launch.pr {
        Some(pr) => pr.head.clone(),
        None => crate::quick_prompt::target_branch(app, launch)
            .unwrap_or_else(|| "(worktree gone)".into()),
    };

    // Right: the toggle and its state, or — on a PR SESSION — what the
    // DAEMON will do with the head branch, there being nothing to flip.
    let (right, clickable) = if launch.pr.is_some() {
        (vec![Span::styled("reused or cut on Enter", dim)], false)
    } else if fresh {
        let on = Style::default().fg(th.ok).add_modifier(Modifier::BOLD);
        (
            vec![
                Span::styled("[✓] new worktree", on),
                Span::styled(" ^N", Style::default().fg(th.ok)),
            ],
            true,
        )
    } else {
        (
            vec![
                Span::styled("[ ] new worktree", dim),
                Span::styled(" ^N", dim),
            ],
            true,
        )
    };
    let right_w: usize = right.iter().map(|s| s.width()).sum();

    // The crumb, longest form first: with the project, then the branch
    // alone — the branch is the half that changes — then nothing, rather
    // than crowd the question below its floor.
    let branch_style = Style::default()
        .fg(if fresh { th.ok } else { th.muted })
        .add_modifier(Modifier::BOLD);
    // A box aimed away from the grid behind it fires a BACKGROUND LAUNCH:
    // the project is the half that changed, and nothing on screen will
    // move when Enter lands, so the crumb says so up front.
    let background = crate::launcher::project_of(app, &launch.target)
        .is_some_and(|project| crate::launcher::is_background(app, &project));
    let project_style = if background {
        Style::default().fg(th.accent).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(th.muted)
    };
    let crumbs = [
        vec![
            Span::styled("(", dim),
            Span::styled(launch_project(app, launch), project_style),
            Span::styled(" / ", dim),
            Span::styled(branch.clone(), branch_style),
            Span::styled(")", dim),
        ],
        vec![
            Span::styled("(", dim),
            Span::styled(branch, branch_style),
            Span::styled(")", dim),
        ],
    ];
    let width = usize::from(width);
    let room = width.saturating_sub(right_w + 1);
    let label_w = label.chars().count();
    let crumb = crumbs.into_iter().find(|c| {
        let w: usize = c.iter().map(|s| s.width()).sum::<usize>() + CRUMB_GAP;
        room.saturating_sub(w) >= LABEL_FLOOR.min(label_w)
    });
    let crumb_w = crumb.as_ref().map_or(0, |c| {
        c.iter().map(|s| s.width()).sum::<usize>() + CRUMB_GAP
    });

    let label = truncate(label, room.saturating_sub(crumb_w));
    let mut used = label.chars().count();
    let mut spans = vec![Span::styled(label, dim)];
    if let Some(crumb) = crumb {
        spans.push(Span::raw(" ".repeat(CRUMB_GAP)));
        used += crumb_w;
        spans.extend(crumb);
    }
    // The right half goes whole or not at all, as the panels' box's does.
    let mut toggle = None;
    if width > used + right_w {
        let pad = width - used - right_w;
        spans.push(Span::raw(" ".repeat(pad)));
        if clickable {
            toggle = Some(((used + pad) as u16, right_w as u16));
        }
        spans.extend(right);
    }
    (Line::from(spans), toggle)
}

/// The view's box title: what Enter starts. The harness, the model and
/// the effort are on [`detail_line`] under it, beside the chords that
/// change them; what is left here is what the box is *for* — an issue, a
/// pull request, an AGENT PRESET — as the panels' box names them.
pub(super) fn box_title(launch: &QuickLaunch) -> String {
    let mut head = vec!["New session".to_string()];
    if let Some(issue) = &launch.issue {
        head.push(format!("issue #{}", issue.number));
    }
    if let Some(pr) = &launch.pr {
        head.push(format!("PR #{}", pr.number));
    }
    if let Some(preset) = &launch.preset {
        head.push(preset.name.clone());
    }
    head.join(" · ")
}

/// The view's box hints, widest that fits in `width`. `^P`, `Tab`, `^O`
/// and `^N` are not here: each one is now inside the box beside the thing
/// it changes, and a second copy along the border was most of what made
/// this box read as a wall of text.
pub(super) fn box_hint(width: u16) -> &'static str {
    if width >= 60 {
        " Enter launch · ⇧Enter newline · ⇧Tab preset · Esc cancel "
    } else if width >= 43 {
        " Enter launch · ⇧Tab preset · Esc cancel "
    } else if width >= 25 {
        " ↵ launch · Esc cancel "
    } else {
        " ↵ · Esc "
    }
}

/// Where the view's QUICK PROMPT is drawn — one place, so the PROJECT
/// PICKER can float over exactly the rect the box is in.
pub(super) fn box_rect(frame: Rect) -> Rect {
    centered_rect(frame, BOX_SIZE.0, BOX_SIZE.1)
}

/// Does the PROJECT PICKER float over the box, rather than stand on its
/// own in the middle of the screen? Only when a box was up to come back
/// to, and only in the view whose box `^P` belongs to.
pub(super) fn picker_over_box(app: &App, picker: &ProjectPicker) -> bool {
    app.launcher && picker.back.from_box
}

/// Where the PROJECT PICKER goes, sized to the list it has to show.
/// Opened from the box, it floats over it: inset inside the box's rect,
/// so the box's frame, its title and its details row stay on screen
/// around the list. The list gives up the rows that costs — the box
/// behind is worth more than four more projects. With no box under it,
/// it is centered on the screen as any other modal.
fn picker_rect(frame: Rect, over: Option<Rect>, matches: usize) -> Rect {
    // Unlike the menus that float over the box, the picker shrinks to fit
    // inside it rather than spilling back out to the middle of the
    // screen: a list has rows to give up, and the box behind is worth
    // more than four more projects.
    let max_rows = over
        .map(|b| b.height.saturating_sub(OVER_BOX_INSET + 4))
        .unwrap_or(PICKER_ROWS)
        .clamp(1, PICKER_ROWS);
    let rows = (matches as u16).clamp(1, max_rows);
    let width = over.map_or(PICKER_W, |b| {
        PICKER_W.min(b.width.saturating_sub(OVER_BOX_INSET))
    });
    over_box_rect(frame, over, width, rows + 4)
}

/// The PROJECT PICKER: the query on top, the projects under it — the name
/// with the typed letters lit, the path (and, for another workspace's, the
/// workspace) dim after it. Drawn over the box it was opened from
/// (`ui::draw_overlay` puts that box down first), inset inside it.
pub(super) fn draw_project_picker(f: &mut Frame, app: &mut App, picker: &ProjectPicker) {
    let th = app.theme;
    let over = picker_over_box(app, picker).then(|| box_rect(f.area()));
    let area = picker_rect(f.area(), over, picker.matches.len());
    let title = if picker.query.is_empty() {
        " Project ".to_string()
    } else {
        format!(
            " Project ({}/{}) ",
            picker.matches.len(),
            picker.projects.len()
        )
    };
    let inner = render_modal_frame(f, area, title, th);
    if let Some(query_area) = row_rect(inner, 0) {
        let line = search_line(&picker.query, "type a project name…", query_area, th);
        f.render_widget(Paragraph::new(line), query_area);
    }
    let list = below_first_row(inner);
    if picker.matches.is_empty() {
        empty_list_row(f, list, NO_MATCHES, th);
    }
    let selected = picker.selected.min(picker.matches.len().saturating_sub(1));
    let start = crate::app::window_start(selected, list.height as usize);
    for (row, (i, (index, positions))) in picker.matches.iter().enumerate().skip(start).enumerate()
    {
        let Some(row_area) = row_rect(list, row) else {
            break;
        };
        let project = &picker.projects[*index];
        let budget = (list.width as usize).saturating_sub(4);
        let name = truncate(&project.name, budget);
        let lit = visible_positions(positions, &name, &project.name);
        let mut spans = vec![Span::styled("▪ ", Style::default().fg(th.accent))];
        spans.extend(fuzzy_highlight_spans(&name, lit, th));
        let used = name.chars().count();
        let after = match &project.elsewhere {
            Some(workspace) => format!("  ◇ {workspace}"),
            None => format!("  {}", project.path),
        };
        if used + 2 < budget {
            spans.push(Span::styled(
                truncate(&after, budget - used),
                Style::default().fg(th.dim),
            ));
        }
        render_row(f, row_area, spans, i == selected, true, th);
    }
    if let Some(Overlay::ProjectPicker(p)) = &mut app.overlay {
        p.area = area;
        p.list_area = list;
        p.selected = selected;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `^P` opens the PROJECT PICKER *over* the box, so the rect it takes
    /// is strictly inside the box's on all four sides — the frame, the
    /// title and the details row above the list stay on screen however
    /// long the project list is. With no box under it the picker is
    /// centered on the screen, as it always was.
    #[test]
    fn the_picker_floats_inside_the_box_it_was_opened_from() {
        let frame = Rect::new(0, 0, 130, 34);
        let boxed = box_rect(frame);
        for matches in [0usize, 1, 3, 200] {
            let area = picker_rect(frame, Some(boxed), matches);
            assert!(
                area.x > boxed.x && area.right() < boxed.right(),
                "{matches}: {area:?} is not inside {boxed:?} sideways"
            );
            assert!(
                area.y > boxed.y && area.bottom() < boxed.bottom(),
                "{matches}: {area:?} is not inside {boxed:?} top to bottom"
            );
        }
        // Long list, no box: the screen's middle and the full height.
        let loose = picker_rect(frame, None, 200);
        assert_eq!(loose.height, PICKER_ROWS + 4);
        assert_eq!(loose, centered_rect(frame, PICKER_W, PICKER_ROWS + 4));
    }

    /// A terminal too small for the box to float anything inside still
    /// gets a picker — clamped, never a zero-sized or off-screen rect.
    #[test]
    fn a_tiny_screen_still_draws_the_picker() {
        for (w, h) in [(20u16, 6u16), (30, 9), (60, 12)] {
            let frame = Rect::new(0, 0, w, h);
            let area = picker_rect(frame, Some(box_rect(frame)), 50);
            assert!(area.width > 0 && area.height > 0, "{w}x{h}: {area:?}");
            assert!(
                area.right() <= frame.right() && area.bottom() <= frame.bottom(),
                "{w}x{h}: {area:?} runs off {frame:?}"
            );
        }
    }

    /// Every tier of the box's hint fits the border it is drawn in.
    #[test]
    fn every_box_hint_fits_its_border() {
        for width in 11..=120u16 {
            let hint = box_hint(width);
            assert!(
                hint.chars().count() + 2 <= width as usize,
                "{width}: {hint:?}"
            );
        }
        // The chords the box used to repeat along its border now live
        // beside what they change, so the hint must not name them again.
        for width in 11..=120u16 {
            let hint = box_hint(width);
            for chord in ["^P", "^O", "^N", "Tab agent"] {
                assert!(
                    !hint.contains(chord),
                    "{width}: {hint:?} still says {chord}"
                );
            }
        }
    }

    fn a_launch() -> QuickLaunch {
        let cfg = crate::config::Config::default();
        let target =
            crate::quick_prompt::QuickTarget::Worktree(nebula_core::WorktreeId("w".into()));
        QuickLaunch::of_kind(
            target,
            nebula_core::AgentKind::Claude,
            None,
            None,
            None,
            &cfg,
        )
    }

    fn text_of(line: &Line<'static>) -> String {
        line.spans.iter().map(|s| s.content.as_ref()).collect()
    }

    /// The title is what the box is *for*; the harness, the model and the
    /// effort moved down beside the chords that change them.
    #[test]
    fn the_box_title_is_what_the_box_is_for() {
        let mut launch = a_launch();
        launch.model = None;
        launch.effort = None;
        assert_eq!(box_title(&launch), "New session");
        launch.model = Some("opus".into());
        launch.effort = Some("high".into());
        assert_eq!(box_title(&launch), "New session");
    }

    /// The details row names all three values and all three chords, and
    /// fits the box it is drawn in.
    #[test]
    fn the_details_row_carries_every_chord() {
        let th = Theme::default();
        let app = App::new();
        let mut launch = a_launch();
        launch.model = None;
        launch.effort = None;
        let inner = BOX_SIZE.0 - 4;
        let line = detail_line(&app, &launch, inner, th);
        let text = text_of(&line);
        for want in [
            "project ", "^P", "agent ", "claude", "Tab", "model ", "default", "^O",
        ] {
            assert!(text.contains(want), "{text:?} is missing {want:?}");
        }
        assert!(line.width() <= inner as usize, "{text:?}");

        launch.model = Some("opus".into());
        launch.effort = Some("high".into());
        assert!(
            text_of(&detail_line(&app, &launch, inner, th)).contains("opus high"),
            "the effort belongs on the model"
        );
    }

    /// However narrow the box gets, the details row fits inside it and
    /// still says which project the launch is aimed at — it gives up a
    /// whole field before it clips a name in half.
    #[test]
    fn the_details_row_fits_every_width() {
        let th = Theme::default();
        let app = App::new();
        let mut launch = a_launch();
        launch.model = Some("opus".into());
        launch.effort = Some("high".into());
        for width in 10..=120u16 {
            let line = detail_line(&app, &launch, width, th);
            let text = text_of(&line);
            assert!(line.width() <= width as usize, "{width}: {text:?}");
            assert!(text.contains("^P"), "{width}: {text:?}");
        }
    }

    /// Two projects in the open workspace, one session in each: `api`
    /// with a `feat` checkout, `web` with one of its own. Both sessions
    /// start idle, so nothing sweeps until a test says it does.
    fn a_tree() -> App {
        use nebula_core::{
            Agent, AgentId, AgentKind, AgentStatus, Project, ProjectId, Workspace, WorkspaceId,
            Worktree, WorktreeId,
        };
        let mut app = App::new();
        app.tree.workspaces = vec![Workspace {
            id: WorkspaceId::default(),
            name: "default".into(),
        }];
        app.tree.projects = ["api", "web"]
            .iter()
            .enumerate()
            .map(|(i, name)| Project {
                workspace_id: WorkspaceId::default(),
                id: ProjectId(format!("p{i}")),
                name: (*name).into(),
                repo_path: format!("/tmp/{name}").into(),
                sort_order: 0,
            })
            .collect();
        app.tree.worktrees = (0..2)
            .map(|i| Worktree {
                id: WorktreeId(format!("w{i}")),
                project_id: ProjectId(format!("p{i}")),
                path: format!("/tmp/w{i}").into(),
                branch: "feat".into(),
                is_main: false,
                sort_order: 0,
            })
            .collect();
        app.tree.agents = (0..2)
            .map(|i| Agent {
                id: AgentId(format!("a{i}")),
                worktree_id: WorktreeId(format!("w{i}")),
                name: format!("s{i}"),
                status: AgentStatus::Finished,
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
            })
            .collect();
        app
    }

    /// Put the PROJECTS cursor on the named project. The cursor is a row
    /// index and a turn starting anywhere reorders the rows, so a test
    /// that starts one has to say again which project the grid is on.
    fn select(app: &mut App, name: &str) {
        app.sel_project = app
            .project_rows()
            .iter()
            .position(|i| app.tree.projects[*i].name == name)
            .expect("a row for the project");
    }

    /// A tally with something in every state, so a test can tell the
    /// dots apart by their counts as well as by their colors.
    fn a_tally() -> Tally {
        Tally {
            needs_you: 2,
            done: 1,
            running: 3,
            merged: 4,
        }
    }

    /// The row as one string, the way it lands on screen.
    fn row_text(spans: &[Span<'static>]) -> String {
        spans.iter().map(|s| s.content.as_ref()).collect()
    }

    /// The header's trail never moves and never animates: whatever the
    /// work under it is doing, the words are the same words in the same
    /// columns, with the STATUS TALLY's dots after them.
    #[test]
    fn the_header_trail_stands_still_whatever_is_running() {
        use nebula_core::AgentStatus;
        let r = Rect::new(0, 0, 80, 1);
        let mut app = a_tree();
        let trail = "nebula / default / api / sessions";
        select(&mut app, "api");
        let quiet = head_crumbs(&mut app, r, 0, Tally::default());
        assert_eq!(row_text(&quiet), trail);
        // One span per word, not one per cell: nothing is sweeping.
        for word in ["nebula", "default", "api", "sessions"] {
            assert!(
                quiet.iter().any(|s| s.content == word),
                "{word} is not one still span: {:?}",
                row_text(&quiet)
            );
        }

        // Every session mid-turn — what used to light the whole trail.
        for a in &mut app.tree.agents {
            a.status = AgentStatus::Running;
        }
        select(&mut app, "api");
        app.hits.clear();
        let busy = head_crumbs(&mut app, r, 0, Tally::default());
        assert_eq!(row_text(&busy), trail, "the trail moved with the work");
        assert_eq!(quiet.len(), busy.len(), "and took the same spans to say");
    }

    /// The TALLY: a dot and a count per state with a card in it, in the
    /// one order — needs-you red, done blue, running yellow, merged purple
    /// — with no word of its own, after the trail and never over it.
    #[test]
    fn the_header_tallies_the_grid_in_dots() {
        let r = Rect::new(0, 0, 80, 1);
        let mut app = a_tree();
        let th = app.theme;
        select(&mut app, "api");
        let spans = head_crumbs(&mut app, r, 0, a_tally());
        let text = row_text(&spans);
        assert_eq!(
            text, "nebula / default / api / sessions  ● 2  ● 1  ● 3  ● 4",
            "the trail, then the dots"
        );
        // Each count in its own state's color, in that order.
        let dots: Vec<(String, Option<Color>)> = spans
            .iter()
            .filter(|s| s.content.contains('●'))
            .map(|s| (s.content.to_string(), s.style.fg))
            .collect();
        assert_eq!(
            dots,
            vec![
                ("  ● 2".to_string(), Some(th.err)),
                ("  ● 1".to_string(), Some(th.done)),
                ("  ● 3".to_string(), Some(th.warn)),
                ("  ● 4".to_string(), Some(th.merged)),
            ]
        );

        // A state with nothing in it takes no columns at all.
        app.hits.clear();
        let spans = head_crumbs(
            &mut app,
            r,
            0,
            Tally {
                running: 1,
                ..Tally::default()
            },
        );
        assert_eq!(row_text(&spans), "nebula / default / api / sessions  ● 1");
    }

    /// The tally is reserved before the names are, so it never prints over
    /// the trail — and a click on a crumb lands in the same columns with
    /// the dots there as without. On a row with nothing to spare the dots
    /// go, rather than leave half a word under one.
    #[test]
    fn the_tally_never_moves_the_words_a_click_lands_on() {
        let r = Rect::new(0, 0, 80, 1);
        let mut app = a_tree();
        select(&mut app, "api");
        let bare = head_crumbs(&mut app, r, 0, Tally::default());
        let bare_hits = std::mem::take(&mut app.hits);

        select(&mut app, "api");
        let tallied = head_crumbs(&mut app, r, 0, a_tally());
        let tallied_hits = std::mem::take(&mut app.hits);
        assert!(row_text(&tallied).starts_with(&row_text(&bare)));
        assert_eq!(
            bare_hits
                .iter()
                .map(|(r, h)| (*r, h.clone()))
                .collect::<Vec<_>>(),
            tallied_hits
                .iter()
                .map(|(r, h)| (*r, h.clone()))
                .collect::<Vec<_>>(),
            "the same words to click on"
        );

        // Nothing fits but the head of the trail: the dots give way too.
        for width in 8..=120u16 {
            let row = Rect::new(0, 0, width, 1);
            app.hits.clear();
            let spans = head_crumbs(&mut app, row, 0, a_tally());
            let text = row_text(&spans);
            assert!(text.starts_with(ROOT), "{width}: {text:?}");
            assert!(
                spans.iter().map(|s| s.width()).sum::<usize>() <= width as usize,
                "{width}: {text:?} overruns its row"
            );
        }
    }

    /// The prompt header keeps the toggle whatever else it has to drop,
    /// never overruns its row, and hands back the columns a click on the
    /// toggle lands in.
    #[test]
    fn the_prompt_header_never_drops_the_toggle() {
        let th = Theme::default();
        let app = App::new();
        let launch = a_launch();
        let label = "what should the agent do?";
        for width in 40..=120u16 {
            let (line, toggle) = target_line(&app, &launch, label, width, th);
            let text = text_of(&line);
            assert!(line.width() <= width as usize, "{width}: {text:?}");
            assert!(text.contains("new worktree"), "{width}: {text:?}");
            let (x, w) = toggle.expect("a worktree launch has a toggle to click");
            assert_eq!(usize::from(x + w), line.width(), "{width}: {text:?}");
        }
    }
    /// A card names its checkout in the SCOPE COLOR — `⌂` root, `↳`
    /// worktree — glyph and branch both, and nothing else on that row, so
    /// the two scopes sort a screenful of cards before a word is read.
    #[test]
    fn a_card_paints_its_checkout_in_the_scope_color() {
        use nebula_core::{Agent, AgentId, AgentKind, AgentStatus, WorktreeId};

        fn card(is_main: bool, branch: &str) -> LauncherRow {
            LauncherRow {
                agent: Agent {
                    id: AgentId("a1".into()),
                    worktree_id: WorktreeId("w1".into()),
                    name: "fix login".into(),
                    status: AgentStatus::Finished,
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
                },
                project: "nebula".into(),
                branch: branch.into(),
                is_main,
                pr: None,
            }
        }

        let th = Theme::default();
        let app = App::new();
        let area = Rect::new(0, 0, 44, crate::launcher::CARD_H);
        let painted = |row: &LauncherRow, want: Color| -> String {
            let mut terminal =
                ratatui::Terminal::new(ratatui::backend::TestBackend::new(area.width, area.height))
                    .unwrap();
            terminal
                .draw(|f| draw_card(f, &app, area, row, false, true, th, &mut None))
                .unwrap();
            let buf = terminal.backend().buffer().clone();
            // Row 2 of the buffer is the card's second line: one row of
            // border, one of name, then this one.
            (0..area.width)
                .filter_map(|x| buf.cell((x, 2)))
                .filter(|c| c.fg == want)
                .map(|c| c.symbol().to_string())
                .collect()
        };

        let root = card(true, "main");
        let worktree = card(false, "feat-x");
        assert_eq!(painted(&root, th.root), "⌂ main");
        assert_eq!(painted(&worktree, th.worktree), "↳ feat-x");
        // And neither wears the other's color anywhere on that row.
        assert_eq!(painted(&root, th.worktree), "");
        assert_eq!(painted(&worktree, th.root), "");
    }
}
