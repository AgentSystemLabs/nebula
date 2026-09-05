# The PR SESSION Picker Offers Every AGENT KIND Through One Shared Kind Picker — 2026-09-04

**Asked:** "when a user creates a new session when focused on a pr link, it only show claude for some
reason, we should be showing all harness options similar to the create session modal, find a way to dry
up and reuse logic"
→ refined: `n` (or the CONTEXT MENU) on a PROJECT OPEN PRS GROUP row opens a PR SESSION picker that lists
only Claude, because `CreatePrAgent` hardcodes Claude and the DAEMON delivers the PR rule through Claude's
`--append-system-prompt`. Offer every enabled AGENT KIND there, exactly as the NEW SESSION PICKER does
(HARNESS TOGGLE respected, MODEL / EFFORT submenus, naming step). Build all three kind menus (NEW SESSION
PICKER, PR SESSION, QUICK PROMPT `Tab`) from one shared helper. (Assuming Codex and Cursor get the PR rule
as their positional STARTING PROMPT on the cold spawn, since they have no system-prompt flag, and
`CreatePrAgent` gains a `kind`, bumping the PROTOCOL VERSION.) Keep Claude's launch exactly as it is.
(no questions asked)

**Did:** The "some reason" was two hardcodes: `ClientRequest::CreatePrAgent` had no `kind` (`server.rs`
filled in Claude) and `registry.rs::create_agent` bailed on `pr_url` for any other kind, because the PR
rule only had Claude's `--append-system-prompt` to ride on. Now: `protocol.rs` `CreatePrAgent { kind }`
(PROTOCOL VERSION 34 → 35; another session's bump the same hour left the tree at 36 — one shared bump).
New `crates/nebula-daemon/src/pr_scope.rs`: `rule(url)` (the text that was `claude_pr_system_prompt`),
`launch_prompts(kind, resumed, pr_url, initial) -> LaunchPrompts { system, initial }` — Claude keeps the
rule in the system prompt on every spawn; every other kind gets it as the positional first prompt of a
*cold* spawn only (`rule_as_first_prompt` adds "context, not a task: acknowledge … and wait"), a resume's
transcript already holds it — and `validate_pr_url` (moved; `normalize_url` went `pub(crate)`).
`spawn_agent_session_with` calls it with `agent.session_id.is_some()` as `resumed`. New
`crates/nebula-tui/src/agent_picker.rs`: `KindPicker::{new_session, pr_session, quick_prompt}`,
`open_kind_picker` (one `ContextMenu` from `enabled_kinds()`, `NO_HARNESS_FLASH` when empty),
`kind_rows`, `pr_session_menu_rows` ("New Claude session" / "New Codex session" / …) and `kind_label`
(moved out of `event_loop.rs`). `event_loop.rs`: `open_new_agent_picker`, `open_pr_agent_picker` and both
OPEN PRS CONTEXT MENU sites (`pr_row_menu_items`) go through it; `claude_enabled()`,
`CLAUDE_DISABLED_FLASH` and `pr_agent_menu_item` are gone; `create_agent` sends the picked kind.
`quick_prompt.rs::open_launch_picker` is one line. FOOTER hint on a PR row reads `n: new session`.
Docs: README, `docs/keys.md`, `docs/sessions.md`, `docs/how-it-works.md`, `docs/configuration.md`.
Tests: `pr_scope` unit tests, `pr_launch_context_is_accepted_for_every_kind_but_never_with_cloud`
(registry), `agent_picker` tests, and the PR-row tests in `event_loop.rs` rewritten to derive their
expectations from `enabled_kinds()` / `AgentKind::ALL`. Gate: nebula-core 12 + nebula-daemon 171 green on
the SHARED CHECKOUT (with the other session's Pi work in); nebula-tui 586 green before `AgentKind::Pi`
landed, then the whole change re-verified on a `git archive HEAD` copy in the scratchpad: core 12, daemon
167, tui 574 green, clippy clean (see Gotchas).
Not committed (SHARED CHECKOUT).

**Gotchas:**
- **Another session added `AgentKind::Pi` mid-task and then edited my untracked `pr_scope.rs` directly**
  (Pi joins Claude on the system-prompt arm, with its own test). The exhaustive `match kind` in the two new
  files became compile errors that were mine to cover even though the variant was not; `launch_prompts`
  takes a `_` arm (any CLI without a system-prompt flag opens with the rule), `kind_label` stays
  exhaustive on purpose. With `config.rs` still missing its Pi arms the shared tree could not build
  nebula-tui, so verification moved to `git archive HEAD | tar -x` in the scratchpad with every edit
  replayed from exact-match Python scripts and the Pi references stripped — keep edits as scripts on a
  day like this; a `git diff` cannot separate your hunks from theirs.
- `ContextMenu::hovered_claude_cloud` keys the cloud `Tab` on the literal title `"New session"`, so the PR
  SESSION and QUICK PROMPT pickers share the rows but never offer CLOUD LAUNCH — correct today (the
  DAEMON refuses `pr_url` + cloud), and the reason a fourth surface that wants cloud must carry that title
  or extend the check. Now said at the trap site.
- The two OPEN PRS CONTEXT MENU sites disagreed when the PROJECT had no ROOT WORKTREE: the key path
  flashed and opened nothing, the mouse path just omitted the session row. Unified on the mouse path's
  behaviour (browser and diff verbs stay).
- The PR picker and both context menus now call `Config::load()`, so their tests had to be wrapped in
  `with_default_config` (the CONFIG.JSON gotcha, avoided rather than hit); the new `agent_picker` tests
  pin a temp file the same way.
- The GUARD HOOK blocks `cargo check … | grep` as well as `cargo test … | tail`, and a blocked compound
  command runs *none* of its parts — the `cat > script.py` heredoc before the pipe never landed either.
  Write the file in one command and run cargo into a log in the next.
- Codex / Cursor / Pi PR SESSIONS are not verified live: the positional first prompt is the same argv
  path AGENT PRESETS use, and `NEBULA_AGENT_CMD` erases argv in E2E PTY, so only the daemon unit tests
  cover the shape. `retire:` one live `codex` PR SESSION that reads the rule and waits.
