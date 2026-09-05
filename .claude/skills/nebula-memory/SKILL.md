---
name: nebula-memory
description: "Record what was just worked on into the MEMORY LOG — an entry file in .claude/memory/entries/, one index line in .claude/MEMORY.md, and any durable trap into .claude/memory/gotchas.md — so the next agent starts with that context instead of rediscovering it. Use at the end of any task that changed code or behavior, or that turned up a non-obvious fact about this repo, the daemon, the TUI, the vendored vt100, or the agent hook dialects. Also use when the user says \"remember this\", \"log this\", or \"write this to memory\"."
user-invocable: true
---

Nebula keeps a shared, committed MEMORY LOG. It has three layers, and only the first two are read by
every session — the third is fetched when it matters:

| Layer | File | Who reads it | Cap |
|---|---|---|---|
| **Index** | `.claude/MEMORY.md` — one line per entry, newest first | every session, in full | 200 lines |
| **Standing gotchas** | `.claude/memory/gotchas.md` — the durable traps, grouped by TERM | every session, in full | 300 lines |
| **Entries** | `.claude/memory/entries/YYYY-MM-DD-<slug>.md` — the full Asked / Did / Gotchas | the RECALL HOOK injects the matching ones per prompt; agents open the rest by index line | none |

Index lines that fall off the bottom move to `.claude/memory/archive.md`, which the RECALL HOOK still
searches. `make memory-check` (part of `make ci`) fails the build when a cap is exceeded or the index
and the entry files disagree, so the caps are the rule, not a request.

Your job here is the write: one entry file, one index line, and — when the task left a durable trap
behind — a line in the standing gotchas.

## When to write an entry

Write one when the task is done and it left something durable behind:

- code changed, or behavior changed
- a bug was diagnosed — especially if the cause was not where the symptom was
- you hit something surprising: a flaky test, a platform quirk, an agent CLI that lies about its state, a build step that has to happen in a particular order
- the user made a decision worth not relitigating ("we're not doing X because Y")

**Do not write an entry** for: pure questions you answered without changing anything, trivial one-line edits with no surprise in them, plain git or `gh` housekeeping of finished work (a commit, push, merge, PR or branch cleanup that surfaced nothing new — the PR ARCHIVE already records the merge), or anything the code and `git log` already say plainly. An entry that restates a diff costs every future retrieval tokens and teaches nothing.

If you found nothing durable, say so and skip the write. That is a valid outcome.

## 1. The entry file

Create `.claude/memory/entries/YYYY-MM-DD-<slug>.md` — the date from `date +%F` (do not guess it), the
slug the title in lower-kebab-case, at most ~60 characters. Never overwrite another entry's file.

```markdown
# <Short Title Case summary, in TERMS> — YYYY-MM-DD

**Asked:** The user's original prompt, quoted verbatim — the words they actually typed, not your
restatement of it. Trim a long prompt with an ellipsis rather than paraphrasing it. A future agent
matches against the *request*, so the user's own vocabulary is the part that has to survive.
→ refined: the REFINED PROMPT that PROMPT DADDY logged, verbatim, with each question it asked and the
answer in a following parenthesis ("(asked: which 'done' → UNSEEN)"). Omit the line if it skipped.

**Did:** What actually changed. Name the files and functions (`crates/nebula-tui/src/app.rs:412`). If you
rejected an approach, say which and why in one clause. One clause on the gate ("nebula-tui 499 passed").

**Gotchas:** The non-obvious parts — what bit you, what looked right but wasn't, what has to happen in a
specific order, what a test or tool reported misleadingly. One bullet each. Omit the section entirely if
there genuinely were none.

**Corrections:** N — the number of user turns after your first reply that changed the work (a "no, I
meant…", a re-scope, a rejected approach). Omit when 0. This is the loop's own quality signal.
```

Rules for the content:

- **Verify before you write it.** Only record what you actually observed — a test that ran, output you read, a file you opened. Never record a fix you did not confirm, and never write "should work."
- **Write the gotcha, not the task.** "Rebuilt the release binary" is noise. "Overwriting `~/.cargo/bin/nebula` in place gets the process SIGKILLed by macOS — cp to a temp name and mv" is the entry.
- **Be specific enough to act on.** Paths, function names, exact flags, exact error strings. A future agent should be able to grep for what you wrote.
- **Keep it to what you'd want handed to you** — a few lines per section, not a transcript.
- **Use the TERMS** in caps, as `TERMS.md` spells them, in the title and the Did / Gotchas lines. The index line and the RECALL HOOK both key off them.

## 2. The index line

Prepend one line directly under `## Index` in `.claude/MEMORY.md` (newest first), in exactly this shape —
`make memory-check` parses it:

```markdown
- YYYY-MM-DD · [<Title>](memory/entries/YYYY-MM-DD-<slug>.md) · TERMS: <A>; <B>; <C> · files: <a.rs>; <b.rs> · gotchas: <N>
```

- **TERMS:** up to six TERMS the entry is *about* — the ones a future prompt on this subject would use,
  not every TERM the text happens to mention. The RECALL HOOK matches the prompt's words onto these, so a
  poor TERMS cell is an entry nobody is ever shown again.
- **files:** up to four file basenames the change lives in (`status.rs; hooks/mod.rs`), so a prompt that
  names a file finds the entry too.
- **gotchas:** the bullet count in the entry.

Then `python3 .claude/memory/recall_eval.py --entry <the entry file>`: it runs the RECALL HOOK on the
entry's own **Asked** prompt and prints the rank the entry gets. `> 5` means a future prompt in those
words will not be shown it — add the alias-reachable TERMS to the cell (or the user's word to
`TERMS.md`) until it ranks. `make recall-eval` (part of `make ci`) gates the whole log the same way.

If the index is at its cap, move the oldest lines (from the bottom) to `.claude/memory/archive.md` — same
line shape, under its `## Archived index` header — until it fits. Do not delete index lines; the archive
is still searched.

## 3. Standing gotchas

`.claude/memory/gotchas.md` holds the traps that outlive their task: a platform or tool quirk, a required
ordering, a signal that lies, a settled decision. It is grouped by TERM (`## STOP GATE`, `## E2E PTY`, …),
one line per trap:

```markdown
- <the trap, one sentence, exact symbols and flags in backticks> ⟵ YYYY-MM-DD <entry-slug>[ · re-hit ×N YYYY-MM-DD][ · retire: <test or hook>]
```

After writing the entry, walk its Gotchas once:

- **Durable?** (would an agent on this subsystem step on it again next month?) → add a line under its
  TERM's group, or create the group. Change-specific detail stays in the entry file only.
- **Already there?** — you hit a trap `gotchas.md` already lists: do not add a twin. Append
  `· re-hit ×N YYYY-MM-DD` to the existing line (bump N). A line at ×2 or more is a gotcha memory is
  failing to prevent: turn it into a regression test, a doc comment at the trap site, or a rule in the
  GUARD HOOK (`.claude/hooks/guard.py`) as part of *this* task if it is small, and say so in Did.
- **Enforced now?** — a test, a type, or a GUARD HOOK rule now prevents it: delete the line. An enforced
  gotcha is not a gotcha. Name the enforcement in the entry's Did so the deletion is traceable.
- **retire:** — when you know what *would* enforce it but did not build it, name it on the line
  (`· retire: e2e test for the drain grace`) so the next agent in that file can.

## Updating instead of appending

Before writing, read the index and grep the entries (`grep -ril '<symbol or TERM>' .claude/memory/entries`).
If one already covers this ground:

- **Superseded** — the old entry is now wrong (the code moved, the workaround is no longer needed): edit that entry file in place and note what changed at the top of Did. Do not leave a stale entry standing next to a correct one; the next agent has no way to tell which one to trust. Fix or delete its gotchas lines too.
- **Extended** — you learned more about the same thing: fold it into the existing entry rather than adding a near-duplicate. Update its index line's counts.
- **Genuinely new** — write a new entry.

Delete entries you discover are flatly wrong (file, index line, gotchas lines). A wrong memory is worse than a missing one.

## Size discipline

The caps are enforced by `make memory-check`: index 200 lines, standing gotchas 300 lines. Run it after
you write (`make memory-check`). Over the gotchas cap, prune before you add — in this order, each one a
line freed:

1. **Enforced.** `grep -n 'retire:' .claude/memory/gotchas.md` — a line whose named test, hook rule or
   type now exists is done; delete it. Then every line whose trap a GUARD HOOK rule (`guard.py::RULES`)
   already blocks — probe the rule with the trap if unsure.
2. **Twins.** Two lines under one TERM about the same subject (two `CARGO_TARGET_DIR` lines, two "other
   sessions' hunks" lines) — one line, the more specific text kept, both `⟵` sources listed.
3. **Migration-era.** A line about a state that cannot recur (a session "still holding the old skill" a
   week after the split) — delete; its entry file keeps the history.
4. **Change-specific detail** — back to its entry file; the standing line keeps the one-sentence trap.

Over the index cap, archive from the bottom.

Then confirm to the user in one line what you recorded — the entry title, the gotcha count, any standing
gotcha added / re-hit / retired — so they can tell you if you logged the wrong lesson.
