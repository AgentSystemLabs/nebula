use super::{harness_line, session_look, truncate};
use crate::app::{App, Focus, HitTarget, SessionRow, WorktreeRow};
use crate::theme::Theme;
use nebula_core::{AgentStatus, ProjectId, WorktreeId};
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

const HEADER_H: u16 = 2;

pub(super) fn draw(f: &mut Frame, app: &mut App, body: Rect) {
    app.chrome.body_area = body;
    let columns = visible_columns(app);
    if columns.is_empty() || body.height <= HEADER_H {
        return;
    }
    let rects = split_columns(body, columns.len());
    for (column, area) in columns.into_iter().zip(rects) {
        draw_column(f, app, column, area);
    }
}

#[derive(Clone, Copy)]
enum Column {
    Projects,
    Worktrees,
    Sessions,
}

impl Column {
    fn focus(self) -> Focus {
        match self {
            Column::Projects => Focus::Projects,
            Column::Worktrees => Focus::Worktrees,
            Column::Sessions => Focus::Sessions,
        }
    }

    fn title(self) -> &'static str {
        match self {
            Column::Projects => "PROJECTS",
            Column::Worktrees => "WORKTREES",
            Column::Sessions => "SESSIONS",
        }
    }
}

fn visible_columns(app: &App) -> Vec<Column> {
    let mut out = Vec::new();
    if !app.launcher.columns_hide_projects {
        out.push(Column::Projects);
    }
    if !app.launcher.columns_hide_worktrees {
        out.push(Column::Worktrees);
    }
    if !app.launcher.columns_hide_sessions {
        out.push(Column::Sessions);
    }
    if out.is_empty() {
        out.push(Column::Sessions);
    }
    out
}

fn split_columns(area: Rect, n: usize) -> Vec<Rect> {
    let constraints = (0..n)
        .map(|_| Constraint::Ratio(1, n as u32))
        .collect::<Vec<_>>();
    Layout::horizontal(constraints)
        .spacing(1)
        .split(area)
        .iter()
        .copied()
        .collect()
}

fn draw_column(f: &mut Frame, app: &mut App, column: Column, area: Rect) {
    let th = app.chrome.theme;
    fill_panel(f.buffer_mut(), area, th);
    draw_header(f, app, column, area);
    let list = Rect {
        y: area.y + HEADER_H,
        height: area.height.saturating_sub(HEADER_H),
        ..area
    };
    if list.height == 0 {
        return;
    }
    app.chrome
        .hits
        .push((area, HitTarget::PanelBg(column.focus())));
    match column {
        Column::Projects => draw_projects(f, app, list),
        Column::Worktrees => draw_worktrees(f, app, list),
        Column::Sessions => draw_sessions(f, app, list),
    }
}

fn fill_panel(buf: &mut Buffer, area: Rect, th: Theme) {
    for y in area.y..area.bottom() {
        for x in area.x..area.right() {
            let cell = &mut buf[(x, y)];
            cell.set_symbol(" ");
            cell.set_style(Style::default());
        }
    }
    if area.width > 0 {
        let x = area.right().saturating_sub(1);
        for y in area.y..area.bottom() {
            buf[(x, y)]
                .set_symbol("│")
                .set_style(Style::default().fg(th.edge));
        }
    }
}

fn draw_header(f: &mut Frame, app: &App, column: Column, area: Rect) {
    let th = app.chrome.theme;
    let focused = app.nav.focus == column.focus();
    let style = Style::default()
        .fg(if focused { th.accent } else { th.muted })
        .add_modifier(Modifier::BOLD);
    let title = Line::from(vec![Span::styled(format!(" {} ", column.title()), style)]);
    f.render_widget(Paragraph::new(title), Rect { height: 1, ..area });
    if area.height > 1 {
        let rule = "─".repeat(area.width as usize);
        f.render_widget(
            Paragraph::new(Line::from(Span::styled(rule, Style::default().fg(th.edge)))),
            Rect {
                y: area.y + 1,
                height: 1,
                ..area
            },
        );
    }
}

fn draw_projects(f: &mut Frame, app: &mut App, area: Rect) {
    let rows = app.project_rows();
    let sel = app.nav.sel_project.min(rows.len().saturating_sub(1));
    let start = settle_scroll(
        &mut app.launcher.columns_projects_scroll,
        sel,
        rows.len(),
        area.height,
    );
    for (screen, i) in visible_indices(start, rows.len(), area.height) {
        let Some(project) = rows.get(i).and_then(|idx| app.tree.projects.get(*idx)) else {
            continue;
        };
        let selected = i == sel;
        let mut spans = Vec::new();
        spans.push(project_dot(app, &project.id));
        spans.push(Span::styled(
            truncate(&project.name, area.width.saturating_sub(12) as usize),
            row_text_style(app, Focus::Projects, selected),
        ));
        let (prs, issues) = app.project_open_counts(&project.id);
        if let Some(prs) = prs.filter(|n| *n > 0) {
            spans.push(Span::styled(
                format!("  {prs} pr{}", if prs == 1 { "" } else { "s" }),
                Style::default().fg(app.chrome.theme.muted),
            ));
        }
        if let Some(issues) = issues.filter(|n| *n > 0) {
            spans.push(Span::styled(
                format!("  {issues} issue{}", if issues == 1 { "" } else { "s" }),
                Style::default().fg(app.chrome.theme.muted),
            ));
        }
        draw_row(
            f,
            app,
            area,
            screen,
            selected,
            HitTarget::ProjectRow(i),
            spans,
        );
    }
}

fn draw_worktrees(f: &mut Frame, app: &mut App, area: Rect) {
    let sel;
    let len;
    let items = {
        let rows = app.worktree_rows();
        len = rows.len();
        sel = app.nav.sel_worktree.min(rows.len().saturating_sub(1));
        rows.into_iter()
            .enumerate()
            .map(|(i, row)| {
                let selected = i == sel;
                match row {
                    WorktreeRow::Checkout(w) | WorktreeRow::PrCheckout(w) => checkout_spans(app, w),
                    WorktreeRow::Pr(pr) => vec![
                        Span::styled("↗ ", Style::default().fg(app.chrome.theme.accent)),
                        Span::styled(
                            truncate(&pr.label(), area.width.saturating_sub(4) as usize),
                            row_text_style(app, Focus::Worktrees, selected),
                        ),
                        Span::styled(
                            format!(" {}", pr.standing().badge()),
                            Style::default().fg(if pr.trouble().is_some() {
                                app.chrome.theme.err
                            } else {
                                app.chrome.theme.muted
                            }),
                        ),
                    ],
                    WorktreeRow::Issue(issue) => vec![
                        Span::styled("# ", Style::default().fg(app.chrome.theme.special)),
                        Span::styled(
                            truncate(&issue.label(), area.width.saturating_sub(4) as usize),
                            row_text_style(app, Focus::Worktrees, selected),
                        ),
                    ],
                }
            })
            .collect::<Vec<_>>()
    };
    let start = settle_scroll(
        &mut app.launcher.columns_worktrees_scroll,
        sel,
        len,
        area.height,
    );
    for (screen, i) in visible_indices(start, len, area.height) {
        let Some(spans) = items.get(i).cloned() else {
            continue;
        };
        let selected = i == sel;
        draw_row(
            f,
            app,
            area,
            screen,
            selected,
            HitTarget::WorktreeRow(i),
            spans,
        );
    }
}

fn draw_sessions(f: &mut Frame, app: &mut App, area: Rect) {
    let rows = app.visible_session_rows();
    let sel = app.nav.sel_session.min(rows.len().saturating_sub(1));
    let start = settle_scroll(
        &mut app.launcher.columns_sessions_scroll,
        sel,
        rows.len(),
        area.height,
    );
    let mut cfg = None;
    for (screen, i) in visible_indices(start, rows.len(), area.height) {
        let Some(row) = rows.get(i) else {
            continue;
        };
        let selected = i == sel;
        let spans = match row {
            SessionRow::Agent(a) => {
                let look = session_look(app, a, selected, app.chrome.theme);
                let mut spans = vec![
                    look.dot,
                    Span::styled(
                        truncate(&a.name, area.width.saturating_sub(18) as usize),
                        look.name_style,
                    ),
                    Span::styled(look.ago, look.ago_style),
                ];
                spans.push(Span::styled(
                    format!(" {}", harness_line(a, &mut cfg)),
                    Style::default().fg(look.quiet),
                ));
                if let Some(prompt) = recent_prompts_for_columns(app, a) {
                    spans.push(Span::styled(
                        format!(" · {}", truncate(&prompt, 30)),
                        Style::default().fg(app.chrome.theme.dim),
                    ));
                }
                spans
            }
            SessionRow::Terminal(t) => vec![
                Span::styled("$ ", Style::default().fg(app.chrome.theme.special)),
                Span::styled(
                    truncate(&t.name, area.width.saturating_sub(4) as usize),
                    row_text_style(app, Focus::Sessions, selected),
                ),
            ],
            SessionRow::Link(link) => vec![
                Span::styled("↗ ", Style::default().fg(app.chrome.theme.accent)),
                Span::styled(
                    truncate(&link.label(), area.width.saturating_sub(4) as usize),
                    row_text_style(app, Focus::Sessions, selected),
                ),
            ],
        };
        draw_row(
            f,
            app,
            area,
            screen,
            selected,
            HitTarget::SessionRow(i),
            spans,
        );
    }
}

fn checkout_spans(app: &App, w: &nebula_core::Worktree) -> Vec<Span<'static>> {
    let color = if w.is_main {
        app.chrome.theme.root
    } else {
        app.chrome.theme.worktree
    };
    vec![
        worktree_dot(app, &w.id),
        Span::styled(
            truncate(&w.branch, 28),
            Style::default().fg(color).add_modifier(Modifier::BOLD),
        ),
    ]
}

fn project_dot(app: &App, id: &ProjectId) -> Span<'static> {
    status_span(crate::app::project_rollup(&app.tree, id), app.chrome.theme)
}

fn worktree_dot(app: &App, id: &WorktreeId) -> Span<'static> {
    status_span(crate::app::worktree_rollup(&app.tree, id), app.chrome.theme)
}

fn status_span(status: Option<AgentStatus>, th: Theme) -> Span<'static> {
    super::status_dot(status, false, th)
}

fn row_text_style(app: &App, focus: Focus, selected: bool) -> Style {
    Style::default()
        .fg(if selected && app.nav.focus == focus {
            app.chrome.theme.text
        } else {
            app.chrome.theme.muted
        })
        .add_modifier(if selected {
            Modifier::BOLD
        } else {
            Modifier::empty()
        })
}

fn draw_row(
    f: &mut Frame,
    app: &mut App,
    area: Rect,
    screen: u16,
    selected: bool,
    hit: HitTarget,
    spans: Vec<Span<'static>>,
) {
    let rect = Rect {
        y: area.y + screen,
        height: 1,
        ..area
    };
    if selected {
        let bg = if app.nav.focus == hit_focus(&hit) {
            app.chrome.theme.sel_bg
        } else {
            app.chrome.theme.sel_bg_dim
        };
        for x in rect.x..rect.right() {
            f.buffer_mut()[(x, rect.y)].set_bg(bg);
        }
    }
    app.chrome.hits.push((rect, hit));
    f.render_widget(Paragraph::new(Line::from(spans)), rect);
}

fn hit_focus(hit: &HitTarget) -> Focus {
    match hit {
        HitTarget::ProjectRow(_) => Focus::Projects,
        HitTarget::WorktreeRow(_) => Focus::Worktrees,
        HitTarget::SessionRow(_) => Focus::Sessions,
        _ => Focus::Sessions,
    }
}

fn settle_scroll(scroll: &mut u16, selected: usize, len: usize, height: u16) -> u16 {
    if len == 0 || height == 0 {
        *scroll = 0;
        return 0;
    }
    let max = len.saturating_sub(height as usize) as u16;
    let selected = selected as u16;
    if selected < *scroll {
        *scroll = selected;
    } else if selected >= scroll.saturating_add(height) {
        *scroll = selected.saturating_sub(height.saturating_sub(1));
    }
    *scroll = (*scroll).min(max);
    *scroll
}

fn visible_indices(start: u16, len: usize, height: u16) -> impl Iterator<Item = (u16, usize)> {
    (0..height).filter_map(move |screen| {
        let i = usize::from(start + screen);
        (i < len).then_some((screen, i))
    })
}

fn recent_prompts_for_columns(app: &App, agent: &nebula_core::Agent) -> Option<String> {
    if !app.launcher.columns_recent_prompts
        || app.launcher.columns_recent_prompts_count == 0
        || app.launcher.show_archived
    {
        return None;
    }
    let count = app.launcher.columns_recent_prompts_count;
    let prompts = agent
        .recent_prompts
        .iter()
        .rev()
        .take(count)
        .map(|p| p.text.as_str())
        .collect::<Vec<_>>();
    (!prompts.is_empty()).then(|| prompts.into_iter().rev().collect::<Vec<_>>().join(" / "))
}
