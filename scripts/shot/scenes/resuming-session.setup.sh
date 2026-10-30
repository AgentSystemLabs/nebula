# RESUMING AN IDLE SESSION: the idle reaper takes a session while the user looks at another checkout,
# and coming back resumes it. The stand-in agent's first launch reports a session id and paints a
# prompt; its second launch (the `--resume`) prints only what Claude prints before its first frame —
# terminal mode switches — and then sits loading, so the frame shows what the pane says meanwhile.
mkdir -p "$WORK/data"
cat > "$WORK/data/config.json" <<'JSON'
{"prewarm_agents": false, "prewarm_sessions": false, "session_idle_timeout": "2s"}
JSON
export NEBULA_IDLE_REAP_MS=500
export NEBULA_SHOT_BIN="$BIN" NEBULA_SHOT_COUNTER="$RUNTIME/launches"
cat > "$RUNTIME/agent" <<'AGENT'
#!/bin/sh
n=$(cat "$NEBULA_SHOT_COUNTER" 2>/dev/null || echo 0); n=$((n + 1)); echo "$n" > "$NEBULA_SHOT_COUNTER"
post() {
  curl -sS -m 3 -X POST -H "Authorization: Bearer $NEBULA_API_TOKEN" -H 'Content-Type: application/json' \
    -d "$2" "$NEBULA_API_URL/api/hooks/claude?agentId=$NEBULA_AGENT_ID&hookEvent=$1" >/dev/null 2>&1
}
if [ "$n" = 1 ]; then
  post UserPromptSubmit '{"session_id":"shot-1","prompt":"Fix the login redirect loop"}'
  sleep 0.15
  post Stop '{"session_id":"shot-1"}'
  "$NEBULA_SHOT_BIN" rename "Login redirect loop" >/dev/null 2>&1 || true
  printf '> Fix the login redirect loop\r\n\r\n  Done.\r\n'
  exec /bin/cat
fi
# The resume: Claude's preamble, then a long transcript load.
printf '\0337\033[r\0338\033[?25h\033[?25l\033[?2004h\033[?2031h\033[?1004h\033]0;Claude Code\007'
exec sleep 600
AGENT
chmod +x "$RUNTIME/agent"
export NEBULA_AGENT_CMD="$RUNTIME/agent"
