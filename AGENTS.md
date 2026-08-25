# Nebula

## Project memory

This repo keeps a shared work log at **`.claude/MEMORY.md`** — one entry per task, recording what was
asked, what was actually done, and the gotchas hit along the way. It is committed, so every agent and
every session shares the same history.

**Before you start a task, read `.claude/MEMORY.md`.** Scan it for entries related to what the user is
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

(Claude Code sessions get this same protocol from `CLAUDE.md`, and can invoke the skill directly as
`Skill(skill: "nebula-memory")`.)
