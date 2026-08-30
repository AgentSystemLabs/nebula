# PR #1 — Give every typed field real terminal line-editing

- **URL:** https://github.com/AgentSystemLabs/nebula/pull/1
- **Author:** @webdevcody
- **Merged:** 2026-08-22T17:02:21Z by @webdevcody (`15aeae7af446`)
- **Opened:** 2026-08-22T17:00:14Z
- **Branch:** `fixing-input-ux` → `main`
- **Diff:** +897 −240 across 6 file(s)

## Description

> ## What
>
> Every typed field in the TUI — note names, the prompt dialog, the fuzzy filters (diff, tree, palette, file finder), the grep query, the ssh destination — tracked a bare `String` and was append-only. There was no caret, `←`/`→` did nothing, and **⌥← typed a literal `b` into the field** instead of stepping back a word (macOS terminals send ⌥←/⌥→ as `ESC b` / `ESC f` — Terminal.app's bundled `keyMappings.plist` maps `~F702`/`~F703` to exactly that, and iTerm2 does the same).
>
> All eight fields now share one `TextInput` (text + caret) in the new `crates/nebula-tui/src/text_input.rs`.
>
> ## Keys
>
> | | |
> |---|---|
> | `←→` / `⌥←→` / `^b` `^f` | char and word motion |
> | `Home`/`End` / `^a` `^e` | line start / end |
> | `⌫` / `⌦` | delete around the caret |
> | `⌥⌫` `^w` / `⌥⌦` `⌥d` | delete a word either side |
> | `^u` / `^k` | kill to line start / end |
>
> Word motion splits on punctuation, so `⌥←` in a path walks it a segment at a time.
>
> ## No bindings were stolen
>
> `handle_key` returns `Edit::Ignored` for anything it doesn't own, and each overlay runs it **last**, after its own arms. So:
>
> - the tree browser's `←`/`→` still fold/unfold
> - the add-project prompt's `←`/`→` still ascend/dive the directory listing (the one place they were already spoken for — caret motion there is `⌥←→`, `^b`/`^f`, `Home`/`End`)
> - `^d`/`^u` still half-page the diff and tree views **when nothing is typed**; with a live filter `^u` is the line editor's kill-to-start (identical to the old "clear" when the caret is at the end, which it usually is)
> - `^n`/`^p`/`^o`/`^f`/`^y` in the palette, finder and tree are untouched
>
> Bracketed paste now lands in the focused field instead of falling through to the terminal pane.
>
> ## Rendering
>
> The block cursor draws **in place** rather than always at the tail, and a value longer than its field scrolls under it with a `…` on whichever end is clipped (previously long values just showed their tail). One `input_spans` helper replaced eight copies of the old `text + "█"` snippet.
>
> Help gained a `TYPING IN A FIELD` section; the note and non-path prompt hints mention `⌥←→`.
>
> ## Testing
>
> - 14 unit tests on `TextInput` (motion, deletes, multibyte, "⌥← arrives as Alt+b", unknown keys pass through)
> - 4 render tests on caret placement and scroll-window ellipsis
> - 3 event-loop tests driving the real overlays: editing a note with `⌥←` + insert-at-caret, `⌫` in a note edit not leaking to the list's delete-note binding, and `⌥←`/`^w` in the palette query re-running the filter
> - `cargo test --workspace` green (e2e included); `cargo fmt` / `clippy` clean on touched files
> - Verified live in tmux: after two `⌥←` the caret block sits on the `l` of "login" (`fg 0 / bg 6`) and typing inserts there
>
> 🤖 Generated with [Claude Code](https://claude.com/claude-code)

## Changed files (6)

- `crates/nebula-tui/src/app.rs` +18 −16
- `crates/nebula-tui/src/event_loop.rs` +230 −104
- `crates/nebula-tui/src/lib.rs` +1 −0
- `crates/nebula-tui/src/text_input.rs` +476 −0
- `crates/nebula-tui/src/tree_browser.rs` +3 −2
- `crates/nebula-tui/src/ui.rs` +169 −118

## Commits (1)

- `cd07baa20947` Give every typed field real terminal line-editing — @webdevcody, @claude

## Conversation (0)

_(no issue comments)_

## Reviews (0)

_(no review submissions)_

## Inline review comments (0)

_(no inline comments)_
