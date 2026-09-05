pub mod agent_picker;
pub mod agent_presets;
pub mod app;
pub mod branch_name;
pub mod claude_catalogue;
pub mod completion;
pub mod config;
pub mod cursor_catalogue;
pub mod event_loop;
pub mod file_tabs;
pub mod fuzzy;
pub mod git_diff;
pub mod grep_search;
pub mod hosts;
pub mod ipc;
pub mod keymap;
pub mod keys;
pub mod links;
pub mod overlay_close;
pub mod palette;
pub mod pr_preview;
pub mod preset_overlays;
pub mod pull_request;
pub mod quick_prompt;
pub mod raw_attach;
pub mod remote;
pub mod review;
pub mod splash;
pub mod syntax;
pub mod text_input;
pub mod theme;
pub mod tree_browser;
pub mod ui;
pub mod update_check;
pub mod vim_term;

use anyhow::Result;

fn runtime() -> Result<tokio::runtime::Runtime> {
    Ok(tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()?)
}

/// Entry point for the TUI client. Terminal setup/teardown lives here so the
/// binary crate stays a thin arg-parser. `Some(entry)` means the user picked
/// a recent ssh host — the terminal is restored and the caller should exec
/// `nebula ssh` at it.
///
/// `workspace` is `--workspace <name>`: which workspace this instance opens
/// into, independent of any other instance already running.
pub fn run_tui(workspace: Option<String>) -> Result<Option<hosts::HostEntry>> {
    runtime()?.block_on(event_loop::run_app(workspace))
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

/// `nebula rename` — agent-side session titling (see `ipc::rename_current_agent`).
/// `mode` is the CLI's `--force`, decided where the flag is parsed.
pub fn run_rename(title: String, mode: RenameMode) -> Result<()> {
    runtime()?.block_on(ipc::rename_current_agent(&title, mode))
}

/// `nebula worktree [name] [--base <ref>]` — move the current agent session
/// into a worktree of its project (see `ipc::enter_worktree_for_current_agent`).
pub fn run_worktree(name: String, base: Option<String>) -> Result<()> {
    runtime()?.block_on(ipc::enter_worktree_for_current_agent(&name, base))
}

/// `nebula open <file>…` — show files in every attached TUI's FILE TABS
/// (see `ipc::open_files_for_current_agent`).
pub fn run_open(files: Vec<String>) -> Result<()> {
    runtime()?.block_on(ipc::open_files_for_current_agent(&files))
}

/// `nebula spawn "<task>" [--kind <kind>]` — start a new agent session
/// beside the current one (see `ipc::spawn_sibling_for_current_agent`).
/// `kind` is the CLI's `--kind`, already parsed where the flag is.
pub fn run_spawn(task: String, kind: Option<nebula_core::AgentKind>) -> Result<()> {
    runtime()?.block_on(ipc::spawn_sibling_for_current_agent(&task, kind))
}

/// `nebula add <dir>` / bare `nebula <dir>` — register a directory as a
/// project (see `ipc::add_project`).
pub fn run_add_project(path: String) -> Result<()> {
    runtime()?.block_on(ipc::add_project(&path))
}

pub use ipc::{RenameMode, WorkspaceOp};

/// `nebula workspace <add|open|list|delete|rename>` (see `ipc::run_workspace_op`).
pub fn run_workspace(op: WorkspaceOp) -> Result<()> {
    runtime()?.block_on(ipc::run_workspace_op(op))
}

/// `nebula kill`.
pub fn run_kill() -> Result<()> {
    runtime()?.block_on(async {
        if ipc::kill_daemon().await? {
            println!("nebula daemon shut down");
        } else {
            println!("no nebula daemon running");
        }
        Ok(())
    })
}
