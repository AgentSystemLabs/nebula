# Claude's `/rename` And The SESSION Row Name Are Tied (Issue #25) — 2026-09-04

**Asked:** "work on fixing this issue https://github.com/AgentSystemLabs/nebula/issues/25" — the issue:
"session name should be tied to the agent name session … It'd be very useful if /rename (claude) a
session would also update the session name … I still use tons of /rename to have a better visibility
while using /rc, and for restarting the agent … It makes sense to tie these two "session" names together"
→ refined: When I run `/rename <name>` inside a claude SESSION, retitle its row in the SESSIONS PANEL to
that name, as if I had pressed RENAME (`r`) — a user-set title AUTO-TITLE never overrides — so the name I
see in `/resume`, `/rc` and after a restart matches the NEBULA row (issue #25). Keep AUTO-TITLE and `r`
RENAME exactly as they are. (Assuming the tie should hold both ways wherever Claude Code allows it
without typing into the PTY: a NEBULA title becomes the claude session title too.) Codex and cursor rows
are untouched.

**Did:** CLAUDE TITLE SYNC, both directions, Claude only. Claude → nebula: new
`crates/nebula-daemon/src/session_title.rs` — `TranscriptRef::read_title` reads
`<transcript dir>/<session id>/custom-title.json` (falling back to the last `{"type":"custom-title"}`
line in the transcript's 64 KB tail), `Daemon::sync_claude_title` adopts a title Claude did not hold
before through `Store::adopt_claude_title` (one conditional UPDATE against the new `claude_title` column,
migration 23), and `Daemon::on_pty_title` chases a new window title with reads at 0 / 300 ms / 1 s / 3 s.
The cue is a new `pty/title.rs` `TitleScanner` (OSC 0/2) → `PtyEvent::Title`, consumed in
`registry.rs::watch_for_exit`; the HOOK RECEIVER's `HookDelivery.transcript` (payload `transcript_path` +
`session_id`, Claude route only) is noted in the `lib.rs` drain loop, which also syncs on every hook.
The comparison is against the title last seen from Claude, never the row name, so a RENAME with `r`
survives re-reads. nebula → Claude: `hooks/mod.rs::user_prompt_reply` adds
`hookSpecificOutput.sessionTitle` to the Claude route's `UserPromptSubmit` reply whenever
`TitleState::to_push` finds a settled, non-`agent-N` row name Claude doesn't hold (routes split into
`HookCli::{Claude, Codex, Cursor}`). Rejected: reading the name off the window title (Claude's AI titles
ride the same OSC), `claude --name` at spawn (an older CLI would refuse the flag; an ignored reply field
costs nothing), a `SessionStart` push (display-only, see gotchas), typing `/rename` into the PTY. Docs:
`docs/how-it-works.md` bullet, README clause. Tests: `title.rs` (6), `session_title.rs` (7, one against a
real `Daemon`), `store.rs`, `hooks/mod.rs`, `pty/mod.rs`, E2E PTY `claude_session_title_and_row_name_stay_tied`;
`auto_title_instruction_and_rename_flow` now expects the push on the post-title prompt, and
`migration_22_adds_pr_context_without_backfill` asserts `MIGRATIONS.len()` instead of `22`. Verified live
in a DEV INSTANCE with Claude Code 2.1.261: AUTO-TITLE's `Say Pong Reply` appeared in Claude's prompt box
on the next prompt, and `/rename Live Sync Check` retitled the row within ~4 s. Gate: nebula-daemon 178
passed, e2e_pty 27 passed, clippy and fmt clean. Done in the `session-title-sync` WORKTREE because the
SHARED CHECKOUT had 22 dirty files from other sessions, `registry.rs` among them.

**Gotchas:**
- **`/rename` fires no hook at all.** A logging hook in a scratch project saw only `SessionStart`, the
  real prompts' `UserPromptSubmit` and `Stop`. Its footprints are the window title (`ESC ] 0 ; ✳ <name>
  BEL`, rewritten at once) and, beside the transcript, `<dir>/<session id>/custom-title.json`
  (`{"customTitle":…}`) plus `{"type":"custom-title"}` / `{"type":"agent-name"}` transcript lines,
  re-appended around every prompt. None of it is in the hooks docs; `grep -a customTitle` on the CLI
  binary (`~/.local/share/claude/versions/<v>`) is where the zod schemas are.
- **`hookSpecificOutput.sessionTitle` is an undocumented reply field.** On `UserPromptSubmit` it renames
  the session and persists it exactly like `/rename` (sidecar + transcript lines); on `SessionStart` it
  changes the prompt-box title only — after `--resume` the sidecar still held the old name.
- **The window title is a cue, not a source.** Claude's own AI summaries (`ai-title`) and the glyph flip
  around a permission prompt ride the same OSC, and the sidecar write is not ordered against the bytes —
  hence read-the-file with a few delayed retries, and `title_text` strips the glyph only to decide
  whether a read is worth it.
- Reading the title in tmux needs no byte parsing: `tmux display -p '#{pane_title}'` is the OSC title;
  a trust dialog in a scratch dir defaults to **No, exit** (send `Down` then `Enter`; the answer persists
  in `~/.claude.json`, so a later headless run in that dir sails through).
- E2E PTY: waiting on PTY output for a marker the *typed command* contains matches the shell's echo of
  the command line before it runs — split it in the typed text (`echo REPLY-D""ONE`, wait for `REPLY-DONE`).
- `migration_22_adds_pr_context_without_backfill` asserted `user_version == 22`, so any new migration
  failed it — it now asserts `MIGRATIONS.len()`; a new `PtyEvent` variant must also be added to
  `server.rs`'s forwarder match (exhaustive, no wildcard).
- A headless live check of a real agent: `make dev SEED=0` in tmux, then
  `NEBULA_RUNTIME_DIR=/tmp/nebula-dev-<slot> NEBULA_DATA_DIR=~/.nebula-dev/<checkout>-<slot>
  target/debug/nebula add <repo>` registers a project in the dev daemon (`<slot>` = `printf '%s' "$PWD" |
  shasum | cut -c1-8`), `p` launches a real claude, `z` locks the pane for typing, `Ctrl+q` returns to
  the panels, `make dev-stop` ends it.
- `echo =====` in zsh is `=cmd` expansion (`===== not found`) and aborts the chain — quote separators.
