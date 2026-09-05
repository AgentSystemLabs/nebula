---

## 💬 ==== YOU ASKED ====

"A user on iTerm2 had nebula working normally, then no click landed anywhere in the TUI and WHEEL
SCROLL scrolled iTerm's own view instead of the focused SESSION's TERMINAL PANE. Find out why nebula
stops receiving mouse events mid-run and fix it in nebula if the cause is ours; if it is a terminal
setting, name it. Keep DRAG SELECT, CLICK OUTSIDE and the WHEEL SCROLL steps as they are. (Assuming
iTerm2 3.5.x on macOS and a current release; agent unknown.)"

---

## 📋 ==== OVERVIEW ====

Two causes, both ours, both fixed. A panic on any background task handed the HOST TERMINAL back —
mouse off, raw mode off — while the TUI lived on; and iTerm2's Session ▸ Reset (⌘R) clears mouse
reporting without telling the app. The TUI now restores the terminal only for a panic on its own
thread, re-asks the HOST TERMINAL for its modes every 2 s and on resize, and flashes "a background task
crashed" instead of going quiet. nebula-tui 574 passed and clippy is clean — on a copy of HEAD, because
the SHARED CHECKOUT did not compile with another session's half-done change. MAKE INSTALL has run.

---

## 🔧 ==== DETAILS ====

**Two mechanisms, one symptom pair** — the report carried no log to pick one, so both live in the new
HOST TERMINAL module: `install_panic_hook` (`crates/nebula-tui/src/event_loop/host_terminal.rs:111`)
restores only for the loop's thread and counts the rest; `reassert_modes`, in the same file, re-sends
mouse capture, bracketed paste, focus reports and the KITTY PROTOCOL flags on a 2 s tick.

**Why not `Terminal::clear()` on resize** — ratatui's `clear()` asks the terminal for the cursor
position and crossterm gives up after 2 s with an error the loop turned into a fatal exit;
`Terminal::resize` clears without asking.

**Not a setting** — the profile's Mouse Reporting was still on; ⌘R is what turns it off.

Logged the entry (six gotchas); HOST TERMINAL row updated in `TERMS.md`, promoted nothing.

---

## 👉 ==== NEXT STEPS ====

1. Good to commit — `git add crates/nebula-tui/src/event_loop.rs crates/nebula-tui/src/event_loop/host_terminal.rs docs/keys.md && git commit`.
2. To see it, quit and relaunch the TUI — the DAEMON keeps every SESSION, so nothing else restarts.
