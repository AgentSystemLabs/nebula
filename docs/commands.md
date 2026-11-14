# Commands

<sub>[← README](../README.md) · [Keys](keys.md) · [Commands](commands.md) · [Sessions](sessions.md) · [Configuration](configuration.md) · [How it works](how-it-works.md)</sub>

The `orbit` CLI. Every command carries its own help — `orbit <command> --help` is the full page,
flags and examples included, and `-h` is the one-screen reminder. `orbit --version` (short `-V`)
prints the version of the binary you're running (`orbit 1.0.0`) — the same version the TUI's
FOOTER carries at its left edge (with `⇡ vX.Y.Z` beside it once a newer release is published), and
what to check after `orbit upgrade`. This page is the same surface in one place. Commands marked *(agents run this)* are the ones a coding agent invokes on
your behalf — see [How it works](how-it-works.md).

```
orbit                      open the TUI (auto-starts the daemon)
orbit add <dir>            register a git checkout as a project
orbit daemon               run the daemon that owns every session
orbit kill                 shut the running daemon down (stops all sessions)
orbit reload               move the daemon onto the installed binary, keeping every session
orbit rename <title>       title the session this runs inside          (agents run this)
orbit worktree [name]      move this session into a worktree           (agents run this)
orbit spawn <task>         start another agent session beside it       (agents run this)
orbit open <file>…         show files/images/diagrams in file tabs     (agents run this)
orbit sessions             list every session across every project     (agents run this)
orbit read <session>       read recent context from another session    (agents run this)
orbit ask <session> <q>    ask another session and print its answer    (agents run this)
orbit config <cmd>         back up, restore or locate this machine's settings
orbit browser              serve this TUI in a web browser via ttyd
orbit ssh <host>           open orbit on a remote host over ssh
orbit tunnel <host>        open a remote host's orbit in a tab here
orbit sbx [name]           open orbit inside a local Docker sandbox
orbit upgrade              install the latest published orbit
orbit doctor [--fix]       check this machine's setup; --fix installs graphify and agent-browser
```

## The TUI

```sh
orbit                    # launch the TUI (auto-starts the daemon). With no project yet, a
                          # launch from inside a git repo offers it on the splash — Enter
                          # opens it; anywhere else, `o` browses for one
```

## Projects and the daemon

```sh
orbit add <dir>          # add a repo as a project, named after its root directory
orbit add .              # same, for the repo you're in (bare `orbit <dir>` / `orbit .` also work)
                          # — but a directory whose name collides with a subcommand needs the long
                          # form (`orbit add browser`) or a `./` prefix, or bare `orbit browser`
                          # serves the TUI over ttyd instead of adding the directory
                          # If an existing project's old folder has gone missing, use the TUI's
                          # Locate project prompt on that project to relink it; `orbit add` still
                          # registers the path as a project and does not guess which stranded row
                          # should move.
orbit daemon             # run the daemon (normally auto-spawned)
orbit daemon --foreground  # daemon with logs to stdout, for debugging
orbit kill               # stop the daemon and all sessions cleanly
orbit reload             # restart the daemon in place onto the binary this runs from: every
                          # session keeps its process, scrollback and status, and agents' hooks
                          # keep reaching it. Open TUIs drop their connection — relaunch `orbit`
                          # to pick the sessions back up. The cut-over after `make install`; a
                          # daemon from before reload existed can only be restarted by `kill`
```

## What agents run for you

```sh
orbit rename <title>     # title the current session (agents run this; --force to retitle)
orbit worktree [name] [--base <ref>]  # move the current session into a worktree of its project,
                          # creating the branch if it's new (agents run this when you ask for a
                          # worktree; no name invents one; --base picks a new branch's start point,
                          # a branch name meaning origin's fetched copy — main is origin/main —
                          # but is ignored when the branch already exists; without it the
                          # worktree_base_branch setting applies, else origin's default)
orbit spawn <task> [--kind <claude|codex|cursor|pi|muse|grok|opencode>] [--worktree <branch> [--base <ref>]]
                          # start a new agent session beside the current one, in the same worktree,
                          # opening on <task> (agents run this when you ask for a new orbit session;
                          # --kind defaults to this session's harness; custom harnesses launch from
                          # the TUI picker and presets, not --kind); --worktree starts it in the
                          # project's worktree on <branch> instead, creating it when the branch has
                          # none (--base picks a new branch's start point and is refused when the
                          # branch already exists), and leaves this session where it is
orbit open <file>…       # show the files in this orbit's FILE TABS — a modal with one tab per
                          # file, the focused one previewed, Enter editing it (agents run this only
                          # when you ask to see a file, image, mockup or diagram; text files,
                          # supported local images and Mermaid diagrams render inline, other
                          # binaries are refused and the agent names the path instead)
orbit sessions [--json]  # list every session across every project: id, name, project, worktree
                          # branch/path, harness, status and one-line recent summary. Agents run
                          # this when you name another session and they need an exact id.
orbit read <session> [--lines N]
                          # print clean recent context from another session, resolving by id or
                          # unambiguous fuzzy name. Claude transcript text is preferred; otherwise
                          # the daemon strips ANSI from bounded PTY scrollback.
orbit ask <session> "<question>" [--timeout S] [--no-wait]
                          # send a labelled question into another local agent, wait for that turn
                          # to finish, and print the answer. A busy target is queued until idle by
                          # default; --no-wait fails fast instead.
```

## Settings

```sh
orbit config path              # where config.json, config.local.json, agent_presets.json and
                                # ssh_hosts.json live
orbit config export [path]     # one JSON file of this machine's settings: stdout, a file, or
                                # orbit-settings.json inside a folder. Never config.local.json
orbit config import <source>   # merge a backup in: an export, a bare config.json /
                                # agent_presets.json / ssh_hosts.json, a folder holding any of
                                # them, or - for stdin. Keys it sets replace this machine's, keys
                                # it lacks stay, and config.local.json is never written
orbit config harnesses         # print the effective harness registry: every harness with
                                # the program, flags, resume shape, hook dialect and defaults a
                                # launch uses. Copy a row into config.json `harnesses` to override it
```

See [Configuration](configuration.md#backup-restore-and-other-machines).

## Other machines, other screens

```sh
orbit ssh <host> [dir]   # open orbit on a remote machine over ssh (installs it there if
                          # missing); destinations are remembered for the TUI's HOSTS PICKER
                          # (`Shift+H`). Needs the OpenSSH client (`ssh`) on PATH here. This
                          # machine's config.json and agent presets ride along, and the remote
                          # orbit merges them into its own settings (its config.local.json
                          # still wins); --no-sync-config or the ssh_sync_config setting leaves
                          # them here
orbit tunnel <host> [dir] [--port N] [--remote-port N]
                          # that host's orbit in a browser tab here, over one ssh tunnel: installs
                          # orbit there if missing, runs `orbit browser` on its loopback, forwards
                          # the port, and opens the local URL. Nothing is exposed on the remote's
                          # network — the tunnel is the only way in — so it needs no --credential.
                          # If that host already has a `orbit browser` on the port, the tunnel
                          # reuses it instead of failing on the clash (a --credential one will ask
                          # for it in the tab).
                          # Needs the OpenSSH client (`ssh`) on PATH here and ttyd on the remote;
                          # Ctrl+C takes both ends down. --port is the local end (same rules as
                          # `orbit browser`), --remote-port the far end when something there
                          # already holds that number. Settings ride along as they do for
                          # `orbit ssh` (--no-sync-config leaves them here)
orbit sbx [name] [--image IMAGE]
                          # orbit inside a local Docker container, so agents never see this
                          # machine's files: creates `orbit-sbx` (or `orbit-sbx-<name>`) from
                          # buildpack-deps:bookworm-scm on first use, starts it when stopped, and
                          # gets in the way `orbit ssh` does — the same self-installing script,
                          # over `docker exec` instead of ssh. The container is kept, daemon,
                          # sessions and clones included; `docker rm -f orbit-sbx` discards it.
                          # --image only applies when the container is created. Needs `docker`
                          # on PATH; settings ride along as they do for `orbit ssh`
orbit browser [--port N] [--bind ADDR | --public] [--credential USER:PASSWORD] [--no-open]
                          # serve this TUI in a browser tab via ttyd and open it; needs ttyd on
                          # PATH. With no --port it takes 7681 when that's free and a free port
                          # otherwise, saying which — so one per checkout can serve at once.
                          # --port 0 always picks a free one; --port N is that port or an error,
                          # which is what you want behind an ssh tunnel. Listens on 127.0.0.1
                          # unless --bind names an interface address or --public takes them all
                          # (0.0.0.0) — for a orbit on a remote box, where the access control
                          # is the firewall/security group in front of the port. That serves a
                          # live, writable terminal, so put something in front of it and use
                          # --credential to add ttyd's HTTP basic auth on top. --no-open serves
                          # without launching a desktop browser, for a box that has none
orbit upgrade            # install the latest release (--force on a dev build); a Homebrew
                          # install runs `brew upgrade amenadem/orbit/orbit` instead. Swapping the
                          # binary doesn't touch a running daemon, so afterwards it shuts an idle
                          # one (no live sessions) down for you and the next launch starts on the
                          # new binary. With sessions live it restarts the daemon in place onto
                          # the new binary (what `orbit reload` does), and they keep running.
                          # A daemon from before in-place restarts is left up — its sessions
                          # would die with it — and it says to run `orbit kill` when you're
                          # ready. When the new build speaks another protocol, and so can't
                          # attach to that daemon, it says that too and offers the restart then
                          # and there
```
