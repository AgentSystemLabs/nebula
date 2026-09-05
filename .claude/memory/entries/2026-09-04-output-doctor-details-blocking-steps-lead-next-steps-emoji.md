# OUTPUT DOCTOR: DETAILS Replaces TECHNICAL OVERVIEW, Blocking Steps Lead NEXT STEPS, Headings Carry An Emoji — 2026-09-04

**Asked:** "is there a better way to structure the output or format of output doctor?"
→ on the assessment's NEXT STEPS decision: "make the changes then re output to me so I can read as example"
→ mid-turn: "I'm also having a hard time telling the different secions apart, try to add color or more
spave between sections so it's easier to read for a human"
→ no REFINED PROMPT — a question, then a reply to the decision it left.

**Did:** Rewrote `.claude/skills/output-doctor/SKILL.md` on four points. (1) ACTION REQUIRED is gone as
a section: its items lead NEXT STEPS, each opened `🚧 **Blocking:**` — the two sections' "counts" lists
shared three items (a command, a decision, a manual check) and NEXT STEPS needed a rule for pointing
back at ACTION REQUIRED instead of repeating it. (2) YOU ASKED is present iff a REFINED PROMPT was
logged for the work the reply closes (this turn, or the earlier turn a bare confirmation picked up);
absent for a question, a reply to a question, a correction or a skill trigger, where it only echoed the
message directly above. (3) TECHNICAL OVERVIEW is renamed DETAILS and made conditional — present only
for a mechanism, a `file:line`, a rejected approach or a gotcha to know now; the "one bullet group per
OVERVIEW item" rule is dropped, and the `nebula-memory` / `project-terms` one-liners are left out when
they would be the section's only content. (4) For the mid-turn ask: every section opens with a `---`
rule and a `##` heading carrying a fixed emoji (💬 YOU ASKED, 📋 OVERVIEW, 🔧 DETAILS, 👉 NEXT STEPS) —
the rule is the space, the heading is the weight, the emoji is the color. A pure question is now two
sections; three worked examples (four-section, blocking-step, two-section). `TERMS.md`: the OUTPUT
DOCTOR and NEXT STEPS rows rewritten, "action required" moved to NEXT STEPS' aliases and "technical
overview" added to OUTPUT DOCTOR's, both in the Alias index; the OUTPUT DOCTOR `Where` repointed from
`CLAUDE.md` § "Before you reply" (gone since 2026-08-29) to `AGENTS.md` § "After you finish a task".
The OUTPUT DOCTOR standing gotcha now greps `==== NEXT STEPS ====`. `CLAUDE.md`, `AGENTS.md` and
`prompt-daddy` name only the trigger and the YOU ASKED quote, so they stand. Gates: `make memory-check`,
`make terms-check`, `make recall-eval`.

**Gotchas:**
- The overlap was in the user's own two specs — "give a command" in the NEXT STEPS ask, "run a command"
  in the ACTION REQUIRED ask — and the skill had copied both into two "counts" lists. When two sections
  need a rule for pointing at each other, they are one section. Settled: one to-do section, blocking
  items first, marked.
- The first reply under the new rules has no YOU ASKED: "make the changes" answered the decision the
  assessment left, so no REFINED PROMPT existed. Expected, not a bug — a confirmation only inherits a
  REFINED PROMPT that was logged earlier in the same session.
- The section names live in `TERMS.md` in three spots (the OUTPUT DOCTOR row, the NEXT STEPS row, the
  Alias index), not one — plus the skill and the standing gotcha. Old entries keep the old names; the
  aliases "technical overview" and "action required" are what map them.
- A `---` rule directly under a line of text is a markdown setext heading, not a rule: the blank line
  above every rule in the format is load-bearing, and the worked example with a nested code block still
  needs a four-backtick outer fence.
- **A literal `---` inside a SKILL.md frontmatter `description` ends the frontmatter** for the Skill
  tool's parser: the live skill listing showed the description starting right after it ("rule and a
  ## heading…"), and everything past that point — `user-invocable: true` included — was body text.
  Write "a horizontal rule" in frontmatter; the `---` belongs in the body only.

**Corrections:** 1
