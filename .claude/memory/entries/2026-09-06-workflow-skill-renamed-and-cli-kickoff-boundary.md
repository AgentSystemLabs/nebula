# MANAGED WORKFLOW Skill Rename And CLI Kickoff Boundary - 2026-09-06

**Asked:** "By the way lets change the skill name to be proper now leike /nebula-workflow or something ... is ti possible to kickoff worflows only from nebula cli? or do we need to invoke from an agent?"
→ refined: Rename the MANAGED WORKFLOW kickoff skill to `/nebula-workflow`, update its shared skill link and current documentation, and explain whether `nebula workflow start` can run from an ordinary terminal or requires an AGENT SESSION. Preserve the current execution behavior.

**Did:** Renamed the source directory and frontmatter to `.claude/skills/nebula-workflow`, replaced its `.agents/skills` symlink, and updated invocation examples and TERMS. `docs/workflows.md` now distinguishes direct CLI calls from skill invocation and lists which commands require an AGENT SESSION. Runtime directories and Rust behavior are unchanged. Manually verified the new name and link, then ran the built CLI with `NEBULA_AGENT_ID` removed and isolated data/runtime paths: start rejected the missing AGENT context before creating a DAEMON; inspect succeeded without one. No provider run, DAEMON restart, or Rust suite repeat was needed.

**Gotchas:**
- The project skill's directory name determines its slash command, so renaming only YAML `name` is insufficient. Both the source directory and the shared `.agents/skills` link must move; the old skill path is removed. Historical references name the replacement.
- The skill is optional, but kickoff still resolves its PROJECT through the registered caller SESSION in `start_workflow`, not the CLI's working directory. `caller()` requires `NEBULA_AGENT_ID`; the DAEMON rejects archived or workflow-assigned callers and requires their ROOT WORKTREE on `main`. Local inspection succeeding outside NEBULA does not imply standalone kickoff is supported.
