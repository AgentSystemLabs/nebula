@AGENTS.md

# Claude Code

The SELF-IMPROVING LOOP above is the protocol. In Claude Code the four steps are skills — invoke
them with the Skill tool instead of reading their `SKILL.md`, and each one's body carries its own
rules:

| When | Invoke | Skip when |
|---|---|---|
| every new prompt that is a task, before planning or grepping | `Skill(skill: "prompt-daddy")` | the skill's own skip list: a reply to a question you asked, a bare confirmation, a specific mid-task correction, a slash-command or skill trigger, a pure question that changes nothing |
| the task changed code or behavior, diagnosed a bug, or surfaced something non-obvious | `Skill(skill: "nebula-memory")` | pure questions, and trivial edits that held no surprise |
| every task, right after `nebula-memory` | `Skill(skill: "project-terms")` | never |
| before writing the reply that answers or closes the request | `Skill(skill: "output-doctor")` | never — a pure question takes its short form, not no form |

Two hooks run on their own:

- **RECALL HOOK** (`.claude/hooks/recall.py`, `UserPromptSubmit`) scores the prompt against the
  MEMORY LOG and injects the matching entries and standing gotchas as `[nebula recall] …`. Read it
  as you would the files themselves; it is what actually reaches you, so an entry it did not surface
  may still exist — grep when the subject is one the log is likely to have.
- **GUARD HOOK** (`.claude/hooks/guard.py`, `PreToolUse` on Bash) blocks commands past gotchas
  turned into rules. If it blocks you, do what it says.
