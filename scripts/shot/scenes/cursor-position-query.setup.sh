# A probe that asks for the cursor position the way crossterm's cursor::position() does: raw mode,
# `ESC[6n`, and a two-second deadline for the `ESC[row;colR` reply, printing crossterm's own error
# when none comes. It sits in every checkout (git ignores it) so the terminal finds it whichever one
# is selected. SHELL is a /bin/sh that reads no profile and prompts with a bare `$`, so the pane shows
# the probe rather than this machine's login profile and host name, the PREWARM POOL is off so `t`
# spawns the terminal the keys type into, and the keys wait out the deadline.
mkdir -p "$WORK/data"
cat > "$WORK/data/config.json" <<'JSON'
{"prewarm_agents": false, "prewarm_sessions": false}
JSON
for checkout in "$DEMO" "$WORK"/demo-worktrees/*; do
  cat > "$checkout/cursor-probe" <<'PY'
#!/usr/bin/env python3
import os, select, sys, termios, time, tty
fd = sys.stdin.fileno()
saved = termios.tcgetattr(fd)
tty.setraw(fd)
try:
    start = time.monotonic()
    os.write(1, b"\x1b[6n")
    reply = b""
    while not reply.endswith(b"R"):
        left = 2.0 - (time.monotonic() - start)
        if left <= 0 or not select.select([fd], [], [], left)[0]:
            break
        reply += os.read(fd, 32)
    ms = (time.monotonic() - start) * 1000
finally:
    termios.tcsetattr(fd, termios.TCSADRAIN, saved)
if reply.endswith(b"R"):
    row, col = reply[reply.rindex(b"[") + 1:-1].split(b";")
    print(f"cursor at row {int(row)}, col {int(col)} (answered in {ms:.0f} ms)")
else:
    print(f"The cursor position could not be read within a normal duration ({ms:.0f} ms)")
PY
  chmod +x "$checkout/cursor-probe"
done
mkdir -p "$DEMO/.git/info" && echo cursor-probe >> "$DEMO/.git/info/exclude"
cat > "$WORK/shell" <<'SH'
#!/bin/sh
# tmux starts the TUI through `$SHELL -c`; that one runs as asked.
for arg; do [ "$arg" = -c ] && exec /bin/sh "$@"; done
export PS1='$ '
exec /bin/sh --noprofile --norc -i
SH
chmod +x "$WORK/shell"
export SHELL="$WORK/shell"
export SHOT_KEY_SECS=2
