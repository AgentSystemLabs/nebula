# Contributing to Orbit

Thanks for wanting to help. Orbit is a young project that moves fast, so small, focused pull requests
land quickest. This page covers everything from your first build to opening the PR.

## Ways to help

- **Report a bug.** [Open an issue](https://github.com/AmenAdem/Orbit/issues/new) with what you did,
  what you expected, what happened, and the output of `orbit --version` and `orbit doctor`. A daemon
  log helps a lot: start with `ORBIT_LOG=debug orbit`, then attach the relevant lines from
  `~/Library/Application Support/dev.orbit.orbit/state/daemon.log` (Linux: `~/.local/state/orbit/`).
- **Suggest a feature.** Open an issue first, describing the job you're trying to get done, before
  writing code. The planned work is listed in the guide under
  [What's coming](docs/guide.md#16-whats-coming).
- **Fix the docs.** Typos, unclear steps and missing troubleshooting entries are all welcome. The
  website is built from the same Markdown files, so one fix covers both.
- **Send code.** Read on.

## Set up

You need macOS or Linux (Windows through WSL2), plus:

| Tool | Why | Get it |
|---|---|---|
| Rust (stable) | builds Orbit | [rustup.rs](https://rustup.rs) |
| `git` | worktrees, diffs, everything | your package manager |
| `make` | the dev commands below | Xcode Command Line Tools / `build-essential` |
| `claude` | the agent Orbit drives (optional for UI work, see `AGENT=` below) | [Claude Code setup](https://code.claude.com/docs/en/setup) |

```sh
git clone https://github.com/<you>/Orbit.git
cd Orbit
make dev
```

`make dev` builds the current code and opens an **isolated copy** of Orbit, with its own daemon, its
own database and its own settings, so it never touches your everyday install or its sessions.

```sh
make dev AGENT=/bin/cat   # a stand-in instead of Claude: try the UI for free
make dev-seed             # copy your real projects/settings into the dev copy (once)
make dev-reset            # wipe the dev copy's data and start fresh
make dev-stop             # stop the dev daemon
```

## The code

Four crates, one binary:

| Crate | What lives there |
|---|---|
| `crates/orbit-core` | the protocol between TUI and daemon, and the shared entities |
| `crates/orbit-daemon` | sessions and their terminals, SQLite, git worktrees, the Claude hooks, status |
| `crates/orbit-tui` | the interface (ratatui) |
| `crates/orbit-fuzzy` | fuzzy matching for the pickers |
| `crates/orbit` | the `orbit` binary and its subcommands |

[ARCHITECTURE.md](ARCHITECTURE.md) explains the process model, how the pieces talk, and how to add a
modal. [How it works](docs/how-it-works.md) covers the hooks and status dots in depth.

## Before you open a pull request

Run the same gate CI runs:

```sh
make ci     # cargo fmt --check, clippy with -D warnings, the whole test suite
```

Quicker loops while you work:

```sh
make check  # typecheck the workspace
make fmt    # format
make lint   # clippy (STRICT=1 to fail on warnings)
make test   # tests; e2e_pty spawns real daemons, so it's the slow part
```

If `make ci` fails with `fatal: no tag message?`, your global git config signs tags. Run it as
`GIT_CONFIG_GLOBAL=/dev/null make ci`.

For changes to the interface, attach a screenshot. `make shot SCENE=… KEYS="…"` renders the debug
TUI with demo data so you don't need real sessions.

## Pull request checklist

- One topic per pull request, branched from `main`.
- `make ci` passes on your machine.
- New behaviour has a test where it can (the daemon and the protocol are well covered; follow the
  existing tests next to the code you change).
- User-visible changes update the docs in `docs/` and, when a key changes, the key tables.
- Commit messages say what changed and why in plain words, like the existing history.
- Keep source files under about 3,000 lines. CI warns past that; split the module instead.

## Docs and the website

The site at [orbit-tui.pages.dev](https://orbit-tui.pages.dev) is built from `docs/*.md`,
`ARCHITECTURE.md` and this file by `website/build.mjs`, so edit the Markdown and the site follows.
The landing page itself is `website/src/index.html`.

```sh
cd website
npm install
npm run dev      # build into website/dist and serve it at http://localhost:4321
```

Pushing to `main` deploys the site when anything under `docs/`, `website/`, `ARCHITECTURE.md` or
`CONTRIBUTING.md` changed.

## Upstream changes

Orbit is a fork of [nebula](https://github.com/AgentSystemLabs/nebula). Upstream changes come in
through `scripts/rebrand.sh`, which renames an upstream branch to Orbit before it is merged; its
header has the exact steps. When you add something new, prefer a new module or a setting over
reshaping shared code, so upstream merges stay easy.

## Releases

Maintainers push a `v*` tag; CI builds macOS (Apple Silicon and Intel) and Linux (x86_64 and
aarch64) binaries and attaches them to the GitHub release, and `scripts/homebrew-formula.sh` updates
the Homebrew formula.

## License

By contributing you agree that your work is released under the [MIT license](LICENSE), the same as
the rest of Orbit.
