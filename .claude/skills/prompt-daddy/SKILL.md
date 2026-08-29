---
name: prompt-daddy
description: "Before starting any new task, rewrite the user's prompt once into the best fully specified version of it — the ambiguous words closed, the unstated \"keep X as-is\" named, the why and the evidence in place, the user's aliases replaced by the ALL-CAPS TERMS from TERMS.md — asking the user only for the context the work cannot proceed without (who, what, when, where, why, how), then log the final prompt and proceed on it without asking whether it is right. Use on every new request: feature asks, bug reports, questions, refactors. Also use when the user says \"prompt daddy\", \"prompt doctor\", \"improve my prompt\", \"tighten this prompt\", or \"what should I have asked\"."
user-invocable: true
---

The user's prompt is the spec, and in this repo the spec is where turns get lost. The MEMORY LOG
records it plainly: "done" took four turns because *"the first two readings were both wrong"*; the
Shift+J/K move *"satisfied the literal words and not the request"*; a Ctrl+Shift+H/L tweak broke the
walk the user liked because the prompt never said what to keep. Every one of those was a one-sentence
fix the user already had in their head. This skill gets that sentence out **before** the work starts —
by asking for it when the prompt genuinely does not contain it, and otherwise by writing it yourself.

You are not judging the prompt and you are not starting the task. You are producing **one** better
version of the ask, asking only for what you cannot supply, and then working from that version. You do
not offer alternatives to pick from, and you do not ask whether the rewrite is acceptable — once it is
logged, it is the request.

The rewrite is also where the user's words become the project's words. `TERMS.md` (repo root) holds one
ALL-CAPS canonical name per feature, panel, key, command and mechanism, with the aliases the user has
used for each and where it lives in the code. The rewrite says **WORKSPACES BAR** where the prompt said
"top nav", **the edge of the PANEL WALK** where it said "the locked layer", **UNSEEN** or **FINISHED**
where it said "done" — and when an alias maps to two TERMS and the work differs by which, that split is
a question, not a guess.

## When to run it

On every new prompt from the user — a feature, a bug report, a question, a refactor, a "debug this".
Run it after reading `.claude/MEMORY.md` (the index), `.claude/memory/gotchas.md`, `TERMS.md` and
whatever the RECALL HOOK injected under `[nebula recall]`, and before planning, grepping the code in
earnest, or answering.

Skip it, and just proceed, only when the prompt is:

- a reply to a question **you** asked (an `AskUserQuestion` answer, "yes do it", "the second one",
  "entirely") — the refinement already happened
- a mid-task correction that is itself specific ("no, green after I focus it, purple while unread") —
  take the correction. If the correction is as ambiguous as the original, run it on the correction.
- a slash-command, an explicit skill invocation, or a phrase that triggers one (`/release`,
  "commit push release", "remember this") — the skill is the refinement
- a pure question that changes nothing — an explanation, an assessment, "what does X do", "is Y worth
  it". Answer it directly, in TERMS, grounded by the RECALL HOOK's context; the rewrite only earns its
  cost when work follows. A question that is a task in disguise ("why is X broken" that ends in a fix)
  is a task: run it.
- pure conversation with no task in it

When there is no interactive user (a `-p` / headless run), still rewrite and still log — but skip the
questions: `AskUserQuestion` would hang. Write every gap you would have asked about into the rewrite as
a stated assumption and proceed on it.

## Steps

### 1. Read the prompt against the failure list

Find which of these the prompt has. Most prompts have one or two; the rewrite targets those, not all
eleven.

| The prompt… | …so the rewrite should |
|---|---|
| names a thing by an alias — *top nav, the walk, locked layer, the h picker, done* | say the TERM from `TERMS.md` in ALL CAPS (WORKSPACES BAR, PANEL WALK, UNSEEN); if the alias maps to two TERMS and the work differs by which, that is a question (step 3) |
| hangs the whole spec on one word — *done, new, move, remove, auto focus, fix it, remember* | spell out the states: *when* ‹event›, ‹thing› is ‹value›; otherwise ‹value› |
| changes a behavior the user likes without saying what stays | add "keep ‹X› exactly as it is; only change ‹Y›" |
| has a circular *when* ("auto focus when focused") | name the event, the result, and the boundary ("when Ctrl+Shift+L lands on the pane, focus it and stop the walk there") |
| reports a bug with no evidence | restate what is known; ask for the exact on-screen text, the steps, where it *does* work, and the terminal, agent, and version when the diagnosis cannot start without them |
| asks for a feature with no *why* | add "so that I can ‹…›" when the why is obvious from the MEMORY LOG or the feature; ask when the mechanism hinges on it (the unread counter went daemon-side for exactly this reason) |
| describes output with adjectives | give one literal example of the output (`'23m ago'`, `yellow-fox-jumps`, `nebula v0.13.0`) |
| bundles asks that touch the same code path | split them, or order them ("first A, then B on top of A") |
| is visual but has no screenshot or target | state the target ("flush with the tab", "10% opacity") or ask for the screenshot |
| reverses a decision recorded in the MEMORY LOG | say so: "I know we removed ‹X› on ‹date›; bring it back because ‹Y›" |
| is silent on constraints the user usually cares about | name them: rate limits, no new deps, non-goals, which terminal, what must not get slower |
| says "fix" when the user might want to understand first | keep the fix unless the MEMORY LOG shows they asked to understand first on this subsystem; then write it as "find out why and tell me before changing anything" |

### 2. Ground it — briefly

Map every noun in the prompt onto `TERMS.md` first — the **Alias index** at its bottom turns the user's
word into the TERM, and the TERM's row names the file and symbol, so most grounding is a lookup, not a
grep. A noun with no TERM is worth noticing: the rewrite should name it in words the user can confirm,
and `project-terms` will record it when the task ends.

Then use the MEMORY LOG entries you already read and the ones the RECALL HOOK injected: a related gotcha, a recorded decision, the file where
that subsystem lives. One quick `grep` to name the real symbol, panel, or idiom is fine if it makes the
rewrite concrete ("the link row's unread-count idiom", `row_badges` in `ui.rs`). Do not investigate the
bug or start the design — that is the task, and it has not been written yet.

### 3. Decide whether anything has to be asked

Draft the rewrite in your head. Then apply one test to every blank in it:

> Is this a **who / what / when / where / why / how** that the implementation actually hinges on, and
> that neither the prompt, nor the MEMORY LOG, nor `TERMS.md`, nor one grep can fill?

- **No** — every blank has a conventional default, a recorded decision, or an answer in the code. Do
  not ask. Fill it, and where you filled it with a judgment rather than a fact, write that judgment
  into the rewrite as a stated assumption ("(assuming this is in Ghostty)", "(so that I can see which
  sessions need me)"). The user reads it in the log and corrects it if it is wrong.
- **Yes** — the work would go one of two materially different ways depending on the answer, or cannot
  start at all (a bug with no reproduction, a "make it look better" with no target, an alias that maps
  to two TERMS with different code paths). Ask.

The bar is *cannot implement without it*, not *would be nice to know*. Ask about things the user could
not reasonably expect you to know — which of two readings they meant, what they actually saw on
screen, what the feature is for when the design hinges on the purpose. Do not ask about things a
careful colleague would just decide.

When you ask: **one** `AskUserQuestion`, with one question per gap (up to four), each question's
`header` naming the gap in a word or two (`Which "done"`, `Evidence`, `Why`, `Scope`), and its options
being the plausible answers written in TERMS — for a two-TERM alias, the two TERMS; for a why, the two
or three purposes the feature could serve; for a scope, the readings of the ambiguous word. Put the
answer you would pick first, labeled `(Recommended)`, so a fast "yes" is one keystroke. The user's
*Other* text is an answer like any other. Do not explain the questions in chat before asking them.
Ask once — fold the answers in and go; do not come back with a second round unless an answer opened a
gap that fails the same test.

### 4. Write the one rewrite

- **A complete prompt in the user's own voice** — first person, imperative, something they could have
  typed. Not a plan, not a question back to them, not a restatement with nicer grammar.
- **…but in the project's TERMS.** Every feature, panel, key, command, status and mechanism the rewrite
  mentions is named by its ALL-CAPS TERM from `TERMS.md`, exactly as spelled there — the user's alias
  may follow in parentheses the first time when the mapping is not obvious ("the WORKSPACES BAR (the top
  nav)"). Never coin a new caps name inside the rewrite; a thing with no TERM is described in plain
  words and flagged as unnamed. Code identifiers, keys and commands keep their real spelling
  (`Ctrl+Shift+L`, `nebula tunnel`, `row_badges`).
- **Close the gaps from step 1**, with the answers from step 3 where you asked and stated assumptions
  where you did not.
- **Never invent facts the user did not give.** A filled blank is either an answer they gave, a fact
  from the repo, or a visibly marked assumption. Nothing else.
- **Keep every constraint the original had.** A rewrite that drops "make sure it's efficient, gh has
  rate limits" is worse than the original.
- **Keep the scope the original had.** Closing an ambiguity is not license to add a feature, split the
  task into a project, or turn a fix into an investigation the user did not ask for.
- Under ~100 words. If the original is already tight, the rewrite is nearly the original with the
  aliases swapped for TERMS; do not pad it to look like work was done.

### 5. Log it and go

Print the final prompt in the chat, as a progress note, in exactly this shape — one short lead-in
line, then the prompt as a quote:

```
Refined prompt:

> <the rewrite, verbatim>
```

That is the only output before the work starts. **Do not ask whether it is right.** Do not offer to
change it, do not list what you assumed separately (the assumptions are in the prompt, in parentheses),
and do not wait. The rewrite is now the user's prompt: work from it exactly as if it had been the first
message, and do not re-litigate what it settled. If the user corrects it mid-task, take the correction
as a mid-task correction (see "When to run it").

Downstream, this logged text is what the other skills use:

- `output-doctor` quotes it verbatim as `==== YOU ASKED ====`
- `nebula-memory` quotes the **original** prompt on the **Asked** line, as that skill requires, and puts
  the rewrite underneath on the correction line it already allows — "→ refined: ‹text›" — with each
  question asked and its answer in a following parenthesis ("(asked: which 'done' → UNSEEN)"). Future
  analysis of how prompts get refined depends on both surviving.

## Worked examples

**A prompt that has to ask.** Original (from the MEMORY LOG, 2026-08-27): *"can you make the status dot for
done a different color than green so it's obvious something needs to be addressed"*

"done" sits in the Alias index under both UNSEEN and FINISHED, and the two are different code paths
(one flag that clears on focus, one status that never does). The work hinges on which — so one
question, header `Which "done"`, options *UNSEEN — finished while I wasn't looking, back to green once
I focus it (Recommended)* / *FINISHED — every finished SESSION, whether or not I've looked*. The user
picks the first. The logged prompt:

> Refined prompt:
>
> > SESSIONS that finished while I wasn't looking (UNSEEN) should show a violet STATUS DOT; once I
> > focus that SESSION it goes back to the normal green. Use the same UNSEEN flag the DONE BADGE
> > already counts — don't add a second notion of "needs attention". RUNNING stays yellow.

That is the sentence the user eventually typed by hand, three turns later.

**A prompt that does not.** Original: *"the top nav should show the branch name next to each
workspace, keep it short"*

Every blank has a default: "top nav" is the WORKSPACES BAR by the Alias index, "the branch name" is the
WORKTREE's branch, and "short" has a literal example in the MEMORY LOG's own tab-title convention. No
question. The logged prompt:

> Refined prompt:
>
> > In the WORKSPACES BAR (the top nav), show each WORKSPACE's WORKTREE branch name after its title,
> > dimmed, truncated to ~16 characters with `…` (assuming `fix-login-redirect` → `fix-login-redir…`).
> > Keep the tab underline and the current tab order exactly as they are.

Then the work starts.
