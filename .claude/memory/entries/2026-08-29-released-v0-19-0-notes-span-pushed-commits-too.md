# Released v0.19.0 — The RELEASE NOTES Span Pushed Commits, Not Just The Dirty Tree — 2026-08-29

**Asked:** "commit, push, release"
→ refined: skipped — PROMPT DADDY does not rewrite a bare RELEASE SKILL trigger.

**Did:** Cut **v0.19.0** by the RELEASE SKILL recipe. Preflight was the easy branch of the SHARED
CHECKOUT gotcha — `git rev-list --left-right --count HEAD...origin/main` returned `0 0`, so a plain
`cp` of the nine modified plus four untracked files into the RELEASE WORKTREE
(`git worktree add -b release-v0.19.0 <scratch>/release origin/main`) was safe; proved it with equal
`git diff HEAD | shasum` (`b2f2a97…`) on both sides and `cmp` on every untracked file. Green gate
`cargo test --workspace --no-fail-fast` under its own VTARGET (`CARGO_TARGET_DIR=<scratch>/vtarget`): **801
passed, 0 failed**, exit 0, `e2e_tui` 26 and `e2e_pty` included. Three commits: the code
(`config.rs`, `event_loop.rs`, `e2e_tui.rs` — the quit CONFIRM DIALOG and `close_finder_on_open`),
the project-memory scaffolding on its own, then `Cargo.toml` 0.18.0 → 0.19.0 + `Cargo.lock`
(all four members) as `Release v0.19.0`. Pushed `release-v0.19.0:main` then the tag; the matrix went
green on all four targets and `gh release edit v0.19.0 --notes-file` replaced the generated commit
list with benefit-grouped RELEASE NOTES (🛟 Quit on purpose / 🫥 Fewer things in your way). No
⚠️ Heads up group — PROTOCOL VERSION is 34 at both v0.18.0 and v0.19.0.

**Gotchas:**
- **The changelog spans everything since the *tag*, not since `HEAD`.** `HEAD...origin/main` was
  `0 0`, which says the working tree is the only *uncarried* work — it does not say it is the only
  *unreleased* work. Three commits already sat on `origin/main` above `v0.18.0`, one of them
  user-facing (`9462e22` always-on FOCUS TINT). Writing the notes from the dirty diff alone would
  have shipped the tint change silently. `git log --oneline v<last>..HEAD` is the changelog's input;
  `git diff HEAD` is only the carry list.
- **`.claude/`-only commits are not changelog material.** Two of those three commits
  (`b5fc1a2`, `cdfbdf7`) touched only the MEMORY LOG, TERMS.md, hooks and the Makefile — read each
  commit's `--stat` to sort product from scaffolding before drafting groups.
- **The GUARD HOOK caught the zsh `for f in $(…)` split** on the first copy attempt, exactly as its
  rule promises; the `{ …; …; } | while IFS= read -r f` form is what the RELEASE SKILL's file-by-file
  carry actually needs, since it feeds two commands' output into one loop.
- **The shared tree is now 3 behind `origin/main` and still dirty-but-identical** — its `crates/`
  files match `v0.19.0` byte for byte and only `Cargo.toml` differs (the bump lives on the branch).
  Left it for the user per the SHARED CHECKOUT rule rather than resetting it.
