# Orbit — the user guide

<sub>[← README](../README.md) · [Keys](keys.md) · [Commands](commands.md) · [Sessions](sessions.md) · [Configuration](configuration.md) · [How it works](how-it-works.md)</sub>

This is the practical guide: what Orbit is, how to run it, and how to do each everyday job. The other
docs go deeper on each topic; start here.

**Contents**

1. [What Orbit is](#1-what-orbit-is)
2. [Install and run](#2-install-and-run)
3. [Your first session](#3-your-first-session)
4. [Reading the screen](#4-reading-the-screen)
5. [The everyday loop](#5-the-everyday-loop)
6. [Worktrees: parallel work without collisions](#6-worktrees-parallel-work-without-collisions)
7. [Reviewing what an agent did](#7-reviewing-what-an-agent-did)
   - [The code graph (graphify)](#the-code-graph-graphify)
8. [GitHub: pull requests and issues](#8-github-pull-requests-and-issues)
9. [Presets: tasks you repeat](#9-presets-tasks-you-repeat)
10. [Agents that drive Orbit](#10-agents-that-drive-orbit)
11. [Running and opening your project](#11-running-and-opening-your-project)
12. [Settings worth changing](#12-settings-worth-changing)
13. [Key reference](#13-key-reference)
14. [Command-line reference](#14-command-line-reference)
15. [Troubleshooting](#15-troubleshooting)
16. [What's coming](#16-whats-coming)

---

## 1. What Orbit is

Orbit runs your Claude Code sessions — several at once, across projects and branches — and shows them
all on one screen, with a coloured dot telling you which one needs you.

It is two programs in one binary:

- **The daemon** runs in the background and owns every session. Closing the screen, or your laptop
  lid, does not stop an agent.
- **The TUI** is the screen you look at. Quit it with `q` and reopen it with `orbit` whenever you
  like; every session is where you left it, its output replayed.

Everything is nested three levels deep:

```
Project      a git repository                     →  a tab across the top
└ Worktree   a checkout of it (main, or a branch)  →  a band (a row) on the grid
  └ Session  one Claude Code conversation          →  a card in that band
```

A project can have several worktrees, so two agents can work on two branches of the same repo at the
same time without editing the same files.

## 2. Install and run

**What you need:** `git` and the `claude` CLI, plus **`uv`** and **`npm`**. Orbit fetches the tools it
integrates through those two on first use — graphify through `uvx`, agent-browser through `npx` — so
there is nothing else to install. Run **`orbit doctor`** to see what your machine has and what each
missing piece would unlock; `orbit doctor --fix` fetches those tools right away instead of on first
use. (`gh` is the one optional extra worth installing yourself, for pull requests and issues.)

To build Orbit from this repository you also need Rust (`rustup`, on your PATH).

**Try your latest code without touching anything else** — an isolated copy with its own daemon and
data:

```sh
make dev                  # build and open it
make dev AGENT=/bin/cat   # same, but a stand-in instead of Claude: try the UI for free
make dev-reset            # wipe the dev copy's data and start fresh
```

Quitting `make dev` also stops its daemon and its sessions.

**Install it for real use:**

```sh
make install PREFIX=$HOME/.local/bin   # build a release and put `orbit` in ~/.local/bin
orbit                                  # open the TUI (the daemon starts by itself)
```

(`make install` alone targets `~/.cargo/bin`, which a Homebrew rustup doesn't create.)

After installing a new build while sessions are running, `orbit reload` moves the daemon onto the new
binary without stopping any session.

## 3. Your first session

1. Open a terminal **inside the repository** you want to work on and run `orbit` (or `make dev` from
   this repo).
2. The splash screen offers that repository: press **`Enter`** to open it as your first project.
   Anywhere else, **`o`** browses for a folder.
3. Press **`p`** — the task box opens. Type what you want done and press **`Enter`**.
   (`Shift+Enter`, `Option+Enter` or `Ctrl+J` start a new line.)
4. A card appears with a **yellow** dot: Claude is working. When it turns **blue**, the turn is done.
5. Press **`Enter`** on the card to step into the session and type to Claude directly.
   **`Ctrl+Q`** brings you back to the cards.

You never name sessions: after the first prompt each one renames itself (`Fix Login Redirect`, not
`agent-3`). **`r`** renames one by hand.

## 4. Reading the screen

```
 ┌ project tabs ─────────────────────────────────────────────┐
 │  my-app ●1 ●2   api ●1                                     │
 ├ band: one checkout (branch, changes, pull request) ───────┤
 │  ⌂ main ───── +3 files +60 −28 ── ↗ #57 ready ──           │
 │  ┃● Fix login     ┃  ┃● Add tests  ┃   ← cards (sessions)  │
 │  ↳ feat-search ──────────────────────                      │
 │  ┃● Search API    ┃                                        │
 ├───────────────────────────────────────────────────────────┤
 │  ↑↓←→ move  ⏎ open  p new task  ␣ follow-up  j jump …     │  ← footer: keys for here
 └───────────────────────────────────────────────────────────┘
```

The pane (to the right or below the cards) shows the live terminal of the card under the cursor.

**The dots** — you read these instead of reading screens:

| Dot | Means |
|---|---|
| ● yellow | working on a turn |
| ● red | **waiting on you** — a permission prompt or a question |
| ● blue | finished, and you haven't looked yet |
| ● green | finished, and you've seen it |
| ● gray | never run yet |
| ● magenta | the process died mid-run |

Project tabs add their sessions up: `api ●1` in red means one session in that project is waiting on
you, even while you look at another project. A red dot also plays a sound, and sends a desktop
notification when the terminal window is in the background.

## 5. The everyday loop

| I want to… | Press |
|---|---|
| Start a new task | **`p`**, type, **`Enter`** |
| Give a running session its next instruction, without opening it | **`Space`** on its card, type, **`Enter`** |
| Talk to a session directly | **`Enter`** on its card · **`Ctrl+Q`** to come back |
| Go to the next session that needs me | **`;`** (and **`,`** for the previous one) |
| Find any session by name, in any project | **`j`**, type part of its name, **`Enter`** |
| Move around the cards | **arrow keys** · **`Tab`** opens/folds a band |
| Switch project | **`<`** / **`>`** · **`+`** lists them all · `1`–`9` jump to a tab |
| See a session full-screen | **`Ctrl+F`** |
| Hide or show the pane | **`z`** |
| Archive a finished session | **`a`** (`⇧A` shows archived ones, `u` restores) |
| Delete a session | **`d`** |
| Open a plain shell in this checkout | **`t`** |
| See every key | **`h`** |
| Quit (sessions keep running) | **`q`** |

The loop that makes Orbit worth it: start a few tasks with `p`, go do something else, and when you
hear a sound or see a red/blue dot, press `;` to land on exactly the session that needs you.

**Inside the task box (`p`)** the row above the text shows where the task will run. Each part has a
key, so you can change it without losing what you typed:

| In the box | Changes |
|---|---|
| `Ctrl+P` | the project |
| `Ctrl+T` | the worktree (checkout) |
| `Ctrl+N` | run it in a **new worktree** (the frame turns green) |
| `Tab` | the harness (Claude, …) |
| `Ctrl+O` | the model and effort |
| `Shift+Tab` | apply a preset |
| `Esc` | close — what you typed is kept for next time |

## 6. Worktrees: parallel work without collisions

Two agents editing the same checkout trip over each other. A **worktree** is a separate checkout of
the same repo on its own branch, so each agent gets its own folder.

- **New task in a new worktree:** in the task box press **`Ctrl+N`** before `Enter`. Orbit cuts a
  branch, creates the checkout next to your repo (`<repo>-worktrees/<branch>`), and starts Claude
  there. It shows up as a new band.
- **Move a running session into a worktree:** just tell Claude *"do this in a worktree"*. It runs
  `orbit worktree`, and the session restarts inside the new checkout, conversation intact.
- **Change the branch of the main checkout:** **`c`** on a card in the root band lists branches; type
  to filter, `Enter` to switch (it asks what to do with uncommitted changes).
- **Delete a worktree:** delete its last session with `d`. The confirmation then asks whether to
  remove the now-empty checkout from disk too: `y` yes, `n` keep it, `Esc` cancel.

Each band's header shows its branch, its uncommitted changes (`+3 files +60 −28`) and its pull
request if it has one.

## 7. Reviewing what an agent did

All of these open for the checkout of the card under the cursor:

| Key | Opens |
|---|---|
| **`g`** | the **diff** of everything changed, file by file; mark files as reviewed as you go |
| **`f`** | **find a file** by name (fuzzy); `Enter` opens it, `Ctrl+Y` copies its path |
| **`⇧F`** | **search inside files** (`git grep`) |
| **`b`** | **browse** the file tree |
| **`⇧O`** | **open the checkout** in your editor or browser (see [section 11](#11-running-and-opening-your-project)) |

Markdown files open as rendered pages, images preview inline, and Mermaid diagrams render when `mmdc`
is installed.

### The code graph (graphify)

Orbit keeps a **knowledge graph of your code** for every checkout using
[graphify](https://github.com/Graphify-Labs/graphify), with nothing to set up — it only needs `uv`
(it runs graphify through `uvx`, which fetches it on first use; an installed `graphify` is used
instead when there is one):

- It is built when you add a project or create a worktree, and refreshed after every Claude turn.
  It uses code analysis only — no LLM, no cost — and takes a few seconds (incremental after that).
- It lives in `graphify-out/` inside the checkout and is kept out of git automatically (through
  `.git/info/exclude`, so no tracked file changes).
- Claude sessions are told the graph exists and query it (`graphify query`, `explain`, `path`,
  `affected`) before searching file by file — those commands are pre-approved, so no prompts.
- **`k`** opens the graph in your browser (and builds it first if there isn't one yet).

To turn it off: `"graphify": false` in `config.json` for every project, or in a project's
`.orbit.json` for just that one.

## 8. GitHub: pull requests and issues

Orbit asks the `gh` CLI what's open on the repo, so `gh` must be installed and logged in.

- **`v`** — every open pull request. Read the description, checks and conversation; `Ctrl+G` shows its
  diff, `Ctrl+C` comments, **`Enter`** starts a Claude session on it in the PR's own checkout.
- **`i`** — every open issue. **`Enter`** opens the task box for it; the issue's link travels with the
  session so Claude knows what it is fixing. `Ctrl+E` edits the issue.
- **`⇧V`** opens a card's pull request in the browser, **`⇧I`** its issue, **`⇧G`** the repository.

A band whose branch has a pull request shows it on its header — red when it no longer merges or a
check failed, purple once merged.

## 9. Presets: tasks you repeat

If you keep typing the same framing ("review this for security…", "write tests for…"), save it as a
**preset**:

1. **`e`** on any card lists your presets.
2. **`Ctrl+A`** creates one: a name, the model and effort, and text that goes **before** and/or
   **after** whatever task you type.
3. **`Enter`** on a preset asks for the task and launches it. Set **Task** to `skip` for presets that
   need no task at all (a "commit and push"), so `Enter` launches immediately.

In the task box, **`Shift+Tab`** applies a preset to what you've already typed.

## 10. Agents that drive Orbit

Claude sessions started by Orbit know a few `orbit` commands, so you can ask in plain words:

| Say to Claude | It runs |
|---|---|
| "do this in a worktree" | `orbit worktree <name>` — the session moves into a new checkout |
| "start a new session that writes the tests" | `orbit spawn "<task>"` — a second agent starts beside it |
| "show me that file" / "show the diagram" | `orbit open <file>` — it opens in a viewer inside Orbit |
| "what did the research session find?" | `orbit read research` |
| "ask the api session which library it chose" | `orbit ask api "<question>"` |
| "list the other sessions" | `orbit sessions` |

**F10** switches to the **orchestrator** view: one pinned Claude chat that can list, read, message and
start sessions across all your projects for you. See [Orchestrator](orchestrator.md).

## 11. Running and opening your project

Commit a `.orbit.json` at the root of a repository to tell Orbit how to run it:

```json
{
  "run": "npm run dev",
  "open": "open http://localhost:3000"
}
```

- **Run** — right-click a card (or the project tab) → **Run**. The command runs in a terminal card in
  that worktree; **Stop run** stops it.
- **Open** — **`⇧O`** on a worktree runs the `open` command once (a browser tab, your editor, …).

Nothing in the file runs until you press its key. To keep the commands out of the repo, set them in
Settings (`s`) → **Project** instead.

## 12. Settings worth changing

Press **`s`**. Each tab is a list; `Enter` or `←`/`→` change a row. Suggested for a Claude-only setup:

| Where | Setting | Why |
|---|---|---|
| Agents | turn **Codex, Cursor, Pi, Muse, Grok, OpenCode** off | the pickers then only offer Claude |
| Agents | **Claude → Model / Effort** | the default every new task uses |
| Sessions | **Warm spare agent** off | otherwise an idle Claude waits in the background (150–300 MB) to make launches instant |
| Sessions | **Prewarm dead sessions** off | otherwise sleeping sessions are woken as soon as your cursor rests on their checkout |
| Sessions | **Idle session timeout** | how long an unwatched session lives before it is put to sleep (it resumes when you return) |
| Sessions | **Done sound / Feedback sound** | the sounds for "finished" and "needs you" |
| Appearance | **Worktree layout: list** | one line per session — calmer with many sessions |
| Appearance | **Theme**, **Session pane: right/bottom** | taste |
| Hotkeys | any action | rebind any key; it warns before taking one already in use |

Settings live in `~/Library/Application Support/dev.orbit.orbit/config.json`. `orbit config export`
backs them up; `orbit config import <file>` restores them.

## 13. Key reference

Keys on the grid (where the cards are). Inside a box or a list, the footer shows that screen's keys.
Every key is rebindable in Settings → Hotkeys.

**Move**

| Key | Action |
|---|---|
| `↑` `↓` `←` `→` | move between bands and cards |
| `Tab` | open / fold the band under the cursor |
| `Enter` | step into the session (type to Claude) |
| `Ctrl+Q` | step back out to the cards |
| `j` (or `/`) | jump to any session, any project |
| `;` / `,` | next / previous session that needs you |
| `<` / `>` | previous / next project tab |
| `+` | list every project |
| `1` … `9` | project tab by position |
| `x` | close the project tab (sessions keep running) |
| `Ctrl+D` / `Ctrl+U` | half a page down / up |

**Sessions**

| Key | Action |
|---|---|
| `p` | new task (the task box) |
| `n` | new session without a task |
| `Space` | follow-up: send the next instruction to the card |
| `e` | presets |
| `r` | rename |
| `a` / `u` / `⇧A` | archive / unarchive / show archived |
| `d` / `⇧D` | delete / delete all |
| `m` | move the session to another worktree |
| `⇧P` | duplicate the session |
| `t` | new shell terminal |
| `w` | cycle the pane through this checkout's terminals |
| `z` | hide / show the pane |
| `Ctrl+F` | full-screen session |

**Code and GitHub**

| Key | Action |
|---|---|
| `g` | diff |
| `f` / `⇧F` | find a file / search in files |
| `b` | file tree |
| `c` | switch the main checkout's branch |
| `⇧O` | open the checkout (editor / browser) |
| `k` | the code graph (graphify) in the browser |
| `v` / `i` | pull requests / issues |
| `⇧V` / `⇧I` / `⇧G` | open PR / issue / repo in the browser |
| `y` | comment on the pull request |
| `⇧R` | reload from GitHub |

**General**

| Key | Action |
|---|---|
| `o` | open a folder as a new project |
| `s` | settings |
| `h` (or `?`) | help: every key |
| `F10` | orchestrator view |
| `⇧H` | SSH hosts |
| `⇧M` | memory usage |
| `q` | quit (sessions keep running) |

## 14. Command-line reference

```sh
orbit                     # open the TUI
orbit add <dir>           # add a repository as a project (orbit add . for the current one)
orbit sessions            # list every session
orbit read <session>      # print a session's recent output
orbit ask <session> "…"   # ask a session something and print the answer
orbit kill                # stop the daemon and every session
orbit reload              # move the daemon onto a new build, keeping sessions
orbit doctor              # what's installed, what's missing, what each tool unlocks
orbit doctor --fix        # fetch graphify and agent-browser now (through uv / npm)
orbit config path         # where the settings files are
orbit config export       # back up settings
orbit ssh <host>          # open Orbit on another machine
```

The full list, with every flag: [Commands](commands.md), or `orbit <command> --help`.

## 15. Troubleshooting

| Problem | Fix |
|---|---|
| A card's dot stays gray while Claude works | the status hooks didn't install. Check `.claude/settings.local.json` in that checkout has entries tagged `_orbitManaged`, and that `curl` is on your PATH |
| "daemon did not come up" | run `orbit kill`, then `orbit` again. Logs: `~/Library/Application Support/dev.orbit.orbit/state/daemon.log` |
| A change I built isn't there | the old daemon is still running: `orbit reload` (or `orbit kill` then `orbit`) |
| `path must be shorter than SUN_LEN` | the socket folder path is too long for macOS; set `ORBIT_RUNTIME_DIR` to a short path such as `/tmp/orbit` |
| `k` says graphify couldn't start, or no graph appears | an old copy of `graphify` may come first on your PATH (`which -a graphify`); Orbit skips copies whose Python is gone, but remove stale ones or run `uv tool upgrade graphifyy` |
| `make ci` fails with `fatal: no tag message?` | your global git config signs tags (`tag.gpgsign`). Run tests as `GIT_CONFIG_GLOBAL=/dev/null make ci` |
| `cargo: command not found` | rustup isn't on your PATH: `export PATH="/opt/homebrew/opt/rustup/bin:$PATH"` in `~/.zshrc` |

For more detail, set `ORBIT_LOG=debug` before starting.

## 16. What's coming

These are planned for this fork and **not built yet**:

- **Skill picker** — `Ctrl+S` in the task box lists your Claude skills (personal, project and
  plugins); tick several, and Claude is told to use exactly those. Saved **workflows** per project
  (e.g. "feature" = spec + incremental + tests) tick a whole set in one go.
- **Command palette** — `:` and type what you want ("diff", "rename"); see the action and its key,
  `Enter` runs it. No more remembering keys.
- **Calmer view with many sessions** — a filter for "needs me / active only", and a compact layout that
  switches on by itself past a number of sessions.
- **Lessons learned** — `orbit lesson add` lets an agent record a hard-won fix in `.orbit/lessons/`
  (committed), and new sessions start with the relevant ones.
- **MCP manager** — choose which MCP servers each project's sessions load.
