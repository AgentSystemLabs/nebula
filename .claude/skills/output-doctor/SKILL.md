---
name: output-doctor
description: "Shape every reply to the user into one fixed three-section layout — ==== YOU ASKED ==== (the prompt-daddy pick, verbatim), ==== OVERVIEW ==== (what happened, a few plain sentences), ==== TECHNICAL OVERVIEW ==== (the details, kept short) — so a reply can be read top-down and stopped at any line. Use before writing the reply that answers or closes any request: features, bug fixes, questions, refactors, releases, recommendations. Also use when the user says \"output doctor\", \"format this\", \"use the output format\", or \"rewrite this in the format\"."
user-invocable: true
---

The reply is the part of the task the user actually reads. Everything else — the grep, the build, the
test run — is scaffolding they see a few lines of. A reply that opens with a file path, or with a
paragraph that mixes the headline with the mechanism, makes the user do the sorting. This skill puts
the sorting on you: what they asked, what happened, then how — in that order, under fixed headers, so
they can stop reading the moment they have what they need.

## When to run it

Right before you write the reply that answers or closes a request — after the work is done, after
`nebula-memory` and `project-terms` have run (their one-line results fold into the last section). Every
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

Three sections, these exact headers, this order, nothing before the first and nothing after the last:

```
==== YOU ASKED ====
"<the prompt-daddy pick, verbatim>"

==== OVERVIEW ====
<what happened, at a glance>

==== TECHNICAL OVERVIEW ====
<the details, simplified>
```

Four `=` on each side of every header. Blank line under each header. No title above `YOU ASKED`, no
sign-off below the technical section — an offer of follow-up work goes in the last line of the
technical section if it goes anywhere.

### `==== YOU ASKED ====`

One quoted prompt: **the rewrite the user picked in `prompt-daddy`**, verbatim, in double quotes. Only
the pick — not the original beside it, not the option's label, no `→ picked:` prefix. That line belongs
in the memory entry, not here.

When there was no pick, quote the prompt you actually worked from:

- the user chose **Keep original** → the original, as typed
- `prompt-daddy` skipped itself (a mid-task correction, a confirmation, a skill trigger, a headless run)
  → the user's message, as typed
- the user typed something under *Other* → that text

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

A reader who stops after this section should know the outcome and what, if anything, still needs them.

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
```

The original reply had the same content in prose that opened with a heading and a rationale paragraph;
the user asked for it twice more before it had this shape. Start with the shape.
