# The QUICK PROMPT's New WORKTREE And Its SESSION Show Up Before The DAEMON Answers — 2026-09-06

**Asked:** "when I do a quick prompt to create a new worktree, add optimistic updates so it feels
instance, right now there is a few second delay before the worktree or session even shows up in the
list, it would be nice if they showed up instantly and hooked into the real directories or sessions
later after they actually get created.  on failure, just undo the optimstic updates."
→ refined: When I press `p` on the WORKTREES PANEL with `hide_root_worktree` on and submit the QUICK
PROMPT, show the new WORKTREE row and its SESSION row instantly, before the DAEMON's `CreateWorktree` /
`CreateAgent` Acks: placeholder rows (the BRANCH NAME GENERATOR name, the default `agent-N` name,
visibly pending) selected exactly as the real rows would be, then swapped for the real rows when the
Acks and upserts land, without flicker. On an Error from either request remove both placeholders and
keep today's behavior of reopening the box with my text. (Assuming only this QUICK PROMPT path, and the
TERMINAL PANE never attaches to a placeholder.)

**Did:** (`p` on the WORKTREES PANEL under the HIDE ROOT WORKTREE SETTING.) New `crates/nebula-tui/src/event_loop/placeholder.rs`: `stage` pushes a `Worktree` (empty
`path`) and an `Agent` (named by `default_session_name`, stamped `now_ms()`) into `app.tree`, selects
both with FOCUS kept, and builds the pane's `AttachedTerm` by hand so it reads "starting session…";
`resolve_worktree` / `resolve_agent` take the Ack's id — upsert-first drops the stand-in, Ack-first
renames it in place so the upsert overwrites by id — re-home the session and rekey
`last_session_for_worktree` / `last_worktree_for_project`; `discard` / `discard_agent` take the rows
down on Error, `restore_context` putting the cursor back on the pre-`p` row when it is still on the
stand-in, `reconcile_selection` otherwise. `app.rs`: `PlaceholderRows`, carried by
`PendingIntent::LaunchInCreatedWorktree { placeholder }` and `AttachCreatedWithCloudRetry { placeholder:
Option<AgentId> }` (via `AgentLaunchDraft.placeholder`); `PendingIntent::placeholder_worktree/agent`
and `App::is_placeholder_worktree/agent/session`, `pane_shows_placeholder` scan `app.pending` — the
in-flight intent is the one record, nothing to keep in sync. `event_loop.rs`: the Ack arm's attach
body became `attach_created(..)` (resolves the stand-in first), the Error arm's reopen became
`reopen_prompt_with`; `create_agent` takes the name off the stand-in and refuses a stand-in worktree
(`WORKTREE_STILL_CREATING`, also `create_terminal`, `d`, the CONTEXT MENU delete,
`quick_prompt::open_quick_prompt`); `send_attach`, `release_attachment`, `fire_pending_prewarm`,
`fire_keepwarm` and the three pane `Input` sites skip stand-ins; `quick_launch.rs` stages before the
`CreateWorktree` and re-arms `schedule_prewarm` after the Ack. `ui.rs`: `WorktreeRowData` gained
`pending`; a stand-in draws the hollow `○` dot, no sweep, ` creating` in the ago slot / ` starting` in
the harness slot, no ago label (`PENDING_WORKTREE_BADGE` / `PENDING_SESSION_BADGE`). Six tests in
`placeholder.rs::tests` (the `event_loop::tests` helpers became `pub(super)`); gate: nebula-tui 613
passed, `cargo clippy --workspace --all-targets -- -D warnings` clean, `cargo test --workspace` green.

**Gotchas:**
- The stand-in's only mark is its PENDING INTENT, which `send_with` allocates *after* the staging code
  runs and the `Ack` arm *removes* before running the intent — in both windows `App::is_placeholder_*`
  says "real". So `stage` builds the pane's `AttachedTerm` itself instead of `attach_now` (the
  `send_attach` guard could not fire yet and an `Attach` for a made-up id went out), and `create_agent`
  takes the name off the stand-in row, because `default_session_name` counted it as taken in that window
  and named the real create `agent-2`.
- `release_attachment` falls back to `term.sref` when nothing is attached (the debounced-attach case),
  so leaving a stand-in pane pushed a `Detach` for an id the DAEMON never had — filtered out; the
  existing `p_on_the_worktrees_panel_cuts_a_fresh_worktree_first_when_the_root_is_hidden` test caught
  it through its exact `out` match.
- The DAEMON stamps a created agent `status_changed_at: epoch_ms()` (`registry.rs::create_agent`) and
  broadcasts it `alive: true` before the Ack: a stand-in stamped 0 would sit at the bottom of RECENCY
  ORDER and jump to the top on resolve, so it is stamped `now_ms()`. Flip side: a real worktree whose
  stand-in session was refused has no stamp and drops below never-run peers — the cursor follows it by
  id.
- `fit_ago` drops a badge when the name would keep fewer than `MIN_NAME_W` (8) columns, so ` creating`
  survives beside a truncated three-word branch (`eager-p… creating`) — a screen assertion has to look
  for the badge, not `"{branch} creating"`; and a stamped stand-in session read `agent-1 just now
  starting` until the ago label was suppressed for pending rows.
- `event_loop::tests`' helpers (`seed_tree`, `seed_feat_worktree`, `press`, `hse`, `with_default_config`,
  `buffer_text`, `worktree_branches`) are private to that module: a child module's tests
  (`event_loop::placeholder::tests`) only see them as `pub(super)`.
- The main loop drains every queued `ServerEvent` (`while let Ok(ev) = channels.rx.try_recv()`) before a
  draw, so the DAEMON's upsert + Ack pair resolves within one frame — resolution keys on the Ack's id
  alone, no (project, branch) matching against the upsert needed.
- A transient screen cannot be shot on the SCREENSHOT HARNESS as is: its demo repo has no origin, so
  the DAEMON's `git worktree add` closes the stand-in window in milliseconds. `scripts/shot/bin/git`
  is now the real git plus a `NEBULA_SHOT_SLOW_GIT_SECS` sleep before `worktree add`; the
  `quick-prompt-stand-in` scene runs with it set to 6 (capture lands 1.6 s after the last key).
