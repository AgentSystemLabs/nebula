# launcher-changes with a tracked file the checkout the launches land in (api-server's root) has
# already edited — eight of its ten lines gone, two new: each card follows `+5 files` with the
# lines behind it (CARD LINE COUNTS), `+6 -8`, the added in green and the removed in red.
. "$HERE/scenes/launcher-changes.setup.sh"
seq 1 10 > "$WORK/api-server/app.rs"
git -C "$WORK/api-server" add app.rs
git -C "$WORK/api-server" -c user.name=shot -c user.email=shot@example.invalid commit -q -m "app"
printf '1\n2\nthree\nfour\n' > "$WORK/api-server/app.rs"
