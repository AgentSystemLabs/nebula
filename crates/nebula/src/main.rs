mod browser;
mod ssh;
mod tunnel;
mod upgrade;

use anyhow::Result;
use clap::{Parser, Subcommand};
use std::path::Path;

#[derive(Parser)]
#[command(
    name = "nebula",
    version,
    about = "Terminal multiplexer for Claude Code agents"
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
    /// Directory to add as a project — shorthand for `nebula add <dir>`.
    /// (A directory whose name collides with a subcommand needs the long
    /// form or a `./` prefix.)
    dir: Option<String>,
    /// Open this instance on the named workspace instead of the last one
    /// opened. Each nebula window scopes itself, so two can sit on two
    /// different workspaces at once.
    #[arg(long, value_name = "NAME")]
    workspace: Option<String>,
}

#[derive(Subcommand)]
enum Command {
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
enum WorkspaceCommand {
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

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Some(Command::Daemon { foreground }) => {
            init_daemon_logging(foreground)?;
            log_fatal(
                nebula_daemon::run_daemon(),
                &nebula_core::paths::daemon_log_path(),
            )
        }
        Some(Command::Add { path }) => nebula_tui::run_add_project(path),
        Some(Command::Workspace { command }) => {
            use nebula_tui::WorkspaceOp;
            let op = match command {
                WorkspaceCommand::Add { name } => WorkspaceOp::Add { name },
                WorkspaceCommand::Open { name } => WorkspaceOp::Open { name },
                WorkspaceCommand::List => WorkspaceOp::List,
                WorkspaceCommand::Delete { name } => WorkspaceOp::Delete { name },
                WorkspaceCommand::Rename { name, new_name } => {
                    WorkspaceOp::Rename { name, new_name }
                }
            };
            nebula_tui::run_workspace(op)
        }
        Some(Command::Kill) => nebula_tui::run_kill(),
        Some(Command::Rename { title, force }) => {
            let mode = if force {
                nebula_tui::RenameMode::Force
            } else {
                nebula_tui::RenameMode::Auto
            };
            nebula_tui::run_rename(title.join(" "), mode)
        }
        Some(Command::Worktree { name, base }) => nebula_tui::run_worktree(name.join(" "), base),
        Some(Command::Browser {
            port,
            bind,
            public,
            credential,
            no_open,
        }) => browser::run_browser(browser::BrowserOpts {
            port,
            // --public is --bind 0.0.0.0 with a name; clap keeps the two
            // from being given at once.
            bind: bind.unwrap_or(if public {
                browser::PUBLIC_BIND
            } else {
                browser::DEFAULT_BIND
            }),
            credential,
            open: !no_open,
        }),
        Some(Command::Ssh { host, path }) => ssh::run_ssh(&host, path.as_deref()),
        Some(Command::Tunnel {
            host,
            path,
            port,
            remote_port,
        }) => tunnel::run_tunnel(tunnel::TunnelOpts {
            host,
            path,
            port,
            remote_port,
        }),
        Some(Command::Upgrade { force }) => upgrade::run_upgrade(force),
        Some(Command::StaleDaemonNote) => {
            if nebula_daemon::lifecycle::daemon_is_stale() {
                println!("note: the running daemon was built from older code.");
                println!("{}", upgrade::KILL_HINT);
            }
            Ok(())
        }
        Some(Command::RawAttach { name }) => nebula_tui::run_raw_attach(&name),
        None => match cli.dir {
            Some(dir) => nebula_tui::run_add_project(dir),
            None => {
                init_tui_logging()?;
                let handoff = log_fatal(
                    nebula_tui::run_tui(cli.workspace),
                    &nebula_core::paths::tui_log_path(),
                )?;
                match handoff {
                    // Hosts-picker handoff: the TUI quit and restored the
                    // terminal so a fresh `nebula ssh` can exec over us (the
                    // local daemon and its sessions stay up).
                    Some(entry) => {
                        eprintln!("nebula: connecting to {}…", entry.host);
                        ssh::run_ssh(&entry.host, entry.path.as_deref())
                    }
                    None => Ok(()),
                }
            }
        },
    }
}

/// Record a fatal top-level error in the log file before it goes to stderr —
/// the TUI's stderr disappears with the terminal, the daemon's is /dev/null.
fn log_fatal<T>(result: Result<T>, log_path: &Path) -> Result<T> {
    if let Err(err) = &result {
        nebula_core::crashlog::append(log_path, &format!("FATAL {err:#}"));
    }
    result
}

fn log_filter() -> tracing_subscriber::EnvFilter {
    tracing_subscriber::EnvFilter::try_from_env(nebula_core::env::LOG)
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"))
}

/// Route tracing to `log_path` (created on demand, appended, no ANSI) —
/// neither binary can log to the terminal: the TUI owns it and the daemon
/// has no stderr.
fn init_file_logging(log_path: &Path) -> Result<()> {
    std::fs::create_dir_all(nebula_core::paths::log_dir())?;
    let file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_path)?;
    tracing_subscriber::fmt()
        .with_env_filter(log_filter())
        .with_writer(file)
        .with_ansi(false)
        .init();
    Ok(())
}

fn init_daemon_logging(foreground: bool) -> Result<()> {
    // The daemon runs detached with stderr on /dev/null — without this hook a
    // panic (on any thread, tokio workers included) leaves no trace.
    let log_path = nebula_core::paths::daemon_log_path();
    nebula_core::crashlog::install_panic_hook(log_path.clone());
    if foreground {
        tracing_subscriber::fmt()
            .with_env_filter(log_filter())
            .init();
        return Ok(());
    }
    init_file_logging(&log_path)
}

fn init_tui_logging() -> Result<()> {
    // Panic output to stderr dies with the alternate screen — capture it to
    // the log file. The TUI later wraps this hook with its terminal-restore,
    // so the chain on panic is: restore terminal → log to file → stderr.
    let log_path = nebula_core::paths::tui_log_path();
    nebula_core::crashlog::install_panic_hook(log_path.clone());
    // stdout belongs to the UI — log to file only.
    init_file_logging(&log_path)
}
