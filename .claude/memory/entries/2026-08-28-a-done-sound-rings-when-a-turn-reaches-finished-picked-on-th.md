# A DONE SOUND Rings When A Turn Reaches FINISHED, Picked On The Sessions Tab — 2026-08-28

**Asked:** "play a ding sound when anything goes into the done status. make this configurable in the
settings to turn off or style"
→ refined: When a SESSION's AGENT STATUS goes into FINISHED (your "done") — the RUNNING / NEEDS
FEEDBACK → FINISHED edge the DONE BADGE already keys off, for every SESSION, on screen or not — play a
ding from the TUI. Add a `done_sound` SETTING on the SETTINGS OVERLAY: `off`, `bell` (the terminal BEL,
the one route that reaches my terminal over NEBULA SSH), and a handful of named system sounds (assuming
macOS `afplay` sounds like `Glass`, `Ping`, `Pop`, `Hero`, falling back to `bell` where `afplay` is absent
or over ssh). Default it on; no ding on the startup Snapshot, only on live status flips. Keep the STATUS
DOT, UNSEEN and DONE BADGE behavior exactly as they are.

**Did:** TUI-only. `crates/nebula-tui/src/config.rs`: `DONE_SOUNDS` (`off`, `bell`, then the 14
`/System/Library/Sounds/*.aiff` stems), `SettingKind::DoneSound` as the "Done sound" row on the
SETTINGS OVERLAY's Sessions tab, `Config::done_sound: String` (default `Glass`), and
`Config::done_sound() -> Option<DoneSound>` via the pure `resolve_done_sound(name, remote, macos)`:
`off`/blank → `None`; `bell`, `SSH_CONNECTION` set, non-macOS, a non-alphanumeric name, or a stem with no
file → `DoneSound::Bell`; else `DoneSound::File(path)`. `crates/nebula-tui/src/event_loop.rs`: the
`ServerEvent::StatusChanged` arm sets `app.pending_ding` when the row's *previous* status was RUNNING or
NEEDS FEEDBACK and the new one is FINISHED (a bool, so several finishes in one frame ring once; the
Snapshot and a Finished→Finished re-stamp never set it); the main loop drains it right after the
`pending_clipboard` OSC 52 write into `play_done_sound(backend)`, which spawns `afplay <file>` detached
(stdio null, a thread reaps the child) or writes `\x07` through the ratatui backend. `App::pending_ding`
in `app.rs`; README settings paragraph names the key. Tests:
`config::done_sound_defaults_to_bell_cycles_persists_and_resolves` (name is stale by one edit — it now
asserts the `Glass` default), `event_loop::a_finish_rings_the_done_sound_once`. nebula-tui 499 passed.

**Gotchas:**
- **A bare terminal BEL is silent in Ghostty out of the box**: `ghostty +show-config --default` prints
  `bell-features = no-system,no-audio,attention,title,no-border` — BEL only bounces the dock icon and
  marks the title. That is why the default is the `Glass` system sound, not `bell`; a user who wants
  `bell` in Ghostty needs `bell-features = audio` (or `system`) in their Ghostty config.
- The daemon never plays anything — `afplay` on a `nebula ssh` host would ring the remote box's
  speakers, so `resolve_done_sound` forces `Bell` whenever `nebula_core::host::is_remote_session()` is
  true; the BEL travels the same path as the CLIPBOARD ROUTE's OSC 52 and rings the local terminal.
- Keying the ding off the *previous* status in `app.tree`, not the event's `unseen` flag: `unseen`
  is also true when the row was already FINISHED-and-unread and merely got re-stamped, and it is false
  for a finish on the on-screen session (which should still ding — the user may be in another window).
