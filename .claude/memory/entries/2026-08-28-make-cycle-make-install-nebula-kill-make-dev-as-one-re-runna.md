# `make cycle`: MAKE INSTALL → NEBULA KILL → MAKE DEV As One Re-runnable Target — 2026-08-28

**Asked:** "update makefile to include a kill & install & dev all in one I can keep re-running when I
need" → picked (prompt-daddy): *Chain the three targets* — MAKE INSTALL, then NEBULA KILL (the real daemon
and every SESSION), then MAKE DEV, install first so a build failure stops before anything is killed; the
existing `install`, `kill` and `dev` targets unchanged.

**Did:** Added `Makefile::cycle` — three `$(MAKE) --no-print-directory` recipe lines (`install`, `kill`,
`dev`), listed in `make help` and `.PHONY`, plus a line in the header comment. Verified with `make help`
and `make -n cycle`; did **not** run it for real — the kill step would take down the nebula session the
agent was running in.

**Gotchas:**
- Order is install → kill, not the user's kill → install: `cargo build --release` failing after the kill
  would have stopped every real SESSION for nothing. The STALE DAEMON NOTE `install` prints is then
  immediately acted on by `kill`, which is the point.
- `nebula kill` is safe on a cold machine: `run_kill` (`nebula-tui/src/lib.rs`) prints "no nebula daemon
  running" and exits 0 when nothing is listening, and `ipc::kill_daemon` waits for the daemon to exit
  (`wait_for_daemon_exit`) before returning — so `dev` never races the old daemon.
- The steps are recipe lines rather than prerequisites (`cycle: install kill dev`) on purpose: `make -j`
  may run prerequisites in parallel and could kill before the build finished.
- `make cycle` from inside a nebula SESSION kills that session mid-run. Run it from a terminal outside
  nebula.
