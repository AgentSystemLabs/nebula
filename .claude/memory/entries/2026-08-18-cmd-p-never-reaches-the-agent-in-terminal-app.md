# Cmd+P Never Reaches The Agent In Terminal.app — 2026-08-18

**Asked:** "when I try command + p in a claude session, it just pastes the pi character and recommends I
run /setup-terminal which I already have, can you figure out if maybe command + p is not properly being
sent to the claude session? this is inside a terminal.app I'm running nebula. this works perfectly fine…"

**Did:** No code change — diagnosed as not-a-nebula-bug and gave remedies.

**Gotchas:**
- Terminal.app **never encodes Cmd into pty bytes** (⌘P is File→Print at the menu layer). The press
  arrives as Option+P's character `π`. Nebula's chain was verified sound end to end: kitty probe in
  `event_loop.rs` setup_terminal → legacy encoder swallows SUPER (`keys.rs` `encode_legacy`) → kitty
  re-encode would have sent `\x1b[112;9u`.
- Agent PTYs get `TERM=xterm-256color` (`pty/mod.rs`) but inherit the **daemon's** `TERM_PROGRAM`, so
  `/terminal-setup` run inside nebula detects whatever terminal the daemon was first spawned from, not
  the one currently attached.
- Remedy given: `/model` opens the same picker, or bind `ctrl+p` → `chat:modelPicker` in
  `~/.claude/keybindings.json`.
