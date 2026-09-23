# LAUNCHER VIEW scenes: the Experimental switch on, the PREWARM POOL off (every launch is one of the
# scripted ones below), two more projects beside the demo for the PROJECT PICKER to type through, and
# a stand-in agent whose three launches tell three stories — one keeps running and opens a pull
# request (it writes the stub gh's `pr view` answer for its own checkout, as a real agent's `gh pr
# create` would make one), one finishes, one stops on a permission question — each painting a short
# transcript so the pane beside the list has something to show.
mkdir -p "$WORK/data"
cat > "$WORK/data/config.json" <<'JSON'
{"prewarm_agents": false, "prewarm_sessions": false}
JSON
for extra in api-server web-app; do
  mkdir -p "$WORK/$extra"
  git -C "$WORK/$extra" init -q -b main
  git -C "$WORK/$extra" -c user.name=shot -c user.email=shot@example.invalid commit -q --allow-empty -m "$extra"
done
# shot.sh registers the demo with `$BIN add`; this wrapper registers the other two first.
REAL_BIN="$BIN"
cat > "$RUNTIME/nebula" <<WRAP
#!/bin/sh
if [ "\$1" = add ]; then
  "$REAL_BIN" add "$WORK/api-server" >/dev/null
  "$REAL_BIN" add "$WORK/web-app" >/dev/null
fi
exec "$REAL_BIN" "\$@"
WRAP
chmod +x "$RUNTIME/nebula"
BIN="$RUNTIME/nebula"
cp -R "$HERE/fixtures" "$WORK/fx"
export NEBULA_GH_FIXTURES="$WORK/fx"
export NEBULA_SHOT_BIN="$REAL_BIN" NEBULA_SHOT_COUNTER="$RUNTIME/launches"
cat > "$RUNTIME/agent" <<'AGENT'
#!/bin/sh
n=$(cat "$NEBULA_SHOT_COUNTER" 2>/dev/null || echo 0); n=$((n + 1)); echo "$n" > "$NEBULA_SHOT_COUNTER"
post() {
  curl -sS -m 3 -X POST -H "Authorization: Bearer $NEBULA_API_TOKEN" -H 'Content-Type: application/json' \
    -d "$2" "$NEBULA_API_URL/api/hooks/claude?agentId=$NEBULA_AGENT_ID&hookEvent=$1" >/dev/null 2>&1
}
say() { printf '%s\r\n' "$@"; }
case "$n" in
  1) title="Login redirect loop"
     post UserPromptSubmit "{\"session_id\":\"shot-1\",\"prompt\":\"Fix the login redirect loop when the session cookie has expired\"}"
     printf '{"number": 57, "url": "https://github.com/AgentSystemLabs/nebula/pull/57", "title": "Fix the login redirect loop on expired cookies", "state": "OPEN", "isDraft": false, "comments": [], "reviews": []}' \
       > "$NEBULA_GH_FIXTURES/pr-view-$(basename "$PWD").json"
     say "> Fix the login redirect loop when the session cookie has expired" "" \
         "⏺ Read(src/auth/session.rs)" "  ⎿  Read 214 lines" "" \
         "⏺ The redirect fires before the expired cookie is cleared, so the" \
         "  login page sees a session and bounces back. Clearing it first." "" \
         "⏺ Update(src/auth/session.rs)" "  ⎿  Updated with 6 additions and 2 removals" "" \
         "⏺ Bash(cargo test auth::)" "  ⎿  test result: ok. 24 passed; 0 failed" "" \
         "⏺ Bash(gh pr create --fill)" "  ⎿  https://github.com/AgentSystemLabs/nebula/pull/57" "" \
         "✻ Watching CI… (2m 14s · esc to interrupt)" ;;
  2) title="Changelog by date"
     post UserPromptSubmit "{\"session_id\":\"shot-2\",\"prompt\":\"Sort the changelog entries by date, newest first\"}"
     say "> Sort the changelog entries by date, newest first" "" \
         "⏺ Read(CHANGELOG.md)" "  ⎿  Read 480 lines" "" \
         "⏺ Update(CHANGELOG.md)" "  ⎿  Updated with 40 additions and 40 removals" "" \
         "⏺ Done — entries are newest first and Unreleased stays pinned on top."
     sleep 0.3; post Stop "{\"session_id\":\"shot-2\"}" ;;
  *) title="Daemon startup profile"
     post UserPromptSubmit "{\"session_id\":\"shot-3\",\"prompt\":\"Profile the daemon's startup and find what takes 400ms\"}"
     say "> Profile the daemon's startup and find what takes 400ms" "" \
         "⏺ Bash(cargo build --release --timings)" "" \
         "  Do you want to proceed?" "  ❯ 1. Yes" "    2. Yes, and don't ask again for cargo build" "    3. No"
     sleep 0.3; post PermissionRequest "{\"session_id\":\"shot-3\"}" ;;
esac
"$NEBULA_SHOT_BIN" rename "$title" >/dev/null 2>&1 || true
exec /bin/cat
AGENT
chmod +x "$RUNTIME/agent"
export NEBULA_AGENT_CMD="$RUNTIME/agent"
