# A worktree with more sessions than the SESSIONS PANEL has rows: thirty quick prompts, the newest
# twenty-eight archived, so the ARCHIVED group runs off the bottom of the column — what Ctrl+d /
# Ctrl+u are for there. The PREWARM POOL is off so every launch is one of the scripted rows, and the
# stand-in agent titles its row from the launch counter (`nebula rename`, as a real agent does), then
# behaves like /bin/cat.
mkdir -p "$WORK/data"
cat > "$WORK/data/config.json" <<'JSON'
{"prewarm_agents": false, "prewarm_sessions": false}
JSON
export NEBULA_SHOT_BIN="$BIN" NEBULA_SHOT_COUNTER="$RUNTIME/launches"
cat > "$RUNTIME/agent" <<'AGENT'
#!/bin/sh
# Stand-in agent for the sessions-half-page scenes: launch N takes title N.
n=$(cat "$NEBULA_SHOT_COUNTER" 2>/dev/null || echo 0); n=$((n + 1)); echo "$n" > "$NEBULA_SHOT_COUNTER"
title=$(sed -n "${n}p" <<'TITLES'
Login redirect loop
Changelog by date
Daemon startup profile
Wheel scroll one line
Done sound once
Palette across workspaces
Quick prompt worktree
Hotkeys tab
Root badge yields
Merged checkouts purple
Confirm on archive
Tree browser preview
Recent prompts
Splash fade
Stale hosts
Ctrl+q unlock
Fold open PRs
Reviewed mark
Unread finishes
PR row
Agent presets
Placeholder rows
Double-tap flash
Memory overlay
Update check
Grep to line
Issues modal
IME cursor
Mouse to the app
Draft PRs hidden
TITLES
)
[ -n "$title" ] && "$NEBULA_SHOT_BIN" rename "$title" >/dev/null 2>&1
exec /bin/cat
AGENT
chmod +x "$RUNTIME/agent"
export NEBULA_AGENT_CMD="$RUNTIME/agent"
