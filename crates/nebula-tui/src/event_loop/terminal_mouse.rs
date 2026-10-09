//! Extracted event-loop helper section.

use super::*;

pub(crate) fn pane_cell(area: ratatui::layout::Rect, col: u16, row: u16) -> (u16, u16) {
    let max_x = area.x + area.width.saturating_sub(1);
    let max_y = area.y + area.height.saturating_sub(1);
    (
        col.clamp(area.x, max_x) - area.x,
        row.clamp(area.y, max_y) - area.y,
    )
}

/// The modifier bits of an xterm mouse report: ⇧ 4, ⌥ 8, ^ 16.
pub(crate) fn mouse_modifier_bits(modifiers: KeyModifiers) -> u16 {
    let mut bits = 0;
    if modifiers.contains(KeyModifiers::SHIFT) {
        bits |= 4;
    }
    if modifiers.contains(KeyModifiers::ALT) {
        bits |= 8;
    }
    if modifiers.contains(KeyModifiers::CONTROL) {
        bits |= 16;
    }
    bits
}

/// One xterm mouse report in the encoding the program asked for: SGR
/// (`CSI < button ; x ; y M`, a lowercase `m` for a release) or the legacy
/// X10 bytes (`CSI M` and three offset bytes, coordinates capped at that
/// encoding's 223 limit, a release folded into button 3). `col` and `row`
/// are pane-relative cells; the report is 1-based.
pub(crate) fn mouse_report(sgr: bool, button: u16, release: bool, col: u16, row: u16) -> Vec<u8> {
    if sgr {
        let last = if release { 'm' } else { 'M' };
        format!("\x1b[<{button};{};{}{last}", col + 1, row + 1).into_bytes()
    } else {
        let button = if release {
            (button & !0b11) | 3
        } else {
            button
        };
        vec![
            0x1b,
            b'[',
            b'M',
            32 + button as u8,
            32 + (col + 1).min(223) as u8,
            32 + (row + 1).min(223) as u8,
        ]
    }
}

/// Hand the program in the pane one report of `button` at the pointer,
/// clamped to the pane the way a drag-selection's head is.
pub(crate) fn forward_mouse(
    app: &App,
    out: &mut Vec<ClientRequest>,
    sgr: bool,
    button: u16,
    release: bool,
    mouse: &MouseEvent,
) {
    if let Some(term) = &app.pane.term {
        let (col, row) = pane_cell(app.pane.term_area, mouse.column, mouse.row);
        out.push(ClientRequest::Input {
            session: term.sref.clone(),
            data: mouse_report(sgr, button, release, col, row),
        });
    }
}

/// Text under the current selection, by HISTORY LINE: rows the selection
/// scrolled past on the way (the EDGE AUTO-SCROLL, the wheel) read back
/// whole whether or not they are still on screen, and wrapped rows join.
pub(crate) fn selection_text(app: &App) -> Option<String> {
    let sel = app.pane.term_selection.as_ref()?;
    if !sel.active {
        return None;
    }
    let screen = app.pane.term.as_ref()?.parser.screen();
    let (rows, cols) = screen.size();
    let end = screen.history_end();
    if rows == 0 || cols == 0 || end == 0 {
        return None;
    }
    let ((start_col, start_line), (end_col, end_line)) = sel.bounds();
    if start_line >= end {
        return None;
    }
    let text = screen.contents_between_history(
        start_line,
        start_col.min(cols - 1),
        end_line.min(end - 1),
        // contents_between's end column is exclusive; the selection's head
        // cell is inclusive.
        (end_col + 1).min(cols),
    );
    (!text.is_empty()).then_some(text)
}

/// Complete a drag-selection: copy the text to the system clipboard and keep
/// the highlight (it clears on the next click / scroll / keypress). A drag
/// that never left its starting cell is just a click — drop it.
pub(crate) fn finish_selection(app: &mut App) {
    app.chrome.dirty = true;
    app.pane.next_drag_autoscroll = None;
    let Some(sel) = &mut app.pane.term_selection else {
        return;
    };
    if !sel.active {
        app.pane.term_selection = None;
        return;
    }
    sel.dragging = false;
    copy_selection(app);
}

/// Copy the current selection's text to the clipboard, flashing the result.
pub(crate) fn copy_selection(app: &mut App) {
    if let Some(text) = selection_text(app) {
        let label = format!("copied {} chars", text.chars().count());
        copy_and_flash(app, &text, &label);
    }
}

/// A drag report landed at `pointer` (host cells): the selection's head
/// follows it. The cell is the pane's nearest — a pointer past an edge
/// selects to that edge — and its HISTORY LINE is read at the current
/// scroll, so the head names the text under the pointer, not the row.
/// Past the pane's top or bottom edge the EDGE AUTO-SCROLL starts: one
/// step now, then `drag_autoscroll_tick` on its beat until the pointer is
/// back inside or the button comes up.
pub(crate) fn drag_select_to(app: &mut App, pointer: (u16, u16), out: &mut Vec<ClientRequest>) {
    let Some(sel) = &mut app.pane.term_selection else {
        return;
    };
    if !sel.dragging {
        return;
    }
    sel.pointer = pointer;
    place_drag_head(app);
    match edge_overshoot(app.pane.term_area, pointer.1) {
        Some(_) if app.pane.next_drag_autoscroll.is_none() => drag_autoscroll_tick(app, out),
        Some(_) => {}
        None => app.pane.next_drag_autoscroll = None,
    }
}

/// Put the dragged selection's head on the cell under its pointer, at the
/// pane's current scroll. A head that has left the anchor cell makes the
/// selection real (and it stays real if the head returns: a 1-cell
/// selection is still a selection). Only a head that moved repaints — the
/// EDGE AUTO-SCROLL calls this on every tick, scrolled or not.
pub(crate) fn place_drag_head(app: &mut App) {
    let area = app.pane.term_area;
    let Some(base) = app
        .pane
        .term
        .as_ref()
        .map(|t| t.parser.screen().history_base())
    else {
        return;
    };
    let Some(sel) = &mut app.pane.term_selection else {
        return;
    };
    let (col, row) = pane_cell(area, sel.pointer.0, sel.pointer.1);
    let head = (col, base + u64::from(row));
    if sel.head != head {
        sel.head = head;
        app.chrome.dirty = true;
    }
    if sel.head != sel.anchor {
        sel.active = true;
    }
}

/// How far past the pane's top (`true`) or bottom (`false`) edge a pointer
/// row is, in rows — None inside the pane. The rule line above the pane
/// is one row past it; the footer below, one or two.
pub(crate) fn edge_overshoot(area: ratatui::layout::Rect, row: u16) -> Option<(bool, u16)> {
    if row < area.y {
        Some((true, area.y - row))
    } else if row >= area.y + area.height {
        Some((false, row - (area.y + area.height) + 1))
    } else {
        None
    }
}

/// One EDGE AUTO-SCROLL step: scroll the history under a drag whose
/// pointer rests past the pane's edge — a line per row past it, up to
/// `DRAG_AUTOSCROLL_MAX_LINES` — put the head on the edge row's new text,
/// and book the next step. The beat runs until the pointer comes back
/// inside or the button comes up; a step with nothing left to scroll to
/// (the top of the history, the live bottom) moves nothing and paints
/// nothing.
pub(crate) fn drag_autoscroll_tick(app: &mut App, out: &mut Vec<ClientRequest>) {
    app.pane.next_drag_autoscroll = None;
    let Some(sel) = app.pane.term_selection.filter(|s| s.dragging) else {
        return;
    };
    let Some((up, distance)) = edge_overshoot(app.pane.term_area, sel.pointer.1) else {
        return;
    };
    let lines = usize::from(distance).min(DRAG_AUTOSCROLL_MAX_LINES);
    let Some(term) = &app.pane.term else {
        return;
    };
    // A step past the top of the history stops there, and past a history
    // that was let go it is what asks for it back (`scroll_pane_to`).
    let current = term.scroll_offset();
    let target = if up {
        current.saturating_add(lines)
    } else {
        current.saturating_sub(lines)
    };
    scroll_pane_to(app, target, out);
    place_drag_head(app);
    app.pane.next_drag_autoscroll = Some(std::time::Instant::now() + DRAG_AUTOSCROLL_TICK);
}

/// Scroll the pane's view of its history to `target` lines above the live
/// edge — the wheel's notch, the EDGE AUTO-SCROLL's step. The view stops
/// at the top of the history it holds (`AttachedTerm::set_scroll`), so a
/// notch past it is not a debt the way back down pays off first. Scrolling
/// up into a history that was let go asks the DAEMON for it back
/// (`rehydrate_history`), and a notch while that replay is on its way
/// moves where it lands. Paints only when the view (or where it is headed)
/// moved: a step at the top or the live bottom is nothing.
pub(crate) fn scroll_pane_to(app: &mut App, target: usize, out: &mut Vec<ClientRequest>) {
    let Some(term) = &mut app.pane.term else {
        return;
    };
    let before = term.scroll_offset();
    if term.pending_scroll.is_some() {
        term.pending_scroll = Some(target);
    } else {
        term.set_scroll(target);
        if target > before {
            rehydrate_history(app, target, out);
        }
    }
    if app
        .pane
        .term
        .as_ref()
        .is_some_and(|t| t.scroll_offset() != before)
    {
        app.chrome.dirty = true;
    }
}

/// Select the maximal run of non-blank cells around `cell` on its row (a
/// double-click "word": handles identifiers, paths, and URLs alike).
pub(crate) fn select_word_at(app: &mut App, cell: (u16, u16)) {
    let Some(term) = &app.pane.term else {
        return;
    };
    let screen = term.parser.screen();
    let (rows, cols) = screen.size();
    let (col, row) = cell;
    if row >= rows || col >= cols {
        return;
    }
    let is_word = |c: u16| {
        screen
            .cell(row, c)
            .is_some_and(|cell| !cell.contents().trim().is_empty())
    };
    if !is_word(col) {
        return;
    }
    let mut start = col;
    while start > 0 && is_word(start - 1) {
        start -= 1;
    }
    let mut end = col;
    while end + 1 < cols && is_word(end + 1) {
        end += 1;
    }
    let line = screen.history_base() + u64::from(row);
    app.pane.term_selection = Some(TermSelection {
        anchor: (start, line),
        head: (end, line),
        dragging: false,
        active: true,
        pointer: (0, 0),
    });
    copy_selection(app);
}

/// Copy `text` to the clipboard the user is actually looking at, flashing
/// `label` when it goes out.
///
/// Two routes, because "the clipboard" is not always on this machine. Run
/// locally, we shell out to the platform tool (`copy_to_clipboard`). Run over
/// `nebula ssh`, that tool would target the *remote* box — and a headless VM
/// has no clipboard at all, which is what used to surface as "copy failed
/// (clipboard unavailable)". There we ask the terminal on the near end of the
/// ssh connection instead, via OSC 52; the main loop writes the request.
///
/// OSC 52 is also the fallback for a local host with no display tool, and it
/// is silently dropped by terminals that do not implement it (Terminal.app),
/// so the flash names the route it took rather than claiming success.
pub(crate) fn copy_and_flash(app: &mut App, text: &str, label: &str) {
    // Unit tests exercise the copy flows; don't clobber the developer's real
    // clipboard, and don't depend on their terminal or their $SSH_TTY.
    if cfg!(test) {
        app.chrome.flash = Some(label.to_string());
        return;
    }
    let via_terminal = format!("{label} (via terminal)");
    // `pbcopy` is a process to start and a pasteboard server to reach —
    // ten to twenty milliseconds the loop used to spend on mouse-up. It
    // all but never fails here, so the flash says so now, and the rare
    // failure falls back to the terminal's OSC 52 when it is known.
    if let (false, Some(jobs)) = (app.chrome.is_remote, app.jobs.view_jobs.clone()) {
        app.chrome.flash = Some(label.to_string());
        let text = text.to_string();
        jobs.run(move || {
            (!copy_to_clipboard(&text)).then(|| crate::view_jobs::Answer::ClipboardViaTerminal {
                payload: base64_encode(text.as_bytes()),
                flash: via_terminal,
            })
        });
        return;
    }
    if !app.chrome.is_remote && copy_to_clipboard(text) {
        app.chrome.flash = Some(label.to_string());
        return;
    }
    app.chrome.pending_clipboard = Some(base64_encode(text.as_bytes()));
    app.chrome.flash = Some(via_terminal);
}

/// Base64 (RFC 4648, padded) for OSC 52 payloads.
pub(crate) fn base64_encode(bytes: &[u8]) -> String {
    use base64::Engine as _;
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

/// Copy to *this machine's* system clipboard.
/// macOS: pbcopy. Linux: wl-copy on Wayland, xclip (or xsel) on X11.
pub(crate) fn copy_to_clipboard(text: &str) -> bool {
    // Unit tests exercise the selection flow; don't clobber the developer's
    // real clipboard from `cargo test`.
    if cfg!(test) {
        return true;
    }

    use std::io::Write as _;
    use std::process::{Command, Stdio};

    let copy_via = |cmd: &str, args: &[&str]| -> bool {
        let Ok(mut child) = Command::new(cmd)
            .args(args)
            .stdin(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
        else {
            return false;
        };
        let wrote = child
            .stdin
            .take()
            .is_some_and(|mut stdin| stdin.write_all(text.as_bytes()).is_ok());
        wrote && child.wait().is_ok_and(|status| status.success())
    };

    #[cfg(target_os = "macos")]
    {
        copy_via("pbcopy", &[])
    }

    #[cfg(not(target_os = "macos"))]
    {
        if std::env::var_os("WAYLAND_DISPLAY").is_some() {
            return copy_via("wl-copy", &[]);
        }
        // X11: prefer xclip, fall back to xsel
        if copy_via("xclip", &["-selection", "clipboard"]) {
            return true;
        }
        copy_via("xsel", &["--clipboard", "--input"])
    }
}

/// Open a URL in the default browser via open(1) (this tool targets macOS).
/// The scheme allowlist is defense in depth — the link scanner only ever
/// produces http(s) URLs, but the text originates from untrusted PTY output.
pub(crate) fn open_url(url: &str) -> bool {
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        return false;
    }
    if cfg!(test) {
        return true;
    }
    #[cfg(target_os = "macos")]
    {
        let mut open = std::process::Command::new("open");
        open.arg(url);
        spawn_and_reap(open, "open url")
    }
    #[cfg(not(target_os = "macos"))]
    {
        false
    }
}

/// Start `command` and let it finish on its own: true once it is running.
/// `open` spends 50 to 150 ms talking to LaunchServices before it exits,
/// and the key that asked for a browser tab used to spend them with it —
/// the loop frozen, the flash unpainted. Whether it then succeeds is not
/// something the keypress can wait to learn; a failure is logged (the
/// `spawn_open_command` rule), and the reaper thread is what keeps the
/// child from lingering as a zombie.
pub(crate) fn spawn_and_reap(mut command: std::process::Command, what: &'static str) -> bool {
    use std::process::Stdio;
    let child = command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
    let Ok(mut child) = child else {
        return false;
    };
    std::thread::spawn(move || match child.wait() {
        Ok(status) if !status.success() => tracing::warn!(what, %status, "open failed"),
        Err(err) => tracing::warn!(what, %err, "open not reaped"),
        Ok(_) => {}
    });
    true
}

/// Two clicks on the same cell within this window make a double-click.
const DOUBLE_CLICK: Duration = Duration::from_millis(400);

/// Whether this click on `key` is the second of a double-click. The slot
/// is consumed either way: a double-click is spent, so a third click starts
/// over; a single click re-arms the slot with itself for the next one.
pub(crate) fn is_double_click<T: PartialEq>(
    slot: &mut Option<(std::time::Instant, T)>,
    key: T,
) -> bool {
    let now = std::time::Instant::now();
    let double = slot
        .take()
        .is_some_and(|(at, id)| id == key && now.duration_since(at) <= DOUBLE_CLICK);
    if !double {
        *slot = Some((now, key));
    }
    double
}

/// The two touching border cells at a vertical panel boundary `bx`, bounded
/// by `area` — the shared grab-zone rule for every splitter.
pub(crate) fn on_vsplit(bx: u16, area: ratatui::layout::Rect, column: u16, row: u16) -> bool {
    area.width > 0
        && row >= area.y
        && row < area.y + area.height
        && column.saturating_add(1) >= bx
        && column <= bx
}

/// Whether the mouse is somewhere a horizontal resize could start (or one is
/// already in progress): a main-screen splitter, or the file-list border of
/// the diff / tree modals.
pub(crate) fn pointer_wants_resize(app: &App, column: u16, row: u16) -> bool {
    if app.pane.vim.is_some() {
        return false;
    }
    match &app.modals.overlay {
        Some(Overlay::Diff(view)) => {
            view.files_drag.is_some() || on_vsplit(view.splitter_x(), view.area, column, row)
        }
        Some(Overlay::Tree(view)) => {
            view.files_drag.is_some() || on_vsplit(view.splitter_x(), view.area, column, row)
        }
        Some(_) => false,
        None => false,
    }
}

/// Track the mouse for the resize affordances: the pointer shape the outer
/// terminal should show (col-resize over any draggable boundary) and the
/// main-screen grip highlight. Runs on every mouse event — including plain
/// motion, the only kind that arrives with nothing pressed. Terminals that
/// don't report motion (Terminal.app) still pass through here on clicks and
/// drags, so drag state keeps the shape honest where hover can't.
pub(crate) fn update_pointer(app: &mut App, mouse: &MouseEvent) {
    // One hit-test feeds both boundaries and both grips. The modals draw
    // their own file-list edge outside the hit map, so `pointer_wants_resize`
    // still measures that one itself.
    let on_panels = app.pane.vim.is_none() && app.modals.overlay.is_none();
    let hit = on_panels
        .then(|| app.hit_at(mouse.column, mouse.row))
        .flatten();
    // The LAUNCHER VIEW's pane edge gets the arrows for the way it moves —
    // up and down under the cards, side to side beside them — and it is
    // asked first, the two boundaries never sharing a cell.
    let on_pane_edge = on_panels
        && (app.launcher.launcher_pane_drag.is_some()
            || matches!(&hit, Some(HitTarget::LauncherPaneSplitter)));
    app.chrome.pointer_shape = if on_pane_edge && app.launcher_pane_side().beside() {
        PointerShape::ColResize
    } else if on_pane_edge {
        PointerShape::RowResize
    } else if pointer_wants_resize(app, mouse.column, mouse.row) {
        PointerShape::ColResize
    } else {
        PointerShape::Default
    };
    if app.launcher.hover_launcher_pane != on_pane_edge {
        app.launcher.hover_launcher_pane = on_pane_edge;
        app.chrome.dirty = true;
    }
    // The header's PROJECT TABS (each tab, its `×`, the `+` after them)
    // and a full-screen session's `‹ sessions` are the other things on
    // the main screen a click acts on without the cursor moving there
    // first, and nothing about a word says it is a button. So the one
    // under the pointer is marked while it is there (the draw reads this
    // in `ui::launcher_view`). Only those take it: every other target is
    // a card, which has its own highlight, or the background.
    // The header's PR & ISSUE COUNTS are buttons of the same kind, and
    // take the same underline; so are the pane's CLOSE and SIDE BUTTONS
    // and the footer's memory readout. A card's PULL REQUEST LINE is a
    // link on a card whose own highlight marks the cursor, not the
    // pointer, so the line takes it for itself.
    let crumb = hit.filter(|h| {
        matches!(
            h,
            HitTarget::LauncherTab(_)
                | HitTarget::LauncherBandPr(_)
                | HitTarget::LauncherCardIssue(_)
                | HitTarget::LauncherStripLeft(_)
                | HitTarget::LauncherStripRight(_)
                | HitTarget::LauncherBandMore(_)
                | HitTarget::LauncherBandFold(_)
                | HitTarget::LauncherThreadPr(_)
                | HitTarget::LauncherTabClose(_)
                | HitTarget::LauncherPaneClose
                | HitTarget::LauncherPaneSide
                | HitTarget::LauncherPaneZoom
                | HitTarget::LauncherTabAdd
                | HitTarget::LauncherTabMore
                | HitTarget::LauncherCrumb
                | HitTarget::LauncherPullRequests
                | HitTarget::LauncherIssues
                | HitTarget::LauncherWelcomePrompt
                | HitTarget::FooterUsage
        )
    });
    // The ISSUES and PULL REQUESTS MODALS' `↗ open in browser` button is
    // the one thing on a modal a click acts on without the cursor moving
    // there first, so it takes the same mark; the modals hold their own
    // rects outside the hit map, as they do their list edges.
    let crumb = crumb.or_else(|| {
        crate::ui::browser_button_under(
            app,
            ratatui::layout::Position::new(mouse.column, mouse.row),
        )
    });
    if app.launcher.hover_crumb != crumb {
        app.launcher.hover_crumb = crumb;
        app.chrome.dirty = true;
    }
}
