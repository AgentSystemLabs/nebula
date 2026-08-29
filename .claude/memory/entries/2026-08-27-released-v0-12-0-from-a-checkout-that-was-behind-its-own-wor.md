# Released v0.12.0 From A Checkout That Was Behind Its Own Work — 2026-08-27

**Asked:** "commit push release"

**Did:** Cut **v0.12.0** (0.11.0 → minor: new user-facing feature) carrying the done-vs-read violet work
in the entry below. RELEASE SKILL (`release`) as written: private RELEASE WORKTREE on `release-v0.12.0` off `origin/main`,
five files copied in by content (`README.md`, `app.rs`, `event_loop.rs`, `theme.rs`, `ui.rs`),
`cargo test --workspace --no-fail-fast` in an isolated `CARGO_TARGET_DIR` — **647 passed, 0 failed**,
exit 0 — three commits (feature / `.claude/MEMORY.md` / `Release v0.12.0`), `git push
origin release-v0.12.0:main`, tag, all four matrix targets green, RELEASE NOTES replaced via `gh release edit`.
`random.txt` left untracked again.

**Gotchas:**
- **The SHARED CHECKOUT's `main` was three commits behind `origin/main`, and its working tree already
  held everything v0.11.0 shipped.** So `git diff` (vs local HEAD) showed 19 files / 2,255 insertions
  and `git diff origin/main` showed 5 — the second number is the release. Always scope the "what is
  unreleased" question to `origin/main`, never to HEAD, in this tree: the previous release was cut from
  a worktree and never fast-forwarded the SHARED CHECKOUT.
- `cargo test --workspace` **fail-fast makes a flake look like a truncated suite**: run 2 died at
  `e2e_pty` (1 of 23) and never started `e2e_tui` or the lib tests. Runs 1 and 3 were fully green. Use
  `--no-fail-fast` for the gate so one flaky e2e can't hide the other six binaries' results.
