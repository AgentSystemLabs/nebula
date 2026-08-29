# The Focus-Key Odyssey → Ctrl+Q — 2026-08-04

**Asked:** "make cmd arrow change focus of the panels, require an enter of the session panel to focus lock
into it" — which turned into a long elimination, punctuated by "I'm not even using ghostty you fuck" and
ended by "fuck it go back to control + q, also shift drag doesn't do shit. fix it".

**Did:** Ctrl+Q is the unlock/escape hatch. Fallbacks kept: Ctrl+] / Ctrl+Esc / Ctrl+←. Shift-drag was
replaced with app-side plain drag-selection in the terminal pane (REVERSED overlay for highlight, text via
vt100 `contents_between`, `pbcopy` on mouse-up).

**Gotchas:**
- **The user runs Terminal.app**, not Ghostty, despite Ghostty being installed. Terminal.app fails the
  kitty-keyboard probe, so Cmd-modified keys and Ctrl+Esc never reach the app there.
- Everything else was eliminated for a reason: Cmd+arrows (no kitty protocol), Ctrl+arrows (Mission
  Control), Ctrl+Esc / Option+Esc (undeliverable), Ctrl+]: vetoed on feel, double-Esc: implemented then
  reverted because Claude Code owns Esc, Shift+arrows and Ctrl+G/T: Claude Code binds them. **Ctrl+Q is
  settled — don't relitigate it**; the user's Cmd+Q-adjacency worry lost to familiarity.
- crossterm collapses a same-read `\x1b\x1b` pair into **one** Esc event (escaped-escape rule), which is
  what made double-Esc unworkable.
- "Shift+drag selects text" is a lie in Terminal.app — there's no mouse-reporting bypass there, unlike
  Ghostty/iTerm.
- The user runs `nebula` via a `~/.cargo/bin` symlink to `target/release` (as of 2026-08-27 it is a
  regular file, and the live TUI was `target/debug` — check `ps` for which) — **rebuild and restart the
  TUI** before testing keybinding changes, or you are testing a stale process.
