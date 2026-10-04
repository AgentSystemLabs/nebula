# The PANELS over the FOLLOW-UP scene's demo: two sessions that finish quietly, so the pills read as
# idle agents waiting for their next turn and nothing sweeps over the box.
. "$HERE/scenes/follow-up.setup.sh"
printf '{"theme":"%s","layout":"panels","prewarm_agents":false,"prewarm_sessions":false}\n' \
  "${NEBULA_SHOT_THEME:-default}" > "$WORK/data/config.json"
