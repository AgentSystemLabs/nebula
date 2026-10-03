//! The PANELS' drawing (`crate::panels` is their model,
//! `event_loop::panels` their keys): PROJECTS | WORKTREES | SESSIONS down
//! the left of the body, each column a header over a list of rows — a
//! STATUS DOT, the name, how long since it moved and what it runs on —
//! with the cursor's row on the raised selection bar, and the PANE beside
//! them reading the session under the SESSIONS cursor (`draw_terminal`,
//! the very pane the GRID's full-screen session is drawn into). The
//! focused column wears the FOCUS TINT, as the pane does when it has the
//! keys.
//!
//! Every row registers a `HitTarget::PanelsRow` ahead of its column's
//! `PanelBg`, so a click lands on the row and a click on the air under the
//! rows only takes FOCUS.

use super::{
    ago_badge, draw_focus_tint, draw_terminal, fit_ago, key_hint, render_button, row_rect,
    status_color, status_dot, status_name_spans, sweep_ramp, truncate, HelpSection,
    PENDING_SESSION_BADGE,
};
use crate::app::{App, Focus, HitTarget, SessionRow, WorktreeRow};
use crate::keymap::Action;
use crate::panels::{Line as PanelLine, Row};
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
/// The RUNNING badge of a checkout whose RUN COMMAND is up (`r`).
const RUN_BADGE: &str = " ▶";
/// What hangs a checkout under the pull request on its head branch.
const NESTED_INDENT: &str = "└";

/// The PANELS over `body`: the three columns and the pane beside them.
pub(super) fn draw(f: &mut Frame, app: &mut App, body: Rect) {
    app.body_area = body;
    let cols = crate::panels::columns(body);
    draw_projects(f, app, cols.projects);
    draw_worktrees(f, app, cols.worktrees);
    draw_sessions(f, app, cols.sessions);
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
}

/// A column's frame: its rule down the right, a blank row, the title with
/// its count, a blank row. Returns the rect its list fills, one column
/// short of the rule so a row's text never touches it.
fn column(f: &mut Frame, area: Rect, title: &str, count: usize, focused: bool, th: Theme) -> Rect {
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
    }
    Rect {
        y: inner.y + 3,
        height: inner.height.saturating_sub(3),
        width: inner.width.saturating_sub(1),
        ..inner
    }
}

/// A column's list: the `lines` from where the column is scrolled to
/// (`panels::ColumnScroll`, which brings the `cursor`'s row back on screen
/// when it moves), each row drawn by `row` as its spans
/// and the color its selection rail takes. Registers each row, and each
/// header that folds, as a `PanelsRow`, then the whole column as its
/// `PanelBg`.
#[allow(clippy::too_many_arguments)]
fn draw_list(
    f: &mut Frame,
    app: &mut App,
    area: Rect,
    list: Rect,
    lines: &[PanelLine],
    cursor: Option<Row>,
    focus: Focus,
    mut row: impl FnMut(&App, Row, usize) -> (Vec<Span<'static>>, Color),
) {
    let th = app.theme;
    let focused = app.focus == focus;
    let height = usize::from(list.height);
    let top = app.panels_scroll[crate::panels::scroll_slot(focus)].settle(lines, cursor, height);
    for (y, line) in lines.iter().skip(top).take(height).enumerate() {
        let Some(r) = row_rect(list, y) else {
            break;
        };
        match line {
            PanelLine::Blank => {}
            PanelLine::Header { text, fold } => {
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
            PanelLine::Row(at) => {
                let (spans, mark) = row(app, *at, usize::from(r.width));
                let selected = Some(*at) == cursor;
                render_button(f, r, vec![spans], selected, focused, th, 0, mark);
                app.hits.push((r, HitTarget::PanelsRow(*at)));
            }
        }
    }
    app.hits.push((area, HitTarget::PanelBg(focus)));
}

/// The `?` overlay's two columns while the PANELS are up: the keys the
/// columns answer to, in place of the grid's cards and PROJECT TABS. Every
/// chord is the live keymap's, as the grid's help is; the rows that read
/// the selection — checkouts, GitHub, sessions — are the grid's own.
pub(super) fn help_sections() -> (&'static [HelpSection], &'static [HelpSection]) {
    use super::HelpKeys::{Act, Lit};
    use Action::*;
    const LEFT: &[HelpSection] = &[
        (
            "THE COLUMNS",
            &[
                (Act(&[FocusLeft, FocusRight]), "walk the three columns"),
                (Act(&[FocusNext]), "next column, then the pane"),
                (Act(&[MoveDown, MoveUp]), "walk the column's rows"),
                (Act(&[HalfPageDown, HalfPageUp]), "half a page of rows"),
                (Act(&[Activate]), "drill in; session: attach"),
                (Act(&[FollowUp]), "session: follow-up modal"),
                (Act(&[ToggleFullScreen]), "session full-screen / back"),
                (Act(&[ToggleArchived]), "fold the ARCHIVED group"),
                (Act(&[Palette]), "fuzzy jump to anything"),
                (
                    Act(&[NextAttention, PrevAttention, NextProjectTab, PrevProjectTab]),
                    "next/prev session needing you",
                ),
                (
                    Act(&[CloseProjectTab, ProjectDropdown]),
                    "project tabs: none here",
                ),
                (Lit("click"), "select; again: Enter"),
                (Lit("wheel"), "walk the column under it"),
            ],
        ),
        (
            "CHECKOUTS & GITHUB",
            &[
                (Act(&[OpenWorktree]), "open in editor (open command)"),
                (Act(&[GitDiff]), "diff (^r reviewed, ^t tree)"),
                (Act(&[OpenRepo]), "the repo on GitHub"),
                (Act(&[OpenPullRequest, OpenIssue]), "PR / issue on GitHub"),
                (Act(&[RefreshPullRequests]), "reload PRs + issues (GitHub)"),
                (Act(&[Issues]), "issues: prompt, preset, edit"),
                (Act(&[PullRequests]), "pull requests: read / launch"),
                (Act(&[SwitchBranch]), "switch the ⌂ root's branch"),
            ],
        ),
    ];
    const RIGHT: &[HelpSection] = &[
        (
            "SESSIONS",
            &[
                (Act(&[QuickPrompt]), "quick prompt: Enter launches"),
                (Act(&[New]), "new, per column"),
                (Act(&[DuplicateSession]), "quick prompt as this session"),
                (Act(&[AgentPresets]), "agent presets: saved launches"),
                (
                    Act(&[NewTerminal, OpenGhosttyTab]),
                    "terminal: here / in Ghostty",
                ),
                (Act(&[Rename]), "rename the session"),
                (Act(&[Archive, Unarchive]), "archive / unarchive"),
                (Act(&[Delete, DeleteAll]), "delete one / delete all"),
            ],
        ),
        (
            "TERMINAL & MOUSE",
            &[
                (Act(&[Activate]), "lock input"),
                (Act(&[UnlockTerminal]), "unlock, back to the column"),
                (Lit("drag"), "select + copy (2×click: word)"),
                (Lit("⌥click"), "open URL / file under cursor"),
                (Lit("⇧drag"), "select via your terminal"),
                (Lit("right-click"), "row menu: run, restart"),
                (Lit("click outside"), "dismiss any modal (= Esc)"),
            ],
        ),
        (
            "GENERAL",
            &[
                (Lit("⇧ + letter"), "bigger, or outside nebula"),
                (Act(&[Hosts]), "ssh hosts (a: new, d: del)"),
                (Act(&[Settings]), "settings; Hotkeys tab rebinds"),
                (Act(&[Metrics]), "memory: nebula + agents"),
                (Act(&[Quit, Help]), "quit / toggle this help"),
            ],
        ),
    ];
    (LEFT, RIGHT)
}

/// PROJECTS: every project on the machine, the one last worked in first —
/// what the PROJECT TABS are in the GRID, with the cursor's row the lit
/// tab.
fn draw_projects(f: &mut Frame, app: &mut App, area: Rect) {
    let th = app.theme;
    let count = app.project_rows().len();
    let list = column(f, area, "PROJECTS", count, app.focus == Focus::Projects, th);
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
        Focus::Projects,
        |app, at, width| {
            let Row::Project(i) = at else {
                return (Vec::new(), th.accent);
            };
            let Some(p) = rows.get(i).and_then(|i| app.tree.projects.get(*i)) else {
                return (Vec::new(), th.accent);
            };
            let roll = crate::app::project_rollup(&app.tree, &p.id);
            let unseen = crate::app::project_unseen(&app.tree, &p.id);
            let fresh = crate::panels::project_fresh_done(app, &p.id);
            let stamped = crate::app::project_recency(&app.tree, &p.id, now).stamped;
            let badge = unseen_badge(unseen);
            let free = width.saturating_sub(3 + badge.chars().count());
            let (ago, name_max) = fit_ago(ago_badge(stamped), free);
            let mut spans = vec![status_dot(roll, unseen > 0, th)];
            spans.extend(status_name_spans(
                truncate(&p.name, name_max),
                Style::default().add_modifier(Modifier::BOLD),
                sweep_ramp(roll, fresh, th, app.animations),
                app.sweep_phase(),
            ));
            push_dim(&mut spans, ago, th);
            spans.push(Span::styled(badge, Style::default().fg(th.done)));
            (spans, status_color(roll, unseen > 0, th))
        },
    );
}

/// WORKTREES: the selected project's checkouts, then its open pull
/// requests and issues under the headers that fold them
/// (`panels::worktree_lines`).
fn draw_worktrees(f: &mut Frame, app: &mut App, area: Rect) {
    let th = app.theme;
    let count = app.visible_worktrees().len();
    let list = column(
        f,
        area,
        "WORKTREES",
        count,
        app.focus == Focus::Worktrees,
        th,
    );
    // The page Ctrl+d / Ctrl+u halve.
    app.worktrees_view_rows = usize::from(list.height);
    let lines = crate::panels::worktree_lines(app);
    let cursor = Some(Row::Worktree(app.sel_worktree));
    let now = crate::app::now_ms();
    draw_list(
        f,
        app,
        area,
        list,
        &lines,
        cursor,
        Focus::Worktrees,
        |app, at, width| {
            let Row::Worktree(i) = at else {
                return (Vec::new(), th.accent);
            };
            match app.worktree_rows().get(i).copied() {
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
            }
        },
    );
}

/// One checkout's row: the rolled-up STATUS DOT of its sessions, its
/// branch — under a `└` when it is a pull request's — the RUNNING badge
/// while its RUN COMMAND is up, `⌂ root` on the ROOT WORKTREE, how long
/// since anything in it moved, and the finishes in it nobody has read.
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
    let roll = if pending {
        None
    } else {
        crate::app::worktree_rollup(&app.tree, &w.id)
    };
    let unseen = crate::app::worktree_unseen(&app.tree, &w.id);
    let fresh = crate::panels::worktree_fresh_done(app, &w.id);
    let badge = unseen_badge(unseen);
    let indent = if nested { NESTED_INDENT } else { "" };
    let run = app.worktree_running(&w.id).then_some(RUN_BADGE);
    let free = width.saturating_sub(
        3 + badge.chars().count() + indent.chars().count() + run.map_or(0, |r| r.chars().count()),
    );
    let ago = if pending {
        PENDING_WORKTREE_BADGE.to_string()
    } else {
        ago_badge(crate::app::worktree_recency(&app.tree, &w.id, now).stamped)
    };
    let (ago, free) = fit_ago(ago, free);
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
    spans.push(status_dot(roll, unseen > 0, th));
    spans.extend(status_name_spans(
        truncate(&w.branch, max),
        Style::default().fg(th.text),
        sweep_ramp(roll, fresh, th, app.animations),
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
    (spans, status_color(roll, unseen > 0, th))
}

/// SESSIONS: the selected checkout's sessions, terminals, pull request and
/// archived sessions under their headers (`panels::session_lines`). A
/// checkout with nothing in it says which keys start something; a pull
/// request or an issue under the WORKTREES cursor has no sessions, and the
/// column is left empty while the pane reads it.
fn draw_sessions(f: &mut Frame, app: &mut App, area: Rect) {
    let th = app.theme;
    let rows = app.visible_session_rows();
    let count = rows.iter().filter(|r| r.as_link().is_none()).count();
    let list = column(f, area, "SESSIONS", count, app.focus == Focus::Sessions, th);
    app.sessions_view_rows = usize::from(list.height);
    let lines = crate::panels::session_lines(app);
    if lines.is_empty() && app.selected_worktree().is_some() {
        let key = Style::default().fg(th.accent);
        let dim = Style::default().fg(th.dim);
        let hint = Line::from(vec![
            Span::raw(ROW_GUTTER),
            Span::styled(key_hint(app, Action::New), key),
            Span::styled(" agent · ", dim),
            Span::styled(key_hint(app, Action::NewTerminal), key),
            Span::styled(" terminal", dim),
        ]);
        f.render_widget(Paragraph::new(hint), list);
    }
    let cursor = Some(Row::Session(app.sel_session));
    let mut cfg: Option<crate::config::Config> = None;
    draw_list(
        f,
        app,
        area,
        list,
        &lines,
        cursor,
        Focus::Sessions,
        |app, at, width| {
            let Row::Session(i) = at else {
                return (Vec::new(), th.accent);
            };
            match rows.get(i) {
                Some(SessionRow::Agent(a)) => agent_row(app, a, width, &mut cfg),
                Some(SessionRow::Terminal(t)) => terminal_row(t, width, th),
                Some(SessionRow::Link(l)) => {
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
                    let badge = pr.map(|pr| match pr.trouble() {
                        Some(trouble) => (format!(" {}", trouble.badge()), look.badge),
                        None => (format!(" {}", pr.standing().badge()), look.badge),
                    });
                    (
                        crate::pr_row::spans(look, &l.label(), width, badge),
                        look.rail,
                    )
                }
                None => (Vec::new(), th.accent),
            }
        },
    );
}

/// A session's row: its STATUS DOT — gray while no PTY is behind it (the
/// IDLE REAPER took it), hollow while it is a stand-in still being
/// created, `⊘` once archived — its name, how long since it moved, and the
/// harness it runs on while the name has room beside it; an unread finish
/// takes the harness's slot as ` done`, a Claude Cloud row says ` cloud`.
fn agent_row(
    app: &App,
    a: &nebula_core::Agent,
    width: usize,
    cfg: &mut Option<crate::config::Config>,
) -> (Vec<Span<'static>>, Color) {
    let th = app.theme;
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
        // The harness is the one thing on the row it can do without: on
        // a column too narrow for the whole name and its age beside it,
        // it goes first.
        let harness = if a.kind == nebula_core::AgentKind::Custom {
            let cfg = cfg.get_or_insert_with(crate::config::Config::load);
            crate::agent_picker::session_harness_badge_in(a, cfg)
        } else {
            a.kind.as_str().to_string()
        };
        let room = width.saturating_sub(3 + 1 + harness.chars().count() + ago.chars().count());
        if a.name.chars().count() <= room {
            (format!(" {harness}"), th.dim)
        } else {
            (String::new(), th.dim)
        }
    };
    let free = width.saturating_sub(3 + badge.chars().count());
    let (ago, name_max) = fit_ago(ago, free);
    let quiet = a.archived || pending || cold;
    let ramp = if quiet {
        None
    } else {
        sweep_ramp(Some(a.status), app.agent_fresh_done(a), th, app.animations)
    };
    let name_style = Style::default().fg(if a.archived { th.dim } else { th.text });
    let mut spans = vec![dot];
    spans.extend(status_name_spans(
        truncate(&a.name, name_max),
        name_style,
        ramp,
        app.sweep_phase(),
    ));
    push_dim(&mut spans, ago, th);
    spans.push(Span::styled(badge, Style::default().fg(badge_color)));
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
        Span::styled(name, Style::default().fg(th.text)),
    ];
    if let Some(command) = t.run_command.as_deref().filter(|_| room > 1) {
        spans.push(Span::styled(
            format!(" {}", truncate(command, room)),
            Style::default().fg(th.dim),
        ));
    }
    (spans, th.accent)
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
