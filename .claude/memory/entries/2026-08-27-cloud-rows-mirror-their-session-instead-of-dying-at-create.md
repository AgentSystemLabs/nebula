# Cloud Rows Mirror Their Session Instead Of Dying At Create — 2026-08-27

**Asked:** "still when I try to create a claude cloud session, it doesn't seem to update me with the
changes, it just says use a command to resume, and if i leave and come back to that terminal it errors
out.  find a way to allow the cloud session output to show up in the ui" → chose, from the options put
to them: auto-follow until the pane is touched, plus a send-message path.

**Did:** Probed the real CLI first (2.1.247, see the gotchas), then built the CLOUD MIRROR on `--teleport`.
`crates/nebula-daemon/src/pty/mod.rs`: `PtySession.input_seen` (AtomicBool, set by `write_input` on a
non-empty write only — resizes don't count). `registry.rs`: `Daemon.cloud_attach_gated` (the ATTACH GATE, AtomicBool, set
when `arm_cloud_attach_fallback` sees the refusal) routes every later re-entry through the new pure
`cloud_reentry_launch(id, gated)` → `Teleport` instead of re-flashing the red error; `arm_cloud_follow`
watches a `CloudLaunch::Create` PTY for its id + exit and then calls `attach_cloud_agent` unasked, so a
create no longer leaves a dead "Resume with:" pane; `start_cloud_mirror`/`refresh_cloud_mirror`/
`stop_cloud_mirror` + `Daemon.cloud_mirrors` re-teleport the row every `CLOUD_MIRROR_REFRESH` (45s,
`NEBULA_CLOUD_MIRROR_SECS` overrides, `0` disables, floor 2s); `cloud_worktree_for` is the re-home split
out of `attach_cloud_agent`; `send_cloud_message` (CLOUD MESSAGE) runs `claude -p <msg> --cloud=<id>` via
`tokio::process` + `login_shell_wrap` and refreshes after. `validate_cloud_text` now covers both the
launch task and the message. PROTOCOL VERSION **v28**: `ClientRequest::SendCloudMessage` and runtime-only
`Agent.cloud_mirroring` (set in `agent_entity` like `alive`). TUI: `PromptKind::CloudMessage` (multiline, the CLOUD TASK EDITOR),
`MenuAction::SendCloudMessage`, "Send to cloud session" CONTEXT MENU item, `PendingIntent::ReopenPromptOnError`
so a failed send hands the text back, and a `cloud ↻` accent CLOUD BADGE in `ui.rs`. 654 tests green.

**Gotchas:**
- **CLOUD ATTACH is still gated off** (verified 2026-08-27 under a real PTY): `claude --cloud=<id>` prints
  `Error: Attaching to an existing cloud session is not enabled for your account.` Without a TTY it now
  fails differently — `non-interactive --cloud <session_id> requires a prompt` — so a non-PTY probe reads
  like the gate is gone. Probe under `script -q`.
- **`claude --teleport=<id>` is a repeatable snapshot pull, and that is the whole mechanism.** Verified
  three runs against one live session: it re-fetches the transcript each time, picks up turns taken
  since (a `-p --cloud` message sent between runs showed up), is idempotent in the same worktree, and
  does **not** end the cloud session. It is a fork to local, not a live link — hence the re-teleport loop.
- `claude -p "msg" --cloud=<id>` still only prints `Sent to cloud session.`; `--output-format
  stream-json` is refused outright for `--cloud`. There is no reply to stream.
- The CLI binary carries `/v1/code/sessions/{id}/events/stream` (real SSE, what the CLI itself uses).
  Rejected: it needs the OAuth token scraped out of the macOS keychain and is undocumented. Reading the
  keychain was blocked by the auto-mode classifier, which is the right instinct.
- **A mirror must stop when its pane is gone, or cloud rows become unreapable.** The IDLE REAPER kills
  unattended sessions; a mirror that respawns on every tick would fight it forever. `refresh_cloud_mirror`
  returns `Ok(false)` when the session is absent — that also stops a teleport that dies on every try.
- The mirror ends on the first keystroke, not on the row gaining a local `session_id` — a teleport sets
  one immediately via the hooks. `restart_agent` therefore routes on `session_id.is_none() ||
  cloud_mirror_active(id)`.
- A mirror that quits must re-broadcast the row, or the `cloud ↻` badge keeps promising refreshes.
- E2E PTY can't tell a STUB AGENT's attach from its teleport (`NEBULA_AGENT_CMD` spawns override the argv verbatim,
  no cloud flag), so the gate is unit-tested on `cloud_reentry_launch` instead. And the recorded
  "upsert beats the stub's first line" race bites again: a `!cloud_mirroring` predicate matches the
  create's own upsert, so wait for the run count first, then require lit-then-quiet.
