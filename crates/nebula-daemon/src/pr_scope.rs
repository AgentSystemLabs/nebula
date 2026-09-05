//! The PR SESSION's launch context: the scope rule an AGENT created from an
//! OPEN PRS row carries, and the shape it takes for each AGENT KIND's CLI.
//!
//! The rule is regenerated from the persisted URL for every fresh process,
//! so a RESUME cannot silently lose the scope the user chose at creation
//! time. Claude and pi take it as an appended system prompt on every spawn.
//! Codex and Cursor have no system-prompt flag, so on their cold spawn it
//! becomes the positional first prompt — their transcripts keep it, so a
//! resume of either needs nothing added.

use anyhow::{bail, Result};
use nebula_core::AgentKind;

/// The invariant attached to an AGENT created from an OPEN PRS row.
pub(crate) fn rule(pr_url: &str) -> String {
    format!(
        "[nebula] This session was created from the OPEN PRS row for {pr_url}. All work in this \
         session must be scoped to that pull request. Inspect the PR before acting, and do not \
         modify or report on unrelated work. Before editing, make sure changes are made on the \
         PR's head branch (using a dedicated worktree if necessary), never in an unrelated \
         checkout. Keep reviews, tests, commits, pushes, and GitHub actions limited to this PR."
    )
}

/// The rule as the first prompt of a CLI with no system-prompt flag: the
/// same text, plus a line that keeps the agent from treating it as a task.
fn rule_as_first_prompt(pr_url: &str) -> String {
    format!(
        "{}\n\nThis message is context, not a task: acknowledge it in one line and wait for the \
         user's request.",
        rule(pr_url)
    )
}

/// What the argv builder gets once the PR rule is folded in: the text to
/// append to the system prompt, and the first prompt the CLI submits on
/// its own.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct LaunchPrompts {
    pub system: Option<String>,
    pub initial: Option<String>,
}

/// Fold the PR rule (when the AGENT carries a URL) into a spawn's prompts.
/// `initial` is the first prompt the spawn already had — a RELOCATION
/// PROMPT or an AGENT PRESET's composed task — and stays where it was;
/// `resumed` tells a Codex / Cursor resume (which needs no rule) from a
/// cold spawn (which opens with it).
pub(crate) fn launch_prompts(
    kind: AgentKind,
    resumed: bool,
    pr_url: Option<&str>,
    initial: Option<&str>,
) -> LaunchPrompts {
    let initial_owned = initial.map(str::to_string);
    let Some(pr_url) = pr_url else {
        return LaunchPrompts {
            system: None,
            initial: initial_owned,
        };
    };
    match kind {
        AgentKind::Claude | AgentKind::Pi => LaunchPrompts {
            system: Some(rule(pr_url)),
            initial: initial_owned,
        },
        // Every other CLI has no system-prompt flag: the rule opens a cold
        // spawn as its first prompt, and a resume's transcript already
        // holds it.
        _ if resumed => LaunchPrompts {
            system: None,
            initial: initial_owned,
        },
        _ => LaunchPrompts {
            system: None,
            initial: Some(match initial {
                Some(task) => format!("{}\n\n{task}", rule(pr_url)),
                None => rule_as_first_prompt(pr_url),
            }),
        },
    }
}

/// Validate the persisted URL before it becomes part of a CLI's argv on
/// every spawn. OPEN PRS rows already supply HTTP(S), but the DAEMON treats
/// IPC as a real boundary and rechecks the invariant itself.
pub(crate) fn validate_pr_url(raw: &str) -> Result<String> {
    const MAX_PR_URL_BYTES: usize = 4 * 1024;
    let url = crate::registry::normalize_url(raw)?;
    if url.len() > MAX_PR_URL_BYTES {
        bail!("pull request URL is too long (max 4 KiB)");
    }
    if !url.contains("/pull/") {
        bail!("not a pull request URL: {url}");
    }
    Ok(url)
}

#[cfg(test)]
mod tests {
    use super::*;

    const URL: &str = "https://github.com/AgentSystemLabs/nebula/pull/42";

    #[test]
    fn claude_and_pi_take_the_rule_as_a_system_prompt_and_keep_their_first_prompt() {
        for kind in [AgentKind::Claude, AgentKind::Pi] {
            for resumed in [false, true] {
                let prompts = launch_prompts(kind, resumed, Some(URL), Some("relocated"));
                assert_eq!(
                    prompts.system.as_deref(),
                    Some(rule(URL).as_str()),
                    "{kind:?}"
                );
                assert_eq!(prompts.initial.as_deref(), Some("relocated"), "{kind:?}");
            }
        }
    }

    #[test]
    fn codex_and_cursor_open_a_cold_spawn_with_the_rule_and_resume_without_it() {
        for kind in [AgentKind::Codex, AgentKind::Cursor] {
            let cold = launch_prompts(kind, false, Some(URL), None);
            assert_eq!(cold.system, None, "{kind:?} has no system-prompt flag");
            let first = cold.initial.expect("the rule is the first prompt");
            assert!(first.starts_with(&rule(URL)), "{first}");
            assert!(first.contains("wait for the user's request"), "{first}");

            let with_task = launch_prompts(kind, false, Some(URL), Some("fix the tests"));
            assert_eq!(
                with_task.initial.as_deref(),
                Some(format!("{}\n\nfix the tests", rule(URL)).as_str())
            );

            let resumed = launch_prompts(kind, true, Some(URL), None);
            assert_eq!(
                resumed,
                LaunchPrompts::default(),
                "the transcript carries the rule through a resume"
            );
        }
    }

    #[test]
    fn no_pr_url_changes_nothing() {
        for kind in AgentKind::ALL {
            let prompts = launch_prompts(kind, false, None, Some("task"));
            assert_eq!(
                prompts,
                LaunchPrompts {
                    system: None,
                    initial: Some("task".into()),
                }
            );
        }
    }

    #[test]
    fn validate_pr_url_normalizes_and_refuses_non_pull_urls() {
        assert_eq!(
            validate_pr_url("github.com/o/r/pull/7").unwrap(),
            "https://github.com/o/r/pull/7"
        );
        assert!(validate_pr_url("https://github.com/o/r/issues/7").is_err());
        assert!(validate_pr_url("javascript:alert(1)").is_err());
    }
}
