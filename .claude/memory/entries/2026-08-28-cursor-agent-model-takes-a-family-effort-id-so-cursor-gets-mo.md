# `cursor-agent --model` Takes A `<family>-<effort>` Id, So Cursor Gets MODEL / EFFORT In The PRESET EDITOR, The NEW SESSION PICKER And The AGENTS TAB — 2026-08-28

**Asked:** "I'm unable to pick a cursor model from inside the AGENT PRESET modal, be sure to lookup the
current cursor models and allow me to select the model and effort. lookup cursor-agent to determine how
to pre launch cursor with settings"
→ refined: In the PRESET EDITOR the Model and Effort rows read `n/a` for Cursor. `cursor-agent --help`
and `--list-models` show it now takes `--model <id>` from a catalogue whose ids bake the effort into a
suffix (`claude-opus-5-thinking-high`; the `[effort=…]` bracket form is rejected). Give Cursor a MODEL
list of the current families and an EFFORT list of the suffixes each family has, have the DAEMON launch
`cursor-agent --model <family>-<effort>`, light the same choices up in the NEW SESSION PICKER submenus
and (assuming) Cursor rows on the AGENTS TAB, keep Claude and Codex as they are. (no questions asked)

**Did:** Supersedes the 2026-08-14 "cursor-agent has no knobs" gotcha — cursor-agent 2026.08.25 has
`--model`, `--list-models` and `models`. `crates/nebula-tui/src/config.rs`: `CURSOR_MODELS` (16
families curated from `--list-models`, `-fast` variants left out), `CURSOR_EFFORTS` (per-family suffix
lists: Opus 5 has no `xhigh`, GPT-5.5 spells it `extra-high`, `gpt-5.3-codex` bare = medium, `auto` /
`composer-2.5` none), `model_choices(Cursor)` now real, **`effort_choices(kind, model)`** (model-aware;
empty for Cursor without a family), `fit_effort(kind, model, effort)`, SETTINGs `cursor_model` /
`cursor_effort` with `SettingKind::CursorModel` / `CursorEffort` rows on the AGENTS TAB (`value_label`
shows `n/a` while the family has no efforts; cycling the family drops an effort it lacks),
`default_model` / `default_effort(Cursor)` live. `preset_overlays.rs`: `PresetField::available(kind,
model)` / `step(kind, model, delta)`, `AgentPresetEditor::fit_effort` after a Kind or Model cycle,
free `fits`. `app.rs::MenuAction::submenu`: a model row whose effort list is empty is a leaf.
`event_loop.rs::build_submenu` passes the model; both launch sites (`AgentPresetTask`,
`NewAgentOfKind`) run `fit_effort`. `nebula-daemon/src/registry.rs::agent_spawn_command_with` Cursor arm:
`--model <m>-<e>` / `--model <m>`, an effort alone dropped; no PROTOCOL VERSION change (`Agent.model` /
`effort` already existed). README picker paragraph. Tests: config 3 (`cursor_settings_rows_follow_the_family`,
`cursor_catalogue_is_consistent`, extended `model_effort_defaults_resolve_and_cycle` / save),
event_loop `cursor_preset_effort_follows_the_model_family` (replaces `cursor_preset_hides_model_and_effort`),
`picker_cursor_drills_into_family_then_effort_and_auto_is_a_leaf`, registry cursor argv cases. Gate:
nebula-tui 523, nebula-daemon 162, clippy + fmt clean; live `cursor-agent --force -p --model
gpt-5.3-codex-low` answered `"result":"ok"`.
**Follow-up, same day (user: "yes add a runtime cursor-agent --list-models refresh and the -fast axis"):**
new `crates/nebula-tui/src/cursor_catalogue.rs` (KEEP MODULES SMALL) — `SEED_IDS` (112 observed ids),
`split_id` (`<family>[-<effort>][-fast]`, longest effort word first), `Catalogue::from_ids` (dedup,
first-seen family order, pick-order efforts with `-fast` twins), `fallback_effort` (`high` > `medium` >
first non-fast), `parse_list_models`, a leaked `'static` view behind `OnceLock<RwLock>` (`models()`,
`efforts()`, `install()`), `cursor_models.json` cache in the DATA DIR with `CACHE_TTL` 24 h, and
`bootstrap(cursor_enabled)` called from `event_loop::main_loop` right after `Config::load()` — installs
the cache at once, refreshes on a `std::thread` when stale, skipped under `NEBULA_AGENT_CMD`. `-fast`
rides in the effort string (`high-fast`, bare `fast`), so `Agent.model` / `effort`, the PREWARM key and
the DAEMON join are untouched. `config.rs` now delegates (`CURSOR_MODELS` / `CURSOR_EFFORTS` are gone),
`fits` moved there, and `fit_effort` gained the bare-id rule below. Tests: 6 in `cursor_catalogue.rs`,
config `fit_effort_resolves_cursor_pairs`, the settings / editor / picker tests re-scripted. Gate:
nebula-tui 531, nebula-daemon 162, clippy + fmt clean, live `--model claude-opus-5-high-fast` → `ok`,
MAKE INSTALL run. Not done: `make cycle` on the user's DEV INSTANCE.

**Gotchas:**
- **`cursor-agent --help` advertises `--model 'claude-opus-4-8[context=1m,effort=high,fast=false]'`, but
  the validator rejects every bracket form** (`Cannot use this model: claude-opus-5[effort=low]`), so
  the flat catalogue id is the only shape that launches. The same error lists the *whole* catalogue
  (~200 ids) — `--list-models` shows only the featured ~60; use `--model bogus` to see them all.
- The catalogue's effort suffixes are irregular per family, so a kind-only `effort_choices(kind)` cannot
  serve Cursor: `claude-opus-5` stops at `high` while `claude-opus-5-thinking` goes to `max`, `gpt-5.5`
  says `extra-high`, `gpt-5.3-codex`'s bare id *is* medium. Every surface that cycles a Cursor effort
  must re-fit it when the family changes (`fit_effort`), or the DAEMON composes an id the CLI refuses at
  spawn and the AGENT dies on its first screen.
- `Config`'s settings accessor is `value_label(kind)`, not `value(kind)` — a test written from the
  `cycle(tab, row, delta)` side guesses wrong and the compile error suggests an unrelated itertools trait.
- The docs at cursor.com/docs/cli/reference/parameters list only `--model` and `--list-models`; the
  local `--help` is the richer (and, on the bracket form, wrong) source. Verify against the binary.
- **Most families have no bare id**: `claude-fable-5`, `claude-opus-5`, `gpt-5.5`, `gpt-5.6-*`,
  `cursor-grok-4.6`, `gemini-3.7-flash` exist only with a suffix (`gpt-5.3-codex`, `gpt-5.2`,
  `composer-2.5`, `auto` are the bare ones). The first round of this task launched `--model
  claude-fable-5` for "default" effort — refused at spawn. "default" for such a family must resolve to a
  real suffix (`fit_effort` → `fallback_effort`), and the effort list only offers "default" when the bare
  id exists.
- `split_id` must try the longest effort word first: `gpt-5.5-extra-high` ends in `-high` too, and the
  naive order parsed it as family `gpt-5.5-extra`.
- The seed catalogue is a `&'static` view leaked once per install; tests must never call `install`
  (process-global, tests run in parallel) — test `Catalogue::from_ids` and friends as pure functions.
