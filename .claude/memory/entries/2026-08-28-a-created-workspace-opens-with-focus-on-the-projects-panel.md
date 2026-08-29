# A Created Workspace Opens With Focus On The PROJECTS PANEL — 2026-08-28

**Asked:** "when someone makes a new workspace, it should focus on the projects list"
→ picked (prompt-daddy): *All three create paths* — "When I create a WORKSPACE from the TUI — `n` in the
WORKSPACE SWITCHER, `n` in the WORKSPACES BAR, or "New workspace" in the bar's `m` menu — and the TUI
opens it on the daemon's Ack, land focus on the PROJECTS PANEL instead of leaving it where it was … Keep
the open-on-Ack switch exactly as it is; only the landing focus changes. Cover it with a unit test."

**Did:** One landing site covers all three paths: every `PromptKind::NewWorkspace` submit allocates
`PendingIntent::OpenCreatedWorkspace`, and the `ServerEvent::Ack` arm for it
(`crates/nebula-tui/src/event_loop.rs` ~6434, `handle_server_event`) already called `switch_workspace`;
it now also sets `app.term_locked = false; app.focus = Focus::Projects`. Tests:
`a_workspace_created_from_the_bar_lands_focus_on_projects` (new, starts on `Focus::Workspaces`) and
`switcher_creates_a_workspace_and_opens_it_on_ack` (now starts on Sessions and asserts the landing).
README rows for the WORKSPACE SWITCHER and Workspaces keymap note it. 468 nebula-tui tests green.

**Gotchas:**
- Focus stays put between the prompt's Enter and the Ack — the switch and the landing both happen on the
  Ack, not on submit. A test that asserts `Focus::Projects` right after Enter fails; feed the `Ack` with
  `created: Some(EntityId::Workspace(..))` first (the existing switcher test shows the shape).
- The two clippy warnings the crate prints (`event_loop.rs:5183` unneeded return, `config.rs:1007` field
  assignment after `Default::default()`) predate this task — they are other sessions' hunks in the shared
  tree, not this one's.
