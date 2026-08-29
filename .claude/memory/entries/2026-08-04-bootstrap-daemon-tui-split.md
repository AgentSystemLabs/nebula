# Bootstrap: DAEMON/TUI Split — 2026-08-04

**Asked:** "I want to build out a cli tool which is performant, uses very little memory, but kind of acts
like a multi plexer to allow creating new terminal windows (similar to ghostty). the main things I need to
include, like the peak user experience I'm going for is. left side panel for project, then if you c…"

**Did:** `47037e8`. Cargo workspace `crates/{nebula-core,nebula-daemon,nebula-tui,nebula}` shipping one
binary. A detached tmux-style DAEMON owns the PTYs (portable-pty, 1MB SCROLLBACK RING with seq
numbers); the TUI attaches over the DAEMON SOCKET with the IPC CODEC's length-prefixed MessagePack (`nebula-core/src/codec.rs`).

**Gotchas (locked decisions — user-approved, don't relitigate):**
- **No server-side VT grid.** ATTACH replays the ring into the client's vt100 parser plus a SIGWINCH
  resize-jiggle.
- **tui-term is a renderer only**, kept behind `nebula-tui/src/ui.rs` as a swap point.
- **AGENT STATUS comes from agent hooks, not MCP** — MCP was proven unreliable in ../mission-control. MANAGED
  HOOKS are merged into the worktree's settings and curl the HOOK RECEIVER (a loopback axum server) with a per-boot BEARER
  TOKEN. Keep the logic in the pure STATUS MACHINE `AgentStatusMachine` (`nebula-daemon/src/status.rs`, unit-tested with
  injected clocks) and **never trust a bare `Stop`**.
- KITTY PROTOCOL passthrough (`nebula-daemon/src/pty/kitty.rs`) is what makes Cmd/Option combos
  and Shift+Enter reach Claude Code at all.
- **DAEMON SOCKET paths must stay short** — SUN_LEN is ~104 bytes, so a long `NEBULA_RUNTIME_DIR` breaks
  `bind()`. This bites the test harnesses and the SCREENSHOT HARNESS constantly.
- Ideas were borrowed from ../mission-control, but **all code is written fresh** — that was a hard user
  requirement.
