# Keys

<sub>[← README](../README.md) · [Keys](keys.md) · [Commands](commands.md) · [Sessions](sessions.md) · [Configuration](configuration.md) · [How it works](how-it-works.md)</sub>

Every binding below is a default, and every one of them is rebindable in the SETTINGS OVERLAY's
HOTKEYS TAB (`s`) — see [Configuration](configuration.md).

## Worktree views

The panels aren't the only view. With a worktree selected, from any panel:

| Key | View |
|---|---|
| **`g`** | **Git diff.** Changed files down the left, the diff on the right, with a live fuzzy filter. On an open-PR row it shows that pull request's diff instead, fetched whole with `gh pr diff`. `Ctrl+r` marks a file reviewed ✓ and sinks it to the bottom — nebula-side bookkeeping only, no git state is touched — and every mark clears itself when HEAD moves or the file changes again, so what's left unticked is genuinely what you haven't read. |
| **`f`** | **Find file.** Fuzzy finder over the worktree. `Enter` opens the file in an editor modal (vim by default; the `editor` setting or `NEBULA_EDITOR` picks another), `Ctrl+y` copies the path — ready to paste into an agent. |
| **`F`** | **Find in files.** `git grep` into the same modal; `Enter` opens the hit at its line. |
| **`b`** | **File tree browser.** Tree on the left, syntax-highlighted preview on the right, and an always-live filter that narrows the tree to matching files and the directories holding them. |

## Full keymap

| Context | Key | Action |
|---|---|---|
| Panels | `Tab`/`Shift+Tab`, `h/l` or `←/→`, `j/k` | move FOCUS / selection through visible panels; the walk stops at both ends (`Tab` at the TERMINAL PANE, `Shift+Tab` at the first visible panel) instead of cycling, and landing on a live pane takes its input; `h`/`l` stop one short of each end, and a double tap there (`l`,`l` at Sessions, `h`,`h` at the first visible panel) jumps the boundary; so do `k`,`k` on a panel's first row (up into the workspaces bar) and `j`,`j` in the bar (back down to the panel you came from) |
| Panels | `Ctrl+→` | cross into the terminal pane *without* taking its input (`Tab`, or a double tap of `l`/`→` at Sessions, takes it) |
| Panels | `Enter` | drill in; on a session: attach |
| Any panel | `/` | fuzzy jump across every workspace, project, worktree and session — in *every* workspace, each row pathed `workspace/project/branch/session`, so typing another workspace's name jumps you into it (`Ctrl+n/p` move, `Ctrl+o` opens the hit, `Ctrl+f` just lands the selection on it). Until you type, the rows sit in attention order — sessions waiting on you, then running ones, then unread finishes, then everything else by last interaction — so `/` `Enter` is the fastest way back to what needs you or to the worktree you just left |
| Projects | `n` / `d` | add project / remove from list |
| Any panel | `o` | add ("open") a project — same prompt as `n`, from any focus |
| Add project | type + `Tab`, `↓↑` / `→` / `←` | browse for the repo: type to filter (bash-style Tab completion), arrows pick a directory, `→` steps in, `←` steps up, `Enter` adds the highlighted (or typed) path; `●` marks git repos |
| Projects | `r` | rename the row — a label, not a move: the folder on disk keeps its name and hangs off a `└` under the new one. A terminal cell has one font size (Kitty's OSC 66 renders half-size text, but WezTerm and Ghostty don't implement it), so the hierarchy is weight, opacity and position instead: the name you chose is bold, the folder is the dimmest theme color plus the faint attribute. An empty name puts the row back on the folder's name |
| Worktrees (checkout row) | `n` / `d` | new worktree / delete (typed confirm — deletes files) |
| Worktrees (PROJECT OPEN PRS row) | `n`; `m` / right-click | new Claude SESSION scoped to that PR; the context menu also opens the PR or its diff |
| New worktree | type a sentence, or `Enter` on the empty prompt | the branch name is slugified (`fix login redirect` → `fix-login-redirect`); empty takes a random `<adj>-<noun>-<verb>` |
| Sessions | `n` | new session (agent or shell terminal) |
| New session picker (Claude) | `Tab` | toggle Claude Cloud; Cloud adds a wrapped task prompt (`Shift+Enter` or `Ctrl+J` inserts a line) before launch |
| Sessions (cloud row) | `m` | **Attach cloud session** re-pulls the transcript now; **Send to cloud session** queues a message on it |
| Sessions | `r`, `a`, `u`, `d`, `A` | rename, archive, unarchive, delete, toggle archived |
| Sessions | `e` | agent presets: saved launch definitions (harness, model, effort, optional prefix/postfix text). `Enter` asks for a task and starts the agent with prefix + task + postfix as its first prompt; `a` / `e` / `d` create, edit, delete |
| Any panel | `p` | quick prompt: a wrapped, multi-row task box (`Shift+Enter` or `Ctrl+J` inserts a line, `Esc` cancels, `Enter` launches) that starts a new agent in the selected worktree with what you typed as its first prompt — no picker, no name step. Which CLI it launches is the `Quick prompt agent` setting in Settings → Agents, run with that harness's own default model and effort; the session titles itself from the prompt. For one launch only, `Tab` picks a different harness (`→` drills into its model and effort, same as the new-session picker) and `Shift+Tab` picks one of your saved agent presets — adopting its harness, model, effort and prefix/postfix wrapping. Either picker hands the box back with your text intact, on `Esc` too. The launch stays out of your way: the new session's row is selected and shown in the pane, but focus stays on the panel you fired from — turn on `Quick prompt focus` in Settings → Agents to drop straight into its terminal instead |
| Any panel | `Shift+D` | delete every row of the focused panel (confirm lists the casualties) |
| Any panel | `g` | git diff for the selected worktree: filter, `↑↓` files, `Shift+↑↓`/`PgUp/PgDn`/`Ctrl+d/u` scroll, `Ctrl+r` marks a file reviewed ✓ |
| Any panel | `Shift+G` | open the selected repo's page on its git host — the `origin` remote (`git@github.com:o/r.git`, `ssh://`, `https://`) turned into a browsable URL, credentials stripped |
| Any panel | `f` / `F` / `b` | find file / find in files (`git grep`) / file tree browser, all scoped to the selected worktree — `Enter` opens the file in an editor modal (at the matched line, for `F`); in `f` and `b`, `Ctrl+y` copies the path |
| Sessions | `Enter` on an OPEN PRS row | open it in the browser (a previously saved link can still be edited with `r` or deleted with `d`; the detected pull request cannot). Resting on the pull request reads it in the pane; `g` shows its diff, `PgUp/PgDn` scroll |
| Any panel | `t` | new shell terminal in the selected worktree's directory (Projects panel: the repo root) |
| Any panel | `w` or click the `◇ workspace` nameplate bottom-left | workspace switcher: `Enter` opens, `n`/`r`/`d` create/rename/delete; a created workspace opens with FOCUS on the first visible panel; delete asks first, and deleting the open one lands on the tab to its right, or the one to its left from the last tab (the panels scope to the open workspace; `/` doesn't, and switches for you). Per window, switching here leaves your other nebula instances on the workspace you left them on |
| Any panel | `Shift+W` | show / hide the Workspaces bar across the top: `WORKSPACES` on the left, directly above `PROJECTS`, and one tab per workspace to its right with the rolled-up status of the agents under it (plus a count of the ones that finished unread), so a run in a workspace you don't have open still shows at the top level. The choice is remembered — it's the `Workspaces bar` setting, also in Settings → Appearance |
| Any panel | `Shift+P` | show / hide the PROJECTS PANEL. The TERMINAL PANE takes the released width; showing the panel restores its remembered width without stealing FOCUS. Persisted as `hide_projects` in CONFIG.JSON and also available in Settings → Appearance |
| Any panel | `Shift+B` | show / hide the WORKTREES PANEL independently from the PROJECTS PANEL. The TERMINAL PANE takes the released width; showing the panel restores its remembered width without stealing FOCUS. Persisted as `hide_worktrees` in CONFIG.JSON and also available in Settings → Appearance |
| Any panel | `1`–`9` (or `⌘1`–`⌘9`) | open that numbered tab in the Workspaces bar without leaving the panel you're in. `⌘` is what the tabs advertise, but Terminal.app and most other emulators never encode it into pty bytes — the bare digit is the one that always arrives. Rebindable per slot in Settings → Hotkeys |
| Workspaces | `←/→`, `↓`/`Enter`, `n`/`r`/`d`, `m` | the cursor is the open workspace, so `←/→` switches; `↓` or `Enter` steps down into the first visible panel; create / rename / delete the open one (a created workspace opens there too; delete asks first, refuses a non-empty workspace, and lands on the tab to the right, or the one to its left from the last tab); `m` or right-click lists the same verbs |
| Any panel | `Shift+H` | ssh hosts: every `nebula ssh` / `nebula tunnel` destination, newest first. `Enter`/click reconnects (quits this TUI and execs a fresh `nebula ssh` — local sessions keep running), `a` types a new `user@host [dir]`, `d` removes |
| Any panel | `m` or right-click | context menu |
| Any panel | `z` | full-screen terminal: collapse the sidebars and lock input into the attached session |
| Any panel | `s` | settings overlay (theme, editor, which agents to offer and their defaults, timeouts) — its Hotkeys tab rebinds every key in this table; `R` inside it resets everything to the defaults (with a confirmation). A first open lands on the tab strip; reopening within a minute of closing lands back on the tab and row you left, and after that it opens fresh again |
| Any panel | `Shift+M` | memory usage: RAM per agent/terminal process tree, nebula itself, and the machine-wide share; `↑/↓` + `Enter` opens the selected session |
| Any panel | `Shift+N` | replay the startup splash (any key returns) |
| Any panel | `?` | help overlay |
| Any panel | `q` / `Ctrl+c` | quit the TUI (sessions keep running) |
| Terminal | anything | forwarded raw to the PTY |
| Terminal | `Ctrl+q` | back to panels (also expands sidebars) — `Ctrl+]`, `Ctrl+Esc` and `Ctrl+←` do the same, for terminals that eat one of them |
| Terminal | mouse wheel | scrollback (arrow keys on alt-screen apps) |
| Any typed field | `←→`/`⌥←→`, `Ctrl+a`/`Ctrl+e`, `⌥⌫`, `Ctrl+u`/`Ctrl+k` | every prompt, filter and query is the same line editor: move by character / word, jump to ends, delete word, kill line |

## Mouse

Left-click selects/attaches, right-click opens context menus, double-click in the terminal selects
a word, `⌥`-click opens the URL or `file:line` under the cursor (browser / editor modal), and dragging a
visible panel border resizes it. Hidden panels keep their last width for the next time they are shown.
A click outside any modal (help, settings, a confirm, a prompt, the pickers)
dismisses it, exactly as `Esc` would. Text selection: hold `Shift` while dragging (mouse capture bypass —
same as tmux).
