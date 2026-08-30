# Configuration

<sub>[← README](../README.md) · [Keys](keys.md) · [Commands](commands.md) · [Sessions](sessions.md) · [Configuration](configuration.md) · [How it works](how-it-works.md)</sub>

Settings: `~/.local/share/nebula/config.json` (or the platform equivalent), beside the database —
hand-editable, and what the `s` overlay writes.

`hide_projects` and `hide_worktrees` default to `false`. Set either to `true` to start with that panel
hidden; the SESSIONS PANEL always remains visible.

Logs: `~/.local/state/nebula/daemon.log` and `tui.log` (`NEBULA_LOG=debug` for more). `NEBULA_EDITOR`
overrides the configured editor. Overrides for tests/parallel instances: `NEBULA_RUNTIME_DIR`,
`NEBULA_DATA_DIR`, `NEBULA_AGENT_CMD`, `NEBULA_INSTALL_URL`.

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
