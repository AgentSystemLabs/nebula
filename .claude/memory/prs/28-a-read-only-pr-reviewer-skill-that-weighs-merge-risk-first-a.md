# PR #28 — A read-only PR reviewer skill that weighs merge risk first and never runs the code

- **URL:** https://github.com/AgentSystemLabs/nebula/pull/28
- **Author:** @webdevcody
- **Merged:** 2026-09-05T02:44:57Z by @webdevcody (`a6b8b74ee6e9`)
- **Opened:** 2026-09-05T01:50:46Z
- **Branch:** `pr-reviewer-skill` → `main`
- **Diff:** +48 −14 across 5 file(s)

## Description

> `/pr-reviewer` reviews a pull request the way a careful colleague would before a production merge: it reads the diff, the description, the CI state, the surrounding code and the MEMORY LOG, then leaves one review comment that weighs security and production-merge risk first, performance second, and fit with the codebase's patterns third. It never builds, tests, executes or checks out the PR — the reviewer is a reader, and whatever reading cannot settle is named for a human to run.
>
> ## Contents
>
> - [🔒 Merge risk first, in nebula's own terms](#merge-risk-first-in-nebulas-own-terms)
> - [🙅 Read, never run](#read-never-run)
> - [📝 One review comment, one fixed shape](#one-review-comment-one-fixed-shape)
> - [📸 Screenshots](#screenshots)
> - [🧭 How a review flows](#how-a-review-flows)
> - [🔧 Technical overview](#technical-overview)
> - [Notes](#notes)
>
> ## 🔒 Merge risk first, in nebula's own terms <a id="merge-risk-first-in-nebulas-own-terms"></a>
>
> - **Security and production risk lead every review.** The checklist is nebula's real attack surface, taken from the 2026-08-30 security walkthrough in the MEMORY LOG: the DAEMON SOCKET as an unauthenticated control plane, the HOOK RECEIVER and its one BEARER TOKEN per boot, every spawn argv and `--append-system-prompt`, MANAGED HOOKS writes into the user's own `~/.claude`, `~/.codex`, `~/.cursor` and `~/.pi`, NEBULA BROWSER and NEBULA TUNNEL, secrets in argv or the DAEMON LOG, supply chain, and scope drift between what the description claims and what the diff touches.
> - **Rollout impact is a finding, not a footnote.** A MIGRATION runs once and irreversibly on every user's SQLITE STORE; a PROTOCOL VERSION bump makes every running DAEMON refuse the new TUI until NEBULA KILL stops every live session. The review states both plainly, with the rollback path or its absence.
> - **Performance is weighed by how often the path runs.** The draw loop, the PTY byte path into the VENDORED VT100, the SCROLLBACK RING replay, the 2 s WORKTREE SYNC, the GIT POLL, and blocking calls on the daemon's async runtime. A cost paid once at launch is a nit; the same cost per frame is a should-fix.
> - **Fit is checked against the written rules, not taste.** KEEP MODULES SMALL, reuse of an existing helper before a re-implementation, a PROTOCOL VERSION bump for a shared type and an appended MIGRATION for a schema change, the deliberately dep-light workspace, tests beside the change, and TERMS in comments and docs.
>
> ## 🙅 Read, never run <a id="read-never-run"></a>
>
> - **An explicit allow list.** `gh pr view`, `gh pr diff`, `gh pr checks`, the inline-comments API, a fetch of `pull/N/head`, `git show` of the PR's files by SHA and of the base's files, and `git grep`. Every one is a read that touches no working tree and executes nothing from the PR.
> - **An explicit forbid list.** Every `cargo` and `make` target — even `cargo check` runs `build.rs` and proc-macros as you, with your `gh` token and ssh keys in reach — the `nebula` binary, INSTALL.SH, interpreters on any file the PR touches, `gh pr checkout` and every `git` command that moves the SHARED CHECKOUT, and every `gh` write except one `--comment` review. Approval stays a human's signature.
> - **A fact every review must state.** Nothing on GitHub builds or tests a PR in this repo — `gh pr checks` lists `claude-review` alone — so the review says the test gate has not run instead of reading "pass" as green. MAKE CI on a developer's box is the only gate there is.
>
> ## 📝 One review comment, one fixed shape <a id="one-review-comment-one-fixed-shape"></a>
>
> A verdict (🔴 Do not merge as-is · 🟡 Merge with care · 🟢 Low risk) and its basis; then security and production risk, performance, fit with the codebase, scope, and an "unsettled by reading" list naming the exact checks a human should run and which result would change the verdict. Every finding carries a `path:line` on the PR's side of the diff, a severity (Blocker, Should fix, Nit), and whether it is *confirmed by reading* or *suspected*. Findings the automated Claude Code Review workflow already posted inline are cited, not repeated. The review is posted with `gh pr review --comment --body-file`, which works on your own PR too; "don't post" keeps it in chat.
>
> ## 📸 Screenshots <a id="screenshots"></a>
>
> There is no screen to shoot: the change is a skill file, and the review it produces lives on GitHub. In its place, the skill's fetch block dry-run read-only against PR #26 — every command exited 0 and nothing was posted:
>
> ```
> $ gh pr view 26 --json …                 → head 69f60d5 · base main · 15 files · +1419/−42
> $ gh pr diff 26                          → 1895 lines
> $ gh pr checks 26                        → claude-review   pass   49s
> $ git fetch origin pull/26/head
> $ git show 69f60d5:<first file> | wc -l  → 176
> ```
>
> ## 🧭 How a review flows <a id="how-a-review-flows"></a>
>
> ```mermaid
> flowchart LR
>   A["gh pr view / diff / checks<br/>+ the inline-comments API"] --> B["Read the diff in risk order<br/>.github → install.sh → Cargo → server.rs → hooks → registry.rs → store.rs → protocol.rs → TUI"]
>   B --> C["MEMORY LOG · STANDING GOTCHAS · PR ARCHIVE<br/>what the repo already knows about these files"]
>   C --> D["🔒 Security & production risk"]
>   D --> E["⚡ Performance"]
>   E --> F["🧩 Fit with the codebase"]
>   F --> G["pr-review.md<br/>verdict · findings · scope · unsettled"]
>   G --> H["gh pr review --comment --body-file"]
>   B -. never .-> X["cargo · make · nebula · install.sh<br/>gh pr checkout · git checkout / merge · --approve"]
>   classDef never stroke:#c0392b,stroke-dasharray: 4 3
>   class X never
> ```
>
> ## 🔧 Technical overview <a id="technical-overview"></a>
>
> - **The skill** is `.claude/skills/pr-reviewer/SKILL.md`, user-invocable, 241 lines: the allow/forbid contract, eight steps (resolve and fetch into the scratchpad, look the touched files up in the MEMORY LOG and PR ARCHIVE, read the diff in risk order, the three weighted sections, the fixed template, the single `--comment` post), and a closing "when the reviewer is tempted to run something" section that routes each temptation to a read or to the unsettled list.
> - **Why no SCRATCH WORKTREE.** The 2026-08-28 SECURITY REVIEW recipe needed one because that skill snapshots the checkout it runs in. `gh pr diff` plus `git show <headRefOid>:<path>` after `git fetch origin pull/N/head` reads both sides with nothing of the PR touching a working tree. The SHA rather than `FETCH_HEAD`, because every session's fetch overwrites that one file in the shared repo.
> - **Why `--comment` only.** GitHub refuses `--approve` and `--request-changes` on the author's own PR, and an agent's approval should not count as a signature anyway. The verdict lives in the body.
> - **The MEMORY LOG carries the task.** Entry `2026-09-04-a-pr-reviewer-skill-reads-a-pr-for-merge-risk-never-runs.md` with 5 gotchas; two standing gotchas added (the PR-CI fact under MAKE CI, the `FETCH_HEAD` race under SHARED CHECKOUT) and two existing pairs merged so the file stays at its 300-line cap; the "a rewritten SKILL.md is live at once" line extended with the new-skill case.
> - **TERMS.md.** SECURITY REVIEW promoted from the Candidates ledger on its second sighting, PR REVIEWER SKILL ledgered, and the MAKE CI row brought up to the Makefile's real `ci` target.
>
> ## Notes <a id="notes"></a>
>
> - **Gate:** `make memory-check`, `make terms-check` and `make recall-eval` are green on this branch. No crate is touched, so `cargo` was not run — nothing here compiles.
> - **Dry run:** the skill's fetch block ran read-only against PR #26 (see Screenshots); no review was posted anywhere.
> - **Try it:** `/pr-reviewer <this PR's number>` from a nebula SESSION on this repo.
> - **Merge state:** branch `pr-reviewer-skill` off `origin/main` at `eb9422e`, no conflicts.
>
> 🤖 Generated with [Claude Code](https://claude.com/claude-code)
>
> https://claude.ai/code/session_01TmuS64UtyRD5H39njusNDe

## Changed files (5)

- `.claude/MEMORY.md` +1 −1
- `.claude/memory/entries/2026-09-04-a-pr-reviewer-skill-reads-a-pr-for-merge-risk-never-runs.md` +25 −0
- `.claude/memory/gotchas.md` +1 −1
- `.claude/skills/pr-reviewer/SKILL.md` +19 −10
- `TERMS.md` +2 −2

## Commits (4)

- `35c1ac70a910` A read-only PR reviewer skill that weighs merge risk first and never … — @webdevcody, @claude
- `08796166eeb0` Log the WORKTREE RELOCATION follow-up: it works live, and uncommitted… — @webdevcody, @claude
- `0913b4097937` Merge origin/main into pr-reviewer-skill — @webdevcody, @claude
- `fbe4a0f962e6` Answer the three Claude Code Review comments on the PR REVIEWER SKILL — @webdevcody, @claude

## Conversation (0)

_(no issue comments)_

## Reviews (6)

### @claude · COMMENTED · 2026-09-05T02:09:50Z

_(empty)_

### @claude · COMMENTED · 2026-09-05T02:09:55Z

_(empty)_

### @claude · COMMENTED · 2026-09-05T02:09:58Z

_(empty)_

### @webdevcody · COMMENTED · 2026-09-05T02:43:28Z

_(empty)_

### @webdevcody · COMMENTED · 2026-09-05T02:43:29Z

_(empty)_

### @webdevcody · COMMENTED · 2026-09-05T02:43:30Z

_(empty)_

## Inline review comments (6)

### `.claude/skills/pr-reviewer/SKILL.md:86` — @claude[bot] · 2026-09-05T02:09:50Z

```diff
@@ -0,0 +1,241 @@
+---
+name: pr-reviewer
+description: "Review a pull request by reading alone — its diff, description, CI state, the surrounding code at base and head, and the MEMORY LOG for the area — and leave one review comment on the PR that weighs, in this order, the security and production-merge risk of the change, its performance cost, and whether it follows the patterns already in this codebase. It never builds, tests, executes, installs or checks out the PR, and never approves or requests changes. Use when the user says \"review this PR\", \"review PR 26\", \"pr review\", \"pr reviewer\", \"is this safe to merge\", \"what would merging this break\", or asks for a risk read on a pull request."
+user-invocable: true
+---
+
+A pull request review here answers one question first: **what happens to the people running nebula
+in production if this merges?** Nebula is a DAEMON that owns every agent PTY on a developer's box,
+speaks to its TUI over an unauthenticated DAEMON SOCKET, installs MANAGED HOOKS into the user's own
+`~/.claude`, `~/.codex`, `~/.cursor` and `~/.pi`, spawns `claude` / `codex` / `cursor-agent` / `pi`
+with a composed argv and system prompt, runs `curl | sh` on remote hosts over NEBULA SSH, and migrates
+a SQLITE STORE on every user's disk at the first launch of a new binary. A wrong line in the wrong
+place is not a bug report; it is every user's sessions gone, or every user's shell running someone
+else's script. So the review weighs security and merge risk first and hardest, performance second, and
+fit with the codebase's patterns third — and says which of the three every finding belongs to.
+
+The second rule shapes everything else: **the reviewer reads; it never runs.** Not the PR's code, not
+its tests, not the build, not the binary, not a script the PR adds. Three reasons, each sufficient on
+its own. A PR — above all one from a fork (`isCrossRepository: true`) — is untrusted input, and even
+`cargo check` executes its `build.rs` and proc-macros as you, with your `gh` token, `~/.claude` and ssh
+keys in reach. The SHARED CHECKOUT belongs to several sessions at once, so a `gh pr checkout`, a merge
+or a stash yanks the tree out from under them. And a review that says "I ran the tests" asserts
+something the reader cannot audit anyway: CI's verdict is the test gate — and on this repo nothing on
+GitHub builds or tests a PR at all (`claude-code-review.yml` is the only PR workflow; `release.yml`
+runs on tags), which is itself a fact the review states. What reading cannot settle goes into the
+review as *unsettled*, named precisely enough that a human can run it. It is never quietly run.
+
+## Read, never run
+
+Allowed — every one of these is a read: it touches no working tree and executes nothing from the PR.
+
+```bash
+gh pr view <n> --json …                          # metadata, description, files, checks, reviews
+gh pr diff <n>                                   # the unified diff
+gh pr checks <n>                                 # CI state as GitHub reports it (exit 8 = pending)
+gh api repos/<owner>/<repo>/pulls/<n>/comments   # inline review comments — not a --json field
+git fetch origin && git fetch origin pull/<n>/head   # refresh origin/<base>; the PR head lands in FETCH_HEAD, no ref, no checkout
+git show <headRefOid>:<path>                     # the PR's whole file (the SHA is stable; FETCH_HEAD is not, other sessions fetch too)
+git show origin/<base>:<path>                    # the same file before the PR
+git log · git grep · grep · cat · sed -n · wc · diff   # over files, never through them
+```
+
+Forbidden, whatever the PR is — when a review seems to need one of these, it needs a human instead:
+
+- `cargo` anything, including `cargo check`, `cargo fmt --check`, `cargo clippy`, `cargo metadata`
+  and `cargo tree` — build scripts and proc-macros run under all of them; `make` anything; the
+  `nebula` binary or anything under `target/`; INSTALL.SH; `python3`, `node`, `sh`, `bash`, `source`
+  on any file the PR adds or changes — reading a script means `cat`, not `sh -n`.
+- `gh pr checkout`, `git checkout`, `git switch`, `git merge`, `git rebase`, `git apply`,
+  `git cherry-pick`, `git stash`, `git worktree add`, `git reset` — nothing that moves the SHARED
+  CHECKOUT or creates a tree; editing or creating any file inside the repo.
+- `gh pr review --approve`, `gh pr review --request-changes`, `gh pr merge`, `gh pr edit`,
+  `gh pr close`, `gh pr comment` — the skill posts exactly one `--comment` review and changes nothing
+  else about the PR. Approval is a human's signature.
+
+The `--comment` review is the only write the skill performs, and it is the last step.
+
+## Steps
+
+### 1. Resolve the PR and fetch what GitHub knows
+
+The PR is the number, URL or branch the user gave; otherwise the current branch's PR (`gh pr view`
+with no argument); otherwise the PR this SESSION was opened on (a PR SESSION carries its URL in the
+system prompt). No PR → ask for the number; never pick one from `gh pr list`.
+
+```bash
+N=<number>; R=AgentSystemLabs/nebula; S=<scratchpad>
+gh pr view $N --repo $R --json number,title,url,body,author,baseRefName,headRefName,headRefOid,isDraft,isCrossRepository,mergeable,additions,deletions,changedFiles,files,commits,reviews,comments,statusCheckRollup > $S/pr.json
+gh pr diff $N --repo $R > $S/pr.diff
+gh pr checks $N --repo $R > $S/pr.checks 2>&1 || true      # exit 8 = pending; the output is still the answer
+gh api repos/$R/pulls/$N/comments --paginate --jq '.[] | "\(.path):\(.line // .original_line) \(.user.login): \(.body)"' > $S/pr.inline
+git fetch -q origin && git fetch -q origin pull/$N/head
+```
+
+Read `pr.json` first. `isCrossRepository` — a fork raises the bar on everything under `.github/`,
+`build.rs`, `Cargo.toml` and INSTALL.SH. `files` — sort them into the risk order of step 3 before
+opening the diff. `additions` / `deletions` / `changedFiles` — the only counts the review may quote.
+`statusCheckRollup` and `pr.checks` — on this repo that is `claude-review` alone; say so. `reviews`,
+`comments` and `pr.inline` — what the automated Claude Code Review workflow and any human already
+said; reference it, do not repeat it. Then read the description against the diff: a body that claims a
+focus-tint fix while the diff touches INSTALL.SH is the review's first finding.
+
+### 2. Read what the repo already knows about this area
+
+```bash
+grep -ril '<each touched file basename> \| <each TERM the PR touches>' .claude/memory/entries .claude/memory/prs | head
```

> **Bug: this `grep` regex is broken on both GNU and BSD grep**
>
> Two independent problems with `'<each touched file basename> \| <each TERM the PR touches>'`:
> - `\|` is a GNU BRE extension for alternation. On BSD/macOS `grep` it is *not* alternation — it matches a literal `|`, so the whole pattern silently matches nothing.
> - Even under GNU grep, the spaces sit inside the alternation branches (`"server.rs "` OR `" DAEMON SOCKET"`). Every real MEMORY LOG mention of a basename is followed by a backtick/comma/colon, never a space, so the file-basename branch matches 0/13 real occurrences of `server.rs` in `.claude/memory/entries` even under GNU grep.
>
> Because an empty result is indistinguishable from "the log has nothing on this area," this turns the skill's own "check what the repo already knows" step into a silent no-op. Compare [`AGENTS.md`'s canonical form](https://github.com/AgentSystemLabs/nebula/blob/08796166eeb0438110cff1aabf90c50d6b9f2e1d/AGENTS.md#L25), which has no alternation at all.
>
> ```suggestion
> grep -rilE '<each touched file basename>|<each TERM the PR touches>' .claude/memory/entries .claude/memory/prs | head
> ```

### `.claude/skills/pr-reviewer/SKILL.md:51` — @claude[bot] · 2026-09-05T02:09:55Z

```diff
@@ -0,0 +1,241 @@
+---
+name: pr-reviewer
+description: "Review a pull request by reading alone — its diff, description, CI state, the surrounding code at base and head, and the MEMORY LOG for the area — and leave one review comment on the PR that weighs, in this order, the security and production-merge risk of the change, its performance cost, and whether it follows the patterns already in this codebase. It never builds, tests, executes, installs or checks out the PR, and never approves or requests changes. Use when the user says \"review this PR\", \"review PR 26\", \"pr review\", \"pr reviewer\", \"is this safe to merge\", \"what would merging this break\", or asks for a risk read on a pull request."
+user-invocable: true
+---
+
+A pull request review here answers one question first: **what happens to the people running nebula
+in production if this merges?** Nebula is a DAEMON that owns every agent PTY on a developer's box,
+speaks to its TUI over an unauthenticated DAEMON SOCKET, installs MANAGED HOOKS into the user's own
+`~/.claude`, `~/.codex`, `~/.cursor` and `~/.pi`, spawns `claude` / `codex` / `cursor-agent` / `pi`
+with a composed argv and system prompt, runs `curl | sh` on remote hosts over NEBULA SSH, and migrates
+a SQLITE STORE on every user's disk at the first launch of a new binary. A wrong line in the wrong
+place is not a bug report; it is every user's sessions gone, or every user's shell running someone
+else's script. So the review weighs security and merge risk first and hardest, performance second, and
+fit with the codebase's patterns third — and says which of the three every finding belongs to.
+
+The second rule shapes everything else: **the reviewer reads; it never runs.** Not the PR's code, not
+its tests, not the build, not the binary, not a script the PR adds. Three reasons, each sufficient on
+its own. A PR — above all one from a fork (`isCrossRepository: true`) — is untrusted input, and even
+`cargo check` executes its `build.rs` and proc-macros as you, with your `gh` token, `~/.claude` and ssh
+keys in reach. The SHARED CHECKOUT belongs to several sessions at once, so a `gh pr checkout`, a merge
+or a stash yanks the tree out from under them. And a review that says "I ran the tests" asserts
+something the reader cannot audit anyway: CI's verdict is the test gate — and on this repo nothing on
+GitHub builds or tests a PR at all (`claude-code-review.yml` is the only PR workflow; `release.yml`
+runs on tags), which is itself a fact the review states. What reading cannot settle goes into the
+review as *unsettled*, named precisely enough that a human can run it. It is never quietly run.
+
+## Read, never run
+
+Allowed — every one of these is a read: it touches no working tree and executes nothing from the PR.
+
+```bash
+gh pr view <n> --json …                          # metadata, description, files, checks, reviews
+gh pr diff <n>                                   # the unified diff
+gh pr checks <n>                                 # CI state as GitHub reports it (exit 8 = pending)
+gh api repos/<owner>/<repo>/pulls/<n>/comments   # inline review comments — not a --json field
+git fetch origin && git fetch origin pull/<n>/head   # refresh origin/<base>; the PR head lands in FETCH_HEAD, no ref, no checkout
+git show <headRefOid>:<path>                     # the PR's whole file (the SHA is stable; FETCH_HEAD is not, other sessions fetch too)
+git show origin/<base>:<path>                    # the same file before the PR
+git log · git grep · grep · cat · sed -n · wc · diff   # over files, never through them
+```
+
+Forbidden, whatever the PR is — when a review seems to need one of these, it needs a human instead:
+
+- `cargo` anything, including `cargo check`, `cargo fmt --check`, `cargo clippy`, `cargo metadata`
+  and `cargo tree` — build scripts and proc-macros run under all of them; `make` anything; the
+  `nebula` binary or anything under `target/`; INSTALL.SH; `python3`, `node`, `sh`, `bash`, `source`
+  on any file the PR adds or changes — reading a script means `cat`, not `sh -n`.
+- `gh pr checkout`, `git checkout`, `git switch`, `git merge`, `git rebase`, `git apply`,
+  `git cherry-pick`, `git stash`, `git worktree add`, `git reset` — nothing that moves the SHARED
+  CHECKOUT or creates a tree; editing or creating any file inside the repo.
```

> **Bug: "editing or creating any file inside the repo" is forbidden here, but the repo's own protocol makes that mandatory on every task**
>
> `AGENTS.md` requires PROJECT TERMS "on **every** task, including one that recorded no entry" (https://github.com/AgentSystemLabs/nebula/blob/08796166eeb0438110cff1aabf90c50d6b9f2e1d/AGENTS.md#L60-L61), and `CLAUDE.md`'s trigger table lists its skip condition as "never" (https://github.com/AgentSystemLabs/nebula/blob/08796166eeb0438110cff1aabf90c50d6b9f2e1d/CLAUDE.md#L12-L13). NEBULA-MEMORY is likewise mandatory whenever a task "turned up something non-obvious about this repo." Both write inside the repo (`TERMS.md`, `.claude/MEMORY.md`, `.claude/memory/gotchas.md`, `.claude/memory/entries/`).
>
> This skill's own step 6 even checks whether the PR *being reviewed* left a MEMORY LOG entry, but its forbidden list gives the reviewing agent no exception to close the loop on itself — since the reviewer never puts the PR's own files on disk, "any file inside the repo" can only mean the reviewer's own SHARED CHECKOUT. A `/pr-reviewer` run that surfaces a non-obvious finding (as this PR's own authoring session did, producing the gotchas added elsewhere in this diff) is left with two directly conflicting mandatory instructions. Worth adding an explicit carve-out for the SELF-IMPROVING LOOP files.

### `.claude/skills/pr-reviewer/SKILL.md:39` — @claude[bot] · 2026-09-05T02:09:58Z

```diff
@@ -0,0 +1,241 @@
+---
+name: pr-reviewer
+description: "Review a pull request by reading alone — its diff, description, CI state, the surrounding code at base and head, and the MEMORY LOG for the area — and leave one review comment on the PR that weighs, in this order, the security and production-merge risk of the change, its performance cost, and whether it follows the patterns already in this codebase. It never builds, tests, executes, installs or checks out the PR, and never approves or requests changes. Use when the user says \"review this PR\", \"review PR 26\", \"pr review\", \"pr reviewer\", \"is this safe to merge\", \"what would merging this break\", or asks for a risk read on a pull request."
+user-invocable: true
+---
+
+A pull request review here answers one question first: **what happens to the people running nebula
+in production if this merges?** Nebula is a DAEMON that owns every agent PTY on a developer's box,
+speaks to its TUI over an unauthenticated DAEMON SOCKET, installs MANAGED HOOKS into the user's own
+`~/.claude`, `~/.codex`, `~/.cursor` and `~/.pi`, spawns `claude` / `codex` / `cursor-agent` / `pi`
+with a composed argv and system prompt, runs `curl | sh` on remote hosts over NEBULA SSH, and migrates
+a SQLITE STORE on every user's disk at the first launch of a new binary. A wrong line in the wrong
+place is not a bug report; it is every user's sessions gone, or every user's shell running someone
+else's script. So the review weighs security and merge risk first and hardest, performance second, and
+fit with the codebase's patterns third — and says which of the three every finding belongs to.
+
+The second rule shapes everything else: **the reviewer reads; it never runs.** Not the PR's code, not
+its tests, not the build, not the binary, not a script the PR adds. Three reasons, each sufficient on
+its own. A PR — above all one from a fork (`isCrossRepository: true`) — is untrusted input, and even
+`cargo check` executes its `build.rs` and proc-macros as you, with your `gh` token, `~/.claude` and ssh
+keys in reach. The SHARED CHECKOUT belongs to several sessions at once, so a `gh pr checkout`, a merge
+or a stash yanks the tree out from under them. And a review that says "I ran the tests" asserts
+something the reader cannot audit anyway: CI's verdict is the test gate — and on this repo nothing on
+GitHub builds or tests a PR at all (`claude-code-review.yml` is the only PR workflow; `release.yml`
+runs on tags), which is itself a fact the review states. What reading cannot settle goes into the
+review as *unsettled*, named precisely enough that a human can run it. It is never quietly run.
+
+## Read, never run
+
+Allowed — every one of these is a read: it touches no working tree and executes nothing from the PR.
+
+```bash
+gh pr view <n> --json …                          # metadata, description, files, checks, reviews
+gh pr diff <n>                                   # the unified diff
+gh pr checks <n>                                 # CI state as GitHub reports it (exit 8 = pending)
+gh api repos/<owner>/<repo>/pulls/<n>/comments   # inline review comments — not a --json field
+git fetch origin && git fetch origin pull/<n>/head   # refresh origin/<base>; the PR head lands in FETCH_HEAD, no ref, no checkout
+git show <headRefOid>:<path>                     # the PR's whole file (the SHA is stable; FETCH_HEAD is not, other sessions fetch too)
+git show origin/<base>:<path>                    # the same file before the PR
```

> **Bug: `origin/<base>` drifts to the current tip of the base branch, not this PR's merge base**
>
> Step 1 runs `git fetch origin` first, which fast-forwards local `origin/<base>` to whatever `main` currently is — not the commit this PR actually branched from. `gh pr diff` (used elsewhere in this same skill) is merge-base-relative, so on a repo where `main` merges multiple times a day, `git show origin/<base>:<path>` and `pr.diff` will disagree about a function's "previous shape" whenever another PR has touched the same file since this one branched — the comment `# the same file before the PR` is not accurate in that case.
>
> The skill then treats this comparison as evidence for a *confirmed by reading* finding — reused in [step 3](https://github.com/AgentSystemLabs/nebula/blob/08796166eeb0438110cff1aabf90c50d6b9f2e1d/.claude/skills/pr-reviewer/SKILL.md#L100-L104) and cited as the posted review's "Basis" — so it can fabricate a Blocker (attributing an already-merged PR's change to this one) or hide a real regression. Nothing in the skill computes an actual merge base (e.g. `git merge-base origin/<base> <headRefOid>`) anywhere.

### `.claude/skills/pr-reviewer/SKILL.md:86` — @webdevcody · 2026-09-05T02:43:28Z · reply

```diff
@@ -0,0 +1,241 @@
+---
+name: pr-reviewer
+description: "Review a pull request by reading alone — its diff, description, CI state, the surrounding code at base and head, and the MEMORY LOG for the area — and leave one review comment on the PR that weighs, in this order, the security and production-merge risk of the change, its performance cost, and whether it follows the patterns already in this codebase. It never builds, tests, executes, installs or checks out the PR, and never approves or requests changes. Use when the user says \"review this PR\", \"review PR 26\", \"pr review\", \"pr reviewer\", \"is this safe to merge\", \"what would merging this break\", or asks for a risk read on a pull request."
+user-invocable: true
+---
+
+A pull request review here answers one question first: **what happens to the people running nebula
+in production if this merges?** Nebula is a DAEMON that owns every agent PTY on a developer's box,
+speaks to its TUI over an unauthenticated DAEMON SOCKET, installs MANAGED HOOKS into the user's own
+`~/.claude`, `~/.codex`, `~/.cursor` and `~/.pi`, spawns `claude` / `codex` / `cursor-agent` / `pi`
+with a composed argv and system prompt, runs `curl | sh` on remote hosts over NEBULA SSH, and migrates
+a SQLITE STORE on every user's disk at the first launch of a new binary. A wrong line in the wrong
+place is not a bug report; it is every user's sessions gone, or every user's shell running someone
+else's script. So the review weighs security and merge risk first and hardest, performance second, and
+fit with the codebase's patterns third — and says which of the three every finding belongs to.
+
+The second rule shapes everything else: **the reviewer reads; it never runs.** Not the PR's code, not
+its tests, not the build, not the binary, not a script the PR adds. Three reasons, each sufficient on
+its own. A PR — above all one from a fork (`isCrossRepository: true`) — is untrusted input, and even
+`cargo check` executes its `build.rs` and proc-macros as you, with your `gh` token, `~/.claude` and ssh
+keys in reach. The SHARED CHECKOUT belongs to several sessions at once, so a `gh pr checkout`, a merge
+or a stash yanks the tree out from under them. And a review that says "I ran the tests" asserts
+something the reader cannot audit anyway: CI's verdict is the test gate — and on this repo nothing on
+GitHub builds or tests a PR at all (`claude-code-review.yml` is the only PR workflow; `release.yml`
+runs on tags), which is itself a fact the review states. What reading cannot settle goes into the
+review as *unsettled*, named precisely enough that a human can run it. It is never quietly run.
+
+## Read, never run
+
+Allowed — every one of these is a read: it touches no working tree and executes nothing from the PR.
+
+```bash
+gh pr view <n> --json …                          # metadata, description, files, checks, reviews
+gh pr diff <n>                                   # the unified diff
+gh pr checks <n>                                 # CI state as GitHub reports it (exit 8 = pending)
+gh api repos/<owner>/<repo>/pulls/<n>/comments   # inline review comments — not a --json field
+git fetch origin && git fetch origin pull/<n>/head   # refresh origin/<base>; the PR head lands in FETCH_HEAD, no ref, no checkout
+git show <headRefOid>:<path>                     # the PR's whole file (the SHA is stable; FETCH_HEAD is not, other sessions fetch too)
+git show origin/<base>:<path>                    # the same file before the PR
+git log · git grep · grep · cat · sed -n · wc · diff   # over files, never through them
+```
+
+Forbidden, whatever the PR is — when a review seems to need one of these, it needs a human instead:
+
+- `cargo` anything, including `cargo check`, `cargo fmt --check`, `cargo clippy`, `cargo metadata`
+  and `cargo tree` — build scripts and proc-macros run under all of them; `make` anything; the
+  `nebula` binary or anything under `target/`; INSTALL.SH; `python3`, `node`, `sh`, `bash`, `source`
+  on any file the PR adds or changes — reading a script means `cat`, not `sh -n`.
+- `gh pr checkout`, `git checkout`, `git switch`, `git merge`, `git rebase`, `git apply`,
+  `git cherry-pick`, `git stash`, `git worktree add`, `git reset` — nothing that moves the SHARED
+  CHECKOUT or creates a tree; editing or creating any file inside the repo.
+- `gh pr review --approve`, `gh pr review --request-changes`, `gh pr merge`, `gh pr edit`,
+  `gh pr close`, `gh pr comment` — the skill posts exactly one `--comment` review and changes nothing
+  else about the PR. Approval is a human's signature.
+
+The `--comment` review is the only write the skill performs, and it is the last step.
+
+## Steps
+
+### 1. Resolve the PR and fetch what GitHub knows
+
+The PR is the number, URL or branch the user gave; otherwise the current branch's PR (`gh pr view`
+with no argument); otherwise the PR this SESSION was opened on (a PR SESSION carries its URL in the
+system prompt). No PR → ask for the number; never pick one from `gh pr list`.
+
+```bash
+N=<number>; R=AgentSystemLabs/nebula; S=<scratchpad>
+gh pr view $N --repo $R --json number,title,url,body,author,baseRefName,headRefName,headRefOid,isDraft,isCrossRepository,mergeable,additions,deletions,changedFiles,files,commits,reviews,comments,statusCheckRollup > $S/pr.json
+gh pr diff $N --repo $R > $S/pr.diff
+gh pr checks $N --repo $R > $S/pr.checks 2>&1 || true      # exit 8 = pending; the output is still the answer
+gh api repos/$R/pulls/$N/comments --paginate --jq '.[] | "\(.path):\(.line // .original_line) \(.user.login): \(.body)"' > $S/pr.inline
+git fetch -q origin && git fetch -q origin pull/$N/head
+```
+
+Read `pr.json` first. `isCrossRepository` — a fork raises the bar on everything under `.github/`,
+`build.rs`, `Cargo.toml` and INSTALL.SH. `files` — sort them into the risk order of step 3 before
+opening the diff. `additions` / `deletions` / `changedFiles` — the only counts the review may quote.
+`statusCheckRollup` and `pr.checks` — on this repo that is `claude-review` alone; say so. `reviews`,
+`comments` and `pr.inline` — what the automated Claude Code Review workflow and any human already
+said; reference it, do not repeat it. Then read the description against the diff: a body that claims a
+focus-tint fix while the diff touches INSTALL.SH is the review's first finding.
+
+### 2. Read what the repo already knows about this area
+
+```bash
+grep -ril '<each touched file basename> \| <each TERM the PR touches>' .claude/memory/entries .claude/memory/prs | head
```

> Fixed in fbe4a0f: step 2 is now one `-E` pattern — `grep -rilE '<basename>|<TERM>' …` — with a comment on why (`\|` is GNU-only, and a space-padded branch never matches a backticked name).

### `.claude/skills/pr-reviewer/SKILL.md:51` — @webdevcody · 2026-09-05T02:43:29Z · reply

```diff
@@ -0,0 +1,241 @@
+---
+name: pr-reviewer
+description: "Review a pull request by reading alone — its diff, description, CI state, the surrounding code at base and head, and the MEMORY LOG for the area — and leave one review comment on the PR that weighs, in this order, the security and production-merge risk of the change, its performance cost, and whether it follows the patterns already in this codebase. It never builds, tests, executes, installs or checks out the PR, and never approves or requests changes. Use when the user says \"review this PR\", \"review PR 26\", \"pr review\", \"pr reviewer\", \"is this safe to merge\", \"what would merging this break\", or asks for a risk read on a pull request."
+user-invocable: true
+---
+
+A pull request review here answers one question first: **what happens to the people running nebula
+in production if this merges?** Nebula is a DAEMON that owns every agent PTY on a developer's box,
+speaks to its TUI over an unauthenticated DAEMON SOCKET, installs MANAGED HOOKS into the user's own
+`~/.claude`, `~/.codex`, `~/.cursor` and `~/.pi`, spawns `claude` / `codex` / `cursor-agent` / `pi`
+with a composed argv and system prompt, runs `curl | sh` on remote hosts over NEBULA SSH, and migrates
+a SQLITE STORE on every user's disk at the first launch of a new binary. A wrong line in the wrong
+place is not a bug report; it is every user's sessions gone, or every user's shell running someone
+else's script. So the review weighs security and merge risk first and hardest, performance second, and
+fit with the codebase's patterns third — and says which of the three every finding belongs to.
+
+The second rule shapes everything else: **the reviewer reads; it never runs.** Not the PR's code, not
+its tests, not the build, not the binary, not a script the PR adds. Three reasons, each sufficient on
+its own. A PR — above all one from a fork (`isCrossRepository: true`) — is untrusted input, and even
+`cargo check` executes its `build.rs` and proc-macros as you, with your `gh` token, `~/.claude` and ssh
+keys in reach. The SHARED CHECKOUT belongs to several sessions at once, so a `gh pr checkout`, a merge
+or a stash yanks the tree out from under them. And a review that says "I ran the tests" asserts
+something the reader cannot audit anyway: CI's verdict is the test gate — and on this repo nothing on
+GitHub builds or tests a PR at all (`claude-code-review.yml` is the only PR workflow; `release.yml`
+runs on tags), which is itself a fact the review states. What reading cannot settle goes into the
+review as *unsettled*, named precisely enough that a human can run it. It is never quietly run.
+
+## Read, never run
+
+Allowed — every one of these is a read: it touches no working tree and executes nothing from the PR.
+
+```bash
+gh pr view <n> --json …                          # metadata, description, files, checks, reviews
+gh pr diff <n>                                   # the unified diff
+gh pr checks <n>                                 # CI state as GitHub reports it (exit 8 = pending)
+gh api repos/<owner>/<repo>/pulls/<n>/comments   # inline review comments — not a --json field
+git fetch origin && git fetch origin pull/<n>/head   # refresh origin/<base>; the PR head lands in FETCH_HEAD, no ref, no checkout
+git show <headRefOid>:<path>                     # the PR's whole file (the SHA is stable; FETCH_HEAD is not, other sessions fetch too)
+git show origin/<base>:<path>                    # the same file before the PR
+git log · git grep · grep · cat · sed -n · wc · diff   # over files, never through them
+```
+
+Forbidden, whatever the PR is — when a review seems to need one of these, it needs a human instead:
+
+- `cargo` anything, including `cargo check`, `cargo fmt --check`, `cargo clippy`, `cargo metadata`
+  and `cargo tree` — build scripts and proc-macros run under all of them; `make` anything; the
+  `nebula` binary or anything under `target/`; INSTALL.SH; `python3`, `node`, `sh`, `bash`, `source`
+  on any file the PR adds or changes — reading a script means `cat`, not `sh -n`.
+- `gh pr checkout`, `git checkout`, `git switch`, `git merge`, `git rebase`, `git apply`,
+  `git cherry-pick`, `git stash`, `git worktree add`, `git reset` — nothing that moves the SHARED
+  CHECKOUT or creates a tree; editing or creating any file inside the repo.
```

> Fixed in fbe4a0f: the forbid list now carves out the SELF-IMPROVING LOOP's own files (`.claude/MEMORY.md`, `.claude/memory/`, `TERMS.md`), which NEBULA-MEMORY and PROJECT TERMS write after the review is posted, as on every task. Nothing from the PR lands on disk; the review body goes to the scratchpad outside the repo.

### `.claude/skills/pr-reviewer/SKILL.md:39` — @webdevcody · 2026-09-05T02:43:30Z · reply

```diff
@@ -0,0 +1,241 @@
+---
+name: pr-reviewer
+description: "Review a pull request by reading alone — its diff, description, CI state, the surrounding code at base and head, and the MEMORY LOG for the area — and leave one review comment on the PR that weighs, in this order, the security and production-merge risk of the change, its performance cost, and whether it follows the patterns already in this codebase. It never builds, tests, executes, installs or checks out the PR, and never approves or requests changes. Use when the user says \"review this PR\", \"review PR 26\", \"pr review\", \"pr reviewer\", \"is this safe to merge\", \"what would merging this break\", or asks for a risk read on a pull request."
+user-invocable: true
+---
+
+A pull request review here answers one question first: **what happens to the people running nebula
+in production if this merges?** Nebula is a DAEMON that owns every agent PTY on a developer's box,
+speaks to its TUI over an unauthenticated DAEMON SOCKET, installs MANAGED HOOKS into the user's own
+`~/.claude`, `~/.codex`, `~/.cursor` and `~/.pi`, spawns `claude` / `codex` / `cursor-agent` / `pi`
+with a composed argv and system prompt, runs `curl | sh` on remote hosts over NEBULA SSH, and migrates
+a SQLITE STORE on every user's disk at the first launch of a new binary. A wrong line in the wrong
+place is not a bug report; it is every user's sessions gone, or every user's shell running someone
+else's script. So the review weighs security and merge risk first and hardest, performance second, and
+fit with the codebase's patterns third — and says which of the three every finding belongs to.
+
+The second rule shapes everything else: **the reviewer reads; it never runs.** Not the PR's code, not
+its tests, not the build, not the binary, not a script the PR adds. Three reasons, each sufficient on
+its own. A PR — above all one from a fork (`isCrossRepository: true`) — is untrusted input, and even
+`cargo check` executes its `build.rs` and proc-macros as you, with your `gh` token, `~/.claude` and ssh
+keys in reach. The SHARED CHECKOUT belongs to several sessions at once, so a `gh pr checkout`, a merge
+or a stash yanks the tree out from under them. And a review that says "I ran the tests" asserts
+something the reader cannot audit anyway: CI's verdict is the test gate — and on this repo nothing on
+GitHub builds or tests a PR at all (`claude-code-review.yml` is the only PR workflow; `release.yml`
+runs on tags), which is itself a fact the review states. What reading cannot settle goes into the
+review as *unsettled*, named precisely enough that a human can run it. It is never quietly run.
+
+## Read, never run
+
+Allowed — every one of these is a read: it touches no working tree and executes nothing from the PR.
+
+```bash
+gh pr view <n> --json …                          # metadata, description, files, checks, reviews
+gh pr diff <n>                                   # the unified diff
+gh pr checks <n>                                 # CI state as GitHub reports it (exit 8 = pending)
+gh api repos/<owner>/<repo>/pulls/<n>/comments   # inline review comments — not a --json field
+git fetch origin && git fetch origin pull/<n>/head   # refresh origin/<base>; the PR head lands in FETCH_HEAD, no ref, no checkout
+git show <headRefOid>:<path>                     # the PR's whole file (the SHA is stable; FETCH_HEAD is not, other sessions fetch too)
+git show origin/<base>:<path>                    # the same file before the PR
```

> Fixed in fbe4a0f: the "before" side is the merge base now. Step 1 records `git merge-base origin/<base> <headRefOid>` in `pr.base`, the allow list shows `git show <mergeBase>:<path>` in place of `origin/<base>`, step 3 reads the previous shape from `pr.base`, and the review's Basis line names the merge base and head SHAs. `origin/<base>` is only fetched, never compared against.
