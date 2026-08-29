# Worktree Watcher And Selection Memory — 2026-08-05

**Asked:** "verify we have some type of directory watcher on .worktrees or the github worktrees so that
when a new worktree is created from an agent or manually it'll update the worktrees list automatically.
right now i created a worktree and it did not show up in that list until i restarted nebula" — then:
"change of plans, we should remember the last agent that was selected for that project so that if i
switch between projects it'll automatically just show the last selected worktree & agent…"

**Did:** `91c29c0` (auto-sync + selection restore) and `02bb5a3` (refresh branches on external checkouts).
