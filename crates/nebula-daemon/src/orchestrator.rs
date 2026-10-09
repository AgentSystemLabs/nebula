//! Privileged commands for the ORCHESTRATOR SESSION.
//!
//! The CLI wrappers are just another same-user socket client, so prompt text
//! is not part of the trust boundary. Every operation here starts by checking
//! the caller row persisted in SQLite is the orchestrator.

use std::sync::Arc;

use anyhow::{bail, Context, Result};
use nebula_core::{
    Agent, AgentId, AgentKind, AgentRole, EntityId, Link, OutputTail, Project, ProjectId,
    ReviewTabKind, ServerEvent, SessionRef, TerminalTab, Worktree, WorktreeId,
    MAX_CLOUD_PROMPT_BYTES,
};

use crate::registry::{CreateAgentSpec, Daemon};

pub const CLAUDE_ORCHESTRATOR_GUIDANCE: &str = "[nebula] You are the pinned ORCHESTRATOR \
session for nebula. Your job is to coordinate work across every project, worktree and session \
without making the user click through them. You have these orchestrator-only commands; the daemon \
will refuse them unless this session is the persisted orchestrator:\n\n  nebula orchestrator list \
[--json]\n  nebula orchestrator read <session-id> [--bytes N]\n  nebula orchestrator send \
<session-id> \"<prompt>\"\n  nebula orchestrator spawn --project <project-id-or-name> \
[--worktree <branch-or-id>] \"<task>\"\n  nebula orchestrator review <session-id> \
[--tab terminal|diff|history|pr]...\n\nUse `list --json` before acting when you need current \
IDs or status. Use `read` for bounded recent output, not full transcript export. Use `send` for \
follow-ups the user asked you to delegate. Use `spawn` to start workers in another project or \
worktree. Use `review` when the user wants to inspect work; it opens nebula's tabbed review modal \
for them. Default to tell-and-ask for permission prompts: do not approve another agent's \
permission request unless the user explicitly tells you to.";

pub type OrchestratorTree = (
    Vec<Project>,
    Vec<Worktree>,
    Vec<Agent>,
    Vec<TerminalTab>,
    Vec<Link>,
);

impl Daemon {
    pub(crate) fn require_orchestrator(&self, caller: &AgentId) -> Result<Agent> {
        let agent = self
            .store
            .get_agent(caller)?
            .context("orchestrator session not found")?;
        if agent.archived {
            bail!("orchestrator session is archived");
        }
        if agent.role != AgentRole::Orchestrator {
            bail!("this command is only available to the orchestrator session");
        }
        Ok(agent)
    }

    pub fn orchestrator_tree(&self, caller: &AgentId) -> Result<OrchestratorTree> {
        self.require_orchestrator(caller)?;
        let (projects, worktrees, mut agents, mut terminals) = self.store.load_tree()?;
        for a in &mut agents {
            a.alive = self.is_alive(&SessionRef::Agent(a.id.clone()));
        }
        for t in &mut terminals {
            t.alive = self.is_alive(&SessionRef::Terminal(t.id.clone()));
        }
        Ok((
            projects,
            worktrees,
            agents,
            terminals,
            self.store.load_links()?,
        ))
    }

    pub fn orchestrator_send(
        self: &Arc<Self>,
        caller: &AgentId,
        target: &AgentId,
        message: &str,
    ) -> Result<()> {
        self.require_orchestrator(caller)?;
        let text = validate_message(message)?;
        let agent = self
            .store
            .get_agent(target)?
            .context("target agent not found")?;
        if agent.archived {
            bail!("target agent is archived");
        }
        if agent.cloud_session_id.is_some() {
            bail!("target agent is a Claude Cloud session; use its cloud message path");
        }
        let sref = SessionRef::Agent(target.clone());
        let session =
            self.ensure_session(&sref, crate::pty::DEFAULT_COLS, crate::pty::DEFAULT_ROWS)?;
        crate::session_context::write_prompt(&session, text)?;
        Ok(())
    }

    pub fn orchestrator_read(
        &self,
        caller: &AgentId,
        session: &SessionRef,
        max_bytes: u32,
    ) -> Result<Option<OutputTail>> {
        self.require_orchestrator(caller)?;
        Ok(self.read_output_tail_clean(session, max_bytes))
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn orchestrator_spawn(
        self: &Arc<Self>,
        caller: &AgentId,
        project: &ProjectId,
        worktree: Option<WorktreeId>,
        branch: Option<String>,
        base: Option<String>,
        name: String,
        kind: AgentKind,
        custom_harness: Option<String>,
        model: Option<String>,
        effort: Option<String>,
        starting_prompt: String,
    ) -> Result<EntityId> {
        self.require_orchestrator(caller)?;
        let worktree = match worktree {
            Some(id) => {
                let wt = self
                    .store
                    .get_worktree(&id)?
                    .context("worktree not found")?;
                if &wt.project_id != project {
                    bail!("worktree does not belong to the requested project");
                }
                wt
            }
            None => {
                if branch.as_ref().is_none_or(|b| b.trim().is_empty()) {
                    if let Some(existing) = self
                        .store
                        .load_tree()?
                        .1
                        .into_iter()
                        .filter(|w| &w.project_id == project)
                        .min_by_key(|w| (!w.is_main, w.sort_order))
                    {
                        existing
                    } else {
                        bail!("project has no worktrees");
                    }
                } else {
                    let branch = slugify_branch(branch.unwrap_or_default().as_str());
                    let branch = if branch.is_empty() {
                        random_branch_name()
                    } else {
                        branch
                    };
                    if let Some(existing) = self
                        .store
                        .load_tree()?
                        .1
                        .into_iter()
                        .find(|w| &w.project_id == project && w.branch == branch)
                    {
                        existing
                    } else {
                        let created = self
                            .create_worktree(project, &branch, base.as_deref())
                            .await?;
                        let EntityId::Worktree(id) = created else {
                            unreachable!("create_worktree returns a worktree id");
                        };
                        self.store
                            .get_worktree(&id)?
                            .context("created worktree not found")?
                    }
                }
            }
        };
        let (_, _, agents, _) = self.store.load_tree()?;
        let taken = agents
            .iter()
            .filter(|a| a.worktree_id == worktree.id)
            .map(|a| a.name.clone())
            .collect::<Vec<_>>();
        let name = if name.trim().is_empty() {
            crate::sibling::sibling_name(&taken)
        } else {
            name
        };
        self.create_agent(CreateAgentSpec {
            worktree: worktree.id,
            name,
            kind,
            custom_harness,
            model,
            effort,
            auto_title: true,
            cloud_prompt: None,
            starting_prompt: Some(starting_prompt),
            pr_url: None,
            issue_url: None,
            role: AgentRole::Worker,
        })
        .await
    }

    pub fn orchestrator_open_review(
        &self,
        caller: &AgentId,
        sessions: Vec<SessionRef>,
        tabs: Vec<ReviewTabKind>,
    ) -> Result<()> {
        self.require_orchestrator(caller)?;
        if sessions.is_empty() {
            bail!("no sessions named for review");
        }
        for session in &sessions {
            match session {
                SessionRef::Agent(id) => {
                    self.store.get_agent(id)?.context("agent not found")?;
                }
                SessionRef::Terminal(id) => {
                    self.store.get_terminal(id)?.context("terminal not found")?;
                }
            }
        }
        self.broadcast(ServerEvent::ReviewOpened {
            opener: caller.clone(),
            sessions,
            tabs: if tabs.is_empty() {
                vec![
                    ReviewTabKind::Terminal,
                    ReviewTabKind::Diff,
                    ReviewTabKind::History,
                    ReviewTabKind::PullRequest,
                ]
            } else {
                tabs
            },
        });
        Ok(())
    }
}

fn validate_message(message: &str) -> Result<&str> {
    let text = message.trim();
    if text.is_empty() {
        bail!("message is empty");
    }
    if text.contains('\0') {
        bail!("message cannot contain NUL bytes");
    }
    if text.len() > MAX_CLOUD_PROMPT_BYTES {
        bail!(
            "message is too long (max {} KiB)",
            MAX_CLOUD_PROMPT_BYTES / 1024
        );
    }
    Ok(text)
}

fn slugify_branch(raw: &str) -> String {
    raw.trim()
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect::<String>()
        .split('-')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("-")
}

fn random_branch_name() -> String {
    format!("orchestrator-{}", nebula_core::clock::now_ms())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hooks::HookEnv;
    use crate::store::Store;
    use nebula_core::{AgentStatus, ProjectId};

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
                name: "demo".into(),
                repo_path: "/tmp/demo".into(),
                sort_order: 0,
            })
            .unwrap();
        daemon
            .store
            .insert_worktree(&Worktree {
                id: WorktreeId("w".into()),
                project_id: ProjectId("p".into()),
                path: "/tmp/demo".into(),
                branch: "main".into(),
                is_main: true,
                sort_order: 0,
            })
            .unwrap();
        daemon
    }

    fn agent(id: &str, role: AgentRole) -> Agent {
        Agent {
            id: AgentId(id.into()),
            worktree_id: WorktreeId("w".into()),
            name: id.into(),
            status: AgentStatus::Fresh,
            archived: false,
            archived_at: 0,
            unseen: false,
            status_changed_at: 0,
            kind: AgentKind::Claude,
            custom_harness: None,
            model: None,
            effort: None,
            session_id: None,
            cloud_session_id: None,
            issue_url: None,
            role,
            sort_order: 0,
            alive: false,
            recent_prompts: Vec::new(),
        }
    }

    #[test]
    fn only_orchestrator_role_can_use_privileged_list() {
        let daemon = daemon();
        daemon
            .store
            .insert_agent(&agent("worker", AgentRole::Worker))
            .unwrap();
        let err = daemon
            .orchestrator_tree(&AgentId("worker".into()))
            .unwrap_err();
        assert!(err.to_string().contains("only available"));

        daemon
            .store
            .insert_agent(&agent("orchestrator", AgentRole::Orchestrator))
            .unwrap();
        let (_, _, agents, _, _) = daemon
            .orchestrator_tree(&AgentId("orchestrator".into()))
            .unwrap();
        assert_eq!(agents.len(), 2);
        assert!(agents
            .iter()
            .any(|a| a.id.0 == "orchestrator" && a.role == AgentRole::Orchestrator));
    }
}
