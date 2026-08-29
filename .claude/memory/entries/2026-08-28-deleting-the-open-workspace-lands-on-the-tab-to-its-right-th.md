# Deleting The OPEN WORKSPACE Lands On The Tab To Its Right, Then Its Left — 2026-08-28

**Asked:** "when the workspace is deleted, it should select the previous workspace in the list. if deleting
the first workspace, select the next, if deleting from the middle, we should focus on the workspace to the
right"
→ picked (prompt-daddy): *Right, else left* — "When the OPEN WORKSPACE is deleted — `d` / `m` in the
WORKSPACES BAR, `d` in the WORKSPACE SWITCHER, or from another instance / `nebula workspace delete` —
open the WORKSPACE TAB that was to its right … If it was the last tab, open the one to its left. Deleting
a workspace that is not the open one leaves the OPEN WORKSPACE alone. Replace the current 'fall back to
the first workspace' reseat."

**Did:** `reseat_deleted_workspace` (`crates/nebula-tui/src/event_loop.rs` ~2685) takes a new
`removed_tab: Option<usize>` and lands on `workspaces[removed_tab.min(len - 1)]` instead of
`workspaces.first()`. The `ServerEvent::EntityRemoved` arm in `handle_server_event` computes that
position with `workspaces.iter().position(..)` **before** `apply_removal`. Every delete path — bar, switcher,
CONTEXT MENU, other instance, CLI — arrives through that one delta, so nothing else changed. Test
`deleting_the_open_workspace_lands_on_its_right_neighbor_then_its_left` (first / middle / last / not-open).
README rows for the switcher and the Workspaces keymap note it. 469 nebula-tui tests green.

**Gotchas:**
- The prompt's first sentence ("select the previous") contradicts its third ("to the right"); the pick
  settled it as right-first, left only from the last tab. Don't relitigate — the "previous" case exists
  only because the last tab has no right neighbor.
- The deleted row's index must be captured before `apply_removal` runs its `retain`; after it the row is
  gone and only `first()` is knowable — which is exactly why the old reseat jumped to the first tab.
- After the retain, the right neighbor sits at the deleted tab's *own* index, so `min(len - 1)` is the
  whole first/middle/last logic; no branching on position needed.
