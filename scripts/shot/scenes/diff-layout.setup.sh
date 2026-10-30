# A committed file with a change well down it, so the DIFF VIEWER has a hunk to lay out split or
# inline, and Ctrl+o's editor opens at the hunk's line rather than at the top of the file.
for i in $(seq 1 60); do printf 'line %02d of the demo file\n' "$i"; done > "$DEMO/server.rs"
git -C "$DEMO" add server.rs
git -C "$DEMO" -c user.name=shot -c user.email=shot@example.invalid commit -q -m "server"
sed -i '' -e 's/^line 31 .*/line 31 — retry with backoff/' -e 's/^line 33 .*/line 33 — log the attempt/' \
  -e '/^line 34 /a\
an added line after 34' "$DEMO/server.rs"
export NEBULA_EDITOR=vim
