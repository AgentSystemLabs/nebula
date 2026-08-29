# Lists Order Themselves: MOVE PROJECT Removed, PROJECTS And WORKTREES Sort By Last Interaction — 2026-08-28

**Asked:** "remove the ability to reorder the session, worktrees, or projects list rows. each should always
just move recent to top with a time last updated timestamp like we do on sessions row"
→ refined: Remove MOVE PROJECT (`Shift+J`/`Shift+K`, `Shift+↑/↓`) end to end — the `Action`s, the keymap
rows, `ClientRequest::MoveProject`, `Daemon::move_project`, the README/help rows. (The SESSIONS PANEL and
WORKTREES PANEL never had a reorder key, so nothing to remove there.) Instead, the PROJECTS PANEL and
WORKTREES PANEL sort like the SESSIONS PANEL already does: most recently interacted first, with the same
dimmed `23m ago` label to the right of the name. (Assuming a WORKTREE's stamp is the newest last-interaction
of its SESSIONS — a RUNNING one counts as now — and a PROJECT's is the newest of its WORKTREES; rows with no
stamp keep their current order at the bottom, no label.) Keep the PINNED / UNPINNED groups and the RECENT
WINDOW exactly as they are; the cursor must follow its row across a re-sort. (No questions asked.)

**Did:** MOVE PROJECT is gone: `Action::MoveProjectUp/Down` and their `ActionSpec`s (`keymap.rs`), the
handler arms and `move_project()` (`event_loop.rs`), `ClientRequest::MoveProject` (`protocol.rs`, PROTOCOL
VERSION 30 → 31), the `server.rs` arm, `Daemon::move_project` + its two tests (`registry.rs`),
`Store::set_project_position` (`store.rs`), the help-overlay row and the `{}/{}: move` FOOTER hint (`ui.rs`),
the README key row. The `sort_order` column and `Project.sort_order` stay — they're still the insertion
order and the tiebreak; not worth a migration. New in `app.rs`: `Recency { interacted, stamped }` with
`worktree_recency()` / `project_recency()` (free-standing, mirroring `worktree_rollup`), `interacted` being
the newest `last_interaction_ms` (RUNNING = now) and `stamped` the newest raw `status_changed_at` the "23m
ago" label reads. `project_rows()` sorts by it (stable, so never-run rows keep tree order at the bottom);
`visible_worktrees()` sorts each of PINNED / UNPINNED by it, so the ROOT WORKTREE moves like any row. `ui.rs`
grew `fit_ago()` (the sessions rule — the label drops before the name goes under `MIN_NAME_W`, renamed from
`MIN_SESSION_NAME_W`) used by all three panels; project rows render `name 23m ago  badges`, worktree rows
`branch ⌂ root 23m ago  badges`. The `ServerEvent::StatusChanged` handler now takes a `selection_snapshot`
and runs `reconcile_selection_inner` so all three cursors follow their rows across the re-sort, not just the
session one; the AGO tick condition became "any agent in the tree has a stamp". README gained a "Lists
that order themselves" bullet. Tests: `shifted_keys_no_longer_reorder_projects`,
`projects_sort_by_last_interaction_and_selection_follows`,
`worktrees_sort_by_last_interaction_within_their_group`,
`project_and_worktree_rows_show_time_since_last_interaction`. Workspace green: 497 TUI / 156 daemon /
25 e2e_pty / 6 e2e_tui, clippy + fmt clean.

**Gotchas:**
- **The default WORKTREES PANEL is 22 columns (`DEFAULT_PANEL_WIDTHS[1]`), and `main ⌂ root 23m ago` does
  not fit in it.** First cut dropped the whole root badge whenever the root had a stamp — i.e. always, in
  the default layout. Fix: the badge degrades to its bare glyph (`ROOT_GLYPH = " ⌂"`) before it goes, so
  the default width shows `main ⌂ 23m ago`; a 34-column panel gets the word back. Yield order on a
  worktree row is now: ago label (if the name would drop under 8 columns), then the word "root", then the
  glyph. `unwatched_finishes_badge_the_rows_until_read` asserted the old `main 1 done` and now expects
  `main ⌂ 1 done` (there the " just now" label is what drops).
- Archived sessions **count** toward a worktree's / project's stamp — archiving is housekeeping, not
  activity — so a worktree whose only session is archived still says "5m ago" over an empty SESSIONS
  PANEL. Deliberate; revisit if it reads wrong.
- `SessionRow::last_interaction_at()` became dead once the AGO tick stopped looking at visible session
  rows; it was removed rather than left as a warning.
- Shift+J/K/↑/↓ are now unbound in every panel (they used to fall through to `move_selection` outside the
  PROJECTS PANEL). `shift+up`/`shift+down` are free again for whoever wanted them for PR PREVIEW scrolling
  (see the 2026-08-27 PR preview entry).
- Another session was editing `status.rs`, `hooks/mod.rs`, `config.rs` and `event_loop.rs` concurrently
  (harness toggles / subagent orphaning): a `subagent_id` compile error, a `NoticeLevel` import error and
  fmt diffs in `event_loop.rs` all showed up mid-task and were theirs, not this change's. Same SHARED
  CHECKOUT race as before — `git status` before believing a red build.
