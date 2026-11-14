//! `orbit doctor`: what this machine has, what it is missing, and what each
//! missing piece would unlock — so a new user sees their setup in one place.
//!
//! Orbit asks for little: `git` and `claude` to work at all, then `uv` and
//! `npm`, through which it fetches the tools it integrates (graphify via
//! `uvx`, agent-browser via `npx`) on first use. `--fix` does those fetches
//! now — graphify into uv's cache, agent-browser and its Chrome for Testing
//! into npm's — and nothing else: it never installs system packages.

use anyhow::Result;
use orbit_core::tools::{self, Invocation, Via};
use std::path::PathBuf;

/// One line of the report.
struct Row {
    name: &'static str,
    found: Option<String>,
    unlocks: &'static str,
    get: &'static str,
}

fn tool(name: &'static str, unlocks: &'static str, get: &'static str) -> Row {
    Row {
        name,
        found: tools::find(name).map(|p| tidy(&p)),
        unlocks,
        get,
    }
}

fn integration(
    name: &'static str,
    found: Option<Invocation>,
    unlocks: &'static str,
    get: &'static str,
) -> Row {
    Row {
        name,
        found: found.map(|i| match i.via {
            Via::Installed => tidy(&i.program),
            _ => i.describe(),
        }),
        unlocks,
        get,
    }
}

/// `~/…` instead of the full home path, for a narrower report.
fn tidy(path: &std::path::Path) -> String {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    match home.and_then(|h| path.strip_prefix(&h).ok().map(|rest| rest.to_path_buf())) {
        Some(rest) => format!("~/{}", rest.display()),
        None => path.display().to_string(),
    }
}

fn section(title: &str, rows: &[Row]) -> usize {
    println!("\n{title}");
    let mut missing = 0;
    for row in rows {
        match &row.found {
            Some(at) => println!("  ✓ {:<14} {at}", row.name),
            None => {
                missing += 1;
                println!("  ✗ {:<14} {} — {}", row.name, row.unlocks, row.get);
            }
        }
    }
    missing
}

pub fn run(fix: bool) -> Result<()> {
    println!("orbit doctor");

    let required = [
        tool(
            "git",
            "projects and worktrees",
            "xcode-select --install (macOS) or your package manager",
        ),
        tool(
            "claude",
            "the agents themselves",
            "curl -fsSL https://claude.ai/install.sh | bash",
        ),
        tool("curl", "session status reporting", "your package manager"),
    ];
    let missing_required = section("Required", &required);

    let runners = [
        tool(
            "uv",
            "fetches graphify for you",
            "curl -LsSf https://astral.sh/uv/install.sh | sh",
        ),
        tool(
            "npx",
            "fetches agent-browser for you",
            "install Node.js (npm comes with it)",
        ),
    ];
    section(
        "Tool runners (Orbit fetches its tools through these)",
        &runners,
    );

    let integrations = [
        integration(
            "graphify",
            tools::graphify(),
            "the code graph Claude queries, `k`",
            "needs uv (above)",
        ),
        integration(
            "agent-browser",
            tools::agent_browser(),
            "browser testing for Claude",
            "needs npm (above)",
        ),
        tool(
            "gh",
            "pull requests (v), issues (i)",
            "brew install gh && gh auth login",
        ),
        tool(
            "mmdc",
            "Mermaid diagrams drawn as pictures",
            "npm i -g @mermaid-js/mermaid-cli",
        ),
    ];
    section("Integrations", &integrations);

    let remote = [
        tool("ssh", "orbit ssh / orbit tunnel", "OpenSSH client"),
        tool("ttyd", "orbit browser", "brew install ttyd"),
        tool("docker", "orbit sbx", "Docker Desktop"),
    ];
    section("Other machines (optional)", &remote);

    if fix {
        println!();
        prefetch();
    }

    println!();
    if missing_required > 0 {
        println!("{missing_required} required tool(s) missing — Orbit can't run agents until they're installed.");
        std::process::exit(1);
    }
    if fix {
        println!("Everything required is installed, and the tools Orbit fetches are ready.");
    } else {
        println!("Everything required is installed. `orbit doctor --fix` fetches graphify and agent-browser (with its browser) now instead of on first use.");
    }
    Ok(())
}

/// Fetch through uv / npm now what Orbit would otherwise fetch on first use.
fn prefetch() {
    match tools::graphify() {
        Some(graphify) => {
            println!("Fetching graphify ({}) …", graphify.shell);
            report(graphify.command().arg("--help"), "graphify");
        }
        None => println!("Skipping graphify: install uv first."),
    }
    match tools::agent_browser() {
        Some(browser) => {
            println!(
                "Fetching agent-browser and its browser ({} install) …",
                browser.shell
            );
            report(browser.command().arg("install"), "agent-browser");
        }
        None => println!("Skipping agent-browser: install Node.js (npm) first."),
    }
}

fn report(command: &mut std::process::Command, what: &str) {
    // Progress goes straight to the terminal; only the outcome is ours.
    let quiet_stdout = what == "graphify";
    if quiet_stdout {
        command.stdout(std::process::Stdio::null());
    }
    match command.status() {
        Ok(status) if status.success() => println!("  ✓ {what} ready"),
        Ok(status) => println!("  ✗ {what}: exited with {status}"),
        Err(e) => println!("  ✗ {what}: could not start ({e})"),
    }
}
