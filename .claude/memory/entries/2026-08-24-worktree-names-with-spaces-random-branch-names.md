# Worktree Names With Spaces, Random Branch Names — 2026-08-24

**Asked:** "when I create a worktree name, allow a user to type in spaces in the worktree name but you
must convert the spaces to hyphens. also allow a user to just enter on the branch which will pick a
random branch name using three words combined such as yellow-fox-jumps <adj>-<noun>-<verb>"

**Did:** Added `crates/nebula-tui/src/branch_name.rs` for the `<adj>-<noun>-<verb>` generator (the BRANCH NAME GENERATOR); the
worktree name field slugifies spaces to hyphens.
