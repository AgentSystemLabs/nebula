# Done Reads Violet And Says "done" — 2026-08-27

**Asked:** Four turns, one thread. "don't put the number of running sessions in workspace hreader tabs,
just show \"2 done\"" → "replace word \"new\" with \"done\"" → "can you make the status dot for done a
different color than green so it's obvious something needs to be addressed" → the correction that settled
it: **"no you misunderstood, it should be green after I focus on the session, but purple when done and not
yet read"**.

**Did:** The UNSEEN finish is now a state with its own color, not a synonym for finished.
`workspace_running` → **`workspace_unseen`** (`crates/nebula-tui/src/app.rs:1477`) counts `a.unseen`,
mirroring `worktree_unseen` / `project_unseen`, so all three tiers count the same thing and the count dies
as you read. The STATUS DOT (`ui.rs::status_dot`) took an **`unseen: bool`** third arg: `Finished` draws `th.done` when
unread and `th.ok` once seen; every other status ignores it. All four call sites pass it (WORKSPACE TAB,
project row, worktree row, session row — `a.unseen && !a.archived`), and `PaletteItem` gained an `unseen`
field so the PALETTE (`/`) splits the same way. Wording: the DONE BADGE `unseen_badge` → ` n done` (was ` n new`), session row's
HARNESS BADGE slot takeover → ` done`. New THEME ROLE **`done`** = `Color::Indexed(141)` violet, `Indexed(45)`
turquoise in the `rose` preset (whose `special` is already 141). `th.ok` keeps green for diff-adds,
`⏻ connected`, reviewed-file ticks — and now for read finishes. The PR ROW's ` n new` was left alone:
it counts unread review comments, not finished turns. README dot table, feature bullet, badge paragraph and
`Shift+W` row updated. Tests: theme asserts `done` differs from `ok`/`warn`/`err`/`special`/`dim` in all 5
presets; `unwatched_finishes_badge_the_rows_until_read` now asserts violet before the read and green after,
on the session row *and* the project row above it. 450 nebula-tui + 143 daemon + 7 core green, fmt clean.

**Gotchas:**
- **The panel columns are a fixed 20 cells wide, so a wider badge clips instead of reflowing** — widening
  the `TestBackend` from 100 to 120 changed nothing but the TERMINAL pane. ` 1 new` → ` 1 done` is one
  cell more and the worktree root row started rendering `root 1 don`.
- **The project and worktree name budgets never billed the pill marker.** `render_pill` / `render_button`
  both `spans.insert(0, marker)` — one cell, rail or space — but `ui.rs` subtracted only the dot's 2
  (`saturating_sub(2 + badge_len)`). Pre-existing off-by-one that only bit once the badge grew; now 3.
  The PR row two arms down had it right all along (`saturating_sub(3)` for a 2-char `↗ `), which is what
  confirmed the convention.
- With that fixed, `main` ellipsized to `ma…` to keep ` ⌂ root`. The ROOT BADGE now **yields** to a branch
  it would otherwise truncate (`● main 1 done`, not `● ma… ⌂ root 1 done`) — the ⌂ is decoration, the
  branch is the row's identity. e2e's `wait_for_text("main ⌂ root")` still passes: no badge, no contention.
- `"client 1 done".contains("client 1 ")` is **true** — a negative assertion written to prove the bare
  running count was gone would have passed for the wrong reason. Dropped it.
- **"Done" is ambiguous and the first two readings were both wrong.** It went `Running` count → all
  `Finished` → unread-`Finished`, and the dot went all-`Finished`-violet → violet-only-while-unread. The
  distinction the user wanted was never finished-vs-not, it was **read-vs-unread** — the same axis
  `Agent::unseen` already tracked for the counters. When a color is asked for "so it's obvious something
  needs to be addressed", find the flag that already means "needs addressing" instead of coloring a status.
- Test counts drifted 450 ↔ 451 between back-to-back runs and two workspaces-bar tests failed once and
  never again: another session was editing this SHARED CHECKOUT mid-build. Rerun before debugging (see
  [Shared tree races] in the user memory).
