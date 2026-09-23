//! The RELEASE WATCH: one unarchive per press of the key, however long
//! the key is held.
//!
//! A terminal repeats a held key — after its initial delay, then every
//! few dozen milliseconds until the key comes up — and in the legacy key
//! encoding every repeat is another press of the same key, with nothing
//! to tell the two apart. `u` unarchives the card under the cursor and
//! the next card slides under it, so a held `u` walked the ARCHIVED VIEW,
//! a card per repeat — as a held `a` once walked the live grid backwards
//! ("it seemed to have run so fast it archived two"). `a` asks first now,
//! and its repeats land on the confirm, where they are nothing; `u` has
//! no dialog, so the watch is its.
//!
//! The kitty keyboard protocol marks repeats and releases, but only on
//! keys it reports as escape codes — a text key like `u` stays plain text
//! under the flags nebula rests on (`host_terminal::KITTY_FLAGS`), and
//! reporting every key as an escape code all the time would cost composed
//! text (a dead key's `é`, a non-Latin layout). So the watch asks for it
//! only while it matters: the press that unarchived flips the host to
//! `host_terminal::HOLD_FLAGS`; the held key's repeats then arrive marked
//! `Repeat` and are swallowed here; its `Release` — or any fresh press —
//! ends the watch and flips the host back. A fresh press of the same key
//! is a new unarchive: the key had to come up first.
//!
//! A host without the protocol (Terminal.app, `nebula browser`, tmux)
//! marks nothing, so there a held key's first repeat arrives as a press,
//! ends the watch and unarchives as it always did: the watch is only as
//! good as the host's reports.

use crate::keymap::KeyChord;
use crossterm::event::{KeyCode, KeyEvent, KeyEventKind};
use std::time::{Duration, Instant};

/// A watch no key has touched for this long is let go on the loop's mode
/// beat ([`expire`]): a host that took the flags but reports no events
/// (tmux) would otherwise keep them until the next key.
pub(super) const LINGER: Duration = Duration::from_secs(2);

/// The key that just unarchived a card, watched until the host reports
/// it let go.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReleaseWatch {
    /// The chord as the press arrived. Its repeats and release arrive as
    /// the same chord whichever encoding the host uses for them:
    /// `KeyChord::from_event` folds a shifted key's two spellings.
    chord: KeyChord,
    /// The last press or repeat of it — the watch's age for [`LINGER`].
    since: Instant,
}

/// Start watching `chord`, the key that just unarchived, at `now`.
pub(super) fn arm(watch: &mut Option<ReleaseWatch>, chord: KeyChord, now: Instant) {
    *watch = Some(ReleaseWatch { chord, since: now });
}

/// The watch's reading of a key event, taken before anything else handles
/// it — true when the event is the held key repeating, and swallowed.
///
/// A repeat of another key is left to its handler (holding `j` still
/// walks). A release of another key is nothing: rolled keys let `j` go
/// after `u` went down, and that must not free the `u` still held. A
/// press of any key, or the held key's release, ends the watch. A modifier
/// key on its own (Shift going down — reported only under `HOLD_FLAGS`)
/// is swallowed whether or not a watch is on: nothing is bound to one,
/// and it is not the press that ends a watch while `u` is still held.
pub(super) fn take(watch: &mut Option<ReleaseWatch>, key: &KeyEvent, now: Instant) -> bool {
    if matches!(key.code, KeyCode::Modifier(_)) {
        return true;
    }
    let Some(held) = watch.as_mut() else {
        return false;
    };
    let same = KeyChord::from_event(key) == held.chord;
    match key.kind {
        KeyEventKind::Repeat if same => {
            held.since = now;
            true
        }
        KeyEventKind::Repeat => false,
        KeyEventKind::Release => {
            if same {
                *watch = None;
            }
            false
        }
        KeyEventKind::Press => {
            *watch = None;
            false
        }
    }
}

/// Let a watch older than [`LINGER`] go; true when one was.
pub(super) fn expire(watch: &mut Option<ReleaseWatch>, now: Instant) -> bool {
    let stale = watch
        .as_ref()
        .is_some_and(|w| now.duration_since(w.since) >= LINGER);
    if stale {
        *watch = None;
    }
    stale
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyModifiers;

    fn ev(code: KeyCode, mods: KeyModifiers, kind: KeyEventKind) -> KeyEvent {
        KeyEvent::new_with_kind(code, mods, kind)
    }

    fn u(kind: KeyEventKind) -> KeyEvent {
        ev(KeyCode::Char('u'), KeyModifiers::NONE, kind)
    }

    fn armed(now: Instant) -> Option<ReleaseWatch> {
        let mut watch = None;
        arm(
            &mut watch,
            KeyChord::from_event(&u(KeyEventKind::Press)),
            now,
        );
        watch
    }

    /// The held key's repeats are swallowed and keep the watch fresh; its
    /// release ends it; a fresh press of it is not swallowed — it is the
    /// next archive — and ends it too, for the archive to arm again.
    #[test]
    fn the_held_keys_repeats_are_swallowed_until_it_comes_up() {
        let t0 = Instant::now();
        let mut watch = armed(t0);
        let later = t0 + Duration::from_millis(400);
        assert!(take(&mut watch, &u(KeyEventKind::Repeat), later));
        assert!(take(&mut watch, &u(KeyEventKind::Repeat), later));
        assert_eq!(
            watch.as_ref().map(|w| w.since),
            Some(later),
            "a repeat is the key still held: the watch's age restarts"
        );
        assert!(!take(&mut watch, &u(KeyEventKind::Release), later));
        assert!(watch.is_none(), "the release lets the watch go");

        let mut watch = armed(t0);
        assert!(
            !take(&mut watch, &u(KeyEventKind::Press), later),
            "a fresh press is the next unarchive, never swallowed"
        );
        assert!(watch.is_none(), "and it ends the old watch");
    }

    /// The repeats and release under `HOLD_FLAGS` arrive as escape codes
    /// — a shifted key as its shifted character with the modifier — and
    /// still read as the chord the plain-text press armed.
    #[test]
    fn a_shifted_keys_escape_code_form_is_the_same_chord() {
        let t0 = Instant::now();
        let mut watch = None;
        // The press, as legacy text: `X`.
        arm(
            &mut watch,
            KeyChord::from_event(&ev(
                KeyCode::Char('X'),
                KeyModifiers::NONE,
                KeyEventKind::Press,
            )),
            t0,
        );
        // Its repeat, as `CSI 120:88;2:2u` parses: `X` with SHIFT.
        assert!(take(
            &mut watch,
            &ev(
                KeyCode::Char('X'),
                KeyModifiers::SHIFT,
                KeyEventKind::Repeat
            ),
            t0,
        ));
    }

    /// Other keys: their repeats go to their handlers and leave the watch
    /// on (`j` held beside `u`), their releases are nothing (rolled keys),
    /// and any press ends it.
    #[test]
    fn other_keys_walk_through_and_only_a_press_ends_the_watch() {
        let t0 = Instant::now();
        let j = |kind| ev(KeyCode::Char('j'), KeyModifiers::NONE, kind);
        let mut watch = armed(t0);
        assert!(!take(&mut watch, &j(KeyEventKind::Repeat), t0));
        assert!(watch.is_some(), "another key's repeat is its own business");
        assert!(!take(&mut watch, &j(KeyEventKind::Release), t0));
        assert!(
            watch.is_some(),
            "another key let go does not free the held one"
        );
        assert!(
            take(&mut watch, &u(KeyEventKind::Repeat), t0),
            "so the held key's next repeat is still swallowed"
        );
        assert!(!take(&mut watch, &j(KeyEventKind::Press), t0));
        assert!(watch.is_none(), "a press of anything ends the watch");
        assert!(
            !take(&mut watch, &u(KeyEventKind::Repeat), t0),
            "with no watch nothing is swallowed"
        );
    }

    /// A modifier key on its own is swallowed and changes nothing: Shift
    /// going down while `u` is held is not a press that ends the watch.
    #[test]
    fn a_lone_modifier_is_swallowed_and_keeps_the_watch() {
        use crossterm::event::ModifierKeyCode;
        let t0 = Instant::now();
        let shift = ev(
            KeyCode::Modifier(ModifierKeyCode::LeftShift),
            KeyModifiers::SHIFT,
            KeyEventKind::Press,
        );
        let mut watch = armed(t0);
        assert!(take(&mut watch, &shift, t0));
        assert!(watch.is_some());
        let mut none = None;
        assert!(take(&mut none, &shift, t0), "with no watch on, too");
    }

    /// The mode beat lets a watch go once nothing has touched it for
    /// `LINGER`: a repeat inside that keeps it.
    #[test]
    fn a_watch_nothing_touched_for_linger_expires() {
        let t0 = Instant::now();
        let mut watch = armed(t0);
        assert!(!expire(&mut watch, t0 + LINGER / 2));
        assert!(watch.is_some());
        assert!(take(&mut watch, &u(KeyEventKind::Repeat), t0 + LINGER / 2));
        assert!(
            !expire(&mut watch, t0 + LINGER),
            "the repeat restarted the clock"
        );
        assert!(expire(&mut watch, t0 + LINGER / 2 + LINGER));
        assert!(watch.is_none());
        assert!(
            !expire(&mut watch, t0 + LINGER * 3),
            "nothing left to expire"
        );
    }
}
