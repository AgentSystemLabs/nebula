# The WORKSPACES COLUMN Remembers Itself, And `/` Crosses Workspaces — 2026-08-25

**Asked:** "remember if someone had the workspaces panel collapsed so you don't show it the next time,
also allow to configure showing it or not in the settings" — then, mid-task: "also allow the jump to to
include the entire workspaces path so I can quickly jump between workspaces whenever"

**Did:** Two things.

(1) **Visibility moved out of the UI STATE BLOB into CONFIG.JSON.** `UiState::show_workspaces` is deleted
(`crates/nebula-tui/src/app.rs`); the home is now `Config::show_workspaces` (default true), with a
`SettingKind::ShowWorkspaces` row at the bottom of the SETTINGS OVERLAY's **Appearance** tab. `Action::ToggleWorkspaces`
(`event_loop.rs:~1330`) saves the file as it flips, so the choice survives a kill, a crash, or a closed
`nebula browser` tab — `ui_state_json` is only sent on `app.should_quit`, which is exactly why the old
one didn't stick. New `apply_config(app, &cfg)` + `set_show_workspaces(app, shown)` are shared by startup
and `apply_setting_at`, so the settings row and the hotkey are the same code path.

(2) **The `/` PALETTE is no longer workspace-scoped.** `build_palette_items` (`app.rs:~824`) now walks
`palette_workspace_order(tree)` — active workspace first, then tree order — emitting a
`PaletteTarget::Workspace` row per workspace plus every project/worktree/session/PR under it, each
pathed `workspace/project/branch/session`. `jump_to_target` switches workspace first via the new
`target_workspace()`; a workspace row gets the full `switch_workspace`, everything else gets
`switch_workspace_quietly` (new `switch_workspace_inner(.., restore: bool, ..)`).

629 tests green, fmt clean, no new clippy warnings. README + the `palette` keymap hint updated.

**Gotchas:**
- **A hotkey that writes `Config::save()` makes the test suite edit the dev's real CONFIG.JSON.**
  `shift_w_toggles_the_workspaces_column_and_parks_focus` had no path override, so the run wrote
  `show_workspaces: false` into a live `config.json` — and because an agent working inside `make dev`
  has `NEBULA_DATA_DIR=~/.nebula-dev` exported, it lands in the DEV INSTANCE's config, not the one you'd
  think to check. It also correlated with `e2e_pty::workspace_scope_is_per_connection` failing 4 of 5
  full-suite runs (0 of 7 on a clean tree, and never when e2e_pty ran alone); the failure went away for
  good once the test was pinned. `Config::save()` now `assert!`s in `#[cfg(test)]` that
  `CONFIG_PATH_OVERRIDE` is set — wrap any test that presses such a key in `with_default_config`.
- **A cross-workspace jump attaches twice if you reuse `switch_workspace`.** It calls
  `restore_context` → `restore_session` → `attach`, so the destination's *remembered* session gets
  attached, then detached one request later when the jump lands on the row actually picked:
  `[Detach a1, Attach a8, OpenWorkspace, Detach a8, Attach a9]`. Hence `switch_workspace_quietly`.
  The test for this only discriminates if the remembered session **differs** from the jump target —
  with one agent in the destination workspace, `attach`'s already-attached early return hides the bug
  and the test passes either way.
- **A quiet switch means the branches' early-outs are wrong.** The cursor can already sit on the target
  row in the new workspace while the pane still shows the workspace you left, so `PaletteTarget::Project`
  needs `switched || changed` and `PaletteTarget::Worktree` needs `!switched && …` on its early return.
- **`build_palette_items` must not require `tree.workspaces` to be complete.** Grouping strictly by the
  workspace list emptied the palette for every `seed_tree`-only test (projects carry
  `workspace_id: Default::default()`, and nothing upserts the matching `Workspace`). A project whose
  workspace is unknown still gets its rows, just with no path prefix — vanishing from the
  find-anything tool is the worst failure it has.
- `TestBackend` renders the palette rows, so a failing palette assertion prints the whole modal — that
  dump is the fastest way to eyeball glyphs and paths (`◇ client` / `▫ client/secret`).
