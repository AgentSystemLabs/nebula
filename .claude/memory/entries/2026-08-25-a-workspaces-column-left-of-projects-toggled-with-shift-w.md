# A WORKSPACES COLUMN Left Of Projects, Toggled With Shift+W — 2026-08-25

**Asked:** "add the ability to show a "workspaces" column to the left of projects which acts similar as
projects, basically we should be able to see from a top level which workspaces are running something, add a
hotkey of capital W shift + w to toggle that entire panel away or not. also clicking on the workspace in the
bottom bar should show the workspace select modal"

**Did:** New `Focus::Workspaces` (first variant) + `App::show_workspaces` (default shown; it was persisted
in `UiState.show_workspaces: Option<bool>` at the time — that field is **gone**, see [The Workspaces
Column Remembers Itself] below, which moved it to the `show_workspaces` config key). `ui.rs::draw_workspaces` renders every
`tree.workspaces` row as a 3-row project-style button with `app::workspace_rollup` (all unarchived agents
under the workspace's projects, folded by `rollup`) plus a warn-colored WORKSPACE RUNNING COUNT
(`workspace_running`); the OPEN WORKSPACE is the selected row. The cursor IS the active workspace:
`move_selection` and left-click call `switch_workspace`, Enter steps into Projects, `n`/`r`/`d`/`m` map to
`PromptKind::NewWorkspace` / `RenameWorkspace` / `remove_workspace` / `workspace_menu` (three new
`MenuAction`s). `Action::ToggleWorkspaces` (`shift+w`, keymap.rs) flips the column and parks a cursor
in it on Projects. The column shipped fixed at 18 columns; `splitter_x` / `set_splitter` /
`normalize_panel_widths` all carry the offset via `App::workspaces_panel_w()`. (Superseded — it is
draggable now, see [The Workspaces Column Drags To Resize].) FOOTER: `draw_footer` is now a wrapper over
`draw_footer_bar(&App) -> Option<Rect>` that registers `HitTarget::FooterWorkspace` on the `◇ name`
span (the WORKSPACE NAMEPLATE); left-click opens the WORKSPACE SWITCHER (`open_workspace_picker`). 8 unit tests + e2e updated; README keymap rows added.

**Gotchas:**
- **Tab from the TERMINAL PANE now wraps to Workspaces, not Projects** (`App::leftmost_focus`). e2e
  `tui_projects_worktrees_agents_navigation` timed out on `FOOTER_PROJECTS` after the fourth Tab — the
  fix is a fifth stop (`FOOTER_WORKSPACES = "w: switcher"`) in the PANEL WALK. Any future e2e that Tab-wraps
  needs the same.
- **Every 100-col draw test compresses when the column is shown**: budget = 100 − 18 − 20, so Sessions
  drops to 20 and the terminal pane truncates its own text. Six existing tests (`seed_splitters` and the
  five `TestBackend::new(100, 30)` draw tests that assert positions or pane text) now set
  `app.show_workspaces = false`; test the column at 140 cols.
- `render_button` lifts a `dim` span to `muted` on the selected row, so asserting a fresh dot's color on
  the active workspace row must expect `theme.muted`, not `theme.dim`.
- `seed_tree` points its project at `WorkspaceId::default()` but never upserts the 'default' Workspace
  entity — the footer's "default" is `active_workspace_name`'s fallback. A column test needs
  `seed_default_workspace` or the list shows only `seed_other_workspace`'s row with nothing selected.
- The auto-mode classifier blocked `git checkout origin/main -- <files>` to un-stale the SHARED CHECKOUT's
  `event_loop.rs`/`keymap.rs`/`e2e_tui.rs` (they still carry the pre-#12 `h`/`l` hunks and the macOS-only
  clipboard), so this feature was built on the working tree as it stood. Expect those stale hunks to
  show up when rebasing onto `origin/main`; they are not part of this work.
