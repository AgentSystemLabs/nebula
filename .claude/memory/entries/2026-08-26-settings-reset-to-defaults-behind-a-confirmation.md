# Settings Reset To Defaults Behind A Confirmation — 2026-08-26

**Asked:** "on settings modal add a hotkey to reset to default with confirmation that your settings will
be cleared"

**Did:** `Shift+R` anywhere in the settings overlay (tab strip or list) swaps in a `ConfirmDialog` with
`PendingAction::ResetSettings`; confirming runs `reset_settings` (`crates/nebula-tui/src/event_loop.rs`),
which calls the new `Config::reset_to_defaults()` (`config.rs`), `apply_config`s the result, replaces
`app.keymap` with the default keymap, and reopens the overlay on its remembered tab/row with an info
notice. Esc/`n` on that particular confirm reopens the overlay too (special-cased in the Confirm key
handler) instead of dropping back to the panels. The key is deliberately not in the rebindable keymap —
none of the overlay's own keys are. Hints in `ui.rs::settings_keys_hint` and the README say `R: reset all`.

**Gotchas:**
- **`Config::save()` is a patch, not a write — it can't reset.** It merges the TUI's known keys into
  whatever JSON is already in `config.json`, on purpose, so daemon-owned keys (`prewarm_agents`,
  `prewarm_sessions`, hand-added ones) survive every overlay edit. Saving `Config::default()` through it
  would leave those behind. `reset_to_defaults` writes over `json!({})` via the split-out `write_into`,
  so the file reads as never-edited; `config::tests::reset_rewrites_the_file_from_scratch` pins the
  difference.
- **The settings modal's inner width is 82 columns and `settings_keys_hint` is `truncate`d to it** (with
  a leading space, so ≤81 usable). Adding a key to the hotkeys-tab hint pushed it to 86 and silently
  chopped the end; shorten wording rather than appending.
- **Another session was editing `app.rs` while this ran:** `SettingsView::new` grew a third `on_tabs`
  argument between my first read and my edit. Re-read any signature you call right before writing the
  call, then `grep` your symbols after the build to confirm they're still on disk (see [Shared Working
  Tree Is Raced By Other Sessions]).
