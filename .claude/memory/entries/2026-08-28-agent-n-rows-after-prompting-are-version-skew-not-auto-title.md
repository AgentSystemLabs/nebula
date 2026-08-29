# `agent-N` Rows After Prompting Are VERSION SKEW, Not A Broken AUTO-TITLE — 2026-08-28

**Asked:** "there are a lot of sessions that still just are named agent-1 agent-2 even after prompting
claude. did the auto name break?"
→ refined: Several SESSIONS in the SESSIONS PANEL are still named `agent-1`, `agent-2`, `agent-3` after
I've sent Claude a prompt in them, so AUTO-TITLE ("the auto name") didn't retitle them. Find out why and
tell me before changing anything — is AUTO-TITLE broken, or is something else (VERSION SKEW, the hook)
blocking the `nebula rename` it runs? (Assuming this is the DEV INSTANCE started with `make dev` at 20:19
today, and the three untitled rows are the ones created 21:09–21:14.)

**Did:** Diagnosis only, no code changed. AUTO-TITLE is intact: the HOOK RECEIVER still injects the
AUTO-TITLE INSTRUCTION (this very session received it) and `hooks/mod.rs` re-injects it on *every*
`UserPromptSubmit` while `agents.auto_title_pending = 1`. What fails is the CLI half: the DEV INSTANCE
daemon (`target/debug/nebula daemon`, pid 43151, started 20:19 at PROTOCOL VERSION 32 = HEAD/v0.16.0)
vs the bare `nebula rename` resolving on PATH to `~/.cargo/bin/nebula`, which another session's
`make install` replaced at 21:09 with a v33 build (the SHARED CHECKOUT is uncommitted at v34 now). Every
`nebula rename` since then dies with `protocol mismatch: the daemon speaks v32, this client v33`. The
SQLITE STORE proves it: `agent-1/2/3` (`created_at` 21:09:09, 21:12:02, 21:13:51) all have
`auto_title_pending=1`; every row created before 21:09 is titled. Fix left to the user: quit the dev
TUI (`dev-stop` kills the dev daemon and its seven live SESSIONS) and `make cycle` / `make install` +
`make dev` so both binaries are v34; the pending rows self-title on their next prompt, or `r` RENAME
now. Proposed enforcement: route `nebula rename` over the unversioned HOOK RECEIVER
(`$NEBULA_API_URL`, AGENT ENV already carries the BEARER TOKEN) so VERSION SKEW can't break it.

**Gotchas:**
- The tell is the row set, not a log: the DAEMON LOG records nothing for a refused handshake (the v33
  client refuses client-side before sending), so `select name, auto_title_pending, created_at from
  agents` in the instance's `nebula.db` against `ls -la ~/.cargo/bin/nebula`'s mtime dates the break.
- A DEV INSTANCE is exposed to *other sessions'* `make install`: its daemon is `current_exe()` and stays
  put, but AUTO-TITLE's bare `nebula rename` follows whatever PATH holds now — so a sibling session
  cutting a build silently untitles every SESSION this instance starts afterwards.
- `make kill` / `make cycle`'s kill step stops the *real* daemon (`/tmp/nebula-501`), not the dev one;
  the dev daemon only dies via `dev-stop` (run by `dev-prep` and on TUI exit), so re-running `make dev`
  is what actually cuts a DEV INSTANCE over.
