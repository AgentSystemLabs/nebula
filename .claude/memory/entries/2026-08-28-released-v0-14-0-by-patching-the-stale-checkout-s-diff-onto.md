# Released v0.14.0 By Patching The Stale Checkout's Diff Onto origin/main — 2026-08-28

**Asked:** "commit push and release"

**Did:** Cut **v0.14.0** (0.13.0 → minor: new user-facing features). `release` skill in a private
worktree on `release-v0.14.0` off `origin/main`, `cargo test --workspace` in an isolated
`CARGO_TARGET_DIR` — **704 passed, 0 failed** — three commits (feature / scaffolding / `Release v0.14.0`),
`git push origin release-v0.14.0:main`, tag, all four matrix targets green, notes replaced via
`gh release edit --notes-file`. Carried the DOUBLE TAP of `h`/`l` at the PANEL WALK's ends, click-outside
dismiss for every modal, the SETTINGS OVERLAY position expiry, PROJECTS PANEL focus on a created
WORKSPACE, the deleted-OPEN-WORKSPACE reseat, `make cycle`, plus `TERMS.md` and the `prompt-daddy` /
`project-terms` / `output-doctor` skills. The changelog also credits PR #17 (workspace-switch cold boot,
paused-rebase relabel), which landed on origin after v0.13.0.

**Gotchas:**
- **The shared checkout was 8 commits behind `origin/main` again — this time a merged PR (#17), not a
  release cut elsewhere**, and it touched `app.rs`, `event_loop.rs`, `ui.rs` and `MEMORY.md`, the same
  files as the uncommitted work. Copying those files by content would have silently reverted the PR.
  Better recipe than the skill's cp: in the shared tree `git diff HEAD > local.patch`, in the worktree
  `git apply --3way local.patch` — 13 of 14 files merged cleanly, including both sides' `MEMORY.md`
  entries. `git diff HEAD` does not carry untracked files; `TERMS.md` and the three new skill dirs still
  needed a `cp`.
- The one conflict was a signature skew: PR #17 gave `enter_terminal_pane` an `out: &mut Vec<ClientRequest>`
  (it now calls `fire_pending_attach`), while the DOUBLE TAP work had just wrapped it in
  `walk_focus_forward(app)`. Resolved by threading `out` through `walk_focus_forward` and its five call
  sites in `handle_key`. The shared tree's uncommitted `event_loop.rs` is therefore *not* what shipped —
  reconcile the shared tree with `git stash -u && git pull --ff-only && git stash drop`; everything in
  that stash is on origin already, in its merged form.
- `git ls-remote --tags origin | tail` sorts lexically, so `v0.9.0` comes last and `v0.13.0` looks
  missing. `grep` the version you want instead.
- `gh run list` right after `git push` of the tag can return the *previous* release's run; a `sleep 20`
  (or checking `headBranch` equals the tag) before `gh run watch` avoids watching v0.13.0 again.
