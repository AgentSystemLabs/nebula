# Prompt Daddy: One Rewrite, Ask Only For Missing Context, Log It And Go — 2026-08-28

**Asked:** "update prompt doctor to not give 3 examples to pick from, instead it should just do it's best to
rephrase the prompt and present questions if the original prompt seems to be lacking context to even convert
it to a good prompt, if the original prompt doesn't have enough context to successfully implement the
request, such as (who, what, when, where, why, how), then just ask the user for it and show them the final
prompt in logs (do not ask if that prompt is ok, just assume it's good after we run it through prompt doctor
and get some clarification."
→ refined: Rewrite PROMPT DADDY so it produces one best rewrite in TERMS; ask — one `AskUserQuestion`, one
question per gap — only for who/what/when/where/why/how the implementation hinges on; print the final
prompt in the chat and proceed on it, never asking whether it is OK; update `CLAUDE.md`, OUTPUT DOCTOR,
`TERMS.md` and the memory convention to match. (assumed: "in logs" = the chat stream)

**Did:** Rewrote `.claude/skills/prompt-daddy/SKILL.md`. Steps 3–5 are now: a *decide whether to ask* test
("a who/what/when/where/why/how the implementation hinges on that the prompt, MEMORY LOG, `TERMS.md` and
one grep cannot fill" — the bar is *cannot implement without it*), one `AskUserQuestion` with one question
per gap (header names the gap, options are the readings in TERMS, recommended first), one rewrite in the
user's voice with filled judgments written as "(assuming …)" in parentheses, and a fixed log shape —
`Refined prompt:` + the text as a `>` quote — after which work starts with no confirmation. The
three-readings / Tight-Grounded-Staged split and the **Keep original** option are gone; a two-TERM alias
is now a question's options instead of competing prompts. Headless: still rewrites and logs, questions
become stated assumptions. Two worked examples (one that asks, one that does not). Mirrored: `CLAUDE.md`
§ "Refine the prompt before acting on it" and the two-TERMS line in § "Before you start a task"; OUTPUT
DOCTOR's `YOU ASKED` is now "the refined prompt `prompt-daddy` logged, verbatim" (a mid-task correction
is applied to it; no **Keep original** / *Other* cases); the PROMPT DADDY and OUTPUT DOCTOR rows and the
Alias index in `TERMS.md` ("prompt doctor" added as an alias). `AGENTS.md` never named the pick ("the
prompt you worked from") and was left alone. The MEMORY LOG correction line is now `→ refined: ‹text›`
plus `(asked: ‹gap› → ‹answer›)` per question, replacing `→ picked:`.

**Gotchas:**
- This prompt was itself refined the new way, not the old: running the three-option picker on "stop
  giving me three options" would have been the wrong tool. When a prompt changes the protocol, apply the
  protocol the prompt asks for.
- **The prompt-refinement protocol lives in four places** — the skill, `CLAUDE.md` (two spots: the
  two-TERMS line near the top and the "Refine the prompt" section), `output-doctor`'s `YOU ASKED` rules
  (description + section), and `TERMS.md` (PROMPT DADDY row, OUTPUT DOCTOR row, Alias index). A behavior
  change is all of them; grep `pick` afterwards to catch the strays.
- The skill listing's description refreshed mid-session the moment the frontmatter was rewritten (same
  live discovery the 2026-08-27 entry records) — the new description was in effect before `CLAUDE.md` was.
- `.claude/MEMORY.md` is ~2800 lines against the skill's ~300-line pruning rule; not pruned here (shared
  tree, out of scope) — whoever prunes should merge the four PROMPT DADDY / OUTPUT DOCTOR / PROJECT
  TERMS entries first, they circle the same protocol.
