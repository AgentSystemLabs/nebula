# A Paused Rebase Renamed The Worktree Row To `detached @ …` — 2026-08-27

**Asked:** "something in this conversation caused the worktree to show up in the UI but then it switvhed
yp detached at f816b5f.  I wouldn't have expected the wortree name to become detached"

**Did:** Diagnosed, then fixed in `95c0a18`. The cause was the `git rebase origin/main` in the entry above
pausing on conflicts: a rebase parks HEAD on the commits it replays, so `git worktree list --porcelain`
prints `detached` (no `branch` line) for the checkout for as long as it sits there. The 2s worktree sync
(`reconcile_project_worktrees`, `registry.rs`) saw `known.branch != entry.branch`, wrote `detached @
f816b5f` into the row and broadcast it, then wrote the branch back on the tick after `rebase --continue`
finished. `git::list_worktrees` (`crates/nebula-daemon/src/git.rs`) now resolves a branch-less entry
through the new `rebasing_branch(checkout)`: `git rev-parse --absolute-git-dir`, then
`<git-dir>/{rebase-merge,rebase-apply}/head-name` (`refs/heads/<branch>` — the same file `git status`
reads to say "rebasing branch X"). Only a checkout with no rebase in progress, or one rebasing from an
already-detached HEAD (`head-name` reads `detached HEAD`), still gets `detached_label`. Test
`a_paused_rebase_keeps_the_worktree_on_its_branch` walks mid-rebase → `--abort` → `checkout --detach`.
147 daemon tests green.

**Gotchas:**
- **The row heals itself, so the bug is easy to write off as cosmetic — it isn't.** `nebula worktree
  <name>` finds its target by `w.branch == branch` (`registry.rs:~1377`) and *creates* a worktree when
  nothing matches, so an agent running it mid-rebase would have tried to add a second checkout for a
  branch that already has one. Anything keyed on the branch string is blind for the whole pause.
- The rebase state lives in the **per-worktree** git dir (`<repo>/.git/worktrees/<name>/rebase-merge/`),
  not the shared `.git` — `git_common_dir` in `lib.rs` deliberately hops *to* the shared one for the
  mtime probe and is the wrong helper here; `rev-parse --absolute-git-dir` from the checkout is right.
- `git worktree list` may print canonical paths while `add_worktree` returns the tempdir spelling
  (`/var/…` vs `/private/var/…` on macOS): the existing `remove_worktree_*` tests only assert
  `e.path != wt`, which passes trivially either way. Canonicalize both sides when a test needs to *find*
  the entry.
- `git::current_branch` is a second, uncalled copy of this label logic with its own `detached@` format.
  Left alone, but don't reach for it thinking it agrees with `list_worktrees`.
- **A "load race" can be a build-layout race.** After this change `workspace_scope_is_per_connection`
  (e2e_pty) failed 7 of 11 *idle* runs while the pre-change `git.rs` passed 13 of 13 — yet a probe
  showed `rebasing_branch` was never called and the porcelain parse was identical. Adding file I/O to
  `list_worktrees` for the probe made it pass 3/3: pure timing. It was the documented Ack-beats-upsert
  race, and a code change that never runs in the test still shifts the odds. Fixed on the test side
  (`6638952`): wait for the upsert *and* the Ack, as `cli_add_project` does; the TUI never relied on the
  order (`event_loop.rs` "usually lands just before this Ack; if not, …"). A/B the old file in place
  (`git show <sha>:path > path`, run, `git checkout -- path`) before believing either verdict.
