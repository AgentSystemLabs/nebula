# The TUI Re-Asks The HOST TERMINAL For The Mouse After A Worker Panic Or An iTerm2 ⌘R Reset — 2026-09-04

**Asked:** "a user ran into an issue when using nebula in iterm where it was working perfect but then
something broke and then they could never click inside the nebula app again, debug and fix if issue or
if you think it was a mac setting that disabled mouse interactions. also the scroll wheel seemed like it
was scrolling iterm and not the focused nebula session"
→ refined: A user on iTerm2 (assuming iTerm2 3.5.x on macOS, a current nebula release, agent unknown)
had nebula working normally, then "something broke" and from then on no click landed anywhere in the
TUI and WHEEL SCROLL scrolled iTerm's own view instead of the focused SESSION's TERMINAL PANE. Find out
why nebula stops receiving mouse events mid-run: an in-app path that turns mouse capture off (or leaves
the alt screen) and never restores it, or an iTerm2/macOS setting. Fix it in nebula if the cause is
ours; if it is a terminal setting, name it and how to re-enable it. Keep DRAG SELECT, CLICK OUTSIDE and
the WHEEL SCROLL steps as they are.

**Did:** Two mechanisms leave the TUI without the HOST TERMINAL's mouse and produce exactly that symptom pair, and the report carried no log to pick one, so
both are handled. (1) The panic hook `setup_terminal` installed called `restore_terminal()` for a panic
on *any* thread — the TUI runtime is `new_multi_thread` (2 workers) and every `tokio::spawn` (`gh`
lookups, ipc reader/writer, update check) or `std::thread::spawn` (vim reader, cursor catalogue) panic
is swallowed by tokio or dies with its thread, so the loop lived on with mouse capture off, raw mode off
and the primary screen under it. (2) iTerm2's Session ▸ Reset (`⌘R`) clears mouse reporting, focus
reports, bracketed paste, both kitty key-reporting stacks and shows the primary buffer without a byte
to the app. New `crates/nebula-tui/src/event_loop/host_terminal.rs` — the HOST TERMINAL module (setup/restore moved
out of `event_loop.rs` unchanged): `install_panic_hook(owner, restore_terminal)` restores only for the loop's
thread and counts the rest (`take_worker_panic` → the loop repaints and flashes "a background task
crashed — logged to tui.log"); `reassert_modes` re-sends `EnableMouseCapture`, `EnableBracketedPaste`,
`EnableFocusChange` and the KITTY PROTOCOL *set* form `CSI = 1 ; 1 u` every `MODE_REASSERT` (2 s) from a new
select arm; `on_host_resize` writes `?1047h` plus the modes and `repaint`s (`Terminal::resize(size)`)
before `Event::Resize` is handled. `docs/keys.md` Mouse section gained the paragraph. Gate: the SHARED
CHECKOUT did not compile (another session's `AgentKind::Pi`), so the change was proven on a `git
archive HEAD` copy in the scratchpad — nebula-tui 574 passed, clippy `-D warnings` clean, rustfmt
clean — plus a Python `pty.fork()` probe of the built binary: re-asserts at a 2 s cadence, a SIGWINCH
produced `?1047h` + modes + `2J` + a full 26 KB frame with the TUI alive, and no `CSI 6n` anywhere.

**Gotchas:**
- `main.rs::init_tui_logging`'s comment ("the chain on panic is: restore terminal → log to file →
  stderr") reads as if every panic were fatal; with `new_multi_thread` a task panic is a `JoinError`
  nobody reads, and the hook had already handed the terminal back. Pinned by
  `a_worker_thread_panic_is_counted_and_leaves_the_terminal_alone`.
- ratatui 0.30's `Terminal::clear()` opens with `backend.get_cursor_position()` — `CSI 6n` — and
  crossterm waits 2 s for the reply, then errors "The cursor position could not be read within a
  normal duration", which `?` turned into a FATAL exit of the TUI (`ratatui-core-0.1.2/src/terminal/
  buffers.rs:147`). nebula never called `clear()` before this task; `Terminal::resize(size)` clears and
  resets the diff buffer without asking (`resize.rs` → `clear_viewport`), which is what `repaint` does.
- iTerm2 3.5.10: `MainMenu.xib` binds `title="Reset" keyEquivalent="r"` (⌘R) to `reset:`, and
  `VT100Terminal.m::resetAllowingResize:` does `mouseMode = MOUSE_REPORTING_NONE`, `reportFocus = NO`,
  `bracketedPasteMode = NO`, `removeAllObjects` on both key-reporting stacks and
  `terminalShowPrimaryBuffer`. A dead mouse plus a wheel that scrolls iTerm is that, not a setting —
  the profile's `Mouse Reporting` key was still 1 on this machine.
- Re-entering the alternate screen is `?1047h`, never `?1049h`: xterm and iTerm2 guard the switch when
  already on it but 1049 saves the cursor unconditionally, so the exit's `?1049l` would restore an
  alternate-screen position onto the shell. Re-arming kitty flags is the set form `CSI = flags ; 1 u` —
  a second push leaves an entry behind the single pop at exit. Both pinned by unit tests.
- A Python `pty.fork()` probe of the binary needs a short `NEBULA_RUNTIME_DIR` (`path must be shorter
  than SUN_LEN` — the scratchpad path is 100+ bytes; `$TMPDIR/…` is 73), and because nothing answers
  `CSI ? u` the kitty probe burns its full 2 s before the loop starts, so the first re-assert lands at
  ~4 s, not 2. The same silence is what made `Terminal::clear()` fatal — a fake terminal is a good DSR
  smoke test.
- The dev `~/.nebula-dev/state/tui.log` holds 10 `PANIC` lines from 2026-08-25, all `failed printing to
  stderr: Input/output error (os error 5)` — the default hook printing to a tty that had gone away, a
  cascade inside a hook, not an app bug. Don't chase them.
