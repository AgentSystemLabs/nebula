# PR #18 — Workspace-wide dedup: named literals, shared helpers, nebula_core::env

- **URL:** https://github.com/AgentSystemLabs/nebula/pull/18
- **Author:** @webdevcody
- **Merged:** 2026-08-28T14:02:04Z by @webdevcody (`32b0f515b9d1`)
- **Opened:** 2026-08-28T03:15:59Z
- **Branch:** `clean-up` → `main`
- **Diff:** +3169 −2809 across 37 file(s)

## Description

> ## Summary
>
> A behavior-preserving cleanup pass over all four crates: magic numbers named, copy-pasted blocks folded into helpers, and a handful of idiom fixes (`&String`/`&Option<String>` params, `bool` args → enums, `match`-on-`Option` → `map_or`, `.unwrap()` in row mappers → `?`). Every refactor was gated on a test: where one already covered the code it was named and run first; where none did, a pinning test was written against the *old* code and run green before the change. Net: **+20 tests** (710 across the workspace), **0 clippy warnings** workspace-wide (was 7), `cargo fmt --check` clean.
>
> One commit per area so each is reviewable on its own:
>
> | Commit | What |
> |---|---|
> | `nebula-core` | New `nebula_core::env`: every `NEBULA_*` variable name as a const, plus `non_empty()` and `home_dir()`, used by the daemon, TUI, CLI, hook installer and the e2e tests instead of string literals (a typo now fails to build instead of silently falling back to a default). `paths.rs` uses it and one `project_dirs()`. `codec.rs` has one `check_len()`/`invalid_data()` instead of two frame-too-large blocks. `crashlog.rs` gets `SECS_PER_*` and a testable `format_timestamp`. |
> | `nebula` CLI | `KILL_HINT` shared by `upgrade.rs` and `main.rs` (the two copies had drifted: "all" vs "ALL"); one `init_file_logging()` for daemon and TUI; `log_fatal(&Path)`, `spawn_ttyd(&OsStr)`. |
> | e2e tests | `make_executable()` replaces 12 chmod blocks, `subscribe()` 8 subscribe-and-wait blocks, `agent_cli()` 4 CLI spawns, `make_repo` delegates to `make_repo_at`; 120 bare `Duration` literals and the escape-byte sends become named consts. |
> | `nebula-tui` (non-event-loop) | `modal_block`/`render_modal_frame` (6 byte-identical sites), `empty_list_row`, `below_first_row`, `visible_positions`, `hint_line`; popup sizes named. `window_start`/`clamp_selection`/`max_scroll`/`clamp_files_width` shared by 6/5/2/2 impls. One `truncate` (was also `grep_search::clip`), one `cap_lines`. `ipc.rs`: `await_ack`, `current_agent_id`, `RenameMode`. `pull_request.rs`: one `gh()` wrapper and `str_at`/`arr_at`/`web_url` extractors; `STATE_OPEN`. `keymap.rs`: one `KEY_NAMES` table backs `parse_key_name`/`key_name`/`key_display` (pinned first by a test asserting every current spelling and glyph); `CTRL_COLLISIONS`; `UNBOUND`. `config.rs`: `DEFAULT_CHOICE`. |
> | `nebula-daemon` | `DEFAULT_COLS/ROWS` (9 × `80, 24`), `broadcast_agent`/`try_broadcast_agent` (15 sites), `kill_sessions_in`, `LOGIN_SHELL_ARGS`, `CLI_PROBE_TIMEOUT`; `AGENT_SESSION_VARS` replaces the per-spawn `scrubbed_env_names()` allocation. `store.rs`: one `*_COLUMNS` const + `row_to_*` mapper per entity shared by point lookups and `load_tree` (the point lookups' ~41 `.unwrap()`s now propagate via `?`), `delete_by_id`, `DEFAULT_WORKSPACE_ID` bound instead of an inlined `'default'`. `status.rs`: `end_turn` shared by `Stop` and `Progress{busy:false}`. `hooks/installer.rs`: `install_claude_hooks` reuses `install_managed_hooks`, `root_object_mut`/`object_mut`/`array_mut` replace 9 bail blocks (messages verbatim), `purge_nebula_groups`, `HOOKS_FILE`/dir consts, shared curl prelude (pinned verbatim by a new test). `hooks/mod.rs`: `HookDialect` enum for the `bool`, `HOOK_OK`/`HOOK_NOT_OK`, handlers moved above the test module. `lib.rs`: `env_period_ms` (tested) + `env_interval`. `server.rs`: `reply_done` for 21 arms. `pty`: `pty_size`, `ESC`/`BEL`. `git.rs`: `parse_worktree_list` is a pure, tested function. |
> | `event_loop.rs` | `contains()` for 10 open-coded rect tests; `selected_checkout` + `load_worktree_files` (4 modal preambles); `spawn_editor_modal` (4); `settings`/`settings_mut` (17); `send`/`send_with` (~24 `alloc_req_id` + push pairs); `upsert_by`; `worktree_in_context`; `select_project_row`/`select_worktree_row` shared by key and mouse; `is_double_click`; `leave_terminal_lock`; `bracketed()` + `PASTE_START/END`; `edit_keymap`; `confirm_delete_agent`/`confirm_close_terminal`/`confirm_remove_project` shared by key and menu (pinned by a new test); `MenuItem::new`/`destructive` for 46 literals; `Landing` enum for `jump_to_target`'s `bool`; `next_focus` behind the three focus-walk tables; `home_dir`; `step_selection`; `clamp_index`; wheel-step, restore-clamp, anchor and fallback-pane consts. |
>
> ### Deliberately not done (medium risk, untested draw paths)
> - Folding the six list-modal mouse arms / four overlay key handlers in `event_loop.rs` into one — arm order is load-bearing for Tree.
> - `jump_to_target` vs `open_session` (~45 near-identical lines) — the non-attach path has extra `preview_selected` bookkeeping.
> - The two-pane modal scaffold in `ui.rs` (Diff vs Tree, ~120 lines) — no test covers the Tree arm.
> - A `str_enum!` macro for `AgentStatus`/`AgentKind` — a typo there is a silent persistence break.
> - `Config::write_into`/`value_label`/`cycle` table-driving — serde field-name drift hazard.
>
> A `/code-review` pass over the branch (no high-severity findings) fed a final commit: the agents' overlapping helpers were reconciled (`step_selection`/`clamp_index` → `app::clamp_selection`, `home_dir` → `nebula_core::env::home_dir`, `pr_preview::fit` → `truncate`, `grep_search` → `git_diff::run_git`, hit-testing → ratatui's `Rect::contains`), `PROJECT_COLUMNS` no longer hides a positional `?1` bind (the default workspace is applied in `row_to_project`), `remove_project` loads the tree once again, `RenameMode` is built where `--force` is parsed, and the `paths.rs` env test restores its vars on drop.
>
> ### Small semantic deltas worth knowing
> - `store.rs` point lookups return `Err` on a corrupt row instead of panicking the daemon thread.
> - `nebula stale-daemon-note` now prints "(stops all sessions)" like the two `upgrade` sites did, not "ALL" — the three copies had drifted and `KILL_HINT` picked the majority.
> - The metrics modal's `k`/wheel-up now clamps to the last row (it was an unclamped `saturating_sub`); identical while the cursor is in range.
> - `~/` expansion everywhere goes through `home_dir()` (`var_os`), so a non-UTF-8 `$HOME` now expands where `ipc::add_project` used to fall through.
>
> ## Test plan
> - [x] `cargo fmt --all --check`
> - [x] `cargo clippy --workspace --all-targets` — 0 warnings
> - [x] `cargo test --workspace --no-fail-fast` — 709 passed, 0 failed (incl. `e2e_pty` 25/25, `e2e_tui` 5/5)
> - [x] Every new helper's pinning test was run green against the pre-refactor code first
>
> 🤖 Generated with [Claude Code](https://claude.com/claude-code)

## Changed files (37)

- `.claude/MEMORY.md` +73 −0
- `crates/nebula-core/Cargo.toml` +3 −0
- `crates/nebula-core/src/codec.rs` +57 −15
- `crates/nebula-core/src/crashlog.rs` +27 −5
- `crates/nebula-core/src/env.rs` +65 −0
- `crates/nebula-core/src/host.rs` +6 −4
- `crates/nebula-core/src/lib.rs` +1 −0
- `crates/nebula-core/src/paths.rs` +84 −21
- `crates/nebula-daemon/src/git.rs` +76 −27
- `crates/nebula-daemon/src/hooks/installer.rs` +169 −102
- `crates/nebula-daemon/src/hooks/mod.rs` +111 −94
- `crates/nebula-daemon/src/lib.rs` +51 −13
- `crates/nebula-daemon/src/pty/kitty.rs` +4 −2
- `crates/nebula-daemon/src/pty/mod.rs` +31 −30
- `crates/nebula-daemon/src/pty/progress.rs` +7 −5
- `crates/nebula-daemon/src/registry.rs` +123 −127
- `crates/nebula-daemon/src/server.rs` +27 −77
- `crates/nebula-daemon/src/status.rs` +20 −23
- `crates/nebula-daemon/src/store.rs` +164 −167
- `crates/nebula-tui/src/app.rs` +124 −21
- `crates/nebula-tui/src/config.rs` +25 −16
- `crates/nebula-tui/src/event_loop.rs` +892 −931
- `crates/nebula-tui/src/git_diff.rs` +36 −8
- `crates/nebula-tui/src/grep_search.rs` +4 −18
- `crates/nebula-tui/src/ipc.rs` +114 −83
- `crates/nebula-tui/src/keymap.rs` +146 −96
- `crates/nebula-tui/src/lib.rs` +5 −4
- `crates/nebula-tui/src/pr_preview.rs` +11 −11
- `crates/nebula-tui/src/pull_request.rs` +139 −145
- `crates/nebula-tui/src/tree_browser.rs` +11 −20
- `crates/nebula-tui/src/ui.rs` +191 −194
- `crates/nebula/src/browser.rs` +2 −2
- `crates/nebula/src/main.rs` +44 −36
- `crates/nebula/src/upgrade.rs` +10 −10
- `crates/nebula/tests/e2e_pty.rs` +238 −430
- `crates/nebula/tests/e2e_tui.rs` +77 −71
- `crates/nebula/tests/tunnel_cli.rs` +1 −1

## Commits (10)

- `1ed166b52412` nebula-core: name env vars, dedupe codec and path helpers — @webdevcody, @claude
- `515e994599d3` nebula CLI: share the kill hint and logging init, dedupe e2e helpers — @webdevcody, @claude
- `78b97582fe94` e2e tests: name the event timeouts and raw key bytes — @webdevcody, @claude
- `f14724127b32` Dedup nebula-tui: modal frames, list math, gh wrapper, key-name table — @webdevcody, @claude
- `a473f4f2d097` nebula-daemon: dedupe row mappers, broadcasts, hook helpers, and spaw… — @webdevcody, @claude
- `e80f4de00a42` Dedupe event_loop.rs: shared helpers, named consts, idiom cleanups — @webdevcody, @claude
- `5ff1721da9f5` Log the workspace-wide dedup pass — @webdevcody, @claude
- `485293892498` Address review: reconcile the agents' overlapping helpers, finish the… — @webdevcody, @claude
- `828def4a8c54` Log the review-pass lessons from the dedup PR — @webdevcody, @claude
- `098d34ad6651` Merge remote-tracking branch 'origin/main' into clean-up — @webdevcody

## Conversation (0)

_(no issue comments)_

## Reviews (0)

_(no review submissions)_

## Inline review comments (0)

_(no inline comments)_
