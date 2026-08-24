//! The pull-request reading pane: what the terminal pane shows while the
//! Worktrees cursor rests on an open-PR row.
//!
//! The whole preview is laid out as a flat `Vec<Line>` — every wrap decided
//! up front against the pane width — so scrolling is a slice and the line
//! count is exact. That matters because a pull request body is arbitrary
//! prose from someone else's keyboard: it has no natural row count, and a
//! renderer that wrapped at draw time could not tell the scroller how far
//! down it is allowed to go.
//!
//! The body is markdown, and it is rendered as **plain wrapped text on
//! purpose**. nebula is not a markdown viewer; interpreting someone's fenced
//! code block or table would mangle it more often than it would help. The
//! one concession is that hard line breaks are honored, because a PR
//! description written as a list reads as a list.

use crate::pull_request::{PrComment, PrDetail};
use crate::theme::Theme;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

/// Left inset of the body text, so prose doesn't hug the pane rule.
const INDENT: &str = " ";

/// Wrap `text` to `width` columns on word boundaries, honoring the hard
/// line breaks already in it. A word longer than the whole width (a URL, a
/// long path) is broken at the edge rather than being allowed to overflow.
/// An empty input is one empty line — a blank line in a body is a paragraph
/// break and has to survive.
pub fn wrap(text: &str, width: usize) -> Vec<String> {
    let width = width.max(1);
    let mut out = Vec::new();
    for raw in text.replace('\t', "    ").lines() {
        let mut line = String::new();
        let mut len = 0usize;
        for word in raw.split(' ') {
            let wlen = word.chars().count();
            // A word that can never fit: emit what we have, then break the
            // word across as many rows as it takes.
            if wlen > width {
                if len > 0 {
                    out.push(std::mem::take(&mut line));
                }
                let mut chunk = String::new();
                for c in word.chars() {
                    if chunk.chars().count() == width {
                        out.push(std::mem::take(&mut chunk));
                    }
                    chunk.push(c);
                }
                line = chunk;
                len = line.chars().count();
                continue;
            }
            let need = if len == 0 { wlen } else { wlen + 1 };
            if len + need > width {
                out.push(std::mem::take(&mut line));
                len = 0;
            }
            if len > 0 {
                line.push(' ');
                len += 1;
            }
            line.push_str(word);
            len += wlen;
        }
        out.push(line);
    }
    if out.is_empty() {
        out.push(String::new());
    }
    out
}

/// Trim a row of styled segments to `width` columns. Whole segments fall
/// off the end first — the headline rows are built most-important-first, so
/// a narrow pane loses the branch names before it loses the state word —
/// and whatever segment straddles the edge is clipped with an ellipsis.
/// Every line this module emits goes through here or through [`wrap`];
/// ratatui silently clips an overwide line, taking the rest of the row with
/// it, so "it'll probably fit" is not good enough.
fn fit(spans: Vec<Span<'static>>, width: usize) -> Line<'static> {
    let mut kept: Vec<Span<'static>> = Vec::new();
    let mut used = 0usize;
    for span in spans {
        let len = span.content.chars().count();
        if used + len <= width {
            used += len;
            kept.push(span);
            continue;
        }
        let room = width - used;
        if room > 1 {
            let text: String = span
                .content
                .chars()
                .take(room - 1)
                .chain(std::iter::once('…'))
                .collect();
            kept.push(Span::styled(text, span.style));
        }
        break;
    }
    Line::from(kept)
}

/// The preview as styled lines, ready to slice by the scroll offset.
/// `width` is the pane's inner width.
pub fn lines(detail: &PrDetail, width: usize, th: Theme) -> Vec<Line<'static>> {
    let body_w = width.saturating_sub(INDENT.len() + 1).max(20);
    let dim = Style::default().fg(th.dim);
    let muted = Style::default().fg(th.muted);
    let mut out: Vec<Line<'static>> = Vec::new();

    // ---- headline ----
    out.push(fit(
        vec![
            Span::styled(format!("{INDENT}#{} ", detail.number), dim),
            Span::styled(
                detail.title.clone(),
                Style::default().fg(th.accent).add_modifier(Modifier::BOLD),
            ),
        ],
        width,
    ));
    let state = match (detail.state.as_str(), detail.is_draft) {
        ("OPEN", true) => ("draft", th.dim),
        ("OPEN", false) => ("open", th.ok),
        ("MERGED", _) => ("merged", th.special),
        ("CLOSED", _) => ("closed", th.err),
        _ => (detail.state.as_str(), th.dim),
    };
    let mut meta = vec![
        Span::styled(INDENT.to_string(), dim),
        Span::styled(
            state.0.to_string(),
            Style::default().fg(state.1).add_modifier(Modifier::BOLD),
        ),
    ];
    if !detail.author.is_empty() {
        meta.push(Span::styled(format!(" · {}", detail.author), muted));
    }
    if !detail.head.is_empty() {
        meta.push(Span::styled(
            format!(" · {} ← {}", detail.base, detail.head),
            dim,
        ));
    }
    out.push(fit(meta, width));
    out.push(fit(
        vec![
            Span::styled(INDENT.to_string(), dim),
            Span::styled(format!("+{}", detail.additions), Style::default().fg(th.ok)),
            Span::styled(" ", dim),
            Span::styled(
                format!("-{}", detail.deletions),
                Style::default().fg(th.err),
            ),
            Span::styled(
                format!(
                    " · {} file{}",
                    detail.changed_files,
                    if detail.changed_files == 1 { "" } else { "s" }
                ),
                dim,
            ),
        ],
        width,
    ));
    out.push(Line::from(""));

    // ---- description ----
    if detail.body.trim().is_empty() {
        out.push(Line::from(Span::styled(
            format!("{INDENT}(no description)"),
            dim,
        )));
    } else {
        for row in wrap(detail.body.trim_end(), body_w) {
            out.push(Line::from(Span::styled(format!("{INDENT}{row}"), muted)));
        }
    }

    // ---- conversation ----
    if !detail.comments.is_empty() {
        out.push(Line::from(""));
        out.push(fit(
            vec![Span::styled(
                format!(
                    "{INDENT}── {} comment{} ──",
                    detail.comments.len(),
                    if detail.comments.len() == 1 { "" } else { "s" }
                ),
                dim,
            )],
            width,
        ));
        for c in &detail.comments {
            out.push(Line::from(""));
            out.extend(comment_lines(c, width, body_w, th));
        }
    }
    out
}

/// One comment: an attribution row, then its wrapped body.
fn comment_lines(c: &PrComment, width: usize, body_w: usize, th: Theme) -> Vec<Line<'static>> {
    let dim = Style::default().fg(th.dim);
    let mut head = vec![Span::styled(
        format!("{INDENT}{}", c.author),
        Style::default().fg(th.accent),
    )];
    // A verdict is the whole point of a review row — it goes loud, and in
    // the color the panels already use for "this wants you".
    if let Some(verdict) = c.verdict() {
        let color = if verdict == "approved" {
            th.ok
        } else {
            th.warn
        };
        head.push(Span::styled(
            format!(" {verdict}"),
            Style::default().fg(color).add_modifier(Modifier::BOLD),
        ));
    }
    if let Some(day) = c.at.split('T').next().filter(|d| !d.is_empty()) {
        head.push(Span::styled(format!(" · {day}"), dim));
    }
    let mut out = vec![fit(head, width)];
    if c.body.trim().is_empty() {
        return out;
    }
    for row in wrap(c.body.trim_end(), body_w.saturating_sub(2)) {
        out.push(Line::from(Span::styled(
            format!("{INDENT}  {row}"),
            Style::default().fg(th.muted),
        )));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wrap_breaks_on_words_and_keeps_hard_breaks() {
        assert_eq!(wrap("one two three", 7), ["one two", "three"]);
        // A blank line is a paragraph break and must survive.
        assert_eq!(wrap("a\n\nb", 10), ["a", "", "b"]);
        // Nothing at all is still one row: the caller renders it.
        assert_eq!(wrap("", 10), [""]);
    }

    /// A word wider than the pane can't be allowed to overflow the rect —
    /// ratatui would clip it and the rest of the line with it.
    #[test]
    fn wrap_breaks_a_word_too_long_to_fit() {
        assert_eq!(
            wrap("https://example.dev/a/b", 8),
            ["https://", "example.", "dev/a/b"]
        );
        assert_eq!(
            wrap("hi https://example.dev", 8),
            ["hi", "https://", "example.", "dev"]
        );
        // Every row respects the budget, whatever the input.
        for row in wrap("supercalifragilistic and some ordinary words", 9) {
            assert!(row.chars().count() <= 9, "{row:?} is too wide");
        }
    }

    fn detail(body: &str, comments: Vec<PrComment>) -> PrDetail {
        PrDetail {
            number: 42,
            url: "https://github.com/o/r/pull/42".into(),
            title: "Attach links".into(),
            state: "OPEN".into(),
            is_draft: false,
            author: "webdevcody".into(),
            base: "main".into(),
            head: "feat/links".into(),
            additions: 106,
            deletions: 4,
            changed_files: 2,
            body: body.into(),
            comments,
        }
    }

    fn text(lines: &[Line<'static>]) -> String {
        lines
            .iter()
            .map(|l| {
                l.spans
                    .iter()
                    .map(|s| s.content.as_ref())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn the_preview_leads_with_the_headline_then_body_then_conversation() {
        let d = detail(
            "Makes the row.",
            vec![
                PrComment {
                    author: "kate".into(),
                    at: "2024-04-25T19:55:42Z".into(),
                    review_state: "APPROVED".into(),
                    body: String::new(),
                },
                PrComment {
                    author: "steiza".into(),
                    at: "2024-04-26T21:44:55Z".into(),
                    review_state: String::new(),
                    body: "nice".into(),
                },
            ],
        );
        let out = text(&lines(&d, 60, Theme::default()));
        assert!(out.starts_with(" #42 Attach links"), "{out}");
        assert!(
            out.contains("open · webdevcody · main ← feat/links"),
            "{out}"
        );
        assert!(out.contains("+106 -4 · 2 files"), "{out}");
        assert!(out.contains("Makes the row."), "{out}");
        assert!(out.contains("── 2 comments ──"), "{out}");
        // A bodyless approval is still worth a row — the verdict is the news.
        assert!(out.contains("kate approved · 2024-04-25"), "{out}");
        assert!(out.contains("steiza · 2024-04-26"), "{out}");
        assert!(out.contains("nice"), "{out}");
    }

    /// An empty description says so rather than rendering a silent gap that
    /// reads as "still loading".
    #[test]
    fn an_empty_body_says_so() {
        let out = text(&lines(&detail("   \n", vec![]), 60, Theme::default()));
        assert!(out.contains("(no description)"), "{out}");
        assert!(!out.contains("── "), "no conversation rule: {out}");
    }

    /// Every rendered row has to fit the pane, or ratatui clips it.
    #[test]
    fn no_rendered_line_overflows_the_pane() {
        let d = detail(
            "A description with a very long unbroken token: \
             https://github.com/AgentSystemLabs/nebula/pull/12345/files#diff-abcdef",
            vec![PrComment {
                author: "steiza".into(),
                at: "2024-04-26T21:44:55Z".into(),
                review_state: String::new(),
                body: "a".repeat(200),
            }],
        );
        for w in [24usize, 40, 80] {
            for line in lines(&d, w, Theme::default()) {
                let len: usize = line.spans.iter().map(|s| s.content.chars().count()).sum();
                assert!(len <= w, "width {w}: {len} cols in {line:?}");
            }
        }
    }
}
