# Restyle, Focus Wash, And The SCREENSHOT HARNESS — 2026-08-20 → 08-21

**Asked:** A run of visual passes: "would it be possible to space out the items in the projects worktrees
and sessions lists? like to make them feel like larger buttons, also visual hieachy…", "when a list panel
is in focus, render a themed gradient that comes up from the bottom, but very subtle…" → "the bottom focus
gradient looks like shit... let's think of a differnt indicator… maybe just make the entire panel a very
lightly colored (like 10% opactiy) theme color", and "when a session is running (when it's yellow status
or red), make the text animate with colors… it should be a sweeping animation."

**Did:** `d704da7` (borderless columns, raised-fill PILL ROW selection, quiet chrome) plus the STATUS SWEEP animation pass, with
a SETTING toggle to disable animations for CPU.

**Gotchas (recipe for screenshotting the TUI with demo data):**
- Isolate with `NEBULA_RUNTIME_DIR=/tmp/<short>` (SUN_LEN!) and `NEBULA_DATA_DIR=<scratch>/demo/data`.
  Never touch the real DAEMON — and note the daemon **detaches and outlives the tmux server**, so
  `kill $(cat $NEBULA_RUNTIME_DIR/daemon.pid)` when done.
- **Set `NEBULA_AGENT_CMD` even if you never create an agent** — the PREWARM POOL's warm slot launches a real
  `claude` on its own (shows as "1 agent · ~600MB" with zero agent rows in the DB). `/bin/cat` works.
- **One Bash call per drive**: the sandbox kills the private tmux server when the tool call ends, so
  new-session, send-keys, captures and kill-server must all happen in a single call. Send one key per
  call with 0.3–1s sleeps — batched keystrokes concatenate into the name prompt.
- `tmux capture-pane -epN` — **without `-N`** tmux trims trailing styled spaces and any background fill on
  the rightmost pane silently vanishes from the capture.
- Color and animation checks don't need PNGs: `capture-pane -ep` keeps SGR escapes; decode with
  `LC_ALL=C sed 's/\x1b\[/¶/g'` and grep for `38;5;N`, capturing 2–3 frames ~350ms apart to prove motion.
- charmbracelet freeze wrecks the cell grid — use a small pillow grid renderer instead. (This bullet used
  to also say Chrome headless gets SIGKILLed on this Mac; that is **no longer true** — see
  [Browser Terminal Stopped ~24 Columns Short], which drives `/Applications/Google Chrome.app` with
  `--headless=new` over CDP without trouble.)
