# "Commit Push And Merge" From A Clean SHARED CHECKOUT: Branch, PR #29, Merge Commit, Fast-Forward Back — 2026-09-04

**Asked:** "commit push and merge"
→ refined: Land the uncommitted PR ROW / PROJECT OPEN PRS GROUP work in the SHARED CHECKOUT (the ten
modified files, the new `pr_row.rs` and its MEMORY LOG entry) as one commit on a new branch off `main`,
push it, open a pull request and merge it into `main` with a merge commit (`gh pr merge --merge`, as PRs
#26–#28 were), without waiting for CLAUDE REVIEW (assuming so, since `main` has no required checks). Run
MAKE CI first and stop if it is red. Afterwards fast-forward the SHARED CHECKOUT's `main` to `origin/main`
and delete the local branch so the tree ends clean on `main`. (no questions asked)

**Did:** The SHARED CHECKOUT sat on `main` at `origin/main` (`rev-list --left-right --count` → `0 0`) with
only the previous task's dirty files, so "merge" could only mean a pull request. MAKE CI first (900 tests,
MEMORY CHECK, RECALL EVAL, TERMS CHECK, fmt, STRICT clippy all green, logged to the scratchpad since the
GUARD HOOK blocks `cargo … | tail`), then `git switch -c pr-rows-drafts-last`, an explicit-path `git add`
of the twelve files, `git commit -F <file>` (commit `2e10a62`), `git push -u origin`, `gh pr create
--body-file` (PR #29, a short hand-written body — the PR DESCRIPTION SKILL was not run), `gh pr merge 29
--merge` (merge commit `201faee`), then `git switch main && git merge --ff-only origin/main && git branch
-d pr-rows-drafts-last`. Tree ends clean on `main` at `d921c77`, the PR ARCHIVE's own commit for #29. The
recipe for this prompt from a clean-on-`main` SHARED CHECKOUT is those seven commands; nothing about it
needs a WORKTREE.

**Gotchas:**
- `gh pr merge 29 --merge` printed nothing and merged at once while `mergeStateStatus` was `UNSTABLE`
  (CLAUDE REVIEW still pending): `main` has no branch protection (`branches/main/protection` → 404) and no
  rulesets (`rules/branches/main` → `[]`), so "merge" never waits on CLAUDE REVIEW unless the prompt says
  to — its comments, if any, land on an already-merged PR. Silence from `gh pr merge` is success; read
  `gh pr view N --json state,mergeCommit`, not the exit line.
- The PR ARCHIVE workflow's "Archive merged PR(s) 29" commit was on `origin/main` within ~10 s of the merge,
  so the fast-forward pulls two commits (the merge and the archive), not one.
- The installed `~/.cargo/bin/nebula` (PROTOCOL VERSION 37) refused the AUTO-TITLE `nebula rename` against
  the `target/debug` daemon (38); `./target/debug/nebula rename` worked. Standing VERSION SKEW gotcha,
  third hit; its retire pointer (rename over the HOOK RECEIVER) is still unbuilt.
