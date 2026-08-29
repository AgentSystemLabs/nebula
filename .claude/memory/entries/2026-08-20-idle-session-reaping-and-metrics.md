# Idle Session Reaping And Metrics — 2026-08-20

**Asked:** "right now when a user opens a session, it takes some time I think for nebula to connect maybe
to the server and actually show the terminal... can we find a way to prefetch these connections…" → "add
logic to auto suspend or kill claude sessions that are not in focus…" → then the user pushed back on their
own idea: "I'm concerned now because some claude sessions might have schedules or long running jobs and I
don't want them killed.... is the latest change potentially breaking that requirement?" → "ok for now
never reap pinned sessions, also make this entire reap process a setting configurstion to just turn it
off." Alongside: "add some type of metrics modal which will show the overal usage of nebula combined with
all the other terminals open, including memory usage for individual and overall."

**Did:** `e11f838` — idle reaping, metrics tracking, memory stats in the footer.

**Gotchas:**
- **Pinned sessions are never reaped**, and reaping is switchable off entirely. That constraint came from
  the user realizing mid-feature that agents may be running long jobs — treat it as load-bearing.
  *(Superseded 2026-08-28: PIN was removed outright, and its reap exemption with it; the off switch is the
  remaining protection — see `2026-08-28-pin-and-the-recent-group-are-gone-one-flat-list-in-recency`.)*
