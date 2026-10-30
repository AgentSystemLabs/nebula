//! Color theme: every color the UI draws, keyed by role rather than
//! hardcoded at the call site. The active theme is picked by name in the
//! settings overlay (`theme` in config.json) and lives on `App`, refreshed
//! by the event loop when the setting changes.
//!
//! Presets stick to ANSI-16 and 256-color indexed values so they render
//! everywhere. One exception: `focus_tint` needs a near-black shade of the
//! accent that the 256 palette simply doesn't have (its darkest chromatic
//! steps start around 40%), so it's truecolor RGB — supported by modern
//! terminals including Terminal.app since macOS Tahoe.

use ratatui::style::Color;
use std::sync::atomic::{AtomicU8, Ordering};

/// What the BLACK BACKGROUND setting paints under every cell nothing else
/// colored. Truecolor rather than ANSI `Black`, which a terminal palette is
/// free to map to a dark gray (a stock Ghostty's is #1d1f21) — the very
/// gray the setting exists to get away from.
pub const BLACK_BACKGROUND: Color = Color::Rgb(0, 0, 0);

/// The light APPEARANCE's counterpart of [`BLACK_BACKGROUND`].
pub const WHITE_BACKGROUND: Color = Color::Rgb(255, 255, 255);

/// The APPEARANCE values the settings overlay cycles through: `auto`
/// follows the terminal's own background (asked with OSC 11 at startup,
/// `COLORFGBG` when it doesn't answer), the other two force one.
pub const APPEARANCES: &[&str] = &["auto", "dark", "light"];

/// What the startup probe learned about the host terminal's background:
/// 0 = nothing (no answer yet, or no probe), 1 = dark, 2 = light.
static DETECTED_BACKGROUND: AtomicU8 = AtomicU8::new(0);

/// Record the startup probe's answer: whether the terminal's background
/// is light.
pub fn set_detected_light(light: bool) {
    DETECTED_BACKGROUND.store(if light { 2 } else { 1 }, Ordering::Relaxed);
}

/// Whether the terminal's background is light, as far as anything told
/// us: the startup probe first, then `COLORFGBG` (`"15;0"` — fg;bg, an
/// ANSI index — exported by rxvt, Konsole and iTerm2), else dark.
fn detected_light() -> bool {
    match DETECTED_BACKGROUND.load(Ordering::Relaxed) {
        1 => false,
        2 => true,
        _ => std::env::var("COLORFGBG")
            .ok()
            .and_then(|v| colorfgbg_is_light(&v))
            .unwrap_or(false),
    }
}

/// `COLORFGBG`'s background is its last field: 7 (light gray) and 9–15
/// (the bright colors but 8, the dark gray) are light backgrounds.
fn colorfgbg_is_light(v: &str) -> Option<bool> {
    let bg: u8 = v.rsplit(';').next()?.trim().parse().ok()?;
    Some(bg == 7 || (9..=15).contains(&bg))
}

/// Whether `appearance` (a config value off [`APPEARANCES`]) resolves to
/// the light palette. Anything off the list is `auto`.
pub fn appearance_is_light(appearance: &str) -> bool {
    match appearance.trim().to_ascii_lowercase().as_str() {
        "light" => true,
        "dark" => false,
        _ => detected_light(),
    }
}

/// Parse an OSC 11 reply (`ESC ] 11 ; rgb:RRRR/GGGG/BBBB` then BEL or ST,
/// one to four hex digits a channel) into 8-bit RGB.
pub fn parse_osc11(reply: &[u8]) -> Option<(u8, u8, u8)> {
    let s = std::str::from_utf8(reply).ok()?;
    let start = s.find("]11;")? + 4;
    let body = &s[start..];
    let body = body
        .strip_prefix("rgba:")
        .or_else(|| body.strip_prefix("rgb:"))?;
    let end = body
        .find(|c: char| !(c.is_ascii_hexdigit() || c == '/'))
        .unwrap_or(body.len());
    let mut parts = body[..end].split('/');
    let mut channel = || -> Option<u8> {
        let hex = parts.next()?;
        if hex.is_empty() || hex.len() > 4 {
            return None;
        }
        let v = u32::from_str_radix(hex, 16).ok()?;
        let max = (1u32 << (4 * hex.len())) - 1;
        Some(((v * 255 + max / 2) / max) as u8)
    };
    Some((channel()?, channel()?, channel()?))
}

/// Whether a background color reads as light: WCAG relative luminance
/// over the midpoint between black and white text contrast (~0.18).
pub fn rgb_is_light((r, g, b): (u8, u8, u8)) -> bool {
    let lin = |c: u8| {
        let c = f64::from(c) / 255.0;
        if c <= 0.04045 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * lin(r) + 0.7152 * lin(g) + 0.0722 * lin(b) > 0.18
}

/// The RGB a terminal most likely shows for `c`: xterm's defaults for the
/// sixteen named colors, the 6×6×6 cube and the gray ramp for the rest of
/// the 256. None for a color with no fixed value (`Reset`).
pub fn color_rgb(c: Color) -> Option<(u8, u8, u8)> {
    const ANSI: [(u8, u8, u8); 16] = [
        (0, 0, 0),
        (205, 0, 0),
        (0, 205, 0),
        (205, 205, 0),
        (0, 0, 238),
        (205, 0, 205),
        (0, 205, 205),
        (229, 229, 229),
        (127, 127, 127),
        (255, 0, 0),
        (0, 255, 0),
        (255, 255, 0),
        (92, 92, 255),
        (255, 0, 255),
        (0, 255, 255),
        (255, 255, 255),
    ];
    let index = match c {
        Color::Rgb(r, g, b) => return Some((r, g, b)),
        Color::Indexed(i) => i,
        Color::Black => 0,
        Color::Red => 1,
        Color::Green => 2,
        Color::Yellow => 3,
        Color::Blue => 4,
        Color::Magenta => 5,
        Color::Cyan => 6,
        Color::Gray => 7,
        Color::DarkGray => 8,
        Color::LightRed => 9,
        Color::LightGreen => 10,
        Color::LightYellow => 11,
        Color::LightBlue => 12,
        Color::LightMagenta => 13,
        Color::LightCyan => 14,
        Color::White => 15,
        Color::Reset => return None,
    };
    Some(match index {
        0..=15 => ANSI[usize::from(index)],
        16..=231 => {
            let i = index - 16;
            let level = |v: u8| if v == 0 { 0 } else { 55 + v * 40 };
            (level(i / 36), level(i / 6 % 6), level(i % 6))
        }
        _ => {
            let v = 8 + (index - 232) * 10;
            (v, v, v)
        }
    })
}

/// `c` mixed over `base` at `level` (0 = all base, 1 = all `c`), as
/// truecolor. A color with no fixed value is returned as is.
pub fn mix(base: Color, c: Color, level: f32) -> Color {
    let (Some((br, bg, bb)), Some((r, g, b))) = (color_rgb(base), color_rgb(c)) else {
        return c;
    };
    let f = |base: u8, v: u8| {
        (f32::from(base) + (f32::from(v) - f32::from(base)) * level)
            .round()
            .clamp(0.0, 255.0) as u8
    };
    Color::Rgb(f(br, r), f(bg, g), f(bb, b))
}

/// Names the settings overlay cycles through; `by_name` accepts them
/// case-insensitively and falls back to the first entry.
pub const THEMES: &[&str] = &[
    "default", "ocean", "forest", "rose", "amber", "lavender", "coral", "slate", "sand", "mono",
];

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
    /// dot green. Sky blue unless the preset already owns blue, and never
    /// a violet: `merged` is purple in every preset, a merged checkout's
    /// row sits right above its unread session's, and the two dots say
    /// opposite things (come read this / this landed, delete it).
    pub done: Color,
    /// Running — the STATUS DOT and, on the grid, the running card's
    /// frame, one yellow for both — modified / flash messages / remote
    /// host.
    pub warn: Color,
    /// Needs feedback / deleted / destructive actions.
    pub err: Color,
    /// Terminated sessions and the session kind badge.
    pub special: Color,
    /// A merged pull request: its row's arrow, rail and badge, and the
    /// state word in the PR PREVIEW. Purple in every preset — the color
    /// GitHub paints a merge in, so the row says "landed" before the badge
    /// is read — and its own role rather than `special`, so a merged PR
    /// never wears the same color as a terminated session, whatever the
    /// preset makes of that one.
    pub merged: Color,
    /// The checkout a LAUNCHER CARD's session runs in, when that checkout
    /// is the project's ROOT WORKTREE: the branch its edits land on with
    /// nothing fenced around them. Amber in every preset, the way `merged`
    /// is purple in every preset — which of the two scopes a card is in is
    /// not a matter of taste, and two presets (`amber`, `sand`) carry a
    /// warm `accent` that would leave them unable to tell one from the
    /// other.
    pub root: Color,
    /// The counterpart, for a session in a worktree of its own — work
    /// fenced off from the root branch. Far enough from `root` across the
    /// 256-color cube to read as a different hue rather than a shade of
    /// it: telling the pair apart at a glance is the whole job, and
    /// `the_two_scope_colors_are_a_hue_apart_in_every_preset` holds them
    /// to it.
    pub worktree: Color,
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
    /// Merged counterpart of `warn_sweep`, for the checkout row whose pull
    /// request has landed. Purple family, paired with `merged`. A ONE-SHOT
    /// SWEEP: the row rides it for a few seconds after the merge is seen,
    /// then rests on `merged`.
    pub merged_sweep: [Color; 3],
    /// Unread-done counterpart, the other ONE-SHOT SWEEP: a row rides it
    /// for a few seconds after a turn finishes unread, then holds still.
    /// Rests on `done`, so a preset that moves `done` moves this with it.
    pub done_sweep: [Color; 3],
    /// Focused-surface background, behind the session pane or the card
    /// keys land in: the accent's own hue taken down to a near-black —
    /// OKLCH lightness 0.20 and chroma 0.04 in every preset (mono's is
    /// the one gray). On a black window it reads as the accent glowing
    /// faintly rather than as a gray slab, and it is darker than the gray
    /// it replaced, so dim text on it keeps more of its contrast.
    /// Truecolor by necessity (see module docs). In the light APPEARANCE
    /// the same idea runs the other way: the accent taken up to a
    /// near-white.
    pub focus_tint: Color,
    /// Drawn for a light terminal background (the APPEARANCE setting):
    /// every role above is then a dark-on-light shade, the solid
    /// background is white, and washes mix toward white, not black.
    pub light: bool,
}

/// `done` and its sweep for a preset that already owns blue.
const DONE_PINK: [Color; 3] = [
    Color::Indexed(212),
    Color::Indexed(218),
    Color::Indexed(225),
];
/// `done` and its sweep for a preset whose pinks and violets crowd the blue.
const DONE_TURQUOISE: [Color; 3] = [Color::Indexed(45), Color::Indexed(81), Color::Indexed(159)];

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
            done: Color::Indexed(75), // sky blue — a hue away from merged's purple, not a shade
            warn: Color::Yellow,
            err: Color::Red,
            special: Color::Magenta,
            merged: Color::Indexed(135),  // purple — GitHub's merged
            root: Color::Indexed(214),    // amber — the branch itself
            worktree: Color::Indexed(79), // aquamarine — fenced off from it
            sel_bg: Color::Indexed(237),
            sel_bg_dim: Color::Indexed(235),
            edge: Color::Indexed(238),
            warn_sweep: [Color::Yellow, Color::Indexed(220), Color::Indexed(230)],
            err_sweep: [Color::Red, Color::Indexed(203), Color::Indexed(217)],
            merged_sweep: [
                Color::Indexed(135),
                Color::Indexed(141),
                Color::Indexed(183),
            ],
            done_sweep: [Color::Indexed(75), Color::Indexed(111), Color::Indexed(153)],
            focus_tint: Color::Rgb(0, 27, 28),
            light: false,
        }
    }
}

/// The light APPEARANCE's `done` families, a shade darker than the dark
/// ones so they keep their contrast on white; each sweep darkens toward
/// its head, the way the dark ones brighten toward theirs.
const DONE_BLUE_LIGHT: [Color; 3] = [Color::Indexed(33), Color::Indexed(26), Color::Indexed(19)];
const DONE_PINK_LIGHT: [Color; 3] = [Color::Indexed(162), Color::Indexed(125), Color::Indexed(89)];
const DONE_TURQUOISE_LIGHT: [Color; 3] =
    [Color::Indexed(30), Color::Indexed(23), Color::Indexed(17)];

impl Theme {
    /// The palette the UI draws with: preset `name` in the APPEARANCE
    /// `appearance` resolves to (see [`appearance_is_light`]).
    pub fn resolve(name: &str, appearance: &str) -> Self {
        if appearance_is_light(appearance) {
            Self::light_by_name(name)
        } else {
            Self::by_name(name)
        }
    }

    /// What the solid-background setting paints under every cell nothing
    /// else colored: black, or white in the light APPEARANCE.
    pub fn canvas(&self) -> Color {
        if self.light {
            WHITE_BACKGROUND
        } else {
            BLACK_BACKGROUND
        }
    }

    /// `c` at `level` of its strength over the canvas: a faint wash of it
    /// — taken toward black on a dark background, toward white on a
    /// light one — as truecolor, since the 256 palette has no faint shade
    /// of most hues.
    pub fn wash(&self, c: Color, level: f32) -> Color {
        mix(self.canvas(), c, level)
    }

    /// Preset `name` for a light terminal background. The status hues
    /// keep their meaning — and every distinctness the dark presets hold
    /// to — but take shades dark enough to read on white; the gray
    /// tiers invert (text near-black, raised surfaces a step under
    /// white); and each accent is that preset's hue at a weight white
    /// doesn't wash out.
    pub fn light_by_name(name: &str) -> Self {
        let dark = Self::by_name(name);
        // The done family follows the dark preset's choice of hue.
        let done_sweep = if dark.done_sweep == DONE_PINK {
            DONE_PINK_LIGHT
        } else if dark.done_sweep == DONE_TURQUOISE {
            DONE_TURQUOISE_LIGHT
        } else {
            DONE_BLUE_LIGHT
        };
        let base = Self {
            accent: Color::Indexed(30), // teal — cyan, legible on white
            on_accent: Color::Indexed(231),
            text: Color::Indexed(235),
            muted: Color::Indexed(240),
            dim: Color::Indexed(245),
            ok: Color::Indexed(64), // olive green, a hue off the worktree's
            done: done_sweep[0],
            warn: Color::Indexed(136), // dark goldenrod: yellow on white
            err: Color::Indexed(160),
            special: Color::Indexed(127),
            merged: Color::Indexed(135),
            root: Color::Indexed(166),    // burnt orange
            worktree: Color::Indexed(29), // sea green
            sel_bg: Color::Indexed(253),
            sel_bg_dim: Color::Indexed(255),
            edge: Color::Indexed(250),
            warn_sweep: [Color::Indexed(136), Color::Indexed(94), Color::Indexed(58)],
            err_sweep: [Color::Indexed(160), Color::Indexed(124), Color::Indexed(88)],
            merged_sweep: [Color::Indexed(135), Color::Indexed(92), Color::Indexed(54)],
            done_sweep,
            focus_tint: Color::Reset, // derived from the accent below
            light: true,
        };
        let (accent, special) = match name.trim().to_ascii_lowercase().as_str() {
            "ocean" => (Color::Indexed(25), Color::Indexed(67)),
            "forest" => (Color::Indexed(22), Color::Indexed(65)),
            "rose" => (Color::Indexed(161), Color::Indexed(97)),
            "amber" => (Color::Indexed(130), Color::Indexed(95)),
            "lavender" => (Color::Indexed(61), Color::Indexed(133)),
            "coral" => (Color::Indexed(167), Color::Indexed(31)),
            "slate" => (Color::Indexed(60), Color::Indexed(24)),
            "sand" => (Color::Indexed(137), Color::Indexed(94)),
            "mono" => (Color::Indexed(232), Color::Indexed(243)),
            _ => (base.accent, base.special),
        };
        let mut th = Self {
            accent,
            special,
            ..base
        };
        if name.trim().eq_ignore_ascii_case("mono") {
            // Grayscale chrome, as in the dark mono: the accent is the
            // darkest step, text one over it, muted one over that.
            th.text = Color::Indexed(236);
            th.muted = Color::Indexed(241);
            th.focus_tint = Color::Rgb(238, 238, 238);
        } else {
            th.focus_tint = mix(WHITE_BACKGROUND, accent, 0.10);
        }
        th
    }

    pub fn by_name(name: &str) -> Self {
        let base = Self::default();
        match name.trim().to_ascii_lowercase().as_str() {
            "ocean" => Self {
                accent: Color::Indexed(39),  // deep sky blue
                special: Color::Indexed(75), // steel blue
                // Blue is spoken for twice over here (the done sky blue IS
                // this preset's terminated), so done goes pink — the one
                // hue a blue preset leaves free.
                done: DONE_PINK[0],
                done_sweep: DONE_PINK,
                focus_tint: Color::Rgb(3, 24, 38),
                ..base
            },
            "forest" => Self {
                accent: Color::Indexed(114),  // pale green
                special: Color::Indexed(108), // sage
                focus_tint: Color::Rgb(10, 27, 10),
                ..base
            },
            "rose" => Self {
                accent: Color::Indexed(211),  // pink
                special: Color::Indexed(141), // violet
                // Pink and violet are both spoken for here, and sky blue
                // sits too near the violet, so done goes turquoise — the one
                // hue this preset leaves free.
                done: DONE_TURQUOISE[0],
                done_sweep: DONE_TURQUOISE,
                focus_tint: Color::Rgb(36, 14, 21),
                ..base
            },
            "amber" => Self {
                accent: Color::Indexed(214),  // orange
                special: Color::Indexed(173), // copper
                focus_tint: Color::Rgb(32, 19, 1),
                ..base
            },
            "lavender" => Self {
                accent: Color::Indexed(147),  // periwinkle
                special: Color::Indexed(176), // orchid
                // The done sky blue would vanish into a periwinkle accent,
                // so done goes turquoise here, as in rose.
                done: DONE_TURQUOISE[0],
                done_sweep: DONE_TURQUOISE,
                focus_tint: Color::Rgb(20, 19, 39),
                ..base
            },
            "coral" => Self {
                accent: Color::Indexed(209), // coral
                // The cooled-off complement, the way default pairs cyan with
                // magenta: a terminated session in a warm preset reads as
                // gone cold rather than as a faded needs-feedback red.
                special: Color::Indexed(73), // cadet teal
                focus_tint: Color::Rgb(37, 15, 8),
                ..base
            },
            "slate" => Self {
                accent: Color::Indexed(110), // dusty sky blue
                special: Color::Indexed(67), // steel blue
                // The done sky blue would land between this preset's two
                // blues, so done goes pink here, as in ocean.
                done: DONE_PINK[0],
                done_sweep: DONE_PINK,
                focus_tint: Color::Rgb(6, 23, 39),
                ..base
            },
            "sand" => Self {
                accent: Color::Indexed(180),  // tan
                special: Color::Indexed(137), // bronze
                focus_tint: Color::Rgb(34, 18, 2),
                ..base
            },
            "mono" => Self {
                // Grayscale chrome for a terminal whose own palette is
                // colorful enough: the frame, titles and cursors go white
                // and gray, and only the statuses keep their color.
                accent: Color::White,
                // A step under the accent, so a white-on-white match
                // highlight still stands out from the text around it.
                text: Color::Indexed(252),
                // And a step under that, so unfocused titles still read as
                // secondary next to the text.
                muted: Color::Indexed(247),
                special: Color::Indexed(245),
                focus_tint: Color::Rgb(22, 22, 22),
                ..base
            },
            _ => base,
        }
    }
}

/// The NESTED layout's grays (Settings → Appearance → **Worktree layout**
/// → `nested`): a live root's title against an idle one's, the ages and
/// counts, and the cursor row's fill. Truecolor, like `focus_tint`: an
/// ANSI gray is whatever the terminal's palette makes of it, and the two
/// title steps could land on one shade there. Every hue the layout draws
/// — the status dots, the cursor's accent — is still the preset's own.
pub mod nested {
    use super::Theme;
    use ratatui::style::Color;

    /// A live root's title: working, or waiting on you.
    pub fn bright(th: &Theme) -> Color {
        if th.light {
            Color::Rgb(0x1c, 0x1c, 0x1c)
        } else {
            Color::Rgb(0xf2, 0xf2, 0xf2)
        }
    }
    /// An idle root's title, and every row's age, count and connector.
    pub fn dim(th: &Theme) -> Color {
        if th.light {
            Color::Rgb(0x70, 0x70, 0x70)
        } else {
            Color::Rgb(0x8c, 0x8c, 0x8c)
        }
    }
    /// A pull request's `#42`, underlined: a link, in the blue links wear.
    pub fn link(th: &Theme) -> Color {
        if th.light {
            Color::Rgb(0x09, 0x69, 0xda)
        } else {
            Color::Rgb(0x58, 0xa6, 0xff)
        }
    }
    /// How much of the accent the cursor's row keeps as its fill: the
    /// accent taken nearly to the canvas, so the row is washed in it.
    pub const SELECTED_FILL: f32 = 0.18;
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every preset in both APPEARANCES, named for the assertion
    /// messages: the rules below hold for a light terminal too.
    fn palettes() -> Vec<(String, Theme)> {
        THEMES
            .iter()
            .flat_map(|n| {
                [
                    (n.to_string(), Theme::by_name(n)),
                    (format!("{n} (light)"), Theme::light_by_name(n)),
                ]
            })
            .collect()
    }

    /// Red, green, blue coordinates (0..=5) of a 6x6x6 cube color.
    fn cube(c: Color) -> Option<[u8; 3]> {
        match c {
            Color::Indexed(n @ 16..=231) => {
                let n = n - 16;
                Some([n / 36, (n / 6) % 6, n % 6])
            }
            _ => None,
        }
    }

    /// How far two cube colors sit apart, in steps; None when either is
    /// not a cube color and the distance is not ours to measure.
    fn cube_steps(a: Color, b: Color) -> Option<u8> {
        let (a, b) = (cube(a)?, cube(b)?);
        Some((0..3).map(|i| a[i].abs_diff(b[i])).sum())
    }

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
        for (name, th) in palettes() {
            assert_ne!(th.done, th.ok, "{name}: done reads as plain success");
            assert_ne!(th.done, th.warn, "{name}: done reads as running");
            assert_ne!(th.done, th.err, "{name}: done reads as needs-feedback");
            assert_ne!(th.done, th.special, "{name}: done reads as terminated");
            assert_ne!(th.done, th.dim, "{name}: done reads as fresh");
            assert_ne!(th.done, th.accent, "{name}: done reads as the cursor");
        }
    }

    /// A running card's frame is the running yellow itself — the color its
    /// STATUS DOT wears — so no preset may hand `warn` to the accent or to
    /// the quiet edge: a running card would then pass for the cursor's, or
    /// for a card with nothing going on.
    #[test]
    fn a_running_frame_never_reads_as_the_focus() {
        for (name, th) in palettes() {
            assert_ne!(th.warn, th.accent, "{name}: running reads as the cursor");
            assert_ne!(th.warn, th.edge, "{name}: running reads as a quiet card");
        }
    }

    /// An unread-done dot sits right under a merged checkout's purple one,
    /// and the two mean opposite things — so done keeps a whole hue away
    /// from the merge, not a shade: it is no color the merged sweep passes
    /// through (the branch name would flash "done" on every pass), and —
    /// when both sit in the 256-color cube — it is at least three steps
    /// from the merge there. The violet this replaced was one.
    #[test]
    fn done_is_never_a_shade_of_merged() {
        for (name, th) in palettes() {
            assert!(
                !th.merged_sweep.contains(&th.done),
                "{name}: the merged sweep flashes the done color"
            );
            if let (Some(done), Some(merged)) = (cube(th.done), cube(th.merged)) {
                let steps: u8 = (0..3).map(|i| done[i].abs_diff(merged[i])).sum();
                assert!(
                    steps >= 3,
                    "{name}: done is {steps} cube steps from merged: a shade, not a hue"
                );
            }
        }
    }

    /// The ONE-SHOT SWEEP an unread finish rides rests on that preset's
    /// `done` — a preset that moves the one moves the other — brightens
    /// toward its head, and is nobody else's sweep.
    #[test]
    fn done_sweep_rests_on_done_in_every_preset() {
        for (name, th) in palettes() {
            assert_eq!(th.done_sweep[0], th.done, "{name}: the sweep rests on done");
            let [tail, mid, head] = th.done_sweep;
            assert!(
                tail != mid && mid != head && tail != head,
                "{name}: a flat band"
            );
            for other in [th.warn_sweep, th.err_sweep, th.merged_sweep] {
                assert!(
                    th.done_sweep.iter().all(|c| !other.contains(c)),
                    "{name}: the done sweep borrows a shade from another"
                );
            }
        }
    }

    /// A LAUNCHER CARD names the checkout its session runs in in a SCOPE
    /// COLOR — `⌂` amber on the project's root branch, `↳` aquamarine in a
    /// worktree of its own — so a screenful of cards sorts into the two
    /// without a word being read. That only holds while the pair keeps a
    /// whole hue between them (the three cube steps `done` keeps from
    /// `merged`), and while neither lands on a color the same card already
    /// wears somewhere else: the status its dot and name take, the `done`
    /// its ago label takes, the `merged` its pull-request row takes, or
    /// the dim its harness takes.
    #[test]
    fn the_two_scope_colors_are_a_hue_apart_in_every_preset() {
        for (name, th) in palettes() {
            assert_ne!(th.root, th.worktree, "{name}: one scope, painted twice");
            let steps = cube_steps(th.root, th.worktree);
            assert!(
                steps.is_none_or(|s| s >= 3),
                "{name}: the scopes are {steps:?} cube steps apart: a shade, not a hue"
            );
            for (role, color) in [
                ("ok", th.ok),
                ("warn", th.warn),
                ("err", th.err),
                ("done", th.done),
                ("merged", th.merged),
                ("dim", th.dim),
                ("muted", th.muted),
            ] {
                assert_ne!(th.root, color, "{name}: a root checkout reads as {role}");
                assert_ne!(th.worktree, color, "{name}: a worktree reads as {role}");
            }
        }
    }

    /// The accent has to stand out from the text it highlights, and the
    /// text from the muted tier under it, in every preset — including the
    /// grayscale one, which is the preset that moves those roles.
    #[test]
    fn accent_text_and_muted_stay_three_tiers_in_every_preset() {
        for (name, th) in palettes() {
            assert_ne!(th.accent, th.text, "{name}: a match highlight vanishes");
            assert_ne!(th.text, th.muted, "{name}: secondary text reads as primary");
            assert_ne!(th.muted, th.dim, "{name}: secondary text reads as a hint");
            assert_ne!(
                th.accent, th.muted,
                "{name}: an unfocused title reads as focused"
            );
        }
    }

    /// A merged pull request is purple in every preset, and that purple is
    /// nobody else's: not the terminated `special`, not the unread `done`
    /// a preset may move around, and not a status that would make a
    /// landed pull request look like it needs someone.
    #[test]
    fn merged_is_purple_and_its_own_color_in_every_preset() {
        for (name, th) in palettes() {
            assert_eq!(th.merged, Color::Indexed(135), "{name}: merged is purple");
            assert_ne!(th.merged, th.special, "{name}: merged reads as terminated");
            assert_ne!(th.merged, th.done, "{name}: merged reads as unread done");
            assert_ne!(th.merged, th.ok, "{name}: merged reads as plain success");
            assert_ne!(th.merged, th.warn, "{name}: merged reads as running");
            assert_ne!(th.merged, th.err, "{name}: merged reads as needs-feedback");
            assert_ne!(th.merged, th.accent, "{name}: merged reads as open");
            assert_ne!(th.merged, th.dim, "{name}: merged reads as a draft");
            // The sweep a merged checkout's branch name rides rests on that
            // same purple, and is nobody else's sweep.
            assert_eq!(
                th.merged_sweep[0], th.merged,
                "{name}: the sweep rests on merged"
            );
            assert_ne!(
                th.merged_sweep, th.warn_sweep,
                "{name}: the sweep reads as running"
            );
            assert_ne!(
                th.merged_sweep, th.err_sweep,
                "{name}: the sweep reads as needs-feedback"
            );
        }
    }

    /// `focus_tint` fills the surface keys land in, so it has to be seen —
    /// much darker and it reads as plain black (issue #6) — read as the
    /// accent rather than as gray, which every channel near one value is
    /// on a black window, and stay dark enough that dim text keeps its
    /// contrast: `#666` holds over 3.1:1 of the 3.7:1 it has on black.
    /// Mono is the one gray preset, and takes the one gray tint.
    #[test]
    fn focus_tint_is_visible_hued_and_dark() {
        // WCAG relative luminance.
        let luminance = |rgb: [u8; 3]| {
            let lin = |c: u8| {
                let c = f64::from(c) / 255.0;
                if c <= 0.04045 {
                    c / 12.92
                } else {
                    ((c + 0.055) / 1.055).powf(2.4)
                }
            };
            0.2126 * lin(rgb[0]) + 0.7152 * lin(rgb[1]) + 0.0722 * lin(rgb[2])
        };
        for name in THEMES {
            let th = Theme::by_name(name);
            let Color::Rgb(r, g, b) = th.focus_tint else {
                panic!("{name}: focus_tint must be truecolor RGB");
            };
            let (hi, lo) = (r.max(g).max(b), r.min(g).min(b));
            assert!(
                hi >= 22,
                "{name}: focus_tint {:?} reads as black",
                (r, g, b)
            );
            assert!(
                luminance([r, g, b]) <= 0.009,
                "{name}: focus_tint {:?} costs dim text its contrast",
                (r, g, b)
            );
            if *name == "mono" {
                assert_eq!(hi, lo, "mono: a gray preset takes a gray tint");
            } else {
                assert!(
                    hi - lo >= 15,
                    "{name}: focus_tint {:?} reads as gray",
                    (r, g, b)
                );
            }
        }
    }

    /// WCAG contrast of `c` against white.
    fn contrast_on_white(c: Color) -> f64 {
        let (r, g, b) = color_rgb(c).expect("a fixed color");
        let lin = |c: u8| {
            let c = f64::from(c) / 255.0;
            if c <= 0.04045 {
                c / 12.92
            } else {
                ((c + 0.055) / 1.055).powf(2.4)
            }
        };
        let l = 0.2126 * lin(r) + 0.7152 * lin(g) + 0.0722 * lin(b);
        1.05 / (l + 0.05)
    }

    /// A light palette has to read on white: the text tiers keep their
    /// order of contrast, and every hue a glyph wears — accent, statuses,
    /// scopes — holds the 3:1 WCAG asks of large text and UI glyphs.
    #[test]
    fn light_palettes_read_on_white() {
        for name in THEMES {
            let th = Theme::light_by_name(name);
            assert!(th.light, "{name}: marked light");
            assert_eq!(th.canvas(), WHITE_BACKGROUND);
            assert!(contrast_on_white(th.text) >= 7.0, "{name}: text");
            assert!(contrast_on_white(th.muted) >= 4.5, "{name}: muted");
            assert!(contrast_on_white(th.dim) >= 3.0, "{name}: dim");
            for (role, c) in [
                ("accent", th.accent),
                ("ok", th.ok),
                ("done", th.done),
                ("warn", th.warn),
                ("err", th.err),
                ("special", th.special),
                ("merged", th.merged),
                ("root", th.root),
                ("worktree", th.worktree),
            ] {
                let ratio = contrast_on_white(c);
                assert!(ratio >= 3.0, "{name}: {role} is {ratio:.2}:1 on white");
            }
            // Raised surfaces sit a step under white, the focused one
            // further than the unfocused one, and the frame under both.
            let lum = |c: Color| 1.05 / contrast_on_white(c) - 0.05;
            assert!(lum(th.sel_bg) < lum(th.sel_bg_dim), "{name}: sel fills");
            assert!(lum(th.edge) < lum(th.sel_bg), "{name}: the edge recedes");
            // The focus tint is a pale wash of the accent: near-white, but
            // not white, so the surface keys land in still shows.
            let Color::Rgb(r, g, b) = th.focus_tint else {
                panic!("{name}: focus_tint must be truecolor RGB");
            };
            assert_ne!((r, g, b), (255, 255, 255), "{name}: tint invisible");
            assert!(lum(th.focus_tint) > 0.75, "{name}: tint too dark for text");
        }
    }

    /// Washes mix toward the canvas: black in the dark APPEARANCE (the
    /// `dim_toward_black` the cards always wore), white in the light one.
    #[test]
    fn washes_mix_toward_the_canvas() {
        let dark = Theme::default();
        let light = Theme::light_by_name("default");
        let c = Color::Indexed(209);
        assert_eq!(dark.wash(c, 0.5), Color::Rgb(128, 68, 48));
        assert_eq!(dark.wash(c, 0.0), Color::Rgb(0, 0, 0));
        assert_eq!(light.wash(c, 0.0), Color::Rgb(255, 255, 255));
        assert_eq!(light.wash(c, 1.0), Color::Rgb(255, 135, 95));
        assert_eq!(light.wash(Color::Reset, 0.3), Color::Reset);
    }

    #[test]
    fn osc11_replies_parse_at_every_channel_width() {
        assert_eq!(
            parse_osc11(b"\x1b]11;rgb:ffff/ffff/ffff\x1b\\"),
            Some((255, 255, 255))
        );
        assert_eq!(
            parse_osc11(b"\x1b]11;rgb:1d1d/1f1f/2121\x07"),
            Some((29, 31, 33))
        );
        assert_eq!(parse_osc11(b"\x1b]11;rgb:f/8/0\x07"), Some((255, 136, 0)));
        assert_eq!(
            parse_osc11(b"\x1b]11;rgba:fa/fa/fa/ff\x07"),
            Some((250, 250, 250))
        );
        // A DA1 reply riding behind it is not part of the color.
        assert_eq!(
            parse_osc11(b"\x1b]11;rgb:0000/0000/0000\x1b\\\x1b[?62;22c"),
            Some((0, 0, 0))
        );
        assert_eq!(parse_osc11(b"\x1b[?62;22c"), None);
        assert_eq!(parse_osc11(b"\x1b]11;rgb:zz/00/00\x07"), None);
        assert!(rgb_is_light((255, 255, 255)));
        assert!(rgb_is_light((0xfd, 0xf6, 0xe3)), "solarized light");
        assert!(!rgb_is_light((0x1d, 0x1f, 0x21)));
        assert!(!rgb_is_light((0x00, 0x2b, 0x36)), "solarized dark");
    }

    #[test]
    fn colorfgbg_names_the_background_last() {
        assert_eq!(colorfgbg_is_light("0;15"), Some(true));
        assert_eq!(colorfgbg_is_light("0;default;15"), Some(true));
        assert_eq!(colorfgbg_is_light("15;0"), Some(false));
        assert_eq!(colorfgbg_is_light("12;8"), Some(false));
        assert_eq!(colorfgbg_is_light("0;7"), Some(true));
        assert_eq!(colorfgbg_is_light("default;default"), None);
    }

    /// A forced APPEARANCE ignores whatever the terminal said.
    #[test]
    fn a_forced_appearance_wins() {
        assert!(appearance_is_light("light"));
        assert!(appearance_is_light(" Light "));
        assert!(!appearance_is_light("dark"));
        assert!(Theme::resolve("ocean", "light").light);
        assert_eq!(Theme::resolve("ocean", "dark"), Theme::by_name("ocean"));
    }
}
