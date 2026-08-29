# The PANEL WALK Stops At The Pane Going Forward And Locks It — 2026-08-27

**Asked:** "when using control shift h or l it should auto focus on claude code session when focused.
also we shouldn't loop the nav, if the hit terminal panel then control shift l stops" — then, after the
first cut: "I think you misunderstood me, I liked the control shift h and control shift l, but now it
doesn't work, I just wanted so if a user presses control shift l and gets to the terminal it should auto
focus it and stop allowing the user to cycle next to the workspaces top nav"

**Did:** FOCUS NEXT / FOCUS PREV (`Action::FocusNext` / `Action::FocusPrev`) in `crates/nebula-tui/src/event_loop.rs:1275`.
Forward (Tab / `^⇧L`) from `Focus::Terminal` now stays put instead of wrapping to `first_focus()`, and
landing on the TERMINAL PANE goes through new `enter_terminal_pane(app)` (`event_loop.rs:4695`): `focus =
Terminal` plus `term_locked = true` (LOCKED PANE) when `app.term` is live (an empty/exited pane is focused, never
locked). `Action::Activate`'s `Focus::Terminal` arm reuses it. Back (⇧Tab / `^⇧H`) **still wraps** from
the first panel — the bar when shown, Projects when hidden — into the pane, and that arrival locks too;
`^⇧H` is also the ESCAPE HATCH back out, so `^⇧H` alone cycles Projects → pane → Sessions → … Docs: KEYMAP
hints, HELP OVERLAY row, the locked-input comment, two README spots. Tests
`ctrl_shift_hl_walk_forward_stops_at_the_pane_and_back_wraps_into_it`,
`focus_walk_includes_the_workspaces_bar_only_when_shown`, and the E2E TUI walk section. 686 green.

**Gotchas:**
- **The first cut also removed the backward wrap, and the user read that as "^⇧H doesn't work".** The
  ask named one direction only. `^⇧H` from the top panel is the one-key jump into the agent, and
  taking it away broke a habit. Scope the no-wrap to the direction that was asked for.
- **`Ctrl+→` (CROSS WITHOUT LOCKING) is now the only unlocked way into the pane.** `FocusTerminal`'s documented purpose is
  "cross without locking", so it was left alone — but the old comment claiming "Tab / Ctrl+arrows do not
  lock, Enter does" was load-bearing prose in the ESCAPE HATCH block and had to be corrected.
- **`^⇧L` pressed again on a locked pane is forwarded to the agent, not swallowed** — the locked path
  only intercepts `UnlockTerminal` chords and the HARDWIRED UNLOCK `^q`. KITTY PROTOCOL terminals send a harmless
  `CSI 108;6u`; legacy degrades it to `Ctrl+L` (0x0C), pre-existing and unavoidable.
- **Proving a key does what the user sees, not what the unit test sees:** a throwaway E2E TUI test that
  sent the raw kitty bytes (`\x1b[108;6u` / `\x1b[104;6u`) to the real binary settled "is it the code
  or the build?" in one run. `strings target/debug/nebula | grep <new hint text>` settles which build
  is running. Both were fine — the disagreement was about the spec.
- `^⇧H`/`^⇧L` only reach nebula on a terminal that encodes the chord — it did when this was written
  (Ghostty). **Stale as of 2026-08-28: `TERM_PROGRAM=WezTerm`**, Ghostty 1.3.1 still installed. Check
  `TERM_PROGRAM` rather than trusting either note; the older Terminal.app note is stale too.
- In `crates/nebula/tests/e2e_tui.rs`, "the walk stops here" is untestable with `wait_for_text` alone —
  it passes trivially if the footer is already up — so the stop is proved by pressing the extra key and
  then walking one step the other way. ⇧Tab is `\x1b[Z`, Ctrl+→ is `\x1b[1;5C`.
- `e2e_pty::external_worktrees_are_adopted_and_dropped` failed once mid-run and passed alone on rerun —
  the usual e2e flake, unrelated to a TUI keymap change.
