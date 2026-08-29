# Cursor's Hooks Are Not Claude-Shaped — 2026-08-20

**Asked:** "cursor doesn't seem to update the status of the wortree or sessions when it is running, debug
and fix, verify it has hooks, if not, then setup some type of skill that is injected to cursor as a system
prompt or something so that it knows how to phone home to nebula to update the status"

**Did:** `install_cursor_hooks` in `hooks/installer.rs:260` is its own writer (plus a migration purge of
nebula groups under every key), and the installer maps cursor event names onto Claude-equivalent
`hookEvent` query values so `parse_event` stays single-dialect. `HookPayload` in `hooks/mod.rs` grew
aliases.

**Gotchas:**
- The installer originally assumed "same hooks JSON shape across all three CLIs". Cursor **silently
  ignored** the PascalCase Claude-shaped groups, so no status ever phoned home — no error, just nothing.
- Cursor's dialect: camelCase events (`sessionStart`, `beforeSubmitPrompt`, `stop`, `subagentStart/Stop`,
  `sessionEnd`), **flat** `{"command": …}` entries (no nested `hooks` array, no `type`), and a required
  top-level `"version": 1`. Hooks must print `{"continue": true}` to stdout or gating events degrade.
- Payloads carry `session_id` == `conversation_id` (the `--resume` chatId), have **no `cwd`** (use
  `workspace_roots[0]`), and subagent hooks use `subagent_id`, not `agent_id`.
- `beforeSubmitPrompt` and `stop` fire **only in interactive TUI mode**. A `-p` print-mode test fires only
  sessionStart / tool hooks / afterAgentThought / sessionEnd — **never conclude hooks are broken from a
  `-p` test**. To drive one interactively: pipe timed keystrokes through
  `script -q /dev/null cursor-agent --force --trust`.
