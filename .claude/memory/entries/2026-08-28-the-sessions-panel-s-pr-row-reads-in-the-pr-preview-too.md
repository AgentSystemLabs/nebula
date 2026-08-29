# The SESSIONS PANEL's PR ROW Reads In The PR PREVIEW Too — 2026-08-28

**Asked:** "when i focus on a pull request in the session row on the sessions list, it should show the
pr description on the right similar to how it works on the prs on worktree list"
→ refined: When FOCUS rests on the PR ROW in the WORKTREE OPEN PRS GROUP of the SESSIONS PANEL, show
that PR's PR PREVIEW in the TERMINAL PANE exactly like resting on a PROJECT OPEN PRS GROUP row in the
WORKTREES PANEL does — same debounced `gh pr view` fetch, same cache, same PgUp/PgDn/wheel scroll
(assuming `g` for the diff too). Saved LINK rows keep their current behaviour. The ATTACH underneath
must stay live. (no questions asked)

**Did:** One notion of "the pull request the pane is reading": `App::previewed_pr()` in
`crates/nebula-tui/src/app.rs` returns a `PreviewedPr { number, url, label }` from either the Worktrees
cursor's `OpenPr` or — only while `focus == Focus::Sessions` — the `selected_link()`'s
`pull_request()` (a saved LINK that *is* the PR counts, a bare URL does not). Every PR PREVIEW site
switched from `selected_worktree_pr()` onto it: `ui.rs::draw_terminal` early return and
`draw_pr_preview`, `event_loop.rs::schedule_pr_detail`, `request_pr_diff`, the PgUp/PgDn/Home/End
branch (now `Focus::Worktrees | Focus::Sessions`), `Action::GitDiff`, the WHEEL SCROLL over the pane.
The row-change choke point is new `note_preview_change(app, before)`: the loop takes
`previewed_pr().map(url)` beside `focus_before` and compares after the burst drains, re-arming the
detail fetch (and rewinding scroll) only when the URL changed. FOOTER for the discovered PR ROW gained
`g: diff  PgUp/PgDn: scroll`; `menu_items_for_link` offers **View diff** on a PR row. README updated.
3 new tests; nebula-tui 523 green, clippy `-D warnings` and fmt clean; `make install` done (TUI-only).

**Gotchas:**
- **The SESSIONS PANEL half of `previewed_pr()` is keyed on `Focus::Sessions` on purpose**, unlike the
  Worktrees half: a session is still ATTACHED behind that pane, so `l` / a click into the TERMINAL
  PANE has to bring the terminal back — otherwise keys go to a PTY you can't see. A Worktrees PR row
  has no `selected_worktree()`, so nothing is behind its pane and focus doesn't matter there. "The
  preview disappears when I focus the pane" is by design, not a bug.
- **Hook "the pane reads something else" at one choke point, not per assignment**: there are 32
  `app.sel_session =` writes in `event_loop.rs`. `note_preview_change` sits *after* the
  `channels.rx` / `vim_rx` burst drains so a tree update in the drain counts, and compares the URL
  rather than the whole `PreviewedPr` so a re-titled PR arriving on the GIT POLL doesn't zero the
  reader's scroll. The explicit `schedule_pr_detail` calls in `select_worktree_row` / `restore_context`
  / `drop_retired_pr` stay: unit tests drive `handle_key` directly and never run the loop.
- Both rows fetch from `selected_project().repo_path` — `gh pr view <n>` resolves the number from any
  checkout of the repo, and the detail cache is keyed by URL, so the PR ROW and the same PR in the
  PROJECT OPEN PRS GROUP share one fetch.
- A single click and a right-click on a SESSIONS PANEL row both set `Focus::Sessions` before acting,
  which is what makes the focus-keyed preview and the **View diff** menu row work from the mouse.
