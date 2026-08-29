# Type-Ahead Filters The MODEL / EFFORT Submenus And The PRESET EDITOR's Choice Rows — 2026-08-28

**Asked:** "when picking a cursor model do a type agead type of approach to filter quick to a model, do
on presets modal as well as new session modal"
→ refined: When I pick a Cursor model, let me type to filter instead of cycling through the whole list —
in the PRESET EDITOR (the "presets modal") and in the NEW SESSION PICKER (the "new session modal").
Assuming: in any MODEL / EFFORT submenu of the picker, typed characters build a filter shown in the modal
title (`Cursor model ⌕ opus`) that narrows the rows with the repo's `fuzzy_match` (subsequence, best
match first); ↑/↓/←/→/Enter/Esc/Backspace keep navigating there, so `h`/`j`/`k`/`l` type; a character
that would leave no rows is refused; Esc clears a non-empty filter before it backs out. In the PRESET
EDITOR, typing on the Harness / Model / Effort row does the same: the row jumps to the best match and
←/→ cycle only the matches, the filter is shown beside the value, Backspace edits it, Tab / ↑ / ↓ leave
the row and drop it. Claude and Codex lists get the same behavior; the AGENTS TAB is unchanged. (no
questions asked)

**Did:** `app.rs`: `ContextMenu.filter: Option<MenuFilter { query, all }>` — `Some` only on the MODEL /
EFFORT submenus (`event_loop::build_submenu`); `set_filter` (fuzzy `rank`, best first; empty query =
full list hovering the ✓ row), `type_filter` (refuses a letter that would empty the rows),
`pop_filter`, `filter_query`, `has_filter_text`. `event_loop.rs` `Overlay::Menu` arm: `Char(c)` (not
Space, not Ctrl) types when `filter.is_some()`, Backspace pops, `Esc` clears text before the existing
back-out; a refused letter sets `app.flash`. `ui.rs`: the title reads `<title> ⌕ <query>` (bare ⌕
before typing) and the bottom border says "type to filter  ↑↓: move  Backspace  Esc: back".
`preset_overlays.rs`: `AgentPresetEditor.filter: String`, `row_choices` / `row_value` /
`set_row_value` (Kind via `AgentKind::parse` + `set_kind`, Model re-fits the effort), `filtered_choices`,
`type_filter`, `pop_filter`; `cycle` now walks the filtered matches; `h`/`l` no longer cycle choice rows
(they type); Tab/↑/↓ clear the filter, Esc clears it before backing out; the row draws `⌕ <filter> (n)`
after `▸`; hint reworded to "←/→ or type: choose". README picker paragraph. Tests:
`picker_model_submenu_types_to_filter`, `preset_editor_types_to_filter_choice_rows`;
`skip_session_naming_keeps_the_submenu_model_pick` moves with ↓ instead of `j`. Gate: nebula-tui 533
green, clippy + fmt clean, MAKE INSTALL run.

**Gotchas:**
- **Letters type in a MODEL / EFFORT submenu now**, so a test that walks one with `KeyCode::Char('j')`
  filters on "j" instead of moving (and, since no row matches "j", is refused and flashes). Drive
  submenus with `KeyCode::Down` / `Up`; the root picker (three rows) still takes `j`/`k`.
- `ContextMenu` is built by five struct literals in `event_loop.rs` plus tests' `menu.clone()` parents —
  a new field means touching every literal (`filter: None`); there is no `Default`.
- `fuzzy::rank` on an empty query returns every candidate in list order (no reshuffle), which is what
  makes "clear the filter" restore the original menu; a query of only whitespace counts as empty too.
