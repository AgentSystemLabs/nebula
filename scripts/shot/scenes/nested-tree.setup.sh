# The NESTED layout as a tree: the nested-layout scene's orbit-api project, with more than one
# session in a worktree so children hang off their root. fix/rate-limits holds a finished root and
# a child stopped on a permission prompt; feat/auth-tokens a running root and a running child;
# main a running session and a terminal. Threads the cursor is not on fold, so their roots sum up
# what they hide.
. "$HERE/scenes/nested-layout.setup.sh"
cat > "$RUNTIME/agent" <<'AGENT'
#!/bin/sh
# Stand-in agent: launch 1 finishes, launch 2 stops on a permission prompt, the rest keep running.
n=$(cat "$NEBULA_SHOT_COUNTER" 2>/dev/null || echo 0); n=$((n + 1)); echo "$n" > "$NEBULA_SHOT_COUNTER"
post() {
  curl -sS -m 3 -X POST -H "Authorization: Bearer $NEBULA_API_TOKEN" -H 'Content-Type: application/json' \
    -d "$2" "$NEBULA_API_URL/api/hooks/claude?agentId=$NEBULA_AGENT_ID&hookEvent=$1" >/dev/null 2>&1
}
case "$n" in
  1) title="Merge Rate Limits"
     post UserPromptSubmit '{"session_id":"shot-1","prompt":"Review the rate limit pull request and merge it once the checks pass"}'
     (sleep 3; post Stop '{"session_id":"shot-1"}') & ;;
  2) title="Reproduce The 429 Burst"
     post UserPromptSubmit '{"session_id":"shot-2","prompt":"Reproduce the burst of 429s the load test shows"}'
     sleep 0.3; post PermissionRequest '{"session_id":"shot-2","tool_name":"Bash"}' ;;
  3) title="Review Token Store"
     post UserPromptSubmit '{"session_id":"shot-3","prompt":"Do one last review to check for major security issues"}' ;;
  4) title="Write Migration Tests"
     post UserPromptSubmit '{"session_id":"shot-4","prompt":"Write tests for the tokens migration"}' ;;
  *) title="Label PRs Missing Screenshots"
     post UserPromptSubmit '{"session_id":"shot-5","prompt":"Add a label of screenshots requested to any pr that seems to be missing its screenshots"}' ;;
esac
"$NEBULA_SHOT_BIN" rename "$title" >/dev/null 2>&1 || true
exec /bin/cat
AGENT
chmod +x "$RUNTIME/agent"
