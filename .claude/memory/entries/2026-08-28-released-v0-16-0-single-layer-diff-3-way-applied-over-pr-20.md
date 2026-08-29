# Released v0.16.0: A Single-Layer Diff 3-Way Applied Over PR #20's Merge — 2026-08-28

**Asked:** "commit and push and release" (RELEASE SKILL trigger — PROMPT DADDY skipped).

**Did:** Cut **v0.16.0** (0.15.0 → minor: AGENT PRESETS, DONE SOUND, RECENCY ORDER, harness toggles,
`k`,`k` / `j`,`j`, plus PR #20's TOGGLE PROJECTS PANEL / TOGGLE WORKTREES PANEL already on origin). The
SHARED CHECKOUT's HEAD (`31d3add`) was the v0.15.0 tip itself and `origin/main` was 5 commits ahead (PR
#20's merge), so the uncommitted diff was **one layer**: `git diff HEAD > local.patch`, `git worktree add
-b release-v0.16.0 … origin/main`, `git apply --3way local.patch`, `cp` the 125 untracked files (`while
IFS= read -r`). Five conflicts, all mechanical except one: `config.rs` (keep `HideProjects`/`HideWorktrees`
*and* `ClaudeEnabled`; `shown_hidden` + `pub(crate) cycle_choice`), `event_loop.rs` (keep the ⇧P/⇧B arms,
drop the retired `MoveProject*` arms, take the `j`,`j` MoveDown arm; delete the old walk functions),
README / TERMS.md column-by-column, `.claude/MEMORY.md` (take the local index, extract origin's PR #20
block into `entries/2026-08-28-independent-projects-panel-and-worktrees-panel-visibility.md` +
`index_line.py`). The non-mechanical one: PR #20's `next_visible_focus` / `previous_visible_focus` walk
had to be ported by hand into `crates/nebula-tui/src/event_loop/focus_walk.rs` (`next_focus` removed,
new `bar_return_target` so `j`,`j` never lands on a hidden panel). Gate in the RELEASE WORKTREE on the
v0.15.0 `vtarget`: `make memory-check` ok, fmt, clippy `-D warnings`, **772 passed, 0 failed**. Three
commits (`0225eac` feature / `bad8ea4` scaffolding / `bbe8bf0` Release), `git push origin
release-v0.16.0:main`, tag, all four matrix targets green, notes via `gh release edit --notes-file`.
PROTOCOL VERSION 30 → **32** (origin never touched `protocol.rs`; no collision; no MIGRATION). Then
reconciled the SHARED CHECKOUT: `git diff HEAD | shasum` and the untracked count still matched the
snapshot, so `git stash push -u -m "pre-v0.16.0-release shared-tree state"` + `git pull --ff-only origin
main` → `git diff origin/main` empty; the stash is kept, the RELEASE WORKTREE removed, the branch kept.

**Gotchas:**
- **Single-layer tell.** `git rev-list --left-right --count HEAD...origin/main` reading `0 N` with HEAD
  itself a commit on origin means `git diff HEAD` is exactly the unreleased delta — `--3way` + `cp` is
  the whole recipe; the snapshot / cherry-pick dance from v0.15.0 is only for a HEAD behind its own work.
- **A merged PR vs. a local extraction conflicts at the wrong site.** PR #20 edited `walk_focus_forward`
  / `walk_focus_back` in `event_loop.rs`; the local task had moved them to `event_loop/focus_walk.rs`.
  The conflict showed up as ours = PR's version, theirs = deleted — taking "theirs" compiles, and the
  new module silently keeps the pre-PR logic (hidden panels not skipped). Grep the ours side for its
  identifiers (`next_visible_focus`) and re-apply them in the module by hand.
- `git commit` after a hand-resolved `--3way` refuses with "Committing is not possible because you have
  unmerged files" until every conflicted path is `git add`ed — and a partial `git add` that skips them
  leaves the first commit's staged set to be swallowed by the next commit. `git add` the conflict list
  first; unwind a wrong split with `git reset --soft origin/main && git reset`.
- Origin's monolithic `.claude/MEMORY.md` (a branch cut before the split) conflicts with the whole
  index: take the index, then turn origin's new `### Title - date` block into an entry file with a
  `# Title — date` heading (`check.py` parses the em dash) and `python3 .claude/memory/index_line.py`.
- `j`,`j` out of the WORKSPACES BAR reads `App::bar_return`, which ⇧P / ⇧B can hide after the fact;
  `focus_walk.rs::bar_return_target` falls back to `first_sidebar_focus()` (no test yet).
- The harness blocks `sleep N; gh run list …` as a chained wait; ~30 s after the tag push a bare
  `gh run list --workflow=release.yml` already listed `headBranch v0.16.0`.
- The installed `~/.cargo/bin/nebula` is still **0.15.0** and the live daemon is on PROTOCOL VERSION
  30; the next install needs NEBULA KILL (or MAKE CYCLE) before the new TUI attaches.
