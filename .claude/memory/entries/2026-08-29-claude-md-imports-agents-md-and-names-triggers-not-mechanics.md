# `CLAUDE.md` Imports `AGENTS.md` And Names Each Skill's Trigger Instead Of Restating It — 2026-08-29

**Asked:** "yes, ok do the rewrite of my claude.md with my current skills workflow setup by following best practices, do 3 rounds of reviews of your changes before you stop"
→ refined: Rewrite `CLAUDE.md` to Anthropic's current best practices while keeping the SELF-IMPROVING LOOP
working exactly as it does: `@AGENTS.md` as the single source of the loop, each of the four skills named by
trigger and skip rule rather than by restated mechanics, KEEP MODULES SMALL moved to a path-scoped
`.claude/rules/` file, the always-on rules kept. Then review the changes three separate times.

**Did:**
- **`AGENTS.md` is now the protocol** (68 → 72 lines), harness-neutral: the four-file table with its caps
  and the three MAKE CI gates, "Before you start" as four numbered steps ending in PROMPT DADDY, "Speak in
  the TERMS", "After you finish" (NEBULA-MEMORY SKILL → PROJECT TERMS → OUTPUT DOCTOR, each by SKILL.md
  path), and a "Writing code here" section pointing at the rule file and the SHARED CHECKOUT rule.
- **`CLAUDE.md` is 23 lines**: `@AGENTS.md` on line 1, then a `# Claude Code` block — a when/invoke/skip
  table for the four `Skill(...)` calls and the two hooks that run on their own. Down from 177 lines; the
  ~750 words that restated PROMPT DADDY's asking rule, the NEBULA-MEMORY SKILL's entry rules, PROJECT
  TERMS' promotion rule and OUTPUT DOCTOR's five sections are gone — each skill's body already carries them.
- **`.claude/rules/rust-modules.md`** holds KEEP MODULES SMALL under `paths: ["crates/**/*.rs"]`, so it
  loads only when a Rust file is read.
- Follow-through: TERMS.md's SELF-IMPROVING LOOP and KEEP MODULES SMALL *Where* cells repointed, the
  "reply shape lives in four files" STANDING GOTCHA rewritten (it is two now — the skill and the TERMS
  row), PROJECT TERMS' SKILL.md header now points at `AGENTS.md`.
- **Three review rounds.** R1 (completeness): 37 rules from the old files all still present; found the
  stale four-files gotcha and two TERMS *Where* cells; gates green with the shrunken TERMS CHECK corpus
  (`dead` stayed 0). R2 (claims): every path, `make` target, skill `name:` frontmatter and cap number
  verified against the repo; found the table listed four files under a "first two / third" sentence, and
  that the four skills were never written in their TERMS — both fixed. R3 (skeptical): found an invented
  rule (a `Skill(skill: "release")` line the original never had) and removed it, rewrapped prose to the
  repo's ~100 columns, re-ran the completeness check after the rewrap. Gates: MEMORY CHECK, RECALL EVAL,
  TERMS CHECK ok.

**Gotchas:**
- Anthropic's docs are explicit about all three moves: target under 200 lines, *"Create a skill when… a
  section of CLAUDE.md has grown into a procedure rather than a fact"*, and — new — *"Claude Code reads
  CLAUDE.md, not AGENTS.md… create a CLAUDE.md that imports it so both tools read the same instructions
  without duplicating them"*. `@path` imports do **not** save context (imported files load at launch);
  the win is one source, not fewer tokens.
- `@AGENTS.md` must sit outside backticks — import parsing skips code spans, so `` `@AGENTS.md` `` is
  literal text. Max import depth is four hops; `AGENTS.md` has no imports of its own.
- A path-scoped rule only loads when Claude reads a matching file, so a task that *plans* a Rust change
  without opening one never sees KEEP MODULES SMALL — `AGENTS.md` names the file for that reason.
- Shrinking `CLAUDE.md`/`AGENTS.md` shrinks TERMS CHECK's corpus, so a TERM only ever written there would
  turn up as `dead`: run `make terms-check` after editing either file, not just the gates you changed.
- A rewrapper that regroups lines into paragraphs must skip tables, fences and frontmatter, and must
  re-run the completeness check afterwards — reflowing is a silent-content-loss operation.
