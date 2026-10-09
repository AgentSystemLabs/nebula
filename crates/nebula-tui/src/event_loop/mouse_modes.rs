//! Mouse dispatch split by overlay and grid/pane handling.

use super::*;

pub(super) fn handle_mouse(app: &mut App, mouse: MouseEvent, out: &mut Vec<ClientRequest>) {
    let mouse_pos = ratatui::layout::Position::new(mouse.column, mouse.row);
    update_pointer(app, &mouse);
    // The editor modal swallows the mouse entirely — its selection/scroll
    // story is vim's, not ours.
    if app.pane.vim.is_some() {
        return;
    }

    if handle_overlay_mouse(app, mouse, mouse_pos, out) {
        return;
    }

    handle_grid_or_pane_mouse(app, mouse, out);
}

fn handle_overlay_mouse(
    app: &mut App,
    mouse: MouseEvent,
    mouse_pos: ratatui::layout::Position,
    out: &mut Vec<ClientRequest>,
) -> bool {
    // A left-click outside any modal dismisses it, exactly as Esc would, and
    // lands its focus — only its focus — on the panel underneath. One
    // hit-test covers all fifteen variants; what each has to unwind on the
    // way out lives in `overlay_close`, and where focus goes after in
    // `land_click_focus`. Dismissing can put another modal up (a confirm
    // backs out to the settings it came from); focus stays put under that.
    if let MouseEventKind::Down(MouseButton::Left) = mouse.kind {
        if let Some(overlay) = &app.modals.overlay {
            if crate::overlay_close::click_is_outside(overlay, mouse_pos) {
                crate::overlay_close::click_outside(app, out);
                if app.modals.overlay.is_none() {
                    land_click_focus(app, mouse.column, mouse.row, out);
                }
                app.chrome.dirty = true;
                return true;
            }
        }
    }
    let Some(overlay) = &app.modals.overlay else {
        return false;
    };
    match overlay {
        Overlay::Menu(_) => handle_menu_mouse(app, mouse, mouse_pos, out),
        Overlay::Prompt(_) => handle_prompt_mouse(app, mouse, mouse_pos, out),
        Overlay::ProjectPicker(_) => handle_project_picker_mouse(app, mouse, mouse_pos, out),
        Overlay::Diff(_) => handle_diff_mouse(app, mouse, mouse_pos, out),
        Overlay::Palette(_) => handle_palette_mouse(app, mouse, mouse_pos, out),
        Overlay::Files(_) => handle_files_mouse(app, mouse, mouse_pos, out),
        Overlay::Grep(_) => handle_grep_mouse(app, mouse, mouse_pos, out),
        Overlay::Tree(_) => handle_tree_mouse(app, mouse, mouse_pos, out),
        Overlay::AgentPresets(_) => {
            crate::preset_overlays::handle_list_mouse(app, mouse, mouse_pos, out);
            true
        }
        Overlay::Issues(_) => {
            crate::issues::handle_mouse(app, mouse, mouse_pos, out);
            true
        }
        Overlay::PullRequests(_) => {
            crate::pr_modal::handle_mouse(app, mouse, mouse_pos, out);
            true
        }
        Overlay::BranchSwitch(_) => {
            crate::branch_switch::handle_mouse(app, mouse, mouse_pos);
            true
        }
        Overlay::Hosts(_) => handle_hosts_mouse(app, mouse, mouse_pos, out),
        Overlay::FileTabs(_) => {
            crate::file_tabs::handle_mouse(app, mouse, mouse_pos);
            true
        }
        Overlay::Settings(_) => handle_settings_mouse(app, mouse, mouse_pos, out),
        Overlay::Metrics(_) => handle_metrics_mouse(app, mouse, mouse_pos, out),
        _ => true,
    }
}

fn handle_menu_mouse(
    app: &mut App,
    mouse: MouseEvent,
    mouse_pos: ratatui::layout::Position,
    out: &mut Vec<ClientRequest>,
) -> bool {
    let _ = &mut *out;
    let _ = mouse_pos;
    // An open context menu owns the rest of the mouse: a click on a row
    // activates it, a right- or middle-click off the rows closes and lands
    // its focus like the left click above (the left button never gets here
    // — the pre-check took it), and everything is swallowed either way.
    if let Some(Overlay::Menu(menu)) = &app.modals.overlay {
        if let MouseEventKind::Down(_) = mouse.kind {
            let area = menu.area;
            let inside = mouse.column > area.x
                && mouse.column < area.x + area.width
                && mouse.row > area.y
                && mouse.row < area.y + area.height.saturating_sub(1);
            let hit = inside
                .then(|| (mouse.row - area.y - 1) as usize)
                .filter(|index| *index < menu.items.len());
            match hit {
                // Enter on that row, whichever row the hover was on.
                Some(index) => activate::menu_row(app, index, out),
                // A click on the menu's own border or a blank row is inert.
                None if inside => {}
                None => {
                    crate::overlay_close::click_outside(app, out);
                    if app.modals.overlay.is_none() {
                        land_click_focus(app, mouse.column, mouse.row, out);
                    }
                }
            }
            app.chrome.dirty = true;
        }
        return true;
    }
    false
}

fn handle_prompt_mouse(
    app: &mut App,
    mouse: MouseEvent,
    mouse_pos: ratatui::layout::Position,
    out: &mut Vec<ClientRequest>,
) -> bool {
    let _ = &mut *out;
    let _ = mouse_pos;
    // A prompt dialog is modal too: the wheel and clicks drive the
    // Add-project directory listing (click highlights, a second click on
    // the highlighted row steps in) or a task box's text (the wheel
    // scrolls it, a click puts the caret where it points); everything
    // else is swallowed.
    // The `[ ] new worktree` toggle on the LAUNCHER VIEW box's prompt
    // header is a button: a click on it flips the launch exactly as `^N`
    // does. Every other box leaves the rect empty, and an empty rect
    // contains no point.
    //
    // So are the four details above it — `project ^P`, `worktree main ^T`,
    // `agent Tab`, `model ^O`: a click on one opens the same picker its
    // chord does, the branch the WORKTREE PICKER.
    // Both are tested before the editor gets the click, since both sit
    // outside it.
    if let (Some(Overlay::Prompt(prompt)), MouseEventKind::Down(MouseButton::Left)) =
        (&app.modals.overlay, mouse.kind)
    {
        if prompt.toggle_area.contains(mouse_pos) {
            launcher::click_new_worktree(app);
            app.chrome.dirty = true;
            return true;
        }
        if let Some(field) = prompt
            .detail_areas
            .iter()
            .find(|(_, area)| area.contains(mouse_pos))
            .map(|(field, _)| *field)
        {
            launcher::click_box_field(app, field);
            app.chrome.dirty = true;
            return true;
        }
    }
    if let Some(Overlay::Prompt(prompt)) = &mut app.modals.overlay {
        let task_box = prompt.is_multiline();
        match mouse.kind {
            MouseEventKind::ScrollDown if task_box => {
                prompt.input.scroll_rows(MODAL_WHEEL_LINES as isize);
                app.chrome.dirty = true;
            }
            MouseEventKind::ScrollUp if task_box => {
                prompt.input.scroll_rows(-(MODAL_WHEEL_LINES as isize));
                app.chrome.dirty = true;
            }
            MouseEventKind::Down(MouseButton::Left) if task_box => {
                let area = prompt.editor_area;
                if area.contains(mouse_pos) {
                    let (row, col) = (mouse.row - area.y, mouse.column - area.x);
                    prompt.input.click(row, col);
                }
                app.chrome.dirty = true;
            }
            MouseEventKind::ScrollDown => {
                prompt.move_hover(1);
                app.chrome.dirty = true;
            }
            MouseEventKind::ScrollUp => {
                prompt.move_hover(-1);
                app.chrome.dirty = true;
            }
            MouseEventKind::Down(MouseButton::Left) => {
                let area = prompt.list_area;
                let first = prompt.window_start(area.height as usize);
                if let Some(i) = crate::list_hit::row_at(area, first, prompt.dirs.len(), mouse_pos)
                {
                    // A click highlights, as ↓ onto the row does; a second
                    // click on it steps in, as → does.
                    if prompt.hover == Some(i) {
                        prompt.dive(i);
                    } else {
                        prompt.hover = Some(i);
                    }
                }
                app.chrome.dirty = true;
            }
            _ => {}
        }
        return true;
    }
    false
}

fn handle_project_picker_mouse(
    app: &mut App,
    mouse: MouseEvent,
    mouse_pos: ratatui::layout::Position,
    out: &mut Vec<ClientRequest>,
) -> bool {
    let _ = &mut *out;
    // The PROJECT PICKER: the wheel moves its cursor, a click on a row
    // picks it (Enter on it); everything else is swallowed.
    if let Some(Overlay::ProjectPicker(picker)) = &mut app.modals.overlay {
        match mouse.kind {
            MouseEventKind::ScrollDown => picker.select(1),
            MouseEventKind::ScrollUp => picker.select(-1),
            MouseEventKind::Down(MouseButton::Left) => {
                let area = picker.list_area;
                let first = picker.window_start(area.height as usize);
                if let Some(i) =
                    crate::list_hit::row_at(area, first, picker.matches.len(), mouse_pos)
                {
                    launcher::click_picker_row(app, i);
                }
            }
            _ => {}
        }
        app.chrome.dirty = true;
        return true;
    }
    false
}

fn handle_diff_mouse(
    app: &mut App,
    mouse: MouseEvent,
    mouse_pos: ratatui::layout::Position,
    out: &mut Vec<ClientRequest>,
) -> bool {
    let _ = &mut *out;
    // Diff modal: the wheel over the file list walks its cursor a row a
    // notch (↑/↓'s own step), anywhere else it scrolls the diff; a click on
    // a file-list row selects that file (and folds or unfolds a tree
    // directory's), a drag on the files/diff border resizes the file list;
    // everything else is swallowed.
    if let Some(Overlay::Diff(view)) = &mut app.modals.overlay {
        let over_files = view.area.contains(mouse_pos) && mouse.column < view.splitter_x();
        match mouse.kind {
            MouseEventKind::ScrollUp if over_files => {
                let at = view.side_cursor();
                activate::diff_file(view, at as i64 - 1);
                app.chrome.dirty = true;
            }
            MouseEventKind::ScrollDown if over_files => {
                let at = view.side_cursor();
                activate::diff_file(view, at as i64 + 1);
                app.chrome.dirty = true;
            }
            MouseEventKind::ScrollUp => {
                view.scroll_by(-MODAL_WHEEL_LINES);
                app.chrome.dirty = true;
            }
            MouseEventKind::ScrollDown => {
                view.scroll_by(MODAL_WHEEL_LINES);
                app.chrome.dirty = true;
            }
            MouseEventKind::Down(MouseButton::Left) => {
                // Border grab zone: the two touching border cells at the
                // files/diff boundary.
                let bx = view.splitter_x();
                if on_vsplit(bx, view.area, mouse.column, mouse.row) {
                    view.files_drag = Some(bx as i32 - mouse.column as i32);
                    return true;
                }
                let area = view.list_area;
                if view.log.is_some() {
                    let first = crate::app::window_start(view.side_cursor(), area.height as usize);
                    if let Some(index) =
                        crate::list_hit::row_at(area, first, view.side_len(), mouse_pos)
                    {
                        activate::diff_row(view, index as i64);
                        app.chrome.dirty = true;
                    }
                } else {
                    let first = view.window_start(area.height as usize);
                    if let Some(index) =
                        crate::list_hit::row_at(area, first, view.row_count(), mouse_pos)
                    {
                        activate::diff_row(view, index as i64 + 1);
                        app.chrome.dirty = true;
                    }
                }
            }
            MouseEventKind::Drag(MouseButton::Left) => {
                if let Some(offset) = view.files_drag {
                    view.set_files_width(mouse.column as i32 + offset);
                    app.modals.diff_files_width = view.files_width;
                    app.chrome.dirty = true;
                }
            }
            MouseEventKind::Up(MouseButton::Left) if view.files_drag.take().is_some() => {
                app.chrome.dirty = true;
            }
            _ => {}
        }
        return true;
    }
    false
}

fn handle_palette_mouse(
    app: &mut App,
    mouse: MouseEvent,
    mouse_pos: ratatui::layout::Position,
    out: &mut Vec<ClientRequest>,
) -> bool {
    let _ = &mut *out;
    // Palette: the wheel moves the selection, a click on a result row jumps
    // there; everything else inside the box is swallowed.
    if let Some(Overlay::Palette(palette)) = &mut app.modals.overlay {
        match mouse.kind {
            MouseEventKind::ScrollUp => {
                palette.list.step(-1);
                app.chrome.dirty = true;
            }
            MouseEventKind::ScrollDown => {
                palette.list.step(1);
                app.chrome.dirty = true;
            }
            MouseEventKind::Down(MouseButton::Left) => {
                if let Some(index) = palette.list.hit(mouse_pos) {
                    palette.list.select(index as i64);
                    activate::palette_row(app, None, out);
                }
                app.chrome.dirty = true;
            }
            _ => {}
        }
        return true;
    }
    false
}

fn handle_files_mouse(
    app: &mut App,
    mouse: MouseEvent,
    mouse_pos: ratatui::layout::Position,
    out: &mut Vec<ClientRequest>,
) -> bool {
    let _ = &mut *out;
    // File finder: the wheel moves the selection, a click on a result row
    // opens it in the editor (closing the finder unless
    // `close_finder_on_open` is off); everything else inside the box is
    // swallowed.
    if let Some(Overlay::Files(finder)) = &mut app.modals.overlay {
        match mouse.kind {
            MouseEventKind::ScrollUp => {
                finder.list.step(-1);
                app.chrome.dirty = true;
            }
            MouseEventKind::ScrollDown => {
                finder.list.step(1);
                app.chrome.dirty = true;
            }
            MouseEventKind::Down(MouseButton::Left) => {
                if let Some(index) = finder.list.hit(mouse_pos) {
                    finder.list.select(index as i64);
                    // Enter on that row — the FILE TABS reader for a
                    // markdown file, the editor for anything else. The
                    // click used to call the editor half directly, and so
                    // missed the reader when Enter learned it.
                    open_selected_file(app);
                }
                app.chrome.dirty = true;
            }
            _ => {}
        }
        return true;
    }
    false
}

fn handle_grep_mouse(
    app: &mut App,
    mouse: MouseEvent,
    mouse_pos: ratatui::layout::Position,
    out: &mut Vec<ClientRequest>,
) -> bool {
    let _ = &mut *out;
    // Find-in-files: the wheel moves the selection, a click on a result row
    // opens it in the editor (closing this overlay unless
    // `close_finder_on_open` is off); everything else inside the box is
    // swallowed.
    if let Some(Overlay::Grep(view)) = &mut app.modals.overlay {
        match mouse.kind {
            MouseEventKind::ScrollUp => {
                view.list.step(-1);
                app.chrome.dirty = true;
            }
            MouseEventKind::ScrollDown => {
                view.list.step(1);
                app.chrome.dirty = true;
            }
            MouseEventKind::Down(MouseButton::Left) => {
                if let Some(index) = view.list.hit(mouse_pos) {
                    view.list.select(index as i64);
                    open_selected_hit_in_editor(app);
                }
                app.chrome.dirty = true;
            }
            _ => {}
        }
        return true;
    }
    false
}

fn handle_tree_mouse(
    app: &mut App,
    mouse: MouseEvent,
    mouse_pos: ratatui::layout::Position,
    out: &mut Vec<ClientRequest>,
) -> bool {
    let _ = &mut *out;
    // Tree browser: the wheel scrolls the preview, a click selects a row
    // (folding/unfolding directories), a drag on the tree/preview border
    // resizes the tree panel; everything else inside the box is swallowed.
    if let Some(Overlay::Tree(view)) = &mut app.modals.overlay {
        match mouse.kind {
            MouseEventKind::ScrollUp => {
                view.scroll_by(-MODAL_WHEEL_LINES);
                app.chrome.dirty = true;
            }
            MouseEventKind::ScrollDown => {
                view.scroll_by(MODAL_WHEEL_LINES);
                app.chrome.dirty = true;
            }
            MouseEventKind::Down(MouseButton::Left) => {
                // Border grab zone: the two touching border cells at the
                // tree/preview boundary.
                let bx = view.splitter_x();
                if on_vsplit(bx, view.area, mouse.column, mouse.row) {
                    view.files_drag = Some(bx as i32 - mouse.column as i32);
                    return true;
                }
                if let Some(index) = view.list.hit(mouse_pos) {
                    view.select(index as i64);
                    view.toggle_row(index); // no-op on files / under a filter
                }
                app.chrome.dirty = true;
            }
            MouseEventKind::Drag(MouseButton::Left) => {
                if let Some(offset) = view.files_drag {
                    view.set_files_width(mouse.column as i32 + offset);
                    app.chrome.dirty = true;
                }
            }
            MouseEventKind::Up(MouseButton::Left) if view.files_drag.take().is_some() => {
                app.chrome.dirty = true;
            }
            _ => {}
        }
        return true;
    }
    false
}

fn handle_hosts_mouse(
    app: &mut App,
    mouse: MouseEvent,
    mouse_pos: ratatui::layout::Position,
    out: &mut Vec<ClientRequest>,
) -> bool {
    let _ = &mut *out;
    // Hosts picker: the wheel moves the selection, a click on a row connects
    // (the context-menu convention — rows are actions, not editable items);
    // everything else inside the box is swallowed.
    if let Some(Overlay::Hosts(view)) = &mut app.modals.overlay {
        match mouse.kind {
            MouseEventKind::ScrollUp => {
                view.selected = clamp_selection(view.selected as i64 + (-1), view.hosts.len());
                app.chrome.dirty = true;
            }
            MouseEventKind::ScrollDown => {
                view.selected = clamp_selection(view.selected as i64 + (1), view.hosts.len());
                app.chrome.dirty = true;
            }
            MouseEventKind::Down(MouseButton::Left) => {
                let list = view.list_area;
                let first = view.window_start(list.height as usize);
                if let Some(index) =
                    crate::list_hit::row_at(list, first, view.hosts.len(), mouse_pos)
                {
                    view.selected = index;
                    let entry = view.hosts[index].clone();
                    activate::host(app, entry);
                }
                app.chrome.dirty = true;
            }
            _ => {}
        }
        return true;
    }
    false
}

fn handle_settings_mouse(
    app: &mut App,
    mouse: MouseEvent,
    mouse_pos: ratatui::layout::Position,
    out: &mut Vec<ClientRequest>,
) -> bool {
    let _ = &mut *out;
    let _ = mouse_pos;
    // Settings: click a tab to switch, a row to select (or activate it if
    // it was already selected); everything else inside the box is swallowed.
    // While a hotkey capture is live the mouse is inert — the overlay is
    // waiting for a key, and a stray click shouldn't answer it.
    if matches!(&app.modals.overlay, Some(Overlay::Settings(_))) {
        if let MouseEventKind::Down(MouseButton::Left) = mouse.kind {
            let Some(view) = settings(app) else {
                return true;
            };
            if view.capture.is_some() {
                return true;
            }
            let (area, tab, selected, body, first_row, hotkeys) = (
                view.area,
                view.tab,
                view.selected,
                view.body_area,
                view.first_row,
                view.is_hotkeys(),
            );
            let tab_hits = view.tab_hits.clone();
            // The mouse only says which command; `run_settings_cmd` is the
            // one place the overlay changes, as it is for the keys.
            // The strip first: its labels are recorded during draw. A
            // click on one is that tab's digit, with the cursor put in its
            // list.
            if let Some(next) = tab_hits
                .iter()
                .position(|(x0, x1)| mouse.column >= *x0 && mouse.column < *x1)
            {
                if mouse.row == area.y.saturating_add(1) {
                    run_settings_cmd(app, SettingsCmd::Tab(next));
                    run_settings_cmd(app, SettingsCmd::EnterList);
                    app.chrome.dirty = true;
                    return true;
                }
            }
            if body.height > 0 && mouse.row >= body.y && mouse.row < body.y + body.height {
                let row = first_row + (mouse.row - body.y) as usize;
                // Group headers and blanks aren't clickable; the shared
                // row map keeps this in step with the renderer.
                if let Some(index) = crate::config::settings_rows(tab)
                    .get(row)
                    .and_then(|r| r.index())
                {
                    // A click moves the cursor there, as j/k would; on the
                    // row it was already on, it is Enter.
                    run_settings_cmd(app, SettingsCmd::Move(index));
                    run_settings_cmd(app, SettingsCmd::EnterList);
                    if selected == index {
                        run_settings_cmd(app, activate::settings_row_cmd(hotkeys, index));
                    }
                }
            }
            app.chrome.dirty = true;
        }
        return true;
    }
    false
}

fn handle_metrics_mouse(
    app: &mut App,
    mouse: MouseEvent,
    mouse_pos: ratatui::layout::Position,
    out: &mut Vec<ClientRequest>,
) -> bool {
    let _ = &mut *out;
    // Metrics: the wheel moves the selection, a click on a row selects it
    // (a click on the selected row opens it); everything else inside the box
    // is swallowed.
    if let Some(Overlay::Metrics(view)) = &mut app.modals.overlay {
        let mut open = false;
        match mouse.kind {
            MouseEventKind::ScrollUp => {
                view.selected = clamp_selection(view.selected as i64 + (-1), view.rows.len());
                app.chrome.dirty = true;
            }
            MouseEventKind::ScrollDown => {
                view.selected = clamp_selection(view.selected as i64 + (1), view.rows.len());
                app.chrome.dirty = true;
            }
            MouseEventKind::Down(MouseButton::Left) => {
                if let Some(index) =
                    crate::list_hit::row_at(view.list_area, view.scroll, view.rows.len(), mouse_pos)
                {
                    // A click selects; on the row already selected it is
                    // Enter.
                    open = view.selected == index;
                    view.selected = index;
                }
                app.chrome.dirty = true;
            }
            _ => {}
        }
        if open {
            activate::metrics_row(app, out);
        }
        return true;
    }
    false
}

fn handle_grid_or_pane_mouse(app: &mut App, mouse: MouseEvent, out: &mut Vec<ClientRequest>) {
    // Motion while nebula holds the left button — the press came, its
    // release has not — is the drag going on, whatever button the host put
    // on the report: a host that lost track of the button between two
    // reports would otherwise end a selection the user is still making.
    // The release is the only end of a drag.
    let mouse = match mouse.kind {
        MouseEventKind::Moved if app.mouse_held() => MouseEvent {
            kind: MouseEventKind::Drag(MouseButton::Left),
            ..mouse
        },
        _ => mouse,
    };
    match mouse.kind {
        MouseEventKind::Down(MouseButton::Left) => handle_left_click(app, mouse, out),
        MouseEventKind::Drag(MouseButton::Left) => handle_left_drag(app, mouse, out),
        MouseEventKind::Up(MouseButton::Left) => handle_left_release(app, mouse, out),
        MouseEventKind::ScrollUp | MouseEventKind::ScrollDown => handle_wheel(app, mouse, out),
        MouseEventKind::Down(MouseButton::Right) => handle_right_click(app, mouse, out),
        _ => {}
    }
}

fn handle_left_click(app: &mut App, mouse: MouseEvent, out: &mut Vec<ClientRequest>) {
    if handle_alt_terminal_link_click(app, mouse) {
        return;
    }
    // Any fresh click clears a stale selection highlight; a click on
    // the terminal pane below re-arms one. A button the program was
    // still holding (its release never arrived) is let go the same
    // way.
    app.pane.term_selection = None;
    app.pane.next_drag_autoscroll = None;
    app.pane.term_mouse_grab = None;
    app.launcher.card_drag = None;
    handle_left_click_target(app, mouse, out);
    app.chrome.dirty = true;
}

fn handle_alt_terminal_link_click(app: &mut App, mouse: MouseEvent) -> bool {
    // ⌥click on a detected URL opens it in the browser; the click is
    // swallowed so it doesn't move focus or disturb the selection.
    // (Cmd never reaches us — the SGR mouse protocol has no such
    // bit — so Option is the "open link" modifier.)
    if mouse.modifiers.contains(KeyModifiers::ALT)
        && matches!(
            app.hit_at(mouse.column, mouse.row),
            Some(HitTarget::TerminalPane)
        )
    {
        let cell = pane_cell(app.pane.term_area, mouse.column, mouse.row);
        if let Some(url) = app
            .pane
            .term_links
            .iter()
            .find(|link| link.contains(cell))
            .map(|link| link.url.clone())
        {
            app.chrome.flash = Some(if open_url(&url) {
                format!("opened {url}")
            } else {
                format!("open failed: {url}")
            });
            app.chrome.dirty = true;
            return true;
        }
        // Not a URL — a detected file path opens in the editor
        // modal instead (claude/cursor/codex print `path:line`).
        if let Some((path, line)) = app
            .pane
            .term_file_links
            .iter()
            .find(|link| link.contains(cell))
            .map(|link| (link.path.clone(), link.line))
        {
            open_file_link(app, &path, line);
            app.chrome.dirty = true;
            return true;
        }
    }
    false
}

fn handle_left_click_target(app: &mut App, mouse: MouseEvent, out: &mut Vec<ClientRequest>) {
    match app.hit_at(mouse.column, mouse.row) {
        Some(HitTarget::LauncherPaneSplitter) => {
            // A second press on the edge within the double-click
            // window snaps it to the middle of the body
            // (`launcher::center_pane`), and arms no drag: the
            // edge has moved out from under the pointer, and a
            // drag from there would yank it straight back.
            if is_double_click(&mut app.pane.last_pane_edge_click, ()) {
                launcher::center_pane(app);
                return;
            }
            // The LAUNCHER VIEW's pane edge, armed the same way and
            // as quietly: the offset from the grabbed row — or,
            // beside the cards, column — is kept so the edge does
            // not jump depending on which of the two grab cells was
            // caught. The boundary is measured by the arithmetic
            // the draw laid it out with.
            let at = app.launcher_pane_side().along(mouse.column, mouse.row);
            let boundary = app.launcher_pane_boundary().unwrap_or(at);
            app.launcher.launcher_pane_drag = Some(boundary - at);
        }
        // A LAUNCHER VIEW card: the cursor lands on it, inside its
        // worktree; a second click is Enter, down into the PANE
        // beside the cards — which comes back first if it was
        // folded away.
        // The press also arms a drag of the card onto another
        // band, which moves its session there. Armed first: the
        // card is found by the grid as drawn, before the click.
        Some(HitTarget::LauncherCard(at)) => {
            launcher::press_card(app, at, (mouse.column, mouse.row));
            launcher::click_card(app, at, out);
        }
        // A BAND's rule: the cursor lands on the band, as `j`/`k`
        // walking onto it do.
        Some(HitTarget::LauncherBand(i)) => launcher::click_band(app, i, out),
        // The `❮` / `❯` beside a band's row: one card that way
        // along the band, the very step `h` / `l` take.
        Some(HitTarget::LauncherStripLeft(i)) => launcher::click_strip_arrow(app, i, -1, out),
        Some(HitTarget::LauncherStripRight(i)) => launcher::click_strip_arrow(app, i, 1, out),
        // `▾ 6 more · Tab: see all 8` under a band's row: the band
        // opens, the very toggle Tab runs.
        Some(HitTarget::LauncherBandMore(i)) => launcher::click_band_more(app, i, out),
        // The fold caret on a worktree's header (the NESTED
        // layout): the band folds or opens, the very toggle Tab
        // runs.
        Some(HitTarget::LauncherBandFold(i)) => launcher::click_band_fold(app, i, out),
        // A NESTED thread's `#42`: the PULL REQUESTS MODAL on it.
        Some(HitTarget::LauncherThreadPr(wid)) => launcher::click_thread_pr(app, &wid),
        // The PULL REQUEST on a band's rule: it opens in the
        // browser, through the very `open_pull_request` `⇧V` runs.
        Some(HitTarget::LauncherBandPr(wid)) => launcher::click_pull_request(app, &wid, out),
        // The ISSUE NUMBER on a session's card: the cursor onto
        // the card, and the issue in the browser, through the very
        // `open_issue` `⇧I` runs.
        Some(HitTarget::LauncherCardIssue(id)) => launcher::click_issue(app, &id, out),
        // `‹ sessions` in a full-screen session's header: back
        // down to the pane beside the grid, the same way `^q`
        // goes back.
        Some(HitTarget::LauncherCrumb) => {
            launcher::toggle_full_screen(app, out);
        }
        // The GRID header's PROJECT TABS: a tab opens its
        // project, through the `open_tab` that `[` and `]` walk
        // with — or, with the header holding the keys, through
        // the `choose_tab` Enter on it runs; its `×` closes it;
        // the `+` after them drops the PROJECT DROPDOWN — every
        // project, narrowed by whatever you type, and a row that
        // opens a folder — whose pick opens a tab; the MORE CHIP
        // drops the tabs the row had no room for.
        Some(HitTarget::LauncherTab(id)) => launcher::click_tab(app, &id, out),
        Some(HitTarget::LauncherTabClose(id)) => launcher::close_tab(app, &id, out),
        Some(HitTarget::LauncherTabAdd) => launcher::open_project_menu(app),
        Some(HitTarget::LauncherTabMore) => launcher::open_more_tabs_menu(app),
        // The key cap in the empty grid's welcome: the QUICK
        // PROMPT, through the `open_box` its key runs.
        Some(HitTarget::LauncherWelcomePrompt) => launcher::open_box(app),
        // The header's PR & ISSUE COUNTS: each opens its own list
        // for the project in front of you, through the very
        // function `v` / `i` run — the click is the key's twin.
        Some(HitTarget::LauncherPullRequests) => crate::pr_modal::open(app),
        Some(HitTarget::LauncherIssues) => crate::issues::open_issues(app),
        // The footer's memory readout: the modal `⇧M` opens,
        // through the same `open_metrics`.
        Some(HitTarget::FooterUsage) => open_metrics(app, out),
        // The CLOSE BUTTON at the strip's right end: the pane
        // folds away through the one `toggle_pane` `^~` runs. It
        // is only drawn on a pane that is showing, so the toggle
        // can only ever fold.
        Some(HitTarget::LauncherPaneClose) => launcher::toggle_pane(app),
        // The SIDE BUTTON beside it: the pane moves to the other
        // side of the cards, written to Settings as the
        // **Session pane** row's own cycling writes it.
        Some(HitTarget::LauncherPaneSide) => launcher::move_pane(app),
        // The FULL-SCREEN BUTTON before them, and the NORMAL-SIZE
        // BUTTON in a full-screen session's header: the one
        // toggle `^F` runs.
        Some(HitTarget::LauncherPaneZoom) => {
            launcher::toggle_full_screen(app, out);
        }
        // Never in the hit map: the ISSUES and PULL REQUESTS MODALS route
        // the click on their button themselves, before this is reached.
        Some(HitTarget::ModalBrowser) => {}
        Some(
            target @ (HitTarget::ProjectRow(_)
            | HitTarget::WorktreeRow(_)
            | HitTarget::SessionRow(_)),
        ) => {
            select_clicked_row(app, &target, out);
        }
        Some(HitTarget::PanelBg(focus)) => {
            // The LAUNCHER VIEW's GRID lies on the same
            // background, and a click on the air between its
            // cards is a MISS: it takes FOCUS and does nothing
            // else. It used to run `launcher::clear_aim`, which
            // folds the PANE along the bottom away with the
            // card — so a click anywhere in the gutter, or on
            // the blank rows under a short row of cards, shut
            // the session you were reading. Letting the card go
            // is Esc's (and `^~` folds the pane outright); a
            // click that lands on nothing changes nothing.
            app.nav.focus = focus;
        }
        Some(HitTarget::CloudSessionLink) => {
            activate::cloud_link(app, out);
        }
        Some(HitTarget::TerminalPane) => {
            // A click into the pane is deliberate — it is Enter on
            // the pane (`enter_terminal_pane`): FOCUS, the input
            // lock for a live session, and a debounced attach sent
            // now, since keystrokes are about to need it. The
            // click used to set the first two itself and skipped
            // the third, so typing right after clicking into a
            // session the cursor had just swept onto went to a
            // pane the daemon had not been asked for yet.
            if let Some(sref) = app.pane.term.as_ref().map(|t| t.sref.clone()) {
                enter_terminal_pane(app, out);
                let cell = pane_cell(app.pane.term_area, mouse.column, mouse.row);
                let (mode, sgr) = app.child_mouse_mode();
                if mode != vt100::MouseProtocolMode::None {
                    // The program asked for the mouse (claude's
                    // fullscreen renderer, vim `mouse=a`, htop): the
                    // press is its, and so are the drag and release
                    // to come. Its own selection knows its layout —
                    // claude's diff panel sits beside the
                    // conversation, and a screen-row copy of ours
                    // took both (#52). ⇧drag still selects through
                    // the terminal.
                    let button = mouse_modifier_bits(mouse.modifiers);
                    forward_mouse(app, out, sgr, button, false, &mouse);
                    app.pane.term_mouse_grab = Some(sref);
                } else if is_double_click(&mut app.pane.last_term_click, cell) {
                    // Double-click: select (and copy) the word under
                    // the cursor.
                    select_word_at(app, cell);
                } else {
                    // Arm a drag-selection; it becomes visible (and
                    // copyable) once the drag leaves this cell.
                    let base = app
                        .pane
                        .term
                        .as_ref()
                        .map_or(0, |t| t.parser.screen().history_base());
                    let cell = (cell.0, base + u64::from(cell.1));
                    app.pane.term_selection = Some(TermSelection {
                        anchor: cell,
                        head: cell,
                        dragging: true,
                        active: false,
                        pointer: (mouse.column, mouse.row),
                    });
                }
            }
        }
        None => {}
    }
}

fn handle_left_drag(app: &mut App, mouse: MouseEvent, out: &mut Vec<ClientRequest>) {
    if app.launcher.card_drag.is_some() {
        launcher::drag_card(app, (mouse.column, mouse.row));
    } else if let Some(grab) = app.launcher.launcher_pane_drag {
        let at = app.launcher_pane_side().along(mouse.column, mouse.row);
        app.set_launcher_pane(at + grab);
        // A press that became a drag is not the first half of a
        // double-click: letting the edge go and pressing it again
        // straight away must not snap it to the middle.
        app.pane.last_pane_edge_click = None;
        app.chrome.dirty = true;
    } else if let Some(sref) = &app.pane.term_mouse_grab {
        // The program holding the button gets the motion — if it
        // asked for motion at all (`?1002h` / `?1003h`); press-only
        // and press/release tracking hear nothing until the release.
        let (mode, sgr) = app.child_mouse_mode();
        let held = app.pane.term.as_ref().is_some_and(|t| &t.sref == sref);
        if held
            && matches!(
                mode,
                vt100::MouseProtocolMode::ButtonMotion | vt100::MouseProtocolMode::AnyMotion
            )
        {
            let button = 32 | mouse_modifier_bits(mouse.modifiers);
            forward_mouse(app, out, sgr, button, false, &mouse);
        }
    } else {
        drag_select_to(app, (mouse.column, mouse.row), out);
    }
}

fn handle_left_release(app: &mut App, mouse: MouseEvent, out: &mut Vec<ClientRequest>) {
    // The pane edge lets go here.
    let pane_ended = app.launcher.launcher_pane_drag.take().is_some();
    if pane_ended {
        app.chrome.dirty = true;
    } else if app.launcher.card_drag.is_some() {
        launcher::drop_card(app, out);
    } else if let Some(sref) = app.pane.term_mouse_grab.take() {
        // The release closes the program's button — except under
        // press-only tracking (`?9h`), which has no release report.
        let (mode, sgr) = app.child_mouse_mode();
        let held = app.pane.term.as_ref().is_some_and(|t| t.sref == sref);
        if held
            && !matches!(
                mode,
                vt100::MouseProtocolMode::None | vt100::MouseProtocolMode::Press
            )
        {
            let button = mouse_modifier_bits(mouse.modifiers);
            forward_mouse(app, out, sgr, button, true, &mouse);
        }
    } else if app.pane.term_selection.is_some_and(|s| s.dragging) {
        finish_selection(app);
    }
}

fn handle_wheel(app: &mut App, mouse: MouseEvent, out: &mut Vec<ClientRequest>) {
    let up = matches!(mouse.kind, MouseEventKind::ScrollUp);
    let over = app.hit_at(mouse.column, mouse.row);
    if app.launcher_grid() && app.launcher.launcher_columns {
        let column = match over {
            Some(HitTarget::ProjectRow(_) | HitTarget::PanelBg(Focus::Projects)) => {
                Some(Focus::Projects)
            }
            Some(HitTarget::WorktreeRow(_) | HitTarget::PanelBg(Focus::Worktrees)) => {
                Some(Focus::Worktrees)
            }
            Some(HitTarget::SessionRow(_) | HitTarget::PanelBg(Focus::Sessions)) => {
                Some(Focus::Sessions)
            }
            _ => None,
        };
        if let Some(focus) = column {
            scroll_columns(app, focus, up);
            return;
        }
    }
    // The LAUNCHER VIEW's grid: a notch over the cards scrolls them
    // under a cursor that stays put (`launcher::wheel_grid`). A
    // notch used to walk the cursor, and walking it swaps the pane
    // onto another session and reads it — far too easy to do by
    // accident on a trackpad while reading the one you are on — so
    // the keys walk the grid and the wheel only moves the window.
    // It stops here rather than falling through to the panels'
    // scrolling, which this view never draws.
    if app.launcher_grid()
        && matches!(
            over,
            Some(
                HitTarget::LauncherCard(_)
                    | HitTarget::LauncherBand(_)
                    | HitTarget::LauncherBandPr(_)
                    | HitTarget::LauncherCardIssue(_)
                    | HitTarget::LauncherStripLeft(_)
                    | HitTarget::LauncherStripRight(_)
                    | HitTarget::LauncherBandMore(_)
                    | HitTarget::LauncherBandFold(_)
                    | HitTarget::LauncherThreadPr(_)
                    | HitTarget::PanelBg(Focus::Sessions)
            )
        )
    {
        launcher::wheel_grid(app, up);
        return;
    }
    let in_term = matches!(over, Some(HitTarget::TerminalPane)) || app.pane.collapsed;
    if in_term && app.reading_url().is_some() {
        // The pane is showing a pull request or an issue, not a
        // session: the wheel reads it rather than reaching the
        // PTY underneath.
        let max = app.pr_preview_max_scroll();
        app.github.pr_preview_scroll = if up {
            app.github
                .pr_preview_scroll
                .saturating_sub(PR_PREVIEW_WHEEL_STEP)
        } else {
            app.github
                .pr_preview_scroll
                .saturating_add(PR_PREVIEW_WHEEL_STEP)
                .min(max)
        };
        app.chrome.dirty = true;
    } else if in_term {
        // A stand-in pane has no PTY to forward the wheel to; its
        // grid is empty, so there is nothing to scroll either
        // (`child_mouse_mode` calls it mouseless).
        let (mouse_mode, sgr) = app.child_mouse_mode();
        if let Some(term) = &mut app.pane.term {
            // The wheel takes a finished selection's highlight with
            // it. One still being dragged rides along: its lines
            // are the history's, so the highlight stays on its
            // text as the view moves.
            if !app.pane.term_selection.is_some_and(|s| s.dragging) {
                app.pane.term_selection = None;
            }
            let alternate = term.parser.screen().alternate_screen();
            if mouse_mode != vt100::MouseProtocolMode::None {
                // The child asked for the mouse (claude's alt-screen
                // UI, vim `mouse=a`, htop): forward the wheel event
                // itself. Synthesized arrows would land in claude's
                // input box — cycling prompt history and tripping its
                // "Scroll wheel is sending arrow keys" warning.
                let (col, row) = pane_cell(app.pane.term_area, mouse.column, mouse.row);
                let button: u16 = if up { 64 } else { 65 };
                out.push(ClientRequest::Input {
                    session: term.sref.clone(),
                    data: mouse_report(sgr, button, false, col, row),
                });
            } else if alternate {
                // Full-screen apps that ignore the mouse (plain vim,
                // less, htop with mouse off) expect arrows, one per
                // line the notch would have scrolled.
                let arrow: &[u8] = if up { b"\x1b[A" } else { b"\x1b[B" };
                out.push(ClientRequest::Input {
                    session: term.sref.clone(),
                    data: arrow.repeat(TERM_WHEEL_LINES),
                });
            } else {
                let current = term.scroll_offset();
                let new_scroll = if up {
                    current.saturating_add(TERM_WHEEL_LINES)
                } else {
                    current.saturating_sub(TERM_WHEEL_LINES)
                };
                scroll_pane_to(app, new_scroll, out);
            }
            app.chrome.dirty = true;
        }
    }
}

fn scroll_columns(app: &mut App, focus: Focus, up: bool) {
    const STEP: u16 = 3;
    let (len, scroll) = match focus {
        Focus::Projects => (
            app.project_rows().len(),
            &mut app.launcher.columns_projects_scroll,
        ),
        Focus::Worktrees => (
            app.worktree_rows().len(),
            &mut app.launcher.columns_worktrees_scroll,
        ),
        Focus::Sessions => (
            app.visible_session_rows().len(),
            &mut app.launcher.columns_sessions_scroll,
        ),
        Focus::Terminal => return,
    };
    let max = len.saturating_sub(1) as u16;
    let next = if up {
        scroll.saturating_sub(STEP)
    } else {
        scroll.saturating_add(STEP).min(max)
    };
    if *scroll != next {
        *scroll = next;
        app.chrome.dirty = true;
    }
}

fn handle_right_click(app: &mut App, mouse: MouseEvent, out: &mut Vec<ClientRequest>) {
    // The right button is two steps: the cursor moves onto the row
    // as a left click moves it (`select_clicked_row`), then the
    // row's own CONTEXT MENU opens, from the one builder
    // (`context_menu_items`). A panel's
    // background has no row and no cursor, so its menu is the
    // mouse's alone.
    let at = (mouse.column, mouse.row);
    match app.hit_at(mouse.column, mouse.row) {
        // A PROJECT TAB has no cursor to move, but it is the
        // project's own handle: the right button opens it, with
        // the project's menu hung under the tab.
        Some(HitTarget::LauncherTab(id)) => launcher::tab_menu(app, &id, out),
        // The MORE CHIP has no menu but its list: either button
        // drops it.
        Some(HitTarget::LauncherTabMore) => launcher::open_more_tabs_menu(app),
        Some(HitTarget::PanelBg(focus)) => {
            app.nav.focus = focus;
            let items = panel_menu_items(app, focus);
            open_menu(app, items, at);
        }
        Some(target) if select_clicked_row(app, &target, out) => {
            if let Some(items) = context_menu_items(app, app.nav.focus) {
                open_menu(app, items, at);
            }
        }
        Some(_) => {}
        None => {}
    }
    app.chrome.dirty = true;
}
