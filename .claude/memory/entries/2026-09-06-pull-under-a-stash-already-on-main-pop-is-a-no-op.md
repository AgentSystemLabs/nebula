# Pulled 11 Commits Under A Stash Whose WIP Had Already Shipped: The Pop Was A No-Op The GUARD HOOK Blocked — 2026-09-06

**Asked:** "pull latest from main, fix conflicts, then stash pop"

**Did:** `git fetch` put the SHARED CHECKOUT 0 ahead / 11 behind `origin/main` (`aed589f` → `ec7c200`, PR #30 and the
v0.22.0 release) with no tracked change on disk — only untracked files, four of them byte-identical (`cmp`) to files
the incoming range adds (`transcript.py`, `transcript-lookup/SKILL.md`, two 2026-09-05 entries). Parked those four in the
scratchpad, `git pull --ff-only origin main` went through with no conflict, the ff wrote them back identically;
`.claude/memory/skill-audit/` stays untracked (not on `origin/main`). `stash@{0}` ("WIP on main: aed589f", seven
files: `MEMORY.md`, `gotchas.md`, `TERMS.md`, two entries, `nebula-memory/SKILL.md`, `project-terms/SKILL.md`) turned
out to be the v0.22.0 MEMORY LOG + glossary work that already landed as `1e43b00` and `0abf2ff`: three files are
byte-identical to `origin/main`, every `+` line of the other four is in `git show origin/main:<file>`, every `-` line is
gone. A `git merge-tree` dry run of the pop conflicted only in `.claude/MEMORY.md` and `TERMS.md`, both hunks "HEAD has
two more lines, stash side empty", and `gotchas.md` / `project-terms/SKILL.md` auto-merged to no change — so the pop's
correct resolution is HEAD on both and its net content change is zero. The GUARD HOOK
(`git-stash-on-the-shared-checkout`) blocked `git stash pop`; not bypassed. Tree left clean at `ec7c200`,
`stash@{0}` left for the user to drop. Extended the SHARED CHECKOUT stash-plumbing line in `gotchas.md` with the
dry run (file stays at its 300-line cap).

**Gotchas:**
- Dry-run a stash pop without touching the SHARED CHECKOUT: `git merge-tree --write-tree --merge-base='stash@{0}^1'
  HEAD 'stash@{0}'` (git ≥ 2.38) prints the merged tree id first, then `<mode> <blob> <stage>\t<path>` rows and
  `CONFLICT (content)` lines, exit 1 on conflict; `git diff --stat HEAD <tree-id>` that shows only `+3` per
  conflicted file is just the markers — the stash side is a subset of HEAD and the pop is a no-op. Prove it the other
  way too: every `+` line of `git diff 'stash@{0}^1' 'stash@{0}' -- <file>` already in `git show origin/main:<file>`,
  every `-` line absent (`grep -qxF`). A stash titled "WIP on main" from before a release is routinely this.
- The GUARD HOOK's `GIT_STASH_MUTATES` blocks every `git stash` verb but `list`/`show`/`create`/`store` — `pop`,
  `apply` and `drop` alike — with no allowance for an explicit "then stash pop" ask. Prove containment, dry-run the
  merge, report, and hand `git stash drop 'stash@{0}'` back to the user; do not route around it with
  `git cherry-pick -n -m1 stash@{0}` or `git reflog delete`.
