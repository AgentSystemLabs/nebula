# Proving A SHARED CHECKOUT Reset Loses Nothing Before Running It — 2026-08-29

**Asked:** "are you sure my main wont get wiped if I reset to origin/main" → "just get my main good
without losing shit"
→ refined: skipped — a direct instruction, and a question answered before it.

**Did:** Reconciled the SHARED CHECKOUT after two releases in a row had left `main` 12 commits behind
with 24 dirty paths. No code changed. Proved the reset was lossless *before* running it rather than
asserting it: `cmp` of every modified path against `git show origin/main:<path>` (12 of 15 byte-
identical), `git cat-file -e origin/main:<path>` for every untracked path (all 9 already committed),
and `comm -23 <(sort -u <local>) <(sort -u <origin copy>)` on the three that differed — 0 local-only
lines in `.claude/MEMORY.md`, 5 in `gotchas.md`, 1 in `TERMS.md`, each one the pre-edit version of a
STANDING GOTCHA or candidate row this session had rewritten, with its replacement grepped for on
origin. Took three backups first, then `git reset --hard origin/main` → `e158595`, 0 dirty, and
MEMORY CHECK / RECALL EVAL / TERMS CHECK all green.

**Gotchas:**
- **`git stash create` snapshots the working tree into a real commit object without touching the tree
  or the index** — the only safe "backup before a destructive op" on a SHARED CHECKOUT, since
  `git stash push` would yank the files out from under whatever other session is editing them. Keep it
  alive with `git update-ref refs/backups/pre-reset-<ts> <sha>`; a bare `stash create` commit is
  unreferenced and eventually gets gc'd. It does **not** include untracked files — tar those separately.
- **"Is anything lost?" is a line-containment question, not a diff question.** `git diff` shows the
  three memory files as heavily changed and tells you nothing about *loss*; `comm -23` over the sorted
  unique lines answers it exactly, and reduced 2694 diff lines to 6 sentences worth reading.
- **A file being older on both sides at once is the normal state after a release from this repo.** The
  three files differed because this session had edited them *further on origin* after carrying them —
  so the local copy was behind, not ahead, even though `git status` calls it modified.

**Corrections:** 1 — the user asked me to stop mid-PROMPT-DADDY on the RELEASE SKILL rewrite and settle
the reset question first; that rewrite is still unstarted.
