//! URL detection over the visible vt100 screen, for underlining links in the
//! terminal pane and opening them on ⌥click. vt100 0.15 drops OSC 8
//! hyperlinks, so links are found by scanning the rendered cell text.

/// A detected http(s) URL and the screen cells it occupies, as inclusive
/// `(row, col_start, col_end)` segments — one per screen row, so a link
/// wrapped at the pane edge carries a segment per visual row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TermLink {
    pub url: String,
    pub segments: Vec<(u16, u16, u16)>,
}

impl TermLink {
    /// Whether the pane-relative `(col, row)` cell lies on this link.
    pub fn contains(&self, cell: (u16, u16)) -> bool {
        let (col, row) = cell;
        self.segments
            .iter()
            .any(|&(r, c0, c1)| r == row && (c0..=c1).contains(&col))
    }
}

/// Scan the visible screen for http(s) URLs. Consecutive rows are joined
/// while `row_wrapped` says the line continues, so a URL split at the pane
/// edge still matches as one link.
pub fn visible_links(screen: &vt100::Screen) -> Vec<TermLink> {
    let (rows, cols) = screen.size();
    let mut links = Vec::new();
    // One logical line at a time: each entry is a char plus the cell it came
    // from, so match offsets map straight back to screen coordinates.
    let mut line: Vec<(char, (u16, u16))> = Vec::new();
    for row in 0..rows {
        for col in 0..cols {
            let Some(cell) = screen.cell(row, col) else {
                continue;
            };
            if cell.is_wide_continuation() {
                continue;
            }
            let contents = cell.contents();
            // An empty cell is a gap — it terminates any URL run.
            let ch = contents.chars().next().unwrap_or(' ');
            line.push((ch, (col, row)));
        }
        // `wrapped` means this row flows into the next with no line break.
        if !screen.row_wrapped(row) {
            scan_line(&line, &mut links);
            line.clear();
        }
    }
    if !line.is_empty() {
        scan_line(&line, &mut links);
    }
    links
}

fn scan_line(line: &[(char, (u16, u16))], links: &mut Vec<TermLink>) {
    let n = line.len();
    let mut i = 0;
    while i < n {
        let Some(scheme_len) = scheme_at(line, i) else {
            i += 1;
            continue;
        };
        let mut end = i + scheme_len;
        while end < n && is_url_char(line[end].0) {
            end += 1;
        }
        // Trailing punctuation is almost always sentence/markup context
        // ("see https://x.dev." / "(https://x.dev)"), not part of the URL.
        while end > i + scheme_len
            && matches!(
                line[end - 1].0,
                '.' | ',' | ';' | ':' | '!' | '?' | ')' | ']' | '}'
            )
        {
            end -= 1;
        }
        if end == i + scheme_len {
            // Bare scheme with no host — not a link.
            i = end;
            continue;
        }
        let url: String = line[i..end].iter().map(|&(c, _)| c).collect();
        let mut segments: Vec<(u16, u16, u16)> = Vec::new();
        for &(_, (col, row)) in &line[i..end] {
            match segments.last_mut() {
                Some(seg) if seg.0 == row && seg.2 + 1 == col => seg.2 = col,
                _ => segments.push((row, col, col)),
            }
        }
        links.push(TermLink { url, segments });
        i = end;
    }
}

/// Length of the URL scheme starting at `at`, if any (case-insensitive).
fn scheme_at(line: &[(char, (u16, u16))], at: usize) -> Option<usize> {
    for scheme in ["https://", "http://"] {
        let len = scheme.len();
        if line.len() >= at + len
            && line[at..at + len]
                .iter()
                .zip(scheme.chars())
                .all(|(&(a, _), b)| a.to_ascii_lowercase() == b)
        {
            return Some(len);
        }
    }
    None
}

/// RFC 3986-ish URL character set (quotes, angle brackets, backticks and all
/// non-ASCII excluded, so box-drawing borders never glue onto a link).
fn is_url_char(c: char) -> bool {
    c.is_ascii_alphanumeric()
        || matches!(
            c,
            '-' | '.'
                | '_'
                | '~'
                | ':'
                | '/'
                | '?'
                | '#'
                | '['
                | ']'
                | '@'
                | '!'
                | '$'
                | '&'
                | '('
                | ')'
                | '*'
                | '+'
                | ','
                | ';'
                | '='
                | '%'
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn screen_links(cols: u16, data: &[u8]) -> Vec<TermLink> {
        let mut parser = vt100::Parser::new(24, cols, 0);
        parser.process(data);
        visible_links(parser.screen())
    }

    #[test]
    fn finds_url_and_trims_trailing_punctuation() {
        let links = screen_links(80, b"see https://example.com/foo. done");
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].url, "https://example.com/foo");
        assert_eq!(links[0].segments, vec![(0, 4, 26)]);
        assert!(links[0].contains((4, 0)));
        assert!(links[0].contains((26, 0)));
        assert!(!links[0].contains((27, 0)));
        assert!(!links[0].contains((4, 1)));
    }

    #[test]
    fn finds_multiple_urls_on_one_row() {
        let links = screen_links(80, b"http://a.dev and https://b.dev/x");
        let urls: Vec<&str> = links.iter().map(|l| l.url.as_str()).collect();
        assert_eq!(urls, ["http://a.dev", "https://b.dev/x"]);
    }

    #[test]
    fn joins_wrapped_rows_into_one_link() {
        // 20 columns: the URL hard-wraps mid-token onto row 1.
        let links = screen_links(20, b"go https://example.com/long/path now");
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].url, "https://example.com/long/path");
        // Row 0 cols 3..=19, continuing on row 1 from col 0.
        assert_eq!(links[0].segments[0], (0, 3, 19));
        assert_eq!(links[0].segments[1].0, 1);
        assert!(links[0].contains((0, 1)));
    }

    #[test]
    fn hard_line_breaks_do_not_join() {
        let links = screen_links(80, b"https://a.dev\r\ncom/path");
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].url, "https://a.dev");
    }

    #[test]
    fn bare_scheme_is_not_a_link() {
        assert!(screen_links(80, b"the https:// prefix").is_empty());
        assert!(screen_links(80, b"no links here").is_empty());
    }
}
