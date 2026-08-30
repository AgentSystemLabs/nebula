# TERMINAL PANE WHEEL SCROLL Steps One Line A Notch — 2026-08-30

**Asked:** "the terminal scroll seems to skip 2 lines at a time, tweak to 1 line at a time"
→ refined: In the TERMINAL PANE, change WHEEL SCROLL so each wheel notch moves scrollback by 1 line instead of the current multi-line step (`TERM_WHEEL_LINES`); keep MOUSE MODE forwarding and alt-screen arrow emulation unchanged except they also step 1 line per notch.

**Did:** Set `TERM_WHEEL_LINES` from 3 to 1 in `crates/nebula-tui/src/event_loop.rs` (scrollback offset, and arrow-key repeat for mouseless alt-screen apps). Updated `wheel_sends_arrows_to_mouseless_alt_screen_apps` and the WHEEL SCROLL row in `TERMS.md`. `cargo test -p nebula-tui wheel_sends_arrows` passed.

**Gotchas:** The step was 3 lines in code, not 2 — user perception may have been coarser scroll or trackpad bursts; MOUSE MODE apps still get one SGR wheel report per notch (unchanged).

**Corrections:** 0
