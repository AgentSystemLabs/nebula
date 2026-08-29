# PR Links And New-Comment Counts — 2026-08-23 → 08-24

**Asked:** "I noticed that one of my sessions created a pull request but that link was not auto detected,
I think when I switch to a worktree you should run a background process to check if any pull request are
open and show them as links…" Then: "if possible, track how many NEW comments were added since the last
click on a pull request link, it would be nice to see when others have left comments…"

**Did:** `crates/nebula-tui/src/pull_request.rs` plus a `pr_seen` read-marker map on `App`
(`app.rs:1718`). Links pin to a worktree; commit `44bd270`.

**Gotchas:**
- `gh pr view --json comments,reviews`: `comments[]` has **`viewerDidAuthor`**, `reviews[]` does **not** —
  telling your own reviews apart needs `gh api user --jq .login`. Inline per-line review comments aren't
  exposed as a `--json` field at all; counting review submissions is the cheap approximation.
- Both timestamps are RFC 3339 UTC, which sorts **lexicographically in chronological order**. `pr_seen`
  stores the newest stamp seen at open time, so "newer than X" is a string compare — no clock, no date
  parsing, and no `chrono`/`time` dependency added to a deliberately dep-light workspace. Empty string
  works as the sentinel because every real stamp sorts above it.
