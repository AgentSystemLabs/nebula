//! The KEY COMBO DISPLAY (Settings → Experimental, `show_key_combos`):
//! each key pressed in the panels, spelled at the bottom left of the
//! screen with what it did — `j - Move down`, `^d - Half page down`,
//! `l l - Enter pane` — so someone watching over a shoulder or a screen
//! share can pick the shortcuts up as they are used. Modelled on vim's
//! `showcmd`: the keys show as they land and clear on their own a moment
//! later ([`LINGER`]), with nothing to dismiss.
//!
//! The event loop is the one caller of [`note`]: it records the chord at
//! the point it is dispatched, once it knows what the press meant — the
//! [`crate::keymap::ActionSpec::label`] of the action it fired, the
//! second tap of a double tap as one two-key combo, or nothing at all for
//! an unbound key, which still shows so a watcher sees it did nothing.
//!
//! What it never shows: keys typed into a LOCKED PANE (they are the
//! agent's, and a password typed at a prompt in there must not land on
//! the screen — only the unlock hatch shows), and anything typed into an
//! overlay's text field. Inside a modal only the keys that cannot be
//! text show, bare: Esc, Enter, Tab, the arrows, a ^chord
//! ([`is_text_key`] draws the line).

use crate::app::App;
use crate::keymap::KeyChord;
use crossterm::event::{KeyCode, KeyModifiers};
use std::time::{Duration, Instant};

/// How long the last combo stays on screen after its press. Long enough
/// to read a label across a screen share; short enough that a stale key
/// isn't still up when the next thing happens.
pub const LINGER: Duration = Duration::from_secs(3);

/// What the display is showing: the chord(s) of the last press and what
/// they did, stamped so the loop can clear it after [`LINGER`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyCombo {
    /// The press, or both presses of a double tap, in order.
    pub keys: Vec<KeyChord>,
    /// What it did, when it did something: the action's label.
    pub does: Option<String>,
    pub at: Instant,
}

impl KeyCombo {
    /// The keys as shown, space-separated: `j`, `^d`, `h h`.
    pub fn keys_text(&self) -> String {
        self.keys
            .iter()
            .map(|c| c.display())
            .collect::<Vec<_>>()
            .join(" ")
    }

    /// The whole line: `j - Move down`, or a bare `x` for a key that fired
    /// nothing.
    pub fn text(&self) -> String {
        match &self.does {
            Some(does) => format!("{} - {does}", self.keys_text()),
            None => self.keys_text(),
        }
    }

    /// When the loop takes it back down.
    pub fn deadline(&self) -> Instant {
        self.at + LINGER
    }

    pub fn expired(&self, now: Instant) -> bool {
        now >= self.deadline()
    }
}

/// Record a press for the display. A no-op while the setting is off, so
/// callers need not check; the newest press always replaces the last.
pub fn note(app: &mut App, keys: &[KeyChord], does: Option<&str>) {
    if !app.show_key_combos || keys.is_empty() {
        return;
    }
    app.key_combo = Some(KeyCombo {
        keys: keys.to_vec(),
        does: does.map(str::to_string),
        at: Instant::now(),
    });
    app.dirty = true;
}

/// The second press of a double tap, shown as one combo: `h h` and what
/// the pair did, worded from the footer's `h again: …` hint.
pub fn note_double_tap(app: &mut App, chord: &KeyChord, does: &str) {
    let mut label = String::with_capacity(does.len());
    let mut chars = does.chars();
    if let Some(first) = chars.next() {
        label.extend(first.to_uppercase());
        label.push_str(chars.as_str());
    }
    note(app, &[*chord, *chord], Some(&label));
}

/// A chord an overlay would take as typed text rather than as a command —
/// a printable character, shifted or not, or an edit to one (Backspace,
/// Delete) — so the display leaves it alone: a filter, a session name or
/// an ssh destination being typed is not a shortcut, and echoing it
/// would put the text on screen twice. A ^chord or ⌥chord on a letter
/// is a command everywhere and shows.
pub fn is_text_key(chord: &KeyChord) -> bool {
    let command_mods = KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SUPER;
    if chord.mods.intersects(command_mods) {
        return false;
    }
    matches!(
        chord.code,
        KeyCode::Char(_) | KeyCode::Backspace | KeyCode::Delete
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyEvent;

    fn chord(code: KeyCode, mods: KeyModifiers) -> KeyChord {
        KeyChord::from_event(&KeyEvent::new(code, mods))
    }

    #[test]
    fn a_bound_key_reads_as_shortcut_dash_what_it_does() {
        let combo = KeyCombo {
            keys: vec![chord(KeyCode::Char('j'), KeyModifiers::NONE)],
            does: Some("Move down".into()),
            at: Instant::now(),
        };
        assert_eq!(combo.text(), "j - Move down");
        let combo = KeyCombo {
            keys: vec![chord(KeyCode::Char('d'), KeyModifiers::CONTROL)],
            does: Some("Half page down".into()),
            at: Instant::now(),
        };
        assert_eq!(
            combo.text(),
            "^d - Half page down",
            "chords use the footer's spelling"
        );
    }

    #[test]
    fn an_unbound_key_shows_bare_and_a_double_tap_shows_both_presses() {
        let combo = KeyCombo {
            keys: vec![chord(KeyCode::Char('x'), KeyModifiers::NONE)],
            does: None,
            at: Instant::now(),
        };
        assert_eq!(combo.text(), "x", "no dash with nothing after it");
        let l = chord(KeyCode::Char('l'), KeyModifiers::NONE);
        let combo = KeyCombo {
            keys: vec![l, l],
            does: Some("Enter pane".into()),
            at: Instant::now(),
        };
        assert_eq!(combo.text(), "l l - Enter pane");
    }

    #[test]
    fn note_is_a_no_op_while_the_setting_is_off_and_replaces_the_last_press_while_on() {
        let mut app = App::new();
        let j = chord(KeyCode::Char('j'), KeyModifiers::NONE);
        note(&mut app, &[j], Some("Move down"));
        assert!(app.key_combo.is_none(), "off by default: nothing recorded");

        app.show_key_combos = true;
        app.dirty = false;
        note(&mut app, &[j], Some("Move down"));
        assert_eq!(app.key_combo.as_ref().unwrap().text(), "j - Move down");
        assert!(app.dirty, "a fresh combo wants a frame");

        let k = chord(KeyCode::Char('k'), KeyModifiers::NONE);
        note(&mut app, &[k], None);
        assert_eq!(
            app.key_combo.as_ref().unwrap().text(),
            "k",
            "newest press wins"
        );
        note(&mut app, &[], Some("nothing"));
        assert_eq!(
            app.key_combo.as_ref().unwrap().text(),
            "k",
            "an empty press is not a combo"
        );
    }

    #[test]
    fn a_double_tap_is_one_combo_worded_from_the_footer_hint() {
        let mut app = App::new();
        app.show_key_combos = true;
        let l = chord(KeyCode::Right, KeyModifiers::NONE);
        note_double_tap(&mut app, &l, "enter pane");
        assert_eq!(app.key_combo.as_ref().unwrap().text(), "→ → - Enter pane");
        note_double_tap(&mut app, &l, "");
        assert_eq!(
            app.key_combo.as_ref().unwrap().does.as_deref(),
            Some(""),
            "an empty hint capitalizes to nothing rather than panicking"
        );
    }

    #[test]
    fn the_combo_expires_after_linger() {
        let combo = KeyCombo {
            keys: vec![chord(KeyCode::Enter, KeyModifiers::NONE)],
            does: None,
            at: Instant::now() - LINGER + Duration::from_millis(500),
        };
        assert!(!combo.expired(Instant::now()), "half a second still to go");
        let combo = KeyCombo {
            at: Instant::now() - LINGER,
            ..combo
        };
        assert!(combo.expired(Instant::now()));
        assert_eq!(combo.deadline(), combo.at + LINGER);
    }

    /// Inside a modal, what is typed is text and what is not is a key:
    /// letters, digits, punctuation and their edits stay off the display;
    /// Esc, Enter, Tab, arrows and any ^/⌥/⌘ chord show.
    #[test]
    fn text_keys_are_printable_characters_and_their_edits() {
        let text = [
            chord(KeyCode::Char('a'), KeyModifiers::NONE),
            chord(KeyCode::Char('A'), KeyModifiers::SHIFT),
            chord(KeyCode::Char('7'), KeyModifiers::NONE),
            chord(KeyCode::Char('/'), KeyModifiers::NONE),
            chord(KeyCode::Char(' '), KeyModifiers::NONE),
            chord(KeyCode::Backspace, KeyModifiers::NONE),
            chord(KeyCode::Delete, KeyModifiers::NONE),
        ];
        for c in text {
            assert!(is_text_key(&c), "{} is text", c.display());
        }
        let keys = [
            chord(KeyCode::Esc, KeyModifiers::NONE),
            chord(KeyCode::Enter, KeyModifiers::NONE),
            chord(KeyCode::Tab, KeyModifiers::NONE),
            chord(KeyCode::BackTab, KeyModifiers::NONE),
            chord(KeyCode::Down, KeyModifiers::NONE),
            chord(KeyCode::PageDown, KeyModifiers::NONE),
            chord(KeyCode::Char('u'), KeyModifiers::CONTROL),
            chord(KeyCode::Char('p'), KeyModifiers::ALT),
            chord(KeyCode::Backspace, KeyModifiers::ALT),
        ];
        for c in keys {
            assert!(!is_text_key(&c), "{} is a key", c.display());
        }
    }
}
