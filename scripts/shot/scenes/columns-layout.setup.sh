# The COLUMNS layout (Settings -> Appearance -> Worktree layout -> columns)
# on the README's orbit-api project, using the same seeded sessions as the
# nested-layout scene so the three columns show projects, PR-backed worktrees
# and sessions beside the pane.
NEBULA_SHOT_PANE="${NEBULA_SHOT_PANE:-right}"
. "$HERE/scenes/nested-layout.setup.sh"
python3 - <<'PY'
import json, os
from pathlib import Path

path = Path(os.environ["WORK"]) / "data" / "config.json"
cfg = json.loads(path.read_text())
cfg["worktree_layout"] = "columns"
cfg["recent_prompts"] = True
cfg["recent_prompts_count"] = 2
path.write_text(json.dumps(cfg, indent=2) + "\n")
PY
