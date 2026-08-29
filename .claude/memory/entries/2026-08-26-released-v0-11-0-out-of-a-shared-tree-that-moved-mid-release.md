# Released v0.11.0 Out Of A Shared Tree That Moved Mid-Release — 2026-08-26

**Asked:** "commit push release"

**Did:** Cut **v0.11.0** (0.10.0 → minor: new features) from the ~1,900 lines of uncommitted work sitting
in the SHARED CHECKOUT — CLOUD ATTACH, UNSEEN counters, workspace-delete confirm, workspace context
restore, NOTES removal, the VERSION SKEW message. Followed the RELEASE SKILL: private RELEASE WORKTREE on
`release-v0.11.0` off `origin/main`, files copied in by content, `cargo test --workspace` in an isolated
`CARGO_TARGET_DIR` (**647 passed, 0 failed**, all 7 binaries incl. e2e_pty 23 / e2e_tui 5), three commits
(feature / `.claude/MEMORY.md` / `Release v0.11.0`), `git push origin release-v0.11.0:main`, tag, then
`gh release edit --notes-file`. All four matrix targets green; 4 assets attached. `random.txt` (untracked
scratch, "nothing here is load-bearing") deliberately left out.

**Gotchas:**
- **`for f in $(git diff --name-only)` silently copies nothing in zsh.** Unquoted expansions are not
  word-split, so the loop ran once with all 19 paths as a single filename and `cp` failed with one
  `No such file or directory`. The tell was `git status` in the new worktree showing only the untracked
  file. Use `... | while IFS= read -r f`.
- **`cargo test … | tail -60` reports `tail`'s exit code, not cargo's.** The first run "passed" with exit 0
  while the tail showed no e2e results at all. Redirect to a file and check `$?` directly, or set
  `pipefail` — never trust a piped cargo exit status for a green gate.
- **An untracked file reads as `deleted` in `git diff <commit> -- <path>`.** Diffing the SHARED CHECKOUT
  against the release commit showed `cloud.rs | 286 ------` because untracked files aren't in the index.
  It was byte-identical (`cmp`); nothing was lost. Verify with `cmp` before believing a deletion.
- **The SHARED CHECKOUT moved between the snapshot and the push.** `git diff | shasum` went `8d64c39` →
  `3b9c7e0`: another session changed the WORKSPACES BAR badge from "count running" to "count done"
  (`workspace_running` → `workspace_done` in `app.rs`/`ui.rs`, plus a README cell). Not in v0.11.0, by
  design. Checksum the diff before and after the copy — it is the cheapest proof of what you actually shipped.
- The user's local `main` stays at `0361f0a` while `origin/main` is `8102fa4`; the working tree still holds
  every released change as uncommitted edits, so a plain `git pull` will refuse. Branch `release-v0.11.0`
  is kept locally as the handle to those commits.
