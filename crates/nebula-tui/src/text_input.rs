//! One-line text field with the editing keys a terminal user expects.
//!
//! Every typed field in the TUI — the prompt dialog, the fuzzy filters,
//! the grep query, the ssh destination — is one of these, so
//! the keys are learned once and work everywhere: arrows and Home/End,
//! word motion on ⌥←/⌥→, the readline control chords (Ctrl+A/E/B/F/W/U/K),
//! and word/line deletes.
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

/// Editable single-line text plus a cursor into it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TextInput {
    text: String,
    /// Byte offset into `text`; always on a char boundary, always ≤ len.
    cursor: usize,
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
        Self { text, cursor }
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

    /// Insert a whole run at the cursor — a bracketed paste, minus the
    /// newlines a one-line field can't hold.
    pub fn insert_str(&mut self, s: &str) {
        let flat: String = s
            .chars()
            .map(|c| if c == '\n' || c == '\r' { ' ' } else { c })
            .collect();
        self.text.insert_str(self.cursor, &flat);
        self.cursor += flat.len();
    }

    /// Insert a bracketed paste while preserving line breaks. This is kept
    /// opt-in so every established one-line field retains its flattening
    /// contract; the Claude Cloud task editor is the sole caller.
    pub fn insert_multiline_str(&mut self, s: &str) {
        let normalized = s.replace("\r\n", "\n").replace('\r', "\n");
        self.text.insert_str(self.cursor, &normalized);
        self.cursor += normalized.len();
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
            // ---- motion ----
            KeyCode::Left if cmd => self.move_to(0),
            KeyCode::Left if alt || ctrl => self.move_to(self.word_left(self.cursor)),
            KeyCode::Left => self.move_to(self.prev_boundary(self.cursor)),
            KeyCode::Right if cmd => self.move_to(self.text.len()),
            KeyCode::Right if alt || ctrl => self.move_to(self.word_right(self.cursor)),
            KeyCode::Right => self.move_to(self.next_boundary(self.cursor)),
            KeyCode::Home => self.move_to(0),
            KeyCode::End => self.move_to(self.text.len()),

            // ---- deletion ----
            // Cmd+⌫ kills the line, ⌥⌫ / Ctrl+⌫ the previous word.
            KeyCode::Backspace if cmd => self.delete(0, self.cursor),
            KeyCode::Backspace if alt || ctrl => {
                self.delete(self.word_left(self.cursor), self.cursor)
            }
            KeyCode::Backspace => self.delete(self.prev_boundary(self.cursor), self.cursor),
            KeyCode::Delete if cmd => self.delete(self.cursor, self.text.len()),
            KeyCode::Delete if alt || ctrl => {
                self.delete(self.cursor, self.word_right(self.cursor))
            }
            KeyCode::Delete => self.delete(self.cursor, self.next_boundary(self.cursor)),

            // ---- readline chords ----
            // Cmd+key never means "type this" — leave it to the caller.
            KeyCode::Char(_) if cmd => Edit::Ignored,
            KeyCode::Char(c) if ctrl => match c.to_ascii_lowercase() {
                'a' => self.move_to(0),
                'e' => self.move_to(self.text.len()),
                'b' => self.move_to(self.prev_boundary(self.cursor)),
                'f' => self.move_to(self.next_boundary(self.cursor)),
                'd' => self.delete(self.cursor, self.next_boundary(self.cursor)),
                'w' => self.delete(self.word_left(self.cursor), self.cursor),
                'u' => self.delete(0, self.cursor),
                'k' => self.delete(self.cursor, self.text.len()),
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

    #[test]
    fn paste_inserts_at_the_cursor_and_flattens_newlines() {
        let mut input = typed("ab");
        press(&mut input, KeyCode::Left, KeyModifiers::NONE);
        input.insert_str("one\ntwo");
        assert_eq!(input.as_str(), "aone twob");
        assert_eq!(input.cursor_chars(), 8);
    }

    #[test]
    fn multiline_paste_preserves_and_normalizes_line_breaks() {
        let mut input = typed("ab");
        press(&mut input, KeyCode::Left, KeyModifiers::NONE);
        input.insert_multiline_str("one\r\ntwo\rthree");
        assert_eq!(input.as_str(), "aone\ntwo\nthreeb");
        assert_eq!(input.cursor_chars(), 14);
    }

    #[test]
    fn with_text_parks_the_cursor_at_the_end() {
        let mut input = TextInput::with_text("note");
        assert_eq!(input.cursor_chars(), 4);
        press(&mut input, KeyCode::Backspace, KeyModifiers::NONE);
        assert_eq!(input.as_str(), "not");
    }
}
