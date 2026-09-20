# The FILE FINDER up ahead of its listing: `git ls-files` is held for a few seconds (scripts/shot/bin/git)
# so the shot catches what a large checkout shows for a moment — the modal open on the keypress, the
# query typed meanwhile kept, `listing files…` where the rows will be.
export NEBULA_SHOT_SLOW_GIT_SECS=8 NEBULA_SHOT_SLOW_GIT_MATCH=ls-files
