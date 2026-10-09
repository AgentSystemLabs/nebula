//! `nebula spawn "<task>"` from inside an agent session: a new AGENT beside
//! the caller — same WORKTREE unless `--worktree` names another branch,
//! same harness unless another is named — that opens on the task as its
//! STARTING PROMPT. The caller's own process is never touched (unlike
//! `nebula worktree`, nothing here waits on a turn end), so the model runs
//! it, tells the user, and carries on.

use std::sync::Arc;

use anyhow::{bail, Context, Result};
use nebula_core::{Agent, AgentId, AgentKind, EntityId, WorktreeId};

use crate::registry::{CreateAgentSpec, Daemon};

/// What nebula appends to Claude's system prompt so "start a new nebula
/// session that …" becomes one `nebula spawn` call instead of the model
/// trying to launch an agent process itself. Claude and pi, like the
/// worktree guidance (both take `--append-system-prompt`): codex and cursor
/// have no system-prompt flag.
pub const CLAUDE_SPAWN_GUIDANCE: &str = "[nebula] When the user asks you to start, spin up, or hand \
off work to a separate new/parallel nebula session (\"start a new nebula session that …\", \"spin up \
another session to …\", \"open a new session for …\", \"hand this off\"), do not launch an agent \
process yourself. Run this shell command instead, exactly once:\n\n  nebula spawn \"<task>\"\n\nwhere \
<task> is the work the user wants that session to do, in their own words — the new session opens on \
it as its first prompt, so make it self-contained. Add `--kind \
claude|codex|cursor|pi|muse|grok|opencode` only when the user names the harness; otherwise the new \
session matches this one. nebula starts it beside this session, in the same worktree, and it shows \
up in the sessions list on its own. When the user wants that separate new/parallel session on its \
own branch or worktree (\"start a new session on branch fix-login\", \"spin up an agent in a new \
worktree to fix login\"), add `--worktree <branch>`: nebula starts it in the project's worktree on \
that branch, creating it when there is none (`--base <ref>` picks a new branch's start point, only \
when the user names one). Do not use `nebula spawn --worktree` for requests that ask this current \
session to \"do this in a new worktree\" or \"move to a worktree\"; follow the worktree guidance and \
run `nebula worktree <name>` for those. This session is unaffected by a spawn either way: carry on \
with whatever else the user asked, and if starting the session was the whole request, tell the user \
in one line that it is running. If the command fails, report the error.";

/// `nebula spawn --worktree <branch> [--base <ref>]`: the branch whose
/// worktree the new session starts in, and where a new branch is cut from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SiblingWorktree<'a> {
    pub branch: &'a str,
    /// Start point for a branch with no worktree yet; None is the
    /// `worktree_base_branch` SETTING, else origin's default branch.
    pub base: Option<&'a str>,
}

/// The first free `agent-N` among `taken` — the same default the TUI's
/// name prompt offers, which is what makes the new row eligible for
/// AUTO-TITLE (the daemon titles only rows created on the default name).
pub(crate) fn sibling_name(taken: &[String]) -> String {
    (1..)
        .map(|n| format!("agent-{n}"))
        .find(|candidate| !taken.contains(candidate))
        .expect("an unbounded counter always finds a free name")
}

impl Daemon {
    /// The agent running `nebula spawn`: refused when unknown or archived.
    fn spawning_caller(&self, id: &AgentId) -> Result<Agent> {
        let caller = self.store.get_agent(id)?.context("agent not found")?;
        if caller.archived {
            bail!("agent is archived");
        }
        Ok(caller)
    }

    /// The first free `agent-N` among the rows in `worktree`.
    fn free_name_in(&self, worktree: &WorktreeId) -> Result<String> {
        let (_, _, agents, _) = self.store.load_tree()?;
        let taken = agents
            .iter()
            .filter(|a| &a.worktree_id == worktree)
            .map(|a| a.name.clone())
            .collect::<Vec<_>>();
        Ok(sibling_name(&taken))
    }

    /// The harness a session started beside `caller` runs — its own (with
    /// its model, effort and custom registry id) unless `kind` names
    /// another, which drops them: a different CLI cannot take this one's
    /// model name, and an override to Custom without an id is refused at
    /// create with its reason.
    fn sibling_harness(caller: &Agent, kind: Option<AgentKind>) -> SiblingHarness {
        let kind = kind.unwrap_or(caller.kind);
        if kind == caller.kind {
            SiblingHarness {
                kind,
                custom_harness: caller.custom_harness.clone(),
                model: caller.model.clone(),
                effort: caller.effort.clone(),
            }
        } else {
            SiblingHarness {
                kind,
                custom_harness: None,
                model: None,
                effort: None,
            }
        }
    }

    /// The create spec for a session started beside `caller` in
    /// `worktree`: the caller's harness unless `kind` overrides, a default
    /// `agent-N` name free in that worktree so AUTO-TITLE applies, and
    /// `starting_prompt` as the first prompt (which `create_agent`
    /// validates). Pure lookup, so it is unit-testable without a PTY.
    pub(crate) fn sibling_spec(
        &self,
        caller: &Agent,
        kind: Option<AgentKind>,
        worktree: &WorktreeId,
        starting_prompt: &str,
    ) -> Result<CreateAgentSpec> {
        let SiblingHarness {
            kind,
            custom_harness,
            model,
            effort,
        } = Self::sibling_harness(caller, kind);
        Ok(CreateAgentSpec {
            worktree: worktree.clone(),
            name: self.free_name_in(worktree)?,
            kind,
            custom_harness,
            model,
            effort,
            auto_title: true,
            cloud_prompt: None,
            starting_prompt: Some(starting_prompt.to_string()),
            pr_url: None,
            issue_url: None,
        })
    }

    /// `nebula spawn`, run by the agent inside its own session: create and
    /// boot a new agent beside it with `starting_prompt` as its first
    /// prompt — in the caller's worktree, or with `worktree` in the
    /// caller's PROJECT's worktree on that branch, cut first when there is
    /// none. Returns the new row's id; a created worktree and the agent
    /// reach every client through the ordinary create paths.
    pub async fn spawn_sibling_agent(
        self: &Arc<Self>,
        id: &AgentId,
        kind: Option<AgentKind>,
        worktree: Option<SiblingWorktree<'_>>,
        starting_prompt: &str,
    ) -> Result<EntityId> {
        let caller = self.spawning_caller(id)?;
        let target = match worktree {
            None => caller.worktree_id.clone(),
            Some(SiblingWorktree { branch, base }) => {
                let branch = branch.trim();
                if branch.is_empty() {
                    bail!("branch name is empty");
                }
                // Whatever the create would refuse — the prompt, the
                // harness, a missing CLI — is asked before git cuts a
                // checkout nobody would then launch in.
                let harness = Self::sibling_harness(&caller, kind);
                self.check_cold_launch(
                    harness.kind,
                    harness.custom_harness.as_deref(),
                    starting_prompt,
                )
                .await?;
                let project = self
                    .store
                    .get_worktree(&caller.worktree_id)?
                    .context("worktree not found")?
                    .project_id;
                self.worktree_on_branch_for_spawn(&project, branch, base)
                    .await?
                    .id
            }
        };
        let spec = self.sibling_spec(&caller, kind, &target, starting_prompt)?;
        self.create_agent(spec).await
    }
}

/// [`Daemon::sibling_harness`]: what the new session runs.
struct SiblingHarness {
    kind: AgentKind,
    custom_harness: Option<String>,
    model: Option<String>,
    effort: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hooks::HookEnv;
    use crate::store::Store;
    use nebula_core::{Agent, AgentStatus, Project, ProjectId, Worktree, WorktreeId};

    fn daemon() -> Arc<Daemon> {
        let daemon = Daemon::new(
            Arc::new(Store::open_in_memory().unwrap()),
            HookEnv {
                port: 0,
                token: String::new(),
            },
        );
        daemon
            .store
            .insert_project(&Project {
                id: ProjectId("p".into()),
                name: "p".into(),
                repo_path: "/nebula-test/p".into(),
                sort_order: 0,
            })
            .unwrap();
        for (id, is_main) in [("root", true), ("feat", false)] {
            daemon
                .store
                .insert_worktree(&Worktree {
                    id: WorktreeId(id.into()),
                    project_id: ProjectId("p".into()),
                    path: format!("/nebula-test/p-{id}").into(),
                    branch: id.into(),
                    is_main,
                    sort_order: 0,
                })
                .unwrap();
        }
        daemon
    }

    fn agent(id: &str, worktree: &str, kind: AgentKind, model: Option<&str>) -> Agent {
        Agent {
            id: AgentId(id.into()),
            worktree_id: WorktreeId(worktree.into()),
            name: id.into(),
            status: AgentStatus::Running,
            archived: false,
            archived_at: 0,
            unseen: false,
            kind,
            custom_harness: None,
            model: model.map(str::to_string),
            effort: model.map(|_| "high".to_string()),
            session_id: Some("s1".into()),
            cloud_session_id: None,
            sort_order: 0,
            status_changed_at: 0,
            alive: false,
            issue_url: None,
            recent_prompts: Vec::new(),
        }
    }

    #[test]
    fn sibling_name_is_the_first_free_agent_n() {
        assert_eq!(sibling_name(&[]), "agent-1");
        let taken = ["agent-1", "Fix Login Redirect", "agent-3"]
            .map(String::from)
            .to_vec();
        assert_eq!(sibling_name(&taken), "agent-2");
    }

    #[test]
    fn sibling_spec_lands_in_the_callers_worktree_with_its_harness() {
        let daemon = daemon();
        daemon
            .store
            .insert_agent(&agent("agent-1", "feat", AgentKind::Claude, Some("opus")))
            .unwrap();
        // A row in another worktree does not take a name in this one.
        daemon
            .store
            .insert_agent(&agent("agent-2", "root", AgentKind::Codex, None))
            .unwrap();

        let caller = daemon.spawning_caller(&AgentId("agent-1".into())).unwrap();
        let spec = daemon
            .sibling_spec(&caller, None, &caller.worktree_id, "Fix the login redirect")
            .unwrap();
        assert_eq!(spec.worktree.to_string(), "feat");
        assert_eq!(spec.name, "agent-2");
        assert_eq!(spec.kind, AgentKind::Claude);
        assert_eq!(spec.model.as_deref(), Some("opus"));
        assert_eq!(spec.effort.as_deref(), Some("high"));
        assert!(spec.auto_title, "a default name earns an auto-title");
        assert_eq!(
            spec.starting_prompt.as_deref(),
            Some("Fix the login redirect")
        );
        assert!(spec.cloud_prompt.is_none() && spec.pr_url.is_none() && spec.issue_url.is_none());
    }

    #[test]
    fn a_named_harness_drops_the_callers_model_and_effort() {
        let daemon = daemon();
        daemon
            .store
            .insert_agent(&agent("agent-1", "feat", AgentKind::Claude, Some("opus")))
            .unwrap();
        let caller = daemon.spawning_caller(&AgentId("agent-1".into())).unwrap();
        let spec = daemon
            .sibling_spec(
                &caller,
                Some(AgentKind::Codex),
                &caller.worktree_id,
                "Run the tests",
            )
            .unwrap();
        assert_eq!(spec.kind, AgentKind::Codex);
        assert!(
            spec.model.is_none() && spec.effort.is_none(),
            "a Claude model name means nothing to codex"
        );
        // Naming the caller's own harness keeps its knobs.
        let same = daemon
            .sibling_spec(
                &caller,
                Some(AgentKind::Claude),
                &caller.worktree_id,
                "Run the tests",
            )
            .unwrap();
        assert_eq!(same.model.as_deref(), Some("opus"));
    }

    #[test]
    fn unknown_and_archived_callers_are_refused() {
        let daemon = daemon();
        let missing = daemon
            .spawning_caller(&AgentId("nope".into()))
            .expect_err("an unknown caller is refused");
        assert!(missing.to_string().contains("agent not found"));

        let mut archived = agent("agent-1", "feat", AgentKind::Claude, None);
        archived.archived = true;
        daemon.store.insert_agent(&archived).unwrap();
        let err = daemon
            .spawning_caller(&AgentId("agent-1".into()))
            .expect_err("an archived caller is refused");
        assert!(err.to_string().contains("archived"));
    }

    /// Without `--worktree` the prompt is `create_agent`'s to validate
    /// (blank, NUL, too long).
    #[tokio::test]
    async fn spawn_sibling_agent_rejects_a_blank_prompt() {
        let daemon = daemon();
        daemon
            .store
            .insert_agent(&agent("agent-1", "feat", AgentKind::Claude, None))
            .unwrap();
        let err = daemon
            .spawn_sibling_agent(&AgentId("agent-1".into()), None, None, " \n ")
            .await
            .unwrap_err();
        assert!(err.to_string().contains("is empty"), "{err}");
    }

    #[test]
    fn a_named_worktree_names_the_spawn_against_its_own_rows() {
        let daemon = daemon();
        daemon
            .store
            .insert_agent(&agent("agent-1", "feat", AgentKind::Claude, None))
            .unwrap();
        let mut in_root = agent("other", "root", AgentKind::Codex, None);
        in_root.name = "agent-2".into();
        daemon.store.insert_agent(&in_root).unwrap();
        // `agent-1` is the caller's, in another worktree; `agent-2` is
        // taken where the spawn lands.
        assert_eq!(
            daemon.free_name_in(&WorktreeId("root".into())).unwrap(),
            "agent-1"
        );
    }

    /// A branch that already has a checkout is taken as it stands, no git
    /// involved (the test project is no repo, so a cut would fail). Plain
    /// `nebula worktree --base` keeps that legacy reuse, while spawn
    /// refuses a named base it cannot apply to an existing checkout.
    #[tokio::test]
    async fn a_branch_with_a_worktree_is_reused_not_cut() {
        let daemon = daemon();
        let project = ProjectId("p".into());
        let worktree = daemon
            .worktree_on_branch(&project, "feat", None)
            .await
            .unwrap();
        assert_eq!(worktree.id, WorktreeId("feat".into()));

        let worktree = daemon
            .worktree_on_branch(&project, "feat", Some("main"))
            .await
            .unwrap();
        assert_eq!(worktree.id, WorktreeId("feat".into()));

        let err = daemon
            .worktree_on_branch_for_spawn(&project, "feat", Some("main"))
            .await
            .unwrap_err();
        assert!(err.to_string().contains("already has a worktree"), "{err}");
    }

    /// With `--worktree`, a call the create would refuse is refused before
    /// git is asked for a worktree: the test project is no repo, so a git
    /// error here would mean the checks came too late.
    #[tokio::test]
    async fn a_worktree_spawn_is_refused_before_any_worktree_is_cut() {
        let daemon = daemon();
        daemon
            .store
            .insert_agent(&agent("agent-1", "feat", AgentKind::Claude, None))
            .unwrap();
        let caller = AgentId("agent-1".into());
        let fresh = SiblingWorktree {
            branch: "fix-login",
            base: None,
        };

        let blank_branch = SiblingWorktree {
            branch: "  ",
            base: None,
        };
        let err = daemon
            .spawn_sibling_agent(&caller, None, Some(blank_branch), "Fix it")
            .await
            .unwrap_err();
        assert!(err.to_string().contains("branch name is empty"), "{err}");

        let err = daemon
            .spawn_sibling_agent(&caller, None, Some(fresh), " \n ")
            .await
            .unwrap_err();
        assert!(err.to_string().contains("is empty"), "{err}");

        let err = daemon
            .spawn_sibling_agent(&AgentId("nope".into()), None, Some(fresh), "Fix it")
            .await
            .unwrap_err();
        assert!(err.to_string().contains("agent not found"), "{err}");

        // The harness is checked before git too: a Custom row without its
        // registry id is refused by the same check `create_agent` makes.
        daemon
            .store
            .insert_agent(&agent("custom-1", "feat", AgentKind::Custom, None))
            .unwrap();
        let err = daemon
            .spawn_sibling_agent(&AgentId("custom-1".into()), None, Some(fresh), "Fix it")
            .await
            .unwrap_err();
        assert!(err.to_string().contains("missing its registry id"), "{err}");

        let (_, worktrees, _, _) = daemon.store.load_tree().unwrap();
        assert_eq!(worktrees.len(), 2, "no worktree was registered");
    }
}
