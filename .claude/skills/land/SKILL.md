---
name: land
description: "Land finished work on main with the git and gh chores and nothing else — MAKE CI, a branch, one commit, a push, a pull request, CLAUDE REVIEW's comments answered when asked to babysit, a merge commit, the checkout fast-forwarded back — replacing PROMPT DADDY, NEBULA-MEMORY and PROJECT TERMS for that prompt. Use when the user says \"commit push and merge\", \"commit and push\", \"land this\", \"make pr\", \"open a PR and merge it\", \"merge the pr\", or \"fix conflicts on the pr then fix comments, babysit until all passing then merge\"."
user-invocable: true
---

A landing chore is seven commands. On 2026-09-04 a three-word "commit push and merge" took 17 minutes
because the SELF-IMPROVING LOOP ran around it, and the user asked why. This skill *is* the refinement:
no PROMPT DADDY rewrite, no MEMORY LOG entry unless something new bit you, no PROJECT TERMS pass. The
reply is OUTPUT DOCTOR's short form — OVERVIEW and NEXT STEPS.

## 1. Read the situation — three commands, then decide the shape

```bash
git status --short && git branch --show-current && git rev-list --left-right --count HEAD...origin/main
gh pr view --json number,state,url,mergeStateStatus,headRefName 2>/dev/null   # a PR on this branch?
git diff --stat origin/main -- <the dirty paths>                                # is every hunk yours?
```

- **Dirty SHARED CHECKOUT on `main`** ("commit push and merge") → shape A: branch, commit, push, PR,
  merge, fast-forward back. Only the paths the task touched — `git add <paths>`, never `-A`: other
  sessions' hunks are in this tree too (`.claude/MEMORY.md`, `TERMS.md`, `gotchas.md` are usually
  theirs as well as yours; take the file if your lines are in it, they ride along).
- **A branch with an open PR** (a PR SESSION, "fix conflicts … babysit … merge") → shape B: bring
  `origin/main` in, answer the comments, wait for the check, merge.
- **"make pr"** → shape A up to the pull request, then stop.

## 2. The gate — before anything leaves the machine

`make ci` (or `make memory-check recall-eval terms-check` when no crate changed), logged to the
scratchpad — the GUARD HOOK blocks `cargo … | tail`. Red and not yours (`git stash` is blocked; prove it
with `git diff origin/main -- <file>`) → say so and stop; do not push over a red gate.

## 3. Commit and push

```bash
git switch -c <kebab-branch>                      # from main; a worktree branch is already on one
git add <explicit paths>
git commit -F <scratchpad>/msg                    # never -m: backticks in "…" are command-substituted
git push -u origin <branch>
```

Subject in the repo's voice — what a user now gets, not what the diff did (`git log --oneline -10`
shows the register). Body: two or three lines, then the attribution trailer the harness gave you.

## 4. The pull request

```bash
gh pr create --title "<subject>" --body-file <scratchpad>/pr-body.md   # a short body: what, why, gate
```

The PR DESCRIPTION SKILL only when the user asks for a description; `--draft` when the gate did not
run. `gh auth status` first — `webdevcody` is the admin account, `codyseibert` is read-only.

## 5. Shape B: conflicts, comments, checks

- **Conflicts.** Read the set without touching a tree: `git merge-tree --write-tree --name-only
  origin/main <branch>`. Then, in the branch's *own* worktree, `git merge --no-ff --no-commit
  origin/main`. A squash of the SHARED CHECKOUT often pre-landed the branch's own commits: when every
  `+` line of `git diff <merge-base> <commit> -- <file>` is already in `git show origin/main:<file>`,
  the resolution is main's file plus only the later commits' hunks (`git checkout origin/main --
  <paths>` when nothing is left). Run the Markdown gates after; `cargo check --workspace` when a crate
  differs from `origin/main`. Commit the merge.
- **Comments.** `gh api repos/$R/pulls/$N/comments --paginate --jq '.[] | "\(.id) \(.path):\(.line //
  .original_line) \(.body)"'` — CLAUDE REVIEW's inline findings. Fix each in the code, or reply with
  `gh api -X POST repos/$R/pulls/$N/comments/<id>/replies -F body=@<file>`; push.
- **Checks.** `gh pr checks $N --watch --interval 10` returns when `claude-review` finishes (~50 s
  after a push; exit 8 while pending; "no checks reported" for a few seconds right after the push is
  not a failure). It is the only check, and it posts nothing when it has no new findings — the pass
  signal is no comment row with `created_at` after your push. "Babysit" means repeat until that.

## 6. Merge, and bring the checkout back

```bash
gh pr merge $N --merge                                       # a merge commit, as PRs #20–#29
gh pr view $N --json state,mergeCommit                       # silence from merge is success; this proves it
git switch main && git merge --ff-only origin/main && git branch -d <branch>
```

`main` has no branch protection: `--merge` lands at once, CLAUDE REVIEW pending or not — wait for it
only when the user said "babysit" or "until passing". The fast-forward brings two commits, the merge
and the PR ARCHIVE's own "Archive merged PR(s) N". A worktree branch stays in its worktree; the SHARED
CHECKOUT is left where it was unless the user said to move it.

## 7. Reply, and record only what bit you

OVERVIEW: the PR URL, the merge commit, where the tree now stands, what the gate ran. NEXT STEPS: what
is theirs, or "Nothing — this is done." If something surprised you — a new `gh` behaviour, a conflict
shape not listed above — run the NEBULA-MEMORY SKILL for that alone; a clean landing records nothing.
