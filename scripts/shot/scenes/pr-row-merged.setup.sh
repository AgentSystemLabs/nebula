# The stand-in gh answers from fixtures/merged: the wheel-one-line branch's pull request has merged
# (it leaves the open list, and `gh pr view` on the branch still answers with it, state MERGED). The
# PREWARM POOL is off so the checkout's Sessions panel holds nothing but its PR ROW.
export NEBULA_GH_FIXTURES="$HERE/fixtures/merged"
mkdir -p "$WORK/data"
cat > "$WORK/data/config.json" <<'JSON'
{"prewarm_agents": false, "prewarm_sessions": false}
JSON
