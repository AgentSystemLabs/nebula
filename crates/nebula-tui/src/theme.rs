//! Color theme: every color the UI draws, keyed by role rather than
//! hardcoded at the call site. The active theme is picked by name in the
//! settings overlay (`theme` in config.json) and lives on `App`, refreshed
//! by the event loop when the setting changes.
//!
//! Presets stick to ANSI-16 and 256-color indexed values so they render
//! everywhere. One exception: `focus_tint` needs a ~10%-opacity accent
//! shade that the 256 palette simply doesn't have (its darkest chromatic
//! steps start around 40%), so it's truecolor RGB — supported by modern
//! terminals including Terminal.app since macOS Tahoe.

use ratatui::style::Color;

/// Names the settings overlay cycles through; `by_name` accepts them
/// case-insensitively and falls back to the first entry.
pub const THEMES: &[&str] = &["default", "ocean", "forest", "rose", "amber"];

/// Semantic color roles for the whole TUI.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Theme {
    /// Focus borders, titles, cursors, match highlights.
    pub accent: Color,
    /// Text drawn on top of an `accent` background (focused title chip).
    pub on_accent: Color,
    /// Primary input text.
    pub text: Color,
    /// Secondary text: unfocused titles, inactive breadcrumb segments.
    /// Also what `dim` spans brighten to on an unfocused selection bar.
    pub muted: Color,
    /// Hints, dividers, badges, archived rows; also the unfocused
    /// selection-bar background.
    pub dim: Color,
    /// Added / connected / a review file ticked off — the plain "this went
    /// well" green. Also a finished turn you have already read: still a
    /// good outcome, just no longer a job.
    pub ok: Color,
    /// A turn that finished and nobody has read yet: the dot, and the
    /// `n done` counts pointing at it. Deliberately NOT `ok` — the whole
    /// point is that this one wants a human, and green is the color a
    /// terminal teaches you to skip over. Reading the session turns the
    /// dot green.
    pub done: Color,
    /// Running / modified / flash messages / remote host.
    pub warn: Color,
    /// Needs feedback / deleted / destructive actions.
    pub err: Color,
    /// Terminated sessions and the session kind badge.
    pub special: Color,
    /// Selected-row fill in the focused panel (a subtle raised surface,
    /// not a reverse-video slab).
    pub sel_bg: Color,
    /// Selected-row fill in unfocused panels (barely raised).
    pub sel_bg_dim: Color,
    /// Structural chrome: column rules, header underlines, dividers.
    /// Darker than `dim` so the frame recedes behind the content.
    pub edge: Color,
    /// Shades for the running-row text sweep, `[tail, mid, head]`: the
    /// whole name sits on the tail shade while a brighter two-cell band
    /// sweeps across it. Yellow family, paired with `warn`.
    pub warn_sweep: [Color; 3],
    /// Needs-feedback counterpart of `warn_sweep`. Red family, paired
    /// with `err`.
    pub err_sweep: [Color; 3],
    /// Focused-panel background: a dark neutral-gray floor with a faint
    /// lean toward the accent, filling the whole focused panel (and the
    /// rounded corners of a selected PILL ROW's pad rows) so it reads as
    /// a faintly lit gray surface rather than plain black. Truecolor by
    /// necessity (see module docs).
    pub focus_tint: Color,
}

impl Default for Theme {
    fn default() -> Self {
        // The classic nebula look: cyan accent on ANSI colors.
        Self {
            accent: Color::Cyan,
            on_accent: Color::Black,
            text: Color::White,
            muted: Color::Gray,
            dim: Color::DarkGray,
            ok: Color::Green,
            done: Color::Indexed(141), // violet — no other status is near it
            warn: Color::Yellow,
            err: Color::Red,
            special: Color::Magenta,
            sel_bg: Color::Indexed(237),
            sel_bg_dim: Color::Indexed(235),
            edge: Color::Indexed(238),
            warn_sweep: [Color::Yellow, Color::Indexed(220), Color::Indexed(230)],
            err_sweep: [Color::Red, Color::Indexed(203), Color::Indexed(217)],
            focus_tint: Color::Rgb(22, 33, 34),
        }
    }
}

impl Theme {
    pub fn by_name(name: &str) -> Self {
        let base = Self::default();
        match name.trim().to_ascii_lowercase().as_str() {
            "ocean" => Self {
                accent: Color::Indexed(39),  // deep sky blue
                special: Color::Indexed(75), // steel blue
                focus_tint: Color::Rgb(21, 31, 38),
                ..base
            },
            "forest" => Self {
                accent: Color::Indexed(114),  // pale green
                special: Color::Indexed(108), // sage
                focus_tint: Color::Rgb(26, 34, 27),
                ..base
            },
            "rose" => Self {
                accent: Color::Indexed(211),  // pink
                special: Color::Indexed(141), // violet
                // Violet is spoken for here, so done goes turquoise — the
                // one hue this preset leaves free.
                done: Color::Indexed(45),
                focus_tint: Color::Rgb(37, 28, 32),
                ..base
            },
            "amber" => Self {
                accent: Color::Indexed(214),  // orange
                special: Color::Indexed(173), // copper
                focus_tint: Color::Rgb(37, 32, 22),
                ..base
            },
            _ => base,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn by_name_covers_all_presets_and_falls_back() {
        for name in THEMES {
            // Every listed preset must parse (and not accidentally be a
            // misspelling that falls back to default without noticing).
            let theme = Theme::by_name(name);
            if *name != "default" {
                assert_ne!(theme, Theme::default(), "{name} fell back to default");
            }
        }
        assert_eq!(Theme::by_name("no-such-theme"), Theme::default());
        assert_eq!(Theme::by_name(" Ocean "), Theme::by_name("ocean"));

        // A done dot has to stay readable AS done: never green (that's
        // `ok`, which the eye files as "nothing to do here"), and never
        // the same color as another status in the same preset.
        for name in THEMES {
            let th = Theme::by_name(name);
            assert_ne!(th.done, th.ok, "{name}: done reads as plain success");
            assert_ne!(th.done, th.warn, "{name}: done reads as running");
            assert_ne!(th.done, th.err, "{name}: done reads as needs-feedback");
            assert_ne!(th.done, th.special, "{name}: done reads as terminated");
            assert_ne!(th.done, th.dim, "{name}: done reads as fresh");
        }
    }

    /// `focus_tint` paints over every untouched cell of the focused panel,
    /// including the rounded pad-row corners of a selected PILL ROW — a
    /// channel much below this floor reads as plain black there instead of
    /// a gray tint (issue #6).
    #[test]
    fn focus_tint_has_a_visible_gray_floor() {
        const MIN_CHANNEL: u8 = 18;
        for name in THEMES {
            let th = Theme::by_name(name);
            let Color::Rgb(r, g, b) = th.focus_tint else {
                panic!("{name}: focus_tint must be truecolor RGB");
            };
            assert!(
                r >= MIN_CHANNEL && g >= MIN_CHANNEL && b >= MIN_CHANNEL,
                "{name}: focus_tint {:?} is too close to black",
                (r, g, b)
            );
        }
    }
}
