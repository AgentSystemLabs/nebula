# A Merged Or Closed PR ROW Goes; Drafts Sink Dimmed To The Bottom Of The PROJECT OPEN PRS GROUP — 2026-09-04

**Asked:** "when a pr is merged, it should never show up in the pr list.  merged or closed shoulds not
show up. make sure draft ones are always at bottom of list and different color so it's obvious they in
draft. also show label"
→ refined: The SESSIONS PANEL's OPEN PRS group shows a PR ROW for my worktree's branch even after that
pull request is merged or closed (all three worktrees show a `merged` badge right now, because `gh pr
view` answers with the branch's last PR whatever its state). A merged or closed PR must never appear
there: treat it as "no PR" so the row disappears. Keep the PROJECT OPEN PRS GROUP's existing 15 s pruning
as is. In the PROJECT OPEN PRS GROUP, sort draft PRs after every non-draft PR (newest first within each
half), render draft rows in a different color from open ones (assuming dim, the same role the PR PREVIEW
uses for `draft`), and keep the DRAFT BADGE's `draft` text label on them (assuming "show label" means
that mark, since no PR on this repo has ever carried a GitHub label). Apply the same draft coloring to a
draft PR ROW in the SESSIONS PANEL. (no questions asked)

**Did:** The "pr list" that showed merged PRs was the WORKTREE OPEN PRS GROUP's PR ROW, not the PROJECT
OPEN PRS GROUP (which `gh pr list --state open` already prunes on the GIT POLL): verified live that
`gh pr view` in every worktree answered `"state":"MERGED"` for #26/#27/#28.
`crates/nebula-tui/src/pull_request.rs::parse` now returns `None` unless `state` is `OPEN`, so a
merged or closed branch PR is an ordinary "no PR" and the row goes on the next GIT POLL beat;
`PullRequest.state` and `is_open()` are gone (always open now), `badge()` is `draft` / `pr`. New
`pull_request::drafts_last` (a stable `sort_by_key(is_draft)`) runs in
`event_loop.rs::note_open_prs_answer` before the dirty compare — the one path every list lands
through — and the URL-following cursor reconcile absorbs the reorder. New module
`crates/nebula-tui/src/pr_row.rs` (`look(is_draft, th)` → glyph / label / rail colors, `spans(...)`)
is what both `ui.rs::draw_worktrees`'s `WorktreeEntry::Row` arm and `draw_session_row`'s
`SessionRow::Link` arm build through: a draft is `th.dim` end to end (the PR PREVIEW's `draft` role),
a finished PR keeps `th.accent`; the DRAFT BADGE text stays and is billed before the title. Docs:
`docs/sessions.md`, `docs/how-it-works.md`. Tests: `a_merged_or_closed_branch_pull_request_is_no_row`,
`drafts_sink_below_the_open_rows_in_their_own_order`, two in `pr_row.rs` (every THEME preset; badge
before title), `draft_pull_requests_render_at_the_bottom_of_the_group`. Gate: `cargo fmt --all
--check`, `make lint`, `make test` (nebula-tui 601, e2e included) all green. No screenshot: the
SCREENSHOT HARNESS has to be rebuilt from its recipe and needs a repo with live drafts.

**Gotchas:**
- `gh pr view` with no argument answers with the branch's *latest* pull request whatever its state —
  it prefers an open one when several exist, but a lone merged PR keeps coming back for as long as the
  branch does. The PR ROW had carried `merged` / `closed` badges by design since 2026-08-23, under an
  `OPEN PRS` header. `gh pr list --state open` never had this problem: the two lists retire differently,
  and "merged PRs show up" points at the SESSIONS PANEL, not the WORKTREES PANEL.
- `the_cursor_follows_its_pull_request_across_a_reorder` seeded its "brand new" PR as a draft; with
  `drafts_last` a new draft sinks *below* the cursor and reshuffles nothing, so the test lost its
  point. A test that needs `gh`'s newest-first reshuffle must seed a finished PR; one that seeds a
  draft as the newest row and expects it on top is now wrong.
- "also show label" was read as the DRAFT BADGE's `draft` text, not GitHub labels: `gh pr list
  --state all --json labels` found 0 labelled PRs in the last 50, and the badge already existed but
  had never been on screen (no draft was open). A judgment, not a fact — reopen it if the user meant
  GitHub labels (they would need `labels` in `list()`'s `--json` and room in a 22-cell column).
- The installed `~/.cargo/bin/nebula` (protocol v37) refused `nebula rename` against the
  `target/debug` daemon (v38); `./target/debug/nebula rename` worked. Standing VERSION SKEW / AUTO-TITLE
  gotcha, re-hit.
