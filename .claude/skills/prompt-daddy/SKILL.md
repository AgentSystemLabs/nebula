---
name: prompt-daddy
description: "Before starting any new task, rewrite the user's prompt three ways — each a fully specified version that closes a gap the original left open (an ambiguous word, an unstated \"keep X as-is\", a missing why, a bug report without its evidence), written in the ALL-CAPS TERMS from TERMS.md instead of the user's aliases — and let the user pick one, or keep the original, before any work begins. Use on every new request: feature asks, bug reports, questions, refactors. Also use when the user says \"prompt daddy\", \"improve my prompt\", \"tighten this prompt\", or \"what should I have asked\"."
user-invocable: true
---

The user's prompt is the spec, and in this repo the spec is where turns get lost. `.claude/MEMORY.md`
records it plainly: "done" took four turns because *"the first two readings were both wrong"*; the
Shift+J/K move *"satisfied the literal words and not the request"*; a Ctrl+Shift+H/L tweak broke the
walk the user liked because the prompt never said what to keep. Every one of those was a one-sentence
fix the user already had in their head. This skill gets that sentence out **before** the work starts,
by showing the user three concrete rewrites and letting them point at the right one.

You are not judging the prompt and you are not starting the task. You are offering three better
versions of the ask and taking the pick as the request.

The rewrites are also where the user's words become the project's words. `TERMS.md` (repo root) holds
one ALL-CAPS canonical name per feature, panel, key, command and mechanism, with the aliases the user
has used for each and where it lives in the code. A rewrite says **WORKSPACES BAR** where the prompt
said "top nav", **the edge of the PANEL WALK** where it said "the locked layer", **UNSEEN** or
**FINISHED** where it said "done" — and when an alias maps to two TERMS, that split *is* the three
readings. Picking a rewrite therefore also confirms which thing the user meant, by its name.

## When to run it

On every new prompt from the user — a feature, a bug report, a question, a refactor, a "debug this".
Run it after reading `.claude/MEMORY.md` and `TERMS.md`, and before planning, grepping the code in
earnest, or answering.

Skip it, and just proceed, only when the prompt is:

- a reply to a question **you** asked (an `AskUserQuestion` answer, "yes do it", "the second one",
  "entirely") — the refinement already happened
- a mid-task correction that is itself specific ("no, green after I focus it, purple while unread") —
  take the correction. If the correction is as ambiguous as the original, run it on the correction.
- a slash-command, an explicit skill invocation, or a phrase that triggers one (`/release`,
  "commit push release", "remember this") — the skill is the refinement
- pure conversation with no task in it

Also skip it when there is no interactive user (a `-p` / headless run): `AskUserQuestion` would hang.
Say in one line that you are proceeding on the original prompt and why.

## Steps

### 1. Read the prompt against the failure list

Find which of these the prompt has. Most prompts have one or two; the rewrites target those, not all
eleven.

| The prompt… | …so a rewrite should |
|---|---|
| names a thing by an alias — *top nav, the walk, locked layer, the h picker, done* | say the TERM from `TERMS.md` in ALL CAPS (WORKSPACES BAR, PANEL WALK, UNSEEN); if the alias maps to two TERMS, make the three rewrites the two readings plus the staged ask |
| hangs the whole spec on one word — *done, new, move, remove, auto focus, fix it, remember* | spell out the states: *when* ‹event›, ‹thing› is ‹value›; otherwise ‹value› |
| changes a behavior the user likes without saying what stays | add "keep ‹X› exactly as it is; only change ‹Y›" |
| has a circular *when* ("auto focus when focused") | name the event, the result, and the boundary ("when Ctrl+Shift+L lands on the pane, focus it and stop the walk there") |
| reports a bug with no evidence | ask for / restate the exact on-screen text, the steps, where it *does* work, and the terminal, agent, and version |
| asks for a feature with no *why* | add "so that I can ‹…›" — the mechanism follows from the why (the unread counter went daemon-side for exactly this reason) |
| describes output with adjectives | give one literal example of the output (`'23m ago'`, `yellow-fox-jumps`, `nebula v0.13.0`) |
| bundles asks that touch the same code path | split them, or order them ("first A, then B on top of A") |
| is visual but has no screenshot or target | state the target ("flush with the tab", "10% opacity") or ask for the screenshot |
| reverses a decision recorded in MEMORY.md | say so: "I know we removed ‹X› on ‹date›; bring it back because ‹Y›" |
| is silent on constraints the user usually cares about | name them: rate limits, no new deps, non-goals, which terminal, what must not get slower |
| says "fix" when the user might want to understand first | offer the two-step: "find out why and tell me before changing anything" |

### 2. Ground it — briefly

Map every noun in the prompt onto `TERMS.md` first — the **Alias index** at its bottom turns the user's
word into the TERM, and the TERM's row names the file and symbol, so most grounding is a lookup, not a
grep. A noun with no TERM is worth noticing: the rewrite should name it in words the user can confirm,
and `project-terms` will record it when the task ends.

Then use the MEMORY.md entries you already read: a related gotcha, a recorded decision, the file where
that subsystem lives. One quick `grep` to name the real symbol, panel, or idiom is fine if it makes a rewrite
concrete ("the link row's unread-count idiom", `row_badges` in `ui.rs`). Do not investigate the bug or
start the design — that is the task, and it has not been chosen yet.

### 3. Write three rewrites

Rules that make the three worth reading:

- **Each is a complete prompt in the user's own voice** — first person, imperative, something they
  could have typed. Not a plan, not a question back to them, not a restatement with nicer grammar.
- **…but in the project's TERMS.** Every feature, panel, key, command, status and mechanism the rewrite
  mentions is named by its ALL-CAPS TERM from `TERMS.md`, exactly as spelled there — the user's alias
  may follow in parentheses the first time when the mapping is not obvious ("the WORKSPACES BAR (the top
  nav)"). Never coin a new caps name inside a rewrite; a thing with no TERM is described in plain words
  and flagged as unnamed. Code identifiers, keys and commands keep their real spelling (`Ctrl+Shift+L`,
  `nebula tunnel`, `row_badges`).
- **The three differ in something material, not in length.** Pick the split by what the prompt is:
  - *Ambiguous prompt* → the three are the three most plausible **readings**, each fully specified.
    Picking one settles the ambiguity in one turn instead of four. (For "make the done dot a different
    color so it's obvious something needs addressing": all-finished-violet / violet-while-unread-then-
    green / violet-while-unread-and-clear-the-counter-on-focus.)
  - *Clear prompt* → the three are **depth variants**: **Tight** (same scope, the ambiguous words closed,
    the preserved behavior named), **Grounded** (adds the why, the acceptance check, and how the user
    will verify it), **Staged** (diagnose-then-confirm, or the bundle split into ordered independent
    asks).
- **Never invent facts the user did not give.** Where a rewrite needs the why, the terminal app, or the
  exact error, write it as a stated assumption — "(assuming this is in Ghostty)", "(so that I can see
  which sessions need me — correct me if that's not the goal)" — so the pick confirms it, or the user
  fills it in via *Other*.
- **Keep every constraint the original had.** A rewrite that drops "make sure it's efficient, gh has
  rate limits" is worse than the original.
- Under ~80 words each. If the original is already tight, say so in the option descriptions and make
  the rewrites small; do not pad.

### 4. Present the pick

One `AskUserQuestion`, single-select, header `Prompt`, four options in this order:

1. the rewrite you would pick, label ending in `(Recommended)`
2. rewrite two
3. rewrite three
4. **Keep original** — description: "Proceed on the prompt exactly as typed"

For each rewrite: `label` = the angle in 2–5 words ("Unread means violet", "Diagnose first, then fix",
"Keep the walk, add the stop"); `description` = the one gap it closes, in a sentence; `preview` = the
full rewritten prompt, verbatim, so the four render side by side and the user reads the prompts, not
your summaries of them.

Do not explain the options in chat before the question. The question is the explanation.

### 5. Take the pick as the request

The chosen text — or whatever the user typed under *Other* — is now the user's prompt. Work from it
exactly as if it had been the first message. Do not run this skill on the pick, and do not re-litigate
what the pick settled.

When you write the `nebula-memory` entry at the end, quote the **original** prompt verbatim as that
skill requires, and put the picked rewrite underneath it on the correction line that skill already
allows ("→ picked: ‹label› — ‹text›"). Future analysis of how prompts get refined depends on both
surviving.

## Worked example

Original (from MEMORY.md, 2026-08-27): *"can you make the status dot for done a different color than
green so it's obvious something needs to be addressed"*

- **Unread means violet (Recommended)** — closes *done* as UNSEEN: "SESSIONS that finished while I
  wasn't looking (UNSEEN) should show a violet STATUS DOT; once I focus that SESSION it goes back to the
  normal green. Use the same UNSEEN flag the DONE BADGE already counts — don't add a second notion of
  'needs attention'."
- **All finished is violet** — reads *done* as the FINISHED status: "Every SESSION in the FINISHED status
  shows a violet STATUS DOT instead of green, whether or not I've looked at it. RUNNING stays yellow."
- **Diagnose the flag first** — staged: "Before changing colors, tell me which existing state already
  means 'needs my attention' (UNSEEN? FINISHED? NEEDS FEEDBACK?) and propose which one the STATUS DOT
  should key off. Then I'll pick and you change the color."
- **Keep original**

The first option is the one the user eventually typed by hand, three turns later — and "done" now
sits in the Alias index under both UNSEEN and FINISHED, so the split is named before the next prompt.
