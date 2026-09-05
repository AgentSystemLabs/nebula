//! The PR SESSION: an AGENT created from an OPEN PRS row. This module owns
//! its launch — the worktree it runs in and the scope rule it carries —
//! and the shape that rule takes for each AGENT KIND's CLI.
//!
//! A PR SESSION never runs in the ROOT WORKTREE. `Daemon::create_pr_agent`
//! finds the PROJECT's worktree checked out on the PR's head branch, or
//! creates one, and every PR SESSION for that PR shares it; the rule names
//! that checkout so the agent works there and nowhere else.
//!
//! The rule is regenerated from the persisted URL and the row's current
//! worktree for every fresh process, so a RESUME cannot silently lose the
//! scope the user chose at creation time. Claude and pi take it as an
//! appended system prompt on every spawn. Codex and Cursor have no
//! system-prompt flag, so on their cold spawn it becomes the positional
//! first prompt — their transcripts keep it, so a resume of either needs
//! nothing added.

use std::path::Path;
use std::sync::Arc;

use anyhow::{bail, Context, Result};
use nebula_core::{AgentKind, EntityId, ProjectId};

use crate::registry::{CreateAgentSpec, Daemon};

/// Everything the rule says about where a PR SESSION works: the PR, the
/// checkout it runs in, and the main checkout it must leave alone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PrScope<'a> {
    pub url: &'a str,
    /// The worktree the session runs in.
    pub worktree: &'a Path,
    /// That worktree's branch — the PR's head.
    pub branch: &'a str,
    /// The PROJECT's ROOT WORKTREE, when it is a different checkout: named
    /// as the place *not* to work. None when the PR's branch is checked
    /// out in the root itself, which is then the only checkout of it.
    pub root: Option<&'a Path>,
}

/// The invariant attached to an AGENT created from an OPEN PRS row.
pub(crate) fn rule(scope: &PrScope<'_>) -> String {
    let PrScope {
        url,
        worktree,
        branch,
        root,
    } = scope;
    let elsewhere = match root {
        Some(root) => format!(
            " — never in the main checkout at {} or any other checkout",
            root.display()
        ),
        None => String::new(),
    };
    format!(
        "[nebula] This session was created from the OPEN PRS row for {url}. All work in this \
         session must be scoped to that pull request. It runs in the worktree at {wt}, checked \
         out on the PR's head branch `{branch}`: do every edit, test, commit and push there{elsewhere}. \
         Other sessions may share this worktree, so pull before you push. Inspect the PR before \
         acting, and do not modify or report on unrelated work. Keep reviews, tests, commits, \
         pushes, and GitHub actions limited to this PR.",
        wt = worktree.display(),
    )
}

/// The rule as the first prompt of a CLI with no system-prompt flag: the
/// same text, plus a line that keeps the agent from treating it as a task.
fn rule_as_first_prompt(scope: &PrScope<'_>) -> String {
    format!(
        "{}\n\nThis message is context, not a task: acknowledge it in one line and wait for the \
         user's request.",
        rule(scope)
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

/// Fold the PR rule (when the AGENT carries a scope) into a spawn's
/// prompts. `initial` is the first prompt the spawn already had — a
/// RELOCATION PROMPT or an AGENT PRESET's composed task — and stays where
/// it was; `resumed` tells a Codex / Cursor resume (which needs no rule)
/// from a cold spawn (which opens with it).
pub(crate) fn launch_prompts(
    kind: AgentKind,
    resumed: bool,
    scope: Option<&PrScope<'_>>,
    initial: Option<&str>,
) -> LaunchPrompts {
    let initial_owned = initial.map(str::to_string);
    let Some(scope) = scope else {
        return LaunchPrompts {
            system: None,
            initial: initial_owned,
        };
    };
    match kind {
        AgentKind::Claude | AgentKind::Pi => LaunchPrompts {
            system: Some(rule(scope)),
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
                Some(task) => format!("{}\n\n{task}", rule(scope)),
                None => rule_as_first_prompt(scope),
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

/// The pull request's number, read off its validated URL (`…/pull/42`,
/// with or without a trailing path).
pub(crate) fn pr_number(url: &str) -> Result<u64> {
    let (_, tail) = url.split_once("/pull/").context("not a pull request URL")?;
    let digits = tail.split(['/', '?', '#']).next().unwrap_or_default();
    digits
        .parse::<u64>()
        .ok()
        .filter(|n| *n > 0)
        .with_context(|| format!("no pull request number in {url}"))
}

/// The PR's head branch as `gh` reports it, checked before it becomes a
/// git argument and a directory name: one line, no shell-ish characters,
/// not an option.
pub(crate) fn validate_head(raw: &str) -> Result<String> {
    const MAX_HEAD_BYTES: usize = 256;
    let head = raw.trim();
    if head.is_empty() {
        bail!("the pull request has no head branch");
    }
    if head.len() > MAX_HEAD_BYTES {
        bail!("head branch name is too long (max {MAX_HEAD_BYTES} bytes)");
    }
    if head.starts_with('-')
        || head
            .chars()
            .any(|c| c.is_whitespace() || c.is_control() || matches!(c, '~' | '^' | ':' | '\\'))
    {
        bail!("not a branch name: {head:?}");
    }
    Ok(head.to_string())
}

/// What `ClientRequest::CreatePrAgent` carries: `CreateAgentSpec` minus
/// the worktree the daemon picks itself, plus the PR the rule is for.
pub(crate) struct CreatePrAgentSpec {
    pub project: ProjectId,
    pub name: String,
    pub kind: AgentKind,
    pub model: Option<String>,
    pub effort: Option<String>,
    pub auto_title: bool,
    pub pr_url: String,
    pub head: String,
}

impl Daemon {
    /// Create a PR SESSION: put a checkout of the PR's head branch under
    /// the PROJECT (or find the one already there), then create the AGENT
    /// in it with the PR URL as its launch context.
    pub(crate) async fn create_pr_agent(
        self: &Arc<Self>,
        spec: CreatePrAgentSpec,
    ) -> Result<EntityId> {
        let CreatePrAgentSpec {
            project,
            name,
            kind,
            model,
            effort,
            auto_title,
            pr_url,
            head,
        } = spec;
        let pr_url = validate_pr_url(&pr_url)?;
        let number = pr_number(&pr_url)?;
        let head = validate_head(&head)?;
        let worktree = self.pr_worktree(&project, number, &head).await?;
        self.create_agent(CreateAgentSpec {
            worktree: worktree.id,
            name,
            kind,
            model,
            effort,
            auto_title,
            cloud_prompt: None,
            starting_prompt: None,
            pr_url: Some(pr_url),
        })
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    const URL: &str = "https://github.com/AgentSystemLabs/nebula/pull/42";

    fn scope<'a>(root: Option<&'a Path>) -> PrScope<'a> {
        PrScope {
            url: URL,
            worktree: Path::new("/w/nebula-worktrees/fix-login"),
            branch: "fix-login",
            root,
        }
    }

    #[test]
    fn the_rule_names_the_worktree_its_branch_and_the_root_to_avoid() {
        let root = PathBuf::from("/w/nebula");
        let text = rule(&scope(Some(&root)));
        assert!(text.contains(URL), "{text}");
        assert!(text.contains("/w/nebula-worktrees/fix-login"), "{text}");
        assert!(text.contains("`fix-login`"), "{text}");
        assert!(
            text.contains("never in the main checkout at /w/nebula"),
            "{text}"
        );
        assert!(text.contains("pull before you push"), "{text}");

        // The PR's branch checked out in the root itself: nothing to avoid.
        let alone = rule(&scope(None));
        assert!(!alone.contains("main checkout"), "{alone}");
        assert!(alone.contains("/w/nebula-worktrees/fix-login"), "{alone}");
    }

    #[test]
    fn claude_and_pi_take_the_rule_as_a_system_prompt_and_keep_their_first_prompt() {
        let scope = scope(None);
        for kind in [AgentKind::Claude, AgentKind::Pi] {
            for resumed in [false, true] {
                let prompts = launch_prompts(kind, resumed, Some(&scope), Some("relocated"));
                assert_eq!(
                    prompts.system.as_deref(),
                    Some(rule(&scope).as_str()),
                    "{kind:?}"
                );
                assert_eq!(prompts.initial.as_deref(), Some("relocated"), "{kind:?}");
            }
        }
    }

    #[test]
    fn codex_and_cursor_open_a_cold_spawn_with_the_rule_and_resume_without_it() {
        let scope = scope(None);
        for kind in [AgentKind::Codex, AgentKind::Cursor] {
            let cold = launch_prompts(kind, false, Some(&scope), None);
            assert_eq!(cold.system, None, "{kind:?} has no system-prompt flag");
            let first = cold.initial.expect("the rule is the first prompt");
            assert!(first.starts_with(&rule(&scope)), "{first}");
            assert!(first.contains("wait for the user's request"), "{first}");

            let with_task = launch_prompts(kind, false, Some(&scope), Some("fix the tests"));
            assert_eq!(
                with_task.initial.as_deref(),
                Some(format!("{}\n\nfix the tests", rule(&scope)).as_str())
            );

            let resumed = launch_prompts(kind, true, Some(&scope), None);
            assert_eq!(
                resumed,
                LaunchPrompts::default(),
                "the transcript carries the rule through a resume"
            );
        }
    }

    #[test]
    fn no_scope_changes_nothing() {
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

    #[test]
    fn pr_number_reads_the_url_tail() {
        assert_eq!(pr_number("https://github.com/o/r/pull/7").unwrap(), 7);
        assert_eq!(
            pr_number("https://github.com/o/r/pull/42/files").unwrap(),
            42
        );
        assert_eq!(
            pr_number("https://github.com/o/r/pull/9?diff=split").unwrap(),
            9
        );
        assert!(pr_number("https://github.com/o/r/pull/").is_err());
        assert!(pr_number("https://github.com/o/r/pull/0").is_err());
        assert!(pr_number("https://github.com/o/r/pull/abc").is_err());
    }

    #[test]
    fn validate_head_takes_branch_names_and_refuses_options_and_spaces() {
        assert_eq!(validate_head(" feat/login ").unwrap(), "feat/login");
        assert_eq!(validate_head("their-fix.v2").unwrap(), "their-fix.v2");
        for bad in [
            "",
            "  ",
            "-b",
            "a b",
            "a\nb",
            "a:b",
            "x".repeat(300).as_str(),
        ] {
            assert!(validate_head(bad).is_err(), "{bad:?}");
        }
    }
}
