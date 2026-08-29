# `make dev` Still On 0.9.0: Pulling v0.10.0 Under Real In-Flight Work — 2026-08-26

**Asked:** "make dev is still showing the wrong version. pull latest from main into this"

**Did:** Same root cause as the v0.4.0 entry below — the shared checkout was at `1506bbf` / 0.9.0 while
`origin/main` was `0361f0a` / v0.10.0 (PR #16, the tab-bar merge). The difference this time: the dirty
tree was not stale leftovers but ~1,300 lines of *uncommitted, un-branched* work (the three entries just
under this one: cloud re-attach, workspace-delete confirm, unseen badges) based on `249668e`, overlapping
almost every incoming file. Recipe that worked, in order: `git stash create` (a stash commit without
touching the tree) → `git worktree add --detach <scratch> <that sha>` → `git merge origin/main` there →
resolve the 7 conflicts → `cargo build`/`clippy`/`test` with `CARGO_TARGET_DIR` outside the repo → then
in the shared tree `git diff --quiet <sha>` (nobody else edited meanwhile), `git stash push -u`,
`git merge --ff-only origin/main`, `git restore --source=<scratch commit> --worktree -- <files>`. Result:
HEAD = origin/main, working tree = v0.10.0 + the three features, still uncommitted, `target/debug/nebula
--version` → 0.10.0. The WIP is kept as `stash@{0}` ("cloud/unseen/ws-delete wip before v0.10.0 pull").

**Gotchas:**
- `git stash create` ignores untracked files, so the scratch merge failed with `E0583 file not found for
  module cloud` (`pty/cloud.rs`). Copy untracked files in by hand, and after the ff restore them from the
  stash's third parent: `git restore --source='stash@{0}^3' --worktree -- <paths>`.
- Conflict shape when the WIP already contains part of the incoming range: hunks where origin/main's later
  commits did not touch the file (registry.rs, store.rs migrations 19/20) resolve as **ours** wholesale;
  only files the tab-bar prototype (`30042e9`) rewrote needed thought — `leftmost_focus` → `first_focus`
  in `app.rs`, the `Action::Delete` arm in `event_loop.rs` (local `open_delete_confirm` already routes
  `Focus::Workspaces` through `open_remove_workspace_confirm`, so `ours` wins), and `ui.rs` where the
  `TAB_*` consts and the `ProjectRowData`/`WorktreeRowData` aliases land on the same lines (keep both).
  `git diff <base-commit> origin/main --stat -- crates/` tells you which files need thought.
- `git diff --quiet <commit>` only compares tracked files — it will say the tree matches even when
  untracked WIP is missing. `cmp` the untracked files separately.
- `workspace_scope_is_per_connection` (e2e_pty) failed twice under a parallel clippy+test run and passed
  alone: the Ack-beats-upsert load race the v0.10.0 entry describes, not the merge (test fixed
  2026-08-27, `6638952`).
- The old Makefile's dev daemon lives at `/tmp/nebula-dev`; the new per-checkout slot is
  `/tmp/nebula-dev-<8 chars of shasum of $CURDIR>` (`2f3f877f` for the main checkout), so the new
  `dev-stop` cannot see a daemon the old recipe started. A `make dev` TUI that was already open keeps its
  0.9.0 daemon until it quits — the old recipe's trailing `dev-stop` then reaps it. Quit and rerun.
- The new slot also means a fresh `$HOME/.nebula-dev/nebula-<slot>` data dir: first `make dev` re-seeds
  from the real DB instead of reusing the old `~/.nebula-dev/nebula.db`.
