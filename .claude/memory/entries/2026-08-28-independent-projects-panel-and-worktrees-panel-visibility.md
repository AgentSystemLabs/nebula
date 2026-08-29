# Independent PROJECTS PANEL And WORKTREES PANEL Visibility — 2026-08-28

**Asked:** "Lets add a feature to allow us to toggle the PROJECTS column and also to toggle WORKTREES
column separate. Having those toggles avaialble llow us to have more space, thats why we want it.

grill me with docs" - then corrected: "I think instead of show_* in the configuration should be hide_*"

**Did:** Added independent, global panel visibility to the TUI without a protocol, store, or UI STATE
BLOB change. `crates/nebula-tui/src/config.rs::Config` now persists `hide_projects` and
`hide_worktrees` (both default `false`) and exposes both on the SETTINGS OVERLAY's Appearance tab;
`keymap.rs` adds `toggle_projects` on `Shift+P` and `toggle_worktrees` on `Shift+B`. Runtime layout in
`app.rs::visible_panel_indices` and `ui.rs::draw` omits hidden panels, preserves their draggable widths,
and gives the released width to the TERMINAL PANE. The PANEL WALK, ACTIVATE, new PROJECT selection,
new WORKSPACE selection, and PALETTE project picks all skip hidden panels; hiding the focused panel
moves FOCUS right, while restoring never steals it. The FOOTER leads with restore hints while a panel
is hidden. README, CONFIG.JSON coverage, render tests, navigation tests, creation/PALETTE tests, and the
real-PTY E2E TUI flow were updated. `cargo test --workspace`: 715 passed, 0 failed.

**Gotchas:**
- A restore hint appended to the normal FOOTER text is clipped at a 120-column PTY because the focus
  hints and right-side metrics consume the row. Prepending `Shift+P: show projects` and
  `Shift+B: show worktrees` keeps the recovery path visible.
- `App::term_area` records the TERMINAL PANE's inner PTY rect after a one-cell left inset, so its `x` is
  one cell past the outer layout boundary. Layout tests that measure released sidebar width must account
  for that inset.
- `cargo clippy --workspace --all-targets -- -D warnings` still stops on the pre-existing
  `clippy::needless_return` at `event_loop.rs:5375` (`return copy_via("pbcopy", &[])`), outside this diff.
