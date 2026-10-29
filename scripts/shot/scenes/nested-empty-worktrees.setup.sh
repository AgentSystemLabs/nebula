# The NESTED layout with old, empty worktrees under the threads: the nested-layout scene's project
# plus four linked checkouts nothing has ever run in, as Claude subagents and scratch work leave
# behind. With Show all worktrees on (the default) each is a row of its own: its branch, dim, and the
# key hint only on the row under the cursor.
. "$HERE/scenes/nested-layout.setup.sh"
for b in ui-map-tab ui-main-menu remove-schematics recipes-redesign; do
  git -C "$WORK/orbit-api" worktree add -q -b "$b" "$WORK/orbit-api-worktrees/$b" main
done
