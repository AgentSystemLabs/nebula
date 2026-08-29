# The PALETTE Sorts By Attention, Then RECENCY ORDER, Before You Type — 2026-08-29

**Asked:** "when doing the fuzzy / finder, make sure all sessions needing user input are always sorted to
the top by default, followed by running agents, followed by done.  next we should show the most recently
used project worktree so a user can quickly get back to a project or workspace they were just in,
everything else should sort under those rules."
→ refined: In the PALETTE (the `/` fuzzy finder), with an empty query, order the rows by attention
instead of tree order: SESSIONS in NEEDS FEEDBACK first, then RUNNING, then FINISHED + UNSEEN (assuming
"done" means the violet unread ones — every read FINISHED session would otherwise bury what follows),
each tier in RECENCY ORDER. Below those, everything else in RECENCY ORDER — WORKTREE, PROJECT and
WORKSPACE rows carrying the newest stamp under them — so I can get straight back to the project or
workspace I was just in; never-run rows keep tree order at the bottom (assuming ARCHIVED sessions sink
there too). Typing still ranks best match first, with this order as the tiebreak (assuming "by default"
means the empty query). Keep the panels as they are. (No questions asked.)

**Did:** The PALETTE moved out of `app.rs` into its own module first, behavior unchanged
(`crates/nebula-tui/src/palette.rs`: `PaletteTarget`, `PaletteItem`, `PaletteMatch`, `Palette`,
`build_palette_items`, `palette_workspace_order`; `app.rs` re-exports them so `crate::app::Palette` in
`ui.rs`, `event_loop.rs` and the tests still resolves), 18 palette tests green across the move. Then the
order: `PaletteItem` gained `tier: PaletteTier` (`NeedsFeedback < Running < Unseen < Rest < Archived`,
from `session_tier`; every non-session row is `Rest`) and `interacted: i64` (`last_interaction_ms` for a
session, `worktree_recency` / `project_recency` / the new `app.rs::workspace_recency` for the rows above
it, `archived_at` for an archived row, 0 for a PR). `palette.rs::attention_rank` turns
`(tier, Reverse(interacted), build index)` into each item's position; `Palette::apply_filter` feeds that
as the tiebreak to the new `fuzzy::rank_by(query, candidates, key)` — an empty query lists in key order,
a typed one scores first and breaks ties by key — while `fuzzy::rank` keeps its own blank guard and
delegates with `(len, i)`, so the FILE FINDER, DIFF VIEWER, TREE BROWSER and TYPE-AHEAD are untouched.
`items` stays in build order (the two tests asserting it are unchanged); only `matches` reorders. README
`/` row says so. Tests: four in `palette.rs` over a hand-built `Tree` (tiers, recency leading the rest,
score-beats-tier + tie-by-attention, archived sinks) and `fuzzy::rank_by_…`; nebula-tui 538 green,
clippy / fmt / rustdoc clean; no E2E test reads the PALETTE.

**Gotchas:**
- The PALETTE has two orders now: `items` is build order (workspace → projects → worktrees → sessions →
  PRs, open workspace first) and is what `slash_opens_palette_listing_…` and
  `other_workspaces_are_off_the_panels_…` assert; the order the user sees is `matches`, from
  `attention_rank`. A future "show X first" is a change to `attention_rank`'s key, never a reshuffle of
  `build_palette_items` — the build index is the last tiebreak and is what keeps a workspace over its
  project over its worktree over its session when all four share a stamp.
- `fuzzy::rank` must keep its own whitespace guard rather than delegate to `rank_by` blindly:
  `rank_by`'s blank path sorts by the key, and `rank`'s key is `(text length, index)` — delegating
  without the guard re-introduces the 2026-08-24 shortest-first re-sort on a query that says nothing.
- "done" in the PALETTE tiers means FINISHED + UNSEEN, not every FINISHED session: nearly every session
  in the tree is a read FINISHED one, and tiering all of them would bury the recency rows the second
  half of the ask is about. Read FINISHED rows sort by RECENCY ORDER with the worktrees, which is where a
  session you just left lands anyway.
