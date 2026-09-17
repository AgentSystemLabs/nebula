//! FRAME PACING: when the loop may paint again.
//!
//! A flat "one frame per 16 ms" cap is right for a PTY streaming output —
//! bursts coalesce, the CPU stays idle — and wrong for a keypress. The key
//! paints its own frame at once, and the thing it asked for (a PTY echo,
//! the DAEMON's Ack, a replay) lands two to five milliseconds later, just
//! inside that frame's 16 ms shadow: measured with the INPUT LATENCY PROBE,
//! a typed character took 23 ms to show in a locked pane, 20 of them spent
//! waiting out the cap, and a rename, an archive and a new terminal each
//! painted their answer a whole interval late the same way.
//!
//! So the cap is a token bucket rather than a metronome: [`BURST`] frames
//! may follow each other [`MIN_GAP`] apart, and a token comes back every
//! [`FRAME_INTERVAL`]. An idle app — which is what a keypress finds —
//! has a full bucket, so the key's frame and its answer's frame go out
//! back to back; sustained output drains the bucket and paints at exactly
//! the old 60 fps, so the streaming cost is what it was plus `BURST - 1`
//! frames at the head of each burst.

use std::time::Duration;
use tokio::time::Instant;

/// Steady-state redraw cap (~60 fps): how often a spent token comes back.
pub(super) const FRAME_INTERVAL: Duration = Duration::from_millis(16);

/// Frames an idle loop may paint back to back: the key's own, its
/// answer's, and one for an answer that arrives in two parts (an Ack, then
/// the replay it set off).
const BURST: u32 = 3;

/// The least time between two frames of a burst — long enough that the
/// events of one answer (Ack, upsert and Scrollback leave the DAEMON as
/// three writes) are all in hand before the frame that shows them, short
/// enough to be invisible.
const MIN_GAP: Duration = Duration::from_millis(2);

pub(super) struct FramePacer {
    tokens: u32,
    /// When the token being earned started accruing.
    since: Instant,
}

impl FramePacer {
    pub(super) fn new(now: Instant) -> Self {
        Self {
            tokens: BURST,
            since: now,
        }
    }

    fn refill(&mut self, now: Instant) {
        if self.tokens >= BURST {
            self.since = now;
            return;
        }
        let earned = (now.saturating_duration_since(self.since).as_micros()
            / FRAME_INTERVAL.as_micros()) as u32;
        if earned == 0 {
            return;
        }
        self.tokens = (self.tokens + earned).min(BURST);
        self.since = if self.tokens >= BURST {
            now
        } else {
            self.since + FRAME_INTERVAL * earned
        };
    }

    /// A frame was just painted, its draw taking `took` and finishing at
    /// `now`: when the next may be. Never sooner than the draw itself took,
    /// so a frame that is slow to paint (a huge window, an unoptimised
    /// build) leaves the loop at least half its time for everything else.
    pub(super) fn drew(&mut self, now: Instant, took: Duration) -> Instant {
        self.refill(now);
        self.tokens = self.tokens.saturating_sub(1);
        let gap = now + took.max(MIN_GAP);
        if self.tokens > 0 {
            gap
        } else {
            gap.max(self.since + FRAME_INTERVAL)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    /// The case the bucket exists for: a key's frame, then its answer's a
    /// few milliseconds later, with no 16 ms wait between them.
    #[test]
    fn an_idle_loop_paints_a_key_and_its_answer_back_to_back() {
        let t0 = Instant::now();
        let mut pacer = FramePacer::new(t0);
        let next = pacer.drew(t0, ms(1));
        assert_eq!(next, t0 + MIN_GAP, "the answer's frame waits only the gap");
        let next = pacer.drew(t0 + ms(4), ms(1));
        assert_eq!(
            next,
            t0 + ms(4) + MIN_GAP,
            "and so does a two-part answer's"
        );
    }

    /// Sustained output must cost what it always has: once the burst is
    /// spent, frames are a full interval apart.
    #[test]
    fn sustained_output_settles_at_the_frame_interval() {
        let t0 = Instant::now();
        let mut pacer = FramePacer::new(t0);
        let mut now = t0;
        let mut painted = Vec::new();
        // Always dirty: paint the moment the pacer allows.
        for _ in 0..40 {
            painted.push(now);
            now = pacer.drew(now, ms(1));
        }
        let tail: Vec<Duration> = painted[BURST as usize..]
            .windows(2)
            .map(|w| w[1] - w[0])
            .collect();
        assert!(
            tail.iter().all(|gap| *gap == FRAME_INTERVAL),
            "after the burst every frame is one interval apart: {tail:?}"
        );
        let span = *painted.last().unwrap() - t0;
        let budget = FRAME_INTERVAL * (painted.len() as u32 - BURST);
        assert!(
            span >= budget,
            "40 frames took {span:?}, under the {budget:?} a 60 fps cap allows"
        );
    }

    /// The bucket refills while nothing paints, so the next keypress after
    /// a streaming burst gets its back-to-back frames again.
    #[test]
    fn a_quiet_spell_refills_the_burst() {
        let t0 = Instant::now();
        let mut pacer = FramePacer::new(t0);
        let mut now = t0;
        for _ in 0..10 {
            now = pacer.drew(now, ms(1));
        }
        let later = now + FRAME_INTERVAL * BURST;
        assert_eq!(pacer.drew(later, ms(1)), later + MIN_GAP);
        assert_eq!(pacer.drew(later + ms(3), ms(1)), later + ms(3) + MIN_GAP);
    }

    /// A frame that comes late does not reset the clock on the token being
    /// earned: the cadence stays on the interval's grid, so output that
    /// stutters still averages 60 fps rather than drifting under it.
    #[test]
    fn a_partly_earned_token_keeps_accruing() {
        let t0 = Instant::now();
        let mut pacer = FramePacer::new(t0);
        let mut now = t0;
        for _ in 0..BURST {
            now = pacer.drew(now, ms(1));
        }
        assert_eq!(
            now,
            t0 + FRAME_INTERVAL,
            "drained: the next token's arrival"
        );
        // Painted 4 ms after it was allowed to.
        let next = pacer.drew(now + ms(4), ms(1));
        assert_eq!(next, t0 + FRAME_INTERVAL * 2, "the 4 ms still count");
    }

    /// A draw slower than the gap sets the gap: the loop is never painting
    /// more than half the time.
    #[test]
    fn a_slow_draw_is_followed_by_as_long_a_pause() {
        let t0 = Instant::now();
        let mut pacer = FramePacer::new(t0);
        assert_eq!(pacer.drew(t0 + ms(30), ms(30)), t0 + ms(60));
    }
}
