# VENDORED VT100 So Codex Scrollback Works — 2026-08-14

**Asked:** "scrolling back using codex doesn't work, but claude works fine, debug and fix"

**Did:** The VENDORED VT100 — vt100 0.15.2 in `vendor/vt100` with a one-line semantic change and wired it via
`[patch.crates-io]` in the root `Cargo.toml`, so both `nebula-tui` and `tui-term` pick it up
(`d1d1a50`). Two regression tests in `app.rs` — one replays a codex-style region scroll, and it also
fails if anyone drops the `[patch.crates-io]` wiring.

**Gotchas:**
- The bug was in the parser, not in nebula's WHEEL SCROLL handling. Codex is a ratatui **inline-viewport** app:
  it inserts history by setting a top-anchored DECSTBM scroll region (`ESC[1;{viewport_top}r`) and
  scrolling inside it. Stock vt100 0.15.2 **discards** any line scrolled out while a scroll region is
  active (`grid.rs`, `scroll_up`), so codex's scrollback stayed empty. Real terminals keep top-anchored
  region scrolls — which is why codex scrolls fine *outside* nebula.
- `vendor/vt100` is a **patched fork**. Do not upgrade or re-vendor it without re-applying this change.
- Full-screen apps are unaffected: the alternate screen's grid is created with zero scrollback capacity.
