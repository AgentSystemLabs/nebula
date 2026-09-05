<div align="center">

# nebula

**Mission control for your coding agents.**

Run **Claude Code**, **Codex**, **Cursor** and **Pi** across every project and git WORKTREE you own — from one
terminal, one keyboard, one tree. They keep working when you close it.

[![Release](https://img.shields.io/github/v/release/AgentSystemLabs/nebula?style=flat-square&color=e8c547&label=release)](https://github.com/AgentSystemLabs/nebula/releases)
[![Build](https://img.shields.io/github/actions/workflow/status/AgentSystemLabs/nebula/release.yml?style=flat-square&label=build)](https://github.com/AgentSystemLabs/nebula/actions)
[![License](https://img.shields.io/badge/license-MIT-blue?style=flat-square)](LICENSE)
[![Platform](https://img.shields.io/badge/platform-macOS%20%7C%20Linux-lightgrey?style=flat-square)](#install)
[![Built with Rust](https://img.shields.io/badge/built%20with-Rust-dea584?style=flat-square)](https://www.rust-lang.org)

[**Keys**](docs/keys.md) · [**Commands**](docs/commands.md) · [**Sessions**](docs/sessions.md) · [**Configuration**](docs/configuration.md) · [**How it works**](docs/how-it-works.md)

```sh
curl -fsSL https://raw.githubusercontent.com/AgentSystemLabs/nebula/main/install.sh | sh
```

<img src="assets/screenshot.png" alt="nebula: projects, worktrees and sessions on the left, a live Claude Code session on the right" width="100%">

</div>

---

## Three agents, three tabs, and no idea which one needs you

You start three agents in three terminal tabs. Five minutes later you don't know which one is waiting on
a permission prompt, which one finished, and which one is still thinking — so you tab through all three,
every time, and read the screens. Start a fourth and you aren't running more agents, you're doing a worse
job of watching the ones you have.

nebula replaces the reading with a tree and a color. Every PROJECT, WORKTREE and SESSION is a row; every
SESSION carries a STATUS DOT that says what it's doing; and every parent ROLLS UP its children, so a red
dot on a collapsed PROJECT tells you exactly where to look without opening anything.

**No Electron, no browser, no server, no MCP.** One ~4 MB Rust binary and a unix socket.

## What you get

| | |
|---|---|
| **One tree, up to four PANELS** | PROJECTS → WORKTREES → SESSIONS → TERMINAL PANE. `h`/`j`/`k`/`l` moves, `Enter` drills in, and landing on a live pane hands it the keyboard — so `Tab` all the way right and start typing at the agent. |
| **A DAEMON that owns the PTYs** | Quit the TUI, shut the laptop, come back tomorrow. The agents never stopped, and the SCROLLBACK RING is replayed on ATTACH. |
| **STATUS DOTS you read instead of screens** | ● yellow mid-turn, ● violet finished and UNSEEN, ● green finished and read, ● red waiting on you — plus a violet `n done` DONE BADGE counting the terminals you still owe a look. |
| **Lists that order themselves** | PROJECTS, WORKTREES and SESSIONS all sit most-recent-first in RECENCY ORDER, with a dim `23m ago` after the name saying why the row is where it is. The one fixed seat is the ROOT WORKTREE, always the first WORKTREES PANEL row; nothing else is pinned or dragged into place by hand. |
| **Real git WORKTREES, one keystroke** | `n` in the WORKTREES PANEL branches off into an actual `git worktree`. Two agents in two directories never collide. |
| **Agents that drive nebula back** | Tell a Claude SESSION *"do this in a worktree"* and it runs `nebula worktree`, then restarts itself resumed inside the new checkout. Say *"show me the file"* and `nebula open` puts it in front of you in a tabbed modal. Say *"start a new nebula session that…"* and `nebula spawn` has a second agent working beside it before you look. |
| **Every open pull request, in place** | nebula asks `gh` what's still open on the repo. Rest on a PR ROW and the PR PREVIEW reads it to you — description, stats, the whole conversation. `g` for its diff, `Enter` for the browser, `n` for a SESSION on any harness, scoped to that PR. |
| **Diff, find, grep, browse** | `g` opens the DIFF VIEWER with REVIEWED MARKS, `f` the FILE FINDER, `F` a `git grep`, `b` the TREE BROWSER — all scoped to the selected WORKTREE, all one key from anywhere. |
| **`/` finds anything, anywhere** | The PALETTE spans every WORKSPACE, not just the open one. Before you type it sorts by attention: NEEDS FEEDBACK first, then RUNNING, then UNSEEN — so `/` `Enter` is the fastest way back to whatever needs you. |
| **It follows you to other machines** | `nebula ssh <host>` opens nebula there, installing it if missing. `nebula tunnel <host>` puts that machine's TUI in a browser tab over a single ssh tunnel. |

## Install

macOS or Linux — the same command installs and updates:

```sh
curl -fsSL https://raw.githubusercontent.com/AgentSystemLabs/nebula/main/install.sh | sh
```

It downloads the prebuilt binary for your platform from the latest GitHub release into `~/.local/bin`
(override with `NEBULA_INSTALL_DIR`), falling back to `cargo install --git` when no release matches.
Afterwards, `nebula upgrade` runs that same script for you; it refuses to clobber a local `cargo build`
(pass `--force` if you mean it). Upgrading with a DAEMON running is safe: an idle one — nothing live in
it — is shut down for you, so the next launch comes up on the new binary. A DAEMON with live SESSIONS is
left alone and they keep running the old binary until you `nebula kill` and relaunch. `nebula --version`
(`-V`) says which binary you are on.

> **Prerequisite:** at least one agent CLI on your `PATH` — `claude`, `codex`, `cursor-agent`, or `pi`.
> nebula spawns them; it doesn't ship them.
>
> Three commands each want one more binary, and only those commands: `nebula ssh` and `nebula tunnel`
> exit if there is no OpenSSH client (`ssh`), and `nebula browser` needs `ttyd` on your `PATH` — for
> `nebula tunnel` it is the *remote* host that needs it. The TUI itself needs neither.

## Quickstart

**1. Add a repo.** nebula is project-first, and a PROJECT is just a git checkout:

```sh
nebula add ~/code/my-app       # or, from inside the repo: nebula add .
```

**2. Open the TUI.** A bare `nebula` launches it and auto-starts the DAEMON:

```sh
nebula
```

`Tab` / `Shift+Tab` (or `h` / `l`) move FOCUS between PANELS, `j` / `k` move the selection inside one, and
`Enter` drills in. With no PROJECTS yet you get the SPLASH — press `n` to add one without leaving the TUI.

**3. Pick where the agent runs.** Every PROJECT starts with one WORKTREE: the checkout itself. Press `n`
in the WORKTREES PANEL to branch off into a real `git worktree`. That's the whole point of the column —
two agents in two WORKTREES edit two directories and never collide.

**4. Start the agent.** `n` in the SESSIONS PANEL opens the NEW SESSION PICKER — **Claude**, **Codex**,
**Cursor** or **Pi**, `→` for MODEL and EFFORT, `Enter` for your defaults. Or skip the picker entirely: `p` from any
PANEL opens the QUICK PROMPT, you type the task, and an agent starts working on it in the selected
WORKTREE. Save a framing you keep retyping as an AGENT PRESET (`e`) and it becomes one keystroke.

**5. Walk away.** `Ctrl+q` leaves the TERMINAL PANE for the panels; `q` asks first — a CONFIRM DIALOG
reading *Leave the TUI? Sessions keep running in the daemon.* that `Enter` accepts and a second `Ctrl+C`
walks straight through. The DAEMON still owns every PTY — come back with `nebula` an hour later and each
SESSION is exactly where you left it, scrollback replayed.

Leave a new SESSION on its default name and AUTO-TITLE renames it from your first prompt — `Fix Login
Redirect`, not `agent-3`. Type a name yourself and nebula never touches it. A Claude SESSION's own name
is the same name: `/rename` inside Claude Code retitles the row, and a name set in nebula reaches
Claude's prompt box and `/resume` picker on your next prompt.

## Read the dots, not the screens

| Dot | AGENT STATUS |
|---|---|
| ● gray | FRESH — agent never run |
| ● yellow | RUNNING — turn in progress (the STOP GATE holds it open while subagents are live) |
| ● violet | UNSEEN — turn complete and nobody has looked at it yet |
| ● green | FINISHED — the same finished turn, once the cursor has been on the SESSION |
| ● red | NEEDS FEEDBACK — permission prompt or question waiting on you |
| ● magenta | terminated — process died mid-run |
| ○ | disconnected — the DAEMON restarted while the agent was live |

A Cursor SESSION never goes red: nebula runs `cursor-agent --force` and Cursor reports no permission
event, so waiting-on-you is not detectable there.

WORKTREE and PROJECT rows ROLL UP their children: red beats yellow beats done, and a parent's dot is
violet whenever anything UNSEEN finished under it — so the violet walks up the tree and turns green as
you read your way down it.

A dot going violet while you were looking elsewhere is easy to miss, so nebula counts those for you. When
a turn finishes in a pane that isn't on screen, its WORKTREE and PROJECT rows grow a violet `n done`
DONE BADGE — the number of terminals you have left to go read — and the SESSION row says `done` where its
HARNESS BADGE normally sits. Walking the cursor onto a SESSION previews it, which reads it: the badges
count down as you go and disappear at zero. The flag lives in the DAEMON, so it survives closing the TUI
and is shared by every client; a turn that finishes in the pane you're already looking at never counts.

## Where the status actually comes from

nebula doesn't poll the agents and it doesn't guess from the screen. At spawn it merges MANAGED HOOKS
into the WORKTREE's `.claude/settings.local.json`, `.cursor/hooks.json` or `~/.codex/hooks.json` — tagged
`_nebulaManaged`, your own hooks preserved, rebuilt every spawn — and each one is a fail-soft `curl` to
the DAEMON's loopback HOOK RECEIVER, authenticated with a per-boot BEARER TOKEN. Pi has no shell hooks,
so it gets one managed extension at `~/.pi/agent/extensions/nebula.ts` that posts the same events. For the one event no CLI
reports — a turn you cancelled with `Esc` — the PROGRESS SCANNER reads the CLI's own OSC 9;4 progress
escapes straight off the PTY, a signal that survives the cancel and stays busy while a permission prompt
is open.

## Documentation

| | |
|---|---|
| [**Keys**](docs/keys.md) | Every default binding, the WORKTREE views (`g` `f` `F` `b`), and the mouse. All of it rebindable. |
| [**Commands**](docs/commands.md) | The `nebula` CLI: `add`, `rename`, `worktree`, `spawn`, `workspace`, `ssh`, `tunnel`, `browser`, `daemon`, `kill`, `upgrade`. |
| [**Sessions**](docs/sessions.md) | The NEW SESSION PICKER, MODEL / EFFORT, Claude Cloud and the CLOUD MIRROR, AGENT PRESETS, the PROJECT OPEN PRS group. |
| [**Configuration**](docs/configuration.md) | `config.json`, the SETTINGS OVERLAY, the HOTKEYS TAB, logs and environment overrides. |
| [**How it works**](docs/how-it-works.md) | The DAEMON, the hook dialects, AUTO-TITLE, WORKTREE RELOCATION, prewarm and reaping, persistence. |
| [**Architecture**](ARCHITECTURE.md) | Process model, the IPC CODEC and the crate layout. |

## Building

```sh
cargo build --release     # → target/release/nebula (~4 MB)
cargo test                # unit + end-to-end suite (spawns real daemons/PTYs)
```

`nebula-core` (shared protocol/entities), `nebula-daemon` (PTYs, SQLite, HOOK RECEIVER, STATUS MACHINE),
`nebula-tui` (ratatui client), `nebula` (the binary). `vendor/vt100` is a patched copy of the terminal
parser wired in through `[patch.crates-io]`: rows scrolled out of a top-anchored scroll region go to the
SCROLLBACK RING instead of being discarded, so wheel-up over a codex SESSION has something to show.

Releases: push a `v*` tag (`git tag v0.1.0 && git push --tags`) and CI builds mac (arm/intel) and linux (x64/arm64, static musl) binaries and
attaches them to a GitHub release — which is what `install.sh` downloads.

## License

MIT — see [LICENSE](LICENSE).

<div align="center">
<br>
<sub>If nebula saves you a tab, a ⭐ helps other people find it.</sub>
</div>
