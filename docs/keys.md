# Keys

<sub>[← README](../README.md) · [Keys](keys.md) · [Commands](commands.md) · [Sessions](sessions.md) · [Configuration](configuration.md) · [How it works](how-it-works.md)</sub>

Every binding below is a default, and every one of them is rebindable in the SETTINGS OVERLAY's
HOTKEYS TAB (`s`) — see [Configuration](configuration.md).

## Worktree views

The panels aren't the only view. With a worktree selected, from any panel:

| Key | View |
|---|---|
| **`g`** | **Git diff.** Changed files down the left, the diff on the right, with a live fuzzy filter. On an open-PR row it shows that pull request's diff instead, fetched whole with `gh pr diff`. `Ctrl+r` marks a file reviewed ✓ and sinks it to the bottom — nebula-side bookkeeping only, no git state is touched — and every mark clears itself when HEAD moves or the file changes again, so what's left unticked is genuinely what you haven't read. |
| **`f`** | **Find file.** Fuzzy finder over the worktree. `Enter` opens the file in an editor modal (vim by default; the `editor` setting or `NEBULA_EDITOR` picks another), `Ctrl+y` copies the path — ready to paste into an agent. The editor modal sits above every OVERLAY and is handed every key raw — vim owns `Esc` — so `Ctrl+q` is the only way out of it: one press kills the editor process and closes the modal, and the mouse is inert the whole time it is up. |
| **`F`** | **Find in files.** `git grep` into the same modal; `Enter` opens the hit at its line. |
| *(none — an agent opens it)* | **File tabs.** What `nebula open <file>…` raises when a session runs it — ask an agent to "open it" or "show me the file", or let it put its own work in front of you. A modal with a tab strip like the SETTINGS OVERLAY's, one tab per file: `←`/`→`, `Tab`/`⇧Tab` and `1`-`9` switch tabs, the focused tab's file is previewed underneath with the TREE BROWSER's highlighting, and `Enter` runs the editor in that pane (the tabs stay put; `:q` lands back on the strip with the file re-read). `↓`/`j` from the strip drops into the preview, where `j`/`k`, `Ctrl+d`/`Ctrl+u`, `PgUp`/`PgDn` and `Home`/`End` scroll and `↑` off the top returns to the strip. This is the one OVERLAY where `Ctrl+q` is staged: from the editor or the preview it steps back to the tab strip, and only from the strip does it close; `Esc` backs out the same way. |
| **`b`** | **File tree browser.** Tree on the left, syntax-highlighted preview on the right, and an always-live filter that narrows the tree to matching files and the directories holding them. `→` expands a directory and `←` collapses it; `Enter` folds or unfolds a directory, and on a file it loads the file into the *preview pane* rather than the floating editor modal that `f` and `F` open — the tree stays where it is. The preview takes the DIFF VIEWER's scroll keys: `Ctrl+d`/`Ctrl+u` half pages (`Ctrl+u` only while the filter is empty — with text typed it is the LINE EDITOR's kill-to-start), `Shift+↑`/`Shift+↓` a line, `PgUp`/`PgDn` a page, `Home`/`End` the ends. `Ctrl+y` copies the selected path, and dragging the tree/preview border resizes the tree side. |

## Full keymap

| Context | Key | Action |
|---|---|---|
| Panels | `Tab`/`Shift+Tab` (or `Ctrl+Shift+L`/`Ctrl+Shift+H`), `h`/`l` or `←`/`→`, `j`/`k` or `↑`/`↓` | move FOCUS / selection through visible panels; the walk stops at both ends (`Tab` at the TERMINAL PANE, `Shift+Tab` at the WORKSPACES BAR while it is shown and at the first visible panel when it is hidden) instead of cycling, and landing on a live pane takes its input; `h`/`l` stop one short of each end, and a double tap there (`l`,`l` at Sessions, `h`,`h` at the first visible panel) jumps the boundary; so do `k`,`k` on a panel's first row (up into the WORKSPACES BAR) and `j`,`j` in the bar (back down to the panel you came from). The double taps are the letter keys' way over that boundary, but `Shift+Tab` needs no double tap — it steps into the WORKSPACES BAR in one press. `Ctrl+Shift+L`/`Ctrl+Shift+H` are the same two walk actions and need the KITTY PROTOCOL to arrive at all, so Ghostty and kitty get them and Terminal.app never will |
| Panels | `Ctrl+→` | cross into the terminal pane *without* taking its input (`Tab`, or a double tap of `l`/`→` at Sessions, takes it) |
| Panels | `Enter` | drill in; on a session: attach |
| Any panel | `/` | fuzzy jump across every workspace, project, worktree and session — in *every* workspace, each row pathed `workspace/project/branch/session`, so typing another workspace's name jumps you into it (`Ctrl+n/p` move, `Ctrl+o` opens the hit, `Ctrl+f` just lands the selection on it). Until you type, the rows sit in attention order — sessions waiting on you, then running ones, then unread finishes, then everything else by last interaction — so `/` `Enter` is the fastest way back to what needs you or to the worktree you just left |
| Projects | `n` / `d` | add project / remove from list |
| Any panel | `o` | add ("open") a project — same prompt as `n`, from any focus |
| Add project | type + `Tab`, `↓↑` / `→` / `←` | browse for the repo: type to filter (bash-style Tab completion), arrows pick a directory, `→` steps in, `←` steps up, `Enter` adds the highlighted (or typed) path; `●` marks git repos |
| Projects | `r` | rename the row — a label, not a move: the folder on disk keeps its name and hangs off a `└` under the new one. A terminal cell has one font size (Kitty's OSC 66 renders half-size text, but WezTerm and Ghostty don't implement it), so the hierarchy is weight, opacity and position instead: the name you chose is bold, the folder is the dimmest theme color plus the faint attribute. An empty name puts the row back on the folder's name |
| Worktrees (checkout row) | `n` / `d` | new worktree / delete (typed confirm — deletes files) |
| Worktrees (PROJECT OPEN PRS row) | `n`; `m` / right-click | new SESSION scoped to that PR, on any enabled harness (the same rows as the NEW SESSION PICKER); the context menu also opens the PR or its diff |
| New worktree | type a sentence, or `Enter` on the empty prompt | the branch name is slugified (`fix login redirect` → `fix-login-redirect`); empty takes a random `<adj>-<noun>-<verb>` |
| Sessions | `n` | new session (agent or shell terminal) |
| New session picker (Claude) | `Tab` | toggle Claude Cloud; Cloud adds a wrapped task prompt (`Shift+Enter` or `Ctrl+J` inserts a line) before launch |
| Sessions (cloud row) | `m` | **Attach cloud session** re-pulls the transcript now; **Send to cloud session** queues a message on it |
| Sessions | `r`, `a`, `u`, `d`, `A` | rename, archive, unarchive, delete, toggle archived. `Delete` and `Backspace` are bound alongside `d` and do the same thing, here and everywhere else `d` deletes a row (a project, a worktree) — and archive, cheap to undo with `u`, is the one verb here that skips the CONFIRM DIALOG `d` goes behind |
| Sessions | `e` | agent presets: saved launch definitions (harness, model, effort, optional prefix/postfix text). `Enter` asks for a task and starts the agent with prefix + task + postfix as its first prompt; `a` / `e` / `d` create, edit, delete |
| Any panel | `p` | quick prompt: a wrapped, multi-row task box (`Shift+Enter` or `Ctrl+J` inserts a line, `Esc` cancels, `Enter` launches) that starts a new agent in the selected worktree with what you typed as its first prompt — no picker, no name step. Which CLI it launches is the `Agent` row under **Quick prompt** in Settings → Agents, run with that harness's own default model and effort; the session titles itself from the prompt. For one launch only, `Tab` picks a different harness (`→` drills into its model and effort, same as the new-session picker) and `Shift+Tab` picks one of your saved agent presets — adopting its harness, model, effort and prefix/postfix wrapping. Either picker hands the box back with your text intact, on `Esc` too. The launch stays out of your way: the new session's row is selected and shown in the pane, but focus stays on the panel you fired from — turn on `Focus` under **Quick prompt** in Settings → Agents to drop straight into its terminal instead |
| Any panel | `Shift+D` | delete every row of the focused panel (confirm lists the casualties) |
| Any panel | `g` | git diff for the selected worktree: filter, `↑↓` files, `Shift+↑↓`/`PgUp/PgDn`/`Ctrl+d/u`/`Home`/`End` scroll, `Ctrl+r` marks a file reviewed ✓. `Ctrl+u` scrolls only while the filter is empty — with something typed it is the LINE EDITOR's kill-to-start |
| Any panel | `Shift+G` | open the selected repo's page on its git host — the `origin` remote (`git@github.com:o/r.git`, `ssh://`, `https://`) turned into a browsable URL, credentials stripped |
| Any panel | `f` / `F` / `b` | find file / find in files (`git grep`) / file tree browser, all scoped to the selected worktree — `Enter` opens the file in an editor modal (at the matched line, for `F`); in `f` and `b`, `Ctrl+y` copies the path |
| PALETTE, FILE FINDER (and its grep view), TREE BROWSER | `↑`/`↓` or `Ctrl+n`/`Ctrl+p` | move the selection inside a fuzzy overlay. `j`/`k` deliberately do *not* — they are typed into the query, so a branch or filename with a `j` in it stays reachable — which makes these four the one place the app-wide `j`/`k` convention above stops at the modal's edge. `Esc` is two-staged here as well: the first press clears what you typed, and only the second closes |
| Sessions | `Enter` on an OPEN PRS row | open it in the browser (a previously saved link can still be edited with `r` or deleted with `d`; the detected pull request cannot). Resting on the pull request reads it in the pane; `g` shows its diff, `PgUp/PgDn` scroll it and `Home`/`End` jump to its ends (the arrows keep walking the OPEN PRS list itself, so they are not offered here) |
| Any panel | `t` / `Shift+T` | new shell terminal in the selected worktree's directory (Projects panel: the repo root) |
| Any panel | `w` or click the `◇ workspace` nameplate bottom-left | workspace switcher: `Enter` opens, `n`/`r`/`d` create/rename/delete; a created workspace opens with FOCUS on the first visible panel; delete asks first, and deleting the open one lands on the tab to its right, or the one to its left from the last tab (the panels scope to the open workspace; `/` doesn't, and switches for you). Per window, switching here leaves your other nebula instances on the workspace you left them on |
| Any panel | `Shift+W` | show / hide the Workspaces bar across the top: `WORKSPACES` on the left, directly above `PROJECTS`, and one tab per workspace to its right with the rolled-up status of the agents under it (plus a count of the ones that finished unread), so a run in a workspace you don't have open still shows at the top level. The choice is remembered — persisted as `show_workspaces` in CONFIG.JSON, written by the hotkey itself so a hidden bar survives a crash, and also available as the `Workspaces bar` row in Settings → Appearance |
| Any panel | `Shift+P` | show / hide the PROJECTS PANEL. The TERMINAL PANE takes the released width; showing the panel restores its remembered width without stealing FOCUS. Persisted as `hide_projects` in CONFIG.JSON and also available in Settings → Appearance |
| Any panel | `Shift+B` | show / hide the WORKTREES PANEL independently from the PROJECTS PANEL. The TERMINAL PANE takes the released width; showing the panel restores its remembered width without stealing FOCUS. Persisted as `hide_worktrees` in CONFIG.JSON and also available in Settings → Appearance |
| Any panel | `1`–`9` (or `⌘1`–`⌘9`) | open that numbered tab in the Workspaces bar without leaving the panel you're in. `⌘` is what the tabs advertise, but Terminal.app and most other emulators never encode it into pty bytes — the bare digit is the one that always arrives. Rebindable per slot in Settings → Hotkeys |
| Workspaces | `←/→`, `↓`/`Enter`, `n`/`r`/`d`, `m` | the cursor is the open workspace, so `←/→` switches; `↓` or `Enter` steps down into the first visible panel; create / rename / delete the open one (a created workspace opens there too; delete asks first, refuses a non-empty workspace, and lands on the tab to the right, or the one to its left from the last tab); `m` or right-click lists the same verbs |
| Any panel | `Shift+H` | ssh hosts: every `nebula ssh` / `nebula tunnel` destination, newest first. `Enter`/click reconnects (quits this TUI and execs a fresh `nebula ssh` — local sessions keep running), `a` types a new `user@host [dir]`, and `d` (or `x`, `Delete`, `Backspace`) forgets one — alone among nebula's destructive keys it asks nothing first, because the next `nebula ssh` to that host just re-adds it |
| Any panel | `m` or right-click | context menu |
| Any panel | `z` | full-screen terminal: collapse the sidebars and lock input into the attached session |
| Any panel | `s` | settings overlay (theme, editor, which agents to offer and their defaults, timeouts) — its Hotkeys tab rebinds every key in this table; `R` inside it resets everything to the defaults (with a confirmation). A first open lands on the tab strip; reopening within a minute of closing lands back on the tab and row you left, and after that it opens fresh again. `Tab`/`Shift+Tab`, `[`/`]` and the digits `1`–`9` move between tabs, `j`/`k` (or `↑`/`↓`) move down the rows — `↑` off the top row steps back onto the strip — and `h`/`l` cycle the selected row's value. On a HOTKEYS TAB row the grammar changes: `Enter` or `Space` starts a capture, `a` or `+` captures a *second* chord beside the one already there, `Backspace`/`Delete` puts the default back, `x` unbinds it outright, and `Esc` cancels a capture in progress. Pressing a chord that some other action already owns does not take it — the row warns you who has it (`… is already “Git diff” — Enter to move it here, Esc to keep it there`) and a second `Enter` is what actually steals it |
| Any panel | `Shift+M` | memory usage: RAM per agent/terminal process tree, nebula itself, and the machine-wide share; `↑/↓` + `Enter` opens the selected session |
| Any panel | `Shift+N` | replay the startup splash (any key returns) |
| Any panel | `?` | help overlay |
| Any panel | `q` / `Ctrl+c` | quit the TUI — behind a CONFIRM DIALOG ("Leave the TUI? Sessions keep running in the daemon."), because `q` sits one finger away from every other panel hotkey and a stray letter aimed at an agent used to end the client outright. `y`/`Enter` goes, `Esc`/`n` stays, and a second `Ctrl+C` while the dialog is up leaves without asking again — that shortcut is wired to the quit confirm alone, no other CONFIRM DIALOG takes it. Nothing is killed either way: the DAEMON keeps every session running |
| Terminal | anything | forwarded raw to the PTY — until the session's process exits, when nothing is forwarded any more: `Esc`, `Enter` and `q` leave the lock and every other key falls through to panel navigation, so an `exited` pane never swallows a keystroke it cannot use |
| Terminal / any OVERLAY | `Ctrl+q` | the HARDWIRED UNLOCK, and the one binding that is more than a hotkey. From a locked TERMINAL PANE it goes back to the panels (also expanding collapsed sidebars). With any OVERLAY open it force-closes that overlay in a single press from any nesting depth — a typed filter, an open submenu, a live hotkey capture, the preset editor stacked over its list, the editor modal — landing you on the panels every time. The one staged exception is the file tabs an agent opens with `nebula open`: there it steps back to the tab strip first, and closes from the strip. It stays wired in on top of whatever the KEYMAP says, so no rebind, nested modal or half-typed field can ever trap you. `Ctrl+Shift+H` (KITTY PROTOCOL only), `Ctrl+]`, `Ctrl+Esc` and `Ctrl+←` are the other four defaults for leaving the pane, for terminals that eat one of them — but those four are ordinary rebindable bindings, and only `Ctrl+q` closes an OVERLAY |
| Terminal | mouse wheel | three cases, tried in this order. A child that has asked for the mouse — Claude Code's alt-screen UI does, as do `vim` with `mouse=a` and `htop` — gets the notch forwarded as a real mouse-wheel report (SGR, or legacy X10 bytes when that is the encoding it negotiated), because synthesized arrows would land in its input box, cycle its prompt history and trip its "Scroll wheel is sending arrow keys" warning. An alternate-screen app that wants no mouse (plain `vim`, `less`) gets `↑`/`↓` instead, one per line the notch would have scrolled. Anything else scrolls nebula's own scrollback, a line a notch, and the header counts how far back you are as `scroll N` |
| Any typed field | `←→`/`⌥←→`, `Ctrl+a`/`Ctrl+e`, `⌥⌫`, `Ctrl+u`/`Ctrl+k` | every prompt, filter and query is the same line editor: move by character / word, jump to ends, delete word, kill line |

## Chips and readouts

Two strips report state without being asked. The TERMINAL PANE's header shows one chip at a
time — the first of these that applies, so a scrolled-back exited session reads `exited`:

| Chip | What it is telling you |
|---|---|
| `exited` (red) | the session's process is gone. Keys stop reaching it — `Esc`, `Enter` and `q` leave the lock and everything else falls through to panel navigation — and the next attach is what respawns it |
| `scroll N` (yellow) | you are N lines back in the scrollback rather than at the live edge. Typing anything, or scrolling back down to the bottom, clears it |
| `starting…` (dim) | nothing has come off the PTY yet: the session was reaped while you were elsewhere and its CLI is booting now. The blank grid on its own reads as a hang, so the chip says otherwise |
| `INPUT` (accent) | the pane is locked and every key is going to the PTY — where `Enter`, the forward walk, a double tap of `l`/`→` and `z` all leave you |

The FOOTER carries the rest, left to right:

| Readout | What it is telling you |
|---|---|
| `nebula vX.Y.Z` at the far left | which nebula this is — the same string `nebula --version` prints, and the one thing on the bar that never moves with the cursor. It costs the key hints ~18 columns and steps aside only for a flash message that would not otherwise fit |
| `⇡ v0.22.0` right after it, in the warning color | a newer nebula is published on GitHub — the nudge to run `nebula upgrade`. Checked once at start and hourly after with one `curl` to the release page's redirect (no `gh` token, no API quota); it says nothing while it can't ask, and `NEBULA_UPDATE_CHECK_SECS=0` turns the check off |
| `◇ workspace` beside it | the open workspace everything else on screen is scoped to; click it for the workspace switcher (`w`) |
| `+3 files` after the breadcrumb | the selected worktree's dirty-file count, in the warning color. It rides the breadcrumb because it is context about that checkout, not chrome, and it is absent at zero |
| `✗ disconnected` | this client has lost the DAEMON. It is the client-to-daemon link and nothing else — a session's own health is its `●` STATUS DOT, and a hollow `○` is an agent the daemon restarted out from under. A healthy connection says nothing at all: the bar reports the link only when something is wrong |
| `2 agents · 1 term · 3 warm · 412 MB` at the right edge | live counts and nebula's whole memory footprint — this TUI, the DAEMON and every session's process subtree summed — re-read every 5 seconds while no modal is up. Prewarm spares are counted apart as `warm` so the agent figure matches the sidebar, and the whole readout is blank until the first reading lands |

## Mouse

Left-click selects/attaches, right-click opens context menus, double-click in the terminal selects
a word, `⌥`-click opens the URL or `file:line` under the cursor (browser / editor modal), and dragging a
visible panel border resizes it. Hidden panels keep their last width for the next time they are shown.
A click outside any modal (help, settings, a confirm, a prompt, the pickers) dismisses it, and is
swallowed rather than landing on the panel underneath. That is not always the same thing as `Esc`:
the DIFF VIEWER, the PALETTE, the FILE FINDER, its grep view and the TREE BROWSER all stage `Esc` —
the first press clears the typed filter and only a second one closes — where a click outside closes
them outright, first press. The CONTEXT MENU splits the same way: `Esc` backs out one submenu level
at a time, a click outside takes the whole menu. The rest (a CONFIRM DIALOG, a prompt, the agent
presets list, the preset editor) really do run their own `Esc`, side effects and landing included.
The wheel scrolls whatever the pointer is over: the WORKTREES PANEL and SESSIONS PANEL move their
lists without touching the selection, and the PR PREVIEW scrolls three lines a notch while a cursor
is resting on a pull request. Text selection: hold `Shift` while dragging (mouse capture bypass —
same as tmux).

A host terminal can forget that it was asked for the mouse — iTerm2's Session ▸ Reset (`⌘R`) does,
and so does a stray full reset — after which every click goes to the terminal and the wheel scrolls
its own scrollback instead of the pane. nebula asks for the mouse (and bracketed paste, focus reports
and the kitty keyboard flags) again every two seconds, so the mouse comes back on its own; resizing the
window also re-enters the alternate screen and repaints every cell.
