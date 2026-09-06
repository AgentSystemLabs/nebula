---
name: nebula-workflow
description: Choose and start a named workflow from .nebula/workflows using reusable or inline AGENTS. Use when the user names a workflow, asks to kick off a managed task, or requests an ordered planning, implementation, or review sequence in NEBULA.
---

# Start a workflow

Invocation: `/nebula-workflow [workflow-selector] <task>`.

The DAEMON owns WORKTREE creation, SESSIONS, scheduling, and durable state. This skill selects
a definition and submits the task through `nebula workflow start`. Kickoff currently requires
a NEBULA AGENT SESSION in the ROOT WORKTREE on `main`; the CLI resolves the PROJECT through
that SESSION. Read `docs/workflows.md` for configuration or recovery.

1. Check `nebula workflow catalog --help`. Use this checkout's `target/debug/nebula` if the
   installed executable lacks the command. Use that same executable throughout. Check
   `workflow status --json`: if this SESSION is already assigned to a run, continue its
   current stage instead of starting a nested workflow. "Does not belong" is normal for
   the kickoff SESSION; resolve other errors before starting. A VERSION SKEW requires a
   matching DAEMON; never install or stop the user's live DAEMON without authorization.
2. Run `workflow catalog --json`. Honor an explicit filename selector. Otherwise choose the
   definition whose description and stage order fit the user's task. Prefer `default` for an
   ordinary implementation task when it exists and fits. When two plausible choices would
   change the outcome, ask one concise question. If the user supplied only a selector, ask
   for the task. Report a selected definition's validation error instead of silently
   choosing another workflow.
3. Run `workflow inspect <selector> --json`. Read the resolved stages, AGENT source paths,
   MODEL / EFFORT, and composed instructions. Preserve them unless the user asked for an
   edit. State the selected workflow and stage order in one line; no confirmation is
   required for a task the user already authorized. The optional display `name` is a label;
   the filename without `.toml` is the command selector.
4. Write the task and its constraints to a temporary UTF-8 file. From the ROOT WORKTREE on
   `main`, run the checked executable with the chosen selector:

   ```sh
   nebula workflow start --task-file /absolute/path/to/task.txt \
     --workflow <selector>
   ```

   Use the actual paths and shell-quote them. Delete only the temporary task file you created
   after a successful response. If the request times out, inspect `nebula workflow list`
   before considering another start: WORKTREE creation may have succeeded. If the catalog
   exposes a legacy JSON entry, use `--definition <its path>` for inspect and start instead.
5. Report the run id, WORKTREE path, configured AGENT order, and current status. The caller
   remains on `main`. The DAEMON continues independently after this SESSION ends its turn.

Workers must send `workflow report` through the executable named in their STARTING PROMPT,
as their final tool call. A completed result requires a Markdown file; the DAEMON copies its
contents into the SQLITE STORE. Report `blocked` with a concrete reason when work cannot
finish. Never infer completion from FINISHED alone or manually edit the database.

The prototype leaves all changes uncommitted. It does not authorize commits, pushes, merges,
deployments, permission bypasses, or other actions outside the user's task.
