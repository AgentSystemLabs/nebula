//! View layer: draws the three panels + terminal pane + footer, and records
//! hit regions for mouse interaction.

use crate::app::{
    App, ConnState, Focus, HitTarget, Overlay, PaletteTarget, ProjectRow, SessionRow,
};
use crate::git_diff::{classify_diff_line, DiffLineKind};
use crate::theme::Theme;
use nebula_core::{AgentStatus, SessionRef};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
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
        if app.focus_tint && app.focus == Focus::Terminal {
            draw_focus_tint(f.buffer_mut(), body, app.theme);
        }
        draw_footer(f, app, footer);
        draw_overlay(f, app);
        draw_vim(f, app);
        return;
    }

    // First run (or an empty workspace): no visible projects means three
    // empty panels, so the whole body becomes the animated nebula splash
    // until the first project lands. N summons the same splash as a
    // dismissable preview.
    if !app.tree.has_visible_projects() || app.splash_preview {
        crate::splash::draw_splash(f, app, body);
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
    draw_splitter_grips(f.buffer_mut(), app, body);
    // Focus cue (opt-in, `focus_tint` setting): the focused panel's whole
    // background picks up a faint accent tint. The sidebar columns stop
    // one cell short of their right rule so the tint stays inside the
    // panel.
    if app.focus_tint {
        let tinted = match app.focus {
            Focus::Projects => shrink_r(projects_a),
            Focus::Worktrees => shrink_r(worktrees_a),
            Focus::Sessions => shrink_r(sessions_a),
            Focus::Terminal => term_a,
        };
        draw_focus_tint(f.buffer_mut(), tinted, app.theme);
    }
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
        .border_type(BorderType::Rounded)
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
            let label_w = menu
                .items
                .iter()
                .map(|i| i.label.chars().count())
                .max()
                .unwrap_or(8);
            // Rows that expand into a submenu get a right-aligned ▸ in an
            // extra column so the affordance is visible before hovering.
            let any_submenu = menu.items.iter().any(|i| i.action.submenu().is_some());
            // The workspace switcher carries its key verbs in the bottom
            // border, the notes-modal pattern; the modal widens to fit.
            let hint = menu
                .is_workspace_picker()
                .then_some(" n: new  r: rename  d: delete ");
            let width = (label_w + 4 + if any_submenu { 2 } else { 0 })
                .max(title_width + 2)
                .max(hint.map_or(0, |h| h.chars().count() + 2))
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
                .border_type(BorderType::Rounded)
                .border_style(Style::default().fg(th.accent));
            if let Some(title) = &menu.title {
                block = block.title(Span::styled(
                    format!(" {title} "),
                    Style::default().fg(th.accent).add_modifier(Modifier::BOLD),
                ));
            }
            if let Some(hint) = hint {
                block =
                    block.title_bottom(Line::from(Span::styled(hint, Style::default().fg(th.dim))));
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
                    style = style.bg(th.sel_bg).add_modifier(Modifier::BOLD);
                }
                let text = if item.action.submenu().is_some() {
                    format!(" {:<label_w$} ▸ ", item.label)
                } else if any_submenu {
                    format!(" {:<label_w$}   ", item.label)
                } else {
                    format!(" {} ", item.label)
                };
                f.render_widget(Paragraph::new(Span::styled(text, style)), row);
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
                .border_type(BorderType::Rounded)
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
            // Path prompts get a wide dialog with the live directory
            // listing between the input and the hint; the dialog grows to
            // fit the listing (at least one row, for the empty message).
            let is_path = prompt.completes_paths();
            let width = if is_path { 72 } else { 52 };
            let list_h = if is_path {
                prompt.dirs.len().clamp(1, 8) as u16
            } else {
                0
            };
            let area = centered_rect(f.area(), width, 6 + list_h);
            f.render_widget(Clear, area);
            let block = Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .border_style(Style::default().fg(th.accent))
                .title(Span::styled(
                    format!(" {} ", prompt.title),
                    Style::default().fg(th.accent),
                ));
            let inner = block.inner(area);
            f.render_widget(block, area);

            // Row 0: the label, with the listing size tucked after it.
            if let Some(r) = row_rect(inner, 0) {
                let mut spans = vec![Span::styled(
                    prompt.label.clone(),
                    Style::default().fg(th.dim),
                )];
                if prompt.dirs.len() > list_h as usize {
                    spans.push(Span::styled(
                        format!("  ·  {} dirs", prompt.dirs.len()),
                        Style::default().fg(th.dim),
                    ));
                }
                f.render_widget(Paragraph::new(Line::from(spans)), r);
            }

            // Row 1: the input. Long paths show the tail so the cursor
            // stays visible; the cursor dims while a listing row is
            // highlighted (Enter takes the highlight, not the text).
            if let Some(r) = row_rect(inner, 1) {
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
                let cursor = if prompt.hover.is_some() {
                    th.dim
                } else {
                    th.text
                };
                f.render_widget(
                    Paragraph::new(Line::from(vec![
                        Span::raw("> "),
                        Span::styled(shown_input, Style::default().fg(th.text)),
                        Span::styled("█", Style::default().fg(cursor)),
                    ])),
                    r,
                );
            }

            // The listing: one raised-fill row per directory, a ● on git
            // repos, the typed partial lit like a fuzzy match. A stateless
            // follow-window keeps the highlighted row visible.
            let mut list_area = Rect::default();
            if is_path {
                list_area = Rect {
                    x: inner.x,
                    y: inner.y + 2,
                    width: inner.width,
                    height: list_h.min(inner.height.saturating_sub(2)),
                };
                if prompt.dirs.is_empty() {
                    if let Some(r) = row_rect(list_area, 0) {
                        f.render_widget(
                            Paragraph::new(Span::styled(
                                "  no matching directories",
                                Style::default().fg(th.dim),
                            )),
                            r,
                        );
                    }
                }
                let (_, partial) = crate::completion::split_input(&prompt.input);
                let hit = partial.chars().count();
                let start = prompt.window_start(list_area.height as usize);
                for (row, (i, entry)) in prompt.dirs.iter().enumerate().skip(start).enumerate() {
                    let Some(r) = row_rect(list_area, row) else {
                        break;
                    };
                    let marker = if entry.is_repo {
                        Span::styled("● ", Style::default().fg(th.ok))
                    } else {
                        Span::styled("· ", Style::default().fg(th.dim))
                    };
                    let budget = (inner.width as usize).saturating_sub(5);
                    let shown = truncate(&entry.name, budget);
                    let positions: Vec<usize> = (0..hit.min(shown.chars().count())).collect();
                    let mut spans = vec![Span::raw(" "), marker];
                    spans.extend(fuzzy_highlight_spans(&shown, &positions, th));
                    spans.push(Span::styled("/", Style::default().fg(th.dim)));
                    render_row(f, r, spans, prompt.hover == Some(i), true, th);
                }
            }

            // Bottom row: the key hints.
            if let Some(r) = row_rect(inner, (3 + list_h) as usize) {
                let hint = if is_path {
                    "[Enter] add  [↓↑] pick  [→] open  [←] up  [Tab] complete  [Esc] cancel"
                } else {
                    "[Enter] ok   [Ctrl+u] clear   [Esc] cancel"
                };
                f.render_widget(
                    Paragraph::new(Span::styled(hint, Style::default().fg(th.dim))),
                    r,
                );
            }
            // Record the listing rect for click hit-testing.
            if let Some(Overlay::Prompt(p)) = &mut app.overlay {
                p.list_area = list_area;
            }
        }
        Overlay::Help => {
            // Grouped keymap in two columns: reads by task instead of one
            // giant list, and at ~24 rows it fits a stock terminal window
            // (the old single list clipped its tail on short screens).
            type HelpSection = (&'static str, &'static [(&'static str, &'static str)]);
            const LEFT: &[HelpSection] = &[
                (
                    "NAVIGATE & SEARCH",
                    &[
                        ("Tab / ⇧Tab", "cycle focus between panels"),
                        ("←↓↑→ / jkl", "move focus / selection"),
                        ("Enter", "drill in / attach session"),
                        ("/", "fuzzy jump to anything"),
                        ("^o / ^f", "jump pick: open / focus row"),
                        ("f", "find file (^y copies path)"),
                        ("F", "find in files (git grep)"),
                        ("b", "file tree browser"),
                    ],
                ),
                (
                    "PROJECTS",
                    &[
                        ("n / o", "add project (o: from anywhere)"),
                        ("e", "project-level notes"),
                        ("⇧J/K", "reorder project"),
                        ("-", "divider below (Enter/r: label)"),
                        ("d / ⌫", "remove from list"),
                    ],
                ),
                (
                    "WORKTREES",
                    &[
                        ("n", "new worktree"),
                        ("e", "notes for the worktree"),
                        ("g", "git diff (^r: mark reviewed ✓)"),
                        ("p", "pin / unpin"),
                        ("d / D", "delete one / delete all"),
                    ],
                ),
            ];
            const RIGHT: &[HelpSection] = &[
                (
                    "SESSIONS",
                    &[
                        ("n", "new agent (pick CLI kind)"),
                        ("t", "new shell terminal"),
                        ("Enter", "attach + lock input"),
                        ("r", "rename agent / terminal"),
                        ("p", "pin / unpin"),
                        ("a / u", "archive / unarchive (A: show/hide)"),
                        ("m", "context menu (right-click)"),
                        ("d / D", "delete one / delete all"),
                    ],
                ),
                (
                    "TERMINAL & MOUSE",
                    &[
                        ("Enter / z", "lock input (z: full-screen)"),
                        ("^q / ^]", "unlock, back to panels"),
                        ("drag", "select + copy (2×click: word)"),
                        ("⌥click", "open URL / file under cursor"),
                        ("⇧drag", "select via your terminal"),
                        ("drag border", "resize panels"),
                    ],
                ),
                (
                    "GENERAL",
                    &[
                        ("w", "workspaces: switch (n/r/d manage)"),
                        ("h", "ssh hosts: connect (a: new, d: del)"),
                        ("s", "settings"),
                        ("M", "memory usage (nebula + agents)"),
                        ("N", "nebula splash (any key returns)"),
                        ("^u", "clear typed input"),
                        ("q / ?", "quit / toggle this help"),
                    ],
                ),
            ];
            // Rows a column needs: each section is a header plus its
            // entries, with a blank line between sections.
            let rows = |sections: &[HelpSection]| -> u16 {
                sections
                    .iter()
                    .map(|(_, entries)| entries.len() as u16 + 1)
                    .sum::<u16>()
                    + sections.len().saturating_sub(1) as u16
            };
            let height = rows(LEFT).max(rows(RIGHT)) + 2;
            let area = centered_rect(f.area(), 92, height);
            f.render_widget(Clear, area);
            let block = Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .border_style(Style::default().fg(th.accent))
                .title(" Help ");
            let inner = block.inner(area);
            f.render_widget(block, area);
            let [left_a, right_a] =
                Layout::horizontal([Constraint::Percentage(50), Constraint::Percentage(50)])
                    .areas(inner);
            let column = |sections: &[HelpSection], width: u16| -> Vec<Line> {
                let mut lines = Vec::new();
                for (i, (title, entries)) in sections.iter().enumerate() {
                    if i > 0 {
                        lines.push(Line::from(""));
                    }
                    lines.push(Line::from(Span::styled(
                        format!(" {title}"),
                        Style::default().fg(th.muted).add_modifier(Modifier::BOLD),
                    )));
                    for (k, v) in *entries {
                        lines.push(Line::from(vec![
                            Span::styled(format!(" {k:<13}"), Style::default().fg(th.accent)),
                            Span::styled(
                                truncate(v, (width as usize).saturating_sub(15)),
                                Style::default().fg(th.dim),
                            ),
                        ]));
                    }
                }
                lines
            };
            f.render_widget(Paragraph::new(column(LEFT, left_a.width)), left_a);
            f.render_widget(Paragraph::new(column(RIGHT, right_a.width)), right_a);
        }
        Overlay::Settings(view) => {
            // Grouped like the Help overlay: titled sections instead of one
            // flat list. Each setting is a single label+value row; the
            // selected row's hint shows in the footer, which keeps the
            // modal short enough for a stock terminal as settings grow.
            let cfg = crate::config::Config::load();
            let rows = crate::config::settings_rows();
            // Body rows, then footer: blank + hint + keys + path.
            let height = rows.len() as u16 + 4 + 2;
            let area = centered_rect(f.area(), 80, height);
            f.render_widget(Clear, area);
            let block = Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .border_style(Style::default().fg(th.accent))
                .title(Span::styled(
                    " Settings ",
                    Style::default().fg(th.accent).add_modifier(Modifier::BOLD),
                ));
            let inner = block.inner(area);
            f.render_widget(block, area);

            let dim = Style::default().fg(th.dim);
            let mut lines: Vec<Line> = Vec::new();
            for row in &rows {
                match row {
                    crate::config::SettingsRow::Blank => lines.push(Line::from("")),
                    crate::config::SettingsRow::Header(title) => {
                        lines.push(Line::from(Span::styled(
                            format!(" {title}"),
                            Style::default().fg(th.muted).add_modifier(Modifier::BOLD),
                        )));
                    }
                    crate::config::SettingsRow::Setting(i) => {
                        let spec = crate::config::setting_at(*i)
                            .expect("settings_rows indexes the flat settings");
                        let value = cfg.value_label(spec.kind);
                        let mut label_style = Style::default();
                        let mut value_style = Style::default().fg(th.accent);
                        if *i == view.selected {
                            label_style = label_style.bg(th.sel_bg).add_modifier(Modifier::BOLD);
                            value_style = value_style.bg(th.sel_bg).add_modifier(Modifier::BOLD);
                        }
                        lines.push(Line::from(vec![
                            Span::styled(format!("   {:<28}", spec.label), label_style),
                            Span::styled(format!("[{value}]"), value_style),
                        ]));
                    }
                }
            }
            lines.push(Line::from(""));
            let hint = crate::config::setting_at(view.selected)
                .map(|s| s.hint)
                .unwrap_or("");
            lines.push(Line::from(Span::styled(
                truncate(&format!(" {hint}"), inner.width as usize),
                dim,
            )));
            lines.push(Line::from(Span::styled(
                " Enter/Space: toggle   h/l: cycle   Esc: close",
                dim,
            )));
            let path = nebula_core::paths::config_path();
            lines.push(Line::from(Span::styled(
                truncate(&format!(" {}", path.display()), inner.width as usize),
                dim,
            )));
            f.render_widget(Paragraph::new(lines), inner);
            if let Some(Overlay::Settings(v)) = &mut app.overlay {
                v.area = area;
            }
        }
        Overlay::Metrics(view) => {
            // One row per live session (biggest first), then nebula's own
            // two processes; above them, a rollup per agent kind so "how
            // much is claude using?" reads off in one line.
            struct Row {
                name: String,
                context: String,
                pid: u32,
                procs: u32,
                bytes: u64,
                /// None = one of nebula's own processes (not openable).
                sref: Option<SessionRef>,
            }
            let mut rows: Vec<Row> = Vec::new();
            // kind label → (session count, procs, bytes); BTreeMap for a
            // stable claude / codex / cursor / shells order.
            let mut kinds: std::collections::BTreeMap<&'static str, (u32, u32, u64)> =
                std::collections::BTreeMap::new();
            let mut sessions_total: u64 = 0;

            // `project/branch` home of a worktree, for the WHERE column.
            let wt_context = |wt_id: &nebula_core::WorktreeId| -> String {
                app.tree
                    .worktrees
                    .iter()
                    .find(|w| &w.id == wt_id)
                    .map(|w| {
                        let project = app
                            .tree
                            .projects
                            .iter()
                            .find(|p| p.id == w.project_id)
                            .map(|p| p.name.as_str())
                            .unwrap_or("?");
                        format!("{project}/{}", w.branch)
                    })
                    .unwrap_or_default()
            };

            if let Some(snap) = &view.snapshot {
                for m in &snap.sessions {
                    let (name, context, kind) = match &m.session {
                        SessionRef::Agent(id) => {
                            let agent = app.tree.agents.iter().find(|a| &a.id == id);
                            let name = agent
                                .map(|a| format!("{} ({})", a.name, a.kind.as_str()))
                                .unwrap_or_else(|| "(unknown agent)".into());
                            let context = agent
                                .map(|a| wt_context(&a.worktree_id))
                                .unwrap_or_default();
                            let kind = agent.map(|a| a.kind.as_str()).unwrap_or("agents");
                            (name, context, kind)
                        }
                        SessionRef::Terminal(id) => {
                            let term = app.tree.terminals.iter().find(|t| &t.id == id);
                            let name = term
                                .map(|t| t.name.clone())
                                .unwrap_or_else(|| "(unknown terminal)".into());
                            let context =
                                term.map(|t| wt_context(&t.worktree_id)).unwrap_or_default();
                            (name, context, "shells")
                        }
                    };
                    let entry = kinds.entry(kind).or_default();
                    entry.0 += 1;
                    entry.1 += m.procs;
                    entry.2 += m.rss_bytes;
                    sessions_total += m.rss_bytes;
                    rows.push(Row {
                        name,
                        context,
                        pid: m.pid,
                        procs: m.procs,
                        bytes: m.rss_bytes,
                        sref: Some(m.session.clone()),
                    });
                }
                rows.sort_by(|a, b| b.bytes.cmp(&a.bytes));
                rows.push(Row {
                    name: "nebula daemon".into(),
                    context: String::new(),
                    pid: snap.daemon_pid,
                    procs: 1,
                    bytes: snap.daemon_rss_bytes,
                    sref: None,
                });
                rows.push(Row {
                    name: "nebula ui (this window)".into(),
                    context: String::new(),
                    pid: std::process::id(),
                    procs: 1,
                    bytes: view.client_rss_bytes,
                    sref: None,
                });
            }

            // The cursor follows the session it was on across refresh
            // re-sorts (sizes move rows around); nebula's own rows sit at
            // fixed positions, so the index fallback covers them.
            let prev = view.rows.get(view.selected).cloned().flatten();
            let selected = prev
                .and_then(|sref| rows.iter().position(|r| r.sref.as_ref() == Some(&sref)))
                .unwrap_or(view.selected)
                .min(rows.len().saturating_sub(1));

            let dim = Style::default().fg(th.dim);
            let header = Style::default().fg(th.muted).add_modifier(Modifier::BOLD);
            let mem_style = Style::default().fg(th.accent);
            let plural = |n: u32| if n == 1 { "" } else { "s" };

            let mut lines: Vec<Line> = Vec::new();
            let mut scroll = 0usize;
            let mut shown = 0usize;
            let mut rows_start = 0usize;
            if let Some(snap) = &view.snapshot {
                // Rollup: one line per agent kind, then nebula, then total.
                for (kind, (n, procs, bytes)) in &kinds {
                    let unit = if *kind == "shells" {
                        "terminal"
                    } else {
                        "session"
                    };
                    let detail =
                        format!("{n} {unit}{} · {procs} proc{}", plural(*n), plural(*procs));
                    lines.push(Line::from(vec![
                        Span::styled(format!(" {kind:<8} "), header),
                        Span::styled(format!("{detail:<42}"), dim),
                        Span::styled(format!("{:>9}", fmt_mem(*bytes)), mem_style),
                    ]));
                }
                let nebula_bytes = snap.daemon_rss_bytes + view.client_rss_bytes;
                lines.push(Line::from(vec![
                    Span::styled(" nebula   ", header),
                    Span::styled(format!("{:<42}", "daemon + this ui"), dim),
                    Span::styled(format!("{:>9}", fmt_mem(nebula_bytes)), mem_style),
                ]));
                let total = sessions_total + nebula_bytes;
                let note = if snap.system_total_bytes > 0 {
                    format!(
                        "{:.1}% of {} installed",
                        100.0 * total as f64 / snap.system_total_bytes as f64,
                        fmt_mem(snap.system_total_bytes)
                    )
                } else {
                    String::new()
                };
                lines.push(Line::from(vec![
                    Span::styled(" total    ", Style::default().add_modifier(Modifier::BOLD)),
                    Span::styled(format!("{note:<42}"), dim),
                    Span::styled(
                        format!("{:>9}", fmt_mem(total)),
                        mem_style.add_modifier(Modifier::BOLD),
                    ),
                ]));
                lines.push(Line::from(""));
                lines.push(Line::from(Span::styled(
                    format!(
                        " {:<28} {:<15} {:>6} {:>5} {:>9}",
                        "SESSION", "WHERE", "PID", "PROCS", "MEM"
                    ),
                    header,
                )));
                // Scrolled window over the rows; everything above stays put.
                let space = f.area().height.saturating_sub(lines.len() as u16 + 4) as usize;
                shown = rows.len().min(16).min(space.max(3));
                scroll = view.scroll.min(rows.len().saturating_sub(shown));
                // Keep the cursor inside the window.
                if selected < scroll {
                    scroll = selected;
                } else if shown > 0 && selected >= scroll + shown {
                    scroll = selected + 1 - shown;
                }
                rows_start = lines.len();
                for (i, row) in rows.iter().enumerate().skip(scroll).take(shown) {
                    let name_style = if row.sref.is_none() {
                        dim
                    } else {
                        Style::default()
                    };
                    let sel = |s: Style| {
                        if i == selected {
                            s.bg(th.sel_bg).add_modifier(Modifier::BOLD)
                        } else {
                            s
                        }
                    };
                    lines.push(Line::from(vec![
                        Span::styled(
                            format!(" {:<28} ", truncate(&row.name, 28)),
                            sel(name_style),
                        ),
                        Span::styled(format!("{:<15} ", truncate(&row.context, 15)), sel(dim)),
                        Span::styled(format!("{:>6} {:>5} ", row.pid, row.procs), sel(dim)),
                        Span::styled(format!("{:>9}", fmt_mem(row.bytes)), sel(mem_style)),
                    ]));
                }
                if rows.len() > shown {
                    lines.push(Line::from(Span::styled(
                        format!(" {}-{} of {}", scroll + 1, scroll + shown, rows.len()),
                        dim,
                    )));
                }
            } else {
                lines.push(Line::from(Span::styled(" measuring…", dim)));
            }

            let height = (lines.len() as u16 + 2).min(f.area().height.saturating_sub(2));
            let area = centered_rect(f.area(), 74, height);
            f.render_widget(Clear, area);
            let block = Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .border_style(Style::default().fg(th.accent))
                .title(Span::styled(
                    " Memory ",
                    Style::default().fg(th.accent).add_modifier(Modifier::BOLD),
                ));
            let inner = block.inner(area);
            f.render_widget(block, area);
            f.render_widget(Paragraph::new(lines), inner);
            if let Some(Overlay::Metrics(v)) = &mut app.overlay {
                v.area = area;
                v.scroll = scroll;
                v.selected = selected;
                v.rows = rows.into_iter().map(|r| r.sref).collect();
                v.list_area = Rect {
                    x: inner.x,
                    y: inner.y + rows_start as u16,
                    width: inner.width,
                    height: (shown as u16).min(inner.height.saturating_sub(rows_start as u16)),
                };
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
                    Line::from(Span::styled("type to filter…", Style::default().fg(th.dim)))
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
                        Paragraph::new(Span::styled("no matches", Style::default().fg(th.dim))),
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
                .border_type(BorderType::Rounded)
                .border_style(Style::default().fg(th.accent))
                .title(Span::styled(
                    title,
                    Style::default().fg(th.accent).add_modifier(Modifier::BOLD),
                ));
            let inner = block.inner(area);
            f.render_widget(block, area);

            // First row: the always-on fuzzy query input.
            if let Some(query_area) = row_rect(inner, 0) {
                let line = if palette.query.is_empty() {
                    Line::from(Span::styled("type to search…", Style::default().fg(th.dim)))
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
                        Paragraph::new(Span::styled("no matches", Style::default().fg(th.dim))),
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
                // Kind indicator: a small colored glyph; the row text stays
                // quiet (dim parent path, bright leaf) so the cyan-bold
                // match highlight is the loudest thing in the list.
                let (glyph, glyph_color) = match &item.target {
                    PaletteTarget::Project(_) => ("▪ ", th.accent),
                    PaletteTarget::Worktree(_) => ("▸ ", th.ok),
                    PaletteTarget::Session(_) => ("● ", th.special),
                };
                let glyph_color = if item.archived { th.dim } else { glyph_color };
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
                let mut spans = vec![Span::styled(glyph, Style::default().fg(glyph_color))];
                spans.extend(path_highlight_spans(&shown, positions, item.archived, th));
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
                .border_type(BorderType::Rounded)
                .border_style(Style::default().fg(th.accent))
                .title(Span::styled(
                    title,
                    Style::default().fg(th.accent).add_modifier(Modifier::BOLD),
                ));
            let inner = block.inner(area);
            f.render_widget(block, area);

            // First row: the always-on fuzzy query input.
            if let Some(query_area) = row_rect(inner, 0) {
                let line = if finder.query.is_empty() {
                    Line::from(Span::styled("type to filter…", Style::default().fg(th.dim)))
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
                        Paragraph::new(Span::styled("no matches", Style::default().fg(th.dim))),
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
                .border_type(BorderType::Rounded)
                .border_style(Style::default().fg(th.accent))
                .title(Span::styled(
                    title,
                    Style::default().fg(th.accent).add_modifier(Modifier::BOLD),
                ));
            let inner = block.inner(area);
            f.render_widget(block, area);

            // First row: the always-live grep query.
            if let Some(query_area) = row_rect(inner, 0) {
                let line = if view.query.is_empty() {
                    Line::from(Span::styled("type to search…", Style::default().fg(th.dim)))
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
        Overlay::Hosts(view) => {
            let total = view.hosts.len();
            let selected = view.selected.min(total.saturating_sub(1));
            let adding = view.input.is_some();
            let list_rows = (total + adding as usize).max(1);
            let height = (list_rows as u16)
                .saturating_add(2)
                .clamp(5, f.area().height.max(5));
            let area = centered_rect(f.area(), 64, height);
            f.render_widget(Clear, area);
            let hint = if adding {
                " type user@host [dir]  Enter: connect  Esc: cancel "
            } else {
                " Enter: connect  a: new host  d: remove  Esc: close "
            };
            let block = Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .border_style(Style::default().fg(th.accent))
                .title(Span::styled(
                    " SSH Hosts ",
                    Style::default().fg(th.accent).add_modifier(Modifier::BOLD),
                ))
                .title_bottom(Line::from(Span::styled(hint, Style::default().fg(th.dim))));
            let inner = block.inner(area);
            f.render_widget(block, area);

            if total == 0 && !adding {
                if let Some(row_area) = row_rect(inner, 0) {
                    f.render_widget(
                        Paragraph::new(Span::styled(
                            "no hosts yet — a connects to a new one",
                            Style::default().fg(th.dim),
                        )),
                        row_area,
                    );
                }
            }
            // Follow-window keeps the cursor visible; while adding, pin the
            // window to the tail so the input row is always on screen.
            let start = if adding {
                list_rows.saturating_sub(inner.height as usize)
            } else {
                view.window_start(inner.height as usize)
            };
            let now = crate::hosts::now_ms();
            for (i, entry) in view.hosts.iter().enumerate().skip(start) {
                let Some(row_area) = row_rect(inner, i - start) else {
                    break;
                };
                let budget = (inner.width as usize).saturating_sub(2);
                // "host  dir" left, a dim "2h ago" pinned right.
                let ago = if entry.last_used_ms > 0 {
                    crate::hosts::ago_label(now - entry.last_used_ms)
                } else {
                    String::new()
                };
                let ago_w = ago.chars().count();
                let text_budget = budget.saturating_sub(if ago_w > 0 { ago_w + 2 } else { 0 });
                let host_txt = truncate(&entry.host, text_budget);
                let mut used = host_txt.chars().count();
                let mut spans = vec![Span::raw(host_txt)];
                if let Some(p) = &entry.path {
                    if used + 2 < text_budget {
                        let dir = truncate(&format!("  {p}"), text_budget - used);
                        used += dir.chars().count();
                        spans.push(Span::styled(dir, Style::default().fg(th.dim)));
                    }
                }
                if ago_w > 0 && used + ago_w < budget {
                    spans.push(Span::raw(" ".repeat(budget - used - ago_w)));
                    spans.push(Span::styled(ago, Style::default().fg(th.dim)));
                }
                render_row(f, row_area, spans, i == selected && !adding, true, th);
            }
            if let Some(input) = &view.input {
                if let Some(row_area) = row_rect(inner, total.saturating_sub(start)) {
                    let budget = (inner.width as usize).saturating_sub(3);
                    // Long inputs: show the tail so the cursor stays visible.
                    let shown: String = if input.chars().count() > budget {
                        let skip = input.chars().count() - budget.saturating_sub(1);
                        format!("…{}", input.chars().skip(skip).collect::<String>())
                    } else {
                        input.clone()
                    };
                    f.render_widget(
                        Paragraph::new(Line::from(vec![
                            Span::styled("+ ", Style::default().fg(th.accent)),
                            Span::raw(shown),
                            Span::styled("█", Style::default().fg(th.accent)),
                        ])),
                        row_area,
                    );
                }
            }

            // Write-back (draw works on a clone): rects for mouse
            // hit-testing, plus the clamped cursor.
            if let Some(Overlay::Hosts(v)) = &mut app.overlay {
                v.area = area;
                v.list_area = inner;
                v.selected = selected;
            }
        }
        Overlay::Notes(view) => {
            // Rows come straight from the tree, so daemon upserts (another
            // client editing the same list) render live.
            let notes: Vec<&nebula_core::Note> = app
                .tree
                .notes
                .iter()
                .filter(|t| t.owner == view.owner)
                .collect();
            let total = notes.len();
            let open = notes.iter().filter(|t| !t.done).count();
            let selected = view.selected.min(total.saturating_sub(1));
            let creating = view.input.as_ref().is_some_and(|i| i.editing.is_none());

            let list_rows = (total + creating as usize).max(1);
            // centered_rect caps to the frame; the max(5) only keeps clamp's
            // bounds ordered on a tiny screen.
            let height = (list_rows as u16)
                .saturating_add(2)
                .clamp(5, f.area().height.max(5));
            let area = centered_rect(f.area(), 58, height);
            f.render_widget(Clear, area);
            let title = if total == 0 {
                format!(" Notes — {} ", view.context)
            } else if open > 0 {
                format!(" Notes — {} ({open} open) ", view.context)
            } else {
                format!(" Notes — {} (all {total} done) ", view.context)
            };
            let hint = if view.input.is_some() {
                " Enter: save  Esc: cancel "
            } else {
                " e: add  Enter: edit  Space: done  d: delete "
            };
            let block = Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .border_style(Style::default().fg(th.accent))
                .title(Span::styled(
                    truncate(&title, (area.width as usize).saturating_sub(2)),
                    Style::default().fg(th.accent).add_modifier(Modifier::BOLD),
                ))
                .title_bottom(Line::from(Span::styled(hint, Style::default().fg(th.dim))));
            let inner = block.inner(area);
            f.render_widget(block, area);

            if total == 0 && !creating {
                if let Some(row_area) = row_rect(inner, 0) {
                    f.render_widget(
                        Paragraph::new(Span::styled(
                            "no notes yet — e adds one",
                            Style::default().fg(th.dim),
                        )),
                        row_area,
                    );
                }
            }
            // Follow-window keeps the cursor visible; while adding, pin the
            // window to the tail so the input row is always on screen.
            let start = if creating {
                list_rows.saturating_sub(inner.height as usize)
            } else {
                view.window_start(inner.height as usize)
            };
            let mut screen_row = 0usize;
            for (i, note) in notes.iter().enumerate().skip(start) {
                let Some(row_area) = row_rect(inner, screen_row) else {
                    break;
                };
                screen_row += 1;
                let budget = (inner.width as usize).saturating_sub(2);
                let editing_this =
                    view.input.as_ref().and_then(|inp| inp.editing.as_ref()) == Some(&note.id);
                let spans = if editing_this {
                    let text = view
                        .input
                        .as_ref()
                        .map(|inp| inp.text.clone())
                        .unwrap_or_default();
                    // Long edits: show the tail so the cursor stays visible.
                    let shown: String = if text.chars().count() > budget.saturating_sub(1) {
                        let skip = text.chars().count() - budget.saturating_sub(2);
                        format!("…{}", text.chars().skip(skip).collect::<String>())
                    } else {
                        text
                    };
                    vec![
                        Span::styled("☐ ", Style::default().fg(th.warn)),
                        Span::raw(shown),
                        Span::styled("█", Style::default().fg(th.accent)),
                    ]
                } else if note.done {
                    vec![
                        Span::styled("✓ ", Style::default().fg(th.ok)),
                        Span::styled(truncate(&note.text, budget), Style::default().fg(th.dim)),
                    ]
                } else {
                    vec![
                        Span::styled("☐ ", Style::default().fg(th.warn)),
                        Span::raw(truncate(&note.text, budget)),
                    ]
                };
                render_row(
                    f,
                    row_area,
                    spans,
                    i == selected && view.input.is_none(),
                    true,
                    th,
                );
            }
            if creating {
                if let Some(row_area) = row_rect(inner, screen_row) {
                    let text = view
                        .input
                        .as_ref()
                        .map(|inp| inp.text.clone())
                        .unwrap_or_default();
                    let budget = (inner.width as usize).saturating_sub(3);
                    let shown: String = if text.chars().count() > budget {
                        let skip = text.chars().count() - budget.saturating_sub(1);
                        format!("…{}", text.chars().skip(skip).collect::<String>())
                    } else {
                        text
                    };
                    f.render_widget(
                        Paragraph::new(Line::from(vec![
                            Span::styled("+ ", Style::default().fg(th.accent)),
                            Span::raw(shown),
                            Span::styled("█", Style::default().fg(th.accent)),
                        ])),
                        row_area,
                    );
                }
            }

            // Write-back (draw works on a clone): rects for mouse
            // hit-testing, plus the clamped cursor.
            if let Some(Overlay::Notes(v)) = &mut app.overlay {
                v.area = area;
                v.list_area = inner;
                v.selected = selected;
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
                    Line::from(Span::styled("type to filter…", Style::default().fg(th.dim)))
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
                        Paragraph::new(Span::styled("no matches", Style::default().fg(th.dim))),
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
                let budget = (list_inner.width as usize).saturating_sub(indent.chars().count() + 3);
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

/// A sidebar column's rect minus its right rule column.
fn shrink_r(area: Rect) -> Rect {
    Rect {
        width: area.width.saturating_sub(1),
        ..area
    }
}

/// Subtle focus cue: fill the whole focused panel with the theme's
/// `focus_tint` — the accent at ~10% opacity, so the panel reads as a
/// faintly lit surface. Painted after content, and only onto cells whose
/// background is still untouched, so selection fills and PTY-drawn
/// colors sit on top of the tint instead of under it.
/// Drag affordance for the panel splitters: a short thick grip centered on
/// each column rule, one step brighter than the rule so the boundary reads
/// as grabbable without turning the chrome back up. Accent while that
/// splitter is hovered (terminals that report motion) or mid-drag.
fn draw_splitter_grips(buf: &mut ratatui::buffer::Buffer, app: &App, body: Rect) {
    if body.height < 7 {
        return; // no room for a grip plus breathing space
    }
    let th = app.theme;
    let mid = body.y + body.height / 2;
    for i in 0..3 {
        // The rule column: the left panel's `Borders::RIGHT` cell, one
        // short of the boundary where the next panel starts.
        let x = app.splitter_x(i).saturating_sub(1);
        let active = app.splitter_drag.map(|d| d.idx) == Some(i) || app.hover_splitter == Some(i);
        let fg = if active { th.accent } else { th.muted };
        for y in mid - 1..=mid + 1 {
            if let Some(cell) = buf.cell_mut((x, y)) {
                cell.set_symbol("┃");
                cell.set_style(Style::default().fg(fg));
            }
        }
    }
}

fn draw_focus_tint(buf: &mut ratatui::buffer::Buffer, area: Rect, th: Theme) {
    for y in area.y..area.y + area.height {
        for x in area.x..area.x + area.width {
            if let Some(cell) = buf.cell_mut((x, y)) {
                if cell.bg == Color::Reset {
                    cell.bg = th.focus_tint;
                }
            }
        }
    }
}

/// Bordered panel frame: rounded corners everywhere for a softer, modern
/// look. Focus has to be unmissable, so the focused panel gets an accent
/// border plus a solid accent-background title chip, versus a thin dim
/// border and plain muted title.
fn panel_block(title: &str, focused: bool, th: Theme) -> Block<'_> {
    if focused {
        Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
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
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(th.dim))
            .title(Span::styled(
                format!(" {title} "),
                Style::default().fg(th.muted),
            ))
    }
}

/// Note-count row badge for a (open, total) pair: open notes as `✎n`, an
/// all-done list as `✓n`; no notes, no badge. The pencil is U+270E — a
/// text-presentation glyph with no emoji variant, so it stays single-width.
fn note_badge((open, total): (usize, usize), th: Theme) -> Option<(String, Style)> {
    match (open, total) {
        (_, 0) => None,
        (0, total) => Some((format!(" ✓{total}"), Style::default().fg(th.ok))),
        (open, _) => Some((format!(" ✎{open}"), Style::default().fg(th.warn))),
    }
}

/// Sweep shades for a status that animates: running rows shimmer yellow,
/// needs-feedback rows red; every other status holds still. `enabled` is
/// the animations setting — off, nothing animates.
fn sweep_ramp(status: Option<AgentStatus>, th: Theme, enabled: bool) -> Option<[Color; 3]> {
    if !enabled {
        return None;
    }
    match status {
        Some(AgentStatus::Running) => Some(th.warn_sweep),
        Some(AgentStatus::NeedsFeedback) => Some(th.err_sweep),
        _ => None,
    }
}

/// Per-cell spans for `text` with a highlight band sweeping left to right:
/// the whole text sits on the ramp's tail shade while the band head (bright,
/// bold) crosses it with the mid shade trailing one cell behind. The band
/// wraps on a period a few cells longer than the text so each pass reads as
/// a wipe with a beat between; `phase` advances one cell per frame.
fn sweep_spans(text: &str, base: Style, ramp: [Color; 3], phase: usize) -> Vec<Span<'static>> {
    // Off-text cells appended to the period: the pause between passes.
    const GAP: usize = 4;
    let chars: Vec<char> = text.chars().collect();
    if chars.is_empty() {
        return Vec::new();
    }
    let head = phase % (chars.len() + GAP);
    chars
        .into_iter()
        .enumerate()
        .map(|(i, c)| {
            let style = match head.checked_sub(i) {
                Some(0) => base.fg(ramp[2]).add_modifier(Modifier::BOLD),
                Some(1) => base.fg(ramp[1]),
                _ => base.fg(ramp[0]),
            };
            Span::styled(c.to_string(), style)
        })
        .collect()
}

/// The name spans for a status-bearing row: one plain span normally,
/// per-cell [`sweep_spans`] while the row's status animates.
fn status_name_spans(
    name: String,
    base: Style,
    ramp: Option<[Color; 3]>,
    phase: usize,
) -> Vec<Span<'static>> {
    match ramp {
        Some(ramp) => sweep_spans(&name, base, ramp, phase),
        None => vec![Span::styled(name, base)],
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

/// Base style for a whole list row. Selection reads as a subtly raised
/// full-width surface (never a reverse-video slab), brighter in the
/// focused panel than in unfocused ones.
fn row_bar(selected: bool, focused: bool, th: Theme) -> Style {
    if selected && focused {
        Style::default().bg(th.sel_bg).add_modifier(Modifier::BOLD)
    } else if selected {
        Style::default().bg(th.sel_bg_dim)
    } else {
        Style::default()
    }
}

/// Render one list row as a full-width bar: an accent `▌` marker pins the
/// selection in the focused panel; every other row gets a plain 1-cell
/// gutter so text stays aligned. Dim spans (idle dots, archived names)
/// would sink into the selection fill, so they get lifted to muted there.
fn render_row(
    f: &mut Frame,
    area: Rect,
    spans: Vec<Span>,
    selected: bool,
    focused: bool,
    th: Theme,
) {
    render_button(f, area, spans, selected, focused, th, 0);
}

/// Render one list entry as a button `area.height` rows tall: the
/// selection fill covers the whole rect, the `▌` marker runs down its
/// left edge, and the text sits on `text_row` (0-based, inside the rect).
/// Dim spans (idle dots, archived names) would sink into the selection
/// fill, so they get lifted to muted there.
fn render_button(
    f: &mut Frame,
    area: Rect,
    mut spans: Vec<Span>,
    selected: bool,
    focused: bool,
    th: Theme,
    text_row: u16,
) {
    if selected {
        for s in &mut spans {
            if s.style.fg == Some(th.dim) {
                s.style.fg = Some(th.muted);
            }
        }
    }
    let marker = || {
        if selected && focused {
            Span::styled("▌", Style::default().fg(th.accent))
        } else if selected {
            Span::styled("▌", Style::default().fg(th.dim))
        } else {
            Span::raw(" ")
        }
    };
    let mut text_spans = Some(spans);
    let mut lines: Vec<Line> = Vec::with_capacity(area.height as usize);
    for r in 0..area.height {
        if r == text_row {
            if let Some(mut spans) = text_spans.take() {
                spans.insert(0, marker());
                lines.push(Line::from(spans));
                continue;
            }
        }
        lines.push(Line::from(marker()));
    }
    f.render_widget(
        Paragraph::new(lines).style(row_bar(selected, focused, th)),
        area,
    );
}

/// Borderless sidebar column: a single dim rule on the right edge, an
/// uppercase header row, one blank spacer, then the list area (returned).
/// The header carries the focus signal — accent when focused, muted
/// otherwise — so the chrome itself can stay quiet.
fn draw_column(
    f: &mut Frame,
    area: Rect,
    title: &str,
    count: Option<usize>,
    focused: bool,
    th: Theme,
) -> Rect {
    let block = Block::default()
        .borders(Borders::RIGHT)
        .border_style(Style::default().fg(th.edge));
    let inner = block.inner(area);
    f.render_widget(block, area);
    let header_style = if focused {
        Style::default().fg(th.accent).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(th.muted).add_modifier(Modifier::BOLD)
    };
    // Row 0 is a blank spacer so the title never sits flush against the
    // very top of the screen; row 1 carries it. `ROW_GUTTER` is the same
    // 3-column indent a list row gets from its 1-column selection marker
    // plus a 2-column status glyph, so the title's text lines up with
    // row text below it.
    if let Some(r) = row_rect(inner, 1) {
        let mut spans = vec![Span::styled(format!("{ROW_GUTTER}{title}"), header_style)];
        if let Some(n) = count {
            spans.push(Span::styled(format!(" · {n}"), Style::default().fg(th.dim)));
        }
        f.render_widget(Paragraph::new(Line::from(spans)), r);
    }
    // One extra column of right padding so row text never touches the
    // column rule.
    Rect {
        y: inner.y + 3,
        height: inner.height.saturating_sub(3),
        width: inner.width.saturating_sub(1),
        ..inner
    }
}

/// Left gutter every list row gets from its 1-column selection marker
/// (`▌`/space) plus a 2-column status glyph (`● `/`○ `/`❯ `): headers and
/// empty-panel hints use the same string so their text lines up with row
/// text below them.
const ROW_GUTTER: &str = "   ";

/// Visual hierarchy of the sidebar lists, stepping down the tree.
/// Projects are 3-row buttons (bold, text centered). Worktrees and
/// sessions are ~2-row pills: a 3-row cell with half-block pads so the
/// name stays vertically centered, stacked on a 2-row stride so pads
/// overlap and items don't pick up an extra gap (the step down reads
/// through text weight instead — bold, plain, muted).
const PROJECT_BTN_H: u16 = 3;
const PILL_H: u16 = 2;
const PILL_HALF: (char, char) = ('▄', '▀');
/// Quadrant caps for the selection rail on the pad rows — the left half
/// of each `PILL_HALF` glyph — so the rail runs the pill's full visual
/// height instead of stopping at the text row.
const PILL_RAIL_CAPS: (char, char) = ('▖', '▘');

/// Render one list entry into a 3-row cell starting at `top`: half-block
/// pad, text, half-block pad. The name sits on the middle row so it
/// stays vertically centered in the ~2-row pill. The pads run the full
/// width so the fill has no dark notch beside the status dot, except in
/// the rail column, where a quadrant cap extends the accent `▌` across
/// the pads so the rail spans the pill's full visual height (matching
/// the 3-row project bar; the cap costs the fill quarter beside it — a
/// cell can't hold a rail quadrant, a fill quarter, and bare background
/// at once — but the bright rail owns that corner anyway). Dim spans get
/// lifted to muted on the fill, same as `render_button`.
fn render_pill(
    f: &mut Frame,
    inner: Rect,
    top: usize,
    mut spans: Vec<Span>,
    selected: bool,
    focused: bool,
    th: Theme,
) {
    let Some(text_area) = rows_rect(inner, top + 1, 1) else {
        return;
    };
    if selected {
        for s in &mut spans {
            if s.style.fg == Some(th.dim) {
                s.style.fg = Some(th.muted);
            }
        }
        let fill = if focused { th.sel_bg } else { th.sel_bg_dim };
        let rail = if focused { th.accent } else { th.dim };
        let mut pad = |glyph: char, cap: char, row: usize| {
            if let Some(r) = rows_rect(inner, row, 1) {
                f.render_widget(
                    Paragraph::new(Line::from(Span::styled(
                        glyph.to_string().repeat(inner.width as usize),
                        Style::default().fg(fill),
                    ))),
                    r,
                );
                f.render_widget(
                    Paragraph::new(Span::styled(cap.to_string(), Style::default().fg(rail))),
                    Rect { width: 1, ..r },
                );
            }
        };
        pad(PILL_HALF.0, PILL_RAIL_CAPS.0, top);
        pad(PILL_HALF.1, PILL_RAIL_CAPS.1, top + 2);
    }
    let marker = if selected && focused {
        Span::styled("▌", Style::default().fg(th.accent))
    } else if selected {
        Span::styled("▌", Style::default().fg(th.dim))
    } else {
        Span::raw(" ")
    };
    spans.insert(0, marker);
    f.render_widget(
        Paragraph::new(Line::from(spans)).style(row_bar(selected, focused, th)),
        text_area,
    );
}

fn draw_projects(f: &mut Frame, app: &mut App, area: Rect) {
    let th = app.theme;
    let focused = app.focus == Focus::Projects;
    let count = Some(app.tree.visible_project_count()).filter(|n| *n > 0);
    let inner = draw_column(f, area, "PROJECTS", count, focused, th);

    if !app.tree.has_visible_projects() {
        f.render_widget(
            Paragraph::new(vec![
                Line::from(Span::styled(
                    format!("{ROW_GUTTER}no projects yet"),
                    Style::default().fg(th.dim),
                )),
                Line::from(vec![
                    Span::styled(format!("{ROW_GUTTER}n"), Style::default().fg(th.accent)),
                    Span::styled(" adds one", Style::default().fg(th.dim)),
                ]),
            ]),
            inner,
        );
        app.hits.push((inner, HitTarget::PanelBg(Focus::Projects)));
        return;
    }

    // Projects and their dividers are one selectable row list; the payload
    // pre-collects per-row display data to end the tree borrow.
    let rows: Vec<(ProjectRow, String, Option<AgentStatus>, (usize, usize))> = app
        .project_rows()
        .into_iter()
        .map(|row| match row {
            ProjectRow::Project(i) => {
                let p = &app.tree.projects[i];
                (
                    row,
                    p.name.clone(),
                    app.project_rollup(&p.id),
                    app.note_stats(&nebula_core::NoteOwner::Project(p.id.clone())),
                )
            }
            ProjectRow::Divider { project, before } => {
                let p = &app.tree.projects[project];
                let label = if before {
                    p.divider_before_label.clone()
                } else {
                    p.divider_label.clone()
                }
                .unwrap_or_default();
                (row, label, None, (0, 0))
            }
        })
        .collect();
    let mut screen_row = 0usize;
    for (row_idx, (row, text, roll, notes)) in rows.iter().enumerate() {
        match row {
            ProjectRow::Project(_) => {
                let Some(row_area) = rows_rect(inner, screen_row, PROJECT_BTN_H) else {
                    break;
                };
                // Same note-count badge as worktree rows: the project's own
                // notes only (worktree notes badge on their worktree).
                let note_badge = note_badge(*notes, th);
                let badge_len = note_badge.as_ref().map_or(0, |(s, _)| s.chars().count());
                // Bold name: the top of the tree reads "biggest".
                let mut spans = vec![status_dot(*roll, th)];
                spans.extend(status_name_spans(
                    truncate(text, (inner.width as usize).saturating_sub(2 + badge_len)),
                    Style::default().add_modifier(Modifier::BOLD),
                    sweep_ramp(*roll, th, app.animations),
                    app.sweep_phase(),
                ));
                if let Some((text, style)) = note_badge {
                    spans.push(Span::styled(text, style));
                }
                render_button(
                    f,
                    row_area,
                    spans,
                    row_idx == app.sel_project,
                    focused,
                    th,
                    PROJECT_BTN_H / 2,
                );
                app.hits.push((row_area, HitTarget::Project(row_idx)));
                screen_row += PROJECT_BTN_H as usize;
            }
            ProjectRow::Divider { .. } => {
                let Some(row_area) = row_rect(inner, screen_row) else {
                    break;
                };
                let spans = divider_spans(text, inner.width, th);
                render_row(f, row_area, spans, row_idx == app.sel_project, focused, th);
                app.hits.push((row_area, HitTarget::Project(row_idx)));
                screen_row += 1;
            }
        }
    }
    app.hits.push((inner, HitTarget::PanelBg(Focus::Projects)));
}

/// A divider line, with the group label woven in when present:
/// `─ label ────────`.
fn divider_spans(label: &str, width: u16, th: Theme) -> Vec<Span<'static>> {
    let w = width as usize;
    let dim = Style::default().fg(th.edge);
    if label.is_empty() {
        return vec![Span::styled("─".repeat(w), dim)];
    }
    let label = truncate(label, w.saturating_sub(4));
    let tail = w.saturating_sub(label.chars().count() + 3);
    vec![
        Span::styled("─ ".to_string(), dim),
        Span::styled(
            label,
            Style::default().fg(th.muted).add_modifier(Modifier::BOLD),
        ),
        Span::styled(format!(" {}", "─".repeat(tail)), dim),
    ]
}

fn draw_worktrees(f: &mut Frame, app: &mut App, area: Rect) {
    let th = app.theme;
    let focused = app.focus == Focus::Worktrees;
    let wt_count = app.visible_worktrees().len();
    let count = Some(wt_count).filter(|n| *n > 0 && !app.divider_focused());
    let inner = draw_column(f, area, "WORKTREES", count, focused, th);

    // A selected separator has nothing underneath it: keep the panel, hide
    // the rows (the terminal pane carries the hint).
    if app.divider_focused() {
        app.hits.push((inner, HitTarget::PanelBg(Focus::Worktrees)));
        return;
    }

    let worktrees: Vec<(String, bool, Option<AgentStatus>, (usize, usize))> = app
        .visible_worktrees()
        .iter()
        .map(|w| {
            (
                w.branch.clone(),
                w.is_main,
                app.worktree_rollup(&w.id),
                app.note_stats(&nebula_core::NoteOwner::Worktree(w.id.clone())),
            )
        })
        .collect();
    if worktrees.is_empty() {
        if app.tree.has_visible_projects() {
            f.render_widget(
                Paragraph::new(Line::from(vec![
                    Span::styled(format!("{ROW_GUTTER}n"), Style::default().fg(th.accent)),
                    Span::styled(" starts a worktree", Style::default().fg(th.dim)),
                ])),
                inner,
            );
        }
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
            f.render_widget(Paragraph::new(Span::styled(format!(" {text}"), dim)), r);
            *screen_row += 1;
        }
    };
    if grouped {
        header(f, "PINNED".into(), &mut screen_row);
    }
    for (i, (branch, is_main, roll, notes)) in worktrees.iter().enumerate() {
        if grouped && i == pinned_count {
            header(f, "UNPINNED".into(), &mut screen_row);
        }
        if row_rect(inner, screen_row + 1).is_none() {
            break;
        }
        let note_badge = note_badge(*notes, th);
        let badge_len = note_badge.as_ref().map_or(0, |(s, _)| s.chars().count());
        let ramp = sweep_ramp(*roll, th, app.animations);
        let mut spans = vec![status_dot(*roll, th)];
        if *is_main {
            let max =
                (inner.width as usize).saturating_sub(2 + ROOT_BADGE.chars().count() + badge_len);
            spans.extend(status_name_spans(
                truncate(branch, max),
                Style::default(),
                ramp,
                app.sweep_phase(),
            ));
            spans.push(Span::styled(ROOT_BADGE, Style::default().fg(th.dim)));
        } else {
            spans.extend(status_name_spans(
                truncate(branch, (inner.width as usize).saturating_sub(2 + badge_len)),
                Style::default(),
                ramp,
                app.sweep_phase(),
            ));
        }
        if let Some((text, style)) = note_badge {
            spans.push(Span::styled(text, style));
        }
        render_pill(
            f,
            inner,
            screen_row,
            spans,
            i == app.sel_worktree,
            focused,
            th,
        );
        if let Some(hit) = rows_rect(inner, screen_row, PILL_H) {
            app.hits.push((hit, HitTarget::Worktree(i)));
        }
        screen_row += PILL_H as usize;
        // An extra quiet row separates the main checkout from the true
        // worktrees below; group headers take over once something is
        // pinned.
        if !grouped && *is_main && worktrees.len() > 1 {
            screen_row += 1;
        }
    }
    app.hits.push((inner, HitTarget::PanelBg(Focus::Worktrees)));
}

fn draw_sessions(f: &mut Frame, app: &mut App, area: Rect) {
    let th = app.theme;
    let focused = app.focus == Focus::Sessions;
    let visible = app.visible_session_rows().len();
    let count = Some(visible).filter(|n| *n > 0 && !app.divider_focused());
    let inner = draw_column(f, area, "SESSIONS", count, focused, th);

    // A selected separator has nothing underneath it: keep the panel, hide
    // the rows (the terminal pane carries the hint).
    if app.divider_focused() {
        app.hits.push((inner, HitTarget::PanelBg(Focus::Sessions)));
        return;
    }

    let rows = app.visible_session_rows();
    if rows.is_empty() && app.selected_worktree().is_some() {
        f.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(format!("{ROW_GUTTER}n"), Style::default().fg(th.accent)),
                Span::styled(" agent · ", Style::default().fg(th.dim)),
                Span::styled("t", Style::default().fg(th.accent)),
                Span::styled(" terminal", Style::default().fg(th.dim)),
            ])),
            inner,
        );
    }
    let (pinned_count, recent_count, unpinned_count, archived_count) = app.session_group_counts();
    let active_count = pinned_count + recent_count + unpinned_count;
    let terminal_count = rows
        .iter()
        .filter(|r| matches!(r, SessionRow::Terminal(_)))
        .count();
    let dim = Style::default().fg(th.dim);

    let mut screen_row: usize = 0;
    let header = |f: &mut Frame, text: String, screen_row: &mut usize| {
        // A blank row above every group after the first keeps the groups
        // scannable without drawing more chrome.
        if *screen_row > 0 {
            *screen_row += 1;
        }
        if let Some(r) = row_rect(inner, *screen_row) {
            f.render_widget(Paragraph::new(Span::styled(format!(" {text}"), dim)), r);
            *screen_row += 1;
        }
    };

    // Group headers only appear once something is pinned or recent;
    // otherwise the list stays flat with no group header.
    let grouped = pinned_count > 0 || recent_count > 0;
    if pinned_count > 0 {
        header(f, "PINNED".into(), &mut screen_row);
        for (i, row) in rows.iter().enumerate().take(pinned_count) {
            if row_rect(inner, screen_row + 1).is_none() {
                break;
            }
            draw_session_row(f, app, inner, screen_row, i, row, focused);
            screen_row += PILL_H as usize;
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
            if row_rect(inner, screen_row + 1).is_none() {
                break;
            }
            draw_session_row(f, app, inner, screen_row, i, row, focused);
            screen_row += PILL_H as usize;
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
        if row_rect(inner, screen_row + 1).is_none() {
            break;
        }
        draw_session_row(f, app, inner, screen_row, i, row, focused);
        screen_row += PILL_H as usize;
    }
    if terminal_count > 0 {
        header(f, "TERMINALS".into(), &mut screen_row);
        for (i, row) in rows
            .iter()
            .enumerate()
            .skip(active_count)
            .take(terminal_count)
        {
            if row_rect(inner, screen_row + 1).is_none() {
                break;
            }
            draw_session_row(f, app, inner, screen_row, i, row, focused);
            screen_row += PILL_H as usize;
        }
    }
    if archived_count > 0 {
        if screen_row > 0 {
            screen_row += 1;
        }
        // Both header forms are click targets: a click expands/collapses
        // the group, same as the A key.
        let text = if app.show_archived {
            format!(" ARCHIVED · {archived_count} (A hides)")
        } else {
            format!(" … {archived_count} archived (A shows)")
        };
        if let Some(r) = row_rect(inner, screen_row) {
            f.render_widget(Paragraph::new(Span::styled(text, dim)), r);
            app.hits.push((r, HitTarget::ArchivedHeader));
            screen_row += 1;
        }
        if app.show_archived {
            for (i, row) in rows.iter().enumerate().skip(active_count + terminal_count) {
                if row_rect(inner, screen_row + 1).is_none() {
                    break;
                }
                draw_session_row(f, app, inner, screen_row, i, row, focused);
                screen_row += PILL_H as usize;
            }
        }
    }

    // Panel background (registered last so rows win the hit-test).
    app.hits.push((inner, HitTarget::PanelBg(Focus::Sessions)));
}

fn draw_session_row(
    f: &mut Frame,
    app: &mut App,
    inner: Rect,
    top: usize,
    index: usize,
    row: &SessionRow,
    focused: bool,
) {
    let th = app.theme;
    let width = inner.width;
    let spans = match row {
        SessionRow::Agent(a) => {
            let dot = if a.archived {
                Span::styled("⊘ ", Style::default().fg(th.dim))
            } else {
                status_dot(Some(a.status), th)
            };
            // Muted names: sessions sit at the bottom of the tree, so
            // their text reads "smallest" next to the bold project
            // buttons.
            let name_style = if a.archived {
                Style::default().fg(th.dim)
            } else {
                Style::default().fg(th.muted)
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
            // Archived rows stay quiet even if their last status was live.
            let ramp = if a.archived {
                None
            } else {
                sweep_ramp(Some(a.status), th, app.animations)
            };
            let mut spans = vec![dot];
            spans.extend(status_name_spans(
                truncate(&a.name, name_max),
                name_style,
                ramp,
                app.sweep_phase(),
            ));
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
                Span::styled(
                    truncate(&t.name, width.saturating_sub(2) as usize),
                    Style::default().fg(th.muted),
                ),
            ]
        }
    };
    render_pill(f, inner, top, spans, index == app.sel_session, focused, th);
    if let Some(hit) = rows_rect(inner, top, PILL_H) {
        app.hits.push((hit, HitTarget::Session(index)));
    }
}

/// Borderless terminal frame: a header row (`TERMINAL · session` plus a
/// right-aligned state tag), a thin rule, then the content area. The
/// header carries the focus signal like the sidebar columns do.
fn terminal_frame(
    f: &mut Frame,
    area: Rect,
    left: Vec<Span<'static>>,
    right: Option<Span<'static>>,
    focused: bool,
    th: Theme,
) -> Rect {
    let header_style = if focused {
        Style::default().fg(th.accent).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(th.muted).add_modifier(Modifier::BOLD)
    };
    // Row 0 is a blank spacer so the header sits on the same screen row
    // as the sidebar column titles (`draw_column` does the same).
    if let Some(r) = row_rect(area, 1) {
        let mut spans = vec![Span::styled("  TERMINAL".to_string(), header_style)];
        spans.extend(left);
        f.render_widget(Paragraph::new(Line::from(spans)), r);
        if let Some(tag) = right {
            f.render_widget(
                Paragraph::new(Line::from(vec![tag, Span::raw(" ")]))
                    .alignment(ratatui::layout::Alignment::Right),
                r,
            );
        }
    }
    if let Some(r) = row_rect(area, 2) {
        let rule_style = if focused {
            Style::default().fg(th.accent)
        } else {
            Style::default().fg(th.edge)
        };
        f.render_widget(
            Paragraph::new(Span::styled("─".repeat(area.width as usize), rule_style)),
            r,
        );
    }
    Rect {
        y: area.y + 3,
        height: area.height.saturating_sub(3),
        ..area
    }
}

fn draw_terminal(f: &mut Frame, app: &mut App, area: Rect) {
    let th = app.theme;
    let focused = app.focus == Focus::Terminal;
    // A selected separator has no session behind it: keep the pane, swap
    // the content for a hint. The attachment itself stays live so walking
    // the list across a divider doesn't churn detach/attach.
    if app.divider_focused() {
        let inner = terminal_frame(f, area, Vec::new(), None, focused, th);
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
    // Name the attached session in the header so it's clear what you're
    // looking at (and typing into) even with the sidebars collapsed.
    let mut left = Vec::new();
    if let Some(name) = attached_session_name(app) {
        left.push(Span::styled(" · ".to_string(), Style::default().fg(th.dim)));
        left.push(Span::styled(name, Style::default().fg(th.muted)));
    }
    let right = match &app.term {
        Some(t) if t.exited => Some(Span::styled(
            "exited".to_string(),
            Style::default().fg(th.err).add_modifier(Modifier::BOLD),
        )),
        Some(t) if t.scroll > 0 => Some(Span::styled(
            format!("scroll {}", t.scroll),
            Style::default().fg(th.warn).add_modifier(Modifier::BOLD),
        )),
        Some(_) if app.term_locked => Some(Span::styled(
            "INPUT".to_string(),
            Style::default().fg(th.accent).add_modifier(Modifier::BOLD),
        )),
        _ => None,
    };
    let inner = terminal_frame(f, area, left, right, focused, th);
    // One cell of inset so PTY content doesn't hug the sessions rule.
    let inner = Rect {
        x: inner.x + 1,
        width: inner.width.saturating_sub(1),
        ..inner
    };
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
            // Empty-pane hero: vertically centered wordmark + a compact
            // key cheat-sheet, so the big blank pane earns its keep.
            let key = |k: &str, label: &str| {
                vec![
                    Span::styled(
                        k.to_string(),
                        Style::default().fg(th.accent).add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(format!(" {label}"), Style::default().fg(th.dim)),
                ]
            };
            let sep = || Span::styled("   ·   ", Style::default().fg(th.dim));
            let mut hint = Vec::new();
            hint.extend(key("Enter", "attach"));
            hint.push(sep());
            hint.extend(key("n", "new agent"));
            hint.push(sep());
            hint.extend(key("/", "jump"));
            hint.push(sep());
            hint.extend(key("?", "help"));
            let mut lines = vec![Line::from("")];
            let blank = inner.height.saturating_sub(6) / 2;
            for _ in 0..blank {
                lines.insert(0, Line::from(""));
            }
            lines.push(Line::from(vec![
                Span::styled("◆ ", Style::default().fg(th.accent)),
                Span::styled(
                    "nebula",
                    Style::default().fg(th.text).add_modifier(Modifier::BOLD),
                ),
            ]));
            lines.push(Line::from(Span::styled(
                "your agents keep running, even when you leave",
                Style::default().fg(th.dim),
            )));
            lines.push(Line::from(""));
            lines.push(Line::from(hint));
            let msg = Paragraph::new(lines).centered();
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
                Style::default().fg(th.accent).add_modifier(Modifier::BOLD)
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

/// Short display name for an editor command: the basename when it's a
/// path, so footer hints say "edit in nvim", not the full path.
fn editor_name(cmd: &str) -> &str {
    std::path::Path::new(cmd)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(cmd)
}

fn draw_footer(f: &mut Frame, app: &App, area: Rect) {
    let th = app.theme;
    // The hint branches below build with `dim`; lift to muted at the end
    // so hints read as secondary, not disabled (flash/warn stays as-is).
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
    } else if let Some(Overlay::Grep(view)) = &app.overlay {
        Span::styled(
            format!(
                "type: search  ↑/↓: move  Enter: edit in {}  Ctrl+u: clear  Esc: clear/close",
                editor_name(&view.editor)
            ),
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
    } else if let Some(Overlay::Files(view)) = &app.overlay {
        Span::styled(
            format!(
                "type: search  ↑/↓: move  Enter: edit in {}  Ctrl+y: copy path  Ctrl+u: clear  Esc: clear/close",
                editor_name(&view.editor)
            ),
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
    } else if let Some(Overlay::Notes(view)) = &app.overlay {
        Span::styled(
            if view.input.is_some() {
                "type the note  Enter: save  Esc: cancel"
            } else {
                "e: add  Enter: edit  Space: toggle done  d: delete  Esc: close"
            },
            Style::default().fg(th.dim),
        )
    } else if matches!(&app.overlay, Some(Overlay::Metrics(_))) {
        Span::styled(
            "↑/↓: select  Enter: open session  Esc: close  (refreshes every 2s)",
            Style::default().fg(th.dim),
        )
    } else if let Some(Overlay::Hosts(view)) = &app.overlay {
        Span::styled(
            if view.input.is_some() {
                "type user@host [dir]  Enter: connect (restarts nebula over ssh)  Esc: cancel"
            } else {
                "↑/↓: select  Enter: connect (restarts nebula over ssh)  a: new  d: remove  Esc: close"
            },
            Style::default().fg(th.dim),
        )
    } else if matches!(&app.overlay, Some(Overlay::Menu(m)) if m.is_workspace_picker()) {
        Span::styled(
            "Enter: open  n: new  r: rename  d: delete  Esc: close",
            Style::default().fg(th.dim),
        )
    } else if app.overlay.is_some() {
        Span::styled("Esc: close  Enter: confirm", Style::default().fg(th.dim))
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
                _ => "n/o: add  e: notes  d: remove  -: divider  ⇧J/K: move  /: search  m: menu  ?: help",
            },
            Focus::Worktrees => {
                "n: new worktree  e: notes  t: terminal  p: pin  d: delete  /: search  m: menu  ?: help"
            }
            Focus::Sessions => {
                "↵: focus  n: agent  t: terminal  r: rename  p: pin  a: archive  d: del  m: menu  ?: help"
            }
        };
        Span::styled(text, Style::default().fg(th.dim))
    };
    // Quiet footer: context on the left, live stats on the right. The
    // hostname only earns a slot when it's a remote session, and the
    // connection state only when something is wrong.
    let mut spans = vec![Span::raw(" ")];
    // The open workspace leads the bar — it scopes everything else shown.
    spans.push(Span::styled(
        format!("◇ {}", truncate(app.tree.active_workspace_name(), 20)),
        Style::default().fg(th.accent).add_modifier(Modifier::BOLD),
    ));
    spans.push(Span::styled("  ·  ", Style::default().fg(th.dim)));
    if app.is_remote {
        spans.push(Span::styled(
            truncate(&app.hostname, 24),
            Style::default().fg(th.warn).add_modifier(Modifier::BOLD),
        ));
        spans.push(Span::styled("  ·  ", Style::default().fg(th.dim)));
    }
    if matches!(app.conn, ConnState::Disconnected) {
        spans.push(conn);
        spans.push(Span::styled("  ·  ", Style::default().fg(th.dim)));
    }
    let crumbs = breadcrumb(app);
    if !crumbs.is_empty() {
        spans.extend(crumbs);
        // The selected checkout's dirty-file count rides the breadcrumb —
        // it's context, not chrome.
        if !app.divider_focused() {
            if let Some(n) = app.selected_worktree_changes().filter(|n| *n > 0) {
                spans.push(Span::styled(
                    format!("  +{n} file{}", if n == 1 { "" } else { "s" }),
                    Style::default().fg(th.warn),
                ));
            }
        }
        spans.push(Span::styled("    ", Style::default()));
    }
    let mut hints = hints;
    if hints.style.fg == Some(th.dim) {
        hints.style.fg = Some(th.muted);
    }
    spans.push(hints);
    // Right edge: live session/process counts and nebula's total memory
    // footprint, fed by the footer metrics poll. The hints clip before the
    // readout does.
    let usage = footer_usage(app);
    let right_w = usage
        .as_ref()
        .map(|s| s.chars().count() as u16 + 2)
        .unwrap_or(0)
        .min(area.width);
    let left = Rect {
        width: area.width.saturating_sub(right_w),
        ..area
    };
    f.render_widget(Paragraph::new(Line::from(spans)), left);
    if let Some(usage) = usage {
        let right = Rect {
            x: area.x + area.width.saturating_sub(right_w),
            width: right_w,
            ..area
        };
        f.render_widget(
            Paragraph::new(Line::from(Span::styled(usage, Style::default().fg(th.dim))))
                .alignment(ratatui::layout::Alignment::Right),
            right,
        );
    }
}

/// The footer's right-edge readout: live sessions, their process count,
/// and nebula's total memory footprint (TUI + daemon + every session's
/// process subtree). None until the first metrics reply arrives.
fn footer_usage(app: &App) -> Option<String> {
    let m = app.last_metrics.as_ref()?;
    let agents = m
        .sessions
        .iter()
        .filter(|s| matches!(s.session, SessionRef::Agent(_)))
        .count();
    let terms = m.sessions.len() - agents;
    let total = m.daemon_rss_bytes
        + app.client_rss_bytes
        + m.sessions.iter().map(|s| s.rss_bytes).sum::<u64>();
    let plural = |n: usize| if n == 1 { "" } else { "s" };
    Some(format!(
        "{agents} agent{} · {terms} term{} · {}",
        plural(agents),
        plural(terms),
        fmt_mem(total)
    ))
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

/// Palette row text: dim `parent/path/` prefix, normal leaf segment, with
/// fuzzy-match chars lit accent-bold on top. Archived rows stay dim all
/// the way through.
fn path_highlight_spans(
    shown: &str,
    positions: &[usize],
    archived: bool,
    th: Theme,
) -> Vec<Span<'static>> {
    let boundary = shown
        .rfind('/')
        .map(|b| shown[..=b].chars().count())
        .unwrap_or(0);
    let hl = Style::default().fg(th.accent).add_modifier(Modifier::BOLD);
    let mut spans = Vec::new();
    let mut run = String::new();
    let mut run_style: Option<Style> = None;
    for (i, c) in shown.chars().enumerate() {
        let style = if positions.binary_search(&i).is_ok() {
            hl
        } else if archived || i < boundary {
            Style::default().fg(th.dim)
        } else {
            Style::default().fg(th.text)
        };
        if run_style != Some(style) {
            if let Some(s) = run_style.take() {
                if !run.is_empty() {
                    spans.push(Span::styled(std::mem::take(&mut run), s));
                }
            }
            run_style = Some(style);
        }
        run.push(c);
    }
    if let (Some(s), false) = (run_style, run.is_empty()) {
        spans.push(Span::styled(run, s));
    }
    spans
}

/// Split a (possibly truncated) path into spans, lighting the chars the
/// fuzzy filter matched. `positions` are ascending char indices into the
/// untruncated path; anything cut off by truncation simply isn't lit.
fn fuzzy_highlight_spans(shown: &str, positions: &[usize], th: Theme) -> Vec<Span<'static>> {
    if positions.is_empty() {
        return vec![Span::raw(shown.to_string())];
    }
    let hl = Style::default().fg(th.accent).add_modifier(Modifier::BOLD);
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
    rows_rect(inner, i, 1)
}

/// A rect `height` rows tall starting at the i-th row inside `inner`:
/// None once the first row overflows, clamped when only the tail does.
fn rows_rect(inner: Rect, i: usize, height: u16) -> Option<Rect> {
    let y = inner.y + i as u16;
    if y >= inner.y + inner.height {
        return None;
    }
    Some(Rect {
        x: inner.x,
        y,
        width: inner.width,
        height: height.min(inner.y + inner.height - y),
    })
}

/// Human-readable byte count for the metrics modal.
fn fmt_mem(bytes: u64) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = KB * 1024.0;
    const GB: f64 = MB * 1024.0;
    let b = bytes as f64;
    if b >= 10.0 * GB {
        format!("{:.0} GB", b / GB)
    } else if b >= GB {
        format!("{:.1} GB", b / GB)
    } else if b >= MB {
        format!("{:.0} MB", b / MB)
    } else if b >= KB {
        format!("{:.0} KB", b / KB)
    } else {
        format!("{bytes} B")
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    const RAMP: [Color; 3] = [Color::Yellow, Color::Indexed(220), Color::Indexed(230)];

    fn colors(spans: &[Span]) -> Vec<Color> {
        spans.iter().map(|s| s.style.fg.unwrap()).collect()
    }

    /// The sweep must recolor cells without ever changing what they spell.
    #[test]
    fn sweep_spans_preserve_text() {
        for phase in 0..12 {
            let spans = sweep_spans("run", Style::default(), RAMP, phase);
            let text: String = spans.iter().map(|s| s.content.as_ref()).collect();
            assert_eq!(text, "run", "phase {phase}");
        }
    }

    #[test]
    fn sweep_band_marches_then_pauses() {
        // Phase 1 on "run": head on 'u' (bright + bold), mid trailing on
        // 'r', tail ahead on 'n'.
        let spans = sweep_spans("run", Style::default(), RAMP, 1);
        assert_eq!(colors(&spans), vec![RAMP[1], RAMP[2], RAMP[0]]);
        assert!(spans[1].style.add_modifier.contains(Modifier::BOLD));
        assert!(!spans[0].style.add_modifier.contains(Modifier::BOLD));
        // Off-text phases: the whole word rests on the tail shade.
        let spans = sweep_spans("run", Style::default(), RAMP, 5);
        assert_eq!(colors(&spans), vec![RAMP[0]; 3]);
        // The period is len + gap (3 + 4), so phase 7 restarts the pass.
        assert_eq!(
            colors(&sweep_spans("run", Style::default(), RAMP, 7)),
            colors(&sweep_spans("run", Style::default(), RAMP, 0)),
        );
    }

    /// Only yellow (running) and red (needs feedback) animate; every other
    /// status renders still text, and the animations setting kills even
    /// those two.
    #[test]
    fn sweep_ramp_gates_on_live_statuses_and_the_setting() {
        let th = Theme::default();
        assert_eq!(
            sweep_ramp(Some(AgentStatus::Running), th, true),
            Some(th.warn_sweep)
        );
        assert_eq!(
            sweep_ramp(Some(AgentStatus::NeedsFeedback), th, true),
            Some(th.err_sweep)
        );
        for status in [
            AgentStatus::Fresh,
            AgentStatus::Finished,
            AgentStatus::Terminated,
            AgentStatus::Disconnected,
        ] {
            assert_eq!(sweep_ramp(Some(status), th, true), None, "{status:?}");
        }
        assert_eq!(sweep_ramp(None, th, true), None);
        assert_eq!(sweep_ramp(Some(AgentStatus::Running), th, false), None);
        assert_eq!(
            sweep_ramp(Some(AgentStatus::NeedsFeedback), th, false),
            None
        );
    }

    /// The tint fills every untouched cell of the panel rect — and only
    /// those: a selection fill keeps its own, and cells outside the rect
    /// stay untinted.
    #[test]
    fn focus_tint_fills_panel_and_skips_painted_cells() {
        let th = Theme::default();
        let area = Rect::new(1, 1, 3, 4);
        let mut buf = ratatui::buffer::Buffer::empty(Rect::new(0, 0, 5, 6));
        buf.cell_mut((2, 2)).unwrap().bg = th.sel_bg;
        draw_focus_tint(&mut buf, area, th);
        let bg = |x, y| buf.cell((x, y)).unwrap().bg;
        for y in 1..5 {
            for x in 1..4 {
                if (x, y) == (2, 2) {
                    assert_eq!(bg(x, y), th.sel_bg, "painted cell must keep its fill");
                } else {
                    assert_eq!(bg(x, y), th.focus_tint, "({x},{y})");
                }
            }
        }
        assert_eq!(bg(0, 1), Color::Reset, "left of the panel");
        assert_eq!(bg(4, 1), Color::Reset, "right of the panel");
        assert_eq!(bg(1, 0), Color::Reset, "above the panel");
        assert_eq!(bg(1, 5), Color::Reset, "below the panel");
    }

    /// Each grip sits on its rule column (one left of the boundary), three
    /// cells centered vertically: muted at rest, accent under hover.
    #[test]
    fn splitter_grips_center_on_the_rules() {
        let th = Theme::default();
        let mut app = App::new();
        let body = Rect::new(0, 0, 120, 35);
        let mut buf = ratatui::buffer::Buffer::empty(body);
        draw_splitter_grips(&mut buf, &app, body);
        let mid = body.height / 2; // 17
        for i in 0..3 {
            let x = app.splitter_x(i) - 1;
            for y in mid - 1..=mid + 1 {
                let cell = buf.cell((x, y)).unwrap();
                assert_eq!(cell.symbol(), "┃", "splitter {i} y={y}");
                assert_eq!(cell.fg, th.muted, "splitter {i} rests muted");
            }
            assert_eq!(buf.cell((x, mid - 2)).unwrap().symbol(), " ");
            assert_eq!(buf.cell((x, mid + 2)).unwrap().symbol(), " ");
        }

        // Hover lights only that splitter's grip.
        app.hover_splitter = Some(1);
        draw_splitter_grips(&mut buf, &app, body);
        assert_eq!(
            buf.cell((app.splitter_x(1) - 1, mid)).unwrap().fg,
            th.accent
        );
        assert_eq!(buf.cell((app.splitter_x(0) - 1, mid)).unwrap().fg, th.muted);

        // A body too short for a grip plus breathing space draws nothing.
        let tiny = Rect::new(0, 0, 120, 6);
        let mut buf = ratatui::buffer::Buffer::empty(tiny);
        draw_splitter_grips(&mut buf, &app, tiny);
        assert!(buf.content().iter().all(|c| c.symbol() == " "));
    }
}
