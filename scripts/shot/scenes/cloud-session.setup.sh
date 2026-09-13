# The PREWARM POOL off (so the one launch below is the scripted one) and a stand-in agent that says
# what `claude --cloud <task>` says — the new session's URL — and exits, the way the real CLI does.
# The DAEMON reads the session id off that output; the row's pane is then the CLOUD SESSION PANEL.
mkdir -p "$WORK/data"
cat > "$WORK/data/config.json" <<'JSON'
{"prewarm_agents": false, "prewarm_sessions": false}
JSON
cat > "$RUNTIME/agent" <<'AGENT'
#!/bin/sh
printf 'Created cloud session: Login redirect loop\r\n'
printf 'View: https://claude.ai/code/session_01SQugK2HDyk33coSrfqFJk4?from=cli&m=0\r\n'
printf 'Resume with: claude --teleport session_01SQugK2HDyk33coSrfqFJk4\r\n'
exit 0
AGENT
chmod +x "$RUNTIME/agent"
export NEBULA_AGENT_CMD="$RUNTIME/agent"
