//! OSC 0/2 window-title scanner — the session's name, read straight off
//! the agent's output stream.
//!
//! Claude Code keeps the session's title in the terminal window title
//! (`✳ Fix Login Redirect`: a status glyph, a space, the name) and rewrites
//! it the moment `/rename` runs. That command fires no hook at all, so this
//! scanner is the only immediate sign that the title changed. The daemon
//! treats a sighting as a cue to read the title Claude persisted (see
//! `session_title`), never as the title itself: the same bytes carry
//! Claude's AI-generated summaries and the glyph flips around permission
//! prompts, none of which should rename a row.
//!
//! Verified against Claude Code 2.1.261: `ESC ] 0 ; ✳ <name> BEL` on start
//! (`--name`), on `/rename`, and on a hook reply's `sessionTitle`.

use super::{BEL, ESC};

/// Longest title payload buffered; anything longer (a hyperlink, a base64
/// image) is poisoned and skipped without allocating further.
const MAX_TITLE: usize = 512;

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

/// Tracks the child's window title across chunk boundaries.
#[derive(Debug)]
pub struct TitleScanner {
    state: State,
    buf: Vec<u8>,
    /// Last title the child set; `None` until it sets one.
    title: Option<String>,
    /// Bumped on every title change, so `feed` can report one without
    /// cloning the title on every chunk of ordinary output.
    generation: u64,
}

impl Default for TitleScanner {
    fn default() -> Self {
        Self::new()
    }
}

impl TitleScanner {
    pub fn new() -> Self {
        Self {
            state: State::Ground,
            buf: Vec::new(),
            title: None,
            generation: 0,
        }
    }

    /// The last title the child set, if any.
    pub fn title(&self) -> Option<&str> {
        self.title.as_deref()
    }

    /// Scan a chunk. `Some(title)` when the title at the end of the chunk
    /// is not the one at its start; a title re-set to the same text is not
    /// a change.
    pub fn feed(&mut self, data: &[u8]) -> Option<String> {
        let before = self.generation;
        for &b in data {
            self.step(b);
        }
        if self.generation != before {
            self.title.clone()
        } else {
            None
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
                        // Bail as soon as the payload can't be a title: only
                        // `0;` (icon + window) and `2;` (window) are ours.
                        if self.buf.len() > MAX_TITLE || !prefix_possible(&self.buf) {
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
        let Some(text) = payload
            .strip_prefix(b"0;")
            .or_else(|| payload.strip_prefix(b"2;"))
        else {
            return;
        };
        let text = String::from_utf8_lossy(text).into_owned();
        if self.title.as_deref() != Some(text.as_str()) {
            self.title = Some(text);
            self.generation += 1;
        }
    }
}

/// Could `buf` still grow into a `0;…` or `2;…` payload?
fn prefix_possible(buf: &[u8]) -> bool {
    match buf {
        [] => true,
        [first] => matches!(first, b'0' | b'2'),
        [first, second, ..] => matches!(first, b'0' | b'2') && *second == b';',
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reports_a_window_title_set_with_bel_or_st() {
        let mut s = TitleScanner::new();
        assert_eq!(
            s.feed("\x1b]0;✳ Fix Login Redirect\x07".as_bytes()),
            Some("✳ Fix Login Redirect".into())
        );
        assert_eq!(s.title(), Some("✳ Fix Login Redirect"));
        // OSC 2 (window only) and the ST terminator are the same title.
        assert_eq!(s.feed(b"\x1b]2;plain\x1b\\"), Some("plain".into()));
    }

    #[test]
    fn unchanged_title_and_other_oscs_are_silent() {
        let mut s = TitleScanner::new();
        assert_eq!(s.feed(b"\x1b]0;same\x07"), Some("same".into()));
        // Claude re-sets the same title on every frame, interleaved with
        // progress and hyperlinks; none of that is a change.
        assert_eq!(
            s.feed(b"\x1b]9;4;3;\x07\x1b]0;same\x07\x1b]8;;https://x\x07text\x1b]1;icon\x07"),
            None
        );
        assert_eq!(s.title(), Some("same"));
    }

    #[test]
    fn title_split_across_chunks_arrives_whole() {
        let mut s = TitleScanner::new();
        assert_eq!(s.feed(b"\x1b]0;Fix Lo"), None);
        assert_eq!(s.feed(b"gin\x07"), Some("Fix Login".into()));
    }

    #[test]
    fn two_titles_in_one_chunk_report_the_last() {
        let mut s = TitleScanner::new();
        assert_eq!(
            s.feed(b"\x1b]0;first\x07\x1b]0;second\x07"),
            Some("second".into())
        );
    }

    #[test]
    fn long_payload_is_skipped_without_growing_the_buffer() {
        let mut s = TitleScanner::new();
        let long = format!("\x1b]0;{}\x07", "x".repeat(4096));
        assert_eq!(s.feed(long.as_bytes()), None);
        assert!(s.buf.len() <= MAX_TITLE + 1);
        // …and the scanner resyncs for the next title.
        assert_eq!(s.feed(b"\x1b]0;ok\x07"), Some("ok".into()));
    }

    #[test]
    fn aborted_escape_inside_osc_resyncs() {
        let mut s = TitleScanner::new();
        assert_eq!(
            s.feed(b"\x1b]0;abc\x1b[31m\x1b]0;real\x07"),
            Some("real".into())
        );
    }
}
