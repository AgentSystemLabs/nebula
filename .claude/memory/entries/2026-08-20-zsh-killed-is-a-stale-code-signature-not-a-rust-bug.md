# `zsh: killed` Is A Stale Code Signature, Not A Rust Bug — 2026-08-20

**Asked:** "debug why when I run nebula if fails … `nebula upgrade` → `zsh: killed nebula upgrade` …
`nebula` → `zsh: killed nebula`" Same thread: "nebula fails when I try to run it, give me hte proper
commands I should run locally to use the latest built version" → "make that into a single script and maybe
a makefile" → "rename kill-server to just kill, do that everywhere kill-server is too verbose."

**Did:** Added the `Makefile` for the local dev loop and renamed `kill-server` → `kill` (KILL-SERVER → NEBULA KILL).

**Gotchas:**
- The crash report says `SIGKILL (Code Signature Invalid)` / `Taskgated Invalid Signature` **even though
  `codesign -vv ~/.cargo/bin/nebula` reports valid on disk**. Cause: `cargo install --path` rewrote the
  binary **in place (same inode)** while the kernel held a cached signing blob for that vnode, so every
  later exec was killed.
- Fix is to refresh the inode, not the code (what MAKE INSTALL's cp + mv does):
  `cp ~/.cargo/bin/nebula ~/.cargo/bin/nebula.new && mv -f ~/.cargo/bin/nebula.new ~/.cargo/bin/nebula`.
  Identical bytes on a fresh inode exec fine.
- Confirm before debugging anything else: `~/Library/Logs/DiagnosticReports/nebula-*.ips`.
- A lingering `nebula daemon` from the old inode keeps running **old code**. NEBULA KILL (`nebula kill`) is the user's
  call — it stops live sessions.
