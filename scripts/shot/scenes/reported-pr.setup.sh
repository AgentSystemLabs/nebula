# A Codex session reports a PR made outside its managed checkout. Branch
# lookup misses; only the Stop payload can supply the link and merged state.
mkdir -p "$WORK/data" "$WORK/fixtures"
printf '{"theme":"ocean","worktree_layout":"nested","prewarm_agents":false,"prewarm_sessions":false}\n' > "$WORK/data/config.json"
cp "$HERE/fixtures/user" "$WORK/fixtures/"
printf '[]\n' > "$WORK/fixtures/pr-list.json"
cat > "$WORK/fixtures/pr-384.json" <<'JSON'
{"number":384,"url":"https://github.com/other/game/pull/384","title":"Desert homes and ranch scenery","state":"MERGED","isDraft":false,"headRefName":"feat/desert-homes-kit","headRefOid":"external-sha","baseRefName":"map3-epic","comments":[],"reviews":[],"body":"Created in a separate repository. This session still tracks its merged PR."}
JSON
touch "$WORK/fixtures/pr-view-absent"
export NEBULA_GH_FIXTURES="$WORK/fixtures" NEBULA_SHOT_BIN="$BIN"
cat > "$RUNTIME/agent" <<'AGENT'
#!/bin/sh
post() {
  curl -sS -m 3 -X POST -H "Authorization: Bearer $NEBULA_API_TOKEN" -H 'Content-Type: application/json' \
    -d "$2" "$NEBULA_API_URL/api/hooks/codex?agentId=$NEBULA_AGENT_ID&hookEvent=$1" >/dev/null
}
post UserPromptSubmit '{"session_id":"shot-codex","prompt":"Build the desert homes kit in the game repo"}'
"$NEBULA_SHOT_BIN" rename Desert Homes Kit >/dev/null 2>&1 || true
sleep 1
post Stop '{"session_id":"shot-codex","last_assistant_message":"Merged [PR #384](https://github.com/other/game/pull/384) into map3-epic."}'
printf 'Merged PR #384 in the game repository.\r\n'
exec /bin/cat
AGENT
chmod +x "$RUNTIME/agent"
export NEBULA_AGENT_CMD="$RUNTIME/agent"
