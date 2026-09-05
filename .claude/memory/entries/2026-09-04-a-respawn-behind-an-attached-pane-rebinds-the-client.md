# A Respawn Behind An ATTACHed Pane Rebinds The Client Instead Of Freezing It — 2026-09-04

**Asked:** "when a user is focused on the main branch and tells claude to do the work in a work tree, it
correctly creates the new work tree, but there seems to be a small bug where the existing session in
focus loses connection and stops updating until a users clicks away then focuses on the new session in
the other worktree session list. either fix"
→ refined: When the selected SESSION in the ROOT WORKTREE runs `nebula worktree <name>` (WORKTREE
RELOCATION), the WORKTREE row is created and the SESSION row moves under it correctly, but the TERMINAL
PANE freezes on the old PTY's last frame and stops streaming until I move the cursor off the SESSION and
back onto it under the new WORKTREE in the SESSIONS PANEL. Fix it so the pane re-ATTACHes to the
respawned PTY SESSION on its own when the relocation completes, keeping FOCUS and the LOCKED PANE where
they were. (Assuming Claude in Ghostty; keep the row-now / respawn-at-Stop two-phase design as it is.)

**Did:** Cause was in the DAEMON, not the TUI. A connection's forward task (`forward_pty`, was in
`crates/nebula-daemon/src/server.rs`) held one `Arc<PtySession>` and its event receiver; `complete_pending_move`
(also `restart_agent`, `attach_cloud_agent`, the CLOUD MIRROR tick) kills that PTY and `install_session`s a
new one under the same `SessionRef`, so the task sent `SessionExited` and ended, the connection's `attached`
map kept a dead handle, and the TUI — `attached_sref` still equal to the sref — deduped every later
`attach()` / `send_attach()`; only walking off the row (Detach) and back (Attach) rebound it. Fix: new
`crates/nebula-daemon/src/attach.rs` (`bind`, `forward`, `step`; the PTY plane extracted from `server.rs`,
which keeps only the inline first `bind` in its Attach arm), a `Daemon::session_installs`
broadcast fired at the end of `registry.rs::install_session`, and a forward task that `select!`s over the PTY
events and that broadcast: after `Exited` it parks (`live = None`), and on an install for its ref (or a
`Lagged`) it re-`bind`s to `daemon.session(&sref)` when the Arc differs — fresh `Scrollback`, `KittyFlags`,
`resize_with_jiggle` to the client's `PaneSize`. No TUI or PROTOCOL VERSION change: `AttachedTerm::reset()` on
`Scrollback` already clears `exited`. Gate: E2E PTY 27 passed (new `restart_rebinds_an_attached_client_to_the_new_pty`;
`nebula_worktree_cli_relocates_the_session_when_the_turn_ends` now attaches before the relocation and asserts
the rebind's `Scrollback` plus the new boot's `booted in …/feat-x`), nebula-daemon 162 passed in a SCRATCH
WORKTREE, clippy clean. TERMS ATTACH row and `docs/how-it-works.md` updated. Not verified in the live TUI:
the running DAEMON is the old binary until `make install` + `nebula kill` + relaunch.

**Gotchas:**
- The symptom reads as a TUI bug ("clicking away and back fixes it") but the TUI was right by its own
  contract: `attached_sref == sref` dedups every re-attach and `SessionExited` only sets `term.exited`, so
  nothing on the client can know the daemon swapped the PTY. Every kill-and-respawn under one ref
  (Restart in the CONTEXT MENU, cloud re-entry, the mirror re-teleport) froze the pane the same way —
  one daemon-side rebind fixes all of them; a client-side re-attach on `SessionExited` would race
  `complete_pending_move`'s own spawn through `ensure_session` (two CLIs).
- The first `bind` (the attach `Scrollback`) must stay inline in the request loop, not move into the
  spawned task: it is queued before any later reply, and e2e predicates that expect the replay ahead of a
  following Ack depend on that order.
- The old child's `Exited` lands before *or* after the new PTY's install (SIGHUP vs. fork), so the rebind
  swaps receivers with the binding — a late `SessionExited` from the replaced PTY can never mark the new
  one dead — and the client side needs nothing because a `Scrollback` resets the pane.
- `tokio::select!` over an `Option`-held receiver — `ev = async { live.as_mut().expect("bound").rx.recv().await }, if live.is_some()`
  — compiles and lets the other arm reassign `live` (branch futures are dropped before a handler runs).
  `live` is an `Option`, not a bool, so the parked task drops the old `Arc<PtySession>` and does not pin
  its 1 MiB SCROLLBACK RING.
- Proving a rebind in E2E PTY without a second Attach: the STUB AGENT echoes `booted $n` (a counter
  file) on every boot; assert a `Scrollback` for the sref among the events read *after* the restart / Stop
  plus the new boot's text via `collected_output` — and consume the first attach's `Scrollback` with an
  earlier `read_events_until`, or it satisfies the predicate trivially.
- SHARED CHECKOUT: minutes after `cargo build -p nebula-daemon` passed, another session's
  `CreatePrAgent { kind }` in `protocol.rs` made `cargo test -p nebula-daemon` fail to compile (E0027 at
  `server.rs:363`) — gated instead in a SCRATCH WORKTREE at HEAD with only my five files copied in and
  its own `CARGO_TARGET_DIR`.
