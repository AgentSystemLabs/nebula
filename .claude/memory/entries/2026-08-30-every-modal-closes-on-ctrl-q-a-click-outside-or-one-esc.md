# Every Modal Closes On Ctrl+Q, A CLICK OUTSIDE Or One Esc — 2026-08-30

**Asked:** "verify every single modal can be closed with a control q, click outside, or 1 esc press,
make reusable modal if necessary"
→ refined: Audit every OVERLAY variant (`app.rs::Overlay` — Menu, Confirm, Prompt, Help, Settings,
Diff, Palette, Files, Grep, Tree, Metrics, Hosts, AgentPresets, AgentPresetEditor) plus the VIM MODAL,
and make all three exits work on each: HARDWIRED UNLOCK (`Ctrl+Q`) always closes the modal outright
from any state, back to the panels; CLICK OUTSIDE dismisses it; and one Esc closes it from its base
state — keep the existing two-stage Esc where a filter/query is typed or a submenu is open. Factor the
duplicated dismissal into one shared path if that removes real duplication (KEEP MODULES SMALL). Add
tests covering all three exits per overlay. (asked: should "1 esc press" flatten the seven staged Esc
arms → keep the staging, Ctrl+Q is the one-shot)

**Did:** The audit found **Esc and CLICK OUTSIDE already worked on all 14 variants; Ctrl+Q worked on
none of them.** New `crates/nebula-tui/src/overlay_close.rs` holds every exit in one place:
`overlay_area` (exhaustive over all 14), `click_is_outside` (the `width > 0` guard), `click_outside`
(the per-variant dismissal dispatch) and `force_close` (Ctrl+Q). `event_loop.rs::handle_key` now tests
`HARDWIRED_UNLOCK` *before* `handle_overlay_key`; `handle_mouse` opens with one `Down(Left)`
outside pre-check that replaced the 5-arm `boxed` block plus eight per-arm `!inside_modal` hit-tests
(Palette, Files, Grep, Tree, Hosts, Metrics, Settings, and `preset_overlays.rs::handle_list_mouse`).
Extracted `abandoned_prompt_prewarm` so the warm-slot restore runs on whichever exit abandons a
PROMPT DIALOG. Behavior changes beyond Ctrl+Q: clicking outside a CONTEXT MENU submenu or the AGENT
PRESETS picker now hands the QUICK PROMPT its box back with the typed text instead of dropping it —
CLICK OUTSIDE's TERM contract is "exactly as Esc would". Test helper `drawn_modal_area` now goes
through `overlay_area`, so it works for all 14 instead of 4. Eight new tests, including a
table of all 14 openers whose `overlay_label` is an exhaustive match. Gate: `cargo fmt`,
`clippy --workspace --all-targets -D warnings` clean, workspace green (nebula-tui 570 passed, up
from 562).

**Gotchas:**
- **An OVERLAY swallowed literally every key.** `handle_key`'s `if app.overlay.is_some() { … return }`
  sits *above* the LOCKED PANE branch that owns `HARDWIRED_UNLOCK`, so Ctrl+Q never reached a modal.
  It appeared to work in the VIM MODAL only because `handle_vim_key` carries its own copy of the check
  — and the editor is the one thing drawn *above* `app.overlay`.
- **The VIM MODAL is the one deliberate exception and needs no fix.** Its Esc belongs to vim, its
  mouse is vim's ("its selection/scroll story is vim's, not ours" — the early `return` in
  `handle_mouse`), and killing an unsaved buffer on a stray click would be the bug. It already
  advertises ` Ctrl+Q: force close ` on its bottom border (`ui.rs::draw_vim`); no other modal
  advertises its Ctrl+Q, which is still open.
- **`Ctrl+Q` is no longer capturable in the HOTKEYS TAB.** The intercept runs before
  `handle_settings_key`, so a live HOTKEY CAPTURE never sees the chord and the SETTINGS OVERLAY closes
  under it. That contradicts `a_capture_takes_keys_the_overlay_would_normally_use`'s premise for
  exactly one chord — deliberate, and harmless because HARDWIRED UNLOCK overrode any binding anyway.
  Pinned by `ctrl_q_closes_settings_out_from_under_a_hotkey_capture`.
- **A generic "a click inside keeps the modal open" probe is wrong.** In the CONTEXT MENU, the HOSTS
  PICKER and the AGENT PRESETS list a row *is* an action, and `area.y + 1` is row 0 — the menu launched
  a session mid-test. The generic test asserts only the outside click; "inside stays" stays per-modal.
- **`App` is not `Clone`**, so comparing what Esc does against what Ctrl+Q does on the same staged
  state means building that state twice — hence the `peel_with(open, code)` helper.
- **All 14 variants already wrote their `area` back during draw** (13 arms in `ui.rs`, 2 in
  `preset_overlays.rs`), which is the only reason `overlay_area` can be a total function.
- **Anchoring a Python insert on the `fn` line drops the new function between the *previous*
  function's doc comment and its body.** `abandoned_prompt_prewarm` landed under
  `default_claude_prewarm`'s `///` block and silently stole it; anchor on the doc block's first line.
- The SHARED CHECKOUT already held another session's uncommitted work in the same file
  (`TERM_WHEEL_LINES` 3→1, `session_has_live_pty` and the ATTACH debounce change) — none of it mine.
