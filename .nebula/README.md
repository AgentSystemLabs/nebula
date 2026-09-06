# Workflow configuration

Edit a workflow to change the sequence. Edit an AGENT to change its provider,
exact model ID, effort, or reusable instructions.

```text
.nebula/
  workflows/
    default.toml     planner -> implementer
    reviewed.toml    planner -> implementer -> reviewer
    review.toml      reviewer
  agents/
    planner.toml
    implementer.toml
    reviewer.toml
```

The filename is the selector: `reviewed.toml` means `--workflow reviewed`.
The optional `name` is a display label; `description` helps the kickoff skill choose.
`default.toml` is the default when no selector is supplied.

From the repository root, inspect the configuration without starting SESSIONS:

```sh
./target/debug/nebula workflow catalog
./target/debug/nebula workflow inspect reviewed
```

Start from an AGENT in the ROOT WORKTREE on `main`:

```text
/nebula-workflow reviewed <your task>
```

The skill selects and submits through `nebula workflow start`; calling that CLI directly
also works inside a NEBULA AGENT SESSION. Kickoff from an ordinary terminal is not yet
supported. `catalog` and `inspect` work from either context.

Stages can reference an AGENT file or define an inline table. Stage `model` and
`effort` override the AGENT's values; values absent from both use NEBULA defaults. Stage
`instructions` are appended to the AGENT's reusable instructions. Inspect shows
the final values and source paths.

The DAEMON freezes the resolved configuration in the SQLITE STORE at kickoff.
File edits affect new runs. Review findings stop at a blocker; there is no automatic
review/fix loop. See [the full guide](../docs/workflows.md) for inline examples,
validation, legacy JSON, and recovery.
