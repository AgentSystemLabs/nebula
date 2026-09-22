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
use crate::launcher::{BoxField, Hidden, LauncherRow, ProjectPicker, ProjectTab, Tally};
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
/// What a full-screen session's back button says — the word a click on
/// it goes back to.
const CRUMB: &str = "sessions";
/// Longest a project's name is drawn on its PROJECT TAB before it is
/// clipped, so one long name cannot push every other tab off the row.
const PROJECT_TAB_MAX: usize = 20;
/// A name the row has to cut is never cut under this: at that point the
/// tab gives way whole, and the count at the edge of the row says so.
const TAB_NAME_MIN: usize = 3;
/// The button before the first PROJECT TAB, and what it says with no tab
/// beside it — the one time the header has room to say what it does.
/// A column of air either side is part of the button, so the pointer has
/// more than one cell to find.
const ADD: &str = "+";
const ADD_EMPTY: &str = "+ open a project";

/// The view's area, with the view on: the PROJECT TABS header, then the
/// GRID of the lit project's session cards under it. `body` is what
/// `crate::launcher::split` left over the PANE along the bottom, which
/// `ui::draw` fills with the card under the cursor; Enter on a session
/// card steps down into that pane (`event_loop::launcher::enter_pane`)
/// and `z` full-screens it over the lot
/// (`event_loop::launcher::open_session`, the `collapsed` arm of
/// `ui::draw`).
pub(super) fn draw(f: &mut Frame, app: &mut App, body: Rect) {
    app.body_area = body;
    app.settle_launcher_focus();
    // A TAB STRIP pin the cursor has walked out from under goes here, so
    // the strip and the pane under it agree on this frame.
    app.settle_pane_tab();
    // And the project the grid is on gets its tab, whichever way it was
    // opened, before the header lays the tabs out.
    app.settle_project_tabs();
    let g = crate::launcher::grid(body);
    let rows = crate::launcher::rows(app);
    let hidden = g.hidden(crate::launcher::cursor(app, &rows), rows.len());
    draw_head(f, app, body, rows.len(), hidden);
    if rows.is_empty() {
        // Nothing archived is not nothing at all: the hero's "type a
        // task" would be advice about the wrong list, so the ARCHIVED
        // VIEW gets the plain empty line and the way back out of it.
        if app.show_archived {
            draw_list_empty(f, app, g.area, NO_ARCHIVED);
        } else {
            draw_empty(f, app, g.area);
        }
        return;
    }
    draw_grid(f, app, g, &rows);
}

/// What the ARCHIVED VIEW says with nothing in it.
const NO_ARCHIVED: &str = "nothing archived in this project — ⇧A back to the live sessions";

/// The header over the grid: the PROJECT TABS on the left, each with its
/// status dots, how many cards the grid holds on the right — and how many
/// of them it could not fit — a rule under both: the same three-row head
/// the panels' columns sit on.
fn draw_head(f: &mut Frame, app: &mut App, body: Rect, count: usize, hidden: Hidden) {
    let th = app.theme;
    if let Some(r) = row_rect(body, 1) {
        let r = pad_x(r);
        let right = head_count(app, count, hidden, r.width as usize, th);
        let used: usize = right.iter().map(|(s, _)| s.width()).sum();
        f.render_widget(Paragraph::new(Line::from(head_tabs(app, r, used))), r);
        // The PR & ISSUE COUNTS are buttons, laid down where the
        // right-aligned row puts each word: a click opens that list for
        // the project in front of you, as `v` and `i` do.
        let mut x = r.x + (r.width as usize).saturating_sub(used) as u16;
        let mut spans = Vec::with_capacity(right.len());
        for (span, hit) in right {
            let width = span.width() as u16;
            if let Some(hit) = hit {
                app.hits.push((Rect { x, width, ..r }, hit));
            }
            x += width;
            spans.push(span);
        }
        f.render_widget(
            Paragraph::new(Line::from(spans)).alignment(ratatui::layout::Alignment::Right),
            r,
        );
    }
    draw_rule(f, body, 2, th.edge);
}

/// The header's PROJECT TABS: one tab per project opened since its tab
/// was last closed, behind a `+` for the rest, the most recently opened
/// next to it ([`crate::launcher::project_tabs`]) — where the `+` puts
/// the project it opens. The tab the grid is on is lit — a raised chip,
/// its name in the accent — and a click on any other opens it, as `[`
/// and `]` walking onto it do (`event_loop::launcher::open_tab`). The `×`
/// on a tab closes it
/// (`event_loop::launcher::close_tab`) — all but the last one, which is
/// the project on screen and has nowhere to hand the grid to; the `+`
/// drops the PROJECT DROPDOWN under itself, every project narrowed by
/// whatever you type plus a row that opens a folder, and the pick opens a
/// tab (`event_loop::launcher::open_project_menu`). A right-click on a
/// tab opens that project with its menu over it.
///
/// Each tab carries its project's STATUS DOTS after the name — waiting on
/// you (red), finished unread (blue), working (yellow), each with its
/// count and no word ([`tab_dots`]) — so a project that wants you says so
/// from the header, whichever project the grid is on. Its name sweeps on
/// the loudest of the three ([`tab_ramp`]), so the tab says it in motion
/// too.
///
/// The hit rects are laid down as the spans are measured, so a click
/// lands on the tab itself and never on the air between two, and the
/// dropdown hangs off the `+`'s own rect.
///
/// With the keys up here (`k`,`k` off the top row of cards — see
/// [`App::launcher_tab_cursor`]) the header has a cursor of its own: the
/// tab it is on wears the accent as a solid block, the way a focused
/// title chip does, whichever tab is lit.
///
/// `taken` is what the count on the right of the same row has already
/// spent: the tabs get the rest, less a column of air. Tabs that will not
/// fit are counted at the edge they went past (`‹2`, `3›`) rather than
/// drawn half, and the window always holds the header's cursor, or with
/// none the lit tab.
fn head_tabs(app: &mut App, r: Rect, taken: usize) -> Vec<Span<'static>> {
    let th = app.theme;
    let hover = app.hover_crumb.clone();
    let sweep = app.animations.then(|| app.sweep_phase());
    let tabs = crate::launcher::project_tabs(app);
    let room = (r.width as usize).saturating_sub(taken + 2);
    let add = if tabs.is_empty() { ADD_EMPTY } else { ADD };
    let add_w = add.chars().count() + 2;
    // The `+` is laid out first: it is the only way to a project with no
    // tab, so the tabs shrink around it rather than push it off the row.
    let budget = room.saturating_sub(add_w + 1);
    // A lone tab is the project on screen with nothing to hand the grid
    // to, so it carries no `×` (`event_loop::launcher::close_tab` refuses
    // it too).
    let closable = tabs.len() > 1;
    let mut chips: Vec<[PaneTab; 2]> = tabs
        .iter()
        .map(|t| project_chip(t, PROJECT_TAB_MAX, hover.as_ref(), closable, sweep, th))
        .collect();
    let width = |chip: &[PaneTab; 2]| chip.iter().map(PaneTab::width).sum::<usize>();
    // The tabs that fit from `start` in `budget` columns: the index one
    // past the last. Room is kept for the count of those left over on the
    // right, except behind the very last tab, which leaves nothing over.
    let fit = |chips: &[[PaneTab; 2]], start: usize, budget: usize| -> usize {
        let mut left = budget;
        let mut end = start;
        for (i, chip) in chips.iter().enumerate().skip(start) {
            let gap = usize::from(i > start);
            let spare = if i + 1 == chips.len() { 0 } else { MORE_ROOM };
            if gap + width(chip) + spare > left {
                break;
            }
            left -= gap + width(chip);
            end = i + 1;
        }
        end
    };
    let lit = tabs
        .iter()
        .position(|t| t.focused)
        .or_else(|| tabs.iter().position(|t| t.active));
    let mut start = 0;
    let mut end = fit(&chips, 0, budget);
    if let Some(lit) = lit.filter(|lit| *lit >= end) {
        start = lit;
        end = fit(&chips, start, budget.saturating_sub(MORE_ROOM));
    }
    // Not even one tab whole: the one the window starts on is cut to what
    // is left, down to TAB_NAME_MIN, before it gives way altogether.
    if end == start && start < chips.len() {
        let markers = MORE_ROOM * (usize::from(start > 0) + usize::from(start + 1 < chips.len()));
        let overhead = width(&chips[start]) - tabs[start].name.chars().count().min(PROJECT_TAB_MAX);
        let name_room = budget.saturating_sub(markers + overhead);
        if name_room >= TAB_NAME_MIN {
            chips[start] =
                project_chip(&tabs[start], name_room, hover.as_ref(), closable, sweep, th);
            end = start + 1;
        }
    }
    // And with no room even for that, the count of them all stands in for
    // the tabs — if there is room for the count.
    if end == start {
        (start, end) = (0, 0);
    }

    // The `+` leads the row, on the side a project it opens lands on: a
    // button after the last tab would read as appending one there.
    let mut row: Vec<PaneTab> = Vec::new();
    if add_w <= room {
        let mut style = Style::default().fg(th.muted);
        if hover.as_ref() == Some(&HitTarget::LauncherTabAdd) {
            style = Style::default()
                .fg(th.accent)
                .add_modifier(Modifier::UNDERLINED);
        }
        row.push(PaneTab {
            spans: vec![Span::raw(" "), Span::styled(add, style), Span::raw(" ")],
            hit: Some(HitTarget::LauncherTabAdd),
        });
    }
    let mut tabs_row: Vec<PaneTab> = Vec::new();
    if start > 0 {
        tabs_row.push(PaneTab::plain(vec![Span::styled(
            format!("‹{start} "),
            Style::default().fg(th.dim),
        )]));
    }
    let len = chips.len();
    for (i, chip) in chips.into_iter().enumerate().take(end).skip(start) {
        if i > start {
            tabs_row.push(PaneTab::plain(vec![Span::raw(" ")]));
        }
        tabs_row.extend(chip);
    }
    let used: usize = tabs_row.iter().map(PaneTab::width).sum();
    let right = PaneTab::plain(vec![Span::styled(
        format!(" {}›", len - end),
        Style::default().fg(th.dim),
    )]);
    if end < len && used + right.width() <= budget {
        tabs_row.push(right);
    }
    if !row.is_empty() && !tabs_row.is_empty() {
        row.push(PaneTab::plain(vec![Span::raw(" ")]));
    }
    row.extend(tabs_row);

    let mut spans = Vec::new();
    let mut x = r.x;
    for tab in row {
        let width = u16::try_from(tab.width()).unwrap_or(u16::MAX);
        if let Some(hit) = tab.hit {
            app.hits.push((
                Rect {
                    x,
                    width,
                    height: 1,
                    ..r
                },
                hit,
            ));
        }
        x = x.saturating_add(width);
        spans.extend(tab.spans);
    }
    spans
}

/// One PROJECT TAB, as two hits side by side: the tab — its name and its
/// STATUS DOTS — and the `×` that closes it, a target of its own so a
/// click on the cross never reads as a click on the tab. The lit tab is a
/// raised chip, pads and all, with its name in the accent; the rest sit
/// flat and muted. The name is cut to `name_max`. A tab that cannot be
/// closed (`closable` false: the only one) ends on a plain pad instead of
/// the cross.
///
/// `sweep` is the frame's sweep phase, `None` with the animations off:
/// with it, the name sweeps on the loudest thing its sessions are doing
/// ([`tab_ramp`]) — lit or not, since the tab is what says so from
/// another project. The header's cursor holds still, so the block that
/// says where Enter goes stays legible.
fn project_chip(
    tab: &ProjectTab,
    name_max: usize,
    hover: Option<&HitTarget>,
    closable: bool,
    sweep: Option<usize>,
    th: Theme,
) -> [PaneTab; 2] {
    let fill = |style: Style| {
        if tab.active {
            style.bg(th.sel_bg)
        } else {
            style
        }
    };
    let mut name = if tab.active {
        Style::default().fg(th.accent).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(th.muted)
    };
    if hover == Some(&HitTarget::LauncherTab(tab.id.clone())) {
        name = name.add_modifier(Modifier::UNDERLINED);
    }
    // The header's own cursor: the pad and the name as one accent block,
    // so where Enter would go reads apart from which tab is lit.
    let (pad, name) = if tab.focused {
        let cursor = Style::default().bg(th.accent).fg(th.on_accent);
        (cursor, cursor.add_modifier(Modifier::BOLD))
    } else {
        (fill(Style::default()), fill(name))
    };
    let (ramp, phase) = match sweep {
        Some(phase) if !tab.focused => (tab_ramp(tab.tally, th), phase),
        _ => (None, 0),
    };
    let mut label = vec![Span::styled(" ", pad)];
    label.extend(status_name_spans(
        truncate(&tab.name, name_max),
        name,
        ramp,
        phase,
    ));
    label.extend(
        tab_dots(tab.tally, th)
            .into_iter()
            .map(|dot| Span::styled(dot.content, fill(dot.style))),
    );
    let cross = if hover == Some(&HitTarget::LauncherTabClose(tab.id.clone())) {
        th.err
    } else if tab.active {
        th.muted
    } else {
        th.dim
    };
    [
        PaneTab {
            spans: label,
            hit: Some(HitTarget::LauncherTab(tab.id.clone())),
        },
        if closable {
            PaneTab {
                spans: vec![Span::styled(" × ", fill(Style::default().fg(cross)))],
                hit: Some(HitTarget::LauncherTabClose(tab.id.clone())),
            }
        } else {
            PaneTab::plain(vec![Span::styled(" ", fill(Style::default()))])
        },
    ]
}

/// A PROJECT TAB's STATUS DOTS: one per state its sessions are in, each
/// carrying that state's count and no word at all, so the header is read
/// at a glance rather than parsed. Waiting on you leads (red), then
/// finished unread (blue), then working (yellow) — the three a row's own
/// STATUS DOT wears. A state with nothing in it is left out, so a quiet
/// project is its bare name — and because the order is fixed, the dots
/// that are there never move as the work under them does.
fn tab_dots(tally: Tally, th: Theme) -> Vec<Span<'static>> {
    [
        (tally.needs_you, th.err),
        (tally.done, th.done),
        (tally.running, th.warn),
    ]
    .into_iter()
    .filter(|(n, _)| *n > 0)
    .map(|(n, color)| Span::styled(format!(" ●{n}"), Style::default().fg(color)))
    .collect()
}

/// The ramp a PROJECT TAB's name sweeps on: red while any of its sessions
/// waits on you, whatever else is going on; yellow while any is mid-turn;
/// and blue once none is, for as long as a finish is left unread — the
/// project's work is done and you have not looked. A quiet project holds
/// still. Unlike a card's ONE-SHOT SWEEP the blue lasts, since the tab is
/// how a finish in a project you are not on gets noticed at all.
fn tab_ramp(tally: Tally, th: Theme) -> Option<[Color; 3]> {
    if tally.needs_you > 0 {
        Some(th.err_sweep)
    } else if tally.running > 0 {
        Some(th.warn_sweep)
    } else if tally.done > 0 {
        Some(th.done_sweep)
    } else {
        None
    }
}

/// The header's right side: how many cards the grid holds, and how many
/// of them are off screen. Which of them want something is the STATUS
/// DOTS' business on the PROJECT TABS, told in dots rather than in a
/// second sentence.
///
/// The HIDDEN MARKER holds the right edge whatever else has to go: a
/// screenful of cards with more behind it looks exactly like a project
/// with that many sessions in it, and the PANE dragged up over the grid
/// is the usual way of getting there — so the one thing on this row that
/// says cards are missing outranks the PR & ISSUE COUNTS beside it, which
/// `v` and `i` say again anyway.
///
/// Each span comes with the button it is, if any: only the two counts are,
/// and [`draw_head`] lays their hit rects where the row lands them.
fn head_count(
    app: &App,
    count: usize,
    hidden: Hidden,
    width: usize,
    th: Theme,
) -> Vec<(Span<'static>, Option<HitTarget>)> {
    // The ARCHIVED VIEW counts the same cards under their own word, so
    // the header says which of the two lists is on screen without a
    // second line to read.
    let noun = if app.show_archived {
        "archived session"
    } else {
        "session"
    };
    let mut spans = vec![(
        Span::styled(
            format!("{count} {noun}{}", plural(count)),
            Style::default().fg(th.dim),
        ),
        None,
    )];
    // The PR & ISSUE COUNTS the PROJECTS PANEL's rows used to carry: how
    // many open pull requests and issues the project in front of you has,
    // so the number is read without opening `v` or `i` to find it — and a
    // click on either count opens that list, the pointer's way to the
    // same modal.
    let mark = hidden_mark(hidden, th);
    let mark_w: usize = mark.iter().map(|s| s.width()).sum();
    if let Some(project) = app.selected_project() {
        let counts = app.project_open_counts(&project.id);
        if let Some(badge) = crate::ui::open_counts_badge(counts, th) {
            let used: usize = spans.iter().map(|(s, _)| s.width()).sum();
            if used + badge.1 + mark_w <= width {
                spans.extend(badge.0.into_iter().map(|(text, mut style, hit)| {
                    // Nothing about a word says it is a button, so the one
                    // under the pointer is underlined, as the header's
                    // tabs are.
                    if hit.is_some() && app.hover_crumb == hit {
                        style = style.add_modifier(Modifier::UNDERLINED);
                    }
                    (Span::styled(text, style), hit)
                }));
            }
        }
    }
    spans.extend(mark.into_iter().map(|s| (s, None)));
    spans
}

/// The HIDDEN MARKER: how many cards the grid holds that are not on
/// screen, with an arrow saying which way they went — `↓` past the bottom
/// edge, `↑` scrolled off the top, `↑↓` both. Nothing at all when the
/// window holds every card, so a grid that fits reads exactly as it does
/// today.
///
/// This is the row's answer to a PANE dragged up over the cards: the
/// count beside it still says how many sessions the project has, and this
/// says how many of them the room left can show — so a card that went
/// missing reads as the pane taking its room rather than as the session
/// disappearing, and the arrow says whether the way back to it is a step
/// down through the grid or the pane's own edge dragged back.
fn hidden_mark(hidden: Hidden, th: Theme) -> Vec<Span<'static>> {
    let total = hidden.total();
    if total == 0 {
        return Vec::new();
    }
    let arrow = match (hidden.above > 0, hidden.below > 0) {
        (true, true) => "↑↓",
        (true, false) => "↑",
        _ => "↓",
    };
    vec![Span::styled(
        format!("  {arrow} {total} hidden"),
        Style::default().fg(th.muted),
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
    // The keys are in the pane, or up on the PROJECT TABS: the card under
    // the cursor keeps it, unfocused.
    let focused = app.focus != Focus::Terminal && app.launcher_tab_cursor.is_none();
    let cursor = crate::launcher::cursor(app, rows);
    // Nothing selected (`App::launcher_unaimed`): no card wears the
    // cursor. The window it scrolled to stays where it is — letting the
    // aim go is not a scroll.
    let on = wearing(app, cursor);
    // Scrolling is by row: the window starts on the first card of
    // whichever row keeps the cursor's on screen.
    let (start, shown) = g.window(cursor, rows.len());
    // One CONFIG.JSON read for the whole frame, and only if some card on
    // it runs a CUSTOM harness whose label lives in there — a screenful
    // of cards must not reload the file once per card.
    let mut cfg = None;
    for (slot, (index, row)) in rows.iter().enumerate().skip(start).enumerate().take(shown) {
        let Some(cell) = fitting_cell(&g, slot) else {
            break;
        };
        draw_card(f, app, cell, row, on == Some(index), focused, th, &mut cfg);
        app.hits.push((cell, HitTarget::LauncherRow(index)));
    }
    draw_more_below(f, &g, shown, rows.len().saturating_sub(start + shown), th);
    // Last, so the cards themselves win `hit_at`'s first-match scan and
    // only the air between them falls through to the grid.
    app.hits.push((g.area, HitTarget::PanelBg(Focus::Sessions)));
}

/// A card's frame color when its status wants one: red for a turn waiting
/// on you, blue for one finished and unread, a faint yellow (`warn_edge`)
/// for one still running — the three the PROJECT TABS count, and no other.
/// A read finish, a fresh or terminated session and a `quiet` card (cold,
/// pending or archived: nothing on it is live) keep the plain edge. The
/// focused card's accent outranks all three, which is why running is the
/// faint one: in a warm preset a full-strength yellow frame sits a shade
/// off the focus.
fn card_edge(a: &nebula_core::Agent, quiet: bool, th: Theme) -> Option<Color> {
    use nebula_core::AgentStatus;
    if quiet {
        return None;
    }
    match a.status {
        AgentStatus::NeedsFeedback => Some(th.err),
        AgentStatus::Finished if a.unseen => Some(th.done),
        AgentStatus::Running => Some(th.warn_edge),
        _ => None,
    }
}

/// What an ARCHIVED card wears where a live one wears its STATUS DOT: the
/// round dot squared off. Two columns wide like the dot it stands in for,
/// so the name behind it starts in the same column on both grids.
const ARCHIVED_MARK: &str = "▪ ";

/// One session's card: its name and how long since it last moved, where
/// it runs — with that checkout's uncommitted file count — and with what,
/// its pull request, and the last thing it was
/// asked to do. The cursor's card takes the selection fill and an accent
/// border; every other card's frame answers to its status ([`card_edge`]).
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
    // An ARCHIVED card is the same card put away, and it is drawn as such:
    // nothing on it is live, so nothing on it is colored. Every part of it
    // takes `quiet` — dim on its own, lifted to muted under the selection
    // fill the way `render_row` lifts a dim row, so the card the cursor is
    // on stays legible — its name the plain `muted`, its STATUS DOT gives
    // way to `ARCHIVED_MARK`, and its frame squares off (the `block`
    // below). The colors say it at a glance and the two shapes say it
    // again with the colors off, which is the whole grid's answer to
    // "which of the two lists am I looking at" without a word repeated on
    // every card - the header's `n archived sessions` says that once.
    let archived = a.archived;
    let quiet = if selected { th.muted } else { th.dim };
    let quiet_or = |live: Color| if archived { quiet } else { live };
    // The dot and the sweep read the status the panels' session rows read,
    // cold and pending alike (see `draw_session_row`).
    let dot = if archived {
        Span::styled(ARCHIVED_MARK, Style::default().fg(quiet))
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
    let border = if selected && focused {
        th.accent
    } else if let Some(edge) = card_edge(a, archived || pending || cold, th) {
        edge
    } else if selected {
        th.muted
    } else {
        th.edge
    };
    let block = Block::default()
        .borders(Borders::ALL)
        // Square corners on an archived card, round on a live one: the one
        // difference between the two grids that survives a terminal with
        // no color at all.
        .border_type(if archived {
            BorderType::Plain
        } else {
            BorderType::Rounded
        })
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

    // How long ago it was filed, on an archived card, rather than when its
    // turn last moved: the status behind it stopped being news the moment
    // it was put away. A row archived before the stamp existed carries 0
    // and simply has no badge.
    let ago = if archived {
        ago_badge(a.archived_at)
    } else if pending {
        PENDING_SESSION_BADGE.to_string()
    } else if a.unseen {
        " done".to_string()
    } else {
        ago_badge(a.status_changed_at)
    };
    let (ago, name_max) = fit_ago(ago, width);
    let ramp = if pending || cold || archived {
        None
    } else {
        sweep_ramp(Some(a.status), app.agent_fresh_done(a), th, app.animations)
    };
    let name = truncate(&a.name, name_max.saturating_sub(2));
    let mut first = vec![dot];
    let used = name.chars().count() + 2;
    // An archived name is not the loud thing on the screen any more: it
    // gives up the bold with the rest of the card's weight and sits one
    // step above the quiet the rest of the card is in — muted over dim,
    // and text over muted on the card the cursor is on, which is the same
    // step drawn on the brighter fill.
    let name_style = if archived {
        Style::default().fg(if selected { th.text } else { th.muted })
    } else {
        Style::default().fg(th.text).add_modifier(Modifier::BOLD)
    };
    first.extend(status_name_spans(name, name_style, ramp, app.sweep_phase()));
    if !ago.is_empty() {
        let ago = ago.trim_start().to_string();
        let pad = width.saturating_sub(used + ago.chars().count());
        first.push(Span::raw(" ".repeat(pad)));
        first.push(Span::styled(
            ago,
            if archived {
                Style::default().fg(quiet)
            } else if a.unseen && !pending {
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
    //
    // An ARCHIVED card keeps the glyph and drops the color: `root` is a
    // warning about what a session is editing right now, and an archived
    // session is editing nothing. The glyph alone still tells the two
    // checkouts apart.
    let glyph = if row.is_main { "⌂ " } else { "↳ " };
    let scope = quiet_or(if row.is_main { th.root } else { th.worktree });
    let harness = harness_line(a, cfg);
    let room = width
        .saturating_sub(harness.chars().count() + 3)
        .saturating_sub(glyph.chars().count());
    // The checkout's uncommitted changes ride right behind its branch, in
    // the heads-up color — `↳ feat +3 files` — so the count reads as that
    // branch's, on every card in it, not only the one under the cursor
    // (`App::worktree_changes`, swept over every checkout the grid lists).
    // A clean checkout says nothing. The word goes before the branch
    // gives up a letter for it: `+3` on a card too narrow for both.
    //
    // CARD LINE COUNTS follows the count with the lines behind it, in the
    // DIFF VIEWER's own green and red — `+3 files +120 -45` — and they
    // yield after the word and before the branch (`change_labels`).
    let changes = app
        .worktree_changes(&a.worktree_id)
        .filter(|n| *n > 0)
        .map(|n| {
            change_labels(
                n,
                app.worktree_lines(&a.worktree_id),
                row.branch.chars().count(),
                room,
            )
        });
    let taken = changes.as_ref().map_or(0, |(files, lines)| {
        files.chars().count()
            + lines.as_ref().map_or(0, |(added, removed)| {
                added.chars().count() + removed.chars().count()
            })
    });
    let branch = truncate(&row.branch, room.saturating_sub(taken));
    let mut second = vec![
        Span::styled(glyph, Style::default().fg(scope)),
        Span::styled(branch, Style::default().fg(scope)),
    ];
    if let Some((files, lines)) = changes {
        second.push(Span::styled(files, Style::default().fg(quiet_or(th.warn))));
        if let Some((added, removed)) = lines {
            second.push(Span::styled(added, Style::default().fg(quiet_or(th.ok))));
            second.push(Span::styled(removed, Style::default().fg(quiet_or(th.err))));
        }
    }
    if !harness.is_empty() {
        second.push(Span::styled(" · ", Style::default().fg(quiet_or(th.dim))));
        second.push(Span::styled(harness, Style::default().fg(quiet_or(th.dim))));
    }

    // Its pull request, in the PR rows' own colors; a session with none
    // leaves the row blank rather than saying so four times over a screen
    // of cards.
    let third = match &row.pr {
        Some(pr) => {
            // On an archived card the PR is history too: a merged purple
            // or a red conflict there would be the loudest thing on a grid
            // of filed-away work, and neither is a job any more.
            let look = if archived {
                crate::pr_row::Look {
                    glyph: quiet,
                    label: quiet,
                    rail: quiet,
                    badge: quiet,
                }
            } else {
                crate::pr_row::look(pr.standing, pr.trouble, th)
            };
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
        quiet_or(th.dim),
        quiet_or(th.muted),
    ));

    for (i, spans) in lines.into_iter().enumerate() {
        if spans.is_empty() {
            continue;
        }
        let Some(r) = row_rect(inner, i) else { break };
        f.render_widget(Paragraph::new(Line::from(spans)), r);
    }
}

/// Which card wears the cursor: the one the cursor is on, or none at all
/// once the aim has been let go of (`event_loop::launcher::clear_aim` —
/// a click on the air between the cards, or the first Esc). The grid
/// still knows where the cursor is; it simply draws no card as selected,
/// which is what says the box `p` opens will ask where its session lands.
fn wearing(app: &App, cursor: Option<usize>) -> Option<usize> {
    (!app.launcher_unaimed).then_some(cursor).flatten()
}

/// The MORE-BELOW CUE: the air under the last whole row of cards — rows
/// too few for another card — says how many cards the window left past
/// its bottom edge, so a screenful that happens to end on a full row
/// does not read as the whole project, and the arrow says the way to
/// them is down. Centered in that air, in the header's HIDDEN MARKER
/// color. Nothing once every card from the window down is on screen, and
/// nothing when the cards fill the grid to its last row: the header's
/// marker still counts them then.
fn draw_more_below(
    f: &mut Frame,
    g: &crate::launcher::Grid,
    shown: usize,
    below: usize,
    th: Theme,
) {
    if below == 0 || shown == 0 {
        return;
    }
    let last = g.cell(shown - 1);
    let top = last.y + last.height;
    let bottom = g.area.y + g.area.height;
    if top >= bottom {
        return;
    }
    // Centered under the cards themselves, not the area: the columns'
    // even split leaves a sliver of the area spare on the right.
    let edge = g.cell(g.cols.min(shown) - 1);
    let r = Rect {
        y: top + (bottom - top) / 2,
        width: edge.x + edge.width - g.area.x,
        height: 1,
        ..g.area
    };
    f.render_widget(
        Paragraph::new(Span::styled(
            format!("↓ {below} more session{} below", plural(below)),
            Style::default().fg(th.muted),
        ))
        .alignment(ratatui::layout::Alignment::Center),
        r,
    );
}

/// Where the card in `slot` goes, or None once the grid has run out of
/// rows for it — a card is drawn whole or not at all.
fn fitting_cell(g: &crate::launcher::Grid, slot: usize) -> Option<Rect> {
    let cell = g.cell(slot);
    (cell.y + cell.height <= g.area.y + g.area.height).then_some(cell)
}

/// What the ARCHIVED VIEW shows with no cards at all — one line where the
/// grid would be, rather than the live list's hero, which is about
/// starting a session and has nothing to say here.
fn draw_list_empty(f: &mut Frame, app: &mut App, area: Rect, what: &str) {
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
fn prompt_lines(prompt: &str, width: usize, mark: Color, text: Color) -> Vec<Vec<Span<'static>>> {
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
            let line_text = if last && cut {
                truncate(&format!("{line}…"), body)
            } else {
                line
            };
            vec![
                if i == 0 {
                    Span::styled(MARK, Style::default().fg(mark))
                } else {
                    Span::raw(" ".repeat(indent))
                },
                Span::styled(line_text, Style::default().fg(text)),
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
/// What a card's branch row says about its checkout's uncommitted changes,
/// beside a branch `branch` characters long in `room`: the file count —
/// ` +3 files`, or ` +3` — and, with `lines` counted, the lines behind it,
/// ` +120` and ` -45`. The word yields first, then the lines, and only then
/// does the branch give up a letter.
fn change_labels(
    files: usize,
    lines: Option<crate::git_diff::LineChanges>,
    branch: usize,
    room: usize,
) -> (String, Option<(String, String)>) {
    let long = format!(" +{files} file{}", if files == 1 { "" } else { "s" });
    let short = format!(" +{files}");
    let width = |s: &str| s.chars().count();
    if let Some(l) = lines {
        let (added, removed) = (format!(" +{}", l.added), format!(" -{}", l.removed));
        let tail = width(&added) + width(&removed);
        for count in [&long, &short] {
            if branch + width(count) + tail <= room {
                return (count.clone(), Some((added, removed)));
            }
        }
    }
    let count = if branch + width(&long) <= room {
        long
    } else {
        short
    };
    (count, None)
}

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

/// Smallest GRID the welcome turns a nebula in: under it the sky would be
/// a few smudges of dust, so the words stand alone, centered.
const SKY_MIN_W: u16 = 30;
const SKY_MIN_H: u16 = 10;

/// The grid with nothing in it: the splash's animated nebula across the
/// space the cards would take, and one welcome under its core — the name,
/// and the one key that starts a session, as a key cap a click presses
/// too ([`HitTarget::LauncherWelcomePrompt`], through the same
/// `event_loop::launcher::open_box` the key runs). The key is whatever
/// the keymap binds, so a rebound prompt is never advertised as `p`.
fn draw_empty(f: &mut Frame, app: &mut App, area: Rect) {
    // Under the box (or any modal) the welcome would only peek out around
    // its edges in fragments; the box is saying the same thing.
    if app.overlay.is_some() {
        app.hits.push((area, HitTarget::PanelBg(Focus::Sessions)));
        return;
    }
    let th = app.theme;
    let t = crate::splash::scene_time(app, app.splash_epoch);
    let mut welcome = vec![Span::styled("Welcome to ", Style::default().fg(th.text))];
    welcome.extend(crate::splash::wordmark_word("nebula", t));
    // The key line is a button, and marked as one under the pointer the
    // way the header's are.
    let mut words = Style::default().fg(th.muted);
    if app.hover_crumb == Some(HitTarget::LauncherWelcomePrompt) {
        words = words.add_modifier(Modifier::UNDERLINED);
    }
    let key = super::key_hint(app, crate::keymap::Action::QuickPrompt);
    let prompt = Line::from(vec![
        Span::styled("press ", words),
        Span::styled(
            format!(" {key} "),
            Style::default()
                .fg(th.on_accent)
                .bg(th.accent)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(" to prompt", words),
    ]);
    let prompt_w = (prompt.width() as u16).min(area.width);
    let lines = vec![Line::from(welcome), Line::from(""), prompt];
    let w = (lines.iter().map(Line::width).max().unwrap_or(0) as u16).min(area.width);
    let h = (lines.len() as u16).min(area.height);
    // The words sit a little under the middle, leaving the galaxy the sky
    // above them to turn in; with no room for one they are just centered.
    let sky = area.width >= SKY_MIN_W && area.height >= SKY_MIN_H;
    let y = if sky {
        area.y + (area.height * 3 / 5).saturating_sub(h / 2)
    } else {
        area.y + area.height.saturating_sub(h) / 2
    };
    let text = Rect {
        x: area.x + (area.width - w) / 2,
        y,
        width: w,
        height: h,
    };
    if sky {
        // A wider berth than the splash's: here the arms curl down past
        // the words, where the splash's run out above them.
        let clear = Rect {
            x: text.x.saturating_sub(4),
            width: text.width + 8,
            ..text
        }
        .intersection(area);
        crate::splash::draw_sky(f.buffer_mut(), area, clear, t, th.accent);
    }
    // Even with no sky, the name's shine moves.
    app.welcome_on_screen = true;
    f.render_widget(Paragraph::new(lines).centered(), text);
    // Ahead of the background, so it wins `hit_at`'s first-match scan.
    let key_row = Rect {
        x: area.x + (area.width - prompt_w) / 2,
        y: text.y + 2,
        width: prompt_w,
        height: 1,
    }
    .intersection(area);
    app.hits.push((key_row, HitTarget::LauncherWelcomePrompt));
    app.hits.push((area, HitTarget::PanelBg(Focus::Sessions)));
}

/// One tab in the PANE's TAB STRIP, or in the header's PROJECT TABS: what
/// it draws, and what a click on it means. The gaps and the divider
/// between tabs are tabs with no hit of their own, so one walk both lays
/// the strip out and hit-tests it.
struct PaneTab {
    spans: Vec<Span<'static>>,
    hit: Option<HitTarget>,
}

impl PaneTab {
    fn plain(spans: Vec<Span<'static>>) -> Self {
        Self { spans, hit: None }
    }

    fn width(&self) -> usize {
        self.spans.iter().map(|s| s.content.chars().count()).sum()
    }
}

/// Longest a TERMINAL's name is drawn on the strip before it is clipped:
/// enough for the names nebula gives them (`shell-1`) and for a renamed
/// one to still be recognisable, short enough that one long name cannot
/// push every other tab off the row.
const TAB_MAX: usize = 18;
/// The button that closes a TERMINAL from its own tab — one click, then
/// the same confirm the SESSIONS PANEL's `d` puts up on a terminal row.
const CLOSE: &str = " ×";
/// Room the strip keeps back for the `+N` that counts the terminals past
/// the right edge, so the last tab that fits is never the one that
/// leaves no room to say there are more.
const MORE_ROOM: usize = 5;
/// The PANE's CLOSE BUTTON, at the right end of its TAB STRIP: one click
/// folds the pane away, as `^~` does. A cell of air either side, so the
/// target is wider than the glyph.
const PANE_CLOSE: &str = " × ";

/// The PANE's own header in the LAUNCHER VIEW: the TAB STRIP that says
/// what the pane is reading and is how it gets swapped — the `SESSION`
/// tab, the checkout the strip is scoped to, a divider, then one tab per
/// TERMINAL open in that checkout. A rule under it, and the rect the PTY
/// draws in returned, exactly as `ui::terminal_frame` does for the
/// panels' pane.
///
/// The checkout is named up here because the terminals beside it are its
/// and nobody else's (`App::visible_terminals`): walking the cursor onto
/// a card in another one swaps the whole strip, and without the branch
/// next to them that reads as terminals coming and going at random.
///
/// INPUT PARITY: a click on a tab and the `` ` `` key walk this same
/// strip through the one `event_loop::launcher::show_pane_tab`.
pub(super) fn pane_frame(
    f: &mut Frame,
    app: &mut App,
    area: Rect,
    right: Option<Span<'static>>,
    focused: bool,
) -> Rect {
    let th = app.theme;
    if let Some(r) = row_rect(area, 1) {
        let r = pad_x(r);
        // The CLOSE BUTTON holds the right end of the row and the state
        // tag (`scroll 4`, exited) is right-aligned just before it: the
        // strip gets what those two leave.
        let close_w = PANE_CLOSE.chars().count() as u16;
        let taken = usize::from(close_w)
            + right
                .as_ref()
                .map_or(0, |tag| tag.content.chars().count() + 2);
        let tabs = pane_tabs(app, usize::from(r.width).saturating_sub(taken));
        let mut spans = Vec::new();
        let mut x = r.x;
        for tab in tabs {
            let width = u16::try_from(tab.width()).unwrap_or(u16::MAX);
            if let Some(hit) = tab.hit {
                app.hits.push((
                    Rect {
                        x,
                        y: r.y,
                        width,
                        height: 1,
                    },
                    hit,
                ));
            }
            x = x.saturating_add(width);
            spans.extend(tab.spans);
        }
        f.render_widget(Paragraph::new(Line::from(spans)), r);
        let tag_r = Rect {
            width: r.width.saturating_sub(close_w),
            ..r
        };
        if let Some(tag) = right {
            f.render_widget(
                Paragraph::new(Line::from(vec![tag, Span::raw(" ")]))
                    .alignment(ratatui::layout::Alignment::Right),
                tag_r,
            );
        }
        if r.width >= close_w {
            let close = Rect {
                x: tag_r.x + tag_r.width,
                width: close_w,
                ..r
            };
            // Marked under the pointer the way the PROJECT TABS' `×` is:
            // nothing about a cross says it is a button until then.
            let fg = if app.hover_crumb == Some(HitTarget::LauncherPaneClose) {
                th.err
            } else {
                th.muted
            };
            f.render_widget(
                Paragraph::new(Span::styled(PANE_CLOSE, Style::default().fg(fg))),
                close,
            );
            app.hits.push((close, HitTarget::LauncherPaneClose));
        }
    }
    draw_rule(f, area, 2, if focused { th.accent } else { th.edge });
    Rect {
        y: area.y + 3,
        height: area.height.saturating_sub(3),
        ..area
    }
}

/// The strip itself, laid out left to right in `room` columns. The
/// SESSION tab and the checkout never give way — they are what the rest
/// of the strip is scoped by — and a terminal that will not fit is
/// counted in the `+N` at the end rather than drawn half.
fn pane_tabs(app: &App, room: usize) -> Vec<PaneTab> {
    let th = app.theme;
    // Active tab in the accent, the rest muted — and a terminal whose PTY
    // the daemon no longer holds dimmer still, the way the SESSIONS
    // PANEL dims an exited shell's row.
    let look = |active: bool, alive: bool| {
        if active {
            Style::default().fg(th.accent).add_modifier(Modifier::BOLD)
        } else if alive {
            Style::default().fg(th.muted)
        } else {
            Style::default().fg(th.dim)
        }
    };
    let pinned = app.pinned_terminal();

    // The SESSION tab names the card under the cursor, not whatever is
    // attached: with the pane on a terminal the attachment is that
    // terminal, and the tab has to go on saying what SESSION would come
    // back to.
    let rows = crate::launcher::rows(app);
    let card = crate::launcher::cursor(app, &rows).and_then(|at| rows.get(at));
    let mut label = vec![Span::styled("SESSION", look(pinned.is_none(), true))];
    if let Some(row) = card {
        label.push(Span::styled(" · ", Style::default().fg(th.dim)));
        label.push(Span::styled(
            truncate(&row.agent.name, (room / 4).max(8)),
            look(pinned.is_none(), true),
        ));
    }
    let mut tabs = vec![PaneTab {
        spans: label,
        hit: Some(HitTarget::LauncherPaneSession),
    }];

    // The checkout everything after it belongs to, in the SCOPE COLOR the
    // cards paint theirs in — `⌂` on the project's root branch, `↳` on a
    // checkout of its own — so the header and the card under the cursor
    // say the same thing the same way. This is what makes the terminals
    // beside it read as that checkout's rather than as tabs coming and
    // going at random as the grid is walked.
    let checkout = card
        .map(|row| (row.is_main, row.branch.clone()))
        .or_else(|| {
            app.selected_worktree()
                .map(|w| (w.is_main, w.branch.clone()))
        });
    if let Some((is_main, branch)) = checkout {
        let (glyph, scope) = if is_main {
            ("⌂ ", th.root)
        } else {
            ("↳ ", th.worktree)
        };
        tabs.push(PaneTab::plain(vec![
            Span::raw("  "),
            Span::styled(glyph, Style::default().fg(scope)),
            Span::styled(
                truncate(&branch, (room / 4).max(8)),
                Style::default().fg(scope),
            ),
        ]));
    }
    tabs.push(PaneTab::plain(vec![Span::styled(
        "  │  ",
        Style::default().fg(th.edge),
    )]));

    let terminals = app.pane_terminals();
    if terminals.is_empty() {
        // Nothing to tab to yet: the strip says which key opens one
        // rather than trailing off after the divider.
        tabs.push(PaneTab::plain(vec![Span::styled(
            format!(
                "{} opens a terminal here",
                super::key_hint(app, crate::keymap::Action::NewTerminal)
            ),
            Style::default().fg(th.dim),
        )]));
        return tabs;
    }
    // The tabs that fit, from `start`, within `budget` columns: the tabs
    // themselves and the index one past the last one drawn.
    let fit = |start: usize, budget: usize| -> (Vec<PaneTab>, usize) {
        let mut out: Vec<PaneTab> = Vec::new();
        let mut left = budget;
        let mut end = start;
        for (i, t) in terminals.iter().enumerate().skip(start) {
            let gap = if i == start { 0 } else { 2 };
            // The glyph the SESSIONS PANEL gives the same row: `▶` for a
            // RUN TERMINAL, `❯` for a plain shell, in `th.ok` while its
            // PTY is alive.
            let glyph = if t.run_command.is_some() {
                "▶ "
            } else {
                "❯ "
            };
            let name = truncate(&t.name, TAB_MAX);
            let need = gap + glyph.chars().count() + name.chars().count() + CLOSE.chars().count();
            let spare = if i + 1 == terminals.len() {
                0
            } else {
                MORE_ROOM
            };
            if need + spare > left {
                break;
            }
            left -= need;
            if gap > 0 {
                out.push(PaneTab::plain(vec![Span::raw("  ")]));
            }
            let active = pinned.as_ref() == Some(&t.id);
            out.push(PaneTab {
                spans: vec![
                    Span::styled(
                        glyph,
                        Style::default().fg(if t.alive { th.ok } else { th.dim }),
                    ),
                    Span::styled(name, look(active, t.alive)),
                ],
                hit: Some(HitTarget::LauncherPaneTerminal(i)),
            });
            // The `×` is a tab of its own, not part of the name's: two
            // hits that cannot overlap, so a click on the cross never
            // reads as a click on the tab it closes. Quiet on the tabs
            // the pane is not reading — it is an affordance, not a
            // warning — and one notch brighter on the one it is, which
            // is the tab `d` closes.
            out.push(PaneTab {
                spans: vec![Span::styled(
                    CLOSE,
                    Style::default().fg(if active { th.muted } else { th.dim }),
                )],
                hit: Some(HitTarget::LauncherPaneCloseTerminal(i)),
            });
            end = i + 1;
        }
        (out, end)
    };

    let fixed: usize = tabs.iter().map(PaneTab::width).sum();
    let budget = room.saturating_sub(fixed);
    // Lay the tabs out from the first terminal; if that leaves the tab the
    // pane is READING past the right edge, lay them out again starting at
    // it. The same rule the GRID's scroll follows — the window is whatever
    // keeps what is selected on screen — and without it `` ` `` could walk
    // onto a tab nothing on the strip shows as active.
    let at = pinned
        .as_ref()
        .and_then(|id| terminals.iter().position(|t| &t.id == id));
    let (mut window, mut end) = fit(0, budget);
    let mut start = 0;
    if at.is_some_and(|at| at >= end) {
        start = at.unwrap_or(0);
        (window, end) = fit(start, budget.saturating_sub(MORE_ROOM));
    }
    // What the window left off either side is counted, never clipped.
    if start > 0 {
        tabs.push(PaneTab::plain(vec![Span::styled(
            format!("+{start}  "),
            Style::default().fg(th.dim),
        )]));
    }
    tabs.extend(window);
    if end < terminals.len() {
        tabs.push(PaneTab::plain(vec![Span::styled(
            format!("  +{}", terminals.len() - end),
            Style::default().fg(th.dim),
        )]));
    }
    tabs
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
        // The hatch out of a full-screen session is a button too, and
        // wears the same underline while the pointer is on it.
        let mut style = Style::default().fg(th.accent).add_modifier(Modifier::BOLD);
        if app.hover_crumb.as_ref() == Some(&HitTarget::LauncherCrumb) {
            style = style.add_modifier(Modifier::UNDERLINED);
        }
        let mut spans = vec![Span::styled(back.clone(), style)];
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

/// The view's box, row 0: where the session runs — the project and the
/// checkout in it — what runs there and on which model, each named with
/// the chord that changes it, set far enough apart that no two read as
/// one phrase. The checkout wears a `▾` where the others wear a chord: a
/// click on it drops the WORKTREE PICKER down from it.
///
/// Widest form that fits, in order: labelled and airy; labelled and
/// tight; the values and their chords alone; then the same without the
/// model, without the agent, and without the worktree. The airy form is
/// drawn whole or not at all — tighter gaps beat a cut name. The project
/// is never dropped, only cut — where a session lands is the one thing
/// worth a whole row on its own — and a long branch is cut before it is.
///
/// A fresh worktree's branch — the one Enter will cut — is drawn in the
/// green the box's frame turns. A box aimed away from the grid behind it
/// fires a BACKGROUND LAUNCH: the project is the half that changed, and
/// nothing on screen will move when Enter lands, so the project is lit.
/// A PR SESSION's head branch wears no `▾` and is no button: its checkout
/// is the DAEMON's to pick.
///
/// Returns the row, each field's own columns within it — the label, the
/// value and the chord together, never the air between two fields — so a
/// click there can be given the same picker the chord opens
/// ([`crate::app::PromptDialog::detail_areas`]), and the branch's own, for
/// the picker to hang from ([`crate::app::PromptDialog::branch_area`]). A
/// tier that drops a field hands back no columns for it: what is not drawn
/// is not a button.
pub(super) fn detail_line(app: &App, launch: &QuickLaunch, width: u16, th: Theme) -> DetailLine {
    let details = Details::of(app, launch);
    let width = usize::from(width);
    for (labels, gap, fields, cuts) in [
        (true, DETAIL_GAP, 4, false),
        (true, DETAIL_TIGHT, 4, true),
        (false, DETAIL_TIGHT, 4, true),
        (false, DETAIL_TIGHT, 3, true),
        (false, DETAIL_TIGHT, 2, true),
        (false, DETAIL_TIGHT, 1, true),
    ] {
        let drawn = details.spans(fields, labels, gap, th);
        let w = drawn.line.width();
        if w <= width {
            return drawn;
        }
        // This tier with the branch, then the project, cut to what is left
        // over — but not past the point where either stops saying which
        // one it is.
        if let Some(cut) = details.cut(w - width, fields).filter(|_| cuts) {
            return cut.spans(fields, labels, gap, th);
        }
    }
    // Narrower than the project and its chord together: cut it to the row.
    let cut = Details {
        project: truncate(&details.project, width.saturating_sub(3)),
        ..details
    };
    cut.spans(1, false, DETAIL_TIGHT, th)
}

/// [`detail_line`]'s answer: the row, each drawn field's `(field, first
/// column, width)` within it, and the branch's `(first column, width)` —
/// the value and its `▾`, None when it was dropped for room or names a
/// checkout nobody picks.
pub(super) struct DetailLine {
    pub line: Line<'static>,
    pub fields: Vec<(BoxField, u16, u16)>,
    pub branch: Option<(u16, u16)>,
}

/// Shortest a project name is cut to before [`detail_line`] gives up a
/// whole field instead.
const MIN_PROJECT: usize = 8;

/// Shortest a branch is cut to before [`detail_line`] starts on the
/// project.
const MIN_BRANCH: usize = 12;

/// The gap between two of [`detail_line`]'s fields — wide enough that
/// `api-server ^P` and `agent claude` never read as one phrase — and the
/// one a box too narrow for that falls back to.
const DETAIL_GAP: &str = "   ·   ";
const DETAIL_TIGHT: &str = " · ";

/// What [`detail_line`] names, before a tier decides how much of it fits.
#[derive(Clone)]
struct Details {
    project: String,
    branch: String,
    harness: String,
    model: String,
    /// The branch is the WORKTREE PICKER's button — anything but a PR
    /// SESSION's head.
    pickable: bool,
    /// Enter cuts the branch as a fresh worktree first.
    fresh: bool,
    /// The box is aimed away from the grid behind it.
    background: bool,
}

impl Details {
    fn of(app: &App, launch: &QuickLaunch) -> Self {
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
        let branch = match &launch.pr {
            Some(pr) => pr.head.clone(),
            None => crate::quick_prompt::target_branch(app, launch)
                .unwrap_or_else(|| "(worktree gone)".into()),
        };
        Self {
            project: launch_project(app, launch),
            branch,
            harness,
            model,
            pickable: launch.pr.is_none(),
            fresh: launch.is_new_worktree(),
            background: crate::launcher::project_of(app, &launch.target)
                .is_some_and(|project| crate::launcher::is_background(app, &project)),
        }
    }

    /// These details `over` columns narrower: taken from the branch while
    /// the tier draws it, down to [`MIN_BRANCH`], then from the project,
    /// down to [`MIN_PROJECT`]. None when that is not enough.
    fn cut(&self, over: usize, fields: usize) -> Option<Self> {
        let branch = self.branch.chars().count();
        let from_branch = if fields >= 2 {
            branch.saturating_sub(MIN_BRANCH).min(over)
        } else {
            0
        };
        let from_project = over - from_branch;
        let project = self.project.chars().count();
        if from_project > 0 && project.saturating_sub(from_project) < MIN_PROJECT {
            return None;
        }
        Some(Self {
            project: truncate(&self.project, project - from_project),
            branch: truncate(&self.branch, branch - from_branch),
            ..self.clone()
        })
    }

    /// The row at one tier: `fields` of them, each a `value ^key` with the
    /// word for it ahead when `labels`, held apart by `gap`. Each field's
    /// columns are measured as its spans are pushed, so a name's own width
    /// is what the button is worth.
    fn spans(&self, fields: usize, labels: bool, gap: &str, th: Theme) -> DetailLine {
        let bold = |fg| Style::default().fg(fg).add_modifier(Modifier::BOLD);
        let mut spans = Vec::new();
        let mut hits = Vec::new();
        let mut branch = None;
        let mut x = 0usize;
        let push = |spans: &mut Vec<Span<'static>>, x: &mut usize, span: Span<'static>| {
            *x += span.width();
            spans.push(span);
        };
        let project_fg = if self.background { th.accent } else { th.text };
        let branch_fg = if self.fresh { th.ok } else { th.text };
        for (i, (field, label, value, fg, key)) in [
            (
                BoxField::Project,
                "project",
                &self.project,
                project_fg,
                Some("^P"),
            ),
            (
                BoxField::Worktree,
                "worktree",
                &self.branch,
                branch_fg,
                self.pickable.then_some("▾"),
            ),
            (
                BoxField::Agent,
                "agent",
                &self.harness,
                th.text,
                Some("Tab"),
            ),
            (BoxField::Model, "model", &self.model, th.text, Some("^O")),
        ]
        .into_iter()
        .take(fields)
        .enumerate()
        {
            if i > 0 {
                push(
                    &mut spans,
                    &mut x,
                    Span::styled(gap.to_string(), Style::default().fg(th.edge)),
                );
            }
            let start = x;
            if labels {
                push(
                    &mut spans,
                    &mut x,
                    Span::styled(format!("{label} "), Style::default().fg(th.dim)),
                );
            }
            let value_at = x;
            push(&mut spans, &mut x, Span::styled(value.clone(), bold(fg)));
            if let Some(key) = key {
                push(
                    &mut spans,
                    &mut x,
                    Span::styled(format!(" {key}"), Style::default().fg(th.accent)),
                );
            }
            if field == BoxField::Worktree {
                if !self.pickable {
                    continue;
                }
                branch = Some((value_at as u16, (x - value_at) as u16));
            }
            hits.push((field, start as u16, (x - start) as u16));
        }
        DetailLine {
            line: Line::from(spans),
            fields: hits,
            branch,
        }
    }
}

/// The PROJECT a launch is aimed at, by name.
fn launch_project(app: &App, launch: &QuickLaunch) -> String {
    crate::launcher::project_of(app, &launch.target)
        .and_then(|id| crate::launcher::project_name(app, &id))
        .unwrap_or_else(|| "(project gone)".into())
}

/// The view's box prompt header: what Enter sends on the left, and the
/// toggle that cuts a fresh worktree, right. A PR SESSION says what the
/// DAEMON will do with the head branch instead, its checkout not being
/// ours to flip. Where the launch lands — the project and the checkout in
/// it — is on the details row above ([`detail_line`]).
///
/// Returns the line and the toggle's columns within it, so a click there
/// can be given the same flip `^N` has
/// ([`crate::app::PromptDialog::toggle_area`]).
pub(super) fn target_line(launch: &QuickLaunch, label: &str, width: u16, th: Theme) -> TargetLine {
    let dim = Style::default().fg(th.dim);
    // Right: the toggle and its state, or — on a PR SESSION — what the
    // DAEMON will do with the head branch, there being nothing to flip.
    let (right, clickable) = if launch.pr.is_some() {
        (vec![Span::styled("reused or cut on Enter", dim)], false)
    } else if launch.is_new_worktree() {
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

    let width = usize::from(width);
    let label = truncate(label, width.saturating_sub(right_w + 1));
    let used = label.chars().count();
    let mut spans = vec![Span::styled(label, dim)];
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
    TargetLine {
        line: Line::from(spans),
        toggle,
    }
}

/// [`target_line`]'s answer: the row, and where its toggle landed in it —
/// `(first column, width)`, None when it was dropped for room or has
/// nothing to do.
pub(super) struct TargetLine {
    pub line: Line<'static>,
    pub toggle: Option<(u16, u16)>,
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
/// this box read as a wall of text. `⌘Enter` — launch and step down into
/// the new session — outlasts the newline and the preset, since nothing
/// else in the box says it exists.
pub(super) fn box_hint(width: u16) -> &'static str {
    if width >= 84 {
        " Enter launch · ⌘Enter launch + focus · ⇧Enter newline · ⇧Tab preset · Esc cancel "
    } else if width >= 67 {
        " Enter launch · ⌘Enter launch + focus · ⇧Tab preset · Esc cancel "
    } else if width >= 46 {
        " Enter launch · ⌘Enter + focus · Esc cancel "
    } else if width >= 29 {
        " ↵ launch · ⌘↵ focus · Esc "
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
/// to.
pub(super) fn picker_over_box(_app: &App, picker: &ProjectPicker) -> bool {
    picker.back.from_box
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
/// with the typed letters lit, the path dim after it. Drawn over the box it was opened from
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
        let after = format!("  {}", project.path);
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
        // And the box at its own size names every key it has.
        let full = box_hint(BOX_SIZE.0);
        for key in ["Enter launch", "⌘Enter launch + focus", "⇧Enter newline"] {
            assert!(full.contains(key), "{full:?} lost {key}");
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

    /// The details row names all four values, the three chords and the
    /// branch's `▾`, and fits the box it is drawn in.
    #[test]
    fn the_details_row_carries_every_chord() {
        let th = Theme::default();
        let app = App::new();
        let mut launch = a_launch();
        launch.model = None;
        launch.effort = None;
        let inner = BOX_SIZE.0 - 4;
        let line = detail_line(&app, &launch, inner, th).line;
        let text = text_of(&line);
        for want in [
            "project ",
            "^P",
            "worktree ",
            "▾",
            "agent ",
            "claude",
            "Tab",
            "model ",
            "default",
            "^O",
        ] {
            assert!(text.contains(want), "{text:?} is missing {want:?}");
        }
        assert!(line.width() <= inner as usize, "{text:?}");

        launch.model = Some("opus".into());
        launch.effort = Some("high".into());
        assert!(
            text_of(&detail_line(&app, &launch, inner, th).line).contains("opus high"),
            "the effort belongs on the model"
        );

        // One column short of the airy form: the gaps tighten and every
        // name stays whole, rather than one name losing its last letter.
        let airy = detail_line(&app, &launch, 400, th).line.width() as u16;
        let text = text_of(&detail_line(&app, &launch, airy - 1, th).line);
        assert!(!text.contains('…'), "{text:?}");
        assert!(text.contains(" · worktree "), "{text:?}");
    }

    /// Every field the row draws hands back the columns it was drawn in,
    /// and those columns hold exactly that field's own text — so a click
    /// on `agent claude Tab` cannot open the model list. A tier that drops
    /// a field hands back nothing for it: what is not drawn is no button.
    #[test]
    fn every_drawn_detail_hands_back_its_own_columns() {
        let th = Theme::default();
        let app = App::new();
        let mut launch = a_launch();
        launch.model = Some("opus".into());
        launch.effort = Some("high".into());
        for width in 10..=120u16 {
            let DetailLine {
                line, fields: hits, ..
            } = detail_line(&app, &launch, width, th);
            let text: Vec<char> = text_of(&line).chars().collect();
            assert!(
                hits.iter().any(|(f, _, _)| *f == BoxField::Project),
                "{width}: the project is never dropped"
            );
            for (field, x, w) in &hits {
                let (x, w) = (*x as usize, *w as usize);
                assert!(x + w <= text.len(), "{width}: {field:?} runs past the row");
                let cut: String = text[x..x + w].iter().collect();
                let chord = match field {
                    BoxField::Project => "^P",
                    BoxField::Worktree => "▾",
                    BoxField::Agent => "Tab",
                    BoxField::Model => "^O",
                };
                assert!(
                    cut.ends_with(chord),
                    "{width}: {field:?} is {cut:?}, which does not end in {chord}"
                );
                assert!(
                    !cut.starts_with(' ') && !cut.contains('·'),
                    "{width}: {field:?} is {cut:?}, which reaches into the air beside it"
                );
            }
        }
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
            let line = detail_line(&app, &launch, width, th).line;
            let text = text_of(&line);
            assert!(line.width() <= width as usize, "{width}: {text:?}");
            assert!(text.contains("^P"), "{width}: {text:?}");
        }
    }

    /// Two projects, one session in each: `api` with a `feat` checkout,
    /// `web` with one of its own. Both sessions start idle, so nothing
    /// sweeps until a test says it does.
    fn a_tree() -> App {
        use nebula_core::{
            Agent, AgentId, AgentKind, AgentStatus, Project, ProjectId, Worktree, WorktreeId,
        };
        let mut app = App::new();
        app.tree.projects = ["api", "web"]
            .iter()
            .enumerate()
            .map(|(i, name)| Project {
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

    /// `a_tree` with `count` sessions in `api` instead of one, so a grid
    /// can be given more cards than the body has room for.
    fn a_crowded_tree(count: usize) -> App {
        use nebula_core::{Agent, AgentId, AgentKind, AgentStatus, WorktreeId};
        let mut app = a_tree();
        let one = app.tree.agents[0].clone();
        app.tree.agents = (0..count)
            .map(|i| Agent {
                id: AgentId(format!("api{i}")),
                worktree_id: WorktreeId("w0".into()),
                name: format!("session {i}"),
                status: AgentStatus::Finished,
                kind: AgentKind::Claude,
                sort_order: i as i64,
                ..one.clone()
            })
            .collect();
        app
    }

    /// Aim the SESSIONS cursor at the session with this id, whichever
    /// row of the panels' list it happens to be on.
    fn aim_at(app: &mut App, id: &str) {
        app.sel_session = app
            .visible_session_rows()
            .iter()
            .position(|row| matches!(row, crate::app::SessionRow::Agent(a) if a.id.0 == id))
            .expect("a row for the session");
    }

    /// The whole view, drawn: a body too short for every card says so in
    /// its header, and says it about exactly the cards the grid left off.
    /// This is what a PANE dragged up over the grid looks like — the
    /// cards that lost their room are counted rather than simply gone.
    #[test]
    fn a_body_too_short_for_its_cards_counts_them_in_the_header() {
        use crate::launcher::{CARD_H, HEAD_H};
        let mut app = a_crowded_tree(9);
        select(&mut app, "api");
        // The cursor on the grid's first card, the way a project opens:
        // everything missing is under the fold.
        let first = crate::launcher::rows(&app)[0].agent.id.0.clone();
        aim_at(&mut app, &first);
        // One column, room for two rows of cards: seven of the nine are
        // under the fold.
        let body = Rect::new(0, 0, 60, HEAD_H + CARD_H * 2 + 1);
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(body.width, body.height))
                .unwrap();
        terminal.draw(|f| draw(f, &mut app, body)).unwrap();
        let buf = terminal.backend().buffer().clone();
        let text = |row: u16| -> String {
            (0..body.width)
                .filter_map(|x| buf.cell((x, row)))
                .map(|c| c.symbol().to_string())
                .collect()
        };
        let head = text(1);
        assert!(head.contains("9 sessions"), "{head:?}");
        assert!(head.contains("↓ 7 hidden"), "{head:?}");

        // And the grid really did draw two of them: the header is not
        // guessing at a number the drawing disagrees with.
        let drawn = (0..body.height)
            .map(text)
            .filter(|line| line.contains("session "))
            .count();
        assert_eq!(drawn, 2, "two cards on screen, seven hidden");
    }

    /// A grid with room for every card keeps the header it has today —
    /// the marker is a thing that appears, not a thing that is always
    /// there saying zero.
    #[test]
    fn a_grid_that_fits_says_nothing_about_hiding() {
        use crate::launcher::{CARD_H, HEAD_H};
        let mut app = a_crowded_tree(2);
        select(&mut app, "api");
        let body = Rect::new(0, 0, 60, HEAD_H + CARD_H * 2 + 1);
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(body.width, body.height))
                .unwrap();
        terminal.draw(|f| draw(f, &mut app, body)).unwrap();
        let buf = terminal.backend().buffer().clone();
        let head: String = (0..body.width)
            .filter_map(|x| buf.cell((x, 1)))
            .map(|c| c.symbol().to_string())
            .collect();
        assert!(head.contains("2 sessions"), "{head:?}");
        assert!(!head.contains("hidden"), "{head:?}");
    }

    /// Every row of `body` as drawn with the view on it.
    fn drawn_lines(app: &mut App, body: Rect) -> Vec<String> {
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(body.width, body.height))
                .unwrap();
        terminal.draw(|f| draw(f, app, body)).unwrap();
        let buf = terminal.backend().buffer().clone();
        (0..body.height)
            .map(|y| {
                (0..body.width)
                    .filter_map(|x| buf.cell((x, y)))
                    .map(|c| c.symbol().to_string())
                    .collect()
            })
            .collect()
    }

    /// The air under the last whole row of cards says how many more are
    /// down there — in that air, under the cards, not over them — and
    /// says nothing once the walk down has brought the last one on screen.
    #[test]
    fn the_air_under_the_cards_says_how_many_more_are_below() {
        use crate::launcher::{CARD_H, GAP_Y, HEAD_H};
        let mut app = a_crowded_tree(9);
        select(&mut app, "api");
        let ids: Vec<String> = crate::launcher::rows(&app)
            .iter()
            .map(|row| row.agent.id.0.clone())
            .collect();
        aim_at(&mut app, &ids[0]);
        // One column, two rows of cards and four rows of air under them.
        let cards_end = HEAD_H + CARD_H * 2 + GAP_Y;
        let body = Rect::new(0, 0, 60, cards_end + 4);
        let lines = drawn_lines(&mut app, body);
        let cue: Vec<usize> = (0..lines.len())
            .filter(|&y| lines[y].contains("more session"))
            .collect();
        assert_eq!(cue.len(), 1, "{lines:#?}");
        assert!(
            lines[cue[0]].contains("↓ 7 more sessions below"),
            "{:?}",
            lines[cue[0]]
        );
        assert!(cue[0] >= cards_end as usize, "under the cards: {lines:#?}");

        // Walked to the last card, nothing is left below to point at.
        aim_at(&mut app, &ids[8]);
        let lines = drawn_lines(&mut app, body);
        assert!(
            lines.iter().all(|line| !line.contains("more session")),
            "{lines:#?}"
        );
    }

    /// A grid with every card on screen leaves its air empty, however
    /// much of it there is.
    #[test]
    fn a_grid_that_fits_leaves_its_air_empty() {
        use crate::launcher::{CARD_H, HEAD_H};
        let mut app = a_crowded_tree(2);
        select(&mut app, "api");
        let body = Rect::new(0, 0, 60, HEAD_H + CARD_H * 2 + 6);
        let lines = drawn_lines(&mut app, body);
        assert!(
            lines.iter().all(|line| !line.contains("more session")),
            "{lines:#?}"
        );
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

    /// The PANE's TAB STRIP, as one string.
    fn strip_text(app: &App, room: usize) -> String {
        pane_tabs(app, room)
            .iter()
            .flat_map(|tab| tab.spans.iter())
            .map(|s| s.content.as_ref())
            .collect()
    }

    /// A tree whose `api` card runs in a `feat` checkout holding `count`
    /// terminals — the strip's whole subject.
    fn a_tree_with_terminals(count: usize) -> App {
        use nebula_core::{TerminalId, TerminalTab, WorktreeId};
        let mut app = a_tree();
        select(&mut app, "api");
        app.tree.terminals = (1..=count)
            .map(|i| TerminalTab {
                id: TerminalId(format!("t{i}")),
                worktree_id: WorktreeId("w0".into()),
                name: format!("shell-{i}"),
                sort_order: i as i64,
                alive: true,
                run_command: None,
            })
            .collect();
        app
    }

    /// A strip too narrow for every tab counts what it could not draw
    /// rather than clipping a name in half — and never gives up the
    /// SESSION tab or the checkout to do it, since those are what the
    /// terminals after them are scoped by.
    #[test]
    fn a_narrow_strip_counts_the_tabs_it_cannot_draw() {
        let app = a_tree_with_terminals(6);
        let width = |room| {
            pane_tabs(&app, room)
                .iter()
                .map(PaneTab::width)
                .sum::<usize>()
        };

        let wide = strip_text(&app, 140);
        assert!(wide.contains("shell-6"), "all six fit: {wide:?}");
        assert!(!wide.contains('+'), "so nothing is counted: {wide:?}");

        let tight = strip_text(&app, 60);
        assert!(tight.contains("SESSION"), "{tight:?}");
        assert!(
            tight.contains("feat"),
            "the checkout never gives way: {tight:?}"
        );
        assert!(
            tight.contains('+'),
            "what did not fit is counted: {tight:?}"
        );
        assert!(
            !tight.contains("shell-6"),
            "and is not also drawn: {tight:?}"
        );
        assert!(width(60) <= 60, "the strip fits its room: {tight:?}");
    }

    /// The window slides to keep the tab the pane is READING on screen:
    /// with more terminals than fit, a pinned one past the right edge
    /// pulls the strip to it and what is now behind is counted on the
    /// left. Otherwise `` ` `` could walk onto a tab that nothing on the
    /// strip shows as active.
    #[test]
    fn the_strip_keeps_the_tab_it_is_reading_on_screen() {
        use nebula_core::TerminalId;
        let mut app = a_tree_with_terminals(6);
        // Narrow enough that the last terminals fall off the end.
        let room = 60;
        let drawn = |app: &App| -> Vec<usize> {
            pane_tabs(app, room)
                .iter()
                .filter_map(|tab| match tab.hit {
                    Some(HitTarget::LauncherPaneTerminal(i)) => Some(i),
                    _ => None,
                })
                .collect()
        };
        assert!(
            !drawn(&app).contains(&5),
            "the last tab is off the end to begin with: {:?}",
            drawn(&app)
        );

        app.launcher_terminal = Some(TerminalId("t6".into()));
        assert!(
            drawn(&app).contains(&5),
            "reading it pulls the strip to it: {:?}",
            drawn(&app)
        );
        let text = strip_text(&app, room);
        assert!(text.contains("shell-6"), "and draws it: {text:?}");
        assert!(
            text.contains('+'),
            "with what is behind it counted: {text:?}"
        );
        assert!(
            pane_tabs(&app, room)
                .iter()
                .map(PaneTab::width)
                .sum::<usize>()
                <= room,
            "and still fits: {text:?}"
        );
    }

    /// With no terminals in the checkout the strip names the key that
    /// opens one rather than trailing off after the divider.
    #[test]
    fn an_empty_checkout_says_which_key_opens_a_terminal() {
        let app = a_tree_with_terminals(0);
        let text = strip_text(&app, 120);
        assert!(text.contains("SESSION"), "{text:?}");
        assert!(text.contains('│'), "the divider is still drawn: {text:?}");
        assert!(text.contains("opens a terminal here"), "{text:?}");
    }

    /// The HIDDEN MARKER: a grid that holds every card says nothing, and
    /// one that does not says how many it left off and which way they
    /// went — so a PANE dragged up over the cards reads as the pane
    /// taking their room rather than as sessions going missing.
    #[test]
    fn the_header_says_how_many_cards_it_could_not_fit() {
        let app = App::new();
        let th = app.theme;
        let mark = |hidden| row_text(&count_spans(&app, 9, hidden, 80, th));

        assert_eq!(mark(Hidden::default()), "9 sessions", "nothing is missing");
        assert_eq!(
            mark(Hidden { above: 0, below: 5 }),
            "9 sessions  ↓ 5 hidden",
            "under the fold: step down to them"
        );
        assert_eq!(
            mark(Hidden { above: 4, below: 0 }),
            "9 sessions  ↑ 4 hidden"
        );
        assert_eq!(
            mark(Hidden { above: 2, below: 3 }),
            "9 sessions  ↑↓ 5 hidden",
            "both ways at once, counted together"
        );
    }

    /// The marker holds the right edge however narrow the row: the PR &
    /// ISSUE COUNTS beside it give way first, and the row never overruns.
    #[test]
    fn the_hidden_marker_outlasts_the_counts_beside_it() {
        let mut app = a_tree();
        select(&mut app, "api");
        let th = app.theme;
        let hidden = Hidden { above: 0, below: 5 };
        for width in 10..=120usize {
            let spans = count_spans(&app, 9, hidden, width, th);
            let text = row_text(&spans);
            assert!(text.contains("5 hidden"), "{width}: {text:?}");
            let used: usize = spans.iter().map(|s| s.width()).sum();
            if used > width {
                // Only the count itself and the marker are left; nothing
                // else was there to drop.
                assert_eq!(text, "9 sessions  ↓ 5 hidden", "{width}: {text:?}");
            }
        }
    }

    /// [`head_count`]'s spans without the buttons they carry.
    fn count_spans(
        app: &App,
        count: usize,
        hidden: Hidden,
        width: usize,
        th: Theme,
    ) -> Vec<Span<'static>> {
        head_count(app, count, hidden, width, th)
            .into_iter()
            .map(|(span, _)| span)
            .collect()
    }

    /// The row as one string, the way it lands on screen.
    fn row_text(spans: &[Span<'static>]) -> String {
        spans.iter().map(|s| s.content.as_ref()).collect()
    }

    /// `a_tree` on `api`, with both projects open: `web` opened last, so
    /// its tab leads.
    fn a_tabbed_tree() -> App {
        let mut app = a_tree();
        select(&mut app, "api");
        app.launcher_tabs = vec![ProjectId("p1".into()), ProjectId("p0".into())];
        app
    }

    use nebula_core::ProjectId;

    /// The header's buttons, in the order they were laid down.
    fn head_hits(app: &App) -> Vec<HitTarget> {
        app.hits.iter().map(|(_, h)| h.clone()).collect()
    }

    /// The header is a `+` and then the open projects as tabs, the newest
    /// opened next to it — where the `+` puts the next one — each with its
    /// own `×`: no `nebula`, no trail. The tab the grid is on is lit: a raised
    /// chip with its name in the accent, where the rest sit flat.
    #[test]
    fn the_header_is_the_open_projects_as_tabs() {
        let r = Rect::new(0, 0, 80, 1);
        let mut app = a_tabbed_tree();
        let th = app.theme;
        app.hits.clear();
        let spans = head_tabs(&mut app, r, 0);
        let text = row_text(&spans);
        assert_eq!(text, " +   web ×   api × ");
        let (web, api) = (ProjectId("p1".into()), ProjectId("p0".into()));
        assert_eq!(
            head_hits(&app),
            vec![
                HitTarget::LauncherTabAdd,
                HitTarget::LauncherTab(web.clone()),
                HitTarget::LauncherTabClose(web),
                HitTarget::LauncherTab(api.clone()),
                HitTarget::LauncherTabClose(api.clone()),
            ]
        );
        // Each tab's rect is its own name and never the `×` beside it.
        let (rect, _) = app
            .hits
            .iter()
            .find(|(_, h)| *h == HitTarget::LauncherTab(api.clone()))
            .expect("api's tab");
        let cells: Vec<char> = text.chars().collect();
        let tab: String = cells[rect.x as usize..(rect.x + rect.width) as usize]
            .iter()
            .collect();
        assert_eq!(tab, " api", "the name, and never the × beside it");

        let lit = spans.iter().find(|s| s.content == "api").expect("api");
        assert_eq!(lit.style.fg, Some(th.accent));
        assert_eq!(lit.style.bg, Some(th.sel_bg), "the lit tab is a chip");
        let flat = spans.iter().find(|s| s.content == "web").expect("web");
        assert_eq!(flat.style.fg, Some(th.muted));
        assert_eq!(flat.style.bg, None);

        // With nothing open, the `+` says what it does.
        app.launcher_tabs.clear();
        app.hits.clear();
        let spans = head_tabs(&mut app, r, 0);
        assert_eq!(row_text(&spans), " + open a project ");
        assert_eq!(head_hits(&app), vec![HitTarget::LauncherTabAdd]);
    }

    /// A tab's STATUS DOTS: a dot and a count per state its project's
    /// sessions are in, right of the name, in the one order — needs-you
    /// red, done blue, running yellow — with no word of its own. A quiet
    /// project is its bare name.
    #[test]
    fn a_tab_carries_its_projects_status_dots() {
        use nebula_core::{AgentId, AgentStatus};
        let r = Rect::new(0, 0, 80, 1);
        let mut app = a_tabbed_tree();
        let th = app.theme;
        // api: one asking, one mid-turn; web: one finished unread.
        let mut asking = app.tree.agents[0].clone();
        asking.id = AgentId("a9".into());
        asking.status = AgentStatus::NeedsFeedback;
        app.tree.agents.push(asking);
        app.tree.agents[0].status = AgentStatus::Running;
        app.tree.agents[1].unseen = true;
        select(&mut app, "api");
        app.hits.clear();
        let spans = head_tabs(&mut app, r, 0);
        assert_eq!(row_text(&spans), " +   web ●1 ×   api ●1 ●1 × ");
        let dots: Vec<(String, Option<Color>)> = spans
            .iter()
            .filter(|s| s.content.contains('●'))
            .map(|s| (s.content.to_string(), s.style.fg))
            .collect();
        assert_eq!(
            dots,
            vec![
                (" ●1".to_string(), Some(th.done)),
                (" ●1".to_string(), Some(th.err)),
                (" ●1".to_string(), Some(th.warn)),
            ]
        );
    }

    /// A lone tab is the project on screen with nothing to hand the grid
    /// to, so it draws no `×` and lays down no target for one — the key
    /// that would close it says why instead (`event_loop::launcher::
    /// close_tab`). A second tab brings the crosses back on both.
    #[test]
    fn a_lone_tab_has_no_cross() {
        let r = Rect::new(0, 0, 80, 1);
        let mut app = a_tabbed_tree();
        app.launcher_tabs = vec![ProjectId("p0".into())];
        let spans = head_tabs(&mut app, r, 0);
        assert_eq!(row_text(&spans), " +   api ");
        assert!(!head_hits(&app)
            .iter()
            .any(|h| matches!(h, HitTarget::LauncherTabClose(_))));

        let mut app = a_tabbed_tree();
        app.hits.clear();
        let spans = head_tabs(&mut app, r, 0);
        assert_eq!(row_text(&spans), " +   web ×   api × ");
    }

    /// The tabs sweep in place: whatever the work under them is doing, the
    /// names spell the same names in the same columns — a running project's
    /// name is recolored a cell at a time ([`tab_ramp`]), never moved.
    #[test]
    fn the_tabs_sweep_in_place_whatever_is_running() {
        use nebula_core::AgentStatus;
        let r = Rect::new(0, 0, 80, 1);
        let mut app = a_tabbed_tree();
        app.hits.clear();
        head_tabs(&mut app, r, 0);
        let quiet = std::mem::take(&mut app.hits);
        for a in &mut app.tree.agents {
            a.status = AgentStatus::Running;
        }
        select(&mut app, "api");
        let busy = head_tabs(&mut app, r, 0);
        assert!(row_text(&busy).contains(" web"), "{:?}", row_text(&busy));
        assert!(
            !busy.iter().any(|s| s.content == "web"),
            "swept a cell at a time"
        );
        let busy_hits = std::mem::take(&mut app.hits);
        let first_tab = |hits: &[(Rect, HitTarget)]| {
            hits.iter()
                .find(|(_, h)| matches!(h, HitTarget::LauncherTab(_)))
                .map(|(r, h)| (r.x, h.clone()))
        };
        assert_eq!(
            first_tab(&quiet),
            first_tab(&busy_hits),
            "the first tab starts where it did"
        );
    }

    /// Red outranks everything, yellow outranks an unread finish, and blue
    /// is only a project with nothing live left; a quiet one holds still.
    #[test]
    fn a_tabs_ramp_is_the_loudest_state_under_it() {
        let th = Theme::default();
        let tally = |needs_you, running, done| Tally {
            needs_you,
            done,
            running,
        };
        assert_eq!(tab_ramp(tally(1, 3, 2), th), Some(th.err_sweep));
        assert_eq!(tab_ramp(tally(0, 1, 2), th), Some(th.warn_sweep));
        assert_eq!(tab_ramp(tally(0, 0, 2), th), Some(th.done_sweep));
        assert_eq!(tab_ramp(tally(0, 0, 0), th), None);
    }

    /// The pointer marks the button it rests on: a tab's name underlines,
    /// its `×` turns red, the `+` underlines — and nothing else does.
    #[test]
    fn the_pointer_marks_the_tab_it_rests_on() {
        let r = Rect::new(0, 0, 80, 1);
        let mut app = a_tabbed_tree();
        let th = app.theme;
        let web = ProjectId("p1".into());
        let underlined = |spans: &[Span<'static>]| -> Vec<String> {
            spans
                .iter()
                .filter(|s| s.style.add_modifier.contains(Modifier::UNDERLINED))
                .map(|s| s.content.to_string())
                .collect()
        };
        assert!(underlined(&head_tabs(&mut app, r, 0)).is_empty());

        app.hover_crumb = Some(HitTarget::LauncherTab(web.clone()));
        assert_eq!(underlined(&head_tabs(&mut app, r, 0)), ["web"]);

        app.hover_crumb = Some(HitTarget::LauncherTabAdd);
        assert_eq!(underlined(&head_tabs(&mut app, r, 0)), ["+"]);

        app.hover_crumb = Some(HitTarget::LauncherTabClose(web));
        let spans = head_tabs(&mut app, r, 0);
        assert!(underlined(&spans).is_empty());
        let crosses: Vec<Option<Color>> = spans
            .iter()
            .filter(|s| s.content.contains('×'))
            .map(|s| s.style.fg)
            .collect();
        assert_eq!(crosses, [Some(th.err), Some(th.muted)], "web's is red");
    }

    /// More tabs than the row holds: the ones past the edge are counted
    /// there rather than drawn half, the lit tab is always in the window,
    /// the `+` is never pushed off, and nothing overruns the row at any
    /// width.
    #[test]
    fn tabs_that_do_not_fit_are_counted_at_the_edge() {
        use nebula_core::Project;
        let mut app = a_tree();
        for i in 2..9 {
            app.tree.projects.push(Project {
                id: ProjectId(format!("p{i}")),
                name: format!("project-number-{i}"),
                repo_path: format!("/tmp/p{i}").into(),
                sort_order: 0,
            });
        }
        // Every project open, `api` — the oldest opened — last.
        app.launcher_tabs = (0..9).rev().map(|i| ProjectId(format!("p{i}"))).collect();
        select(&mut app, "api");
        for width in 5..=160u16 {
            let r = Rect::new(0, 0, width, 1);
            app.hits.clear();
            let spans = head_tabs(&mut app, r, 0);
            let text = row_text(&spans);
            let used: usize = spans.iter().map(|s| s.content.chars().count()).sum();
            assert!(used + 2 <= width as usize, "{width}: {text:?} overruns");
            assert!(text.contains('+'), "{width}: the + went: {text:?}");
            if width >= 24 {
                assert!(text.contains("api"), "{width}: the lit tab went: {text:?}");
                assert!(text.contains('‹'), "{width}: {text:?}");
            }
        }
        // Wide enough for all of them, nothing is counted.
        let r = Rect::new(0, 0, 400, 1);
        let text = row_text(&head_tabs(&mut app, r, 0));
        assert!(!text.contains('‹') && !text.contains('›'), "{text:?}");
    }

    /// The prompt header keeps the toggle whatever else it has to drop,
    /// never overruns its row, and hands back the columns a click on the
    /// toggle lands in. Where the launch lands is the details row's to
    /// say: the header names no project and no branch.
    #[test]
    fn the_prompt_header_never_drops_the_toggle() {
        let th = Theme::default();
        let launch = a_launch();
        let label = "what should the agent do?";
        for width in 40..=120u16 {
            let TargetLine { line, toggle } = target_line(&launch, label, width, th);
            let text = text_of(&line);
            assert!(line.width() <= width as usize, "{width}: {text:?}");
            assert!(text.contains("new worktree"), "{width}: {text:?}");
            assert!(
                !text.contains(" / ") && !text.contains('▾'),
                "{width}: the header carries no checkout crumb: {text:?}"
            );
            let (x, w) = toggle.expect("a worktree launch has a toggle to click");
            assert_eq!(usize::from(x + w), line.width(), "{width}: {text:?}");
        }
    }

    /// The branch in the details row is the WORKTREE PICKER's button: the
    /// columns handed back are exactly the branch and its `▾`, inside the
    /// `worktree` field's own, however the row had to shrink — and none
    /// once the field is dropped for room. A PR SESSION's branch is no
    /// button at all, and wears no `▾`: its checkout is the DAEMON's to
    /// pick.
    #[test]
    fn the_details_row_hands_back_the_branch_button() {
        let th = Theme::default();
        let app = App::new();
        let mut seen = 0;
        for width in 10..=120u16 {
            let details = detail_line(&app, &a_launch(), width, th);
            let text: Vec<char> = text_of(&details.line).chars().collect();
            let field = details
                .fields
                .iter()
                .find(|(f, _, _)| *f == BoxField::Worktree);
            let Some((x, w)) = details.branch else {
                assert!(!text.contains(&'▾'), "{width}: no button, no ▾");
                assert!(field.is_none(), "{width}: no branch, no field");
                continue;
            };
            seen += 1;
            let under: String = text[usize::from(x)..usize::from(x + w)].iter().collect();
            assert!(
                under.starts_with("(worktree") && under.ends_with(" ▾"),
                "{width}: {under:?}"
            );
            let (_, fx, fw) = field.expect("a drawn branch is a field");
            assert!(
                *fx <= x && x + w <= fx + fw,
                "{width}: the branch sits inside its field"
            );
        }
        assert!(seen > 0, "some width has room for the branch");

        let pr = a_launch().with_pr(Some(crate::pull_request::PrLaunch {
            url: "https://github.com/o/r/pull/7".into(),
            head: "fix-nav".into(),
            number: 7,
        }));
        let details = detail_line(&app, &pr, 120, th);
        let text = text_of(&details.line);
        assert!(text.contains("worktree fix-nav"), "{text:?}");
        assert!(!text.contains('▾'), "{text:?}");
        assert_eq!(details.branch, None);
        assert!(
            !details
                .fields
                .iter()
                .any(|(f, _, _)| *f == BoxField::Worktree),
            "{text:?}"
        );
    }

    /// The branch a fresh worktree will be cut on is drawn in the green
    /// the box's frame turns, and cut before the project is when even the
    /// tight row runs short.
    #[test]
    fn a_fresh_worktree_branch_is_green_and_cut_first() {
        let th = Theme::default();
        let app = App::new();
        let mut launch = a_launch();
        launch.target = crate::quick_prompt::QuickTarget::NewWorktree {
            project: nebula_core::ProjectId("p".into()),
            branch: "yellow-fox-jumps-over-the-lazy-dog".into(),
        };
        let wide = detail_line(&app, &launch, 400, th).line;
        let branch = wide
            .spans
            .iter()
            .find(|s| s.content == "yellow-fox-jumps-over-the-lazy-dog")
            .expect("the branch, whole, on a wide row");
        assert_eq!(branch.style.fg, Some(th.ok));

        // Five columns short of the tight form — the airy one gave up
        // whole, as it does — so something has to be cut.
        let tight = detail_line(&app, &launch, wide.width() as u16 - 1, th).line;
        let short = tight.width() as u16 - 5;
        let text = text_of(&detail_line(&app, &launch, short, th).line);
        assert!(
            text.contains("(project gone)"),
            "the project kept whole: {text:?}"
        );
        assert!(text.contains("yellow-fox-"), "the branch cut: {text:?}");
        assert!(!text.contains("lazy-dog"), "the branch cut: {text:?}");
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

    /// The checkout's changed-file count rides right behind the branch in
    /// the heads-up color, and on a card too narrow for the word it keeps
    /// the number and drops `files` before the branch gives up more.
    #[test]
    fn a_card_counts_its_checkouts_changes_behind_the_branch() {
        use nebula_core::{Agent, AgentId, AgentKind, AgentStatus, WorktreeId};
        let row = LauncherRow {
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
            branch: "feat-x".into(),
            is_main: false,
            pr: None,
        };
        let th = Theme::default();
        let mut app = App::new();
        app.worktree_changes.insert(
            WorktreeId("w1".into()),
            (Some(3), std::time::Instant::now()),
        );
        let second_row = |width: u16| -> (String, String) {
            let area = Rect::new(0, 0, width, crate::launcher::CARD_H);
            let mut terminal =
                ratatui::Terminal::new(ratatui::backend::TestBackend::new(width, area.height))
                    .unwrap();
            terminal
                .draw(|f| draw_card(f, &app, area, &row, false, true, th, &mut None))
                .unwrap();
            let buf = terminal.backend().buffer().clone();
            let cells = || (0..width).filter_map(|x| buf.cell((x, 2)));
            (
                cells().map(|c| c.symbol().to_string()).collect(),
                cells()
                    .filter(|c| c.fg == th.warn)
                    .map(|c| c.symbol().to_string())
                    .collect(),
            )
        };

        let (text, warn) = second_row(44);
        assert!(text.contains("↳ feat-x +3 files · claude"), "{text:?}");
        assert_eq!(warn.trim(), "+3 files");

        let (text, warn) = second_row(22);
        assert_eq!(warn.trim(), "+3", "narrow: {text:?}");
        assert!(text.contains("· claude"), "{text:?}");
    }

    /// A card's frame carries its status — red asking, blue done unread,
    /// the faint running yellow — and nothing else does: a read finish, a
    /// cold or archived card keeps the edge, and the focused card's accent
    /// outranks every status, so focus and running never look alike.
    #[test]
    fn a_cards_frame_answers_to_its_status_under_the_focus() {
        use nebula_core::{Agent, AgentId, AgentKind, AgentStatus, WorktreeId};
        let base = Agent {
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
        };
        let th = Theme::by_name("amber");
        let mut app = App::new();
        app.theme = th;
        let frame = |agent: Agent, selected: bool, focused: bool| {
            let row = LauncherRow {
                agent,
                project: "nebula".into(),
                branch: "feat-x".into(),
                is_main: false,
                pr: None,
            };
            let area = Rect::new(0, 0, 40, crate::launcher::CARD_H);
            let mut terminal =
                ratatui::Terminal::new(ratatui::backend::TestBackend::new(area.width, area.height))
                    .unwrap();
            terminal
                .draw(|f| draw_card(f, &app, area, &row, selected, focused, th, &mut None))
                .unwrap();
            let buf = terminal.backend().buffer().clone();
            let corner = buf.cell((0, 0)).unwrap().fg;
            let side = buf.cell((0, 1)).unwrap().fg;
            assert_eq!(corner, side, "one color all the way round");
            corner
        };
        let with = |status: AgentStatus, unseen: bool| Agent {
            status,
            unseen,
            ..base.clone()
        };

        assert_eq!(
            frame(with(AgentStatus::NeedsFeedback, false), false, true),
            th.err
        );
        assert_eq!(
            frame(with(AgentStatus::Finished, true), false, true),
            th.done
        );
        assert_eq!(
            frame(with(AgentStatus::Running, false), false, true),
            th.warn_edge
        );
        assert_eq!(
            frame(with(AgentStatus::Finished, false), false, true),
            th.edge
        );
        assert_eq!(frame(with(AgentStatus::Fresh, false), false, true), th.edge);
        let cold = Agent {
            alive: false,
            ..with(AgentStatus::Running, false)
        };
        assert_eq!(frame(cold, false, true), th.edge);
        let archived = Agent {
            archived: true,
            ..with(AgentStatus::NeedsFeedback, false)
        };
        assert_eq!(frame(archived, false, true), th.edge);

        // The focus wins over every status; off the grid, the cursor's
        // card shows its status like any other.
        assert_eq!(
            frame(with(AgentStatus::Running, false), true, true),
            th.accent
        );
        assert_eq!(
            frame(with(AgentStatus::NeedsFeedback, false), true, true),
            th.accent
        );
        assert_eq!(
            frame(with(AgentStatus::Running, false), true, false),
            th.warn_edge
        );
        assert_eq!(
            frame(with(AgentStatus::Finished, false), true, false),
            th.muted
        );
        assert_ne!(th.warn_edge, th.accent);
    }

    /// CARD LINE COUNTS: with the setting on and a count read, the lines
    /// follow the file count in the DIFF VIEWER's green and red; a card too
    /// narrow for all of it drops the word, then the lines; with the setting
    /// off they never show.
    #[test]
    fn card_line_counts_follow_the_file_count_in_green_and_red() {
        use nebula_core::{Agent, AgentId, AgentKind, AgentStatus, WorktreeId};
        let row = LauncherRow {
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
            branch: "feat-x".into(),
            is_main: false,
            pr: None,
        };
        let th = Theme::default();
        let mut app = App::new();
        app.worktree_changes.insert(
            WorktreeId("w1".into()),
            (Some(3), std::time::Instant::now()),
        );
        app.worktree_lines.insert(
            WorktreeId("w1".into()),
            crate::git_diff::LineChanges {
                added: 120,
                removed: 45,
            },
        );
        let second_row = |app: &App, width: u16| -> (String, String, String, String) {
            let area = Rect::new(0, 0, width, crate::launcher::CARD_H);
            let mut terminal =
                ratatui::Terminal::new(ratatui::backend::TestBackend::new(width, area.height))
                    .unwrap();
            terminal
                .draw(|f| draw_card(f, app, area, &row, false, true, th, &mut None))
                .unwrap();
            let buf = terminal.backend().buffer().clone();
            let cells = || (0..width).filter_map(|x| buf.cell((x, 2)));
            let painted = |color| -> String {
                cells()
                    .filter(|c| c.fg == color)
                    .map(|c| c.symbol().to_string())
                    .collect()
            };
            (
                cells().map(|c| c.symbol().to_string()).collect(),
                painted(th.warn),
                painted(th.ok),
                painted(th.err),
            )
        };

        let (text, _, added, removed) = second_row(&app, 60);
        assert!(!text.contains("+120"), "off: {text:?}");
        assert_eq!((added.trim(), removed.trim()), ("", ""));

        app.card_line_changes = true;
        let (text, warn, added, removed) = second_row(&app, 60);
        assert!(
            text.contains("↳ feat-x +3 files +120 -45 · claude"),
            "{text:?}"
        );
        assert_eq!(warn.trim(), "+3 files");
        assert_eq!(added.trim(), "+120");
        assert_eq!(removed.trim(), "-45");

        let (text, warn, added, _) = second_row(&app, 36);
        assert!(text.contains("↳ feat-x +3 +120 -45 · claude"), "{text:?}");
        assert_eq!((warn.trim(), added.trim()), ("+3", "+120"));

        let (text, warn, added, _) = second_row(&app, 24);
        assert_eq!(warn.trim(), "+3", "narrowest: {text:?}");
        assert_eq!(added.trim(), "", "the lines go before the branch does");
    }

    /// An ARCHIVED card is the live card put away rather than the live card
    /// with a word added: its frame squares off, the round STATUS DOT gives
    /// way to the square ARCHIVED MARK, the name drops to muted, the SCOPE
    /// COLOR goes with the rest of the color, and the badge counts from when
    /// it was filed rather than from its last turn.
    #[test]
    fn an_archived_card_is_the_live_card_put_away() {
        use nebula_core::{Agent, AgentId, AgentKind, AgentStatus, WorktreeId};

        fn card(archived: bool) -> LauncherRow {
            LauncherRow {
                agent: Agent {
                    id: AgentId("a1".into()),
                    worktree_id: WorktreeId("w1".into()),
                    name: "fix login".into(),
                    status: AgentStatus::Finished,
                    archived,
                    // Filed two hours ago, last turn a minute ago: the two
                    // badges cannot be mistaken for one another.
                    archived_at: crate::app::now_ms() - 2 * 3_600_000,
                    unseen: false,
                    kind: AgentKind::Claude,
                    custom_harness: None,
                    model: None,
                    effort: None,
                    session_id: None,
                    cloud_session_id: None,
                    sort_order: 0,
                    status_changed_at: crate::app::now_ms() - 60_000,
                    alive: true,
                    recent_prompts: Vec::new(),
                },
                project: "nebula".into(),
                branch: "main".into(),
                is_main: true,
                pr: None,
            }
        }

        let th = Theme::default();
        let app = App::new();
        let area = Rect::new(0, 0, 44, crate::launcher::CARD_H);
        let drawn = |row: &LauncherRow| {
            let mut terminal =
                ratatui::Terminal::new(ratatui::backend::TestBackend::new(area.width, area.height))
                    .unwrap();
            terminal
                .draw(|f| draw_card(f, &app, area, row, false, true, th, &mut None))
                .unwrap();
            terminal.backend().buffer().clone()
        };
        // One row of the card as text, the frame and its air trimmed off
        // both ends.
        let line = |buf: &ratatui::buffer::Buffer, y: u16| -> String {
            let row: String = (0..area.width)
                .filter_map(|x| buf.cell((x, y)))
                .map(|c| c.symbol().to_string())
                .collect();
            row.trim_matches(|c| c == '\u{2502}' || c == ' ')
                .to_string()
        };
        // And one row's cells in a single color, for the rows whose whole
        // point is which color they are in.
        let colored = |buf: &ratatui::buffer::Buffer, y: u16, want: Color| -> String {
            (0..area.width)
                .filter_map(|x| buf.cell((x, y)))
                .filter(|c| c.fg == want)
                .map(|c| c.symbol().to_string())
                .collect::<String>()
                .trim_end()
                .to_string()
        };

        let live = drawn(&card(false));
        let gone = drawn(&card(true));

        // The frame: round corners live, square once filed \u{2014} the one part of
        // this a terminal with no color at all still says.
        assert!(
            line(&live, 0).starts_with('\u{256d}'),
            "{:?}",
            line(&live, 0)
        );
        assert!(
            line(&gone, 0).starts_with('\u{250c}'),
            "{:?}",
            line(&gone, 0)
        );

        // The mark stands where the dot stood, the name behind it starting
        // in the same column on both.
        assert!(
            line(&live, 1).starts_with("\u{25cf} fix login"),
            "{:?}",
            line(&live, 1)
        );
        assert!(
            line(&gone, 1).starts_with("\u{25aa} fix login"),
            "{:?}",
            line(&gone, 1)
        );

        // The archived name is muted, never the live card's text color.
        assert_eq!(colored(&gone, 1, th.muted), "fix login");
        assert_eq!(colored(&gone, 1, th.text), "");

        // The badge counts from the archiving, not from the last turn.
        assert!(line(&live, 1).ends_with("1m ago"), "{:?}", line(&live, 1));
        assert!(line(&gone, 1).ends_with("2h ago"), "{:?}", line(&gone, 1));

        // And the SCOPE COLOR goes with the rest of the color, the glyph
        // left to tell the two checkouts apart.
        assert_eq!(colored(&live, 2, th.root), "\u{2302} main");
        assert_eq!(colored(&gone, 2, th.root), "");
        assert_eq!(colored(&gone, 2, th.dim), "\u{2302} main \u{b7} claude");
    }
}
