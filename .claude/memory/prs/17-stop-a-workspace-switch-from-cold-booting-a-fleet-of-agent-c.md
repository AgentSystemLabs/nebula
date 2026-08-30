# PR #17 — Stop a workspace switch from cold-booting a fleet of agent CLIs

- **URL:** https://github.com/AgentSystemLabs/nebula/pull/17
- **Author:** @webdevcody
- **Merged:** 2026-08-28T02:23:51Z by @webdevcody (`5b662984ada8`)
- **Opened:** 2026-08-28T02:13:09Z
- **Branch:** `worktree-workspace-switch-lag` → `main`
- **Diff:** +774 −61 across 8 file(s)

## Description

> ## Summary
>
> Switching workspaces could leave the terminal pane blank for 5–10s. Three compounding causes, all confirmed against the live dev daemon log and DB:
>
> 1. **Every switch attached a dead session.** `session_idle_timeout` (5m) reaps everything in a workspace nobody is looking at, so switching back always landed on a reaped session and the daemon cold-spawned `zsh -l -i -c 'exec claude --resume …'` — with an empty replay, so the pane showed nothing. One fresh `claude` under a login shell measures 0.67s to first byte, 1.47s to a painted screen.
> 2. **250ms later the prewarm booted every other dead session in the worktree, inline** on the connection's request loop. Five agents → five concurrent CLI boots contending with the one the user was waiting on, and that client's `Input` frames stalled for the whole burst.
> 3. **`attach` had no debounce.** In the Workspaces column every cursor step is a full `switch_workspace`, so walking past four workspaces cold-booted four CLIs and abandoned three.
>
> ### Fixes
>
> - **Daemon:** `ensure_session` holds a `spawn_gate` across its check-and-install, so an Attach and the prewarm sweep racing the same dead session fork one CLI, not two. That lets the sweep leave the request loop: `run_worktree_prewarm` runs on its own task, boots one session per `PREWARM_STAGGER` (1.5s), skips ones already alive, and is aborted by a newer sweep.
> - **TUI:** selection-driven attaches wait out `ATTACH_DEBOUNCE` (180ms). The pane swaps immediately; only the `Attach` waits. Explicit picks — Enter, click, menu, palette, a freshly created session, the post-delete reconcile, and the boot snapshot — go through `attach_now` and don't wait. `attached_sref` tracks what the daemon actually holds while `term.sref` runs ahead.
> - **UI:** the pane says `starting…` instead of rendering a blank grid, so a boot no longer reads as a hang.
>
> ### Also in this PR
>
> **Keep a worktree row on its branch while a rebase is paused** (`95c0a18`). Found while rebasing this branch: a rebase parks HEAD on the commits it replays, so `git worktree list --porcelain` prints `detached` for as long as it sits on a conflict. The worktree sync renamed the row to `detached @ <sha>` and back — and since `nebula worktree <name>` finds rows by branch string, it would have tried to create a *second* checkout for the branch in the meantime. `list_worktrees` now reads `<git-dir>/rebase-merge/head-name` (the same file `git status` uses to say "rebasing branch X") before falling back to the detached label.
>
> **Wait for the upsert in `workspace_scope_is_per_connection`.** The e2e test read events until the `AddProject` Ack and expected the project upsert among them, but the daemon writes the Ack from the request loop and the upsert from the broadcast forwarder and promises no order (the TUI handles either). The memory log had this test losing that race twice under load; a timing shift from the `git.rs` change made it lose ~60% of idle runs even though the new code path never executes in that test. It now waits for both, as `cli_add_project` already did — 10/10 after, 4/11 before.
>
> ### Rebase notes
>
> The branch was cut at v0.9.0 and rebased onto v0.13.0, then again onto `8f56ca6`. Two semantic conflicts, both resolved the same way — main factored a path into a helper, and the branch's hook moved into the helper so it covers every caller:
>
> - `main` added `mark_agent_seen` at the top of `attach()`; after the debounce split that funnel is `attach_inner`, so the call moved there — keyed to the pane swap, not the Attach, since the user is reading the screen during the debounce.
> - `main` extracted the input-lock into `enter_terminal_pane`, shared by Enter and the new Tab/^⇧L panel walk. `fire_pending_attach` now lives inside it, so landing on the pane by either route settles a debounced attach before the first keystroke.
>
> ## Test plan
>
> - [x] `cargo test --workspace --no-fail-fast` — 693 passed, 0 failed, exit 0 on the rebase onto `8f56ca6` (all binaries incl. e2e_pty 25 / e2e_tui 5)
> - [x] New: `walking_the_workspaces_column_attaches_only_where_it_stops`, `a_paused_rebase_keeps_the_worktree_on_its_branch`
> - [x] `workspace_scope_is_per_connection` 10/10 idle runs
> - [x] `cargo fmt --check` clean; clippy shows only the 7 warnings already on `main` (verified the warned lines are untouched by this diff)
> - [ ] Manual: switch between workspaces in the live TUI — expect `starting…` instead of a blank pane, and one CLI boot per stop rather than per row passed
>
> 🤖 Generated with [Claude Code](https://claude.com/claude-code)

## Changed files (8)

- `.claude/MEMORY.md` +133 −2
- `crates/nebula-daemon/src/git.rs` +107 −12
- `crates/nebula-daemon/src/registry.rs` +68 −4
- `crates/nebula-daemon/src/server.rs` +5 −5
- `crates/nebula-tui/src/app.rs` +26 −0
- `crates/nebula-tui/src/event_loop.rs` +385 −37
- `crates/nebula-tui/src/ui.rs` +27 −0
- `crates/nebula/tests/e2e_pty.rs` +23 −1

## Commits (7)

- `5950a15e0043` Stop a workspace switch from cold-booting a fleet of agent CLIs — @webdevcody, @claude
- `9111cd4f20c6` Log the workspace-switch cold-boot investigation — @webdevcody, @claude
- `3662c30d0715` Rebase the workspace-switch fix onto v0.13.0 — @webdevcody, @claude
- `d49e0ad89431` Keep a worktree row on its branch while a rebase is paused — @webdevcody, @claude
- `7bbb65f5e15f` Log the paused-rebase worktree relabel — @webdevcody, @claude
- `1d1386d55311` Wait for the upsert in workspace_scope_is_per_connection — @webdevcody, @claude
- `18555ed8ae1d` Log the Ack race that the git.rs change surfaced — @webdevcody, @claude

## Conversation (0)

_(no issue comments)_

## Reviews (0)

_(no review submissions)_

## Inline review comments (0)

_(no inline comments)_
