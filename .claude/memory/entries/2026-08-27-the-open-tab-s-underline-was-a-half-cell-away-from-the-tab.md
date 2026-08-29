# The Open Tab's Underline Was A Half-Cell Away From The Tab — 2026-08-27

**Asked:** "fix the small gap between the header workspace name and the bottom bar if possible" — with a
screenshot of the Workspaces bar: the open tab's dark `sel_bg` block, a strip of black, then the green
accent underline.

**Did:** One glyph. `crates/nebula-tui/src/ui.rs::draw_workspaces_bar` drew the open WORKSPACE TAB's TAB UNDERLINE as
`━` (U+2501) into the rule row; it now draws **`▀`** (U+2580, upper half block) with the same `th.accent`
fg. The tab's `sel_bg` fill still stops at `area.height - 1`, so the block's bottom edge and the half
block's top edge are the same pixel row — flush, no gap. Rejected: extending the fill through the rule row
and painting `set_bg(th.sel_bg)` under a kept `━` — it closes the black gap but leaves a half-cell of
`sel_bg` *below* the accent, so the tab hangs past its own underline. Tests updated in
`crates/nebula-tui/src/event_loop.rs`: the cell assertion in the tab-surface test (`"━"` → `"▀"`, plus the
why) and `rule.contains("━━")` → `"▀▀"` in `the_workspaces_bar_sits_directly_above_projects`. 452
nebula-tui tests green, fmt clean.

**Gotchas:**
- **A sub-cell gap is invisible to the TESTBACKEND (`TestBackend`).** The buffer holds a symbol and a style, not pixels, so
  no `buffer_text` or cell assertion can see that `━` renders at the cell's *midline* and leaves the top
  ~40% of the cell unpainted. The only guard available is asserting the symbol, which is why the test
  carries the reason in a comment.
- **Box-drawing line glyphs never touch a cell edge; block elements do.** Any "join two rows of fill"
  problem in this TUI is a block-element problem (`▀`/`▄`), not a heavier-line problem.
- **Judging this needs a real font raster, not a terminal.** Pillow + `/System/Library/Fonts/Menlo.ttc` at
  ~27px, drawing the candidate cell stacks side by side, settles it in one PNG — no demo daemon, no tmux.
  See the SCREENSHOT HARNESS (`tui-screenshot-harness`) note for the full-app version when a change is bigger than a glyph.
- The accent underline now sits slightly *above* the `─` edge rule flanking it (mid-cell vs top-of-cell).
  That is deliberate — the tab indicator reads as heavier than the divider — not a misalignment to "fix".
