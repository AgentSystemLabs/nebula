# Reading A Pull Request And Its Diff Inside Nebula — 2026-08-24

**Asked:** "is it possible so that when I hover over a PR i nthe open pr list, it'll show the contents of
the PR directly in nebula for me to read? also the ability to just view the git diff of that PR directly
in nebula?" (follow-on to the OPEN PRS group below.)

**Did:** Hover (cursor rests on an open-PR row) → the TERMINAL PANE becomes a reader (the PR PREVIEW): headline, state,
`+adds -dels · N files`, description, then the whole conversation. New
`crates/nebula-tui/src/pr_preview.rs` (`wrap`, `fit`, `lines`) builds it as a flat `Vec<Line>` so
scrolling is a slice. Fetch is `pull_request::detail()` (`gh pr view <n> --json …`), debounced
`PR_DETAIL_DEBOUNCE = 300ms` via `schedule_pr_detail`/`lookup_pr_detail`, cached per URL for the
session. `g` on a PR row runs `pull_request::diff()` (`gh pr diff <n>`), `split_unified_diff()` cuts it
per file, and `open_pr_diff_view` opens the **existing** DIFF VIEWER (`DiffView`) on it via a new
`DiffView::prefetched: Option<HashMap<String,String>>` that `git_diff::load_selected_diff` reads instead
of shelling out. `ui::terminal_frame` now delegates to `titled_frame(title, …)` so the pane can be
called PULL REQUEST.

**Gotchas:**
- **`draw_terminal` returning early on a PR row is the whole trick** — the attachment underneath stays
  live, so walking into the PROJECT OPEN PRS GROUP and back never churns detach/attach. (It was modeled on the
  PROJECT DIVIDERS branch that sat above it until dividers were removed on 2026-08-25; it is now the only
  early return there.) Do not try to "clear" the terminal for this.
- **ratatui silently clips an overwide `Line`, taking the rest of the row with it.** A header row built
  from spans (state · author · base ← head) blew past the pane at width 24 and the test
  `no_rendered_line_overflows_the_pane` is what caught it. Everything the preview emits now goes through
  `wrap` (prose) or `fit` (span rows); `fit` drops whole segments from the end, then ellipsises.
- Reviewed ✓ marks (the REVIEWED MARK) are **deliberately not persisted** for a PR diff: `review::store_marks` prunes any
  key that isn't a directory on disk (`store.worktrees.retain(|root, _| Path::new(root).is_dir())`), and
  a pull request has no path. In-session marks still sink files to the bottom, which is the useful half.
- `shift+up`/`shift+down` were `move_project_up/down` (MOVE PROJECT) at the time (unbound since 2026-08-28), so the preview scrolls on
  **PgUp/PgDn/Home/End** (+ wheel) only — handled as raw `KeyCode`s *before* the KEYMAP lookup in
  `handle_key`, since those chords are unbound at panel scope.
- Key handlers can't reach the loop's channels, so the `gh pr diff` sender lives on
  `App::pr_diff_tx` — the `vim_tx` precedent. Without it `request_pr_diff` silently no-ops (which is
  also why its test installs a channel by hand).
- `gh pr view --json` field names verified live: `author`/`baseRefName`/`headRefName`/`additions`/
  `deletions`/`changedFiles`/`body`, comments carry `createdAt`, reviews carry `submittedAt` + `state`.
  Reviews with **no `submittedAt`** are your own pending draft — drop them.
- `split_unified_diff` takes the path from `+++ b/…` when present and falls back to the `diff --git`
  header, because a **deleted** file's `+++` is `/dev/null` and a **rename**'s two halves differ. The
  invariant worth keeping: every input line lands in exactly one chunk (asserted in the unit test, and
  verified against real `gh pr diff -R cli/cli` output).
- A `gh` that can't answer is remembered in `pr_detail_failed` — without it the pane re-asks on every
  pass and sits on "reading it…" forever.
