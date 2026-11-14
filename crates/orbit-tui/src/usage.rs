//! USAGE AND LIMITS, the TUI's half: what the DAEMON's `ServerEvent::Usage`
//! says — each Claude session's context window use and the account's plan
//! limits — kept for the footer (`5h 23% ↻14:05 · wk 41%`) and the session
//! cards (`ctx 42%`). The daemon reads them off Claude Code's status line
//! (`orbit-daemon/src/usage.rs`).

use crate::theme::Theme;
use orbit_core::{AgentId, RateWindow};
use ratatui::style::{Color, Style};
use ratatui::text::Span;
use std::collections::HashMap;

/// From here a reading is worth a glance: drawn in the theme's `warn`.
pub const WARN_PCT: f32 = 80.0;
/// From here it is about to run out: drawn in the theme's `err`.
pub const ERR_PCT: f32 = 95.0;

#[derive(Debug, Clone, Default)]
pub struct Usage {
    /// Context window use per agent, 0–100, for agents whose status line
    /// has counted it.
    pub context: HashMap<AgentId, f32>,
    pub five_hour: Option<RateWindow>,
    pub seven_day: Option<RateWindow>,
}

impl Usage {
    /// Fold in one `ServerEvent::Usage`. The limits are the account's as
    /// the daemon holds them now, so they replace whatever was here.
    pub fn land(
        &mut self,
        agent: AgentId,
        context_pct: Option<f32>,
        five_hour: Option<RateWindow>,
        seven_day: Option<RateWindow>,
    ) {
        match context_pct {
            Some(pct) => self.context.insert(agent, pct),
            None => self.context.remove(&agent),
        };
        self.five_hour = five_hour;
        self.seven_day = seven_day;
    }

    pub fn context_pct(&self, agent: &AgentId) -> Option<f32> {
        self.context.get(agent).copied()
    }
}

/// `base`, or the warning colours once `pct` reaches [`WARN_PCT`] /
/// [`ERR_PCT`].
pub fn level_color(pct: f32, base: Color, th: Theme) -> Color {
    if pct >= ERR_PCT {
        th.err
    } else if pct >= WARN_PCT {
        th.warn
    } else {
        base
    }
}

/// A card's `ctx 42%`, and its colour on a card whose plain text is `base`.
pub fn context_label(pct: f32, base: Color, th: Theme) -> (String, Color) {
    (format!("ctx {pct:.0}%"), level_color(pct, base, th))
}

/// The footer's plan limits at `now` (epoch seconds), in the most detailed
/// form that fits `room` cells: `5h 23% ↻14:05 · wk 41%`, then without the
/// reset time, then the 5-hour window alone. Empty with no limits known
/// (or all of them past their reset), or no room.
pub fn limit_spans(usage: &Usage, now: i64, room: usize, th: Theme) -> Vec<Span<'static>> {
    let five = usage.five_hour.filter(|w| w.live_at(now));
    let seven = usage.seven_day.filter(|w| w.live_at(now));
    let window = |label: &str, w: RateWindow| {
        Span::styled(
            format!("{label} {:.0}%", w.used_pct),
            Style::default().fg(level_color(w.used_pct, th.dim, th)),
        )
    };
    let dim = |s: String| Span::styled(s, Style::default().fg(th.dim));
    let mut forms: Vec<Vec<Span<'static>>> = Vec::new();
    let full = |reset: bool| {
        let mut spans = Vec::new();
        if let Some(w) = five {
            spans.push(window("5h", w));
            if reset {
                spans.push(dim(format!(
                    " ↻{}",
                    orbit_core::clock::local_hh_mm(w.resets_at)
                )));
            }
        }
        if let Some(w) = seven {
            if !spans.is_empty() {
                spans.push(dim(" · ".into()));
            }
            spans.push(window("wk", w));
        }
        spans
    };
    forms.push(full(true));
    forms.push(full(false));
    forms.extend(five.map(|w| vec![window("5h", w)]));
    forms
        .into_iter()
        .find(|spans| {
            let w: usize = spans.iter().map(Span::width).sum();
            w > 0 && w <= room
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(spans: &[Span]) -> String {
        spans.iter().map(|s| s.content.as_ref()).collect()
    }

    #[test]
    fn limits_drop_the_reset_then_the_weekly_as_room_shrinks() {
        let th = Theme::default();
        let now = 1_000;
        let usage = Usage {
            five_hour: Some(RateWindow {
                used_pct: 23.4,
                resets_at: now + 3600,
            }),
            seven_day: Some(RateWindow {
                used_pct: 41.2,
                resets_at: now + 86_400,
            }),
            ..Usage::default()
        };
        let at = orbit_core::clock::local_hh_mm(now + 3600);
        let full = format!("5h 23% ↻{at} · wk 41%");
        assert_eq!(text(&limit_spans(&usage, now, 80, th)), full);
        assert_eq!(
            text(&limit_spans(&usage, now, full.chars().count() - 1, th)),
            "5h 23% · wk 41%"
        );
        assert_eq!(text(&limit_spans(&usage, now, 10, th)), "5h 23%");
        assert!(limit_spans(&usage, now, 3, th).is_empty());
        // Past the 5-hour reset only the weekly is left.
        assert_eq!(text(&limit_spans(&usage, now + 3600, 80, th)), "wk 41%");
        assert!(limit_spans(&Usage::default(), now, 80, th).is_empty());
    }

    #[test]
    fn levels_warn_at_80_and_err_at_95() {
        let th = Theme::default();
        assert_eq!(level_color(79.9, th.dim, th), th.dim);
        assert_eq!(level_color(80.0, th.dim, th), th.warn);
        assert_eq!(level_color(95.0, th.dim, th), th.err);
        assert_eq!(context_label(42.4, th.dim, th), ("ctx 42%".into(), th.dim));
    }

    #[test]
    fn landing_replaces_the_limits_and_tracks_context_per_agent() {
        let mut usage = Usage::default();
        let w = RateWindow {
            used_pct: 10.0,
            resets_at: 5,
        };
        usage.land(AgentId("a".into()), Some(42.0), Some(w), None);
        usage.land(AgentId("b".into()), None, None, Some(w));
        assert_eq!(usage.context_pct(&AgentId("a".into())), Some(42.0));
        assert_eq!(usage.context_pct(&AgentId("b".into())), None);
        assert!(usage.five_hour.is_none());
        assert_eq!(usage.seven_day, Some(w));
        usage.land(AgentId("a".into()), None, None, None);
        assert_eq!(usage.context_pct(&AgentId("a".into())), None);
    }
}
