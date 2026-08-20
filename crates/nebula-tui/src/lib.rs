pub mod app;
pub mod completion;
pub mod config;
pub mod event_loop;
pub mod fuzzy;
pub mod git_diff;
pub mod grep_search;
pub mod ipc;
pub mod keys;
pub mod links;
pub mod raw_attach;
pub mod syntax;
pub mod theme;
pub mod tree_browser;
pub mod ui;
pub mod vim_term;

use anyhow::Result;

fn runtime() -> Result<tokio::runtime::Runtime> {
    Ok(tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()?)
}

/// Entry point for the TUI client. Terminal setup/teardown lives here so the
/// binary crate stays a thin arg-parser.
pub fn run_tui() -> Result<()> {
    runtime()?.block_on(event_loop::run_app())
}

/// Phase-2 throwaway raw-mode client (`nebula _raw-attach`).
pub fn run_raw_attach(name: &str) -> Result<()> {
    runtime()?.block_on(raw_attach::run(name))
}

/// Post-upgrade daemon handoff: shut the daemon down only when it holds no
/// live sessions (see `ipc::shutdown_if_idle`).
pub fn shutdown_daemon_if_idle() -> Result<ipc::IdleShutdown> {
    runtime()?.block_on(ipc::shutdown_if_idle())
}

/// `nebula kill-server`.
pub fn run_kill_server() -> Result<()> {
    runtime()?.block_on(async {
        if ipc::kill_server().await? {
            println!("nebula daemon shut down");
        } else {
            println!("no nebula daemon running");
        }
        Ok(())
    })
}
