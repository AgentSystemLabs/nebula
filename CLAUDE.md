@AGENTS.md

# Claude Code

The SELF-IMPROVING LOOP above is the protocol. In Claude Code the four steps are skills — invoke
them with the Skill tool instead of reading their `SKILL.md`, and each one's body carries its own
rules:

| When | Invoke | Skip when |
|---|---|---|
| a new task prompt the RECALL HOOK could not settle: a word that maps to two TERMS, a spec hanging on one word ("done", "new", "move", "fix it"), a bug with no evidence, a visual ask with no target, a change that never says what stays | `Skill(skill: "prompt-daddy")` | the prompt already names what to change, what to keep and why — work from it as written (2026-09-06: the A/B showed a rewrite of a clear prompt costs ~$2 and asks nothing) — plus the skill's own skip list: a reply to a question you asked, a bare confirmation, a mid-task correction, a skill trigger, a pure question, a git/`gh` housekeeping ask |
| the task surfaced a gotcha, a diagnosed cause, a decision not to relitigate, or a non-obvious fact about this repo, the DAEMON, the TUI or the hook dialects | `Skill(skill: "nebula-memory")` | a code change that held no surprise, however large — the diff and `git log` already record it (2026-09-06: the closing chores were ~$3 of a $21 task); pure questions; git/`gh` housekeeping |
| right after `nebula-memory`, when the user used a word for a thing that has no TERM or that its row does not list, or the task added, renamed, moved or removed something people will name out loud | `Skill(skill: "project-terms")` | no new vocabulary surfaced (most tasks), and git/`gh` housekeeping of finished work (a `land` run) |
| before writing the reply that answers or closes the request | `Skill(skill: "output-doctor")` | never — a pure question takes its short form, not no form |
| a landing chore — "commit push and merge", "make pr", "fix conflicts … babysit … merge" | `Skill(skill: "land")` — it replaces the three rows above it for that prompt | the change itself is not finished |
| **a pull request is about to be created, or its body rewritten** — inside `land` ("make pr", "commit push and merge", "open a PR") or on any other path | `Skill(skill: "pr-description")` before `gh pr create` — every PR body is the house style: TOC, screenshots, mermaid diagram, risk read, technical overview | never — a short hand-written body is not a shape (2026-09-05: "make pr" shipped one and the user asked why) |

Three hooks run on their own:

- **RECALL HOOK** (`.claude/hooks/recall.py`, `UserPromptSubmit`) scores the prompt against the
  MEMORY LOG and injects the matching entries and standing gotchas as `[nebula recall] …`. Read it
  as you would the files themselves; it is what actually reaches you, so an entry it did not surface
  may still exist — grep when the subject is one the log is likely to have.
- **GUARD HOOK** (`.claude/hooks/guard.py`, `PreToolUse` on Bash) blocks commands past gotchas
  turned into rules. If it blocks you, do what it says.
- **SKILL AUDIT HOOK** (`.claude/hooks/skill_audit.py`, `Stop`) fires after a turn in which
  `nebula-memory` ran, at most once a week by default (`NEBULA_SKILL_AUDIT_COOLDOWN_MIN`, 0 for every
  closed task; the clock lives in `/tmp`), and keeps the turn going with a brief: the skills this
  session invoked and their sizes. Answer it from your own context of the turn: what in each body you
  followed, what you did not need, what it cost against what it changed; propose at most three cuts,
  merges, tightened rules or a new skill as one `AskUserQuestion`; apply exactly what the user picks.
  Unattended, write the proposals to `.claude/memory/skill-audit/` instead. It is not a task: no entry,
  no terms pass, no OUTPUT DOCTOR layout. `NEBULA_SKILL_AUDIT=off` silences it.
