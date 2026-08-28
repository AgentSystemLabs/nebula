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

Persistence is SQLite at `~/.local/share/nebula/nebula.db`: workspaces (one flagged open), projects, worktrees, agents (kind + CLI session id, plus a PR URL when the AGENT was created from one), links, last UI selection.

Worktrees can still carry a persisted **link list** from earlier versions: URLs pinned to a checkout and normalized to http(s) on the way in. The TUI no longer exposes manual link creation, but keeps existing rows visible and editable so no stored data disappears. The Sessions panel presents those rows under OPEN PRS with the row nothing stores: the pull request on that branch, looked up client-side with `gh pr view` on the git-poll tick and cached per worktree. The detected row opens in the browser but can't be edited or deleted — it comes back from git on the next lookup. A saved link that matches the detected PR is shown once, as the pull-request row.

The WORKTREES PANEL's PROJECT OPEN PRS GROUP has a separate creation path: `n`, `m`, or right-click can create a local Claude SESSION for the selected PR. Because that row has no checkout of its own, the AGENT starts in the PROJECT's ROOT WORKTREE. The TUI sends `CreatePrAgent` with the PR URL; the daemon validates and stores it, refuses to adopt a PREWARM POOL process that started without the constraint, and composes the URL plus the PR-only work rule into Claude's existing `--append-system-prompt`. Every later cold spawn or RESUME rebuilds the same system prompt from SQLite.

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

**Remote hosts path:** `nebula ssh host [dir]` execs `ssh -t` with a self-installing remote command, and first records the destination in `~/.local/share/nebula/ssh_hosts.json` (newest first, capped at 20, keyed by host + start dir). The TUI's `h` picker lists that file; choosing an entry — or typing a new `user@host [dir]` with `a` — quits the TUI cleanly (UI state saved, terminal restored) and hands the destination back to the binary, which execs a fresh `nebula ssh` over the same terminal. The local daemon and its sessions keep running; exiting the remote nebula lands back in the local shell. `d` in the picker forgets an entry.

**Tunnel path:** `nebula tunnel host [dir]` is the remote-hosts path and the browser path joined into one command: a single `ssh -tt -L 127.0.0.1:<local>:127.0.0.1:<remote>` whose remote command is the same self-installing prelude with a different tail — `nebula browser --no-open --port <remote> >/dev/null`. The remote stays on its own loopback, so the ssh channel is the only way in and there is no port to firewall and no ttyd password to set; `--no-open` because the desktop that should get the tab is the local one, and the redirect because the remote's own "serving on …" names a port that means nothing here (its stderr is kept — the install progress, a missing ttyd, and a port clash are the whole diagnosis). Before starting anything the remote command asks its own loopback port whether a ttyd is already there (`curl -I`, matched on the `server: ttyd/…` header, which a 401 behind `--credential` carries too); if so — a `nebula browser` the user left serving on that box, typically a `--public` one — it says so and `exec`s a long sleep instead of a second server, so the forward reaches the existing one and Ctrl+C or hang-up still ends the session. The probe is plain shell, so it needs nothing new from the remote's nebula. Unlike `nebula ssh` this spawns rather than execs, because work remains after the connection: the local port is settled first (same rules as `nebula browser`, so the printed URL and the forward agree), then the tunnel is polled with a real `GET /` — a bare connect proves nothing, since ssh accepts on the forwarded port from the moment the session is up and only then discovers the far end is refusing — and the URL is opened once bytes come back. Each poll before the remote is listening is a refused channel OpenSSH logs, so ssh's stderr is filtered through a thread that drops exactly that line and keeps the rest. The pty is forced (`-tt`, not `-t`) so hanging up reaps the remote ttyd and its TUI; `-t` allocates nothing when stdin is not a terminal, which is precisely when nobody is watching for orphans. Destinations land in the same `ssh_hosts.json` `nebula ssh` writes, so a host tunnelled into shows up in the `h` picker (which reconnects over `nebula ssh`, not the tunnel).

**Browser path:** `nebula browser` shells out to [ttyd](https://github.com/tsl0922/ttyd) rather than serving anything itself — ttyd runs a command in a PTY and bridges it to xterm.js in the page, so pointing it at this binary (`current_exe`, not whatever `nebula` resolves to on PATH) puts the real TUI in a browser tab, sidebar and all. The command polls `127.0.0.1:<port>` until it accepts, opens the URL, then blocks on ttyd; Ctrl+C reaches both through the shared process group. It binds loopback and stays unauthenticated by default — ttyd ships no auth and what it serves is a live terminal, so local access needs neither and remote access is normally `ssh -L` or a tunnel. `--bind ADDR` / `--public` widen the bind for the case a tunnel cannot cover — nebula running *on* the remote box, where the access control is the firewall or security group in front of the port — and `--credential USER:PASSWORD` adds ttyd's HTTP basic auth. Any non-loopback bind warns on stderr, twice as loudly without a credential. Widening also moves the readiness poll and the opened URL off `127.0.0.1`: the free-port probe asks the address ttyd will actually bind (a port free on one interface may be taken on another), and an unspecified bind (`0.0.0.0`/`::`) is polled and opened on loopback of its family, since it names every interface rather than a destination. The daemon is uninvolved: this is a second TUI client like any other, so it obeys the same one-open-workspace rule.

## Crate layout

| Crate | Role |
|---|---|
| `nebula` | Thin CLI: no args → TUI; `daemon`, `kill`, `rename`, `upgrade`, `ssh`, `browser` |
| `nebula-core` | Shared protocol, entities, IDs, paths, codec |
| `nebula-daemon` | PTYs, SQLite, git, hook receiver, status engine |
| `nebula-tui` | ratatui UI, keyboard/mouse, attach/scrollback |

The TUI also has extras on top of the multiplexer: git diff viewer, grep, a vim-like terminal overlay, fuzzy finders — those are client-side. The daemon is the source of truth for sessions and the tree.

**Mental model:** tmux, but the “windows” are agent CLIs bound to git worktrees, and the sidebar is a mission-control view of which agents are working, waiting, or dead.
