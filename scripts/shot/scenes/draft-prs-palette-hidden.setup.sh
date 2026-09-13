# Settings › Appearance › Draft pull requests set to `hidden` before the TUI starts: the draft (#39)
# leaves the PROJECT OPEN PRS GROUP and the header counts `2/3` — listed over open.
mkdir -p "$WORK/data"
cat > "$WORK/data/config.json" <<'JSON'
{"hide_draft_prs": true}
JSON
