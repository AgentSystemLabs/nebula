# Released v0.22.0 Level With `origin/main`: Nothing To Carry, 38 Commits To Explain — 2026-09-05

**Asked:** "do a release"
→ refined: skipped — PROMPT DADDY does not rewrite a bare RELEASE SKILL trigger.

**Did:** Cut **v0.22.0** from a SHARED CHECKOUT that was, for once, level with `origin/main`
(`git rev-list --left-right --count HEAD...origin/main` = `0 0`). The CARRY SET was ten `.claude/` and
`TERMS.md` scaffolding paths from another session (today's for-in GUARD HOOK entry, the
`transcript-lookup` skill with `.claude/memory/transcript.py`, two SKILL.md clarifications, the CLAUDE
TRANSCRIPT row), every hunk read, `cp`'d into a RELEASE WORKTREE at `$CLAUDE_JOB_DIR/tmp/release`
(branch `release-v0.22.0` off `origin/main`; `git diff HEAD | shasum` equal in both trees, `cmp` on the
three untracked) and committed on their own as `1e43b00`. The release itself was the 38 commits already on
`origin/main` above `v0.21.0` — Pi AGENT KIND, FILE TABS (NEBULA OPEN), CLAUDE TITLE SYNC, a PR SESSION
in the PR head branch's WORKTREE, the UPDATE INDICATOR on the VERSION NAMEPLATE, COMMAND HELP, the
DOCS PAGES, the three OVERLAY exits, PR ROW drafts, HOST TERMINAL mode re-assert, the ATTACH REBIND,
one-line WHEEL SCROLL — PROTOCOL VERSION 34 → 38. Gate under a borrowed idle `vtarget`
(`/private/tmp/claude-501/<slug>/49081389…/scratchpad/vtarget`, untouched 12 h): `make memory-check
recall-eval terms-check` ok, `cargo fmt --all -- --check` clean, `cargo test --workspace` **901 passed,
0 failed**, cargo exit 0 (nebula-tui 602, nebula-daemon 193, E2E PTY 29, E2E TUI 8, help_cli 6; one
orphan daemon on the box). Bump in `Cargo.toml` plus the `nebula 0.22.0` example in `docs/commands.md`,
`cargo check --workspace` rewrote the four `Cargo.lock` pins, commit `e8d372e`; pushed
`release-v0.22.0:main` behind `[ "$(git rev-parse origin/main)" = aed589f9… ]`, tagged `e8d372e`, pushed
the tag; the run was found on the first `gh run list` poll by `headBranch == "v0.22.0"`. RELEASE NOTES
built from `git log v0.21.0..HEAD --no-merges --format='%h %s%n%b' -- . ':!.claude'` plus the Did sections
of 17 entries (`awk '/^\*\*Did:\*\*/{p=1} /^\*\*Gotchas:\*\*/{p=0} p'`), every feature word grepped in
the RELEASE WORKTREE before it went in; six benefit groups, `⚠️ Heads up` for the PROTOCOL VERSION,
`gh release edit --notes-file`. Matrix green on all four targets (run `33978816362`, ~2 min a
build), four `.tar.gz` assets on the release. The GUARD HOOK fired twice (`for-in-unquoted-command-substitution` on a
`cmp` loop, `piped-cargo-hides-its-exit-code` on `cargo check | tail`), both complied with. The SHARED
CHECKOUT is left 2 behind and dirty-but-identical: `git diff --stat origin/main -- <7 tracked>` is empty
and `git show origin/main:<p> | cmp -s - <p>` matches the 3 untracked, so `git reset --hard origin/main`
loses nothing — the user's to run.

**Gotchas:**
- **The scratchpad can be withdrawn mid-job.** An "Environment update: the scratchpad directory announced
  earlier is no longer available" arrived during preflight of a background job; the RELEASE WORKTREE,
  logs and notes went under `$CLAUDE_JOB_DIR/tmp` (`~/.claude/jobs/<id>/tmp`, cleaned with the job).
  Another session's idle `vtarget` under `/private/tmp/claude-501/<slug>/<session>/scratchpad/` is still
  the right target dir to borrow.
- **The version bump has a second site.** `docs/commands.md` quotes the `--version` output as
  `nebula 0.21.0`; `grep -rn '<old version>' --exclude-dir=target --exclude-dir=.claude .` finds it before
  the release commit. `cli.rs`'s `nebula worktree hotfix --base v0.21.0` is a tag example, not a version,
  and stays.
- **An entry's Did is a snapshot, like its REFINED PROMPT.** The OVERLAY entry said "all fourteen"
  modals; FILE TABS made it fifteen (`every_overlay` count 15) by release time, so the notes say "any
  overlay". The code, not the entry, is the source for a RELEASE NOTES claim — folded into the standing
  RELEASE NOTES line.
