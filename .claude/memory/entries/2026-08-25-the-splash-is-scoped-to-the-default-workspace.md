# The SPLASH Is Scoped To The DEFAULT WORKSPACE — 2026-08-25

**Asked:** "when a user has multiple workspaces, and he hovers over a workspace with no projects, it should
NOT show the nebula splash screen. that screen should only show when a user is on default workspace with no
projects"

**Did:** `App::splash_showing()` (`crates/nebula-tui/src/app.rs:~2170`) now requires the OPEN WORKSPACE to
be the built-in `default` one (the DEFAULT WORKSPACE) before an empty tree counts as a first run: new
`Tree::in_default_workspace()` (`app.rs:~1640`, compares `active_workspace` to
`nebula_core::DEFAULT_WORKSPACE_ID`). `splash_preview` (N) is unchanged. An empty non-default workspace
now renders the normal layout — WORKSPACES COLUMN plus the three panels with their existing "no projects
yet / n adds one" hints — instead of swapping the whole body for the nebula. The "hover" in the request is
`move_selection` in the Workspaces column (`event_loop.rs:~4864`), which does a full `switch_workspace`
per step, so previously stepping onto a fresh workspace hid the column you were stepping through.
Flipped `switching_to_empty_workspace_blanks_the_pane` to assert `!splash_showing()`, added
`empty_non_default_workspace_keeps_the_panels_not_the_splash` (TestBackend draw: "WORKSPACES" and "no
projects yet" on screen after the step, splash back after stepping to the empty default). nebula-tui: 446
green.

**Gotchas:**
- The SHARED CHECKOUT didn't compile while this was done — another session was mid-removal of the PROJECT DIVIDERS
  feature (`ClientRequest::MoveDivider` / `SetProjectDivider` gone from nebula-core, ~700 lines in flux).
  Verified by `git worktree add --detach <scratchpad>/wt HEAD`, re-applying only these hunks there, and
  running `cargo test -p nebula-tui` in that worktree. Same recipe works for any change while
  [Shared tree races] is in effect; remove the worktree afterwards (`git worktree remove --force`).
- `Tree::has_visible_projects()` is deliberately still workspace-scoped and still drives the panel hints,
  `Action::New`'s add-project shortcut, and the splash's own "create your first project" line — only the
  splash gate got the default-workspace condition. Don't fold the check into `has_visible_projects`.
