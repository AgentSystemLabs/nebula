//! View layer: draws the three panels + terminal pane + footer, and records
//! hit regions for mouse interaction.

use crate::app::{App, ConnState, Focus, HitTarget, Overlay, ProjectRow};
use crate::git_diff::{classify_diff_line, DiffLineKind};
use nebula_core::{Agent, AgentStatus, SessionRef};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use ratatui::Frame;

pub fn draw(f: &mut Frame, app: &mut App) {
    app.hits.clear();

    let [body, footer] =
        Layout::vertical([Constraint::Min(3), Constraint::Length(1)]).areas(f.area());

    if app.collapsed {
        draw_terminal(f, app, body);
        draw_footer(f, app, footer);
        draw_overlay(f, app);
        return;
    }

    app.body_area = body;
    app.normalize_panel_widths(body.width);
    let [projects_a, worktrees_a, sessions_a, term_a] = Layout::horizontal([
        Constraint::Length(app.panel_widths[0]),
        Constraint::Length(app.panel_widths[1]),
        Constraint::Length(app.panel_widths[2]),
        Constraint::Min(20),
    ])
    .areas(body);

    // Splitter grab zones: the two touching border cells at each panel
    // boundary. Registered first so they win `hit_at`'s first-match scan.
    for i in 0..3 {
        let x = app.splitter_x(i);
        app.hits.push((
            Rect {
                x: x.saturating_sub(1),
                y: body.y,
                width: 2,
                height: body.height,
            },
            HitTarget::Splitter(i),
        ));
    }

    draw_projects(f, app, projects_a);
    draw_worktrees(f, app, worktrees_a);
    draw_sessions(f, app, sessions_a);
    draw_terminal(f, app, term_a);
    draw_footer(f, app, footer);
    draw_overlay(f, app);
}

fn draw_overlay(f: &mut Frame, app: &mut App) {
    let Some(overlay) = app.overlay.clone() else {
        return;
    };
    match overlay {
        Overlay::Menu(menu) => {
            let title_width = menu
                .title
                .as_deref()
                .map(|t| t.chars().count() + 2)
                .unwrap_or(0);
            let width = (menu
                .items
                .iter()
                .map(|i| i.label.chars().count())
                .max()
                .unwrap_or(8)
                + 4)
            .max(title_width + 2)
            .min(f.area().width as usize) as u16;
            let height = menu.items.len() as u16 + 2;
            let x = menu.at.0.min(f.area().width.saturating_sub(width));
            let y = if menu.at.1 + height > f.area().height {
                menu.at.1.saturating_sub(height)
            } else {
                menu.at.1
            };
            let area = Rect {
                x,
                y,
                width,
                height: height.min(f.area().height),
            };
            f.render_widget(Clear, area);
            let mut block = Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Cyan));
            if let Some(title) = &menu.title {
                block = block.title(Span::styled(
                    format!(" {title} "),
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD),
                ));
            }
            let inner = block.inner(area);
            f.render_widget(block, area);
            for (i, item) in menu.items.iter().enumerate() {
                let Some(row) = row_rect(inner, i) else { break };
                let mut style = if item.destructive {
                    Style::default().fg(Color::Red)
                } else {
                    Style::default()
                };
                if i == menu.hover {
                    style = style.add_modifier(Modifier::REVERSED);
                }
                f.render_widget(
                    Paragraph::new(Span::styled(format!(" {} ", item.label), style)),
                    row,
                );
            }
            // Record the drawn area for click hit-testing.
            if let Some(Overlay::Menu(m)) = &mut app.overlay {
                m.area = area;
            }
        }
        Overlay::Confirm(confirm) => {
            let area = centered_rect(f.area(), 52, 7);
            f.render_widget(Clear, area);
            let block = Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Red))
                .title(Span::styled(
                    format!(" {} ", confirm.title),
                    Style::default().fg(Color::Red),
                ));
            let inner = block.inner(area);
            f.render_widget(block, area);
            let lines = vec![
                Line::from(confirm.message.clone()),
                Line::from(""),
                Line::from(vec![
                    Span::styled("[Enter/y] confirm", Style::default().fg(Color::Red)),
                    Span::raw("   "),
                    Span::styled("[Esc/n] cancel", Style::default().fg(Color::DarkGray)),
                ]),
            ];
            f.render_widget(Paragraph::new(lines), inner);
        }
        Overlay::Prompt(prompt) => {
            // Paths get wide dialogs; candidate list adds a row when present.
            let width = if prompt.completes_paths() { 72 } else { 52 };
            let height = if prompt.candidates.is_empty() { 6 } else { 7 };
            let area = centered_rect(f.area(), width, height);
            f.render_widget(Clear, area);
            let block = Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Cyan))
                .title(Span::styled(
                    format!(" {} ", prompt.title),
                    Style::default().fg(Color::Cyan),
                ));
            let inner = block.inner(area);
            f.render_widget(block, area);

            // Long paths: show the tail so the cursor position stays visible.
            let input_budget = inner.width.saturating_sub(3) as usize;
            let shown_input: String = if prompt.input.chars().count() > input_budget {
                let skip = prompt.input.chars().count() - input_budget;
                format!(
                    "…{}",
                    prompt.input.chars().skip(skip + 1).collect::<String>()
                )
            } else {
                prompt.input.clone()
            };

            let mut lines = vec![
                Line::from(Span::styled(
                    prompt.label.clone(),
                    Style::default().fg(Color::DarkGray),
                )),
                Line::from(vec![
                    Span::raw("> "),
                    Span::styled(shown_input, Style::default().fg(Color::White)),
                    Span::styled("█", Style::default().fg(Color::White)),
                ]),
            ];
            if !prompt.candidates.is_empty() {
                let max_shown = 6;
                let mut list = prompt
                    .candidates
                    .iter()
                    .take(max_shown)
                    .cloned()
                    .collect::<Vec<_>>()
                    .join("  ");
                if prompt.candidates.len() > max_shown {
                    list.push_str(&format!(
                        "  (+{} more)",
                        prompt.candidates.len() - max_shown
                    ));
                }
                lines.push(Line::from(Span::styled(
                    truncate(&list, inner.width as usize),
                    Style::default().fg(Color::Yellow),
                )));
            }
            lines.push(Line::from(""));
            let hint = if prompt.completes_paths() {
                "[Enter] ok   [Tab] complete   [Esc] cancel"
            } else {
                "[Enter] ok   [Esc] cancel"
            };
            lines.push(Line::from(Span::styled(
                hint,
                Style::default().fg(Color::DarkGray),
            )));
            f.render_widget(Paragraph::new(lines), inner);
        }
        Overlay::Help => {
            let area = centered_rect(f.area(), 60, 25);
            f.render_widget(Clear, area);
            let block = Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Cyan))
                .title(" Help ");
            let inner = block.inner(area);
            f.render_widget(block, area);
            let dim = Style::default().fg(Color::DarkGray);
            let lines: Vec<Line> = [
                ("Tab / Shift+Tab", "cycle focus between panels"),
                ("h/l or ←/→", "move focus left / right (Ctrl+←/→ too)"),
                ("j/k or ↓/↑", "move selection"),
                ("Enter", "drill in / attach; terminal pane: lock input"),
                ("n", "new project / worktree / agent (per panel)"),
                ("Shift+J/K", "move project up / down (⇧↑/↓ where supported)"),
                ("-", "divider below project / remove selected divider"),
                ("Enter or r", "on a divider: edit its label"),
                ("r", "rename (sessions panel)"),
                ("a", "archive agent"),
                ("u", "unarchive agent"),
                ("d / ⌫", "delete (confirms first)"),
                ("A", "toggle archived agents"),
                ("m / right-click", "context menu"),
                ("g", "git diff of the selected worktree"),
                ("Ctrl+q", "terminal input → back to panels (also Ctrl+])"),
                ("drag", "select terminal text → copies to clipboard"),
                ("double-click", "select the word under the cursor"),
                ("⌥+click", "open the URL under the cursor"),
                ("Shift+drag", "select text with your terminal"),
                ("drag border", "resize the panels"),
                ("q", "quit"),
            ]
            .iter()
            .map(|(k, v)| {
                Line::from(vec![
                    Span::styled(format!(" {k:<16}"), Style::default().fg(Color::Cyan)),
                    Span::styled((*v).to_string(), dim),
                ])
            })
            .collect();
            f.render_widget(Paragraph::new(lines), inner);
        }
        Overlay::Diff(view) => {
            let area = centered_rect_pct(f.area(), 92, 90);
            f.render_widget(Clear, area);
            let [files_a, diff_a] =
                Layout::horizontal([Constraint::Length(34), Constraint::Min(20)]).areas(area);

            // Left: changed-file list; a stateless follow-window keeps the
            // selected row visible.
            let files_title = format!("Files ({})", view.files.len());
            let block = panel_block(&files_title, true);
            let files_inner = block.inner(files_a);
            f.render_widget(block, files_a);
            let start = (view.selected + 1).saturating_sub(files_inner.height as usize);
            for (row, (i, file)) in view.files.iter().enumerate().skip(start).enumerate() {
                let Some(row_area) = row_rect(files_inner, row) else {
                    break;
                };
                let status_color = match (file.xy[0], file.xy[1]) {
                    ('?', '?') | ('A', _) => Color::Green,
                    ('D', _) | (_, 'D') => Color::Red,
                    ('R', _) | ('C', _) => Color::Cyan,
                    _ => Color::Yellow,
                };
                let budget = (files_inner.width as usize).saturating_sub(3);
                let mut spans = vec![Span::styled(
                    format!("{} ", file.status_str()),
                    Style::default().fg(status_color),
                )];
                let shown = truncate(&file.path, budget);
                let used = shown.chars().count();
                spans.push(Span::raw(shown));
                if let Some(orig) = &file.orig_path {
                    let rest = budget.saturating_sub(used);
                    if rest > 3 {
                        spans.push(Span::styled(
                            truncate(&format!(" ← {orig}"), rest),
                            Style::default().fg(Color::DarkGray),
                        ));
                    }
                }
                render_row(f, row_area, spans, i == view.selected, true);
            }

            // Right: the selected file's diff, scrolled.
            let sel_path = view
                .files
                .get(view.selected)
                .map(|d| d.path.as_str())
                .unwrap_or("");
            let title = truncate(
                &format!("{}: {}", view.branch, sel_path),
                (diff_a.width as usize).saturating_sub(4),
            );
            let mut block = panel_block(&title, true);
            let diff_inner = block.inner(diff_a);
            let max_scroll = (view.diff_line_count as u16).saturating_sub(diff_inner.height.max(1));
            let scroll = view.scroll.min(max_scroll);
            if max_scroll > 0 {
                block = block.title_bottom(
                    Line::from(Span::styled(
                        format!(" {}/{} ", scroll + 1, view.diff_line_count),
                        Style::default().fg(Color::DarkGray),
                    ))
                    .right_aligned(),
                );
            }
            f.render_widget(block, diff_a);
            let lines: Vec<Line> = view
                .diff
                .lines()
                .map(|l| {
                    let style = match classify_diff_line(l) {
                        DiffLineKind::Add => Style::default().fg(Color::Green),
                        DiffLineKind::Remove => Style::default().fg(Color::Red),
                        DiffLineKind::Hunk => Style::default().fg(Color::Cyan),
                        DiffLineKind::Header => Style::default().fg(Color::DarkGray),
                        DiffLineKind::Context => Style::default(),
                    };
                    Line::from(Span::styled(l.to_string(), style))
                })
                .collect();
            f.render_widget(Paragraph::new(lines).scroll((scroll, 0)), diff_inner);

            // Write-back (draw works on a clone): page size for key paging,
            // scroll re-clamped so resizes never strand the view.
            if let Some(Overlay::Diff(v)) = &mut app.overlay {
                v.view_height = diff_inner.height;
                v.scroll = scroll;
            }
        }
    }
}

fn centered_rect(frame: Rect, width: u16, height: u16) -> Rect {
    let width = width.min(frame.width);
    let height = height.min(frame.height);
    Rect {
        x: frame.x + (frame.width - width) / 2,
        y: frame.y + (frame.height - height) / 2,
        width,
        height,
    }
}

/// A centered rect sized as a percentage of the frame.
fn centered_rect_pct(frame: Rect, pct_w: u16, pct_h: u16) -> Rect {
    centered_rect(frame, frame.width * pct_w / 100, frame.height * pct_h / 100)
}

fn panel_block(title: &str, focused: bool) -> Block<'_> {
    let border_style = if focused {
        Style::default().fg(Color::Cyan)
    } else {
        Style::default().fg(Color::DarkGray)
    };
    Block::default()
        .borders(Borders::ALL)
        .border_style(border_style)
        .title(Span::styled(
            format!(" {title} "),
            if focused {
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::Gray)
            },
        ))
}

fn status_dot(status: Option<AgentStatus>) -> Span<'static> {
    match status {
        Some(AgentStatus::Fresh) => Span::styled("● ", Style::default().fg(Color::DarkGray)),
        Some(AgentStatus::Running) => Span::styled("● ", Style::default().fg(Color::Yellow)),
        Some(AgentStatus::Finished) => Span::styled("● ", Style::default().fg(Color::Green)),
        Some(AgentStatus::NeedsFeedback) => Span::styled("● ", Style::default().fg(Color::Red)),
        Some(AgentStatus::Terminated) => Span::styled("● ", Style::default().fg(Color::Magenta)),
        Some(AgentStatus::Disconnected) => Span::styled("○ ", Style::default().fg(Color::DarkGray)),
        None => Span::styled("○ ", Style::default().fg(Color::DarkGray)),
    }
}

/// Base style for a whole list row. Selection reads as a full-width bar so
/// the active project → worktree → session path stays visible at a glance:
/// bright (reversed) in the focused panel, dim gray in unfocused panels.
fn row_bar(selected: bool, focused: bool) -> Style {
    if selected && focused {
        Style::default().add_modifier(Modifier::REVERSED | Modifier::BOLD)
    } else if selected {
        Style::default()
            .bg(Color::DarkGray)
            .fg(Color::White)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default()
    }
}

/// Render one list row as a full-width bar. DarkGray spans (idle dots,
/// archived names) would vanish against the unfocused-selection bar, so
/// they get lifted to Gray there.
fn render_row(f: &mut Frame, area: Rect, mut spans: Vec<Span>, selected: bool, focused: bool) {
    if selected && !focused {
        for s in &mut spans {
            if s.style.fg == Some(Color::DarkGray) {
                s.style.fg = Some(Color::Gray);
            }
        }
    }
    f.render_widget(
        Paragraph::new(Line::from(spans)).style(row_bar(selected, focused)),
        area,
    );
}

fn draw_projects(f: &mut Frame, app: &mut App, area: Rect) {
    let focused = app.focus == Focus::Projects;
    let block = panel_block("Projects", focused);
    let inner = block.inner(area);
    f.render_widget(block, area);

    if app.tree.projects.is_empty() {
        f.render_widget(
            Paragraph::new(vec![
                Line::from(Span::styled(
                    "No projects yet",
                    Style::default().fg(Color::DarkGray),
                )),
                Line::from(Span::styled(
                    "n: add project",
                    Style::default().fg(Color::DarkGray),
                )),
            ]),
            inner,
        );
        app.hits.push((inner, HitTarget::PanelBg(Focus::Projects)));
        return;
    }

    // Projects and their dividers are one selectable row list; the payload
    // pre-collects per-row display data to end the tree borrow.
    let rows: Vec<(ProjectRow, String, Option<AgentStatus>)> = app
        .project_rows()
        .into_iter()
        .map(|row| match row {
            ProjectRow::Project(i) => {
                let p = &app.tree.projects[i];
                (row, p.name.clone(), app.project_rollup(&p.id))
            }
            ProjectRow::Divider(i) => {
                let label = app.tree.projects[i]
                    .divider_label
                    .clone()
                    .unwrap_or_default();
                (row, label, None)
            }
        })
        .collect();
    for (row_idx, (row, text, roll)) in rows.iter().enumerate() {
        let Some(row_area) = row_rect(inner, row_idx) else {
            break;
        };
        let spans = match row {
            ProjectRow::Project(_) => vec![
                status_dot(*roll),
                Span::raw(truncate(text, inner.width.saturating_sub(2) as usize)),
            ],
            ProjectRow::Divider(_) => divider_spans(text, inner.width),
        };
        render_row(f, row_area, spans, row_idx == app.sel_project, focused);
        app.hits.push((row_area, HitTarget::Project(row_idx)));
    }
    app.hits.push((inner, HitTarget::PanelBg(Focus::Projects)));
}

/// A divider line, with the group label woven in when present:
/// `─ label ────────`.
fn divider_spans(label: &str, width: u16) -> Vec<Span<'static>> {
    let w = width as usize;
    let dim = Style::default().fg(Color::DarkGray);
    if label.is_empty() {
        return vec![Span::styled("─".repeat(w), dim)];
    }
    let label = truncate(label, w.saturating_sub(4));
    let tail = w.saturating_sub(label.chars().count() + 3);
    vec![
        Span::styled("─ ".to_string(), dim),
        Span::styled(
            label,
            Style::default()
                .fg(Color::Gray)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(format!(" {}", "─".repeat(tail)), dim),
    ]
}

fn draw_worktrees(f: &mut Frame, app: &mut App, area: Rect) {
    let focused = app.focus == Focus::Worktrees;
    let block = panel_block("Worktrees", focused);
    let inner = block.inner(area);
    f.render_widget(block, area);

    let worktrees: Vec<(String, bool, Option<AgentStatus>)> = app
        .visible_worktrees()
        .iter()
        .map(|w| (w.branch.clone(), w.is_main, app.worktree_rollup(&w.id)))
        .collect();
    if worktrees.is_empty() {
        let hint = if app.tree.projects.is_empty() {
            "—"
        } else {
            "n: new worktree"
        };
        f.render_widget(
            Paragraph::new(hint).style(Style::default().fg(Color::DarkGray)),
            inner,
        );
        app.hits.push((inner, HitTarget::PanelBg(Focus::Worktrees)));
        return;
    }

    // The main checkout renders as `branch ⌂ root` (dim badge — the branch
    // is live, the badge marks root-ness) with a rule separating it from the
    // true worktrees below, so rows after it sit one screen line lower.
    const ROOT_BADGE: &str = " ⌂ root";
    let mut screen_row = 0usize;
    for (i, (branch, is_main, roll)) in worktrees.iter().enumerate() {
        let Some(row_area) = row_rect(inner, screen_row) else {
            break;
        };
        let mut spans = vec![status_dot(*roll)];
        if *is_main {
            let max = (inner.width as usize).saturating_sub(2 + ROOT_BADGE.chars().count());
            spans.push(Span::raw(truncate(branch, max)));
            spans.push(Span::styled(
                ROOT_BADGE,
                Style::default().fg(Color::DarkGray),
            ));
        } else {
            spans.push(Span::raw(truncate(
                branch,
                inner.width.saturating_sub(2) as usize,
            )));
        }
        render_row(f, row_area, spans, i == app.sel_worktree, focused);
        app.hits.push((row_area, HitTarget::Worktree(i)));
        screen_row += 1;
        if *is_main && worktrees.len() > 1 {
            if let Some(r) = row_rect(inner, screen_row) {
                f.render_widget(
                    Paragraph::new(Span::styled(
                        "─".repeat(inner.width as usize),
                        Style::default().fg(Color::DarkGray),
                    )),
                    r,
                );
                screen_row += 1;
            }
        }
    }
    app.hits.push((inner, HitTarget::PanelBg(Focus::Worktrees)));
}

fn draw_sessions(f: &mut Frame, app: &mut App, area: Rect) {
    let focused = app.focus == Focus::Sessions;
    let block = panel_block("Sessions", focused);
    let inner = block.inner(area);
    f.render_widget(block, area);

    let sessions = app.visible_sessions();
    let (agent_count, archived_count) = app.session_group_counts();
    let dim = Style::default().fg(Color::DarkGray);

    let mut screen_row: usize = 0;
    let header = |f: &mut Frame, text: String, screen_row: &mut usize| {
        if let Some(r) = row_rect(inner, *screen_row) {
            f.render_widget(Paragraph::new(Span::styled(text, dim)), r);
            *screen_row += 1;
        }
    };

    header(f, "AGENTS".into(), &mut screen_row);
    for (i, row) in sessions.iter().enumerate().take(agent_count) {
        let Some(r) = row_rect(inner, screen_row) else {
            break;
        };
        draw_session_row(f, app, r, i, row, focused, inner.width);
        screen_row += 1;
    }
    if archived_count > 0 {
        header(f, "─".repeat(inner.width as usize), &mut screen_row);
        if app.show_archived {
            header(f, format!("ARCHIVED ({archived_count})"), &mut screen_row);
            for (i, row) in sessions.iter().enumerate().skip(agent_count) {
                let Some(r) = row_rect(inner, screen_row) else {
                    break;
                };
                draw_session_row(f, app, r, i, row, focused, inner.width);
                screen_row += 1;
            }
        } else {
            header(
                f,
                format!("… {archived_count} archived (A to show)"),
                &mut screen_row,
            );
        }
    }

    // Panel background (registered last so rows win the hit-test).
    app.hits.push((inner, HitTarget::PanelBg(Focus::Sessions)));
}

fn draw_session_row(
    f: &mut Frame,
    app: &mut App,
    area: Rect,
    index: usize,
    row: &Agent,
    focused: bool,
    width: u16,
) {
    let dot = if row.archived {
        Span::styled("⊘ ", Style::default().fg(Color::DarkGray))
    } else {
        status_dot(Some(row.status))
    };
    let name_style = if row.archived {
        Style::default().fg(Color::DarkGray)
    } else {
        Style::default()
    };
    // Non-default CLIs get a dim badge (same idiom as the worktree root row).
    let badge = match row.kind {
        nebula_core::AgentKind::Claude => None,
        nebula_core::AgentKind::Codex => Some(" codex"),
        nebula_core::AgentKind::Cursor => Some(" cursor"),
    };
    let name_max = (width.saturating_sub(2) as usize).saturating_sub(badge.map_or(0, str::len));
    let mut spans = vec![dot, Span::styled(truncate(&row.name, name_max), name_style)];
    if let Some(badge) = badge {
        spans.push(Span::styled(badge, Style::default().fg(Color::DarkGray)));
    }
    render_row(f, area, spans, index == app.sel_session, focused);
    app.hits.push((area, HitTarget::Session(index)));
}

fn draw_terminal(f: &mut Frame, app: &mut App, area: Rect) {
    let focused = app.focus == Focus::Terminal;
    // Name the attached session in the title so it's clear what you're
    // looking at (and typing into) even with the sidebars collapsed.
    let who = attached_session_name(app)
        .map(|n| format!(" — {n}"))
        .unwrap_or_default();
    let title = match &app.term {
        Some(t) if t.exited => format!("Terminal{who} (exited)"),
        Some(t) if t.scroll > 0 => format!("Terminal{who} [SCROLL {}]", t.scroll),
        Some(_) if app.term_locked => format!("Terminal{who} [input]"),
        Some(_) => format!("Terminal{who}"),
        None => "Terminal".to_string(),
    };
    let block = panel_block(&title, focused);
    let inner = block.inner(area);
    f.render_widget(block, area);
    app.term_area = inner;
    app.hits.push((inner, HitTarget::TerminalPane));

    let links = match &app.term {
        Some(term) => {
            let screen = term.parser.screen();
            let widget = tui_term::widget::PseudoTerminal::new(screen);
            f.render_widget(widget, inner);
            // Selection highlight: overlay REVERSED on the selected cells
            // (stream selection — full rows between the endpoints).
            if let Some(sel) = app.term_selection.filter(|s| s.active) {
                let ((start_col, start_row), (end_col, end_row)) = sel.bounds();
                let reversed = Style::default().add_modifier(Modifier::REVERSED);
                let last_col = inner.width.saturating_sub(1);
                for row in start_row..=end_row {
                    let (from, to) = if start_row == end_row {
                        (start_col, end_col)
                    } else if row == start_row {
                        (start_col, last_col)
                    } else if row == end_row {
                        (0, end_col)
                    } else {
                        (0, last_col)
                    };
                    let width = to.saturating_sub(from) + 1;
                    let line =
                        Rect::new(inner.x + from, inner.y + row, width, 1).intersection(inner);
                    f.buffer_mut().set_style(line, reversed);
                }
            }
            crate::links::visible_links(term.parser.screen())
        }
        None => {
            let msg = Paragraph::new(vec![
                Line::from(""),
                Line::from(Span::styled(
                    "nebula",
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD),
                )),
                Line::from(""),
                Line::from(Span::styled(
                    "select a session and press Enter to attach",
                    Style::default().fg(Color::DarkGray),
                )),
            ])
            .centered();
            f.render_widget(msg, inner);
            Vec::new()
        }
    };
    // Underline detected URLs so ⌥click has a visible affordance; kept on
    // the App for click-time hit-testing against the drawn frame.
    let underline = Style::default().add_modifier(Modifier::UNDERLINED);
    for link in &links {
        for &(row, c0, c1) in &link.segments {
            let seg = Rect::new(inner.x + c0, inner.y + row, c1 - c0 + 1, 1).intersection(inner);
            f.buffer_mut().set_style(seg, underline);
        }
    }
    app.term_links = links;
}

fn attached_session_name(app: &App) -> Option<String> {
    match &app.term.as_ref()?.sref {
        SessionRef::Agent(id) => app
            .tree
            .agents
            .iter()
            .find(|a| &a.id == id)
            .map(|a| a.name.clone()),
        // The TUI only attaches agents; scratch terminals belong to
        // `nebula raw-attach`.
        SessionRef::Terminal(_) => None,
    }
}

/// `project ▸ branch ▸ session` breadcrumb of the current selection; the
/// segment matching the focused panel is highlighted. Sessions/Terminal
/// focus both highlight the session segment.
fn breadcrumb(app: &App) -> Vec<Span<'static>> {
    let seg = |name: &str, active: bool| {
        Span::styled(
            truncate(name, 20),
            if active {
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::Gray)
            },
        )
    };
    let sep = || Span::styled(" ▸ ", Style::default().fg(Color::DarkGray));

    let mut spans = Vec::new();
    let Some(project) = app.selected_project() else {
        return spans;
    };
    spans.push(seg(&project.name, app.focus == Focus::Projects));
    if let Some(worktree) = app.selected_worktree() {
        spans.push(sep());
        spans.push(seg(&worktree.branch, app.focus == Focus::Worktrees));
        if let Some(session) = app.selected_session() {
            spans.push(sep());
            spans.push(seg(
                &session.name,
                matches!(app.focus, Focus::Sessions | Focus::Terminal),
            ));
        }
    }
    spans
}

fn draw_footer(f: &mut Frame, app: &App, area: Rect) {
    let conn = match app.conn {
        ConnState::Connected => Span::styled("⏻ connected", Style::default().fg(Color::Green)),
        ConnState::Disconnected => Span::styled("✗ disconnected", Style::default().fg(Color::Red)),
    };
    let hints = if let Some(flash) = &app.flash {
        Span::styled(flash.clone(), Style::default().fg(Color::Yellow))
    } else if matches!(&app.overlay, Some(Overlay::Diff(_))) {
        Span::styled(
            "j/k: file  J/K: scroll  Ctrl+d/u: page  g/G: top/end  Esc: close",
            Style::default().fg(Color::DarkGray),
        )
    } else if app.overlay.is_some() {
        Span::styled(
            "Esc: close  Enter: confirm",
            Style::default().fg(Color::DarkGray),
        )
    } else {
        let text = match app.focus {
            Focus::Terminal if app.term.as_ref().is_some_and(|t| t.exited) => {
                "session exited — Esc: back to sessions"
            }
            Focus::Terminal if app.term_locked => {
                "Ctrl+q: panels  drag: select+copy  ⌥click: open link"
            }
            Focus::Terminal if app.term.is_some() => "Enter: type into terminal  ←: sessions",
            Focus::Terminal => "select a session and press Enter to attach",
            Focus::Projects => match app.selected_project_row() {
                Some(ProjectRow::Divider(_)) => {
                    "Enter/r: label  d: delete divider  m: menu  ?: help"
                }
                _ => "n: add  d: remove  -: divider  ⇧J/K: move  m: menu  ?: help",
            },
            Focus::Worktrees => "n: new worktree  d: delete  m: menu  ?: help",
            Focus::Sessions => "n: agent  r: rename  a: archive  d: del  m: menu  ?: help",
        };
        Span::styled(text, Style::default().fg(Color::DarkGray))
    };
    let mut spans = vec![conn, Span::raw("  │  ")];
    let crumbs = breadcrumb(app);
    if !crumbs.is_empty() {
        spans.extend(crumbs);
        spans.push(Span::raw("  │  "));
    }
    spans.push(hints);
    f.render_widget(Paragraph::new(Line::from(spans)), area);
}

/// The i-th single-height row inside `inner`, or None when it overflows.
fn row_rect(inner: Rect, i: usize) -> Option<Rect> {
    let y = inner.y + i as u16;
    if y >= inner.y + inner.height {
        return None;
    }
    Some(Rect {
        x: inner.x,
        y,
        width: inner.width,
        height: 1,
    })
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
        out.push('…');
        out
    }
}
