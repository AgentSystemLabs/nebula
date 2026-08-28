//! Kitty keyboard protocol negotiation, tmux-style: the child talks to a
//! virtual terminal (the ring + client-side vt100 parser), so nobody would
//! ever answer its `CSI ? u` support query. We scan the output stream here,
//! answer queries ourselves, and track the child's flag stack so attached
//! clients know how to encode keys for it.
//!
//! Also answers DA1 (`CSI c`) — the common detection recipe is "send the
//! kitty query then DA1, protocol is supported iff the kitty reply arrives
//! before the DA1 reply", which needs a DA1 reply to terminate promptly.

use super::ESC;

/// Max nesting the spec suggests implementations may cap the stack at.
const MAX_STACK: usize = 32;
/// A real kitty sequence has short params; anything longer is not for us.
const MAX_PARAMS: usize = 16;

/// What the pump should do in response to scanned output.
#[derive(Debug, Default, PartialEq)]
pub struct ScanActions {
    /// Bytes to write to the child's stdin (query/DA1 replies).
    pub reply: Vec<u8>,
    /// Set when the effective flags changed; broadcast to attached clients.
    pub flags_changed: Option<u8>,
}

#[derive(Debug, Clone, Copy)]
enum State {
    Ground,
    Esc,
    /// Inside a CSI; params (0x30..=0x3F) collect, intermediates (0x20..=0x2F)
    /// or overflow poison the sequence (we only relay interesting finals).
    Csi {
        poisoned: bool,
    },
}

pub struct KittyScanner {
    state: State,
    params: Vec<u8>,
    stack: Vec<u8>,
}

impl Default for KittyScanner {
    fn default() -> Self {
        Self::new()
    }
}

impl KittyScanner {
    pub fn new() -> Self {
        Self {
            state: State::Ground,
            params: Vec::new(),
            stack: Vec::new(),
        }
    }

    /// Effective flags: top of the stack, or 0 (legacy) when empty.
    pub fn flags(&self) -> u8 {
        self.stack.last().copied().unwrap_or(0)
    }

    /// Scan a chunk of child output. Sequences split across chunks are fine —
    /// the state machine persists between calls.
    pub fn feed(&mut self, data: &[u8]) -> ScanActions {
        let mut actions = ScanActions::default();
        let before = self.flags();
        for &b in data {
            self.step(b, &mut actions);
        }
        let after = self.flags();
        if after != before {
            actions.flags_changed = Some(after);
        }
        actions
    }

    fn step(&mut self, b: u8, actions: &mut ScanActions) {
        match self.state {
            State::Ground => {
                if b == ESC {
                    self.state = State::Esc;
                }
            }
            State::Esc => {
                if b == b'[' {
                    self.params.clear();
                    self.state = State::Csi { poisoned: false };
                } else {
                    // Includes ESC ESC; any other escape kind is not a CSI.
                    self.state = if b == ESC { State::Esc } else { State::Ground };
                }
            }
            State::Csi { poisoned } => match b {
                0x30..=0x3F => {
                    if self.params.len() < MAX_PARAMS {
                        self.params.push(b);
                    } else {
                        self.state = State::Csi { poisoned: true };
                    }
                }
                0x20..=0x2F => self.state = State::Csi { poisoned: true },
                0x40..=0x7E => {
                    if !poisoned {
                        self.dispatch(b, actions);
                    }
                    self.state = State::Ground;
                }
                // Cancelled / malformed sequence.
                _ => self.state = State::Ground,
            },
        }
    }

    fn dispatch(&mut self, final_byte: u8, actions: &mut ScanActions) {
        let params = std::mem::take(&mut self.params);
        match final_byte {
            b'u' => match params.split_first() {
                // CSI ? u — "do you speak kitty?" Reply with current flags.
                Some((b'?', [])) => {
                    actions
                        .reply
                        .extend_from_slice(format!("\x1b[?{}u", self.flags()).as_bytes());
                }
                // CSI > flags u — push (flags default 0).
                Some((b'>', rest)) => {
                    let flags = parse_num(rest).unwrap_or(0) as u8;
                    if self.stack.len() < MAX_STACK {
                        self.stack.push(flags);
                    } else {
                        // Spec: at the cap, the oldest entry is evicted.
                        self.stack.remove(0);
                        self.stack.push(flags);
                    }
                }
                // CSI < n u — pop n (default 1).
                Some((b'<', rest)) => {
                    let n = parse_num(rest).unwrap_or(1).max(1) as usize;
                    for _ in 0..n {
                        if self.stack.pop().is_none() {
                            break;
                        }
                    }
                }
                // CSI = flags ; mode u — modify the current entry in place.
                Some((b'=', rest)) => {
                    let mut it = rest.split(|&b| b == b';');
                    let flags = it.next().and_then(parse_num).unwrap_or(0) as u8;
                    let mode = it.next().and_then(parse_num).unwrap_or(1);
                    if self.stack.is_empty() {
                        self.stack.push(0);
                    }
                    let top = self.stack.last_mut().expect("just ensured non-empty");
                    match mode {
                        1 => *top = flags,
                        2 => *top |= flags,
                        3 => *top &= !flags,
                        _ => {}
                    }
                }
                // Bare CSI u = SCO restore-cursor; not ours.
                _ => {}
            },
            // DA1 (CSI c / CSI 0 c): claim VT102 so detection loops terminate.
            b'c' if params.is_empty() || params == [b'0'] => {
                actions.reply.extend_from_slice(b"\x1b[?6c");
            }
            _ => {}
        }
    }
}

fn parse_num(bytes: &[u8]) -> Option<u32> {
    if bytes.is_empty() || bytes.len() > 9 || !bytes.iter().all(u8::is_ascii_digit) {
        return None;
    }
    std::str::from_utf8(bytes).ok()?.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn query_gets_reply_with_current_flags() {
        let mut s = KittyScanner::new();
        let a = s.feed(b"\x1b[?u");
        assert_eq!(a.reply, b"\x1b[?0u");
        s.feed(b"\x1b[>5u");
        let a = s.feed(b"\x1b[?u");
        assert_eq!(a.reply, b"\x1b[?5u");
    }

    #[test]
    fn push_pop_track_flags() {
        let mut s = KittyScanner::new();
        let a = s.feed(b"\x1b[>1u");
        assert_eq!(a.flags_changed, Some(1));
        assert_eq!(s.flags(), 1);
        let a = s.feed(b"\x1b[>15u");
        assert_eq!(a.flags_changed, Some(15));
        let a = s.feed(b"\x1b[<u");
        assert_eq!(a.flags_changed, Some(1));
        let a = s.feed(b"\x1b[<5u");
        assert_eq!(a.flags_changed, Some(0));
        assert_eq!(s.flags(), 0);
    }

    #[test]
    fn set_mode_modifies_in_place() {
        let mut s = KittyScanner::new();
        s.feed(b"\x1b[=5;1u");
        assert_eq!(s.flags(), 5);
        s.feed(b"\x1b[=2;2u");
        assert_eq!(s.flags(), 7);
        s.feed(b"\x1b[=1;3u");
        assert_eq!(s.flags(), 6);
    }

    #[test]
    fn sequences_split_across_chunks() {
        let mut s = KittyScanner::new();
        assert_eq!(s.feed(b"hello \x1b"), ScanActions::default());
        assert_eq!(s.feed(b"[>"), ScanActions::default());
        let a = s.feed(b"1u world");
        assert_eq!(a.flags_changed, Some(1));
    }

    #[test]
    fn unrelated_sequences_ignored() {
        let mut s = KittyScanner::new();
        // Colors, cursor moves, bare CSI u (SCO restore cursor), long params.
        let a = s.feed(b"\x1b[31mred\x1b[H\x1b[u\x1b[12345678901234567890u");
        assert_eq!(a, ScanActions::default());
        assert_eq!(s.flags(), 0);
    }

    #[test]
    fn da1_gets_vt102_reply() {
        let mut s = KittyScanner::new();
        assert_eq!(s.feed(b"\x1b[c").reply, b"\x1b[?6c");
        assert_eq!(s.feed(b"\x1b[0c").reply, b"\x1b[?6c");
        // DA2 / DA-with-args are not answered.
        assert_eq!(s.feed(b"\x1b[>c"), ScanActions::default());
    }
}
