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
        Nebula keeps a tree — workspaces hold projects, projects hold worktrees, \
        worktrees hold sessions — and a background daemon owns every PTY in it. \
        Agents keep running after the TUI quits, and their scrollback is replayed \
        when you come back.\n\n\
        A bare `nebula` opens the TUI. The commands below drive the same tree from \
        a shell; `rename`, `worktree` and `spawn` are the ones an agent runs on \
        your behalf from inside a session.",
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
    /// Open this instance on the named workspace.
    ///
    /// Overrides the last workspace opened, for this instance only. Each
    /// nebula window scopes itself, so two can sit on two different
    /// workspaces at once.
    #[arg(long, value_name = "NAME")]
    pub(crate) workspace: Option<String>,
}

const ROOT_EXAMPLES: &str = "\
Examples:
  nebula                            open the TUI (auto-starts the daemon)
  nebula add ~/code/my-app          register a project
  nebula --workspace client-work    open the TUI on a named workspace
  nebula browser --port 8080        serve this TUI in a browser tab

Run `nebula <command> --help` for a command's flags and examples.";

/// `--kind` for `nebula spawn`: one of the agent CLIs nebula runs.
fn parse_agent_kind(s: &str) -> Result<nebula_core::AgentKind, String> {
    nebula_core::AgentKind::parse(s).ok_or_else(|| {
        format!(
            "unknown harness `{s}` — expected one of {}",
            nebula_core::AgentKind::ALL
                .iter()
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
    /// Adds a directory to the project list of the open workspace, named after
    /// the repository's root directory. Bare `nebula <dir>` is the same
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
        /// Start point for a new branch (default: the checkout's HEAD).
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
    /// Manage workspaces — named groups of projects.
    ///
    /// Each nebula instance has exactly one workspace open and scopes its
    /// project list — and the `/` search — to it, so two windows can sit on
    /// two workspaces at once. Every install starts with one named `default`,
    /// which cannot be deleted.
    #[command(after_help = WORKSPACE_EXAMPLES)]
    Workspace {
        #[command(subcommand)]
        command: WorkspaceCommand,
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
    /// and sessions. Destinations are remembered for the TUI's host picker
    /// (`Shift+H`).
    #[command(after_help = SSH_EXAMPLES)]
    Ssh {
        /// ssh destination, passed verbatim (e.g. user@server).
        host: String,
        /// Remote directory to start in (default: remote $HOME).
        path: Option<String>,
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
    },
    /// Install the latest published nebula over this one.
    ///
    /// Runs the install script for the newest release. Upgrading with a daemon
    /// running is safe: sessions keep running on the old binary until you
    /// restart it with `nebula kill` (which stops all sessions).
    #[command(after_help = UPGRADE_EXAMPLES)]
    Upgrade {
        /// Upgrade even when running from a local cargo build.
        #[arg(long)]
        force: bool,
    },
    /// Phase-2 debug client: raw passthrough to a scratch session (Ctrl+\ detaches).
    #[command(hide = true, name = "_raw-attach")]
    RawAttach {
        #[arg(default_value = "0")]
        name: String,
    },
    /// Installer hook: print the cutover note only when a live daemon is on
    /// a different build than this binary (see `make install` / install.sh).
    #[command(hide = true, name = "_stale-daemon-note")]
    StaleDaemonNote,
}

const ADD_EXAMPLES: &str = "\
Examples:
  nebula add .                     add the repo you are standing in
  nebula add ~/code/my-app         add one by path
  nebula ~/code/my-app             the same, without the subcommand";

const DAEMON_EXAMPLES: &str = "\
Examples:
  nebula daemon --foreground       run it attached, logs on stderr
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
  nebula worktree fix-login-redirect  branch off HEAD and move there
  nebula worktree fix login redirect  the same; the words are slugified
  nebula worktree                     invent a random branch name
  nebula worktree hotfix --base v0.21.0
                                      branch from a named start point";

const SPAWN_EXAMPLES: &str = "\
Examples:
  nebula spawn \"port the tests to the new fixture\"
  nebula spawn --kind codex \"review the diff on this branch\"";

const WORKSPACE_EXAMPLES: &str = "\
Examples:
  nebula workspace add client-work   create one
  nebula workspace list              list them; * marks the next default
  nebula workspace open client-work  the next instance opens into it
  nebula --workspace client-work     aim one instance without switching";

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
  nebula ssh user@server /srv/app  start in a directory there";

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
pub(crate) enum WorkspaceCommand {
    /// Create a workspace (does not open it).
    ///
    /// The new workspace starts empty and nothing switches to it;
    /// `nebula workspace open` sets what the next instance boots into.
    #[command(after_help = "Example:\n  nebula workspace add client-work")]
    Add {
        /// Name for the new workspace.
        name: String,
    },
    /// Open a workspace in the next nebula instance launched.
    ///
    /// Running instances keep the workspace they booted on — aim a single one
    /// with `nebula --workspace <name>` instead.
    #[command(after_help = "Example:\n  nebula workspace open client-work")]
    Open {
        /// Workspace the next instance should open into.
        name: String,
    },
    /// List workspaces; `*` marks the one new instances open into.
    #[command(after_help = "Example:\n  nebula workspace list")]
    List,
    /// Delete an empty workspace.
    ///
    /// A workspace that still holds projects is refused, and the `default`
    /// workspace can never be deleted.
    #[command(after_help = "Example:\n  nebula workspace delete client-work")]
    Delete {
        /// Workspace to delete; it must hold no projects.
        name: String,
    },
    /// Rename a workspace.
    #[command(after_help = "Example:\n  nebula workspace rename old-name new-name")]
    Rename {
        /// Workspace to rename.
        name: String,
        /// Its new name.
        new_name: String,
    },
}
