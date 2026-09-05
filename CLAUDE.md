@AGENTS.md

# Claude Code

The SELF-IMPROVING LOOP above is the protocol. In Claude Code the four steps are skills — invoke
them with the Skill tool instead of reading their `SKILL.md`, and each one's body carries its own
rules:

| When | Invoke | Skip when |
|---|---|---|
| every new prompt that is a task, before planning or grepping | `Skill(skill: "prompt-daddy")` | the skill's own skip list: a reply to a question you asked, a bare confirmation, a specific mid-task correction, a slash-command or skill trigger, a pure question that changes nothing, a plain git/`gh` housekeeping ask ("commit push and merge") |
| the task changed code or behavior, diagnosed a bug, or surfaced something non-obvious | `Skill(skill: "nebula-memory")` | pure questions, trivial edits that held no surprise, and git/`gh` housekeeping of finished work that surfaced nothing |
| every task, right after `nebula-memory` | `Skill(skill: "project-terms")` | git/`gh` housekeeping of finished work (a `land` run) |
| before writing the reply that answers or closes the request | `Skill(skill: "output-doctor")` | never — a pure question takes its short form, not no form |
| a landing chore — "commit push and merge", "make pr", "fix conflicts … babysit … merge" | `Skill(skill: "land")` — it replaces the three rows above it for that prompt | the change itself is not finished, or the user asks for a PR *description* (PR DESCRIPTION SKILL) |

Three hooks run on their own:

- **RECALL HOOK** (`.claude/hooks/recall.py`, `UserPromptSubmit`) scores the prompt against the
  MEMORY LOG and injects the matching entries and standing gotchas as `[nebula recall] …`. Read it
  as you would the files themselves; it is what actually reaches you, so an entry it did not surface
  may still exist — grep when the subject is one the log is likely to have.
- **GUARD HOOK** (`.claude/hooks/guard.py`, `PreToolUse` on Bash) blocks commands past gotchas
  turned into rules. If it blocks you, do what it says.
- **SKILL AUDIT HOOK** (`.claude/hooks/skill_audit.py`, `Stop`) fires once per closed task — after a
  turn in which `nebula-memory` ran (`NEBULA_SKILL_AUDIT_COOLDOWN_MIN` spaces audits out) — and keeps the turn
  going with a brief: the skills this session invoked and their sizes. Answer it from your own context
  of the turn: what in each body you followed, what you did not need, what it cost against what it
  changed; propose at most three cuts, merges, tightened rules or a new skill as one
  `AskUserQuestion`; apply exactly what the user picks. Unattended, write the proposals to
  `.claude/memory/skill-audit/` instead. It is not a task: no entry, no terms pass, no OUTPUT DOCTOR
  layout. `NEBULA_SKILL_AUDIT=off` silences it.
