# Nebula

## Project memory and vocabulary

This repo keeps a shared, committed memory that every agent and every session reads and maintains:

- **The MEMORY LOG**, in three layers, written by the `nebula-memory` skill:
  - `.claude/MEMORY.md` — the **index**: one line per task (date, title, the TERMS and files it is
    about, its gotcha count), newest first, capped at 200 lines. Read in full.
  - `.claude/memory/gotchas.md` — the **standing gotchas**: the traps that outlive their task, one line
    each, grouped by TERM, capped at 300 lines. Read in full.
  - `.claude/memory/entries/<date>-<slug>.md` — the **entries**: the full Asked / Did / Gotchas of each
    task. Not read wholesale: the RECALL HOOK (`.claude/hooks/recall.py`, a `UserPromptSubmit` hook)
    injects the ones that match the prompt's TERMS and file names as `[nebula recall] …` context, and
    you open any other whose index line matches what you are working on.
  - `make memory-check` (part of `make ci`) fails when a cap is exceeded or the index and the entry
    files disagree; the caps are enforced, not advisory.
- **`TERMS.md`** — the glossary: one ALL-CAPS canonical name per feature, panel, key, CLI command, hook
  route, daemon mechanism, status and dev workflow, with what the user calls it and where it lives in
  the code. Written by the `project-terms` skill.
- **The GUARD HOOK** (`.claude/hooks/guard.py`, a `PreToolUse` hook on Bash) — gotchas that kept
  re-hitting, turned into blocked commands with the right way fed back. If it blocks you, do what it says.

### Before you start a task

**Read `.claude/MEMORY.md`, `.claude/memory/gotchas.md` and `TERMS.md` first**, before touching code
or planning an approach, and read whatever the RECALL HOOK injected under `[nebula recall]`.

Scan the index for entries related to what the user is asking — the same TERMS, the same crate, the
same symptom, the same file — and open those entry files (`.claude/memory/entries/…`); grep the
entries (`grep -ril '<symbol or TERM>' .claude/memory/entries`) when the index line is not enough.
Match on the user's vocabulary as well as yours; entries record the original request in the user's own
framing for exactly this reason. When an entry or a standing gotcha is related, fold its context into
how you work:

- a recorded gotcha is a mine already stepped on — do not step on it again
- a recorded decision ("we're not doing X because Y") is settled unless the user reopens it
- a recorded fix tells you where the code that matters actually lives

Then map every noun in the prompt onto `TERMS.md`: the **Alias index** at the bottom turns the user's
words ("top nav", "locked layer", "done") into the TERM they mean, and the TERM's row tells you where
that thing lives. If a word maps to two TERMS, that is the ambiguity `prompt-daddy` has to ask about.

All of it describes what was true when it was written. If an entry or a gotcha names a file, function,
or flag, confirm it still exists before you rely on it, and correct it if it has gone stale.

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
- in MEMORY LOG entries — the title, the **Did** and **Gotchas** lines and the index line's TERMS cell
  especially, so the log stays greppable and the RECALL HOOK can find the entry again
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

It rewrites the prompt once, into its best fully specified version — the gaps the original left open
closed (an ambiguous word like "done" or "move", an unstated "keep X as-is", a missing why, a bug report
without its evidence), the user's aliases replaced by the TERMS they map to. It asks the user **only**
for context the work cannot proceed without — a who / what / when / where / why / how that neither the
prompt, the memory log, the glossary nor a quick grep can fill — in one `AskUserQuestion`, then folds
the answers in. It logs the final prompt in the chat (`Refined prompt:` + the text as a quote) and
proceeds on it at once; it never asks whether the rewrite is right. **The refined prompt is the
request you work from.**

Run it on every new prompt that is a task: features, bug reports, refactors, "debug this", and a
question that is a task in disguise ("why is X broken"). The skill lists the cases it skips on its own —
a reply to a question you asked, a bare confirmation, a mid-task correction that is already specific, a
slash-command or skill trigger like "commit push release", and a **pure question** that changes nothing
(an explanation, an assessment, "what does X do"): answer that directly, in TERMS, grounded by what the
RECALL HOOK injected.
In headless runs it still rewrites and logs, but writes its questions into the prompt as stated
assumptions instead of asking.

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

It fixes the reply's shape to three sections in this order: `==== YOU ASKED ====` (the refined prompt
`prompt-daddy` logged, verbatim — only the rewrite), `==== OVERVIEW ====` (what happened, in a few
plain sentences a reader can stop after), and `==== TECHNICAL OVERVIEW ====` (the details, kept short
enough that the user asks for more rather than skims) — plus `==== ACTION REQUIRED ====` between the
overview and the technical section, present if and only if the user must do something before the work
is complete (run a command, flip a setting, restart, decide, approve): numbered imperative steps with
the exact command. Use it on every kind of reply — a feature, a bug fix, a question, a recommendation,
a release. A pure question that changed nothing takes the short form: `YOU ASKED` (the prompt as typed)
and `OVERVIEW` (the answer), with `TECHNICAL OVERVIEW` only when there are details beyond the answer.
The only text outside it is the one-line "about to" preamble, mid-task progress notes, and
`AskUserQuestion` prompts; the skill lists those exceptions.

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
