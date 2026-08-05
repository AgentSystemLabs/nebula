# nebula

A fast, low-memory terminal multiplexer for managing Claude Code agents across
projects and git worktrees. Think tmux ergonomics with a mission-control-style
agent manager — entirely inside your terminal.

```
┌ Projects ─┬ Worktrees ─┬ Sessions ────┬ Terminal ──────────────────────┐
│ ● nebula  │ ● main     │ AGENTS       │ $ claude                       │
│ ○ herdr   │ ● feat/x   │ ● auth-bot   │ …live claude session…          │
│           │            │ ● refactor   │                                │
│           │            │ ──────────── │                                │
│           │            │ TERMINALS    │                                │
│           │            │ ○ term-1     │                                │
├───────────┴────────────┴──────────────┴────────────────────────────────┤
│ ⏻ connected │ n: agent  t: term  r: rename  a: archive  m: menu  ?: help│
└─────────────────────────────────────────────────────────────────────────┘
```

## Install

macOS or Linux — the same command installs and updates:

```
curl -fsSL https://raw.githubusercontent.com/webdevcody/nebula/main/install.sh | sh
```

Downloads the prebuilt binary for your platform from the latest GitHub release
into `~/.local/bin` (override with `NEBULA_INSTALL_DIR`), falling back to
`cargo install --git` when no release matches. Updating while a daemon is
running is safe: sessions keep running on the old binary until you run
`nebula kill-server` (which stops all sessions) and relaunch.

## How it works

- **Detached daemon (tmux-style).** A background `nebula` daemon owns every
  PTY, so agents keep running when the TUI closes. The TUI is a client that
  attaches over a unix socket (`$XDG_RUNTIME_DIR/nebula/` or
  `/tmp/nebula-<uid>/`, mode 0700). Quit the TUI, relaunch later, and your
  sessions are still alive with scrollback replayed.
- **Projects → worktrees → sessions.** All work happens in the main checkout
  or a git worktree. Worktrees are real (`git worktree add/remove`), created
  under `<repo>/../<repo-name>-worktrees/<branch>`.
- **Agents boot `claude`.** Creating an agent spawns a Claude Code session in
  the worktree. Restored agents resume with `claude --resume <session-id>`
  (falling back to a fresh session when the old one is gone).
- **Status via Claude Code hooks, not MCP.** At agent spawn, nebula merges
  managed hooks into the worktree's `.claude/settings.local.json` (tagged
  `_nebulaManaged`, user hooks preserved, rebuilt each spawn). Each hook is a
  fail-soft curl to the daemon's loopback HTTP endpoint, authenticated with a
  per-boot bearer token injected into the agent's environment only.
- **Everything persists in SQLite** (`~/.local/share/nebula/nebula.db` or the
  platform equivalent): projects, worktrees, agents (with claude session ids),
  terminals, and your last selection.

## Status dots

| Dot | Meaning |
|---|---|
| ● gray | fresh — agent never run |
| ● yellow | running — turn in progress (Stop is gated on active subagents) |
| ● green | finished — turn complete |
| ● red | needs feedback — permission prompt or question waiting on you |
| ● magenta | terminated — process died mid-run |
| ○ | disconnected — daemon restarted while the agent was live |

Worktree and project rows roll up their children: red beats yellow beats green.

## Keys

| Context | Key | Action |
|---|---|---|
| Panels | `Tab`/`Shift+Tab`, `h/l`, `j/k` | move focus / selection |
| Panels | `Enter` | drill in; on a session: attach |
| Projects | `n` / `d` | add project / remove from list |
| Worktrees | `n` / `d` | new worktree / delete (typed confirm — deletes files) |
| Sessions | `n` / `t` | new agent / new terminal |
| Sessions | `r`, `a`, `u`, `d`, `A` | rename, archive, unarchive, delete, toggle archived |
| Any panel | `m` or right-click | context menu |
| Any panel | `z` | collapse sidebars (full-width terminal) |
| Any panel | `?` | help overlay |
| Terminal | anything | forwarded raw to the PTY |
| Terminal | `Ctrl+q` | back to panels (also expands sidebars) |
| Terminal | mouse wheel | scrollback (arrow keys on alt-screen apps) |

Mouse: left-click selects/attaches, right-click opens context menus. Text
selection: hold `Shift` while dragging (mouse capture bypass — same as tmux).

## Commands

```
nebula                    # launch the TUI (auto-starts the daemon)
nebula daemon             # run the daemon (normally auto-spawned)
nebula daemon --foreground  # daemon with logs to stderr, for debugging
nebula kill-server        # stop the daemon and all sessions cleanly
```

Logs: `~/.local/state/nebula/daemon.log` and `tui.log` (`NEBULA_LOG=debug` for
more). Overrides for tests/parallel instances: `NEBULA_RUNTIME_DIR`,
`NEBULA_DATA_DIR`, `NEBULA_AGENT_CMD`.

## Building

```
cargo build --release     # → target/release/nebula (~4 MB)
cargo test                # unit + end-to-end suite (spawns real daemons/PTYs)
```

Workspace layout: `nebula-core` (shared protocol/entities), `nebula-daemon`
(PTYs, SQLite, hook receiver, status engine), `nebula-tui` (ratatui client),
`nebula` (the binary).

Releases: push a `v*` tag (`git tag v0.1.0 && git push --tags`) and CI builds
mac (arm/intel) and linux (x64/arm64, static musl) binaries and attaches them
to a GitHub release — which is what `install.sh` downloads.
