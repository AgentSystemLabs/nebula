# Nebula

## Project memory

This repo keeps a shared, committed MEMORY LOG in three layers: **`.claude/MEMORY.md`** is the index —
one line per task (date, title, the TERMS and files it is about), newest first, capped at 200 lines;
**`.claude/memory/gotchas.md`** holds the standing gotchas — the traps that outlive their task, one line
each, grouped by TERM, capped at 300 lines; **`.claude/memory/entries/<date>-<slug>.md`** hold the full
Asked / Did / Gotchas of each task and are opened by index line or grep, never read wholesale. Claude
Code sessions also get the matching entries injected per prompt by `.claude/hooks/recall.py`
(`[nebula recall] …`); other harnesses grep `.claude/memory/entries` for the TERMS and files the prompt
names. `make memory-check` enforces the caps.

It also keeps a shared glossary at **`TERMS.md`** (repo root): one ALL-CAPS canonical name per feature,
panel, key, CLI command, hook route, daemon mechanism, status and dev workflow, with the words the user
has used for it and where it lives in the code. Read it with the memory log, map the user's words onto
its **Alias index**, and use the TERMS — in caps, as spelled there — in all the output you produce
about this project: replies, summaries, plans, commit messages, PR descriptions, memory entries, and
code comments. Text written in the TERMS is much easier for the team to read than a fresh paraphrase of
the same thing each time, so prefer a TERM over a synonym even when the synonym reads more naturally.
Code identifiers are not renamed to match it; the glossary points at them.

**Before you start a task, read `.claude/MEMORY.md`, `.claude/memory/gotchas.md` and `TERMS.md`.** Scan
the index for entries related to what the user is asking — the same TERMS, the same crate, the same
symptom, the same file — open those entry files, and match on the user's vocabulary as well as your own. A recorded gotcha is a mine already stepped on. A recorded decision
("we're not doing X because Y") is settled unless the user reopens it. A recorded fix tells you where the
code that matters actually lives. Entries describe what was true when written: if one names a file or
flag, confirm it still exists before relying on it.

**After you finish a task, record it** by reading `.claude/skills/nebula-memory/SKILL.md` and following
it — an entry file, an index line, and any durable trap into the standing gotchas. Do this whenever the task changed code or behavior, diagnosed a bug, or turned up something
non-obvious about this repo, the daemon, the TUI, the vendored vt100, or the agent hook dialects. Skip it
for pure questions and for trivial edits that held no surprise — the log is only useful if it stays free
of restated diffs.

**Then keep the glossary true** by reading `.claude/skills/project-terms/SKILL.md` and following it — on
every task, even one that recorded no memory entry: record any word the user used for an existing TERM
that its row did not list, rename or retire a TERM the task changed, and put any *new* name in the
**Candidates** ledger at the bottom of `TERMS.md` — it is promoted to a TERM only once a later, separate
task uses it again.

**Then shape the reply** by reading `.claude/skills/output-doctor/SKILL.md` and following it, before
you write the reply that answers or closes the request: three fixed sections — `==== YOU ASKED ====`
(the prompt you worked from, verbatim), `==== OVERVIEW ====` (what happened, in plain sentences), and
`==== TECHNICAL OVERVIEW ====` (the details, kept short) — with `==== ACTION REQUIRED ====` between the
overview and the technical section if and only if the user must do something before the work is
complete (run a command, flip a setting, restart, decide, approve), as numbered steps with the exact
command. Every reply, every kind of task; only the one-line preamble and mid-task progress notes sit
outside it.

(Claude Code sessions get this same protocol from `CLAUDE.md`, and can invoke the skills directly as
`Skill(skill: "nebula-memory")`, `Skill(skill: "project-terms")` and `Skill(skill: "output-doctor")`.)

## Keep modules small

A file, type or function that has grown long is a refactoring smell, not a fact of life. This repo has
20k-line `event_loop.rs` and 4k-line `ui.rs` / `registry.rs` files precisely because every task added a
little more to the file it found; do not keep adding to the pile.

- **Split what you touch.** When the file, `impl` block, struct, enum or function you are editing is
  long — many screens, several unrelated concerns, a `match` with dozens of arms, a function that needs
  section comments to be read — extract the part you are working on (or the coherent piece next to it)
  into its own module, type or function with a name that says what it does. A `mod foo;` in a new file
  beside the old one is cheap; the next agent's grep finds `foo.rs` instead of a 20k-line haystack.
- **Split when it makes sense, not by ruler.** There is no line limit. A long table or a long, flat
  test module is fine; a function that does three things, or a file whose name no longer describes its
  contents, is not. Prefer one module per concern (a panel, an overlay, a hook dialect, a
  subcommand) over one module per crate.
- **Behavior-preserving, tested first.** An extraction is a refactor: confirm a test covers the code
  (write one if not), run it green against the old shape, then move the code and run it again. Do not
  change behavior and layout in the same commit, and keep the public names callers use unless the task
  is to rename them.
- **Stay in your lane.** Extract from the file the task already has you in; do not launch drive-by
  refactors of files the task does not touch — the SHARED CHECKOUT has other sessions mid-edit, and a
  wholesale move of a file they are in is a merge conflict for everyone. A file that deserves a split
  but is out of scope is worth one line in the reply, not a change.
