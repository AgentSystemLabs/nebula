# The ROOT WORKTREE Row's Lower Half Wasn't Clickable — 2026-08-27

**Asked:** "I can't seem to click on certain places of the root worktree row in some areas, other rows are
fully clickable zones, fix it"

**Did:** Every sidebar PILL ROW is a 3-row cell (pad, text, pad) stacked on a 2-row `PILL_H` stride, but its
hit rect was `rows_rect_at(inner, y, PILL_H)` — top pad + text only. Stacked rows hide that because the
next pill's top pad covers the gap; the ROOT WORKTREE row sits over a quiet spacer row, so its bottom pad (the
lower half of the pill as drawn) fell through to `PanelBg`. Same dangling pad on the last pill of any
group and the last of the list, in both panels. New `pill_hit_height(top, next_top)` in
`crates/nebula-tui/src/ui.rs` (next to `rows_rect_at`) sizes the target as `min(3, next_top - top)`:
a shared pad row still goes to the lower pill (unchanged), an unshared one stays with its pill.
`draw_worktrees` and `draw_sessions` iterate the layout with `enumerate()` to peek the next top;
`draw_session_row` grew a `hit_h` arg. Tests: `ui.rs::worktree_pills_are_clickable_over_their_whole_height`
and `session_pills_are_clickable_over_their_whole_height` (both fail on the old `PILL_H` rect).

**Gotchas:**
- **The "blank row" between the root row / a group's last pill and what follows is not an empty row —
  it is that pill's un-overlapped bottom pad.** The layout only bumps `vrow` by 1 after such a pill, and
  `WorktreeEntry::height()` is already `PILL_H + 1`, so `next_top - top` is 3 there, 2 when stacked.
  My first sessions-panel test expected a real gap row before the UNPINNED header and was off by one.
- `draw_column` returns rows starting at `area.y + 3` (spacer, title, spacer), so in a
  `TestBackend` drawn from `Rect::new(0,0,..)` the first pill's rows are y=3..=5.
