# "Do This In A Worktree" Goes Through `nebula worktree`, Not A Button — 2026-08-26

**Asked:** "remove the move to worktree button and instead find a better way to hook into when a user
prompts for a worktree, claude via a skill + system prompt or something knows to create the proper
worktree in nebula and assiocate the sesion with it"

**Did:** The Sessions context-menu verb "Move to worktree" and its picker are gone
(`MenuAction::MoveAgent`/`MoveAgentToWorktree`, `open_move_agent_picker` in
`crates/nebula-tui/src/event_loop.rs`); `ClientRequest::MoveAgent` stays as the daemon primitive
(e2e `move_agent_respawns_live_session_in_target_worktree` still covers it). In its place:

- **CLI** `nebula worktree [name…] [--base <ref>]` (`crates/nebula/src/main.rs`,
  `ipc::enter_worktree_for_current_agent`) — same `NEBULA_AGENT_ID` + socket path as `nebula rename`;
  spaces slugify, no name = `branch_name::random_name`. Sends the new `ClientRequest::EnterWorktree`,
  gets `ServerEvent::WorktreeEntered { worktree, outcome: EnterOutcome }` back. Its stdout is written
  for the model that ran it ("finish now, you'll be resumed inside the worktree").
- **Daemon** `Daemon::enter_worktree` (`registry.rs`): existing branch row or `create_worktree` (nebula's
  `<repo>-worktrees/<branch>` layout), `set_agent_worktree` + broadcast **immediately**, and — only if
  the PTY is alive — an entry in the new `pending_moves` map. `complete_pending_move`, called from the
  hook drain loop in `lib.rs`, does the kill + `claude --resume <sid> … "<relocation prompt>"` respawn on
  `Stop` (or `Notification idle_prompt`). Any other spawn of the agent clears its pending entry.
- **Claude guidance** rides `--append-system-prompt` on every non-cloud claude spawn
  (`CLAUDE_WORKTREE_GUIDANCE`, `agent_spawn_command_with`): don't use `EnterWorktree`, run
  `nebula worktree <name>`, then end the turn. Installer adds `Bash(nebula worktree:*)` to the allow list
  next to the rename rule (`CLAUDE_ALLOW_RULES`). Rejected a `~/.claude/skills` install: a skill is only
  loaded on description match and would live outside nebula's per-spawn hook management.
- **TUI** follows a daemon-initiated re-home: an agent upsert whose `worktree_id` changed for the
  *selected* session sets `select_when_seen` (event_loop.rs `Entity::Agent` arm), so the cursor and pane
  ride along instead of landing on whatever slid into the slot. Also helps the hook-cwd reparent.

Tests: `enter_worktree_*` + `pending_relocation_ignores_the_old_cwd_until_the_turn_ends` (registry),
`spawn_command_initial_prompt_is_claudes_positional_argument`,
`selection_follows_the_selected_agent_when_the_daemon_rehomes_it` (TUI), and e2e
`nebula_worktree_cli_relocates_the_session_when_the_turn_ends`. README updated.

**Gotchas:**
- **The CLI the model runs *is* the session's foreground tool call.** `move_agent`'s kill-and-respawn
  can't be reused directly — it would cut claude off mid-turn with a dangling tool_use. Hence the
  two-phase design: row now, process at the turn's `Stop`. Ordering in the `lib.rs` drain loop matters:
  `reparent_agent_by_cwd` runs *before* `complete_pending_move`, because the `Stop` payload itself still
  carries the old checkout's cwd and must be ignored (pending guard in `try_reparent_agent_by_cwd`)
  before the pending entry is consumed. The e2e posts a `PostToolUse` + `Stop` with the old cwd to pin
  this down.
- **Claude can't `cd` out of its start directory** (hook cwd is reset outside the workspace root — see
  the 08-23 EnterWorktree experiment in the user's auto-memory), so a restart is the *only* way to put
  the process in nebula's sibling worktree layout. `claude --resume <sid> "<prompt>"` is the documented
  resume-with-initial-prompt shape and `--append-system-prompt` is listed for interactive use in
  `claude --help` (2.1.246); the argv shape is unit-tested, but **the live auto-continue after a resume
  was not exercised in this session** — first thing to watch when trying it for real. Codex/cursor get no
  continuation prompt (unknown whether their resume takes one); they come back idle.
- **Proving a respawn landed in the right directory without attaching:** the e2e stub agent does
  `pwd >> $NEBULA_AGENT_ID.pwd` on every boot, so "one line, then two lines with the second ending in
  `repo-worktrees/feat-x`" is the whole assertion — cheaper than the Attach/Input/`pwd` dance the
  MoveAgent e2e uses.
- `agent_spawn_command` (the 5-arg form) is now `#[cfg(test)]`: production goes through
  `agent_spawn_command_with(.., initial_prompt, guidance)`, and clippy flagged the wrapper as dead.
