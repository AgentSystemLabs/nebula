//! The NESTED layout's drawing (Settings → Appearance → **Worktree
//! layout** → `nested`; `crate::launcher::nested_panel_layout` is its
//! geometry): one thread per worktree, the one that moved last on top,
//! packed with no blank row between them. The earliest session in the
//! worktree is the root row — its title is the thread's title — and every
//! later prompt and terminal is a child row under it, one line each. The
//! root checkout is not one thread: each of its sessions and terminals is
//! a root row of its own, with no children. No
//! worktree header, no card border, no rail: what a row runs on, where,
//! and its pull request are on the DETAIL STRIP pinned under the list
//! ([`draw_strip`]).

use super::{draw_panel_edge_marks, session_look, truncate, EMPTY_BAND_DELETE, EMPTY_BAND_HINT};
use crate::app::{App, Focus, HitTarget};
use crate::keymap::Action;
use crate::launcher::{Band, BandsLayout, Card, CardRef, PanelLayout, RowPr, NESTED_ROW_H};
use crate::pull_request::Standing;
use crate::theme::{nested as gray, Theme};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Widget};
use ratatui::Frame;

/// The fold caret at the left of a thread's root: open, and folded.
const CARET_OPEN: &str = "▾";
const CARET_FOLDED: &str = "▸";
/// The tree under a root: every child but the last, and the last.
const BRANCH: &str = "├";
const LAST_BRANCH: &str = "└";
/// How the DETAIL STRIP names the project's ROOT checkout, its branch
/// after it.
const ROOT_TAG: &str = "⌂ root";
/// The scope mark between a root's caret and its title: the project's ROOT
/// checkout, and a worktree.
const ROOT_MARK: &str = "⌂";
const WORKTREE_MARK: &str = "↳";

/// The PR column, right-aligned: `#141`, on a root whose checkout has an
/// open pull request, blank on every other row.
const PR_W: usize = 5;
/// The age column, right-aligned: `6m`, `now`.
const AGE_W: usize = 4;
/// Fewest letters a title keeps before the columns right of it give way.
const MIN_TITLE: usize = 4;

/// Rows the DETAIL STRIP takes, its border included: four lines of
/// detail in a box. Fixed, so the list over it never resizes as the
/// cursor moves.
pub(super) const STRIP_H: u16 = 6;
/// The column the strip's values start in, past their labels.
const LABEL_W: usize = 9;

/// [`truncate`] for a title: cut to `max` cells with the last spent on
/// `…`, and no space left dangling in front of it.
fn clip(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let Some(keep) = max.checked_sub(1) else {
        return String::new();
    };
    let kept: String = s.chars().take(keep).collect();
    format!("{}…", kept.trim_end())
}

/// How long since `at`, the way a thread row says it: `now`, `6m`, `1h`,
/// `2d`. Empty when the row has no stamp.
fn thread_age(at: i64) -> String {
    if at <= 0 {
        return String::new();
    }
    let delta = crate::app::now_ms().saturating_sub(at);
    match delta / 1000 {
        s if s < 60 => "now".into(),
        s if s < 3_600 => format!("{}m", s / 60),
        s if s < 86_400 => format!("{}h", s / 3_600),
        s => format!("{}d", s / 86_400),
    }
}

/// `n` children, the way a collapsed root says it: `1 sub`, `3 subs`.
fn sub_label(n: usize) -> String {
    if n == 1 {
        "1 sub".into()
    } else {
        format!("{n} subs")
    }
}

/// The pull request a thread's PR column shows: one still open, draft or
/// not. A merged or closed one is the DETAIL STRIP's to mention.
fn open_pr(band: &Band) -> Option<&RowPr> {
    band.pr
        .as_ref()
        .filter(|pr| matches!(pr.standing, Standing::Open | Standing::Draft))
}

/// A pull request's number, as a link.
fn pr_link(pr: &RowPr) -> Span<'static> {
    Span::styled(
        format!("#{}", pr.number),
        Style::default()
            .fg(gray::LINK)
            .add_modifier(Modifier::UNDERLINED),
    )
}

/// The GRID in the NESTED layout: one thread after another, top to
/// bottom, no blank row between them. Each thread is its root row — the
/// first prompt in the worktree — and, unless it is folded, a child row
/// per later prompt and terminal. Every row is one line. The whole panel scrolls by rows as the other
/// layouts' does.
///
/// A card is `HitTarget::LauncherCard`. The fold caret, the first cell of
/// a root that has children, is `HitTarget::LauncherBandFold`, and the
/// `#42` in a root's PR column `HitTarget::LauncherThreadPr`, each
/// registered ahead of the row so a click on it does its own thing rather
/// than selecting.
pub(super) fn draw_bands(
    f: &mut Frame,
    app: &mut App,
    g: &BandsLayout,
    panel: &PanelLayout,
    bands: &[Band],
    cursor: Option<usize>,
    scroll: u16,
) {
    let th = app.chrome.theme;
    let window = panel.window();
    let keys = app.nav.focus != Focus::Terminal && app.launcher.launcher_tab_cursor.is_none();
    for (index, band) in bands.iter().enumerate() {
        let pb = &panel.bands[index];
        let order = crate::launcher::thread_order(band);
        let on = cursor == Some(index);
        let at = on
            .then(|| crate::launcher::card_cursor(app, band))
            .flatten();
        let folded = app.band_folded(band);
        if order.is_empty() {
            let row = Rect {
                y: pb.rule_y,
                height: NESTED_ROW_H,
                ..g.area
            };
            if let Some(placed) = crate::launcher::place(window, scroll, row) {
                draw_empty(f.buffer_mut(), app, placed.rect, band, on && keys);
                app.chrome
                    .hits
                    .push((placed.rect, HitTarget::LauncherBand(index)));
            }
            continue;
        }
        let children = order.len() - 1;
        let merged = app.worktree_wears_merge(&band.worktree);
        let merge_sweeps = merged && app.chrome.animations && app.merge_is_fresh(&band.worktree);
        for (n, &card_index) in order.iter().enumerate() {
            let Some(placed) = pb
                .cell(card_index)
                .and_then(|cell| crate::launcher::place(window, scroll, cell))
            else {
                continue;
            };
            let child = n > 0;
            if !child && children > 0 {
                app.chrome.hits.push((
                    Rect {
                        width: placed.rect.width.min(1),
                        ..placed.rect
                    },
                    HitTarget::LauncherBandFold(index),
                ));
            }
            let row = Row {
                band_index: index,
                child,
                last: n + 1 == order.len(),
                children,
                folded,
                selected: at == Some(card_index),
                keys,
                root: band.is_main,
                pr: if child { None } else { open_pr(band) },
                merged: !child && merged,
                merge_sweeps: !child && merge_sweeps,
            };
            let pr_hit = draw_row(
                f.buffer_mut(),
                app,
                placed.rect,
                &row,
                &band.cards[card_index],
            );
            if let Some(rect) = pr_hit {
                app.chrome
                    .hits
                    .push((rect, HitTarget::LauncherThreadPr(band.worktree.clone())));
            }
            app.chrome.hits.push((
                placed.rect,
                HitTarget::LauncherCard(CardRef {
                    band: index,
                    card: card_index,
                }),
            ));
        }
    }
    draw_panel_edge_marks(f, panel, scroll, th);
    app.chrome
        .hits
        .push((g.area, HitTarget::PanelBg(Focus::Sessions)));
}

/// An empty worktree (**Show all worktrees**): one line, in the title
/// column the roots use, saying what can be done there.
fn draw_empty(buf: &mut Buffer, app: &App, r: Rect, band: &Band, lit: bool) {
    let th = app.chrome.theme;
    let mut text = String::from(EMPTY_BAND_HINT);
    if !band.is_main {
        text.push_str(EMPTY_BAND_DELETE);
    }
    let fg = if lit { th.accent } else { gray::DIM };
    // Four cells: the caret column and the scope mark's, each with its
    // space, so the words start where a root's title does.
    let shown = format!(
        "    {}",
        truncate(&text, usize::from(r.width).saturating_sub(4))
    );
    Paragraph::new(Span::styled(
        truncate(&shown, usize::from(r.width)),
        Style::default().fg(fg),
    ))
    .render(r, buf);
}

/// Which row [`draw_row`] is drawing.
struct Row<'a> {
    band_index: usize,
    /// A child of the thread, indented under the root.
    child: bool,
    /// The last row of the thread: its connector is `└`.
    last: bool,
    /// How many rows fold under the root.
    children: usize,
    /// The thread is folded to its root.
    folded: bool,
    /// This row is the one under the cursor.
    selected: bool,
    /// The keys are on the grid, so the cursor's row wears the accent.
    keys: bool,
    /// The thread is in the project's ROOT checkout: its root row's scope
    /// mark is `⌂`, not a worktree's `↳`.
    root: bool,
    /// A root's open pull request, for its PR column.
    pr: Option<&'a RowPr>,
    /// A root whose checkout wears its merged pull request
    /// (`App::worktree_wears_merge`): its title is purple.
    merged: bool,
    /// The merge was seen to land moments ago: the title rides the
    /// merged sweep instead of holding still.
    merge_sweeps: bool,
}

/// The columns right of a row's title: the PR column and the age, each
/// fixed and right-aligned so both line up down the list, a space before
/// each. A row too narrow for both keeps the age; narrower still, neither.
#[derive(Clone, Copy)]
enum Columns {
    Both,
    Age,
    None,
}

impl Columns {
    fn fit(width: usize, lead: usize) -> Self {
        if width >= lead + MIN_TITLE + 1 + PR_W + 1 + AGE_W {
            Columns::Both
        } else if width >= lead + MIN_TITLE + 1 + AGE_W {
            Columns::Age
        } else {
            Columns::None
        }
    }

    fn width(self) -> usize {
        match self {
            Columns::Both => 1 + PR_W + 1 + AGE_W,
            Columns::Age => 1 + AGE_W,
            Columns::None => 0,
        }
    }
}

/// `text` right-aligned in a column `w` wide.
fn right_aligned(text: &str, w: usize) -> String {
    format!("{text:>w$}")
}

/// Fill a selected row: the accent taken nearly to black across it, and
/// the one-cell accent bar in the padding left of it, so no column moves.
fn fill_selected(buf: &mut Buffer, r: Rect, th: Theme) {
    let fill = super::dim_toward_black(th.accent, gray::SELECTED_FILL);
    buf.set_style(r, Style::default().bg(fill));
    if r.x > 0 {
        if let Some(bar) = buf.cell_mut((r.x - 1, r.y)) {
            bar.set_symbol(" ");
            bar.set_style(Style::default().fg(th.accent).bg(th.accent));
        }
    }
}

/// One thread row. A root is `▾ ↳ Title`, the caret a blank space when
/// the thread has no children, the scope mark `⌂` in the root checkout
/// and `↳` in a worktree, and `1 sub` after the title only while those
/// children are folded. No status dot: the title is in the status color.
/// A child is `    ├ Title`, the connector sitting under the root's title.
/// A terminal keeps its `▶`/`❯` mark before its title.
/// Right of the title, the PR column — a root's open `#42`, as a link —
/// and the dim age, both fixed width and right-aligned on every row.
/// Returns where the `#42` landed, for its hit.
fn draw_row(buf: &mut Buffer, app: &App, r: Rect, row: &Row, card: &Card) -> Option<Rect> {
    let th = app.chrome.theme;
    let width = usize::from(r.width);
    if width == 0 || r.height == 0 {
        return None;
    }
    let RowParts {
        mut mark,
        age,
        color,
        title,
        mut ramp,
    } = row_parts(app, card, th);
    if row.merged {
        if let Some(mark) = mark.as_mut() {
            mark.style = mark.style.fg(th.merged);
        }
        ramp = row.merge_sweeps.then_some(th.merged_sweep);
    }
    let mark_w = mark.as_ref().map_or(0, Span::width);
    let lead = if row.child {
        4 + 1 + 1 + mark_w
    } else {
        1 + 1 + 1 + 1 + mark_w
    };
    let columns = Columns::fit(width, lead);
    let room = width.saturating_sub(lead + columns.width());
    let mut count = if !row.child && row.folded && row.children > 0 {
        sub_label(row.children)
    } else {
        String::new()
    };
    // The title gives up letters first, down to a few; then the count.
    let extra = |count: &str| {
        if count.is_empty() {
            0
        } else {
            1 + count.chars().count()
        }
    };
    if room < MIN_TITLE + extra(&count) {
        count.clear();
    }
    let title = clip(&title, room.saturating_sub(extra(&count)));
    let title_style = title_style(th, row, color);
    let dim = Style::default().fg(gray::DIM);

    let mut spans = Vec::new();
    if row.child {
        spans.push(Span::raw("    "));
        let connector = if row.last { LAST_BRANCH } else { BRANCH };
        spans.push(Span::styled(connector, dim));
        spans.push(Span::raw(" "));
    } else {
        let caret = if row.children == 0 {
            " "
        } else if row.folded {
            CARET_FOLDED
        } else {
            CARET_OPEN
        };
        let fold = HitTarget::LauncherBandFold(row.band_index);
        let caret_fg = if row.selected && row.keys {
            th.accent
        } else if app.launcher.hover_crumb.as_ref() == Some(&fold) {
            gray::BRIGHT
        } else {
            gray::DIM
        };
        spans.push(Span::styled(caret, Style::default().fg(caret_fg)));
        spans.push(Span::raw(" "));
        let (mark, mark_fg) = if row.root {
            (ROOT_MARK, th.root)
        } else {
            (WORKTREE_MARK, th.worktree)
        };
        spans.push(Span::styled(mark, Style::default().fg(mark_fg)));
        spans.push(Span::raw(" "));
    }
    spans.extend(mark);
    let mut used = lead + title.chars().count();
    spans.extend(super::status_name_spans(
        title,
        title_style,
        ramp,
        app.sweep_phase(),
    ));
    if !count.is_empty() {
        spans.push(Span::raw(" "));
        spans.push(Span::styled(count.clone(), dim));
        used += 1 + count.chars().count();
    }
    let mut pr_hit = None;
    match columns {
        Columns::None => {}
        Columns::Age | Columns::Both => {
            spans.push(Span::raw(" ".repeat(
                width.saturating_sub(columns.width()).saturating_sub(used),
            )));
            if let Columns::Both = columns {
                spans.push(Span::raw(" "));
                match row.pr {
                    Some(pr) => {
                        let link = pr_link(pr);
                        let w = link.width();
                        spans.push(Span::raw(" ".repeat(PR_W.saturating_sub(w))));
                        let x = r.x + (width - columns.width() + 1 + PR_W.saturating_sub(w)) as u16;
                        pr_hit = Some(Rect {
                            x,
                            y: r.y,
                            width: (w as u16).min(r.right().saturating_sub(x)),
                            height: 1,
                        });
                        spans.push(link);
                    }
                    None => spans.push(Span::raw(" ".repeat(PR_W))),
                }
            }
            spans.push(Span::raw(" "));
            spans.push(Span::styled(right_aligned(&age, AGE_W), dim));
        }
    }
    Paragraph::new(Line::from(spans)).render(Rect { height: 1, ..r }, buf);
    if row.selected {
        fill_selected(buf, r, th);
    }
    pr_hit
}

/// What a row shows, before it is fitted to the width.
struct RowParts {
    /// A terminal's `▶`/`❯` before its title; a session has none.
    mark: Option<Span<'static>>,
    age: String,
    /// The title's color: the status color the cards' STATUS DOT wears.
    color: Color,
    title: String,
    /// The status sweep across the title, while it animates — the same
    /// ramp the cards and the list sweep a session's name with.
    ramp: Option<[Color; 3]>,
}

fn row_parts(app: &App, card: &Card, th: Theme) -> RowParts {
    match card {
        Card::Session(row) => {
            let look = session_look(app, &row.agent, false, th);
            let color = match look.dot.style.fg {
                Some(c) if c != th.dim => c,
                _ => gray::DIM,
            };
            let age = if row.agent.archived {
                thread_age(row.agent.archived_at)
            } else {
                thread_age(row.agent.status_changed_at)
            };
            RowParts {
                mark: None,
                age,
                color,
                title: row.agent.name.clone(),
                ramp: look.ramp,
            }
        }
        Card::Terminal(t) => {
            let color = if t.alive { th.ok } else { gray::DIM };
            // As the cards and the list draw it: `▶` running a command,
            // `❯` a shell.
            let mark = if t.run_command.is_some() {
                "▶ "
            } else {
                "❯ "
            };
            RowParts {
                mark: Some(Span::styled(mark, Style::default().fg(color))),
                age: String::new(),
                color,
                title: t.name.clone(),
                ramp: None,
            }
        }
    }
}

/// Title colour: a merged root is purple, any other row its status color.
/// Only the selected title is bold.
fn title_style(th: Theme, row: &Row, color: Color) -> Style {
    let fg = if row.merged { th.merged } else { color };
    let style = Style::default().fg(fg);
    if row.selected {
        style.add_modifier(Modifier::BOLD)
    } else {
        style
    }
}

// ---- the DETAIL STRIP ----

/// What a pull request's state reads as on the strip, and in what.
fn pr_state(pr: &RowPr, th: Theme) -> (&'static str, Color) {
    match pr.standing {
        Standing::Open => ("open", th.ok),
        Standing::Draft => ("draft", gray::DIM),
        Standing::Merged => ("merged", th.merged),
        Standing::Closed => ("closed", th.err),
    }
}

/// What a session runs on, the strip's way: `claude · opus · high`, the
/// parts it was launched with and none it was not. A Claude Cloud row
/// says `cloud`.
fn runs_on(a: &nebula_core::Agent, cfg: &mut Option<crate::config::Config>) -> String {
    if a.cloud_session_id.is_some() {
        return "cloud".into();
    }
    let harness = if a.kind == nebula_core::AgentKind::Custom {
        let cfg = cfg.get_or_insert_with(crate::config::Config::load);
        crate::agent_picker::session_harness_badge_in(a, cfg)
    } else {
        a.kind.as_str().to_string()
    };
    let mut parts = vec![harness];
    for part in [a.model.as_deref(), a.effort.as_deref()]
        .into_iter()
        .flatten()
    {
        if !part.is_empty() {
            parts.push(part.to_string());
        }
    }
    parts.join(" · ")
}

/// One of the strip's lines: `left` from the start, `right` against the
/// end, the left's last span clipped so the two never touch.
fn strip_line(
    mut left: Vec<Span<'static>>,
    right: Vec<Span<'static>>,
    width: usize,
) -> Line<'static> {
    let right_w: usize = right.iter().map(|s| s.width()).sum();
    let right = if right_w + 1 < width {
        right
    } else {
        Vec::new()
    };
    let right_w: usize = right.iter().map(|s| s.width()).sum();
    let room = width.saturating_sub(right_w + usize::from(right_w > 0));
    let mut used = 0;
    for span in &mut left {
        let w = span.width();
        if used + w > room {
            span.content = clip(&span.content, room.saturating_sub(used)).into();
        }
        used += span.width();
    }
    let mut spans = left;
    if !right.is_empty() {
        spans.push(Span::raw(" ".repeat(width.saturating_sub(used + right_w))));
        spans.extend(right);
    }
    Line::from(spans)
}

/// A strip line's dim label, padded to the values' column.
fn label(word: &str) -> Span<'static> {
    Span::styled(format!("{word:<LABEL_W$}"), Style::default().fg(gray::DIM))
}

/// The DETAIL STRIP, pinned under the list and over the key bar: the row
/// under the cursor, in four lines. Its title in the accent with the
/// checkout's lines added and removed at the right; what it runs on; the
/// checkout; and the checkout's pull request with the key that opens it in
/// the PULL REQUESTS MODAL. A child reads its own title and harness and its
/// thread's checkout. It never scrolls with the list, and keeps its height with nothing selected.
pub(super) fn draw_strip(
    f: &mut Frame,
    app: &mut App,
    area: Rect,
    bands: &[Band],
    cursor: Option<usize>,
) {
    let th = app.chrome.theme;
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(th.edge));
    let inner = block.inner(area);
    f.render_widget(block, area);
    let inner = Rect {
        x: inner.x + 1,
        width: inner.width.saturating_sub(2),
        ..inner
    };
    let width = usize::from(inner.width);
    let dim = Style::default().fg(gray::DIM);
    let Some(band) = cursor.and_then(|i| bands.get(i)) else {
        f.render_widget(
            Paragraph::new(Span::styled("nothing selected — j/k picks a row", dim)),
            inner,
        );
        app.chrome
            .hits
            .push((area, HitTarget::PanelBg(Focus::Sessions)));
        return;
    };
    let card = crate::launcher::card_cursor(app, band).and_then(|i| band.cards.get(i));
    let mut cfg = None;
    let (title, runs) = match card {
        Some(Card::Session(row)) => (row.agent.name.clone(), runs_on(&row.agent, &mut cfg)),
        Some(Card::Terminal(t)) => (t.name.clone(), "terminal".to_string()),
        None => (band.branch.clone(), String::new()),
    };
    let diff = app
        .worktree_lines(&band.worktree)
        .filter(|l| l.added + l.removed > 0)
        .map(|l| {
            vec![
                Span::styled(format!("+{}", l.added), Style::default().fg(th.ok)),
                Span::raw(" "),
                Span::styled(format!("−{}", l.removed), Style::default().fg(th.err)),
            ]
        })
        .unwrap_or_default();
    let scope = if band.is_main { th.root } else { th.worktree };
    let branch = Span::styled(band.branch.clone(), Style::default().fg(scope));
    let worktree = if band.is_main {
        vec![
            label("worktree"),
            Span::styled(ROOT_TAG, Style::default().fg(th.root)),
            Span::raw(" "),
            branch,
        ]
    } else {
        vec![label("worktree"), branch]
    };
    let mut pr_hit = None;
    let (pr_left, pr_right) = match &band.pr {
        Some(pr) => {
            let link = pr_link(pr);
            pr_hit = Some(Rect {
                x: inner.x + LABEL_W as u16,
                y: inner.y + 3,
                width: (link.width() as u16).min(inner.width.saturating_sub(LABEL_W as u16)),
                height: 1,
            });
            let (state, color) = pr_state(pr, th);
            let mut left = vec![
                label("pr"),
                link,
                Span::raw(" "),
                Span::styled(state, Style::default().fg(color)),
            ];
            if let Some(trouble) = pr.trouble {
                left.push(Span::styled(
                    format!(" · {}", trouble.badge()),
                    Style::default().fg(th.err),
                ));
            }
            let key = super::super::key_hint(app, Action::PullRequests);
            let right = vec![
                Span::styled(key, Style::default().fg(th.accent)),
                Span::styled(" open PR", dim),
            ];
            (left, right)
        }
        None => (vec![label("pr"), Span::styled("none", dim)], Vec::new()),
    };
    let lines = vec![
        strip_line(
            vec![Span::styled(
                title,
                Style::default().fg(th.accent).add_modifier(Modifier::BOLD),
            )],
            diff,
            width,
        ),
        strip_line(
            vec![
                label("agent"),
                Span::styled(runs, Style::default().fg(th.text)),
            ],
            Vec::new(),
            width,
        ),
        strip_line(worktree, Vec::new(), width),
        strip_line(pr_left, pr_right, width),
    ];
    f.render_widget(Paragraph::new(lines), inner);
    if let Some(rect) = pr_hit.filter(|r| r.width > 0 && inner.height > 3) {
        app.chrome
            .hits
            .push((rect, HitTarget::LauncherThreadPr(band.worktree.clone())));
    }
    app.chrome
        .hits
        .push((area, HitTarget::PanelBg(Focus::Sessions)));
}
