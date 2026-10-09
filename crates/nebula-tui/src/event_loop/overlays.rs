//! Overlay keyboard dispatch.

use super::*;

pub(crate) fn handle_overlay_key(app: &mut App, key: KeyEvent, out: &mut Vec<ClientRequest>) {
    if matches!(&app.modals.overlay, Some(Overlay::Settings(_))) {
        handle_settings_key(app, key);
        return;
    }
    if matches!(&app.modals.overlay, Some(Overlay::FileTabs(_))) {
        crate::file_tabs::handle_key(app, key);
        return;
    }
    // The chord that dropped the PROJECT DROPDOWN (`⌘P`) puts it away
    // again, rather than landing in its type-ahead as a `p`.
    if matches!(&app.modals.overlay, Some(Overlay::Menu(m)) if m.is_project_picker())
        && drops_project_dropdown(app, &crate::keymap::KeyChord::from_event(&key))
    {
        app.modals.overlay = None;
        app.chrome.dirty = true;
        return;
    }
    let Some(overlay) = &app.modals.overlay else {
        return;
    };
    match overlay {
        Overlay::Settings(_) | Overlay::FileTabs(_) => {}
        Overlay::Help(_) => handle_help_key(app, key, out),
        Overlay::Metrics(_) => handle_metrics_key(app, key, out),
        Overlay::Hosts(_) => handle_hosts_key(app, key, out),
        Overlay::AgentPresets(_) => crate::preset_overlays::handle_list_key(app, key, out),
        Overlay::AgentPresetEditor(_) => crate::preset_overlays::handle_editor_key(app, key),
        Overlay::Issues(_) => crate::issues::handle_key(app, key, out),
        Overlay::PullRequests(_) => crate::pr_modal::handle_key(app, key, out),
        Overlay::BranchSwitch(_) => crate::branch_switch::handle_key(app, key),
        Overlay::Review(_) => {
            crate::review_modal::handle_key(app, key);
        }
        Overlay::ProjectPicker(_) => launcher::handle_picker_key(app, key),
        Overlay::Menu(_) => handle_menu_key(app, key, out),
        Overlay::Prompt(_) => handle_prompt_key(app, key, out),
        Overlay::Confirm(_) => handle_confirm_key(app, key, out),
        Overlay::Diff(_) => handle_diff_key(app, key, out),
        Overlay::Palette(_) => handle_palette_key(app, key, out),
        Overlay::Files(_) => handle_files_key(app, key, out),
        Overlay::Grep(_) => handle_grep_key(app, key, out),
        Overlay::Tree(_) => handle_tree_key(app, key, out),
    }
}

fn handle_help_key(app: &mut App, key: KeyEvent, out: &mut Vec<ClientRequest>) {
    let _ = &mut *out;
    let Some(overlay) = &mut app.modals.overlay else {
        return;
    };
    match overlay {
        Overlay::Help(_) => {
            if matches!(
                key.code,
                KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('?')
            ) {
                app.modals.overlay = None;
            }
        }
        _ => unreachable!("overlay dispatcher passed the wrong overlay"),
    }
}

fn handle_metrics_key(app: &mut App, key: KeyEvent, out: &mut Vec<ClientRequest>) {
    let _ = &mut *out;
    let Some(overlay) = &mut app.modals.overlay else {
        return;
    };
    match overlay {
        Overlay::Metrics(view) => match key.code {
            KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('M') => app.modals.overlay = None,
            KeyCode::Char('j') | KeyCode::Down => {
                view.selected = clamp_selection(view.selected as i64 + (1), view.rows.len());
            }
            KeyCode::Char('k') | KeyCode::Up => {
                view.selected = clamp_selection(view.selected as i64 + (-1), view.rows.len());
            }
            KeyCode::Enter => activate::metrics_row(app, out),
            _ => {}
        },
        _ => unreachable!("overlay dispatcher passed the wrong overlay"),
    }
}

fn handle_hosts_key(app: &mut App, key: KeyEvent, out: &mut Vec<ClientRequest>) {
    let _ = &mut *out;
    let Some(overlay) = &mut app.modals.overlay else {
        return;
    };
    match overlay {
        Overlay::Hosts(view) => {
            // Typing a new destination (`a`): the input owns printable keys.
            if let Some(input) = &mut view.input {
                match key.code {
                    KeyCode::Esc => view.input = None,
                    KeyCode::Enter => {
                        let entry = crate::hosts::parse_destination(input);
                        view.input = None;
                        // Nothing typed = cancel; otherwise connect exactly
                        // like `nebula ssh host [dir]` would.
                        if let Some(entry) = entry {
                            activate::host(app, entry);
                        }
                    }
                    // Everything else is the line editor's: arrows,
                    // ⌥←/⌥→ by word, the readline chords (text_input).
                    _ => {
                        input.handle_key(&key);
                    }
                }
                return;
            }
            match key.code {
                KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('h') => app.modals.overlay = None,
                KeyCode::Char('j') | KeyCode::Down => {
                    view.selected = clamp_selection(view.selected as i64 + (1), view.hosts.len());
                }
                KeyCode::Char('k') | KeyCode::Up => {
                    view.selected = clamp_selection(view.selected as i64 + (-1), view.hosts.len());
                }
                // A destination the list doesn't have yet — typed here so an
                // open nebula never needs a shell for `nebula ssh`.
                KeyCode::Char('a') | KeyCode::Char('n') => view.input = Some(TextInput::new()),
                // Enter hands off: quit the TUI, then the binary execs a
                // fresh `nebula ssh` at the entry (the daemon and its
                // sessions stay up).
                KeyCode::Enter => {
                    if let Some(entry) = view.hosts.get(view.selected).cloned() {
                        activate::host(app, entry);
                    }
                }
                // Forget the entry — no confirm, the next `nebula ssh` to it
                // just re-adds it.
                KeyCode::Char('d') | KeyCode::Char('x') | KeyCode::Backspace | KeyCode::Delete
                    if view.selected < view.hosts.len() =>
                {
                    let entry = view.hosts.remove(view.selected);
                    view.selected = clamp_selection(view.selected as i64, view.hosts.len());
                    crate::hosts::remove(&entry);
                }
                _ => {}
            }
        }
        _ => unreachable!("overlay dispatcher passed the wrong overlay"),
    }
}

fn handle_menu_key(app: &mut App, key: KeyEvent, out: &mut Vec<ClientRequest>) {
    let _ = &mut *out;
    let Some(overlay) = &mut app.modals.overlay else {
        return;
    };
    match overlay {
        Overlay::Menu(menu) => match key.code {
            // `?` (and `s` where no filter eats letters) on a row that
            // starts a session jumps to that harness's Agents section,
            // where its defaults live. Ahead of type-ahead on purpose:
            // `?` is a jump on these rows, never a filter letter (no row
            // contains one); `s` only jumps without a filter, since in
            // the model/effort submenus it narrows the list. `s` keeps
            // its global meaning — settings — so the picker agrees with
            // the panels.
            KeyCode::Char(c)
                if (c == '?' || (c == 's' && menu.filter.is_none()))
                    && menu.hovered_agent_kind().is_some() =>
            {
                let (kind, custom) = menu
                    .hovered_agent_kind()
                    .expect("the guard checked the hovered row");
                open_harness_settings(app, kind, custom);
            }
            // Type-ahead in the MODEL / EFFORT submenus: letters narrow the
            // rows (so ↑/↓ move there, not j/k), Backspace widens, and Esc
            // clears the text before it backs out. A letter no row matches
            // is refused, so the list never empties.
            KeyCode::Char(c)
                if menu.filter.is_some()
                    && c != ' '
                    && !key.modifiers.contains(KeyModifiers::CONTROL) =>
            {
                let query = format!("{}{c}", menu.filter_query());
                if !menu.type_filter(c) {
                    app.chrome.flash = Some(format!("no row matches '{query}'"));
                }
            }
            KeyCode::Backspace if menu.filter.is_some() => menu.pop_filter(),
            KeyCode::Esc if menu.has_filter_text() => {
                menu.set_filter("");
            }
            // Esc in a submenu backs out one level; at the top it closes —
            // unless the picker was opened from the QUICK PROMPT, which is
            // owed its box back with the text still in it. Only a box that
            // was up comes back: `n`'s NEW SESSION PICKER is reached with
            // none and closes as any menu does (`QuickReturn::from_box`).
            KeyCode::Esc => match menu.parent.take() {
                Some(parent) => *menu = *parent,
                None => match menu_quick_return(menu) {
                    Some(back) if back.from_box => {
                        app.modals.overlay = None;
                        crate::quick_prompt::reopen(app, back.launch, &back.text);
                    }
                    _ => app.modals.overlay = None,
                },
            },
            KeyCode::Char('j') | KeyCode::Down => {
                menu.hover = (menu.hover + 1).min(menu.items.len() - 1)
            }
            KeyCode::Char('k') | KeyCode::Up => menu.hover = menu.hover.saturating_sub(1),
            // → expands a row marked ▸ into its submenu; ← returns.
            KeyCode::Char('l') | KeyCode::Right => {
                if let Some(mut sub) = build_submenu(&menu.items[menu.hover]) {
                    sub.parent = Some(Box::new(menu.clone()));
                    *menu = sub;
                }
            }
            KeyCode::Char('h') | KeyCode::Left => {
                if let Some(parent) = menu.parent.take() {
                    *menu = *parent;
                }
            }
            // The NEW SESSION PICKER's Claude row — and the QUICK PROMPT
            // `Tab` picker's, for a box that can go to the cloud — owns
            // Tab as a launch-mode toggle, and so do the rows of the
            // Claude MODEL / EFFORT lists behind it (`→`, or the box's
            // `^O`). Every other menu leaves it untouched.
            KeyCode::Tab if menu.toggle_hovered_claude_cloud() => {}
            KeyCode::Enter => {
                let hover = menu.hover;
                activate::menu_row(app, hover, out);
            }
            _ => {}
        },
        _ => unreachable!("overlay dispatcher passed the wrong overlay"),
    }
}

fn handle_prompt_key(app: &mut App, key: KeyEvent, out: &mut Vec<ClientRequest>) {
    let _ = &mut *out;
    let Some(overlay) = &mut app.modals.overlay else {
        return;
    };
    match overlay {
        Overlay::Prompt(prompt) => match key.code {
            KeyCode::Esc => {
                // Abandoning a preset's task goes back to the list it came
                // from, on the same row, rather than to the panels.
                let back_to_presets = match &prompt.kind {
                    PromptKind::AgentPresetTask { worktree, preset } => {
                        Some((worktree.clone(), preset.name.clone()))
                    }
                    _ => None,
                };
                // A typed setting's prompt stood in for the overlay: Esc
                // keeps the old value and puts the overlay back on its row.
                let back_to_settings = matches!(prompt.kind, PromptKind::SettingText { .. });
                // The comment box stood in for the ISSUES MODAL: Esc puts
                // the modal back on its row, the comment unposted.
                let back_to_issues = match &prompt.kind {
                    PromptKind::IssueComment { view, .. } => Some(view.clone()),
                    _ => None,
                };
                // And the PULL REQUESTS MODAL's, the same way.
                let back_to_prs = match &prompt.kind {
                    PromptKind::PrComment { back, .. } => back.clone(),
                    _ => None,
                };
                // A QUICK PROMPT opened over either modal stood on it: Esc
                // takes the box off and leaves the modal on its row.
                let back_to_modal = match &prompt.kind {
                    PromptKind::QuickPrompt(launch) => launch.under.clone(),
                    _ => None,
                };
                // Esc on a box with something typed in it is the accident
                // that costs nothing: the box is parked as a DRAFT, and the
                // next QUICK PROMPT opens on it.
                let parked = crate::quick_prompt::draft_of(prompt);
                app.modals.overlay = None;
                if let Some(draft) = parked {
                    app.modals.quick_draft = Some(draft);
                }
                if back_to_settings {
                    reopen_settings(app);
                } else if let Some(view) = back_to_issues {
                    crate::issues::reopen(app, view);
                } else if let Some(view) = back_to_prs {
                    crate::pr_modal::reopen(app, *view);
                } else if let Some(under) = back_to_modal {
                    under.reopen(app);
                } else if let Some((worktree, name)) = back_to_presets {
                    let index = crate::agent_presets::load()
                        .iter()
                        .position(|p| p.name == name)
                        .unwrap_or(0);
                    crate::preset_overlays::reopen_agent_presets(app, worktree, index);
                }
            }
            // A line break in a task box — Shift+Enter, Option+Enter or
            // Ctrl+J, as in Claude Code's prompt — is the line editor's
            // (the `_` arm below); the guard keeps the send off it. Enter
            // on a highlighted listing row adds that directory; on the
            // input row it submits the typed path as before.
            KeyCode::Enter if !prompt.input.takes_newline(&key) => {
                let mut prompt = prompt.clone();
                if let Some(path) = prompt.hovered_path() {
                    prompt.input.set_text(path);
                }
                app.modals.overlay = None;
                submit_prompt(app, prompt, out);
            }
            // The LAUNCHER VIEW's box chords: `^P` the project, `^T` the
            // checkout in it, `^O` the model, and a `^N` that flips between
            // a fresh worktree and the project the box is aimed at (not the
            // one under the cursor).
            KeyCode::Char('p' | 'P' | 't' | 'T' | 'o' | 'O' | 'n' | 'N')
                if key.modifiers.contains(KeyModifiers::CONTROL)
                    && matches!(prompt.kind, PromptKind::QuickPrompt(_)) =>
            {
                if let PromptKind::QuickPrompt(launch) = &prompt.kind {
                    let (launch, input) = (launch.clone(), prompt.input.clone());
                    launcher::handle_box_key(app, &key, &launch, &input);
                }
            }
            // Tab / Shift+Tab retarget this one launch: the harness (and
            // its MODEL / EFFORT submenus) or a saved AGENT PRESET. Both
            // are free here — `TextInput` ignores them — and both come back
            // with the text. Only the QUICK PROMPT has anything to retarget.
            KeyCode::Tab if matches!(prompt.kind, PromptKind::QuickPrompt(_)) => {
                if let Some(back) = quick_return_of(prompt) {
                    crate::quick_prompt::open_launch_picker(app, back);
                }
            }
            KeyCode::BackTab if matches!(prompt.kind, PromptKind::QuickPrompt(_)) => {
                if let Some(back) = quick_return_of(prompt) {
                    crate::quick_prompt::open_preset_picker(app, back);
                }
            }
            // Ctrl+N flips the launch between the selected WORKTREE and a
            // fresh one — what `p` on the WORKTREES PANEL does, from any
            // panel, and the way back from there. Free here too: the line
            // editor leaves ^N alone. The box is rebuilt around the new
            // target with the text and the caret kept.
            KeyCode::Char('n' | 'N')
                if key.modifiers.contains(KeyModifiers::CONTROL)
                    && matches!(prompt.kind, PromptKind::QuickPrompt(_)) =>
            {
                if let Some(back) = quick_return_of(prompt) {
                    let input = prompt.input.clone();
                    crate::quick_prompt::toggle_new_worktree(app, back.launch, input);
                }
            }
            KeyCode::Tab if prompt.completes_paths() => {
                let home = nebula_core::env::home_dir();
                let result = crate::completion::complete_path(&prompt.input, home.as_deref());
                if let Some(completed) = result.completed {
                    prompt.input.set_text(completed);
                    prompt.refresh_dirs();
                }
            }
            KeyCode::Down if prompt.completes_paths() => prompt.move_hover(1),
            KeyCode::Up if prompt.completes_paths() => prompt.move_hover(-1),
            // ←/→ stay the path browser's dive/ascend here — the one
            // prompt where they are already spoken for. Caret motion in a
            // path is ⌥←/⌥→ (by segment), Ctrl+B/F, Home/End.
            KeyCode::Right if prompt.completes_paths() => {
                if let Some(i) = prompt.hover {
                    prompt.dive(i);
                }
            }
            KeyCode::Left if prompt.completes_paths() => prompt.ascend(),
            // The untouched "~/" prefill yields to an absolute (or
            // re-typed tilde) path — no clearing required first.
            KeyCode::Char(c)
                if prompt.completes_paths()
                    && prompt.input == "~/"
                    && (c == '/' || c == '~')
                    && !key
                        .modifiers
                        .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                prompt.input.set_text(c.to_string());
                prompt.refresh_dirs();
            }
            // Everything else is the line editor's (see text_input). ↑ on
            // a task box's top row and ↓ on its bottom one go on to the
            // very start and end of the text, as a text area on the web
            // does: the field leaves them over for a form to step fields
            // on, and this box has no other field.
            _ => {
                let edit = prompt.input.handle_key(&key);
                if edit.changed() {
                    prompt.refresh_dirs();
                } else if !edit.consumed() && prompt.is_multiline() {
                    match key.code {
                        KeyCode::Up => prompt.input.cursor_to_start(),
                        KeyCode::Down => prompt.input.cursor_to_end(),
                        _ => {}
                    }
                }
            }
        },
        _ => unreachable!("overlay dispatcher passed the wrong overlay"),
    }
}

fn handle_confirm_key(app: &mut App, key: KeyEvent, out: &mut Vec<ClientRequest>) {
    let _ = &mut *out;
    let Some(overlay) = &mut app.modals.overlay else {
        return;
    };
    match overlay {
        Overlay::Confirm(confirm) => match key.code {
            // The three-way dialog's "no": the card goes, the emptied
            // worktree stays. Esc below is the "cancel" that keeps both.
            KeyCode::Char('n')
                if matches!(
                    confirm.action,
                    PendingAction::ThenDeleteWorktree { offered: true, .. }
                ) =>
            {
                let PendingAction::ThenDeleteWorktree { first, .. } = confirm.action.clone() else {
                    unreachable!("guarded above");
                };
                app.modals.overlay = None;
                run_pending_action(app, *first, out);
            }
            KeyCode::Esc | KeyCode::Char('n') => {
                // Backing out lands where you were: a settings reset
                // reopens the overlay, a preset delete the presets list —
                // not the panels.
                let to_settings = matches!(confirm.action, PendingAction::ResetSettings);
                if let PendingAction::LocateProjectPath { id, .. } = &confirm.action {
                    app.launcher.dismissed_repath_projects.insert(id.clone());
                }
                let to_presets = match &confirm.action {
                    PendingAction::DeleteAgentPreset {
                        index,
                        worktree,
                        quick,
                    } => Some((*index, worktree.clone(), quick.clone())),
                    _ => None,
                };
                app.modals.overlay = None;
                if to_settings {
                    reopen_settings(app);
                } else if let Some((index, worktree, quick)) = to_presets {
                    crate::preset_overlays::reopen_presets_list(
                        app,
                        worktree,
                        quick.map(|back| *back),
                        index,
                    );
                }
            }
            KeyCode::Enter | KeyCode::Char('y') => {
                let action = confirm.action.clone();
                app.modals.overlay = None;
                run_pending_action(app, action, out);
            }
            // Ctrl+C twice always gets out. The gate exists to catch a
            // letter aimed at an agent, not to argue with someone who
            // pressed the terminal's own quit chord on purpose — and in
            // raw mode this dialog is the only thing standing in the way.
            KeyCode::Char('c')
                if key.modifiers.contains(KeyModifiers::CONTROL)
                    && confirm.action == PendingAction::Quit =>
            {
                app.modals.overlay = None;
                app.chrome.should_quit = true;
            }
            _ => {}
        },
        _ => unreachable!("overlay dispatcher passed the wrong overlay"),
    }
}

fn handle_diff_key(app: &mut App, key: KeyEvent, out: &mut Vec<ClientRequest>) {
    let _ = &mut *out;
    let Some(overlay) = &mut app.modals.overlay else {
        return;
    };
    match overlay {
        Overlay::Diff(view) => {
            let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
            let shift = key.modifiers.contains(KeyModifiers::SHIFT);
            // Ctrl+d/u walk the sidebar half its height, as in vim; the
            // diff pages on PgUp/PgDn.
            let half = (view.list_area.height / 2).max(1) as i64;
            let page = view.view_height.max(1) as i32;
            let at = view.side_cursor() as i64;
            match key.code {
                // Two-stage escape: an active filter is cleared before the
                // second Esc closes the modal.
                KeyCode::Esc if !view.filter.is_empty() => {
                    view.filter.clear();
                    activate::diff_filter_changed(view);
                }
                KeyCode::Esc => app.modals.overlay = None,
                KeyCode::Char('d') if ctrl => activate::diff_file(view, at + half),
                // Ctrl+u is the line editor's kill-to-start while something
                // is typed; only with an empty filter does it move.
                KeyCode::Char('u') if ctrl && view.filter.is_empty() => {
                    activate::diff_file(view, at - half)
                }
                // Ctrl+r toggles the reviewed ✓ on the selected file —
                // nebula-side bookkeeping only, no git state is touched.
                // Reviewed files sink to the bottom; marking advances to the
                // next file and unmarking to the next still-marked file, so
                // held Ctrl+r sweeps either way (see
                // `DiffView::toggle_reviewed`).
                KeyCode::Char('r') if ctrl => {
                    if let Some(changed) = view.toggle_reviewed() {
                        crate::review::store_marks(&view.root, &view.head_key, &view.reviewed);
                        if changed {
                            crate::git_diff::load_selected_diff(view);
                        }
                    }
                }
                // Ctrl+t flips the file list between flat paths and the
                // directory tree (`diff_tree`), the cursor staying on its
                // file; remembered for the next open, like the list's width.
                KeyCode::Char('t') if ctrl => {
                    activate::diff_tree_toggled(view);
                    app.modals.diff_tree = view.tree.is_some();
                }
                // Ctrl+s flips the diff side by side, or unified;
                // remembered for the next open, like the tree and width.
                KeyCode::Char('s') if ctrl => {
                    view.toggle_split();
                    app.modals.diff_split = view.split;
                }
                KeyCode::Down if shift => view.scroll_by(1),
                KeyCode::Up if shift => view.scroll_by(-1),
                KeyCode::Down => activate::diff_file(view, at + 1),
                KeyCode::Up => activate::diff_file(view, at - 1),
                // ->/<- unfold and fold what the cursor is on (a section, a
                // commit, a tree directory) or step in / out to the parent,
                // as in the TREE BROWSER; Enter flips it. On the flat
                // list's files all three stay the filter's.
                KeyCode::Right if view.folds_on_arrows() => activate::diff_fold(view, true),
                KeyCode::Left if view.folds_on_arrows() => activate::diff_fold(view, false),
                KeyCode::Enter if view.folds_on_arrows() => activate::diff_row(view, at),
                KeyCode::PageDown => view.scroll_by(page),
                KeyCode::PageUp => view.scroll_by(-page),
                KeyCode::Home => view.scroll = 0,
                KeyCode::End => view.scroll = view.max_scroll(),
                // Everything else feeds the always-on fuzzy filter, which
                // edits like a terminal line (see text_input).
                _ => {
                    if view.filter.handle_key(&key).changed() {
                        activate::diff_filter_changed(view);
                    }
                }
            }
        }
        _ => unreachable!("overlay dispatcher passed the wrong overlay"),
    }
}

fn handle_palette_key(app: &mut App, key: KeyEvent, out: &mut Vec<ClientRequest>) {
    let _ = &mut *out;
    let Some(overlay) = &mut app.modals.overlay else {
        return;
    };
    match overlay {
        Overlay::Palette(palette) => {
            let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
            let action = palette
                .list
                .handle_standard_key(&key, palette.list.list_area.height.max(1) as i64);
            match key.code {
                // Enter picks per the config setting; Ctrl+O always opens
                // (attach + terminal focus; the browser, for a pull
                // request), Ctrl+F only focuses the row.
                KeyCode::Enter => activate::palette_row(app, None, out),
                KeyCode::Char('o') if ctrl => {
                    activate::palette_row(app, Some(Landing::Attach), out)
                }
                KeyCode::Char('f') if ctrl => {
                    activate::palette_row(app, Some(Landing::FocusOnly), out)
                }
                _ => match action {
                    crate::filter_list::FilterListKey::Close => app.modals.overlay = None,
                    crate::filter_list::FilterListKey::Cleared
                    | crate::filter_list::FilterListKey::QueryChanged => palette.apply_filter(),
                    crate::filter_list::FilterListKey::Moved
                    | crate::filter_list::FilterListKey::None => {}
                },
            }
        }
        _ => unreachable!("overlay dispatcher passed the wrong overlay"),
    }
}

fn handle_files_key(app: &mut App, key: KeyEvent, out: &mut Vec<ClientRequest>) {
    let _ = &mut *out;
    let Some(overlay) = &mut app.modals.overlay else {
        return;
    };
    match overlay {
        Overlay::Files(finder) => {
            let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
            let action = finder
                .list
                .handle_standard_key(&key, finder.list.list_area.height.max(1) as i64);
            match key.code {
                // Enter opens the selected file in the editor modal, which
                // closes the finder unless `close_finder_on_open` is off —
                // or, for a markdown file, reads it first in the FILE TABS.
                KeyCode::Enter => open_selected_file(app),
                // Ctrl+y copies the selected path (relative to the worktree
                // root) to the clipboard — ready to paste into an agent.
                KeyCode::Char('y') if ctrl => {
                    if let Some(path) = finder.selected_path().map(str::to_string) {
                        app.modals.overlay = None;
                        let label = format!("copied {path}");
                        copy_and_flash(app, &path, &label);
                    }
                }
                _ => match action {
                    crate::filter_list::FilterListKey::Close => app.modals.overlay = None,
                    crate::filter_list::FilterListKey::Cleared
                    | crate::filter_list::FilterListKey::QueryChanged => finder.apply_filter(),
                    crate::filter_list::FilterListKey::Moved
                    | crate::filter_list::FilterListKey::None => {}
                },
            }
        }
        _ => unreachable!("overlay dispatcher passed the wrong overlay"),
    }
}

fn handle_grep_key(app: &mut App, key: KeyEvent, out: &mut Vec<ClientRequest>) {
    let _ = &mut *out;
    let Some(overlay) = &mut app.modals.overlay else {
        return;
    };
    match overlay {
        Overlay::Grep(view) => {
            let action = view
                .list
                .handle_standard_key(&key, view.list.list_area.height.max(1) as i64);
            match key.code {
                // Enter opens the hit in the editor modal, which closes this
                // overlay unless `close_finder_on_open` is off.
                KeyCode::Enter => open_selected_hit_in_editor(app),
                _ => match action {
                    crate::filter_list::FilterListKey::Close => app.modals.overlay = None,
                    crate::filter_list::FilterListKey::Cleared
                    | crate::filter_list::FilterListKey::QueryChanged => view.run_search(),
                    crate::filter_list::FilterListKey::Moved
                    | crate::filter_list::FilterListKey::None => {}
                },
            }
        }
        _ => unreachable!("overlay dispatcher passed the wrong overlay"),
    }
}

fn handle_tree_key(app: &mut App, key: KeyEvent, out: &mut Vec<ClientRequest>) {
    let _ = &mut *out;
    let Some(overlay) = &mut app.modals.overlay else {
        return;
    };
    match overlay {
        Overlay::Tree(view) => {
            let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
            let shift = key.modifiers.contains(KeyModifiers::SHIFT);
            let half = (view.view_height / 2).max(1) as i32;
            let page = view.view_height.max(1) as i32;
            match key.code {
                // Two-stage escape: an active filter is cleared before the
                // second Esc closes the modal.
                KeyCode::Esc if !view.list.query.is_empty() => {
                    view.list.query.clear();
                    view.apply_filter();
                }
                KeyCode::Esc => app.modals.overlay = None,
                // The preview scrolls on the diff-modal keys: ⇧↑/↓ lines,
                // Ctrl+d/u half pages, PageUp/Down, Home/End.
                KeyCode::Char('d') if ctrl => view.scroll_by(half),
                // Ctrl+u is the line editor's kill-to-start while something
                // is typed; only with an empty filter does it scroll.
                KeyCode::Char('u') if ctrl && view.list.query.is_empty() => view.scroll_by(-half),
                KeyCode::Down if shift => view.scroll_by(1),
                KeyCode::Up if shift => view.scroll_by(-1),
                KeyCode::PageDown => view.scroll_by(page),
                KeyCode::PageUp => view.scroll_by(-page),
                KeyCode::Home => view.scroll = 0,
                KeyCode::End => view.scroll = view.max_scroll(),
                // j/k stay typeable in the filter; Ctrl+n/p mirror ↑/↓.
                KeyCode::Down => view.select(view.list.cursor as i64 + 1),
                KeyCode::Up => view.select(view.list.cursor as i64 - 1),
                KeyCode::Char('n') if ctrl => view.select(view.list.cursor as i64 + 1),
                KeyCode::Char('p') if ctrl => view.select(view.list.cursor as i64 - 1),
                KeyCode::Right => view.expand_selected(),
                KeyCode::Left => view.collapse_selected(),
                // Enter folds/unfolds a directory; on a file it opens the
                // editor modal, with the browser staying open underneath.
                KeyCode::Enter => {
                    if view.selected_is_dir() {
                        view.toggle_row(view.list.cursor);
                    } else {
                        open_selected_tree_file_in_editor(app);
                    }
                }
                // Ctrl+y copies the selected path (relative to the worktree
                // root) to the clipboard — ready to paste into an agent.
                KeyCode::Char('y') if ctrl => {
                    if let Some(path) = view.selected_node().map(|n| n.path.clone()) {
                        app.modals.overlay = None;
                        let label = format!("copied {path}");
                        copy_and_flash(app, &path, &label);
                    }
                }
                // Ctrl+r flips a markdown file between its rendered page
                // and its source (the FILE TABS' `m`, which the filter
                // would type here); on any other file it is the filter's.
                KeyCode::Char('r') if ctrl && view.markdown => view.toggle_pretty(),
                // Everything else feeds the always-on fuzzy filter, which
                // edits like a terminal line (see text_input).
                _ => {
                    if view.list.query.handle_key(&key).changed() {
                        view.apply_filter();
                    }
                }
            }
        }
        _ => unreachable!("overlay dispatcher passed the wrong overlay"),
    }
}
