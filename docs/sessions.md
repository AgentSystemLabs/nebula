# Sessions

<sub>[← README](../README.md) · [Keys](keys.md) · [Commands](commands.md) · [Sessions](sessions.md) · [Configuration](configuration.md) · [How it works](how-it-works.md)</sub>

Everything that can start an AGENT, and what each launch path does differently.

## The NEW SESSION PICKER

With a WORKTREE selected, press `n` in the SESSIONS PANEL. A menu asks what to
run — **Claude**, **Codex**, or **Cursor** (a plain shell is `t` — see [Keys](keys.md)); a CLI you never use can be
switched off on the settings overlay's Agents tab and drops out of the menu entirely. `→` on any row drills
into model and reasoning-effort submenus (Cursor's model is a family such as `claude-opus-5-thinking`, and
its effort list follows the family, `-fast` variants included — `cursor-agent --list-models` bakes both
into the id, so nebula launches `--model claude-opus-5-thinking-high-fast`; the list is a built-in seed
merged with `--list-models`, cached in `cursor_models.json` beside `config.json` and refreshed daily);
In those submenus you type to filter — `opus` narrows the rows to the Opus families, `↑`/`↓` move, `Backspace` widens, `Esc` clears — and the preset editor's Harness / Model / Effort rows take the same type-ahead.
`Enter` anywhere takes your configured defaults. On the
Claude row, `Tab` toggles Cloud mode: after the optional name, enter the task in the wrapped editor
(`Shift+Enter` or `Ctrl+J` adds a line) and nebula launches `claude --cloud <task>`. On accounts without
Claude's live-attach rollout the CLI prints the session URL and exits — so nebula reads the session id off
that output and re-enters the session for you, without being asked. The row becomes a **mirror** of the
cloud session: nebula runs `claude --cloud <id>`, falling back to `claude --teleport <id>` (the transcript
and branch pulled into a local session) when the account can't attach, and then re-teleports every 45s so
turns the cloud agent takes keep landing in the pane. The badge reads `cloud ↻` while it is following.
Since either CLI switches the checkout to the cloud branch, a row still in the main checkout is first
re-homed into a `cloud-<id>` worktree of its own.

A teleport is a snapshot, not a live link, which is why the mirror re-pulls — and why **the first key you
type into the pane ends it**: from then on the session is yours, an ordinary local Claude that started from
a cloud transcript, and nebula stops respawning it under you. `NEBULA_CLOUD_MIRROR_SECS` changes the
cadence; `0` turns the follow off, leaving **Attach cloud session** (the row's `m` menu) as the manual
refresh. To steer the cloud agent without a browser, pick **Send to cloud session** — the same wrapped
editor — and nebula runs `claude -p <message> --cloud <id>` and pulls the transcript straight after. The
reply shows up on a later refresh; the CLI never returns one. Otherwise, name the session or accept the
default and nebula spawns the CLI in that worktree and drops you straight into it.

## AGENT PRESETS

If you keep starting the same kind of session with the same framing, save it as an **agent preset**:
`e` in the Sessions column lists them, `a` opens a small form — name, harness, model, effort, and an
optional prefix and postfix — and `e` / `d` edit or delete. `Enter` on a preset asks for the task in the
same wrapped editor, then launches the CLI with `prefix + task + postfix` as its very first prompt, so
the agent is already working when the pane opens. The row it creates is an ordinary session: it names
itself on that first turn, resumes, and shows status like any other. Presets live in
`agent_presets.json` beside `config.json`.

## The PROJECT OPEN PRS group

Under the checkouts, an `OPEN PRS` group lists every pull request still open on the repo — drafts
included, badged as such — fetched with `gh` when you open the project, re-asked every 15 seconds, and
again whenever the Worktrees or Sessions panel or the terminal window takes focus (one `gh pr list` per
project, so a repo with a hundred open PRs still costs one API call). That beat is
also how rows retire: merge or close a pull request and it stops coming back, so it leaves the list on
its own, and the one under your cursor goes the moment GitHub says it's merged. Rest the cursor on one
and the right-hand pane reads it to you — description, stats and the whole conversation — without
leaving nebula; `g` opens its diff in the same viewer your worktree diffs use, `Enter` or a double-click
opens it in the browser, and `/` finds it by title. Press `n` — or choose **New Claude session** from
`m` / right-click — to start a Claude SESSION in the PROJECT's ROOT WORKTREE with an injected system
prompt that limits all work to that PR and includes its URL. The URL is kept with the AGENT, so RESUME
reapplies the same scope. Only the row you actually stop on is fetched.
