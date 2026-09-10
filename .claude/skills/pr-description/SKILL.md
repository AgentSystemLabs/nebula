---
name: pr-description
description: "Write a pull request description for the current branch in nebula's house style — a clickable table of contents, benefit-grouped sections a reader can scan, screenshots of the change, a mermaid diagram of the change, a risk read in the pr-reviewer's order, and a technical overview — from one of ten templates, then open or update the PR with gh pr create --body-file. Use when the user says \"write the PR description\", \"draft the PR\", \"open a PR\", \"describe this PR\", \"pr body\", \"pr text\", or asks what a PR should look like."
user-invocable: true
---

A PR description is read three times: by the reviewer deciding whether to open the diff, by the
person six weeks later asking why the code looks like this, and by whoever searches the merged PRs
for the *why* behind a change. Write for all three: high level first, a picture before a paragraph,
the mechanism at the bottom where only the reviewer scrolls.

This skill produces the **body**. The title follows the commit-subject rule: what a
*user* now gets, not what the diff did ("Workspaces as a top tab bar, and one nebula instance per
checkout", never "feat(tui): add tabs").

## What every PR body carries — no exceptions

1. **A table of contents at the top**, one link per `##` section, so the reader clicks straight to
   the part they want. GitHub gives a PR body's headings no anchors of their own, so every linked
   heading ends with `<a id="…"></a>` and the link names that id *with GitHub's prefix*,
   `#user-content-…` — see *GitHub anchors* below; a dead TOC is worse than none.
2. **Screenshots of the change.** A TUI change without a picture is a claim. At least one PNG of the
   screen after the change; a before/after pair when the change replaces something. Captured with
   the SCREENSHOT HARNESS, hosted on the `pr-assets` branch (recipe below). A change with no screen
   (a daemon mechanism, a CLI flag) shows its terminal output in a fenced block *and* a diagram, and
   says why there is no screenshot.
3. **A mermaid diagram of the change**, in a ```` ```mermaid ```` fence — GitHub renders it inline.
   Draw the *change*, not the whole system: the new flow, the state that moved, the crates touched.
   `flowchart`, `sequenceDiagram` and `stateDiagram-v2` cover nearly every PR here; the templates
   show which fits which shape.
4. **Categories a reader can scan.** Benefit groups (`🚀 Launch faster`, `🔔 Know when it's done`)
   or change classes (✨ Feature · 🐛 Fix · 📝 Docs · 🧪 Tests · ♻️ Refactor) — one emoji per
   heading, never on the bullets. A fix files under the feature whose promise it keeps. Inside a section a bullet is a **bold hook** and a dash, then two to four indented sub-bullets of
   one fact each — the key or command in backticks, a setting in the `Settings › Sessions › done_sound`
   form, a behaviour — never a paragraph (2026-09-05: PR #30's first cut could not be glanced through).
   When the change touches something the user likes, one bullet says what stayed the same. Credit
   contributors inline: `Thanks @handle (#NN).` No file paths, symbols or line numbers above the
   technical overview.
5. **A technical overview section**, last before the notes, for the reviewer: the mechanism in a
   few sentences, the files that matter with a clause each, the rejected approach they would ask
   about, the gate ("`make ci` green: fmt, clippy, 687 tests"). Everything above it stays high level.
6. **A risk section**, directly above the technical overview: the author's own read of what merging
   could break, in the PR REVIEWER SKILL's order — 🔒 security and production-merge risk, ⚡ performance
   cost, 🧩 fit with the codebase's patterns — one table row each, rated Low / Medium / High with a
   one-clause why, a 🟢 / 🟡 / 🔴 verdict above the table, and a **Rollback** line saying what a
   `git revert` undoes and what it does not (a PROTOCOL VERSION bump, a migrated store, a pushed
   branch). "No risk" is never bare: a docs-only or prose-only PR says *why* nothing runs. The
   reviewer checks this read against the diff, so write it as the reviewer would, not as the seller.
   When the change **adds a surface** — the DAEMON execs a program, opens a socket, a route or a
   `ClientRequest`, reads a new config source, writes a file it did not before — the Risk section
   opens with an `### 🎯 Attack surface` subsection above the verdict: one bullet per way in, saying
   who or what reaches it, what holds it, and what does not. The 🔒 row then rates the residue, not
   the whole list. One table cell is never enough for an exec surface (PR #42, 2026-09-09: the exec
   of a git-config path shipped as one cell, and the maintainer asked where the attack vectors were).
   The templates carry the block behind a guidance comment; a PR that adds no surface deletes it.
7. **The footer.** Every PR body ends with the line the harness gives you (currently
   `🤖 Generated with [Claude Code](https://claude.com/claude-code)` plus the session link) — keep
   it verbatim, last, after a blank line.

Every fact comes from the diff and the commits; do not add a claim
the code does not make. A gate that did not run is stated as such (PR #22: "`cargo test` could not be
run in this headless session — please run the test suite before merging"), never implied.

## The house style

PR #16 is the reference shape: an opening paragraph that says what landed and why in two sentences,
`##` sections named for what the user gets, bullets with a **bold lead-in** and the key or setting in
backticks, a `## Notes` section for merge state and the gate, the footer. Name nebula's mechanisms by
their capitalised names (DAEMON, TUI, HOOK RECEIVER) so the PRs rhyme.

## Steps

### 1. Gather the facts

```bash
git fetch origin
git log --oneline origin/main..HEAD                 # the commits: the story in order
git diff --stat origin/main...HEAD                  # the files: what the technical overview names
gh pr view --json number,title,url,body 2>/dev/null # an existing PR to update, or nothing
```

Read the commit messages before writing a word: they carry the user's framing of the change, and a
decision recorded there ("we're not doing X because Y") belongs in the body so the reviewer does not
re-ask it.

### 2. Pick a template

Ten variants live in `templates/`, each self-contained with every mandatory part in place. Pick by
the shape of the change, copy it to the scratchpad, and fill the `<placeholders>`; delete any guidance
comment (`<!-- … -->`) as you go.

| # | Template | Reach for it when |
|---|---|---|
| 01 | `01-benefit-groups.md` | the default — a feature PR with two to four things the user gets |
| 02 | `02-before-after.md` | the change *replaces* something visible: a layout, a key, a status |
| 03 | `03-category-table.md` | a mixed bag — features, fixes, docs and tests in one branch |
| 04 | `04-story.md` | one change with a strong why: problem → change → proof |
| 05 | `05-release-note.md` | the PR is most of a release; groups mirror the RELEASE NOTES |
| 06 | `06-user-journey.md` | the change is a sequence of things the user does and sees |
| 07 | `07-collapsible.md` | a big diff whose detail would bury the summary — `<details>` per section |
| 08 | `08-bug-fix.md` | an issue-driven fix: symptom, cause, fix, proof; closes an issue |
| 09 | `09-scorecard.md` | numbers tell the story: perf, size, test count, a protocol bump |
| 10 | `10-crate-map.md` | the change spans crates; organise by where it lives in the workspace |

Do not merge two templates into one body. If none fits, the closest one with a section renamed
beats a new shape — the archive is easier to read when the PRs rhyme.

### 3. Capture the screenshots

Never screenshot the real DAEMON's screen — someone else is working in it. Use the SCREENSHOT
HARNESS: `make shot SCENE=<name> KEYS="Tab j"` (`scripts/shot/shot.sh`) builds the debug binary — into the
repo's own `target/`, so do not export `CARGO_TARGET_DIR` for it, and start `cargo build` in the
background while gathering the facts or the first shot waits on a full rebuild — then runs
an isolated nebula against a demo repo with a stand-in `gh` (`scripts/shot/fixtures/` — add a fixture
or a `scripts/shot/scenes/<name>.keys` file for the screen the PR needs), drives it in a private tmux
and writes `design-screenshots/<name>.{txt,ansi,png}`. Its traps are encoded in the script (short
`NEBULA_RUNTIME_DIR`, `NEBULA_AGENT_CMD=/bin/cat`, one Bash call per drive, `capture-pane -epN`).

Take the "before" shot from an `origin/main` build only when the template asks for a pair. Crop
nothing; the whole screen at `190x50` is the house frame, and `design-screenshots/` shows the look.

### 4. Host them on the `pr-assets` branch

GitHub only shows an image the body can reach by URL, and `gh` cannot upload attachments. Images do
not go into `main`. They go on an orphan branch `pr-assets`, one directory per PR branch, addressed by
raw URL. Do it in a private worktree in the scratchpad — never in the SHARED CHECKOUT's index:

```bash
R=<repo>; W=<scratchpad>/pr-assets; SLUG=<branch-name>
git -C "$R" fetch origin pr-assets 2>/dev/null \
  && git -C "$R" worktree add "$W" -B pr-assets origin/pr-assets \
  || { git -C "$R" worktree add --detach "$W" && git -C "$W" checkout --orphan pr-assets && git -C "$W" rm -rfq .; }
mkdir -p "$W/$SLUG" && cp <scratchpad>/shots/*.png "$W/$SLUG/"
git -C "$W" add "$SLUG" && git -C "$W" commit -q -m "PR assets: $SLUG" && git -C "$W" push -q origin pr-assets
git -C "$R" worktree remove "$W"
```

Then reference each image as
`https://raw.githubusercontent.com/AgentSystemLabs/nebula/pr-assets/<branch-name>/<name>.png`.
Two images side by side is a two-column table with one `![…](url)` per cell — GitHub scales them to
the cell. Give every image alt text that says what it shows; a text search of the PR keeps the alt
text, not the pixels.

When no screenshot can be produced (no display path, harness broken, a pure daemon change), leave
the section in with the fenced terminal output and one line saying why there is no PNG. Do not delete
the section and do not ship a body that pretends.

### 5. Draw the diagram

One diagram that shows the change, ten to twenty nodes at most. Choose by shape:

- a new path through the system (a hook route, a new command's data flow) → `flowchart LR`
- a conversation between processes (TUI ↔ DAEMON ↔ agent CLI, a HANDSHAKE change) → `sequenceDiagram`
- a status or mode that gained or lost a transition (RUNNING → FINISHED → UNSEEN) → `stateDiagram-v2`
- crates or modules touched → `flowchart` with the changed nodes given a `classDef` fill

Name nodes in TERMS or real identifiers; quote any label with punctuation (`A["nebula kill"]`). Keep
the fence exactly ```` ```mermaid ```` — a language tag GitHub does not know renders as text.

### 6. Write the body to a file, and fix the anchors

Write the finished body to `<scratchpad>/pr-body.md`. Then check that every TOC link names
`#user-content-<id>` and has its `<a id>` anchor — the script in *GitHub anchors* does it in one call. Read the body once as the
reviewer: does the overview say what the user gets, does every picture have a caption, is the risk
verdict one the diff supports, does the technical overview name the file the reviewer would open
first?

### 7. Open or update the PR

Always `--body-file`, never `--body "$(cat …)"`: backticks in the body would be command-substituted
by zsh.

```bash
gh pr create --title "<title>" --body-file <scratchpad>/pr-body.md          # new
gh pr edit <number> --body-file <scratchpad>/pr-body.md                     # update
```

`gh pr create` needs the branch pushed and an account with write access — `gh auth status`; the
admin account is `webdevcody`, and `gh auth switch --hostname github.com --user webdevcody` if it
drifted. Add `--draft` when the gate has not run. Put `Closes #N` in the body, not the title, so
GitHub links the issue. After it lands, print the PR URL.

## GitHub anchors

A PR body is rendered by GitHub's *comment* pipeline (`gh api /markdown` with `"mode":"gfm"`), which
emits a bare `<h2 dir="auto">` for every heading — no id, no permalink. Only *file* views (a README,
a blob, a wiki page) get the auto-generated `#-screenshots`-style slugs, so a TOC that links to a
heading's slug is dead in every PR, however carefully the slug is computed. What survives the
sanitizer is an explicit anchor: GitHub keeps `<a id="…"></a>` and prefixes the id with
`user-content-`. Nothing puts that prefix back on click: a `#screenshots` link finds no element on
the PR page (PR #30, 2026-09-05 — the user clicked and nothing moved; the file-view hash handler that
bridges the prefix is not on the PR conversation page). So the link names the id *as rendered*,
`#user-content-<id>`, and the browser's own fragment navigation does the rest, no JS involved.

So every heading a TOC link targets ends with its own anchor, and the link names that id with the
prefix:

| Heading | Link |
|---|---|
| `## 📸 Screenshots <a id="screenshots"></a>` | `#user-content-screenshots` |
| `## Before / After <a id="before-after"></a>` | `#user-content-before-after` |
| `## 1. What changed <a id="1-what-changed"></a>` | `#user-content-1-what-changed` |
| `### 🚀 Launch faster <a id="launch-faster"></a>` | `#user-content-launch-faster` |

The id is the heading text lower-cased, emoji and punctuation dropped, spaces to single hyphens, no
leading hyphen (GitHub's *file* slugs keep one after a stripped emoji; ours never do). Two headings
with the same text get `-1`, `-2`. The anchor sits at the *end* of the heading line so the raw
Markdown — the PR PREVIEW, `gh pr view` — still reads as a heading; the anchor itself
never carries the prefix (GitHub would double it). The templates already carry the anchors and the
prefixed links for their own headings; when you rename or add a heading, add its anchor and recompute
the link. This checks a body file:

```bash
python3 - <scratchpad>/pr-body.md <<'PY'
import re, sys
body = open(sys.argv[1]).read()
anchors = set(re.findall(r'<a id="([^"]+)"></a>', body))
links = set(re.findall(r'\]\(#([^)]+)\)', body))
unprefixed = sorted(l for l in links if not l.startswith('user-content-'))
targets = {l.removeprefix('user-content-') for l in links}
bare = [h for h in re.findall(r'^## +(.+?)[ \t]*$', body, re.M) if '<a id=' not in h]
print('links missing the user-content- prefix:', unprefixed or 'none')
print('dead TOC links:', sorted(targets - anchors) or 'none')
print('anchors carrying the prefix themselves:', sorted(a for a in anchors if a.startswith('user-content-')) or 'none')
print('## headings without an anchor:', bare or 'none')
PY
```

To see what GitHub will really render, `gh api /markdown --input body.json` with `"mode":"gfm"` —
`"mode":"markdown"` is the file pipeline and shows anchors a PR body will never have.
