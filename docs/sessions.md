# Sessions

<sub>[← README](../README.md) · [Keys](keys.md) · [Commands](commands.md) · [Sessions](sessions.md) · [Configuration](configuration.md) · [How it works](how-it-works.md)</sub>

Everything that can start an AGENT, and what each launch path does differently.

## The NEW SESSION PICKER

With a WORKTREE selected, press `n` in the SESSIONS PANEL. A menu asks what to
run — **Claude**, **Codex**, **Cursor**, **Pi**, or **Muse** (a plain shell is `t` — see [Keys](keys.md)); a CLI you never use can be
switched off on the settings overlay's Agents tab and drops out of the menu entirely. Turn on `Hide missing CLIs`
on the Agents tab and the menu lists only enabled harnesses whose CLI is found on PATH (the daemon still
checks through the login shell at launch). Your own CLIs join the menu too: add them to config.json
`custom_harnesses` (see [Configuration](configuration.md)) and they appear after the built-ins under their
own labels, toggled per entry on the Agents tab's Custom harnesses row, with the session row wearing the
entry's label as its badge. A custom entry launches with its program and model flag, boots fresh every
time (no resume mapping), and — unless it names a built-in hook dialect — stays process-based: yellow
while the PTY is live, green when it ends, never red. `→` on any row drills
into model and reasoning-effort submenus (Cursor's model is a family such as `claude-opus-5-thinking`, and
its effort list follows the family, `-fast` variants included — `cursor-agent --list-models` bakes both
into the id, so nebula launches `--model claude-opus-5-thinking-high-fast`; the list is a built-in seed
merged with `--list-models`, cached in `cursor_models.json` beside `config.json` and refreshed daily;
Pi's model is a fuzzy `--model` pattern — `opus`, `sonnet`, or a `provider/id` you set in `config.json` —
and its effort is the `--thinking` level, `off` through `max`).
In those submenus you type to filter — `opus` narrows the rows to the Opus families, `↑`/`↓` move, `Backspace` widens, `Esc` clears — and the preset editor's Harness / Model / Effort rows take the same type-ahead.
`Enter` anywhere takes your configured defaults. On the
Claude row, `Tab` toggles Cloud mode: enter the task in the wrapped editor
(`Shift+Enter`, `Option+Enter` or `Ctrl+J` adds a line) and nebula launches `claude --cloud=<task>` — the value binds
with `=` and never a space, because `--cloud` takes an *optional* value, so a separate argv item starting
with `--` would be read as another Claude flag instead. The CLI creates the session, prints its URL and
exits — nebula reads the session id off that output, and that is where the local side ends. The agent
runs in Claude's cloud sandbox, not in a terminal here, so the row wears a `cloud` badge and its pane is
the **CLOUD SESSION PANEL** instead of a terminal: a line saying so, and the session's
`https://claude.ai/code/session_…` link, underlined. `Enter` on the row — or a click on the link — opens
the page in the browser. Nothing is attached, teleported or re-homed on your behalf, the checkout never
switches branch, and **Attach** and **Restart** are not offered: there is no local session behind the
row, and the daemon refuses to boot a bare `claude` in its name. To steer the cloud agent without a
browser, pick **Send to cloud session** from the row's `m` menu — the same wrapped editor — and nebula
runs `claude -p <message> --cloud=<id>`; the reply lands on the session's page, the CLI never returns
one. Otherwise `Enter` on a row is the launch: nothing asks for a name or a task first — nebula spawns
the CLI in that worktree and drops you straight into it, and you type the agent's first prompt there.
The session titles itself from that first prompt (AUTO-TITLE); `r` renames it whenever you like. To
start an agent on a task you type up front instead, use `p` (the QUICK PROMPT — see [Keys](keys.md)).

The picker opens on its first row every time, whatever you picked last — unless **Remember harness**
is on (Settings → Experimental, `remember_harness` in CONFIG.JSON). Then every launch you walk
through this picker, the PR SESSION picker or the QUICK PROMPT's `Tab` picker writes its harness into
the Agents tab's **Quick prompt › Agent** row, and a model or effort you drilled into through the
submenus into that harness's own **Model** / **Effort** rows: the next `n` opens on that harness with
its ✓ on that model, `Enter` launches it, and `p` launches it too. The rows are the ordinary settings,
so the Agents tab always shows what the next launch will be, and you can still change them there. An
AGENT PRESET launch leaves them alone — its harness is the preset's, not a change of mind.

The rows do not run under the same permissions, and the picker is where you decide that. Claude
is spawned with no permission flag at all and keeps its normal prompts — it stops and asks before the
things it is configured to ask about. Codex is spawned with `--yolo` and Cursor with `--force`, so
**neither of those two ever stops to ask**: they edit files and run commands on their own judgment for
the life of the SESSION, and nothing in the picker or the settings overlay softens that. Pi has no
permission gate to begin with — nebula passes no flag, and it runs its tools as it sees fit. Muse is
the same: no flag mapped yet. Pick the
harness with that in mind, especially in the ROOT WORKTREE.

The same choice reaches the STATUS DOT, because an AGENT can only report what its hook set can see.
Claude installs the full set — `UserPromptSubmit`, `Stop`, `SessionStart`, `PermissionRequest`,
`Notification` (which is where the idle prompt comes from), `PreToolUse` on `AskUserQuestion` and an
unmatched `PostToolUse` — so a Claude row walks the whole range of states, red NEEDS FEEDBACK
included, and leaves red the moment you answer: a question's answer is its own tool's `PostToolUse`,
and an approved permission prompt shows up as the gated tool running, whichever tool it was. Codex
has no `Notification` hook and no `AskUserQuestion` tool, but its native `PermissionRequest` is
installed, so the red state stays reachable there (and, with no `PostToolUse` to say you approved,
a Codex row stays red until the turn ends). Cursor has no `PermissionRequest` hook to install,
and since nebula runs it with `--force` there is nothing left to wait on anyway: its hooks are
`sessionStart`, `beforeSubmitPrompt`, `stop`, `subagentStart` and `subagentStop`, which is busy versus
idle and nothing else. **A Cursor SESSION can never show the red NEEDS FEEDBACK dot** — if you are
watching one and waiting for it to ask you something, it is not going to. Pi has no shell hooks at all:
nebula installs one managed extension (`~/.pi/agent/extensions/nebula.ts`, inert outside nebula) that
posts pi's `session_start`, `before_agent_start`, `agent_end` and `ask_question` tool events as
`SessionStart`, `UserPromptSubmit`, `Stop` and `PreToolUse` / `PostToolUse`, and any blocking prompt an
extension raises mid-run as `PermissionRequest` — so a Pi row goes yellow, red while its `ask_question`
tool waits on you, and green when the run ends, a cancelled run included. Muse has no hooks at all yet:
its row is yellow while the PTY is live and green when the process ends, and it never goes red.

## AGENT PRESETS

If you keep starting the same kind of session with the same framing, save it as an **agent preset**:
`e` in the Sessions column — or on a worktree's row in the Worktrees column, which launches into
that worktree without walking over to its sessions — lists them. Type to find one by name: letters
narrow the list to the fuzzy matches, `↑`/`↓` move, `Backspace` widens and `Esc` clears — as in the
model and effort submenus. `Ctrl+a` opens a small form — name, harness, model, effort, **Text**
(which side of the task the preset's text goes: `prefix`, `postfix` or `prefix & postfix`, with a
box for each side it names) and **Task** (`ask` or `skip`) — and `Ctrl+e` / `Ctrl+d` edit or
delete; they are chords because plain letters type. A new preset starts on the side **Preset text**
in Settings → Sessions names (`preset_text` in CONFIG.JSON) — `prefix` unless you change it: one
box, sent before your task — and cycling the row shows the other box or both; a side the row leaves
out saves blank. Editing a preset also shows any side that already holds text, so one saved with
both never loses either. `Enter` on a
preset asks for the task in the same wrapped editor, then launches the CLI with `prefix + task + postfix`
as its very first prompt, so the agent is already working when the pane opens. The task is optional:
send the box empty and the prefix and postfix go on their own (a preset with neither starts the CLI
with no first prompt). Set **Task** to `skip` for a preset that never needs one — a "commit and push" —
and `Enter` launches it at once, no box at all; the list marks those rows `no task`. A `skip` preset
picked with `Shift+Tab` in a quick prompt, or with `e` in the issues modal, launches the same way when
the box is still empty, while text you already typed stays yours to send. The row it creates is an
ordinary session: it names itself on that first turn, resumes, and shows status like any other. Presets
live in `agent_presets.json` beside `config.json`. The form's Harness row lists custom registry entries
by id alongside the built-ins — a preset on one launches with the entry's program and defaults, and
refuses with the reason when its entry is switched off or gone.

On an open pull request's row in the Worktrees column (the project's `OPEN PRS` group), `e` is a
picker instead of the manager: the preset you pick launches a PR SESSION on that pull request. The
box comes back with the preset applied and the PR in its title, and `Enter` starts the agent in the
project's worktree on the PR's head branch — reused when one is already checked out on it, cut by
the daemon otherwise — with the PR link and its work rule in the system prompt and the preset's
`prefix + task + postfix` as the first prompt, so the agent is already working on the PR when the
pane opens. A `skip`-task preset launches straight from the picker. `p` on the row is the same launch
without a preset: the box titled for the PR, your text alone as the first prompt, and the checkout's
row up under the pull request as soon as you press `Enter` — never a random-branch worktree the
session would have to move onto the pull request by hand. Both keys work from whichever panel has
focus while the pull request's row is selected, the pane reading it included.

A contributor's pull request from a fork gets a checkout named for the fork: `givemeurhats/main`,
`wende/feat/settings-hotkey`. A fork's branch shares nothing with yours but possibly a name — `main`
above all, which every fork has and your root checkout is on — so under its bare name a PR SESSION on
a fork's `main` ran in the root checkout, on your code, with nothing cut and nothing nested. Under the
fork's name it matches no branch of yours: the daemon seeds it from the pull request's own ref
(`refs/pull/N/head`) and points its upstream there, as `gh pr checkout` does, so `git pull` in the
checkout follows the contributor's pushes and the checkout's own PR ROW finds the pull request. A
checkout of a fork's pull request made before this, under the bare branch name, is a plain worktree
row now; the next PR SESSION cuts the fork-named one.

If the daemon refuses a PR SESSION — the fetch failed offline, the fork is gone — the `creating` rows
come down and the box you sent it from comes back with your text, as any refused quick prompt does.

## The FOLLOW-UP COMPOSER

The next turn for a session already running, typed into its own card. Put the cursor on an agent's row
in the SESSIONS PANEL and press `Space` — or click the `▸` at the end of its name row, or pick
**Follow-up prompt** from its `m` menu — and the card expands in place: a small framed box opens inside
the pill, under whatever RECENT PROMPTS the row carries, with `follow-up` on its top border and the keys
on its bottom one. There is no modal over the screen; the panels stay exactly where they were, and every
card below this one in the column moves down by what the box took, off the bottom of the column if it
runs out. The column scrolls to keep the box you are typing into on screen, and goes on doing so as the
box grows — it takes up to four lines of text before it starts scrolling under its own caret.

`Enter` sends what you typed to the agent as its next turn and folds the card back up, with
`sent to <session>` in the footer; `Shift+Enter`, `Option+Enter` and `Ctrl+J` break a line, as in Claude
Code's own prompt, and `Esc` folds the card without sending. The text goes straight down the session's
PTY — the same path your keystrokes take in the pane — so the CLI sees it as a prompt typed at it, and
the pane swaps to that session so you can watch the turn land. A prompt with line breaks in it crosses
as one bracketed paste rather than as typing, so nothing auto-indents it to mush.

While the box is open it owns the keyboard: the panel's own verbs are bare letters, so `a`, `d` and `r`
are letters in your prompt and not archive, delete and rename aimed at the session you are prompting.
`Tab` still walks to the next panel and leaves the card expanded behind it, and clicking another card
folds the box. The toggle on each card says which state it is in — `▸` folded, `▾` expanded — and a
click on it does either.

Only a live local agent has a card to expand. An archived session's turn is over, a Claude Cloud row's
agent runs in a sandbox with a message queue of its own (**Send to cloud session** in its menu), a shell
terminal takes typing in the pane, and a pull request row is not a conversation — each says so if you
ask. A session whose CLI is not up — reaped by the IDLE REAPER, or cold since the daemon started — is
booted first and the box left as it is with `starting <session>` in the footer: nothing is typed into a
process that is still starting, so press `Enter` again once it is up.

## RECENT PROMPTS

An experimental read on what each session was last asked to do. Turn on **Recent prompts** under
Settings → Experimental (`recent_prompts` in CONFIG.JSON) and every session row in the SESSIONS PANEL
grows a short list under its pill: the last few prompts typed into it, oldest first so the bottom line
is the latest ask, each condensed to one line and clipped to the column, with a dim `30m ago` pinned
to the right — the same label the rows themselves carry. **Recent prompts shown**
(`recent_prompts_count`, `3` by default, `1` to `5` in the overlay) says how many; the DAEMON keeps the
newest ten per session, so raising the number later has history to draw from at once.

The text is the prompt as you typed it, not a paraphrase. The DAEMON reads it off the
`UserPromptSubmit` hook payload every harness sends (Claude, Codex and Cursor name it `prompt`; Pi's
managed extension posts the same field), collapses its whitespace and keeps the first 200 characters,
so a pasted file shows as its opening line. It costs the agent nothing — no extra turn, no tool call,
nothing added to its context — which is why it is the prompt and not a summary the model wrote.
Prompts nebula composes itself, such as a PR SESSION's scope or the note a `nebula worktree`
relocation reopens on, are left out, and so are blank ones. The lines belong to their row: they sit
inside its pill, and on the row the cursor is on they take the pill's fill with the rail running down
beside them, so the list reads as part of the selected session rather than as rows beneath it. A
click on any of them lands on the session, archived rows list none, and a session created before the
feature simply has nothing to show until its next prompt.

## The LAUNCHER VIEW

An experimental layout for working prompt-first. Turn on **Launcher view** under Settings →
Experimental (`launcher_view` in CONFIG.JSON) and nebula opens on the QUICK PROMPT, focused, so the
first thing you do is type the task:

- **The box** starts on the selected project and on a fresh worktree cut for the session, launching
  the harness, model and effort the Agents tab defaults name — the title spells them out, `New
  session (claude · opus · high)`, and the row under it names the project and the worktree.
  `^P` puts the PROJECT PICKER over it — literally over it: the list floats inside the box, which
  stays on screen under it with its title, its details row and the task already typed into it, so
  aiming the launch never costs you sight of what you are launching. Every project on the machine is
  in the list, the open workspace's first, narrowed as you type; Enter aims the box there with your
  text kept. Aiming the box is not
  navigation — the grid behind it stays on the project you are working in, and the open workspace
  stays open even when the pick lives in another one, so a prompt fired at another project is a
  **background launch**: the session starts over there and nothing on screen moves. The crumb
  highlights the project when the box is aimed away, and the footer names it once Enter lands.
  `^O` opens the harness's model list straight away (`→` on a model reaches its efforts) and `Tab`
  the harness picker — both over the box, as the project picker is, so the task stays in front of
  you while you pick what will run it. `⇧Tab` takes a preset, and `^N` flips between a fresh
  worktree and the project's own checkout — the choice sticks for the next box. Enter
  launches; Esc leaves the box for the grid, keeping what you typed — `p` opens on it again.
- **The grid** replaces the three panels and takes the top of the body: every unarchived session of
  the **selected project**, most recently touched first — the Sessions panel's own order — as a wall
  of cards — up to four a row, fewer as the terminal narrows,
  under a `nebula / default / web / sessions` header that counts them, with a STATUS TALLY of dots
  beside the trail — one dot per state the grid has a card in, carrying that state's count and no
  word at all: red waiting on you, blue an unread finish, yellow mid-turn, purple landed, in that
  fixed order and left out entirely where a state is empty, so a quiet grid keeps a bare trail and
  the dots that are there never move as the work under them does. The trail itself never animates —
  the dots are what changes. Every crumb is a button that opens what its word names, the way a path
  segment does:
  a click on the workspace opens that workspace's projects, a click on `nebula` the machine's
  workspaces, landing exactly where `Esc` would. Each
  card is the session's name with its status dot and how long ago, then where it runs and with what
  (`↳ feat · claude opus`, `⌂` for a root checkout — the project is the grid's own scope, named
  once in the header rather than on every card), then its pull request —
  `↗ #42 Polish the nav  ready` in the colors the PR rows wear (red for conflicts or failing checks,
  purple once merged) — then the last thing it was asked to do, on a `›`, over three rows so a
sentence reads as one (what still does not fit ends in an ellipsis). The pull requests come
  from the same `gh` lookups the panels make, swept over every checkout the level lists rather than
  only the one under the worktree cursor.
- **Walking it** is `h`/`j`/`k`/`l` (or the arrows): `h` and `l` move along a row and stop at its
  ends, `j` and `k` move down the column. The wheel moves nothing: a notch over the cards is
  ignored, so a trackpad cannot swap the pane out from under the card you are reading. The window
  scrolls only as far as it must to keep the cursor's card on screen.
- **The pane** runs along the bottom, under the cards, and reads whichever card the cursor is on:
  `SESSION · polish-nav` on its header and that session live under it, swapping as you walk the grid,
  so stepping across a wall of cards reads each one's progress in turn. It is the same pane the
  panels have — the same attach, the same scrollback and wheel, and the card it shows is marked read
  the moment it lands there, so a `done` badge comes down as you arrive rather than when you open it.
  A click into the pane types into that session where it stands, with the grid still up over it, and
  `^q` hands the keys back to the cards. Its top edge is draggable — a short `━` grip marks it, and
  pulling it up or down trades rows between the cards and the session under them, stopping against
  the pane's own minimum one way and the header plus one row of cards the other. The height you
  leave it at is remembered across restarts, and re-fitted to the window each frame. On a terminal
  too short for the header, a row of cards and a pane worth the name, there is no pane and a session
  is only ever seen full-screen.
- **Stepping into one** is Enter (or `Tab`, `^→`, or a double-click): the keys cross into the pane
  along the bottom, where that session is already running, with its input locked and the grid still
  up over it — the same place a click into the pane lands. `^q` hands the keys back to the cards.
- **Opening one full-screen** is `z`: that session takes the whole screen — the grid and its pane
  both give way — with its input locked, exactly as `z` full-screens the pane out of the panels. Its
  header is a
  breadcrumb — `‹ sessions / ● Fix the login redirect loop`, with the harness, model and checkout
  right-aligned — and `^q`, or a click on `‹ sessions`, comes back to the grid with the cursor on
  the card you came from. `p` (or `n`) opens the box again, on the project under the cursor.

- **The levels** are what the grid is one of. The view is one path down the tree — **workspaces →
  projects → sessions** — with `Enter` a step in and `Esc` a step back out; `k`,`k` off the grid's
  top row walks out too, the way `k`,`k` on a panel's first row steps up into the workspaces bar.
  Every level's cards are the same height, so the screen never jumps as you walk; only what a card
  says changes.
  - **Projects** is a card per project of the open workspace — its name with the loudest status dot
    under it and how long since anything in it moved, then its sessions, the ones waiting on a human
    first, then the ones running, then the rest most recently touched first, with `+ 3 more` when
    they outrun the card. The cards themselves are in that same order, so the project to look at is
    the one the eye lands on top left. `Enter` walks into the project under the cursor: the sessions
    level, scoped to it, with the cursor on the session its card led with. A project with nothing in
    it opens the box on it instead.
  - **Workspaces** is a card per workspace, naming the projects under it in the projects level's own
    order — so the card is a preview of what `Enter` opens. These stay in tab order however loud one
    gets: the cursor here *is* the open workspace, so a card that moved would take the cursor with
    it, and moving the cursor is opening that workspace (quietly — nothing is restored or attached
    until `Enter`). `w` goes straight here from any level, and `1`–`9` open the Nth and land on its
    projects.
  - Neither level has a pane: a project or a workspace card is not something the pane can read, and
    previewing one would boot a session nobody is looking at. The session the level came from keeps
    running, so walking back in is instant.
  - A key that acts on a session (`a`, `d`, `r`, `e`) is swallowed above the sessions rather than
    falling through — `a` must not archive a session you cannot see — while the keys that act on a
    project (`i`, `v`, `g`, `c`, `/`) keep working, since the cursor there is a project.
  - **Workspaces are a level, not a dialog** here: `w`, `⇧W`, the `1`–`9` tabs and a click on the
    footer's `◇ name` nameplate never drop the panels' switcher over the grid — `w` and the
    header's `nebula` crumb both walk to the WORKSPACES level instead. `^P` in the box is the
    other way across — it lists every project on the machine, and picking one only aims the box:
    the workspace you are in stays open, and the launch runs in the background over there.

The grid's cursor is the panels' own selection, so every other key keeps its meaning on the session
under it — `a` archives, `d` deletes, `g` opens its diff, `m` its menu, `/` jumps, `s` opens
Settings. Turning the switch off brings the panels back where the cursor was, workspaces included.

## The PROJECT OPEN PRS group

Under the checkouts, an `OPEN PRS` group lists every pull request still open on the repo — drafts
included, sunk to the bottom of the group, dimmed and badged `draft` so they are told apart from the
ones asking for a reviewer (the `/` PALETTE lists the same rows and spells both states out, `draft` and
`ready for review`). A pull request GitHub says cannot merge — its branch conflicts with the base, or a
check is failing — is red end to end instead, arrow, title and rail, and badged `conflicts` or `failing`
in place of its state (`merge conflicts` / `checks failing` in the PALETTE), draft or not: that row needs
a person, and the red is the one the STATUS DOT wears on a session that needs someone. Conflicts win
the badge when both hold; the row goes back to its state on the refresh that finds it clean. All of it is
fetched with `gh` when you open the project, re-asked every 15 seconds once
that PROJECT has answered with at least one open pull request, and again whenever the Worktrees or
Sessions panel or the terminal window takes focus (one `gh pr list` per project, so a repo with a
hundred open PRs still costs one API call) — or at once, past every timer, when you press `Shift+R`
from any panel, which also re-reads the pull request the pane is showing. A PROJECT that answers empty — or one where `gh` is
missing, unauthenticated, or too slow to answer at all — never settles onto that beat and backs off
instead: 30 seconds to the next attempt, doubling every round to a 10-minute ceiling, so a repo with
nothing open, or a machine with no `gh` on it, stops asking all day. A call that fails outright keeps
whatever list was already on screen; one flaky round trip is no reason to blank the group. The 15-second
beat is also how rows retire: merge or close a pull request and it stops coming back, so it leaves the
list on its own, and the one under your cursor goes the moment GitHub says it's merged. Rest the cursor
on one and the right-hand pane reads it to you — description, stats and the whole conversation — without
leaving nebula; `g` opens its diff in the same viewer your worktree diffs use, `y` opens a COMMENT BOX
whose `Enter` posts what you typed on the pull request through `gh pr comment` (the pane re-reads the
conversation once it lands, and a post `gh` refused brings the box back with your text), `Enter` or a double-click
opens it in the browser, and `/` finds it by title. Press `n` — or choose **New Claude session**, **New
Codex session**, **New Cursor session**, **New Pi session** or **New Muse session** from `m` / right-click — to start a SESSION on any enabled
harness in a checkout of the pull request's head branch — the project's worktree already on that
branch, or one the DAEMON cuts for it — through the same MODEL / EFFORT submenus as the NEW SESSION
PICKER, and as directly (`Enter` on a row starts it; `p` or `e` on the row is the launch that takes a
task first), with a rule that limits all work to that PR and includes its URL: Claude and Pi get it as an appended
system prompt, Codex, Cursor and Muse as their first prompt. The URL is kept with the AGENT, so RESUME
reapplies the same scope. Only the row you actually stop on is fetched. While the cursor rests on a
pull request the Sessions column folds to its bare rule — a pull request has no checkout, so it has
no sessions to list, and the pane reading it takes the width — and opens again on the next checkout.
That fold is the row's, not yours: `Shift+S` (`hide_sessions`) is neither read nor written by it, so a
Sessions panel you collapsed stays a rail on the checkout too, chevron and all.

That checkout lists under its pull request. A worktree on an open pull request's head branch — the
one a PR SESSION or a PR-scoped AGENT PRESET works in, or one you cut with `n` and later opened a pull
request from — is not among the plain checkouts above the group but directly beneath the pull
request's row, stepped in behind a `└` that runs into its status dot, so the checkout and the pull
request it is for read as one thing and there is no guessing which worktree a review is happening
in. It is still a worktree row: the cursor on it has that checkout's sessions in the Sessions panel
(its own PR ROW among them), `n` starts a session there, `d` deletes it, and the pull request itself
is the row above. A PR SESSION's stand-in checkout goes up in the same place, so nothing jumps when
the DAEMON's real row replaces it. Move away while it is being cut — a key or a click onto another
row, panel or workspace — and you stay there: the session starts in its row, and neither the cursor
nor FOCUS is taken back to it (true of every launch, not only a pull request's). The ROOT WORKTREE
never nests, whatever branch it is on, and a branch two open pull requests share nests under the
first listed. Only a pull request on screen
takes its checkout: fold the group, or keep the draft it is out with **Hide draft PRs**, and the
checkout is a plain row again — hiding pull requests never hides work you have. The cursor follows
its checkout through every one of those moves, and through the `gh pr list` answer that first lists
the pull request (the checkout moves under it) or retires it (the checkout moves back out).

The group folds. Click its header — or pick **Show/hide open PRs** from the panel's right-click
menu — and the list drops to the one line `▸ OPEN PRS · 12`, the triangle turned sideways and the
count still honest, because `gh pr list` keeps its beat behind the fold; open, the header reads
`▾ OPEN PRS · 12` over the rows. Folding away the row the cursor is on lands it on the last checkout
and brings that checkout's session back into the pane, a checkout that sat under its pull request
rejoins the plain rows with the cursor still on it, `↑/↓` then stop at the checkouts, and `/`
still finds every pull request either way. Stepping `↓` off the last checkout into a folded group
opens it onto its first pull request rather than stopping at the header. The fold is remembered
across restarts, like the ARCHIVED toggle.

Drafts can be kept out altogether. **Draft pull requests** under Settings → Appearance
(`hide_draft_prs` in CONFIG.JSON, `shown` by default) — or **Hide draft PRs** from the panel's
right-click menu, offered whenever the list holds one — drops them from the group and from `/` alike,
and the header counts `9/12`: nine rows listed of twelve open, so the rows that are not there read as
a setting rather than a loss. It is a view, not a fetch: the list still holds every draft, so **Show
draft PRs** brings them back without a round trip, and a draft marked ready on GitHub joins the rows
on the refresh that says so (one converted back to a draft leaves on the next). A checkout on a
draft's branch keeps its row, its sessions and its own PR ROW in the SESSIONS PANEL — the toggle is
for browsing what is open, not for hiding work you have. Hiding the row the cursor is on lands it on
the nearest row left, as a fold does. The choice is remembered across restarts.

## The PROJECT ISSUES group

Under the pull requests, an `ISSUES` group lists every issue open on the repo — the ISSUES MODAL's
rows (`gh issue list`, newest first, pull requests left out), so it is there as soon as the cursor
has rested on the project, kept fresh on the modal's own beat, and counts `100+` when the answer hit
the fetch cap. Each row is `↗ #15 title`, in the green the modal paints `open` in. Rest the cursor on
one and the pane reads it the way it reads a pull request — number and title, who opened it and
when, its labels, the description as markdown and, once the cursor has rested a moment, its comments
(one `gh issue view` per row you actually stop on, remembered for the session); `PgUp`/`PgDn`,
`Home`/`End` and the wheel scroll it. The Sessions column folds to its rule meanwhile, as it does
beside a pull request. `Enter` or a double-click opens the issue in the browser, `p` is the modal's
`Enter` for it — the QUICK PROMPT carrying the issue, into the project's root checkout — `e`
launches an AGENT PRESET on it, and `m` / right-click offers the browser. The checkout verbs (`n`,
`d`, `r`, `Shift+Enter`) say there is no checkout here, as they do on a pull request.

The group folds like the one above it: click its header — or pick **Show/hide issues** from the
panel's right-click menu — and it drops to `▸ ISSUES · 12`; open, the header reads `▾ ISSUES · 12`
over the rows. Folding away the row the cursor is on lands it on the row above the header — the last
pull request, or the last checkout, whose session comes back into the pane. Stepping `↓` off that
row into a folded group opens it onto its first issue (a folded OPEN PRS group opens first, on its
own step). The fold is remembered across restarts, beside the OPEN PRS one.

## The ISSUES MODAL and ISSUE SESSIONS

`i` from any panel lists the selected PROJECT's open GitHub issues — `gh issue list`, newest first,
pull requests left out — down the left of a modal, and reads the one under the cursor on the right:
number and title, who opened it and when, its labels, the description rendered as markdown (a newline
is a line break, as GitHub shows a comment), and,
once the cursor has rested on the row for a moment, its comments (`gh issue view`, one call per issue
you actually stop on, remembered for the session). `o` opens the issue in the browser and `r` asks
GitHub again; a machine with no `gh`, or one that is not logged in, gets a line saying so in the pane
rather than an empty modal. The list is asked for before you press `i`: once the cursor has rested
on a project for a moment its open issues are fetched in the background, and re-fetched every couple
of minutes while the project stays selected (backing off when the repo has none, or `gh` can't
answer), so the modal opens on rows instead of an empty pane. A list that landed in the last thirty
seconds is what you see; an older one paints while the fresh list lands underneath — and the cursor
stays on the issue it was on, by URL, when a refresh retires a row above it.

`c` leaves a comment on the issue under the cursor without leaving the modal for long: a multi-row
box (the task prompts' shape, `Shift+Enter` for a newline) whose `Enter` posts the text as you —
`gh issue comment`, so it appears under your GitHub login — and puts the modal back on the row at
once, the pane saying the comment is on its way until GitHub answers; then the conversation is read
again with it in. `Esc`, or an empty box, puts the modal back without posting, and a post `gh`
refused (not logged in, no network) brings the box back with your text so nothing is lost.

`E` edits the issue itself without leaving the modal at all: the reading pane becomes a form on
the row's title and description — `Tab`, `↑`/`↓` or a click move between the two fields, and the
description takes `Shift+Enter` (or `Option+Enter`, or `Ctrl+J`) for a line break and `↑`/`↓` to walk
its lines, as every multi-row box does — the preset editor's prefix and postfix included.
`Enter` sends both to GitHub as one `gh issue edit` (the title on the command line, the description
on its stdin) and holds the form, its foot saying `saving…`, until GitHub answers: the row and the
pane then carry the new text at once, the list is asked for again underneath, and the footer says
`issue #15 updated`. `Esc` drops the draft and puts the reading pane back. An unchanged form closes
without a call, a blank title is refused on the spot, and a save GitHub refuses — not logged in, no
push access to the repo — keeps the form up with `gh`'s own reason on its frame and your text
intact, so nothing typed is lost. Labels, assignees and milestones stay GitHub's to edit.

Two keys put an agent on the issue. `Enter` (or `p`) opens the QUICK PROMPT for it — the same box
`p` opens anywhere, titled `Quick prompt · issue #15 (claude · opus)`, launching the `Agent` row's
harness from Settings → Agents into the selected worktree (or the PROJECT's ROOT WORKTREE when the
cursor is not on one of its checkouts). `e` opens the AGENT PRESETS list as a picker instead, and
`Enter` on a preset hands the same box back with that preset's harness, model, effort and
prefix/postfix applied. Inside the box `Tab` and `Shift+Tab` still switch the harness or the preset
and `Ctrl+N` still flips to a fresh worktree — named `issue-15-fix-login-redirect` here, the number
first and the title slugified, rather than a random name — and the issue survives every one of those
round trips. Send the box empty and the task is `Fix GitHub issue #15: <title> (<url>)`.

Either way the launch is an ISSUE SESSION. The create carries the issue's URL
(`CreateAgent::issue_url`); the DAEMON validates it, keeps it with the AGENT row beside a PR
SESSION's URL, refuses to hand the launch to a PREWARM POOL spare (which booted without it), and on
every cold spawn and RESUME composes an issue-context rule naming the URL, the checkout and its
branch — Claude and Pi receive it through `--append-system-prompt`, Codex, Cursor and Muse as the opening
of their first prompt, exactly as the PR rule travels. The harness therefore knows which issue the
session exists for before it reads your task, is told to read the issue with `gh issue view` first,
and to reference it in commits and close it from the pull request. The row it creates is an
ordinary agent from then on: auto-title, hooks, status, resume.

## The PULL REQUESTS MODAL

`v` from any panel is the ISSUES MODAL for pull requests: the selected PROJECT's open pull requests
down the left of a modal — in the OPEN PRS group's order, newest first with the drafts sunk below the
finished ones, and drafts listed even while `hide_draft_prs` keeps them out of the panel — and the one
under the cursor read on the right, as the pane reads a group row: state, checks and mergeability,
author, branches and size, the description rendered as markdown, then the conversation. A row reads
the way its group row does — a draft dimmed with a `draft` badge, one GitHub says cannot merge red
end to end with `conflicts` or `failing` — and the modal opens on the pull request the Worktrees
cursor rests on, when it rests on one.

Nothing new is asked of GitHub to paint it. The rows are the project's open list the OPEN PRS beat
already keeps warm (and remembers across launches), so the modal opens on them at once; a list older
than thirty seconds is asked for again underneath, and the cursor follows its pull request by URL
when the answer reorders the rows or retires one. The reading pane shares the pane's fetch: a pull
request read in one is read in the other, and resting on a row for a moment fetches its body (`gh pr
view`) the same way. `r` asks for the list and the row's body again now.

The keys are the ISSUES MODAL's, and the group row's. `Enter` (or `p`) opens the QUICK PROMPT for a
PR SESSION on the pull request — the box `p` opens on its group row, titled `Quick prompt · PR #42 …`
— `e` launches one of your AGENT PRESETS on it, and `n` picks a harness and starts the session bare,
`→` drilling into the MODEL / EFFORT submenus. Every one of them is the group row's launch: a
`CreatePrAgent` that runs in the project's checkout of the pull request's head branch, reused when
one is there and cut by the DAEMON otherwise, its stand-in rows up under the pull request from the
moment you launch, and the PR's URL and work rule in the harness's context. `c` (or `y`) opens the
COMMENT BOX on the pull request and comes back to the modal on the row — after `Enter` posts, after
`Esc`, and after a post `gh` refused, with your text back in the box — `g` opens the pull request's
whole diff, `o` opens it in the browser, and `Esc`, `q` or `v` closes the modal. The hotkey is
rebindable (`pull_requests`).
