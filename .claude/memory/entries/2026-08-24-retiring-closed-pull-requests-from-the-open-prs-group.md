# Retiring Closed Pull Requests From The OPEN PRS Group — 2026-08-24

**Asked:** "when a pr is closed, we should periodically check from github to see if we should remove from
our list, also maje sure draft prs are included in that list we show" — ambiguous between the OPEN PRS
group and the worktree's own PR row in LINKS; the user picked **the OPEN PRS group** when asked.

**Did:** `OPEN_PRS_REFRESH` 3min → **60s** (→ **15 s** plus refresh-on-focus on 2026-08-28, see "Pull Requests
Refresh Every 15 s And On Focus") in `crates/nebula-tui/src/event_loop.rs` (that beat *is* the
pruning mechanism — `--state open` stops returning a merged/closed PR, so nothing tracks closures
separately). `note_open_prs_answer` gained an `out: &mut Vec<ClientRequest>` 4th arg and now calls two
new helpers: `reconcile_open_pr_cursor` (follows the cursor's PR across a reorder by URL; on retirement
clamps to the nearest surviving row, `restore_session`s if that's a checkout, and flashes
`#N is no longer open`) and `forget_retired_prs` (retains `pr_detail` / `pr_detail_failed` to URLs still
in some project's list). New `PrDetail::is_open()` + `drop_retired_pr` retire a row the instant the
hover-detail fetch comes back `MERGED`/`CLOSED`, ahead of the next list. Drafts needed **no code change**
— verified live that `gh pr list --state open` returns them (24 of 75 on `cli/cli`) — so the work there
was a doc note on `list()` and two regression tests. 5 new tests; workspace suite 598 green, clippy
unchanged (3 pre-existing warnings).

**Gotchas:**
- **`schedule_pr_detail` zeroes `app.pr_preview_scroll`.** Calling it unconditionally from the cursor
  reconcile meant every 60s refresh yanked a reader back to the top of the PR conversation they were
  halfway down. It is now called only on the branch where the row actually went away. Any new caller on
  a *timer* path has the same trap.
- Capture the cursor's PR **before** mutating `app.open_prs`, and follow it by **URL, not index** —
  `gh pr list` is newest-first, so anyone opening a PR reshuffles every row below it and an index-based
  cursor silently lands on a different pull request.
- The reconcile is inherently scoped to what's on screen: `visible_open_prs()` reads the *selected*
  project, so a late answer for a different project finds the cursor's URL unchanged and no-ops. No
  project-id comparison needed.
- Retiring the row the cursor is on is jarring without the flash — the pane just jumps. `app.flash`
  turns it into an explanation, and it's the one place both the list refresh and the detail-driven
  eviction funnel through.
- `note_open_prs_answer` is also called from `lookup_open_prs`'s not-a-directory branch, so `out` had to
  be threaded through that too (and into the git-poll tick's call).
- `gh pr list --state open` includes drafts by construction — do not "fix" a missing draft by adding a
  filter. If drafts ever go missing the bug is downstream, in `parse_list` or the `is_draft` badge at
  `ui.rs:2615`.
