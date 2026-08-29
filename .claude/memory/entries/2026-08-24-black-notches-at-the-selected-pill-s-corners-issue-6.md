# Black Notches At The Selected Pill's Corners (Issue #6) — 2026-08-24

**Asked:** "try to fix this style issue, see attached image https://github.com/AgentSystemLabs/nebula/issues/6"
— the issue body: "there are little black bars top and bottom when the focus terminal setting is enabled,
find a way to make sure those are gray and make the row itself."

**Did:** The PILL ROW — `render_pill` in `crates/nebula-tui/src/ui.rs`. The rail now owns the pill's first column
outright: `PILL_RAIL` (`█`) on the text row, and each pad row's own `PILL_HALF` glyph (`▄`/`▀`) drawn in
the rail color instead of the old `PILL_RAIL_CAPS` quadrants (`▖`/`▘`, added in `4bea626`). Updated the
glyph assertions in `event_loop.rs::pill_rail_spans_pads_and_sessions_match_worktree_stride` and added
`ui.rs::pill_rail_leaves_no_untinted_quarter_at_the_corners`.

**Gotchas:**
- **A terminal cell holds one glyph and two colors, so three colors in one cell is impossible.** The pad
  row's rail cell wants panel-bg (outside the pill), rail, *and* fill — a quadrant cap can only pick two,
  so the fill quarter beside it fell through to bare background. That is the whole bug; there is no
  cleverer glyph. Options are: rail takes the full cell (chosen), or the fill takes it and the rail stops
  at the text row.
- **The setting in the issue is the FOCUS TINT (`focus_tint`, "Focused panel tint"), not anything called "focus
  terminal".** The notch has always been there — without the tint it's the terminal's own background
  (`#282c34` on this user's Terminal.app) against `sel_bg` `#3a3a3a` and nearly invisible. `draw_focus_tint`
  only repaints cells whose `bg == Color::Reset`, so it turns exactly that stranded quarter near-black.
- **Do not evaluate a TUI style change by reading code.** Mocking the four candidate geometries as PNGs
  settled it in one look — the "fill quarter with `bg = fill`" variant sprouts a gray tab above the pill,
  and a half-block rail with full-width caps flares into an I-beam.
- **You can render the real buffer without tmux or a font.** A temporary `#[test]` that draws
  `ui::draw` into a TESTBACKEND (`TestBackend`), dumps `symbol\tfg\tbg` per cell, plus a ~60-line pure-Python PNG
  writer that paints block glyphs as rects and any text glyph as a bar, reproduces the artifact exactly
  and proves the fix. Much cheaper than the `NEBULA_RUNTIME_DIR` + tmux SCREENSHOT HARNESS for anything
  made of block-drawing characters.
