# The AGENT PRESETS the picker lists: seeded straight into the isolated DATA DIR before boot.
mkdir -p "$WORK/data"
cat > "$WORK/data/agent_presets.json" <<'JSON'
[
  {"name": "reviewer", "kind": "claude", "model": "opus", "effort": "high",
   "prefix": "Be strict: read the issue and its comments before touching code.", "postfix": "Run the tests before you stop."},
  {"name": "scratch", "kind": "codex", "model": "gpt-5.5", "effort": null, "prefix": "", "postfix": ""}
]
JSON
