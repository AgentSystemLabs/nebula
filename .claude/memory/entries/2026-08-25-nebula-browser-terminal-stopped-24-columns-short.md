# `nebula browser` Terminal Stopped ~24 Columns Short — 2026-08-25

**Asked:** "when running nebula browser, there is a bunch of empty space in the right side of the terminal
panel... fix this. running nebula in iterm or ghostty doesn't have this extra space"

**Did:** One line of NEBULA BROWSER's ttyd args: `ttyd_args` (`crates/nebula/src/browser.rs:87`) now passes
`-t fontSize=13`, plus a test `a_font_client_option_is_passed_so_ttyd_refits_after_the_renderer_swap`.
The TUI was never at fault — it filled every column it was given; the xterm.js **grid** was too narrow.
Measured before/after through the real `nebula browser` at a 1600px window: 201 cols / 1407px grid /
183px dead → 225 cols / 1575px grid / 15px. Rejected `-t rendererType=dom` (also fills the width, since
the DOM renderer never rounds — but it is the slow renderer for a TUI that redraws constantly).

**Gotchas:**
- **The cause is a measure/render split inside xterm, invisible to the server.** ttyd calls
  `fitAddon.fit()` right after `Terminal.open()`, while the **DOM** renderer is live and
  `dimensions.css.cell.width` is the raw measured advance (7.8267px at size 13, Menlo).
  `cols = floor(avail / cellWidth)` → 201. ttyd *then* swaps in WebGL/canvas, which **floors the cell to
  a whole pixel (7px)** and never re-fits. 201 × 7 = 1407px of grid in 1590px of page.
- **`-t fontSize=13` is ttyd's own default and looks like a no-op — it is load-bearing.** ttyd's
  `applyPreferences` loop ends in `t.options[r]=n, 0===r.indexOf("font") && i.fit()`: *any* client option
  **named** `font…` buys a second `fit()`, and that one runs after the renderer swap. `rendererType` is
  merged in ahead of the server's `-t` keys, so the ordering holds. The test guards the flag, the `font`
  prefix, and its position before `--`.
- **Rows never showed the bug** — the cell height was already an integer (15px), so flooring changed
  nothing vertically. A "why is only the width wrong" symptom is the tell for integer-rounding of a cell.
- **`ps` renders `-t fontSize=13` as `-t fontSize 13`.** ttyd's `strsep(&option, "=")` NULs the `=` in
  argv in place. That is ttyd having *parsed* it, not nebula having passed it wrong.
- **Scale depends on `devicePixelRatio`.** At dpr 1 (external monitor, headless) the floor is to a whole
  pixel → ~10% loss; on Retina it floors to a half pixel → ~4%. Don't conclude "not reproducing" from a
  Retina window alone.
- **`--virtual-time-budget` cannot screenshot ttyd** — it fast-forwards timers and tears the page down
  while the PTY bytes are still arriving in real time. Drive Chrome over CDP instead: launch
  `--headless=new --remote-debugging-port=9333 --user-data-dir=/tmp/cdp-profile --window-size=1600,1000`,
  then `Page.navigate` + a real `setTimeout` + `Page.captureScreenshot`. Node 22 has a global `WebSocket`,
  so the whole client is ~25 dependency-free lines.
- **ttyd exposes the live terminal as `window.term`** (no React/preact fiber to dig through — the
  container has no framework keys). `window.term._core._renderService.dimensions` and
  `_charSizeService.width` are what prove a measure/render mismatch; the `.xterm-helper-textarea`'s inline
  `width`/`height` are the rendered cell dimensions if you only need a quick read.
- A `make browser` (MAKE BROWSER) run of your own leaves a `ttyd … ~/.cargo/bin/nebula` on 7681 that is **the user's**,
  not test residue. Match on the port you started before `pkill`.
