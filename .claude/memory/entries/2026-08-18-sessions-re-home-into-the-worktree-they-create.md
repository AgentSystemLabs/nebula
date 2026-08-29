# Sessions Re-Home Into The Worktree They Create — 2026-08-18 → 08-24

**Asked:** "sometimes I'll be on the main root worktree and I'll start a session, and inside that session
I'll prompt it to do the work inside a worktree, which claude or codex will then create the worktree. if
possible, when this happens I want to move the session out of that main worktree root and move it to…"
Later, twice more: "there is a strange bug where … after I manually move a session to that work tree, at
some point in the future that original session seems to switch back to whatever worktree it originally
was…" and "the session takes a while before it is moved into the worktree… is there a way to make
automatically move…"

**Did:** `7570387` re-homes an agent row by hook-reported cwd. The cwd probe is the
`("PostToolUse", Some("Bash|EnterWorktree|ExitWorktree"))` matcher in `hooks/installer.rs`.

**Gotchas:**
- Claude uses its own **EnterWorktree** tool, not `git worktree add`. That creates a **locked** worktree
  at `<repo>/.claude/worktrees/<name>` on branch `worktree-<name>`.
- A Bash `cd` to a directory **outside the session's workspace root is silently reset** ("Shell cwd was
  reset to …") and the hook cwd never changes. So nebula's own sibling layout
  (`<repo>/../<repo>-worktrees/<branch>`, `git.rs` `worktree_dir`) is unreachable by cwd-following — only
  checkouts *inside* the repo re-home.
- Before the `EnterWorktree` matcher existed, the row only moved at the turn's `Stop` — measured **~34s
  late**, which is exactly the "takes a while" the user reported.
- **Hooks are snapshotted at session start**, so any hook-set change only reaches newly spawned sessions.
