# Workspaces Are Per Instance, Not Daemon-Global — 2026-08-24

**Asked:** "when I load up 2 separate nebula instances, they both seem to switch workspaces when one
does... this isn't how it should work, each new nebula instance can point to a different workspace (or
even point to a separate host - verify that is possible)" Then: "please verify that this issue won't
happen again when I'm doing development" and "update nebula-memory as well after you've confirmed this
is fixed."

**Did:** The OPEN WORKSPACE was daemon-global: `store.set_active_workspace` plus a
`ServerEvent::ActiveWorkspaceChanged` broadcast every client applied. Deleted that event outright
(PROTOCOL_VERSION **22 → 23**) and moved the scope onto the connection — `handle_client` in
`crates/nebula-daemon/src/server.rs` holds `workspace: Option<WorkspaceId>`, `OpenWorkspace` sets it,
and `add_project` takes it as a new 4th arg. `registry.rs::open_workspace` became
`set_default_workspace` (persists, notifies nobody). TUI-side, `switch_workspace` and
`reseat_deleted_workspace` in `event_loop.rs` replaced the removed `ActiveWorkspaceChanged` arm, and
`apply_startup_workspace` lands the new `nebula --workspace <name>` flag (STARTUP WORKSPACE) on the first snapshot.
Separate hosts already worked and needed no change (see gotchas).

**Gotchas:**
- **Per-connection state alone is not enough; it has to be pinned at `Subscribe`.** The first cut left
  `workspace = None` until a client switched, falling back to the store default — so instance B, which
  had never touched its workspace, silently followed A's switch on its next `AddProject`. "The current
  default" is not a stable answer once anyone can move it. `e2e_pty::workspace_scope_is_per_connection`
  is the test that caught it; it fails on the None-fallback version.
- `None` still has to survive for connections that **never** subscribe — that is the one-shot
  `nebula add`, whose workspace genuinely is the current default. Don't default it at connect time.
- `apply_startup_workspace` must run **before** `restore_ui_state` in the Snapshot arm: the restored
  project id only resolves against the workspace actually on screen.
- Semantics that changed and are now documented: **`nebula workspace open <name>` no longer switches a
  running TUI.** It sets where the *next* instance boots. Aiming one live window is `nebula --workspace
  <name>`. Removing the broadcast is exactly the reported bug, so there was no way to keep both.
- **Separate hosts already work and are fully independent** — NEBULA SSH (`nebula ssh HOST`, `crates/nebula/src/ssh.rs`)
  `exec`s `ssh -t` and runs a whole remote nebula with its own daemon, socket and SQLite. Two local
  instances against different daemons also work via `NEBULA_RUNTIME_DIR` + `NEBULA_DATA_DIR`. Note the
  TUI's `h` picker (HOSTS PICKER) is a *handoff* (HOSTS HANDOFF): it quits the local TUI and execs over it rather than opening a second
  window.
- `crates/nebula/tests/e2e_tui.rs`'s `FOOTER_TERMINAL_LOCKED` had been stale since `87d2b24` — it
  expected `"Ctrl+q: panels"` while `KeyChord::display()` renders `^q`. Six e2e_tui tests were failing on
  main for that alone; fixed to `"^q: panels"`. If E2E TUI fails on a FOOTER string, suspect the constant
  before the code.
- Verified live, not just by unit test: two TUIs in one tmux server against an isolated daemon
  (`NEBULA_RUNTIME_DIR=/tmp/nbws`, short for SUN_LEN). Pane 0 pressed `w j Enter` → footer `◇ client`
  showing its project; pane 1 stayed `◇ default` showing its own. Full suite: 553 tests green.
