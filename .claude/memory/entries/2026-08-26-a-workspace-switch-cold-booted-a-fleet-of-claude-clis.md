# A Workspace Switch Cold-Booted A Fleet Of Claude CLIs — 2026-08-26

**Asked:** "in a worktree, debug a performance issue when a user switches between workspaces sometimes it
seems like it's lagging or stuck loading up multiple claude sessions as the terminal panel doesn't show for
like 5-10 seconds"

**Did:** Three compounding causes, all confirmed against the live `~/.nebula-dev` daemon log and DB, fixed
in `327757f` (originally `7246a47`; the branch sat unmerged for a day and was rebased onto `origin/main`
at v0.13.0 on 2026-08-27 — see the rebase notes at the end of this entry).

1. **Every switch attaches a *dead* session.** `session_idle_timeout` defaults to `5m`
   (`nebula-daemon/src/config.rs:44`), and `reap_idle_sessions` kills everything in a workspace nobody is
   attached to — the dev `daemon.log` is wall-to-wall `reaping idle session … idle_secs=300`. So
   `switch_workspace_inner` → `restore_context` → `restore_session` → `attach` lands on a dead sref and
   the daemon's `Attach` arm cold-spawns `zsh -l -i -c 'exec claude --resume <sid>'`. The replay it sends
   back is **empty** (fresh ring), so the pane rendered a blank vt100 grid with no indication of anything
   happening.
2. **250ms later the prewarm booted every *other* session in the worktree, inline.**
   `PrewarmWorktreeSessions` was handled synchronously on the connection's request loop
   (`server.rs:426`, the old comment said "Deliberately inline"), and `prewarm_worktree_sessions` called
   `ensure_session` for every dead non-archived row. `main` in the dev DB has **5 agents** → 5 concurrent
   login-shell + claude boots, plus a 6th from `PrewarmAgent`, all starving the one the user was waiting
   on — and stalling that client's `Input` frames for the whole burst.
3. **`attach` had no debounce**, unlike prewarm. In the Workspaces column `move_selection` runs a full
   `switch_workspace` per row (`event_loop.rs`), so walking past four workspaces cold-spawned four CLIs
   and abandoned three, each then living 5 more minutes.

Fixes: `Daemon::spawn_gate` (`registry.rs`) makes `ensure_session`'s check-and-install atomic, which is
what lets the sweep leave the request loop — `run_worktree_prewarm` is now its own task, boots one session
per `PREWARM_STAGGER` (1.5s), skips `is_alive` rows, and `prewarm_sweep` aborts a superseded sweep.
TUI-side, `attach` defers the request by `ATTACH_DEBOUNCE` (180ms) via `pending_attach`, with
`attached_sref` tracking what the daemon actually holds while `term.sref` runs ahead; `attach_now` /
`preview_selected_now` skip the wait for explicit picks. `AttachedTerm::painted` drives a `starting…` tag
plus a centered notice in `draw_terminal`. 631 tests green when written; **657 green after the rebase**
(exit 0, `--no-fail-fast`, all 7 binaries).

**Gotchas:**
- **Measure the boot before blaming the code.** One fresh `claude` under `zsh -l -i` on this machine is
  **0.67s to first byte, 1.47s to a painted screen** — and that's without `--resume` reloading a
  transcript. Six of those at once is the whole 5-10s. A pty-fork bench spawning 4 at once got the shell
  OOM-killed (exit 137) and 3 at once never finished in 120s; benchmark agent CLIs **one at a time**.
- **`app.term` and the daemon's attachment are now two different things.** Tests that set
  `app.term = Some(AttachedTerm::new(…))` directly leave `attached_sref` unset, so a `detach_if_attached`
  keyed only on `attached_sref` silently stopped emitting `Detach` and broke 4 tests. Both
  `detach_if_attached` and `release_attachment` deliberately fall back to `term.sref` — a `Detach` the
  daemon holds no attachment for is a no-op there (`server.rs` `attached.remove()` returns `None`).
- **Debouncing `attach` breaks every test that asserts an immediate `Attach` after a selection move.** 10
  failed. Only 2 were genuinely the new contract (`session_arrows_preview_without_focusing`,
  `switching_contexts_restores_the_remembered_session` — both now call `fire_pending_attach` to settle).
  The other 8 were paths that *should* stay immediate: wrap `reconcile_selection` and `jump_to_target`
  (both have early `return`s, so wrapping beats appending) and route clicks through
  `preview_selected_now`.
- **Input is dropped for a session the daemon hasn't spawned**, so a pending attach must land before the
  first keystroke. `handle_terminal_event` fires it up front when `term_locked`; the two paths that take
  the lock without attaching (`Action::Activate` on an already-focused pane, `Action::Zoom`) fire it too.
- The 100-col draw-test truncation trap from [A Workspaces Column Left Of Projects] bites again: the
  `starting…` test asserts pane body text, so it needs `show_workspaces = false` **and**
  `TestBackend::new(140, 30)` or the string is clipped mid-assert.
- The `switch_workspace_quietly` double-attach gotcha below is unaffected — the debounce happens to mask
  that churn now, but the quiet variant is still what makes a cross-workspace jump correct.

**Rebase onto v0.13.0 (2026-08-27), 20 commits later:**
- Only two files conflicted (`registry.rs`, `event_loop.rs`); `server.rs`, `app.rs` and `ui.rs` merged
  clean. Both `registry.rs` conflicts were additive-on-both-sides (main's `pending_moves` /
  `cloud_attach_gated` / `cloud_mirrors` vs. this branch's `spawn_gate` / `prewarm_sweep`) — keep both.
- **The one semantic conflict is `attach`.** Main added `mark_agent_seen` to the top of it (see
  [Unwatched Finishes Count On The Project And Worktree Rows]) on the reasoning that every path landing
  the pane on a session goes through `attach`. After this branch's split that funnel is `attach_inner`,
  so the call moves there — keyed to the **pane swap, not the Attach**, because the user is reading the
  screen during the debounce just the same. Putting it in `attach` alone would have skipped
  `attach_now` / `preview_selected_now` and leaked unread counts on every explicit pick.
- Two of main's newer tests failed, and they are not the same kind of failure:
  `switching_back_to_a_workspace_restores_project_worktree_and_session` is *exactly* the path the
  debounce exists for, so the test was updated to the new contract (pane restores now, Attach after
  `fire_pending_attach`). `snapshot_reattaches_the_remembered_session` is not — a boot restores one
  remembered session once, with no cursor sweep to wait out, so the Snapshot arm moved to
  `preview_selected_now` and main's assertion stands unchanged. **A failing attach-timing test is a
  question about which path it is, not a licence to relax the assertion.**
- Struct drift in the branch's new test only: `Project` lost the four `divider_*` fields (migration 18)
  and `Agent` gained `unseen` / `cloud_session_id` / `cloud_mirroring`. `cargo build` was clean and only
  `cargo test --no-run` surfaced it — test-only literals need the test compile to be checked.
- The workspaces *column* became a top tab bar on main, but `move_selection` there still does a full
  `switch_workspace` per step, so the premise of cause 3 survived the rework and
  `walking_the_workspaces_column_attaches_only_where_it_stops` passes untouched.
- `cargo fmt --check` and clippy are **dirty on `origin/main` itself** (a `base64_encode` line, 7 clippy
  warnings incl. `needless_return` at `event_loop.rs:5197`). None are in this diff — confirm with
  `git diff origin/main --stat -- <file>` before assuming a warning is yours.
