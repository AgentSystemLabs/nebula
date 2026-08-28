# Nebula

## Project memory

This repo keeps a shared work log at **`.claude/MEMORY.md`** — one entry per task, recording what was
asked, what was actually done, and the gotchas hit along the way. It is committed, so every agent and
every session shares the same history.

It also keeps a shared glossary at **`TERMS.md`** (repo root): one ALL-CAPS canonical name per feature,
panel, key, CLI command, hook route, daemon mechanism, status and dev workflow, with the words the user
has used for it and where it lives in the code. Read it with the memory log, map the user's words onto
its **Alias index**, and use the TERMS — in caps, as spelled there — in all the output you produce
about this project: replies, summaries, plans, commit messages, PR descriptions, memory entries, and
code comments. Text written in the TERMS is much easier for the team to read than a fresh paraphrase of
the same thing each time, so prefer a TERM over a synonym even when the synonym reads more naturally.
Code identifiers are not renamed to match it; the glossary points at them.

**Before you start a task, read `.claude/MEMORY.md` and `TERMS.md`.** Scan it for entries related to what the user is
asking — the same crate, the same subsystem, the same symptom, the same tool — and match on the user's
vocabulary as well as your own. A recorded gotcha is a mine already stepped on. A recorded decision
("we're not doing X because Y") is settled unless the user reopens it. A recorded fix tells you where the
code that matters actually lives. Entries describe what was true when written: if one names a file or
flag, confirm it still exists before relying on it.

**After you finish a task, record it** by reading `.claude/skills/nebula-memory/SKILL.md` and following
it. Do this whenever the task changed code or behavior, diagnosed a bug, or turned up something
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
