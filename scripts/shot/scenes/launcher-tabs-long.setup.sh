# launcher-tabs-narrow's projects plus two with long names, so a narrow header has more tabs than room
# and the MORE CHIP has something to stand in for.
. "$HERE/scenes/launcher.setup.sh"
for extra in agentsystemlabs-website orbit-marketing-docs; do
  mkdir -p "$WORK/$extra"
  git -C "$WORK/$extra" init -q -b main
  git -C "$WORK/$extra" -c user.name=shot -c user.email=shot@example.invalid commit -q --allow-empty -m "$extra"
done
INNER_BIN="$BIN"
cat > "$RUNTIME/orbit-long" <<WRAP
#!/bin/sh
if [ "\$1" = add ]; then
  "$REAL_BIN" add "$WORK/agentsystemlabs-website" >/dev/null
  "$REAL_BIN" add "$WORK/orbit-marketing-docs" >/dev/null
fi
exec "$INNER_BIN" "\$@"
WRAP
chmod +x "$RUNTIME/orbit-long"
BIN="$RUNTIME/orbit-long"
