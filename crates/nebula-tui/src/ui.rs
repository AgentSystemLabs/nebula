//! View layer: draws the visible panels + terminal pane + footer, and
//! records hit regions for mouse interaction.

use crate::app::{App, ConnState, Focus, HitTarget, Overlay, PaletteTarget, SessionRow};
use crate::git_diff::{classify_diff_line, DiffLineKind};
use crate::keymap::Action;
use crate::text_input::TextInput;
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
/// Outer size of the two split modals (diff, tree), percent of the frame.
const SPLIT_MODAL_PCT: (u16, u16) = (92, 90);
/// Outer size of the find-in-files modal, percent of the frame.
const GREP_MODAL_PCT: (u16, u16) = (88, 76);
/// Fixed (width, height) of the jump palette.
const PALETTE_SIZE: (u16, u16) = (64, 18);
/// Fixed (width, height) of the find-file modal.
const FILES_SIZE: (u16, u16) = (72, 20);
/// Fixed (width, height) of the multi-line task prompt.
const TASK_PROMPT_SIZE: (u16, u16) = (76, 14);
/// Width of a one-line prompt, and of the wider one carrying a directory
/// listing under its input.
const PROMPT_W: u16 = 56;
const PATH_PROMPT_W: u16 = 72;
/// Narrowest a confirm dialog gets, so a short question still reads as one.
const CONFIRM_MIN_W: u16 = 52;
/// Widths of the modals whose height follows their content.
const HELP_W: u16 = 92;
const SETTINGS_W: u16 = 84;
const MEMORY_W: u16 = 74;
const HOSTS_W: u16 = 64;
/// Layout floor for a split modal's right pane. Deliberately below
/// `MIN_DIFF_PANE_W`: the file list is clamped to keep that minimum first,
/// so on a tiny screen this lets the layout squeeze the diff/preview pane
/// rather than the list.
const SPLIT_PANE_LAYOUT_MIN: u16 = 20;
/// What every filtered list says when nothing survives the filter.
const NO_MATCHES: &str = "no matches";

/// Columns the tree-browser preview must keep for the file text itself
/// before a line-number gutter is worth drawing.
const MIN_PREVIEW_TEXT_W: usize = 16;

pub fn draw(f: &mut Frame, app: &mut App) {
    app.hits.clear();

    // The bar gets a blank row above it so it breathes off the panel
    // borders, matching the terminal's own padding below the last row.
    let [body, footer] =
        Layout::vertical([Constraint::Min(3), Constraint::Length(2)]).areas(f.area());

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

    // First run (the default workspace is empty): no visible projects
    // means three empty panels, so the whole body becomes the animated
    // nebula splash until the first project lands. Other empty workspaces
    // keep their panels. N summons the same splash as a dismissable
    // preview.
    if app.splash_showing() {
        crate::splash::draw_splash(f, app, body);
        draw_footer(f, app, footer);
        draw_overlay(f, app);
        draw_vim(f, app);
        return;
    }

    app.body_area = body;
    app.normalize_panel_widths(body.width);
    // The Workspaces bar (Shift+W) runs across the top of the body — zero
    // rows tall when hidden; the three panels and the terminal pane take
    // the full width of whatever is left under it.
    let [workspaces_a, panels_a] = Layout::vertical([
        Constraint::Length(app.workspaces_bar_h()),
        Constraint::Min(0),
    ])
    .areas(body);
    let visible_panels = app.visible_panel_indices();
    let constraints = visible_panels
        .iter()
        .map(|idx| Constraint::Length(app.panel_widths[*idx]))
        .chain(std::iter::once(Constraint::Min(crate::app::MIN_TERM_W)));
    let areas = panels_a.layout_vec(&Layout::horizontal(constraints));
    let mut panel_areas: [Option<Rect>; 3] = [None; 3];
    for (idx, area) in visible_panels.iter().copied().zip(areas.iter().copied()) {
        panel_areas[idx] = Some(area);
    }
    let term_a = areas[visible_panels.len()];

    // Splitter grab zones: the two touching border cells at each panel
    // boundary. Registered first so they win `hit_at`'s first-match scan —
    // and only over the panels, so the tab bar above stays clickable.
    for i in app.splitter_indices() {
        let x = app.splitter_x(i);
        app.hits.push((
            Rect {
                x: x.saturating_sub(1),
                y: panels_a.y,
                width: 2,
                height: panels_a.height,
            },
            HitTarget::Splitter(i),
        ));
    }

    if app.show_workspaces {
        draw_workspaces_bar(f, app, workspaces_a);
    }
    if let Some(area) = panel_areas[0] {
        draw_projects(f, app, area);
    }
    if let Some(area) = panel_areas[1] {
        draw_worktrees(f, app, area);
    }
    draw_sessions(
        f,
        app,
        panel_areas[2].expect("Sessions panel is always visible"),
    );
    draw_terminal(f, app, term_a);
    draw_splitter_grips(f.buffer_mut(), app, panels_a);
    // Focus cue (opt-in, `focus_tint` setting): the focused panel's whole
    // background picks up a faint accent tint. The sidebar columns stop
    // one cell short of their right rule so the tint stays inside the
    // panel.
    if app.focus_tint {
        let tinted = match app.focus {
            // The bar's last row is its rule, which belongs to the boundary
            // rather than to the bar — leave it untinted.
            Focus::Workspaces => Some(shrink_b(workspaces_a)),
            Focus::Projects => panel_areas[0].map(shrink_r),
            Focus::Worktrees => panel_areas[1].map(shrink_r),
            Focus::Sessions => panel_areas[2].map(shrink_r),
            Focus::Terminal => Some(term_a),
        };
        if let Some(tinted) = tinted {
            draw_focus_tint(f.buffer_mut(), tinted, app.theme);
        }
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
            // border; the modal widens to fit.
            let hint = if menu.is_workspace_picker() {
                Some(" n: new  r: rename  d: delete ")
            } else {
                menu.hovered_claude_cloud().map(|cloud| {
                    if cloud {
                        " Tab: cloud on "
                    } else {
                        " Tab: cloud off "
                    }
                })
            };
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
            let width = (longest.unwrap_or(0) as u16 + 4).max(CONFIRM_MIN_W);
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
            // Record the drawn area for click hit-testing.
            if let Some(Overlay::Confirm(c)) = &mut app.overlay {
                c.area = area;
            }
        }
        Overlay::Prompt(prompt) if prompt.is_multiline() => {
            let area = centered_rect(f.area(), TASK_PROMPT_SIZE.0, TASK_PROMPT_SIZE.1);
            f.render_widget(Clear, area);
            let hint = if area.width >= 64 {
                " Enter: launch · Shift+Enter/^J: newline · Esc: cancel "
            } else if area.width >= 36 {
                " Enter launch · ^J newline · Esc cancel "
            } else {
                " Esc · ^J · Enter "
            };
            let block = Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .border_style(Style::default().fg(th.accent))
                .title(Span::styled(
                    format!(" {} ", prompt.title),
                    Style::default().fg(th.accent),
                ))
                .title_bottom(Line::from(Span::styled(hint, Style::default().fg(th.dim))));
            let inner = block.inner(area);
            f.render_widget(block, area);

            let label_rows = usize::from(inner.height >= 4);
            if label_rows == 1 {
                let row = row_rect(inner, 0).expect("a four-row inner area has a label row");
                f.render_widget(
                    Paragraph::new(Span::styled(prompt.label, Style::default().fg(th.dim))),
                    row,
                );
            }

            // A bordered, multi-row task editor. Its own wrapping helper
            // keeps words intact and follows the caret once the task grows
            // beyond the visible rows.
            let editor_area = Rect {
                x: inner.x,
                y: inner.y.saturating_add(label_rows as u16),
                width: inner.width,
                height: inner.height.saturating_sub(label_rows as u16),
            };
            let editor_inner = if editor_area.height >= 3 && editor_area.width >= 4 {
                let editor_block = Block::default()
                    .borders(Borders::ALL)
                    .border_type(BorderType::Rounded)
                    .border_style(Style::default().fg(th.dim));
                let editor_inner = editor_block.inner(editor_area);
                f.render_widget(editor_block, editor_area);
                editor_inner
            } else {
                editor_area
            };
            let (lines, caret_row) =
                multiline_input_lines(&prompt.input, editor_inner.width as usize, th.accent, th);
            let visible = editor_inner.height.max(1) as usize;
            let max_start = lines.len().saturating_sub(visible);
            let start = caret_row.saturating_sub(visible / 2).min(max_start);
            let shown: Vec<Line> = lines.into_iter().skip(start).take(visible).collect();
            f.render_widget(Paragraph::new(shown), editor_inner);
            // Record the drawn area for click hit-testing.
            if let Some(Overlay::Prompt(p)) = &mut app.overlay {
                p.area = area;
            }
        }
        Overlay::Prompt(prompt) => {
            // Path prompts get a wide dialog with the live directory
            // listing between the input and the hint; the dialog grows to
            // fit the listing (at least one row, for the empty message).
            let is_path = prompt.completes_paths();
            let width = if is_path { PATH_PROMPT_W } else { PROMPT_W };
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

            // Row 1: the input. Long paths scroll under it around the
            // caret; the caret dims while a listing row is highlighted
            // (Enter takes the highlight, not the text).
            if let Some(r) = row_rect(inner, 1) {
                let budget = inner.width.saturating_sub(2) as usize;
                let cursor = if prompt.hover.is_some() {
                    th.dim
                } else {
                    th.text
                };
                let mut spans = vec![Span::raw("> ")];
                spans.extend(input_spans(&prompt.input, budget, cursor, th));
                f.render_widget(Paragraph::new(Line::from(spans)), r);
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
                    "[Enter] ok  [⌥←→] word  [Ctrl+u] clear  [Esc] cancel"
                };
                f.render_widget(
                    Paragraph::new(Span::styled(hint, Style::default().fg(th.dim))),
                    r,
                );
            }
            // Record the listing and dialog rects for click hit-testing.
            if let Some(Overlay::Prompt(p)) = &mut app.overlay {
                p.list_area = list_area;
                p.area = area;
            }
        }
        Overlay::Help(_) => {
            // Grouped keymap in two columns: reads by task instead of one
            // giant list, and at ~24 rows it fits a stock terminal window
            // (the old single list clipped its tail on short screens).
            // Key columns come from the live keymap, not hardcoded text:
            // every one of these is rebindable in Settings → Hotkeys, and
            // help that lies about that is worse than no help. Literals
            // are for keys that belong to an overlay rather than the
            // panels, which is why they aren't rebindable.
            use crate::keymap::Action::*;
            enum HelpKeys {
                Lit(&'static str),
                Act(&'static [crate::keymap::Action]),
            }
            use HelpKeys::{Act, Lit};
            type HelpSection = (&'static str, &'static [(HelpKeys, &'static str)]);
            const LEFT: &[HelpSection] = &[
                (
                    "NAVIGATE & SEARCH",
                    &[
                        (
                            Act(&[FocusNext, FocusPrev]),
                            "walk panels (fwd locks input)",
                        ),
                        (
                            Act(&[FocusLeft, FocusRight]),
                            "focus left / right (2×: jump)",
                        ),
                        (Act(&[MoveDown, MoveUp]), "move selection (2×: bar)"),
                        (Act(&[Activate]), "drill in / attach session"),
                        (Act(&[Palette]), "fuzzy jump to anything"),
                        (Lit("^o / ^f"), "jump pick: open / focus row"),
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
                        (Act(&[GitDiff]), "git diff (^r: mark reviewed ✓)"),
                        (Act(&[OpenRepo]), "open the repo on GitHub"),
                        (Act(&[Delete, DeleteAll]), "delete one / delete all"),
                    ],
                ),
                (
                    // Every typed field — names, filters, queries — is the
                    // same line editor (text_input.rs).
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
                        (Act(&[AgentPresets]), "agent presets: launch with a task"),
                        (Act(&[NewTerminal]), "new shell terminal"),
                        (Act(&[Activate]), "attach session / open link"),
                        (Act(&[Rename]), "rename agent / edit link URL"),
                        (
                            Act(&[Archive, Unarchive, ToggleArchived]),
                            "archive / unarchive / show",
                        ),
                        (Act(&[ContextMenu]), "context menu (right-click)"),
                        (Act(&[Delete, DeleteAll]), "delete one / delete all"),
                    ],
                ),
                (
                    "TERMINAL & MOUSE",
                    &[
                        (Act(&[Activate, Zoom]), "lock input (2nd: full-screen)"),
                        (Act(&[UnlockTerminal]), "unlock, back to panels"),
                        (Lit("drag"), "select + copy (2×click: word)"),
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
                            Act(&[Workspaces, ToggleWorkspaces]),
                            "workspace switcher / workspaces bar",
                        ),
                        (
                            Act(&[ToggleProjects, ToggleWorktrees]),
                            "show / hide Projects / Worktrees",
                        ),
                        (Lit("⌘1-9 / 1-9"), "open that workspace tab"),
                        (Act(&[Hosts]), "ssh hosts: connect (a: new, d: del)"),
                        (Act(&[Settings]), "settings (Hotkeys tab rebinds these)"),
                        (Act(&[Metrics]), "memory usage (nebula + agents)"),
                        (Act(&[Splash]), "nebula splash (any key returns)"),
                        (Act(&[Quit, Help]), "quit / toggle this help"),
                    ],
                ),
            ];
            // What to print in the key column: a literal, or every chord
            // each action currently answers to.
            let keys_of = |k: &HelpKeys| -> String {
                match k {
                    Lit(s) => (*s).to_string(),
                    Act(actions) => actions
                        .iter()
                        .map(|a| app.keymap.label(*a))
                        .collect::<Vec<_>>()
                        .join(" / "),
                }
            };
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
            let area = centered_rect(f.area(), HELP_W, height);
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
                        // Rebindable chords vary in width, so the key
                        // column is padded to a fixed 14 and clipped there
                        // — an exotic binding can't shove the descriptions
                        // out of alignment.
                        let keys = truncate(&keys_of(k), 14);
                        lines.push(Line::from(vec![
                            Span::styled(format!(" {keys:<14}"), Style::default().fg(th.accent)),
                            Span::styled(
                                truncate(v, (width as usize).saturating_sub(16)),
                                Style::default().fg(th.dim),
                            ),
                        ]));
                    }
                }
                lines
            };
            f.render_widget(Paragraph::new(column(LEFT, left_a.width)), left_a);
            f.render_widget(Paragraph::new(column(RIGHT, right_a.width)), right_a);
            // Record the drawn area for click hit-testing.
            if let Some(Overlay::Help(h)) = &mut app.overlay {
                h.area = area;
            }
        }
        Overlay::Settings(view) => {
            // A tab strip over a scrolling list. Splitting the settings by
            // tab is what keeps the modal short enough for a stock 24-row
            // terminal now that the Hotkeys tab alone is forty rows.
            let cfg = crate::config::Config::load();
            let tab = view.tab;
            let rows = crate::config::settings_rows(tab);
            // Rows the modal spends on anything but settings: the tab
            // strip and its rule above the body, and a blank + hint +
            // keys + config path below it.
            const CHROME: u16 = 2 + 4;
            let want = rows.len() as u16 + CHROME + 2;
            let height = want.min(f.area().height.saturating_sub(2)).max(CHROME + 3);
            let area = centered_rect(f.area(), SETTINGS_W, height);
            let inner = render_modal_frame(f, area, " Settings ", th);

            let dim = Style::default().fg(th.dim);
            let capturing = view.capturing();

            // ---- tab strip ----
            let mut strip: Vec<Span> = Vec::new();
            let mut hits: Vec<(u16, u16)> = Vec::new();
            let mut x = inner.x;
            for (i, t) in crate::config::SETTINGS_TABS.iter().enumerate() {
                strip.push(Span::raw(" "));
                x += 1;
                let label = format!(" {} ", t.title);
                let mut style = Style::default().fg(th.dim);
                if i == tab {
                    style = Style::default()
                        .fg(th.accent)
                        .bg(th.sel_bg)
                        .add_modifier(Modifier::BOLD);
                    // Cursor parked on the strip: brighten the active tab
                    // so ←/→ visibly belong to it.
                    if view.on_tabs {
                        style = style.add_modifier(Modifier::REVERSED);
                    }
                }
                let w = label.chars().count() as u16;
                hits.push((x, x + w));
                x += w;
                strip.push(Span::styled(label, style));
            }
            let mut lines: Vec<Line> = vec![
                Line::from(strip),
                Line::from(Span::styled(
                    "─".repeat(inner.width as usize),
                    Style::default().fg(th.muted),
                )),
            ];

            // ---- body ----
            let body_h = inner.height.saturating_sub(CHROME).max(1) as usize;
            // Same stateless follow-window the panels use, in row space:
            // the selected row stays on screen without any scroll state.
            let sel_row = rows
                .iter()
                .position(|r| r.index() == Some(view.selected))
                .unwrap_or(0);
            let first_row = (sel_row + 1).saturating_sub(body_h);
            for row in rows.iter().skip(first_row).take(body_h) {
                match row {
                    crate::config::SettingsRow::Blank => lines.push(Line::from("")),
                    crate::config::SettingsRow::Header(title) => {
                        lines.push(Line::from(Span::styled(
                            format!(" {title}"),
                            Style::default().fg(th.muted).add_modifier(Modifier::BOLD),
                        )));
                    }
                    crate::config::SettingsRow::Setting(i) => {
                        let spec = crate::config::setting_at(tab, *i)
                            .expect("settings_rows indexes this tab's settings");
                        let value = cfg.value_label(spec.kind);
                        let selected = *i == view.selected && !view.on_tabs;
                        let mut label_style = Style::default();
                        let mut value_style = Style::default().fg(th.accent);
                        if selected {
                            label_style = label_style.bg(th.sel_bg).add_modifier(Modifier::BOLD);
                            value_style = value_style.bg(th.sel_bg).add_modifier(Modifier::BOLD);
                        }
                        lines.push(Line::from(vec![
                            Span::styled(format!("   {:<28}", spec.label), label_style),
                            Span::styled(format!("[{value}]"), value_style),
                        ]));
                    }
                    crate::config::SettingsRow::Hotkey(i) => {
                        let spec = crate::keymap::spec_at(*i)
                            .expect("settings_rows indexes the action table");
                        let selected = *i == view.selected && !view.on_tabs;
                        let value = if selected && capturing {
                            "press a key…".to_string()
                        } else {
                            app.keymap.display_at(*i)
                        };
                        let reach = app.keymap.reach_at(*i);
                        let ambiguous = app.keymap.is_ambiguous(*i);
                        let mut label_style = Style::default();
                        let mut value_style =
                            Style::default().fg(if reach.is_fine() && !ambiguous {
                                th.accent
                            } else {
                                th.warn
                            });
                        if selected {
                            label_style = label_style.bg(th.sel_bg).add_modifier(Modifier::BOLD);
                            value_style = value_style.bg(th.sel_bg).add_modifier(Modifier::BOLD);
                        }
                        // A row the host terminal probably can't deliver
                        // says so on the row, not only when you bind it.
                        let flag = match (ambiguous, reach) {
                            (true, _) | (_, crate::keymap::Reach::Blocked) => "✗",
                            (_, crate::keymap::Reach::Risky) => "⚠",
                            _ => " ",
                        };
                        // No brackets here, unlike the value tabs: `^]` is
                        // a bindable chord and `[^q ^]]` is unreadable.
                        lines.push(Line::from(vec![
                            Span::styled(format!("   {:<28}", spec.label), label_style),
                            Span::styled(format!("{value:<18}"), value_style),
                            Span::styled(flag.to_string(), Style::default().fg(th.warn)),
                        ]));
                    }
                }
            }
            for _ in lines.len()..(body_h + 2) {
                lines.push(Line::from(""));
            }

            // ---- footer: notice or hint, then the keys, then the file ----
            lines.push(Line::from(""));
            match &view.notice {
                Some((text, level)) => lines.push(Line::from(Span::styled(
                    truncate(&format!(" {text}"), inner.width as usize),
                    match level {
                        crate::app::NoticeLevel::Warn => Style::default().fg(th.warn),
                        crate::app::NoticeLevel::Info => Style::default().fg(th.muted),
                    },
                ))),
                None => {
                    // A row the config file has double-booked explains
                    // itself in place of its usual hint — that's the more
                    // urgent thing to say about it.
                    let shadowed = view
                        .is_hotkeys()
                        .then(|| app.keymap.shadowed_by(view.selected))
                        .filter(|names| !names.is_empty());
                    match shadowed {
                        Some(names) => lines.push(Line::from(Span::styled(
                            truncate(
                                &format!(
                                    " ✗ this key also belongs to {} — whichever is listed first wins",
                                    names.join(", ")
                                ),
                                inner.width as usize,
                            ),
                            Style::default().fg(th.warn),
                        ))),
                        None => {
                            let hint = crate::config::hint_at(tab, view.selected);
                            lines.push(Line::from(Span::styled(
                                truncate(&format!(" {hint}"), inner.width as usize),
                                dim,
                            )));
                        }
                    }
                }
            }
            lines.push(Line::from(Span::styled(
                truncate(
                    &format!(" {}", settings_keys_hint(&view)),
                    inner.width as usize,
                ),
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
                v.tab_hits = hits;
                v.first_row = first_row;
                v.body_area = Rect {
                    x: inner.x,
                    y: inner.y + 2,
                    width: inner.width,
                    height: body_h as u16,
                };
            }
        }
        Overlay::Metrics(view) => {
            // One row per live session (biggest first); then the prewarm
            // pool's spares — CLIs booted ahead of a new-agent request,
            // with no row of their own — grouped under one header so they
            // can't pass for sessions; then nebula's own two processes.
            // Above them, a rollup per agent kind so "how much is claude
            // using?" reads off in one line.
            struct Row {
                name: String,
                context: String,
                /// None = a group header, which is no one process.
                pid: Option<u32>,
                procs: u32,
                bytes: u64,
                /// None = not openable: nebula's own processes, a group
                /// header, or a pool spare (nothing to open until a
                /// CreateAgent adopts it).
                sref: Option<SessionRef>,
            }
            let mut rows: Vec<Row> = Vec::new();
            let mut spares: Vec<Row> = Vec::new();
            // kind label → (session count, procs, bytes); BTreeMap for a
            // stable claude / codex / cursor / shells / warm order.
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
                    // A pool spare: name it by what it booted as and where
                    // it waits, and keep it out of the live-session rows.
                    if let (SessionRef::Agent(_), Some(home)) = (&m.session, &m.prewarm) {
                        let model = home
                            .model
                            .as_deref()
                            .map(|model| format!(" · {model}"))
                            .unwrap_or_default();
                        let entry = kinds.entry("warm").or_default();
                        entry.0 += 1;
                        entry.1 += m.procs;
                        entry.2 += m.rss_bytes;
                        sessions_total += m.rss_bytes;
                        spares.push(Row {
                            name: format!("{}{model}", home.kind.as_str()),
                            context: wt_context(&home.worktree),
                            pid: Some(m.pid),
                            procs: m.procs,
                            bytes: m.rss_bytes,
                            sref: None,
                        });
                        continue;
                    }
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
                        pid: Some(m.pid),
                        procs: m.procs,
                        bytes: m.rss_bytes,
                        sref: Some(m.session.clone()),
                    });
                }
                rows.sort_by(|a, b| b.bytes.cmp(&a.bytes));
                // The spares hang off one header row as a small tree:
                // the header carries their sum, each leaf its own reading.
                if !spares.is_empty() {
                    spares.sort_by(|a, b| b.bytes.cmp(&a.bytes));
                    let count = spares.len();
                    rows.push(Row {
                        name: format!("warm spares ({count})"),
                        context: String::new(),
                        pid: None,
                        procs: spares.iter().map(|r| r.procs).sum(),
                        bytes: spares.iter().map(|r| r.bytes).sum(),
                        sref: None,
                    });
                    for (i, mut spare) in spares.into_iter().enumerate() {
                        let branch = if i + 1 == count { "└ " } else { "├ " };
                        spare.name = format!("{branch}{}", spare.name);
                        rows.push(spare);
                    }
                }
                rows.push(Row {
                    name: "nebula daemon".into(),
                    context: String::new(),
                    pid: Some(snap.daemon_pid),
                    procs: 1,
                    bytes: snap.daemon_rss_bytes,
                    sref: None,
                });
                rows.push(Row {
                    name: "nebula ui (this window)".into(),
                    context: String::new(),
                    pid: Some(std::process::id()),
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
                    let unit = match *kind {
                        "shells" => "terminal",
                        "warm" => "spare",
                        _ => "session",
                    };
                    let mut detail =
                        format!("{n} {unit}{} · {procs} proc{}", plural(*n), plural(*procs));
                    if *kind == "warm" {
                        detail.push_str(" · pre-booted for new agents");
                    }
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
                        Span::styled(
                            format!(
                                "{:>6} {:>5} ",
                                row.pid.map(|p| p.to_string()).unwrap_or_default(),
                                row.procs
                            ),
                            sel(dim),
                        ),
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
            let area = centered_rect(f.area(), MEMORY_W, height);
            let inner = render_modal_frame(f, area, " Memory ", th);
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
            let area = centered_rect_pct(f.area(), SPLIT_MODAL_PCT.0, SPLIT_MODAL_PCT.1);
            f.render_widget(Clear, area);
            // Cap first, floor second: on a tiny screen the file list keeps
            // its minimum and SPLIT_PANE_LAYOUT_MIN squeezes the diff pane
            // instead.
            let files_w = view
                .files_width
                .min(area.width.saturating_sub(crate::app::MIN_DIFF_PANE_W))
                .max(crate::app::MIN_DIFF_FILES_W);
            let [files_a, diff_a] = Layout::horizontal([
                Constraint::Length(files_w),
                Constraint::Min(SPLIT_PANE_LAYOUT_MIN),
            ])
            .areas(area);

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
                let line = search_line(&view.filter, "type to filter…", filter_area, th);
                f.render_widget(Paragraph::new(line), filter_area);
            }
            let list_inner = below_first_row(files_inner);

            if view.matches.is_empty() {
                empty_list_row(f, list_inner, NO_MATCHES, th);
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
            let area = centered_rect(f.area(), PALETTE_SIZE.0, PALETTE_SIZE.1);
            let title = if palette.query.is_empty() {
                " Jump to ".to_string()
            } else {
                format!(
                    " Jump to ({}/{}) ",
                    palette.matches.len(),
                    palette.items.len()
                )
            };
            let inner = render_modal_frame(f, area, title, th);

            // First row: the always-on fuzzy query input.
            if let Some(query_area) = row_rect(inner, 0) {
                let line = search_line(&palette.query, "type to search…", query_area, th);
                f.render_widget(Paragraph::new(line), query_area);
            }
            let list_inner = below_first_row(inner);

            if palette.matches.is_empty() {
                empty_list_row(f, list_inner, NO_MATCHES, th);
            }
            let start = palette.window_start(list_inner.height as usize);
            for (row, (i, m)) in palette.matches.iter().enumerate().skip(start).enumerate() {
                let Some(row_area) = row_rect(list_inner, row) else {
                    break;
                };
                let item = &palette.items[m.item];
                // Kind lives in the glyph's shape; its color — and the
                // hollow variant standing in for the panels' `○` — come
                // from the same status the row carries in its panel, so a
                // running session reads as running here too. The row text
                // stays quiet (dim parent path, bright leaf) so the
                // cyan-bold match highlight is the loudest thing in the
                // list, and the leaf sweeps exactly like its panel row.
                let (solid, hollow) = match &item.target {
                    // The status bar's workspace glyph, so a `/` row and
                    // the "◇ name" readout name the same thing.
                    PaletteTarget::Workspace(_) => ("◆ ", "◇ "),
                    PaletteTarget::Project(_) => ("▪ ", "▫ "),
                    PaletteTarget::Worktree(_) => ("▸ ", "▹ "),
                    PaletteTarget::Session(_) => ("● ", "○ "),
                    // The panels' "leaves nebula" arrow: a pull request row
                    // opens a browser, it doesn't move a cursor.
                    PaletteTarget::PullRequest(_) => ("↗ ", "↗ "),
                };
                // Archived rows stay quiet even if their last status was
                // live — the Sessions panel's `⊘` rule.
                let status = if item.archived { None } else { item.status };
                let (glyph, glyph_color) = if item.archived {
                    ("⊘ ", th.dim)
                } else if matches!(item.target, PaletteTarget::PullRequest(_)) {
                    // No status to carry: an open pull request wears the
                    // same accent its Worktrees-panel row does.
                    (solid, th.accent)
                } else {
                    match status {
                        Some(AgentStatus::Running) => (solid, th.warn),
                        Some(AgentStatus::Finished) if item.unseen => (solid, th.done),
                        Some(AgentStatus::Finished) => (solid, th.ok),
                        Some(AgentStatus::NeedsFeedback) => (solid, th.err),
                        Some(AgentStatus::Terminated) => (solid, th.special),
                        Some(AgentStatus::Fresh) => (solid, th.dim),
                        Some(AgentStatus::Disconnected) | None => (hollow, th.dim),
                    }
                };
                let budget = (list_inner.width as usize).saturating_sub(4);
                let shown = truncate(&item.text, budget);
                let positions = visible_positions(&m.positions, &shown, &item.text);
                let mut spans = vec![Span::styled(glyph, Style::default().fg(glyph_color))];
                spans.extend(path_highlight_spans(
                    &shown,
                    positions,
                    item.archived,
                    sweep_ramp(status, th, app.animations),
                    app.sweep_phase(),
                    th,
                ));
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
            let area = centered_rect(f.area(), FILES_SIZE.0, FILES_SIZE.1);
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
            let inner = render_modal_frame(f, area, title, th);

            // First row: the always-on fuzzy query input.
            if let Some(query_area) = row_rect(inner, 0) {
                let line = search_line(&finder.query, "type to filter…", query_area, th);
                f.render_widget(Paragraph::new(line), query_area);
            }
            let list_inner = below_first_row(inner);

            if finder.matches.is_empty() {
                empty_list_row(f, list_inner, NO_MATCHES, th);
            }
            let start = finder.window_start(list_inner.height as usize);
            for (row, (i, m)) in finder.matches.iter().enumerate().skip(start).enumerate() {
                let Some(row_area) = row_rect(list_inner, row) else {
                    break;
                };
                let path = &finder.files[m.file];
                let budget = (list_inner.width as usize).saturating_sub(2);
                let shown = truncate(path, budget);
                let positions = visible_positions(&m.positions, &shown, path);
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
            let area = centered_rect_pct(f.area(), GREP_MODAL_PCT.0, GREP_MODAL_PCT.1);
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
            let inner = render_modal_frame(f, area, title, th);

            // First row: the always-live grep query.
            if let Some(query_area) = row_rect(inner, 0) {
                let line = search_line(&view.query, "type to search…", query_area, th);
                f.render_widget(Paragraph::new(line), query_area);
            }
            let list_inner = below_first_row(inner);

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
                Some(Span::styled(NO_MATCHES, Style::default().fg(th.dim)))
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
            let area = centered_rect(f.area(), HOSTS_W, height);
            f.render_widget(Clear, area);
            let hint = if adding {
                " type user@host [dir]  Enter: connect  Esc: cancel "
            } else {
                " Enter: connect  a: new host  d: remove  Esc: close "
            };
            let block = modal_block(" SSH Hosts ", th)
                .title_bottom(Line::from(Span::styled(hint, Style::default().fg(th.dim))));
            let inner = block.inner(area);
            f.render_widget(block, area);

            if total == 0 && !adding {
                empty_list_row(f, inner, "no hosts yet — a connects to a new one", th);
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
                    let budget = (inner.width as usize).saturating_sub(2);
                    let mut spans = vec![Span::styled("+ ", Style::default().fg(th.accent))];
                    spans.extend(input_spans(input, budget, th.accent, th));
                    f.render_widget(Paragraph::new(Line::from(spans)), row_area);
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
        Overlay::AgentPresets(view) => crate::preset_overlays::draw_list(f, app, &view, th),
        Overlay::AgentPresetEditor(editor) => {
            crate::preset_overlays::draw_editor(f, app, &editor, th)
        }
        Overlay::Tree(view) => {
            let area = centered_rect_pct(f.area(), SPLIT_MODAL_PCT.0, SPLIT_MODAL_PCT.1);
            f.render_widget(Clear, area);
            // Cap first, floor second: on a tiny screen the tree keeps its
            // minimum and SPLIT_PANE_LAYOUT_MIN squeezes the preview pane
            // instead.
            let files_w = view
                .files_width
                .min(area.width.saturating_sub(crate::app::MIN_DIFF_PANE_W))
                .max(crate::app::MIN_DIFF_FILES_W);
            let [tree_a, preview_a] = Layout::horizontal([
                Constraint::Length(files_w),
                Constraint::Min(SPLIT_PANE_LAYOUT_MIN),
            ])
            .areas(area);

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
                let line = search_line(&view.filter, "type to filter…", filter_area, th);
                f.render_widget(Paragraph::new(line), filter_area);
            }
            let list_inner = below_first_row(tree_inner);

            if view.rows.is_empty() {
                empty_list_row(f, list_inner, NO_MATCHES, th);
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
                let positions = visible_positions(&r.positions, &shown, &node.name);
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
                // Line-number gutter, for real file contents only —
                // directory listings and placeholders have no lines to
                // number. Dropped entirely when the pane is too narrow to
                // leave room for the code itself.
                let num_w = view.preview_line_count.to_string().len().max(2);
                let gutter = view.preview_is_file
                    && (preview_inner.width as usize) > num_w + 1 + MIN_PREVIEW_TEXT_W;
                let lines: Vec<Line> = view
                    .preview_lines
                    .iter()
                    .enumerate()
                    .skip(scroll as usize)
                    .take(preview_inner.height as usize)
                    .map(|(i, runs)| {
                        let mut spans = Vec::with_capacity(runs.len() + 1);
                        if gutter {
                            spans.push(Span::styled(
                                format!("{:>num_w$} ", i + 1),
                                Style::default().fg(th.edge),
                            ));
                        }
                        spans.extend(runs.iter().map(|(kind, text)| {
                            Span::styled(text.clone(), token_style(*kind, th))
                        }));
                        Line::from(spans)
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

/// An action's primary chord, for a footer hint. Unbound reads as `—`,
/// which is the truth: that verb has no key right now.
fn key_hint(app: &App, action: crate::keymap::Action) -> String {
    app.keymap
        .first(action)
        .map(|c| c.display())
        .unwrap_or_else(|| "—".into())
}

/// The keys line at the bottom of the settings overlay. It changes with
/// what the cursor is on, because the three places it can be — the tab
/// strip, a value row, a hotkey row — take genuinely different keys, and a
/// single union of all of them would read as noise.
fn settings_keys_hint(view: &crate::app::SettingsView) -> &'static str {
    if view.capturing() {
        return "press the key you want   Esc: cancel";
    }
    if view.capture.is_some() {
        return "Enter: reassign it here   Esc: leave it where it is";
    }
    if view.on_tabs {
        return "←/→: tab   ↓: into the list   1-9: jump   R: reset all   Esc: close";
    }
    if view.is_hotkeys() {
        return "Enter: rebind  a: add  ⌫: default  x: unbind  R: reset all  Tab: next  ↑: tabs";
    }
    "↑/↓: move  Enter: toggle  ←/→: cycle  R: reset all  Tab: next tab  ↑ at top: tabs"
}

pub(crate) fn centered_rect(frame: Rect, width: u16, height: u16) -> Rect {
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

/// A modal's inner rect minus its first row — the list under an always-on
/// filter input, which every fuzzy overlay lays out the same way.
fn below_first_row(inner: Rect) -> Rect {
    Rect {
        y: inner.y + 1,
        height: inner.height.saturating_sub(1),
        ..inner
    }
}

/// The match positions that still point at real characters once `full`
/// was truncated to `shown`: truncation puts `…` at the last char of
/// `shown`, and a match landing on that index must not light the ellipsis.
/// Untruncated text keeps every position.
fn visible_positions<'a>(positions: &'a [usize], shown: &str, full: &str) -> &'a [usize] {
    let shown_len = shown.chars().count();
    if shown_len < full.chars().count() {
        let keep = positions.iter().take_while(|&&p| p + 1 < shown_len).count();
        &positions[..keep]
    } else {
        positions
    }
}

/// A sidebar column's rect minus its right rule column.
fn shrink_r(area: Rect) -> Rect {
    Rect {
        width: area.width.saturating_sub(1),
        ..area
    }
}

/// The Workspaces bar's rect minus its bottom rule row.
fn shrink_b(area: Rect) -> Rect {
    Rect {
        height: area.height.saturating_sub(1),
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
    for i in app.splitter_indices() {
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

/// The frame every accent modal shares — rounded accent border, bold accent
/// title — so the overlays can't drift apart one border style at a time.
pub(crate) fn modal_block<'a>(title: impl Into<std::borrow::Cow<'a, str>>, th: Theme) -> Block<'a> {
    Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(th.accent))
        .title(Span::styled(
            title,
            Style::default().fg(th.accent).add_modifier(Modifier::BOLD),
        ))
}

/// Clear `area` and draw a [`modal_block`] over it, returning the inner
/// rect the modal's content goes in.
fn render_modal_frame<'a>(
    f: &mut Frame,
    area: Rect,
    title: impl Into<std::borrow::Cow<'a, str>>,
    th: Theme,
) -> Rect {
    f.render_widget(Clear, area);
    let block = modal_block(title, th);
    let inner = block.inner(area);
    f.render_widget(block, area);
    inner
}

/// A dim one-line placeholder on the first row of an otherwise empty list,
/// when the list has a first row at all.
pub(crate) fn empty_list_row(f: &mut Frame, list_inner: Rect, text: &str, th: Theme) {
    if let Some(row_area) = row_rect(list_inner, 0) {
        f.render_widget(
            Paragraph::new(Span::styled(text, Style::default().fg(th.dim))),
            row_area,
        );
    }
}

/// An empty panel's one-line nudge: accent keys and dim prose alternating,
/// the first key sitting in the row gutter so it lines up with row text.
fn hint_line(pairs: &[(&str, &str)], th: Theme) -> Line<'static> {
    let mut spans = Vec::with_capacity(pairs.len() * 2);
    for (i, (key, prose)) in pairs.iter().enumerate() {
        let key = if i == 0 {
            format!("{ROW_GUTTER}{key}")
        } else {
            key.to_string()
        };
        spans.push(Span::styled(key, Style::default().fg(th.accent)));
        spans.push(Span::styled(prose.to_string(), Style::default().fg(th.dim)));
    }
    Line::from(spans)
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

/// Unwatched-finish count badge for a project or worktree row: how many
/// sessions under it went green with nobody looking (`Agent::unseen`), as
/// ` n done` in the done color — the same word and hue the Workspaces
/// tabs use, so a count reads the same at every tier. The count is the number
/// of terminals to go read; it drops as the cursor lands on each one, and
/// the badge goes with it at zero.
fn unseen_badge(unseen: usize, th: Theme) -> Option<(String, Style)> {
    (unseen > 0).then(|| (format!(" {unseen} done"), Style::default().fg(th.done)))
}

/// The trailing badges of a project or worktree row, and the columns they
/// take together, so the name can be truncated around them.
fn row_badges(unseen: usize, th: Theme) -> (Vec<(String, Style)>, usize) {
    let badges: Vec<(String, Style)> = unseen_badge(unseen, th).into_iter().collect();
    let len = badges.iter().map(|(s, _)| s.chars().count()).sum();
    (badges, len)
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
    let chars: Vec<char> = text.chars().collect();
    if chars.is_empty() {
        return Vec::new();
    }
    let len = chars.len();
    chars
        .into_iter()
        .enumerate()
        .map(|(i, c)| Span::styled(c.to_string(), sweep_style(base, ramp, phase, i, len)))
        .collect()
}

/// Off-text cells appended to the sweep period: the pause between passes.
const SWEEP_GAP: usize = 4;

/// The shade cell `index` of a `len`-cell sweeping run takes at `phase`.
/// Split out of [`sweep_spans`] so the `/` palette can sweep a row's leaf
/// segment on the same band while the rest of the row keeps its own styling.
fn sweep_style(base: Style, ramp: [Color; 3], phase: usize, index: usize, len: usize) -> Style {
    let head = phase % (len + SWEEP_GAP);
    match head.checked_sub(index) {
        Some(0) => base.fg(ramp[2]).add_modifier(Modifier::BOLD),
        Some(1) => base.fg(ramp[1]),
        _ => base.fg(ramp[0]),
    }
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

/// Columns a row's name must keep before the "23m ago" label is worth
/// the space it costs. Below this the label drops and the name gets it all.
const MIN_NAME_W: usize = 8;

/// " 23m ago" for a list row, or empty for one that has never run. Reads
/// the raw status stamp rather than the sort key, so a session that has
/// been working for an hour says "1h ago" — when you last spoke to it —
/// instead of a permanent "just now". Worktree and project rows pass the
/// newest stamp under them and read the same way.
fn ago_badge(status_changed_at: i64) -> String {
    if status_changed_at <= 0 {
        return String::new();
    }
    match crate::hosts::ago_label(crate::app::now_ms() - status_changed_at) {
        s if s.is_empty() => s,
        s => format!(" {s}"),
    }
}

/// Fit an ago label into `free` columns beside a name: the label and the
/// columns the name keeps. A narrow panel spends its columns on the name —
/// the label drops out entirely rather than squeezing the title to nothing.
fn fit_ago(ago: String, free: usize) -> (String, usize) {
    match free.checked_sub(ago.chars().count()) {
        Some(rest) if rest >= MIN_NAME_W => (ago, rest),
        _ => (String::new(), free),
    }
}

/// The dot. `unseen` splits the finished state in two: violet while a
/// finished turn is still unread — the one state that wants a human — and
/// green once the cursor has been on it, which is a result filed away, not
/// a job. Every other status ignores the flag.
fn status_dot(status: Option<AgentStatus>, unseen: bool, th: Theme) -> Span<'static> {
    let finished = if unseen { th.done } else { th.ok };
    match status {
        Some(AgentStatus::Fresh) => Span::styled("● ", Style::default().fg(th.dim)),
        Some(AgentStatus::Running) => Span::styled("● ", Style::default().fg(th.warn)),
        Some(AgentStatus::Finished) => Span::styled("● ", Style::default().fg(finished)),
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
pub(crate) fn render_row(
    f: &mut Frame,
    area: Rect,
    spans: Vec<Span>,
    selected: bool,
    focused: bool,
    th: Theme,
) {
    render_button(f, area, vec![spans], selected, focused, th, 0);
}

/// Render one list entry as a button `area.height` rows tall: the
/// selection fill covers the whole rect, the `▌` marker runs down its
/// left edge, and `text` takes consecutive rows starting at `text_row`
/// (0-based, inside the rect). A second entry is a terminal's answer to a
/// smaller line under the first, so the caller must size `area` for it.
/// Dim spans (idle dots, archived names, subtitles) would sink into the
/// selection fill, so they get lifted to muted there.
fn render_button<'a>(
    f: &mut Frame,
    area: Rect,
    mut text: Vec<Vec<Span<'a>>>,
    selected: bool,
    focused: bool,
    th: Theme,
    text_row: u16,
) {
    if selected {
        for s in text.iter_mut().flatten() {
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
    let mut lines: Vec<Line> = Vec::with_capacity(area.height as usize);
    for r in 0..area.height {
        let mut spans = vec![marker()];
        if let Some(row) = r
            .checked_sub(text_row)
            .and_then(|i| text.get_mut(i as usize))
        {
            spans.append(row);
        }
        lines.push(Line::from(spans));
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
/// The selection rail owns the pill's first column outright: a solid `█`
/// on the text row, the pad's own `PILL_HALF` glyph on the pads. A
/// half-width `▌` can't run the pill's full height — a cell holds one
/// glyph and two colors, so a quadrant cap on a pad row strands the fill
/// quarter beside it on bare panel background, which `focus_tint` turns
/// into a black notch at each of the pill's left corners.
const PILL_RAIL: &str = "█";

/// Render one list entry into a 3-row cell starting at `top`: half-block
/// pad, text, half-block pad. The name sits on the middle row so it
/// stays vertically centered in the ~2-row pill. The pads run the full
/// width so the fill has no dark notch beside the status dot, and the
/// `PILL_RAIL` column carries the pad's own half-block in the rail color
/// so the rail spans the pill's full visual height without stranding a
/// bare-background quarter at either left corner. Dim spans get lifted
/// to muted on the fill, same as `render_button`.
fn render_pill(
    f: &mut Frame,
    inner: Rect,
    top: isize,
    mut spans: Vec<Span>,
    selected: bool,
    focused: bool,
    th: Theme,
) {
    let Some(text_area) = row_rect_at(inner, top + 1) else {
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
        let mut pad = |glyph: char, row: isize| {
            if let Some(r) = row_rect_at(inner, row) {
                f.render_widget(
                    Paragraph::new(Line::from(Span::styled(
                        glyph.to_string().repeat(inner.width as usize),
                        Style::default().fg(fill),
                    ))),
                    r,
                );
                // Same half-block, rail-colored: the rail's cap and the
                // fill quarter beside it are one cell, so they have to be
                // one color, and the rail is the one worth keeping.
                f.render_widget(
                    Paragraph::new(Span::styled(glyph.to_string(), Style::default().fg(rail))),
                    Rect { width: 1, ..r },
                );
            }
        };
        pad(PILL_HALF.0, top);
        pad(PILL_HALF.1, top + 2);
    }
    let marker = if selected && focused {
        Span::styled(PILL_RAIL, Style::default().fg(th.accent))
    } else if selected {
        Span::styled(PILL_RAIL, Style::default().fg(th.dim))
    } else {
        Span::raw(" ")
    };
    spans.insert(0, marker);
    f.render_widget(
        Paragraph::new(Line::from(spans)).style(row_bar(selected, focused, th)),
        text_area,
    );
}

/// The Workspaces bar: `WORKSPACES` on the left, on the same row-1 / x-3
/// grid the panel headers use so it sits directly above `PROJECTS` and
/// reads as the tier over it — then one tab per workspace to its right,
/// each carrying the rolled-up status of every live agent underneath, so a
/// run finishing (or asking for feedback) in a workspace you don't have
/// open still shows at the top level. The open workspace is the selected
/// tab, and picking a tab IS a switch, the way moving in the Projects
/// column re-scopes the worktrees.
///
/// Tabs answer to `⌘1`..`⌘9` / `1`..`9` from anywhere, to `←`/`→` once the
/// bar has focus, and to a click. A blank row sits above the tabs and
/// another below them, so the bar reads as its own tier rather than as a
/// header crowded against its rule. That rule — the bar's last row — closes
/// it off from the panels, broken under the open tab, so that tab reads as
/// attached to what's below it.
fn draw_workspaces_bar(f: &mut Frame, app: &mut App, area: Rect) {
    let th = app.theme;
    let focused = app.focus == Focus::Workspaces;
    if area.width == 0 || area.height < 3 {
        return;
    }
    // Last row is the rule; the label and the tabs share `area.y + 1`,
    // where a panel header lands too. Everything between that row and the
    // rule is padding, so the tabs sit in air on both sides.
    let rule_y = area.y + area.height - 1;
    let row_y = area.y + 1;

    f.render_widget(
        Paragraph::new(Line::from(Span::styled(
            "─".repeat(area.width as usize),
            Style::default().fg(th.edge),
        ))),
        Rect {
            y: rule_y,
            height: 1,
            ..area
        },
    );

    // The header carries the focus signal, exactly as a column title does.
    let header_style = if focused {
        Style::default().fg(th.accent).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(th.muted).add_modifier(Modifier::BOLD)
    };
    let mut label = vec![Span::styled(
        format!("{ROW_GUTTER}WORKSPACES"),
        header_style,
    )];
    if !app.tree.workspaces.is_empty() {
        label.push(Span::styled(
            format!(" · {}", app.tree.workspaces.len()),
            Style::default().fg(th.dim),
        ));
    }
    let label_w: u16 = label.iter().map(|s| s.content.chars().count() as u16).sum();
    f.render_widget(
        Paragraph::new(Line::from(label)),
        Rect {
            y: row_y,
            height: 1,
            ..area
        },
    );

    // Everything from here is drawn over that row, so the tabs win the
    // cells they land on.
    let tabs_x = area.x + label_w + TAB_GAP;
    if app.tree.workspaces.is_empty() {
        app.hits
            .push((shrink_b(area), HitTarget::PanelBg(Focus::Workspaces)));
        if tabs_x < area.x + area.width {
            f.render_widget(
                Paragraph::new(Line::from(Span::styled(
                    "no workspaces",
                    Style::default().fg(th.dim),
                ))),
                Rect {
                    x: tabs_x,
                    y: row_y,
                    width: area.x + area.width - tabs_x,
                    height: 1,
                },
            );
        }
        return;
    }

    let active = app.tree.active_workspace_index();
    // Per-tab display data, pre-collected to end the tree borrow: name,
    // rollup, and how many sessions under it finished unread — the same
    // count the project and worktree rows carry, one tier up.
    let rows: Vec<(String, Option<AgentStatus>, usize)> = app
        .tree
        .workspaces
        .iter()
        .map(|w| {
            (
                w.name.clone(),
                app.workspace_rollup(&w.id),
                app.workspace_unseen(&w.id),
            )
        })
        .collect();
    let (phase, anim) = (app.sweep_phase(), app.animations);
    let tabs: Vec<(Vec<Span<'static>>, u16)> = rows
        .iter()
        .enumerate()
        .map(|(i, (name, roll, done))| {
            let selected = Some(i) == active;
            // Only nine tabs have a shortcut; past that the slot stays
            // blank so every name still starts on the same column.
            let mut spans = vec![Span::styled(
                if i < 9 {
                    format!(" {} ", i + 1)
                } else {
                    "   ".to_string()
                },
                Style::default().fg(if selected { th.accent } else { th.dim }),
            )];
            spans.push(status_dot(*roll, *done > 0, th));
            spans.extend(status_name_spans(
                truncate(name, TAB_NAME_MAX),
                Style::default().add_modifier(Modifier::BOLD),
                sweep_ramp(*roll, th, anim),
                phase,
            ));
            if *done > 0 {
                spans.push(Span::styled(
                    format!(" {done} done"),
                    Style::default().fg(th.done),
                ));
            }
            spans.push(Span::raw(" "));
            if selected {
                for sp in &mut spans {
                    if sp.style.fg == Some(th.dim) {
                        sp.style.fg = Some(th.muted);
                    }
                }
            }
            let w = spans
                .iter()
                .map(|s| s.content.chars().count())
                .sum::<usize>() as u16;
            (spans, w)
        })
        .collect();

    // Horizontal scroll: drop leading tabs until the open one fits. The
    // last column is reserved for the `›` overflow mark, so a tab is never
    // half-drawn under it. Stride is the tab plus the gap that follows it,
    // which over-counts the last one by `TAB_SEP` — slack, not a bug.
    let right = (area.x + area.width).saturating_sub(1);
    let budget = right.saturating_sub(tabs_x);
    let active_i = active.unwrap_or(0);
    let stride = |t: &(Vec<Span<'static>>, u16)| t.1 + TAB_SEP;
    let mut start = 0usize;
    while start < active_i && tabs[start..=active_i].iter().map(stride).sum::<u16>() > budget {
        start += 1;
    }

    let mut x = tabs_x;
    let mut drawn = start;
    for (i, (spans, w)) in tabs.iter().enumerate().skip(start) {
        if x + w > right {
            break;
        }
        let selected = Some(i) == active;
        if selected {
            // The open tab is a surface, not a highlighted row: its fill
            // takes the bar's whole height above the rule, padding rows
            // included, so it reads as one raised block carrying the name.
            f.render_widget(
                Block::default().style(Style::default().bg(th.sel_bg)),
                Rect {
                    x,
                    y: area.y,
                    width: *w,
                    height: area.height - 1,
                },
            );
        }
        f.render_widget(
            Paragraph::new(Line::from(spans.clone())).style(if selected {
                Style::default().bg(th.sel_bg).add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            }),
            Rect {
                x,
                y: row_y,
                width: *w,
                height: 1,
            },
        );
        if selected {
            // The bottom border stays under the open tab — it just turns
            // into that tab's underline, so the tab-to-content join reads
            // as a join rather than a hole. It is a half block, not a
            // heavy rule: a line glyph draws at the cell's midline, which
            // leaves a strip of unpainted background between the tab's
            // fill and the accent and reads as a gap. `▀` paints from the
            // cell's top edge, flush against the block above it.
            for cx in x..x + w {
                if let Some(cell) = f.buffer_mut().cell_mut((cx, rule_y)) {
                    cell.set_symbol("▀").set_fg(th.accent);
                }
            }
        }
        // The whole column band clicks, rule row included — a 1-row target
        // is a hard thing to hit with a mouse.
        app.hits.push((
            Rect {
                x,
                y: area.y,
                width: *w,
                height: area.height,
            },
            HitTarget::Workspace(i),
        ));
        x += w + TAB_SEP;
        drawn = i + 1;
    }
    // Overflow marks, so a workspace scrolled off the bar isn't silently
    // missing.
    let mark = |f: &mut Frame, x: u16, g: &'static str| {
        f.render_widget(
            Paragraph::new(Line::from(Span::styled(g, Style::default().fg(th.dim)))),
            Rect {
                x,
                y: row_y,
                width: 1,
                height: 1,
            },
        );
    };
    if start > 0 {
        mark(f, tabs_x.saturating_sub(1), "‹");
    }
    if drawn < tabs.len() {
        mark(f, right, "›");
    }
    // Last, so every tab wins the cells it covers.
    app.hits
        .push((shrink_b(area), HitTarget::PanelBg(Focus::Workspaces)));
}

/// Per-row display data of the Projects panel, pre-collected to end the
/// tree borrow: name, the folder name to show under it (Some only once the
/// row has been renamed away from it), rollup, unwatched-finish count,
/// last-turn stamp.
type ProjectRowData = (String, Option<String>, Option<AgentStatus>, usize, i64);

/// The same for the Worktrees panel: branch, is-root, rollup,
/// unwatched-finish count, last-turn stamp.
type WorktreeRowData = (String, bool, Option<AgentStatus>, usize, i64);

/// Columns between the `WORKSPACES` label and the first tab.
const TAB_GAP: u16 = 2;
/// Columns between two tabs. Outside either tab's fill, so the open one's
/// selection surface never runs up against its neighbour.
const TAB_SEP: u16 = 1;
/// A workspace name is truncated to this before it becomes a tab.
const TAB_NAME_MAX: usize = 20;

fn draw_projects(f: &mut Frame, app: &mut App, area: Rect) {
    let th = app.theme;
    let focused = app.focus == Focus::Projects;
    let count = Some(app.tree.visible_project_count()).filter(|n| *n > 0);
    // With the Workspaces bar hidden nothing else on screen names the open
    // workspace, so this header takes the job — the column only ever lists
    // that workspace's projects anyway. Upper-cased to stay in the header
    // voice the other columns speak in, and trimmed to what's left of the
    // row once the gutter, the ` · n` count and the column rule are paid for.
    let title = if app.show_workspaces {
        "PROJECTS".to_string()
    } else {
        let room = (area.width as usize)
            .saturating_sub(ROW_GUTTER.len() + 1 + count.map_or(0, |n| 3 + n.to_string().len()));
        truncate(&app.tree.active_workspace_name().to_uppercase(), room)
    };
    let inner = draw_column(f, area, &title, count, focused, th);

    if !app.tree.has_visible_projects() {
        f.render_widget(
            Paragraph::new(vec![
                Line::from(Span::styled(
                    format!("{ROW_GUTTER}no projects yet"),
                    Style::default().fg(th.dim),
                )),
                hint_line(&[("n", " adds one")], th),
            ]),
            inner,
        );
        app.hits.push((inner, HitTarget::PanelBg(Focus::Projects)));
        return;
    }

    let rows: Vec<ProjectRowData> = app
        .project_rows()
        .into_iter()
        .map(|i| {
            let p = &app.tree.projects[i];
            (
                p.name.clone(),
                p.folder_subtitle(),
                app.project_rollup(&p.id),
                app.project_unseen(&p.id),
                app.project_recency(&p.id).stamped,
            )
        })
        .collect();
    let mut screen_row = 0usize;
    for (row_idx, (text, folder, roll, unseen, stamped)) in rows.iter().enumerate() {
        // A renamed row grows by the one line its folder name takes, so the
        // pads above and below stay a row each either way.
        let height = PROJECT_BTN_H + folder.is_some() as u16;
        let Some(row_area) = rows_rect(inner, screen_row, height) else {
            break;
        };
        // Same badge as worktree rows: sessions that finished unwatched
        // anywhere under the project.
        let (badges, badge_len) = row_badges(*unseen, th);
        // How long since anything under the project last did something,
        // dim after the name. The column is sorted on this stamp, so the
        // label is what makes the order legible.
        let free = (inner.width as usize).saturating_sub(3 + badge_len);
        let (ago, name_max) = fit_ago(ago_badge(*stamped), free);
        // Bold name: the top of the tree reads "biggest".
        let mut spans = vec![status_dot(*roll, *unseen > 0, th)];
        spans.extend(status_name_spans(
            truncate(text, name_max),
            Style::default().add_modifier(Modifier::BOLD),
            sweep_ramp(*roll, th, app.animations),
            app.sweep_phase(),
        ));
        if !ago.is_empty() {
            spans.push(Span::styled(ago, Style::default().fg(th.dim)));
        }
        for (text, style) in badges {
            spans.push(Span::styled(text, style));
        }
        // Renaming a project is a label change, never a move on disk, so the
        // folder keeps its name on the row underneath — as a child of the
        // label, not a second label. A terminal cell has exactly one font
        // size (Kitty's OSC 66 can render half-size text, but neither
        // WezTerm nor Ghostty implements it), so "smaller" is spelled with
        // the three signals that do work everywhere: the name above is
        // BOLD at full strength, this line is the dimmest color the theme
        // has *plus* DIM (SGR 2, faint, which blends fg toward bg), and a
        // `└ ` hangs it off the name — the same tree glyph the metrics
        // modal uses. The glyph lands under the name's first letter, so
        // the folder text itself sits two columns further in.
        let mut text = vec![spans];
        if let Some(folder) = folder {
            text.push(vec![
                Span::raw("  "),
                Span::styled(
                    format!(
                        "└ {}",
                        truncate(folder, (inner.width as usize).saturating_sub(5))
                    ),
                    Style::default().fg(th.dim).add_modifier(Modifier::DIM),
                ),
            ]);
        }
        render_button(
            f,
            row_area,
            text,
            row_idx == app.sel_project,
            focused,
            th,
            PROJECT_BTN_H / 2,
        );
        app.hits.push((row_area, HitTarget::Project(row_idx)));
        screen_row += height as usize;
    }
    app.hits.push((inner, HitTarget::PanelBg(Focus::Projects)));
}

/// One laid-out entry of the Worktrees panel. Checkout rows and pull-request
/// rows share a single virtual-row layout, computed unbounded by the panel
/// height, so a project with a long open-PR list scrolls as one column.
enum WorktreeEntry {
    Header(String),
    /// Index into `visible_worktree_rows()`.
    Row(usize),
}

impl WorktreeEntry {
    /// Rows the entry occupies: a header one, a pill its 3-row cell (they
    /// stack on a `PILL_H` stride, so neighboring pads overlap).
    fn height(&self) -> usize {
        match self {
            WorktreeEntry::Row(_) => PILL_H as usize + 1,
            _ => 1,
        }
    }
}

fn draw_worktrees(f: &mut Frame, app: &mut App, area: Rect) {
    let th = app.theme;
    let focused = app.focus == Focus::Worktrees;
    // The title's count stays a worktree count: the open-PR rows below are
    // links out of nebula, and counting them here would say "9 worktrees"
    // over a list of two checkouts.
    let wt_count = app.visible_worktrees().len();
    let count = Some(wt_count).filter(|n| *n > 0);
    let inner = draw_column(f, area, "WORKTREES", count, focused, th);

    let worktrees: Vec<WorktreeRowData> = app
        .visible_worktrees()
        .iter()
        .map(|w| {
            (
                w.branch.clone(),
                w.is_main,
                app.worktree_rollup(&w.id),
                app.worktree_unseen(&w.id),
                app.worktree_recency(&w.id).stamped,
            )
        })
        .collect();
    let prs = app.visible_open_prs().to_vec();
    if worktrees.is_empty() && prs.is_empty() {
        if app.tree.has_visible_projects() {
            f.render_widget(
                Paragraph::new(hint_line(&[("n", " starts a worktree")], th)),
                inner,
            );
        }
        app.hits.push((inner, HitTarget::PanelBg(Focus::Worktrees)));
        return;
    }

    let dim = Style::default().fg(th.dim);

    // ---- lay the column out in virtual rows ----
    let mut layout: Vec<(usize, WorktreeEntry)> = Vec::new();
    let mut vrow: usize = 0;
    let header = |layout: &mut Vec<(usize, WorktreeEntry)>, vrow: &mut usize, text: String| {
        // A blank row above every group after the first keeps the groups
        // scannable without drawing more chrome.
        if *vrow > 0 {
            *vrow += 1;
        }
        let e = WorktreeEntry::Header(text);
        let h = e.height();
        layout.push((*vrow, e));
        *vrow += h;
    };
    for i in 0..worktrees.len() {
        layout.push((vrow, WorktreeEntry::Row(i)));
        vrow += PILL_H as usize;
        // An extra quiet row separates the main checkout from the true
        // worktrees below.
        if worktrees[i].1 && worktrees.len() > 1 {
            vrow += 1;
        }
    }
    if !prs.is_empty() {
        // A list cut off at the fetch cap says so rather than passing
        // itself off as the whole set.
        let more = if prs.len() >= crate::pull_request::LIST_LIMIT {
            "+"
        } else {
            ""
        };
        header(
            &mut layout,
            &mut vrow,
            format!("OPEN PRS · {}{more}", prs.len()),
        );
        for i in 0..prs.len() {
            layout.push((vrow, WorktreeEntry::Row(worktrees.len() + i)));
            vrow += PILL_H as usize;
        }
    }

    // ---- resolve the scroll offset ----
    let view_h = inner.height as usize;
    let content_h = layout.last().map_or(0, |(top, e)| top + e.height());
    // The cursor pulls the viewport, but only on the frames where it
    // actually moved — otherwise a wheel scroll would snap straight back.
    // The project is part of the anchor so switching projects re-homes the
    // column even when the row index happens to be unchanged.
    let anchor = (app.sel_project, app.sel_worktree);
    if app.worktrees_anchor != Some(anchor) {
        app.worktrees_anchor = Some(anchor);
        if let Some(pos) = layout
            .iter()
            .position(|(_, e)| matches!(e, WorktreeEntry::Row(i) if *i == app.sel_worktree))
        {
            let (top, entry) = &layout[pos];
            // Scrolling up to the first row of a group brings that group's
            // header along, so the cursor never sits under a bare edge.
            let up_to = match pos.checked_sub(1).map(|p| &layout[p]) {
                Some((h, WorktreeEntry::Header(_))) => *h,
                _ => *top,
            };
            let bottom = top + entry.height();
            if up_to < app.worktrees_scroll {
                app.worktrees_scroll = up_to;
            } else if bottom > app.worktrees_scroll + view_h {
                app.worktrees_scroll = bottom - view_h;
            }
        }
    }
    // The wheel scrolls past the end freely; the clamp lands here so it
    // can't run away from the list.
    app.worktrees_scroll = app.worktrees_scroll.min(content_h.saturating_sub(view_h));
    let scroll = app.worktrees_scroll as isize;

    // ---- draw ----
    // The main checkout renders as `branch ⌂ root` (dim badge — the branch
    // is live, the badge marks root-ness). When the ago label leaves no
    // room for the word, the glyph alone still marks the row: at the
    // default column width `main ⌂ 23m ago` is what fits.
    const ROOT_BADGE: &str = " ⌂ root";
    const ROOT_GLYPH: &str = " ⌂";
    for (pos, (top, entry)) in layout.iter().enumerate() {
        let y = *top as isize - scroll;
        if y >= view_h as isize {
            break;
        }
        let hit_h = pill_hit_height(*top, layout.get(pos + 1).map(|(t, _)| *t));
        match entry {
            WorktreeEntry::Header(text) => {
                if let Some(r) = row_rect_at(inner, y) {
                    f.render_widget(Paragraph::new(Span::styled(format!(" {text}"), dim)), r);
                }
            }
            WorktreeEntry::Row(i) if *i < worktrees.len() => {
                let (branch, is_main, roll, unseen, stamped) = &worktrees[*i];
                let (badges, badge_len) = row_badges(*unseen, th);
                let ramp = sweep_ramp(*roll, th, app.animations);
                // 3, not 2: the dot's two cells plus the pill marker
                // `render_pill` prepends — bill them here or the trailing
                // badge is what falls off the end of a twenty-cell column.
                let free = (inner.width as usize).saturating_sub(3 + badge_len);
                // How long since a session in this checkout last did
                // something — the stamp the group is sorted on, so the
                // label is what makes the order legible. It yields to the
                // branch name first (same rule as the session rows)...
                let (ago, free) = fit_ago(ago_badge(*stamped), free);
                // ...and the root badge then yields to a branch it would push
                // into an ellipsis: in a narrow column `main 1 done` beats
                // `ma… ⌂ root 1 done` — the ⌂ is the least load-bearing
                // thing on the row, the branch is the row's identity. It
                // shrinks to the bare glyph before it goes.
                let fits = |badge: &str| {
                    branch.chars().count() <= free.saturating_sub(badge.chars().count())
                };
                let root = if !*is_main {
                    None
                } else if fits(ROOT_BADGE) {
                    Some(ROOT_BADGE)
                } else if fits(ROOT_GLYPH) {
                    Some(ROOT_GLYPH)
                } else {
                    None
                };
                let max = free - root.map_or(0, |r| r.chars().count());
                let mut spans = vec![status_dot(*roll, *unseen > 0, th)];
                spans.extend(status_name_spans(
                    truncate(branch, max),
                    Style::default(),
                    ramp,
                    app.sweep_phase(),
                ));
                if let Some(root) = root {
                    spans.push(Span::styled(root, Style::default().fg(th.dim)));
                }
                if !ago.is_empty() {
                    spans.push(Span::styled(ago, Style::default().fg(th.dim)));
                }
                for (text, style) in badges {
                    spans.push(Span::styled(text, style));
                }
                render_pill(f, inner, y, spans, *i == app.sel_worktree, focused, th);
                if let Some(hit) = rows_rect_at(inner, y, hit_h) {
                    app.hits.push((hit, HitTarget::Worktree(*i)));
                }
            }
            WorktreeEntry::Row(i) => {
                // A pull request reads like the Sessions panel's link rows —
                // the arrow says "leaves nebula". The group header already
                // says these are open, so only a draft earns a badge; in a
                // column this narrow the width is better spent on the title.
                let pr = &prs[*i - worktrees.len()];
                let badge = pr.is_draft.then(|| format!(" {}", pr.badge()));
                let badge_len = badge.as_ref().map_or(0, |b| b.chars().count());
                let label_max = (inner.width as usize)
                    .saturating_sub(3)
                    .saturating_sub(badge_len);
                let mut spans = vec![
                    Span::styled("↗ ", Style::default().fg(th.accent)),
                    Span::styled(
                        truncate(&pr.label(), label_max),
                        Style::default().fg(th.muted),
                    ),
                ];
                if let Some(badge) = badge {
                    spans.push(Span::styled(badge, Style::default().fg(th.dim)));
                }
                render_pill(f, inner, y, spans, *i == app.sel_worktree, focused, th);
                if let Some(hit) = rows_rect_at(inner, y, hit_h) {
                    app.hits.push((hit, HitTarget::Worktree(*i)));
                }
            }
        }
    }

    // Panel background (registered last so rows win the hit-test).
    app.hits.push((inner, HitTarget::PanelBg(Focus::Worktrees)));
}

/// One laid-out entry of the Sessions panel. Group headers and session
/// rows share a single virtual-row layout, computed unbounded by the
/// panel height, so the whole column can scroll as one list.
enum SessionEntry {
    Header(String),
    /// The ARCHIVED group header, in whichever form the toggle is in.
    ArchivedHeader(String),
    /// Index into `visible_session_rows()`.
    Row(usize),
}

impl SessionEntry {
    /// Rows the entry occupies: a header one, a pill its 3-row cell (they
    /// stack on a `PILL_H` stride, so neighboring pads overlap).
    fn height(&self) -> usize {
        match self {
            SessionEntry::Row(_) => PILL_H as usize + 1,
            _ => 1,
        }
    }
}

fn draw_sessions(f: &mut Frame, app: &mut App, area: Rect) {
    let th = app.theme;
    let focused = app.focus == Focus::Sessions;
    // The title's count is a session count: link rows are bookmarks, and
    // counting them here would say "4 sessions" over a list of two.
    let visible = app
        .visible_session_rows()
        .iter()
        .filter(|r| r.as_link().is_none())
        .count();
    let count = Some(visible).filter(|n| *n > 0);
    let inner = draw_column(f, area, "SESSIONS", count, focused, th);

    let rows = app.visible_session_rows();
    if rows.is_empty() && app.selected_worktree().is_some() {
        f.render_widget(
            Paragraph::new(hint_line(&[("n", " agent · "), ("t", " terminal")], th)),
            inner,
        );
    }
    let (active_count, archived_count) = app.session_group_counts();
    let terminal_count = rows
        .iter()
        .filter(|r| matches!(r, SessionRow::Terminal(_)))
        .count();
    let link_count = rows.iter().filter(|r| r.as_link().is_some()).count();
    let dim = Style::default().fg(th.dim);

    // ---- lay the column out in virtual rows ----
    let mut layout: Vec<(usize, SessionEntry)> = Vec::new();
    let mut vrow: usize = 0;
    let header = |layout: &mut Vec<(usize, SessionEntry)>, vrow: &mut usize, e: SessionEntry| {
        // A blank row above every group after the first keeps the groups
        // scannable without drawing more chrome.
        if *vrow > 0 {
            *vrow += 1;
        }
        let h = e.height();
        layout.push((*vrow, e));
        *vrow += h;
    };
    let push_rows =
        |layout: &mut Vec<(usize, SessionEntry)>, vrow: &mut usize, start: usize, len: usize| {
            for i in start..(start + len).min(rows.len()) {
                layout.push((*vrow, SessionEntry::Row(i)));
                *vrow += PILL_H as usize;
            }
        };

    // The live agents are one flat list with no header of its own — the
    // headers below name what *isn't* an agent.
    push_rows(&mut layout, &mut vrow, 0, active_count);
    if terminal_count > 0 {
        header(
            &mut layout,
            &mut vrow,
            SessionEntry::Header("TERMINALS".into()),
        );
        push_rows(&mut layout, &mut vrow, active_count, terminal_count);
    }
    if link_count > 0 {
        header(
            &mut layout,
            &mut vrow,
            SessionEntry::Header("OPEN PRS".into()),
        );
        push_rows(
            &mut layout,
            &mut vrow,
            active_count + terminal_count,
            link_count,
        );
    }
    if archived_count > 0 {
        let text = if app.show_archived {
            format!(" ARCHIVED · {archived_count} (A hides)")
        } else {
            format!(" … {archived_count} archived (A shows)")
        };
        header(&mut layout, &mut vrow, SessionEntry::ArchivedHeader(text));
        if app.show_archived {
            let start = active_count + terminal_count + link_count;
            push_rows(
                &mut layout,
                &mut vrow,
                start,
                rows.len().saturating_sub(start),
            );
        }
    }

    // ---- resolve the scroll offset ----
    let view_h = inner.height as usize;
    let content_h = layout.last().map_or(0, |(top, e)| top + e.height());
    // The cursor pulls the viewport, but only on the frames where it
    // actually moved — otherwise a wheel scroll would snap straight back.
    let anchor = (app.sel_worktree, app.sel_session);
    if app.sessions_anchor != Some(anchor) {
        app.sessions_anchor = Some(anchor);
        if let Some(pos) = layout
            .iter()
            .position(|(_, e)| matches!(e, SessionEntry::Row(i) if *i == app.sel_session))
        {
            let (top, entry) = &layout[pos];
            // Scrolling up to the first row of a group brings that group's
            // header along, so the cursor never sits under a bare edge.
            let up_to = match pos.checked_sub(1).map(|p| &layout[p]) {
                Some((h, SessionEntry::Header(_) | SessionEntry::ArchivedHeader(_))) => *h,
                _ => *top,
            };
            let bottom = top + entry.height();
            if up_to < app.sessions_scroll {
                app.sessions_scroll = up_to;
            } else if bottom > app.sessions_scroll + view_h {
                app.sessions_scroll = bottom - view_h;
            }
        }
    }
    // The wheel scrolls past the end freely; the clamp lands here so it
    // can't run away from the list.
    app.sessions_scroll = app.sessions_scroll.min(content_h.saturating_sub(view_h));
    let scroll = app.sessions_scroll as isize;

    // ---- draw ----
    for (pos, (top, entry)) in layout.iter().enumerate() {
        let y = *top as isize - scroll;
        if y >= view_h as isize {
            break;
        }
        let hit_h = pill_hit_height(*top, layout.get(pos + 1).map(|(t, _)| *t));
        match entry {
            SessionEntry::Header(text) => {
                if let Some(r) = row_rect_at(inner, y) {
                    f.render_widget(Paragraph::new(Span::styled(format!(" {text}"), dim)), r);
                }
            }
            SessionEntry::ArchivedHeader(text) => {
                // Both header forms are click targets: a click expands or
                // collapses the group, same as the A key.
                if let Some(r) = row_rect_at(inner, y) {
                    f.render_widget(Paragraph::new(Span::styled(text.as_str(), dim)), r);
                    app.hits.push((r, HitTarget::ArchivedHeader));
                }
            }
            SessionEntry::Row(i) => {
                draw_session_row(f, app, inner, y, hit_h, *i, &rows[*i], focused)
            }
        }
    }

    // Panel background (registered last so rows win the hit-test).
    app.hits.push((inner, HitTarget::PanelBg(Focus::Sessions)));
}

/// `hit_h` is the row's click target height (see [`pill_hit_height`]).
#[allow(clippy::too_many_arguments)]
fn draw_session_row(
    f: &mut Frame,
    app: &mut App,
    inner: Rect,
    top: isize,
    hit_h: u16,
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
                status_dot(Some(a.status), a.unseen && !a.archived, th)
            };
            // Muted names: sessions sit at the bottom of the tree, so
            // their text reads "smallest" next to the bold project
            // buttons.
            let name_style = if a.archived {
                Style::default().fg(th.dim)
            } else {
                Style::default().fg(th.muted)
            };
            // The CLI behind the session, as a dim trailing badge (same
            // idiom as the worktree root row) — every kind, so the column
            // reads as one consistent "name · when · harness" list. A turn
            // that finished with nobody looking takes the slot over and
            // goes loud (as a link row's unread count does): these rows
            // are what the parent rows' counts are counting, so each one
            // says so until the cursor lands on it.
            let (badge, badge_style) = if a.unseen && !a.archived {
                (" done".to_string(), Style::default().fg(th.done))
            } else if a.cloud_mirroring && !a.archived {
                // Following the cloud session: the pane is re-pulled on a
                // timer, so what it shows is the cloud agent's own work.
                // Worth saying loudly — otherwise a pane that changes on
                // its own looks like a glitch.
                (" cloud ↻".to_string(), Style::default().fg(th.accent))
            } else if a.cloud_session_id.is_some() {
                // A Claude Cloud row: the harness that matters is the cloud
                // sandbox, and the badge is how the user tells this row
                // re-enters that session rather than booting a local CLI.
                (" cloud".to_string(), Style::default().fg(th.dim))
            } else {
                (format!(" {}", a.kind.as_str()), Style::default().fg(th.dim))
            };
            // How long since this session last did anything, sat between
            // the name and the harness. The list is sorted on this stamp,
            // so the label is what makes the order legible.
            let ago = ago_badge(a.status_changed_at);
            // 3 = the pill's selection marker plus the status dot, both of
            // which render ahead of the name.
            let free = (width.saturating_sub(3) as usize).saturating_sub(badge.chars().count());
            let (ago, name_max) = fit_ago(ago, free);
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
            if !ago.is_empty() {
                spans.push(Span::styled(ago, Style::default().fg(th.dim)));
            }
            spans.push(Span::styled(badge, badge_style));
            spans
        }
        SessionRow::Terminal(t) => {
            // Shell prompt glyph instead of a status dot; dim once the
            // shell has exited (re-attach respawns it).
            let glyph_color = if t.alive { th.ok } else { th.dim };
            vec![
                Span::styled("❯ ", Style::default().fg(glyph_color)),
                Span::styled(
                    truncate(&t.name, width.saturating_sub(3) as usize),
                    Style::default().fg(th.muted),
                ),
            ]
        }
        SessionRow::Link(l) => {
            // Same shape as an agent row — glyph, name, trailing badge — so
            // the column reads as one list. The arrow says "leaves nebula";
            // a pull request earns the accent, everything else is as quiet
            // as a terminal row.
            //
            // The badge slot is normally the dim state word, but comments
            // that landed since the row was last opened take it over and go
            // loud: an unread count is the one thing here worth walking
            // over to look at, and the state is already in the glyph.
            let pr = l.pull_request();
            let unseen = l.unseen_comments(&app.pr_seen);
            let badge = match pr {
                Some(_) if unseen > 0 => Some((format!(" {unseen} new"), th.warn)),
                Some(pr) => Some((format!(" {}", pr.badge()), th.dim)),
                None => None,
            };
            let badge_len = badge.as_ref().map_or(0, |(b, _)| b.chars().count());
            let glyph_color = match pr {
                Some(pr) if pr.is_open() => th.accent,
                Some(_) => th.dim,
                None => th.muted,
            };
            let label_max = (width.saturating_sub(3) as usize).saturating_sub(badge_len);
            let mut spans = vec![
                Span::styled("↗ ", Style::default().fg(glyph_color)),
                Span::styled(
                    truncate(&l.label(), label_max),
                    Style::default().fg(th.muted),
                ),
            ];
            if let Some((badge, color)) = badge {
                spans.push(Span::styled(badge, Style::default().fg(color)));
            }
            spans
        }
    };
    render_pill(f, inner, top, spans, index == app.sel_session, focused, th);
    if let Some(hit) = rows_rect_at(inner, top, hit_h) {
        app.hits.push((hit, HitTarget::Session(index)));
    }
}

/// The pull-request reading pane. Replaces the session view while the
/// Worktrees cursor rests on an open-PR row, or the focused Sessions cursor
/// on the PR ROW (`App::previewed_pr`): headline, description, then the
/// conversation, scrolled by `pr_preview_scroll`.
///
/// The line count is written back to `app.pr_preview_lines` so the scroll
/// handlers know how far down they may go — the pane is the only thing that
/// knows how wide the prose wrapped.
fn draw_pr_preview(f: &mut Frame, app: &mut App, area: Rect, focused: bool) {
    let th = app.theme;
    let Some(pr) = app.previewed_pr() else {
        return;
    };
    let detail = app.pr_detail.get(&pr.url).cloned();
    let failed = app.pr_detail_failed.contains(&pr.url);

    let left = vec![
        Span::styled(" · ".to_string(), Style::default().fg(th.dim)),
        Span::styled(format!("#{}", pr.number), Style::default().fg(th.muted)),
    ];
    // The right-hand tag is the pane's state word, the same slot the PTY
    // view uses for "exited" / "scroll N" / "INPUT". A loaded PR needs none:
    // its state is the first thing in the body.
    let right = match (&detail, failed) {
        (Some(_), _) => None,
        (None, true) => Some(Span::styled(
            "unavailable".to_string(),
            Style::default().fg(th.err).add_modifier(Modifier::BOLD),
        )),
        (None, false) => Some(Span::styled(
            "loading…".to_string(),
            Style::default().fg(th.dim),
        )),
    };
    let inner = titled_frame(f, area, "PULL REQUEST", left, right, focused, th);
    let inner = Rect {
        x: inner.x + 1,
        width: inner.width.saturating_sub(1),
        ..inner
    };
    app.term_area = inner;
    app.hits.push((inner, HitTarget::TerminalPane));
    // Nothing in this pane is a PTY, so the link/file scanners have nothing
    // to find — clear them or ⌥click would still hit last frame's hits.
    app.term_links = Vec::new();
    app.term_file_links = Vec::new();

    // The placeholders wrap through the same helper the body does: the pane
    // is as narrow as the user drags it, and ratatui clips an overwide line
    // rather than folding it.
    let placeholder = |message: &str| {
        let w = (inner.width as usize).saturating_sub(2).max(20);
        let row = |text: &str, style: Style| Line::from(Span::styled(format!(" {text}"), style));
        let mut lines = vec![Line::from("")];
        lines.extend(
            crate::pr_preview::wrap(&pr.label, w)
                .iter()
                .map(|t| row(t, Style::default().fg(th.muted))),
        );
        lines.push(Line::from(""));
        lines.extend(
            crate::pr_preview::wrap(message, w)
                .iter()
                .map(|t| row(t, Style::default().fg(th.dim))),
        );
        lines
    };
    let lines: Vec<Line> = match (&detail, failed) {
        (Some(detail), _) => crate::pr_preview::lines(detail, inner.width as usize, th),
        (None, true) => placeholder(&format!(
                "couldn't read this pull request — is `gh` installed and logged in?                  {} still opens it in the browser.",
            key_hint(app, Action::Activate)
        )),
        (None, false) => placeholder("reading it…"),
    };
    app.pr_preview_lines = lines.len();
    // Clamp here rather than in the handlers: the pane is what knows how
    // many rows the prose wrapped to, and a narrower window can strand the
    // offset past the end.
    let max = (lines.len() as u16).saturating_sub(inner.height.max(1));
    let scroll = app.pr_preview_scroll.min(max);
    app.pr_preview_scroll = scroll;
    let shown: Vec<Line> = lines.into_iter().skip(scroll as usize).collect();
    f.render_widget(Paragraph::new(shown), inner);
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
    titled_frame(f, area, "TERMINAL", left, right, focused, th)
}

/// The same frame under another name, for the pane's other tenants — the
/// pull-request reader borrows the whole right-hand column, and calling it
/// TERMINAL while it shows prose would be a lie.
fn titled_frame(
    f: &mut Frame,
    area: Rect,
    title: &str,
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
        let mut spans = vec![Span::styled(format!("  {title}"), header_style)];
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
    // A cursor is resting on an open pull request — the Worktrees cursor
    // on a PROJECT OPEN PRS GROUP row, or the focused Sessions cursor on
    // the PR ROW: the pane reads it. The attachment underneath stays live —
    // walking down into either OPEN PRS group and back must not churn
    // detach/attach.
    if app.previewed_pr().is_some() {
        draw_pr_preview(f, app, area, focused);
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
        // Nothing has come off the PTY yet: the session was reaped while the
        // user was elsewhere and its CLI is booting. Say so — the blank grid
        // on its own reads as a hang.
        Some(t) if !t.painted => Some(Span::styled(
            "starting…".to_string(),
            Style::default().fg(th.dim),
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
        // Booting: the grid is empty because the CLI hasn't painted yet, so
        // there is nothing to render and nothing to scan for links. A word
        // in the middle of the pane beats an unexplained void.
        Some(term) if !term.painted && !term.exited => {
            let msg = Paragraph::new(vec![
                Line::from(""),
                Line::from(Span::styled(
                    "starting session…",
                    Style::default().fg(th.muted).add_modifier(Modifier::BOLD),
                )),
                Line::from(""),
                Line::from(Span::styled(
                    "booting — the screen appears as soon as it paints",
                    Style::default().fg(th.dim),
                )),
            ])
            .centered();
            f.render_widget(msg, inner);
            (Vec::new(), Vec::new())
        }
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
    let Some(project) = app.selected_project() else {
        return spans;
    };
    spans.push(seg(&project.name, app.focus == Focus::Projects));
    if let Some(worktree) = app.selected_worktree() {
        spans.push(sep());
        spans.push(seg(&worktree.branch, app.focus == Focus::Worktrees));
        if let Some(session) = app.selected_session_row() {
            spans.push(sep());
            // A link's crumb is its display label, not the raw URL — the
            // crumb has 20 cells and "https://" would eat eight of them.
            let name = match session.as_link() {
                Some(link) => link.label(),
                None => session.name().to_string(),
            };
            spans.push(seg(
                &name,
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

/// The bottom bar, plus its one clickable cell run: the `◇ workspace`
/// nameplate registers as a hit target so a click on it opens the switcher.
/// The bar is drawn under the splash and the collapsed view too, so the
/// registration lives here rather than in `draw`'s panel branch.
fn draw_footer(f: &mut Frame, app: &mut App, area: Rect) {
    if let Some(rect) = draw_footer_bar(f, app, area) {
        app.hits.push((rect, HitTarget::FooterWorkspace));
    }
}

/// Draw the bar; returns the screen rect of the workspace nameplate when
/// it fit on the bar.
fn draw_footer_bar(f: &mut Frame, app: &App, area: Rect) -> Option<Rect> {
    // `area` includes the blank padding row; the bar itself is its last row.
    let area = Rect {
        y: area.y + area.height.saturating_sub(1),
        height: area.height.min(1),
        ..area
    };
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
            match &app.overlay {
                Some(Overlay::Settings(view)) => settings_keys_hint(view),
                _ => "",
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
    } else if matches!(&app.overlay, Some(Overlay::AgentPresets(_))) {
        Span::styled(
            "↑/↓: select  Enter: launch with a task  a: new  e: edit  d: delete  Esc: close",
            Style::default().fg(th.dim),
        )
    } else if matches!(&app.overlay, Some(Overlay::AgentPresetEditor(_))) {
        Span::styled(
            "Tab/↑↓: next field  ←/→: cycle  Shift+Enter/^J: newline  Enter: save  Esc: back to list",
            Style::default().fg(th.dim),
        )
    } else if matches!(&app.overlay, Some(Overlay::Menu(m)) if m.is_workspace_picker()) {
        Span::styled(
            "Enter: open  n: new  r: rename  d: delete  Esc: close",
            Style::default().fg(th.dim),
        )
    } else if app.overlay.is_some() {
        Span::styled("Esc: close  Enter: confirm", Style::default().fg(th.dim))
    } else if app.splash_showing() {
        // The splash covers the panels, so every panel hotkey is dead here.
        // List only what actually fires — and in preview, that's one thing:
        // the next key dismisses it (q included).
        Span::styled(
            if app.splash_preview {
                "any key: back to panels".to_string()
            } else {
                let k = |a| key_hint(app, a);
                format!(
                    "{}/{}: add project  {}: workspaces  {}: ssh host  {}: settings  {}: help  {}: quit",
                    k(Action::New),
                    k(Action::AddProject),
                    k(Action::Workspaces),
                    k(Action::Hosts),
                    k(Action::Settings),
                    k(Action::Help),
                    k(Action::Quit),
                )
            },
            Style::default().fg(th.dim),
        )
    } else {
        // Spelled from the live keymap for the same reason the Help
        // overlay is: these are the first place a rebound key would start
        // lying.
        let k = |a| key_hint(app, a);
        let text = match app.focus {
            Focus::Terminal if app.term.as_ref().is_some_and(|t| t.exited) => {
                "session exited — Esc: back to sessions".to_string()
            }
            Focus::Terminal if app.term_locked => format!(
                "{}: panels  drag: select+copy  ⌥click: open link",
                app.keymap
                    .first(Action::UnlockTerminal)
                    .map(|c| c.display())
                    .unwrap_or_else(|| "^q".into()),
            ),
            Focus::Terminal if app.term.is_some() => format!(
                "{}: type into terminal  {}: sessions",
                k(Action::Activate),
                k(Action::FocusLeft)
            ),
            Focus::Terminal => "select a session and press Enter to attach".to_string(),
            // The cursor here is the open workspace, so ←/→ already
            // switches; the verbs are the switcher's, plus the way out.
            Focus::Workspaces => format!(
                "←/→ or 1-9: switch  {}: panels  {}: new  {}: rename  {}: delete  {}: hide bar  {}: help",
                k(Action::Activate),
                k(Action::New),
                k(Action::Rename),
                k(Action::Delete),
                k(Action::ToggleWorkspaces),
                k(Action::Help)
            ),
            Focus::Projects => format!(
                "{}/{}: add  {}: rename  {}: remove  {}: search  {}: menu  {}: help",
                k(Action::New),
                k(Action::AddProject),
                k(Action::Rename),
                k(Action::Delete),
                k(Action::Palette),
                k(Action::ContextMenu),
                k(Action::Help)
            ),
            // An open-PR row answers to a different set of verbs than a
            // checkout does, so the hint follows the cursor into the group.
            Focus::Worktrees if app.selected_worktree_pr().is_some() => format!(
                "{}: claude session  {}: open in browser  {}: diff  PgUp/PgDn: scroll  {}: search  {}: menu  {}: help",
                k(Action::New),
                k(Action::Activate),
                k(Action::GitDiff),
                k(Action::Palette),
                k(Action::ContextMenu),
                k(Action::Help)
            ),
            Focus::Worktrees => format!(
                "{}: new worktree  {}: terminal  {}: delete  {}: search  {}: menu  {}: help",
                k(Action::New),
                k(Action::NewTerminal),
                k(Action::Delete),
                k(Action::Palette),
                k(Action::ContextMenu),
                k(Action::Help)
            ),
            // A discovered pull request opens, reads in the pane and shows
            // its diff; it has no stored row to edit or delete. Previously
            // saved rows retain those two verbs.
            Focus::Sessions
                if app
                    .selected_link()
                    .is_some_and(|row| row.id().is_none()) =>
            {
                format!(
                    "{}: open in browser  {}: diff  PgUp/PgDn: scroll  {}: menu  {}: help",
                    k(Action::Activate),
                    k(Action::GitDiff),
                    k(Action::ContextMenu),
                    k(Action::Help)
                )
            }
            Focus::Sessions if app.selected_link().is_some() => format!(
                "{}: open in browser  {}: edit URL  {}: delete  {}: menu  {}: help",
                k(Action::Activate),
                k(Action::Rename),
                k(Action::Delete),
                k(Action::ContextMenu),
                k(Action::Help)
            ),
            Focus::Sessions => format!(
                "{}: focus  {}: agent  {}: presets  {}: terminal  {}: rename  {}: archive  {}: del  {}: menu  {}: help",
                k(Action::Activate),
                k(Action::New),
                k(Action::AgentPresets),
                k(Action::NewTerminal),
                k(Action::Rename),
                k(Action::Archive),
                k(Action::Delete),
                k(Action::ContextMenu),
                k(Action::Help)
            ),
        };
        let mut text = text;
        if !app.term_locked {
            let mut restore = Vec::new();
            let hint = |action, panel: &str| {
                app.keymap.first(action).map_or_else(
                    || format!("{}: show {panel} in settings", k(Action::Settings)),
                    |chord| format!("{}: show {panel}", chord.display()),
                )
            };
            if app.hide_projects {
                restore.push(hint(Action::ToggleProjects, "projects"));
            }
            if app.hide_worktrees {
                restore.push(hint(Action::ToggleWorktrees, "worktrees"));
            }
            if !restore.is_empty() {
                text = format!("{}  {text}", restore.join("  "));
            }
        }
        Span::styled(text, Style::default().fg(th.dim))
    };
    // Quiet footer: context on the left, live stats on the right. The
    // hostname only earns a slot when it's a remote session, and the
    // connection state only when something is wrong.
    let mut spans = vec![Span::raw(" ")];
    // The open workspace leads the bar — it scopes everything else shown.
    // (The version nameplate is spliced in ahead of it further down, once
    // the width left for the hints is known.)
    let mut workspace_idx = spans.len();
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
        if let Some(n) = app.selected_worktree_changes().filter(|n| *n > 0) {
            spans.push(Span::styled(
                format!("  +{n} file{}", if n == 1 { "" } else { "s" }),
                Style::default().fg(th.warn),
            ));
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
    // Which nebula this is, at the far left: the one thing on the bar that
    // never moves with the cursor, so it reads as a nameplate rather than
    // context. It costs the hints ~18 columns, which they can afford — a
    // clipped key list still spells the keys that matter, in order. A
    // clipped *flash* loses the end of a sentence, so the nameplate steps
    // aside for one that would not otherwise fit.
    let plate = format!("nebula v{}", env!("CARGO_PKG_VERSION"));
    let plate_w = plate.chars().count() + "  ·  ".chars().count();
    let body_w: usize = spans.iter().map(|s| s.width()).sum();
    if app.flash.is_none() || body_w + plate_w <= left.width as usize {
        spans.splice(
            1..1,
            [
                Span::styled(plate, Style::default().fg(th.dim)),
                Span::styled("  ·  ", Style::default().fg(th.dim)),
            ],
        );
        workspace_idx += 2;
    }
    // Where the workspace nameplate landed: everything ahead of it on the
    // bar is fixed-width chrome, so its cells are a prefix sum. Clipped
    // off the bar (a very narrow screen) means no target.
    let workspace_x: usize = spans[..workspace_idx].iter().map(|s| s.width()).sum();
    let workspace_w = spans[workspace_idx].width();
    let workspace_rect = (workspace_x + workspace_w <= left.width as usize).then(|| Rect {
        x: left.x + workspace_x as u16,
        y: left.y,
        width: workspace_w as u16,
        height: 1,
    });
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
    workspace_rect
}

/// The footer's right-edge readout: live sessions, their process count,
/// and nebula's total memory footprint (TUI + daemon + every session's
/// process subtree). None until the first metrics reply arrives.
fn footer_usage(app: &App) -> Option<String> {
    let m = app.last_metrics.as_ref()?;
    // Prewarm-pool spares are agent CLIs but not agents anyone opened;
    // they get their own count so the agent figure matches the sidebar.
    let spares = m.sessions.iter().filter(|s| s.prewarm.is_some()).count();
    let agents = m
        .sessions
        .iter()
        .filter(|s| matches!(s.session, SessionRef::Agent(_)) && s.prewarm.is_none())
        .count();
    let terms = m.sessions.len() - agents - spares;
    let total = m.daemon_rss_bytes
        + app.client_rss_bytes
        + m.sessions.iter().map(|s| s.rss_bytes).sum::<u64>();
    let plural = |n: usize| if n == 1 { "" } else { "s" };
    let warm = if spares > 0 {
        format!(" · {spares} warm")
    } else {
        String::new()
    };
    Some(format!(
        "{agents} agent{} · {terms} term{}{warm} · {}",
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
/// the way through. With a `ramp`, the leaf segment — the entity's own
/// name, the very text that sweeps in its panel row — rides the same
/// left-to-right band; matched chars keep the accent highlight so the
/// sweep never buries what the query hit.
fn path_highlight_spans(
    shown: &str,
    positions: &[usize],
    archived: bool,
    ramp: Option<[Color; 3]>,
    phase: usize,
    th: Theme,
) -> Vec<Span<'static>> {
    let boundary = shown
        .rfind('/')
        .map(|b| shown[..=b].chars().count())
        .unwrap_or(0);
    let leaf_len = shown.chars().count() - boundary;
    let hl = Style::default().fg(th.accent).add_modifier(Modifier::BOLD);
    let mut spans = Vec::new();
    let mut run = String::new();
    let mut run_style: Option<Style> = None;
    for (i, c) in shown.chars().enumerate() {
        let style = if positions.binary_search(&i).is_ok() {
            hl
        } else if archived || i < boundary {
            Style::default().fg(th.dim)
        } else if let Some(ramp) = ramp {
            sweep_style(Style::default(), ramp, phase, i - boundary, leaf_len)
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

/// Word-wrapped rows for the Claude Cloud task editor. Explicit newlines
/// always break; soft breaks prefer the last whitespace that fits. The
/// returned row index is where the caret rendered, so the caller can keep
/// that row inside its fixed-height viewport.
pub(crate) fn multiline_input_lines(
    input: &TextInput,
    width: usize,
    cursor: Color,
    th: Theme,
) -> (Vec<Line<'static>>, usize) {
    let chars: Vec<char> = input.chars().collect();
    let caret = input.cursor_chars();
    let width = width.max(1);
    let mut ranges = Vec::new();
    let mut paragraph_start = 0usize;
    loop {
        let paragraph_end = chars[paragraph_start..]
            .iter()
            .position(|c| *c == '\n')
            .map(|offset| paragraph_start + offset)
            .unwrap_or(chars.len());
        if paragraph_start == paragraph_end {
            ranges.push((paragraph_start, paragraph_end));
        } else {
            let mut start = paragraph_start;
            while start < paragraph_end {
                let hard_end = (start + width).min(paragraph_end);
                let end = if hard_end < paragraph_end {
                    chars[start..hard_end]
                        .iter()
                        .rposition(|c| c.is_whitespace())
                        .map(|offset| start + offset + 1)
                        .filter(|cut| *cut > start)
                        .unwrap_or(hard_end)
                } else {
                    hard_end
                };
                ranges.push((start, end));
                start = end;
            }
        }
        if paragraph_end == chars.len() {
            break;
        }
        paragraph_start = paragraph_end + 1;
    }

    let plain = Style::default().fg(th.text);
    let block = Style::default().fg(th.on_accent).bg(cursor);
    let mut caret_row = 0usize;
    let mut found_caret = false;
    let lines = ranges
        .into_iter()
        .enumerate()
        .map(|(row, (start, end))| {
            let mut cells: Vec<(char, bool)> =
                (start..end).map(|i| (chars[i], i == caret)).collect();
            // At EOF, on an empty line, or immediately before an explicit
            // newline, the caret needs its own blank cell.
            if (start == end && caret == start)
                || (caret == end
                    && (end == chars.len() || chars.get(end).is_some_and(|c| *c == '\n')))
            {
                cells.push((' ', true));
            }
            if cells.iter().any(|(_, is_caret)| *is_caret) {
                caret_row = row;
                found_caret = true;
            }

            let mut spans = Vec::new();
            let mut run = String::new();
            let mut run_is_caret = false;
            for (c, is_caret) in cells {
                if is_caret != run_is_caret && !run.is_empty() {
                    spans.push(Span::styled(
                        std::mem::take(&mut run),
                        if run_is_caret { block } else { plain },
                    ));
                }
                run_is_caret = is_caret;
                run.push(c);
            }
            if !run.is_empty() {
                spans.push(Span::styled(run, if run_is_caret { block } else { plain }));
            }
            Line::from(spans)
        })
        .collect::<Vec<_>>();
    if !found_caret {
        caret_row = lines.len().saturating_sub(1);
    }
    (lines, caret_row)
}

/// Spans for a one-line text field: the value with a block cursor sitting
/// where the caret is. Long values scroll under the field — the window
/// keeps the caret near the middle, and a `…` marks each end that has text
/// scrolled off it.
///
/// `cursor` colors the caret block; pass `th.dim` to park it (the prompt
/// does that while a listing row, not the text, holds Enter).
pub(crate) fn input_spans(
    input: &TextInput,
    budget: usize,
    cursor: Color,
    th: Theme,
) -> Vec<Span<'static>> {
    let chars: Vec<char> = input.chars().collect();
    let caret = input.cursor_chars();
    let budget = budget.max(1);
    // A caret parked past the last character needs one extra cell to sit in.
    let total = chars.len() + usize::from(caret >= chars.len());
    let start = if total <= budget {
        0
    } else {
        caret.saturating_sub(budget / 2).min(total - budget)
    };
    let end = (start + budget).min(total);

    let mut cells: Vec<(char, bool)> = (start..end)
        .map(|i| (chars.get(i).copied().unwrap_or(' '), i == caret))
        .collect();
    // The window is centered on the caret, so an elided edge is never the
    // caret's own cell.
    if start > 0 {
        if let Some(first) = cells.first_mut() {
            first.0 = '…';
        }
    }
    if end < total {
        if let Some(last) = cells.last_mut() {
            last.0 = '…';
        }
    }

    let plain = Style::default().fg(th.text);
    let block = Style::default().fg(th.on_accent).bg(cursor);
    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut run = String::new();
    let mut run_is_caret = false;
    for (c, is_caret) in cells {
        if is_caret != run_is_caret && !run.is_empty() {
            let style = if run_is_caret { block } else { plain };
            spans.push(Span::styled(std::mem::take(&mut run), style));
        }
        run_is_caret = is_caret;
        run.push(c);
    }
    if !run.is_empty() {
        let style = if run_is_caret { block } else { plain };
        spans.push(Span::styled(run, style));
    }
    spans
}

/// The always-live search row every fuzzy overlay shares: a dim placeholder
/// until something is typed, then the field itself.
fn search_line(input: &TextInput, placeholder: &str, area: Rect, th: Theme) -> Line<'static> {
    if input.is_empty() {
        return Line::from(Span::styled(
            placeholder.to_string(),
            Style::default().fg(th.dim),
        ));
    }
    Line::from(input_spans(input, area.width as usize, th.accent, th))
}

/// The i-th single-height row inside `inner`, or None when it overflows.
pub(crate) fn row_rect(inner: Rect, i: usize) -> Option<Rect> {
    rows_rect(inner, i, 1)
}

/// [`row_rect`] for a row that may have scrolled off the top of the
/// panel: negative indices land above it and draw nothing.
fn row_rect_at(inner: Rect, i: isize) -> Option<Rect> {
    rows_rect_at(inner, i, 1)
}

/// [`rows_rect`] for a scrolled rect: one straddling the panel top is
/// clipped to the rows still on screen, one entirely above it is None.
fn rows_rect_at(inner: Rect, i: isize, height: u16) -> Option<Rect> {
    let visible = height as isize + i.min(0);
    if visible <= 0 {
        return None;
    }
    rows_rect(inner, i.max(0) as usize, visible as u16)
}

/// Rows a pill's click target spans. A pill is a 3-row cell stacked on
/// a `PILL_H` stride, so its bottom pad is usually the next pill's top
/// pad; that shared row goes to the lower pill (whose selection fill
/// owns the cell's bottom half), and the upper one's target stops at
/// `PILL_H`. A pill with nothing stacked under it — the root checkout
/// over its quiet row, the last of a group, the last of the list — keeps
/// its bottom pad, or the lower half of the pill would be a click on the
/// panel background.
fn pill_hit_height(top: usize, next_top: Option<usize>) -> u16 {
    let cell = PILL_H as usize + 1;
    next_top.map_or(cell, |n| n.saturating_sub(top).min(cell)) as u16
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

/// Clip `s` to `max` chars, spending the last one on `…` when it had to
/// cut. Counts chars, not columns — wide glyphs are the caller's problem.
/// The one clipper for every row, title and grep hit, so they all cut the
/// same way.
pub fn truncate(s: &str, max: usize) -> String {
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
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    #[test]
    fn truncate_clips_to_max_chars_with_an_ellipsis() {
        assert_eq!(truncate("short", 10), "short");
        assert_eq!(truncate("exact", 5), "exact");
        assert_eq!(truncate("toolong", 5), "tool…");
        assert_eq!(truncate("toolong", 5).chars().count(), 5);
        assert_eq!(
            truncate("héllo wörld", 6),
            "héllo…",
            "counts chars, not bytes"
        );
        // Degenerate budgets: nothing fits but the ellipsis itself.
        assert_eq!(truncate("ab", 1), "…");
        assert_eq!(truncate("ab", 0), "…");
        assert_eq!(truncate("", 0), "");
    }

    #[test]
    fn visible_positions_drops_matches_on_the_ellipsis() {
        let full = "abcdefgh";
        let positions = [0, 3, 4, 7];
        // Truncated to 5 chars: "abcd…" — index 4 is the ellipsis, so only
        // positions before it survive; 7 is off the end entirely.
        assert_eq!(visible_positions(&positions, "abcd…", full), &[0, 3]);
        // Untruncated keeps everything, even a match on the last char.
        assert_eq!(visible_positions(&positions, full, full), &positions);
        let none: [usize; 0] = [];
        assert_eq!(visible_positions(&none, "abcd…", full), &none);
    }

    const RAMP: [Color; 3] = [Color::Yellow, Color::Indexed(220), Color::Indexed(230)];

    fn colors(spans: &[Span]) -> Vec<Color> {
        spans.iter().map(|s| s.style.fg.unwrap()).collect()
    }

    /// Render an input the way the widgets do, marking the caret cell with
    /// `[]` so placement is readable in an assertion.
    fn rendered(input: &TextInput, budget: usize) -> String {
        let th = Theme::default();
        input_spans(input, budget, th.accent, th)
            .iter()
            .map(|s| {
                if s.style.bg == Some(th.accent) {
                    format!("[{}]", s.content)
                } else {
                    s.content.to_string()
                }
            })
            .collect()
    }

    #[test]
    fn caret_sits_past_the_last_character_by_default() {
        let input = TextInput::with_text("note");
        assert_eq!(rendered(&input, 20), "note[ ]");
    }

    #[test]
    fn caret_renders_in_place_mid_string() {
        let mut input = TextInput::with_text("note");
        for _ in 0..2 {
            input.handle_key(&KeyEvent::new(KeyCode::Left, KeyModifiers::NONE));
        }
        assert_eq!(rendered(&input, 20), "no[t]e");
    }

    /// A value longer than the field scrolls under it, keeping the caret in
    /// view with a `…` on whichever end is clipped.
    #[test]
    fn long_values_scroll_around_the_caret() {
        let input = TextInput::with_text("abcdefghijklmnop");
        // Caret at the end: the tail is shown, the head elided.
        assert_eq!(rendered(&input, 8), "…klmnop[ ]");
        let mut input = input;
        input.handle_key(&KeyEvent::new(KeyCode::Home, KeyModifiers::NONE));
        // Caret at the start: the head is shown, the tail elided.
        assert_eq!(rendered(&input, 8), "[a]bcdefg…");
    }

    #[test]
    fn empty_search_fields_show_their_placeholder() {
        let th = Theme::default();
        let area = Rect::new(0, 0, 20, 1);
        let line = search_line(&TextInput::new(), "type to filter…", area, th);
        assert_eq!(line.spans[0].content.as_ref(), "type to filter…");
        let line = search_line(&TextInput::with_text("ab"), "type to filter…", area, th);
        let text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(text, "ab ");
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

    /// The selected pill's rail column is solid rail color top to bottom:
    /// the pad's own half-block on the pads, `█` on the text row. Nothing
    /// in it may be left on bare background — a quadrant cap used to
    /// strand the fill quarter beside it, which `focus_tint` then painted
    /// near-black, reading as a notch at each left corner of the pill.
    #[test]
    fn pill_rail_leaves_no_untinted_quarter_at_the_corners() {
        let th = Theme::default();
        let inner = Rect::new(0, 0, 8, 3);
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(8, 3)).unwrap();
        terminal
            .draw(|f| {
                render_pill(f, inner, 0, vec![Span::raw("● ok")], true, true, th);
            })
            .unwrap();
        let buf = terminal.backend().buffer().clone();
        let cell = |x, y| buf.cell((x, y)).unwrap().clone();

        // Pads: rail column carries the fill's own half-block, so the
        // whole cell is glyph — no background quarter survives.
        for (y, glyph) in [(0, PILL_HALF.0), (2, PILL_HALF.1)] {
            let glyph = glyph.to_string();
            let c = cell(0, y);
            assert_eq!(c.symbol(), glyph, "pad row {y} rail glyph");
            assert_eq!(c.fg, th.accent, "pad row {y} rail color");
            // The rail cell covers exactly what the fill cells beside it
            // do; a narrower glyph there is the notch coming back.
            for x in 1..8 {
                assert_eq!(
                    cell(x, y).symbol(),
                    glyph,
                    "pad row {y} fill glyph at x={x}"
                );
                assert_eq!(cell(x, y).fg, th.sel_bg, "pad row {y} fill color at x={x}");
            }
        }
        // Text row: a solid block, sitting on the fill.
        let c = cell(0, 1);
        assert_eq!(c.symbol(), PILL_RAIL);
        assert_eq!(c.fg, th.accent);
        assert_eq!(c.bg, th.sel_bg);
    }

    /// Each grip sits on its rule column (one left of the boundary), three
    /// cells centered vertically: muted at rest, accent under hover. All
    /// three visible sidebar boundaries get one.
    #[test]
    fn splitter_grips_center_on_the_rules() {
        let th = Theme::default();
        let mut app = App::new();
        let body = Rect::new(0, 0, 120, 35);
        let mut buf = ratatui::buffer::Buffer::empty(body);
        draw_splitter_grips(&mut buf, &app, body);
        let mid = body.height / 2; // 17
        assert_eq!(app.splitter_indices(), vec![0, 1, 2]);
        for i in app.splitter_indices() {
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
        app.hover_splitter = Some(0);
        draw_splitter_grips(&mut buf, &app, body);
        assert_eq!(
            buf.cell((app.splitter_x(0) - 1, mid)).unwrap().fg,
            th.accent
        );
        assert_eq!(buf.cell((app.splitter_x(1) - 1, mid)).unwrap().fg, th.muted);

        // The Workspaces bar runs across the top and owns no boundary, so
        // hiding it leaves every grip exactly where it was.
        app.show_workspaces = false;
        app.hover_splitter = None;
        let mut buf = ratatui::buffer::Buffer::empty(body);
        draw_splitter_grips(&mut buf, &app, body);
        assert_eq!(
            buf.cell((app.splitter_x(0) - 1, mid)).unwrap().symbol(),
            "┃"
        );
        app.show_workspaces = true;

        // A body too short for a grip plus breathing space draws nothing.
        let tiny = Rect::new(0, 0, 120, 6);
        let mut buf = ratatui::buffer::Buffer::empty(tiny);
        draw_splitter_grips(&mut buf, &app, tiny);
        assert!(buf.content().iter().all(|c| c.symbol() == " "));
    }

    /// A test tree: one project, `branches` as its worktrees (the first is
    /// the root checkout), `agents` and `terminals` under the first worktree.
    fn hit_test_app(branches: &[&str], agents: &[&str], terminals: &[&str]) -> App {
        use nebula_core::{Agent, AgentId, AgentStatus, Project, ProjectId, Worktree, WorktreeId};
        let mut app = App::new();
        let project_id = ProjectId("p1".into());
        app.tree.projects.push(Project {
            workspace_id: Default::default(),
            id: project_id.clone(),
            name: "demo".into(),
            repo_path: "/tmp/demo".into(),
            sort_order: 0,
        });
        for (i, branch) in branches.iter().enumerate() {
            app.tree.worktrees.push(Worktree {
                id: WorktreeId(format!("w{i}")),
                project_id: project_id.clone(),
                path: format!("/tmp/{branch}").into(),
                branch: (*branch).into(),
                is_main: i == 0,
                sort_order: i as i64,
            });
        }
        for (i, name) in agents.iter().enumerate() {
            app.tree.agents.push(Agent {
                id: AgentId(format!("a{i}")),
                worktree_id: WorktreeId("w0".into()),
                name: (*name).into(),
                status: AgentStatus::Fresh,
                archived: false,
                archived_at: 0,
                unseen: false,
                status_changed_at: 0,
                kind: nebula_core::AgentKind::Claude,
                model: None,
                effort: None,
                session_id: None,
                cloud_session_id: None,
                sort_order: i as i64,
                alive: false,
                cloud_mirroring: false,
            });
        }
        for (i, name) in terminals.iter().enumerate() {
            app.tree.terminals.push(nebula_core::TerminalTab {
                id: nebula_core::TerminalId(format!("t{i}")),
                worktree_id: WorktreeId("w0".into()),
                name: (*name).into(),
                sort_order: i as i64,
                alive: false,
            });
        }
        app
    }

    /// Pills are 3-row cells on a 2-row stride, so a pill's bottom pad is
    /// normally the next pill's top pad and clicks there select the lower
    /// one. The root checkout sits over a quiet row and the last pill
    /// over nothing: their bottom pads — the lower half of the pill as
    /// drawn — must still hit the pill, not the panel background.
    #[test]
    fn worktree_pills_are_clickable_over_their_whole_height() {
        let mut app = hit_test_app(&["main", "feature", "other"], &[], &[]);
        let area = Rect::new(0, 0, 30, 20);
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(30, 20)).unwrap();
        terminal
            .draw(|f| draw_worktrees(f, &mut app, area))
            .unwrap();

        // `draw_column` hands the list rows from y=3: root at 3..=5, then a
        // quiet row, `feature` at 6..=8 sharing its bottom pad with
        // `other` at 8..=10.
        let at = |y: u16| app.hit_at(1, y);
        for y in 3..=5 {
            assert_eq!(at(y), Some(HitTarget::Worktree(0)), "root row y={y}");
        }
        assert_eq!(at(6), Some(HitTarget::Worktree(1)));
        assert_eq!(at(7), Some(HitTarget::Worktree(1)));
        assert_eq!(
            at(8),
            Some(HitTarget::Worktree(2)),
            "shared pad goes to the lower pill"
        );
        assert_eq!(at(9), Some(HitTarget::Worktree(2)));
        assert_eq!(
            at(10),
            Some(HitTarget::Worktree(2)),
            "last pill keeps its bottom pad"
        );
        assert_eq!(at(11), Some(HitTarget::PanelBg(Focus::Worktrees)));
    }

    /// The same rule in the Sessions panel: the last pill of a group has
    /// a header under it instead of another pill, and keeps its bottom pad.
    #[test]
    fn session_pills_are_clickable_over_their_whole_height() {
        let mut app = hit_test_app(&["main"], &["agent"], &["shell"]);
        let area = Rect::new(0, 0, 30, 20);
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(30, 20)).unwrap();
        terminal.draw(|f| draw_sessions(f, &mut app, area)).unwrap();

        // The agent has no header: its pill at 3..=5 (the "blank" row above
        // the next header is that pill's bottom pad), TERMINALS header at
        // 6, the terminal's pill at 7..=9.
        let at = |y: u16| app.hit_at(1, y);
        for y in 3..=5 {
            assert_eq!(at(y), Some(HitTarget::Session(0)), "agent row y={y}");
        }
        assert_eq!(at(6), Some(HitTarget::PanelBg(Focus::Sessions)), "header");
        for y in 7..=9 {
            assert_eq!(at(y), Some(HitTarget::Session(1)), "terminal row y={y}");
        }
        assert_eq!(at(10), Some(HitTarget::PanelBg(Focus::Sessions)));
    }
}
