# Released v0.21.0 — `cp` What Is Strictly Ahead, Merge What Moved Both Ways — 2026-08-29

**Asked:** "do one more commit push and release"
→ refined: skipped — PROMPT DADDY does not rewrite a bare RELEASE SKILL trigger.

**Did:** Cut **v0.21.0** (QUICK PROMPT FOCUS: `p` no longer enters and locks the TERMINAL PANE) an hour
after v0.20.0, from the same SHARED CHECKOUT, still 8 behind `origin/main`. The CARRY SET split into two
classes this time and that split was the whole job: `crates/nebula-tui/src/{app,config,event_loop}.rs`
and `README.md` were **strictly ahead** of `origin/main` — their `origin/main` blob still equalled
`v0.20.0`'s, so `cp` was correct — while `.claude/MEMORY.md`, `gotchas.md` and `TERMS.md` had moved on
*both* sides (the other session's entry and TERM rows locally, my v0.20.0 scaffolding commit on origin),
so a `cp` would have reverted the CARRY SET row, the GUARD HOOK rule's STANDING GOTCHA deletion and my
index line. Merged those three with `git diff v0.20.0 -- <the three> | git apply --3way`, which left two
keep-both conflicts (my v0.20.0 index line vs their new one; RECALL RE-RUN vs QUICK PROMPT FOCUS in the
CANDIDATES LEDGER) resolved newest-first. Green gate under the reused VTARGET after removing the
finished v0.20.0 RELEASE WORKTREE: **819 passed, 0 failed**, cargo exit 0. Three commits, tag, matrix
green on all four targets, `gh release edit --notes-file`. PROTOCOL VERSION stays 34.

**Gotchas:**
- **`git diff HEAD` is the wrong patch base once a release has already been cut from the same dirty
  tree.** It spans *both* releases, so `git apply --3way` collides with everything the last release
  landed — 5 conflicted files here, including the two code files. `git diff v<lasttag> -- <paths>` is
  the isolating patch: the tag is exactly the base the other session's work was written on top of, so it
  reduced to 2 trivial conflicts. Now folded into the carry STANDING GOTCHA.
- **Decide `cp` vs merge per file, not per tree.** A path is safe to `cp` only when `origin/main`'s blob
  still equals the last tag's (nothing has touched it since the release); the test is two
  `git hash-object` reads, and it is the difference between carrying work and silently reverting a
  pushed commit.
- **`grep -E '^[+-][^+-]'` reads a markdown diff as empty.** Every added list line is `+- **TERM** …`,
  whose second character is `-`, so the filter drops it — I briefly concluded the merged `gotchas.md`
  carried no changes when it carried two. `git diff -U0` and read it.
- **A full SHA reconstructed from a short one never matches.** The push guard aborted on an
  `origin/main` that had not moved because I extended `9c7d9d8` by hand instead of reading
  `git rev-parse origin/main`. The guard did its job; the SHA has to come from git.
- **The RELEASE NOTES-from-code rule paid out on its first release.** The entry's REFINED PROMPT named
  the SETTING `Focus new session`; the shipped label is `Quick prompt focus` (`config.rs:280`). The
  notes say what the code says.
- **Merging another session's memory files pushed `gotchas.md` to 301/300 and I pushed it that way** —
  `make memory-check` failed on `origin/main` for one commit. A merge that carries someone else's new
  gotcha line inherits their cap debt: run `make memory-check` on the merged tree *before* the release
  push, not only after writing your own entry.
