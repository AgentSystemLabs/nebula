//! View layer: draws the LAUNCHER VIEW's grid + terminal pane + footer,
//! and records hit regions for mouse interaction.

use crate::app::{App, ConnState, Focus, HitTarget, Overlay, PaletteTarget, PromptDialog};
use crate::git_diff::{classify_diff_line, DiffLineKind};
use crate::keymap::Action;
use crate::text_input::{TextInput, TextView};
use crate::theme::Theme;
use nebula_core::{AgentStatus, SessionRef};
use ratatui::layout::{Constraint, Layout, Position, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Paragraph};
use ratatui::Frame;

mod launcher_view;

/// Outer size of the editor modal, as (width, height) percent of the frame.
/// Shared with the event loop's pre-draw PTY size guess.
pub const VIM_MODAL_PCT: (u16, u16) = (94, 92);
/// Outer size of the two split modals (diff, tree), percent of the frame.
pub(crate) const SPLIT_MODAL_PCT: (u16, u16) = (92, 90);
/// Outer size of the find-in-files modal, percent of the frame.
const GREP_MODAL_PCT: (u16, u16) = (88, 76);
/// Fixed (width, height) of the jump palette — tall enough for a screenful
/// of recent sessions.
const PALETTE_SIZE: (u16, u16) = (64, 22);
/// Fixed (width, height) of the find-file modal.
const FILES_SIZE: (u16, u16) = (72, 20);
/// Fixed (width, height) of the multi-line task prompt. 80 wide so the
/// QUICK PROMPT's full hint — `⇧Enter newline` spelled out — fits its
/// border.
const TASK_PROMPT_SIZE: (u16, u16) = (80, 14);
/// The FOLLOW-UP MODAL's size — the LAUNCHER VIEW's next-turn box. Short
/// and wide: four rows of typing, the same a card's own composer holds
/// ([`FOLLOW_UP_MAX_LINES`]), since both are a turn's worth of instruction
/// to a session already running.
const FOLLOW_UP_PROMPT_SIZE: (u16, u16) = (76, 9);

/// The key hints on a task box's bottom border, widest that fits inside
/// `width` (the block's, so two columns go to its edges). The QUICK PROMPT
/// has three more keys to advertise than the cloud and preset boxes —
/// `Tab` retargets the harness, `⇧Tab` picks an AGENT PRESET, `^N` flips
/// the launch into a fresh worktree — and a hint wider than the border is
/// silently chopped, hence the tiers and the test that measures them.
/// Its line break is advertised as `⇧Enter`, the chord a web form uses;
/// `^J` still breaks the line, unadvertised, for the terminal that flattens
/// a shifted Enter (tmux, Terminal.app).
fn task_prompt_hint(kind: &crate::app::PromptKind, width: u16) -> &'static str {
    if matches!(kind, crate::app::PromptKind::QuickPrompt(_)) {
        return if width >= 79 {
            " Enter launch · ⇧Enter newline · Tab agent · ⇧Tab preset · ^N worktree · Esc "
        } else if width >= 65 {
            " Enter launch · ⇧Enter newline · Tab agent · ⇧Tab preset · Esc "
        } else if width >= 57 {
            " ↵ launch · ⇧↵ newline · Tab agent · ⇧Tab preset · Esc "
        } else if width >= 44 {
            " ↵ launch · Tab agent · ⇧Tab preset · Esc "
        } else if width >= 22 {
            " Esc · ⇧↵ · Tab · ↵ "
        } else {
            " Esc · ⇧↵ · ↵ "
        };
    }
    // The FOLLOW-UP MODAL sends a turn to a session already running: no
    // picker, and Enter sends rather than launching anything.
    if matches!(kind, crate::app::PromptKind::FollowUp { .. }) {
        return if width >= 55 {
            " Enter: send · Shift+Enter/^J: newline · Esc: cancel "
        } else if width >= 40 {
            " Enter send · ^J newline · Esc cancel "
        } else {
            " Esc · ^J · Enter "
        };
    }
    // A comment posts rather than launches, and Esc goes back to the
    // ISSUES MODAL rather than cancelling into the panels.
    if matches!(kind, crate::app::PromptKind::IssueComment { .. }) {
        return if width >= 53 {
            " Enter: post · Shift+Enter/^J: newline · Esc: back "
        } else if width >= 38 {
            " Enter post · ^J newline · Esc back "
        } else {
            " Esc · ^J · Enter "
        };
    }
    // The COMMENT BOX posts rather than launches, and has no picker.
    if matches!(kind, crate::app::PromptKind::PrComment { .. }) {
        return if width >= 55 {
            " Enter: post · Shift+Enter/^J: newline · Esc: cancel "
        } else if width >= 40 {
            " Enter post · ^J newline · Esc cancel "
        } else {
            " Esc · ^J · Enter "
        };
    }
    if width >= 57 {
        " Enter: launch · Shift+Enter/^J: newline · Esc: cancel "
    } else if width >= 42 {
        // Not 36: at 36–41 columns this line was two characters wider than
        // the border and lost its tail.
        " Enter launch · ^J newline · Esc cancel "
    } else {
        " Esc · ^J · Enter "
    }
}

/// The QUICK PROMPT's target row, the first inside its frame. Off — the
/// launch lands in the selected checkout — it is a quiet `worktree: feat`
/// with the `[ ] new worktree ^N` toggle pinned right. On, it is the
/// loudest row in the box: a filled NEW WORKTREE chip, the branch Enter
/// will cut beside it in bold, and the toggle ticked, all in the green
/// the frame has turned. A PR SESSION's box names the pull request and
/// its head branch instead — the checkout the DAEMON reuses or cuts —
/// with no toggle, there being nothing to flip. The right half is dropped
/// whole before the left is cut short.
fn quick_target_line(
    app: &App,
    launch: &crate::quick_prompt::QuickLaunch,
    width: u16,
    th: Theme,
) -> Line<'static> {
    let branch = match &launch.pr {
        Some(pr) => pr.head.clone(),
        None => crate::quick_prompt::target_branch(app, launch)
            .unwrap_or_else(|| "(worktree gone)".into()),
    };
    let (left, right) = if let Some(pr) = &launch.pr {
        let dim = Style::default().fg(th.dim);
        (
            vec![
                Span::styled(format!("PR #{} · worktree: ", pr.number), dim),
                Span::styled(branch, Style::default().fg(th.muted)),
            ],
            vec![Span::styled("reused or cut on Enter ", dim)],
        )
    } else if launch.is_new_worktree() {
        let chip = Style::default()
            .fg(th.on_accent)
            .bg(th.ok)
            .add_modifier(Modifier::BOLD);
        let on = Style::default().fg(th.ok).add_modifier(Modifier::BOLD);
        (
            vec![
                Span::styled(" NEW WORKTREE ", chip),
                Span::styled(format!(" {branch}"), on),
            ],
            vec![
                Span::styled("[✓] new worktree", on),
                Span::styled(" ^N ", Style::default().fg(th.dim)),
            ],
        )
    } else {
        let dim = Style::default().fg(th.dim);
        (
            vec![
                Span::styled("worktree: ", dim),
                Span::styled(branch, Style::default().fg(th.muted)),
            ],
            vec![
                Span::styled("[ ] new worktree", dim),
                Span::styled(" ^N ", dim),
            ],
        )
    };
    let left_w: usize = left.iter().map(|s| s.width()).sum();
    let right_w: usize = right.iter().map(|s| s.width()).sum();
    let mut spans = left;
    if usize::from(width) > left_w + right_w {
        spans.push(Span::raw(" ".repeat(usize::from(width) - left_w - right_w)));
        spans.extend(right);
    }
    Line::from(spans)
}
/// Width of a one-line prompt, and of the wider one carrying a directory
/// listing under its input.
const PROMPT_W: u16 = 56;
const PATH_PROMPT_W: u16 = 72;
/// Narrowest a confirm dialog gets, so a short question still reads as one.
const CONFIRM_MIN_W: u16 = 52;
/// Widths of the modals whose height follows their content.
const HELP_W: u16 = 92;
/// The help overlay's key column: chords past it are dropped whole.
const HELP_KEY_W: usize = 14;
const SETTINGS_W: u16 = 84;
const MEMORY_W: u16 = 74;
const HOSTS_W: u16 = 64;
/// Layout floor for a split modal's right pane. Deliberately below
/// `MIN_DIFF_PANE_W`: the file list is clamped to keep that minimum first,
/// so on a tiny screen this lets the layout squeeze the diff/preview pane
/// rather than the list.
pub(crate) const SPLIT_PANE_LAYOUT_MIN: u16 = 20;
/// What every filtered list says when nothing survives the filter.
pub(crate) const NO_MATCHES: &str = "no matches";

/// Columns the tree-browser preview must keep for the file text itself
/// before a line-number gutter is worth drawing.
const MIN_PREVIEW_TEXT_W: usize = 16;

pub fn draw(f: &mut Frame, app: &mut App) {
    // A frame asks where the cursor is a dozen times over and moves it
    // none of them: the ROWS MEMO works it out once.
    app.rows_memo.arm();
    draw_screen(f, app);
    app.rows_memo.disarm();
    if app.black_background {
        let area = f.area();
        draw_black_background(f.buffer_mut(), area);
    }
}

fn draw_screen(f: &mut Frame, app: &mut App) {
    app.hits.clear();
    app.tail_cards.clear();
    app.host_cursor = None;
    app.welcome_on_screen = false;

    // The bar gets a blank row above it so it breathes off the panel
    // borders, matching the terminal's own padding below the last row.
    let [body, footer] =
        Layout::vertical([Constraint::Min(3), Constraint::Length(2)]).areas(f.area());

    if app.collapsed {
        draw_terminal(f, app, body);
        if app.focus == Focus::Terminal {
            draw_focus_tint(f.buffer_mut(), body, app.theme);
        }
        draw_footer(f, app, footer);
        draw_overlay(f, app);
        draw_vim(f, app);
        return;
    }

    // The LAUNCHER VIEW, which is the whole body: the PROJECT TABS over a
    // grid of the lit project's session cards, with the session under the
    // cursor live in the pane along the bottom, so walking the grid swaps
    // what the pane reads. With no card under the cursor there is no
    // pane: the grid takes the whole body until a card is clicked or
    // walked onto. A body too short for both is all grid.
    //
    // Any project on the machine puts it up; with none, the splash below
    // is the first run's "open a project".
    if app.launcher_active() {
        // `launcher_view::draw` takes `body_area` for the grid's half, so
        // the whole body is kept here for the pane drag to measure against.
        app.launcher_body = body;
        let (view_a, pane_a) = app.launcher_split(body);
        let side = app.launcher_pane_side();
        // The pane's edge facing the cards is draggable, as the panels'
        // boundaries are: its opening row (the rule) — or, with the pane
        // beside the cards, column — and the grid's one next to it are the
        // grab zone (`launcher::pane_grab_zone`), registered first so they win
        // `hit_at`'s first-match scan against a card that lands there.
        if let Some(pane_a) = pane_a {
            app.hits.push((
                crate::launcher::pane_grab_zone(side, pane_a),
                HitTarget::LauncherPaneSplitter,
            ));
        }
        launcher_view::draw(f, app, view_a);
        if let Some(pane_a) = pane_a {
            draw_terminal(f, app, crate::launcher::pane_content(side, pane_a));
            if app.focus == Focus::Terminal {
                draw_focus_tint(f.buffer_mut(), pane_a, app.theme);
            }
            draw_launcher_pane_grip(f.buffer_mut(), app, side, pane_a);
        }
        draw_footer(f, app, footer);
        draw_overlay(f, app);
        draw_vim(f, app);
        return;
    }

    // Nothing in the tree yet (first run): the animated nebula takes the
    // whole body until a project lands, which is what the view above
    // needs to draw at all.
    crate::splash::draw_splash(f, app, body);
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
        let pane = match &app.overlay {
            Some(Overlay::Tree(view)) => Some(view.preview_area),
            Some(Overlay::FileTabs(view)) => Some(view.body_area),
            _ => None,
        };
        if let Some(inner) = pane {
            if inner.width < 2 || inner.height < 2 {
                return; // pane not drawn yet
            }
            f.render_widget(
                tui_term::widget::PseudoTerminal::new(vim.parser.screen()),
                inner,
            );
            // Every key goes to the editor while it is up, so the host
            // cursor follows it rather than the pane underneath.
            app.host_cursor = pty_cursor_cell(vim.parser.screen(), inner);
            // Write-back: the post-draw sync resizes the PTY to the pane.
            if let Some(vim) = &mut app.vim {
                vim.area = inner;
            }
            return;
        }
        // The owning overlay gone under an embedded editor — fall through
        // to the modal so the session is never invisible.
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
    app.host_cursor = pty_cursor_cell(vim.parser.screen(), inner);
    // Write-back: the post-draw sync resizes the PTY to the drawn rect.
    if let Some(vim) = &mut app.vim {
        vim.area = inner;
    }
}

/// How much of the box a modal floating over it leaves showing on every
/// side: two rows and two columns, enough for the frame, the title and
/// the details row above whatever is drawn over them.
const OVER_BOX_INSET: u16 = 4;

/// The rect a multi-row task box is drawn in — one place, so a modal that
/// floats over the box ([`over_box_rect`]) can ask where the box is
/// before the box is drawn.
fn multiline_prompt_rect(frame: Rect, prompt: &PromptDialog) -> Rect {
    let quick = matches!(prompt.kind, crate::app::PromptKind::QuickPrompt(_));
    if quick {
        launcher_view::box_rect(frame)
    } else if matches!(prompt.kind, crate::app::PromptKind::FollowUp { .. }) {
        // Smaller than the task boxes: a follow-up is a sentence to a
        // session that is already running, and a box this size leaves the
        // grid it floats over readable around it — which card is being
        // prompted is read off the cards, not off the box.
        centered_rect(frame, FOLLOW_UP_PROMPT_SIZE.0, FOLLOW_UP_PROMPT_SIZE.1)
    } else {
        centered_rect(
            frame,
            TASK_PROMPT_SIZE.0,
            TASK_PROMPT_SIZE.1 + u16::from(quick),
        )
    }
}

/// Where a modal that floats over the box goes: inset inside the box's
/// rect when it fits there, so the box's frame, its title and its details
/// row stay on screen around it. A modal too big for that is centered on
/// the screen as it always was — over the box still, just not inside it.
pub(crate) fn over_box_rect(frame: Rect, over: Option<Rect>, width: u16, height: u16) -> Rect {
    match over {
        Some(b) if width + OVER_BOX_INSET <= b.width && height + OVER_BOX_INSET <= b.height => {
            centered_rect(b, width, height)
        }
        _ => centered_rect(frame, width, height),
    }
}

/// The footer bar's keys while `menu` is up, for the menus that have keys
/// of their own beyond Enter and Esc: the session pickers (the `?` jump
/// to the hovered harness's Agents section, and `Tab` on a Claude row
/// that can go to the cloud, with the state it would flip) and their
/// type-ahead MODEL / EFFORT submenus. None for a plain context menu,
/// which keeps the generic `Esc: close  Enter: confirm`. The keys live
/// down here, not in the modal's bottom border, so the modal stays as
/// narrow as its rows.
pub(crate) fn menu_footer_hint(menu: &crate::app::ContextMenu) -> Option<String> {
    let agent_jump = menu.hovered_agent_kind().is_some();
    if menu.filter.is_some() {
        return Some(
            if agent_jump {
                "type to filter  ↑/↓: move  Backspace: widen  ?: settings  Enter: pick  Esc: back"
            } else {
                "type to filter  ↑/↓: move  Backspace: widen  Enter: pick  Esc: back"
            }
            .to_string(),
        );
    }
    let cloud = menu.hovered_claude_cloud().map(|on| {
        if on {
            "Tab: cloud on  "
        } else {
            "Tab: cloud off  "
        }
    });
    if cloud.is_none() && !agent_jump {
        return None;
    }
    // A picker opened from the QUICK PROMPT is owed its box back.
    let esc = if crate::event_loop::menu_quick_return(menu).is_some_and(|back| back.from_box) {
        "Esc: back to the box"
    } else {
        "Esc: close"
    };
    Some(format!(
        "{}s/?: settings  Enter: pick  {esc}",
        cloud.unwrap_or("")
    ))
}

/// The box a menu floats over: the QUICK PROMPT its rows owe back, drawn
/// under it — returned as its rect, for [`over_box_rect`], and where its
/// branch landed this frame, for the WORKTREE PICKER that hangs from it
/// (empty when the details row had no room for it). A menu with no box
/// behind it — a context menu, a picker reached from a PR or an issue row
/// with no box up — draws nothing and floats where it always did.
fn draw_menu_backdrop(
    f: &mut Frame,
    app: &mut App,
    menu: &crate::app::ContextMenu,
) -> Option<(Rect, Rect)> {
    let back = crate::event_loop::menu_quick_return(menu).filter(|back| back.from_box)?;
    let box_behind = crate::quick_prompt::backdrop_box(&back);
    let rect = multiline_prompt_rect(f.area(), &box_behind);
    let branch = draw_multiline_prompt(f, app, &box_behind, true);
    Some((rect, branch))
}

/// A multi-row task box — the QUICK PROMPT and its siblings — drawn
/// into `f`. `backdrop` draws it as the layer *under* something else:
/// the PROJECT PICKER floats over the box `^P` was pressed in, so the
/// box is still on screen, dimmed, while you aim it somewhere. A
/// backdrop records no click areas and no field view — the overlay
/// drawn over it owns both — but still hands back where its branch was
/// drawn (empty when it was not), which a picker over it hangs from.
fn draw_multiline_prompt(
    f: &mut Frame,
    app: &mut App,
    prompt: &PromptDialog,
    backdrop: bool,
) -> Rect {
    let th = app.theme;
    // The QUICK PROMPT carries one row the other task boxes do not — where
    // the launch lands — and takes it in height rather than out of the
    // editor. Its frame turns green while Enter will cut a fresh worktree
    // first, so the state reads from across the room, before the row or the
    // title does.
    let quick = match &prompt.kind {
        crate::app::PromptKind::QuickPrompt(launch) => Some(launch),
        _ => None,
    };
    let new_worktree = quick.is_some_and(|launch| launch.is_new_worktree());
    // A box under a picker reads as the layer under it: a dim frame
    // and a dim caret, so the thing in front has the eye.
    let frame = if backdrop {
        th.dim
    } else if new_worktree {
        th.ok
    } else {
        th.accent
    };
    // The LAUNCHER VIEW's box is its front door: bigger, and with the
    // project on its target row and `^P` / `^O` in its hints.
    let launcher = quick.is_some();
    let area = multiline_prompt_rect(f.area(), prompt);
    f.render_widget(Clear, area);
    // A backdrop's border says nothing: Enter and Esc belong to whatever
    // is drawn over it, and naming the box's own keys there would be a
    // lie about which press does what.
    let hint = if backdrop {
        ""
    } else if launcher {
        launcher_view::box_hint(area.width)
    } else {
        task_prompt_hint(&prompt.kind, area.width)
    };
    let title = match quick {
        Some(launch) if launcher => launcher_view::box_title(launch),
        _ => prompt.title.clone(),
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(frame))
        .title(Span::styled(
            format!(" {title} "),
            Style::default().fg(frame),
        ))
        .title_bottom(Line::from(Span::styled(hint, Style::default().fg(th.dim))));
    let inner = block.inner(area);
    f.render_widget(block, area);
    // The view's box gets a column of air inside its border as well, so
    // nothing in it reads as hung off the frame.
    let inner = if launcher && inner.width >= 6 {
        Rect {
            x: inner.x + 1,
            width: inner.width - 2,
            ..inner
        }
    } else {
        inner
    };

    let label = prompt.label.clone();
    // The view's box leads with the row of details — the project, the
    // checkout, the harness and its model — then a blank row, then the
    // prompt header: the question, and the toggle that cuts a fresh
    // worktree. The blank row is the point: without it the details read as
    // part of the question under them.
    let mut toggle_area = Rect::default();
    let mut detail_areas: Vec<(crate::launcher::BoxField, Rect)> = Vec::new();
    let mut branch_area = Rect::default();
    let head_rows = match quick.filter(|_| launcher && inner.height >= 5) {
        Some(launch) => {
            let row = row_rect(inner, 0).expect("a five-row inner area has row 0");
            let details = launcher_view::detail_line(app, launch, row.width, th);
            f.render_widget(details.line, row);
            // Each detail is a button: the columns it was drawn in, in
            // screen coordinates, so a click there opens its own picker.
            let cols = |row: Rect, (x, width): (u16, u16)| Rect {
                x: row.x + x,
                width,
                ..row
            };
            detail_areas = details
                .fields
                .into_iter()
                .map(|(field, x, width)| (field, cols(row, (x, width))))
                .collect();
            branch_area = details.branch.map(|at| cols(row, at)).unwrap_or_default();
            let row = row_rect(inner, 2).expect("a five-row inner area has row 2");
            let header = launcher_view::target_line(launch, &label, row.width, th);
            f.render_widget(header.line, row);
            toggle_area = header.toggle.map(|at| cols(row, at)).unwrap_or_default();
            3
        }
        None => {
            // The QUICK PROMPT's target row above the label: the toggle
            // and its state in one glance, whichever way it stands.
            let target_rows = u16::from(quick.is_some() && inner.height >= 5);
            if let (1, Some(launch)) = (target_rows, quick) {
                let row = row_rect(inner, 0).expect("a five-row inner area has a target row");
                f.render_widget(quick_target_line(app, launch, row.width, th), row);
            }
            let label_rows = u16::from(inner.height.saturating_sub(target_rows) >= 4);
            if label_rows == 1 {
                let row = row_rect(inner, usize::from(target_rows))
                    .expect("a four-row inner area has a label row");
                f.render_widget(
                    Paragraph::new(Span::styled(label, Style::default().fg(th.dim))),
                    row,
                );
            }
            target_rows + label_rows
        }
    };

    // A bordered, multi-row task editor. Its own wrapping helper keeps
    // words intact and follows the caret once the task grows beyond the
    // visible rows.
    let editor_area = Rect {
        x: inner.x,
        y: inner.y.saturating_add(head_rows),
        width: inner.width,
        height: inner.height.saturating_sub(head_rows),
    };
    let editor_inner = if editor_area.height >= 3 && editor_area.width >= 4 {
        let editor_block = Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(th.dim));
        let editor_inner = editor_block.inner(editor_area);
        f.render_widget(editor_block, editor_area);
        // The view's box keeps a column of air inside this border too, so
        // the task never starts hard against it.
        if launcher && editor_inner.width >= 4 {
            Rect {
                x: editor_inner.x + 1,
                width: editor_inner.width - 2,
                ..editor_inner
            }
        } else {
            editor_inner
        }
    } else {
        editor_area
    };
    let caret = if backdrop { th.dim } else { th.accent };
    let (view, rows) = draw_multiline_input_with_caret(f, &prompt.input, editor_inner, th, caret);
    if editor_inner != editor_area {
        draw_scroll_marks(f, editor_area, view, rows, th.dim);
    }
    // Record the drawn areas for click hit-testing, and the view the keys,
    // wheel and clicks walk the rows by. A backdrop records none of it: the
    // overlay that is up is the one drawn over it.
    if backdrop {
        return branch_area;
    }
    if let Some(Overlay::Prompt(p)) = &mut app.overlay {
        p.area = area;
        p.editor_area = editor_inner;
        p.toggle_area = toggle_area;
        p.detail_areas = detail_areas;
        p.branch_area = branch_area;
        p.input.set_view(view);
    }
    branch_area
}

fn draw_overlay(f: &mut Frame, app: &mut App) {
    let th = app.theme;
    let Some(overlay) = app.overlay.clone() else {
        return;
    };
    // A box opened from the ISSUES MODAL or the PULL REQUESTS MODAL stands
    // on it rather than taking it away: the modal is the bottom layer, the
    // box — and any picker the box has up — is drawn over it.
    use crate::quick_prompt::ModalUnder;
    match crate::quick_prompt::modal_under(&overlay) {
        Some(ModalUnder::Issues(view)) => crate::issues::draw(f, app, &view, th, true),
        Some(ModalUnder::PullRequests(view)) => crate::pr_modal::draw(f, app, &view, th, true),
        None => {}
    }
    match overlay {
        Overlay::ProjectPicker(picker) => {
            // `^P` layers the project list over the box rather than
            // taking the box away: the task you typed is still in front
            // of you while you pick where it lands.
            if launcher_view::picker_over_box(app, &picker) {
                let box_behind = crate::quick_prompt::backdrop_box(&picker.back);
                draw_multiline_prompt(f, app, &box_behind, true);
            }
            launcher_view::draw_project_picker(f, app, &picker)
        }
        Overlay::Menu(menu) => {
            // `Tab` (the harness) and `^O` (the model) layer their list
            // over the box rather than taking the box away, as `^P` does:
            // the task you typed is still in front of you while you pick
            // what will run it.
            let backdrop = draw_menu_backdrop(f, app, &menu);
            let over = backdrop.map(|(rect, _)| rect);
            // The WORKTREE PICKER hangs from the branch it was opened on,
            // wherever the box has it this frame — a resize moves both —
            // its rows' text in the branch's column (a border and a space
            // in). Centered over the box when the details row found no
            // room for the branch.
            let at = menu.at.or_else(|| {
                let (_, branch) = backdrop.filter(|_| menu.is_launch_worktree_picker())?;
                (branch.width > 0).then(|| (branch.x.saturating_sub(2), branch.y + 1))
            });
            // A type-ahead submenu shows its query in the title: `Cursor
            // model ⌕ opus`, the bare ⌕ while nothing is typed yet.
            let title_text = menu.title.as_deref().map(|t| match &menu.filter {
                Some(f) if !f.query.is_empty() => format!("{t} ⌕ {}", f.query),
                Some(_) => format!("{t} ⌕"),
                None => t.to_string(),
            });
            let title_width = title_text
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
            // The modal is as wide as its rows or its title, whichever is
            // longer, and no wider: its keys go in the footer bar (see
            // `menu_footer_hint`), not in the bottom border, so a hint
            // that outgrows the rows — the pickers' `Tab: cloud off  s/?:
            // settings` did, doubling the width of a six-row list — never
            // pads the modal with empty space, and hovering a row with more
            // keys (the Claude row's Tab) never resizes it.
            let width = (label_w + 4 + if any_submenu { 2 } else { 0 })
                .max(title_width + 2)
                .min(f.area().width as usize) as u16;
            let height = menu.items.len() as u16 + 2;
            let area = match at {
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
                None => over_box_rect(f.area(), over, width, height),
            };
            f.render_widget(Clear, area);
            let mut block = Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .border_style(Style::default().fg(th.accent));
            if let Some(title) = &title_text {
                block = block.title(Span::styled(
                    format!(" {title} "),
                    Style::default().fg(th.accent).add_modifier(Modifier::BOLD),
                ));
            }
            let inner = block.inner(area);
            f.render_widget(block, area);
            // Every row spans the modal — the hovered row's bar reaches
            // the border, and the ▸ sits at the right edge — so a title
            // wider than the rows leaves no ragged gap beside them.
            let row_w = inner.width as usize;
            let label_w = row_w.saturating_sub(if any_submenu { 4 } else { 2 });
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
                    format!(" {:<label_w$} ", item.label)
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
            draw_multiline_prompt(f, app, &prompt, false);
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
                        (
                            Act(&[NextAttention, PrevAttention]),
                            "next/prev session needing you",
                        ),
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
                        (Act(&[Rename]), "run / stop the project's run command"),
                        (Act(&[OpenWorktree]), "fire its open command"),
                        (Act(&[HalfPageDown, HalfPageUp]), "half a panel down / up"),
                        (Act(&[GitDiff]), "git diff (^r: mark reviewed ✓, ^t: tree)"),
                        (
                            Act(&[OpenRepo, OpenGhosttyTab]),
                            "repo on GitHub / Ghostty tab",
                        ),
                        (
                            Act(&[OpenPullRequest, OpenIssue]),
                            "card's PR / issue on GitHub",
                        ),
                        (Act(&[RefreshPullRequests]), "refresh pull requests now"),
                        (
                            Act(&[CommentPullRequest]),
                            "comment on the pull request row",
                        ),
                        (Act(&[Issues]), "github issues: prompt / preset / edit one"),
                        (
                            Act(&[PullRequests]),
                            "pull requests: read one, launch a PR session on it",
                        ),
                        (Act(&[SwitchBranch]), "switch the ⌂ root checkout's branch"),
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
                        (
                            Act(&[DuplicateSession]),
                            "quick prompt on the card's settings",
                        ),
                        (Act(&[AgentPresets]), "agent presets: saved launches"),
                        (Act(&[NewTerminal]), "new shell terminal"),
                        (Act(&[Activate]), "attach session / open link"),
                        (Act(&[HalfPageDown, HalfPageUp]), "half a panel down / up"),
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
                        (Act(&[Activate]), "lock input"),
                        (Act(&[UnlockTerminal]), "unlock, back to panels"),
                        (Lit("drag"), "select + copy (2×click: word)"),
                        (Lit("click / drag"), "an app that took the mouse gets it"),
                        (Lit("⌥click"), "open URL / file under cursor"),
                        (Lit("⇧drag"), "select via your terminal"),
                        (Lit("drag the pane edge"), "resize the pane"),
                        (Lit("click outside"), "dismiss any modal (= Esc)"),
                    ],
                ),
                (
                    "GENERAL",
                    &[
                        (
                            Act(&[ToggleLauncherPane, ToggleSidebars]),
                            "fold the pane away / bring it back",
                        ),
                        (Lit("⌘1-9 / 1-9"), "open that project tab"),
                        (Act(&[Hosts]), "ssh hosts: connect (a: new, d: del)"),
                        (Act(&[Settings]), "settings (Hotkeys tab rebinds these)"),
                        (Act(&[Metrics]), "memory usage (nebula + agents)"),
                        (Act(&[Quit, Help]), "quit / toggle this help"),
                    ],
                ),
            ];
            // What to print in the key column: a literal, or every chord
            // each action currently answers to.
            // An action bound to more chords than the key column holds —
            // open's ⇧Enter ⇧O ⌥Enter — loses whole chords off the end
            // and gains an ellipsis, never a cut mid-chord; the Hotkeys
            // tab lists every one.
            let keys_of = |k: &HelpKeys| -> String {
                match k {
                    Lit(s) => (*s).to_string(),
                    Act(actions) => {
                        let full = actions
                            .iter()
                            .map(|a| app.keymap.label(*a))
                            .collect::<Vec<_>>()
                            .join(" / ");
                        if actions.len() != 1 || full.chars().count() <= HELP_KEY_W {
                            return full;
                        }
                        let chords: Vec<String> = app
                            .keymap
                            .chords(actions[0])
                            .iter()
                            .map(|c| c.display().to_string())
                            .collect();
                        (1..chords.len())
                            .rev()
                            .map(|kept| format!("{} …", chords[..kept].join(" ")))
                            .find(|shown| shown.chars().count() <= HELP_KEY_W)
                            .unwrap_or(full)
                    }
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
                        // column is padded to a fixed width and clipped
                        // there — an exotic binding can't shove the
                        // descriptions out of alignment.
                        let keys = truncate(&keys_of(k), HELP_KEY_W);
                        lines.push(Line::from(vec![
                            Span::styled(
                                format!(" {keys:<width$}", width = HELP_KEY_W),
                                Style::default().fg(th.accent),
                            ),
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
            // The Project tab's rows are the selected project's: its
            // name and path head the tab, and its entry is what the
            // values read. No project (an empty tree) leaves them n/a.
            let project = app
                .selected_project()
                .map(|p| (p.name.clone(), p.repo_path.clone()));
            let project_settings = project.as_ref().map(|(_, path)| cfg.project(path));
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
            let (strip, hits) = tab_strip(
                inner.x,
                crate::config::SETTINGS_TABS.iter().map(|t| t.title),
                tab,
                view.on_tabs,
                th,
            );
            let mut lines: Vec<Line> = vec![Line::from(strip), strip_rule(inner.width, th)];

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
                    crate::config::SettingsRow::Project => match &project {
                        Some((name, path)) => {
                            let name = format!(" {name}");
                            let room = (inner.width as usize).saturating_sub(name.chars().count());
                            lines.push(Line::from(vec![
                                Span::styled(
                                    name,
                                    Style::default().fg(th.muted).add_modifier(Modifier::BOLD),
                                ),
                                Span::styled(truncate(&format!("  {}", path.display()), room), dim),
                            ]));
                        }
                        None => lines.push(Line::from(Span::styled(
                            " no project selected — these rows are the selected project's",
                            Style::default().fg(th.warn),
                        ))),
                    },
                    crate::config::SettingsRow::Setting(i) => {
                        // The Agents tab resolves its harness rows through
                        // the registry; every other values tab reads its
                        // static spec, and a PROJECT TAB row reads the
                        // selected project's entry.
                        let (label, value) = if tab == crate::config::agents_tab() {
                            match crate::config::AGENTS_HEAD.get(*i) {
                                Some(spec) => (spec.label.to_string(), cfg.value_label(spec.kind)),
                                None => {
                                    let (id, field) = cfg.agent_row(*i).expect(
                                        "settings_rows indexes the Agents tab's harness rows",
                                    );
                                    (field.label().to_string(), cfg.agent_value(&id, field))
                                }
                            }
                        } else {
                            let spec = crate::config::setting_at(tab, *i)
                                .expect("settings_rows indexes this tab's settings");
                            let value = if spec.kind.is_project() {
                                project_settings
                                    .as_ref()
                                    .map(|s| s.value_label(spec.kind))
                                    .unwrap_or_else(|| "n/a".into())
                            } else {
                                cfg.value_label(spec.kind)
                            };
                            (spec.label.to_string(), value)
                        };
                        let selected = *i == view.selected && !view.on_tabs;
                        let mut label_style = Style::default();
                        let mut value_style = Style::default().fg(th.accent);
                        if selected {
                            label_style = label_style.bg(th.sel_bg).add_modifier(Modifier::BOLD);
                            value_style = value_style.bg(th.sel_bg).add_modifier(Modifier::BOLD);
                        }
                        // A typed command can outrun the column: clip it
                        // with an ellipsis rather than at the frame.
                        let room = (inner.width as usize).saturating_sub(3 + 28 + 2);
                        lines.push(Line::from(vec![
                            Span::styled(format!("   {:<28}", label), label_style),
                            Span::styled(format!("[{}]", truncate(&value, room)), value_style),
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

            // Left: changed-file list — flat paths, or the directory tree
            // (`Ctrl+t`); a stateless follow-window keeps the selected row
            // visible.
            let mut files_title = if view.listing.is_some() && view.files.is_empty() {
                "Files (…)".to_string()
            } else if view.filter.is_empty() {
                format!("Files ({})", view.files.len())
            } else {
                format!("Files ({}/{})", view.matches.len(), view.files.len())
            };
            if !view.reviewed.is_empty() {
                files_title.push_str(&format!(" · {}✓", view.reviewed.len()));
            }
            // The hint names the list `Ctrl+t` leads to, not the one up.
            let block = panel_block(&files_title, true, th).title_bottom(Line::from(Span::styled(
                if view.tree.is_some() {
                    " ^t: flat list "
                } else {
                    " ^t: tree "
                },
                Style::default().fg(th.dim),
            )));
            let files_inner = block.inner(files_a);
            f.render_widget(block, files_a);

            // First row: the always-on fuzzy filter input.
            if let Some(filter_area) = row_rect(files_inner, 0) {
                let line = search_line(&view.filter, "type to filter…", filter_area, th);
                f.render_widget(Paragraph::new(line), filter_area);
            }
            let list_inner = below_first_row(files_inner);

            if view.listing.is_some() && view.files.is_empty() {
                empty_list_row(f, list_inner, "reading changes…", th);
            } else if view.row_count() == 0 {
                empty_list_row(f, list_inner, NO_MATCHES, th);
            }
            let start = view.window_start(list_inner.height as usize);
            // Both lists open a row the same way: the status code, then the
            // ✓ — so the two columns read straight down whichever is up.
            let gutter = |file: Option<&crate::git_diff::DiffFile>, reviewed: bool| {
                let status = match file {
                    Some(file) => Span::styled(
                        format!("{} ", file.status_str()),
                        Style::default().fg(match (file.xy[0], file.xy[1]) {
                            ('?', '?') | ('A', _) => th.ok,
                            ('D', _) | (_, 'D') => th.err,
                            ('R', _) | ('C', _) => th.accent,
                            _ => th.warn,
                        }),
                    ),
                    None => Span::raw("   "),
                };
                let mark = if reviewed {
                    Span::styled("✓ ", Style::default().fg(th.ok))
                } else {
                    Span::raw("  ")
                };
                vec![status, mark]
            };
            match &view.tree {
                None => {
                    for (row, (i, m)) in view.matches.iter().enumerate().skip(start).enumerate() {
                        let Some(row_area) = row_rect(list_inner, row) else {
                            break;
                        };
                        let file = &view.files[m.file];
                        let budget = (list_inner.width as usize).saturating_sub(5);
                        let mut spans = gutter(Some(file), view.reviewed.contains_key(&file.path));
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
                }
                // The TREE BROWSER's rows behind the flat list's gutter: a
                // directory wears the fold marker and the accent, and its ✓
                // once every file under it has one.
                Some(tree) => {
                    let done = tree.reviewed_nodes(&view.files, &view.reviewed);
                    for (row, (i, r)) in tree.rows.iter().enumerate().skip(start).enumerate() {
                        let Some(row_area) = row_rect(list_inner, row) else {
                            break;
                        };
                        let node = &tree.nodes[r.node];
                        let file = tree.file_of[r.node].map(|f| &view.files[f]);
                        let indent = "  ".repeat(node.depth);
                        let marker = if !node.is_dir {
                            "  "
                        } else if tree.is_open(r.node, !view.filter.is_empty()) {
                            "▾ "
                        } else {
                            "▸ "
                        };
                        let budget = (list_inner.width as usize)
                            .saturating_sub(5 + indent.chars().count() + 2);
                        let shown = truncate(&node.name, budget);
                        let mut spans = gutter(file, done[r.node]);
                        spans.push(Span::raw(indent));
                        spans.push(Span::styled(marker, Style::default().fg(th.accent)));
                        if node.is_dir {
                            spans.push(Span::styled(shown, Style::default().fg(th.accent)));
                        } else {
                            let positions = visible_positions(&r.positions, &shown, &node.name);
                            spans.extend(fuzzy_highlight_spans(&shown, positions, th));
                        }
                        render_row(f, row_area, spans, i == tree.selected, true, th);
                    }
                }
            }

            // Right: the selected file's diff, scrolled — or, on a tree
            // directory's row, the list of what changed under it.
            let sel_path = match view.selected_dir() {
                Some(dir) => format!("{dir}/"),
                None => view.selected_path().unwrap_or("").to_string(),
            };
            let sel_reviewed = view.reviewed.contains_key(&sel_path);
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
            // Only the rows in view are styled: a diff runs to 20 000
            // lines, and building a `Line` for each of them on every frame
            // was most of what scrolling a large one cost.
            let lines: Vec<Line> = view
                .diff
                .lines()
                .skip(scroll as usize)
                .take(diff_inner.height as usize)
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
            f.render_widget(Paragraph::new(lines), diff_inner);

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
                format!(" Jump to ({}/{}) ", palette.hits(), palette.items.len())
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
                // running session reads as running here too. The row draws
                // the project it lives in dim, then its own name — a
                // project row in bold, a dim "23m ago" pinned right — so
                // the cyan-bold match highlight is the loudest thing in the
                // list, and a title sweeps exactly like its panel row.
                let (solid, hollow) = match &item.target {
                    PaletteTarget::Project(_) => ("▪ ", "▫ "),
                    PaletteTarget::Worktree(_) => ("▸ ", "▹ "),
                    PaletteTarget::Session(_) => ("● ", "○ "),
                    // The arrow its Worktrees-panel row wears (`pr_row`),
                    // since that row is where picking it lands.
                    PaletteTarget::PullRequest { .. } => ("↗ ", "↗ "),
                };
                let status = item.status;
                // A pull request carries no status; its colors are its
                // standing's, the look its Worktrees-panel row wears — the
                // accent for one ready for review, the dim end to end for
                // a draft, red for one GitHub says cannot merge — and a
                // trailing badge spells that state out in full (`draft`,
                // `ready for review`, or the trouble: `merge conflicts`,
                // `checks failing`), the sidebar's words at this modal's
                // width, so the rows are told apart before one is picked,
                // by the word and not only by the color.
                let pr = item
                    .standing
                    .map(|standing| (standing, crate::pr_row::look(standing, item.trouble, th)));
                let (glyph, glyph_color) = if let Some((_, look)) = pr {
                    (solid, look.glyph)
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
                let badge = pr.map(|(standing, look)| {
                    let word = item.trouble.map_or(standing.label(), |t| t.label());
                    (format!(" {word}"), look.badge)
                });
                // The label is the row's own name, with the project it
                // lives in drawn dim in front of it — `demo/fix-login`, one
                // line, no header above it. The rest of the searched path
                // (a session's branch) still narrows the list; it is simply
                // not drawn.
                let label: String = item.text.chars().skip(item.label_at).collect();
                let label_positions: Vec<usize> = m
                    .positions
                    .iter()
                    .filter_map(|p| p.checked_sub(item.label_at))
                    .collect();
                let (crumb, crumb_hits) = match item.crumb {
                    Some((at, end)) => (
                        format!(
                            "{}/",
                            item.text.chars().take(end).skip(at).collect::<String>()
                        ),
                        m.positions
                            .iter()
                            .filter(|p| (at..end).contains(p))
                            .map(|p| p - at)
                            .collect(),
                    ),
                    None => (String::new(), Vec::new()),
                };
                // Pinned right, dim: when the row last ran — its panel
                // row's "23m ago".
                let tail = if item.stamped > 0 {
                    crate::hosts::ago_label(crate::app::now_ms() - item.stamped)
                } else {
                    String::new()
                };
                let tail_w = tail.chars().count();
                // The badge is billed before the text, as `pr_row::spans`
                // does, so a long title shortens and the state never clips;
                // the tail too, with a two-column gap before it. The width
                // leaves the selection marker's column and a right margin.
                let badge_len = badge.as_ref().map_or(0, |(b, _)| b.chars().count());
                let width = (list_inner.width as usize).saturating_sub(2);
                // The crumb never eats the row: a long project name gets a
                // third of the width, the row's own name keeps the rest.
                let crumb_shown = truncate(&crumb, width / 3);
                let crumb_hits = visible_positions(&crumb_hits, &crumb_shown, &crumb);
                let lead = 2 + crumb_shown.chars().count();
                let budget = width
                    .saturating_sub(lead + badge_len)
                    .saturating_sub(if tail_w > 0 { tail_w + 2 } else { 0 });
                let shown = truncate(&label, budget);
                let positions = visible_positions(&label_positions, &shown, &label);
                let quiet = item.trouble.is_none()
                    && matches!(item.standing, Some(crate::pull_request::Standing::Draft));
                let mut text = label_highlight_spans(
                    &shown,
                    positions,
                    quiet,
                    // No ONE-SHOT SWEEP in a list the user just summoned:
                    // it is for the change nobody was looking at.
                    sweep_ramp(status, false, th, app.animations),
                    app.sweep_phase(),
                    // A pull request in trouble paints its title in its
                    // row's red — the end-to-end red its sidebar row wears.
                    pr.filter(|_| item.trouble.is_some())
                        .map_or(th.text, |(_, look)| look.label),
                    th,
                );
                if matches!(item.target, PaletteTarget::Project(_)) {
                    for s in &mut text {
                        s.style = s.style.add_modifier(Modifier::BOLD);
                    }
                }
                let mut spans = vec![Span::styled(glyph, Style::default().fg(glyph_color))];
                if !crumb_shown.is_empty() {
                    // Dim end to end, bar the chars the query hit: the crumb
                    // places the row, the name is what you are reading for.
                    spans.extend(label_highlight_spans(
                        &crumb_shown,
                        crumb_hits,
                        true,
                        None,
                        0,
                        th.dim,
                        th,
                    ));
                }
                spans.extend(text);
                if let Some((badge, color)) = badge {
                    spans.push(Span::styled(badge, Style::default().fg(color)));
                }
                let used = lead + shown.chars().count() + badge_len;
                if tail_w > 0 && used + tail_w < width {
                    spans.push(Span::raw(" ".repeat(width - used - tail_w)));
                    spans.push(Span::styled(tail, Style::default().fg(th.dim)));
                }
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
            // No count to show until the listing lands: `(0/0)` reads as
            // "no files", which is not what is known yet.
            let title = if finder.listing.is_some() {
                format!(" Find file — {} (listing…) ", finder.branch)
            } else if finder.query.is_empty() {
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

            if finder.listing.is_some() {
                empty_list_row(f, list_inner, "listing files…", th);
            } else if finder.matches.is_empty() {
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
            } else if view.waiting.is_some() {
                format!(" Find in files — {} (searching…) ", view.branch)
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
            } else if view.hits.is_empty() && view.waiting.is_none() {
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
        Overlay::Issues(view) => crate::issues::draw(f, app, &view, th, false),
        Overlay::PullRequests(view) => crate::pr_modal::draw(f, app, &view, th, false),
        Overlay::BranchSwitch(view) => crate::branch_switch::draw(f, app, &view, th),
        Overlay::FileTabs(mut view) => {
            // The TREE BROWSER's footprint: the editor Enter opens wants the
            // room, and the preview is a whole file.
            let area = centered_rect_pct(f.area(), SPLIT_MODAL_PCT.0, SPLIT_MODAL_PCT.1);
            let title = format!(" Open files ({}) ", view.tabs.len());
            let inner = render_modal_frame(f, area, title, th);

            // ---- tab strip and its rule ----
            let (strip, hits) = tab_strip(
                inner.x,
                view.tabs.iter().map(|t| t.label.as_str()),
                view.tab,
                view.on_tabs,
                th,
            );
            let head = Rect {
                height: inner.height.min(2),
                ..inner
            };
            f.render_widget(
                Paragraph::new(vec![Line::from(strip), strip_rule(inner.width, th)]),
                head,
            );

            // ---- body: the preview, or the embedded editor draw_vim paints
            // over it after us ----
            let body = Rect {
                x: inner.x,
                y: inner.y.saturating_add(2),
                width: inner.width,
                height: inner.height.saturating_sub(3),
            };
            let editing = app.vim.as_ref().is_some_and(|v| v.embedded);
            // A markdown tab shows the rendered page — flowed for this
            // width, kept on the view between draws — unless `m` asked
            // for the source. No gutter: rendered rows aren't source lines.
            let rendered = (view.renders_markdown() && !editing).then(|| {
                crate::markdown::Rendered::for_width(
                    view.rendered.take(),
                    &view.preview_text,
                    body.width,
                    crate::markdown::Breaks::Reflow,
                    th,
                )
            });
            let line_count = match &rendered {
                Some(r) => r.lines.len(),
                None => view.preview_lines.len(),
            };
            let max_scroll = line_count
                .saturating_sub(body.height as usize)
                .min(u16::MAX as usize) as u16;
            let scroll = view.scroll.min(max_scroll);
            if !editing && body.height > 0 {
                let lines = match &rendered {
                    Some(r) => r
                        .lines
                        .iter()
                        .skip(scroll as usize)
                        .take(body.height as usize)
                        .cloned()
                        .collect(),
                    None => preview_window(
                        &view.preview_lines,
                        line_count,
                        view.preview_is_file,
                        scroll,
                        body,
                        th,
                    ),
                };
                f.render_widget(Paragraph::new(lines), body);
            }

            // ---- keys hint on the last row ----
            if let Some(hint_area) = row_rect(inner, inner.height.saturating_sub(1) as usize) {
                f.render_widget(
                    Paragraph::new(Span::styled(
                        truncate(
                            &format!(" {}", file_tabs_keys_hint(&view, editing)),
                            inner.width as usize,
                        ),
                        Style::default().fg(th.dim),
                    )),
                    hint_area,
                );
            }

            // Write-back (draw works on a clone): hit rects for the mouse,
            // the pane for the embedded editor, the page size for paging,
            // the scroll re-clamped so resizes never strand the view, the
            // shown line count and the flowed page for the next draw.
            if let Some(Overlay::FileTabs(v)) = &mut app.overlay {
                v.area = area;
                v.tab_hits = hits;
                v.body_area = body;
                v.view_height = body.height;
                v.scroll = scroll;
                v.preview_line_count = line_count;
                if rendered.is_some() {
                    v.rendered = rendered;
                }
            }
        }
        Overlay::Tree(mut view) => {
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
            let tree_title = if view.listing.is_some() {
                format!("Tree — {} (listing…)", view.branch)
            } else if view.filter.is_empty() {
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

            if view.listing.is_some() {
                empty_list_row(f, list_inner, "listing files…", th);
            } else if view.rows.is_empty() {
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
            // A markdown file shows the rendered page (the FILE TABS'
            // rule), flowed for this width and kept between draws, unless
            // Ctrl+r asked for the source.
            let rendered = (view.renders_markdown() && !editing).then(|| {
                crate::markdown::Rendered::for_width(
                    view.rendered.take(),
                    &view.preview,
                    preview_inner.width,
                    crate::markdown::Breaks::Reflow,
                    th,
                )
            });
            let line_count = match &rendered {
                Some(r) => r.lines.len(),
                None => view.preview_lines.len(),
            };
            let max_scroll = (line_count.min(u16::MAX as usize) as u16)
                .saturating_sub(preview_inner.height.max(1));
            let scroll = view.scroll.min(max_scroll);
            if !editing && max_scroll > 0 {
                block = block.title_bottom(
                    Line::from(Span::styled(
                        format!(" {}/{} ", scroll + 1, line_count),
                        Style::default().fg(th.dim),
                    ))
                    .right_aligned(),
                );
            }
            f.render_widget(block, preview_a);
            if !editing {
                // Line-number gutter, for real file contents only —
                // directory listings and placeholders have no lines to
                // number, and a rendered page's rows aren't source lines.
                // Dropped entirely when the pane is too narrow to leave
                // room for the code itself.
                let lines = match &rendered {
                    Some(r) => r
                        .lines
                        .iter()
                        .skip(scroll as usize)
                        .take(preview_inner.height as usize)
                        .cloned()
                        .collect(),
                    None => preview_window(
                        &view.preview_lines,
                        line_count,
                        view.preview_is_file,
                        scroll,
                        preview_inner,
                        th,
                    ),
                };
                f.render_widget(Paragraph::new(lines), preview_inner);
            }

            // Write-back (draw works on a clone): page size for key paging,
            // scroll re-clamped so resizes never strand the view, preview
            // rect for the embedded editor.
            if let Some(Overlay::Tree(v)) = &mut app.overlay {
                v.view_height = preview_inner.height;
                v.scroll = scroll;
                v.preview_line_count = line_count;
                if rendered.is_some() {
                    v.rendered = rendered;
                }
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
/// The FILE TABS' bottom-row hint: what the keys do from where the cursor
/// is — the strip, the preview, or the editor drawn over it.
fn file_tabs_keys_hint(view: &crate::file_tabs::FileTabsView, editing: bool) -> String {
    let editor = editor_name(&view.editor);
    let md = markdown_toggle_hint("m", view.markdown, view.pretty);
    if editing {
        format!(
            "{editor} has the keys  Ctrl+q: back to the tabs (kills {editor}; :q keeps the file)"
        )
    } else if view.on_tabs {
        format!(
            "←/→ or Tab: switch  1-9: jump  ↓: preview  Enter: edit in {editor}{md}  \
             Esc / Ctrl+q: close"
        )
    } else {
        format!(
            "j/k: scroll  Ctrl+d/u: half page  ↑ off the top: tabs  Enter: edit in {editor}{md}  \
             Esc / Ctrl+q: back to the tabs"
        )
    }
}

/// The hint segment for a markdown preview's toggle, naming the view the
/// key switches *to*; nothing at all for a file that isn't markdown.
fn markdown_toggle_hint(key: &str, markdown: bool, pretty: bool) -> String {
    match (markdown, pretty) {
        (true, true) => format!("  {key}: source"),
        (true, false) => format!("  {key}: rendered"),
        (false, _) => String::new(),
    }
}

/// The tab strip the SETTINGS OVERLAY and the FILE TABS share: labels laid
/// out left to right from `x`, the active one lit — and reversed while the
/// cursor is parked on the strip, so ←/→ visibly belong to it — returning
/// the spans and each label's screen x-range for click hit-testing.
fn tab_strip<'a>(
    x: u16,
    labels: impl Iterator<Item = &'a str>,
    active: usize,
    on_tabs: bool,
    th: Theme,
) -> (Vec<Span<'static>>, Vec<(u16, u16)>) {
    let mut strip: Vec<Span> = Vec::new();
    let mut hits: Vec<(u16, u16)> = Vec::new();
    let mut x = x;
    for (i, t) in labels.enumerate() {
        strip.push(Span::raw(" "));
        x += 1;
        let label = format!(" {t} ");
        let mut style = Style::default().fg(th.dim);
        if i == active {
            style = Style::default()
                .fg(th.accent)
                .bg(th.sel_bg)
                .add_modifier(Modifier::BOLD);
            if on_tabs {
                style = style.add_modifier(Modifier::REVERSED);
            }
        }
        let w = label.chars().count() as u16;
        hits.push((x, x + w));
        x += w;
        strip.push(Span::styled(label, style));
    }
    (strip, hits)
}

/// The rule under a tab strip, the modal's inner width.
fn strip_rule(width: u16, th: Theme) -> Line<'static> {
    Line::from(Span::styled(
        "─".repeat(width as usize),
        Style::default().fg(th.muted),
    ))
}

/// The highlighted preview the TREE BROWSER and the FILE TABS draw: the
/// visible window of `lines` from `scroll`, with a line-number gutter when
/// the text is a real file and the pane is wide enough to spare it and
/// still leave room for the code itself.
fn preview_window(
    lines: &[Vec<(crate::syntax::TokenKind, String)>],
    line_count: usize,
    is_file: bool,
    scroll: u16,
    inner: Rect,
    th: Theme,
) -> Vec<Line<'static>> {
    let num_w = line_count.to_string().len().max(2);
    let gutter = is_file && (inner.width as usize) > num_w + 1 + MIN_PREVIEW_TEXT_W;
    lines
        .iter()
        .enumerate()
        .skip(scroll as usize)
        .take(inner.height as usize)
        .map(|(i, runs)| {
            let mut spans = Vec::with_capacity(runs.len() + 1);
            if gutter {
                spans.push(Span::styled(
                    format!("{:>num_w$} ", i + 1),
                    Style::default().fg(th.edge),
                ));
            }
            spans.extend(
                runs.iter()
                    .map(|(kind, text)| Span::styled(text.clone(), token_style(*kind, th))),
            );
            Line::from(spans)
        })
        .collect()
}

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
    if crate::config::setting_at(view.tab, view.selected).is_some_and(|s| s.kind.is_text()) {
        return "↑/↓: move  Enter: type a value (empty = default)  R: reset all  Tab: next tab";
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
pub(crate) fn centered_rect_pct(frame: Rect, pct_w: u16, pct_h: u16) -> Rect {
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
pub(crate) fn visible_positions<'a>(
    positions: &'a [usize],
    shown: &str,
    full: &str,
) -> &'a [usize] {
    let shown_len = shown.chars().count();
    if shown_len < full.chars().count() {
        let keep = positions.iter().take_while(|&&p| p + 1 < shown_len).count();
        &positions[..keep]
    } else {
        positions
    }
}

/// The LAUNCHER VIEW's pane boundary: a rule along the whole edge the
/// pane opens with — across its first row under the cards, or down the
/// column it keeps clear beside them (`launcher::pane_edge`) — so the
/// pane reads as a panel of its own even with the keys on the grid and no
/// focus tint to set it apart — without it the TAB STRIP looked like more
/// text under the cards. On it, the grip: a short heavy stretch across
/// the middle, the one visible sign that the edge can be dragged, as the
/// `┃` grips are on the panels' rules. Lit while the pointer rests on it
/// or while it is being dragged.
fn draw_launcher_pane_grip(
    buf: &mut ratatui::buffer::Buffer,
    app: &App,
    side: crate::launcher::PaneSide,
    pane: Rect,
) {
    /// Cells the grip runs across: wide enough to read as a handle rather
    /// than as a stray mark on the rule.
    const GRIP_W: u16 = 8;
    /// Rows it runs down a pane beside the cards: a cell is about twice as
    /// tall as it is wide, so half the width reads as the same handle.
    const GRIP_H: u16 = GRIP_W / 2;
    let th = app.theme;
    let edge = crate::launcher::pane_edge(side, pane);
    let (cells, rule, grip, len): (Vec<(u16, u16)>, _, _, _) = if side.beside() {
        let cells = (edge.y..edge.y + edge.height).map(|y| (edge.x, y));
        (cells.collect(), "│", "┃", GRIP_H)
    } else {
        let cells = (edge.x..edge.x + edge.width).map(|x| (x, edge.y));
        (cells.collect(), "─", "━", GRIP_W)
    };
    for &at in &cells {
        if let Some(cell) = buf.cell_mut(at) {
            cell.set_symbol(rule);
            cell.set_style(Style::default().fg(th.edge));
        }
    }
    // Beside the cards the rule crosses the one under both headers — the
    // grid's and the pane's TAB STRIP's, on the same row — so it meets it.
    if side.beside() && edge.height > 2 {
        if let Some(cell) = buf.cell_mut((edge.x, edge.y + 2)) {
            cell.set_symbol("┼");
        }
    }
    let span = u16::try_from(cells.len()).unwrap_or(u16::MAX);
    if span < len + 2 {
        return; // no room for the grip and rule either side of it
    }
    let active = app.launcher_pane_drag.is_some() || app.hover_launcher_pane;
    let fg = if active { th.accent } else { th.muted };
    let from = usize::from((span - len) / 2);
    for &at in &cells[from..from + usize::from(len)] {
        if let Some(cell) = buf.cell_mut(at) {
            cell.set_symbol(grip);
            cell.set_style(Style::default().fg(fg));
        }
    }
}

/// Subtle focus cue: fill the whole focused panel with the theme's
/// `focus_tint` — a near-black shade of the accent, so the panel reads as
/// a faintly lit surface. Painted after content, and only onto cells whose
/// background is still untouched, so selection fills and PTY-drawn
/// colors sit on top of the tint instead of under it. The pane wears it
/// whenever it has the keys; while the grid has them, the cursor's card
/// wears the same wash instead (`launcher_view::draw_card`).
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

/// The BLACK BACKGROUND setting: paint every cell still on the terminal's
/// default background pure black. Runs last in a frame, after the overlays
/// and the focus tint, and — like the tint — only touches `Reset` cells, so
/// selection fills, the tint and the colors a session draws itself stay on
/// top of it.
fn draw_black_background(buf: &mut ratatui::buffer::Buffer, area: Rect) {
    for y in area.y..area.y + area.height {
        for x in area.x..area.x + area.width {
            if let Some(cell) = buf.cell_mut((x, y)) {
                if cell.bg == Color::Reset {
                    cell.bg = crate::theme::BLACK_BACKGROUND;
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

/// Bordered panel frame: rounded corners everywhere for a softer, modern
/// look. Focus has to be unmissable, so the focused panel gets an accent
/// border plus a solid accent-background title chip, versus a thin dim
/// border and plain muted title.
pub(crate) fn panel_block(title: &str, focused: bool, th: Theme) -> Block<'_> {
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

/// The `↗ open in browser` BUTTON's label, spaces and all.
pub(crate) const BROWSER_BUTTON: &str = " ↗ open in browser ";

/// The `↗ open in browser` BUTTON on a reading pane's top border — the
/// ISSUES and PULL REQUESTS MODALS' right frame (`HitTarget::ModalBrowser`).
/// Drawn over the border after the block has, pinned right, and its rect
/// handed back for the modal to write into its view, where the click
/// (`handle_mouse`) and the pointer ([`browser_button_under`]) find it.
/// `title_w` is the width of the frame's own title on the left, spaces
/// included: the button is left off — `Rect::default()`, which no point is
/// inside — when the frame cannot hold both a cell apart, since a label
/// written over the title would read as neither. `hovered` underlines it in
/// the accent, the mark the header's buttons take while the pointer rests
/// on them; otherwise it wears the frame title's muted.
pub(crate) fn browser_button(
    f: &mut Frame,
    frame: Rect,
    title_w: u16,
    hovered: bool,
    th: Theme,
) -> Rect {
    let w = BROWSER_BUTTON.chars().count() as u16;
    // The left corner, the title, a cell of air, the button, the right corner.
    if frame.height == 0 || frame.width < 1 + title_w + 1 + w + 1 {
        return Rect::default();
    }
    let rect = Rect {
        x: frame.x + frame.width - 1 - w,
        y: frame.y,
        width: w,
        height: 1,
    };
    let style = if hovered {
        Style::default()
            .fg(th.accent)
            .add_modifier(Modifier::UNDERLINED)
    } else {
        Style::default().fg(th.muted)
    };
    f.render_widget(Paragraph::new(Span::styled(BROWSER_BUTTON, style)), rect);
    rect
}

/// The `↗ open in browser` BUTTON under the pointer: `HitTarget::ModalBrowser`
/// when a modal with one is up and `pos` is on it, what
/// `event_loop::update_pointer` puts in `App::hover_crumb` — the modals
/// keep their rects outside the hit map, as they do their list edges.
pub(crate) fn browser_button_under(app: &App, pos: Position) -> Option<HitTarget> {
    let button = match &app.overlay {
        Some(Overlay::PullRequests(v)) => v.browser_area,
        Some(Overlay::Issues(v)) => v.browser_area,
        _ => return None,
    };
    button.contains(pos).then_some(HitTarget::ModalBrowser)
}

/// One piece of the PR & ISSUE COUNTS badge: its text, its style, and the
/// button it is, if it is one.
pub(crate) type BadgePart = (String, Style, Option<HitTarget>);

/// The PR & ISSUE COUNTS badge (Settings → Experimental):
/// ` 3 prs · 2 issues` — the pull requests in the accent the OPEN PRS rows
/// wear (`pr_row::look`), the issues in the green the ISSUES MODAL paints
/// `open` in, a dim `·` between — as spans, with the columns they take
/// together so the name can be truncated around them. A count that is
/// zero, or not known yet, leaves its word out, and the badge goes with
/// both; one `pr` or `issue` is singular; a list cut off at the fetch cap
/// counts `100+`, as the OPEN PRS header does.
///
/// Each count carries the button it is: `2 prs` opens the PULL REQUESTS
/// MODAL and `1 issue` the ISSUES MODAL. The air before the first and the
/// `·` between them carry none, so a click lands on a word or on nothing.
pub(crate) fn open_counts_badge(
    counts: (Option<usize>, Option<usize>),
    th: Theme,
) -> Option<(Vec<BadgePart>, usize)> {
    fn word(n: usize, one: &str, many: &str, cap: usize) -> Option<String> {
        match n {
            0 => None,
            1 => Some(format!("1 {one}")),
            n if n >= cap => Some(format!("{cap}+ {many}")),
            n => Some(format!("{n} {many}")),
        }
    }
    let (prs, issues) = counts;
    let parts = [
        prs.and_then(|n| word(n, "pr", "prs", crate::pull_request::LIST_LIMIT))
            .map(|text| (text, th.accent, HitTarget::LauncherPullRequests)),
        issues
            .and_then(|n| word(n, "issue", "issues", crate::issues::LIST_LIMIT))
            .map(|text| (text, th.ok, HitTarget::LauncherIssues)),
    ];
    let mut spans: Vec<BadgePart> = Vec::new();
    for (text, color, hit) in parts.into_iter().flatten() {
        let gap = if spans.is_empty() { " " } else { " · " };
        spans.push((gap.into(), Style::default().fg(th.dim), None));
        spans.push((text, Style::default().fg(color), Some(hit)));
    }
    if spans.is_empty() {
        return None;
    }
    let len = spans.iter().map(|(s, _, _)| s.chars().count()).sum();
    Some((spans, len))
}

/// Sweep shades for a status that animates. The live two sweep for as long
/// as they last: running rows shimmer yellow, needs-feedback rows red. A
/// finished row takes the ONE-SHOT SWEEP — the done ramp, while `fresh`
/// says an unread finish under it is only seconds old
/// (`app::fresh_done`) — and then holds still like every other status:
/// motion means live, or just changed; a row at rest is at rest. `enabled`
/// is the animations setting — off, nothing animates.
fn sweep_ramp(
    status: Option<AgentStatus>,
    fresh: bool,
    th: Theme,
    enabled: bool,
) -> Option<[Color; 3]> {
    if !enabled {
        return None;
    }
    match status {
        Some(AgentStatus::Running) => Some(th.warn_sweep),
        Some(AgentStatus::NeedsFeedback) => Some(th.err_sweep),
        Some(AgentStatus::Finished) if fresh => Some(th.done_sweep),
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

pub(crate) const PENDING_SESSION_BADGE: &str = " starting";

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

/// The dot. `unseen` splits the finished state in two: blue while a
/// finished turn is still unread — the one state that wants a human — and
/// green once the cursor has been on it, which is a result filed away, not
/// a job. Every other status ignores the flag.
fn status_dot(status: Option<AgentStatus>, unseen: bool, th: Theme) -> Span<'static> {
    let glyph = match status {
        Some(AgentStatus::Disconnected) | None => "○ ",
        Some(_) => "● ",
    };
    Span::styled(glyph, Style::default().fg(status_color(status, unseen, th)))
}

/// The STATUS DOT's color on its own, for the marks that answer to it:
/// the selection rail of a PILL ROW, the `▌` of a PROJECT button and the
/// TAB UNDERLINE all take the row's dot color, so the cursor carries the
/// row's status rather than the theme accent.
fn status_color(status: Option<AgentStatus>, unseen: bool, th: Theme) -> Color {
    match status {
        Some(AgentStatus::Fresh) => th.dim,
        Some(AgentStatus::Running) => th.warn,
        Some(AgentStatus::Finished) if unseen => th.done,
        Some(AgentStatus::Finished) => th.ok,
        Some(AgentStatus::NeedsFeedback) => th.err,
        Some(AgentStatus::Terminated) => th.special,
        Some(AgentStatus::Disconnected) | None => th.dim,
    }
}

/// The selection mark's color on a focused selection: the row's `mark`
/// (its STATUS DOT color, or the accent for a row that has no dot),
/// lifted from dim to muted the way a dim dot is lifted on the fill — a
/// FRESH row's mark is gray, but not the gray of an unfocused panel.
fn selection_mark(mark: Color, th: Theme) -> Color {
    if mark == th.dim {
        th.muted
    } else {
        mark
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
/// These rows (overlay lists) carry no STATUS DOT, so the mark is the
/// accent.
pub(crate) fn render_row(
    f: &mut Frame,
    area: Rect,
    spans: Vec<Span>,
    selected: bool,
    focused: bool,
    th: Theme,
) {
    render_button(f, area, vec![spans], selected, focused, th, 0, th.accent);
}

/// Render one list entry as a button `area.height` rows tall: the
/// selection fill covers the whole rect, the `▌` marker runs down its
/// left edge in `mark` (the row's STATUS DOT color — see
/// `selection_mark`), and `text` takes consecutive rows starting at
/// `text_row` (0-based, inside the rect). A second entry is a terminal's
/// answer to a smaller line under the first, so the caller must size
/// `area` for it. Dim spans (idle dots, archived names, subtitles) would
/// sink into the selection fill, so they get lifted to muted there.
#[allow(clippy::too_many_arguments)]
fn render_button<'a>(
    f: &mut Frame,
    area: Rect,
    mut text: Vec<Vec<Span<'a>>>,
    selected: bool,
    focused: bool,
    th: Theme,
    text_row: u16,
    mark: Color,
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
            Span::styled("▌", Style::default().fg(selection_mark(mark, th)))
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

/// The ISSUE PREVIEW: what the pane shows while the Worktrees cursor rests
/// on a PROJECT ISSUES GROUP row (`App::previewed_issue`) — the ISSUES
/// MODAL's reading pane, in the pane: headline, description, then the
/// conversation once it lands, scrolled by `pr_preview_scroll` like the
/// pull request's. The description rides the list, so there is nothing to
/// wait for before the first paint; only the comments are fetched on the
/// rest, and `issues::lines` says so until they land.
fn draw_issue_preview(f: &mut Frame, app: &mut App, area: Rect, focused: bool) {
    let th = app.theme;
    let Some(issue) = app.previewed_issue().cloned() else {
        return;
    };
    let left = vec![
        Span::styled(" · ".to_string(), Style::default().fg(th.dim)),
        Span::styled(format!("#{}", issue.number), Style::default().fg(th.muted)),
    ];
    let inner = titled_frame(f, area, "ISSUE", left, None, focused, th);
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

    let lines = crate::issues::lines(
        &issue,
        app.issue_detail.get(&issue.url),
        app.issue_detail_failed.contains(&issue.url),
        app.issue_comment_inflight.contains(&issue.url),
        inner.width as usize,
        th,
    );
    app.pr_preview_lines = lines.len();
    // Clamp here, as the pull request's pane does: this is what knows how
    // many rows the prose wrapped to.
    let max = (lines.len() as u16).saturating_sub(inner.height.max(1));
    let scroll = app.pr_preview_scroll.min(max);
    app.pr_preview_scroll = scroll;
    let shown: Vec<Line> = lines.into_iter().skip(scroll as usize).collect();
    f.render_widget(Paragraph::new(shown), inner);
}

/// The CLOUD SESSION PANEL: what the pane shows for a Claude Cloud row.
/// The agent works in a cloud sandbox nebula has no terminal into — the
/// `claude --cloud <task>` create prints the session id and exits — so
/// rather than a dead pane ending in "Resume with: …" the row gets a
/// short explanation and the session's link, underlined and clickable,
/// with the keys that open it. Nothing here is a PTY: no cursor, no
/// scrollback, nothing to lock the keyboard into.
fn draw_cloud_session(f: &mut Frame, app: &mut App, area: Rect, focused: bool) {
    let th = app.theme;
    let Some(cloud) = app.previewed_cloud() else {
        return;
    };
    let left = vec![
        Span::styled(" · ".to_string(), Style::default().fg(th.dim)),
        Span::styled(cloud.name.clone(), Style::default().fg(th.muted)),
    ];
    let inner = titled_frame(f, area, "CLAUDE CLOUD", left, None, focused, th);
    let inner = Rect {
        x: inner.x + 1,
        width: inner.width.saturating_sub(1),
        ..inner
    };
    app.term_area = inner;
    // Nothing in this pane is a PTY, so the link/file scanners have nothing
    // to find — clear them or ⌥click would still hit last frame's hits.
    app.term_links = Vec::new();
    app.term_file_links = Vec::new();

    let w = (inner.width as usize).saturating_sub(2).max(20);
    let prose = |text: &str, style: Style| -> Vec<Line<'static>> {
        crate::pr_preview::wrap(text, w)
            .into_iter()
            .map(|t| Line::from(Span::styled(format!(" {t}"), style)))
            .collect()
    };
    let mut lines: Vec<Line<'static>> = vec![
        Line::from(""),
        Line::from(vec![
            Span::styled(" ◆ ", Style::default().fg(th.accent)),
            Span::styled(
                "This session runs in Claude Cloud",
                Style::default().fg(th.text).add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(""),
    ];
    lines.extend(prose(
        "The agent works in a cloud sandbox, not in a terminal here. Its turns, its questions and its diff are on the session's page:",
        Style::default().fg(th.muted),
    ));
    lines.push(Line::from(""));
    // The link: folded at the pane edge rather than clipped — a link with
    // its tail cut off is a link that cannot be trusted — every row of it
    // a hit target, registered ahead of the pane so a click on it wins.
    let link_style = Style::default()
        .fg(th.accent)
        .add_modifier(Modifier::UNDERLINED);
    let fold = (inner.width as usize).saturating_sub(1).max(1);
    let url_chars: Vec<char> = cloud.url.chars().collect();
    for chunk in url_chars.chunks(fold) {
        let row = lines.len() as u16;
        let text: String = chunk.iter().collect();
        if row < inner.height {
            let width = (chunk.len() as u16 + 1).min(inner.width);
            let link = Rect::new(inner.x, inner.y + row, width, 1);
            app.hits.push((link, HitTarget::CloudSessionLink));
        }
        lines.push(Line::from(Span::styled(format!(" {text}"), link_style)));
    }
    lines.push(Line::from(""));
    lines.push(Line::from(vec![
        Span::styled(
            format!(" {}", key_hint(app, Action::Activate)),
            Style::default().fg(th.accent).add_modifier(Modifier::BOLD),
        ),
        Span::styled(" or click: open in browser", Style::default().fg(th.dim)),
        Span::styled("   ·   ", Style::default().fg(th.dim)),
        Span::styled(
            key_hint(app, Action::ContextMenu),
            Style::default().fg(th.accent).add_modifier(Modifier::BOLD),
        ),
        Span::styled(": send a message", Style::default().fg(th.dim)),
    ]));
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        format!(" {}", cloud.cloud_session_id),
        Style::default().fg(th.dim),
    )));
    app.hits.push((inner, HitTarget::TerminalPane));
    f.render_widget(Paragraph::new(lines), inner);
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

/// The buffer cell under a PTY's cursor when its screen is drawn at
/// `area`, by the same arithmetic `PseudoTerminal` paints it with: the
/// row shifted down by however far the pane is scrolled back, and None
/// once that puts it below the pane. A cursor resting past the last
/// column (a line filled to the edge) clamps onto it. Whether the PTY
/// has hidden its cursor is not asked: the host cursor is never shown
/// from here, only placed — a real terminal anchors IME composition to
/// the cursor's cell whether or not it is drawn (see `App::host_cursor`).
fn pty_cursor_cell(screen: &vt100::Screen, area: Rect) -> Option<Position> {
    if area.width == 0 {
        return None;
    }
    let (row, col) = screen.cursor_position();
    let scrollback = u16::try_from(screen.scrollback()).unwrap_or(u16::MAX);
    let row = row.saturating_add(scrollback);
    (row < area.height).then(|| Position::new(area.x + col.min(area.width - 1), area.y + row))
}

fn draw_terminal(f: &mut Frame, app: &mut App, area: Rect) {
    let th = app.theme;
    let focused = app.focus == Focus::Terminal;
    // A cursor is resting on an open pull request — the Worktrees cursor
    // on a PROJECT OPEN PRS GROUP row, or the focused Sessions cursor on
    // the PR ROW: the pane reads it. The attachment underneath stays live —
    // walking down into either pull-request group and back must not churn
    // detach/attach.
    if app.previewed_pr().is_some() {
        draw_pr_preview(f, app, area, focused);
        return;
    }
    // An open issue under the Worktrees cursor: the pane reads it, as it
    // reads a pull request.
    if app.previewed_issue().is_some() {
        draw_issue_preview(f, app, area, focused);
        return;
    }
    // A Claude Cloud row: the agent runs in the cloud sandbox, so the pane
    // says where and links there instead of showing a PTY nebula would
    // have to keep teleporting to stay current.
    if app.previewed_cloud().is_some() {
        draw_cloud_session(f, app, area, focused);
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
        // Nothing has come off the PTY yet and nothing will for a while:
        // the session was reaped while the user was elsewhere and its CLI
        // is booting. Say so — the blank grid on its own reads as a hang.
        // (A live session's replay lands within a frame; that blank is
        // not worth a word that would only flash.)
        Some(t) if t.booting => Some(Span::styled(
            "starting…".to_string(),
            Style::default().fg(th.dim),
        )),
        // The LAUNCHER VIEW's pane says nothing of the lock: its header's
        // right end is the CLOSE BUTTON, and the accent rule under the
        // strip already says the keys are in there.
        Some(_) if app.term_locked && !app.launcher_active() => Some(Span::styled(
            "INPUT".to_string(),
            Style::default().fg(th.accent).add_modifier(Modifier::BOLD),
        )),
        _ => None,
    };
    // A LAUNCHER VIEW session full-screened over its grid gets a breadcrumb
    // back to the grid rather than the panels' `TERMINAL · name`; the pane
    // under the grid gets the TAB STRIP, which says what it is reading —
    // the SESSION the cursor is on, or one of the checkout's TERMINALS —
    // and is how that gets swapped. The strip names the card under the
    // cursor itself, so the panels' ` · <attached>` is not added beside
    // it: with the pane on a terminal the attachment IS that terminal,
    // and the SESSION tab has to go on saying what it would come back to.
    let inner = if app.launcher_active() && app.collapsed {
        launcher_view::crumb_frame(f, app, area)
    } else if app.launcher_active() {
        launcher_view::pane_frame(f, app, area, right, focused)
    } else {
        terminal_frame(f, area, left, right, focused, th)
    };
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
        Some(term) if term.booting && !term.exited => {
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
            app.host_cursor = pty_cursor_cell(screen, inner);
            // Selection highlight: overlay REVERSED on the selected cells
            // (stream selection — full rows between the endpoints).
            if let Some(sel) = app.term_selection.filter(|s| s.active) {
                let ((start_col, start_line), (end_col, end_line)) = sel.bounds();
                let reversed = Style::default().add_modifier(Modifier::REVERSED);
                let last_col = inner.width.saturating_sub(1);
                // The selection names HISTORY LINES; the rows on screen
                // are `base..base + height` of them at this scroll, and
                // only the part of the selection in that window paints.
                let base = screen.history_base();
                for row in 0..inner.height {
                    let line = base + u64::from(row);
                    if line < start_line || line > end_line {
                        continue;
                    }
                    let (from, to) = if start_line == end_line {
                        (start_col, end_col)
                    } else if line == start_line {
                        (start_col, last_col)
                    } else if line == end_line {
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
        // The LAUNCHER VIEW's pane with nothing in it — a project with no
        // session yet — is an empty panel: the strip over it already says
        // which key opens a terminal here, and is a button for it.
        None if app.launcher_active() => (Vec::new(), Vec::new()),
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

/// The bottom bar, drawn under the splash and the collapsed view too,
/// with the KEY COMBO DISPLAY on the padding row above it.
fn draw_footer(f: &mut Frame, app: &mut App, area: Rect) {
    draw_footer_bar(f, app, area);
    draw_key_combo(f, app, area);
}

/// The KEY COMBO DISPLAY (Settings → Experimental): the last key press
/// and what it did — `j - Move down` — on the footer's padding row at the
/// far left, the one blank row on screen and right where vim keeps
/// `showcmd`. Each key sits in a keycap (the selected-row fill) so it
/// reads across a screen share; the label is plain text. Nothing is drawn
/// once the press has aged out (`key_combo::LINGER`; the loop clears it)
/// or while the setting is off, so the row stays the breathing space it
/// was.
fn draw_key_combo(f: &mut Frame, app: &App, area: Rect) {
    let Some(combo) = &app.key_combo else {
        return;
    };
    if area.height < 2 || area.width == 0 {
        return;
    }
    let row = Rect {
        y: area.y,
        height: 1,
        ..area
    };
    let th = app.theme;
    let cap = Style::default()
        .fg(th.accent)
        .bg(th.sel_bg)
        .add_modifier(Modifier::BOLD);
    let mut spans = vec![Span::raw(" ")];
    for (i, key) in combo.keys.iter().enumerate() {
        if i > 0 {
            spans.push(Span::raw(" "));
        }
        spans.push(Span::styled(format!(" {} ", key.display()), cap));
    }
    if let Some(does) = &combo.does {
        spans.push(Span::styled(" - ", Style::default().fg(th.dim)));
        spans.push(Span::styled(does.as_str(), Style::default().fg(th.text)));
    }
    f.render_widget(Paragraph::new(Line::from(spans)), row);
}

/// Draw the bar.
fn draw_footer_bar(f: &mut Frame, app: &mut App, area: Rect) {
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
    } else if let Some(Overlay::Diff(view)) = &app.overlay {
        Span::styled(
            if view.tree.is_some() {
                "type: filter  ↑/↓: move  ←/→: fold  ⇧↑/↓: scroll  Ctrl+d/u: page  Ctrl+t: flat list  Ctrl+u: clear filter  Esc: clear/close"
            } else {
                "type: filter  ↑/↓: file  ⇧↑/↓: scroll  Ctrl+d/u: page  Ctrl+t: tree  Ctrl+u: clear filter  Esc: clear/close"
            },
            Style::default().fg(th.dim),
        )
    } else if let Some(Overlay::FileTabs(view)) = &app.overlay {
        Span::styled(
            file_tabs_keys_hint(view, app.vim.as_ref().is_some_and(|v| v.embedded)),
            Style::default().fg(th.dim),
        )
    } else if let Some(Overlay::Tree(view)) = &app.overlay {
        let md = markdown_toggle_hint("Ctrl+r", view.markdown, view.pretty);
        Span::styled(
            format!(
                "type: filter  ↑/↓: move  ←/→: fold  Enter: open/edit  ⇧↑/↓: scroll{md}  \
                 Ctrl+u: clear filter  Esc: clear/close"
            ),
            Style::default().fg(th.dim),
        )
    } else if let Some(Overlay::Files(view)) = &app.overlay {
        // A markdown selection is read first (the FILE TABS); the hint
        // says so rather than promising the editor.
        let enter = if view
            .selected_path()
            .is_some_and(crate::markdown::is_markdown_path)
        {
            "Enter: preview".to_string()
        } else {
            format!("Enter: edit in {}", editor_name(&view.editor))
        };
        Span::styled(
            format!(
                "type: search  ↑/↓: move  {enter}  Ctrl+y: copy path  Ctrl+u: clear  Esc: clear/close"
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
    } else if let Some(Overlay::AgentPresets(view)) = &app.overlay {
        Span::styled(
            crate::preset_overlays::footer_hint(view),
            Style::default().fg(th.dim),
        )
    } else if matches!(&app.overlay, Some(Overlay::AgentPresetEditor(_))) {
        Span::styled(
            "Tab/↑↓: next field  ←/→: cycle  Shift+Enter/^J: newline  Enter: save  Esc: back to list",
            Style::default().fg(th.dim),
        )
    } else if let Some(Overlay::Issues(view)) = &app.overlay {
        Span::styled(
            crate::issues::footer_hint(view),
            Style::default().fg(th.dim),
        )
    } else if matches!(&app.overlay, Some(Overlay::PullRequests(_))) {
        Span::styled(crate::pr_modal::footer_hint(), Style::default().fg(th.dim))
    } else if let Some(Overlay::BranchSwitch(view)) = &app.overlay {
        Span::styled(
            crate::branch_switch::footer_hint(view),
            Style::default().fg(th.dim),
        )
    } else if matches!(&app.overlay, Some(Overlay::Menu(m)) if m.is_project_picker()) {
        Span::styled(
            "type: filter  Enter: open the project  ↑/↓: move  Esc: close",
            Style::default().fg(th.dim),
        )
    } else if let Some(hint) = app.overlay.as_ref().and_then(|o| match o {
        Overlay::Menu(m) => menu_footer_hint(m),
        _ => None,
    }) {
        Span::styled(hint, Style::default().fg(th.dim))
    } else if matches!(&app.overlay, Some(Overlay::Prompt(p)) if matches!(p.kind, crate::app::PromptKind::QuickPrompt(_)))
    {
        // `^P`, `^O`, `Tab` and `^N` are on the box itself now, each
        // beside the thing it changes — a third copy down here was most
        // of what made this screen read as a wall of chords. A box
        // standing on a modal goes back to it.
        let hint = match app
            .overlay
            .as_ref()
            .and_then(crate::quick_prompt::modal_under)
        {
            Some(crate::quick_prompt::ModalUnder::Issues(_)) => {
                "Enter: launch  ⇧Tab: preset  Esc: back to issues"
            }
            Some(crate::quick_prompt::ModalUnder::PullRequests(_)) => {
                "Enter: launch  ⇧Tab: preset  Esc: back to pull requests"
            }
            None => "Enter: launch  ⇧Tab: preset  Esc: back to sessions",
        };
        Span::styled(hint, Style::default().fg(th.dim))
    } else if matches!(&app.overlay, Some(Overlay::ProjectPicker(_))) {
        Span::styled(
            "type: filter projects  ↑/↓: move  Enter: aim the box there  Esc: clear/back to the box",
            Style::default().fg(th.dim),
        )
    } else if app.overlay.is_some() {
        Span::styled("Esc: close  Enter: confirm", Style::default().fg(th.dim))
    } else if app.splash_showing() {
        // The splash covers the panels, so every panel hotkey is dead here.
        // List only what actually fires.
        Span::styled(
            {
                let k = |a| key_hint(app, a);
                // Launched inside a repo: Enter opens it, and `o` is for
                // any other folder.
                let here = app
                    .launch_repo_name()
                    .map(|name| format!("{}: open {name}  ", k(Action::Activate)))
                    .unwrap_or_default();
                format!(
                    "{here}{}: open {}folder  {}: ssh host  {}: settings  {}: help  {}: quit",
                    k(Action::AddProject),
                    if here.is_empty() { "a " } else { "another " },
                    k(Action::Hosts),
                    k(Action::Settings),
                    k(Action::Help),
                    k(Action::Quit),
                )
            },
            Style::default().fg(th.dim),
        )
    } else if app.launcher_grid()
        && app.focus != Focus::Terminal
        && app.launcher_tab_cursor.is_some()
    {
        // The LAUNCHER VIEW's PROJECT TABS holding the keys (`k`,`k` off
        // the top row of cards): walking the header's cursor, which
        // switches the grid as it goes, and the ways back down.
        let k = |a| key_hint(app, a);
        let down = k(Action::MoveDown);
        Span::styled(
            format!(
                "{}{}: switch project  {} or {down}{down}: into its cards  {}: close tab  esc: back to the cards  {}: help  {}: quit",
                k(Action::FocusLeft),
                k(Action::FocusRight),
                k(Action::Activate),
                k(Action::CloseProjectTab),
                k(Action::Help),
                k(Action::Quit),
            ),
            Style::default().fg(th.dim),
        )
    } else if app.launcher_grid() && app.focus != Focus::Terminal {
        // The LAUNCHER VIEW's GRID: walking the cards, opening the one
        // under the cursor, and the PROJECT TABS beside them. Not while
        // the pane under it has the keys — those are the pane's own
        // hints, below.
        let k = |a| key_hint(app, a);
        let move_keys = format!(
            "{}{}{}{}",
            k(Action::FocusLeft),
            k(Action::MoveDown),
            k(Action::MoveUp),
            k(Action::FocusRight),
        );
        Span::styled(
            if app.show_archived {
                // The ARCHIVED VIEW is a different list with different
                // verbs on it: there is nothing to attach, prompt or
                // archive there, only the two a card in it takes.
                format!(
                    "{move_keys}: move  {}: unarchive  {}: delete  {}: back to live sessions  {}: jump  {}: help  {}: quit",
                    k(Action::Unarchive),
                    k(Action::Delete),
                    k(Action::ToggleArchived),
                    k(Action::Palette),
                    k(Action::Help),
                    k(Action::Quit),
                )
            } else {
                format!(
                    "{move_keys}: move  {}: open  {}: new session  {}{}: project tabs  {}: terminals  {}: archive  {}: archived  {}: diff  {}: jump  {}: settings  {}: help  {}: quit",
                    k(Action::Activate),
                    k(Action::QuickPrompt),
                    k(Action::PrevProjectTab),
                    k(Action::NextProjectTab),
                    k(Action::PaneTabs),
                    k(Action::Archive),
                    k(Action::ToggleArchived),
                    k(Action::GitDiff),
                    k(Action::Palette),
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
            // The pane is the CLOUD SESSION PANEL: there is no terminal to
            // type into, and Enter hands the session to the browser.
            Focus::Terminal if app.previewed_cloud().is_some() => format!(
                "{}: open in browser  {}: sessions",
                k(Action::Activate),
                k(Action::FocusLeft)
            ),
            Focus::Terminal if app.term.as_ref().is_some_and(|t| t.exited) => {
                "session exited — Esc: back to sessions".to_string()
            }
            Focus::Terminal if app.term_locked => format!(
                "{}: {}  {}  ⌥click: open link",
                // The pane under the cards is left by the fold's own key
                // (`^``: back to the card, again: fold the pane).
                if app.launcher_grid() {
                    app.keymap
                        .chords(Action::ToggleLauncherPane)
                        .iter()
                        .find(|c| !crate::key_combo::is_text_key(c))
                        .map(|c| c.display())
                } else {
                    None
                }
                .or_else(|| app.keymap.first(Action::UnlockTerminal).map(|c| c.display()))
                .unwrap_or_else(|| "^q".into()),
                // The LAUNCHER VIEW has its grid of sessions to go back to.
                if app.launcher_grid() {
                    "back to the card"
                } else if app.launcher_active() {
                    "sessions"
                } else {
                    "panels"
                },
                // A program that asked for the mouse gets the drag (its
                // own selection copies); promising nebula's would lie.
                if app.child_mouse_mode().0 != vt100::MouseProtocolMode::None {
                    "drag: to the app (⇧drag: terminal)"
                } else {
                    "drag: select+copy"
                },
            ),
            Focus::Terminal if app.term.is_some() => format!(
                "{}: type into terminal  {}: sessions",
                k(Action::Activate),
                k(Action::FocusLeft)
            ),
            Focus::Terminal => "select a session and press Enter to attach".to_string(),
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
                "{}: new session  {}: preset  {}: open in browser  {}: diff  PgUp/PgDn: scroll  {}: refresh  {}: search  {}: menu  {}: help",
                k(Action::New),
                k(Action::AgentPresets),
                k(Action::Activate),
                k(Action::GitDiff),
                k(Action::RefreshPullRequests),
                k(Action::Palette),
                k(Action::ContextMenu),
                k(Action::Help)
            ),
            // An issue row: the browser, a prompt or a preset on it, and
            // the pane's scroll keys.
            Focus::Worktrees if app.selected_worktree_issue().is_some() => format!(
                "{}: open in browser  {}: prompt  {}: preset  PgUp/PgDn: scroll  {}: search  {}: menu  {}: help",
                k(Action::Activate),
                k(Action::QuickPrompt),
                k(Action::AgentPresets),
                k(Action::Palette),
                k(Action::ContextMenu),
                k(Action::Help)
            ),
            Focus::Worktrees => format!(
                "{}: new worktree  {}: presets  {}: {}  {}: open  {}: terminal  {}: delete  {}: refresh PRs  {}: search  {}: menu  {}: help",
                k(Action::New),
                k(Action::AgentPresets),
                k(Action::Rename),
                if app
                    .selected_worktree()
                    .is_some_and(|w| app.worktree_running(&w.id))
                {
                    "stop"
                } else {
                    "run"
                },
                k(Action::OpenWorktree),
                k(Action::NewTerminal),
                k(Action::Delete),
                k(Action::RefreshPullRequests),
                k(Action::Palette),
                k(Action::ContextMenu),
                k(Action::Help)
            ),
            // A discovered pull request opens, reads in the pane and shows
            // its diff; it has no stored row to edit or delete, and the
            // refresh key re-asks GitHub for it. Previously saved rows
            // retain the edit and delete verbs.
            Focus::Sessions
                if app
                    .selected_link()
                    .is_some_and(|row| row.id().is_none()) =>
            {
                format!(
                    "{}: open in browser  {}: diff  PgUp/PgDn: scroll  {}: refresh  {}: menu  {}: help",
                    k(Action::Activate),
                    k(Action::GitDiff),
                    k(Action::RefreshPullRequests),
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
            // A Cloud row leads out of nebula like a link row does; the
            // menu holds the one verb that reaches the session from here.
            Focus::Sessions if app.previewed_cloud().is_some() => format!(
                "{}: open in browser  {}: rename  {}: archive  {}: del  {}: menu  {}: help",
                k(Action::Activate),
                k(Action::Rename),
                k(Action::Archive),
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
        Span::styled(text, Style::default().fg(th.dim))
    };
    // Quiet footer: context on the left, live stats on the right. The
    // hostname only earns a slot when it's a remote session, and the
    // connection state only when something is wrong.
    // (The version nameplate is spliced in at the front further down, once
    // the width left for the hints is known.)
    let mut spans = vec![Span::raw(" ")];
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
        // No changed-file count here: it rides each card's branch, where
        // it reads as that checkout's (`launcher_view::draw_card`).
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
    // A newer published release rides the nameplate as `⇡ v0.22.0`, in the
    // heads-up color: the version is already what this span says, so "and
    // a newer one exists" belongs beside it rather than anywhere else on
    // the bar. It is part of the plate for the yield below — a flash that
    // would be clipped drops both.
    let update = app.update_available.as_ref().map(|v| format!(" ⇡ v{v}"));
    let plate_w = plate.chars().count()
        + update.as_ref().map_or(0, |u| u.chars().count())
        + "  ·  ".chars().count();
    let body_w: usize = spans.iter().map(|s| s.width()).sum();
    if app.flash.is_none() || body_w + plate_w <= left.width as usize {
        let mut plate_spans = vec![Span::styled(plate, Style::default().fg(th.dim))];
        if let Some(update) = update {
            plate_spans.push(Span::styled(
                update,
                Style::default().fg(th.warn).add_modifier(Modifier::BOLD),
            ));
        }
        plate_spans.push(Span::styled("  ·  ", Style::default().fg(th.dim)));
        spans.splice(1..1, plate_spans);
    }
    f.render_widget(Paragraph::new(Line::from(spans)), left);
    if let Some(usage) = usage {
        let right = Rect {
            x: area.x + area.width.saturating_sub(right_w),
            width: right_w,
            ..area
        };
        // The readout is a button — a click opens the memory modal, as
        // `⇧M` does — and nothing about a dim figure says so, so the
        // pointer on it lifts it to full text and underlines it, as the
        // header's buttons are. Only the words are the target, laid where
        // the right alignment puts them, not the padding beside them.
        let span = Span::styled(usage, Style::default().fg(th.dim));
        let span = if app.hover_crumb == Some(HitTarget::FooterUsage) {
            span.style(
                Style::default()
                    .fg(th.text)
                    .add_modifier(Modifier::UNDERLINED),
            )
        } else {
            span
        };
        let width = (span.width() as u16).min(right.width);
        app.hits.push((
            Rect {
                x: right.x + right.width - width,
                width,
                ..right
            },
            HitTarget::FooterUsage,
        ));
        f.render_widget(
            Paragraph::new(Line::from(span)).alignment(ratatui::layout::Alignment::Right),
            right,
        );
    }
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
pub(crate) fn token_style(kind: crate::syntax::TokenKind, th: Theme) -> Style {
    use crate::syntax::TokenKind;
    match kind {
        TokenKind::Keyword => Style::default().fg(th.special),
        TokenKind::String => Style::default().fg(th.ok),
        TokenKind::Comment => Style::default().fg(th.dim),
        TokenKind::Number => Style::default().fg(th.warn),
        TokenKind::Text => Style::default(),
    }
}

/// Palette row label — the row's own name, its path left to the header
/// above it — with fuzzy-match chars lit accent-bold on top. A `quiet`
/// row — archived, or a draft pull request, dimmed end to end like its
/// panel row — stays dim all the way through. With a `ramp`, the label —
/// the very text that sweeps in its panel row — rides the same
/// left-to-right band; matched chars keep the accent highlight so the
/// sweep never buries what the query hit. No `/` in it is a path break:
/// a branch, a pull request title or a session title may carry one.
fn label_highlight_spans(
    shown: &str,
    positions: &[usize],
    quiet: bool,
    ramp: Option<[Color; 3]>,
    phase: usize,
    text: Color,
    th: Theme,
) -> Vec<Span<'static>> {
    let len = shown.chars().count();
    let hl = Style::default().fg(th.accent).add_modifier(Modifier::BOLD);
    let mut spans = Vec::new();
    let mut run = String::new();
    let mut run_style: Option<Style> = None;
    for (i, c) in shown.chars().enumerate() {
        let style = if positions.binary_search(&i).is_ok() {
            hl
        } else if quiet {
            Style::default().fg(th.dim)
        } else if let Some(ramp) = ramp {
            sweep_style(Style::default(), ramp, phase, i, len)
        } else {
            Style::default().fg(text)
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
pub(crate) fn fuzzy_highlight_spans(
    shown: &str,
    positions: &[usize],
    th: Theme,
) -> Vec<Span<'static>> {
    fuzzy_highlight_styled(shown, positions, Style::default(), th)
}

/// [`fuzzy_highlight_spans`] for text that has a color of its own — a
/// pull request row's title, red for one that cannot merge — `base` on
/// the runs the filter did not match, the accent highlight on the ones
/// it did.
pub(crate) fn fuzzy_highlight_styled(
    shown: &str,
    positions: &[usize],
    base: Style,
    th: Theme,
) -> Vec<Span<'static>> {
    if positions.is_empty() {
        return vec![Span::styled(shown.to_string(), base)];
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
                Span::styled(text, base)
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

/// Draw a multi-row field into `area` — the inside of its box — scrolled
/// only as far as keeps the caret in sight, and return the view it was
/// drawn with and how many rows the text takes. The caller hands the view
/// to the live field ([`TextInput::set_view`]; the draw works on a clone),
/// so ↑/↓, the page keys, the wheel and a click walk the rows drawn here,
/// and can put [`draw_scroll_marks`] on the box.
pub(crate) fn draw_multiline_input(
    f: &mut Frame,
    input: &TextInput,
    area: Rect,
    th: Theme,
) -> (TextView, usize) {
    draw_multiline_input_with_caret(f, input, area, th, th.accent)
}

/// [`draw_multiline_input`] with the caret in `caret` rather than the
/// accent — the dim caret a box drawn as a backdrop under a picker gets,
/// since the caret you can actually type at is the picker's.
pub(crate) fn draw_multiline_input_with_caret(
    f: &mut Frame,
    input: &TextInput,
    area: Rect,
    th: Theme,
    caret: Color,
) -> (TextView, usize) {
    let view = input.view_for(area.width.max(1), area.height);
    let (lines, _) = multiline_input_lines(input, view.width.into(), caret, th);
    let rows = lines.len();
    let shown: Vec<Line> = lines
        .into_iter()
        .skip(view.top.into())
        .take(view.height.into())
        .collect();
    f.render_widget(Paragraph::new(shown), area);
    (view, rows)
}

/// `↑ 3 more` / `↓ 5 more` at the right of a multi-row field's box while
/// rows of it are scrolled out of sight above or below — how a long prompt
/// says there is more of it than the box holds.
pub(crate) fn draw_scroll_marks(
    f: &mut Frame,
    box_area: Rect,
    view: TextView,
    rows: usize,
    color: Color,
) {
    let above = usize::from(view.top);
    let below = rows.saturating_sub(above + usize::from(view.height));
    let edges = [
        (above, '↑', box_area.y),
        (below, '↓', box_area.bottom().saturating_sub(1)),
    ];
    for (count, arrow, y) in edges {
        let mark = format!(" {arrow} {count} more ");
        let width = mark.chars().count() as u16;
        if count == 0 || box_area.width < width + 4 {
            continue;
        }
        let x = box_area.right() - 2 - width;
        f.render_widget(
            Paragraph::new(Span::styled(mark, Style::default().fg(color))),
            Rect::new(x, y, width, 1),
        );
    }
}

/// Word-wrapped rows for a multi-row field, in the field's own layout
/// ([`TextInput::rows`]) so the rows drawn are the rows ↑/↓ walk. The
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
    let ranges = input.rows(width.max(1));

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
pub(crate) fn search_line(
    input: &TextInput,
    placeholder: &str,
    area: Rect,
    th: Theme,
) -> Line<'static> {
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

    /// The selection highlight paints the part of the selection on screen
    /// at the current scroll: its endpoints are history lines, and the
    /// rows under them move as the pane scrolls back.
    #[test]
    fn selection_highlight_follows_its_text_through_the_scroll() {
        use crate::app::{AttachedTerm, TermSelection};
        use nebula_core::{AgentId, SessionRef};
        let mut app = App::new();
        let mut term = AttachedTerm::new(SessionRef::Agent(AgentId("a1".into())), 20, 5);
        let lines: Vec<String> = (0..20).map(|i| format!("line {i}")).collect();
        term.parser.process(b"\x1b[?25l");
        term.parser.process(lines.join("\r\n").as_bytes());
        app.term = Some(term);
        // Lines 17–18, through column 3 of the last: rows 2–3 of the pane
        // at the live tail.
        app.term_selection = Some(TermSelection {
            anchor: (0, 17),
            head: (3, 18),
            dragging: true,
            active: true,
            pointer: (0, 0),
        });
        // The frame takes three rows and the pane is inset a column:
        // content at x 1..=20, y 3..=7.
        let reversed_rows = |app: &mut App| -> Vec<(u16, Vec<u16>)> {
            let area = Rect::new(0, 0, 21, 8);
            let mut terminal =
                ratatui::Terminal::new(ratatui::backend::TestBackend::new(21, 8)).unwrap();
            terminal.draw(|f| draw_terminal(f, app, area)).unwrap();
            let buf = terminal.backend().buffer().clone();
            (0..8)
                .filter_map(|y| {
                    let xs: Vec<u16> = (0..21)
                        .filter(|&x| {
                            buf.cell((x, y))
                                .unwrap()
                                .modifier
                                .contains(Modifier::REVERSED)
                        })
                        .collect();
                    (!xs.is_empty()).then_some((y, xs))
                })
                .collect()
        };
        assert_eq!(
            reversed_rows(&mut app),
            vec![(5, (1..=20).collect()), (6, (1..=4).collect())]
        );
        // Scrolled back two lines: line 17 is the bottom row, line 18 is
        // below the screen.
        app.term.as_mut().unwrap().set_scroll(2);
        assert_eq!(reversed_rows(&mut app), vec![(7, (1..=20).collect())]);
        // Scrolled past the selection: nothing to paint.
        app.term.as_mut().unwrap().set_scroll(5);
        assert!(reversed_rows(&mut app).is_empty());
    }

    /// The BLACK BACKGROUND setting leaves nothing on the terminal's own
    /// background once a frame is drawn, and paints only what was: a cell
    /// something else filled keeps its color.
    #[test]
    fn black_background_paints_every_default_cell_and_nothing_else() {
        let resets = |app: &mut App| -> usize {
            let mut terminal =
                ratatui::Terminal::new(ratatui::backend::TestBackend::new(60, 20)).unwrap();
            terminal.draw(|f| draw(f, app)).unwrap();
            let buf = terminal.backend().buffer().clone();
            buf.content.iter().filter(|c| c.bg == Color::Reset).count()
        };
        let mut app = App::new();
        assert!(resets(&mut app) > 0, "off: the terminal's background shows");
        app.black_background = true;
        assert_eq!(resets(&mut app), 0, "on: every default cell goes black");

        let area = Rect::new(0, 0, 2, 1);
        let mut buf = ratatui::buffer::Buffer::empty(area);
        buf[(1, 0)].bg = app.theme.sel_bg;
        draw_black_background(&mut buf, area);
        assert_eq!(buf[(0, 0)].bg, crate::theme::BLACK_BACKGROUND);
        assert_eq!(buf[(1, 0)].bg, app.theme.sel_bg, "a fill stays on top");
    }

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

    /// A modal that floats over the box sits inside the box's rect while
    /// it fits there, leaving the frame, the title and the details row
    /// showing around it; one too wide or too tall for that falls back to
    /// the middle of the screen rather than spilling off the box.
    #[test]
    fn a_modal_over_the_box_sits_inside_it_while_it_fits() {
        let frame = Rect::new(0, 0, 130, 34);
        let boxed = centered_rect(frame, 92, 18);
        let inside = over_box_rect(frame, Some(boxed), 40, 8);
        assert!(
            inside.x > boxed.x
                && inside.right() < boxed.right()
                && inside.y > boxed.y
                && inside.bottom() < boxed.bottom(),
            "{inside:?} is not inside {boxed:?}"
        );

        // Too tall, too wide, and no box at all: centered on the screen.
        // A box off the screen's own middle tells the two apart — over a
        // centered box they are the same rect.
        let middle = |w, h| centered_rect(frame, w, h);
        let corner = Rect::new(4, 2, 92, 18);
        assert_eq!(over_box_rect(frame, Some(corner), 40, 16), middle(40, 16));
        assert_eq!(over_box_rect(frame, Some(corner), 90, 8), middle(90, 8));
        assert_eq!(over_box_rect(frame, None, 40, 8), middle(40, 8));

        // The edge of fitting: the inset's worth of box left over, which
        // still goes inside it.
        assert_eq!(
            over_box_rect(frame, Some(corner), 88, 14),
            centered_rect(corner, 88, 14)
        );
    }

    /// Every tier of a task box's hint has to fit between the borders it
    /// is drawn on, or ratatui clips the end silently — the SETTINGS
    /// OVERLAY has been bitten by exactly that.
    #[test]
    fn task_prompt_hints_fit_the_border_they_sit_on() {
        use crate::app::PromptKind;
        let quick = PromptKind::QuickPrompt(crate::quick_prompt::QuickLaunch {
            target: crate::quick_prompt::QuickTarget::Worktree(nebula_core::WorktreeId::from(
                "wt".to_string(),
            )),
            kind: nebula_core::AgentKind::Claude,
            custom: None,
            model: None,
            effort: None,
            preset: None,
            issue: None,
            pr: None,
            under: None,
            cloud: false,
        });
        let cloud = PromptKind::CloudMessage {
            id: nebula_core::AgentId::from("a".to_string()),
        };
        let comment = PromptKind::IssueComment {
            view: crate::issues::IssuesView::new(
                nebula_core::ProjectId("p".into()),
                "p".into(),
                "/tmp/p".into(),
            ),
            issue: crate::issues::IssueRef {
                url: "https://github.com/o/r/issues/1".into(),
                number: 1,
                title: "t".into(),
            },
        };
        let pr_comment = PromptKind::PrComment {
            number: 7,
            url: "https://github.com/o/r/pull/7".into(),
            label: "#7 Attach links".into(),
            back: None,
        };
        for width in 20..=TASK_PROMPT_SIZE.0 {
            for kind in [&quick, &cloud, &comment, &pr_comment] {
                let hint = task_prompt_hint(kind, width);
                assert!(
                    hint.chars().count() <= width.saturating_sub(2) as usize,
                    "{width}: {hint:?}"
                );
            }
        }
        // The full-width box advertises the two pickers and the toggle,
        // and its line break as the web's Shift+Enter, never ^J.
        let full = task_prompt_hint(&quick, TASK_PROMPT_SIZE.0);
        assert!(
            full.contains("Tab agent")
                && full.contains("⇧Tab preset")
                && full.contains("^N worktree")
                && full.contains("⇧Enter newline"),
            "{full}"
        );
        for width in 20..=TASK_PROMPT_SIZE.0 {
            let hint = task_prompt_hint(&quick, width);
            assert!(!hint.contains("^J"), "{width}: {hint:?}");
        }
        assert!(!task_prompt_hint(&cloud, TASK_PROMPT_SIZE.0).contains("Tab"));
        // The COMMENT BOX posts rather than launches, and offers no picker.
        let post = task_prompt_hint(&pr_comment, TASK_PROMPT_SIZE.0);
        assert!(
            post.contains("post") && !post.contains("launch") && !post.contains("Tab"),
            "{post}"
        );
        // A comment posts, and Esc goes back to the modal.
        let posts = task_prompt_hint(&comment, TASK_PROMPT_SIZE.0);
        assert!(posts.contains("post") && posts.contains("back"), "{posts}");
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

    /// Yellow (running) and red (needs feedback) animate for as long as
    /// they last, whatever `fresh` says; a finished row animates only while
    /// its unread finish is fresh — the ONE-SHOT SWEEP, on the done ramp —
    /// and every other status renders still text. The animations setting
    /// kills all three.
    #[test]
    fn sweep_ramp_gates_on_live_statuses_a_fresh_finish_and_the_setting() {
        let th = Theme::default();
        for fresh in [false, true] {
            assert_eq!(
                sweep_ramp(Some(AgentStatus::Running), fresh, th, true),
                Some(th.warn_sweep)
            );
            assert_eq!(
                sweep_ramp(Some(AgentStatus::NeedsFeedback), fresh, th, true),
                Some(th.err_sweep)
            );
            for status in [
                AgentStatus::Fresh,
                AgentStatus::Terminated,
                AgentStatus::Disconnected,
            ] {
                assert_eq!(
                    sweep_ramp(Some(status), fresh, th, true),
                    None,
                    "{status:?}"
                );
            }
            assert_eq!(sweep_ramp(None, fresh, th, true), None);
            for status in [
                AgentStatus::Running,
                AgentStatus::NeedsFeedback,
                AgentStatus::Finished,
            ] {
                assert_eq!(
                    sweep_ramp(Some(status), fresh, th, false),
                    None,
                    "{status:?}, animations off"
                );
            }
        }
        assert_eq!(
            sweep_ramp(Some(AgentStatus::Finished), true, th, true),
            Some(th.done_sweep),
            "just finished, unread: the one-shot"
        );
        assert_eq!(
            sweep_ramp(Some(AgentStatus::Finished), false, th, true),
            None,
            "and then it holds still"
        );
    }
}
