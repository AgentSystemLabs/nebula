# A WORKTREE HOOK that refuses, so deleting a worktree shows the hook warning in the footer.
# Lives under RUNTIME: short enough to read in one footer line, and cleaned up with the run.
mkdir -p "$RUNTIME/hooks"
cat > "$RUNTIME/hooks/worktree-cleanup" <<'HOOK'
#!/bin/sh
# $1 = main repo, $2 = deleted worktree
echo "caddy: no site block for $(basename "$2")" >&2
exit 1
HOOK
chmod +x "$RUNTIME/hooks/worktree-cleanup"
git -C "$DEMO" config nebula.worktreeDeleteHook "$RUNTIME/hooks/worktree-cleanup"
