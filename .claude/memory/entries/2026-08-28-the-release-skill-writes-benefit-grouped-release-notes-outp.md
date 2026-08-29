# The RELEASE SKILL Writes Benefit-Grouped RELEASE NOTES; OUTPUT DOCTOR Gained NEXT STEPS — 2026-08-28

**Asked:** "generate 5 variations of this release description to try and make it more readabile and a
bit marketing the successful build … ask me which ones are best" → picked 2 (short and punchy) and 5
(grouped by what it does for you) → "merge and show example" → "ok I like the merge format, update the
release skill to do that, then run a sub agent to update the output-doctor to include a ==== Next Steps
==== section that explains what is left to do for me, am I good to commit? make a pr? answer a question,
give a command, etc"
→ refined: Two changes. (1) Update the RELEASE SKILL so the changelog it writes for the GitHub RELEASE
NOTES uses the merged format: a one-line opener, features grouped under benefit headers with one emoji
each, tight bullets with a bold lead-in, fixes filed under the feature they belong to, a "Heads up" line
for the PROTOCOL VERSION bump, the INSTALL.SH one-liner last. (2) Run a subagent to update the OUTPUT
DOCTOR skill with a `==== NEXT STEPS ====` section (assuming it is always present, last, after
TECHNICAL OVERVIEW, and ACTION REQUIRED stays as the blocking-steps section; and that `CLAUDE.md` is
updated to agree).

**Did:** `.claude/skills/release/SKILL.md` §7 now specifies the RELEASE NOTES shape — `**Nebula vX.Y.Z
is out.**` opener, `###` benefit groups with one emoji each (`🚀 Launch faster`, `🔔 Know when it's
done`, `🧭 Lists that look after themselves`, `🫥 Shape the screen`), bold-lead-in bullets, fixes filed
under the feature they keep, `### ⚠️ Heads up` for the PROTOCOL VERSION / NEBULA KILL line, the install
block last — with the v0.16.0 notes condensed to two groups as the template. A subagent added the
always-present, always-last `==== NEXT STEPS ====` section to `.claude/skills/output-doctor/SKILL.md`
(its own `###` block: counts / does-not-count lists, "Nothing — this is done." as the only empty state,
pointing at ACTION REQUIRED instead of repeating it) and the matching `CLAUDE.md` § "Before you reply"
sentence; I then updated the two places it did not know about — `AGENTS.md` "Then shape the reply" and
the OUTPUT DOCTOR row of `TERMS.md` — plus the RELEASE SKILL row. The five variations themselves and the
merge live only in the session transcript; the template in §7 is the durable copy. No code changed, no
tests run (docs and skills only); `make memory-check` green.

**Gotchas:**
- **A subagent told "touch only these two files" will miss the other places a protocol lives.** The
  OUTPUT DOCTOR section list is spelled out in four files (skill, `CLAUDE.md`, `AGENTS.md`, the TERMS
  row) — the prior entry recorded exactly this and the delegation prompt still named two. Hand a
  subagent the `grep -rl '==== TECHNICAL OVERVIEW ===='` list, not a file pair; now a standing gotcha.
- A template that carries a fenced install block inside a ```` ```bash ```` example closes the outer
  fence early — the RELEASE SKILL's §7 example is a four-backtick fence for that reason (same trap the
  OUTPUT DOCTOR worked examples hit).
- The GUARD HOOK's backtick rule matches `git commit -m` only (`COMMIT_MSG_WITH_BACKTICK` in
  `.claude/hooks/guard.py`), not `gh release edit --notes "…"`; the `'EOF'`-quoted heredoc or
  `--notes-file` is the only protection there. Don't cite the hook as covering `gh`.
- The subagent reported `.claude/skills/release/SKILL.md` and `TERMS.md` as "another session's
  concurrent edits" — they were this session's, made in parallel with it. A subagent's SHARED CHECKOUT
  warning about files its parent is editing is noise, not a race.
