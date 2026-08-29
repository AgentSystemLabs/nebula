# Released v0.4.0 From A Tree Two Sessions Were Writing To — 2026-08-24

**Asked:** "commit and push and do a release" (the open-PR reading pane + PR diff work below).

**Did:** **v0.4.0** — `1ac59a3`, tag pushed, all four binaries attached, changelog replaced. Three
commits: `cca5fbb` (feature), `684d562` (memory), `1ac59a3` (version bump). Local `main` is still at
`c340baf` (v0.2.0) and two releases behind `origin/main`.

**Gotchas:**
- **The SHARED CHECKOUT held two agents' unfinished features at once** — a Claude CLOUD LAUNCH flow and
  per-instance workspaces — tangled into `app.rs`, `ui.rs`, `event_loop.rs`, `lib.rs`, `README.md`. What
  worked: diff the *working tree* against `origin/main` per file, split into hunks, classify each one,
  and apply only mine onto the pristine copy. Scripts worth rebuilding:
  `hunks.py <old> <new>` (list hunks with a preview), `show.py <old> <new> 3,7` (dump specific ones),
  `pick.py <old> <new> <out> 0,2,6` (apply a subset with `patch -p0`, which tolerates the wrong line
  numbers a filtered patch carries). Residual-hunk count after picking is the check: it must equal the
  number you classified as theirs.
- **A hunk is not a semantic unit.** Two of mine had another session's line inside them — a
  `switch_workspace(...)` call adjacent to a `Palette::new` signature change, and a whole test of theirs
  (`palette_query_terms_match_independently_across_a_space`, which used *my* `seed_open_prs` helper to
  test *their* fuzzy change) sitting next to my test block. Both compiled fine in the shared tree and
  only failed in the isolated build. **The green gate is the only thing that catches this** — grepping
  the staged diff for their identifiers (`cloud`, `startup_workspace`, `switch_workspace`,
  `is_multiline`) afterwards is a cheap second check and found nothing left.
- **`README.md` had been rewritten wholesale** by the other session (243-line hunk). Hunk surgery was
  hopeless; re-applying my three edits by hand onto `origin/main`'s copy took two minutes.
- **A green SHARED CHECKOUT is not evidence about `main`.** `e2e_tui::tui_projects_worktrees_agents_navigation`
  passed locally the whole time because someone else had already fixed `FOOTER_TERMINAL_LOCKED` there —
  at `origin/main` it was still red, as it had been since v0.2.0. I had "corrected" the memory entry
  below to say it was fixed; that was wrong, and it is fixed properly now (in `e2e_tui.rs`, shipped in
  v0.4.0). Always run the doubted test in a **detached worktree at `origin/main`**.
- Use a separate `CARGO_TARGET_DIR` for the RELEASE WORKTREE (`$SP/vtarget`). Sharing the main one with
  a concurrently building session makes both thrash fingerprints.
