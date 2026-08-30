# PR #22 — Fix FOCUS TINT reading as black instead of gray (issue #6)

- **URL:** https://github.com/AgentSystemLabs/nebula/pull/22
- **Author:** @webdevcody
- **Merged:** 2026-08-29T01:30:20Z by @webdevcody (`d5a152c61863`)
- **Opened:** 2026-08-29T01:30:12Z
- **Branch:** `claude/issue-6-20260829-0120` → `main`
- **Closes:** #6
- **Diff:** +30 −8 across 1 file(s)

## Description

> Fixes #6.
>
> Every theme preset's `focus_tint` color was an almost-pure-black truecolor RGB, so `draw_focus_tint` painted untouched cells — including the rounded pad-row corners of a selected pill row — as flat black instead of a visible gray tint. Lifted every preset's `focus_tint` to a real gray floor and added a regression test.
>
> ⚠️ `cargo test` could not be run in this headless session (approval-gated) — please run the test suite before merging.
>
> Generated with [Claude Code](https://claude.ai/code)

## Changed files (1)

- `crates/nebula-tui/src/theme.rs` +30 −8

## Commits (1)

- `f2966e1b37b1` Fix FOCUS TINT reading as black instead of gray (issue #6) — @github-actions[bot], @webdevcody

## Conversation (0)

_(no issue comments)_

## Reviews (0)

_(no review submissions)_

## Inline review comments (0)

_(no inline comments)_
