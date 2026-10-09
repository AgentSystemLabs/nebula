use super::*;

pub(super) fn draw_overlay(f: &mut Frame, app: &mut App) {
    let th = app.chrome.theme;
    let Some(overlay) = app.modals.overlay.clone() else {
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
    overlay.draw(f, app);
}

impl Overlay {
    fn draw(self, f: &mut Frame, app: &mut App) {
        match self {
            Overlay::ProjectPicker(picker) => draw_project_picker_overlay(f, app, picker),
            Overlay::Menu(menu) => draw_menu_overlay(f, app, menu),
            Overlay::Confirm(confirm) => draw_confirm_overlay(f, app, confirm),
            Overlay::Prompt(prompt) if prompt.is_multiline() => {
                draw_multiline_prompt_overlay(f, app, prompt)
            }
            Overlay::Prompt(prompt) => draw_prompt_overlay(f, app, prompt),
            Overlay::Help(view) => draw_help_overlay(f, app, view),
            Overlay::Settings(view) => draw_settings_overlay(f, app, view),
            Overlay::Metrics(view) => draw_metrics_overlay(f, app, view),
            Overlay::Diff(view) => draw_diff_overlay(f, app, view),
            Overlay::Palette(palette) => draw_palette_overlay(f, app, palette),
            Overlay::Files(finder) => draw_files_overlay(f, app, finder),
            Overlay::Grep(view) => draw_grep_overlay(f, app, view),
            Overlay::Hosts(view) => draw_hosts_overlay(f, app, view),
            Overlay::AgentPresets(view) => draw_agent_presets_overlay(f, app, view),
            Overlay::AgentPresetEditor(editor) => draw_agent_preset_editor_overlay(f, app, editor),
            Overlay::Issues(view) => draw_issues_overlay(f, app, view),
            Overlay::PullRequests(view) => draw_pull_requests_overlay(f, app, view),
            Overlay::BranchSwitch(view) => draw_branch_switch_overlay(f, app, view),
            Overlay::FileTabs(view) => draw_file_tabs_overlay(f, app, view),
            Overlay::Tree(view) => draw_tree_overlay(f, app, view),
        }
    }
}

fn draw_project_picker_overlay(
    f: &mut Frame,
    app: &mut App,
    picker: crate::launcher::ProjectPicker,
) {
    // `^P` layers the project list over the box rather than
    // taking the box away: the task you typed is still in front
    // of you while you pick where it lands.
    if launcher_view::picker_over_box(app, &picker) {
        let box_behind = crate::quick_prompt::backdrop_box(&picker.back);
        draw_multiline_prompt(f, app, &box_behind, true);
    }
    launcher_view::draw_project_picker(f, app, &picker)
}

fn draw_menu_overlay(f: &mut Frame, app: &mut App, menu: crate::app::ContextMenu) {
    let th = app.chrome.theme;
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
    // model ⌕ opus`, the bare ⌕ while nothing is typed yet. A
    // Claude list `Tab` sent to the cloud says so there as well —
    // `Claude model · cloud ⌕` — its rows being model names.
    let cloud = if menu.lists_claude_cloud() {
        crate::app::CLOUD_LABEL
    } else {
        ""
    };
    let title_text = menu.title.as_deref().map(|t| match &menu.filter {
        Some(f) if !f.query.is_empty() => format!("{t}{cloud} ⌕ {}", f.query),
        Some(_) => format!("{t}{cloud} ⌕"),
        None => format!("{t}{cloud}"),
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
    if let Some(Overlay::Menu(m)) = &mut app.modals.overlay {
        m.area = area;
    }
}

fn draw_confirm_overlay(f: &mut Frame, app: &mut App, confirm: crate::app::ConfirmDialog) {
    let th = app.chrome.theme;
    // Bulk deletes itemize their casualties across several message
    // lines — size the dialog to fit them.
    let msg_lines: Vec<&str> = confirm.message.lines().collect();
    // A delete that empties a linked worktree asks about the
    // checkout in the same dialog, so its legend has three
    // answers: yes takes both, no takes the card alone, and
    // cancel keeps the card alive. The dialog is sized to the
    // legend too, so the three never wrap.
    let three_way = matches!(
        confirm.action,
        crate::app::PendingAction::ThenDeleteWorktree { offered: true, .. }
    );
    let legend = if three_way {
        Line::from(vec![
            Span::styled("[Enter/y] yes, both", Style::default().fg(th.err)),
            Span::raw("   "),
            Span::styled("[n] no, the card only", Style::default().fg(th.err)),
            Span::raw("   "),
            Span::styled("[Esc] cancel", Style::default().fg(th.dim)),
        ])
    } else {
        Line::from(vec![
            Span::styled("[Enter/y] confirm", Style::default().fg(th.err)),
            Span::raw("   "),
            Span::styled("[Esc/n] cancel", Style::default().fg(th.dim)),
        ])
    };
    let longest = msg_lines
        .iter()
        .map(|l| l.chars().count())
        .max()
        .unwrap_or(0)
        .max(legend.width());
    let width = (longest as u16 + 4).max(CONFIRM_MIN_W);
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
    lines.push(legend);
    f.render_widget(Paragraph::new(lines), inner);
    // Record the drawn area for click hit-testing.
    if let Some(Overlay::Confirm(c)) = &mut app.modals.overlay {
        c.area = area;
    }
}

fn draw_multiline_prompt_overlay(f: &mut Frame, app: &mut App, prompt: PromptDialog) {
    draw_multiline_prompt(f, app, &prompt, false);
}

fn draw_prompt_overlay(f: &mut Frame, app: &mut App, prompt: PromptDialog) {
    let th = app.chrome.theme;
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
    if let Some(Overlay::Prompt(p)) = &mut app.modals.overlay {
        p.list_area = list_area;
        p.area = area;
    }
}

use crate::keymap::Action::*;
enum HelpKeys {
    Lit(&'static str),
    Act(&'static [crate::keymap::Action]),
}
use HelpKeys::{Act, Lit};
type HelpSection = (&'static str, &'static [(HelpKeys, &'static str)]);
const HELP_LEFT: &[HelpSection] = &[
    (
        "NAVIGATE & SEARCH",
        &[
            (Act(&[MoveDown, MoveUp]), "walk the cards (2×: tabs)"),
            (Act(&[FocusLeft, FocusRight]), "step along a row of cards"),
            (Act(&[FocusNext]), "open / fold the checkout"),
            (Act(&[Activate]), "into the pane (attach)"),
            (Act(&[HalfPageDown, HalfPageUp]), "jump two rows of cards"),
            (
                Act(&[NextProjectTab, PrevProjectTab]),
                "next / previous project tab",
            ),
            (Lit("1-9"), "open that project tab"),
            (
                Act(&[ProjectDropdown, CloseProjectTab]),
                "project list / close tab",
            ),
            (Act(&[AddProject]), "open a folder as a project"),
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
        "CHECKOUTS & GITHUB",
        &[
            (Act(&[OpenWorktree]), "open in editor (open command)"),
            (Act(&[GitDiff]), "diff (^r reviewed, ^t tree)"),
            (Act(&[OpenRepo]), "the repo on GitHub"),
            (
                Act(&[OpenPullRequest, OpenIssue]),
                "card's PR / issue on GitHub",
            ),
            (Act(&[RefreshPullRequests]), "reload PRs + issues (GitHub)"),
            (Act(&[Issues]), "issues: prompt, preset, edit"),
            (Act(&[PullRequests]), "pull requests: read / launch"),
            (Act(&[CommentPullRequest]), "comment on the card's PR"),
            (Act(&[SwitchBranch]), "switch the ⌂ root's branch"),
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
const HELP_RIGHT: &[HelpSection] = &[
    (
        "SESSIONS",
        &[
            (Act(&[QuickPrompt]), "quick prompt: Enter launches"),
            (Act(&[New]), "new session: pick a CLI first"),
            (Act(&[DuplicateSession]), "quick prompt as this card"),
            (Act(&[AgentPresets]), "agent presets: saved launches"),
            (
                Act(&[NewTerminal, OpenGhosttyTab]),
                "terminal: here / in Ghostty",
            ),
            (Act(&[FollowUp]), "follow-up prompt to the agent"),
            (Act(&[Rename]), "rename the session"),
            (
                Act(&[Archive, Unarchive, ToggleArchived]),
                "archive / unarchive / show",
            ),
            (Act(&[Delete, DeleteAll]), "delete one / delete all"),
        ],
    ),
    (
        "TERMINAL & MOUSE",
        &[
            (Act(&[Activate]), "lock input"),
            (Act(&[UnlockTerminal]), "unlock, back to the card"),
            (Act(&[PaneTabs]), "pane: session ↔ its terminals"),
            (Lit("drag"), "select + copy (2×click: word)"),
            (Lit("click / drag"), "the app that took the mouse"),
            (Lit("⌥click"), "open URL / file under cursor"),
            (Lit("⇧drag"), "select via your terminal"),
            (Lit("right-click"), "card / tab menu: run, restart"),
            (Lit("drag the pane edge"), "resize the pane"),
            (Lit("click outside"), "dismiss any modal (= Esc)"),
        ],
    ),
    (
        "GENERAL",
        &[
            (Act(&[ToggleLauncherPane]), "fold / unfold the pane"),
            (Act(&[ToggleFullScreen]), "full-screen / normal size"),
            // The SHIFT PAIRS' rule (#93), once, for every
            // letter above that has a shifted twin.
            (Lit("⇧ + letter"), "bigger, or outside nebula"),
            (Act(&[Hosts]), "ssh hosts (a: new, d: del)"),
            (Act(&[Settings]), "settings; Hotkeys tab rebinds"),
            (Act(&[Metrics]), "memory: nebula + agents"),
            (Act(&[Quit, Help]), "quit / toggle this help"),
        ],
    ),
];

fn draw_help_overlay(f: &mut Frame, app: &mut App, _: crate::app::HelpView) {
    let th = app.chrome.theme;
    // Grouped keymap in two columns: reads by task instead of one
    // giant list, and at ~24 rows it fits a stock terminal window
    // (the old single list clipped its tail on short screens).
    // Key columns come from the live keymap, not hardcoded text:
    // every one of these is rebindable in Settings → Hotkeys, and
    // help that lies about that is worse than no help. Literals
    // are for keys that belong to an overlay rather than the
    // grid, which is why they aren't rebindable.
    // What to print in the key column: a literal, or every chord
    // each action currently answers to but the ⌘ aliases
    // (`Keymap::shown_chords`).
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
                    .map(|a| app.chrome.keymap.shown_label(*a))
                    .collect::<Vec<_>>()
                    .join(" / ");
                if actions.len() != 1 || full.chars().count() <= HELP_KEY_W {
                    return full;
                }
                let chords: Vec<String> = app
                    .chrome
                    .keymap
                    .shown_chords(actions[0])
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
    let height = rows(HELP_LEFT).max(rows(HELP_RIGHT)) + 2;
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
        Layout::horizontal([Constraint::Percentage(50), Constraint::Percentage(50)]).areas(inner);
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
    f.render_widget(Paragraph::new(column(HELP_LEFT, left_a.width)), left_a);
    f.render_widget(Paragraph::new(column(HELP_RIGHT, right_a.width)), right_a);
    // Record the drawn area for click hit-testing.
    if let Some(Overlay::Help(h)) = &mut app.modals.overlay {
        h.area = area;
    }
}

fn draw_settings_overlay(f: &mut Frame, app: &mut App, view: crate::app::SettingsView) {
    let th = app.chrome.theme;
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
    // Rows the modal spends on anything but settings: the tab
    // strip and its rule above the body, and a blank + hint +
    // keys + config path below it.
    const CHROME: u16 = 2 + 4;
    let want = rows.len() as u16 + CHROME + 2;
    let height = want.min(f.area().height.saturating_sub(2)).max(CHROME + 3);
    let area = centered_rect(f.area(), SETTINGS_W, height);
    let inner = render_modal_frame(f, area, " Settings ", th);

    let dim = Style::default().fg(th.dim);
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
    let first_row = append_settings_body_lines(
        &mut lines,
        app,
        &cfg,
        &rows,
        &view,
        tab,
        body_h,
        inner,
        project.as_ref(),
        th,
        dim,
    );

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
                .then(|| app.chrome.keymap.shadowed_by(view.selected))
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
    if let Some(Overlay::Settings(v)) = &mut app.modals.overlay {
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

#[allow(clippy::too_many_arguments)]
fn append_settings_body_lines(
    lines: &mut Vec<Line<'static>>,
    app: &App,
    cfg: &crate::config::Config,
    rows: &[crate::config::SettingsRow],
    view: &crate::app::SettingsView,
    tab: usize,
    body_h: usize,
    inner: Rect,
    project: Option<&(String, std::path::PathBuf)>,
    th: Theme,
    dim: Style,
) -> usize {
    // Same stateless follow-window the panels use, in row space: the selected
    // row stays on screen without any scroll state.
    let sel_row = rows
        .iter()
        .position(|r| r.index() == Some(view.selected))
        .unwrap_or(0);
    let first_row = (sel_row + 1).saturating_sub(body_h);
    let today = crate::config::today_days();
    let new_prefix = |kind: crate::config::SettingKind| {
        if kind.is_new(today) {
            crate::config::NEW_SETTING_PREFIX
        } else {
            ""
        }
    };
    let label_w = crate::config::tab_settings(tab)
        .iter()
        .map(|spec| new_prefix(spec.kind).len() + spec.label.chars().count() + 1)
        .fold(28, usize::max);
    let project_settings = project.map(|(_, path)| cfg.project(path));
    for row in rows.iter().skip(first_row).take(body_h) {
        append_settings_row(
            lines,
            app,
            cfg,
            row,
            view,
            tab,
            inner,
            project,
            project_settings.as_ref(),
            label_w,
            &new_prefix,
            th,
            dim,
        );
    }
    for _ in lines.len()..(body_h + 2) {
        lines.push(Line::from(""));
    }
    first_row
}

#[allow(clippy::too_many_arguments)]
fn append_settings_row(
    lines: &mut Vec<Line<'static>>,
    app: &App,
    cfg: &crate::config::Config,
    row: &crate::config::SettingsRow,
    view: &crate::app::SettingsView,
    tab: usize,
    inner: Rect,
    project: Option<&(String, std::path::PathBuf)>,
    project_settings: Option<&crate::config::ProjectSettings>,
    label_w: usize,
    new_prefix: &impl Fn(crate::config::SettingKind) -> &'static str,
    th: Theme,
    dim: Style,
) {
    match row {
        crate::config::SettingsRow::Blank => lines.push(Line::from("")),
        crate::config::SettingsRow::Header(title) => lines.push(Line::from(Span::styled(
            format!(" {title}"),
            Style::default().fg(th.muted).add_modifier(Modifier::BOLD),
        ))),
        crate::config::SettingsRow::Project => match project {
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
        crate::config::SettingsRow::Setting(i) => append_setting_value_row(
            lines,
            cfg,
            *i,
            view,
            tab,
            inner,
            project_settings,
            label_w,
            new_prefix,
            th,
        ),
        crate::config::SettingsRow::Hotkey(i) => append_hotkey_row(lines, app, *i, view, th),
    }
}

#[allow(clippy::too_many_arguments)]
fn append_setting_value_row(
    lines: &mut Vec<Line<'static>>,
    cfg: &crate::config::Config,
    i: usize,
    view: &crate::app::SettingsView,
    tab: usize,
    inner: Rect,
    project_settings: Option<&crate::config::ProjectSettings>,
    label_w: usize,
    new_prefix: &impl Fn(crate::config::SettingKind) -> &'static str,
    th: Theme,
) {
    let (label, value, prefix) = if tab == crate::config::agents_tab() {
        match crate::config::AGENTS_HEAD.get(i) {
            Some(spec) => (
                spec.label.to_string(),
                cfg.value_label(spec.kind),
                new_prefix(spec.kind),
            ),
            None => {
                let (id, field) = cfg
                    .agent_row(i)
                    .expect("settings_rows indexes the Agents tab's harness rows");
                (field.label().to_string(), cfg.agent_value(&id, field), "")
            }
        }
    } else {
        let spec =
            crate::config::setting_at(tab, i).expect("settings_rows indexes this tab's settings");
        let value = if spec.kind.is_project() {
            project_settings
                .map(|s| s.value_label(spec.kind))
                .unwrap_or_else(|| "n/a".into())
        } else {
            cfg.value_label(spec.kind)
        };
        (spec.label.to_string(), value, new_prefix(spec.kind))
    };
    let selected = i == view.selected && !view.on_tabs;
    let mut label_style = Style::default();
    let mut value_style = Style::default().fg(th.accent);
    if selected {
        label_style = label_style.bg(th.sel_bg).add_modifier(Modifier::BOLD);
        value_style = value_style.bg(th.sel_bg).add_modifier(Modifier::BOLD);
    }
    let room = (inner.width as usize).saturating_sub(3 + label_w + 2);
    let mut prefix_style = Style::default().fg(th.ok);
    if selected {
        prefix_style = prefix_style.bg(th.sel_bg).add_modifier(Modifier::BOLD);
    }
    let label_room = label_w - prefix.len();
    lines.push(Line::from(vec![
        Span::styled("   ", label_style),
        Span::styled(prefix, prefix_style),
        Span::styled(format!("{label:<label_room$}"), label_style),
        Span::styled(format!("[{}]", truncate(&value, room)), value_style),
    ]));
}

fn append_hotkey_row(
    lines: &mut Vec<Line<'static>>,
    app: &App,
    i: usize,
    view: &crate::app::SettingsView,
    th: Theme,
) {
    let spec = crate::keymap::spec_at(i).expect("settings_rows indexes the action table");
    let selected = i == view.selected && !view.on_tabs;
    let value = if selected && view.capturing() {
        "press a key…".to_string()
    } else {
        app.chrome.keymap.display_at(i)
    };
    let reach = app.chrome.keymap.reach_at(i);
    let ambiguous = app.chrome.keymap.is_ambiguous(i);
    let mut label_style = Style::default();
    let mut value_style = Style::default().fg(if reach.is_fine() && !ambiguous {
        th.accent
    } else {
        th.warn
    });
    if selected {
        label_style = label_style.bg(th.sel_bg).add_modifier(Modifier::BOLD);
        value_style = value_style.bg(th.sel_bg).add_modifier(Modifier::BOLD);
    }
    let flag = match (ambiguous, reach) {
        (true, _) | (_, crate::keymap::Reach::Blocked) => "✗",
        (_, crate::keymap::Reach::Risky) => "⚠",
        _ => " ",
    };
    lines.push(Line::from(vec![
        Span::styled(format!("   {:<28}", spec.label), label_style),
        Span::styled(format!("{value:<18}"), value_style),
        Span::styled(flag.to_string(), Style::default().fg(th.warn)),
    ]));
}

struct MetricsRow {
    name: String,
    context: String,
    /// None = a group header, which is no one process.
    pid: Option<u32>,
    procs: u32,
    bytes: u64,
    /// None = not openable: nebula's own processes, a group header, or a
    /// pool spare (nothing to open until a CreateAgent adopts it).
    sref: Option<SessionRef>,
}

type MetricsKindTotals = std::collections::BTreeMap<&'static str, (u32, u32, u64)>;

fn metrics_rows(
    app: &App,
    view: &crate::app::MetricsView,
) -> (Vec<MetricsRow>, MetricsKindTotals, u64) {
    let mut rows: Vec<MetricsRow> = Vec::new();
    let mut spares: Vec<MetricsRow> = Vec::new();
    let mut kinds: MetricsKindTotals = std::collections::BTreeMap::new();
    let mut sessions_total: u64 = 0;
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

    let Some(snap) = &view.snapshot else {
        return (rows, kinds, sessions_total);
    };
    for m in &snap.sessions {
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
            spares.push(MetricsRow {
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
                let context = term.map(|t| wt_context(&t.worktree_id)).unwrap_or_default();
                (name, context, "shells")
            }
        };
        let entry = kinds.entry(kind).or_default();
        entry.0 += 1;
        entry.1 += m.procs;
        entry.2 += m.rss_bytes;
        sessions_total += m.rss_bytes;
        rows.push(MetricsRow {
            name,
            context,
            pid: Some(m.pid),
            procs: m.procs,
            bytes: m.rss_bytes,
            sref: Some(m.session.clone()),
        });
    }
    rows.sort_by_key(|row| std::cmp::Reverse(row.bytes));
    append_warm_spares(&mut rows, spares);
    rows.push(MetricsRow {
        name: "nebula daemon".into(),
        context: String::new(),
        pid: Some(snap.daemon_pid),
        procs: 1,
        bytes: snap.daemon_rss_bytes,
        sref: None,
    });
    rows.push(MetricsRow {
        name: "nebula ui (this window)".into(),
        context: String::new(),
        pid: Some(std::process::id()),
        procs: 1,
        bytes: view.client_rss_bytes,
        sref: None,
    });
    (rows, kinds, sessions_total)
}

fn append_warm_spares(rows: &mut Vec<MetricsRow>, mut spares: Vec<MetricsRow>) {
    if spares.is_empty() {
        return;
    }
    spares.sort_by_key(|row| std::cmp::Reverse(row.bytes));
    let count = spares.len();
    rows.push(MetricsRow {
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

fn draw_metrics_overlay(f: &mut Frame, app: &mut App, view: crate::app::MetricsView) {
    let th = app.chrome.theme;
    let (rows, kinds, sessions_total) = metrics_rows(app, &view);

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
            let mut detail = format!("{n} {unit}{} · {procs} proc{}", plural(*n), plural(*procs));
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
    if let Some(Overlay::Metrics(v)) = &mut app.modals.overlay {
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

fn draw_diff_overlay(f: &mut Frame, app: &mut App, view: crate::app::DiffView) {
    let th = app.chrome.theme;
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

    let list_inner = draw_diff_file_list(f, &view, files_a, th);

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
    // Side by side when it fits; a narrow pane reads the unified diff
    // instead.
    let split = view
        .split_rows
        .as_deref()
        .filter(|_| diff_inner.width >= crate::app::MIN_SPLIT_W);
    let total = split.map_or(view.diff_line_count, <[_]>::len);
    let max_scroll = (total as u16).saturating_sub(diff_inner.height.max(1));
    let scroll = view.scroll.min(max_scroll);
    if max_scroll > 0 {
        block = block.title_bottom(
            Line::from(Span::styled(
                format!(" {}/{} ", scroll + 1, total),
                Style::default().fg(th.dim),
            ))
            .right_aligned(),
        );
    }
    f.render_widget(block, diff_a);
    if let Some(rows) = split {
        draw_split_diff(f, rows, scroll, diff_inner, th);
    } else {
        // Only the rows in view are styled: a diff runs to 20 000
        // lines, and building a `Line` for each of them on every frame
        // was most of what scrolling a large one cost.
        let lines: Vec<Line> = view
            .diff
            .lines()
            .skip(scroll as usize)
            .take(diff_inner.height as usize)
            .map(|l| Line::from(Span::styled(l.to_string(), diff_line_style(l, th))))
            .collect();
        f.render_widget(Paragraph::new(lines), diff_inner);
    }

    // Write-back (draw works on a clone): page size for key paging,
    // scroll re-clamped so resizes never strand the view.
    if let Some(Overlay::Diff(v)) = &mut app.modals.overlay {
        v.view_height = diff_inner.height;
        v.scroll = scroll;
        v.list_area = list_inner;
        v.area = area;
        v.files_width = files_w;
        v.split_shown = split.is_some();
    }
}

/// A unified diff line's color: added green, removed red, hunk headers in
/// the accent, file headers dimmed.
fn diff_line_style(line: &str, th: Theme) -> Style {
    match classify_diff_line(line) {
        DiffLineKind::Add => Style::default().fg(th.ok),
        DiffLineKind::Remove => Style::default().fg(th.err),
        DiffLineKind::Hunk => Style::default().fg(th.accent),
        DiffLineKind::Header => Style::default().fg(th.dim),
        DiffLineKind::Context => Style::default(),
    }
}

/// A diff side by side (`git_diff::split_rows`): the old file left and the
/// new one right, each line numbered, a removed line facing what replaced
/// it. Only the rows in view are built.
fn draw_split_diff(
    f: &mut Frame,
    rows: &[crate::git_diff::SplitRow],
    scroll: u16,
    area: Rect,
    th: Theme,
) {
    use crate::git_diff::SplitRow;
    let widest = rows
        .iter()
        .filter_map(|row| match row {
            SplitRow::Pair { left, right, .. } => Some(
                left.as_ref()
                    .map_or(0, |l| l.0)
                    .max(right.as_ref().map_or(0, |r| r.0)),
            ),
            SplitRow::Note(..) => None,
        })
        .max()
        .unwrap_or(0);
    let num_w = widest.to_string().len();
    let [left_a, sep_a, right_a] = Layout::horizontal([
        Constraint::Length(area.width.saturating_sub(1) / 2),
        Constraint::Length(1),
        Constraint::Min(0),
    ])
    .areas(area);
    let dim = Style::default().fg(th.dim);
    let cell = |side: &Option<(u32, String)>, style: Style| match side {
        Some((n, text)) => Line::from(vec![
            Span::styled(format!("{n:>num_w$} "), dim),
            Span::styled(text.replace('\t', "    "), style),
        ]),
        None => Line::default(),
    };
    let (mut left, mut right) = (Vec::new(), Vec::new());
    for row in rows.iter().skip(scroll as usize).take(area.height as usize) {
        match row {
            SplitRow::Note(text, kind) => {
                let style = if *kind == DiffLineKind::Hunk {
                    Style::default().fg(th.accent)
                } else {
                    dim
                };
                left.push(Line::from(Span::styled(text.clone(), style)));
                right.push(Line::default());
            }
            SplitRow::Pair {
                left: old,
                right: new,
                changed,
            } => {
                let (old_style, new_style) = if *changed {
                    (Style::default().fg(th.err), Style::default().fg(th.ok))
                } else {
                    (Style::default(), Style::default())
                };
                left.push(cell(old, old_style));
                right.push(cell(new, new_style));
            }
        }
    }
    let seam = vec![Line::from(Span::styled("│", dim)); area.height as usize];
    f.render_widget(Paragraph::new(left), left_a);
    f.render_widget(Paragraph::new(seam), sep_a);
    f.render_widget(Paragraph::new(right), right_a);
}

fn draw_diff_file_list(
    f: &mut Frame,
    view: &crate::app::DiffView,
    files_a: Rect,
    th: Theme,
) -> Rect {
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
                let budget =
                    (list_inner.width as usize).saturating_sub(5 + indent.chars().count() + 2);
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

    list_inner
}

fn draw_palette_overlay(f: &mut Frame, app: &mut App, palette: crate::palette::Palette) {
    let th = app.chrome.theme;
    let area = centered_rect(f.area(), PALETTE_SIZE.0, PALETTE_SIZE.1);
    let title = if palette.list.query.is_empty() {
        " Jump to ".to_string()
    } else {
        format!(
            " Jump to ({}/{}) ",
            palette.hits(),
            palette.list.items.len()
        )
    };
    let inner = render_modal_frame(f, area, title, th);

    // First row: the always-on fuzzy query input.
    if let Some(query_area) = row_rect(inner, 0) {
        let line = search_line(&palette.list.query, "type to search…", query_area, th);
        f.render_widget(Paragraph::new(line), query_area);
    }
    let list_inner = below_first_row(inner);

    if palette.list.matches.is_empty() {
        empty_list_row(f, list_inner, NO_MATCHES, th);
    }
    let start = palette.list.window_start(list_inner.height as usize);
    for (row, (i, m)) in palette
        .list
        .matches
        .iter()
        .enumerate()
        .skip(start)
        .enumerate()
    {
        let Some(row_area) = row_rect(list_inner, row) else {
            break;
        };
        let item = &palette.list.items[m.item];
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
            sweep_ramp(status, false, th, app.chrome.animations),
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
        render_row(f, row_area, spans, i == palette.list.cursor, true, th);
    }

    // Write-back (draw works on a clone): rects for mouse
    // hit-testing.
    if let Some(Overlay::Palette(p)) = &mut app.modals.overlay {
        p.area = area;
        p.list.list_area = list_inner;
        p.list.sync_scroll(list_inner.height as usize);
    }
}

fn draw_files_overlay(f: &mut Frame, app: &mut App, finder: crate::app::FileFinder) {
    let th = app.chrome.theme;
    let area = centered_rect(f.area(), FILES_SIZE.0, FILES_SIZE.1);
    // No count to show until the listing lands: `(0/0)` reads as
    // "no files", which is not what is known yet.
    let title = if finder.listing.is_some() {
        format!(" Find file — {} (listing…) ", finder.branch)
    } else if finder.list.query.is_empty() {
        format!(
            " Find file — {} ({}) ",
            finder.branch,
            finder.list.items.len()
        )
    } else {
        format!(
            " Find file — {} ({}/{}) ",
            finder.branch,
            finder.list.matches.len(),
            finder.list.items.len()
        )
    };
    let inner = render_modal_frame(f, area, title, th);

    // First row: the always-on fuzzy query input.
    if let Some(query_area) = row_rect(inner, 0) {
        let line = search_line(&finder.list.query, "type to filter…", query_area, th);
        f.render_widget(Paragraph::new(line), query_area);
    }
    let list_inner = below_first_row(inner);

    if finder.listing.is_some() {
        empty_list_row(f, list_inner, "listing files…", th);
    } else if finder.list.matches.is_empty() {
        empty_list_row(f, list_inner, NO_MATCHES, th);
    }
    let start = finder.list.window_start(list_inner.height as usize);
    for (row, (i, m)) in finder
        .list
        .matches
        .iter()
        .enumerate()
        .skip(start)
        .enumerate()
    {
        let Some(row_area) = row_rect(list_inner, row) else {
            break;
        };
        let path = &finder.list.items[m.item];
        let budget = (list_inner.width as usize).saturating_sub(2);
        let shown = truncate(path, budget);
        let positions = visible_positions(&m.positions, &shown, path);
        let mut spans = vec![Span::raw(" ")];
        spans.extend(fuzzy_highlight_spans(&shown, positions, th));
        render_row(f, row_area, spans, i == finder.list.cursor, true, th);
    }

    // Write-back (draw works on a clone): rects for mouse
    // hit-testing.
    if let Some(Overlay::Files(fin)) = &mut app.modals.overlay {
        fin.area = area;
        fin.list.list_area = list_inner;
        fin.list.sync_scroll(list_inner.height as usize);
    }
}

fn draw_grep_overlay(f: &mut Frame, app: &mut App, view: crate::app::GrepView) {
    let th = app.chrome.theme;
    let area = centered_rect_pct(f.area(), GREP_MODAL_PCT.0, GREP_MODAL_PCT.1);
    let title = if view.list.query.chars().count() < crate::grep_search::MIN_QUERY_LEN {
        format!(" Find in files — {} ", view.branch)
    } else if view.waiting.is_some() {
        format!(" Find in files — {} (searching…) ", view.branch)
    } else if view.truncated {
        format!(
            " Find in files — {} ({}+ hits) ",
            view.branch,
            view.list.items.len()
        )
    } else {
        format!(
            " Find in files — {} ({} hits) ",
            view.branch,
            view.list.items.len()
        )
    };
    let inner = render_modal_frame(f, area, title, th);

    // First row: the always-live grep query.
    if let Some(query_area) = row_rect(inner, 0) {
        let line = search_line(&view.list.query, "type to search…", query_area, th);
        f.render_widget(Paragraph::new(line), query_area);
    }
    let list_inner = below_first_row(inner);

    // Placeholder row: error, too-short query, or an empty result.
    let placeholder = if let Some(err) = &view.error {
        Some(Span::styled(err.clone(), Style::default().fg(th.err)))
    } else if view.list.query.chars().count() < crate::grep_search::MIN_QUERY_LEN {
        Some(Span::styled(
            format!(
                "type at least {} characters to search",
                crate::grep_search::MIN_QUERY_LEN
            ),
            Style::default().fg(th.dim),
        ))
    } else if view.list.items.is_empty() && view.waiting.is_none() {
        Some(Span::styled(NO_MATCHES, Style::default().fg(th.dim)))
    } else {
        None
    };
    if let (Some(span), Some(row_area)) = (placeholder, row_rect(list_inner, 0)) {
        f.render_widget(Paragraph::new(span), row_area);
    }

    let start = view.list.window_start(list_inner.height as usize);
    for (row, (i, hit)) in view.list.items.iter().enumerate().skip(start).enumerate() {
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
        render_row(f, row_area, spans, i == view.list.cursor, true, th);
    }

    // Write-back (draw works on a clone): rects for mouse
    // hit-testing.
    if let Some(Overlay::Grep(v)) = &mut app.modals.overlay {
        v.area = area;
        v.list.list_area = list_inner;
        v.list.sync_scroll(list_inner.height as usize);
    }
}

fn draw_hosts_overlay(f: &mut Frame, app: &mut App, view: crate::app::HostsView) {
    let th = app.chrome.theme;
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
    let now = nebula_core::clock::now_ms();
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
    if let Some(Overlay::Hosts(v)) = &mut app.modals.overlay {
        v.area = area;
        v.list_area = inner;
        v.selected = selected;
    }
}

fn draw_agent_presets_overlay(
    f: &mut Frame,
    app: &mut App,
    view: crate::preset_overlays::AgentPresetsView,
) {
    let th = app.chrome.theme;
    crate::preset_overlays::draw_list(f, app, &view, th);
}

fn draw_agent_preset_editor_overlay(
    f: &mut Frame,
    app: &mut App,
    editor: crate::preset_overlays::AgentPresetEditor,
) {
    let th = app.chrome.theme;
    crate::preset_overlays::draw_editor(f, app, &editor, th)
}

fn draw_issues_overlay(f: &mut Frame, app: &mut App, view: crate::issues::IssuesView) {
    let th = app.chrome.theme;
    crate::issues::draw(f, app, &view, th, false);
}

fn draw_pull_requests_overlay(
    f: &mut Frame,
    app: &mut App,
    view: crate::pr_modal::PullRequestsView,
) {
    let th = app.chrome.theme;
    crate::pr_modal::draw(f, app, &view, th, false);
}

fn draw_branch_switch_overlay(
    f: &mut Frame,
    app: &mut App,
    view: crate::branch_switch::BranchSwitchView,
) {
    let th = app.chrome.theme;
    crate::branch_switch::draw(f, app, &view, th);
}

fn draw_file_tabs_overlay(f: &mut Frame, app: &mut App, mut view: crate::file_tabs::FileTabsView) {
    let th = app.chrome.theme;
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
    let editing = app.pane.vim.as_ref().is_some_and(|v| v.embedded);
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
    if let Some(Overlay::FileTabs(v)) = &mut app.modals.overlay {
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

fn draw_tree_overlay(f: &mut Frame, app: &mut App, mut view: crate::tree_browser::TreeBrowser) {
    let th = app.chrome.theme;
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
    } else if view.list.query.is_empty() {
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
        let line = search_line(&view.list.query, "type to filter…", filter_area, th);
        f.render_widget(Paragraph::new(line), filter_area);
    }
    let list_inner = below_first_row(tree_inner);

    if view.listing.is_some() {
        empty_list_row(f, list_inner, "listing files…", th);
    } else if view.list.matches.is_empty() {
        empty_list_row(f, list_inner, NO_MATCHES, th);
    }
    let start = view.list.window_start(list_inner.height as usize);
    for (row, (i, m)) in view.list.matches.iter().enumerate().skip(start).enumerate() {
        let Some(row_area) = row_rect(list_inner, row) else {
            break;
        };
        let r = &view.list.items[m.item];
        let node = &view.nodes[r.node];
        let indent = "  ".repeat(node.depth);
        // Directories fold; a live filter forces them all open.
        let marker = if !node.is_dir {
            "  "
        } else if !view.list.query.is_empty() || view.expanded[r.node] {
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
        render_row(f, row_area, spans, i == view.list.cursor, true, th);
    }

    // Right: the selected node's preview, syntax-highlighted and
    // scrolled — or the embedded editor, which draw_vim paints into
    // this pane after us.
    let editing = app.pane.vim.as_ref().is_some_and(|v| v.embedded);
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
    let max_scroll =
        (line_count.min(u16::MAX as usize) as u16).saturating_sub(preview_inner.height.max(1));
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
    if let Some(Overlay::Tree(v)) = &mut app.modals.overlay {
        v.view_height = preview_inner.height;
        v.scroll = scroll;
        v.preview_line_count = line_count;
        if rendered.is_some() {
            v.rendered = rendered;
        }
        v.list.list_area = list_inner;
        v.list.sync_scroll(list_inner.height as usize);
        v.preview_area = preview_inner;
        v.area = area;
        v.files_width = files_w;
    }
}
