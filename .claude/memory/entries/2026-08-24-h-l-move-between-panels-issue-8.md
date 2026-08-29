# h/l Move Between Panels (Issue #8) — 2026-08-24

**Asked:** "work on https://github.com/AgentSystemLabs/nebula/issues/8 in a worktree make pr when done,
move notes and links to other hotkeys, but h and l should be for left and right" — issue #8 asks for the
vim pairing, since `h`/`l` opened the ssh hosts picker and the add-link prompt instead.

**Did:** (extended 2026-08-27: a double tap at either end now jumps like ⇧Tab/Tab — see the entry above.)
Four `defaults:` arrays in `crates/nebula-tui/src/keymap.rs` — `focus_left` → `["h", "left"]`,
`focus_right` → `["l", "right"]`, `hosts` → `["shift+h"]`, `new_link` → `["shift+l"]`. No dispatch code
changed. New test `h_and_l_walk_panel_focus_like_the_arrows` in `event_loop.rs`; the hosts and link tests
(8 unit + 3 in `crates/nebula/tests/e2e_tui.rs`) now drive `⇧H`/`⇧L`. PR #12 off `worktree-hl-panel-nav`.

**Gotchas:**
- The user said "move **notes** and links", but notes is `e` and never conflicted — the two actions
  actually sitting on `h`/`l` were **hosts** and links. Read the issue, not just the prompt.
- **Never bulk-replace `Char('h')`/`Char('l')` in `event_loop.rs`.** Most hits are overlay-local grammar
  the keymap doesn't own and must not change: settings tab/value cycling (~3495-3514), the diff and tree
  browsers (~2919, ~2960-2966). Only the *test* presses needed swapping.
- Footer and Help hints come from `Keymap::first(action)`, so **the chord you want displayed has to lead
  the `defaults:` list** — `["h", "left"]` shows `h`, `["left", "h"]` would still show `←`.
- Changing a default is safe for existing users: `Keymap::overrides()` (keymap.rs:860) persists only rows
  that differ from `defaults`, so an untouched action picks the new key up on upgrade while anyone who
  rebound it keeps theirs.
- `defaults_do_not_collide_within_a_scope` is the guard for this kind of edit — it fails loudly if a new
  default double-books a chord in the same `Scope`.
