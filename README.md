<div align="center">

# nebula

**Mission control for your coding agents.**

Run **Claude Code**, **Codex**, **Cursor**, **Pi** and **Muse** across every project and git WORKTREE you own — from one
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
| **STATUS DOTS you read instead of screens** | ● yellow mid-turn, ● blue finished and UNSEEN, ● green finished and read, ● red waiting on you — plus a blue `n done` DONE BADGE counting the terminals you still owe a look. |
| **Lists that order themselves** | PROJECTS, WORKTREES and SESSIONS all sit most-recent-first in RECENCY ORDER, with a dim `23m ago` after the name saying why the row is where it is. The one fixed seat is the ROOT WORKTREE, always the first WORKTREES PANEL row; nothing else is pinned or dragged into place by hand. |
| **Real git WORKTREES, one keystroke** | `n` in the WORKTREES PANEL branches off into an actual `git worktree`. Two agents in two directories never collide. A WORKTREE HOOK in git config lets a project claim a port or a route when a checkout is created and release it when it is deleted. |
| **The root checkout on any branch, no shell** | `c` on the ROOT WORKTREE lists every branch and remote branch, fuzzy-filtered as you type; `Enter` switches, or creates the branch when nothing matches. Uncommitted changes? The BRANCH SWITCHER asks first — stash them, bring them along, commit them, or discard them — the way an IDE would. |
| **Agents that drive nebula back** | Tell a Claude SESSION *"do this in a worktree"* and it runs `nebula worktree`, then restarts itself resumed inside the new checkout. Say *"show me the file"* and `nebula open` puts it in front of you in a tabbed modal. Say *"start a new nebula session that…"* and `nebula spawn` has a second agent working beside it before you look. |
| **Every open pull request, in place** | nebula asks `gh` what's still open on the repo. Rest on a PR ROW and the PR PREVIEW reads it to you — description, stats, the whole conversation. A row goes red, badged `conflicts` or `failing`, when GitHub says the branch no longer merges or a check failed. `g` for its diff, `y` to comment on it without leaving the keyboard, `Enter` for the browser, `n` for a SESSION on any harness, scoped to that PR. |
| **Every open issue, one key from an agent** | `i` lists the project's open GitHub issues, newest first, and reads the one under the cursor — description, labels, comments. `Enter` opens a QUICK PROMPT for it, `e` launches one of your AGENT PRESETS on it, `E` edits its title and description in place; the issue's URL travels with the session as context on every spawn, so the harness knows what it is fixing. |
| **Every open pull request, one key from a review** | `v` lists the project's open pull requests and reads the one under the cursor — description, checks, conversation — from the list nebula already keeps warm. `Enter` opens a QUICK PROMPT for a PR SESSION on it, `e` launches one of your AGENT PRESETS on it, `n` picks a harness; the session runs in a checkout of the PR's branch with its URL as context. |
| **Diff, find, grep, browse** | `g` opens the DIFF VIEWER with REVIEWED MARKS, `f` the FILE FINDER, `F` a `git grep`, `b` the TREE BROWSER — all scoped to the selected WORKTREE, all one key from anywhere. Markdown previews are rendered pages, not raw `#` and `*`. |
| **`/` finds anything, anywhere** | The PALETTE spans every WORKSPACE, not just the open one, each project a header with its sessions listed under it by title. Before you type it is that overview, sorted by attention: NEEDS FEEDBACK first, then RUNNING, then UNSEEN — so `/` `Enter` is the fastest way back to whatever needs you, and `]` / `[` cycle that same attention order with no modal at all, one session per press, workspaces included. Open pull requests are rows too: `Enter` on one lands on its PR ROW with the PR PREVIEW reading it, `Ctrl+o` hands it to the browser. |
| **It follows you to other machines** | `nebula ssh <host>` opens nebula there, installing it if missing. `nebula tunnel <host>` puts that machine's TUI in a browser tab over a single ssh tunnel. Your settings and agent presets go along, and `nebula config export` / `import` back them up. |

## Supported harnesses

Five CLIs work out of the box, each with its own Agents tab section and model/effort rows. A
sixth, Grok, needs one config block (see "Teach nebula a new agent CLI" below). Install the CLI,
pick it in the `n` picker, done. A CLI missing from PATH still shows in the picker; the DAEMON
re-checks through the login shell at launch.

| | Harness | CLI | Install |
|---|---|---|---|
| <img src="https://www.google.com/s2/favicons?domain=claude.com&sz=128" width="24" height="24" alt="Claude"> | [Claude](https://code.claude.com/docs/en/setup) | `claude` | `curl -fsSL https://claude.ai/install.sh \| bash` |
| <img src="https://www.google.com/s2/favicons?domain=openai.com&sz=128" width="24" height="24" alt="Codex"> | [Codex](https://github.com/openai/codex) | `codex` | `npm i -g @openai/codex` |
| <img src="https://www.google.com/s2/favicons?domain=cursor.com&sz=128" width="24" height="24" alt="Cursor"> | [Cursor](https://cursor.com/install) | `cursor-agent` | `curl -fsSL https://cursor.com/install \| bash` |
| <img src="https://www.google.com/s2/favicons?domain=pi.dev&sz=128" width="24" height="24" alt="Pi"> | [Pi](https://pi.dev) | `pi` | `curl -fsSL https://pi.dev/install.sh \| sh` |
| <img src="https://www.google.com/s2/favicons?domain=meta.com&sz=128" width="24" height="24" alt="Muse"> | [Muse](https://developer.meta.com/ai/lp/muse-code) | `muse` | `curl -fsSL https://dev.meta.ai/install.sh \| bash` |
| <img src="https://www.google.com/s2/favicons?domain=x.ai&sz=128" width="24" height="24" alt="Grok"> | [Grok](https://github.com/xai-org/grok-build) | `grok` | `curl -fsSL https://x.ai/cli/install.sh \| bash` |

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
left alone and they keep running the old binary until you `nebula kill` and relaunch — unless the new
build speaks a different protocol, in which case it can't attach until that restart, and `nebula upgrade`
says so and offers to do it for you. `nebula --version`
(`-V`) says which binary you are on.

> **Prerequisite:** at least one agent CLI on your `PATH` — `claude`, `codex`, `cursor-agent`, `pi`, or `muse`.
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
**Cursor**, **Pi** or **Muse**, `→` for MODEL and EFFORT, `Enter` for your defaults — and the session starts, its
pane yours to type the first prompt into. Or skip the picker entirely: `p` from any
PANEL opens the QUICK PROMPT, you type the task, and an agent starts working on it in the selected
WORKTREE — or, from the WORKTREES PANEL or with `Ctrl+N` inside the box, in a fresh worktree cut for the
job, the box turning green to say so. Save a framing you keep retyping as an AGENT PRESET (`e`) and it
becomes one keystroke.

**5. Walk away.** `Ctrl+q` leaves the TERMINAL PANE for the panels; `q` asks first — a CONFIRM DIALOG
reading *Leave the TUI? Sessions keep running in the daemon.* that `Enter` accepts and a second `Ctrl+C`
walks straight through. The DAEMON still owns every PTY — come back with `nebula` an hour later and each
SESSION is exactly where you left it, scrollback replayed.

A new SESSION starts on a default name and AUTO-TITLE renames it from your first prompt — `Fix Login
Redirect`, not `agent-3`; `r` renames it whenever you like. A Claude SESSION's own name
is the same name: `/rename` inside Claude Code retitles the row, and a name set in nebula reaches
Claude's prompt box and `/resume` picker on your next prompt.

## Read the dots, not the screens

| Dot | AGENT STATUS |
|---|---|
| ● gray | FRESH — agent never run |
| ● yellow | RUNNING — turn in progress (the STOP GATE holds it open while subagents are live) |
| ● blue | UNSEEN — turn complete and nobody has looked at it yet |
| ● green | FINISHED — the same finished turn, once the cursor has been on the SESSION |
| ● red | NEEDS FEEDBACK — permission prompt or question waiting on you |
| ● magenta | terminated — process died mid-run |
| ○ | disconnected — the DAEMON restarted while the agent was live |

A Cursor SESSION never goes red: nebula runs `cursor-agent --force` and Cursor reports no permission
event, so waiting-on-you is not detectable there. A Muse SESSION never goes red either yet: `muse`
has no managed hooks, so its status is process-based until a hook dialect is mapped.

WORKTREE and PROJECT rows ROLL UP their children: red beats yellow beats done, and a parent's dot is
blue whenever anything UNSEEN finished under it — so the blue walks up the tree and turns green as
you read your way down it.

A dot going blue while you were looking elsewhere is easy to miss, so nebula marks the moment and then
keeps count. The moment: a row's name sweeps — a bright band crossing it — in yellow while it runs and in
red while it waits on you, for as long as either lasts; a turn that finishes unread sweeps blue for about
five seconds, on its own row and every row that rolls it up, and then holds still. Motion means live, or
just changed; a row at rest is at rest (`animations` off stills all of it). The count: when
a turn finishes in a pane that isn't on screen, its WORKTREE and PROJECT rows grow a blue `n done`
DONE BADGE — the number of terminals you have left to go read — and the SESSION row says `done` where its
HARNESS BADGE normally sits. Walking the cursor onto a SESSION previews it, which reads it: the badges
count down as you go and disappear at zero — `]` walks you onto the next one owed a look without hunting
for it. The flag lives in the DAEMON, so it survives closing the TUI
and is shared by every client; a turn that finishes in the pane you're already looking at never counts.

A dot going red is the one you can't afford to miss — a blocked agent burns the clock while you're in
another window — so that one reaches you: the FEEDBACK SOUND rings (`Sosumi` by default, distinct from
the `Glass` DONE SOUND a finish gets), and when the terminal window is in the background a desktop
notification names the session and its worktree. Neither fires for the pane you're locked into typing
at with the window focused — that prompt is already under your hands. One setting, `feedback_sound`,
owns both; `off` silences the pair. See [Configuration](docs/configuration.md).

## Where the status actually comes from

nebula doesn't poll the agents and it doesn't guess from the screen. At spawn it merges MANAGED HOOKS
into the WORKTREE's `.claude/settings.local.json`, `.cursor/hooks.json` or `~/.codex/hooks.json` — tagged
`_nebulaManaged`, your own hooks preserved, rebuilt every spawn — and each one is a fail-soft `curl` to
the DAEMON's loopback HOOK RECEIVER, authenticated with a per-boot BEARER TOKEN. Pi has no shell hooks,
so it gets one managed extension at `~/.pi/agent/extensions/nebula.ts` that posts the same events. For the one event no CLI
reports — a turn you cancelled with `Esc` — the PROGRESS SCANNER reads the CLI's own OSC 9;4 progress
escapes straight off the PTY, a signal that survives the cancel and stays busy while a permission prompt
is open.

## Teach nebula a new agent CLI

The five built-ins are just rows in a table, and the table is open. One block in `config.json` adds
a CLI everywhere at once: the `n` picker, the `e` presets, spawn, resume, and the Agents tab, which
grows it a section to tune without hand-editing.

```json
{
  "harnesses": {
    "mycli": {
      "program": "mycli",
      "model_flag": "--model",
      "model_default": "large",
      "hooks": "claude"
    }
  }
}
```

`program` is the only required row: the binary nebula launches, resolved on PATH through your login
shell. A new id starts enabled, takes the id as its label, boots fresh every launch (no resume), and
hides the Effort row until you map effort. `nebula config harnesses` prints the effective rows to copy
from, and a block that stops making sense refuses its launches with the reason while everything else
keeps working. Ids use lowercase letters, digits and hyphens, and must not collide with a built-in.

`hooks` names a built-in dialect, not your own scripts: `claude`, `codex`, `cursor` or `pi`. At spawn
nebula installs that dialect's MANAGED HOOKS for the session (the same `.claude/settings.local.json`,
`.cursor/hooks.json`, `~/.codex/hooks.json` or pi extension the built-in gets), so a CLI that speaks
that protocol reports status, prompts and permission waits exactly like the real thing. A
Claude-compatible CLI with `"hooks": "claude"` even gets title sync and auto-title. Leave `hooks` out
and the sessions stay process-based: running while the PTY is live, never red. Either way your own
hooks are preserved (nebula's entries are tagged `_nebulaManaged`) and the merge is rebuilt every
spawn.

One boundary to know: `nebula ssh` syncs `config.json` to the remote, but exec-capable harness keys
never travel with it: each machine runs only the programs its own files name. Full row reference
(resume styles, effort mapping, system-prompt passing, clearing a row with `null`): [Configuration](docs/configuration.md),
"The harness registry".

## Documentation

| | |
|---|---|
| [**Keys**](docs/keys.md) | Every default binding, the WORKTREE views (`g` `f` `F` `b`), and the mouse. All of it rebindable. |
| [**Commands**](docs/commands.md) | The `nebula` CLI: `add`, `rename`, `worktree`, `spawn`, `workspace`, `config`, `ssh`, `tunnel`, `browser`, `daemon`, `kill`, `upgrade`. |
| [**Sessions**](docs/sessions.md) | The NEW SESSION PICKER, MODEL / EFFORT, Claude Cloud and the CLOUD SESSION PANEL, AGENT PRESETS, the PROJECT OPEN PRS group, the ISSUES MODAL, the PULL REQUESTS MODAL. |
| [**Configuration**](docs/configuration.md) | `config.json` and `config.local.json`, backup and restore, the SETTINGS OVERLAY, the HOTKEYS TAB, the `.nebula.json` PROJECT FILE (`r` runs a worktree, `Shift+Enter` opens it), compatibility rules, logs and environment overrides. |
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

Pull requests: a branch pushed to this repository gets an automated Claude code review on its PR, as
inline comments. A PR from a fork does not — GitHub withholds the credentials the reviewer needs from a
fork's workflow runs — so a maintainer reviews it by hand, or asks for the review with `@claude` in a
PR comment.

## License

MIT — see [LICENSE](LICENSE).

<div align="center">
<br>
<sub>If nebula saves you a tab, a ⭐ helps other people find it.</sub>
</div>
