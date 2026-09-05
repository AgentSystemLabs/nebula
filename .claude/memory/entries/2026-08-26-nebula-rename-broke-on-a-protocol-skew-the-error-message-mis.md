# `nebula rename` Broke On A Protocol Skew The Error Message Misdiagnosed — 2026-08-26

**Asked:** "why is it printing … Error: daemon speaks protocol v26, this client v24 — run `nebula kill`
and relaunch I've ran kill but the hook still seems to fail" — then "why doesn't make dev do this
already though", and after the diagnosis "what do you recomend" → fix the message, not the plumbing.

**Did:** Diagnosis: `make dev` (MAKE DEV) runs `target/debug/nebula`, which spawns its daemon from `current_exe()`
(`nebula-tui/src/ipc.rs:52`), so the daemon is always this checkout's build. But the AUTO-TITLE hook
injects a **bare** `nebula rename` (the AUTO-TITLE INSTRUCTION `AUTO_TITLE_INSTRUCTION`, `nebula-daemon/src/hooks/mod.rs:29`), and
the agent's shell resolves that on PATH to `~/.cargo/bin/nebula` — 0.9.0/v24 there against the daemon's
v26. Fix: the HANDSHAKE's `handshake()` in `nebula-tui/src/ipc.rs` now calls a new VERSION SKEW message, `version_skew_message()`, which
compares the two versions, prints both binaries' paths, and recommends MAKE INSTALL (`make install`) when the *client*
is older and NEBULA KILL (`nebula kill`) only when the *daemon* is; new `daemon_exe_path()` resolves the daemon's
binary from `paths::pidfile_path()` via `/proc/<pid>/exe` or `ps -p <pid> -o comm=`. Two tests in a new
`mod tests`. Then `make install` (0.9.0 → 0.10.0). Rejected: a daemon-side PATH shim (agents run through the LOGIN SHELL WRAP,
`$SHELL -l -i -c`, `registry.rs:2015`, and this user's `.zshrc` prepends to PATH on ~11 lines, so a
prepended dir lands behind them and breaks silently later), and an absolute path in the instruction
(`CLAUDE_ALLOW_RULES` is `Bash(nebula rename:*)` — an absolute path stops matching and every auto-title
turns into a permission prompt).

**Gotchas:**
- **Re-hit 2026-09-05, from the user's side: "why even after `make dev`?"** MAKE DEV is `cargo build` + run
  `target/debug/nebula` under `DEV_ENV`; it never writes `$(PREFIX)/nebula`, so the PATH client only moves on
  MAKE INSTALL / `make cycle`. And `nebula --version` cannot show the skew: the 37→38 bump landed in PR #26
  (`4c82532`, 23:24) an hour after the 22:21 `make install`, with no release between, so both binaries print
  `0.21.0`. Date it with `ls -l ~/.cargo/bin/nebula target/debug/nebula` against
  `git log -S'PROTOCOL_VERSION: u32 = 38' --format='%h %cd'`; `target/debug/nebula rename …` works meanwhile.
- `nebula kill` is the wrong advice when the **client** is the older side, and it was the message's only
  advice. Killing the daemon just makes the live TUI respawn an identical one from `current_exe()`, so
  the skew survives every restart — a guaranteed dead end for whoever follows it.
- Do **not** add a field to `ServerEvent::Incompatible` to carry the daemon's path. That frame is what a
  *newer* daemon sends an *older* client and rmp is positional, so the clients that need the information
  most would fail to decode it and report "unexpected handshake reply" instead.
- The BUILDSTAMP (`paths::buildstamp_path()`) is a **content hash** (`lifecycle.rs::fingerprint_file`), not a path — it
  can prove the daemon is stale but cannot name its binary. Pidfile + `ps` is the way.
- Only the *model-facing* instruction goes through the CLI. The real hooks are `curl` to the HOOK RECEIVER at
  `$NEBULA_API_URL/api/hooks/...` (`hooks/installer.rs:100`) — loopback HTTP, unversioned, fail-soft. So
  a PROTOCOL VERSION bump can only ever break the auto-title and `nebula worktree` instructions, never status
  reporting. Note the `silence` branch there: `UserPromptSubmit` keeps stdout precisely so the injected
  instruction reaches the model; every other event is fully silenced.
- The DEV INSTANCE's isolation is *inherited*, not propagated: `NEBULA_RUNTIME_DIR`/`NEBULA_DATA_DIR`
  come from the Makefile's `DEV_ENV` and flow down make → TUI → daemon → agent PTY. The daemon itself
  only ever adds `NEBULA_AGENT_ID`/`NEBULA_API_URL`/`NEBULA_API_TOKEN` (`registry.rs:2023`). Nothing
  propagates *which binary*, which is the entire hole.
- The SHARED CHECKOUT moved 26 → 27 under me mid-task (the notes-removal entry below), flipping the skew:
  after `make install` the PATH binary is v27 and the still-running dev daemon is v26, so the hook stays
  broken — now correctly reported as "The daemon is the older build". Restarting that daemon stops live
  sessions, so it was left to the user.
