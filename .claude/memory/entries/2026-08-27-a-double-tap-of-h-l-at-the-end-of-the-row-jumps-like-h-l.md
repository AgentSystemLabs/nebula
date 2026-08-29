# A DOUBLE TAP Of `h`/`l` At The End Of The Row Jumps Like ^⇧H/^⇧L — 2026-08-27

**Asked:** "when navigating with the h and l keys, extend it so when i double tap the h or l when I'm at
the locked layer, it should act as if I did a control shift h or l command"
→ picked (prompt-daddy): *Second press at the stop* — "When h or l is pressed and focus is already at the
edge where it stops … that press should act like Ctrl+Shift+L / Ctrl+Shift+H instead of doing nothing"
→ corrected after the first cut: "it incorrectly allows me to toggle past sessions list into the terminal
panel, and the reverse end I incorrecly can press h once to get to the workspaces. how it really should
work is a user must double tap h to 'jump' over that blocked boundary. you should just be able to add a
50ms elapse time check to add this, but find other options"

**Did:** FOCUS LEFT / FOCUS RIGHT (`Action::FocusLeft` / `Action::FocusRight`) in `crates/nebula-tui/src/event_loop.rs` (~1300)
share `walk_focus_back(app)` / `walk_focus_forward(app)` (next to `enter_terminal_pane`, ~4745) with
`FocusPrev` / `FocusNext`, except at the WALK EDGE: `l` at Sessions (or on an unlocked live pane)
and `h` at Projects-with-the-bar-shown go through `double_tapped(app, action, armed, &chord, does)`. First
press: stays put, arms `app.edge_tap = Some((action, Instant))` (new field in `app.rs` next to `flash`),
and flashes "`l` again: enter pane" / "`h` again: workspaces" in the FOOTER. Second press of the same
action within `DOUBLE_TAP` (400 ms, matching `DOUBLE_CLICK`) jumps. `handle_key` `take()`s the arm before
the keymap lookup, so any other key — bound or not — breaks the pair; a late second press re-arms. `h` at
Projects with the bar hidden neither arms nor flashes. Docs: `focus_left`/`focus_right` hints, HELP OVERLAY row
"focus left / right (2×: jump)", the locked-input comment, README rows 257–258. Tests:
`h_and_l_walk_panel_focus_like_the_arrows`, `a_slow_or_interrupted_second_tap_at_the_edge_stays_put`,
`double_tapped_right_at_sessions_enters_the_pane_and_locks_it` (was `plain_right_stops_at_sessions`),
`focus_walk_includes_the_workspaces_bar_only_when_shown`. 467 unit + 5 e2e_tui green.

**Gotchas:**
- **"Double tap" means double tap.** The PROMPT DADDY pick read it as "a press at the edge that would
  otherwise be a no-op" — no timer — and shipped a single `l` at Sessions crossing into the pane. The user
  rejected that within a minute: the single press at the boundary *must* stay a no-op; only the pair jumps.
  When the user says a gesture, implement the gesture, not the state it implies.
- **50 ms is not a human double tap.** Two deliberate key-downs are ~120–250 ms apart; 50 ms would only
  ever be hit by held-key auto-repeat. Reused the app's `DOUBLE_CLICK` value (400 ms) so the keyboard and
  mouse "again, deliberately" gestures feel the same; it's one const to tune.
- **Held-key auto-repeat still spills.** macOS default repeat is ~90 ms, inside any sane window, so holding
  `l` from Worktrees will arm and then fire into the pane. Nebula pushes only
  `KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES` (`event_loop.rs:166`); adding `REPORT_EVENT_TYPES`
  would deliver repeats as `KeyEventKind::Repeat` and let `double_tapped` ignore them — not done, since it
  changes what every terminal sends. The open option if the spill bites.
- "Locked layer" in the original prompt meant the WALK EDGE where h/l stop, not the LOCKED PANE — in a locked
  pane `h`/`l` are forwarded to the agent and can't be a gesture without eating keystrokes.
- `h`/`l` and `←`/`→` are one action each (`focus_left`/`focus_right`), so **the arrows got the DOUBLE
  TAP too**; the flash uses `chord.display()` so it names whichever key was pressed.
- `app.flash` is cleared at the top of every key event (`handle_terminal_event`, ~1057) *before* dispatch,
  so a FLASH set during the press survives exactly until the next key — a free one-shot hint.
- HELP OVERLAY descriptions are clipped at `width - 16` of a 46-column half (≈30 chars); longer rows lose
  their tail silently.
- The locked-input comment above the hatch block still said "⇧Tab / ^⇧H wrapping round from the first
  panel" — stale since the PANEL WALK change earlier today; prose near `HARDWIRED_UNLOCK` lies easily.
- **The live TUI was `target/debug/nebula`** (a 1h+ old process), and `~/.cargo/bin/nebula` is now a
  regular 8 MB file that differs from `target/release/nebula` — not the symlink the Focus-Key Odyssey entry
  describes. Rebuild debug *and* release, and the user has to restart the TUI to see a keymap change.
