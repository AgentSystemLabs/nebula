use super::*;
use crate::app::FeedbackAlert;
use nebula_core::{AgentId, LinkId, ServerEvent, SessionRef};
use ratatui::backend::TestBackend;
use ratatui::Terminal;

/// A second agent, `a2` / "agent-2", under the seeded worktree.
/// A live session in `worktree`, so the GRID lists that checkout.
fn seed_agent_in(app: &mut App, id: &str, worktree: &nebula_core::WorktreeId) {
    use nebula_core::{Agent, AgentStatus, Entity};
    hse(
        app,
        ServerEvent::EntityUpserted {
            entity: Entity::Agent(Agent {
                id: AgentId(id.into()),
                worktree_id: worktree.clone(),
                name: id.into(),
                status: AgentStatus::Finished,
                archived: false,
                archived_at: 0,
                unseen: false,
                kind: nebula_core::AgentKind::Claude,
                custom_harness: None,
                model: None,
                effort: None,
                session_id: None,
                cloud_session_id: None,
                sort_order: 9,
                status_changed_at: 0,
                alive: true,
                issue_url: None,
                recent_prompts: Vec::new(),
            }),
        },
    );
}

fn seed_second_agent(app: &mut App, status: nebula_core::AgentStatus) {
    use nebula_core::{Agent, Entity, WorktreeId};
    hse(
        app,
        ServerEvent::EntityUpserted {
            entity: Entity::Agent(Agent {
                id: AgentId("a2".into()),
                worktree_id: WorktreeId("w1".into()),
                name: "agent-2".into(),
                status,
                archived: false,
                archived_at: 0,
                unseen: false,
                kind: nebula_core::AgentKind::Claude,
                custom_harness: None,
                model: None,
                effort: None,
                session_id: None,
                cloud_session_id: None,
                sort_order: 1,
                status_changed_at: 0,
                alive: true,
                issue_url: None,
                recent_prompts: Vec::new(),
            }),
        },
    );
}

/// Row index of an agent in the Sessions panel.
fn row_of(app: &App, id: &str) -> usize {
    let sref = SessionRef::Agent(AgentId(id.into()));
    app.visible_session_rows()
        .iter()
        .position(|r| r.sref().as_ref() == Some(&sref))
        .unwrap_or_else(|| panic!("{id} has no row"))
}

/// A session that goes green while the pane shows something else counts
/// on its worktree and project rows — the number of terminals to go
/// read — and stays counted until the cursor walks onto it. Landing
/// previews it, so that is the moment the counts come down: locally on
/// the spot, and at the daemon for every other client.
#[test]
fn an_unwatched_finish_counts_until_the_cursor_lands_on_it() {
    use nebula_core::{AgentStatus, ProjectId, WorktreeId};
    let mut app = App::new();
    seed_tree(&mut app);
    seed_second_agent(&mut app, AgentStatus::Running);
    let a2 = AgentId("a2".into());
    let (w1, p1) = (WorktreeId("w1".into()), ProjectId("p1".into()));
    app.term = Some(AttachedTerm::new(
        SessionRef::Agent(AgentId("a1".into())),
        40,
        10,
    ));
    app.focus = Focus::Sessions;
    app.sel_session = row_of(&app, "a1");

    hse(
        &mut app,
        ServerEvent::StatusChanged {
            agent: a2.clone(),
            status: AgentStatus::Finished,
            changed_at: crate::app::now_ms(),
            unseen: true,
        },
    );
    assert_eq!(app.worktree_unseen(&w1), 1, "one terminal to go read");
    assert_eq!(app.project_unseen(&p1), 1);
    assert_eq!(
        app.sel_session,
        row_of(&app, "a1"),
        "the cursor stayed on the session it was on"
    );

    let mut out = Vec::new();
    let delta = row_of(&app, "a2") as i64 - app.sel_session as i64;
    move_selection(&mut app, delta, &mut out);
    assert_eq!(app.sel_session, row_of(&app, "a2"));
    assert_eq!(app.worktree_unseen(&w1), 0, "landing on the row reads it");
    assert_eq!(app.project_unseen(&p1), 0);
    assert!(
        out.iter()
            .any(|r| matches!(r, ClientRequest::MarkAgentSeen { id } if *id == a2)),
        "the daemon is told: {out:?}"
    );

    // Walking off and back sends nothing: there is nothing left to clear.
    out.clear();
    move_selection(&mut app, -delta, &mut out);
    move_selection(&mut app, delta, &mut out);
    assert!(
        !out.iter()
            .any(|r| matches!(r, ClientRequest::MarkAgentSeen { .. })),
        "{out:?}"
    );
}

/// The DONE SOUND rings on the RUNNING / NEEDS FEEDBACK → FINISHED
/// edge only — not on the way into NEEDS FEEDBACK, not on a re-stamp
/// of a row already finished — and the flag is one bool, so a frame
/// with several finishes rings once.
#[test]
fn a_finish_rings_the_done_sound_once() {
    use nebula_core::AgentStatus;
    let mut app = App::new();
    seed_tree(&mut app);
    seed_second_agent(&mut app, AgentStatus::Running);
    assert!(!app.pending_ding, "the snapshot is silent");
    let a2 = AgentId("a2".into());
    let flip = |status: AgentStatus| ServerEvent::StatusChanged {
        agent: a2.clone(),
        status,
        changed_at: crate::app::now_ms(),
        unseen: status == AgentStatus::Finished,
    };

    hse(&mut app, flip(AgentStatus::NeedsFeedback));
    assert!(!app.pending_ding, "waiting on the user is not done");
    hse(&mut app, flip(AgentStatus::Finished));
    assert!(app.pending_ding, "NEEDS FEEDBACK -> FINISHED rings");

    app.pending_ding = false;
    hse(&mut app, flip(AgentStatus::Finished));
    assert!(!app.pending_ding, "a re-stamp of a finished row is silent");

    hse(&mut app, flip(AgentStatus::Running));
    assert!(!app.pending_ding);
    // On screen or not makes no difference to the sound.
    app.term = Some(AttachedTerm::new(SessionRef::Agent(a2.clone()), 40, 10));
    hse(&mut app, flip(AgentStatus::Finished));
    assert!(
        app.pending_ding,
        "RUNNING -> FINISHED rings, even on screen"
    );
    hse(&mut app, flip(AgentStatus::Running));
    hse(&mut app, flip(AgentStatus::Finished));
    assert!(
        app.pending_ding,
        "two finishes in a frame are still one ding"
    );
}

/// A turn that finishes in the pane the user is already looking at was
/// watched: the flag is dropped the moment it arrives, so it never
/// counts anywhere, and the daemon is told so every client agrees.
#[test]
fn a_finish_in_the_pane_on_screen_is_already_seen() {
    use nebula_core::{AgentStatus, WorktreeId};
    let mut app = App::new();
    seed_tree(&mut app);
    seed_second_agent(&mut app, AgentStatus::Running);
    let a2 = AgentId("a2".into());
    app.term = Some(AttachedTerm::new(SessionRef::Agent(a2.clone()), 40, 10));

    let mut out = Vec::new();
    handle_server_event(
        &mut app,
        ServerEvent::StatusChanged {
            agent: a2.clone(),
            status: AgentStatus::Finished,
            changed_at: crate::app::now_ms(),
            unseen: true,
        },
        &mut out,
    );
    assert_eq!(app.worktree_unseen(&WorktreeId("w1".into())), 0);
    assert!(
        out.iter()
            .any(|r| matches!(r, ClientRequest::MarkAgentSeen { id } if *id == a2)),
        "{out:?}"
    );
}

/// The FEEDBACK SOUND queues on the edge into NEEDS FEEDBACK only — not
/// on a re-stamp of a row already red, not on the way out to FINISHED
/// (that edge is the DONE SOUND's) — and every session that stops in
/// one frame is queued once, named as its row reads, so the desktop
/// notification can say who and where. The frame's drain rings once
/// for the lot.
#[test]
fn a_turn_stopping_to_ask_queues_the_feedback_alert() {
    use nebula_core::AgentStatus;
    let mut app = App::new();
    seed_tree(&mut app);
    seed_second_agent(&mut app, AgentStatus::Running);
    assert!(app.pending_feedback.is_empty(), "the snapshot is silent");
    let flip = |agent: &str, status: AgentStatus| ServerEvent::StatusChanged {
        agent: AgentId(agent.into()),
        status,
        changed_at: crate::app::now_ms(),
        unseen: status == AgentStatus::Finished,
    };

    hse(&mut app, flip("a2", AgentStatus::NeedsFeedback));
    assert_eq!(
        app.pending_feedback,
        vec![FeedbackAlert {
            session: "agent-2".into(),
            place: "demo · main".into(),
        }]
    );
    assert!(!app.pending_ding, "waiting on the user is not done");

    hse(&mut app, flip("a2", AgentStatus::NeedsFeedback));
    assert_eq!(
        app.pending_feedback.len(),
        1,
        "a re-stamp of a red row is silent"
    );

    app.pending_feedback.clear();
    hse(&mut app, flip("a2", AgentStatus::Finished));
    assert!(
        app.pending_feedback.is_empty(),
        "leaving red is the done sound's edge"
    );
    assert!(app.pending_ding);

    // Two sessions stopping in one frame: two names for the
    // notification; the drain plays the sound once for both.
    hse(&mut app, flip("a1", AgentStatus::Running));
    hse(&mut app, flip("a1", AgentStatus::NeedsFeedback));
    hse(&mut app, flip("a2", AgentStatus::Running));
    hse(&mut app, flip("a2", AgentStatus::NeedsFeedback));
    let names: Vec<_> = app
        .pending_feedback
        .iter()
        .map(|a| a.session.as_str())
        .collect();
    assert_eq!(names, ["agent-1", "agent-2"]);
}

fn key(code: KeyCode, mods: KeyModifiers) -> Event {
    Event::Key(KeyEvent::new(code, mods))
}

fn locked_pane_app() -> App {
    let mut app = App::new();
    seed_tree(&mut app);
    app.term = Some(AttachedTerm::new(
        SessionRef::Agent(AgentId("a1".into())),
        40,
        10,
    ));
    app.focus = Focus::Terminal;
    app.term_locked = true;
    // The tabs as a draw leaves them: the pane's project already
    // leads, so a key typed at it moves no tab.
    app.settle_project_tabs();
    app.dirty = false;
    app
}

/// TYPING ECHO: a key that only goes to the PTY leaves the screen as it
/// was, so it asks for no frame — the frame that matters is the one
/// the PTY's answer asks for a moment later, and an identical one
/// painted first only stood in its way.
#[test]
fn a_key_that_only_goes_to_the_pty_asks_for_no_frame() {
    let mut app = locked_pane_app();
    let mut out = Vec::new();
    handle_terminal_event(
        &mut app,
        key(KeyCode::Char('x'), KeyModifiers::NONE),
        &mut out,
    );
    assert!(
        matches!(out.as_slice(), [ClientRequest::Input { .. }]),
        "the key went to the PTY: {out:?}"
    );
    assert!(!app.dirty, "and changed nothing on screen");
}

/// …but whatever a forwarded key takes down is a change, and is
/// painted: a flash, a selection highlight, a scrolled-back view.
#[test]
fn a_forwarded_key_that_clears_something_still_paints() {
    let mut out = Vec::new();

    let mut app = locked_pane_app();
    app.flash = Some("copied".into());
    handle_terminal_event(
        &mut app,
        key(KeyCode::Char('x'), KeyModifiers::NONE),
        &mut out,
    );
    assert!(app.dirty && app.flash.is_none(), "the flash came down");

    let mut app = locked_pane_app();
    if let Some(term) = &mut app.term {
        term.parser.process(&b"line\r\n".repeat(40));
        term.set_scroll(3);
        assert!(term.scroll_offset() > 0);
    }
    handle_terminal_event(
        &mut app,
        key(KeyCode::Char('x'), KeyModifiers::NONE),
        &mut out,
    );
    assert!(app.dirty, "typing left the scrollback for the live edge");
    assert_eq!(app.term.as_ref().map(|t| t.scroll_offset()), Some(0));
}

/// The hatch out of the pane is not a forwarded key: FOCUS moves, and
/// that is a frame.
#[test]
fn the_unlock_key_in_a_locked_pane_paints() {
    let mut app = locked_pane_app();
    let mut out = Vec::new();
    handle_terminal_event(
        &mut app,
        key(KeyCode::Char('q'), KeyModifiers::CONTROL),
        &mut out,
    );
    assert!(out.is_empty(), "nothing went to the PTY");
    assert!(!app.term_locked && app.dirty);
}

/// A turn that stops to ask in the pane the user is locked into typing
/// at, terminal window focused, is already in front of them: nothing
/// is queued. Previewing that pane from a panel (unlocked) still
/// rings — the cursor may be on another row — and so does a locked
/// pane in a window that lost focus, which is the whole point.
#[test]
fn a_red_turn_in_the_pane_you_are_typing_at_is_silent() {
    use nebula_core::AgentStatus;
    let mut app = App::new();
    seed_tree(&mut app);
    seed_second_agent(&mut app, AgentStatus::Running);
    let a2 = AgentId("a2".into());
    let flip = |status: AgentStatus| ServerEvent::StatusChanged {
        agent: a2.clone(),
        status,
        changed_at: crate::app::now_ms(),
        unseen: false,
    };
    app.term = Some(AttachedTerm::new(SessionRef::Agent(a2.clone()), 40, 10));
    app.focus = Focus::Terminal;
    app.term_locked = true;
    assert!(
        app.window_focused,
        "a fresh TUI assumes the window has focus"
    );

    hse(&mut app, flip(AgentStatus::NeedsFeedback));
    assert!(
        app.pending_feedback.is_empty(),
        "the prompt is on screen, under the user's hands"
    );

    // Unlocked: the pane merely previews it, the user is on the panels.
    hse(&mut app, flip(AgentStatus::Running));
    app.term_locked = false;
    hse(&mut app, flip(AgentStatus::NeedsFeedback));
    assert_eq!(app.pending_feedback.len(), 1, "previewing is not typing at");

    // Locked, but the terminal window went to the background.
    app.pending_feedback.clear();
    hse(&mut app, flip(AgentStatus::Running));
    app.term_locked = true;
    let mut out = Vec::new();
    handle_terminal_event(&mut app, Event::FocusLost, &mut out);
    assert!(!app.window_focused);
    hse(&mut app, flip(AgentStatus::NeedsFeedback));
    assert_eq!(
        app.pending_feedback.len(),
        1,
        "nobody is looking at that pane"
    );
    handle_terminal_event(&mut app, Event::FocusGained, &mut out);
    assert!(app.window_focused);

    // Another session's pane on screen, locked, makes no difference.
    app.pending_feedback.clear();
    app.term = Some(AttachedTerm::new(
        SessionRef::Agent(AgentId("a1".into())),
        40,
        10,
    ));
    hse(&mut app, flip(AgentStatus::Running));
    hse(&mut app, flip(AgentStatus::NeedsFeedback));
    assert_eq!(app.pending_feedback.len(), 1);
}

/// A session with no live PTY behind it — reaped, or not booted since
/// the daemon started — wears a gray dot whatever its last status was,
/// and takes its color back once it is warm again. An unread finish
/// keeps its loud `done` badge while cold: the dot says the process is
/// gone, the badge that there is still a result to read. A Cloud row has
/// no local PTY to be warm, so it keeps its status color.
#[test]
fn cold_session_dot_is_gray_until_warm() {
    use nebula_core::{AgentStatus, Entity};
    let mut app = App::new();
    seed_tree(&mut app);
    seed_second_agent(&mut app, AgentStatus::Finished);
    // The band open, so both cards are on screen whichever the
    // cursor's row keeps on its strip.
    app.launcher_expanded = Some(nebula_core::WorktreeId("w1".into()));
    let a2 = AgentId("a2".into());
    // The agent-2 row's dot color, and the row's text.
    let dot = |app: &mut App| {
        let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
        terminal.draw(|f| ui::draw(f, app)).unwrap();
        let text = buffer_text(&terminal);
        let (x, y) = find_cell(&terminal, "agent-2");
        let cell = terminal.backend().buffer()[(x - 2, y)].clone();
        assert_eq!(cell.symbol(), "●", "{text}");
        let row = text.lines().find(|l| l.contains("agent-2")).unwrap();
        (cell.fg, row.to_string())
    };
    let upsert = |app: &mut App, edit: &dyn Fn(&mut nebula_core::Agent)| {
        let mut agent = app
            .tree
            .agents
            .iter()
            .find(|a| a.id.0 == "a2")
            .unwrap()
            .clone();
        edit(&mut agent);
        hse(
            app,
            ServerEvent::EntityUpserted {
                entity: Entity::Agent(agent),
            },
        );
    };

    let (fg, row) = dot(&mut app);
    assert_eq!(fg, app.theme.ok, "warm: the finished green: {row}");

    upsert(&mut app, &|a| a.alive = false);
    let (fg, row) = dot(&mut app);
    assert_eq!(fg, app.theme.dim, "cold: gray: {row}");

    hse(
        &mut app,
        ServerEvent::StatusChanged {
            agent: a2.clone(),
            status: AgentStatus::Finished,
            changed_at: crate::app::now_ms(),
            unseen: true,
        },
    );
    let (fg, row) = dot(&mut app);
    assert_eq!(fg, app.theme.dim, "cold wins over unread: {row}");
    let tail = &row[row.find("agent-2").unwrap()..];
    assert!(tail.contains(" done"), "the badge still says so: {row}");

    upsert(&mut app, &|a| a.alive = true);
    let (fg, row) = dot(&mut app);
    assert_eq!(fg, app.theme.done, "warm again: unread blue: {row}");

    upsert(&mut app, &|a| {
        a.alive = false;
        a.cloud_session_id = Some("session_016SiQW5Lem2LbnUf1A3undt".into());
    });
    let (fg, row) = dot(&mut app);
    assert_eq!(fg, app.theme.done, "a cloud row is never cold: {row}");
}

/// The grid's beat asks the daemon after every terminal the last
/// frame drew but the one the pane is on and the dead — each ask
/// carrying the ring end its card last heard — and the answer lands
/// as the card's lines, laid out at the PTY's width in the colours
/// they were printed in. An answer with no bytes, or none at all once
/// the shell is gone, leaves them.
#[test]
fn terminal_tails_are_asked_after_the_drawn_cards_and_land_as_lines() {
    use crate::app::TerminalTail;
    use nebula_core::{OutputTail, TerminalTab};
    let mut app = App::new();
    let tab = |id: &str, alive: bool| TerminalTab {
        id: TerminalId(id.into()),
        worktree_id: WorktreeId("w1".into()),
        name: id.into(),
        sort_order: 0,
        alive,
        run_command: None,
    };
    app.tree.terminals = vec![tab("t1", true), tab("t2", true), tab("t3", false)];
    app.term = Some(AttachedTerm::new(
        SessionRef::Terminal(TerminalId("t2".into())),
        80,
        24,
    ));
    app.terminal_tails.insert(
        TerminalId("t1".into()),
        TerminalTail {
            lines: vec!["old".into()],
            end_seq: 40,
        },
    );
    // As a frame leaves them: t1 twice (a band's strip and the pane's
    // header could both draw it), the attached t2, the dead t3.
    app.tail_cards = vec![
        TerminalId("t1".into()),
        TerminalId("t1".into()),
        TerminalId("t2".into()),
        TerminalId("t3".into()),
    ];
    let mut out = Vec::new();
    request_terminal_tails(&mut app, &mut out);
    let asks: Vec<(&SessionRef, Option<u64>)> = out
        .iter()
        .map(|r| match r {
            ClientRequest::TailOutput {
                session, after_seq, ..
            } => (session, *after_seq),
            other => panic!("expected TailOutput, got {other:?}"),
        })
        .collect();
    assert_eq!(
        asks,
        vec![(&SessionRef::Terminal(TerminalId("t1".into())), Some(40))],
        "one ask, for the live terminal the pane is not on, from where it left off"
    );
    assert_eq!(
        app.tail_cards.len(),
        4,
        "the frame's notes are read, not taken: an idle grid keeps asking"
    );
    let req_id = match &out[0] {
        ClientRequest::TailOutput { req_id, .. } => *req_id,
        other => panic!("{other:?}"),
    };
    assert!(app.pending.contains_key(&req_id));

    // The answer: a shell's last lines, wrapped at the PTY's width.
    app.dirty = false;
    hse(
        &mut app,
        ServerEvent::OutputTail {
            req_id,
            session: SessionRef::Terminal(TerminalId("t1".into())),
            tail: Some(OutputTail {
                cols: 10,
                rows: 5,
                end_seq: 70,
                data: b"$ npm test\r\n\x1b[32mok\x1b[0m 12\r\n\r\n$ ".to_vec(),
            }),
        },
    );
    assert!(
        !app.pending.contains_key(&req_id),
        "the slot is cleared by hand"
    );
    let tail = &app.terminal_tails[&TerminalId("t1".into())];
    let texts: Vec<String> = tail.lines.iter().map(|r| r.text()).collect();
    assert_eq!(texts, ["$ npm test", "ok 12", "$"]);
    assert_eq!(
        tail.lines[1].runs[0],
        (
            "ok".to_string(),
            ratatui::style::Style::default().fg(ratatui::style::Color::Indexed(2))
        ),
        "the lines keep the colours they were printed in"
    );
    assert_eq!(tail.end_seq, 70);
    assert!(app.dirty, "the card changed");

    // Nothing new: the lines stay, the frame is spared.
    app.dirty = false;
    hse(
        &mut app,
        ServerEvent::OutputTail {
            req_id: 999,
            session: SessionRef::Terminal(TerminalId("t1".into())),
            tail: Some(OutputTail {
                cols: 10,
                rows: 5,
                end_seq: 70,
                data: Vec::new(),
            }),
        },
    );
    assert_eq!(app.terminal_tails[&TerminalId("t1".into())].lines.len(), 3);
    assert!(!app.dirty);
    // The shell gone: likewise.
    hse(
        &mut app,
        ServerEvent::OutputTail {
            req_id: 1000,
            session: SessionRef::Terminal(TerminalId("t1".into())),
            tail: None,
        },
    );
    assert_eq!(app.terminal_tails[&TerminalId("t1".into())].lines.len(), 3);
    // The row removed: the lines go with it.
    hse(
        &mut app,
        ServerEvent::EntityRemoved {
            id: EntityId::Terminal(TerminalId("t1".into())),
        },
    );
    assert!(!app.terminal_tails.contains_key(&TerminalId("t1".into())));
}

pub(super) fn hse(app: &mut App, ev: ServerEvent) {
    let mut out = Vec::new();
    handle_server_event(app, ev, &mut out);
}

/// The pane paints its own cursor and the host's stays hidden — but
/// where the hidden one sits still matters: a CJK input method anchors
/// its composition, the preedit text and the candidate window, to the
/// hardware cursor's cell, drawn or not. Left where the frame's last
/// diff run ended, Japanese preedit came up at the edge of the window
/// (#53). A frame parks it on the attached PTY's cursor instead — the
/// row and column the PTY reports, a wide character counting for two.
#[test]
fn a_frame_parks_the_host_cursor_on_the_attached_ptys_cursor() {
    use crate::app::{AttachedTerm, Focus};
    use ratatui::layout::Position;
    let mut app = App::new();
    seed_tree(&mut app);
    let mut term = AttachedTerm::new(SessionRef::Agent(AgentId("a1".into())), 40, 10);
    // `$ cat`, then one wide character: row 1, two cells in.
    term.parser.process("$ cat\r\n亜".as_bytes());
    app.term = Some(term);
    app.focus = Focus::Terminal;
    app.term_locked = true;
    let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
    draw_frame(&mut terminal, &mut app).unwrap();
    let want = Position::new(app.term_area.x + 2, app.term_area.y + 1);
    assert_eq!(app.host_cursor, Some(want), "the frame named the cell");
    assert_eq!(
        terminal.backend().cursor_position(),
        want,
        "and the host cursor was moved onto it"
    );
    assert!(
        !terminal.backend().cursor_visible(),
        "still hidden: the pane paints its own"
    );
}

/// Scrolled back far enough that the PTY's cursor is below the pane,
/// there is no cell to park on: the host cursor is left where it was
/// rather than pinned to some row that isn't the cursor's.
#[test]
fn a_pane_scrolled_past_its_cursor_leaves_the_host_cursor_alone() {
    use crate::app::{AttachedTerm, Focus};
    use ratatui::layout::Position;
    let mut app = App::new();
    seed_tree(&mut app);
    let mut term = AttachedTerm::new(SessionRef::Agent(AgentId("a1".into())), 40, 10);
    for i in 0..40 {
        term.parser.process(format!("line {i}\r\n").as_bytes());
    }
    // Twenty rows of scrollback above a ten-row grid: the cursor's row
    // is well below the pane.
    term.set_scroll(20);
    app.term = Some(term);
    app.focus = Focus::Terminal;
    app.term_locked = true;
    let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
    let parked = Position::new(7, 7);
    terminal.set_cursor_position(parked).unwrap();
    draw_frame(&mut terminal, &mut app).unwrap();
    assert_eq!(app.host_cursor, None, "no cell to name");
    assert_eq!(terminal.backend().cursor_position(), parked, "so no move");
}

/// With the editor modal up every key goes to it, so the host cursor
/// follows the editor's PTY cursor, not the attached pane's underneath.
#[test]
fn the_editor_modal_takes_the_host_cursor_over_the_pane() {
    use crate::app::{AttachedTerm, Focus};
    use ratatui::layout::Position;
    let mut app = App::new();
    seed_tree(&mut app);
    let mut term = AttachedTerm::new(SessionRef::Agent(AgentId("a1".into())), 40, 10);
    term.parser.process(b"$ cat\r\nabc");
    app.term = Some(term);
    app.focus = Focus::Terminal;
    app.term_locked = true;
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let dir = tempfile::tempdir().unwrap();
    let mut vim = crate::vim_term::VimTerm::spawn_cmd(
        "/bin/sh",
        &["-c".into(), "sleep 30".into()],
        dir.path(),
        "a.txt:1".into(),
        80,
        24,
        1,
        tx,
    )
    .unwrap();
    vim.kill();
    // Row 2, column 4 of the editor's grid.
    vim.process(b"\x1b[3;5H");
    app.vim = Some(vim);
    let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
    draw_frame(&mut terminal, &mut app).unwrap();
    let modal = app.vim.as_ref().unwrap().area;
    let want = Position::new(modal.x + 4, modal.y + 2);
    assert_eq!(app.host_cursor, Some(want), "the editor is where keys go");
    assert_eq!(terminal.backend().cursor_position(), want);
}

pub(super) fn buffer_text(terminal: &Terminal<TestBackend>) -> String {
    let buffer = terminal.backend().buffer();
    let mut out = String::new();
    for y in 0..buffer.area.height {
        for x in 0..buffer.area.width {
            out.push_str(buffer[(x, y)].symbol());
        }
        out.push('\n');
    }
    out
}

/// (x, y) of the first cell of `needle` in the rendered buffer.
fn find_cell(terminal: &Terminal<TestBackend>, needle: &str) -> (u16, u16) {
    let buffer = terminal.backend().buffer();
    for y in 0..buffer.area.height {
        let line: String = (0..buffer.area.width)
            .map(|x| buffer[(x, y)].symbol())
            .collect();
        if let Some(byte) = line.find(needle) {
            return (line[..byte].chars().count() as u16, y);
        }
    }
    panic!("{needle:?} is not on screen");
}

pub(super) fn seed_tree(app: &mut App) {
    use nebula_core::{Agent, AgentStatus, Entity, Project, ProjectId, Worktree, WorktreeId};
    let project_id = ProjectId("p1".into());
    let worktree_id = WorktreeId("w1".into());
    hse(
        app,
        ServerEvent::EntityUpserted {
            entity: Entity::Project(Project {
                id: project_id.clone(),
                name: "demo".into(),
                repo_path: "/tmp/demo".into(),
                sort_order: 0,
            }),
        },
    );
    hse(
        app,
        ServerEvent::EntityUpserted {
            entity: Entity::Worktree(Worktree {
                id: worktree_id.clone(),
                project_id,
                path: "/tmp/demo".into(),
                branch: "main".into(),
                is_main: true,
                sort_order: 0,
            }),
        },
    );
    hse(
        app,
        ServerEvent::EntityUpserted {
            entity: Entity::Agent(Agent {
                id: AgentId("a1".into()),
                worktree_id,
                name: "agent-1".into(),
                status: AgentStatus::Fresh,
                archived: false,
                archived_at: 0,
                unseen: false,
                kind: nebula_core::AgentKind::Claude,
                custom_harness: None,
                model: None,
                effort: None,
                session_id: None,
                cloud_session_id: None,
                sort_order: 0,
                status_changed_at: 0,
                alive: true,
                issue_url: None,
                recent_prompts: Vec::new(),
            }),
        },
    );
}

/// One project whose only worktree is checked out at `dir`.
fn seed_worktree_at(app: &mut App, dir: &std::path::Path) {
    use nebula_core::{Entity, Project, ProjectId, Worktree, WorktreeId};
    hse(
        app,
        ServerEvent::EntityUpserted {
            entity: Entity::Project(Project {
                id: ProjectId("p1".into()),
                name: "demo".into(),
                repo_path: dir.to_path_buf(),
                sort_order: 0,
            }),
        },
    );
    hse(
        app,
        ServerEvent::EntityUpserted {
            entity: Entity::Worktree(Worktree {
                id: WorktreeId("w1".into()),
                project_id: ProjectId("p1".into()),
                path: dir.to_path_buf(),
                branch: "main".into(),
                is_main: true,
                sort_order: 0,
            }),
        },
    );
}

const GHOSTTY: &str = "/Applications/Ghostty.app";

#[test]
fn ghostty_tab_opens_in_the_selected_worktree() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = App::new();
    app.is_remote = false;
    seed_worktree_at(&mut app, dir.path());
    app.flash = None;
    open_ghostty_tab_with(&mut app, Some(GHOSTTY.into()));
    assert_eq!(
        app.flash,
        Some(format!("opened a Ghostty tab in {}", dir.path().display()))
    );
}

#[test]
fn ghostty_tab_is_silent_without_ghostty_or_over_ssh() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = App::new();
    app.is_remote = false;
    seed_worktree_at(&mut app, dir.path());
    app.flash = None;
    open_ghostty_tab_with(&mut app, None);
    assert_eq!(app.flash, None, "no Ghostty.app: not a word");

    let mut empty = App::new();
    empty.flash = None;
    open_ghostty_tab_with(&mut empty, None);
    assert_eq!(empty.flash, None, "nor a select-first nudge");

    app.is_remote = true;
    open_ghostty_tab_with(&mut app, Some(GHOSTTY.into()));
    assert_eq!(app.flash, None, "over ssh the tab would open elsewhere");
}

#[test]
fn ghostty_tab_names_a_checkout_gone_from_disk() {
    let dir = tempfile::tempdir().unwrap();
    let gone = dir.path().join("gone");
    let mut app = App::new();
    app.is_remote = false;
    seed_worktree_at(&mut app, &gone);
    app.flash = None;
    open_ghostty_tab_with(&mut app, Some(GHOSTTY.into()));
    assert_eq!(
        app.flash,
        Some(format!("path missing on disk: {}", gone.display()))
    );
}

#[test]
fn ghostty_app_is_found_in_either_applications_folder() {
    let system = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    let roots = [system.path().to_path_buf(), home.path().to_path_buf()];
    assert_eq!(ghostty_app_in(&roots), None);

    let user_app = home.path().join("Applications/Ghostty.app");
    std::fs::create_dir_all(&user_app).unwrap();
    assert_eq!(ghostty_app_in(&roots), Some(user_app));

    let system_app = system.path().join("Applications/Ghostty.app");
    std::fs::create_dir_all(&system_app).unwrap();
    assert_eq!(
        ghostty_app_in(&roots),
        Some(system_app),
        "/Applications wins"
    );
}

const CLOUD_ID: &str = "session_01SQugK2HDyk33coSrfqFJk4";

/// The CONTEXT MENU of the row under the focused panel's cursor, as a
/// right-click on that row opens it — these tests never draw, so there
/// is no cell to click.
fn open_row_menu(app: &mut App) {
    if let Some(items) = context_menu_items(app, app.focus) {
        open_menu(app, items, KEYBOARD_MENU_ANCHOR);
    }
}

/// Turn the seeded row into a Claude Cloud row — the upsert the daemon
/// sends once the create has printed its session id.
fn make_cloud_row(app: &mut App, out: &mut Vec<ClientRequest>) {
    let mut agent = app.tree.agents[0].clone();
    agent.cloud_session_id = Some(CLOUD_ID.into());
    handle_server_event(
        app,
        ServerEvent::EntityUpserted {
            entity: nebula_core::Entity::Agent(agent),
        },
        out,
    );
}

/// A Cloud row can be steered from nebula: its menu offers a message to
/// queue on the session, in the same multi-row editor the launch task
/// uses, and a failed send hands the text back rather than eating it.
#[test]
fn cloud_row_can_send_a_message_to_its_session() {
    let mut app = App::new();
    seed_tree(&mut app);
    let mut out = Vec::new();
    make_cloud_row(&mut app, &mut out);
    app.focus = Focus::Sessions;

    open_row_menu(&mut app);
    let Some(Overlay::Menu(menu)) = &app.overlay else {
        panic!("no menu: {:?}", app.overlay)
    };
    let labels: Vec<&str> = menu.items.iter().map(|i| i.label.as_str()).collect();
    assert!(
        labels.contains(&"Open in browser") && labels.contains(&"Send to cloud session"),
        "cloud rows get both cloud verbs: {labels:?}"
    );
    assert!(
        !labels.contains(&"Attach") && !labels.contains(&"Restart"),
        "nothing local to attach or restart: {labels:?}"
    );

    let idx = menu
        .items
        .iter()
        .position(|i| i.label == "Send to cloud session")
        .unwrap();
    let Some(Overlay::Menu(menu)) = &mut app.overlay else {
        unreachable!()
    };
    menu.hover = idx;
    press(&mut app, KeyCode::Enter, KeyModifiers::NONE, &mut out);
    let Some(Overlay::Prompt(prompt)) = &mut app.overlay else {
        panic!("no message prompt: {:?}", app.overlay)
    };
    assert!(
        prompt.is_multiline(),
        "steering a cloud agent is rarely one line"
    );
    prompt.input.set_text("also update the README");
    out.clear();
    press(&mut app, KeyCode::Enter, KeyModifiers::NONE, &mut out);
    let req_id = match &out[..] {
        [ClientRequest::SendCloudMessage {
            req_id,
            id,
            message,
        }] => {
            assert_eq!(id.0, "a1");
            assert_eq!(message, "also update the README");
            *req_id
        }
        other => panic!("expected a cloud send: {other:?}"),
    };
    assert!(app.overlay.is_none(), "the prompt closes on submit");

    // A failed send reopens the editor with the message intact.
    hse(
        &mut app,
        ServerEvent::Error {
            req_id: Some(req_id),
            message: "claude could not reach the cloud session".into(),
        },
    );
    let Some(Overlay::Prompt(prompt)) = &app.overlay else {
        panic!("a lost message should come back: {:?}", app.overlay)
    };
    assert_eq!(prompt.input.as_str(), "also update the README");
}

/// A Cloud row has no terminal: the agent runs in the cloud sandbox,
/// so the pane is a panel pointing at the session's page rather than
/// a PTY nebula would have to teleport, and re-teleport, to keep
/// fresh. Landing on the row attaches nothing; Enter and a click on
/// the link hand the URL to the browser and stay put.
#[test]
fn cloud_row_pane_links_to_the_session_instead_of_attaching() {
    let mut app = App::new();
    seed_tree(&mut app);
    let mut out = Vec::new();
    make_cloud_row(&mut app, &mut out);
    app.focus = Focus::Sessions;
    app.sel_session = 0;
    out.clear();

    preview_selected_now(&mut app, &mut out);
    assert!(
        !out.iter()
            .any(|r| matches!(r, ClientRequest::Attach { .. })),
        "a cloud row is never attached: {out:?}"
    );
    assert!(app.term.is_none(), "no PTY stands behind the panel");

    let mut terminal = Terminal::new(TestBackend::new(160, 30)).unwrap();
    terminal.draw(|f| ui::draw(f, &mut app)).unwrap();
    let text = buffer_text(&terminal);
    assert!(text.contains("CLAUDE CLOUD"), "{text}");
    assert!(
        text.contains(&format!("https://claude.ai/code/{CLOUD_ID}")),
        "the link is on the panel: {text}"
    );
    assert!(text.contains(" cloud"), "the row keeps its badge: {text}");
    assert_eq!(
        app.hits
            .iter()
            .filter(|(_, t)| *t == HitTarget::CloudSessionLink)
            .count(),
        1,
        "a wide pane shows the link on one row"
    );

    // A pane too narrow for the URL folds it rather than clipping it —
    // every row of the fold is clickable. Full-screen, so the panel
    // is the whole body and its width is the frame's.
    app.collapsed = true;
    let mut narrow = Terminal::new(TestBackend::new(40, 30)).unwrap();
    narrow.draw(|f| ui::draw(f, &mut app)).unwrap();
    let text = buffer_text(&narrow);
    let rows: Vec<ratatui::layout::Rect> = app
        .hits
        .iter()
        .filter(|(_, t)| *t == HitTarget::CloudSessionLink)
        .map(|(r, _)| *r)
        .collect();
    assert!(rows.len() > 1, "the link folds: {text}");
    let folded: String = text
        .lines()
        .map(|l| l.trim_end().rsplit("  ").next().unwrap_or("").trim())
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .concat();
    assert!(
        folded.contains(&format!("https://claude.ai/code/{CLOUD_ID}")),
        "the whole URL is on the panel: {text}"
    );
    terminal.draw(|f| ui::draw(f, &mut app)).unwrap();

    // Enter opens the page and stays on the panel — nothing to lock into.
    out.clear();
    press(&mut app, KeyCode::Enter, KeyModifiers::NONE, &mut out);
    assert_eq!(
        app.flash.as_deref(),
        Some(&*format!("opened claude.ai/code/{CLOUD_ID}"))
    );
    assert_eq!(app.focus, Focus::Sessions);
    assert!(!app.term_locked);
    assert!(
        !out.iter()
            .any(|r| matches!(r, ClientRequest::Attach { .. })),
        "{out:?}"
    );

    // So does a click on the link itself.
    app.flash = None;
    let (link, _) = app
        .hits
        .iter()
        .find(|(_, t)| *t == HitTarget::CloudSessionLink)
        .cloned()
        .expect("the link is a hit target");
    click(&mut app, link.x + 1, link.y, &mut out);
    assert_eq!(
        app.flash.as_deref(),
        Some(&*format!("opened claude.ai/code/{CLOUD_ID}"))
    );
}

/// The `claude --cloud <task>` create runs in an ordinary pane — the
/// row is attached and locked like any fresh session — until the id it
/// prints turns the row into a Cloud row. That upsert lets the dead
/// create PTY go and gives the keyboard back: from here the pane is the
/// link panel, and Enter opens the browser instead of feeding a process
/// that has already exited.
#[test]
fn a_row_that_gains_its_cloud_session_lets_go_of_the_create_pane() {
    let mut app = App::new();
    seed_tree(&mut app);
    let mut out = Vec::new();
    attach_now(&mut app, SessionRef::Agent(AgentId("a1".into())), &mut out);
    app.focus = Focus::Terminal;
    app.term_locked = true;
    assert!(app.attached_sref.is_some());
    out.clear();

    make_cloud_row(&mut app, &mut out);
    assert!(
        out.iter()
            .any(|r| matches!(r, ClientRequest::Detach { .. })),
        "the create pane is released: {out:?}"
    );
    assert!(app.term.is_none());
    assert!(!app.term_locked);
    assert_eq!(app.focus, Focus::Sessions);
    assert!(app.previewed_cloud().is_some());
}

/// An empty tree replaces the panel columns with the animated splash
/// (wordmark + create hint); the first project upsert swaps the normal
/// columns back in.
#[test]
fn empty_tree_draws_splash_until_first_project() {
    let mut app = App::new();
    let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
    terminal.draw(|f| ui::draw(f, &mut app)).unwrap();
    let text = buffer_text(&terminal);
    assert!(text.contains("create your first project"), "{text}");
    assert!(
        text.contains("your agents keep running"),
        "tagline on the splash: {text}"
    );
    let grid_head = |app: &App| {
        app.hits
            .iter()
            .any(|(_, h)| *h == HitTarget::LauncherTabAdd)
    };
    assert!(!grid_head(&app), "no grid chrome: {text}");
    assert!(app.splash_active());

    seed_tree(&mut app);
    assert!(!app.splash_active());
    terminal.draw(|f| ui::draw(f, &mut app)).unwrap();
    let text = buffer_text(&terminal);
    assert!(grid_head(&app), "the grid is back: {text}");
}

/// The animations setting is a master off-switch for both repaint
/// tickers: the status sweep (running/red rows) and the splash.
#[test]
fn animations_off_stops_sweep_and_splash_ticking() {
    let mut app = App::new();
    assert!(app.splash_active(), "empty tree splash ticks by default");
    app.animations = false;
    assert!(!app.splash_active(), "still splash: drawn but not ticked");

    app.animations = true;
    seed_tree(&mut app);
    assert!(!app.status_anim_active(), "fresh agent doesn't animate");
    app.tree.agents[0].status = nebula_core::AgentStatus::Running;
    assert!(app.status_anim_active());
    app.animations = false;
    assert!(!app.status_anim_active());
}

/// A stamp `ONE_SHOT_SWEEP` and a second old: long enough ago that the
/// row it timed has settled.
fn settled() -> std::time::Duration {
    crate::app::ONE_SHOT_SWEEP + Duration::from_secs(1)
}

/// A checkout whose pull request is seen to merge sweeps purple for
/// `ONE_SHOT_SWEEP`, so it keeps the sweep clock running that long —
/// while its row is on screen — and then stops asking for frames: a
/// merged checkout left lying around repaints nothing. Another
/// project's merged checkout has no row to animate.
#[test]
fn a_merge_seen_to_land_keeps_the_sweep_ticking_for_a_few_seconds() {
    let mut app = App::new();
    seed_tree(&mut app);
    let w1 = nebula_core::WorktreeId("w1".into());
    assert!(
        !app.status_anim_active(),
        "fresh agent, open PR: nothing sweeps"
    );
    seed_branch_pr(&mut app, 7, "Attach links");
    assert!(!app.status_anim_active());

    let mut merged = a_detail(7, "shipped", vec![]);
    merged.state = "MERGED".into();
    adopt_pr_state(&mut app, &merged);
    assert!(app.merge_is_fresh(&w1), "open to merged, seen: stamped");
    assert!(app.status_anim_active(), "the merged row sweeps");
    app.animations = false;
    assert!(!app.status_anim_active(), "unless animations are off");
    app.animations = true;

    // The window runs out: the row is still purple, and at rest.
    let long_ago = std::time::Instant::now()
        .checked_sub(settled())
        .expect("uptime past the window");
    app.merge_landed.insert(w1.clone(), long_ago);
    assert!(app.worktree_wears_merge(&w1), "still the merged row");
    assert!(!app.status_anim_active(), "settled: no frames asked for");
    // Hearing the same state again is not a second merge.
    adopt_pr_state(&mut app, &merged);
    assert!(!app.merge_is_fresh(&w1));
    app.note_merge_landed(w1.clone());
    assert!(
        app.status_anim_active(),
        "fresh again, for the rest of this"
    );

    // Another project selected: the merged checkout is not a row.
    use nebula_core::{Entity, Project, ProjectId};
    hse(
        &mut app,
        ServerEvent::EntityUpserted {
            entity: Entity::Project(Project {
                id: ProjectId("p2".into()),
                name: "other".into(),
                repo_path: "/tmp/other".into(),
                sort_order: 1,
            }),
        },
    );
    let p2 = app
        .tree
        .projects
        .iter()
        .position(|p| p.id == ProjectId("p2".into()))
        .expect("the other project");
    app.sel_project = app
        .project_rows()
        .iter()
        .position(|i| *i == p2)
        .expect("its row");
    assert_eq!(
        app.selected_project().map(|p| p.name.as_str()),
        Some("other")
    );
    assert!(!app.status_anim_active(), "no merged row on screen");
}

/// `⇧N` used to summon the splash over a populated tree. The key is
/// gone: the grid stays on screen, and the splash is the first run's
/// alone.
#[test]
fn shift_n_no_longer_summons_the_splash() {
    let mut app = App::new();
    seed_tree(&mut app);
    let mut out = Vec::new();
    press(&mut app, KeyCode::Char('N'), KeyModifiers::SHIFT, &mut out);
    assert!(!app.splash_showing());

    let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
    terminal.draw(|f| ui::draw(f, &mut app)).unwrap();
    let text = buffer_text(&terminal);
    assert!(!text.contains("any key returns"), "{text}");
    assert!(
        app.hits
            .iter()
            .any(|(_, h)| *h == HitTarget::LauncherTabAdd),
        "the grid is on screen: {text}"
    );
}

/// While the tree is empty, `n` opens the add-project prompt from any
/// focus — the splash hides the panels, so the per-panel meanings of
/// `n` would just dead-end.
#[test]
fn n_adds_project_from_any_focus_while_tree_empty() {
    let mut app = App::new();
    app.focus = Focus::Sessions;
    let mut out = Vec::new();
    press(&mut app, KeyCode::Char('n'), KeyModifiers::NONE, &mut out);
    let Some(Overlay::Prompt(p)) = &app.overlay else {
        panic!("expected add-project prompt, got {:?}", app.overlay);
    };
    assert_eq!(p.kind, crate::app::PromptKind::AddProject);
}

/// The splash hides the panels, so the footer drops the panel keymap
/// for the handful of keys that still fire under it.
#[test]
fn splash_footer_lists_only_keys_that_work() {
    let mut app = App::new();
    // Wide enough that the panel hints reach `?: help` unclipped —
    // the version nameplate on the far left costs ~18 columns.
    let mut terminal = Terminal::new(TestBackend::new(160, 30)).unwrap();
    terminal.draw(|f| ui::draw(f, &mut app)).unwrap();
    let text = buffer_text(&terminal);
    assert!(text.contains("o: open a folder"), "{text}");
    assert!(!text.contains("workspace"), "{text}");
    assert!(text.contains("q: quit"), "{text}");
    for dead in ["d: remove", "m: menu", "/: search"] {
        assert!(
            !text.contains(dead),
            "{dead} does nothing on the splash: {text}"
        );
    }

    // A project lands: the grid, the grid's keymap — without `o`,
    // which the Help overlay lists; the footer keeps the keys a card
    // takes.
    seed_tree(&mut app);
    terminal.draw(|f| ui::draw(f, &mut app)).unwrap();
    let text = buffer_text(&terminal);
    assert!(text.contains("new session"), "{text}");
    assert!(!text.contains("open a folder"), "{text}");
}

#[test]
fn opening_a_project_whose_folder_is_missing_offers_to_locate_it() {
    let tmp = tempfile::tempdir().unwrap();
    let gone = tmp.path().join("moved-away");
    let mut app = App::new();
    app.prompt_missing_project_paths = true;
    seed_worktree_at(&mut app, &gone);
    let mut out = Vec::new();

    launcher::open_project(&mut app, &ProjectId("p1".into()), &mut out);
    assert!(out.is_empty(), "{out:?}");
    let Some(Overlay::Confirm(confirm)) = &app.overlay else {
        panic!("expected locate confirm, got {:?}", app.overlay);
    };
    assert_eq!(confirm.title, "Project folder not found");
    assert!(confirm.message.contains(&gone.display().to_string()));
    assert!(matches!(
        &confirm.action,
        PendingAction::LocateProjectPath { id, old_path }
            if id == &ProjectId("p1".into()) && old_path == &gone
    ));

    press(&mut app, KeyCode::Char('n'), KeyModifiers::NONE, &mut out);
    assert!(app.overlay.is_none());
    assert!(
        app.dismissed_repath_projects
            .contains(&ProjectId("p1".into())),
        "n dismisses it for this TUI run"
    );
    launcher::open_project(&mut app, &ProjectId("p1".into()), &mut out);
    assert!(app.overlay.is_none(), "dismissed projects do not nag");
}

#[test]
fn locating_a_missing_project_sends_set_project_path_and_reopens_on_error() {
    let tmp = tempfile::tempdir().unwrap();
    let gone = tmp.path().join("old");
    let moved = tmp.path().join("new");
    std::fs::create_dir_all(&moved).unwrap();
    let moved_canon = std::fs::canonicalize(&moved).unwrap();
    let mut app = App::new();
    app.prompt_missing_project_paths = true;
    seed_worktree_at(&mut app, &gone);
    let mut out = Vec::new();

    launcher::open_project(&mut app, &ProjectId("p1".into()), &mut out);
    press(&mut app, KeyCode::Char('y'), KeyModifiers::NONE, &mut out);
    let Some(Overlay::Prompt(prompt)) = &mut app.overlay else {
        panic!("expected locate prompt, got {:?}", app.overlay);
    };
    assert!(matches!(
        prompt.kind,
        PromptKind::SetProjectPath {
            id: ProjectId(ref id),
            ..
        } if id == "p1"
    ));
    prompt.input.set_text(moved.display().to_string());
    prompt.refresh_dirs();
    press(&mut app, KeyCode::Enter, KeyModifiers::NONE, &mut out);
    let (req_id, path) = match out.as_slice() {
        [ClientRequest::SetProjectPath { req_id, path, .. }] => (*req_id, path.clone()),
        other => panic!("expected SetProjectPath, got {other:?}"),
    };
    assert_eq!(path, moved_canon);

    hse(
        &mut app,
        ServerEvent::Error {
            req_id: Some(req_id),
            message: "not a git repository".into(),
        },
    );
    let Some(Overlay::Prompt(prompt)) = &app.overlay else {
        panic!("error reopens the locate prompt, got {:?}", app.overlay);
    };
    assert_eq!(prompt.input.as_str(), moved_canon.display().to_string());
    assert_eq!(app.flash.as_deref(), Some("not a git repository"));
}

#[test]
fn successful_project_repath_rekeys_project_settings() {
    let tmp = tempfile::tempdir().unwrap();
    let gone = tmp.path().join("old");
    let moved = tmp.path().join("new");
    std::fs::create_dir_all(&moved).unwrap();
    let moved_canon = std::fs::canonicalize(&moved).unwrap();
    let config_path = tmp.path().join("config.json");
    std::fs::write(
        &config_path,
        format!(
            r#"{{"projects":{{"{}":{{"run_command":"npm run dev"}}}}}}"#,
            gone.display()
        ),
    )
    .unwrap();

    crate::config::with_config_path(config_path.clone(), || {
        let mut app = App::new();
        app.prompt_missing_project_paths = true;
        seed_worktree_at(&mut app, &gone);
        let mut out = Vec::new();

        launcher::open_project(&mut app, &ProjectId("p1".into()), &mut out);
        press(&mut app, KeyCode::Char('y'), KeyModifiers::NONE, &mut out);
        if let Some(Overlay::Prompt(prompt)) = &mut app.overlay {
            prompt.input.set_text(moved.display().to_string());
            prompt.refresh_dirs();
        } else {
            panic!("expected locate prompt");
        }
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE, &mut out);
        let req_id = match out.as_slice() {
            [ClientRequest::SetProjectPath { req_id, .. }] => *req_id,
            other => panic!("expected SetProjectPath, got {other:?}"),
        };
        hse(
            &mut app,
            ServerEvent::Ack {
                req_id,
                created: None,
            },
        );

        let cfg = crate::config::Config::load();
        assert_eq!(
            cfg.project_text_value(&gone, crate::config::SettingKind::RunCommand),
            ""
        );
        assert_eq!(
            cfg.project_text_value(&moved_canon, crate::config::SettingKind::RunCommand),
            "npm run dev"
        );
    });
}

/// A tempdir holding `ws/alpha` (a git repo) and `ws/beta` (not one),
/// with `alpha` as the repo nebula was started in.
fn launched_in_alpha(app: &mut App) -> tempfile::TempDir {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(tmp.path().join("ws/alpha/.git")).unwrap();
    std::fs::create_dir_all(tmp.path().join("ws/beta")).unwrap();
    app.launch_repo = Some(tmp.path().join("ws/alpha"));
    tmp
}

/// First run, started inside a repo: the SPLASH names the folder, and
/// Enter opens it as a project — the one key between a fresh install
/// and a project on screen, whichever focus the app booted with.
#[test]
fn enter_on_the_first_run_splash_opens_the_folder_nebula_started_in() {
    let mut app = App::new();
    let tmp = launched_in_alpha(&mut app);
    app.focus = Focus::Terminal;
    let mut terminal = Terminal::new(TestBackend::new(120, 30)).unwrap();
    terminal.draw(|f| ui::draw(f, &mut app)).unwrap();
    let text = buffer_text(&terminal);
    assert!(text.contains("Enter: open alpha"), "{text}");
    assert!(!text.contains("workspace"), "{text}");

    let mut out = Vec::new();
    press(&mut app, KeyCode::Enter, KeyModifiers::NONE, &mut out);
    assert!(
        matches!(
            out.as_slice(),
            [ClientRequest::AddProject { path, create_missing: false, .. }]
                if path == &tmp.path().join("ws/alpha")
        ),
        "{out:?}"
    );
}

/// Started outside a repo there is no folder to name: the splash says
/// `o` opens one, and Enter opens the prompt rather than nothing.
#[test]
fn enter_on_the_first_run_splash_elsewhere_opens_the_prompt() {
    let mut app = App::new();
    let mut terminal = Terminal::new(TestBackend::new(120, 30)).unwrap();
    terminal.draw(|f| ui::draw(f, &mut app)).unwrap();
    let text = buffer_text(&terminal);
    assert!(text.contains("o: open a folder"), "{text}");
    let mut out = Vec::new();
    press(&mut app, KeyCode::Enter, KeyModifiers::NONE, &mut out);
    let Some(Overlay::Prompt(p)) = &app.overlay else {
        panic!("expected the open-project prompt, got {:?}", app.overlay);
    };
    assert_eq!(p.kind, crate::app::PromptKind::AddProject);
    assert!(out.is_empty(), "{out:?}");
}

/// A project is a git repository: opening a folder in none asks to
/// `git init` it — `y` sends the add with `create_missing`, which is
/// the daemon's cue to init, and `n` sends nothing. A folder inside a
/// repository opens without asking.
#[test]
fn opening_a_folder_outside_git_asks_to_git_init_it() {
    let mut app = App::new();
    let tmp = launched_in_alpha(&mut app);
    let beta = tmp.path().join("ws/beta");
    let mut out = Vec::new();

    open_folder(&mut app, beta.clone(), &mut out);
    assert!(out.is_empty(), "{out:?}");
    assert!(
        matches!(&app.overlay, Some(Overlay::Confirm(c))
                if c.title == "Not a git repository"
                    && c.action == PendingAction::InitProjectRepo(beta.clone())),
        "{:?}",
        app.overlay
    );
    press(&mut app, KeyCode::Char('n'), KeyModifiers::NONE, &mut out);
    assert!(app.overlay.is_none());
    assert!(out.is_empty(), "{out:?}");

    open_folder(&mut app, beta.clone(), &mut out);
    press(&mut app, KeyCode::Char('y'), KeyModifiers::NONE, &mut out);
    assert!(
        matches!(
            out.as_slice(),
            [ClientRequest::AddProject { path, create_missing: true, .. }] if path == &beta
        ),
        "{out:?}"
    );

    out.clear();
    let inner = tmp.path().join("ws/alpha/src");
    std::fs::create_dir_all(&inner).unwrap();
    open_folder(&mut app, inner.clone(), &mut out);
    assert!(
        matches!(
            out.as_slice(),
            [ClientRequest::AddProject { path, create_missing: false, .. }] if path == &inner
        ),
        "{out:?}"
    );
}

/// The open-project prompt starts on the folder nebula was started in:
/// its parent listed, the folder itself highlighted — so Enter opens
/// it and ↑/↓ reach the folders beside it.
#[test]
fn the_open_project_prompt_starts_on_the_launch_folder() {
    let mut app = App::new();
    let tmp = launched_in_alpha(&mut app);
    let mut out = Vec::new();
    press(&mut app, KeyCode::Char('o'), KeyModifiers::NONE, &mut out);
    let Some(Overlay::Prompt(p)) = &app.overlay else {
        panic!("expected the open-project prompt, got {:?}", app.overlay);
    };
    assert_eq!(p.title, "Open project");
    assert_eq!(dir_names(p), vec!["alpha", "beta"]);
    assert_eq!(
        p.hovered_path(),
        Some(format!("{}/ws/alpha", tmp.path().display()))
    );
    press(&mut app, KeyCode::Enter, KeyModifiers::NONE, &mut out);
    assert!(
        matches!(
            out.as_slice(),
            [ClientRequest::AddProject { path, .. }] if path == &tmp.path().join("ws/alpha")
        ),
        "{out:?}"
    );
}

/// A folder that already is a project — its root, or a folder inside
/// it — opens that project instead of asking the daemon to register it
/// a second time, which would only come back refused.
#[test]
fn opening_a_known_folder_opens_its_project() {
    let tmp = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(tmp.path()).unwrap();
    std::fs::create_dir_all(root.join("demo/src")).unwrap();
    let mut app = App::new();
    seed_tree(&mut app);
    app.tree.projects[0].repo_path = root.join("demo");
    let mut out = Vec::new();
    open_folder(&mut app, root.join("demo/src"), &mut out);
    assert!(
        !out.iter()
            .any(|r| matches!(r, ClientRequest::AddProject { .. })),
        "{out:?}"
    );
    assert_eq!(
        app.flash.as_deref(),
        Some("demo is already a project — opened it")
    );
    assert_eq!(
        app.selected_project().map(|p| p.name.as_str()),
        Some("demo")
    );
}

/// `o` opens the add-project prompt regardless of focus or tree state —
/// unlike `n` it never takes on a per-panel meaning.
#[test]
fn o_adds_project_from_any_focus() {
    for focus in [Focus::Projects, Focus::Worktrees, Focus::Sessions] {
        let mut app = App::new();
        seed_tree(&mut app);
        app.focus = focus;
        let mut out = Vec::new();
        press(&mut app, KeyCode::Char('o'), KeyModifiers::NONE, &mut out);
        let Some(Overlay::Prompt(p)) = &app.overlay else {
            panic!(
                "expected add-project prompt at {focus:?}, got {:?}",
                app.overlay
            );
        };
        assert_eq!(p.kind, crate::app::PromptKind::AddProject);
    }
}

// ---- worktree links ----

/// `seed_tree` plus one saved link on w1, cursor parked on it.
fn seed_link(app: &mut App, url: &str) {
    hse(
        app,
        ServerEvent::EntityUpserted {
            entity: nebula_core::Entity::Link(nebula_core::Link {
                id: LinkId("l1".into()),
                worktree_id: nebula_core::WorktreeId("w1".into()),
                url: url.into(),
                sort_order: 0,
            }),
        },
    );
    app.focus = Focus::Sessions;
    app.sel_session = app
        .visible_session_rows()
        .iter()
        .position(|r| r.as_link().is_some())
        .expect("link row");
}

/// The double tap is a gesture, not a state: a second press that comes
/// too late, or after any other key, is a fresh single press.
#[test]
fn a_slow_or_interrupted_second_tap_at_the_edge_stays_put() {
    let mut app = App::new();
    let mut out = Vec::new();
    let l = |app: &mut App, out: &mut Vec<ClientRequest>| {
        press(app, KeyCode::Char('l'), KeyModifiers::NONE, out)
    };

    app.focus = Focus::Sessions;
    l(&mut app, &mut out);
    let (armed, _) = app.edge_tap.expect("the first press arms");
    assert_eq!(armed, crate::keymap::Action::FocusRight);
    app.edge_tap = Some((
        armed,
        std::time::Instant::now() - focus_walk::DOUBLE_TAP - Duration::from_millis(100),
    ));
    l(&mut app, &mut out);
    assert_eq!(app.focus, Focus::Sessions, "too slow: two single presses");
    assert!(app.edge_tap.is_some(), "but the late one arms again");

    press(&mut app, KeyCode::Char('j'), KeyModifiers::NONE, &mut out);
    assert!(app.edge_tap.is_none(), "any other key breaks the pair");
    l(&mut app, &mut out);
    assert_eq!(app.focus, Focus::Sessions, "so this is a first press again");
    l(&mut app, &mut out);
    assert_eq!(app.focus, Focus::Terminal, "and this one completes it");
}

#[test]
fn shift_l_no_longer_opens_manual_link_creation() {
    let mut app = App::new();
    seed_tree(&mut app);
    app.focus = Focus::Sessions;
    let mut out = Vec::new();

    press(&mut app, KeyCode::Char('L'), KeyModifiers::SHIFT, &mut out);

    assert!(app.overlay.is_none(), "manual LINK creation stays closed");
    assert!(
        out.is_empty(),
        "manual LINK creation sends no request: {out:?}"
    );
}

#[test]
fn r_edits_a_link_and_d_deletes_it() {
    let mut app = App::new();
    seed_tree(&mut app);
    seed_link(&mut app, "https://example.dev/spec");
    let mut out = Vec::new();

    press(&mut app, KeyCode::Char('r'), KeyModifiers::NONE, &mut out);
    let Some(Overlay::Prompt(p)) = &app.overlay else {
        panic!("expected the edit-link prompt, got {:?}", app.overlay);
    };
    assert_eq!(p.title, "Edit link");
    assert_eq!(p.input.trim(), "https://example.dev/spec", "prefilled");
    press(&mut app, KeyCode::Esc, KeyModifiers::NONE, &mut out);

    press(&mut app, KeyCode::Char('d'), KeyModifiers::NONE, &mut out);
    assert!(
        matches!(&app.overlay, Some(Overlay::Confirm(c)) if c.title == "Delete link"),
        "expected the delete confirm, got {:?}",
        app.overlay
    );
    press(&mut app, KeyCode::Char('y'), KeyModifiers::NONE, &mut out);
    assert!(
        out.iter()
            .any(|r| matches!(r, ClientRequest::DeleteLink { id, .. } if id.as_str() == "l1")),
        "expected DeleteLink, got {out:?}"
    );
}

/// Seed the selected project's open-pull-request list, as though a
/// `gh pr list` had just answered.
pub(super) fn seed_open_prs(app: &mut App, prs: &[(u64, &str)]) {
    let id = app.selected_project().expect("a project").id.clone();
    let now = std::time::Instant::now();
    app.open_prs.insert(
        id,
        crate::app::OpenPrs {
            list: prs
                .iter()
                .map(|(number, title)| crate::pull_request::OpenPr {
                    number: *number,
                    title: (*title).into(),
                    url: format!("https://github.com/o/r/pull/{number}"),
                    is_draft: false,
                    health: Default::default(),
                    head: format!("pr-{number}-head"),
                })
                .collect(),
            at: now,
            due: now + OPEN_PRS_REFRESH,
            step: OPEN_PRS_REFRESH,
        },
    );
}

/// Open issues on the selected project, as `gh issue list` would have
/// answered — the PROJECT ISSUES GROUP's rows.
pub(super) fn seed_issues(app: &mut App, issues: &[(u64, &str)]) {
    let id = app.selected_project().expect("a project").id.clone();
    app.issues.insert(
        id,
        crate::issues::IssueList {
            list: issues
                .iter()
                .map(|(number, title)| crate::issues::Issue {
                    number: *number,
                    url: format!("https://github.com/o/r/issues/{number}"),
                    title: (*title).into(),
                    author: "webdevcody".into(),
                    created_at: "2026-09-10T12:00:00Z".into(),
                    updated_at: "2026-09-11T12:00:00Z".into(),
                    labels: Vec::new(),
                    body: format!("Body of issue {number}"),
                })
                .collect(),
            at: std::time::Instant::now(),
        },
    );
}

/// A control chord pressed wherever the cursor is: `ctrl(app, 'd', out)`.
fn ctrl(app: &mut App, c: char, out: &mut Vec<ClientRequest>) {
    press(app, KeyCode::Char(c), KeyModifiers::CONTROL, out);
}

/// The open pull requests take the rows *after* the checkouts, which is
/// what lets every "index into visible_worktrees()" in the app stay
/// correct: a cursor on a PR row simply has no selected worktree.
#[test]
fn open_prs_take_the_rows_below_the_worktrees() {
    let mut app = App::new();
    seed_tree(&mut app); // p1 / w1(main) / a1
    assert_eq!(app.worktree_row_count(), 1, "no list fetched yet");
    seed_open_prs(&mut app, &[(7, "Attach links"), (9, "Number the lines")]);
    assert_eq!(app.worktree_row_count(), 3);

    assert!(app.selected_worktree().is_some(), "row 0 is the checkout");
    assert!(app.selected_worktree_pr().is_none());

    app.sel_worktree = 1;
    assert!(app.selected_worktree().is_none(), "row 1 is a pull request");
    assert_eq!(app.selected_worktree_pr().map(|p| p.number), Some(7));
    app.sel_worktree = 2;
    assert_eq!(app.selected_worktree_pr().map(|p| p.number), Some(9));
}

/// A checkout on an open pull request's head branch — the one a PR
/// SESSION works in — lists under that pull request's row, not among
/// the plain checkouts above the group, so the checkout and the pull
/// request it is for read as one thing. It is still a worktree row:
/// the cursor on it has that worktree and no pull request. The ROOT
/// WORKTREE never nests, and a pull request off screen — the group
/// folded, or a draft `hide_draft_prs` keeps out — takes no checkout
/// with it: the row is plain again rather than gone.
#[test]
fn a_checkout_on_a_pull_requests_head_branch_nests_under_its_row() {
    use crate::app::WorktreeRow;
    let mut app = App::new();
    seed_tree(&mut app); // p1 / w1(main) / a1
    seed_feat_worktree(&mut app, "w2", "pr-7-head");
    seed_feat_worktree(&mut app, "w3", "feat");
    seed_open_prs(&mut app, &[(7, "Attach links"), (9, "Number the lines")]);
    let rows = |app: &App| -> Vec<String> {
        app.worktree_rows()
            .iter()
            .map(|r| match r {
                WorktreeRow::Checkout(w) => w.branch.clone(),
                WorktreeRow::Pr(pr) => format!("#{}", pr.number),
                WorktreeRow::PrCheckout(worktree) => format!("└ {}", worktree.branch),
                WorktreeRow::Issue(issue) => format!("issue #{}", issue.number),
            })
            .collect()
    };
    assert_eq!(rows(&app), ["main", "feat", "#7", "└ pr-7-head", "#9"]);
    assert_eq!(app.worktree_row_count(), 5);

    app.sel_worktree = 2;
    assert_eq!(app.selected_worktree_pr().map(|p| p.number), Some(7));
    assert!(app.selected_worktree().is_none(), "a pull request row");
    app.sel_worktree = 3;
    assert_eq!(
        app.selected_worktree().map(|w| w.id.0.clone()),
        Some("w2".into()),
        "the row under it is the checkout"
    );
    assert!(
        app.selected_worktree_pr().is_none(),
        "…and not the pull request"
    );
    assert_eq!(app.worktree_row_of(&WorktreeId("w2".into())), Some(3));
    assert_eq!(app.open_pr_row_of(&pr_url(7)), Some(2));

    // Folded, the pull requests are off screen and the checkout is a
    // plain row again — nothing you have is hidden by hiding them.
    app.open_prs_collapsed = true;
    assert_eq!(rows(&app), ["main", "pr-7-head", "feat"]);
    app.open_prs_collapsed = false;

    // A draft kept out by the setting takes no checkout with it either.
    let pid = app.selected_project().expect("a project").id.clone();
    app.open_prs.get_mut(&pid).unwrap().list[0].is_draft = true;
    app.hide_draft_prs = true;
    assert_eq!(rows(&app), ["main", "pr-7-head", "feat", "#9"]);
    app.hide_draft_prs = false;
    assert_eq!(rows(&app), ["main", "feat", "#7", "└ pr-7-head", "#9"]);

    // The ROOT WORKTREE never nests, whatever branch it is on.
    app.open_prs.get_mut(&pid).unwrap().list[1].head = "main".into();
    assert_eq!(rows(&app), ["main", "feat", "#7", "└ pr-7-head", "#9"]);
}

/// The cursor keeps its checkout as the rows regroup around it: the
/// `gh pr list` answer that first lists a pull request moves the
/// checkout on its branch under it; a fold puts it back among the
/// plain rows and an unfold takes it under again; hiding a draft frees
/// its checkout and showing it takes it back; and the answer that
/// retires the pull request frees it for good. None of these lands
/// the cursor on some other row.
#[test]
fn the_cursor_follows_a_checkout_that_moves_under_or_out_from_under_its_pull_request() {
    let mut app = App::new();
    seed_tree(&mut app);
    seed_feat_worktree(&mut app, "w2", "pr-7-head");
    app.focus = Focus::Worktrees;
    let mut out = Vec::new();
    let w2 = WorktreeId("w2".into());
    let on_w2 = |app: &App| app.selected_worktree().map(|w| w.id.clone()) == Some(w2.clone());
    app.sel_worktree = 1;
    assert!(on_w2(&app));
    let pid = app.selected_project().expect("a project").id.clone();
    let listed = vec![crate::pull_request::OpenPr {
        number: 7,
        title: "Attach links".into(),
        url: pr_url(7),
        is_draft: false,
        health: Default::default(),
        head: "pr-7-head".into(),
    }];

    // The list lands: the checkout moves under #7, the cursor with it.
    note_open_prs_answer(&mut app, pid.clone(), Some(listed), &mut out);
    assert_eq!(app.worktree_row_of(&w2), Some(2));
    assert_eq!(app.sel_worktree, 2);
    assert!(on_w2(&app));

    // Folding frees it; unfolding nests it again.
    toggle_open_prs(&mut app, &mut out);
    assert!(app.open_prs_collapsed);
    assert_eq!(app.sel_worktree, 1);
    assert!(on_w2(&app));
    toggle_open_prs(&mut app, &mut out);
    assert_eq!(app.sel_worktree, 2);
    assert!(on_w2(&app));

    // A draft hidden by the setting frees its checkout; shown, it
    // takes it back.
    app.open_prs.get_mut(&pid).unwrap().list[0].is_draft = true;
    assert!(
        !set_hide_draft_prs(&mut app, true),
        "the cursor never left a checkout"
    );
    assert_eq!(app.sel_worktree, 1);
    assert!(on_w2(&app));
    set_hide_draft_prs(&mut app, false);
    assert_eq!(app.sel_worktree, 2);
    assert!(on_w2(&app));

    // The pull request merges: the next answer lists it no more, and
    // the checkout is a plain row, still under the cursor.
    note_open_prs_answer(&mut app, pid, Some(Vec::new()), &mut out);
    assert_eq!(app.worktree_row_count(), 2);
    assert_eq!(app.sel_worktree, 1);
    assert!(on_w2(&app));
}

/// The half-page chords belong to the Worktrees and Sessions columns.
/// Projects leaves them unclaimed as before, and a locked pane
/// forwards them to the PTY as the control bytes they are — Ctrl+d is
/// the shell's EOF and Ctrl+u its kill-to-start, and an agent running
/// in there is owed both.
#[test]
fn half_page_chords_stay_out_of_the_projects_panel_and_the_locked_pane() {
    let mut app = App::new();
    seed_tree(&mut app);
    seed_open_prs(&mut app, &[(7, "Attach links"), (9, "Number the lines")]);
    app.worktrees_view_rows = 6;
    app.sessions_view_rows = 6;
    let mut out = Vec::new();

    app.focus = Focus::Projects;
    let before = (app.sel_project, app.sel_worktree, app.sel_session);
    ctrl(&mut app, 'd', &mut out);
    ctrl(&mut app, 'u', &mut out);
    assert_eq!(
        (app.sel_project, app.sel_worktree, app.sel_session),
        before,
        "Projects: no cursor moved"
    );
    assert!(app.overlay.is_none(), "Projects: nothing opened");

    app.focus = Focus::Terminal;
    app.term_locked = true;
    app.term = Some(AttachedTerm::new(
        SessionRef::Agent(AgentId("a1".into())),
        80,
        24,
    ));
    out.clear();
    ctrl(&mut app, 'd', &mut out);
    assert!(
        matches!(out.last(), Some(ClientRequest::Input { data, .. }) if data == b"\x04"),
        "^d reaches the PTY as EOF: {out:?}"
    );
    ctrl(&mut app, 'u', &mut out);
    assert!(
        matches!(out.last(), Some(ClientRequest::Input { data, .. }) if data == b"\x15"),
        "^u reaches the PTY as kill-to-start: {out:?}"
    );
    assert_eq!(app.sel_worktree, 0, "the Worktrees cursor never moved");
    assert!(app.term_locked, "and the lock held");
}

/// A repo with nothing open backs off instead of asking every beat, and
/// a call `gh` couldn't answer keeps whatever list was already on screen
/// — one flaky round trip is no reason to blank the group.
#[test]
fn the_open_pr_list_backs_off_when_empty_and_survives_a_failed_call() {
    let mut app = App::new();
    seed_tree(&mut app);
    let pid = app.selected_project().expect("a project").id.clone();

    note_open_prs_answer(&mut app, pid.clone(), Some(vec![]), &mut Vec::new());
    assert_eq!(app.open_prs[&pid].step, OPEN_PRS_RECHECK_MIN);
    note_open_prs_answer(&mut app, pid.clone(), Some(vec![]), &mut Vec::new());
    assert_eq!(app.open_prs[&pid].step, OPEN_PRS_RECHECK_MIN * 2);

    let found = vec![crate::pull_request::OpenPr {
        number: 7,
        title: "Attach links".into(),
        url: "https://github.com/o/r/pull/7".into(),
        is_draft: false,
        health: Default::default(),
        head: "attach-links".into(),
    }];
    note_open_prs_answer(&mut app, pid.clone(), Some(found.clone()), &mut Vec::new());
    assert_eq!(
        app.open_prs[&pid].step, OPEN_PRS_REFRESH,
        "a repo with pull requests settles onto the steady beat"
    );
    assert_eq!(app.visible_open_prs().len(), 1);

    assert!(!app.open_prs_failed.contains(&pid));
    note_open_prs_answer(&mut app, pid.clone(), None, &mut Vec::new());
    assert_eq!(
        app.open_prs[&pid].list, found,
        "a failed call keeps the last good list"
    );
    assert!(app.open_prs[&pid].step > OPEN_PRS_REFRESH, "but backs off");
    assert!(
        app.open_prs_failed.contains(&pid),
        "and marks it as one that couldn't be refreshed (#106)"
    );

    note_open_prs_answer(&mut app, pid.clone(), Some(vec![]), &mut Vec::new());
    assert!(
        !app.open_prs_failed.contains(&pid),
        "the next real answer clears the mark"
    );
}

/// Arriving at a project asks again promptly — but never more often than
/// `OPEN_PRS_MIN_AGE`, so bouncing between two projects re-reads the
/// cache instead of spending an API call per switch.
#[test]
fn arriving_at_a_project_re_asks_but_not_faster_than_the_floor() {
    let mut app = App::new();
    seed_tree(&mut app);
    let pid = app.selected_project().expect("a project").id.clone();
    note_open_prs_answer(&mut app, pid.clone(), Some(vec![]), &mut Vec::new());

    schedule_open_prs_lookup(&mut app);
    assert!(
        !app.open_prs_lookup_due(&pid),
        "the answer is seconds old: the floor holds the next call off"
    );

    // An answer older than the floor is re-asked the moment we arrive.
    let stale = std::time::Instant::now() - crate::app::OPEN_PRS_MIN_AGE * 2;
    app.open_prs.get_mut(&pid).unwrap().at = stale;
    schedule_open_prs_lookup(&mut app);
    assert!(app.open_prs_lookup_due(&pid));

    // An in-flight call is never doubled up on.
    app.open_prs_inflight.insert(pid.clone());
    assert!(!app.open_prs_lookup_due(&pid));
}

/// Focusing the Worktrees or Sessions panel re-asks GitHub for the open
/// list and the worktree's own PR; focusing the pane does not. The list
/// keeps its floor, so a quick bounce between panels stays one call.
#[test]
fn focusing_a_sidebar_panel_re_asks_for_pull_requests() {
    let mut app = App::new();
    seed_tree(&mut app);
    let pid = app.selected_project().expect("a project").id.clone();
    let wid = app.selected_worktree().expect("a worktree").id.clone();
    let stale = std::time::Instant::now() - crate::app::OPEN_PRS_MIN_AGE * 2;
    let settle = |app: &mut App| {
        note_open_prs_answer(app, pid.clone(), Some(vec![]), &mut Vec::new());
        app.open_prs.get_mut(&pid).unwrap().at = stale;
        note_pr_answer(app, &wid, true);
    };

    settle(&mut app);
    app.focus = Focus::Terminal;
    note_focus_change(&mut app);
    assert!(
        !app.open_prs_lookup_due(&pid),
        "the pane isn't a PR surface"
    );
    assert!(!app.pr_lookup_due(&wid));

    app.focus = Focus::Worktrees;
    note_focus_change(&mut app);
    assert!(app.open_prs_lookup_due(&pid), "the Worktrees panel is");
    assert!(app.pr_lookup_due(&wid));

    settle(&mut app);
    app.focus = Focus::Sessions;
    note_focus_change(&mut app);
    assert!(app.open_prs_lookup_due(&pid), "so is the Sessions panel");
    assert!(app.pr_lookup_due(&wid));

    // Seconds-fresh answer: the floor holds the list off, the PR row
    // (one `gh pr view`, no floor of its own) is still re-asked.
    note_open_prs_answer(&mut app, pid.clone(), Some(vec![]), &mut Vec::new());
    note_pr_answer(&mut app, &wid, true);
    app.focus = Focus::Worktrees;
    note_focus_change(&mut app);
    assert!(!app.open_prs_lookup_due(&pid), "floored");
    assert!(app.pr_lookup_due(&wid));
}

/// The terminal window taking focus again — back from the browser where
/// a pull request was just closed — re-asks on the next tick.
#[test]
fn terminal_window_focus_re_asks_for_pull_requests() {
    let mut app = App::new();
    seed_tree(&mut app);
    let pid = app.selected_project().expect("a project").id.clone();
    let wid = app.selected_worktree().expect("a worktree").id.clone();
    note_open_prs_answer(&mut app, pid.clone(), Some(vec![]), &mut Vec::new());
    app.open_prs.get_mut(&pid).unwrap().at =
        std::time::Instant::now() - crate::app::OPEN_PRS_MIN_AGE * 2;
    note_pr_answer(&mut app, &wid, true);
    assert!(!app.open_prs_lookup_due(&pid));
    assert!(!app.pr_lookup_due(&wid));

    let mut out = Vec::new();
    handle_terminal_event(&mut app, Event::FocusLost, &mut out);
    assert!(!app.open_prs_lookup_due(&pid), "losing focus asks nothing");
    handle_terminal_event(&mut app, Event::FocusGained, &mut out);
    assert!(app.open_prs_lookup_due(&pid));
    assert!(app.pr_lookup_due(&wid));
    assert!(out.is_empty(), "no daemon traffic — gh runs client-side");
}

/// The filter reads each pull request's draft flag as GitHub last gave
/// it, so a draft marked ready joins the rows on the refresh that says
/// so and one turned back into a draft leaves on the next — no toggle
/// involved. And a folded group with nothing listed is not something
/// `↓` steps into.
#[test]
fn a_refresh_that_changes_draft_status_moves_the_row_in_or_out() {
    with_default_config(|| {
        let mut app = App::new();
        seed_tree(&mut app);
        let pid = app.selected_project().expect("a project").id.clone();
        apply_config(
            &mut app,
            &crate::config::Config {
                hide_draft_prs: true,
                ..Default::default()
            },
        );
        let answer = |app: &mut App, list: Vec<crate::pull_request::OpenPr>| {
            note_open_prs_answer(app, pid.clone(), Some(list), &mut Vec::new());
        };

        answer(
            &mut app,
            vec![
                a_pr(9, "Still cooking", true),
                a_pr(7, "Attach links", false),
            ],
        );
        assert_eq!(open_pr_numbers(&app), vec![7]);
        // #9 marked ready for review.
        answer(
            &mut app,
            vec![
                a_pr(9, "Still cooking", false),
                a_pr(7, "Attach links", false),
            ],
        );
        assert_eq!(open_pr_numbers(&app), vec![9, 7], "in, in gh's order");
        // #7 converted back to a draft.
        answer(
            &mut app,
            vec![
                a_pr(9, "Still cooking", false),
                a_pr(7, "Attach links", true),
            ],
        );
        assert_eq!(open_pr_numbers(&app), vec![9], "and out again");

        // Nothing listed once both are drafts: ↓ off the last checkout
        // has no row to open the folded group onto, so it stays put.
        answer(
            &mut app,
            vec![
                a_pr(9, "Still cooking", true),
                a_pr(7, "Attach links", true),
            ],
        );
        assert!(open_pr_numbers(&app).is_empty());
        app.focus = Focus::Worktrees;
        app.open_prs_collapsed = true;
        app.sel_worktree = app.visible_worktrees().len() - 1;
        let mut out = Vec::new();
        press(&mut app, KeyCode::Down, KeyModifiers::NONE, &mut out);
        assert!(app.open_prs_collapsed, "nothing to step into");
        assert!(app.selected_worktree().is_some());
    });
}

/// `/` searches pull requests by title alongside everything else, and
/// Enter on one lands the Worktrees cursor on its OPEN PRS row so the
/// pane reads it — inside nebula, not in a browser.
#[test]
fn the_palette_finds_open_prs_by_title_and_reads_them_in_the_pane() {
    let mut app = App::new();
    seed_tree(&mut app);
    seed_open_prs(
        &mut app,
        &[(7, "Attach links to worktrees"), (9, "Number the lines")],
    );
    app.focus = Focus::Sessions;
    let mut out = Vec::new();

    press(&mut app, KeyCode::Char('/'), KeyModifiers::NONE, &mut out);
    for c in "number".chars() {
        press(&mut app, KeyCode::Char(c), KeyModifiers::NONE, &mut out);
    }
    {
        let p = palette(&app);
        assert_eq!(
            p.items[p.matches[p.selected].item].text, "demo/#9 Number the lines",
            "the project paths it, like every other row"
        );
        assert_eq!(
            p.items[p.matches[p.selected].item].crumb,
            Some((0, 4)),
            "drawn with its project in front: `demo/#9 Number the lines`"
        );
    }
    // Enter lands on the row whether or not "Enter attaches" is on: the
    // setting is about attaching sessions, and a pull request has
    // nothing to attach — the browser is never the quiet default.
    set_enter_attaches(&mut app, true);
    press(&mut app, KeyCode::Enter, KeyModifiers::NONE, &mut out);
    assert!(app.overlay.is_none(), "the palette closes");
    assert_eq!(app.flash, None, "no browser opened: {:?}", app.flash);
    assert_eq!(app.focus, Focus::Worktrees);
    assert_eq!(app.sel_worktree, 2, "the row after the one checkout and #7");
    assert_eq!(
        app.selected_worktree_pr().map(|pr| pr.number),
        Some(9),
        "the Worktrees cursor is on the pull request"
    );
    assert_eq!(
        app.previewed_pr().map(|pr| pr.url),
        Some("https://github.com/o/r/pull/9".into()),
        "the pane is reading it"
    );
    assert_eq!(
        app.pending_pr_detail.as_ref().map(|(p, _)| p.url.as_str()),
        Some("https://github.com/o/r/pull/9"),
        "its description is asked for, as any move onto the row would"
    );
}

/// A folded OPEN PRS group has no rows to land on, so the jump unfolds
/// it first — the row is where the pick is headed, the way ↓ off the
/// last checkout opens the group rather than stopping at its header.
#[test]
fn a_palette_pr_pick_unfolds_the_open_prs_group() {
    let mut app = App::new();
    seed_tree(&mut app);
    seed_open_prs(&mut app, &[(7, "Attach links to worktrees")]);
    app.open_prs_collapsed = true;
    assert_eq!(app.worktree_row_count(), 1, "folded: only the checkout");
    let mut out = Vec::new();

    press(&mut app, KeyCode::Char('/'), KeyModifiers::NONE, &mut out);
    for c in "#7".chars() {
        press(&mut app, KeyCode::Char(c), KeyModifiers::NONE, &mut out);
    }
    press(&mut app, KeyCode::Enter, KeyModifiers::NONE, &mut out);
    assert!(!app.open_prs_collapsed, "the group is open");
    assert_eq!(app.selected_worktree_pr().map(|pr| pr.number), Some(7));
    assert_eq!(app.focus, Focus::Worktrees);
}

/// `Ctrl+O` is the palette's "open the hit", and on a pull request that
/// is the browser — the explicit way out, the same as Enter on the row
/// once landed. It still lands the cursor there first, so the pane is
/// reading the pull request when the browser is dismissed.
#[test]
fn palette_ctrl_o_on_a_pr_lands_on_it_and_opens_the_browser() {
    let mut app = App::new();
    seed_tree(&mut app);
    seed_open_prs(&mut app, &[(7, "Attach links to worktrees")]);
    let mut out = Vec::new();

    press(&mut app, KeyCode::Char('/'), KeyModifiers::NONE, &mut out);
    for c in "attach".chars() {
        press(&mut app, KeyCode::Char(c), KeyModifiers::NONE, &mut out);
    }
    press(
        &mut app,
        KeyCode::Char('o'),
        KeyModifiers::CONTROL,
        &mut out,
    );
    assert!(app.overlay.is_none(), "the palette closes");
    assert_eq!(app.flash.as_deref(), Some("opened github.com/o/r/pull/7"));
    assert_eq!(app.selected_worktree_pr().map(|pr| pr.number), Some(7));
    assert_eq!(app.focus, Focus::Worktrees);
}

/// A pull request row in `/` says where it stands, in words, before it
/// is picked: one ready for review is badged `ready for review` and
/// wears the accent its Worktrees-panel row does; a draft is badged
/// `draft` and dimmed end to end, like that row. The words are the
/// sidebar's — `draft` there too, `ready` cut to fit its column — so
/// the two lists never disagree about a state. And the list refresh
/// that learns a draft was marked ready flips the row under an open
/// palette, without moving the cursor off it.
#[test]
fn palette_pull_request_rows_say_draft_or_ready_for_review_and_follow_the_refresh() {
    let th = crate::theme::Theme::default();
    let mut app = App::new();
    seed_tree(&mut app);
    let pid = app.selected_project().expect("a project").id.clone();
    let answer = |app: &mut App, prs: Vec<(u64, &str, bool)>| {
        let list = prs
            .into_iter()
            .map(|(number, title, is_draft)| crate::pull_request::OpenPr {
                number,
                title: title.into(),
                url: pr_url(number),
                is_draft,
                health: Default::default(),
                head: format!("pr-{number}-head"),
            })
            .collect();
        note_open_prs_answer(app, pid.clone(), Some(list), &mut Vec::new());
    };
    answer(
        &mut app,
        vec![(7, "Attach links", false), (9, "Number the lines", true)],
    );
    let mut out = Vec::new();
    press(&mut app, KeyCode::Char('/'), KeyModifiers::NONE, &mut out);
    // The overview is the sessions; `#` reaches the pull requests, each
    // with its project in front of it like every other row.
    press(&mut app, KeyCode::Char('#'), KeyModifiers::NONE, &mut out);

    let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
    terminal.draw(|f| ui::draw(f, &mut app)).unwrap();
    let text = buffer_text(&terminal);
    assert!(
        text.contains("↗ demo/#7 Attach links ready for review"),
        "a finished pull request says so after its title:\n{text}"
    );
    assert!(
        text.contains("↗ demo/#9 Number the lines draft"),
        "a draft says so after its title:\n{text}"
    );
    // The colors are the sidebar's: accent arrow and plain title for
    // the ready one, dim arrow and dim title for the draft — the same
    // `pr_row::look` both panels paint from.
    let buffer = terminal.backend().buffer();
    let (x, y) = find_cell(&terminal, "↗ demo/#7");
    assert_eq!(buffer[(x, y)].fg, th.accent, "ready: the accent arrow");
    // Past the dim crumb and the `#` the query lit: the title proper.
    let title_x = x + "↗ demo/#7 ".chars().count() as u16;
    assert_eq!(buffer[(title_x, y)].fg, th.text, "ready: the title reads");
    let (x, y) = find_cell(&terminal, "↗ demo/#9");
    assert_eq!(buffer[(x, y)].fg, th.dim, "draft: the dim arrow");
    assert_eq!(buffer[(title_x, y)].fg, th.dim, "draft: dimmed end to end");
    let badge_x = x + "↗ demo/#9 Number the lines ".chars().count() as u16;
    assert_eq!(buffer[(badge_x, y)].fg, th.dim, "draft: the badge is dim");

    // Park the cursor on the draft, then let a refresh say it was
    // marked ready for review: the word flips, the cursor stays.
    if let Some(Overlay::Palette(p)) = &mut app.overlay {
        let row = p
            .matches
            .iter()
            .position(|m| {
                p.items[m.item].target
                    == PaletteTarget::PullRequest {
                        project: pid.clone(),
                        url: pr_url(9),
                    }
            })
            .expect("the draft's row");
        p.select(row as i64);
    }
    answer(
        &mut app,
        vec![(7, "Attach links", false), (9, "Number the lines", false)],
    );
    assert_eq!(
        palette(&app).selected_target(),
        Some(&PaletteTarget::PullRequest {
            project: pid.clone(),
            url: pr_url(9),
        }),
        "the rebuild keeps the cursor on its pull request"
    );
    terminal.draw(|f| ui::draw(f, &mut app)).unwrap();
    let text = buffer_text(&terminal);
    assert!(
        text.contains("↗ demo/#9 Number the lines ready for review"),
        "the refresh flips the word:\n{text}"
    );
    assert!(
        !text.contains(" draft"),
        "no draft is left on screen:\n{text}"
    );

    // And back: a pull request turned into a draft again reads so.
    answer(
        &mut app,
        vec![(7, "Attach links", false), (9, "Number the lines", true)],
    );
    terminal.draw(|f| ui::draw(f, &mut app)).unwrap();
    let text = buffer_text(&terminal);
    assert!(
        text.contains("↗ demo/#9 Number the lines draft"),
        "back to a draft:\n{text}"
    );
}

/// A pull request that merged between the palette listing it and the
/// pick has no row left to land on: the project is selected, and the
/// footer says why the cursor went no further.
#[test]
fn a_palette_pr_pick_that_lost_its_row_flashes() {
    let mut app = App::new();
    seed_tree(&mut app);
    seed_open_prs(&mut app, &[(7, "Attach links to worktrees")]);
    let mut out = Vec::new();

    jump_to_target(
        &mut app,
        PaletteTarget::PullRequest {
            project: nebula_core::ProjectId("p1".into()),
            url: "https://github.com/o/r/pull/8".into(),
        },
        Landing::FocusOnly,
        &mut out,
    );
    assert_eq!(app.flash.as_deref(), Some(PR_GONE));
    assert!(app.selected_worktree_pr().is_none());
    assert_eq!(app.sel_worktree, 0, "the cursor stays on the checkout");
}

/// A space in the query is an AND between terms, not a char to match: the
/// PR row has no space between its project prefix and its `#7`, and
/// "demo #7" still has to find it.
#[test]
fn palette_query_terms_match_independently_across_a_space() {
    let mut app = App::new();
    seed_tree(&mut app);
    seed_open_prs(&mut app, &[(7, "Attach links to worktrees")]);
    let mut out = Vec::new();

    press(&mut app, KeyCode::Char('/'), KeyModifiers::NONE, &mut out);
    for c in "demo #7".chars() {
        press(&mut app, KeyCode::Char(c), KeyModifiers::NONE, &mut out);
    }
    let p = palette(&app);
    assert_eq!(p.query, "demo #7", "the space reaches the query");
    // The one hit, on a line of its own — nothing listed above it just
    // to hold it.
    let texts: Vec<&str> = p
        .matches
        .iter()
        .map(|m| p.items[m.item].text.as_str())
        .collect();
    assert_eq!(texts, vec!["demo/#7 Attach links to worktrees"]);
}

/// A pull request merged or closed on GitHub simply stops coming back
/// from `gh pr list`, and that is the whole retirement mechanism: the
/// next refresh drops the row, the cursor lands on a surviving one
/// rather than on whatever inherited its index, and the body cached for
/// the reading pane is forgotten with it. A draft is an open pull
/// request and comes through the same pass untouched.
#[test]
fn a_merged_pull_request_leaves_the_list_on_the_next_refresh() {
    let mut app = App::new();
    seed_tree(&mut app); // p1 / w1(main) / a1
    let pid = app.selected_project().expect("a project").id.clone();
    let answer = |app: &mut App, prs: Vec<(u64, bool)>| {
        let list = prs
            .into_iter()
            .map(|(number, is_draft)| crate::pull_request::OpenPr {
                number,
                title: format!("pull {number}"),
                url: format!("https://github.com/o/r/pull/{number}"),
                is_draft,
                health: Default::default(),
                head: format!("pr-{number}-head"),
            })
            .collect();
        note_open_prs_answer(app, pid.clone(), Some(list), &mut Vec::new());
    };

    answer(&mut app, vec![(7, false), (9, true)]);
    assert_eq!(app.worktree_row_count(), 3, "the checkout, then both PRs");
    app.pr_detail
        .insert(pr_url(7), a_detail(7, "read on a hover", vec![]));
    app.sel_worktree = 1;
    assert_eq!(app.selected_worktree_pr().map(|p| p.number), Some(7));

    // #7 is merged: the next list doesn't mention it.
    answer(&mut app, vec![(9, true)]);
    assert_eq!(
        open_pr_numbers(&app),
        vec![9],
        "the draft is still open and stays; the merged one goes"
    );
    assert_eq!(
        app.selected_worktree_pr().map(|p| p.number),
        Some(9),
        "the cursor lands on the row that survived"
    );
    assert!(
        !app.pr_detail.contains_key(&pr_url(7)),
        "and the body cached for the reading pane goes with it"
    );

    // The last one closes too: the cursor falls back to the checkout.
    answer(&mut app, vec![]);
    assert!(app.visible_open_prs().is_empty());
    assert!(
        app.selected_worktree().is_some(),
        "the cursor lands on the checkout, not past the end of the list"
    );
}

/// A refresh that merely reorders the list keeps the cursor on the pull
/// request it was reading, not on whatever now holds that index — `gh`
/// sorts newest first, so anyone opening a PR reshuffles everything
/// below it. (A finished one: a new *draft* would sink below the cursor
/// instead and reshuffle nothing.)
#[test]
fn the_cursor_follows_its_pull_request_across_a_reorder() {
    let mut app = App::new();
    seed_tree(&mut app);
    seed_open_prs(&mut app, &[(9, "Number lines"), (7, "Attach links")]);
    app.sel_worktree = 2;
    assert_eq!(app.selected_worktree_pr().map(|p| p.number), Some(7));

    let pid = app.selected_project().expect("a project").id.clone();
    let list = vec![
        crate::pull_request::OpenPr {
            number: 11,
            title: "Brand new".into(),
            url: pr_url(11),
            is_draft: false,
            health: Default::default(),
            head: "brand-new".into(),
        },
        crate::pull_request::OpenPr {
            number: 9,
            title: "Number lines".into(),
            url: pr_url(9),
            is_draft: false,
            health: Default::default(),
            head: "number-lines".into(),
        },
        crate::pull_request::OpenPr {
            number: 7,
            title: "Attach links".into(),
            url: pr_url(7),
            is_draft: false,
            health: Default::default(),
            head: "attach-links".into(),
        },
    ];
    // Halfway down #7's conversation when the refresh lands.
    app.pr_preview_scroll = 12;
    note_open_prs_answer(&mut app, pid, Some(list), &mut Vec::new());
    assert_eq!(open_pr_numbers(&app), vec![11, 9, 7]);
    assert_eq!(
        app.selected_worktree_pr().map(|p| p.number),
        Some(7),
        "still on #7, two rows further down"
    );
    assert_eq!(app.sel_worktree, 3);
    assert_eq!(
        app.pr_preview_scroll, 12,
        "and still where they were reading — a beat this quick must not \
             rewind the pane under them"
    );
}

/// The detail fetched for the row under the cursor is GitHub's answer
/// about that one pull request, so a `MERGED` or `CLOSED` state retires
/// the row on the spot instead of leaving the user reading something
/// the next refresh is about to take away.
#[test]
fn a_detail_that_says_merged_retires_the_row_on_the_spot() {
    let mut app = App::new();
    seed_tree(&mut app);
    seed_open_prs(&mut app, &[(7, "Attach links"), (9, "Number lines")]);
    app.sel_worktree = 1;

    let mut merged = a_detail(7, "shipped", vec![]);
    merged.state = "MERGED".into();
    assert!(!merged.is_open());
    app.pr_detail.insert(pr_url(7), merged);
    drop_retired_pr(&mut app, &pr_url(7), &mut Vec::new());

    assert_eq!(open_pr_numbers(&app), vec![9]);
    assert_eq!(app.selected_worktree_pr().map(|p| p.number), Some(9));
    assert_eq!(
        app.flash.as_deref(),
        Some("#7 is no longer open"),
        "a row that evaporates mid-read says why"
    );

    // A draft is open, so nothing about it is retired.
    let mut draft = a_detail(9, "still cooking", vec![]);
    draft.is_draft = true;
    assert!(draft.is_open(), "a draft is an open pull request");
}

/// The checkout's own PR ROW keeps a merged pull request, so GitHub's
/// answer about that one pull request — fetched for the pane — flips
/// the row's badge on the spot instead of retiring it, and the list
/// refresh's cache sweep leaves its body alone: the row is still on
/// screen, and re-fetching what it reads on every beat would spend an
/// API call a refresh.
#[test]
fn a_detail_that_says_merged_flips_the_branch_row_and_keeps_its_body() {
    let mut app = App::new();
    seed_tree(&mut app);
    seed_branch_pr(&mut app, 7, "Attach links");
    let pid = app.selected_project().expect("a project").id.clone();
    let wid = nebula_core::WorktreeId("w1".into());
    let branch_pr = |app: &App| {
        app.pull_requests
            .get(&wid)
            .cloned()
            .flatten()
            .expect("the branch's pull request")
    };
    assert_eq!(branch_pr(&app).badge(), "ready");

    let mut merged = a_detail(7, "shipped", vec![]);
    merged.state = "MERGED".into();
    adopt_pr_state(&mut app, &merged);
    assert_eq!(
        branch_pr(&app).badge(),
        "merged",
        "the row wears the answer"
    );
    assert!(app.dirty, "and repaints for it");
    assert_eq!(app.visible_links().len(), 1, "but the row is not retired");

    app.pr_detail.insert(pr_url(7), merged);
    // The project's open list no longer carries #7 — it is merged — and
    // its refresh sweeps the detail cache.
    note_open_prs_answer(&mut app, pid, Some(vec![]), &mut Vec::new());
    assert!(
        app.pr_detail.contains_key(&pr_url(7)),
        "the body of a pull request still on a row is kept"
    );

    // A detail for some other pull request leaves the row alone.
    let other = a_detail(9, "unrelated", vec![]);
    app.dirty = false;
    adopt_pr_state(&mut app, &other);
    assert_eq!(branch_pr(&app).badge(), "merged");
    assert!(!app.dirty);
}

fn pr_url(number: u64) -> String {
    format!("https://github.com/o/r/pull/{number}")
}

/// One `gh pr list` row, as `parse_list` would build it.
fn a_pr(number: u64, title: &str, is_draft: bool) -> crate::pull_request::OpenPr {
    crate::pull_request::OpenPr {
        number,
        title: title.into(),
        url: pr_url(number),
        is_draft,
        health: Default::default(),
        head: format!("pr-{number}-head"),
    }
}

fn open_pr_numbers(app: &App) -> Vec<u64> {
    app.visible_open_prs().iter().map(|p| p.number).collect()
}

fn a_detail(
    number: u64,
    body: &str,
    comments: Vec<crate::pull_request::PrComment>,
) -> crate::pull_request::PrDetail {
    crate::pull_request::PrDetail {
        number,
        url: format!("https://github.com/o/r/pull/{number}"),
        title: "Attach links".into(),
        state: "OPEN".into(),
        is_draft: false,
        health: Default::default(),
        author: "webdevcody".into(),
        base: "main".into(),
        head: "feat/links".into(),
        additions: 106,
        deletions: 4,
        changed_files: 2,
        body: body.into(),
        comments,
    }
}

/// A pull request `gh` couldn't read says so rather than sitting on
/// "reading it…" forever.
#[test]
fn an_unreadable_pull_request_says_so_in_the_pane() {
    let mut app = App::new();
    seed_tree(&mut app);
    seed_open_prs(&mut app, &[(7, "Attach links")]);
    app.focus = Focus::Worktrees;
    app.sel_worktree = 1;
    app.pr_detail_failed
        .insert("https://github.com/o/r/pull/7".into());
    let mut terminal = Terminal::new(TestBackend::new(140, 30)).unwrap();
    terminal.draw(|f| ui::draw(f, &mut app)).unwrap();
    let text = buffer_text(&terminal);
    assert!(text.contains("unavailable"), "{text}");
    assert!(text.contains("couldn't read this pull request"), "{text}");
}

/// Seed the Sessions panel's PR ROW: the pull request `gh` found on the
/// seeded worktree's branch.
fn seed_branch_pr(app: &mut App, number: u64, title: &str) {
    app.pull_requests.insert(
        nebula_core::WorktreeId("w1".into()),
        Some(crate::pull_request::PullRequest {
            number,
            url: format!("https://github.com/o/r/pull/{number}"),
            title: title.into(),
            state: crate::pull_request::STATE_OPEN.into(),
            is_draft: false,
            health: Default::default(),
            activity: Vec::new(),
        }),
    );
}

fn sessions_pr_row(app: &App) -> usize {
    app.visible_session_rows()
        .iter()
        .position(|r| r.as_link().is_some())
        .expect("the PR ROW")
}

/// The loop notices the pane reading something else by URL: landing on
/// the PR ROW arms one debounced fetch, a same-URL turn leaves a
/// reader's scroll alone, and leaving it (for the pane here) disarms.
/// The `/` PALETTE is what lands a cursor on a pull request now, so
/// the cursor is put there rather than walked there.
#[test]
fn stepping_onto_the_sessions_pr_row_arms_the_detail_fetch() {
    let mut app = App::new();
    seed_tree(&mut app);
    seed_branch_pr(&mut app, 7, "Attach links");
    app.focus = Focus::Sessions;

    assert!(
        app.pending_pr_detail.is_none(),
        "a session row arms nothing"
    );
    let before = app.previewed_pr().map(|pr| pr.url);
    app.sel_session = sessions_pr_row(&app);
    note_preview_change(&mut app, before);
    let (pending, _) = app.pending_pr_detail.clone().expect("armed on #7");
    assert_eq!(pending.number, 7);
    assert_eq!(pending.url, "https://github.com/o/r/pull/7");
    assert_eq!(pending.dir, std::path::PathBuf::from("/tmp/demo"));
    assert!(
        app.pr_detail_delay()
            .is_some_and(|d| d <= PR_DETAIL_DEBOUNCE),
        "a delay, not an immediate fetch"
    );

    // A turn that changes nothing about the row keeps the reader's place.
    app.pr_preview_scroll = 9;
    let before = app.previewed_pr().map(|pr| pr.url);
    note_preview_change(&mut app, before);
    assert_eq!(app.pr_preview_scroll, 9);
    assert!(app.pending_pr_detail.is_some(), "still armed");

    // Focus into the pane: a terminal has nothing to fetch.
    let before = app.previewed_pr().map(|pr| pr.url);
    app.focus = Focus::Terminal;
    note_preview_change(&mut app, before);
    assert!(app.pending_pr_detail.is_none());
    assert_eq!(app.pr_preview_scroll, 0);
}

/// `g` on the Sessions PR ROW asks GitHub for that pull request's diff
/// rather than opening the checkout's, and its row menu offers the same.
#[test]
fn g_on_the_sessions_pr_row_reads_its_diff() {
    let mut app = App::new();
    seed_tree(&mut app);
    seed_branch_pr(&mut app, 7, "Attach links");
    app.focus = Focus::Sessions;
    app.sel_session = sessions_pr_row(&app);
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    app.pr_diff_tx = Some(tx);
    app.pr_diff_inflight = Some(7);
    let mut out = Vec::new();

    press(&mut app, KeyCode::Char('g'), KeyModifiers::NONE, &mut out);
    assert!(app.overlay.is_none(), "not the worktree's diff modal");
    assert_eq!(
        app.flash.as_deref(),
        Some("still fetching the diff for #7…")
    );

    let items = menu_items_for_link(&app.selected_link().expect("the PR ROW"));
    assert!(items.iter().any(|i| i.label == "View diff"), "{items:?}");
    assert!(
        !items.iter().any(|i| i.label == "Delete"),
        "still not the user's row: {items:?}"
    );
}

/// `y` on a pull request row — the PROJECT OPEN PRS GROUP's or the
/// Sessions panel's PR ROW — opens the COMMENT BOX: a multi-row task
/// box titled with the row, carrying the pull request Enter will post
/// to. Off a pull request row the key only says what it wants, and
/// both rows' menus offer the same box as **Comment…**.
#[test]
fn y_on_a_pull_request_row_opens_the_comment_box() {
    let mut app = App::new();
    seed_tree(&mut app);
    seed_open_prs(&mut app, &[(7, "Attach links")]);
    app.focus = Focus::Worktrees;
    app.sel_worktree = 1;
    let mut out = Vec::new();
    press(&mut app, KeyCode::Char('y'), KeyModifiers::NONE, &mut out);
    let Some(Overlay::Prompt(prompt)) = &app.overlay else {
        panic!("expected the COMMENT BOX, got {:?}", app.overlay);
    };
    assert!(
        prompt.is_multiline(),
        "a comment is a task box, not a one-liner"
    );
    assert_eq!(prompt.title, "Comment on #7 Attach links");
    assert!(
        matches!(&prompt.kind, PromptKind::PrComment { number: 7, url, .. } if *url == pr_url(7)),
        "{:?}",
        prompt.kind
    );
    assert!(out.is_empty(), "opening the box sends the daemon nothing");

    // The row's menu offers the same box.
    app.overlay = None;
    let pr = app.all_open_prs()[0].clone();
    let items = pr_row_menu_items(&app, &pr);
    assert!(
        items
            .iter()
            .any(|i| i.label == "Comment…" && i.action == MenuAction::CommentPullRequest),
        "{items:?}"
    );
    run_menu_action(&mut app, MenuAction::CommentPullRequest, &mut out);
    assert!(
        matches!(&app.overlay, Some(Overlay::Prompt(p))
                if matches!(&p.kind, PromptKind::PrComment { number: 7, .. })),
        "{:?}",
        app.overlay
    );

    // The Sessions panel's PR ROW opens it too, and so does its menu.
    app.overlay = None;
    app.sel_worktree = 0;
    seed_branch_pr(&mut app, 9, "Fix login");
    app.focus = Focus::Sessions;
    app.sel_session = sessions_pr_row(&app);
    press(&mut app, KeyCode::Char('y'), KeyModifiers::NONE, &mut out);
    assert!(
        matches!(&app.overlay, Some(Overlay::Prompt(p))
                if matches!(&p.kind, PromptKind::PrComment { number: 9, .. })),
        "{:?}",
        app.overlay
    );
    let items = menu_items_for_link(&app.selected_link().expect("the PR ROW"));
    assert!(items.iter().any(|i| i.label == "Comment…"), "{items:?}");

    // Off a pull request row, on a card, `y` comments on the card's
    // pull request — its checkout's, the one `⇧V` opens.
    app.overlay = None;
    let pr_row = sessions_pr_row(&app);
    app.sel_session = (0..app.visible_session_rows().len())
        .find(|i| *i != pr_row)
        .expect("an agent row");
    press(&mut app, KeyCode::Char('y'), KeyModifiers::NONE, &mut out);
    assert!(
        matches!(&app.overlay, Some(Overlay::Prompt(p))
                if matches!(&p.kind, PromptKind::PrComment { number: 9, .. })),
        "{:?}",
        app.overlay
    );
}

/// Enter in the COMMENT BOX hands the text to `gh` off the loop, and
/// what lands decides the rest. A post that cannot start — the
/// checkout is not on disk, one is already running on this pull
/// request — hands the box straight back with the text; a refusal
/// from `gh` does the same in its words, or keeps the text for the
/// next box when another modal is up; a success says so and re-reads
/// the pull request so the pane shows the comment, the reader's place
/// kept. An empty box is a change of mind.
#[test]
fn the_comment_box_posts_on_enter_and_never_loses_the_text() {
    let mut app = App::new();
    seed_tree(&mut app);
    seed_open_prs(&mut app, &[(7, "Attach links")]);
    app.focus = Focus::Worktrees;
    app.sel_worktree = 1;
    app.tree.projects[0].repo_path = "/nonexistent/nebula-demo".into();
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    app.pr_comment_tx = Some(tx);
    let mut out = Vec::new();
    let answer = |body: &str, result: Result<String, String>| PrCommentAnswer {
        number: 7,
        url: pr_url(7),
        label: "#7 Attach links".into(),
        body: body.into(),
        result,
    };

    // Nothing typed: the box closes and nothing is posted.
    press(&mut app, KeyCode::Char('y'), KeyModifiers::NONE, &mut out);
    press(&mut app, KeyCode::Enter, KeyModifiers::NONE, &mut out);
    assert!(app.overlay.is_none(), "{:?}", app.overlay);
    assert_eq!(app.flash.as_deref(), Some("cancelled: empty input"));

    // Shift+Enter breaks a line; Enter posts — here from a checkout
    // that is not on disk, so the box comes back with the text.
    press(&mut app, KeyCode::Char('y'), KeyModifiers::NONE, &mut out);
    type_text(&mut app, "Looks good", &mut out);
    press(&mut app, KeyCode::Enter, KeyModifiers::SHIFT, &mut out);
    type_text(&mut app, "to me", &mut out);
    press(&mut app, KeyCode::Enter, KeyModifiers::NONE, &mut out);
    let Some(Overlay::Prompt(prompt)) = &app.overlay else {
        panic!("expected the box back, got {:?}", app.overlay);
    };
    assert_eq!(prompt.input.as_str(), "Looks good\nto me");
    assert_eq!(
        app.flash.as_deref(),
        Some("repo path missing on disk: /nonexistent/nebula-demo")
    );
    assert!(app.pr_comment_inflight.is_empty(), "nothing started");
    assert!(rx.try_recv().is_err(), "nothing posted");

    // One already on its way to this pull request: wait, text kept.
    app.pr_comment_inflight.insert(pr_url(7));
    press(&mut app, KeyCode::Enter, KeyModifiers::NONE, &mut out);
    assert!(
        matches!(&app.overlay, Some(Overlay::Prompt(p)) if p.input.as_str() == "Looks good\nto me"),
        "{:?}",
        app.overlay
    );
    assert_eq!(
        app.flash.as_deref(),
        Some("still posting the last comment on #7…")
    );
    app.pr_comment_inflight.clear();
    app.overlay = None;

    // A refusal lands in gh's words and the box comes back with the text.
    land_pr_comment(&mut app, answer("Looks good", Err("not logged in".into())));
    assert_eq!(
        app.flash.as_deref(),
        Some("couldn't post the comment on #7: not logged in")
    );
    assert!(
        matches!(&app.overlay, Some(Overlay::Prompt(p))
                if p.input.as_str() == "Looks good" && p.title == "Comment on #7 Attach links"),
        "{:?}",
        app.overlay
    );

    // Landing under another modal leaves it up and keeps the text for
    // the next box on this pull request.
    app.overlay = Some(Overlay::Help(HelpView::default()));
    land_pr_comment(&mut app, answer("Second try", Err("no network".into())));
    assert!(
        matches!(&app.overlay, Some(Overlay::Help(_))),
        "{:?}",
        app.overlay
    );
    app.overlay = None;
    press(&mut app, KeyCode::Char('y'), KeyModifiers::NONE, &mut out);
    assert!(
        matches!(&app.overlay, Some(Overlay::Prompt(p)) if p.input.as_str() == "Second try"),
        "{:?}",
        app.overlay
    );
    app.overlay = None;
    assert!(app.pr_comment_drafts.is_empty(), "the draft was consumed");

    // Posted: the pane re-reads the pull request it is on, in place.
    app.pr_detail.insert(pr_url(7), a_detail(7, "body", vec![]));
    app.pending_pr_detail = None;
    app.pr_preview_scroll = 3;
    land_pr_comment(
        &mut app,
        answer("Looks good", Ok(format!("{}#issuecomment-1", pr_url(7)))),
    );
    assert_eq!(app.flash.as_deref(), Some("comment posted on #7"));
    assert!(
        app.pending_pr_detail
            .as_ref()
            .is_some_and(|(p, _)| p.url == pr_url(7)),
        "the conversation is asked for again: {:?}",
        app.pending_pr_detail
    );
    assert_eq!(app.pr_preview_scroll, 3, "the reader's place is kept");
    assert!(app.overlay.is_none());

    // Posted on a pull request the pane has since left: stale, so the
    // next visit reads it afresh.
    app.sel_worktree = 0;
    app.pending_pr_detail = None;
    land_pr_comment(&mut app, answer("Looks good", Ok(String::new())));
    assert!(app.pending_pr_detail.is_none());
    assert!(app.pr_detail_stale.contains(&pr_url(7)));
}

/// PgDn/PgUp/Home/End page the preview, clamped to its real length —
/// the pane writes the line count back on every draw.
#[test]
fn the_preview_pages_and_clamps_to_its_length() {
    let mut app = App::new();
    seed_tree(&mut app);
    seed_open_prs(&mut app, &[(7, "Attach links")]);
    app.focus = Focus::Worktrees;
    app.sel_worktree = 1;
    let body = (0..200)
        .map(|i| format!("line {i}"))
        .collect::<Vec<_>>()
        .join("\n");
    app.pr_detail.insert(
        "https://github.com/o/r/pull/7".into(),
        a_detail(7, &body, vec![]),
    );
    let mut terminal = Terminal::new(TestBackend::new(140, 30)).unwrap();
    terminal.draw(|f| ui::draw(f, &mut app)).unwrap();
    assert!(app.pr_preview_lines > 200, "the body wrapped long");

    let mut out = Vec::new();
    press(&mut app, KeyCode::PageDown, KeyModifiers::NONE, &mut out);
    let paged = app.pr_preview_scroll;
    assert!(paged > 0, "PgDn moved");
    press(&mut app, KeyCode::End, KeyModifiers::NONE, &mut out);
    assert_eq!(app.pr_preview_scroll, app.pr_preview_max_scroll());
    press(&mut app, KeyCode::PageDown, KeyModifiers::NONE, &mut out);
    assert_eq!(
        app.pr_preview_scroll,
        app.pr_preview_max_scroll(),
        "the end is the end"
    );
    press(&mut app, KeyCode::Home, KeyModifiers::NONE, &mut out);
    assert_eq!(app.pr_preview_scroll, 0);
    assert!(out.is_empty(), "reading a PR sends the daemon nothing");

    // Moving to another row starts its preview at the top.
    app.pr_preview_scroll = paged;
    app.sel_worktree = 0;
    schedule_pr_detail(&mut app);
    assert_eq!(app.pr_preview_scroll, 0);
}

/// `g` on a pull-request row opens the ordinary diff modal on the
/// fetched diff — file list from the diff itself, and switching files
/// reads the text already in hand rather than shelling out at git.
#[test]
fn the_fetched_pr_diff_opens_in_the_diff_modal() {
    let mut app = App::new();
    seed_tree(&mut app);
    seed_open_prs(&mut app, &[(7, "Attach links")]);
    app.sel_worktree = 1;
    let diff = "\
diff --git a/src/a.rs b/src/a.rs
--- a/src/a.rs
+++ b/src/a.rs
@@ -1 +1 @@
-old
+new
diff --git a/src/b.rs b/src/b.rs
--- a/src/b.rs
+++ b/src/b.rs
@@ -1 +1 @@
-x
+y
";
    app.pr_diff_inflight = Some(7);
    open_pr_diff_view(
        &mut app,
        7,
        &pr_url(7),
        "#7 Attach links".into(),
        Some(diff.into()),
    );
    assert!(app.pr_diff_inflight.is_none(), "the fetch is done");
    let Some(Overlay::Diff(view)) = &app.overlay else {
        panic!("expected the diff modal, got {:?}", app.overlay);
    };
    assert_eq!(view.branch, "#7 Attach links", "the PR titles the modal");
    assert_eq!(
        view.files
            .iter()
            .map(|f| f.path.as_str())
            .collect::<Vec<_>>(),
        ["src/a.rs", "src/b.rs"]
    );
    assert!(
        view.diff.contains("+new"),
        "first file's diff: {}",
        view.diff
    );

    // Selecting the second file reads the prefetched chunk — no repo is
    // touched (seed_tree's /tmp/demo isn't even a git checkout).
    let Some(Overlay::Diff(view)) = &mut app.overlay else {
        unreachable!()
    };
    view.select(1);
    crate::git_diff::load_selected_diff(view);
    assert!(
        view.diff.contains("+y"),
        "second file's diff: {}",
        view.diff
    );
    assert!(!view.diff.contains("+new"), "chunks don't bleed");
}

/// A pull request's diff spread over two directories, one of them a
/// chain the tree folds into a single row.
const TREE_PR_DIFF: &str = "\
diff --git a/crates/tui/src/a.rs b/crates/tui/src/a.rs
--- a/crates/tui/src/a.rs
+++ b/crates/tui/src/a.rs
@@ -1 +1 @@
-old
+new-a
diff --git a/crates/tui/src/b.rs b/crates/tui/src/b.rs
--- a/crates/tui/src/b.rs
+++ b/crates/tui/src/b.rs
@@ -1 +1 @@
-x
+new-b
diff --git a/docs/keys.md b/docs/keys.md
--- a/docs/keys.md
+++ b/docs/keys.md
@@ -1 +1 @@
-k
+new-keys
";

/// An app with the DIFF modal up on [`TREE_PR_DIFF`], flat or as the
/// tree per `app.diff_tree` — which `tree` sets before opening.
fn pr_diff_app(tree: bool) -> App {
    let mut app = App::new();
    seed_tree(&mut app);
    seed_open_prs(&mut app, &[(7, "Attach links")]);
    app.sel_worktree = 1;
    app.diff_tree = tree;
    open_pr_diff_view(
        &mut app,
        7,
        &pr_url(7),
        "#7 Attach links".into(),
        Some(TREE_PR_DIFF.into()),
    );
    app
}

/// The tree list's rows as the panel names them, the cursor's starred.
fn diff_tree_rows(app: &App) -> Vec<String> {
    let tree = diff_view(app).tree.as_ref().expect("the tree list");
    tree.rows
        .iter()
        .enumerate()
        .map(|(i, r)| {
            let star = if i == tree.selected { "*" } else { "" };
            format!("{star}{}", tree.nodes[r.node].name)
        })
        .collect()
}

/// `Ctrl+t` folds the pull request's file list into a directory tree
/// and back. The reader keeps their file and their place in it both
/// ways; a directory's row reads as the list of what changed under it,
/// and leaving the tree from one lands on that directory's first file.
#[test]
fn ctrl_t_flips_the_pr_diff_file_list_between_flat_and_tree() {
    let mut app = pr_diff_app(false);
    let mut out = Vec::new();
    press(&mut app, KeyCode::Down, KeyModifiers::NONE, &mut out);
    assert!(diff_view(&app).diff.contains("+new-b"));
    if let Some(Overlay::Diff(view)) = &mut app.overlay {
        view.view_height = 2;
        view.scroll = 3;
    }

    ctrl(&mut app, 't', &mut out);
    assert_eq!(
        diff_tree_rows(&app),
        ["crates/tui/src", "a.rs", "*b.rs", "docs", "keys.md"],
        "every directory open, the chain one row, the cursor still on b.rs"
    );
    assert!(diff_view(&app).diff.contains("+new-b"));
    assert_eq!(diff_view(&app).scroll, 3, "same file: the place is kept");
    assert!(app.diff_tree, "remembered for the next open");

    let mut terminal = Terminal::new(TestBackend::new(140, 30)).unwrap();
    terminal.draw(|f| ui::draw(f, &mut app)).unwrap();
    let text = buffer_text(&terminal);
    assert!(text.contains("▾ crates/tui/src"), "{text}");
    assert!(text.contains("^t: flat list"), "{text}");
    assert!(text.contains("←/→: fold"), "{text}");

    // Up onto the directory's row: the pane lists what is under it.
    press(&mut app, KeyCode::Up, KeyModifiers::NONE, &mut out);
    press(&mut app, KeyCode::Up, KeyModifiers::NONE, &mut out);
    let view = diff_view(&app);
    assert_eq!(view.selected_dir(), Some("crates/tui/src"));
    assert!(view.selected_file().is_none());
    assert_eq!(
        view.diff,
        "crates/tui/src/ — 2 changed files\n\nM    a.rs\nM    b.rs"
    );
    terminal.draw(|f| ui::draw(f, &mut app)).unwrap();
    let text = buffer_text(&terminal);
    assert!(text.contains("#7 Attach links: crates/tui/src/"), "{text}");

    // ← folds it, → opens it again, a second → steps inside.
    press(&mut app, KeyCode::Left, KeyModifiers::NONE, &mut out);
    assert_eq!(diff_tree_rows(&app), ["*crates/tui/src", "docs", "keys.md"]);
    press(&mut app, KeyCode::Right, KeyModifiers::NONE, &mut out);
    press(&mut app, KeyCode::Right, KeyModifiers::NONE, &mut out);
    assert_eq!(
        diff_tree_rows(&app),
        ["crates/tui/src", "*a.rs", "b.rs", "docs", "keys.md"]
    );
    assert!(diff_view(&app).diff.contains("+new-a"));

    // Back to the flat list from a directory's row: its first file.
    press(&mut app, KeyCode::Down, KeyModifiers::NONE, &mut out);
    press(&mut app, KeyCode::Down, KeyModifiers::NONE, &mut out);
    assert_eq!(diff_view(&app).selected_dir(), Some("docs"));
    ctrl(&mut app, 't', &mut out);
    let view = diff_view(&app);
    assert!(view.tree.is_none());
    assert_eq!(view.selected_file().unwrap().path, "docs/keys.md");
    assert!(view.diff.contains("+new-keys"), "{}", view.diff);
    assert!(!app.diff_tree);
    assert!(out.is_empty(), "the list's shape is nebula's own business");
}

/// INPUT PARITY: Enter on a tree directory's row and a click on it are
/// the same choice — the row folds, the cursor rests on it — and a
/// click on a file's row is ↓ onto it.
#[test]
fn a_click_on_a_diff_tree_row_is_enter_on_it() {
    let mut keyed = pr_diff_app(true);
    let mut clicked = pr_diff_app(true);
    let mut out = Vec::new();
    assert_eq!(
        diff_tree_rows(&keyed),
        ["crates/tui/src", "*a.rs", "b.rs", "docs", "keys.md"],
        "a tree opens on its first file, not on the directory above it"
    );
    let mut terminal = Terminal::new(TestBackend::new(140, 30)).unwrap();
    terminal.draw(|f| ui::draw(f, &mut clicked)).unwrap();
    let list = diff_view(&clicked).list_area;
    let click = |app: &mut App, row: u16, out: &mut Vec<ClientRequest>| {
        handle_mouse(
            app,
            mev(
                MouseEventKind::Down(MouseButton::Left),
                list.x + 8,
                list.y + row,
            ),
            out,
        );
    };

    press(&mut keyed, KeyCode::Up, KeyModifiers::NONE, &mut out);
    press(&mut keyed, KeyCode::Enter, KeyModifiers::NONE, &mut out);
    click(&mut clicked, 0, &mut out);
    assert_eq!(
        diff_tree_rows(&keyed),
        ["*crates/tui/src", "docs", "keys.md"]
    );
    assert_eq!(diff_tree_rows(&clicked), diff_tree_rows(&keyed));
    assert_eq!(diff_view(&clicked).diff, diff_view(&keyed).diff);

    // A file's row: the cursor lands and its diff is read, nothing folds.
    press(&mut keyed, KeyCode::Down, KeyModifiers::NONE, &mut out);
    press(&mut keyed, KeyCode::Down, KeyModifiers::NONE, &mut out);
    press(&mut keyed, KeyCode::Enter, KeyModifiers::NONE, &mut out);
    click(&mut clicked, 2, &mut out);
    assert_eq!(
        diff_tree_rows(&keyed),
        ["crates/tui/src", "docs", "*keys.md"]
    );
    assert_eq!(diff_tree_rows(&clicked), diff_tree_rows(&keyed));
    assert!(diff_view(&clicked).diff.contains("+new-keys"));
    assert!(out.is_empty());
}

/// The tree filters the way the TREE BROWSER does — matching files
/// under their directories, the cursor parked on the best match — and
/// `Ctrl+r` sweeps down the files without stopping on a directory.
#[test]
fn the_diff_tree_filters_and_sweeps_reviewed_marks() {
    let mut app = pr_diff_app(true);
    let mut out = Vec::new();
    for c in "keys".chars() {
        press(&mut app, KeyCode::Char(c), KeyModifiers::NONE, &mut out);
    }
    assert_eq!(diff_tree_rows(&app), ["docs", "*keys.md"]);
    assert!(diff_view(&app).diff.contains("+new-keys"));
    press(&mut app, KeyCode::Esc, KeyModifiers::NONE, &mut out);
    assert_eq!(
        diff_tree_rows(&app),
        ["crates/tui/src", "*a.rs", "b.rs", "docs", "keys.md"],
        "Esc clears the filter first, and the cursor goes home"
    );

    ctrl(&mut app, 'r', &mut out);
    ctrl(&mut app, 'r', &mut out);
    assert_eq!(
        diff_tree_rows(&app),
        ["crates/tui/src", "a.rs", "b.rs", "docs", "*keys.md"],
        "two marks later the cursor is past `docs`, on the next file"
    );
    let view = diff_view(&app);
    assert!(view.reviewed.contains_key("crates/tui/src/a.rs"));
    assert!(view.reviewed.contains_key("crates/tui/src/b.rs"));
    assert!(view.diff.contains("+new-keys"));
    let tree = view.tree.as_ref().unwrap();
    let done = tree.reviewed_nodes(&view.files, &view.reviewed);
    assert!(done[tree.rows[0].node], "a directory read end to end");
    assert!(!done[tree.rows[3].node]);
}

/// A fresh `gh pr diff` landing under the tree keeps what the reader
/// folded and the file they were on; `diff_tree` rides the UI state
/// blob, so the next launch opens the modal the way this one left it.
#[test]
fn the_diff_tree_survives_a_refresh_and_a_relaunch() {
    let mut app = pr_diff_app(true);
    let mut out = Vec::new();
    for _ in 0..3 {
        press(&mut app, KeyCode::Down, KeyModifiers::NONE, &mut out);
    }
    assert_eq!(
        diff_tree_rows(&app),
        ["crates/tui/src", "a.rs", "b.rs", "docs", "*keys.md"]
    );
    let Some(Overlay::Diff(view)) = &mut app.overlay else {
        unreachable!()
    };
    view.toggle_dir(0);
    let fresh = format!(
            "{TREE_PR_DIFF}diff --git a/docs/new.md b/docs/new.md\n--- /dev/null\n+++ b/docs/new.md\n@@ -0,0 +1 @@\n+n\n"
        );
    assert!(refresh_pr_diff_view(view, &fresh));
    assert_eq!(
        diff_tree_rows(&app),
        ["crates/tui/src", "docs", "*keys.md", "new.md"]
    );
    assert!(diff_view(&app).diff.contains("+new-keys"));

    let json = ui_state_json(&app);
    let mut next = App::new();
    seed_tree(&mut next);
    restore_ui_state(&mut next, &json);
    assert!(next.diff_tree);
    // A blob from before the tree existed keeps the flat list.
    restore_ui_state(&mut next, "{\"show_archived\":false,\"collapsed\":false}");
    assert!(!next.diff_tree);
}

/// A diff `gh` couldn't fetch flashes and leaves the modal shut, and a
/// second `g` while one is already in flight doesn't stack a request.
#[test]
fn a_failed_pr_diff_flashes_instead_of_opening() {
    let mut app = App::new();
    seed_tree(&mut app);
    seed_open_prs(&mut app, &[(7, "Attach links")]);
    app.sel_worktree = 1;

    app.pr_diff_inflight = Some(7);
    open_pr_diff_view(&mut app, 7, &pr_url(7), "#7 Attach links".into(), None);
    assert!(app.overlay.is_none());
    assert!(
        app.flash
            .as_deref()
            .is_some_and(|f| f.contains("couldn't read the diff for #7")),
        "got {:?}",
        app.flash
    );

    // An empty diff is not a modal with no rows in it.
    open_pr_diff_view(&mut app, 7, &pr_url(7), "#7".into(), Some(String::new()));
    assert!(app.overlay.is_none());
    assert_eq!(app.flash.as_deref(), Some("#7 changes no files"));

    // Mashing the key while one is in flight is a nudge, not a request.
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    app.pr_diff_tx = Some(tx);
    app.pr_diff_inflight = Some(7);
    request_pr_diff(&mut app);
    assert_eq!(
        app.flash.as_deref(),
        Some("still fetching the diff for #7…")
    );
}

/// A pull request as the cache would hand it back, for the rows.
fn cached_pr(number: u64) -> crate::pull_request::PullRequest {
    crate::pull_request::PullRequest {
        number,
        url: pr_url(number),
        title: format!("PR {number}"),
        state: crate::pull_request::STATE_OPEN.into(),
        is_draft: false,
        health: Default::default(),
        activity: Vec::new(),
    }
}

fn cached_open(number: u64) -> crate::pull_request::OpenPr {
    crate::pull_request::OpenPr {
        number,
        title: format!("PR {number}"),
        url: pr_url(number),
        is_draft: false,
        health: Default::default(),
        head: format!("head-{number}"),
    }
}

/// Only a merge seen to happen starts the ONE-SHOT SWEEP: the last
/// answer was something other than merged, and this one is. A
/// checkout's first answer ever — and the first beat of a row the cache
/// hydrated as merged — landed some other day: solid purple, no sweep.
#[test]
fn only_a_merge_seen_to_happen_is_stamped() {
    let merged = |number| {
        let mut pr = cached_pr(number);
        pr.state = crate::pull_request::STATE_MERGED.into();
        pr
    };
    let mut app = App::new();
    seed_tree(&mut app);
    let w1 = nebula_core::WorktreeId("w1".into());

    land_pull_request(&mut app, w1.clone(), Lookup::Found(merged(7)));
    assert!(app.worktree_wears_merge(&w1));
    assert!(!app.merge_is_fresh(&w1), "met already merged: not news");
    land_pull_request(&mut app, w1.clone(), Lookup::Found(merged(7)));
    assert!(!app.merge_is_fresh(&w1), "nor is hearing it again");

    land_pull_request(&mut app, w1.clone(), Lookup::Found(cached_pr(8)));
    assert!(!app.merge_is_fresh(&w1), "an open one is no merge");
    land_pull_request(&mut app, w1.clone(), Lookup::Found(merged(8)));
    assert!(app.merge_is_fresh(&w1), "open, then merged: seen to land");

    // Opened and landed between two beats of a checkout with no PR.
    app.merge_landed.clear();
    land_pull_request(&mut app, w1.clone(), Lookup::Absent);
    land_pull_request(&mut app, w1.clone(), Lookup::Found(merged(9)));
    assert!(app.merge_is_fresh(&w1), "no pull request, then merged");

    // A lookup that never reached GitHub says nothing either way.
    app.merge_landed.clear();
    land_pull_request(&mut app, w1.clone(), Lookup::Unavailable);
    assert!(!app.merge_is_fresh(&w1));
    assert!(app.worktree_wears_merge(&w1), "the row keeps its merge");
}

/// A turn that finishes unread sweeps blue for `ONE_SHOT_SWEEP` on its
/// card, so the sweep clock runs for those seconds and no longer. Reading it ends the sweep on the spot;
/// an old unread finish, an unstamped one and an archived one never ask
/// for a frame.
#[test]
fn a_fresh_unread_finish_keeps_the_sweep_ticking_for_a_few_seconds() {
    use nebula_core::AgentStatus;
    let mut app = App::new();
    seed_tree(&mut app);
    let now = crate::app::now_ms();
    let finish = |app: &mut App, at: i64| {
        let a = &mut app.tree.agents[0];
        a.status = AgentStatus::Finished;
        a.unseen = true;
        a.archived = false;
        a.status_changed_at = at;
    };
    let fresh = |app: &App| app.agent_fresh_done(&app.tree.agents[0]);

    finish(&mut app, now);
    assert!(fresh(&app), "the card sweeps");
    assert!(app.status_anim_active());
    app.animations = false;
    assert!(!app.status_anim_active(), "unless animations are off");
    app.animations = true;

    app.tree.agents[0].unseen = false;
    assert!(!fresh(&app), "read: over");
    assert!(!app.status_anim_active());

    finish(&mut app, now - settled().as_millis() as i64);
    assert!(!fresh(&app), "settled");
    assert!(!app.status_anim_active(), "an old unread finish is still");

    finish(&mut app, 0);
    assert!(!fresh(&app), "never stamped");

    finish(&mut app, now);
    app.tree.agents[0].archived = true;
    assert!(!fresh(&app), "archived: out of sight");
    assert!(!app.status_anim_active());

    // A DAEMON clock a little ahead of this one still sweeps — and one
    // an hour ahead does not sweep for an hour.
    finish(&mut app, now + 2_000);
    assert!(app.agent_fresh_done(&app.tree.agents[0]), "small skew");
    finish(&mut app, now + 3_600_000);
    assert!(!app.agent_fresh_done(&app.tree.agents[0]), "large skew");
}

/// A branch lookup that never reached GitHub leaves the row alone — the
/// one a previous launch cached included — while a definite "no pull
/// request" clears it and a found one paints it; only an answer that
/// changes the row marks the cache for writing.
#[test]
fn an_unavailable_lookup_keeps_the_cached_row() {
    let mut app = App::new();
    seed_tree(&mut app);
    let w1 = nebula_core::WorktreeId("w1".into());
    seed_branch_pr(&mut app, 7, "Attach links");
    app.pr_cache_dirty = false;

    app.pr_inflight.insert(w1.clone());
    land_pull_request(&mut app, w1.clone(), Lookup::Unavailable);
    assert!(!app.pr_inflight.contains(&w1));
    assert_eq!(
        app.pull_requests[&w1].as_ref().map(|p| p.number),
        Some(7),
        "the last answer stays on the row"
    );
    assert!(!app.pr_cache_dirty, "nothing changed, nothing to write");
    assert!(
        !app.pr_lookup_due(&w1),
        "but the next attempt is backed off"
    );

    land_pull_request(&mut app, w1.clone(), Lookup::Absent);
    assert_eq!(app.pull_requests[&w1], None, "a definite miss clears it");
    assert!(app.pr_cache_dirty);

    app.pr_cache_dirty = false;
    let found = cached_pr(8);
    land_pull_request(&mut app, w1.clone(), Lookup::Found(found.clone()));
    assert_eq!(app.pull_requests[&w1].as_ref(), Some(&found));
    assert!(app.pr_cache_dirty);
    app.pr_cache_dirty = false;
    land_pull_request(&mut app, w1, Lookup::Found(found));
    assert!(!app.pr_cache_dirty, "the same answer again is not a change");
}

/// A body hydrated from the cache is shown the moment the cursor rests
/// on its row — no "loading…" — and that same rest fetches a fresh copy
/// over it, the way it would fetch a missing one. A failed fetch leaves
/// the cached copy up; a landed one takes the body off the stale list
/// and marks the cache for writing.
#[test]
fn a_cached_body_is_shown_at_once_and_refreshed_underneath() {
    let mut app = App::new();
    seed_tree(&mut app);
    seed_open_prs(&mut app, &[(7, "Attach links")]);
    let url = pr_url(7);
    app.pr_detail
        .insert(url.clone(), a_detail(7, "from the cache", Vec::new()));
    app.pr_detail_stale.insert(url.clone());
    app.focus = Focus::Worktrees;
    app.sel_worktree = 1;
    schedule_pr_detail(&mut app);
    assert_eq!(
        app.pending_pr_detail.as_ref().map(|(p, _)| p.url.as_str()),
        Some(url.as_str()),
        "stale: re-asked"
    );
    let mut terminal = Terminal::new(TestBackend::new(140, 30)).unwrap();
    terminal.draw(|f| ui::draw(f, &mut app)).unwrap();
    let text = buffer_text(&terminal);
    assert!(text.contains("from the cache"), "{text}");
    assert!(!text.contains("loading…"), "{text}");

    let mut out = Vec::new();
    app.pending_pr_detail = None;
    app.pr_detail_inflight.insert(url.clone());
    land_pr_detail(&mut app, url.clone(), None, &mut out);
    assert_eq!(
        app.pr_detail.get(&url).map(|d| d.body.as_str()),
        Some("from the cache"),
        "a failed refresh keeps the cached copy"
    );

    app.pr_detail_failed.clear();
    app.pr_detail_inflight.insert(url.clone());
    app.pr_cache_dirty = false;
    land_pr_detail(
        &mut app,
        url.clone(),
        Some(a_detail(7, "rewritten", Vec::new())),
        &mut out,
    );
    assert_eq!(app.pr_detail[&url].body, "rewritten");
    assert!(!app.pr_detail_stale.contains(&url));
    assert!(app.pr_cache_dirty);
    schedule_pr_detail(&mut app);
    assert!(app.pending_pr_detail.is_none(), "fresh: nothing to ask");
}

/// The daemon's snapshot is the truth about which checkouts and
/// projects exist: cached rows for ones it no longer has go, before
/// they could be written back, and the bodies hanging off them with
/// them.
#[test]
fn the_snapshot_prunes_cached_rows_the_tree_no_longer_has() {
    let mut app = App::new();
    seed_tree(&mut app);
    let tree = app.tree.clone();

    let mut fresh = App::new();
    let w1 = nebula_core::WorktreeId("w1".into());
    let p1 = nebula_core::ProjectId("p1".into());
    let gone_w = nebula_core::WorktreeId("w-gone".into());
    let gone_p = nebula_core::ProjectId("p-gone".into());
    fresh.pull_requests.insert(w1.clone(), Some(cached_pr(7)));
    fresh
        .pull_requests
        .insert(gone_w.clone(), Some(cached_pr(8)));
    let now = std::time::Instant::now();
    fresh
        .pr_recheck
        .insert(gone_w.clone(), (now, PR_RECHECK_MIN));
    let open = |list| crate::app::OpenPrs {
        list,
        at: now,
        due: now,
        step: OPEN_PRS_REFRESH,
    };
    fresh
        .open_prs
        .insert(p1.clone(), open(vec![cached_open(9)]));
    fresh
        .open_prs
        .insert(gone_p.clone(), open(vec![cached_open(10)]));
    for number in [7, 8, 9, 10] {
        fresh
            .pr_detail
            .insert(pr_url(number), a_detail(number, "body", Vec::new()));
        fresh.pr_detail_stale.insert(pr_url(number));
    }

    hse(
        &mut fresh,
        ServerEvent::Snapshot {
            projects: tree.projects,
            worktrees: tree.worktrees,
            agents: tree.agents,
            terminals: tree.terminals,
            links: tree.links,
            pr_seen: Vec::new(),
            ui_state: None,
        },
    );
    assert!(fresh.pull_requests.contains_key(&w1));
    assert!(!fresh.pull_requests.contains_key(&gone_w));
    assert!(!fresh.pr_recheck.contains_key(&gone_w));
    assert!(fresh.open_prs.contains_key(&p1));
    assert!(!fresh.open_prs.contains_key(&gone_p));
    let mut kept: Vec<u64> = fresh.pr_detail.values().map(|d| d.number).collect();
    kept.sort();
    assert_eq!(kept, [7, 9], "bodies follow their rows");
    assert!(!fresh.pr_detail_stale.contains(&pr_url(8)));
    assert!(fresh.pr_cache_dirty);
}

/// The background pass over the projects the cursor is not on: the
/// first in row order that was never asked, or whose list is older
/// than the sweep's beat (or its own backoff, when longer), gets the
/// tick. The selected project never does — it has its own, faster beat
/// — nor does one already in flight.
#[test]
fn the_open_list_sweep_visits_the_other_projects_on_a_slow_beat() {
    let mut app = App::new();
    seed_tree(&mut app);
    let p1 = nebula_core::ProjectId("p1".into());
    let p2 = nebula_core::ProjectId("p2".into());
    hse(
        &mut app,
        ServerEvent::EntityUpserted {
            entity: nebula_core::Entity::Project(nebula_core::Project {
                id: p2.clone(),
                name: "other".into(),
                repo_path: "/tmp/other".into(),
                sort_order: 1,
            }),
        },
    );
    assert_eq!(
        app.selected_project().map(|p| p.id.clone()),
        Some(p1.clone())
    );
    let target = |app: &App| open_prs_sweep_target(app).map(|(id, _)| id);
    assert_eq!(target(&app), Some(p2.clone()), "never asked: due");

    let now = std::time::Instant::now();
    app.open_prs.insert(
        p2.clone(),
        crate::app::OpenPrs {
            list: Vec::new(),
            at: now,
            due: now,
            step: OPEN_PRS_RECHECK_MIN,
        },
    );
    assert_eq!(target(&app), None, "asked just now");

    let stale = now
        .checked_sub(OPEN_PRS_SWEEP_REFRESH + Duration::from_secs(1))
        .expect("machine up for minutes");
    app.open_prs.get_mut(&p2).unwrap().at = stale;
    assert_eq!(target(&app), Some(p2.clone()), "older than the beat");

    app.open_prs.get_mut(&p2).unwrap().step = OPEN_PRS_RECHECK_MAX;
    assert_eq!(target(&app), None, "its own, longer backoff holds it");
    app.open_prs.get_mut(&p2).unwrap().step = OPEN_PRS_RECHECK_MIN;

    app.open_prs_inflight.insert(p2.clone());
    assert_eq!(target(&app), None, "already in flight");
    app.open_prs_inflight.clear();

    app.open_prs.remove(&p1);
    assert_eq!(
        target(&app),
        Some(p2),
        "the selected project is never the sweep's, even unasked"
    );
    app.open_prs
        .get_mut(&nebula_core::ProjectId("p2".into()))
        .unwrap()
        .at = now;
    assert_eq!(target(&app), None);
}

/// `g` with a diff in the cache opens the modal on it at once; the
/// fresh fetch landing underneath replaces the contents in place when
/// they differ — the reader keeps the file they were on — and only
/// goes to the cache when the modal has been closed since. A failed
/// refresh leaves the cached copy up, and a request that had nothing
/// cached opens on landing, as it always did.
#[test]
fn a_cached_diff_opens_at_once_and_is_refreshed_in_place() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = App::new();
    app.pr_cache = Some(crate::pr_cache::PrCache::at(dir.path().to_path_buf()));
    seed_tree(&mut app);
    seed_open_prs(&mut app, &[(7, "Attach links")]);
    app.sel_worktree = 1;
    let url = pr_url(7);
    let title = || "#7 Attach links".to_string();
    let cached = "\
diff --git a/src/a.rs b/src/a.rs
--- a/src/a.rs
+++ b/src/a.rs
@@ -1 +1 @@
-old
+new
diff --git a/src/b.rs b/src/b.rs
--- a/src/b.rs
+++ b/src/b.rs
@@ -1 +1 @@
-x
+y
";
    let answer = |diff: Option<&str>| PrDiffAnswer {
        number: 7,
        url: pr_url(7),
        title: title(),
        diff: diff.map(str::to_string),
    };
    assert!(
        !open_cached_pr_diff(&mut app, 7, &url, &title()),
        "nothing cached yet"
    );
    app.pr_cache
        .as_ref()
        .unwrap()
        .store_diff(&url, cached)
        .unwrap();

    assert!(open_cached_pr_diff(&mut app, 7, &url, &title()));
    let Some(Overlay::Diff(view)) = &mut app.overlay else {
        panic!("expected the diff modal, got {:?}", app.overlay);
    };
    assert_eq!(view.pr_url.as_deref(), Some(url.as_str()));
    assert!(view.diff.contains("+new"));
    // The reader moves on to the second file.
    view.select(1);
    crate::git_diff::load_selected_diff(view);
    assert!(view.diff.contains("+y"));

    // The same diff lands: nothing moves, and the fetch is done.
    app.pr_diff_refreshing.insert(url.clone());
    app.pr_diff_inflight = Some(7);
    land_pr_diff(&mut app, answer(Some(cached)));
    assert!(app.pr_diff_inflight.is_none());
    assert!(app.pr_diff_refreshing.is_empty());
    assert!(!app.flash.as_deref().unwrap_or("").contains("changed"));
    let Some(Overlay::Diff(view)) = &app.overlay else {
        panic!("still open");
    };
    assert!(view.diff.contains("+y"), "still on b.rs");

    // A changed diff: b.rs stays under the cursor with its new text,
    // and a new file joins the list.
    let fresh = "\
diff --git a/src/a.rs b/src/a.rs
--- a/src/a.rs
+++ b/src/a.rs
@@ -1 +1 @@
-old
+newer
diff --git a/src/b.rs b/src/b.rs
--- a/src/b.rs
+++ b/src/b.rs
@@ -1 +1 @@
-x
+z
diff --git a/src/c.rs b/src/c.rs
--- /dev/null
+++ b/src/c.rs
@@ -0,0 +1 @@
+c
";
    app.pr_diff_refreshing.insert(url.clone());
    land_pr_diff(&mut app, answer(Some(fresh)));
    let Some(Overlay::Diff(view)) = &app.overlay else {
        panic!("still open");
    };
    assert_eq!(
        view.files
            .iter()
            .map(|f| f.path.as_str())
            .collect::<Vec<_>>(),
        ["src/a.rs", "src/b.rs", "src/c.rs"]
    );
    assert!(
        view.diff.contains("+z"),
        "the reader's file, refreshed: {}",
        view.diff
    );
    assert!(app.flash.as_deref().is_some_and(|f| f.contains("changed")));
    assert_eq!(
        crate::pr_cache::recall_diff(&app, &url).as_deref(),
        Some(fresh),
        "the fresh diff is what the next launch reads"
    );

    // Closed before the answer landed: cached, not reopened.
    app.overlay = None;
    app.pr_diff_refreshing.insert(url.clone());
    land_pr_diff(&mut app, answer(Some(cached)));
    assert!(app.overlay.is_none());
    assert_eq!(
        crate::pr_cache::recall_diff(&app, &url).as_deref(),
        Some(cached)
    );

    // A refresh `gh` couldn't do leaves the cached copy on screen.
    assert!(open_cached_pr_diff(&mut app, 7, &url, &title()));
    app.pr_diff_refreshing.insert(url.clone());
    land_pr_diff(&mut app, answer(None));
    assert!(matches!(&app.overlay, Some(Overlay::Diff(_))));

    // Nothing cached for #9: its diff opens on landing, as always.
    app.overlay = None;
    land_pr_diff(
        &mut app,
        PrDiffAnswer {
            number: 9,
            url: pr_url(9),
            title: "#9".into(),
            diff: Some(cached.into()),
        },
    );
    assert!(
        matches!(&app.overlay, Some(Overlay::Diff(v)) if v.pr_url.as_deref() == Some(pr_url(9).as_str()))
    );
    assert_eq!(
        crate::pr_cache::recall_diff(&app, &pr_url(9)).as_deref(),
        Some(cached),
        "and is cached for next time"
    );
}

/// `Shift+R` in the Worktrees panel asks GitHub again at once: the
/// project's open list is due despite the `OPEN_PRS_MIN_AGE` floor a
/// seconds-old answer keeps for focus events, the worktree's own PR is
/// due despite its beat, the loop is flagged to fire both on its next
/// turn rather than the git tick, and the flash says so. No prompt
/// opens, and no daemon traffic: `gh` runs client-side. Plain `r` —
/// the rename key — does nothing here, where nothing is renameable.
#[test]
fn shift_r_on_the_worktrees_panel_refreshes_pull_requests_now() {
    let mut app = App::new();
    seed_tree(&mut app);
    let pid = app.selected_project().expect("a project").id.clone();
    let wid = app.selected_worktree().expect("a worktree").id.clone();
    note_open_prs_answer(&mut app, pid.clone(), Some(vec![]), &mut Vec::new());
    app.focus = Focus::Worktrees;
    schedule_pull_request_refresh(&mut app);
    assert!(
        !app.open_prs_lookup_due(&pid),
        "seconds old: a focus event is held off by the floor"
    );
    // The worktree's own PR has no floor, so a focus event does re-ask
    // it; settle it onto its beat so the key has a timer to skip.
    note_pr_answer(&mut app, &wid, true);
    assert!(!app.pr_lookup_due(&wid), "the beat hasn't come round");

    let mut out = Vec::new();
    press(&mut app, KeyCode::Char('r'), KeyModifiers::NONE, &mut out);
    assert!(
        !app.open_prs_lookup_due(&pid) && !app.pr_refresh_requested,
        "plain r runs the checkout, and refreshes nothing"
    );
    assert!(
        out.iter()
            .all(|r| matches!(r, ClientRequest::StartRun { .. })),
        "{out:?}"
    );
    assert!(app.overlay.is_none() && app.flash.is_none());
    out.clear();

    // Another checkout of the project, resting on the sweep's beat.
    let other = add_worktree(&mut app, "w2", "/tmp/demo-w2");
    note_pr_answer(&mut app, &other, true);
    assert!(!app.pr_lookup_due(&other), "swept minutes from now");

    press(&mut app, KeyCode::Char('R'), KeyModifiers::SHIFT, &mut out);
    assert!(app.open_prs_lookup_due(&pid), "the key skips the floor");
    assert!(app.pr_lookup_due(&wid), "and the beat");
    assert!(
        app.pr_lookup_due(&other),
        "and every other checkout's sweep timer"
    );
    assert!(
        app.pr_refresh_requested,
        "fired on the loop's next turn, not the next git tick"
    );
    assert!(app.overlay.is_none(), "nothing to rename here");
    assert_eq!(app.flash.as_deref(), Some(RELOAD_FLASH));
    assert!(out.is_empty(), "no daemon traffic — gh runs client-side");

    // The same from an open-PR row of the group, which the pane is
    // reading: its conversation is asked for again too.
    seed_open_prs(&mut app, &[(7, "Attach links")]);
    app.sel_worktree = app.visible_worktrees().len();
    app.pr_detail.insert(pr_url(7), a_detail(7, "body", vec![]));
    app.pr_refresh_requested = false;
    press(&mut app, KeyCode::Char('R'), KeyModifiers::SHIFT, &mut out);
    assert!(app.pr_refresh_requested);
    assert_eq!(
        app.pending_pr_detail.as_ref().map(|(p, _)| p.url.as_str()),
        Some(pr_url(7).as_str()),
        "the cached conversation is re-asked"
    );
    assert!(app.overlay.is_none());
}

/// `Shift+R` on the Sessions panel's PR ROW re-asks GitHub: the row's
/// own lookup, the project's list, and the conversation the pane is
/// reading, fetched again over the cached copy (a refused one is asked
/// again too) without rewinding the reader's scroll. It is not tied to
/// the row: from an agent row it refreshes just the same, while `r`
/// there keeps its rename.
#[test]
fn shift_r_on_the_pr_row_refreshes_pull_requests() {
    let mut app = App::new();
    seed_tree(&mut app);
    let url = pr_url(7);
    let wid = app.selected_worktree().expect("a worktree").id.clone();
    app.pull_requests.insert(
        wid.clone(),
        Some(crate::pull_request::PullRequest {
            number: 7,
            url: url.clone(),
            title: "Attach links".into(),
            state: crate::pull_request::STATE_OPEN.into(),
            is_draft: false,
            health: Default::default(),
            activity: Vec::new(),
        }),
    );
    note_pr_answer(&mut app, &wid, true);
    app.focus = Focus::Sessions;
    app.sel_session = app
        .visible_session_rows()
        .iter()
        .position(|r| r.as_link().is_some())
        .expect("pull-request row");
    // Read once, refused once since, and the reader is partway down.
    app.pr_detail
        .insert(url.clone(), a_detail(7, "body", vec![]));
    app.pr_detail_failed.insert(url.clone());
    app.pending_pr_detail = None;
    app.pr_preview_scroll = 12;
    assert!(!app.pr_lookup_due(&wid));

    let mut out = Vec::new();
    press(&mut app, KeyCode::Char('R'), KeyModifiers::SHIFT, &mut out);
    assert!(app.overlay.is_none(), "no prompt");
    assert_eq!(app.flash.as_deref(), Some(RELOAD_FLASH));
    assert!(app.pr_lookup_due(&wid), "the row's own lookup is due");
    assert!(app.pr_refresh_requested);
    let (pending, at) = app
        .pending_pr_detail
        .as_ref()
        .expect("the conversation is fetched again");
    assert_eq!(pending.url, url);
    assert_eq!(pending.number, 7);
    assert!(*at <= std::time::Instant::now(), "no hover debounce");
    assert!(
        app.pr_detail.contains_key(&url),
        "the cached copy stays on screen until the new one lands"
    );
    assert!(
        !app.pr_detail_failed.contains(&url),
        "a refusal is not the last word"
    );
    assert_eq!(app.pr_preview_scroll, 12, "the reader is not rewound");

    // One already in flight is left to land: nothing is stacked on it.
    app.pending_pr_detail = None;
    app.pr_detail_inflight.insert(url.clone());
    press(&mut app, KeyCode::Char('R'), KeyModifiers::SHIFT, &mut out);
    assert!(app.pending_pr_detail.is_none());

    // From an agent row the key refreshes all the same…
    app.sel_session = 0;
    app.pr_refresh_requested = false;
    app.flash = None;
    press(&mut app, KeyCode::Char('R'), KeyModifiers::SHIFT, &mut out);
    assert!(app.pr_refresh_requested && app.overlay.is_none());
    assert_eq!(app.flash.as_deref(), Some(RELOAD_FLASH));

    // …while `r` there still renames.
    press(&mut app, KeyCode::Char('r'), KeyModifiers::NONE, &mut out);
    match &app.overlay {
        Some(Overlay::Prompt(p)) => assert!(
            matches!(p.kind, PromptKind::RenameAgent { .. }),
            "got {:?}",
            p.kind
        ),
        other => panic!("expected the rename prompt, got {other:?}"),
    }
}

/// Shift+D wipes the panel's sessions; links are bookmarks and survive.
#[test]
fn delete_all_sessions_leaves_links_alone() {
    let mut app = App::new();
    seed_tree(&mut app);
    seed_link(&mut app, "https://example.dev/spec");
    let mut out = Vec::new();
    press(&mut app, KeyCode::Char('D'), KeyModifiers::NONE, &mut out);
    let Some(Overlay::Confirm(c)) = &app.overlay else {
        panic!("expected the bulk confirm, got {:?}", app.overlay);
    };
    assert!(
        !c.message.contains("example.dev"),
        "links are not up for deletion: {}",
        c.message
    );
    let PendingAction::DeleteAllSessions { agents, terminals } = &c.action else {
        panic!("wrong action: {:?}", c.action);
    };
    assert_eq!(agents.len(), 1);
    assert!(terminals.is_empty());
}
/// The always-live search fields edit the same way — and ⌥←/⌥→ move the
/// caret rather than typing a literal "b"/"f" into the query.
#[test]
fn palette_query_edits_like_a_line_and_refilters() {
    let mut app = App::new();
    seed_tree(&mut app);
    let mut out = Vec::new();
    press(&mut app, KeyCode::Char('/'), KeyModifiers::NONE, &mut out);
    for c in "demo".chars() {
        press(&mut app, KeyCode::Char(c), KeyModifiers::NONE, &mut out);
    }
    press(&mut app, KeyCode::Char('b'), KeyModifiers::ALT, &mut out);
    press(&mut app, KeyCode::Char('x'), KeyModifiers::NONE, &mut out);
    let matched = |app: &App| match &app.overlay {
        Some(Overlay::Palette(p)) => p.matches.len(),
        other => panic!("expected palette, got {other:?}"),
    };
    let Some(Overlay::Palette(p)) = &app.overlay else {
        panic!("palette closed")
    };
    assert_eq!(p.query.as_str(), "xdemo", "⌥← moves, it does not type 'b'");
    assert_eq!(matched(&app), 0, "the edit re-ran the filter");

    // Ctrl+W kills the word back to an empty query, which matches all.
    press(
        &mut app,
        KeyCode::Char('e'),
        KeyModifiers::CONTROL,
        &mut out,
    );
    press(
        &mut app,
        KeyCode::Char('w'),
        KeyModifiers::CONTROL,
        &mut out,
    );
    let Some(Overlay::Palette(p)) = &app.overlay else {
        panic!("palette closed")
    };
    assert_eq!(p.query.as_str(), "");
    assert!(matched(&app) > 0, "clearing the query restores every row");
}

/// Resting the worktree selection arms the debounced prewarm; firing it
/// sends one PrewarmWorktreeSessions plus the standing default-spec
/// Claude keep-warm for that worktree, then disarms.
#[test]
fn worktree_move_arms_prewarm_and_fire_sends_request() {
    use nebula_core::{Entity, ProjectId, Worktree, WorktreeId};
    let mut app = App::new();
    seed_tree(&mut app);
    hse(
        &mut app,
        ServerEvent::EntityUpserted {
            entity: Entity::Worktree(Worktree {
                id: WorktreeId("w2".into()),
                project_id: ProjectId("p1".into()),
                path: "/tmp/demo-w2".into(),
                branch: "feature".into(),
                is_main: false,
                sort_order: 1,
            }),
        },
    );
    app.pending_prewarm = None;
    app.focus = Focus::Worktrees;
    let mut out = Vec::new();
    move_selection(&mut app, 1, &mut out);
    let (armed, _) = app.pending_prewarm.clone().expect("prewarm armed");
    assert_eq!(armed, WorktreeId("w2".into()));

    out.clear();
    with_default_config(|| fire_pending_prewarm(&mut app, &mut out));
    assert!(app.pending_prewarm.is_none(), "fires once, then disarms");
    assert!(matches!(
        out.as_slice(),
        [
            ClientRequest::PrewarmWorktreeSessions { worktree, .. },
            ClientRequest::PrewarmAgent {
                worktree: agent_wt,
                kind: AgentKind::Claude,
                model: None,
                effort: None,
            },
        ] if worktree == &WorktreeId("w2".into()) && agent_wt == &WorktreeId("w2".into())
    ));
    assert!(app.next_keepwarm.is_some(), "keep-warm re-send is armed");
}

/// An empty `gh` answer doesn't retire the worktree: the next attempt
/// is armed one backoff step out, growing to the cap so a checkout that
/// never grows a PR stops costing a process every few seconds.
#[test]
fn empty_pr_answers_back_off_instead_of_settling() {
    use nebula_core::WorktreeId;
    let mut app = App::new();
    let wt = WorktreeId("w1".into());
    assert!(app.pr_lookup_due(&wt), "never asked: due immediately");

    note_pr_answer(&mut app, &wt, false);
    let (_, first) = *app.pr_recheck.get(&wt).expect("backoff armed");
    assert_eq!(first, PR_RECHECK_MIN);
    assert!(!app.pr_lookup_due(&wt), "not due until the backoff expires");

    note_pr_answer(&mut app, &wt, false);
    let (_, second) = *app.pr_recheck.get(&wt).expect("backoff armed");
    assert_eq!(second, PR_RECHECK_MIN * 2, "each miss doubles the gap");

    for _ in 0..12 {
        note_pr_answer(&mut app, &wt, false);
    }
    let (_, capped) = *app.pr_recheck.get(&wt).expect("backoff armed");
    assert_eq!(capped, PR_RECHECK_MAX, "growth stops at the cap");
}

/// A due backoff makes the worktree askable again — this is what lets a
/// PR opened by a session after the first lookup still land on the row.
#[test]
fn an_expired_backoff_asks_again() {
    use nebula_core::WorktreeId;
    let mut app = App::new();
    let wt = WorktreeId("w1".into());
    app.pull_requests.insert(wt.clone(), None);
    app.pr_recheck.insert(
        wt.clone(),
        (
            std::time::Instant::now() - Duration::from_secs(1),
            PR_RECHECK_MIN,
        ),
    );
    assert!(app.pr_lookup_due(&wt), "a cached miss is not the last word");
}

/// Finding the PR settles the worktree onto a steady beat rather than
/// retiring it: the PR won't change, but its conversation will, and the
/// unread-comment badge is only as fresh as the last poll. The beat
/// depends on where the cursor is: the selected checkout keeps up with
/// its conversation, any other only with its merge.
#[test]
fn a_found_pr_keeps_being_refreshed() {
    use nebula_core::WorktreeId;
    let mut app = App::new();
    seed_tree(&mut app);
    let wt = WorktreeId("w1".into());
    assert_eq!(
        app.selected_worktree().map(|w| w.id.clone()),
        Some(wt.clone())
    );
    note_pr_answer(&mut app, &wt, false);
    note_pr_answer(&mut app, &wt, true);
    let (_, step) = *app.pr_recheck.get(&wt).expect("still scheduled");
    assert_eq!(step, PR_REFRESH, "the miss backoff gives way to the beat");

    // A checkout the cursor is not on settles onto the sweep's slower
    // beat instead: nobody is reading its badge.
    let other = WorktreeId("w2".into());
    note_pr_answer(&mut app, &other, true);
    let (_, step) = *app.pr_recheck.get(&other).expect("scheduled");
    assert_eq!(step, PR_SWEEP_REFRESH, "an unselected checkout is swept");
    assert!(
        PR_SWEEP_REFRESH > PR_RECHECK_MAX,
        "slower than any miss backoff"
    );

    app.pull_requests.insert(
        wt.clone(),
        Some(crate::pull_request::PullRequest {
            number: 7,
            url: "https://github.com/o/r/pull/7".into(),
            title: "done".into(),
            state: crate::pull_request::STATE_OPEN.into(),
            is_draft: false,
            health: Default::default(),
            activity: Vec::new(),
        }),
    );
    assert!(!app.pr_lookup_due(&wt), "not before the beat comes round");

    // Switching into the checkout is a reason to ask right now — that's
    // when the user wants to know whether anyone has commented.
    seed_tree(&mut app);
    schedule_pr_lookup(&mut app);
    assert!(app.pr_lookup_due(&wt), "arriving re-asks immediately");
}

/// Opening a pull request row banks everything nebula knows about its
/// conversation, so the badge clears on the spot and the daemon is told
/// to remember it. What lands afterwards is what counts as new.
#[test]
fn opening_a_pull_request_marks_it_read() {
    use nebula_core::WorktreeId;
    let mut app = App::new();
    let url = "https://github.com/o/r/pull/7";
    let wt = WorktreeId("w1".into());
    app.pull_requests.insert(
        wt.clone(),
        Some(crate::pull_request::PullRequest {
            number: 7,
            url: url.into(),
            title: "done".into(),
            state: crate::pull_request::STATE_OPEN.into(),
            is_draft: false,
            health: Default::default(),
            activity: vec!["2024-04-25T19:55:42Z".into()],
        }),
    );
    let mut out = Vec::new();
    mark_pr_seen(&mut app, url, &mut out);
    assert_eq!(
        app.pr_seen.get(url).map(String::as_str),
        Some("2024-04-25T19:55:42Z"),
        "applied locally so the badge clears this frame"
    );
    assert!(matches!(
        out.as_slice(),
        [ClientRequest::MarkPrSeen { url: u, marker: m }]
            if u == url && m == "2024-04-25T19:55:42Z"
    ));

    // Opening it again with nothing new says nothing to the daemon.
    out.clear();
    mark_pr_seen(&mut app, url, &mut out);
    assert!(out.is_empty(), "an unmoved mark is not worth a round trip");

    // A URL that isn't a pull request has no conversation to bank.
    mark_pr_seen(&mut app, "https://example.dev/spec", &mut out);
    assert!(out.is_empty());
    assert_eq!(app.pr_seen.len(), 1);
}

/// A lookup in flight blocks a second one, so the 2s git tick can't
/// stack `gh` processes on a slow network.
#[test]
fn an_inflight_lookup_blocks_a_second_one() {
    use nebula_core::WorktreeId;
    let mut app = App::new();
    let wt = WorktreeId("w1".into());
    app.pr_inflight.insert(wt.clone());
    assert!(!app.pr_lookup_due(&wt));
    app.pr_inflight.remove(&wt);
    assert!(app.pr_lookup_due(&wt));
}

/// Switching into a worktree drops its accumulated backoff, so arriving
/// somewhere asks `gh` again on the next tick rather than up to three
/// minutes later.
#[test]
fn a_worktree_switch_clears_the_backoff() {
    use nebula_core::{Entity, ProjectId, Worktree, WorktreeId};
    let mut app = App::new();
    seed_tree(&mut app);
    hse(
        &mut app,
        ServerEvent::EntityUpserted {
            entity: Entity::Worktree(Worktree {
                id: WorktreeId("w2".into()),
                project_id: ProjectId("p1".into()),
                path: "/tmp/demo-w2".into(),
                branch: "feature".into(),
                is_main: false,
                sort_order: 1,
            }),
        },
    );
    let w2 = WorktreeId("w2".into());
    app.pull_requests.insert(w2.clone(), None);
    app.pr_recheck.insert(
        w2.clone(),
        (std::time::Instant::now() + PR_RECHECK_MAX, PR_RECHECK_MAX),
    );
    assert!(!app.pr_lookup_due(&w2), "backed off before the switch");

    app.focus = Focus::Worktrees;
    let mut out = Vec::new();
    move_selection(&mut app, 1, &mut out);
    assert_eq!(
        app.selected_worktree().map(|w| w.id.clone()),
        Some(w2.clone())
    );
    assert!(app.pr_lookup_due(&w2), "the switch re-arms the lookup");
}

/// Add a non-root checkout to the seeded project, as the daemon's
/// upsert would.
fn add_worktree(app: &mut App, id: &str, path: &str) -> nebula_core::WorktreeId {
    use nebula_core::{Entity, ProjectId, Worktree, WorktreeId};
    let wid = WorktreeId(id.into());
    hse(
        app,
        ServerEvent::EntityUpserted {
            entity: Entity::Worktree(Worktree {
                id: wid.clone(),
                project_id: ProjectId("p1".into()),
                path: path.into(),
                branch: id.into(),
                is_main: false,
                sort_order: 1,
            }),
        },
    );
    wid
}

/// The sweep hands out one checkout per tick: the first unselected,
/// non-root worktree in row order that is due and on disk. A checkout
/// with a lookup in flight or a timer still running is passed over;
/// one that isn't on disk is noted as a miss on the way past without
/// taking the tick; the ROOT WORKTREE and the selected checkout are
/// never the sweep's — the root has no merge worth a call, and the
/// selected one has its own lookup.
#[test]
fn the_sweep_takes_the_projects_other_checkouts_one_per_tick() {
    let mut app = App::new();
    seed_tree(&mut app);
    let root = app.selected_worktree().expect("the root").id.clone();
    assert!(app.selected_worktree().unwrap().is_main);
    let on_disk = tempfile::tempdir().unwrap();
    let also_on_disk = tempfile::tempdir().unwrap();
    let w2 = add_worktree(&mut app, "w2", on_disk.path().to_str().unwrap());
    let w3 = add_worktree(&mut app, "w3", also_on_disk.path().to_str().unwrap());
    let gone = add_worktree(&mut app, "w4", "/tmp/nebula-sweep-no-such-checkout");
    // The GRID names a pull request under every session it lists, so
    // the sweep covers the checkouts its cards run in: give each one
    // a session to be listed by.
    for (i, w) in [&w2, &w3, &gone].into_iter().enumerate() {
        seed_agent_in(&mut app, &format!("sweep-a{i}"), w);
    }
    let order: Vec<_> = app
        .visible_worktrees()
        .iter()
        .map(|w| w.id.clone())
        .collect();
    assert_eq!(order[0], root, "root first");
    let rest: Vec<_> = order[1..]
        .iter()
        .filter(|id| **id != gone)
        .cloned()
        .collect();

    // First tick: the first on-disk checkout in row order.
    let (first, path) = sweep_target(&mut app).expect("a checkout to sweep");
    assert_eq!(first, rest[0]);
    assert!(path.is_dir());
    // Still the same one until its answer is in — the loop marks it in
    // flight before spawning; here that is done by hand.
    assert_eq!(
        sweep_target(&mut app).map(|(id, _)| id),
        Some(first.clone())
    );
    app.pr_inflight.insert(first.clone());
    // Next tick: the other one. Then nothing until a timer expires.
    let (second, _) = sweep_target(&mut app).expect("the other checkout");
    assert_eq!(second, rest[1]);
    assert_ne!(second, first);
    app.pr_inflight.remove(&second);
    note_pr_answer(&mut app, &second, true);
    assert!(app.pr_lookup_due(&gone), "not yet reached");
    assert_eq!(
        sweep_target(&mut app),
        None,
        "everything is in flight or on its beat"
    );
    assert!(
        !app.pr_lookup_due(&gone),
        "the missing checkout was noted as a miss on the way past"
    );
    assert_eq!(app.pull_requests.get(&gone), Some(&None));

    // With a non-root checkout selected, that one is left to its own
    // lookup and the root is still not swept.
    app.pr_inflight.clear();
    app.pr_recheck.clear();
    app.focus = Focus::Worktrees;
    app.sel_worktree = order.iter().position(|id| *id == w2).unwrap();
    assert_eq!(
        app.selected_worktree().map(|w| w.id.clone()),
        Some(w2.clone())
    );
    assert_eq!(
        sweep_target(&mut app).map(|(id, _)| id),
        Some(w3.clone()),
        "not the selected checkout, not the root"
    );
    app.pr_inflight.insert(w3);
    assert_eq!(
        sweep_target(&mut app),
        None,
        "the root is never the sweep's"
    );
}

/// A pull request that drops out of the project's open list has merged
/// or closed; the checkout on that branch is re-asked on the next tick
/// rather than when its sweep timer comes round, so the row turns
/// purple within seconds of the merge. A failed list keeps its rows
/// and retires nothing; a checkout whose PR is already known merged
/// has nothing to learn.
#[test]
fn a_pull_request_that_leaves_the_open_list_re_asks_its_checkout() {
    let mut app = App::new();
    seed_tree(&mut app);
    let pid = app.selected_project().expect("a project").id.clone();
    let wid = nebula_core::WorktreeId("w1".into());
    seed_branch_pr(&mut app, 7, "Attach links");
    seed_open_prs(&mut app, &[(7, "Attach links"), (9, "Number lines")]);
    let resting = || {
        (
            std::time::Instant::now() + PR_SWEEP_REFRESH,
            PR_SWEEP_REFRESH,
        )
    };
    app.pr_recheck.insert(wid.clone(), resting());
    assert!(!app.pr_lookup_due(&wid), "on the sweep's beat");

    // #9 is still open: nothing about #7's checkout changes.
    let seven = app.open_prs[&pid].list[0].clone();
    let nine = app.open_prs[&pid].list[1].clone();
    assert_eq!((seven.number, nine.number), (7, 9));
    note_open_prs_answer(
        &mut app,
        pid.clone(),
        Some(vec![nine.clone(), seven]),
        &mut Vec::new(),
    );
    assert!(!app.pr_lookup_due(&wid), "still listed: still on its beat");

    // #7 leaves the list: its checkout is due now.
    note_open_prs_answer(
        &mut app,
        pid.clone(),
        Some(vec![nine.clone()]),
        &mut Vec::new(),
    );
    assert!(
        app.pr_lookup_due(&wid),
        "its pull request just left the open list"
    );

    // A call that couldn't be made keeps the old rows and re-asks nothing.
    seed_open_prs(&mut app, &[(7, "Attach links")]);
    app.pr_recheck.insert(wid.clone(), resting());
    note_open_prs_answer(&mut app, pid.clone(), None, &mut Vec::new());
    assert!(!app.pr_lookup_due(&wid), "no answer, no retirement");

    // Known merged already: leaving the list says nothing new.
    let mut merged = a_detail(7, "shipped", vec![]);
    merged.state = "MERGED".into();
    adopt_pr_state(&mut app, &merged);
    note_open_prs_answer(&mut app, pid, Some(vec![]), &mut Vec::new());
    assert!(!app.pr_lookup_due(&wid), "already wearing the merge");
}

/// The keep-warm tick re-sends the default-spec Claude prewarm for the
/// selected worktree and re-arms itself; with nothing selected it
/// disarms until the next worktree rest re-arms it.
#[test]
fn keepwarm_refires_for_selected_worktree_and_rearms() {
    with_default_config(|| {
        let mut app = App::new();
        seed_tree(&mut app);
        app.next_keepwarm = Some(std::time::Instant::now());
        let mut out = Vec::new();
        fire_keepwarm(&mut app, &mut out);
        assert!(matches!(
            out.as_slice(),
            [ClientRequest::PrewarmAgent {
                worktree,
                kind: AgentKind::Claude,
                model: None,
                effort: None,
            }] if worktree == &nebula_core::WorktreeId("w1".into())
        ));
        assert!(app.next_keepwarm.is_some(), "re-arms after sending");

        let mut empty = App::new();
        empty.next_keepwarm = Some(std::time::Instant::now());
        out.clear();
        fire_keepwarm(&mut empty, &mut out);
        assert!(out.is_empty(), "nothing selected, nothing to keep warm");
        assert!(empty.next_keepwarm.is_none(), "disarms without a worktree");
    })
}

/// Esc on the NEW SESSION PICKER sends nothing: opening it warmed
/// nothing, and only Enter on a row creates.
#[test]
fn esc_on_the_new_session_picker_sends_nothing() {
    with_default_config(|| {
        let mut app = App::new();
        seed_tree(&mut app);
        app.focus = Focus::Sessions;
        let mut out = Vec::new();
        open_picker(&mut app);
        assert!(matches!(&app.overlay, Some(Overlay::Menu(_))));
        press(&mut app, KeyCode::Esc, KeyModifiers::NONE, &mut out);
        assert!(app.overlay.is_none());
        assert!(
            out.is_empty(),
            "nothing to create, nothing to warm: {out:?}"
        );
    })
}

/// The startup snapshot arms the prewarm for the restored worktree, so
/// its sessions boot before the user presses anything.
#[test]
fn snapshot_arms_prewarm_for_selected_worktree() {
    let mut app = App::new();
    seed_tree(&mut app);
    let tree = app.tree.clone();
    let mut fresh = App::new();
    assert!(fresh.pending_prewarm.is_none());
    hse(
        &mut fresh,
        ServerEvent::Snapshot {
            projects: tree.projects,
            worktrees: tree.worktrees,
            agents: tree.agents,
            terminals: tree.terminals,
            links: tree.links,
            pr_seen: Vec::new(),
            ui_state: None,
        },
    );
    let (armed, _) = fresh.pending_prewarm.clone().expect("prewarm armed");
    assert_eq!(armed, nebula_core::WorktreeId("w1".into()));
}

/// The startup snapshot puts the cursor back on the session the user
/// left on — and its terminal back in the pane. A restored selection
/// over a blank pane reads as "nebula forgot", even though the row is
/// highlighted.
#[test]
fn snapshot_reattaches_the_remembered_session() {
    let mut app = App::new();
    seed_tree(&mut app); // p1 / w1 / a1
    let tree = app.tree.clone();
    let snapshot = |ui_state: Option<String>| ServerEvent::Snapshot {
        projects: tree.projects.clone(),
        worktrees: tree.worktrees.clone(),
        agents: tree.agents.clone(),
        terminals: tree.terminals.clone(),
        links: tree.links.clone(),
        pr_seen: Vec::new(),
        ui_state,
    };
    let a1 = SessionRef::Agent(nebula_core::AgentId("a1".into()));

    // Remembered session present: the pane comes back with the cursor.
    let mut fresh = App::new();
    let mut out = Vec::new();
    handle_server_event(
            &mut fresh,
            snapshot(Some(
                r#"{"project":"p1","worktree":"w1","session_agent":"a1","show_archived":false,"collapsed":false}"#
                    .into(),
            )),
            &mut out,
        );
    assert_eq!(
        fresh.term.as_ref().map(|t| t.sref.clone()),
        Some(a1.clone())
    );
    assert!(
        out.iter()
            .any(|r| matches!(r, ClientRequest::Attach { session, .. } if *session == a1)),
        "expected an Attach for a1, got {out:?}"
    );
    // The cursor stays on the grid: this is a preview, not Enter.
    assert_eq!(fresh.focus, Focus::Sessions);
    assert!(!fresh.term_locked);

    // No blob (first launch) or a blob whose session is gone: nothing
    // to bring back, so the pane stays blank rather than guessing.
    for blob in [
            None,
            Some(
                r#"{"project":"p1","worktree":"w1","session_agent":"gone","show_archived":false,"collapsed":false}"#
                    .to_string(),
            ),
        ] {
            let mut fresh = App::new();
            let mut out = Vec::new();
            handle_server_event(&mut fresh, snapshot(blob), &mut out);
            // The GRID's own cursor fills the pane when the blob named no
            // session to come back to, so what must not happen is an
            // attach for the session that is gone.
            assert!(!out.iter().any(|r| matches!(
                r,
                ClientRequest::Attach { session, .. } if *session == SessionRef::Agent(AgentId("gone".into()))
            )));
        }
}

/// The footer's far left is a nameplate: which nebula this is, ahead
/// of every cursor-driven crumb after it. It yields
/// the columns back to a flash that would otherwise be cut off mid
/// sentence — a clipped key list is still readable, a clipped message
/// is not.
#[test]
fn footer_shows_the_nebula_version_but_never_truncates_a_flash() {
    let stamp = concat!("nebula v", env!("CARGO_PKG_VERSION"));
    let mut app = App::new();
    seed_tree(&mut app);
    let mut terminal = Terminal::new(TestBackend::new(120, 30)).unwrap();
    terminal.draw(|f| ui::draw(f, &mut app)).unwrap();
    let text = buffer_text(&terminal);
    assert!(text.contains(stamp), "{stamp} missing from:\n{text}");

    // A flash short enough to share the bar keeps the nameplate.
    app.flash = Some("saved".into());
    terminal.draw(|f| ui::draw(f, &mut app)).unwrap();
    let text = buffer_text(&terminal);
    assert!(text.contains(stamp), "{stamp} missing from:\n{text}");
    assert!(text.contains("saved"), "{text}");

    // One that isn't takes the whole left edge instead.
    let long = "the pull request link can't be deleted from here, close it on github";
    app.flash = Some(long.into());
    terminal.draw(|f| ui::draw(f, &mut app)).unwrap();
    let text = buffer_text(&terminal);
    assert!(
        !text.contains(stamp),
        "nameplate should have yielded:\n{text}"
    );
}

/// A newer published release rides the nameplate as `⇡ vX.Y.Z` and
/// yields with it — the flash rule is the nameplate's, not a second
/// one.
#[test]
fn footer_flags_a_newer_release_beside_the_nameplate() {
    let stamp = concat!("nebula v", env!("CARGO_PKG_VERSION"));
    let mut app = App::new();
    seed_tree(&mut app);
    let mut terminal = Terminal::new(TestBackend::new(120, 30)).unwrap();
    terminal.draw(|f| ui::draw(f, &mut app)).unwrap();
    assert!(!buffer_text(&terminal).contains('⇡'), "nothing to flag yet");

    app.update_available = Some("9.9.9".into());
    terminal.draw(|f| ui::draw(f, &mut app)).unwrap();
    let text = buffer_text(&terminal);
    let flagged = format!("{stamp} ⇡ v9.9.9");
    assert!(text.contains(&flagged), "{flagged} missing from:\n{text}");
    assert!(!text.contains("◇ "), "no workspace on the footer: {text}");

    // A flash that would be clipped drops the whole plate, indicator too.
    let long = "the pull request link can't be deleted from here, close it on github";
    app.flash = Some(long.into());
    terminal.draw(|f| ui::draw(f, &mut app)).unwrap();
    let text = buffer_text(&terminal);
    assert!(
        !text.contains(stamp) && !text.contains("⇡ v9.9.9"),
        "plate and indicator should have yielded:\n{text}"
    );
}

/// The footer's right edge shows live session counts and nebula's
/// total memory once a metrics reading arrives.
#[test]
fn footer_shows_session_counts_and_memory() {
    use nebula_core::{MetricsSnapshot, SessionMetrics, TerminalId};
    let mut app = App::new();
    seed_tree(&mut app);
    let mut terminal = Terminal::new(TestBackend::new(120, 30)).unwrap();
    terminal.draw(|f| ui::draw(f, &mut app)).unwrap();
    assert!(
        !buffer_text(&terminal).contains("agent ·"),
        "no readout before the first reading"
    );

    app.client_rss_bytes = 100 * 1024 * 1024;
    app.last_metrics = Some(MetricsSnapshot {
        daemon_pid: 1,
        daemon_rss_bytes: 200 * 1024 * 1024,
        system_total_bytes: 0,
        sessions: vec![
            SessionMetrics {
                session: SessionRef::Agent(AgentId("a1".into())),
                pid: 10,
                rss_bytes: 700 * 1024 * 1024,
                procs: 3,
                prewarm: None,
            },
            SessionMetrics {
                session: SessionRef::Terminal(TerminalId("t1".into())),
                pid: 11,
                rss_bytes: 24 * 1024 * 1024,
                procs: 2,
                prewarm: None,
            },
        ],
    });
    terminal.draw(|f| ui::draw(f, &mut app)).unwrap();
    let text = buffer_text(&terminal);
    assert!(
        text.contains("1 agent · 1 term · 1.0 GB"),
        "footer readout rendered:\n{text}"
    );
}

/// INPUT PARITY: the footer's readout is a button. Its target is the
/// words alone, the pointer on it underlines them, and a click opens
/// the memory modal — the one `⇧M` opens, through the same
/// `open_metrics`, reading requested and all.
#[test]
fn clicking_the_footer_readout_opens_the_memory_modal() {
    use crossterm::event::Event;
    use nebula_core::{MetricsSnapshot, SessionMetrics};
    use ratatui::style::Modifier;
    let mut app = App::new();
    seed_tree(&mut app);
    app.client_rss_bytes = 100 * 1024 * 1024;
    app.last_metrics = Some(MetricsSnapshot {
        daemon_pid: 1,
        daemon_rss_bytes: 200 * 1024 * 1024,
        system_total_bytes: 0,
        sessions: vec![SessionMetrics {
            session: SessionRef::Agent(AgentId("a1".into())),
            pid: 10,
            rss_bytes: 724 * 1024 * 1024,
            procs: 3,
            prewarm: None,
        }],
    });
    let mut terminal = Terminal::new(TestBackend::new(120, 30)).unwrap();
    terminal.draw(|f| ui::draw(f, &mut app)).unwrap();
    let rect = app
        .hit_rect(&HitTarget::FooterUsage)
        .expect("the readout was drawn");
    let row = |terminal: &Terminal<TestBackend>, underlined: bool| -> String {
        let buf = terminal.backend().buffer();
        (0..buf.area.width)
            .filter_map(|x| buf.cell((x, rect.y)))
            .filter(|c| !underlined || c.modifier.contains(Modifier::UNDERLINED))
            .map(|c| c.symbol())
            .collect()
    };
    let line: Vec<char> = row(&terminal, false).chars().collect();
    let word: String = line[rect.x as usize..(rect.x + rect.width) as usize]
        .iter()
        .collect();
    assert_eq!(word, "1 agent · 0 terms · 1.0 GB", "the words, no padding");
    assert_eq!(rect.y, 29, "on the bar's own row");
    assert_eq!(row(&terminal, true), "", "at rest, nothing is underlined");

    let mouse = |app: &mut App, kind: MouseEventKind, out: &mut Vec<ClientRequest>| {
        handle_terminal_event(app, Event::Mouse(mev(kind, rect.x, rect.y)), out);
    };
    let mut out = Vec::new();
    mouse(&mut app, MouseEventKind::Moved, &mut out);
    assert_eq!(app.hover_crumb, Some(HitTarget::FooterUsage));
    terminal.draw(|f| ui::draw(f, &mut app)).unwrap();
    assert_eq!(row(&terminal, true), word, "the pointer underlines it");

    mouse(&mut app, MouseEventKind::Down(MouseButton::Left), &mut out);
    assert!(
        matches!(app.overlay, Some(Overlay::Metrics(_))),
        "the click opens the memory modal: {:?}",
        app.overlay
    );
    assert!(
        matches!(out.last(), Some(ClientRequest::GetMetrics { .. })),
        "and asks for a reading at once: {out:?}"
    );
    // The client's own RSS is a live `ps` of this test process, and
    // moves between the two readings: it is the one field let go.
    let steady = |overlay: Option<Overlay>| match overlay {
        Some(Overlay::Metrics(mut view)) => {
            view.client_rss_bytes = 0;
            format!("{view:?}")
        }
        other => panic!("expected the memory modal, got {other:?}"),
    };
    let clicked = steady(app.overlay.take());
    let mut keyed = Vec::new();
    handle_terminal_event(
        &mut app,
        Event::Key(KeyEvent::new(KeyCode::Char('M'), KeyModifiers::SHIFT)),
        &mut keyed,
    );
    assert_eq!(steady(app.overlay.take()), clicked, "the click is `⇧M`");
    assert!(matches!(
        keyed.last(),
        Some(ClientRequest::GetMetrics { .. })
    ));
}

/// Running / needs-feedback sessions head the list and hold their
/// place there however long they have been working — an old status
/// timestamp doesn't drop them below a fresher finish.
#[test]
fn working_sessions_head_the_list_regardless_of_age() {
    use nebula_core::{Agent, AgentStatus, Entity};
    let mut app = App::new();
    seed_tree(&mut app);
    let now = crate::app::now_ms();
    let stale = now - 2 * 3_600_000;
    let mk =
        |id: &str, status: AgentStatus, changed_at: i64, sort: i64| ServerEvent::EntityUpserted {
            entity: Entity::Agent(Agent {
                id: AgentId(id.into()),
                worktree_id: WorktreeId("w1".into()),
                name: id.into(),
                status,
                archived: false,
                archived_at: 0,
                unseen: false,
                kind: nebula_core::AgentKind::Claude,
                custom_harness: None,
                model: None,
                effort: None,
                session_id: None,
                cloud_session_id: None,
                sort_order: sort,
                status_changed_at: changed_at,
                alive: true,
                issue_url: None,
                recent_prompts: Vec::new(),
            }),
        };
    // Finished a moment ago: near the top on the timestamp alone.
    hse(
        &mut app,
        mk("just-finished", AgentStatus::Finished, now - 1_000, 1),
    );
    // Working for hours: still above it.
    hse(&mut app, mk("long-running", AgentStatus::Running, stale, 2));
    hse(
        &mut app,
        mk("long-blocked", AgentStatus::NeedsFeedback, stale, 3),
    );

    let rows = app.visible_sessions();
    let names: Vec<&str> = rows.iter().map(|a| a.name.as_str()).collect();
    assert_eq!(
        names,
        vec!["long-running", "long-blocked", "just-finished", "agent-1"],
        "working sessions on top, then the freshly-changed row, then the rest"
    );
    assert_eq!(app.session_group_counts(), (4, 0));
}

/// The list is ordered by last interaction, newest first — the session
/// you just ran surfaces at the top, and sessions that have never run
/// sink to the bottom in tree order. No group headers split it.
#[test]
fn sessions_order_by_last_interaction() {
    use nebula_core::{Agent, AgentStatus, Entity};
    let mut app = App::new();
    seed_tree(&mut app); // agent-1: fresh, never run (stamp 0)
    let now = crate::app::now_ms();
    let mins = |n: i64| now - n * 60_000;
    let mk = |id: &str, status: AgentStatus, at: i64, sort: i64| ServerEvent::EntityUpserted {
        entity: Entity::Agent(Agent {
            id: AgentId(id.into()),
            worktree_id: WorktreeId("w1".into()),
            name: id.into(),
            status,
            archived: false,
            archived_at: 0,
            unseen: false,
            kind: nebula_core::AgentKind::Claude,
            custom_harness: None,
            model: None,
            effort: None,
            session_id: None,
            cloud_session_id: None,
            sort_order: sort,
            status_changed_at: at,
            alive: true,
            issue_url: None,
            recent_prompts: Vec::new(),
        }),
    };
    // A long-running turn outranks a more recent finish, because a
    // working session is interacting with you right now. Seeded out of
    // stamp order on purpose.
    hse(&mut app, mk("working", AgentStatus::Running, mins(25), 1));
    hse(&mut app, mk("done-1m", AgentStatus::Finished, mins(1), 2));
    hse(&mut app, mk("done-10m", AgentStatus::Finished, mins(10), 3));
    hse(&mut app, mk("cold-2h", AgentStatus::Finished, mins(120), 4));
    hse(&mut app, mk("cold-45m", AgentStatus::Finished, mins(45), 5));

    let rows = app.visible_sessions();
    let names: Vec<&str> = rows.iter().map(|a| a.name.as_str()).collect();
    assert_eq!(
        names,
        vec![
            "working", "done-1m", "done-10m", // working on top, then newest first
            "cold-45m", "cold-2h", "agent-1", // never-run last
        ],
    );
    assert_eq!(app.session_group_counts(), (6, 0));

    let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
    terminal.draw(|f| ui::draw(f, &mut app)).unwrap();
    let text = buffer_text(&terminal);
    for header in ["PINNED", "RECENT", "UNPINNED"] {
        assert!(!text.contains(header), "no {header} header:\n{text}");
    }

    // A status flip is an interaction: the coldest row jumps the queue.
    hse(
        &mut app,
        ServerEvent::StatusChanged {
            agent: AgentId("cold-2h".into()),
            status: AgentStatus::Finished,
            changed_at: now,
            unseen: false,
        },
    );
    let rows = app.visible_sessions();
    let names: Vec<&str> = rows.iter().map(|a| a.name.as_str()).collect();
    assert_eq!(
        names,
        vec![
            "working", "cold-2h", "done-1m", "done-10m", //
            "cold-45m", "agent-1",
        ],
        "the cold row jumped to the top, behind only the live turn it ties with"
    );
}

/// Every session mid-turn counts as interacting *now*, so the raw
/// stamp breaks that tie: the newest turn leads. This is what puts the
/// session just launched — created `running`, stamped as it was created
/// — at the top of the list instead of under the sessions that have
/// been working for an hour, in the checkout's row and the project's too.
#[test]
fn a_session_created_running_leads_the_working_ones() {
    use nebula_core::{Agent, AgentStatus, Entity};
    let mut app = App::new();
    seed_tree(&mut app); // agent-1: fresh, never run
    let now = crate::app::now_ms();
    let mk = |id: &str, status: AgentStatus, at: i64, sort: i64| ServerEvent::EntityUpserted {
        entity: Entity::Agent(Agent {
            id: AgentId(id.into()),
            worktree_id: WorktreeId("w1".into()),
            name: id.into(),
            status,
            archived: false,
            archived_at: 0,
            unseen: false,
            kind: nebula_core::AgentKind::Claude,
            custom_harness: None,
            model: None,
            effort: None,
            session_id: None,
            cloud_session_id: None,
            sort_order: sort,
            status_changed_at: at,
            alive: true,
            issue_url: None,
            recent_prompts: Vec::new(),
        }),
    };
    hse(
        &mut app,
        mk("working-1h", AgentStatus::Running, now - 3_600_000, 1),
    );
    hse(
        &mut app,
        mk(
            "blocked-20m",
            AgentStatus::NeedsFeedback,
            now - 1_200_000,
            2,
        ),
    );
    // The create the user just fired: the DAEMON stamps it as it makes
    // the row, and says `running` because the launch carried a task.
    hse(&mut app, mk("just-launched", AgentStatus::Running, now, 3));

    let names: Vec<String> = app
        .visible_sessions()
        .iter()
        .map(|a| a.name.clone())
        .collect();
    assert_eq!(
        names,
        vec!["just-launched", "blocked-20m", "working-1h", "agent-1"],
        "the newest turn first, the oldest working row last"
    );
}

/// A StatusChanged delta stamps the agent's timestamp, pulls it to the
/// top, and the selection follows the session it was on.
#[test]
fn status_change_resorts_and_selection_follows() {
    use nebula_core::{Agent, AgentStatus, Entity};
    let mut app = App::new();
    seed_tree(&mut app);
    hse(
        &mut app,
        ServerEvent::EntityUpserted {
            entity: Entity::Agent(Agent {
                id: AgentId("a2".into()),
                worktree_id: WorktreeId("w1".into()),
                name: "agent-2".into(),
                status: AgentStatus::Fresh,
                archived: false,
                archived_at: 0,
                unseen: false,
                kind: nebula_core::AgentKind::Claude,
                custom_harness: None,
                model: None,
                effort: None,
                session_id: None,
                cloud_session_id: None,
                sort_order: 1,
                status_changed_at: 0,
                alive: true,
                issue_url: None,
                recent_prompts: Vec::new(),
            }),
        },
    );
    app.focus = Focus::Sessions;
    app.sel_session = 1; // agent-2

    hse(
        &mut app,
        ServerEvent::StatusChanged {
            agent: AgentId("a2".into()),
            status: AgentStatus::Finished,
            changed_at: crate::app::now_ms(),
            unseen: false,
        },
    );
    let rows = app.visible_sessions();
    assert_eq!(
        rows[0].name, "agent-2",
        "the stamped agent bubbled to the top"
    );
    assert_eq!(app.session_group_counts(), (2, 0));
    assert_eq!(app.sel_session, 0, "selection followed agent-2");
}

/// Confirming a worktree delete drops the row (and its agents)
/// immediately — the daemon deletes in the background — and an Error
/// reply for that request restores them where they were.
#[test]
fn worktree_delete_is_optimistic_and_rolls_back_on_error() {
    use nebula_core::{Agent, AgentStatus, Entity, Worktree, WorktreeId};
    let mut app = App::new();
    seed_tree(&mut app);
    let wt_id = WorktreeId("w2".into());
    hse(
        &mut app,
        ServerEvent::EntityUpserted {
            entity: Entity::Worktree(Worktree {
                id: wt_id.clone(),
                project_id: nebula_core::ProjectId("p1".into()),
                path: "/tmp/demo-feature".into(),
                branch: "feature".into(),
                is_main: false,
                sort_order: 0,
            }),
        },
    );
    hse(
        &mut app,
        ServerEvent::EntityUpserted {
            entity: Entity::Agent(Agent {
                id: AgentId("a2".into()),
                worktree_id: wt_id.clone(),
                name: "agent-2".into(),
                status: AgentStatus::Fresh,
                archived: false,
                archived_at: 0,
                unseen: false,
                kind: nebula_core::AgentKind::Claude,
                custom_harness: None,
                model: None,
                effort: None,
                session_id: None,
                cloud_session_id: None,
                sort_order: 0,
                status_changed_at: 0,
                alive: true,
                issue_url: None,
                recent_prompts: Vec::new(),
            }),
        },
    );

    // Confirmed delete: rows vanish before any daemon reply.
    let mut out = Vec::new();
    run_pending_action(
        &mut app,
        PendingAction::DeleteWorktree(wt_id.clone()),
        &mut out,
    );
    let req_id = match out.as_slice() {
        [ClientRequest::DeleteWorktree { req_id, id, .. }] if *id == wt_id => *req_id,
        other => panic!("expected DeleteWorktree request, got {other:?}"),
    };
    assert!(!app.tree.worktrees.iter().any(|w| w.id == wt_id));
    assert!(!app.tree.agents.iter().any(|a| a.worktree_id == wt_id));

    // Daemon says the delete failed: rows come back, error flashes.
    hse(
        &mut app,
        ServerEvent::Error {
            req_id: Some(req_id),
            message: "worktree dirty".into(),
        },
    );
    assert_eq!(
        app.tree.worktrees.iter().position(|w| w.id == wt_id),
        Some(1),
        "worktree restored at its old index"
    );
    assert!(app.tree.agents.iter().any(|a| a.worktree_id == wt_id));
    assert_eq!(app.flash.as_deref(), Some("worktree dirty"));
    assert!(
        app.pending.is_empty(),
        "failed request leaves no pending intent"
    );
}

fn dir_names(p: &crate::app::PromptDialog) -> Vec<&str> {
    p.dirs.iter().map(|d| d.name.as_str()).collect()
}

#[test]
fn tab_in_add_project_prompt_completes_paths() {
    use crate::app::{Overlay, PromptDialog, PromptKind};
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(tmp.path().join("workspace/nebula")).unwrap();
    std::fs::create_dir_all(tmp.path().join("workspace/herdr")).unwrap();

    let mut app = App::new();
    let mut out = Vec::new();
    app.overlay = Some(Overlay::Prompt(PromptDialog::new(
        "Add project",
        "path",
        format!("{}/work", tmp.path().display()),
        PromptKind::AddProject,
    )));

    // Unambiguous: work → workspace/, and the listing follows it in.
    handle_key(
        &mut app,
        KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE),
        &mut out,
    );
    let Some(Overlay::Prompt(p)) = &app.overlay else {
        panic!("prompt closed")
    };
    assert_eq!(p.input, format!("{}/workspace/", tmp.path().display()));
    assert_eq!(dir_names(p), vec!["herdr", "nebula"]);

    // Ambiguous: Tab makes no progress, the listing already shows both.
    handle_key(
        &mut app,
        KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE),
        &mut out,
    );
    let Some(Overlay::Prompt(p)) = &app.overlay else {
        panic!("prompt closed")
    };
    assert_eq!(p.input, format!("{}/workspace/", tmp.path().display()));
    assert_eq!(dir_names(p), vec!["herdr", "nebula"]);

    // Typing narrows the listing; the next Tab completes fully.
    handle_key(
        &mut app,
        KeyEvent::new(KeyCode::Char('n'), KeyModifiers::NONE),
        &mut out,
    );
    let Some(Overlay::Prompt(p)) = &app.overlay else {
        panic!("prompt closed")
    };
    assert_eq!(dir_names(p), vec!["nebula"]);
    handle_key(
        &mut app,
        KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE),
        &mut out,
    );
    let Some(Overlay::Prompt(p)) = &app.overlay else {
        panic!("prompt closed")
    };
    assert_eq!(
        p.input,
        format!("{}/workspace/nebula/", tmp.path().display())
    );
}

#[test]
fn add_project_prompt_browses_with_arrows_and_submits_hovered() {
    use crate::app::{Overlay, PromptDialog, PromptKind};
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(tmp.path().join("ws/beta/inner")).unwrap();
    std::fs::create_dir_all(tmp.path().join("ws/alpha/.git")).unwrap();

    let mut app = App::new();
    let mut out = Vec::new();
    app.overlay = Some(Overlay::Prompt(PromptDialog::new(
        "Add project",
        "path",
        format!("{}/ws/", tmp.path().display()),
        PromptKind::AddProject,
    )));
    let Some(Overlay::Prompt(p)) = &app.overlay else {
        panic!("prompt closed")
    };
    assert_eq!(dir_names(p), vec!["alpha", "beta"]);
    assert!(p.dirs[0].is_repo && !p.dirs[1].is_repo);
    assert_eq!(p.hover, None, "opens on the input row");

    // ↓↓ highlights beta; → dives into it and lists its children.
    for _ in 0..2 {
        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Down, KeyModifiers::NONE),
            &mut out,
        );
    }
    let Some(Overlay::Prompt(p)) = &app.overlay else {
        panic!("prompt closed")
    };
    assert_eq!(
        p.hovered_path(),
        Some(format!("{}/ws/beta", tmp.path().display()))
    );
    handle_key(
        &mut app,
        KeyEvent::new(KeyCode::Right, KeyModifiers::NONE),
        &mut out,
    );
    let Some(Overlay::Prompt(p)) = &app.overlay else {
        panic!("prompt closed")
    };
    assert_eq!(p.input, format!("{}/ws/beta/", tmp.path().display()));
    assert_eq!(dir_names(p), vec!["inner"]);
    assert_eq!(p.hover, None, "diving resets the highlight");

    // ← steps back up to ws/.
    handle_key(
        &mut app,
        KeyEvent::new(KeyCode::Left, KeyModifiers::NONE),
        &mut out,
    );
    let Some(Overlay::Prompt(p)) = &app.overlay else {
        panic!("prompt closed")
    };
    assert_eq!(p.input, format!("{}/ws/", tmp.path().display()));

    // ↓ + Enter adds the highlighted directory, not the typed parent.
    handle_key(
        &mut app,
        KeyEvent::new(KeyCode::Down, KeyModifiers::NONE),
        &mut out,
    );
    handle_key(
        &mut app,
        KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
        &mut out,
    );
    assert!(app.overlay.is_none());
    assert!(matches!(
        out.as_slice(),
        [ClientRequest::AddProject { path, create_missing: false, .. }]
            if path == &tmp.path().join("ws/alpha")
    ));
}

#[test]
fn add_project_prefill_yields_to_absolute_paths() {
    use crate::app::{Overlay, PromptDialog, PromptKind};
    let mut app = App::new();
    let mut out = Vec::new();
    app.overlay = Some(Overlay::Prompt(PromptDialog::new(
        "Add project",
        "path",
        "~/",
        PromptKind::AddProject,
    )));
    handle_key(
        &mut app,
        KeyEvent::new(KeyCode::Char('/'), KeyModifiers::NONE),
        &mut out,
    );
    let Some(Overlay::Prompt(p)) = &app.overlay else {
        panic!("prompt closed")
    };
    assert_eq!(p.input, "/", "leading '/' replaces the untouched prefill");
}

#[test]
fn tab_in_name_prompt_does_not_complete() {
    use crate::app::{Overlay, PromptDialog, PromptKind};
    let mut app = App::new();
    seed_tree(&mut app);
    let mut out = Vec::new();
    app.overlay = Some(Overlay::Prompt(PromptDialog::new(
        "Rename agent",
        "name",
        "src", // a dir that exists in cwd — must NOT complete
        PromptKind::RenameAgent {
            id: nebula_core::AgentId("a1".into()),
        },
    )));
    handle_key(
        &mut app,
        KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE),
        &mut out,
    );
    let Some(Overlay::Prompt(p)) = &app.overlay else {
        panic!("prompt closed")
    };
    assert_eq!(p.input, "src", "name prompts ignore Tab");
}

#[test]
fn keys_route_by_focus() {
    let mut app = App::new();
    seed_tree(&mut app);
    let mut out = Vec::new();

    // Panel focus: 'q' asks first, then quits on `y`.
    handle_key(
        &mut app,
        KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE),
        &mut out,
    );
    assert!(!app.should_quit, "q asks before it quits");
    assert!(
        matches!(&app.overlay, Some(Overlay::Confirm(c)) if c.action == PendingAction::Quit),
        "q opens the quit confirm"
    );
    handle_key(
        &mut app,
        KeyEvent::new(KeyCode::Char('y'), KeyModifiers::NONE),
        &mut out,
    );
    assert!(app.should_quit);
    app.should_quit = false;

    // Terminal input-locked: 'q' is forwarded, Ctrl+q escapes and unlocks.
    app.focus = Focus::Terminal;
    app.term_locked = true;
    let sref = SessionRef::Agent(AgentId("a1".into()));
    app.term = Some(AttachedTerm::new(sref, 80, 24));
    handle_key(
        &mut app,
        KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE),
        &mut out,
    );
    assert!(!app.should_quit, "q must forward to pty, not quit");
    assert!(matches!(out.last(), Some(ClientRequest::Input { data, .. }) if data == b"q"));
    handle_key(
        &mut app,
        KeyEvent::new(KeyCode::Char('q'), KeyModifiers::CONTROL),
        &mut out,
    );
    assert_eq!(app.focus, Focus::Sessions, "Ctrl+q escapes to panels");
    assert!(!app.term_locked, "Ctrl+q clears the input lock");
}

/// `q` sits among the panel hotkeys, so a letter meant for an agent used
/// to end the client outright. Both quit chords ask first, and backing
/// out leaves the app exactly where it was.
#[test]
fn quit_asks_before_it_closes_the_tui() {
    for chord in [
        KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE),
        KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL),
    ] {
        let mut app = App::new();
        seed_tree(&mut app);
        let mut out = Vec::new();

        handle_key(&mut app, chord, &mut out);
        let Some(Overlay::Confirm(c)) = &app.overlay else {
            panic!("{chord:?} opens the quit confirm, got {:?}", app.overlay)
        };
        assert_eq!(c.action, PendingAction::Quit);
        // The dialog is sized to its longest line and never wraps.
        assert!(
            c.message.lines().all(|l| l.chars().count() < 52),
            "quit message must fit the dialog: {:?}",
            c.message
        );
        assert!(!app.should_quit);

        // Esc backs out to the panels, still running.
        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
            &mut out,
        );
        assert!(app.overlay.is_none(), "Esc closes the confirm");
        assert!(!app.should_quit, "Esc keeps the TUI up");

        // Asked again, `y` goes through.
        handle_key(&mut app, chord, &mut out);
        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('y'), KeyModifiers::NONE),
            &mut out,
        );
        assert!(app.should_quit, "y quits");
    }

    // Ctrl+C twice is never a trap: the second press goes through.
    let mut app = App::new();
    seed_tree(&mut app);
    let mut out = Vec::new();
    let ctrl_c = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
    handle_key(&mut app, ctrl_c, &mut out);
    handle_key(&mut app, ctrl_c, &mut out);
    assert!(app.should_quit, "a second Ctrl+C quits");
    assert!(app.overlay.is_none());
}

/// Picker/submenu tests resolve model/effort through `Config::load`, so
/// pin the config to an empty temp file to stay off the dev's real one.
pub(super) fn with_default_config<T>(f: impl FnOnce() -> T) -> T {
    let dir = tempfile::tempdir().unwrap();
    crate::config::with_config_path(dir.path().join("config.json"), f)
}

/// Pin the config to a temp file holding `json` for the duration of `f`.
pub(super) fn with_config_json<T>(json: &str, f: impl FnOnce() -> T) -> T {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.json");
    std::fs::write(&path, json).unwrap();
    crate::config::with_config_path(path, f)
}

/// `n`, then Enter on a harness, is the whole flow: the session is
/// created right there. No box asks for a name or a first prompt — a
/// launch that starts from a typed task is the QUICK PROMPT's (`p`).
#[test]
fn n_in_sessions_opens_agent_type_picker_and_enter_starts_the_session() {
    with_default_config(|| {
        let mut app = App::new();
        seed_tree(&mut app);
        app.focus = Focus::Sessions;
        let mut out = Vec::new();

        open_picker(&mut app);
        let Some(Overlay::Menu(menu)) = &app.overlay else {
            panic!("expected agent-type picker, got {:?}", app.overlay);
        };
        assert_eq!(menu.title.as_deref(), Some("New session"));
        assert_eq!(
                menu.items.len(),
                AgentKind::ALL.len() - 1,
                "one row per harness, no Terminal row: NEW TERMINAL (`t`) already covers it.                  A bare Custom kind never lists (entries come from the registry, empty here)"
            );
        assert_eq!(menu.items[0].label, "Claude");
        assert_eq!(menu.items[1].label, "Codex");
        assert_eq!(menu.items[2].label, "Cursor");
        assert_eq!(menu.hover, 0, "Claude is the default");

        // Enter on the default is the launch, with kind=Claude: no box
        // follows, so there is no STARTING PROMPT — the CLI's own input
        // is the first prompt — and the row takes the next free default
        // name and AUTO-TITLE. Nothing configured → no model/effort
        // flags. The default-spec warm slot the create adopts is
        // refilled right behind it, and that is the only prewarm: with
        // no box there is no typing to warm under.
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE, &mut out);
        assert!(app.overlay.is_none(), "no box: {:?}", app.overlay);
        assert!(
            matches!(
                out.as_slice(),
                [
                    ClientRequest::CreateAgent {
                        name,
                        kind: AgentKind::Claude,
                        model: None,
                        effort: None,
                        auto_title: true,
                        cloud_prompt: None,
                        starting_prompt: None,
                        ..
                    },
                    ClientRequest::PrewarmAgent {
                        kind: AgentKind::Claude,
                        model: None,
                        effort: None,
                        ..
                    }
                ] if name == "agent-2"
            ),
            "one create, then the refill: {out:?}"
        );
    })
}

/// The pick takes the TERMINAL PANE, as every picker-walked launch
/// does — the first prompt is typed there.
#[test]
fn a_picked_session_launches_into_the_selected_worktree_and_takes_the_pane() {
    use nebula_core::{Agent, AgentStatus, Entity, WorktreeId};

    with_default_config(|| {
        let mut app = App::new();
        seed_tree(&mut app);
        app.focus = Focus::Sessions;
        let worktree = app.selected_worktree().unwrap().id.clone();
        let mut out = Vec::new();

        open_picker(&mut app);
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE, &mut out);
        assert!(app.overlay.is_none(), "no box: {:?}", app.overlay);
        assert!(
            matches!(
                &out[0],
                ClientRequest::CreateAgent { worktree: w, .. } if *w == worktree
            ),
            "{out:?}"
        );

        // The daemon broadcasts the new row before it acks the create;
        // the Ack enters and locks its pane.
        let req_id = match &out[0] {
            ClientRequest::CreateAgent { req_id, .. } => *req_id,
            other => panic!("expected create request, got {other:?}"),
        };
        let template = app.tree.agents[0].clone();
        hse(
            &mut app,
            ServerEvent::EntityUpserted {
                entity: Entity::Agent(Agent {
                    id: AgentId("a2".into()),
                    worktree_id: WorktreeId("w1".into()),
                    name: "agent-2".into(),
                    status: AgentStatus::Fresh,
                    kind: AgentKind::Claude,
                    custom_harness: None,
                    sort_order: 1,
                    ..template
                }),
            },
        );
        hse(
            &mut app,
            ServerEvent::Ack {
                req_id,
                created: Some(EntityId::Agent(AgentId("a2".into()))),
            },
        );
        assert_eq!(app.focus, Focus::Terminal, "straight into the pane");
        assert!(app.term_locked, "typing goes to the new agent");
    })
}

#[test]
fn tab_on_claude_toggles_cloud_and_collects_a_multiline_task() {
    with_default_config(|| {
        let mut app = App::new();
        seed_tree(&mut app);
        app.focus = Focus::Sessions;
        let mut out = Vec::new();

        open_picker(&mut app);
        press(&mut app, KeyCode::Tab, KeyModifiers::NONE, &mut out);
        let Some(Overlay::Menu(menu)) = &app.overlay else {
            panic!("expected new-session picker");
        };
        assert_eq!(menu.items[0].label, "Claude · cloud");
        assert!(matches!(
            &menu.items[0].action,
            MenuAction::NewAgentOfKind {
                kind: AgentKind::Claude,
                custom: None,
                cloud: true,
                ..
            }
        ));

        // Cloud creation is cold on purpose: a bare-Claude warm PTY
        // cannot be adopted because it never received --cloud + task.
        // No name step either: the multiline task prompt opens straight
        // from the picker.
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE, &mut out);
        assert!(out.is_empty(), "cloud task entry must not prewarm: {out:?}");
        let Some(Overlay::Prompt(prompt)) = &app.overlay else {
            panic!("expected Claude Cloud task prompt");
        };
        assert_eq!(prompt.title, "Claude Cloud task");
        assert!(prompt.is_multiline());

        assert!(paste_into_overlay(&mut app, "Fix auth"));
        press(&mut app, KeyCode::Enter, KeyModifiers::SHIFT, &mut out);
        assert!(paste_into_overlay(&mut app, "Run the tests"));
        press(
            &mut app,
            KeyCode::Char('j'),
            KeyModifiers::CONTROL,
            &mut out,
        );
        assert!(paste_into_overlay(&mut app, "Ship it"));
        let Some(Overlay::Prompt(prompt)) = &app.overlay else {
            panic!("task prompt closed while editing");
        };
        assert_eq!(prompt.input.as_str(), "Fix auth\nRun the tests\nShip it");

        press(&mut app, KeyCode::Enter, KeyModifiers::NONE, &mut out);
        assert!(app.overlay.is_none());
        assert!(matches!(
            out.as_slice(),
            [ClientRequest::CreateAgent {
                kind: AgentKind::Claude,
                custom_harness: None,
                cloud_prompt: Some(task),
                auto_title: true,
                ..
            }] if task == "Fix auth\nRun the tests\nShip it"
        ));
        assert!(
            !out.iter()
                .any(|request| matches!(request, ClientRequest::PrewarmAgent { .. })),
            "cloud launch must never consume/refill the local warm slot: {out:?}"
        );

        let req_id = match &out[0] {
            ClientRequest::CreateAgent { req_id, .. } => *req_id,
            other => panic!("expected create request, got {other:?}"),
        };
        handle_server_event(
            &mut app,
            ServerEvent::Error {
                req_id: Some(req_id),
                message: "cloud unavailable — retry".into(),
            },
            &mut out,
        );
        assert_eq!(app.flash.as_deref(), Some("cloud unavailable — retry"));
        assert!(matches!(
            &app.overlay,
            Some(Overlay::Prompt(prompt))
                if prompt.is_multiline()
                    && prompt.input.as_str() == "Fix auth\nRun the tests\nShip it"
        ));
    })
}

/// `Tab` is the cloud toggle in the Claude MODEL / EFFORT lists too,
/// not only on the picker's Claude row: one launch, one switch. `←`
/// backs out to a picker whose Claude row agrees, and the pick asks
/// for the cloud task on the model and effort drilled to. Another
/// harness's list leaves Tab alone.
#[test]
fn tab_in_a_claude_submenu_toggles_cloud_for_the_whole_launch() {
    with_default_config(|| {
        let mut app = App::new();
        seed_tree(&mut app);
        app.focus = Focus::Sessions;
        let mut out = Vec::new();
        fn menu(app: &App) -> &crate::app::ContextMenu {
            match &app.overlay {
                Some(Overlay::Menu(menu)) => menu,
                other => panic!("expected a menu, got {other:?}"),
            }
        }

        open_picker(&mut app);
        press(&mut app, KeyCode::Right, KeyModifiers::NONE, &mut out);
        assert_eq!(menu(&app).hovered_claude_cloud(), Some(false));
        assert_eq!(
                ui::menu_footer_hint(menu(&app)).as_deref(),
                Some("Tab: cloud off  type to filter  ↑/↓: move  Backspace: widen  ?: settings  Enter: pick  Esc: back")
            );
        press(&mut app, KeyCode::Tab, KeyModifiers::NONE, &mut out);
        assert_eq!(menu(&app).items[2].label, "opus", "a model row stays one");
        assert!(menu(&app).lists_claude_cloud());
        assert!(
            ui::menu_footer_hint(menu(&app))
                .is_some_and(|hint| hint.starts_with("Tab: cloud on  ")),
            "{:?}",
            ui::menu_footer_hint(menu(&app))
        );

        // ← to the picker: its Claude row took the toggle with it.
        press(&mut app, KeyCode::Left, KeyModifiers::NONE, &mut out);
        assert_eq!(menu(&app).items[0].label, "Claude · cloud");
        assert_eq!(menu(&app).hovered_claude_cloud(), Some(true));

        // → again, down to opus, → to its efforts: still cloud.
        press(&mut app, KeyCode::Right, KeyModifiers::NONE, &mut out);
        press(&mut app, KeyCode::Down, KeyModifiers::NONE, &mut out);
        press(&mut app, KeyCode::Down, KeyModifiers::NONE, &mut out);
        press(&mut app, KeyCode::Right, KeyModifiers::NONE, &mut out);
        assert_eq!(menu(&app).title.as_deref(), Some("Claude effort"));
        assert_eq!(menu(&app).hovered_claude_cloud(), Some(true));
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE, &mut out);
        assert!(out.is_empty(), "the cloud task comes first: {out:?}");
        assert!(
            matches!(
                &app.overlay,
                Some(Overlay::Prompt(p)) if matches!(
                    &p.kind,
                    PromptKind::ClaudeCloudTask { model: Some(m), .. } if m == "opus"
                )
            ),
            "{:?}",
            app.overlay
        );

        // Codex's model list: Tab is nobody's, and the footer is the
        // plain type-ahead one.
        open_picker(&mut app);
        press(&mut app, KeyCode::Down, KeyModifiers::NONE, &mut out);
        press(&mut app, KeyCode::Right, KeyModifiers::NONE, &mut out);
        assert_eq!(menu(&app).title.as_deref(), Some("Codex model"));
        assert_eq!(menu(&app).hovered_claude_cloud(), None);
        press(&mut app, KeyCode::Tab, KeyModifiers::NONE, &mut out);
        assert!(!menu(&app).lists_claude_cloud());
        assert!(
            ui::menu_footer_hint(menu(&app)).is_some_and(|hint| !hint.contains("cloud")),
            "{:?}",
            ui::menu_footer_hint(menu(&app))
        );
    })
}

/// The one picker row that still asks: a cloud launch is nothing
/// without its task.
#[test]
fn cloud_task_is_still_required_though_the_picker_launches_directly() {
    with_default_config(|| {
        let mut app = App::new();
        seed_tree(&mut app);
        app.focus = Focus::Sessions;
        let mut out = Vec::new();

        open_picker(&mut app);
        press(&mut app, KeyCode::Tab, KeyModifiers::NONE, &mut out);
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE, &mut out);

        assert!(out.is_empty(), "the task dialog comes before creation");
        assert!(matches!(
            &app.overlay,
            Some(Overlay::Prompt(p)) if p.is_multiline()
        ));

        // Empty is validation, not dismissal: keep the dialog open so
        // the user can correct it in place.
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE, &mut out);
        assert!(out.is_empty());
        assert!(matches!(
            &app.overlay,
            Some(Overlay::Prompt(p)) if p.is_multiline()
        ));

        let Some(Overlay::Prompt(prompt)) = &mut app.overlay else {
            unreachable!()
        };
        prompt.input.set_text("fix\0auth");
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE, &mut out);
        assert_eq!(
            app.flash.as_deref(),
            Some("Claude Cloud task cannot contain NUL bytes")
        );
        assert!(matches!(&app.overlay, Some(Overlay::Prompt(p)) if p.is_multiline()));

        let Some(Overlay::Prompt(prompt)) = &mut app.overlay else {
            unreachable!()
        };
        prompt
            .input
            .set_text("x".repeat(MAX_CLOUD_PROMPT_BYTES + 1));
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE, &mut out);
        assert_eq!(
            app.flash.as_deref(),
            Some("Claude Cloud task is too long (max 16 KiB)")
        );
        assert!(matches!(&app.overlay, Some(Overlay::Prompt(p)) if p.is_multiline()));
    });
}

#[test]
fn claude_cloud_task_prompt_soft_wraps_instead_of_horizontally_scrolling() {
    let mut app = App::new();
    seed_tree(&mut app);
    app.overlay = Some(Overlay::Prompt(PromptDialog::new(
        "Claude Cloud task",
        "what should Claude do?",
        format!("BEGIN {} END", "word ".repeat(20)),
        PromptKind::ClaudeCloudTask {
            worktree: WorktreeId("w1".into()),
            name: String::new(),
            model: None,
            effort: None,
        },
    )));
    let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
    terminal.draw(|f| crate::ui::draw(f, &mut app)).unwrap();

    let begin = find_cell(&terminal, "BEGIN");
    let end = find_cell(&terminal, "END");
    assert_ne!(begin.1, end.1, "long task should wrap across rows");
    assert!(buffer_text(&terminal).contains("Shift+Enter/^J: newline"));

    let Some(Overlay::Prompt(prompt)) = &mut app.overlay else {
        unreachable!()
    };
    prompt.input.set_text("VISIBLE");
    let mut small = Terminal::new(TestBackend::new(32, 6)).unwrap();
    small.draw(|f| crate::ui::draw(f, &mut app)).unwrap();
    let text = buffer_text(&small);
    assert!(
        text.contains("VISIBLE"),
        "small prompt lost its editor: {text}"
    );
    assert!(text.contains("Esc") && text.contains("^J") && text.contains("Enter"));
}

/// The submenu picks apply to the direct launch: the model row Enter
/// lands on is what the create carries.
#[test]
fn enter_on_a_submenu_model_row_creates_with_that_model() {
    with_default_config(|| {
        let mut app = App::new();
        seed_tree(&mut app);
        app.focus = Focus::Sessions;
        let mut out = Vec::new();

        open_picker(&mut app);
        // → into Claude's model list, down to "opus", Enter.
        press(&mut app, KeyCode::Right, KeyModifiers::NONE, &mut out);
        let Some(Overlay::Menu(menu)) = &app.overlay else {
            panic!("expected model submenu, got {:?}", app.overlay);
        };
        let opus = menu
            .items
            .iter()
            .position(|i| i.label.starts_with("opus"))
            .expect("opus row");
        // Letters type ahead in a model submenu, so move with ↓.
        for _ in 0..opus {
            press(&mut app, KeyCode::Down, KeyModifiers::NONE, &mut out);
        }
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE, &mut out);
        assert!(app.overlay.is_none(), "no box: {:?}", app.overlay);
        assert!(matches!(
            &out[0],
            ClientRequest::CreateAgent {
                kind: AgentKind::Claude,
                custom_harness: None,
                model: Some(m),
                auto_title: true,
                ..
            } if m == "opus"
        ));
    })
}

#[test]
fn picker_right_drills_into_model_then_effort_submenus() {
    with_default_config(|| {
        let mut app = App::new();
        seed_tree(&mut app);
        app.focus = Focus::Sessions;
        let mut out = Vec::new();

        open_picker(&mut app);
        let Some(Overlay::Menu(menu)) = &app.overlay else {
            panic!("expected picker, got {:?}", app.overlay);
        };
        // Every row advertises a model submenu (the ▸ affordance) —
        // Cursor too, since `cursor-agent --model` grew a catalogue.
        assert_eq!(menu.items[0].action.submenu(), Some(SubmenuKind::Models));
        assert_eq!(menu.items[1].action.submenu(), Some(SubmenuKind::Models));
        assert_eq!(menu.items[2].action.submenu(), Some(SubmenuKind::Models));

        // → opens the model list; nothing configured, so the "default"
        // row is checked and highlighted, and the parent is kept for ←.
        press(&mut app, KeyCode::Right, KeyModifiers::NONE, &mut out);
        let Some(Overlay::Menu(menu)) = &app.overlay else {
            panic!("expected model submenu, got {:?}", app.overlay);
        };
        assert_eq!(menu.title.as_deref(), Some("Claude model"));
        assert_eq!(
            menu.items.len(),
            crate::config::model_choices(AgentKind::Claude, None).len()
        );
        assert_eq!(menu.items[0].label, "default ✓");
        assert_eq!(menu.items[2].label, "opus");
        assert_eq!(menu.hover, 0);
        assert!(menu.parent.is_some());
        // Model rows drill further into the effort list…
        assert_eq!(menu.items[2].action.submenu(), Some(SubmenuKind::Efforts));

        // …so ↓↓ to opus, → again: efforts for that model.
        press(&mut app, KeyCode::Down, KeyModifiers::NONE, &mut out);
        press(&mut app, KeyCode::Down, KeyModifiers::NONE, &mut out);
        press(&mut app, KeyCode::Right, KeyModifiers::NONE, &mut out);
        let Some(Overlay::Menu(menu)) = &app.overlay else {
            panic!("expected effort submenu, got {:?}", app.overlay);
        };
        assert_eq!(menu.title.as_deref(), Some("Claude effort"));
        assert_eq!(
            menu.items.len(),
            crate::config::effort_choices(AgentKind::Claude, Some("opus"), None).len()
        );
        assert!(matches!(
            &menu.items[3].action,
            MenuAction::NewAgentOfKind { kind: AgentKind::Claude, model: Some(m), effort: Some(e), .. }
                if m == "opus" && e == "high"
        ));
        // Effort rows are leaves.
        assert_eq!(menu.items[3].action.submenu(), None);

        // ← backs out to the models; Esc also backs out one level, and
        // only closes from the top.
        press(&mut app, KeyCode::Left, KeyModifiers::NONE, &mut out);
        let Some(Overlay::Menu(menu)) = &app.overlay else {
            panic!("expected model submenu after ←");
        };
        assert_eq!(menu.title.as_deref(), Some("Claude model"));
        assert_eq!(menu.hover, 2, "← restores the parent's hover");
        press(&mut app, KeyCode::Esc, KeyModifiers::NONE, &mut out);
        let Some(Overlay::Menu(menu)) = &app.overlay else {
            panic!("expected root picker after Esc");
        };
        assert_eq!(menu.title.as_deref(), Some("New session"));
        press(&mut app, KeyCode::Esc, KeyModifiers::NONE, &mut out);
        assert!(app.overlay.is_none());
        assert!(
            !out.iter()
                .any(|r| matches!(r, ClientRequest::CreateAgent { .. })),
            "browsing submenus must not create anything"
        );
    })
}

/// `?` on a New session row swaps the picker for settings parked on
/// that harness's Agents section; `?` in a model submenu lands on
/// the same section. Esc, then `s`, comes back where `?` left.
#[test]
fn question_mark_jumps_from_picker_to_harness_settings() {
    use crate::config::{agents_tab, locate_agent, HarnessField};
    with_default_config(|| {
        let mut app = App::new();
        seed_tree(&mut app);
        app.focus = Focus::Sessions;
        let mut out = Vec::new();

        open_picker(&mut app);
        // Down to Codex, `?`: settings on the Agents tab, cursor on
        // Codex's Enabled row.
        press(&mut app, KeyCode::Down, KeyModifiers::NONE, &mut out);
        press(&mut app, KeyCode::Char('?'), KeyModifiers::NONE, &mut out);
        let Some(Overlay::Settings(view)) = &app.overlay else {
            panic!("expected settings, got {:?}", app.overlay);
        };
        let (tab, row) = locate_agent("codex", HarnessField::Enabled).unwrap();
        assert_eq!(tab, agents_tab());
        assert_eq!(view.tab, tab);
        assert_eq!(view.selected, row);
        assert!(!view.on_tabs);

        // Esc, then `s`: back where `?` left.
        press(&mut app, KeyCode::Esc, KeyModifiers::NONE, &mut out);
        press(&mut app, KeyCode::Char('s'), KeyModifiers::NONE, &mut out);
        let Some(Overlay::Settings(view)) = &app.overlay else {
            panic!("expected settings, got {:?}", app.overlay);
        };
        assert_eq!((view.tab, view.selected), (tab, row));

        // `?` inside a model submenu lands on the same section.
        press(&mut app, KeyCode::Esc, KeyModifiers::NONE, &mut out);
        open_picker(&mut app);
        press(&mut app, KeyCode::Right, KeyModifiers::NONE, &mut out);
        press(&mut app, KeyCode::Char('?'), KeyModifiers::NONE, &mut out);
        let Some(Overlay::Settings(view)) = &app.overlay else {
            panic!("expected settings, got {:?}", app.overlay);
        };
        let (tab, row) = locate_agent("claude", HarnessField::Enabled).unwrap();
        assert_eq!((view.tab, view.selected), (tab, row));
    })
}

/// `s` jumps like `?` on the filter-less picker, but types in the
/// model submenu, where it narrows the list instead.
#[test]
fn s_jumps_on_the_picker_and_types_in_submenus() {
    use crate::config::{agents_tab, locate_agent, HarnessField};
    with_default_config(|| {
        let mut app = App::new();
        seed_tree(&mut app);
        app.focus = Focus::Sessions;
        let mut out = Vec::new();

        open_picker(&mut app);
        press(&mut app, KeyCode::Down, KeyModifiers::NONE, &mut out);
        press(&mut app, KeyCode::Char('s'), KeyModifiers::NONE, &mut out);
        let Some(Overlay::Settings(view)) = &app.overlay else {
            panic!("expected settings, got {:?}", app.overlay);
        };
        let (tab, row) = locate_agent("codex", HarnessField::Enabled).unwrap();
        assert_eq!(tab, agents_tab());
        assert_eq!((view.tab, view.selected), (tab, row));

        // In a model submenu `s` is a filter letter, not a jump.
        press(&mut app, KeyCode::Esc, KeyModifiers::NONE, &mut out);
        open_picker(&mut app);
        press(&mut app, KeyCode::Right, KeyModifiers::NONE, &mut out);
        press(&mut app, KeyCode::Char('s'), KeyModifiers::NONE, &mut out);
        assert!(
            matches!(app.overlay, Some(Overlay::Menu(_))),
            "`s` narrows instead of jumping"
        );
        assert!(
            !out.iter()
                .any(|r| matches!(r, ClientRequest::CreateAgent { .. })),
            "filtering must not create anything"
        );
    })
}

/// `?` on a menu with no session rows is ignored, and the kind
/// picker names the jump in its footer.
#[test]
fn question_mark_ignores_menus_without_session_rows() {
    with_default_config(|| {
        let mut app = App::new();
        seed_tree(&mut app);
        app.focus = Focus::Sessions;
        let mut out = Vec::new();

        app.overlay = Some(Overlay::Menu(ContextMenu {
            title: Some("test".into()),
            items: vec![MenuItem::new("x", MenuAction::ToggleArchived)],
            at: None,
            hover: 0,
            area: ratatui::layout::Rect::default(),
            parent: None,
            filter: None,
        }));
        press(&mut app, KeyCode::Char('?'), KeyModifiers::NONE, &mut out);
        assert!(
            matches!(app.overlay, Some(Overlay::Menu(_))),
            "`?` leaves other menus alone"
        );

        // The New session picker itself advertises the jump.
        press(&mut app, KeyCode::Esc, KeyModifiers::NONE, &mut out);
        open_picker(&mut app);
        let mut terminal = Terminal::new(TestBackend::new(100, 40)).unwrap();
        terminal.draw(|f| ui::draw(f, &mut app)).unwrap();
        let text = buffer_text(&terminal);
        assert!(text.contains("s/?: settings"), "footer names it:\n{text}");
    })
}

#[test]
fn picker_enter_on_effort_row_carries_model_and_effort() {
    with_default_config(|| {
        let mut app = App::new();
        seed_tree(&mut app);
        app.focus = Focus::Sessions;
        let mut out = Vec::new();

        // n → Codex row → models → Luna → efforts → minimal → Enter.
        open_picker(&mut app);
        press(&mut app, KeyCode::Down, KeyModifiers::NONE, &mut out);
        press(&mut app, KeyCode::Right, KeyModifiers::NONE, &mut out);
        let Some(Overlay::Menu(menu)) = &app.overlay else {
            panic!("expected codex model submenu");
        };
        assert_eq!(menu.title.as_deref(), Some("Codex model"));
        assert!(menu.items.iter().any(|item| item.label == "gpt-5.6-terra"));
        let luna = menu
            .items
            .iter()
            .position(|item| item.label.starts_with("gpt-5.6-luna"))
            .expect("Luna row");
        for _ in 0..luna {
            press(&mut app, KeyCode::Down, KeyModifiers::NONE, &mut out);
        }
        press(&mut app, KeyCode::Right, KeyModifiers::NONE, &mut out);
        press(&mut app, KeyCode::Down, KeyModifiers::NONE, &mut out);
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE, &mut out);

        assert!(app.overlay.is_none(), "no box: {:?}", app.overlay);
        assert!(
            matches!(
                out.as_slice(),
                [ClientRequest::CreateAgent {
                    kind: AgentKind::Codex,
                    model: Some(m),
                    effort: Some(e),
                    ..
                }] if m == "gpt-5.6-luna" && e == "minimal"
            ),
            "one create, and no Claude refill behind a Codex launch: {out:?}"
        );
    })
}

#[test]
fn picker_resolves_configured_defaults() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.json");
    std::fs::write(
        &path,
        r#"{"claude_model": "sonnet", "claude_effort": "max"}"#,
    )
    .unwrap();
    crate::config::with_config_path(path, || {
        let mut app = App::new();
        seed_tree(&mut app);
        app.focus = Focus::Sessions;
        let mut out = Vec::new();

        // Enter straight on the Claude row: both settings apply.
        open_picker(&mut app);
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE, &mut out);
        assert!(app.overlay.is_none(), "no box: {:?}", app.overlay);
        assert!(
            matches!(
                &out[0],
                ClientRequest::CreateAgent {
                    kind: AgentKind::Claude,
                    model: Some(m),
                    effort: Some(e),
                    ..
                } if m == "sonnet" && e == "max"
            ),
            "{out:?}"
        );
        out.clear();

        // The model submenu highlights and checks the configured model,
        // and its explicit "default" row resolves to the same setting.
        open_picker(&mut app);
        press(&mut app, KeyCode::Right, KeyModifiers::NONE, &mut out);
        let Some(Overlay::Menu(menu)) = &app.overlay else {
            panic!("expected model submenu");
        };
        assert_eq!(menu.items[3].label, "sonnet ✓");
        assert_eq!(menu.hover, 3, "hover starts on the configured model");
        press(&mut app, KeyCode::Up, KeyModifiers::NONE, &mut out);
        press(&mut app, KeyCode::Up, KeyModifiers::NONE, &mut out);
        press(&mut app, KeyCode::Up, KeyModifiers::NONE, &mut out);
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE, &mut out);
        assert!(app.overlay.is_none(), "no box: {:?}", app.overlay);
        assert!(
            matches!(
                &out[0],
                ClientRequest::CreateAgent {
                    model: Some(m),
                    effort: Some(e),
                    ..
                } if m == "sonnet" && e == "max"
            ),
            "{out:?}"
        );
    })
}

#[test]
fn picker_second_row_creates_codex_agent() {
    // The picker reads the harness toggles, so pin the config: a dev
    // whose real config.json hides a kind would shift the rows.
    with_default_config(|| {
        let mut app = App::new();
        seed_tree(&mut app);
        app.focus = Focus::Sessions;
        let mut out = Vec::new();

        open_picker(&mut app);
        for code in [KeyCode::Char('j'), KeyCode::Enter] {
            handle_key(&mut app, KeyEvent::new(code, KeyModifiers::NONE), &mut out);
        }
        assert!(app.overlay.is_none(), "no box: {:?}", app.overlay);
        assert!(matches!(
            out.last(),
            Some(ClientRequest::CreateAgent {
                kind: AgentKind::Codex,
                custom_harness: None,
                ..
            })
        ));
    });
}

#[test]
fn picker_third_row_creates_cursor_agent() {
    with_default_config(|| {
        let mut app = App::new();
        seed_tree(&mut app);
        app.focus = Focus::Sessions;
        let mut out = Vec::new();

        open_picker(&mut app);
        for code in [KeyCode::Char('j'), KeyCode::Char('j'), KeyCode::Enter] {
            handle_key(&mut app, KeyEvent::new(code, KeyModifiers::NONE), &mut out);
        }
        assert!(app.overlay.is_none(), "no box: {:?}", app.overlay);
        assert!(matches!(
            out.last(),
            Some(ClientRequest::CreateAgent {
                kind: AgentKind::Cursor,
                custom_harness: None,
                ..
            })
        ));
    });
}

// ---- REMEMBER HARNESS (Settings → Experimental) ----

/// A row the hovered menu shows, without the `✓` a default row wears.
fn hovered_choice(app: &App) -> String {
    let Some(Overlay::Menu(menu)) = &app.overlay else {
        panic!("expected a menu, got {:?}", app.overlay);
    };
    menu.items[menu.hover]
        .label
        .trim_end_matches(" ✓")
        .to_string()
}

/// With the switch on, the harness picked on `n` — and a model drilled
/// into through `→` — become the AGENTS TAB defaults: the next picker
/// opens on that harness with its `✓` on that model, and the QUICK
/// PROMPT's harness follows.
#[test]
fn a_picker_launch_is_remembered_as_the_next_default_while_the_switch_is_on() {
    with_config_json(r#"{"remember_harness": true}"#, || {
        let mut app = App::new();
        seed_tree(&mut app);
        app.focus = Focus::Sessions;
        let mut out = Vec::new();

        // The harness alone: Codex, second row.
        open_picker(&mut app);
        press(&mut app, KeyCode::Down, KeyModifiers::NONE, &mut out);
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE, &mut out);
        assert!(
            matches!(
                out.first(),
                Some(ClientRequest::CreateAgent {
                    kind: AgentKind::Codex,
                    model: None,
                    ..
                })
            ),
            "{out:?}"
        );
        let cfg = crate::config::Config::load();
        assert_eq!(
            cfg.quick_prompt_kind(),
            AgentKind::Codex,
            "written to the Agents tab"
        );
        assert_eq!(
            cfg.codex_model, "default",
            "no model was picked, so none was written"
        );
        assert!(app.flash.is_none(), "{:?}", app.flash);

        // The next picker opens on it.
        out.clear();
        open_picker(&mut app);
        let Some(Overlay::Menu(menu)) = &app.overlay else {
            panic!("{:?}", app.overlay);
        };
        assert_eq!(menu.items[menu.hover].label, "Codex");

        // A model drilled into on the Claude row lands on Claude's
        // Model row — the pick, not the fallback.
        press(&mut app, KeyCode::Up, KeyModifiers::NONE, &mut out);
        press(&mut app, KeyCode::Right, KeyModifiers::NONE, &mut out);
        let Some(Overlay::Menu(menu)) = &app.overlay else {
            panic!("{:?}", app.overlay);
        };
        assert_eq!(menu.title.as_deref(), Some("Claude model"));
        assert!(menu.items.len() >= 2, "{:?}", menu.items);
        press(&mut app, KeyCode::Down, KeyModifiers::NONE, &mut out);
        let picked = hovered_choice(&app);
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE, &mut out);
        assert!(
            matches!(
                out.first(),
                Some(ClientRequest::CreateAgent {
                    kind: AgentKind::Claude,
                    model: Some(model),
                    ..
                }) if *model == picked
            ),
            "{out:?}"
        );
        let cfg = crate::config::Config::load();
        assert_eq!(cfg.quick_prompt_kind(), AgentKind::Claude);
        assert_eq!(cfg.claude_model, picked, "the Agents tab's Claude model");
        assert_eq!(
            cfg.codex_model, "default",
            "another harness's row is its own"
        );

        // The next picker: Claude, with the ✓ (and the cursor) on it.
        out.clear();
        open_picker(&mut app);
        assert_eq!(hovered_choice(&app), "Claude");
        press(&mut app, KeyCode::Right, KeyModifiers::NONE, &mut out);
        let Some(Overlay::Menu(menu)) = &app.overlay else {
            panic!("{:?}", app.overlay);
        };
        assert_eq!(menu.items[menu.hover].label, format!("{picked} ✓"));
        press(&mut app, KeyCode::Esc, KeyModifiers::NONE, &mut out);
    });
}

/// Off — the default — a pick is one session's: nothing is written,
/// and the next picker opens on its first row as it always has.
#[test]
fn a_picker_launch_is_not_remembered_while_the_switch_is_off() {
    with_default_config(|| {
        let mut app = App::new();
        seed_tree(&mut app);
        app.focus = Focus::Sessions;
        let mut out = Vec::new();

        open_picker(&mut app);
        press(&mut app, KeyCode::Down, KeyModifiers::NONE, &mut out);
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE, &mut out);
        assert!(
            matches!(
                out.first(),
                Some(ClientRequest::CreateAgent {
                    kind: AgentKind::Codex,
                    ..
                })
            ),
            "{out:?}"
        );
        let cfg = crate::config::Config::load();
        assert_eq!(cfg.quick_prompt_kind(), AgentKind::Claude, "untouched");
        assert!(!cfg.remember_harness);

        open_picker(&mut app);
        assert_eq!(hovered_choice(&app), "Claude", "the first row, as ever");
        press(&mut app, KeyCode::Esc, KeyModifiers::NONE, &mut out);
    });
}

/// A QUICK PROMPT fired on a harness picked through `Tab` is a change
/// of default too — recorded when the box fires, not when the picker
/// hands it back — so the next `p` opens on that harness.
#[test]
fn a_quick_prompt_fired_on_a_tab_picked_harness_is_remembered() {
    with_config_json(r#"{"remember_harness": true}"#, || {
        let mut app = App::new();
        seed_tree(&mut app);
        app.focus = Focus::Sessions;
        let mut out = Vec::new();

        press(&mut app, KeyCode::Char('p'), KeyModifiers::NONE, &mut out);
        assert!(paste_into_overlay(&mut app, "Fix auth"));
        press(&mut app, KeyCode::Tab, KeyModifiers::NONE, &mut out);
        assert_eq!(
            hovered_choice(&app),
            "Claude",
            "opens on the box's own harness"
        );
        press(&mut app, KeyCode::Down, KeyModifiers::NONE, &mut out);
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE, &mut out);
        let Some(Overlay::Prompt(prompt)) = &app.overlay else {
            panic!("the pick should hand the box back, got {:?}", app.overlay);
        };
        assert_eq!(prompt.title, "Quick prompt (codex)");
        assert_eq!(
            crate::config::Config::load().quick_prompt_kind(),
            AgentKind::Claude,
            "handing the box back is not a launch: nothing written yet"
        );

        press(&mut app, KeyCode::Enter, KeyModifiers::NONE, &mut out);
        assert!(
            matches!(
                out.first(),
                Some(ClientRequest::CreateAgent {
                    kind: AgentKind::Codex,
                    starting_prompt: Some(text),
                    ..
                }) if text == "Fix auth"
            ),
            "{out:?}"
        );
        assert_eq!(
            crate::config::Config::load().quick_prompt_kind(),
            AgentKind::Codex,
            "the fire is the change of default"
        );

        press(&mut app, KeyCode::Char('p'), KeyModifiers::NONE, &mut out);
        let Some(Overlay::Prompt(prompt)) = &app.overlay else {
            panic!("{:?}", app.overlay);
        };
        assert_eq!(prompt.title, "Quick prompt (codex)", "the next box follows");
        press(&mut app, KeyCode::Esc, KeyModifiers::NONE, &mut out);
    });
}

/// An AGENT PRESET launch is the preset's harness and model, not a
/// change of mind: with the switch on, the AGENTS TAB rows stay put.
#[test]
fn a_preset_launch_is_not_remembered_as_the_default() {
    with_seeded_presets(|| {
        let mut cfg = crate::config::Config::load();
        cfg.remember_harness = true;
        cfg.save().unwrap();

        let mut app = App::new();
        seed_tree(&mut app);
        app.focus = Focus::Sessions;
        let mut out = Vec::new();
        press(&mut app, KeyCode::Char('p'), KeyModifiers::NONE, &mut out);
        assert!(paste_into_overlay(&mut app, "Fix auth"));
        // "reviewer": claude · opus · high, the first preset.
        press(&mut app, KeyCode::BackTab, KeyModifiers::NONE, &mut out);
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE, &mut out);
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE, &mut out);
        assert!(
            matches!(
                out.first(),
                Some(ClientRequest::CreateAgent {
                    kind: AgentKind::Claude,
                    model: Some(model),
                    effort: Some(effort),
                    ..
                }) if model == "opus" && effort == "high"
            ),
            "{out:?}"
        );
        let cfg = crate::config::Config::load();
        assert!(cfg.remember_harness);
        assert_eq!(
            cfg.claude_model, "default",
            "the preset's model stays the preset's"
        );
        assert_eq!(cfg.claude_effort, "default");
        assert_eq!(cfg.quick_prompt_kind(), AgentKind::Claude);
    });
}

// ---- harnesses disabled in Settings leave the picker ----

#[test]
fn picker_omits_a_disabled_harness() {
    with_config_json(r#"{"codex_enabled": false}"#, || {
        let mut app = App::new();
        seed_tree(&mut app);
        app.focus = Focus::Sessions;
        let mut out = Vec::new();

        open_picker(&mut app);
        let Some(Overlay::Menu(menu)) = &app.overlay else {
            panic!("expected the NEW SESSION PICKER, got {:?}", app.overlay);
        };
        let labels: Vec<&str> = menu.items.iter().map(|item| item.label.as_str()).collect();
        assert_eq!(
            labels,
            ["Claude", "Cursor", "Pi", "Muse", "Grok Build", "OpenCode"],
            "Codex is absent, not greyed"
        );

        // The second row is now Cursor: the rows shift, nothing is dead.
        press(&mut app, KeyCode::Char('j'), KeyModifiers::NONE, &mut out);
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE, &mut out);
        assert!(app.overlay.is_none(), "no box: {:?}", app.overlay);
        assert!(
            matches!(
                out.as_slice(),
                [ClientRequest::CreateAgent {
                    kind: AgentKind::Cursor,
                    ..
                }]
            ),
            "{out:?}"
        );
    });
}

#[test]
fn picker_with_every_harness_disabled_flashes_instead_of_opening() {
    with_config_json(
        r#"{"claude_enabled": false, "codex_enabled": false, "cursor_enabled": false, "pi_enabled": false, "muse_enabled": false, "opencode_enabled": false, "harnesses":{"grok":{"enabled":false}}}"#,
        || {
            let mut app = App::new();
            seed_tree(&mut app);
            app.focus = Focus::Sessions;
            let mut out = Vec::new();

            open_picker(&mut app);
            assert!(app.overlay.is_none(), "an empty picker must never open");
            let flash = app.flash.as_deref().expect("a flash says why");
            assert!(flash.contains("Settings"), "{flash}");

            // The CONTEXT MENU's "New agent" row lands on the same guard.
            app.flash = None;
            let worktree = app.selected_worktree().unwrap().id.clone();
            run_menu_action(&mut app, MenuAction::NewAgent(worktree), &mut out);
            assert!(app.overlay.is_none());
            assert!(app.flash.is_some());
        },
    );
}

#[test]
fn disabled_claude_skips_the_standing_prewarm() {
    with_config_json(r#"{"claude_enabled": false}"#, || {
        let mut app = App::new();
        seed_tree(&mut app);
        let worktree = app.selected_worktree().unwrap().id.clone();
        app.pending_prewarm = Some((worktree.clone(), std::time::Instant::now()));
        let mut out = Vec::new();

        fire_pending_prewarm(&mut app, &mut out);
        assert!(
            matches!(
                out.as_slice(),
                [ClientRequest::PrewarmWorktreeSessions { .. }]
            ),
            "dead sessions still prewarm; no Claude WARM SPARE: {out:?}"
        );

        out.clear();
        fire_keepwarm(&mut app, &mut out);
        assert!(
            out.is_empty(),
            "keep-warm sends nothing for a disabled harness: {out:?}"
        );
        assert!(
            app.next_keepwarm.is_some(),
            "still re-armed, so re-enabling warms again"
        );
    });
}

#[test]
fn agents_tab_toggles_a_harness_and_refuses_the_last_one() {
    use crate::config::{locate_agent, HarnessField};
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.json");
    crate::config::with_config_path(path.clone(), || {
        let (tab, claude_row) = locate_agent("claude", HarnessField::Enabled).unwrap();
        let (_, codex_row) = locate_agent("codex", HarnessField::Enabled).unwrap();
        let (_, cursor_row) = locate_agent("cursor", HarnessField::Enabled).unwrap();
        let (_, pi_row) = locate_agent("pi", HarnessField::Enabled).unwrap();
        let (_, muse_row) = locate_agent("muse", HarnessField::Enabled).unwrap();
        let (_, grok_row) = locate_agent("grok", HarnessField::Enabled).unwrap();
        let (_, opencode_row) = locate_agent("opencode", HarnessField::Enabled).unwrap();
        let mut app = App::new();
        let mut out = Vec::new();
        open_settings_on(&mut app, tab, &mut out);
        for _ in 0..claude_row {
            press(&mut app, KeyCode::Down, KeyModifiers::NONE, &mut out);
        }
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE, &mut out);
        let saved: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(saved["claude_enabled"], false, "Enter toggles Claude off");
        assert_eq!(saved["codex_enabled"], true);

        for _ in claude_row..codex_row {
            press(&mut app, KeyCode::Down, KeyModifiers::NONE, &mut out);
        }
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE, &mut out);
        assert!(!crate::config::Config::load().codex_enabled);

        for _ in codex_row..cursor_row {
            press(&mut app, KeyCode::Down, KeyModifiers::NONE, &mut out);
        }
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE, &mut out);
        assert!(!crate::config::Config::load().cursor_enabled);

        for _ in cursor_row..pi_row {
            press(&mut app, KeyCode::Down, KeyModifiers::NONE, &mut out);
        }
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE, &mut out);
        assert!(!crate::config::Config::load().pi_enabled);

        for _ in pi_row..muse_row {
            press(&mut app, KeyCode::Down, KeyModifiers::NONE, &mut out);
        }
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE, &mut out);
        assert!(!crate::config::Config::load().muse_enabled);

        for _ in muse_row..grok_row {
            press(&mut app, KeyCode::Down, KeyModifiers::NONE, &mut out);
        }
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE, &mut out);
        assert!(!crate::config::Config::load().kind_enabled(AgentKind::Grok));

        for _ in grok_row..opencode_row {
            press(&mut app, KeyCode::Down, KeyModifiers::NONE, &mut out);
        }
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE, &mut out);
        let cfg = crate::config::Config::load();
        assert!(
            cfg.opencode_enabled,
            "the last harness cannot be switched off"
        );
        assert_eq!(cfg.enabled_kinds(), vec![AgentKind::OpenCode]);
        let (text, level) = settings_view(&app).notice.clone().expect("a warning");
        assert!(matches!(level, crate::app::NoticeLevel::Warn));
        assert!(text.contains("at least one harness"), "{text}");
        assert!(
            matches!(app.overlay, Some(Overlay::Settings(_))),
            "the refusal keeps the overlay open"
        );
    });
}

#[test]
fn esc_cancels_agent_type_picker() {
    let mut app = App::new();
    seed_tree(&mut app);
    app.focus = Focus::Sessions;
    let mut out = Vec::new();

    open_picker(&mut app);
    assert!(matches!(&app.overlay, Some(Overlay::Menu(_))));
    handle_key(
        &mut app,
        KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
        &mut out,
    );
    assert!(app.overlay.is_none());
    assert!(
        !out.iter()
            .any(|r| matches!(r, ClientRequest::CreateAgent { .. })),
        "cancelled picker must not create anything"
    );
}

#[test]
fn menu_new_agent_action_routes_through_picker() {
    use nebula_core::WorktreeId;
    let mut app = App::new();
    seed_tree(&mut app);
    let mut out = Vec::new();

    run_menu_action(
        &mut app,
        MenuAction::NewAgent(WorktreeId("w1".into())),
        &mut out,
    );
    assert!(matches!(
        &app.overlay,
        Some(Overlay::Menu(m)) if m.title.as_deref() == Some("New session")
    ));
}

fn seed_terminal(app: &mut App, id: &str, name: &str) {
    use nebula_core::{Entity, TerminalId, TerminalTab, WorktreeId};
    hse(
        app,
        ServerEvent::EntityUpserted {
            entity: Entity::Terminal(TerminalTab {
                id: TerminalId(id.into()),
                worktree_id: WorktreeId("w1".into()),
                name: name.into(),
                sort_order: 0,
                alive: true,
                run_command: None,
            }),
        },
    );
}

/// `r` on the Worktrees panel runs the checkout — nothing is renamed
/// there — the Ack names what is running, and `r` again stops it.
#[test]
fn r_on_a_worktree_starts_its_run_and_again_stops_it() {
    use nebula_core::{Entity, TerminalId, TerminalTab, WorktreeId};
    let mut app = App::new();
    seed_tree(&mut app);
    app.focus = Focus::Worktrees;
    let mut out = Vec::new();
    press(&mut app, KeyCode::Char('r'), KeyModifiers::NONE, &mut out);
    let req_id = out
        .iter()
        .find_map(|r| match r {
            ClientRequest::StartRun { req_id, worktree } if worktree.0 == "w1" => Some(*req_id),
            _ => None,
        })
        .unwrap_or_else(|| panic!("r sends StartRun: {out:?}"));
    assert!(app.overlay.is_none(), "no rename prompt on a worktree");
    assert!(!app.worktree_running(&WorktreeId("w1".into())));

    hse(
        &mut app,
        ServerEvent::EntityUpserted {
            entity: Entity::Terminal(TerminalTab {
                id: TerminalId("run1".into()),
                worktree_id: WorktreeId("w1".into()),
                name: "run".into(),
                sort_order: 0,
                alive: true,
                run_command: Some("npm run dev".into()),
            }),
        },
    );
    hse(
        &mut app,
        ServerEvent::Ack {
            req_id,
            created: Some(EntityId::Terminal(TerminalId("run1".into()))),
        },
    );
    assert_eq!(app.flash.as_deref(), Some("▶ running npm run dev in main"));
    assert!(app.worktree_running(&WorktreeId("w1".into())));
    assert_eq!(app.run_flash_when_seen, None);

    out.clear();
    press(&mut app, KeyCode::Char('r'), KeyModifiers::NONE, &mut out);
    assert!(
        out.iter()
            .any(|r| matches!(r, ClientRequest::StopRun { worktree, .. } if worktree.0 == "w1")),
        "r on a running worktree stops it: {out:?}"
    );
}

/// The DAEMON's reply to `StartRun` usually reaches the TUI before the
/// broadcast upsert of the terminal it started: the flash names the
/// branch at once and the command when the row lands.
#[test]
fn a_run_ack_ahead_of_its_terminal_names_the_command_when_it_lands() {
    use nebula_core::{Entity, TerminalTab, WorktreeId};
    let mut app = App::new();
    seed_tree(&mut app);
    app.focus = Focus::Worktrees;
    let mut out = Vec::new();
    press(&mut app, KeyCode::Char('r'), KeyModifiers::NONE, &mut out);
    let req_id = out
        .iter()
        .find_map(|r| match r {
            ClientRequest::StartRun { req_id, .. } => Some(*req_id),
            _ => None,
        })
        .expect("r sends StartRun");
    hse(
        &mut app,
        ServerEvent::Ack {
            req_id,
            created: Some(EntityId::Terminal(TerminalId("run1".into()))),
        },
    );
    assert_eq!(app.flash.as_deref(), Some("▶ running in main"));

    hse(
        &mut app,
        ServerEvent::EntityUpserted {
            entity: Entity::Terminal(TerminalTab {
                id: TerminalId("run1".into()),
                worktree_id: WorktreeId("w1".into()),
                name: "run".into(),
                sort_order: 0,
                alive: true,
                run_command: Some("bun dev".into()),
            }),
        },
    );
    assert_eq!(app.flash.as_deref(), Some("▶ running bun dev in main"));
    assert_eq!(app.run_flash_when_seen, None, "spent");
}

/// `Shift+Enter` on a worktree fires its OPEN COMMAND — the project's
/// **Open command** setting first, else `.nebula.json`'s `open` — and
/// with neither says where to put one, without Enter's drill-in. The
/// key answers from the Sessions panel too, where the cursor's
/// worktree is just as much the context, and `Shift+O` beside it.
#[test]
fn shift_enter_on_a_worktree_fires_its_open_command() {
    use crate::config::SettingKind::OpenCommand;
    let dir = tempfile::tempdir().unwrap();
    let config_path = dir.path().join("config.json");
    crate::config::with_config_path(config_path.clone(), || {
        let mut app = App::new();
        seed_repo_tree(&mut app, dir.path());
        app.focus = Focus::Worktrees;
        let mut out = Vec::new();
        press(&mut app, KeyCode::Enter, KeyModifiers::SHIFT, &mut out);
        let flash = app.flash.clone().unwrap_or_default();
        assert!(
            flash.contains("no open command")
                && flash.contains("Settings")
                && flash.contains(".nebula.json"),
            "names both places: {flash}"
        );
        assert_eq!(app.focus, Focus::Worktrees, "not Enter's drill-in");

        std::fs::write(
            dir.path().join(".nebula.json"),
            r#"{"open": "open http://localhost:3000"}"#,
        )
        .unwrap();
        press(&mut app, KeyCode::Enter, KeyModifiers::SHIFT, &mut out);
        assert_eq!(app.flash.as_deref(), Some("↗ open http://localhost:3000"));

        // The setting wins over the file, and is read fresh per press.
        let mut cfg = crate::config::Config::load();
        assert!(cfg.set_project_text(dir.path(), OpenCommand, "open http://localhost:5173"));
        cfg.save_to(&config_path).unwrap();
        press(&mut app, KeyCode::Enter, KeyModifiers::SHIFT, &mut out);
        assert_eq!(app.flash.as_deref(), Some("↗ open http://localhost:5173"));

        // Sessions panel, Shift+O: the same worktree is under the
        // cursor, so the same command fires, and focus stays put.
        app.focus = Focus::Sessions;
        app.flash = None;
        press(&mut app, KeyCode::Char('O'), KeyModifiers::SHIFT, &mut out);
        assert_eq!(app.flash.as_deref(), Some("↗ open http://localhost:5173"));
        assert_eq!(app.focus, Focus::Sessions);
        // Alt+Enter — the ESC CR a terminal without the kitty protocol
        // sends for a mapped Shift+Enter — is the same key.
        app.flash = None;
        press(&mut app, KeyCode::Enter, KeyModifiers::ALT, &mut out);
        assert_eq!(app.flash.as_deref(), Some("↗ open http://localhost:5173"));
        assert_eq!(app.focus, Focus::Sessions, "still not Enter's drill-in");
        assert!(
            out.is_empty(),
            "opening is the TUI's own doing, never a request"
        );
    });
}

#[test]
fn t_creates_terminal_in_selected_worktree() {
    use nebula_core::WorktreeId;
    let mut app = App::new();
    seed_tree(&mut app);
    app.focus = Focus::Sessions;
    let mut out = Vec::new();

    handle_key(
        &mut app,
        KeyEvent::new(KeyCode::Char('t'), KeyModifiers::NONE),
        &mut out,
    );
    assert!(matches!(
        out.last(),
        Some(ClientRequest::CreateTerminal { worktree, name: None, .. })
            if worktree == &WorktreeId("w1".into())
    ));
}

/// From the Projects panel, `t` targets the project's main checkout
/// (root), not whatever worktree row happens to be selected.
#[test]
fn t_from_projects_targets_the_root_checkout() {
    use nebula_core::{Entity, Worktree, WorktreeId};
    let mut app = App::new();
    seed_tree(&mut app);
    hse(
        &mut app,
        ServerEvent::EntityUpserted {
            entity: Entity::Worktree(Worktree {
                id: WorktreeId("w2".into()),
                project_id: nebula_core::ProjectId("p1".into()),
                path: "/tmp/demo-worktrees/feat".into(),
                branch: "feat".into(),
                is_main: false,
                sort_order: 1,
            }),
        },
    );
    app.sel_worktree = 1; // the feat worktree
    app.focus = Focus::Projects;
    let mut out = Vec::new();

    handle_key(
        &mut app,
        KeyEvent::new(KeyCode::Char('t'), KeyModifiers::NONE),
        &mut out,
    );
    assert!(matches!(
        out.last(),
        Some(ClientRequest::CreateTerminal { worktree, .. })
            if worktree == &WorktreeId("w1".into())
    ));
}

/// The CreateTerminal Ack attaches the new terminal, and its upsert
/// lands the selection on the new row.
#[test]
fn create_terminal_ack_attaches_and_selects_it() {
    use nebula_core::TerminalId;
    let mut app = App::new();
    seed_tree(&mut app);
    app.focus = Focus::Sessions;
    let mut out = Vec::new();

    handle_key(
        &mut app,
        KeyEvent::new(KeyCode::Char('t'), KeyModifiers::NONE),
        &mut out,
    );
    let Some(ClientRequest::CreateTerminal { req_id, .. }) = out.last() else {
        panic!("expected CreateTerminal, got {:?}", out.last());
    };
    let req_id = *req_id;

    // The daemon broadcasts the upsert before it replies with the Ack.
    seed_terminal(&mut app, "t1", "term-1");
    hse(
        &mut app,
        ServerEvent::Ack {
            req_id,
            created: Some(EntityId::Terminal(TerminalId("t1".into()))),
        },
    );
    assert_eq!(
        app.term.as_ref().map(|t| t.sref.clone()),
        Some(SessionRef::Terminal(TerminalId("t1".into())))
    );
    assert_eq!(app.focus, Focus::Terminal);
    assert!(app.term_locked, "a created terminal takes the input lock");
    assert_eq!(app.sel_session, 1, "selection follows the new terminal row");
}

#[test]
fn d_on_terminal_row_confirms_then_closes() {
    use nebula_core::TerminalId;
    let mut app = App::new();
    seed_tree(&mut app);
    seed_terminal(&mut app, "t1", "term-1");
    app.focus = Focus::Sessions;
    app.sel_session = 1;
    let mut out = Vec::new();

    handle_key(
        &mut app,
        KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE),
        &mut out,
    );
    assert!(matches!(
        &app.overlay,
        Some(Overlay::Confirm(c)) if matches!(c.action, PendingAction::CloseTerminal(_))
    ));

    handle_key(
        &mut app,
        KeyEvent::new(KeyCode::Char('y'), KeyModifiers::NONE),
        &mut out,
    );
    assert!(matches!(
        out.last(),
        Some(ClientRequest::CloseTerminal { id, .. }) if id == &TerminalId("t1".into())
    ));
}

#[test]
fn r_on_terminal_row_renames_it() {
    use nebula_core::TerminalId;
    let mut app = App::new();
    seed_tree(&mut app);
    seed_terminal(&mut app, "t1", "term-1");
    app.focus = Focus::Sessions;
    app.sel_session = 1;
    let mut out = Vec::new();

    handle_key(
        &mut app,
        KeyEvent::new(KeyCode::Char('r'), KeyModifiers::NONE),
        &mut out,
    );
    let Some(Overlay::Prompt(p)) = &app.overlay else {
        panic!("expected rename prompt, got {:?}", app.overlay);
    };
    assert_eq!(p.title, "Rename terminal");
    assert_eq!(p.input, "term-1", "prompt starts from the current name");

    handle_key(
        &mut app,
        KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
        &mut out,
    );
    assert!(matches!(
        out.last(),
        Some(ClientRequest::RenameTerminal { id, name, .. })
            if id == &TerminalId("t1".into()) && name == "term-1"
    ));
}

/// The context menu's Delete / Close / Remove rows open the very confirm
/// the `d` key does — same title, same message, same pending action —
/// so the two routes can never drift apart in wording.
#[test]
fn menu_confirms_match_the_key_path_word_for_word() {
    use nebula_core::{ProjectId, TerminalId};
    let mut app = App::new();
    seed_tree(&mut app);
    seed_terminal(&mut app, "t1", "term-1");

    fn take_confirm(app: &mut App) -> ConfirmDialog {
        match app.overlay.take() {
            Some(Overlay::Confirm(c)) => c,
            other => panic!("expected a confirm, got {other:?}"),
        }
    }
    let via_key = |app: &mut App, focus: Focus, row: usize| {
        app.focus = focus;
        app.sel_session = row;
        press(app, KeyCode::Char('d'), KeyModifiers::NONE, &mut Vec::new());
        take_confirm(app)
    };
    let key_agent = via_key(&mut app, Focus::Sessions, 0);
    let key_term = via_key(&mut app, Focus::Sessions, 1);
    let key_project = via_key(&mut app, Focus::Projects, 0);

    let via_menu = |app: &mut App, action: MenuAction| {
        run_menu_action(app, action, &mut Vec::new());
        take_confirm(app)
    };
    let menu_agent = via_menu(&mut app, MenuAction::DeleteAgent(AgentId("a1".into())));
    let menu_term = via_menu(&mut app, MenuAction::CloseTerminal(TerminalId("t1".into())));
    let menu_project = via_menu(&mut app, MenuAction::RemoveProject(ProjectId("p1".into())));

    for (key, menu) in [
        (&key_agent, &menu_agent),
        (&key_term, &menu_term),
        (&key_project, &menu_project),
    ] {
        assert_eq!(key.title, menu.title);
        assert_eq!(key.message, menu.message);
    }
    assert_eq!(key_agent.title, "Delete agent");
    assert_eq!(
        key_agent.message,
        "Delete agent 'agent-1'? Its session and history go away."
    );
    assert!(matches!(menu_agent.action, PendingAction::DeleteAgent(_)));
    assert_eq!(key_term.title, "Close terminal");
    assert_eq!(
        key_term.message,
        "Close terminal 'term-1'? Its shell is killed."
    );
    assert!(matches!(menu_term.action, PendingAction::CloseTerminal(_)));
    assert_eq!(key_project.title, "Remove project");
    assert_eq!(
        key_project.message,
        "Remove 'demo' from nebula? Nothing on disk is touched."
    );
    assert!(matches!(
        menu_project.action,
        PendingAction::RemoveProject(_)
    ));
}

/// A paste into a locked pane whose child turned bracketed paste on
/// reaches the PTY wrapped in the markers, so the child (claude, vim…)
/// takes it as one block instead of keystrokes to auto-indent.
/// Unlocked, it goes nowhere.
#[test]
fn paste_into_a_locked_pane_is_bracketed() {
    let mut app = App::new();
    seed_tree(&mut app);
    let mut out = Vec::new();
    let sref = SessionRef::Agent(AgentId("a1".into()));
    let mut term = AttachedTerm::new(sref.clone(), 80, 24);
    term.apply_output(0, b"\x1b[?2004h");
    app.term = Some(term);
    app.focus = Focus::Terminal;
    app.term_locked = true;

    handle_terminal_event(&mut app, Event::Paste("fn main() {}\n".into()), &mut out);
    match out.as_slice() {
        [ClientRequest::Input { session, data }] => {
            assert_eq!(session, &sref);
            assert_eq!(data, b"\x1b[200~fn main() {}\n\x1b[201~");
        }
        other => panic!("expected one Input request, got {other:?}"),
    }

    out.clear();
    app.term_locked = false;
    handle_terminal_event(&mut app, Event::Paste("x".into()), &mut out);
    assert!(out.is_empty(), "an unlocked pane takes no paste: {out:?}");
}

/// A program that never turned bracketed paste on gets a paste as a
/// terminal sends it, as though typed: a token pasted at a password
/// prompt the shell ran (`hf auth login`) arrives as the token alone,
/// not wrapped in markers that make it a bad one (#107).
#[test]
fn paste_into_a_program_that_never_asked_is_not_bracketed() {
    let mut app = App::new();
    seed_tree(&mut app);
    let mut out = Vec::new();
    let sref = SessionRef::Terminal(TerminalId("t1".into()));
    let mut term = AttachedTerm::new(sref.clone(), 80, 24);
    // The shell's prompt asks for bracketed paste and gives it up again
    // when it runs the command; the command's prompt never asks.
    let screen = b"$ \x1b[?2004huv run hf auth login\r\n\x1b[?2004lEnter your token: ";
    term.apply_output(0, screen);
    app.term = Some(term);
    app.focus = Focus::Terminal;
    app.term_locked = true;

    handle_terminal_event(&mut app, Event::Paste("hf_token".into()), &mut out);
    handle_terminal_event(&mut app, Event::Paste("one\ntwo\r\n".into()), &mut out);
    let sent: Vec<&[u8]> = out
        .iter()
        .map(|r| match r {
            ClientRequest::Input { session, data } if *session == sref => data.as_slice(),
            other => panic!("expected Input to the pane, got {other:?}"),
        })
        .collect();
    assert_eq!(sent, [&b"hf_token"[..], b"one\rtwo\r"]);
}

#[test]
fn escape_hatches_leave_terminal_lock() {
    let mut app = App::new();
    seed_tree(&mut app);
    let mut out = Vec::new();

    let sref = SessionRef::Agent(AgentId("a1".into()));
    app.term = Some(AttachedTerm::new(sref, 80, 24));

    // Ctrl+q plus the fallback: Ctrl+] in both spellings (kitty reports
    // ']', legacy 0x1D parses as Ctrl+5).
    let hatches = [KeyCode::Char('q'), KeyCode::Char(']'), KeyCode::Char('5')];
    for code in hatches {
        app.focus = Focus::Terminal;
        app.term_locked = true;
        handle_key(
            &mut app,
            KeyEvent::new(code, KeyModifiers::CONTROL),
            &mut out,
        );
        assert_eq!(
            app.focus,
            Focus::Sessions,
            "Ctrl+{code:?} leaves terminal input"
        );
        assert!(!app.term_locked, "Ctrl+{code:?} clears the input lock");
        assert!(out.is_empty(), "Ctrl+{code:?} must not reach the pty");
    }

    // Ctrl+Shift+H leaves too. Kitty-protocol emulators only —
    // crossterm may spell it either as 'H' or as shift + 'h', and
    // `from_event` folds both.
    for code in [KeyCode::Char('H'), KeyCode::Char('h')] {
        app.focus = Focus::Terminal;
        app.term_locked = true;
        handle_key(
            &mut app,
            KeyEvent::new(code, KeyModifiers::CONTROL | KeyModifiers::SHIFT),
            &mut out,
        );
        assert_eq!(
            app.focus,
            Focus::Sessions,
            "Ctrl+Shift+{code:?} leaves terminal input"
        );
        assert!(!app.term_locked, "Ctrl+Shift+{code:?} clears the lock");
        assert!(out.is_empty(), "Ctrl+Shift+{code:?} must not reach the pty");
    }

    // Ctrl+Esc and Ctrl+← are hatches no more: Ctrl+← is the agent's
    // word-left, as Ctrl+→ is its word-right. They stay in the
    // session and reach the pty.
    for code in [KeyCode::Esc, KeyCode::Left] {
        app.focus = Focus::Terminal;
        app.term_locked = true;
        out.clear();
        handle_key(
            &mut app,
            KeyEvent::new(code, KeyModifiers::CONTROL),
            &mut out,
        );
        assert_eq!(app.focus, Focus::Terminal, "Ctrl+{code:?} does not escape");
        assert!(app.term_locked, "Ctrl+{code:?} keeps the input lock");
    }
    out.clear();

    // Bare Esc is NOT a hatch: it forwards to the pty untouched — Claude
    // Code owns Esc (interrupt) and double-Esc (clear input / jump back).
    app.focus = Focus::Terminal;
    app.term_locked = true;
    handle_key(
        &mut app,
        KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
        &mut out,
    );
    assert_eq!(app.focus, Focus::Terminal, "Esc stays in the terminal");
    assert!(app.term_locked, "Esc keeps the input lock");
    assert!(
        matches!(out.last(), Some(ClientRequest::Input { data, .. }) if data == b"\x1b"),
        "Esc forwards to the pty immediately"
    );
    out.clear();

    // Cmd+Left is not a hatch: it stays in the terminal (and is
    // swallowed rather than forwarded — no legacy encoding for Super).
    app.focus = Focus::Terminal;
    app.term_locked = true;
    handle_key(
        &mut app,
        KeyEvent::new(KeyCode::Left, KeyModifiers::SUPER),
        &mut out,
    );
    assert_eq!(app.focus, Focus::Terminal, "Cmd+Left does not escape");
    assert!(app.term_locked, "Cmd+Left keeps the input lock");
    assert!(out.is_empty(), "Cmd+Left has no legacy pty encoding");
}

#[test]
fn focus_without_lock_navigates_instead_of_forwarding() {
    let mut app = App::new();
    seed_tree(&mut app);
    let mut out = Vec::new();

    let sref = SessionRef::Agent(AgentId("a1".into()));
    app.term = Some(AttachedTerm::new(sref, 80, 24));
    app.focus = Focus::Terminal; // focused via Tab/arrows — NOT locked

    // The GRID over it takes the arrows; nothing reaches the pty.
    handle_key(
        &mut app,
        KeyEvent::new(KeyCode::Left, KeyModifiers::NONE),
        &mut out,
    );
    assert!(
        !out.iter().any(|r| matches!(r, ClientRequest::Input { .. })),
        "no input to the pty while unlocked: {out:?}"
    );

    // Enter from the sessions panel attaches AND locks.
    handle_key(
        &mut app,
        KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
        &mut out,
    );
    assert_eq!(app.focus, Focus::Terminal);
    assert!(
        app.term_locked,
        "Enter on a session locks input into the terminal"
    );

    // `^q` hands the keys back to the cards.
    handle_key(
        &mut app,
        KeyEvent::new(KeyCode::Char('q'), KeyModifiers::CONTROL),
        &mut out,
    );
    assert_eq!(app.focus, Focus::Sessions);
    assert!(!app.term_locked);
}

/// The other half of the rule above: a session the daemon has reaped
/// still waits out ATTACH_DEBOUNCE, because attaching one cold-spawns
/// an agent CLI and a cursor passing through has not asked for that.
#[test]
fn walking_onto_a_reaped_session_still_waits_out_the_debounce() {
    use nebula_core::{Agent, AgentStatus, Entity, WorktreeId};
    let mut app = App::new();
    seed_tree(&mut app);
    hse(
        &mut app,
        ServerEvent::EntityUpserted {
            entity: Entity::Agent(Agent {
                id: AgentId("a2".into()),
                worktree_id: WorktreeId("w1".into()),
                name: "agent-2".into(),
                status: AgentStatus::Fresh,
                archived: false,
                archived_at: 0,
                unseen: false,
                kind: nebula_core::AgentKind::Claude,
                custom_harness: None,
                model: None,
                effort: None,
                session_id: None,
                cloud_session_id: None,
                sort_order: 1,
                status_changed_at: 0,
                alive: false,
                issue_url: None,
                recent_prompts: Vec::new(),
            }),
        },
    );
    app.focus = Focus::Sessions;
    // The grid's geometry: the step across is by cards on a row, so
    // the body has to be the size a frame would give it.
    app.body_area = ratatui::layout::Rect::new(0, 0, 120, 35);
    let mut out = Vec::new();

    let a2 = SessionRef::Agent(AgentId("a2".into()));
    for code in [KeyCode::Right, KeyCode::Down, KeyCode::Left, KeyCode::Up] {
        press(&mut app, code, KeyModifiers::NONE, &mut out);
        if app.term.as_ref().map(|t| t.sref.clone()) == Some(a2.clone()) {
            break;
        }
    }
    assert_eq!(
        app.term.as_ref().map(|t| t.sref.clone()),
        Some(a2.clone()),
        "the pane still swaps at once"
    );
    assert!(
        !out.iter()
            .any(|r| matches!(r, ClientRequest::Attach { .. })),
        "but a dead session isn't booted by a passing cursor: {out:?}"
    );
    assert_eq!(
        app.pending_attach.as_ref().map(|(s, _)| s.clone()),
        Some(a2.clone()),
        "the attach is armed for the walked-to session"
    );
    fire_pending_attach(&mut app, &mut out);
    assert!(
        matches!(out.last(), Some(ClientRequest::Attach { session, .. }) if *session == a2),
        "settling attaches so the CLI boots: {out:?}"
    );
}

/// Switching worktrees onto a live session attaches it on the keypress
/// — the same instant replay a click gets — so the pane is never left
/// blank for a debounce that exists only to keep a reaped session from
/// being booted by a passing cursor.
#[test]
fn switching_worktrees_onto_a_live_session_attaches_at_once() {
    let mut app = App::new();
    seed_tree(&mut app); // p1/w1(main) + a1 (live)
    hse(
        &mut app,
        ServerEvent::EntityUpserted {
            entity: wt_entity("w2", "p1", "other", false),
        },
    );
    hse(
        &mut app,
        ServerEvent::EntityUpserted {
            entity: agent_entity("a2", "w2", "agent-2", false),
        },
    );
    let a2 = SessionRef::Agent(AgentId("a2".into()));
    let mut out = Vec::new();
    attach_now(&mut app, SessionRef::Agent(AgentId("a1".into())), &mut out);
    out.clear();

    select_worktree_row(&mut app, 1, &mut out);
    assert!(
        app.pending_attach.is_none(),
        "a live session needs no debounce: {out:?}"
    );
    assert!(
        matches!(out.last(), Some(ClientRequest::Attach { session, .. }) if *session == a2),
        "the switch attaches the worktree's session at once: {out:?}"
    );
    assert!(
        !app.term.as_ref().expect("pane").booting,
        "nothing is booting, so the pane shows no notice while the replay lands"
    );
}

/// The other half: a worktree whose session the daemon reaped still
/// waits out ATTACH_DEBOUNCE before attaching, because that attach
/// cold-spawns an agent CLI — and the pane says so meanwhile.
#[test]
fn switching_worktrees_onto_a_reaped_session_waits_out_the_debounce() {
    let mut app = App::new();
    seed_tree(&mut app);
    hse(
        &mut app,
        ServerEvent::EntityUpserted {
            entity: wt_entity("w2", "p1", "other", false),
        },
    );
    hse(
        &mut app,
        ServerEvent::EntityUpserted {
            entity: agent_entity("a2", "w2", "agent-2", false),
        },
    );
    app.tree
        .agents
        .iter_mut()
        .find(|a| a.id == AgentId("a2".into()))
        .expect("a2 seeded")
        .alive = false;
    let a2 = SessionRef::Agent(AgentId("a2".into()));
    let mut out = Vec::new();

    select_worktree_row(&mut app, 1, &mut out);
    assert!(
        !out.iter()
            .any(|r| matches!(r, ClientRequest::Attach { .. })),
        "a passing cursor boots nothing: {out:?}"
    );
    assert_eq!(
        app.pending_attach.as_ref().map(|(s, _)| s.clone()),
        Some(a2.clone()),
        "the attach is armed for when the cursor settles"
    );
    assert!(
        app.term.as_ref().expect("pane").booting,
        "the pane says the session is booting while it waits"
    );
    fire_pending_attach(&mut app, &mut out);
    assert!(
        matches!(out.last(), Some(ClientRequest::Attach { session, .. }) if *session == a2),
        "settling attaches so the CLI boots: {out:?}"
    );
}

/// Coming back to a session shown a moment ago puts its last screen up
/// on the same frame and asks the daemon only for the bytes it missed.
/// A replay continuing from there lands on the kept screen; one from
/// anywhere else — the ring wrapped past what was seen, or a new
/// process — rebuilds it.
/// A session the DAEMON refuses to start (its checkout was deleted
/// outside nebula) must not leave the pane booting forever: the pane
/// keeps the full reason, the status line its first sentence, and a
/// later attach that does start the session clears both. A refusal of
/// some other session leaves the pane on screen alone.
#[test]
fn a_refused_attach_tells_the_pane_why_until_the_session_starts() {
    let mut app = App::new();
    seed_tree(&mut app);
    let a1 = SessionRef::Agent(AgentId("a1".into()));
    let mut out = Vec::new();
    attach_now(&mut app, a1.clone(), &mut out);
    let why = "the checkout for 'feat' is gone from disk (/x/feat). Recreate it where it was";

    hse(
        &mut app,
        ServerEvent::AttachRefused {
            session: SessionRef::Agent(AgentId("other".into())),
            message: "elsewhere".into(),
        },
    );
    assert_eq!(app.term.as_ref().unwrap().refused, None, "not this pane's");

    hse(
        &mut app,
        ServerEvent::AttachRefused {
            session: a1.clone(),
            message: why.into(),
        },
    );
    assert_eq!(app.term.as_ref().unwrap().refused.as_deref(), Some(why));
    assert_eq!(
        app.flash.as_deref(),
        Some("couldn't start this session: the checkout for 'feat' is gone from disk (/x/feat)")
    );

    // Away and back by mouse (no key press clears the line): the pane
    // is rebuilt, and the line must still clear when the session starts.
    hse(
        &mut app,
        ServerEvent::EntityUpserted {
            entity: agent_entity("a2", "w1", "agent-2", false),
        },
    );
    attach_now(&mut app, SessionRef::Agent(AgentId("a2".into())), &mut out);
    attach_now(&mut app, a1.clone(), &mut out);
    hse(
        &mut app,
        ServerEvent::Scrollback {
            session: a1.clone(),
            base_seq: 0,
            data: b"back".to_vec(),
        },
    );
    assert_eq!(
        app.term.as_ref().unwrap().refused,
        None,
        "started after all"
    );
    assert_eq!(app.flash, None, "the refusal no longer stands");
}

#[test]
fn returning_to_a_session_keeps_its_screen_and_asks_for_the_delta() {
    let mut app = App::new();
    seed_tree(&mut app);
    hse(
        &mut app,
        ServerEvent::EntityUpserted {
            entity: agent_entity("a2", "w1", "agent-2", false),
        },
    );
    let a1 = SessionRef::Agent(AgentId("a1".into()));
    let a2 = SessionRef::Agent(AgentId("a2".into()));
    let mut out = Vec::new();
    attach_now(&mut app, a1.clone(), &mut out);
    hse(
        &mut app,
        ServerEvent::Scrollback {
            session: a1.clone(),
            base_seq: 0,
            data: b"hello".to_vec(),
        },
    );

    // Away, and back.
    attach_now(&mut app, a2.clone(), &mut out);
    assert_eq!(
        app.term_cache
            .iter()
            .map(|t| t.sref.clone())
            .collect::<Vec<_>>(),
        std::slice::from_ref(&a1),
        "the screen left behind is kept"
    );
    out.clear();
    attach_now(&mut app, a1.clone(), &mut out);
    let term = app.term.as_ref().expect("pane");
    let contents = term.parser.screen().contents();
    assert!(
        term.painted && contents.contains("hello"),
        "the kept screen is up before the daemon answers: {contents:?}"
    );
    assert!(
        matches!(
            out.last(),
            Some(ClientRequest::Attach { session, from_seq: Some(5), .. }) if *session == a1
        ),
        "the attach asks for what it missed: {out:?}"
    );
    assert!(
        app.term_cache.is_empty(),
        "the screen is back in the pane, not the cache"
    );

    // A continuation lands on the kept screen.
    hse(
        &mut app,
        ServerEvent::Scrollback {
            session: a1.clone(),
            base_seq: 5,
            data: b" world".to_vec(),
        },
    );
    let contents = app.term.as_ref().expect("pane").parser.screen().contents();
    assert!(contents.contains("hello world"), "{contents:?}");

    // Anything else starts the screen over.
    hse(
        &mut app,
        ServerEvent::Scrollback {
            session: a1.clone(),
            base_seq: 100,
            data: b"fresh".to_vec(),
        },
    );
    let term = app.term.as_ref().expect("pane");
    let contents = term.parser.screen().contents();
    assert!(
        contents.contains("fresh") && !contents.contains("hello"),
        "{contents:?}"
    );
    assert_eq!(term.next_seq, 105, "and the delta point follows the replay");
}

/// A kept screen is shown only for a session the daemon still holds:
/// one that died comes back as a new process with a ring of its own,
/// and its old screen would mislead for the frame before the replay.
#[test]
fn a_session_that_died_is_not_shown_from_the_cache() {
    let mut app = App::new();
    seed_tree(&mut app);
    hse(
        &mut app,
        ServerEvent::EntityUpserted {
            entity: agent_entity("a2", "w1", "agent-2", false),
        },
    );
    let a1 = SessionRef::Agent(AgentId("a1".into()));
    let a2 = SessionRef::Agent(AgentId("a2".into()));
    let mut out = Vec::new();
    attach_now(&mut app, a1.clone(), &mut out);
    hse(
        &mut app,
        ServerEvent::Scrollback {
            session: a1.clone(),
            base_seq: 0,
            data: b"hello".to_vec(),
        },
    );
    attach_now(&mut app, a2.clone(), &mut out);
    assert_eq!(app.term_cache.len(), 1);

    app.tree
        .agents
        .iter_mut()
        .find(|a| a.id == AgentId("a1".into()))
        .expect("a1 seeded")
        .alive = false;
    out.clear();
    attach_now(&mut app, a1.clone(), &mut out);
    assert!(
        !app.term.as_ref().expect("pane").painted,
        "a fresh, blank pane"
    );
    assert!(
        matches!(
            out.last(),
            Some(ClientRequest::Attach { from_seq: None, .. })
        ),
        "and a whole-ring replay is asked for: {out:?}"
    );
    assert!(
        app.term_cache.is_empty(),
        "the dead session's screen is gone"
    );
}

/// The cache holds the last few screens shown, most recent first; one
/// older than that is re-parsed on the way back like a first visit.
#[test]
fn the_screen_cache_keeps_the_last_few_sessions() {
    let mut app = App::new();
    seed_tree(&mut app);
    let ids: Vec<String> = (1..=crate::app::TERM_CACHE_MAX + 3)
        .map(|n| format!("a{n}"))
        .collect();
    for id in &ids[1..] {
        hse(
            &mut app,
            ServerEvent::EntityUpserted {
                entity: agent_entity(id, "w1", id, false),
            },
        );
    }
    let mut out = Vec::new();
    for id in &ids {
        let sref = SessionRef::Agent(AgentId(id.clone()));
        attach_now(&mut app, sref.clone(), &mut out);
        hse(
            &mut app,
            ServerEvent::Scrollback {
                session: sref,
                base_seq: 0,
                data: id.as_bytes().to_vec(),
            },
        );
    }
    // Everything but the one in the pane has been stashed; the newest
    // TERM_CACHE_MAX of those are what is left, newest first.
    let kept: Vec<SessionRef> = ids[..ids.len() - 1]
        .iter()
        .rev()
        .take(crate::app::TERM_CACHE_MAX)
        .map(|id| SessionRef::Agent(AgentId(id.clone())))
        .collect();
    assert_eq!(
        app.term_cache
            .iter()
            .map(|t| t.sref.clone())
            .collect::<Vec<_>>(),
        kept,
        "the newest few, newest first; the oldest were evicted"
    );
}

/// A pane with `lines` lines of shell history above a prompt, attached
/// and painted, as the DAEMON's replay leaves it.
fn pane_with_history(app: &mut App, sref: &SessionRef, lines: usize) {
    let mut out = Vec::new();
    attach_now(app, sref.clone(), &mut out);
    let mut data = Vec::new();
    for n in 0..lines {
        data.extend_from_slice(format!("line {n}\r\n").as_bytes());
    }
    data.extend_from_slice(b"$ ");
    hse(
        app,
        ServerEvent::Scrollback {
            session: sref.clone(),
            base_seq: 0,
            data,
        },
    );
}

/// A screen whose history is over the budget used not to be kept, so
/// every return to a long-running shell re-parsed its whole ring. It is
/// kept now, without the history: the return paints what was on screen
/// on the keypress and asks only for what it missed.
#[test]
fn a_long_history_is_let_go_and_the_screen_still_returns_at_once() {
    let mut app = App::new();
    seed_tree(&mut app);
    hse(
        &mut app,
        ServerEvent::EntityUpserted {
            entity: agent_entity("a2", "w1", "agent-2", false),
        },
    );
    let a1 = SessionRef::Agent(AgentId("a1".into()));
    let a2 = SessionRef::Agent(AgentId("a2".into()));
    // Enough rows that the grid alone is over the whole budget.
    let cols = pane_size(&app).0 as usize;
    let lines = crate::app::TERM_CACHE_CELLS / cols + 50;
    pane_with_history(&mut app, &a1, lines);
    let seen = app.term.as_ref().expect("pane").next_seq;

    let mut out = Vec::new();
    attach_now(&mut app, a2.clone(), &mut out);
    let kept = app.term_cache.first().expect("a1's screen is kept");
    assert!(kept.history_dropped, "without its history");
    assert_eq!(kept.parser.screen().scrollback_rows(), 0);
    assert!(
        kept.estimated_cells() <= crate::app::TERM_CACHE_CELLS,
        "which is what fits it in the budget"
    );

    out.clear();
    attach_now(&mut app, a1.clone(), &mut out);
    let pane = app.term.as_ref().expect("pane");
    assert!(pane.painted, "the screen is up on this frame");
    assert!(
        pane.parser.screen().contents().contains("$ "),
        "as it was left: {:?}",
        pane.parser.screen().contents()
    );
    assert!(
        matches!(
            out.last(),
            Some(ClientRequest::Attach { from_seq: Some(n), .. }) if *n == seen
        ),
        "and only what it missed is asked for: {out:?}"
    );
}

/// Scrolling up in a pane whose history was let go brings it back: the
/// attachment is dropped and re-made for the whole ring, and the replay
/// lands the reader on the notch that asked.
#[test]
fn scrolling_up_replays_a_history_that_was_let_go() {
    let mut app = App::new();
    seed_tree(&mut app);
    let a1 = SessionRef::Agent(AgentId("a1".into()));
    pane_with_history(&mut app, &a1, 200);
    let end = app.term.as_ref().expect("pane").next_seq;
    app.term.as_mut().expect("pane").drop_history();
    assert!(app.term.as_ref().expect("pane").history_dropped);

    let mut out = Vec::new();
    rehydrate_history(&mut app, 3, &mut out);
    assert!(
        matches!(
            out.as_slice(),
            [
                ClientRequest::Detach { session },
                ClientRequest::Attach { from_seq: None, .. }
            ] if *session == a1
        ),
        "let go, then the whole ring: {out:?}"
    );

    // The whole ring again, as the DAEMON replays it.
    let mut data = Vec::new();
    for n in 0..200 {
        data.extend_from_slice(format!("line {n}\r\n").as_bytes());
    }
    data.extend_from_slice(b"$ ");
    assert_eq!(data.len() as u64, end);
    hse(
        &mut app,
        ServerEvent::Scrollback {
            session: a1.clone(),
            base_seq: 0,
            data,
        },
    );
    let pane = app.term.as_ref().expect("pane");
    assert!(!pane.history_dropped && pane.pending_scroll.is_none());
    assert!(
        pane.parser.screen().scrollback_rows() > 100,
        "history is back"
    );
    assert_eq!(
        pane.scroll_offset(),
        3,
        "and the reader is where the wheel was headed"
    );

    // A second notch is an ordinary scroll: nothing more is asked.
    out.clear();
    rehydrate_history(&mut app, 6, &mut out);
    assert!(out.is_empty(), "{out:?}");
}

/// Output the replay already covered is not parsed a second time: a
/// frame the replaced attachment's forwarder had queued can cross the
/// replay that supersedes it.
#[test]
fn output_a_replay_covered_is_skipped() {
    let sref = SessionRef::Agent(AgentId("a1".into()));
    let mut term = AttachedTerm::new(sref, 40, 10);
    term.apply_scrollback(0, b"hello world");
    // Wholly covered.
    term.apply_output(6, b"world");
    // Half covered: only the tail is new.
    term.apply_output(9, b"ld!");
    assert_eq!(term.next_seq, 12);
    assert!(
        term.parser.screen().contents().starts_with("hello world!"),
        "{:?}",
        term.parser.screen().contents()
    );
    assert!(!term.parser.screen().contents().contains("worldworld"));
}

/// A session that leaves the tree takes its kept screen with it.
#[test]
fn a_removed_session_leaves_the_screen_cache() {
    let mut app = App::new();
    seed_tree(&mut app);
    hse(
        &mut app,
        ServerEvent::EntityUpserted {
            entity: agent_entity("a2", "w1", "agent-2", false),
        },
    );
    let a1 = SessionRef::Agent(AgentId("a1".into()));
    let a2 = SessionRef::Agent(AgentId("a2".into()));
    let mut out = Vec::new();
    attach_now(&mut app, a1.clone(), &mut out);
    hse(
        &mut app,
        ServerEvent::Scrollback {
            session: a1.clone(),
            base_seq: 0,
            data: b"hello".to_vec(),
        },
    );
    attach_now(&mut app, a2.clone(), &mut out);
    assert_eq!(app.term_cache.len(), 1);
    hse(
        &mut app,
        ServerEvent::EntityRemoved {
            id: nebula_core::EntityId::Agent(AgentId("a1".into())),
        },
    );
    assert!(
        app.term_cache.is_empty(),
        "the removed session's screen is gone"
    );
}

fn archived_agent(id: &str, name: &str, archived_at: i64, sort: i64) -> nebula_core::Entity {
    use nebula_core::{Agent, AgentStatus, Entity, WorktreeId};
    Entity::Agent(Agent {
        id: AgentId(id.into()),
        worktree_id: WorktreeId("w1".into()),
        name: name.into(),
        status: AgentStatus::Fresh,
        archived: true,
        unseen: false,
        archived_at,
        kind: nebula_core::AgentKind::Claude,
        custom_harness: None,
        model: None,
        effort: None,
        session_id: None,
        cloud_session_id: None,
        sort_order: sort,
        status_changed_at: 0,
        alive: false,
        issue_url: None,
        recent_prompts: Vec::new(),
    })
}

/// The ARCHIVED group lists the most recently archived session first;
/// never-stamped legacy rows (archived_at == 0) sink to the bottom.
#[test]
fn archived_group_orders_newest_first() {
    let mut app = App::new();
    seed_tree(&mut app);
    for ev in [
        archived_agent("old", "old", 100, 1),
        archived_agent("newest", "newest", 300, 2),
        archived_agent("mid", "mid", 200, 3),
        archived_agent("legacy", "legacy", 0, 4),
    ] {
        hse(&mut app, ServerEvent::EntityUpserted { entity: ev });
    }
    app.show_archived = true;
    let names: Vec<String> = app
        .visible_session_rows()
        .iter()
        .filter(|r| r.is_archived_agent())
        .map(|r| match r {
            SessionRow::Agent(a) => a.name.clone(),
            SessionRow::Terminal(_) | SessionRow::Link(_) => unreachable!(),
        })
        .collect();
    assert_eq!(names, ["newest", "mid", "old", "legacy"]);
}

/// Collapsing the ARCHIVED group (A) while the cursor sits on an
/// archived row re-lands it on a surviving row instead of leaving it
/// dangling past the end of the list.
#[test]
fn collapsing_archived_relands_the_cursor() {
    let mut app = App::new();
    seed_tree(&mut app);
    hse(
        &mut app,
        ServerEvent::EntityUpserted {
            entity: archived_agent("a9", "old-agent", 100, 9),
        },
    );
    app.show_archived = true;
    app.focus = Focus::Sessions;
    let mut out = Vec::new();
    press(&mut app, KeyCode::Down, KeyModifiers::NONE, &mut out);
    assert!(
        app.selected_session_row()
            .is_some_and(|r| r.is_archived_agent()),
        "cursor sits on the archived row"
    );

    press(&mut app, KeyCode::Char('A'), KeyModifiers::SHIFT, &mut out);
    assert!(!app.show_archived, "A collapses the group");
    assert_eq!(
        app.selected_session().map(|a| a.name),
        Some("agent-1".into()),
        "cursor lands on a surviving row"
    );
}

/// The ARCHIVED collapse is one global switch, not a per-worktree one:
/// collapsing it under one worktree hides the archived rows of every
/// other worktree too, walking the cursor across worktrees never flips
/// it back, and it round-trips through the persisted UI state as a
/// single flag.
#[test]
fn archived_collapse_is_global_across_worktrees() {
    let mut app = App::new();
    seed_tree(&mut app); // p1 / w1(main) / a1
    for entity in [
        wt_entity("w2", "p1", "feature", false),
        agent_entity("a2", "w2", "agent-2", false),
        archived_agent("old-1", "old-1", 100, 5), // on w1
        agent_entity("old-2", "w2", "old-2", true),
    ] {
        hse(&mut app, ServerEvent::EntityUpserted { entity });
    }
    app.show_archived = true;
    let mut out = Vec::new();
    let w1 = WorktreeId("w1".into());
    let w2 = WorktreeId("w2".into());
    let archived_visible = |app: &App| app.visible_sessions().iter().any(|a| a.archived);
    assert!(
        archived_visible(&app),
        "expanded: w1 lists its archived row"
    );

    // Collapse under w1.
    app.focus = Focus::Sessions;
    press(&mut app, KeyCode::Char('A'), KeyModifiers::SHIFT, &mut out);
    assert!(!app.show_archived, "A collapses the group");
    assert!(
        !archived_visible(&app),
        "collapsed: w1 hides its archived row"
    );

    // Walk to w2: still collapsed there, the header still counts the row.
    app.focus = Focus::Worktrees;
    move_selection(&mut app, 1, &mut out);
    assert_eq!(app.selected_worktree().map(|w| w.id.clone()), Some(w2));
    assert!(!app.show_archived, "a worktree switch never re-expands");
    assert!(
        !archived_visible(&app),
        "collapsed: w2 hides its archived row too"
    );
    assert_eq!(
        app.session_group_counts(),
        (1, 1),
        "the header still counts w2's archived row"
    );

    // Expand under w2, walk back to w1: expanded there as well.
    app.focus = Focus::Sessions;
    press(&mut app, KeyCode::Char('A'), KeyModifiers::SHIFT, &mut out);
    assert!(archived_visible(&app), "expanded under w2");
    app.focus = Focus::Worktrees;
    move_selection(&mut app, -1, &mut out);
    assert_eq!(app.selected_worktree().map(|w| w.id.clone()), Some(w1));
    assert!(app.show_archived, "a worktree switch never re-collapses");
    assert!(archived_visible(&app), "expanded under w1 again");

    // One flag in the persisted blob, restored the same for every worktree.
    let json = ui_state_json(&app);
    assert!(json.contains(r#""show_archived":true"#), "{json}");
    let mut restored = App::new();
    seed_tree(&mut restored);
    restore_ui_state(&mut restored, &json);
    assert!(restored.show_archived);
}

#[test]
fn drag_selection_selects_and_extracts_text() {
    let mut app = App::new();
    seed_tree(&mut app);
    let mut out = Vec::new();

    let sref = SessionRef::Agent(AgentId("a1".into()));
    let mut term = AttachedTerm::new(sref, 80, 24);
    term.parser.process(b"hello world");
    app.term = Some(term);
    app.term_area = ratatui::layout::Rect::new(0, 0, 80, 24);
    app.hits.push((app.term_area, HitTarget::TerminalPane));

    let ev = |kind, column, row| MouseEvent {
        kind,
        column,
        row,
        modifiers: KeyModifiers::NONE,
    };

    // Mouse-down on the pane arms an (inactive) selection and locks input.
    handle_mouse(
        &mut app,
        ev(MouseEventKind::Down(MouseButton::Left), 0, 0),
        &mut out,
    );
    assert!(app.term_selection.is_some_and(|s| s.dragging && !s.active));
    assert!(app.term_locked, "click into the pane still locks input");

    // Dragging extends the selection; the text under it is extractable.
    handle_mouse(
        &mut app,
        ev(MouseEventKind::Drag(MouseButton::Left), 10, 0),
        &mut out,
    );
    let sel = app.term_selection.expect("drag keeps the selection");
    assert!(
        sel.active,
        "leaving the anchor cell activates the selection"
    );
    assert_eq!(sel.bounds(), ((0, 0), (10, 0)));
    assert_eq!(selection_text(&app).as_deref(), Some("hello world"));

    // A drag that wanders outside the pane clamps to the nearest edge.
    handle_mouse(
        &mut app,
        ev(MouseEventKind::Drag(MouseButton::Left), 200, 50),
        &mut out,
    );
    assert_eq!(app.term_selection.expect("still selecting").head, (79, 23));

    // Mouse-up copies AND keeps the highlight (dragging over).
    handle_mouse(
        &mut app,
        ev(MouseEventKind::Up(MouseButton::Left), 200, 50),
        &mut out,
    );
    let sel = app
        .term_selection
        .expect("highlight persists after release");
    assert!(!sel.dragging && sel.active);
    assert!(
        app.flash
            .as_deref()
            .is_some_and(|f| f.starts_with("copied")),
        "release copies the selection"
    );
    assert!(
        selection_text(&app).is_some(),
        "persisted selection is still extractable"
    );

    // A fresh click outside the pane clears the highlight.
    app.hits.clear();
    handle_mouse(
        &mut app,
        ev(MouseEventKind::Down(MouseButton::Left), 0, 0),
        &mut out,
    );
    assert!(
        app.term_selection.is_none(),
        "click elsewhere clears the selection"
    );
}

#[test]
fn plain_click_without_drag_leaves_no_selection() {
    let mut app = App::new();
    seed_tree(&mut app);
    let mut out = Vec::new();

    let sref = SessionRef::Agent(AgentId("a1".into()));
    let mut term = AttachedTerm::new(sref, 80, 24);
    term.parser.process(b"hello world");
    app.term = Some(term);
    app.term_area = ratatui::layout::Rect::new(0, 0, 80, 24);
    app.hits.push((app.term_area, HitTarget::TerminalPane));

    handle_mouse(
        &mut app,
        mev(MouseEventKind::Down(MouseButton::Left), 3, 0),
        &mut out,
    );
    handle_mouse(
        &mut app,
        mev(MouseEventKind::Up(MouseButton::Left), 3, 0),
        &mut out,
    );
    assert!(
        app.term_selection.is_none(),
        "a click that never dragged is not a selection"
    );
    assert!(app.flash.is_none(), "nothing was copied");
}

/// Twenty numbered lines into a five-row pane whose top row is host
/// row `y`: lines 0–14 in the scrollback, 15–19 on screen, and history
/// line `n` reads `line n`. The PTY cursor is hidden so its cell never
/// reads as a highlight.
fn twenty_line_pane(app: &mut App, y: u16) {
    let sref = SessionRef::Agent(AgentId("a1".into()));
    let mut term = AttachedTerm::new(sref, 80, 5);
    let lines: Vec<String> = (0..20).map(|i| format!("line {i}")).collect();
    term.parser.process(b"\x1b[?25l");
    term.parser.process(lines.join("\r\n").as_bytes());
    app.term = Some(term);
    app.term_area = ratatui::layout::Rect::new(0, y, 80, 5);
    app.hits.push((app.term_area, HitTarget::TerminalPane));
}

/// `line from` through `line to`, one per row, as a copy reads back.
fn lines_text(from: usize, to: usize) -> String {
    (from..=to)
        .map(|i| format!("line {i}"))
        .collect::<Vec<_>>()
        .join("\n")
}

fn history_base(app: &App) -> u64 {
    app.term.as_ref().unwrap().parser.screen().history_base()
}

fn bounds(app: &App) -> ((u16, u64), (u16, u64)) {
    app.term_selection.expect("a selection").bounds()
}

/// A drag under way is anchored to the text, not to screen rows: the
/// agent printing on and the wheel both move rows under the pointer,
/// and the selection stays on the rows it started on, reading back
/// whole at release. A finished selection still goes with the wheel.
#[test]
fn a_drag_is_pinned_to_its_text_through_new_output_and_the_wheel() {
    let mut app = App::new();
    seed_tree(&mut app);
    let mut out = Vec::new();
    twenty_line_pane(&mut app, 1);
    assert_eq!(history_base(&app), 15);

    // Press on `line 17` (host row 3), drag to the end of `line 18`.
    handle_mouse(
        &mut app,
        mev(MouseEventKind::Down(MouseButton::Left), 0, 3),
        &mut out,
    );
    handle_mouse(
        &mut app,
        mev(MouseEventKind::Drag(MouseButton::Left), 6, 4),
        &mut out,
    );
    assert_eq!(bounds(&app), ((0, 17), (6, 18)));
    assert_eq!(
        selection_text(&app).as_deref(),
        Some(lines_text(17, 18).as_str())
    );

    // Two more lines arrive mid-drag and push everything up two rows:
    // the selection still names lines 17–18, and the next drag report
    // — same host row — reads the text now under the pointer.
    app.term
        .as_mut()
        .unwrap()
        .parser
        .process(b"\r\nline 20\r\nline 21");
    assert_eq!(history_base(&app), 17);
    assert_eq!(bounds(&app), ((0, 17), (6, 18)));
    assert_eq!(
        selection_text(&app).as_deref(),
        Some(lines_text(17, 18).as_str())
    );
    handle_mouse(
        &mut app,
        mev(MouseEventKind::Drag(MouseButton::Left), 6, 4),
        &mut out,
    );
    assert_eq!(bounds(&app), ((0, 17), (6, 20)));

    // The wheel mid-drag scrolls the view; the selection rides along.
    handle_mouse(&mut app, mev(MouseEventKind::ScrollUp, 40, 3), &mut out);
    assert_eq!(app.term.as_ref().unwrap().scroll_offset(), 1);
    assert_eq!(
        bounds(&app),
        ((0, 17), (6, 20)),
        "the wheel keeps a drag on its text"
    );

    // The release copies every row the drag crossed, on screen or not.
    handle_mouse(
        &mut app,
        mev(MouseEventKind::Up(MouseButton::Left), 6, 4),
        &mut out,
    );
    assert_eq!(
        selection_text(&app).as_deref(),
        Some(lines_text(17, 20).as_str())
    );
    assert!(app
        .flash
        .as_deref()
        .is_some_and(|f| f.starts_with("copied")));

    // A finished selection goes with the wheel, as it always has.
    handle_mouse(&mut app, mev(MouseEventKind::ScrollUp, 40, 3), &mut out);
    assert!(app.term_selection.is_none());
}

/// Dragging past the pane's top edge — onto the rule above it — scrolls
/// the history under the pointer: one step at once, then a step per
/// EDGE AUTO-SCROLL tick, faster the further past the edge the pointer
/// rests, with the head on the edge row's new text each time. Back
/// inside, the beat stops; at the top of the history, it moves nothing.
#[test]
fn dragging_past_the_top_edge_scrolls_the_history_under_the_pointer() {
    let mut app = App::new();
    seed_tree(&mut app);
    let mut out = Vec::new();
    // Pane at host rows 3–7: rows 0–2 are past its top edge.
    twenty_line_pane(&mut app, 3);
    let scroll = |app: &App| app.term.as_ref().unwrap().scroll_offset();

    // Press at the end of `line 17`, drag to the pane's top row:
    // inside, so no beat.
    handle_mouse(
        &mut app,
        mev(MouseEventKind::Down(MouseButton::Left), 79, 5),
        &mut out,
    );
    handle_mouse(
        &mut app,
        mev(MouseEventKind::Drag(MouseButton::Left), 0, 3),
        &mut out,
    );
    assert_eq!(bounds(&app), ((0, 15), (79, 17)));
    assert!(
        app.next_drag_autoscroll.is_none(),
        "the top row is inside the pane"
    );
    assert_eq!(scroll(&app), 0);

    // One row past the edge: a step now — one line — and the beat is on.
    handle_mouse(
        &mut app,
        mev(MouseEventKind::Drag(MouseButton::Left), 0, 2),
        &mut out,
    );
    assert_eq!(scroll(&app), 1);
    assert_eq!(
        bounds(&app),
        ((0, 14), (79, 17)),
        "the head is on the edge row's new text"
    );
    assert!(app.next_drag_autoscroll.is_some());

    // A tick: another line, with no mouse report in between.
    drag_autoscroll_tick(&mut app, &mut out);
    assert_eq!(scroll(&app), 2);
    assert_eq!(bounds(&app), ((0, 13), (79, 17)));
    assert!(app.next_drag_autoscroll.is_some());

    // Three rows past the edge: three lines a tick.
    handle_mouse(
        &mut app,
        mev(MouseEventKind::Drag(MouseButton::Left), 0, 0),
        &mut out,
    );
    assert_eq!(
        scroll(&app),
        2,
        "a report while the beat runs is not a step of its own"
    );
    drag_autoscroll_tick(&mut app, &mut out);
    assert_eq!(scroll(&app), 5);
    assert_eq!(bounds(&app), ((0, 10), (79, 17)));

    // The top of the history: ticks move nothing and the beat goes on
    // waiting for the pointer, not the history.
    for _ in 0..20 {
        drag_autoscroll_tick(&mut app, &mut out);
    }
    assert_eq!(scroll(&app), 15, "stops at the oldest row");
    assert_eq!(bounds(&app), ((0, 0), (79, 17)));
    assert!(app.next_drag_autoscroll.is_some());

    // Back inside: the beat stops and the head is under the pointer.
    handle_mouse(
        &mut app,
        mev(MouseEventKind::Drag(MouseButton::Left), 0, 4),
        &mut out,
    );
    assert!(app.next_drag_autoscroll.is_none());
    assert_eq!(bounds(&app), ((0, 1), (79, 17)));

    // The release copies the whole run — 17 rows into a 5-row pane.
    handle_mouse(
        &mut app,
        mev(MouseEventKind::Up(MouseButton::Left), 0, 4),
        &mut out,
    );
    assert!(app.next_drag_autoscroll.is_none());
    assert_eq!(
        selection_text(&app).as_deref(),
        Some(lines_text(1, 17).as_str())
    );
}

/// …and past the bottom edge the view scrolls back down toward the
/// live tail, two lines a tick from the footer's second row, stopping
/// at the tail.
#[test]
fn dragging_past_the_bottom_edge_scrolls_back_toward_the_tail() {
    let mut app = App::new();
    seed_tree(&mut app);
    let mut out = Vec::new();
    // Pane at host rows 1–5, scrolled to the top of its history: rows
    // 6 and up are past its bottom edge.
    twenty_line_pane(&mut app, 1);
    app.term.as_mut().unwrap().set_scroll(15);
    let scroll = |app: &App| app.term.as_ref().unwrap().scroll_offset();

    // Press on `line 2` (host row 3), drag two rows past the bottom.
    handle_mouse(
        &mut app,
        mev(MouseEventKind::Down(MouseButton::Left), 0, 3),
        &mut out,
    );
    handle_mouse(
        &mut app,
        mev(MouseEventKind::Drag(MouseButton::Left), 79, 7),
        &mut out,
    );
    assert_eq!(scroll(&app), 13);
    assert_eq!(bounds(&app), ((0, 2), (79, 6)));
    drag_autoscroll_tick(&mut app, &mut out);
    assert_eq!(scroll(&app), 11);
    assert_eq!(bounds(&app), ((0, 2), (79, 8)));

    for _ in 0..20 {
        drag_autoscroll_tick(&mut app, &mut out);
    }
    assert_eq!(scroll(&app), 0, "stops at the live tail");
    assert_eq!(bounds(&app), ((0, 2), (79, 19)));
    assert!(app.next_drag_autoscroll.is_some());

    handle_mouse(
        &mut app,
        mev(MouseEventKind::Up(MouseButton::Left), 79, 7),
        &mut out,
    );
    assert_eq!(
        selection_text(&app).as_deref(),
        Some(lines_text(2, 19).as_str())
    );
}

/// A replay that rebuilds the screen from scratch (a ring that wrapped,
/// a new process under the session) starts its history lines over. A
/// finished selection is dropped, as ever; a drag under way is carried
/// across by its screen rows, so the button ends it and not the replay.
#[test]
fn a_rebuilding_replay_carries_a_drag_across_and_drops_a_finished_selection() {
    let mut app = App::new();
    seed_tree(&mut app);
    let mut out = Vec::new();
    twenty_line_pane(&mut app, 1);
    let sref = SessionRef::Agent(AgentId("a1".into()));
    handle_mouse(
        &mut app,
        mev(MouseEventKind::Down(MouseButton::Left), 0, 3),
        &mut out,
    );
    handle_mouse(
        &mut app,
        mev(MouseEventKind::Drag(MouseButton::Left), 6, 4),
        &mut out,
    );
    assert_eq!(bounds(&app), ((0, 17), (6, 18)));

    // A replay from a seq this parser never saw: 22 lines, rebuilt —
    // the same two screen rows are now lines 19–20.
    let lines: Vec<String> = (0..22).map(|i| format!("line {i}")).collect();
    let replay = lines.join("\r\n").into_bytes();
    hse(
        &mut app,
        ServerEvent::Scrollback {
            session: sref.clone(),
            base_seq: 5_000,
            data: replay.clone(),
        },
    );
    assert_eq!(history_base(&app), 17);
    let sel = app.term_selection.expect("the drag survives the replay");
    assert!(sel.dragging);
    assert_eq!(sel.bounds(), ((0, 19), (6, 20)));
    assert_eq!(
        selection_text(&app).as_deref(),
        Some(lines_text(19, 20).as_str())
    );

    handle_mouse(
        &mut app,
        mev(MouseEventKind::Up(MouseButton::Left), 6, 4),
        &mut out,
    );
    assert!(app.term_selection.is_some_and(|s| !s.dragging));
    hse(
        &mut app,
        ServerEvent::Scrollback {
            session: sref,
            base_seq: 9_000,
            data: replay,
        },
    );
    assert!(
        app.term_selection.is_none(),
        "a rebuild drops a finished highlight"
    );
}

/// The parser's ring holds 10,000 rows; past that the oldest go, and
/// history lines keep counting from where they were, so a selection
/// stays on its text as the ring turns under it. A row that has gone
/// off the ring reads as nothing.
#[test]
fn a_selection_keeps_its_text_as_the_scrollback_ring_turns() {
    let mut app = App::new();
    seed_tree(&mut app);
    let mut out = Vec::new();
    let sref = SessionRef::Agent(AgentId("a1".into()));
    let mut term = AttachedTerm::new(sref, 80, 5);
    let lines: Vec<String> = (0..10_010).map(|i| format!("l{i}")).collect();
    term.parser.process(lines.join("\r\n").as_bytes());
    app.term = Some(term);
    app.term_area = ratatui::layout::Rect::new(0, 1, 80, 5);
    app.hits.push((app.term_area, HitTarget::TerminalPane));
    assert_eq!(
        app.term.as_ref().unwrap().parser.screen().scrollback_rows(),
        10_000
    );
    assert_eq!(
        history_base(&app),
        10_005,
        "five rows have gone off the ring"
    );

    handle_mouse(
        &mut app,
        mev(MouseEventKind::Down(MouseButton::Left), 0, 1),
        &mut out,
    );
    handle_mouse(
        &mut app,
        mev(MouseEventKind::Drag(MouseButton::Left), 5, 1),
        &mut out,
    );
    assert_eq!(bounds(&app), ((0, 10_005), (5, 10_005)));
    assert_eq!(selection_text(&app).as_deref(), Some("l10005"));

    // Three more lines: three more rows off the ring, the selected row
    // into the scrollback.
    app.term
        .as_mut()
        .unwrap()
        .parser
        .process(b"\r\nl10010\r\nl10011\r\nl10012");
    assert_eq!(history_base(&app), 10_008);
    assert_eq!(selection_text(&app).as_deref(), Some("l10005"));

    app.term_selection = Some(TermSelection {
        anchor: (0, 2),
        head: (5, 2),
        dragging: false,
        active: true,
        pointer: (0, 0),
    });
    assert_eq!(selection_text(&app), None, "line 2 fell off the ring");
}

#[test]
fn double_click_selects_word_and_persists() {
    let mut app = App::new();
    seed_tree(&mut app);
    let mut out = Vec::new();

    let sref = SessionRef::Agent(AgentId("a1".into()));
    let mut term = AttachedTerm::new(sref, 80, 24);
    term.parser.process(b"hello world");
    app.term = Some(term);
    app.term_area = ratatui::layout::Rect::new(0, 0, 80, 24);
    app.hits.push((app.term_area, HitTarget::TerminalPane));

    // Click, release, click again on the same cell (a fast double-click).
    handle_mouse(
        &mut app,
        mev(MouseEventKind::Down(MouseButton::Left), 2, 0),
        &mut out,
    );
    handle_mouse(
        &mut app,
        mev(MouseEventKind::Up(MouseButton::Left), 2, 0),
        &mut out,
    );
    handle_mouse(
        &mut app,
        mev(MouseEventKind::Down(MouseButton::Left), 2, 0),
        &mut out,
    );
    let sel = app.term_selection.expect("double-click selects the word");
    assert!(sel.active && !sel.dragging);
    assert_eq!(sel.bounds(), ((0, 0), (4, 0)));
    assert_eq!(selection_text(&app).as_deref(), Some("hello"));
    assert!(
        app.flash
            .as_deref()
            .is_some_and(|f| f.starts_with("copied")),
        "double-click copies the word"
    );

    // The release after the second click must not disturb the selection.
    handle_mouse(
        &mut app,
        mev(MouseEventKind::Up(MouseButton::Left), 2, 0),
        &mut out,
    );
    assert!(app.term_selection.is_some_and(|s| s.active));
}

#[test]
fn double_click_selects_single_char_word() {
    let mut app = App::new();
    seed_tree(&mut app);
    let mut out = Vec::new();

    let sref = SessionRef::Agent(AgentId("a1".into()));
    let mut term = AttachedTerm::new(sref, 80, 24);
    term.parser.process(b"a bc");
    app.term = Some(term);
    app.term_area = ratatui::layout::Rect::new(0, 0, 80, 24);
    app.hits.push((app.term_area, HitTarget::TerminalPane));

    handle_mouse(
        &mut app,
        mev(MouseEventKind::Down(MouseButton::Left), 0, 0),
        &mut out,
    );
    handle_mouse(
        &mut app,
        mev(MouseEventKind::Up(MouseButton::Left), 0, 0),
        &mut out,
    );
    handle_mouse(
        &mut app,
        mev(MouseEventKind::Down(MouseButton::Left), 0, 0),
        &mut out,
    );
    // A one-cell word: anchor == head but the selection is real.
    let sel = app.term_selection.expect("single-char word selected");
    assert!(sel.active);
    assert_eq!(sel.bounds(), ((0, 0), (0, 0)));
    assert_eq!(selection_text(&app).as_deref(), Some("a"));
}

#[test]
fn slow_second_click_arms_a_plain_drag() {
    let mut app = App::new();
    seed_tree(&mut app);
    let mut out = Vec::new();

    let sref = SessionRef::Agent(AgentId("a1".into()));
    let mut term = AttachedTerm::new(sref, 80, 24);
    term.parser.process(b"hello world");
    app.term = Some(term);
    app.term_area = ratatui::layout::Rect::new(0, 0, 80, 24);
    app.hits.push((app.term_area, HitTarget::TerminalPane));

    // A stale first click, well outside the double-click window.
    app.last_term_click = Some((
        std::time::Instant::now() - Duration::from_millis(500),
        (2, 0),
    ));
    handle_mouse(
        &mut app,
        mev(MouseEventKind::Down(MouseButton::Left), 2, 0),
        &mut out,
    );
    assert!(
        app.term_selection.is_some_and(|s| s.dragging && !s.active),
        "slow second click starts a fresh drag, not a word selection"
    );
}

#[test]
fn alt_click_opens_link_under_cursor() {
    let mut app = App::new();
    seed_tree(&mut app);
    let mut out = Vec::new();

    let sref = SessionRef::Agent(AgentId("a1".into()));
    let mut term = AttachedTerm::new(sref, 80, 24);
    term.parser.process(b"see https://example.com ok");
    app.term = Some(term);
    app.term_area = ratatui::layout::Rect::new(0, 0, 80, 24);
    app.hits.push((app.term_area, HitTarget::TerminalPane));
    app.term_links = crate::links::visible_links(app.term.as_ref().unwrap().parser.screen());
    assert_eq!(app.term_links.len(), 1);

    let alt = |kind, column, row| MouseEvent {
        kind,
        column,
        row,
        modifiers: KeyModifiers::ALT,
    };

    // ⌥click on the link opens it and swallows the click entirely.
    app.focus = Focus::Projects;
    handle_mouse(
        &mut app,
        alt(MouseEventKind::Down(MouseButton::Left), 6, 0),
        &mut out,
    );
    assert_eq!(
        app.flash.as_deref(),
        Some("opened https://example.com"),
        "the URL under the cursor is opened"
    );
    assert_eq!(app.focus, Focus::Projects, "focus is untouched");
    assert!(!app.term_locked, "input stays unlocked");
    assert!(app.term_selection.is_none(), "no selection armed");

    // ⌥click on a non-link cell falls through to a normal click.
    app.flash = None;
    handle_mouse(
        &mut app,
        alt(MouseEventKind::Down(MouseButton::Left), 0, 0),
        &mut out,
    );
    assert!(app.flash.is_none());
    assert_eq!(app.focus, Focus::Terminal);
    assert!(app.term_selection.is_some_and(|s| s.dragging));
}

#[test]
fn alt_click_on_file_path_resolves_against_attached_worktree() {
    let mut app = App::new();
    seed_tree(&mut app);
    let mut out = Vec::new();

    // Attach a1 (worktree /tmp/demo); the printed path doesn't exist
    // there, so the click reports it instead of spawning an editor.
    let sref = SessionRef::Agent(AgentId("a1".into()));
    let mut term = AttachedTerm::new(sref, 80, 24);
    term.parser.process(b"edited src/nope.rs:12 just now");
    app.term = Some(term);
    app.term_area = ratatui::layout::Rect::new(0, 0, 80, 24);
    app.hits.push((app.term_area, HitTarget::TerminalPane));
    app.term_file_links =
        crate::links::visible_file_links(app.term.as_ref().unwrap().parser.screen());
    assert_eq!(app.term_file_links.len(), 1);

    let alt = |column| MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column,
        row: 0,
        modifiers: KeyModifiers::ALT,
    };

    app.focus = Focus::Projects;
    handle_mouse(&mut app, alt(9), &mut out);
    assert_eq!(app.flash.as_deref(), Some("file not found: src/nope.rs"));
    assert!(app.vim.is_none());
    assert_eq!(app.focus, Focus::Projects, "the click is swallowed");
    assert!(app.term_selection.is_none(), "no selection armed");
}

#[test]
fn resolve_file_link_handles_diff_prefixes_and_absolutes() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src/app.rs"), "").unwrap();

    assert_eq!(
        resolve_file_link(root, "src/app.rs").as_deref(),
        Some("src/app.rs"),
        "relative paths stay relative (editor cwd is the worktree)"
    );
    assert_eq!(
        resolve_file_link(root, "a/src/app.rs").as_deref(),
        Some("src/app.rs"),
        "git-diff a/ prefix is stripped when the raw path is missing"
    );
    let abs = root.join("src/app.rs");
    assert_eq!(
        resolve_file_link(root, abs.to_str().unwrap()).as_deref(),
        abs.to_str(),
        "absolute paths pass through"
    );
    assert_eq!(resolve_file_link(root, "src/nope.rs"), None);
    assert_eq!(
        resolve_file_link(root, "src"),
        None,
        "directories don't open"
    );
}

fn mev(kind: MouseEventKind, column: u16, row: u16) -> MouseEvent {
    MouseEvent {
        kind,
        column,
        row,
        modifiers: KeyModifiers::NONE,
    }
}

/// A wheel tick over an app that enabled mouse reporting (claude's
/// alt-screen UI, vim `mouse=a`, htop) forwards the wheel event itself.
/// Synthesized arrows would land in claude's input box, cycling prompt
/// history and tripping its "Scroll wheel is sending arrow keys" hint.
#[test]
fn wheel_forwards_mouse_report_when_child_wants_mouse() {
    let mut app = App::new();
    let mut out = Vec::new();

    let sref = SessionRef::Agent(AgentId("a1".into()));
    let mut term = AttachedTerm::new(sref, 80, 24);
    // Claude's alt-screen entry: 1049 + tracking modes + SGR encoding.
    term.parser
        .process(b"\x1b[?1049h\x1b[?1000h\x1b[?1002h\x1b[?1003h\x1b[?1006h");
    app.term = Some(term);
    app.term_area = ratatui::layout::Rect::new(0, 0, 80, 24);
    app.hits.push((app.term_area, HitTarget::TerminalPane));

    handle_mouse(&mut app, mev(MouseEventKind::ScrollUp, 10, 5), &mut out);
    match out.as_slice() {
        [ClientRequest::Input { data, .. }] => assert_eq!(
            data, b"\x1b[<64;11;6M",
            "wheel-up becomes an SGR report at the 1-based pane cell"
        ),
        other => panic!("expected one Input request, got {other:?}"),
    }

    out.clear();
    handle_mouse(&mut app, mev(MouseEventKind::ScrollDown, 0, 0), &mut out);
    match out.as_slice() {
        [ClientRequest::Input { data, .. }] => assert_eq!(data, b"\x1b[<65;1;1M"),
        other => panic!("expected one Input request, got {other:?}"),
    }
}

/// Alt-screen apps that never asked for the mouse (plain vim, less) keep
/// the arrow-key emulation.
#[test]
fn wheel_sends_arrows_to_mouseless_alt_screen_apps() {
    let mut app = App::new();
    let mut out = Vec::new();

    let sref = SessionRef::Agent(AgentId("a1".into()));
    let mut term = AttachedTerm::new(sref, 80, 24);
    term.parser.process(b"\x1b[?1049h");
    app.term = Some(term);
    app.term_area = ratatui::layout::Rect::new(0, 0, 80, 24);
    app.hits.push((app.term_area, HitTarget::TerminalPane));

    handle_mouse(&mut app, mev(MouseEventKind::ScrollUp, 10, 5), &mut out);
    match out.as_slice() {
        [ClientRequest::Input { data, .. }] => assert_eq!(data, b"\x1b[A"),
        other => panic!("expected one Input request, got {other:?}"),
    }
}

/// A pane over a plain shell with `lines` of output behind its
/// 24-row grid, the wheel aimed at it.
fn scrolling_pane(app: &mut App, lines: usize) {
    let sref = SessionRef::Agent(AgentId("a1".into()));
    let mut term = AttachedTerm::new(sref, 80, 24);
    term.parser.process(&b"line\r\n".repeat(lines));
    app.term = Some(term);
    app.term_area = ratatui::layout::Rect::new(0, 0, 80, 24);
    app.hits.push((app.term_area, HitTarget::TerminalPane));
}

fn pane_offset(app: &App) -> usize {
    app.term.as_ref().unwrap().scroll_offset()
}

/// The wheel stops at the top of the history: a notch past it lands on
/// the top, not on an offset the next notch down would have to work off
/// before the view moved again. (The offset used to count up forever,
/// and the way back down took as many notches as were spent up there.)
#[test]
fn the_wheel_stops_at_the_top_of_the_history() {
    let mut app = App::new();
    let mut out = Vec::new();
    scrolling_pane(&mut app, 30);
    let top = app.term.as_ref().unwrap().parser.screen().scrollback_rows();
    assert!(top > 0 && top < 20, "a few rows of history: {top}");

    for _ in 0..20 {
        handle_mouse(&mut app, mev(MouseEventKind::ScrollUp, 10, 5), &mut out);
    }
    assert_eq!(pane_offset(&app), top, "the top, not twenty notches up");
    assert!(out.is_empty(), "nothing forwarded: {out:?}");

    app.dirty = false;
    handle_mouse(&mut app, mev(MouseEventKind::ScrollUp, 10, 5), &mut out);
    assert_eq!(pane_offset(&app), top, "a notch at the top stays there");

    handle_mouse(&mut app, mev(MouseEventKind::ScrollDown, 10, 5), &mut out);
    assert_eq!(
        pane_offset(&app),
        top - 1,
        "and the first notch down moves the view"
    );
    assert!(app.dirty);
}

/// Output arriving while scrolled back moves the offset up under the
/// view to keep it still (vt100's rule); the next notch, and the
/// header's `scroll N`, count from where the view really is.
#[test]
fn the_wheel_counts_from_where_output_left_the_view() {
    let mut app = App::new();
    let mut out = Vec::new();
    scrolling_pane(&mut app, 30);
    app.term.as_mut().unwrap().set_scroll(3);
    // Four more lines scroll out under the view.
    app.term
        .as_mut()
        .unwrap()
        .parser
        .process(&b"more\r\n".repeat(4));
    assert_eq!(pane_offset(&app), 7, "the view held still over them");

    handle_mouse(&mut app, mev(MouseEventKind::ScrollDown, 10, 5), &mut out);
    assert_eq!(pane_offset(&app), 6, "one line down from there, not from 3");
}

/// Wheel notches while a let-go history is on its way back move where
/// the replay lands, rather than being lost or counted past it.
#[test]
fn notches_while_the_history_is_on_its_way_back_move_the_landing() {
    let mut app = App::new();
    seed_tree(&mut app);
    let a1 = SessionRef::Agent(AgentId("a1".into()));
    pane_with_history(&mut app, &a1, 200);
    app.term.as_mut().expect("pane").drop_history();
    app.term_area = ratatui::layout::Rect::new(0, 0, 80, 24);
    app.hits.push((app.term_area, HitTarget::TerminalPane));

    let mut out = Vec::new();
    handle_mouse(&mut app, mev(MouseEventKind::ScrollUp, 10, 5), &mut out);
    assert!(
        matches!(
            out.last(),
            Some(ClientRequest::Attach { from_seq: None, .. })
        ),
        "the first notch asks for the whole ring: {out:?}"
    );
    assert_eq!(app.term.as_ref().unwrap().pending_scroll, Some(1));
    assert_eq!(pane_offset(&app), 1, "and the header counts it");

    out.clear();
    handle_mouse(&mut app, mev(MouseEventKind::ScrollUp, 10, 5), &mut out);
    handle_mouse(&mut app, mev(MouseEventKind::ScrollUp, 10, 5), &mut out);
    assert!(out.is_empty(), "no second ask: {out:?}");
    assert_eq!(app.term.as_ref().unwrap().pending_scroll, Some(3));

    let mut data = Vec::new();
    for n in 0..200 {
        data.extend_from_slice(format!("line {n}\r\n").as_bytes());
    }
    data.extend_from_slice(b"$ ");
    hse(
        &mut app,
        ServerEvent::Scrollback {
            session: a1,
            base_seq: 0,
            data,
        },
    );
    let pane = app.term.as_ref().expect("pane");
    assert!(pane.pending_scroll.is_none());
    assert_eq!(
        pane.scroll_offset(),
        3,
        "the replay lands on the last notch"
    );
    assert_eq!(pane.parser.screen().scrollback(), 3);
}

/// A term whose program asked for the mouse with `modes` (the DECSET
/// numbers), on a pane sitting right of the sidebars so reports have to
/// be pane-relative.
fn mouse_owning_pane(app: &mut App, modes: &[u16]) {
    let sref = SessionRef::Agent(AgentId("a1".into()));
    let mut term = AttachedTerm::new(sref, 80, 24);
    term.parser.process(b"\x1b[?1049h");
    for mode in modes {
        term.parser.process(format!("\x1b[?{mode}h").as_bytes());
    }
    app.term = Some(term);
    app.term_area = ratatui::layout::Rect::new(10, 2, 80, 24);
    app.hits.push((app.term_area, HitTarget::TerminalPane));
}

fn only_input(out: &[ClientRequest]) -> &[u8] {
    match out {
        [ClientRequest::Input { data, .. }] => data,
        other => panic!("expected one Input request, got {other:?}"),
    }
}

/// A program that asked for the mouse — claude's fullscreen renderer,
/// vim `mouse=a` — gets the left button: press, drag and release, in its
/// encoding and pane-relative, and nebula arms no selection of its own.
/// (#52: claude's diff panel sits beside the conversation, and a
/// screen-row copy of ours took both; claude's selection knows better.)
#[test]
fn the_left_button_goes_to_a_program_that_asked_for_the_mouse() {
    let mut app = App::new();
    let mut out = Vec::new();
    mouse_owning_pane(&mut app, &[1002, 1006]);

    handle_mouse(
        &mut app,
        mev(MouseEventKind::Down(MouseButton::Left), 15, 5),
        &mut out,
    );
    assert_eq!(only_input(&out), b"\x1b[<0;6;4M");
    assert!(app.term_selection.is_none(), "no selection of nebula's");
    assert!(app.term_locked, "a click into the pane still locks input");
    assert_eq!(app.focus, Focus::Terminal);
    assert!(
        app.term_mouse_grab.is_some(),
        "the program holds the button"
    );

    out.clear();
    handle_mouse(
        &mut app,
        mev(MouseEventKind::Drag(MouseButton::Left), 20, 6),
        &mut out,
    );
    assert_eq!(only_input(&out), b"\x1b[<32;11;5M");
    assert!(app.term_selection.is_none());

    // Wandering off the pane clamps to its edge, as our own drag does.
    out.clear();
    handle_mouse(
        &mut app,
        mev(MouseEventKind::Drag(MouseButton::Left), 200, 50),
        &mut out,
    );
    assert_eq!(only_input(&out), b"\x1b[<32;80;24M");

    out.clear();
    handle_mouse(
        &mut app,
        mev(MouseEventKind::Up(MouseButton::Left), 20, 6),
        &mut out,
    );
    assert_eq!(only_input(&out), b"\x1b[<0;11;5m");
    assert!(app.term_mouse_grab.is_none(), "the release lets go");
    assert!(app.flash.is_none(), "nebula copied nothing");
    assert!(app.term_selection.is_none());
}

/// Press-only tracking (`?9h`) has no drag or release reports, and a
/// program that never asked for SGR gets the legacy X10 bytes.
#[test]
fn press_only_tracking_gets_the_press_alone_in_x10_bytes() {
    let mut app = App::new();
    let mut out = Vec::new();
    mouse_owning_pane(&mut app, &[9]);

    handle_mouse(
        &mut app,
        mev(MouseEventKind::Down(MouseButton::Left), 15, 5),
        &mut out,
    );
    assert_eq!(only_input(&out), &[0x1b, b'[', b'M', 32, 32 + 6, 32 + 4]);

    out.clear();
    handle_mouse(
        &mut app,
        mev(MouseEventKind::Drag(MouseButton::Left), 20, 6),
        &mut out,
    );
    handle_mouse(
        &mut app,
        mev(MouseEventKind::Up(MouseButton::Left), 20, 6),
        &mut out,
    );
    assert!(out.is_empty(), "nothing after the press: {out:?}");
    assert!(app.term_mouse_grab.is_none());
}

/// Press/release tracking (`?1000h`) hears the press and the release but
/// never the motion between them.
#[test]
fn press_release_tracking_hears_no_drag() {
    let mut app = App::new();
    let mut out = Vec::new();
    mouse_owning_pane(&mut app, &[1000]);

    handle_mouse(
        &mut app,
        mev(MouseEventKind::Down(MouseButton::Left), 10, 2),
        &mut out,
    );
    assert_eq!(only_input(&out), &[0x1b, b'[', b'M', 32, 33, 33]);

    out.clear();
    handle_mouse(
        &mut app,
        mev(MouseEventKind::Drag(MouseButton::Left), 13, 2),
        &mut out,
    );
    assert!(out.is_empty(), "no motion report: {out:?}");

    handle_mouse(
        &mut app,
        mev(MouseEventKind::Up(MouseButton::Left), 13, 2),
        &mut out,
    );
    assert_eq!(only_input(&out), &[0x1b, b'[', b'M', 35, 36, 33]);
}

/// ⇧, ⌥ and ^ ride along in the report's modifier bits. (A ⌥click on a
/// link never gets this far — nebula opens the link itself.)
#[test]
fn modifiers_ride_along_in_the_report() {
    let mut app = App::new();
    let mut out = Vec::new();
    mouse_owning_pane(&mut app, &[1002, 1006]);

    handle_mouse(
        &mut app,
        MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 10,
            row: 2,
            modifiers: KeyModifiers::SHIFT | KeyModifiers::CONTROL,
        },
        &mut out,
    );
    assert_eq!(only_input(&out), b"\x1b[<20;1;1M");
}

/// A program that took the mouse and then exited: its last screen is
/// still ours to select from, so the click arms nebula's selection.
#[test]
fn an_exited_programs_screen_is_still_ours_to_select() {
    let mut app = App::new();
    let mut out = Vec::new();
    mouse_owning_pane(&mut app, &[1002, 1006]);
    let term = app.term.as_mut().unwrap();
    term.parser.process(b"hello world");
    term.exited = true;

    handle_mouse(
        &mut app,
        mev(MouseEventKind::Down(MouseButton::Left), 10, 2),
        &mut out,
    );
    assert!(out.is_empty(), "nothing to forward to: {out:?}");
    assert!(app.term_selection.is_some_and(|s| s.dragging && !s.active));

    handle_mouse(
        &mut app,
        mev(MouseEventKind::Drag(MouseButton::Left), 20, 2),
        &mut out,
    );
    assert_eq!(selection_text(&app).as_deref(), Some("hello world"));
}

/// A program's OSC 52 copy — claude's fullscreen renderer over ssh, vim,
/// tmux — is passed on to the terminal the user is sitting at; a ring
/// replay carrying an old one is not.
#[test]
fn a_programs_clipboard_write_is_relayed_to_the_terminal() {
    let mut app = App::new();
    let mut out = Vec::new();
    let sref = SessionRef::Agent(AgentId("a1".into()));
    app.term = Some(AttachedTerm::new(sref.clone(), 80, 24));

    handle_server_event(
        &mut app,
        ServerEvent::Output {
            session: sref.clone(),
            seq: 0,
            data: b"\x1b]52;c;aGVsbG8=\x07".to_vec(),
        },
        &mut out,
    );
    assert_eq!(app.pending_clipboard.as_deref(), Some("aGVsbG8="));
    assert!(
        app.flash
            .as_deref()
            .is_some_and(|f| f.contains("via terminal")),
        "flash names the route: {:?}",
        app.flash
    );

    app.pending_clipboard = None;
    app.flash = None;
    handle_server_event(
        &mut app,
        ServerEvent::Scrollback {
            session: sref,
            base_seq: 0,
            data: b"\x1b]52;c;aGVsbG8=\x07".to_vec(),
        },
        &mut out,
    );
    assert!(
        app.pending_clipboard.is_none(),
        "a replay is history, not a fresh copy"
    );
    assert!(app.flash.is_none());
}

#[test]
fn base64_encodes_every_padding_case() {
    // RFC 4648 vectors — the OSC 52 payload is unusable if padding slips.
    assert_eq!(base64_encode(b""), "");
    assert_eq!(base64_encode(b"f"), "Zg==");
    assert_eq!(base64_encode(b"fo"), "Zm8=");
    assert_eq!(base64_encode(b"foo"), "Zm9v");
    assert_eq!(base64_encode(b"foob"), "Zm9vYg==");
    assert_eq!(base64_encode(b"fooba"), "Zm9vYmE=");
    assert_eq!(base64_encode(b"foobar"), "Zm9vYmFy");
    // Non-ASCII selections go over the wire as UTF-8 bytes.
    assert_eq!(base64_encode("→".as_bytes()), "4oaS");
}

/// The LAUNCHER VIEW's pane edge drags like a panel boundary: hovering
/// it lights the grip and asks for the resize arrows, the grab keeps
/// its offset from the edge, motion trades rows between the cards and
/// the pane, both ends stop against their minimums, and mouse-up ends
/// it.
#[test]
fn launcher_pane_edge_drags_the_pane_taller_and_shorter() {
    use crate::launcher::{pane_height, CARD_H, HEAD_H, PANE_MIN_H};
    let mut app = App::new();
    // A pane along the bottom, whose edge trades rows; the one beside
    // the cards has its own drag test in `event_loop::launcher`.
    app.launcher_pane_at = crate::launcher::PaneSide::Bottom;
    // Tall enough that the default share, and a drag either side of
    // it, sit clear of both stops — the stops get their own drags below.
    let body = ratatui::layout::Rect::new(0, 0, 120, 60);
    app.launcher_body = body;
    let boundary = body.height - pane_height(body, None).expect("60 rows fits a pane");
    app.hits.push((
        ratatui::layout::Rect::new(0, boundary - 1, 120, 2),
        HitTarget::LauncherPaneSplitter,
    ));
    let mut out = Vec::new();

    // Hover the edge: the other arrows, and the grip lit.
    app.dirty = false;
    handle_mouse(&mut app, mev(MouseEventKind::Moved, 60, boundary), &mut out);
    assert_eq!(app.pointer_shape, PointerShape::RowResize);
    assert!(app.hover_launcher_pane);
    assert!(app.dirty, "hover change repaints the grip");

    // Off it: back to default, grip resting.
    app.dirty = false;
    handle_mouse(&mut app, mev(MouseEventKind::Moved, 60, 2), &mut out);
    assert_eq!(app.pointer_shape, PointerShape::Default);
    assert!(!app.hover_launcher_pane);
    assert!(app.dirty);

    handle_mouse(
        &mut app,
        mev(MouseEventKind::Down(MouseButton::Left), 60, boundary),
        &mut out,
    );
    assert_eq!(
        app.launcher_pane_drag,
        Some(0),
        "grabbed on the edge itself"
    );
    assert!(
        app.term_selection.is_none(),
        "a pane-edge grab must not arm a terminal selection"
    );
    assert!(
        app.mouse_held(),
        "the edge is held: the host gets no mode re-ask under the drag"
    );

    // Up five rows: the pane takes them off the cards.
    handle_mouse(
        &mut app,
        mev(MouseEventKind::Drag(MouseButton::Left), 60, boundary - 5),
        &mut out,
    );
    assert_eq!(app.launcher_pane_h, Some(body.height - boundary + 5));

    // A motion report with no button named, mid-drag, is still the
    // drag: a host that lost the button between two reports.
    handle_mouse(
        &mut app,
        mev(MouseEventKind::Moved, 60, boundary - 2),
        &mut out,
    );
    assert_eq!(app.launcher_pane_h, Some(body.height - boundary + 2));

    // Down again: the cards get them back.
    handle_mouse(
        &mut app,
        mev(MouseEventKind::Drag(MouseButton::Left), 60, boundary + 3),
        &mut out,
    );
    assert_eq!(app.launcher_pane_h, Some(body.height - boundary - 3));

    // Past the top: the header and a row of cards are kept.
    handle_mouse(
        &mut app,
        mev(MouseEventKind::Drag(MouseButton::Left), 60, 0),
        &mut out,
    );
    assert_eq!(app.launcher_pane_h, Some(body.height - (HEAD_H + CARD_H)));

    // Past the bottom: the pane keeps its own minimum.
    handle_mouse(
        &mut app,
        mev(MouseEventKind::Drag(MouseButton::Left), 60, body.height),
        &mut out,
    );
    assert_eq!(app.launcher_pane_h, Some(PANE_MIN_H));

    handle_mouse(
        &mut app,
        mev(MouseEventKind::Up(MouseButton::Left), 60, body.height),
        &mut out,
    );
    assert!(app.launcher_pane_drag.is_none(), "mouse-up ends the drag");
    assert!(!app.mouse_held(), "…and the mode re-ask beat resumes");
}

/// The ISSUES fold rides the UI-state blob like the OPEN PRS one, and
/// a blob from before it existed leaves the group open.
#[test]
fn ui_state_roundtrip_includes_the_issues_fold() {
    let mut app = App::new();
    app.issues_collapsed = true;
    let json = ui_state_json(&app);
    assert!(json.contains(r#""issues_collapsed":true"#), "{json}");

    let mut restored = App::new();
    restore_ui_state(&mut restored, &json);
    assert!(restored.issues_collapsed);

    let mut legacy = App::new();
    restore_ui_state(
        &mut legacy,
        r#"{"project":null,"worktree":null,"session_agent":null,"show_archived":false,"collapsed":false}"#,
    );
    assert!(!legacy.issues_collapsed, "old blobs keep the group open");
}

/// The fold is remembered like the ARCHIVED toggle: it rides the
/// UI-state blob out and back, and a blob from before it existed leaves
/// the group open.
#[test]
fn ui_state_roundtrip_includes_the_open_prs_fold() {
    let mut app = App::new();
    app.open_prs_collapsed = true;
    let json = ui_state_json(&app);
    assert!(json.contains(r#""open_prs_collapsed":true"#), "{json}");

    let mut restored = App::new();
    restore_ui_state(&mut restored, &json);
    assert!(restored.open_prs_collapsed);

    let mut legacy = App::new();
    restore_ui_state(
        &mut legacy,
        r#"{"project":null,"worktree":null,"session_agent":null,"show_archived":false,"collapsed":false}"#,
    );
    assert!(!legacy.open_prs_collapsed, "old blobs keep the group open");
}

fn project(id: &str, name: &str, sort_order: i64) -> nebula_core::Entity {
    use nebula_core::{Entity, Project, ProjectId};
    Entity::Project(Project {
        id: ProjectId(id.into()),
        name: name.into(),
        repo_path: format!("/tmp/{name}").into(),
        sort_order,
    })
}

/// The daemon re-homes rows on its own — a `nebula worktree` run
/// inside the session, or a hook cwd that walked into another checkout.
/// The selected session must not vanish from under the cursor when
/// that happens: the selection follows it into its new worktree.
#[test]
fn selection_follows_the_selected_agent_when_the_daemon_rehomes_it() {
    use nebula_core::{Agent, AgentStatus, Entity, Worktree};
    let mut app = App::new();
    seed_tree(&mut app); // p1 / w1(main) / a1
    hse(
        &mut app,
        ServerEvent::EntityUpserted {
            entity: Entity::Worktree(Worktree {
                id: WorktreeId("w2".into()),
                project_id: nebula_core::ProjectId("p1".into()),
                path: "/tmp/demo-feat".into(),
                branch: "feat".into(),
                is_main: false,
                sort_order: 0,
            }),
        },
    );
    app.focus = Focus::Sessions;
    let moved = |worktree: &str| Agent {
        id: AgentId("a1".into()),
        worktree_id: WorktreeId(worktree.into()),
        name: "agent-1".into(),
        status: AgentStatus::Fresh,
        archived: false,
        archived_at: 0,
        unseen: false,
        kind: nebula_core::AgentKind::Claude,
        custom_harness: None,
        model: None,
        effort: None,
        session_id: None,
        cloud_session_id: None,
        sort_order: 0,
        status_changed_at: 0,
        alive: true,
        issue_url: None,
        recent_prompts: Vec::new(),
    };

    // a1 is the selected session; its upsert lands under w2.
    assert_eq!(app.selected_session().map(|a| a.id.0), Some("a1".into()));
    hse(
        &mut app,
        ServerEvent::EntityUpserted {
            entity: Entity::Agent(moved("w2")),
        },
    );
    assert_eq!(
        app.selected_worktree().map(|w| w.branch.clone()),
        Some("feat".into()),
        "worktree selection followed the re-homed agent"
    );
    assert_eq!(app.selected_session().map(|a| a.id.0), Some("a1".into()));
    assert!(app.select_when_seen.is_none(), "follow intent consumed");

    // An agent that is NOT selected moving elsewhere leaves the cursor
    // where the user put it.
    app.sel_worktree = 0;
    hse(
        &mut app,
        ServerEvent::EntityUpserted {
            entity: Entity::Agent(moved("w1")),
        },
    );
    app.sel_session = 0;
    let before = app.selected_worktree().map(|w| w.id.clone());
    hse(
        &mut app,
        ServerEvent::EntityUpserted {
            entity: Entity::Agent(Agent {
                id: AgentId("a9".into()),
                ..moved("w1")
            }),
        },
    );
    hse(
        &mut app,
        ServerEvent::EntityUpserted {
            entity: Entity::Agent(Agent {
                id: AgentId("a9".into()),
                ..moved("w2")
            }),
        },
    );
    assert_eq!(app.selected_worktree().map(|w| w.id.clone()), before);
    assert!(app.select_when_seen.is_none());
}

/// `r` in the Projects panel retitles the row: the prompt opens on the
/// current name and the request carries a name and nothing else —
/// renaming a project never moves the folder it points at.
#[test]
fn r_renames_the_selected_project_row() {
    use nebula_core::ProjectId;
    let mut app = App::new();
    seed_tree(&mut app); // p1 "demo" at /tmp/demo
    let mut out = Vec::new();
    app.focus = Focus::Projects;

    press(&mut app, KeyCode::Char('r'), KeyModifiers::NONE, &mut out);
    match &app.overlay {
        Some(Overlay::Prompt(p)) => {
            assert_eq!(
                p.kind,
                PromptKind::RenameProject {
                    id: ProjectId("p1".into())
                }
            );
            assert_eq!(p.input.as_str(), "demo", "prefilled with the current name");
        }
        other => panic!("r: {other:?}"),
    }

    // Retype it and submit.
    press(
        &mut app,
        KeyCode::Char('u'),
        KeyModifiers::CONTROL,
        &mut out,
    );
    for c in "Acme API".chars() {
        press(&mut app, KeyCode::Char(c), KeyModifiers::NONE, &mut out);
    }
    press(&mut app, KeyCode::Enter, KeyModifiers::NONE, &mut out);
    assert!(
        matches!(
            out.last(),
            Some(ClientRequest::RenameProject { id, name, .. })
                if id.as_str() == "p1" && name == "Acme API"
        ),
        "expected RenameProject, got {out:?}"
    );
}

/// Submitting an empty name undoes the rename: the row goes back to the
/// folder's own name. `submit_prompt` cancels empty input for most
/// prompts, so this is the arm that has to opt out of that — the daemon
/// has always handled the reset, but the request never reached it.
#[test]
fn renaming_a_project_to_nothing_undoes_the_rename() {
    let mut app = App::new();
    seed_tree(&mut app);
    app.tree.projects[0].name = "Acme API".into();
    app.tree.projects[0].repo_path = "/tmp/acme-repo".into();
    let mut out = Vec::new();
    app.focus = Focus::Projects;

    press(&mut app, KeyCode::Char('r'), KeyModifiers::NONE, &mut out);
    press(
        &mut app,
        KeyCode::Char('u'),
        KeyModifiers::CONTROL,
        &mut out,
    );
    press(&mut app, KeyCode::Enter, KeyModifiers::NONE, &mut out);

    assert!(
        matches!(
            out.last(),
            Some(ClientRequest::RenameProject { id, name, .. })
                if id.as_str() == "p1" && name.is_empty()
        ),
        "an empty name is the undo, not a cancel: {out:?} flash={:?}",
        app.flash
    );
    assert!(
        app.overlay.is_none(),
        "the prompt closes: {:?}",
        app.overlay
    );
}

/// The shifted keys used to reorder projects; the column orders itself
/// now, so they neither send a request nor move anything.
#[test]
fn shifted_keys_no_longer_reorder_projects() {
    let mut app = App::new();
    seed_tree(&mut app);
    hse(
        &mut app,
        ServerEvent::EntityUpserted {
            entity: project("p2", "two", 1),
        },
    );
    let mut out = Vec::new();
    app.focus = Focus::Projects;
    for key in [
        KeyEvent::new(KeyCode::Down, KeyModifiers::SHIFT),
        KeyEvent::new(KeyCode::Char('J'), KeyModifiers::SHIFT),
        KeyEvent::new(KeyCode::Char('K'), KeyModifiers::SHIFT),
    ] {
        handle_key(&mut app, key, &mut out);
    }
    assert!(out.is_empty(), "no reorder request goes out: {out:?}");
    let order: Vec<&str> = app
        .project_rows()
        .into_iter()
        .map(|i| app.tree.projects[i].name.as_str())
        .collect();
    assert_eq!(order, ["demo", "two"], "nothing moved");
}

/// A finished-or-fresh agent under `wt`, stamped `at`.
fn agent_stamped(id: &str, wt: &str, at: i64) -> nebula_core::Entity {
    use nebula_core::{Agent, AgentStatus, Entity};
    Entity::Agent(Agent {
        id: AgentId(id.into()),
        worktree_id: WorktreeId(wt.into()),
        name: id.into(),
        status: if at > 0 {
            AgentStatus::Finished
        } else {
            AgentStatus::Fresh
        },
        archived: false,
        archived_at: 0,
        unseen: false,
        kind: nebula_core::AgentKind::Claude,
        custom_harness: None,
        model: None,
        effort: None,
        session_id: None,
        cloud_session_id: None,
        sort_order: 0,
        status_changed_at: at,
        alive: true,
        issue_url: None,
        recent_prompts: Vec::new(),
    })
}

/// Projects list most-recently-interacted first — the newest stamp of
/// any session under the project — and every cursor follows its row
/// across the re-sort. Never-run projects keep tree order at the
/// bottom.
#[test]
fn projects_sort_by_last_interaction_and_selection_follows() {
    use nebula_core::AgentStatus;
    let mut app = App::new();
    seed_tree(&mut app); // p1 "demo" / w1 / a1 (never run)
    hse(
        &mut app,
        ServerEvent::EntityUpserted {
            entity: project("p2", "two", 1),
        },
    );
    hse(
        &mut app,
        ServerEvent::EntityUpserted {
            entity: wt_entity("w2", "p2", "main", true),
        },
    );
    hse(
        &mut app,
        ServerEvent::EntityUpserted {
            entity: agent_stamped("a2", "w2", 0),
        },
    );
    let names = |app: &App| -> Vec<String> {
        app.project_rows()
            .into_iter()
            .map(|i| app.tree.projects[i].name.clone())
            .collect()
    };
    assert_eq!(
        names(&app),
        ["demo", "two"],
        "never-run rows keep tree order"
    );

    // Rest on "two", then its session finishes a turn: the project
    // heads the column and the cursor stays on it.
    app.focus = Focus::Projects;
    app.sel_project = 1;
    app.sel_worktree = 0;
    app.sel_session = 0;
    let now = crate::app::now_ms();
    hse(
        &mut app,
        ServerEvent::StatusChanged {
            agent: AgentId("a2".into()),
            status: AgentStatus::Finished,
            changed_at: now - 60_000,
            unseen: false,
        },
    );
    assert_eq!(names(&app), ["two", "demo"]);
    assert_eq!(
        app.sel_project, 0,
        "selection follows the project it was on"
    );
    assert_eq!(app.selected_worktree().map(|w| w.id.as_str()), Some("w2"));

    // Now "demo"'s session speaks: it overtakes, and "two" slides down
    // under the cursor.
    hse(
        &mut app,
        ServerEvent::StatusChanged {
            agent: AgentId("a1".into()),
            status: AgentStatus::Finished,
            changed_at: now,
            unseen: false,
        },
    );
    assert_eq!(names(&app), ["demo", "two"]);
    assert_eq!(app.sel_project, 1, "still on \"two\"");
}

/// Worktrees sort most-recently-interacted first below the root
/// checkout, which stays the first row no matter what; the cursor
/// follows its row across the re-sort.
#[test]
fn worktrees_sort_by_last_interaction() {
    use nebula_core::AgentStatus;
    let mut app = App::new();
    seed_tree(&mut app); // w1 "main" (root) / a1 never run
    let now = crate::app::now_ms();
    let mins = |n: i64| now - n * 60_000;
    hse(
        &mut app,
        ServerEvent::EntityUpserted {
            entity: wt_entity("w2", "p1", "feat", false),
        },
    );
    hse(
        &mut app,
        ServerEvent::EntityUpserted {
            entity: wt_entity("w3", "p1", "older", false),
        },
    );
    hse(
        &mut app,
        ServerEvent::EntityUpserted {
            entity: agent_stamped("a2", "w2", mins(5)),
        },
    );
    hse(
        &mut app,
        ServerEvent::EntityUpserted {
            entity: agent_stamped("a3", "w3", mins(50)),
        },
    );
    let branches = |app: &App| -> Vec<String> {
        app.visible_worktrees()
            .iter()
            .map(|w| w.branch.clone())
            .collect()
    };
    assert_eq!(
        branches(&app),
        ["main", "feat", "older"],
        "the never-run root stays on top; the rest sit newest first"
    );

    // Rest on "older", then its session finishes: it overtakes "feat"
    // but never the root, and the cursor follows it.
    app.focus = Focus::Worktrees;
    app.sel_worktree = 2;
    hse(
        &mut app,
        ServerEvent::StatusChanged {
            agent: AgentId("a3".into()),
            status: AgentStatus::Finished,
            changed_at: now,
            unseen: false,
        },
    );
    assert_eq!(branches(&app), ["main", "older", "feat"]);
    assert_eq!(app.sel_worktree, 1, "selection follows the \"older\" row");
}

#[test]
fn created_worktree_gets_selected() {
    use nebula_core::{Entity, Worktree, WorktreeId};
    let mut app = App::new();
    seed_tree(&mut app); // p1/w1(main) + agent-1
    let mut out = Vec::new();

    // The NEW WORKTREE box; submitting requests the worktree.
    let project = app.selected_project().expect("a project").id.clone();
    open_new_worktree_prompt(&mut app, project);
    for c in "feat".chars() {
        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE),
            &mut out,
        );
    }
    handle_key(
        &mut app,
        KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
        &mut out,
    );
    let Some(ClientRequest::CreateWorktree { req_id, .. }) = out.last() else {
        panic!("prompt submit requests a worktree: {out:?}");
    };
    let req_id = *req_id;

    // Enter already selected a stand-in row for the checkout, children
    // reset, sessions panel focused so `n` creates a session right
    // away (`event_loop::placeholder`); the daemon's upsert, then its
    // Ack, swap the real row in under that same cursor.
    let w2 = Worktree {
        id: WorktreeId("w2".into()),
        project_id: nebula_core::ProjectId("p1".into()),
        path: "/tmp/demo-worktrees/feat".into(),
        branch: "feat".into(),
        is_main: false,
        sort_order: 0,
    };
    hse(
        &mut app,
        ServerEvent::EntityUpserted {
            entity: Entity::Worktree(w2.clone()),
        },
    );
    hse(
        &mut app,
        ServerEvent::Ack {
            req_id,
            created: Some(EntityId::Worktree(w2.id.clone())),
        },
    );
    assert_eq!(app.focus, Focus::Sessions);
    assert_eq!(app.selected_worktree().map(|w| w.id.clone()), Some(w2.id));
    assert_eq!(app.sel_session, 0);
}

/// A branch is often described as a sentence ("fix login redirect");
/// git wants it hyphenated, so the prompt does that conversion rather
/// than handing git a ref it refuses.
#[test]
fn typed_worktree_name_hyphenates_spaces() {
    let mut app = App::new();
    seed_tree(&mut app); // p1/w1(main) + agent-1
    let mut out = Vec::new();
    app.focus = Focus::Worktrees;

    let project = app.selected_project().expect("a project").id.clone();
    open_new_worktree_prompt(&mut app, project);
    for c in "  fix login  redirect ".chars() {
        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE),
            &mut out,
        );
    }
    handle_key(
        &mut app,
        KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
        &mut out,
    );
    let Some(ClientRequest::CreateWorktree { branch, .. }) = out.last() else {
        panic!("prompt submit requests a worktree: {out:?}");
    };
    assert_eq!(branch, "fix-login-redirect");
}

/// Enter on an empty prompt takes the random name the prompt was
/// offering — the same one the label showed, not a fresh roll.
#[test]
fn empty_worktree_prompt_uses_the_offered_random_name() {
    let mut app = App::new();
    seed_tree(&mut app); // p1/w1(main) + agent-1
    let mut out = Vec::new();
    app.focus = Focus::Worktrees;

    let project = app.selected_project().expect("a project").id.clone();
    open_new_worktree_prompt(&mut app, project);
    let Some(Overlay::Prompt(prompt)) = &app.overlay else {
        panic!("the new-worktree prompt is up");
    };
    let PromptKind::NewWorktree { suggestion, .. } = &prompt.kind else {
        panic!("wrong prompt: {:?}", prompt.kind);
    };
    let offered = suggestion.clone();
    assert!(
        prompt.label.contains(&offered),
        "the offered name is not in the label: {}",
        prompt.label
    );
    assert_eq!(offered.split('-').count(), 3, "not three words: {offered}");

    handle_key(
        &mut app,
        KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
        &mut out,
    );
    let Some(ClientRequest::CreateWorktree { branch, .. }) = out.last() else {
        panic!("empty submit still requests a worktree: {out:?}");
    };
    assert_eq!(branch, &offered);
}

/// Typing only spaces is the same as typing nothing: no empty ref, no
/// "cancelled" flash — the offered name stands in.
#[test]
fn whitespace_only_worktree_name_falls_back_to_the_random_one() {
    let mut app = App::new();
    seed_tree(&mut app); // p1/w1(main) + agent-1
    let mut out = Vec::new();
    app.focus = Focus::Worktrees;

    let project = app.selected_project().expect("a project").id.clone();
    open_new_worktree_prompt(&mut app, project);
    for _ in 0..3 {
        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE),
            &mut out,
        );
    }
    handle_key(
        &mut app,
        KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
        &mut out,
    );
    let Some(ClientRequest::CreateWorktree { branch, .. }) = out.last() else {
        panic!("whitespace submit still requests a worktree: {out:?}");
    };
    assert_eq!(branch.split('-').count(), 3, "not a random name: {branch}");
}

/// A worktree with no remembered session — the first visit, or a
/// remembered row that has since vanished — comes up on its top row,
/// the most recently interacted session, not on a blank pane.
#[test]
fn a_worktree_with_nothing_remembered_shows_its_most_recent_session() {
    use nebula_core::Entity;
    let mut app = App::new();
    seed_tree(&mut app); // p1/w1(main) + a1
    hse(
        &mut app,
        ServerEvent::EntityUpserted {
            entity: wt_entity("w2", "p1", "other", false),
        },
    );
    // Two idle sessions on w2, upserted oldest-interaction first so
    // tree order and recency order disagree: the fallback must land on
    // the row the list shows at the top, which is by recency.
    let stamped = |id: &str, name: &str, stamp: i64| {
        let Entity::Agent(mut a) = agent_entity(id, "w2", name, false) else {
            unreachable!()
        };
        a.status = nebula_core::AgentStatus::Finished;
        a.status_changed_at = stamp;
        Entity::Agent(a)
    };
    hse(
        &mut app,
        ServerEvent::EntityUpserted {
            entity: stamped("a2", "older", 1_000),
        },
    );
    hse(
        &mut app,
        ServerEvent::EntityUpserted {
            entity: stamped("a3", "newer", 2_000),
        },
    );
    let a1 = SessionRef::Agent(AgentId("a1".into()));
    let a3 = SessionRef::Agent(AgentId("a3".into()));
    let w2 = WorktreeId("w2".into());
    let mut out = Vec::new();
    attach(&mut app, a1.clone(), &mut out);
    out.clear();

    // First visit: nothing is remembered for w2.
    assert!(!app.last_session_for_worktree.contains_key(&w2));
    select_worktree_row(&mut app, 1, &mut out);
    assert_eq!(
        app.selected_worktree().map(|w| w.branch.clone()),
        Some("other".into())
    );
    fire_pending_attach(&mut app, &mut out);
    assert_eq!(app.sel_session, 0, "cursor on the top row");
    assert_eq!(
        app.term.as_ref().map(|t| t.sref.clone()),
        Some(a3.clone()),
        "the pane shows the most recently interacted session"
    );
    assert!(
        out.iter()
            .any(|r| matches!(r, ClientRequest::Attach { session, .. } if *session == a3)),
        "a3 is attached: {out:?}"
    );

    // A remembered session that no longer exists falls back the same
    // way instead of blanking.
    select_worktree_row(&mut app, 0, &mut out);
    fire_pending_attach(&mut app, &mut out);
    assert_eq!(
        app.term.as_ref().map(|t| t.sref.clone()),
        Some(a1.clone()),
        "back on w1, its remembered session"
    );
    app.last_session_for_worktree
        .insert(w2.clone(), SessionRef::Agent(AgentId("gone".into())));
    out.clear();
    select_worktree_row(&mut app, 1, &mut out);
    fire_pending_attach(&mut app, &mut out);
    assert_eq!(app.sel_session, 0);
    assert_eq!(
        app.term.as_ref().map(|t| t.sref.clone()),
        Some(a3.clone()),
        "a vanished memory lands on the top row"
    );
    assert!(
        out.iter()
            .any(|r| matches!(r, ClientRequest::Attach { session, .. } if *session == a3)),
        "a3 is re-attached: {out:?}"
    );
}

#[test]
fn switching_contexts_restores_the_remembered_session() {
    use nebula_core::{Entity, Worktree, WorktreeId};
    let mut app = App::new();
    seed_tree(&mut app); // p1/w1(main) + agent-1
    let sref = SessionRef::Agent(AgentId("a1".into()));
    app.term = Some(AttachedTerm::new(sref.clone(), 40, 10));
    let mut out = Vec::new();

    // Moving within the session's own context keeps the pane: the
    // worktree list clamps at its single row.
    app.focus = Focus::Worktrees;
    handle_key(
        &mut app,
        KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE),
        &mut out,
    );
    assert!(app.term.is_some(), "clamped move keeps the pane");

    // Walking onto a sibling worktree with no history blanks the pane.
    hse(
        &mut app,
        ServerEvent::EntityUpserted {
            entity: Entity::Worktree(Worktree {
                id: WorktreeId("w2".into()),
                project_id: nebula_core::ProjectId("p1".into()),
                path: "/tmp/demo-worktrees/other".into(),
                branch: "other".into(),
                is_main: false,
                sort_order: 1,
            }),
        },
    );
    assert!(select_worktree_by_id(
        &mut app,
        &WorktreeId("w2".into()),
        &mut out
    ));
    assert!(
        matches!(out.last(), Some(ClientRequest::Detach { session }) if *session == sref),
        "leaving the worktree detaches: {out:?}"
    );
    assert!(app.term.is_none(), "no history on w2 — pane blanks");

    // Walking back restores the remembered session, re-attached.
    assert!(select_worktree_by_id(
        &mut app,
        &WorktreeId("w1".into()),
        &mut out
    ));
    // The pane comes back at once; the Attach waits out the debounce, so
    // sweeping through worktrees doesn't cold-boot each one in passing.
    fire_pending_attach(&mut app, &mut out);
    assert!(
        matches!(out.last(), Some(ClientRequest::Attach { session, .. }) if *session == sref),
        "returning to w1 re-attaches its session: {out:?}"
    );
    assert_eq!(
        app.term.as_ref().map(|t| t.sref.clone()),
        Some(sref.clone())
    );
    assert_eq!(app.sel_session, 0);

    // Project switches remember the whole context: leaving p1 blanks
    // (p2 has no history), returning restores worktree AND session.
    hse(
        &mut app,
        ServerEvent::EntityUpserted {
            entity: project("p2", "two", 1),
        },
    );
    select_project_row(&mut app, 1, &mut out);
    assert_eq!(
        app.selected_project().map(|p| p.name.clone()),
        Some("two".into())
    );
    assert!(app.term.is_none(), "no history on p2 — pane blanks");

    select_project_row(&mut app, 0, &mut out);
    assert_eq!(
        app.selected_project().map(|p| p.name.clone()),
        Some("demo".into())
    );
    assert_eq!(app.sel_worktree, 0, "p1 remembered its worktree row");
    assert_eq!(
        app.term.as_ref().map(|t| t.sref.clone()),
        Some(sref),
        "returning to p1 re-shows its session"
    );
}

#[test]
fn backspace_opens_delete_confirm_per_panel() {
    let mut app = App::new();
    seed_tree(&mut app);
    let mut out = Vec::new();

    app.focus = Focus::Projects;
    press(&mut app, KeyCode::Backspace, KeyModifiers::NONE, &mut out);
    assert!(
        matches!(
            &app.overlay,
            Some(Overlay::Confirm(c)) if matches!(c.action, PendingAction::RemoveProject(_))
        ),
        "backspace on a project confirms removal: {:?}",
        app.overlay
    );
    press(&mut app, KeyCode::Esc, KeyModifiers::NONE, &mut out);

    // The seeded worktree is the main checkout — deletion is refused.
    app.focus = Focus::Worktrees;
    press(&mut app, KeyCode::Backspace, KeyModifiers::NONE, &mut out);
    assert!(app.overlay.is_none(), "main checkout never gets a confirm");
    assert!(app.flash.is_some(), "main checkout delete flashes instead");

    app.focus = Focus::Sessions;
    press(&mut app, KeyCode::Backspace, KeyModifiers::NONE, &mut out);
    assert!(
        matches!(
            &app.overlay,
            Some(Overlay::Confirm(c)) if matches!(c.action, PendingAction::DeleteAgent(_))
        ),
        "backspace on a session confirms agent delete: {:?}",
        app.overlay
    );
}

#[test]
fn exited_session_does_not_trap_keys() {
    let mut app = App::new();
    seed_tree(&mut app);
    let mut out = Vec::new();

    let sref = SessionRef::Agent(AgentId("a1".into()));
    app.term = Some(AttachedTerm::new(sref, 80, 24));
    app.term.as_mut().unwrap().exited = true;
    app.focus = Focus::Terminal;
    app.term_locked = true;
    app.collapsed = true;

    // No input reaches a dead PTY.
    handle_key(
        &mut app,
        KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE),
        &mut out,
    );
    assert!(out.is_empty(), "no input to a dead pty");

    // Esc leaves the pane and expands collapsed sidebars.
    handle_key(
        &mut app,
        KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
        &mut out,
    );
    assert_eq!(app.focus, Focus::Sessions, "Esc leaves an exited pane");
    assert!(!app.collapsed, "escape expands sidebars");

    // Keys fall through to the grid instead of being swallowed by a
    // pane with nothing behind it.
    app.focus = Focus::Terminal;
    out.clear();
    handle_key(
        &mut app,
        KeyEvent::new(KeyCode::Left, KeyModifiers::NONE),
        &mut out,
    );
    assert!(
        !out.iter().any(|r| matches!(r, ClientRequest::Input { .. })),
        "an exited pane forwards nothing: {out:?}"
    );
}

// ---- branch switcher ----

/// A linked worktree beside `seed_tree`'s root, on its own branch.
fn seed_linked_worktree(app: &mut App) {
    use nebula_core::{Entity, ProjectId, Worktree, WorktreeId};
    hse(
        app,
        ServerEvent::EntityUpserted {
            entity: Entity::Worktree(Worktree {
                id: WorktreeId("w2".into()),
                project_id: ProjectId("p1".into()),
                path: "/tmp/demo-worktrees/feature".into(),
                branch: "feature".into(),
                is_main: false,
                sort_order: 0,
            }),
        },
    );
}

fn branch_switch_target(app: &App) -> Option<WorktreeId> {
    match &app.overlay {
        Some(Overlay::BranchSwitch(view)) => Some(view.worktree.clone()),
        _ => None,
    }
}

/// `c` moves the ROOT WORKTREE and nothing else: from its own row, from
/// the Projects panel whatever the Worktrees cursor is on, and never
/// from a linked worktree's row, which stays on the branch it was cut
/// for.
#[test]
fn c_opens_the_branch_switcher_on_the_root_and_refuses_a_linked_worktree() {
    let mut app = App::new();
    let mut out = Vec::new();
    seed_tree(&mut app);
    seed_linked_worktree(&mut app);
    let root = WorktreeId("w1".into());
    let mains: Vec<bool> = app.visible_worktrees().iter().map(|w| w.is_main).collect();
    assert_eq!(mains, [true, false], "the root row leads");

    app.focus = Focus::Worktrees;
    app.sel_worktree = 1;
    press(&mut app, KeyCode::Char('c'), KeyModifiers::NONE, &mut out);
    assert!(app.overlay.is_none(), "{:?}", app.overlay);
    assert!(
        app.flash
            .as_deref()
            .is_some_and(|f| f.contains("root checkout")),
        "{:?}",
        app.flash
    );

    app.focus = Focus::Projects;
    press(&mut app, KeyCode::Char('c'), KeyModifiers::NONE, &mut out);
    assert_eq!(
        branch_switch_target(&app),
        Some(root.clone()),
        "the Projects panel means the project's root"
    );
    app.overlay = None;

    app.focus = Focus::Worktrees;
    app.sel_worktree = 0;
    press(&mut app, KeyCode::Char('c'), KeyModifiers::NONE, &mut out);
    assert_eq!(branch_switch_target(&app), Some(root));
}

/// The root row's menu is where the mouse finds the switcher, in the
/// seat a linked worktree's Delete takes.
#[test]
fn the_root_rows_menu_offers_switch_branch_and_a_worktree_row_offers_delete() {
    let mut app = App::new();
    let mut out = Vec::new();
    seed_tree(&mut app);
    seed_linked_worktree(&mut app);
    app.focus = Focus::Worktrees;
    let labels = |app: &App| match &app.overlay {
        Some(Overlay::Menu(menu)) => menu
            .items
            .iter()
            .map(|i| i.label.clone())
            .collect::<Vec<_>>(),
        other => panic!("no menu: {other:?}"),
    };

    app.sel_worktree = 1;
    open_row_menu(&mut app);
    let linked = labels(&app);
    assert!(linked.iter().any(|l| l == "Delete worktree"), "{linked:?}");
    assert!(!linked.iter().any(|l| l == "Switch branch…"), "{linked:?}");
    app.overlay = None;

    app.sel_worktree = 0;
    open_row_menu(&mut app);
    let root = labels(&app);
    assert!(!root.iter().any(|l| l == "Delete worktree"), "{root:?}");
    let at = root
        .iter()
        .position(|l| l == "Switch branch…")
        .expect("the root row offers it");
    if let Some(Overlay::Menu(menu)) = &mut app.overlay {
        menu.hover = at;
    }
    press(&mut app, KeyCode::Enter, KeyModifiers::NONE, &mut out);
    assert_eq!(branch_switch_target(&app), Some(WorktreeId("w1".into())));
}

/// Through the real key path — the KEYMAP's `c`, a query typed into the
/// modal (so `f`, `e` and `a` feed the filter instead of firing their
/// panel hotkeys), `Enter` — a real root checkout moves and its row
/// follows before the DAEMON's sync has said a word.
#[test]
fn the_branch_switcher_moves_a_real_root_through_the_key_path() {
    let dir = tempfile::tempdir().unwrap();
    let repo = test_repo(&dir);
    run_git(&repo, &["branch", "feature"]);
    let mut app = App::new();
    let mut out = Vec::new();
    seed_repo_tree(&mut app, &repo);
    app.focus = Focus::Worktrees;
    press(&mut app, KeyCode::Char('c'), KeyModifiers::NONE, &mut out);
    for c in "feat".chars() {
        press(&mut app, KeyCode::Char(c), KeyModifiers::NONE, &mut out);
    }
    press(&mut app, KeyCode::Enter, KeyModifiers::NONE, &mut out);
    assert!(app.overlay.is_none(), "{:?}", app.overlay);
    let head = std::process::Command::new("git")
        .arg("-C")
        .arg(&repo)
        .args(["rev-parse", "--abbrev-ref", "HEAD"])
        .output()
        .unwrap();
    assert_eq!(String::from_utf8_lossy(&head.stdout).trim(), "feature");
    assert_eq!(
        app.selected_worktree().map(|w| w.branch.as_str()),
        Some("feature")
    );
}

// ---- git-diff modal ----

/// The NEW SESSION PICKER for the selected checkout — what the card
/// menu's **New agent** opens, and what the Sessions panel's `n` used
/// to. The GRID's `n` is the QUICK PROMPT box, so the picker is opened
/// here rather than typed into being.
pub(super) fn open_picker(app: &mut App) {
    let worktree = app
        .selected_worktree()
        .map(|w| w.id.clone())
        .expect("a checkout is selected");
    open_new_agent_picker(app, worktree);
}

pub(super) fn press(
    app: &mut App,
    code: KeyCode,
    mods: KeyModifiers,
    out: &mut Vec<ClientRequest>,
) {
    handle_key(app, KeyEvent::new(code, mods), out);
}

fn run_git(repo: &std::path::Path, args: &[&str]) {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .output()
        .expect("run git");
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// `git init` + one commit containing a.txt.
fn test_repo(dir: &tempfile::TempDir) -> std::path::PathBuf {
    let repo = dir.path().join("repo");
    std::fs::create_dir(&repo).unwrap();
    run_git(&repo, &["init", "-b", "main"]);
    run_git(&repo, &["config", "user.email", "t@t"]);
    run_git(&repo, &["config", "user.name", "t"]);
    std::fs::write(repo.join("a.txt"), "orig\n").unwrap();
    run_git(&repo, &["add", "."]);
    run_git(&repo, &["commit", "-m", "init"]);
    repo
}

/// Like `seed_tree`, but the worktree points at a real checkout.
fn seed_repo_tree(app: &mut App, path: &std::path::Path) {
    use nebula_core::{Entity, Project, ProjectId, Worktree, WorktreeId};
    hse(
        app,
        ServerEvent::EntityUpserted {
            entity: Entity::Project(Project {
                id: ProjectId("p1".into()),
                name: "demo".into(),
                repo_path: path.to_path_buf(),
                sort_order: 0,
            }),
        },
    );
    hse(
        app,
        ServerEvent::EntityUpserted {
            entity: Entity::Worktree(Worktree {
                id: WorktreeId("w1".into()),
                project_id: ProjectId("p1".into()),
                path: path.to_path_buf(),
                branch: "main".into(),
                is_main: true,
                sort_order: 0,
            }),
        },
    );
}

/// Hand-built modal state — no git involved.
fn fake_diff_view(lines: usize) -> crate::app::DiffView {
    use crate::git_diff::DiffFile;
    let mut view = DiffView::new(
        "/nonexistent-nebula-diff-test".into(),
        "main".into(),
        vec![
            DiffFile {
                path: "alpha.rs".into(),
                orig_path: None,
                xy: ['M', ' '],
            },
            DiffFile {
                path: "beta.rs".into(),
                orig_path: None,
                xy: ['?', '?'],
            },
        ],
        true,
    );
    view.diff = (0..lines)
        .map(|i| format!("line {i}"))
        .collect::<Vec<_>>()
        .join("\n");
    view.diff_line_count = lines;
    view.view_height = 20;
    view
}

#[test]
fn g_opens_diff_modal_and_esc_closes() {
    let dir = tempfile::tempdir().unwrap();
    let repo = test_repo(&dir);
    std::fs::write(repo.join("a.txt"), "changed\n").unwrap();
    std::fs::write(repo.join("z.txt"), "fresh\n").unwrap();

    let mut app = App::new();
    seed_repo_tree(&mut app, &repo);
    let mut out = Vec::new();
    press(&mut app, KeyCode::Char('g'), KeyModifiers::NONE, &mut out);
    match &app.overlay {
        Some(Overlay::Diff(v)) => {
            assert_eq!(v.files.len(), 2, "{:?}", v.files);
            assert_eq!(v.branch, "main");
            assert!(v.head_ok);
            // Status is path-ordered, so a.txt is selected first.
            assert!(v.diff.contains("-orig"), "{}", v.diff);
            assert!(v.diff.contains("+changed"), "{}", v.diff);
        }
        other => panic!("expected diff overlay, got {other:?}"),
    }
    press(&mut app, KeyCode::Esc, KeyModifiers::NONE, &mut out);
    assert!(app.overlay.is_none(), "Esc closes the modal");
    assert!(out.is_empty(), "the diff modal never talks to the daemon");
}

#[test]
fn g_with_clean_repo_flashes_no_changes() {
    let dir = tempfile::tempdir().unwrap();
    let repo = test_repo(&dir);
    let mut app = App::new();
    seed_repo_tree(&mut app, &repo);
    let mut out = Vec::new();
    press(&mut app, KeyCode::Char('g'), KeyModifiers::NONE, &mut out);
    assert!(app.overlay.is_none(), "clean tree opens no modal");
    assert!(
        app.flash
            .as_deref()
            .unwrap_or("")
            .contains("no changes in main"),
        "{:?}",
        app.flash
    );
}

/// `G` turns the checkout's remote into a page and hands it to the
/// browser (`open_url` is a no-op under test, so the flash is the
/// observable half).
#[test]
fn shift_g_opens_the_repos_remote_in_the_browser() {
    let dir = tempfile::tempdir().unwrap();
    let repo = test_repo(&dir);
    run_git(
        &repo,
        &["remote", "add", "origin", "git@github.com:o/r.git"],
    );
    let mut app = App::new();
    seed_repo_tree(&mut app, &repo);
    let mut out = Vec::new();
    press(&mut app, KeyCode::Char('G'), KeyModifiers::SHIFT, &mut out);
    assert_eq!(app.flash.as_deref(), Some("opened github.com/o/r"));
    assert!(app.overlay.is_none(), "the browser is the whole feature");
    assert!(out.is_empty(), "nothing to tell the daemon about");
}

#[test]
fn shift_g_without_a_remote_says_so() {
    let dir = tempfile::tempdir().unwrap();
    let repo = test_repo(&dir);
    let mut app = App::new();
    seed_repo_tree(&mut app, &repo);
    let mut out = Vec::new();
    press(&mut app, KeyCode::Char('G'), KeyModifiers::SHIFT, &mut out);
    assert_eq!(app.flash.as_deref(), Some("no git remote on this repo"));
}

/// The badge cache follows the checkout: dirty counts (staged, unstaged
/// and untracked alike), clean clears, an unreadable path shows nothing,
/// and every value change marks the app dirty so a frame gets drawn.
#[test]
fn refresh_git_changes_tracks_the_checkout() {
    let dir = tempfile::tempdir().unwrap();
    let repo = test_repo(&dir);
    let mut app = App::new();
    seed_repo_tree(&mut app, &repo);

    app.dirty = false;
    refresh_git_changes(&mut app);
    assert_eq!(app.selected_worktree_changes(), Some(0), "clean tree");
    assert!(app.dirty, "first computation redraws");
    assert!(!app.git_changes_stale(), "cache matches the selection");

    std::fs::write(repo.join("a.txt"), "changed\n").unwrap();
    std::fs::write(repo.join("z.txt"), "fresh\n").unwrap();
    app.dirty = false;
    refresh_git_changes(&mut app);
    assert_eq!(app.selected_worktree_changes(), Some(2), "dirty tree");
    assert!(app.dirty, "count change redraws");

    app.dirty = false;
    refresh_git_changes(&mut app);
    assert!(!app.dirty, "an unchanged count skips the redraw");

    run_git(&repo, &["add", "."]);
    run_git(&repo, &["commit", "-m", "wip"]);
    refresh_git_changes(&mut app);
    assert_eq!(app.selected_worktree_changes(), Some(0), "commit clears");
}

/// A count that lands for a checkout the cursor has since left is kept
/// for that checkout and never shown for the selected one, which reads
/// as stale until its own count lands.
#[test]
fn a_count_for_another_checkout_keeps_the_badge_quiet() {
    let mut app = App::new();
    seed_tree(&mut app); // w1 selected
    app.git_changes_inflight = Some(WorktreeId("w2".into()));
    land_git_changes(&mut app, WorktreeId("w2".into()), Some(5));
    assert!(
        app.git_changes_inflight.is_none(),
        "the answer frees the slot"
    );
    assert_eq!(
        app.selected_worktree_changes(),
        None,
        "another checkout's count never shows"
    );
    assert!(
        app.git_changes_stale(),
        "so the selection still wants its own"
    );
    land_git_changes(&mut app, WorktreeId("w1".into()), Some(3));
    assert_eq!(app.selected_worktree_changes(), Some(3));
    assert!(!app.git_changes_stale());
}

#[test]
fn refresh_git_changes_survives_a_missing_repo() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = App::new();
    seed_repo_tree(&mut app, &dir.path().join("nope"));
    refresh_git_changes(&mut app);
    assert_eq!(app.selected_worktree_changes(), None);
    assert!(!app.git_changes_stale(), "the failed read is still cached");
}

/// A band's rule prints its checkout's changed-file count right behind
/// the branch, from whichever read landed there last; a clean checkout,
/// and a count read in some other checkout, print nothing — and the
/// footer no longer carries it at all.
#[test]
fn a_card_shows_its_checkouts_change_count() {
    let mut app = App::new();
    seed_tree(&mut app);
    let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();

    terminal.draw(|f| ui::draw(f, &mut app)).unwrap();
    assert!(
        !buffer_text(&terminal).contains("⌂ main +"),
        "no count before one is read"
    );

    land_git_changes(&mut app, WorktreeId("w1".into()), Some(2));
    terminal.draw(|f| ui::draw(f, &mut app)).unwrap();
    let text = buffer_text(&terminal);
    assert!(text.contains("⌂ main +2 files"), "card:\n{text}");
    let footer = text.lines().last().unwrap_or_default();
    assert!(!footer.contains("+2 file"), "footer: {footer}");

    land_git_changes(&mut app, WorktreeId("w1".into()), Some(1));
    terminal.draw(|f| ui::draw(f, &mut app)).unwrap();
    let text = buffer_text(&terminal);
    assert!(text.contains("⌂ main +1 file "), "singular:\n{text}");

    land_git_changes(&mut app, WorktreeId("w1".into()), Some(0));
    terminal.draw(|f| ui::draw(f, &mut app)).unwrap();
    assert!(
        !buffer_text(&terminal).contains("⌂ main +"),
        "a clean checkout stays quiet"
    );

    // Another checkout's count stays on that checkout's cards.
    land_swept_changes(&mut app, WorktreeId("w2".into()), Some(5));
    terminal.draw(|f| ui::draw(f, &mut app)).unwrap();
    assert!(
        !buffer_text(&terminal).contains("+5 file"),
        "no card of w1 wears w2's count"
    );
}

/// The cards' sweep reads the grid's other checkouts, never the
/// selected one (its own reads keep it fresh): one never read first,
/// then whichever was read longest ago.
#[test]
fn the_change_sweep_takes_the_stalest_unselected_checkout() {
    use nebula_core::{Agent, Entity, Worktree};
    let mut app = App::new();
    seed_tree(&mut app); // w1 selected, its agent a1 on the grid
    let one = app.tree.agents[0].clone();
    for i in 2..=3 {
        hse(
            &mut app,
            ServerEvent::EntityUpserted {
                entity: Entity::Worktree(Worktree {
                    id: WorktreeId(format!("w{i}")),
                    project_id: nebula_core::ProjectId("p1".into()),
                    path: format!("/tmp/demo-{i}").into(),
                    branch: format!("feat-{i}"),
                    is_main: false,
                    sort_order: i,
                }),
            },
        );
        hse(
            &mut app,
            ServerEvent::EntityUpserted {
                entity: Entity::Agent(Agent {
                    id: AgentId(format!("a{i}")),
                    worktree_id: WorktreeId(format!("w{i}")),
                    name: format!("agent-{i}"),
                    sort_order: i,
                    ..one.clone()
                }),
            },
        );
    }
    let target = |app: &App| changes_sweep_target(app).map(|(id, _)| id.0);
    let first = target(&app).expect("a checkout to sweep");
    assert_ne!(first, "w1", "never the selected checkout");
    land_swept_changes(&mut app, WorktreeId(first.clone()), Some(1));
    let second = target(&app).expect("the other one");
    assert!(
        second != first && second != "w1",
        "the never-read one comes next: {second}"
    );
    land_swept_changes(&mut app, WorktreeId(second), Some(0));
    assert_eq!(
        target(&app).as_deref(),
        Some(first.as_str()),
        "then the stalest"
    );
}

#[test]
fn g_with_missing_path_flashes() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = App::new();
    seed_repo_tree(&mut app, &dir.path().join("nope"));
    let mut out = Vec::new();
    press(&mut app, KeyCode::Char('g'), KeyModifiers::NONE, &mut out);
    assert!(app.overlay.is_none());
    assert!(
        app.flash.as_deref().unwrap_or("").contains("missing"),
        "{:?}",
        app.flash
    );
}

#[test]
fn diff_modal_keys_switch_files_and_scroll() {
    let mut app = App::new();
    seed_tree(&mut app);
    app.overlay = Some(Overlay::Diff(fake_diff_view(100)));
    let mut out = Vec::new();
    let scroll = |app: &App| match &app.overlay {
        Some(Overlay::Diff(v)) => (v.selected, v.scroll),
        _ => panic!("diff overlay gone"),
    };

    press(&mut app, KeyCode::Down, KeyModifiers::SHIFT, &mut out);
    assert_eq!(scroll(&app), (0, 1), "Shift+Down scrolls down one line");
    press(&mut app, KeyCode::Up, KeyModifiers::SHIFT, &mut out);
    press(&mut app, KeyCode::Up, KeyModifiers::SHIFT, &mut out);
    assert_eq!(scroll(&app), (0, 0), "Shift+Up clamps at the top");
    press(&mut app, KeyCode::End, KeyModifiers::NONE, &mut out);
    assert_eq!(scroll(&app), (0, 80), "End jumps to max scroll");
    press(&mut app, KeyCode::PageDown, KeyModifiers::NONE, &mut out);
    assert_eq!(scroll(&app), (0, 80), "paging clamps at the bottom");
    press(&mut app, KeyCode::Home, KeyModifiers::NONE, &mut out);
    assert_eq!(scroll(&app), (0, 0), "Home jumps back to the top");

    // File switch resets the scroll; the fake root makes the reload an
    // error body, which must not panic.
    press(&mut app, KeyCode::End, KeyModifiers::NONE, &mut out);
    press(&mut app, KeyCode::Down, KeyModifiers::NONE, &mut out);
    assert_eq!(scroll(&app).0, 1, "Down selects the next file");
    assert_eq!(scroll(&app).1, 0, "file switch resets scroll");
    press(&mut app, KeyCode::Down, KeyModifiers::NONE, &mut out);
    assert_eq!(scroll(&app).0, 1, "selection clamps at the last file");
    press(&mut app, KeyCode::Up, KeyModifiers::NONE, &mut out);
    assert_eq!(scroll(&app).0, 0, "Up selects the previous file");
    press(
        &mut app,
        KeyCode::Char('d'),
        KeyModifiers::CONTROL,
        &mut out,
    );
    assert_eq!(scroll(&app).0, 1, "Ctrl+d walks the file list");
    press(
        &mut app,
        KeyCode::Char('u'),
        KeyModifiers::CONTROL,
        &mut out,
    );
    assert_eq!(scroll(&app).0, 0, "Ctrl+u walks it back");

    press(&mut app, KeyCode::Esc, KeyModifiers::NONE, &mut out);
    assert!(app.overlay.is_none(), "Esc closes the modal");
    assert!(out.is_empty());
}

/// INPUT PARITY: the wheel over the file list walks the file cursor the
/// way ↑/↓ do; over the diff pane it still scrolls the diff.
#[test]
fn diff_modal_wheel_over_file_list_walks_files() {
    let mut app = App::new();
    seed_tree(&mut app);
    let mut view = fake_diff_view(100);
    view.area = ratatui::layout::Rect::new(0, 0, 100, 30);
    view.files_width = 30;
    view.list_area = ratatui::layout::Rect::new(1, 2, 28, 26);
    app.overlay = Some(Overlay::Diff(view));
    let mut out = Vec::new();
    let state = |app: &App| match &app.overlay {
        Some(Overlay::Diff(v)) => (v.selected, v.scroll),
        _ => panic!("diff overlay gone"),
    };

    // The diff pane first: a file switch reloads the fake view's diff.
    handle_mouse(&mut app, mev(MouseEventKind::ScrollDown, 60, 5), &mut out);
    assert_eq!(
        state(&app),
        (0, MODAL_WHEEL_LINES as u16),
        "the diff pane's wheel scrolls the diff, not the files"
    );

    handle_mouse(&mut app, mev(MouseEventKind::ScrollDown, 10, 5), &mut out);
    assert_eq!(
        state(&app),
        (1, 0),
        "wheel down over the files selects the next"
    );
    handle_mouse(&mut app, mev(MouseEventKind::ScrollDown, 10, 5), &mut out);
    assert_eq!(state(&app).0, 1, "clamps at the last file");
    handle_mouse(&mut app, mev(MouseEventKind::ScrollUp, 10, 1), &mut out);
    assert_eq!(
        state(&app).0,
        0,
        "wheel up (filter row too) selects the previous"
    );
    assert!(out.is_empty());
}

/// Ctrl+d / Ctrl+u move the file cursor half the list's height, vim
/// style, in the flat list and in the tree alike, and leave the diff's
/// scroll to PgUp/PgDn.
#[test]
fn diff_modal_ctrl_d_u_half_the_file_list() {
    use crate::git_diff::DiffFile;
    let files = (0..30)
        .map(|i| DiffFile {
            path: format!("src/f{i:02}.rs"),
            orig_path: None,
            xy: ['M', ' '],
        })
        .collect();
    let mut view = DiffView::new(
        "/nonexistent-nebula-diff-test".into(),
        "main".into(),
        files,
        true,
    );
    view.list_area = ratatui::layout::Rect::new(1, 2, 28, 10);
    let mut app = App::new();
    seed_tree(&mut app);
    app.overlay = Some(Overlay::Diff(view));
    let mut out = Vec::new();
    let cursor = |app: &App| match &app.overlay {
        Some(Overlay::Diff(v)) => v.cursor(),
        _ => panic!("diff overlay gone"),
    };
    let ctrl = |app: &mut App, c: char, out: &mut Vec<ClientRequest>| {
        press(app, KeyCode::Char(c), KeyModifiers::CONTROL, out)
    };

    ctrl(&mut app, 'd', &mut out);
    assert_eq!(cursor(&app), 5, "flat: Ctrl+d is half the list's 10 rows");
    ctrl(&mut app, 'd', &mut out);
    ctrl(&mut app, 'u', &mut out);
    assert_eq!(cursor(&app), 5, "flat: Ctrl+u comes back up half");
    for _ in 0..10 {
        ctrl(&mut app, 'd', &mut out);
    }
    assert_eq!(cursor(&app), 29, "clamps on the last file");

    ctrl(&mut app, 't', &mut out);
    let Some(Overlay::Diff(v)) = &mut app.overlay else {
        panic!("diff overlay gone")
    };
    assert!(v.tree.is_some(), "Ctrl+t showed the tree");
    v.select(0);
    ctrl(&mut app, 'd', &mut out);
    assert_eq!(cursor(&app), 5, "tree: Ctrl+d is half the list too");
    ctrl(&mut app, 'u', &mut out);
    assert_eq!(cursor(&app), 0, "tree: Ctrl+u back to the top");
    assert!(out.is_empty());
}

#[test]
fn diff_modal_type_to_filter() {
    let mut app = App::new();
    seed_tree(&mut app);
    app.overlay = Some(Overlay::Diff(fake_diff_view(10)));
    let mut out = Vec::new();
    let view = |app: &App| match &app.overlay {
        Some(Overlay::Diff(v)) => v.clone(),
        _ => panic!("diff overlay gone"),
    };

    // Typing narrows to the fuzzy matches; the diff reload against the
    // fake root yields an error body, which must not panic.
    press(&mut app, KeyCode::Char('b'), KeyModifiers::NONE, &mut out);
    let v = view(&app);
    assert_eq!(v.filter, "b");
    assert_eq!(v.matches.len(), 1, "only beta.rs matches");
    assert_eq!(v.selected_file().unwrap().path, "beta.rs");

    // Uppercase (SHIFT-modified) chars land in the filter too, and the
    // match is case-insensitive.
    press(&mut app, KeyCode::Char('T'), KeyModifiers::SHIFT, &mut out);
    let v = view(&app);
    assert_eq!(v.filter, "bT");
    assert_eq!(v.matches.len(), 1, "bT still fuzzy-matches beta.rs");

    // A dead-end query empties the list without panicking.
    press(&mut app, KeyCode::Char('z'), KeyModifiers::NONE, &mut out);
    let v = view(&app);
    assert!(v.matches.is_empty(), "no file matches bTz");
    assert!(v.selected_file().is_none());
    assert_eq!(v.diff, "", "no selection clears the diff pane");

    // Backspace restores the previous narrowing.
    press(&mut app, KeyCode::Backspace, KeyModifiers::NONE, &mut out);
    assert_eq!(view(&app).matches.len(), 1);

    // First Esc clears the filter, second closes.
    press(&mut app, KeyCode::Esc, KeyModifiers::NONE, &mut out);
    let v = view(&app);
    assert_eq!(v.filter, "", "Esc clears the filter first");
    assert_eq!(v.matches.len(), 2, "full list restored in git order");
    assert_eq!(v.selected_file().unwrap().path, "alpha.rs");
    press(&mut app, KeyCode::Esc, KeyModifiers::NONE, &mut out);
    assert!(app.overlay.is_none(), "second Esc closes the modal");
    assert!(out.is_empty(), "filtering never talks to the daemon");
}

/// The current diff view, or panic.
fn diff_view(app: &App) -> &crate::app::DiffView {
    match &app.overlay {
        Some(Overlay::Diff(v)) => v,
        other => panic!("expected diff overlay, got {other:?}"),
    }
}

/// The visible file list in display order.
fn diff_order(app: &App) -> Vec<String> {
    let v = diff_view(app);
    v.matches
        .iter()
        .map(|m| v.files[m.file].path.clone())
        .collect()
}

#[test]
fn ctrl_r_toggles_reviewed_and_marks_survive_reopen() {
    let dir = tempfile::tempdir().unwrap();
    crate::review::with_store_path(dir.path().join("reviewed.json"), || {
        let repo = test_repo(&dir);
        std::fs::write(repo.join("a.txt"), "changed\n").unwrap();
        std::fs::write(repo.join("z.txt"), "fresh\n").unwrap();

        let mut app = App::new();
        seed_repo_tree(&mut app, &repo);
        let mut out = Vec::new();
        press(&mut app, KeyCode::Char('g'), KeyModifiers::NONE, &mut out);
        // Status is path-ordered, so a.txt is the selected file. Marking
        // sinks it below z.txt and advances to the next file.
        press(
            &mut app,
            KeyCode::Char('r'),
            KeyModifiers::CONTROL,
            &mut out,
        );
        let v = diff_view(&app);
        assert!(v.reviewed.contains_key("a.txt"), "{:?}", v.reviewed);
        assert!(!v.head_key.is_empty(), "head OID captured for scoping");
        assert_eq!(diff_order(&app), ["z.txt", "a.txt"], "reviewed sinks");
        let v = diff_view(&app);
        assert_eq!(v.selected_file().unwrap().path, "z.txt", "auto-advance");
        assert!(
            v.diff.contains("+fresh"),
            "next file's diff loaded: {}",
            v.diff
        );

        // Reopen: the mark comes back from the store, already sunk, and
        // the first unreviewed file starts selected.
        press(&mut app, KeyCode::Esc, KeyModifiers::NONE, &mut out);
        press(&mut app, KeyCode::Char('g'), KeyModifiers::NONE, &mut out);
        assert_eq!(diff_order(&app), ["z.txt", "a.txt"], "restored + sunk");
        assert_eq!(diff_view(&app).selected_file().unwrap().path, "z.txt");

        // Ctrl+r on the reviewed row unmarks it; the file pops back up
        // to git order, stays selected, and the store forgets the mark.
        press(&mut app, KeyCode::Down, KeyModifiers::NONE, &mut out);
        press(
            &mut app,
            KeyCode::Char('r'),
            KeyModifiers::CONTROL,
            &mut out,
        );
        let v = diff_view(&app);
        assert!(v.reviewed.is_empty());
        assert_eq!(diff_order(&app), ["a.txt", "z.txt"], "git order back");
        let v = diff_view(&app);
        assert_eq!(
            v.selected_file().unwrap().path,
            "a.txt",
            "selection follows the unmarked file"
        );
        press(&mut app, KeyCode::Esc, KeyModifiers::NONE, &mut out);
        press(&mut app, KeyCode::Char('g'), KeyModifiers::NONE, &mut out);
        assert!(diff_view(&app).reviewed.is_empty(), "unmark persisted");
        assert!(out.is_empty(), "reviewed marks never talk to the daemon");
    });
}

#[test]
fn editing_a_reviewed_file_drops_its_mark_on_reopen() {
    let dir = tempfile::tempdir().unwrap();
    crate::review::with_store_path(dir.path().join("reviewed.json"), || {
        let repo = test_repo(&dir);
        std::fs::write(repo.join("a.txt"), "changed\n").unwrap();

        let mut app = App::new();
        seed_repo_tree(&mut app, &repo);
        let mut out = Vec::new();
        press(&mut app, KeyCode::Char('g'), KeyModifiers::NONE, &mut out);
        press(
            &mut app,
            KeyCode::Char('r'),
            KeyModifiers::CONTROL,
            &mut out,
        );
        press(&mut app, KeyCode::Esc, KeyModifiers::NONE, &mut out);

        // The approved diff no longer matches what's on disk.
        std::fs::write(repo.join("a.txt"), "changed again\n").unwrap();
        press(&mut app, KeyCode::Char('g'), KeyModifiers::NONE, &mut out);
        assert!(
            diff_view(&app).reviewed.is_empty(),
            "an edited file comes back unreviewed"
        );
    });
}

#[test]
fn a_commit_resets_reviewed_marks() {
    let dir = tempfile::tempdir().unwrap();
    crate::review::with_store_path(dir.path().join("reviewed.json"), || {
        let repo = test_repo(&dir);
        std::fs::write(repo.join("a.txt"), "changed\n").unwrap();

        let mut app = App::new();
        seed_repo_tree(&mut app, &repo);
        let mut out = Vec::new();
        press(&mut app, KeyCode::Char('g'), KeyModifiers::NONE, &mut out);
        press(
            &mut app,
            KeyCode::Char('r'),
            KeyModifiers::CONTROL,
            &mut out,
        );
        press(&mut app, KeyCode::Esc, KeyModifiers::NONE, &mut out);

        // Commit moves HEAD; the next round of changes starts unreviewed.
        run_git(&repo, &["add", "."]);
        run_git(&repo, &["commit", "-m", "wip"]);
        std::fs::write(repo.join("a.txt"), "post-commit\n").unwrap();
        press(&mut app, KeyCode::Char('g'), KeyModifiers::NONE, &mut out);
        assert!(
            diff_view(&app).reviewed.is_empty(),
            "a commit resets the worktree's marks"
        );
    });
}

#[test]
fn diff_modal_ctrl_u_clears_filter_before_moving() {
    let mut app = App::new();
    seed_tree(&mut app);
    app.overlay = Some(Overlay::Diff(fake_diff_view(100)));
    let mut out = Vec::new();
    let view = |app: &App| match &app.overlay {
        Some(Overlay::Diff(v)) => v.clone(),
        _ => panic!("diff overlay gone"),
    };

    // With nothing typed, Ctrl+u keeps its half-list-up role.
    press(
        &mut app,
        KeyCode::Char('d'),
        KeyModifiers::CONTROL,
        &mut out,
    );
    assert_eq!(view(&app).selected, 1, "Ctrl+d moves down the files");
    press(
        &mut app,
        KeyCode::Char('u'),
        KeyModifiers::CONTROL,
        &mut out,
    );
    assert_eq!(view(&app).selected, 0, "empty filter: Ctrl+u moves up");

    // With a filter typed, Ctrl+u clears it instead of scrolling.
    press(&mut app, KeyCode::Char('b'), KeyModifiers::NONE, &mut out);
    assert_eq!(view(&app).matches.len(), 1, "filter narrows to beta.rs");
    press(
        &mut app,
        KeyCode::Char('u'),
        KeyModifiers::CONTROL,
        &mut out,
    );
    let v = view(&app);
    assert_eq!(v.filter, "", "Ctrl+u clears the filter");
    assert_eq!(v.matches.len(), 2, "full list restored");
    assert!(
        matches!(app.overlay, Some(Overlay::Diff(_))),
        "the modal stays open"
    );
    assert!(out.is_empty(), "filtering never talks to the daemon");
}

#[test]
fn diff_filter_sorts_best_match_first() {
    use crate::git_diff::DiffFile;
    let file = |path: &str| DiffFile {
        path: path.into(),
        orig_path: None,
        xy: ['M', ' '],
    };
    let mut view = DiffView::new(
        "/nonexistent-nebula-diff-test".into(),
        "main".into(),
        vec![file("build.rs"), file("src/ui.rs")],
        true,
    );
    view.filter = "ui".into();
    view.apply_filter();
    assert_eq!(view.matches.len(), 2);
    // Segment-start match on src/ui.rs outranks the mid-word one in
    // build.rs despite git order listing build.rs first.
    assert_eq!(view.selected_file().unwrap().path, "src/ui.rs");
}

#[test]
fn diff_modal_renders_two_panes() {
    let mut app = App::new();
    seed_tree(&mut app);
    let mut view = fake_diff_view(4);
    view.diff = "diff --git a/a.rs b/a.rs\n@@ -1,2 +1,2 @@\n-old line\n+new line".into();
    view.diff_line_count = 4;
    app.overlay = Some(Overlay::Diff(view));

    let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
    terminal.draw(|f| ui::draw(f, &mut app)).unwrap();
    let text = buffer_text(&terminal);
    assert!(text.contains("Files (2)"), "file pane title:\n{text}");
    assert!(text.contains("alpha.rs"), "file row:\n{text}");
    assert!(text.contains("type to filter"), "filter row:\n{text}");
    assert!(text.contains("+new line"), "diff body:\n{text}");
    assert!(text.contains("type: filter"), "footer hint:\n{text}");
    match &app.overlay {
        Some(Overlay::Diff(v)) => {
            assert!(v.view_height > 0, "view_height written back during draw")
        }
        _ => panic!("diff overlay gone"),
    }
}

#[test]
fn diff_modal_swallows_mouse_and_wheel_scrolls() {
    let mut app = App::new();
    seed_tree(&mut app);
    app.focus = Focus::Projects;
    app.overlay = Some(Overlay::Diff(fake_diff_view(100)));
    let mut out = Vec::new();

    let wheel = MouseEvent {
        kind: MouseEventKind::ScrollDown,
        column: 50,
        row: 10,
        modifiers: KeyModifiers::NONE,
    };
    handle_mouse(&mut app, wheel, &mut out);
    match &app.overlay {
        Some(Overlay::Diff(v)) => assert_eq!(v.scroll, 3, "wheel scrolls the diff"),
        _ => panic!("diff overlay gone"),
    }

    let (focus_before, sel_before) = (app.focus, app.sel_project);
    let click = MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: 2,
        row: 1,
        modifiers: KeyModifiers::NONE,
    };
    handle_mouse(&mut app, click, &mut out);
    assert!(
        matches!(app.overlay, Some(Overlay::Diff(_))),
        "clicks do not close the modal"
    );
    assert_eq!(app.focus, focus_before, "clicks do not change focus");
    assert_eq!(app.sel_project, sel_before);
    assert!(out.is_empty(), "mouse in the modal sends nothing");
}

#[test]
fn diff_modal_click_selects_file_row() {
    let mut app = App::new();
    seed_tree(&mut app);
    app.overlay = Some(Overlay::Diff(fake_diff_view(4)));
    let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
    terminal.draw(|f| ui::draw(f, &mut app)).unwrap();
    let area = match &app.overlay {
        Some(Overlay::Diff(v)) => v.list_area,
        _ => panic!("diff overlay gone"),
    };
    assert!(
        area.height >= 2,
        "list area written back during draw: {area:?}"
    );

    let mut out = Vec::new();
    // Click the second row: beta.rs becomes the selection and its diff
    // loads (the fake root makes that an error string, still a reload).
    handle_mouse(
        &mut app,
        mev(
            MouseEventKind::Down(MouseButton::Left),
            area.x + 2,
            area.y + 1,
        ),
        &mut out,
    );
    match &app.overlay {
        Some(Overlay::Diff(v)) => {
            assert_eq!(v.selected, 1);
            assert_eq!(v.selected_file().unwrap().path, "beta.rs");
            assert_eq!(v.scroll, 0, "reload resets the scroll");
        }
        _ => panic!("diff overlay gone"),
    }

    // A click below the last populated row is a no-op.
    handle_mouse(
        &mut app,
        mev(
            MouseEventKind::Down(MouseButton::Left),
            area.x + 2,
            area.y + area.height - 1,
        ),
        &mut out,
    );
    match &app.overlay {
        Some(Overlay::Diff(v)) => assert_eq!(v.selected, 1, "empty-row click ignored"),
        _ => panic!("diff overlay gone"),
    }
    assert!(out.is_empty(), "clicks in the modal send nothing");
}

#[test]
fn diff_modal_border_drag_resizes_file_list() {
    let mut app = App::new();
    seed_tree(&mut app);
    app.overlay = Some(Overlay::Diff(fake_diff_view(4)));
    let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
    terminal.draw(|f| ui::draw(f, &mut app)).unwrap();
    let (area, width_before) = match &app.overlay {
        Some(Overlay::Diff(v)) => (v.area, v.files_width),
        _ => panic!("diff overlay gone"),
    };
    assert!(area.width > 0, "modal area written back during draw");
    assert_eq!(width_before, crate::app::DEFAULT_DIFF_FILES_W);

    let bx = area.x + width_before;
    let mut out = Vec::new();
    // Grab the boundary's left border cell and drag 10 columns right.
    handle_mouse(
        &mut app,
        mev(MouseEventKind::Down(MouseButton::Left), bx - 1, area.y + 5),
        &mut out,
    );
    match &app.overlay {
        Some(Overlay::Diff(v)) => {
            assert!(v.files_drag.is_some(), "border click arms the drag");
            assert_eq!(v.selected, 0, "border click selects no row");
        }
        _ => panic!("diff overlay gone"),
    }
    assert!(
        app.mouse_held(),
        "the border is held: no mode re-ask mid-drag"
    );
    handle_mouse(
        &mut app,
        mev(MouseEventKind::Drag(MouseButton::Left), bx + 9, area.y + 5),
        &mut out,
    );
    match &app.overlay {
        Some(Overlay::Diff(v)) => assert_eq!(v.files_width, width_before + 10),
        _ => panic!("diff overlay gone"),
    }
    assert_eq!(
        app.diff_files_width,
        width_before + 10,
        "width remembered for the next open"
    );

    // A drag far past the right edge clamps so the diff pane keeps its
    // minimum; far left clamps to the file-list minimum.
    handle_mouse(
        &mut app,
        mev(MouseEventKind::Drag(MouseButton::Left), area.x + 200, 5),
        &mut out,
    );
    match &app.overlay {
        Some(Overlay::Diff(v)) => {
            assert_eq!(v.files_width, area.width - crate::app::MIN_DIFF_PANE_W)
        }
        _ => panic!("diff overlay gone"),
    }
    handle_mouse(
        &mut app,
        mev(MouseEventKind::Drag(MouseButton::Left), area.x, 5),
        &mut out,
    );
    match &app.overlay {
        Some(Overlay::Diff(v)) => {
            assert_eq!(v.files_width, crate::app::MIN_DIFF_FILES_W)
        }
        _ => panic!("diff overlay gone"),
    }

    handle_mouse(
        &mut app,
        mev(MouseEventKind::Up(MouseButton::Left), area.x, 5),
        &mut out,
    );
    match &app.overlay {
        Some(Overlay::Diff(v)) => assert!(v.files_drag.is_none(), "mouse-up ends the drag"),
        _ => panic!("diff overlay gone"),
    }
    assert!(
        !app.mouse_held(),
        "mouse-up frees the host for the re-ask beat"
    );
    assert!(out.is_empty(), "resizing never talks to the daemon");
}

// ---- `/` fuzzy-search palette ----

/// A second project ("nebula", branch feat-x, session codex-1) next to
/// `seed_tree`'s demo/main/agent-1, plus an archived session on demo.
fn seed_second_project(app: &mut App) {
    use nebula_core::{Agent, AgentStatus, Entity, Project, ProjectId, Worktree, WorktreeId};
    hse(
        app,
        ServerEvent::EntityUpserted {
            entity: Entity::Project(Project {
                id: ProjectId("p2".into()),
                name: "nebula".into(),
                repo_path: "/tmp/nebula".into(),
                sort_order: 1,
            }),
        },
    );
    hse(
        app,
        ServerEvent::EntityUpserted {
            entity: Entity::Worktree(Worktree {
                id: WorktreeId("w2".into()),
                project_id: ProjectId("p2".into()),
                path: "/tmp/nebula".into(),
                branch: "feat-x".into(),
                is_main: true,
                sort_order: 0,
            }),
        },
    );
    hse(
        app,
        ServerEvent::EntityUpserted {
            entity: Entity::Agent(Agent {
                id: AgentId("a2".into()),
                worktree_id: WorktreeId("w2".into()),
                name: "codex-1".into(),
                status: AgentStatus::Fresh,
                archived: false,
                archived_at: 0,
                unseen: false,
                kind: nebula_core::AgentKind::Codex,
                custom_harness: None,
                model: None,
                effort: None,
                session_id: None,
                cloud_session_id: None,
                sort_order: 0,
                status_changed_at: 0,
                alive: true,
                issue_url: None,
                recent_prompts: Vec::new(),
            }),
        },
    );
    hse(
        app,
        ServerEvent::EntityUpserted {
            entity: Entity::Agent(Agent {
                id: AgentId("a3".into()),
                worktree_id: WorktreeId("w1".into()),
                name: "old-1".into(),
                status: AgentStatus::Terminated,
                archived: true,
                archived_at: 0,
                unseen: false,
                kind: nebula_core::AgentKind::Claude,
                custom_harness: None,
                model: None,
                effort: None,
                session_id: None,
                cloud_session_id: None,
                sort_order: 1,
                status_changed_at: 0,
                alive: false,
                issue_url: None,
                recent_prompts: Vec::new(),
            }),
        },
    );
}

fn palette(app: &App) -> &crate::app::Palette {
    match &app.overlay {
        Some(Overlay::Palette(p)) => p,
        other => panic!("expected palette overlay, got {other:?}"),
    }
}

/// Pin the open palette's Enter behavior: `/` snapshots it from the
/// machine's real config.json, which tests must not depend on.
fn set_enter_attaches(app: &mut App, v: bool) {
    match &mut app.overlay {
        Some(Overlay::Palette(p)) => p.enter_attaches = v,
        other => panic!("expected palette overlay, got {other:?}"),
    }
}

#[test]
fn slash_opens_palette_listing_projects_then_worktrees_then_sessions() {
    let mut app = App::new();
    seed_tree(&mut app);
    seed_second_project(&mut app);
    let mut out = Vec::new();
    press(&mut app, KeyCode::Char('/'), KeyModifiers::NONE, &mut out);
    let texts: Vec<&str> = palette(&app)
        .items
        .iter()
        .map(|i| i.text.as_str())
        .collect();
    assert_eq!(
        texts,
        vec![
            "demo",
            "nebula",
            "demo/main",
            "nebula/feat-x",
            "demo/main/agent-1",
            "nebula/feat-x/codex-1",
        ],
        "grouped build order, archived never listed"
    );
    // The empty query is the RECENT SESSIONS list: the sessions alone,
    // flat — the projects and worktrees wait for a query.
    let p = palette(&app);
    let shown: Vec<&str> = p
        .matches
        .iter()
        .map(|m| p.items[m.item].text.as_str())
        .collect();
    assert_eq!(shown, ["demo/main/agent-1", "nebula/feat-x/codex-1"]);
    assert!(out.is_empty(), "opening the palette sends nothing");
}

/// The SESSIONS panel's ARCHIVED toggle has no say over `/`: the
/// archived `old-1` is not a row with the toggle off, and not one with
/// it on either — a released PTY is nothing to jump to.
#[test]
fn palette_never_lists_archived_sessions() {
    for show_archived in [false, true] {
        let mut app = App::new();
        seed_tree(&mut app);
        seed_second_project(&mut app);
        app.show_archived = show_archived;
        let mut out = Vec::new();
        press(&mut app, KeyCode::Char('/'), KeyModifiers::NONE, &mut out);
        let listed: Vec<&str> = palette(&app)
            .items
            .iter()
            .map(|i| i.text.as_str())
            .collect();
        assert!(
            !listed.iter().any(|t| t.ends_with("old-1")),
            "archived row with show_archived={show_archived}: {listed:?}"
        );
    }
}

#[test]
fn palette_typing_filters_best_match_first_and_esc_is_two_stage() {
    let mut app = App::new();
    seed_tree(&mut app);
    seed_second_project(&mut app);
    let mut out = Vec::new();
    press(&mut app, KeyCode::Char('/'), KeyModifiers::NONE, &mut out);
    for c in "main".chars() {
        press(&mut app, KeyCode::Char(c), KeyModifiers::NONE, &mut out);
    }
    {
        let p = palette(&app);
        assert_eq!(p.query, "main");
        let top = &p.items[p.matches[p.selected].item];
        // Same boundary match; the attention order breaks the tie, and
        // for two never-run rows that is build order — the worktree
        // before its session.
        assert_eq!(top.text, "demo/main");
        assert!(p
            .matches
            .iter()
            .all(|m| p.items[m.item].text.contains("main")));
    }
    // First Esc clears the query, second closes.
    press(&mut app, KeyCode::Esc, KeyModifiers::NONE, &mut out);
    assert_eq!(palette(&app).query, "");
    assert!(!palette(&app).matches.is_empty());
    press(&mut app, KeyCode::Esc, KeyModifiers::NONE, &mut out);
    assert!(app.overlay.is_none());
    assert!(out.is_empty(), "browsing the palette sends nothing");
}

#[test]
fn palette_enter_on_session_selects_the_chain_and_attaches() {
    let mut app = App::new();
    seed_tree(&mut app);
    seed_second_project(&mut app);
    let mut out = Vec::new();
    press(&mut app, KeyCode::Char('/'), KeyModifiers::NONE, &mut out);
    set_enter_attaches(&mut app, true);
    for c in "codex".chars() {
        press(&mut app, KeyCode::Char(c), KeyModifiers::NONE, &mut out);
    }
    press(&mut app, KeyCode::Enter, KeyModifiers::NONE, &mut out);

    assert!(app.overlay.is_none());
    assert_eq!(app.selected_project().unwrap().name, "nebula");
    assert_eq!(app.selected_worktree().unwrap().branch, "feat-x");
    assert_eq!(app.selected_session().unwrap().name, "codex-1");
    assert_eq!(app.focus, Focus::Terminal);
    assert!(app.term_locked, "a session pick locks input immediately");
    assert!(
        out.iter()
            .any(|r| matches!(r, ClientRequest::Attach { session, .. }
                    if *session == SessionRef::Agent(AgentId("a2".into())))),
        "a session pick attaches: {out:?}"
    );
}

#[test]
fn palette_enter_only_focuses_the_row_when_auto_attach_is_off() {
    let mut app = App::new();
    seed_tree(&mut app);
    seed_second_project(&mut app);
    let mut out = Vec::new();
    press(&mut app, KeyCode::Char('/'), KeyModifiers::NONE, &mut out);
    set_enter_attaches(&mut app, false);
    for c in "codex".chars() {
        press(&mut app, KeyCode::Char(c), KeyModifiers::NONE, &mut out);
    }
    press(&mut app, KeyCode::Enter, KeyModifiers::NONE, &mut out);

    assert!(app.overlay.is_none());
    assert_eq!(app.selected_session().unwrap().name, "codex-1");
    assert_eq!(
        app.focus,
        Focus::Sessions,
        "lands on the list, not the terminal"
    );
    assert!(!app.term_locked, "no input lock — Enter on the row commits");
    assert!(
        out.iter()
            .any(|r| matches!(r, ClientRequest::Attach { session, .. }
                    if *session == SessionRef::Agent(AgentId("a2".into())))),
        "the pane still previews the picked session: {out:?}"
    );
}

/// A session waiting on you — the red dot — is the one row the setting
/// does not get a say over: the whole point of jumping to a question is
/// to answer it, so Enter attaches and takes the input lock even with
/// auto-attach off.
#[test]
fn palette_enter_attaches_a_red_session_with_auto_attach_off() {
    let mut app = App::new();
    seed_tree(&mut app);
    seed_second_project(&mut app);
    let mut out = Vec::new();
    hse(
        &mut app,
        ServerEvent::StatusChanged {
            agent: AgentId("a2".into()),
            status: nebula_core::AgentStatus::NeedsFeedback,
            changed_at: crate::app::now_ms(),
            unseen: false,
        },
    );
    press(&mut app, KeyCode::Char('/'), KeyModifiers::NONE, &mut out);
    set_enter_attaches(&mut app, false);
    for c in "codex".chars() {
        press(&mut app, KeyCode::Char(c), KeyModifiers::NONE, &mut out);
    }
    press(&mut app, KeyCode::Enter, KeyModifiers::NONE, &mut out);

    assert!(app.overlay.is_none());
    assert_eq!(app.selected_session().unwrap().name, "codex-1");
    assert_eq!(app.focus, Focus::Terminal, "a red row attaches anyway");
    assert!(app.term_locked, "and takes the input lock, ready to answer");
}

/// The override is Enter's alone: `Ctrl+F` asked for the quiet landing
/// by name, so a red session still only lands on its row.
#[test]
fn palette_ctrl_f_still_only_focuses_a_red_session() {
    let mut app = App::new();
    seed_tree(&mut app);
    seed_second_project(&mut app);
    let mut out = Vec::new();
    hse(
        &mut app,
        ServerEvent::StatusChanged {
            agent: AgentId("a2".into()),
            status: nebula_core::AgentStatus::NeedsFeedback,
            changed_at: crate::app::now_ms(),
            unseen: false,
        },
    );
    press(&mut app, KeyCode::Char('/'), KeyModifiers::NONE, &mut out);
    set_enter_attaches(&mut app, true);
    for c in "codex".chars() {
        press(&mut app, KeyCode::Char(c), KeyModifiers::NONE, &mut out);
    }
    press(
        &mut app,
        KeyCode::Char('f'),
        KeyModifiers::CONTROL,
        &mut out,
    );

    assert!(app.overlay.is_none());
    assert_eq!(app.selected_session().unwrap().name, "codex-1");
    assert_eq!(app.focus, Focus::Sessions, "the chord named the landing");
    assert!(!app.term_locked);
}

#[test]
fn palette_ctrl_o_opens_the_session_regardless_of_the_setting() {
    let mut app = App::new();
    seed_tree(&mut app);
    seed_second_project(&mut app);
    let mut out = Vec::new();
    press(&mut app, KeyCode::Char('/'), KeyModifiers::NONE, &mut out);
    set_enter_attaches(&mut app, false);
    for c in "codex".chars() {
        press(&mut app, KeyCode::Char(c), KeyModifiers::NONE, &mut out);
    }
    press(
        &mut app,
        KeyCode::Char('o'),
        KeyModifiers::CONTROL,
        &mut out,
    );

    assert!(app.overlay.is_none());
    assert_eq!(app.selected_session().unwrap().name, "codex-1");
    assert_eq!(app.focus, Focus::Terminal);
    assert!(app.term_locked);
}

#[test]
fn palette_ctrl_f_focuses_the_row_regardless_of_the_setting() {
    let mut app = App::new();
    seed_tree(&mut app);
    seed_second_project(&mut app);
    let mut out = Vec::new();
    press(&mut app, KeyCode::Char('/'), KeyModifiers::NONE, &mut out);
    set_enter_attaches(&mut app, true);
    for c in "codex".chars() {
        press(&mut app, KeyCode::Char(c), KeyModifiers::NONE, &mut out);
    }
    press(
        &mut app,
        KeyCode::Char('f'),
        KeyModifiers::CONTROL,
        &mut out,
    );

    assert!(app.overlay.is_none());
    assert_eq!(app.selected_session().unwrap().name, "codex-1");
    assert_eq!(app.focus, Focus::Sessions);
    assert!(!app.term_locked);
}

// ---- `.` / `,`: the palette's attention order with no modal ----

/// Three more sessions across the two seeded projects, one per
/// attention tier: `ask` waits on you in nebula/feat-x, `run` is
/// mid-turn and `unread` finished unwatched in demo/main — beside the
/// never-run `agent-1` (demo/main) and `codex-1` (nebula/feat-x) the
/// seeds already have, and the archived `old-1` that must stay off the
/// ring.
fn seed_attention_ring(app: &mut App) {
    use nebula_core::{Agent, AgentStatus, Entity, WorktreeId};
    let agent = |id: &str, wt: &str, status: AgentStatus, unseen: bool| Agent {
        id: AgentId(id.into()),
        worktree_id: WorktreeId(wt.into()),
        name: id.into(),
        status,
        archived: false,
        archived_at: 0,
        unseen,
        kind: nebula_core::AgentKind::Claude,
        custom_harness: None,
        model: None,
        effort: None,
        session_id: None,
        cloud_session_id: None,
        sort_order: 5,
        status_changed_at: 500,
        alive: true,
        issue_url: None,
        recent_prompts: Vec::new(),
    };
    for a in [
        agent("ask", "w2", AgentStatus::NeedsFeedback, false),
        agent("run", "w1", AgentStatus::Running, false),
        agent("unread", "w1", AgentStatus::Finished, true),
    ] {
        hse(
            app,
            ServerEvent::EntityUpserted {
                entity: Entity::Agent(a),
            },
        );
    }
}

/// The session names `step` visits from the cursor's current row,
/// `n` presses in a row, with auto-attach off — so every row but a
/// red one only lands.
fn walk_attention(app: &mut App, step: i64, n: usize) -> Vec<String> {
    let mut out = Vec::new();
    (0..n)
        .map(|_| {
            jump_attention(app, step, false, &mut out);
            app.selected_session().expect("landed on a session").name
        })
        .collect()
}

/// `.` walks the ring `/` shows before a query: NEEDS FEEDBACK, then
/// RUNNING, then UNSEEN, then the rest — crossing projects as it goes,
/// skipping the archived row, and wrapping from the bottom to the top.
#[test]
fn next_attention_walks_the_palette_order_and_wraps() {
    let mut app = App::new();
    seed_tree(&mut app);
    seed_second_project(&mut app);
    seed_attention_ring(&mut app);
    app.show_archived = true;
    app.focus = Focus::Sessions;
    app.sel_session = row_of(&app, "a1");

    assert_eq!(
        walk_attention(&mut app, 1, 6),
        ["codex-1", "ask", "run", "unread", "agent-1", "codex-1"]
    );
    assert_eq!(app.selected_project().unwrap().name, "nebula");
    assert_eq!(app.selected_worktree().unwrap().branch, "feat-x");
    assert!(app.flash.is_none(), "flash: {:?}", app.flash);
}

/// `,` is the same ring backwards, wrapping from the top to the bottom.
#[test]
fn prev_attention_walks_the_same_ring_backwards() {
    let mut app = App::new();
    seed_tree(&mut app);
    seed_second_project(&mut app);
    seed_attention_ring(&mut app);
    app.focus = Focus::Sessions;
    app.sel_session = row_of(&app, "a1");

    assert_eq!(
        walk_attention(&mut app, -1, 6),
        ["unread", "run", "ask", "codex-1", "agent-1", "unread"]
    );
}

/// With the cursor off the ring — here a worktree with no sessions at
/// all — `.` starts at the top (the row that needs you most) and `,` at
/// the bottom, so the two stay each other's reverse.
#[test]
fn attention_jump_from_off_the_ring_starts_at_either_end() {
    use nebula_core::{Entity, ProjectId, Worktree, WorktreeId};
    let mut app = App::new();
    seed_tree(&mut app);
    seed_second_project(&mut app);
    seed_attention_ring(&mut app);
    hse(
        &mut app,
        ServerEvent::EntityUpserted {
            entity: Entity::Worktree(Worktree {
                id: WorktreeId("w3".into()),
                project_id: ProjectId("p1".into()),
                path: "/tmp/demo-empty".into(),
                branch: "empty".into(),
                is_main: false,
                sort_order: 1,
            }),
        },
    );
    let mut out = Vec::new();
    let empty = app
        .visible_worktrees()
        .iter()
        .position(|w| w.branch == "empty")
        .unwrap();
    select_worktree_row(&mut app, empty, &mut out);
    assert!(app.selected_session().is_none(), "nothing to start from");

    assert_eq!(walk_attention(&mut app, 1, 1), ["ask"]);

    select_worktree_row(&mut app, empty, &mut out);
    assert_eq!(walk_attention(&mut app, -1, 1), ["codex-1"]);
}

/// The keys are wired: `.` and `,` are the panel chords, live from any
/// panel, and the landing is the palette's — with the default setting
/// on, the pick attaches and locks input exactly like `/` Enter.
#[test]
fn attention_keys_jump_and_land_like_the_palettes_enter() {
    let mut app = App::new();
    seed_tree(&mut app);
    seed_second_project(&mut app);
    seed_attention_ring(&mut app);
    app.focus = Focus::Projects;
    app.sel_session = row_of(&app, "a1");
    let mut out = Vec::new();

    with_default_config(|| {
        press(&mut app, KeyCode::Char('.'), KeyModifiers::NONE, &mut out);
    });
    assert!(app.overlay.is_none(), "no modal opened");
    assert_eq!(app.selected_session().unwrap().name, "codex-1");
    assert_eq!(app.focus, Focus::Terminal);
    assert!(
        app.term_locked,
        "the default landing attaches, like / Enter"
    );
    assert!(
        out.iter()
            .any(|r| matches!(r, ClientRequest::Attach { session, .. }
                    if *session == SessionRef::Agent(AgentId("a2".into())))),
        "the pick attaches: {out:?}"
    );

    // From the locked pane, `,` is forwarded to the agent, so unlock
    // first — then it walks back to where we came from.
    leave_terminal_lock(&mut app);
    out.clear();
    with_default_config(|| {
        press(&mut app, KeyCode::Char(','), KeyModifiers::NONE, &mut out);
    });
    assert_eq!(app.selected_session().unwrap().name, "agent-1");
}

/// `palette_enter_attaches` off lands the cursor on the row and
/// previews it, no input lock — the palette's quieter Enter.
#[test]
fn attention_keys_only_focus_the_row_when_auto_attach_is_off() {
    let mut app = App::new();
    seed_tree(&mut app);
    seed_second_project(&mut app);
    seed_attention_ring(&mut app);
    app.focus = Focus::Sessions;
    app.sel_session = row_of(&app, "a1");
    let mut out = Vec::new();

    with_config_json(r#"{"palette_enter_attaches": false}"#, || {
        press(&mut app, KeyCode::Char('.'), KeyModifiers::NONE, &mut out);
    });
    assert_eq!(app.selected_session().unwrap().name, "codex-1");
    assert_eq!(app.focus, Focus::Sessions, "lands on the list");
    assert!(!app.term_locked, "no input lock");
    assert!(
        out.iter()
            .any(|r| matches!(r, ClientRequest::Attach { session, .. }
                    if *session == SessionRef::Agent(AgentId("a2".into())))),
        "the pane still previews the picked session: {out:?}"
    );
}

/// `.` carries the palette's rule with it: with auto-attach off the
/// never-run row only lands, and the next press — onto the session
/// waiting on you — attaches anyway.
#[test]
fn bracket_keys_attach_a_red_session_with_auto_attach_off() {
    let mut app = App::new();
    seed_tree(&mut app);
    seed_second_project(&mut app);
    seed_attention_ring(&mut app);
    app.focus = Focus::Sessions;
    app.sel_session = row_of(&app, "a1");
    let mut out = Vec::new();

    with_config_json(r#"{"palette_enter_attaches": false}"#, || {
        press(&mut app, KeyCode::Char('.'), KeyModifiers::NONE, &mut out);
    });
    assert_eq!(app.selected_session().unwrap().name, "codex-1");
    assert_eq!(app.focus, Focus::Sessions, "a never-run row only lands");
    assert!(!app.term_locked);

    with_config_json(r#"{"palette_enter_attaches": false}"#, || {
        press(&mut app, KeyCode::Char('.'), KeyModifiers::NONE, &mut out);
    });
    assert_eq!(app.selected_session().unwrap().name, "ask");
    assert_eq!(app.focus, Focus::Terminal, "the red row attaches anyway");
    assert!(app.term_locked);
}

/// The ring spans every project: a RUNNING session in another project
/// outranks the never-run one under the cursor, so `.` goes there —
/// project, worktree and session row all selected — and a second `.`
/// wraps back home.
#[test]
fn next_attention_crosses_into_another_project_and_back() {
    let mut app = App::new();
    seed_tree(&mut app);
    seed_other_project(&mut app);
    seed_background_run(&mut app);
    app.focus = Focus::Sessions;
    app.sel_session = row_of(&app, "a1");
    let mut out = Vec::new();

    jump_attention(&mut app, 1, false, &mut out);
    assert_eq!(app.selected_project().unwrap().name, "secret");
    assert_eq!(app.selected_worktree().unwrap().branch, "main");
    assert_eq!(app.selected_session().unwrap().name, "bg-run");
    assert_eq!(app.focus, Focus::Sessions);
    assert!(
        out.iter()
            .any(|r| matches!(r, ClientRequest::Attach { session, .. }
                    if *session == SessionRef::Agent(AgentId("a9".into())))),
        "the pane previews the session over there: {out:?}"
    );

    out.clear();
    jump_attention(&mut app, 1, false, &mut out);
    assert_eq!(app.selected_project().unwrap().name, "demo");
    assert_eq!(app.selected_session().unwrap().name, "agent-1");
}

/// Landing on an unwatched finish reads it, the way ↑/↓ onto the row
/// does: blue to green, the DONE BADGE counts down, the daemon hears.
#[test]
fn next_attention_reads_the_unseen_finish_it_lands_on() {
    use nebula_core::{AgentStatus, ProjectId, WorktreeId};
    let mut app = App::new();
    seed_tree(&mut app);
    seed_second_agent(&mut app, AgentStatus::Running);
    let a2 = AgentId("a2".into());
    let (w1, p1) = (WorktreeId("w1".into()), ProjectId("p1".into()));
    app.term = Some(AttachedTerm::new(
        SessionRef::Agent(AgentId("a1".into())),
        40,
        10,
    ));
    app.focus = Focus::Sessions;
    app.sel_session = row_of(&app, "a1");
    hse(
        &mut app,
        ServerEvent::StatusChanged {
            agent: a2.clone(),
            status: AgentStatus::Finished,
            changed_at: crate::app::now_ms(),
            unseen: true,
        },
    );
    assert_eq!(app.worktree_unseen(&w1), 1, "one terminal to go read");

    let mut out = Vec::new();
    jump_attention(&mut app, 1, false, &mut out);
    assert_eq!(app.selected_session().unwrap().name, "agent-2");
    assert_eq!(app.worktree_unseen(&w1), 0, "landing on the row reads it");
    assert_eq!(app.project_unseen(&p1), 0);
    assert!(
        out.iter()
            .any(|r| matches!(r, ClientRequest::MarkAgentSeen { id } if *id == a2)),
        "the daemon is told: {out:?}"
    );
}

/// No sessions anywhere: nothing moves, the footer says so.
#[test]
fn next_attention_with_no_sessions_flashes() {
    use nebula_core::{Entity, Project, ProjectId, Worktree, WorktreeId};
    let mut app = App::new();
    hse(
        &mut app,
        ServerEvent::EntityUpserted {
            entity: Entity::Project(Project {
                id: ProjectId("p1".into()),
                name: "demo".into(),
                repo_path: "/tmp/demo".into(),
                sort_order: 0,
            }),
        },
    );
    hse(
        &mut app,
        ServerEvent::EntityUpserted {
            entity: Entity::Worktree(Worktree {
                id: WorktreeId("w1".into()),
                project_id: ProjectId("p1".into()),
                path: "/tmp/demo".into(),
                branch: "main".into(),
                is_main: true,
                sort_order: 0,
            }),
        },
    );
    app.focus = Focus::Sessions;
    let mut out = Vec::new();
    with_default_config(|| {
        press(&mut app, KeyCode::Char('.'), KeyModifiers::NONE, &mut out);
    });
    assert_eq!(app.flash.as_deref(), Some(NO_SESSIONS_TO_JUMP));
    assert_eq!(app.focus, Focus::Sessions);
    assert!(out.is_empty(), "nothing to attach: {out:?}");
}

#[test]
fn palette_enter_on_worktree_previews_its_top_session_without_locking() {
    let mut app = App::new();
    seed_tree(&mut app);
    seed_second_project(&mut app);
    let mut out = Vec::new();
    press(&mut app, KeyCode::Char('/'), KeyModifiers::NONE, &mut out);
    for c in "featx".chars() {
        press(&mut app, KeyCode::Char(c), KeyModifiers::NONE, &mut out);
    }
    press(&mut app, KeyCode::Enter, KeyModifiers::NONE, &mut out);

    assert!(app.overlay.is_none());
    assert_eq!(app.selected_project().unwrap().name, "nebula");
    assert_eq!(app.selected_worktree().unwrap().branch, "feat-x");
    assert_eq!(
        app.focus,
        Focus::Sessions,
        "a worktree pick lands in its Sessions panel, not the Worktrees column"
    );
    // Nothing is remembered on the target worktree, so the switch lands
    // on its top row and previews it, exactly like a manual switch —
    // a preview, not Enter on the row, so the pane is not locked.
    let a2 = SessionRef::Agent(AgentId("a2".into()));
    assert_eq!(app.sel_session, 0);
    assert_eq!(app.term.as_ref().map(|t| t.sref.clone()), Some(a2.clone()));
    assert!(!app.term_locked);
    assert!(
        out.iter()
            .any(|r| matches!(r, ClientRequest::Attach { session, .. } if *session == a2)),
        "the top session previews: {out:?}"
    );
}

#[test]
fn palette_rebuilds_when_the_tree_changes_under_it() {
    use nebula_core::{Entity, EntityId, Project, ProjectId};
    let mut app = App::new();
    seed_tree(&mut app);
    let mut out = Vec::new();
    press(&mut app, KeyCode::Char('/'), KeyModifiers::NONE, &mut out);
    assert_eq!(palette(&app).items.len(), 3);
    // Park the cursor on the session row before the tree churns.
    press(&mut app, KeyCode::Down, KeyModifiers::NONE, &mut out);
    press(&mut app, KeyCode::Down, KeyModifiers::NONE, &mut out);

    hse(
        &mut app,
        ServerEvent::EntityUpserted {
            entity: Entity::Project(Project {
                id: ProjectId("p9".into()),
                name: "fresh".into(),
                repo_path: "/tmp/fresh".into(),
                sort_order: 9,
            }),
        },
    );
    assert!(
        palette(&app).items.iter().any(|i| i.text == "fresh"),
        "an upsert lands in the open palette"
    );
    assert_eq!(
        palette(&app).selected_target(),
        Some(&crate::app::PaletteTarget::Session(AgentId("a1".into()))),
        "a rebuild keeps the cursor on its target"
    );
    hse(
        &mut app,
        ServerEvent::EntityRemoved {
            id: EntityId::Project(ProjectId("p9".into())),
        },
    );
    assert!(
        !palette(&app).items.iter().any(|i| i.text == "fresh"),
        "a removal drops out of the open palette"
    );
}

#[test]
fn palette_renders_with_kind_glyphs_and_column_headers() {
    let mut app = App::new();
    seed_tree(&mut app);
    let mut out = Vec::new();
    press(&mut app, KeyCode::Char('/'), KeyModifiers::NONE, &mut out);
    // Tall enough that the sidebar's headers show past the modal.
    let mut terminal = Terminal::new(TestBackend::new(100, 40)).unwrap();
    terminal.draw(|f| ui::draw(f, &mut app)).unwrap();
    let text = buffer_text(&terminal);
    assert!(text.contains("Jump to"), "palette title rendered:\n{text}");
    assert!(
        text.contains("type to search"),
        "query placeholder rendered:\n{text}"
    );
    // Palette rows carry per-kind glyphs, FLAT: ● the session on a line
    // of its own, the project it lives in in front of it — but not the
    // branch, which is searched and not drawn.
    assert!(
        text.contains("● demo/agent-1"),
        "session row with its project in front:\n{text}"
    );
    assert!(
        !text.contains("demo/main/agent-1"),
        "the branch is searched, not drawn:\n{text}"
    );
    // A query reaches the worktree, ▸, its project in front of it too.
    for c in "main".chars() {
        press(&mut app, KeyCode::Char(c), KeyModifiers::NONE, &mut out);
    }
    terminal.draw(|f| ui::draw(f, &mut app)).unwrap();
    let text = buffer_text(&terminal);
    assert!(text.contains("▸ demo/main"), "worktree glyph row:\n{text}");
    // Rects for mouse hit-testing were written back during the draw.
    assert!(palette(&app).list_area.width > 0);
}

/// A palette row wears the status its panel row wears: the session's
/// own, rolled up for its worktree and project. The status arrives
/// while the palette is open, so the rebuild must carry it through.
#[test]
fn palette_rows_take_their_status_color_and_sweep() {
    use nebula_core::{Agent, AgentStatus, Entity};
    let th = crate::theme::Theme::default();
    let mut app = App::new();
    seed_tree(&mut app);
    let mut out = Vec::new();
    press(&mut app, KeyCode::Char('/'), KeyModifiers::NONE, &mut out);
    hse(
        &mut app,
        ServerEvent::EntityUpserted {
            entity: Entity::Agent(Agent {
                id: AgentId("a1".into()),
                worktree_id: nebula_core::WorktreeId("w1".into()),
                name: "agent-1".into(),
                status: AgentStatus::Running,
                archived: false,
                archived_at: 0,
                unseen: false,
                kind: nebula_core::AgentKind::Claude,
                custom_harness: None,
                model: None,
                effort: None,
                session_id: None,
                cloud_session_id: None,
                sort_order: 0,
                status_changed_at: 0,
                alive: true,
                issue_url: None,
                recent_prompts: Vec::new(),
            }),
        },
    );
    let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
    terminal.draw(|f| ui::draw(f, &mut app)).unwrap();

    // The one running agent lights its own row's glyph…
    let (x, y) = find_cell(&terminal, "● demo/agent-1");
    assert_eq!(
        terminal.backend().buffer()[(x, y)].fg,
        th.warn,
        "the session glyph reads running"
    );
    // ...and its title — its own name, past the dim project crumb —
    // rides the running sweep, not plain text.
    let buffer = terminal.backend().buffer();
    let title_x = x + "● demo/".chars().count() as u16;
    for i in 0.."agent-1".chars().count() as u16 {
        let fg = buffer[(title_x + i, y)].fg;
        assert!(
            th.warn_sweep.contains(&fg),
            "title cell {i} is on the sweep ramp, got {fg:?}"
        );
    }
    // …and so do its worktree's and its project's rollups, once a query
    // reaches those rows.
    for c in "demo".chars() {
        press(&mut app, KeyCode::Char(c), KeyModifiers::NONE, &mut out);
    }
    terminal.draw(|f| ui::draw(f, &mut app)).unwrap();
    for row in ["▪ demo", "▸ demo/main"] {
        let (x, y) = find_cell(&terminal, row);
        assert_eq!(
            terminal.backend().buffer()[(x, y)].fg,
            th.warn,
            "{row:?} glyph reads running"
        );
    }
}

/// Nothing live under a row: the glyph goes hollow and dim, mirroring
/// the panels' `○`.
#[test]
fn palette_rows_with_no_live_status_render_hollow() {
    let th = crate::theme::Theme::default();
    let mut app = App::new();
    seed_tree(&mut app);
    app.tree.agents.clear();
    let mut out = Vec::new();
    press(&mut app, KeyCode::Char('/'), KeyModifiers::NONE, &mut out);
    for c in "demo".chars() {
        press(&mut app, KeyCode::Char(c), KeyModifiers::NONE, &mut out);
    }
    let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
    terminal.draw(|f| ui::draw(f, &mut app)).unwrap();
    let text = buffer_text(&terminal);
    assert!(text.contains("▫ demo"), "hollow project glyph:\n{text}");
    assert!(
        text.contains("▹ demo/main"),
        "hollow worktree glyph:\n{text}"
    );
    // The cursor sits on the first row, under the selection fill that
    // lifts dim to muted; read the worktree row below it for the
    // resting shade.
    let (x, y) = find_cell(&terminal, "▹ demo/main");
    assert_eq!(terminal.backend().buffer()[(x, y)].fg, th.dim);
}

#[test]
fn s_opens_settings_and_esc_closes() {
    let mut app = App::new();
    let mut out = Vec::new();
    press(&mut app, KeyCode::Char('s'), KeyModifiers::NONE, &mut out);
    assert!(matches!(app.overlay, Some(Overlay::Settings(_))));
    press(&mut app, KeyCode::Esc, KeyModifiers::NONE, &mut out);
    assert!(app.overlay.is_none());
}

#[test]
fn s_toggles_settings_closed_like_help() {
    let mut app = App::new();
    let mut out = Vec::new();
    press(&mut app, KeyCode::Char('s'), KeyModifiers::NONE, &mut out);
    press(&mut app, KeyCode::Char('s'), KeyModifiers::NONE, &mut out);
    assert!(app.overlay.is_none());
}

#[test]
fn settings_j_k_move_selection() {
    let mut app = App::new();
    let mut out = Vec::new();
    open_settings_on(&mut app, 0, &mut out);
    press(&mut app, KeyCode::Char('j'), KeyModifiers::NONE, &mut out);
    let Some(Overlay::Settings(view)) = &app.overlay else {
        panic!("settings closed");
    };
    assert_eq!(view.selected, 1);
    press(&mut app, KeyCode::Char('k'), KeyModifiers::NONE, &mut out);
    let Some(Overlay::Settings(view)) = &app.overlay else {
        panic!("settings closed");
    };
    assert_eq!(view.selected, 0);
    press(&mut app, KeyCode::Char('k'), KeyModifiers::NONE, &mut out);
    let Some(Overlay::Settings(view)) = &app.overlay else {
        panic!("settings closed");
    };
    assert_eq!(view.selected, 0, "selection does not wrap");
}

#[test]
fn settings_reopens_on_last_focused_row() {
    let mut app = App::new();
    let mut out = Vec::new();
    open_settings_on(&mut app, 0, &mut out);
    press(&mut app, KeyCode::Char('j'), KeyModifiers::NONE, &mut out);
    press(&mut app, KeyCode::Char('j'), KeyModifiers::NONE, &mut out);
    press(&mut app, KeyCode::Esc, KeyModifiers::NONE, &mut out);
    press(&mut app, KeyCode::Char('s'), KeyModifiers::NONE, &mut out);
    let Some(Overlay::Settings(view)) = &app.overlay else {
        panic!("settings closed");
    };
    assert_eq!(view.selected, 2, "reopen lands on the last focused row");
    assert!(!view.on_tabs, "…in the list, where we left the cursor");
}

/// Nothing visited yet: the strip has the cursor, so ←/→ mean "walk the
/// tabs" the moment the overlay is up. Once the cursor has been parked
/// somewhere, a reopen restores that tab/row/focus instead.
#[test]
fn settings_first_open_lands_on_the_tab_strip() {
    let mut app = App::new();
    let mut out = Vec::new();
    press(&mut app, KeyCode::Char('s'), KeyModifiers::NONE, &mut out);
    assert!(settings_view(&app).on_tabs, "fresh open parks on the strip");
    // ←/→ steer the strip straight away, no ↑ needed first.
    press(&mut app, KeyCode::Right, KeyModifiers::NONE, &mut out);
    assert_eq!(settings_view(&app).tab, 1);
    press(&mut app, KeyCode::Esc, KeyModifiers::NONE, &mut out);
    press(&mut app, KeyCode::Char('s'), KeyModifiers::NONE, &mut out);
    assert_eq!(settings_view(&app).tab, 1, "reopen keeps the tab");
    assert!(settings_view(&app).on_tabs, "…and the strip focus");
    // Drop into the list, leave, come back: the list has it now.
    press(&mut app, KeyCode::Down, KeyModifiers::NONE, &mut out);
    press(&mut app, KeyCode::Esc, KeyModifiers::NONE, &mut out);
    press(&mut app, KeyCode::Char('s'), KeyModifiers::NONE, &mut out);
    assert!(!settings_view(&app).on_tabs, "reopen restores list focus");
}

/// The remembered position has a shelf life. Closed and reopened within
/// `SETTINGS_MEMORY_TTL` it's restored as usual; reopened later than
/// that it's forgotten and the overlay looks exactly like a first open.
#[test]
fn settings_memory_expires_a_minute_after_closing() {
    use crate::app::SETTINGS_MEMORY_TTL;
    let mut app = App::new();
    let mut out = Vec::new();
    assert!(
        app.settings_closed_at.is_none() && !app.settings_memory_expired(),
        "nothing to forget before the first close"
    );
    // Park the cursor somewhere distinctive: tab 1, second row, in the list.
    open_settings_on(&mut app, 1, &mut out);
    press(&mut app, KeyCode::Down, KeyModifiers::NONE, &mut out);
    let (tab, row) = (settings_view(&app).tab, settings_view(&app).selected);
    assert_eq!((tab, row), (1, 1));
    press(&mut app, KeyCode::Esc, KeyModifiers::NONE, &mut out);
    assert!(app.settings_closed_at.is_some(), "Esc stamps the close");

    // Straight back in: everything restored.
    press(&mut app, KeyCode::Char('s'), KeyModifiers::NONE, &mut out);
    assert_eq!(settings_view(&app).tab, 1, "within the minute, the tab");
    assert_eq!(settings_view(&app).selected, 1, "…the row");
    assert!(!settings_view(&app).on_tabs, "…and the list focus survive");
    press(&mut app, KeyCode::Esc, KeyModifiers::NONE, &mut out);

    // Pretend the close was a minute ago.
    app.settings_closed_at = std::time::Instant::now().checked_sub(SETTINGS_MEMORY_TTL);
    assert!(app.settings_memory_expired());
    press(&mut app, KeyCode::Char('s'), KeyModifiers::NONE, &mut out);
    assert_eq!(settings_view(&app).tab, 0, "stale memory: first tab");
    assert_eq!(settings_view(&app).selected, 0, "…top row");
    assert!(settings_view(&app).on_tabs, "…cursor on the strip");
    assert_eq!(
        app.settings_row(1),
        0,
        "the other tabs' rows are forgotten too"
    );
    assert!(
        app.settings_closed_at.is_none(),
        "the stale stamp is cleared, so the visit is a fresh one"
    );
}

/// The reset confirmation swaps the overlay out and back mid-visit;
/// that round trip must not consult the clock, or a stale stamp from an
/// earlier close would reset the cursor under the user's hands.
#[test]
fn settings_reset_round_trip_ignores_the_memory_clock() {
    use crate::app::SETTINGS_MEMORY_TTL;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.json");
    crate::config::with_config_path(path, || {
        let mut app = App::new();
        let mut out = Vec::new();
        open_settings_on(&mut app, 1, &mut out);
        // A stamp that would expire the memory if it were consulted.
        app.settings_closed_at = std::time::Instant::now().checked_sub(SETTINGS_MEMORY_TTL);
        press(&mut app, KeyCode::Char('R'), KeyModifiers::SHIFT, &mut out);
        assert!(matches!(app.overlay, Some(Overlay::Confirm(_))));
        press(&mut app, KeyCode::Esc, KeyModifiers::NONE, &mut out);
        assert_eq!(settings_view(&app).tab, 1, "back on the same tab");
        assert!(!settings_view(&app).on_tabs, "…still in the list");
    });
}

/// A click outside the modal is the other way out, and it starts the
/// same clock as Esc does.
#[test]
fn clicking_outside_settings_stamps_the_close() {
    let mut app = App::new();
    let mut out = Vec::new();
    press(&mut app, KeyCode::Char('s'), KeyModifiers::NONE, &mut out);
    if let Some(Overlay::Settings(view)) = &mut app.overlay {
        view.area = ratatui::layout::Rect::new(10, 5, 40, 20);
    }
    handle_mouse(
        &mut app,
        MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 0,
            row: 0,
            modifiers: KeyModifiers::NONE,
        },
        &mut out,
    );
    assert!(app.overlay.is_none(), "click outside closes");
    assert!(app.settings_closed_at.is_some(), "…and stamps the close");
}

#[test]
fn settings_enter_persists_toggle_to_config_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.json");
    crate::config::with_config_path(path.clone(), || {
        let mut app = App::new();
        let mut out = Vec::new();
        open_settings_on(&mut app, 0, &mut out);
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE, &mut out);
        let cfg = crate::config::Config::load();
        assert!(
            !cfg.palette_enter_attaches,
            "Enter toggles the first setting off"
        );
        let saved: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(saved["palette_enter_attaches"], false);
        assert!(
            matches!(app.overlay, Some(Overlay::Settings(_))),
            "toggle keeps the overlay open"
        );
    });
}

#[test]
fn settings_hl_cycles_session_idle_timeout_and_persists() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.json");
    crate::config::with_config_path(path, || {
        let mut app = App::new();
        let mut out = Vec::new();
        let (tab, row) =
            crate::config::locate(crate::config::SettingKind::SessionIdleTimeout).unwrap();
        open_settings_on(&mut app, tab, &mut out);
        for _ in 0..row {
            press(&mut app, KeyCode::Char('j'), KeyModifiers::NONE, &mut out);
        }
        press(&mut app, KeyCode::Char('l'), KeyModifiers::NONE, &mut out);
        let cfg = crate::config::Config::load();
        assert_eq!(cfg.session_idle_timeout, "15m");
        press(&mut app, KeyCode::Char('h'), KeyModifiers::NONE, &mut out);
        let cfg = crate::config::Config::load();
        assert_eq!(cfg.session_idle_timeout, "5m");
    });
}

/// The WORKTREE BASE BRANCH row is typed, not cycled: Enter opens a
/// prompt in the overlay's place, pre-filled with the stored value;
/// Enter there saves the name and lands back on the same row, Esc
/// keeps the old value and lands there too, and an empty Enter puts
/// `auto` back. ←/→ only explain themselves.
#[test]
fn settings_worktree_base_branch_is_typed_through_a_prompt() {
    use crate::config::SettingKind::WorktreeBaseBranch;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.json");
    crate::config::with_config_path(path.clone(), || {
        let mut app = App::new();
        let mut out = Vec::new();
        let (tab, row) = crate::config::locate(WorktreeBaseBranch).unwrap();
        open_settings_on(&mut app, tab, &mut out);
        for _ in 0..row {
            press(&mut app, KeyCode::Char('j'), KeyModifiers::NONE, &mut out);
        }
        let saved = |path: &std::path::Path| -> serde_json::Value {
            serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
        };

        // ←/→: nothing to cycle, the overlay stays and explains.
        press(&mut app, KeyCode::Char('l'), KeyModifiers::NONE, &mut out);
        let view = settings_view(&app);
        assert_eq!(view.selected, row);
        assert!(
            view.notice
                .as_ref()
                .is_some_and(|(t, _)| t.contains("Enter")),
            "→ on a typed row says Enter types it: {:?}",
            view.notice
        );
        assert_eq!(crate::config::Config::load().worktree_base_branch, "");

        // Enter: a prompt in the overlay's place, empty like the value.
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE, &mut out);
        let Some(Overlay::Prompt(prompt)) = &app.overlay else {
            panic!("expected the branch prompt, got {:?}", app.overlay);
        };
        assert_eq!(prompt.title, "Worktree base branch");
        assert_eq!(prompt.input.as_str(), "");
        assert!(
            matches!(prompt.kind, PromptKind::SettingText { kind, .. } if kind == WorktreeBaseBranch)
        );

        type_text(&mut app, "master", &mut out);
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE, &mut out);
        assert!(out.is_empty(), "a setting is a file write, not a request");
        assert_eq!(crate::config::Config::load().worktree_base_branch, "master");
        assert_eq!(saved(&path)["worktree_base_branch"], "master");
        let view = settings_view(&app);
        assert_eq!((view.tab, view.selected, view.on_tabs), (tab, row, false));
        assert!(
            view.notice
                .as_ref()
                .is_some_and(|(t, _)| t.contains("master")),
            "the notice shows the saved value: {:?}",
            view.notice
        );

        // Reopened, the prompt carries the stored value; Esc keeps it.
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE, &mut out);
        let Some(Overlay::Prompt(prompt)) = &app.overlay else {
            panic!("expected the branch prompt, got {:?}", app.overlay);
        };
        assert_eq!(prompt.input.as_str(), "master");
        type_text(&mut app, "-typo", &mut out);
        press(&mut app, KeyCode::Esc, KeyModifiers::NONE, &mut out);
        assert_eq!(crate::config::Config::load().worktree_base_branch, "master");
        let view = settings_view(&app);
        assert_eq!((view.tab, view.selected), (tab, row));

        // An empty Enter is the way back to auto — stored as "".
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE, &mut out);
        for _ in 0.."master".len() {
            press(&mut app, KeyCode::Backspace, KeyModifiers::NONE, &mut out);
        }
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE, &mut out);
        assert!(out.is_empty());
        assert_eq!(crate::config::Config::load().worktree_base_branch, "");
        assert_eq!(saved(&path)["worktree_base_branch"], "");
        let view = settings_view(&app);
        assert_eq!(view.selected, row);
        assert!(
            view.notice
                .as_ref()
                .is_some_and(|(t, _)| t.contains("auto")),
            "back to auto: {:?}",
            view.notice
        );
    });
}

#[test]
fn settings_overlay_renders_labels() {
    let mut app = App::new();
    let mut out = Vec::new();
    press(&mut app, KeyCode::Char('s'), KeyModifiers::NONE, &mut out);
    let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
    terminal.draw(|f| ui::draw(f, &mut app)).unwrap();
    let text = buffer_text(&terminal);
    assert!(text.contains("Settings"), "title rendered:\n{text}");
    assert!(
        text.contains("Search Enter attaches"),
        "bool setting rendered:\n{text}"
    );
    // Settings live on their own tab now, so a row from another tab
    // is only on screen once you switch to it.
    assert!(
        !text.contains("Idle session timeout"),
        "another tab's rows stay off screen:\n{text}"
    );
    press(&mut app, KeyCode::Tab, KeyModifiers::NONE, &mut out);
    terminal.draw(|f| ui::draw(f, &mut app)).unwrap();
    let sessions_text = buffer_text(&terminal);
    assert!(
        sessions_text.contains("Idle session timeout"),
        "Tab reaches the Sessions tab:\n{sessions_text}"
    );
    for tab in crate::config::SETTINGS_TABS {
        assert!(text.contains(tab.title), "tab strip rendered:\n{text}");
    }
    assert!(
        text.contains("Enter in / search opens the session"),
        "selected setting's hint shown in the footer:\n{text}"
    );
    let Some(Overlay::Settings(view)) = &app.overlay else {
        panic!("settings closed");
    };
    assert!(view.area.width > 0, "draw writes hit-test area");
    assert_eq!(
        view.tab_hits.len(),
        crate::config::tab_count(),
        "draw records a click target per tab"
    );
}

#[test]
fn agents_tab_renders_its_harness_groups() {
    use crate::config::{locate_agent, HarnessField};
    let mut app = App::new();
    let mut out = Vec::new();
    let (agents, _) = locate_agent("claude", HarnessField::Enabled).unwrap();
    open_settings_on(&mut app, agents, &mut out);
    // Tall enough for every row, so nothing scrolls off.
    let mut terminal = Terminal::new(TestBackend::new(100, 40)).unwrap();
    terminal.draw(|f| ui::draw(f, &mut app)).unwrap();
    let text = buffer_text(&terminal);

    // Each header sits above its own rows, in order, and the harness
    // name is no longer repeated on every row under it.
    let mut pos = 0;
    for needle in [
        "Quick prompt",
        "Agent",
        "Focus",
        "Follow new",
        "New worktree",
        "Hide missing CLIs",
        "Claude",
        "Enabled",
        "Model",
        "Effort",
        "Codex",
        "Enabled",
        "Model",
        "Effort",
        "Cursor",
        "Enabled",
        "Model",
        "Effort",
    ] {
        let at = text[pos..]
            .find(needle)
            .unwrap_or_else(|| panic!("{needle} after column {pos}:\n{text}"));
        pos += at + needle.len();
    }
    assert!(
        !text.contains("Claude model") && !text.contains("Quick prompt agent"),
        "the old flat labels are gone:\n{text}"
    );

    // Headers and blanks are not rows the cursor can land on: five ↓
    // from the first row reach Claude's Enabled row, not a header.
    press(&mut app, KeyCode::Char('j'), KeyModifiers::NONE, &mut out);
    press(&mut app, KeyCode::Char('j'), KeyModifiers::NONE, &mut out);
    press(&mut app, KeyCode::Char('j'), KeyModifiers::NONE, &mut out);
    press(&mut app, KeyCode::Char('j'), KeyModifiers::NONE, &mut out);
    press(&mut app, KeyCode::Char('j'), KeyModifiers::NONE, &mut out);
    let (_, claude_enabled) = locate_agent("claude", HarnessField::Enabled).unwrap();
    assert_eq!(settings_view(&app).selected, claude_enabled);

    // Nor can a click land on one: the "Codex" header is a dead cell.
    let (x, y) = find_cell(&terminal, "Codex");
    click(&mut app, x, y, &mut out);
    assert_eq!(settings_view(&app).selected, claude_enabled);
}

// ---- settings tabs & hotkeys ----

/// Open the settings overlay parked on `tab`, cursor down in the list.
/// A fresh overlay opens on the tab strip, so the ↓ is what these tests
/// mean by "parked on the tab".
fn open_settings_on(app: &mut App, tab: usize, out: &mut Vec<ClientRequest>) {
    press(app, KeyCode::Char('s'), KeyModifiers::NONE, out);
    for _ in 0..tab {
        press(app, KeyCode::Tab, KeyModifiers::NONE, out);
    }
    press(app, KeyCode::Down, KeyModifiers::NONE, out);
}

fn settings_view(app: &App) -> &crate::app::SettingsView {
    match &app.overlay {
        Some(Overlay::Settings(view)) => view,
        _ => panic!("settings closed"),
    }
}

#[test]
fn tab_and_backtab_walk_the_strip_and_wrap() {
    let mut app = App::new();
    let mut out = Vec::new();
    let tabs = crate::config::tab_count();
    press(&mut app, KeyCode::Char('s'), KeyModifiers::NONE, &mut out);
    assert_eq!(settings_view(&app).tab, 0);
    for i in 1..tabs {
        press(&mut app, KeyCode::Tab, KeyModifiers::NONE, &mut out);
        assert_eq!(settings_view(&app).tab, i);
    }
    press(&mut app, KeyCode::Tab, KeyModifiers::NONE, &mut out);
    assert_eq!(settings_view(&app).tab, 0, "Tab wraps round the strip");
    press(&mut app, KeyCode::BackTab, KeyModifiers::SHIFT, &mut out);
    assert_eq!(settings_view(&app).tab, tabs - 1, "⇧Tab wraps back");
}

#[test]
fn digits_jump_straight_to_a_tab() {
    let mut app = App::new();
    let mut out = Vec::new();
    press(&mut app, KeyCode::Char('s'), KeyModifiers::NONE, &mut out);
    press(&mut app, KeyCode::Char('3'), KeyModifiers::NONE, &mut out);
    assert_eq!(settings_view(&app).tab, 2);
    // A digit past the last tab is ignored rather than clamped.
    press(&mut app, KeyCode::Char('9'), KeyModifiers::NONE, &mut out);
    assert_eq!(settings_view(&app).tab, 2);
}

/// The arrows do double duty: cycling a value inside the list, walking
/// the tabs once the cursor has stepped up onto the strip.
#[test]
fn up_from_the_top_row_parks_on_the_strip_where_arrows_move_tabs() {
    let dir = tempfile::tempdir().unwrap();
    crate::config::with_config_path(dir.path().join("config.json"), || {
        let mut app = App::new();
        let mut out = Vec::new();
        // The Editor row, found by name: the General tab's rows above
        // it are free to change.
        let (tab, editor_row) = crate::config::locate(crate::config::SettingKind::Editor).unwrap();
        assert_eq!(tab, 0);
        open_settings_on(&mut app, tab, &mut out);
        assert!(!settings_view(&app).on_tabs);
        // In the list, → cycles the selected setting's value.
        for _ in 0..editor_row {
            press(&mut app, KeyCode::Char('j'), KeyModifiers::NONE, &mut out);
        }
        press(&mut app, KeyCode::Right, KeyModifiers::NONE, &mut out);
        assert_eq!(crate::config::Config::load().editor, "nvim");
        assert_eq!(settings_view(&app).tab, 0, "→ did not move the tab");

        // ↑ off the top row steps onto the strip; now → is the tab.
        for _ in 0..=editor_row {
            press(&mut app, KeyCode::Up, KeyModifiers::NONE, &mut out);
        }
        assert!(settings_view(&app).on_tabs, "↑ off the top row parks here");
        press(&mut app, KeyCode::Right, KeyModifiers::NONE, &mut out);
        assert_eq!(settings_view(&app).tab, 1);
        assert_eq!(
            crate::config::Config::load().editor,
            "nvim",
            "no value was cycled while the strip had focus"
        );
        // ↓ drops back into the list.
        press(&mut app, KeyCode::Down, KeyModifiers::NONE, &mut out);
        assert!(!settings_view(&app).on_tabs);
    });
}

#[test]
fn each_tab_remembers_its_own_cursor_row() {
    let mut app = App::new();
    let mut out = Vec::new();
    open_settings_on(&mut app, 0, &mut out);
    press(&mut app, KeyCode::Char('j'), KeyModifiers::NONE, &mut out);
    press(&mut app, KeyCode::Char('j'), KeyModifiers::NONE, &mut out);
    assert_eq!(settings_view(&app).selected, 2);
    press(&mut app, KeyCode::Tab, KeyModifiers::NONE, &mut out);
    assert_eq!(settings_view(&app).selected, 0, "a fresh tab starts at 0");
    press(&mut app, KeyCode::Char('j'), KeyModifiers::NONE, &mut out);
    press(&mut app, KeyCode::BackTab, KeyModifiers::SHIFT, &mut out);
    assert_eq!(settings_view(&app).selected, 2, "back where we left it");
    press(&mut app, KeyCode::Esc, KeyModifiers::NONE, &mut out);
    press(&mut app, KeyCode::Char('s'), KeyModifiers::NONE, &mut out);
    assert_eq!(settings_view(&app).selected, 2, "and across a reopen");
}

#[test]
fn hotkeys_tab_lists_every_action_with_its_chords() {
    let mut app = App::new();
    let mut out = Vec::new();
    open_settings_on(&mut app, crate::config::hotkeys_tab(), &mut out);
    let mut terminal = Terminal::new(TestBackend::new(100, 40)).unwrap();
    terminal.draw(|f| ui::draw(f, &mut app)).unwrap();
    let text = buffer_text(&terminal);
    assert!(text.contains("Hotkeys"), "tab strip:\n{text}");
    assert!(text.contains("NAVIGATE"), "group header:\n{text}");
    assert!(
        text.contains("Open / fold checkout"),
        "an action label:\n{text}"
    );
    assert!(
        text.contains("Open / fold checkout        Tab"),
        "its chord, in the value column:\n{text}"
    );
}

/// The headline of the whole tab: press Enter, press a key, and that
/// key now drives the action — through the config file, not just in
/// memory.
#[test]
fn rebinding_an_action_takes_effect_and_persists() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.json");
    crate::config::with_config_path(path.clone(), || {
        let mut app = App::new();
        let mut out = Vec::new();
        open_settings_on(&mut app, crate::config::hotkeys_tab(), &mut out);
        let row = crate::keymap::index_of(crate::keymap::Action::Help).unwrap();
        for _ in 0..row {
            press(&mut app, KeyCode::Char('j'), KeyModifiers::NONE, &mut out);
        }
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE, &mut out);
        assert!(settings_view(&app).capturing(), "waiting for a key");
        press(&mut app, KeyCode::F(6), KeyModifiers::NONE, &mut out);
        assert!(
            !settings_view(&app).capturing(),
            "the press was the binding"
        );
        assert_eq!(app.keymap.label(crate::keymap::Action::Help), "F6");

        let saved: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(saved["keybindings"]["help"], "f6");

        // And the new key actually opens help from the panels.
        press(&mut app, KeyCode::Esc, KeyModifiers::NONE, &mut out);
        assert!(app.overlay.is_none());
        press(&mut app, KeyCode::F(6), KeyModifiers::NONE, &mut out);
        assert!(matches!(app.overlay, Some(Overlay::Help(_))));
        // …and the old one no longer does.
        let mut fresh = App::new();
        fresh.keymap = crate::config::Config::load().keymap();
        press(&mut fresh, KeyCode::Char('?'), KeyModifiers::NONE, &mut out);
        assert!(fresh.overlay.is_none(), "? is unbound now");
    });
}

#[test]
fn a_duplicate_chord_warns_before_it_is_taken() {
    let dir = tempfile::tempdir().unwrap();
    crate::config::with_config_path(dir.path().join("config.json"), || {
        let mut app = App::new();
        let mut out = Vec::new();
        open_settings_on(&mut app, crate::config::hotkeys_tab(), &mut out);
        let row = crate::keymap::index_of(crate::keymap::Action::Help).unwrap();
        for _ in 0..row {
            press(&mut app, KeyCode::Char('j'), KeyModifiers::NONE, &mut out);
        }
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE, &mut out);
        // `g` is Git diff's — capturing it must not silently steal it.
        press(&mut app, KeyCode::Char('g'), KeyModifiers::NONE, &mut out);
        let view = settings_view(&app);
        let (text, level) = view.notice.clone().expect("a warning");
        assert_eq!(level, crate::app::NoticeLevel::Warn);
        assert!(text.contains("already"), "{text}");
        assert!(text.contains("Git diff"), "names the current owner: {text}");
        assert!(!view.capturing(), "the capture is paused on the warning");
        assert_eq!(
            app.keymap.label(crate::keymap::Action::Help),
            "?",
            "nothing changed yet"
        );

        // Esc leaves it where it was.
        press(&mut app, KeyCode::Esc, KeyModifiers::NONE, &mut out);
        assert_eq!(app.keymap.label(crate::keymap::Action::GitDiff), "g");
        assert_eq!(app.keymap.label(crate::keymap::Action::Help), "?");
    });
}

#[test]
fn confirming_a_duplicate_moves_the_chord_off_its_old_action() {
    let dir = tempfile::tempdir().unwrap();
    crate::config::with_config_path(dir.path().join("config.json"), || {
        let mut app = App::new();
        let mut out = Vec::new();
        open_settings_on(&mut app, crate::config::hotkeys_tab(), &mut out);
        let row = crate::keymap::index_of(crate::keymap::Action::Help).unwrap();
        for _ in 0..row {
            press(&mut app, KeyCode::Char('j'), KeyModifiers::NONE, &mut out);
        }
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE, &mut out);
        press(&mut app, KeyCode::Char('g'), KeyModifiers::NONE, &mut out);
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE, &mut out);
        assert_eq!(app.keymap.label(crate::keymap::Action::Help), "g");
        assert_eq!(
            app.keymap.label(crate::keymap::Action::GitDiff),
            "—",
            "one keystroke can only mean one thing"
        );
        // The panels agree with the map.
        press(&mut app, KeyCode::Esc, KeyModifiers::NONE, &mut out);
        seed_tree(&mut app);
        app.focus = Focus::Worktrees;
        press(&mut app, KeyCode::Char('g'), KeyModifiers::NONE, &mut out);
        assert!(matches!(app.overlay, Some(Overlay::Help(_))));
    });
}

/// nebula is a guest inside Terminal.app / Ghostty, which take some
/// chords before it ever sees them. Binding one is allowed — the user
/// may be on a terminal that delivers it — but never silently.
#[test]
fn binding_a_chord_the_host_terminal_eats_says_so() {
    let dir = tempfile::tempdir().unwrap();
    crate::config::with_config_path(dir.path().join("config.json"), || {
        let mut app = App::new();
        let mut out = Vec::new();
        open_settings_on(&mut app, crate::config::hotkeys_tab(), &mut out);
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE, &mut out);
        press(&mut app, KeyCode::Char(']'), KeyModifiers::SUPER, &mut out);
        let (text, level) = settings_view(&app).notice.clone().expect("a warning");
        assert_eq!(level, crate::app::NoticeLevel::Warn);
        assert!(text.contains('⌘'), "{text}");
        assert_eq!(
            app.keymap.label(crate::keymap::Action::FocusNext),
            "⌘]",
            "warned, not refused"
        );
    });
}

#[test]
fn a_hotkey_row_resets_to_its_default_and_can_be_unbound() {
    let dir = tempfile::tempdir().unwrap();
    crate::config::with_config_path(dir.path().join("config.json"), || {
        let mut app = App::new();
        let mut out = Vec::new();
        open_settings_on(&mut app, crate::config::hotkeys_tab(), &mut out);
        // Row 0 is Next panel (Tab).
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE, &mut out);
        press(&mut app, KeyCode::F(8), KeyModifiers::NONE, &mut out);
        assert_eq!(app.keymap.label(crate::keymap::Action::FocusNext), "F8");
        press(&mut app, KeyCode::Char('x'), KeyModifiers::NONE, &mut out);
        assert_eq!(app.keymap.label(crate::keymap::Action::FocusNext), "—");
        press(&mut app, KeyCode::Backspace, KeyModifiers::NONE, &mut out);
        assert_eq!(app.keymap.label(crate::keymap::Action::FocusNext), "Tab");
        assert!(
            crate::config::Config::load().keybindings.is_empty(),
            "back to the default = nothing left to write down"
        );
    });
}

#[test]
fn adding_an_alternate_keeps_the_original() {
    let dir = tempfile::tempdir().unwrap();
    crate::config::with_config_path(dir.path().join("config.json"), || {
        let mut app = App::new();
        let mut out = Vec::new();
        open_settings_on(&mut app, crate::config::hotkeys_tab(), &mut out);
        press(&mut app, KeyCode::Char('a'), KeyModifiers::NONE, &mut out);
        press(&mut app, KeyCode::F(7), KeyModifiers::NONE, &mut out);
        assert_eq!(app.keymap.label(crate::keymap::Action::FocusNext), "Tab F7");
    });
}

#[test]
fn esc_backs_out_of_a_capture_without_binding_it() {
    let mut app = App::new();
    let mut out = Vec::new();
    open_settings_on(&mut app, crate::config::hotkeys_tab(), &mut out);
    press(&mut app, KeyCode::Enter, KeyModifiers::NONE, &mut out);
    press(&mut app, KeyCode::Esc, KeyModifiers::NONE, &mut out);
    assert!(!settings_view(&app).capturing());
    assert!(
        matches!(app.overlay, Some(Overlay::Settings(_))),
        "Esc left the capture, not the overlay"
    );
    assert_eq!(app.keymap.label(crate::keymap::Action::FocusNext), "Tab");
}

/// A capture swallows the overlay's own keys — otherwise half the
/// keyboard would be unbindable.
#[test]
fn a_capture_takes_keys_the_overlay_would_normally_use() {
    let dir = tempfile::tempdir().unwrap();
    crate::config::with_config_path(dir.path().join("config.json"), || {
        let mut app = App::new();
        let mut out = Vec::new();
        open_settings_on(&mut app, crate::config::hotkeys_tab(), &mut out);
        let row = crate::keymap::index_of(crate::keymap::Action::Metrics).unwrap();
        for _ in 0..row {
            press(&mut app, KeyCode::Char('j'), KeyModifiers::NONE, &mut out);
        }
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE, &mut out);
        // 'q' would close the overlay; here it is just a key.
        press(&mut app, KeyCode::Char('q'), KeyModifiers::NONE, &mut out);
        assert!(
            matches!(app.overlay, Some(Overlay::Settings(_))),
            "the overlay stayed open"
        );
        // It belongs to Quit, so this is the duplicate warning path.
        let (text, _) = settings_view(&app).notice.clone().expect("a warning");
        assert!(text.contains("Quit"), "{text}");
    });
}

#[test]
fn ctrl_q_still_unlocks_a_terminal_after_the_hatch_is_rebound() {
    let dir = tempfile::tempdir().unwrap();
    crate::config::with_config_path(dir.path().join("config.json"), || {
        let mut app = App::new();
        seed_tree(&mut app);
        let mut out = Vec::new();
        // Rebind the unlock action to something else entirely.
        let mut keymap = app.keymap.clone();
        let idx = crate::keymap::index_of(crate::keymap::Action::UnlockTerminal).unwrap();
        keymap.bind(idx, crate::keymap::KeyChord::parse("f4").unwrap(), false);
        app.keymap = keymap;

        app.focus = Focus::Sessions;
        attach_selected(&mut app, &mut out);
        app.term_locked = true;
        assert!(app.term.is_some(), "a live pane to be locked into");
        press(
            &mut app,
            KeyCode::Char('q'),
            KeyModifiers::CONTROL,
            &mut out,
        );
        assert!(!app.term_locked, "^q is wired in, not merely bound");
        assert_eq!(app.focus, Focus::Sessions);

        // And the rebound key works too.
        app.term_locked = true;
        app.focus = Focus::Terminal;
        press(&mut app, KeyCode::F(4), KeyModifiers::NONE, &mut out);
        assert!(!app.term_locked);
    });
}

// ---- settings reset ----

#[test]
fn shift_r_in_settings_asks_first_and_n_goes_back_to_the_overlay() {
    with_default_config(|| {
        let mut app = App::new();
        let mut out = Vec::new();
        press(&mut app, KeyCode::Char('s'), KeyModifiers::NONE, &mut out);
        press(&mut app, KeyCode::Tab, KeyModifiers::NONE, &mut out);
        press(&mut app, KeyCode::Char('R'), KeyModifiers::SHIFT, &mut out);
        let Some(Overlay::Confirm(c)) = &app.overlay else {
            panic!("expected a confirmation, got {:?}", app.overlay);
        };
        assert_eq!(c.title, "Reset settings");
        assert!(matches!(c.action, PendingAction::ResetSettings));
        assert!(c.message.contains("hotkey"), "{}", c.message);

        press(&mut app, KeyCode::Char('n'), KeyModifiers::NONE, &mut out);
        assert!(
            matches!(app.overlay, Some(Overlay::Settings(_))),
            "n returns to the overlay, not the panels: {:?}",
            app.overlay
        );
        assert_eq!(settings_view(&app).tab, 1, "on the tab it was opened from");
        assert!(out.is_empty(), "nothing goes to the daemon");
    });
}

#[test]
fn a_rebound_key_shows_up_in_help_and_the_footer() {
    let dir = tempfile::tempdir().unwrap();
    crate::config::with_config_path(dir.path().join("config.json"), || {
        let mut app = App::new();
        let mut keymap = app.keymap.clone();
        let idx = crate::keymap::index_of(crate::keymap::Action::Hosts).unwrap();
        keymap.bind(idx, crate::keymap::KeyChord::parse("f9").unwrap(), false);
        app.keymap = keymap;

        let mut out = Vec::new();
        press(&mut app, KeyCode::Char('?'), KeyModifiers::NONE, &mut out);
        let mut terminal = Terminal::new(TestBackend::new(120, 40)).unwrap();
        terminal.draw(|f| ui::draw(f, &mut app)).unwrap();
        let text = buffer_text(&terminal);
        assert!(text.contains("F9"), "help follows the keymap:\n{text}");
        assert!(
            !text.contains("H             ssh hosts"),
            "and drops the old key:\n{text}"
        );

        // The first-run footer names the same keys; it follows too.
        press(&mut app, KeyCode::Esc, KeyModifiers::NONE, &mut out);
        terminal.draw(|f| ui::draw(f, &mut app)).unwrap();
        let footer = buffer_text(&terminal);
        assert!(
            footer.contains("F9: ssh host"),
            "footer follows too:\n{footer}"
        );
    });
}

/// The bind-time warning can't see a duplicate somebody typed into the
/// config file by hand, so the row says it too.
#[test]
fn a_hand_edited_duplicate_is_called_out_on_the_row() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.json");
    std::fs::write(&path, r#"{"keybindings": {"help": "g"}}"#).unwrap();
    crate::config::with_config_path(path, || {
        let mut app = App::new();
        app.keymap = crate::config::Config::load().keymap();
        let mut out = Vec::new();
        open_settings_on(&mut app, crate::config::hotkeys_tab(), &mut out);
        let row = crate::keymap::index_of(crate::keymap::Action::GitDiff).unwrap();
        for _ in 0..row {
            press(&mut app, KeyCode::Char('j'), KeyModifiers::NONE, &mut out);
        }
        let mut terminal = Terminal::new(TestBackend::new(100, 40)).unwrap();
        terminal.draw(|f| ui::draw(f, &mut app)).unwrap();
        let text = buffer_text(&terminal);
        assert!(
            text.contains("also belongs to Help"),
            "the row names its rival:\n{text}"
        );
    });
}

#[test]
fn clicking_a_tab_switches_to_it() {
    let mut app = App::new();
    let mut out = Vec::new();
    press(&mut app, KeyCode::Char('s'), KeyModifiers::NONE, &mut out);
    let mut terminal = Terminal::new(TestBackend::new(100, 40)).unwrap();
    terminal.draw(|f| ui::draw(f, &mut app)).unwrap();
    let (area, hits) = {
        let view = settings_view(&app);
        (view.area, view.tab_hits.clone())
    };
    let (x0, _) = hits[2];
    handle_mouse(
        &mut app,
        mev(MouseEventKind::Down(MouseButton::Left), x0, area.y + 1),
        &mut out,
    );
    assert_eq!(settings_view(&app).tab, 2, "clicked the third tab");
}

// ---- `M` metrics modal ----

#[test]
fn metrics_modal_opens_requests_and_renders() {
    let mut app = App::new();
    seed_tree(&mut app);
    let mut out = Vec::new();
    press(&mut app, KeyCode::Char('M'), KeyModifiers::SHIFT, &mut out);
    assert!(matches!(app.overlay, Some(Overlay::Metrics(_))));

    // The keypress itself fires the initial reading's request.
    let req_id = match out.last() {
        Some(ClientRequest::GetMetrics { req_id }) => *req_id,
        other => panic!("expected GetMetrics, got {other:?}"),
    };
    hse(
        &mut app,
        ServerEvent::Metrics {
            req_id,
            snapshot: nebula_core::MetricsSnapshot {
                daemon_pid: 42,
                daemon_rss_bytes: 40 * 1024 * 1024,
                system_total_bytes: 32 * 1024 * 1024 * 1024,
                sessions: vec![nebula_core::SessionMetrics {
                    session: SessionRef::Agent(AgentId("a1".into())),
                    pid: 4321,
                    rss_bytes: 1_610_612_736, // 1.5 GB
                    procs: 3,
                    prewarm: None,
                }],
            },
        },
    );
    assert!(
        app.pending.is_empty(),
        "the Metrics reply must clear its pending slot"
    );
    let Some(Overlay::Metrics(view)) = &app.overlay else {
        panic!("metrics closed");
    };
    assert!(view.snapshot.is_some());

    let mut terminal = Terminal::new(TestBackend::new(100, 40)).unwrap();
    terminal.draw(|f| ui::draw(f, &mut app)).unwrap();
    let text = buffer_text(&terminal);
    assert!(text.contains("Memory"), "title rendered:\n{text}");
    assert!(
        text.contains("1 session · 3 procs"),
        "claude rollup rendered:\n{text}"
    );
    assert!(
        text.contains("agent-1 (claude)") && text.contains("demo/main"),
        "session row joined with the tree:\n{text}"
    );
    assert!(text.contains("1.5 GB"), "subtree memory rendered:\n{text}");
    assert!(
        text.contains("nebula daemon") && text.contains("40 MB"),
        "daemon row rendered:\n{text}"
    );
    assert!(
        text.contains("% of 32 GB installed"),
        "system share rendered:\n{text}"
    );
    let Some(Overlay::Metrics(view)) = &app.overlay else {
        panic!("metrics closed");
    };
    assert!(view.area.width > 0, "draw writes the hit-test area back");

    press(&mut app, KeyCode::Esc, KeyModifiers::NONE, &mut out);
    assert!(app.overlay.is_none());
}

/// Prewarm-pool spares have no agent row; without the home the daemon
/// reports they'd render as "(unknown agent)". They group under one
/// header as a small tree, named by kind/model and placed by worktree,
/// with their own rollup line — and stay out of the live-agent counts.
#[test]
fn metrics_groups_prewarm_spares_under_their_own_header() {
    let mut app = App::new();
    seed_tree(&mut app);
    let mut out = Vec::new();
    press(&mut app, KeyCode::Char('M'), KeyModifiers::SHIFT, &mut out);
    let req_id = match out.last() {
        Some(ClientRequest::GetMetrics { req_id }) => *req_id,
        other => panic!("expected GetMetrics, got {other:?}"),
    };
    let spare = |id: &str, pid: u32, mb: u64, model: Option<&str>| nebula_core::SessionMetrics {
        session: SessionRef::Agent(AgentId(id.into())),
        pid,
        rss_bytes: mb * 1024 * 1024,
        procs: 3,
        prewarm: Some(nebula_core::PrewarmInfo {
            worktree: nebula_core::WorktreeId("w1".into()),
            kind: nebula_core::AgentKind::Claude,
            model: model.map(str::to_string),
        }),
    };
    hse(
        &mut app,
        ServerEvent::Metrics {
            req_id,
            snapshot: nebula_core::MetricsSnapshot {
                daemon_pid: 42,
                daemon_rss_bytes: 40 * 1024 * 1024,
                system_total_bytes: 0,
                sessions: vec![
                    nebula_core::SessionMetrics {
                        session: SessionRef::Agent(AgentId("a1".into())),
                        pid: 4321,
                        rss_bytes: 500 * 1024 * 1024,
                        procs: 4,
                        prewarm: None,
                    },
                    spare("warm-1", 7001, 300, Some("opus")),
                    spare("warm-2", 7002, 250, None),
                ],
            },
        },
    );
    let mut terminal = Terminal::new(TestBackend::new(100, 40)).unwrap();
    terminal.draw(|f| ui::draw(f, &mut app)).unwrap();
    let text = buffer_text(&terminal);
    assert!(
        !text.contains("(unknown agent)"),
        "spares are named, not unknown:\n{text}"
    );
    assert!(
        text.contains("1 session · 4 procs"),
        "claude rollup counts live sessions only:\n{text}"
    );
    assert!(
        text.contains("2 spares · 6 procs · pre-booted for new agents"),
        "spares get their own rollup line:\n{text}"
    );
    let pos = |s: &str| {
        text.find(s)
            .unwrap_or_else(|| panic!("{s} missing:\n{text}"))
    };
    assert!(
        pos("agent-1 (claude)") < pos("warm spares (2)")
            && pos("warm spares (2)") < pos("├ claude · opus")
            && pos("├ claude · opus") < pos("└ claude")
            && pos("└ claude") < pos("nebula daemon"),
        "live rows, then the spares tree, then nebula's own:\n{text}"
    );
    let leaf = text
        .lines()
        .find(|l| l.contains("├ claude · opus"))
        .expect("spare row");
    assert!(
        leaf.contains("demo/main") && leaf.contains("7001") && leaf.contains("300 MB"),
        "a spare is placed in its worktree with its own reading: {leaf}"
    );
    let header = text
        .lines()
        .find(|l| l.contains("warm spares (2)"))
        .expect("header row");
    assert!(
        header.contains("550 MB") && !header.contains("7001"),
        "the header sums its spares and points at no pid: {header}"
    );

    // Enter on a spare opens nothing: there's no agent row to attach
    // until a CreateAgent adopts it.
    let Some(Overlay::Metrics(view)) = &mut app.overlay else {
        panic!("metrics closed");
    };
    view.selected = 2;
    out.clear();
    press(&mut app, KeyCode::Enter, KeyModifiers::NONE, &mut out);
    assert!(
        matches!(app.overlay, Some(Overlay::Metrics(_))) && out.is_empty(),
        "a spare row is inert: {out:?}"
    );
}

#[test]
fn metrics_enter_opens_selected_session() {
    let mut app = App::new();
    seed_tree(&mut app);
    let mut out = Vec::new();
    press(&mut app, KeyCode::Char('M'), KeyModifiers::SHIFT, &mut out);
    request_metrics(&mut app, &mut out);
    let req_id = match out.last() {
        Some(ClientRequest::GetMetrics { req_id }) => *req_id,
        other => panic!("expected GetMetrics, got {other:?}"),
    };
    let snapshot = nebula_core::MetricsSnapshot {
        daemon_pid: 42,
        daemon_rss_bytes: 1024,
        system_total_bytes: 0,
        sessions: vec![nebula_core::SessionMetrics {
            session: SessionRef::Agent(AgentId("a1".into())),
            pid: 4321,
            rss_bytes: 2048,
            procs: 1,
            prewarm: None,
        }],
    };
    hse(
        &mut app,
        ServerEvent::Metrics {
            req_id,
            snapshot: snapshot.clone(),
        },
    );

    // A draw writes the row order back into the view; Enter reads it.
    let mut terminal = Terminal::new(TestBackend::new(100, 40)).unwrap();
    terminal.draw(|f| ui::draw(f, &mut app)).unwrap();
    let Some(Overlay::Metrics(view)) = &app.overlay else {
        panic!("metrics closed");
    };
    assert_eq!(view.rows.len(), 3, "session + daemon + ui rows");
    assert_eq!(view.selected, 0, "cursor starts on the biggest session");

    out.clear();
    press(&mut app, KeyCode::Enter, KeyModifiers::NONE, &mut out);
    assert!(app.overlay.is_none(), "Enter closes the modal");
    assert_eq!(app.focus, Focus::Terminal);
    assert!(app.term_locked, "opened session locks input like an attach");
    let sref = SessionRef::Agent(AgentId("a1".into()));
    assert!(
        out.iter()
            .any(|r| matches!(r, ClientRequest::Attach { session, .. } if *session == sref)),
        "Enter attaches the selected session: {out:?}"
    );
    assert_eq!(
        app.visible_session_rows()
            .get(app.sel_session)
            .and_then(|r| r.sref()),
        Some(sref),
        "the panel selection landed on the opened session"
    );

    // Reopen (Ctrl+q first — the attach locked input to the terminal);
    // Enter on one of nebula's own rows (no session) is inert.
    press(
        &mut app,
        KeyCode::Char('q'),
        KeyModifiers::CONTROL,
        &mut out,
    );
    press(&mut app, KeyCode::Char('M'), KeyModifiers::SHIFT, &mut out);
    request_metrics(&mut app, &mut out);
    let req_id = match out.last() {
        Some(ClientRequest::GetMetrics { req_id }) => *req_id,
        other => panic!("expected GetMetrics, got {other:?}"),
    };
    hse(&mut app, ServerEvent::Metrics { req_id, snapshot });
    terminal.draw(|f| ui::draw(f, &mut app)).unwrap();
    press(&mut app, KeyCode::Char('j'), KeyModifiers::NONE, &mut out);
    press(&mut app, KeyCode::Char('j'), KeyModifiers::NONE, &mut out);
    let Some(Overlay::Metrics(view)) = &app.overlay else {
        panic!("metrics closed");
    };
    assert_eq!(view.selected, 2, "j walks down to the ui row");
    press(&mut app, KeyCode::Char('j'), KeyModifiers::NONE, &mut out);
    let Some(Overlay::Metrics(view)) = &app.overlay else {
        panic!("metrics closed");
    };
    assert_eq!(view.selected, 2, "selection does not run past the last row");
    press(&mut app, KeyCode::Enter, KeyModifiers::NONE, &mut out);
    assert!(
        matches!(app.overlay, Some(Overlay::Metrics(_))),
        "Enter on a nebula row keeps the modal open"
    );
}

#[test]
fn metrics_reply_after_close_is_dropped() {
    let mut app = App::new();
    let mut out = Vec::new();
    press(&mut app, KeyCode::Char('M'), KeyModifiers::SHIFT, &mut out);
    let req_id = match out.last() {
        Some(ClientRequest::GetMetrics { req_id }) => *req_id,
        other => panic!("expected GetMetrics, got {other:?}"),
    };
    press(&mut app, KeyCode::Char('q'), KeyModifiers::NONE, &mut out);
    assert!(app.overlay.is_none(), "q closes the modal");
    hse(
        &mut app,
        ServerEvent::Metrics {
            req_id,
            snapshot: nebula_core::MetricsSnapshot {
                daemon_pid: 42,
                daemon_rss_bytes: 0,
                system_total_bytes: 0,
                sessions: vec![],
            },
        },
    );
    assert!(
        app.overlay.is_none(),
        "late reply must not reopen the modal"
    );
    assert!(app.pending.is_empty(), "late reply still clears its slot");
}

// ---- `f` fuzzy file finder ----

fn finder(app: &App) -> &crate::app::FileFinder {
    match &app.overlay {
        Some(Overlay::Files(f)) => f,
        other => panic!("expected file finder overlay, got {other:?}"),
    }
}

#[test]
fn f_opens_file_finder_listing_tracked_and_untracked() {
    // Pinned to the legacy behavior: with `close_finder_on_open`
    // (the default) the finder would be gone under the editor, and
    // the Ctrl+y at the end would have nothing to copy from.
    with_config_json(r#"{"close_finder_on_open": false}"#, || {
        let dir = tempfile::tempdir().unwrap();
        let repo = test_repo(&dir);
        std::fs::write(repo.join("fresh.txt"), "hello\n").unwrap();
        let mut app = App::new();
        seed_repo_tree(&mut app, &repo);
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        app.vim_tx = Some(tx);
        let mut out = Vec::new();

        press(&mut app, KeyCode::Char('f'), KeyModifiers::NONE, &mut out);
        let files = &finder(&app).files;
        assert!(files.contains(&"a.txt".to_string()), "{files:?}");
        assert!(files.contains(&"fresh.txt".to_string()), "{files:?}");
        // The empty query shows everything.
        assert_eq!(finder(&app).matches.len(), files.len());
        assert!(out.is_empty(), "opening the finder sends nothing");

        // Typing narrows to the fuzzy matches.
        for c in ['f', 'r'] {
            press(&mut app, KeyCode::Char(c), KeyModifiers::NONE, &mut out);
        }
        assert_eq!(finder(&app).matches.len(), 1, "fr matches only fresh.txt");
        assert_eq!(finder(&app).selected_path(), Some("fresh.txt"));

        // Enter opens the selection in the editor modal; the finder stays
        // open underneath. A shell stands in for vim (`sh +1 fresh.txt`
        // still spawns fine).
        if let Some(Overlay::Files(f)) = &mut app.overlay {
            f.editor = "/bin/sh".into();
        }
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE, &mut out);
        let vim = app.vim.as_ref().expect("enter spawns the editor modal");
        assert_eq!(vim.title, "fresh.txt:1");
        assert!(
            matches!(&app.overlay, Some(Overlay::Files(_))),
            "the finder stays open under the editor"
        );

        // Ctrl+Q closes the editor, landing back on the finder; Ctrl+y
        // copies the selected path and closes.
        press(
            &mut app,
            KeyCode::Char('q'),
            KeyModifiers::CONTROL,
            &mut out,
        );
        assert!(app.vim.is_none(), "Ctrl+Q force-closes the editor");
        press(
            &mut app,
            KeyCode::Char('y'),
            KeyModifiers::CONTROL,
            &mut out,
        );
        assert!(app.overlay.is_none(), "ctrl+y closes the finder");
        assert_eq!(app.flash.as_deref(), Some("copied fresh.txt"));
    });
}

/// Enter on a markdown file reads it first: the FILE TABS take the
/// finder's place with the rendered page, and Enter there is the
/// editor. The same rule serves a `.md` ⌥clicked in the pane.
#[test]
fn a_markdown_file_from_the_finder_or_a_clicked_path_is_read_first() {
    let dir = tempfile::tempdir().unwrap();
    let repo = test_repo(&dir);
    std::fs::write(repo.join("notes.md"), "# Notes\n\n- one\n").unwrap();
    let mut app = App::new();
    seed_repo_tree(&mut app, &repo);
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    app.vim_tx = Some(tx);
    let mut out = Vec::new();

    press(&mut app, KeyCode::Char('f'), KeyModifiers::NONE, &mut out);
    for c in ['n', 'o', 't'] {
        press(&mut app, KeyCode::Char(c), KeyModifiers::NONE, &mut out);
    }
    assert_eq!(finder(&app).selected_path(), Some("notes.md"));
    press(&mut app, KeyCode::Enter, KeyModifiers::NONE, &mut out);
    assert!(app.vim.is_none(), "no editor yet");
    let Some(Overlay::FileTabs(tabs)) = &app.overlay else {
        panic!("expected the file tabs, got {:?}", app.overlay);
    };
    assert_eq!(tabs.tabs.len(), 1);
    assert_eq!(tabs.tabs[0].label, "notes.md");
    assert!(tabs.renders_markdown(), "the rendered page, not the source");
    assert_eq!(tabs.preview_text, "# Notes\n\n- one");

    // Enter in the tabs is the editor, embedded under the strip. A
    // shell stands in for vim.
    if let Some(Overlay::FileTabs(t)) = &mut app.overlay {
        t.editor = "/bin/sh".into();
    }
    press(&mut app, KeyCode::Enter, KeyModifiers::NONE, &mut out);
    let vim = app.vim.as_ref().expect("enter edits");
    assert!(vim.embedded, "the editor takes the preview pane");
    assert!(
        matches!(&app.overlay, Some(Overlay::FileTabs(_))),
        "the tabs stay underneath"
    );

    // A clicked path takes the same route; a code file still goes
    // straight to the editor.
    app.vim = None;
    app.overlay = None;
    open_file_link(&mut app, "notes.md", Some(3));
    let Some(Overlay::FileTabs(tabs)) = &app.overlay else {
        panic!("expected the file tabs, got {:?}", app.overlay);
    };
    assert_eq!(tabs.tabs[0].label, "notes.md");
    assert!(tabs.renders_markdown());
    assert!(app.vim.is_none());
    app.overlay = None;
    with_config_json(r#"{"editor": "/bin/sh"}"#, || {
        open_file_link(&mut app, "a.txt", Some(1));
    });
    assert!(app.overlay.is_none(), "no tabs for a text file");
    assert_eq!(
        app.vim.as_ref().map(|v| v.title.as_str()),
        Some("a.txt:1"),
        "the editor, at the line"
    );
}

#[test]
fn close_finder_on_open_leaves_only_the_editor_modal() {
    let dir = tempfile::tempdir().unwrap();
    let repo = test_repo(&dir);
    // Default-on: the finder is gone by the time the editor is up, so
    // quitting the editor lands on the panels with nothing left to Esc.
    with_default_config(|| {
        let mut app = App::new();
        seed_repo_tree(&mut app, &repo);
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        app.vim_tx = Some(tx);
        let mut out = Vec::new();

        press(&mut app, KeyCode::Char('f'), KeyModifiers::NONE, &mut out);
        // A shell stands in for vim (`sh +1 a.txt` still spawns fine).
        if let Some(Overlay::Files(f)) = &mut app.overlay {
            f.editor = "/bin/sh".into();
        }
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE, &mut out);
        assert!(app.vim.is_some(), "enter spawns the editor modal");
        assert!(
            app.overlay.is_none(),
            "the finder closes as the editor opens"
        );

        press(
            &mut app,
            KeyCode::Char('q'),
            KeyModifiers::CONTROL,
            &mut out,
        );
        assert!(app.vim.is_none(), "Ctrl+Q force-closes the editor");
        assert!(app.overlay.is_none(), "no finder left to dismiss");
    });
}

#[test]
fn a_failed_editor_spawn_keeps_the_finder_open() {
    let dir = tempfile::tempdir().unwrap();
    let repo = test_repo(&dir);
    with_default_config(|| {
        let mut app = App::new();
        seed_repo_tree(&mut app, &repo);
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        app.vim_tx = Some(tx);
        let mut out = Vec::new();

        press(&mut app, KeyCode::Char('f'), KeyModifiers::NONE, &mut out);
        if let Some(Overlay::Files(f)) = &mut app.overlay {
            f.editor = "/nonexistent/nebula-not-an-editor".into();
        }
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE, &mut out);
        assert!(app.vim.is_none(), "the spawn failed");
        assert!(app.flash.is_some(), "the failure flashes");
        assert!(
            matches!(&app.overlay, Some(Overlay::Files(_))),
            "a finder dismissed for an editor that never opened would \
                 leave the user staring at the panels"
        );
    });
}

#[test]
fn file_finder_escape_clears_query_then_closes() {
    let mut app = App::new();
    app.overlay = Some(Overlay::Files(FileFinder::new(
        "/nonexistent-nebula-finder-test".into(),
        "main".into(),
        "vim".into(),
        vec!["src/alpha.rs".into(), "src/beta.rs".into()],
    )));
    let mut out = Vec::new();
    press(&mut app, KeyCode::Char('b'), KeyModifiers::NONE, &mut out);
    assert_eq!(finder(&app).matches.len(), 1, "b matches only beta.rs");
    press(&mut app, KeyCode::Esc, KeyModifiers::NONE, &mut out);
    assert_eq!(finder(&app).query, "", "first Esc clears the query");
    assert_eq!(finder(&app).matches.len(), 2, "cleared query shows all");
    press(&mut app, KeyCode::Esc, KeyModifiers::NONE, &mut out);
    assert!(app.overlay.is_none(), "second Esc closes the finder");
}

#[test]
fn file_overlays_launch_the_configured_editor() {
    let dir = tempfile::tempdir().unwrap();
    let repo = test_repo(&dir);
    let cfg_dir = tempfile::tempdir().unwrap();
    crate::config::with_config_path(cfg_dir.path().join("config.json"), || {
        let mut cfg = crate::config::Config::load();
        cfg.editor = "nvim".into();
        cfg.save().unwrap();
        // What the overlays should capture: the setting, unless the
        // test environment carries a NEBULA_EDITOR override.
        let expect = crate::config::Config::load().editor_command();

        let mut app = App::new();
        seed_repo_tree(&mut app, &repo);
        let mut out = Vec::new();
        press(&mut app, KeyCode::Char('f'), KeyModifiers::NONE, &mut out);
        assert_eq!(finder(&app).editor, expect);
        app.overlay = None;
        press(&mut app, KeyCode::Char('b'), KeyModifiers::NONE, &mut out);
        assert_eq!(tree_view(&app).editor, expect);
        app.overlay = None;
        press(&mut app, KeyCode::Char('F'), KeyModifiers::SHIFT, &mut out);
        let Some(Overlay::Grep(view)) = &app.overlay else {
            panic!("F opens the grep overlay");
        };
        assert_eq!(view.editor, expect);
    });
}

#[test]
fn f_without_worktree_flashes() {
    let mut app = App::new();
    let mut out = Vec::new();
    press(&mut app, KeyCode::Char('f'), KeyModifiers::NONE, &mut out);
    assert!(app.overlay.is_none());
    assert_eq!(app.flash.as_deref(), Some("no worktree selected"));
}

#[test]
fn file_finder_renders_query_row_and_matches() {
    let mut app = App::new();
    app.overlay = Some(Overlay::Files(FileFinder::new(
        "/nonexistent-nebula-finder-test".into(),
        "main".into(),
        "vim".into(),
        vec!["src/alpha.rs".into(), "src/beta.rs".into()],
    )));
    let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
    terminal.draw(|f| ui::draw(f, &mut app)).unwrap();
    let text = buffer_text(&terminal);
    assert!(text.contains("Find file — main (2)"), "title:\n{text}");
    assert!(text.contains("type to filter…"), "query hint:\n{text}");
    assert!(text.contains("src/alpha.rs"), "rows rendered:\n{text}");
    let fin = finder(&app);
    assert!(fin.area.width > 0, "draw writes hit-test area");
    assert!(fin.list_area.height > 0, "draw writes list area");
}

// ---- `b` tree browser ----

fn tree_view(app: &App) -> &crate::tree_browser::TreeBrowser {
    match &app.overlay {
        Some(Overlay::Tree(v)) => v,
        other => panic!("expected tree overlay, got {other:?}"),
    }
}

fn tree_rows(app: &App) -> Vec<String> {
    let v = tree_view(app);
    v.rows
        .iter()
        .map(|r| v.nodes[r.node].path.clone())
        .collect()
}

/// A markdown file previews as the rendered page — a bullet, no `#`,
/// the scroller counting rendered rows — and Ctrl+r flips it to the
/// numbered source and back. On any other file Ctrl+r is nothing: the
/// filter ignores it, and the choice made on the markdown file holds.
#[test]
fn ctrl_r_flips_a_markdown_preview_between_the_page_and_its_source() {
    let dir = tempfile::tempdir().unwrap();
    let repo = test_repo(&dir);
    std::fs::write(repo.join("notes.md"), "# Notes\n\n- one\n").unwrap();
    let mut app = App::new();
    seed_repo_tree(&mut app, &repo);
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    app.vim_tx = Some(tx);
    let mut out = Vec::new();
    let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();

    press(&mut app, KeyCode::Char('b'), KeyModifiers::NONE, &mut out);
    for c in ['n', 'o', 't'] {
        press(&mut app, KeyCode::Char(c), KeyModifiers::NONE, &mut out);
    }
    assert_eq!(tree_view(&app).selected_node().unwrap().path, "notes.md");
    assert!(
        tree_view(&app).renders_markdown(),
        "markdown opens rendered"
    );
    terminal.draw(|f| ui::draw(f, &mut app)).unwrap();
    let text = buffer_text(&terminal);
    assert!(text.contains("• one"), "the page:\n{text}");
    assert!(!text.contains("# Notes"), "no heading marker:\n{text}");
    let view = tree_view(&app);
    assert_eq!(
        view.preview_line_count, 4,
        "heading, rule, blank, bullet — the rendered rows"
    );
    assert!(view.rendered.is_some(), "the flowed page is kept");

    press(
        &mut app,
        KeyCode::Char('r'),
        KeyModifiers::CONTROL,
        &mut out,
    );
    assert!(!tree_view(&app).renders_markdown());
    terminal.draw(|f| ui::draw(f, &mut app)).unwrap();
    let text = buffer_text(&terminal);
    assert!(text.contains(" 1 # Notes"), "the source, numbered:\n{text}");
    assert_eq!(tree_view(&app).preview_line_count, 3, "the source lines");

    // Off markdown, Ctrl+r reaches the filter, which ignores it.
    press(&mut app, KeyCode::Esc, KeyModifiers::NONE, &mut out);
    for c in ['a', '.', 't'] {
        press(&mut app, KeyCode::Char(c), KeyModifiers::NONE, &mut out);
    }
    assert_eq!(tree_view(&app).selected_node().unwrap().path, "a.txt");
    assert!(!tree_view(&app).markdown);
    press(
        &mut app,
        KeyCode::Char('r'),
        KeyModifiers::CONTROL,
        &mut out,
    );
    assert_eq!(tree_view(&app).filter.as_str(), "a.t", "not typed");
    press(&mut app, KeyCode::Esc, KeyModifiers::NONE, &mut out);
    for c in ['n', 'o', 't'] {
        press(&mut app, KeyCode::Char(c), KeyModifiers::NONE, &mut out);
    }
    assert!(
        !tree_view(&app).renders_markdown(),
        "the source stays chosen across files"
    );
}

#[test]
fn t_opens_tree_browser_folds_dirs_and_filters_hierarchies() {
    let dir = tempfile::tempdir().unwrap();
    let repo = test_repo(&dir);
    std::fs::create_dir_all(repo.join("src/sub")).unwrap();
    std::fs::write(repo.join("src/lib.rs"), "hello tree\n").unwrap();
    std::fs::write(repo.join("src/sub/deep.rs"), "deep\n").unwrap();
    let mut app = App::new();
    seed_repo_tree(&mut app, &repo);
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    app.vim_tx = Some(tx);
    let mut out = Vec::new();

    press(&mut app, KeyCode::Char('b'), KeyModifiers::NONE, &mut out);
    assert_eq!(tree_view(&app).file_count, 3);
    // Collapsed by default: dirs first, then top-level files; the
    // selected dir previews its children.
    assert_eq!(tree_rows(&app), vec!["src", "a.txt"]);
    assert_eq!(tree_view(&app).preview, "sub/\nlib.rs");
    assert!(out.is_empty(), "opening the browser sends nothing");

    // Enter on a directory unfolds it, and folds it again.
    press(&mut app, KeyCode::Enter, KeyModifiers::NONE, &mut out);
    assert_eq!(
        tree_rows(&app),
        vec!["src", "src/sub", "src/lib.rs", "a.txt"]
    );
    press(&mut app, KeyCode::Enter, KeyModifiers::NONE, &mut out);
    assert_eq!(tree_rows(&app), vec!["src", "a.txt"]);

    // Typing narrows the tree to matching files plus the hierarchies
    // containing them, forced open, with the selection on the match.
    for c in ['d', 'e', 'e', 'p'] {
        press(&mut app, KeyCode::Char(c), KeyModifiers::NONE, &mut out);
    }
    assert_eq!(tree_rows(&app), vec!["src", "src/sub", "src/sub/deep.rs"]);
    assert_eq!(tree_view(&app).match_count, 1);
    assert_eq!(
        tree_view(&app).selected_node().unwrap().path,
        "src/sub/deep.rs"
    );
    assert_eq!(tree_view(&app).preview, "deep");

    // Enter opens the selected file in an editor embedded in the
    // preview pane; the browser stays open. A shell stands in for vim.
    if let Some(Overlay::Tree(v)) = &mut app.overlay {
        v.editor = "/bin/sh".into();
    }
    press(&mut app, KeyCode::Enter, KeyModifiers::NONE, &mut out);
    let vim = app.vim.as_ref().expect("enter spawns the editor");
    assert_eq!(vim.title, "src/sub/deep.rs:1");
    assert!(vim.embedded, "tree editor renders in the preview pane");
    assert!(
        matches!(&app.overlay, Some(Overlay::Tree(_))),
        "the browser stays open around the editor"
    );

    // Closing the editor reloads the preview — the file may have been
    // edited under it.
    std::fs::write(repo.join("src/sub/deep.rs"), "deeper\n").unwrap();
    press(
        &mut app,
        KeyCode::Char('q'),
        KeyModifiers::CONTROL,
        &mut out,
    );
    assert!(app.vim.is_none(), "Ctrl+Q force-closes the editor");
    assert_eq!(tree_view(&app).preview, "deeper");

    // Two-stage escape: clear the filter (restoring the folded tree),
    // then close.
    press(&mut app, KeyCode::Esc, KeyModifiers::NONE, &mut out);
    assert_eq!(tree_view(&app).filter, "", "first Esc clears the filter");
    assert_eq!(tree_rows(&app), vec!["src", "a.txt"]);
    press(&mut app, KeyCode::Esc, KeyModifiers::NONE, &mut out);
    assert!(app.overlay.is_none(), "second Esc closes the browser");
}

#[test]
fn tree_browser_ctrl_u_clears_filter() {
    let dir = tempfile::tempdir().unwrap();
    let repo = test_repo(&dir);
    std::fs::create_dir_all(repo.join("src")).unwrap();
    std::fs::write(repo.join("src/lib.rs"), "hello tree\n").unwrap();
    let mut app = App::new();
    seed_repo_tree(&mut app, &repo);
    let mut out = Vec::new();

    press(&mut app, KeyCode::Char('b'), KeyModifiers::NONE, &mut out);
    for c in ['l', 'i', 'b'] {
        press(&mut app, KeyCode::Char(c), KeyModifiers::NONE, &mut out);
    }
    assert_eq!(tree_rows(&app), vec!["src", "src/lib.rs"]);

    press(
        &mut app,
        KeyCode::Char('u'),
        KeyModifiers::CONTROL,
        &mut out,
    );
    assert_eq!(tree_view(&app).filter, "", "Ctrl+u clears the filter");
    assert_eq!(
        tree_rows(&app),
        vec!["src", "a.txt"],
        "folded tree restored"
    );

    // With nothing typed, Ctrl+u falls back to scrolling: the browser
    // stays open and the filter stays empty.
    press(
        &mut app,
        KeyCode::Char('u'),
        KeyModifiers::CONTROL,
        &mut out,
    );
    assert_eq!(tree_view(&app).filter, "");
    assert!(
        matches!(app.overlay, Some(Overlay::Tree(_))),
        "the browser stays open"
    );
}

#[test]
fn b_without_worktree_flashes() {
    let mut app = App::new();
    let mut out = Vec::new();
    press(&mut app, KeyCode::Char('b'), KeyModifiers::NONE, &mut out);
    assert!(app.overlay.is_none());
    assert_eq!(app.flash.as_deref(), Some("no worktree selected"));
}

#[test]
fn tree_browser_renders_tree_and_preview_panes() {
    let dir = tempfile::tempdir().unwrap();
    let repo = test_repo(&dir); // commits a.txt containing "orig"
    let mut app = App::new();
    seed_repo_tree(&mut app, &repo);
    let mut out = Vec::new();
    press(&mut app, KeyCode::Char('b'), KeyModifiers::NONE, &mut out);

    let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
    terminal.draw(|f| ui::draw(f, &mut app)).unwrap();
    let text = buffer_text(&terminal);
    assert!(text.contains("Tree — main (1)"), "tree title:\n{text}");
    assert!(text.contains("type to filter…"), "filter hint:\n{text}");
    assert!(text.contains("a.txt"), "tree rows rendered:\n{text}");
    assert!(text.contains("orig"), "preview rendered:\n{text}");
    let v = tree_view(&app);
    assert!(v.area.width > 0, "draw writes hit-test area");
    assert!(v.list_area.height > 0, "draw writes list area");
    assert!(v.view_height > 0, "draw writes preview page size");
}

#[test]
fn file_preview_gets_a_line_number_gutter_but_listings_dont() {
    let dir = tempfile::tempdir().unwrap();
    let repo = test_repo(&dir);
    std::fs::create_dir_all(repo.join("src")).unwrap();
    std::fs::write(repo.join("src/lib.rs"), "one\ntwo\nthree\n").unwrap();
    let mut app = App::new();
    seed_repo_tree(&mut app, &repo);
    let mut out = Vec::new();
    press(&mut app, KeyCode::Char('b'), KeyModifiers::NONE, &mut out);
    for c in ['l', 'i', 'b'] {
        press(&mut app, KeyCode::Char(c), KeyModifiers::NONE, &mut out);
    }
    assert_eq!(tree_rows(&app), vec!["src", "src/lib.rs"]);

    let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
    terminal.draw(|f| ui::draw(f, &mut app)).unwrap();
    let text = buffer_text(&terminal);
    for (n, line) in [(1, "one"), (2, "two"), (3, "three")] {
        assert!(
            text.contains(&format!(" {n} {line}")),
            "file preview numbers its lines:\n{text}"
        );
    }

    // A directory's child listing isn't file content — no gutter.
    press(&mut app, KeyCode::Up, KeyModifiers::NONE, &mut out);
    assert_eq!(tree_view(&app).selected_node().unwrap().path, "src");
    terminal.draw(|f| ui::draw(f, &mut app)).unwrap();
    let text = buffer_text(&terminal);
    assert!(text.contains("lib.rs"), "listing rendered:\n{text}");
    assert!(
        !text.contains(" 1 lib.rs"),
        "directory listings stay unnumbered:\n{text}"
    );
}

#[test]
fn embedded_editor_takes_over_the_preview_pane() {
    let dir = tempfile::tempdir().unwrap();
    let repo = test_repo(&dir);
    let mut app = App::new();
    seed_repo_tree(&mut app, &repo);
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    app.vim_tx = Some(tx);
    let mut out = Vec::new();
    press(&mut app, KeyCode::Char('b'), KeyModifiers::NONE, &mut out);

    // A draw teaches the browser its preview rect, so the editor can
    // spawn at the pane's size. Row 0 is a.txt (the only file).
    let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
    terminal.draw(|f| ui::draw(f, &mut app)).unwrap();
    if let Some(Overlay::Tree(v)) = &mut app.overlay {
        v.editor = "/bin/sh".into();
    }
    press(&mut app, KeyCode::Enter, KeyModifiers::NONE, &mut out);
    let pane = tree_view(&app).preview_area;
    let vim = app.vim.as_ref().expect("enter spawns the editor");
    assert!(vim.embedded);
    assert_eq!(
        (vim.cols, vim.rows),
        (pane.width, pane.height),
        "editor spawns at the pane size"
    );

    terminal.draw(|f| ui::draw(f, &mut app)).unwrap();
    let text = buffer_text(&terminal);
    assert!(
        text.contains("— editing"),
        "title shows edit state:\n{text}"
    );
    assert_eq!(
        app.vim.as_ref().unwrap().area,
        tree_view(&app).preview_area,
        "editor renders into the preview pane, not the modal"
    );
}

// ---- `F` find in files + editor modal ----

fn grep_view(app: &App) -> &crate::app::GrepView {
    match &app.overlay {
        Some(Overlay::Grep(v)) => v,
        other => panic!("expected grep overlay, got {other:?}"),
    }
}

fn fake_grep_view(hits: Vec<crate::grep_search::GrepHit>) -> GrepView {
    let mut view = GrepView::new(
        "/nonexistent-nebula-grep-test".into(),
        "main".into(),
        "vim".into(),
    );
    view.query = "zz".into();
    view.hits = hits;
    view
}

#[test]
fn shift_f_opens_grep_and_typing_searches() {
    let dir = tempfile::tempdir().unwrap();
    let repo = test_repo(&dir);
    std::fs::write(repo.join("hay.txt"), "one\nneedle here\n").unwrap();
    let mut app = App::new();
    seed_repo_tree(&mut app, &repo);
    let mut out = Vec::new();

    press(&mut app, KeyCode::Char('F'), KeyModifiers::SHIFT, &mut out);
    assert!(grep_view(&app).hits.is_empty(), "opens with no results");
    assert!(out.is_empty(), "opening the overlay sends nothing");

    for c in "needle".chars() {
        press(&mut app, KeyCode::Char(c), KeyModifiers::NONE, &mut out);
    }
    let view = grep_view(&app);
    assert_eq!(view.hits.len(), 1, "{:?}", view.hits);
    assert_eq!(view.hits[0].path, "hay.txt");
    assert_eq!(view.hits[0].line, 2);
    assert_eq!(view.hits[0].text, "needle here");

    // Two-stage escape: clear the query, then close.
    press(&mut app, KeyCode::Esc, KeyModifiers::NONE, &mut out);
    assert_eq!(grep_view(&app).query, "", "first Esc clears the query");
    assert!(
        grep_view(&app).hits.is_empty(),
        "cleared query shows no hits"
    );
    press(&mut app, KeyCode::Esc, KeyModifiers::NONE, &mut out);
    assert!(app.overlay.is_none(), "second Esc closes the overlay");
}

#[test]
fn shift_f_without_worktree_flashes() {
    let mut app = App::new();
    let mut out = Vec::new();
    press(&mut app, KeyCode::Char('F'), KeyModifiers::SHIFT, &mut out);
    assert!(app.overlay.is_none());
    assert_eq!(app.flash.as_deref(), Some("no worktree selected"));
}

#[test]
fn grep_enter_spawns_editor_and_ctrl_q_closes_it() {
    // Pinned to the legacy behavior — the "lands back on the
    // results" half of this test is what `close_finder_on_open`
    // off buys you.
    with_config_json(r#"{"close_finder_on_open": false}"#, || {
        let dir = tempfile::tempdir().unwrap();
        let repo = test_repo(&dir);
        let mut app = App::new();
        seed_repo_tree(&mut app, &repo);
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        app.vim_tx = Some(tx);
        let mut out = Vec::new();

        press(&mut app, KeyCode::Char('F'), KeyModifiers::SHIFT, &mut out);
        for c in "orig".chars() {
            press(&mut app, KeyCode::Char(c), KeyModifiers::NONE, &mut out);
        }
        assert_eq!(grep_view(&app).selected_hit().unwrap().path, "a.txt");
        // A shell stands in for vim (`sh +1 a.txt` still spawns fine).
        if let Some(Overlay::Grep(v)) = &mut app.overlay {
            v.editor = "/bin/sh".into();
        }

        press(&mut app, KeyCode::Enter, KeyModifiers::NONE, &mut out);
        let vim = app.vim.as_ref().expect("enter spawns the editor modal");
        assert_eq!(vim.title, "a.txt:1");
        assert_eq!(vim.generation, 1);
        assert!(
            matches!(&app.overlay, Some(Overlay::Grep(_))),
            "the grep overlay stays open under the editor"
        );

        // With the modal open, keys forward to the editor — q must not quit.
        press(&mut app, KeyCode::Char('q'), KeyModifiers::NONE, &mut out);
        assert!(!app.should_quit, "q goes to the editor, not the app");
        assert!(app.vim.is_some());

        // Ctrl+Q is the hatch.
        press(
            &mut app,
            KeyCode::Char('q'),
            KeyModifiers::CONTROL,
            &mut out,
        );
        assert!(app.vim.is_none(), "Ctrl+Q force-closes the editor");
        assert!(
            matches!(&app.overlay, Some(Overlay::Grep(_))),
            "closing the editor lands back on the results"
        );
    });
}

#[test]
fn close_finder_on_open_also_closes_the_grep_overlay() {
    let dir = tempfile::tempdir().unwrap();
    let repo = test_repo(&dir);
    with_default_config(|| {
        let mut app = App::new();
        seed_repo_tree(&mut app, &repo);
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        app.vim_tx = Some(tx);
        let mut out = Vec::new();

        press(&mut app, KeyCode::Char('F'), KeyModifiers::SHIFT, &mut out);
        for c in "orig".chars() {
            press(&mut app, KeyCode::Char(c), KeyModifiers::NONE, &mut out);
        }
        // A shell stands in for vim (`sh +1 a.txt` still spawns fine).
        if let Some(Overlay::Grep(v)) = &mut app.overlay {
            v.editor = "/bin/sh".into();
        }
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE, &mut out);
        assert!(app.vim.is_some(), "enter spawns the editor modal");
        assert!(
            app.overlay.is_none(),
            "find-in-files closes as the editor opens"
        );
    });
}

#[test]
fn stale_generation_editor_events_are_dropped() {
    let mut app = App::new();
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let dir = tempfile::tempdir().unwrap();
    let mut vim = crate::vim_term::VimTerm::spawn_cmd(
        "/bin/sh",
        &["-c".into(), "sleep 30".into()],
        dir.path(),
        "a.txt:1".into(),
        80,
        24,
        2,
        tx,
    )
    .unwrap();
    vim.kill();
    app.vim = Some(vim);

    // Output and exit stamped with a previous spawn's generation: ignored.
    handle_vim_event(
        &mut app,
        VimEvent::Output {
            generation: 1,
            data: b"stale".to_vec(),
        },
    );
    handle_vim_event(&mut app, VimEvent::Exited { generation: 1 });
    assert!(app.vim.is_some(), "stale exit must not close a new editor");
    assert!(
        !app.vim
            .as_ref()
            .unwrap()
            .parser
            .screen()
            .contents()
            .contains("stale"),
        "stale output must not reach the new editor's screen"
    );

    // The current generation's exit closes the modal.
    handle_vim_event(&mut app, VimEvent::Exited { generation: 2 });
    assert!(app.vim.is_none());
}

#[test]
fn grep_overlay_renders_hits_and_editor_modal_renders_on_top() {
    let mut app = App::new();
    app.overlay = Some(Overlay::Grep(fake_grep_view(vec![
        crate::grep_search::GrepHit {
            path: "src/alpha.rs".into(),
            line: 3,
            text: "let zz = 1;".into(),
        },
        crate::grep_search::GrepHit {
            path: "src/beta.rs".into(),
            line: 14,
            text: "zz += 1;".into(),
        },
    ])));
    let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
    terminal.draw(|f| ui::draw(f, &mut app)).unwrap();
    let text = buffer_text(&terminal);
    assert!(
        text.contains("Find in files — main (2 hits)"),
        "title:\n{text}"
    );
    assert!(text.contains("src/alpha.rs:3"), "hit location:\n{text}");
    assert!(text.contains("let zz = 1;"), "hit text:\n{text}");
    let view = grep_view(&app);
    assert!(view.area.width > 0, "draw writes hit-test area");
    assert!(view.list_area.height > 0, "draw writes list area");

    // Spawn an editor modal: it draws on top and gets its rect written
    // back for the PTY resize sync.
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let dir = tempfile::tempdir().unwrap();
    let mut vim = crate::vim_term::VimTerm::spawn_cmd(
        "/bin/sh",
        &["-c".into(), "sleep 30".into()],
        dir.path(),
        "src/alpha.rs:3".into(),
        80,
        24,
        1,
        tx,
    )
    .unwrap();
    vim.kill();
    app.vim = Some(vim);
    terminal.draw(|f| ui::draw(f, &mut app)).unwrap();
    let text = buffer_text(&terminal);
    assert!(text.contains("src/alpha.rs:3"), "modal title:\n{text}");
    assert!(text.contains("Ctrl+Q: force close"), "hatch hint:\n{text}");
    let vim = app.vim.as_ref().unwrap();
    assert!(vim.area.width > 0, "draw writes the editor rect");
    sync_vim_size(&mut app);
    let vim = app.vim.as_ref().unwrap();
    assert_eq!(
        (vim.cols, vim.rows),
        (vim.area.width, vim.area.height),
        "sync resizes the PTY to the drawn rect"
    );
}

// ---- Shift+D bulk delete ----

/// Shift+D in the worktrees panel confirms deleting EVERY non-main
/// worktree of the project — itemized in the dialog — and confirming
/// fires one delete per worktree, dropping the rows optimistically.
#[test]
fn shift_d_bulk_deletes_worktrees_behind_an_itemized_confirm() {
    use nebula_core::{Entity, Worktree, WorktreeId};
    let mut app = App::new();
    seed_tree(&mut app); // p1/w1(main) + agent-1
    let mut out = Vec::new();
    for (id, branch) in [("w2", "feat"), ("w3", "fix")] {
        hse(
            &mut app,
            ServerEvent::EntityUpserted {
                entity: Entity::Worktree(Worktree {
                    id: WorktreeId(id.into()),
                    project_id: nebula_core::ProjectId("p1".into()),
                    path: format!("/tmp/demo-worktrees/{branch}").into(),
                    branch: branch.into(),
                    is_main: false,
                    sort_order: 0,
                }),
            },
        );
    }
    app.focus = Focus::Worktrees;

    press(&mut app, KeyCode::Char('D'), KeyModifiers::SHIFT, &mut out);
    let Some(Overlay::Confirm(c)) = &app.overlay else {
        panic!("Shift+D confirms first: {:?}", app.overlay);
    };
    assert!(
        c.message.contains("• feat") && c.message.contains("• fix"),
        "casualties are itemized: {}",
        c.message
    );
    assert!(
        !c.message.contains("• main"),
        "main checkout is not on the kill list: {}",
        c.message
    );
    assert!(
        matches!(&c.action, PendingAction::DeleteAllWorktrees(ids) if ids.len() == 2),
        "main checkout excluded from the action: {:?}",
        c.action
    );

    // The dialog really shows the list (multi-line confirm rendering).
    let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
    terminal.draw(|f| ui::draw(f, &mut app)).unwrap();
    let text = buffer_text(&terminal);
    assert!(text.contains("• feat"), "dialog lists feat:\n{text}");
    assert!(text.contains("• fix"), "dialog lists fix:\n{text}");

    press(&mut app, KeyCode::Char('y'), KeyModifiers::NONE, &mut out);
    let deleted: Vec<&str> = out
        .iter()
        .filter_map(|r| match r {
            ClientRequest::DeleteWorktree { id, .. } => Some(id.0.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(deleted, ["w2", "w3"], "one request per worktree: {out:?}");
    assert!(app.overlay.is_none());
    let left: Vec<&str> = app.tree.worktrees.iter().map(|w| w.id.0.as_str()).collect();
    assert_eq!(left, ["w1"], "only the main checkout survives");
}

/// With only the main checkout, Shift+D has nothing to offer — flash,
/// no dialog.
#[test]
fn shift_d_with_only_the_main_checkout_flashes() {
    let mut app = App::new();
    seed_tree(&mut app); // p1/w1(main) + agent-1
    let mut out = Vec::new();
    app.focus = Focus::Worktrees;
    press(&mut app, KeyCode::Char('D'), KeyModifiers::SHIFT, &mut out);
    assert!(app.overlay.is_none(), "nothing to confirm");
    assert!(app.flash.is_some(), "the refusal explains itself");
    assert!(out.is_empty(), "nothing is requested");
}

/// Shift+D in the sessions panel confirms deleting every LISTED session
/// — hidden archived rows are spared — and an attached doomed session
/// detaches before its delete.
#[test]
fn shift_d_bulk_deletes_the_visible_sessions() {
    use nebula_core::{Agent, AgentStatus, Entity};
    let mut app = App::new();
    seed_tree(&mut app); // p1/w1(main) + agent-1
    for (id, name, archived) in [("a2", "agent-2", false), ("a3", "agent-3", true)] {
        hse(
            &mut app,
            ServerEvent::EntityUpserted {
                entity: Entity::Agent(Agent {
                    id: AgentId(id.into()),
                    worktree_id: WorktreeId("w1".into()),
                    name: name.into(),
                    status: AgentStatus::Fresh,
                    archived,
                    archived_at: 0,
                    unseen: false,
                    kind: nebula_core::AgentKind::Claude,
                    custom_harness: None,
                    model: None,
                    effort: None,
                    session_id: None,
                    cloud_session_id: None,
                    sort_order: 1,
                    status_changed_at: 0,
                    alive: true,
                    issue_url: None,
                    recent_prompts: Vec::new(),
                }),
            },
        );
    }
    app.focus = Focus::Sessions;
    let sref = SessionRef::Agent(AgentId("a1".into()));
    app.term = Some(AttachedTerm::new(sref.clone(), 40, 10));
    let mut out = Vec::new();

    press(&mut app, KeyCode::Char('D'), KeyModifiers::SHIFT, &mut out);
    let Some(Overlay::Confirm(c)) = &app.overlay else {
        panic!("Shift+D confirms first: {:?}", app.overlay);
    };
    assert!(
        c.message.contains("• agent-1") && c.message.contains("• agent-2"),
        "listed sessions are itemized: {}",
        c.message
    );
    assert!(
        !c.message.contains("agent-3"),
        "hidden archived rows are spared: {}",
        c.message
    );

    press(&mut app, KeyCode::Enter, KeyModifiers::NONE, &mut out);
    assert!(
        matches!(out.first(), Some(ClientRequest::Detach { session }) if *session == sref),
        "attached doomed session detaches first: {out:?}"
    );
    assert!(app.term.is_none(), "the pane blanks with the detach");
    let deleted: Vec<&str> = out
        .iter()
        .filter_map(|r| match r {
            ClientRequest::DeleteAgent { id, .. } => Some(id.0.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(deleted, ["a1", "a2"], "one request per session: {out:?}");
}

fn wt_entity(id: &str, project: &str, branch: &str, is_main: bool) -> nebula_core::Entity {
    use nebula_core::{Entity, Worktree};
    Entity::Worktree(Worktree {
        id: WorktreeId(id.into()),
        project_id: nebula_core::ProjectId(project.into()),
        path: format!("/tmp/{branch}").into(),
        branch: branch.into(),
        is_main,
        sort_order: 0,
    })
}

fn agent_entity(id: &str, wt: &str, name: &str, archived: bool) -> nebula_core::Entity {
    use nebula_core::{Agent, AgentStatus, Entity};
    Entity::Agent(Agent {
        id: AgentId(id.into()),
        worktree_id: WorktreeId(wt.into()),
        name: name.into(),
        status: AgentStatus::Fresh,
        archived,
        archived_at: 0,
        unseen: false,
        kind: nebula_core::AgentKind::Claude,
        custom_harness: None,
        model: None,
        effort: None,
        session_id: None,
        cloud_session_id: None,
        sort_order: 1,
        status_changed_at: 0,
        alive: true,
        issue_url: None,
        recent_prompts: Vec::new(),
    })
}

/// Archiving the selected session lands the cursor on the next row AND
/// attaches it — the pane must show the newly highlighted session, not
/// stay blank after the archive's detach. All of it on the confirm's
/// Enter (an OPTIMISTIC UPDATE): the DAEMON's upsert, when it lands,
/// finds the row already archived and changes nothing.
#[test]
fn archiving_selected_agent_previews_the_next_row() {
    let mut app = App::new();
    seed_tree(&mut app); // p1 / w1(main) / a1
    hse(
        &mut app,
        ServerEvent::EntityUpserted {
            entity: agent_entity("a2", "w1", "agent-2", false),
        },
    );
    app.focus = Focus::Sessions;
    app.sel_session = 0; // a1
    let a1 = SessionRef::Agent(AgentId("a1".into()));
    app.term = Some(AttachedTerm::new(a1.clone(), 40, 10));

    let mut out = Vec::new();
    // `a` asks first; Enter on the confirm is the archive.
    press(&mut app, KeyCode::Char('a'), KeyModifiers::NONE, &mut out);
    press(&mut app, KeyCode::Enter, KeyModifiers::NONE, &mut out);
    assert!(
        app.overlay.is_none(),
        "Enter answered the confirm: {:?}",
        app.overlay
    );
    assert!(
        out.iter()
            .any(|r| matches!(r, ClientRequest::ArchiveAgent { .. })),
        "a requests the archive: {out:?}"
    );

    // The row has left the list already: the cursor is on agent-2, and
    // agent-2 is what the pane shows.
    let a2 = SessionRef::Agent(AgentId("a2".into()));
    assert_eq!(
        app.selected_session().map(|a| a.name),
        Some("agent-2".into()),
        "cursor landed on the next row"
    );
    assert!(
        out.iter()
            .any(|r| matches!(r, ClientRequest::Attach { session, .. } if *session == a2)),
        "the next row's session attaches: {out:?}"
    );
    assert_eq!(
        app.term.as_ref().map(|t| t.sref.clone()),
        Some(a2.clone()),
        "the pane shows the newly highlighted session"
    );

    // The daemon's upsert says what the screen already does.
    out.clear();
    handle_server_event(
        &mut app,
        ServerEvent::EntityUpserted {
            entity: agent_entity("a1", "w1", "agent-1", true),
        },
        &mut out,
    );
    assert!(out.is_empty(), "nothing left to do: {out:?}");
    assert_eq!(app.term.as_ref().map(|t| t.sref.clone()), Some(a2));
}

/// `a` asks first: nothing is sent until the dialog is answered, Esc
/// keeps the session, and Enter archives it — pane released and all.
/// The retired `confirm_on_archive` key's `false`, which every file
/// an older build saved holds, does not switch the confirm off: only
/// `ask_before_archive` does.
#[test]
fn a_asks_before_archiving_by_default() {
    with_config_json(r#"{"confirm_on_archive": false}"#, || {
        let mut app = App::new();
        seed_tree(&mut app); // p1 / w1(main) / a1
        app.focus = Focus::Sessions;
        app.sel_session = 0;
        let a1 = SessionRef::Agent(AgentId("a1".into()));
        app.term = Some(AttachedTerm::new(a1.clone(), 40, 10));
        let mut out = Vec::new();

        press(&mut app, KeyCode::Char('a'), KeyModifiers::NONE, &mut out);
        assert!(
            matches!(
                &app.overlay,
                Some(Overlay::Confirm(c))
                    if c.action == PendingAction::ArchiveAgent(AgentId("a1".into()))
            ),
            "a opens the archive confirm: {:?}",
            app.overlay
        );
        assert!(
            !out.iter()
                .any(|r| matches!(r, ClientRequest::ArchiveAgent { .. })),
            "nothing is archived before the answer: {out:?}"
        );
        assert!(
            app.term.is_some(),
            "the pane stays attached while the dialog is up"
        );

        // Esc backs out: the session stays, nothing was sent.
        press(&mut app, KeyCode::Esc, KeyModifiers::NONE, &mut out);
        assert!(app.overlay.is_none(), "Esc closes the confirm");
        assert!(
            !out.iter()
                .any(|r| matches!(r, ClientRequest::ArchiveAgent { .. })),
            "backing out archives nothing: {out:?}"
        );
        assert!(app.term.is_some(), "backing out keeps the pane");

        // Enter goes through, exactly as the bare key would have.
        press(&mut app, KeyCode::Char('a'), KeyModifiers::NONE, &mut out);
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE, &mut out);
        assert!(app.overlay.is_none(), "Enter closes the confirm");
        assert!(
            out.iter().any(|r| matches!(
                r,
                ClientRequest::ArchiveAgent { id, .. } if *id == AgentId("a1".into())
            )),
            "Enter archives the agent: {out:?}"
        );
        assert!(app.term.is_none(), "the archive releases the pane");
    })
}

/// The row menu's Archive asks the same way `a` does.
#[test]
fn the_row_menus_archive_asks_too() {
    with_default_config(|| {
        let mut app = App::new();
        seed_tree(&mut app);
        let mut out = Vec::new();
        run_menu_action(
            &mut app,
            MenuAction::ArchiveAgent(AgentId("a1".into())),
            &mut out,
        );
        assert!(
            matches!(
                &app.overlay,
                Some(Overlay::Confirm(c)) if matches!(c.action, PendingAction::ArchiveAgent(_))
            ),
            "the menu's Archive asks too: {:?}",
            app.overlay
        );
        assert!(out.is_empty(), "nothing sent before the answer: {out:?}");
    })
}

/// With **Confirm on archive** off, `a` archives at once — no dialog,
/// the pane released — and arms the RELEASE WATCH, so a held `a`
/// archives the one card.
#[test]
fn a_archives_at_once_with_the_confirm_off() {
    with_config_json(r#"{"ask_before_archive": false}"#, || {
        let mut app = App::new();
        seed_tree(&mut app); // p1 / w1(main) / a1
        app.focus = Focus::Sessions;
        app.sel_session = 0;
        let a1 = SessionRef::Agent(AgentId("a1".into()));
        app.term = Some(AttachedTerm::new(a1, 40, 10));
        let mut out = Vec::new();

        press(&mut app, KeyCode::Char('a'), KeyModifiers::NONE, &mut out);
        assert!(app.overlay.is_none(), "no confirm: {:?}", app.overlay);
        assert!(
            out.iter().any(|r| matches!(
                r,
                ClientRequest::ArchiveAgent { id, .. } if *id == AgentId("a1".into())
            )),
            "a archives the agent: {out:?}"
        );
        assert!(app.term.is_none(), "the archive releases the pane");
        assert!(app.release_watch.is_some(), "and the key is watched");
    })
}

/// The row menu's Archive skips the dialog the same way `a` does.
#[test]
fn the_row_menus_archive_skips_the_confirm_when_it_is_off() {
    with_config_json(r#"{"ask_before_archive": false}"#, || {
        let mut app = App::new();
        seed_tree(&mut app);
        let mut out = Vec::new();
        run_menu_action(
            &mut app,
            MenuAction::ArchiveAgent(AgentId("a1".into())),
            &mut out,
        );
        assert!(app.overlay.is_none(), "no confirm: {:?}", app.overlay);
        assert!(
            out.iter()
                .any(|r| matches!(r, ClientRequest::ArchiveAgent { .. })),
            "the menu's Archive archives at once: {out:?}"
        );
        assert!(app.release_watch.is_none(), "a click has no key to watch");
    })
}

/// Archiving a row ABOVE the cursor must not drag the highlight onto a
/// different session — the cursor follows the session it was on.
#[test]
fn archiving_a_row_above_keeps_the_cursor_on_its_session() {
    let mut app = App::new();
    seed_tree(&mut app); // p1 / w1(main) / a1
    hse(
        &mut app,
        ServerEvent::EntityUpserted {
            entity: agent_entity("a2", "w1", "agent-2", false),
        },
    );
    app.focus = Focus::Sessions;
    app.sel_session = 1; // a2
    let a2 = SessionRef::Agent(AgentId("a2".into()));
    app.term = Some(AttachedTerm::new(a2.clone(), 40, 10));

    let mut out = Vec::new();
    handle_server_event(
        &mut app,
        ServerEvent::EntityUpserted {
            entity: agent_entity("a1", "w1", "agent-1", true),
        },
        &mut out,
    );
    assert_eq!(
        app.selected_session().map(|a| a.name),
        Some("agent-2".into()),
        "cursor followed its session up the list"
    );
    assert_eq!(
        app.term.as_ref().map(|t| t.sref.clone()),
        Some(a2),
        "the attached pane is untouched"
    );
    assert!(
        !out.iter()
            .any(|r| matches!(r, ClientRequest::Attach { .. })),
        "no re-attach when the highlighted session didn't change: {out:?}"
    );
}

/// Archiving the LAST live agent steps the cursor up onto the agent
/// above it, not down onto the first terminal that slides into its
/// slot. Archiving the top one still moves the cursor down.
#[test]
fn archiving_the_last_live_agent_lands_on_the_one_above() {
    let mut app = App::new();
    seed_tree(&mut app); // p1 / w1(main) / a1
    for (id, name) in [("a2", "agent-2"), ("a3", "agent-3")] {
        hse(
            &mut app,
            ServerEvent::EntityUpserted {
                entity: agent_entity(id, "w1", name, false),
            },
        );
    }
    seed_terminal(&mut app, "t1", "shell");
    app.focus = Focus::Sessions;
    app.sel_session = row_of(&app, "a3");
    assert!(
        matches!(
            app.visible_session_rows().get(app.sel_session + 1),
            Some(SessionRow::Terminal(_))
        ),
        "agent-3 is the last live agent, the terminal right under it"
    );
    let a3 = SessionRef::Agent(AgentId("a3".into()));
    app.term = Some(AttachedTerm::new(a3, 40, 10));

    let mut out = Vec::new();
    handle_server_event(
        &mut app,
        ServerEvent::EntityUpserted {
            entity: agent_entity("a3", "w1", "agent-3", true),
        },
        &mut out,
    );
    let a2 = SessionRef::Agent(AgentId("a2".into()));
    assert_eq!(
        app.selected_session().map(|a| a.name),
        Some("agent-2".into()),
        "the cursor stepped up onto the agent above"
    );
    assert!(
        out.iter()
            .any(|r| matches!(r, ClientRequest::Attach { session, .. } if *session == a2)),
        "the agent above attaches: {out:?}"
    );
    assert_eq!(app.term.as_ref().map(|t| t.sref.clone()), Some(a2.clone()));

    // From the top of the list the cursor moves down, as before.
    app.sel_session = row_of(&app, "a1");
    let a1 = SessionRef::Agent(AgentId("a1".into()));
    app.term = Some(AttachedTerm::new(a1, 40, 10));
    out.clear();
    handle_server_event(
        &mut app,
        ServerEvent::EntityUpserted {
            entity: agent_entity("a1", "w1", "agent-1", true),
        },
        &mut out,
    );
    assert_eq!(
        app.selected_session().map(|a| a.name),
        Some("agent-2".into()),
        "the cursor moved down onto the agent below"
    );
    assert_eq!(app.term.as_ref().map(|t| t.sref.clone()), Some(a2));
}

/// Deleting the selected session lands the cursor on the next row and
/// shows it in the pane.
#[test]
fn deleting_selected_agent_previews_the_next_row() {
    use nebula_core::EntityId;
    let mut app = App::new();
    seed_tree(&mut app); // p1 / w1(main) / a1
    hse(
        &mut app,
        ServerEvent::EntityUpserted {
            entity: agent_entity("a2", "w1", "agent-2", false),
        },
    );
    app.focus = Focus::Sessions;
    app.sel_session = 0; // a1
    let a1 = SessionRef::Agent(AgentId("a1".into()));
    app.term = Some(AttachedTerm::new(a1.clone(), 40, 10));

    let mut out = Vec::new();
    handle_server_event(
        &mut app,
        ServerEvent::EntityRemoved {
            id: EntityId::Agent(AgentId("a1".into())),
        },
        &mut out,
    );
    let a2 = SessionRef::Agent(AgentId("a2".into()));
    assert_eq!(
        app.selected_session().map(|a| a.name),
        Some("agent-2".into()),
        "cursor landed on the next row"
    );
    assert!(
        out.iter()
            .any(|r| matches!(r, ClientRequest::Attach { session, .. } if *session == a2)),
        "the next row's session attaches: {out:?}"
    );
    assert_eq!(app.term.as_ref().map(|t| t.sref.clone()), Some(a2));
}

/// Removing the only session leaves nothing to preview: the pane blanks
/// instead of keeping the dead session's screen.
#[test]
fn deleting_the_last_session_blanks_the_pane() {
    use nebula_core::EntityId;
    let mut app = App::new();
    seed_tree(&mut app); // p1 / w1(main) / a1
    let a1 = SessionRef::Agent(AgentId("a1".into()));
    app.term = Some(AttachedTerm::new(a1.clone(), 40, 10));
    app.focus = Focus::Terminal;
    app.term_locked = true;

    let mut out = Vec::new();
    handle_server_event(
        &mut app,
        ServerEvent::EntityRemoved {
            id: EntityId::Agent(AgentId("a1".into())),
        },
        &mut out,
    );
    assert!(app.term.is_none(), "the pane blanks");
    assert_eq!(app.focus, Focus::Sessions, "focus hands back to the list");
    assert!(
        out.iter()
            .any(|r| matches!(r, ClientRequest::Detach { session } if *session == a1)),
        "the dead session detaches: {out:?}"
    );
}

/// Deleting the selected worktree lands the cursor on a neighbor and
/// brings up that neighbor's remembered session, like a manual switch.
#[test]
fn deleting_selected_worktree_shows_the_neighbor_worktrees_session() {
    let mut app = App::new();
    seed_tree(&mut app); // p1 / w1(main) / a1
    hse(
        &mut app,
        ServerEvent::EntityUpserted {
            entity: wt_entity("w2", "p1", "feat", false),
        },
    );
    hse(
        &mut app,
        ServerEvent::EntityUpserted {
            entity: agent_entity("a2", "w2", "agent-2", false),
        },
    );
    let w2_index = app
        .visible_worktrees()
        .iter()
        .position(|w| w.id.0 == "w2")
        .unwrap();
    app.sel_worktree = w2_index;
    app.sel_session = 0; // a2
    app.focus = Focus::Worktrees;
    let a1 = SessionRef::Agent(AgentId("a1".into()));
    let a2 = SessionRef::Agent(AgentId("a2".into()));
    app.term = Some(AttachedTerm::new(a2.clone(), 40, 10));
    app.last_session_for_worktree
        .insert(WorktreeId("w1".into()), a1.clone());

    let mut out = Vec::new();
    run_pending_action(
        &mut app,
        PendingAction::DeleteWorktree(WorktreeId("w2".into())),
        &mut out,
    );
    assert_eq!(
        app.selected_worktree().map(|w| w.id.0.clone()),
        Some("w1".into()),
        "cursor landed on the surviving worktree"
    );
    assert!(
        out.iter()
            .any(|r| matches!(r, ClientRequest::Attach { session, .. } if *session == a1)),
        "the survivor's remembered session attaches: {out:?}"
    );
    assert_eq!(
        app.term.as_ref().map(|t| t.sref.clone()),
        Some(a1),
        "the pane shows the survivor's session, not the deleted one"
    );
}

/// Removing the selected project restores the neighbor project's
/// remembered worktree + session, like switching to it manually.
#[test]
fn removing_selected_project_restores_the_neighbor_projects_context() {
    use nebula_core::EntityId;
    let mut app = App::new();
    seed_tree(&mut app); // p1 / w1(main) / a1
    hse(
        &mut app,
        ServerEvent::EntityUpserted {
            entity: project("p2", "two", 1),
        },
    );
    hse(
        &mut app,
        ServerEvent::EntityUpserted {
            entity: wt_entity("w2", "p2", "main2", true),
        },
    );
    hse(
        &mut app,
        ServerEvent::EntityUpserted {
            entity: agent_entity("a2", "w2", "agent-2", false),
        },
    );
    app.sel_project = 1; // p2
    app.sel_worktree = 0; // w2
    app.sel_session = 0; // a2
    app.focus = Focus::Projects;
    let a1 = SessionRef::Agent(AgentId("a1".into()));
    let a2 = SessionRef::Agent(AgentId("a2".into()));
    app.term = Some(AttachedTerm::new(a2.clone(), 40, 10));
    app.last_worktree_for_project
        .insert(nebula_core::ProjectId("p1".into()), WorktreeId("w1".into()));
    app.last_session_for_worktree
        .insert(WorktreeId("w1".into()), a1.clone());

    let mut out = Vec::new();
    handle_server_event(
        &mut app,
        ServerEvent::EntityRemoved {
            id: EntityId::Project(nebula_core::ProjectId("p2".into())),
        },
        &mut out,
    );
    assert_eq!(
        app.selected_project().map(|p| p.name.clone()),
        Some("demo".into()),
        "cursor landed on the surviving project"
    );
    assert_eq!(
        app.selected_worktree().map(|w| w.id.0.clone()),
        Some("w1".into()),
        "its remembered worktree is selected"
    );
    assert_eq!(
        app.term.as_ref().map(|t| t.sref.clone()),
        Some(a1),
        "the pane shows the survivor's remembered session"
    );
}

// ---- another project ----

/// A second project, "secret", with its root checkout `main` (w9),
/// next to `seed_tree`'s demo project.
pub(super) fn seed_other_project(app: &mut App) {
    use nebula_core::{Entity, Project, ProjectId, Worktree, WorktreeId};
    hse(
        app,
        ServerEvent::EntityUpserted {
            entity: Entity::Project(Project {
                id: ProjectId("p9".into()),
                name: "secret".into(),
                repo_path: "/tmp/secret".into(),
                sort_order: 9,
            }),
        },
    );
    hse(
        app,
        ServerEvent::EntityUpserted {
            entity: Entity::Worktree(Worktree {
                id: WorktreeId("w9".into()),
                project_id: ProjectId("p9".into()),
                path: "/tmp/secret".into(),
                branch: "main".into(),
                is_main: true,
                sort_order: 0,
            }),
        },
    );
}

/// Every project has a row, and `/` paths each row from its project
/// down — no grouping above the project to scope anything to.
#[test]
fn every_project_has_a_row_and_a_palette_path() {
    let mut app = App::new();
    seed_tree(&mut app);
    seed_other_project(&mut app);
    assert_eq!(app.project_rows().len(), 2, "demo and secret");

    let palette = Palette::new(&app.tree, false, &app.open_prs, false);
    let texts: Vec<&str> = palette.items.iter().map(|i| i.text.as_str()).collect();
    assert_eq!(
        texts,
        [
            "demo",
            "secret",
            "demo/main",
            "secret/main",
            "demo/main/agent-1",
        ],
        "every row pathed from its project down"
    );
}

/// Picking a row in another project lands there — its project and
/// checkout selected.
#[test]
fn jumping_to_another_projects_worktree_selects_it() {
    let mut app = App::new();
    seed_tree(&mut app);
    seed_other_project(&mut app);
    let mut out = Vec::new();

    jump_to_target(
        &mut app,
        PaletteTarget::Worktree(WorktreeId("w9".into())),
        Landing::FocusOnly,
        &mut out,
    );
    assert_eq!(
        app.selected_project().map(|p| p.name.clone()),
        Some("secret".into())
    );
    assert_eq!(
        app.selected_worktree().map(|w| w.branch.clone()),
        Some("main".into())
    );
    assert_eq!(app.focus, Focus::Sessions);
    assert!(app.flash.is_none(), "flash: {:?}", app.flash);
}

/// A pull request in another project is a jump there too: its project
/// is selected, and the cursor lands on the row under that project's
/// checkouts — nothing about the pull request needs a worktree or
/// session of its own.
#[test]
fn jumping_to_another_projects_pr_selects_it() {
    let mut app = App::new();
    seed_tree(&mut app);
    seed_other_project(&mut app);
    let now = std::time::Instant::now();
    app.open_prs.insert(
        nebula_core::ProjectId("p9".into()),
        crate::app::OpenPrs {
            list: vec![crate::pull_request::OpenPr {
                number: 3,
                title: "Hush the logs".into(),
                url: "https://github.com/o/secret/pull/3".into(),
                is_draft: false,
                health: Default::default(),
                head: "hush".into(),
            }],
            at: now,
            due: now + OPEN_PRS_REFRESH,
            step: OPEN_PRS_REFRESH,
        },
    );
    let mut out = Vec::new();

    press(&mut app, KeyCode::Char('/'), KeyModifiers::NONE, &mut out);
    for c in "hush".chars() {
        press(&mut app, KeyCode::Char(c), KeyModifiers::NONE, &mut out);
    }
    {
        let p = palette(&app);
        assert_eq!(
            p.items[p.matches[p.selected].item].text, "secret/#3 Hush the logs",
            "pathed under its project like every other row"
        );
    }
    press(&mut app, KeyCode::Enter, KeyModifiers::NONE, &mut out);
    assert_eq!(
        app.selected_project().map(|p| p.name.clone()),
        Some("secret".into())
    );
    assert_eq!(app.selected_worktree_pr().map(|pr| pr.number), Some(3));
    assert_eq!(
        app.previewed_pr().map(|pr| pr.url),
        Some("https://github.com/o/secret/pull/3".into())
    );
    assert_eq!(app.focus, Focus::Worktrees);
    assert!(app.flash.is_none(), "flash: {:?}", app.flash);
}

/// Crossing projects attaches exactly once. The jump deliberately skips
/// the destination's remembered-session restore: doing it would attach
/// that session and then detach it one request later, when the jump
/// lands on the row it was actually asked for.
#[test]
fn a_cross_project_session_jump_attaches_only_the_session_picked() {
    let mut app = App::new();
    seed_tree(&mut app);
    seed_other_project(&mut app);
    seed_background_run(&mut app);
    // A second session over there, and it's the remembered one — so a
    // restoring switch would attach *it* before the jump attaches the
    // one actually picked.
    hse(
        &mut app,
        ServerEvent::EntityUpserted {
            entity: nebula_core::Entity::Agent(nebula_core::Agent {
                id: AgentId("a8".into()),
                worktree_id: WorktreeId("w9".into()),
                name: "other".into(),
                status: nebula_core::AgentStatus::Fresh,
                archived: false,
                archived_at: 0,
                unseen: false,
                kind: AgentKind::Claude,
                custom_harness: None,
                model: None,
                effort: None,
                session_id: None,
                cloud_session_id: None,
                sort_order: 0,
                status_changed_at: 0,
                alive: true,
                issue_url: None,
                recent_prompts: Vec::new(),
            }),
        },
    );
    // Park the pane on a session in the project we're leaving.
    let mut out = Vec::new();
    attach(&mut app, SessionRef::Agent(AgentId("a1".into())), &mut out);
    app.last_session_for_worktree.insert(
        WorktreeId("w9".into()),
        SessionRef::Agent(AgentId("a8".into())),
    );
    out.clear();

    jump_to_target(
        &mut app,
        PaletteTarget::Session(AgentId("a9".into())),
        Landing::Attach,
        &mut out,
    );
    let attaches: Vec<&ClientRequest> = out
        .iter()
        .filter(|r| matches!(r, ClientRequest::Attach { .. }))
        .collect();
    assert_eq!(
        attaches.len(),
        1,
        "one attach, not attach-detach-attach: {out:?}"
    );
    assert!(
        matches!(
            attaches[0],
            ClientRequest::Attach { session, .. } if session == &SessionRef::Agent(AgentId("a9".into()))
        ),
        "{:?}",
        attaches[0]
    );
    assert_eq!(app.focus, Focus::Terminal);
    assert_eq!(
        app.selected_project().map(|p| p.name.clone()),
        Some("secret".into())
    );
}

/// A running agent in the other project's checkout.
fn seed_background_run(app: &mut App) {
    use nebula_core::{Agent, AgentId, AgentStatus, Entity, WorktreeId};
    hse(
        app,
        ServerEvent::EntityUpserted {
            entity: Entity::Agent(Agent {
                id: AgentId("a9".into()),
                worktree_id: WorktreeId("w9".into()),
                name: "bg-run".into(),
                status: AgentStatus::Running,
                archived: false,
                archived_at: 0,
                unseen: false,
                kind: nebula_core::AgentKind::Claude,
                custom_harness: None,
                model: None,
                effort: None,
                session_id: None,
                cloud_session_id: None,
                sort_order: 0,
                status_changed_at: 0,
                alive: true,
                issue_url: None,
                recent_prompts: Vec::new(),
            }),
        },
    );
}

// ---- agent presets ----

/// Route the preset store at a temp file (and the config at a default
/// one) and pre-seed two presets: "reviewer" (claude · opus · high,
/// wrapped) then "scratch" (codex · gpt-5.5, bare).
pub(super) fn with_seeded_presets(f: impl FnOnce()) {
    use crate::agent_presets::AgentPreset;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("agent_presets.json");
    with_default_config(|| {
        crate::agent_presets::with_presets_path(path, || {
            crate::agent_presets::save(&[
                AgentPreset {
                    name: "reviewer".into(),
                    kind: AgentKind::Claude,
                    custom_harness: None,
                    model: Some("opus".into()),
                    effort: Some("high".into()),
                    prefix: "Be strict.".into(),
                    postfix: "Run the tests.".into(),
                    skip_task: false,
                },
                AgentPreset {
                    name: "scratch".into(),
                    kind: AgentKind::Codex,
                    custom_harness: None,
                    model: Some("gpt-5.5".into()),
                    effort: None,
                    prefix: String::new(),
                    postfix: String::new(),
                    skip_task: false,
                },
            ])
            .unwrap();
            f();
        })
    });
}

/// A seeded app with FOCUS on the SESSIONS PANEL and the list open.
fn open_presets(app: &mut App, out: &mut Vec<ClientRequest>) {
    seed_tree(app);
    app.focus = Focus::Sessions;
    press(app, KeyCode::Char('e'), KeyModifiers::NONE, out);
    assert!(
        matches!(&app.overlay, Some(Overlay::AgentPresets(_))),
        "e in Sessions should open the presets list, got {:?}",
        app.overlay
    );
}

fn type_text(app: &mut App, text: &str, out: &mut Vec<ClientRequest>) {
    for c in text.chars() {
        press(app, KeyCode::Char(c), KeyModifiers::NONE, out);
    }
}

/// The launch that landed a PR SESSION in the main checkout: `e` on
/// the OPEN PRS row of a contributor's pull request from their fork's
/// own `main`, a `skip`-task preset, Enter — no box, no typing. The
/// row's checkout branch is the fork's (`givemeurhats/main`, as `gh pr
/// list` is parsed), so the ROOT WORKTREE on our `main` is not "the
/// checkout already on the head branch": the stand-in rows go up at
/// once, nested under the pull request, the create names the fork's
/// branch, and nothing is launched into — or staged under — the root.
#[test]
fn a_skip_preset_on_a_forks_main_cuts_the_prs_own_checkout_not_the_root() {
    with_seeded_presets(|| {
        let mut presets = crate::agent_presets::load();
        presets.push(crate::agent_presets::AgentPreset {
            name: "review and merge".into(),
            kind: AgentKind::Claude,
            custom_harness: None,
            model: None,
            effort: None,
            prefix: "Investigate whether this pull request should be merged.".into(),
            postfix: String::new(),
            skip_task: true,
        });
        crate::agent_presets::save(&presets).unwrap();
        let skip_row = presets.len() - 1;

        let mut app = App::new();
        seed_tree(&mut app);
        let root = WorktreeId("w1".into());
        assert!(
            app.tree
                .worktrees
                .iter()
                .any(|w| w.id == root && w.is_main && w.branch == "main"),
            "the ROOT WORKTREE is on our main: {:?}",
            app.tree.worktrees
        );
        let list = crate::pull_request::parse_list(&crate::pull_request::list_answer(
            r#"[{"number":129,"title":"Prefer PowerShell 7",
                     "url":"https://github.com/o/r/pull/129","isDraft":false,
                     "headRefName":"main","isCrossRepository":true,
                     "headRepositoryOwner":{"login":"givemeurhats"}}]"#,
        ))
        .expect("parsed");
        let project = app.selected_project().expect("a project").id.clone();
        let now = std::time::Instant::now();
        app.open_prs.insert(
            project,
            crate::app::OpenPrs {
                list,
                at: now,
                due: now + OPEN_PRS_REFRESH,
                step: OPEN_PRS_REFRESH,
            },
        );
        let url = "https://github.com/o/r/pull/129";
        app.focus = Focus::Worktrees;
        app.sel_worktree = app.open_pr_row_of(url).expect("#129 is a row");
        let pr_row = app.sel_worktree;
        let agents_under_root = |app: &App| {
            app.tree
                .agents
                .iter()
                .filter(|a| a.worktree_id == root)
                .count()
        };
        let before = agents_under_root(&app);
        let mut out = Vec::new();

        press(&mut app, KeyCode::Char('e'), KeyModifiers::NONE, &mut out);
        for _ in 0..skip_row {
            press(&mut app, KeyCode::Down, KeyModifiers::NONE, &mut out);
        }
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE, &mut out);
        assert!(
            app.overlay.is_none(),
            "no box for a skip preset: {:?}",
            app.overlay
        );

        let creates: Vec<&ClientRequest> = out
            .iter()
            .filter(|r| {
                matches!(
                    r,
                    ClientRequest::CreatePrAgent { .. }
                        | ClientRequest::CreateAgent { .. }
                        | ClientRequest::CreateWorktree { .. }
                )
            })
            .collect();
        assert!(
            matches!(
                creates.as_slice(),
                [ClientRequest::CreatePrAgent {
                    pr_url,
                    head,
                    starting_prompt: Some(text),
                    ..
                }] if pr_url == url
                    && head == "givemeurhats/main"
                    && text == "Investigate whether this pull request should be merged."
            ),
            "one PR create, on the fork's branch: {out:?}"
        );

        let stand_in = app
            .tree
            .worktrees
            .iter()
            .find(|w| w.branch == "givemeurhats/main")
            .map(|w| w.id.clone())
            .expect("the fork branch's stand-in row");
        assert!(app.is_placeholder_worktree(&stand_in));
        assert_eq!(
            app.worktree_row_of(&stand_in),
            Some(pr_row + 1),
            "nested under the pull request's row"
        );
        assert_eq!(app.sel_worktree, pr_row + 1, "the cursor is on it");
        assert_eq!(
            agents_under_root(&app),
            before,
            "nothing is staged under the root"
        );
        assert!(
            app.tree
                .agents
                .iter()
                .any(|a| a.worktree_id == stand_in && app.is_placeholder_agent(&a.id)),
            "the session's stand-in is under the fork's checkout: {:?}",
            app.tree.agents
        );
    });
}

/// The pull request under the Worktrees cursor is what `e` is for from
/// whichever panel has FOCUS — the pane reading it above all, where a
/// review preset is reached for — as it is for `p`: a pull request row
/// has no worktree and no sessions for the presets list to manage, so
/// the key opens the PR SESSION picker rather than saying where it
/// works.
#[test]
fn e_reaches_the_pr_preset_picker_from_any_panel() {
    with_seeded_presets(|| {
        for focus in [Focus::Projects, Focus::Sessions, Focus::Terminal] {
            let mut app = App::new();
            seed_tree(&mut app);
            seed_open_prs(&mut app, &[(7, "Attach links")]);
            app.sel_worktree = 1;
            app.focus = focus;
            assert_eq!(app.selected_worktree_pr().map(|p| p.number), Some(7));
            let mut out = Vec::new();
            press(&mut app, KeyCode::Char('e'), KeyModifiers::NONE, &mut out);
            let Some(Overlay::AgentPresets(view)) = &app.overlay else {
                panic!(
                    "e from {focus:?} opens the PR preset picker, got {:?} ({:?})",
                    app.overlay, app.flash
                );
            };
            let back = view.quick.as_ref().expect("a picker, not the manager");
            assert_eq!(back.launch.pr.as_ref().map(|pr| pr.number), Some(7));
            assert!(out.is_empty(), "{out:?}");
        }
    });
}

/// The presets list `e` opens on a PROJECT OPEN PRS GROUP row manages
/// presets as the one on a checkout's row does: `a` adds, `e` edits,
/// `d` deletes. Every way back — the editor's save and its Esc, the
/// delete confirm's either answer — lands in the PR picker with the
/// pull request still riding it, so the next Enter is still a PR
/// SESSION.
#[test]
fn the_pr_preset_picker_adds_edits_and_deletes_presets() {
    with_seeded_presets(|| {
        let mut app = App::new();
        seed_tree(&mut app);
        seed_open_prs(&mut app, &[(7, "Attach links")]);
        app.focus = Focus::Worktrees;
        app.sel_worktree = 1;
        let mut out = Vec::new();
        press(&mut app, KeyCode::Char('e'), KeyModifiers::NONE, &mut out);
        // The row names, and the cursor, of the PR picker on screen.
        fn pr_picker(app: &App) -> (Vec<String>, usize) {
            let Some(Overlay::AgentPresets(view)) = &app.overlay else {
                panic!("back in the PR picker, got {:?}", app.overlay);
            };
            let back = view.quick.as_ref().expect("the picker, not the manager");
            assert_eq!(back.launch.pr.as_ref().map(|pr| pr.number), Some(7));
            let names = view.presets.iter().map(|p| p.name.clone()).collect();
            (names, view.selected)
        }
        let (seeded, _) = pr_picker(&app);
        assert_eq!(seeded, ["reviewer", "scratch"]);

        // a: a new preset, saved, the cursor on it.
        press(
            &mut app,
            KeyCode::Char('a'),
            KeyModifiers::CONTROL,
            &mut out,
        );
        assert!(
            matches!(&app.overlay, Some(Overlay::AgentPresetEditor(_))),
            "a opens the editor: {:?}",
            app.overlay
        );
        type_text(&mut app, "triage", &mut out);
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE, &mut out);
        let (names, cursor) = pr_picker(&app);
        assert_eq!(names, ["reviewer", "scratch", "triage"]);
        assert_eq!(cursor, 2, "the cursor lands on the new row");

        // e: edited in place.
        press(
            &mut app,
            KeyCode::Char('e'),
            KeyModifiers::CONTROL,
            &mut out,
        );
        assert!(
            matches!(&app.overlay, Some(Overlay::AgentPresetEditor(e)) if e.editing == Some(2)),
            "e edits the row under the cursor: {:?}",
            app.overlay
        );
        type_text(&mut app, " pr", &mut out);
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE, &mut out);
        assert_eq!(pr_picker(&app).0[2], "triage pr");

        // Esc backs out of the editor unsaved, to the picker too.
        press(
            &mut app,
            KeyCode::Char('a'),
            KeyModifiers::CONTROL,
            &mut out,
        );
        type_text(&mut app, "abandoned", &mut out);
        press(&mut app, KeyCode::Esc, KeyModifiers::NONE, &mut out);
        assert_eq!(pr_picker(&app).0.len(), 3);

        // d: `n` keeps the preset, `y` drops it — the picker either way.
        press(&mut app, KeyCode::Down, KeyModifiers::NONE, &mut out);
        press(&mut app, KeyCode::Down, KeyModifiers::NONE, &mut out);
        press(
            &mut app,
            KeyCode::Char('d'),
            KeyModifiers::CONTROL,
            &mut out,
        );
        assert!(
            matches!(&app.overlay, Some(Overlay::Confirm(_))),
            "d asks first: {:?}",
            app.overlay
        );
        press(&mut app, KeyCode::Char('n'), KeyModifiers::NONE, &mut out);
        assert_eq!(pr_picker(&app).0.len(), 3);
        press(
            &mut app,
            KeyCode::Char('d'),
            KeyModifiers::CONTROL,
            &mut out,
        );
        press(&mut app, KeyCode::Char('y'), KeyModifiers::NONE, &mut out);
        assert_eq!(pr_picker(&app).0, seeded);
        assert_eq!(crate::agent_presets::load().len(), 2, "the store agrees");

        // And a pick from it is still for the pull request.
        press(&mut app, KeyCode::Up, KeyModifiers::NONE, &mut out);
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE, &mut out);
        let Some(Overlay::Prompt(prompt)) = &app.overlay else {
            panic!("the pick hands the box back, got {:?}", app.overlay);
        };
        assert_eq!(
            prompt.title,
            "Quick prompt · PR #7 · reviewer (claude · opus · high)"
        );
        assert!(out.is_empty(), "managing presets sends nothing: {out:?}");
    });
}

/// `e` on a PROJECT OPEN PRS GROUP row is the preset picker for a PR
/// SESSION: the pick hands the QUICK PROMPT back titled for the PR,
/// `Ctrl+N` in it is refused (the DAEMON picks the checkout), and
/// Enter sends one `CreatePrAgent` — the PR's URL and head branch,
/// the preset's harness / model / effort, and prefix + task + postfix
/// as its STARTING PROMPT — with the stand-in rows up for the checkout
/// the DAEMON is about to cut. A checkout already on the head branch
/// stages nothing: the DAEMON reuses it. A `skip`-task preset launches
/// from the picker at once.
#[test]
fn e_on_an_open_pr_row_picks_a_preset_and_launches_a_pr_session_on_it() {
    with_seeded_presets(|| {
        let mut app = App::new();
        seed_tree(&mut app);
        seed_open_prs(&mut app, &[(7, "Attach links")]);
        app.focus = Focus::Worktrees;
        app.sel_worktree = 1;
        assert_eq!(app.selected_worktree_pr().map(|p| p.number), Some(7));
        let mut out = Vec::new();

        press(&mut app, KeyCode::Char('e'), KeyModifiers::NONE, &mut out);
        let Some(Overlay::AgentPresets(view)) = &app.overlay else {
            panic!(
                "e on a PR row opens the preset picker, got {:?}",
                app.overlay
            );
        };
        assert!(view.is_picker(), "a picker for the PR, not the manager");
        let back = view.quick.as_ref().expect("is_picker");
        assert_eq!(back.launch.pr.as_ref().map(|pr| pr.number), Some(7));
        assert_eq!(
            back.launch.target,
            crate::quick_prompt::QuickTarget::Worktree(WorktreeId("w1".into())),
            "the ROOT WORKTREE names the project the create goes to"
        );
        assert!(out.is_empty(), "nothing is sent for a pick: {out:?}");

        // Enter on "reviewer": the box, for the PR and the preset.
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE, &mut out);
        let Some(Overlay::Prompt(prompt)) = &app.overlay else {
            panic!("the pick hands the box back, got {:?}", app.overlay);
        };
        assert_eq!(
            prompt.title,
            "Quick prompt · PR #7 · reviewer (claude · opus · high)"
        );

        // Ctrl+N has nothing to flip: the checkout is the DAEMON's.
        press(
            &mut app,
            KeyCode::Char('n'),
            KeyModifiers::CONTROL,
            &mut out,
        );
        assert_eq!(
            app.flash.as_deref(),
            Some("quick prompt: a PR session runs in the pull request's own checkout")
        );
        assert!(
            matches!(
                &app.overlay,
                Some(Overlay::Prompt(prompt))
                    if matches!(&prompt.kind, PromptKind::QuickPrompt(launch) if launch.pr.is_some())
            ),
            "the box stays up, for the PR: {:?}",
            app.overlay
        );

        type_text(&mut app, "Fix auth", &mut out);
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE, &mut out);
        assert!(app.overlay.is_none(), "{:?}", app.overlay);
        let creates: Vec<&ClientRequest> = out
            .iter()
            .filter(|r| {
                matches!(
                    r,
                    ClientRequest::CreatePrAgent { .. } | ClientRequest::CreateAgent { .. }
                )
            })
            .collect();
        assert!(
            matches!(
                creates.as_slice(),
                [ClientRequest::CreatePrAgent {
                    project,
                    kind: AgentKind::Claude,
                    model: Some(model),
                    effort: Some(effort),
                    auto_title: true,
                    pr_url,
                    head,
                    starting_prompt: Some(text),
                    ..
                }] if project.as_str() == "p1"
                    && model == "opus"
                    && effort == "high"
                    && pr_url == "https://github.com/o/r/pull/7"
                    && head == "pr-7-head"
                    && text == "Be strict.\n\nFix auth\n\nRun the tests."
            ),
            "one PR create with the sandwiched prompt: {out:?}"
        );
        assert!(
            out.iter()
                .all(|r| !matches!(r, ClientRequest::PrewarmAgent { .. })),
            "an unscoped warm CLI must not start for a PR SESSION: {out:?}"
        );
        // No checkout on the head branch yet: the stand-in rows are up
        // for the one the DAEMON is cutting.
        assert!(
            app.tree
                .worktrees
                .iter()
                .any(|w| w.branch == "pr-7-head" && app.is_placeholder_worktree(&w.id)),
            "{:?}",
            app.tree.worktrees
        );
        out.clear();

        // A checkout already on the head branch is reused: nothing is
        // staged, and the create goes out the same.
        let mut app = App::new();
        seed_tree(&mut app);
        hse(
            &mut app,
            ServerEvent::EntityUpserted {
                entity: nebula_core::Entity::Worktree(nebula_core::Worktree {
                    id: WorktreeId("w7".into()),
                    project_id: nebula_core::ProjectId("p1".into()),
                    path: "/tmp/demo-worktrees/pr-7-head".into(),
                    branch: "pr-7-head".into(),
                    is_main: false,
                    sort_order: 1,
                }),
            },
        );
        seed_open_prs(&mut app, &[(7, "Attach links")]);
        app.focus = Focus::Worktrees;
        // The checkout on the head branch lists under the pull
        // request, so the pull request's row is looked up, not counted.
        app.sel_worktree = app.open_pr_row_of(&pr_url(7)).expect("#7 is a row");
        assert_eq!(app.selected_worktree_pr().map(|p| p.number), Some(7));
        let rows = app.tree.worktrees.len();
        press(&mut app, KeyCode::Char('e'), KeyModifiers::NONE, &mut out);
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE, &mut out);
        type_text(&mut app, "Fix auth", &mut out);
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE, &mut out);
        assert_eq!(
            app.tree.worktrees.len(),
            rows,
            "no stand-in for a reused checkout"
        );
        assert!(
            out.iter().any(|r| matches!(
                r,
                ClientRequest::CreatePrAgent { head, starting_prompt: Some(_), .. }
                    if head == "pr-7-head"
            )),
            "{out:?}"
        );
        out.clear();

        // A `skip`-task preset launches from the picker at once, on
        // its prefix and postfix alone.
        let mut presets = crate::agent_presets::load();
        presets.push(crate::agent_presets::AgentPreset {
            name: "ship".into(),
            kind: AgentKind::Claude,
            custom_harness: None,
            model: None,
            effort: None,
            prefix: "Commit and push.".into(),
            postfix: String::new(),
            skip_task: true,
        });
        crate::agent_presets::save(&presets).unwrap();
        press(&mut app, KeyCode::Char('e'), KeyModifiers::NONE, &mut out);
        press(&mut app, KeyCode::Down, KeyModifiers::NONE, &mut out);
        press(&mut app, KeyCode::Down, KeyModifiers::NONE, &mut out);
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE, &mut out);
        assert!(
            app.overlay.is_none(),
            "no box for a skip preset: {:?}",
            app.overlay
        );
        assert!(
            out.iter().any(|r| matches!(
                r,
                ClientRequest::CreatePrAgent { head, starting_prompt: Some(text), .. }
                    if head == "pr-7-head" && text == "Commit and push."
            )),
            "{out:?}"
        );
    });
}

/// The ISSUES MODAL's comment box round-trips: Esc puts the modal back
/// on its row with nothing posted, an empty Enter does the same, and
/// Enter with text hands it to `gh` — here a checkout that isn't on
/// disk, so the box comes back with the text instead.
#[test]
fn the_issue_comment_box_comes_back_to_the_modal_on_its_row() {
    let mut app = App::new();
    let mut out = Vec::new();
    let project = nebula_core::ProjectId("p1".into());
    app.overlay = Some(Overlay::Issues(crate::issues::IssuesView::new(
        project.clone(),
        "demo".into(),
        "/nonexistent/nebula-issue-comment-box".into(),
    )));
    let issue = |number: u64| crate::issues::Issue {
        number,
        url: format!("https://github.com/o/r/issues/{number}"),
        title: format!("issue {number}"),
        author: "webdevcody".into(),
        created_at: "2026-09-10T12:00:00Z".into(),
        updated_at: "2026-09-11T12:00:00Z".into(),
        labels: vec![],
        body: String::new(),
    };
    crate::issues::land_answer(
        &mut app,
        crate::issues::IssuesAnswer::List {
            project,
            list: Some(vec![issue(15), issue(14)]),
        },
    );
    press(&mut app, KeyCode::Down, KeyModifiers::NONE, &mut out);
    let on_row = |app: &App| matches!(&app.overlay, Some(Overlay::Issues(v)) if v.selected == 1);
    assert!(on_row(&app));

    // Esc: back, nothing posted.
    press(
        &mut app,
        KeyCode::Char('c'),
        KeyModifiers::CONTROL,
        &mut out,
    );
    assert!(matches!(&app.overlay, Some(Overlay::Prompt(p)) if p.title.contains("#14")));
    type_text(&mut app, "never mind", &mut out);
    press(&mut app, KeyCode::Esc, KeyModifiers::NONE, &mut out);
    assert!(on_row(&app), "Esc puts the modal back: {:?}", app.overlay);

    // An empty Enter: the same.
    press(
        &mut app,
        KeyCode::Char('c'),
        KeyModifiers::CONTROL,
        &mut out,
    );
    press(&mut app, KeyCode::Enter, KeyModifiers::NONE, &mut out);
    assert!(on_row(&app), "an empty box is a cancel: {:?}", app.overlay);
    assert_eq!(app.flash.as_deref(), Some("cancelled: empty input"));

    // Enter with text: posted — or, off a checkout that isn't on disk,
    // the box comes back with the text, newline and all.
    press(
        &mut app,
        KeyCode::Char('c'),
        KeyModifiers::CONTROL,
        &mut out,
    );
    type_text(&mut app, "on it", &mut out);
    press(&mut app, KeyCode::Enter, KeyModifiers::SHIFT, &mut out);
    type_text(&mut app, "today", &mut out);
    press(&mut app, KeyCode::Enter, KeyModifiers::NONE, &mut out);
    let Some(Overlay::Prompt(prompt)) = &app.overlay else {
        panic!(
            "the box should come back with the text, got {:?}",
            app.overlay
        );
    };
    assert_eq!(prompt.input.as_str(), "on it\ntoday");
    assert!(
        app.flash.as_deref().unwrap().contains("isn't on disk"),
        "{:?}",
        app.flash
    );
    assert!(out.is_empty(), "nothing goes to the daemon: {out:?}");
}

/// The ISSUES MODAL's box lands where every new session's box does:
/// the project's ROOT BRANCH, whatever card the cursor was on — or,
/// with the `quick_prompt_new_worktree` SETTING on, a fresh worktree
/// named after the issue.
#[test]
fn the_issue_quick_prompt_starts_on_the_root_branch_or_a_fresh_worktree() {
    use crate::quick_prompt::QuickTarget;
    let target = |json: &str| {
        with_config_json(json, || {
            let mut app = App::new();
            seed_tree(&mut app);
            seed_feat_worktree(&mut app, "w2", "feat");
            app.sel_worktree = 1;
            assert_eq!(
                app.selected_worktree().map(|w| w.branch.as_str()),
                Some("feat")
            );
            let mut out = Vec::new();
            let project = nebula_core::ProjectId("p1".into());
            app.overlay = Some(Overlay::Issues(crate::issues::IssuesView::new(
                project.clone(),
                "demo".into(),
                "/tmp/demo".into(),
            )));
            crate::issues::land_answer(
                &mut app,
                crate::issues::IssuesAnswer::List {
                    project,
                    list: Some(vec![crate::issues::Issue {
                        number: 15,
                        url: "https://github.com/o/r/issues/15".into(),
                        title: "Login fails".into(),
                        author: "webdevcody".into(),
                        created_at: "2026-09-10T12:00:00Z".into(),
                        updated_at: "2026-09-11T12:00:00Z".into(),
                        labels: vec![],
                        body: String::new(),
                    }]),
                },
            );
            press(&mut app, KeyCode::Enter, KeyModifiers::NONE, &mut out);
            match &app.overlay {
                Some(Overlay::Prompt(prompt)) => match &prompt.kind {
                    PromptKind::QuickPrompt(launch) => launch.target.clone(),
                    other => panic!("{other:?}"),
                },
                other => panic!("Enter: expected the box, got {other:?}"),
            }
        })
    };
    assert_eq!(
        target("{}"),
        QuickTarget::Worktree(WorktreeId("w1".into())),
        "not the selected card's checkout"
    );
    match target(r#"{"quick_prompt_new_worktree": true}"#) {
        QuickTarget::NewWorktree { branch, .. } => {
            assert!(branch.starts_with("issue-15"), "{branch}")
        }
        other => panic!("the setting cuts a fresh worktree: {other:?}"),
    }
}

/// `Enter` in the ISSUES MODAL puts the QUICK PROMPT up over the modal
/// rather than in its place — both on screen, the box on top. A box
/// that goes without launching (Esc, a click outside it) leaves the
/// modal on its row; the launch closes the modal too, and its Ack puts
/// the grid's cursor on the new session's card, the keys still there.
#[test]
fn the_issue_quick_prompt_stands_on_the_modal() {
    with_config_json(r#"{"follow_new_session": true}"#, || {
        let mut app = App::new();
        seed_tree(&mut app);
        let focus = app.focus;
        let mut out = Vec::new();
        let project = nebula_core::ProjectId("p1".into());
        app.overlay = Some(Overlay::Issues(crate::issues::IssuesView::new(
            project.clone(),
            "demo".into(),
            "/tmp/demo".into(),
        )));
        let issue = |number: u64| crate::issues::Issue {
            number,
            url: format!("https://github.com/o/r/issues/{number}"),
            title: format!("issue {number}"),
            author: "webdevcody".into(),
            created_at: "2026-09-10T12:00:00Z".into(),
            updated_at: "2026-09-11T12:00:00Z".into(),
            labels: vec![],
            body: String::new(),
        };
        crate::issues::land_answer(
            &mut app,
            crate::issues::IssuesAnswer::List {
                project,
                list: Some(vec![issue(15), issue(14)]),
            },
        );
        press(&mut app, KeyCode::Down, KeyModifiers::NONE, &mut out);
        let on_row =
            |app: &App| matches!(&app.overlay, Some(Overlay::Issues(v)) if v.selected == 1);
        let mut terminal = Terminal::new(TestBackend::new(140, 40)).unwrap();

        press(&mut app, KeyCode::Enter, KeyModifiers::NONE, &mut out);
        let Some(Overlay::Prompt(prompt)) = &app.overlay else {
            panic!("Enter: expected the box, got {:?}", app.overlay);
        };
        let PromptKind::QuickPrompt(launch) = &prompt.kind else {
            panic!("{:?}", prompt.kind);
        };
        assert!(
            matches!(&launch.under, Some(crate::quick_prompt::ModalUnder::Issues(v)) if v.selected == 1),
            "the box carries the modal it stands on: {:?}",
            launch.under
        );
        terminal.draw(|f| crate::ui::draw(f, &mut app)).unwrap();
        let screen = buffer_text(&terminal);
        assert!(
            screen.contains("Issues — demo"),
            "the modal under:\n{screen}"
        );
        assert!(screen.contains("issue #14"), "the box over it:\n{screen}");
        assert!(screen.contains("Esc: back to issues"), "{screen}");

        // Esc: the box goes, the modal stays.
        press(&mut app, KeyCode::Esc, KeyModifiers::NONE, &mut out);
        assert!(on_row(&app), "Esc leaves the modal: {:?}", app.overlay);

        // A click outside the box: the same.
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE, &mut out);
        crate::overlay_close::click_outside(&mut app, &mut out);
        assert!(on_row(&app), "a click outside: {:?}", app.overlay);

        // The launch, through the loop's own entry point (so a follow
        // it may not take is caught): the create goes out, and the
        // modal goes with the box.
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE, &mut out);
        type_text(&mut app, "look into it", &mut out);
        out.clear();
        let enter = Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        handle_terminal_event(&mut app, enter, &mut out);
        let (req_id, worktree) = match out.as_slice() {
            [ClientRequest::CreateAgent {
                req_id,
                worktree,
                issue_url: Some(url),
                ..
            }] if url == "https://github.com/o/r/issues/14" => (*req_id, worktree.clone()),
            other => panic!("one create, for the issue: {other:?}"),
        };
        assert!(
            app.overlay.is_none(),
            "the launch closes the modal: {:?}",
            app.overlay
        );

        // The DAEMON's side: the row, then the Ack — and the cursor is
        // on the new card, the keys on the grid.
        let fresh = AgentId("a9".into());
        hse(
            &mut app,
            ServerEvent::EntityUpserted {
                entity: nebula_core::Entity::Agent(nebula_core::Agent {
                    id: fresh.clone(),
                    worktree_id: worktree,
                    name: "agent-2".into(),
                    status: nebula_core::AgentStatus::Fresh,
                    archived: false,
                    archived_at: 0,
                    unseen: false,
                    kind: nebula_core::AgentKind::Claude,
                    custom_harness: None,
                    model: None,
                    effort: None,
                    session_id: None,
                    cloud_session_id: None,
                    sort_order: 0,
                    status_changed_at: crate::app::now_ms(),
                    alive: true,
                    issue_url: None,
                    recent_prompts: Vec::new(),
                }),
            },
        );
        handle_server_event(
            &mut app,
            ServerEvent::Ack {
                req_id,
                created: Some(nebula_core::EntityId::Agent(fresh.clone())),
            },
            &mut out,
        );
        assert!(app.overlay.is_none(), "nothing comes back up");
        assert_eq!(
            app.selected_session().map(|a| a.id),
            Some(fresh),
            "the cursor is on the new card"
        );
        assert_eq!(app.focus, focus, "the keys stay on the grid");
        assert!(!app.term_locked);
    });
}

/// `Shift+Tab` in the ISSUES MODAL puts the AGENT PRESETS picker up over
/// the modal rather than in its place. Esc and a click outside leave the
/// modal on its row; a pick hands the box over, still on the modal, and
/// the box's own Esc goes back to it too.
#[test]
fn the_issue_preset_picker_stands_on_the_modal() {
    with_seeded_presets(|| {
        let mut app = App::new();
        seed_tree(&mut app);
        let mut out = Vec::new();
        let project = nebula_core::ProjectId("p1".into());
        app.overlay = Some(Overlay::Issues(crate::issues::IssuesView::new(
            project.clone(),
            "demo".into(),
            "/tmp/demo".into(),
        )));
        let issue = |number: u64| crate::issues::Issue {
            number,
            url: format!("https://github.com/o/r/issues/{number}"),
            title: format!("issue {number}"),
            author: "webdevcody".into(),
            created_at: "2026-09-10T12:00:00Z".into(),
            updated_at: "2026-09-11T12:00:00Z".into(),
            labels: vec![],
            body: String::new(),
        };
        crate::issues::land_answer(
            &mut app,
            crate::issues::IssuesAnswer::List {
                project,
                list: Some(vec![issue(15), issue(14)]),
            },
        );
        press(&mut app, KeyCode::Down, KeyModifiers::NONE, &mut out);
        let on_row =
            |app: &App| matches!(&app.overlay, Some(Overlay::Issues(v)) if v.selected == 1);
        let mut terminal = Terminal::new(TestBackend::new(140, 40)).unwrap();

        press(&mut app, KeyCode::BackTab, KeyModifiers::SHIFT, &mut out);
        let Some(Overlay::AgentPresets(view)) = &app.overlay else {
            panic!(
                "Shift+Tab: expected the preset picker, got {:?}",
                app.overlay
            );
        };
        let back = view.quick.as_ref().expect("a picker for a launch");
        assert!(
            matches!(&back.launch.under, Some(crate::quick_prompt::ModalUnder::Issues(v)) if v.selected == 1),
            "the picker carries the modal it stands on: {:?}",
            back.launch.under
        );
        terminal.draw(|f| crate::ui::draw(f, &mut app)).unwrap();
        let screen = buffer_text(&terminal);
        assert!(
            screen.contains("Issues — demo"),
            "the modal under:\n{screen}"
        );
        assert!(screen.contains("reviewer"), "the picker over it:\n{screen}");

        // Esc: the picker goes, the modal stays on its row.
        press(&mut app, KeyCode::Esc, KeyModifiers::NONE, &mut out);
        assert!(on_row(&app), "Esc leaves the modal: {:?}", app.overlay);

        // A click outside the picker: the same.
        press(&mut app, KeyCode::BackTab, KeyModifiers::SHIFT, &mut out);
        terminal.draw(|f| crate::ui::draw(f, &mut app)).unwrap();
        crate::overlay_close::click_outside(&mut app, &mut out);
        assert!(on_row(&app), "a click outside: {:?}", app.overlay);

        // A pick: the box, preset applied, still standing on the modal.
        press(&mut app, KeyCode::BackTab, KeyModifiers::SHIFT, &mut out);
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE, &mut out);
        let Some(Overlay::Prompt(prompt)) = &app.overlay else {
            panic!("the pick: expected the box, got {:?}", app.overlay);
        };
        let PromptKind::QuickPrompt(launch) = &prompt.kind else {
            panic!("{:?}", prompt.kind);
        };
        assert_eq!(
            launch.preset.as_ref().map(|p| p.name.as_str()),
            Some("reviewer")
        );
        assert!(
            matches!(&launch.under, Some(crate::quick_prompt::ModalUnder::Issues(v)) if v.selected == 1),
            "{:?}",
            launch.under
        );
        terminal.draw(|f| crate::ui::draw(f, &mut app)).unwrap();
        let screen = buffer_text(&terminal);
        assert!(
            screen.contains("Issues — demo"),
            "the modal under:\n{screen}"
        );
        press(&mut app, KeyCode::Esc, KeyModifiers::NONE, &mut out);
        assert!(on_row(&app), "the box's Esc: {:?}", app.overlay);
    });
}

/// `Tab` on the AGENT PRESETS list `e` opens flips the launch onto a
/// fresh worktree of the project, and back; a click on the list's
/// `[ ] new worktree` row is the same flip (INPUT PARITY). Enter on a
/// preset flipped on puts the QUICK PROMPT up with the preset on it,
/// aimed at the fresh branch, and its Enter cuts the worktree first.
#[test]
fn the_preset_list_tab_flips_a_new_worktree() {
    with_seeded_presets(|| {
        let mut app = App::new();
        let mut out = Vec::new();
        open_presets(&mut app, &mut out);
        let fresh = |app: &App| match &app.overlay {
            Some(Overlay::AgentPresets(v)) => v.is_new_worktree(),
            other => panic!("expected the presets list, got {other:?}"),
        };
        let mut terminal = Terminal::new(TestBackend::new(140, 40)).unwrap();
        terminal.draw(|f| crate::ui::draw(f, &mut app)).unwrap();
        let screen = buffer_text(&terminal);
        assert!(!fresh(&app), "the list starts on the worktree `e` was on");
        assert!(screen.contains("worktree: main"), "{screen}");
        assert!(screen.contains("[ ] new worktree Tab"), "{screen}");

        press(&mut app, KeyCode::Tab, KeyModifiers::NONE, &mut out);
        assert!(fresh(&app), "Tab flips it on");
        terminal.draw(|f| crate::ui::draw(f, &mut app)).unwrap();
        let screen = buffer_text(&terminal);
        assert!(screen.contains("NEW WORKTREE"), "{screen}");
        assert!(screen.contains("[✓] new worktree Tab"), "{screen}");
        press(&mut app, KeyCode::Tab, KeyModifiers::NONE, &mut out);
        assert!(!fresh(&app), "and off again");

        // The click: the same flip.
        let Some(Overlay::AgentPresets(view)) = &app.overlay else {
            unreachable!()
        };
        let row = view.toggle_area;
        click(&mut app, row.x + row.width - 4, row.y, &mut out);
        assert!(fresh(&app), "a click on the row flips it on");
        let Some(Overlay::AgentPresets(view)) = &app.overlay else {
            unreachable!()
        };
        let Some(crate::quick_prompt::QuickTarget::NewWorktree { branch, .. }) = view.aim.clone()
        else {
            panic!("{:?}", view.aim);
        };

        // Enter on "reviewer": the box, preset on, aimed at the branch.
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE, &mut out);
        let Some(Overlay::Prompt(prompt)) = &app.overlay else {
            panic!("expected the quick prompt, got {:?}", app.overlay);
        };
        let PromptKind::QuickPrompt(launch) = &prompt.kind else {
            panic!("{:?}", prompt.kind);
        };
        assert_eq!(
            launch.preset.as_ref().map(|p| p.name.as_str()),
            Some("reviewer")
        );
        assert!(
            matches!(&launch.target, crate::quick_prompt::QuickTarget::NewWorktree { branch: b, .. } if *b == branch),
            "{:?}",
            launch.target
        );
        out.clear();
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE, &mut out);
        assert!(
            out.iter().any(
                |r| matches!(r, ClientRequest::CreateWorktree { branch: b, .. } if *b == branch)
            ),
            "the worktree is cut first: {out:?}"
        );
    });
}

/// `Shift+Tab` in the ISSUES MODAL, then `Tab`: the picker flips the
/// launch onto a fresh worktree named after the issue, and the box its
/// Enter hands over is aimed there — still standing on the modal. Esc
/// before a pick drops the flip with the picker.
#[test]
fn the_issue_preset_picker_tab_flips_a_new_worktree() {
    with_seeded_presets(|| {
        let mut app = App::new();
        seed_tree(&mut app);
        let mut out = Vec::new();
        let project = nebula_core::ProjectId("p1".into());
        app.overlay = Some(Overlay::Issues(crate::issues::IssuesView::new(
            project.clone(),
            "demo".into(),
            "/tmp/demo".into(),
        )));
        crate::issues::land_answer(
            &mut app,
            crate::issues::IssuesAnswer::List {
                project,
                list: Some(vec![crate::issues::Issue {
                    number: 15,
                    url: "https://github.com/o/r/issues/15".into(),
                    title: "Fix login redirect".into(),
                    author: "webdevcody".into(),
                    created_at: "2026-09-10T12:00:00Z".into(),
                    updated_at: "2026-09-11T12:00:00Z".into(),
                    labels: vec![],
                    body: String::new(),
                }]),
            },
        );
        let fresh = |app: &App| match &app.overlay {
            Some(Overlay::AgentPresets(v)) => v.is_new_worktree(),
            other => panic!("expected the preset picker, got {other:?}"),
        };

        press(&mut app, KeyCode::BackTab, KeyModifiers::SHIFT, &mut out);
        assert!(!fresh(&app), "the root worktree, as Enter's box");
        press(&mut app, KeyCode::Tab, KeyModifiers::NONE, &mut out);
        assert!(fresh(&app));
        press(&mut app, KeyCode::Esc, KeyModifiers::NONE, &mut out);
        assert!(matches!(&app.overlay, Some(Overlay::Issues(_))));
        press(&mut app, KeyCode::BackTab, KeyModifiers::SHIFT, &mut out);
        assert!(!fresh(&app), "Esc dropped the flip");

        press(&mut app, KeyCode::Tab, KeyModifiers::NONE, &mut out);
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE, &mut out);
        let Some(Overlay::Prompt(prompt)) = &app.overlay else {
            panic!("the pick: expected the box, got {:?}", app.overlay);
        };
        let PromptKind::QuickPrompt(launch) = &prompt.kind else {
            panic!("{:?}", prompt.kind);
        };
        assert!(
            matches!(&launch.target, crate::quick_prompt::QuickTarget::NewWorktree { branch, .. } if branch.starts_with("issue-15")),
            "{:?}",
            launch.target
        );
        assert_eq!(launch.issue.as_ref().map(|i| i.number), Some(15));
        assert!(matches!(
            &launch.under,
            Some(crate::quick_prompt::ModalUnder::Issues(_))
        ));
    });
}

#[test]
fn presets_a_fills_the_editor_and_enter_persists() {
    use crate::preset_overlays::PresetField;
    with_seeded_presets(|| {
        let mut app = App::new();
        let mut out = Vec::new();
        open_presets(&mut app, &mut out);
        press(
            &mut app,
            KeyCode::Char('a'),
            KeyModifiers::CONTROL,
            &mut out,
        );
        let Some(Overlay::AgentPresetEditor(editor)) = &app.overlay else {
            panic!("a should open the editor, got {:?}", app.overlay);
        };
        assert_eq!(editor.editing, None);
        assert_eq!(editor.field, PresetField::Name);

        // List verbs are just characters while a text field has the
        // caret — `e`, `d`, `a` type rather than act.
        type_text(&mut app, "tested", &mut out);
        press(&mut app, KeyCode::Tab, KeyModifiers::NONE, &mut out);
        press(&mut app, KeyCode::Right, KeyModifiers::NONE, &mut out);
        press(&mut app, KeyCode::Tab, KeyModifiers::NONE, &mut out);
        press(&mut app, KeyCode::Right, KeyModifiers::NONE, &mut out);
        press(&mut app, KeyCode::Tab, KeyModifiers::NONE, &mut out);
        press(&mut app, KeyCode::Right, KeyModifiers::NONE, &mut out);
        // Effort, then the Text row: a new preset starts on `prefix`
        // alone — one box, and no Postfix row for Tab to land on.
        press(&mut app, KeyCode::Tab, KeyModifiers::NONE, &mut out);
        let Some(Overlay::AgentPresetEditor(editor)) = &app.overlay else {
            panic!("editor should stay open, got {:?}", app.overlay);
        };
        assert_eq!(editor.field, PresetField::Text);
        assert_eq!(editor.text, crate::agent_presets::PresetText::Prefix);
        let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
        terminal.draw(|f| ui::draw(f, &mut app)).unwrap();
        let text = buffer_text(&terminal);
        assert!(text.contains("Text  ◂ prefix ▸"), "text row:\n{text}");
        assert!(text.contains("Prefix (optional)"), "one box:\n{text}");
        assert!(
            !text.contains("Postfix (optional)"),
            "no postfix box on `prefix`:\n{text}"
        );
        // → → : postfix, then both sides — and the Postfix row is back.
        press(&mut app, KeyCode::Right, KeyModifiers::NONE, &mut out);
        press(&mut app, KeyCode::Right, KeyModifiers::NONE, &mut out);
        press(&mut app, KeyCode::Tab, KeyModifiers::NONE, &mut out);
        assert!(paste_into_overlay(&mut app, "Line one"));
        press(&mut app, KeyCode::Enter, KeyModifiers::SHIFT, &mut out);
        assert!(paste_into_overlay(&mut app, "Line two"));
        // Option+Enter — the ESC CR a mapped Shift+Enter sends — and
        // Ctrl+J break lines too, as in Claude Code's own prompt; none
        // of the three is the save.
        press(&mut app, KeyCode::Enter, KeyModifiers::ALT, &mut out);
        press(
            &mut app,
            KeyCode::Char('j'),
            KeyModifiers::CONTROL,
            &mut out,
        );
        assert!(paste_into_overlay(&mut app, "Line four"));
        // ↑ walks the box's lines rather than leaving the field …
        press(&mut app, KeyCode::Up, KeyModifiers::NONE, &mut out);
        let Some(Overlay::AgentPresetEditor(editor)) = &app.overlay else {
            panic!("editor should stay open, got {:?}", app.overlay);
        };
        assert_eq!(editor.field, PresetField::Prefix);
        assert_eq!(
            editor.prefix.cursor_chars(),
            "Line one\nLine two\n".len(),
            "↑ from the end of line four lands on the empty third line"
        );
        // … and ↓ past the last line steps to the next field, as Tab does.
        press(&mut app, KeyCode::Down, KeyModifiers::NONE, &mut out);
        press(&mut app, KeyCode::Down, KeyModifiers::NONE, &mut out);
        type_text(&mut app, "END", &mut out);
        let Some(Overlay::AgentPresetEditor(editor)) = &app.overlay else {
            panic!("editor should stay open, got {:?}", app.overlay);
        };
        assert_eq!(editor.name.as_str(), "tested");
        assert_eq!(editor.kind, AgentKind::Codex);
        assert_eq!(editor.model, "gpt-5.6-sol", "→ steps off the default");
        assert_eq!(editor.effort, "minimal");
        assert_eq!(editor.prefix.as_str(), "Line one\nLine two\n\nLine four");
        assert_eq!(editor.field, PresetField::Postfix);

        let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
        terminal.draw(|f| ui::draw(f, &mut app)).unwrap();
        let text = buffer_text(&terminal);
        assert!(text.contains("New preset"), "title:\n{text}");
        assert!(text.contains("Harness  codex"), "kind row:\n{text}");
        assert!(text.contains("Line two"), "prefix box:\n{text}");
        assert!(
            text.contains("◂ END ▸") || text.contains("END"),
            "postfix caret:\n{text}"
        );

        press(&mut app, KeyCode::Enter, KeyModifiers::NONE, &mut out);
        let Some(Overlay::AgentPresets(view)) = &app.overlay else {
            panic!("save should land back in the list, got {:?}", app.overlay);
        };
        assert_eq!(view.presets.len(), 3);
        assert_eq!(view.selected, 2, "cursor on the saved row");
        let saved = &crate::agent_presets::load()[2];
        assert_eq!(saved.name, "tested");
        assert_eq!(saved.kind, AgentKind::Codex);
        assert_eq!(saved.model.as_deref(), Some("gpt-5.6-sol"));
        assert_eq!(saved.effort.as_deref(), Some("minimal"));
        assert_eq!(saved.prefix, "Line one\nLine two\n\nLine four");
        assert_eq!(saved.postfix, "END");
        assert!(
            out.is_empty(),
            "editing sends nothing to the daemon: {out:?}"
        );
    });
}

/// Enter on a form that can't save keeps the form and says why on a
/// banner at the top of it — in the error color, with the Name label
/// and caret painted to match — and brings the caret back to the Name
/// row from wherever it was, so the next keystroke is the fix. That
/// keystroke takes the banner down; the next Enter judges the new
/// text afresh. Nothing goes to the FOOTER, which is nowhere near the
/// form.
#[test]
fn presets_editor_refuses_blank_and_duplicate_names() {
    use crate::preset_overlays::{FormError, PresetField};
    use ratatui::style::Modifier;
    with_seeded_presets(|| {
        let mut app = App::new();
        let mut out = Vec::new();
        open_presets(&mut app, &mut out);
        press(
            &mut app,
            KeyCode::Char('a'),
            KeyModifiers::CONTROL,
            &mut out,
        );
        // Enter from the far end of the form.
        press(&mut app, KeyCode::BackTab, KeyModifiers::NONE, &mut out);
        let Some(Overlay::AgentPresetEditor(editor)) = &app.overlay else {
            panic!("a should open the editor, got {:?}", app.overlay);
        };
        assert_eq!(editor.field, PresetField::Task);
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE, &mut out);
        let Some(Overlay::AgentPresetEditor(editor)) = &app.overlay else {
            panic!("a refused save keeps the form, got {:?}", app.overlay);
        };
        assert_eq!(
            editor.error,
            Some(FormError {
                field: PresetField::Name,
                message: "the preset needs a name".into(),
            })
        );
        assert_eq!(
            editor.field,
            PresetField::Name,
            "the caret comes back to the field to fix"
        );
        assert_eq!(app.flash, None, "the reason is in the form, not the footer");

        let th = app.theme;
        let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
        terminal.draw(|f| ui::draw(f, &mut app)).unwrap();
        let text = buffer_text(&terminal);
        let (bx, by) = find_cell(&terminal, "✗ the preset needs a name");
        let (nx, ny) = find_cell(&terminal, "Name  ");
        assert_eq!(
            ny,
            by + 1,
            "the banner sits right above the Name row:\n{text}"
        );
        let buffer = terminal.backend().buffer();
        let banner = &buffer[(bx, by)];
        assert_eq!(banner.fg, th.err, "the banner in the error color:\n{text}");
        assert!(
            banner.modifier.contains(Modifier::BOLD),
            "and bold:\n{text}"
        );
        assert_eq!(
            buffer[(nx, ny)].fg,
            th.err,
            "the Name label with it:\n{text}"
        );
        // The caret block sits right after the two-space gap.
        let caret = &buffer[(nx + 6, ny)];
        assert_eq!(caret.bg, th.err, "and the caret:\n{text}");

        // The first keystroke into the name is the fix: the banner
        // goes, the label is back in the focus color, the form keeps
        // its place.
        type_text(&mut app, "R", &mut out);
        let Some(Overlay::AgentPresetEditor(editor)) = &app.overlay else {
            panic!("typing keeps the form, got {:?}", app.overlay);
        };
        assert_eq!(editor.error, None);
        assert_eq!(editor.name.as_str(), "R");
        terminal.draw(|f| ui::draw(f, &mut app)).unwrap();
        let text = buffer_text(&terminal);
        assert!(
            !text.contains("✗ the preset"),
            "the banner is gone:\n{text}"
        );
        let (nx, ny) = find_cell(&terminal, "Name  ");
        assert_eq!(
            terminal.backend().buffer()[(nx, ny)].fg,
            th.accent,
            "{text}"
        );

        type_text(&mut app, "eviewer", &mut out);
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE, &mut out);
        let Some(Overlay::AgentPresetEditor(editor)) = &app.overlay else {
            panic!("a duplicate keeps the form, got {:?}", app.overlay);
        };
        let error = editor.error.clone().expect("case-insensitive duplicate");
        assert_eq!(error.field, PresetField::Name);
        assert!(error.message.contains("already exists"), "{error:?}");
        terminal.draw(|f| ui::draw(f, &mut app)).unwrap();
        let text = buffer_text(&terminal);
        assert!(
            text.contains("✗ a preset named 'Reviewer' already exists"),
            "{text}"
        );

        // Tabbing away leaves the banner up — the name is still the
        // problem; the first change to it takes the banner down.
        press(&mut app, KeyCode::Tab, KeyModifiers::NONE, &mut out);
        let Some(Overlay::AgentPresetEditor(editor)) = &app.overlay else {
            panic!("Tab keeps the form, got {:?}", app.overlay);
        };
        assert_eq!(editor.field, PresetField::Kind);
        assert_eq!(editor.error.as_ref(), Some(&error));
        press(&mut app, KeyCode::BackTab, KeyModifiers::NONE, &mut out);
        press(&mut app, KeyCode::Backspace, KeyModifiers::NONE, &mut out);
        let Some(Overlay::AgentPresetEditor(editor)) = &app.overlay else {
            panic!("Backspace keeps the form, got {:?}", app.overlay);
        };
        assert_eq!(editor.error, None);
        assert_eq!(editor.name.as_str(), "Reviewe");

        press(&mut app, KeyCode::Esc, KeyModifiers::NONE, &mut out);
        let Some(Overlay::AgentPresets(view)) = &app.overlay else {
            panic!("Esc goes back to the list, got {:?}", app.overlay);
        };
        assert_eq!(view.presets.len(), 2, "nothing saved");
        assert_eq!(crate::agent_presets::load().len(), 2);
    });
}

/// Letters in the AGENT PRESETS list type ahead over the names, as in
/// the MODEL / EFFORT submenus: the rows narrow to the fuzzy matches,
/// the cursor on the best, the query in the title. The old verb
/// letters type too — `e` and `q` are no edit and no close — and a
/// letter nothing matches is refused, so the list never empties. Esc
/// clears the letters before it closes, leaving the found row
/// selected; Enter, a click and the Ctrl verbs all act on the row the
/// filter left.
#[test]
fn presets_list_types_ahead_to_find_a_preset_by_name() {
    with_seeded_presets(|| {
        let mut app = App::new();
        let mut out = Vec::new();
        open_presets(&mut app, &mut out);
        fn list(app: &App) -> crate::preset_overlays::AgentPresetsView {
            match &app.overlay {
                Some(Overlay::AgentPresets(view)) => view.clone(),
                other => panic!("expected the presets list, got {other:?}"),
            }
        }
        let rows = |app: &App| {
            let view = list(app);
            view.visible()
                .iter()
                .map(|(i, _)| view.presets[*i].name.clone())
                .collect::<Vec<_>>()
        };

        // `e` — the old edit verb — narrows to "reviewer" instead.
        type_text(&mut app, "e", &mut out);
        assert_eq!(rows(&app), ["reviewer"]);
        assert_eq!(list(&app).filter, "e");

        // `q` matches nothing: refused, the list still up.
        type_text(&mut app, "q", &mut out);
        assert_eq!(list(&app).filter, "e", "a dead letter is refused");
        assert_eq!(app.flash.as_deref(), Some("no preset matches 'eq'"));

        // Backspace widens; "scr" finds the second row and the cursor
        // follows it.
        press(&mut app, KeyCode::Backspace, KeyModifiers::NONE, &mut out);
        assert_eq!(rows(&app), ["reviewer", "scratch"]);
        type_text(&mut app, "scr", &mut out);
        assert_eq!(rows(&app), ["scratch"]);
        assert_eq!(list(&app).selected, 1, "the cursor is on the match");

        let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
        terminal.draw(|f| ui::draw(f, &mut app)).unwrap();
        let text = buffer_text(&terminal);
        assert!(
            text.contains("Agent presets ⌕ scr"),
            "query in the title:\n{text}"
        );
        assert!(!text.contains("reviewer"), "filtered out:\n{text}");
        assert!(text.contains("Esc: clear"), "Esc clears first:\n{text}");

        // The first Esc clears the letters and keeps the found row …
        press(&mut app, KeyCode::Esc, KeyModifiers::NONE, &mut out);
        assert_eq!(list(&app).filter, "");
        assert_eq!(rows(&app), ["reviewer", "scratch"]);
        assert_eq!(list(&app).selected, 1, "still on the preset found");
        // … the second closes.
        press(&mut app, KeyCode::Esc, KeyModifiers::NONE, &mut out);
        assert!(app.overlay.is_none());

        // Ctrl+e edits the row the filter left, not the stored first.
        open_presets(&mut app, &mut out);
        type_text(&mut app, "scr", &mut out);
        press(
            &mut app,
            KeyCode::Char('e'),
            KeyModifiers::CONTROL,
            &mut out,
        );
        assert!(
            matches!(&app.overlay, Some(Overlay::AgentPresetEditor(e)) if e.editing == Some(1)),
            "{:?}",
            app.overlay
        );
        press(&mut app, KeyCode::Esc, KeyModifiers::NONE, &mut out);

        // Enter launches the filtered row: "scratch" asks for its task.
        type_text(&mut app, "scr", &mut out);
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE, &mut out);
        assert!(
            matches!(
                &app.overlay,
                Some(Overlay::Prompt(p))
                    if matches!(&p.kind, PromptKind::AgentPresetTask { preset, .. } if preset.name == "scratch")
            ),
            "{:?}",
            app.overlay
        );

        // A click on the filtered list's first row is that same row.
        app.overlay = None;
        open_presets(&mut app, &mut out);
        type_text(&mut app, "scr", &mut out);
        terminal.draw(|f| ui::draw(f, &mut app)).unwrap();
        let area = list(&app).list_area;
        click(&mut app, area.x + 1, area.y, &mut out);
        assert!(
            matches!(
                &app.overlay,
                Some(Overlay::Prompt(p))
                    if matches!(&p.kind, PromptKind::AgentPresetTask { preset, .. } if preset.name == "scratch")
            ),
            "{:?}",
            app.overlay
        );
    });
}

#[test]
fn presets_e_edits_in_place() {
    with_seeded_presets(|| {
        let mut app = App::new();
        let mut out = Vec::new();
        open_presets(&mut app, &mut out);
        press(&mut app, KeyCode::Down, KeyModifiers::NONE, &mut out);
        press(
            &mut app,
            KeyCode::Char('e'),
            KeyModifiers::CONTROL,
            &mut out,
        );
        let Some(Overlay::AgentPresetEditor(editor)) = &app.overlay else {
            panic!("e should open the editor on the row, got {:?}", app.overlay);
        };
        assert_eq!(editor.editing, Some(1));
        assert_eq!(editor.name.as_str(), "scratch");
        assert_eq!(editor.kind, AgentKind::Codex);
        assert_eq!(editor.model, "gpt-5.5");
        assert_eq!(editor.effort, crate::config::DEFAULT_CHOICE);

        // Tab to Model; the last choice wraps to the default.
        press(&mut app, KeyCode::Tab, KeyModifiers::NONE, &mut out);
        press(&mut app, KeyCode::Tab, KeyModifiers::NONE, &mut out);
        press(&mut app, KeyCode::Right, KeyModifiers::NONE, &mut out);
        let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
        terminal.draw(|f| ui::draw(f, &mut app)).unwrap();
        let text = buffer_text(&terminal);
        assert!(text.contains("Edit preset — scratch"), "title:\n{text}");
        assert!(text.contains("◂ default ▸"), "focused choice:\n{text}");

        press(&mut app, KeyCode::Enter, KeyModifiers::NONE, &mut out);
        let Some(Overlay::AgentPresets(view)) = &app.overlay else {
            panic!("save should land back in the list, got {:?}", app.overlay);
        };
        assert_eq!(view.presets.len(), 2, "edited in place, not appended");
        assert_eq!(view.selected, 1);
        let saved = crate::agent_presets::load();
        assert_eq!(saved[1].name, "scratch");
        assert_eq!(saved[1].model, None, "default folds to None");
        assert_eq!(saved[0].name, "reviewer", "order kept");
    });
}

#[test]
fn presets_d_confirms_deletes_and_reopens_and_cancel_keeps() {
    with_seeded_presets(|| {
        let mut app = App::new();
        let mut out = Vec::new();
        open_presets(&mut app, &mut out);
        press(
            &mut app,
            KeyCode::Char('d'),
            KeyModifiers::CONTROL,
            &mut out,
        );
        match &app.overlay {
            Some(Overlay::Confirm(c)) => {
                assert!(c.message.contains("'reviewer'"), "{}", c.message);
                assert!(matches!(
                    c.action,
                    PendingAction::DeleteAgentPreset { index: 0, .. }
                ));
            }
            other => panic!("d should ask first, got {other:?}"),
        }
        // Backing out puts the list back, untouched.
        press(&mut app, KeyCode::Esc, KeyModifiers::NONE, &mut out);
        match &app.overlay {
            Some(Overlay::AgentPresets(view)) => assert_eq!(view.presets.len(), 2),
            other => panic!("cancel should reopen the list, got {other:?}"),
        }
        press(
            &mut app,
            KeyCode::Char('d'),
            KeyModifiers::CONTROL,
            &mut out,
        );
        press(&mut app, KeyCode::Char('y'), KeyModifiers::NONE, &mut out);
        match &app.overlay {
            Some(Overlay::AgentPresets(view)) => {
                assert_eq!(view.presets.len(), 1, "row dropped");
                assert_eq!(view.presets[0].name, "scratch");
                assert_eq!(view.selected, 0, "cursor clamped");
            }
            other => panic!("delete should reopen the list, got {other:?}"),
        }
        assert_eq!(app.flash.as_deref(), Some("deleted preset 'reviewer'"));
        let left = crate::agent_presets::load();
        assert_eq!(left.len(), 1, "removal reached the store");
        assert_eq!(left[0].name, "scratch");
        assert!(out.is_empty(), "{out:?}");
    });
}

#[test]
fn preset_enter_asks_for_a_task_and_launches_with_the_composed_prompt() {
    with_seeded_presets(|| {
        let mut app = App::new();
        let mut out = Vec::new();
        open_presets(&mut app, &mut out);
        let worktree = app.selected_worktree().unwrap().id.clone();
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE, &mut out);
        let Some(Overlay::Prompt(prompt)) = &app.overlay else {
            panic!("Enter should ask for the task, got {:?}", app.overlay);
        };
        assert_eq!(prompt.title, "Task for reviewer");
        assert!(
            prompt.label.contains("claude · opus · high"),
            "{}",
            prompt.label
        );
        assert!(
            prompt.label.contains("prefix + your task + postfix"),
            "{}",
            prompt.label
        );
        assert!(prompt.is_multiline());
        assert!(out.is_empty(), "no prewarm for a preset launch: {out:?}");

        assert!(paste_into_overlay(&mut app, "Fix auth"));
        press(&mut app, KeyCode::Enter, KeyModifiers::SHIFT, &mut out);
        assert!(paste_into_overlay(&mut app, "Ship it"));
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE, &mut out);
        assert!(app.overlay.is_none(), "launch closes the prompt");
        assert!(
            matches!(
                out.as_slice(),
                [ClientRequest::CreateAgent {
                    worktree: w,
                    kind: AgentKind::Claude,
                    custom_harness: None,
                    model: Some(model),
                    effort: Some(effort),
                    auto_title: true,
                    cloud_prompt: None,
                    starting_prompt: Some(text),
                    ..
                }] if *w == worktree
                    && model == "opus"
                    && effort == "high"
                    && text == "Be strict.\n\nFix auth\nShip it\n\nRun the tests."
            ),
            "one create with the sandwiched prompt, no warm-slot refill: {out:?}"
        );

        // The daemon refusing it brings the task back to be fixed.
        let req_id = match &out[0] {
            ClientRequest::CreateAgent { req_id, .. } => *req_id,
            other => panic!("expected create request, got {other:?}"),
        };
        handle_server_event(
            &mut app,
            ServerEvent::Error {
                req_id: Some(req_id),
                message: "claude is not installed".into(),
            },
            &mut out,
        );
        assert_eq!(app.flash.as_deref(), Some("claude is not installed"));
        assert!(matches!(
            &app.overlay,
            Some(Overlay::Prompt(prompt))
                if matches!(&prompt.kind, PromptKind::AgentPresetTask { preset, .. } if preset.name == "reviewer")
                    && prompt.input.as_str() == "Fix auth\nShip it"
        ));
    });
}

/// The task is optional: Esc on the box goes back to the list on the
/// same row, and an empty Enter launches — prefix + postfix alone for a
/// wrapping preset, no STARTING PROMPT at all for a bare one.
#[test]
fn preset_task_is_optional_and_esc_returns_to_the_list() {
    with_seeded_presets(|| {
        let mut app = App::new();
        let mut out = Vec::new();
        open_presets(&mut app, &mut out);
        let worktree = app.selected_worktree().unwrap().id.clone();
        press(&mut app, KeyCode::Down, KeyModifiers::NONE, &mut out);
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE, &mut out);
        let Some(Overlay::Prompt(prompt)) = &app.overlay else {
            panic!("Enter should ask for the task, got {:?}", app.overlay);
        };
        assert_eq!(prompt.title, "Task for scratch");
        assert!(
            prompt
                .label
                .contains("sent as the first prompt (empty = start with no prompt)"),
            "{}",
            prompt.label
        );

        press(&mut app, KeyCode::Esc, KeyModifiers::NONE, &mut out);
        match &app.overlay {
            Some(Overlay::AgentPresets(view)) => assert_eq!(view.selected, 1, "same row"),
            other => panic!("Esc goes back to the list, got {other:?}"),
        }
        assert!(out.is_empty(), "{out:?}");

        // The bare preset, sent empty: the CLI starts with no first prompt.
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE, &mut out);
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE, &mut out);
        assert!(
            app.overlay.is_none(),
            "an empty task launches: {:?}",
            app.overlay
        );
        assert!(
            matches!(
                out.as_slice(),
                [ClientRequest::CreateAgent {
                    worktree: w,
                    kind: AgentKind::Codex,
                    custom_harness: None,
                    starting_prompt: None,
                    ..
                }] if *w == worktree
            ),
            "one create with no first prompt: {out:?}"
        );

        // The wrapping preset, sent empty: prefix + postfix alone.
        out.clear();
        crate::preset_overlays::reopen_agent_presets(&mut app, worktree, 0);
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE, &mut out);
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE, &mut out);
        assert!(
            matches!(
                out.as_slice(),
                [ClientRequest::CreateAgent {
                    kind: AgentKind::Claude,
                    custom_harness: None,
                    starting_prompt: Some(text),
                    ..
                }] if text == "Be strict.\n\nRun the tests."
            ),
            "one create on the wrapping alone: {out:?}"
        );
    });
}

/// A `skip_task` preset asks for nothing: the editor's Task row turns
/// it on, the list marks the row, and Enter on it launches straight
/// away on prefix + postfix — no task box in between.
#[test]
fn a_skip_task_preset_launches_from_the_list_without_asking() {
    use crate::preset_overlays::PresetField;
    with_seeded_presets(|| {
        let mut app = App::new();
        let mut out = Vec::new();
        open_presets(&mut app, &mut out);
        let worktree = app.selected_worktree().unwrap().id.clone();

        // `e` on "reviewer"; Task is the last field, so ⇧Tab from Name
        // wraps onto it.
        press(
            &mut app,
            KeyCode::Char('e'),
            KeyModifiers::CONTROL,
            &mut out,
        );
        press(&mut app, KeyCode::BackTab, KeyModifiers::SHIFT, &mut out);
        let Some(Overlay::AgentPresetEditor(editor)) = &app.overlay else {
            panic!("e should open the editor, got {:?}", app.overlay);
        };
        assert_eq!(editor.field, PresetField::Task);
        assert!(!editor.skip_task, "presets ask by default");
        press(&mut app, KeyCode::Right, KeyModifiers::NONE, &mut out);
        let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
        terminal.draw(|f| ui::draw(f, &mut app)).unwrap();
        let text = buffer_text(&terminal);
        assert!(text.contains("◂ skip ▸"), "Task row:\n{text}");
        assert!(
            text.contains("Enter launches at once"),
            "Task hint:\n{text}"
        );
        assert!(
            text.contains("Run the tests."),
            "boxes still drawn:\n{text}"
        );

        press(&mut app, KeyCode::Enter, KeyModifiers::NONE, &mut out);
        assert!(crate::agent_presets::load()[0].skip_task, "saved");
        terminal.draw(|f| ui::draw(f, &mut app)).unwrap();
        let text = buffer_text(&terminal);
        assert!(text.contains("no task"), "list mark:\n{text}");
        assert!(out.is_empty(), "editing sends nothing: {out:?}");

        press(&mut app, KeyCode::Enter, KeyModifiers::NONE, &mut out);
        assert!(app.overlay.is_none(), "no task box: {:?}", app.overlay);
        assert!(
            matches!(
                out.as_slice(),
                [ClientRequest::CreateAgent {
                    worktree: w,
                    kind: AgentKind::Claude,
                    custom_harness: None,
                    model: Some(model),
                    starting_prompt: Some(text),
                    ..
                }] if *w == worktree
                    && model == "opus"
                    && text == "Be strict.\n\nRun the tests."
            ),
            "one create on prefix + postfix: {out:?}"
        );

        // A refused create hands back the (empty) task box rather than
        // losing the launch without a word.
        let req_id = match &out[0] {
            ClientRequest::CreateAgent { req_id, .. } => *req_id,
            other => panic!("expected create request, got {other:?}"),
        };
        handle_server_event(
            &mut app,
            ServerEvent::Error {
                req_id: Some(req_id),
                message: "claude is not installed".into(),
            },
            &mut out,
        );
        assert_eq!(app.flash.as_deref(), Some("claude is not installed"));
        assert!(matches!(
            &app.overlay,
            Some(Overlay::Prompt(prompt))
                if matches!(&prompt.kind, PromptKind::AgentPresetTask { preset, .. } if preset.skip_task)
        ));
    });
}

/// The QUICK PROMPT: `p` anywhere in the panels, a multi-row box, and
/// Enter starts an agent of the configured harness on what was typed —
/// no picker on the way.
#[test]
fn p_opens_the_quick_prompt_and_launches_on_what_was_typed() {
    with_config_json(
        r#"{"quick_prompt_kind": "codex", "codex_model": "gpt-5.5", "codex_effort": "high"}"#,
        || {
            let mut app = App::new();
            let mut out = Vec::new();
            seed_tree(&mut app);
            // Deliberately not the Sessions panel: unlike the presets
            // list, the quick prompt answers from wherever you are.
            app.focus = Focus::Projects;
            let worktree = app.selected_worktree().unwrap().id.clone();

            press(&mut app, KeyCode::Char('p'), KeyModifiers::NONE, &mut out);
            let Some(Overlay::Prompt(prompt)) = &app.overlay else {
                panic!("p should open the quick prompt, got {:?}", app.overlay);
            };
            assert_eq!(prompt.title, "Quick prompt (codex · gpt-5.5 · high)");
            assert!(prompt.is_multiline(), "a task box, not a one-line name");
            assert!(out.is_empty(), "opening it sends nothing: {out:?}");

            // Shift+Enter keeps typing; only a bare Enter launches.
            assert!(paste_into_overlay(&mut app, "Fix auth"));
            press(&mut app, KeyCode::Enter, KeyModifiers::SHIFT, &mut out);
            assert!(paste_into_overlay(&mut app, "then ship it"));
            assert!(
                matches!(&app.overlay, Some(Overlay::Prompt(_))),
                "Shift+Enter stays in the box"
            );

            press(&mut app, KeyCode::Enter, KeyModifiers::NONE, &mut out);
            assert!(app.overlay.is_none(), "launching closes the box");
            assert!(
                matches!(
                    out.as_slice(),
                    [ClientRequest::CreateAgent {
                        worktree: w,
                        kind: AgentKind::Codex,
                        custom_harness: None,
                        model: Some(model),
                        effort: Some(effort),
                        auto_title: true,
                        cloud_prompt: None,
                        starting_prompt: Some(text),
                        ..
                    }] if *w == worktree
                        && model == "gpt-5.5"
                        && effort == "high"
                        && text == "Fix auth\nthen ship it"
                ),
                "one create carrying the typed prompt: {out:?}"
            );

            // A refused create brings the text back rather than losing it.
            let req_id = match &out[0] {
                ClientRequest::CreateAgent { req_id, .. } => *req_id,
                other => panic!("expected create request, got {other:?}"),
            };
            handle_server_event(
                &mut app,
                ServerEvent::Error {
                    req_id: Some(req_id),
                    message: "codex is not installed".into(),
                },
                &mut out,
            );
            assert_eq!(app.flash.as_deref(), Some("codex is not installed"));
            assert!(matches!(
                &app.overlay,
                Some(Overlay::Prompt(prompt))
                    if matches!(prompt.kind, PromptKind::QuickPrompt { .. })
                        && prompt.input.as_str() == "Fix auth\nthen ship it"
            ));
        },
    );
}

/// A second, linked checkout beside the seeded ROOT WORKTREE.
pub(super) fn seed_feat_worktree(app: &mut App, id: &str, branch: &str) {
    use nebula_core::{Entity, ProjectId, Worktree, WorktreeId};
    hse(
        app,
        ServerEvent::EntityUpserted {
            entity: Entity::Worktree(Worktree {
                id: WorktreeId(id.into()),
                project_id: ProjectId("p1".into()),
                path: format!("/tmp/demo-worktrees/{branch}").into(),
                branch: branch.into(),
                is_main: false,
                sort_order: 1,
            }),
        },
    );
}

pub(super) fn worktree_branches(app: &App) -> Vec<String> {
    app.visible_worktrees()
        .iter()
        .map(|w| w.branch.clone())
        .collect()
}

/// **Run command** on the PROJECT TAB: Enter opens a prompt titled with
/// the project, pre-filled with its stored command; Enter there writes
/// `run_command` into that project's entry (and nothing else), the
/// row reads it back, the tab on another project still reads
/// `.nebula.json`; ←/→ only explain themselves; Esc keeps the old
/// value; an empty Enter puts the file back and drops the key. With
/// no project in the tree the row reads `n/a`, and Enter opens nothing
/// and says why.
#[test]
fn the_project_tab_run_command_is_typed_into_the_selected_projects_entry() {
    use crate::config::SettingKind::RunCommand;
    use nebula_core::{Entity, Project, ProjectId};
    let draw_to_string = |app: &mut App, w: u16, h: u16| {
        let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
        terminal.draw(|f| ui::draw(f, app)).unwrap();
        buffer_text(&terminal)
    };
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.json");
    let saved = |path: &std::path::Path| -> serde_json::Value {
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
    };
    crate::config::with_config_path(path.clone(), || {
        let mut app = App::new();
        let mut out = Vec::new();
        seed_tree(&mut app);
        hse(
            &mut app,
            ServerEvent::EntityUpserted {
                entity: Entity::Project(Project {
                    id: ProjectId("p2".into()),
                    name: "other".into(),
                    repo_path: "/tmp/other".into(),
                    sort_order: 1,
                }),
            },
        );
        app.sel_project = 0;
        let tab = crate::config::project_tab();
        let (_, row) = crate::config::locate(RunCommand).unwrap();
        let open_on_row = |app: &mut App, out: &mut Vec<ClientRequest>| {
            press(app, KeyCode::Char('s'), KeyModifiers::NONE, out);
            let digit = char::from_digit(tab as u32 + 1, 10).unwrap();
            press(app, KeyCode::Char(digit), KeyModifiers::NONE, out);
            // Wherever the jump left the cursor — the strip, or the
            // row the overlay remembered — climb to the strip and
            // walk down to the row.
            for _ in 0..crate::config::tab_len(tab) {
                press(app, KeyCode::Up, KeyModifiers::NONE, out);
            }
            for _ in 0..=row {
                press(app, KeyCode::Down, KeyModifiers::NONE, out);
            }
            let view = settings(app).expect("settings open");
            assert_eq!((view.tab, view.selected, view.on_tabs), (tab, row, false));
        };

        open_on_row(&mut app, &mut out);
        let screen = draw_to_string(&mut app, 100, 40);
        assert!(screen.contains("Run command"), "{screen}");
        assert!(
            screen.contains("[.nebula.json]"),
            "unset reads as the file: {screen}"
        );

        // ←/→: nothing to cycle, the overlay stays and explains.
        press(&mut app, KeyCode::Char('l'), KeyModifiers::NONE, &mut out);
        let view = settings_view(&app);
        assert_eq!(view.selected, row);
        assert!(
            matches!(&view.notice, Some((t, _)) if t.contains("Enter")),
            "{:?}",
            view.notice
        );

        // Enter: a prompt in the overlay's place, named for the project.
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE, &mut out);
        let Some(Overlay::Prompt(prompt)) = &app.overlay else {
            panic!("expected the run command prompt, got {:?}", app.overlay);
        };
        assert_eq!(prompt.title, "Run command · demo");
        assert_eq!(prompt.input.as_str(), "");
        assert!(matches!(
            &prompt.kind,
            PromptKind::SettingText { kind: RunCommand, project: Some(p) }
                if p == std::path::Path::new("/tmp/demo")
        ));
        type_text(&mut app, "npm run dev", &mut out);
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE, &mut out);
        assert!(out.is_empty(), "a setting is a file write, not a request");
        assert_eq!(
            saved(&path)["projects"],
            serde_json::json!({
                "/tmp/demo": { "run_command": "npm run dev" }
            })
        );
        let view = settings_view(&app);
        assert_eq!((view.tab, view.selected, view.on_tabs), (tab, row, false));
        assert!(
            matches!(&view.notice, Some((t, _)) if t.contains("npm run dev")),
            "the notice shows the saved value: {:?}",
            view.notice
        );
        let screen = draw_to_string(&mut app, 100, 40);
        assert!(screen.contains("[npm run dev]"), "{screen}");

        // Reopened, the prompt carries the stored value; Esc keeps it.
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE, &mut out);
        let Some(Overlay::Prompt(prompt)) = &app.overlay else {
            panic!("expected the run command prompt, got {:?}", app.overlay);
        };
        assert_eq!(prompt.input.as_str(), "npm run dev");
        type_text(&mut app, " --typo", &mut out);
        press(&mut app, KeyCode::Esc, KeyModifiers::NONE, &mut out);
        assert_eq!(
            crate::config::Config::load()
                .project(std::path::Path::new("/tmp/demo"))
                .run_command,
            "npm run dev"
        );
        let view = settings_view(&app);
        assert_eq!((view.tab, view.selected), (tab, row));
        press(&mut app, KeyCode::Esc, KeyModifiers::NONE, &mut out);

        // The other project reads its own (unset) value.
        app.sel_project = 1;
        open_on_row(&mut app, &mut out);
        let screen = draw_to_string(&mut app, 100, 40);
        assert!(screen.contains("/tmp/other"), "{screen}");
        assert!(screen.contains("[.nebula.json]"), "{screen}");
        press(&mut app, KeyCode::Esc, KeyModifiers::NONE, &mut out);

        // An empty Enter is the way back to the file — the entry goes.
        app.sel_project = 0;
        open_on_row(&mut app, &mut out);
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE, &mut out);
        for _ in 0.."npm run dev".len() {
            press(&mut app, KeyCode::Backspace, KeyModifiers::NONE, &mut out);
        }
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE, &mut out);
        assert_eq!(saved(&path)["projects"], serde_json::json!({}));
        let view = settings_view(&app);
        assert!(
            matches!(&view.notice, Some((t, _)) if t.contains(".nebula.json")),
            "back to the file: {:?}",
            view.notice
        );
        press(&mut app, KeyCode::Esc, KeyModifiers::NONE, &mut out);

        // No project at all: no prompt, and the row says why.
        let mut app = App::new();
        open_on_row(&mut app, &mut out);
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE, &mut out);
        let view = settings(&app).unwrap();
        assert!(
            matches!(&view.notice, Some((text, _)) if text.contains("no project selected")),
            "{:?}",
            view.notice
        );
        let screen = draw_to_string(&mut app, 100, 40);
        assert!(screen.contains("[n/a]"), "{screen}");
        assert_eq!(saved(&path)["projects"], serde_json::json!({}), "untouched");
    });
}

/// `p` on the WORKTREES PANEL is "a fresh worktree, then this task in
/// it", whatever checkout the cursor is on — here the root itself: Enter
/// asks the DAEMON for the checkout (no base — it fetches `origin/HEAD`
/// itself), the Ack moves the cursor onto the new row and fires the
/// create there with the typed prompt, and FOCUS stays on the panel
/// `p` was pressed in. A refused worktree brings the box back with the
/// text.
#[test]
fn p_on_the_worktrees_panel_cuts_a_fresh_worktree_first() {
    use crate::quick_prompt::QuickTarget;
    use nebula_core::{EntityId, ProjectId, WorktreeId};
    let json = r#"{"quick_prompt_new_worktree": true, "follow_new_session": true}"#;
    with_config_json(json, || {
        let mut app = App::new();
        let mut out = Vec::new();
        seed_tree(&mut app);
        seed_feat_worktree(&mut app, "w2", "feat");
        app.focus = Focus::Worktrees;
        app.sel_worktree = 0;
        assert_eq!(
            app.selected_worktree().map(|w| w.branch.as_str()),
            Some("main"),
            "the cursor is on the root row"
        );

        press(&mut app, KeyCode::Char('p'), KeyModifiers::NONE, &mut out);
        let branch = match &app.overlay {
            Some(Overlay::Prompt(prompt)) => match &prompt.kind {
                PromptKind::QuickPrompt(launch) => match &launch.target {
                    QuickTarget::NewWorktree { project, branch } => {
                        assert_eq!(project, &ProjectId("p1".into()));
                        assert_eq!(
                            prompt.title,
                            format!("Quick prompt · new worktree {branch} (claude)")
                        );
                        branch.clone()
                    }
                    other => panic!("expected a new-worktree target, got {other:?}"),
                },
                other => panic!("expected the quick prompt, got {other:?}"),
            },
            other => panic!("p should open the quick prompt, got {other:?}"),
        };
        assert!(
            !["main", "feat"].contains(&branch.as_str()),
            "{branch} is taken"
        );
        assert!(out.is_empty(), "opening it sends nothing: {out:?}");

        // The Tab picker round trip hands the new-worktree target back
        // untouched: its menu is built against the ROOT WORKTREE, and
        // the launch must not be rebuilt from that.
        press(&mut app, KeyCode::Tab, KeyModifiers::NONE, &mut out);
        assert!(
            matches!(&app.overlay, Some(Overlay::Menu(menu))
                    if menu.title.as_deref() == Some("Quick prompt agent")),
            "{:?}",
            app.overlay
        );
        press(&mut app, KeyCode::Down, KeyModifiers::NONE, &mut out);
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE, &mut out);
        match &app.overlay {
            Some(Overlay::Prompt(prompt)) => {
                assert_eq!(
                    prompt.title,
                    format!("Quick prompt · new worktree {branch} (codex)")
                );
                assert!(matches!(
                    &prompt.kind,
                    PromptKind::QuickPrompt(launch)
                        if matches!(&launch.target, QuickTarget::NewWorktree { branch: b, .. } if b == &branch)
                ));
            }
            other => panic!("the pick should hand the box back, got {other:?}"),
        }

        assert!(paste_into_overlay(&mut app, "Fix auth"));
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE, &mut out);
        assert!(app.overlay.is_none(), "launching closes the box");
        let req_id = match out.as_slice() {
            [ClientRequest::CreateWorktree {
                req_id,
                project,
                branch: b,
                base: None,
            }] if project == &ProjectId("p1".into()) && b == &branch => *req_id,
            other => panic!("one base-less CreateWorktree first: {other:?}"),
        };
        out.clear();

        // The daemon: the row's upsert, then the Ack naming it.
        seed_feat_worktree(&mut app, "w3", &branch);
        handle_server_event(
            &mut app,
            ServerEvent::Ack {
                req_id,
                created: Some(EntityId::Worktree(WorktreeId("w3".into()))),
            },
            &mut out,
        );
        assert_eq!(
            app.selected_worktree().map(|w| w.id.0.as_str()),
            Some("w3"),
            "the cursor is on the new row"
        );
        assert_eq!(
            app.focus,
            Focus::Worktrees,
            "focus stays where p was pressed"
        );
        assert!(
            matches!(
                out.as_slice(),
                [ClientRequest::CreateAgent {
                    worktree,
                    kind: AgentKind::Codex,
                    custom_harness: None,
                    auto_title: true,
                    cloud_prompt: None,
                    starting_prompt: Some(text),
                    ..
                }] if worktree.0 == "w3" && text == "Fix auth"
            ),
            "then one create in it carrying the typed prompt: {out:?}"
        );
        out.clear();

        // A worktree the daemon refuses (a branch that exists, a fetch
        // that failed) hands the text back for a retry.
        press(&mut app, KeyCode::Char('p'), KeyModifiers::NONE, &mut out);
        assert!(paste_into_overlay(&mut app, "Try again"));
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE, &mut out);
        let req_id = match out.as_slice() {
            [ClientRequest::CreateWorktree { req_id, .. }] => *req_id,
            other => panic!("expected a CreateWorktree, got {other:?}"),
        };
        handle_server_event(
            &mut app,
            ServerEvent::Error {
                req_id: Some(req_id),
                message: "branch exists".into(),
            },
            &mut out,
        );
        assert_eq!(app.flash.as_deref(), Some("branch exists"));
        assert!(matches!(
            &app.overlay,
            Some(Overlay::Prompt(prompt))
                if matches!(
                    &prompt.kind,
                    PromptKind::QuickPrompt(launch)
                        if matches!(launch.target, QuickTarget::NewWorktree { .. })
                ) && prompt.input.as_str() == "Try again"
        ));
    });
}

/// A QUICK PROMPT is fire-and-forget: its Ack selects the new SESSION's
/// row (so the pane previews it and it is marked seen) but leaves FOCUS
/// on the panel the prompt was fired from. `quick_prompt_focus` puts the
/// old behaviour — enter the pane and lock it — back.
#[test]
fn a_quick_prompt_lands_the_row_without_taking_the_pane() {
    use nebula_core::{Agent, AgentStatus, Entity, WorktreeId};

    fn launch(app: &mut App) {
        let mut out = Vec::new();
        seed_tree(app);
        app.focus = Focus::Sessions;
        press(app, KeyCode::Char('p'), KeyModifiers::NONE, &mut out);
        assert!(paste_into_overlay(app, "Fix auth"));
        press(app, KeyCode::Enter, KeyModifiers::NONE, &mut out);
        let Some(ClientRequest::CreateAgent { req_id, .. }) = out.last() else {
            panic!("expected CreateAgent, got {:?}", out.last());
        };
        let req_id = *req_id;
        // The daemon broadcasts the new row before it acks the create.
        hse(
            app,
            ServerEvent::EntityUpserted {
                entity: Entity::Agent(Agent {
                    id: AgentId("a2".into()),
                    worktree_id: WorktreeId("w1".into()),
                    name: "agent-2".into(),
                    status: AgentStatus::Fresh,
                    archived: false,
                    archived_at: 0,
                    unseen: false,
                    kind: nebula_core::AgentKind::Codex,
                    custom_harness: None,
                    model: None,
                    effort: None,
                    session_id: None,
                    cloud_session_id: None,
                    sort_order: 1,
                    status_changed_at: 0,
                    alive: true,
                    issue_url: None,
                    recent_prompts: Vec::new(),
                }),
            },
        );
        hse(
            app,
            ServerEvent::Ack {
                req_id,
                created: Some(EntityId::Agent(AgentId("a2".into()))),
            },
        );
    }

    with_config_json(
        r#"{"quick_prompt_kind": "codex", "follow_new_session": true}"#,
        || {
            let mut app = App::new();
            launch(&mut app);
            assert_eq!(
                app.term.as_ref().map(|t| t.sref.clone()),
                Some(SessionRef::Agent(AgentId("a2".into()))),
                "the pane still previews what was just launched"
            );
            assert_eq!(
                app.selected_session().map(|a| a.id),
                Some(AgentId("a2".into())),
                "and the cursor lands on its row"
            );
            assert_eq!(app.focus, Focus::Sessions, "but focus stays put");
            assert!(!app.term_locked, "and the pane is not locked");
        },
    );

    with_config_json(
        r#"{"quick_prompt_kind": "codex", "quick_prompt_focus": true}"#,
        || {
            let mut app = App::new();
            launch(&mut app);
            assert_eq!(
                app.focus,
                Focus::Terminal,
                "opted in: straight into the pane"
            );
            assert!(app.term_locked, "typing goes to the new agent");
        },
    );
}

/// The harness picker is as wide as its title or its rows and no
/// wider: its keys are in the footer bar, not in its bottom border,
/// where `Tab: cloud off  s/?: settings` doubled a six-row list's
/// width with empty space and hovering the Claude row resized it.
#[test]
fn the_harness_picker_is_no_wider_than_its_rows_and_keeps_its_keys_in_the_footer() {
    with_default_config(|| {
        let mut app = App::new();
        let mut out = Vec::new();
        seed_tree(&mut app);
        app.focus = Focus::Sessions;
        press(&mut app, KeyCode::Char('p'), KeyModifiers::NONE, &mut out);
        press(&mut app, KeyCode::Tab, KeyModifiers::NONE, &mut out);

        let mut terminal = Terminal::new(TestBackend::new(100, 40)).unwrap();
        let mut drawn = |app: &mut App| -> (ratatui::layout::Rect, String) {
            terminal.draw(|f| ui::draw(f, app)).unwrap();
            let area = match &app.overlay {
                Some(Overlay::Menu(m)) => m.area,
                other => panic!("expected the harness picker, got {other:?}"),
            };
            (area, buffer_text(&terminal))
        };
        let (on_claude, text) = drawn(&mut app);
        let (title_w, rows_w) = match &app.overlay {
            Some(Overlay::Menu(m)) => (
                m.title.as_deref().unwrap().chars().count() + 4,
                m.items
                    .iter()
                    .map(|i| i.label.chars().count())
                    .max()
                    .unwrap()
                    + 6,
            ),
            other => panic!("{other:?}"),
        };
        assert_eq!(
            on_claude.width as usize,
            title_w.max(rows_w),
            "the title or the rows set the width, nothing else:\n{text}"
        );
        let lines: Vec<&str> = text.lines().collect();
        let bottom = (on_claude.y + on_claude.height - 1) as usize;
        assert!(
            !lines[bottom].contains("cloud") && !lines[bottom].contains("settings"),
            "no keys in the bottom border: {}",
            lines[bottom]
        );
        let footer = lines
            .iter()
            .position(|l| l.contains("Tab: cloud off") && l.contains("s/?: settings"))
            .unwrap_or_else(|| panic!("the footer names the picker's keys:\n{text}"));
        assert!(footer > bottom, "below the modal, in the footer bar");

        // Down to Codex: Tab is the Claude row's alone, the width holds.
        press(&mut app, KeyCode::Down, KeyModifiers::NONE, &mut out);
        let (on_codex, text) = drawn(&mut app);
        assert_eq!(on_codex, on_claude, "hovering another row never resizes it");
        assert!(!text.contains("Tab: cloud"), "{text}");
        assert!(text.contains("s/?: settings"), "{text}");
    })
}

/// `Tab` in the box retargets this one launch: the same harness rows
/// the NEW SESSION PICKER offers, with the MODEL / EFFORT submenus
/// behind them — and the typed text survives the trip.
#[test]
fn tab_in_the_quick_prompt_picks_the_harness_and_keeps_the_text() {
    with_default_config(|| {
        let mut app = App::new();
        let mut out = Vec::new();
        seed_tree(&mut app);
        app.focus = Focus::Sessions;
        let worktree = app.selected_worktree().unwrap().id.clone();

        press(&mut app, KeyCode::Char('p'), KeyModifiers::NONE, &mut out);
        assert!(paste_into_overlay(&mut app, "Fix auth"));
        press(&mut app, KeyCode::Tab, KeyModifiers::NONE, &mut out);
        let Some(Overlay::Menu(menu)) = &app.overlay else {
            panic!("Tab should open the harness picker, got {:?}", app.overlay);
        };
        assert_eq!(menu.title.as_deref(), Some("Quick prompt agent"));
        assert_eq!(menu.hover, 0, "starts on the harness the box has");
        assert!(out.is_empty(), "no session, no prewarm yet: {out:?}");

        // Down to Codex, → into its model list, and take a model.
        press(&mut app, KeyCode::Down, KeyModifiers::NONE, &mut out);
        press(&mut app, KeyCode::Right, KeyModifiers::NONE, &mut out);
        let Some(Overlay::Menu(menu)) = &app.overlay else {
            panic!("→ should open the model submenu, got {:?}", app.overlay);
        };
        assert_eq!(menu.title.as_deref(), Some("Codex model"));
        press(&mut app, KeyCode::Down, KeyModifiers::NONE, &mut out);
        let model = match &app.overlay {
            Some(Overlay::Menu(menu)) => menu.items[menu.hover].label.clone(),
            other => panic!("{other:?}"),
        };
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE, &mut out);

        let Some(Overlay::Prompt(prompt)) = &app.overlay else {
            panic!("the pick should hand the box back, got {:?}", app.overlay);
        };
        assert_eq!(prompt.input.as_str(), "Fix auth", "the text came back");
        assert_eq!(prompt.title, format!("Quick prompt (codex · {model})"));
        assert!(out.is_empty(), "still nothing sent: {out:?}");

        press(&mut app, KeyCode::Enter, KeyModifiers::NONE, &mut out);
        assert!(
            matches!(
                out.as_slice(),
                [ClientRequest::CreateAgent {
                    worktree: w,
                    kind: AgentKind::Codex,
                    custom_harness: None,
                    model: Some(m),
                    starting_prompt: Some(text),
                    ..
                }] if *w == worktree && *m == model && text == "Fix auth"
            ),
            "the picked harness launches: {out:?}"
        );
    });
}

/// `Tab` on the Claude row of the box's own `Tab` picker is the NEW
/// SESSION PICKER's cloud toggle: the pick hands back a CLAUDE CLOUD
/// box with the text kept, the next `Tab` opens on the toggle as the
/// box left it, and Enter sends the text as the cloud task — never as
/// a STARTING PROMPT, and with no warm slot consumed or refilled.
#[test]
fn tab_on_claude_in_the_quick_prompt_picker_launches_in_the_cloud() {
    with_default_config(|| {
        let mut app = App::new();
        let mut out = Vec::new();
        seed_tree(&mut app);
        app.focus = Focus::Sessions;
        let worktree = app.selected_worktree().unwrap().id.clone();

        press(&mut app, KeyCode::Char('p'), KeyModifiers::NONE, &mut out);
        assert!(paste_into_overlay(&mut app, "Fix auth"));
        press(&mut app, KeyCode::Tab, KeyModifiers::NONE, &mut out);
        press(&mut app, KeyCode::Tab, KeyModifiers::NONE, &mut out);
        let Some(Overlay::Menu(menu)) = &app.overlay else {
            panic!("Tab should toggle the row in place, got {:?}", app.overlay);
        };
        assert_eq!(menu.title.as_deref(), Some("Quick prompt agent"));
        assert_eq!(menu.items[menu.hover].label, "Claude · cloud");
        assert_eq!(menu.hovered_claude_cloud(), Some(true));

        press(&mut app, KeyCode::Enter, KeyModifiers::NONE, &mut out);
        let Some(Overlay::Prompt(prompt)) = &app.overlay else {
            panic!("the pick should hand the box back, got {:?}", app.overlay);
        };
        assert_eq!(prompt.input.as_str(), "Fix auth", "the text came back");
        assert_eq!(prompt.title, "Quick prompt (claude · cloud)");
        assert!(out.is_empty(), "nothing sent yet: {out:?}");

        press(&mut app, KeyCode::Tab, KeyModifiers::NONE, &mut out);
        let Some(Overlay::Menu(menu)) = &app.overlay else {
            panic!("{:?}", app.overlay);
        };
        assert_eq!(menu.hovered_claude_cloud(), Some(true), "opens as left");
        press(&mut app, KeyCode::Esc, KeyModifiers::NONE, &mut out);

        press(&mut app, KeyCode::Enter, KeyModifiers::NONE, &mut out);
        assert!(app.overlay.is_none(), "{:?}", app.overlay);
        assert!(
            matches!(
                out.as_slice(),
                [ClientRequest::CreateAgent {
                    worktree: w,
                    kind: AgentKind::Claude,
                    custom_harness: None,
                    cloud_prompt: Some(task),
                    starting_prompt: None,
                    ..
                }] if *w == worktree && task == "Fix auth"
            ),
            "a cloud launch, no prewarm behind it: {out:?}"
        );
    });
}

/// Backing out of either picker is a return trip: the box comes back
/// exactly as it left.
#[test]
fn esc_out_of_a_quick_prompt_picker_restores_the_box() {
    with_seeded_presets(|| {
        let mut app = App::new();
        let mut out = Vec::new();
        seed_tree(&mut app);
        app.focus = Focus::Sessions;
        press(&mut app, KeyCode::Char('p'), KeyModifiers::NONE, &mut out);
        assert!(paste_into_overlay(&mut app, "Fix auth"));

        for open in [KeyCode::Tab, KeyCode::BackTab] {
            press(&mut app, open, KeyModifiers::NONE, &mut out);
            assert!(
                !matches!(&app.overlay, Some(Overlay::Prompt(_))),
                "{open:?} should open a picker"
            );
            press(&mut app, KeyCode::Esc, KeyModifiers::NONE, &mut out);
            let Some(Overlay::Prompt(prompt)) = &app.overlay else {
                panic!("Esc should hand the box back, got {:?}", app.overlay);
            };
            assert_eq!(prompt.input.as_str(), "Fix auth");
            assert_eq!(prompt.title, "Quick prompt (claude)", "spec unchanged");
        }
        assert!(out.is_empty(), "{out:?}");
    });
}

/// Closing the box is not throwing the prompt away: Esc, a click
/// outside and the HARDWIRED UNLOCK park what was typed, and the next
/// QUICK PROMPT takes it back — whole, harness pick and all, when it
/// is aimed at the same checkout, and text-only into a box aimed
/// somewhere else (the cursor moved onto another worktree's band, or
/// `^N` flipped the box). Clearing the box and closing it is how a
/// draft is thrown away; an empty box parks nothing.
#[test]
fn a_closed_quick_prompt_is_parked_and_the_next_box_takes_it_back() {
    with_default_config(|| {
        let mut app = App::new();
        let mut out = Vec::new();
        seed_tree(&mut app);
        seed_feat_worktree(&mut app, "w2", "feat");
        app.focus = Focus::Sessions;
        assert_eq!(
            app.selected_worktree().map(|w| w.branch.as_str()),
            Some("main")
        );

        // A box with a harness picked in it and a prompt typed.
        press(&mut app, KeyCode::Char('p'), KeyModifiers::NONE, &mut out);
        press(&mut app, KeyCode::Tab, KeyModifiers::NONE, &mut out);
        press(&mut app, KeyCode::Down, KeyModifiers::NONE, &mut out);
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE, &mut out);
        assert!(paste_into_overlay(&mut app, "Fix auth"));
        assert!(matches!(&app.overlay, Some(Overlay::Prompt(p))
                if p.title == "Quick prompt (codex)"));

        press(&mut app, KeyCode::Esc, KeyModifiers::NONE, &mut out);
        assert!(app.overlay.is_none(), "Esc closes the box");
        assert!(app.quick_draft.is_some(), "and parks what was typed");

        // The same place: the box comes back whole, and the slot is
        // empty again.
        press(&mut app, KeyCode::Char('p'), KeyModifiers::NONE, &mut out);
        let Some(Overlay::Prompt(prompt)) = &app.overlay else {
            panic!("p should open the box, got {:?}", app.overlay);
        };
        assert_eq!(prompt.input.as_str(), "Fix auth");
        assert_eq!(prompt.title, "Quick prompt (codex)", "the pick too");
        assert!(app.quick_draft.is_none(), "taken back, not copied");

        // The HARDWIRED UNLOCK parks it the same way.
        press(
            &mut app,
            KeyCode::Char('q'),
            KeyModifiers::CONTROL,
            &mut out,
        );
        assert!(app.overlay.is_none(), "^q closes the box");
        assert!(app.quick_draft.is_some(), "and parks it");

        // The cursor on a checkout with nothing running in it has no
        // band to be on, so it moves nothing: the box still lands on
        // the root branch, so it is the same box and comes back whole.
        let root = crate::quick_prompt::QuickTarget::Worktree(nebula_core::WorktreeId("w1".into()));
        app.sel_worktree = 1;
        assert_eq!(
            app.selected_worktree().map(|w| w.branch.as_str()),
            Some("feat")
        );
        press(&mut app, KeyCode::Char('p'), KeyModifiers::NONE, &mut out);
        let Some(Overlay::Prompt(prompt)) = &app.overlay else {
            panic!("p should open the box, got {:?}", app.overlay);
        };
        assert_eq!(prompt.input.as_str(), "Fix auth");
        assert_eq!(prompt.title, "Quick prompt (codex)", "the pick too");
        assert!(matches!(&prompt.kind, PromptKind::QuickPrompt(launch)
                if launch.target == root));

        // A box aimed somewhere else — `^N` flipped it onto a fresh
        // worktree before it was closed — does not hand its aim and
        // spec to the next box; the text is the user's, so it still
        // comes back.
        press(
            &mut app,
            KeyCode::Char('n'),
            KeyModifiers::CONTROL,
            &mut out,
        );
        press(&mut app, KeyCode::Esc, KeyModifiers::NONE, &mut out);
        assert!(app.quick_draft.is_some(), "Esc parks the flipped box");
        press(&mut app, KeyCode::Char('p'), KeyModifiers::NONE, &mut out);
        let Some(Overlay::Prompt(prompt)) = &app.overlay else {
            panic!("p should open the box, got {:?}", app.overlay);
        };
        assert_eq!(prompt.input.as_str(), "Fix auth");
        assert_eq!(
            prompt.title, "Quick prompt (claude)",
            "a differently aimed box is the settings' harness again"
        );
        assert!(matches!(&prompt.kind, PromptKind::QuickPrompt(launch)
                if launch.target == root));

        // A session running in `feat` gives it a band, and the cursor
        // on that band aims the box there: a different checkout, so
        // the text comes back into feat's own box.
        press(&mut app, KeyCode::Esc, KeyModifiers::NONE, &mut out);
        assert!(app.quick_draft.is_some(), "Esc parks the root box");
        seed_agent_in(&mut app, "a2", &nebula_core::WorktreeId("w2".into()));
        app.sel_worktree = 1;
        assert_eq!(
            app.selected_worktree().map(|w| w.branch.as_str()),
            Some("feat")
        );
        press(&mut app, KeyCode::Char('p'), KeyModifiers::NONE, &mut out);
        let Some(Overlay::Prompt(prompt)) = &app.overlay else {
            panic!("p should open the box, got {:?}", app.overlay);
        };
        assert_eq!(prompt.input.as_str(), "Fix auth");
        let feat = crate::quick_prompt::QuickTarget::Worktree(nebula_core::WorktreeId("w2".into()));
        assert!(
            matches!(&prompt.kind, PromptKind::QuickPrompt(launch) if launch.target == feat),
            "the box is the band's checkout's: {:?}",
            prompt.kind
        );

        // Cleared and closed: nothing is parked, and the next box is
        // the empty one it should be.
        if let Some(Overlay::Prompt(prompt)) = &mut app.overlay {
            prompt.input.clear();
        }
        press(&mut app, KeyCode::Esc, KeyModifiers::NONE, &mut out);
        assert!(app.quick_draft.is_none(), "an empty box parks nothing");
        press(&mut app, KeyCode::Char('p'), KeyModifiers::NONE, &mut out);
        assert!(matches!(&app.overlay, Some(Overlay::Prompt(p)) if p.input.is_empty()));
        assert!(out.is_empty(), "none of this launches anything: {out:?}");
    });
}

/// `Shift+Tab` picks a saved AGENT PRESET for this launch: its harness,
/// its MODEL / EFFORT, and its prefix/postfix around what was typed.
#[test]
fn shift_tab_applies_a_preset_and_wraps_the_task() {
    with_seeded_presets(|| {
        let mut app = App::new();
        let mut out = Vec::new();
        seed_tree(&mut app);
        app.focus = Focus::Sessions;
        press(&mut app, KeyCode::Char('p'), KeyModifiers::NONE, &mut out);
        assert!(paste_into_overlay(&mut app, "Fix auth"));

        press(&mut app, KeyCode::BackTab, KeyModifiers::NONE, &mut out);
        let Some(Overlay::AgentPresets(view)) = &app.overlay else {
            panic!("⇧Tab should open the presets, got {:?}", app.overlay);
        };
        assert!(view.is_picker(), "opened to pick, not to manage");
        assert_eq!(view.presets[0].name, "reviewer");

        // The manage verbs answer here as in the Sessions panel's list,
        // and come back to the picker: `a` then Esc leaves the box and
        // its text waiting behind it.
        press(
            &mut app,
            KeyCode::Char('a'),
            KeyModifiers::CONTROL,
            &mut out,
        );
        assert!(
            matches!(&app.overlay, Some(Overlay::AgentPresetEditor(e)) if e.quick.is_some()),
            "a opens the editor from the picker: {:?}",
            app.overlay
        );
        press(&mut app, KeyCode::Esc, KeyModifiers::NONE, &mut out);
        let Some(Overlay::AgentPresets(view)) = &app.overlay else {
            panic!("Esc goes back to the picker, got {:?}", app.overlay);
        };
        assert!(view.is_picker(), "still the picker, not the manager");
        assert_eq!(
            view.quick.as_ref().map(|back| back.text.as_str()),
            Some("Fix auth")
        );

        press(&mut app, KeyCode::Enter, KeyModifiers::NONE, &mut out);
        let Some(Overlay::Prompt(prompt)) = &app.overlay else {
            panic!("the pick should hand the box back, got {:?}", app.overlay);
        };
        assert_eq!(prompt.input.as_str(), "Fix auth");
        assert_eq!(
            prompt.title,
            "Quick prompt · reviewer (claude · opus · high)"
        );
        assert!(
            prompt.label.contains("prefix + your task + postfix"),
            "{}",
            prompt.label
        );

        press(&mut app, KeyCode::Enter, KeyModifiers::NONE, &mut out);
        assert!(
            matches!(
                out.as_slice(),
                [ClientRequest::CreateAgent {
                    kind: AgentKind::Claude,
                    custom_harness: None,
                    model: Some(model),
                    effort: Some(effort),
                    starting_prompt: Some(text),
                    ..
                }] if model == "opus"
                    && effort == "high"
                    && text == "Be strict.\n\nFix auth\n\nRun the tests."
            ),
            "the preset wraps the typed task: {out:?}"
        );
    });
}

/// A preset's task is optional in the QUICK PROMPT too: an empty box it
/// is on launches on prefix + postfix. A `skip_task` preset picked over
/// an empty box launches at once; over typed text it is only applied.
#[test]
fn shift_tab_presets_make_the_quick_prompt_task_optional() {
    with_seeded_presets(|| {
        let mut app = App::new();
        let mut out = Vec::new();
        seed_tree(&mut app);
        app.focus = Focus::Sessions;
        let wrapped = |out: &[ClientRequest]| {
            matches!(
                out,
                [ClientRequest::CreateAgent {
                    kind: AgentKind::Claude,
                    custom_harness: None,
                    starting_prompt: Some(text),
                    ..
                }] if text == "Be strict.\n\nRun the tests."
            )
        };

        // An ordinary preset: picked, then the empty box sent.
        press(&mut app, KeyCode::Char('p'), KeyModifiers::NONE, &mut out);
        press(&mut app, KeyCode::BackTab, KeyModifiers::NONE, &mut out);
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE, &mut out);
        assert!(
            matches!(&app.overlay, Some(Overlay::Prompt(_))),
            "an asking preset hands the box back: {:?}",
            app.overlay
        );
        assert!(out.is_empty(), "{out:?}");
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE, &mut out);
        assert!(app.overlay.is_none(), "{:?}", app.overlay);
        assert!(wrapped(&out), "the empty box sends the wrapping: {out:?}");

        // The same preset set to skip the task.
        let mut presets = crate::agent_presets::load();
        presets[0].skip_task = true;
        crate::agent_presets::save(&presets).unwrap();

        // Over typed text it is applied, not launched.
        out.clear();
        press(&mut app, KeyCode::Char('p'), KeyModifiers::NONE, &mut out);
        assert!(paste_into_overlay(&mut app, "Fix auth"));
        press(&mut app, KeyCode::BackTab, KeyModifiers::NONE, &mut out);
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE, &mut out);
        let Some(Overlay::Prompt(prompt)) = &app.overlay else {
            panic!("typed text keeps the box, got {:?}", app.overlay);
        };
        assert_eq!(prompt.input.as_str(), "Fix auth");
        assert!(out.is_empty(), "{out:?}");

        // Over an empty box it launches the moment it is picked.
        app.overlay = None;
        press(&mut app, KeyCode::Char('p'), KeyModifiers::NONE, &mut out);
        press(&mut app, KeyCode::BackTab, KeyModifiers::NONE, &mut out);
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE, &mut out);
        assert!(app.overlay.is_none(), "launched: {:?}", app.overlay);
        assert!(wrapped(&out), "{out:?}");
    });
}

/// With no presets saved the picker still opens — empty, on the
/// manager's `a` hint — so the first one is made right there: saving it
/// lands back in the picker on it, and Enter hands the box back, text
/// intact, under that preset.
#[test]
fn shift_tab_without_presets_opens_a_picker_that_adds_one() {
    with_default_config(|| {
        let dir = tempfile::tempdir().unwrap();
        crate::agent_presets::with_presets_path(dir.path().join("agent_presets.json"), || {
            let mut app = App::new();
            let mut out = Vec::new();
            seed_tree(&mut app);
            app.focus = Focus::Sessions;
            press(&mut app, KeyCode::Char('p'), KeyModifiers::NONE, &mut out);
            assert!(paste_into_overlay(&mut app, "Fix auth"));
            press(&mut app, KeyCode::BackTab, KeyModifiers::NONE, &mut out);
            let Some(Overlay::AgentPresets(view)) = &app.overlay else {
                panic!("an empty picker, got {:?}", app.overlay);
            };
            assert!(view.is_picker() && view.presets.is_empty(), "{view:?}");
            let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
            terminal.draw(|f| ui::draw(f, &mut app)).unwrap();
            let text = buffer_text(&terminal);
            assert!(
                text.contains("no presets yet — Ctrl+a creates one"),
                "{text}"
            );

            press(
                &mut app,
                KeyCode::Char('a'),
                KeyModifiers::CONTROL,
                &mut out,
            );
            type_text(&mut app, "tidy", &mut out);
            press(&mut app, KeyCode::Enter, KeyModifiers::NONE, &mut out);
            let Some(Overlay::AgentPresets(view)) = &app.overlay else {
                panic!("the save lands back in the picker, got {:?}", app.overlay);
            };
            assert!(view.is_picker(), "still the picker, not the manager");
            assert_eq!(view.presets.len(), 1);
            assert_eq!(view.presets[view.selected].name, "tidy");

            press(&mut app, KeyCode::Enter, KeyModifiers::NONE, &mut out);
            let Some(Overlay::Prompt(prompt)) = &app.overlay else {
                panic!("the pick hands the box back, got {:?}", app.overlay);
            };
            assert_eq!(prompt.input.as_str(), "Fix auth");
            assert!(prompt.title.contains("tidy"), "{}", prompt.title);
            assert!(out.is_empty(), "{out:?}");
        });
    });
}

/// A long prompt typed without one Shift+Enter is ONE line wrapped over
/// many rows, and used to be a dead end for ↑/↓ and ⌥↑. Now ↑/↓ walk
/// the rows as drawn, ⌥↑/⌥↓ jump to the paragraph's start or end, ↑ on
/// the top row goes to the text's start and ↓ on the bottom row to its
/// end, the box scrolls only as the caret leaves it (with `↑ N more`
/// on its border), the wheel scrolls it, and a click puts the caret
/// where it points.
#[test]
fn the_quick_prompt_walks_a_long_wrapped_prompt() {
    fn input(app: &App) -> &TextInput {
        let Some(Overlay::Prompt(prompt)) = &app.overlay else {
            panic!("the quick prompt should be up, got {:?}", app.overlay);
        };
        &prompt.input
    }
    fn row_of(input: &TextInput) -> usize {
        input.caret_row(&input.rows(input.view().width.into()))
    }
    with_default_config(|| {
        let mut app = App::new();
        let mut out = Vec::new();
        seed_tree(&mut app);
        app.focus = Focus::Sessions;
        press(&mut app, KeyCode::Char('p'), KeyModifiers::NONE, &mut out);
        let words: Vec<String> = (0..300).map(|i| format!("w{i:03}")).collect();
        let long = words.join(" ");
        assert!(paste_into_overlay(&mut app, &long));
        let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
        let mut draw = |app: &mut App| {
            terminal.draw(|f| ui::draw(f, app)).unwrap();
            buffer_text(&terminal)
        };

        let text = draw(&mut app);
        let view = input(&app).view();
        let width = usize::from(view.width);
        let (top, height) = (usize::from(view.top), usize::from(view.height));
        let rows = input(&app).rows(width).len();
        assert!(width > 0 && rows > height, "{view:?}, {rows} rows");
        assert_eq!(row_of(input(&app)), rows - 1, "caret at the end");
        assert_eq!(top, rows - height, "the tail in sight");
        assert!(text.contains("↑ ") && text.contains(" more"), "{text}");

        // ↑ is a row up the wrapped paragraph, and the box stays put.
        press(&mut app, KeyCode::Up, KeyModifiers::NONE, &mut out);
        assert_eq!(row_of(input(&app)), rows - 2);
        draw(&mut app);
        assert_eq!(input(&app).view().top, view.top, "no scroll inside the box");

        // ⌥↑ — what the user reached for — is the paragraph's start.
        press(&mut app, KeyCode::Up, KeyModifiers::ALT, &mut out);
        assert_eq!(input(&app).cursor_chars(), 0);
        let text = draw(&mut app);
        assert_eq!(input(&app).view().top, 0);
        assert!(text.contains("w000") && text.contains("↓ "), "{text}");

        // ↓ then ↑ walk the rows keeping the column; ↑ on the top row
        // is the very start.
        press(&mut app, KeyCode::Right, KeyModifiers::NONE, &mut out);
        press(&mut app, KeyCode::Right, KeyModifiers::NONE, &mut out);
        press(&mut app, KeyCode::Down, KeyModifiers::NONE, &mut out);
        assert_eq!(row_of(input(&app)), 1);
        let second = input(&app).rows(width)[1].0;
        assert_eq!(input(&app).cursor_chars(), second + 2);
        press(&mut app, KeyCode::Up, KeyModifiers::NONE, &mut out);
        assert_eq!(input(&app).cursor_chars(), 2);
        press(&mut app, KeyCode::Up, KeyModifiers::NONE, &mut out);
        assert_eq!(input(&app).cursor_chars(), 0, "top row ↑: the start");

        // ⌥↓ is the end again, and ↓ on the bottom row too.
        press(&mut app, KeyCode::Down, KeyModifiers::ALT, &mut out);
        assert_eq!(input(&app).cursor_chars(), long.len());
        press(&mut app, KeyCode::Left, KeyModifiers::NONE, &mut out);
        press(&mut app, KeyCode::Down, KeyModifiers::NONE, &mut out);
        assert_eq!(input(&app).cursor_chars(), long.len(), "bottom row ↓");
        draw(&mut app);

        // The wheel scrolls the box, dragging the caret into sight.
        let editor = match &app.overlay {
            Some(Overlay::Prompt(prompt)) => prompt.editor_area,
            _ => unreachable!(),
        };
        let bottom_top = usize::from(input(&app).view().top);
        handle_mouse(
            &mut app,
            mev(MouseEventKind::ScrollUp, editor.x + 1, editor.y + 1),
            &mut out,
        );
        let top = usize::from(input(&app).view().top);
        assert_eq!(top, bottom_top - MODAL_WHEEL_LINES as usize);
        assert!(row_of(input(&app)) < top + height);
        draw(&mut app);

        // A click lands the caret where it points.
        click(&mut app, editor.x + 3, editor.y + 1, &mut out);
        let rows_now = input(&app).rows(width);
        let top = usize::from(input(&app).view().top);
        assert_eq!(input(&app).cursor_chars(), rows_now[top + 1].0 + 3);
        assert!(
            matches!(&app.overlay, Some(Overlay::Prompt(p)) if p.input.as_str() == long),
            "nothing typed, nothing lost"
        );
        assert!(out.is_empty(), "{out:?}");
    });
}

/// `Ctrl+N` in the box flips where the launch lands — the selected
/// checkout or a fresh worktree — from whichever panel `p` was pressed
/// in, keeping the text and the caret, and Enter then takes the route
/// the WORKTREES PANEL's `p` takes: a `CreateWorktree` first, the
/// create following its Ack into the new checkout.
#[test]
fn ctrl_n_in_the_quick_prompt_flips_the_launch_into_a_fresh_worktree() {
    use crate::quick_prompt::QuickTarget;
    use nebula_core::{EntityId, ProjectId, WorktreeId};
    with_config_json(r#"{"follow_new_session": true}"#, || {
        let mut app = App::new();
        let mut out = Vec::new();
        seed_tree(&mut app);
        seed_feat_worktree(&mut app, "w2", "feat");
        app.focus = Focus::Sessions;
        let selected = app.selected_worktree().unwrap().id.clone();
        // Where the box would launch, what it is titled, and the text
        // and caret in it.
        let state = |app: &App| match &app.overlay {
            Some(Overlay::Prompt(prompt)) => match &prompt.kind {
                PromptKind::QuickPrompt(launch) => (
                    launch.target.clone(),
                    prompt.title.clone(),
                    prompt.input.as_str().to_string(),
                    prompt.input.cursor_chars(),
                ),
                other => panic!("expected the quick prompt, got {other:?}"),
            },
            other => panic!("expected the box, got {other:?}"),
        };

        press(&mut app, KeyCode::Char('p'), KeyModifiers::NONE, &mut out);
        assert!(paste_into_overlay(&mut app, "Fix auth"));
        // The caret parked mid-text, to prove the flip keeps it.
        press(&mut app, KeyCode::Left, KeyModifiers::NONE, &mut out);
        press(&mut app, KeyCode::Left, KeyModifiers::NONE, &mut out);
        assert_eq!(state(&app).0, QuickTarget::Worktree(selected.clone()));

        press(
            &mut app,
            KeyCode::Char('n'),
            KeyModifiers::CONTROL,
            &mut out,
        );
        let (target, title, text, caret) = state(&app);
        let branch = match target {
            QuickTarget::NewWorktree { project, branch } => {
                assert_eq!(project, ProjectId("p1".into()));
                branch
            }
            other => panic!("^N should aim at a fresh worktree, got {other:?}"),
        };
        assert!(
            !["main", "feat"].contains(&branch.as_str()),
            "{branch} is taken"
        );
        assert_eq!(
            title,
            format!("Quick prompt · new worktree {branch} (claude)")
        );
        assert_eq!(text, "Fix auth", "the text survives the flip");
        assert_eq!(caret, 6, "and so does the caret");
        assert!(out.is_empty(), "flipping sends nothing: {out:?}");

        // And back: the selected checkout, text and caret still there.
        press(
            &mut app,
            KeyCode::Char('n'),
            KeyModifiers::CONTROL,
            &mut out,
        );
        let (target, title, text, caret) = state(&app);
        assert_eq!(target, QuickTarget::Worktree(selected.clone()));
        assert_eq!(title, "Quick prompt (claude)");
        assert_eq!((text.as_str(), caret), ("Fix auth", 6));

        // On again and Enter: the worktree is cut first, the launch
        // follows the Ack into it, and FOCUS stays where p was pressed.
        press(
            &mut app,
            KeyCode::Char('n'),
            KeyModifiers::CONTROL,
            &mut out,
        );
        let branch = match state(&app).0 {
            QuickTarget::NewWorktree { branch, .. } => branch,
            other => panic!("{other:?}"),
        };
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE, &mut out);
        assert!(app.overlay.is_none(), "launching closes the box");
        // Both stand-in rows are up before git has run — and the
        // session's is up *working*, because the create it stands for
        // carries the typed prompt: the CLI submits it as it boots, so
        // the row goes up the way it will come back.
        assert_eq!(
            app.visible_sessions()
                .first()
                .map(|a| a.status)
                .expect("the stand-in session row is up"),
            nebula_core::AgentStatus::Running
        );
        let req_id = match out.as_slice() {
            [ClientRequest::CreateWorktree {
                req_id,
                project,
                branch: b,
                base: None,
            }] if project == &ProjectId("p1".into()) && b == &branch => *req_id,
            other => panic!("one base-less CreateWorktree first: {other:?}"),
        };
        out.clear();
        seed_feat_worktree(&mut app, "w3", &branch);
        handle_server_event(
            &mut app,
            ServerEvent::Ack {
                req_id,
                created: Some(EntityId::Worktree(WorktreeId("w3".into()))),
            },
            &mut out,
        );
        assert!(
            matches!(
                out.as_slice(),
                [ClientRequest::CreateAgent {
                    worktree,
                    starting_prompt: Some(text),
                    ..
                }] if worktree.0 == "w3" && text == "Fix auth"
            ),
            "then one create in it carrying the typed prompt: {out:?}"
        );
        assert_eq!(
            app.selected_worktree().map(|w| w.id.0.as_str()),
            Some("w3"),
            "the cursor is on the new row"
        );
        assert_eq!(
            app.focus,
            Focus::Sessions,
            "focus stays where p was pressed"
        );
    });
}

/// Nothing typed is not a change of mind: Enter on the empty box
/// starts the session the title names — that harness, model and
/// effort — with no first prompt, the CLI's own input being it, as
/// `n` does. The create carries no task and the stand-in row goes up
/// fresh, since nothing is on its way to the CLI. Only a CLAUDE CLOUD
/// box, which cannot start without its task, closes instead.
#[test]
fn an_empty_quick_prompt_starts_the_cli_with_no_first_prompt() {
    with_default_config(|| {
        let mut app = App::new();
        let mut out = Vec::new();
        seed_tree(&mut app);
        app.focus = Focus::Sessions;
        let worktree = app.selected_worktree().unwrap().id.clone();

        press(&mut app, KeyCode::Char('p'), KeyModifiers::NONE, &mut out);
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE, &mut out);
        assert!(
            app.overlay.is_none(),
            "empty Enter launches and closes the box: {:?}",
            app.overlay
        );
        assert_ne!(app.flash.as_deref(), Some("cancelled: empty input"));
        // A bare create is one a warm CLI can stand in for, so the
        // slot is refilled behind it — the same trail `n` leaves.
        assert!(
            matches!(
                out.as_slice(),
                [
                    ClientRequest::CreateAgent {
                        worktree: w,
                        kind: AgentKind::Claude,
                        cloud_prompt: None,
                        starting_prompt: None,
                        issue_url: None,
                        ..
                    },
                    ClientRequest::PrewarmAgent { .. }
                ] if *w == worktree
            ),
            "one bare create in the selected checkout, then the refill: {out:?}"
        );
        assert_eq!(
            app.visible_sessions().first().map(|a| a.status),
            Some(nebula_core::AgentStatus::Fresh),
            "the stand-in row goes up fresh: no prompt is on its way"
        );

        // The cloud box is the one that cannot: there is nothing to
        // send as the task.
        out.clear();
        press(&mut app, KeyCode::Char('p'), KeyModifiers::NONE, &mut out);
        press(&mut app, KeyCode::Tab, KeyModifiers::NONE, &mut out);
        press(&mut app, KeyCode::Tab, KeyModifiers::NONE, &mut out);
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE, &mut out);
        let Some(Overlay::Prompt(prompt)) = &app.overlay else {
            panic!(
                "the pick should hand a cloud box back, got {:?}",
                app.overlay
            );
        };
        assert_eq!(prompt.title, "Quick prompt (claude · cloud)");
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE, &mut out);
        assert!(app.overlay.is_none(), "{:?}", app.overlay);
        assert_eq!(app.flash.as_deref(), Some("cancelled: empty input"));
        assert!(out.is_empty(), "a cloud box needs its task: {out:?}");
    });
}

/// The agent has to run somewhere: with no checkout under the cursor
/// (an empty tree) `p` says so instead of opening a box that cannot
/// launch. On the WORKTREES PANEL the checkout is cut on the way, so
/// only a PROJECT is needed — and an empty tree has none of those
/// either. (A cursor parked on an OPEN PRS row is not this case: the
/// pull request's own checkout is where that box launches.)
#[test]
fn the_quick_prompt_needs_a_worktree() {
    with_default_config(|| {
        let mut app = App::new();
        let mut out = Vec::new();
        press(&mut app, KeyCode::Char('p'), KeyModifiers::NONE, &mut out);
        assert!(app.overlay.is_none(), "{:?}", app.overlay);
        assert_eq!(
            app.flash.as_deref(),
            Some("quick prompt: select a worktree first")
        );
        assert!(out.is_empty(), "{out:?}");

        app.focus = Focus::Worktrees;
        press(&mut app, KeyCode::Char('p'), KeyModifiers::NONE, &mut out);
        assert!(app.overlay.is_none(), "{:?}", app.overlay);
        assert_eq!(
            app.flash.as_deref(),
            Some("quick prompt: select a project first")
        );
        assert!(out.is_empty(), "{out:?}");
    });
}

#[test]
fn cursor_preset_effort_follows_the_model_family() {
    use crate::preset_overlays::PresetField;
    with_seeded_presets(|| {
        let mut app = App::new();
        let mut out = Vec::new();
        open_presets(&mut app, &mut out);
        press(
            &mut app,
            KeyCode::Char('a'),
            KeyModifiers::CONTROL,
            &mut out,
        );
        type_text(&mut app, "cur", &mut out);
        press(&mut app, KeyCode::Tab, KeyModifiers::NONE, &mut out);
        press(&mut app, KeyCode::Right, KeyModifiers::NONE, &mut out);
        press(&mut app, KeyCode::Right, KeyModifiers::NONE, &mut out);
        press(&mut app, KeyCode::Tab, KeyModifiers::NONE, &mut out);
        let Some(Overlay::AgentPresetEditor(editor)) = &app.overlay else {
            panic!("editor should stay open, got {:?}", app.overlay);
        };
        assert_eq!(editor.kind, AgentKind::Cursor);
        assert_eq!(editor.field, PresetField::Model, "cursor has a model row");
        // With the model still default there is no suffix to pick:
        // Effort reads n/a and Tab skips it.
        let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
        terminal.draw(|f| ui::draw(f, &mut app)).unwrap();
        let text = buffer_text(&terminal);
        assert!(text.contains("Effort  n/a"), "no family yet:\n{text}");
        press(&mut app, KeyCode::Tab, KeyModifiers::NONE, &mut out);
        let Some(Overlay::AgentPresetEditor(editor)) = &app.overlay else {
            unreachable!()
        };
        assert_eq!(editor.field, PresetField::Text, "Tab skips the n/a effort");
        press(&mut app, KeyCode::BackTab, KeyModifiers::SHIFT, &mut out);

        // → → : default → auto → claude-fable-5, whose efforts exist.
        press(&mut app, KeyCode::Right, KeyModifiers::NONE, &mut out);
        press(&mut app, KeyCode::Right, KeyModifiers::NONE, &mut out);
        press(&mut app, KeyCode::Tab, KeyModifiers::NONE, &mut out);
        let Some(Overlay::AgentPresetEditor(editor)) = &app.overlay else {
            unreachable!()
        };
        assert_eq!(editor.model, "claude-fable-5");
        assert_eq!(
            editor.field,
            PresetField::Effort,
            "a family with efforts gets the row"
        );
        // No bare `claude-fable-5` id exists, so the effort lands on the
        // family's fallback at once; → → walks up to max.
        assert_eq!(editor.effort, "high");
        press(&mut app, KeyCode::Right, KeyModifiers::NONE, &mut out);
        press(&mut app, KeyCode::Right, KeyModifiers::NONE, &mut out);
        let Some(Overlay::AgentPresetEditor(editor)) = &app.overlay else {
            unreachable!()
        };
        assert_eq!(editor.effort, "max");
        // Stepping the model past fable-5-thinking (has max) onto Opus 5
        // (no max variant) drops it back to that family's fallback.
        press(&mut app, KeyCode::BackTab, KeyModifiers::SHIFT, &mut out);
        press(&mut app, KeyCode::Right, KeyModifiers::NONE, &mut out);
        press(&mut app, KeyCode::Right, KeyModifiers::NONE, &mut out);
        let Some(Overlay::AgentPresetEditor(editor)) = &app.overlay else {
            unreachable!()
        };
        assert_eq!(editor.model, "claude-opus-5");
        assert_eq!(editor.effort, "high", "max dropped with the family");
        press(&mut app, KeyCode::Tab, KeyModifiers::NONE, &mut out);
        press(&mut app, KeyCode::Right, KeyModifiers::NONE, &mut out);
        let Some(Overlay::AgentPresetEditor(editor)) = &app.overlay else {
            unreachable!()
        };
        assert_eq!(editor.effort, "high-fast", "-fast is the next effort row");
        press(&mut app, KeyCode::Left, KeyModifiers::NONE, &mut out);

        press(&mut app, KeyCode::Enter, KeyModifiers::NONE, &mut out);
        let saved = crate::agent_presets::load();
        assert_eq!(saved[2].kind, AgentKind::Cursor);
        assert_eq!(saved[2].model.as_deref(), Some("claude-opus-5"));
        assert_eq!(saved[2].effort.as_deref(), Some("high"));
        terminal.draw(|f| ui::draw(f, &mut app)).unwrap();
        let text = buffer_text(&terminal);
        assert!(
            text.contains("cur  cursor · claude-opus-5 · high"),
            "spec label names family and effort:\n{text}"
        );
    });
}

#[test]
fn picker_model_submenu_types_to_filter() {
    with_default_config(|| {
        let mut app = App::new();
        seed_tree(&mut app);
        app.focus = Focus::Sessions;
        let mut out = Vec::new();
        open_picker(&mut app);
        press(&mut app, KeyCode::Down, KeyModifiers::NONE, &mut out);
        press(&mut app, KeyCode::Down, KeyModifiers::NONE, &mut out);
        press(&mut app, KeyCode::Right, KeyModifiers::NONE, &mut out);
        let Some(Overlay::Menu(menu)) = &app.overlay else {
            panic!("expected cursor model submenu, got {:?}", app.overlay);
        };
        let all = menu.items.len();
        assert!(menu.filter.is_some(), "model submenus take type-ahead");

        // The title shows the affordance before anything is typed.
        let mut terminal = Terminal::new(TestBackend::new(120, 40)).unwrap();
        terminal.draw(|f| ui::draw(f, &mut app)).unwrap();
        let text = buffer_text(&terminal);
        assert!(
            text.contains("Cursor model ⌕"),
            "bare ⌕ in the title:\n{text}"
        );
        assert!(
            text.contains("type to filter"),
            "hint on the border:\n{text}"
        );

        // "opus" narrows to the two Opus families, best first, hover
        // on the first; the letters did not move the hover as j/k.
        type_text(&mut app, "opus", &mut out);
        let Some(Overlay::Menu(menu)) = &app.overlay else {
            unreachable!()
        };
        assert_eq!(
            menu.items
                .iter()
                .map(|i| i.label.as_str())
                .collect::<Vec<_>>(),
            vec!["claude-opus-5", "claude-opus-5-thinking"]
        );
        assert_eq!(menu.hover, 0);
        assert_eq!(menu.filter_query(), "opus");
        terminal.draw(|f| ui::draw(f, &mut app)).unwrap();
        let text = buffer_text(&terminal);
        assert!(
            text.contains("Cursor model ⌕ opus"),
            "query in the title:\n{text}"
        );

        // A letter no row matches is refused and flashed.
        type_text(&mut app, "z", &mut out);
        let Some(Overlay::Menu(menu)) = &app.overlay else {
            unreachable!()
        };
        assert_eq!(menu.filter_query(), "opus");
        assert_eq!(menu.items.len(), 2);
        assert!(app.flash.as_deref().is_some_and(|f| f.contains("opusz")));

        // ↓ moves within the matches; → drills the hovered family into
        // its efforts, which start with an empty filter of their own.
        press(&mut app, KeyCode::Down, KeyModifiers::NONE, &mut out);
        press(&mut app, KeyCode::Right, KeyModifiers::NONE, &mut out);
        let Some(Overlay::Menu(menu)) = &app.overlay else {
            unreachable!()
        };
        assert_eq!(menu.title.as_deref(), Some("Cursor effort"));
        assert_eq!(menu.filter_query(), "");
        type_text(&mut app, "hf", &mut out);
        let Some(Overlay::Menu(menu)) = &app.overlay else {
            unreachable!()
        };
        assert!(
            menu.items.iter().all(|i| i.label.ends_with("-fast")),
            "hf keeps only fast rows: {:?}",
            menu.items.iter().map(|i| &i.label).collect::<Vec<_>>()
        );
        assert_eq!(menu.items[0].label, "high-fast");
        assert!(matches!(
            &menu.items[0].action,
            MenuAction::NewAgentOfKind { kind: AgentKind::Cursor, model: Some(m), effort: Some(e), .. }
                if m == "claude-opus-5-thinking" && e == "high-fast"
        ));

        // Backspace widens; Esc clears the text first, then backs out
        // to the (still filtered) model list.
        press(&mut app, KeyCode::Backspace, KeyModifiers::NONE, &mut out);
        let Some(Overlay::Menu(menu)) = &app.overlay else {
            unreachable!()
        };
        assert_eq!(menu.filter_query(), "h");
        press(&mut app, KeyCode::Esc, KeyModifiers::NONE, &mut out);
        let Some(Overlay::Menu(menu)) = &app.overlay else {
            unreachable!()
        };
        assert_eq!(menu.title.as_deref(), Some("Cursor effort"));
        assert_eq!(menu.filter_query(), "");
        press(&mut app, KeyCode::Esc, KeyModifiers::NONE, &mut out);
        let Some(Overlay::Menu(menu)) = &app.overlay else {
            unreachable!()
        };
        assert_eq!(menu.title.as_deref(), Some("Cursor model"));
        assert_eq!(menu.filter_query(), "opus");
        assert_eq!(menu.hover, 1, "← restores the parent's hover");
        // Clearing the filter brings every row back, hovering the ✓.
        press(&mut app, KeyCode::Esc, KeyModifiers::NONE, &mut out);
        let Some(Overlay::Menu(menu)) = &app.overlay else {
            unreachable!()
        };
        assert_eq!(menu.items.len(), all);
        assert_eq!(menu.items[menu.hover].label, "default ✓");
    });
}

#[test]
fn preset_editor_types_to_filter_choice_rows() {
    use crate::preset_overlays::PresetField;
    with_seeded_presets(|| {
        let mut app = App::new();
        let mut out = Vec::new();
        open_presets(&mut app, &mut out);
        press(
            &mut app,
            KeyCode::Char('a'),
            KeyModifiers::CONTROL,
            &mut out,
        );
        type_text(&mut app, "typed", &mut out);
        press(&mut app, KeyCode::Tab, KeyModifiers::NONE, &mut out);
        // Harness: "cu" jumps to cursor; h/l no longer cycle.
        type_text(&mut app, "cu", &mut out);
        let Some(Overlay::AgentPresetEditor(editor)) = &app.overlay else {
            panic!("editor should stay open, got {:?}", app.overlay);
        };
        assert_eq!(editor.field, PresetField::Kind);
        assert_eq!(editor.kind, AgentKind::Cursor);
        assert_eq!(editor.filter, "cu");
        // Tab drops the filter and lands on Model.
        press(&mut app, KeyCode::Tab, KeyModifiers::NONE, &mut out);
        let Some(Overlay::AgentPresetEditor(editor)) = &app.overlay else {
            unreachable!()
        };
        assert_eq!(editor.field, PresetField::Model);
        assert_eq!(editor.filter, "");
        // "sol" → gpt-5.6-sol at once, effort on its fallback; → cycles
        // only the matches (one), so it stays.
        type_text(&mut app, "sol", &mut out);
        let Some(Overlay::AgentPresetEditor(editor)) = &app.overlay else {
            unreachable!()
        };
        assert_eq!(editor.model, "gpt-5.6-sol");
        assert_eq!(editor.effort, "high");
        assert_eq!(editor.filtered_choices(), vec!["gpt-5.6-sol"]);
        press(&mut app, KeyCode::Right, KeyModifiers::NONE, &mut out);
        let Some(Overlay::AgentPresetEditor(editor)) = &app.overlay else {
            unreachable!()
        };
        assert_eq!(editor.model, "gpt-5.6-sol");
        let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
        terminal.draw(|f| ui::draw(f, &mut app)).unwrap();
        let text = buffer_text(&terminal);
        assert!(
            text.contains("gpt-5.6-sol ▸  ⌕ sol (1)"),
            "filter beside the value:\n{text}"
        );

        // A refused letter leaves everything as it was and flashes.
        type_text(&mut app, "q", &mut out);
        let Some(Overlay::AgentPresetEditor(editor)) = &app.overlay else {
            unreachable!()
        };
        assert_eq!(editor.filter, "sol");
        assert!(app.flash.as_deref().is_some_and(|f| f.contains("solq")));
        // Backspace to "so": sol still matches, so the value stays;
        // Esc clears the filter but keeps the value; a second Esc
        // backs out to the list as before.
        press(&mut app, KeyCode::Backspace, KeyModifiers::NONE, &mut out);
        let Some(Overlay::AgentPresetEditor(editor)) = &app.overlay else {
            unreachable!()
        };
        assert_eq!(editor.filter, "so");
        assert_eq!(editor.model, "gpt-5.6-sol");
        press(&mut app, KeyCode::Esc, KeyModifiers::NONE, &mut out);
        let Some(Overlay::AgentPresetEditor(editor)) = &app.overlay else {
            panic!("first Esc only clears the filter, got {:?}", app.overlay);
        };
        assert_eq!(editor.filter, "");
        assert_eq!(editor.model, "gpt-5.6-sol");
        // On the Effort row, "xf" lands on xhigh-fast.
        press(&mut app, KeyCode::Tab, KeyModifiers::NONE, &mut out);
        type_text(&mut app, "xf", &mut out);
        let Some(Overlay::AgentPresetEditor(editor)) = &app.overlay else {
            unreachable!()
        };
        assert_eq!(editor.field, PresetField::Effort);
        assert_eq!(editor.effort, "xhigh-fast");
        press(&mut app, KeyCode::Esc, KeyModifiers::NONE, &mut out);
        press(&mut app, KeyCode::Esc, KeyModifiers::NONE, &mut out);
        assert!(
            matches!(app.overlay, Some(Overlay::AgentPresets(_))),
            "second Esc backs out: {:?}",
            app.overlay
        );
    });
}

#[test]
fn picker_cursor_drills_into_family_then_effort_and_auto_is_a_leaf() {
    with_default_config(|| {
        let mut app = App::new();
        seed_tree(&mut app);
        app.focus = Focus::Sessions;
        let mut out = Vec::new();

        open_picker(&mut app);
        press(&mut app, KeyCode::Down, KeyModifiers::NONE, &mut out);
        press(&mut app, KeyCode::Down, KeyModifiers::NONE, &mut out);
        press(&mut app, KeyCode::Right, KeyModifiers::NONE, &mut out);
        let Some(Overlay::Menu(menu)) = &app.overlay else {
            panic!("expected cursor model submenu, got {:?}", app.overlay);
        };
        assert_eq!(menu.title.as_deref(), Some("Cursor model"));
        assert_eq!(
            menu.items.len(),
            crate::config::model_choices(AgentKind::Cursor, None).len()
        );
        assert_eq!(menu.items[0].label, "default ✓");
        assert_eq!(menu.items[1].label, "auto");
        // `auto` has no effort variants: a leaf. Opus 5 thinking drills.
        assert_eq!(menu.items[1].action.submenu(), None);
        assert_eq!(menu.items[5].label, "claude-opus-5-thinking");
        assert_eq!(menu.items[5].action.submenu(), Some(SubmenuKind::Efforts));

        for _ in 0..5 {
            press(&mut app, KeyCode::Down, KeyModifiers::NONE, &mut out);
        }
        press(&mut app, KeyCode::Right, KeyModifiers::NONE, &mut out);
        let Some(Overlay::Menu(menu)) = &app.overlay else {
            panic!("expected cursor effort submenu, got {:?}", app.overlay);
        };
        assert_eq!(menu.title.as_deref(), Some("Cursor effort"));
        // No bare `claude-opus-5-thinking` id, so no default row; the
        // -fast twins follow their base.
        assert_eq!(
            menu.items
                .iter()
                .map(|i| i.label.as_str())
                .collect::<Vec<_>>(),
            vec![
                "low",
                "low-fast",
                "medium",
                "medium-fast",
                "high",
                "high-fast",
                "xhigh",
                "xhigh-fast",
                "max",
                "max-fast"
            ]
        );
        assert!(matches!(
            &menu.items[9].action,
            MenuAction::NewAgentOfKind { kind: AgentKind::Cursor, model: Some(m), effort: Some(e), .. }
                if m == "claude-opus-5-thinking" && e == "max-fast"
        ));
        assert_eq!(menu.items[9].action.submenu(), None);
    });
}

/// A click on a row of the PR SESSION preset picker is Enter on that
/// row: the pull request rides the launch. The picker's context
/// checkout is the ROOT WORKTREE — it only names the project — and a
/// click used to launch the row straight into it: a `skip`-task preset
/// clicked on an OPEN PRS row started a plain session in the main
/// checkout, no pull request, no checkout cut, nothing nested. The
/// delete keys reach the delete confirm, as in the manager — whose
/// exits once reopened the list in manage mode against the root, and
/// now come back to the picker, the pull request still on it.
#[test]
fn a_click_in_the_pr_preset_picker_launches_the_pr_session_not_a_root_session() {
    with_seeded_presets(|| {
        let mut presets = crate::agent_presets::load();
        presets.push(crate::agent_presets::AgentPreset {
            name: "review and merge".into(),
            kind: AgentKind::Claude,
            custom_harness: None,
            model: None,
            effort: None,
            prefix: "Review this pull request.".into(),
            postfix: String::new(),
            skip_task: true,
        });
        crate::agent_presets::save(&presets).unwrap();
        let skip_row = (presets.len() - 1) as u16;

        let mut app = App::new();
        seed_tree(&mut app);
        seed_open_prs(&mut app, &[(7, "Attach links")]);
        app.focus = Focus::Worktrees;
        app.sel_worktree = 1;
        let mut out = Vec::new();
        press(&mut app, KeyCode::Char('e'), KeyModifiers::NONE, &mut out);

        // The delete keys ask first, and backing out of the confirm
        // lands in the PR picker again.
        for (key, mods) in [
            (KeyCode::Char('d'), KeyModifiers::CONTROL),
            (KeyCode::Delete, KeyModifiers::NONE),
        ] {
            press(&mut app, key, mods, &mut out);
            assert!(
                matches!(
                    &app.overlay,
                    Some(Overlay::Confirm(c))
                        if matches!(&c.action, PendingAction::DeleteAgentPreset { quick: Some(_), .. })
                ),
                "{key:?} asks before deleting: {:?}",
                app.overlay
            );
            press(&mut app, KeyCode::Esc, KeyModifiers::NONE, &mut out);
            assert!(
                matches!(
                    &app.overlay,
                    Some(Overlay::AgentPresets(view))
                        if view.quick.as_ref().is_some_and(|back| back.launch.pr.is_some())
                ),
                "{key:?}'s confirm backs out to the PR picker: {:?}",
                app.overlay
            );
        }

        let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
        terminal.draw(|f| ui::draw(f, &mut app)).unwrap();
        let list = match &app.overlay {
            Some(Overlay::AgentPresets(view)) => view.list_area,
            other => panic!("{other:?}"),
        };
        click(&mut app, list.x + 1, list.y + skip_row, &mut out);
        assert!(app.overlay.is_none(), "{:?}", app.overlay);
        let creates: Vec<&ClientRequest> = out
            .iter()
            .filter(|r| {
                matches!(
                    r,
                    ClientRequest::CreatePrAgent { .. } | ClientRequest::CreateAgent { .. }
                )
            })
            .collect();
        assert!(
            matches!(
                creates.as_slice(),
                [ClientRequest::CreatePrAgent {
                    pr_url,
                    head,
                    starting_prompt: Some(text),
                    ..
                }] if pr_url == "https://github.com/o/r/pull/7"
                    && head == "pr-7-head"
                    && text == "Review this pull request."
            ),
            "a PR create, never a plain one into the root: {out:?}"
        );
        let stand_in = app
            .tree
            .worktrees
            .iter()
            .find(|w| w.branch == "pr-7-head")
            .map(|w| w.id.clone())
            .expect("the head branch's stand-in row");
        assert_eq!(app.worktree_row_of(&stand_in), Some(2), "under the PR");

        // A non-skip row clicked: the box comes back for the PR.
        let mut app = App::new();
        seed_tree(&mut app);
        seed_open_prs(&mut app, &[(7, "Attach links")]);
        app.focus = Focus::Worktrees;
        app.sel_worktree = 1;
        let mut out = Vec::new();
        press(&mut app, KeyCode::Char('e'), KeyModifiers::NONE, &mut out);
        terminal.draw(|f| ui::draw(f, &mut app)).unwrap();
        let list = match &app.overlay {
            Some(Overlay::AgentPresets(view)) => view.list_area,
            other => panic!("{other:?}"),
        };
        click(&mut app, list.x + 1, list.y, &mut out);
        assert!(
            matches!(
                &app.overlay,
                Some(Overlay::Prompt(prompt))
                    if prompt.title == "Quick prompt · PR #7 · reviewer (claude · opus · high)"
            ),
            "{:?}",
            app.overlay
        );
        assert!(out.is_empty(), "{out:?}");
    });
}

#[test]
fn presets_click_row_launches_and_outside_closes() {
    with_seeded_presets(|| {
        let mut app = App::new();
        let mut out = Vec::new();
        open_presets(&mut app, &mut out);
        let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
        terminal.draw(|f| ui::draw(f, &mut app)).unwrap();
        let list = match &app.overlay {
            Some(Overlay::AgentPresets(view)) => view.list_area,
            other => panic!("{other:?}"),
        };
        assert!(list.width > 0, "drawn list rect recorded");
        click(&mut app, list.x + 1, list.y + 1, &mut out);
        match &app.overlay {
            Some(Overlay::Prompt(prompt)) => assert_eq!(prompt.title, "Task for scratch"),
            other => panic!("a click on the second row launches it, got {other:?}"),
        }
        press(&mut app, KeyCode::Esc, KeyModifiers::NONE, &mut out);
        terminal.draw(|f| ui::draw(f, &mut app)).unwrap();
        assert!(matches!(&app.overlay, Some(Overlay::AgentPresets(_))));
        click(&mut app, 0, 0, &mut out);
        assert!(app.overlay.is_none(), "a click outside closes the list");
        assert!(out.is_empty(), "{out:?}");
    });
}

#[test]
fn empty_presets_list_shows_the_hint() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("agent_presets.json");
    with_default_config(|| {
        crate::agent_presets::with_presets_path(path, || {
            let mut app = App::new();
            let mut out = Vec::new();
            open_presets(&mut app, &mut out);
            let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
            terminal.draw(|f| ui::draw(f, &mut app)).unwrap();
            let text = buffer_text(&terminal);
            assert!(
                text.contains("no presets yet — Ctrl+a creates one"),
                "{text}"
            );
            // Enter and Ctrl+e on nothing only nudge toward Ctrl+a.
            press(&mut app, KeyCode::Enter, KeyModifiers::NONE, &mut out);
            assert!(matches!(&app.overlay, Some(Overlay::AgentPresets(_))));
            assert_eq!(
                app.flash.as_deref(),
                Some("no preset selected — Ctrl+a creates one")
            );
            press(
                &mut app,
                KeyCode::Char('e'),
                KeyModifiers::CONTROL,
                &mut out,
            );
            assert!(matches!(&app.overlay, Some(Overlay::AgentPresets(_))));
        })
    });
}

// ---- ssh hosts picker ----

/// Route the host store at a temp file and pre-seed it with two
/// destinations, "old@one" first, then "new@two /srv/app" (so the list
/// reads newest-first: new@two, old@one).
fn with_seeded_hosts(f: impl FnOnce()) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("ssh_hosts.json");
    crate::hosts::with_hosts_path(path, || {
        crate::hosts::record("old@one", None);
        crate::hosts::record("new@two", Some("/srv/app"));
        f();
    });
}

#[test]
fn shift_h_opens_hosts_picker_newest_first() {
    with_seeded_hosts(|| {
        let mut app = App::new();
        let mut out = Vec::new();
        press(&mut app, KeyCode::Char('H'), KeyModifiers::SHIFT, &mut out);
        let Some(Overlay::Hosts(view)) = &app.overlay else {
            panic!(
                "shift+h should open the hosts picker, got {:?}",
                app.overlay
            );
        };
        assert_eq!(view.hosts.len(), 2);
        assert_eq!(view.hosts[0].host, "new@two", "most recent first");
        assert_eq!(view.hosts[0].path.as_deref(), Some("/srv/app"));
        assert_eq!(view.hosts[1].host, "old@one");
        assert_eq!(view.selected, 0);

        let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
        terminal.draw(|f| ui::draw(f, &mut app)).unwrap();
        let text = buffer_text(&terminal);
        assert!(text.contains("SSH Hosts"), "title rendered:\n{text}");
        assert!(text.contains("new@two"), "hosts rendered:\n{text}");
        assert!(text.contains("/srv/app"), "start dir rendered:\n{text}");
        assert!(text.contains("just now"), "ago label rendered:\n{text}");

        press(&mut app, KeyCode::Esc, KeyModifiers::NONE, &mut out);
        assert!(app.overlay.is_none(), "Esc closes the picker");
        assert!(!app.should_quit);
    });
}

#[test]
fn hosts_enter_quits_with_the_selected_destination() {
    with_seeded_hosts(|| {
        let mut app = App::new();
        let mut out = Vec::new();
        press(&mut app, KeyCode::Char('H'), KeyModifiers::SHIFT, &mut out);
        press(&mut app, KeyCode::Char('j'), KeyModifiers::NONE, &mut out);
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE, &mut out);
        assert!(app.should_quit, "Enter hands off by quitting");
        assert!(app.overlay.is_none());
        let entry = app.pending_ssh.as_ref().expect("handoff target set");
        assert_eq!(entry.host, "old@one");
        assert_eq!(entry.path, None);
    });
}

#[test]
fn hosts_d_removes_the_entry_and_persists() {
    with_seeded_hosts(|| {
        let mut app = App::new();
        let mut out = Vec::new();
        press(&mut app, KeyCode::Char('H'), KeyModifiers::SHIFT, &mut out);
        press(&mut app, KeyCode::Char('d'), KeyModifiers::NONE, &mut out);
        match &app.overlay {
            Some(Overlay::Hosts(view)) => {
                assert_eq!(view.hosts.len(), 1, "row dropped in place");
                assert_eq!(view.hosts[0].host, "old@one");
                assert_eq!(view.selected, 0, "cursor clamped");
            }
            other => panic!("picker should stay open, got {other:?}"),
        }
        let left = crate::hosts::load();
        assert_eq!(left.len(), 1, "removal reached the store");
        assert_eq!(left[0].host, "old@one");
    });
}

#[test]
fn hosts_click_on_a_row_connects_and_outside_closes() {
    with_seeded_hosts(|| {
        let mut app = App::new();
        let mut out = Vec::new();
        press(&mut app, KeyCode::Char('H'), KeyModifiers::SHIFT, &mut out);
        // Draw once so the modal writes back its hit-test rects.
        let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
        terminal.draw(|f| ui::draw(f, &mut app)).unwrap();
        let list = match &app.overlay {
            Some(Overlay::Hosts(view)) => view.list_area,
            other => panic!("picker open, got {other:?}"),
        };
        // Click the second row: connect to it.
        handle_mouse(
            &mut app,
            mev(
                MouseEventKind::Down(MouseButton::Left),
                list.x + 1,
                list.y + 1,
            ),
            &mut out,
        );
        assert!(app.should_quit, "click connects");
        assert_eq!(app.pending_ssh.as_ref().unwrap().host, "old@one");

        // Reopened, a click outside the modal closes it.
        app.should_quit = false;
        app.pending_ssh = None;
        press(&mut app, KeyCode::Char('H'), KeyModifiers::SHIFT, &mut out);
        terminal.draw(|f| ui::draw(f, &mut app)).unwrap();
        handle_mouse(
            &mut app,
            mev(MouseEventKind::Down(MouseButton::Left), 0, 0),
            &mut out,
        );
        assert!(app.overlay.is_none(), "outside click closes");
        assert!(!app.should_quit);
    });
}

// ---- a click outside any modal dismisses it ----

/// Draw once so the modal writes back its hit-test rect, and return it.
/// Every variant is inset from the frame, so (0, 0) is always outside.
fn drawn_modal_area(app: &mut App) -> ratatui::layout::Rect {
    let mut terminal = Terminal::new(TestBackend::new(140, 40)).unwrap();
    terminal.draw(|f| ui::draw(f, app)).unwrap();
    let overlay = app.overlay.as_ref().expect("no modal open");
    let area = crate::overlay_close::overlay_area(overlay);
    assert!(area.width > 0 && area.x > 0 && area.y > 0, "{area:?}");
    area
}

fn click(app: &mut App, column: u16, row: u16, out: &mut Vec<ClientRequest>) {
    handle_mouse(
        app,
        mev(MouseEventKind::Down(MouseButton::Left), column, row),
        out,
    );
}

// ---- the FOLLOW-UP COMPOSER ----

/// One live agent under the cursor of a focused SESSIONS PANEL.
fn follow_up_app() -> App {
    let mut app = App::new();
    seed_tree(&mut app);
    app.focus = Focus::Sessions;
    app.sel_session = 0;
    app
}

/// What the open FOLLOW-UP box holds — the modal the grid's `Space`
/// opens over it.
fn follow_up_text(app: &App) -> Option<String> {
    match &app.overlay {
        Some(Overlay::Prompt(p)) if matches!(p.kind, PromptKind::FollowUp { .. }) => {
            Some(p.input.as_str().to_string())
        }
        _ => None,
    }
}

/// A turn with line breaks in it crosses as a BRACKETED PASTE, so the
/// CLI takes it as one block instead of auto-indenting it to mush.
#[test]
fn a_multi_line_turn_goes_as_a_bracketed_paste() {
    let mut app = follow_up_app();
    let mut out = Vec::new();
    press(&mut app, KeyCode::Char(' '), KeyModifiers::NONE, &mut out);
    press(&mut app, KeyCode::Char('a'), KeyModifiers::NONE, &mut out);
    // Shift+Enter breaks the line rather than sending.
    press(&mut app, KeyCode::Enter, KeyModifiers::SHIFT, &mut out);
    press(&mut app, KeyCode::Char('b'), KeyModifiers::NONE, &mut out);
    assert_eq!(follow_up_text(&app).as_deref(), Some("a\nb"));
    out.clear();

    press(&mut app, KeyCode::Enter, KeyModifiers::NONE, &mut out);
    let sent = out
        .iter()
        .find_map(|r| match r {
            ClientRequest::Input { data, .. } if data != b"\r" => Some(data.clone()),
            _ => None,
        })
        .expect("the turn was sent");
    assert_eq!(sent, bracketed("a\nb"));
}

/// A session with no live PTY is booted first and the box kept as it
/// is: the daemon drops Input for a session it has not spawned, and
/// the CLI that attach starts is seconds from reading anything.
#[test]
fn a_cold_session_is_booted_instead_of_typed_at() {
    let mut app = follow_up_app();
    app.tree.agents[0].alive = false;
    let mut out = Vec::new();
    press(&mut app, KeyCode::Char(' '), KeyModifiers::NONE, &mut out);
    press(&mut app, KeyCode::Char('x'), KeyModifiers::NONE, &mut out);
    out.clear();
    press(&mut app, KeyCode::Enter, KeyModifiers::NONE, &mut out);

    assert_eq!(
        follow_up_text(&app).as_deref(),
        Some("x"),
        "the box and what is in it stay"
    );
    assert!(
        !out.iter().any(|r| matches!(r, ClientRequest::Input { .. })),
        "nothing was typed into a session that is not up: {out:?}"
    );
    assert!(
        out.iter()
            .any(|r| matches!(r, ClientRequest::Attach { .. })),
        "it was booted: {out:?}"
    );
    assert!(app.flash.as_deref().is_some_and(|f| f.contains("starting")));
}

/// The FOLLOW-UP box is a modal over the grid, so Esc is the way out
/// of it and the cards have the keys again afterwards.
#[test]
fn esc_closes_the_follow_up_box_and_the_grid_has_the_keys() {
    let mut app = follow_up_app();
    let mut out = Vec::new();
    press(&mut app, KeyCode::Char(' '), KeyModifiers::NONE, &mut out);
    assert!(follow_up_text(&app).is_some(), "the box is up");
    press(&mut app, KeyCode::Esc, KeyModifiers::NONE, &mut out);
    assert!(app.overlay.is_none(), "{:?}", app.overlay);
    assert_eq!(app.focus, Focus::Sessions, "the cards have the keys");
}

/// A paste lands in the box, line breaks and all, rather than in the
/// pane behind it.
#[test]
fn a_paste_lands_in_the_open_box() {
    let mut app = follow_up_app();
    let mut out = Vec::new();
    press(&mut app, KeyCode::Char(' '), KeyModifiers::NONE, &mut out);
    dispatch_terminal_event(&mut app, Event::Paste("one\ntwo".into()), &mut out);
    assert_eq!(follow_up_text(&app).as_deref(), Some("one\ntwo"));
}

/// A screenshot dragged from its floating thumbnail onto the box lands
/// as a copy in the ATTACHMENTS DIR under a plain name, and that copy
/// is what the turn names: macOS deletes the thumbnail's file soon
/// after the drop, and the agent types its U+202F back as a space.
#[test]
fn a_dropped_screenshot_reaches_the_agent_as_a_copy_it_can_find() {
    let root = tempfile::tempdir().unwrap();
    let thumbnails = root
        .path()
        .join("T/TemporaryItems/NSIRD_screencaptureui_LySI4r");
    std::fs::create_dir_all(&thumbnails).unwrap();
    let shot = thumbnails.join("Screenshot 2026-09-21 at 11.13.58\u{202f}PM.png");
    std::fs::write(&shot, b"png").unwrap();
    // The way Ghostty pastes a drop: a shell-escaped path.
    let dropped = shot.to_string_lossy().replace(' ', "\\ ");
    let mut app = follow_up_app();
    app.attachments_dir = Some(root.path().join("attachments"));
    let mut out = Vec::new();

    press(&mut app, KeyCode::Char(' '), KeyModifiers::NONE, &mut out);
    dispatch_terminal_event(&mut app, Event::Paste(dropped), &mut out);
    std::fs::remove_file(&shot).unwrap();
    let text = follow_up_text(&app).expect("the box is up");
    let copy = root
        .path()
        .join("attachments/Screenshot-2026-09-21-at-11.13.58-PM.png");
    assert!(text.contains(&*copy.to_string_lossy()), "{text:?}");
    assert!(copy.is_file(), "the copy outlives the thumbnail");

    out.clear();
    press(&mut app, KeyCode::Enter, KeyModifiers::NONE, &mut out);
    let sent = out
        .iter()
        .find_map(|r| match r {
            ClientRequest::Input { data, .. } if data != b"\r" => Some(data.clone()),
            _ => None,
        })
        .expect("the turn was sent");
    assert_eq!(String::from_utf8(sent).unwrap(), text);
}

/// Without an ATTACHMENTS DIR (every other unit test) a drop is pasted
/// as it came, so no test copies into the real user's data dir.
#[test]
fn with_no_attachments_dir_a_drop_is_pasted_as_it_came() {
    let root = tempfile::tempdir().unwrap();
    let thumbnails = root.path().join("TemporaryItems");
    std::fs::create_dir_all(&thumbnails).unwrap();
    let shot = thumbnails.join("shot.png");
    std::fs::write(&shot, b"png").unwrap();
    let dropped = shot.to_string_lossy().into_owned();
    let mut app = follow_up_app();
    let mut out = Vec::new();

    press(&mut app, KeyCode::Char(' '), KeyModifiers::NONE, &mut out);
    dispatch_terminal_event(&mut app, Event::Paste(dropped.clone()), &mut out);

    assert_eq!(follow_up_text(&app), Some(dropped));
}

// ---- INPUT PARITY: a row chosen by the pointer is the row chosen by key ----
//
// `event_loop::activate` is the rule; these are its teeth. Each builds
// the same app twice, chooses the same row once with the keyboard and
// once with the mouse, and compares everything a user could tell the
// two apart by: the modal left on screen, the cursors, FOCUS, the pane,
// the flash, and every request sent to the DAEMON.

/// Everything observable about where an input left the app.
fn ui_digest(app: &App, out: &[ClientRequest]) -> String {
    let overlay = match &app.overlay {
        None => "none".to_string(),
        Some(Overlay::Prompt(p)) => format!("prompt[{}|{}]", p.title, p.input.as_str()),
        Some(Overlay::Menu(m)) => format!(
            "menu[{:?}|{}]",
            m.title,
            m.items
                .iter()
                .map(|item| item.label.as_str())
                .collect::<Vec<_>>()
                .join(",")
        ),
        Some(Overlay::FileTabs(t)) => format!(
            "filetabs[{}]",
            t.tabs
                .iter()
                .map(|tab| tab.label.as_str())
                .collect::<Vec<_>>()
                .join(",")
        ),
        Some(Overlay::Settings(v)) => format!(
            "settings[tab {} row {} on_tabs {} capture {} notice {:?}]",
            v.tab,
            v.selected,
            v.on_tabs,
            v.capture.is_some(),
            v.notice.as_ref().map(|(text, _)| text.as_str())
        ),
        Some(other) => format!("{:?}", std::mem::discriminant(other)),
    };
    format!(
        "overlay={overlay} focus={:?} cursors=({},{},{}) pane={:?} locked={} editor={} \
             flash={:?} quit={} out={out:?}",
        app.focus,
        app.sel_project,
        app.sel_worktree,
        app.sel_session,
        app.term.as_ref().map(|t| t.sref.clone()),
        app.term_locked,
        app.vim.is_some(),
        app.flash,
        app.should_quit,
    )
}

/// Draw, then press `button` on the first cell registered for `target`.
fn click_target(
    app: &mut App,
    target: HitTarget,
    button: MouseButton,
    out: &mut Vec<ClientRequest>,
) {
    let mut terminal = Terminal::new(TestBackend::new(160, 40)).unwrap();
    terminal.draw(|f| ui::draw(f, app)).unwrap();
    let rect = app
        .hits
        .iter()
        .find(|(_, hit)| *hit == target)
        .map(|(rect, _)| *rect)
        .unwrap_or_else(|| panic!("{target:?} is not on screen: {:?}", app.hits));
    // The middle of the row: its first cell is a splitter's grab zone.
    let (x, y) = (rect.x + rect.width / 2, rect.y);
    assert_eq!(app.hit_at(x, y), Some(target), "the click lands on it");
    handle_mouse(app, mev(MouseEventKind::Down(button), x, y), out);
}

/// The drawn list box of the open modal.
fn drawn_list_area(app: &mut App) -> ratatui::layout::Rect {
    let mut terminal = Terminal::new(TestBackend::new(160, 40)).unwrap();
    terminal.draw(|f| ui::draw(f, app)).unwrap();
    match &app.overlay {
        Some(Overlay::Palette(v)) => v.list_area,
        Some(Overlay::Files(v)) => v.list_area,
        Some(Overlay::AgentPresets(v)) => v.list_area,
        Some(Overlay::Menu(v)) => {
            // A menu's rows sit inside its border.
            ratatui::layout::Rect::new(
                v.area.x + 1,
                v.area.y + 1,
                v.area.width.saturating_sub(2),
                v.area.height.saturating_sub(2),
            )
        }
        other => panic!("no list to click in {other:?}"),
    }
}

/// A checkout beside the root, with a session of its own, and the
/// cursor left on the root's.
fn parity_tree() -> App {
    use nebula_core::{Agent, AgentStatus, Entity};
    let mut app = App::new();
    app.body_area = ratatui::layout::Rect::new(0, 0, 40, 35);
    seed_tree(&mut app);
    seed_feat_worktree(&mut app, "w2", "feat");
    hse(
        &mut app,
        ServerEvent::EntityUpserted {
            entity: Entity::Agent(Agent {
                id: AgentId("a9".into()),
                worktree_id: WorktreeId("w2".into()),
                name: "feat-agent".into(),
                status: AgentStatus::Finished,
                archived: false,
                archived_at: 0,
                unseen: false,
                kind: AgentKind::Claude,
                custom_harness: None,
                model: None,
                effort: None,
                session_id: None,
                cloud_session_id: None,
                sort_order: 0,
                status_changed_at: 1,
                alive: true,
                issue_url: None,
                recent_prompts: Vec::new(),
            }),
        },
    );
    app.focus = Focus::Worktrees;
    let root = app
        .worktree_row_of(&WorktreeId("w1".into()))
        .expect("the root is a row");
    let mut out = Vec::new();
    select_worktree_row(&mut app, root, &mut out);
    app.sel_worktree = root;
    app
}

/// A CONTEXT MENU row is its hotkey: **Delete worktree** is `d` on the
/// checkout — the same confirm, which says how many sessions go down
/// with it (the menu's own copy had lost that) — **Attach** is Enter on
/// the session, **Unarchive** is `u`.
#[test]
fn a_context_menu_row_is_its_hotkey() {
    with_default_config(|| {
        let confirm = |app: &App| match &app.overlay {
            Some(Overlay::Confirm(c)) => format!("{}|{}|{:?}", c.title, c.message, c.action),
            other => format!("{other:?}"),
        };
        let on_feat = || {
            let mut app = parity_tree();
            let mut out = Vec::new();
            let row = app
                .worktree_row_of(&WorktreeId("w2".into()))
                .expect("feat is a row");
            select_worktree_row(&mut app, row, &mut out);
            app.focus = Focus::Worktrees;
            app
        };

        let mut by_key = on_feat();
        let mut out = Vec::new();
        press(
            &mut by_key,
            KeyCode::Char('d'),
            KeyModifiers::NONE,
            &mut out,
        );
        let mut by_menu = on_feat();
        run_menu_action(
            &mut by_menu,
            MenuAction::DeleteWorktree(WorktreeId("w2".into())),
            &mut out,
        );
        assert_eq!(confirm(&by_menu), confirm(&by_key));
        assert!(
            confirm(&by_menu).contains("1 session(s) will be killed"),
            "{}",
            confirm(&by_menu)
        );

        // The root is never deleted, by either route.
        let mut by_menu = parity_tree();
        run_menu_action(
            &mut by_menu,
            MenuAction::DeleteWorktree(WorktreeId("w1".into())),
            &mut out,
        );
        assert!(by_menu.overlay.is_none(), "{:?}", by_menu.overlay);
        assert_eq!(
            by_menu.flash.as_deref(),
            Some("cannot delete the main checkout")
        );

        // Attach: Enter on the row.
        let on_session = || {
            let mut app = on_feat();
            app.focus = Focus::Sessions;
            app
        };
        let mut by_key = on_session();
        let mut key_out = Vec::new();
        press(
            &mut by_key,
            KeyCode::Enter,
            KeyModifiers::NONE,
            &mut key_out,
        );
        let mut by_menu = on_session();
        let mut menu_out = Vec::new();
        run_menu_action(
            &mut by_menu,
            MenuAction::Attach(SessionRef::Agent(AgentId("a9".into()))),
            &mut menu_out,
        );
        assert_eq!(ui_digest(&by_menu, &menu_out), ui_digest(&by_key, &key_out));
    });
}

/// A click into the pane is Enter on it: FOCUS, the input lock, and the
/// attach a sweep of the cursor had left waiting on its debounce sent
/// at once — keystrokes are about to need it. The click used to skip
/// that last part.
#[test]
fn a_click_into_the_pane_is_enter_on_it() {
    with_default_config(|| {
        let sweep = || {
            let mut app = parity_tree();
            app.focus = Focus::Sessions;
            seed_second_agent(&mut app, nebula_core::AgentStatus::Finished);
            // Reaped: the one kind of session a sweep debounces, since
            // attaching it boots a CLI.
            for agent in &mut app.tree.agents {
                agent.alive = agent.id != AgentId("a2".into());
            }
            let mut out = Vec::new();
            // Onto the reaped session: shown, the attach still waiting.
            // `h` along the band onto it: the newest card leads the
            // row, so the reaped one sits left of the cursor's.
            let reaped = AgentId("a2".into());
            for _ in 0..crate::launcher::rows(&app).len() {
                if app.selected_session().map(|a| a.id) == Some(reaped.clone()) {
                    break;
                }
                press(&mut app, KeyCode::Char('h'), KeyModifiers::NONE, &mut out);
            }
            assert_eq!(
                app.selected_session().map(|a| a.id),
                Some(reaped),
                "the walk landed on the reaped session"
            );
            assert!(app.pending_attach.is_some(), "the sweep debounces");
            out.clear();
            (app, out)
        };

        let (mut by_keys, mut keys_out) = sweep();
        // Drawn, as the clicked one is: the pane's size rides the Attach.
        let mut terminal = Terminal::new(TestBackend::new(160, 40)).unwrap();
        terminal.draw(|f| ui::draw(f, &mut by_keys)).unwrap();
        enter_terminal_pane(&mut by_keys, &mut keys_out);

        let (mut by_mouse, mut mouse_out) = sweep();
        click_target(
            &mut by_mouse,
            HitTarget::TerminalPane,
            MouseButton::Left,
            &mut mouse_out,
        );
        // The press arms a drag-selection, which a key has no part in.
        by_mouse.term_selection = None;

        assert!(
            mouse_out
                .iter()
                .any(|r| matches!(r, ClientRequest::Attach { .. })),
            "the waiting attach goes out with the click: {mouse_out:?}"
        );
        assert!(by_mouse.pending_attach.is_none());
        assert_eq!(
            ui_digest(&by_mouse, &mouse_out),
            ui_digest(&by_keys, &keys_out)
        );
    });
}

/// A click on a row of a picking list is Enter with the cursor on that
/// row: the `/` palette, the NEW SESSION PICKER's menu, the AGENT
/// PRESETS list in both of its modes.
#[test]
fn a_click_on_a_list_row_is_enter_on_it() {
    with_seeded_presets(|| {
        type Open = fn(&mut App, &mut Vec<ClientRequest>);
        let palette: Open = |app, out| {
            press(app, KeyCode::Char('/'), KeyModifiers::NONE, out);
        };
        let new_session: Open = |app, _out| {
            app.focus = Focus::Sessions;
            open_picker(app);
        };
        let presets: Open = |app, out| {
            app.focus = Focus::Sessions;
            press(app, KeyCode::Char('e'), KeyModifiers::NONE, out);
        };
        let pr_presets: Open = |app, out| {
            seed_open_prs(app, &[(7, "Attach links")]);
            app.sel_worktree = app.open_pr_row_of(&pr_url(7)).expect("#7 is a row");
            press(app, KeyCode::Char('e'), KeyModifiers::NONE, out);
        };
        let surfaces: [(&str, Open); 4] = [
            ("palette", palette),
            ("new session picker", new_session),
            ("presets", presets),
            ("PR presets", pr_presets),
        ];
        for (name, open) in surfaces {
            for row in 0..2u16 {
                let mut by_keys = parity_tree();
                let mut keys_out = Vec::new();
                open(&mut by_keys, &mut keys_out);
                // Drawn, as the clicked one is: the pane's size rides
                // every Attach.
                drawn_list_area(&mut by_keys);
                // Letters type ahead in the palette and the preset
                // lists, so ↓ moves there.
                let down = if name == "palette" || name.ends_with("presets") {
                    KeyCode::Down
                } else {
                    KeyCode::Char('j')
                };
                // The palette opens with its cursor on the top
                // session, under the project header drawn above it:
                // walk up to the first row before counting down.
                if let Some(Overlay::Palette(p)) = &by_keys.overlay {
                    for _ in 0..p.selected {
                        press(&mut by_keys, KeyCode::Up, KeyModifiers::NONE, &mut keys_out);
                    }
                }
                for _ in 0..row {
                    press(&mut by_keys, down, KeyModifiers::NONE, &mut keys_out);
                }
                press(
                    &mut by_keys,
                    KeyCode::Enter,
                    KeyModifiers::NONE,
                    &mut keys_out,
                );

                let mut by_mouse = parity_tree();
                let mut mouse_out = Vec::new();
                open(&mut by_mouse, &mut mouse_out);
                let list = drawn_list_area(&mut by_mouse);
                click(&mut by_mouse, list.x + 1, list.y + row, &mut mouse_out);

                assert_eq!(
                    ui_digest(&by_mouse, &mouse_out),
                    ui_digest(&by_keys, &keys_out),
                    "{name}, row {row}"
                );
            }
        }
    });
}

/// A click on a file finder row is Enter on it — the FILE TABS reader
/// for a markdown file. The click used to call the editor directly, and
/// so missed the reader when Enter learned it.
#[test]
fn a_click_on_a_finder_row_is_enter_on_it() {
    let dir = tempfile::tempdir().unwrap();
    let repo = test_repo(&dir);
    std::fs::write(repo.join("notes.md"), "# Notes\n\n- one\n").unwrap();
    let open = || {
        let mut app = App::new();
        seed_repo_tree(&mut app, &repo);
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        app.vim_tx = Some(tx);
        let mut out = Vec::new();
        press(&mut app, KeyCode::Char('f'), KeyModifiers::NONE, &mut out);
        for c in ['n', 'o', 't'] {
            press(&mut app, KeyCode::Char(c), KeyModifiers::NONE, &mut out);
        }
        assert_eq!(finder(&app).selected_path(), Some("notes.md"));
        (app, out)
    };

    let (mut by_keys, mut keys_out) = open();
    press(
        &mut by_keys,
        KeyCode::Enter,
        KeyModifiers::NONE,
        &mut keys_out,
    );

    let (mut by_mouse, mut mouse_out) = open();
    let list = drawn_list_area(&mut by_mouse);
    click(&mut by_mouse, list.x + 1, list.y, &mut mouse_out);

    assert!(
        matches!(&by_mouse.overlay, Some(Overlay::FileTabs(_))),
        "the reader, not the editor: {:?}",
        by_mouse.overlay
    );
    assert!(by_mouse.vim.is_none());
    assert_eq!(
        ui_digest(&by_mouse, &mouse_out),
        ui_digest(&by_keys, &keys_out)
    );
}

/// A second click on a SETTINGS OVERLAY row is Enter on it, and a click
/// on a tab is that tab's digit: the mouse names the keys' own
/// commands (`run_settings_cmd`) instead of changing the view itself.
#[test]
fn a_second_click_on_a_settings_row_is_enter_on_it() {
    with_default_config(|| {
        let open = || {
            let mut app = App::new();
            seed_tree(&mut app);
            let mut out = Vec::new();
            press(&mut app, KeyCode::Char('s'), KeyModifiers::NONE, &mut out);
            let mut terminal = Terminal::new(TestBackend::new(160, 40)).unwrap();
            terminal.draw(|f| ui::draw(f, &mut app)).unwrap();
            (app, out)
        };
        // The first clickable row of the open tab, and its screen line.
        let row_line = |app: &App| {
            let view = settings(app).expect("settings are open");
            let rows = crate::config::settings_rows(view.tab);
            let line = rows
                .iter()
                .position(|r| r.index().is_some())
                .expect("a tab has a row");
            let body = view.body_area;
            (body.x + 1, body.y + (line - view.first_row) as u16)
        };

        let (mut by_keys, mut keys_out) = open();
        let (x, y) = row_line(&by_keys);
        click(&mut by_keys, x, y, &mut keys_out);
        if settings(&by_keys).is_some_and(|v| v.selected != 0) {
            panic!("the first row is row 0");
        }
        press(
            &mut by_keys,
            KeyCode::Enter,
            KeyModifiers::NONE,
            &mut keys_out,
        );

        let (mut by_mouse, mut mouse_out) = open();
        let (x, y) = row_line(&by_mouse);
        click(&mut by_mouse, x, y, &mut mouse_out);
        // Already on it after the first click: the second is Enter.
        click(&mut by_mouse, x, y, &mut mouse_out);

        assert_eq!(
            ui_digest(&by_mouse, &mouse_out),
            ui_digest(&by_keys, &keys_out)
        );

        // A click on the second tab is `2`, with the cursor in its list.
        let (mut by_keys, mut keys_out) = open();
        press(
            &mut by_keys,
            KeyCode::Char('2'),
            KeyModifiers::NONE,
            &mut keys_out,
        );
        let (mut by_mouse, mut mouse_out) = open();
        let (area, hits) = {
            let view = settings(&by_mouse).unwrap();
            (view.area, view.tab_hits.clone())
        };
        let (x0, _) = hits[1];
        click(&mut by_mouse, x0, area.y + 1, &mut mouse_out);
        let tab = |app: &App| settings(app).map(|v| (v.tab, v.selected));
        assert_eq!(tab(&by_mouse), tab(&by_keys));
    });
}

#[test]
fn help_click_outside_closes_and_inside_stays() {
    let mut app = App::new();
    let mut out = Vec::new();
    press(&mut app, KeyCode::Char('?'), KeyModifiers::NONE, &mut out);
    let area = drawn_modal_area(&mut app);
    click(&mut app, area.x + 2, area.y + 2, &mut out);
    assert!(
        matches!(app.overlay, Some(Overlay::Help(_))),
        "inside click keeps it"
    );
    click(&mut app, 0, 0, &mut out);
    assert!(app.overlay.is_none(), "outside click closes");
    assert!(app.dirty);
}

#[test]
fn confirm_click_outside_cancels_without_confirming() {
    let mut app = App::new();
    seed_tree(&mut app);
    seed_link(&mut app, "https://example.dev/spec");
    let mut out = Vec::new();
    press(&mut app, KeyCode::Char('D'), KeyModifiers::NONE, &mut out);
    assert!(
        matches!(app.overlay, Some(Overlay::Confirm(_))),
        "{:?}",
        app.overlay
    );
    let area = drawn_modal_area(&mut app);
    out.clear();
    click(&mut app, area.x + 2, area.y + 1, &mut out);
    assert!(
        matches!(app.overlay, Some(Overlay::Confirm(_))),
        "inside click keeps it"
    );
    click(&mut app, 0, 0, &mut out);
    assert!(app.overlay.is_none(), "outside click cancels");
    assert!(out.is_empty(), "nothing was confirmed: {out:?}");
}

/// The outside click is Esc, not a bare close: backing out of the
/// settings-reset confirm lands back in the settings overlay.
#[test]
fn confirm_click_outside_lands_where_esc_would() {
    with_default_config(|| {
        let mut app = App::new();
        let mut out = Vec::new();
        press(&mut app, KeyCode::Char('s'), KeyModifiers::NONE, &mut out);
        press(&mut app, KeyCode::Tab, KeyModifiers::NONE, &mut out);
        press(&mut app, KeyCode::Char('R'), KeyModifiers::SHIFT, &mut out);
        assert!(
            matches!(app.overlay, Some(Overlay::Confirm(_))),
            "{:?}",
            app.overlay
        );
        drawn_modal_area(&mut app);
        click(&mut app, 0, 0, &mut out);
        assert!(
            matches!(app.overlay, Some(Overlay::Settings(_))),
            "back to the overlay, not the panels: {:?}",
            app.overlay
        );
    });
}

#[test]
fn prompt_click_outside_abandons_it() {
    let mut app = App::new();
    let mut out = Vec::new();
    press(&mut app, KeyCode::Char('n'), KeyModifiers::NONE, &mut out);
    assert!(
        matches!(app.overlay, Some(Overlay::Prompt(_))),
        "{:?}",
        app.overlay
    );
    let area = drawn_modal_area(&mut app);
    click(&mut app, area.x + 2, area.y + 1, &mut out);
    assert!(
        matches!(app.overlay, Some(Overlay::Prompt(_))),
        "inside click keeps it"
    );
    click(&mut app, 0, 0, &mut out);
    assert!(app.overlay.is_none(), "outside click abandons the prompt");
}

#[test]
fn diff_click_outside_closes_and_inside_stays() {
    let mut app = App::new();
    seed_tree(&mut app);
    app.overlay = Some(Overlay::Diff(fake_diff_view(10)));
    let mut out = Vec::new();
    let area = drawn_modal_area(&mut app);
    click(
        &mut app,
        area.x + area.width / 2,
        area.y + area.height / 2,
        &mut out,
    );
    assert!(
        matches!(app.overlay, Some(Overlay::Diff(_))),
        "inside click keeps it"
    );
    click(&mut app, 0, 0, &mut out);
    assert!(app.overlay.is_none(), "outside click closes");
}

// ---- …and lands its focus on the panel it hit ----

/// Landing in the pane is the pane's own click: focus and the input
/// lock, so what the user types next reaches the agent.
#[test]
fn click_outside_into_the_pane_focuses_and_locks_it() {
    let mut app = App::new();
    seed_tree(&mut app);
    app.term = Some(AttachedTerm::new(
        SessionRef::Agent(AgentId("a1".into())),
        40,
        10,
    ));
    app.focus = Focus::Sessions;
    let mut out = Vec::new();
    press(&mut app, KeyCode::Char('?'), KeyModifiers::NONE, &mut out);
    let modal = drawn_modal_area(&mut app);
    // The modal is centred over the pane; its bottom-right cell is not.
    let pane = app.term_area;
    let (x, y) = (pane.x + pane.width - 1, pane.y + pane.height - 1);
    assert!(
        !modal.contains(ratatui::layout::Position::new(x, y)),
        "the pane's corner {x},{y} is beside the modal {modal:?}"
    );
    assert!(!app.term_locked);
    click(&mut app, x, y, &mut out);
    assert!(app.overlay.is_none(), "outside click closes");
    assert_eq!(app.focus, Focus::Terminal);
    assert!(app.term_locked, "a click into the pane locks input");
}

// ---- every modal has all three exits ----

/// Every `Overlay` variant, the opener that puts it on screen, and where
/// its two staged exits land: `None` for the panels, `Some(label)` for
/// the modal it backs out to. The HARDWIRED UNLOCK always lands on the
/// panels, so it needs no column.
///
/// `overlay_label` below is an exhaustive match, so a new variant cannot
/// be added without landing in this table — which is what keeps the
/// three-exits contract from drifting as overlays come and go.
#[allow(clippy::type_complexity)]
fn every_overlay() -> Vec<(&'static str, fn(&mut App), Option<&'static str>)> {
    vec![
        (
            "Menu",
            |app| {
                seed_tree(app);
                app.focus = Focus::Sessions;
                open_row_menu(app);
            },
            None,
        ),
        (
            "Confirm",
            |app| {
                seed_tree(app);
                seed_link(app, "https://example.dev/spec");
                press(app, KeyCode::Char('D'), KeyModifiers::NONE, &mut Vec::new());
            },
            None,
        ),
        (
            "Prompt",
            |app| press(app, KeyCode::Char('n'), KeyModifiers::NONE, &mut Vec::new()),
            None,
        ),
        (
            "Help",
            |app| press(app, KeyCode::Char('?'), KeyModifiers::NONE, &mut Vec::new()),
            None,
        ),
        (
            "Settings",
            |app| press(app, KeyCode::Char('s'), KeyModifiers::NONE, &mut Vec::new()),
            None,
        ),
        (
            "Diff",
            |app| {
                seed_tree(app);
                app.overlay = Some(Overlay::Diff(fake_diff_view(10)));
            },
            None,
        ),
        (
            "Palette",
            |app| {
                seed_tree(app);
                press(app, KeyCode::Char('/'), KeyModifiers::NONE, &mut Vec::new());
            },
            None,
        ),
        (
            "Files",
            |app| {
                app.overlay = Some(Overlay::Files(FileFinder::new(
                    "/tmp/demo".into(),
                    "main".into(),
                    "vi".into(),
                    vec!["src/main.rs".into(), "README.md".into()],
                )));
            },
            None,
        ),
        (
            "Grep",
            |app| {
                app.overlay = Some(Overlay::Grep(GrepView::new(
                    "/tmp/demo".into(),
                    "main".into(),
                    "vi".into(),
                )));
            },
            None,
        ),
        (
            "Tree",
            |app| {
                app.overlay = Some(Overlay::Tree(TreeBrowser::new(
                    "/tmp/demo".into(),
                    "main".into(),
                    "vi".into(),
                    vec!["src/main.rs".into(), "README.md".into()],
                )));
            },
            None,
        ),
        (
            "FileTabs",
            |app| {
                app.overlay = Some(Overlay::FileTabs(crate::file_tabs::FileTabsView::new(
                    "/tmp/demo".into(),
                    "vi".into(),
                    vec!["/tmp/demo/README.md".into()],
                )));
            },
            None,
        ),
        (
            "Metrics",
            |app| app.overlay = Some(Overlay::Metrics(MetricsView::new())),
            None,
        ),
        (
            "Issues",
            |app| {
                seed_tree(app);
                press(app, KeyCode::Char('i'), KeyModifiers::NONE, &mut Vec::new());
            },
            None,
        ),
        (
            "PullRequests",
            |app| {
                seed_tree(app);
                press(app, KeyCode::Char('v'), KeyModifiers::NONE, &mut Vec::new());
            },
            None,
        ),
        (
            "BranchSwitch",
            |app| {
                seed_tree(app);
                press(app, KeyCode::Char('c'), KeyModifiers::NONE, &mut Vec::new());
            },
            None,
        ),
        (
            "Hosts",
            |app| {
                app.overlay = Some(Overlay::Hosts(crate::app::HostsView::new(vec![
                    crate::hosts::HostEntry {
                        host: "old@one".into(),
                        path: None,
                        last_used_ms: 1,
                    },
                ])));
            },
            None,
        ),
        (
            "AgentPresets",
            |app| {
                crate::preset_overlays::reopen_agent_presets(
                    app,
                    nebula_core::WorktreeId("w1".into()),
                    0,
                );
            },
            None,
        ),
        (
            // The only staged landing: the editor backs out to the list
            // it was opened from, unsaved, exactly as its own Esc does.
            "AgentPresetEditor",
            |app| {
                crate::preset_overlays::open_agent_preset_editor(
                    app,
                    nebula_core::WorktreeId("w1".into()),
                    None,
                    None,
                );
            },
            Some("AgentPresets"),
        ),
        (
            // Owes the LAUNCHER VIEW's box back, as a picker opened from
            // the QUICK PROMPT does.
            "ProjectPicker",
            |app| {
                seed_tree(app);
                press(app, KeyCode::Char('p'), KeyModifiers::NONE, &mut Vec::new());
                press(
                    app,
                    KeyCode::Char('p'),
                    KeyModifiers::CONTROL,
                    &mut Vec::new(),
                );
            },
            Some("Prompt"),
        ),
    ]
}

fn file_tabs(app: &App) -> &crate::file_tabs::FileTabsView {
    match &app.overlay {
        Some(Overlay::FileTabs(v)) => v,
        other => panic!("expected the file tabs, got {other:?}"),
    }
}

fn preview_text(view: &crate::file_tabs::FileTabsView, line: usize) -> String {
    view.preview_lines[line]
        .iter()
        .map(|(_, t)| t.as_str())
        .collect()
}

/// `nebula open` in a session: the daemon's FilesOpened raises the FILE
/// TABS on the files, one tab each, the first previewed, cursor on the
/// strip — and → walks to the next file's preview.
#[test]
fn files_opened_event_raises_the_file_tabs_previewing_the_first() {
    with_default_config(|| {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a.md");
        let b = dir.path().join("b.rs");
        std::fs::write(&a, "# alpha\n").unwrap();
        std::fs::write(&b, "fn main() {}\n").unwrap();
        let mut app = App::new();
        let mut out = Vec::new();
        handle_server_event(
            &mut app,
            ServerEvent::FilesOpened {
                agent: AgentId("a1".into()),
                root: dir.path().to_path_buf(),
                paths: vec![a, b],
            },
            &mut out,
        );
        let view = file_tabs(&app);
        let labels: Vec<&str> = view.tabs.iter().map(|t| t.label.as_str()).collect();
        assert_eq!(labels, ["a.md", "b.rs"]);
        assert!(view.on_tabs, "opens on the strip");
        assert!(view.preview_is_file);
        assert_eq!(preview_text(view, 0), "# alpha");
        assert!(app.dirty);

        press(&mut app, KeyCode::Right, KeyModifiers::NONE, &mut out);
        let view = file_tabs(&app);
        assert_eq!(view.tab, 1);
        assert_eq!(preview_text(view, 0), "fn main() {}");
        assert!(out.is_empty(), "nothing goes to the daemon");
    });
}

/// The one modal where Ctrl+Q backs out a level: from the editor it
/// lands on the strip (editor killed, file re-read), from the preview
/// it lands on the strip, and only from the strip does it close.
#[test]
fn file_tabs_ctrl_q_steps_back_to_the_strip_before_closing() {
    with_default_config(|| {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a.md");
        std::fs::write(&a, "one\ntwo\nthree\n").unwrap();
        let mut app = App::new();
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        app.vim_tx = Some(tx);
        let mut out = Vec::new();
        // A shell stands in for vim (`sh +1 a.md` still spawns fine).
        app.overlay = Some(Overlay::FileTabs(crate::file_tabs::FileTabsView::new(
            dir.path().to_path_buf(),
            "/bin/sh".into(),
            vec![a],
        )));

        press(&mut app, KeyCode::Enter, KeyModifiers::NONE, &mut out);
        let vim = app.vim.as_ref().expect("Enter spawns the editor");
        assert!(vim.embedded, "embedded in the modal's body");
        assert!(
            matches!(&app.overlay, Some(Overlay::FileTabs(_))),
            "the tabs stay under the editor"
        );

        press(
            &mut app,
            KeyCode::Char('q'),
            KeyModifiers::CONTROL,
            &mut out,
        );
        assert!(app.vim.is_none(), "Ctrl+Q kills the editor");
        assert!(file_tabs(&app).on_tabs, "…and lands on the strip");

        press(&mut app, KeyCode::Down, KeyModifiers::NONE, &mut out);
        assert!(!file_tabs(&app).on_tabs, "↓ drops into the preview");
        press(
            &mut app,
            KeyCode::Char('q'),
            KeyModifiers::CONTROL,
            &mut out,
        );
        assert!(
            matches!(&app.overlay, Some(Overlay::FileTabs(v)) if v.on_tabs),
            "Ctrl+Q from the preview is the strip, not the panels"
        );

        press(
            &mut app,
            KeyCode::Char('q'),
            KeyModifiers::CONTROL,
            &mut out,
        );
        assert!(app.overlay.is_none(), "from the strip it closes");
    });
}

/// The variant's name. Exhaustive on purpose — see `every_overlay`.
fn overlay_label(overlay: &Overlay) -> &'static str {
    match overlay {
        Overlay::Menu(_) => "Menu",
        Overlay::Confirm(_) => "Confirm",
        Overlay::Prompt(_) => "Prompt",
        Overlay::Help(_) => "Help",
        Overlay::Settings(_) => "Settings",
        Overlay::Diff(_) => "Diff",
        Overlay::Palette(_) => "Palette",
        Overlay::Files(_) => "Files",
        Overlay::Grep(_) => "Grep",
        Overlay::Tree(_) => "Tree",
        Overlay::FileTabs(_) => "FileTabs",
        Overlay::Metrics(_) => "Metrics",
        Overlay::Hosts(_) => "Hosts",
        Overlay::AgentPresets(_) => "AgentPresets",
        Overlay::AgentPresetEditor(_) => "AgentPresetEditor",
        Overlay::Issues(_) => "Issues",
        Overlay::PullRequests(_) => "PullRequests",
        Overlay::BranchSwitch(_) => "BranchSwitch",
        Overlay::ProjectPicker(_) => "ProjectPicker",
    }
}

/// What the three exit tests below stand on: every row's opener really
/// does put its own variant on screen, and the table has a row per
/// variant. Bump the count when an overlay is added or retired.
#[test]
fn the_overlay_table_covers_every_variant_exactly_once() {
    with_seeded_presets(|| {
        let mut seen: Vec<&'static str> = Vec::new();
        for (label, open, _) in every_overlay() {
            let mut app = App::new();
            open(&mut app);
            let overlay = app
                .overlay
                .as_ref()
                .unwrap_or_else(|| panic!("{label}: the opener left no overlay open"));
            assert_eq!(
                overlay_label(overlay),
                label,
                "{label}: the opener produced another variant"
            );
            seen.push(label);
        }
        seen.sort_unstable();
        let mut unique = seen.clone();
        unique.dedup();
        assert_eq!(unique, seen, "two rows for the same variant");
        assert_eq!(seen.len(), 19, "a variant came or went: {seen:?}");
    });
}

/// One Esc from a modal's base state — nothing typed, no submenu open —
/// is enough to leave it.
#[test]
fn one_esc_closes_every_overlay() {
    with_seeded_presets(|| {
        for (label, open, lands) in every_overlay() {
            let mut app = App::new();
            let mut out = Vec::new();
            open(&mut app);
            press(&mut app, KeyCode::Esc, KeyModifiers::NONE, &mut out);
            assert_eq!(
                app.overlay.as_ref().map(overlay_label),
                lands,
                "{label}: one Esc landed somewhere else"
            );
        }
    });
}

/// A left-click off the modal's box is the mouse's way out of all of
/// them, and it lands exactly where Esc would. What a click *inside* the
/// box does is each modal's own business — a row is an action in the
/// CONTEXT MENU, the HOSTS PICKER and the AGENT PRESETS list, inert in
/// the rest — so the per-modal tests above own that half.
#[test]
fn a_click_outside_closes_every_overlay() {
    with_seeded_presets(|| {
        for (label, open, lands) in every_overlay() {
            let mut app = App::new();
            let mut out = Vec::new();
            open(&mut app);
            drawn_modal_area(&mut app);
            click(&mut app, 0, 0, &mut out);
            assert_eq!(
                app.overlay.as_ref().map(overlay_label),
                lands,
                "{label}: a click outside landed somewhere else"
            );
        }
    });
}

/// The HARDWIRED UNLOCK reaches every modal, and unlike Esc it always
/// lands on the panels — a modal opened from a modal included.
#[test]
fn ctrl_q_closes_every_overlay() {
    with_seeded_presets(|| {
        for (label, open, _) in every_overlay() {
            let mut app = App::new();
            let mut out = Vec::new();
            open(&mut app);
            press(
                &mut app,
                KeyCode::Char('q'),
                KeyModifiers::CONTROL,
                &mut out,
            );
            assert!(
                app.overlay.is_none(),
                "{label}: Ctrl+Q left {:?} on screen",
                app.overlay.as_ref().map(overlay_label)
            );
        }
    });
}

/// Where the two exits part company: Esc peels a typed filter, an open
/// submenu and a nested form one layer at a time — deliberately. Ctrl+Q
/// takes the lot in one press, which is what makes it the exit that can
/// never leave the user stuck.
#[test]
fn ctrl_q_takes_the_layers_esc_would_peel() {
    with_seeded_presets(|| {
        // A typed diff filter: Esc clears it and keeps the viewer.
        let staged_diff = |app: &mut App| {
            seed_tree(app);
            app.overlay = Some(Overlay::Diff(fake_diff_view(10)));
            assert!(paste_into_overlay(app, "alpha"));
        };
        assert!(
            matches!(peel_with(staged_diff, KeyCode::Esc), Some(Overlay::Diff(_))),
            "Esc should clear the filter first"
        );
        assert!(peel_with(staged_diff, KeyCode::Char('q')).is_none());

        // An open MODEL submenu: Esc backs out one level.
        let submenu = |app: &mut App| {
            let mut out = Vec::new();
            seed_tree(app);
            app.focus = Focus::Sessions;
            open_picker(app);
            press(app, KeyCode::Right, KeyModifiers::NONE, &mut out);
            assert!(
                matches!(&app.overlay, Some(Overlay::Menu(m)) if m.parent.is_some()),
                "→ should open a submenu, got {:?}",
                app.overlay
            );
        };
        assert!(
            matches!(peel_with(submenu, KeyCode::Esc), Some(Overlay::Menu(m)) if m.parent.is_none()),
            "Esc should pop to the root menu"
        );
        assert!(peel_with(submenu, KeyCode::Char('q')).is_none());

        // The PRESET EDITOR over its list: Esc goes back to the list.
        let editor = |app: &mut App| {
            let mut out = Vec::new();
            seed_tree(app);
            app.focus = Focus::Sessions;
            press(app, KeyCode::Char('e'), KeyModifiers::NONE, &mut out);
            press(app, KeyCode::Char('a'), KeyModifiers::CONTROL, &mut out);
            assert!(
                matches!(app.overlay, Some(Overlay::AgentPresetEditor(_))),
                "{:?}",
                app.overlay
            );
        };
        assert!(
            matches!(
                peel_with(editor, KeyCode::Esc),
                Some(Overlay::AgentPresets(_))
            ),
            "Esc should back out to the list"
        );
        assert!(peel_with(editor, KeyCode::Char('q')).is_none());
    });
}

/// Build a staged modal from scratch, press one exit key on it, and hand
/// back whatever is left on screen. `App` is not `Clone`, so comparing
/// two exits on the same state means opening it twice.
fn peel_with(open: impl Fn(&mut App), code: KeyCode) -> Option<Overlay> {
    let mut app = App::new();
    let mut out = Vec::new();
    open(&mut app);
    let mods = match code {
        KeyCode::Esc => KeyModifiers::NONE,
        _ => KeyModifiers::CONTROL,
    };
    press(&mut app, code, mods, &mut out);
    app.overlay
}

/// Backing out of a QUICK PROMPT picker with the mouse is the same
/// return trip Esc makes: the box comes back with its text. The
/// HARDWIRED UNLOCK is the one exit that doesn't — it owes the panels,
/// not the box.
#[test]
fn clicking_outside_a_quick_prompt_picker_restores_the_box() {
    with_seeded_presets(|| {
        for opener in [KeyCode::Tab, KeyCode::BackTab] {
            let picker = |app: &mut App| {
                let mut out = Vec::new();
                seed_tree(app);
                app.focus = Focus::Sessions;
                press(app, KeyCode::Char('p'), KeyModifiers::NONE, &mut out);
                assert!(paste_into_overlay(app, "Fix auth"));
                press(app, opener, KeyModifiers::NONE, &mut out);
            };

            let mut app = App::new();
            let mut out = Vec::new();
            picker(&mut app);
            drawn_modal_area(&mut app);
            click(&mut app, 0, 0, &mut out);
            let Some(Overlay::Prompt(prompt)) = &app.overlay else {
                panic!(
                    "{opener:?}: the click should hand the box back, got {:?}",
                    app.overlay
                );
            };
            assert_eq!(prompt.input.as_str(), "Fix auth");

            assert!(
                peel_with(picker, KeyCode::Char('q')).is_none(),
                "{opener:?}: Ctrl+Q owes the panels"
            );
        }
    });
}

/// A live HOTKEY CAPTURE swallows the overlay's own keys so nearly the
/// whole keyboard stays bindable — but not this one. Ctrl+Q closes the
/// SETTINGS OVERLAY out from under the capture, which is the price of it
/// meaning the same thing everywhere; the chord is the HARDWIRED UNLOCK
/// on top of whatever is bound anyway, so capturing it bound nothing.
#[test]
fn ctrl_q_closes_settings_out_from_under_a_hotkey_capture() {
    with_default_config(|| {
        let mut app = App::new();
        let mut out = Vec::new();
        open_settings_on(&mut app, crate::config::hotkeys_tab(), &mut out);
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE, &mut out);
        assert!(settings_view(&app).capturing(), "a capture is live");
        press(
            &mut app,
            KeyCode::Char('q'),
            KeyModifiers::CONTROL,
            &mut out,
        );
        assert!(app.overlay.is_none(), "{:?}", app.overlay);
    });
}

/// A submenu clicked away from closes outright rather than popping one
/// level — but it still owes the QUICK PROMPT its box.
#[test]
fn clicking_outside_a_submenu_closes_the_whole_menu() {
    with_default_config(|| {
        let mut app = App::new();
        let mut out = Vec::new();
        seed_tree(&mut app);
        app.focus = Focus::Sessions;
        press(&mut app, KeyCode::Char('n'), KeyModifiers::NONE, &mut out);
        press(&mut app, KeyCode::Right, KeyModifiers::NONE, &mut out);
        drawn_modal_area(&mut app);
        click(&mut app, 0, 0, &mut out);
        assert!(
            app.overlay.is_none(),
            "outside click closes the whole menu, not one level: {:?}",
            app.overlay
        );
    });
}

#[test]
fn hosts_a_types_a_new_destination_and_enter_connects() {
    with_seeded_hosts(|| {
        let mut app = App::new();
        let mut out = Vec::new();
        press(&mut app, KeyCode::Char('H'), KeyModifiers::SHIFT, &mut out);
        press(&mut app, KeyCode::Char('a'), KeyModifiers::NONE, &mut out);
        // While typing, list verbs are just characters — q must not
        // close, d must not delete.
        for c in "qd@db /var".chars() {
            press(&mut app, KeyCode::Char(c), KeyModifiers::NONE, &mut out);
        }
        match &app.overlay {
            Some(Overlay::Hosts(view)) => {
                assert_eq!(view.input.as_deref(), Some("qd@db /var"));
                assert_eq!(view.hosts.len(), 2, "d typed, not deleted");
            }
            other => panic!("picker should stay open, got {other:?}"),
        }
        // Draw shows the input row.
        let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
        terminal.draw(|f| ui::draw(f, &mut app)).unwrap();
        let text = buffer_text(&terminal);
        assert!(text.contains("+ qd@db /var"), "input row rendered:\n{text}");

        press(&mut app, KeyCode::Enter, KeyModifiers::NONE, &mut out);
        assert!(app.should_quit, "Enter connects to the typed host");
        let entry = app.pending_ssh.as_ref().expect("handoff target set");
        assert_eq!(entry.host, "qd@db");
        assert_eq!(entry.path.as_deref(), Some("/var"));
    });
}

#[test]
fn hosts_input_esc_cancels_and_empty_enter_is_a_noop() {
    with_seeded_hosts(|| {
        let mut app = App::new();
        let mut out = Vec::new();
        press(&mut app, KeyCode::Char('H'), KeyModifiers::SHIFT, &mut out);
        press(&mut app, KeyCode::Char('a'), KeyModifiers::NONE, &mut out);
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE, &mut out);
        match &app.overlay {
            Some(Overlay::Hosts(view)) => {
                assert!(view.input.is_none(), "empty Enter cancels the input");
            }
            other => panic!("picker should stay open, got {other:?}"),
        }
        assert!(!app.should_quit);
        press(&mut app, KeyCode::Char('a'), KeyModifiers::NONE, &mut out);
        press(&mut app, KeyCode::Char('x'), KeyModifiers::NONE, &mut out);
        press(&mut app, KeyCode::Esc, KeyModifiers::NONE, &mut out);
        match &app.overlay {
            Some(Overlay::Hosts(view)) => assert!(view.input.is_none()),
            other => panic!("Esc only cancels the input, got {other:?}"),
        }
    });
}

#[test]
fn empty_hosts_picker_shows_the_hint() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("ssh_hosts.json");
    crate::hosts::with_hosts_path(path, || {
        let mut app = App::new();
        let mut out = Vec::new();
        press(&mut app, KeyCode::Char('H'), KeyModifiers::SHIFT, &mut out);
        assert!(matches!(app.overlay, Some(Overlay::Hosts(_))));
        let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
        terminal.draw(|f| ui::draw(f, &mut app)).unwrap();
        let text = buffer_text(&terminal);
        assert!(
            text.contains("no hosts yet"),
            "empty state introduces the feature:\n{text}"
        );
        // d on the empty list must not panic or write.
        press(&mut app, KeyCode::Char('d'), KeyModifiers::NONE, &mut out);
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE, &mut out);
        assert!(!app.should_quit, "Enter on an empty list is a no-op");
    });
}

// ---- KEY COMBO DISPLAY ----

fn combo_text(app: &App) -> Option<String> {
    app.key_combo.as_ref().map(|c| c.text())
}

/// A panel key shows as `key - label` and an unbound one bare, out of
/// the box: the display is always on. Inside a modal only the keys
/// that cannot be text show, bare.
#[test]
fn key_combo_display_spells_each_press_with_what_it_did() {
    let mut app = App::new();
    seed_tree(&mut app);
    let mut out = Vec::new();
    assert!(app.key_combo.is_none(), "nothing pressed yet");
    press(&mut app, KeyCode::Char('j'), KeyModifiers::NONE, &mut out);
    assert_eq!(combo_text(&app).as_deref(), Some("j - Move down"));
    press(
        &mut app,
        KeyCode::Char('d'),
        KeyModifiers::CONTROL,
        &mut out,
    );
    assert_eq!(combo_text(&app).as_deref(), Some("^d - Half page down"));
    press(&mut app, KeyCode::Char(';'), KeyModifiers::NONE, &mut out);
    assert_eq!(
        combo_text(&app).as_deref(),
        Some(";"),
        "an unbound key shows bare: it did nothing, and the watcher sees that"
    );

    // The palette is a modal with a text field: what is typed into it
    // never shows, its navigation keys show bare.
    press(&mut app, KeyCode::Char('/'), KeyModifiers::NONE, &mut out);
    assert_eq!(combo_text(&app).as_deref(), Some("/ - Fuzzy jump"));
    assert!(matches!(app.overlay, Some(Overlay::Palette(_))));
    press(&mut app, KeyCode::Char('a'), KeyModifiers::NONE, &mut out);
    press(&mut app, KeyCode::Char('B'), KeyModifiers::SHIFT, &mut out);
    press(&mut app, KeyCode::Backspace, KeyModifiers::NONE, &mut out);
    assert_eq!(
        combo_text(&app).as_deref(),
        Some("/ - Fuzzy jump"),
        "typed text leaves the last combo standing"
    );
    press(
        &mut app,
        KeyCode::Char('u'),
        KeyModifiers::CONTROL,
        &mut out,
    );
    assert_eq!(combo_text(&app).as_deref(), Some("^u"));
    press(&mut app, KeyCode::Esc, KeyModifiers::NONE, &mut out);
    assert_eq!(combo_text(&app).as_deref(), Some("Esc"));
    press(&mut app, KeyCode::Esc, KeyModifiers::NONE, &mut out);
    assert!(app.overlay.is_none());

    // The hardwired hatch out of any modal names itself.
    press(&mut app, KeyCode::Char('?'), KeyModifiers::NONE, &mut out);
    assert_eq!(combo_text(&app).as_deref(), Some("? - Help"));
    press(
        &mut app,
        KeyCode::Char('q'),
        KeyModifiers::CONTROL,
        &mut out,
    );
    assert_eq!(combo_text(&app).as_deref(), Some("^q - Force close"));
    assert!(app.overlay.is_none());
}

/// Keys typed into a LOCKED PANE are the agent's — a password at a
/// prompt in there must never land on the screen — so the display
/// ignores them all and names only the hatch out.
#[test]
fn key_combo_display_never_echoes_what_is_typed_into_a_locked_pane() {
    let mut app = App::new();
    seed_tree(&mut app);
    let sref = SessionRef::Agent(AgentId("a1".into()));
    app.term = Some(AttachedTerm::new(sref, 80, 24));
    app.focus = Focus::Terminal;
    app.term_locked = true;
    let mut out = Vec::new();
    for code in [
        KeyCode::Char('h'),
        KeyCode::Char('u'),
        KeyCode::Char('n'),
        KeyCode::Enter,
        KeyCode::Char('j'),
    ] {
        press(&mut app, code, KeyModifiers::NONE, &mut out);
    }
    assert!(
        app.key_combo.is_none(),
        "typed keys are the agent's, not the display's"
    );
    assert!(app.term_locked, "and none of them was taken for a hotkey");
    press(
        &mut app,
        KeyCode::Char('q'),
        KeyModifiers::CONTROL,
        &mut out,
    );
    assert_eq!(
        combo_text(&app).as_deref(),
        Some("^q - Unlock terminal input")
    );
    assert!(!app.term_locked);
}

/// The retired `show_key_combos` key changes nothing: a config that
/// still says `false` — written by a build where the display was an
/// Experimental switch — leaves every press showing, and applying it
/// live takes nothing down.
#[test]
fn the_key_combo_display_shows_whatever_the_retired_key_says() {
    let mut app = App::new();
    seed_tree(&mut app);
    let cfg = crate::config::Config {
        show_key_combos: false,
        ..Default::default()
    };
    apply_config(&mut app, &cfg);
    let mut out = Vec::new();
    press(&mut app, KeyCode::Char('j'), KeyModifiers::NONE, &mut out);
    assert_eq!(combo_text(&app).as_deref(), Some("j - Move down"));
    apply_config(&mut app, &cfg);
    assert!(app.key_combo.is_some(), "a config apply leaves it standing");
}

/// PR & ISSUE COUNTS is live the same way: the switch reaches the
/// running app on apply, on and off, so the rows change on the next
/// paint and the issues sweep starts (or stops) on the next tick.
/// CARD LINE COUNTS are always read: a config that still says
/// `card_line_changes: false` (an older build's) turns nothing off,
/// and a read that finds nothing changed by the line drops its
/// checkout's entry.
#[test]
fn card_line_counts_are_always_kept_and_drop_when_nothing_changed() {
    use crate::git_diff::LineChanges;
    let mut app = App::new();
    let w1 = WorktreeId("w1".into());
    let lines = LineChanges {
        added: 12,
        removed: 4,
    };
    apply_config(&mut app, &crate::config::Config::default());
    note_worktree_lines(&mut app, &w1, Some(lines));
    assert_eq!(app.worktree_lines(&w1), Some(lines));

    app.dirty = false;
    note_worktree_lines(&mut app, &w1, Some(lines));
    assert!(!app.dirty, "the same count again paints nothing");
    note_worktree_lines(&mut app, &w1, Some(LineChanges::default()));
    assert_eq!(app.worktree_lines(&w1), None, "nothing changed by the line");
    assert!(app.dirty);

    note_worktree_lines(&mut app, &w1, Some(lines));
    apply_config(&mut app, &crate::config::Config::default());
    assert_eq!(app.worktree_lines(&w1), Some(lines), "no setting drops it");
    assert_eq!(line_changes_of(std::path::Path::new("."), None), None);
}

/// Where it draws: the footer's padding row, far left — the one blank
/// row on screen — with the key in a keycap and the bar under it
/// untouched; nothing on the row once the combo is gone.
#[test]
fn key_combo_display_sits_on_the_footers_padding_row_at_the_left() {
    let mut app = App::new();
    seed_tree(&mut app);
    let mut out = Vec::new();
    press(&mut app, KeyCode::Char('j'), KeyModifiers::NONE, &mut out);
    let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
    draw_frame(&mut terminal, &mut app).unwrap();
    let text = buffer_text(&terminal);
    let rows: Vec<&str> = text.lines().collect();
    assert_eq!(
        rows[28].trim_end(),
        "  j  - Move down",
        "the row above the bar, at the far left"
    );
    assert!(rows[29].contains("nebula v"), "the bar itself is untouched");
    let cell = &terminal.backend().buffer()[(2, 28)];
    assert_eq!(cell.symbol(), "j");
    assert_eq!(cell.bg, app.theme.sel_bg, "the key sits in a keycap");
    assert_eq!(cell.fg, app.theme.accent);

    app.key_combo = None;
    draw_frame(&mut terminal, &mut app).unwrap();
    let text = buffer_text(&terminal);
    assert_eq!(
        text.lines().nth(28).unwrap().trim(),
        "",
        "gone: the row is breathing space again"
    );
}

// ---- DELETE EMPTIED WORKTREE -------------------------------------

fn terminal_entity(id: &str, wt: &str, name: &str) -> nebula_core::Entity {
    nebula_core::Entity::Terminal(nebula_core::TerminalTab {
        id: TerminalId(id.into()),
        worktree_id: WorktreeId(wt.into()),
        name: name.into(),
        sort_order: 0,
        alive: true,
        run_command: None,
    })
}

/// p1 / w1 (main, a1) plus a linked worktree w2 on `feature`, holding
/// whatever rows the test upserts next.
fn seed_emptiable_tree(app: &mut App) {
    seed_tree(app);
    seed_linked_worktree(app);
}

fn delete_worktree_requests(out: &[ClientRequest]) -> Vec<&str> {
    out.iter()
        .filter_map(|r| match r {
            ClientRequest::DeleteWorktree { id, .. } => Some(id.0.as_str()),
            _ => None,
        })
        .collect()
}

/// The row menu's Delete on `id`, with the confirm it opens.
fn menu_delete(app: &mut App, id: &str) -> ConfirmDialog {
    let mut out = Vec::new();
    run_menu_action(app, MenuAction::DeleteAgent(AgentId(id.into())), &mut out);
    match &app.overlay {
        Some(Overlay::Confirm(c)) => c.clone(),
        other => panic!("expected a confirm, got {other:?}"),
    }
}

fn upsert_agent(app: &mut App, id: &str, wt: &str, name: &str, archived: bool) {
    hse(
        app,
        ServerEvent::EntityUpserted {
            entity: agent_entity(id, wt, name, archived),
        },
    );
}

/// The last card of a linked worktree: `d`'s confirm asks about the
/// checkout in the same dialog, BEFORE anything is deleted — and the
/// key and the row menu build the very same dialog (INPUT PARITY).
#[test]
fn deleting_the_last_card_asks_about_the_worktree_first() {
    with_default_config(|| {
        let mut app = App::new();
        seed_emptiable_tree(&mut app);
        upsert_agent(&mut app, "a2", "w2", "agent-2", false);
        let by_menu = menu_delete(&mut app, "a2");
        app.overlay = None;

        app.focus = Focus::Sessions;
        app.sel_worktree = app
            .visible_worktrees()
            .iter()
            .position(|w| w.id.0 == "w2")
            .unwrap();
        app.sel_session = 0;
        let mut out = Vec::new();
        press(&mut app, KeyCode::Char('d'), KeyModifiers::NONE, &mut out);
        let by_key = match &app.overlay {
            Some(Overlay::Confirm(c)) => c.clone(),
            other => panic!("d opens the confirm, got {other:?}"),
        };
        assert_eq!(
            by_key.action, by_menu.action,
            "key and menu share the dialog"
        );
        assert_eq!(by_key.message, by_menu.message);

        assert_eq!(
            by_key.action,
            PendingAction::ThenDeleteWorktree {
                first: Box::new(PendingAction::DeleteAgent(AgentId("a2".into()))),
                worktree: WorktreeId("w2".into()),
                offered: true,
            }
        );
        assert_eq!(by_key.title, "Delete agent", "still the card's own delete");
        assert!(
            by_key.message.contains("Delete agent 'agent-2'?")
                && by_key.message.contains("worktree 'feature'")
                && by_key.message.contains("delete it from disk too?"),
            "one dialog carries both questions: {}",
            by_key.message
        );
        assert!(
            out.is_empty(),
            "nothing is deleted before the answer: {out:?}"
        );
        assert!(app.tree.agents.iter().any(|a| a.id.0 == "a2"));
    });
}

/// `Enter`/`y` is yes: the card's delete goes out, then the same
/// forced worktree delete the band's own `d` sends, and the band drops.
#[test]
fn yes_deletes_the_card_and_then_the_worktree() {
    with_default_config(|| {
        let mut app = App::new();
        seed_emptiable_tree(&mut app);
        upsert_agent(&mut app, "a2", "w2", "agent-2", false);
        menu_delete(&mut app, "a2");
        let mut out = Vec::new();
        press(&mut app, KeyCode::Char('y'), KeyModifiers::NONE, &mut out);
        assert!(app.overlay.is_none(), "y answered it: {:?}", app.overlay);
        let kinds: Vec<&str> = out
            .iter()
            .filter_map(|r| match r {
                ClientRequest::DeleteAgent { id, .. } if id.0 == "a2" => Some("delete-agent"),
                ClientRequest::DeleteWorktree {
                    force: true, id, ..
                } if id.0 == "w2" => Some("delete-worktree"),
                _ => None,
            })
            .collect();
        assert_eq!(kinds, ["delete-agent", "delete-worktree"], "{out:?}");
        assert!(!app.tree.agents.iter().any(|a| a.id.0 == "a2"));
        assert!(
            !app.tree.worktrees.iter().any(|w| w.id.0 == "w2"),
            "the band drops optimistically"
        );
    });
}

/// `n` is no: the card goes, the emptied checkout stays.
#[test]
fn no_deletes_the_card_and_keeps_the_worktree() {
    with_default_config(|| {
        let mut app = App::new();
        seed_emptiable_tree(&mut app);
        upsert_agent(&mut app, "a2", "w2", "agent-2", false);
        menu_delete(&mut app, "a2");
        let mut out = Vec::new();
        press(&mut app, KeyCode::Char('n'), KeyModifiers::NONE, &mut out);
        assert!(app.overlay.is_none(), "n answered it: {:?}", app.overlay);
        assert!(
            out.iter()
                .any(|r| matches!(r, ClientRequest::DeleteAgent { id, .. } if id.0 == "a2")),
            "the card's own delete goes out: {out:?}"
        );
        assert!(
            delete_worktree_requests(&out).is_empty(),
            "the worktree stays: {out:?}"
        );
        assert!(app.tree.worktrees.iter().any(|w| w.id.0 == "w2"));
    });
}

/// `Esc` is cancel: the card stays alive and nothing is sent.
#[test]
fn esc_keeps_the_last_card_alive() {
    with_default_config(|| {
        let mut app = App::new();
        seed_emptiable_tree(&mut app);
        upsert_agent(&mut app, "a2", "w2", "agent-2", false);
        menu_delete(&mut app, "a2");
        let mut out = Vec::new();
        press(&mut app, KeyCode::Esc, KeyModifiers::NONE, &mut out);
        assert!(app.overlay.is_none());
        assert!(out.is_empty(), "Esc deletes nothing: {out:?}");
        assert!(
            app.tree.agents.iter().any(|a| a.id.0 == "a2"),
            "the card lives"
        );
        assert!(app.tree.worktrees.iter().any(|w| w.id.0 == "w2"));
    });
}

/// The last terminal of a linked worktree asks the same way.
#[test]
fn closing_the_last_terminal_asks_about_the_worktree() {
    with_default_config(|| {
        let mut app = App::new();
        seed_emptiable_tree(&mut app);
        hse(
            &mut app,
            ServerEvent::EntityUpserted {
                entity: terminal_entity("t1", "w2", "shell-1"),
            },
        );
        let mut out = Vec::new();
        run_menu_action(
            &mut app,
            MenuAction::CloseTerminal(TerminalId("t1".into())),
            &mut out,
        );
        match &app.overlay {
            Some(Overlay::Confirm(c)) => {
                assert_eq!(c.title, "Close terminal");
                assert_eq!(
                    c.action,
                    PendingAction::ThenDeleteWorktree {
                        first: Box::new(PendingAction::CloseTerminal(TerminalId("t1".into()))),
                        worktree: WorktreeId("w2".into()),
                        offered: true,
                    }
                );
            }
            other => panic!("expected the close confirm, got {other:?}"),
        }
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE, &mut out);
        assert!(
            out.iter()
                .any(|r| matches!(r, ClientRequest::CloseTerminal { .. })),
            "{out:?}"
        );
        assert_eq!(delete_worktree_requests(&out), ["w2"]);
    });
}

/// A card left behind, or the ROOT WORKTREE, and the confirm is the
/// plain one: `n` cancels as it does everywhere else.
#[test]
fn a_card_left_behind_or_the_root_worktree_asks_nothing_extra() {
    with_default_config(|| {
        let mut app = App::new();
        seed_emptiable_tree(&mut app);
        upsert_agent(&mut app, "a2", "w2", "agent-2", false);
        upsert_agent(&mut app, "a3", "w2", "agent-3", false);
        let c = menu_delete(&mut app, "a2");
        assert_eq!(
            c.action,
            PendingAction::DeleteAgent(AgentId("a2".into())),
            "agent-3 still holds the worktree"
        );
        assert!(!c.message.contains("worktree"), "{}", c.message);
        let mut out = Vec::new();
        press(&mut app, KeyCode::Char('n'), KeyModifiers::NONE, &mut out);
        assert!(out.is_empty(), "n on a plain confirm cancels: {out:?}");
        assert!(app.tree.agents.iter().any(|a| a.id.0 == "a2"));

        // a1 is the root worktree's only session: the root is never offered.
        let c = menu_delete(&mut app, "a1");
        assert_eq!(c.action, PendingAction::DeleteAgent(AgentId("a1".into())));
    });
}

/// A `D` that takes every live row of a worktree asks about the
/// checkout in its own dialog, the same way.
#[test]
fn deleting_all_sessions_asks_about_the_worktree() {
    with_default_config(|| {
        let mut app = App::new();
        seed_emptiable_tree(&mut app);
        upsert_agent(&mut app, "a2", "w2", "agent-2", false);
        hse(
            &mut app,
            ServerEvent::EntityUpserted {
                entity: terminal_entity("t1", "w2", "shell-1"),
            },
        );
        app.focus = Focus::Sessions;
        app.sel_worktree = app
            .visible_worktrees()
            .iter()
            .position(|w| w.id.0 == "w2")
            .unwrap();
        open_delete_all_confirm(&mut app);
        let action = match &app.overlay {
            Some(Overlay::Confirm(c)) => c.action.clone(),
            other => panic!("expected the delete-all confirm, got {other:?}"),
        };
        match action {
            PendingAction::ThenDeleteWorktree {
                first,
                worktree,
                offered: true,
            } => {
                assert_eq!(worktree, WorktreeId("w2".into()));
                assert!(
                    matches!(*first, PendingAction::DeleteAllSessions { .. }),
                    "{first:?}"
                );
            }
            other => panic!("expected the worktree question, got {other:?}"),
        }
        let mut out = Vec::new();
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE, &mut out);
        assert_eq!(
            out.iter()
                .filter(|r| matches!(
                    r,
                    ClientRequest::DeleteAgent { .. } | ClientRequest::CloseTerminal { .. }
                ))
                .count(),
            2,
            "both rows' own deletes went out: {out:?}"
        );
        assert_eq!(delete_worktree_requests(&out), ["w2"]);
    });
}

/// With **Delete emptied worktree** on, the question is not asked: the
/// card's ordinary confirm says the worktree goes with it, `Enter`
/// deletes both, and `n` cancels like any two-way confirm.
#[test]
fn delete_empty_worktree_setting_skips_the_question() {
    with_config_json(r#"{"delete_empty_worktree": true}"#, || {
        let mut app = App::new();
        seed_emptiable_tree(&mut app);
        upsert_agent(&mut app, "a2", "w2", "agent-2", false);
        let c = menu_delete(&mut app, "a2");
        assert_eq!(
            c.action,
            PendingAction::ThenDeleteWorktree {
                first: Box::new(PendingAction::DeleteAgent(AgentId("a2".into()))),
                worktree: WorktreeId("w2".into()),
                offered: false,
            }
        );
        let last = c.message.lines().last().unwrap();
        assert!(
            last.contains("goes with it") && !last.contains('?'),
            "a statement, not a question, after the card's own one: {last}"
        );
        let mut out = Vec::new();
        press(&mut app, KeyCode::Char('n'), KeyModifiers::NONE, &mut out);
        assert!(out.is_empty(), "n cancels a two-way confirm: {out:?}");

        menu_delete(&mut app, "a2");
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE, &mut out);
        assert!(
            out.iter()
                .any(|r| matches!(r, ClientRequest::DeleteAgent { id, .. } if id.0 == "a2")),
            "{out:?}"
        );
        assert_eq!(delete_worktree_requests(&out), ["w2"]);
        assert!(!app.tree.worktrees.iter().any(|w| w.id.0 == "w2"));
    });
}

/// Archived sessions still filed under the worktree hold history the
/// delete would take, so even the forcing setting asks — and the
/// dialog says how many.
#[test]
fn archived_sessions_keep_the_question_even_when_forced() {
    with_config_json(r#"{"delete_empty_worktree": true}"#, || {
        let mut app = App::new();
        seed_emptiable_tree(&mut app);
        upsert_agent(&mut app, "a2", "w2", "agent-2", false);
        upsert_agent(&mut app, "a3", "w2", "agent-3", true);
        let c = menu_delete(&mut app, "a2");
        assert!(
            matches!(
                c.action,
                PendingAction::ThenDeleteWorktree { offered: true, .. }
            ),
            "{:?}",
            c.action
        );
        assert!(
            c.message.contains("1 archived session"),
            "names the history at stake: {}",
            c.message
        );
    });
}

/// **Show all worktrees** drops the question, not the setting: with
/// both on, the last session's delete and the last terminal's close
/// each take the worktree with them, the way they do with it off.
#[test]
fn delete_empty_worktree_setting_holds_with_show_all_worktrees_on() {
    with_config_json(r#"{"delete_empty_worktree": true}"#, || {
        let mut app = App::new();
        seed_emptiable_tree(&mut app);
        app.show_all_worktrees = true;
        upsert_agent(&mut app, "a2", "w2", "agent-2", false);
        let c = menu_delete(&mut app, "a2");
        assert_eq!(
            c.action,
            PendingAction::ThenDeleteWorktree {
                first: Box::new(PendingAction::DeleteAgent(AgentId("a2".into()))),
                worktree: WorktreeId("w2".into()),
                offered: false,
            }
        );
        assert!(c.message.contains("goes with it"), "{}", c.message);
        let mut out = Vec::new();
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE, &mut out);
        assert!(
            out.iter()
                .any(|r| matches!(r, ClientRequest::DeleteAgent { id, .. } if id.0 == "a2")),
            "{out:?}"
        );
        assert_eq!(delete_worktree_requests(&out), ["w2"]);
        assert!(
            !app.tree.worktrees.iter().any(|w| w.id.0 == "w2"),
            "no empty band is left behind"
        );
    });
    with_config_json(r#"{"delete_empty_worktree": true}"#, || {
        let mut app = App::new();
        seed_emptiable_tree(&mut app);
        app.show_all_worktrees = true;
        hse(
            &mut app,
            ServerEvent::EntityUpserted {
                entity: terminal_entity("t1", "w2", "shell-1"),
            },
        );
        let mut out = Vec::new();
        run_menu_action(
            &mut app,
            MenuAction::CloseTerminal(TerminalId("t1".into())),
            &mut out,
        );
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE, &mut out);
        assert!(
            out.iter()
                .any(|r| matches!(r, ClientRequest::CloseTerminal { .. })),
            "{out:?}"
        );
        assert_eq!(delete_worktree_requests(&out), ["w2"]);
        assert!(!app.tree.worktrees.iter().any(|w| w.id.0 == "w2"));
    });
}

/// Both settings on, and archived sessions still filed under the
/// worktree: their history is at stake, so the question comes back.
#[test]
fn archived_sessions_keep_the_question_with_show_all_worktrees_on() {
    with_config_json(r#"{"delete_empty_worktree": true}"#, || {
        let mut app = App::new();
        seed_emptiable_tree(&mut app);
        app.show_all_worktrees = true;
        upsert_agent(&mut app, "a2", "w2", "agent-2", false);
        upsert_agent(&mut app, "a3", "w2", "agent-3", true);
        let c = menu_delete(&mut app, "a2");
        assert!(
            matches!(
                c.action,
                PendingAction::ThenDeleteWorktree { offered: true, .. }
            ),
            "{:?}",
            c.action
        );
    });
}
