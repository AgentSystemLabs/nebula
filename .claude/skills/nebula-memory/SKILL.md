---
name: nebula-memory
description: "Record what was just worked on into .claude/MEMORY.md — the original request, how it was fixed or implemented, and the gotchas hit along the way — so the next agent starts with that context instead of rediscovering it. Use at the end of any task that changed code or behavior, or that turned up a non-obvious fact about this repo, the daemon, the TUI, the vendored vt100, or the agent hook dialects. Also use when the user says \"remember this\", \"log this\", or \"write this to memory\"."
user-invocable: true
---

Nebula keeps a shared, committed work log at **`.claude/MEMORY.md`**. Every agent reads it before starting and appends to it after finishing. Your job here is the append.

## When to write an entry

Write one when the task is done and it left something durable behind:

- code changed, or behavior changed
- a bug was diagnosed — especially if the cause was not where the symptom was
- you hit something surprising: a flaky test, a platform quirk, an agent CLI that lies about its state, a build step that has to happen in a particular order
- the user made a decision worth not relitigating ("we're not doing X because Y")

**Do not write an entry** for: pure questions you answered without changing anything, trivial one-line edits with no surprise in them, or anything the code and `git log` already say plainly. A memory file full of restated diffs is worse than an empty one — it costs every future agent tokens and teaches them nothing.

If you found nothing durable, say so and skip the write. That is a valid outcome.

## Entry format

Append to the **top** of the `## Entries` section in `.claude/MEMORY.md` (newest first). Get the date with `date +%F` — do not guess it.

```markdown
### <Short Title Case summary> — YYYY-MM-DD

**Asked:** The user's original prompt, quoted verbatim — the words they actually typed, not your
restatement of it and not the refined version you arrived at after the back-and-forth. Trim a long prompt
with an ellipsis rather than paraphrasing it, and if the task only came into focus over several messages,
quote the first prompt and add the correction underneath. A future agent matches against the *request*, so
the user's own vocabulary is the part that has to survive.

**Did:** What actually changed. Name the files and functions (`crates/nebula-tui/src/app.rs:412`). If you rejected an approach, say which and why in one clause.

**Gotchas:** The non-obvious parts — what bit you, what looked right but wasn't, what has to happen in a specific order, what a test or tool reported misleadingly. One bullet each. Omit this line entirely if there genuinely were none.
```

Rules for the content:

- **Verify before you write it.** Only record what you actually observed — a test that ran, output you read, a file you opened. Never record a fix you did not confirm, and never write "should work."
- **Write the gotcha, not the task.** "Rebuilt the release binary" is noise. "Overwriting `~/.cargo/bin/nebula` in place gets the process SIGKILLed by macOS — cp to a temp name and mv" is the entry.
- **Be specific enough to act on.** Paths, function names, exact flags, exact error strings. A future agent should be able to grep for what you wrote.
- **Keep it to what you'd want handed to you** — a few lines per section, not a transcript.

## Updating instead of appending

Before appending, read the existing entries. If one already covers this ground:

- **Superseded** — the old entry is now wrong (the code moved, the workaround is no longer needed): edit that entry in place and note what changed. Do not leave a stale entry standing next to a correct one; the next agent has no way to tell which one to trust.
- **Extended** — you learned more about the same thing: fold it into the existing entry rather than adding a near-duplicate.
- **Genuinely new** — append a new entry.

Delete entries you discover are flatly wrong. A wrong memory is worse than a missing one.

## Size discipline

`.claude/MEMORY.md` is read at the start of every session, so it has to stay readable. When it grows past roughly 300 lines, prune before you append: merge entries that circle the same subsystem, and drop ones whose lesson is now enforced by the code itself (a gotcha that a test or a type now prevents is no longer a gotcha).

## Writing the file

If `.claude/MEMORY.md` does not exist, create it with the header from this repo's `CLAUDE.md` conventions:

```markdown
# Nebula Memory

Work log written by the `nebula-memory` skill. Newest first. Read this before starting a task; append after finishing one.

## Entries
```

Then confirm to the user in one line what you recorded — the title and the gotcha count — so they can tell you if you logged the wrong lesson.
