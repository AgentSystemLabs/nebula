# Nebula

## Project memory

This repo keeps a shared work log at **`.claude/MEMORY.md`** — one entry per task, recording what was
asked, what was actually done, and the gotchas hit along the way. It is written by the
`nebula-memory` skill and committed, so every agent and every session shares the same history.

### Before you start a task

**Read `.claude/MEMORY.md` first**, before touching code or planning an approach.

Scan it for entries related to what the user is asking — the same crate, the same subsystem, the same
symptom, the same tool. Match on the user's vocabulary as well as yours; entries record the original
request in the user's own framing for exactly this reason. When an entry is related, fold its context
into how you work:

- a recorded gotcha is a mine already stepped on — do not step on it again
- a recorded decision ("we're not doing X because Y") is settled unless the user reopens it
- a recorded fix tells you where the code that matters actually lives

Memory entries describe what was true when they were written. If one names a file, function, or flag,
confirm it still exists before you rely on it, and correct the entry if it has gone stale.

### After you finish a task

Invoke the `nebula-memory` skill to record it:

```
Skill(skill: "nebula-memory")
```

Do this whenever the task changed code or behavior, diagnosed a bug, or turned up something
non-obvious about this repo, the daemon, the TUI, the vendored vt100, or the agent hook dialects.
The skill has the entry format and the rules for what is worth recording — including when the right
answer is to record nothing.

Skip it for pure questions you answered without changing anything, and for trivial edits that held no
surprise. The log is only useful to the next agent if it stays free of restated diffs.
