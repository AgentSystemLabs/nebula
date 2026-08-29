# Project Dividers And Shift+J/K Reordering — 2026-08-05

**Asked:** "add a way to put dividers between projects, also a way to hold shift and move projects up and
down in regards to their order in the list so that I can group projects together" — then, after the first
attempt only swapped neighbours: "when I do shift j and k, it doesn't seem to move projects under
dividers, it just swaps projects, you must treat a divider as something I can move a project under or
above separate" and, escalating, "I should be able to move a project into any fucking divider I want."

**Did:** `98dc681` — reordering treats dividers as real positions, and dividers are labelable and movable.
**Superseded 2026-08-25:** dividers were removed entirely (see [Project Dividers Removed From The Projects
Column]); Shift+J/K reordering stayed as a plain move — **until 2026-08-28**, when MOVE PROJECT was removed too
and the PROJECTS PANEL began sorting itself by last interaction (see [Lists Order Themselves]).

**Gotchas:**
- Shift+↑/↓ is **undeliverable in Terminal.app**: `keyMappings.plist` has entries for `$F702`/`$F703`
  (Shift+←/→) but **none** for `$F700`/`$F701`, so Terminal drops the shift and sends a plain arrow.
  Shift+J/K works everywhere because crossterm tags uppercase chars with SHIFT.
- "Move" has to mean move-across-groups, not swap-with-neighbour. The first implementation satisfied the
  literal words and not the request.
