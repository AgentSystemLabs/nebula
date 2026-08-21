//! URL and file-path detection over the visible vt100 screen, for
//! underlining links in the terminal pane and opening them on ⌥click (URLs
//! in the browser, paths in the editor modal). vt100 0.15 drops OSC 8
//! hyperlinks, so links are found by scanning the rendered cell text —
//! which also catches the plain `path:line` references agent CLIs print
//! (claude, cursor, codex) that never were hyperlinks to begin with.

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

/// A detected file reference and the screen cells it occupies (the
/// `TermLink` segment shape). `line` comes from a `:12`, `:12:5`, `#L12` or
/// `†L12` suffix when present.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileLink {
    pub path: String,
    pub line: Option<u64>,
    pub segments: Vec<(u16, u16, u16)>,
}

impl FileLink {
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
    let mut links = Vec::new();
    for_each_logical_line(screen, |line| scan_line(line, &mut links));
    links
}

/// Scan the visible screen for file-path references (`src/app.rs:12`,
/// `Cargo.toml`, `/abs/path.py`, `a/diff/path.ts`), joining wrapped rows
/// the way `visible_links` does.
pub fn visible_file_links(screen: &vt100::Screen) -> Vec<FileLink> {
    let mut links = Vec::new();
    for_each_logical_line(screen, |line| scan_file_line(line, &mut links));
    links
}

/// Walk the screen one logical line at a time: each entry is a char plus
/// the cell it came from, so match offsets map straight back to screen
/// coordinates. `wrapped` rows flow into the next with no line break.
fn for_each_logical_line(screen: &vt100::Screen, mut f: impl FnMut(&[(char, (u16, u16))])) {
    let (rows, cols) = screen.size();
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
            // An empty cell is a gap — it terminates any URL/path run.
            let ch = contents.chars().next().unwrap_or(' ');
            line.push((ch, (col, row)));
        }
        if !screen.row_wrapped(row) {
            f(&line);
            line.clear();
        }
    }
    if !line.is_empty() {
        f(&line);
    }
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

/// Scan one logical line for file-path references.
fn scan_file_line(line: &[(char, (u16, u16))], links: &mut Vec<FileLink>) {
    let n = line.len();
    let mut i = 0;
    while i < n {
        // URLs own their span: `example.com/x.rs` inside one is not a file.
        if let Some(len) = scheme_at(line, i) {
            let mut end = i + len;
            while end < n && is_url_char(line[end].0) {
                end += 1;
            }
            i = end;
            continue;
        }
        if !is_path_char(line[i].0) {
            i += 1;
            continue;
        }
        let start = i;
        let mut end = i;
        while end < n && is_path_char(line[end].0) {
            end += 1;
        }
        // A trailing dot is sentence context ("see src/app.rs."), never an
        // extension.
        let mut tok_end = end;
        while tok_end > start && line[tok_end - 1].0 == '.' {
            tok_end -= 1;
        }
        let token: String = line[start..tok_end].iter().map(|&(c, _)| c).collect();
        let (line_no, suffix_len) = if tok_end == end {
            line_suffix(&line[end..])
        } else {
            (None, 0) // "src/app.rs." — the dot severs any suffix
        };
        if path_qualifies(&token, line_no.is_some()) {
            let span = &line[start..end + suffix_len];
            let mut segments: Vec<(u16, u16, u16)> = Vec::new();
            for &(_, (col, row)) in span {
                match segments.last_mut() {
                    Some(seg) if seg.0 == row && seg.2 + 1 == col => seg.2 = col,
                    _ => segments.push((row, col, col)),
                }
            }
            links.push(FileLink {
                path: token,
                line: line_no,
                segments,
            });
        }
        i = end + suffix_len;
    }
}

/// Parse a `:12`, `:12:5` (line:col), `#L12`, or `†L12` suffix at the head
/// of `rest`. Returns the line number and how many cells the suffix spans.
fn line_suffix(rest: &[(char, (u16, u16))]) -> (Option<u64>, usize) {
    let digits = |from: usize| -> usize {
        rest[from..]
            .iter()
            .take_while(|&&(c, _)| c.is_ascii_digit())
            .count()
    };
    // ":<line>" (optionally ":<col>"), the claude/cursor shape.
    if rest.first().is_some_and(|&(c, _)| c == ':') {
        let d = digits(1);
        // Digits running into a word (":12abc") are not a line reference.
        let word_after = |at: usize| {
            rest.get(at)
                .is_some_and(|&(c, _)| c.is_ascii_alphanumeric())
        };
        if d > 0 && !word_after(1 + d) {
            let line: u64 = rest[1..1 + d]
                .iter()
                .map(|&(c, _)| c)
                .collect::<String>()
                .parse()
                .unwrap_or(1);
            let mut len = 1 + d;
            // Column suffix: consumed so the underline covers it, ignored.
            if rest.get(len).is_some_and(|&(c, _)| c == ':') {
                let cd = digits(len + 1);
                if cd > 0 && !word_after(len + 1 + cd) {
                    len += 1 + cd;
                }
            }
            return (Some(line.max(1)), len);
        }
    }
    // "#L12" (github) / "†L12" (codex citations).
    if rest.first().is_some_and(|&(c, _)| c == '#' || c == '†')
        && rest.get(1).is_some_and(|&(c, _)| c == 'L')
    {
        let d = digits(2);
        if d > 0 {
            let line: u64 = rest[2..2 + d]
                .iter()
                .map(|&(c, _)| c)
                .collect::<String>()
                .parse()
                .unwrap_or(1);
            return (Some(line.max(1)), 2 + d);
        }
    }
    (None, 0)
}

/// Whether a token reads as a file path rather than prose. Tokens with a
/// directory separator need a plausible extension (or an explicit line
/// suffix) so "and/or" and "24/7" stay prose; bare filenames additionally
/// need a well-known source extension so "example.com" and "e.g" stay
/// prose too.
fn path_qualifies(token: &str, has_line: bool) -> bool {
    if token.len() < 3 || token.ends_with('/') || token.starts_with("//") {
        return false;
    }
    let name = token.rsplit('/').next().unwrap_or(token);
    // Extension = letters/digits after the last dot, at least one letter
    // (so "1.2.3" is a version, not a file). Empty stems ok: `config/.env`.
    let ext = name.rsplit_once('.').map(|(_, e)| e).filter(|e| {
        (1..=9).contains(&e.len())
            && e.chars().all(|c| c.is_ascii_alphanumeric())
            && e.chars().any(|c| c.is_ascii_alphabetic())
    });
    if token.contains('/') {
        ext.is_some() || has_line
    } else {
        ext.is_some_and(|e| KNOWN_EXTENSIONS.contains(&e.to_ascii_lowercase().as_str()))
    }
}

/// Extensions that make a bare (slash-less) filename clickable.
const KNOWN_EXTENSIONS: &[&str] = &[
    "bash", "c", "cc", "cfg", "cjs", "conf", "cpp", "cs", "css", "dart", "env", "erl", "ex", "exs",
    "fish", "go", "gql", "graphql", "h", "hpp", "hs", "htm", "html", "ini", "ipynb", "java", "js",
    "json", "jsx", "kt", "kts", "less", "lock", "lua", "md", "mjs", "php", "prisma", "proto", "py",
    "rb", "rs", "scss", "sh", "sql", "svelte", "swift", "tf", "toml", "ts", "tsx", "txt", "vue",
    "xml", "yaml", "yml", "zig", "zsh",
];

/// Characters a path token may contain. `:`, `#` and `†` are excluded —
/// line suffixes are parsed separately — as are quotes, backticks,
/// parens/brackets and all non-ASCII, so `Read(src/app.rs)` and box-drawing
/// borders never glue onto a path.
fn is_path_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | '/' | '~' | '+' | '@')
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

    fn screen_files(cols: u16, data: &[u8]) -> Vec<FileLink> {
        let mut parser = vt100::Parser::new(24, cols, 0);
        parser.process(data);
        visible_file_links(parser.screen())
    }

    #[test]
    fn finds_claude_style_path_with_line() {
        let files = screen_files(80, b"see crates/nebula-tui/src/ui.rs:1226 for it");
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].path, "crates/nebula-tui/src/ui.rs");
        assert_eq!(files[0].line, Some(1226));
        // The underline (and click target) covers the ":1226" suffix too.
        assert!(files[0].contains((4, 0)));
        assert!(files[0].contains((35, 0)));
        assert!(!files[0].contains((36, 0)));
    }

    #[test]
    fn line_col_and_github_and_codex_suffixes() {
        let files = screen_files(
            80,
            "a src/x.ts:12:5 b src/y.go#L7 c src/z.py\u{2020}L42".as_bytes(),
        );
        let got: Vec<(&str, Option<u64>)> =
            files.iter().map(|f| (f.path.as_str(), f.line)).collect();
        assert_eq!(
            got,
            [
                ("src/x.ts", Some(12)),
                ("src/y.go", Some(7)),
                ("src/z.py", Some(42)),
            ]
        );
    }

    #[test]
    fn plain_path_and_trailing_period() {
        let files = screen_files(80, b"edited src/app.rs. done");
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].path, "src/app.rs");
        assert_eq!(files[0].line, None);
    }

    #[test]
    fn tool_header_parens_do_not_glue() {
        let files = screen_files(80, b"Read(crates/nebula/src/main.rs)");
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].path, "crates/nebula/src/main.rs");
    }

    #[test]
    fn bare_filenames_need_a_known_extension() {
        assert_eq!(
            screen_files(80, b"check Cargo.toml please")[0].path,
            "Cargo.toml"
        );
        assert!(screen_files(80, b"visit example.com today").is_empty());
        assert!(screen_files(80, b"e.g. this, i.e. that").is_empty());
    }

    #[test]
    fn prose_slashes_and_versions_are_not_paths() {
        assert!(screen_files(80, b"and/or 24/7 TCP/IP v1.2.3 either/or").is_empty());
    }

    #[test]
    fn paths_inside_urls_are_urls() {
        assert!(screen_files(80, b"https://x.dev/src/app.rs:12").is_empty());
    }

    #[test]
    fn absolute_and_diff_prefixed_paths_match() {
        let files = screen_files(80, b"--- a/src/lib.rs and /Users/x/proj/main.py:3");
        let got: Vec<&str> = files.iter().map(|f| f.path.as_str()).collect();
        assert_eq!(got, ["a/src/lib.rs", "/Users/x/proj/main.py"]);
        assert_eq!(files[1].line, Some(3));
    }

    #[test]
    fn wrapped_path_joins_across_rows() {
        // 20 columns: the path hard-wraps onto row 1.
        let files = screen_files(20, b"in crates/nebula-tui/src/app.rs:5 x");
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].path, "crates/nebula-tui/src/app.rs");
        assert_eq!(files[0].line, Some(5));
        assert_eq!(files[0].segments.len(), 2);
    }
}
