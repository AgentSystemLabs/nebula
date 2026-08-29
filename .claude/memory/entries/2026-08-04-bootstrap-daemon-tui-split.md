# Bootstrap: Daemon/TUI Split — 2026-08-04

**Asked:** "I want to build out a cli tool which is performant, uses very little memory, but kind of acts
like a multi plexer to allow creating new terminal windows (similar to ghostty). the main things I need to
include, like the peak user experience I'm going for is. left side panel for project, then if you c…"

**Did:** `47037e8`. Cargo workspace `crates/{nebula-core,nebula-daemon,nebula-tui,nebula}` shipping one
binary. A detached tmux-style daemon owns the PTYs (portable-pty, 1MB byte-ring scrollback with seq
numbers); the TUI attaches over a unix socket with length-prefixed MessagePack (`nebula-core/src/codec.rs`).

**Gotchas (locked decisions — user-approved, don't relitigate):**
- **No server-side VT grid.** Attach replays the ring into the client's vt100 parser plus a SIGWINCH
  resize-jiggle.
- **tui-term is a renderer only**, kept behind `nebula-tui/src/ui.rs` as a swap point.
- **Status comes from agent hooks, not MCP** — MCP was proven unreliable in ../mission-control. Managed
  hooks are merged into the worktree's settings and curl a loopback axum server with a per-boot bearer
  token. Keep the logic in the pure `AgentStatusMachine` (`nebula-daemon/src/status.rs`, unit-tested with
  injected clocks) and **never trust a bare `Stop`**.
- Kitty keyboard protocol passthrough (`nebula-daemon/src/pty/kitty.rs`) is what makes Cmd/Option combos
  and Shift+Enter reach Claude Code at all.
- **Unix socket paths must stay short** — SUN_LEN is ~104 bytes, so a long `NEBULA_RUNTIME_DIR` breaks
  `bind()`. This bites the test harnesses and the screenshot harness constantly.
- Ideas were borrowed from ../mission-control, but **all code is written fresh** — that was a hard user
  requirement.
