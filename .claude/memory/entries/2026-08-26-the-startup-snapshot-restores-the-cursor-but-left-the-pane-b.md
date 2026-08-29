# The Startup Snapshot Restores The Cursor But Left The Pane Blank — 2026-08-26

**Asked:** "when nebula first loads, it seems to auto remember my last select pref, but it doesn't seem
to show the focused session terminal"

**Did:** The `ServerEvent::Snapshot` arm in `crates/nebula-tui/src/event_loop.rs` (`handle_server_event`)
called `restore_ui_state` to re-seat `sel_project` / `sel_worktree` / `sel_session` from the persisted
`UiState` blob and then stopped — nothing ever sent the `Attach`. `restore_ui_state` now returns `bool`
("the remembered `session_agent` landed under the cursor") and the Snapshot arm calls `preview_selected`
when it does, so the pane comes back with the row, focus staying on the panels exactly like a cursor
move. No blob, or a blob whose session is gone/archived, still leaves the pane blank. Unit test
`snapshot_reattaches_the_remembered_session`. TUI-only change: reopening the TUI is enough, the daemon
does not need a restart.

**Gotchas:**
- `restore_context` / `restore_session` can't be reused on the startup path: they read
  `last_worktree_for_project` / `last_session_for_worktree`, which `remember_context` only fills as the
  user moves *away* from a context — both maps are empty on the first snapshot, so `restore_session`
  would blank the pane it was asked to restore. The blob's ids are the only memory at boot.
- Snapshot is a one-shot reply to `Subscribe` (event_loop.rs:136); the TUI never resubscribes, so
  attaching there can't double up. If a reconnect path ever re-sends it, `attach`'s already-attached
  early return is what keeps this safe.
