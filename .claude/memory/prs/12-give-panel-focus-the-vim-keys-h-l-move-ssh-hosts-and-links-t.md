# PR #12 — Give panel focus the vim keys (h/l), move ssh hosts and links to Shift

- **URL:** https://github.com/AgentSystemLabs/nebula/pull/12
- **Author:** @webdevcody
- **Merged:** 2026-08-25T02:36:01Z by @webdevcody (`6a0effaae35c`)
- **Opened:** 2026-08-25T02:31:27Z
- **Branch:** `worktree-hl-panel-nav` → `main`
- **Closes:** #8
- **Diff:** +91 −27 across 5 file(s)

## Description

> Closes #8.
>
> `h` / `l` now move focus between the four panels — the horizontal twins of the `j` / `k` already bound to selection. The arrow keys keep working exactly as before; `h` and `l` are **added** to `focus_left` / `focus_right`, nothing was taken away.
>
> The two actions that were sitting on those letters move onto the shifted keys:
>
> | Action | Was | Now |
> |---|---|---|
> | Focus left / right | `←` `→` | `h` `←` / `l` `→` |
> | SSH hosts | `h` | `⇧H` |
> | Attach a link | `l` | `⇧L` |
>
> Both `⇧H` and `⇧L` were free.
>
> ### Notes
>
> - The letter leads each default list, so the footer hints and the Help overlay — which are spelled from the live keymap — now show `h` / `l` for the focus walk, and `⇧H` / `⇧L` for hosts and links, with no extra edits.
> - Only *defaults* changed. The config persists overrides rather than the whole table (`Keymap::overrides`), so anyone who had already rebound `hosts` or `new_link` keeps their binding, and everyone else picks these up on upgrade. Every key here is still rebindable in Settings → Hotkeys.
> - `h` / `l` inside overlays (settings tabs, the diff viewer, the tree browser) are overlay-local grammar and are untouched.
>
> ### Verification
>
> - New unit test `h_and_l_walk_panel_focus_like_the_arrows` covers the full walk in both directions plus the stops at Projects and Sessions, and asserts neither key opens an overlay any more.
> - The existing hosts-picker and link tests now drive `⇧H` / `⇧L`, including the three `e2e_tui` PTY tests.
> - `defaults_do_not_collide_within_a_scope` still passes, so the new defaults are conflict-free.
> - `cargo test --workspace`, `cargo fmt --check` and `cargo clippy --workspace --all-targets` are clean (clippy warnings present are pre-existing on `main`).
> - README keymap table and the two prose mentions updated.
>
> 🤖 Generated with [Claude Code](https://claude.com/claude-code)

## Changed files (5)

- `.claude/MEMORY.md` +25 −0
- `README.md` +7 −6
- `crates/nebula-tui/src/event_loop.rs` +51 −13
- `crates/nebula-tui/src/keymap.rs` +4 −4
- `crates/nebula/tests/e2e_tui.rs` +4 −4

## Commits (2)

- `a4f6853b7c83` Give panel focus the vim keys, and move ssh hosts and links to shift — @webdevcody, @claude
- `ac32c46a35ca` Log the h/l panel-navigation remap in the shared memory — @webdevcody, @claude

## Conversation (0)

_(no issue comments)_

## Reviews (0)

_(no review submissions)_

## Inline review comments (0)

_(no inline comments)_
