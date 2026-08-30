# `q` Asks Before It Quits The TUI — 2026-08-29

**Asked:** "why does q quit nebula? it feels like a bad ux as I accidently closed out when trying to type"
→ refined: "`q` quits the TUI instantly whenever a PANEL has FOCUS, and I lost my session by typing at
what I thought was the TERMINAL PANE. Explain why it's bound that way, then gate it: the quit ACTION
(`q` and `Ctrl+C`, KEYMAP id `quit`) should open a CONFIRM DIALOG — 'Quit nebula? Sessions keep
running.' — `y`/`Enter` quits, `Esc`/`n` cancels. Use the existing unused `PendingAction::Quit` rather
than a new mechanism. Keep the HARDWIRED UNLOCK (`Ctrl+Q`) and `q`-closes-an-overlay behavior exactly as
they are, and don't gate `q` inside a LOCKED PANE (it already forwards to the PTY)."
(asked: what to do about it → CONFIRM DIALOG on quit, over dropping `q` from the defaults or explaining only)

**Did:** `Action::Quit` no longer sets `should_quit` straight from the KEYMAP dispatch
(`event_loop.rs:1400`); it opens `confirm_quit()` (new, `event_loop.rs` beside `confirm_delete_agent`,
title `Quit nebula`, message `Leave the TUI?\nSessions keep running in the daemon.`) carrying the
`PendingAction::Quit` that had sat unused in `app.rs::PendingAction` since it was added. The CONFIRM
DIALOG handler gained one arm before its `_ => {}`: `Ctrl+C` on a `PendingAction::Quit` confirm commits
immediately, so the terminal's own quit chord pressed twice is never a trap. Tests:
`quit_asks_before_it_closes_the_tui` (both chords ask, `Esc` backs out, `y` goes through, `Ctrl+C` twice
quits, message lines stay under the dialog's 52-column floor), `keys_route_by_focus` updated to press
`y` after `q`, and `e2e_tui.rs`'s clean-quit path now waits for `Quit nebula` and sends `y`. Gate:
`cargo test --workspace` all green (nebula-tui 543), clippy clean.

**Gotchas:**
- **`PendingAction::Quit` existed and nothing constructed it** — `run_pending_action` had the arm, the
  enum had the variant, and `TERMS.md`'s CONFIRM DIALOG row already listed "quit" among the actions it
  gates. The glossary described the intended design, not the shipped one; the wiring was the whole task.
- **HOSTS HANDOFF sets `should_quit` directly and must keep doing so** (`event_loop.rs` ~3081, ~3110,
  ~6059 — Enter, a typed destination, and a click in the HOSTS PICKER). Those are not the quit ACTION;
  gating them would put a dialog in front of `nebula ssh` re-exec. Only the KEYMAP dispatch changed.
- **In raw mode the CONFIRM DIALOG is the only thing between the user and the exit** — `Ctrl+C` arrives
  as a key event, not SIGINT, so without the extra arm a confirm that somehow stopped taking `y` would
  be inescapable. Same reasoning as the HARDWIRED UNLOCK.
- **`q` in a LOCKED PANE and `q` closing an overlay were already separate code paths** (the terminal
  branch returns before the KEYMAP lookup; overlays are handled above it), so neither needed touching —
  `keys_route_by_focus` already pinned the LOCKED PANE half.
