//! The daemon's world: persisted entity tree + live PTY sessions, and the
//! operations the IPC surface exposes over them.

use crate::git;
use crate::hooks::{self, HookEnv};
use crate::pty::{PtyEvent, PtySession, SpawnSpec};
use crate::status::{AgentStatusMachine, Effect, HookEvent};
use crate::store::Store;
use anyhow::{bail, Context, Result};
use nebula_core::{
    Agent, AgentId, AgentStatus, Entity, EntityId, Project, ProjectId, ServerEvent, SessionRef,
    TerminalId, TerminalTab, Worktree, WorktreeId,
};
use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Instant;
use tokio::sync::broadcast;

pub struct Daemon {
    sessions: Mutex<HashMap<SessionRef, Arc<PtySession>>>,
    status_machines: Mutex<HashMap<AgentId, AgentStatusMachine>>,
    pub hook_env: HookEnv,
    pub store: Store,
    /// Entity/status deltas fanned out to every subscribed client.
    pub events: broadcast::Sender<ServerEvent>,
    pub shutdown: tokio_util::sync::CancellationToken,
}

impl Daemon {
    pub fn new(store: Store, hook_env: HookEnv) -> Arc<Self> {
        let (events, _) = broadcast::channel(1024);
        Arc::new(Self {
            sessions: Mutex::new(HashMap::new()),
            status_machines: Mutex::new(HashMap::new()),
            hook_env,
            store,
            events,
            shutdown: tokio_util::sync::CancellationToken::new(),
        })
    }

    // ---- status machine plumbing ----

    /// Feed one hook (or synthetic) event through the agent's status machine
    /// and apply the resulting effects (persist + broadcast).
    pub fn apply_hook_event(&self, agent_id: &AgentId, event: HookEvent, session_id: Option<String>) {
        let effects = {
            let mut machines = self.status_machines.lock().unwrap();
            let machine = match machines.entry(agent_id.clone()) {
                std::collections::hash_map::Entry::Occupied(e) => e.into_mut(),
                std::collections::hash_map::Entry::Vacant(slot) => {
                    // Lazily seed from the persisted row; unknown ids (stale
                    // env, deleted agent) are dropped.
                    match self.store.get_agent(agent_id) {
                        Ok(Some(agent)) => slot
                            .insert(AgentStatusMachine::new(agent.status, agent.claude_session_id)),
                        _ => return,
                    }
                }
            };
            machine.handle(event, session_id.as_deref(), Instant::now())
        };
        self.apply_status_effects(agent_id, effects);
    }

    /// Deferred-finish recheck across all machines (runs on a timer).
    pub fn tick_status_machines(&self) {
        let now = Instant::now();
        let ticked: Vec<(AgentId, Vec<Effect>)> = {
            let mut machines = self.status_machines.lock().unwrap();
            machines.iter_mut().map(|(id, m)| (id.clone(), m.tick(now))).collect()
        };
        for (id, effects) in ticked {
            self.apply_status_effects(&id, effects);
        }
    }

    fn apply_status_effects(&self, agent_id: &AgentId, effects: Vec<Effect>) {
        for effect in effects {
            match effect {
                Effect::SetStatus(status) => {
                    if let Err(e) = self.store.set_agent_status(agent_id, status) {
                        tracing::warn!(error = %e, "persist status failed");
                    }
                    self.broadcast(ServerEvent::StatusChanged { agent: agent_id.clone(), status });
                }
                Effect::SaveSessionId(sid) => {
                    if let Err(e) = self.store.set_agent_session_id(agent_id, Some(&sid)) {
                        tracing::warn!(error = %e, "persist session id failed");
                    }
                }
            }
        }
    }

    pub fn broadcast(&self, ev: ServerEvent) {
        let _ = self.events.send(ev);
    }

    pub fn session(&self, sref: &SessionRef) -> Option<Arc<PtySession>> {
        self.sessions.lock().unwrap().get(sref).cloned()
    }

    pub fn is_alive(&self, sref: &SessionRef) -> bool {
        self.sessions.lock().unwrap().contains_key(sref)
    }

    pub fn remove_session(&self, sref: &SessionRef) -> Option<Arc<PtySession>> {
        self.sessions.lock().unwrap().remove(sref)
    }

    pub fn kill_session(&self, sref: &SessionRef) {
        if let Some(s) = self.remove_session(sref) {
            s.kill();
        }
    }

    pub fn kill_all(&self) {
        for (_, s) in self.sessions.lock().unwrap().drain() {
            s.kill();
        }
    }

    // ---- snapshot ----

    pub fn snapshot(&self) -> Result<ServerEvent> {
        let (projects, worktrees, mut agents, mut terminals) = self.store.load_tree()?;
        {
            let sessions = self.sessions.lock().unwrap();
            for a in &mut agents {
                a.alive = sessions.contains_key(&SessionRef::Agent(a.id.clone()));
            }
            for t in &mut terminals {
                t.alive = sessions.contains_key(&SessionRef::Terminal(t.id.clone()));
            }
        }
        Ok(ServerEvent::Snapshot {
            projects,
            worktrees,
            agents,
            terminals,
            ui_state: self.store.load_ui_state()?,
        })
    }

    fn agent_entity(&self, id: &AgentId) -> Result<Agent> {
        let mut agent = self.store.get_agent(id)?.context("agent not found")?;
        agent.alive = self.is_alive(&SessionRef::Agent(id.clone()));
        Ok(agent)
    }

    fn terminal_entity(&self, id: &TerminalId) -> Result<TerminalTab> {
        let mut term = self.store.get_terminal(id)?.context("terminal not found")?;
        term.alive = self.is_alive(&SessionRef::Terminal(id.clone()));
        Ok(term)
    }

    // ---- projects ----

    pub async fn add_project(self: &Arc<Self>, path: &Path, name: Option<String>) -> Result<EntityId> {
        let toplevel = git::repo_toplevel(path)
            .await
            .with_context(|| format!("{} is not a git repository", path.display()))?;
        if self.store.project_by_path(&toplevel)?.is_some() {
            bail!("project already added: {}", toplevel.display());
        }
        let name = name.unwrap_or_else(|| {
            toplevel.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "project".into())
        });
        let project = Project {
            id: ProjectId::generate(),
            name,
            repo_path: toplevel.clone(),
            sort_order: self.store.next_project_sort_order()?,
            divider_after: false,
            divider_label: None,
        };
        self.store.insert_project(&project)?;
        self.broadcast(ServerEvent::EntityUpserted { entity: Entity::Project(project.clone()) });

        // Main checkout is modeled as a worktree row; adopt pre-existing
        // worktrees too so `nebula` matches reality on day one.
        let entries = git::list_worktrees(&toplevel).await.unwrap_or_default();
        let mut first = true;
        for entry in entries {
            let worktree = Worktree {
                id: WorktreeId::generate(),
                project_id: project.id.clone(),
                path: entry.path.clone(),
                branch: entry.branch,
                is_main: first,
                sort_order: 0,
            };
            first = false;
            self.store.insert_worktree(&worktree)?;
            self.broadcast(ServerEvent::EntityUpserted { entity: Entity::Worktree(worktree) });
        }
        Ok(EntityId::Project(project.id))
    }

    pub fn remove_project(self: &Arc<Self>, id: &ProjectId) -> Result<()> {
        // Kill any live sessions under this project first.
        let (_, worktrees, agents, terminals) = self.store.load_tree()?;
        let wt_ids: Vec<WorktreeId> = worktrees
            .into_iter()
            .filter(|w| &w.project_id == id)
            .map(|w| w.id)
            .collect();
        for a in agents.iter().filter(|a| wt_ids.contains(&a.worktree_id)) {
            self.kill_session(&SessionRef::Agent(a.id.clone()));
        }
        for t in terminals.iter().filter(|t| wt_ids.contains(&t.worktree_id)) {
            self.kill_session(&SessionRef::Terminal(t.id.clone()));
        }
        // Removing a project only forgets it in nebula — never touches disk.
        self.store.delete_project(id)?;
        self.broadcast(ServerEvent::EntityRemoved { id: EntityId::Project(id.clone()) });
        Ok(())
    }

    /// Move a project `delta` slots in the display order (clamped at the
    /// edges). Sort orders are rewritten to the display index for every row,
    /// which also normalizes legacy all-zero orders on first use.
    pub fn move_project(self: &Arc<Self>, id: &ProjectId, delta: i64) -> Result<()> {
        let (mut projects, _, _, _) = self.store.load_tree()?;
        let Some(index) = projects.iter().position(|p| &p.id == id) else {
            bail!("project not found");
        };
        let target = (index as i64 + delta).clamp(0, projects.len() as i64 - 1) as usize;
        if target == index {
            return Ok(());
        }
        let before: HashMap<ProjectId, (i64, bool, Option<String>)> = projects
            .iter()
            .map(|p| (p.id.clone(), (p.sort_order, p.divider_after, p.divider_label.clone())))
            .collect();
        // Dividers mark the gap below a display slot, not the project in it:
        // keep them (and their labels) pinned to their slots while the
        // projects move through.
        let dividers: Vec<(bool, Option<String>)> =
            projects.iter().map(|p| (p.divider_after, p.divider_label.clone())).collect();
        let moved = projects.remove(index);
        projects.insert(target, moved);
        for (slot, project) in projects.iter_mut().enumerate() {
            project.sort_order = slot as i64;
            (project.divider_after, project.divider_label) = dividers[slot].clone();
            let now = (project.sort_order, project.divider_after, project.divider_label.clone());
            if before.get(&project.id) != Some(&now) {
                self.store.set_project_position(
                    &project.id,
                    project.sort_order,
                    project.divider_after,
                    project.divider_label.as_deref(),
                )?;
                self.broadcast(ServerEvent::EntityUpserted { entity: Entity::Project(project.clone()) });
            }
        }
        Ok(())
    }

    pub fn set_project_divider(
        self: &Arc<Self>,
        id: &ProjectId,
        divider_after: bool,
        label: Option<String>,
    ) -> Result<()> {
        let mut project = self.store.get_project(id)?.context("project not found")?;
        // A removed divider keeps no label.
        let label = if divider_after { label.filter(|l| !l.trim().is_empty()) } else { None };
        if (project.divider_after, &project.divider_label) == (divider_after, &label) {
            return Ok(());
        }
        project.divider_after = divider_after;
        project.divider_label = label;
        self.store.set_project_position(
            id,
            project.sort_order,
            divider_after,
            project.divider_label.as_deref(),
        )?;
        self.broadcast(ServerEvent::EntityUpserted { entity: Entity::Project(project) });
        Ok(())
    }

    // ---- worktrees ----

    pub async fn create_worktree(
        self: &Arc<Self>,
        project_id: &ProjectId,
        branch: &str,
        base: Option<&str>,
    ) -> Result<EntityId> {
        if branch.trim().is_empty() {
            bail!("branch name is empty");
        }
        let project = self.store.get_project(project_id)?.context("project not found")?;
        let path = git::add_worktree(&project.repo_path, branch, base).await?;
        let worktree = Worktree {
            id: WorktreeId::generate(),
            project_id: project_id.clone(),
            path,
            branch: branch.to_string(),
            is_main: false,
            sort_order: 0,
        };
        self.store.insert_worktree(&worktree)?;
        self.broadcast(ServerEvent::EntityUpserted { entity: Entity::Worktree(worktree.clone()) });
        Ok(EntityId::Worktree(worktree.id))
    }

    pub async fn delete_worktree(self: &Arc<Self>, id: &WorktreeId, force: bool) -> Result<()> {
        let worktree = self.store.get_worktree(id)?.context("worktree not found")?;
        if worktree.is_main {
            bail!("cannot delete the main checkout — remove the project instead");
        }
        let project = self.store.get_project(&worktree.project_id)?.context("project not found")?;

        // Kill sessions living in this worktree.
        let (_, _, agents, terminals) = self.store.load_tree()?;
        for a in agents.iter().filter(|a| &a.worktree_id == id) {
            self.kill_session(&SessionRef::Agent(a.id.clone()));
        }
        for t in terminals.iter().filter(|t| &t.worktree_id == id) {
            self.kill_session(&SessionRef::Terminal(t.id.clone()));
        }

        git::remove_worktree(&project.repo_path, &worktree.path, force).await?;
        self.store.delete_worktree(id)?;
        self.broadcast(ServerEvent::EntityRemoved { id: EntityId::Worktree(id.clone()) });
        Ok(())
    }

    // ---- agents ----

    pub fn create_agent(self: &Arc<Self>, worktree_id: &WorktreeId, name: &str) -> Result<EntityId> {
        let worktree = self.store.get_worktree(worktree_id)?.context("worktree not found")?;
        let agent = Agent {
            id: AgentId::generate(),
            worktree_id: worktree_id.clone(),
            name: if name.trim().is_empty() { "agent".into() } else { name.trim().to_string() },
            status: AgentStatus::Fresh,
            archived: false,
            claude_session_id: None,
            sort_order: 0,
            alive: false,
        };
        self.store.insert_agent(&agent)?;
        // Spawn immediately — a new agent should boot claude right away.
        self.spawn_agent_session(&agent, &worktree, 80, 24)?;
        let mut broadcast_agent = agent.clone();
        broadcast_agent.alive = true;
        self.broadcast(ServerEvent::EntityUpserted { entity: Entity::Agent(broadcast_agent) });
        Ok(EntityId::Agent(agent.id))
    }

    pub fn rename_agent(self: &Arc<Self>, id: &AgentId, name: &str) -> Result<()> {
        if name.trim().is_empty() {
            bail!("name is empty");
        }
        self.store.rename_agent(id, name.trim())?;
        let agent = self.agent_entity(id)?;
        self.broadcast(ServerEvent::EntityUpserted { entity: Entity::Agent(agent) });
        Ok(())
    }

    pub fn archive_agent(self: &Arc<Self>, id: &AgentId) -> Result<()> {
        self.kill_session(&SessionRef::Agent(id.clone()));
        self.store.set_agent_archived(id, true)?;
        let agent = self.agent_entity(id)?;
        self.broadcast(ServerEvent::EntityUpserted { entity: Entity::Agent(agent) });
        Ok(())
    }

    pub fn unarchive_agent(self: &Arc<Self>, id: &AgentId) -> Result<()> {
        self.store.set_agent_archived(id, false)?;
        let agent = self.agent_entity(id)?;
        self.broadcast(ServerEvent::EntityUpserted { entity: Entity::Agent(agent) });
        Ok(())
    }

    pub fn delete_agent(self: &Arc<Self>, id: &AgentId) -> Result<()> {
        self.kill_session(&SessionRef::Agent(id.clone()));
        self.store.delete_agent(id)?;
        self.broadcast(ServerEvent::EntityRemoved { id: EntityId::Agent(id.clone()) });
        Ok(())
    }

    pub fn restart_agent(self: &Arc<Self>, id: &AgentId) -> Result<()> {
        let agent = self.store.get_agent(id)?.context("agent not found")?;
        if agent.archived {
            bail!("agent is archived — unarchive it first");
        }
        let worktree = self.store.get_worktree(&agent.worktree_id)?.context("worktree not found")?;
        self.kill_session(&SessionRef::Agent(id.clone()));
        self.spawn_agent_session(&agent, &worktree, 80, 24)?;
        let mut broadcast_agent = agent.clone();
        broadcast_agent.alive = true;
        self.broadcast(ServerEvent::EntityUpserted { entity: Entity::Agent(broadcast_agent) });
        Ok(())
    }

    // ---- terminals ----

    pub fn create_terminal(
        self: &Arc<Self>,
        worktree_id: &WorktreeId,
        name: Option<String>,
    ) -> Result<EntityId> {
        let worktree = self.store.get_worktree(worktree_id)?.context("worktree not found")?;
        let name = name.filter(|n| !n.trim().is_empty()).unwrap_or_else(|| {
            let n = self.store.count_terminals(worktree_id).unwrap_or(0);
            format!("term-{}", n + 1)
        });
        let terminal = TerminalTab {
            id: TerminalId::generate(),
            worktree_id: worktree_id.clone(),
            name,
            sort_order: 0,
            alive: false,
        };
        self.store.insert_terminal(&terminal)?;
        self.spawn_terminal_session(&terminal, &worktree, 80, 24)?;
        let mut broadcast_term = terminal.clone();
        broadcast_term.alive = true;
        self.broadcast(ServerEvent::EntityUpserted { entity: Entity::Terminal(broadcast_term) });
        Ok(EntityId::Terminal(terminal.id))
    }

    pub fn rename_terminal(self: &Arc<Self>, id: &TerminalId, name: &str) -> Result<()> {
        if name.trim().is_empty() {
            bail!("name is empty");
        }
        self.store.rename_terminal(id, name.trim())?;
        let term = self.terminal_entity(id)?;
        self.broadcast(ServerEvent::EntityUpserted { entity: Entity::Terminal(term) });
        Ok(())
    }

    pub fn close_terminal(self: &Arc<Self>, id: &TerminalId) -> Result<()> {
        self.kill_session(&SessionRef::Terminal(id.clone()));
        self.store.delete_terminal(id)?;
        self.broadcast(ServerEvent::EntityRemoved { id: EntityId::Terminal(id.clone()) });
        Ok(())
    }

    // ---- attach / spawn ----

    /// Get the live session for an entity, lazily (re)spawning its PTY when
    /// none is running (restored agents, closed shells).
    pub fn ensure_session(
        self: &Arc<Self>,
        sref: &SessionRef,
        cols: u16,
        rows: u16,
    ) -> Result<Arc<PtySession>> {
        if let Some(s) = self.session(sref) {
            return Ok(s);
        }
        match sref {
            SessionRef::Agent(id) => {
                let agent = self.store.get_agent(id)?.context("agent not found")?;
                if agent.archived {
                    bail!("agent is archived — unarchive it first");
                }
                let worktree =
                    self.store.get_worktree(&agent.worktree_id)?.context("worktree not found")?;
                let session = self.spawn_agent_session(&agent, &worktree, cols, rows)?;
                let mut broadcast_agent = agent;
                broadcast_agent.alive = true;
                self.broadcast(ServerEvent::EntityUpserted { entity: Entity::Agent(broadcast_agent) });
                Ok(session)
            }
            SessionRef::Terminal(id) => {
                let term = self.store.get_terminal(id)?.context("terminal not found")?;
                let worktree =
                    self.store.get_worktree(&term.worktree_id)?.context("worktree not found")?;
                let session = self.spawn_terminal_session(&term, &worktree, cols, rows)?;
                let mut broadcast_term = term;
                broadcast_term.alive = true;
                self.broadcast(ServerEvent::EntityUpserted { entity: Entity::Terminal(broadcast_term) });
                Ok(session)
            }
        }
    }

    fn spawn_agent_session(
        self: &Arc<Self>,
        agent: &Agent,
        worktree: &Worktree,
        cols: u16,
        rows: u16,
    ) -> Result<Arc<PtySession>> {
        // Managed status hooks; a failure here degrades to "no status
        // updates", never blocks the spawn.
        if let Err(e) = hooks::installer::install_hooks(&worktree.path) {
            tracing::warn!(error = %e, cwd = %worktree.path.display(), "hook install failed");
        }

        // NEBULA_AGENT_CMD overrides for tests; default is claude.
        let cmd_override = std::env::var("NEBULA_AGENT_CMD").ok();
        let is_default_claude = cmd_override.is_none();
        let cmd = cmd_override.unwrap_or_else(|| "claude".into());
        let mut parts = cmd.split_whitespace().map(String::from).collect::<Vec<_>>();
        if parts.is_empty() {
            parts.push("claude".into());
        }
        let program = parts.remove(0);
        let mut args = parts;
        let resumed = if is_default_claude {
            if let Some(sid) = &agent.claude_session_id {
                args.extend(["--resume".to_string(), sid.clone()]);
                true
            } else {
                false
            }
        } else {
            false
        };

        let spec = SpawnSpec {
            program,
            args,
            cwd: worktree.path.clone(),
            env: vec![
                ("NEBULA_AGENT_ID".into(), agent.id.to_string()),
                ("NEBULA_API_URL".into(), format!("http://127.0.0.1:{}", self.hook_env.port)),
                ("NEBULA_API_TOKEN".into(), self.hook_env.token.clone()),
            ],
            scrub_env: scrubbed_env_names(),
            cols,
            rows,
        };
        let sref = SessionRef::Agent(agent.id.clone());
        let session = PtySession::spawn(sref, spec)?;
        self.install_session(session.clone());
        if resumed {
            self.arm_resume_fallback(agent.clone(), worktree.clone(), session.clone(), cols, rows);
        }
        Ok(session)
    }

    /// `claude --resume <id>` dies fast when the session is stale/deleted —
    /// fall back to a fresh session instead of leaving a dead pane.
    fn arm_resume_fallback(
        self: &Arc<Self>,
        agent: Agent,
        worktree: Worktree,
        session: Arc<PtySession>,
        cols: u16,
        rows: u16,
    ) {
        let daemon = self.clone();
        let mut rx = session.events.subscribe();
        tokio::spawn(async move {
            let early_exit = tokio::time::timeout(std::time::Duration::from_secs(2), async {
                loop {
                    match rx.recv().await {
                        Ok(PtyEvent::Exited { exit_code }) => return exit_code.unwrap_or(1) != 0,
                        Ok(_) => continue,
                        Err(broadcast::error::RecvError::Lagged(_)) => continue,
                        Err(broadcast::error::RecvError::Closed) => return false,
                    }
                }
            })
            .await;
            if early_exit != Ok(true) {
                return;
            }
            tracing::info!(agent = %agent.id, "resume failed fast — respawning fresh");
            let _ = daemon.store.set_agent_session_id(&agent.id, None);
            let mut fresh = agent.clone();
            fresh.claude_session_id = None;
            if let Ok(_session) = daemon.spawn_agent_session(&fresh, &worktree, cols, rows) {
                let mut broadcast_agent = fresh;
                broadcast_agent.alive = true;
                daemon.broadcast(ServerEvent::EntityUpserted { entity: Entity::Agent(broadcast_agent) });
            }
        });
    }

    fn spawn_terminal_session(
        self: &Arc<Self>,
        terminal: &TerminalTab,
        worktree: &Worktree,
        cols: u16,
        rows: u16,
    ) -> Result<Arc<PtySession>> {
        let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".into());
        let spec = SpawnSpec {
            program: shell,
            args: vec![],
            cwd: worktree.path.clone(),
            env: vec![],
            scrub_env: scrubbed_env_names(),
            cols,
            rows,
        };
        let sref = SessionRef::Terminal(terminal.id.clone());
        let session = PtySession::spawn(sref, spec)?;
        self.install_session(session.clone());
        Ok(session)
    }

    fn install_session(self: &Arc<Self>, session: Arc<PtySession>) {
        self.sessions.lock().unwrap().insert(session.sref.clone(), session.clone());
        self.watch_for_exit(session);
    }

    /// Once the child dies: drop it from the registry, feed the status
    /// machine (agents), and tell subscribers the entity is no longer alive.
    fn watch_for_exit(self: &Arc<Self>, session: Arc<PtySession>) {
        let daemon = self.clone();
        let mut rx = session.events.subscribe();
        let sref = session.sref.clone();
        tokio::spawn(async move {
            loop {
                match rx.recv().await {
                    Ok(PtyEvent::Exited { exit_code }) => {
                        // Deliberate kills (archive/restart/delete) remove the
                        // entry first — only a *natural* death of the still-
                        // registered session drives status, so a restart never
                        // flags the fresh PTY's agent as terminated.
                        let was_registered = {
                            let mut sessions = daemon.sessions.lock().unwrap();
                            match sessions.get(&sref) {
                                Some(current) if Arc::ptr_eq(current, &session) => {
                                    sessions.remove(&sref);
                                    true
                                }
                                _ => false,
                            }
                        };
                        if !was_registered {
                            break;
                        }
                        tracing::info!(session = ?sref, exit_code, "session exited");
                        if let SessionRef::Agent(id) = &sref {
                            daemon.apply_hook_event(id, HookEvent::SessionEnded { exit_code }, None);
                        }
                        let upsert = match &sref {
                            SessionRef::Agent(id) => daemon.agent_entity(id).map(Entity::Agent),
                            SessionRef::Terminal(id) => daemon.terminal_entity(id).map(Entity::Terminal),
                        };
                        if let Ok(entity) = upsert {
                            daemon.broadcast(ServerEvent::EntityUpserted { entity });
                        }
                        break;
                    }
                    Ok(_) => {}
                    Err(broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
        });
    }
}

/// Env vars that must never leak into plain terminals (and are re-set
/// explicitly for agent PTYs).
pub fn scrubbed_env_names() -> Vec<String> {
    vec![
        "NEBULA_AGENT_ID".into(),
        "NEBULA_API_URL".into(),
        "NEBULA_API_TOKEN".into(),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_daemon() -> Arc<Daemon> {
        let store = Store::open_in_memory().unwrap();
        Daemon::new(store, HookEnv { port: 0, token: String::new() })
    }

    fn seed_projects(daemon: &Daemon, names: &[&str]) {
        for (i, name) in names.iter().enumerate() {
            daemon
                .store
                .insert_project(&Project {
                    id: ProjectId((*name).into()),
                    name: (*name).into(),
                    repo_path: format!("/tmp/{name}").into(),
                    sort_order: i as i64,
                    divider_after: false,
                    divider_label: None,
                })
                .unwrap();
        }
    }

    /// (name, divider_after, divider_label) in display order.
    fn layout(daemon: &Daemon) -> Vec<(String, bool, Option<String>)> {
        let (projects, _, _, _) = daemon.store.load_tree().unwrap();
        projects.into_iter().map(|p| (p.name, p.divider_after, p.divider_label)).collect()
    }

    fn names(daemon: &Daemon) -> Vec<String> {
        layout(daemon).into_iter().map(|(n, _, _)| n).collect()
    }

    #[test]
    fn move_project_reorders_and_normalizes_sort_orders() {
        let daemon = test_daemon();
        seed_projects(&daemon, &["a", "b", "c", "d"]);

        daemon.move_project(&ProjectId("d".into()), -2).unwrap();
        assert_eq!(names(&daemon), ["a", "d", "b", "c"]);
        let (projects, _, _, _) = daemon.store.load_tree().unwrap();
        assert_eq!(projects.iter().map(|p| p.sort_order).collect::<Vec<_>>(), [0, 1, 2, 3]);

        // Edge moves clamp to no-ops.
        daemon.move_project(&ProjectId("a".into()), -1).unwrap();
        daemon.move_project(&ProjectId("c".into()), 5).unwrap();
        assert_eq!(names(&daemon), ["a", "d", "b", "c"]);
    }

    #[test]
    fn dividers_stay_pinned_while_projects_move_through() {
        let daemon = test_daemon();
        seed_projects(&daemon, &["a", "b", "c", "d"]);
        // Groups: [a b] [c d], labeled "work".
        daemon.set_project_divider(&ProjectId("b".into()), true, Some("work".into())).unwrap();

        // c crosses the divider into the first group; the divider (and its
        // label) keeps marking the gap between slots 1 and 2.
        daemon.move_project(&ProjectId("c".into()), -1).unwrap();
        assert_eq!(
            layout(&daemon),
            [
                ("a".to_string(), false, None),
                ("c".to_string(), true, Some("work".to_string())),
                ("b".to_string(), false, None),
                ("d".to_string(), false, None),
            ]
        );

        // Relabeling keeps the divider; removing it drops the label too.
        daemon.set_project_divider(&ProjectId("c".into()), true, Some("play".into())).unwrap();
        assert_eq!(layout(&daemon)[1].2.as_deref(), Some("play"));
        daemon.set_project_divider(&ProjectId("c".into()), false, Some("ignored".into())).unwrap();
        assert!(layout(&daemon).iter().all(|(_, divider, label)| !divider && label.is_none()));
    }
}
