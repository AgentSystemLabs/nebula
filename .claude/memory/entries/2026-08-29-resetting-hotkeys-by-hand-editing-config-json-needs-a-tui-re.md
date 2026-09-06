# Resetting HOTKEYS By Hand-Editing CONFIG.JSON Needs A TUI Restart — 2026-08-29

**Asked:** "Please reset my keybindings to the default"
→ refined: Reset my HOTKEYS to the KEYMAP defaults: drop the `keybindings` SETTING from CONFIG.JSON in
the DATA DIR (the main instance, `~/Library/Application Support/dev.nebula.nebula/config.json`), leaving
every other SETTING untouched. Don't use the SETTINGS OVERLAY's `R` reset — that resets everything, not
just the HOTKEYS.

**Did:** No code changed. The user's four overrides (`focus_left: "]"`, `focus_right: "["`,
`focus_terminal: "\\"`, `unlock_terminal: "ctrl+q, d"`) were not in the app-support DATA DIR at all —
they lived in the DEV INSTANCE's, `~/.nebula-dev/nebula-d87bd951/config.json` (`Makefile:32`,
`DEV_DATA = $(HOME)/.nebula-dev/$(notdir $(CURDIR))-$(DEV_SLOT)`). Backed that file up to
`config.json.bak-20260829-133803` and removed the whole `keybindings` key; every other SETTING kept its
value. The four actions go back to `h`/`←`, `l`/`→`, `Ctrl+→`, and `Ctrl+Q`/`Ctrl+Shift+H`/`Ctrl+]`/
`Ctrl+Esc`/`Ctrl+←` (`crates/nebula-tui/src/keymap.rs:159,168,177,381`).

**Gotchas:**
- **CONFIG.JSON is *not* read fresh for HOTKEYS.** `TERMS.md`'s CONFIG.JSON row says hand edits apply
  live, and that holds for everything the code reads through `Config::load()` — but the KEYMAP is cached
  once into `app.keymap` at startup (`event_loop.rs:279`, field at `app.rs:2128`). A hand edit to
  `keybindings` does nothing until the TUI restarts.
- **Worse, the next rebind silently reverts the hand edit.** `save_keymap` (`event_loop.rs:3773`) writes
  `app.keymap.overrides()` — the *cached* map — so one `⌫`/Enter on the HOTKEYS TAB after a hand edit
  writes all the old overrides back. Restart before touching the tab, or skip the file entirely.
- **The in-app equivalent needs no restart:** `⌫` on a HOTKEYS TAB row is `SettingsCmd::ResetHotkey` →
  `Keymap::reset(index)` (`event_loop.rs:3612`, `keymap.rs:971`), per row. The overlay's `R` is
  `PendingAction::ResetSettings` → `Config::reset_to_defaults`, which rewrites the file from `json!({})`
  and so resets *every* SETTING, not just the HOTKEYS.
- **Look in the DEV INSTANCE DATA DIR first on this machine.** The app-support `config.json` does not
  exist at all here; `~/.nebula-dev/<checkout>-<slot>/` is where a nebula developer's real settings are.
