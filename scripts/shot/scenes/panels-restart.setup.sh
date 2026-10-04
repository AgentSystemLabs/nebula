# The PANELS (Settings › Appearance › Layout `panels`) over the LAUNCHER VIEW scenes' demo: the same
# three projects and the same stand-in agent whose three launches run, finish and stop on a question.
. "$HERE/scenes/launcher.setup.sh"
cat > "$WORK/data/config.json" <<'JSON'
{"prewarm_agents": false, "prewarm_sessions": false, "layout": "panels"}
JSON
RESTART=1
