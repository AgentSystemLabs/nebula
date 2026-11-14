# The two colors that must not be mistaken for each other, in one frame: the wheel-one-line checkout's
# pull request has merged (fixtures/merged, as in pr-row-merged), and a session under it finished while
# the cursor was elsewhere. So the checkout's row wears the `merged` purple — dot, rail and name, solid: it
# was met already merged, so no ONE-SHOT SWEEP — with the `done` color right beside it in its `1 done` badge
# and on the unread row below. The preset is ORBIT_SHOT_THEME (`default` when unset):
# `ORBIT_SHOT_THEME=ocean scripts/shot/shot.sh merged-unread-done`.
export ORBIT_GH_FIXTURES="$HERE/fixtures/merged"
mkdir -p "$WORK/data"
printf '{"theme":"%s","prewarm_agents":false,"prewarm_sessions":false}\n' "${ORBIT_SHOT_THEME:-default}" \
  > "$WORK/data/config.json"
export ORBIT_SHOT_BIN="$BIN" ORBIT_SHOT_COUNTER="$RUNTIME/launches"
cat > "$RUNTIME/agent" <<'AGENT'
#!/bin/sh
# Stand-in agent: launch 1 finishes once the cursor has moved on (unread done), launch 2 finishes under
# the cursor (read). Neither is left running or asking — either would outrank the merge on the row.
n=$(cat "$ORBIT_SHOT_COUNTER" 2>/dev/null || echo 0); n=$((n + 1)); echo "$n" > "$ORBIT_SHOT_COUNTER"
post() {
  curl -sS -m 3 -X POST -H "Authorization: Bearer $ORBIT_API_TOKEN" -H 'Content-Type: application/json' \
    -d "$2" "$ORBIT_API_URL/api/hooks/claude?agentId=$ORBIT_AGENT_ID&hookEvent=$1" >/dev/null 2>&1
}
case "$n" in
  1) title="Login redirect loop"
     post UserPromptSubmit '{"session_id":"shot-1","prompt":"Fix the login redirect loop"}'
     (sleep 3; post Stop '{"session_id":"shot-1"}') & ;;
  *) title="Changelog by date"
     post UserPromptSubmit '{"session_id":"shot-2","prompt":"Sort the changelog by date"}'; sleep 0.15
     post Stop '{"session_id":"shot-2"}' ;;
esac
"$ORBIT_SHOT_BIN" rename "$title" >/dev/null 2>&1 || true
exec /bin/cat
AGENT
chmod +x "$RUNTIME/agent"
export ORBIT_AGENT_CMD="$RUNTIME/agent"
