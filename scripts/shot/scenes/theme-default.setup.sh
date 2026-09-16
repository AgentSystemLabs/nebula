# The frame every THEME shot shares: the PREWARM POOL off, `theme` written before boot (the preset
# named in NEBULA_SHOT_THEME, `default` here — theme-<name>.setup.sh sets it and sources this file), and a
# stand-in agent that leaves the three scripted launches in three states, so the shot shows every
# status color the preset has to keep apart: launch 1 finishes after the cursor has moved on (unread
# done), launch 2 stays running (the warn sweep), launch 3 stops at a permission prompt (needs feedback).
mkdir -p "$WORK/data"
printf '{"theme":"%s","prewarm_agents":false,"prewarm_sessions":false}\n' "${NEBULA_SHOT_THEME:-default}" \
  > "$WORK/data/config.json"
export NEBULA_SHOT_BIN="$BIN" NEBULA_SHOT_COUNTER="$RUNTIME/launches"
cat > "$RUNTIME/agent" <<'AGENT'
#!/bin/sh
# Stand-in agent for the theme scenes. Launch N picks the state its row ends in.
n=$(cat "$NEBULA_SHOT_COUNTER" 2>/dev/null || echo 0); n=$((n + 1)); echo "$n" > "$NEBULA_SHOT_COUNTER"
post() {
  curl -sS -m 3 -X POST -H "Authorization: Bearer $NEBULA_API_TOKEN" -H 'Content-Type: application/json' \
    -d "$2" "$NEBULA_API_URL/api/hooks/claude?agentId=$NEBULA_AGENT_ID&hookEvent=$1" >/dev/null 2>&1
}
case "$n" in
  1) title="Login redirect loop"
     post UserPromptSubmit '{"session_id":"shot-1","prompt":"Fix the login redirect loop"}'
     # Finish once the keys have moved the cursor to the next launch, so the finish goes unread.
     (sleep 4; post Stop '{"session_id":"shot-1"}') & ;;
  2) title="Changelog by date"
     post UserPromptSubmit '{"session_id":"shot-2","prompt":"Sort the changelog by date"}' ;;
  *) title="Daemon startup profile"
     post UserPromptSubmit '{"session_id":"shot-3","prompt":"Profile the daemon startup"}'; sleep 0.15
     post PermissionRequest '{"session_id":"shot-3","tool_name":"Bash"}' ;;
esac
"$NEBULA_SHOT_BIN" rename "$title" >/dev/null 2>&1 || true
exec /bin/cat
AGENT
chmod +x "$RUNTIME/agent"
export NEBULA_AGENT_CMD="$RUNTIME/agent"
