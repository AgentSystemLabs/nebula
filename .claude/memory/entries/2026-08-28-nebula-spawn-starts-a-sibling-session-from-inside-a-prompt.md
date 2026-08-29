# `nebula spawn "<task>"` Starts A Sibling SESSION From Inside A Prompt — 2026-08-28

**Asked:** "add the ability for me to say start a new nebula session in a prompt and it will know how to
call the daemon using the prompt the user made to run a new session auto,atically and it should show
up in the sessions list of the worktree I'm on"
→ refined: From inside a Claude SESSION that nebula is running, when I type something like "start a new
nebula session that ‹task›", the AGENT should know — from the same injected guidance that already
teaches it NEBULA RENAME and NEBULA WORKTREE — to run a new CLI subcommand (no TERM yet; assuming
`nebula spawn "<task>"`, with an optional `--kind claude|codex|cursor`) that asks the DAEMON to create a
new AGENT in the caller's own WORKTREE, with ‹task› as its STARTING PROMPT so it starts working at once.
The DAEMON finds the caller by the AGENT ENV (`NEBULA_AGENT_ID`) like `nebula rename` does; the new row
inherits the caller's AGENT KIND and MODEL / EFFORT (assuming that, unless `--kind` overrides), gets
AUTO-TITLE like any `n`-created row, and appears in the SESSIONS PANEL of the WORKTREE I'm on without
stealing FOCUS or the TERMINAL PANE from the session I'm typing in. Keep NEBULA RENAME and NEBULA
WORKTREE exactly as they are. (no questions asked — headless-style assumptions)

**Did:** NEBULA SPAWN, a third agent-side CLI verb beside NEBULA RENAME and NEBULA WORKTREE. `ClientRequest::
SpawnSiblingAgent { req_id, id, kind: Option<AgentKind>, starting_prompt }` (`nebula-core/src/
protocol.rs`), PROTOCOL VERSION 32 → **33**, answered with the ordinary `Ack { created }`. Daemon side is
a **new module** `crates/nebula-daemon/src/sibling.rs` — `impl Daemon` from outside `registry.rs` works
because `Daemon.store` / `.events` are `pub` and `create_agent` / `CreateAgentSpec` are `pub(crate)`, so
a new concern need not grow the 4.6k-line file (KEEP MODULES SMALL): `CLAUDE_SPAWN_GUIDANCE`,
`sibling_name` (first free `agent-N` in the caller's worktree — the TUI's `default_session_name` rule, so
`auto_title: true` is honest), `Daemon::sibling_spec` (caller's WORKTREE, AGENT KIND and MODEL / EFFORT;
`--kind` of another harness drops model/effort since a Claude model name means nothing to codex;
archived caller refused; no `cloud_prompt`, no `pr_url`) and `Daemon::spawn_sibling_agent` → the
existing `create_agent` (which validates the STARTING PROMPT and skips the PREWARM POOL). `registry.rs`
only gained one line: the spawn guidance is pushed after `CLAUDE_WORKTREE_GUIDANCE` into the same
`--append-system-prompt`. `server.rs` dispatches it with `launch_mode = "sibling"` (never the prompt
text); `hooks/installer.rs` `CLAUDE_ALLOW_RULES` += `Bash(nebula spawn:*)`. Client: `nebula spawn
<task…> [--kind claude|codex|cursor]` in `crates/nebula/src/main.rs` (clap `value_parser =
parse_agent_kind` over `AgentKind::parse`), `nebula_tui::run_spawn` → `ipc::spawn_sibling_for_current_
agent` (same `current_agent_id` + socket path as rename; blank task and no-daemon are nonzero exits;
success prints "started a new … session in this worktree … carry on" for the model to relay). The
TUI needed nothing: the row arrives as an `EntityUpserted` and lands in the SESSIONS PANEL without
SELECT-WHEN-SEEN, so FOCUS stays where it was. The guidance is Claude-only like WORKTREE GUIDANCE (codex
and cursor can run the command but nothing tells them to). README feature bullet + CLI list. Tests:
`sibling.rs` ×5, `guided()` / PR-prompt tests in `registry.rs` updated, e2e
`nebula_spawn_cli_starts_a_sibling_session_in_the_same_worktree` (STUB AGENT logs its env per boot:
sibling row `agent-2` alive in the caller's worktree, `--kind codex` → `agent-3`, `--kind gemini` and a
blank task fail, caller never rebooted). Gate: nebula-daemon 162, nebula-tui 520, nebula-core 12, E2E
PTY 26/26, clippy `-D warnings` and fmt clean. **Not done:** a live run against the real `claude` — the
installed daemon is still PROTOCOL VERSION 32, so it needs a MAKE CYCLE from a terminal outside nebula.

**Gotchas:**
- `CreateAgentSpec` is deliberately not `Debug` (it carries the prompt), so a test on a
  `Result<CreateAgentSpec>` cannot `unwrap_err()` — take `.err().expect(..)`; don't derive `Debug` to
  make the test compile.
- The registry test helper `guided()` spells the exact Claude `--append-system-prompt` argv, so every
  guidance constant added to `agent_spawn_command_with` must be joined into it in the same order
  (`"\n\n"`), or `spawn_command_per_kind_resume_shapes` fails on a string diff.
