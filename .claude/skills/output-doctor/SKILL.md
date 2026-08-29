---
name: output-doctor
description: "Shape every reply to the user into one fixed layout — ==== YOU ASKED ==== (the refined prompt prompt-daddy logged, verbatim), ==== OVERVIEW ==== (what happened, a few plain sentences), ==== ACTION REQUIRED ==== (present if and only if the user must do something before the work is complete: numbered steps, exact commands), ==== TECHNICAL OVERVIEW ==== (the details, kept short), ==== NEXT STEPS ==== (always present, always last: what is left for the user to do — commit, PR, a question, a command, a decision, or nothing) — so a reply can be read top-down and stopped at any line. Use before writing the reply that answers or closes any request: features, bug fixes, questions, refactors, releases, recommendations. Also use when the user says \"output doctor\", \"format this\", \"use the output format\", or \"rewrite this in the format\"."
user-invocable: true
---

The reply is the part of the task the user actually reads. Everything else — the grep, the build, the
test run — is scaffolding they see a few lines of. A reply that opens with a file path, or with a
paragraph that mixes the headline with the mechanism, makes the user do the sorting. This skill puts
the sorting on you: what they asked, what happened, what they must do (only when they must), how, then
what is left for them — in that order, under fixed headers, so they can stop reading the moment they
have what they need.

## When to run it

Right before you write the reply that answers or closes a request — after the work is done, after
`nebula-memory` and `project-terms` have run (their one-line results fold into the technical section). Every
kind of request: a feature, a bug fix, a question, a refactor, a release, a recommendation, a diagnosis
that changed nothing.

It does **not** govern:

- the one-line "I'll start by…" preamble and the brief progress notes you post mid-task
- an `AskUserQuestion` — its options and previews keep their own shape
- text that lives in files: commits, PR bodies, memory entries, `TERMS.md`, code comments
- a reply that is itself only a question back to the user, or a one-word acknowledgement of a
  mid-task correction you are about to act on

When in doubt, use the format. A reply that is short is fine; a reply that is shapeless is not.

## The format

Four sections, these exact headers, this order, nothing before the first and nothing after the last —
plus a fifth, `ACTION REQUIRED`, between `OVERVIEW` and `TECHNICAL OVERVIEW`, that is present **if and
only if** the reply needs the user to do something. `NEXT STEPS` is always present and always last:

```
==== YOU ASKED ====
"<the refined prompt prompt-daddy logged, verbatim>"

==== OVERVIEW ====
<what happened, at a glance>

==== ACTION REQUIRED ====
1. <one imperative step, with the exact command — only when the user must act>

==== TECHNICAL OVERVIEW ====
<the details, simplified>

==== NEXT STEPS ====
<what is left for the user: commit, PR, a question, a command, a decision — or "Nothing — this is done.">
```

Four `=` on each side of every header. Blank line under each header. No title above `YOU ASKED`, no
sign-off below `NEXT STEPS` — it is the last section, and nothing follows it. An offer of follow-up
work goes in the last line of the technical section if it goes anywhere, never under `ACTION REQUIRED`
and never under `NEXT STEPS`.

**The short form, for a pure question that changed nothing** (an explanation, an assessment, "what does
X do" — the prompts PROMPT DADDY skips): `YOU ASKED` quotes the prompt as typed, `OVERVIEW` carries the
whole answer, `TECHNICAL OVERVIEW` appears only when there are details beyond the answer worth a
`file:line`, and `NEXT STEPS` is still present — usually the single line "Nothing — this is done.",
unless the answer left the user a decision to make or a question to answer. Three sections is a
complete reply; do not pad a question into four.

### `==== YOU ASKED ====`

One quoted prompt: **the refined prompt `prompt-daddy` logged**, verbatim, in double quotes — the text
under its `Refined prompt:` line, stated assumptions and all. Only the rewrite — not the original beside
it, not the questions it asked, no `→ refined:` prefix. Those belong in the memory entry, not here.

When there was no rewrite — `prompt-daddy` skipped itself (a mid-task correction, a confirmation, a
skill trigger) — quote the user's message you actually worked from, as typed. If the user corrected the
refined prompt mid-task, quote the refined prompt with the correction applied to it, so the section
still shows the request the work answered.

Never paraphrase it and never improve it here. This section exists so the user can check the reply
against the request without scrolling up.

### `==== OVERVIEW ====`

What the change was — or, for a question, what the answer is — at the altitude of a commit subject
line stretched to a short paragraph or a numbered list of at most five items. Rules:

- plain sentences; no file paths, no symbols, no line numbers
- name the TERMS from `TERMS.md` in caps, as `CLAUDE.md` requires
- if code changed, one clause on the gate: "687 tests green" / "the e2e suite was not run because…"
- if anything was left out, blocked, or scaled down, say so **here**, not buried below
- if nothing changed (a question, a diagnosis, a recommendation), open with that: "No code changed."

A reader who stops after this section should know the outcome and *whether* anything still needs them;
*what* they must do before the work is complete is the next section, which exists only when the answer
is yes, and what is left for them once it is complete is the last section, `NEXT STEPS`.

### `==== ACTION REQUIRED ====`

Present **if and only if** the work cannot be complete until the user does something. The test: is
there a step you could not take yourself that the outcome depends on? Then the section is there.
Otherwise it is not — no empty section, no "nothing required" placeholder, no header without steps.

Counts, and so creates the section:

- a command only they can run — an interactive login, NEBULA KILL / MAKE CYCLE (they take every live
  session down, this one included, so they run from a terminal outside nebula), a call that was denied
  or needs their terminal — offered in the `! <command>` form where its output should land in the session
- a setting they must flip, a process they must restart, a tool they must install or upgrade
- a decision you are blocked on that an `AskUserQuestion` did not settle, stated as the choice to make
- an approval for a destructive or outward-facing step you did not take without it
- a check only they can do — a manual test in their terminal, a look at a screenshot, a review before
  a merge

Does not count, and so does not create the section:

- an offer of optional follow-up work ("I can also…") — the last line of the technical section
- a gotcha or a behavior that changed which they only need to *know* — the technical section
- something left out or scaled down — the overview says so; it becomes a step here only if finishing
  it needs them
- anything you could do yourself but have not — do it, then reply

Shape: a numbered list, one imperative step per item, in the order to do them, the exact command or
setting in a code block. No prose above the list; the *why* is one clause on the step, or lives in the
technical section. Two or three steps is typical — a longer list is usually a task you should have
finished.

### `==== TECHNICAL OVERVIEW ====`

The details, for the reader who kept going. Err on the side of **too little**: the user can ask for
more, and a reply they have to scroll is a reply they skim. Rules:

- one bullet group per item in the overview, in the same order, with a bold lead-in
- `file:line` references where they help the user jump to the code — clickable, so use the real path
- the mechanism in one or two sentences per item; skip what the diff already shows
- rejected approaches only when the user would otherwise ask "why not X"
- gotchas the user should know about *now* (a behavior that changed, a setting they must flip); the
  rest go to `nebula-memory`, not here
- the one-line results of `nebula-memory` and `project-terms` ("logged the entry", "aliased 'top nav' to
  WORKSPACES BAR, promoted nothing") close the section, and an offer of follow-up work is the last line

### `==== NEXT STEPS ====`

Always present, always last. `ACTION REQUIRED` is a gate — what the work cannot finish without; this is
the hand-off — where the user stands now that the reply is done, and what, if anything, is theirs to do
next. A reader who jumps to the bottom of the reply should find that here, and nothing else.

Counts, and so is an item:

- a git hand-off: "Good to commit — `git add -A && git commit`", "Ready for a PR from branch X",
  "Nothing to commit"
- a question the user still owes an answer to — restated, not pointed at
- a command to run, in the `! <command>` form when its output should land in the session
- a decision the user must make — the options in one line
- a verification only they can do — a manual test in their terminal, a look at a screenshot, a review
  before a merge
- when there is genuinely nothing left, the single line "Nothing — this is done." — that line alone,
  never an empty section and never a header without a body

Does not count, and so is not an item:

- an offer of optional follow-up work ("I can also…") — the last line of the technical section, as
  today; never pad this section with offers
- a step you could still take yourself — take it, then reply
- a restatement of what changed, or a gotcha they only need to *know* — the overview and the technical
  section already carry those

Shape: a single line, or a numbered list of one to four items in the order to do them, in the user's
terms — the TERMS from `TERMS.md`, not file paths. When `ACTION REQUIRED` is present, do not repeat its
commands here: the first item points at it in one line ("Do the ACTION REQUIRED restart above, then …")
and the rest is what comes after it. No prose above the list, no sign-off below it — this is the end of
the reply.

## Worked example

From the recommendation that produced this skill (2026-08-28), condensed:

```
==== YOU ASKED ====
"Survey nebula's whole feature surface and recommend the 3 features that cost the most — lines of
code, protocol/store surface, recorded gotchas in MEMORY.md — relative to what they actually deliver.
For each: what it does, why it's a poor trade, roughly how much code goes away, and what else depends
on it. Recommend only; don't remove anything yet."

==== OVERVIEW ====
No code changed. I surveyed the README, CLI, protocol, overlays, per-file line counts and the gotcha
history in `.claude/MEMORY.md`, then ranked features by cost against delivered value. Three stand out:

1. **CLAUDE CLOUD sessions** — ~1,800 lines to fake a live view of something the CLI won't let you attach to.
2. **`nebula browser` + `nebula tunnel`** — ~1,000 lines for a second remote path that duplicates `nebula ssh`.
3. **The PREWARM POOL** — ~800 lines and up to ~1.7 GB of idle `claude` processes to save ~2s on `n`.

Cutting all three removes roughly 3,500 lines, one external dependency, and needs one protocol bump.

==== TECHNICAL OVERVIEW ====

**1. CLAUDE CLOUD sessions**
- *What:* `Tab` on the Claude picker launches `claude --cloud=<task>`; the daemon re-runs
  `claude --teleport=<id>` every 45s to mirror it, because live attach is refused for this account.
- *Why cut:* three protocol bumps, a store migration, three memory entries of CLI workarounds.
- *Goes away:* `crates/nebula-daemon/src/pty/cloud.rs`, ~600 lines of `registry.rs`, ~490 lines of e2e.
- *Dependents:* the multiline prompt editor has only cloud callers — keep or drop.

**2. …**

==== NEXT STEPS ====
1. Decide which of the three to cut first; say the name and I'll start with it.
```

The original reply had the same content in prose that opened with a heading and a rationale paragraph;
the user asked for it twice more before it had this shape. Start with the shape.

A second, hypothetical and condensed, showing the conditional section — a DAEMON change that the
running daemon cannot pick up without the user:

````
==== YOU ASKED ====
"Make the HOOK RECEIVER accept the Cursor dialect's camelCase `stop` event so Cursor sessions turn
FINISHED instead of sticking on RUNNING."

==== OVERVIEW ====
The HOOK RECEIVER now maps Cursor's `stop` to the same FINISHED transition as Claude's `Stop`; 690
tests green and MAKE INSTALL has run. The live DAEMON is still the old build, so nothing changes on
screen until it is restarted — and that restart is yours to run.

==== ACTION REQUIRED ====
1. From a terminal outside nebula (it stops every live session, this one included), restart the daemon:
   ```
   nebula kill && nebula
   ```

==== TECHNICAL OVERVIEW ====
…

==== NEXT STEPS ====
1. Do the ACTION REQUIRED restart above.
2. Good to commit once a Cursor session shows FINISHED in the SESSIONS PANEL.
````

Had MAKE INSTALL been the only step left, the reply would have run it and carried no `ACTION REQUIRED`
at all — the section is for what the user must do, not for what you could still do — and `NEXT STEPS`
would have opened with the commit hand-off instead of pointing at a restart.
