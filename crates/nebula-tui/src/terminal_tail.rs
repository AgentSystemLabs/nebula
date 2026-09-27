//! The last lines a TERMINAL printed, for its card on the grid.
//!
//! The daemon keeps bytes, not a screen (its `pty::cursor` says why: the
//! emulator lives in the client), so the grid lays the end of a
//! terminal's ring out for itself — a throwaway screen the PTY's size,
//! the bytes run through it, the rows read back the way the pane would
//! show them: the shell's own colours, bold, underline and reverse
//! video, and its cursor, so the card reads as a small terminal rather
//! than a transcript. The pane's own screen goes through the same
//! [`screen_tail`] for the terminal it is on.

use ratatui::style::{Color, Modifier, Style};

/// One row of a terminal's tail, as its screen painted it: the text in
/// runs of one look each — the colours the program asked for, `None`
/// where it left the terminal's default, so the card's own theme shows
/// through — and the column the shell's cursor sits in, when it is on
/// this row and showing.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TailRow {
    pub runs: Vec<(String, Style)>,
    pub cursor: Option<u16>,
}

impl TailRow {
    /// The row's text, the looks dropped.
    pub fn text(&self) -> String {
        self.runs.iter().map(|(s, _)| s.as_str()).collect()
    }
}

#[cfg(test)]
impl From<&str> for TailRow {
    fn from(s: &str) -> Self {
        TailRow {
            runs: vec![(s.to_string(), Style::default())],
            cursor: None,
        }
    }
}

/// The rows a card shows of a screen, at most `keep`, blank rows left
/// out: on the primary screen the last ones up to the cursor's — a
/// shell's prompt and what came before it — and on the alternate screen
/// (an editor, a pager, `htop`) the first, where such a program keeps its
/// title and its opening lines. Each row is the screen's width with its
/// trailing blanks trimmed — a blank on a coloured background is not
/// one, so a status bar keeps its fill; the card cuts them to its own.
pub fn screen_tail(screen: &vt100::Screen, keep: usize) -> Vec<TailRow> {
    let (rows, _) = screen.size();
    let (cursor_row, cursor_col) = screen.cursor_position();
    let cursor = (!screen.hide_cursor()).then_some((cursor_row, cursor_col));
    let read = |r: u16| {
        tail_row(
            screen,
            r,
            cursor.filter(|(row, _)| *row == r).map(|(_, col)| col),
        )
    };
    if screen.alternate_screen() {
        return (0..rows)
            .map(read)
            .filter(|r| !r.runs.is_empty())
            .take(keep)
            .collect();
    }
    let mut out: Vec<TailRow> = (0..=cursor_row.min(rows.saturating_sub(1)))
        .map(read)
        .filter(|r| !r.runs.is_empty())
        .collect();
    let start = out.len().saturating_sub(keep);
    out.drain(..start);
    out
}

/// Screen row `row` as runs of one look, its trailing blanks dropped.
fn tail_row(screen: &vt100::Screen, row: u16, cursor: Option<u16>) -> TailRow {
    let (_, cols) = screen.size();
    let cells: Vec<&vt100::Cell> = (0..cols)
        .filter_map(|c| screen.cell(row, c))
        .filter(|c| !c.is_wide_continuation())
        .collect();
    let end = cells.iter().rposition(|c| !blank(c)).map_or(0, |i| i + 1);
    let mut runs: Vec<(String, Style)> = Vec::new();
    for cell in &cells[..end] {
        let text = if cell.has_contents() {
            cell.contents()
        } else {
            " "
        };
        let style = cell_style(cell);
        match runs.last_mut() {
            Some((s, last)) if *last == style => s.push_str(text),
            _ => runs.push((text.to_string(), style)),
        }
    }
    TailRow { runs, cursor }
}

/// A cell that shows nothing: no character, or a space, on the
/// terminal's own background and not in reverse video.
fn blank(cell: &vt100::Cell) -> bool {
    cell.contents().trim().is_empty() && cell.bgcolor() == vt100::Color::Default && !cell.inverse()
}

/// A cell's look as the pane's renderer paints it (`tui_term`), except
/// that a default colour is left unset rather than reset — the card's
/// text colour and focus tint are what a terminal's defaults are there.
fn cell_style(cell: &vt100::Cell) -> Style {
    let color = |c: vt100::Color| match c {
        vt100::Color::Default => None,
        vt100::Color::Idx(i) => Some(Color::Indexed(i)),
        vt100::Color::Rgb(r, g, b) => Some(Color::Rgb(r, g, b)),
    };
    let mut style = Style {
        fg: color(cell.fgcolor()),
        bg: color(cell.bgcolor()),
        ..Style::default()
    };
    for (on, m) in [
        (cell.bold(), Modifier::BOLD),
        (cell.dim(), Modifier::DIM),
        (cell.italic(), Modifier::ITALIC),
        (cell.underline(), Modifier::UNDERLINED),
        (cell.inverse(), Modifier::REVERSED),
    ] {
        if on {
            style = style.add_modifier(m);
        }
    }
    style
}

/// `data`, the end of a ring, laid out on a fresh `cols`×`rows` screen and
/// read back with [`screen_tail`]. The bytes start wherever the cut fell —
/// mid-sequence, mid-character — and a screen shrugs at that: a junk
/// character on its first row, which the tail seldom reaches. No
/// scrollback: only what is on the screen is read.
pub fn parse_tail(data: &[u8], cols: u16, rows: u16, keep: usize) -> Vec<TailRow> {
    // vt100's grid arithmetic needs two cells a side (the daemon's
    // `CursorTracker` floors the same way).
    let mut parser = vt100::Parser::new(rows.max(2), cols.max(2), 0);
    parser.process(data);
    screen_tail(parser.screen(), keep)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The rows' text, their looks dropped.
    fn text(data: &[u8], cols: u16, rows: u16, keep: usize) -> Vec<String> {
        parse_tail(data, cols, rows, keep)
            .iter()
            .map(TailRow::text)
            .collect()
    }

    #[test]
    fn the_last_lines_up_to_the_prompt_blank_rows_left_out() {
        let data = b"$ npm test\r\nok 1\r\nok 2\r\n\r\n$ ";
        assert_eq!(text(data, 80, 24, 4), ["$ npm test", "ok 1", "ok 2", "$"]);
        assert_eq!(text(data, 80, 24, 2), ["ok 2", "$"]);
    }

    #[test]
    fn colour_and_erasures_come_out_as_text() {
        let data = b"\x1b[31mred\x1b[0m line\r\n\x1b[2K$ \x1b[?25h";
        assert_eq!(text(data, 80, 24, 4), ["red line", "$"]);
    }

    #[test]
    fn rows_wrap_at_the_screens_width() {
        assert_eq!(text(b"abcdefgh\r\n$ ", 5, 4, 4), ["abcde", "fgh", "$"]);
    }

    #[test]
    fn rows_below_the_cursor_are_not_the_tail() {
        // A program that moved the cursor back up: what is under it was
        // printed earlier and is not what the shell is on now.
        let data = b"one\r\ntwo\r\nthree\r\n\x1b[2;1Hnow";
        assert_eq!(text(data, 80, 24, 4), ["one", "now"]);
    }

    #[test]
    fn an_alternate_screen_shows_its_top() {
        let data =
            b"$ vim\r\n\x1b[?1049h\x1b[H\x1b[2J\r\n\r\ntitle\r\nbody\r\nmore\x1b[24;1Hstatus";
        assert_eq!(text(data, 80, 24, 2), ["title", "body"]);
    }

    #[test]
    fn a_cut_mid_sequence_only_muddles_the_first_row() {
        let data = b"1;31mjunk\r\nreal\r\n$ ";
        assert_eq!(text(data, 80, 24, 2), ["real", "$"]);
    }

    #[test]
    fn an_empty_ring_is_no_lines_and_a_tiny_screen_no_panic() {
        assert!(text(b"", 80, 24, 4).is_empty());
        assert_eq!(text(b"hi\r\n$ ", 0, 0, 4), ["hi", "$"]);
    }

    #[test]
    fn more_than_a_screenful_keeps_the_end() {
        let mut data = Vec::new();
        for i in 0..100 {
            data.extend_from_slice(format!("line {i}\r\n").as_bytes());
        }
        data.extend_from_slice(b"$ ");
        assert_eq!(text(&data, 80, 10, 3), ["line 98", "line 99", "$"]);
    }

    /// The card reads as a terminal: each run keeps the colour and the
    /// weight the program printed it in, and what it left at the
    /// terminal's default stays unset for the card's theme to fill.
    #[test]
    fn colour_and_weight_come_out_as_runs() {
        let data = b"\x1b[1;32m\xe2\x9c\x93\x1b[0m built \x1b[38;5;208min\x1b[0m \x1b[38;2;1;2;3;4m2s\x1b[0m\r\n$ ";
        let rows = parse_tail(data, 80, 24, 4);
        assert_eq!(
            rows[0].runs,
            vec![
                (
                    "\u{2713}".to_string(),
                    Style::default()
                        .fg(Color::Indexed(2))
                        .add_modifier(Modifier::BOLD)
                ),
                (" built ".to_string(), Style::default()),
                ("in".to_string(), Style::default().fg(Color::Indexed(208))),
                (" ".to_string(), Style::default()),
                (
                    "2s".to_string(),
                    Style::default()
                        .fg(Color::Rgb(1, 2, 3))
                        .add_modifier(Modifier::UNDERLINED)
                ),
            ]
        );
        assert_eq!(rows[0].runs[1].1.fg, None, "the default is left unset");
    }

    /// A blank on a coloured background, or in reverse video, is paint —
    /// a status bar's fill, zsh's `%` mark — and stays; plain trailing
    /// blanks go.
    #[test]
    fn a_coloured_blank_is_not_a_blank() {
        let data = b"\x1b[44m bar   \x1b[0m   \r\n\x1b[7m \x1b[0m\r\n$ ";
        let rows = parse_tail(data, 80, 24, 4);
        assert_eq!(rows[0].text(), " bar   ");
        assert_eq!(rows[0].runs[0].1.bg, Some(Color::Indexed(4)));
        assert_eq!(rows[1].text(), " ");
        assert!(rows[1].runs[0].1.add_modifier.contains(Modifier::REVERSED));
    }

    /// The shell's cursor rides its row — at the prompt, past the
    /// trimmed text — and not once the program hides it.
    #[test]
    fn the_cursor_rides_its_row_until_it_is_hidden() {
        let rows = parse_tail(b"ok\r\n$ ", 80, 24, 4);
        assert_eq!(rows[0].cursor, None);
        assert_eq!(rows[1].cursor, Some(2));
        let rows = parse_tail(b"ok\r\n$ \x1b[?25l", 80, 24, 4);
        assert_eq!(rows[1].cursor, None);
    }

    /// A wide character is one run's text, its second cell not a space.
    #[test]
    fn a_wide_character_takes_no_second_blank() {
        assert_eq!(
            text("\u{4f60}\u{597d} ok\r\n$ ".as_bytes(), 80, 24, 4)[0],
            "\u{4f60}\u{597d} ok"
        );
    }
}
