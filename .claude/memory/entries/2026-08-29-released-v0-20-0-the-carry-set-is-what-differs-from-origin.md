# Released v0.20.0 — The CARRY SET Is What Differs From `origin/main`, Not What `git status` Lists — 2026-08-29

**Asked:** "commit push and release"
→ refined: skipped — PROMPT DADDY does not rewrite a bare RELEASE SKILL trigger.

**Did:** Cut **v0.20.0** (the QUICK PROMPT and its `Tab` / `⇧Tab` launch retargeting) by the RELEASE
SKILL recipe. Preflight hit the `0 N` branch of the SHARED CHECKOUT gotcha —
`git rev-list --left-right --count HEAD...origin/main` returned `0 4`, so the dirty tree straddled
released and unreleased work. Instead of `git diff HEAD`, built the CARRY SET by `cmp`ing every path
`git status` listed against the RELEASE WORKTREE's copy
(`git worktree add -b release-v0.20.0 <scratch>/release origin/main`): 11 modified + 3 new
(`quick_prompt.rs`, two entry files) differed and were carried, 4 were byte-identical to `origin/main`
(carried in by `71d1f31` / `15bda3c` during v0.19.0) and `.claude/rules/rust-modules.md` was already
*tracked* there. Green gate `cargo test --workspace --no-fail-fast` under its own VTARGET: **817
passed, 0 failed**, cargo exit 0. Three commits — code + README, the project-memory scaffolding, then
`Cargo.toml` 0.19.0 → 0.20.0 + `Cargo.lock` (all four members) as `Release v0.20.0`. Pushed
`release-v0.20.0:main` behind the pinned-SHA guard (`aa6731d`) then the tag; the matrix went green on
all four targets and `gh release edit v0.20.0 --notes-file` replaced the generated commit list with
benefit-grouped RELEASE NOTES (a single 🚀 Launch faster group — the release is one feature). No
⚠️ Heads up: PROTOCOL VERSION stays 34. Also turned the twice-hit `| tail` trap into the
`piped-cargo-hides-its-exit-code` rule in the GUARD HOOK (`.claude/hooks/guard.py`) and deleted the
standing gotcha it now enforces.

**Gotchas:**
- **`git status` overstates the CARRY SET whenever the SHARED CHECKOUT is behind `origin/main`.** At
  `0 4`, a third of the dirty paths were work already merged during the *previous* release, and one
  `??` path (`.claude/rules/rust-modules.md`) was not untracked at all — it is tracked on
  `origin/main` and only looks new because local HEAD predates the commit that added it. `cmp` each
  listed path against the RELEASE WORKTREE's copy first: what is identical is noise in the release
  diff, and the untracked-looking one reports a cheerful "OK" while copying nothing.
- **The `| tail` exit-code trap bit a second time**, so it became a GUARD HOOK rule — and the rule
  written from the sibling rules' `[^|;&\n]*` idiom silently *allowed* the very command it was written
  for, because `cargo test … 2>&1 | tail` contains `&` and the scan stopped there. Use
  `(?:[^|;&\n]|&(?!&))*`, and probe a new rule against the trap **and** each remedy before trusting
  it; a guard that never fires looks exactly like a guard that works.
- **RELEASE NOTES facts come from the code, never from an entry's REFINED PROMPT.** The QUICK PROMPT
  entry's refined prompt promises the `Tab` picker has "TYPE-AHEAD and all"; `grep -rn "type_ahead"
  crates/` returns nothing. A REFINED PROMPT records what was *asked for*, which is not what shipped —
  it is the wrong source for a changelog claim.
- **A release entry cannot rank its own prompt, and that is by design of the corpus, not a defect.**
  `recall_eval.py --entry` puts this one at `rank > 5` for "commit push and release" — and the v0.19.0
  entry at `rank > 5` for "commit, push, release" — because ten release entries share those three
  words; `make recall-eval` still passes (aggregate 88% top-5, curated 19/19). What actually reaches
  the next release session is the **RELEASE SKILL group of the STANDING GOTCHAS**, which the hook
  injects in full. Put a release lesson there, not only in the entry, or it will not be read again.
