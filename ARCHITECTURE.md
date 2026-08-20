# How Nebula works

Nebula is a tmux-style terminal multiplexer for AI coding agents. You run multiple Claude / Codex / Cursor CLI sessions across git repos and worktrees, and they keep running after you close the UI.

## Process model

There are two processes, same binary:

1. **Daemon** (`nebula daemon`) — owns every PTY, SQLite, git worktrees, and agent status. Lives in the background.
2. **TUI** (`nebula`) — a ratatui client. Quit it and nothing dies; relaunch and scrollback is replayed.

On launch the TUI connects to a unix socket (`$XDG_RUNTIME_DIR/nebula/daemon.sock`, mode `0700`). If nothing is listening, it spawns `nebula daemon` in a new process group so the daemon outlives the client and does not get Ctrl+C.

IPC is length-prefixed MessagePack: the client sends `ClientRequest`s (CRUD, attach, keystrokes, resize); the daemon pushes `ServerEvent`s (entity deltas, status, PTY output).

## Domain tree

Everything is nested:

**Project** (a git repo) → **Worktree** (main checkout or `git worktree add`) → **Session** (an agent *or* a plain terminal tab).

Worktrees are real git worktrees, created under `<repo>/../<repo-name>-worktrees/<branch>`. The daemon also polls git metadata so worktrees created outside Nebula still show up.

An agent is a PTY running `claude`, `codex`, or `cursor-agent` in that worktree. Restart uses `--resume <session-id>` when one is stored.

Persistence is SQLite at `~/.local/share/nebula/nebula.db`: projects, worktrees, agents (kind + CLI session id), last UI selection.

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

## Crate layout

| Crate | Role |
|---|---|
| `nebula` | Thin CLI: no args → TUI; `daemon`, `kill-server`, `upgrade`, `ssh` |
| `nebula-core` | Shared protocol, entities, IDs, paths, codec |
| `nebula-daemon` | PTYs, SQLite, git, hook receiver, status engine |
| `nebula-tui` | ratatui UI, keyboard/mouse, attach/scrollback |

The TUI also has extras on top of the multiplexer: git diff viewer, grep, a vim-like terminal overlay, fuzzy finders — those are client-side. The daemon is the source of truth for sessions and the tree.

**Mental model:** tmux, but the “windows” are agent CLIs bound to git worktrees, and the sidebar is a mission-control view of which agents are working, waiting, or dead.
