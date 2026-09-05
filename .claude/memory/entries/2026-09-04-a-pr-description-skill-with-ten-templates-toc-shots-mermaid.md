# A `pr-description` Skill With Ten Templates: TOC, Screenshots, Mermaid, Technical Overview — 2026-09-04

**Asked:** "do we have a skill to describe pr descriptions?" → no → "yes draft a pr skill, then generate
10 varients of how a pr description could look (including emoji, category breakdown, before / afters,
etc). at the very least every pr should include screenshots of the change and mermaid diagrams of the
change. keep high level, but leave a techincal overview section. make the categories easy to view, add a
top table of contents with links so I can quick click to sections"
→ refined: Draft a `pr-description` skill at `.claude/skills/pr-description/SKILL.md` (user-invocable,
same frontmatter shape as the RELEASE SKILL) that writes a PR body in the PR ARCHIVE's house style, then
ten template variants under `.claude/skills/pr-description/templates/`. Every variant stays high level
and always carries: a top table of contents whose links hit GitHub's auto-anchors, at least one
screenshot (assuming: captured with the SCREENSHOT HARNESS, hosted on a `pr-assets` branch, placeholder
when none can be made), at least one mermaid diagram of the change, easy-to-scan categories, and a
technical overview section (a PR-body section, not the OUTPUT DOCTOR's DETAILS). The skill ends by
writing the body to a file for `gh pr create --body-file` and keeps the Claude Code footer. (no questions asked)

**Did:** *Superseded in part on 2026-09-04:* the TOC anchor rule below targeted GitHub's file-view slugs,
which a PR body never gets; the templates and the skill's *GitHub anchors* section now use explicit `<a id>`
anchors — see `2026-09-04-pr-body-headings-get-no-github-anchors-toc-links-a-id`. New skill `.claude/skills/pr-description/SKILL.md` (208 lines): six mandatory parts of every PR
body (TOC, screenshots, mermaid, scannable categories, technical overview, harness footer), the house
style lifted from PR ARCHIVE #16 (opener, benefit groups, `## Notes` gate line), seven steps — gather
facts from `git log origin/main..HEAD` + the task's MEMORY LOG entry, pick a template, capture with the
SCREENSHOT HARNESS, host PNGs on an orphan `pr-assets` branch via a private worktree (raw URL
`https://raw.githubusercontent.com/AgentSystemLabs/nebula/pr-assets/<branch>/<name>.png`; `gh` cannot
upload attachments and images do not go into `main`), draw one diagram, write to `<scratchpad>/pr-body.md`,
`gh pr create --body-file` (never `--body`, backticks get substituted and the GUARD HOOK only covers
`git commit -m`). Ten templates in `templates/01-benefit-groups.md` … `10-crate-map.md` (before/after,
category table, story, release-note, user journey, collapsible `<details>`, bug fix, scorecard, crate
map), each ~65 lines with the TOC links precomputed. Verified: a github-slugger port checked every TOC
link in all ten (0 dead); every ALL-CAPS phrase in the skill and templates is a TERM row (fixed
STUB AGENTS → STUB AGENT, DOCS PAGE → DOCS PAGES); `terms_check.py` still `ok`. Mermaid blocks were
reviewed by eye, not rendered — no `mmdc` here and headless Chrome is SIGKILLed on this Mac.

**Live run (PR #27):** In the NEBULA WORKTREE `pr-description-skill` the skill was followed end to end: the branch pushed, the gate output rendered to PNG with Pillow + Menlo (no TUI screen changed, so no SCREENSHOT HARNESS capture), the orphan `pr-assets` branch created and pushed from a scratchpad worktree, the body written from `templates/01-benefit-groups.md` to `<scratchpad>/pr-body.md`, the anchor checker run (0 dead), `gh pr create --body-file` → https://github.com/AgentSystemLabs/nebula/pull/27, opened with `open -a "Google Chrome"`. The REST render (`Accept: application/vnd.github.html+json`) confirmed one `<pre lang="mermaid">` block and the `<img>` pointing at the raw `pr-assets` URL, which served 200 on the first request.

**Gotchas:**
- **GitHub's *file-view* heading anchor keeps the space after a stripped emoji**, so `## 📸 Screenshots` is
  `#-screenshots` (leading hyphen), `## Before / After` is `#before--after` (the slash leaves two
  spaces), `## 1. What changed` is `#1-what-changed` — in a README or blob. A PR body gets no heading
  anchors at all (corrected 2026-09-04), so the skill now links to explicit `<a id>`s and its checker
  matches links against those.
- **A `<placeholder>` sanitiser eats mermaid's `<-->` arrow**: `re.sub(r'<([^<>\n]+)>', r'\1')` turned
  `E <-->|"…"| S` into `E --|"…"| S`. Angle-bracket placeholders inside mermaid fences also risk being
  read as HTML in labels — the templates keep placeholders out of the fences entirely.
- **`terms_check.py::corpus_lines` reads only `<skill>/SKILL.md`**, not other files in a skill
  directory, so caps words in `templates/*.md` are invisible to TERMS CHECK: a TERM used only there
  still reports `dead`, and a stale TERM there is never flagged.
- **`pr-assets` does not exist on `origin` yet** (`git ls-remote --heads origin pr-assets` is empty);
  the skill's step 4 creates it as an orphan (`git worktree add --detach` + `checkout --orphan`) on
  first use, from a scratchpad worktree, never from the SHARED CHECKOUT's index.
- The Skill tool's listing picked the new skill up the moment `SKILL.md` existed (already recorded for
  OUTPUT DOCTOR and PROMPT DADDY): the `pr-description` row appeared in the next tool result.
- **Corrected 2026-09-04: GitHub does *not* add heading anchors in the browser for a PR body.** The REST
  `body_html` and a logged-out `curl` of the PR page showing no `id="user-content-…"` was the truth, not a
  pre-render gap — the comment pipeline never emits them; the slug port was checking against the file
  pipeline. See `2026-09-04-pr-body-headings-get-no-github-anchors-toc-links-a-id`.
- **Menlo has no emoji glyphs**: a `📸` in text rendered with Pillow + `Menlo.ttc` comes out as a tofu box. Keep
  emoji out of terminal text bound for a PNG, or paste Apple Color Emoji per the SCREENSHOT HARNESS recipe.
- **Replace a hosted PNG by renaming, not overwriting.** `raw.githubusercontent.com` served the new branch's file
  on the first request; the corrected image went in as `gate.png` with the old name deleted, so no CDN cache was
  ever asked to refresh. Overwriting in place is untested and may serve the stale bytes for minutes.
- **A NEBULA WORKTREE carries only committed skills.** Branched off `origin/main`, the worktree loaded the
  pre-2026-09-04 OUTPUT DOCTOR (ACTION REQUIRED / TECHNICAL OVERVIEW) because the rewrite was still
  uncommitted in the SHARED CHECKOUT; the new `pr-description` skill had to be `cp`'d across the same way.
  Uncommitted skill edits do not follow a session into its worktree.
