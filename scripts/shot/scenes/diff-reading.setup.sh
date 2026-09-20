# The DIFF VIEWER up ahead of its file list: `git status` is held (scripts/shot/bin/git), which also
# keeps the changed-files badge from ever having a list for `g` to open on — so the shot is the
# first-open state, `reading changes…`, that a cold `git status` on a large checkout shows.
export NEBULA_SHOT_SLOW_GIT_SECS=8 NEBULA_SHOT_SLOW_GIT_MATCH=status
printf 'work in progress\n' > "$DEMO/notes.txt"
