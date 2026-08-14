pub mod app;
pub mod completion;
pub mod event_loop;
pub mod git_diff;
pub mod ipc;
pub mod keys;
pub mod raw_attach;
pub mod ui;

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
