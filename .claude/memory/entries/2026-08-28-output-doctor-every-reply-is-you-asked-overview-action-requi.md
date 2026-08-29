# Output Doctor: Every Reply Is YOU ASKED / OVERVIEW / [ACTION REQUIRED] / TECHNICAL OVERVIEW — 2026-08-28

**Asked:** "what 3 features would you recommend I remove" → picked (prompt-daddy): *Cost vs. value audit*
→ then, on the answer: "rewrite this in a format like this: ==== YOU ASKED ==== … ==== OVERVIEW ==== …
==== TECHNICAL OVERVIEW ===" → "in the YOU ASKED section we should be only displaying the prompt created
by prompt-daddy" → "use this final output structure as a skill called output-doctor which prompts the
agent to use that format I provided for all output. update claude.md to always use this skill before it
outputs anything"
→ later the same day: "update the output-doctor to include a section for ==== ACTION REQUIRED ==== if and
only if the llm is expecting a user to do something" → picked (prompt-daddy): *After OVERVIEW, strict
trigger* — between OVERVIEW and TECHNICAL OVERVIEW, present iff the user must do something before the work
is complete (run a command, flip a setting, restart, decide, approve); numbered imperative steps, exact
commands in code blocks; optional follow-up offers do not count.

**Did:** New `.claude/skills/output-doctor/SKILL.md` (user-invocable): three fixed headers, four `=` each
side, in order — `YOU ASKED` (the `prompt-daddy` pick verbatim, only the pick; the original/Other/as-typed
prompt when there was none), `OVERVIEW` (plain sentences, no paths or symbols, TERMS in caps, the test
gate and anything left out stated here), `TECHNICAL OVERVIEW` (bullets per overview item, `file:line`
refs, err short — the user asks for more). It runs *after* `nebula-memory` and `project-terms`, whose
one-line results close the technical section. Exceptions it lists: the "about to" preamble, mid-task
progress notes, `AskUserQuestion`, file-bound text (commits, memory, TERMS), a reply that is only a
question. `CLAUDE.md` gained a "Before you reply" step after the `project-terms` one; `AGENTS.md` gained
the matching "Then shape the reply" paragraph. The audit that preceded it — CLAUDE CLOUD sessions,
`nebula browser`/`tunnel`, the PREWARM POOL as the three costliest features — was recommendation only;
nothing was cut and no decision was taken.
Follow-up: the skill gained a conditional fourth section, `==== ACTION REQUIRED ====`, between OVERVIEW and
TECHNICAL OVERVIEW — its own `###` block with the iff test ("a step you could not take yourself that the
outcome depends on"), a *counts* list (user-only commands such as NEBULA KILL / MAKE CYCLE from a terminal
outside nebula, settings, restarts, blocked decisions, approvals, manual checks) and a *does not count*
list (follow-up offers, gotchas, scaled-down scope, anything the agent could still do itself), the shape
(numbered imperative steps, exact command in a code block, no prose above), and a second worked example.
OVERVIEW's closing rule now says it tells the reader *whether* anything needs them, ACTION REQUIRED the
*what*. Mirrored in `CLAUDE.md` § "Before you reply", `AGENTS.md` "Then shape the reply", and the OUTPUT
DOCTOR row of `TERMS.md`.

**Gotchas:**
- **`YOU ASKED` is the rewrite alone** (since 2026-08-28 the refined prompt, before that the pick). The
  first cut showed the original prompt with a `→ picked:` line
  under it (the `nebula-memory` convention) and the user rejected it: the memory entry keeps both, the
  reply keeps only the rewrite that was worked from.
- The user's template had three `=` on the `TECHNICAL OVERVIEW` header and four on the other two;
  standardized on four for all three in the skill. Match the skill, not the transcript.
- **`AGENTS.md` is not a copy of `CLAUDE.md`** — it is the shorter, skill-tool-free version of the same
  protocol for non-Claude agents, and it references the skills by file path. A protocol change has to be
  written twice, once per file, in each file's own register.
- Skill discovery is live (as the prompt-daddy entry says): `output-doctor` appeared in the session's
  skill listing the moment the file existed, before `CLAUDE.md` was edited — and the listing's description
  refreshed the moment the frontmatter changed, mid-session.
- **The reply shape lives in four places**, not two: the skill, `CLAUDE.md`, `AGENTS.md`, *and* the OUTPUT
  DOCTOR row in `TERMS.md` all spell out the section list. A section change is four edits; grep
  `==== TECHNICAL OVERVIEW ====` to find them all.
- A worked example that itself contains a code block (the ACTION REQUIRED step's command) needs a
  four-backtick outer fence — the triple-fenced examples in the skill can't nest one.
