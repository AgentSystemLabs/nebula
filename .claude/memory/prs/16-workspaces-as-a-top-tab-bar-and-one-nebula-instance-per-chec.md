# PR #16 — Workspaces as a top tab bar, and one nebula instance per checkout

- **URL:** https://github.com/AgentSystemLabs/nebula/pull/16
- **Author:** @webdevcody
- **Merged:** 2026-08-27T01:05:17Z by @webdevcody (`0361f0a3495f`)
- **Opened:** 2026-08-27T00:55:58Z
- **Branch:** `worktree-workspace-tabs` → `main`
- **Diff:** +846 −346 across 12 file(s)

## Description

> Two independent things landed on this branch: the Workspaces column became a top tab bar, and running a
> nebula checkout per worktree stopped colliding with itself.
>
> ## The Workspaces column becomes a top tab bar
>
> `WORKSPACES` now sits across the top of the body on the same `x=3` / row-1 grid as the panel headers, so
> it reads as the tier directly above `PROJECTS`, with one tab per workspace to its right. The rule closing
> the bar off breaks under the open tab, joining it to the panels below.
>
> - Tabs answer to `⌘1`–`⌘9` and to the bare digits `1`–`9`. `⌘` is what the tabs advertise, but Terminal.app
>   (and most other emulators) never encode it into pty bytes, so the bare digit is the binding that
>   actually fires. Both are rebindable per slot in Settings → Hotkeys.
> - `←/→` switch once the bar has focus; `↓` or `Enter` steps down into Projects; a click works too.
> - Splitters reindex to `0..2` — a full-width bar owns no vertical boundary, so the column's
>   drag-to-resize and its persisted width are gone.
>
> ## One checkout per worktree, without the collisions
>
> Running a checkout per worktree meant three collisions, only one of them a port:
>
> - **`nebula browser` hard-failed on a busy 7681.** It now prefers 7681, falls back to a free port and says
>   which, and takes `--port 0` as "any free one" instead of refusing it. An explicitly named `--port N` is
>   still that port or an error — silently moving would break an ssh tunnel pointed at the number.
> - **`make dev` and `make browser` shared one runtime dir across every checkout**, so the second TUI attached
>   to the first's daemon and drove the other checkout's binary — and `dev-prep` SIGTERMed whichever daemon
>   got there first. Runtime and data dirs are now keyed to a hash of the checkout path.
> - **`make browser` pinned `PORT=7681`.** It now passes no `--port` at all unless you set one.
>
> Adds `make dev-ls` to see every checkout's instance, since slots outlive the worktrees that made them.
>
> ## Notes
>
> - `origin/main` (v0.10.0) is merged in. Two trivial conflicts, both resolved by keeping both sides; one
>   semantic break the merge couldn't see — main removed the project divider fields, so the
>   `seed_many_workspaces` test helper no longer sets them.
> - `make ci` is green: fmt, clippy, and 631 tests across the workspace.
>
> 🤖 Generated with [Claude Code](https://claude.com/claude-code)

## Changed files (12)

- `.claude/MEMORY.md` +119 −0
- `Makefile` +42 −13
- `README.md` +11 −7
- `crates/nebula-tui/src/app.rs` +29 −64
- `crates/nebula-tui/src/config.rs` +2 −2
- `crates/nebula-tui/src/event_loop.rs` +261 −163
- `crates/nebula-tui/src/keymap.rs` +32 −2
- `crates/nebula-tui/src/ui.rs` +231 −72
- `crates/nebula/src/browser.rs` +97 −14
- `crates/nebula/src/main.rs` +6 −3
- `crates/nebula/tests/browser_cli.rs` +13 −3
- `crates/nebula/tests/e2e_tui.rs` +3 −3

## Commits (4)

- `30042e9c6ea0` Prototype: the Workspaces column becomes a top tab bar — @webdevcody, @claude
- `eb91435d3380` Merge origin/main (v0.10.0) into the workspace-tabs prototype — @webdevcody, @claude
- `6f30a5536c0d` Let nebula browser pick its port, and key the dev instance to the che… — @webdevcody, @claude
- `c94e6dee9074` Log the v0.10.0 merge and the per-checkout port/dev-slot work — @webdevcody, @claude

## Conversation (0)

_(no issue comments)_

## Reviews (0)

_(no review submissions)_

## Inline review comments (0)

_(no inline comments)_
