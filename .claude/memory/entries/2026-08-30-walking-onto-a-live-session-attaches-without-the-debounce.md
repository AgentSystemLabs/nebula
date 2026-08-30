# Walking Onto A Live SESSION ATTACHes Without The Debounce — 2026-08-30

**Asked:** "when I click session rows it feels instant but using my keys to go up and down seems to shows
a loading session view first then show the terminal. why? fix this"
→ refined: "Walking the SESSIONS PANEL with ↑/↓ flashes the TERMINAL PANE's `starting session…`
placeholder before the terminal appears, while clicking the same row is instant — a keyboard move arms
the debounced ATTACH (`ATTACH_DEBOUNCE`, 180 ms) but a click goes through `preview_selected_now`. Make
↑/↓ attach immediately when the daemon already holds a live PTY for that SESSION (`Agent::alive` /
`TerminalTab::alive`), keeping the 180 ms debounce only for SESSIONS with no live PTY so sweeping the
list still can't cold-boot a fleet of agent CLIs. Leave click, Enter and the placeholder itself as they
are." (no questions asked)

**Did:** The flash is `draw_terminal`'s `!term.painted` branch (`ui.rs:3536`, "starting session… /
booting — the screen appears as soon as it paints"): `attach_inner` swaps the pane to a fresh unpainted
`AttachedTerm` at once but holds the `Attach` for `ATTACH_DEBOUNCE`, so nothing paints for 180 ms +
round trip. Clicks don't wait (`preview_selected_now`). Added `session_has_live_pty` next to
`preview_inner` (`event_loop.rs:~5210`) reading `Agent::alive` / `TerminalTab::alive` off the tree, and
`preview_inner` now collapses its delay to `Duration::ZERO` for a live SESSION. Rejected zeroing the
delay inside `attach_inner`: that path also serves the WORKSPACES BAR / WORKTREES PANEL walks, whose
"nothing attaches while the cursor is still moving" contract is deliberate. Updated
`session_arrows_preview_without_focusing` to the new contract and added
`walking_onto_a_reaped_session_still_waits_out_the_debounce`. nebula-tui 562 passed, `cargo fmt --check`
clean.

**Gotchas:**
- **The debounce was never about IPC cost — only about forking CLIs.** The 2026-08-26 entry's whole
  premise is `Attach` on a *reaped* sref cold-spawning `zsh -l -i -c 'exec claude --resume …'`. A live
  PTY just replays its SCROLLBACK RING, so gating the wait on `alive` restores the original intent
  rather than relaxing it.
- **`Agent::alive` / `TerminalTab::alive` are already client-side** ("true when the daemon currently
  holds a live PTY"), so no protocol change was needed — `event_loop.rs:~7226` already had the same
  lookup inline for `detach_if_attached`.
- **A reaped SESSION still flashes the placeholder, by design** — and now flashes it identically whether
  you click it or walk onto it, which is the honest state (the CLI really is booting, ~1.5 s to paint).
- **The WORKTREES PANEL and WORKSPACES BAR walks still debounce even for live SESSIONS**
  (`select_worktree_row` / `switch_workspace` → `restore_session` → `attach`). Same flash, untouched
  scope — `walking_the_workspaces_column_attaches_only_where_it_stops` seeds `alive: true` agents and
  would have failed had the gate gone into `attach_inner`.
