# Release v0.7.0 — Merge, Don't Copy, When The Shared Tree Is Behind — 2026-08-25

**Asked:** "commit and push and make a next version release"

**Did:** Released the SHARED CHECKOUT's uncommitted WORKSPACES COLUMN and the new NEBULA BROWSER `nebula browser`
(`crates/nebula/src/browser.rs`, ttyd on loopback) as `v0.7.0`. Commits `1e0c093` (feature), `71e2035`
(memory), `ead231d` (merge), `d9239d1` (focus-walk fix), `3306dc8` (bump). All 4 matrix targets built;
RELEASE NOTES rewritten.

**Gotchas:**
- **The RELEASE SKILL's "copy files by content into a RELEASE WORKTREE cut from `origin/main`" is wrong when
  local `main` is behind.** It was 12 commits behind here, and the dirty files predate all of them, so
  copying would have silently reverted the `h`/`l` panel remap (#12), the Linux clipboard (#11), and the
  pill-corner fix. What works: `git worktree add -b release-vX.Y.Z "$W" <local HEAD>` — the dirty files
  share that base, so a straight `cp` reproduces `git diff HEAD` exactly — commit there, **then**
  `git merge origin/main`. Only `.claude/MEMORY.md` and `README.md` conflicted.
- **A textually clean merge is not a green one.** Every code file auto-merged, and
  `event_loop.rs::h_and_l_walk_panel_focus_like_the_arrows` still failed: it asserts `h` from Projects
  "stops at projects", written before the WORKSPACES COLUMN made `App::leftmost_focus()`
  (`crates/nebula-tui/src/app.rs:2701`) return `Focus::Workspaces`. The `Action::FocusLeft` hint in
  `keymap.rs` carried the same stale wording. Merge conflicts flag the text collisions, not the
  semantic ones — run the suite before you tag, not after.
- The `README.md` conflict is the same staleness in prose: keep our new rows, but take `origin/main`'s
  `Shift+H` / `Shift+L` wording. The `.claude/MEMORY.md` conflict is pure placement — both sides append
  at the top of `## Entries` and `origin/main`'s half of the conflict is empty, so strip the three
  markers and keep ours.
- `assets/screenshot.png` shows as staged-added in the shared tree but is byte-identical to
  `origin/main`'s (v0.5.0 added it). Not something to carry.
