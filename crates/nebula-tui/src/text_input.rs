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
//! all break the line; ↑/↓ walk the lines and fall through to the caller
//! past the first or last, so a form can step to its next field; Home/End
//! and the readline chords work on the line under the caret; a paste keeps
//! its newlines. A one-line field flattens a paste and leaves the break
//! keys to the caller, so Enter — always the caller's — stays the only
//! way out of it.
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
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TextInput {
    text: String,
    /// Byte offset into `text`; always on a char boundary, always ≤ len.
    cursor: usize,
    /// Hard line breaks allowed: the break keys insert one and a paste
    /// keeps its own. Off, the field is one line whatever comes in.
    multiline: bool,
}

/// Word characters for ⌥-arrow / Ctrl+W motion: a run of these is one word,
/// everything else (spaces, `/`, `-`, `.`) separates. Matches what readline
/// does in a shell, which is where the muscle memory comes from.
fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
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
            multiline: false,
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

    /// Replace the whole value, cursor to the end.
    pub fn set_text(&mut self, text: impl Into<String>) {
        self.text = text.into();
        self.cursor = self.text.len();
    }

    pub fn clear(&mut self) {
        self.text.clear();
        self.cursor = 0;
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
            KeyCode::Home => self.move_to(self.line_start(self.cursor)),
            KeyCode::End => self.move_to(self.line_end(self.cursor)),
            // ↑/↓ walk a multi-row field's lines, keeping the column where
            // the line has it; past the first or last line they are the
            // caller's (a form steps to its next field), and a one-line
            // field never has a second line to walk to.
            KeyCode::Up => self.line_up(),
            KeyCode::Down => self.line_down(),

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

    /// The caret's column on its line, in characters — what ↑/↓ keep.
    fn column(&self) -> usize {
        self.text[self.line_start(self.cursor)..self.cursor]
            .chars()
            .count()
    }

    /// `col` characters into the line starting at `start`, or that line's
    /// end when it is shorter.
    fn at_column(&self, start: usize, col: usize) -> usize {
        let end = self.line_end(start);
        self.text[start..end]
            .char_indices()
            .nth(col)
            .map_or(end, |(i, _)| start + i)
    }

    /// ↑: the same column one line up, or [`Edit::Ignored`] on the first
    /// line so the caller can act on the key.
    fn line_up(&mut self) -> Edit {
        let start = self.line_start(self.cursor);
        if start == 0 {
            return Edit::Ignored;
        }
        let col = self.column();
        let above = self.line_start(start - 1);
        self.move_to(self.at_column(above, col))
    }

    /// ↓: the same column one line down, or [`Edit::Ignored`] on the last.
    fn line_down(&mut self) -> Edit {
        let end = self.line_end(self.cursor);
        if end == self.text.len() {
            return Edit::Ignored;
        }
        let col = self.column();
        self.move_to(self.at_column(end + 1, col))
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

    /// ↑/↓ walk the lines keeping the column (clamped to a shorter line),
    /// and past the first or last line they are Ignored, so a form can
    /// step to its next field on the very same key.
    #[test]
    fn arrows_walk_the_lines_and_fall_through_at_the_ends() {
        let mut input = TextInput::multiline_with_text("first line\nhi\nthird");
        // From the end of "third" (column 5): "hi" is shorter, so its end.
        assert_eq!(
            press(&mut input, KeyCode::Up, KeyModifiers::NONE),
            Edit::Moved
        );
        assert_eq!(input.cursor_chars(), "first line\nhi".len());
        // Column 2 now, onto the first line.
        assert_eq!(
            press(&mut input, KeyCode::Up, KeyModifiers::NONE),
            Edit::Moved
        );
        assert_eq!(input.cursor_chars(), 2);
        assert_eq!(
            press(&mut input, KeyCode::Up, KeyModifiers::NONE),
            Edit::Ignored,
            "no line above the first"
        );
        assert_eq!(input.cursor_chars(), 2, "the caret stays put");
        press(&mut input, KeyCode::Down, KeyModifiers::NONE);
        press(&mut input, KeyCode::Down, KeyModifiers::NONE);
        assert_eq!(input.cursor_chars(), "first line\nhi\nth".len());
        assert_eq!(
            press(&mut input, KeyCode::Down, KeyModifiers::NONE),
            Edit::Ignored,
            "no line below the last"
        );
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

    #[test]
    fn with_text_parks_the_cursor_at_the_end() {
        let mut input = TextInput::with_text("note");
        assert_eq!(input.cursor_chars(), 4);
        press(&mut input, KeyCode::Backspace, KeyModifiers::NONE);
        assert_eq!(input.as_str(), "not");
    }
}
