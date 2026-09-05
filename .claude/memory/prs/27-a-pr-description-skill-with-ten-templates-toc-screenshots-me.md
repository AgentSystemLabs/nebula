# PR #27 — A PR DESCRIPTION SKILL with ten templates: TOC, screenshots, mermaid, technical overview

- **URL:** https://github.com/AgentSystemLabs/nebula/pull/27
- **Author:** @webdevcody
- **Merged:** 2026-09-05T02:46:36Z by @webdevcody (`dc95e588ee49`)
- **Opened:** 2026-09-05T01:50:29Z
- **Branch:** `pr-description-skill` → `main`
- **Diff:** +0 −0 across 0 file(s)

## Description

> Every merged PR in the PR ARCHIVE was shaped by whoever wrote it — eleven bodies, each in its author's shape, none with a picture, a diagram or a table of contents; now one `pr-description` skill writes every PR body the same way, from one of ten templates, and opens the PR itself.
>
> **Contents:** [1. The problem](#1-the-problem) · [2. What changed](#2-what-changed) · [3. How it looks](#3-how-it-looks) · [4. How it works](#4-how-it-works) · [5. Risk](#5-risk) · [6. Technical overview](#6-technical-overview) · [7. Notes](#7-notes)
>
> ## 1. The problem <a id="1-the-problem"></a>
>
> A PR body here is read three times: by the reviewer deciding whether to open the diff, by whoever asks six weeks later why the code looks like this, and by the PR ARCHIVE, which renders every merged PR to Markdown so a future agent can grep the *why* without a network call. Nothing said what a body had to contain, so each one was whatever its author felt like that day. Of the eleven merged PRs in the archive, none has a screenshot, none a diagram, none a table of contents, and two have no section headings at all. PR #16 had become the reference shape by habit, not by rule.
>
> > at the very least every pr should include screenshots of the change and mermaid diagrams of the change. keep high level […] make the categories easy to view, add a top table of contents with links so I can quick click to sections
>
> ## 2. What changed <a id="2-what-changed"></a>
>
> - **One skill writes the body.** Say "write the PR description", "draft the PR" or "open a PR" and the PR DESCRIPTION SKILL gathers the facts — the branch's commits, its diffstat, the task's MEMORY LOG entry — picks a template, writes the body to a file and opens or updates the PR with `gh pr create --body-file`. The title still follows the RELEASE SKILL's rule: what the user now gets, never what the diff did.
> - **Seven parts, no exceptions.** A clickable table of contents, at least one screenshot, one mermaid diagram of the change, sections a reader can scan, a risk read, a technical overview last, the harness footer verbatim. A change with no screen — a daemon mechanism, a CLI flag — shows its terminal output and says why there is no PNG; it does not drop the section.
> - **Ten templates, one per shape of change.** Benefit groups, before / after, category table, story (this one), release note, user journey, collapsible, bug fix, scorecard, crate map. Each is complete on its own; the skill says which fits which and never merges two into one body.
> - **Pictures have a home.** PNGs go on the PR ASSETS BRANCH — an orphan `pr-assets` branch, one directory per PR branch, addressed by raw URL — because `gh` cannot upload attachments and images never land on `main`. TUI shots come from the SCREENSHOT HARNESS, never from the real DAEMON's screen.
> - **TOC links that land.** GitHub gives a PR body's headings no anchors, so every linked heading ends with an explicit `<a id="…"></a>` and the skill's checker flags a dead link before the PR opens.
> - **A risk read in every body.** Directly above the technical overview: a 🟢 / 🟡 / 🔴 verdict, then one row each for security and production-merge risk, performance cost and fit with the codebase — the PR REVIEWER SKILL's order, rated Low / Medium / High with a one-clause why — and a rollback line. "No risk" is never bare: a prose-only PR says why nothing runs.
> - **Unchanged.** The PR ARCHIVE's format, the RELEASE SKILL, and every PR already merged: nothing is rewritten backwards.
>
> ## 3. How it looks <a id="3-how-it-looks"></a>
>
> | The top of this PR, as github.com renders a body the skill wrote | Further down: the sequence diagram, rendered inline by GitHub |
> |---|---|
> | ![PR #27 on github.com: the one-sentence opener, the Contents line with six numbered links, and section 1, The problem](https://raw.githubusercontent.com/AgentSystemLabs/nebula/pr-assets/pr-description-skill/story-top.png) | ![PR #27 scrolled to section 4, How it works: GitHub's rendering of the mermaid sequence diagram between the User, the skill, git, the harness, the pr-assets branch and GitHub](https://raw.githubusercontent.com/AgentSystemLabs/nebula/pr-assets/pr-description-skill/story-diagram.png) |
>
> This PR changes no TUI screen, so the pictures are of its own rendered page — the skill's output is the thing to look at. Both were captured headless (WebKit) from the public PR page and pushed to the PR ASSETS BRANCH by the recipe in the skill.
>
> ## 4. How it works <a id="4-how-it-works"></a>
>
> ```mermaid
> sequenceDiagram
>   actor U as User
>   participant S as PR DESCRIPTION SKILL
>   participant G as git + MEMORY LOG
>   participant H as SCREENSHOT HARNESS
>   participant P as PR ASSETS BRANCH
>   participant GH as GitHub
>   U->>S: "write the PR description"
>   S->>G: git log, diff --stat, the task's entry
>   G-->>S: commits, files, the why, the gotchas
>   Note over S: pick one of ten templates
>   S->>H: drive the demo daemon, capture the screen
>   H-->>S: after.png
>   S->>P: commit the PNG from a scratchpad worktree, push
>   P-->>S: raw.githubusercontent.com URL
>   Note over S: fill the template, draw the diagram, check every TOC anchor
>   S->>GH: gh pr create --body-file pr-body.md
>   GH-->>U: the PR — TOC, pictures, diagram, technical overview
>   Note over P,GH: on merge, the PR ARCHIVE keeps the body
> ```
>
> ## 5. Risk <a id="5-risk"></a>
>
> **Verdict:** 🟢 Low risk — the PR adds prose an agent follows, a MEMORY LOG entry and two glossary rows; nothing in it compiles, runs, or is read by the DAEMON or the TUI.
>
> | | Level | Why |
> |---|---|---|
> | 🔒 **Security & production** | Low | No new surface: no Rust, no hook route, no `ClientRequest`, nothing executed at merge time. The skill's own commands run later under the user's `gh` login — a PR on the user's branch and a push to `pr-assets`, a branch that never merges. |
> | ⚡ **Performance** | Low | Off every hot path: nothing here executes at runtime. |
> | 🧩 **Fit with the codebase** | Low | The same shape as the RELEASE SKILL and the PR REVIEWER SKILL — frontmatter, numbered steps, a house-style section — and the entry, index line and CANDIDATES LEDGER rows follow the protocol `AGENTS.md` sets out. |
>
> **Rollback:** `git revert` of the merge removes the skill, the templates, the entry and the two glossary rows; it does not delete the `pr-assets` branch (`git push origin --delete pr-assets`) and does not un-write the PR bodies already produced with the skill.
>
> ## 6. Technical overview <a id="6-technical-overview"></a>
>
> - **Mechanism.** `.claude/skills/pr-description/SKILL.md` (229 lines, `user-invocable: true`, the RELEASE SKILL's frontmatter shape) carries the seven mandatory parts, the house style lifted from PR ARCHIVE #16, and seven steps: gather facts, pick a template, capture with the SCREENSHOT HARNESS, host on `pr-assets`, draw one diagram, write `pr-body.md` and check its anchors, `gh pr create --body-file` — never `--body`, since zsh would command-substitute the backticks and the GUARD HOOK only covers `git commit -m`. The `pr-assets` recipe makes the orphan branch from a scratchpad worktree (`git worktree add --detach` + `checkout --orphan`) and never touches the SHARED CHECKOUT's index.
> - **Files.** `templates/01-benefit-groups.md` … `10-crate-map.md` — 66 to 95 lines each, `<placeholders>` in the prose, guidance in HTML comments, TOC links and anchors precomputed, the RISK table's three rows in place, no placeholder inside a mermaid fence (a `<placeholder>` sanitiser once ate a `<-->` arrow); `.claude/memory/entries/2026-09-04-a-pr-description-skill-with-ten-templates-toc-shots-mermaid.md` and its line in `.claude/MEMORY.md` — the task's entry, nine gotchas; `TERMS.md` — PR DESCRIPTION SKILL and PR ASSETS BRANCH in the CANDIDATES LEDGER.
> - **Risk section.** Above the technical overview in every template: a verdict line, a three-row table (🔒 security & production, ⚡ performance, 🧩 fit with the codebase) and a rollback line — the PR REVIEWER SKILL's own headings, so the author's read and the reviewer's comment line up row for row.
> - **Why not heading slugs.** GitHub's comment pipeline (`gh api /markdown` in `gfm` mode, the pull's `body_html`) emits a bare `<h2>` with no id; only file views get `#-screenshots` slugs. An explicit `<a id="x"></a>` survives as `user-content-x` and `#x` resolves to it, so that is what the TOC links to. `"mode":"markdown"` shows anchors a PR will never have — do not trust it.
> - **Why not `gh` attachments or PNGs on `main`.** `gh` has no upload for PR images, and a PNG on `main` ships with every clone and never leaves. The orphan branch is reachable by raw URL and never merges.
> - **Known gap.** `terms_check.py::corpus_lines` reads only a skill's `SKILL.md`, so caps words in `templates/*.md` are outside the TERMS CHECK corpus; recorded in the entry, not changed here.
> - **Gate.** `make memory-check`, `make recall-eval` and `make terms-check` green in the branch's worktree at `02bdcb1` (index 176/200 lines, 156 entries; self-recall top-5 85 %; 238 TERMS). No Rust changed, so `cargo test` and `make ci` were not run.
>
> ## 7. Notes <a id="7-notes"></a>
>
> - Branched off `origin/main` at `eb9422e`; no merge needed.
> - Three commits: the skill and its templates; the first run's MEMORY LOG entry; then the anchor correction and the RISK section together — the first push had computed GitHub-style slugs for the TOC before it turned out a PR body's headings get no anchors at all.
> - This is the skill's second pass over its own PR: the story template replaces the first run's benefit groups, the pictures replace a terminal capture with the rendered page, and the RISK section was added to the skill during this pass, so section 5 above is its first use.
>
> 🤖 Generated with [Claude Code](https://claude.com/claude-code)
>
> https://claude.ai/code/session_01XqxqJMGstArcuf9SGsSM1g

## Commits (4)

- `4505414e4ef3` Add the PR DESCRIPTION SKILL with ten PR-body templates — @webdevcody, @claude
- `b17206fc89ee` Log the PR DESCRIPTION SKILL's first live run (PR #27) — @webdevcody, @claude
- `02bdcb1a7161` Link the TOC to explicit anchors and add a RISK section to every PR body — @webdevcody, @claude
- `574de1e598ee` Merge origin/main into pr-description-skill — @webdevcody, @claude

## Conversation (0)

_(no issue comments)_

## Reviews (0)

_(no review submissions)_

## Inline review comments (0)

_(no inline comments)_
