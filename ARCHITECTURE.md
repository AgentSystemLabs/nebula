# How Nebula works

Nebula is a tmux-style terminal multiplexer for AI coding agents. You run multiple Claude / Codex / Cursor CLI sessions across git repos and worktrees, and they keep running after you close the UI.

## Process model

There are two processes, same binary:

1. **Daemon** (`nebula daemon`) — owns every PTY, SQLite, git worktrees, and agent status. Lives in the background.
2. **TUI** (`nebula`) — a ratatui client. Quit it and nothing dies; relaunch and scrollback is replayed.

On launch the TUI connects to a unix socket (`$XDG_RUNTIME_DIR/nebula/daemon.sock`, mode `0700`). If nothing is listening, it spawns `nebula daemon` in its own session (`setsid`) so the daemon outlives the client, does not get Ctrl+C, and holds no controlling terminal — daemon subprocesses that run the user's interactive shell must not be able to reach the TUI's tty via `/dev/tty`.

IPC is length-prefixed MessagePack: the client sends `ClientRequest`s (CRUD, attach, keystrokes, resize); the daemon pushes `ServerEvent`s (entity deltas, status, PTY output).

## Domain tree

Everything is nested:

**Workspace** (a named project group) → **Project** (a git repo) → **Worktree** (main checkout or `git worktree add`) → **Session** (an agent *or* a plain terminal tab).

Exactly one workspace is *open* at a time — daemon-global state, switched with `nebula workspace open <name>` or the TUI's `w` picker and broadcast to every client. The TUI scopes its Projects panel and `/` search to the open workspace; other workspaces' sessions keep running (and keep receiving status updates) in the background. Every install starts with the built-in `default` workspace, and `nebula add` files new projects under whichever workspace is open.

Worktrees are real git worktrees, created under `<repo>/../<repo-name>-worktrees/<branch>`. The daemon also polls git metadata so worktrees created outside Nebula still show up.

An agent is a PTY running `claude`, `codex`, or `cursor-agent` in that worktree. Restart uses `--resume <session-id>` when one is stored.

Persistence is SQLite at `~/.local/share/nebula/nebula.db`: workspaces (one flagged open), projects, worktrees, agents (kind + CLI session id), todos, last UI selection.

Projects and worktrees each carry their own **todo list**: plain notes with a done flag, edited in the TUI's `t` modal (Projects panel → the project's high-level notes; elsewhere → the selected worktree's notes) and counted as a badge on the owning row.

## How the pieces talk

```
┌──────────── TUI (ratatui) ────────────┐
│  panels: projects / worktrees / sessions │
│  attached terminal: vt100 parser + PTY   │
└───────────────┬───────────────────────┘
                │ unix socket
┌───────────────▼───────────────────────┐
│  Daemon                                 │
│  ┌ registry ┐  ┌ PTY ring buffers ┐   │
│  │ SQLite   │  │ portable-pty     │   │
│  └──────────┘  └──────────────────┘   │
│  ┌ hook HTTP (loopback) ──────────┐   │
│  │ claude/codex/cursor POSTs      │   │
│  │ → status state machine         │   │
│  └────────────────────────────────┘   │
└───────────────────────────────────────┘
```

**Attach path:** TUI sends `Attach { session, from_seq, cols, rows }`. Daemon replays the PTY ring as `Scrollback`, then streams live `Output`. Keystrokes go the other way as `Input`. Detach does not kill the child.

**Status path (not MCP):** at spawn, Nebula writes managed hooks into the worktree (`.claude/settings.local.json`, `.codex/hooks.json`, or `.cursor/hooks.json`). Those hooks `curl` a loopback HTTP server with a per-boot bearer token. Events like `UserPromptSubmit`, `Stop`, `PermissionRequest`, `SubagentStart` feed a status machine that maps to the colored dots (running / finished / needs feedback / …). Stop is gated on active subagents so a turn is not marked done while workers are still going. Claude and Codex share one hooks dialect; Cursor speaks its own (camelCase events like `beforeSubmitPrompt`/`stop`, flat `{"command"}` entries, JSON replies on stdout), so its installer translates event names into the `hookEvent` query param and the receiver aliases its payload fields (`conversation_id` → session id, first `workspace_roots` entry → cwd). Cursor has no permission-request hook and runs with `--force`, so cursor agents report busy/idle but never needs-feedback.

**Auto-title path (hooks again, still not MCP):** a session created with the default `agent-N` name carries a store-only `auto_title_pending` flag. While it's set, the daemon answers the Claude/Codex `UserPromptSubmit` hook POST with an instruction body instead of the usual discarded JSON — the installer's `UserPromptSubmit` command (alone among the hooks) pipes the response to stdout, which those CLIs add to the model's context. The instruction tells the agent to run `nebula rename <3-4 word title>` once; that subcommand resolves the agent from `NEBULA_AGENT_ID`, does a one-shot IPC `AutoRenameAgent`, and the daemon applies it only while the flag is still pending (atomic conditional update), so a user rename — which clears the flag — always wins and repeated attempts get a polite "already titled" error. Claude also gets a `Bash(nebula rename:*)` entry merged into `permissions.allow` so the command runs unprompted; Codex/Cursor already run with `--yolo`/`--force`. Cursor's hooks can't inject context, so it gets a managed, env-guarded `.cursor/rules/nebula-title.mdc` project rule carrying the same instruction — safe to fire repeatedly because the daemon-side flag is the arbiter.

**Metrics path:** the memory modal (`Shift+M`) asks the daemon for one reading (`GetMetrics` → `Metrics`). The daemon runs a single machine-wide `ps` sweep and sums RSS over each live session's process subtree (the PTY child plus its descendants — an agent CLI fans out into workers and MCP servers), reporting itself separately since sessions are its own descendants. The TUI adds its own RSS client-side (it is not a daemon child) and re-polls every 2s while the modal is open.

## Crate layout

| Crate | Role |
|---|---|
| `nebula` | Thin CLI: no args → TUI; `daemon`, `kill`, `rename`, `upgrade`, `ssh` |
| `nebula-core` | Shared protocol, entities, IDs, paths, codec |
| `nebula-daemon` | PTYs, SQLite, git, hook receiver, status engine |
| `nebula-tui` | ratatui UI, keyboard/mouse, attach/scrollback |

The TUI also has extras on top of the multiplexer: git diff viewer, grep, a vim-like terminal overlay, fuzzy finders — those are client-side. The daemon is the source of truth for sessions and the tree.

**Mental model:** tmux, but the “windows” are agent CLIs bound to git worktrees, and the sidebar is a mission-control view of which agents are working, waiting, or dead.
