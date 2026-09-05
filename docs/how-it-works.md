# How it works

<sub>[← README](../README.md) · [Keys](keys.md) · [Commands](commands.md) · [Sessions](sessions.md) · [Configuration](configuration.md) · [How it works](how-it-works.md)</sub>

- **Detached daemon (tmux-style).** A background `nebula` daemon owns every PTY, so agents keep running
  when the TUI closes. The TUI is a client that attaches over a unix socket (`$XDG_RUNTIME_DIR/nebula/`
  or `/tmp/nebula-<uid>/`, mode 0700). Quit the TUI, relaunch later, and your sessions are still alive
  with scrollback replayed. When the daemon swaps the process under a session you are looking at — a
  restart, or the `nebula worktree` relocation at the end of a turn — the pane is rebound to the new
  one on its own.
- **Client and DAEMON must agree on the PROTOCOL VERSION.** IPC frames are positional msgpack, so any
  change to the shared types bumps `PROTOCOL_VERSION` (`crates/nebula-core/src/protocol.rs`) and the
  handshake refuses a mismatched pair — the DAEMON answers `Incompatible`, the TUI bails, and the
  VERSION SKEW message names both binaries. Which side is stale decides the fix, and getting it
  backwards costs an afternoon: when the DAEMON is the *older* build, `nebula kill` and relaunch is the
  whole remedy (it stops every live session on the way). When the DAEMON is *ahead* of the `nebula` you
  just ran, `nebula kill` does nothing for you — a live instance respawns its DAEMON from its own
  binary, so the skew survives every restart, and the fix is to install the DAEMON's build over yours
  (`make install` from that checkout) instead. The usual shape in a checkout is a `make dev` DAEMON out
  of `target/debug` while your PATH still finds an older `nebula` from the last `make install`.
- **RECENCY ORDER stamps every row.** A session is stamped when it last did anything, a worktree carries
  the newest stamp of its sessions, and a project the newest of its worktrees — which is why the lists
  sort themselves most-recent-first. The one fixed seat is the ROOT WORKTREE, always the first worktree
  row.
- **Projects → worktrees → sessions.** All work happens in the main checkout or a git worktree.
  Worktrees are real (`git worktree add/remove`), created under
  `<repo>/../<repo-name>-worktrees/<branch>`.
- **Worktrees made outside nebula show up anyway — WORKTREE SYNC.** Every 2 s the DAEMON mtime-probes
  the git files a worktree operation touches — the repo's shared `.git/HEAD`, the `.git/worktrees`
  directory, and each linked checkout's own `HEAD` — and only when the newest of those stamps has moved
  does it spend a `git worktree list` and reconcile the rows. So an agent that runs `git worktree add`
  itself, a `git checkout` you did in another terminal, or a worktree someone removed lands in the
  panel within a couple of seconds without a restart, while an idle repo costs nothing but a few
  `stat` calls (`NEBULA_WORKTREE_SYNC_MS` overrides the 2 s beat; the e2e tests turn it down to
  100 ms). This structural sync is the *only* git polling the DAEMON does — the pull request lookups
  further down are the TUI's own.
- **Agents boot `claude`, `codex`, `cursor-agent`, or `pi`.** Creating an agent (`n`) first asks which CLI to
  run, then spawns it in the worktree. Claude's picker can also dispatch a one-shot Cloud task as
  `claude --cloud <task>`; because Claude accepts that description as a process argument, don't put
  secrets in the Cloud task. Restored agents resume with `claude --resume <session-id>` /
  `codex resume <session-id>` / `cursor-agent --resume <session-id>` (falling back to a fresh session
  when the old one is gone) / `pi --session-id <session-id>` (which creates a missing id instead of
  dying). An AGENT created from a PROJECT OPEN PRS row also receives the PR URL and a PR-only
  work rule — Claude and Pi through `--append-system-prompt` on every spawn, Codex and Cursor as the first prompt of
  their cold spawn (their transcripts carry it through a resume); nebula persists that URL.
- **Status via agent-CLI hooks, not MCP.** At agent spawn, nebula merges managed hooks into the
  worktree's `.claude/settings.local.json` (Claude Code) or `.cursor/hooks.json` (Cursor CLI), and into
  `~/.codex/hooks.json` (Codex — codex records hook approvals against the hook file's path, so a
  per-worktree file would re-prompt forever; from its home, you approve nebula's hooks once at codex's
  "Hooks need review" prompt and every later worktree is silent). Groups are tagged `_nebulaManaged`,
  user hooks preserved, rebuilt each spawn. Each hook is a fail-soft curl to the daemon's loopback HTTP
  endpoint, authenticated with a per-boot bearer token injected into the agent's environment only.
- **…plus the progress bar, for the cancel no hook reports.** Escaping out of a turn fires no `Stop` and
  suppresses the idle notification that normally un-sticks one, so nebula also reads the CLI's terminal
  progress-bar escapes (OSC 9;4) straight off the PTY. That signal survives a cancel, and it stays busy
  while a permission prompt is open — so it can't mark an agent done while it is actually waiting on you.
- **…and the IDLE PROMPT, which is a hold rather than a finish.** Claude posts a
  `Notification{idle_prompt}` after roughly 60 s parked at the input box with nobody touching the
  keyboard, and that is the notification which un-sticks a turn that ended without a `Stop` — a
  rejected prompt, an escape mid-turn. Since Claude Code 2.1 the Agent tool runs subagents in the
  *background*, so it also fires while workers are still going: the foreground turn ended and the input
  box came back, but the session is anything but done. So nebula treats an IDLE PROMPT as a hold —
  exactly the hold a gated `Stop` gets — whenever any subagent is still tracked, and only a set that
  has gone quiet is ever presumed orphaned and finished on the strength of it.
- **The STOP GATE's four graces, on a 30 s tick.** A `Stop` (or an IDLE PROMPT) is held while
  `SubagentStart`s outnumber `SubagentStop`s, and a recheck every 30 s — fixed in the DAEMON, with no
  knob to turn it down — decides what becomes of the hold. Once the set drains and stays empty for
  180 s the session is finished; a `SubagentStart` that lands within 30 s of a finish instead heals it
  back to running, on the reading that the `Stop` raced that subagent's own POST. When the set never
  drains, a subagent that has shown no sign of life for 30 min — no `SubagentStart`/`SubagentStop`, no
  subagent tool traffic — is presumed killed and the turn finishes anyway. That last grace is why a
  session whose worker died can sit yellow far longer than you expect, and it is generous on purpose:
  one silent `cargo test` can run for many minutes, and a wrong green is the bug it exists to prevent.
  An individually tracked subagent older than 2 h is dropped from the set outright.
- **Which of those signals you get depends on the harness.** Claude is installed with all ten hook
  groups — `UserPromptSubmit`, `Stop`, `SessionStart`, `PermissionRequest`, `Notification`, a
  `PreToolUse` and a `PostToolUse` on `AskUserQuestion`, and a `PostToolUse` on
  `Bash|EnterWorktree|ExitWorktree` so a session that moves re-homes its row seconds later instead of
  at the turn's `Stop`, plus `SubagentStart` and `SubagentStop`. Codex gets six of them: no
  `Notification` and neither `*ToolUse` group, because it has no `AskUserQuestion` tool and its native
  `PermissionRequest` already covers waiting on you. Cursor gets five camelCase events —
  `sessionStart`, `beforeSubmitPrompt`, `stop`, `subagentStart`, `subagentStop` — and no permission
  event at all; nebula runs `cursor-agent --force`, so waiting-on-you is simply not detectable there
  and a Cursor session never reaches NEEDS FEEDBACK, only busy or idle. Pi runs TypeScript extensions
  instead of shell hooks, so nebula writes one managed extension into its global agent dir
  (`~/.pi/agent/extensions/nebula.ts`, or `$PI_CODING_AGENT_DIR/extensions/` — global because pi loads
  those without the trust prompt a per-project `.pi/extensions/` raises) that maps pi's events onto the
  same names: `session_start` → `SessionStart`, `before_agent_start` → `UserPromptSubmit`,
  `agent_end` → `Stop` (it fires on an abort too, so a cancelled pi turn goes green on its own), the
  `ask_question` tool's start and end → `PreToolUse` / `PostToolUse`, and a blocking extension prompt
  mid-run → `PermissionRequest`. The file is env-guarded, so a `pi` you run outside nebula loads it and
  does nothing.
- **Sessions title themselves.** Create a session with the default name and the agent renames it after
  your first prompt — a 3-4 word title describing the ask (e.g. `Fix Login Redirect`), via a
  `nebula rename <title>` command the CLI runs in its own turn (no extra API calls, no MCP server).
  Claude Code and Codex get the instruction injected through the `UserPromptSubmit` hook response — as
  `hookSpecificOutput.additionalContext`, the one envelope both read (the daemon sends it only while the
  session is untitled) — Pi's extension reads the same envelope and appends it to that run's system
  prompt; Cursor gets a managed `.cursor/rules/nebula-title.mdc` project rule instead,
  since its hooks can't inject context. Titling is one-shot and never clobbers a name you typed or set
  with `r` — a late agent attempt is politely declined. `nebula rename --force` overrides.
- **A Claude session's own name and its row stay tied.** `/rename <name>` inside Claude Code retitles
  the row within a moment — the same name then shows in the SESSIONS PANEL, Claude's prompt box, its
  `/resume` picker and `/rc` list, and survives a restart. Claude fires no hook for `/rename`; it
  rewrites the window title (`✳ <name>`) and writes `custom-title.json` beside the transcript, so the
  DAEMON reads that file when the PTY's title changes (and on every hook), and adopts a title Claude
  did not hold before as if you had pressed `r`. The other way round, a name set in nebula — typed at
  creation, set with `r`, or chosen by AUTO-TITLE — reaches Claude on your next prompt through the
  same `UserPromptSubmit` hook reply, as `hookSpecificOutput.sessionTitle`. Whichever side changed
  last wins; a name you set in nebula is never undone by re-reading Claude's older one. Claude only —
  Codex and Cursor have no session name of their own.
- **Ask the agent for a worktree and it moves there.** Tell a Claude session "do this in a worktree" and
  it runs `nebula worktree <name>` instead of its own `EnterWorktree` tool (whose checkouts land under
  `<repo>/.claude/worktrees/` on a `worktree-*` branch). nebula creates the checkout in its usual
  `<repo-name>-worktrees/<branch>` spot — or takes the existing one for that branch — re-homes the
  session's row under it at once, and the moment that turn ends restarts the CLI resumed inside the
  worktree, opening with a note saying where it now runs, so the conversation carries on there without
  you typing anything. Claude learns the rule from a short `--append-system-prompt` nebula passes at
  spawn, plus a `Bash(nebula worktree:*)` permission so the command never prompts; Pi gets the same
  appended prompt and reopens on the same note. Codex and Cursor
  sessions can run the same command; they resume silent and wait for your next prompt. The restart is
  the only way there: an agent CLI can't `cd` out of the directory it was started in.
- **Ask the agent for another session and it starts one.** Tell a Claude session "start a new nebula
  session that fixes the login redirect" and it runs `nebula spawn "<task>"`: the daemon starts a second
  agent beside it — same worktree, same harness, model and effort unless `--kind claude|codex|cursor|pi`
  names another — opening on that task as its first prompt, so it is working before you look. The new
  row appears in the sessions list on its own (default name, so it titles itself), and the session you
  asked from is untouched: no restart, no focus change. Claude learns this from the same appended system
  prompt as the worktree rule, plus a `Bash(nebula spawn:*)` permission.
- **Ask the agent to show you a file and it opens in nebula.** Say "open it" or "show me the examples"
  and the session runs `nebula open <file>…`; every TUI attached to the daemon raises its file tabs on
  them — a modal with one tab per file, the focused one previewed with syntax highlighting, `Enter`
  editing it in place — so the agent puts the file in front of you instead of pasting it into the
  reply. The CLI resolves the paths against the session's own directory and refuses a path that isn't
  there; the daemon only checks the caller is a known session and passes the agent's checkout along as
  the editor's working directory. Same appended prompt, plus a `Bash(nebula open:*)` permission.
- **Everything persists in SQLite** (`~/.local/share/nebula/nebula.db` or the platform equivalent):
  projects, worktrees, agents (with kind + CLI session ids), links, workspaces, and your
  last selection.
- **Sessions warm up, then get reaped.** The daemon can pre-spawn an agent CLI while you're still naming
  the session, and pre-boot a worktree's dead sessions while your selection rests on it, so attaching
  lands on a booted screen instead of a booting shell. To bound what that costs, idle PTYs in worktrees
  no client is watching are killed after `session_idle_timeout` (5m by default) — working agents, ones
  waiting on you, and terminals with a command running are all spared, and a reaped agent
  revives on the next attach with its conversation resumed. Both halves of the PREWARM POOL are
  switchable — `prewarm_agents` and `prewarm_sessions` in CONFIG.JSON, `true` by default and
  hand-edit-only, since neither has a SETTINGS OVERLAY row (see [Configuration](configuration.md)) —
  and a warm spare nobody claims inside 15 min is reaped on its own, because it holds real memory and
  its context goes stale. The IDLE REAPER's check is a 15 s sweep (`NEBULA_IDLE_REAP_MS`), so the real
  latency is the timeout plus up to 15 s more; `session_idle_timeout` also takes `"off"`, which
  switches reaping off entirely.

## Pull requests

nebula finds the pull request open on each branch with `gh` and shows it in the Sessions panel's
OPEN PRS group, including a count of comments that landed while you were away; once that pull request
is merged or closed the row goes (a draft stays, dimmed and badged `draft`). Rest on that row and the
pane reads the pull request — description, stats, conversation — exactly as it does for the project-wide
OPEN PRS rows under the worktrees; `g` shows its diff. Manual link attachment is currently unavailable;
previously saved links remain visible so the change does not discard data.

This is the one part of nebula the TUI asks for itself rather than the DAEMON: every `gh pr view`,
`gh pr list` and `gh pr diff` is spawned by the client, which is why the lookups stop the moment you
quit, and why a machine with no `gh` — or one that is unauthenticated, or pointed at a checkout with no
remote — just shows no rows instead of an error. Only what you are looking at is ever asked about: the
selected worktree's PR ROW and the selected project's PROJECT OPEN PRS GROUP, one process each, never
stacked while one is in flight, each abandoned after 20 s. A repo that answers settles onto a steady
15 s beat; an empty answer backs off by doubling — out to 3 min for a branch that never grows a PR,
10 min for a project with none open — so a workspace of thirty repos does not cost thirty API calls a
beat.

Settings and hotkeys live in [Configuration](configuration.md). The process model, the IPC CODEC and
the crate layout are covered in more depth in [ARCHITECTURE.md](../ARCHITECTURE.md).
