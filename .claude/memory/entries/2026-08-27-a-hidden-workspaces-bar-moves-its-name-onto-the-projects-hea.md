# A Hidden Workspaces Bar Moves Its Name Onto The Projects Header — 2026-08-27

**Asked:** "when a user has the workspaces top bar hidden, display the selected workspace name in place
of where it says Projects inside the projects list"

**Did:** `draw_projects` (`crates/nebula-tui/src/ui.rs:2475`) now computes its title instead of passing
the literal: `"PROJECTS"` while `app.show_workspaces`, otherwise
`app.tree.active_workspace_name().to_uppercase()`, truncated to `area.width - (ROW_GUTTER + 1 + " · n")`
so a long workspace name can't run into the column rule. The ` · n` project count stays either way, and
`draw_column` is untouched — WORKTREES/SESSIONS still pass literals. New test
`a_hidden_bar_moves_the_workspace_name_onto_the_projects_header` (event_loop.rs) renders hidden, switches
to `ws2`, and shows the bar again. 685 workspace tests green, no new clippy warnings.

**Gotchas:**
- Upper-cased on purpose — every other column header is all-caps, and a natural-case name in that slot
  breaks the row. Verified by eye against a rendered `TestBackend` buffer, not just an assertion.
- `Tree::active_workspace_name` falls back to `"default"` when no `Workspace` entity has arrived, so a
  bar-hidden render reads `DEFAULT · 1` even with `tree.workspaces` empty. Several existing tests set
  `app.show_workspaces = false` purely to size the panels (`embedded_terminal_renders_pty_output`,
  `link_rows_render_under_a_links_header`, `archived_list_scrolls_by_wheel_and_follows_the_cursor`) and
  now draw that header — harmless, but a future `text.contains("PROJECTS")` in one of them would fail.
- One existing assertion had to move: `shift_w_toggles_the_workspaces_bar_and_parks_focus` checked
  `lines[1].starts_with("   PROJECTS")` on the hidden path; it's `"   DEFAULT"` now.
