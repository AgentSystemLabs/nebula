# Confirm on archive switched on; the PREWARM POOL off so the one launch is the one row.
mkdir -p "$WORK/data"
cat > "$WORK/data/config.json" <<'JSON'
{"confirm_on_archive": true, "prewarm_agents": false, "prewarm_sessions": false}
JSON
