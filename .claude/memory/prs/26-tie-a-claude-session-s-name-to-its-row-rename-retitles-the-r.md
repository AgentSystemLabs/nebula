# PR #26 — Tie a Claude session's name to its row: /rename retitles the row, the row's name reaches Claude

- **URL:** https://github.com/AgentSystemLabs/nebula/pull/26
- **Author:** @webdevcody
- **Merged:** 2026-09-05T03:30:01Z by @webdevcody (`08f71602a8a8`)
- **Opened:** 2026-09-04T23:01:22Z
- **Branch:** `session-title-sync` → `main`
- **Closes:** #25
- **Diff:** +1435 −43 across 15 file(s)

## Description

> Closes #25.
>
> ## What
>
> A Claude SESSION's own name and its row in the SESSIONS PANEL are now tied, both ways (CLAUDE TITLE SYNC; Claude only — Codex and Cursor have no session name of their own):
>
> - **`/rename <name>` inside Claude retitles the row** within a few seconds, as if you had pressed `r` — so AUTO-TITLE never overrides it.
> - **A name set in nebula reaches Claude on your next prompt** — typed at creation, set with `r`, or chosen by AUTO-TITLE — so `/resume`, `/rc` and Claude's prompt box show what the row shows, and a restart (`--resume`) keeps it.
>
> Whichever side changed last wins; a name set in nebula is never undone by re-reading Claude's older one.
>
> ## How
>
> Claude Code fires **no hook** for `/rename`. It rewrites the PTY window title (`✳ <name>`) and writes `custom-title.json` beside the transcript (`{"type":"custom-title"}` lines in the transcript too).
>
> - **Claude → nebula** — `crates/nebula-daemon/src/session_title.rs`. A new OSC 0/2 `TitleScanner` (`pty/title.rs`) emits `PtyEvent::Title`; the DAEMON then reads `<transcript dir>/<session id>/custom-title.json` (path from every hook payload's `transcript_path`), retrying at 0/300 ms/1 s/3 s since the file can land after the bytes, and also on every hook. `Store::adopt_claude_title` is one conditional UPDATE against a new `claude_title` column (migration 23): the title is adopted only when it differs from the one **last seen from Claude**, not from the row's name — that is what keeps an `r` rename from being reverted. The title text itself is never used as the name: Claude's AI summaries and the permission-prompt glyph flip ride the same OSC.
> - **nebula → Claude** — `hooks/mod.rs::user_prompt_reply`. The `UserPromptSubmit` reply gains `hookSpecificOutput.sessionTitle` (undocumented, in the CLI's schema; verified on 2.1.261 to rename and persist like `/rename`). Sent only on the Claude route, only for a settled non-`agent-N` name Claude does not hold yet, never while the AUTO-TITLE instruction is pending. A `SessionStart` push was rejected: it changes the display only.
>
> Rejected: reading the name off the window title; `claude --name` at spawn (older CLIs would refuse the flag, an ignored reply field costs nothing); typing `/rename` into the PTY.
>
> ## Verification
>
> - Live in a DEV INSTANCE with Claude Code 2.1.261: AUTO-TITLE's `Say Pong Reply` appeared in Claude's prompt box on the next prompt; `/rename Live Sync Check` retitled the row in ~4 s.
> - Tests: `pty/title.rs` scanner (6), `session_title.rs` (7, one against a real `Daemon`), store, receiver, PTY, and E2E PTY `claude_session_title_and_row_name_stay_tied`. `auto_title_instruction_and_rename_flow` now expects the push on the post-title prompt; `migration_22_adds_pr_context_without_backfill` asserts `MIGRATIONS.len()`.
> - `cargo test --workspace --no-fail-fast` green (851 tests, nine binaries), clippy and fmt clean, `make memory-check recall-eval terms-check` ok.
>
> ## Behavior you will notice
>
> Every titled Claude row names its Claude session on the next prompt, so `/resume` lists start showing nebula titles. Existing rows start unsynced and pick this up on their next prompt. Needs the DAEMON restarted on the new build.
>
> 🤖 Generated with [Claude Code](https://claude.com/claude-code)
>
> https://claude.ai/code/session_011Z4TLnGLTY5muye9PcAKsT

## Changed files (15)

- `.claude/MEMORY.md` +1 −0
- `.claude/memory/entries/2026-09-04-claude-s-rename-and-the-session-row-name-are-tied-issue-25.md` +66 −0
- `.claude/memory/gotchas.md` +7 −7
- `README.md` +3 −1
- `TERMS.md` +6 −4
- `crates/nebula-daemon/src/attach.rs` +3 −1
- `crates/nebula-daemon/src/hooks/mod.rs` +207 −25
- `crates/nebula-daemon/src/lib.rs` +9 −0
- `crates/nebula-daemon/src/pty/mod.rs` +58 −0
- `crates/nebula-daemon/src/pty/title.rs` +225 −0
- `crates/nebula-daemon/src/registry.rs` +16 −2
- `crates/nebula-daemon/src/session_title.rs` +476 −0
- `crates/nebula-daemon/src/store.rs` +154 −1
- `crates/nebula/tests/e2e_pty.rs` +194 −2
- `docs/how-it-works.md` +10 −0

## Commits (5)

- `c1caad84f348` Tie a Claude SESSION's own name to its row: /rename retitles the row,… — @webdevcody, @claude
- `69f60d58b550` Log the CLAUDE TITLE SYNC task and ledger the candidate — @webdevcody, @claude
- `e17340b22b6a` Merge main into session-title-sync: the Pi HOOK ROUTE meets the per-C… — @webdevcody, @claude
- `2b74ebbb5054` Merge origin/main (PRs #27 and #28) into session-title-sync — @webdevcody, @claude
- `515c101d30fb` Merge origin/main into session-title-sync: the PR SESSION WORKTREE ch… — @webdevcody

## Conversation (0)

_(no issue comments)_

## Reviews (0)

_(no review submissions)_

## Inline review comments (0)

_(no inline comments)_
