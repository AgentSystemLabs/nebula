# The FILE FINDER Closes Behind The EDITOR It Opens — 2026-08-29

**Asked:** "I need a setting toggle which controls the following: in the FILE FINDER, when a user
opens a file, close the FILE FINDER modal and only show the edit file modal so that when the user
closes that edit modal, they don't also have to close the underlying file finder modal."
→ refined: Add a boolean SETTING `close_finder_on_open` (SETTINGS PANEL General tab, default on) so
that when I press Enter on a FILE FINDER row or a `Shift+F` grep hit, the FILE FINDER OVERLAY closes
as the EDITOR modal opens — quitting the EDITOR then lands me back on the SESSIONS view, not on the
finder I have to Esc a second time. With it off, keep today's behavior (the finder stays open
underneath). Leave the TREE BROWSER's embedded preview-pane EDITOR and OPTION CLICK alone.

**Did:** New SETTING `close_finder_on_open` (default `true`, General tab row "Finder closes on
open") in `crates/nebula-tui/src/config.rs` — the usual seven edits: `SettingKind::CloseFinderOnOpen`,
its `SettingSpec`, the `Config` field, `Default`, the `write_into` `obj.insert`, the `value_label`
arm, the `cycle` arm. In `crates/nebula-tui/src/event_loop.rs` both
`open_selected_file_in_editor` (FILE FINDER `Enter` / click) and `open_selected_hit_in_editor`
(the `Shift+F` grep view) now call a new `close_finder_behind_editor`, which drops `app.overlay`
when the SETTING is on — but only when `spawn_editor_modal` returned `true`. The TREE BROWSER path
(`open_selected_tree_file_in_editor`) and OPTION CLICK (`open_file_link`) are untouched. Two existing
tests that assert the finder / grep view survives under the editor are pinned to
`{"close_finder_on_open": false}` via `with_config_json`; three new tests cover the default-on
close for both overlays and the failed-spawn case (`a_failed_editor_spawn_keeps_the_finder_open`),
plus `config::tests::defaults_close_the_finder_on_open`. Gate: fmt clean, clippy clean, nebula-tui
542 passed, workspace green.

**Gotchas:**
- Close the finder only after `spawn_editor_modal` returns `true`. A bad EDITOR command flashes and
  spawns nothing, and a unit test without `app.vim_tx` never spawns at all — dismissing the finder
  first would leave the user on the bare panels with no editor. Pinned by
  `a_failed_editor_spawn_keeps_the_finder_open`.
- Adding a SETTING is the mirror of retiring one (see the 2026-08-29 FOCUS TINT entry): the same
  seven `config.rs` edits, and the compiler catches every arm except the `write_into` `obj.insert` —
  miss that one and the toggle displays but never persists. Now a standing gotcha under SETTING.
- The open path now calls `Config::load()`, so the two pre-existing finder / grep `Enter` tests would
  have read the developer's real `config.json` — the standing CONFIG.JSON gotcha, applied before it
  bit: they are wrapped in `with_config_json` (they also *need* the off value, since their second
  half asserts the overlay is still there under the editor).
- The TREE BROWSER is deliberately excluded: its EDITOR is `embedded` in the preview pane and
  `close_vim` hands the pane back through `Overlay::Tree` — closing that overlay would orphan the
  embedded vim (`draw_vim` only falls through to the modal frame as a safety net).
