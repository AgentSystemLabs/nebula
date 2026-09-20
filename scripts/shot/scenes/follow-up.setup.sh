# Two sessions that finish quietly, so the cards read as idle agents waiting for their next turn and
# nothing sweeps over the box. The preset is NEBULA_SHOT_THEME (`default` when unset).
mkdir -p "$WORK/data"
printf '{"theme":"%s","prewarm_agents":false,"prewarm_sessions":false}\n' "${NEBULA_SHOT_THEME:-default}" \
  > "$WORK/data/config.json"
export NEBULA_SHOT_BIN="$BIN" NEBULA_SHOT_COUNTER="$RUNTIME/launches"
cat > "$RUNTIME/agent" <<'AGENT'
#!/bin/sh
# Stand-in agent: report the prompt it was launched on, then finish at once and sit on /bin/cat, so the
# session stays live — a follow-up goes to a running CLI, not a reaped one.
n=$(cat "$NEBULA_SHOT_COUNTER" 2>/dev/null || echo 0); n=$((n + 1)); echo "$n" > "$NEBULA_SHOT_COUNTER"
post() {
  curl -sS -m 3 -X POST -H "Authorization: Bearer $NEBULA_API_TOKEN" -H 'Content-Type: application/json' \
    -d "$2" "$NEBULA_API_URL/api/hooks/claude?agentId=$NEBULA_AGENT_ID&hookEvent=$1" >/dev/null 2>&1
}
case "$n" in
  1) title="Login redirect loop"
     post UserPromptSubmit '{"session_id":"shot-1","prompt":"Fix the login redirect loop"}' ;;
  *) title="Changelog by date"
     post UserPromptSubmit '{"session_id":"shot-2","prompt":"Sort the changelog by date"}' ;;
esac
sleep 0.15
post Stop "{\"session_id\":\"shot-$n\"}"
"$NEBULA_SHOT_BIN" rename "$title" >/dev/null 2>&1 || true
exec /bin/cat
AGENT
chmod +x "$RUNTIME/agent"
export NEBULA_AGENT_CMD="$RUNTIME/agent"
