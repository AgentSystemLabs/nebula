# Background Subagents Turned The Session Green: IDLE PROMPT Now Holds The STOP GATE — 2026-08-28

**Asked:** "when claude spins up sub agents in the session, the session turns green but that is incorrect
because our requirements state even subagents off that main session should keep the status yellow for
working"
→ refined: When a Claude AGENT spawns subagents (the Agent tool) mid-turn, the SESSION's STATUS DOT flips
to green (FINISHED) while those subagents are still working. The STOP GATE is supposed to hold the AGENT
at RUNNING until every subagent of that turn has stopped. Find why the gate is bypassed and fix it without
breaking the 180 s drain grace, IDLE PROMPT and PROGRESS SCANNER rescues.

**Did:** Root cause was the IDLE PROMPT, not the STOP GATE. Since Claude Code 2.1 the Agent tool runs
subagents in the *background*: the foreground turn ends (Stop — correctly held, subagents tracked), the
input box comes back, and ~60 s later Claude fires `Notification{idle_prompt}`, which `mark_idle` in
`crates/nebula-daemon/src/status.rs` treated as "anything still tracked is orphaned" — it cleared the set
and went FINISHED. Now `mark_idle(now, …)` prunes and, if subagents remain, calls the new
`hold_for_subagents` (stop_held, RUNNING) exactly like a gated Stop. Safety net so a killed worker cannot
wedge the row on yellow until `SUBAGENT_TTL` (2 h): new `SUBAGENT_QUIET_GRACE` (30 min) and
`subagent_alive_at` — refreshed by SubagentStart/Stop and by subagent tool traffic (`PreToolUse` /
`PostToolUse` gained `subagent_id`, filled from the payload's `agent_id` in `hooks/mod.rs::parse_event`) —
after which `tick` presumes the set orphaned and finishes with no `finished_at` (no heal). Tests:
`idle_notification_holds_running_while_background_subagents_work`,
`idle_hold_drains_through_the_grace_when_nothing_reinvokes_the_turn`,
`idle_notification_with_a_rejected_prompt_still_holds_for_subagents`,
`quiet_subagents_are_presumed_orphaned_after_the_quiet_grace`, `subagent_traffic_resets_the_quiet_clock`
(replacing `idle_notification_drops_held_stop_and_orphaned_subagents`). 156 daemon tests green; the live
DEV INSTANCE daemon still runs the old build (MAKE CYCLE is the user's).

**Gotchas:**
- **Measured, not guessed, against Claude Code 2.1.251 from inside a nebula SESSION**: a 1 s sqlite
  poller on my own `agents` row (`NEBULA_DATA_DIR=~/.nebula-dev/<slot>/nebula.db`, column is
  `claude_session_id`) while spawning haiku subagents that spin for N s. Spawn + end turn → RUNNING held;
  FINISHED at exactly +60 s with the worker still 35 s from done = the idle_prompt. A normal completion
  *does* deliver SubagentStop (Stop after it → FINISHED at once). **`TaskStop`-killing a subagent delivers
  no SubagentStop at all** — that orphan is what the 30 min quiet grace is for; under the old code it
  went green at +60 s, now it stays yellow ≤30 min. Worth knowing before "stuck on yellow" gets reported.
- The daemon log records no hook traffic at `info` (`hook received` is `debug`, and `hookEvent=` never
  appears in `daemon.log`), so the store row is the only cheap live oracle for status timing.
- When a background subagent finishes, Claude re-invokes the main turn (task-notification) — that is
  `Progress{busy:true}` (a no-op while held) followed by a real Stop, so the drain grace is rarely what
  finishes the row; the second Stop does, against an empty set.
- Subagent tool traffic reaches the STATUS MACHINE only for the hooked tools (`PostToolUse` matcher
  `Bash|EnterWorktree|ExitWorktree`, `Pre/PostToolUse` `AskUserQuestion`): a Read/Grep-only worker shows
  life only at its start and stop, so the quiet clock must stay generous.
- The quiet clock counts from the last sign of life, not from the Stop — a test written "from the hold"
  is off by the gap between SubagentStart and Stop.
- `cargo test -p nebula-tui` fails on `project_and_worktree_rows_show_time_since_last_interaction` —
  that test is in another session's uncommitted `event_loop.rs` diff, not this change (SHARED CHECKOUT).
