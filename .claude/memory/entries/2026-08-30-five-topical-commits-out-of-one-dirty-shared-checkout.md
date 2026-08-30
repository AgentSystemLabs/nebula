# Five Topical Commits Out Of One Dirty SHARED CHECKOUT — 2026-08-30

**Asked:** "commit nd oush"
→ refined: "Commit everything currently in the SHARED CHECKOUT and push to origin/main. Split it into
five topical commits — the README→docs split, the PR ARCHIVE GitHub Action, OVERLAY CLOSE, the
`event_loop.rs` wheel/attach fixes, and the MEMORY LOG + TERMS updates — each carrying its own MEMORY
LOG entry. Run `make ci` first; if it's red, report it and stop before pushing rather than committing
over someone else's broken edit. Don't change any code — only stage, commit and push what is already
there." (asked: commit shape → split by topic, per the option's preview, which put all five MEMORY LOG
entries in one final commit rather than one per topic; gate → run `make ci` first)

**Did:** `make ci` green on the dirty tree first, then five commits pushed as `e1d8874..997dd66`. Four
split cleanly by path; the fifth pile did not, because `crates/nebula-tui/src/event_loop.rs` carried
three sessions' work at once — 36 hunks of OVERLAY CLOSE plus 6 of the WHEEL SCROLL / ATTACH fixes.

Split it without `git add -p` (the harness has no interactive git): `git diff <file>` to a patch, the
6 wheel/attach hunks (`@@ -46`, `@@ -5174`, `@@ -5196`, `@@ -12280`, `@@ -12318`, `@@ -12985`) cut out
by `sed -n` line ranges onto a copy of the 4-line diff header, `git apply -R --check` then `git apply
-R` that patch against the working tree to get the OVERLAY-CLOSE-only intermediate, commit it, then
`cp` the saved final file back for the next commit. `cargo check -p nebula-tui --all-targets` on the
intermediate (exit 0) before committing it.

Attribution came from reading hunks, not guessing: `.claude/memory/terms_check.py`'s new `docs/`
corpus scan belongs to the README split, and `AGENTS.md`'s one added table row belongs to the PR
ARCHIVE.

**Gotchas:**
- `make ci` proves only the *final* tree. Every intermediate commit in a split is unproven — `cargo
  check --all-targets` each one or the history is not bisectable.
- `.claude/memory/gotchas.md` was already at exactly its 300-line cap, so adding this task's line
  meant folding two existing SHARED CHECKOUT lines (the `git diff <commit>` and `git stash create`
  untracked-file traps, which say the same thing) into one first.
- The GUARD HOOK blocks `cargo check … | tail`; redirect to a log and `echo $?` instead. Already a
  rule, so nothing to re-log.
