# The WORKSPACES COLUMN Drags To Resize — 2026-08-25

**Asked:** "also allow dragging the workspaces panel to resize like we do on the other panels"

**Did:** Reversed the "not draggable" decision from [A Workspaces Column Left Of Projects]. The column's
width moved out of the `WORKSPACES_PANEL_W` const into `App::workspaces_w`
(`crates/nebula-tui/src/app.rs:1894`), seeded from `DEFAULT_WORKSPACES_PANEL_W = 18` and persisted in the UI STATE BLOB as its
own `UiState::workspaces_w: Option<u16>` field rather than as a fourth slot in `panel_widths` — that blob
stays `[u16; 3]`, so every saved layout still deserializes. **`HitTarget::Splitter(usize)` was reindexed:
0 is now the workspaces|projects boundary, and the three old splitters became 1/2/3.** New
`App::splitter_indices() -> Range<usize>` returns `0..4` or `1..4` depending on `show_workspaces`; both
`ui.rs` loops (grab-zone registration at `ui.rs:78` and `draw_splitter_grips`) iterate it instead of
`0..3`. `splitter_x` dropped its inclusive range (`panel_widths[..idx]`, not `[..=idx]`) so idx 0
naturally means "the column's right edge". SPLITTER DRAG/hover/pointer-shape handling in `event_loop.rs` needed no
changes at all — it was already index-generic. 2 new tests, 4 updated; whole workspace suite green (440
tui unit + 21 e2e_pty + 6 e2e_tui + 133 daemon).

**Gotchas:**
- `set_splitter(0, …)` is *not* the same shape as the other three: the column starts at x=0, so the
  boundary x IS the width — no `offset + left` subtraction. Reusing the panel branch gives a column that
  drifts under the cursor.
- `normalize_panel_widths` had to clamp `workspaces_w` **before** computing the panel budget
  (`max = body_w - 3*MIN_PANEL_W - MIN_TERM_W`). Without it, a width dragged out on a wide screen
  survives into a narrow one, the budget goes to zero, all three panels floor at `MIN_PANEL_W` anyway,
  and the layout overflows the body.
- The grip for splitter 0 lands on the WORKSPACES COLUMN's own `Borders::RIGHT` cell, which exists — but
  a body only 120 wide with the default panels caps the column at 26 (`120 - 74 - MIN_TERM_W`), so a test
  that drags it to 30 and asserts 30 fails at 26. Pick drag targets inside that headroom.
- `seed_splitters` in `event_loop.rs` tests still hides the column (its `x = 20, 42, 68` depend on it),
  so its loop is `app.splitter_indices()` = `1..4` — every assertion in the drag/hover tests that read
  `idx == 0` for the projects|worktrees boundary had to become `1`.
