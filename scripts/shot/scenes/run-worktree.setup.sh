# A project that says how it is run: a committed .nebula.json whose `run` is a stand-in dev server
# (dev-server.sh: a banner, then a request line every two seconds, ending on its own after two
# minutes so a stray run never outlives the harness) and whose `open` names its page. SHELL is
# /bin/sh so the run's pane shows the run rather than this machine's login profile, and the keys wait
# long enough for the run's first lines to land.
cat > "$DEMO/dev-server.sh" <<'SH'
printf '\n  demo dev server\n\n  Local:   http://localhost:3000/\n\n'
for i in $(seq 60); do sleep 2; echo "  GET / 200 in ${i}ms"; done
SH
cat > "$DEMO/.nebula.json" <<'JSON'
{
  "run": "sh dev-server.sh",
  "open": "open http://localhost:3000"
}
JSON
git -C "$DEMO" add .nebula.json dev-server.sh
git -C "$DEMO" -c user.name=shot -c user.email=shot@example.invalid commit -q -m "how the demo runs"
export SHELL=/bin/sh
export SHOT_KEY_SECS=1.5
