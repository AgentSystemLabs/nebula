mod browser;
mod cli;
mod ssh;
mod tunnel;
mod upgrade;

use anyhow::Result;
use clap::Parser;
use cli::{Cli, Command, ConfigCommand};
use std::path::Path;

fn main() -> Result<()> {
    let cli = Cli::parse();
    // A `nebula ssh` / `nebula tunnel` from another machine may have sent its
    // settings along. Merge them before anything reads a setting, and before
    // a thread or a child exists to inherit the variable.
    nebula_tui::bundle::apply_forwarded();
    match cli.command {
        Some(Command::Daemon { foreground }) => {
            init_daemon_logging(foreground)?;
            log_fatal(
                nebula_daemon::run_daemon(),
                &nebula_core::paths::daemon_log_path(),
            )
        }
        Some(Command::Add { path }) => nebula_tui::run_add_project(path),
        Some(Command::Config { command }) => nebula_tui::run_config(match command {
            ConfigCommand::Path => nebula_tui::ConfigOp::Path,
            ConfigCommand::Export { path } => nebula_tui::ConfigOp::Export { path },
            ConfigCommand::Import { source } => nebula_tui::ConfigOp::Import { source },
            ConfigCommand::Harnesses => nebula_tui::ConfigOp::Harnesses,
        }),
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
        Some(Command::Spawn { task, kind }) => nebula_tui::run_spawn(task.join(" "), kind),
        Some(Command::Open { files }) => nebula_tui::run_open(files),
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
        Some(Command::Ssh {
            host,
            path,
            no_sync_config,
        }) => ssh::run_ssh(&host, path.as_deref(), !no_sync_config),
        Some(Command::Tunnel {
            host,
            path,
            port,
            remote_port,
            no_sync_config,
        }) => tunnel::run_tunnel(tunnel::TunnelOpts {
            host,
            path,
            port,
            remote_port,
            sync_config: !no_sync_config,
        }),
        Some(Command::Upgrade { force }) => upgrade::run_upgrade(force),
        Some(Command::StaleDaemonNote) => {
            if nebula_daemon::lifecycle::daemon_is_stale() {
                println!("note: the running daemon was built from older code.");
                println!("{}", upgrade::KILL_HINT);
            }
            Ok(())
        }
        Some(Command::ProtocolVersion) => {
            println!("{}", nebula_core::PROTOCOL_VERSION);
            Ok(())
        }
        Some(Command::RawAttach { name }) => nebula_tui::run_raw_attach(&name),
        None => match cli.dir {
            Some(dir) => nebula_tui::run_add_project(dir),
            None => {
                init_tui_logging()?;
                let handoff =
                    log_fatal(nebula_tui::run_tui(), &nebula_core::paths::tui_log_path())?;
                match handoff {
                    // Hosts-picker handoff: the TUI quit and restored the
                    // terminal so a fresh `nebula ssh` can exec over us (the
                    // local daemon and its sessions stay up).
                    Some(entry) => {
                        eprintln!("nebula: connecting to {}…", entry.host);
                        ssh::run_ssh(&entry.host, entry.path.as_deref(), true)
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
