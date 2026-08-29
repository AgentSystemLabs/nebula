# Settings Opens On The Tab Strip — 2026-08-26

**Asked:** "when I load up the settings, it should always focus on the tab (or last selected tab +
option combo)"

**Did:** `App::settings_on_tabs` (`crates/nebula-tui/src/app.rs`, default `true`) joins the existing
`settings_tab` / `settings_selected` memory, and `SettingsView::new` grew a third `on_tabs` arg. A first
open now parks on the tab strip (←/→ walk tabs immediately, no ↑ first); every later open restores the
tab, the row, *and* whether the cursor was on the strip or in the list. `remember_settings_focus` is
called from `SettingsCmd::FocusTabs` / `EnterList` and from both settings mouse-click paths in
`event_loop.rs`.

**Gotchas:**
- Twelve existing tests silently assumed `s` lands **in the list** — with the strip focused, `j` means
  "drop into the list" and `Enter` means the same, so hotkey-capture and value-cycling tests all failed
  in ways that looked like keymap bugs (`left: "?" right: "F6"`). The fix is one place: the shared
  `open_settings_on` test helper now presses `↓` after picking the tab. Route new settings tests through
  it rather than pressing `s` and navigating.
- All this state is per-process only — `UiState` (the blob persisted in the daemon DB) was deliberately
  left alone, so a fresh `nebula` always starts on the strip.
- Since 2026-08-27 the memory also expires a minute after closing — see [Settings Position Memory
  Expires A Minute After Closing].
