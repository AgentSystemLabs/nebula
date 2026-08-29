# AGENT PRESETS: `e` Lists Saved Launch Definitions And Starts A SESSION With Prefix + Task + Postfix — 2026-08-28

**Asked:** "when a user is focused on the session, I want them to be able to press a hotkey which shows an
"agent" modal which then they can setup a pre-configured agent definition that includes harness, model,
effort, and optional prefix and postfix prompts one can sandwhich the request. show all agent in modal in
list similar to other modals. crud functionality. agents shouls display in the sessions list as normal
sessions. basically when creating the agent, show a prompt input modal and use that as the starting
prompt of the harness configured for the agent. once created it should work the same as any other
session created."
→ refined: When FOCUS is on the SESSIONS PANEL, let me press a hotkey (assuming `e`, free since NOTES was
retired; a rebindable KEYMAP action) that opens a list modal — like the HOSTS PICKER — of saved agent
definitions (no TERM yet): a name, AGENT KIND ("harness"), MODEL / EFFORT, and optional prefix and
postfix text. Create, edit and delete them from the list (`a` / `e` / `d`), persisted in the DATA DIR
beside CONFIG.JSON. Enter on one opens a multi-line prompt like the CLOUD TASK EDITOR; what I type,
sandwiched as prefix + request + postfix, becomes the CLI's starting prompt for a new AGENT in the
selected WORKTREE. From then on that AGENT is an ordinary SESSIONS PANEL row — HARNESS BADGE, AUTO-TITLE,
RESUME, hooks and AGENT STATUS unchanged. (no questions asked)

**Did:** Daemon: `ClientRequest::CreateAgent` gained `#[serde(default)] starting_prompt: Option<String>`
(request-only like `cloud_prompt`, never persisted), PROTOCOL VERSION 31 → **32**;
`registry.rs::CreateAgentSpec.starting_prompt`, `validate_starting_prompt` (NUL / blank /
`MAX_CLOUD_PROMPT_BYTES`), a bail when combined with a cloud task, the PREWARM POOL adoption guard now
`cloud_prompt.is_none() && pr_url.is_none() && starting_prompt.is_none()`, and the cold spawn passes it
as `agent_spawn_command_with`'s `initial_prompt`, which the Codex and Cursor arms now push as the trailing
positional (Claude already did). The RELOCATION PROMPT is filtered to Claude at `complete_pending_move`
so codex/cursor relocation behavior is unchanged. `server.rs` logs `launch_mode = "preset"`. TUI: new
`crates/nebula-tui/src/agent_presets.rs` (the `AgentPreset` struct, `agent_presets.json` in the DATA
DIR beside CONFIG.JSON, tmp+rename save, `compose`, `spec_label`, `with_presets_path` test hook) and new
`crates/nebula-tui/src/preset_overlays.rs` (`AgentPresetsView`, `PresetField`, `AgentPresetEditor`,
open/reopen/save/confirm/task fns, `handle_list_key` / `handle_editor_key` / `handle_list_mouse`,
`draw_list` / `draw_editor`) — extracted out of `app.rs` / `event_loop.rs` / `ui.rs` under CLAUDE.md's
new "Keep modules small" rule, which landed mid-task. `Action::AgentPresets` (`agent_presets`, SESSIONS
group, default `e`), `Overlay::AgentPresets` / `AgentPresetEditor`, `PromptKind::AgentPresetTask`
(multi-line, reuses the CLOUD TASK EDITOR draw arm and validation), `PendingAction::DeleteAgentPreset`
(both answers reopen the list), `AgentLaunchDraft { starting_prompt, reopen_on_error }` with the
existing `PendingIntent::AttachCreatedWithCloudRetry` bringing the task back on a daemon Error, no warm-
slot refill for a preset launch, a `kind_enabled` check at Enter. `ui.rs` helpers `centered_rect`,
`modal_block`, `empty_list_row`, `row_rect`, `render_row`, `input_spans`, `multiline_input_lines` and
`event_loop::open_prompt` became `pub(crate)`; `config::cycle_choice` / `non_default` too. HELP OVERLAY
row, FOOTER hints, README keymap row + paragraph, ARCHITECTURE "Presets path". Tests: 6 in
`agent_presets.rs`, 9 in `event_loop.rs` (list, editor create/edit/validate/delete, launch with the
composed prompt + Error reopen, empty task, cursor n/a, click, empty list), registry
`spawn_command_initial_prompt_is_the_trailing_positional_argument` (rewritten) and
`starting_prompt_is_validated_and_never_adopts_a_warm_cli`; 20 `e2e_pty.rs` `CreateAgent` literals gained
`starting_prompt: None`. Gate: nebula-tui 514, nebula-daemon 157, nebula-core 12 green, clippy and fmt
clean. Not done: a live run against the real CLIs (the prompt reaches argv only outside
`NEBULA_AGENT_CMD`); the installed binary and daemon are still the old PROTOCOL VERSION.

**Gotchas:**
- **`NEBULA_AGENT_CMD` erases the whole argv** (`agent_spawn_command_with` returns the override verbatim
  before building anything), so no E2E PTY test can see a starting prompt, a `--model`, or a system
  prompt — argv coverage lives only in `registry.rs`'s unit tests. Don't write an e2e for it.
- All three CLIs take a positional prompt per `--help` on this box: `claude [options] [command] [prompt]`,
  `codex [OPTIONS] [PROMPT]` **and** `codex resume [OPTIONS] [SESSION_ID] [PROMPT]`, `cursor-agent
  [options] [command] [prompt...]`. Only Claude's resume form is verified live, hence the Claude-only
  filter on the RELOCATION PROMPT.
- **`TextInput::handle_key` returns `Ignored` for Enter / Esc / Tab / BackTab / Up / Down / Ctrl+j**, so a
  multi-field form can use them for navigation; Shift+Enter and Ctrl+J must be matched *before* the
  `handle_key` fallthrough or the `j` types. `insert_str` flattens `\n` — paste into a multi-line field
  must use `insert_multiline_str`.
- **`submit_prompt` does `match prompt.kind`, which moves it**: an arm that wants to re-open the dialog
  on a failed check hits "use of partially moved value: `prompt`". Do kind-specific validation in the
  multiline block *above* the match (it still owns `prompt`), as the composed-length check does.
- In a 100-column TESTBACKEND the Sessions FOOTER hint line is truncated at the right edge (`↑/↓: select
  Enter: la`), and while a modal is up the FOOTER shows the modal's hint, not the panel's. Assert the
  modal's `title_bottom` text while it is open, and draw a 180-column backend for a panel-hint check.
- `open_agent_preset_task` calls `Config::load()` (for `kind_enabled`), so every TUI test on the launch
  path must run inside `with_default_config`, or the real `config.json` decides.
- BSD `sed` has no `\b`; a word-boundary rename across the tree needs `perl -pi -e 's/(?<![:\w])name\(/…/'`.
- **The SHARED CHECKOUT moved three times during this task**: another session bumped PROTOCOL VERSION
  30 → 31 (so this one is 32), CLAUDE.md gained "Keep modules small" (which turned ~700 added lines into
  `preset_overlays.rs`), and the NEBULA-MEMORY SKILL switched to `.claude/memory/entries/` + a
  `gotchas.md` + `check.py` while `MEMORY.md` still held every old entry body. Re-read `git status`
  before each phase, not once at the start.
