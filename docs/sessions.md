# Sessions

<sub>[← README](../README.md) · [Keys](keys.md) · [Commands](commands.md) · [Sessions](sessions.md) · [Configuration](configuration.md) · [How it works](how-it-works.md)</sub>

Everything that can start an AGENT, and what each launch path does differently.

## The NEW SESSION PICKER

With a WORKTREE selected, press `n` in the SESSIONS PANEL. A menu asks what to
run — **Claude**, **Codex**, **Cursor**, or **Pi** (a plain shell is `t` — see [Keys](keys.md)); a CLI you never use can be
switched off on the settings overlay's Agents tab and drops out of the menu entirely. `→` on any row drills
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
permission gate to begin with — nebula passes no flag, and it runs its tools as it sees fit. Pick the
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
tool waits on you, and green when the run ends, a cancelled run included.

## AGENT PRESETS

If you keep starting the same kind of session with the same framing, save it as an **agent preset**:
`e` in the Sessions column lists them, `a` opens a small form — name, harness, model, effort, and an
optional prefix and postfix — and `e` / `d` edit or delete. `Enter` on a preset asks for the task in the
same wrapped editor, then launches the CLI with `prefix + task + postfix` as its very first prompt, so
the agent is already working when the pane opens. The row it creates is an ordinary session: it names
itself on that first turn, resumes, and shows status like any other. Presets live in
`agent_presets.json` beside `config.json`.

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
leaving nebula; `g` opens its diff in the same viewer your worktree diffs use, `Enter` or a double-click
opens it in the browser, and `/` finds it by title. Press `n` — or choose **New Claude session**, **New
Codex session** or **New Cursor session** from `m` / right-click — to start a SESSION on any enabled
harness in the PROJECT's ROOT WORKTREE, through the same MODEL / EFFORT submenus as the NEW SESSION
PICKER, with a rule that limits all work to that PR and includes its URL: Claude gets it as an appended
system prompt, Codex and Cursor as their first prompt. The URL is kept with the AGENT, so RESUME
reapplies the same scope. Only the row you actually stop on is fetched.

The group folds. Click its header — or pick **Show/hide open PRs** from the panel's right-click
menu — and the list drops to the one line `▸ OPEN PRS · 12`, the triangle turned sideways and the
count still honest, because `gh pr list` keeps its beat behind the fold; open, the header reads
`▾ OPEN PRS · 12` over the rows. Folding away the row the cursor is on lands it on the last checkout
and brings that checkout's session back into the pane, `↑/↓` then stop at the checkouts, and `/`
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
number and title, who opened it and when, its labels, the description as plain wrapped text, and,
once the cursor has rested on the row for a moment, its comments (`gh issue view`, one call per issue
you actually stop on, remembered for the session). `o` opens the issue in the browser and `r` asks
GitHub again; a machine with no `gh`, or one that is not logged in, gets a line saying so in the pane
rather than an empty modal. The rows are kept for the session, so reopening the modal paints at
once while the fresh list lands underneath — and the cursor stays on the issue it was on, by URL,
when a refresh retires a row above it.

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
branch — Claude and Pi receive it through `--append-system-prompt`, Codex and Cursor as the opening
of their first prompt, exactly as the PR rule travels. The harness therefore knows which issue the
session exists for before it reads your task, is told to read the issue with `gh issue view` first,
and to reference it in commits and close it from the pull request. The row it creates is an
ordinary agent from then on: auto-title, hooks, status, resume.
