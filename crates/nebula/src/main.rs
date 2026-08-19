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
}

#[derive(Subcommand)]
enum Command {
    /// Run the daemon process (normally auto-spawned by the TUI).
    Daemon {
        /// Stay attached to the terminal instead of logging to file.
        #[arg(long)]
        foreground: bool,
    },
    /// Ask a running daemon to shut down cleanly.
    KillServer,
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
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Some(Command::Daemon { foreground }) => {
            init_daemon_logging(foreground)?;
            log_fatal(nebula_daemon::run_daemon(), nebula_core::paths::daemon_log_path())
        }
        Some(Command::KillServer) => nebula_tui::run_kill_server(),
        Some(Command::Ssh { host, path }) => ssh::run_ssh(&host, path.as_deref()),
        Some(Command::Upgrade { force }) => upgrade::run_upgrade(force),
        Some(Command::RawAttach { name }) => nebula_tui::run_raw_attach(&name),
        None => {
            init_tui_logging()?;
            log_fatal(nebula_tui::run_tui(), nebula_core::paths::tui_log_path())
        }
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
