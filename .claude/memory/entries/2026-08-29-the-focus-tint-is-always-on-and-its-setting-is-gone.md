# The FOCUS TINT Is Always On And Its SETTING Is Gone — 2026-08-29

**Asked:** "force thre panel tint on and remove from settings"
→ refined: Make the FOCUS TINT (your "panel tint") always on: paint the focused PANEL — and the
TERMINAL PANE when it has focus — with the theme's `focus_tint` unconditionally, and remove the
"Focused panel tint" row from the SETTINGS OVERLAY along with the `focus_tint` SETTING
(`SettingKind::FocusTint`, `Config::focus_tint`, `App::focus_tint`). Stop writing `focus_tint` to
CONFIG.JSON (assuming a stale key in an existing file is ignored, not an error). Keep the `focus_tint`
THEME ROLE and its gray-floor test exactly as they are — only the toggle goes. Update or drop the tests
that covered the toggle.

**Did:** `ui.rs::draw` paints `draw_focus_tint` unconditionally (both the collapsed TERMINAL PANE
branch and the panel `match`); dropped `SettingKind::FocusTint`, its `SettingSpec` on the Appearance
tab, `Config::focus_tint` (field, default, `save` insert, display and `cycle` arms) and
`App::focus_tint` plus its `apply_config` mirror in `event_loop.rs`; README's settings sentence no
longer lists the tint. `focus_tint_default_off_toggle_and_persist` became
`config.rs::stale_focus_tint_key_is_ignored` (a `{"focus_tint": true}` file loads, and `save_to` does
not write the key back). The `focus_tint` THEME ROLE, `draw_focus_tint` and
`theme.rs::focus_tint_has_a_visible_gray_floor` are untouched. Gate: nebula-tui 538 passed, fmt and
clippy clean; no E2E suite named the row.

**Gotchas:**
- Retiring a SETTING is seven edits in `config.rs` (enum variant, `SettingSpec`, field, `Default`,
  `save` `obj.insert`, the display arm, the `cycle` arm) plus the `App` mirror field and
  `event_loop.rs::apply_config`; the compiler catches every arm but the `obj.insert` — grep the key
  string too, or the retired key keeps being written.
- `Config` is `#[serde(default)]` with no `deny_unknown_fields`, so a retired key left in CONFIG.JSON
  (the DEV INSTANCE's `~/.nebula-dev/config.json` still carries `focus_tint`) is silently ignored — no
  MIGRATION needed, but pin that with a `stale_<key>_is_ignored` test so a later
  `deny_unknown_fields` cannot break old configs.
