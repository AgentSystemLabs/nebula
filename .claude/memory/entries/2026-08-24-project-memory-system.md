# Project Memory System — 2026-08-24

**Asked:** "update claude.md to invoke a skill called nebula-memory which has instructions on how an
agent should summarize the original request, how we fixed or implemnted it, and any gotchya you ran
into along the way. update the claude.md to instruct agents to read the memory.md file that the skill
updates …" — then: "go through all previous sessions for this project and invoke the nebula-memory
skill starting with oldest last so we can document how we grew this project."

**Did:** Created `CLAUDE.md` (none existed — only an empty `CLAUDE.local.md`), the
`.claude/skills/nebula-memory/` skill, and this file. Backfilled the entries below.

**Gotchas:**
- Real user prompts are recoverable from the transcripts by filtering `type=="user"` **and**
  `promptSource=="typed"` **and** `origin.kind=="human"`. Without that filter you get 8544 tool-result
  records instead of 258 prompts.
- ~12 sessions in this project's transcript dir are not nebula work at all — they are Cartastrophe game
  sessions and one-off test prompts that happened to run from this cwd. Filter by content, not by directory.
