# Released v0.13.0 Off A Checkout Three Releases Stale — 2026-08-27

**Asked:** "commit push and make a release for me" (after the `nebula ssh` clipboard fix in the entry
below).

**Did:** Cut **v0.13.0** (0.12.0 → minor: new user-facing features). `release` skill as written: private
worktree on `release-v0.13.0` off `origin/main`, twelve files copied in by content,
`cargo test --workspace --no-fail-fast` in an isolated `CARGO_TARGET_DIR` — **655 passed, 0 failed**
across all seven binaries — three commits (feature / `.claude/MEMORY.md` / `Release v0.13.0`),
`git push origin release-v0.13.0:main`, tag, all four matrix targets green, notes replaced via
`gh release edit`. Carried the OSC 52 ssh clipboard route, the cloud-mirror work, and the tab underline
glyph. `random.txt` left untracked for the third release running.

**Gotchas:**
- **The shared checkout was 7 commits / 2 releases behind `origin/main`** (local `main` at
  `0361f0a`/0.10.0, origin at v0.12.0) — worse than the v0.12.0 entry's three. Same tell, same rule:
  `git diff` vs HEAD showed 20 files / 3,453 insertions, `git diff origin/main` showed 15 / 1,292, and
  only the second is the release. This has now happened three releases in a row because each release is
  cut from a worktree and never fast-forwards the shared tree — **assume it, don't check for it**.
- Untracked files read as *pure deletions* in `git diff origin/main -- <path>`: `pty/cloud.rs` showed
  286 deletions purely because it is untracked locally while origin has it committed. `cmp` against the
  worktree's copy before concluding anything — it was byte-identical and needed no copy at all.
- Vetting copied files means checking the **deletion** side of each hunk, not the addition side: the
  question is whether the stale tree reverts something origin committed. Here all 48 deletions in
  `registry.rs` were the cloud-mirror rewrite replacing the older attach logic, which is fine — but that
  is only visible by reading them.
