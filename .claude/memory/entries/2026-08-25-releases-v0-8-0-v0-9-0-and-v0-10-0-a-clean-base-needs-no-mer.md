# Releases v0.8.0, v0.9.0 And v0.10.0 — A Clean Base Needs No Merge — 2026-08-25

**Asked:** "ok pull in latest from origin/main then and merge into this work them commit push and make
the next release"

**Did:** Nothing to pull — local `main` already tracked `origin/main` at `8beee5a` (the shared tree was
reconciled between releases), so no merge was needed and the release skill's plain recipe applied:
worktree cut from `origin/main`, the six dirty files copied in by content, `274ece8` (feature), `db718af`
(memory), `199011c` (bump to 0.8.0, tagged `v0.8.0`). 624 tests green, all 4 matrix targets built, notes
rewritten. Shipped the draggable Workspaces column, the ttyd `fontSize` refit, and the `make dev` /
`make browser` isolated dev instance — each already has its own entry above.

**Then v0.9.0**, asked as "commit and push and do another release" — same clean path, same recipe: base
read `0 0`, seven dirty files copied in, `2730e9f` (feature), `7ddee41` (memory), `12554d1` (bump to
0.9.0, tagged `v0.9.0`). 629 tests green, all 4 matrix targets built, notes rewritten. Shipped the
persistent Workspaces column and the cross-workspace `/` palette (entry above).

**Then v0.10.0** (2026-08-26), asked as "commit and push and release" — same recipe at a much larger
scale: base `0 0`, nineteen dirty files (~3.7k lines, six finished tasks bundled in one tree) copied in,
`78a1714` (feature), `249668e` (memory), `fd45d42` (bump to 0.10.0, tagged `v0.10.0`). 628 tests green,
all 4 matrix targets built, notes rewritten. Shipped `nebula worktree`, settings `R` reset, settings
tab focus, snapshot re-attach, divider removal, and the default-workspace splash gate (entries above).

**Gotchas:**
- Nothing bit us. Worth recording only as the contrast to [Release v0.7.0]: the copy-files-into-a-worktree
  recipe is safe **exactly when** `git rev-list --left-right --count HEAD...origin/main` reads `0 0`.
  Check that before choosing between copying and merging — it is the one-line test for which of the two
  release paths you are on.
- Two clean releases in a row now. The shared tree is left dirty-but-identical after each one (the
  release worktree does the committing, `main` never moves locally), so the reconcile the user needs is
  `git reset --hard origin/main`, not a merge — `origin/main` is a strict superset of what their working
  copy holds. Say that explicitly; a plain `git pull` on top of those identical-but-uncommitted files
  just stalls.
- With many files, verify the copy in one line instead of eyeballing hunks: `git diff --stat` in the
  shared tree and `git -C "$W" diff --stat` in the worktree must be byte-identical. Leave untracked junk
  (`random.txt`) behind — `git diff --name-only` never lists it, so the loop skips it on its own.
- `cargo clippy --workspace --all-targets` carried 8 warnings at v0.10.0 (unneeded `return`, items after a
  test module, `&` on an auto-deref, `len() == 1`, a complex-type lint). They are **not** a release
  blocker: the only workflow is `.github/workflows/release.yml` and it runs no clippy or fmt step. Report
  them, don't fix them under a "release" ask.
