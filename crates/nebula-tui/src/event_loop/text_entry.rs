//! Extracted event-loop helper section.

use super::*;

pub(crate) fn bracketed(text: &str) -> Vec<u8> {
    let mut data = PASTE_START.to_vec();
    data.extend_from_slice(text.as_bytes());
    data.extend_from_slice(PASTE_END);
    data
}

/// `text` as a terminal pastes it into the program on `screen`: bracketed
/// when the program turned bracketed paste on (a shell's line editor,
/// claude, vim), so it takes the text as one block; otherwise as though
/// typed, each line break an Enter. A program that never asked — a
/// password prompt, `read` — would take the markers for part of what was
/// pasted: a token pasted into `hf auth login` came out wrapped in them
/// (#107).
pub(crate) fn pasted(screen: &vt100::Screen, text: &str) -> Vec<u8> {
    if screen.bracketed_paste() {
        bracketed(text)
    } else {
        text.replace("\r\n", "\r").replace('\n', "\r").into_bytes()
    }
}

/// Route a bracketed paste into whatever text field the open overlay has
/// live. Returns false when nothing is typing, so the paste falls through to
/// the terminal pane.
pub(crate) fn paste_into_overlay(app: &mut App, text: &str) -> bool {
    // The ISSUE EDITOR's field under the caret, or either modal's `/`
    // FILTER ROW while it has the caret — resolved against the rows, so
    // the cursor lands on the best match as a typed letter's would.
    if matches!(&app.overlay, Some(Overlay::Issues(_))) {
        return crate::issues::paste(app, text);
    }
    if matches!(&app.overlay, Some(Overlay::PullRequests(_))) {
        return crate::pr_modal::paste(app, text);
    }
    let Some(overlay) = &mut app.overlay else {
        return false;
    };
    match overlay {
        // A task box keeps the paste's line breaks, a one-line prompt
        // flattens them: the field knows which it is. A box whose text goes
        // to a local agent takes a dropped file as a copy it can find.
        Overlay::Prompt(prompt) => {
            let text = match prompt.kind.reaches_local_agent() {
                true => staged_drop(app.attachments_dir.as_deref(), text, &mut app.flash),
                false => text.to_string(),
            };
            prompt.input.insert_str(&text);
            prompt.refresh_dirs();
        }
        Overlay::Palette(palette) => {
            palette.query.insert_str(text);
            palette.apply_filter();
        }
        Overlay::ProjectPicker(picker) => {
            picker.query.insert_str(text);
            picker.apply_filter();
        }
        Overlay::Files(finder) => {
            finder.query.insert_str(text);
            finder.apply_filter();
        }
        Overlay::Grep(view) => {
            view.query.insert_str(text);
            view.run_search();
        }
        Overlay::Tree(view) => {
            view.filter.insert_str(text);
            view.apply_filter();
        }
        Overlay::Diff(view) => {
            view.filter.insert_str(text);
            activate::diff_filter_changed(view);
        }
        // The query, or the commit message while that is being typed.
        Overlay::BranchSwitch(view) => {
            if !crate::branch_switch::paste(view, text) {
                return false;
            }
        }
        // Only types while its add/edit input is open.
        Overlay::Hosts(view) => match &mut view.input {
            Some(input) => input.insert_str(text),
            None => return false,
        },
        // The name is one line; prefix and postfix keep their newlines.
        Overlay::AgentPresetEditor(editor) => {
            if !editor.paste(text) {
                return false;
            }
        }
        _ => return false,
    }
    app.dirty = true;
    true
}

// ---- the FOLLOW-UP COMPOSER ----

/// A paste while a session card's FOLLOW-UP COMPOSER is open: it lands in
/// the box, newlines and all. False when no card is expanded, so the paste
/// falls through to the terminal pane.
pub(crate) fn paste_into_follow_up(app: &mut App, text: &str) -> bool {
    if app.focus != Focus::Sessions || !app.follow_up_live() {
        return false;
    }
    let Some(follow_up) = &mut app.follow_up else {
        return false;
    };
    let text = staged_drop(app.attachments_dir.as_deref(), text, &mut app.flash);
    follow_up.input.insert_str(&text);
    app.dirty = true;
    true
}

/// A paste bound for an agent's prompt, with any file dropped in it copied
/// where the agent can find it (`dropped_files`) — a macOS screenshot's
/// thumbnail is deleted soon after the drop, and its name has a U+202F the
/// agent types back as a space. Anything but a drop comes back as it came,
/// as does everything when no `dir` is installed (the unit tests). A copy
/// that failed says so in `flash` and leaves its path as dropped.
pub(crate) fn staged_drop(
    dir: Option<&std::path::Path>,
    text: &str,
    flash: &mut Option<String>,
) -> String {
    let Some(staged) = dir.and_then(|dir| crate::dropped_files::stage(text, dir)) else {
        return text.to_string();
    };
    if let Some((name, err)) = staged.failed.first() {
        *flash = Some(format!("couldn't keep a copy of {name}: {err}"));
    }
    staged.text
}

/// One key while the composer is open and the SESSIONS PANEL has focus.
/// True when the box took it — which is nearly everything: the panel's own
/// verbs are bare letters, so a card with a live box has to swallow them or
/// typing "attach and archive it" would do both. Enter sends, Esc folds the
/// card, Shift+Enter / ⌥Enter / `^J` break the line, the readline chords
/// edit; what is left over is dropped rather than handed on, so no
/// keystroke aimed at the box ever acts on the list behind it.
///
/// Tab and ⇧Tab are the exception, and the way out that isn't Esc: the
/// panel walk still works, and the box stays open on its card behind it.
pub(crate) fn follow_up_key(app: &mut App, key: KeyEvent, out: &mut Vec<ClientRequest>) -> bool {
    if matches!(key.code, KeyCode::Tab | KeyCode::BackTab) {
        return false;
    }
    let chord = crate::keymap::KeyChord::from_event(&key);
    // The KEY COMBO DISPLAY never shows what is typed into a text field
    // (see key_combo.rs), here as in every modal.
    if !crate::key_combo::is_text_key(&chord) {
        let does = match key.code {
            KeyCode::Enter => Some("Send the follow-up"),
            KeyCode::Esc => Some("Close the follow-up"),
            _ => None,
        };
        crate::key_combo::note(app, &[chord], does);
    }
    match key.code {
        KeyCode::Esc => {
            app.follow_up = None;
            app.dirty = true;
        }
        KeyCode::Enter
            if !app
                .follow_up
                .as_ref()
                .is_some_and(|f| f.input.takes_newline(&key)) =>
        {
            send_follow_up(app, out);
        }
        _ => {
            if let Some(follow_up) = &mut app.follow_up {
                if follow_up.input.handle_key(&key).consumed() {
                    app.dirty = true;
                }
            }
        }
    }
    true
}

/// Enter in the composer: what it holds goes to the agent as its next turn
/// and the card folds back up.
///
/// The text crosses as a BRACKETED PASTE when it has line breaks — the
/// CLI (claude, codex…) then takes it as one block instead of auto-indenting
/// it into mush — and as plain bytes when it is the one line it usually is,
/// which keeps it out of the "[Pasted text]" placeholder those CLIs fold a
/// paste into. The carriage return that submits it is a second `Input` of
/// its own, so the child's read of the prompt and its read of the Enter are
/// two reads and it has the prompt in hand before the Enter arrives.
///
/// A session with no live PTY behind it — reaped by the IDLE REAPER, or
/// cold since the daemon started — is booted first and the box kept as it
/// is: the daemon drops `Input` for a session it has not spawned, and the
/// CLI that boot starts is seconds from reading anything, so the prompt
/// would be typed into a process that never saw it.
pub(crate) fn send_follow_up(app: &mut App, out: &mut Vec<ClientRequest>) {
    let Some(follow_up) = &app.follow_up else {
        return;
    };
    let text = follow_up.input.as_str().trim().to_string();
    let id = follow_up.agent.clone();
    if text.is_empty() {
        return;
    }
    // The pane goes to the session being prompted: the answer is about to
    // land there, and the user asked for it by name. (The LAUNCHER VIEW's
    // modal deliberately does not — see `send_turn`.)
    if app.tree.agents.iter().any(|a| a.id == id && a.alive) {
        attach_now(app, SessionRef::Agent(id.clone()), out);
    }
    if !matches!(send_turn(app, &id, &text, out), TurnSent::Booting) {
        app.follow_up = None;
    }
    app.dirty = true;
}

/// What one follow-up send did.
pub(crate) enum TurnSent {
    /// The prompt and its Enter are on their way down the PTY.
    Sent,
    /// The session's CLI was not up, so the daemon was told to start it
    /// and nothing was sent: the box stays as it is.
    Booting,
    /// The row went away under the box — deleted, or the daemon lost it.
    Gone,
}

/// `text` to `id` as that agent's next turn, straight down its PTY — the
/// one send behind both composers, the card's box and the LAUNCHER VIEW's
/// modal.
///
/// The text crosses as a BRACKETED PASTE when it has line breaks — the
/// CLI (claude, codex…) then takes it as one block instead of auto-indenting
/// it into mush — and as plain bytes when it is the one line it usually is,
/// which keeps it out of the "[Pasted text]" placeholder those CLIs fold a
/// paste into. The carriage return that submits it is a second `Input` of
/// its own, so the child's read of the prompt and its read of the Enter are
/// two reads and it has the prompt in hand before the Enter arrives.
///
/// It attaches only when it has to: a session with no live PTY behind it —
/// reaped by the IDLE REAPER, or cold since the daemon started — is booted
/// first and nothing sent, since the daemon drops `Input` for a session it
/// has not spawned and the CLI that boot starts is seconds from reading
/// anything. A live one is written to where it stands, so prompting a card
/// need not disturb what the pane is showing.
pub(crate) fn send_turn(
    app: &mut App,
    id: &AgentId,
    text: &str,
    out: &mut Vec<ClientRequest>,
) -> TurnSent {
    let Some(agent) = app.tree.agents.iter().find(|a| &a.id == id).cloned() else {
        return TurnSent::Gone;
    };
    let sref = SessionRef::Agent(id.clone());
    if !agent.alive {
        attach_now(app, sref, out);
        app.flash = Some(format!(
            "starting {} — press Enter again once it is up",
            agent.name
        ));
        return TurnSent::Booting;
    }
    let data = if text.contains('\n') {
        bracketed(text)
    } else {
        text.as_bytes().to_vec()
    };
    out.push(ClientRequest::Input {
        session: sref.clone(),
        data,
    });
    typed_into(app, &sref);
    out.push(ClientRequest::Input {
        session: sref,
        data: b"\r".to_vec(),
    });
    app.flash = Some(format!("sent to {}", agent.name));
    TurnSent::Sent
}

/// A key, a paste or a turn is going down `session`'s PTY: that is work in
/// its project, whose PROJECT TAB comes to the far left
/// ([`App::bring_tab_forward`]). Runs on every keystroke typed at an agent,
/// so the project already at the front costs a lookup and no allocation.
pub(crate) fn typed_into(app: &mut App, session: &SessionRef) {
    if let Some(project) = app
        .project_of_session(session)
        .filter(|p| app.launcher_tabs.first() != Some(*p))
        .cloned()
    {
        app.bring_tab_forward(&project);
    }
}

/// A session launched, a shell opened or a checkout cut in `worktree`'s
/// project: [`typed_into`]'s work, by the checkout.
pub(crate) fn worked_in(app: &mut App, worktree: &WorktreeId) {
    if let Some(project) = project_of_worktree(app, worktree) {
        app.bring_tab_forward(&project);
    }
}
