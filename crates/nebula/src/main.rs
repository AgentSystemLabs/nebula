mod ssh;
mod upgrade;

use anyhow::Result;
use clap::{Parser, Subcommand};

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
    /// Open nebula on a remote host over ssh (installs it there if missing).
    Ssh {
        /// ssh destination, passed verbatim (e.g. user@server).
        host: String,
        /// Remote directory to start in (default: remote $HOME).
        path: Option<String>,
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

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Some(Command::Daemon { foreground }) => {
            init_daemon_logging(foreground)?;
            log_fatal(nebula_daemon::run_daemon(), nebula_core::paths::daemon_log_path())
        }
        Some(Command::Add { path }) => nebula_tui::run_add_project(path),
        Some(Command::Kill) => nebula_tui::run_kill(),
        Some(Command::Rename { title, force }) => nebula_tui::run_rename(title.join(" "), force),
        Some(Command::Ssh { host, path }) => ssh::run_ssh(&host, path.as_deref()),
        Some(Command::Upgrade { force }) => upgrade::run_upgrade(force),
        Some(Command::StaleDaemonNote) => {
            if nebula_daemon::lifecycle::daemon_is_stale() {
                println!("note: the running daemon was built from older code.");
                println!(
                    "      run 'nebula kill' to restart onto the new binary (stops ALL sessions)."
                );
            }
            Ok(())
        }
        Some(Command::RawAttach { name }) => nebula_tui::run_raw_attach(&name),
        None => match cli.dir {
            Some(dir) => nebula_tui::run_add_project(dir),
            None => {
                init_tui_logging()?;
                log_fatal(nebula_tui::run_tui(), nebula_core::paths::tui_log_path())
            }
        },
    }
}

/// Record a fatal top-level error in the log file before it goes to stderr —
/// the TUI's stderr disappears with the terminal, the daemon's is /dev/null.
fn log_fatal(result: Result<()>, log_path: std::path::PathBuf) -> Result<()> {
    if let Err(err) = &result {
        nebula_core::crashlog::append(&log_path, &format!("FATAL {err:#}"));
    }
    result
}

fn init_daemon_logging(foreground: bool) -> Result<()> {
    // The daemon runs detached with stderr on /dev/null — without this hook a
    // panic (on any thread, tokio workers included) leaves no trace.
    nebula_core::crashlog::install_panic_hook(nebula_core::paths::daemon_log_path());
    let filter = tracing_subscriber::EnvFilter::try_from_env("NEBULA_LOG")
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
    if foreground {
        tracing_subscriber::fmt().with_env_filter(filter).init();
    } else {
        std::fs::create_dir_all(nebula_core::paths::log_dir())?;
        let file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(nebula_core::paths::daemon_log_path())?;
        tracing_subscriber::fmt()
            .with_env_filter(filter)
            .with_writer(file)
            .with_ansi(false)
            .init();
    }
    Ok(())
}

fn init_tui_logging() -> Result<()> {
    // Panic output to stderr dies with the alternate screen — capture it to
    // the log file. The TUI later wraps this hook with its terminal-restore,
    // so the chain on panic is: restore terminal → log to file → stderr.
    nebula_core::crashlog::install_panic_hook(nebula_core::paths::tui_log_path());
    // stdout belongs to the UI — log to file only.
    std::fs::create_dir_all(nebula_core::paths::log_dir())?;
    let file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(nebula_core::paths::tui_log_path())?;
    let filter = tracing_subscriber::EnvFilter::try_from_env("NEBULA_LOG")
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(file)
        .with_ansi(false)
        .init();
    Ok(())
}
