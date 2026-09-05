---
name: pr-reviewer
description: "Review a pull request by reading alone — its diff, description, CI state, the surrounding code at base and head, and the MEMORY LOG for the area — and leave one review comment on the PR that weighs, in this order, the security and production-merge risk of the change, its performance cost, and whether it follows the patterns already in this codebase. It never builds, tests, executes, installs or checks out the PR, and never approves or requests changes. Use when the user says \"review this PR\", \"review PR 26\", \"pr review\", \"pr reviewer\", \"is this safe to merge\", \"what would merging this break\", or asks for a risk read on a pull request."
user-invocable: true
---

A pull request review here answers one question first: **what happens to the people running nebula
in production if this merges?** Nebula is a DAEMON that owns every agent PTY on a developer's box,
speaks to its TUI over an unauthenticated DAEMON SOCKET, installs MANAGED HOOKS into the user's own
`~/.claude`, `~/.codex`, `~/.cursor` and `~/.pi`, spawns `claude` / `codex` / `cursor-agent` / `pi`
with a composed argv and system prompt, runs `curl | sh` on remote hosts over NEBULA SSH, and migrates
a SQLITE STORE on every user's disk at the first launch of a new binary. A wrong line in the wrong
place is not a bug report; it is every user's sessions gone, or every user's shell running someone
else's script. So the review weighs security and merge risk first and hardest, performance second, and
fit with the codebase's patterns third — and says which of the three every finding belongs to.

The second rule shapes everything else: **the reviewer reads; it never runs.** Not the PR's code, not
its tests, not the build, not the binary, not a script the PR adds. Three reasons, each sufficient on
its own. A PR — above all one from a fork (`isCrossRepository: true`) — is untrusted input, and even
`cargo check` executes its `build.rs` and proc-macros as you, with your `gh` token, `~/.claude` and ssh
keys in reach. The SHARED CHECKOUT belongs to several sessions at once, so a `gh pr checkout`, a merge
or a stash yanks the tree out from under them. And a review that says "I ran the tests" asserts
something the reader cannot audit anyway: CI's verdict is the test gate — and on this repo nothing on
GitHub builds or tests a PR at all (`claude-code-review.yml` is the only PR workflow; `release.yml`
runs on tags), which is itself a fact the review states. What reading cannot settle goes into the
review as *unsettled*, named precisely enough that a human can run it. It is never quietly run.

## Read, never run

Allowed — every one of these is a read: it touches no working tree and executes nothing from the PR.

```bash
gh pr view <n> --json …                          # metadata, description, files, checks, reviews
gh pr diff <n>                                   # the unified diff
gh pr checks <n>                                 # CI state as GitHub reports it (exit 8 = pending)
gh api repos/<owner>/<repo>/pulls/<n>/comments   # inline review comments — not a --json field
git fetch origin && git fetch origin pull/<n>/head   # refresh origin/<base>; the PR head lands in FETCH_HEAD, no ref, no checkout
git merge-base origin/<base> <headRefOid>        # the commit the PR branched from — origin/<base> has moved on since, and is not the base
git show <headRefOid>:<path>                     # the PR's whole file (the SHA is stable; FETCH_HEAD is not, other sessions fetch too)
git show <mergeBase>:<path>                      # the same file as the PR saw it — what `gh pr diff` is relative to
git log · git grep · grep · cat · sed -n · wc · diff   # over files, never through them
```

Forbidden, whatever the PR is — when a review seems to need one of these, it needs a human instead:

- `cargo` anything, including `cargo check`, `cargo fmt --check`, `cargo clippy`, `cargo metadata`
  and `cargo tree` — build scripts and proc-macros run under all of them; `make` anything; the
  `nebula` binary or anything under `target/`; INSTALL.SH; `python3`, `node`, `sh`, `bash`, `source`
  on any file the PR adds or changes — reading a script means `cat`, not `sh -n`.
- `gh pr checkout`, `git checkout`, `git switch`, `git merge`, `git rebase`, `git apply`,
  `git cherry-pick`, `git stash`, `git worktree add`, `git reset` — nothing that moves the SHARED
  CHECKOUT or creates a tree; editing or creating any file inside the repo, with one carve-out: the
  SELF-IMPROVING LOOP's own files — `.claude/MEMORY.md`, `.claude/memory/`, `TERMS.md` — which
  NEBULA-MEMORY and PROJECT TERMS write *after* the review is posted, as on every other task. Nothing
  from the PR ever lands on disk; the review itself is written to the scratchpad, outside the repo.
- `gh pr review --approve`, `gh pr review --request-changes`, `gh pr merge`, `gh pr edit`,
  `gh pr close`, `gh pr comment` — the skill posts exactly one `--comment` review and changes nothing
  else about the PR. Approval is a human's signature.

The `--comment` review is the only write the skill performs, and it is the last step.

## Steps

### 1. Resolve the PR and fetch what GitHub knows

The PR is the number, URL or branch the user gave; otherwise the current branch's PR (`gh pr view`
with no argument); otherwise the PR this SESSION was opened on (a PR SESSION carries its URL in the
system prompt). No PR → ask for the number; never pick one from `gh pr list`.

```bash
N=<number>; R=AgentSystemLabs/nebula; S=<scratchpad>
gh pr view $N --repo $R --json number,title,url,body,author,baseRefName,headRefName,headRefOid,isDraft,isCrossRepository,mergeable,additions,deletions,changedFiles,files,commits,reviews,comments,statusCheckRollup > $S/pr.json
gh pr diff $N --repo $R > $S/pr.diff
gh pr checks $N --repo $R > $S/pr.checks 2>&1 || true      # exit 8 = pending; the output is still the answer
gh api repos/$R/pulls/$N/comments --paginate --jq '.[] | "\(.path):\(.line // .original_line) \(.user.login): \(.body)"' > $S/pr.inline
git fetch -q origin && git fetch -q origin pull/$N/head
git merge-base origin/<baseRefName> <headRefOid> > $S/pr.base   # both from pr.json; the commit pr.diff is relative to
```

Read `pr.json` first. `isCrossRepository` — a fork raises the bar on everything under `.github/`,
`build.rs`, `Cargo.toml` and INSTALL.SH. `files` — sort them into the risk order of step 3 before
opening the diff. `additions` / `deletions` / `changedFiles` — the only counts the review may quote.
`statusCheckRollup` and `pr.checks` — on this repo that is `claude-review` alone; say so. `reviews`,
`comments` and `pr.inline` — what the automated Claude Code Review workflow and any human already
said; reference it, do not repeat it. `pr.base` — the merge base, the only "before" this review reads:
`origin/<base>` is whatever `main` is now, and on a day with several merges a file there already
carries other PRs' changes, so comparing against it pins their hunks on this PR or hides a regression.
Then read the description against the diff: a body that claims a focus-tint fix while the diff touches
INSTALL.SH is the review's first finding.

### 2. Read what the repo already knows about this area

```bash
grep -rilE '<each touched file basename>|<each TERM the PR touches>' .claude/memory/entries .claude/memory/prs | head   # one -E pattern: BRE `\|` is GNU-only, and a branch padded with spaces never matches a backticked name
grep -n '<TERM>' .claude/memory/gotchas.md
```

A STANDING GOTCHAS line the diff steps on again is a finding, with the entry as evidence. A recorded
decision ("we are not doing X because Y") the diff reverses is a finding unless the description shows
it knows. The PR ARCHIVE shows what a reviewer pushed back on the last time this file changed.
`TERMS.md` names the mechanism each file belongs to — use those names in the review.

### 3. Read the diff, then the code around it

Read `pr.diff` hunk by hunk in risk order: `.github/`, INSTALL.SH, `Cargo.toml` / `Cargo.lock`,
`build.rs`; then `nebula-daemon/src/server.rs`, `hooks/`, `registry.rs` (spawn, argv, system prompt),
`store.rs` (`MIGRATIONS`), `nebula-core/src/protocol.rs` (PROTOCOL VERSION), `sibling.rs`, `ssh.rs`,
`tunnel.rs`, `upgrade.rs`, `browser.rs`, `paths.rs`, `lifecycle.rs`; then the TUI; then docs and the
MEMORY LOG files. A hunk is not a unit of meaning: for every function the diff touches, `git show
<headRefOid>:<path>` the whole function and `git show $(cat $S/pr.base):<path>` its previous shape —
the merge base, never `origin/<base>` — and `git grep` the callers of anything whose signature or
contract moved.

### 4. Security and production-merge risk — first and heaviest

Ask of each hunk: who can reach it, and what can it reach. Nebula's surfaces, with the facts a review
can lean on (the 2026-08-30 security walkthrough in the MEMORY LOG is the source):

- **DAEMON SOCKET.** `handle_client` (`server.rs`) checks only the PROTOCOL VERSION, then honours every
  `ClientRequest`; the boundary is the 0700 runtime dir, on macOS under world-writable `/tmp`. A new or
  widened `ClientRequest` variant is reachable by any process of the same uid: does it write into a
  PTY, spawn, kill, delete, move a worktree, touch a path?
- **HOOK RECEIVER.** One BEARER TOKEN per daemon boot speaks for every AGENT; `agent_id`, `cwd` and
  `session_id` in a hook payload are trusted verbatim. New handling of a payload field is new trust.
- **Spawn and argv.** Every `Command::new`, `sh -c`, shell string, `--append-system-prompt`, first
  prompt and `-p` argument: what user- or repo-controlled text reaches it — branch names, worktree
  paths, PR titles and URLs (`validate_pr_url` accepts any host), CONFIG.JSON values, hook payloads?
  Text composed into a system prompt is a prompt-injection channel that survives RESUME.
- **Files and permissions.** Anything under `runtime_dir()` or the DATA DIR (symlink following, owner
  checks), and every write into the user's own `~/.claude/settings*.json`, `~/.codex/hooks.json`,
  `~/.cursor/hooks.json`, `~/.pi/agent/extensions/` — the MANAGED HOOKS installers merge into files the
  user also edits; a bug there corrupts the user's real configuration on every spawn.
- **Network and remote execution.** NEBULA BROWSER (`--bind`, `--public`, `--credential`), NEBULA
  TUNNEL, NEBULA SSH, NEBULA UPGRADE, INSTALL.SH, `NEBULA_INSTALL_URL` — the install URL is what runs
  on every remote cold start. Anything that widens a listener, weakens or skips a credential, or adds
  a fetch-and-execute.
- **Secrets.** Tokens or task text into argv (visible in `ps`), the DAEMON LOG, the SQLITE STORE, a
  hook payload, a PR body, a Cloud task argument.
- **State every user carries.** A MIGRATION runs once, irreversibly, on every user's SQLITE STORE at
  the first launch of the new binary; a PROTOCOL VERSION bump makes every running DAEMON refuse the new
  TUI until NEBULA KILL, which stops every live session. Both are production impact to state plainly,
  with the rollback path or its absence.
- **Supply chain.** New crates (why, how maintained, what they pull in), `build.rs`, new or changed
  workflows (`pull_request_target`, `permissions:`, secrets, unpinned actions), INSTALL.SH.
- **Scope drift.** Files touched that the description does not account for.

For each finding: `path:line` on the PR's side of the diff, who triggers it, what it reaches, the blast
radius on a box running nebula, and whether it is *confirmed by reading* or *suspected*. Rank Blocker /
Should fix / Nit within the section. Nothing found is a valid result and is written as "nothing found —
looked at ‹the surfaces this diff touches›", never as silence.

### 5. Performance — second

Weigh by how often the path runs, not by how the code looks. Hot in nebula: the TUI draw (every frame,
`ui.rs` and everything it calls), the event-loop drain, the PTY byte path into the VENDORED VT100, the
ATTACH replay of the 1 MB SCROLLBACK RING, the 2 s WORKTREE SYNC (stat-only by design — a `git` call
per tick is a regression), the GIT POLL (`gh` calls share the user's token with every Claude SESSION on
the box; contention, not the rate limit, is the bound), STATUS MACHINE writes into the SQLITE STORE,
and anything on the DAEMON's async runtime — a blocking `std::fs` call, `Command::output` or sleep
inside a tokio task stalls every session's I/O. Look for a per-frame allocation or clone, a process
spawn per tick, an unbounded `Vec` / `HashMap`, an O(n²) over sessions or worktrees, a timer that wakes
more often than it must. A cost paid once at launch is a Nit; the same cost per frame is a Should fix.

### 6. Fit with the codebase — third

The patterns are written down; check the diff against them rather than against taste:

- **KEEP MODULES SMALL** (`.claude/rules/rust-modules.md`): a change that grows `event_loop.rs`,
  `ui.rs` or `registry.rs` where a new module beside it would do; a `match` that gained arms it should
  have delegated.
- **Reuse before re-implementation** — `git grep` for the helper before flagging: `shell_single_quote`
  (`ssh.rs`), `fit` / `wrap` (`pr_preview.rs`), `nebula_core::env` for every env var name, named
  literals over magic numbers, `with_default_config` / `with_config_path` in any test that touches
  CONFIG.JSON (a test that pins neither reads, and through a saving key writes, the developer's real
  file).
- **The wire and the disk**: a change to a shared type in `protocol.rs` bumps PROTOCOL VERSION; a
  schema change appends a MIGRATION and never edits an old one.
- **Deliberately dep-light**: a new crate for something the workspace already does by hand (RFC 3339
  timestamps compare as strings; there is no `chrono`).
- **Tests beside the change**: TESTBACKEND for TUI drawing, `handle_key`-driven unit tests, E2E TUI and
  E2E PTY for DAEMON behaviour; a behaviour change with no test is a Should fix.
- **The words**: TERMS in comments, docs and messages; `docs/*.md` and the `--help` page updated when a
  key, command or setting changes; a MEMORY LOG entry for a non-trivial change — the SELF-IMPROVING
  LOOP expects one.

Compare the new code's shape to its nearest sibling — a new overlay to an existing overlay, a new hook
dialect to `install_codex_hooks`. The sibling is the pattern.

### 7. Write the review

To `<scratchpad>/pr-review.md`, in this shape. The sections are fixed and their order is the
weighting:

```markdown
## Merge-risk review — read, not run

**Verdict:** 🔴 Do not merge as-is · 🟡 Merge with care · 🟢 Low risk — one clause saying why.
**Basis:** the diff (+N / −M over K files), the surrounding code at the merge base `<mergeBase[:7]>`
and the head `<headRefOid[:7]>`, the description, CI (`claude-review` only — nothing on GitHub builds
or tests a PR here) and the prior reviews. Nothing was built, run or checked out for this review.

### 🔒 Security & production risk
- **Blocker — <what>.** `path:line` — who triggers it, what it reaches, the blast radius. *Confirmed
  by reading* / *Suspected: <what a run would settle>.* What to change.
- **Should fix — …**
- **Nit — …**
*(or)* Nothing found — looked at: <the surfaces this diff touches>.

### ⚡ Performance
- …

### 🧩 Fit with the codebase
- …

### 📐 Scope
What the description claims · what the diff touches · anything outside the claim.

### ❓ Unsettled by reading
- <the exact check a human should run — `cargo test -p nebula-daemon store::`, a manual step — and
  which result would change the verdict>

<the harness footer, verbatim, after a blank line>
```

Rules of the body: every finding carries a `path:line` from the PR's side of the diff and a severity;
*confirmed* and *suspected* are never blurred; a number comes from `pr.json` or a command you ran;
nothing the automated Claude Code Review already posted inline is re-listed — cite it ("the inline
comment at `app.rs:1204` stands"); TERMS in caps; bold lead-ins, one emoji per heading, none on
bullets; under ~70 lines unless the Blockers need more. Speak to the author, not about them.

### 8. Post it — the one write

```bash
gh auth status                                                      # the account that signs it
gh pr review $N --repo $R --comment --body-file <scratchpad>/pr-review.md
```

Always `--body-file` — backticks in `--body "…"` are command-substituted by zsh, and the GUARD HOOK
only catches that for `git commit -m`. `--comment` works on your own PR too (GitHub refuses
`--approve` and `--request-changes` there). Then print the review URL and the body in the reply. When
the user asks for the review in chat only ("don't post", "just tell me"), stop before this step and
print the body — the file is the deliverable either way.

## When the reviewer is tempted to run something

"Does this compile?" — CI would say; here nothing on GitHub does, so it is an *Unsettled* item, named.
"Do the tests pass?" — the same. "What does this script do?" — `cat` it and read it. "Is this helper
already there?" — `git grep`. "Which of these two behaviours does the code have?" — read both call
sites; if it still cannot be settled, say so, with both readings. The review is worth more for saying
what it could not check than for having checked it in a way the reader cannot trust.
