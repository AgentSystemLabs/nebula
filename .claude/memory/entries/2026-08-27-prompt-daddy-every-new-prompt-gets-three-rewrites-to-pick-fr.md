# Prompt Daddy: Every New Prompt Gets Three Rewrites To Pick From — 2026-08-27

> **Superseded 2026-08-28** — the three-rewrites pick is gone; see "Prompt Daddy: One Rewrite, Ask Only For
> Missing Context, Log It And Go" above. The failure-table patterns below still hold.

**Asked:** "analyze memory.md for how i normally prompt and give me patterns you see, good or bad" → "using
this info, create a skill called prompt-daddy which will take your original prompt and give you 3 examples
to pick from that improve the original prompt, add prompt daddy to claude.md instructions so my agent uses
it on every new prompt"

**Did:** New `.claude/skills/prompt-daddy/SKILL.md` (user-invocable). It runs after the MEMORY.md read and
before any planning: read the prompt against an eleven-row failure table distilled from this log, write
three rewrites in the user's voice, present them with `AskUserQuestion` (three previews + **Keep
original**, recommended one first), and take the pick as the request. Ambiguous prompt → the three are the
three plausible *readings*, fully specified; clear prompt → Tight / Grounded / Staged depth variants.
`CLAUDE.md` gained a "Refine the prompt before acting on it" step between the memory read and the work.
The skill skips itself for replies to an agent question, bare confirmations, already-specific mid-task
corrections, slash-commands / skill triggers ("commit push release"), and headless runs.

**Gotchas:**
- The patterns behind the failure table, from the 84 prompts in this log: one word carrying the whole spec
  ("done" — four turns, both first readings wrong; "move" — satisfied the literal words; "remove the
  ability to add notes" — asked twice identically and still needed a scope question), tweaking a liked
  behavior without naming what stays (Ctrl+Shift+H/L), circular when-clauses ("auto focus when focused"),
  visual asks without a screenshot iterating where ones with a screenshot landed in one shot. Bug reports
  with the exact on-screen text and a "works in X, not in Y" contrast were diagnosed in one session every
  time. Escalated re-asks carried no new spec; the fix arrived with the next constraint, not the next tone.
- When writing the memory entry for a task that went through prompt-daddy, quote the **original** prompt
  (this skill's rule) and put the rewrite on the correction line underneath (now `→ refined:`, was
  `→ picked:`) — the refinement is the data the next analysis of prompting needs.
- Skill discovery is live: the new `SKILL.md` appeared in the session's skill listing as soon as the file
  existed, no restart needed.
