# Cancelling Claude Left The Status Stuck — 2026-08-23

**Asked:** "I noticed that when I cancel Claude code, it never actually changed the status back to green
from that yellow animation. Can you debug and fix this?"

**Did:** Added `crates/nebula-daemon/src/pty/progress.rs`, which scans the PTY byte stream for OSC 9;4
progress edges; the pump emits `PtyEvent::Progress` and `status.rs` treats "progress cleared" as a
synthetic `Stop` (same subagent-drain bookkeeping), but only from Running/NeedsFeedback.

**Gotchas:**
- Esc-cancelling a Claude turn fires **no hook at all**. `Stop` is documented not to run on user
  interrupt, and the `idle_prompt` Notification that normally rescues a hookless turn end is suppressed
  because Claude gates it on 60s quiet **AND** the user not having touched the keyboard — pressing Esc
  *is* touching it. Verified against Claude Code 2.1.241 with a `pty.fork` harness; only
  `UserPromptSubmit` then `SessionEnd` ever fired.
- The window **title** is unusable as a busy/idle signal — during a permission prompt it shows idle (`✳`)
  while the OSC 9;4 progress state correctly stays busy (`3`). Trust the progress state, never the title,
  or you will green out an agent that is waiting on the user.
- Codex and cursor-agent emit no OSC 9;4 at all, so this path is inert for them.
