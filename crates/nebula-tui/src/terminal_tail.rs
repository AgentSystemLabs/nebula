//! The last lines a TERMINAL printed, for its card on the grid.
//!
//! The daemon keeps bytes, not a screen (its `pty::cursor` says why: the
//! emulator lives in the client), so the grid lays the end of a
//! terminal's ring out for itself — a throwaway screen the PTY's size,
//! the bytes run through it, the rows read back the way the pane would
//! show them, minus the colour. The pane's own screen goes through the
//! same [`screen_tail`] for the terminal it is on.

/// The rows a card shows of a screen, at most `keep`, blank rows left
/// out: on the primary screen the last ones up to the cursor's — a
/// shell's prompt and what came before it — and on the alternate screen
/// (an editor, a pager, `htop`) the first, where such a program keeps its
/// title and its opening lines. Each row is the screen's width with its
/// trailing blanks trimmed; the card cuts them to its own.
pub fn screen_tail(screen: &vt100::Screen, keep: usize) -> Vec<String> {
    let (_, cols) = screen.size();
    let rows = screen.rows(0, cols).map(|r| r.trim_end().to_string());
    if screen.alternate_screen() {
        return rows.filter(|r| !r.is_empty()).take(keep).collect();
    }
    let (cursor_row, _) = screen.cursor_position();
    let mut out: Vec<String> = rows
        .take(usize::from(cursor_row) + 1)
        .filter(|r| !r.is_empty())
        .collect();
    let start = out.len().saturating_sub(keep);
    out.drain(..start);
    out
}

/// `data`, the end of a ring, laid out on a fresh `cols`×`rows` screen and
/// read back with [`screen_tail`]. The bytes start wherever the cut fell —
/// mid-sequence, mid-character — and a screen shrugs at that: a junk
/// character on its first row, which the tail seldom reaches. No
/// scrollback: only what is on the screen is read.
pub fn parse_tail(data: &[u8], cols: u16, rows: u16, keep: usize) -> Vec<String> {
    // vt100's grid arithmetic needs two cells a side (the daemon's
    // `CursorTracker` floors the same way).
    let mut parser = vt100::Parser::new(rows.max(2), cols.max(2), 0);
    parser.process(data);
    screen_tail(parser.screen(), keep)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_last_lines_up_to_the_prompt_blank_rows_left_out() {
        let data = b"$ npm test\r\nok 1\r\nok 2\r\n\r\n$ ";
        assert_eq!(
            parse_tail(data, 80, 24, 4),
            ["$ npm test", "ok 1", "ok 2", "$"]
        );
        assert_eq!(parse_tail(data, 80, 24, 2), ["ok 2", "$"]);
    }

    #[test]
    fn colour_and_erasures_come_out_as_text() {
        let data = b"\x1b[31mred\x1b[0m line\r\n\x1b[2K$ \x1b[?25h";
        assert_eq!(parse_tail(data, 80, 24, 4), ["red line", "$"]);
    }

    #[test]
    fn rows_wrap_at_the_screens_width() {
        assert_eq!(
            parse_tail(b"abcdefgh\r\n$ ", 5, 4, 4),
            ["abcde", "fgh", "$"]
        );
    }

    #[test]
    fn rows_below_the_cursor_are_not_the_tail() {
        // A program that moved the cursor back up: what is under it was
        // printed earlier and is not what the shell is on now.
        let data = b"one\r\ntwo\r\nthree\r\n\x1b[2;1Hnow";
        assert_eq!(parse_tail(data, 80, 24, 4), ["one", "now"]);
    }

    #[test]
    fn an_alternate_screen_shows_its_top() {
        let data =
            b"$ vim\r\n\x1b[?1049h\x1b[H\x1b[2J\r\n\r\ntitle\r\nbody\r\nmore\x1b[24;1Hstatus";
        assert_eq!(parse_tail(data, 80, 24, 2), ["title", "body"]);
    }

    #[test]
    fn a_cut_mid_sequence_only_muddles_the_first_row() {
        let data = b"1;31mjunk\r\nreal\r\n$ ";
        assert_eq!(parse_tail(data, 80, 24, 2), ["real", "$"]);
    }

    #[test]
    fn an_empty_ring_is_no_lines_and_a_tiny_screen_no_panic() {
        assert!(parse_tail(b"", 80, 24, 4).is_empty());
        assert_eq!(parse_tail(b"hi\r\n$ ", 0, 0, 4), ["hi", "$"]);
    }

    #[test]
    fn more_than_a_screenful_keeps_the_end() {
        let mut data = Vec::new();
        for i in 0..100 {
            data.extend_from_slice(format!("line {i}\r\n").as_bytes());
        }
        data.extend_from_slice(b"$ ");
        assert_eq!(parse_tail(&data, 80, 10, 3), ["line 98", "line 99", "$"]);
    }
}
