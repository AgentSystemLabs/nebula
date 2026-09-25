# Commands

<sub>[← README](../README.md) · [Keys](keys.md) · [Commands](commands.md) · [Sessions](sessions.md) · [Configuration](configuration.md) · [How it works](how-it-works.md)</sub>

The `nebula` CLI. Every command carries its own help — `nebula <command> --help` is the full page,
flags and examples included, and `-h` is the one-screen reminder. `nebula --version` (short `-V`)
prints the version of the binary you're running (`nebula 0.40.0`) — the same version the TUI's
FOOTER carries at its left edge (with `⇡ vX.Y.Z` beside it once a newer release is published), and
what to check after `nebula upgrade`. This page is the same surface in one place. Commands marked *(agents run this)* are the ones a coding agent invokes on
your behalf — see [How it works](how-it-works.md).

```
nebula                      open the TUI (auto-starts the daemon)
nebula add <dir>            register a git checkout as a project
nebula daemon               run the daemon that owns every session
nebula kill                 shut the running daemon down (stops all sessions)
nebula rename <title>       title the session this runs inside          (agents run this)
nebula worktree [name]      move this session into a worktree           (agents run this)
nebula spawn <task>         start another agent session beside it       (agents run this)
nebula open <file>…         show files in this nebula's file tabs       (agents run this)
nebula config <cmd>         back up, restore or locate this machine's settings
nebula browser              serve this TUI in a web browser via ttyd
nebula ssh <host>           open nebula on a remote host over ssh
nebula tunnel <host>        open a remote host's nebula in a tab here
nebula upgrade              install the latest published nebula
```

## The TUI

```sh
nebula                    # launch the TUI (auto-starts the daemon). With no project yet, a
                          # launch from inside a git repo offers it on the splash — Enter
                          # opens it; anywhere else, `o` browses for one
```

## Projects and the daemon

```sh
nebula add <dir>          # add a repo as a project, named after its root directory
nebula add .              # same, for the repo you're in (bare `nebula <dir>` / `nebula .` also work)
                          # — but a directory whose name collides with a subcommand needs the long
                          # form (`nebula add browser`) or a `./` prefix, or bare `nebula browser`
                          # serves the TUI over ttyd instead of adding the directory
nebula daemon             # run the daemon (normally auto-spawned)
nebula daemon --foreground  # daemon with logs to stdout, for debugging
nebula kill               # stop the daemon and all sessions cleanly
```

## What agents run for you

```sh
nebula rename <title>     # title the current session (agents run this; --force to retitle)
nebula worktree [name] [--base <ref>]  # move the current session into a worktree of its project,
                          # creating the branch if it's new (agents run this when you ask for a
                          # worktree; no name invents one; --base picks a new branch's start point,
                          # a branch name meaning origin's fetched copy — main is origin/main;
                          # without it the worktree_base_branch setting, else origin's default)
nebula spawn <task> [--kind <claude|codex|cursor|pi|muse|grok|opencode>]  # start a new agent session beside the current
                          # (custom harnesses launch from the TUI picker and presets, not --kind)
                          # one, in the same worktree, opening on <task> (agents run this when you
                          # ask for a new nebula session; --kind defaults to this session's harness)
nebula open <file>…       # show the files in this nebula's FILE TABS — a modal with one tab per
                          # file, the focused one previewed, Enter editing it (agents run this only
                          # when you ask to see a file; text files only — an image or any other
                          # binary is refused, and the agent names the path instead)
```

## Settings

```sh
nebula config path              # where config.json, config.local.json, agent_presets.json and
                                # ssh_hosts.json live
nebula config export [path]     # one JSON file of this machine's settings: stdout, a file, or
                                # nebula-settings.json inside a folder. Never config.local.json
nebula config import <source>   # merge a backup in: an export, a bare config.json /
                                # agent_presets.json / ssh_hosts.json, a folder holding any of
                                # them, or - for stdin. Keys it sets replace this machine's, keys
                                # it lacks stay, and config.local.json is never written
nebula config harnesses         # print the effective harness registry: every harness with
                                # the program, flags, resume shape, hook dialect and defaults a
                                # launch uses. Copy a row into config.json `harnesses` to override it
```

See [Configuration](configuration.md#backup-restore-and-other-machines).

## Other machines, other screens

```sh
nebula ssh <host> [dir]   # open nebula on a remote machine over ssh (installs it there if
                          # missing); destinations are remembered for the TUI's HOSTS PICKER
                          # (`Shift+H`). Needs the OpenSSH client (`ssh`) on PATH here. This
                          # machine's config.json and agent presets ride along, and the remote
                          # nebula merges them into its own settings (its config.local.json
                          # still wins); --no-sync-config or the ssh_sync_config setting leaves
                          # them here
nebula tunnel <host> [dir] [--port N] [--remote-port N]
                          # that host's nebula in a browser tab here, over one ssh tunnel: installs
                          # nebula there if missing, runs `nebula browser` on its loopback, forwards
                          # the port, and opens the local URL. Nothing is exposed on the remote's
                          # network — the tunnel is the only way in — so it needs no --credential.
                          # If that host already has a `nebula browser` on the port, the tunnel
                          # reuses it instead of failing on the clash (a --credential one will ask
                          # for it in the tab).
                          # Needs the OpenSSH client (`ssh`) on PATH here and ttyd on the remote;
                          # Ctrl+C takes both ends down. --port is the local end (same rules as
                          # `nebula browser`), --remote-port the far end when something there
                          # already holds that number. Settings ride along as they do for
                          # `nebula ssh` (--no-sync-config leaves them here)
nebula browser [--port N] [--bind ADDR | --public] [--credential USER:PASSWORD] [--no-open]
                          # serve this TUI in a browser tab via ttyd and open it; needs ttyd on
                          # PATH. With no --port it takes 7681 when that's free and a free port
                          # otherwise, saying which — so one per checkout can serve at once.
                          # --port 0 always picks a free one; --port N is that port or an error,
                          # which is what you want behind an ssh tunnel. Listens on 127.0.0.1
                          # unless --bind names an interface address or --public takes them all
                          # (0.0.0.0) — for a nebula on a remote box, where the access control
                          # is the firewall/security group in front of the port. That serves a
                          # live, writable terminal, so put something in front of it and use
                          # --credential to add ttyd's HTTP basic auth on top. --no-open serves
                          # without launching a desktop browser, for a box that has none
nebula upgrade            # install the latest release (--force on a dev build). Swapping the
                          # binary doesn't touch a running daemon, so afterwards it shuts an idle
                          # one (no live sessions) down for you and the next launch starts on the
                          # new binary. With sessions live it leaves the daemon up — they'd die
                          # with it — and says to run `nebula kill` when you're ready to restart.
                          # When the new build speaks another protocol, and so can't attach to
                          # that daemon, it says that too and offers the restart then and there
```
