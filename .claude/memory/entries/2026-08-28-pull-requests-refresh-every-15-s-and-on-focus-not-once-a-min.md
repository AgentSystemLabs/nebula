# Pull Requests Refresh Every 15 s And On Focus, Not Once A Minute — 2026-08-28

**Asked:** "pull latest from main, then debug the refresh rate for the pull requests, sometimes I'll close
one on github and it takes a while for the ui to refresh. lookup github rate limits to know how often we
can fetch the pull requests to refresh, or just refresh on focus of the worktrees and sessions in the
background so we often see fresh data"
→ refined: find out why a closed PR lingers in the PROJECT OPEN PRS GROUP / PR ROW, look up GitHub's
limits and pick the fastest safe cadence, and re-fetch when FOCUS lands on the WORKTREES PANEL or
SESSIONS PANEL; keep the cursor reconcile, `None`-keeps-last-list and the PR PREVIEW scroll as they are.
(no questions asked)

**Did:** Nothing was broken — the GIT POLL simply re-asked once a minute (`OPEN_PRS_REFRESH` /
`PR_REFRESH` 60 s), so a PR closed on GitHub sat on screen up to ~62 s. Both beats are now **15 s** in
`crates/nebula-tui/src/event_loop.rs`, `app.rs::OPEN_PRS_MIN_AGE` 30 s → **5 s**, and a new
`schedule_pull_request_refresh` (list + PR ROW) runs from two gestures: `note_focus_change` at the
loop's `focus_before` diff when FOCUS lands on `Focus::Worktrees | Focus::Sessions`, and crossterm
`Event::FocusGained` in `handle_terminal_event` (`EnableFocusChange` / `DisableFocusChange` added to
`setup_terminal` / `restore_terminal`) — the terminal window coming back from the browser is the moment
the close most likely happened. README + TERMS cadence wording updated. 2 new tests; nebula-tui 489
green, clippy and fmt clean; `make install` done.

**Gotchas:**
- **Rate limits were never the bound.** `GH_DEBUG=api gh pr list …` shows one `POST /graphql`, and the
  GraphQL docs price a query at max(1, nodes/100) points against **5,000 points/hour** per user token
  (2,000/min secondary; REST is the same 5,000/hr). 15 s on two calls is 480/hr, under a tenth. The real
  reason not to go faster is that the Claude SESSIONS on this box shell out to the same `gh` token.
- **`OPEN_PRS_MIN_AGE` must sit well below the beat**, or an on-focus refresh can never fire: at the old
  30 s floor with a 15 s beat, `schedule_open_prs_lookup`'s `due.min(at + MIN_AGE)` is always later than
  the timer already armed. Any future tightening of `OPEN_PRS_REFRESH` has to revisit the floor.
- The loop's `if app.focus != focus_before` at the end of each iteration is the one choke point for
  "focus changed" — there are ~135 `app.focus = Focus::…` assignments; hook there, not per assignment.
- `schedule_pr_detail` still zeroes `pr_preview_scroll`; the focus paths deliberately don't call it. A
  closed PR under the cursor is still retired by the list reconcile (`reconcile_open_pr_cursor`).
- The change is TUI-only (no PROTOCOL VERSION bump), so `make install`'s STALE DAEMON NOTE is generic here:
  relaunching the `nebula` TUI picks it up, `nebula kill` isn't needed.
