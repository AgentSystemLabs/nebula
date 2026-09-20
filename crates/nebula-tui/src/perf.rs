//! The INPUT LATENCY PROBE: `NEBULA_PERF_LOG=<file>` makes the TUI write one
//! JSON line per input event, per painted frame and per daemon event, so
//! "does this key feel instant" is a number rather than an impression.
//!
//! * `input` — what arrived (`key:char`, `key:C-q`, `mouse:down` — never the
//!   character typed), how long its handler
//!   held the loop (`handler_us`), and where that left the app (overlay,
//!   FOCUS). A handler that shells out to git shows up here.
//! * `frame` — how long the draw took (`draw_us`), what it showed (overlay
//!   and whether it is still waiting on a background read — `busy` — the
//!   pane's session and whether it has a screen yet), and every input
//!   waiting on it with its arrival-to-paint time (`latency_us`): the figure
//!   the user feels.
//! * `server` — a daemon event's arrival, so an action that completes
//!   off the loop (an attach's replay, a create's Ack) can be timed from
//!   the key that asked for it to the frame that showed it.
//!
//! Off — the variable unset, as it always is outside a measurement run —
//! it is one `Option` check per event. `scripts/perf/` drives and reads it.

use crate::app::{App, Overlay};
use crossterm::event::{Event, KeyCode, KeyModifiers, MouseEventKind};
use std::io::Write;
use std::time::Instant;

pub struct Perf {
    out: std::io::BufWriter<std::fs::File>,
    /// Wall-clock µs `t: 0` stands for.
    epoch_us: u128,
    /// Inputs handled since the last frame: label and arrival.
    waiting: Vec<(String, Instant)>,
}

impl Perf {
    /// The probe, when `NEBULA_PERF_LOG` names a file that can be created.
    pub fn from_env() -> Option<Self> {
        let path = std::env::var_os("NEBULA_PERF_LOG")?;
        let file = std::fs::File::create(path).ok()?;
        let mut out = std::io::BufWriter::new(file);
        // The wall clock `t: 0` stands for, so a driver's own timeline (when
        // it pressed what) lines up with this one.
        let epoch_us = wall_us();
        let _ = writeln!(out, r#"{{"k":"start","epoch_us":{epoch_us}}}"#);
        Some(Self {
            out,
            epoch_us,
            waiting: Vec::new(),
        })
    }

    /// `at` on the run's timeline. Read off the wall clock, not `Instant`:
    /// a driver stamps its own steps with the wall clock, and on macOS the
    /// two drift apart by more than a millisecond every second.
    fn micros(&self, at: Instant) -> u128 {
        wall_us().saturating_sub(at.elapsed().as_micros() + self.epoch_us)
    }

    /// An input event whose handler ran from `arrived` until now.
    pub fn input(&mut self, label: String, arrived: Instant, app: &App) {
        let handler_us = arrived.elapsed().as_micros();
        let _ = writeln!(
            self.out,
            r#"{{"k":"input","t":{},"ev":{:?},"handler_us":{},"overlay":"{}","focus":"{:?}"}}"#,
            self.micros(arrived),
            label,
            handler_us,
            overlay_name(app),
            app.focus,
        );
        self.waiting.push((label, arrived));
    }

    /// A frame whose draw ran from `began` until now.
    pub fn frame(&mut self, began: Instant, app: &App) {
        let done = Instant::now();
        let inputs: Vec<String> = self
            .waiting
            .drain(..)
            .map(|(label, arrived)| {
                format!(
                    r#"{{"ev":{:?},"latency_us":{}}}"#,
                    label,
                    done.duration_since(arrived).as_micros()
                )
            })
            .collect();
        let (pane, painted, booting) = match &app.term {
            Some(t) => (format!("{:?}", t.sref), t.painted, t.booting),
            None => ("none".to_string(), false, false),
        };
        let _ = writeln!(
            self.out,
            r#"{{"k":"frame","t":{},"draw_us":{},"overlay":"{}","focus":"{:?}","pane":{:?},"painted":{},"booting":{},"busy":{},"inputs":[{}]}}"#,
            self.micros(began),
            done.duration_since(began).as_micros(),
            overlay_name(app),
            app.focus,
            pane,
            painted,
            booting,
            overlay_busy(app),
            inputs.join(","),
        );
        // A run is read after the fact, often from a TUI that was killed
        // rather than quit: every frame is on disk once it is drawn.
        let _ = self.out.flush();
    }

    /// A daemon event, by name, as it arrives.
    pub fn server(&mut self, name: &str) {
        let _ = writeln!(
            self.out,
            r#"{{"k":"server","t":{},"ev":"{}"}}"#,
            self.micros(Instant::now()),
            name
        );
    }
}

fn wall_us() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_micros())
}

/// Is the overlay on screen still waiting on a BACKGROUND READ to show what
/// was asked for? A frame that is, has not settled.
fn overlay_busy(app: &App) -> bool {
    match &app.overlay {
        Some(Overlay::Diff(v)) => {
            (v.listing.is_some() && v.files.is_empty())
                || (v.waiting.is_some()
                    && v.shown.as_deref() != v.selected_file().map(|f| f.path.as_str()))
        }
        Some(Overlay::Files(v)) => v.listing.is_some(),
        Some(Overlay::Tree(v)) => v.listing.is_some() || v.waiting.is_some(),
        Some(Overlay::Grep(v)) => v.waiting.is_some(),
        _ => false,
    }
}

fn overlay_name(app: &App) -> &'static str {
    if app.vim.is_some() {
        return "Editor";
    }
    match &app.overlay {
        None => "none",
        Some(Overlay::Menu(_)) => "Menu",
        Some(Overlay::Confirm(_)) => "Confirm",
        Some(Overlay::Prompt(_)) => "Prompt",
        Some(Overlay::Help(_)) => "Help",
        Some(Overlay::Settings(_)) => "Settings",
        Some(Overlay::Diff(_)) => "Diff",
        Some(Overlay::Palette(_)) => "Palette",
        Some(Overlay::Files(_)) => "Files",
        Some(Overlay::Grep(_)) => "Grep",
        Some(Overlay::Tree(_)) => "Tree",
        Some(Overlay::FileTabs(_)) => "FileTabs",
        Some(Overlay::Metrics(_)) => "Metrics",
        Some(Overlay::Hosts(_)) => "Hosts",
        Some(Overlay::AgentPresets(_)) => "AgentPresets",
        Some(Overlay::AgentPresetEditor(_)) => "AgentPresetEditor",
        Some(Overlay::Issues(_)) => "Issues",
        Some(Overlay::PullRequests(_)) => "PullRequests",
        Some(Overlay::BranchSwitch(_)) => "BranchSwitch",
        Some(Overlay::ProjectPicker(_)) => "ProjectPicker",
    }
}

/// The modifiers that make a character key a command rather than text.
const CHORD: KeyModifiers = KeyModifiers::CONTROL
    .union(KeyModifiers::ALT)
    .union(KeyModifiers::SUPER);

/// `key:char`, `key:C-d`, `key:S-Tab`, `key:Enter`, `mouse:down`, `paste`, …
/// — None for the events nobody waits on (pointer motion, focus reports,
/// releases). What was typed is never in it: see the `Char` arm.
pub fn label(event: &Event) -> Option<String> {
    match event {
        Event::Key(key) if key.kind != crossterm::event::KeyEventKind::Release => {
            let mut s = String::from("key:");
            for (bit, tag) in [
                (KeyModifiers::CONTROL, "C-"),
                (KeyModifiers::ALT, "M-"),
                (KeyModifiers::SUPER, "D-"),
            ] {
                if key.modifiers.contains(bit) {
                    s.push_str(tag);
                }
            }
            match key.code {
                // Never the character itself. A plain key is as likely typed
                // at an agent, a shell or a password prompt as at a panel,
                // and a log is not where that belongs — the rule the KEY
                // COMBO DISPLAY keeps. A chord is a command, and is named.
                KeyCode::Char(_) if !key.modifiers.intersects(CHORD) => s.push_str("char"),
                KeyCode::Char(c) => s.push(c),
                KeyCode::BackTab => s.push_str("S-Tab"),
                other => s.push_str(&format!("{other:?}")),
            }
            Some(s)
        }
        Event::Mouse(mouse) => match mouse.kind {
            MouseEventKind::Down(_) => Some("mouse:down".into()),
            MouseEventKind::Up(_) => Some("mouse:up".into()),
            MouseEventKind::ScrollDown | MouseEventKind::ScrollUp => Some("mouse:wheel".into()),
            _ => None,
        },
        Event::Paste(_) => Some("paste".into()),
        _ => None,
    }
}

/// A daemon event's variant name, for [`Perf::server`].
pub fn server_name(ev: &nebula_core::protocol::ServerEvent) -> &'static str {
    use nebula_core::protocol::ServerEvent as E;
    match ev {
        E::Scrollback { .. } => "Scrollback",
        E::Output { .. } => "Output",
        E::Ack { .. } => "Ack",
        E::Error { .. } => "Error",
        E::EntityUpserted { .. } => "EntityUpserted",
        E::EntityRemoved { .. } => "EntityRemoved",
        E::StatusChanged { .. } => "StatusChanged",
        E::Snapshot { .. } => "Snapshot",
        _ => "other",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyEvent;

    fn key(code: KeyCode, mods: KeyModifiers) -> Event {
        Event::Key(KeyEvent::new(code, mods))
    }

    /// The probe times keys; it does not record them. A run left on by
    /// accident must not turn into a keylog of what was typed at an agent.
    #[test]
    fn a_typed_character_is_never_in_the_log() {
        for c in ['a', 'Z', '7', '!', ' '] {
            for mods in [KeyModifiers::NONE, KeyModifiers::SHIFT] {
                assert_eq!(
                    label(&key(KeyCode::Char(c), mods)).as_deref(),
                    Some("key:char"),
                    "{c:?} with {mods:?}"
                );
            }
        }
    }

    /// Chords and named keys are commands, and say which.
    #[test]
    fn chords_and_named_keys_are_named() {
        assert_eq!(
            label(&key(KeyCode::Char('q'), KeyModifiers::CONTROL)).as_deref(),
            Some("key:C-q")
        );
        assert_eq!(
            label(&key(KeyCode::Enter, KeyModifiers::NONE)).as_deref(),
            Some("key:Enter")
        );
        assert_eq!(
            label(&key(KeyCode::BackTab, KeyModifiers::SHIFT)).as_deref(),
            Some("key:S-Tab")
        );
    }
}
