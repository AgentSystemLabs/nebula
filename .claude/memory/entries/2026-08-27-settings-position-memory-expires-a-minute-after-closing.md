# Settings Position Memory Expires A Minute After Closing — 2026-08-27

**Asked:** "if a user tries to open the settings modal for the first time, it should start focused on
the setting bar and if the launch it later it should retain their position. extend this logic to support
timing out that retain so if it's been 1 minute since opening it should revert back"
→ picked (prompt-daddy): *Timer runs from close* — "when I close the modal, note the time; if I reopen
it more than 1 minute later, discard the remembered position and open fresh exactly like a first open
(first tab, row 0, focused on the tab strip). Reopening within the minute restores the position as it
does now."

**Did:** The first-open-on-strip + retain half already existed ([Settings Opens On The Tab Strip]);
this adds the expiry on top. `App::settings_closed_at: Option<Instant>` and
`SETTINGS_MEMORY_TTL` (60s) in `crates/nebula-tui/src/app.rs`, with `note_settings_closed` /
`settings_memory_expired` / `forget_settings_focus` (tab 0, every tab's row 0, `on_tabs = true`, stamp
cleared). In `event_loop.rs`, `Action::Settings` now goes through new `open_settings(app)` = expiry
check + the existing `reopen_settings`; both ways out — `SettingsCmd::Close` (Esc/`q`/`s`) and the
click-outside mouse path — go through new `close_settings(app)`, which stamps the time. README `s` row
mentions the minute. Tests: `settings_memory_expires_a_minute_after_closing`,
`settings_reset_round_trip_ignores_the_memory_clock`, `clicking_outside_settings_stamps_the_close`.

**Gotchas:**
- **`reopen_settings` must stay clock-blind.** The `Shift+R` reset flow swaps Settings → Confirm →
  Settings mid-visit through it; routing that through the expiry check would reset the cursor under the
  user's hands whenever an old stamp had aged out. `open_settings` is the only from-the-panels entry.
- Backdating in tests is `Instant::now().checked_sub(SETTINGS_MEMORY_TTL)` — `Option<Instant>` is the
  field type, so it drops straight in.
- Another session's uncommitted work was in the tree the whole time (Help/Confirm click-outside,
  `Overlay::Help(HelpView)`, delete confirms): `confirm_click_outside_cancels_without_confirming` was
  failing and clippy flagged `event_loop.rs` `copy_via`'s `return` + `config.rs:1007` — none of it
  mine. See [Shared Working Tree Is Raced By Other Sessions] before chasing a red test you didn't cause.
