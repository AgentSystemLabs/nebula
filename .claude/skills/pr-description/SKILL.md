---
name: pr-description
description: "Write a pull request description for the current branch in nebula's house style — a clickable table of contents, benefit-grouped sections a reader can scan, screenshots of the change, a mermaid diagram of the change, and a technical overview — from one of ten templates, then open or update the PR with gh pr create --body-file. Use when the user says \"write the PR description\", \"draft the PR\", \"open a PR\", \"describe this PR\", \"pr body\", \"pr text\", or asks what a PR should look like."
user-invocable: true
---

A PR description is read three times: by the reviewer deciding whether to open the diff, by the
person six weeks later asking why the code looks like this, and by the PR ARCHIVE
(`.claude/memory/prs/`), which renders every merged PR into Markdown so a future agent can grep the
*why* without a network call. Write for all three: high level first, a picture before a paragraph, the
mechanism at the bottom where only the reviewer scrolls.

This skill produces the **body**. The title is the commit-subject rule from the RELEASE SKILL: what a
*user* now gets, not what the diff did ("Workspaces as a top tab bar, and one nebula instance per
checkout", never "feat(tui): add tabs").

## What every PR body carries — no exceptions

1. **A table of contents at the top**, one link per `##` section, so the reader clicks straight to
   the part they want. The links must resolve — see *GitHub anchors* below; a dead TOC is worse than
   none.
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
   heading, never on the bullets. A fix files under the feature whose promise it keeps.
5. **A technical overview section**, last before the notes, for the reviewer: the mechanism in a
   few sentences, the files that matter with a clause each, the rejected approach they would ask
   about, the gate ("`make ci` green: fmt, clippy, 687 tests"). Everything above it stays high level.
6. **The footer.** Every PR body ends with the line the harness gives you (currently
   `🤖 Generated with [Claude Code](https://claude.com/claude-code)` plus the session link) — keep
   it verbatim, last, after a blank line.

Every fact comes from the diff, the commits and the MEMORY LOG entry of the task; do not add a claim
the code does not make. A gate that did not run is stated as such (PR #22: "`cargo test` could not be
run in this headless session — please run the test suite before merging"), never implied.

## The house style, from the PR ARCHIVE

`.claude/memory/prs/16-*.md` is the reference shape: an opening paragraph that says what landed and
why in two sentences, `##` sections named for what the user gets, bullets with a **bold lead-in** and
the key or setting in backticks, a `## Notes` section for merge state and the gate, the footer. Speak
in the TERMS from `TERMS.md`, in caps, as `AGENTS.md` requires — the archive is grep'd by TERM.

## Steps

### 1. Gather the facts

```bash
git fetch origin
git log --oneline origin/main..HEAD                 # the commits: the story in order
git diff --stat origin/main...HEAD                  # the files: what the technical overview names
gh pr view --json number,title,url,body 2>/dev/null # an existing PR to update, or nothing
grep -ril '<slug or TERM>' .claude/memory/entries   # the task's MEMORY LOG entry: the why and the gotchas
```

Read the entry before writing a word: its **Asked** line is the user's own framing of the change,
its **Gotchas** are the technical overview's best material, and a decision recorded there ("we're not
doing X because Y") belongs in the body so the reviewer does not re-ask it.

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
HARNESS: an isolated demo daemon + a STUB AGENT per session, driven by a private tmux server, rendered to PNG. The
recipe with its traps is the MEMORY LOG entry
`.claude/memory/entries/2026-08-20-restyle-focus-wash-and-the-screenshot-harness.md`; the traps that
cost the most: `NEBULA_RUNTIME_DIR` must be short (`/tmp/<short>` — the unix socket path has a
~104-char limit), `NEBULA_AGENT_CMD=/bin/cat` must be set even when no agent is created (the PREWARM
POOL launches a real `claude` otherwise), the whole tmux drive happens in **one** Bash call (the
sandbox kills the private server when the call ends), and `tmux capture-pane -epN` needs the `-N` or
the rightmost pane's background fill vanishes. Build the binary under test first — the harness runs
whatever `target/debug/nebula` is.

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
the cell. Give every image alt text that says what it shows; the PR ARCHIVE keeps the alt text, not
the pixels.

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

Write the finished body to `<scratchpad>/pr-body.md`. Then check every TOC link against GitHub's
anchor rule (below) — the script in *GitHub anchors* does it in one call. Read the body once as the
reviewer: does the overview say what the user gets, does every picture have a caption, does the
technical overview name the file the reviewer would open first?

### 7. Open or update the PR

Always `--body-file`, never `--body "$(cat …)"`: backticks in the body would be command-substituted
by zsh, and the GUARD HOOK only catches that for `git commit -m`.

```bash
gh pr create --title "<title>" --body-file <scratchpad>/pr-body.md          # new
gh pr edit <number> --body-file <scratchpad>/pr-body.md                     # update
```

`gh pr create` needs the branch pushed and an account with write access — `gh auth status`; the
admin account is `webdevcody`, and `gh auth switch --hostname github.com --user webdevcody` if it
drifted. Add `--draft` when the gate has not run. Put `Closes #N` in the body, not the title, so
GitHub links the issue. After it lands, print the PR URL; the PR ARCHIVE picks it up on merge.

## GitHub anchors

GitHub builds a heading's anchor by lower-casing it, deleting every character that is not a letter,
digit, space or hyphen, then turning spaces into hyphens. Emoji and punctuation vanish but the space
after them stays, so a heading that *starts* with an emoji gets a **leading hyphen**:

| Heading | Link |
|---|---|
| `## 📸 Screenshots` | `#-screenshots` |
| `## 🔧 Technical overview` | `#-technical-overview` |
| `## Before / After` | `#before--after` (the slash leaves two spaces) |
| `## 1. What changed` | `#1-what-changed` |
| `## Notes` | `#notes` |

Two headings that slug the same get `-1`, `-2` suffixes in order. The templates already carry the
right links for their own headings; when you rename a heading, recompute. This checks a body file:

```bash
python3 - <scratchpad>/pr-body.md <<'PY'
import re, sys, collections
body = open(sys.argv[1]).read()
seen, anchors = collections.Counter(), set()
for h in re.findall(r'^#{2,4} +(.+?)\s*$', body, re.M):
    s = re.sub(r'[^\w\- ]', '', h.lower()).replace(' ', '-')
    anchors.add(s if not seen[s] else f'{s}-{seen[s]}'); seen[s] += 1
links = set(re.findall(r'\]\(#([^)]+)\)', body))
print('dead TOC links:', sorted(links - anchors) or 'none')
PY
```

## Rules of the body

- **High level above the fold.** The opener and the category sections are for someone who will not
  open the diff. No file paths, no symbols, no line numbers until the technical overview.
- **Bold lead-ins, one emoji per heading, none on bullets.** Keys and identifiers in backticks.
- **Say what stayed the same** when the change touches something the user likes: "the tab underline
  and the current tab order are exactly as they were."
- **No invented facts.** A number comes from a command you ran; a behaviour from code you read; a
  why from the MEMORY LOG entry or the user's prompt.
- **`## Notes` carries the gate and the merge state**, one bullet each: what was run and passed, what
  was not run and why, what `origin/main` merge happened and what it broke.
- **Credit contributors inline**: `Thanks @handle (#NN).`
- **The footer is last, verbatim, after a blank line.** Nothing after it.
