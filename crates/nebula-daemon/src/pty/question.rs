//! Harness dialogs no hook reports, read off the screen: a dialog is open
//! while every string of one of its marker groups is on screen.
//!
//! Cursor's ask-question dialog: cursor-agent fires no
//! hook for its `AskQuestion` tool — no `preToolUse`, no `postToolUse`, even
//! though both fire for its shell calls — and drives no OSC 9;4 progress
//! bar, so the dialog it draws is the only sign the turn is waiting on you
//! (verified against cursor-agent 2026.10.01 by capturing hook payloads and
//! raw PTY bytes).
//!
//! The dialog is a bordered box whose last line is a fixed key legend. It
//! is matched on the screen rather than in the byte stream because the CLI
//! (an Ink app) repaints only the tail of its frame when the lines above
//! did not change: a repaint without the legend says nothing about whether
//! the box is still up. A narrow pane wraps the legend inside the border,
//! so the border and the whitespace are dropped before matching.
//!
//! Opening is reported at once. Closing waits for the legend to stay gone
//! across output [`CLOSE_SETTLE`] apart: a macOS PTY hands over at most
//! 1 KiB per read, the box alone is a few KiB, and a repaint split across
//! two flushes shows the box erased in between. Once answered, the CLI's
//! spinner repaints every quarter second, which is what confirms the close;
//! a turn that ends before that is finished by its `stop` hook instead.
//!
//! Antigravity's permission and trust dialogs: agy fires no hook while one
//! waits (a PreToolUse reply would have to decide it), so they are read
//! the same way. Each pairs a headline with the selection legend under its
//! menu — the headline alone can sit in the transcript the agent printed,
//! and the legend alone also heads agy's pickers — and the pair leaves the
//! screen once the dialog is answered (verified on agy 1.4.2 against
//! captured PTY output; the headlines are agy's own strings).

use nebula_core::AgentKind;
use serde::{Deserialize, Serialize};
use std::time::{Duration, Instant};

/// The dialog's key legend, as the CLI prints it.
const LEGEND: &str = "Space select · Enter next/submit · Esc to skip";

/// agy's selection-menu legend, the second half of every agy marker group.
const AGY_MENU_LEGEND: &str = "↑/↓ Navigate ·";

/// The harness dialog to watch for, also persisted across daemon restarts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DialogKind {
    CursorQuestion,
    AntigravityPermission,
}

impl DialogKind {
    /// The screen-scraped dialog this harness draws without a hook.
    pub fn for_agent(kind: AgentKind) -> Option<Self> {
        match kind {
            AgentKind::Cursor => Some(Self::CursorQuestion),
            AgentKind::Antigravity => Some(Self::AntigravityPermission),
            _ => None,
        }
    }

    /// Marker groups: the dialog is open while every string of any one
    /// group is on screen, in order and each within [`MAX_DIALOG_ROWS`] of
    /// the one before — a dialog draws its legend just under its headline,
    /// so a picker's legend below a stale headline that has scrolled up, or
    /// above one, does not count.
    pub fn markers(self) -> &'static [&'static [&'static str]] {
        match self {
            Self::CursorQuestion => &[&[LEGEND]],
            Self::AntigravityPermission => &[
                // Commands, tool calls, access and admin escalation.
                &["Requesting permission", AGY_MENU_LEGEND],
                &["Accept this file edit?", AGY_MENU_LEGEND],
                &["Allow access to this file?", AGY_MENU_LEGEND],
                &["Do you want to proceed?", AGY_MENU_LEGEND],
                // A checkout agy has not been told to trust yet — every
                // fresh worktree, before the session's first turn.
                &[
                    "Do you trust the contents of this project?",
                    AGY_MENU_LEGEND,
                ],
                &["Trust this folder?", AGY_MENU_LEGEND],
            ],
        }
    }
}

/// How long all markers have to stay off the screen before the dialog counts
/// as closed. Above any split repaint's gap; under the spinner's period.
pub const CLOSE_SETTLE: Duration = Duration::from_millis(150);

/// The most screen rows one marker may sit below the one before it: agy's
/// tallest dialog (a command prompt whose options wrap in a narrow pane)
/// with room to spare.
const MAX_DIALOG_ROWS: usize = 24;

/// Side length the screen is floored at — see `pty::cursor`.
const MIN_SIDE: u16 = 2;

pub struct QuestionScanner {
    markers: &'static [&'static [&'static str]],
    /// No scrollback: only what is on screen now matters.
    parser: vt100::Parser,
    open: bool,
    /// While open: when all markers were first seen missing.
    missing_since: Option<Instant>,
    /// A settle re-check is already scheduled for this close.
    settle_requested: bool,
}

impl QuestionScanner {
    /// A screen at the PTY's size with `history` replayed into it. A dialog
    /// already up in it counts as open without an edge: whoever wrote the
    /// history already acted on it. Markers match substrings of the screen
    /// after whitespace and box borders collapse to single spaces.
    pub fn new(
        cols: u16,
        rows: u16,
        history: &[u8],
        markers: &'static [&'static [&'static str]],
    ) -> Self {
        let (cols, rows) = grid_size(cols, rows);
        let mut parser = vt100::Parser::new(rows, cols, 0);
        parser.process(history);
        let open = markers_shown(parser.screen(), markers);
        Self {
            parser,
            markers,
            open,
            missing_since: None,
            settle_requested: false,
        }
    }

    pub fn resize(&mut self, cols: u16, rows: u16) {
        let (cols, rows) = grid_size(cols, rows);
        self.parser.screen_mut().set_size(rows, cols);
    }

    /// Whether the dialog is up as far as this scanner knows.
    pub fn is_open(&self) -> bool {
        self.open
    }

    /// Once per close in progress: true when the dialog has gone missing
    /// and nobody has yet been asked to come back after [`CLOSE_SETTLE`].
    /// A CLI that erases its dialog in one flush and then sits idle (agy at
    /// an empty prompt after its trust dialog) sends no later output to
    /// confirm the close, so the caller re-feeds nothing once it has passed.
    pub fn take_settle_request(&mut self) -> bool {
        let due = self.open && self.missing_since.is_some() && !self.settle_requested;
        self.settle_requested |= due;
        due
    }

    /// Feed a chunk of child output. `Some(open)` only on an edge.
    pub fn feed(&mut self, data: &[u8], now: Instant) -> Option<bool> {
        self.parser.process(data);
        if markers_shown(self.parser.screen(), self.markers) {
            self.missing_since = None;
            self.settle_requested = false;
            if self.open {
                return None;
            }
            self.open = true;
            return Some(true);
        }
        if !self.open {
            return None;
        }
        let since = *self.missing_since.get_or_insert(now);
        if now.duration_since(since) < CLOSE_SETTLE {
            return None;
        }
        self.open = false;
        self.missing_since = None;
        self.settle_requested = false;
        Some(false)
    }
}

fn markers_shown(screen: &vt100::Screen, markers: &[&[&str]]) -> bool {
    let mut text = String::new();
    // The screen row each byte of `text` came from.
    let mut rows = Vec::new();
    let mut row = 0;
    let mut gap = false;
    for c in screen.contents().chars() {
        if c == '\n' {
            row += 1;
        }
        if c.is_whitespace()
            || matches!(
                c,
                '┌' | '┐' | '└' | '┘' | '╭' | '╮' | '╰' | '╯' | '─' | '┃' | '│'
            )
        {
            gap = true;
            continue;
        }
        if gap && !text.is_empty() {
            text.push(' ');
            rows.push(row);
        }
        gap = false;
        text.push(c);
        rows.resize(text.len(), row);
    }
    markers.iter().any(|group| grouped(&text, &rows, group))
}

/// Whether `group` occurs in `text` in order, each marker the nearest one
/// above the next and no more than [`MAX_DIALOG_ROWS`] rows above it.
/// Anchored on every sighting of the last marker, walking back from it.
fn grouped(text: &str, rows: &[usize], group: &[&str]) -> bool {
    let Some((last, before)) = group.split_last() else {
        return false;
    };
    text.match_indices(last).any(|(anchor, _)| {
        let mut at = anchor;
        before
            .iter()
            .rev()
            .all(|marker| match text[..at].rfind(marker) {
                Some(found) if rows[at] - rows[found] <= MAX_DIALOG_ROWS => {
                    at = found;
                    true
                }
                _ => false,
            })
    })
}

fn grid_size(cols: u16, rows: u16) -> (u16, u16) {
    (cols.max(MIN_SIDE), rows.max(MIN_SIDE))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The dialog as cursor-agent paints it at `width` columns: the
    /// previous frame erased, then the box. The legend wraps inside the
    /// border when the box is too narrow for it.
    fn dialog(width: usize) -> Vec<u8> {
        let inner = width - 4;
        let legend = format!("↑/↓ option · ←/→ question · {LEGEND}");
        let mut lines = vec!["Pick a color".to_string(), String::new()];
        lines.push("› [ ] red".into());
        lines.push("  [ ] blue".into());
        lines.push(String::new());
        let mut line = String::new();
        for word in legend.split(' ') {
            if !line.is_empty() && line.chars().count() + 1 + word.chars().count() > inner {
                lines.push(std::mem::take(&mut line));
            }
            if !line.is_empty() {
                line.push(' ');
            }
            line.push_str(word);
        }
        lines.push(line);
        let mut out = String::from("\x1b[2J\x1b[H");
        out.push_str(&format!(" ┌{}┐\r\n", "─".repeat(inner + 2)));
        for l in lines {
            let pad = inner - l.chars().count();
            out.push_str(&format!(" │ {l}{} │\r\n", " ".repeat(pad)));
        }
        out.push_str(&format!(" └{}┘\r\n", "─".repeat(inner + 2)));
        out.push_str("  → Add a follow-up\r\n");
        out.into_bytes()
    }

    /// One spinner repaint: the frame the CLI draws once the dialog is gone.
    fn composing() -> Vec<u8> {
        "\x1b[2J\x1b[H ⠋ Composing\r\n  → Add a follow-up\r\n"
            .as_bytes()
            .to_vec()
    }

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    /// agy's command prompt as agy 1.4.2 draws it (captured off a PTY).
    fn agy_permission() -> Vec<u8> {
        concat!(
            "\x1b[2J\x1b[H● Ran (touch probe.txt) (ctrl+o to expand)\r\n\r\n",
            "Command\r\n", "────────────────────\r\n\r\n",
            "Requesting permission for:\r\n", "   touch probe.txt\r\n\r\n",
            "Run this command?\r\n", "> 1. Yes, run command\r\n",
            "  2. Yes, and always allow in this conversation for commands that start with 'touch'\r\n",
            "  4. No, cancel\r\n\r\n",
            "  ↑/↓ Navigate · tab Amend · ctrl+g edit/expand command\r\n",
            "esc to cancel                    Gemini 3.8 Flash · high",
        )
        .as_bytes()
        .to_vec()
    }

    /// agy once the prompt is answered: the call's line, the working footer.
    fn agy_running() -> Vec<u8> {
        "\x1b[2J\x1b[H● Ran (touch probe.txt) (ctrl+o to expand)\r\n\r\n⠋ Working\r\nesc to cancel"
            .as_bytes()
            .to_vec()
    }

    #[test]
    fn agy_permission_prompt_opens_and_closes_once_answered() {
        let t = Instant::now();
        let markers = DialogKind::AntigravityPermission.markers();
        let mut s = QuestionScanner::new(120, 40, b"", markers);
        assert_eq!(s.feed(&agy_running(), t), None);
        assert_eq!(s.feed(&agy_permission(), t), Some(true));
        assert_eq!(s.feed(&agy_running(), t + ms(1000)), None, "first sighting");
        assert_eq!(s.feed(&agy_running(), t + ms(1200)), Some(false));
        // A narrow pane wraps the legend; it still reads as one line.
        let mut s = QuestionScanner::new(30, 40, b"", markers);
        assert_eq!(s.feed(&agy_permission(), t), Some(true), "wrapped");
    }

    #[test]
    fn agy_trust_dialog_opens_but_half_a_marker_group_does_not() {
        let t = Instant::now();
        let markers = DialogKind::AntigravityPermission.markers();
        let trust = "\x1b[2J\x1b[HDo you trust the contents of this project?\r\n\r\n\
                     > Yes, I trust this folder\r\n  No, exit\r\n\r\n  ↑/↓ Navigate · enter Confirm";
        let mut s = QuestionScanner::new(120, 40, b"", markers);
        assert_eq!(s.feed(trust.as_bytes(), t), Some(true));
        // The headline in the transcript, with no menu under it.
        let mut s = QuestionScanner::new(120, 40, b"", markers);
        let quoted =
            "\x1b[2J\x1b[Hagy shows Requesting permission for: before a command\r\nesc to cancel";
        assert_eq!(s.feed(quoted.as_bytes(), t), None);
        // A picker drawn above a headline left behind by an answered prompt.
        let stale = "\x1b[2J\x1b[HSelect a model\r\n> Gemini\r\n  ↑/↓ Navigate · enter Confirm\r\n\
                     Requesting permission for:";
        assert_eq!(s.feed(stale.as_bytes(), t), None, "legend above headline");
        // A picker's legend, with no permission headline above it.
        let picker = "\x1b[2J\x1b[HSelect a model\r\n> Gemini\r\n  ↑/↓ Navigate · enter Confirm";
        assert_eq!(s.feed(picker.as_bytes(), t), None);
    }

    /// A headline left by an answered prompt that has since scrolled more
    /// than a dialog's height up does not pair with a picker's legend.
    #[test]
    fn a_far_stale_headline_does_not_pair_with_a_later_picker() {
        let t = Instant::now();
        let markers = DialogKind::AntigravityPermission.markers();
        let mut screen = String::from("\x1b[2J\x1b[HRequesting permission for:\r\n");
        for i in 0..30 {
            screen.push_str(&format!("● step {i}\r\n"));
        }
        screen.push_str("Select a model\r\n> Gemini\r\n  ↑/↓ Navigate · enter Confirm");
        let mut s = QuestionScanner::new(120, 40, b"", markers);
        assert_eq!(s.feed(screen.as_bytes(), t), None);
        // The file-access prompt agy 1.4.2 draws is its own headline.
        let access = "\x1b[2J\x1b[HAllow access to this file?\r\n   /etc/hosts\r\n\
                      > 1. Yes, allow access\r\n  2. No\r\n\r\n  ↑/↓ Navigate · enter Confirm";
        assert_eq!(s.feed(access.as_bytes(), t), Some(true));
    }

    /// A close with no output after it asks once for a settle re-check,
    /// which then confirms it; a dialog that comes back cancels the ask.
    #[test]
    fn a_quiet_close_requests_one_settle_recheck() {
        let t = Instant::now();
        let markers = DialogKind::AntigravityPermission.markers();
        let mut s = QuestionScanner::new(120, 40, b"", markers);
        assert!(!s.take_settle_request(), "nothing open");
        s.feed(&agy_permission(), t);
        assert!(!s.take_settle_request(), "still on screen");
        assert_eq!(s.feed(&agy_running(), t + ms(10)), None);
        assert!(s.take_settle_request());
        assert!(!s.take_settle_request(), "asked once per close");
        assert_eq!(s.feed(b"", t + ms(10) + CLOSE_SETTLE), Some(false));
        assert!(!s.take_settle_request());
    }

    #[test]
    fn any_marker_opens_and_switching_markers_keeps_it_open() {
        let t = Instant::now();
        let mut s = QuestionScanner::new(120, 40, b"", &[&["Allow access"], &["Confirm action"]]);
        assert_eq!(s.feed(b"unrelated output", t), None);
        assert_eq!(
            s.feed("\x1b[2J\x1b[H╭─╮\r\n┃ Allow\taccess │\r\n╰─╯".as_bytes(), t),
            Some(true)
        );
        assert_eq!(
            s.feed(b"\x1b[2J\x1b[HConfirm\r\n action", t + ms(500)),
            None
        );
        assert_eq!(s.feed(b"\x1b[2J\x1b[Hgone", t + ms(1000)), None);
        assert_eq!(s.feed(b"", t + ms(1150)), Some(false));
    }

    #[test]
    fn alternate_marker_in_history_is_open_without_an_edge() {
        let mut s = QuestionScanner::new(
            120,
            40,
            b"second marker",
            &[&["first marker"], &["second marker"]],
        );
        assert_eq!(s.feed(b"", Instant::now()), None);
        assert!(s.open);
    }

    #[test]
    fn the_dialog_opens_at_once() {
        let t = Instant::now();
        let mut s = QuestionScanner::new(120, 40, b"", DialogKind::CursorQuestion.markers());
        assert_eq!(s.feed(b" \xe2\xa0\x8b Composing\r\n", t), None);
        assert_eq!(s.feed(&dialog(120), t), Some(true));
        assert_eq!(
            s.feed(&dialog(120), t + ms(500)),
            None,
            "a repaint is no edge"
        );
    }

    #[test]
    fn a_wrapped_legend_still_matches() {
        let t = Instant::now();
        for width in [40, 50, 64] {
            let mut s =
                QuestionScanner::new(width as u16, 40, b"", DialogKind::CursorQuestion.markers());
            assert_eq!(s.feed(&dialog(width), t), Some(true), "{width} cols");
        }
    }

    #[test]
    fn closing_waits_for_the_legend_to_stay_gone() {
        let t = Instant::now();
        let mut s = QuestionScanner::new(120, 40, b"", DialogKind::CursorQuestion.markers());
        s.feed(&dialog(120), t);
        assert_eq!(s.feed(&composing(), t + ms(1000)), None, "first sighting");
        assert_eq!(
            s.feed(&composing(), t + ms(1100)),
            None,
            "inside the settle"
        );
        assert_eq!(s.feed(&composing(), t + ms(1250)), Some(false));
        assert_eq!(s.feed(&composing(), t + ms(1500)), None);
    }

    #[test]
    fn a_repaint_split_across_flushes_is_not_a_close() {
        let t = Instant::now();
        let mut s = QuestionScanner::new(120, 40, b"", DialogKind::CursorQuestion.markers());
        s.feed(&dialog(120), t);
        let frame = dialog(120);
        let (head, tail) = frame.split_at(frame.len() / 2);
        assert_eq!(s.feed(head, t + ms(1000)), None);
        assert!(
            !markers_shown(s.parser.screen(), s.markers),
            "the head erased the box"
        );
        assert_eq!(s.feed(tail, t + ms(1001)), None);
        // The half-drawn moment does not count toward a later close.
        assert_eq!(s.feed(&composing(), t + ms(1200)), None);
    }

    #[test]
    fn a_tail_only_repaint_leaves_it_open() {
        let t = Instant::now();
        let mut s = QuestionScanner::new(120, 40, b"", DialogKind::CursorQuestion.markers());
        s.feed(&dialog(120), t);
        // Ink repaints just the last line when nothing above it changed.
        let tail = b"\x1b[1A\x1b[2K\x1b[G  \xe2\x86\x92 Add a follow-up \xc2\xb7 6%\r\n";
        assert_eq!(s.feed(tail, t + ms(500)), None);
        assert_eq!(s.feed(tail, t + ms(900)), None);
    }

    #[test]
    fn a_dialog_in_the_history_is_open_without_an_edge() {
        let t = Instant::now();
        let mut s =
            QuestionScanner::new(120, 40, &dialog(120), DialogKind::CursorQuestion.markers());
        assert_eq!(s.feed(&dialog(120), t), None);
        s.feed(&composing(), t + ms(100));
        assert_eq!(s.feed(&composing(), t + ms(400)), Some(false));
    }

    #[test]
    fn a_squeezed_pane_does_not_panic() {
        let t = Instant::now();
        for (cols, rows) in [(0, 0), (1, 1), (2, 2)] {
            let mut s = QuestionScanner::new(
                cols,
                rows,
                &dialog(120),
                DialogKind::CursorQuestion.markers(),
            );
            s.resize(cols, rows);
            s.feed(&dialog(120), t);
        }
    }
}
