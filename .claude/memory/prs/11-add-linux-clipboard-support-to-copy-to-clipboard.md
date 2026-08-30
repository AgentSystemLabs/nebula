# PR #11 — Add Linux clipboard support to copy_to_clipboard

- **URL:** https://github.com/AgentSystemLabs/nebula/pull/11
- **Author:** @rchdlps
- **Merged:** 2026-08-25T02:23:07Z by @webdevcody (`88a52bb8759b`)
- **Opened:** 2026-08-24T23:41:11Z
- **Branch:** `linux-clipboard-support` → `main`
- **Closes:** #9
- **Diff:** +27 −8 across 1 file(s)

## Description

> `copy_to_clipboard` only supports macOS (`pbcopy`); on every other OS it returns `false` unconditionally, so drag-select copy, select+copy mode, and `Ctrl+y` path copy all flash `copy failed (clipboard unavailable)` on Linux.
>
> This PR shells out to the platform clipboard utility instead:
>
> - `wl-copy` when `WAYLAND_DISPLAY` is set (Wayland)
> - `xclip -selection clipboard` (X11, most common)
> - `xsel --clipboard --input` (X11 fallback)
>
> Same spawn/pipe pattern as the existing `pbcopy` path.
>
> **Verified** on Linux X11 (Ubuntu 24.04, patched v0.3.0 build): drag-selected text lands in the system clipboard; readback via `xclip -o` matches. The standalone spawn/pipe logic was also verified in isolation.
>
> Fixes #9
>
> Note: terminals like Tabby do not handle OSC 52, so shelling out to a clipboard utility is the only mechanism that works everywhere.

## Changed files (1)

- `crates/nebula-tui/src/event_loop.rs` +27 −8

## Commits (1)

- `ca23930075d8` Add Linux clipboard support to copy_to_clipboard — @rchdlps

## Conversation (0)

_(no issue comments)_

## Reviews (0)

_(no review submissions)_

## Inline review comments (0)

_(no inline comments)_
