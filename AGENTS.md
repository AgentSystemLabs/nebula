# Nebula

## The shared memory and vocabulary

This repo runs a SELF-IMPROVING LOOP: every task starts from what past tasks recorded and ends by
recording its own. Four committed files carry it. Read the index, the standing gotchas and the
glossary before you start; the entries are fetched by index line or grep, never read wholesale.

| File | What it is | Cap |
|---|---|---|
| `.claude/MEMORY.md` | the **index** of the MEMORY LOG — one line per task: date, title, the TERMS and files it is about, its gotcha count, newest first | 200 lines |
| `.claude/memory/gotchas.md` | the **standing gotchas** — traps that outlive their task, one line each, grouped by TERM | 300 lines |
| `.claude/memory/entries/<date>-<slug>.md` | the **entries** — the full Asked / Did / Gotchas of each task | none |
| `TERMS.md` | the **glossary** — one ALL-CAPS name per feature, panel, key, command, hook route, daemon mechanism, status and workflow, with the user's words for it and where it lives | — |

`make ci` enforces this: `make memory-check` (caps, index ↔ entries), `make recall-eval` (the
entries are still findable from their own prompts), `make terms-check` (no stale `Where` pointers or
dangling aliases).

## Before you start a task

1. **Read `.claude/MEMORY.md`, `.claude/memory/gotchas.md` and `TERMS.md`.**
2. **Open the entries that match what the user is asking** — same TERMS, crate, symptom or file;
   grep when the index line is not enough (`grep -ril '<symbol or TERM>' .claude/memory/entries`).
   Match on the user's vocabulary as well as your own: entries quote the original request verbatim.
   - a recorded gotcha is a mine already stepped on — do not step on it again
   - a recorded decision ("we're not doing X because Y") is settled unless the user reopens it
   - a recorded fix tells you where the code that matters actually lives
   - all of it describes what was true when written: confirm a named file, function or flag still
     exists before relying on it, and correct it if it has gone stale
3. **Map every noun in the prompt onto `TERMS.md`** through its **Alias index** ("top nav" →
   WORKSPACES BAR). A word that maps to two TERMS is an ambiguity to settle before working, not to
   guess.
4. **Rewrite the prompt** — PROMPT DADDY: read `.claude/skills/prompt-daddy/SKILL.md` and follow it.
   Do this on every prompt that is a task, before planning or grepping in earnest. The refined
   prompt is the request you work from. The skill lists what it skips (a reply to your own question,
   a bare confirmation, a specific mid-task correction, a skill trigger, and a pure question that
   changes nothing).

## Speak in the TERMS

Use the TERMS, in ALL CAPS, exactly as `TERMS.md` spells them, in **everything** you write about
this project: replies, plans, `AskUserQuestion` options, commit messages, PR descriptions, release
notes, MEMORY LOG entries, and code comments. Prefer a TERM over a synonym even when the synonym
reads more naturally — a stiffer sentence the whole team parses instantly beats a smoother one only
you understand. When the user says an alias, answer in the TERM with their word beside it the first
time ("the WORKSPACES BAR (your 'top nav')"), the TERM alone after that. When they name something
with no TERM, say so and propose one. Code identifiers are never renamed to match the glossary; it
points at them.

## After you finish a task

1. **Record it** — the NEBULA-MEMORY SKILL: read `.claude/skills/nebula-memory/SKILL.md` and follow
   it: an entry file, an index line, any durable trap into the standing gotchas. Whenever the task
   changed code or behavior, diagnosed a bug, or turned up something non-obvious about this repo,
   the DAEMON, the TUI, the VENDORED VT100 or the agent hook dialects. Skip it for pure questions
   and for trivial edits that held no surprise — the log is only useful if it stays free of restated
   diffs.
2. **Keep the glossary true** — PROJECT TERMS: read `.claude/skills/project-terms/SKILL.md` and
   follow it, on **every** task, including one that recorded no entry.
3. **Shape the reply** — OUTPUT DOCTOR: read `.claude/skills/output-doctor/SKILL.md` and follow it,
   before you write a word of the reply that answers or closes the request. Every kind of reply,
   including a question you answered without changing anything.

## Writing code here

- **Keep modules small** — `.claude/rules/rust-modules.md`. Read it before editing a long file under
  `crates/`.
- **The SHARED CHECKOUT is edited by several sessions at once.** Red tests, compile errors and dirty
  files are routinely someone else's; `git status` and `git diff origin/main` before blaming your
  change, and never `git add`, `git stash pop` or `git checkout` the shared tree on your own
  judgment.
