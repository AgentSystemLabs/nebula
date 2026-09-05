//! `nebula spawn "<task>"` from inside an agent session: a new AGENT beside
//! the caller — same WORKTREE, same harness unless another is named — that
//! opens on the task as its STARTING PROMPT. The caller's own process is
//! never touched (unlike `nebula worktree`, nothing here waits on a turn
//! end), so the model runs it, tells the user, and carries on.

use std::sync::Arc;

use anyhow::{bail, Context, Result};
use nebula_core::{AgentId, AgentKind, EntityId};

use crate::registry::{CreateAgentSpec, Daemon};

/// What nebula appends to Claude's system prompt so "start a new nebula
/// session that …" becomes one `nebula spawn` call instead of the model
/// trying to launch an agent process itself. Claude and pi, like the
/// worktree guidance (both take `--append-system-prompt`): codex and cursor
/// have no system-prompt flag.
pub const CLAUDE_SPAWN_GUIDANCE: &str = "[nebula] When the user asks you to start a new nebula \
session (\"start a new nebula session that …\", \"spin up another session to …\", \"open a new \
session for …\"), do not launch an agent process yourself. Run this shell command instead, exactly \
once:\n\n  nebula spawn \"<task>\"\n\nwhere <task> is the work the user wants that session to do, \
in their own words — the new session opens on it as its first prompt, so make it self-contained. \
Add `--kind claude|codex|cursor|pi` only when the user names the harness; otherwise the new session \
matches this one. nebula starts it beside this session, in the same worktree, and it shows up in \
the sessions list on its own. This session is unaffected: carry on with whatever else the user \
asked, and if starting the session was the whole request, tell the user in one line that it is \
running. If the command fails, report the error.";

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
    /// The create spec for a session started beside `id`: its worktree, its
    /// harness (and model / effort) unless `kind` overrides — a different
    /// CLI cannot take this one's model name — a default `agent-N` name so
    /// AUTO-TITLE applies, and `starting_prompt` as the first prompt (which
    /// `create_agent` validates). Pure lookup, so it is unit-testable
    /// without a PTY.
    pub(crate) fn sibling_spec(
        &self,
        id: &AgentId,
        kind: Option<AgentKind>,
        starting_prompt: &str,
    ) -> Result<CreateAgentSpec> {
        let caller = self.store.get_agent(id)?.context("agent not found")?;
        if caller.archived {
            bail!("agent is archived");
        }
        let (_, _, agents, _) = self.store.load_tree()?;
        let taken = agents
            .iter()
            .filter(|a| a.worktree_id == caller.worktree_id)
            .map(|a| a.name.clone())
            .collect::<Vec<_>>();
        let kind = kind.unwrap_or(caller.kind);
        let (model, effort) = if kind == caller.kind {
            (caller.model.clone(), caller.effort.clone())
        } else {
            (None, None)
        };
        Ok(CreateAgentSpec {
            worktree: caller.worktree_id.clone(),
            name: sibling_name(&taken),
            kind,
            model,
            effort,
            auto_title: true,
            cloud_prompt: None,
            starting_prompt: Some(starting_prompt.to_string()),
            pr_url: None,
        })
    }

    /// `nebula spawn`, run by the agent inside its own session: create and
    /// boot a new agent beside it with `starting_prompt` as its first
    /// prompt. Returns the new row's id; the upsert reaches every client
    /// through the ordinary create path.
    pub async fn spawn_sibling_agent(
        self: &Arc<Self>,
        id: &AgentId,
        kind: Option<AgentKind>,
        starting_prompt: &str,
    ) -> Result<EntityId> {
        let spec = self.sibling_spec(id, kind, starting_prompt)?;
        self.create_agent(spec).await
    }
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
                workspace_id: Default::default(),
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
            model: model.map(str::to_string),
            effort: model.map(|_| "high".to_string()),
            session_id: Some("s1".into()),
            cloud_session_id: None,
            sort_order: 0,
            status_changed_at: 0,
            alive: false,
            cloud_mirroring: false,
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

        let spec = daemon
            .sibling_spec(&AgentId("agent-1".into()), None, "Fix the login redirect")
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
        assert!(spec.cloud_prompt.is_none() && spec.pr_url.is_none());
    }

    #[test]
    fn a_named_harness_drops_the_callers_model_and_effort() {
        let daemon = daemon();
        daemon
            .store
            .insert_agent(&agent("agent-1", "feat", AgentKind::Claude, Some("opus")))
            .unwrap();
        let spec = daemon
            .sibling_spec(
                &AgentId("agent-1".into()),
                Some(AgentKind::Codex),
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
                &AgentId("agent-1".into()),
                Some(AgentKind::Claude),
                "Run the tests",
            )
            .unwrap();
        assert_eq!(same.model.as_deref(), Some("opus"));
    }

    #[test]
    fn unknown_and_archived_callers_are_refused() {
        let daemon = daemon();
        // `CreateAgentSpec` is deliberately not Debug (it carries the
        // prompt), so the Err side is taken by hand.
        let missing = daemon
            .sibling_spec(&AgentId("nope".into()), None, "x")
            .err()
            .expect("an unknown caller is refused");
        assert!(missing.to_string().contains("agent not found"));

        let mut archived = agent("agent-1", "feat", AgentKind::Claude, None);
        archived.archived = true;
        daemon.store.insert_agent(&archived).unwrap();
        let err = daemon
            .sibling_spec(&AgentId("agent-1".into()), None, "x")
            .err()
            .expect("an archived caller is refused");
        assert!(err.to_string().contains("archived"));
    }

    /// The prompt itself is `create_agent`'s to validate (blank, NUL, too
    /// long), so a bad one fails there, before any worktree lookup.
    #[tokio::test]
    async fn spawn_sibling_agent_rejects_a_blank_prompt() {
        let daemon = daemon();
        daemon
            .store
            .insert_agent(&agent("agent-1", "feat", AgentKind::Claude, None))
            .unwrap();
        let err = daemon
            .spawn_sibling_agent(&AgentId("agent-1".into()), None, " \n ")
            .await
            .unwrap_err();
        assert!(err.to_string().contains("is empty"), "{err}");
    }
}
