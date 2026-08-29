# Click Outside Any Modal Dismisses It — 2026-08-27

**Asked:** "when the help modal is up (or all modals), a user should be able to click outside to dismiss it"
→ picked: Every modal, click = Esc — a left-click outside the box does exactly what Esc does (cancel a
confirm, discard a prompt, close the rest), swallowed, never landing on the panel underneath.

**Did:** Only four overlays lacked it — Palette, Files, Grep, Tree, Hosts, Settings, Metrics and the
context menu already closed on an outside click in their own `handle_mouse` arms. A single pre-check at the
top of `handle_mouse` (`crates/nebula-tui/src/event_loop.rs`, just after the menu arm) now covers
`Overlay::Help` / `Confirm` / `Prompt` / `Diff`: Help and Diff set `app.overlay = None`; Confirm and Prompt
route a synthesized `KeyCode::Esc` through `handle_overlay_key`, so the settings-reset confirm still lands
back in the settings overlay, the switcher delete reopens the switcher, and an abandoned Claude name
prompt still restores the warm slot's spec. `Overlay::Help` became `Help(HelpView { area })`;
`ConfirmDialog` and `PromptDialog` grew `area: Rect`, written back in `ui.rs::draw_overlay`. Help overlay
row `click outside → dismiss any modal (= Esc)`, README mouse paragraph. Five tests under
`// ---- a click outside any modal dismisses it ----` in `event_loop.rs`.

**Gotchas:**
- Diff's Esc is two-stage (clears the file filter first), so click-outside must close it directly rather
  than synthesize Esc; Confirm and Prompt have single-stage Esc with side effects worth keeping, so those
  *do* synthesize it. Check the overlay's Esc arm before choosing which path a new modal takes.
- `draw_overlay` matches on `app.overlay.clone()`, so a rect used for hit-testing has to be written back
  with a second `if let Some(Overlay::X(v)) = &mut app.overlay { v.area = area }` at the end of the arm —
  the `ContextMenu::area` pattern. A unit variant (as `Help` was) has nowhere to put it.
- The pre-check requires `area.width > 0`: an overlay that has not been drawn yet has a zero rect, and
  without the guard every click would count as "outside". Tests that click into a modal without drawing
  first rely on this; tests that want the outside path draw once with `TestBackend` (see `drawn_modal_area`).
- `ConfirmDialog` has 14 struct-literal constructors in `event_loop.rs`, every `action:` on one line — a
  new field is a one-pass insert after that line, not 14 hand edits.
- In tests, `D` on a bare `seed_tree` opens no confirm; the bulk-delete confirm needs `seed_link` too
  (mirror `delete_all_sessions_leaves_links_alone`).
