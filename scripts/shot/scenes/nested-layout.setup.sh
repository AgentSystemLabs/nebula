# The NESTED layout (Settings → Appearance → Worktree layout → nested) on the README's orbit-api
# project: three worktrees, each a header over its cards along a rail. `main` holds a running
# session and a terminal; feat/auth-tokens a pull request that is ready, the checkout's changes
# counted beside it, and a session stopped on a permission prompt; fix/rate-limits a pull request
# that has merged and a session that finished unread — the one-line card a finished session
# recedes to. The preset is NEBULA_SHOT_THEME (`default` when unset), the pane beside the cards
# unless NEBULA_SHOT_PANE says `bottom`.
NEBULA_SHOT_PANE="${NEBULA_SHOT_PANE:-right}"
. "$HERE/scenes/readme-grid.setup.sh"
cat > "$WORK/data/config.json" <<JSON
{"prewarm_agents": false, "prewarm_sessions": false, "session_pane": "$NEBULA_SHOT_PANE",
 "worktree_layout": "nested", "theme": "${NEBULA_SHOT_THEME:-default}",
 "claude_model": "opus", "claude_effort": "xhigh"}
JSON
# fix/rate-limits has merged: off the open list, and its own lookup says so.
cat > "$WORK/fx/pr-list.json" <<'JSON'
[
  {"number": 57, "title": "Move the token store to sqlite", "url": "https://github.com/orbit/orbit-api/pull/57", "isDraft": false, "headRefName": "feat/auth-tokens"}
]
JSON
cat > "$WORK/fx/pr-view-fix-rate-limits.json" <<'JSON'
{"number": 58, "url": "https://github.com/orbit/orbit-api/pull/58", "title": "Rate limit the public endpoints", "state": "MERGED", "isDraft": false, "comments": [], "reviews": []}
JSON
cp "$WORK/fx/pr-view-fix-rate-limits.json" "$WORK/fx/pr-58.json"
cat > "$RUNTIME/agent" <<'AGENT'
#!/bin/sh
# Stand-in agent: launch 1 finishes once the cursor has moved on (unread done), launch 2 stops on a
# permission prompt, launch 3 is left running.
n=$(cat "$NEBULA_SHOT_COUNTER" 2>/dev/null || echo 0); n=$((n + 1)); echo "$n" > "$NEBULA_SHOT_COUNTER"
post() {
  curl -sS -m 3 -X POST -H "Authorization: Bearer $NEBULA_API_TOKEN" -H 'Content-Type: application/json' \
    -d "$2" "$NEBULA_API_URL/api/hooks/claude?agentId=$NEBULA_AGENT_ID&hookEvent=$1" >/dev/null 2>&1
}
case "$n" in
  1) title="Merge Rate Limits"
     post UserPromptSubmit '{"session_id":"shot-1","prompt":"Review the rate limit pull request and merge it once the checks pass"}'
     (sleep 5; post Stop '{"session_id":"shot-1"}') & ;;
  2) title="Review Token Store"
     post UserPromptSubmit '{"session_id":"shot-2","prompt":"Do one last review to check for major security issues, fix anything you find and push it"}'
     sleep 0.3; post PermissionRequest '{"session_id":"shot-2","tool_name":"Bash"}' ;;
  *) title="Label PRs Missing Screenshots"
     post UserPromptSubmit '{"session_id":"shot-3","prompt":"Add a label of screenshots requested to any pr that seems to be missing its screenshots"}' ;;
esac
"$NEBULA_SHOT_BIN" rename "$title" >/dev/null 2>&1 || true
exec /bin/cat
AGENT
chmod +x "$RUNTIME/agent"
