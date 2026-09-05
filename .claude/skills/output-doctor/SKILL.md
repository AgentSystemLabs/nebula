---
name: output-doctor
description: "Shape every reply to the user into one fixed layout of up to four sections, each opened by a horizontal rule and a markdown heading with its own emoji so a human can tell them apart at a glance — 💬 ==== YOU ASKED ==== (present if and only if a REFINED PROMPT was logged: that prompt, verbatim), 📋 ==== OVERVIEW ==== (always: what happened, a few plain sentences), 🔧 ==== DETAILS ==== (present only when there is a mechanism, a file:line or a gotcha the overview could not carry), 👉 ==== NEXT STEPS ==== (always present, always last: blocking steps first, each marked 🚧 **Blocking:** with the exact command, then the hand-off — commit, PR, a question, a decision — or the single line \"Nothing — this is done.\") — so a reply can be read top-down and stopped at any line. Use before writing the reply that answers or closes any request: features, bug fixes, questions, refactors, releases, recommendations. Also use when the user says \"output doctor\", \"format this\", \"use the output format\", or \"rewrite this in the format\"."
user-invocable: true
---

The reply is the part of the task the user actually reads. Everything else — the grep, the build, the
test run — is scaffolding they see a few lines of. A reply that opens with a file path, or with a
paragraph that mixes the headline with the mechanism, makes the user do the sorting. This skill puts
the sorting on you: what they asked (when it was rewritten), what happened, how (when it matters), then
what is left for them with the blocking steps first — in that order, under fixed headers that look
different from the text around them, so they can stop reading the moment they have what they need.

## When to run it

Right before you write the reply that answers or closes a request — after the work is done, after
`nebula-memory` and `project-terms` have run (their one-line results fold into DETAILS). Every kind of
request: a feature, a bug fix, a question, a refactor, a release, a recommendation, a diagnosis that
changed nothing.

It does **not** govern:

- the one-line "I'll start by…" preamble and the brief progress notes you post mid-task
- an `AskUserQuestion` — its options and previews keep their own shape
- text that lives in files: commits, PR bodies, memory entries, `TERMS.md`, code comments
- a reply that is itself only a question back to the user, or a one-word acknowledgement of a
  mid-task correction you are about to act on

When in doubt, use the format. A reply that is short is fine; a reply that is shapeless is not.

## The format

Four sections in a fixed order. Two are always present (`OVERVIEW`, `NEXT STEPS`); two are conditional
(`YOU ASKED`, `DETAILS`) and are **absent**, not empty, when their condition fails. `NEXT STEPS` is
always last:

```
---

## 💬 ==== YOU ASKED ====

"<the REFINED PROMPT prompt-daddy logged, verbatim — only when one exists>"

---

## 📋 ==== OVERVIEW ====

<what happened, at a glance>

---

## 🔧 ==== DETAILS ====

<the mechanism, the file:line, the gotcha to know now — only when there is one>

---

## 👉 ==== NEXT STEPS ====

1. 🚧 **Blocking:** <a step only the user can take, with the exact command — first, only when there is one>
2. <the hand-off: commit, PR, a question, a decision — or the single line "Nothing — this is done.">
```

The look, which is what lets a human tell the sections apart in a terminal that renders markdown but
carries no color of its own:

- every section opens with a horizontal rule (`---` on its own line, blank lines on both sides) and
  then a `##` heading — the rule is the space, the heading is the weight
- the heading is the section's emoji, then the name between four `=` on each side, exactly as above:
  `## 📋 ==== OVERVIEW ====`. The emoji is the color; keep the same one per section every time so the
  eye learns it. Never a different emoji, never a different count of `=`
- a blank line under every heading, a blank line above every rule
- the blocking steps in `NEXT STEPS` carry `🚧 **Blocking:**` as their lead so they stand out from the
  hand-off below them
- no title above the first rule, no sign-off below the last section — `NEXT STEPS` ends the reply

An offer of follow-up work goes in the last line of `DETAILS`, or of `OVERVIEW` when there is no
`DETAILS` — never in `NEXT STEPS`.

**Which sections a reply has**, by the kind of request:

- a pure question that changed nothing (an explanation, an assessment, "what does X do" — the prompts
  PROMPT DADDY skips): `OVERVIEW` carries the whole answer; `NEXT STEPS` is "Nothing — this is done."
  unless the answer left the user a decision or a question. **Two sections is a complete reply**; add
  `DETAILS` only when there is a `file:line` beyond the answer, and never pad a question into four.
- a task worked from a REFINED PROMPT: `YOU ASKED`, `OVERVIEW`, usually `DETAILS` (code changed, so
  there is a file to jump to), `NEXT STEPS`.
- work started by a confirmation, a mid-task correction, or a skill trigger: `OVERVIEW`, `DETAILS` when
  earned, `NEXT STEPS` — no `YOU ASKED` unless the confirmation picked up a REFINED PROMPT logged
  earlier in the session (see below).

### `## 💬 ==== YOU ASKED ====`

Present **if and only if** a REFINED PROMPT was logged for the work this reply closes — in this turn,
or in the earlier turn that a bare confirmation ("yes do it", "the second one") picked up. Content: that
prompt, verbatim, in double quotes — the text under `prompt-daddy`'s `Refined prompt:` line, stated
assumptions and all. Only the rewrite: not the original beside it, not the questions it asked, no
`→ refined:` prefix. Those belong in the memory entry, not here. If the user corrected the REFINED PROMPT
mid-task, quote it with the correction applied, so the section still shows the request the work
answered.

Absent when `prompt-daddy` skipped itself — a question, a reply to a question you asked, a mid-task
correction, a skill trigger. The message the section would quote is sitting directly above the reply,
and an echo is not a check. Never paraphrase the prompt and never improve it here: this section exists
so the user can check the reply against the request without scrolling up.

### `## 📋 ==== OVERVIEW ====`

Always present. What the change was — or, for a question, what the answer is — at the altitude of a
commit subject line stretched to a short paragraph or a numbered list of at most five items. Rules:

- plain sentences; no file paths, no symbols, no line numbers
- name the TERMS from `TERMS.md` in caps, as `CLAUDE.md` requires
- if code changed, one clause on the gate: "687 tests green" / "the e2e suite was not run because…"
- if anything was left out, blocked, or scaled down, say so **here**, not buried below
- if nothing changed (a question, a diagnosis, a recommendation), open with that: "No code changed."

A reader who stops after this section should know the outcome and *whether* anything still needs them.
*What* they must do — the blocking steps and the hand-off — is `NEXT STEPS`, at the bottom, which is
what is on screen when the reply lands.

### `## 🔧 ==== DETAILS ====`

Present only when there is something `OVERVIEW` could not carry: a mechanism worth a sentence, a
`file:line` the user would jump to, a rejected approach they would otherwise ask about, a gotcha they
must know *now*. Absent for a question whose answer fit in `OVERVIEW` and for a change whose diff says
it all. Err on the side of **too little**: the user can ask for more, and a reply they have to scroll is
a reply they skim. Rules:

- bullets with a bold lead-in, in `OVERVIEW`'s order — but only for the items that have something
  beyond `OVERVIEW`; an item the overview already covered gets no bullet group
- `file:line` references where they help the user jump to the code — clickable, so use the real path
- the mechanism in one or two sentences per item; skip what the diff already shows
- rejected approaches only when the user would otherwise ask "why not X"
- gotchas the user should know about *now* (a behavior that changed, a setting they must flip); the
  rest go to `nebula-memory`, not here
- the one-line results of `nebula-memory` and `project-terms` ("logged the entry", "aliased 'top nav'
  to WORKSPACES BAR, promoted nothing") close the section; when they would be its only content, leave
  the section out — a question logs nothing worth a section
- an offer of follow-up work is the last line

### `## 👉 ==== NEXT STEPS ====`

Always present, always last. Where the user stands now that the reply is done, and what is theirs to
do: the steps the work cannot finish without come first, the hand-off after them. A reader who jumps to
the bottom of the reply finds everything they must do here, and nothing else.

**Blocking steps** lead the list, each opened with `🚧 **Blocking:**`. The test: is there a step you
could not take yourself that the outcome depends on? Then it is a blocking step. Counts:

- a command only they can run — an interactive login, NEBULA KILL / MAKE CYCLE (they take every live
  session down, this one included, so they run from a terminal outside nebula), a call that was denied
  or needs their terminal — offered in the `! <command>` form where its output should land in the session
- a setting they must flip, a process they must restart, a tool they must install or upgrade
- a decision you are blocked on that an `AskUserQuestion` did not settle, stated as the choice to make
- an approval for a destructive or outward-facing step you did not take without it
- a check only they can do that the work cannot be called done without — a manual test in their
  terminal, a look at a screenshot, a review before a merge

The exact command or setting goes in a code block on the step; the *why* is one clause on the step or
lives in `DETAILS`. Two or three blocking steps is typical — a longer list is usually a task you should
have finished.

**The hand-off** follows the blocking steps, or opens the list when there are none:

- a git hand-off: "Good to commit — `git add -A && git commit`", "Ready for a PR from branch X",
  "Nothing to commit"
- a question the user still owes an answer to — restated, not pointed at
- a command to run, in the `! <command>` form when its output should land in the session
- a decision the user must make — the options in one line
- a verification that is theirs but does not gate the work — a look at the result, a manual check
- when there is genuinely nothing left, the single line "Nothing — this is done." — that line alone,
  never an empty section and never a header without a body. This is the one section with an empty-state
  line, because the bottom of the reply is what the terminal shows when it lands

Does not count, and so is not an item:

- an offer of optional follow-up work ("I can also…") — the last line of `DETAILS`; never pad this
  section with offers
- a step you could still take yourself — take it, then reply
- a restatement of what changed, or a gotcha they only need to *know* — `OVERVIEW` and `DETAILS`
  already carry those
- something left out or scaled down — `OVERVIEW` says so; it becomes a step here only if finishing it
  needs them

Shape: a single line, or a numbered list of one to four items in the order to do them, in the user's
terms — the TERMS from `TERMS.md`, not file paths. No prose above the list, no sign-off below it — this
is the end of the reply.

## Budget

Measured over ten sessions on 2026-09-04, replies in this shape ran 550 to 1,770 words — three to
five screens the user scrolls past to reach NEXT STEPS. The shape is not a licence to write more.
Count before you send:

- a task reply: under 350 words in all; a question: under 150; a diagnosis that leaves a 🚧 step: under 250. YOU ASKED is a quote and is not counted
- OVERVIEW: one paragraph, or a list of at most five items — under 120 words
- DETAILS: at most five bullets of at most two sentences each; the diff carries the rest
- NEXT STEPS: at most four items

Over budget, cut the DETAILS bullets the diff already shows, then the OVERVIEW sentences that restate
them. Never cut a 🚧 step, a "left out" sentence, or the skills' one-line results.

## Worked examples

One, the pure-question shape (the task and blocked-task examples stood here until 2026-09-05; the
format block and "Which sections a reply has" already determine those and they were cut):

A pure question — two sections, no `YOU ASKED` because PROMPT DADDY skipped it, no `DETAILS` because
the answer fit:

```
---

## 📋 ==== OVERVIEW ====

No code changed. The DONE BADGE counts UNSEEN SESSIONS — the ones that finished while you weren't
looking — and clears each one the moment you focus it. A FINISHED SESSION you have already looked at
is not counted.

---

## 👉 ==== NEXT STEPS ====

Nothing — this is done.
```
