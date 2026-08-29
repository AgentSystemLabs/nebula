# Fixing A Fork PR's Conflicts From The Shared Tree, Then A Security Audit Of Its Diff — 2026-08-28

**Asked:** "fix the conflicts on https://github.com/AgentSystemLabs/nebula/pull/20, then do a pr security audit
review skill" (headless session — `prompt-daddy` skipped, worked from the prompt as typed).

**Did:** PR #20 (`lnmunhoz:toggle-column`, the PROJECTS PANEL / WORKTREES PANEL toggles) was CONFLICTING
against the dedup pass (PR #18). Recipe, without touching the dirty shared tree: `git fetch origin
pull/20/head:pr-20-toggle-column` → `git worktree add <scratchpad>/pr20 pr-20-toggle-column` → `git merge
origin/main` there → resolve → `cargo fmt/clippy/test --workspace` with `CARGO_TARGET_DIR` in the scratchpad
(734 green, zero clippy warnings) → `git commit -F` → `git push git@github.com:lnmunhoz/nebula.git
HEAD:toggle-column` (no remote needed; `maintainerCanModify` was true) → `gh pr view --json mergeable` read
`MERGEABLE` ~8 s later → `git worktree remove` + `git branch -D`. Two files conflicted: `config.rs` (keep the
PR's `hide_projects` / `hide_worktrees` defaults + main's `DEFAULT_CHOICE`) and `event_loop.rs`, where #18's
extracted `next_focus() -> Option<Focus>` and the PR's `App::next_visible_focus` (skips hidden panels) landed on
the same lines in `Action::FocusTerminal` and `walk_focus_forward` — the PR's wins, and the now-unused
`next_focus` was deleted (merge `9c3a4df`). The `security-review` pass over the PR's diff found nothing: two
serde bools, layout/focus arithmetic, no new `ClientRequest`, no path or process construction.

**Gotchas:**
- **`security-review` snapshots the checkout it runs in.** Invoked from the shared tree on `main` it produced
  an empty diff and a commit list for `main`, not the PR. Write the PR's diff yourself (`git diff origin/main
  <merge-sha> -- . ':!.claude/MEMORY.md' ':!TERMS.md' ':!README.md'`) and hand that path, plus the scratch
  worktree, to the audit sub-agent.
- A scratch `git worktree add` registers in the shared repo's worktree list, so WORKTREE SYNC would surface it
  as a row within 2 s — remove it (`--force`, the target dir was built in) when the push is done.
- `cargo test --workspace --tests --bins --doc` prints nothing and runs nothing; plain `cargo test --workspace`
  covers all seven binaries plus doc-tests.
- The dedup pass left clippy at zero warnings; the "pre-existing warnings" the PR description mentions are gone,
  so a warning after a merge with main is yours.
