//! Immediate terminal creation feedback while the daemon starts the shell.

use super::*;
use crate::app::TerminalCreation;
use nebula_core::TerminalTab;

const MAX_QUEUED_INPUT: usize = 1024 * 1024;

pub(super) fn start(app: &mut App, worktree: WorktreeId, out: &mut Vec<ClientRequest>) {
    if !app.tree.worktrees.iter().any(|w| w.id == worktree) {
        send_with(
            app,
            out,
            PendingIntent::AttachCreated {
                focus: true,
                placeholder: None,
            },
            |req_id| ClientRequest::CreateTerminal {
                req_id,
                worktree,
                name: None,
            },
        );
        return;
    }
    let id = TerminalId::generate();
    let n = app
        .tree
        .terminals
        .iter()
        .filter(|t| t.worktree_id == worktree)
        .count()
        + 1;
    send_with(
        app,
        out,
        PendingIntent::TerminalCreated(TerminalCreation {
            placeholder: id.clone(),
            input: Vec::new(),
            cancelled: false,
        }),
        |req_id| ClientRequest::CreateTerminal {
            req_id,
            worktree: worktree.clone(),
            name: None,
        },
    );
    app.tree.terminals.push(TerminalTab {
        id: id.clone(),
        worktree_id: worktree,
        name: format!("term-{n}"),
        sort_order: 0,
        alive: false,
        run_command: None,
    });
    launcher::show_created_terminal(app);
    open_session(app, SessionRef::Terminal(id), out);
    launcher::unfold_cursor_band(app);
    app.chrome.dirty = true;
}

/// Keys and paste wait for the real id, never reaching the previous PTY.
/// Overflow rejects the whole event so a partial pasted command cannot run.
pub(super) fn buffer(app: &mut App, data: &[u8]) -> bool {
    let Some(SessionRef::Terminal(id)) = app.pane.term.as_ref().map(|t| &t.sref) else {
        return false;
    };
    let Some(create) = app
        .requests
        .pending
        .values_mut()
        .find_map(|intent| match intent {
            PendingIntent::TerminalCreated(create) if &create.placeholder == id => Some(create),
            _ => None,
        })
    else {
        return false;
    };
    if create.input.len().saturating_add(data.len()) <= MAX_QUEUED_INPUT {
        create.input.extend_from_slice(data);
    } else {
        app.chrome.flash =
            Some("terminal is still starting — wait before sending more input".into());
        app.chrome.dirty = true;
    }
    true
}

pub(super) fn finish(
    app: &mut App,
    create: TerminalCreation,
    real: TerminalId,
    follow: bool,
    out: &mut Vec<ClientRequest>,
) {
    if create.cancelled {
        close_terminal(app, real, out);
        return;
    }
    let fake = SessionRef::Terminal(create.placeholder.clone());
    let actual = SessionRef::Terminal(real.clone());
    let mut before = selection_snapshot(app);
    before.remap_session(&fake, &actual);
    let showing = app.pane.term.as_ref().is_some_and(|t| t.sref == fake);
    if app.tree.terminals.iter().any(|t| t.id == real) {
        app.tree.terminals.retain(|t| t.id != create.placeholder);
    } else if let Some(t) = app
        .tree
        .terminals
        .iter_mut()
        .find(|t| t.id == create.placeholder)
    {
        t.id = real.clone();
    }
    for sref in app.nav.last_session_for_worktree.values_mut() {
        if *sref == fake {
            *sref = actual.clone();
        }
    }
    if app.pane.attach_after_draw.as_ref() == Some(&fake) {
        app.pane.attach_after_draw = Some(actual.clone());
    }
    if showing {
        app.pane.term.as_mut().unwrap().sref = actual.clone();
    }
    reconcile_selection_inner(app, before, out);
    if follow {
        attach_created(app, EntityId::Terminal(real), true, None, true, out);
    } else if showing {
        attach_now(app, actual.clone(), out);
    }
    if !create.input.is_empty() {
        out.push(ClientRequest::Input {
            session: actual,
            data: create.input,
        });
    }
    app.chrome.dirty = true;
}

pub(super) fn discard(app: &mut App, id: &TerminalId, out: &mut Vec<ClientRequest>) {
    let before = selection_snapshot(app);
    let fake = SessionRef::Terminal(id.clone());
    if app.pane.term.as_ref().is_some_and(|t| t.sref == fake) {
        app.pane.term = None;
        app.pane.pending_attach = None;
        app.release_terminal();
        if app.nav.focus == Focus::Terminal {
            app.nav.focus = Focus::Sessions;
        }
    }
    if app.pane.attach_after_draw.as_ref() == Some(&fake) {
        app.pane.attach_after_draw = None;
    }
    app.nav
        .last_session_for_worktree
        .retain(|_, sref| sref != &fake);
    app.tree.terminals.retain(|t| &t.id != id);
    reconcile_selection(app, before, out);
    app.chrome.dirty = true;
}

pub(super) fn cancel(app: &mut App, id: &TerminalId, out: &mut Vec<ClientRequest>) -> bool {
    let pending = app
        .requests
        .pending
        .values_mut()
        .find_map(|intent| match intent {
            PendingIntent::TerminalCreated(create) if &create.placeholder == id => Some(create),
            _ => None,
        });
    let Some(create) = pending else {
        return false;
    };
    create.cancelled = true;
    create.input.clear();
    discard(app, id, out);
    true
}

#[cfg(test)]
mod tests {
    use super::super::tests::{hse, seed_tree};
    use super::*;
    use nebula_core::Entity;

    fn setup() -> (App, Vec<ClientRequest>, u64, TerminalId) {
        let mut app = App::new();
        seed_tree(&mut app);
        let mut out = Vec::new();
        super::super::create_terminal(&mut app, WorktreeId("w1".into()), &mut out);
        let (req, id) = app
            .requests
            .pending
            .iter()
            .find_map(|(req, intent)| match intent {
                PendingIntent::TerminalCreated(create) => Some((*req, create.placeholder.clone())),
                _ => None,
            })
            .unwrap();
        (app, out, req, id)
    }

    fn real(app: &mut App, name: &str) -> TerminalId {
        let id = TerminalId(name.into());
        hse(
            app,
            ServerEvent::EntityUpserted {
                entity: Entity::Terminal(TerminalTab {
                    id: id.clone(),
                    worktree_id: WorktreeId("w1".into()),
                    name: "term-1".into(),
                    sort_order: 0,
                    alive: true,
                    run_command: None,
                }),
            },
        );
        id
    }

    fn ack(app: &mut App, req_id: u64, id: &TerminalId, out: &mut Vec<ClientRequest>) {
        handle_server_event(
            app,
            ServerEvent::Ack {
                req_id,
                created: Some(EntityId::Terminal(id.clone())),
            },
            out,
        );
    }

    #[test]
    fn a_terminal_card_and_starting_pane_appear_before_any_reply() {
        let (mut app, mut out, _req, id) = setup();
        assert_eq!(app.tree.terminals.len(), 1);
        assert_eq!(
            app.pane.term.as_ref().unwrap().sref,
            SessionRef::Terminal(id.clone())
        );
        assert!(app.pane.term.as_ref().unwrap().booting && app.pane.term_locked);
        assert!(!app.pane_accepts_input());
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(190, 50)).unwrap();
        terminal.draw(|f| crate::ui::draw(f, &mut app)).unwrap();
        let rendered: String = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        assert!(
            rendered.contains("term-1") && rendered.contains("starting session…"),
            "the first frame shows the new terminal and its startup state"
        );
        app.pane.term_area = ratatui::layout::Rect::new(1, 1, 90, 30);
        sync_pty_size(&mut app, &mut out);
        assert!(!out.iter().any(|r| matches!(r,
            ClientRequest::Attach { session: SessionRef::Terminal(t), .. } |
            ClientRequest::Resize { session: SessionRef::Terminal(t), .. } |
            ClientRequest::Detach { session: SessionRef::Terminal(t), .. } if *t == id)));
    }

    #[test]
    fn startup_typing_and_paste_wait_for_the_real_terminal_and_attach() {
        let (mut app, mut out, req, fake) = setup();
        for key in [KeyCode::Char('l'), KeyCode::Char('s'), KeyCode::Enter] {
            handle_terminal_event(
                &mut app,
                Event::Key(KeyEvent::new(key, KeyModifiers::NONE)),
                &mut out,
            );
        }
        handle_terminal_event(&mut app, Event::Paste("pwd\n".into()), &mut out);
        assert!(!out.iter().any(|r| matches!(r, ClientRequest::Input { .. })));
        let id = real(&mut app, "real-terminal");
        ack(&mut app, req, &id, &mut out);
        assert_eq!(app.tree.terminals.len(), 1);
        assert_eq!(app.tree.terminals[0].id, id);
        assert!(app.pane.term_locked && !app.is_placeholder_session(&SessionRef::Terminal(fake)));
        let attach = out.iter().position(|r| matches!(r, ClientRequest::Attach { session: SessionRef::Terminal(t), .. } if t == &id)).unwrap();
        let input = out.iter().position(|r| matches!(r, ClientRequest::Input { session: SessionRef::Terminal(t), data } if t == &id && data == b"ls\rpwd\r")).unwrap();
        assert!(attach < input);
    }

    #[test]
    fn an_ack_before_the_upsert_replaces_the_stand_in_without_duplicates() {
        let (mut app, mut out, req, _fake) = setup();
        let id = TerminalId("real-terminal".into());
        ack(&mut app, req, &id, &mut out);
        real(&mut app, id.as_str());
        assert_eq!(app.tree.terminals.len(), 1);
        assert_eq!(
            app.pane.term.as_ref().unwrap().sref,
            SessionRef::Terminal(id)
        );
    }

    #[test]
    fn refusal_removes_the_card_unlocks_input_and_preserves_the_error() {
        let (mut app, mut out, req, fake) = setup();
        buffer(&mut app, b"ls\r");
        handle_server_event(
            &mut app,
            ServerEvent::Error {
                req_id: Some(req),
                message: "shell could not start".into(),
            },
            &mut out,
        );
        assert!(app.tree.terminals.is_empty());
        assert!(!app.pane.term_locked);
        assert_eq!(app.chrome.flash.as_deref(), Some("shell could not start"));
        assert!(!out.iter().any(|r| matches!(r, ClientRequest::Input { .. })));
        assert!(!out.iter().any(|r| matches!(r, ClientRequest::Detach { session: SessionRef::Terminal(id) } if id == &fake)));
    }

    #[test]
    fn navigating_away_prevents_focus_theft_but_delivers_earlier_typing_to_its_terminal() {
        let (mut app, mut out, req, _fake) = setup();
        buffer(&mut app, b"ls\r");
        handle_terminal_event(
            &mut app,
            Event::Key(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::CONTROL)),
            &mut out,
        );
        open_session(&mut app, SessionRef::Agent(AgentId("a1".into())), &mut out);
        let before = app.nav.focus;
        let id = real(&mut app, "real-terminal");
        ack(&mut app, req, &id, &mut out);
        assert_eq!(
            app.pane.term.as_ref().unwrap().sref,
            SessionRef::Agent(AgentId("a1".into()))
        );
        assert_eq!(app.nav.focus, before);
        assert!(out.iter().any(|r| matches!(r, ClientRequest::Input { session: SessionRef::Terminal(t), data } if t == &id && data == b"ls\r")));
    }

    #[test]
    fn closing_a_starting_terminal_closes_the_real_one_on_ack() {
        let (mut app, mut out, req, fake) = setup();
        buffer(&mut app, b"ls\r");
        close_terminal(&mut app, fake, &mut out);
        assert!(app.tree.terminals.is_empty());
        let id = real(&mut app, "real-terminal");
        ack(&mut app, req, &id, &mut out);
        assert!(app.tree.terminals.is_empty());
        assert!(out
            .iter()
            .any(|r| matches!(r, ClientRequest::CloseTerminal { id: t, .. } if t == &id)));
        assert!(!out.iter().any(|r| matches!(r, ClientRequest::Input { session: SessionRef::Terminal(t), .. } if t == &id)));
    }

    #[test]
    fn oversized_startup_paste_is_rejected_atomically_and_debug_hides_input() {
        let (mut app, _out, req, _fake) = setup();
        buffer(&mut app, b"private command");
        buffer(&mut app, &vec![b'x'; MAX_QUEUED_INPUT]);
        let PendingIntent::TerminalCreated(create) = &app.requests.pending[&req] else {
            panic!()
        };
        assert_eq!(create.input, b"private command");
        assert!(!format!("{create:?}").contains("private command"));
        assert!(app
            .chrome
            .flash
            .as_ref()
            .unwrap()
            .contains("still starting"));
    }
    #[test]
    fn two_pending_creates_resolve_independently_without_changing_the_latest_focus() {
        let (mut app, mut out, first_req, first_fake) = setup();
        handle_terminal_event(
            &mut app,
            Event::Key(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::CONTROL)),
            &mut out,
        );
        super::super::create_terminal(&mut app, WorktreeId("w1".into()), &mut out);
        let second_req = *app
            .requests
            .pending
            .iter()
            .find(|(req, intent)| {
                **req != first_req && matches!(intent, PendingIntent::TerminalCreated(_))
            })
            .unwrap()
            .0;
        let second_real = real(&mut app, "second-terminal");
        ack(&mut app, second_req, &second_real, &mut out);
        assert!(app.tree.terminals.iter().any(|t| t.id == first_fake));
        let first_real = real(&mut app, "first-terminal");
        ack(&mut app, first_req, &first_real, &mut out);
        assert_eq!(app.tree.terminals.len(), 2);
        assert_eq!(
            app.pane.term.as_ref().unwrap().sref,
            SessionRef::Terminal(second_real)
        );
    }

    #[test]
    fn a_starting_terminal_cannot_open_a_rename_prompt_for_its_temporary_id() {
        let (mut app, _out, _req, fake) = setup();
        open_prompt(&mut app, PromptKind::RenameTerminal { id: fake });
        assert!(app.modals.overlay.is_none());
        assert!(app
            .chrome
            .flash
            .as_deref()
            .unwrap()
            .contains("still starting"));
    }
    #[test]
    fn a_new_terminal_is_revealed_inside_a_folded_nested_thread_immediately() {
        let mut app = App::new();
        seed_tree(&mut app);
        app.tree.worktrees[0].is_main = false;
        app.launcher.launcher_nested = true;
        app.launcher.launcher_all_open = false;
        let mut out = Vec::new();
        super::super::create_terminal(&mut app, WorktreeId("w1".into()), &mut out);
        assert!(
            !app.on_folded_band(),
            "the temporary card must be visible before the daemon replies"
        );
        assert!(app.pane_shows_placeholder());
    }
}
