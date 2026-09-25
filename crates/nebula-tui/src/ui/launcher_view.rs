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
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Paragraph, Widget};
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
/// Room a row of tabs keeps back for the `3›` that counts the tabs past
/// its right edge, so the last tab that fits is never the one that
/// leaves no room to say there are more.
const MORE_ROOM: usize = 5;
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
/// card steps down into that pane (`event_loop::launcher::enter_pane`);
/// on a body too short to draw one it full-screens the session over the
/// lot instead (`event_loop::launcher::open_session`, the `collapsed` arm
/// of `ui::draw`).
pub(super) fn draw(f: &mut Frame, app: &mut App, body: Rect) {
    app.body_area = body;
    app.settle_launcher_focus();
    // The project the grid is on gets its tab, whichever way it was
    // opened, before the header lays the tabs out.
    app.settle_project_tabs();
    let bands = crate::launcher::bands(app);
    let g = crate::launcher::bands_layout(body);
    let cursor = wearing(app, crate::launcher::band_cursor(app, &bands));
    let count = HeadCount::of(&bands);
    if bands.is_empty() {
        draw_head(f, app, body, count, Hidden::default());
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
    // The ACCORDION: at most one band's cards wrapped into rows in place
    // (`App::launcher_expanded`) — pushing the bands after it down, so
    // the whole panel, not just that band, may run taller than the
    // screen and scrolls as one list.
    // In the compact LIST every band is its entries stacked a line
    // apiece instead (Settings → Appearance → **Worktree layout**).
    let panel = app.panel_layout(&bands);
    let scroll = settle_panel_scroll(app, &panel, &bands, cursor);
    draw_head(f, app, body, count, panel.hidden(scroll));
    draw_bands(f, app, &g, &panel, &bands, cursor, scroll);
}

/// The scroll this frame draws the GRID at, settled from what the last
/// frame left and what has happened since — the rule
/// [`App::launcher_scroll`] spells out. Opening or closing a band as the
/// ACCORDION starts from the top again rather than wherever the last
/// layout's scroll was left; a scroll the wheel set is kept unless a key
/// asked for the cursor's card ([`App::launcher_reveal`]) or the cursor
/// is on another card than the one it was set under; any other scroll
/// keeps in step with the cursor — its band whole on screen, or on the
/// open band its card with the row over it
/// ([`crate::launcher::PanelLayout::reveal`]). Whatever it lands on is
/// held within the panel, so a window that grew or a list that shrank
/// never scrolls past the last row.
fn settle_panel_scroll(
    app: &mut App,
    panel: &crate::launcher::PanelLayout,
    bands: &[crate::launcher::Band],
    cursor: Option<usize>,
) -> u16 {
    if app.launcher_scroll_in != app.launcher_expanded {
        app.launcher_scroll = 0;
        app.launcher_scroll_held = false;
        app.launcher_scroll_in = app.launcher_expanded.clone();
    }
    let mut scroll = panel.clamp(app.launcher_scroll);
    if let Some(index) = cursor {
        let band = &bands[index];
        let at = crate::launcher::card_cursor(app, band);
        let on = at.and_then(|i| band.cards.get(i)).map(|c| c.sref());
        if app.launcher_reveal || !app.launcher_scroll_held || app.launcher_scroll_on != on {
            scroll = panel.reveal(scroll, index, at);
            app.launcher_scroll_held = false;
            app.launcher_scroll_on = on;
        }
    }
    app.launcher_reveal = false;
    app.launcher_scroll = scroll;
    scroll
}

/// What the header counts: the cards on the grid, by kind.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct HeadCount {
    sessions: usize,
    terminals: usize,
}

impl HeadCount {
    fn of(bands: &[crate::launcher::Band]) -> Self {
        Self {
            sessions: bands.iter().map(|b| b.sessions()).sum(),
            terminals: bands.iter().map(|b| b.terminals()).sum(),
        }
    }
}

/// What the ARCHIVED VIEW says with nothing in it.
const NO_ARCHIVED: &str = "nothing archived in this project — ⇧A back to the live sessions";

/// The header over the grid: the PROJECT TABS on the left, each with its
/// status dots, how many cards the grid holds on the right — and how many
/// of them it could not fit — a rule under both: the same three-row head
/// the panels' columns sit on.
fn draw_head(f: &mut Frame, app: &mut App, body: Rect, count: HeadCount, hidden: Hidden) {
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
    let mut chips: Vec<[PaneTab; 2]> = tabs
        .iter()
        .map(|t| project_chip(t, PROJECT_TAB_MAX, hover.as_ref(), sweep, th))
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
            chips[start] = project_chip(&tabs[start], name_room, hover.as_ref(), sweep, th);
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
/// flat and muted. The name is cut to `name_max`. Every tab closes, the
/// last one included: that one closes to the splash.
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
        PaneTab {
            spans: vec![Span::styled(" × ", fill(Style::default().fg(cross)))],
            hit: Some(HitTarget::LauncherTabClose(tab.id.clone())),
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
    count: HeadCount,
    hidden: Hidden,
    width: usize,
    th: Theme,
) -> Vec<(Span<'static>, Option<HitTarget>)> {
    let mut spans = vec![(
        Span::styled(count_words(app, count), Style::default().fg(th.dim)),
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

/// The header's count of what the grid holds: `3 sessions · 2 terminals`
/// — the terminals only when there are any, since most projects have
/// none — under the ARCHIVED VIEW's own word there, so the header says
/// which of the two lists is on screen without a second line to read.
fn count_words(app: &App, count: HeadCount) -> String {
    let noun = if app.show_archived {
        "archived session"
    } else {
        "session"
    };
    let mut out = format!("{} {noun}{}", count.sessions, plural(count.sessions));
    if count.terminals > 0 {
        out.push_str(&format!(
            " · {} terminal{}",
            count.terminals,
            plural(count.terminals)
        ));
    }
    out
}

/// The HIDDEN MARKER: how many bands — or, inside a worktree, cards — the
/// grid holds that are not on screen, with an arrow saying which way
/// they went — `↓` past the bottom edge, `↑` scrolled off the top, `↑↓`
/// both. Nothing at all when the window holds the lot, so a grid that
/// fits reads exactly as it does today.
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

/// The GRID: one band per checkout, top to bottom — a collapsed one its
/// rule and one row of cards, the ACCORDION's open one its rule and
/// every card wrapped into rows under it — the whole panel scrolled by
/// rows so the cursor's card is always drawn, a band that straddles the
/// window's edge drawn cut ([`draw_cut`]) rather than left out.
fn draw_bands(
    f: &mut Frame,
    app: &mut App,
    g: &crate::launcher::BandsLayout,
    panel: &crate::launcher::PanelLayout,
    bands: &[crate::launcher::Band],
    cursor: Option<usize>,
    scroll: u16,
) {
    let th = app.theme;
    let window = panel.window();
    // One CONFIG.JSON read for the whole frame, and only if some card on
    // it runs a CUSTOM harness whose label lives in there — a screenful
    // of cards must not reload the file once per card.
    let mut cfg = None;
    // The keys are up on the PROJECT TABS, or down in the pane: the band
    // under the cursor keeps its bold branch — it still says which
    // checkout the pane reads — but the accent goes with the keys, so
    // only the one thing holding them is lit.
    let keys = app.focus != Focus::Terminal && app.launcher_tab_cursor.is_none();
    // The band the selection is on, aimed at or let go of: its row
    // follows its remembered card either way, so Esc scrolls nothing.
    let aimed = crate::launcher::band_cursor(app, bands);
    // Each band's whole rectangle, rule to last row of cards: a click on
    // the air around its cards picks the band as a click on its rule
    // does. Pushed after every card, rule and arrow so they keep their
    // own targets under `hit_at`'s first-match scan.
    let mut band_areas = Vec::with_capacity(bands.len());
    for (index, band) in bands.iter().enumerate() {
        let pb = &panel.bands[index];
        let whole = Rect {
            y: pb.rule_y,
            height: pb.height,
            ..g.area
        };
        if let Some(placed) = crate::launcher::place(window, scroll, whole) {
            band_areas.push((placed.rect, HitTarget::LauncherBand(index)));
        }
        let on = cursor == Some(index);
        let at = (aimed == Some(index))
            .then(|| crate::launcher::card_cursor(app, band))
            .flatten();
        // Collapsed: the STRIP's one row of cards under the rule, its
        // `y` wherever the panel put the band.
        let strip = pb.content.is_none().then(|| {
            g.strip_at(
                Rect {
                    y: pb.rule_y,
                    height: crate::launcher::BAND_H,
                    ..g.area
                },
                index,
                band,
                at,
            )
        });
        let rule = Rect {
            y: pb.rule_y,
            height: crate::launcher::BAND_RULE_H,
            ..g.area
        };
        // A one-row rule is on screen whole or not at all
        // ([`crate::launcher::place`]), so it needs no [`draw_cut`].
        if let Some(placed) = crate::launcher::place(window, scroll, rule) {
            let hits = draw_band_rule(
                f.buffer_mut(),
                app,
                placed.rect,
                band,
                BandRule {
                    index,
                    on,
                    lit: on && keys,
                    more: match &strip {
                        Some(strip) => strip.hidden(),
                        None => pb.content.as_ref().map_or(0, |c| c.more),
                    },
                    expanded: pb.open,
                    list: app.launcher_list,
                },
            );
            app.hits.extend(hits);
        }
        if band.cards.is_empty() {
            draw_empty_band(f, app, g, pb, band, (window, scroll), on && keys);
            continue;
        }
        // The cards under the rule: on the band the cursor is on, the
        // one it remembers — what the pane reads, and what the keys walk
        // — wears the cursor's accent outline, the tint staying with the
        // lit rule; the rest are a preview. A click on any lands the
        // cursor on it (`HitTarget::LauncherCard`), and a card drawn cut
        // is clicked on the rows of it there are: the landing scrolls
        // the rest of it into view (`settle_panel_scroll`).
        if app.launcher_list {
            draw_list_band(
                f,
                app,
                g,
                pb,
                ListBand {
                    index,
                    band,
                    window,
                    scroll,
                    on,
                    at,
                    lit: on && keys,
                },
                &mut cfg,
            );
            continue;
        }
        match &strip {
            None => {
                // Open: every card, wrapped into rows, the terminals
                // under a section rule of their own. The sessions' rule
                // is the band's, drawn above.
                for (y, label) in pb.rules() {
                    if label == crate::launcher::SESSIONS_RULE {
                        continue;
                    }
                    let rule = Rect {
                        y,
                        height: crate::launcher::BAND_RULE_H,
                        ..g.area
                    };
                    let Some(placed) = crate::launcher::place(window, scroll, rule) else {
                        continue;
                    };
                    let fill = "─".repeat(
                        usize::from(placed.rect.width).saturating_sub(label.chars().count() + 4),
                    );
                    f.render_widget(
                        Paragraph::new(Line::from(vec![
                            Span::styled("── ", Style::default().fg(th.edge)),
                            Span::styled(label.to_string(), Style::default().fg(th.dim)),
                            Span::styled(format!(" {fill}"), Style::default().fg(th.edge)),
                        ])),
                        placed.rect,
                    );
                }
                for (i, card) in band.cards.iter().enumerate() {
                    let Some(placed) = pb
                        .cell(i)
                        .and_then(|cell| crate::launcher::place(window, scroll, cell))
                    else {
                        continue;
                    };
                    let selected = on && at == Some(i);
                    draw_cut(f, placed, |buf, r| {
                        draw_any_card(buf, &*app, r, card, selected, false, th, &mut cfg)
                    });
                    note_tail_card(app, card);
                    app.hits.extend(card_issue_hit(app, card, placed));
                    app.hits.push((
                        placed.rect,
                        HitTarget::LauncherCard(crate::launcher::CardRef {
                            band: index,
                            card: i,
                        }),
                    ));
                }
            }
            Some(strip) => {
                let row = Rect {
                    y: pb.rule_y + crate::launcher::BAND_RULE_H,
                    height: crate::launcher::CARD_H,
                    ..g.area
                };
                let Some(row_placed) = crate::launcher::place(window, scroll, row) else {
                    continue;
                };
                for slot in &strip.cards {
                    let placed = crate::launcher::Placed {
                        rect: Rect {
                            x: slot.rect.x,
                            width: slot.rect.width,
                            ..row_placed.rect
                        },
                        cut_top: row_placed.cut_top,
                        cut_bottom: row_placed.cut_bottom,
                    };
                    let card = &band.cards[slot.at.card];
                    draw_cut(f, placed, |buf, r| {
                        draw_any_card(
                            buf,
                            &*app,
                            r,
                            card,
                            on && at == Some(slot.at.card),
                            false,
                            th,
                            &mut cfg,
                        )
                    });
                    note_tail_card(app, card);
                    app.hits.extend(card_issue_hit(app, card, placed));
                    app.hits
                        .push((placed.rect, HitTarget::LauncherCard(slot.at)));
                }
                // The arrows only on a row drawn whole: they stand beside
                // the cards' full height.
                if row_placed.whole() {
                    draw_strip_arrows(f, app, strip, row_placed.rect.y, index, on && keys, th);
                }
                if strip.hidden() > 0 {
                    // The band's own row under the cards
                    // ([`crate::launcher::MORE_H`]), the gap to the next
                    // band's rule under it.
                    let row = Rect {
                        y: pb.rule_y + crate::launcher::BAND_H,
                        height: crate::launcher::MORE_H,
                        ..g.area
                    };
                    if let Some(placed) = crate::launcher::place(window, scroll, row) {
                        let more = StripMore {
                            index,
                            hidden: strip.hidden(),
                            lit: on && keys,
                            centered: true,
                        };
                        draw_strip_more(f, app, placed.rect, band, more);
                    }
                }
            }
        }
    }
    draw_panel_edge_marks(f, panel, scroll, th);
    app.hits.extend(band_areas);
    // Last, so the bands themselves win `hit_at`'s first-match scan and
    // only the air between them falls through to the grid.
    app.hits.push((g.area, HitTarget::PanelBg(Focus::Sessions)));
}

/// What an EMPTY BAND says under its rule, the root's without the delete
/// it refuses.
const EMPTY_BAND_HINT: &str = "nothing running · p: new session · t: terminal";
const EMPTY_BAND_DELETE: &str = " · d: delete worktree";

/// The line under an EMPTY BAND's rule — a checkout with nothing running
/// in it, drawn only with **Show all worktrees** on — in place of the row
/// of cards it has none of: what can be done there, dim, and in the
/// accent while the cursor is on the band with the keys on the grid.
/// A click on it is a click on the band (the band's whole area).
fn draw_empty_band(
    f: &mut Frame,
    app: &App,
    g: &crate::launcher::BandsLayout,
    pb: &crate::launcher::PanelBand,
    band: &crate::launcher::Band,
    (window, scroll): (Rect, u16),
    lit: bool,
) {
    let th = app.theme;
    let row = Rect {
        y: pb.rule_y + crate::launcher::BAND_RULE_H,
        height: crate::launcher::EMPTY_BAND_ROW_H,
        ..g.area
    };
    let Some(placed) = crate::launcher::place(window, scroll, row) else {
        return;
    };
    let mut text = String::from(EMPTY_BAND_HINT);
    if !band.is_main {
        text.push_str(EMPTY_BAND_DELETE);
    }
    let fg = if lit { th.accent } else { th.dim };
    f.render_widget(
        Paragraph::new(Line::from(Span::styled(
            format!("  {text}"),
            Style::default().fg(fg),
        ))),
        placed.rect,
    );
}

/// The EDGE MARKERS on a grid taller than its window: `↑ 2 more above`
/// on the header's row of air over the cards, `↓ 3 more below` on the
/// row kept under them ([`crate::launcher::PanelLayout::window`]) —
/// each where the eye looks for the rest, so the grid says it scrolls,
/// and which way, without a word painted over a card. With every band
/// collapsed what is past the edge is whole checkouts, and the marker
/// says so (`↓ 3 more worktrees below`), so a screenful that happens to
/// end on a band does not read as the whole project; with one open its
/// cards are in the count too, and the marker just counts. The header's
/// `↑↓ 5 hidden` says the same once more. A grid that fits has neither
/// row marked, and a panel scrolled to an end leaves that end's row as
/// plain air.
fn draw_panel_edge_marks(
    f: &mut Frame,
    panel: &crate::launcher::PanelLayout,
    scroll: u16,
    th: Theme,
) {
    if !panel.overflows() {
        return;
    }
    let hidden = panel.hidden(scroll);
    let what = |n: usize| {
        if panel.open().is_some() {
            String::new()
        } else {
            format!(" worktree{}", plural(n))
        }
    };
    let mark = |f: &mut Frame, r: Rect, words: String| {
        f.render_widget(
            Paragraph::new(Span::styled(words, Style::default().fg(th.muted)))
                .alignment(ratatui::layout::Alignment::Center),
            r,
        );
    };
    if hidden.above > 0 {
        let r = Rect {
            y: panel.area.y.saturating_sub(1),
            height: 1,
            ..panel.area
        };
        let n = hidden.above;
        mark(f, r, format!("↑ {n} more{} above", what(n)));
    }
    if hidden.below > 0 {
        let window = panel.window();
        let r = Rect {
            y: window.y + window.height,
            height: crate::launcher::BELOW_MARK_H,
            ..panel.area
        };
        let n = hidden.below;
        mark(f, r, format!("↓ {n} more{} below", what(n)));
    }
}

/// The `❮` and `❯` beside a band's row.
const STRIP_LEFT: &str = "❮";
const STRIP_RIGHT: &str = "❯";

/// What stands beside a band's row when it holds more cards than it
/// shows: `❮` in the margin before its first card for the ones scrolled
/// past the left edge, `❯` after its last card for the ones past the
/// right, each on the row's middle line — in the accent while the band
/// holds the keys, muted on any other band, so a glance says the row
/// goes on without lighting it up. Each is a button: its hit is the
/// two-cell gap the glyph stands in, the cards' full height, and a click
/// on it is the step `h` / `l` take that way
/// (`HitTarget::LauncherStripLeft`, `LauncherStripRight`). Under the
/// pointer the glyph takes the accent whichever band it is on.
fn draw_strip_arrows(
    f: &mut Frame,
    app: &mut App,
    strip: &crate::launcher::Strip,
    top: u16,
    index: usize,
    lit: bool,
    th: Theme,
) {
    use crate::launcher::{CARD_H, PAD_X};
    let (Some(first), Some(last)) = (strip.cards.first(), strip.cards.last()) else {
        return;
    };
    let frame = f.area();
    // `top` is the row's line on screen: the slots' own `y` counts from
    // the top of the whole panel, before the window and its scroll.
    let y = top + CARD_H / 2;
    let mut arrow = |hit: HitTarget, x: u16, glyph_x: u16, glyph: &str| {
        let hovered = app.hover_crumb.as_ref() == Some(&hit);
        let style = if lit || hovered {
            Style::default().fg(th.accent).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(th.muted)
        };
        let cell = Rect {
            x: glyph_x,
            y,
            width: 1,
            height: 1,
        }
        .intersection(frame);
        if cell.width > 0 {
            f.render_widget(Paragraph::new(Span::styled(glyph.to_string(), style)), cell);
        }
        let target = Rect {
            x,
            y: top,
            width: PAD_X,
            height: CARD_H,
        }
        .intersection(frame);
        if target.width > 0 {
            app.hits.push((target, hit));
        }
    };
    if strip.before > 0 {
        // The margin before the row: the glyph, then a cell of air
        // against the first card.
        let x = first.rect.x.saturating_sub(PAD_X);
        arrow(HitTarget::LauncherStripLeft(index), x, x, STRIP_LEFT);
    }
    if strip.after > 0 {
        // The gap after the last card drawn — between cards or the
        // margin, whichever the row ends in: a cell of air, then the
        // glyph.
        let x = last.rect.x + last.rect.width;
        arrow(HitTarget::LauncherStripRight(index), x, x + 1, STRIP_RIGHT);
    }
}

/// The MORE HINT under a collapsed band's row that left cards off its
/// edges: `▾ 6 more · Tab: see all 8`, centered on the band's own row
/// under the cards ([`crate::launcher::MORE_H`]), so a band hiding sessions says so where the eye leaves the
/// cards rather than only at the far end of its rule. Muted, its key in
/// the accent while the band holds the keys or the pointer is on it. A
/// button: a click opens the band as the ACCORDION, the very toggle Tab
/// runs (`HitTarget::LauncherBandMore`).
fn draw_strip_more(
    f: &mut Frame,
    app: &mut App,
    r: Rect,
    band: &crate::launcher::Band,
    more: StripMore,
) {
    let StripMore {
        index,
        hidden,
        lit,
        centered,
    } = more;
    let th = app.theme;
    let hit = HitTarget::LauncherBandMore(index);
    let hovered = app.hover_crumb.as_ref() == Some(&hit);
    let key = super::key_hint(app, crate::keymap::Action::FocusNext);
    let (words, does) = (
        format!("▾ {hidden} more · "),
        format!(": see all {}", band.cards.len()),
    );
    let key_style = if lit || hovered {
        Style::default().fg(th.accent).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(th.dim)
    };
    let text = Style::default().fg(if hovered { th.accent } else { th.muted });
    let line = Line::from(vec![
        Span::styled(words, text),
        Span::styled(key, key_style),
        Span::styled(does, text),
    ]);
    let w = (line.width() as u16).min(r.width);
    // Centered under a row of cards; under a LIST, in the column its
    // entries' names start in.
    let x = if centered {
        r.x + (r.width - w) / 2
    } else {
        r.x + LIST_LEAD.min(r.width - w)
    };
    let at = Rect { x, width: w, ..r };
    f.render_widget(Paragraph::new(line), at);
    app.hits.push((at, hit));
}

/// What [`draw_strip_more`] says, and where: the band it is under, how
/// many it counts, whether the band holds the keys, and whether it
/// centers under a row of cards or starts where a LIST's names do.
#[derive(Debug, Clone, Copy)]
struct StripMore {
    index: usize,
    hidden: usize,
    lit: bool,
    centered: bool,
}

/// What the cursor's LIST entry wears in front of it.
const LIST_MARK: &str = "▌ ";
/// Columns a LIST entry spends before its name: the cursor mark and the
/// status dot (or a terminal's glyph), two apiece.
const LIST_LEAD: u16 = 4;
/// Longest a LIST's name column grows, and its runs-on column: past these
/// the entries truncate rather than push the prompt off the line.
const LIST_NAME_MAX: usize = 28;
const LIST_RUNS_MAX: usize = 24;
/// Air between a LIST entry's columns.
const LIST_GAP: usize = 2;

/// One band of the compact LIST as [`draw_list_band`] draws it.
struct ListBand<'a> {
    index: usize,
    band: &'a crate::launcher::Band,
    window: Rect,
    scroll: u16,
    /// The cursor is on the band, and `at` is its card.
    on: bool,
    at: Option<usize>,
    /// The keys are on the band too.
    lit: bool,
}

/// A band in the compact LIST: under its rule, one line per entry the
/// layout shows — its most recent sessions, or every card once it is
/// open (`launcher::list_layout`) — the names and what each runs on in
/// columns shared down the band, then the MORE HINT on the line under
/// them when it leaves any off. A click on a line lands the cursor on its
/// card (`HitTarget::LauncherCard`), exactly as a click on a card does.
fn draw_list_band(
    f: &mut Frame,
    app: &mut App,
    g: &crate::launcher::BandsLayout,
    pb: &crate::launcher::PanelBand,
    list: ListBand<'_>,
    cfg: &mut Option<crate::config::Config>,
) {
    let ListBand {
        index,
        band,
        window,
        scroll,
        on,
        at,
        lit,
    } = list;
    let Some(content) = pb.content.as_ref() else {
        return;
    };
    let shown: Vec<usize> = content
        .rows
        .iter()
        .filter_map(|r| r.first().copied())
        .collect();
    let mut cols = (0, 0);
    for &i in &shown {
        let (name, runs) = match &band.cards[i] {
            crate::launcher::Card::Session(row) => (
                row.agent.name.chars().count(),
                runs_on_line(&row.agent, cfg).chars().count(),
            ),
            crate::launcher::Card::Terminal(t) => (
                t.name.chars().count(),
                t.run_command.as_deref().unwrap_or("shell").chars().count(),
            ),
        };
        cols = (cols.0.max(name), cols.1.max(runs));
    }
    let cols = (cols.0.min(LIST_NAME_MAX), cols.1.min(LIST_RUNS_MAX));
    for &i in &shown {
        let Some(placed) = pb
            .cell(i)
            .and_then(|cell| crate::launcher::place(window, scroll, cell))
        else {
            continue;
        };
        let card = &band.cards[i];
        let selected = on && at == Some(i);
        draw_list_row(
            f.buffer_mut(),
            app,
            placed.rect,
            card,
            (selected, selected && lit),
            cols,
            cfg,
        );
        note_tail_card(app, card);
        app.hits.push((
            placed.rect,
            HitTarget::LauncherCard(crate::launcher::CardRef {
                band: index,
                card: i,
            }),
        ));
    }
    if let Some(y) = content.more_y() {
        let row = Rect {
            y: pb.rule_y + y,
            height: crate::launcher::MORE_H,
            ..g.area
        };
        if let Some(placed) = crate::launcher::place(window, scroll, row) {
            let more = StripMore {
                index,
                hidden: content.more,
                lit,
                centered: false,
            };
            draw_strip_more(f, app, placed.rect, band, more);
        }
    }
}

/// One entry of the compact LIST, on one line: the cursor mark, the
/// status dot and name a card heads with, what it runs on, and the last
/// thing it was asked — `›` as on the card — or, for a terminal, the last
/// line its shell printed; how long since it moved (or `exited`) at the
/// right. `cols` are the band's name and runs-on column widths, so the
/// entries line up. `(selected, lit)`: the cursor's entry wears the mark,
/// ([`LIST_MARK`]), and the FOCUSED PANEL TINT across the line while the
/// grid holds the keys.
fn draw_list_row(
    buf: &mut Buffer,
    app: &App,
    r: Rect,
    card: &crate::launcher::Card,
    (selected, lit): (bool, bool),
    (name_col, runs_col): (usize, usize),
    cfg: &mut Option<crate::config::Config>,
) {
    let th = app.theme;
    let width = usize::from(r.width);
    // A bar rather than the rule's `❯`: a shell's own glyph is `❯`, one
    // column over, and the two would read as one mark.
    let mark = if selected {
        Span::styled(LIST_MARK, Style::default().fg(th.accent))
    } else {
        Span::raw("  ")
    };
    struct Entry {
        lead: Span<'static>,
        name: String,
        name_style: Style,
        ramp: Option<[Color; 3]>,
        runs: String,
        runs_style: Style,
        text_mark: &'static str,
        text: String,
        text_style: Style,
        badge: String,
        badge_style: Style,
    }
    let e = match card {
        crate::launcher::Card::Session(row) => {
            let a = &row.agent;
            let look = session_look(app, a, selected, th);
            let quiet_or = |live: Color| if a.archived { look.quiet } else { live };
            Entry {
                lead: look.dot,
                name: a.name.clone(),
                name_style: look.name_style,
                ramp: look.ramp,
                runs: runs_on_line(a, cfg),
                runs_style: Style::default().fg(quiet_or(th.dim)),
                text_mark: "› ",
                text: crate::launcher::last_prompt(a)
                    .unwrap_or_default()
                    .to_string(),
                text_style: Style::default().fg(quiet_or(th.muted)),
                badge: look.ago,
                badge_style: look.ago_style,
            }
        }
        crate::launcher::Card::Terminal(t) => Entry {
            lead: Span::styled(
                if t.run_command.is_some() {
                    "▶ "
                } else {
                    "❯ "
                },
                Style::default().fg(if t.alive { th.ok } else { th.dim }),
            ),
            name: t.name.clone(),
            name_style: Style::default().fg(th.text).add_modifier(Modifier::BOLD),
            ramp: None,
            runs: t.run_command.clone().unwrap_or_else(|| "shell".into()),
            runs_style: Style::default().fg(th.dim),
            text_mark: "",
            text: terminal_tail_lines(app, t)
                .into_iter()
                .rev()
                .find(|l| !l.trim().is_empty())
                .unwrap_or_default(),
            text_style: Style::default().fg(th.muted),
            badge: if t.alive {
                String::new()
            } else {
                EXITED_BADGE.into()
            },
            badge_style: Style::default().fg(th.err),
        },
    };
    let mut spans = vec![mark, e.lead];
    let mut room = width.saturating_sub(usize::from(LIST_LEAD));
    // The badge at the right end, while the name keeps a few letters.
    let badge = e.badge.trim().to_string();
    let badge_w = badge.chars().count();
    let keep_badge = badge_w > 0 && room >= badge_w + 1 + 8;
    if keep_badge {
        room -= badge_w + 1;
    }
    let name_w = name_col.max(1).min(room);
    let name = truncate(&e.name, name_w);
    let mut used = name.chars().count();
    spans.extend(status_name_spans(
        name,
        e.name_style,
        e.ramp,
        app.sweep_phase(),
    ));
    // What it runs on, in the band's column, and the prompt after it —
    // each only where there is room for a few letters of it.
    let runs_at = name_w + LIST_GAP;
    if !e.runs.is_empty() && room >= runs_at + 6 {
        let runs_w = runs_col.min(room - runs_at);
        let runs = truncate(&e.runs, runs_w);
        spans.push(Span::raw(" ".repeat(runs_at - used)));
        used = runs_at + runs.chars().count();
        spans.push(Span::styled(runs, e.runs_style));
        let text_at = runs_at + runs_w + LIST_GAP;
        let text_mark_w = e.text_mark.chars().count();
        if !e.text.is_empty() && room >= text_at + text_mark_w + 6 {
            let text = truncate(&e.text, room - text_at - text_mark_w);
            spans.push(Span::raw(" ".repeat(text_at - used)));
            spans.push(Span::styled(e.text_mark, Style::default().fg(th.dim)));
            used = text_at + text_mark_w + text.chars().count();
            spans.push(Span::styled(text, e.text_style));
        }
    }
    if keep_badge {
        spans.push(Span::raw(" ".repeat(room.saturating_sub(used) + 1)));
        spans.push(Span::styled(badge, e.badge_style));
    }
    let mut line = Paragraph::new(Line::from(spans));
    if lit {
        line = line.style(Style::default().bg(th.focus_tint));
    }
    line.render(r, buf);
}

/// Draw something the window's edges may cut — a card half scrolled off
/// the top — as the terminal draws a line half scrolled off: the rows of
/// it inside the window, and only those. `draw` paints the whole thing
/// into the rect it is handed; one drawn whole paints straight onto the
/// frame, one that is cut paints onto a scratch buffer its own size and
/// the rows on screen are copied across. So a card's top border is
/// never drawn on a card whose top is off screen — a whole-looking card
/// where a cut one stands would say the wrong thing about what is
/// above it.
fn draw_cut(f: &mut Frame, placed: crate::launcher::Placed, draw: impl FnOnce(&mut Buffer, Rect)) {
    if placed.whole() {
        draw(f.buffer_mut(), placed.rect);
        return;
    }
    let full = Rect {
        y: 0,
        height: placed.rect.height + placed.cut_top + placed.cut_bottom,
        ..placed.rect
    };
    let mut scratch = Buffer::empty(full);
    draw(&mut scratch, full);
    let buf = f.buffer_mut();
    for dy in 0..placed.rect.height {
        for x in placed.rect.left()..placed.rect.right() {
            let Some(from) = scratch.cell((x, placed.cut_top + dy)) else {
                continue;
            };
            if let Some(to) = buf.cell_mut((x, placed.rect.y + dy)) {
                *to = from.clone();
            }
        }
    }
}

/// A card of either kind, in `area`.
#[allow(clippy::too_many_arguments)]
fn draw_any_card(
    buf: &mut Buffer,
    app: &App,
    area: Rect,
    card: &crate::launcher::Card,
    selected: bool,
    focused: bool,
    th: Theme,
    cfg: &mut Option<crate::config::Config>,
) {
    match card {
        crate::launcher::Card::Session(row) => {
            draw_card(buf, app, area, row, selected, focused, th, cfg)
        }
        crate::launcher::Card::Terminal(t) => draw_chip(buf, app, area, t, selected, focused, th),
    }
}

/// A terminal card just drawn: the grid's beat asks the daemon what its
/// shell printed since ([`App::tail_cards`], `event_loop::request_terminal_tails`).
fn note_tail_card(app: &mut App, card: &crate::launcher::Card) {
    if let crate::launcher::Card::Terminal(t) = card {
        app.tail_cards.push(t.id.clone());
    }
}

/// The least room a band's rule keeps for its glyph and branch before
/// the Enter hint at its right end gives way: the mark and the counts
/// stay, the words go.
const ENTER_HINT_ROOM: usize = 12;

/// Where a band's rule is being drawn: always on the grid over its
/// cards — the ACCORDION opens a band in place rather than replacing
/// the grid with it, so there is only ever the one kind now.
#[derive(Debug, Clone, Copy)]
struct BandRule {
    /// The band's place in `launcher::bands` — what its hit names.
    index: usize,
    /// The cursor is on the band: its branch goes bold.
    on: bool,
    /// The keys are on the band too — not up on the PROJECT TABS nor
    /// down in the pane: the rule takes the accent, the cursor mark
    /// and the Tab hint.
    lit: bool,
    /// Cards its row had no room for — always 0 once the band is open.
    more: usize,
    /// The band is open as the ACCORDION: its cards are wrapped into
    /// rows under this rule rather than the collapsed STRIP's one row,
    /// and the hint at the right end says Tab closes it.
    expanded: bool,
    /// The grid is the compact LIST: a collapsed band that already lists
    /// every entry has nothing for Tab to open, and its rule says nothing
    /// about it.
    list: bool,
}

/// A BAND's rule: the checkout — `⌂ main` or `↳ feat` in the SCOPE
/// COLOR, its uncommitted changes behind it in the heads-up color, its
/// pull request in the PR rows' own colors — a rule to the right end,
/// and there how many cards are under it (`2 sessions · 1 terminal`), or
/// off its edges (`▸ 2 more`). On the band the cursor is on the
/// branch is bold — this is what says which checkout the pane reads,
/// since no card under it wears the cursor — and for as long as the keys
/// are on it the rule wears the accent, opens on the CURSOR MARK `❯`
/// instead of `──`, and says at its right end what Tab does there
/// (`Tab: see all 8`, `Tab: expand`, or `Tab: collapse` on the one band
/// that is open); gray, unmarked and silent like the rest once the keys
/// are up on the PROJECT TABS or down in the pane.
/// Returns the rule's hits: the pull request ahead of the rule itself,
/// so a click on `#42` opens it and one anywhere else lands on the band.
fn draw_band_rule(
    buf: &mut Buffer,
    app: &App,
    r: Rect,
    band: &crate::launcher::Band,
    rule: BandRule,
) -> Vec<(Rect, HitTarget)> {
    let th = app.theme;
    let width = usize::from(r.width);
    let BandRule {
        on,
        lit,
        more,
        expanded,
        ..
    } = rule;
    let edge = if lit { th.accent } else { th.edge };
    let dash = |n: usize| Span::styled("─".repeat(n), Style::default().fg(edge));

    // The right end first, since the left gives way to it.
    let right = {
        let mut words = format!("{} session{}", band.sessions(), plural(band.sessions()));
        if band.terminals() > 0 {
            words.push_str(&format!(
                " · {} terminal{}",
                band.terminals(),
                plural(band.terminals())
            ));
        }
        let mut spans = vec![Span::styled(words, Style::default().fg(th.dim))];
        if more > 0 {
            spans.push(Span::styled(
                format!("  ▸ {more} more"),
                Style::default().fg(th.muted),
            ));
        }
        // The VERB, on the band the keys are on: a titled rule reads as
        // a divider, and nothing about a divider says a key acts on it,
        // so the rule under the cursor spells what Tab does at its right
        // end. It names what the row could not show (`Tab: see all 8`)
        // when cards hang past the edge, plain `Tab: expand` when they
        // all fit collapsed, and `Tab: collapse` on the one band already
        // open. A rule too narrow to keep the branch legible beside the
        // words drops them: the `❯` at the left still says which band is
        // selected. Tab is the panels' "next panel" key, which the grid
        // takes for itself (`event_loop::launcher::handle_action`).
        let nothing_to_open = rule.list && !expanded && more == 0;
        if lit && !nothing_to_open {
            let key = super::key_hint(app, crate::keymap::Action::FocusNext);
            let does = if expanded {
                ": collapse".to_string()
            } else if more > 0 {
                format!(": see all {}", band.cards.len())
            } else {
                ": expand".to_string()
            };
            let taken: usize = spans.iter().map(|s| s.width()).sum();
            let with_hint = 3 + taken + 2 + key.chars().count() + does.chars().count() + 4;
            if width >= with_hint + ENTER_HINT_ROOM {
                spans.push(Span::raw("  "));
                spans.push(Span::styled(key, Style::default().fg(th.accent)));
                spans.push(Span::styled(does, Style::default().fg(th.dim)));
            }
        }
        spans
    };
    let right_w: usize = right.iter().map(|s| s.width()).sum();

    // The checkout, as the cards used to paint it: the glyph in its
    // scope color never yields, the branch truncates around it.
    let (glyph, scope) = if band.is_main {
        ("⌂ ", th.root)
    } else {
        ("↳ ", th.worktree)
    };
    let mut branch_style = Style::default().fg(scope);
    if on {
        branch_style = branch_style.add_modifier(Modifier::BOLD);
    }
    // `── ` before, ` ` after the words, ` ` before the right end and
    // ` ──` after it: what the words have to fit between.
    let room = width.saturating_sub(3 + right_w + 5);
    let changes = app
        .worktree_changes(&band.worktree)
        .filter(|n| *n > 0)
        .map(|n| {
            change_labels(
                n,
                app.worktree_lines(&band.worktree),
                band.branch.chars().count(),
                room.saturating_sub(glyph.chars().count()),
            )
        });
    let taken = changes.as_ref().map_or(0, |(files, lines)| {
        files.chars().count()
            + lines.as_ref().map_or(0, |(added, removed)| {
                added.chars().count() + removed.chars().count()
            })
    });
    let branch = truncate(
        &band.branch,
        room.saturating_sub(glyph.chars().count() + taken),
    );
    // The rule's left end: `──` like every other band's, or the CURSOR
    // MARK `❯` on the one the keys are on — the mark a prompt puts before
    // the line that takes the keys, so a rule that looks like a divider
    // reads as a row something can be done to. Two cells either way, so
    // the checkouts stay in one column down the grid.
    let lead = if lit {
        Span::styled(
            "❯ ",
            Style::default().fg(th.accent).add_modifier(Modifier::BOLD),
        )
    } else {
        dash(2)
    };
    let mut left = vec![
        lead,
        Span::raw(" "),
        Span::styled(glyph, Style::default().fg(scope)),
        Span::styled(branch, branch_style),
    ];
    if let Some((files, lines)) = changes {
        left.push(Span::styled(files, Style::default().fg(th.warn)));
        if let Some((added, removed)) = lines {
            left.push(Span::styled(added, Style::default().fg(th.ok)));
            left.push(Span::styled(removed, Style::default().fg(th.err)));
        }
    }
    let mut hits = Vec::new();
    let used: usize = left.iter().map(|s| s.width()).sum();
    // Its pull request, after the checkout, in the PR rows' own colors —
    // as much of the title as the rule has room for, and none of it on a
    // rule too short to say the number.
    if let Some(pr) = &band.pr {
        let spare = room.saturating_sub(used + 2 - 3);
        if spare >= 6 {
            let look = crate::pr_row::look(pr.standing, pr.trouble, th);
            let label = if pr.title.is_empty() {
                format!("#{}", pr.number)
            } else {
                format!("#{} {}", pr.number, pr.title)
            };
            let mut spans = crate::pr_row::spans(
                look,
                &label,
                spare,
                Some((format!(" {}", pr.badge()), look.badge)),
            );
            let hovered = app.hover_crumb == Some(HitTarget::LauncherBandPr(band.worktree.clone()));
            if hovered {
                if let Some(label) = spans.get_mut(1) {
                    label.style = label.style.add_modifier(Modifier::UNDERLINED);
                }
            }
            let pr_w: usize = spans.iter().map(|s| s.width()).sum();
            left.push(Span::raw("  "));
            let x = r.x + u16::try_from(used + 2).unwrap_or(u16::MAX);
            hits.push((
                Rect {
                    x,
                    width: u16::try_from(pr_w)
                        .unwrap_or(u16::MAX)
                        .min(r.width.saturating_sub(x - r.x)),
                    height: 1,
                    ..r
                },
                HitTarget::LauncherBandPr(band.worktree.clone()),
            ));
            left.extend(spans);
        }
    }
    let used: usize = left.iter().map(|s| s.width()).sum();
    let fill = width.saturating_sub(used + 1 + 1 + right_w + 1 + 2);
    let mut spans = left;
    spans.push(Span::raw(" "));
    spans.push(dash(fill));
    spans.push(Span::raw(" "));
    spans.extend(right);
    spans.push(Span::raw(" "));
    spans.push(dash(2));
    Paragraph::new(Line::from(spans)).render(r, buf);
    hits.push((r, HitTarget::LauncherBand(rule.index)));
    hits
}

/// A TERMINAL's card: what a session's card is to a session, and the
/// same size — its name behind the glyph the SESSIONS PANEL gave the row
/// (`▶` for a RUN TERMINAL, `❯` for a plain shell, in `th.ok` while its
/// PTY is alive, with `exited` at the row's right once the shell is
/// gone), what runs in it, then the last lines it printed where a
/// session's card has its prompt ([`terminal_tail_lines`]) — so a glance
/// down the grid says what each shell is up to, and an exited one what it
/// was doing when it went. Framed as a card is, the cursor's in the
/// accent.
fn draw_chip(
    buf: &mut Buffer,
    app: &App,
    area: Rect,
    t: &nebula_core::TerminalTab,
    selected: bool,
    focused: bool,
    th: Theme,
) {
    let border = if selected { th.accent } else { th.edge };
    let mut block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(border));
    if selected && focused {
        block = block.style(Style::default().bg(th.focus_tint));
    }
    let inner = block.inner(area);
    block.render(area, buf);
    let inner = Rect {
        x: inner.x + 1,
        width: inner.width.saturating_sub(2),
        ..inner
    };
    let width = usize::from(inner.width);
    if width == 0 {
        return;
    }
    // HIDE CARD MARKS drops the glyph, and the name starts where it was.
    let glyph = if app.hide_card_marks {
        ""
    } else if t.run_command.is_some() {
        "▶ "
    } else {
        "❯ "
    };
    let glyph_w = glyph.chars().count();
    // `exited` sits where a session card keeps its ago badge, and the
    // name gives it the room, as a session's does.
    let badge = if t.alive { "" } else { EXITED_BADGE };
    let (badge, name_max) = fit_ago(badge.to_string(), width);
    let name = truncate(&t.name, name_max.saturating_sub(glyph_w));
    let mut first = vec![
        Span::styled(
            glyph,
            Style::default().fg(if t.alive { th.ok } else { th.dim }),
        ),
        Span::styled(
            name.clone(),
            Style::default().fg(th.text).add_modifier(Modifier::BOLD),
        ),
    ];
    if !badge.is_empty() {
        let badge = badge.trim_start().to_string();
        let pad = width.saturating_sub(name.chars().count() + glyph_w + badge.chars().count());
        first.push(Span::raw(" ".repeat(pad)));
        first.push(Span::styled(badge, Style::default().fg(th.err)));
    }
    let runs = t.run_command.as_deref().unwrap_or("shell");
    let second = vec![Span::styled(
        truncate(runs, width),
        Style::default().fg(th.dim),
    )];
    let mut lines = vec![first, second];
    lines.resize(crate::launcher::CARD_HEAD_H as usize, Vec::new());
    lines.extend(
        terminal_tail_lines(app, t)
            .iter()
            .take(crate::launcher::PROMPT_LINES)
            .map(|line| {
                vec![Span::styled(
                    truncate(line, width),
                    Style::default().fg(th.muted),
                )]
            }),
    );
    for (i, spans) in lines.into_iter().enumerate() {
        if spans.is_empty() {
            continue;
        }
        let Some(r) = row_rect(inner, i) else { break };
        Paragraph::new(Line::from(spans)).render(r, buf);
    }
}

/// What a dead terminal's card says at its name row's right.
const EXITED_BADGE: &str = " exited";

/// The lines under a terminal card's head: the pane's own screen while the
/// pane is on this terminal — live, on the frame it changes — and
/// otherwise the tail the daemon last answered with
/// ([`App::terminal_tails`]), which a card keeps once its shell is gone.
fn terminal_tail_lines(app: &App, t: &nebula_core::TerminalTab) -> Vec<String> {
    let sref = nebula_core::SessionRef::Terminal(t.id.clone());
    if let Some(term) = app
        .term
        .as_ref()
        .filter(|term| term.sref == sref && term.painted)
    {
        return crate::terminal_tail::screen_tail(
            term.parser.screen(),
            crate::launcher::PROMPT_LINES,
        );
    }
    app.terminal_tails
        .get(&t.id)
        .map(|tail| tail.lines.clone())
        .unwrap_or_default()
}

/// A card's frame color when its status wants one: red for a turn waiting
/// on you, blue for one finished and unread, yellow for one still running
/// — the three the PROJECT TABS count, each in the color its STATUS DOT
/// wears, and no other. A read finish, a fresh or terminated session and a
/// `quiet` card (cold, pending or archived: nothing on it is live) keep
/// the plain edge. The focused card's accent outranks all three.
fn card_edge(a: &nebula_core::Agent, quiet: bool, th: Theme) -> Option<Color> {
    use nebula_core::AgentStatus;
    if quiet {
        return None;
    }
    match a.status {
        AgentStatus::NeedsFeedback => Some(th.err),
        AgentStatus::Finished if a.unseen => Some(th.done),
        AgentStatus::Running => Some(th.warn),
        _ => None,
    }
}

/// What an ARCHIVED card wears where a live one wears its STATUS DOT: the
/// round dot squared off. Two columns wide like the dot it stands in for,
/// so the name behind it starts in the same column on both grids.
const ARCHIVED_MARK: &str = "▪ ";

/// One session's card: its name and how long since it last moved, what
/// it runs on, and the last thing it was asked to do. Where it runs —
/// the checkout, its changes, its pull request — is on its BAND's rule,
/// said once for every card in the checkout. The cursor's card takes an
/// accent border wherever the keys are, and — while the grid has them —
/// the FOCUSED PANEL TINT behind it; every other card's frame answers to
/// its status ([`card_edge`]).
#[allow(clippy::too_many_arguments)]
fn draw_card(
    buf: &mut Buffer,
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
    let archived = a.archived;
    let SessionLook {
        dot,
        quiet,
        name_style,
        ramp,
        ago,
        ago_style,
    } = session_look(app, a, selected, th);
    let quiet_or = |live: Color| if archived { quiet } else { live };
    let border = if selected {
        th.accent
    } else if let Some(edge) = card_edge(a, archived || pending || cold, th) {
        edge
    } else {
        th.edge
    };
    let mut block = Block::default()
        .borders(Borders::ALL)
        // Square corners on an archived card, round on a live one: the one
        // difference between the two grids that survives a terminal with
        // no color at all.
        .border_type(if archived {
            BorderType::Plain
        } else {
            BorderType::Rounded
        })
        .border_style(Style::default().fg(border));
    // The card keys land in wears the same wash the session pane wears
    // when it has them, so one surface on screen is lit and it follows the
    // focus between the grid and the pane.
    if selected && focused {
        block = block.style(Style::default().bg(th.focus_tint));
    }
    let inner = block.inner(area);
    block.render(area, buf);
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

    let (ago, name_max) = fit_ago(ago, width);
    let name = truncate(&a.name, name_max.saturating_sub(2));
    let mut first = vec![dot];
    let used = name.chars().count() + 2;
    first.extend(status_name_spans(name, name_style, ramp, app.sweep_phase()));
    if !ago.is_empty() {
        let ago = ago.trim_start().to_string();
        let pad = width.saturating_sub(used + ago.chars().count());
        first.push(Span::raw(" ".repeat(pad)));
        first.push(Span::styled(ago, ago_style));
    }

    // What it runs on: the harness, the model and the reasoning effort
    // the session was launched with, dim. Where it runs — the checkout,
    // its changes, its pull request — is the BAND's rule over the card,
    // said once for every card in the checkout rather than on each. The
    // project is not on the card either: the grid is one project's,
    // named in the header.
    let harness = runs_on_line(a, cfg);
    let mut second = if harness.is_empty() {
        Vec::new()
    } else {
        vec![Span::styled(
            truncate(&harness, width),
            Style::default().fg(quiet_or(th.dim)),
        )]
    };
    // The issue it was started from, at the row's right end — a link, and
    // the only thing on the card under the pointer that is not the card
    // (`HitTarget::LauncherCardIssue`, placed by `card_issue_rect`).
    if let Some(label) = card_issue_label(app, a) {
        let label_w = label.chars().count();
        if card_issue_rect(area, label_w).is_some() {
            let room = width - label_w - 1;
            let harness = truncate(&harness, room);
            let pad = width - label_w - harness.chars().count();
            let mut style = Style::default().fg(quiet_or(th.accent));
            if app.hover_crumb == Some(HitTarget::LauncherCardIssue(a.id.clone())) {
                style = style.add_modifier(Modifier::UNDERLINED);
            }
            second = vec![
                Span::styled(harness, Style::default().fg(quiet_or(th.dim))),
                Span::raw(" ".repeat(pad)),
                Span::styled(label, style),
            ];
        }
    }

    // The last thing it was asked to do, on the prompt's own `›` (which
    // `hide_card_marks` leaves off), over the card's last rows rather than
    // clipped at the first — unless the `hide_card_prompt` setting leaves
    // those rows blank.
    let mut lines = vec![first, second];
    lines.resize(crate::launcher::CARD_HEAD_H as usize, Vec::new());
    if !app.hide_card_prompt {
        lines.extend(prompt_lines(
            crate::launcher::last_prompt(a).unwrap_or_default(),
            width,
            (!app.hide_card_marks).then(|| quiet_or(th.dim)),
            quiet_or(th.muted),
        ));
    }

    for (i, spans) in lines.into_iter().enumerate() {
        if spans.is_empty() {
            continue;
        }
        let Some(r) = row_rect(inner, i) else { break };
        Paragraph::new(Line::from(spans)).render(r, buf);
    }
}

/// The `#15` an ISSUE SESSION's card shows for the GitHub issue it was
/// started from, while the `card_issue_number` setting is on. None on
/// every other card, and on every card with the setting off.
fn card_issue_label(app: &App, a: &nebula_core::Agent) -> Option<String> {
    if !app.card_issue_number {
        return None;
    }
    a.issue_number().map(|n| format!("#{n}"))
}

/// Where a card drawn whole at `area` puts its issue number `label_w`
/// cells wide: the right end of the row saying what it runs on, inside
/// the border and its cell of air (`draw_card`). None on a card too
/// small to hold it with a cell to spare.
fn card_issue_rect(area: Rect, label_w: usize) -> Option<Rect> {
    let inner = Rect {
        x: area.x + 2,
        y: area.y + 1,
        width: area.width.saturating_sub(4),
        height: area.height.saturating_sub(2),
    };
    let label_w = u16::try_from(label_w).ok()?;
    if inner.height < 2 || inner.width <= label_w {
        return None;
    }
    Some(Rect {
        x: inner.x + inner.width - label_w,
        y: inner.y + 1,
        width: label_w,
        height: 1,
    })
}

/// The click target of the issue number on the session card `card`,
/// placed at `placed` (cut or whole): the chip's cell when that row of
/// the card is on screen, for [`draw_bands`] to register ahead of the
/// card so the chip wins.
fn card_issue_hit(
    app: &App,
    card: &crate::launcher::Card,
    placed: crate::launcher::Placed,
) -> Option<(Rect, HitTarget)> {
    let crate::launcher::Card::Session(row) = card else {
        return None;
    };
    let label = card_issue_label(app, &row.agent)?;
    // The chip on the whole card laid out from row 0, then its row moved
    // to the screen — kept only while the window shows that row.
    let full = Rect {
        y: 0,
        height: placed.rect.height + placed.cut_top + placed.cut_bottom,
        ..placed.rect
    };
    let chip = card_issue_rect(full, label.chars().count())?;
    let row_on_screen = chip.y.checked_sub(placed.cut_top)?;
    (row_on_screen < placed.rect.height).then(|| {
        (
            Rect {
                y: placed.rect.y + row_on_screen,
                ..chip
            },
            HitTarget::LauncherCardIssue(row.agent.id.clone()),
        )
    })
}

/// How a session shows its state wherever the grid draws it — a card's
/// head, a LIST entry — so the two never disagree.
struct SessionLook {
    /// The STATUS DOT, or an archived card's square.
    dot: Span<'static>,
    /// What an archived session's every part is drawn in.
    quiet: Color,
    name_style: Style,
    /// The status sweep across the name, while it animates.
    ramp: Option<[Color; 3]>,
    /// How long since it moved (`done`, `starting`, …), with its lead space.
    ago: String,
    ago_style: Style,
}

/// `a`'s [`SessionLook`], on the cursor's entry when `selected`.
///
/// An ARCHIVED session is the same session put away, and it is drawn as
/// such: nothing on it is live, so nothing on it is colored. Every part of
/// it takes `quiet` — dim on its own, lifted to muted on the one the
/// cursor is on, so it reads a step brighter than its neighbours — its
/// name the plain `muted`, its STATUS DOT gives way to `ARCHIVED_MARK`,
/// and a card's frame squares off (`draw_card`). The colors say it at a
/// glance and the shapes say it again with the colors off, which is the
/// whole grid's answer to "which of the two lists am I looking at" without
/// a word repeated on every card - the header's `n archived sessions` says
/// that once.
fn session_look(app: &App, a: &nebula_core::Agent, selected: bool, th: Theme) -> SessionLook {
    let pending = app.is_placeholder_agent(&a.id);
    let cold = !a.alive && a.cloud_session_id.is_none();
    let archived = a.archived;
    let quiet = if selected { th.muted } else { th.dim };
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
    let ramp = if pending || cold || archived {
        None
    } else {
        sweep_ramp(Some(a.status), app.agent_fresh_done(a), th, app.animations)
    };
    // An archived name is not the loud thing on the screen any more: it
    // gives up the bold with the rest of the card's weight and sits one
    // step above the quiet the rest of the card is in — muted over dim,
    // and text over muted on the card the cursor is on, the same one-step
    // lift `quiet` takes there.
    let name_style = if archived {
        Style::default().fg(if selected { th.text } else { th.muted })
    } else {
        Style::default().fg(th.text).add_modifier(Modifier::BOLD)
    };
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
    let ago_style = if archived {
        Style::default().fg(quiet)
    } else if a.unseen && !pending {
        Style::default().fg(th.done)
    } else {
        Style::default().fg(th.dim)
    };
    SessionLook {
        dot,
        quiet,
        name_style,
        ramp,
        ago,
        ago_style,
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
/// `mark` is the `›`'s color, `None` (HIDE CARD MARKS) to leave it off
/// and start every row in its column.
fn prompt_lines(
    prompt: &str,
    width: usize,
    mark: Option<Color>,
    text: Color,
) -> Vec<Vec<Span<'static>>> {
    const MARK: &str = "› ";
    let indent = if mark.is_some() {
        MARK.chars().count()
    } else {
        0
    };
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
            let lead = match mark {
                Some(mark) if i == 0 => Span::styled(MARK, Style::default().fg(mark)),
                _ => Span::raw(" ".repeat(indent)),
            };
            vec![lead, Span::styled(line_text, Style::default().fg(text))]
        })
        .collect()
}

/// The harness (and model) a session runs on, as the PANE's breadcrumb
/// names it: `claude opus`. A Claude Cloud row says `cloud` — the sandbox
/// is the harness that matters there. The card adds the reasoning effort
/// through [`runs_on_line`].
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

/// What a session's card says it runs on: [`harness_line`] with the
/// reasoning effort the session was launched with after the model —
/// `claude opus high`, `codex gpt-5.5 xhigh`. A session on the CLI's own
/// default effort names none, so the line is the harness line alone
/// rather than one with a hole in it.
fn runs_on_line(a: &nebula_core::Agent, cfg: &mut Option<crate::config::Config>) -> String {
    let mut out = harness_line(a, cfg);
    if let Some(effort) = a.effort.as_deref().filter(|e| !e.is_empty()) {
        out.push(' ');
        out.push_str(effort);
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

/// The PANE's CLOSE BUTTON, at the right end of its header: one click
/// folds the pane away, as `^~` does. A cell of air either side, so the
/// target is wider than the glyph.
const PANE_CLOSE: &str = " × ";
/// The PANE's SIDE BUTTON, just before the CLOSE BUTTON: a picture of the
/// layout one click moves it to — the pane down the right of the cards on
/// a pane along the bottom, the pane along the bottom on one down the
/// right. The click writes Settings → Appearance → **Session pane**, so
/// the move lasts. Aired like the close button.
const PANE_TO_RIGHT: &str = " ◨ ";
const PANE_TO_BOTTOM: &str = " ⬓ ";
/// The PANE's FULL-SCREEN BUTTON, before the SIDE BUTTON: one click gives
/// the session the whole screen, as `^F` does. A full-screen session's
/// header ends in the NORMAL-SIZE BUTTON, which brings it back down.
const PANE_FULL_SCREEN: &str = " ⤢ ";
const PANE_NORMAL_SIZE: &str = " ⤡ ";

/// A header button of `glyph` at `rect`, muted until the pointer is on
/// it and lit in `lit` then, registered as `hit`.
fn header_button(
    f: &mut Frame,
    app: &mut App,
    rect: Rect,
    glyph: &'static str,
    hit: HitTarget,
    lit: ratatui::style::Color,
) {
    let fg = if app.hover_crumb.as_ref() == Some(&hit) {
        lit
    } else {
        app.theme.muted
    };
    f.render_widget(
        Paragraph::new(Span::styled(glyph, Style::default().fg(fg))),
        rect,
    );
    app.hits.push((rect, hit));
}

/// The PANE's own header in the LAUNCHER VIEW: what the pane is reading
/// — the card under the cursor, session or terminal, and the checkout
/// it runs in — then the state tag (`scroll 4`, `exited`) right-aligned
/// before the FULL-SCREEN, SIDE and CLOSE BUTTONS. A rule under it, and the rect the
/// PTY draws in returned, exactly as `ui::terminal_frame` does for the
/// panels' pane. Opening a terminal is `t`'s alone: the grid offers no
/// `+` for it.
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
        // The CLOSE BUTTON holds the right end of the row, the SIDE
        // BUTTON stands before it — unless there is nowhere to move the
        // pane (`App::launcher_pane_move_to`) — and the state tag
        // (`scroll 4`, exited) is right-aligned before those: the title
        // gets what the three leave.
        let side_button = app.launcher_pane_move_to().map(|to| match to {
            crate::launcher::PaneSide::Right => PANE_TO_RIGHT,
            crate::launcher::PaneSide::Bottom => PANE_TO_BOTTOM,
        });
        let side_w = side_button.map_or(0, |b| b.chars().count() as u16);
        let zoom_w = PANE_FULL_SCREEN.chars().count() as u16;
        let close_w = PANE_CLOSE.chars().count() as u16;
        let buttons_w = zoom_w + side_w + close_w;
        let taken = usize::from(buttons_w)
            + right
                .as_ref()
                .map_or(0, |tag| tag.content.chars().count() + 2);
        let tabs = pane_title(app, usize::from(r.width).saturating_sub(taken));
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
            width: r.width.saturating_sub(buttons_w),
            ..r
        };
        if let Some(tag) = right {
            f.render_widget(
                Paragraph::new(Line::from(vec![tag, Span::raw(" ")]))
                    .alignment(ratatui::layout::Alignment::Right),
                tag_r,
            );
        }
        if r.width >= buttons_w {
            let mut x = tag_r.x + tag_r.width;
            let zoom = Rect {
                x,
                width: zoom_w,
                ..r
            };
            header_button(
                f,
                app,
                zoom,
                PANE_FULL_SCREEN,
                HitTarget::LauncherPaneZoom,
                th.accent,
            );
            x += zoom_w;
            if let Some(glyph) = side_button {
                let side = Rect {
                    x,
                    width: side_w,
                    ..r
                };
                // Lit under the pointer as the header's other buttons are:
                // a half-filled square does not say it is one until then.
                let fg = if app.hover_crumb == Some(HitTarget::LauncherPaneSide) {
                    th.accent
                } else {
                    th.muted
                };
                f.render_widget(
                    Paragraph::new(Span::styled(glyph, Style::default().fg(fg))),
                    side,
                );
                app.hits.push((side, HitTarget::LauncherPaneSide));
                x += side_w;
            }
            let close = Rect {
                x,
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

/// The pane's title, laid out left to right in `room` columns: the card
/// the cursor is on — its STATUS DOT and name for a session, the
/// SESSIONS PANEL's `❯`/`▶` and name for a terminal — in the accent,
/// since it is what the keys reach; the checkout it runs in after it,
/// in the SCOPE COLOR the band's rule paints it. With the pane on
/// something the grid no longer lists — a card just archived out from
/// under it — the attached session's own name, muted.
fn pane_title(app: &App, room: usize) -> Vec<PaneTab> {
    let th = app.theme;
    let bands = crate::launcher::bands(app);
    let at = crate::launcher::cursor(app, &bands);
    let card = at.and_then(|at| crate::launcher::card_at(&bands, at));
    let name_room = (room / 2).max(8);
    let lit = Style::default().fg(th.accent).add_modifier(Modifier::BOLD);
    let mut tabs = Vec::new();
    match card {
        Some(crate::launcher::Card::Session(row)) => {
            let a = &row.agent;
            tabs.push(PaneTab::plain(vec![
                status_dot(Some(a.status), a.unseen, th),
                Span::styled(truncate(&a.name, name_room), lit),
            ]));
        }
        Some(crate::launcher::Card::Terminal(t)) => {
            let glyph = if t.run_command.is_some() {
                "▶ "
            } else {
                "❯ "
            };
            tabs.push(PaneTab::plain(vec![
                Span::styled(
                    glyph,
                    Style::default().fg(if t.alive { th.ok } else { th.dim }),
                ),
                Span::styled(truncate(&t.name, name_room), lit),
            ]));
        }
        None => {
            if let Some(name) = app.term.as_ref().and_then(|t| attached_name(app, &t.sref)) {
                tabs.push(PaneTab::plain(vec![Span::styled(
                    truncate(&name, name_room),
                    Style::default().fg(th.muted),
                )]));
            }
        }
    }
    // The checkout everything in the pane belongs to, in the SCOPE COLOR
    // the band's rule paints it — `⌂` on the project's root branch, `↳`
    // on a checkout of its own — so the pane and the grid say the same
    // thing the same way.
    let checkout = at
        .and_then(|at| bands.get(at.band))
        .map(|b| (b.is_main, b.branch.clone()))
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
        if !tabs.is_empty() {
            tabs.push(PaneTab::plain(vec![Span::raw("  ")]));
        }
        tabs.push(PaneTab::plain(vec![
            Span::styled(glyph, Style::default().fg(scope)),
            Span::styled(
                truncate(&branch, (room / 4).max(8)),
                Style::default().fg(scope),
            ),
        ]));
    }
    tabs
}

/// The name of the session `sref` attaches, whichever kind it is.
fn attached_name(app: &App, sref: &nebula_core::SessionRef) -> Option<String> {
    match sref {
        nebula_core::SessionRef::Agent(id) => app
            .tree
            .agents
            .iter()
            .find(|a| &a.id == id)
            .map(|a| a.name.clone()),
        nebula_core::SessionRef::Terminal(id) => app
            .tree
            .terminals
            .iter()
            .find(|t| &t.id == id)
            .map(|t| t.name.clone()),
    }
}

/// The full-screen session's own header, in place of the pane's
/// `TERMINAL · name`: a breadcrumb back to the grid — `‹ sessions` is a
/// button, the session's name the crumb it leads out of — with the
/// harness it runs on and where, right-aligned before the NORMAL-SIZE
/// BUTTON. Returns the rect the PTY
/// draws in, as `terminal_frame` does.
pub(super) fn crumb_frame(f: &mut Frame, app: &mut App, area: Rect) -> Rect {
    let th = app.theme;
    if let Some(r) = row_rect(area, 1) {
        let r = pad_x(r);
        // The NORMAL-SIZE BUTTON holds the right end of the row, where the
        // pane's header keeps its buttons; the rest is laid out in what it
        // leaves.
        let size_w = PANE_NORMAL_SIZE.chars().count() as u16;
        if r.width > size_w {
            let size = Rect {
                x: r.x + r.width - size_w,
                width: size_w,
                ..r
            };
            header_button(
                f,
                app,
                size,
                PANE_NORMAL_SIZE,
                HitTarget::LauncherPaneZoom,
                th.accent,
            );
        }
        let r = Rect {
            width: r.width.saturating_sub(size_w),
            ..r
        };
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
/// one phrase. The checkout's chord is `^T`: it, or a click on the
/// branch, drops the WORKTREE PICKER down from it.
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
/// A PR SESSION's head branch wears no `^T` and is no button: its
/// checkout is the DAEMON's to pick.
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
/// the value and its `^T`, None when it was dropped for room or names a
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
/// `api-server ^P` and `harness claude` never read as one phrase — and the
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
        let mut harness = launch
            .custom
            .as_deref()
            .unwrap_or_else(|| launch.kind.as_str())
            .to_string();
        // A CLAUDE CLOUD box says so on the button that toggles it.
        if launch.cloud {
            harness.push_str(" · cloud");
        }
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
                self.pickable.then_some("^T"),
            ),
            (
                BoxField::Agent,
                "harness",
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
    if launch.cloud {
        head.push("Claude Cloud".into());
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
    } else if width >= 29 {
        " Enter launch · Esc cancel "
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

    /// [`draw_card`] for the tests that draw one card alone.
    fn draw_one(
        f: &mut Frame,
        app: &App,
        area: Rect,
        row: &LauncherRow,
        selected: bool,
        focused: bool,
        th: Theme,
    ) {
        draw_card(
            f.buffer_mut(),
            app,
            area,
            row,
            selected,
            focused,
            th,
            &mut None,
        );
    }

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
            for chord in ["^P", "^T", "^O", "^N", "Tab agent"] {
                assert!(
                    !hint.contains(chord),
                    "{width}: {hint:?} still says {chord}"
                );
            }
        }
        // And the box at its own size names every key it has.
        let full = box_hint(BOX_SIZE.0);
        for key in ["Enter launch", "⇧Enter newline", "⇧Tab preset"] {
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

    /// The details row names all four values and their four chords, and
    /// fits the box it is drawn in.
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
            "^T",
            "harness ",
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
    /// on `harness claude Tab` cannot open the model list. A tier that drops
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
                    BoxField::Worktree => "^T",
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
                issue_url: None,
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

    /// The whole view, drawn with its one band open on the ACCORDION: a
    /// body too short for every card says so on the worktree's rule, and
    /// says it about exactly the cards the grid left off. This is what a
    /// PANE dragged up over the grid looks like — the cards that lost
    /// their room are counted rather than simply gone — and the grid's
    /// own edge says it again where the eye looks for the rest.
    #[test]
    fn a_body_too_short_for_its_cards_counts_them_in_the_header() {
        use crate::launcher::{BAND_RULE_H, BELOW_MARK_H, CARD_H, GAP_Y, HEAD_H};
        let mut app = a_crowded_tree(9);
        select(&mut app, "api");
        app.launcher_expanded = Some(nebula_core::WorktreeId("w0".into()));
        // The cursor on the grid's first card, the way a project opens:
        // everything missing is under the fold.
        let first = crate::launcher::rows(&app)[0].agent.id.0.clone();
        aim_at(&mut app, &first);
        // One column, the `sessions` rule, room for two rows of cards and
        // the row under them the `↓` marker takes: seven of the nine are
        // under the fold.
        let body = Rect::new(
            0,
            0,
            60,
            HEAD_H + BAND_RULE_H + CARD_H * 2 + GAP_Y + BELOW_MARK_H,
        );
        let lines = drawn_lines(&mut app, body);
        let head = &lines[1];
        assert!(head.contains("9 sessions"), "{head:?}");
        assert!(head.contains("↓ 7 hidden"), "{head:?}");

        // And the grid really did draw two of them: the header is not
        // guessing at a number the drawing disagrees with.
        let drawn = lines
            .iter()
            .filter(|line| line.contains("session "))
            .count();
        assert_eq!(drawn, 2, "two cards on screen, seven hidden: {lines:#?}");

        // The EDGE MARKERS: the row under the cards says what is below;
        // nothing is scrolled off the top, so the row of air over them
        // says nothing.
        let last = lines.last().unwrap();
        assert!(last.contains("↓ 7 more below"), "{last:?}");
        let air = &lines[usize::from(HEAD_H) - 1];
        assert!(air.trim().is_empty(), "nothing above: {air:?}");
    }

    /// The screenshot: nine sessions and three terminals, the cursor
    /// walked down onto the first terminal in a body two rows short of
    /// the lot. The window scrolls two rows, and the first row of
    /// sessions — one row too far up to be drawn whole — is drawn to the
    /// edge, its top border off screen and its name row on the window's
    /// first, where a card-sized hole used to stand. The row of air over
    /// the grid says what is above, the row kept under it what is below,
    /// and the header counts both.
    #[test]
    fn a_card_cut_by_the_top_edge_is_drawn_to_the_edge_not_dropped() {
        use crate::launcher::HEAD_H;
        use nebula_core::{TerminalId, TerminalTab, WorktreeId};
        let mut app = a_crowded_tree(9);
        select(&mut app, "api");
        app.launcher_expanded = Some(WorktreeId("w0".into()));
        for i in 1..=3 {
            app.tree.terminals.push(TerminalTab {
                id: TerminalId(format!("t{i}")),
                worktree_id: WorktreeId("w0".into()),
                name: format!("term-{i}"),
                sort_order: i,
                alive: true,
                run_command: None,
            });
        }
        app.sel_session = app
            .visible_session_rows()
            .iter()
            .position(|row| matches!(row, crate::app::SessionRow::Terminal(t) if t.id.0 == "t1"))
            .expect("a row for the terminal");
        let body = Rect::new(0, 0, 100, 58);
        let lines = drawn_lines(&mut app, body);
        assert_eq!(
            app.launcher_scroll, 2,
            "two rows, the least that shows the cursor's card"
        );
        let head = &lines[1];
        assert!(head.contains("↑↓ 4 hidden"), "{head:?}");
        let air = &lines[usize::from(HEAD_H) - 1];
        assert!(air.contains("↑ 2 more above"), "{air:?}");
        let top = &lines[usize::from(HEAD_H)];
        assert!(
            top.contains("session "),
            "the cut card's name row is the window's first: {top:?}"
        );
        assert!(!top.contains('╭'), "its top border is off screen: {top:?}");
        let last = lines.last().unwrap();
        assert!(last.contains("↓ 2 more below"), "{last:?}");
        // The cursor's card is whole: its bottom border on the window's
        // last row, just over the marker.
        let bottom = &lines[lines.len() - 2];
        assert!(bottom.contains('╰'), "{bottom:?}");
        assert!(lines.iter().any(|l| l.contains("term-1")), "{lines:#?}");
    }

    /// A grid with room for every card keeps the rule it has today —
    /// the marker is a thing that appears, not a thing that is always
    /// there saying zero.
    #[test]
    fn a_grid_that_fits_says_nothing_about_hiding() {
        use crate::launcher::{BAND_RULE_H, CARD_H, GAP_Y, HEAD_H};
        let mut app = a_crowded_tree(2);
        select(&mut app, "api");
        let body = Rect::new(0, 0, 60, HEAD_H + BAND_RULE_H + CARD_H * 2 + GAP_Y);
        let lines = drawn_lines(&mut app, body);
        let head = &lines[1];
        assert!(head.contains("2 sessions"), "{head:?}");
        assert!(!head.contains("hidden"), "{head:?}");
    }

    /// A terminal's card is two session cards wide and carries, under its
    /// name and what runs in it, the last lines its shell printed — the
    /// tail the daemon answered with — and the frame notes the card for
    /// the next ask. Once the shell is gone the lines stay and `exited`
    /// sits at the name row's right: the card says what it was doing
    /// when it went.
    #[test]
    fn a_terminal_card_shows_what_its_shell_last_printed() {
        use crate::app::TerminalTail;
        use nebula_core::{TerminalId, TerminalTab, WorktreeId};
        let mut app = a_tree();
        select(&mut app, "api");
        app.tree.terminals.push(TerminalTab {
            id: TerminalId("t1".into()),
            worktree_id: WorktreeId("w0".into()),
            name: "shell-1".into(),
            sort_order: 0,
            alive: true,
            run_command: None,
        });
        app.terminal_tails.insert(
            TerminalId("t1".into()),
            TerminalTail {
                lines: vec!["$ npm test".into(), "ok 12 tests".into()],
                end_seq: 9,
            },
        );
        // The band open: the terminal's card is under the session's
        // rather than off the end of its one-column strip.
        app.launcher_expanded = Some(WorktreeId("w0".into()));
        let body = Rect::new(0, 0, 100, 30);
        let lines = drawn_lines(&mut app, body);
        let row = lines
            .iter()
            .position(|l| l.contains("shell-1"))
            .expect("the terminal's card");
        assert!(
            lines[row + 1].contains("shell"),
            "what runs in it: {:?}",
            lines[row + 1]
        );
        assert!(
            lines[row + 2].contains("$ npm test"),
            "{:?}",
            lines[row + 2]
        );
        assert!(
            lines[row + 3].contains("ok 12 tests"),
            "{:?}",
            lines[row + 3]
        );
        assert!(!lines[row].contains("exited"), "alive: {:?}", lines[row]);
        let widths: Vec<u16> = app
            .hits
            .iter()
            .filter(|(_, h)| matches!(h, HitTarget::LauncherCard(_)))
            .map(|(r, _)| r.width)
            .collect();
        assert_eq!(widths.len(), 2, "the session and the terminal: {widths:?}");
        assert_eq!(
            widths[1],
            widths[0] * 2 + crate::launcher::GAP_X,
            "the terminal's card spans two columns"
        );
        assert_eq!(
            app.tail_cards,
            vec![TerminalId("t1".into())],
            "the frame notes the terminal for the grid's next ask"
        );

        app.tree.terminals[0].alive = false;
        let lines = drawn_lines(&mut app, body);
        let row = lines
            .iter()
            .position(|l| l.contains("shell-1"))
            .expect("the terminal's card");
        assert!(lines[row].contains("exited │"), "{:?}", lines[row]);
        assert!(
            lines[row + 2].contains("$ npm test"),
            "the lines stay: {:?}",
            lines[row + 2]
        );
    }

    /// HIDE CARD MARKS: off, a shell's card opens on `❯`, a RUN
    /// TERMINAL's on `▶` and a session card's prompt on `›`; on, none of
    /// them is drawn and each name or prompt starts in the column its mark
    /// held.
    #[test]
    fn hide_card_marks_drops_the_mark_before_the_name_and_the_prompt() {
        use nebula_core::{TerminalId, TerminalTab, WorktreeId};
        let mut app = a_tree();
        select(&mut app, "api");
        app.tree.agents[0].recent_prompts = vec![nebula_core::PromptEntry {
            text: "fix the login redirect".into(),
            submitted_at: 0,
        }];
        for (id, name, run) in [
            ("t1", "shell-1", None),
            ("t2", "dev-srv", Some("npm run dev")),
        ] {
            app.tree.terminals.push(TerminalTab {
                id: TerminalId(id.into()),
                worktree_id: WorktreeId("w0".into()),
                name: name.into(),
                sort_order: 0,
                alive: true,
                run_command: run.map(Into::into),
            });
        }
        app.launcher_expanded = Some(WorktreeId("w0".into()));
        let body = Rect::new(0, 0, 100, 40);
        let row_of = |lines: &[String], name: &str| -> String {
            lines
                .iter()
                .find(|l| l.contains(name))
                .unwrap_or_else(|| panic!("{name}'s card: {lines:?}"))
                .clone()
        };

        let lines = drawn_lines(&mut app, body);
        assert!(row_of(&lines, "shell-1").contains("❯ shell-1"));
        assert!(row_of(&lines, "dev-srv").contains("▶ dev-srv"));
        let shown_col = row_of(&lines, "shell-1").find("❯").unwrap();
        let prompt = row_of(&lines, "fix the login redirect");
        assert!(prompt.contains("› fix the login redirect"), "{prompt:?}");
        let mark_col = prompt.find('›').unwrap();

        app.hide_card_marks = true;
        let lines = drawn_lines(&mut app, body);
        let shell = row_of(&lines, "shell-1");
        let run = row_of(&lines, "dev-srv");
        assert!(
            !shell.contains('❯') && !run.contains('▶'),
            "{shell:?} {run:?}"
        );
        assert_eq!(shell.find("shell-1"), Some(shown_col), "{shell:?}");
        let prompt = row_of(&lines, "fix the login redirect");
        assert!(!prompt.contains('›'), "{prompt:?}");
        assert_eq!(
            prompt.find("fix the login redirect"),
            Some(mark_col),
            "{prompt:?}"
        );
    }

    /// A session card's second row says what the session runs on — the
    /// harness, the model and the reasoning effort it was launched with,
    /// `claude opus high` — and a session on the CLI's own default effort
    /// ends at the model.
    #[test]
    fn a_session_card_names_its_effort_beside_the_model() {
        let mut app = a_tree();
        select(&mut app, "api");
        app.tree.agents[0].model = Some("opus".into());
        app.tree.agents[0].effort = Some("high".into());
        let body = Rect::new(0, 0, 100, 30);
        let lines = drawn_lines(&mut app, body);
        let row = lines
            .iter()
            .position(|l| l.contains("s0"))
            .expect("the session's card");
        assert!(
            lines[row + 1].contains("claude opus high"),
            "what it runs on: {:?}",
            lines[row + 1]
        );

        app.tree.agents[0].effort = None;
        let lines = drawn_lines(&mut app, body);
        let row = lines
            .iter()
            .position(|l| l.contains("s0"))
            .expect("the session's card");
        assert!(
            lines[row + 1].contains("claude opus") && !lines[row + 1].contains("high"),
            "the CLI's default effort is unnamed: {:?}",
            lines[row + 1]
        );
    }

    /// The `hide_card_prompt` setting leaves the last prompt off the card:
    /// shown by default under the name and harness, gone once it is on,
    /// with the name and what it runs on still drawn.
    #[test]
    fn hide_card_prompt_leaves_the_last_prompt_off_the_card() {
        let mut app = a_tree();
        select(&mut app, "api");
        app.tree.agents[0].recent_prompts = vec![nebula_core::PromptEntry {
            text: "fix the login redirect".into(),
            submitted_at: 0,
        }];
        let body = Rect::new(0, 0, 100, 30);
        let shows = |lines: &[String]| lines.iter().any(|l| l.contains("fix the login redirect"));

        let lines = drawn_lines(&mut app, body);
        assert!(shows(&lines), "shown by default: {lines:#?}");

        app.hide_card_prompt = true;
        let lines = drawn_lines(&mut app, body);
        assert!(!shows(&lines), "hidden: {lines:#?}");
        let row = lines
            .iter()
            .position(|l| l.contains("s0"))
            .expect("the session's card still drawn");
        assert!(lines[row + 1].contains("claude"), "{:?}", lines[row + 1]);
    }

    /// The `card_issue_number` setting puts an ISSUE SESSION's `#15` at
    /// the right end of the row saying what it runs on — only with the
    /// setting on, only on a card started from an issue — and registers
    /// it as a click target ahead of the card.
    #[test]
    fn card_issue_number_shows_the_issue_on_the_runs_on_row() {
        let mut app = a_tree();
        select(&mut app, "api");
        app.tree.agents[0].issue_url = Some("https://github.com/o/r/issues/15".into());
        let body = Rect::new(0, 0, 100, 30);
        let runs_row = |lines: &[String]| {
            let row = lines
                .iter()
                .position(|l| l.contains("s0"))
                .expect("the session's card drawn");
            lines[row + 1].clone()
        };

        let lines = drawn_lines(&mut app, body);
        assert!(!runs_row(&lines).contains("#15"), "off by default");
        assert!(!app
            .hits
            .iter()
            .any(|(_, h)| matches!(h, HitTarget::LauncherCardIssue(_))));

        app.card_issue_number = true;
        app.hits.clear();
        let lines = drawn_lines(&mut app, body);
        let row = runs_row(&lines);
        assert!(row.contains("claude"), "{row:?}");
        assert!(row.contains("#15"), "{row:?}");
        assert!(
            row.find("claude") < row.find("#15"),
            "after what it runs on: {row:?}"
        );
        let id = app.tree.agents[0].id.clone();
        let chip = app
            .hit_rect(&HitTarget::LauncherCardIssue(id.clone()))
            .expect("the number is a click target");
        assert_eq!(
            app.hit_at(chip.x, chip.y),
            Some(HitTarget::LauncherCardIssue(id)),
            "registered ahead of the card, so it wins"
        );
        let drawn: String = lines[usize::from(chip.y)]
            .chars()
            .skip(usize::from(chip.x))
            .take(usize::from(chip.width))
            .collect();
        assert_eq!(drawn, "#15", "the target is the text");
    }

    /// The runs-on line is the harness, then the model, then the effort,
    /// each only while the row carries it; a cloud row names the sandbox
    /// in place of harness and model and keeps the effort, as the pane's
    /// breadcrumb does.
    #[test]
    fn the_runs_on_line_is_harness_model_effort() {
        let mut a = a_tree().tree.agents[0].clone();
        assert_eq!(runs_on_line(&a, &mut None), "claude");
        a.effort = Some("high".into());
        assert_eq!(runs_on_line(&a, &mut None), "claude high");
        a.model = Some("opus".into());
        assert_eq!(runs_on_line(&a, &mut None), "claude opus high");
        a.cloud_session_id = Some("c1".into());
        assert_eq!(runs_on_line(&a, &mut None), "cloud high");
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

    /// `a_tree` with `count` checkouts in `api` instead of one, a session
    /// in each, so the grid can be given more bands than the body has
    /// room for.
    fn a_tree_with_worktrees(count: usize) -> App {
        use nebula_core::{Agent, AgentId, AgentKind, AgentStatus, Worktree, WorktreeId};
        let mut app = a_tree();
        let one = app.tree.agents[0].clone();
        app.tree.worktrees = (0..count)
            .map(|i| Worktree {
                id: WorktreeId(format!("wt{i}")),
                project_id: nebula_core::ProjectId("p0".into()),
                path: format!("/tmp/wt{i}").into(),
                branch: format!("feat-{i}"),
                is_main: false,
                sort_order: i as i64,
            })
            .collect();
        app.tree.agents = (0..count)
            .map(|i| Agent {
                id: AgentId(format!("api{i}")),
                worktree_id: WorktreeId(format!("wt{i}")),
                name: format!("session {i}"),
                status: AgentStatus::Finished,
                kind: AgentKind::Claude,
                sort_order: i as i64,
                ..one.clone()
            })
            .collect();
        app.launcher_expanded = None;
        app
    }

    /// Put the cursor on the band of checkout `id`, at the band level.
    fn aim_at_band(app: &mut App, id: &str) {
        app.sel_worktree = app
            .worktree_row_of(&nebula_core::WorktreeId(id.into()))
            .expect("a row for the checkout");
        app.launcher_expanded = None;
    }

    /// The air under the last whole band says how many more are down
    /// there — in that air, under the bands, not over them — and once
    /// the walk down has brought the last one on screen the cue is the
    /// header's row of air pointing back up at the ones scrolled off.
    #[test]
    fn the_air_under_the_cards_says_how_many_more_are_below() {
        use crate::launcher::{BAND_H, GAP_Y, HEAD_H};
        let mut app = a_tree_with_worktrees(9);
        select(&mut app, "api");
        aim_at_band(&mut app, "wt0");
        // Room for two bands and four rows of air under them.
        let bands_end = HEAD_H + BAND_H * 2 + GAP_Y;
        let body = Rect::new(0, 0, 60, bands_end + 4);
        let lines = drawn_lines(&mut app, body);
        let cue: Vec<usize> = (0..lines.len())
            .filter(|&y| lines[y].contains("more worktree"))
            .collect();
        assert_eq!(cue.len(), 1, "{lines:#?}");
        // Seven bands are past the two whole ones — the third's rule fits
        // in the air, its cards cut, and a band drawn cut still counts.
        assert!(
            lines[cue[0]].contains("↓ 7 more worktrees below"),
            "{:?}",
            lines[cue[0]]
        );
        assert!(cue[0] >= bands_end as usize, "under the bands: {lines:#?}");

        // Walked to the last band, nothing is left below to point at: the
        // seven scrolled off the top are counted on the row of air under
        // the tabs instead.
        aim_at_band(&mut app, "wt8");
        let lines = drawn_lines(&mut app, body);
        assert!(
            lines
                .iter()
                .all(|line| !line.contains("more worktrees below")),
            "{lines:#?}"
        );
        assert!(
            lines[usize::from(HEAD_H) - 1].contains("↑ 7 more worktrees above"),
            "{lines:#?}"
        );
    }

    /// A grid with every band on screen leaves its air empty, however
    /// much of it there is.
    #[test]
    fn a_grid_that_fits_leaves_its_air_empty() {
        use crate::launcher::{BAND_H, HEAD_H};
        let mut app = a_tree_with_worktrees(2);
        select(&mut app, "api");
        aim_at_band(&mut app, "wt0");
        let body = Rect::new(0, 0, 60, HEAD_H + BAND_H * 2 + 6);
        let lines = drawn_lines(&mut app, body);
        assert!(
            lines.iter().all(|line| !line.contains("more worktree")),
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
        let count = HeadCount {
            sessions: count,
            terminals: 0,
        };
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

    /// A lone tab carries its `×` like any other: closing it goes back
    /// to the splash (`event_loop::launcher::close_tab`).
    #[test]
    fn a_lone_tab_has_a_cross() {
        let r = Rect::new(0, 0, 80, 1);
        let mut app = a_tabbed_tree();
        app.launcher_tabs = vec![ProjectId("p0".into())];
        let spans = head_tabs(&mut app, r, 0);
        assert_eq!(row_text(&spans), " +   api × ");
        assert!(head_hits(&app)
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
                !text.contains(" / ") && !text.contains("^T"),
                "{width}: the header carries no checkout crumb: {text:?}"
            );
            let (x, w) = toggle.expect("a worktree launch has a toggle to click");
            assert_eq!(usize::from(x + w), line.width(), "{width}: {text:?}");
        }
    }

    /// The branch in the details row is the WORKTREE PICKER's button: the
    /// columns handed back are exactly the branch and its `^T`, inside the
    /// `worktree` field's own, however the row had to shrink — and none
    /// once the field is dropped for room. A PR SESSION's branch is no
    /// button at all, and wears no `^T`: its checkout is the DAEMON's to
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
                let row: String = text.iter().collect();
                assert!(!row.contains("^T"), "{width}: no button, no ^T");
                assert!(field.is_none(), "{width}: no branch, no field");
                continue;
            };
            seen += 1;
            let under: String = text[usize::from(x)..usize::from(x + w)].iter().collect();
            assert!(
                under.starts_with("(worktree") && under.ends_with(" ^T"),
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
        assert!(!text.contains("^T"), "{text:?}");
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
    /// One checkout's BAND, with one session in it, for the rule tests.
    fn a_band(is_main: bool, branch: &str) -> crate::launcher::Band {
        use nebula_core::{Agent, AgentId, AgentKind, AgentStatus, WorktreeId};
        crate::launcher::Band {
            worktree: WorktreeId("w1".into()),
            branch: branch.into(),
            is_main,
            pr: None,
            cards: vec![crate::launcher::Card::Session(LauncherRow {
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
                    issue_url: None,
                    recent_prompts: Vec::new(),
                },
                project: "nebula".into(),
                branch: branch.into(),
                pr: None,
            })],
        }
    }

    /// A band's rule drawn on one row `width` wide, as the buffer: a
    /// band the cursor is not on, with every card on its row.
    fn rule_row(app: &App, band: &crate::launcher::Band, width: u16) -> ratatui::buffer::Buffer {
        rule_row_as(
            app,
            band,
            width,
            BandRule {
                index: 0,
                on: false,
                lit: false,
                more: 0,
                expanded: false,
                list: false,
            },
        )
    }

    /// [`rule_row`] for a band drawn as `rule` says.
    fn rule_row_as(
        app: &App,
        band: &crate::launcher::Band,
        width: u16,
        rule: BandRule,
    ) -> ratatui::buffer::Buffer {
        let area = Rect::new(0, 0, width, 1);
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(width, 1)).unwrap();
        terminal
            .draw(|f| {
                draw_band_rule(f.buffer_mut(), app, area, band, rule);
            })
            .unwrap();
        terminal.backend().buffer().clone()
    }

    /// The cells of a buffer's row `y` painted in `want`, as text.
    fn painted(buf: &ratatui::buffer::Buffer, y: u16, want: Color) -> String {
        (0..buf.area.width)
            .filter_map(|x| buf.cell((x, y)))
            .filter(|c| c.fg == want)
            .map(|c| c.symbol().to_string())
            .collect()
    }

    /// A buffer's row `y` as text.
    fn row_string(buf: &ratatui::buffer::Buffer, y: u16) -> String {
        (0..buf.area.width)
            .filter_map(|x| buf.cell((x, y)))
            .map(|c| c.symbol().to_string())
            .collect()
    }

    /// A band's rule names its checkout in the SCOPE COLOR — `⌂` root,
    /// `↳` worktree — glyph and branch both, and nothing else on the rule,
    /// so the two scopes sort a screenful of bands before a word is read.
    #[test]
    fn a_band_paints_its_checkout_in_the_scope_color() {
        let app = App::new();
        let th = app.theme;
        let root = rule_row(&app, &a_band(true, "main"), 44);
        let worktree = rule_row(&app, &a_band(false, "feat-x"), 44);
        assert_eq!(painted(&root, 0, th.root), "⌂ main");
        assert_eq!(painted(&worktree, 0, th.worktree), "↳ feat-x");
        // And neither wears the other's color anywhere on that row.
        assert_eq!(painted(&root, 0, th.worktree), "");
        assert_eq!(painted(&worktree, 0, th.root), "");
        // The cards under it say what runs in the checkout, not where.
        let text = row_string(&worktree, 0);
        assert!(text.contains("1 session"), "{text:?}");
    }

    /// The checkout's changed-file count rides right behind the branch on
    /// its band's rule, in the heads-up color, and on a rule too narrow
    /// for the word it keeps the number and drops `files` before the
    /// branch gives up more.
    #[test]
    fn a_band_counts_its_checkouts_changes_behind_the_branch() {
        use nebula_core::WorktreeId;
        let mut app = App::new();
        let th = app.theme;
        app.worktree_changes.insert(
            WorktreeId("w1".into()),
            (Some(3), std::time::Instant::now()),
        );
        let band = a_band(false, "feat-x");

        let buf = rule_row(&app, &band, 44);
        let text = row_string(&buf, 0);
        assert!(text.contains("↳ feat-x +3 files"), "{text:?}");
        assert_eq!(painted(&buf, 0, th.warn).trim(), "+3 files");

        let buf = rule_row(&app, &band, 24);
        let text = row_string(&buf, 0);
        assert_eq!(painted(&buf, 0, th.warn).trim(), "+3", "narrow: {text:?}");
    }

    /// The band the keys are on says so, and what Enter does: its rule
    /// opens on the CURSOR MARK `❯` in the accent where every other
    /// band's opens on `──`, and past its counts it spells the key —
    /// `z: see all 8` when cards hang past the row's edge, `z:
    /// open` when the row showed them all. A titled rule looks like a
    /// divider, and nothing about a divider says a key acts on it. With
    /// the cursor on the band but the keys elsewhere (up on the PROJECT
    /// TABS, down in the pane) the rule is the plain gray one again:
    /// Enter does something else from there.
    #[test]
    fn the_band_the_keys_are_on_marks_itself_and_says_what_enter_does() {
        let app = App::new();
        let th = app.theme;
        let key = crate::ui::key_hint(&app, crate::keymap::Action::FocusNext);
        let mut band = a_band(true, "main");
        let card = band.cards[0].clone();
        band.cards.extend(std::iter::repeat_n(card, 7));
        let lit = |more| BandRule {
            index: 0,
            on: true,
            lit: true,
            more,
            expanded: false,
            list: false,
        };

        let buf = rule_row_as(&app, &band, 96, lit(6));
        let text = row_string(&buf, 0);
        assert!(text.starts_with("❯  ⌂ main"), "{text:?}");
        assert!(
            text.contains(&format!("8 sessions  ▸ 6 more  {key}: see all 8 ──")),
            "{text:?}"
        );
        // The mark and the key in the accent, the checkout still in its
        // own color and nothing else in it.
        let accent = painted(&buf, 0, th.accent);
        assert!(accent.starts_with('❯'), "{accent:?}");
        assert!(accent.contains(&key), "{accent:?}");
        assert_eq!(painted(&buf, 0, th.root), "⌂ main");

        // Every card on the row: nothing to see more of, so plain `expand`.
        let buf = rule_row_as(&app, &band, 96, lit(0));
        let text = row_string(&buf, 0);
        assert!(
            text.contains(&format!("8 sessions  {key}: expand ──")),
            "{text:?}"
        );

        // The cursor's band with the keys elsewhere: bold branch, and
        // that is all — no mark, no hint, the count as on any band.
        let buf = rule_row_as(
            &app,
            &band,
            96,
            BandRule {
                index: 0,
                on: true,
                lit: false,
                more: 6,
                expanded: false,
                list: false,
            },
        );
        let text = row_string(&buf, 0);
        assert!(text.starts_with("── ⌂ main"), "keys elsewhere: {text:?}");
        assert!(text.contains("8 sessions  ▸ 6 more ──"), "{text:?}");
        assert!(!text.contains(&key), "{text:?}");
        assert_eq!(painted(&buf, 0, th.accent), "", "{text:?}");
    }

    /// A rule too narrow to keep the branch legible beside the words
    /// drops the Enter hint before the branch gives way — the `❯` still
    /// says which band is selected, and the count still says what hangs
    /// past the edge. Give it the room and the words are back.
    #[test]
    fn a_narrow_rule_keeps_the_cursor_mark_and_drops_the_enter_hint() {
        let app = App::new();
        let key = crate::ui::key_hint(&app, crate::keymap::Action::FocusNext);
        let mut band = a_band(false, "feat-x");
        let card = band.cards[0].clone();
        band.cards.extend(std::iter::repeat_n(card, 2));
        let rule = BandRule {
            index: 0,
            on: true,
            lit: true,
            more: 2,
            expanded: false,
            list: false,
        };

        let buf = rule_row_as(&app, &band, 40, rule);
        let text = row_string(&buf, 0);
        assert!(text.starts_with("❯  ↳ feat-x"), "{text:?}");
        assert!(text.contains("3 sessions  ▸ 2 more ──"), "{text:?}");
        assert!(!text.contains(&key), "{text:?}");

        let buf = rule_row_as(&app, &band, 72, rule);
        let text = row_string(&buf, 0);
        assert!(
            text.contains(&format!("3 sessions  ▸ 2 more  {key}: see all 3 ──")),
            "{text:?}"
        );
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
            issue_url: None,
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
                pr: None,
            };
            let area = Rect::new(0, 0, 40, crate::launcher::CARD_H);
            let mut terminal =
                ratatui::Terminal::new(ratatui::backend::TestBackend::new(area.width, area.height))
                    .unwrap();
            terminal
                .draw(|f| draw_one(f, &app, area, &row, selected, focused, th))
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
            th.warn
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

        // The cursor wins over every status, and keeps winning with the
        // keys off the grid: the outline still says which card the pane
        // reads while the wash has moved to the pane.
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
            th.accent
        );
        assert_eq!(
            frame(with(AgentStatus::Finished, false), true, false),
            th.accent
        );
        assert_ne!(th.warn, th.accent);
    }

    /// The card keys land in is filled with the FOCUSED PANEL TINT, frame
    /// and all; the cursor's card off the grid (outlined, the wash gone to
    /// the pane) and any other card stay on the terminal's background.
    #[test]
    fn only_the_focused_card_wears_the_focus_tint() {
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
                issue_url: None,
                recent_prompts: Vec::new(),
            },
            project: "nebula".into(),
            branch: "feat-x".into(),
            pr: None,
        };
        let th = Theme::by_name("coral");
        let mut app = App::new();
        app.theme = th;
        let fill = |app: &App, selected: bool, focused: bool| {
            let area = Rect::new(0, 0, 40, crate::launcher::CARD_H);
            let mut terminal =
                ratatui::Terminal::new(ratatui::backend::TestBackend::new(area.width, area.height))
                    .unwrap();
            terminal
                .draw(|f| draw_one(f, app, area, &row, selected, focused, th))
                .unwrap();
            let buf = terminal.backend().buffer().clone();
            let corner = buf.cell((0, 0)).unwrap().bg;
            let inside = buf.cell((20, area.height - 2)).unwrap().bg;
            assert_eq!(corner, inside, "one fill, frame and all");
            inside
        };
        assert_eq!(fill(&app, true, true), th.focus_tint);
        assert_eq!(fill(&app, true, false), Color::Reset);
        assert_eq!(fill(&app, false, true), Color::Reset);
    }

    /// CARD LINE COUNTS: the lines behind the file count always follow it
    /// on the band's rule in the DIFF VIEWER's green and red — `+3 files
    /// +120 -45` — and yield after the word and before the branch as the
    /// rule narrows.
    #[test]
    fn card_line_counts_follow_the_file_count_in_green_and_red() {
        use nebula_core::WorktreeId;
        let mut app = App::new();
        let th = app.theme;
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
        let band = a_band(false, "feat-x");
        let rule = |app: &App, width: u16| {
            let buf = rule_row(app, &band, width);
            (
                row_string(&buf, 0),
                painted(&buf, 0, th.warn),
                painted(&buf, 0, th.ok),
                painted(&buf, 0, th.err),
            )
        };

        let (text, warn, added, removed) = rule(&app, 60);
        assert!(text.contains("↳ feat-x +3 files +120 -45"), "{text:?}");
        assert_eq!(warn.trim(), "+3 files");
        assert_eq!(added.trim(), "+120");
        assert_eq!(removed.trim(), "-45");

        let (text, warn, added, _) = rule(&app, 37);
        assert!(text.contains("↳ feat-x +3 +120 -45"), "{text:?}");
        assert_eq!((warn.trim(), added.trim()), ("+3", "+120"));

        let (text, warn, added, _) = rule(&app, 24);
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
                    issue_url: None,
                    recent_prompts: Vec::new(),
                },
                project: "nebula".into(),
                branch: "main".into(),
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
                .draw(|f| draw_one(f, &app, area, row, false, true, th))
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

        // And the harness row is quiet on both: where the session runs is
        // its band's rule, not the card's.
        assert_eq!(colored(&live, 2, th.dim), "claude");
        assert_eq!(colored(&gone, 2, th.dim), "claude");
        assert_eq!(colored(&live, 2, th.root), "");
    }
}
