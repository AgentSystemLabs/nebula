# e2e Daemon-Boot Failures Have Two Different Causes — 2026-08-21 → 08-23

**Asked:** (no prompt — both surfaced while verifying other work)

**Did:** Nothing to commit. Both are environmental, and telling them apart saves hours.

**Gotchas:**
- **Cold-exec flake.** All 16 E2E PTY (`e2e_pty`) tests fail with `daemon socket never appeared`. First exec of a
  freshly relinked `target/debug/nebula` can stall for seconds on macOS signature validation, so the test
  panics at its 5s deadline, `TempDir` drop deletes the runtime dir, and the late daemon logs
  `FATAL bind …/daemon.sock: No such file or directory`. Fingerprint: orphaned
  `$TMPDIR/.tmp*/data/state/daemon.log` files. **Just rerun** — it passes clean the second time.
- **ORPHAN DAEMONS — leak fixed at the source 2026-08-24.** Same generic error, but **no `daemon.log`
  is written at all** and reruns don't help; a test that passes in the full suite fails alone, seemingly
  at random. Cause: dozens of stray `nebula daemon --foreground` processes, each holding watchers/fds.
  The leak was `e2e_pty.rs`'s `TestEnv` having **no `Drop`** — a test that panicked before its closing
  `Shutdown` dropped the `std::process::Child` without killing it, and the DAEMON detaches and outlives
  the whole `cargo test` run. `DaemonProc` (a `Deref`/`DerefMut` newtype around the `Child`, defined just
  above `connect()`) now SIGTERMs on drop, so panicking tests clean up. E2E TUI's `TuiHarness` (`e2e_tui.rs`)
  always had its own `Drop`. Nothing should accumulate any more — **62 had piled up before this**, so a
  machine that predates the fix may still need one reap.
- Diagnosing a suspected orphan pile: `ps -eo pid,command | grep -c "[n]ebula daemon"`. Anything past a
  couple means leftovers. Reaping is safe **except for the live one** — filter to `target/debug/nebula
  daemon` (test daemons) and exclude `$(cat /tmp/nebula-501/daemon.pid)`, which is the
  `~/.cargo/bin/nebula daemon` running the session you are inside. Ask before bulk-killing: it's the
  user's machine and other agents' e2e runs may be in flight.
- `DaemonProc::drop` sends **SIGTERM, not SIGKILL** — the daemon's handler runs the same clean shutdown
  as `ClientRequest::Shutdown` and takes its PTY children with it; SIGKILL would orphan those instead.
  It also `try_wait()`s first: `Child::kill()` on an already-reaped child errors rather than signalling
  whatever now owns that recycled pid, but the check keeps the intent obvious. Drop only runs because
  the workspace has no `panic = "abort"` profile — verify that before relying on a drop guard here.
