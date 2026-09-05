# Harnesses Can Be Switched Off In Settings And Leave The NEW SESSION PICKER — 2026-08-28

**Asked:** "in settings, allow a user to disable harnesses so they don't even show up in the harness picker
modal"
→ refined: a per-AGENT KIND toggle (Claude / Codex / Cursor, on by default) on the SETTINGS OVERLAY's
Agents tab; a disabled kind is absent from the NEW SESSION PICKER (not greyed) with its MODEL / EFFORT
submenu; persisted in CONFIG.JSON; existing SESSIONS keep attaching / RESUME / restarting; disabling Claude
also blocks the PR SESSION launch with a FLASH. (no questions asked)

**Did:** *(2026-09-04: `claude_enabled()` and `CLAUDE_DISABLED_FLASH` are gone — the PR SESSION picker reads `enabled_kinds()` through `agent_picker.rs` like the NEW SESSION PICKER, and the test is now `disabled_claude_leaves_the_pr_session_to_the_other_harnesses`.)* `crates/nebula-tui/src/config.rs`: `claude_enabled` / `codex_enabled` / `cursor_enabled: bool`
(default true), `SettingKind::{ClaudeEnabled, CodexEnabled, CursorEnabled}` as `on_off` toggles, the
AGENTS TAB regrouped per kind (enabled / model / effort), `Config::kind_enabled(kind)` and
`enabled_kinds()` (filters `AgentKind::ALL`), three `write_into` inserts. `event_loop.rs`:
`open_new_agent_picker` builds its rows from `enabled_kinds()` via `kind_label` and flashes instead of
opening when the list is empty; `apply_setting_at` refuses to turn off the last harness with
`settings_mut(app).warn("keep at least one harness enabled")` before `save_config`;
`open_pr_agent_picker` flashes `CLAUDE_DISABLED_FLASH` and the two "New Claude session" CONTEXT MENU
pushes are skipped when Claude is off (`claude_enabled()` helper); `default_claude_prewarm` returns
`Option<ClientRequest>` (None when Claude is off) so no WARM SPARE boots for a disabled harness — its four
callers `out.extend(..)`. Six new tests (`harness_toggles_default_on_and_persist`,
`picker_omits_a_disabled_harness`, `picker_with_every_harness_disabled_flashes_instead_of_opening`,
`disabled_claude_blocks_pr_session_and_hides_its_menu_row`, `disabled_claude_skips_the_standing_prewarm`,
`agents_tab_toggles_a_harness_and_refuses_the_last_one`) plus a `with_config_json` test helper. README
(picker walkthrough, settings list, `s` row) and TERMS (NEW SESSION PICKER, SETTINGS OVERLAY, SETTING)
updated. nebula-tui 496 green, clippy and fmt clean, e2e `tui_projects_worktrees_agents_navigation` green.
TUI-only: no PROTOCOL VERSION bump, no daemon change. Not committed (SHARED CHECKOUT).

**Gotchas:**
- **A test that never pinned the config passes only until the code under it starts calling
  `Config::load()`.** `picker_second_row_creates_codex_agent` / `picker_third_row_creates_cursor_agent`
  ran unwrapped for weeks because the picker read no config; the moment it filtered by `enabled_kinds()`
  they would read the developer's real `config.json`. Any code path that newly loads config must audit
  its tests for a missing `with_default_config` / `with_config_path` — the failure would be on one
  machine only.
- **An empty `ContextMenu` panics twice over**: Enter indexes `items[menu.hover]` and `j` computes
  `items.len() - 1` (usize underflow). Never build a menu with zero rows; the picker flashes instead.
- `Config::cycle` returns `()` and config.rs has no UI, so a refusal has to live in the event loop:
  `apply_setting_at` checks the cycled config and uses the existing `SettingsView::warn` notice (the same
  channel `ResetHotkey` uses). `cycle` stays a pure toggle so the config unit test mirrors `focus_tint`.
- `default_claude_prewarm` is the one choke point for the standing Claude WARM SPARE (the per-pick
  `PrewarmAgent` in `run_menu_action` already uses the picked kind). Its keep-warm re-arm
  (`app.next_keepwarm`) is deliberately left alone so re-enabling Claude warms on the next tick; an
  already-warm spare simply ages out — disabling kills nothing.
- Another session removed the NEW SESSION PICKER's Terminal row the same day (the refined prompt's "Terminal row
  stays" clause was moot by the time work started), and the one red test in the run
  (`project_and_worktree_rows_show_time_since_last_interaction`) is that or another session's
  uncommitted test — it is not in `HEAD`. Check `git grep <test> HEAD` before blaming your change.
