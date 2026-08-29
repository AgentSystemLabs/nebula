# Open Pull Requests Under The Worktrees List — 2026-08-24

**Asked:** "when a user opens a project, it should try to fetch all open pull requests and display those
on the bottom of the worktrees list, so a user can easily see which pull requsts are still open and enter
or click into them to open in browser. make sure you make this efficient as gh might have rate limits,
and some projects might have a LOT of opened pull requests, also make sure a user can easily fuzzy find
(/) to those pull requests by title which when opened will open the browser instead of trying to switch
to a session or worktree, etc"

**Did:** New `OpenPr` + `list()` + `parse_list()` in `crates/nebula-tui/src/pull_request.rs`
(`gh pr list --state open --limit 100 --json number,url,title,isDraft`, `LIST_LIMIT = 100`). Cached per
project as `App::open_prs: HashMap<ProjectId, OpenPrs>` with `open_prs_inflight`; driven from
`lookup_open_prs`/`note_open_prs_answer`/`schedule_open_prs_lookup` in `event_loop.rs`, riding the
existing GIT POLL tick (`GIT_POLL`). `draw_worktrees` in `ui.rs` (the WORKTREES PANEL) was rewritten from a straight-line renderer into a
`WorktreeEntry` virtual-row layout with a follow-window (`worktrees_scroll`/`worktrees_anchor`, wheel
support) mirroring `draw_sessions`, and grew an `OPEN PRS · N` group (the PROJECT OPEN PRS GROUP). `PaletteTarget::PullRequest(url)`
makes them fuzzy-findable in the PALETTE; `jump_to_target` opens the browser instead of moving a cursor.

**Gotchas:**
- **The whole design hinges on PR rows sorting *after* the checkouts.** `sel_worktree` now indexes the
  combined list, so every existing `visible_worktrees().iter().position(...)` (there are ~8 of them, in
  `restore_context`, `reconcile_selection`, `jump_to_target`, `select_worktree_by_id`, the saved UI
  state) stays correct untouched, and `selected_worktree()` returns `None` on a PR row for free — which
  is what makes `p`/`d`/`e`/`n` silently no-op there instead of acting on the wrong worktree. Only
  `move_selection` and `clamp_selections` needed the new `worktree_row_count()`.
- `restore_session` must NOT run when the cursor lands on a PR row — it detaches the terminal when
  `selected_worktree()` is None, so arrowing into the group would blank the pane. Guarded in both
  `move_selection` and the left-click handler. The SESSIONS PANEL does go empty there; that's deliberate
  (it followed the PROJECT DIVIDERS behavior, since removed on 2026-08-25), not a bug.
- **`gh pr list` returns a bare JSON array**, not an object — `parse_list` takes `as_array()`, unlike
  `parse` which reads fields off a map. Verified against `gh pr list -R cli/cli`.
- `Some(vec![])` (repo genuinely has nothing open) and `None` (no `gh`, no remote, timeout) must stay
  distinct: `None` keeps the last good list on screen rather than blanking the group over one flaky
  round trip. Both back off, `OPEN_PRS_RECHECK_MIN` 30s → `OPEN_PRS_RECHECK_MAX` 10min; a non-empty
  answer settles on `OPEN_PRS_REFRESH` — **15 s as of 2026-08-28 (60s before that), not the 3min this
  entry originally shipped**.
- Rate-limit floor: `schedule_open_prs_lookup` pulls the deadline in to `at + OPEN_PRS_MIN_AGE` (30s)
  instead of clearing the entry the way `schedule_pr_lookup` does for worktrees — otherwise bouncing
  between two projects spends an API call per switch.
- The double-click chain (`app.last_session_click`) is shared with the SESSIONS PANEL's LINK rows. A
  click on a *checkout* row has to clear it, or click-PR → click-worktree → click-PR reads as a
  double-click and launches the browser.
- `open_url` short-circuits to `true` under `cfg!(test)`, so Enter/double-click paths are safe to test
  end-to-end — assert on `app.flash == "opened github.com/o/r/pull/7"`.
- The two `clippy::type_complexity` warnings in `ui.rs` (2316, 2446) pre-date this work — the worktrees
  tuple annotation is unchanged from HEAD. `e2e_tui::tui_projects_worktrees_agents_navigation`, recorded
  below as failing at origin/main, **now passes** (6/6 green).
