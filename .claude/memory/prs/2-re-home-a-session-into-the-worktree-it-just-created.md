# PR #2 — Re-home a session into the worktree it just created

- **URL:** https://github.com/AgentSystemLabs/nebula/pull/2
- **Author:** @webdevcody
- **Merged:** 2026-08-22T17:30:27Z by @webdevcody (`869acb033683`)
- **Opened:** 2026-08-22T17:03:00Z
- **Branch:** `move-worktrees-after-create` → `main`
- **Diff:** +221 −8 across 1 file(s)

## Description

> ## The bug
>
> Start a session in the main checkout, ask it to create a worktree and work there — the session's row stays under the main checkout instead of moving into the new worktree.
>
> ## Cause
>
> `reparent_agent_by_cwd` re-homes an agent row when a hook payload reports a cwd inside a different worktree of the same project. But it only matches against worktree rows already in the store, and a checkout the agent created itself isn't adopted until the background sync's next tick (up to 2s away).
>
> The agent reports its new cwd on the very next hook event — usually the `Stop` that ends the same turn — which beats the sync. The reparent found nothing to match, silently no-opped, and nothing re-ran it once the row appeared, so the row sat under the old checkout until the user's next prompt. Whether it worked at all came down to which of the two landed first, which matches the daemon log: three successful re-homes over two days, with many more attempts.
>
> ## Fix
>
> Remember each agent's last hook-reported cwd, and replay it from `sync_project_worktrees` whenever that sync adopts a checkout. The row then follows the session as soon as the worktree it moved into is known.
>
> Two guards keep the remembered cwd from doing damage:
>
> - It's only recorded after the existing foreign-session gate, so a nested CLI in the agent's PTY can't seed it.
> - A deliberate `move_agent` drops it. That call kills and respawns the PTY in the target so the process and the row agree; without dropping the cwd, the next adoption anywhere in the project would replay the stale value straight back over the user's choice.
>
> ## Tests
>
> Two new tests in `crates/nebula-daemon/src/registry.rs`, both verified to fail without the corresponding half of the fix:
>
> - `worktree_sync_replays_a_cwd_reported_before_adoption` — real git repo, real `git worktree add`; covers the hook-lands-first race and the stale-cwd-after-`move_agent` hazard.
> - `cwd_replay_skips_other_projects_and_archived_agents` — the replay stays scoped to the synced project and leaves archived rows alone.
>
> Full workspace suite green (396 tests).
>
> 🤖 Generated with [Claude Code](https://claude.com/claude-code)

## Changed files (1)

- `crates/nebula-daemon/src/registry.rs` +221 −8

## Commits (1)

- `7570387938cd` Re-home a session into the worktree it just created — @webdevcody, @claude

## Conversation (0)

_(no issue comments)_

## Reviews (0)

_(no review submissions)_

## Inline review comments (0)

_(no inline comments)_
