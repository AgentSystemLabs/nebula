# Switching Workspaces Kept The Project Cursor At Row 0 — 2026-08-26

**Asked:** "when i switch between workspaces it should remember the last project, worktree, session
slection"

**Did:** Two of the three were already implemented and simply unreachable. `remember_context` /
`restore_context` (`crates/nebula-tui/src/event_loop.rs`) have kept `App::last_worktree_for_project` and
`App::last_session_for_worktree` since the panel work — but `switch_workspace_inner` hard-set
`app.sel_project = 0`, and both maps are keyed off the project the cursor lands on, so coming back to a
workspace restored *the first project's* worktree and session. Added
`App::last_project_for_workspace: HashMap<WorkspaceId, ProjectId>` (`crates/nebula-tui/src/app.rs:1920`),
recorded at the top of `remember_context`, and new `restore_workspace_project(app)` called from
`switch_workspace_inner` immediately before `restore_context` — only on the `restore: true` path.
Test `switching_back_to_a_workspace_restores_project_worktree_and_session`. 650 workspace tests green,
clippy clean.

**Gotchas:**
- **Order is load-bearing.** `restore_context` reads `selected_project()` to find the remembered
  worktree, and `restore_session` reads `selected_worktree()`. The project has to land *first* or the
  other two restore against row 0's context — which is the original bug, just moved.
- **`remember_context` early-returns when the selected project has no worktree** (`let Some(wid) = …
  selected_worktree() else { return }`). The per-workspace project record goes ABOVE that return, or an
  empty project silently never gets remembered.
- **`switch_workspace_quietly` must keep landing on row 0.** Restoring there re-introduces the
  attach-then-detach double the `/`-crosses-workspaces work added the quiet path to avoid
  (see [The Workspaces Column Remembers Itself]).
- **A one-project-per-workspace test can't fail.** Row 0 and "the row we left on" have to differ at all
  three levels, so the test seeds a second project (`p2`) with a non-main worktree (`w2b`) and its own
  agent. Same shape as the "only discriminates if the remembered session differs" trap in the 08-25
  entry — confirmed by commenting out `restore_workspace_project` and watching it go red
  (`left: Some("demo")`).
