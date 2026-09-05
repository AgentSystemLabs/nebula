# Configuration

<sub>[← README](../README.md) · [Keys](keys.md) · [Commands](commands.md) · [Sessions](sessions.md) · [Configuration](configuration.md) · [How it works](how-it-works.md)</sub>

CONFIG.JSON is the one settings file, in the DATA DIR beside the SQLITE STORE — hand-editable, and
what the `s` SETTINGS OVERLAY writes:

- **macOS**: `~/Library/Application Support/dev.nebula.nebula/config.json`
- **Linux**: `~/.local/share/nebula/config.json`
- `NEBULA_DATA_DIR` moves the whole directory, config included (tests, parallel instances).

Both halves of nebula read that one file. The TUI owns most keys; the DAEMON owns
`git_init_on_create`, `session_idle_timeout`, `prewarm_agents` and `prewarm_sessions`. Each side
deserializes only its own fields and ignores the rest, and both load it fresh on every use — so a
hand edit applies without restarting either. No key is required: a missing file is all defaults, an
unknown field is skipped, and a malformed file is logged and ignored rather than failing the
operation that read it. The overlay patches only the keys it knows and leaves everything else in the
JSON untouched, so hand-written fields survive a save.

Files beside it in the DATA DIR: `nebula.db` (the SQLITE STORE), `agent_presets.json` (AGENT
PRESETS), `cursor_models.json` (the cached `cursor-agent --list-models` answer, refreshed after 24h),
`ssh_hosts.json` (the SSH HOSTS FILE), `reviewed.json` (REVIEWED MARKS). All but the database are
convenience stores: missing or malformed reads as empty.

## Every setting

Twenty-six keys. **Overlay** is the SETTINGS OVERLAY tab whose row edits the key; `—` means the key
exists only in the file, so it is hand-edit-only. The Agents tab groups its rows under **Quick prompt**,
**Claude**, **Codex** and **Cursor** headers, so a harness's rows read `Enabled` / `Model` / `Effort`
under its name rather than repeating it.

| Key | Type | Default | Overlay | What it does |
|---|---|---|---|---|
| `palette_enter_attaches` | bool | `true` | General | `Enter` on a PALETTE (`/`) session attaches and focuses the TERMINAL PANE. Off, `Enter` only lands on the row in the SESSIONS PANEL and previews it; `Ctrl+o` / `Ctrl+f` still pick open / focus explicitly either way. |
| `git_init_on_create` | bool | `true` | General | DAEMON-owned: run `git init` when adding a project whose directory does not exist yet and the ADD PROJECT BROWSER (`o`) creates it. |
| `editor` | string | `"vim"` | General | The EDITOR the FILE FINDER (`f`), TREE BROWSER (`b`), find-in-files (`Shift+F`) and ⌥click launch, invoked as `<editor> +<line> <file>`. The overlay cycles `vim`, `nvim`, `nano`, `emacs`, `hx`; any command passes through verbatim, so a hand edit can name one the picker doesn't. `NEBULA_EDITOR` overrides it for the process. |
| `close_finder_on_open` | bool | `true` | General | Opening a file closes the FILE FINDER behind the editor modal, so quitting the editor is one Esc instead of two. Off leaves the results underneath. Never touches the TREE BROWSER (its editor is its own preview pane) or ⌥click. |
| `skip_session_naming` | bool | `false` | Sessions | New AGENTS launch straight from the NEW SESSION PICKER with no name prompt, taking the generated name and opting into AUTO-TITLE — exactly as accepting an empty prompt does. |
| `session_idle_timeout` | string | `"5m"` | Sessions | DAEMON-owned IDLE TIMEOUT: how long a session in a WORKTREE no client is viewing goes unwatched before the IDLE REAPER kills its PTY. See the values below. |
| `done_sound` | string | `"Glass"` | Sessions | The DONE SOUND rung when a turn reaches FINISHED: `off`, `bell` (the terminal BEL — silent in Ghostty unless its `bell-features` include `audio`), or a macOS system sound from `/System/Library/Sounds` played with `afplay` (`Glass`, `Ping`, `Pop`, `Hero`, …). Over `nebula ssh` and off macOS it is always the bell. |
| `theme` | string | `"default"` | Appearance | The THEME: `default`, `ocean`, `forest`, `rose`, `amber`. An unknown name falls back to `default`. |
| `animations` | bool | `true` | Appearance | Master switch for the STATUS SWEEP and the SPLASH's motion. Off trades them for fewer repaints on a constrained machine. |
| `show_workspaces` | bool | `true` | Appearance | Whether the WORKSPACES BAR is drawn across the top. `Shift+W` writes the key as it toggles, so a hidden bar stays hidden across restarts. |
| `hide_projects` | bool | `false` | Appearance | Hide the PROJECTS PANEL and give its width to the TERMINAL PANE (`Shift+P`). |
| `hide_worktrees` | bool | `false` | Appearance | Hide the WORKTREES PANEL (`Shift+B`), independently of `hide_projects`. |
| `quick_prompt_kind` | string | `"claude"` | Agents | Which AGENT KIND the QUICK PROMPT (`p`) launches: `claude`, `codex`, `cursor` or `pi`. Its model and effort come from that kind's own defaults below, so this is one name, not a third pair. A kind switched off here is stepped around. |
| `quick_prompt_focus` | bool | `false` | Agents | QUICK PROMPT FOCUS: whether a QUICK PROMPT launch enters and locks the new session's TERMINAL PANE. Off, its row is selected and previewed but FOCUS stays on the panel you fired from. Only the QUICK PROMPT reads it — every other launch takes the pane. |
| `claude_enabled` | bool | `true` | Agents | HARNESS TOGGLE. Off leaves Claude out of the NEW SESSION PICKER and the PR SESSION picker, and skips the standing PREWARM POOL slot; existing sessions keep attaching and resuming. The last kind left on cannot be switched off. |
| `codex_enabled` | bool | `true` | Agents | HARNESS TOGGLE for Codex, same rules. |
| `cursor_enabled` | bool | `true` | Agents | HARNESS TOGGLE for Cursor, same rules. |
| `pi_enabled` | bool | `true` | Agents | HARNESS TOGGLE for Pi, same rules. |
| `claude_model` | string | `"default"` | Agents | Default `--model` for new Claude sessions. The literal `"default"` is the sentinel meaning *don't pass the flag, let the CLI pick* — it is what you see in a fresh file, not a missing value. Overlay list: `fable`, `opus`, `sonnet`, `haiku` — unless `claude_models` below or Claude Code's own `availableModels` allowlist replaces it; any other string is passed through verbatim. |
| `claude_models` | array of strings | `[]` | — (hand-edited) | The Claude model rows every picker offers (the NEW SESSION PICKER and QUICK PROMPT submenus, the AGENTS TAB, the PRESET EDITOR) in place of the built-in aliases, verbatim, `"default"` always first: `["claude-sonnet-5", "us.anthropic.claude-opus-5-v1:0"]`. For an organization that restricts models (Claude Code refuses `--model sonnet` with *Model "sonnet" is restricted by your organization's settings. Using claude-sonnet-5 instead.*) or a provider whose ids the aliases don't reach (Bedrock, Vertex, a gateway; on Bedrock `sonnet` even means Sonnet 4.5). Empty, the list follows Claude Code's `availableModels` when one is on disk — `~/.claude/remote-settings.json` (server-managed cache), the macOS MDM profile, `managed-settings.json` and `managed-settings.d/` in the system directory, then `~/.claude/settings.json`, read once at TUI start — else the aliases. A hand edit here applies without a restart. |
| `claude_effort` | string | `"default"` | Agents | Default reasoning effort (`--effort`) for new Claude sessions: `low`, `medium`, `high`, `xhigh`, `max`, or the `"default"` sentinel. |
| `codex_model` | string | `"default"` | Agents | Default `--model` for new Codex sessions (Codex spells the flag the same way Claude does): `gpt-5.6-sol`, `gpt-5.6-terra`, `gpt-5.6-luna`, `gpt-5.5`, or the `"default"` sentinel. |
| `codex_effort` | string | `"default"` | Agents | Default `-c model_reasoning_effort=` for new Codex sessions: `minimal`, `low`, `medium`, `high`, `xhigh`, or the `"default"` sentinel. |
| `cursor_model` | string | `"default"` | Agents | Default model *family* for new Cursor sessions, from the cached catalogue (`cursor_models.json`), or the `"default"` sentinel. |
| `cursor_effort` | string | `"default"` | Agents | The effort suffix the DAEMON joins onto `cursor_model` into one flat `--model <family>-<effort>` id. The choices follow the family, so the overlay row reads `n/a` while the model is unset or has no effort variants. |
| `pi_model` | string | `"default"` | Agents | Default `--model` for new Pi sessions. Pi takes a fuzzy pattern across every provider it has credentials for, so the overlay lists families (`opus`, `sonnet`, `haiku`, `gpt-5.5`); a hand-edited `provider/id` such as `anthropic/claude-sonnet-5` passes through verbatim. |
| `pi_effort` | string | `"default"` | Agents | Default `--thinking` level for new Pi sessions: `off`, `minimal`, `low`, `medium`, `high`, `xhigh`, `max`, or the `"default"` sentinel. |
| `keybindings` | object | `{}` | Hotkeys | KEYMAP overrides, keyed by action id, valued with a comma-separated chord list: `{"git_diff": "ctrl+g, g"}`. An empty string deliberately unbinds; unknown ids are ignored. Only rows that differ from the defaults are written. |
| `prewarm_agents` | bool | `true` | — | PREWARM POOL: pre-spawn an agent CLI while you are still naming the session, so creation feels instant. **Costs one idle CLI process per warm slot** (150–300 MB each, up to 15 minutes). `false` opts out. |
| `prewarm_sessions` | bool | `true` | — | SESSION PREWARM: pre-spawn a WORKTREE's dead sessions when your selection rests on it, so attaching shows an already-booted screen instead of a booting shell. **Costs idle shell/CLI processes for sessions you may never open.** `false` opts out. |

`hide_projects` and `hide_worktrees` default to `false`. Set either to `true` to start with that panel
hidden; the SESSIONS PANEL always remains visible.

### Prewarming is hand-edit-only

`prewarm_agents` and `prewarm_sessions` have no SETTINGS OVERLAY row at all — the only way to turn
prewarming off is to write the key into CONFIG.JSON yourself:

```json
{ "prewarm_agents": false, "prewarm_sessions": false }
```

They are the two settings that cost real processes you never asked for, so they are worth knowing
about on a laptop or a small remote box. `session_idle_timeout` is what bounds their cost when they
are left on.

### What `session_idle_timeout` accepts

The overlay cycles `off`, `1m`, `5m`, `15m`, `30m`, `1h`, but the DAEMON parses more than that: any
`<n>s` / `<n>m` / `<n>h` works, and **`"off"` (or `"0"`) disables reaping entirely**. A malformed
value falls back to the 5m default, *not* to off — a typo makes reaping ordinary, not absent.

The IDLE REAPER sweeps every 15s and only takes sessions in WORKTREES no client is viewing. RUNNING
and NEEDS FEEDBACK agents, and terminals with a command running, are spared. A reaped session revives
on the next ATTACH or prewarm, and an agent RESUMES its conversation there.

## What the settings overlay owns

- **Settings live in one JSON file** (`config.json`, beside the database), read fresh on each use by both
  the daemon and the TUI, so hand edits apply without a restart. `s` opens the settings overlay over the
  same file: color theme, animations, whether the Workspaces bar, PROJECTS PANEL,
  and WORKTREES PANEL are shown,
  editor, which agent CLIs the new-session menu offers (at least one stays on) and their default model
  and reasoning effort, the idle timeout, the done sound (`done_sound`: a ding
  when a turn finishes — a macOS system sound such as `Glass`, the default; `bell` for the terminal
  bell, which Ghostty keeps silent unless its `bell-features` include `audio`; or `off`. Over
  `nebula ssh` and off macOS it is always the bell), and whether new sessions stop to ask for a
  name. `R` inside the overlay puts every setting — hotkeys included — back to its default, after a
  confirmation.
- **Every panel key is rebindable.** The overlay's Hotkeys tab lists every action and what it answers to,
  and writes overrides into the same file (`"keybindings": {"git_diff": "ctrl+g, g"}`); an empty value
  unbinds. Because nebula is always a guest inside Terminal.app / Ghostty / tmux, the tab says at bind
  time when a chord probably won't survive the trip — `⌘` anything, `^⇧` without the kitty protocol,
  `^←` on stock macOS. `Ctrl+q` is the one exception to all of it: it unlocks a terminal no matter what
  you bind, since unbinding your way out would trap you in the session.

## Logs

`daemon.log` and `tui.log` live in the state dir, which is not the DATA DIR on Linux and *is* on
macOS — the `directories` crate has no state dir there, so nebula falls back to `<DATA DIR>/state`:

- **macOS**: `~/Library/Application Support/dev.nebula.nebula/state/`
- **Linux**: `~/.local/state/nebula/`
- With `NEBULA_DATA_DIR` set, always `$NEBULA_DATA_DIR/state/`, so a test or a parallel instance keeps
  its logs beside its own data.

`NEBULA_LOG=debug` for more. No `daemon.log` at all means the DAEMON never started.

## Environment variables

Knobs worth reaching for by hand:

| Var | Default | What it does |
|---|---|---|
| `NEBULA_LOG` | — | `RUST_LOG`-style tracing filter for both the DAEMON and the TUI. |
| `NEBULA_EDITOR` | — | Editor command the file modals open, ahead of the `editor` setting. |
| `NEBULA_CLOUD_MIRROR_SECS` | `45` | CLOUD MIRROR cadence in seconds, floored at 2; `0` turns the follow off and leaves **Attach cloud session** as the manual refresh. See [Sessions](sessions.md). |

Overrides for tests and parallel instances — real, but not things a normal install needs:

| Var | Default | What it does |
|---|---|---|
| `NEBULA_RUNTIME_DIR` | `$XDG_RUNTIME_DIR/nebula`, else `/tmp/nebula-<uid>` | The RUNTIME DIR holding the DAEMON SOCKET and pidfile. |
| `NEBULA_DATA_DIR` | the platform app-support dir | The DATA DIR holding the database, config and logs. |
| `NEBULA_AGENT_CMD` | — | Replaces every agent CLI with one command line, taken verbatim (tests stand in `/bin/sh` or a stub script). |
| `NEBULA_INSTALL_URL` | the published install script | The URL `nebula upgrade` / `nebula ssh` fetch. |
| `NEBULA_UPDATE_CHECK_SECS` | `3600` | How often the TUI asks GitHub whether a newer release is published, for the FOOTER's `⇡ vX.Y.Z` indicator (one `curl` to the release page's redirect, no `gh` token); `0` turns it off. See [Keys](keys.md#chips-and-readouts). |
| `NEBULA_IDLE_REAP_MS` | `15000` | IDLE REAPER sweep period in ms. This is how often it looks, not how long a session may idle — that is `session_idle_timeout`. |
| `NEBULA_WORKTREE_SYNC_MS` | `2000` | WORKTREE SYNC probe period in ms: how often the DAEMON reconciles `git worktree list` so worktrees made outside nebula appear. |

`NEBULA_AGENT_ID`, `NEBULA_API_URL` and `NEBULA_API_TOKEN` are set *by* the DAEMON on every agent
PTY (and scrubbed from plain terminals) so hooks can reach the HOOK RECEIVER — never something you
set yourself. For all of these, empty and unset mean the same thing: use the default.
