# Codex And Cursor As Agent Kinds — 2026-08-14 → 08-15

**Asked:** "add support for codex as well, so when a try to load up a new session using the n hotkey, show
a modal that let's me pick codex or claude, make sure the codex setup has the proper hooks or whatever
else instlaled like we do in claude so that the status indicators can properly reflect the state of th…"
Then: "also add support for cursor cli as a session option" and "run codex with --yolo mode on codex
sessions, same with cursor if it has a type of yolo flag see how we do it on mission-control."

**Did:** `AgentKind` + a picker modal (`5092684`, `986f505`), cursor-agent as a third kind (`f5ed97d`),
permissions always skipped for both (`89f9860`).

**Gotchas:**
- `claude` takes `--model <alias>` and `--effort <low|…|max>`; `codex` takes `-m/--model` but effort only
  via `-c model_reasoning_effort=<…>`; `cursor-agent` had no model/effort knobs **until cursor-agent
  2026.08.25 grew `--model <family>-<effort>` — superseded 2026-08-28, see
  `2026-08-28-cursor-agent-model-takes-a-family-effort-id-so-cursor-gets-mo`.** Pick lists are hardcoded
  in `crates/nebula-tui/src/config.rs` (`CLAUDE_MODELS`, `CODEX_MODELS`, `CURSOR_MODELS`) — "default"
  always means "pass no flag".
- Cursor has no PermissionRequest hook and nebula runs `cursor-agent --force`, so cursor agents report
  busy/idle but **never** needs-feedback. That is expected, not a bug.
