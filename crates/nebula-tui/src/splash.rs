//! First-run splash: a procedurally animated nebula filling the body while
//! the project tree is empty. Every cell is computed per frame — a rotating
//! spiral-arm density field modulated by value noise picks a glyph from a
//! dust ramp, with a hashed starfield twinkling in the empty sky and the
//! wordmark materializing in a carved-out band the dust never paints.
//! Indexed colors only: Terminal.app has no truecolor.
//!
//! The event loop ticks a repaint every [`FRAME`] while [`App::splash_active`]
//! holds; the scene itself is a pure function of elapsed time, so a missed
//! frame skips ahead instead of stuttering.

use crate::app::{App, Focus, HitTarget};
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;
use std::time::Duration;

/// Repaint cadence while the splash is up.
pub const FRAME: Duration = Duration::from_millis(100);

/// Seconds to fade the sky in from black. Also hides the splash flashing
/// briefly on every launch before the daemon's first tree snapshot lands.
const FADE_IN: f32 = 1.2;

/// Glyph ramp, thin dust -> bright core. Single-width chars only.
const RAMP: &[char] = &['.', ':', '·', '+', '*', '*', 'o', '@'];
/// 256-color ramp under `RAMP`: deep blue -> violet -> magenta -> pink.
const DUST: &[u8] = &[17, 54, 55, 92, 93, 129, 135, 177];
/// Wordmark gradient, swept left to right.
const MARK: &[u8] = &[99, 105, 141, 177, 213, 219];

/// 5-row block bitmaps for N E B U L A.
const LETTERS: &[&[&str; 5]] = &[
    &["#...#", "##..#", "#.#.#", "#..##", "#...#"],
    &["####", "#...", "###.", "#...", "####"],
    &["###.", "#..#", "###.", "#..#", "###."],
    &["#..#", "#..#", "#..#", "#..#", ".##."],
    &["#...", "#...", "#...", "#...", "####"],
    &[".##.", "#..#", "####", "#..#", "#..#"],
];

fn hash(x: i32, y: i32, salt: u32) -> u32 {
    let mut h = (x as u32).wrapping_mul(374_761_393)
        ^ (y as u32).wrapping_mul(668_265_263)
        ^ salt.wrapping_mul(2_246_822_519);
    h = (h ^ (h >> 13)).wrapping_mul(1_274_126_177);
    h ^ (h >> 16)
}

fn hash01(x: i32, y: i32, salt: u32) -> f32 {
    (hash(x, y, salt) & 0xffff) as f32 / 65535.0
}

/// One octave of smooth 2D value noise.
fn vnoise(x: f32, y: f32, salt: u32) -> f32 {
    let (xi, yi) = (x.floor() as i32, y.floor() as i32);
    let (fx, fy) = (x - x.floor(), y - y.floor());
    let sx = fx * fx * (3.0 - 2.0 * fx);
    let sy = fy * fy * (3.0 - 2.0 * fy);
    let a = hash01(xi, yi, salt);
    let b = hash01(xi + 1, yi, salt);
    let c = hash01(xi, yi + 1, salt);
    let d = hash01(xi + 1, yi + 1, salt);
    a + (b - a) * sx + (c - a) * sy + (a - b - c + d) * sx * sy
}

/// Dust density at physical offset (dx, dy) from the galaxy core: two
/// spiral arms whose phase advances with radius (rotating with `rot`),
/// broken up by noise sampled in a counter-rotating frame so the wisps
/// shear against the arms instead of riding them.
fn density(dx: f32, dy: f32, rot: f32) -> f32 {
    let r = (dx * dx + dy * dy).sqrt().max(0.001);
    let theta = dy.atan2(dx);
    let arm = ((2.0 * theta - r * 2.6 + rot).cos() * 0.5 + 0.5).powi(3);
    let (sa, ca) = (rot * 0.3).sin_cos();
    let (nx, ny) = (dx * ca - dy * sa, dx * sa + dy * ca);
    let wisp = 0.55 + 0.45 * vnoise(nx * 3.0 + 7.0, ny * 3.0 + 3.0, 991);
    let falloff = (-r * 1.6).exp();
    let core = (-r * r * 22.0).exp();
    (arm * wisp * falloff * 1.5 + core).min(1.0)
}

/// One wordmark row as per-cell spans: gradient across the word, a slow
/// shine sweeping through, and the blocks materializing from static
/// (`░` -> `▒` -> `█`) while the scene fades in.
fn wordmark_line(row: usize, t: f32, fade: f32) -> Line<'static> {
    let width: usize = LETTERS.iter().map(|l| l[0].len()).sum::<usize>() + 2 * (LETTERS.len() - 1);
    let block = if fade < 0.5 {
        "░"
    } else if fade < 0.85 {
        "▒"
    } else {
        "█"
    };
    let mut spans = Vec::new();
    let mut col = 0usize;
    for (i, letter) in LETTERS.iter().enumerate() {
        if i > 0 {
            spans.push(Span::raw("  "));
            col += 2;
        }
        for ch in letter[row].chars() {
            if ch == '#' {
                let u = col as f32 / width as f32;
                let shine = (u * 5.0 - t * 1.4).sin() > 0.93;
                let color = if shine && fade >= 1.0 {
                    231 // near-white glint
                } else {
                    let gi = (u * (MARK.len() as f32 - 1.0)).round() as usize;
                    MARK[gi]
                };
                spans.push(Span::styled(
                    block,
                    Style::default()
                        .fg(Color::Indexed(color))
                        .add_modifier(Modifier::BOLD),
                ));
            } else {
                spans.push(Span::raw(" "));
            }
            col += 1;
        }
    }
    Line::from(spans)
}

pub fn draw_splash(f: &mut Frame, app: &mut App, area: Rect) {
    let th = app.theme;
    if area.width < 8 || area.height < 4 {
        return;
    }
    // Animations off: the event loop doesn't tick us, so hold one finished
    // frame (well past the fade-in) instead of whatever instant a stray
    // redraw lands on.
    let t = if app.animations {
        app.splash_epoch.elapsed().as_secs_f32()
    } else {
        60.0
    };
    let raw = (t / FADE_IN).clamp(0.0, 1.0);
    let fade = raw * raw * (3.0 - 2.0 * raw);

    // ---- text block: wordmark, tagline, key hints, bottom-anchored ----
    let big = area.width >= 50 && area.height >= 18;
    let mut lines: Vec<Line> = Vec::new();
    if big {
        for row in 0..5 {
            lines.push(wordmark_line(row, t, fade));
        }
    } else {
        lines.push(Line::from(vec![
            Span::styled("◆ ", Style::default().fg(th.accent)),
            Span::styled(
                "nebula",
                Style::default().fg(th.text).add_modifier(Modifier::BOLD),
            ),
        ]));
    }
    lines.push(Line::from(""));
    if area.width >= 47 {
        lines.push(Line::from(Span::styled(
            "your agents keep running, even when you leave",
            Style::default().fg(th.dim),
        )));
        lines.push(Line::from(""));
    }
    let key = |k: &str, label: &str| {
        vec![
            Span::styled(
                k.to_string(),
                Style::default().fg(th.accent).add_modifier(Modifier::BOLD),
            ),
            Span::styled(format!(" {label}"), Style::default().fg(th.dim)),
        ]
    };
    let mut hint = Vec::new();
    if !app.tree.has_visible_projects() {
        hint.extend(key("n / o", "create your first project"));
        hint.push(Span::styled("   ·   ", Style::default().fg(th.dim)));
        hint.extend(key("?", "help"));
    } else {
        // Summoned as a preview over a populated tree.
        hint.extend(key("any key", "returns"));
    }
    lines.push(Line::from(hint));

    let block_w = (lines.iter().map(Line::width).max().unwrap_or(0) as u16).min(area.width);
    let block_h = (lines.len() as u16).min(area.height);
    let text = Rect {
        x: area.x + (area.width - block_w) / 2,
        y: area.y + area.height - block_h - u16::from(area.height > block_h),
        width: block_w,
        height: block_h,
    };

    // ---- galaxy centered in the sky above the text ----
    let above = (text.y - area.y).max(4);
    let cx = f32::from(area.x) + f32::from(area.width) / 2.0;
    let cy = f32::from(area.y) + f32::from(above) / 2.0;
    // Independent x/y scales stretch the disc to fill the sky; a terminal
    // cell is ~2x taller than wide, hence the factor 2 on y.
    let sx = 2.35 / (0.42 * f32::from(area.width)).max(4.0);
    let sy = 2.0 * 2.35 / (1.6 * f32::from(above)).max(4.0);
    // Text carve: rows the dust and stars never touch.
    let carve = Rect {
        x: text.x.saturating_sub(3),
        y: text.y.saturating_sub(1),
        width: text.width + 6,
        height: text.height + 2,
    }
    .intersection(area);

    let rot = t * 0.25;
    let buf = f.buffer_mut();
    for y in area.top()..area.bottom() {
        for x in area.left()..area.right() {
            if x >= carve.left() && x < carve.right() && y >= carve.top() && y < carve.bottom() {
                continue;
            }
            let dx = (f32::from(x) - cx) * sx;
            let dy = (f32::from(y) - cy) * sy;
            let d = density(dx, dy, rot) * fade;
            if d >= 0.055 {
                let v = ((d - 0.055) / 0.945).clamp(0.0, 1.0);
                let i = (v * (RAMP.len() as f32 - 1.0)).round() as usize;
                buf[(x, y)]
                    .set_char(RAMP[i])
                    .set_fg(Color::Indexed(DUST[i]));
                continue;
            }
            // Empty sky: sparse stars on their own twinkle phases, plus
            // the rare accent-colored sparkle.
            let h = hash(i32::from(x), i32::from(y), 12_345);
            if !h.is_multiple_of(53) {
                continue;
            }
            let phase = ((h >> 8) % 8) as f32 * 0.8;
            let tw = ((t * 2.5 + phase).sin() * 0.5 + 0.5) * fade;
            if tw <= 0.45 {
                continue;
            }
            if (h >> 4).is_multiple_of(111) {
                buf[(x, y)].set_char('+').set_fg(th.accent);
            } else if tw > 0.8 {
                buf[(x, y)].set_char('·').set_fg(Color::Indexed(189));
            } else {
                buf[(x, y)].set_char('.').set_fg(Color::Indexed(60));
            }
        }
    }

    f.render_widget(Paragraph::new(lines).centered(), text);
    // A click anywhere lands focus back on the (invisible) projects panel,
    // where `n` creates the first project.
    app.hits.push((area, HitTarget::PanelBg(Focus::Projects)));
}
