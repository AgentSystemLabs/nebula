# Pi (pi.dev) Is The Fourth AGENT KIND: Status Via A Managed Extension, RESUME By `--session-id` — 2026-09-04

**Asked:** "https://github.com/AgentSystemLabs/nebula/issues/24 try to add support in for the PI harness,
install it locally on my machines so we can verify it works in testing"
→ refined: Add pi (pi.dev — the `pi` coding agent CLI, npm `@earendil-works/pi-coding-agent`) as a fourth
AGENT KIND beside Claude, Codex and Cursor, per issue #24: the NEW SESSION PICKER, the AGENTS TAB (HARNESS
TOGGLE + MODEL / EFFORT rows), the QUICK PROMPT and AGENT PRESET kind lists, `nebula spawn --kind pi`, the
HARNESS BADGE, and the DAEMON's spawn / RESUME argv, with pi-flavoured MANAGED HOOKS (an env-guarded
extension installed globally under `~/.pi/agent/extensions/`, like the CODEX HOOK DIALECT) so AGENT STATUS,
AUTO-TITLE and CWD REPARENT work. Install pi on this Mac with npm (assuming this machine only — I can't
reach your others) and verify a real pi SESSION goes RUNNING → FINISHED under nebula. Keep Claude / Codex /
Cursor exactly as they are. (no questions asked)

**Did:** `AgentKind::Pi` (`nebula-core/src/entities.rs`: `ALL` is `[AgentKind; 4]`, `as_str` / `parse` /
`cli_program` all `pi`); PROTOCOL VERSION 35 → 36 (an old client cannot decode the new variant).
DAEMON: `registry.rs::agent_spawn_command_with` — `pi [--session-id <sid>] [--model m] [--thinking e]
--append-system-prompt <guidance> [prompt]`, no permission flag (pi has no gate), the Claude system-prompt
composition extracted into `push_system_prompt` and shared; the hook-install arm calls the new
`hooks/pi_extension.rs::install(pi_agent_dir())`, which writes `nebula_pi_extension.ts` (beside it,
`include_str!`) into `<$PI_CODING_AGENT_DIR | ~/.pi/agent>/extensions/nebula.ts` only when the content
drifts; `complete_pending_move` now hands the RELOCATION PROMPT to pi as well as Claude; `pr_scope.rs`
gives pi the PR rule as a system prompt like Claude. The extension maps `session_start` → `SessionStart`,
`before_agent_start` → `UserPromptSubmit` (its reply body, the shared `hookSpecificOutput` envelope, is
appended to that run's `systemPrompt`), `agent_end` → `Stop`, the `ask_question` tool's start / end →
`PreToolUse` / `PostToolUse`, and a mid-run `ui_prompt_start` / `_end` → `PermissionRequest` /
`PostToolUse{ask_question}`; it returns at once without `NEBULA_AGENT_ID` / `NEBULA_API_URL`.
`hooks/mod.rs` routes `/api/hooks/pi` through `receive_injectable_hook`; `status.rs::asks_user` treats
`ask_question` like `AskUserQuestion`; `sibling.rs` guidance says `--kind claude|codex|cursor|pi`.
TUI (`config.rs`): `PI_MODELS` (fuzzy `--model` patterns: `opus`, `sonnet`, `haiku`, `gpt-5.5`),
`PI_EFFORTS` (`off` … `max`), `pi_model` / `pi_effort` / `pi_enabled` SETTINGs with `SettingKind::Pi*`
rows in a `Pi` group on the AGENTS TAB, `AGENT_KIND_NAMES` + every per-kind match; `kind_label` (now in
`agent_picker.rs`, which another session extracted mid-task and patched `Pi => "Pi"` into itself). Seven
existing tests that counted three harnesses or switched every one off gained pi; new tests in
`pi_extension.rs` (3), `status.rs`, `registry.rs`, `pr_scope.rs`, `hooks/mod.rs`, `config.rs`; the e2e
`create_agent_refuses_when_the_cli_is_not_installed` table got `(13, Pi, "pi")`. DOCS PAGES (README,
`docs/sessions.md`, `how-it-works.md`, `configuration.md`, `commands.md`) updated. Gate: nebula-core 12,
nebula-daemon 171, nebula-tui 590, that one e2e, clippy and fmt clean. Installed pi 0.85.0 with
`npm install -g --ignore-scripts @earendil-works/pi-coding-agent` on this Mac (`command -v pi` resolves
through the LOGIN SHELL WRAP); the user's `~/.pi/agent/auth.json` is empty, so the live proof ran an
isolated profile (`PI_CODING_AGENT_DIR=<scratch>/pi-agent` with a `models.json` pointing a mock
`openai-completions` provider at a Python server on loopback) under an isolated DEV-style daemon
(`NEBULA_RUNTIME_DIR=/tmp/nbpi`, `NEBULA_DATA_DIR=<scratch>/data`, `target/debug/nebula` in tmux):
`n` → the 4-row picker → Pi → the row badged `pi`, `nebula.ts` written byte-identical, DAEMON LOG
`SessionStart{startup}` → `UserPromptSubmit` → `Stop`, the SQLITE STORE row `fresh` → `finished` with the
pi session id captured, the mock's system prompt carrying the AUTO-TITLE INSTRUCTION plus both guidance
blocks; CONTEXT MENU **Restart** respawned `--session-id <sid>` and the next prompt reached the model
with the earlier exchange in its history. Support for the pi harness is installed and verified locally on this machine only — the user's other
machines are unreachable from here, and each needs the same npm install plus a pi provider login before real
testing. Not done: `make install` / MAKE CYCLE (kills the user's live SESSIONS) and that provider login —
both the user's. Not committed (SHARED CHECKOUT).

**Gotchas:**
- **The npm package moved.** pi.dev's install line is `npm install -g --ignore-scripts
  @earendil-works/pi-coding-agent` (0.85.0); `@mariozechner/pi-coding-agent` (0.73.1) is deprecated and
  points there. The binary is `pi`, `bin` → `dist/bundle/cli.js`, the real code is in `dist/bundle/chunks/`.
- **`pi --session-id <uuid>` is the RESUME shape, not `--session`.** It resumes an existing id with the
  whole history and takes a trailing positional prompt (both verified in `-p` and interactively), and a
  missing id is *created* with a one-line warning instead of dying — so a relocated session's new cwd never
  hits the `arm_resume_fallback` path. `--session <partial>` and the id lookup both search only the cwd's
  own dir (`findLocalSessionByExactId(sessionId, cwd, sessionDir)` over
  `~/.pi/agent/sessions/<cwd-encoded>/*.jsonl`), which is why history does not follow a WORKTREE RELOCATION.
- **pi has no shell hooks; extensions are `.ts` files loaded without a type-check.** Global
  `~/.pi/agent/extensions/*.ts` load with no trust prompt; a worktree-local `.pi/extensions/` asks on
  every fresh checkout (the CODEX HOOK DIALECT trap again) — hence the global install and no `--approve`
  from nebula (a hostile branch's `.pi/extensions/` would run). Keep the file free of imports
  (`pi: any`): `import type` from the package would tie it to pi's install path.
- **The first *interactive* pi session in a project stops at pi's own "Trust project folder?" prompt**
  (`-p` runs never show it); answer it once per directory — it is pi's, not nebula's, and it happens before
  extensions load, so no `ui_prompt_start` reaches the daemon for it.
- **`agent_end` fires on abort and error too**, so a cancelled pi turn sends `Stop` — the one end-of-turn
  signal Claude's hooks never send. `before_agent_start`'s returned `{ systemPrompt }` is the injection
  channel (`result.systemPrompt !== undefined` replaces the run's prompt). With an instant model, `Stop`
  lands ~30 ms after `UserPromptSubmit`, so the row skips visibly through RUNNING.
- **Driving a real pi turn with no credentials:** `PI_CODING_AGENT_DIR` moves `~/.pi/agent` (tilde
  expanded; the installer honours it), `PI_OFFLINE=1` skips startup network, and a `models.json` custom
  provider (`api: "openai-completions"`, `baseUrl` on loopback) whose model `cost` carries `cacheRead` and
  `cacheWrite` — without those two the file is rejected with a warning and `--list-models` prints
  `No models available`, and `-p --model mock/echo` errors `Model "mock/echo" not found`. A ~40-line Python
  SSE server answering `chat/completions` is enough; `settings.json` `defaultModel` picks it without `--model`.
- **A new `AgentKind` variant turns every concurrent session's build red the moment it lands.** Another
  session extracted `kind_label` into `agent_picker.rs` and added `Pi => "Pi"` itself while I worked, and
  `config.rs` gained `claude_catalogue::models()` between my read and my write; an exact-once replacer that
  aborts a file when any anchor is stale (nothing written) is what kept the two edits from clobbering each
  other — re-read anchors right before each write on a SHARED CHECKOUT.
- macOS has no `timeout`; a hung CLI probe is best run through `python3 -c "subprocess.run(..., timeout=)"`.
  The LOGIN SHELL WRAP prints the user's `No .nvmrc file found` into pi's pane — the user's nvm shell init, not nebula.
