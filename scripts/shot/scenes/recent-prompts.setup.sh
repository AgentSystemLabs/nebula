# RECENT PROMPTS on, the PREWARM POOL off (so every launch is one of the three scripted below), and a
# stand-in agent that posts a few prompts through the HOOK RECEIVER — the same curl the installed
# hooks run — then titles its row with `nebula rename` and behaves like /bin/cat.
mkdir -p "$WORK/data"
cat > "$WORK/data/config.json" <<'JSON'
{"recent_prompts": true, "recent_prompts_count": 3, "prewarm_agents": false, "prewarm_sessions": false}
JSON
export NEBULA_SHOT_BIN="$BIN" NEBULA_SHOT_COUNTER="$RUNTIME/launches"
cat > "$RUNTIME/agent" <<'AGENT'
#!/bin/sh
# Stand-in agent for the recent-prompts scene. Launch N tells one of three stories.
n=$(cat "$NEBULA_SHOT_COUNTER" 2>/dev/null || echo 0); n=$((n + 1)); echo "$n" > "$NEBULA_SHOT_COUNTER"
post() {
  curl -sS -m 3 -X POST -H "Authorization: Bearer $NEBULA_API_TOKEN" -H 'Content-Type: application/json' \
    -d "$2" "$NEBULA_API_URL/api/hooks/claude?agentId=$NEBULA_AGENT_ID&hookEvent=$1" >/dev/null 2>&1
}
case "$n" in
  1) title="Login redirect loop"
     set -- "Fix the login redirect loop when the session cookie has expired" \
            "Add a regression test for the expired-cookie redirect" \
            "Why does the macOS CI job still time out on the new test?" ;;
  2) title="Changelog by date"
     set -- "Sort the changelog entries by date, newest first" \
            "Keep the Unreleased section pinned at the top" ;;
  *) title="Daemon startup profile"
     set -- "Profile the daemon's startup and find what takes 400ms" \
            "Cache the parsed config instead of re-reading it on every request" \
            "Now write that up in ARCHITECTURE.md" \
            "Squash those into one commit" ;;
esac
for p in "$@"; do
  post UserPromptSubmit "{\"session_id\":\"shot-$n\",\"prompt\":\"$p\"}"; sleep 0.15
  post Stop "{\"session_id\":\"shot-$n\"}"; sleep 0.15
done
"$NEBULA_SHOT_BIN" rename "$title" >/dev/null 2>&1 || true
exec /bin/cat
AGENT
chmod +x "$RUNTIME/agent"
export NEBULA_AGENT_CMD="$RUNTIME/agent"
