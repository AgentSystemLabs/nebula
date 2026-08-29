# A Freshly Added Project Selects Itself — 2026-08-25

**Asked:** "when I open / make a new project, it should auto focus it after creating"

**Did:** Both `ClientRequest::AddProject` sites in `crates/nebula-tui/src/event_loop.rs` (the prompt
submit and the `PendingAction::CreateProjectDir` confirm) now allocate
`PendingIntent::SelectCreatedProject` instead of `None`. The Ack arm calls the new
`select_created_project` (= `select_project_row_by_id` + `restore_context` + `Focus::Worktrees`, the
same landing a `/` palette project pick does), stashing into the new `App::select_project_when_seen`
when the upsert hasn't arrived, and the `EntityUpserted` arm drains that stash — the exact
`select_worktree_when_seen` idiom. Unit test `add_project_ack_selects_the_new_project` covers both
orderings; e2e `tui_projects_worktrees_agents_navigation` asserts `beta-proj` is the selected row after
adding it. Workspace suite 601 green.

**Gotchas:**
- **The e2e helpers assume panel stability, so an auto-focus change breaks tests that never mention
  focus.** Three `e2e_tui` tests timed out because the `\r` / `e` / `n` they press right after
  `add_project` now landed in the Worktrees panel. Fix was the `create_worktree` precedent: the
  `add_project` helper itself waits for `FOOTER_WORKTREES`, sends `←`, and waits for
  `FOOTER_PROJECTS`. Any future auto-focus needs the same hop in its helper.
- **A green e2e is not evidence after a focus change.** `tui_pull_request_row_leads_the_links_group`
  *passed* against the new behavior even though its `\r` + `wait_for_text(FOOTER_WORKTREES)` no longer
  matched the real flow — the wait was satisfied by the frame before the keypress landed. In a real
  (non-`cfg(test)`) build that stale-frame pass would have carried on to press Enter on a PR link row.
  Check every test that follows the changed step, not just the ones that went red.
- rustfmt reflows a `//` comment placed on the line directly after a trailing `// …` comment into a
  continuation of it (indents it to that column). Put a blank line between them.
