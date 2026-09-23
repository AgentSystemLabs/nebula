//! The command-line surface: every `nebula` subcommand, its arguments, and the
//! help each one prints. `main.rs` owns only the dispatch and the logging.
//!
//! Two rules keep `--help` readable, and both are load-bearing:
//!
//! * **Every doc comment is two paragraphs.** `clap_derive` takes the first
//!   paragraph as `about` and the whole comment as `long_about`, and the root's
//!   command list only ever renders `about` — so paragraph one is a single
//!   sentence under ~60 characters that has to stand alone, and the prose after
//!   the blank line is what `nebula <command> --help` shows.
//! * **Examples are `after_help`**, which `after_long_help` falls back to, so
//!   one string serves both `-h` and `--help`. Clap runs it through the same
//!   wrapper as everything else: keep every line under ~78 characters or the
//!   hand-aligned columns reflow into a mess on a narrow terminal.
//!
//! Wrapping itself comes from clap's `wrap_help` feature (see Cargo.toml).
//! Without it `StyledStr::wrap` compiles to a no-op and no width setting here
//! does anything at all.

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "nebula",
    version,
    max_term_width = 100,
    about = "Terminal multiplexer for Claude Code agents",
    long_about = "Terminal multiplexer for Claude Code agents.\n\n\
        Nebula keeps a tree — projects hold worktrees, worktrees hold sessions — \
        and a background daemon owns every PTY in it. \
        Agents keep running after the TUI quits, and their scrollback is replayed \
        when you come back.\n\n\
        A bare `nebula` opens the TUI. The commands below drive the same tree from \
        a shell; `rename`, `worktree`, `spawn` and `open` are the ones an agent \
        runs on your behalf from inside a session.",
    after_help = ROOT_EXAMPLES
)]
pub(crate) struct Cli {
    #[command(subcommand)]
    pub(crate) command: Option<Command>,
    /// Directory to add as a project — shorthand for `nebula add <dir>`.
    ///
    /// A directory whose name collides with a subcommand needs the long form
    /// (`nebula add browser`) or a `./` prefix.
    pub(crate) dir: Option<String>,
}

const ROOT_EXAMPLES: &str = "\
Examples:
  nebula                            open the TUI (auto-starts the daemon)
  nebula add ~/code/my-app          register a project
  nebula browser --port 8080        serve this TUI in a browser tab

Run `nebula <command> --help` for a command's flags and examples.";

/// `--kind` for `nebula spawn`: one of the agent CLIs nebula runs. A bare
/// `custom` is never accepted: custom harnesses carry a registry id the
/// flag cannot name, so they launch from the TUI picker and presets.
fn parse_agent_kind(s: &str) -> Result<nebula_core::AgentKind, String> {
    nebula_core::AgentKind::parse(s).ok_or_else(|| {
        format!(
            "unknown harness `{s}` — expected one of {} (custom harnesses launch from the TUI)",
            nebula_core::AgentKind::ALL
                .iter()
                .filter(|k| **k != nebula_core::AgentKind::Custom)
                .map(|k| k.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        )
    })
}

#[derive(Subcommand)]
pub(crate) enum Command {
    /// Register a git checkout as a project.
    ///
    /// Adds a directory to the project list, named after the repository's
    /// root directory. Bare `nebula <dir>` is the same
    /// command, so `nebula .` and `nebula add .` do the same thing.
    #[command(after_help = ADD_EXAMPLES)]
    Add {
        /// Path to a git repository (default: the current directory).
        #[arg(default_value = ".")]
        path: String,
    },
    /// Run the daemon that owns every session.
    ///
    /// The daemon holds every PTY, the store, git and agent status. The TUI
    /// auto-spawns it detached, so you rarely run this by hand — reach for it
    /// when you want to watch what the daemon is doing. Set NEBULA_LOG to
    /// change the log level.
    #[command(after_help = DAEMON_EXAMPLES)]
    Daemon {
        /// Stay attached to the terminal instead of logging to file.
        #[arg(long)]
        foreground: bool,
    },
    /// Shut the running daemon down (stops all sessions).
    ///
    /// Asks the daemon to exit cleanly; every session it owns stops with it.
    /// A daemon from a build on another protocol can't take that request, so
    /// it gets SIGTERM instead, which it handles the same clean way.
    /// Quitting the TUI does not do this — the daemon outlives its clients on
    /// purpose — so this is how you stop everything, and how you move onto a
    /// newly installed binary.
    #[command(after_help = KILL_EXAMPLES)]
    Kill,
    /// Title the session this command runs inside.
    ///
    /// Run from inside a nebula agent session: it titles that session's row.
    /// Agents run it themselves to auto-title on your first prompt. Without
    /// --force it only fills in a title that is still missing, so an agent
    /// can never overwrite a name you chose.
    #[command(after_help = RENAME_EXAMPLES)]
    Rename {
        /// The new title; multiple words need no quotes.
        #[arg(required = true, num_args = 1..)]
        title: Vec<String>,
        /// Replace an existing title instead of only filling in a missing one.
        #[arg(long)]
        force: bool,
    },
    /// Move this session into a worktree of its project.
    ///
    /// Run from inside a nebula agent session; agents run it when you ask them
    /// to work in a worktree. Creates the git worktree when the branch has
    /// none, re-homes the session onto it at once, and restarts the session
    /// resumed inside the new checkout as soon as the current turn ends.
    #[command(after_help = WORKTREE_EXAMPLES)]
    Worktree {
        /// Branch name; several words are joined with hyphens, none at all
        /// gets a random `<adj>-<noun>-<verb>` one.
        name: Vec<String>,
        /// Start point for a new branch (default: the `worktree_base_branch`
        /// setting, else origin's default branch, fetched).
        ///
        /// A branch name origin has means origin's copy of it, fetched first:
        /// `main` is `origin/main`, never this checkout's local branch. A tag,
        /// a SHA or a branch origin lacks is used as named.
        #[arg(long, value_name = "REF")]
        base: Option<String>,
    },
    /// Start another agent session beside this one.
    ///
    /// Run from inside a nebula agent session; agents run it when you ask for
    /// a new nebula session. The new session starts in the same worktree, on
    /// the task you name as its first prompt, and shows up in the sessions
    /// list on its own — this session carries on untouched.
    #[command(after_help = SPAWN_EXAMPLES)]
    Spawn {
        /// The task the new session starts on; multiple words need no quotes.
        #[arg(required = true, num_args = 1..)]
        task: Vec<String>,
        /// Harness for the new session: claude, codex or cursor.
        ///
        /// Defaults to the harness this session is running.
        #[arg(long, value_name = "KIND", value_parser = parse_agent_kind)]
        kind: Option<nebula_core::AgentKind>,
    },
    /// Show files to the user inside this nebula.
    ///
    /// Run from inside a nebula agent session; agents run it only when you
    /// ask to see a file, never unprompted. Text files only: an image or any
    /// other binary is refused, since a terminal has nothing to show for it.
    /// The files open in nebula's file tabs — a modal with one tab per file,
    /// the focused one previewed, Enter editing it — in every nebula
    /// attached to this daemon, and this session carries on untouched.
    #[command(after_help = OPEN_EXAMPLES)]
    Open {
        /// The files to show, relative to the current directory or absolute.
        #[arg(required = true, num_args = 1.., value_name = "FILE")]
        files: Vec<String>,
    },
    /// Back up, restore or locate this machine's settings.
    ///
    /// Settings live in `config.json` — the portable file an export, an
    /// import and `nebula ssh` carry — with `config.local.json` over it for
    /// what only makes sense on this machine, beside `agent_presets.json` and
    /// `ssh_hosts.json`. An export is one JSON file holding all but the local
    /// layer; an import merges one in. Changes apply without a restart.
    #[command(after_help = CONFIG_EXAMPLES)]
    Config {
        #[command(subcommand)]
        command: ConfigCommand,
    },
    /// Serve this TUI in a web browser via ttyd.
    ///
    /// Runs ttyd in front of a nebula TUI and opens a tab on it, so a phone or
    /// another machine can drive this nebula. Needs ttyd on PATH
    /// (`brew install ttyd`); Ctrl+C takes the server down. It listens on
    /// loopback unless --bind or --public widens it.
    #[command(after_help = BROWSER_EXAMPLES)]
    Browser {
        /// Port for ttyd to listen on.
        ///
        /// Omit to take 7681 when it's free and a free one otherwise — so a
        /// checkout per worktree can each serve at once. `--port 0` always
        /// picks a free one; a port named explicitly is used or the command
        /// fails, which is what you want behind an ssh tunnel.
        #[arg(long)]
        port: Option<u16>,
        /// Address to listen on (default 127.0.0.1).
        ///
        /// Name a specific interface address to reach this nebula from another
        /// host — e.g. `--bind 10.0.1.7`. See --public for every interface.
        #[arg(long, value_name = "ADDR", conflicts_with = "public")]
        bind: Option<std::net::IpAddr>,
        /// Listen on every interface (0.0.0.0).
        ///
        /// For a nebula on a remote box. This serves a live, writable terminal
        /// to anything that can reach the port — put a firewall, security
        /// group, or VPN in front of it, and consider --credential.
        #[arg(long)]
        public: bool,
        /// HTTP basic auth for the served terminal, as USER:PASSWORD.
        ///
        /// ttyd asks for it in the browser tab. Worth adding to any bind wider
        /// than loopback, on top of whatever guards the port itself.
        #[arg(long, value_name = "USER:PASSWORD")]
        credential: Option<String>,
        /// Serve the URL but do not open a desktop browser.
        ///
        /// For a machine with no desktop to open it on — `nebula tunnel` runs
        /// the remote half this way.
        #[arg(long)]
        no_open: bool,
    },
    /// Open nebula on a remote host over ssh.
    ///
    /// Connects with ssh and runs nebula there, installing it on the remote
    /// first when it is missing, so what you drive is the remote's own daemon
    /// and sessions. This machine's `config.json` and agent presets ride
    /// along and are merged into the remote's settings, where its own
    /// `config.local.json` still wins. Destinations are remembered for the
    /// TUI's host picker (`Shift+H`).
    #[command(after_help = SSH_EXAMPLES)]
    Ssh {
        /// ssh destination, passed verbatim (e.g. user@server).
        host: String,
        /// Remote directory to start in (default: remote $HOME).
        path: Option<String>,
        /// Leave this machine's settings behind for this connection.
        ///
        /// The `ssh_sync_config` setting turns the forward off for good.
        #[arg(long)]
        no_sync_config: bool,
    },
    /// Open a remote host's nebula in a browser tab here.
    ///
    /// One ssh tunnel does the whole thing: it installs nebula on the remote
    /// if missing, runs `nebula browser` on the remote's own loopback,
    /// forwards the port, and opens the local URL. Nothing is exposed on the
    /// remote's network — the tunnel is the only way in — so it needs no
    /// --credential. A `nebula browser` already serving that port is reused
    /// rather than treated as a clash. Needs ttyd on the remote; Ctrl+C takes
    /// both ends down.
    #[command(after_help = TUNNEL_EXAMPLES)]
    Tunnel {
        /// ssh destination, passed verbatim (e.g. user@server).
        host: String,
        /// Remote directory to start in (default: remote $HOME).
        path: Option<String>,
        /// Local end of the tunnel, and the port the browser opens.
        ///
        /// Omit to take 7681 when it is free and a free port otherwise;
        /// `--port 0` always picks a free one.
        #[arg(long)]
        port: Option<u16>,
        /// Port the remote serves on (default: the same number as --port).
        ///
        /// Name one when something on the remote already holds that port.
        #[arg(long, value_name = "PORT")]
        remote_port: Option<u16>,
        /// Leave this machine's settings behind for this connection.
        ///
        /// By default `config.json` and the agent presets ride along, as they
        /// do for `nebula ssh`; the `ssh_sync_config` setting turns that off
        /// for good.
        #[arg(long)]
        no_sync_config: bool,
    },
    /// Install the latest published nebula over this one.
    ///
    /// Runs the install script for the newest release. Upgrading with a daemon
    /// running is safe: sessions keep running on the old binary until you
    /// restart it with `nebula kill` (which stops all sessions). When the new
    /// build can't attach to that daemon, it says so and offers the restart.
    #[command(after_help = UPGRADE_EXAMPLES)]
    Upgrade {
        /// Upgrade even when running from a local cargo build.
        #[arg(long)]
        force: bool,
    },
    /// Installer hook: print the cutover note only when a live daemon is on
    /// a different build than this binary (see `make install` / install.sh).
    #[command(hide = true, name = "_stale-daemon-note")]
    StaleDaemonNote,
    /// Upgrade hook: print the protocol version this binary speaks, so the
    /// `nebula upgrade` that installed it can tell whether it will still
    /// attach to the daemon left running.
    #[command(hide = true, name = "_protocol-version")]
    ProtocolVersion,
}

const ADD_EXAMPLES: &str = "\
Examples:
  nebula add .                     add the repo you are standing in
  nebula add ~/code/my-app         add one by path
  nebula ~/code/my-app             the same, without the subcommand";

const DAEMON_EXAMPLES: &str = "\
Examples:
  nebula daemon --foreground       run it attached, logs on stdout
  NEBULA_LOG=debug nebula daemon --foreground
                                   the same, at debug level";

const KILL_EXAMPLES: &str = "\
Examples:
  nebula kill                      stop the daemon and every session";

const RENAME_EXAMPLES: &str = "\
Examples:
  nebula rename Fix Login Redirect   title this session
  nebula rename --force Auth Rework  replace a title already set";

const WORKTREE_EXAMPLES: &str = "\
Examples:
  nebula worktree fix-login-redirect  branch off the configured base and move there
  nebula worktree fix login redirect  the same; the words are slugified
  nebula worktree                     invent a random branch name
  nebula worktree hotfix --base v0.21.0
                                      branch from a named start point";

const SPAWN_EXAMPLES: &str = "\
Examples:
  nebula spawn \"port the tests to the new fixture\"
  nebula spawn --kind codex \"review the diff on this branch\"";

const OPEN_EXAMPLES: &str = "\
Examples:
  nebula open README.md                one tab
  nebula open src/main.rs docs/keys.md a tab each, in this order";

const BROWSER_EXAMPLES: &str = "\
Examples:
  nebula browser                   serve on 127.0.0.1:7681, open a tab
  nebula browser --port 8080       take a specific port
  nebula browser --no-open         serve only; print the URL
  nebula browser --public --credential me:secret
                                   reachable off-box, behind basic auth";

const SSH_EXAMPLES: &str = "\
Examples:
  nebula ssh user@server           open the remote's nebula
  nebula ssh user@server /srv/app  start in a directory there
  nebula ssh user@server --no-sync-config
                                   keep this machine's settings here";

const CONFIG_EXAMPLES: &str = "\
Examples:
  nebula config path               where each settings file lives
  nebula config export ~/backups   write ~/backups/nebula-settings.json
  nebula config import ~/backups   merge it back in, here or elsewhere";

const TUNNEL_EXAMPLES: &str = "\
Examples:
  nebula tunnel user@server           the remote's TUI in a tab here
  nebula tunnel user@server /srv/app  start in a directory there
  nebula tunnel user@server --port 9000
                                      pick the local end of the tunnel";

const UPGRADE_EXAMPLES: &str = "\
Examples:
  nebula upgrade                   install the latest release
  nebula upgrade --force           do it over a local cargo build";

#[derive(Subcommand)]
pub(crate) enum ConfigCommand {
    /// Print where each settings file lives.
    ///
    /// `NEBULA_CONFIG_FILE` moves `config.json` alone — into a dotfiles
    /// checkout, say; `NEBULA_DATA_DIR` moves them all.
    #[command(after_help = "Example:\n  nebula config path")]
    Path,
    /// Write this machine's settings to one JSON file.
    ///
    /// Carries `config.json`, the agent presets and the ssh host list, never
    /// `config.local.json`. Keys and presets this build doesn't know are
    /// carried as they are, so a newer nebula's settings survive the trip.
    #[command(
        after_help = "Examples:\n  nebula config export > nebula-settings.json\n  \
                            nebula config export ~/backups   writes ~/backups/nebula-settings.json"
    )]
    Export {
        /// File, or existing folder, to write (default: stdout; `-` too).
        #[arg(value_name = "PATH")]
        path: Option<String>,
    },
    /// Merge a settings backup into this machine's settings.
    ///
    /// Takes an export, a bare `config.json`, `agent_presets.json` or
    /// `ssh_hosts.json`, a folder holding any of them, or `-` for stdin. Keys
    /// the file sets replace this machine's and keys it lacks are left alone;
    /// presets merge by name and hosts by destination. `config.local.json` is
    /// never written, and still wins.
    #[command(
        after_help = "Examples:\n  nebula config import nebula-settings.json\n  \
                            nebula config import ~/dotfiles/nebula   a folder holding config.json"
    )]
    Import {
        /// The file, the folder, or `-` for stdin.
        #[arg(value_name = "SOURCE")]
        source: String,
    },
    /// Print the effective harness registry: every harness nebula knows —
    /// the built-ins, `custom_harnesses` entries and `harnesses` map ids —
    /// with the program, flags, resume shape, hook dialect and defaults a
    /// launch actually uses. Copy a row into config.json `harnesses` to
    /// override it.
    #[command(after_help = "Example:\n  nebula config harnesses")]
    Harnesses,
}
