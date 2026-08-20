//! View layer: draws the three panels + terminal pane + footer, and records
//! hit regions for mouse interaction.

use crate::app::{App, ConnState, Focus, HitTarget, Overlay, PaletteTarget, ProjectRow, SessionRow};
use crate::git_diff::{classify_diff_line, DiffLineKind};
use crate::theme::Theme;
use nebula_core::{AgentStatus, SessionRef};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Paragraph};
use ratatui::Frame;

/// Outer size of the editor modal, as (width, height) percent of the frame.
/// Shared with the event loop's pre-draw PTY size guess.
pub const VIM_MODAL_PCT: (u16, u16) = (94, 92);

pub fn draw(f: &mut Frame, app: &mut App) {
    app.hits.clear();

    let [body, footer] =
        Layout::vertical([Constraint::Min(3), Constraint::Length(1)]).areas(f.area());

    if app.collapsed {
        draw_terminal(f, app, body);
        draw_footer(f, app, footer);
        draw_overlay(f, app);
        draw_vim(f, app);
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
    draw_vim(f, app);
}

/// The editor, above every overlay: a centered modal, or — spawned from the
/// tree browser — embedded in its preview pane (whose block the tree arm
/// already drew).
fn draw_vim(f: &mut Frame, app: &mut App) {
    let th = app.theme;
    let Some(vim) = &app.vim else {
        return;
    };
    if vim.embedded {
        if let Some(Overlay::Tree(view)) = &app.overlay {
            let inner = view.preview_area;
            if inner.width < 2 || inner.height < 2 {
                return; // pane not drawn yet
            }
            f.render_widget(
                tui_term::widget::PseudoTerminal::new(vim.parser.screen()),
                inner,
            );
            // Write-back: the post-draw sync resizes the PTY to the pane.
            if let Some(vim) = &mut app.vim {
                vim.area = inner;
            }
            return;
        }
        // Tree overlay gone under an embedded editor — fall through to the
        // modal so the session is never invisible.
    }
    let area = centered_rect_pct(f.area(), VIM_MODAL_PCT.0, VIM_MODAL_PCT.1);
    f.render_widget(Clear, area);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Thick)
        .border_style(Style::default().fg(th.accent))
        .title(Span::styled(
            format!(" {} ", vim.title),
            Style::default()
                .fg(th.on_accent)
                .bg(th.accent)
                .add_modifier(Modifier::BOLD),
        ))
        .title_bottom(Line::from(Span::styled(
            " Ctrl+Q: force close ",
            Style::default().fg(th.dim),
        )));
    let inner = block.inner(area);
    f.render_widget(block, area);
    f.render_widget(
        tui_term::widget::PseudoTerminal::new(vim.parser.screen()),
        inner,
    );
    // Write-back: the post-draw sync resizes the PTY to the drawn rect.
    if let Some(vim) = &mut app.vim {
        vim.area = inner;
    }
}

fn draw_overlay(f: &mut Frame, app: &mut App) {
    let th = app.theme;
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
            let area = match menu.at {
                Some((ax, ay)) => {
                    let x = ax.min(f.area().width.saturating_sub(width));
                    let y = if ay + height > f.area().height {
                        ay.saturating_sub(height)
                    } else {
                        ay
                    };
                    Rect {
                        x,
                        y,
                        width,
                        height: height.min(f.area().height),
                    }
                }
                None => centered_rect(f.area(), width, height),
            };
            f.render_widget(Clear, area);
            let mut block = Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(th.accent));
            if let Some(title) = &menu.title {
                block = block.title(Span::styled(
                    format!(" {title} "),
                    Style::default()
                        .fg(th.accent)
                        .add_modifier(Modifier::BOLD),
                ));
            }
            let inner = block.inner(area);
            f.render_widget(block, area);
            for (i, item) in menu.items.iter().enumerate() {
                let Some(row) = row_rect(inner, i) else { break };
                let mut style = if item.destructive {
                    Style::default().fg(th.err)
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
            // Bulk deletes itemize their casualties across several message
            // lines — size the dialog to fit them.
            let msg_lines: Vec<&str> = confirm.message.lines().collect();
            let longest = msg_lines.iter().map(|l| l.chars().count()).max();
            let width = (longest.unwrap_or(0) as u16 + 4).max(52);
            let height = msg_lines.len() as u16 + 4;
            let area = centered_rect(f.area(), width, height);
            f.render_widget(Clear, area);
            let block = Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(th.err))
                .title(Span::styled(
                    format!(" {} ", confirm.title),
                    Style::default().fg(th.err),
                ));
            let inner = block.inner(area);
            f.render_widget(block, area);
            let mut lines: Vec<Line> = msg_lines
                .into_iter()
                .map(|l| Line::from(l.to_string()))
                .collect();
            lines.push(Line::from(""));
            lines.push(Line::from(vec![
                Span::styled("[Enter/y] confirm", Style::default().fg(th.err)),
                Span::raw("   "),
                Span::styled("[Esc/n] cancel", Style::default().fg(th.dim)),
            ]));
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
                .border_style(Style::default().fg(th.accent))
                .title(Span::styled(
                    format!(" {} ", prompt.title),
                    Style::default().fg(th.accent),
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
                    Style::default().fg(th.dim),
                )),
                Line::from(vec![
                    Span::raw("> "),
                    Span::styled(shown_input, Style::default().fg(th.text)),
                    Span::styled("█", Style::default().fg(th.text)),
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
                    Style::default().fg(th.warn),
                )));
            }
            lines.push(Line::from(""));
            let hint = if prompt.completes_paths() {
                "[Enter] ok   [Tab] complete   [Ctrl+u] clear   [Esc] cancel"
            } else {
                "[Enter] ok   [Ctrl+u] clear   [Esc] cancel"
            };
            lines.push(Line::from(Span::styled(
                hint,
                Style::default().fg(th.dim),
            )));
            f.render_widget(Paragraph::new(lines), inner);
        }
        Overlay::Help => {
            let area = centered_rect(f.area(), 60, 35);
            f.render_widget(Clear, area);
            let block = Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(th.accent))
                .title(" Help ");
            let inner = block.inner(area);
            f.render_widget(block, area);
            let dim = Style::default().fg(th.dim);
            let lines: Vec<Line> = [
                ("Tab / Shift+Tab", "cycle focus between panels"),
                ("h/l or ←/→", "move focus left / right (Ctrl+←/→ too)"),
                (
                    "j/k or ↓/↑",
                    "move selection (session rows preview in the pane)",
                ),
                ("Enter", "drill in; sessions/terminal pane: lock input"),
                ("n", "new project / worktree / agent (per panel)"),
                ("T", "new terminal in the worktree (projects panel: its root)"),
                ("Shift+J/K", "move project up / down (⇧↑/↓ where supported)"),
                ("-", "divider below project / remove selected divider"),
                ("Enter or r", "on a divider: edit its label"),
                ("r", "rename (sessions panel)"),
                ("p", "pin / unpin worktree or agent (per panel)"),
                ("a", "archive agent"),
                ("u", "unarchive agent"),
                ("d / ⌫", "delete (confirms first)"),
                ("D", "delete ALL worktrees / sessions in the panel"),
                ("A", "toggle archived agents"),
                ("m / right-click", "context menu"),
                ("g", "git diff of the selected worktree"),
                ("Ctrl+r", "diff view: toggle reviewed ✓ on the file"),
                ("f", "fuzzy find a file, Enter edits it in vim, Ctrl+y copies"),
                ("F", "find in files (git grep), Enter edits the hit in vim"),
                ("t", "file tree browser; Enter edits in the preview pane"),
                ("/", "fuzzy search projects / worktrees / sessions"),
                ("s", "settings (toggles write to config.json)"),
                (
                    "Ctrl+o / Ctrl+f",
                    "search pick: open session / focus its row",
                ),
                ("Ctrl+u", "clear the typed input / filter in any overlay"),
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
                    Span::styled(format!(" {k:<16}"), Style::default().fg(th.accent)),
                    Span::styled((*v).to_string(), dim),
                ])
            })
            .collect();
            f.render_widget(Paragraph::new(lines), inner);
        }
        Overlay::Settings(view) => {
            let cfg = crate::config::Config::load();
            let n = crate::config::SETTINGS.len();
            // 2 rows per setting (label + hint), then blank + hints + path.
            let height = (n * 2 + 5) as u16;
            let area = centered_rect(f.area(), 64, height);
            f.render_widget(Clear, area);
            let block = Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(th.accent))
                .title(Span::styled(
                    " Settings ",
                    Style::default()
                        .fg(th.accent)
                        .add_modifier(Modifier::BOLD),
                ));
            let inner = block.inner(area);
            f.render_widget(block, area);

            let dim = Style::default().fg(th.dim);
            let mut lines: Vec<Line> = Vec::new();
            for (i, spec) in crate::config::SETTINGS.iter().enumerate() {
                let value = cfg.value_label(spec.kind);
                let mut label_style = Style::default();
                let mut value_style = Style::default().fg(th.accent);
                if i == view.selected {
                    label_style = label_style.add_modifier(Modifier::REVERSED);
                    value_style = value_style.add_modifier(Modifier::REVERSED);
                }
                lines.push(Line::from(vec![
                    Span::styled(format!(" {:<28}", spec.label), label_style),
                    Span::styled(format!("[{value}]"), value_style),
                ]));
                lines.push(Line::from(Span::styled(format!("   {}", spec.hint), dim)));
            }
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                "Enter/Space: toggle   h/l: cycle   Esc: close",
                dim,
            )));
            let path = nebula_core::paths::config_path();
            lines.push(Line::from(Span::styled(
                truncate(&path.display().to_string(), inner.width as usize),
                dim,
            )));
            f.render_widget(Paragraph::new(lines), inner);
            if let Some(Overlay::Settings(v)) = &mut app.overlay {
                v.area = area;
            }
        }
        Overlay::Diff(view) => {
            let area = centered_rect_pct(f.area(), 92, 90);
            f.render_widget(Clear, area);
            // Cap first, floor second: on a tiny screen the file list keeps
            // its minimum and Min(20) squeezes the diff pane instead.
            let files_w = view
                .files_width
                .min(area.width.saturating_sub(crate::app::MIN_DIFF_PANE_W))
                .max(crate::app::MIN_DIFF_FILES_W);
            let [files_a, diff_a] =
                Layout::horizontal([Constraint::Length(files_w), Constraint::Min(20)]).areas(area);

            // Left: changed-file list; a stateless follow-window keeps the
            // selected row visible.
            let mut files_title = if view.filter.is_empty() {
                format!("Files ({})", view.files.len())
            } else {
                format!("Files ({}/{})", view.matches.len(), view.files.len())
            };
            if !view.reviewed.is_empty() {
                files_title.push_str(&format!(" · {}✓", view.reviewed.len()));
            }
            let block = panel_block(&files_title, true, th);
            let files_inner = block.inner(files_a);
            f.render_widget(block, files_a);

            // First row: the always-on fuzzy filter input.
            if let Some(filter_area) = row_rect(files_inner, 0) {
                let line = if view.filter.is_empty() {
                    Line::from(Span::styled(
                        "type to filter…",
                        Style::default().fg(th.dim),
                    ))
                } else {
                    Line::from(vec![
                        Span::raw(view.filter.clone()),
                        Span::styled("█", Style::default().fg(th.accent)),
                    ])
                };
                f.render_widget(Paragraph::new(line), filter_area);
            }
            let list_inner = Rect {
                y: files_inner.y + 1,
                height: files_inner.height.saturating_sub(1),
                ..files_inner
            };

            if view.matches.is_empty() {
                if let Some(row_area) = row_rect(list_inner, 0) {
                    f.render_widget(
                        Paragraph::new(Span::styled(
                            "no matches",
                            Style::default().fg(th.dim),
                        )),
                        row_area,
                    );
                }
            }
            let start = view.window_start(list_inner.height as usize);
            for (row, (i, m)) in view.matches.iter().enumerate().skip(start).enumerate() {
                let Some(row_area) = row_rect(list_inner, row) else {
                    break;
                };
                let file = &view.files[m.file];
                let status_color = match (file.xy[0], file.xy[1]) {
                    ('?', '?') | ('A', _) => th.ok,
                    ('D', _) | (_, 'D') => th.err,
                    ('R', _) | ('C', _) => th.accent,
                    _ => th.warn,
                };
                let budget = (list_inner.width as usize).saturating_sub(5);
                let mut spans = vec![
                    Span::styled(
                        format!("{} ", file.status_str()),
                        Style::default().fg(status_color),
                    ),
                    if view.reviewed.contains_key(&file.path) {
                        Span::styled("✓ ", Style::default().fg(th.ok))
                    } else {
                        Span::raw("  ")
                    },
                ];
                let shown = truncate(&file.path, budget);
                let used = shown.chars().count();
                spans.extend(fuzzy_highlight_spans(&shown, &m.positions, th));
                if let Some(orig) = &file.orig_path {
                    let rest = budget.saturating_sub(used);
                    if rest > 3 {
                        spans.push(Span::styled(
                            truncate(&format!(" ← {orig}"), rest),
                            Style::default().fg(th.dim),
                        ));
                    }
                }
                render_row(f, row_area, spans, i == view.selected, true, th);
            }

            // Right: the selected file's diff, scrolled.
            let sel_path = view.selected_file().map(|d| d.path.as_str()).unwrap_or("");
            let sel_reviewed = view.reviewed.contains_key(sel_path);
            let title = truncate(
                &format!(
                    "{}: {}{}",
                    view.branch,
                    sel_path,
                    if sel_reviewed { " ✓" } else { "" }
                ),
                (diff_a.width as usize).saturating_sub(4),
            );
            let mut block = panel_block(&title, true, th).title_bottom(Line::from(Span::styled(
                " ^r: toggle reviewed ",
                Style::default().fg(th.dim),
            )));
            let diff_inner = block.inner(diff_a);
            let max_scroll = (view.diff_line_count as u16).saturating_sub(diff_inner.height.max(1));
            let scroll = view.scroll.min(max_scroll);
            if max_scroll > 0 {
                block = block.title_bottom(
                    Line::from(Span::styled(
                        format!(" {}/{} ", scroll + 1, view.diff_line_count),
                        Style::default().fg(th.dim),
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
                        DiffLineKind::Add => Style::default().fg(th.ok),
                        DiffLineKind::Remove => Style::default().fg(th.err),
                        DiffLineKind::Hunk => Style::default().fg(th.accent),
                        DiffLineKind::Header => Style::default().fg(th.dim),
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
                v.list_area = list_inner;
                v.area = area;
                v.files_width = files_w;
            }
        }
        Overlay::Palette(palette) => {
            let area = centered_rect(f.area(), 64, 18);
            f.render_widget(Clear, area);
            let title = if palette.query.is_empty() {
                " Jump to ".to_string()
            } else {
                format!(
                    " Jump to ({}/{}) ",
                    palette.matches.len(),
                    palette.items.len()
                )
            };
            let block = Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(th.accent))
                .title(Span::styled(
                    title,
                    Style::default()
                        .fg(th.accent)
                        .add_modifier(Modifier::BOLD),
                ));
            let inner = block.inner(area);
            f.render_widget(block, area);

            // First row: the always-on fuzzy query input.
            if let Some(query_area) = row_rect(inner, 0) {
                let line = if palette.query.is_empty() {
                    Line::from(Span::styled(
                        "type to search…",
                        Style::default().fg(th.dim),
                    ))
                } else {
                    Line::from(vec![
                        Span::raw(palette.query.clone()),
                        Span::styled("█", Style::default().fg(th.accent)),
                    ])
                };
                f.render_widget(Paragraph::new(line), query_area);
            }
            let list_inner = Rect {
                y: inner.y + 1,
                height: inner.height.saturating_sub(1),
                ..inner
            };

            if palette.matches.is_empty() {
                if let Some(row_area) = row_rect(list_inner, 0) {
                    f.render_widget(
                        Paragraph::new(Span::styled(
                            "no matches",
                            Style::default().fg(th.dim),
                        )),
                        row_area,
                    );
                }
            }
            let start = palette.window_start(list_inner.height as usize);
            for (row, (i, m)) in palette.matches.iter().enumerate().skip(start).enumerate() {
                let Some(row_area) = row_rect(list_inner, row) else {
                    break;
                };
                let item = &palette.items[m.item];
                // Kind indicator: badge emoji + a per-kind text color, so
                // rows read at a glance; the cyan-bold match highlight
                // stands out against all three.
                let (badge, kind_color) = match &item.target {
                    PaletteTarget::Project(_) => ("📁 ", None),
                    PaletteTarget::Worktree(_) => ("🌳 ", Some(th.ok)),
                    PaletteTarget::Session(_) => ("🤖 ", Some(th.special)),
                };
                let base_fg = if item.archived {
                    Some(th.dim)
                } else {
                    kind_color
                };
                let budget = (list_inner.width as usize).saturating_sub(4);
                let shown = truncate(&item.text, budget);
                // Truncation puts `…` at the last char of `shown`; a match
                // landing on that index must not light the ellipsis.
                let shown_len = shown.chars().count();
                let positions = if shown_len < item.text.chars().count() {
                    let keep = m
                        .positions
                        .iter()
                        .take_while(|&&p| p + 1 < shown_len)
                        .count();
                    &m.positions[..keep]
                } else {
                    &m.positions[..]
                };
                let mut spans = vec![Span::raw(badge)];
                let mut text_spans = fuzzy_highlight_spans(&shown, positions, th);
                if let Some(fg) = base_fg {
                    for s in &mut text_spans {
                        if s.style.fg.is_none() {
                            s.style.fg = Some(fg);
                        }
                    }
                }
                spans.extend(text_spans);
                render_row(f, row_area, spans, i == palette.selected, true, th);
            }

            // Write-back (draw works on a clone): rects for mouse
            // hit-testing.
            if let Some(Overlay::Palette(p)) = &mut app.overlay {
                p.area = area;
                p.list_area = list_inner;
            }
        }
        Overlay::Files(finder) => {
            let area = centered_rect(f.area(), 72, 20);
            f.render_widget(Clear, area);
            let title = if finder.query.is_empty() {
                format!(" Find file — {} ({}) ", finder.branch, finder.files.len())
            } else {
                format!(
                    " Find file — {} ({}/{}) ",
                    finder.branch,
                    finder.matches.len(),
                    finder.files.len()
                )
            };
            let block = Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(th.accent))
                .title(Span::styled(
                    title,
                    Style::default()
                        .fg(th.accent)
                        .add_modifier(Modifier::BOLD),
                ));
            let inner = block.inner(area);
            f.render_widget(block, area);

            // First row: the always-on fuzzy query input.
            if let Some(query_area) = row_rect(inner, 0) {
                let line = if finder.query.is_empty() {
                    Line::from(Span::styled(
                        "type to filter…",
                        Style::default().fg(th.dim),
                    ))
                } else {
                    Line::from(vec![
                        Span::raw(finder.query.clone()),
                        Span::styled("█", Style::default().fg(th.accent)),
                    ])
                };
                f.render_widget(Paragraph::new(line), query_area);
            }
            let list_inner = Rect {
                y: inner.y + 1,
                height: inner.height.saturating_sub(1),
                ..inner
            };

            if finder.matches.is_empty() {
                if let Some(row_area) = row_rect(list_inner, 0) {
                    f.render_widget(
                        Paragraph::new(Span::styled(
                            "no matches",
                            Style::default().fg(th.dim),
                        )),
                        row_area,
                    );
                }
            }
            let start = finder.window_start(list_inner.height as usize);
            for (row, (i, m)) in finder.matches.iter().enumerate().skip(start).enumerate() {
                let Some(row_area) = row_rect(list_inner, row) else {
                    break;
                };
                let path = &finder.files[m.file];
                let budget = (list_inner.width as usize).saturating_sub(2);
                let shown = truncate(path, budget);
                // Truncation puts `…` at the last char of `shown`; a match
                // landing on that index must not light the ellipsis.
                let shown_len = shown.chars().count();
                let positions = if shown_len < path.chars().count() {
                    let keep = m
                        .positions
                        .iter()
                        .take_while(|&&p| p + 1 < shown_len)
                        .count();
                    &m.positions[..keep]
                } else {
                    &m.positions[..]
                };
                let mut spans = vec![Span::raw(" ")];
                spans.extend(fuzzy_highlight_spans(&shown, positions, th));
                render_row(f, row_area, spans, i == finder.selected, true, th);
            }

            // Write-back (draw works on a clone): rects for mouse
            // hit-testing.
            if let Some(Overlay::Files(fin)) = &mut app.overlay {
                fin.area = area;
                fin.list_area = list_inner;
            }
        }
        Overlay::Grep(view) => {
            let area = centered_rect_pct(f.area(), 88, 76);
            f.render_widget(Clear, area);
            let title = if view.query.chars().count() < crate::grep_search::MIN_QUERY_LEN {
                format!(" Find in files — {} ", view.branch)
            } else if view.truncated {
                format!(
                    " Find in files — {} ({}+ hits) ",
                    view.branch,
                    view.hits.len()
                )
            } else {
                format!(
                    " Find in files — {} ({} hits) ",
                    view.branch,
                    view.hits.len()
                )
            };
            let block = Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(th.accent))
                .title(Span::styled(
                    title,
                    Style::default()
                        .fg(th.accent)
                        .add_modifier(Modifier::BOLD),
                ));
            let inner = block.inner(area);
            f.render_widget(block, area);

            // First row: the always-live grep query.
            if let Some(query_area) = row_rect(inner, 0) {
                let line = if view.query.is_empty() {
                    Line::from(Span::styled(
                        "type to search…",
                        Style::default().fg(th.dim),
                    ))
                } else {
                    Line::from(vec![
                        Span::raw(view.query.clone()),
                        Span::styled("█", Style::default().fg(th.accent)),
                    ])
                };
                f.render_widget(Paragraph::new(line), query_area);
            }
            let list_inner = Rect {
                y: inner.y + 1,
                height: inner.height.saturating_sub(1),
                ..inner
            };

            // Placeholder row: error, too-short query, or an empty result.
            let placeholder = if let Some(err) = &view.error {
                Some(Span::styled(err.clone(), Style::default().fg(th.err)))
            } else if view.query.chars().count() < crate::grep_search::MIN_QUERY_LEN {
                Some(Span::styled(
                    format!(
                        "type at least {} characters to search",
                        crate::grep_search::MIN_QUERY_LEN
                    ),
                    Style::default().fg(th.dim),
                ))
            } else if view.hits.is_empty() {
                Some(Span::styled("no matches", Style::default().fg(th.dim)))
            } else {
                None
            };
            if let (Some(span), Some(row_area)) = (placeholder, row_rect(list_inner, 0)) {
                f.render_widget(Paragraph::new(span), row_area);
            }

            let start = view.window_start(list_inner.height as usize);
            for (row, (i, hit)) in view.hits.iter().enumerate().skip(start).enumerate() {
                let Some(row_area) = row_rect(list_inner, row) else {
                    break;
                };
                let budget = (list_inner.width as usize).saturating_sub(2);
                let loc = format!("{}:{}", hit.path, hit.line);
                let loc_len = loc.chars().count();
                let mut spans = vec![Span::raw(" ")];
                if loc_len + 2 >= budget {
                    spans.push(Span::styled(
                        truncate(&loc, budget),
                        Style::default().fg(th.accent),
                    ));
                } else {
                    spans.push(Span::styled(loc, Style::default().fg(th.accent)));
                    spans.push(Span::raw("  "));
                    spans.push(Span::raw(truncate(&hit.text, budget - loc_len - 2)));
                }
                render_row(f, row_area, spans, i == view.selected, true, th);
            }

            // Write-back (draw works on a clone): rects for mouse
            // hit-testing.
            if let Some(Overlay::Grep(v)) = &mut app.overlay {
                v.area = area;
                v.list_area = list_inner;
            }
        }
        Overlay::Tree(view) => {
            let area = centered_rect_pct(f.area(), 92, 90);
            f.render_widget(Clear, area);
            // Cap first, floor second: on a tiny screen the tree keeps its
            // minimum and Min(20) squeezes the preview pane instead.
            let files_w = view
                .files_width
                .min(area.width.saturating_sub(crate::app::MIN_DIFF_PANE_W))
                .max(crate::app::MIN_DIFF_FILES_W);
            let [tree_a, preview_a] =
                Layout::horizontal([Constraint::Length(files_w), Constraint::Min(20)]).areas(area);

            // Left: the file tree; a stateless follow-window keeps the
            // selected row visible.
            let tree_title = if view.filter.is_empty() {
                format!("Tree — {} ({})", view.branch, view.file_count)
            } else {
                format!(
                    "Tree — {} ({}/{})",
                    view.branch, view.match_count, view.file_count
                )
            };
            let block = panel_block(&tree_title, true, th);
            let tree_inner = block.inner(tree_a);
            f.render_widget(block, tree_a);

            // First row: the always-on fuzzy filter input.
            if let Some(filter_area) = row_rect(tree_inner, 0) {
                let line = if view.filter.is_empty() {
                    Line::from(Span::styled(
                        "type to filter…",
                        Style::default().fg(th.dim),
                    ))
                } else {
                    Line::from(vec![
                        Span::raw(view.filter.clone()),
                        Span::styled("█", Style::default().fg(th.accent)),
                    ])
                };
                f.render_widget(Paragraph::new(line), filter_area);
            }
            let list_inner = Rect {
                y: tree_inner.y + 1,
                height: tree_inner.height.saturating_sub(1),
                ..tree_inner
            };

            if view.rows.is_empty() {
                if let Some(row_area) = row_rect(list_inner, 0) {
                    f.render_widget(
                        Paragraph::new(Span::styled(
                            "no matches",
                            Style::default().fg(th.dim),
                        )),
                        row_area,
                    );
                }
            }
            let start = view.window_start(list_inner.height as usize);
            for (row, (i, r)) in view.rows.iter().enumerate().skip(start).enumerate() {
                let Some(row_area) = row_rect(list_inner, row) else {
                    break;
                };
                let node = &view.nodes[r.node];
                let indent = "  ".repeat(node.depth);
                // Directories fold; a live filter forces them all open.
                let marker = if !node.is_dir {
                    "  "
                } else if !view.filter.is_empty() || view.expanded[r.node] {
                    "▾ "
                } else {
                    "▸ "
                };
                let budget = (list_inner.width as usize)
                    .saturating_sub(indent.chars().count() + 3);
                let shown = truncate(&node.name, budget);
                // Truncation puts `…` at the last char of `shown`; a match
                // landing on that index must not light the ellipsis.
                let shown_len = shown.chars().count();
                let positions = if shown_len < node.name.chars().count() {
                    let keep = r
                        .positions
                        .iter()
                        .take_while(|&&p| p + 1 < shown_len)
                        .count();
                    &r.positions[..keep]
                } else {
                    &r.positions[..]
                };
                let mut spans = vec![
                    Span::raw(format!(" {indent}")),
                    Span::styled(marker, Style::default().fg(th.accent)),
                ];
                if node.is_dir {
                    spans.push(Span::styled(shown, Style::default().fg(th.accent)));
                } else {
                    spans.extend(fuzzy_highlight_spans(&shown, positions, th));
                }
                render_row(f, row_area, spans, i == view.selected, true, th);
            }

            // Right: the selected node's preview, syntax-highlighted and
            // scrolled — or the embedded editor, which draw_vim paints into
            // this pane after us.
            let editing = app.vim.as_ref().is_some_and(|v| v.embedded);
            let sel_path = view.selected_node().map(|n| n.path.as_str()).unwrap_or("");
            let title = if editing {
                format!(
                    "{} — editing",
                    truncate(sel_path, (preview_a.width as usize).saturating_sub(14))
                )
            } else {
                truncate(sel_path, (preview_a.width as usize).saturating_sub(4))
            };
            let mut block = panel_block(&title, true, th);
            let preview_inner = block.inner(preview_a);
            let max_scroll =
                (view.preview_line_count as u16).saturating_sub(preview_inner.height.max(1));
            let scroll = view.scroll.min(max_scroll);
            if !editing && max_scroll > 0 {
                block = block.title_bottom(
                    Line::from(Span::styled(
                        format!(" {}/{} ", scroll + 1, view.preview_line_count),
                        Style::default().fg(th.dim),
                    ))
                    .right_aligned(),
                );
            }
            f.render_widget(block, preview_a);
            if !editing {
                let lines: Vec<Line> = view
                    .preview_lines
                    .iter()
                    .skip(scroll as usize)
                    .take(preview_inner.height as usize)
                    .map(|runs| {
                        Line::from(
                            runs.iter()
                                .map(|(kind, text)| {
                                    Span::styled(text.clone(), token_style(*kind, th))
                                })
                                .collect::<Vec<_>>(),
                        )
                    })
                    .collect();
                f.render_widget(Paragraph::new(lines), preview_inner);
            }

            // Write-back (draw works on a clone): page size for key paging,
            // scroll re-clamped so resizes never strand the view, preview
            // rect for the embedded editor.
            if let Some(Overlay::Tree(v)) = &mut app.overlay {
                v.view_height = preview_inner.height;
                v.scroll = scroll;
                v.list_area = list_inner;
                v.preview_area = preview_inner;
                v.area = area;
                v.files_width = files_w;
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

/// Bordered panel frame. Focus has to be unmissable, so the focused panel
/// differs in shape as well as color: a THICK border and a solid
/// accent-background title chip, versus a thin dim border and plain title.
fn panel_block(title: &str, focused: bool, th: Theme) -> Block<'_> {
    if focused {
        Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Thick)
            .border_style(Style::default().fg(th.accent))
            .title(Span::styled(
                format!(" {title} "),
                Style::default()
                    .fg(th.on_accent)
                    .bg(th.accent)
                    .add_modifier(Modifier::BOLD),
            ))
    } else {
        Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(th.dim))
            .title(Span::styled(
                format!(" {title} "),
                Style::default().fg(th.muted),
            ))
    }
}

fn status_dot(status: Option<AgentStatus>, th: Theme) -> Span<'static> {
    match status {
        Some(AgentStatus::Fresh) => Span::styled("● ", Style::default().fg(th.dim)),
        Some(AgentStatus::Running) => Span::styled("● ", Style::default().fg(th.warn)),
        Some(AgentStatus::Finished) => Span::styled("● ", Style::default().fg(th.ok)),
        Some(AgentStatus::NeedsFeedback) => Span::styled("● ", Style::default().fg(th.err)),
        Some(AgentStatus::Terminated) => Span::styled("● ", Style::default().fg(th.special)),
        Some(AgentStatus::Disconnected) => Span::styled("○ ", Style::default().fg(th.dim)),
        None => Span::styled("○ ", Style::default().fg(th.dim)),
    }
}

/// Base style for a whole list row. Selection reads as a full-width bar so
/// the active project → worktree → session path stays visible at a glance:
/// bright (reversed) in the focused panel, dim gray in unfocused panels.
fn row_bar(selected: bool, focused: bool, th: Theme) -> Style {
    if selected && focused {
        Style::default().add_modifier(Modifier::REVERSED | Modifier::BOLD)
    } else if selected {
        Style::default()
            .bg(th.dim)
            .fg(th.text)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default()
    }
}

/// Render one list row as a full-width bar. Dim spans (idle dots,
/// archived names) would vanish against the unfocused-selection bar, so
/// they get lifted to muted there.
fn render_row(
    f: &mut Frame,
    area: Rect,
    mut spans: Vec<Span>,
    selected: bool,
    focused: bool,
    th: Theme,
) {
    if selected && !focused {
        for s in &mut spans {
            if s.style.fg == Some(th.dim) {
                s.style.fg = Some(th.muted);
            }
        }
    }
    f.render_widget(
        Paragraph::new(Line::from(spans)).style(row_bar(selected, focused, th)),
        area,
    );
}

fn draw_projects(f: &mut Frame, app: &mut App, area: Rect) {
    let th = app.theme;
    let focused = app.focus == Focus::Projects;
    let block = panel_block("📁 Projects", focused, th);
    let inner = block.inner(area);
    f.render_widget(block, area);

    if app.tree.projects.is_empty() {
        f.render_widget(
            Paragraph::new(vec![
                Line::from(Span::styled(
                    "No projects yet",
                    Style::default().fg(th.dim),
                )),
                Line::from(Span::styled(
                    "n: add project",
                    Style::default().fg(th.dim),
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
            ProjectRow::Divider { project, before } => {
                let p = &app.tree.projects[project];
                let label = if before {
                    p.divider_before_label.clone()
                } else {
                    p.divider_label.clone()
                }
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
                status_dot(*roll, th),
                Span::raw(truncate(text, inner.width.saturating_sub(2) as usize)),
            ],
            ProjectRow::Divider { .. } => divider_spans(text, inner.width, th),
        };
        render_row(f, row_area, spans, row_idx == app.sel_project, focused, th);
        app.hits.push((row_area, HitTarget::Project(row_idx)));
    }
    app.hits.push((inner, HitTarget::PanelBg(Focus::Projects)));
}

/// A divider line, with the group label woven in when present:
/// `─ label ────────`.
fn divider_spans(label: &str, width: u16, th: Theme) -> Vec<Span<'static>> {
    let w = width as usize;
    let dim = Style::default().fg(th.dim);
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
                .fg(th.muted)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(format!(" {}", "─".repeat(tail)), dim),
    ]
}

fn draw_worktrees(f: &mut Frame, app: &mut App, area: Rect) {
    let th = app.theme;
    let focused = app.focus == Focus::Worktrees;
    let mut block = panel_block("🌳 Worktrees", focused, th);
    // Bottom badge: the selected checkout's changed-file count, so a dirty
    // worktree (or root) is obvious at a glance. Clean stays quiet.
    if !app.divider_focused() {
        if let Some(n) = app.selected_worktree_changes().filter(|n| *n > 0) {
            block = block.title_bottom(Line::from(Span::styled(
                format!(" {n} changed file{} ", if n == 1 { "" } else { "s" }),
                Style::default().fg(th.warn),
            )));
        }
    }
    let inner = block.inner(area);
    f.render_widget(block, area);

    // A selected separator has nothing underneath it: keep the panel, hide
    // the rows (the terminal pane carries the hint).
    if app.divider_focused() {
        app.hits.push((inner, HitTarget::PanelBg(Focus::Worktrees)));
        return;
    }

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
            Paragraph::new(hint).style(Style::default().fg(th.dim)),
            inner,
        );
        app.hits.push((inner, HitTarget::PanelBg(Focus::Worktrees)));
        return;
    }

    // The main checkout renders as `branch ⌂ root` (dim badge — the branch
    // is live, the badge marks root-ness) with a rule separating it from the
    // true worktrees below, so rows after it sit one screen line lower.
    const ROOT_BADGE: &str = " ⌂ root";
    // Group headers only appear once something is pinned; otherwise the
    // list stays flat (same idiom as the sessions panel).
    let (pinned_count, _) = app.worktree_group_counts();
    let grouped = pinned_count > 0;
    let dim = Style::default().fg(th.dim);
    let mut screen_row = 0usize;
    let header = |f: &mut Frame, text: String, screen_row: &mut usize| {
        if let Some(r) = row_rect(inner, *screen_row) {
            f.render_widget(Paragraph::new(Span::styled(text, dim)), r);
            *screen_row += 1;
        }
    };
    if grouped {
        header(f, "PINNED".into(), &mut screen_row);
    }
    for (i, (branch, is_main, roll)) in worktrees.iter().enumerate() {
        if grouped && i == pinned_count {
            header(f, "UNPINNED".into(), &mut screen_row);
        }
        let Some(row_area) = row_rect(inner, screen_row) else {
            break;
        };
        let mut spans = vec![status_dot(*roll, th)];
        if *is_main {
            let max = (inner.width as usize).saturating_sub(2 + ROOT_BADGE.chars().count());
            spans.push(Span::raw(truncate(branch, max)));
            spans.push(Span::styled(
                ROOT_BADGE,
                Style::default().fg(th.dim),
            ));
        } else {
            spans.push(Span::raw(truncate(
                branch,
                inner.width.saturating_sub(2) as usize,
            )));
        }
        render_row(f, row_area, spans, i == app.sel_worktree, focused, th);
        app.hits.push((row_area, HitTarget::Worktree(i)));
        screen_row += 1;
        // The root rule belongs to the flat list; group headers already
        // separate the rows once something is pinned.
        if !grouped && *is_main && worktrees.len() > 1 {
            if let Some(r) = row_rect(inner, screen_row) {
                f.render_widget(
                    Paragraph::new(Span::styled(
                        "─".repeat(inner.width as usize),
                        Style::default().fg(th.dim),
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
    let th = app.theme;
    let focused = app.focus == Focus::Sessions;
    let block = panel_block("🤖 Sessions", focused, th);
    let inner = block.inner(area);
    f.render_widget(block, area);

    // A selected separator has nothing underneath it: keep the panel, hide
    // the rows (the terminal pane carries the hint).
    if app.divider_focused() {
        app.hits.push((inner, HitTarget::PanelBg(Focus::Sessions)));
        return;
    }

    let rows = app.visible_session_rows();
    let (pinned_count, recent_count, unpinned_count, archived_count) = app.session_group_counts();
    let active_count = pinned_count + recent_count + unpinned_count;
    let terminal_count = rows
        .iter()
        .filter(|r| matches!(r, SessionRow::Terminal(_)))
        .count();
    let dim = Style::default().fg(th.dim);

    let mut screen_row: usize = 0;
    let header = |f: &mut Frame, text: String, screen_row: &mut usize| {
        if let Some(r) = row_rect(inner, *screen_row) {
            f.render_widget(Paragraph::new(Span::styled(text, dim)), r);
            *screen_row += 1;
        }
    };

    // Group headers only appear once something is pinned or recent;
    // otherwise the list stays flat with no group header.
    let grouped = pinned_count > 0 || recent_count > 0;
    if pinned_count > 0 {
        header(f, "PINNED".into(), &mut screen_row);
        for (i, row) in rows.iter().enumerate().take(pinned_count) {
            let Some(r) = row_rect(inner, screen_row) else {
                break;
            };
            draw_session_row(f, app, r, i, row, focused, inner.width);
            screen_row += 1;
        }
    }
    if recent_count > 0 {
        header(f, "RECENT".into(), &mut screen_row);
        for (i, row) in rows
            .iter()
            .enumerate()
            .skip(pinned_count)
            .take(recent_count)
        {
            let Some(r) = row_rect(inner, screen_row) else {
                break;
            };
            draw_session_row(f, app, r, i, row, focused, inner.width);
            screen_row += 1;
        }
    }
    if grouped && unpinned_count > 0 {
        header(f, "UNPINNED".into(), &mut screen_row);
    }
    for (i, row) in rows
        .iter()
        .enumerate()
        .skip(pinned_count + recent_count)
        .take(unpinned_count)
    {
        let Some(r) = row_rect(inner, screen_row) else {
            break;
        };
        draw_session_row(f, app, r, i, row, focused, inner.width);
        screen_row += 1;
    }
    if terminal_count > 0 {
        header(f, "TERMINALS".into(), &mut screen_row);
        for (i, row) in rows
            .iter()
            .enumerate()
            .skip(active_count)
            .take(terminal_count)
        {
            let Some(r) = row_rect(inner, screen_row) else {
                break;
            };
            draw_session_row(f, app, r, i, row, focused, inner.width);
            screen_row += 1;
        }
    }
    if archived_count > 0 {
        header(f, "─".repeat(inner.width as usize), &mut screen_row);
        if app.show_archived {
            header(f, format!("ARCHIVED ({archived_count})"), &mut screen_row);
            for (i, row) in rows
                .iter()
                .enumerate()
                .skip(active_count + terminal_count)
            {
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
    row: &SessionRow,
    focused: bool,
    width: u16,
) {
    let th = app.theme;
    let spans = match row {
        SessionRow::Agent(a) => {
            let dot = if a.archived {
                Span::styled("⊘ ", Style::default().fg(th.dim))
            } else {
                status_dot(Some(a.status), th)
            };
            let name_style = if a.archived {
                Style::default().fg(th.dim)
            } else {
                Style::default()
            };
            // Non-default CLIs get a dim badge (same idiom as the worktree
            // root row).
            let badge = match a.kind {
                nebula_core::AgentKind::Claude => None,
                nebula_core::AgentKind::Codex => Some(" codex"),
                nebula_core::AgentKind::Cursor => Some(" cursor"),
            };
            let name_max =
                (width.saturating_sub(2) as usize).saturating_sub(badge.map_or(0, str::len));
            let mut spans = vec![dot, Span::styled(truncate(&a.name, name_max), name_style)];
            if let Some(badge) = badge {
                spans.push(Span::styled(badge, Style::default().fg(th.dim)));
            }
            spans
        }
        SessionRow::Terminal(t) => {
            // Shell prompt glyph instead of a status dot; dim once the
            // shell has exited (re-attach respawns it).
            let glyph_color = if t.alive { th.ok } else { th.dim };
            vec![
                Span::styled("❯ ", Style::default().fg(glyph_color)),
                Span::raw(truncate(&t.name, width.saturating_sub(2) as usize)),
            ]
        }
    };
    render_row(f, area, spans, index == app.sel_session, focused, th);
    app.hits.push((area, HitTarget::Session(index)));
}

fn draw_terminal(f: &mut Frame, app: &mut App, area: Rect) {
    let th = app.theme;
    let focused = app.focus == Focus::Terminal;
    // A selected separator has no session behind it: keep the pane, swap
    // the content for a hint. The attachment itself stays live so walking
    // the list across a divider doesn't churn detach/attach.
    if app.divider_focused() {
        let block = panel_block("Terminal", focused, th);
        let inner = block.inner(area);
        f.render_widget(block, area);
        app.term_area = inner;
        let msg = Paragraph::new(vec![
            Line::from(""),
            Line::from(Span::styled(
                "you're focused on a separator",
                Style::default().fg(th.muted).add_modifier(Modifier::BOLD),
            )),
            Line::from(""),
            Line::from(Span::styled(
                "select a project to see its worktrees and sessions",
                Style::default().fg(th.dim),
            )),
        ])
        .centered();
        f.render_widget(msg, inner);
        app.term_links = Vec::new();
        app.term_file_links = Vec::new();
        return;
    }
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
    let block = panel_block(&title, focused, th);
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
            (
                crate::links::visible_links(term.parser.screen()),
                crate::links::visible_file_links(term.parser.screen()),
            )
        }
        None => {
            let msg = Paragraph::new(vec![
                Line::from(""),
                Line::from(Span::styled(
                    "nebula",
                    Style::default()
                        .fg(th.accent)
                        .add_modifier(Modifier::BOLD),
                )),
                Line::from(""),
                Line::from(Span::styled(
                    "select a session and press Enter to attach",
                    Style::default().fg(th.dim),
                )),
            ])
            .centered();
            f.render_widget(msg, inner);
            (Vec::new(), Vec::new())
        }
    };
    let (links, file_links) = links;
    // Underline detected URLs and file paths so ⌥click has a visible
    // affordance; kept on the App for click-time hit-testing against the
    // drawn frame.
    let underline = Style::default().add_modifier(Modifier::UNDERLINED);
    let segments = links
        .iter()
        .flat_map(|l| l.segments.iter())
        .chain(file_links.iter().flat_map(|l| l.segments.iter()));
    for &(row, c0, c1) in segments {
        let seg = Rect::new(inner.x + c0, inner.y + row, c1 - c0 + 1, 1).intersection(inner);
        f.buffer_mut().set_style(seg, underline);
    }
    app.term_links = links;
    app.term_file_links = file_links;
}

fn attached_session_name(app: &App) -> Option<String> {
    match &app.term.as_ref()?.sref {
        SessionRef::Agent(id) => app
            .tree
            .agents
            .iter()
            .find(|a| &a.id == id)
            .map(|a| a.name.clone()),
        SessionRef::Terminal(id) => app
            .tree
            .terminals
            .iter()
            .find(|t| &t.id == id)
            .map(|t| t.name.clone()),
    }
}

/// `project ▸ branch ▸ session` breadcrumb of the current selection; the
/// segment matching the focused panel is highlighted. Sessions/Terminal
/// focus both highlight the session segment.
fn breadcrumb(app: &App) -> Vec<Span<'static>> {
    let th = app.theme;
    let seg = |name: &str, active: bool| {
        Span::styled(
            truncate(name, 20),
            if active {
                Style::default()
                    .fg(th.accent)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(th.muted)
            },
        )
    };
    let sep = || Span::styled(" ▸ ", Style::default().fg(th.dim));

    let mut spans = Vec::new();
    // A focused separator has no worktree/session context to spell out —
    // the crumb is the separator itself.
    if let Some(ProjectRow::Divider { project, before }) = app.selected_project_row() {
        let p = &app.tree.projects[project];
        let label = if before {
            p.divider_before_label.as_deref()
        } else {
            p.divider_label.as_deref()
        };
        spans.push(seg(
            &format!("─ {} ─", label.unwrap_or("separator")),
            app.focus == Focus::Projects,
        ));
        return spans;
    }
    let Some(project) = app.selected_project() else {
        return spans;
    };
    spans.push(seg(&project.name, app.focus == Focus::Projects));
    if let Some(worktree) = app.selected_worktree() {
        spans.push(sep());
        spans.push(seg(&worktree.branch, app.focus == Focus::Worktrees));
        if let Some(session) = app.selected_session_row() {
            spans.push(sep());
            spans.push(seg(
                session.name(),
                matches!(app.focus, Focus::Sessions | Focus::Terminal),
            ));
        }
    }
    spans
}

fn draw_footer(f: &mut Frame, app: &App, area: Rect) {
    let th = app.theme;
    let conn = match app.conn {
        ConnState::Connected => Span::styled("⏻ connected", Style::default().fg(th.ok)),
        ConnState::Disconnected => Span::styled("✗ disconnected", Style::default().fg(th.err)),
    };
    let hints = if let Some(flash) = &app.flash {
        Span::styled(flash.clone(), Style::default().fg(th.warn))
    } else if app.vim.is_some() {
        Span::styled(
            ":wq / :q to finish  Ctrl+Q: force close",
            Style::default().fg(th.dim),
        )
    } else if matches!(&app.overlay, Some(Overlay::Grep(_))) {
        Span::styled(
            "type: search  ↑/↓: move  Enter: edit in vim  Ctrl+u: clear  Esc: clear/close",
            Style::default().fg(th.dim),
        )
    } else if matches!(&app.overlay, Some(Overlay::Diff(_))) {
        Span::styled(
            "type: filter  ↑/↓: file  ⇧↑/↓: scroll  Ctrl+d/u: page  Ctrl+u: clear filter  Esc: clear/close",
            Style::default().fg(th.dim),
        )
    } else if matches!(&app.overlay, Some(Overlay::Tree(_))) {
        Span::styled(
            "type: filter  ↑/↓: move  ←/→: fold  Enter: open/edit  ⇧↑/↓: scroll  Ctrl+u: clear filter  Esc: clear/close",
            Style::default().fg(th.dim),
        )
    } else if matches!(&app.overlay, Some(Overlay::Files(_))) {
        Span::styled(
            "type: search  ↑/↓: move  Enter: edit in vim  Ctrl+y: copy path  Ctrl+u: clear  Esc: clear/close",
            Style::default().fg(th.dim),
        )
    } else if matches!(&app.overlay, Some(Overlay::Palette(_))) {
        Span::styled(
            "type: search  ↑/↓: move  Enter: open  Ctrl+u: clear  Esc: clear/close",
            Style::default().fg(th.dim),
        )
    } else if matches!(&app.overlay, Some(Overlay::Settings(_))) {
        Span::styled(
            "↑/↓: move  Enter/Space: toggle  h/l: cycle  Esc: close",
            Style::default().fg(th.dim),
        )
    } else if app.overlay.is_some() {
        Span::styled(
            "Esc: close  Enter: confirm",
            Style::default().fg(th.dim),
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
                Some(ProjectRow::Divider { .. }) => {
                    "Enter/r: label  d: delete divider  ⇧J/K: move  m: menu  ?: help"
                }
                _ => "n: add  d: remove  -: divider  ⇧J/K: move  /: search  m: menu  ?: help",
            },
            Focus::Worktrees => {
                "n: new worktree  T: terminal  p: pin  d: delete  /: search  m: menu  ?: help"
            }
            Focus::Sessions => {
                "↵: focus  n: agent  T: terminal  r: rename  p: pin  a: archive  d: del  m: menu  ?: help"
            }
        };
        Span::styled(text, Style::default().fg(th.dim))
    };
    let host_style = if app.is_remote {
        Style::default()
            .fg(th.warn)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(th.dim)
    };
    let mut spans = vec![
        Span::styled(truncate(&app.hostname, 24), host_style),
        Span::raw("  │  "),
        conn,
        Span::raw("  │  "),
    ];
    let crumbs = breadcrumb(app);
    if !crumbs.is_empty() {
        spans.extend(crumbs);
        spans.push(Span::raw("  │  "));
    }
    spans.push(hints);
    f.render_widget(Paragraph::new(Line::from(spans)), area);
}

/// Style for one syntax-highlight token kind of the tree-browser preview
/// (classification lives in syntax.rs, the `classify_diff_line` split).
fn token_style(kind: crate::syntax::TokenKind, th: Theme) -> Style {
    use crate::syntax::TokenKind;
    match kind {
        TokenKind::Keyword => Style::default().fg(th.special),
        TokenKind::String => Style::default().fg(th.ok),
        TokenKind::Comment => Style::default().fg(th.dim),
        TokenKind::Number => Style::default().fg(th.warn),
        TokenKind::Text => Style::default(),
    }
}

/// Split a (possibly truncated) path into spans, lighting the chars the
/// fuzzy filter matched. `positions` are ascending char indices into the
/// untruncated path; anything cut off by truncation simply isn't lit.
fn fuzzy_highlight_spans(shown: &str, positions: &[usize], th: Theme) -> Vec<Span<'static>> {
    if positions.is_empty() {
        return vec![Span::raw(shown.to_string())];
    }
    let hl = Style::default()
        .fg(th.accent)
        .add_modifier(Modifier::BOLD);
    let mut spans = Vec::new();
    let mut run = String::new();
    let mut run_hl = false;
    let push = |text: String, lit: bool, spans: &mut Vec<Span<'static>>| {
        if !text.is_empty() {
            spans.push(if lit {
                Span::styled(text, hl)
            } else {
                Span::raw(text)
            });
        }
    };
    for (i, c) in shown.chars().enumerate() {
        let lit = positions.binary_search(&i).is_ok();
        if lit != run_hl {
            push(std::mem::take(&mut run), run_hl, &mut spans);
            run_hl = lit;
        }
        run.push(c);
    }
    push(run, run_hl, &mut spans);
    spans
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
