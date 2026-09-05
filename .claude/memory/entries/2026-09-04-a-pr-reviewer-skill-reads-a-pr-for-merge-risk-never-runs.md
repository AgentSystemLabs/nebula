# A `pr-reviewer` Skill Reads A PR For Merge Risk, Then Performance, Then Fit — And Never Runs Anything — 2026-09-04

**Asked:** "add a pr reviewer skill which reviews a pr, told to never run anything, leaves a description
focusing most on security or risk of this merge on a production system, performance second, and if the
code matches the existing patterns of the code base"
→ refined: Add a `pr-reviewer` skill (`.claude/skills/pr-reviewer/SKILL.md`, user-invocable, sibling of
the pr-description skill) that reviews one pull request — the number or URL I give, else the current
branch's PR — by reading only: it never builds, tests, executes, installs or mutates anything (assuming
read-only `gh pr view` / `gh pr diff` / file reads to fetch the diff, and `gh pr checks` to read CI rather
than run it). It writes a review in three sections weighted in this order — security and the risk of
merging into a production system first, performance second, conformance to this codebase's existing
patterns third — and leaves it on the PR via `gh pr review --comment --body-file` (assuming "leaves a
description" means a posted review comment; never approve or request changes). (no questions asked)

**Did:** New `.claude/skills/pr-reviewer/SKILL.md` (241 lines, `user-invocable: true`; the harness listed
it as invocable the turn after the file existed). The body: a "read, never run" contract with an allow
list (`gh pr view/diff/checks`, `gh api repos/…/pulls/N/comments` for the inline comments, `git fetch
origin pull/N/head` + `git show <headRefOid>:<path>`, `git show origin/<base>:<path>`, `git grep`) and a
forbid list (any `cargo` or `make`, the `nebula` binary, INSTALL.SH, interpreters on PR files,
`gh pr checkout` / `git checkout` / merge / stash / worktree, `--approve` / `--request-changes` /
`gh pr merge` / `gh pr edit`); eight steps — resolve + fetch into the scratchpad, MEMORY LOG / STANDING
GOTCHAS / PR ARCHIVE lookup for the touched files, the diff in risk order (`.github/`, INSTALL.SH,
`Cargo.*`, `server.rs`, `hooks/`, `registry.rs`, `store.rs`, `protocol.rs`, …), a security checklist
built from the 2026-08-30 walkthrough (DAEMON SOCKET, HOOK RECEIVER / BEARER TOKEN, spawn argv and
`--append-system-prompt`, MANAGED HOOKS writes into `~/.claude` and friends, NEBULA BROWSER / NEBULA
TUNNEL / NEBULA SSH / NEBULA UPGRADE, secrets in argv, MIGRATION and PROTOCOL VERSION as production
impact, supply chain, scope drift), a performance section keyed to the hot paths (draw loop, VENDORED
VT100 byte path, SCROLLBACK RING replay, WORKTREE SYNC, GIT POLL, blocking calls on the tokio runtime), a
fit section keyed to the written patterns (KEEP MODULES SMALL, `nebula_core::env`, `with_default_config`,
PROTOCOL VERSION bump, dep-light, tests beside the change), a fixed review template (Verdict 🔴/🟡/🟢 ·
Basis · 🔒 · ⚡ · 🧩 · 📐 Scope · ❓ Unsettled by reading · footer), and `gh pr review --comment
--body-file` as the single write. Rejected: a SCRATCH WORKTREE for the PR (the 2026-08-28 SECURITY REVIEW
recipe) — `gh pr diff` plus `git show <sha>:<path>` needs no checkout, so nothing of the PR ever reaches
a working tree. Gate: frontmatter parsed (`name` / `description` / `user-invocable`, no `---` in the
description); step 1's fetch block dry-run read-only against PR #26 (15 files, +1419/−42) — every
command exit 0, nothing posted. No crate touched. PROJECT TERMS: SECURITY REVIEW promoted (second sighting: this
entry), PR REVIEWER SKILL ledgered, MAKE CI row brought up to the Makefile; `terms-check`'s
`merge? SECURITY REVIEW → SCRATCH WORKTREE` declined — the user can mean the skill without the worktree.

**Gotchas:**
- **Nothing on GitHub builds or tests a PR in this repo.** `gh pr checks` lists `claude-review` alone:
  `claude-code-review.yml` is the only `pull_request` workflow, `release.yml` runs on `v*` tags and
  `workflow_dispatch`, `pr-archive.yml` on close. A green PR check means no `cargo`, no clippy and no
  tests ran anywhere — a review has to say the test gate has not run, not read "pass" as green; MAKE CI
  on a developer's box is the only gate there is.
- `gh pr checks` exits 8 while any check is pending (0 once all passed) — `|| true` it when capturing
  the output, or a `&&` chain dies on a PR whose review job is still running.
- `FETCH_HEAD` in the SHARED CHECKOUT is one file that every session's fetch overwrites — after
  `git fetch origin pull/N/head`, read the PR's files as `git show <headRefOid>:<path>` (the SHA from
  `gh pr view --json headRefOid`), never as `FETCH_HEAD:<path>`.
- `gh pr review --comment` posts on your own PR; GitHub refuses `--approve` and `--request-changes`
  there — a comment-only review is the one shape that works for every author.
- A new `.claude/skills/<name>/SKILL.md` is registered in the running session as soon as the file
  exists — the harness listed `pr-reviewer` as invocable on the very next turn, no restart; the same
  liveness the GUARD HOOK's `settings.json` rule and a rewritten SKILL.md already showed.
