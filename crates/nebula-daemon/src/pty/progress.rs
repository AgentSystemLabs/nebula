//! OSC 9;4 progress scanner — the busy/idle signal nebula reads straight
//! off the agent's output stream.
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

use super::{BEL, ESC};

/// Longest OSC payload we will buffer. `9;4;<state>;<pct>` is far shorter;
/// anything longer (a title, a hyperlink, a base64 image) is poisoned and
/// skipped without allocating.
const MAX_OSC: usize = 24;

#[derive(Debug, Clone, Copy, PartialEq)]
enum State {
    Ground,
    Esc,
    /// Inside an OSC payload. `poisoned` sequences are consumed to their
    /// terminator and discarded.
    Osc {
        poisoned: bool,
    },
    /// Saw ESC inside an OSC: `ESC \` terminates (ST), anything else aborts.
    OscEsc {
        poisoned: bool,
    },
}

/// Tracks the child's advertised progress state across chunk boundaries.
#[derive(Debug)]
pub struct ProgressScanner {
    state: State,
    buf: Vec<u8>,
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
            state: State::Ground,
            buf: Vec::new(),
            busy: None,
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
            self.step(b);
        }
        match self.busy {
            after if after != before => after,
            _ => None,
        }
    }

    fn step(&mut self, b: u8) {
        match self.state {
            State::Ground => {
                if b == ESC {
                    self.state = State::Esc;
                }
            }
            State::Esc => {
                if b == b']' {
                    self.buf.clear();
                    self.state = State::Osc { poisoned: false };
                } else {
                    self.state = if b == ESC { State::Esc } else { State::Ground };
                }
            }
            State::Osc { poisoned } => match b {
                BEL => {
                    if !poisoned {
                        self.dispatch();
                    }
                    self.buf.clear();
                    self.state = State::Ground;
                }
                ESC => self.state = State::OscEsc { poisoned },
                _ => {
                    if !poisoned {
                        self.buf.push(b);
                        // Bail as soon as the payload can't be ours: only the
                        // `9;4;` prefix is worth carrying.
                        if self.buf.len() > MAX_OSC || !prefix_possible(&self.buf) {
                            self.buf.clear();
                            self.state = State::Osc { poisoned: true };
                        }
                    }
                }
            },
            State::OscEsc { poisoned } => {
                if b == b'\\' {
                    if !poisoned {
                        self.dispatch();
                    }
                    self.buf.clear();
                    self.state = State::Ground;
                } else {
                    // Aborted mid-OSC; ESC ESC restarts the escape.
                    self.buf.clear();
                    self.state = if b == ESC { State::Esc } else { State::Ground };
                }
            }
        }
    }

    fn dispatch(&mut self) {
        let payload = std::mem::take(&mut self.buf);
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

/// Could `buf` still grow into a `9;4;…` payload?
fn prefix_possible(buf: &[u8]) -> bool {
    const PREFIX: &[u8] = b"9;4;";
    let n = buf.len().min(PREFIX.len());
    buf[..n] == PREFIX[..n]
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
        assert!(s.buf.len() <= MAX_OSC);
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
