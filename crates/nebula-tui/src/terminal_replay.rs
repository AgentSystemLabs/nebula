//! Large first-visit terminal replays are parsed off the UI thread.
//! A ticket binds each answer to the pane that requested it. Live output
//! and resizes remain ordered while the parser is away; cached sessions
//! and small replays keep their immediate inline path.

use crate::app::{App, AttachedTerm};
use crate::view_jobs::{self, Answer, Cancel, Jobs};

/// Smaller replays take less than a frame and avoid a thread handoff.
pub(crate) const BACKGROUND_BYTES: usize = 64 * 1024;

pub(crate) enum Event {
    Output { seq: u64, data: Vec<u8> },
    Resize { cols: u16, rows: u16 },
}

pub(crate) struct Pending {
    ticket: u64,
    cancel: Cancel,
    events: Vec<Event>,
    bytes: usize,
}

impl Pending {
    pub(crate) fn output(&mut self, seq: u64, data: Vec<u8>) {
        self.bytes += data.len();
        self.events.push(Event::Output { seq, data });
    }

    pub(crate) fn resize(&mut self, cols: u16, rows: u16) {
        self.events.push(Event::Resize { cols, rows });
    }
}

impl Drop for Pending {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

pub struct Replayed {
    ticket: u64,
    term: Box<AttachedTerm>,
}

// Never print terminal contents in a job's Debug output.
impl std::fmt::Debug for Replayed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Replayed")
            .field("ticket", &self.ticket)
            .finish_non_exhaustive()
    }
}

pub(crate) fn start(term: &mut AttachedTerm, base_seq: u64, data: Vec<u8>, jobs: Jobs) {
    let ticket = view_jobs::ticket();
    let cancel = Cancel::default();
    let (sref, cols, rows) = (term.sref.clone(), term.cols, term.rows);
    term.replay = Some(Pending {
        ticket,
        cancel: cancel.clone(),
        events: Vec::new(),
        bytes: 0,
    });
    term.booting = false;
    jobs.run(move || {
        if cancel.is_cancelled() {
            return None;
        }
        let mut parsed = AttachedTerm::new(sref, cols, rows);
        parsed.apply_scrollback(base_seq, &data);
        // History must never repeat an OSC 52 clipboard write.
        parsed.take_clipboard();
        (!cancel.is_cancelled()).then(|| {
            Answer::TerminalReplay(Replayed {
                ticket,
                term: Box::new(parsed),
            })
        })
    });
}

fn apply(term: &mut AttachedTerm, events: Vec<Event>) {
    for event in events {
        match event {
            Event::Output { seq, data } => term.apply_output(seq, &data),
            Event::Resize { cols, rows } => term.resize(cols, rows),
        }
    }
}

pub(crate) fn land(app: &mut App, mut answer: Replayed) {
    let matches = app.pane.term.as_ref().is_some_and(|current| {
        current.sref == answer.term.sref
            && current
                .replay
                .as_ref()
                .is_some_and(|p| p.ticket == answer.ticket)
    });
    if !matches {
        // Retiring a parser with thousands of history rows can itself
        // take a noticeable slice of a frame. Dispose of stale answers
        // on the same pool that created them.
        if let Some(jobs) = app.jobs.view_jobs.clone() {
            jobs.run(move || {
                drop(answer);
                None
            });
        }
        return;
    }
    let current = app.pane.term.as_mut().expect("matching pane");
    let pending = current.replay.as_mut().expect("matching replay");
    let events = std::mem::take(&mut pending.events);
    let bytes = std::mem::take(&mut pending.bytes);
    // A busy process can print another large batch while the replay is
    // parsed. Catch it up off-thread too, rather than moving that stall
    // to the result handler. New arrivals keep their order in Pending.
    if bytes >= BACKGROUND_BYTES {
        if let Some(jobs) = app.jobs.view_jobs.clone() {
            let cancel = pending.cancel.clone();
            jobs.run(move || {
                if cancel.is_cancelled() {
                    return None;
                }
                apply(&mut answer.term, events);
                (!cancel.is_cancelled()).then_some(Answer::TerminalReplay(answer))
            });
            return;
        }
    }
    apply(&mut answer.term, events);
    answer.term.kitty_flags = current.kitty_flags;
    answer.term.exited = current.exited;
    if let Some(scroll) = current.pending_scroll.take() {
        answer.term.set_scroll(scroll);
    }
    if let Some(payload) = answer.term.take_clipboard() {
        app.chrome.pending_clipboard = Some(payload);
        app.chrome.flash = Some("copied (via terminal)".into());
    }
    *current = *answer.term;
    // No screen existed to select when this replay started.
    app.pane.term_selection = None;
    app.chrome.dirty = true;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event_loop::handle_server_event;
    use nebula_core::{AgentId, ServerEvent, SessionRef};
    use tokio::sync::mpsc;

    fn setup() -> (App, mpsc::UnboundedReceiver<Answer>) {
        let (tx, rx) = mpsc::unbounded_channel();
        let mut app = App::new();
        app.jobs.view_jobs = Some(Jobs::new(tx));
        app.pane.term = Some(AttachedTerm::new(sref(), 80, 20));
        (app, rx)
    }

    fn sref() -> SessionRef {
        SessionRef::Agent(AgentId("replay-test".into()))
    }

    fn history() -> Vec<u8> {
        b"\x1b]52;c;aGlzdG9yeQ==\x07\x1b[31mhistory line\x1b[0m\r\n".repeat(2500)
    }

    fn event(app: &mut App, ev: ServerEvent) {
        handle_server_event(app, ev, &mut Vec::new());
    }

    async fn answer(rx: &mut mpsc::UnboundedReceiver<Answer>) -> Replayed {
        match tokio::time::timeout(std::time::Duration::from_secs(5), rx.recv())
            .await
            .unwrap()
            .unwrap()
        {
            Answer::TerminalReplay(answer) => answer,
            other => panic!("unexpected replay answer: {other:?}"),
        }
    }

    #[tokio::test]
    async fn replay_keeps_output_resize_scroll_exit_and_keyboard_state_in_order() {
        let (mut app, mut rx) = setup();
        let data = history();
        let end = 17 + data.len() as u64;
        let mut expected = AttachedTerm::new(sref(), 80, 20);
        expected.apply_scrollback(17, &data);
        expected.take_clipboard();
        event(
            &mut app,
            ServerEvent::Scrollback {
                session: sref(),
                base_seq: 17,
                data,
            },
        );
        assert!(app.pane.term.as_ref().unwrap().replay.is_some());
        assert!(
            !app.pane.term.as_ref().unwrap().painted,
            "handler returns before parsing"
        );
        crate::event_loop::handle_terminal_event(
            &mut app,
            crossterm::event::Event::Key(crossterm::event::KeyEvent::new(
                crossterm::event::KeyCode::Char('?'),
                crossterm::event::KeyModifiers::NONE,
            )),
            &mut Vec::new(),
        );
        assert!(
            matches!(app.modals.overlay, Some(crate::app::Overlay::Help(_))),
            "input still opens help while the replay is pending"
        );
        // A size change and a new program frame arrive while parsing is off-thread.
        app.pane.term.as_mut().unwrap().resize(60, 12);
        expected.resize(60, 12);
        let live = b"\r\nlatest live output\x1b]52;c;bGl2ZQ==\x07".to_vec();
        expected.apply_output(end, &live);
        event(
            &mut app,
            ServerEvent::Output {
                session: sref(),
                seq: end,
                data: live,
            },
        );
        event(
            &mut app,
            ServerEvent::KittyFlags {
                session: sref(),
                flags: 7,
            },
        );
        event(
            &mut app,
            ServerEvent::SessionExited {
                session: sref(),
                exit_code: Some(0),
            },
        );
        app.pane.term.as_mut().unwrap().set_scroll(2);
        expected.set_scroll(2);
        land(&mut app, answer(&mut rx).await);
        let term = app.pane.term.as_ref().unwrap();
        assert!(term.replay.is_none() && term.painted && term.exited);
        assert_eq!((term.cols, term.rows, term.kitty_flags), (60, 12, 7));
        assert_eq!(term.next_seq, expected.next_seq);
        assert_eq!(term.scroll_offset(), expected.scroll_offset());
        assert_eq!(
            term.parser.screen().contents_formatted(),
            expected.parser.screen().contents_formatted()
        );
        assert_eq!(app.chrome.pending_clipboard.as_deref(), Some("bGl2ZQ=="));
    }

    #[tokio::test]
    async fn history_clipboard_is_suppressed_and_large_catchup_stays_off_thread() {
        let (mut app, mut rx) = setup();
        let data = history();
        let end = data.len() as u64;
        event(
            &mut app,
            ServerEvent::Scrollback {
                session: sref(),
                base_seq: 0,
                data,
            },
        );
        let first = answer(&mut rx).await;
        let live = b"live output\r\n".repeat(6000);
        let live_end = end + live.len() as u64;
        event(
            &mut app,
            ServerEvent::Output {
                session: sref(),
                seq: end,
                data: live,
            },
        );
        land(&mut app, first);
        assert!(
            app.pane.term.as_ref().unwrap().replay.is_some(),
            "large catchup returns to worker"
        );
        event(
            &mut app,
            ServerEvent::Output {
                session: sref(),
                seq: live_end,
                data: b"final output".to_vec(),
            },
        );
        land(&mut app, answer(&mut rx).await);
        let term = app.pane.term.as_ref().unwrap();
        assert!(term.replay.is_none());
        assert_eq!(term.next_seq, live_end + 12);
        assert!(term.parser.screen().contents().contains("final output"));
        assert!(
            app.chrome.pending_clipboard.is_none(),
            "history did not rewrite clipboard"
        );
    }

    #[tokio::test]
    async fn stale_results_cannot_replace_a_new_visit_to_the_same_session() {
        let (mut app, mut rx) = setup();
        event(
            &mut app,
            ServerEvent::Scrollback {
                session: sref(),
                base_seq: 0,
                data: history(),
            },
        );
        let stale = answer(&mut rx).await;
        // Leave and return to the same session before the first answer is landed.
        app.pane.term = Some(AttachedTerm::new(sref(), 80, 20));
        let fresh = b"fresh history\r\n".repeat(6000);
        event(
            &mut app,
            ServerEvent::Scrollback {
                session: sref(),
                base_seq: 1000,
                data: fresh,
            },
        );
        land(&mut app, stale);
        assert!(app.pane.term.as_ref().unwrap().replay.is_some());
        land(&mut app, answer(&mut rx).await);
        let term = app.pane.term.as_ref().unwrap();
        assert!(term.parser.screen().contents().contains("fresh history"));
        assert!(!term.parser.screen().contents().contains("history line"));
    }

    #[tokio::test]
    async fn a_newer_inline_replay_or_refusal_supersedes_background_work() {
        for refused in [false, true] {
            let (mut app, mut rx) = setup();
            event(
                &mut app,
                ServerEvent::Scrollback {
                    session: sref(),
                    base_seq: 0,
                    data: history(),
                },
            );
            let stale = answer(&mut rx).await;
            if refused {
                event(
                    &mut app,
                    ServerEvent::AttachRefused {
                        session: sref(),
                        message: "checkout unavailable".into(),
                    },
                );
            } else {
                event(
                    &mut app,
                    ServerEvent::Scrollback {
                        session: sref(),
                        base_seq: 0,
                        data: b"new process".to_vec(),
                    },
                );
            }
            land(&mut app, stale);
            let term = app.pane.term.as_ref().unwrap();
            assert!(term.replay.is_none());
            if refused {
                assert_eq!(term.refused.as_deref(), Some("checkout unavailable"));
            } else {
                assert_eq!(term.parser.screen().contents(), "new process");
            }
        }
    }

    #[tokio::test]
    async fn cached_screens_and_small_replays_keep_the_inline_path() {
        let (mut app, _rx) = setup();
        event(
            &mut app,
            ServerEvent::Scrollback {
                session: sref(),
                base_seq: 0,
                data: b"cached screen".to_vec(),
            },
        );
        assert!(app.pane.term.as_ref().unwrap().replay.is_none());
        let data = history();
        event(
            &mut app,
            ServerEvent::Scrollback {
                session: sref(),
                base_seq: 13,
                data,
            },
        );
        assert!(
            app.pane.term.as_ref().unwrap().replay.is_none(),
            "kept parser is continued inline"
        );
    }
    #[tokio::test]
    #[ignore = "manual replay benchmark; run with --ignored --nocapture"]
    async fn large_replay_ui_handler_cost() {
        let data = b"\x1b[31mterminal history with colour and text\x1b[0m\r\n".repeat(20000);
        let mut inline = std::time::Duration::ZERO;
        let mut offloaded = std::time::Duration::ZERO;
        for _ in 0..20 {
            let mut term = AttachedTerm::new(sref(), 100, 40);
            let began = std::time::Instant::now();
            term.apply_scrollback(0, &data);
            inline += began.elapsed();
            let expected = term.parser.screen().contents_formatted();
            drop(term);
            let (mut app, mut rx) = setup();
            app.pane.term = Some(AttachedTerm::new(sref(), 100, 40));
            let replay = data.clone();
            let began = std::time::Instant::now();
            event(
                &mut app,
                ServerEvent::Scrollback {
                    session: sref(),
                    base_seq: 0,
                    data: replay,
                },
            );
            offloaded += began.elapsed();
            land(&mut app, answer(&mut rx).await);
            assert_eq!(
                app.pane
                    .term
                    .as_ref()
                    .unwrap()
                    .parser
                    .screen()
                    .contents_formatted(),
                expected
            );
        }
        eprintln!(
            "large terminal replay: inline UI {:.3} ms, offloaded UI {:.3} ms",
            inline.as_secs_f64() * 50.0,
            offloaded.as_secs_f64() * 50.0
        );
    }
}
