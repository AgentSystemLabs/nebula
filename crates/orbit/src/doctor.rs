//! `orbit doctor`: what this machine has, what it is missing, and what each
//! missing piece would unlock — so a new user sees their setup in one place.
//!
//! Orbit asks for little: `git` and `claude` to work at all, then `uv` and
//! `npm`, through which it installs the tools it integrates — once per
//! machine, the first time each is needed (`uv tool install graphifyy`,
//! `npm install -g agent-browser`). `--fix` does those installs now instead,
//! and nothing else: it never installs system packages.

use anyhow::Result;
use orbit_core::tools::{self, Tool};
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

/// An integrated tool: installed, or installed once on first use when the
/// package manager is here, or missing that package manager.
fn integration(tool: &Tool, unlocks: &'static str, needs: &'static str) -> Row {
    let found = match tool.installed() {
        Some(at) => Some(tidy(&at.program)),
        None if tool.installer_program().is_some() => Some(format!(
            "not yet — installed once when first needed (`{}`)",
            tool.install_hint()
        )),
        None => None,
    };
    Row {
        name: tool.name,
        found,
        unlocks,
        get: needs,
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
            "installs graphify for you",
            "curl -LsSf https://astral.sh/uv/install.sh | sh",
        ),
        tool(
            "npm",
            "installs agent-browser for you",
            "install Node.js (npm comes with it)",
        ),
    ];
    section(
        "Package managers (Orbit installs its tools through these, once)",
        &runners,
    );

    let integrations = [
        integration(
            &tools::GRAPHIFY,
            "the code graph Claude queries, `k`",
            "needs uv (above)",
        ),
        integration(
            &tools::AGENT_BROWSER,
            "browser testing for Claude (Haiku / Sonnet subagents)",
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
        println!("Everything required is installed, and the tools Orbit integrates are ready.");
    } else {
        println!("Everything required is installed. `orbit doctor --fix` installs graphify and agent-browser now instead of when first needed.");
    }
    Ok(())
}

/// Install now, once, what Orbit would otherwise install when first needed.
fn prefetch() {
    for tool in [&tools::GRAPHIFY, &tools::AGENT_BROWSER] {
        if tool.installed().is_some() {
            println!("  ✓ {} already installed", tool.name);
            continue;
        }
        println!("Installing {} (`{}`) …", tool.name, tool.install_hint());
        match tool.ensure() {
            Ok(at) => println!("  ✓ {} installed at {}", tool.name, tidy(&at.program)),
            Err(why) => println!("  ✗ {why}"),
        }
    }
    // agent-browser's browser download is its own one-time step; it is a
    // no-op when the browser is already there.
    if let Some(browser) = tools::AGENT_BROWSER.installed() {
        let ok = browser
            .command()
            .arg("install")
            .stdout(std::process::Stdio::null())
            .status()
            .is_ok_and(|s| s.success());
        if ok {
            println!("  ✓ agent-browser's browser is ready");
        } else {
            println!(
                "  ✗ agent-browser couldn't download its browser — run `agent-browser install`"
            );
        }
    }
}
