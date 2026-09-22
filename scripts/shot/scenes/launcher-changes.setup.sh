# The launcher-grid scene with uncommitted work left behind by the stand-in agent — three files
# from launch 1, one from launch 2 — so the cards carry their checkout's changed-file count
# behind the branch. The box aims every launch at the checkout under the cursor, so all three
# land in main and all three cards say `main +4 files`.
. "$HERE/scenes/launcher.setup.sh"
mv "$RUNTIME/agent" "$RUNTIME/agent-inner"
cat > "$RUNTIME/agent" <<'AGENT'
#!/bin/sh
n=$(( $(cat "$NEBULA_SHOT_COUNTER" 2>/dev/null || echo 0) + 1 ))
case "$n" in
  1) printf 'a\n' > session.rs; printf 'b\n' > cookie.rs; printf 'c\n' > redirect.rs ;;
  2) printf 'x\n' > CHANGELOG.md ;;
esac
exec "$(dirname "$0")/agent-inner"
AGENT
chmod +x "$RUNTIME/agent"
