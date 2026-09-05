# A PR Body's Headings Get No GitHub Anchors — The TOC Links To Explicit `<a id>`s — 2026-09-04

**Asked:** "the table of contents links at top of pr don't actually navigate me to the next section, fix it"
→ refined: The table of contents at the top of PR #27 and PR #28 (the PR DESCRIPTION SKILL bodies) doesn't
navigate on click on github.com; find out why the links are dead and fix it — the two open PR bodies, the
ten templates, and the skill's anchor rule and checker — so the TOC links land on their sections
(assuming: the GitHub PR page, not the PR PREVIEW pane). Keep every section, heading and the rest of the
bodies exactly as they are. (no questions asked)

**Did:** Diagnosed from GitHub itself, not from the slug: `gh api repos/AgentSystemLabs/nebula/pulls/27 -H
'Accept: application/vnd.github.html+json'` and an anonymous `curl` of the PR page both render every heading
as a bare `<h2 dir="auto">` — no `id`, no `class="anchor"`. GitHub's *comment* pipeline (what PR and issue
bodies use; `gh api /markdown` with `"mode":"gfm"`) never emits heading anchors; only the *file* pipeline
(`"mode":"markdown"`: README, blobs, wikis) makes the `#-screenshots` slugs the skill was computing. What
survives the sanitizer is an explicit `<a id="x"></a>` → `id="user-content-x"`, and the click path in
GitHub's `behaviors-*.js` resolves it: `:target` miss → `getElementById("user-content-"+hash) ||
getElementsByName(…)[0]`. Fix: every TOC-linked heading now ends with `<a id="<slug>"></a>` (heading text
lower-cased, emoji and punctuation dropped, single hyphens, no leading hyphen) and the link names that id —
`## 📸 Screenshots <a id="screenshots"></a>` ← `[📸 Screenshots](#screenshots)`. Rewrote the ten
`templates/*.md` and the *GitHub anchors* section + checker of `.claude/skills/pr-description/SKILL.md` in
the `pr-description-skill` NEBULA WORKTREE (`nebula-worktrees/pr-description-skill`, left uncommitted), then
`cp`'d them over the SHARED CHECKOUT's untracked older copies; `gh pr edit 27|28 --body-file` with the
rewritten bodies, PR #27's bullet that described the old rule rewritten. Verified on both PRs' `body_html`
and live pages: every TOC href has its `id="user-content-<slug>"` (8/8, 7/7), no page-chrome id collides with
any of the slugs, the new checker reports 0 dead on all twelve bodies, TERMS CHECK ok in both trees. Not
verified: an actual click in a browser (no browser automation here). The first PR DESCRIPTION SKILL entry's
two anchor gotchas were corrected in place.

**Gotchas:**
- **GitHub PR and issue bodies have no heading anchors at all.** The comment pipeline (`"mode":"gfm"`, the
  pull's `body_html`, the PR page) emits `<h2 dir="auto">…</h2>` with no id; only the file pipeline
  (`"mode":"markdown"`) makes `#-screenshots`. A slug-only TOC is dead in every PR however the slug is
  computed; an explicit `<a id="x"></a>` survives as `id="user-content-x"` and `#x` resolves to it. The
  earlier "GitHub adds the anchors in the browser" reading was wrong — the absence in `curl` output was the truth.
- **`gh api /markdown` with `"mode":"markdown"` lies about PR bodies** — it renders `markdown-heading`
  anchors a PR will never have. Use `"mode":"gfm"`, or `Accept: application/vnd.github.html+json` on the pull.
- **A `^(#{2,4}) +(.+?)\s*$` heading regex under `re.M` swallows the heading's newline** — `\s*` runs
  across `\n` and `$` still matches before the next one — so a `re.sub` that rebuilds the heading line deletes
  the blank line after every heading (60-odd `-` blank lines in `git diff`). Write `[ \t]*$` in a multiline
  heading regex.
- **The SHARED CHECKOUT's `.claude/skills/pr-description/` is an untracked, older copy** (its footer
  placeholder differs) of what the `pr-description-skill` branch commits, and `git diff <branch> -- <dir>`
  shows untracked files as wholly deleted — `diff -rq` the two directories, edit the worktree copy, `cp` across.
- **An anonymous `curl https://github.com/…/pull/N` is server-rendered for a public repo** — the body's
  `<h2>`s are in the HTML — so `grep -c user-content-` on it is a browser-free anchor check; GitHub's own
  `#top`, `#event-…`, `#ref-commit-…`, `#commits-pushed-…` hrefs are page chrome, not TOC links.
- `gh pr view N` outside a git repo fails with `failed to run git: fatal: not a git repository` — from the
  scratchpad, pass `-R AgentSystemLabs/nebula`.
