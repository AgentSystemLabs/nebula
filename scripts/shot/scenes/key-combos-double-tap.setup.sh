# The PREWARM POOL off so the Sessions panel is quiet; the KEY COMBO DISPLAY is always on.
mkdir -p "$WORK/data"
cat > "$WORK/data/config.json" <<'JSON'
{"prewarm_agents": false, "prewarm_sessions": false}
JSON
# The two l presses must land inside DOUBLE_TAP (400 ms) to read as one gesture.
export SHOT_KEY_SECS=0.25
