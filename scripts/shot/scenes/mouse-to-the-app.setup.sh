# A stand-in agent that asks for the mouse the way Claude Code's fullscreen renderer does (`?1002h`
# button-motion tracking, `?1006h` SGR coordinates) and prints every report it is handed, so a click
# and a drag typed into the pane show up as the bytes the program received. The PREWARM POOL is off
# so the one launch below is the stand-in.
mkdir -p "$WORK/data"
cat > "$WORK/data/config.json" <<'JSON'
{"prewarm_agents": false, "prewarm_sessions": false}
JSON
cat > "$RUNTIME/mouse-stub.py" <<'PY'
import os, sys, tty
out = sys.stdout
out.write("\x1b[?1049h\x1b[H\x1b[2J\x1b[?1002h\x1b[?1006h")
out.write("stand-in agent: asked for the mouse (?1002h button-motion, ?1006h SGR)\r\n")
out.write("every report this program receives is printed below:\r\n\r\n")
out.flush()
tty.setraw(0)
while True:
    data = os.read(0, 1024)
    if not data:
        break
    for report in data.decode("latin1").split("\x1b"):
        if report:
            out.write("  ESC " + report + "\r\n")
    out.flush()
PY
cat > "$RUNTIME/agent" <<AGENT
#!/bin/sh
exec python3 "$RUNTIME/mouse-stub.py"
AGENT
chmod +x "$RUNTIME/agent"
export NEBULA_AGENT_CMD="$RUNTIME/agent"
