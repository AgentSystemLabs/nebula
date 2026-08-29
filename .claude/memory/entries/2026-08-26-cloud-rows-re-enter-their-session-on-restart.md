# Cloud Rows Re-Enter Their Session On Restart — 2026-08-26

**Asked:** "find if there is a way to attach claude when waiting for the cloud to finish so thhat they
don't need to go into abrowser to use" → after the diagnosis (live attach is flag-gated off for this
account, see the 08-24 Cloud entry): "yes do it" — capture the `session_…` id from the spawn output, keep
it on the Agent row, try `--cloud <id>` and fall back to `--teleport <id>` in a fresh worktree.

**Did:** `Agent.cloud_session_id` (entities.rs, store MIGRATION 20, PROTOCOL VERSION v26 — rmp positional
structs, so a new field is a bump). The CLOUD SCANNER (`crates/nebula-daemon/src/pty/cloud.rs::CloudScanner`) reads the id
(`claude.ai/code/session_…` / `--teleport session_…`) and the attach refusal (`… not enabled for your
account`) off the PTY stream; `PtySession::arm_cloud_scan` replays the ring first so arming after spawn
cannot miss it; sightings are `PtyEvent::CloudSession` / `CloudAttachRejected`, persisted in
`watch_for_exit`. `registry.rs`: `CloudLaunch::{Create,Attach,Teleport}` (CLOUD LAUNCH / CLOUD ATTACH / CLOUD TELEPORT) drives
`claude_cloud_spawn_command` (`--cloud=<task>`, `--cloud=<id>`, `--teleport=<id>`);
`restart_agent` (now async) routes a row with `cloud_session_id` and no local `session_id` to
`attach_cloud_agent`, which re-homes a main-checkout row into a `cloud-<last 8 of id>` CLOUD WORKTREE, spawns
the attach, and `arm_cloud_attach_fallback` respawns as a teleport once the refusal was *seen* and the
child exited. **Superseded in part on 2026-08-27** ([Cloud Rows Mirror Their Session Instead Of Dying At
Create]): a create now re-enters on its own, the attach is only tried until this daemon has seen it
refused once (the ATTACH GATE), and the teleported pane keeps re-teleporting (the CLOUD MIRROR) until it is typed into. `ClientRequest::AttachCloudAgent` + the "Attach cloud session" menu item force the chain
any time; the SESSIONS PANEL shows the CLOUD BADGE. e2e `cloud_row_captures_its_session_id_and_reenters_it`
walks the whole chain with a three-run stub. README step 4 documents it.

**Gotchas:**
- The CLOUD TELEPORT fallback must key on the refusal text, not on "exited non-zero fast": a deliberate kill
  (restart/archive) of a *working* attach exits non-zero too and would have spawned a stray teleport.
- Both `--cloud` and `--teleport` take an *optional* value — always bind with `=`; verified both forms
  parse (`--cloud=<id>` → the refusal, `--teleport=<id>` → teleport's stash prompt).
- Teleport refuses a dirty tree ("Stash changes and continue?") and both CLIs switch the checkout's
  branch, hence the mandatory fresh worktree for rows in the ROOT WORKTREE. The placeholder
  `cloud-…` branch stays behind once teleport checks the cloud branch out on top of it.
- `SessionEnded` only flips status from Running/NeedsFeedback, so the dead create row stays gray
  FRESH (`Fresh`) — the CLOUD BADGE and `alive:false` are the only tells. (The create no longer leaves a dead
  row at all as of 2026-08-27, but a failed create still does.)
- In E2E PTY, a spawn's `EntityUpserted{alive:true}` reaches the client before the stub has executed a
  line: don't assert on stub side effects inside the `read_events_until` predicate (it is only
  re-evaluated per event) — wait for the event, then poll the file.
- Running `claude` from an untrusted dir (the scratchpad) hangs on the workspace-trust prompt; probe
  CLI behaviour from the repo checkout.
