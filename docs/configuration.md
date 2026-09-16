# Configuration

<sub>[← README](../README.md) · [Keys](keys.md) · [Commands](commands.md) · [Sessions](sessions.md) · [Configuration](configuration.md) · [How it works](how-it-works.md)</sub>

CONFIG.JSON is the settings file, in the DATA DIR beside the SQLITE STORE — hand-editable, and
what the `s` SETTINGS OVERLAY writes:

- **macOS**: `~/Library/Application Support/dev.nebula.nebula/config.json`
- **Linux**: `~/.local/share/nebula/config.json`
- `NEBULA_CONFIG_FILE` moves that one file — into a dotfiles checkout, say — and leaves everything
  else where it is. A save writes through a symlink instead of replacing it, so linking `config.json`
  into a repo works as well.
- `NEBULA_DATA_DIR` moves the whole directory, config included (tests, parallel instances).
- `nebula config path` prints where each settings file is.

CONFIG.LOCAL.JSON, beside it in the DATA DIR, has the same keys and wins over `config.json` one key at
a time. It is for what only makes sense on this machine — an editor a remote box lacks,
`prewarm_agents: false` on a small one — and it is never exported, never sent over `nebula ssh`, and
never written by an import. Create it by hand with just the keys to pin; after that the overlay
writes a key it holds back into it, so changing that setting here keeps it local, and every other key
into `config.json`:

```json
{ "editor": "nano", "prewarm_agents": false }
```

Both halves of nebula read the two files. The TUI owns most keys; the DAEMON owns
`git_init_on_create`, `worktree_base_branch`, `session_idle_timeout`, `prewarm_agents` and
`prewarm_sessions`. Each side deserializes only its own fields and ignores the rest, and both load
them fresh on every use — so a hand edit applies without restarting either. No key is required: a
missing file is all defaults, an unknown field is skipped, a value this build can't read costs only
that key (it takes its default and stays as stored in the file), and a malformed file is logged and
ignored rather than failing the operation that read it. The overlay patches only the keys it knows
and leaves everything else in the JSON untouched, so hand-written fields survive a save. See
[Compatibility rules](#compatibility-rules) for why.

Files beside it in the DATA DIR: `nebula.db` (the SQLITE STORE), `config.local.json` (above),
`agent_presets.json` (AGENT PRESETS), `cursor_models.json` (the cached `cursor-agent --list-models` answer, refreshed after 24h),
`ssh_hosts.json` (the SSH HOSTS FILE), `reviewed.json` (REVIEWED MARKS). All but the database are
convenience stores: missing or malformed reads as empty.

## Every setting

Forty-five keys. **Overlay** is the SETTINGS OVERLAY tab whose row edits the key; `—` means the key
exists only in the file, so it is hand-edit-only. Most rows toggle or cycle on `Enter` / `←` / `→`; a
*typed* row (`worktree_base_branch`) opens a one-line prompt on `Enter` instead, pre-filled with the
stored value, and an empty answer puts its default back. The Agents tab groups its rows under **Quick
prompt**, **Claude**, **Codex**, **Cursor**, **Pi** and **Muse** headers, so a harness's rows read `Enabled` / `Model` /
`Effort` under its name rather than repeating it. The **Experimental** tab holds behaviors that change
how the tree is worked; every switch there is off by default.

| Key | Type | Default | Overlay | What it does |
|---|---|---|---|---|
| `palette_enter_attaches` | bool | `true` | General | `Enter` on a PALETTE (`/`) session attaches and focuses the TERMINAL PANE. Off, `Enter` only lands on the row in the SESSIONS PANEL and previews it; `Ctrl+o` / `Ctrl+f` still pick open / focus explicitly either way. The `]` / `[` attention jump lands the same way this setting says. |
| `git_init_on_create` | bool | `true` | General | DAEMON-owned: run `git init` when adding a project whose directory does not exist yet and the ADD PROJECT BROWSER (`o`) creates it. |
| `worktree_base_branch` | string | `""` | General | DAEMON-owned WORKTREE BASE BRANCH: where every new WORKTREE nobody named a base for starts — `n` in the WORKTREES PANEL, a bare `nebula worktree`, the QUICK PROMPT's auto-created one (`nebula worktree --base` always wins). Empty, shown as `auto` in the overlay, is origin's own default branch: `origin/HEAD` freshly fetched, normally `origin/main`. A name — `master`, `develop` — is resolved the way `--base` resolves one: origin is fetched and origin's copy of that branch (`origin/master`) is the start point, untracked, never the checkout's local branch of that name, which is only as new as its last pull; a branch origin lacks that the checkout has locally is used as named. The setting is one name for every project, so a repo with no branch of that name at all does not fail the `n`: it falls back to `origin/HEAD` as if the key were empty, and `daemon.log` says which repo ignored it. A leading `origin/` is dropped (`origin/master` means `master`); a tag or SHA is not a branch and falls back too — name those with `--base`. Typed, not cycled: `Enter` on the row opens a prompt, an empty answer puts `auto` back. |
| `editor` | string | `"vim"` | General | The EDITOR the FILE FINDER (`f`), TREE BROWSER (`b`), find-in-files (`Shift+F`) and ⌥click launch, invoked as `<editor> +<line> <file>`. The overlay cycles `vim`, `nvim`, `nano`, `emacs`, `hx`; any command passes through verbatim, so a hand edit can name one the picker doesn't. `NEBULA_EDITOR` overrides it for the process. |
| `close_finder_on_open` | bool | `true` | General | Opening a file closes the FILE FINDER behind the editor modal, so quitting the editor is one Esc instead of two. Off leaves the results underneath. Never touches the TREE BROWSER (its editor is its own preview pane) or ⌥click. |
| `ssh_sync_config` | bool | `true` | General | SETTINGS SYNC: `nebula ssh` and `nebula tunnel` send this machine's `config.json` and AGENT PRESETS along, and the remote nebula merges them into its own settings before it starts — so a remote is set up the way this machine is on every connect, without reconfiguring it. Its `config.local.json` still wins there, and its projects, sessions and SSH HOSTS FILE stay its own. `--no-sync-config` leaves the settings behind for one connection. See [Backup, restore and other machines](#backup-restore-and-other-machines). |
| `skip_session_naming` | bool | `false` | Sessions | New AGENTS launch straight from the NEW SESSION PICKER with no task box, taking the generated name and AUTO-TITLE; the first prompt is typed in the CLI instead. The key predates the box, which stands where a name prompt used to. |
| `confirm_on_archive` | bool | `false` | Sessions | Put a CONFIRM DIALOG in front of archiving a session — `a` and the row menu's **Archive** alike — for when typing aimed at an agent keeps landing on the SESSIONS PANEL and archiving the session under the cursor. Off, archive is the one verb on that panel that skips the dialog `d` goes behind: it is cheap to undo with `u`, and the dialog says so. |
| `session_idle_timeout` | string | `"5m"` | Sessions | DAEMON-owned IDLE TIMEOUT: how long a session in a WORKTREE no client is viewing goes unwatched before the IDLE REAPER kills its PTY. See the values below. |
| `done_sound` | string | `"Glass"` | Sessions | The DONE SOUND rung when a turn reaches FINISHED: `off`, `bell` (the terminal BEL — silent in Ghostty unless its `bell-features` include `audio`), or a macOS system sound from `/System/Library/Sounds` played with `afplay` (`Glass`, `Ping`, `Pop`, `Hero`, …). Over `nebula ssh` and off macOS it is always the bell. |
| `feedback_sound` | string | `"Sosumi"` | Sessions | The FEEDBACK SOUND rung when a turn stops at NEEDS FEEDBACK — a permission prompt or a question — with the same values and fallbacks as `done_sound`, and a different default so red and green sound different from the next room. It never rings for the session whose pane you are locked into typing at while the terminal window has focus: that prompt is already under your hands. The one switch for the DESKTOP NOTIFICATION too: while the terminal window is in the background (from the focus reports nebula asks the terminal for — tmux needs `focus-events on`), each session that goes red is also named in a desktop notification (`osascript` on macOS, `notify-send` on Linux; never over `nebula ssh`, where the desktop is the wrong machine's; a notifier that is missing or fails is a debug line, not an error). `off` silences the sound and the notification together. |
| `theme` | string | `"default"` | Appearance | The THEME: `default`, `ocean`, `forest`, `rose`, `amber`. An unknown name falls back to `default`. |
| `animations` | bool | `true` | Appearance | Master switch for the STATUS SWEEP and the SPLASH's motion. Off trades them for fewer repaints on a constrained machine. |
| `focus_tint` | bool | `true` | Appearance | The FOCUSED PANEL TINT: a faint accent wash behind whichever panel keys land in. Off leaves every cell on the terminal's own background, so a transparency or image configured in the terminal shows through the whole window instead of stopping at the focused panel. |
| `show_workspaces` | bool | `true` | Appearance | Whether the WORKSPACES BAR is drawn across the top. `Shift+W` writes the key as it toggles, so a hidden bar stays hidden across restarts. |
| `hide_projects` | bool | `false` | Appearance | Hide the PROJECTS PANEL and give its width to the TERMINAL PANE (`Shift+P`). |
| `hide_worktrees` | bool | `false` | Appearance | Hide the WORKTREES PANEL (`Shift+B`), independently of `hide_projects`. |
| `hide_draft_prs` | bool | `false` | Appearance | Leave draft pull requests out of the PROJECT OPEN PRS GROUP and the PALETTE's (`/`) pull-request rows, so browsing what's open shows only the rows asking for a reviewer; the group's header then counts `9/12` — listed over open. A view filter, not a fetch filter: `gh pr list` still fetches the drafts and the PR CACHE still holds them, so `shown` brings them back at once and a draft marked ready joins the rows on the refresh that says so. Worktrees, their sessions and a checkout's own PR ROW in the SESSIONS PANEL are never hidden. The Worktrees panel's right-click menu flips it too (**Hide draft PRs** / **Show draft PRs**). |
| `hide_root_worktree` | bool | `false` | Experimental | Leave the ROOT WORKTREE row out of the WORKTREES PANEL, so nothing launched from that panel lands in the shared checkout. The root's sessions keep running and stay reachable from the PALETTE (`/`). Not what makes `p` on that panel cut a fresh WORKTREE — a random `<adj>-<noun>-<verb>` branch off the freshly fetched `origin/HEAD`, or the `worktree_base_branch` above, the agent started in it and the cursor moved onto the new row — that is the panel's own behaviour, on or off. |
| `recent_prompts` | bool | `false` | Experimental | RECENT PROMPTS: list the last few prompts typed into each session under its row in the SESSIONS PANEL — the text the `UserPromptSubmit` hook carried, condensed to one line — oldest first so the bottom line is the latest ask, each with a dim `30m ago` pinned right; a click on any line lands on its session. Every harness reports its prompt (Claude, Codex and Cursor in the hook payload, Pi through its managed extension). Prompts nebula composes itself — a PR SESSION's scope, the note a `nebula worktree` relocation reopens on — are left out, and archived rows list none. Off, the rows are the single pills they always were. See [Sessions](sessions.md#recent-prompts). |
| `recent_prompts_count` | integer | `3` | Experimental | How many of those prompts to list while `recent_prompts` is on. The overlay cycles `1` to `5`; a hand edit is clamped to the ten the DAEMON keeps per session (`0` reads as `1`, `50` as `10`). |
| `show_key_combos` | bool | `false` | Experimental | The KEY COMBO DISPLAY: each key pressed in the panels spelled at the bottom left of the screen — the blank row above the FOOTER — with what it did, `j - Move down`, `^d - Half page down`, `h h - Workspaces` for a double tap, so anyone watching a screen share picks the shortcuts up as they are used; vim's `showcmd`. An unbound key shows bare, so a watcher sees it did nothing. Each press replaces the last and clears itself three seconds on. What never shows: keys typed into a LOCKED PANE (they are the agent's — a password at a prompt in there stays off the screen; only the unlock hatch shows, `^q - Unlock terminal input`), and text typed into an overlay's field — inside a modal only `Esc`, `Enter`, `Tab`, the arrows and `^`/`⌥` chords show, bare. See [Keys](keys.md#chips-and-readouts). |
| `quick_prompt_kind` | string | `"claude"` | Agents | Which AGENT KIND the QUICK PROMPT (`p`) launches: `claude`, `codex`, `cursor`, `pi` or `muse`. Its model and effort come from that kind's own defaults below, so this is one name, not a third pair. A kind switched off here is stepped around. |
| `quick_prompt_focus` | bool | `false` | Agents | QUICK PROMPT FOCUS: whether a QUICK PROMPT launch enters and locks the new session's TERMINAL PANE. Off, its row is selected and previewed but FOCUS stays on the panel you fired from. Only the QUICK PROMPT reads it — every other launch takes the pane. |
| `claude_enabled` | bool | `true` | Agents | HARNESS TOGGLE. Off leaves Claude out of the NEW SESSION PICKER and the PR SESSION picker, and skips the standing PREWARM POOL slot; existing sessions keep attaching and resuming. The last kind left on cannot be switched off. |
| `codex_enabled` | bool | `true` | Agents | HARNESS TOGGLE for Codex, same rules. |
| `cursor_enabled` | bool | `true` | Agents | HARNESS TOGGLE for Cursor, same rules. |
| `pi_enabled` | bool | `true` | Agents | HARNESS TOGGLE for Pi, same rules. |
| `muse_enabled` | bool | `true` | Agents | HARNESS TOGGLE for Muse, same rules. Muse has no managed hooks yet, so its status stays process-based (running while the PTY is live, no waiting-on-you detection). |
| `hide_uninstalled_harnesses` | bool | `false` | Agents | When on, the NEW SESSION PICKER lists only enabled harnesses whose CLI is found on PATH. Off by default: a login shell can see CLIs a plain PATH lookup misses, and the daemon re-checks through the login shell at launch anyway. |
| `claude_model` | string | `"default"` | Agents | Default `--model` for new Claude sessions. The literal `"default"` is the sentinel meaning *don't pass the flag, let the CLI pick* — it is what you see in a fresh file, not a missing value. Overlay list: `fable`, `opus`, `sonnet`, `haiku` — unless `claude_models` below or Claude Code's own `availableModels` allowlist replaces it; any other string is passed through verbatim. |
| `claude_models` | array of strings | `[]` | — (hand-edited) | The Claude model rows every picker offers (the NEW SESSION PICKER and QUICK PROMPT submenus, the AGENTS TAB, the PRESET EDITOR) in place of the built-in aliases, verbatim, `"default"` always first: `["claude-sonnet-5", "us.anthropic.claude-opus-5-v1:0"]`. For an organization that restricts models (Claude Code refuses `--model sonnet` with *Model "sonnet" is restricted by your organization's settings. Using claude-sonnet-5 instead.*) or a provider whose ids the aliases don't reach (Bedrock, Vertex, a gateway; on Bedrock `sonnet` even means Sonnet 4.5). Empty, the list follows Claude Code's `availableModels` when one is on disk — `~/.claude/remote-settings.json` (server-managed cache), the macOS MDM profile, `managed-settings.json` and `managed-settings.d/` in the system directory, then `~/.claude/settings.json`, read once at TUI start — else the aliases. A hand edit here applies without a restart. |
| `claude_effort` | string | `"default"` | Agents | Default reasoning effort (`--effort`) for new Claude sessions: `low`, `medium`, `high`, `xhigh`, `max`, or the `"default"` sentinel. |
| `codex_model` | string | `"default"` | Agents | Default `--model` for new Codex sessions (Codex spells the flag the same way Claude does): `gpt-5.6-sol`, `gpt-5.6-terra`, `gpt-5.6-luna`, `gpt-5.5`, or the `"default"` sentinel. |
| `codex_effort` | string | `"default"` | Agents | Default `-c model_reasoning_effort=` for new Codex sessions: `minimal`, `low`, `medium`, `high`, `xhigh`, or the `"default"` sentinel. |
| `cursor_model` | string | `"default"` | Agents | Default model *family* for new Cursor sessions, from the cached catalogue (`cursor_models.json`), or the `"default"` sentinel. |
| `cursor_effort` | string | `"default"` | Agents | The effort suffix the DAEMON joins onto `cursor_model` into one flat `--model <family>-<effort>` id. The choices follow the family, so the overlay row reads `n/a` while the model is unset or has no effort variants. |
| `pi_model` | string | `"default"` | Agents | Default `--model` for new Pi sessions. Pi takes a fuzzy pattern across every provider it has credentials for, so the overlay lists families (`opus`, `sonnet`, `haiku`, `gpt-5.5`); a hand-edited `provider/id` such as `anthropic/claude-sonnet-5` passes through verbatim. |
| `pi_effort` | string | `"default"` | Agents | Default `--thinking` level for new Pi sessions: `off`, `minimal`, `low`, `medium`, `high`, `xhigh`, `max`, or the `"default"` sentinel. |
| `muse_model` | string | `"default"` | Agents | Default `--model` for new Muse sessions. Any model id passes through verbatim; `"default"` means don't pass the flag. |
| `muse_effort` | string | `"default"` | Agents | Reserved until the `muse` CLI documents a reasoning flag. Stored, never sent. |
| `custom_harnesses` | array | `[]` | Agents | Extra CLIs the NEW SESSION PICKER offers after the built-ins, each with its own Agents tab section (Enabled and Model rows). Each entry is `{id, program}` plus options: `label` (picker text, defaults to the id), `enabled` (default `true`), `model` (default `"default"` = the CLI's pick, else passed verbatim), `model_flag` (default `"--model"`), and `hooks` (a built-in dialect the program speaks: `claude`, `codex`, `cursor` or `pi` — with one set the sessions report status, prompts and permission waits exactly like that harness, including title sync and auto-title for `claude`; without one they stay process-based, running while the PTY is live and never waiting-on-you). Ids use lowercase letters, digits and hyphens and must not collide with a built-in. Legacy: new harnesses belong in `harnesses`, where they also gain resume, effort, system-prompt and hook-dialect rows. Invalid entries never launch — the picker hides them and the daemon refuses them with the reason. |
| `harnesses` | object | `{}` | Agents | The harness registry: per-harness deltas over the compiled-in known harnesses (Claude, Codex, Cursor, Pi, Muse), and whole new third-party CLIs. The Agents tab grows one section per entry — Enabled, Model, and Effort rows while the harness offers effort — and the `n` picker, `e` presets, spawn, resume and hooks all read the merged rows. A hand edit that breaks one entry refuses its launches with the reason, never the whole file. Run `nebula config harnesses` to print the effective rows to copy from. |
| `keybindings` | object | `{}` | Hotkeys | KEYMAP overrides, keyed by action id, valued with a comma-separated chord list: `{"git_diff": "ctrl+g, g"}`. An empty string deliberately unbinds; unknown ids are ignored. Only rows that differ from the defaults are written. |
| `prewarm_agents` | bool | `true` | Sessions | DAEMON-owned PREWARM POOL: keep one booted agent CLI standing by in the selected WORKTREE, so creating a session there adopts it and feels instant. **Costs one idle CLI process per warm slot** (150–300 MB each, up to 15 minutes), and that spare is a real session as far as the CLI is concerned — Claude's own `/list-agents` lists it beside the sessions you made, named after the directory (`my-repo-3f`), and the memory modal (`Shift+M`) groups it under **warm spares**. Off drains the pool on the DAEMON's next sweep (within 30 s). |
| `prewarm_sessions` | bool | `true` | Sessions | DAEMON-owned SESSION PREWARM: boot a WORKTREE's dead sessions when your selection rests on it, so attaching shows an already-booted screen instead of a booting shell. **Costs idle shell/CLI processes for sessions you may never open.** Off stops booting them; sessions already up stay until the IDLE REAPER takes them. |

`hide_projects` and `hide_worktrees` default to `false`. Set either to `true` to start with that panel
hidden; the SESSIONS PANEL always remains visible.

### Prewarming

`prewarm_agents` and `prewarm_sessions` are the two settings that cost real processes you never
asked for, so they are worth knowing about on a laptop or a small remote box — and worth knowing
about if your sessions talk to each other. A warm spare is a bare `claude` sitting at its prompt in
the selected worktree: Claude's `/list-agents` shows it as a peer named after the directory, the
same way it names any session you have not titled yet, so a spare beside a fresh untitled session
reads as two copies of one session (`my-repo-3f`, `my-repo-a1`), one of them forever idle. That is
the spare, not a duplicate — `Shift+M` lists it under **warm spares** with its PID. Both rows live on
the SETTINGS OVERLAY's Sessions tab (`Warm spare agent`, `Prewarm dead sessions`); the same keys in
CONFIG.JSON work by hand:

```json
{ "prewarm_agents": false, "prewarm_sessions": false }
```

Turning the pool off takes the standing spares away on the DAEMON's next sweep. `session_idle_timeout`
is what bounds the cost of both when they are left on.

### The harness registry

The five known harnesses ship compiled in, and `harnesses` edits them per field — or adds a new
CLI outright. Only your deltas go in the file; `nebula config harnesses` prints the effective rows
to copy from. Disabling one is one line, and adding a CLI is one block: picker, presets, spawn,
resume, hooks and the Agents tab section all follow, with no rebuild.

```json
{
  "harnesses": {
    "cursor": { "enabled": false },
    "agy": {
      "program": "agy",
      "model_default": "big-1",
      "resume_flag": "--resume",
      "hooks": "claude"
    }
  }
}
```

Nullable rows (`program`, `model_flag`, `permissions_flag`, `hooks`, …) clear with `null` —
`"claude": {"hooks": null}` runs Claude with process-based status and no title sync. A row that
stops making sense (an empty program, a resume flag plus a resume subcommand, an unknown dialect)
refuses its launches with the reason while every other harness keeps working. The per-harness keys
the Agents tab edits (`claude_model`, `codex_enabled`, …) keep working as a fallback wherever the
map stays silent. Omit the map entirely and you get every built-in, enabled, with its defaults —
including ones a later nebula adds. Like every other object key, a `harnesses` map in
`config.local.json` replaces the whole map from `config.json` rather than merging per harness,
so keep machine-specific overrides in one layer.

### What `session_idle_timeout` accepts

The overlay cycles `off`, `1m`, `5m`, `15m`, `30m`, `1h`, but the DAEMON parses more than that: any
`<n>s` / `<n>m` / `<n>h` works, and **`"off"` (or `"0"`) disables reaping entirely**. A malformed
value falls back to the 5m default, *not* to off — a typo makes reaping ordinary, not absent.

The IDLE REAPER sweeps every 15s and only takes sessions in WORKTREES no client is viewing. RUNNING
and NEEDS FEEDBACK agents, and terminals with a command running, are spared. A reaped session revives
on the next ATTACH or prewarm, and an agent RESUMES its conversation there.

## What the settings overlay owns

- **Settings live in one JSON file** (`config.json`, beside the database, with `config.local.json` over
  it), read fresh on each use by both the daemon and the TUI, so hand edits apply without a restart. `s` opens the settings overlay over the
  same file: color theme, animations, the FOCUSED PANEL TINT, whether the Workspaces bar,
  PROJECTS PANEL, and WORKTREES PANEL are shown,
  editor, the branch new worktrees start from (`worktree_base_branch`: `auto` for origin's default
  branch, or a name such as `master`, typed into a prompt that `Enter` opens on the row), which
  agent CLIs the new-session menu offers (at least one stays on) and their default model
  and reasoning effort, the idle timeout, whether a warm spare and a worktree's dead sessions are
  pre-booted (`prewarm_agents`, `prewarm_sessions`), the done sound (`done_sound`: a ding
  when a turn finishes — a macOS system sound such as `Glass`, the default; `bell` for the terminal
  bell, which Ghostty keeps silent unless its `bell-features` include `audio`; or `off`. Over
  `nebula ssh` and off macOS it is always the bell), the feedback sound (`feedback_sound`: the same
  choices, `Sosumi` by default, rung when a turn stops to ask you — and, while the terminal window
  is in the background, a desktop notification naming the session and its worktree; `off` silences
  both), whether new sessions stop to ask for a first
  prompt (`skip_session_naming`), and whether `a` asks before archiving one (`confirm_on_archive`, off unless you turn it
  on). `R` inside the overlay puts every setting — hotkeys included — back to its default, after a
  confirmation, and removes `config.local.json`.
- **Every panel key is rebindable.** The overlay's Hotkeys tab lists every action and what it answers to,
  and writes overrides into the same file (`"keybindings": {"git_diff": "ctrl+g, g"}`); an empty value
  unbinds. Because nebula is always a guest inside Terminal.app / Ghostty / tmux, the tab says at bind
  time when a chord probably won't survive the trip — `⌘` anything, `^⇧` without the kitty protocol,
  `^←` on stock macOS. `Ctrl+q` is the one exception to all of it: it unlocks a terminal no matter what
  you bind, since unbinding your way out would trap you in the session.

## Backup, restore and other machines

```sh
nebula config path                  # where each settings file is
nebula config export ~/backups      # write ~/backups/nebula-settings.json
nebula config import ~/backups      # merge it back in, here or on a new machine
```

A SETTINGS BUNDLE is one JSON file holding `config.json`, `agent_presets.json` and `ssh_hosts.json`,
each exactly as the file has it, under a marker key:

```json
{
  "nebula_bundle": 1,
  "exported_by": "0.27.0",
  "config": { "theme": "ocean", "keybindings": { "git_diff": "ctrl+g" } },
  "agent_presets": [{ "name": "reviewer", "kind": "claude", "prefix": "Be strict." }],
  "ssh_hosts": [{ "host": "me@box", "last_used_ms": 1789400000000 }]
}
```

- **Export** writes to stdout, to a file, or to `nebula-settings.json` inside a folder you name. It
  never carries `config.local.json`, the database, the logs or the PR and model caches.
- **Import** takes an export, a bare `config.json`, `agent_presets.json` or `ssh_hosts.json`, a folder
  holding any of them (a dotfiles folder, a copy of an old data dir), or `-` for stdin. It merges
  rather than replaces: keys the file sets replace this machine's and keys it lacks are left alone,
  presets merge by name, and hosts merge by destination, keeping the most recent. `config.local.json`
  is never written, and the summary names any imported key it still overrides. A settings file that
  exists but can't be read fails the import before anything is written. Changes apply without a
  restart.
- **Copying the file works too.** Both halves re-read `config.json` on every use, so a backup copied
  over it — or to wherever `NEBULA_CONFIG_FILE` points — applies at once, with no import and no
  restart.
- **Over ssh** (`ssh_sync_config`, on by default), `nebula ssh` and `nebula tunnel` export
  `config.json` and the presets into the one ssh command they already run, and the remote nebula
  imports them before it starts, saying what changed on stderr. Sync is one way: the machine you
  connect from wins key by key on each connect, so a setting the remote should keep goes in the
  remote's `config.local.json`. A remote nebula too old to know about the bundle ignores it and starts
  as before. The bundle travels base64-encoded in the remote command, so that machine's process list
  shows it while ssh starts. Settings hold no credentials, but a preset's prefix text travels too.
  A bundle over 64 KiB is not sent (a real one is a few KiB), and the connection goes ahead without
  it.

### Compatibility rules

Two nebula versions share these files all the time: a remote a few releases behind the machine that
connects to it, a backup restored into a newer build. Every setting follows these rules so neither
version breaks the other:

1. **Keys are only ever added.** A released key keeps its name, its type and its meaning. A new
   meaning gets a new key, and nebula keeps reading and writing the old one for older builds.
2. **Missing means default; unknown is kept.** A key a file lacks is its default. A key a build
   doesn't know is ignored when read and written back untouched on save; the overlay's `R` reset is
   the one thing that drops it.
3. **One bad value costs one key.** A value a build can't read takes its default in that build, is
   logged, and stays as stored unless that setting is changed on that machine. Preset and host
   entries work the same way: one this build can't read, such as a preset for a harness a newer
   nebula added, is kept through a save rather than deleted.
4. **Choices fall back when used, not when read.** Values like `theme` and `quick_prompt_kind` stay
   plain strings, and a name a build doesn't know falls back to the default where it is used.
5. **Old files are tested.** `crates/nebula-core/fixtures/config-<version>.json` holds every key a
   release wrote, each set to something other than its default, and the test suite checks that every
   one still loads as written and survives a save. Each release adds its own fixture.

`config.json` carries no version number. An older build could only refuse a newer file or ignore the
number, and the keys a file holds already tell a newer build everything it needs. The bundle's
`nebula_bundle` marks the file and never gates it: sections are only ever added, and a reader leaves
one it doesn't know alone.

## The project file (`.nebula.json`)

The PROJECT FILE tells nebula how a repository is run. Commit it at the repo root, beside
`package.json` or `Cargo.toml`, and every checkout of the project carries it:

```json
{
  "run": "npm run dev",
  "open": "open http://localhost:3000"
}
```

Both keys are optional shell command lines, and both run in the selected worktree's checkout:

- **`run`** — the RUN COMMAND. `r` on a checkout row in the WORKTREES PANEL starts it; `r` again stops
  it. The DAEMON runs it in a RUN TERMINAL: your login shell (`$SHELL -l -i -c`, the wrapper an agent
  launch uses, so `npm`, `bun` or `mise` resolve the way they do typed) running that line and nothing
  else. Its process is the worktree's RUNNING state — while it lives the worktree row wears a green
  `▶ running`, and the output is the `▶ run` row in the Sessions panel. Like every session it outlives
  the TUI and every client sees it, and the idle reaper never takes it. Stopping kills the whole process
  tree and removes the row. A command that exits on its own leaves its row behind, dimmed; attaching it
  replays how the run ended, and nothing but `r` runs it again — not an attach, not the prewarm sweep.
  There is one run per worktree: a second client's `r` on a worktree already running never starts
  another.
- **`open`** — the OPEN COMMAND. `Shift+Enter` on a checkout row fires it once, from the TUI rather than
  the DAEMON, since what it opens — a browser tab, an editor — belongs on the machine you are sitting
  at. It runs through `$SHELL -c` with its output discarded, and nebula does not wait for it.
  `Shift+Enter` needs the KITTY PROTOCOL (Ghostty, kitty); Terminal.app sends it as a plain `Enter`, so
  `Shift+O` is bound beside it, and the row's context menu has **Open**.

The file is read fresh at every press, so an edit applies on the next `r` or `Shift+Enter`. Nebula looks
in the worktree's own checkout first, so a branch can carry commands of its own, and falls back to the
project's main checkout, so a worktree cut before the file was committed still runs. The first file found
is the whole answer: a worktree's file with no `open` does not borrow the main checkout's. A missing
file, a missing key, or JSON that doesn't parse is a one-line footer message saying what to fix; unknown
keys are ignored.

Unlike the WORKTREE HOOKS below, which nebula runs on its own and so never takes from a checkout,
nothing in the PROJECT FILE runs until you press its key on that worktree — the same trust as typing the
command into a shell there.

## Worktree hooks

A checkout often owns things outside its own directory — a dev-server port, a Caddy or nginx route, a
docker compose project, a database — that nebula knows nothing about. WORKTREE HOOKS are two
executables of yours the DAEMON runs after it creates or deletes a worktree, so a project can provision
and release those itself. They are the one setting that is not in CONFIG.JSON: a hook is per
repository, so it lives in git config, read fresh at each use:

```sh
git config nebula.worktreeCreateHook /absolute/path/to/worktree-setup
git config nebula.worktreeDeleteHook /absolute/path/to/worktree-cleanup
```

Git resolves the key the usual way, so `git config --global` sets one script for every project and a
repo's own `.git/config` overrides it. Nebula never reads a hook from a file inside the checkout — a
committed hook would run whatever a clone brought with it, which is why git refuses working-tree hooks
too. Keep the executable outside the worktrees it serves; the delete hook runs after its checkout is
gone.

What a hook gets:

- **Two arguments**, both absolute: the main repository path, then the created or deleted worktree
  path. The value is spawned directly as an executable — no shell — so a space in either path arrives
  intact.
- **The main checkout as its working directory**, since the deleted directory no longer exists.
- **Environment**: `NEBULA_HOOK` (`worktree-create` or `worktree-delete`, so one script can serve
  both keys), `NEBULA_WORKTREE_BRANCH` and `NEBULA_WORKTREE_ID`.

When they run, and what a failure means:

- **Only after a nebula operation that succeeded.** The create hook fires once the checkout exists and
  its row is in every client — `n` in the WORKTREES PANEL, `nebula worktree`, the QUICK PROMPT's fresh
  worktree, a PR SESSION's checkout. The delete hook fires once `git worktree remove` (forced or not)
  and the row drop went through, including a checkout you had already `rm -rf`'d by hand. A delete
  that fails or is cancelled runs nothing. Worktrees created or removed outside nebula, which WORKTREE
  SYNC merely notices, run nothing either.
- **Skipped while the directory is still there.** When git had already stopped tracking a checkout
  and nebula leaves the untracked directory alone, the delete hook does not run against live files;
  the warning says so.
- **Hooks never overlap.** They run under the DAEMON's worktree lock, so a create of a path waits
  for the delete hook still releasing it, and `Shift+D`'s batch runs its hooks one after another. A
  stuck hook holds the next worktree operation for at most the timeout; keystrokes never wait on it.
- **A hook only reports.** It exits non-zero, cannot start, or runs past the timeout (30 s; then it
  and every process it started are killed) — nebula shows a one-line warning naming the hook and the
  last line it wrote to stderr, and logs the tail of its output in `daemon.log`. The worktree stays
  created or deleted, because it already was. Nothing is retried.
- **It may start something that outlives it.** A hook that launches a dev server in the background
  and exits 0 is a success the moment it exits — its output goes to a file, not a pipe, so a child
  holding it open never stalls the wait — and the server is left running. Only a timeout takes down
  what the hook started.
- **The DAEMON's environment is not a login shell.** On macOS a launchd-started daemon has a thin
  `PATH`; a script that calls `caddy` or `docker` sets its own.

A cleanup script in the spirit of the request that introduced this — release a routing entry and a
development slot keyed by the deleted path:

```sh
#!/bin/sh
# $1 = main repo, $2 = deleted worktree
export PATH="/opt/homebrew/bin:/usr/local/bin:$PATH"
name=$(basename "$2")
[ -e "$HOME/.config/dev-slots/$name" ] || exit 0
rm -f "$HOME/.config/dev-slots/$name" "/etc/caddy/sites/$name.caddy"
caddy reload --config /etc/caddy/Caddyfile
```

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
| `NEBULA_CONFIG_FILE` | `<DATA DIR>/config.json` | Moves `config.json` alone — into a dotfiles checkout, say — leaving the database, logs and `config.local.json` in the DATA DIR. Give an absolute or `~/` path. The DAEMON reads it from its own environment, so after changing it run `nebula kill` and relaunch. |

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
| `NEBULA_HOOK_TIMEOUT_MS` | `30000` | How long a WORKTREE HOOK may run before the DAEMON kills it and warns; tests shorten it. |

`NEBULA_AGENT_ID`, `NEBULA_API_URL` and `NEBULA_API_TOKEN` are set *by* the DAEMON on every agent
PTY (and scrubbed from plain terminals) so hooks can reach the HOOK RECEIVER — never something you
set yourself. `NEBULA_IMPORT_BUNDLE` is likewise set *by* `nebula ssh` and `nebula tunnel` in the
remote command: the SETTINGS BUNDLE the remote nebula merges at startup and removes from its
environment before it starts anything. For all of these, empty and unset mean the same thing: use the default.
