# A PR SESSION on #42 (head `attach-links`, no checkout yet) captured while git is still cutting the
# checkout: the stand-in WORKTREE and SESSION rows. The demo repo has no origin, so the DAEMON's fetch
# fails over to the local branch made here; the slow `git worktree add` (scripts/shot/bin/git) is the
# window the rows are on screen in. No warm spares, so the SESSIONS PANEL holds only the stand-in.
export NEBULA_SHOT_SLOW_GIT_SECS=6
git -C "$DEMO" branch attach-links main
mkdir -p "$WORK/data"
printf '{"prewarm_agents":false,"prewarm_sessions":false}\n' > "$WORK/data/config.json"
