---
name: workflow-prototype
description: Start an ordered AGENT workflow from main using NEBULA WORKTREES and SESSIONS, with durable state in the SQLITE STORE. Use when the user asks to kick off a managed task, run planner then implementer, or execute the stages in .nebula/workflow.json.
---

# Start a workflow

The DAEMON creates the WORKTREE, launches the configured AGENTS, stores their results, and
advances the stages. This skill only submits the task. Read `docs/workflows.md` for recovery
or configuration details.

1. Read `.nebula/workflow.json`. Preserve the declared stage order and MODEL / EFFORT unless
   the user requested a change. Both planner and implementer use `claude-sonnet-5` with medium effort.
2. Check `nebula workflow --help`. If the installed command is older than the prototype,
   use this checkout's `target/debug/nebula` and its isolated development DAEMON. Never
   install a binary or stop the live DAEMON to resolve a VERSION SKEW without authorization.
3. Run `nebula workflow status --json`. Success means this SESSION is already assigned to
   a workflow: continue its current stage and report its result instead of starting another.
   A "does not belong" response is normal for the kickoff SESSION; other errors need resolving.
4. Write the user's task, including its constraints, to a temporary UTF-8 file. From the ROOT
   WORKTREE on `main`, run the checked executable with:

   ```sh
   nebula workflow start --task-file /absolute/path/to/task.txt \
     --definition /absolute/path/to/.nebula/workflow.json
   ```

   Use the actual paths and shell-quote them. Delete only the temporary task file you created
   after a successful response. If the request times out, inspect `nebula workflow list`
   before considering another start: WORKTREE creation may have succeeded.
5. Report the run id, WORKTREE path, configured AGENT order, and current status. The caller
   remains on `main`. The DAEMON continues independently after this SESSION ends its turn.

Workers must send `workflow report` through the executable named in their STARTING PROMPT,
as their final tool call. A completed result requires a Markdown file; the DAEMON copies its
contents into the SQLITE STORE. Report `blocked` with a concrete reason when work cannot
finish. Never infer completion from FINISHED alone or manually edit the database.

The prototype leaves all changes uncommitted. It does not authorize commits, pushes, merges,
deployments, permission bypasses, or other actions outside the user's task.
