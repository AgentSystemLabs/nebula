# The DAEMON Needs Its Own Session, Not Just A Process Group — 2026-08-20

**Asked:** "sometimes nebula will enter this state when I try to start a new claude terminal, it just
keeps writing strange tokens and the entire app is broken basically, I can't interact, it just happened
in a previous session I tried to open"

**Did:** `4502575` (DAEMON SETSID). `spawn_daemon` in `crates/nebula-tui/src/ipc.rs` now calls `setsid()` in `pre_exec`
instead of only creating a new process group, so the DAEMON holds **no controlling terminal** and nothing
it spawns can reach the user's terminal through `/dev/tty`. The `zsh -l -i -c "command -v claude"` CLI
PROBE in `nebula-daemon/src/registry.rs` also `setsid()`s (so even a `--foreground` daemon can't have the
probe shell steal a tty) and gained `.kill_on_drop(true)` — previously a hung probe leaked the child
forever when the 5s timeout dropped the future.

**Gotchas:**
- The garbage tokens were a **shell job-control fight over the controlling terminal**, not a rendering or
  VENDORED VT100 bug. A new process group is not enough; it must be a new *session*.
- With no controlling tty, zsh's `/dev/tty` open fails and it skips job-control init entirely — that's the
  mechanism, and it's why the fix is one call in the right place.
