//! Busy/idle scanner for OSC 9;4 progress and Codex's OSC 7501 lifecycle
//! status, read straight off the agent's output stream.
//!
//! Codex 0.163.0 emits `ESC ] 7501;state=working:app=codex ESC \\` and
//! `state=idle` on turn completion, not OSC 9;4. Reading these messages
//! keeps a finished turn from staying yellow when its Stop hook is not
//! available (for example, hooks awaiting trust). `blocked` stays busy:
//! a pending approval is not a finished turn. `state=clear` only releases
//! terminal ownership on exit or handoff and says nothing about the turn.
//!
//! Why this exists: Claude Code fires no hook when the user cancels a turn
//! (`Stop` is documented as "does not run if the stoppage occurred due to a
//! user interrupt"), and the `idle_prompt` notification that normally
//! un-sticks such a turn is gated on 60s of quiet AND on the user not having
//! touched the keyboard since the last message — pressing escape is touching
//! the keyboard, so after a cancel it never arrives at all. The status
//! machine had no way back out of `running`.
//!
//! It does, however, drive the terminal progress bar (`terminalProgressBarEnabled`,
//! on by default), and that IS emitted on cancel. Verified against Claude
//! Code 2.1.241 by capturing raw PTY bytes:
//!
//! | moment                       | sequence         |
//! |------------------------------|------------------|
//! | startup, parked at the input | `ESC ] 9;4;0; BEL` |
//! | prompt submitted             | `ESC ] 9;4;3; BEL` |
//! | permission prompt open       | *stays* `3`      |
//! | turn ends (Stop fires)       | `ESC ] 9;4;0; BEL` |
//! | turn cancelled with escape   | `ESC ] 9;4;0; BEL`, ~0.4s, no hook |
//!
//! The permission-prompt row is the important one: the window title flips to
//! the idle glyph while the prompt waits, but the progress state does not, so
//! this signal cannot green out an agent that genuinely needs feedback.
//!
//! States follow the ConEmu/Windows-Terminal convention: 0 removes the
//! progress bar, 1/2/3/4 are normal/error/indeterminate/paused. Anything
//! that isn't 0 counts as busy.

use super::osc::OscFramer;

/// Longest OSC payload we will buffer. Both progress and lifecycle status fit;
/// anything longer (a title, a hyperlink, a base64 image) is poisoned and
/// skipped without allocating.
const MAX_OSC: usize = 64;

/// Tracks the child's advertised progress state across chunk boundaries.
#[derive(Debug)]
pub struct ProgressScanner {
    osc: OscFramer,
    /// Last state the child advertised; `None` until it advertises one.
    busy: Option<bool>,
}

impl Default for ProgressScanner {
    fn default() -> Self {
        Self::new()
    }
}

impl ProgressScanner {
    pub fn new() -> Self {
        Self {
            osc: OscFramer::new(MAX_OSC, prefix_possible),
            busy: None,
        }
    }

    /// A scanner that already knows the child's busy state, so the next
    /// edge after an IN-PLACE RESTART is a real one.
    pub fn restored(busy: Option<bool>) -> Self {
        Self {
            busy,
            ..Self::new()
        }
    }

    /// The child's current busy state, or `None` if it never advertised one.
    pub fn busy(&self) -> Option<bool> {
        self.busy
    }

    /// Scan a chunk of child output. Sequences split across chunks are fine —
    /// the state machine persists between calls. Returns `Some(busy)` only
    /// when the state *changed*, so callers see edges, not a fire-hose.
    ///
    /// A chunk that flips the state and back reports only where it ended up.
    /// That is the right answer — the intermediate state is already stale by
    /// the time the caller sees it — and the pump's ~5ms/8KB coalescing means
    /// a whole turn never lands in one chunk anyway.
    pub fn feed(&mut self, data: &[u8]) -> Option<bool> {
        let before = self.busy;
        for &b in data {
            if let Some(payload) = self.osc.step(b) {
                self.dispatch(payload);
            }
        }
        match self.busy {
            after if after != before => after,
            _ => None,
        }
    }

    fn dispatch(&mut self, payload: Vec<u8>) {
        if payload.starts_with(b"7501;") {
            match payload.as_slice() {
                b"7501;state=idle:app=codex" => self.busy = Some(false),
                b"7501;state=working:app=codex" | b"7501;state=blocked:app=codex" => {
                    self.busy = Some(true);
                }
                _ => {}
            }
            return;
        }
        let Some(rest) = payload.strip_prefix(b"9;4;") else {
            return;
        };
        // `9;4;<state>` or `9;4;<state>;<percent>`; an empty state is a no-op
        // rather than a guess.
        let state = rest.split(|&b| b == b';').next().unwrap_or(b"");
        if state.is_empty() || !state.iter().all(|b| b.is_ascii_digit()) {
            return;
        }
        let busy = state.iter().any(|&b| b != b'0');
        self.busy = Some(busy);
    }
}

/// Could `buf` still grow into a progress or lifecycle payload?
fn prefix_possible(buf: &[u8]) -> bool {
    [b"9;4;".as_slice(), b"7501;state=".as_slice()]
        .iter()
        .any(|prefix| {
            let n = buf.len().min(prefix.len());
            buf[..n] == prefix[..n]
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scan(chunks: &[&[u8]]) -> Vec<Option<bool>> {
        let mut s = ProgressScanner::new();
        chunks.iter().map(|c| s.feed(c)).collect()
    }

    #[test]
    fn reports_only_edges() {
        // The real Claude Code turn: idle at startup, busy on submit, idle
        // again when the turn ends. Repeats of a state report nothing.
        assert_eq!(
            scan(&[
                b"\x1b]9;4;0;\x07",
                b"\x1b]9;4;0;\x07",
                b"\x1b]9;4;3;\x07",
                b"\x1b]9;4;3;\x07",
                b"\x1b]9;4;0;\x07",
            ]),
            vec![Some(false), None, Some(true), None, Some(false)]
        );
    }

    #[test]
    fn codex_lifecycle_reports_busy_and_idle_without_hooks() {
        assert_eq!(
            scan(&[
                b"\x1b]7501;state=idle:app=codex\x1b\\",
                b"\x1b]7501;state=working:app=codex\x1b\\",
                b"\x1b]7501;state=blocked:app=codex\x1b\\",
                b"\x1b]7501;state=working:app=codex\x1b\\",
                b"\x1b]7501;state=idle:app=codex\x1b\\",
                b"\x1b]7501;state=idle:app=codex\x1b\\",
                b"\x1b]7501;state=clear\x1b\\",
            ]),
            vec![Some(false), Some(true), None, None, Some(false), None, None]
        );
    }

    #[test]
    fn codex_lifecycle_survives_every_chunk_boundary_and_terminator() {
        for full in [
            b"\x1b]7501;state=working:app=codex\x1b\\".as_slice(),
            b"\x1b]7501;state=working:app=codex\x07".as_slice(),
        ] {
            for split in 0..full.len() {
                let mut s = ProgressScanner::new();
                assert_eq!(s.feed(&full[..split]), None);
                assert_eq!(s.feed(&full[split..]), Some(true), "split at {split}");
            }
        }
    }

    #[test]
    fn codex_lifecycle_ignores_clear_unknown_states_and_other_apps() {
        let mut s = ProgressScanner::restored(Some(true));
        for payload in [
            "7501;state=clear",
            "7501;state=idle",
            "7501;state=idle:app=other",
            "7501;state=idle:app=codex-extra",
            "7501;state=unknown:app=codex",
            "7501;state=idle:app=codex:unexpected",
        ] {
            assert_eq!(s.feed(format!("\x1b]{payload}\x1b\\").as_bytes()), None);
            assert_eq!(s.busy(), Some(true), "{payload}");
        }
        assert_eq!(s.feed(b"\x1b]7501;state=idle:app=codex\x1b\\"), Some(false));
    }

    #[test]
    fn codex_lifecycle_finishes_interactive_and_optimistic_launches() {
        use crate::status::{AgentStatusMachine, HookEvent};
        use nebula_core::AgentStatus;
        use std::time::Instant;

        for mut machine in [
            AgentStatusMachine::new(AgentStatus::Fresh, None),
            AgentStatusMachine::launching(),
        ] {
            let mut scanner = ProgressScanner::new();
            let now = Instant::now();
            let initial = machine.status();
            for (payload, expected) in [
                ("idle", initial),
                ("working", AgentStatus::Running),
                ("blocked", AgentStatus::Running),
                ("working", AgentStatus::Running),
                ("idle", AgentStatus::Finished),
                ("working", AgentStatus::Running),
                ("idle", AgentStatus::Finished),
            ] {
                let bytes = format!("\x1b]7501;state={payload}:app=codex\x1b\\");
                if let Some(busy) = scanner.feed(bytes.as_bytes()) {
                    machine.handle(HookEvent::Progress { busy }, None, now);
                }
                assert_eq!(machine.status(), expected, "{payload}");
            }
        }
    }

    #[test]
    fn a_chunk_reports_only_where_it_ended_up() {
        let mut s = ProgressScanner::new();
        assert_eq!(s.feed(b"\x1b]9;4;3;\x07\x1b]9;4;0;\x07"), Some(false));
        assert_eq!(s.feed(b"\x1b]9;4;3;\x07\x1b]9;4;0;\x07"), None);
    }

    #[test]
    fn survives_chunk_boundaries() {
        // The pump coalesces on a byte budget, so a sequence can be split
        // absolutely anywhere.
        let full = b"\x1b]9;4;3;\x07";
        for split in 0..full.len() {
            let mut s = ProgressScanner::new();
            let a = s.feed(&full[..split]);
            let b = s.feed(&full[split..]);
            assert_eq!(
                (a, b),
                (None, Some(true)),
                "split at {split} lost the sequence"
            );
        }
    }

    #[test]
    fn accepts_st_terminator_and_bare_state() {
        assert_eq!(scan(&[b"\x1b]9;4;3\x1b\\"]), vec![Some(true)]);
        assert_eq!(scan(&[b"\x1b]9;4;1;50\x07"]), vec![Some(true)]);
        // 2 = error, 4 = paused: still "advertising progress".
        assert_eq!(scan(&[b"\x1b]9;4;4;\x07"]), vec![Some(true)]);
    }

    #[test]
    fn ignores_other_oscs_and_malformed_payloads() {
        let mut s = ProgressScanner::new();
        // Claude interleaves title updates with progress on every frame.
        assert_eq!(s.feed("\x1b]0;\u{2733} Claude Code\x07".as_bytes()), None);
        assert_eq!(s.feed(b"\x1b]8;;https://example.com\x07"), None);
        assert_eq!(s.feed(b"\x1b]9;5;\x07"), None, "9;5 is not progress");
        assert_eq!(s.feed(b"\x1b]9;4;\x07"), None, "empty state is not a guess");
        assert_eq!(s.feed(b"\x1b]9;4;x;\x07"), None, "non-numeric state");
        assert!(s.busy().is_none(), "nothing above should have set a state");
        assert_eq!(s.feed(b"\x1b]9;4;3;\x07"), Some(true));
    }

    #[test]
    fn long_osc_payload_does_not_grow_the_buffer() {
        let mut s = ProgressScanner::new();
        let title = format!("\x1b]0;{}\x07", "x".repeat(4096));
        assert_eq!(s.feed(title.as_bytes()), None);
        assert!(s.osc.buffered() <= MAX_OSC);
        // …and the scanner still works afterwards.
        assert_eq!(s.feed(b"\x1b]9;4;3;\x07"), Some(true));
    }

    #[test]
    fn aborted_escape_inside_osc_resyncs() {
        let mut s = ProgressScanner::new();
        assert_eq!(
            s.feed(b"\x1b]9;4;3\x1b[0m"),
            None,
            "aborted, not dispatched"
        );
        assert_eq!(s.feed(b"\x1b]9;4;3;\x07"), Some(true));
    }

    #[test]
    fn plain_output_never_trips_it() {
        let mut s = ProgressScanner::new();
        assert_eq!(
            s.feed(b"9;4;0; is just text\r\n\x1b[1;32mgreen\x1b[0m"),
            None
        );
        assert!(s.busy().is_none());
    }
}
