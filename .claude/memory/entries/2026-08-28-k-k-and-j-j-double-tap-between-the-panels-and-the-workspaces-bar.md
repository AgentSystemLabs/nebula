# `k`,`k` On A Panel's First Row Jumps Into The WORKSPACES BAR, `j`,`j` There Drops Back Where It Came From — 2026-08-28

**Asked:** "when a user is focused on a panel and they press k to toggle up, when they are at the first,
they should be able to double tap k (see how we do it on h and l), to jump up to the workspaces, and
double tab j when on workspaces panel to jump back down to the last session you were at"
→ refined: With the WORKSPACES BAR shown, a MOVE UP (`k`/`↑`) on the first row of the PROJECTS,
WORKTREES or SESSIONS PANEL is a no-op today; make it a DOUBLE TAP like `h`/`l` at a WALK EDGE: the first
press stays put and flashes "`k` again: workspaces", a second press within `DOUBLE_TAP` moves FOCUS up
into the WORKSPACES BAR. Symmetrically, on the WORKSPACES BAR a DOUBLE TAP of MOVE DOWN (`j`/`↓`) jumps
FOCUS back to the PANEL it left (assuming: whichever panel FOCUS was on when it went up — by `k`,`k`,
`h`,`h` or Shift+Tab — with its cursor untouched, Projects if none; and assuming the single `j` there now
stays put and flashes, no longer dropping into Projects, matching the `h`/`l` convention). Keep MOVE UP /
MOVE DOWN inside the panels, the WORKSPACES BAR `←`/`→` tab switching, and the existing `h`/`l` DOUBLE TAP
exactly as they are.

**Did:** Two steps, per the new "Keep modules small" rule. (1) Behavior-preserving extraction: the PANEL
WALK / DOUBLE TAP functions (`next_focus`, `enter_terminal_pane`, `DOUBLE_TAP`, `double_tapped`,
`walk_focus_forward`, `walk_focus_back`) moved out of `event_loop.rs` into the child module
`crates/nebula-tui/src/event_loop/focus_walk.rs` (`mod focus_walk;` + `use focus_walk::{…}` at the top
of `event_loop.rs`); the 10 walk/double-tap unit tests ran green before and after. (2) The feature:
`App::bar_return: Focus` (`app.rs`, next to `edge_tap`, default `Projects`) remembers the panel FOCUS
came up from; `focus_walk.rs` gained `enter_workspaces_bar` (records it — used by `walk_focus_back`,
`h`,`h`, `k`,`k` and both mouse clicks on a WORKSPACE TAB; from the TERMINAL PANE it records Sessions),
`leave_workspaces_bar`, `at_top_row` (`sel_project` / `sel_worktree` / `sel_session == 0`) and
`panel_name`. In `handle_key`: `Action::MoveUp if app.show_workspaces && at_top_row(app)` goes through
`double_tapped(.., "workspaces")`; `Action::MoveDown` in the bar goes through `double_tapped(.., "back to
<panel>")` then `leave_workspaces_bar`. A single `j` in the bar no longer drops into Projects (Enter still
does). Hints: `move_up` / `move_down` in `keymap.rs`, HELP OVERLAY row "move selection (2×: bar)", README
row 274. Tests: new `k_k_at_the_top_row_steps_into_the_bar_and_j_j_drops_back_where_it_came_from`;
`focus_walk_includes_the_workspaces_bar_only_when_shown` now expects `↓`,`↓` out of the bar;
`a_slow_or_interrupted_second_tap_at_the_edge_stays_put` uses `j` as its "any other key" (a `k` on row 0
is now itself an edge tap). Gate: nebula-tui 515 unit + 6 e2e_tui green, clippy clean.

**Gotchas:**
- `app.flash` is cleared once per terminal event in `handle_terminal_event`, not in `handle_key`, so the
  test `press()` helper leaves the previous hint standing: a `flash.is_none()` assertion after a second
  press needs an explicit `app.flash = None` first (the h/l tests already did this by hand).
- A child module of `event_loop.rs` (`event_loop/focus_walk.rs`) is reachable from `event_loop::tests`
  through `use super::*` only for the names the parent re-imports; a test-only item (`DOUBLE_TAP`) is
  spelled `focus_walk::DOUBLE_TAP` in the test so the non-test build has no unused import.
- `cargo fmt` rewraps a long `use focus_walk::{…}` line into a block — a script that patches by exact
  text after fmt has to match the wrapped form.
- Any test that uses `k` as a "neutral" key in a panel now arms the top-edge DOUBLE TAP when the cursor
  is on row 0 (an empty panel counts as row 0); pick `j` or a genuinely unbound key instead.
