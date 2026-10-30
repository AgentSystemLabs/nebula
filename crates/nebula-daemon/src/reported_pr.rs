//! PRs reported by completed sessions, independent of their checkout's repo.
//! These are display links, never launch scope or permission to delete a checkout.
use crate::registry::Daemon;
use anyhow::Result;
use nebula_core::{AgentId, Entity, ServerEvent};

impl Daemon {
    pub fn remember_reported_pr(&self, agent: &AgentId, url: &str) -> Result<()> {
        let Some(url) = nebula_core::github_url::github_pr_url(url) else {
            return Ok(());
        };
        let Some(agent) = self.store.get_agent(agent)? else {
            return Ok(());
        };
        let link = self.store.remember_pr_link(&agent.worktree_id, &url)?;
        // A repeated completion (for example "merged PR #384") also re-arms
        // the TUI lookup, without inserting another link.
        self.broadcast(ServerEvent::EntityUpserted {
            entity: Entity::Link(link),
        });
        Ok(())
    }
}
