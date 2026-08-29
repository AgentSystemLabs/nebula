# Wheel Scrollback Vs Claude's Alt Screen — 2026-08-18 → 08-21

**Asked:** "when I scroll on my mouse wheel know (or track pad), it doesn't seem to scroll back in the
terminal session output, it instead just switches my previous entered prompts in the input" — and again
later: "…it instead it says 'Scroll wheel is sending arrow keys · use PgUp/PgDn to scroll' and it just
keeps showing previous prompts I'm using, how do I fix that"

**Did:** `handle_mouse` in `event_loop.rs` (see `mouse_protocol_mode` at `event_loop.rs:5199`) now
forwards a real SGR wheel report (`\x1b[<64;col;rowM` / 65) at the 1-based pane cell whenever
`screen.mouse_protocol_mode() != None`; arrow synthesis remains only for mouseless alt-screen apps
(plain vim/less).

**Gotchas:**
- Claude Code 2.1.x renders its main UI on the alternate screen and enables mouse tracking
  `?1000h ?1002h ?1003h ?1006h` **in the same write as** `?1049h`, so a vt100 replay sees both or neither.
- The old arrow-synthesis fallback is what triggered Claude's own `arrow-burst` detector and that warning
  banner. Check the child's mouse protocol mode in the vendored vt100 before assuming arrows are right.
