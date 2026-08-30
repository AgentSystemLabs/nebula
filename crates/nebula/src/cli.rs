//! The command-line surface: every `nebula` subcommand, its arguments and the
//! help text they print. `main.rs` owns only the dispatch and the logging.

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "nebula",
    version,
    about = "Terminal multiplexer for Claude Code agents"
)]
pub(crate) struct Cli {
    #[command(subcommand)]
    pub(crate) command: Option<Command>,
    /// Directory to add as a project — shorthand for `nebula add <dir>`.
    /// (A directory whose name collides with a subcommand needs the long
    /// form or a `./` prefix.)
    pub(crate) dir: Option<String>,
    /// Open this instance on the named workspace instead of the last one
    /// opened. Each nebula window scopes itself, so two can sit on two
    /// different workspaces at once.
    #[arg(long, value_name = "NAME")]
    pub(crate) workspace: Option<String>,
}

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
    /// Add a directory as a project, named after the repo's root directory
    /// (`nebula add .` for the current one; bare `nebula <dir>` works too).
    Add {
        /// Path to a git repository (default: the current directory).
        #[arg(default_value = ".")]
        path: String,
    },
    /// Run the daemon process (normally auto-spawned by the TUI).
    Daemon {
        /// Stay attached to the terminal instead of logging to file.
        #[arg(long)]
        foreground: bool,
    },
    /// Ask a running daemon to shut down cleanly.
    Kill,
    /// Title this session (run from inside a nebula agent session; agents
    /// use it to auto-title on the first prompt).
    Rename {
        /// The new title; multiple words need no quotes.
        #[arg(required = true, num_args = 1..)]
        title: Vec<String>,
        /// Replace an existing title instead of only filling in a missing one.
        #[arg(long)]
        force: bool,
    },
    /// Move this session into a worktree of its project (run from inside a
    /// nebula agent session; agents run it when you ask them to work in a
    /// worktree). Creates the checkout when the branch has none, re-homes
    /// the session at once, and restarts it resumed inside the worktree as
    /// soon as the current turn ends.
    Worktree {
        /// Branch name; several words are joined with hyphens, none at all
        /// gets a random `<adj>-<noun>-<verb>` one.
        name: Vec<String>,
        /// Start point for a new branch (default: the checkout's HEAD).
        #[arg(long, value_name = "REF")]
        base: Option<String>,
    },
    /// Start a new agent session beside this one — same worktree, same
    /// harness unless --kind names another — opening on the given task as
    /// its first prompt (run from inside a nebula agent session; agents run
    /// it when you ask for a new nebula session). The new row shows up in
    /// the sessions list on its own; this session carries on untouched.
    Spawn {
        /// The task the new session starts on; multiple words need no quotes.
        #[arg(required = true, num_args = 1..)]
        task: Vec<String>,
        /// Harness for the new session: claude, codex or cursor (default:
        /// the same as this session's).
        #[arg(long, value_name = "KIND", value_parser = parse_agent_kind)]
        kind: Option<nebula_core::AgentKind>,
    },
    /// Manage workspaces — named project groups. Each nebula instance has
    /// one open and scopes its project list (and `/` search) to it.
    Workspace {
        #[command(subcommand)]
        command: WorkspaceCommand,
    },
    /// Serve this TUI in a web browser via ttyd and open it (loopback
    /// unless --bind/--public widens it).
    Browser {
        /// Port for ttyd to listen on. Omit to take 7681 when it's free and
        /// a free one otherwise — so a checkout per worktree can each serve
        /// at once. `--port 0` always picks a free one; a port named
        /// explicitly is used or the command fails.
        #[arg(long)]
        port: Option<u16>,
        /// Address to listen on (default 127.0.0.1). Name a specific
        /// interface address to reach this nebula from another host —
        /// e.g. `--bind 10.0.1.7`. See --public for every interface.
        #[arg(long, value_name = "ADDR", conflicts_with = "public")]
        bind: Option<std::net::IpAddr>,
        /// Listen on every interface (0.0.0.0), for a nebula on a remote
        /// box. This serves a live, writable terminal to anything that can
        /// reach the port — put a firewall, security group, or VPN in front
        /// of it, and consider --credential.
        #[arg(long)]
        public: bool,
        /// HTTP basic auth for the served terminal, as USER:PASSWORD.
        #[arg(long, value_name = "USER:PASSWORD")]
        credential: Option<String>,
        /// Serve the URL but do not hand it to a desktop browser — for a
        /// machine with no desktop to open it on (`nebula tunnel` runs the
        /// remote half this way).
        #[arg(long)]
        no_open: bool,
    },
    /// Open nebula on a remote host over ssh (installs it there if missing).
    Ssh {
        /// ssh destination, passed verbatim (e.g. user@server).
        host: String,
        /// Remote directory to start in (default: remote $HOME).
        path: Option<String>,
    },
    /// Open a remote host's nebula in a browser tab here, over an ssh tunnel
    /// (installs nebula there if missing; needs ttyd on the remote). The
    /// remote serves on its own loopback only — the tunnel is the way in.
    Tunnel {
        /// ssh destination, passed verbatim (e.g. user@server).
        host: String,
        /// Remote directory to start in (default: remote $HOME).
        path: Option<String>,
        /// Local end of the tunnel, and the port the browser opens. Omit to
        /// take 7681 when it is free and a free port otherwise; `--port 0`
        /// always picks a free one.
        #[arg(long)]
        port: Option<u16>,
        /// Port the remote serves on (default: the same number as --port).
        /// Name one when something on the remote already holds that port.
        #[arg(long, value_name = "PORT")]
        remote_port: Option<u16>,
    },
    /// Install the latest published nebula over this one.
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

#[derive(Subcommand)]
pub(crate) enum WorkspaceCommand {
    /// Create a workspace (does not open it).
    Add { name: String },
    /// Open a workspace in the next nebula instance launched. Running ones
    /// keep theirs — aim a single instance with `nebula --workspace <name>`.
    Open { name: String },
    /// List workspaces; `*` marks the one new instances open into.
    List,
    /// Delete an empty workspace.
    Delete { name: String },
    /// Rename a workspace.
    Rename { name: String, new_name: String },
}
