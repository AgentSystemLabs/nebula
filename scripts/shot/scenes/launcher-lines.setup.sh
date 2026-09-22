# launcher-changes with CARD LINE COUNTS on, and a tracked file the checkout the launches land in
# (api-server's root) has already edited — eight of its ten lines gone, two new: each card follows
# `+5 files` with the lines behind it, `+6 -8`, the added in green and the removed in red.
. "$HERE/scenes/launcher-changes.setup.sh"
python3 - "$WORK/data/config.json" <<'PY'
import json, os, sys
path = sys.argv[1]
cfg = json.load(open(path)) if os.path.exists(path) else {}
cfg["card_line_changes"] = True
json.dump(cfg, open(path, "w"))
PY
seq 1 10 > "$WORK/api-server/app.rs"
git -C "$WORK/api-server" add app.rs
git -C "$WORK/api-server" -c user.name=shot -c user.email=shot@example.invalid commit -q -m "app"
printf '1\n2\nthree\nfour\n' > "$WORK/api-server/app.rs"
