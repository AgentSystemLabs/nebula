# Project Terms: Detect Every Session, Promote Only What Recurred — 2026-08-28

**Asked:** "update the project-terms skills: keep the architecture around the self improving look, but the
main improvement I'd make is changing project-terms from \"harvest words from every session\" to \"detect
vocabulary discoveries every session, but only promote concepts that have actually become canonical.\""
→ picked (prompt-daddy): *Recur across tasks* — "new TERMS are not written into the glossary sections on
first sight. They go to a candidates ledger (a section at the bottom of TERMS.md) with the date and where
they were seen; a candidate is promoted to a real TERM only when a later, separate task uses it again (in
my prompt, a commit, or a memory entry). Aliases for existing TERMS stay immediate. Renames and
retirements stay immediate. Prune candidates nobody touched again after ~30 days."

**Did:** Rewrote `.claude/skills/project-terms/SKILL.md` around two tiers. Every run still *detects* —
the nouns in the **Asked** line, the names in **Did**/**Gotchas**, commit messages, new symbols/keys/
commands — but a *new* name goes to a **Candidates** ledger (`TERMS.md` § 14, columns `CANDIDATE | What
it seems to be | Seen | Where`, each sighting as `date source`) and is promoted to a TERM row only when a
second sighting comes from a separate task (different MEMORY LOG entry, session, or commit; `git log
--since --grep -i` catches other sessions' commits). Immediate, ungated edits: a user alias for an existing
TERM, an ambiguity split, a rename, a retirement, a stale *Where*. "name this" / "add this to terms" /
"promote X" from the user promotes on the spot. Candidates whose only sighting is >30 days old are deleted
(not retired). `TERMS.md` got the § 14 section (between Retired and the Alias index, so the index stays
"at the bottom" as `CLAUDE.md` says), an intro sentence about the ledger, and a reworded PROJECT TERMS row;
`CLAUDE.md` (the "no TERM yet" sentence and the `project-terms` paragraph) and `AGENTS.md` (the "keep the
glossary true" paragraph) now describe detect-then-promote instead of add-on-sight. The loop itself
(MEMORY LOG + `TERMS.md` → PROMPT DADDY → work → NEBULA-MEMORY SKILL → PROJECT TERMS) is unchanged.

**Gotchas:**
- **The rule applies to the task that writes it.** First draft added CANDIDATES LEDGER as a full TERM in
  the same task that coined it — exactly the add-on-sight the new rule forbids, and it cross-referenced a
  not-yet-TERM in caps from the PROJECT TERMS row. It is now the ledger's first candidate row instead, with
  its two same-day sightings (prompt + this entry) counting as *one* task; the next task that mentions it
  promotes it.
- The Candidates table has four columns with different headers from every other section (`CANDIDATE`, not
  `TERM`; `Seen`, not `Also called`) and Retired has three — a row regex that assumes the TERM-table shape
  matches neither. Anchor on `| **NAME** |` and the section header.
- The Skill tool's listing re-reads the frontmatter live: the new `description` showed up in the skill
  list on the very next tool call, no restart.
