//! Extracted event-loop helper section.

use super::*;

pub(crate) fn sync_pty_size(app: &mut App, out: &mut Vec<ClientRequest>) {
    let area = app.pane.term_area;
    if !pane_usable(area) {
        return;
    }
    if let Some(term) = &mut app.pane.term {
        if (term.cols, term.rows) != (area.width, area.height) {
            // The grid regrids and the program repaints into it: a
            // finished selection is let go rather than shown over whatever
            // lands. A drag under way keeps its history lines — rows keep
            // their numbers through a resize — and the button ends it.
            if !app.pane.term_selection.is_some_and(|s| s.dragging) {
                app.pane.term_selection = None;
            }
            term.cols = area.width;
            term.rows = area.height;
            term.parser.screen_mut().set_size(area.height, area.width);
            out.push(ClientRequest::Resize {
                session: term.sref.clone(),
                cols: area.width,
                rows: area.height,
            });
        }
    }
}

/// Keep the editor modal's PTY and parser sized to the drawn inner rect
/// (the `sync_pty_size` pattern, minus the daemon round-trip).
pub(crate) fn sync_vim_size(app: &mut App) {
    if let Some(vim) = &mut app.pane.vim {
        if pane_usable(vim.area) {
            vim.resize(vim.area.width, vim.area.height);
        }
    }
}

/// Whether a rect has been drawn large enough to size a grid to.
pub(crate) fn pane_usable(area: ratatui::layout::Rect) -> bool {
    area.width >= MIN_PANE_DIM && area.height >= MIN_PANE_DIM
}

/// Editor reader-thread events. A stale generation (bytes buffered from an
/// editor that was already closed) is dropped on the floor.
pub(crate) fn handle_vim_event(app: &mut App, ev: VimEvent) {
    match ev {
        VimEvent::Output { generation, data } => {
            if let Some(vim) = &mut app.pane.vim {
                if vim.generation == generation {
                    vim.process(&data);
                    app.chrome.dirty = true;
                }
            }
        }
        VimEvent::Exited { generation } => {
            if app
                .pane
                .vim
                .as_ref()
                .is_some_and(|v| v.generation == generation)
            {
                close_vim(app);
                app.chrome.dirty = true;
            }
        }
    }
}

/// Drop the editor; an embedded one hands its preview pane back to the tree
/// browser with the (possibly just-edited) file reloaded. The FILE TABS
/// re-read the file whether the editor was theirs or floating over them,
/// and land the cursor on the strip — the level Ctrl+Q steps back to.
pub(crate) fn close_vim(app: &mut App) {
    let embedded = app.pane.vim.as_ref().is_some_and(|v| v.embedded);
    app.pane.vim = None;
    match &mut app.modals.overlay {
        Some(Overlay::Tree(view)) if embedded => view.load_preview(),
        Some(Overlay::FileTabs(view)) => view.editor_closed(),
        _ => {}
    }
}

/// One frame: paint it, then park the host's cursor on the cell of the
/// PTY cursor the keyboard is headed for (`App::host_cursor`), so a CJK
/// input method's composition lands where the typing goes rather than
/// wherever the frame's last diff run left the cursor (#53). Ratatui's
/// `Frame::set_cursor_position` would show the cursor as well, and the
/// pane paints its own; so the move is made after the frame, on the
/// terminal, with the cursor still hidden — one `CUP` on the wire.
pub(crate) fn draw_frame<B: ratatui::backend::Backend>(
    terminal: &mut Terminal<B>,
    app: &mut App,
) -> Result<(), B::Error> {
    terminal.draw(|f| ui::draw(f, app))?;
    if let Some(cell) = app.pane.host_cursor {
        terminal.set_cursor_position(cell)?;
    }
    Ok(())
}

/// Every key, click and paste comes through here, which makes it the one
/// place that can tell a manual move from the tree shifting under the
/// cursors: whatever changed where the user is (`whereabouts`) across the
/// event, the user did. A create that was in flight before it — seconds,
/// for a PR SESSION's fetch and `git worktree add` — is left behind: its
/// Ack puts the row in the list and takes no cursor, pane or FOCUS back
/// to it. A create the event itself fired is not in the list taken before
/// it, so the launch that moved the cursor onto its own stand-in rows is
/// still followed.
pub(crate) fn handle_terminal_event(app: &mut App, event: Event, out: &mut Vec<ClientRequest>) {
    let watch = follows_to_watch(app);
    dispatch_terminal_event(app, event, out);
    let Some((before, in_flight)) = watch else {
        return;
    };
    if whereabouts(app) == before {
        return;
    }
    // A follow whose Ack already came, waiting only on the row's upsert.
    app.requests.select_when_seen = None;
    app.requests.select_project_when_seen = None;
    app.requests.select_worktree_when_seen = None;
    for req_id in in_flight {
        if app.requests.pending.contains_key(&req_id) {
            app.requests.left_behind.insert(req_id);
        }
    }
}

/// Where the user is, by id: FOCUS and the row under each cursor.
#[derive(PartialEq)]
pub(crate) struct Whereabouts {
    focus: Focus,
    selection: SelectionSnapshot,
}

pub(crate) fn whereabouts(app: &App) -> Whereabouts {
    Whereabouts {
        focus: app.nav.focus,
        selection: selection_snapshot(app),
    }
}

/// Where the user is and the creates still being followed, taken ahead of
/// an input event. None when nothing is — nearly always — so a mouse
/// motion does not pay for a snapshot it has no use for.
pub(crate) fn follows_to_watch(app: &App) -> Option<(Whereabouts, Vec<u64>)> {
    let in_flight: Vec<u64> = app
        .requests
        .pending
        .iter()
        .filter(|(req_id, intent)| intent.follows() && !app.requests.left_behind.contains(req_id))
        .map(|(req_id, _)| *req_id)
        .collect();
    let armed = app.requests.select_when_seen.is_some()
        || app.requests.select_project_when_seen.is_some()
        || app.requests.select_worktree_when_seen.is_some();
    (armed || !in_flight.is_empty()).then(|| (whereabouts(app), in_flight))
}

pub(crate) fn dispatch_terminal_event(app: &mut App, event: Event, out: &mut Vec<ClientRequest>) {
    // With the pane holding input, whatever this event turns into is headed
    // for the PTY — and the daemon drops Input for a session it hasn't
    // spawned. A still-debounced attach has to land before the keystroke.
    if app.pane.term_locked && app.pane_accepts_input() {
        fire_pending_attach(app, out);
    }
    // The LAUNCHER VIEW's card under the cursor, ahead of a key or a click
    // that may take it out of the grid (`launcher::keep_cursor`).
    let pressed = matches!(event, Event::Key(_))
        || matches!(
            event,
            Event::Mouse(MouseEvent {
                kind: MouseEventKind::Down(_),
                ..
            })
        );
    let launcher_before = (pressed && app.launcher_grid() && !app.pane.term_locked)
        .then(|| launcher::cursor_entry(app))
        .flatten();
    dispatch_input(app, event, out);
    if let Some(before) = launcher_before {
        launcher::keep_cursor(app, before, out);
    }
}

/// [`dispatch_terminal_event`]'s body: the event to its handler.
pub(crate) fn dispatch_input(app: &mut App, event: Event, out: &mut Vec<ClientRequest>) {
    match event {
        // The RELEASE WATCH reads every key first: a held unarchive key's
        // repeats end here, one unarchive per press (release_watch.rs).
        Event::Key(key)
            if release_watch::take(
                &mut app.chrome.release_watch,
                &key,
                std::time::Instant::now(),
            ) => {}
        Event::Key(key) if key.kind != KeyEventKind::Release => {
            let typing = typing_into_pane(app);
            app.chrome.flash = None;
            handle_key(app, key, out);
            // A key that only went to the PTY changed nothing here: what
            // it does shows up as the PTY's answer, a couple of
            // milliseconds on. Painting an identical frame for it first
            // put that answer's frame a draw and a FRAME PACING gap
            // behind — 8 ms from key to echo under the INPUT LATENCY
            // PROBE instead of 3 — on every character typed at an agent.
            if !(typing && typing_into_pane(app)) {
                app.chrome.dirty = true;
            }
        }
        Event::Mouse(mouse) => {
            handle_mouse(app, mouse, out);
            // A click anywhere hands the keys back to the cards from the
            // PROJECT TABS' cursor — after the click, which on a tab is
            // Enter on it while the header holds them (`launcher::click_tab`).
            if matches!(mouse.kind, MouseEventKind::Down(_)) {
                launcher::leave_tabs(app);
            }
        }
        Event::Paste(text) if app.pane.vim.is_some() => {
            if let Some(vim) = &mut app.pane.vim {
                // Bracketed paste so vim doesn't auto-indent it to mush.
                vim.input(&bracketed(&text));
            }
        }
        // An overlay with a live text field takes the paste: ⌘V into a
        // filter or the ssh destination lands where the caret is.
        Event::Paste(text) if paste_into_overlay(app, &text) => {}
        // Then an expanded session card's FOLLOW-UP COMPOSER, which holds
        // the keyboard the way an overlay's field does while it is open.
        Event::Paste(text) if paste_into_follow_up(app, &text) => {}
        Event::Paste(text) => {
            // A stand-in pane (QUICK PROMPT, checkout still being cut) has
            // no PTY to paste into.
            if app.nav.focus == Focus::Terminal && app.pane.term_locked && app.pane_accepts_input()
            {
                if let Some(term) = &app.pane.term {
                    let session = term.sref.clone();
                    let data = pasted(term.parser.screen(), &text);
                    typed_into(app, &session);
                    out.push(ClientRequest::Input { session, data });
                }
            }
        }
        Event::Resize(_, _) => app.chrome.dirty = true,
        // The terminal window took focus again — most often back from a
        // browser tab where a pull request was just merged or closed.
        Event::FocusGained => {
            app.chrome.window_focused = true;
            schedule_pull_request_refresh(app);
        }
        // …and left it: from here until it is back, a session that stops
        // to ask gets a desktop notification, since the pane can't be seen.
        Event::FocusLost => app.chrome.window_focused = false,
        _ => {}
    }
}

/// Is the next key headed for the PTY and nowhere else — a LOCKED PANE on a
/// live session, nothing over it, and nothing on screen a keypress takes
/// down (a flash, a selection highlight, a scrolled-back view, the KEY COMBO
/// DISPLAY's last chord)? True before and after a key, that key left the
/// screen exactly as it was: every hatch out of the pane fails the second
/// test, and everything a forwarded key clears fails the first.
pub(crate) fn typing_into_pane(app: &App) -> bool {
    app.pane.vim.is_none()
        && app.modals.overlay.is_none()
        && app.nav.focus == Focus::Terminal
        && app.pane.term_locked
        && !app.splash_active()
        && app.chrome.flash.is_none()
        && app.pane.term_selection.is_none()
        && app.chrome.key_combo.is_none()
        && app.pane_accepts_input()
        && app
            .pane
            .term
            .as_ref()
            .is_some_and(|t| !t.exited && t.scroll_offset() == 0)
}

/// Is `chord` one of the pane fold's (`^~`, `^``, whatever the Hotkeys tab
/// binds to it) that a LOCKED PANE lets through rather than forwards — a
/// chord with a command modifier, never one that types a character.
pub(crate) fn folds_launcher_pane(app: &App, chord: &crate::keymap::KeyChord) -> bool {
    !crate::key_combo::is_text_key(chord)
        && app
            .chrome
            .keymap
            .chords(crate::keymap::Action::ToggleLauncherPane)
            .contains(chord)
}

/// Is `chord` the full-screen toggle's (`^F`, whatever the Hotkeys tab
/// binds to it) in a form a LOCKED PANE lets through rather than forwards
/// — a chord with a command modifier, never one that types a character.
pub(crate) fn toggles_full_screen(app: &App, chord: &crate::keymap::KeyChord) -> bool {
    !crate::key_combo::is_text_key(chord)
        && app
            .chrome
            .keymap
            .chords(crate::keymap::Action::ToggleFullScreen)
            .contains(chord)
}

/// Is `chord` the PROJECT DROPDOWN's (`⌘P`, whatever the Hotkeys tab binds
/// to it) in a form a LOCKED PANE or the dropdown's own TYPE-AHEAD lets
/// through rather than takes as text — a chord with a command modifier,
/// never one that types a character.
pub(crate) fn drops_project_dropdown(app: &App, chord: &crate::keymap::KeyChord) -> bool {
    !crate::key_combo::is_text_key(chord)
        && app
            .chrome
            .keymap
            .chords(crate::keymap::Action::ProjectDropdown)
            .contains(chord)
}
