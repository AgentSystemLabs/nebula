# Copy Over `nebula ssh` Asks The Terminal, Not The Remote Box — 2026-08-27

**Asked:** "when I try to copy text after doing nebula ssh into an ubuntu machine I spun up, it keeps
saying copy failed (clipboard unavvailable). help me debug if this is something I need to support in
nebula using ssh -X or if I just need to install something on the device"

**Did:** Neither — added an OSC 52 CLIPBOARD ROUTE. NEBULA SSH (`nebula ssh`) execs `ssh -t HOST '… exec nebula'`
(`crates/nebula/src/ssh.rs:40`), so the *whole TUI* including the copy path runs on the remote;
`copy_to_clipboard` (`crates/nebula-tui/src/event_loop.rs`) then shells out to wl-copy/xclip/xsel on a
headless VM that has none of them and no `DISPLAY`. New `copy_and_flash(app, text, label)` fronts all
three copy call sites (FILE FINDER Ctrl+y, TREE BROWSER Ctrl+y, `copy_selection` — DRAG SELECT): local → platform tool as
before; `app.is_remote` (already computed from `SSH_CONNECTION`/`SSH_TTY` via
`nebula_core::host::is_remote_session`) → queue `App::pending_clipboard`, which `main_loop` writes as
`\x1b]52;c;<b64>\x07` through `terminal.backend_mut()` next to the existing OSC 22 pointer-shape write.
Dependency-free `base64_encode` (RFC 4648 vectors test) rather than pulling a crate for one call site.
Rejected `ssh -X`: it needs xclip remotely *plus* XQuartz locally, and lands the text in the X11
clipboard rather than the macOS pasteboard.

**Gotchas:**
- **The "clipboard unavailable" failure path no longer exists** — OSC 52 is always available as a
  fallback, so a copy can now silently no-op on a terminal that drops OSC 52 (Terminal.app does; Ghostty
  and iTerm2 don't). That is why the FLASH says `copied N chars (via terminal)` on the OSC 52 route: the
  route is the only thing we can honestly report. iTerm2 additionally needs Settings → General →
  Selection → "Applications in terminal may access clipboard".
- **BEL-terminate OSC 52, not ST.** The neighbouring OSC 22 write uses `\x1b\\`; OSC 52 is the sequence
  where BEL is the form every implementer accepts.
- The payload cannot be written with a bare `std::io::stdout()` from the copy helper — ratatui owns a
  `BufWriter<Stdout>` that would interleave. Queue it on `App` and let the main loop write it through the
  backend, exactly as `pointer_shape` does.
- **The fix has to be on the *remote* binary.** `nebula ssh` only installs nebula when the remote lacks
  it, so an existing remote keeps its old version until NEBULA UPGRADE (`nebula upgrade`) runs *there* — testing this from
  a freshly built local binary proves nothing.
- Not covered: `nebula ssh` launched from *inside* a local nebula pane. The remote's OSC 52 hits the local
  nebula's VENDORED VT100 parser and dies; passing it through the PTY layer is unbuilt.
