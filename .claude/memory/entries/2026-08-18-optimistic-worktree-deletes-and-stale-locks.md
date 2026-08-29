# Optimistic WORKTREE DELETES And Stale Locks — 2026-08-18

**Asked:** "add some type of background task for deleting worktrees, I notice when i try to delete a
worktree, it often freezes up for a bit until it finally removes the worktree, I'd like it to do
optimistic client updates for when it's deleted and rollback if it fails…" Plus: "I'm trying to delete a
worktree and it says 'cannot remove a locked working tree, lock reason: claude session
menu-enable-level'. when I try to delete a worktree, it should force kill and remove any locked sessions…"

**Did:** `d214366` — WORKTREE DELETES are optimistic with rollback (a PENDING INTENT), and stale session locks are force-unlocked.

**Gotchas:**
- The lock is not nebula's; Claude's EnterWorktree creates locked worktrees, so `git worktree remove`
  refuses until the lock is cleared.
