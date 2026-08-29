# Workspace-Wide Dedup Pass: Named Literals, Shared Helpers, `nebula_core::env` — 2026-08-27

**Asked:** "go through the code and find places you can dry up magic numbers, remove duplicate code,
improve code to match rust best practices. verify we have a test before the code you're about to refactor
or write one if not, then refactor, make pr when done"

**Follow-up (2026-08-28):** "fix the conflicts on https://github.com/AgentSystemLabs/nebula/pull/18"

**Did:** Six commits on `clean-up`, one per area, all behavior-preserving; 710 tests (was 690), clippy
warning-free workspace-wide (was 7), `fmt --check` clean. Four read-only survey agents produced ~100
candidates; the low-risk, tested ones were done and the medium-risk ones (list-modal mouse/key arm
folding, `jump_to_target` vs `open_session`, the DIFF VIEWER / TREE BROWSER two-pane scaffold in `ui.rs`, a `str_enum!`
for `AgentStatus`, table-driving `Config::write_into`) were left, listed in the PR. Every extraction was
gated on a test run green against the *old* code first. Highlights: new `nebula_core::env` (`AGENT_ID`,
`API_URL`, `API_TOKEN`, `RUNTIME_DIR`, `DATA_DIR`, `AGENT_SESSION_VARS`, `non_empty()`) used by
`paths.rs`, `registry.rs`, `ipc.rs`, `upgrade.rs`; `store.rs` `*_COLUMNS` + `row_to_*` shared by point
lookups and `load_tree` (the point lookups' ~41 `.unwrap()`s now propagate via `?`); `registry::broadcast_agent` (15 sites),
`kill_sessions_in`, `pty::DEFAULT_COLS/ROWS`; `status::end_turn`; installer `root_object_mut`/`object_mut`/
`array_mut` + `purge_nebula_groups`; `hooks::HookDialect` for the `bool`; `server::reply_done` (21 arms);
`ui::modal_block`/`render_modal_frame`, `app::window_start`/`clamp_selection`; `keymap::KEY_NAMES` behind
the three key-name fns; `ipc::await_ack`/`current_agent_id`/`RenameMode`; `pull_request::gh()`;
`event_loop`: `send`/`send_with`, `selected_checkout`, `spawn_editor_modal`, `settings_mut`, `contains`,
`is_double_click`, `MenuItem::new` (46 literals), `Landing` enum, `next_focus`. e2e: `make_executable`,
`subscribe`, `agent_cli`, named timeouts and key-byte consts.
Merged `origin/main` into PR #18 without rewriting its history, retaining the v0.14.0 PROJECT rename,
DOUBLE TAP, CONFIRM DIALOG hit-testing and paused-rebase worktree label changes. Verification after the
merge: 728 workspace tests, `cargo fmt --check`, and workspace clippy with warnings denied.

**Gotchas:**
- **The four marker conflicts were not the whole merge.** `event_loop.rs::jump_to_target` auto-merged
  the PR's `landing: Landing` signature with main's stale `attach` call, and the PR's shared
  `confirm_*` constructors auto-merged without main's new `ConfirmDialog::area`; `cargo check --workspace`
  caught both markerless failures. Audit the combined diff and compile before treating a marker-clean
  merge as resolved.
- `git.rs` needed both sides: keep the PR's pure `parse_worktree_list` (and its porcelain test), then let
  async `list_worktrees` replace only detached labels via main's `rebasing_branch`. The E2E PTY conflict
  likewise keeps `EVENT_TIMEOUT` while waiting for both the Ack and the project upsert; Ack and broadcast
  order is deliberately unspecified.
- **Agent worktrees (`isolation: "worktree"`) branch from `main`, not from the lead's branch.** All three
  builders reported `nebula_core::env` "does not exist" and found `ui.rs` ~100 lines shifted (a MEMORY
  MODAL had landed on main meanwhile). Commit the shared groundwork, then either rebase your branch onto
  `main` before spawning or tell agents to `git cherry-pick <sha>` (their sandbox blocks `git merge`).
  Cherry-picking their commits back in then applies cleanly; only `.claude/MEMORY.md` conflicted.
- Main had reworked `browser.rs` (configurable `bind: IpAddr`) under my `LOOPBACK` const — the rebase
  conflict was the tell that the refactor was obsolete; take `--ours` and re-apply only the `&OsStr` nit.
- `e2e_pty::workspace_scope_is_per_connection` failed 2 of 3 full-suite runs while three agents were
  compiling alongside and passed 6/6 alone: the documented Ack-beats-upsert load race, not the refactor.
- `key_name`/`key_display` map `KeyCode::BackTab` to `"tab"`/`"Tab"` while `parse_key_name("backtab")`
  yields `BackTab` — one table can't express that, so `key_row` folds BackTab onto Tab and parse
  special-cases the word. `named_keys_keep_their_spellings_and_glyphs` pins every string.
- `host_warning` returns `Option<&'static str>`, so `CTRL_COLLISIONS` holds full messages per entry.
- `tree_browser::read_preview` marks truncation on `lines dropped || byte_capped`; `cap_lines` takes the
  second as `already_cut` rather than the caller appending the mark twice.
- `step_selection` (clamp form) replaced the MEMORY MODAL's unclamped `saturating_sub(1)`: a cursor left
  past a shrunken `rows` now snaps to the last row. `home_dir()` is `var_os("HOME")` where
  `shellexpand_home` used `var` — a non-UTF-8 `$HOME` now expands. Both accepted, noted in the PR.
- A regex dedupe of the `agent_entity + broadcast` pair also rewrote the body of the new
  `broadcast_agent` into a self-call; rustc's `unconditional_recursion` caught it. Exclude the definition
  when pattern-rewriting.
- The two e2e test files already had `subscribe`-shaped closures (`workspace_scope_is_per_connection`)
  that shadowed the new free fn silently — grep for the name before adding a test helper.
- A worktree-isolated agent's Bash refuses `for` loops, heredocs and `&&` chains ("too complex to verify
  it stays inside the worktree") — write the script with the Write tool and run it as one command.
- **Parallel dedup agents dedupe past each other.** The event-loop agent wrote `step_selection`/
  `clamp_index`/`home_dir` while the tui agent wrote `app::clamp_selection` and app.rs already had
  `home()`; `pr_preview::fit` and `grep_search`'s git call survived as third copies of `truncate` and
  `git_diff::run_git`. A `/code-review main high` pass caught all of it — budget one after any fan-out.
- A `COALESCE(workspace_id, ?1)` inside a shared `*_COLUMNS` const made every query using it bind a
  hidden first parameter (`get_project` had to number its id `?2`); rusqlite only checks bind *counts*,
  so a future `WHERE workspace_id = ?1` would silently COALESCE NULL rows into the queried workspace.
  Select the bare column and apply `DEFAULT_WORKSPACE_ID` in `row_to_project` instead.
