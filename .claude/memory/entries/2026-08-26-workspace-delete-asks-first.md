# Workspace Delete Asks First — 2026-08-26

**Asked:** "make sure deleting a workspace shows a confirmation"

**Did:** All three no-confirm paths — `d` in the WORKSPACES COLUMN, "Delete workspace" in its CONTEXT MENU (`m`), and
`d` in the WORKSPACE SWITCHER (`w`) — now go through the new `open_remove_workspace_confirm`
(`crates/nebula-tui/src/event_loop.rs`), which opens a CONFIRM DIALOG (`ConfirmDialog`) with the new
`PendingAction::RemoveWorkspace { id, reopen_picker: Option<usize> }` (`app.rs`). `run_pending_action`
sends the `RemoveWorkspace` request on `y`; the daemon is still the guard (empty workspaces only, never
the last one), so a refusal after confirming just flashes as before. README rows for `w` and the
WORKSPACES COLUMN say "delete asks first". Tests `workspaces_column_verbs_act_on_the_open_workspace` and
`switcher_r_and_d_act_on_the_hovered_workspace` cover Esc (nothing sent) and `y` on both paths.

**Gotchas:**
- **A CONFIRM DIALOG replaces the OVERLAY it came from, so the WORKSPACE SWITCHER's `d` needs a way home.** The old `d`
  left the `w` menu up and let the `EntityRemoved` delta drop the row in place. The confirm is
  `app.overlay`, which evicts the menu, so `reopen_picker` carries the switcher's hover row and both
  answers reopen it there (`reopen_workspace_picker`, also now used by `refresh_workspace_picker`) —
  the `ResetSettings` reopen-the-SETTINGS OVERLAY precedent. The Confirm key handler's Esc/`n` arm is
  where that routing lives; a confirm opened from the column passes `None` and closes to the panels.
- **The CONFIRM DIALOG is sized to its longest message line** (`ui.rs` `Overlay::Confirm`, min 52
  columns, no wrapping), so a single ~85-column sentence outgrows a narrow terminal. Put a `\n` in
  long messages; the bulk-delete confirms already do.
- The switcher test renames `ws2` to `client` before it presses `d`, so asserting the dialog message
  against `'ws2'` fails — match the current name (or the id in the action), not the seed name.
