# Nebula

## Project memory and vocabulary

This repo keeps two shared, committed files that every agent and every session reads and maintains:

- **`.claude/MEMORY.md`** — the work log: one entry per task, recording what was asked, what was
  actually done, and the gotchas hit along the way. Written by the `nebula-memory` skill.
- **`TERMS.md`** — the glossary: one ALL-CAPS canonical name per feature, panel, key, CLI command, hook
  route, daemon mechanism, status and dev workflow, with what the user calls it and where it lives in
  the code. Written by the `project-terms` skill.

### Before you start a task

**Read `.claude/MEMORY.md` and `TERMS.md` first**, before touching code or planning an approach.

Scan the memory log for entries related to what the user is asking — the same crate, the same
subsystem, the same symptom, the same tool. Match on the user's vocabulary as well as yours; entries
record the original request in the user's own framing for exactly this reason. When an entry is
related, fold its context into how you work:

- a recorded gotcha is a mine already stepped on — do not step on it again
- a recorded decision ("we're not doing X because Y") is settled unless the user reopens it
- a recorded fix tells you where the code that matters actually lives

Then map every noun in the prompt onto `TERMS.md`: the **Alias index** at the bottom turns the user's
words ("top nav", "locked layer", "done") into the TERM they mean, and the TERM's row tells you where
that thing lives. If a word maps to two TERMS, that is the ambiguity `prompt-daddy` has to split on.

Both files describe what was true when they were written. If one names a file, function, or flag,
confirm it still exists before you rely on it, and correct the entry if it has gone stale.

### Speak in the project's terms

**All output you produce about this project should use the defined TERMS.** The TERMS are the team's
shared vocabulary — between teammates, between sessions, and between you and the user — and text
written in them is far easier for the team to read than text that reinvents a name for the same thing
each time. A teammate skimming a reply, a commit, or a memory entry should recognize every feature,
panel, key, and mechanism at a glance, without translating your paraphrase back into the name they
already know. So use the TERMS, in ALL CAPS, exactly as `TERMS.md` spells them, in everything you
write about this project:

- in replies, summaries, explanations, plans, and `AskUserQuestion` options — "the WORKSPACES BAR",
  "a LOCKED PANE", "the PREWARM POOL", never a fresh paraphrase of the same thing
- in commit messages, PR descriptions, and release notes
- in `MEMORY.md` entries — the **Did** and **Gotchas** lines especially, so the log stays greppable by
  TERM
- in code comments and doc comments you add
- in anything else you emit — error diagnoses, test-failure write-ups, design notes, TODO lists

Prefer a TERM over a synonym even when the synonym feels more natural in the sentence; a slightly
stiffer sentence that the whole team parses instantly beats a smoother one that only you understand.

When the user uses an alias, answer in the TERM — the first time, with their word beside it so the
mapping is visible ("the WORKSPACES BAR (your 'top nav')"); after that, the TERM alone. When the user
names something that has no TERM yet, say so, propose one, and let `project-terms` ledger it as a
candidate when the task ends — it becomes a TERM once a later task uses it again. Code identifiers are
not TERMS: do not rename symbols, files, config keys, or CLI flags to match the glossary — the glossary
points at them.

### Refine the prompt before acting on it

Once you have read the memory log and the glossary, and **before** planning, grepping the code in
earnest, or answering, invoke the `prompt-daddy` skill on the user's prompt:

```
Skill(skill: "prompt-daddy")
```

It rewrites the prompt three ways — each closing a gap the original left open (an ambiguous word like
"done" or "move", an unstated "keep X as-is", a missing why, a bug report without its evidence) — with
the user's aliases replaced by the TERMS they map to, and asks the user to pick one or keep the
original. **The pick is the request you work from.**

Run it on every new prompt: features, bug reports, questions, refactors, "debug this". The skill lists
the few cases it skips on its own — a reply to a question you asked, a bare confirmation, a mid-task
correction that is already specific, a slash-command or skill trigger like "commit push release", and
headless runs where nobody can pick.

### After you finish a task

Invoke the `nebula-memory` skill to record it:

```
Skill(skill: "nebula-memory")
```

Do this whenever the task changed code or behavior, diagnosed a bug, or turned up something
non-obvious about this repo, the daemon, the TUI, the vendored vt100, or the agent hook dialects. The
skill has the entry format and the rules for what is worth recording — including when the right
answer is to record nothing.

Skip it for pure questions you answered without changing anything, and for trivial edits that held no
surprise. The log is only useful to the next agent if it stays free of restated diffs.

Then — on **every** task, including the ones that recorded no memory entry — invoke the
`project-terms` skill:

```
Skill(skill: "project-terms")
```

It detects the vocabulary the task surfaced and sorts it: any word the user used for an existing TERM
that its row did not list yet is recorded at once, as are renames and retirements; a *new* name goes to
the **Candidates** ledger at the bottom of `TERMS.md` and is promoted to a TERM only when a later,
separate task uses it again. Most runs record a sighting or an alias and promote nothing, and say so in
one line; the alias edits are the ones that make the next prompt land on the first try.

### Before you reply

**Every reply that answers or closes a request goes through the `output-doctor` skill first** —
after `nebula-memory` and `project-terms` have run, and before you write a word of the reply:

```
Skill(skill: "output-doctor")
```

It fixes the reply's shape to three sections in this order: `==== YOU ASKED ====` (the prompt the user
picked in `prompt-daddy`, verbatim — only the pick), `==== OVERVIEW ====` (what happened, in a few
plain sentences a reader can stop after), and `==== TECHNICAL OVERVIEW ====` (the details, kept short
enough that the user asks for more rather than skims). Use it on every kind of reply — a feature, a
bug fix, a question, a recommendation, a release. The only text outside it is the one-line "about to"
preamble, mid-task progress notes, and `AskUserQuestion` prompts; the skill lists those exceptions.
