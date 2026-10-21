# The DELETE CHECK: feature-x holds a commit main lacks, an uncommitted file, and a process
# working in it from outside any nebula session (a `sleep` standing in for an agent's build).
# Its band reads "nothing running" all the same; `d` on it shows what the check found.
FX="$WORK/demo-worktrees/feature-x"
git -C "$FX" -c user.name=shot -c user.email=shot@example.invalid commit -q --allow-empty -m "wip"
printf 'half done\n' > "$FX/notes.md"
(cd "$FX" && exec sleep 90) &
