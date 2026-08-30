# Commands

<sub>[← README](../README.md) · [Keys](keys.md) · [Commands](commands.md) · [Sessions](sessions.md) · [Configuration](configuration.md) · [How it works](how-it-works.md)</sub>

The `nebula` CLI. Every command carries its own help — `nebula <command> --help` is the full page,
flags and examples included, and `-h` is the one-screen reminder. This page is the same surface in
one place. Commands marked *(agents run this)* are the ones a coding agent invokes on your behalf —
see [How it works](how-it-works.md).

```
nebula                      open the TUI (auto-starts the daemon)
nebula add <dir>            register a git checkout as a project
nebula daemon               run the daemon that owns every session
nebula kill                 shut the running daemon down (stops all sessions)
nebula rename <title>       title the session this runs inside          (agents run this)
nebula worktree [name]      move this session into a worktree           (agents run this)
nebula spawn <task>         start another agent session beside it       (agents run this)
nebula workspace <cmd>      manage workspaces — named groups of projects
nebula browser              serve this TUI in a web browser via ttyd
nebula ssh <host>           open nebula on a remote host over ssh
nebula tunnel <host>        open a remote host's nebula in a tab here
nebula upgrade              install the latest published nebula
```

## The TUI

```sh
nebula                    # launch the TUI (auto-starts the daemon)
nebula --workspace <name> # launch it on a named workspace; each instance keeps its own, so
                          # two windows can sit on two workspaces at once
```

## Projects and the daemon

```sh
nebula add <dir>          # add a repo as a project, named after its root directory
nebula add .              # same, for the repo you're in (bare `nebula <dir>` / `nebula .` also work)
nebula daemon             # run the daemon (normally auto-spawned)
nebula daemon --foreground  # daemon with logs to stderr, for debugging
nebula kill               # stop the daemon and all sessions cleanly
```

## What agents run for you

```sh
nebula rename <title>     # title the current session (agents run this; --force to retitle)
nebula worktree [name] [--base <ref>]  # move the current session into a worktree of its project,
                          # creating the branch if it's new (agents run this when you ask for a
                          # worktree; no name invents one; --base picks a new branch's start point)
nebula spawn <task> [--kind <claude|codex|cursor>]  # start a new agent session beside the current
                          # one, in the same worktree, opening on <task> (agents run this when you
                          # ask for a new nebula session; --kind defaults to this session's harness)
```

## Workspaces

```sh
nebula workspace add <name>     # create a workspace (a named project group)
nebula workspace open <name>    # open it in the next instance you launch
nebula workspace list           # list workspaces; * marks the one new instances open into
nebula workspace rename <a> <b> # rename a workspace
nebula workspace delete <name>  # delete an empty workspace
```

## Other machines, other screens

```sh
nebula ssh <host> [dir]   # open nebula on a remote machine over ssh (installs it there if
                          # missing); destinations are remembered for the TUI's `h` picker
nebula tunnel <host> [dir] [--port N] [--remote-port N]
                          # that host's nebula in a browser tab here, over one ssh tunnel: installs
                          # nebula there if missing, runs `nebula browser` on its loopback, forwards
                          # the port, and opens the local URL. Nothing is exposed on the remote's
                          # network — the tunnel is the only way in — so it needs no --credential.
                          # If that host already has a `nebula browser` on the port, the tunnel
                          # reuses it instead of failing on the clash (a --credential one will ask
                          # for it in the tab).
                          # Needs ttyd on the remote; Ctrl+C takes both ends down. --port is the
                          # local end (same rules as `nebula browser`), --remote-port the far end
                          # when something there already holds that number
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
nebula upgrade            # install the latest release (--force on a dev build)
```
