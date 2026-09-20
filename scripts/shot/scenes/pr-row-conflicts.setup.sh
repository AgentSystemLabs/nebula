# The stand-in gh answers from fixtures in which GitHub says two of the open pull requests cannot
# merge: #42's branch conflicts with main (`mergeable: CONFLICTING`) and #37 has a failing check in
# its `statusCheckRollup`, while #39 is a plain draft. Both rows of the PROJECT OPEN PRS GROUP go
# red, badged `conflicts` and `failing`; `gh pr view` on the wheel-one-line branch finds #37, so the
# checkout's Sessions panel PR ROW is red too, and the PR PREVIEW says `checks failing` beside the
# state. The PREWARM POOL is off so that panel holds nothing but the PR ROW.
FIXTURES="$WORK/fixtures"; mkdir -p "$FIXTURES"
cp "$HERE/fixtures/pr-39.json" "$HERE/fixtures/pr-view-feature-x.json" "$HERE/fixtures/user" "$FIXTURES/"
cat > "$FIXTURES/pr-list.json" <<'JSON'
[
  {"number": 42, "title": "Attach links to worktrees", "url": "https://github.com/AgentSystemLabs/nebula/pull/42", "isDraft": false, "headRefName": "attach-links",
   "mergeable": "CONFLICTING", "statusCheckRollup": [{"__typename": "CheckRun", "name": "ci", "status": "COMPLETED", "conclusion": "SUCCESS"}]},
  {"number": 39, "title": "Still cooking: a draft the list must sink", "url": "https://github.com/AgentSystemLabs/nebula/pull/39", "isDraft": true, "headRefName": "feature-x",
   "mergeable": "MERGEABLE", "statusCheckRollup": []},
  {"number": 37, "title": "Wheel scroll steps one line a notch", "url": "https://github.com/AgentSystemLabs/nebula/pull/37", "isDraft": false, "headRefName": "wheel-one-line",
   "mergeable": "MERGEABLE", "statusCheckRollup": [{"__typename": "CheckRun", "name": "ci", "status": "COMPLETED", "conclusion": "FAILURE"}]}
]
JSON
cat > "$FIXTURES/pr-view-wheel-one-line.json" <<'JSON'
{"number": 37, "url": "https://github.com/AgentSystemLabs/nebula/pull/37", "title": "Wheel scroll steps one line a notch", "state": "OPEN", "isDraft": false,
 "mergeable": "MERGEABLE", "statusCheckRollup": [{"__typename": "CheckRun", "name": "ci", "status": "COMPLETED", "conclusion": "FAILURE"}],
 "comments": [], "reviews": []}
JSON
cat > "$FIXTURES/pr-37.json" <<'JSON'
{"number": 37, "url": "https://github.com/AgentSystemLabs/nebula/pull/37", "title": "Wheel scroll steps one line a notch", "state": "OPEN", "isDraft": false,
 "mergeable": "MERGEABLE", "statusCheckRollup": [{"__typename": "CheckRun", "name": "ci", "status": "COMPLETED", "conclusion": "FAILURE"}],
 "author": {"login": "webdevcody"}, "baseRefName": "main", "headRefName": "wheel-one-line",
 "body": "One notch of the wheel scrolls one line, the way every other list in the app does.",
 "additions": 12, "deletions": 4, "changedFiles": 1, "comments": [], "reviews": []}
JSON
cat > "$FIXTURES/pr-42.json" <<'JSON'
{"number": 42, "url": "https://github.com/AgentSystemLabs/nebula/pull/42", "title": "Attach links to worktrees", "state": "OPEN", "isDraft": false,
 "mergeable": "CONFLICTING", "statusCheckRollup": [{"__typename": "CheckRun", "name": "ci", "status": "COMPLETED", "conclusion": "SUCCESS"}],
 "author": {"login": "shot-user"}, "baseRefName": "main", "headRefName": "attach-links", "additions": 188, "deletions": 23, "changedFiles": 5,
 "body": "A worktree can now carry saved links as rows of the SESSIONS PANEL's PULL REQUESTS group.",
 "comments": [], "reviews": []}
JSON
export NEBULA_GH_FIXTURES="$FIXTURES"
mkdir -p "$WORK/data"
cat > "$WORK/data/config.json" <<'JSON'
{"prewarm_agents": false, "prewarm_sessions": false}
JSON
