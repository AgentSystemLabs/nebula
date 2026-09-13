# A project with more open pull requests than the WORKTREES PANEL has rows: the stock fixtures with
# their `gh pr list` answer replaced by a two-dozen-row list, generated here so the repo carries no
# second fixture tree. What Ctrl+d / Ctrl+u are for.
FIX="$WORK/fixtures-long"; mkdir -p "$FIX"; cp "$HERE"/fixtures/* "$FIX"/ 2>/dev/null || true
python3 - "$FIX/pr-list.json" <<'PY'
import json, sys
titles = [
    "Attach links to worktrees", "Wheel scroll steps one line a notch", "Done sound rings once per finish",
    "Palette walks every workspace", "Quick prompt cuts a fresh worktree", "Hotkeys tab rebinds the walk",
    "Root badge yields to the branch", "Merged checkouts turn purple", "Archive asks first when told to",
    "Tree browser previews on Enter", "Recent prompts hang under the pill", "Splash fades in on first run",
    "Hosts picker forgets a stale host", "Ctrl+q leaves the locked pane", "Open PRs fold to a header",
    "Diff viewer marks a file reviewed", "Workspaces bar counts unread finishes", "Sessions panel shows the PR row",
    "Agent presets wrap the task", "Placeholder rows while git cuts", "Footer flash names the double tap",
    "Memory overlay polls the daemon", "Update check runs off the tick", "Grep opens at the matched line",
]
rows = [
    {"number": 80 - i, "title": t, "url": "https://github.com/AgentSystemLabs/nebula/pull/%d" % (80 - i),
     "isDraft": i % 9 == 8, "headRefName": "pr-%d" % (80 - i)}
    for i, t in enumerate(titles)
]
json.dump(rows, open(sys.argv[1], "w"), indent=2)
PY
export NEBULA_GH_FIXTURES="$FIX"
