# The stand-in gh answers from a copy of the stock fixtures plus one more: `gh pr view` on the
# wheel-one-line branch finds #37, open and not a draft, so the checkout's Sessions panel PR ROW
# wears the `ready` badge. The PREWARM POOL is off so that panel holds nothing but the PR ROW.
FIXTURES="$WORK/fixtures"; mkdir -p "$FIXTURES"
cp "$HERE/fixtures/pr-list.json" "$HERE/fixtures/pr-39.json" "$HERE/fixtures/pr-view-feature-x.json" "$HERE/fixtures/user" "$FIXTURES/"
cat > "$FIXTURES/pr-view-wheel-one-line.json" <<'JSON'
{"number": 37, "url": "https://github.com/AgentSystemLabs/nebula/pull/37", "title": "Wheel scroll steps one line a notch", "state": "OPEN", "isDraft": false,
 "comments": [], "reviews": []}
JSON
cat > "$FIXTURES/pr-37.json" <<'JSON'
{"number": 37, "url": "https://github.com/AgentSystemLabs/nebula/pull/37", "title": "Wheel scroll steps one line a notch", "state": "OPEN", "isDraft": false,
 "author": {"login": "webdevcody"}, "baseRefName": "main", "headRefName": "wheel-one-line",
 "body": "One notch of the wheel scrolls one line, the way every other list in the app does.",
 "additions": 12, "deletions": 4, "changedFiles": 1, "comments": [], "reviews": []}
JSON
export NEBULA_GH_FIXTURES="$FIXTURES"
mkdir -p "$WORK/data"
cat > "$WORK/data/config.json" <<'JSON'
{"prewarm_agents": false, "prewarm_sessions": false}
JSON
