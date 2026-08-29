# Unwatched Finishes Count On The Project And Worktree Rows — 2026-08-26

**Asked:** "i neeed a way to track when a session goes from yellow to green, it should put a counter in
the projects, worktrees row so I know how many terminals I need to check, as I navigate down into rows it
should decrement and eventually hide the notification counts"

**Did:** New `Agent::unseen` flag (`crates/nebula-core/src/entities.rs`), owned by the daemon.
`Store::set_agent_status` (`crates/nebula-daemon/src/store.rs`) now returns `(stamp, unseen)` and keeps
the flag in the same `UPDATE`: running/needs_feedback → finished raises it, staying finished keeps it,
leaving finished clears it, archived rows never raise it and `set_agent_archived` clears it; migration 19
adds the column. `ServerEvent::StatusChanged` carries `unseen`; new fire-and-forget
`ClientRequest::MarkAgentSeen { id }` → `Daemon::mark_agent_seen` (`registry.rs`) broadcasts the agent
upsert only when the flag actually flipped. `PROTOCOL_VERSION` 24 → 25. TUI:
`event_loop.rs::mark_agent_seen` runs from `attach()` (every path that lands the pane on a session goes
through it — cursor walk, restore, palette, snapshot re-attach) and from the `StatusChanged` arm when the
flip is for the session already in the pane; `app.rs::worktree_unseen` / `project_unseen` count;
`ui.rs::row_badges` draws ` n done` (`th.done`) on project and worktree rows (it also carried a note badge
until notes were removed 2026-08-26), and a
session row swaps its ` claude` harness badge for ` done` (the link row's unread-count idiom). The badge
read ` n new` in `th.ok` until 2026-08-27 — see [Done Reads Violet And Says "done"] for the rename and
for the dot splitting violet (unread) from green (read) on the same `unseen` flag this entry added. README
"Status dots" documents it. Tests: store `unseen_follows_the_status_and_clears_on_seen`; registry
`status_broadcast_carries_the_unseen_flag`, `mark_agent_seen_broadcasts_only_a_flip`; event_loop
`an_unwatched_finish_counts_until_the_cursor_lands_on_it`, `a_finish_in_the_pane_on_screen_is_already_seen`,
`unwatched_finishes_badge_the_rows_until_read`.

**Gotchas:**
- Daemon-side on purpose: the counter's whole point is turns that finished while no TUI was open, which a
  TUI-side set (even persisted in `UiState`) can never see — and two clients would clobber one blob.
  `pr_seen` / `MarkPrSeen` was the template.
- The rule lives in the SQL `CASE` of `set_agent_status`, not in `AgentStatusMachine`: the machine dedups
  unchanged statuses (`set_status` only emits on change), and the flag has to be atomic with the status
  it qualifies. Red → green counts (NeedsFeedback → Finished happens directly on a Stop / `idle_prompt`
  after a prompt); Fresh → Finished does not — a Stop nebula never saw the prompt for is not a
  yellow-to-green.
- Adding a field to `Agent` touches ~15 struct literals across four crates. The perl pattern
  `archived_at: 0,\n\s+pinned: false,` catches all but the test helpers that pass `pinned` as a
  variable and the one literal with `pinned: true` — let the compiler list the rest.
- `e2e_pty::workspace_scope_is_per_connection` failed 2 of 2 full-suite runs made while a
  `cargo build --release` ran alongside, then passed alone and 2 of 2 idle full-suite runs. Its
  `expect("AddProject upserts the project")` only sees the broadcast upsert if it lands before the Ack —
  `server.rs` writes the Ack from the request loop and the upsert from the broadcast forwarder, so under
  CPU contention the Ack can win. **Fixed 2026-08-27 (`6638952`):** the test now waits for the upsert as
  well as the Ack, like `cli_add_project` always did — see [A Paused Rebase Renamed The Worktree Row].
