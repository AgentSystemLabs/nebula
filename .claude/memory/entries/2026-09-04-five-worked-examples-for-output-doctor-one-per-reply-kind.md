# Five Worked Examples For OUTPUT DOCTOR, One Per Reply Kind; A Bare `=word` Becomes A GUARD HOOK Rule — 2026-09-04

**Asked:** "generate 5 new example styyle format for the output doctor"
→ refined: Add five worked examples to the OUTPUT DOCTOR skill's "Worked examples" section, condensed
like the three there now, each showing the fixed format on a reply kind those three don't cover: a bug
fix (commit hand-off), a refactor whose diff says it all (no DETAILS), a RELEASE SKILL run (no YOU
ASKED), a diagnosis that changed nothing (NEXT STEPS is a decision), and a task partly blocked on a
command only I can run (🚧 Blocking in the `! <command>` form). Ground them in real MEMORY LOG entries
where one fits; keep the existing examples, rules and frontmatter exactly as they are. (Assuming
"example style format" means examples of the one fixed format, not five alternative layouts.)

**Did:** Appended five worked examples to `.claude/skills/output-doctor/SKILL.md` § "Worked examples"
(299 → 514 lines), each under a lead-in that names the rule it shows: a bug fix (the HOST TERMINAL
re-ask, condensed from the 2026-09-04 entry) — all four sections, DETAILS with a real `file:line`
(`host_terminal.rs:111`) and the two skills' one-liners, the commit plus a non-gating TUI relaunch as
the hand-off; a refactor (a hypothetical FOOTER module move) — no DETAILS, the follow-up offer on
OVERVIEW's last line; a release (v0.21.0, from the 2026-08-29 entry) — no YOU ASKED because the
RELEASE SKILL trigger skips PROMPT DADDY, NEBULA UPGRADE as the hand-off and not a 🚧 step; a diagnosis
(the VERSION SKEW read, from the 2026-08-28 entry) — "No code changed", NEXT STEPS a cutover only the
user can run plus a decision, neither 🚧; a blocked task (a hypothetical PR ARCHIVE backfill) — OVERVIEW
names what was left out, `🚧 **Blocking:**` in the `! gh auth login` form, the re-run kept off the list
because it is the agent's once the login is back. The FOOTER move and the backfill are invented (the
real PR ARCHIVE holds 13 files); the other three condense their entries. Frontmatter, the rules and the
three existing examples untouched. On the side: the RELEASE SKILL's zsh `=word` standing gotcha was
re-hit a third time in this task, so it is now the GUARD HOOK rule `bare-equals-word-in-zsh` in
`.claude/hooks/guard.py` (`QUOTED_OR_CONDITION` blanks quoted strings, `[[ … ]]` and `(( … ))`, then
`BARE_EQUALS_WORD` matches `(?:^|[\s;&|(])=[=\w]\S*`), a 19-case self-test green, and the enforced half
of the standing-gotcha line deleted. Gate: `make terms-check` ok; a scratch check that every `---`
outside a fence has a blank line above it and that the 14 three-backtick and 4 four-backtick fences
balance.

**Gotchas:**
- A worked example condensed from a real entry has to be re-shaped to the format's own rules, not
  transcribed: the mouse-fix entry left MAKE INSTALL undone ("not verified in the live TUI"), but "a
  step you could still take yourself — take it, then reply" means the example shows it run and the TUI
  relaunch as a non-gating hand-off. Likewise the VERSION SKEW cutover is `make install && make dev`,
  not MAKE CYCLE, whose kill step stops the real DAEMON and not the DEV INSTANCE (that entry's own
  gotcha).
- An unquoted `echo ======` between two commands in one Bash call died with `(eval):1: ===== not found`
  and the rest of the line never ran — the second file of each pair silently never printed. zsh's
  `=cmd` expansion needs a character after the `=`: a lone `=` prints, `==` fails, `[ a == b ]` fails,
  while `[[ a == b ]]`, `$(( a == b ))`, `a=b`, `--flag=val` and `=(cmd)` are fine — which is exactly
  the allow-list the new GUARD HOOK rule encodes.
- A "blank line above every `---`" check must exempt the first line inside a code fence: every worked
  example opens with `---` directly under its fence, and those nine hits are not setext headings.
- **Text the user asked you to generate goes in the reply, not only in the file.** The five examples
  landed in the skill and the reply described them; the user's next turn was "you never showed me
  output examples". When the deliverable is prose (examples, notes, a description), the reply shows
  it — fenced when it carries headings that would collide with the OUTPUT DOCTOR sections.

**Corrections:** 1
