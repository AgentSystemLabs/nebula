# How it works

<sub>[← README](../README.md) · [Keys](keys.md) · [Commands](commands.md) · [Sessions](sessions.md) · [Configuration](configuration.md) · [How it works](how-it-works.md)</sub>

- **Detached daemon (tmux-style).** A background `nebula` daemon owns every PTY, so agents keep running
  when the TUI closes. The TUI is a client that attaches over a unix socket (`$XDG_RUNTIME_DIR/nebula/`
  or `/tmp/nebula-<uid>/`, mode 0700). Quit the TUI, relaunch later, and your sessions are still alive
  with scrollback replayed.
- **RECENCY ORDER stamps every row.** A session is stamped when it last did anything, a worktree carries
  the newest stamp of its sessions, and a project the newest of its worktrees — which is why the lists
  sort themselves most-recent-first. The one fixed seat is the ROOT WORKTREE, always the first worktree
  row.
- **Projects → worktrees → sessions.** All work happens in the main checkout or a git worktree.
  Worktrees are real (`git worktree add/remove`), created under
  `<repo>/../<repo-name>-worktrees/<branch>`.
- **Agents boot `claude`, `codex`, or `cursor-agent`.** Creating an agent (`n`) first asks which CLI to
  run, then spawns it in the worktree. Claude's picker can also dispatch a one-shot Cloud task as
  `claude --cloud <task>`; because Claude accepts that description as a process argument, don't put
  secrets in the Cloud task. Restored agents resume with `claude --resume <session-id>` /
  `codex resume <session-id>` / `cursor-agent --resume <session-id>` (falling back to a fresh session
  when the old one is gone). A Claude AGENT created from a PROJECT OPEN PRS row also receives the PR URL
  through `--append-system-prompt`; nebula persists that URL and reapplies the constraint on every spawn.
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
- **Sessions title themselves.** Create a session with the default name and the agent renames it after
  your first prompt — a 3-4 word title describing the ask (e.g. `Fix Login Redirect`), via a
  `nebula rename <title>` command the CLI runs in its own turn (no extra API calls, no MCP server).
  Claude Code and Codex get the instruction injected through the `UserPromptSubmit` hook response — as
  `hookSpecificOutput.additionalContext`, the one envelope both read (the daemon sends it only while the
  session is untitled); Cursor gets a managed `.cursor/rules/nebula-title.mdc` project rule instead,
  since its hooks can't inject context. Titling is one-shot and never clobbers a name you typed or set
  with `r` — a late agent attempt is politely declined. `nebula rename --force` overrides.
- **Ask the agent for a worktree and it moves there.** Tell a Claude session "do this in a worktree" and
  it runs `nebula worktree <name>` instead of its own `EnterWorktree` tool (whose checkouts land under
  `<repo>/.claude/worktrees/` on a `worktree-*` branch). nebula creates the checkout in its usual
  `<repo-name>-worktrees/<branch>` spot — or takes the existing one for that branch — re-homes the
  session's row under it at once, and the moment that turn ends restarts the CLI resumed inside the
  worktree, opening with a note saying where it now runs, so the conversation carries on there without
  you typing anything. Claude learns the rule from a short `--append-system-prompt` nebula passes at
  spawn, plus a `Bash(nebula worktree:*)` permission so the command never prompts. Codex and Cursor
  sessions can run the same command; they resume silent and wait for your next prompt. The restart is
  the only way there: an agent CLI can't `cd` out of the directory it was started in.
- **Ask the agent for another session and it starts one.** Tell a Claude session "start a new nebula
  session that fixes the login redirect" and it runs `nebula spawn "<task>"`: the daemon starts a second
  agent beside it — same worktree, same harness, model and effort unless `--kind claude|codex|cursor`
  names another — opening on that task as its first prompt, so it is working before you look. The new
  row appears in the sessions list on its own (default name, so it titles itself), and the session you
  asked from is untouched: no restart, no focus change. Claude learns this from the same appended system
  prompt as the worktree rule, plus a `Bash(nebula spawn:*)` permission.
- **Everything persists in SQLite** (`~/.local/share/nebula/nebula.db` or the platform equivalent):
  projects, worktrees, agents (with kind + CLI session ids), links, workspaces, and your
  last selection.
- **Sessions warm up, then get reaped.** The daemon can pre-spawn an agent CLI while you're still naming
  the session, and pre-boot a worktree's dead sessions while your selection rests on it, so attaching
  lands on a booted screen instead of a booting shell. To bound what that costs, idle PTYs in worktrees
  no client is watching are killed after `session_idle_timeout` (5m by default) — working agents, ones
  waiting on you, and terminals with a command running are all spared, and a reaped agent
  revives on the next attach with its conversation resumed.

## Pull requests

nebula finds the pull request open on each branch with `gh` and shows it in the Sessions panel's
OPEN PRS group, including a count of comments that landed while you were away. Rest on that row and the
pane reads the pull request — description, stats, conversation — exactly as it does for the project-wide
OPEN PRS rows under the worktrees; `g` shows its diff. Manual link attachment is currently unavailable;
previously saved links remain visible so the change does not discard data.

Settings and hotkeys live in [Configuration](configuration.md). The process model, the IPC CODEC and
the crate layout are covered in more depth in [ARCHITECTURE.md](../ARCHITECTURE.md).
