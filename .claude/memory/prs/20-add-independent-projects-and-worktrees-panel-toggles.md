# PR #20 — Add independent PROJECTS and WORKTREES panel toggles

- **URL:** https://github.com/AgentSystemLabs/nebula/pull/20
- **Author:** @lnmunhoz
- **Merged:** 2026-08-29T00:07:03Z by @webdevcody (`d9b97c02a46a`)
- **Opened:** 2026-08-28T08:58:16Z
- **Branch:** `toggle-column` → `main`
- **Diff:** +639 −123 across 9 file(s)

## Description

> ## Why
> The user might not always want to see the PROJECTS panel when the work is focused on a particular project. I added the possibility to hide/show the PROJECTS and also WORKTREES tab to allow more visible workspace for the TERMINAL.
>
> ## Summary
>
> - add independent `Shift+P` and `Shift+B` toggles for the PROJECTS PANEL and WORKTREES PANEL
> - persist visibility through `hide_projects` and `hide_worktrees` in CONFIG.JSON and Settings > Appearance
> - give hidden panel width to the TERMINAL PANE while preserving draggable widths
> - skip hidden panels in FOCUS, PANEL WALK, creation, and PALETTE flows
> - show FOOTER restore hints whenever a panel is hidden
>
> ## Validation
>
> - `cargo test --workspace` (715 passed)
> - `cargo clippy --workspace --all-targets` (passes with pre-existing warnings)
> - real PTY E2E coverage for independent toggling and CONFIG.JSON persistence
>
> ## Notes
>
> - Both panels default to shown.
> - The SESSIONS PANEL always remains visible.
> - No protocol, store, or UI STATE BLOB migration is required.
>
> ## Screenshots
>
> Pressed Shift + P, and 'PROJECTS' panel is hidden
> <img width="1476" height="869" alt="image" src="https://github.com/user-attachments/assets/35a5a283-0c08-442d-9f0e-a0abcf8eace7" />
>
> Pressed Shift + B, and 'WORKTREES' panel is hidden
> <img width="1466" height="875" alt="image" src="https://github.com/user-attachments/assets/48f40003-feb9-4b68-98d4-3a92167aef44" />
>
> Added shortcuts to Help panel:
> <img width="779" height="511" alt="image" src="https://github.com/user-attachments/assets/1175fccd-a9bc-447f-8dc8-89c4efea4ad7" />

## Changed files (9)

- `.claude/MEMORY.md` +47 −0
- `README.md` +20 −9
- `TERMS.md` +21 −16
- `crates/nebula-tui/src/app.rs` +114 −20
- `crates/nebula-tui/src/config.rs` +78 −3
- `crates/nebula-tui/src/event_loop.rs` +242 −51
- `crates/nebula-tui/src/keymap.rs` +23 −3
- `crates/nebula-tui/src/ui.rs` +58 −21
- `crates/nebula/tests/e2e_tui.rs` +36 −0

## Commits (4)

- `6c0feccb6731` Add independent panel visibility toggles — @lnmunhoz
- `9c3a4dfc92e6` Merge origin/main into toggle-column — @webdevcody, @claude
- `7c46cb872893` Merge origin/main into toggle-column — @webdevcody, @claude
- `08761c8b2bec` Log the second PR #20 conflict round, promote SELF-IMPROVING LOOP — @webdevcody, @claude

## Conversation (2)

### @webdevcody · 2026-08-28T14:03:14Z

> curious about your background theme, is that custom in your fork?

### @lnmunhoz · 2026-08-28T16:10:27Z

> @webdevcody That's just my wallpaper. The terminal looks like this because it has a window background opacity and background blur settings.
>
> On WezTerm, you can use this config:
>
> ```
> config.window_background_opacity = 0.85
> config.macos_window_background_blur = 50
> ```

## Reviews (0)

_(no review submissions)_

## Inline review comments (0)

_(no inline comments)_
