//! USAGE AND LIMITS: what Claude Code's status line reports — each
//! session's context window use and the account's plan limits — held in
//! memory and pushed to every TUI as `ServerEvent::Usage`.
//!
//! Claude Code runs the `statusLine` command Orbit manages in each
//! worktree's `.claude/settings.local.json` (`hooks::installer`) after
//! every assistant message, and hands it a JSON document on stdin. The
//! command POSTs that document to the hook receiver's `/api/statusline`,
//! which lands here: [`parse_report`] reads it leniently, [`UsageBoard::record`]
//! keeps it and answers with the one-line text the command prints back
//! into Claude's own status line ([`status_line`]).
//!
//! Nothing is persisted: a daemon restart forgets it all, and the next
//! status line report fills it again.

use orbit_core::{AgentId, RateWindow, ServerEvent};
use parking_lot::Mutex;
use serde_json::Value;
use std::collections::HashMap;
use tokio::sync::broadcast;

/// One status line report, as much of it as Claude sent. Every part is
/// optional: `used_percentage` is null early in a session, and
/// `rate_limits` comes only for Pro/Max subscribers, only after the first
/// API response, and either window may be missing from it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct StatusReport {
    pub model: Option<String>,
    pub context_pct: Option<f32>,
    pub context_window: Option<u64>,
    pub five_hour: Option<RateWindow>,
    pub seven_day: Option<RateWindow>,
}

/// Read a status line document without ever failing: a field missing, null
/// or of the wrong type is simply absent from the report.
pub fn parse_report(body: &str) -> StatusReport {
    let Ok(v) = serde_json::from_str::<Value>(body) else {
        return StatusReport::default();
    };
    let model = v
        .pointer("/model/display_name")
        .or_else(|| v.pointer("/model/id"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    let pct = |p: &str| {
        v.pointer(p)
            .and_then(Value::as_f64)
            .filter(|n| n.is_finite())
            .map(|n| n.clamp(0.0, 100.0) as f32)
    };
    let window = |name: &str| {
        let used = pct(&format!("/rate_limits/{name}/used_percentage"))?;
        let resets_at = v
            .pointer(&format!("/rate_limits/{name}/resets_at"))
            .and_then(Value::as_f64)
            .filter(|n| n.is_finite())? as i64;
        Some(RateWindow {
            used_pct: used,
            resets_at,
        })
    };
    StatusReport {
        model,
        context_pct: pct("/context_window/used_percentage"),
        context_window: v
            .pointer("/context_window/context_window_size")
            .and_then(Value::as_u64),
        five_hour: window("five_hour"),
        seven_day: window("seven_day"),
    }
}

/// The one line Claude's status bar shows for a session run inside Orbit:
/// `Opus · ctx 42% · 5h 23% ↻14:05 · wk 41%`, each part only when known.
pub fn status_line(
    model: Option<&str>,
    context_pct: Option<f32>,
    five_hour: Option<RateWindow>,
    seven_day: Option<RateWindow>,
) -> String {
    let mut parts: Vec<String> = Vec::new();
    if let Some(model) = model {
        parts.push(model.to_string());
    }
    if let Some(pct) = context_pct {
        parts.push(format!("ctx {pct:.0}%"));
    }
    if let Some(w) = five_hour {
        parts.push(format!(
            "5h {:.0}% ↻{}",
            w.used_pct,
            orbit_core::clock::local_hh_mm(w.resets_at)
        ));
    }
    if let Some(w) = seven_day {
        parts.push(format!("wk {:.0}%", w.used_pct));
    }
    parts.join(" · ")
}

#[derive(Debug, Clone, Default)]
struct AgentUsage {
    model: Option<String>,
    context_pct: Option<f32>,
    context_window: Option<u64>,
}

#[derive(Debug, Default)]
struct UsageState {
    agents: HashMap<AgentId, AgentUsage>,
    five_hour: Option<RateWindow>,
    seven_day: Option<RateWindow>,
}

impl UsageState {
    /// The account's limits still counting at `now`: a window whose reset
    /// has passed is forgotten rather than shown stale.
    fn limits(&mut self, now: i64) -> (Option<RateWindow>, Option<RateWindow>) {
        self.five_hour = self.five_hour.filter(|w| w.live_at(now));
        self.seven_day = self.seven_day.filter(|w| w.live_at(now));
        (self.five_hour, self.seven_day)
    }

    fn event(&mut self, agent: &AgentId, now: i64) -> ServerEvent {
        let (five_hour, seven_day) = self.limits(now);
        let usage = self.agents.get(agent).cloned().unwrap_or_default();
        ServerEvent::Usage {
            agent: agent.clone(),
            context_pct: usage.context_pct,
            context_window: usage.context_window,
            five_hour,
            seven_day,
        }
    }
}

/// The daemon's memory of every status line report, and the channel its
/// `Usage` events go out on — a channel of its own, so a connection can
/// listen to it only when its client knows the event
/// (`orbit_core::USAGE_PROTOCOL`).
pub struct UsageBoard {
    state: Mutex<UsageState>,
    events: broadcast::Sender<ServerEvent>,
}

impl Default for UsageBoard {
    fn default() -> Self {
        let (events, _) = broadcast::channel(256);
        Self {
            state: Mutex::new(UsageState::default()),
            events,
        }
    }
}

impl UsageBoard {
    /// Keep `agent`'s report, tell every listening TUI, and answer with the
    /// line its status bar should show. The limits are the account's, so a
    /// report that carries a window replaces it whichever session sent it,
    /// and one that carries none leaves the last one standing.
    pub fn record(&self, agent: &AgentId, report: StatusReport, now: i64) -> String {
        let (event, line) = {
            let mut state = self.state.lock();
            if report.five_hour.is_some() {
                state.five_hour = report.five_hour;
            }
            if report.seven_day.is_some() {
                state.seven_day = report.seven_day;
            }
            state.agents.insert(
                agent.clone(),
                AgentUsage {
                    model: report.model.clone(),
                    context_pct: report.context_pct,
                    context_window: report.context_window,
                },
            );
            let event = state.event(agent, now);
            let (five_hour, seven_day) = state.limits(now);
            let line = status_line(
                report.model.as_deref(),
                report.context_pct,
                five_hour,
                seven_day,
            );
            (event, line)
        };
        // Nobody listening is not an error.
        let _ = self.events.send(event);
        line
    }

    /// One `Usage` per agent that has reported, for a subscriber that just
    /// took its `Snapshot`.
    pub fn snapshot(&self, now: i64) -> Vec<ServerEvent> {
        let mut state = self.state.lock();
        let agents: Vec<AgentId> = state.agents.keys().cloned().collect();
        agents.iter().map(|a| state.event(a, now)).collect()
    }

    pub fn subscribe(&self) -> broadcast::Receiver<ServerEvent> {
        self.events.subscribe()
    }

    /// The last model `agent`'s status line named, if any.
    pub fn model(&self, agent: &AgentId) -> Option<String> {
        self.state.lock().agents.get(agent)?.model.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The documented sample (code.claude.com/docs/en/statusline).
    const SAMPLE: &str = r#"{
        "session_id": "abc", "transcript_path": "/t/abc.jsonl",
        "model": {"id": "claude-opus-5-5", "display_name": "Opus"},
        "context_window": {
            "total_input_tokens": 15500, "context_window_size": 200000,
            "used_percentage": 8, "remaining_percentage": 92
        },
        "rate_limits": {
            "five_hour": {"used_percentage": 23.5, "resets_at": 1738425600},
            "seven_day": {"used_percentage": 41.2, "resets_at": 1738857600}
        },
        "novel": {"field": true}
    }"#;

    #[test]
    fn parses_the_documented_sample() {
        let r = parse_report(SAMPLE);
        assert_eq!(r.model.as_deref(), Some("Opus"));
        assert_eq!(r.context_pct, Some(8.0));
        assert_eq!(r.context_window, Some(200_000));
        assert_eq!(
            r.five_hour,
            Some(RateWindow {
                used_pct: 23.5,
                resets_at: 1_738_425_600
            })
        );
        assert_eq!(r.seven_day.unwrap().used_pct, 41.2);
    }

    #[test]
    fn parsing_tolerates_nulls_absences_and_junk() {
        let r = parse_report(
            r#"{"model":{"id":"claude-x"},"context_window":{"used_percentage":null},
                "rate_limits":{"five_hour":{"used_percentage":"lots"},
                               "seven_day":{"used_percentage":12}}}"#,
        );
        assert_eq!(r.model.as_deref(), Some("claude-x"), "id stands in");
        assert_eq!(r.context_pct, None);
        assert_eq!(r.five_hour, None, "a non-number is no reading");
        assert_eq!(r.seven_day, None, "no reset time, no window");
        assert_eq!(parse_report("not json"), StatusReport::default());
        assert_eq!(parse_report("[1,2]"), StatusReport::default());
    }

    #[test]
    fn status_line_reads_as_one_compact_line() {
        let five = RateWindow {
            used_pct: 23.4,
            resets_at: 1_738_425_600,
        };
        let seven = RateWindow {
            used_pct: 41.2,
            resets_at: 1_738_857_600,
        };
        let at = orbit_core::clock::local_hh_mm(five.resets_at);
        assert_eq!(
            status_line(Some("Opus"), Some(42.0), Some(five), Some(seven)),
            format!("Opus · ctx 42% · 5h 23% ↻{at} · wk 41%")
        );
        assert_eq!(status_line(Some("Opus"), None, None, None), "Opus");
        assert_eq!(status_line(None, None, None, None), "");
    }

    #[test]
    fn limits_are_the_accounts_latest_and_expire_at_their_reset() {
        let board = UsageBoard::default();
        let mut rx = board.subscribe();
        let a = AgentId("a".into());
        let b = AgentId("b".into());
        let now = 1_738_420_000;
        board.record(&a, parse_report(SAMPLE), now);
        match rx.try_recv().unwrap() {
            ServerEvent::Usage {
                agent,
                context_pct,
                five_hour,
                ..
            } => {
                assert_eq!(agent, a);
                assert_eq!(context_pct, Some(8.0));
                assert_eq!(five_hour.unwrap().used_pct, 23.5);
            }
            other => panic!("unexpected {other:?}"),
        }

        // B reports no limits (an API-key session, or before its first
        // response): A's stand, and B's own line shows them.
        let line = board.record(
            &b,
            parse_report(r#"{"context_window":{"used_percentage":90}}"#),
            now,
        );
        assert!(line.starts_with("ctx 90% · 5h "), "{line}");
        assert!(line.ends_with("wk 41%"), "{line}");

        // Every reporting agent is in the replay a new subscriber gets.
        assert_eq!(board.snapshot(now).len(), 2);

        // Past the 5-hour reset, that window is gone; the weekly stays.
        let later = 1_738_425_601;
        match &board.snapshot(later)[0] {
            ServerEvent::Usage {
                five_hour,
                seven_day,
                ..
            } => {
                assert!(five_hour.is_none());
                assert!(seven_day.is_some());
            }
            other => panic!("unexpected {other:?}"),
        }
        assert_eq!(board.model(&a).as_deref(), Some("Opus"));
    }
}
