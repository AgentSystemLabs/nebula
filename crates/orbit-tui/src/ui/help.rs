//! The HELP OVERLAY (`h` / `?`): every grid shortcut, grouped by the job it
//! does, with a `/` filter.
//!
//! Keys are drawn as keycaps from the live keymap — every one of them is
//! rebindable in Settings → Hotkeys, and help that lies about that is worse
//! than none. Literal caps are for keys that belong to an overlay or a text
//! field rather than the grid, and mouse gestures are written out.
//!
//! The layout follows the terminal: one, two or three columns by width, the
//! sections packed so the columns come out as even as they can, and the body
//! scrolls when it is taller than the screen. The box is sized from the full
//! list, so it holds still while a filter narrows what is in it.

use super::*;
use crate::app::HelpView;
use crate::keymap::Action::*;
use crate::keymap::Keymap;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

/// What a row's key column shows.
pub(super) enum Keys {
    /// Every chord the keymap binds each action to; actions are
    /// alternatives, drawn with a `/` between them.
    Act(&'static [Action]),
    /// Fixed keys: ` / ` splits alternatives, spaces split keycaps.
    Caps(&'static str),
    /// A mouse gesture, written out rather than drawn as a key.
    Mouse(&'static str),
}

pub(super) struct Entry {
    keys: Keys,
    /// What the key does, in plain words.
    text: &'static str,
    /// The detail worth knowing, drawn quieter after the text.
    note: &'static str,
}

pub(super) struct Section {
    pub(super) title: &'static str,
    entries: &'static [Entry],
}

const fn e(keys: Keys, text: &'static str, note: &'static str) -> Entry {
    Entry { keys, text, note }
}

use Keys::{Act, Caps, Mouse};

pub(super) const SECTIONS: &[Section] = &[
    Section {
        title: "Move around",
        entries: &[
            e(
                Act(&[MoveDown, MoveUp]),
                "walk the cards",
                "twice at the top: tabs",
            ),
            e(Act(&[FocusLeft, FocusRight]), "along a row of cards", ""),
            e(Act(&[FocusNext]), "open / fold the checkout", ""),
            e(
                Act(&[Activate]),
                "step into the session",
                "type to the agent",
            ),
            e(
                Act(&[HalfPageDown, HalfPageUp]),
                "jump two rows of cards",
                "",
            ),
            e(
                Act(&[NextProjectTab, PrevProjectTab]),
                "next / previous project",
                "",
            ),
            e(Caps("1–9"), "open that project tab", ""),
            e(Act(&[ProjectDropdown]), "list every project", ""),
            e(
                Act(&[CloseProjectTab]),
                "close the project tab",
                "sessions keep running",
            ),
            e(Act(&[AddProject]), "open a folder", "as a new project"),
            e(
                Act(&[NextAttention, PrevAttention]),
                "session needing you",
                "next / previous",
            ),
        ],
    },
    Section {
        title: "Find",
        entries: &[
            e(Act(&[Palette]), "jump to any session", "any project"),
            e(Caps("^o / ^f"), "jump list: open / focus", ""),
            e(
                Act(&[FindFile]),
                "find a file by name",
                "^y copies its path",
            ),
            e(Act(&[Grep]), "search inside files", "git grep"),
            e(Act(&[TreeBrowser]), "browse the file tree", ""),
        ],
    },
    Section {
        title: "Sessions",
        entries: &[
            e(Act(&[QuickPrompt]), "new task", "Enter launches"),
            e(Act(&[New]), "new session, no task", "pick the CLI first"),
            e(Act(&[FollowUp]), "follow-up to the agent", ""),
            e(Act(&[DuplicateSession]), "new task like this card", ""),
            e(Act(&[AgentPresets]), "presets: saved launches", ""),
            e(Act(&[Rename]), "rename the session", ""),
            e(Act(&[MoveSession]), "move to another worktree", ""),
            e(
                Act(&[NewTerminal, OpenGhosttyTab]),
                "shell: here / in Ghostty",
                "",
            ),
            e(
                Act(&[Archive, Unarchive, ToggleArchived]),
                "archive / restore / show",
                "",
            ),
            e(Act(&[Delete, DeleteAll]), "delete one / delete all", ""),
        ],
    },
    Section {
        title: "Review code",
        entries: &[
            e(
                Act(&[GitDiff]),
                "diff of the checkout",
                "^r reviewed · ^t tree",
            ),
            e(Act(&[GitGraph]), "git graph", "commits, branches, refs"),
            e(Act(&[OpenGraph]), "code graph", "graphify, in the browser"),
            e(
                Act(&[OpenWorktree]),
                "open in your editor",
                "the open command",
            ),
            e(Act(&[SwitchBranch]), "switch ⌂ root's branch", ""),
        ],
    },
    Section {
        title: "GitHub",
        entries: &[
            e(
                Act(&[PullRequests]),
                "pull requests",
                "read, start a session",
            ),
            e(Act(&[Issues]), "issues", "prompt, preset, edit"),
            e(Act(&[CommentPullRequest]), "comment on the card's PR", ""),
            e(
                Act(&[OpenPullRequest, OpenIssue]),
                "PR / issue in browser",
                "the card's",
            ),
            e(Act(&[OpenRepo]), "repo in browser", ""),
            e(Act(&[RefreshPullRequests]), "reload PRs and issues", ""),
        ],
    },
    Section {
        title: "Terminal & mouse",
        entries: &[
            e(Act(&[Activate]), "type into the session", ""),
            e(Act(&[UnlockTerminal]), "back to the cards", ""),
            e(Act(&[PaneTabs]), "session ↔ its terminals", "in the pane"),
            e(Mouse("drag"), "select and copy", "double-click: a word"),
            e(Mouse("click, drag"), "to apps using the mouse", ""),
            e(Mouse("⌥ click"), "open URL / file under it", ""),
            e(Mouse("⇧ drag"), "select via your terminal", ""),
            e(Mouse("right-click"), "card / tab menu", "run, restart…"),
            e(Mouse("drag pane edge"), "resize the pane", ""),
            e(Mouse("click outside"), "close any dialog", "same as Esc"),
        ],
    },
    Section {
        title: "General",
        entries: &[
            e(Act(&[ToggleLauncherPane]), "fold / unfold the pane", ""),
            e(Act(&[ToggleFullScreen]), "full screen / normal", ""),
            e(
                Act(&[ToggleOrchestratorView]),
                "orchestrator view",
                "one chat across projects",
            ),
            e(Caps("⇧+letter"), "a key's bigger twin", "or outside Orbit"),
            e(Act(&[Hosts]), "ssh hosts", "a: new · d: delete"),
            e(Act(&[Settings]), "settings", "Hotkeys tab rebinds keys"),
            e(Act(&[Metrics]), "memory: Orbit and agents", ""),
            e(Act(&[Help]), "this help", ""),
            e(Act(&[Quit]), "quit", "sessions keep running"),
        ],
    },
    Section {
        // Every typed field — names, filters, queries — is the same line
        // editor (text_input.rs).
        title: "Typing in a field",
        entries: &[
            e(Caps("← → / ⌥← ⌥→"), "by character / by word", ""),
            e(Caps("^a / ^e"), "start / end of the line", ""),
            e(Caps("⌥⌫"), "delete the word before", ""),
            e(Caps("^u / ^k"), "delete to start / end", ""),
        ],
    },
];

/// Widest a key column gets; an action bound to more chords loses whole
/// chords off the end, never half of one.
const KEY_MAX: usize = 20;
/// A column narrower than this reads badly; below it, one column fewer.
const COL_MIN: u16 = 36;
/// Past this a column is mostly empty space.
const COL_MAX: u16 = 56;
/// The widest a lone column gets, on a terminal too narrow for two.
const COL_ONE_MAX: u16 = 80;
/// Space between columns.
const GUTTER: u16 = 3;
/// Rows of chrome inside the border: the filter line and a spacer.
const HEAD_ROWS: u16 = 2;

/// One drawn unit of a key column.
#[derive(Debug, Clone, PartialEq)]
enum Piece {
    Cap(String),
    /// The `/` between alternatives.
    Or,
    Mouse(String),
    /// Chords dropped for room.
    More,
}

fn pieces(keys: &Keys, keymap: &Keymap) -> Vec<Piece> {
    let mut out = Vec::new();
    match keys {
        Act(actions) => {
            for (i, action) in actions.iter().enumerate() {
                if i > 0 {
                    out.push(Piece::Or);
                }
                let chords = keymap.shown_chords(*action);
                if chords.is_empty() {
                    out.push(Piece::Cap(crate::keymap::UNBOUND.into()));
                }
                out.extend(chords.iter().map(|c| Piece::Cap(c.display())));
            }
        }
        Caps(spec) => {
            for (i, group) in spec.split(" / ").enumerate() {
                if i > 0 {
                    out.push(Piece::Or);
                }
                out.extend(group.split_whitespace().map(|c| Piece::Cap(c.into())));
            }
        }
        Mouse(gesture) => out.push(Piece::Mouse((*gesture).into())),
    }
    out
}

fn pieces_width(pieces: &[Piece]) -> usize {
    let mut width = 0;
    for (i, piece) in pieces.iter().enumerate() {
        width += match piece {
            Piece::Cap(s) => s.width(),
            Piece::Or => 3,
            Piece::Mouse(s) => s.width(),
            Piece::More => 2,
        };
        // Two keycaps side by side get a space between them.
        if i > 0 && matches!(piece, Piece::Cap(_)) && matches!(pieces[i - 1], Piece::Cap(_)) {
            width += 1;
        }
    }
    width
}

/// `pieces` cut to `max` columns: whole keycaps come off the end and an
/// ellipsis says so.
fn fit_pieces(mut pieces: Vec<Piece>, max: usize) -> Vec<Piece> {
    if pieces_width(&pieces) <= max {
        return pieces;
    }
    while pieces.len() > 1 {
        pieces.pop();
        while matches!(pieces.last(), Some(Piece::Or)) {
            pieces.pop();
        }
        let mut cut = pieces.clone();
        cut.push(Piece::More);
        if pieces_width(&cut) <= max {
            return cut;
        }
    }
    pieces
}

fn piece_spans(pieces: &[Piece], th: Theme) -> Vec<Span<'static>> {
    let cap = Style::default()
        .fg(th.accent)
        .bg(th.sel_bg)
        .add_modifier(Modifier::BOLD);
    let mut out = Vec::new();
    for (i, piece) in pieces.iter().enumerate() {
        if i > 0 && matches!(piece, Piece::Cap(_)) && matches!(pieces[i - 1], Piece::Cap(_)) {
            out.push(Span::raw(" "));
        }
        out.push(match piece {
            Piece::Cap(s) => Span::styled(s.clone(), cap),
            Piece::Or => Span::styled(" / ", Style::default().fg(th.dim)),
            Piece::Mouse(s) => Span::styled(
                s.clone(),
                Style::default().fg(th.muted).add_modifier(Modifier::ITALIC),
            ),
            Piece::More => Span::styled(" …", Style::default().fg(th.dim)),
        });
    }
    out
}

fn keys_text(pieces: &[Piece]) -> String {
    pieces
        .iter()
        .filter_map(|p| match p {
            Piece::Cap(s) | Piece::Mouse(s) => Some(s.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// The filter's words, lowercased; empty matches everything.
fn query_words(query: &str) -> Vec<String> {
    query.split_whitespace().map(str::to_lowercase).collect()
}

/// A row stays when every word is somewhere in its section's title, its
/// keys, its text or its note — so `git` keeps both graphs and the diff,
/// and a section's own name keeps the whole section.
fn row_matches(section: &Section, entry: &Entry, keys: &str, words: &[String]) -> bool {
    if words.is_empty() {
        return true;
    }
    let hay = format!("{} {keys} {} {}", section.title, entry.text, entry.note).to_lowercase();
    words.iter().all(|w| hay.contains(w.as_str()))
}

/// The sections left by the filter, each with the indexes of its rows that
/// survive it. A section with no rows left is dropped.
fn visible(keymap: &Keymap, words: &[String]) -> Vec<(&'static Section, Vec<usize>)> {
    SECTIONS
        .iter()
        .filter_map(|section| {
            let rows: Vec<usize> = section
                .entries
                .iter()
                .enumerate()
                .filter(|(_, entry)| {
                    let keys = keys_text(&pieces(&entry.keys, keymap));
                    row_matches(section, entry, &keys, words)
                })
                .map(|(i, _)| i)
                .collect();
            (!rows.is_empty()).then_some((section, rows))
        })
        .collect()
}

/// Split blocks `heights` tall, in order, into at most `cols` runs with
/// the tallest run as short as it can be; a blank row sits between two
/// blocks in one run. Brute force: a handful of sections, three columns.
fn partition(heights: &[usize], cols: usize) -> Vec<std::ops::Range<usize>> {
    let n = heights.len();
    let cols = cols.clamp(1, n.max(1));
    let run = |r: &std::ops::Range<usize>| -> usize {
        heights[r.clone()].iter().sum::<usize>() + r.len().saturating_sub(1)
    };
    let mut best: Option<(usize, Vec<std::ops::Range<usize>>)> = None;
    let mut consider = |runs: Vec<std::ops::Range<usize>>| {
        let tallest = runs.iter().map(run).max().unwrap_or(0);
        if best.as_ref().is_none_or(|(b, _)| tallest < *b) {
            best = Some((tallest, runs));
        }
    };
    match cols {
        1 => consider(std::iter::once(0..n).collect()),
        2 => (1..n).for_each(|a| consider(vec![0..a, a..n])),
        _ => {
            for a in 1..n {
                for b in a + 1..n {
                    consider(vec![0..a, a..b, b..n]);
                }
            }
        }
    }
    best.map(|(_, runs)| runs)
        .unwrap_or_else(|| std::iter::once(0..n).collect())
}

fn column_height(heights: &[usize], run: &std::ops::Range<usize>) -> usize {
    heights[run.clone()].iter().sum::<usize>() + run.len().saturating_sub(1)
}

/// How many columns fit `avail` cells, and how wide each one is.
fn columns_for(avail: u16) -> (u16, u16) {
    let cols = (1..=3u16)
        .rev()
        .find(|n| n * COL_MIN + (n - 1) * GUTTER <= avail)
        .unwrap_or(1);
    // One column takes the screen (up to a comfortable line); several
    // stop at COL_MAX, past which a column is mostly gap.
    let cap = if cols == 1 { COL_ONE_MAX } else { COL_MAX };
    let width = ((avail.saturating_sub((cols - 1) * GUTTER)) / cols).min(cap);
    (cols, width)
}

/// `text`, with every run that matches one of `words` lit up.
fn marked(text: &str, words: &[String], base: Style, hit: Style) -> Vec<Span<'static>> {
    if words.is_empty() || text.is_empty() {
        return vec![Span::styled(text.to_string(), base)];
    }
    let chars: Vec<char> = text.chars().collect();
    let lower: Vec<char> = chars
        .iter()
        .map(|c| c.to_lowercase().next().unwrap_or(*c))
        .collect();
    let mut lit = vec![false; chars.len()];
    for word in words {
        let w: Vec<char> = word.chars().collect();
        if w.is_empty() || w.len() > lower.len() {
            continue;
        }
        let mut i = 0;
        while i + w.len() <= lower.len() {
            if lower[i..i + w.len()] == w[..] {
                lit[i..i + w.len()].iter_mut().for_each(|l| *l = true);
                i += w.len();
            } else {
                i += 1;
            }
        }
    }
    let mut out = Vec::new();
    let mut start = 0;
    for i in 1..=chars.len() {
        if i == chars.len() || lit[i] != lit[start] {
            let run: String = chars[start..i].iter().collect();
            out.push(Span::styled(run, if lit[start] { hit } else { base }));
            start = i;
        }
    }
    out
}

/// `spans` cut to `max` columns, an ellipsis on whatever was cut.
fn fit(spans: Vec<Span<'static>>, max: usize) -> Vec<Span<'static>> {
    let mut out = Vec::new();
    let mut used = 0;
    let total: usize = spans.iter().map(|s| s.content.width()).sum();
    if total <= max {
        return spans;
    }
    for span in spans {
        let w = span.content.width();
        if used + w < max {
            used += w;
            out.push(span);
            continue;
        }
        let room = max.saturating_sub(used + 1);
        let mut cut = String::new();
        let mut cut_w = 0;
        for ch in span.content.chars() {
            let cw = ch.width().unwrap_or(0);
            if cut_w + cw > room {
                break;
            }
            cut.push(ch);
            cut_w += cw;
        }
        cut.push('…');
        out.push(Span::styled(cut, span.style));
        break;
    }
    out
}

fn header_line(title: &str, width: usize, th: Theme) -> Line<'static> {
    let label = format!(" {title} ");
    let rule = width.saturating_sub(1 + label.width());
    Line::from(vec![
        Span::styled("▍", Style::default().fg(th.accent)),
        Span::styled(
            label,
            Style::default().fg(th.text).add_modifier(Modifier::BOLD),
        ),
        Span::styled("─".repeat(rule), Style::default().fg(th.edge)),
    ])
}

/// One column's lines: each section's header and rows, a blank between.
fn column_lines(
    blocks: &[(&'static Section, Vec<usize>)],
    keymap: &Keymap,
    words: &[String],
    width: usize,
    th: Theme,
) -> Vec<Line<'static>> {
    let keys_of = |entry: &Entry| fit_pieces(pieces(&entry.keys, keymap), KEY_MAX);
    // Wide enough for most rows, not for the widest: one long binding
    // (open's three chords) would push every description in its column
    // along. A row past it pushes only its own text.
    let mut widths: Vec<usize> = blocks
        .iter()
        .flat_map(|(section, rows)| rows.iter().map(|&i| &section.entries[i]))
        .map(|entry| pieces_width(&keys_of(entry)))
        .collect();
    widths.sort_unstable();
    let key_w = widths
        .get((widths.len() * 85).div_ceil(100).saturating_sub(1))
        .copied()
        .unwrap_or(0)
        .min(width * 2 / 5);
    let text = Style::default().fg(th.text);
    let note = Style::default().fg(th.dim);
    let hit = Style::default()
        .fg(th.accent)
        .add_modifier(Modifier::BOLD | Modifier::UNDERLINED);
    let mut lines = Vec::new();
    for (b, (section, rows)) in blocks.iter().enumerate() {
        if b > 0 {
            lines.push(Line::from(""));
        }
        lines.push(header_line(section.title, width, th));
        for &i in rows {
            let entry = &section.entries[i];
            let keys = keys_of(entry);
            let mut spans = vec![Span::raw(" ")];
            spans.extend(piece_spans(&keys, th));
            spans.push(Span::raw(
                " ".repeat(key_w.saturating_sub(pieces_width(&keys)) + 2),
            ));
            spans.extend(marked(entry.text, words, text, hit));
            // A note shows whole or not at all: "twice a…" says nothing.
            let used: usize = spans.iter().map(|s| s.content.width()).sum();
            if !entry.note.is_empty() && used + 2 + entry.note.width() <= width {
                spans.push(Span::styled("  ", note));
                spans.extend(marked(entry.note, words, note, hit));
            }
            lines.push(Line::from(fit(spans, width)));
        }
    }
    lines
}

/// The rows a set of blocks takes: a header plus its rows each.
fn block_heights(blocks: &[(&'static Section, Vec<usize>)]) -> Vec<usize> {
    blocks.iter().map(|(_, rows)| rows.len() + 1).collect()
}

fn hint_spans(pairs: &[(&str, &str)], th: Theme) -> Vec<Span<'static>> {
    let mut out = vec![Span::raw(" ")];
    for (i, (key, what)) in pairs.iter().enumerate() {
        if i > 0 {
            out.push(Span::styled("  ·  ", Style::default().fg(th.edge)));
        }
        out.push(Span::styled(
            key.to_string(),
            Style::default().fg(th.accent).add_modifier(Modifier::BOLD),
        ));
        out.push(Span::styled(
            format!(" {what}"),
            Style::default().fg(th.muted),
        ));
    }
    out.push(Span::raw(" "));
    out
}

pub(super) fn draw_help_overlay(f: &mut Frame, app: &mut App, view: HelpView) {
    let th = app.chrome.theme;
    let keymap = &app.chrome.keymap;
    let screen = f.area();

    // Width: as many columns as fit, a cell of margin each side.
    let (cols, col_w) = columns_for(screen.width.saturating_sub(2 + 4));
    let box_w = cols * col_w + (cols - 1) * GUTTER + 4;

    // Height from the unfiltered list, so the box holds still while a
    // filter empties it.
    let everything = visible(keymap, &[]);
    let all_heights = block_heights(&everything);
    let full_body = partition(&all_heights, cols as usize)
        .iter()
        .map(|run| column_height(&all_heights, run))
        .max()
        .unwrap_or(1) as u16;
    let box_h = (full_body + HEAD_ROWS + 2).min(screen.height.saturating_sub(2).max(6));
    let area = centered_rect(screen, box_w, box_h);

    let words = query_words(&view.query);
    let shown = visible(keymap, &words);
    let shown_rows: usize = shown.iter().map(|(_, rows)| rows.len()).sum();
    let heights = block_heights(&shown);
    let runs = partition(&heights, cols as usize);
    let body_h = runs
        .iter()
        .map(|run| column_height(&heights, run))
        .max()
        .unwrap_or(0) as u16;
    let visible_body = box_h.saturating_sub(HEAD_ROWS + 2);
    let max_scroll = body_h.saturating_sub(visible_body);
    let scroll = view.scroll.min(max_scroll);

    // ---- the frame ----
    let more_below = scroll < max_scroll;
    let mut block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(th.accent))
        .title(Line::from(vec![
            Span::raw(" "),
            Span::styled(
                "Keyboard shortcuts",
                Style::default().fg(th.text).add_modifier(Modifier::BOLD),
            ),
            Span::raw(" "),
        ]))
        .title_bottom(Line::from(if view.typing {
            hint_spans(&[("Enter", "done"), ("Esc", "clear")], th)
        } else {
            // A narrow box drops the rebind hint, so the line never runs
            // into the scroll marker on the right.
            let mut hints = vec![("/", "filter"), ("↑↓", "scroll")];
            if box_w >= 72 {
                hints.push(("s", "rebind keys"));
            }
            hints.push(("Esc", "close"));
            hint_spans(&hints, th)
        }));
    if max_scroll > 0 {
        let mark = match (scroll > 0, more_below) {
            (true, true) => " ↑↓ more ",
            (false, true) => " ↓ more ",
            _ => " ↑ more ",
        };
        block = block.title_bottom(
            Line::from(Span::styled(mark, Style::default().fg(th.warn))).right_aligned(),
        );
    }
    f.render_widget(Clear, area);
    let inner = block.inner(area);
    f.render_widget(block, area);
    // A cell of padding inside the border on both sides.
    let inner = Rect {
        x: inner.x + 1,
        width: inner.width.saturating_sub(2),
        ..inner
    };

    // ---- the filter line ----
    if let Some(row) = row_rect(inner, 0) {
        let count = format!(
            "{shown_rows} {}",
            if shown_rows == 1 {
                "shortcut"
            } else {
                "shortcuts"
            }
        );
        let room = (row.width as usize).saturating_sub(4 + count.width());
        let mut spans = vec![Span::styled(
            "/ ",
            Style::default().fg(if view.typing || !view.query.is_empty() {
                th.accent
            } else {
                th.dim
            }),
        )];
        if view.typing {
            let field = Rect {
                x: row.x + 2,
                width: room as u16,
                ..row
            };
            spans.extend(search_line(&view.query, "type to filter…", field, th).spans);
        } else if view.query.is_empty() {
            spans.push(Span::styled(
                "filter shortcuts",
                Style::default().fg(th.dim),
            ));
        } else {
            spans.push(Span::styled(
                truncate(&view.query, room),
                Style::default().fg(th.text).add_modifier(Modifier::BOLD),
            ));
        }
        f.render_widget(Paragraph::new(Line::from(spans)), row);
        f.render_widget(
            Paragraph::new(Span::styled(count, Style::default().fg(th.dim)))
                .alignment(ratatui::layout::Alignment::Right),
            row,
        );
    }

    // ---- the body ----
    let body = Rect {
        y: inner.y + HEAD_ROWS,
        height: inner.height.saturating_sub(HEAD_ROWS),
        ..inner
    };
    if shown.is_empty() {
        let line = Line::from(vec![
            Span::styled("No shortcut matches ", Style::default().fg(th.dim)),
            Span::styled(
                format!("“{}”", view.query.as_str().trim()),
                Style::default().fg(th.text),
            ),
        ])
        .centered();
        if let Some(row) = row_rect(body, body.height.saturating_sub(1) as usize / 2) {
            f.render_widget(Paragraph::new(line), row);
        }
    } else {
        let mut x = body.x;
        for run in &runs {
            let rect = Rect {
                x,
                width: col_w.min(body.right().saturating_sub(x)),
                ..body
            };
            let lines = column_lines(&shown[run.clone()], keymap, &words, rect.width as usize, th);
            f.render_widget(Paragraph::new(lines).scroll((scroll, 0)), rect);
            x += col_w + GUTTER;
        }
    }

    // Record what the key and mouse handlers need: the box for clicks, and
    // how far the body can scroll.
    if let Some(Overlay::Help(h)) = &mut app.modals.overlay {
        h.area = area;
        h.max_scroll = max_scroll;
        h.page = visible_body.max(1);
        h.scroll = scroll;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn render(width: u16, height: u16, view: HelpView) -> (String, App) {
        let mut app = App::new();
        app.modals.overlay = Some(Overlay::Help(view.clone()));
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(width, height)).unwrap();
        terminal
            .draw(|f| draw_help_overlay(f, &mut app, view.clone()))
            .unwrap();
        let buf = terminal.backend().buffer().clone();
        let text = (0..height)
            .map(|y| {
                (0..width)
                    .map(|x| buf[(x, y)].symbol().to_string())
                    .collect::<String>()
                    + "\n"
            })
            .collect();
        (text, app)
    }

    /// A shortcut the help leaves out is one nobody finds: every action in
    /// the keymap has a row, but the tab digits (one literal `1–9` row).
    #[test]
    fn every_keymap_action_has_a_help_row() {
        let listed: Vec<Action> = SECTIONS
            .iter()
            .flat_map(|s| s.entries.iter())
            .filter_map(|entry| match entry.keys {
                Act(actions) => Some(actions),
                _ => None,
            })
            .flatten()
            .copied()
            .collect();
        for spec in crate::keymap::ACTIONS {
            if matches!(spec.action, SelectProjectTab(_)) {
                continue;
            }
            assert!(
                listed.contains(&spec.action),
                "`{}` ({}) is missing from the help",
                spec.id,
                spec.label
            );
        }
    }

    #[test]
    fn sections_split_into_even_columns() {
        assert_eq!(partition(&[5, 5, 5, 5], 2), vec![0..2, 2..4]);
        assert_eq!(partition(&[10, 2, 2, 2, 2], 2), vec![0..1, 1..5]);
        assert_eq!(partition(&[3, 3, 3], 3), vec![0..1, 1..2, 2..3]);
        // Fewer blocks than columns: one block a column.
        assert_eq!(partition(&[4], 3), vec![0..1]);
    }

    #[test]
    fn the_terminal_width_picks_the_column_count() {
        assert_eq!(columns_for(40).0, 1);
        assert_eq!(columns_for(90).0, 2);
        // A stock 120-column terminal (114 inside the margins) takes three.
        assert_eq!(columns_for(114).0, 3);
        assert!(columns_for(400).1 <= COL_MAX);
    }

    #[test]
    fn a_filter_keeps_matching_rows_and_whole_sections_by_name() {
        let keymap = Keymap::default();
        let graph = visible(&keymap, &query_words("graph"));
        let texts: Vec<&str> = graph
            .iter()
            .flat_map(|(s, rows)| rows.iter().map(|&i| s.entries[i].text))
            .collect();
        assert!(texts.contains(&"git graph"), "{texts:?}");
        assert!(texts.contains(&"code graph"), "{texts:?}");

        let github = visible(&keymap, &query_words("github"));
        assert_eq!(github.len(), 1);
        assert_eq!(github[0].1.len(), SECTIONS[4].entries.len());

        assert!(visible(&keymap, &query_words("zzzz nothing")).is_empty());
    }

    #[test]
    fn a_long_key_list_drops_whole_keycaps() {
        let many = vec![
            Piece::Cap("⇧Enter".into()),
            Piece::Cap("⇧O".into()),
            Piece::Cap("⌥Enter".into()),
        ];
        let fitted = fit_pieces(many, 11);
        assert_eq!(
            fitted,
            vec![
                Piece::Cap("⇧Enter".into()),
                Piece::Cap("⇧O".into()),
                Piece::More
            ]
        );
        assert!(pieces_width(&fitted) <= 11);
        // One column less and the second keycap goes too, whole.
        assert_eq!(
            fit_pieces(fitted.clone(), 10),
            vec![Piece::Cap("⇧Enter".into()), Piece::More]
        );
    }

    #[test]
    fn matches_are_lit_case_insensitively() {
        let spans = marked(
            "Git graph",
            &query_words("GRAPH"),
            Style::default(),
            Style::default().add_modifier(Modifier::BOLD),
        );
        let lit: Vec<&str> = spans
            .iter()
            .filter(|s| s.style.add_modifier.contains(Modifier::BOLD))
            .map(|s| s.content.as_ref())
            .collect();
        assert_eq!(lit, vec!["graph"]);
    }

    #[test]
    fn a_roomy_terminal_shows_every_section_at_once() {
        let (text, app) = render(170, 48, HelpView::default());
        for section in SECTIONS {
            assert!(text.contains(section.title), "{}: {text}", section.title);
        }
        assert!(text.contains("Keyboard shortcuts"), "{text}");
        assert!(text.contains("git graph"), "{text}");
        let Some(Overlay::Help(h)) = &app.modals.overlay else {
            panic!("help closed")
        };
        assert_eq!(h.max_scroll, 0, "nothing to scroll: {text}");
    }

    #[test]
    fn a_short_terminal_scrolls_and_says_so() {
        let (text, app) = render(80, 20, HelpView::default());
        assert!(text.contains("↓ more"), "{text}");
        let Some(Overlay::Help(h)) = &app.modals.overlay else {
            panic!("help closed")
        };
        assert!(h.max_scroll > 0);
    }

    #[test]
    fn a_filter_with_no_match_says_so() {
        let mut view = HelpView::default();
        view.query.insert_str("xyzzy");
        let (text, _) = render(120, 36, view);
        assert!(text.contains("No shortcut matches"), "{text}");
        assert!(text.contains("0 shortcuts"), "{text}");
    }
}
