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
use ratatui::widgets::{Block, BorderType, Borders, Clear, Paragraph, Wrap};
use ratatui::Frame;

mod footer;
mod launcher_view;
mod overlay;

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
/// and wide: four rows of typing, a turn's worth of instruction to a
/// session already running.
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
                Span::styled(branch, Style::default().fg(th.worktree)),
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
        let scope = match &launch.target {
            crate::quick_prompt::QuickTarget::Worktree(id) => app
                .tree
                .worktrees
                .iter()
                .find(|w| &w.id == id)
                .map_or(th.muted, |w| if w.is_main { th.root } else { th.worktree }),
            crate::quick_prompt::QuickTarget::NewWorktree { .. } => th.worktree,
        };
        (
            vec![
                Span::styled("worktree: ", dim),
                Span::styled(branch, Style::default().fg(scope)),
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
    app.chrome.rows_memo.arm();
    draw_screen(f, app);
    app.chrome.rows_memo.disarm();
    if app.chrome.black_background {
        let area = f.area();
        draw_black_background(f.buffer_mut(), area);
    }
}

fn draw_screen(f: &mut Frame, app: &mut App) {
    app.chrome.hits.clear();
    app.pane.tail_cards.clear();
    app.pane.host_cursor = None;
    app.chrome.welcome_on_screen = false;

    // The bar gets a blank row above it so it breathes off the panel
    // borders, matching the terminal's own padding below the last row.
    let [body, footer] =
        Layout::vertical([Constraint::Min(3), Constraint::Length(2)]).areas(f.area());

    if app.pane.collapsed {
        draw_terminal(f, app, body);
        if app.nav.focus == Focus::Terminal {
            draw_focus_tint(f.buffer_mut(), body, app.chrome.theme);
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
        app.launcher.launcher_body = body;
        let (view_a, pane_a) = app.launcher_split(body);
        let side = app.launcher_pane_side();
        // The pane's edge facing the cards is draggable, as the panels'
        // boundaries are: its opening row (the rule) — or, with the pane
        // beside the cards, column — and the grid's one next to it are the
        // grab zone (`launcher::pane_grab_zone`), registered first so they win
        // `hit_at`'s first-match scan against a card that lands there.
        if let Some(pane_a) = pane_a {
            app.chrome.hits.push((
                crate::launcher::pane_grab_zone(side, pane_a),
                HitTarget::LauncherPaneSplitter,
            ));
        }
        launcher_view::draw(f, app, view_a);
        if let Some(pane_a) = pane_a {
            draw_terminal(f, app, crate::launcher::pane_content(side, pane_a));
            if app.nav.focus == Focus::Terminal {
                draw_focus_tint(f.buffer_mut(), pane_a, app.chrome.theme);
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
    let th = app.chrome.theme;
    let Some(vim) = &app.pane.vim else {
        return;
    };
    if vim.embedded {
        let pane = match &app.modals.overlay {
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
            app.pane.host_cursor = pty_cursor_cell(vim.parser.screen(), inner);
            // Write-back: the post-draw sync resizes the PTY to the pane.
            if let Some(vim) = &mut app.pane.vim {
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
    app.pane.host_cursor = pty_cursor_cell(vim.parser.screen(), inner);
    // Write-back: the post-draw sync resizes the PTY to the drawn rect.
    if let Some(vim) = &mut app.pane.vim {
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
/// type-ahead MODEL / EFFORT submenus — the Claude ones name `Tab` too,
/// first, where a narrow terminal cuts it last. None for a plain context
/// menu, which keeps the generic `Esc: close  Enter: confirm`. The keys
/// live down here, not in the modal's bottom border, so the modal stays
/// as narrow as its rows.
pub(crate) fn menu_footer_hint(menu: &crate::app::ContextMenu) -> Option<String> {
    let agent_jump = menu.hovered_agent_kind().is_some();
    let cloud = menu.hovered_claude_cloud().map(|on| {
        if on {
            "Tab: cloud on  "
        } else {
            "Tab: cloud off  "
        }
    });
    if menu.filter.is_some() {
        return Some(format!(
            "{}type to filter  ↑/↓: move  Backspace: widen  {}Enter: pick  Esc: back",
            cloud.unwrap_or(""),
            if agent_jump { "?: settings  " } else { "" }
        ));
    }
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
    let th = app.chrome.theme;
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
    if let Some(Overlay::Prompt(p)) = &mut app.modals.overlay {
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
    overlay::draw_overlay(f, app);
}

/// An action's primary chord, for a footer hint. Unbound reads as `—`,
/// which is the truth: that verb has no key right now.
fn key_hint(app: &App, action: crate::keymap::Action) -> String {
    app.chrome
        .keymap
        .shown_first(action)
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
pub(crate) fn below_first_row(inner: Rect) -> Rect {
    Rect {
        y: inner.y.saturating_add(1),
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
    let th = app.chrome.theme;
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
    let active = app.launcher.launcher_pane_drag.is_some() || app.launcher.hover_launcher_pane;
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
    let button = match &app.modals.overlay {
        Some(Overlay::PullRequests(v)) => v.browser_area,
        Some(Overlay::Issues(v)) => v.browser_area,
        _ => return None,
    };
    button.contains(pos).then_some(HitTarget::ModalBrowser)
}

/// One piece of the PR & ISSUE COUNTS badge: its text, its style, and the
/// button it is, if it is one.
pub(crate) type BadgePart = (String, Style, Option<HitTarget>);

/// The PR & ISSUE COUNTS badge (always on; through 0.37 an Experimental
/// switch, `pr_issue_counts`):
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
/// The line count is written back to `app.github.pr_preview_lines` so the scroll
/// handlers know how far down they may go — the pane is the only thing that
/// knows how wide the prose wrapped.
fn draw_pr_preview(f: &mut Frame, app: &mut App, area: Rect, focused: bool) {
    let th = app.chrome.theme;
    let Some(pr) = app.previewed_pr() else {
        return;
    };
    let detail = app.github.pr_detail.get(&pr.url).cloned();
    let failed = app.github.pr_detail_failed.contains(&pr.url);

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
    app.pane.term_area = inner;
    app.chrome.hits.push((inner, HitTarget::TerminalPane));
    // Nothing in this pane is a PTY, so the link/file scanners have nothing
    // to find — clear them or ⌥click would still hit last frame's hits.
    app.pane.term_links = Vec::new();
    app.pane.term_file_links = Vec::new();

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
    app.github.pr_preview_lines = lines.len();
    // Clamp here rather than in the handlers: the pane is what knows how
    // many rows the prose wrapped to, and a narrower window can strand the
    // offset past the end.
    let max = (lines.len() as u16).saturating_sub(inner.height.max(1));
    let scroll = app.github.pr_preview_scroll.min(max);
    app.github.pr_preview_scroll = scroll;
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
    let th = app.chrome.theme;
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
    app.pane.term_area = inner;
    app.chrome.hits.push((inner, HitTarget::TerminalPane));
    // Nothing in this pane is a PTY, so the link/file scanners have nothing
    // to find — clear them or ⌥click would still hit last frame's hits.
    app.pane.term_links = Vec::new();
    app.pane.term_file_links = Vec::new();

    let lines = crate::issues::lines(
        &issue,
        app.github.issue_detail.get(&issue.url),
        app.github.issue_detail_failed.contains(&issue.url),
        app.github.issue_comment_inflight.contains(&issue.url),
        inner.width as usize,
        th,
    );
    app.github.pr_preview_lines = lines.len();
    // Clamp here, as the pull request's pane does: this is what knows how
    // many rows the prose wrapped to.
    let max = (lines.len() as u16).saturating_sub(inner.height.max(1));
    let scroll = app.github.pr_preview_scroll.min(max);
    app.github.pr_preview_scroll = scroll;
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
    let th = app.chrome.theme;
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
    app.pane.term_area = inner;
    // Nothing in this pane is a PTY, so the link/file scanners have nothing
    // to find — clear them or ⌥click would still hit last frame's hits.
    app.pane.term_links = Vec::new();
    app.pane.term_file_links = Vec::new();

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
            app.chrome.hits.push((link, HitTarget::CloudSessionLink));
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
            "right-click",
            Style::default().fg(th.accent).add_modifier(Modifier::BOLD),
        ),
        Span::styled(": send a message", Style::default().fg(th.dim)),
    ]));
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        format!(" {}", cloud.cloud_session_id),
        Style::default().fg(th.dim),
    )));
    app.chrome.hits.push((inner, HitTarget::TerminalPane));
    f.render_widget(Paragraph::new(lines), inner);
}

/// Borderless terminal frame: a header row (`TERMINAL · session` plus a
/// right-aligned state tag), a thin rule, then the content area. The
/// header carries the focus signal.
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
    // Row 0 is a blank spacer above the header.
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

/// Give the host terminal back the OSC 8 hyperlinks the program printed
/// (claude's markdown links, `ls --hyperlink`), so Cmd-click opens them in
/// Ghostty, iTerm2, kitty or WezTerm. `PseudoTerminal` copies only text and
/// looks into the buffer, so each linked cell is re-wrapped in its own
/// open/close pair: ratatui redraws single changed cells, and a cell that
/// carried only the open or the close would lose its link on its own.
/// `ForcedWidth` keeps the diff counting the columns the text covers, not
/// the bytes of the escape around it.
fn stamp_hyperlinks(screen: &vt100::Screen, area: Rect, buf: &mut ratatui::buffer::Buffer) {
    use unicode_width::UnicodeWidthStr as _;
    for row in 0..area.height {
        for col in 0..area.width {
            let Some(cell) = screen.cell(row, col) else {
                continue;
            };
            let Some(uri) = screen.hyperlink(cell) else {
                continue;
            };
            if !cell.has_contents() {
                continue;
            }
            let text = cell.contents();
            let width = u16::try_from(text.width()).unwrap_or(1);
            let Some(width) = std::num::NonZeroU16::new(width) else {
                continue;
            };
            let Some(out) = buf.cell_mut((area.x + col, area.y + row)) else {
                continue;
            };
            out.set_symbol(&format!("\x1b]8;;{uri}\x1b\\{text}\x1b]8;;\x1b\\"))
                .set_diff_option(ratatui::buffer::CellDiffOption::ForcedWidth(width));
        }
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
    let th = app.chrome.theme;
    let focused = app.nav.focus == Focus::Terminal;
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
    let right = match &app.pane.term {
        Some(t) if t.exited => Some(Span::styled(
            "exited".to_string(),
            Style::default().fg(th.err).add_modifier(Modifier::BOLD),
        )),
        // Refused, not booting: nothing is starting.
        Some(t) if t.refused.is_some() => Some(Span::styled(
            "not started".to_string(),
            Style::default().fg(th.warn).add_modifier(Modifier::BOLD),
        )),
        Some(t) if t.scroll_offset() > 0 => Some(Span::styled(
            format!("scroll {}", t.scroll_offset()),
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
        Some(_) if app.pane.term_locked && !app.launcher_active() => Some(Span::styled(
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
    let inner = if app.launcher_active() && app.pane.collapsed {
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
    app.pane.term_area = inner;
    app.chrome.hits.push((inner, HitTarget::TerminalPane));

    let (links, file_links) = draw_terminal_body(f, app, inner, th);
    underline_terminal_links(f, inner, &links, &file_links);
    app.pane.term_links = links;
    app.pane.term_file_links = file_links;
}

fn draw_terminal_body(
    f: &mut Frame,
    app: &mut App,
    inner: Rect,
    th: Theme,
) -> (Vec<crate::links::TermLink>, Vec<crate::links::FileLink>) {
    match &app.pane.term {
        Some(term) if term.refused.is_some() => {
            draw_refused_terminal(f, inner, th, term.refused.clone().unwrap_or_default());
            (Vec::new(), Vec::new())
        }
        Some(term) if term.booting && !term.exited => {
            draw_booting_terminal(f, inner, th);
            (Vec::new(), Vec::new())
        }
        Some(term) => {
            let screen = term.parser.screen();
            let widget = tui_term::widget::PseudoTerminal::new(screen);
            f.render_widget(widget, inner);
            stamp_hyperlinks(screen, inner, f.buffer_mut());
            app.pane.host_cursor = pty_cursor_cell(screen, inner);
            draw_terminal_selection(f, app, screen, inner);
            (
                crate::links::visible_links(term.parser.screen()),
                crate::links::visible_file_links(term.parser.screen()),
            )
        }
        // The LAUNCHER VIEW's pane with nothing in it is an empty panel: the
        // strip over it already says which key opens a terminal here.
        None if app.launcher_active() => (Vec::new(), Vec::new()),
        None => {
            draw_empty_terminal_hero(f, inner, th);
            (Vec::new(), Vec::new())
        }
    }
}

fn draw_refused_terminal(f: &mut Frame, inner: Rect, th: Theme, why: String) {
    let msg = Paragraph::new(vec![
        Line::from(""),
        Line::from(Span::styled(
            "couldn't start this session",
            Style::default().fg(th.warn).add_modifier(Modifier::BOLD),
        ))
        .centered(),
        Line::from(""),
        Line::from(Span::styled(why, Style::default().fg(th.text))),
    ])
    .wrap(Wrap { trim: false });
    let padded = Rect {
        x: inner.x + 2,
        width: inner.width.saturating_sub(4),
        ..inner
    };
    f.render_widget(msg, padded);
}

fn draw_booting_terminal(f: &mut Frame, inner: Rect, th: Theme) {
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
}

fn draw_terminal_selection(f: &mut Frame, app: &App, screen: &vt100::Screen, inner: Rect) {
    let Some(sel) = app.pane.term_selection.filter(|s| s.active) else {
        return;
    };
    let ((start_col, start_line), (end_col, end_line)) = sel.bounds();
    let reversed = Style::default().add_modifier(Modifier::REVERSED);
    let last_col = inner.width.saturating_sub(1);
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
        let line = Rect::new(inner.x + from, inner.y + row, width, 1).intersection(inner);
        f.buffer_mut().set_style(line, reversed);
    }
}

fn draw_empty_terminal_hero(f: &mut Frame, inner: Rect, th: Theme) {
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
    f.render_widget(Paragraph::new(lines).centered(), inner);
}

fn underline_terminal_links(
    f: &mut Frame,
    inner: Rect,
    links: &[crate::links::TermLink],
    file_links: &[crate::links::FileLink],
) {
    let underline = Style::default().add_modifier(Modifier::UNDERLINED);
    let segments = links
        .iter()
        .flat_map(|l| l.segments.iter())
        .chain(file_links.iter().flat_map(|l| l.segments.iter()));
    for &(row, c0, c1) in segments {
        let seg = Rect::new(inner.x + c0, inner.y + row, c1 - c0 + 1, 1).intersection(inner);
        f.buffer_mut().set_style(seg, underline);
    }
}

fn attached_session_name(app: &App) -> Option<String> {
    match &app.pane.term.as_ref()?.sref {
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
    let th = app.chrome.theme;
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
    spans.push(seg(&project.name, app.nav.focus == Focus::Projects));
    if let Some(worktree) = app.selected_worktree() {
        spans.push(sep());
        spans.push(seg(&worktree.branch, app.nav.focus == Focus::Worktrees));
        // A folded worktree's header (the NESTED layout) is where the
        // cursor is: the trail stops at the checkout, never naming the
        // card its fold hides.
        let session = app.selected_session_row().filter(|_| !app.on_folded_band());
        if let Some(session) = session {
            spans.push(sep());
            // A link's crumb is its display label, not the raw URL — the
            // crumb has 20 cells and "https://" would eat eight of them.
            let name = match session.as_link() {
                Some(link) => link.label(),
                None => session.name().to_string(),
            };
            spans.push(seg(
                &name,
                matches!(app.nav.focus, Focus::Sessions | Focus::Terminal),
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

/// The KEY COMBO DISPLAY: the last key press
/// and what it did — `j - Move down` — on the footer's padding row at the
/// far left, the one blank row on screen and right where vim keeps
/// `showcmd`. Each key sits in a keycap (the selected-row fill) so it
/// reads across a screen share; the label is plain text. Nothing is drawn
/// once the press has aged out (`key_combo::LINGER`; the loop clears it),
/// so the row stays the breathing space it was.
fn draw_key_combo(f: &mut Frame, app: &App, area: Rect) {
    let Some(combo) = &app.chrome.key_combo else {
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
    let th = app.chrome.theme;
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
    footer::draw_footer_bar(f, app, area);
}

/// The footer's right-edge readout: live sessions, their process count,
/// and nebula's total memory footprint (TUI + daemon + every session's
/// process subtree). None until the first metrics reply arrives.
fn footer_usage(app: &App) -> Option<String> {
    let m = app.jobs.last_metrics.as_ref()?;
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
        + app.jobs.client_rss_bytes
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

    /// OSC 8 links the program printed reach the buffer wrapped per cell,
    /// whether ST- or BEL-terminated, across an SGR reset inside the link,
    /// and with a `;` in the URI; text after the close and a URI carrying a
    /// control character stay plain.
    #[test]
    fn osc8_hyperlinks_are_handed_to_the_host_terminal() {
        let mut parser = vt100::Parser::new(2, 20, 0);
        parser.process(b"\x1b]8;id=1;https://a.dev/x;y\x1b\\a\x1b[1mb\x1b[0m\x1b]8;;\x1b\\c ");
        parser.process(b"\x1b]8;;file:///r/x.html\x07d\x1b]8;;\x07\r\n");
        // U+009C is a C1 ST: vte passes it through, a host could end on it.
        parser.process("\x1b]8;;https://e\u{9c}vil\x1b\\e\x1b]8;;\x1b\\".as_bytes());
        let area = Rect::new(0, 0, 20, 2);
        let mut buf = ratatui::buffer::Buffer::empty(area);
        f_render(parser.screen(), area, &mut buf);
        let link = |uri: &str, text: &str| format!("\x1b]8;;{uri}\x1b\\{text}\x1b]8;;\x1b\\");
        assert_eq!(buf[(0, 0)].symbol(), link("https://a.dev/x;y", "a"));
        assert_eq!(buf[(1, 0)].symbol(), link("https://a.dev/x;y", "b"));
        assert_eq!(buf[(2, 0)].symbol(), "c");
        assert_eq!(buf[(4, 0)].symbol(), link("file:///r/x.html", "d"));
        assert_eq!(buf[(0, 1)].symbol(), "e");

        fn f_render(screen: &vt100::Screen, area: Rect, buf: &mut ratatui::buffer::Buffer) {
            ratatui::widgets::Widget::render(
                tui_term::widget::PseudoTerminal::new(screen),
                area,
                buf,
            );
            stamp_hyperlinks(screen, area, buf);
        }
    }

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
        app.pane.term = Some(term);
        // Lines 17–18, through column 3 of the last: rows 2–3 of the pane
        // at the live tail.
        app.pane.term_selection = Some(TermSelection {
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
        app.pane.term.as_mut().unwrap().set_scroll(2);
        assert_eq!(reversed_rows(&mut app), vec![(7, (1..=20).collect())]);
        // Scrolled past the selection: nothing to paint.
        app.pane.term.as_mut().unwrap().set_scroll(5);
        assert!(reversed_rows(&mut app).is_empty());
    }

    /// A refused session's pane says so, with the whole reason wrapped,
    /// where a booting one would say "starting session…" and wait for a
    /// screen that is never coming.
    #[test]
    fn a_refused_session_shows_why_instead_of_booting() {
        let mut app = App::new();
        let mut term = crate::app::AttachedTerm::new(
            SessionRef::Agent(nebula_core::ids::AgentId("a1".into())),
            40,
            12,
        );
        term.booting = true;
        term.refused = Some(
            "the checkout for 'feat' is gone from disk (/x/feat). Recreate it: git worktree add /x/feat feat".into(),
        );
        app.pane.term = Some(term);
        let area = Rect::new(0, 0, 44, 16);
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(44, 16)).unwrap();
        terminal.draw(|f| draw_terminal(f, &mut app, area)).unwrap();
        let buf = terminal.backend().buffer().clone();
        let text: String = (0..16)
            .map(|y| {
                (0..44)
                    .map(|x| buf[(x, y)].symbol().to_string())
                    .collect::<String>()
                    + "\n"
            })
            .collect();
        assert!(text.contains("couldn't start this session"), "{text}");
        assert!(text.contains("not started"), "{text}");
        assert!(
            text.contains("worktree add /x/feat feat"),
            "the recovery is on screen: {text}"
        );
        assert!(!text.contains("starting session"), "{text}");
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
        app.chrome.black_background = true;
        assert_eq!(resets(&mut app), 0, "on: every default cell goes black");

        let area = Rect::new(0, 0, 2, 1);
        let mut buf = ratatui::buffer::Buffer::empty(area);
        buf[(1, 0)].bg = app.chrome.theme.sel_bg;
        draw_black_background(&mut buf, area);
        assert_eq!(buf[(0, 0)].bg, crate::theme::BLACK_BACKGROUND);
        assert_eq!(
            buf[(1, 0)].bg,
            app.chrome.theme.sel_bg,
            "a fill stays on top"
        );
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
