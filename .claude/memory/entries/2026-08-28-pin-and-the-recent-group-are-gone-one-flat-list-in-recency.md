# PIN And The RECENT Group Are Gone: SESSIONS And WORKTREES Are One Flat List In RECENCY ORDER — 2026-08-28

**Asked:** "remove the ability to pin and unpin sessions.  i've decided since we already have recent at the
top and time stamps, no need to pin" → mid-turn: "also remove the RECENT label as we won't need it after"
→ refined: Remove PIN end to end — `Action::Pin` and the `p` key, the CONTEXT MENU's pin/unpin MENU ACTION,
the `ClientRequest` pin variants, the daemon's set-pinned paths, `pinned` on `Agent` / `Worktree` in
`entities.rs` (the SQLite `pinned` columns stay, like `sort_order` did), the help-overlay and README rows —
for SESSIONS and (assuming, since the same RECENCY ORDER now sorts them) WORKTREES. The IDLE REAPER loses
its pinned exemption; its RUNNING / NEEDS FEEDBACK exemptions and off switch stay. Drop the PINNED / RECENT
/ UNPINNED SESSION GROUPS and the RECENT WINDOW setting (`RECENT_WINDOWS`, its SETTINGS OVERLAY row): agent
SESSIONS become one header-less list in RECENCY ORDER, followed by TERMINALS / OPEN PRS / ARCHIVED exactly
as now; the WORKTREES PANEL likewise loses PINNED / UNPINNED and keeps the PROJECT OPEN PRS GROUP. Bump the
PROTOCOL VERSION. (No questions asked.)

**Did:** PIN is gone: `Action::Pin` + its `ActionSpec` (`keymap.rs`), the `Action::Pin` handler, the two
CONTEXT MENU items and `MenuAction::SetAgentPinned` / `SetWorktreePinned` (`event_loop.rs`, `app.rs`),
`ClientRequest::SetAgentPinned` / `SetWorktreePinned` (`protocol.rs`, PROTOCOL VERSION 33 → 34), the
`server.rs` arms, `Daemon::set_agent_pinned` / `set_worktree_pinned` and the IDLE REAPER's `agent.pinned`
spare (`registry.rs::reap_idle_sessions`), `Store::set_agent_pinned` / `set_worktree_pinned` and `pinned`
in the INSERTs and `WORKTREE_COLUMNS` / `AGENT_COLUMNS` (`store.rs`; the columns stay in SQLite, unread),
`Agent.pinned` / `Worktree.pinned` (`entities.rs`), the HELP OVERLAY rows, the Worktrees FOOTER `p: pin`
hint, the README key rows. The RECENT group is gone with it: `Config.recent_window`, `RECENT_WINDOWS`,
`DEFAULT_RECENT_WINDOW_MS`, `recent_window_ms()`, `parse_window_ms`, `SettingKind::RecentWindow` and its
Sessions-tab row (`config.rs`); `App.recent_window_ms`, `App.drawn_session`, `is_recent`,
`next_recent_expiry`, `worktree_group_counts` (`app.rs`); the RECENT-expiry `select!` arm (`event_loop.rs`).
`visible_sessions()` is one recency-sorted list of live agents (+ ARCHIVED when shown);
`session_group_counts()` returns `(live, archived)`; `visible_worktrees()` one recency-sorted list.
`ui.rs::draw_sessions` pushes the agents with no header — TERMINALS / OPEN PRS / ARCHIVED headers remain —
and `draw_worktrees` always keeps the quiet row under the ROOT WORKTREE. E2E PTY's reaper test lost its
pinned "keeper" agent. E2E TUI: `tui_projects_worktrees_agents_navigation` had a wrong step (see gotchas)
and `row_is_selected` now checks only the needle's own `│`-delimited panel band. Gate: nebula-tui 519,
nebula-daemon 162, nebula-core 12, e2e_pty 26, e2e_tui 7, clippy + fmt clean.

**Gotchas:**
- **An E2E TUI test that passes on HEAD and fails after a layout shift can be a harness false positive
  hiding a wrong expectation.** `row_is_selected` accepted a sel_bg cell *anywhere on the screen line*;
  `j → feat-b` in `tui_projects_worktrees_agents_navigation` passed only because the selected `term-1`
  pill shared feat-b's line — the RECENT header had pushed the SESSIONS PANEL down two rows. Without the
  header the real order showed: RECENCY ORDER puts the stamped `feat-a` first (`[feat-a, main, feat-b]`),
  so `j` lands on `main`. Fixed the step and the harness. To tell the two apart: `git archive HEAD | tar
  -x` into the scratchpad and dump the screen before/after the key there (a stash would race the SHARED
  CHECKOUT's other sessions).
- **Panel rows share screen lines once the agents have no header.** `session_rows_show_time_since_last_interaction`
  did `row.find("23m ago")` on a whole `buffer_text` line and hit the WORKTREES PANEL's own `23m ago`;
  `row_with` now slices the line from the session name onward.
- **`row_to_agent` / `row_to_worktree` read by positional index**, so dropping `pinned` from
  `AGENT_COLUMNS` / `WORKTREE_COLUMNS` meant renumbering every later `r.get(n)` — the compiler cannot
  catch an off-by-one there; the daemon suite did. The `pinned` columns and migrations 5 / 6 stay (a
  table rebuild for two dead ints is not worth it); the migration test still inserts into `pinned`.
- The 2026-08-20 "pinned sessions are never reaped" constraint is superseded by this decision: only
  RUNNING / NEEDS FEEDBACK agents, busy terminals and the `session_idle_timeout = off` switch spare a
  session now. A stale `"keybindings": {"pin": …}` in CONFIG.JSON is harmless (`Keymap::from_overrides`
  ignores unknown ids, as with the notes removal).
- The mid-turn "also remove the RECENT label" arrived while the pin grep was running — a scope add before
  the first reply, folded into the refined prompt rather than logged as a correction.
