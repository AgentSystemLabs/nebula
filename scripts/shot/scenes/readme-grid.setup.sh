# README screenshots: three projects (`orbit-api`, `atlas-web`, `relay-cli`) in place of the stock
# demo, orbit-api with two linked worktrees, and a stand-in agent whose six launches leave a card in
# every color — running with a pull request, finished unread, finished and read, and stopped on a
# permission prompt — under PROJECT TABS that carry each project's dots. The PREWARM POOL is off so
# every launch is one of the scripted ones, `$SHELL` is a stub that prints a `cargo test` run for the
# terminal card (and passes `-c` probes through to /bin/sh), and the pane sits along the bottom
# (NEBULA_SHOT_PANE=right puts it beside the cards) so a band shows four cards across.
mkdir -p "$WORK/data"
cat > "$WORK/data/config.json" <<JSON
{"prewarm_agents": false, "prewarm_sessions": false, "session_pane": "${NEBULA_SHOT_PANE:-bottom}",
 "claude_model": "opus", "claude_effort": "high", "codex_model": "gpt-5.6-sol", "codex_effort": "high"}
JSON
mk() {
  mkdir -p "$1"
  git -C "$1" init -q -b main
  git -C "$1" -c user.name=shot -c user.email=shot@example.invalid commit -q --allow-empty -m "init"
}
mk "$WORK/orbit-api"; mk "$WORK/atlas-web"; mk "$WORK/relay-cli"
# A tracked file in orbit-api that the feat/auth-tokens checkout rewrites, so its band counts lines.
mkdir -p "$WORK/orbit-api/src/auth"
seq 1 40 | sed 's/^/let line_/' > "$WORK/orbit-api/src/auth/session.rs"
git -C "$WORK/orbit-api" add -A
git -C "$WORK/orbit-api" -c user.name=shot -c user.email=shot@example.invalid commit -q -m "auth session"
git -C "$WORK/orbit-api" worktree add -q -b feat/auth-tokens "$WORK/orbit-api-worktrees/feat-auth-tokens" main
git -C "$WORK/orbit-api" worktree add -q -b fix/rate-limits "$WORK/orbit-api-worktrees/fix-rate-limits" main
FEAT="$WORK/orbit-api-worktrees/feat-auth-tokens"
{ seq 1 12 | sed 's/^/let line_/'; seq 1 58 | sed 's/^/let token_/'; } > "$FEAT/src/auth/session.rs"
mkdir -p "$FEAT/migrations"
printf 'CREATE TABLE tokens (id TEXT PRIMARY KEY, expires_at INTEGER NOT NULL);\n' > "$FEAT/migrations/003_tokens.sql"
printf 'pub mod token_store;\n' > "$FEAT/src/auth/token_store.rs"
# shot.sh registers `$DEMO` with `$BIN add`; this wrapper registers the three projects instead.
REAL_BIN="$BIN"
cat > "$RUNTIME/nebula" <<WRAP
#!/bin/sh
if [ "\$1" = add ]; then
  "$REAL_BIN" add "$WORK/orbit-api" >/dev/null
  "$REAL_BIN" add "$WORK/atlas-web" >/dev/null
  exec "$REAL_BIN" add "$WORK/relay-cli"
fi
exec "$REAL_BIN" "\$@"
WRAP
chmod +x "$RUNTIME/nebula"
BIN="$RUNTIME/nebula"
# Pull requests: one ready on feat/auth-tokens, one draft on fix/rate-limits, one with no checkout.
cp -R "$HERE/fixtures" "$WORK/fx"
export NEBULA_GH_FIXTURES="$WORK/fx"
cat > "$WORK/fx/pr-list.json" <<'JSON'
[
  {"number": 57, "title": "Move the token store to sqlite", "url": "https://github.com/orbit/orbit-api/pull/57", "isDraft": false, "headRefName": "feat/auth-tokens"},
  {"number": 58, "title": "Rate limit the public endpoints", "url": "https://github.com/orbit/orbit-api/pull/58", "isDraft": true, "headRefName": "fix/rate-limits"},
  {"number": 55, "title": "Retry webhook deliveries with backoff", "url": "https://github.com/orbit/orbit-api/pull/55", "isDraft": false, "headRefName": "webhooks-retry"}
]
JSON
cat > "$WORK/fx/pr-view-feat-auth-tokens.json" <<'JSON'
{"number": 57, "url": "https://github.com/orbit/orbit-api/pull/57", "title": "Move the token store to sqlite", "state": "OPEN", "isDraft": false, "comments": [], "reviews": []}
JSON
cp "$WORK/fx/pr-view-feat-auth-tokens.json" "$WORK/fx/pr-57.json"
cat > "$WORK/fx/pr-view-fix-rate-limits.json" <<'JSON'
{"number": 58, "url": "https://github.com/orbit/orbit-api/pull/58", "title": "Rate limit the public endpoints", "state": "OPEN", "isDraft": true, "comments": [], "reviews": []}
JSON
cp "$WORK/fx/pr-view-fix-rate-limits.json" "$WORK/fx/pr-58.json"
# The terminal card's shell: `$SHELL -l` prints a test run and waits; a `-c` probe is /bin/sh's.
cat > "$RUNTIME/shell" <<'SHELL'
#!/bin/sh
case "$*" in
  -l) printf '%s\r\n' '$ cargo test -p orbit-api' \
        '   Compiling orbit-api v0.4.2 (/Users/cody/code/orbit-api)' \
        '    Finished test profile [unoptimized + debuginfo] target(s) in 3.21s' \
        '     Running unittests src/lib.rs' \
        'test result: ok. 42 passed; 0 failed; 0 ignored' \
        '$ '
      exec /bin/cat ;;
  *) exec /bin/sh "$@" ;;
esac
SHELL
chmod +x "$RUNTIME/shell"
export SHELL="$RUNTIME/shell"
export NEBULA_SHOT_BIN="$REAL_BIN" NEBULA_SHOT_COUNTER="$RUNTIME/launches"
cat > "$RUNTIME/agent" <<'AGENT'
#!/bin/sh
# Stand-in agent: launch N tells story N. The dialect posted to follows the harness the keys picked.
n=$(cat "$NEBULA_SHOT_COUNTER" 2>/dev/null || echo 0); n=$((n + 1)); echo "$n" > "$NEBULA_SHOT_COUNTER"
dialect=claude
case "$n" in 3) dialect=codex;; 6) dialect=opencode;; esac
post() {
  curl -sS -m 3 -X POST -H "Authorization: Bearer $NEBULA_API_TOKEN" -H 'Content-Type: application/json' \
    -d "$2" "$NEBULA_API_URL/api/hooks/$dialect?agentId=$NEBULA_AGENT_ID&hookEvent=$1" >/dev/null 2>&1
}
say() { printf '%s\r\n' "$@"; }
case "$n" in
  1) title="Checkout design tokens"
     post UserPromptSubmit '{"session_id":"shot-1","prompt":"Migrate the checkout page to the new design tokens"}'
     say "> Migrate the checkout page to the new design tokens" "" \
         "● Read(src/pages/checkout.tsx)" "  ↳  Read 312 lines" "" \
         "● Update(src/pages/checkout.tsx)" "  ↳  Updated with 41 additions and 39 removals" "" \
         "● Done — every hard-coded color on the page now reads a token."
     (sleep 16; post Stop '{"session_id":"shot-1"}') & ;;
  2) title="JSON flag for lists"
     post UserPromptSubmit '{"session_id":"shot-2","prompt":"Add a --json flag to every list command"}'
     say "> Add a --json flag to every list command" "" \
         "● Bash(cargo add serde_json)" "" \
         "  Do you want to proceed?" "  ❯ 1. Yes" "    2. Yes, and don't ask again for cargo add" "    3. No"
     sleep 0.3; post PermissionRequest '{"session_id":"shot-2","tool_name":"Bash"}' ;;
  3) title="Token expiry tests"
     post UserPromptSubmit '{"session_id":"shot-3","prompt":"Write integration tests for token expiry and refresh"}'
     say "> Write integration tests for token expiry and refresh" "" \
         "• Added tests/token_expiry.rs with four cases: expired, refreshed, revoked, clock skew." "" \
         "• cargo test --test token_expiry" "  test result: ok. 4 passed; 0 failed"
     (sleep 10; post Stop '{"session_id":"shot-3"}') & ;;
  4) title="Token store on sqlite"
     post UserPromptSubmit '{"session_id":"shot-4","prompt":"Move the token store from a JSON file to sqlite"}'
     say "> Move the token store from a JSON file to sqlite" "" \
         "● Read(src/auth/session.rs)" "  ↳  Read 40 lines" "" \
         "● The store rewrites the whole JSON file on every refresh, which is where the" \
         "  lock contention comes from. Moving it to a sqlite table with an expiry index." "" \
         "● Write(migrations/003_tokens.sql)" "  ↳  Wrote 1 line" "" \
         "● Update(src/auth/session.rs)" "  ↳  Updated with 58 additions and 28 removals" "" \
         "● Bash(cargo test auth::)" "  ↳  test result: ok. 42 passed; 0 failed" "" \
         "● Bash(gh pr create --fill)" "  ↳  https://github.com/orbit/orbit-api/pull/57" "" \
         "✻ Watching CI… (2m 14s · esc to interrupt)" ;;
  5) title="Daemon startup profile"
     post UserPromptSubmit '{"session_id":"shot-5","prompt":"Profile the daemon'"'"'s startup and find what takes 400ms"}'
     say "> Profile the daemon's startup and find what takes 400ms" "" \
         "● Bash(cargo build --release --timings)" "" \
         "  Do you want to proceed?" "  ❯ 1. Yes" "    2. Yes, and don't ask again for cargo build" "    3. No"
     sleep 0.3; post PermissionRequest '{"session_id":"shot-5","tool_name":"Bash"}' ;;
  *) title="Changelog by date"
     post UserPromptSubmit '{"session_id":"shot-6","prompt":"Sort the changelog entries by date, newest first"}'
     say "> Sort the changelog entries by date, newest first" "" \
         "→ read CHANGELOG.md" "→ edit CHANGELOG.md  (+40 -40)" "" \
         "Entries are newest first and Unreleased stays pinned on top."
     sleep 0.3; post Stop '{"session_id":"shot-6"}' ;;
esac
"$NEBULA_SHOT_BIN" rename "$title" >/dev/null 2>&1 || true
exec /bin/cat
AGENT
chmod +x "$RUNTIME/agent"
export NEBULA_AGENT_CMD="$RUNTIME/agent"
