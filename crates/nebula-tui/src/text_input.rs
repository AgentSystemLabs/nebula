//! Text field with the editing keys a terminal user expects — one line by
//! default, multi-row on request.
//!
//! Every typed field in the TUI — the prompt dialog, the fuzzy filters,
//! the grep query, the ssh destination, the task boxes, a preset's prefix
//! and postfix, an issue's description — is one of these, so the keys are
//! learned once and work everywhere: arrows and Home/End, word motion on
//! ⌥←/⌥→, the readline control chords (Ctrl+A/E/B/F/W/U/K), and word/line
//! deletes.
//!
//! A [`TextInput::multiline`] field holds hard line breaks as well, and
//! takes them the way Claude Code's own prompt does: Shift+Enter,
//! Option+Enter — the `ESC` `CR` a terminal without the kitty protocol
//! sends for a mapped Shift+Enter, which is Alt+Enter to us — and Ctrl+J
//! all break the line; ↑/↓ walk the rows as drawn — a long paragraph's
//! wrapped rows too, keeping the column through a short one — and fall
//! through to the caller past the first or last, so a form can step to its
//! next field; ⌥↑/⌥↓ jump by paragraph, Cmd+↑/↓ (Ctrl+Home/End) to either
//! end, PageUp/PageDown a screenful; Home/End and the readline chords work
//! on the line under the caret; a paste keeps its newlines. A one-line
//! field flattens a paste and leaves the break keys to the caller, so
//! Enter — always the caller's — stays the only way out of it.
//!
//! On macOS the option-arrow combos are what actually reaches us as
//! `Alt+b` / `Alt+f`: both Terminal.app (its bundled keyMappings.plist maps
//! `~F702`/`~F703` to `ESC b` / `ESC f`) and iTerm2 send the readline word
//! sequences rather than a modified arrow, so those two chords matter more
//! than `Alt+Left`/`Alt+Right` — we accept both.
//!
//! The field never claims a key an overlay wants for itself: `handle_key`
//! returns [`Edit::Ignored`] for anything it doesn't recognize, and callers
//! run it last, after their own bindings have had first refusal.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::fmt;
use std::ops::Deref;

/// What one key press did to the field.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Edit {
    /// Not an editing key — the caller still owns it.
    Ignored,
    /// The cursor moved (or hit an end); the text is unchanged.
    Moved,
    /// The text changed — re-run whatever this field feeds.
    Changed,
}

impl Edit {
    /// Did the key belong to the field at all?
    pub fn consumed(self) -> bool {
        !matches!(self, Edit::Ignored)
    }

    /// Does whatever this field drives (a filter, a search, a listing) need
    /// recomputing?
    pub fn changed(self) -> bool {
        matches!(self, Edit::Changed)
    }
}

/// Editable text plus a cursor into it: one line, or many.
#[derive(Debug, Clone, Default)]
pub struct TextInput {
    text: String,
    /// Byte offset into `text`; always on a char boundary, always ≤ len.
    cursor: usize,
    /// Hard line breaks allowed: the break keys insert one and a paste
    /// keeps its own. Off, the field is one line whatever comes in.
    multiline: bool,
    /// The column a run of ↑/↓ aims for, so passing through a short or
    /// empty row doesn't drag the caret to its end for good. Any other
    /// key lets it go.
    goal: Option<u16>,
    /// Where a multi-row field was last drawn (see [`TextView`]).
    view: TextView,
}

/// Where a multi-row field was last drawn: the columns its rows wrap at,
/// how many rows show, and the first one shown. The renderer hands it back
/// (draws work on a clone) so ↑/↓ walk the rows the user SEES — a long
/// paragraph wrapped over five rows is five rows, not one — PageUp/PageDown
/// move a screenful, and a click lands where it points. All zero before
/// the first draw, where the rows are the hard lines. In cells, as a
/// ratatui `Rect` counts them — every field carries one, so it stays small.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TextView {
    pub width: u16,
    pub height: u16,
    pub top: u16,
}

/// A row or column count as a [`TextView`] keeps it, saturating.
fn cells(n: usize) -> u16 {
    u16::try_from(n).unwrap_or(u16::MAX)
}

/// A field's value is its text, caret and shape; the column a run of ↑/↓
/// aims for and where it was last drawn are not part of it.
impl PartialEq for TextInput {
    fn eq(&self, other: &Self) -> bool {
        self.text == other.text && self.cursor == other.cursor && self.multiline == other.multiline
    }
}

impl Eq for TextInput {}

/// Word characters for ⌥-arrow / Ctrl+W motion: a run of these is one word,
/// everything else (spaces, `/`, `-`, `.`) separates. Matches what readline
/// does in a shell, which is where the muscle memory comes from.
fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// Does row `i` of a [`TextInput::rows`] layout end at a soft wrap — the
/// next row picking up at the very char it stopped before — rather than
/// at a line break or the end of the text?
fn is_soft(rows: &[(usize, usize)], i: usize) -> bool {
    rows.get(i + 1).is_some_and(|next| next.0 == rows[i].1)
}

impl TextInput {
    pub fn new() -> Self {
        Self::default()
    }

    /// A field pre-filled with `text`, cursor parked at the end — the state
    /// you want when an edit starts from an existing value.
    pub fn with_text(text: impl Into<String>) -> Self {
        let text = text.into();
        let cursor = text.len();
        Self {
            text,
            cursor,
            ..Self::default()
        }
    }

    /// An empty multi-row field: line breaks typed, pasted and walked.
    pub fn multiline() -> Self {
        Self {
            multiline: true,
            ..Self::default()
        }
    }

    /// A multi-row field pre-filled with `text`, cursor at the end.
    pub fn multiline_with_text(text: impl Into<String>) -> Self {
        let mut input = Self::with_text(text);
        input.multiline = true;
        input
    }

    /// Does the field hold hard line breaks?
    pub fn is_multiline(&self) -> bool {
        self.multiline
    }

    /// Switch line breaks on or off for a field built before its shape was
    /// known — a prompt dialog decides by its kind.
    pub fn set_multiline(&mut self, multiline: bool) {
        self.multiline = multiline;
    }

    /// Is `key` one of the chords that break a line — Shift+Enter,
    /// Option (Alt)+Enter or Ctrl+J? The three ways the one intent reaches
    /// a terminal program: the kitty protocol delivers the shifted Enter
    /// as a key of its own; a mapped Shift+Enter (Claude Code's
    /// `/terminal-setup`, or Option+Enter with Option as Meta) arrives as
    /// `ESC` `CR`, which is Alt+Enter; and Ctrl+J is the line feed itself,
    /// which every terminal and tmux pass through. Claude Code's prompt
    /// takes all three, so its muscle memory works here.
    pub fn is_newline_key(key: &KeyEvent) -> bool {
        match key.code {
            KeyCode::Enter => key
                .modifiers
                .intersects(KeyModifiers::SHIFT | KeyModifiers::ALT),
            KeyCode::Char('j' | 'J') => key.modifiers.contains(KeyModifiers::CONTROL),
            _ => false,
        }
    }

    /// Will [`handle_key`](Self::handle_key) turn `key` into a line break
    /// here? Only in a multi-row field. A caller whose Enter submits guards
    /// that arm with this, so a shifted Enter is never a send in a box
    /// that breaks lines.
    pub fn takes_newline(&self, key: &KeyEvent) -> bool {
        self.multiline && Self::is_newline_key(key)
    }

    pub fn as_str(&self) -> &str {
        &self.text
    }

    /// Cursor as a char offset — the unit the renderer draws in.
    pub fn cursor_chars(&self) -> usize {
        self.text[..self.cursor].chars().count()
    }

    /// Park the caret `at` chars in, clamped to the end of the text.
    fn set_cursor_chars(&mut self, at: usize) {
        self.cursor = self
            .text
            .char_indices()
            .nth(at)
            .map_or(self.text.len(), |(i, _)| i);
    }

    /// Caret to the very start of the text — ↑ on a task box's top row.
    pub fn cursor_to_start(&mut self) {
        self.goal = None;
        self.cursor = 0;
    }

    /// Caret to the very end of the text — ↓ on a task box's bottom row.
    pub fn cursor_to_end(&mut self) {
        self.goal = None;
        self.cursor = self.text.len();
    }

    /// Replace the whole value, cursor to the end.
    pub fn set_text(&mut self, text: impl Into<String>) {
        self.text = text.into();
        self.cursor = self.text.len();
        self.goal = None;
    }

    pub fn clear(&mut self) {
        self.text.clear();
        self.cursor = 0;
        self.goal = None;
    }

    /// The text as the rows a `width`-column box shows it in, as char
    /// ranges. A hard line break always ends a row (an empty line is an
    /// empty row); a line wider than the box breaks after the last space
    /// that fits — mid-word only for a word wider than the row — and the
    /// row keeps that space. The one layout the renderer draws and ↑/↓,
    /// PageUp/PageDown, the wheel and a click walk. Width 0 wraps nothing:
    /// the rows are the hard lines.
    pub fn rows(&self, width: usize) -> Vec<(usize, usize)> {
        let chars: Vec<char> = self.text.chars().collect();
        let width = if width == 0 { usize::MAX } else { width };
        let mut rows = Vec::new();
        let mut line_start = 0;
        loop {
            let line_end = chars[line_start..]
                .iter()
                .position(|c| *c == '\n')
                .map_or(chars.len(), |i| line_start + i);
            if line_start == line_end {
                rows.push((line_start, line_end));
            }
            let mut start = line_start;
            while start < line_end {
                let hard_end = start.saturating_add(width).min(line_end);
                let end = if hard_end < line_end {
                    chars[start..hard_end]
                        .iter()
                        .rposition(|c| c.is_whitespace())
                        .map_or(hard_end, |i| start + i + 1)
                } else {
                    hard_end
                };
                rows.push((start, end));
                start = end;
            }
            if line_end == chars.len() {
                return rows;
            }
            line_start = line_end + 1;
        }
    }

    /// Which of `rows` the caret is drawn on. At a soft break it starts
    /// the next row; at a hard break, or the end of the text, it closes
    /// its own.
    pub fn caret_row(&self, rows: &[(usize, usize)]) -> usize {
        let caret = self.cursor_chars();
        rows.iter()
            .enumerate()
            .position(|(i, &(start, end))| {
                start <= caret && (caret < end || (caret == end && !is_soft(rows, i)))
            })
            .unwrap_or(rows.len().saturating_sub(1))
    }

    /// Where the field was last drawn.
    pub fn view(&self) -> TextView {
        self.view
    }

    /// Record where the field was drawn — what [`view_for`](Self::view_for)
    /// returned — on the live field, the draw having worked on a clone.
    pub fn set_view(&mut self, view: TextView) {
        self.view = view;
    }

    /// The view to draw a `width` × `height` box with: the last draw's
    /// first row, moved only as far as brings the caret into sight, and
    /// never so far down that rows go unused below the text. So ↑/↓ inside
    /// the box leave it still, and the text doesn't jump about as it is
    /// typed.
    pub fn view_for(&self, width: u16, height: u16) -> TextView {
        let rows = self.rows(width.into());
        let caret = self.caret_row(&rows);
        let height = height.max(1);
        let shown = usize::from(height);
        let top = usize::from(self.view.top)
            .min(rows.len().saturating_sub(shown))
            .clamp(caret.saturating_sub(shown - 1), caret);
        TextView {
            width,
            height,
            top: cells(top),
        }
    }

    /// The wheel over the box: scroll it `delta` rows, bringing the caret
    /// along only when it would leave the rows in sight. [`Edit::Ignored`]
    /// with nowhere further to scroll.
    pub fn scroll_rows(&mut self, delta: isize) -> Edit {
        let rows = self.rows(self.view.width.into());
        let height = usize::from(self.view.height.max(1));
        let old = usize::from(self.view.top);
        let top = old
            .saturating_add_signed(delta)
            .min(rows.len().saturating_sub(height));
        if top == old {
            return Edit::Ignored;
        }
        self.view.top = cells(top);
        let row = self.caret_row(&rows);
        let into = row.clamp(top, top + height - 1);
        if into != row {
            let col = self
                .goal
                .map_or(self.cursor_chars() - rows[row].0, usize::from);
            self.land(&rows, into, col);
            self.goal = Some(cells(col));
        }
        Edit::Moved
    }

    /// A click `row` rows down and `col` columns into the box as last
    /// drawn: the caret to that spot — or the end of that row, clicked
    /// past it, and the end of the text, clicked below the last row.
    pub fn click(&mut self, row: u16, col: u16) {
        self.goal = None;
        let rows = self.rows(self.view.width.into());
        let row = usize::from(self.view.top) + usize::from(row);
        match rows.get(row) {
            Some(_) => self.land(&rows, row, col.into()),
            None => self.cursor = self.text.len(),
        }
    }

    pub fn insert_char(&mut self, c: char) {
        self.text.insert(self.cursor, c);
        self.cursor += c.len_utf8();
    }

    /// Insert a whole run at the cursor — a bracketed paste. A multi-row
    /// field keeps its line breaks (`\r\n` and a bare `\r` become `\n`);
    /// a one-line field turns each into a space.
    pub fn insert_str(&mut self, s: &str) {
        let normalized = s.replace("\r\n", "\n").replace('\r', "\n");
        let run = if self.multiline {
            normalized
        } else {
            normalized.replace('\n', " ")
        };
        self.text.insert_str(self.cursor, &run);
        self.cursor += run.len();
        self.goal = None;
    }

    /// Apply one key press. Returns [`Edit::Ignored`] for anything that
    /// isn't an editing key, leaving it for the caller.
    pub fn handle_key(&mut self, key: &KeyEvent) -> Edit {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        // Cmd on macOS, when a terminal delivers it at all: line-wise.
        let cmd = key
            .modifiers
            .intersects(KeyModifiers::SUPER | KeyModifiers::META | KeyModifiers::HYPER);
        // A run of ↑/↓ (PageUp/PageDown) keeps aiming for the column it
        // set out from; any other key starts afresh.
        let goal = self.goal.take();

        match key.code {
            // ---- line breaks (multi-row fields only) ----
            _ if self.takes_newline(key) => {
                self.insert_char('\n');
                Edit::Changed
            }

            // ---- motion ----
            // Line-wise keys work on the line under the caret — the whole
            // text, in a one-line field.
            KeyCode::Left if cmd => self.move_to(self.line_start(self.cursor)),
            KeyCode::Left if alt || ctrl => self.move_to(self.word_left(self.cursor)),
            KeyCode::Left => self.move_to(self.prev_boundary(self.cursor)),
            KeyCode::Right if cmd => self.move_to(self.line_end(self.cursor)),
            KeyCode::Right if alt || ctrl => self.move_to(self.word_right(self.cursor)),
            KeyCode::Right => self.move_to(self.next_boundary(self.cursor)),
            KeyCode::Home if ctrl || cmd => self.move_to(0),
            KeyCode::End if ctrl || cmd => self.move_to(self.text.len()),
            KeyCode::Home => self.move_to(self.line_start(self.cursor)),
            KeyCode::End => self.move_to(self.line_end(self.cursor)),
            // ↑/↓ walk a multi-row field's rows as drawn — a paragraph
            // wrapped over five rows is five rows — keeping the column;
            // past the first or last row they are the caller's (a form
            // steps to its next field). ⌥↑/⌥↓ jump to the start or end of
            // the paragraph, then the one before or after, as a macOS text
            // view does; Cmd+↑/↓ (Ctrl+Home/End) to the text's very start
            // or end; PageUp/PageDown a screenful. A one-line field has no
            // second row: all of these stay the caller's there.
            KeyCode::Up | KeyCode::Down | KeyCode::PageUp | KeyCode::PageDown
                if !self.multiline =>
            {
                Edit::Ignored
            }
            KeyCode::Up if cmd => self.move_to(0),
            KeyCode::Down if cmd => self.move_to(self.text.len()),
            KeyCode::Up if alt => self.paragraph_up(),
            KeyCode::Down if alt => self.paragraph_down(),
            KeyCode::Up => self.move_rows(-1, goal),
            KeyCode::Down => self.move_rows(1, goal),
            KeyCode::PageUp => self.page(-1, goal),
            KeyCode::PageDown => self.page(1, goal),

            // ---- deletion ----
            // Cmd+⌫ kills the line, ⌥⌫ / Ctrl+⌫ the previous word.
            KeyCode::Backspace if cmd => self.delete(self.line_start(self.cursor), self.cursor),
            KeyCode::Backspace if alt || ctrl => {
                self.delete(self.word_left(self.cursor), self.cursor)
            }
            KeyCode::Backspace => self.delete(self.prev_boundary(self.cursor), self.cursor),
            KeyCode::Delete if cmd => self.delete(self.cursor, self.line_end(self.cursor)),
            KeyCode::Delete if alt || ctrl => {
                self.delete(self.cursor, self.word_right(self.cursor))
            }
            KeyCode::Delete => self.delete(self.cursor, self.next_boundary(self.cursor)),

            // ---- readline chords ----
            // Cmd+key never means "type this" — leave it to the caller.
            KeyCode::Char(_) if cmd => Edit::Ignored,
            KeyCode::Char(c) if ctrl => match c.to_ascii_lowercase() {
                'a' => self.move_to(self.line_start(self.cursor)),
                'e' => self.move_to(self.line_end(self.cursor)),
                'b' => self.move_to(self.prev_boundary(self.cursor)),
                'f' => self.move_to(self.next_boundary(self.cursor)),
                'd' => self.delete(self.cursor, self.next_boundary(self.cursor)),
                'w' => self.delete(self.word_left(self.cursor), self.cursor),
                'u' => self.delete(self.line_start(self.cursor), self.cursor),
                // To the end of the line — or, standing at its end, the
                // line break itself, as readline does.
                'k' => match self.line_end(self.cursor) {
                    end if end == self.cursor => {
                        self.delete(self.cursor, self.next_boundary(self.cursor))
                    }
                    end => self.delete(self.cursor, end),
                },
                _ => Edit::Ignored,
            },
            // ⌥b/⌥f are what macOS terminals send for ⌥←/⌥→; ⌥d is
            // readline's kill-word-forward.
            KeyCode::Char(c) if alt => match c.to_ascii_lowercase() {
                'b' => self.move_to(self.word_left(self.cursor)),
                'f' => self.move_to(self.word_right(self.cursor)),
                'd' => self.delete(self.cursor, self.word_right(self.cursor)),
                // Some emulators send ⌥⌫ as ESC + DEL rather than a
                // modified Backspace key.
                '\u{7f}' | '\u{8}' => self.delete(self.word_left(self.cursor), self.cursor),
                _ => Edit::Ignored,
            },

            // ---- text ----
            // Plain (or shifted) printable keys, including the glyphs a Mac
            // makes from ⌥-letters when the profile isn't option-as-meta.
            KeyCode::Char(c) => {
                self.insert_char(c);
                Edit::Changed
            }
            _ => Edit::Ignored,
        }
    }

    // ---- internals ----

    fn move_to(&mut self, at: usize) -> Edit {
        self.cursor = at;
        Edit::Moved
    }

    fn delete(&mut self, start: usize, end: usize) -> Edit {
        if start >= end {
            // Backspace at column 0 is still the field's key — swallow it so
            // an overlay doesn't read it as "delete the selected row".
            return Edit::Moved;
        }
        self.text.replace_range(start..end, "");
        self.cursor = start;
        Edit::Changed
    }

    fn char_before(&self, at: usize) -> Option<char> {
        self.text[..at].chars().next_back()
    }

    fn char_at(&self, at: usize) -> Option<char> {
        self.text[at..].chars().next()
    }

    fn prev_boundary(&self, at: usize) -> usize {
        self.char_before(at).map_or(at, |c| at - c.len_utf8())
    }

    fn next_boundary(&self, at: usize) -> usize {
        self.char_at(at).map_or(at, |c| at + c.len_utf8())
    }

    /// Start of the line `at` sits on: just past the previous line break,
    /// or 0 — always 0 in a one-line field.
    fn line_start(&self, at: usize) -> usize {
        self.text[..at].rfind('\n').map_or(0, |i| i + 1)
    }

    /// End of the line `at` sits on: its line break, or the end of the
    /// text — always the end in a one-line field.
    fn line_end(&self, at: usize) -> usize {
        self.text[at..]
            .find('\n')
            .map_or(self.text.len(), |i| at + i)
    }

    /// The caret `col` chars into `row` — or as far as that row lets it
    /// stand: a soft-wrapped row's last spot is its final char (one step
    /// on is the next row's start), a hard line's is its end.
    fn land(&mut self, rows: &[(usize, usize)], row: usize, col: usize) {
        let (start, end) = rows[row];
        let last = if is_soft(rows, row) { end - 1 } else { end };
        self.set_cursor_chars((start + col).min(last));
    }

    /// The caret `delta` rows up (−) or down, as last drawn, at the goal
    /// column — clamped to the first and last rows, and [`Edit::Ignored`]
    /// when it already stands there, so the caller can act on the key.
    fn move_rows(&mut self, delta: isize, goal: Option<u16>) -> Edit {
        let rows = self.rows(self.view.width.into());
        let row = self.caret_row(&rows);
        let target = row.saturating_add_signed(delta).min(rows.len() - 1);
        if target == row {
            self.goal = goal;
            return Edit::Ignored;
        }
        let col = goal.map_or(self.cursor_chars() - rows[row].0, usize::from);
        self.land(&rows, target, col);
        self.goal = Some(cells(col));
        Edit::Moved
    }

    /// PageUp/PageDown: a screenful of rows at the goal column, keeping
    /// one row of the old screen in sight, the box scrolling with the
    /// caret; from the first or last row, the very start or end of the
    /// text.
    fn page(&mut self, dir: isize, goal: Option<u16>) -> Edit {
        let rows = self.rows(self.view.width.into());
        let before = self.caret_row(&rows) as isize;
        let step = self.view.height.saturating_sub(1).max(1) as isize;
        match self.move_rows(dir * step, goal) {
            Edit::Ignored => {
                self.goal = None;
                self.move_to(if dir < 0 { 0 } else { self.text.len() })
            }
            moved => {
                let after = self.caret_row(&rows) as isize;
                let top = usize::from(self.view.top).saturating_add_signed(after - before);
                self.view.top = cells(top);
                moved
            }
        }
    }

    /// ⌥↑: to the start of the paragraph — the hard line — the caret is
    /// in, or from its start to the previous paragraph's. [`Edit::Ignored`]
    /// at the very start.
    fn paragraph_up(&mut self) -> Edit {
        if self.cursor == 0 {
            return Edit::Ignored;
        }
        let start = self.line_start(self.cursor);
        if start < self.cursor {
            return self.move_to(start);
        }
        self.move_to(self.line_start(start - 1))
    }

    /// ⌥↓: to the end of the paragraph the caret is in, or from its end
    /// to the next paragraph's. [`Edit::Ignored`] at the very end.
    fn paragraph_down(&mut self) -> Edit {
        if self.cursor == self.text.len() {
            return Edit::Ignored;
        }
        let end = self.line_end(self.cursor);
        if end > self.cursor {
            return self.move_to(end);
        }
        self.move_to(self.line_end(end + 1))
    }

    /// Start of the word at or before `at`: skip back over separators, then
    /// over the word itself (readline's `backward-word`).
    fn word_left(&self, mut at: usize) -> usize {
        while self.char_before(at).is_some_and(|c| !is_word(c)) {
            at = self.prev_boundary(at);
        }
        while self.char_before(at).is_some_and(is_word) {
            at = self.prev_boundary(at);
        }
        at
    }

    /// End of the word at or after `at` (readline's `forward-word`).
    fn word_right(&self, mut at: usize) -> usize {
        while self.char_at(at).is_some_and(|c| !is_word(c)) {
            at = self.next_boundary(at);
        }
        while self.char_at(at).is_some_and(is_word) {
            at = self.next_boundary(at);
        }
        at
    }
}

/// Read access is just `&str`, so every `is_empty()` / `chars()` / `trim()`
/// call site — and every `&str` argument — keeps working unchanged.
impl Deref for TextInput {
    type Target = str;

    fn deref(&self) -> &str {
        &self.text
    }
}

impl fmt::Display for TextInput {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.text)
    }
}

impl From<String> for TextInput {
    fn from(text: String) -> Self {
        Self::with_text(text)
    }
}

impl From<&str> for TextInput {
    fn from(text: &str) -> Self {
        Self::with_text(text)
    }
}

impl PartialEq<str> for TextInput {
    fn eq(&self, other: &str) -> bool {
        self.text == other
    }
}

impl PartialEq<&str> for TextInput {
    fn eq(&self, other: &&str) -> bool {
        self.text == *other
    }
}

impl PartialEq<String> for TextInput {
    fn eq(&self, other: &String) -> bool {
        &self.text == other
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: KeyCode, mods: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, mods)
    }

    /// Type `s` a character at a time, as the event loop would.
    fn typed(s: &str) -> TextInput {
        let mut input = TextInput::new();
        for c in s.chars() {
            input.handle_key(&key(KeyCode::Char(c), KeyModifiers::NONE));
        }
        input
    }

    fn press(input: &mut TextInput, code: KeyCode, mods: KeyModifiers) -> Edit {
        input.handle_key(&key(code, mods))
    }

    #[test]
    fn typing_appends_and_tracks_the_cursor() {
        let input = typed("hello");
        assert_eq!(input.as_str(), "hello");
        assert_eq!(input.cursor_chars(), 5);
    }

    #[test]
    fn arrows_move_and_typing_inserts_at_the_cursor() {
        let mut input = typed("hello");
        press(&mut input, KeyCode::Left, KeyModifiers::NONE);
        press(&mut input, KeyCode::Left, KeyModifiers::NONE);
        assert_eq!(input.cursor_chars(), 3);
        assert_eq!(
            press(&mut input, KeyCode::Char('X'), KeyModifiers::NONE),
            Edit::Changed
        );
        assert_eq!(input.as_str(), "helXlo");
        assert_eq!(input.cursor_chars(), 4);
    }

    #[test]
    fn backspace_deletes_before_the_cursor_only() {
        let mut input = typed("hello");
        press(&mut input, KeyCode::Home, KeyModifiers::NONE);
        press(&mut input, KeyCode::Right, KeyModifiers::NONE);
        press(&mut input, KeyCode::Backspace, KeyModifiers::NONE);
        assert_eq!(input.as_str(), "ello");
        assert_eq!(input.cursor_chars(), 0);
        // At column 0 it is still the field's key — consumed, not passed on.
        assert_eq!(
            press(&mut input, KeyCode::Backspace, KeyModifiers::NONE),
            Edit::Moved
        );
        assert_eq!(input.as_str(), "ello");
    }

    #[test]
    fn delete_removes_forward() {
        let mut input = typed("hello");
        press(&mut input, KeyCode::Home, KeyModifiers::NONE);
        press(&mut input, KeyCode::Delete, KeyModifiers::NONE);
        assert_eq!(input.as_str(), "ello");
        press(&mut input, KeyCode::Char('d'), KeyModifiers::CONTROL);
        assert_eq!(input.as_str(), "llo");
    }

    /// What ⌥← actually sends on macOS: ESC b, i.e. crossterm's Alt+b.
    #[test]
    fn option_arrows_arrive_as_alt_b_and_alt_f() {
        let mut input = typed("fix the login redirect");
        assert_eq!(
            press(&mut input, KeyCode::Char('b'), KeyModifiers::ALT),
            Edit::Moved
        );
        assert_eq!(input.cursor_chars(), "fix the login ".len());
        press(&mut input, KeyCode::Char('b'), KeyModifiers::ALT);
        assert_eq!(input.cursor_chars(), "fix the ".len());
        press(&mut input, KeyCode::Char('f'), KeyModifiers::ALT);
        assert_eq!(input.cursor_chars(), "fix the login".len());
    }

    #[test]
    fn alt_and_ctrl_arrows_move_by_word_too() {
        let mut input = typed("one two three");
        press(&mut input, KeyCode::Left, KeyModifiers::ALT);
        assert_eq!(input.cursor_chars(), "one two ".len());
        press(&mut input, KeyCode::Left, KeyModifiers::CONTROL);
        assert_eq!(input.cursor_chars(), "one ".len());
        press(&mut input, KeyCode::Right, KeyModifiers::ALT);
        assert_eq!(input.cursor_chars(), "one two".len());
    }

    #[test]
    fn word_motion_treats_punctuation_as_a_separator() {
        let mut input = typed("~/src/nebula-tui/app.rs");
        press(&mut input, KeyCode::Char('b'), KeyModifiers::ALT);
        assert_eq!(input.cursor_chars(), "~/src/nebula-tui/app.".len());
        press(&mut input, KeyCode::Char('b'), KeyModifiers::ALT);
        assert_eq!(input.cursor_chars(), "~/src/nebula-tui/".len());
    }

    #[test]
    fn word_and_line_deletes() {
        let mut input = typed("one two three");
        assert_eq!(
            press(&mut input, KeyCode::Char('w'), KeyModifiers::CONTROL),
            Edit::Changed
        );
        assert_eq!(input.as_str(), "one two ");
        press(&mut input, KeyCode::Backspace, KeyModifiers::ALT);
        assert_eq!(input.as_str(), "one ");
        press(&mut input, KeyCode::Char('u'), KeyModifiers::CONTROL);
        assert_eq!(input.as_str(), "");
    }

    #[test]
    fn ctrl_k_kills_to_the_end_and_ctrl_a_e_jump() {
        let mut input = typed("keep this cut this");
        press(&mut input, KeyCode::Char('a'), KeyModifiers::CONTROL);
        assert_eq!(input.cursor_chars(), 0);
        for _ in 0.."keep this ".len() {
            press(&mut input, KeyCode::Char('f'), KeyModifiers::CONTROL);
        }
        press(&mut input, KeyCode::Char('k'), KeyModifiers::CONTROL);
        assert_eq!(input.as_str(), "keep this ");
        press(&mut input, KeyCode::Char('e'), KeyModifiers::CONTROL);
        assert_eq!(input.cursor_chars(), 10);
    }

    #[test]
    fn alt_d_kills_the_word_ahead() {
        let mut input = typed("alpha beta");
        press(&mut input, KeyCode::Home, KeyModifiers::NONE);
        press(&mut input, KeyCode::Char('d'), KeyModifiers::ALT);
        assert_eq!(input.as_str(), " beta");
    }

    #[test]
    fn multibyte_text_moves_by_whole_characters() {
        let mut input = typed("héllo→");
        press(&mut input, KeyCode::Left, KeyModifiers::NONE);
        press(&mut input, KeyCode::Backspace, KeyModifiers::NONE);
        assert_eq!(input.as_str(), "héll→");
        assert_eq!(input.cursor_chars(), 4);
    }

    #[test]
    fn unknown_keys_are_left_to_the_caller() {
        let mut input = typed("x");
        assert_eq!(
            press(&mut input, KeyCode::Enter, KeyModifiers::NONE),
            Edit::Ignored
        );
        assert_eq!(
            press(&mut input, KeyCode::Esc, KeyModifiers::NONE),
            Edit::Ignored
        );
        assert_eq!(
            press(&mut input, KeyCode::Tab, KeyModifiers::NONE),
            Edit::Ignored
        );
        assert_eq!(
            press(&mut input, KeyCode::Up, KeyModifiers::NONE),
            Edit::Ignored
        );
        assert_eq!(
            press(&mut input, KeyCode::Char('n'), KeyModifiers::CONTROL),
            Edit::Ignored
        );
        assert_eq!(input.as_str(), "x");
    }

    /// One paste, two fields: the one-line field spaces out every kind of
    /// line break, the multi-row field keeps them all as `\n`.
    #[test]
    fn a_paste_keeps_its_lines_only_in_a_multi_row_field() {
        let mut one = typed("ab");
        press(&mut one, KeyCode::Left, KeyModifiers::NONE);
        one.insert_str("one\r\ntwo\rthree");
        assert_eq!(one.as_str(), "aone two threeb");
        assert_eq!(one.cursor_chars(), 14);

        let mut many = TextInput::multiline_with_text("ab");
        press(&mut many, KeyCode::Left, KeyModifiers::NONE);
        many.insert_str("one\r\ntwo\rthree");
        assert_eq!(many.as_str(), "aone\ntwo\nthreeb");
        assert_eq!(many.cursor_chars(), 14);
    }

    /// The three chords Claude Code's prompt breaks a line on — the kitty
    /// protocol's Shift+Enter, the `ESC` `CR` (Alt+Enter) a mapped
    /// Shift+Enter or Option+Enter sends, and Ctrl+J — all break one
    /// here; a plain Enter is still the caller's.
    #[test]
    fn a_multi_row_field_breaks_lines_three_ways() {
        let mut input = TextInput::multiline();
        for c in "one".chars() {
            press(&mut input, KeyCode::Char(c), KeyModifiers::NONE);
        }
        assert_eq!(
            press(&mut input, KeyCode::Enter, KeyModifiers::SHIFT),
            Edit::Changed
        );
        for c in "two".chars() {
            press(&mut input, KeyCode::Char(c), KeyModifiers::NONE);
        }
        assert_eq!(
            press(&mut input, KeyCode::Enter, KeyModifiers::ALT),
            Edit::Changed
        );
        for c in "three".chars() {
            press(&mut input, KeyCode::Char(c), KeyModifiers::NONE);
        }
        assert_eq!(
            press(&mut input, KeyCode::Char('j'), KeyModifiers::CONTROL),
            Edit::Changed
        );
        assert_eq!(input.as_str(), "one\ntwo\nthree\n");
        assert_eq!(
            press(&mut input, KeyCode::Enter, KeyModifiers::NONE),
            Edit::Ignored,
            "Enter is the caller's send or save"
        );
        assert!(input.takes_newline(&key(KeyCode::Enter, KeyModifiers::SHIFT)));
        assert!(!input.takes_newline(&key(KeyCode::Enter, KeyModifiers::NONE)));
    }

    /// A one-line field has no line to break: the chords are recognized
    /// (a form can still act on them) but left to the caller untouched.
    #[test]
    fn a_one_line_field_leaves_the_break_keys_to_the_caller() {
        let mut input = typed("one");
        for (code, mods) in [
            (KeyCode::Enter, KeyModifiers::SHIFT),
            (KeyCode::Enter, KeyModifiers::ALT),
            (KeyCode::Char('j'), KeyModifiers::CONTROL),
        ] {
            assert!(TextInput::is_newline_key(&key(code, mods)), "{code:?}");
            assert!(!input.takes_newline(&key(code, mods)), "{code:?}");
            assert_eq!(press(&mut input, code, mods), Edit::Ignored, "{code:?}");
        }
        assert_eq!(input.as_str(), "one");
        assert!(!TextInput::is_newline_key(&key(
            KeyCode::Enter,
            KeyModifiers::NONE
        )));
    }

    /// ↑/↓ walk the lines keeping the column — clamped on a shorter line,
    /// and back to it past one — and past the first or last line they are
    /// Ignored, so a form can step to its next field on the very same key.
    #[test]
    fn arrows_walk_the_lines_and_fall_through_at_the_ends() {
        let mut input = TextInput::multiline_with_text("first line\nhi\nthird");
        // From the end of "third" (column 5): "hi" is shorter, so its end.
        assert_eq!(
            press(&mut input, KeyCode::Up, KeyModifiers::NONE),
            Edit::Moved
        );
        assert_eq!(input.cursor_chars(), "first line\nhi".len());
        // Still aiming for column 5 on the first line, not hi's 2.
        assert_eq!(
            press(&mut input, KeyCode::Up, KeyModifiers::NONE),
            Edit::Moved
        );
        assert_eq!(input.cursor_chars(), "first".len());
        assert_eq!(
            press(&mut input, KeyCode::Up, KeyModifiers::NONE),
            Edit::Ignored,
            "no line above the first"
        );
        assert_eq!(input.cursor_chars(), "first".len(), "the caret stays put");
        press(&mut input, KeyCode::Down, KeyModifiers::NONE);
        press(&mut input, KeyCode::Down, KeyModifiers::NONE);
        assert_eq!(input.cursor_chars(), "first line\nhi\nthird".len());
        assert_eq!(
            press(&mut input, KeyCode::Down, KeyModifiers::NONE),
            Edit::Ignored,
            "no line below the last"
        );
        // Any other key lets the column go: from "fi|rst", ↓ lands at 2.
        press(&mut input, KeyCode::Up, KeyModifiers::NONE);
        press(&mut input, KeyCode::Up, KeyModifiers::NONE);
        press(&mut input, KeyCode::Home, KeyModifiers::NONE);
        press(&mut input, KeyCode::Right, KeyModifiers::NONE);
        press(&mut input, KeyCode::Right, KeyModifiers::NONE);
        press(&mut input, KeyCode::Down, KeyModifiers::NONE);
        press(&mut input, KeyCode::Down, KeyModifiers::NONE);
        assert_eq!(input.cursor_chars(), "first line\nhi\nth".len());
        // A one-line field never walks: still the caller's keys.
        let mut one = typed("solo");
        assert_eq!(
            press(&mut one, KeyCode::Up, KeyModifiers::NONE),
            Edit::Ignored
        );
        assert_eq!(
            press(&mut one, KeyCode::Down, KeyModifiers::NONE),
            Edit::Ignored
        );
    }

    /// Home/End, Ctrl+A/E and the line kills act on the line under the
    /// caret, and Ctrl+K at a line's end joins it to the next.
    #[test]
    fn line_keys_work_on_the_line_under_the_caret() {
        let mut input = TextInput::multiline_with_text("keep this\ncut here");
        press(&mut input, KeyCode::Home, KeyModifiers::NONE);
        assert_eq!(input.cursor_chars(), "keep this\n".len());
        press(&mut input, KeyCode::Char('e'), KeyModifiers::CONTROL);
        assert_eq!(input.cursor_chars(), "keep this\ncut here".len());
        press(&mut input, KeyCode::Char('a'), KeyModifiers::CONTROL);
        press(&mut input, KeyCode::Char('k'), KeyModifiers::CONTROL);
        assert_eq!(
            input.as_str(),
            "keep this\n",
            "^K kills the second line only"
        );
        press(&mut input, KeyCode::Up, KeyModifiers::NONE);
        press(&mut input, KeyCode::End, KeyModifiers::NONE);
        assert_eq!(input.cursor_chars(), "keep this".len());
        press(&mut input, KeyCode::Char('k'), KeyModifiers::CONTROL);
        assert_eq!(input.as_str(), "keep this", "^K at the end eats the break");
        let mut input = TextInput::multiline_with_text("one\ntwo three");
        press(&mut input, KeyCode::Char('u'), KeyModifiers::CONTROL);
        assert_eq!(input.as_str(), "one\n", "^U stops at the line's start");
        assert_eq!(input.cursor_chars(), 4);
        press(&mut input, KeyCode::Backspace, KeyModifiers::NONE);
        assert_eq!(input.as_str(), "one", "⌫ at a line's start joins it");
    }

    /// A multi-row field as a `width` × `height` box last drew it, caret
    /// at the end.
    fn drawn(text: &str, width: u16, height: u16) -> TextInput {
        let mut input = TextInput::multiline_with_text(text);
        let view = input.view_for(width, height);
        input.set_view(view);
        input
    }

    /// The row the caret is drawn on, in the layout the box last used.
    fn row_of(input: &TextInput) -> usize {
        input.caret_row(&input.rows(input.view().width.into()))
    }

    const FOX: &str = "the quick brown fox jumps over the lazy dog";

    /// Ten columns wide the fox wraps into `the quick |brown fox |jumps
    /// |over the |lazy dog`.
    #[test]
    fn rows_wrap_after_the_last_space_that_fits() {
        let input = TextInput::multiline_with_text(FOX);
        assert_eq!(
            input.rows(10),
            vec![(0, 10), (10, 20), (20, 26), (26, 35), (35, 43)]
        );
        assert_eq!(input.rows(0), vec![(0, 43)], "width 0 wraps nothing");
        let hard = TextInput::multiline_with_text("one\n\nabcdefghij");
        assert_eq!(
            hard.rows(4),
            vec![(0, 3), (4, 4), (5, 9), (9, 13), (13, 15)],
            "a word wider than the row breaks mid-word"
        );
    }

    /// The prompt nobody pressed Shift+Enter in is ONE line: ↑/↓ must walk
    /// the rows it wraps into as drawn, keeping the column, or there is no
    /// way up it but ←.
    #[test]
    fn arrows_walk_the_rows_a_long_paragraph_wraps_into() {
        let mut input = drawn(FOX, 10, 5);
        assert_eq!(row_of(&input), 4);
        // Column 8 all the way up; "jumps " stops at its space.
        let ups = [34, 25, 18, 8];
        for want in ups {
            assert_eq!(
                press(&mut input, KeyCode::Up, KeyModifiers::NONE),
                Edit::Moved
            );
            assert_eq!(input.cursor_chars(), want);
        }
        assert_eq!(
            press(&mut input, KeyCode::Up, KeyModifiers::NONE),
            Edit::Ignored,
            "the top row is the caller's"
        );
        for want in [18, 25, 34, 43] {
            press(&mut input, KeyCode::Down, KeyModifiers::NONE);
            assert_eq!(input.cursor_chars(), want);
        }
        // Never drawn, the paragraph is one row, as it always was.
        let mut undrawn = TextInput::multiline_with_text(FOX);
        assert_eq!(
            press(&mut undrawn, KeyCode::Up, KeyModifiers::NONE),
            Edit::Ignored
        );
    }

    /// ⌥↑/⌥↓ — what the user reached for — jump by paragraph as a macOS
    /// text view does: its start (end), then the one before (after).
    #[test]
    fn option_arrows_jump_by_paragraph() {
        let text = "alpha beta\ngamma\n\ndelta";
        let mut input = drawn(text, 80, 5);
        for want in [18, 17, 11, 0] {
            assert_eq!(
                press(&mut input, KeyCode::Up, KeyModifiers::ALT),
                Edit::Moved
            );
            assert_eq!(input.cursor_chars(), want);
        }
        assert_eq!(
            press(&mut input, KeyCode::Up, KeyModifiers::ALT),
            Edit::Ignored
        );
        for want in [10, 16, 17, 23] {
            press(&mut input, KeyCode::Down, KeyModifiers::ALT);
            assert_eq!(input.cursor_chars(), want);
        }
        assert_eq!(
            press(&mut input, KeyCode::Down, KeyModifiers::ALT),
            Edit::Ignored
        );
        let mut one = typed("solo");
        assert_eq!(
            press(&mut one, KeyCode::Up, KeyModifiers::ALT),
            Edit::Ignored,
            "a one-line field leaves it to the caller"
        );
    }

    #[test]
    fn cmd_arrows_and_ctrl_home_end_reach_the_ends_of_the_text() {
        let mut input = drawn("one\ntwo\nthree", 80, 5);
        press(&mut input, KeyCode::Home, KeyModifiers::CONTROL);
        assert_eq!(input.cursor_chars(), 0);
        press(&mut input, KeyCode::End, KeyModifiers::CONTROL);
        assert_eq!(input.cursor_chars(), 13);
        press(&mut input, KeyCode::Up, KeyModifiers::SUPER);
        assert_eq!(input.cursor_chars(), 0);
        press(&mut input, KeyCode::Down, KeyModifiers::SUPER);
        assert_eq!(input.cursor_chars(), 13);
    }

    fn twenty_lines() -> String {
        (0..20)
            .map(|i| format!("l{i}"))
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// PageUp/PageDown move a screenful less one row, the box going with
    /// the caret, and from the first or last row to the text's very end.
    #[test]
    fn page_keys_move_a_screenful() {
        let mut input = drawn(&twenty_lines(), 20, 5);
        assert_eq!((row_of(&input), input.view().top), (19, 15));
        for (row, top) in [(15, 11), (11, 7), (7, 3), (3, 0), (0, 0)] {
            assert_eq!(
                press(&mut input, KeyCode::PageUp, KeyModifiers::NONE),
                Edit::Moved
            );
            let view = input.view_for(20, 5);
            input.set_view(view);
            assert_eq!((row_of(&input), view.top), (row, top));
        }
        assert_eq!(input.cursor_chars(), 2, "column 3 clamped on l0");
        press(&mut input, KeyCode::PageUp, KeyModifiers::NONE);
        assert_eq!(input.cursor_chars(), 0, "the first row's page is the start");
        press(&mut input, KeyCode::PageDown, KeyModifiers::NONE);
        assert_eq!(
            input.cursor_chars(),
            "l0\nl1\nl2\nl3\n".len(),
            "l4, column 0"
        );
    }

    /// The box scrolls only when the caret would leave it — ↑/↓ inside it
    /// leave it still, rather than recentring on every press.
    #[test]
    fn the_view_moves_only_to_keep_the_caret_in_sight() {
        let mut input = drawn(&twenty_lines(), 20, 5);
        for _ in 0..4 {
            press(&mut input, KeyCode::Up, KeyModifiers::NONE);
            let view = input.view_for(20, 5);
            assert_eq!(view.top, 15, "caret on row {}", row_of(&input));
            input.set_view(view);
        }
        press(&mut input, KeyCode::Up, KeyModifiers::NONE);
        assert_eq!(input.view_for(20, 5).top, 14);
        // A box taller than the text shows it all, from the top.
        assert_eq!(input.view_for(20, 30).top, 0);
    }

    /// The wheel scrolls the box, pulling the caret along only when it
    /// would drop out of sight.
    #[test]
    fn the_wheel_scrolls_and_drags_the_caret_into_view() {
        let mut input = drawn(&twenty_lines(), 20, 5);
        assert_eq!(input.scroll_rows(-3), Edit::Moved);
        assert_eq!((input.view().top, row_of(&input)), (12, 16));
        assert_eq!(input.scroll_rows(1), Edit::Moved);
        assert_eq!(
            (input.view().top, row_of(&input)),
            (13, 16),
            "still in sight: the caret stays"
        );
        input.scroll_rows(-100);
        assert_eq!((input.view().top, row_of(&input)), (0, 4));
        assert_eq!(input.scroll_rows(-1), Edit::Ignored);
    }

    /// A click puts the caret where it points in the rows as drawn — past
    /// a row's end at that end, below the last row at the text's end.
    #[test]
    fn a_click_lands_the_caret_where_it_points() {
        let mut input = drawn(FOX, 10, 3);
        assert_eq!(input.view().top, 2, "rows 2-4 in sight");
        input.click(0, 2);
        assert_eq!(input.cursor_chars(), 22, "ju|mps");
        input.click(0, 9);
        assert_eq!(input.cursor_chars(), 25, "the end of `jumps `");
        input.click(9, 0);
        assert_eq!(input.cursor_chars(), 43);
        let mut top = drawn(FOX, 10, 5);
        top.click(1, 0);
        assert_eq!(top.cursor_chars(), 10);
        assert_eq!(row_of(&top), 1, "a soft break's caret starts the next row");
    }

    #[test]
    fn with_text_parks_the_cursor_at_the_end() {
        let mut input = TextInput::with_text("note");
        assert_eq!(input.cursor_chars(), 4);
        press(&mut input, KeyCode::Backspace, KeyModifiers::NONE);
        assert_eq!(input.as_str(), "not");
    }
}
