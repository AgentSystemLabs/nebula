# The README's skill picker: the hero's projects, a fake Claude config with a handful of skills (never
# the real ~/.claude), and a saved workflow in orbit-api's .orbit/config.json.
. "$HERE/scenes/readme-grid.setup.sh"
export CLAUDE_CONFIG_DIR="$WORK/claude"
skill() {
  mkdir -p "$CLAUDE_CONFIG_DIR/skills/$1"
  printf -- '---\nname: %s\ndescription: %s\n---\nbody\n' "$1" "$2" > "$CLAUDE_CONFIG_DIR/skills/$1/SKILL.md"
}
skill api-design "Stable API and interface design: contracts, versioning, error shapes"
skill code-review "Multi-axis review of a diff before it merges"
skill debugging "Systematic root-cause debugging when something broke"
skill security-hardening "Input handling, auth and secrets checked against the OWASP Top Ten"
skill spec-first "Write the spec before the code when requirements are vague"
skill tdd "Red, green, refactor: tests first for every behaviour change"
mkdir -p "$WORK/orbit-api/.orbit"
cat > "$WORK/orbit-api/.orbit/config.json" <<'JSON'
{"workflows": {"feature": {"skills": ["spec-first", "tdd", "code-review"]},
               "bugfix": {"skills": ["debugging", "tdd"]}}}
JSON
