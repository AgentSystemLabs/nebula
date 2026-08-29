# PROJECT DIVIDERS Removed From The Projects Column — 2026-08-25

**Asked:** "remove the ability for a user to divide the projects column"

**Did:** Deleted the PROJECT DIVIDERS feature end to end (~1.4k lines). `Project` lost its four `divider_*` fields
(`crates/nebula-core/src/entities.rs`); `ClientRequest::SetProjectDivider` / `MoveDivider` are gone from
`protocol.rs` and `server.rs`; `Daemon::set_project_divider` / `move_divider` and the leading-divider
hand-down in `remove_project` are gone from `registry.rs`, and `move_project` (MOVE PROJECT) is now a plain remove/insert
that renumbers `sort_order` to the display index. Store: `insert_project` / `set_project_position` /
`get_project` / `load_tree` no longer touch the columns, and **MIGRATION 18** is four
`ALTER TABLE projects DROP COLUMN`s (`migration_18_drops_the_divider_columns` seeds a v17 DB with a
labeled divider and checks `PRAGMA table_info`). TUI: the `ProjectRow` enum is gone — `App::project_rows()`
is now `Vec<usize>` (indices into the full `tree.projects`, workspace-filtered) and
`selected_project_row()` became `selected_project_index()`; `divider_focused()`, `select_divider_when_seen`,
`PromptKind::DividerLabel`, `MenuAction::{SetProjectDivider,LabelDivider}`, `Action::ToggleDivider` (`-` is
now unbound), `ui::divider_spans`, the "you're focused on a separator" pane, and
`SelectionSnapshot::{project_kind,divider_chase}` are all removed. README lost its three divider rows.
Workspace: 618 tests green, clippy/fmt clean.

**Gotchas:**
- A user KEYMAP config that still names `toggle_divider` is harmless: `Keymap` logs
  `ignoring keybinding for unknown action` and moves on (`keymap.rs:~849`).
- Migrations 2, 3, 7 and the migration-14 table rebuild still spell out the divider columns — they must,
  since they already ran on every existing DB. Only migration 18 drops them; don't "tidy" the old SQL.
- Two older entries (PR rows in the WORKTREES PANEL, the PR PREVIEW pane) described their behavior as a copy of
  "the divider precedent" — those early returns now stand on their own and the entries were updated.
