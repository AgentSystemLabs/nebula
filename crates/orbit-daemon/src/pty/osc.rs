//! OSC framing shared by the output scanners (`title`, `progress`): the
//! state machine that finds `ESC ] <payload> BEL` and `ESC ] <payload>
//! ESC \` in the agent's output, across chunk boundaries, and hands each
//! finished payload to its scanner. What a payload means is the scanner's
//! business; which payloads are worth buffering at all is too — the
//! framer only enforces it.

use super::{BEL, ESC};

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

/// One scanner's OSC framing. A payload is buffered only while it stays
/// within `max` bytes and `prefix_possible` still holds; the moment either
/// fails it is poisoned — consumed to its terminator and discarded —
/// without allocating further.
#[derive(Debug)]
pub(super) struct OscFramer {
    state: State,
    buf: Vec<u8>,
    max: usize,
    prefix_possible: fn(&[u8]) -> bool,
}

impl OscFramer {
    pub(super) fn new(max: usize, prefix_possible: fn(&[u8]) -> bool) -> Self {
        Self {
            state: State::Ground,
            buf: Vec::new(),
            max,
            prefix_possible,
        }
    }

    /// Bytes of the payload currently buffered.
    #[cfg(test)]
    pub(super) fn buffered(&self) -> usize {
        self.buf.len()
    }

    /// Feed one byte; `Some(payload)` when it terminates an OSC this
    /// scanner kept.
    pub(super) fn step(&mut self, b: u8) -> Option<Vec<u8>> {
        match self.state {
            State::Ground => {
                if b == ESC {
                    self.state = State::Esc;
                }
                None
            }
            State::Esc => {
                if b == b']' {
                    self.buf.clear();
                    self.state = State::Osc { poisoned: false };
                } else {
                    self.state = if b == ESC { State::Esc } else { State::Ground };
                }
                None
            }
            State::Osc { poisoned } => match b {
                BEL => self.finish(poisoned),
                ESC => {
                    self.state = State::OscEsc { poisoned };
                    None
                }
                _ => {
                    if !poisoned {
                        self.buf.push(b);
                        // Bail as soon as the payload can't be the scanner's.
                        if self.buf.len() > self.max || !(self.prefix_possible)(&self.buf) {
                            self.buf.clear();
                            self.state = State::Osc { poisoned: true };
                        }
                    }
                    None
                }
            },
            State::OscEsc { poisoned } => {
                if b == b'\\' {
                    self.finish(poisoned)
                } else {
                    // Aborted mid-OSC; ESC ESC restarts the escape.
                    self.buf.clear();
                    self.state = if b == ESC { State::Esc } else { State::Ground };
                    None
                }
            }
        }
    }

    /// The OSC ended: back to ground, with the payload unless it was
    /// poisoned.
    fn finish(&mut self, poisoned: bool) -> Option<Vec<u8>> {
        self.state = State::Ground;
        if poisoned {
            self.buf.clear();
            None
        } else {
            Some(std::mem::take(&mut self.buf))
        }
    }
}
