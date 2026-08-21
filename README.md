# nebula

A fast, low-memory terminal multiplexer for managing Claude Code agents across
projects and git worktrees. Think tmux ergonomics with a mission-control-style
agent manager — entirely inside your terminal.

```
┌ Projects ─┬ Worktrees ─┬ Sessions ────┬ Terminal ──────────────────────┐
│ ● nebula  │ ● main     │ AGENTS       │ $ claude                       │
│ ○ herdr   │ ● feat/x   │ ● auth-bot   │ …live claude session…          │
│           │            │ ● refactor   │                                │
│           │            │ + new agent… │                                │
├───────────┴────────────┴──────────────┴────────────────────────────────┤
│ ⏻ connected │ n: agent  r: rename  a: archive  m: menu  ?: help        │
└─────────────────────────────────────────────────────────────────────────┘
```

## Install

macOS or Linux — the same command installs and updates:

```
curl -fsSL https://raw.githubusercontent.com/AgentSystemLabs/nebula/main/install.sh | sh
```

Downloads the prebuilt binary for your platform from the latest GitHub release
into `~/.local/bin` (override with `NEBULA_INSTALL_DIR`), falling back to
`cargo install --git` when no release matches.

Once installed, `nebula upgrade` runs that same script for you — no need to
remember the URL. It refuses to clobber a local `cargo build` (pass `--force`
if you mean it). Upgrading while a daemon is running is safe: sessions keep
running on the old binary until you run `nebula kill` (which stops all
sessions) and relaunch.

## How it works

- **Detached daemon (tmux-style).** A background `nebula` daemon owns every
  PTY, so agents keep running when the TUI closes. The TUI is a client that
  attaches over a unix socket (`$XDG_RUNTIME_DIR/nebula/` or
  `/tmp/nebula-<uid>/`, mode 0700). Quit the TUI, relaunch later, and your
  sessions are still alive with scrollback replayed.
- **Projects → worktrees → sessions.** All work happens in the main checkout
  or a git worktree. Worktrees are real (`git worktree add/remove`), created
  under `<repo>/../<repo-name>-worktrees/<branch>`.
- **Agents boot `claude`, `codex`, or `cursor-agent`.** Creating an agent
  (`n`) first asks which CLI to run, then spawns it in the worktree. Restored
  agents resume with `claude --resume <session-id>` / `codex resume
  <session-id>` / `cursor-agent --resume <session-id>` (falling back to a
  fresh session when the old one is gone).
- **Status via agent-CLI hooks, not MCP.** At agent spawn, nebula merges
  managed hooks into the worktree's `.claude/settings.local.json` (Claude
  Code), `.codex/hooks.json` (Codex; loads once the project is trusted and
  the hooks approved on first run), or `.cursor/hooks.json` (Cursor CLI).
  Groups are tagged `_nebulaManaged`, user
  hooks preserved, rebuilt each spawn. Each hook is a fail-soft curl to the
  daemon's loopback HTTP endpoint, authenticated with a per-boot bearer token
  injected into the agent's environment only.
- **Sessions title themselves.** Create a session with the default name and
  the agent renames it after your first prompt — a 3-4 word title describing
  the ask (e.g. `Fix Login Redirect`), via a new `nebula rename <title>`
  command the CLI runs in its own turn (no extra API calls, no MCP server).
  Claude Code and Codex get the instruction injected through the
  `UserPromptSubmit` hook response (the daemon sends it only while the
  session is untitled); Cursor gets a managed `.cursor/rules/nebula-title.mdc`
  project rule instead, since its hooks can't inject context. Titling is
  one-shot and never clobbers a name you typed or set with `r` — a late
  agent attempt is politely declined. `nebula rename --force` overrides.
- **Everything persists in SQLite** (`~/.local/share/nebula/nebula.db` or the
  platform equivalent): projects, worktrees, agents (with kind + CLI session
  ids), and your last selection.

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
| Panels | `Tab`/`Shift+Tab`, `←/→`, `j/k/l` | move focus / selection |
| Panels | `Enter` | drill in; on a session: attach |
| Projects | `n` / `d` | add project / remove from list |
| Any panel | `o` | add ("open") a project — same prompt as `n`, from any focus |
| Add project | type + `Tab`, `↓↑` / `→` / `←` | browse for the repo: type to filter (bash-style Tab completion), arrows pick a directory, `→` steps in, `←` steps up, `Enter` adds the highlighted (or typed) path; `●` marks git repos |
| Projects | `Shift+J/K` | move project up / down the list (`Shift+↑/↓` too, but Terminal.app never sends those) |
| Projects | `-` | toggle a group divider below the project |
| Projects | `j/k` onto a divider, then `Enter`/`r` | edit the divider's label |
| Projects | `d` or `-` on a divider | delete the divider |
| Worktrees | `n` / `d` | new worktree / delete (typed confirm — deletes files) |
| Projects / Worktrees | `e` | notes for the selected project / worktree |
| Sessions | `n` | new session (agent or shell terminal) |
| Sessions | `r`, `a`, `u`, `d`, `A` | rename, archive, unarchive, delete, toggle archived |
| Any panel | `t` | new shell terminal in the selected worktree's directory (Projects panel: the repo root) |
| Any panel | `w` | workspace switcher: `Enter` opens, `n`/`r`/`d` create/rename/delete (the open workspace shows bottom-left; `/` and the panels scope to it) |
| Any panel | `h` | ssh hosts: every `nebula ssh` destination, newest first. `Enter`/click reconnects (quits this TUI and execs a fresh `nebula ssh` — local sessions keep running), `a` types a new `user@host [dir]`, `d` removes |
| Any panel | `m` or right-click | context menu |
| Any panel | `z` | collapse sidebars (full-width terminal) |
| Any panel | `Shift+M` | memory usage: RAM per agent/terminal process tree, nebula itself, and the machine-wide share; `↑/↓` + `Enter` opens the selected session |
| Any panel | `?` | help overlay |
| Terminal | anything | forwarded raw to the PTY |
| Terminal | `Ctrl+q` | back to panels (also expands sidebars) |
| Terminal | mouse wheel | scrollback (arrow keys on alt-screen apps) |

Mouse: left-click selects/attaches, right-click opens context menus. Text
selection: hold `Shift` while dragging (mouse capture bypass — same as tmux).

## Commands

```
nebula                    # launch the TUI (auto-starts the daemon)
nebula add <dir>          # add a repo as a project, named after its root directory
nebula add .              # same, for the repo you're in (bare `nebula <dir>` / `nebula .` also work)
nebula daemon             # run the daemon (normally auto-spawned)
nebula daemon --foreground  # daemon with logs to stderr, for debugging
nebula kill               # stop the daemon and all sessions cleanly
nebula rename <title>     # title the current session (agents run this; --force to retitle)
nebula workspace add <name>     # create a workspace (a named project group)
nebula workspace open <name>    # open it — projects (and the TUI, live) scope to it
nebula workspace list           # list workspaces; * marks the open one
nebula workspace rename <a> <b> # rename a workspace
nebula workspace delete <name>  # delete an empty workspace
nebula ssh <host> [dir]   # open nebula on a remote machine over ssh (installs it there if
                          # missing); destinations are remembered for the TUI's `h` picker
nebula upgrade            # install the latest release (--force on a dev build)
```

Logs: `~/.local/state/nebula/daemon.log` and `tui.log` (`NEBULA_LOG=debug` for
more). Overrides for tests/parallel instances: `NEBULA_RUNTIME_DIR`,
`NEBULA_DATA_DIR`, `NEBULA_AGENT_CMD`, `NEBULA_INSTALL_URL`.

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
