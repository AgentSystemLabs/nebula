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
(`Shift+Enter` or `Ctrl+J` adds a line) and nebula launches `claude --cloud=<task>` — the value binds
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
one. Otherwise the picker ends in the same task
box `p` opens (the QUICK PROMPT — see [Keys](keys.md)): type the agent's first prompt and `Enter`, or
`Enter` on the empty box to start with none and type it in the CLI, and nebula spawns the CLI in that
worktree with it and drops you straight into it. The session titles itself from that first prompt
(AUTO-TITLE); `r` renames it whenever you like. **Skip starting prompt** (Settings → Sessions) launches
straight from the picker instead.

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
`e` in the Sessions column lists them, `a` opens a small form — name, harness, model, effort, an
optional prefix and postfix, and **Task** (`ask` or `skip`) — and `e` / `d` edit or delete. `Enter` on a
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
pane opens. A `skip`-task preset launches straight from the picker.

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

## The PROJECT OPEN PRS group

Under the checkouts, an `OPEN PRS` group lists every pull request still open on the repo — drafts
included, sunk to the bottom of the group, dimmed and badged `draft` so they are told apart from the
ones asking for a reviewer (the `/` PALETTE lists the same rows and spells both states out, `draft` and
`ready for review`) — fetched with `gh` when you open the project, re-asked every 15 seconds once
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
PICKER, with a rule that limits all work to that PR and includes its URL: Claude and Pi get it as an appended
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
the DAEMON's real row replaces it. The ROOT WORKTREE never nests, whatever branch it is on, and a
branch two open pull requests share nests under the first listed. Only a pull request on screen
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
description takes `Shift+Enter` (or `Ctrl+J`) for a line break, as every multi-row box does.
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
