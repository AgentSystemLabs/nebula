use super::*;

struct FooterHint {
    hints: Span<'static>,
    key_bar: Option<Vec<(String, &'static str)>>,
}

pub(super) fn draw_footer_bar(f: &mut Frame, app: &mut App, area: Rect) {
    // `area` includes the blank padding row; the bar itself is its last row.
    let area = Rect {
        y: area.y + area.height.saturating_sub(1),
        height: area.height.min(1),
        ..area
    };
    let th = app.chrome.theme;
    let FooterHint { hints, key_bar } = footer_hint(app);
    let mut spans = footer_left_context(app, th);
    append_footer_hint(&mut spans, hints, key_bar, th);
    render_footer_left(f, app, area, spans, th);
    render_footer_usage(f, app, area, th);
}

fn footer_hint(app: &App) -> FooterHint {
    let th = app.chrome.theme;
    if let Some(flash) = &app.chrome.flash {
        return span_hint(Span::styled(flash.clone(), Style::default().fg(th.warn)));
    }
    if app.pane.vim.is_some() {
        return dim_hint(":wq / :q to finish  Ctrl+Q: force close", th);
    }
    if let Some(overlay) = &app.modals.overlay {
        return overlay_footer_hint(app, overlay, th);
    }
    if app.splash_showing() {
        return splash_footer_hint(app, th);
    }
    if app.launcher_grid()
        && app.nav.focus != Focus::Terminal
        && app.launcher.launcher_tab_cursor.is_some()
    {
        return launcher_tabs_footer_hint(app, th);
    }
    if app.launcher_grid() && app.nav.focus != Focus::Terminal {
        return launcher_grid_footer_hint(app, th);
    }
    pane_footer_hint(app, th)
}

fn span_hint(hints: Span<'static>) -> FooterHint {
    FooterHint {
        hints,
        key_bar: None,
    }
}

fn dim_hint(text: impl Into<std::borrow::Cow<'static, str>>, th: Theme) -> FooterHint {
    span_hint(Span::styled(text, Style::default().fg(th.dim)))
}

fn overlay_footer_hint(app: &App, overlay: &Overlay, th: Theme) -> FooterHint {
    match overlay {
        Overlay::Grep(view) => dim_hint(
            format!(
                "type: search  ↑/↓: move  Enter: edit in {}  Ctrl+u: clear  Esc: clear/close",
                editor_name(&view.editor)
            ),
            th,
        ),
        Overlay::Diff(view) => dim_hint(
            if view.tree.is_some() {
                "type: filter  ↑/↓: move  ←/→: fold  ⇧↑/↓: scroll  Ctrl+d/u: half list  Ctrl+t: flat list  Ctrl+u: clear filter  Esc: clear/close"
            } else {
                "type: filter  ↑/↓: file  ⇧↑/↓: scroll  Ctrl+d/u: half list  Ctrl+t: tree  Ctrl+u: clear filter  Esc: clear/close"
            },
            th,
        ),
        Overlay::FileTabs(view) => dim_hint(
            file_tabs_keys_hint(view, app.pane.vim.as_ref().is_some_and(|v| v.embedded)),
            th,
        ),
        Overlay::Tree(view) => {
            let md = markdown_toggle_hint("Ctrl+r", view.markdown, view.pretty);
            dim_hint(
                format!(
                    "type: filter  ↑/↓: move  ←/→: fold  Enter: open/edit  ⇧↑/↓: scroll{md}  \
                     Ctrl+u: clear filter  Esc: clear/close"
                ),
                th,
            )
        }
        Overlay::Files(view) => {
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
            dim_hint(
                format!(
                    "type: search  ↑/↓: move  {enter}  Ctrl+y: copy path  Ctrl+u: clear  Esc: clear/close"
                ),
                th,
            )
        }
        Overlay::Palette(_) => dim_hint(
            "type: search  ↑/↓: move  Enter: open  Ctrl+u: clear  Esc: clear/close",
            th,
        ),
        Overlay::Settings(view) => dim_hint(settings_keys_hint(view), th),
        Overlay::Metrics(_) => dim_hint(
            "↑/↓: select  Enter: open session  Esc: close  (refreshes every 2s)",
            th,
        ),
        Overlay::Hosts(view) => dim_hint(
            if view.input.is_some() {
                "type user@host [dir]  Enter: connect (restarts nebula over ssh)  Esc: cancel"
            } else {
                "↑/↓: select  Enter: connect (restarts nebula over ssh)  a: new  d: remove  Esc: close"
            },
            th,
        ),
        Overlay::AgentPresets(view) => dim_hint(crate::preset_overlays::footer_hint(view), th),
        Overlay::AgentPresetEditor(_) => dim_hint(
            "Tab/↑↓: next field  ←/→: cycle  Shift+Enter/^J: newline  Enter: save  Esc: back to list",
            th,
        ),
        Overlay::Issues(view) => dim_hint(crate::issues::footer_hint(view), th),
        Overlay::PullRequests(_) => dim_hint(crate::pr_modal::footer_hint(), th),
        Overlay::BranchSwitch(view) => dim_hint(crate::branch_switch::footer_hint(view), th),
        Overlay::Menu(m) if m.is_project_picker() => dim_hint(
            "type: filter  Enter: open the project  ↑/↓: move  Esc: close",
            th,
        ),
        Overlay::Menu(m) => menu_footer_hint(m)
            .map(|hint| dim_hint(hint, th))
            .unwrap_or_else(|| dim_hint("Esc: close  Enter: confirm", th)),
        Overlay::Prompt(p) if matches!(p.kind, crate::app::PromptKind::QuickPrompt(_)) => {
            quick_prompt_footer_hint(app, th)
        }
        Overlay::ProjectPicker(_) => dim_hint(
            "type: filter projects  ↑/↓: move  Enter: aim the box there  Esc: clear/back to the box",
            th,
        ),
        _ => dim_hint("Esc: close  Enter: confirm", th),
    }
}

fn quick_prompt_footer_hint(app: &App, th: Theme) -> FooterHint {
    // `^P`, `^O`, `Tab` and `^N` are on the box itself now, each
    // beside the thing it changes — a third copy down here was most
    // of what made this screen read as a wall of chords. A box
    // standing on a modal goes back to it.
    let hint = match app
        .modals
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
    dim_hint(hint, th)
}

fn splash_footer_hint(app: &App, th: Theme) -> FooterHint {
    // The splash covers the panels, so every panel hotkey is dead here.
    // List only what actually fires.
    let k = |a| key_hint(app, a);
    // Launched inside a repo: Enter opens it, and `o` is for any other folder.
    let here = app
        .launch_repo_name()
        .map(|name| format!("{}: open {name}  ", k(Action::Activate)))
        .unwrap_or_default();
    dim_hint(
        format!(
            "{here}{}: open {}folder  {}: ssh host  {}: settings  {}: help  {}: quit",
            k(Action::AddProject),
            if here.is_empty() { "a " } else { "another " },
            k(Action::Hosts),
            k(Action::Settings),
            k(Action::Help),
            k(Action::Quit),
        ),
        th,
    )
}

fn launcher_tabs_footer_hint(app: &App, th: Theme) -> FooterHint {
    // The LAUNCHER VIEW's PROJECT TABS holding the keys (`k`,`k` off the top
    // row of cards): walking the header's cursor, switching the grid as it
    // goes, and the ways back down.
    let k = |a| key_hint(app, a);
    let down = k(Action::MoveDown);
    dim_hint(
        format!(
            "{}{}: switch project  {} or {down}{down}: into its cards  {}: close tab  esc: back to the cards  {}: help  {}: quit",
            k(Action::FocusLeft),
            k(Action::FocusRight),
            k(Action::Activate),
            k(Action::CloseProjectTab),
            k(Action::Help),
            k(Action::Quit),
        ),
        th,
    )
}

fn launcher_grid_footer_hint(app: &App, th: Theme) -> FooterHint {
    let k = |a| key_hint(app, a);
    let move_keys = format!(
        "{}{}{}{}",
        k(Action::FocusLeft),
        k(Action::MoveDown),
        k(Action::MoveUp),
        k(Action::FocusRight),
    );
    if app.launcher.launcher_nested && !app.launcher.show_archived {
        let walk = format!("{}{}", k(Action::MoveDown), k(Action::MoveUp));
        return FooterHint {
            hints: Span::raw(""),
            key_bar: Some(vec![
                (walk, "move"),
                (k(Action::Activate), "open"),
                (k(Action::QuickPrompt), "sub-prompt"),
            ]),
        };
    }
    let text = if app.launcher.show_archived {
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
    };
    dim_hint(text, th)
}

fn pane_footer_hint(app: &App, th: Theme) -> FooterHint {
    let k = |a| key_hint(app, a);
    let text = match app.nav.focus {
        Focus::Terminal if app.previewed_cloud().is_some() => format!(
            "{}: open in browser  {}: sessions",
            k(Action::Activate),
            k(Action::FocusLeft)
        ),
        Focus::Terminal if app.pane.term.as_ref().is_some_and(|t| t.exited) => {
            "session exited — Esc: back to sessions".to_string()
        }
        Focus::Terminal if app.pane.term_locked => locked_terminal_footer_hint(app),
        Focus::Terminal if app.pane.term.is_some() => format!(
            "{}: type into terminal  {}: sessions",
            k(Action::Activate),
            k(Action::FocusLeft)
        ),
        Focus::Terminal => "select a session and press Enter to attach".to_string(),
        Focus::Projects => format!(
            "{}/{}: add  {}: rename  {}: remove  {}: search  {}: help",
            k(Action::New),
            k(Action::AddProject),
            k(Action::Rename),
            k(Action::Delete),
            k(Action::Palette),
            k(Action::Help)
        ),
        Focus::Worktrees if app.selected_worktree_pr().is_some() => format!(
            "{}: new session  {}: preset  {}: open in browser  {}: diff  PgUp/PgDn: scroll  {}: refresh  {}: search  {}: help",
            k(Action::New),
            k(Action::AgentPresets),
            k(Action::Activate),
            k(Action::GitDiff),
            k(Action::RefreshPullRequests),
            k(Action::Palette),
            k(Action::Help)
        ),
        Focus::Worktrees if app.selected_worktree_issue().is_some() => format!(
            "{}: open in browser  {}: prompt  {}: preset  PgUp/PgDn: scroll  {}: search  {}: help",
            k(Action::Activate),
            k(Action::QuickPrompt),
            k(Action::AgentPresets),
            k(Action::Palette),
            k(Action::Help)
        ),
        Focus::Worktrees => worktree_footer_hint(app, &k),
        Focus::Sessions if app.selected_link().is_some_and(|row| row.id().is_none()) => format!(
            "{}: open in browser  {}: diff  PgUp/PgDn: scroll  {}: refresh  {}: help",
            k(Action::Activate),
            k(Action::GitDiff),
            k(Action::RefreshPullRequests),
            k(Action::Help)
        ),
        Focus::Sessions if app.selected_link().is_some() => format!(
            "{}: open in browser  {}: edit URL  {}: delete  {}: help",
            k(Action::Activate),
            k(Action::Rename),
            k(Action::Delete),
            k(Action::Help)
        ),
        Focus::Sessions if app.previewed_cloud().is_some() => format!(
            "{}: open in browser  {}: rename  {}: archive  {}: del  {}: help",
            k(Action::Activate),
            k(Action::Rename),
            k(Action::Archive),
            k(Action::Delete),
            k(Action::Help)
        ),
        Focus::Sessions => format!(
            "{}: focus  {}: agent  {}: presets  {}: terminal  {}: rename  {}: archive  {}: del  {}: help",
            k(Action::Activate),
            k(Action::New),
            k(Action::AgentPresets),
            k(Action::NewTerminal),
            k(Action::Rename),
            k(Action::Archive),
            k(Action::Delete),
            k(Action::Help)
        ),
    };
    dim_hint(text, th)
}

fn locked_terminal_footer_hint(app: &App) -> String {
    format!(
        "{}: {}  {}  ⌥click: open link",
        if app.launcher_grid() {
            app.chrome
                .keymap
                .chords(Action::ToggleLauncherPane)
                .iter()
                .find(|c| !crate::key_combo::is_text_key(c))
                .map(|c| c.display())
        } else {
            None
        }
        .or_else(|| app
            .chrome
            .keymap
            .first(Action::UnlockTerminal)
            .map(|c| c.display()))
        .unwrap_or_else(|| "^q".into()),
        if app.launcher_grid() {
            "back to the card"
        } else if app.launcher_active() && app.pane.collapsed {
            "normal size"
        } else {
            "sessions"
        },
        if app.child_mouse_mode().0 != vt100::MouseProtocolMode::None {
            "drag: to the app (⇧drag: terminal)"
        } else {
            "drag: select+copy"
        },
    )
}

fn worktree_footer_hint(app: &App, k: &impl Fn(Action) -> String) -> String {
    format!(
        "{}: new worktree  {}: presets  {}: {}  {}: open  {}: terminal  {}: delete  {}: refresh PRs  {}: search  {}: help",
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
        k(Action::Help)
    )
}

fn footer_left_context(app: &App, th: Theme) -> Vec<Span<'static>> {
    let mut spans = vec![Span::raw(" ")];
    if app.chrome.is_remote {
        spans.push(Span::styled(
            truncate(&app.chrome.hostname, 24),
            Style::default().fg(th.warn).add_modifier(Modifier::BOLD),
        ));
        spans.push(Span::styled("  ·  ", Style::default().fg(th.dim)));
    }
    if matches!(app.chrome.conn, ConnState::Disconnected) {
        spans.push(Span::styled("✗ disconnected", Style::default().fg(th.err)));
        spans.push(Span::styled("  ·  ", Style::default().fg(th.dim)));
    }
    let crumbs = breadcrumb(app);
    if !crumbs.is_empty() {
        spans.extend(crumbs);
        // No changed-file count here: it rides each card's branch, where
        // it reads as that checkout's (`launcher_view::draw_card`).
        spans.push(Span::styled("    ", Style::default()));
    }
    spans
}

fn append_footer_hint(
    spans: &mut Vec<Span<'static>>,
    hints: Span<'static>,
    key_bar: Option<Vec<(String, &'static str)>>,
    th: Theme,
) {
    match key_bar {
        Some(keys) => {
            for (i, (key, does)) in keys.into_iter().enumerate() {
                if i > 0 {
                    spans.push(Span::raw("  "));
                }
                spans.push(Span::styled(key, Style::default().fg(th.text)));
                spans.push(Span::styled(
                    format!(" {does}"),
                    Style::default().fg(th.muted),
                ));
            }
        }
        None => {
            let mut hints = hints;
            if hints.style.fg == Some(th.dim) {
                hints.style.fg = Some(th.muted);
            }
            spans.push(hints);
        }
    }
}

fn render_footer_left(
    f: &mut Frame,
    app: &App,
    area: Rect,
    mut spans: Vec<Span<'static>>,
    th: Theme,
) {
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
    prepend_version_plate(app, &mut spans, left.width, th);
    f.render_widget(Paragraph::new(Line::from(spans)), left);
}

fn prepend_version_plate(app: &App, spans: &mut Vec<Span<'static>>, width: u16, th: Theme) {
    let plate = format!("nebula v{}", env!("CARGO_PKG_VERSION"));
    let update = app
        .chrome
        .update_available
        .as_ref()
        .map(|v| format!(" ⇡ v{v}"));
    let plate_w = plate.chars().count()
        + update.as_ref().map_or(0, |u| u.chars().count())
        + "  ·  ".chars().count();
    let body_w: usize = spans.iter().map(|s| s.width()).sum();
    if app.chrome.flash.is_some() && body_w + plate_w > width as usize {
        return;
    }
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

fn render_footer_usage(f: &mut Frame, app: &mut App, area: Rect, th: Theme) {
    let Some(usage) = footer_usage(app) else {
        return;
    };
    let right_w = (usage.chars().count() as u16 + 2).min(area.width);
    let right = Rect {
        x: area.x + area.width.saturating_sub(right_w),
        width: right_w,
        ..area
    };
    // The readout is a button — a click opens the memory modal, as `⇧M` does.
    let span = Span::styled(usage, Style::default().fg(th.dim));
    let span = if app.launcher.hover_crumb == Some(HitTarget::FooterUsage) {
        span.style(
            Style::default()
                .fg(th.text)
                .add_modifier(Modifier::UNDERLINED),
        )
    } else {
        span
    };
    let width = (span.width() as u16).min(right.width);
    app.chrome.hits.push((
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
