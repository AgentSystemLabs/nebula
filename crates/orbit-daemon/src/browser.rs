//! BROWSER TESTING: Claude sessions test web pages through Vercel's
//! agent-browser CLI, on a cheaper model, instead of the Claude in Chrome
//! extension.
//!
//! When the setting is on (`"browser_testing"`, default true) and
//! agent-browser is installed — or npm is, and orbit installs it once per
//! machine — a built-in Claude spawn gets:
//!
//! - two subagents through `--agents`: `browser-check` on Haiku for quick
//!   single-page checks, `browser-test` on Sonnet for multi-step flows.
//!   Claude picks one by its description, so the choice follows the task;
//! - `--disallowedTools=mcp__claude-in-chrome__*`, which takes every Claude
//!   in Chrome tool out of the session (a per-launch flag: nothing is
//!   written to settings files, so Claude outside orbit is untouched);
//! - a system-prompt note to delegate browser work to those subagents
//!   ([`CLAUDE_GUIDANCE`]), and a pre-approved `Bash(agent-browser:*)`.
//!
//! A spawn that finds agent-browser missing starts its one-time install in
//! the background ([`install_in_background`]), so it is there by the time a
//! subagent needs it. The spawn itself never waits on it.

use std::sync::atomic::{AtomicBool, Ordering};

use orbit_core::harness::HarnessDescriptor;
use orbit_core::tools::AGENT_BROWSER;
use orbit_core::AgentKind;

/// The quick, single-page subagent (Haiku).
pub const CHECK_AGENT: &str = "browser-check";
/// The multi-step subagent (Sonnet).
pub const TEST_AGENT: &str = "browser-test";

/// Every tool of the Claude in Chrome MCP server, as a permission rule
/// matches it (`mcp__<server>__*`).
pub const CHROME_EXTENSION_TOOLS: &str = "mcp__claude-in-chrome__*";

/// Appended to Claude's system prompt when browser testing is on.
pub const CLAUDE_GUIDANCE: &str = "[orbit] For any browser testing or checking of a web page, \
delegate to a subagent instead of driving a browser from this conversation: `browser-check` for a \
quick single-page check (open a URL, confirm something renders or a text appears, read a value, \
take a screenshot), `browser-test` for a multi-step flow (logging in, filling and submitting forms, \
navigating across pages, reproducing a UI bug, end-to-end checks). Give it the URL and exactly \
what to verify. Do not use the Claude in Chrome extension.";

/// How a shell types agent-browser on this machine (`agent-browser` until
/// it is installed, so the instructions still read sensibly).
pub fn shell_command() -> String {
    AGENT_BROWSER
        .installed()
        .map(|found| found.shell)
        .unwrap_or_else(|| AGENT_BROWSER.name.into())
}

/// Whether browser testing is on: the setting, and agent-browser installed
/// or npm there to install it.
pub fn active() -> bool {
    crate::config::Config::load().browser_testing && AGENT_BROWSER.available()
}

/// How agent-browser is typed, when a spawn on `harness` should get
/// browser testing: built-in Claude only (`--agents` and
/// `--disallowedTools` are Claude Code's), with the feature [`active`].
/// Starts the one-time install when the tool isn't there yet.
pub fn for_spawn(harness: &HarnessDescriptor) -> Option<String> {
    if !applies_to(harness) || !active() {
        return None;
    }
    if AGENT_BROWSER.installed().is_none() {
        install_in_background();
    }
    Some(shell_command())
}

/// Whether `harness` takes browser testing's flags: the built-in Claude
/// entry (its program may be repointed, but it is still Claude Code), not
/// a custom CLI that merely speaks Claude's hooks.
pub fn applies_to(harness: &HarnessDescriptor) -> bool {
    harness.id == AgentKind::Claude.as_str()
}

/// Pre-approved permission rules for agent-browser, spelled the way this
/// machine runs it; none when browser testing is off.
pub fn allow_rules() -> Vec<String> {
    if active() {
        vec![format!("Bash({}:*)", shell_command())]
    } else {
        Vec::new()
    }
}

/// The CLI arguments that give a Claude session browser testing:
/// `--agents <json>` defining both subagents, then the Chrome extension's
/// tools denied. The deny binds with `=` because `--disallowedTools` takes
/// a list: a separate item would also swallow a trailing positional
/// prompt as another tool name.
pub fn claude_args(shell: &str) -> Vec<String> {
    vec![
        "--agents".into(),
        agents_json(shell),
        format!("--disallowedTools={CHROME_EXTENSION_TOOLS}"),
    ]
}

/// The `--agents` value: each key a subagent's name, each value its
/// `description` (what Claude chooses by), `prompt`, `tools` and `model`.
pub fn agents_json(shell: &str) -> String {
    let agent = |model: &str, description: &str, scope: &str| {
        serde_json::json!({
            "description": description,
            "prompt": subagent_prompt(shell, scope),
            "tools": ["Bash", "Read"],
            "model": model,
        })
    };
    serde_json::json!({
        CHECK_AGENT: agent(
            "haiku",
            "Quick, single-page browser checks: open a URL, confirm something renders or a text \
appears, read a value, take a screenshot. Use for simple checks of a web page.",
            "a quick check of a single page",
        ),
        TEST_AGENT: agent(
            "sonnet",
            "Multi-step browser testing: logging in, filling and submitting forms, navigating \
across pages, reproducing a UI bug, end-to-end checks. Use when a check takes more than one step.",
            "a multi-step browser test",
        ),
    })
    .to_string()
}

fn subagent_prompt(shell: &str, scope: &str) -> String {
    format!(
        "You do {scope} with the agent-browser CLI, through Bash. Run every browser command as \
`{shell} <command>` — never another browser tool.\n\n\
First, once, run `{shell} skills get core` and follow its instructions.\n\n\
The loop: `{shell} open <url>`, then `{shell} snapshot -i` to list the interactive elements, each \
with a ref like [ref=e1]; act on a ref from the latest snapshot (`{shell} click @e1`, \
`{shell} fill @e2 \"text\"`); then snapshot again. Refs go stale after navigation or any change \
to the page, so re-snapshot before reusing one. If a click fails because something covers the \
target (a consent banner, a modal), dismiss it, re-snapshot and retry. `{shell} get url` reads the \
current URL and `{shell} screenshot` saves a picture. When unsure of a command's syntax, run \
`{shell} <command> --help`. If the command is not found, orbit may still be installing it: wait a \
minute and retry.\n\n\
Always finish with `{shell} close`, pass or fail.\n\n\
Report concisely: what you checked, what you saw, pass or fail, and the path of every \
screenshot you took."
    )
}

/// Install agent-browser once, off the caller's thread, when there is a
/// runtime to run it on. A second call while one install runs does nothing.
pub fn install_in_background() {
    static INSTALLING: AtomicBool = AtomicBool::new(false);
    let Ok(handle) = tokio::runtime::Handle::try_current() else {
        return;
    };
    if INSTALLING.swap(true, Ordering::SeqCst) {
        return;
    }
    handle.spawn_blocking(|| {
        match AGENT_BROWSER.ensure() {
            Ok(found) => tracing::info!(at = %found.program.display(), "agent-browser installed"),
            Err(why) => tracing::warn!(%why, "agent-browser: install failed"),
        }
        INSTALLING.store(false, Ordering::SeqCst);
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subagents_pick_their_model_and_use_the_shell_command() {
        let shell = "/opt/x/agent-browser";
        let agents: serde_json::Value = serde_json::from_str(&agents_json(shell)).unwrap();
        assert_eq!(agents.as_object().unwrap().len(), 2);
        assert_eq!(agents[CHECK_AGENT]["model"], "haiku");
        assert_eq!(agents[TEST_AGENT]["model"], "sonnet");
        for name in [CHECK_AGENT, TEST_AGENT] {
            let agent = &agents[name];
            assert!(agent["description"].as_str().unwrap().len() > 20);
            assert_eq!(agent["tools"], serde_json::json!(["Bash", "Read"]));
            let prompt = agent["prompt"].as_str().unwrap();
            assert!(
                prompt.contains(&format!("`{shell} skills get core`")),
                "{prompt}"
            );
            assert!(prompt.contains(&format!("`{shell} close`")), "{prompt}");
        }
    }

    #[test]
    fn the_chrome_extension_is_denied_in_one_bound_argument() {
        let args = claude_args("agent-browser");
        assert_eq!(args[0], "--agents");
        assert_eq!(args[2], "--disallowedTools=mcp__claude-in-chrome__*");
        assert_eq!(args.len(), 3);
    }
}
