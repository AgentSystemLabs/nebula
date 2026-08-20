//! The daemon's world: persisted entity tree + live PTY sessions, and the
//! operations the IPC surface exposes over them.

use crate::git;
use crate::hooks::{self, HookEnv};
use crate::pty::{PtyEvent, PtySession, SpawnSpec};
use crate::status::{AgentStatusMachine, Effect, HookEvent};
use crate::store::Store;
use anyhow::{bail, Context, Result};
use nebula_core::{
    Agent, AgentId, AgentKind, AgentStatus, Entity, EntityId, Project, ProjectId, ServerEvent,
    SessionRef, TerminalId, TerminalTab, Worktree, WorktreeId,
};
use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::broadcast;

/// A warm agent CLI older than this is reaped — it holds memory and its
/// conversation context grows stale.
const PREWARM_MAX_AGE: Duration = Duration::from_secs(15 * 60);
/// Hook events buffered on a warm session before its row exists (oldest
/// dropped beyond this).
const PREWARM_HOOK_BUFFER_CAP: usize = 64;

/// A pre-spawned agent CLI waiting to be adopted by the next CreateAgent for
/// the same (worktree, kind). The PTY lives in the normal sessions map under
/// a pre-generated agent id, so its NEBULA_AGENT_ID env is already the id
/// the adopted row will use. Hook events that arrive before the row exists
/// (SessionStart carries the resume session id) are buffered here and
/// replayed at adoption.
struct PrewarmEntry {
    agent_id: AgentId,
    spawned_at: Instant,
    buffered_hooks: Vec<(HookEvent, Option<String>)>,
}

/// Wall-clock epoch ms, matching the store's `status_changed_at` stamps.
fn epoch_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

pub struct Daemon {
    sessions: Mutex<HashMap<SessionRef, Arc<PtySession>>>,
    status_machines: Mutex<HashMap<AgentId, AgentStatusMachine>>,
    pub hook_env: HookEnv,
    pub store: Store,
    /// Entity/status deltas fanned out to every subscribed client.
    pub events: broadcast::Sender<ServerEvent>,
    pub shutdown: tokio_util::sync::CancellationToken,
    /// Serializes worktree create/delete with the background auto-sync so
    /// a checkout is never adopted twice while its row is mid-insert.
    worktree_ops: tokio::sync::Mutex<()>,
    /// Warm agent CLIs awaiting adoption, at most one per (worktree, kind).
    prewarmed: Mutex<HashMap<(WorktreeId, AgentKind), PrewarmEntry>>,
    /// Cached `command -v` results per CLI so a missing binary doesn't get
    /// re-probed (login shell spawn) on every prewarm request.
    cli_probes: Mutex<HashMap<AgentKind, (bool, Instant)>>,
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
            worktree_ops: tokio::sync::Mutex::new(()),
            prewarmed: Mutex::new(HashMap::new()),
            cli_probes: Mutex::new(HashMap::new()),
        })
    }

    // ---- status machine plumbing ----

    /// Feed one hook (or synthetic) event through the agent's status machine
    /// and apply the resulting effects (persist + broadcast).
    pub fn apply_hook_event(
        &self,
        agent_id: &AgentId,
        event: HookEvent,
        session_id: Option<String>,
    ) {
        enum Outcome {
            Effects(Vec<Effect>),
            UnknownAgent(HookEvent, Option<String>),
        }
        let outcome = {
            let mut machines = self.status_machines.lock().unwrap();
            match machines.entry(agent_id.clone()) {
                std::collections::hash_map::Entry::Occupied(e) => Outcome::Effects(
                    e.into_mut()
                        .handle(event, session_id.as_deref(), Instant::now()),
                ),
                std::collections::hash_map::Entry::Vacant(slot) => {
                    // Lazily seed from the persisted row.
                    match self.store.get_agent(agent_id) {
                        Ok(Some(agent)) => Outcome::Effects(
                            slot.insert(AgentStatusMachine::new(agent.status, agent.session_id))
                                .handle(event, session_id.as_deref(), Instant::now()),
                        ),
                        _ => Outcome::UnknownAgent(event, session_id),
                    }
                }
            }
        };
        match outcome {
            Outcome::Effects(effects) => self.apply_status_effects(agent_id, effects),
            // Ids with no row are prewarmed sessions (buffer for replay at
            // adoption) or stale env / deleted agents (dropped, as before).
            Outcome::UnknownAgent(event, session_id) => {
                self.buffer_prewarm_hook(agent_id, event, session_id)
            }
        }
    }

    fn buffer_prewarm_hook(&self, agent_id: &AgentId, event: HookEvent, session_id: Option<String>) {
        let mut pool = self.prewarmed.lock().unwrap();
        if let Some(entry) = pool.values_mut().find(|e| &e.agent_id == agent_id) {
            if entry.buffered_hooks.len() >= PREWARM_HOOK_BUFFER_CAP {
                entry.buffered_hooks.remove(0);
            }
            entry.buffered_hooks.push((event, session_id));
        }
    }

    /// Deferred-finish recheck across all machines (runs on a timer).
    pub fn tick_status_machines(&self) {
        let now = Instant::now();
        let ticked: Vec<(AgentId, Vec<Effect>)> = {
            let mut machines = self.status_machines.lock().unwrap();
            machines
                .iter_mut()
                .map(|(id, m)| (id.clone(), m.tick(now)))
                .collect()
        };
        for (id, effects) in ticked {
            self.apply_status_effects(&id, effects);
        }
    }

    fn apply_status_effects(&self, agent_id: &AgentId, effects: Vec<Effect>) {
        for effect in effects {
            match effect {
                Effect::SetStatus(status) => {
                    let changed_at = match self.store.set_agent_status(agent_id, status) {
                        Ok(stamp) => stamp,
                        Err(e) => {
                            tracing::warn!(error = %e, "persist status failed");
                            epoch_ms()
                        }
                    };
                    self.broadcast(ServerEvent::StatusChanged {
                        agent: agent_id.clone(),
                        status,
                        changed_at,
                    });
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

    pub async fn add_project(
        self: &Arc<Self>,
        path: &Path,
        name: Option<String>,
        create_missing: bool,
    ) -> Result<EntityId> {
        if create_missing && !path.exists() {
            tokio::fs::create_dir_all(path)
                .await
                .with_context(|| format!("create {}", path.display()))?;
            if crate::config::Config::load().git_init_on_create {
                git::init(path).await?;
            }
        }
        let toplevel = git::repo_toplevel(path)
            .await
            .with_context(|| format!("{} is not a git repository", path.display()))?;
        if self.store.project_by_path(&toplevel)?.is_some() {
            bail!("project already added: {}", toplevel.display());
        }
        let name = name.unwrap_or_else(|| {
            toplevel
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| "project".into())
        });
        let project = Project {
            id: ProjectId::generate(),
            name,
            repo_path: toplevel.clone(),
            sort_order: self.store.next_project_sort_order()?,
            divider_after: false,
            divider_label: None,
            divider_before: false,
            divider_before_label: None,
        };
        self.store.insert_project(&project)?;
        self.broadcast(ServerEvent::EntityUpserted {
            entity: Entity::Project(project.clone()),
        });

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
                pinned: false,
                sort_order: 0,
            };
            first = false;
            self.store.insert_worktree(&worktree)?;
            self.broadcast(ServerEvent::EntityUpserted {
                entity: Entity::Worktree(worktree),
            });
        }
        Ok(EntityId::Project(project.id))
    }

    pub fn remove_project(self: &Arc<Self>, id: &ProjectId) -> Result<()> {
        // Kill any live sessions under this project first.
        let (projects, worktrees, agents, terminals) = self.store.load_tree()?;
        // The leading divider belongs to the list, not the top project:
        // removing that project hands it down to the next one.
        if let (Some(first), Some(second)) = (projects.first(), projects.get(1)) {
            if &first.id == id && first.divider_before {
                let mut heir = second.clone();
                heir.divider_before = true;
                heir.divider_before_label = first.divider_before_label.clone();
                self.store.set_project_position(&heir)?;
                self.broadcast(ServerEvent::EntityUpserted {
                    entity: Entity::Project(heir),
                });
            }
        }
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
        self.kill_prewarmed_in(&wt_ids);
        // Removing a project only forgets it in nebula — never touches disk.
        self.store.delete_project(id)?;
        self.broadcast(ServerEvent::EntityRemoved {
            id: EntityId::Project(id.clone()),
        });
        Ok(())
    }

    /// Move a project `delta` rows in the displayed list, where dividers
    /// occupy rows of their own (clamped at the edges). A project steps into
    /// the gap beside a divider before it swaps with the next project, so
    /// repeated single-row moves can park it directly above or below a
    /// divider — including below a divider that ends up above the whole
    /// list (the leading divider). Sort orders are rewritten to the display
    /// index for every project, which also normalizes legacy all-zero
    /// orders on first use.
    pub fn move_project(self: &Arc<Self>, id: &ProjectId, delta: i64) -> Result<()> {
        let (projects, _, _, _) = self.store.load_tree()?;
        #[derive(Clone, PartialEq)]
        enum Row {
            Project(ProjectId),
            Divider(Option<String>),
        }
        let mut rows: Vec<Row> = Vec::new();
        if let Some(first) = projects.first() {
            if first.divider_before {
                rows.push(Row::Divider(first.divider_before_label.clone()));
            }
        }
        for p in &projects {
            rows.push(Row::Project(p.id.clone()));
            if p.divider_after {
                rows.push(Row::Divider(p.divider_label.clone()));
            }
        }
        let Some(pos) = rows
            .iter()
            .position(|r| matches!(r, Row::Project(pid) if pid == id))
        else {
            bail!("project not found");
        };
        let mut target = (pos as i64 + delta).clamp(0, rows.len() as i64 - 1) as usize;
        let rows = loop {
            let mut moved = rows.clone();
            let row = moved.remove(pos);
            moved.insert(target, row);
            // Two dividers can't share a gap: a move that would leave them
            // stacked (no project left between) pushes one row further so
            // the keystroke still lands the project across, or no-ops at
            // the edge.
            let stacked = moved
                .windows(2)
                .any(|w| matches!(w, [Row::Divider(_), Row::Divider(_)]));
            if !stacked && moved != rows {
                break moved;
            }
            let next = (target as i64 + delta.signum()).clamp(0, rows.len() as i64 - 1) as usize;
            if next == target {
                return Ok(());
            }
            target = next;
        };
        type Position = (i64, bool, Option<String>, bool, Option<String>);
        fn position(p: &Project) -> Position {
            (
                p.sort_order,
                p.divider_after,
                p.divider_label.clone(),
                p.divider_before,
                p.divider_before_label.clone(),
            )
        }
        let before: HashMap<ProjectId, Position> =
            projects.iter().map(|p| (p.id.clone(), position(p))).collect();
        let mut by_id: HashMap<ProjectId, Project> =
            projects.into_iter().map(|p| (p.id.clone(), p)).collect();
        let mut ordered: Vec<Project> = Vec::new();
        let mut leading: Option<Option<String>> = None;
        for row in rows {
            match row {
                Row::Project(pid) => {
                    let mut project = by_id.remove(&pid).expect("row ids come from projects");
                    project.sort_order = ordered.len() as i64;
                    project.divider_after = false;
                    project.divider_label = None;
                    project.divider_before = false;
                    project.divider_before_label = None;
                    ordered.push(project);
                }
                Row::Divider(label) => match ordered.last_mut() {
                    Some(owner) => {
                        owner.divider_after = true;
                        owner.divider_label = label;
                    }
                    // Ahead of every project: the leading divider, re-owned
                    // by whichever project ends up on top.
                    None => leading = Some(label),
                },
            }
        }
        if let Some(label) = leading {
            let first = ordered.first_mut().expect("rows contain every project");
            first.divider_before = true;
            first.divider_before_label = label;
        }
        for project in &ordered {
            if before.get(&project.id) != Some(&position(project)) {
                self.store.set_project_position(project)?;
                self.broadcast(ServerEvent::EntityUpserted {
                    entity: Entity::Project(project.clone()),
                });
            }
        }
        Ok(())
    }

    /// Set or clear one of a project's dividers. `before` addresses the
    /// leading divider (drawn above the whole list) — only the first
    /// project can carry that one.
    pub fn set_project_divider(
        self: &Arc<Self>,
        id: &ProjectId,
        before: bool,
        present: bool,
        label: Option<String>,
    ) -> Result<()> {
        let mut project = self.store.get_project(id)?.context("project not found")?;
        if before && present {
            let (projects, _, _, _) = self.store.load_tree()?;
            if projects.first().map(|p| &p.id) != Some(id) {
                bail!("only the first project can hold the leading divider");
            }
        }
        // A removed divider keeps no label.
        let label = if present {
            label.filter(|l| !l.trim().is_empty())
        } else {
            None
        };
        let slot = if before {
            (&mut project.divider_before, &mut project.divider_before_label)
        } else {
            (&mut project.divider_after, &mut project.divider_label)
        };
        if (*slot.0, &*slot.1) == (present, &label) {
            return Ok(());
        }
        *slot.0 = present;
        *slot.1 = label;
        self.store.set_project_position(&project)?;
        self.broadcast(ServerEvent::EntityUpserted {
            entity: Entity::Project(project),
        });
        Ok(())
    }

    /// Move project `id`'s divider (`before` picks which one) to the
    /// neighboring gap (sign of `delta`; one step per call). The divider
    /// under the first project can hop above it — the leading divider —
    /// and back down. No-op past the list's edges or when the destination
    /// gap already has a divider — two dividers can't share a gap.
    pub fn move_divider(self: &Arc<Self>, id: &ProjectId, before: bool, delta: i64) -> Result<()> {
        if delta == 0 {
            return Ok(());
        }
        let (projects, _, _, _) = self.store.load_tree()?;
        let Some(index) = projects.iter().position(|p| &p.id == id) else {
            bail!("project not found");
        };
        let down = delta.signum() > 0;
        if before {
            if index != 0 || !projects[0].divider_before {
                bail!("project has no leading divider");
            }
            if !down {
                return Ok(()); // already above everything
            }
            // Hop from above the first project to below it.
            if projects[0].divider_after {
                return Ok(());
            }
            let label = projects[0].divider_before_label.clone();
            self.set_project_divider(id, true, false, None)?;
            self.set_project_divider(id, false, true, label)?;
            return Ok(());
        }
        if !projects[index].divider_after {
            bail!("project has no divider");
        }
        let label = projects[index].divider_label.clone();
        if index == 0 && !down {
            // Hop from below the first project to above it.
            if projects[0].divider_before {
                return Ok(());
            }
            self.set_project_divider(id, false, false, None)?;
            self.set_project_divider(id, true, true, label)?;
            return Ok(());
        }
        let neighbor = index as i64 + delta.signum();
        let Some(neighbor) = usize::try_from(neighbor).ok().and_then(|i| projects.get(i)) else {
            return Ok(()); // no project on that side
        };
        if neighbor.divider_after {
            return Ok(());
        }
        let neighbor_id = neighbor.id.clone();
        self.set_project_divider(id, false, false, None)?;
        self.set_project_divider(&neighbor_id, false, true, label)?;
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
        let _ops = self.worktree_ops.lock().await;
        let project = self
            .store
            .get_project(project_id)?
            .context("project not found")?;
        let path = git::add_worktree(&project.repo_path, branch, base).await?;
        let worktree = Worktree {
            id: WorktreeId::generate(),
            project_id: project_id.clone(),
            path,
            branch: branch.to_string(),
            is_main: false,
            pinned: false,
            sort_order: 0,
        };
        self.store.insert_worktree(&worktree)?;
        self.broadcast(ServerEvent::EntityUpserted {
            entity: Entity::Worktree(worktree.clone()),
        });
        Ok(EntityId::Worktree(worktree.id))
    }

    pub async fn delete_worktree(self: &Arc<Self>, id: &WorktreeId, force: bool) -> Result<()> {
        let _ops = self.worktree_ops.lock().await;
        let worktree = self.store.get_worktree(id)?.context("worktree not found")?;
        if worktree.is_main {
            bail!("cannot delete the main checkout — remove the project instead");
        }
        let project = self
            .store
            .get_project(&worktree.project_id)?
            .context("project not found")?;

        // Kill sessions living in this worktree.
        let (_, _, agents, terminals) = self.store.load_tree()?;
        for a in agents.iter().filter(|a| &a.worktree_id == id) {
            self.kill_session(&SessionRef::Agent(a.id.clone()));
        }
        for t in terminals.iter().filter(|t| &t.worktree_id == id) {
            self.kill_session(&SessionRef::Terminal(t.id.clone()));
        }
        self.kill_prewarmed_in(std::slice::from_ref(id));

        git::remove_worktree(&project.repo_path, &worktree.path, force).await?;
        self.store.delete_worktree(id)?;
        self.broadcast(ServerEvent::EntityRemoved {
            id: EntityId::Worktree(id.clone()),
        });
        Ok(())
    }

    pub fn set_worktree_pinned(self: &Arc<Self>, id: &WorktreeId, pinned: bool) -> Result<()> {
        self.store.set_worktree_pinned(id, pinned)?;
        let worktree = self.store.get_worktree(id)?.context("worktree not found")?;
        self.broadcast(ServerEvent::EntityUpserted {
            entity: Entity::Worktree(worktree),
        });
        Ok(())
    }

    /// Reconcile a project's worktree rows with `git worktree list` so
    /// checkouts made outside nebula (an agent running `git worktree add`,
    /// manual CLI use) appear without a restart. Adopts unknown checkouts;
    /// refreshes the branch on known rows after an in-place checkout;
    /// drops rows whose checkout vanished — except the main row and rows
    /// that still have sessions, which the user must delete deliberately.
    pub async fn sync_project_worktrees(self: &Arc<Self>, project: &Project) -> Result<()> {
        let _ops = self.worktree_ops.lock().await;
        let entries = git::list_worktrees(&project.repo_path).await?;
        let (_, worktrees, agents, terminals) = self.store.load_tree()?;
        let ours: Vec<&Worktree> = worktrees
            .iter()
            .filter(|w| w.project_id == project.id)
            .collect();
        for entry in &entries {
            if let Some(known) = ours.iter().find(|w| w.path == entry.path) {
                // Branch switched in place (checkout on the root or inside a
                // linked worktree): refresh the stored name so the row tracks
                // reality instead of the branch at adoption time.
                if known.branch != entry.branch {
                    self.store
                        .update_worktree_branch(&known.id, &entry.branch)?;
                    let mut updated = (*known).clone();
                    updated.branch = entry.branch.clone();
                    self.broadcast(ServerEvent::EntityUpserted {
                        entity: Entity::Worktree(updated),
                    });
                }
                continue;
            }
            let worktree = Worktree {
                id: WorktreeId::generate(),
                project_id: project.id.clone(),
                path: entry.path.clone(),
                branch: entry.branch.clone(),
                is_main: false,
                pinned: false,
                sort_order: 0,
            };
            self.store.insert_worktree(&worktree)?;
            self.broadcast(ServerEvent::EntityUpserted {
                entity: Entity::Worktree(worktree),
            });
        }
        for w in ours {
            if w.is_main || entries.iter().any(|e| e.path == w.path) {
                continue;
            }
            let occupied = agents.iter().any(|a| a.worktree_id == w.id)
                || terminals.iter().any(|t| t.worktree_id == w.id);
            if occupied {
                continue;
            }
            self.store.delete_worktree(&w.id)?;
            self.broadcast(ServerEvent::EntityRemoved {
                id: EntityId::Worktree(w.id.clone()),
            });
        }
        Ok(())
    }

    // ---- agents ----

    pub fn create_agent(
        self: &Arc<Self>,
        worktree_id: &WorktreeId,
        name: &str,
        kind: AgentKind,
    ) -> Result<EntityId> {
        let worktree = self
            .store
            .get_worktree(worktree_id)?
            .context("worktree not found")?;
        // A warm session for this (worktree, kind) hands over its PTY and
        // its pre-generated id — the CLI booted while the user typed the
        // name, so the create feels instant.
        let adopted = self.take_prewarmed(worktree_id, kind);
        let agent = Agent {
            id: adopted
                .as_ref()
                .map(|e| e.agent_id.clone())
                .unwrap_or_else(AgentId::generate),
            worktree_id: worktree_id.clone(),
            name: if name.trim().is_empty() {
                "agent".into()
            } else {
                name.trim().to_string()
            },
            status: AgentStatus::Fresh,
            archived: false,
            pinned: false,
            kind,
            session_id: None,
            sort_order: 0,
            status_changed_at: epoch_ms(),
            alive: false,
        };
        self.store.insert_agent(&agent)?;
        if adopted.is_none() {
            // Cold path: boot the CLI right away.
            self.spawn_agent_session(&agent, &worktree, 80, 24)?;
        }
        let mut broadcast_agent = agent.clone();
        broadcast_agent.alive = true;
        self.broadcast(ServerEvent::EntityUpserted {
            entity: Entity::Agent(broadcast_agent),
        });
        if let Some(entry) = adopted {
            // Now that the row exists, replay the hooks the warm CLI fired
            // before adoption (SessionStart stores the resume session id).
            for (event, sid) in entry.buffered_hooks {
                self.apply_hook_event(&agent.id, event, sid);
            }
        }
        Ok(EntityId::Agent(agent.id))
    }

    // ---- prewarm pool ----

    /// Pre-spawn an agent CLI for (worktree, kind) so the next create adopts
    /// an already-booted session. Fail-soft by design: a disabled config,
    /// missing CLI, or spawn error just means the create stays cold.
    pub async fn prewarm_agent(
        self: &Arc<Self>,
        worktree_id: &WorktreeId,
        kind: AgentKind,
    ) -> Result<()> {
        if !crate::config::Config::load().prewarm_agents {
            return Ok(());
        }
        let Some(worktree) = self.store.get_worktree(worktree_id)? else {
            return Ok(());
        };
        {
            // One warm slot per key; keep a live one, replace a dead one.
            let mut pool = self.prewarmed.lock().unwrap();
            if let Some(entry) = pool.get(&(worktree_id.clone(), kind)) {
                if self.is_alive(&SessionRef::Agent(entry.agent_id.clone())) {
                    return Ok(());
                }
                pool.remove(&(worktree_id.clone(), kind));
            }
        }
        if !self.cli_available(kind).await {
            tracing::debug!(kind = kind.as_str(), "prewarm skipped: CLI not installed");
            return Ok(());
        }
        let agent = Agent {
            id: AgentId::generate(),
            worktree_id: worktree_id.clone(),
            name: "prewarm".into(),
            status: AgentStatus::Fresh,
            archived: false,
            pinned: false,
            kind,
            session_id: None,
            sort_order: 0,
            status_changed_at: 0,
            alive: false,
        };
        self.spawn_agent_session(&agent, &worktree, 80, 24)?;
        tracing::info!(agent = %agent.id, kind = kind.as_str(), worktree = %worktree.branch, "prewarmed agent session");
        let replaced = self.prewarmed.lock().unwrap().insert(
            (worktree_id.clone(), kind),
            PrewarmEntry {
                agent_id: agent.id,
                spawned_at: Instant::now(),
                buffered_hooks: Vec::new(),
            },
        );
        // Two racing prewarms for the same key: the loser's session would
        // otherwise leak as an orphan CLI process.
        if let Some(old) = replaced {
            self.kill_session(&SessionRef::Agent(old.agent_id));
        }
        Ok(())
    }

    /// Pop the warm entry for (worktree, kind) if its PTY is still running.
    /// A dead entry (CLI missing/crashed while warm) is dropped so the
    /// caller falls back to a cold spawn.
    fn take_prewarmed(&self, worktree_id: &WorktreeId, kind: AgentKind) -> Option<PrewarmEntry> {
        let entry = self
            .prewarmed
            .lock()
            .unwrap()
            .remove(&(worktree_id.clone(), kind))?;
        self.is_alive(&SessionRef::Agent(entry.agent_id.clone()))
            .then_some(entry)
    }

    /// Drop warm sessions that died or sat unclaimed past the max age
    /// (runs on the daemon's periodic tick).
    pub fn reap_prewarmed(&self) {
        let doomed: Vec<AgentId> = {
            let mut pool = self.prewarmed.lock().unwrap();
            let expired: Vec<_> = pool
                .iter()
                .filter(|(_, e)| {
                    e.spawned_at.elapsed() > PREWARM_MAX_AGE
                        || !self.is_alive(&SessionRef::Agent(e.agent_id.clone()))
                })
                .map(|(k, _)| k.clone())
                .collect();
            expired
                .into_iter()
                .filter_map(|k| pool.remove(&k))
                .map(|e| e.agent_id)
                .collect()
        };
        for id in doomed {
            tracing::debug!(agent = %id, "reaping prewarmed session");
            self.kill_session(&SessionRef::Agent(id));
        }
    }

    /// Kill warm sessions homed in any of these worktrees (worktree delete,
    /// project remove — their store rows are gone or going).
    fn kill_prewarmed_in(&self, worktree_ids: &[WorktreeId]) {
        let doomed: Vec<AgentId> = {
            let mut pool = self.prewarmed.lock().unwrap();
            let keys: Vec<_> = pool
                .keys()
                .filter(|(w, _)| worktree_ids.contains(w))
                .cloned()
                .collect();
            keys.into_iter()
                .filter_map(|k| pool.remove(&k))
                .map(|e| e.agent_id)
                .collect()
        };
        for id in doomed {
            self.kill_session(&SessionRef::Agent(id));
        }
    }

    /// Is the kind's CLI on the user's PATH (as their login shell sees it)?
    /// Cached: hits for an hour, misses for a minute so a just-installed CLI
    /// gets picked up quickly. Probe trouble (timeout, spawn error) fails
    /// open — a doomed warm spawn is still graceful.
    async fn cli_available(&self, kind: AgentKind) -> bool {
        if std::env::var("NEBULA_AGENT_CMD").is_ok() {
            return true; // test override is spawned verbatim
        }
        const OK_TTL: Duration = Duration::from_secs(3600);
        const FAIL_TTL: Duration = Duration::from_secs(60);
        {
            let probes = self.cli_probes.lock().unwrap();
            if let Some((ok, at)) = probes.get(&kind) {
                if at.elapsed() < if *ok { OK_TTL } else { FAIL_TTL } {
                    return *ok;
                }
            }
        }
        let check = format!("command -v '{}' >/dev/null 2>&1", kind.cli_program());
        let mut probe = tokio::process::Command::new(user_shell());
        probe
            .args(["-l", "-i", "-c", &check])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            // A timed-out probe must die with the dropped future, not linger.
            .kill_on_drop(true);
        // Own session: the interactive shell must not reach the daemon's
        // controlling terminal (--foreground runs have one). zsh's job-control
        // init opens /dev/tty and makes itself the foreground process group,
        // SIGTTIN-stopping whatever TUI owns that terminal.
        unsafe {
            probe.pre_exec(|| match nix::unistd::setsid() {
                Ok(_) => Ok(()),
                Err(errno) => Err(std::io::Error::from_raw_os_error(errno as i32)),
            });
        }
        let status = tokio::time::timeout(Duration::from_secs(5), probe.status()).await;
        match status {
            Ok(Ok(status)) => {
                let ok = status.success();
                self.cli_probes
                    .lock()
                    .unwrap()
                    .insert(kind, (ok, Instant::now()));
                ok
            }
            _ => true,
        }
    }

    pub fn rename_agent(self: &Arc<Self>, id: &AgentId, name: &str) -> Result<()> {
        if name.trim().is_empty() {
            bail!("name is empty");
        }
        self.store.rename_agent(id, name.trim())?;
        let agent = self.agent_entity(id)?;
        self.broadcast(ServerEvent::EntityUpserted {
            entity: Entity::Agent(agent),
        });
        Ok(())
    }

    /// Re-home an agent row under another worktree of the same project. The
    /// live PTY (if any) is untouched — this moves where the row lives in
    /// the tree, not where the process runs; the next respawn uses the new
    /// worktree's path.
    pub fn move_agent(self: &Arc<Self>, id: &AgentId, worktree_id: &WorktreeId) -> Result<()> {
        let agent = self.store.get_agent(id)?.context("agent not found")?;
        if &agent.worktree_id == worktree_id {
            return Ok(());
        }
        let target = self
            .store
            .get_worktree(worktree_id)?
            .context("worktree not found")?;
        let current = self
            .store
            .get_worktree(&agent.worktree_id)?
            .context("worktree not found")?;
        if target.project_id != current.project_id {
            bail!("target worktree belongs to a different project");
        }
        self.store.set_agent_worktree(id, worktree_id)?;
        let agent = self.agent_entity(id)?;
        self.broadcast(ServerEvent::EntityUpserted {
            entity: Entity::Agent(agent),
        });
        Ok(())
    }

    /// A hook payload reported the agent CLI's working directory. When that
    /// directory sits inside a *different* worktree of the same project (the
    /// session entered a worktree it created mid-conversation), re-home the
    /// agent row so the tree reflects where the work actually happens.
    /// Fail-soft: any error leaves the row where it is.
    pub fn reparent_agent_by_cwd(
        self: &Arc<Self>,
        agent_id: &AgentId,
        cwd: &str,
        payload_session_id: Option<&str>,
        captures_session: bool,
    ) {
        if let Err(e) =
            self.try_reparent_agent_by_cwd(agent_id, cwd, payload_session_id, captures_session)
        {
            tracing::warn!(agent = %agent_id, error = %e, "cwd reparent failed");
        }
    }

    fn try_reparent_agent_by_cwd(
        self: &Arc<Self>,
        agent_id: &AgentId,
        cwd: &str,
        payload_session_id: Option<&str>,
        captures_session: bool,
    ) -> Result<()> {
        let Some(agent) = self.store.get_agent(agent_id)? else {
            return Ok(());
        };
        if agent.archived {
            return Ok(());
        }
        // Same foreign-session rule as the status machine: a payload from a
        // different CLI session only counts when the event (re)establishes
        // session ownership (UserPromptSubmit / SessionStart).
        if !captures_session {
            if let (Some(mine), Some(theirs)) = (agent.session_id.as_deref(), payload_session_id)
            {
                if mine != theirs {
                    return Ok(());
                }
            }
        }
        let Some(current) = self.store.get_worktree(&agent.worktree_id)? else {
            return Ok(());
        };
        let cwd = canonical_or_raw(Path::new(cwd));
        let (_, worktrees, _, _) = self.store.load_tree()?;
        // Deepest worktree of the same project containing cwd — nested
        // layouts (checkouts under the repo root) must not resolve to the
        // root row just because the root path is also a prefix.
        let target = worktrees
            .into_iter()
            .filter(|w| w.project_id == current.project_id)
            .map(|w| {
                let canonical = canonical_or_raw(&w.path);
                (w, canonical)
            })
            .filter(|(_, canonical)| cwd.starts_with(canonical))
            .max_by_key(|(_, canonical)| canonical.components().count());
        if let Some((worktree, _)) = target {
            if worktree.id != agent.worktree_id {
                tracing::info!(
                    agent = %agent_id,
                    from = %current.branch,
                    to = %worktree.branch,
                    "agent re-homed by hook cwd"
                );
                self.move_agent(agent_id, &worktree.id)?;
            }
        }
        Ok(())
    }

    pub fn archive_agent(self: &Arc<Self>, id: &AgentId) -> Result<()> {
        self.kill_session(&SessionRef::Agent(id.clone()));
        self.store.set_agent_archived(id, true)?;
        let agent = self.agent_entity(id)?;
        self.broadcast(ServerEvent::EntityUpserted {
            entity: Entity::Agent(agent),
        });
        Ok(())
    }

    pub fn unarchive_agent(self: &Arc<Self>, id: &AgentId) -> Result<()> {
        self.store.set_agent_archived(id, false)?;
        let agent = self.agent_entity(id)?;
        self.broadcast(ServerEvent::EntityUpserted {
            entity: Entity::Agent(agent),
        });
        Ok(())
    }

    pub fn set_agent_pinned(self: &Arc<Self>, id: &AgentId, pinned: bool) -> Result<()> {
        self.store.set_agent_pinned(id, pinned)?;
        let agent = self.agent_entity(id)?;
        self.broadcast(ServerEvent::EntityUpserted {
            entity: Entity::Agent(agent),
        });
        Ok(())
    }

    pub fn delete_agent(self: &Arc<Self>, id: &AgentId) -> Result<()> {
        self.kill_session(&SessionRef::Agent(id.clone()));
        self.store.delete_agent(id)?;
        self.broadcast(ServerEvent::EntityRemoved {
            id: EntityId::Agent(id.clone()),
        });
        Ok(())
    }

    pub fn restart_agent(self: &Arc<Self>, id: &AgentId) -> Result<()> {
        let agent = self.store.get_agent(id)?.context("agent not found")?;
        if agent.archived {
            bail!("agent is archived — unarchive it first");
        }
        let worktree = self
            .store
            .get_worktree(&agent.worktree_id)?
            .context("worktree not found")?;
        self.kill_session(&SessionRef::Agent(id.clone()));
        self.spawn_agent_session(&agent, &worktree, 80, 24)?;
        let mut broadcast_agent = agent.clone();
        broadcast_agent.alive = true;
        self.broadcast(ServerEvent::EntityUpserted {
            entity: Entity::Agent(broadcast_agent),
        });
        Ok(())
    }

    // ---- terminals ----

    pub fn create_terminal(
        self: &Arc<Self>,
        worktree_id: &WorktreeId,
        name: Option<String>,
    ) -> Result<EntityId> {
        let worktree = self
            .store
            .get_worktree(worktree_id)?
            .context("worktree not found")?;
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
        self.broadcast(ServerEvent::EntityUpserted {
            entity: Entity::Terminal(broadcast_term),
        });
        Ok(EntityId::Terminal(terminal.id))
    }

    pub fn rename_terminal(self: &Arc<Self>, id: &TerminalId, name: &str) -> Result<()> {
        if name.trim().is_empty() {
            bail!("name is empty");
        }
        self.store.rename_terminal(id, name.trim())?;
        let term = self.terminal_entity(id)?;
        self.broadcast(ServerEvent::EntityUpserted {
            entity: Entity::Terminal(term),
        });
        Ok(())
    }

    pub fn close_terminal(self: &Arc<Self>, id: &TerminalId) -> Result<()> {
        self.kill_session(&SessionRef::Terminal(id.clone()));
        self.store.delete_terminal(id)?;
        self.broadcast(ServerEvent::EntityRemoved {
            id: EntityId::Terminal(id.clone()),
        });
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
                let worktree = self
                    .store
                    .get_worktree(&agent.worktree_id)?
                    .context("worktree not found")?;
                let session = self.spawn_agent_session(&agent, &worktree, cols, rows)?;
                let mut broadcast_agent = agent;
                broadcast_agent.alive = true;
                self.broadcast(ServerEvent::EntityUpserted {
                    entity: Entity::Agent(broadcast_agent),
                });
                Ok(session)
            }
            SessionRef::Terminal(id) => {
                let term = self.store.get_terminal(id)?.context("terminal not found")?;
                let worktree = self
                    .store
                    .get_worktree(&term.worktree_id)?
                    .context("worktree not found")?;
                let session = self.spawn_terminal_session(&term, &worktree, cols, rows)?;
                let mut broadcast_term = term;
                broadcast_term.alive = true;
                self.broadcast(ServerEvent::EntityUpserted {
                    entity: Entity::Terminal(broadcast_term),
                });
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
        let install_result = match agent.kind {
            AgentKind::Claude => hooks::installer::install_claude_hooks(&worktree.path),
            AgentKind::Codex => hooks::installer::install_codex_hooks(&worktree.path),
            AgentKind::Cursor => hooks::installer::install_cursor_hooks(&worktree.path),
        };
        if let Err(e) = install_result {
            tracing::warn!(error = %e, cwd = %worktree.path.display(), "hook install failed");
        }

        // NEBULA_AGENT_CMD overrides for tests; default is the kind's CLI.
        let cmd_override = std::env::var("NEBULA_AGENT_CMD").ok();
        let (program, args, resumed) = agent_spawn_command(
            agent.kind,
            agent.session_id.as_deref(),
            cmd_override.as_deref(),
        );
        // Run the agent through the user's login+interactive shell so it sees
        // the same env as a Terminal.app tab (~/.zprofile, ~/.zshrc,
        // path_helper) instead of the daemon's inherited-at-boot env.
        // Overrides (tests) stay verbatim.
        let (program, args) = if cmd_override.is_some() {
            (program, args)
        } else {
            login_shell_wrap(&user_shell(), &program, &args)
        };

        let spec = SpawnSpec {
            program,
            args,
            cwd: worktree.path.clone(),
            env: vec![
                ("NEBULA_AGENT_ID".into(), agent.id.to_string()),
                (
                    "NEBULA_API_URL".into(),
                    format!("http://127.0.0.1:{}", self.hook_env.port),
                ),
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

    /// A resumed session (`claude --resume` / `codex resume` /
    /// `cursor-agent --resume`) dies fast when
    /// it is stale/deleted — fall back to a fresh session instead of leaving
    /// a dead pane.
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
            fresh.session_id = None;
            if let Ok(_session) = daemon.spawn_agent_session(&fresh, &worktree, cols, rows) {
                let mut broadcast_agent = fresh;
                broadcast_agent.alive = true;
                daemon.broadcast(ServerEvent::EntityUpserted {
                    entity: Entity::Agent(broadcast_agent),
                });
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
        // `-l` makes it a login shell, matching Terminal.app: zsh then sources
        // /etc/zprofile (path_helper), ~/.zprofile, and ~/.zshrc.
        let spec = SpawnSpec {
            program: user_shell(),
            args: vec!["-l".into()],
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
        self.sessions
            .lock()
            .unwrap()
            .insert(session.sref.clone(), session.clone());
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
                            daemon.apply_hook_event(
                                id,
                                HookEvent::SessionEnded { exit_code },
                                None,
                            );
                        }
                        let upsert = match &sref {
                            SessionRef::Agent(id) => daemon.agent_entity(id).map(Entity::Agent),
                            SessionRef::Terminal(id) => {
                                daemon.terminal_entity(id).map(Entity::Terminal)
                            }
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

/// Program + args for an agent PTY. An override (tests) is used verbatim —
/// no resume args. Otherwise the kind picks the CLI and its resume shape:
/// `claude --resume <sid>` and `cursor-agent --resume <sid>` (flag) vs
/// `codex resume <sid>` (subcommand, so resume args must lead). Codex and
/// cursor always get their skip-permissions flag (`--yolo` / `--force`),
/// appended after the resume args — same convention as Mission Control.
fn agent_spawn_command(
    kind: AgentKind,
    session_id: Option<&str>,
    cmd_override: Option<&str>,
) -> (String, Vec<String>, bool) {
    if let Some(cmd) = cmd_override {
        let mut parts = cmd.split_whitespace().map(String::from).collect::<Vec<_>>();
        if parts.is_empty() {
            parts.push(kind.cli_program().into());
        }
        let program = parts.remove(0);
        return (program, parts, false);
    }
    let program = kind.cli_program().to_string();
    let (mut args, resumed) = match (kind, session_id) {
        (AgentKind::Claude, Some(sid)) => (vec!["--resume".to_string(), sid.to_string()], true),
        (AgentKind::Codex, Some(sid)) => (vec!["resume".to_string(), sid.to_string()], true),
        (AgentKind::Cursor, Some(sid)) => (vec!["--resume".to_string(), sid.to_string()], true),
        (_, None) => (Vec::new(), false),
    };
    match kind {
        AgentKind::Codex => args.push("--yolo".to_string()),
        AgentKind::Cursor => args.push("--force".to_string()),
        AgentKind::Claude => {}
    }
    (program, args, resumed)
}

/// Canonicalize for path containment tests, falling back to the raw path
/// when it doesn't resolve (deleted checkout, not-yet-created dir). macOS
/// symlinks (`/tmp` → `/private/tmp`) otherwise break `starts_with`.
fn canonical_or_raw(path: &Path) -> std::path::PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

fn user_shell() -> String {
    std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".into())
}

/// Wrap `program args…` in a login + interactive shell (`$SHELL -l -i -c
/// 'exec …'`) so the child gets the user's real environment — ~/.zprofile
/// and ~/.zshrc on zsh — rather than the daemon's. `exec` keeps the child
/// as the PTY's direct process (exit codes and signals pass through).
fn login_shell_wrap(shell: &str, program: &str, args: &[String]) -> (String, Vec<String>) {
    let mut cmdline = String::from("exec");
    for part in std::iter::once(program).chain(args.iter().map(String::as_str)) {
        cmdline.push_str(" '");
        cmdline.push_str(&part.replace('\'', "'\\''"));
        cmdline.push('\'');
    }
    (shell.to_string(), vec!["-l".into(), "-i".into(), "-c".into(), cmdline])
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

    #[test]
    fn spawn_command_per_kind_resume_shapes() {
        // Fresh sessions: bare CLI.
        assert_eq!(
            agent_spawn_command(AgentKind::Claude, None, None),
            ("claude".into(), vec![], false)
        );
        // Codex/cursor always run in skip-permissions mode.
        assert_eq!(
            agent_spawn_command(AgentKind::Codex, None, None),
            ("codex".into(), vec!["--yolo".to_string()], false)
        );
        // Cursor's agent CLI is `cursor-agent`, not `cursor` (the editor).
        assert_eq!(
            agent_spawn_command(AgentKind::Cursor, None, None),
            ("cursor-agent".into(), vec!["--force".to_string()], false)
        );
        // Claude resumes with a flag; codex with a subcommand (order matters).
        assert_eq!(
            agent_spawn_command(AgentKind::Claude, Some("sid-1"), None),
            (
                "claude".into(),
                vec!["--resume".to_string(), "sid-1".to_string()],
                true
            )
        );
        // Skip-permissions flags trail the resume args.
        assert_eq!(
            agent_spawn_command(AgentKind::Codex, Some("sid-2"), None),
            (
                "codex".into(),
                vec![
                    "resume".to_string(),
                    "sid-2".to_string(),
                    "--yolo".to_string()
                ],
                true
            )
        );
        assert_eq!(
            agent_spawn_command(AgentKind::Cursor, Some("sid-3"), None),
            (
                "cursor-agent".into(),
                vec![
                    "--resume".to_string(),
                    "sid-3".to_string(),
                    "--force".to_string()
                ],
                true
            )
        );
        // Override wins for both kinds and never gets resume args.
        assert_eq!(
            agent_spawn_command(AgentKind::Claude, Some("sid"), Some("/bin/sh -i")),
            ("/bin/sh".into(), vec!["-i".to_string()], false)
        );
        assert_eq!(
            agent_spawn_command(AgentKind::Codex, Some("sid"), Some("/bin/sh")),
            ("/bin/sh".into(), vec![], false)
        );
    }

    #[test]
    fn login_shell_wrap_quotes_and_execs() {
        let (program, args) = login_shell_wrap(
            "/bin/zsh",
            "claude",
            &["--resume".to_string(), "sid-1".to_string()],
        );
        assert_eq!(program, "/bin/zsh");
        assert_eq!(
            args,
            vec!["-l", "-i", "-c", "exec 'claude' '--resume' 'sid-1'"]
        );
        // Single quotes in an arg survive the wrapping.
        let (_, args) = login_shell_wrap("/bin/zsh", "echo", &["it's".to_string()]);
        assert_eq!(args[3], r"exec 'echo' 'it'\''s'");
    }

    fn test_daemon() -> Arc<Daemon> {
        let store = Store::open_in_memory().unwrap();
        Daemon::new(
            store,
            HookEnv {
                port: 0,
                token: String::new(),
            },
        )
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
                    divider_before: false,
                    divider_before_label: None,
                })
                .unwrap();
        }
    }

    /// (name, divider_after, divider_label) in display order.
    fn layout(daemon: &Daemon) -> Vec<(String, bool, Option<String>)> {
        let (projects, _, _, _) = daemon.store.load_tree().unwrap();
        projects
            .into_iter()
            .map(|p| (p.name, p.divider_after, p.divider_label))
            .collect()
    }

    /// The leading divider: `Some(label)` when the list has one. Also
    /// asserts the invariant that only the first project ever carries it.
    fn leading(daemon: &Daemon) -> Option<Option<String>> {
        let (projects, _, _, _) = daemon.store.load_tree().unwrap();
        for p in projects.iter().skip(1) {
            assert!(
                !p.divider_before,
                "leading divider drifted off the first project"
            );
        }
        let first = projects.first()?;
        first
            .divider_before
            .then(|| first.divider_before_label.clone())
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
        assert_eq!(
            projects.iter().map(|p| p.sort_order).collect::<Vec<_>>(),
            [0, 1, 2, 3]
        );

        // Edge moves clamp to no-ops.
        daemon.move_project(&ProjectId("a".into()), -1).unwrap();
        daemon.move_project(&ProjectId("c".into()), 5).unwrap();
        assert_eq!(names(&daemon), ["a", "d", "b", "c"]);
    }

    #[test]
    fn moves_step_one_visual_row_so_projects_park_beside_dividers() {
        let daemon = test_daemon();
        seed_projects(&daemon, &["a", "b", "c", "d"]);
        // Groups: [a b] [c d], labeled "work".
        daemon
            .set_project_divider(&ProjectId("b".into()), false, true, Some("work".into()))
            .unwrap();

        // First press: c crosses the divider into the first group without
        // swapping past b — the divider (and its label) keeps marking the
        // same gap, now below c.
        daemon.move_project(&ProjectId("c".into()), -1).unwrap();
        assert_eq!(
            layout(&daemon),
            [
                ("a".to_string(), false, None),
                ("b".to_string(), false, None),
                ("c".to_string(), true, Some("work".to_string())),
                ("d".to_string(), false, None),
            ]
        );

        // Second press: c swaps with b like any project-to-project move.
        daemon.move_project(&ProjectId("c".into()), -1).unwrap();
        assert_eq!(
            layout(&daemon),
            [
                ("a".to_string(), false, None),
                ("c".to_string(), false, None),
                ("b".to_string(), true, Some("work".to_string())),
                ("d".to_string(), false, None),
            ]
        );

        // Moving down retraces the same two steps back to the start.
        daemon.move_project(&ProjectId("c".into()), 1).unwrap();
        daemon.move_project(&ProjectId("c".into()), 1).unwrap();
        assert_eq!(
            layout(&daemon),
            [
                ("a".to_string(), false, None),
                ("b".to_string(), true, Some("work".to_string())),
                ("c".to_string(), false, None),
                ("d".to_string(), false, None),
            ]
        );
    }

    #[test]
    fn top_project_crossing_its_divider_leaves_it_leading() {
        let daemon = test_daemon();
        seed_projects(&daemon, &["a", "b"]);
        daemon
            .set_project_divider(&ProjectId("a".into()), false, true, Some("work".into()))
            .unwrap();

        // a steps below its own divider, which stays put above the whole
        // list — the leading divider, label intact.
        daemon.move_project(&ProjectId("a".into()), 1).unwrap();
        assert_eq!(names(&daemon), ["a", "b"]);
        assert_eq!(leading(&daemon), Some(Some("work".to_string())));
        assert!(layout(&daemon).iter().all(|(_, divider, _)| !divider));

        // The next press swaps a past b like any project move; the leading
        // divider stays on top, now owned by b.
        daemon.move_project(&ProjectId("a".into()), 1).unwrap();
        assert_eq!(names(&daemon), ["b", "a"]);
        assert_eq!(leading(&daemon), Some(Some("work".to_string())));

        // Moving a back up retraces both steps.
        daemon.move_project(&ProjectId("a".into()), -1).unwrap();
        assert_eq!(names(&daemon), ["a", "b"]);
        assert_eq!(leading(&daemon), Some(Some("work".to_string())));
        daemon.move_project(&ProjectId("a".into()), -1).unwrap();
        assert_eq!(leading(&daemon), None);
        assert_eq!(
            layout(&daemon)[0],
            ("a".to_string(), true, Some("work".to_string()))
        );
    }

    #[test]
    fn move_divider_hops_above_the_first_project_and_back() {
        let daemon = test_daemon();
        seed_projects(&daemon, &["a", "b"]);
        daemon
            .set_project_divider(&ProjectId("a".into()), false, true, Some("work".into()))
            .unwrap();

        // Up from under a: the divider crosses onto the top slot.
        daemon.move_divider(&ProjectId("a".into()), false, -1).unwrap();
        assert_eq!(leading(&daemon), Some(Some("work".to_string())));
        assert!(layout(&daemon).iter().all(|(_, divider, _)| !divider));

        // Up again clamps — it is already above everything.
        daemon.move_divider(&ProjectId("a".into()), true, -1).unwrap();
        assert_eq!(leading(&daemon), Some(Some("work".to_string())));

        // Down: back under a.
        daemon.move_divider(&ProjectId("a".into()), true, 1).unwrap();
        assert_eq!(leading(&daemon), None);
        assert_eq!(
            layout(&daemon)[0],
            ("a".to_string(), true, Some("work".to_string()))
        );
    }

    #[test]
    fn leading_divider_blocks_on_a_stacked_gap() {
        let daemon = test_daemon();
        seed_projects(&daemon, &["a", "b"]);
        daemon
            .set_project_divider(&ProjectId("a".into()), true, true, Some("top".into()))
            .unwrap();
        daemon
            .set_project_divider(&ProjectId("a".into()), false, true, Some("mid".into()))
            .unwrap();

        // Neither divider can move onto the other's gap.
        daemon.move_divider(&ProjectId("a".into()), true, 1).unwrap();
        daemon.move_divider(&ProjectId("a".into()), false, -1).unwrap();
        assert_eq!(leading(&daemon), Some(Some("top".to_string())));
        assert_eq!(
            layout(&daemon)[0],
            ("a".to_string(), true, Some("mid".to_string()))
        );

        // And the project pinched between them can't move either — that
        // would stack the dividers into one gap.
        daemon.move_project(&ProjectId("a".into()), 1).unwrap();
        daemon.move_project(&ProjectId("a".into()), -1).unwrap();
        assert_eq!(names(&daemon), ["a", "b"]);
        assert_eq!(leading(&daemon), Some(Some("top".to_string())));
    }

    #[test]
    fn removing_the_top_project_hands_the_leading_divider_down() {
        let daemon = test_daemon();
        seed_projects(&daemon, &["a", "b"]);
        daemon
            .set_project_divider(&ProjectId("a".into()), true, true, Some("work".into()))
            .unwrap();

        daemon.remove_project(&ProjectId("a".into())).unwrap();
        assert_eq!(names(&daemon), ["b"]);
        assert_eq!(leading(&daemon), Some(Some("work".to_string())));
    }

    #[test]
    fn move_divider_steps_between_projects_and_keeps_its_label() {
        let daemon = test_daemon();
        seed_projects(&daemon, &["a", "b", "c"]);
        daemon
            .set_project_divider(&ProjectId("a".into()), false, true, Some("work".into()))
            .unwrap();

        // Down: the divider hops from under a to under b, label intact.
        daemon.move_divider(&ProjectId("a".into()), false, 1).unwrap();
        assert_eq!(
            layout(&daemon),
            [
                ("a".to_string(), false, None),
                ("b".to_string(), true, Some("work".to_string())),
                ("c".to_string(), false, None),
            ]
        );

        // Back up under a (the top hop has its own test).
        daemon.move_divider(&ProjectId("b".into()), false, -1).unwrap();
        assert_eq!(
            layout(&daemon)[0],
            ("a".to_string(), true, Some("work".to_string()))
        );

        // Down past the last project clamps.
        daemon.move_divider(&ProjectId("a".into()), false, 1).unwrap();
        daemon.move_divider(&ProjectId("b".into()), false, 1).unwrap();
        daemon.move_divider(&ProjectId("c".into()), false, 1).unwrap();
        assert_eq!(
            layout(&daemon)[2],
            ("c".to_string(), true, Some("work".to_string()))
        );
    }

    #[test]
    fn move_divider_blocks_on_a_neighboring_divider() {
        let daemon = test_daemon();
        seed_projects(&daemon, &["a", "b"]);
        daemon
            .set_project_divider(&ProjectId("a".into()), false, true, Some("one".into()))
            .unwrap();
        daemon
            .set_project_divider(&ProjectId("b".into()), false, true, Some("two".into()))
            .unwrap();

        // Neither divider can move onto the other's gap.
        daemon.move_divider(&ProjectId("a".into()), false, 1).unwrap();
        daemon.move_divider(&ProjectId("b".into()), false, -1).unwrap();
        assert_eq!(
            layout(&daemon),
            [
                ("a".to_string(), true, Some("one".to_string())),
                ("b".to_string(), true, Some("two".to_string())),
            ]
        );
    }

    #[test]
    fn relabeling_keeps_divider_and_removal_drops_label() {
        let daemon = test_daemon();
        seed_projects(&daemon, &["a", "b"]);
        daemon
            .set_project_divider(&ProjectId("a".into()), false, true, Some("work".into()))
            .unwrap();

        daemon
            .set_project_divider(&ProjectId("a".into()), false, true, Some("play".into()))
            .unwrap();
        assert_eq!(layout(&daemon)[0].2.as_deref(), Some("play"));
        daemon
            .set_project_divider(&ProjectId("a".into()), false, false, Some("ignored".into()))
            .unwrap();
        assert!(layout(&daemon)
            .iter()
            .all(|(_, divider, label)| !divider && label.is_none()));
    }

    fn seed_worktree(daemon: &Daemon, project: &str, id: &str, path: &str, is_main: bool) {
        daemon
            .store
            .insert_worktree(&Worktree {
                id: WorktreeId(id.into()),
                project_id: ProjectId(project.into()),
                path: path.into(),
                branch: id.into(),
                is_main,
                pinned: false,
                sort_order: 0,
            })
            .unwrap();
    }

    fn seed_agent(daemon: &Daemon, id: &str, worktree: &str, session_id: Option<&str>) {
        daemon
            .store
            .insert_agent(&Agent {
                id: AgentId(id.into()),
                worktree_id: WorktreeId(worktree.into()),
                name: id.into(),
                status: AgentStatus::Running,
                archived: false,
                pinned: false,
                kind: AgentKind::Claude,
                session_id: session_id.map(str::to_string),
                sort_order: 0,
                status_changed_at: 0,
                alive: false,
            })
            .unwrap();
    }

    fn agent_worktree(daemon: &Daemon, id: &str) -> String {
        daemon
            .store
            .get_agent(&AgentId(id.into()))
            .unwrap()
            .unwrap()
            .worktree_id
            .to_string()
    }

    #[test]
    fn move_agent_rehomes_row_and_broadcasts() {
        let daemon = test_daemon();
        seed_projects(&daemon, &["p"]);
        seed_worktree(&daemon, "p", "root", "/nebula-test/p", true);
        seed_worktree(&daemon, "p", "feat", "/nebula-test/p-feat", false);
        seed_agent(&daemon, "a1", "root", None);
        let mut rx = daemon.events.subscribe();

        daemon
            .move_agent(&AgentId("a1".into()), &WorktreeId("feat".into()))
            .unwrap();
        assert_eq!(agent_worktree(&daemon, "a1"), "feat");
        match rx.try_recv().unwrap() {
            ServerEvent::EntityUpserted {
                entity: Entity::Agent(a),
            } => assert_eq!(a.worktree_id.to_string(), "feat"),
            other => panic!("expected agent upsert, got {other:?}"),
        }

        // Moving to the worktree it already lives in is a silent no-op.
        daemon
            .move_agent(&AgentId("a1".into()), &WorktreeId("feat".into()))
            .unwrap();
        assert!(rx.try_recv().is_err(), "no broadcast for a no-op move");
    }

    #[test]
    fn move_agent_rejects_cross_project_targets() {
        let daemon = test_daemon();
        seed_projects(&daemon, &["p", "q"]);
        seed_worktree(&daemon, "p", "p-root", "/nebula-test/p", true);
        seed_worktree(&daemon, "q", "q-root", "/nebula-test/q", true);
        seed_agent(&daemon, "a1", "p-root", None);

        let err = daemon
            .move_agent(&AgentId("a1".into()), &WorktreeId("q-root".into()))
            .unwrap_err();
        assert!(err.to_string().contains("different project"));
        assert_eq!(agent_worktree(&daemon, "a1"), "p-root");
    }

    #[test]
    fn reparent_by_cwd_picks_deepest_matching_worktree() {
        let daemon = test_daemon();
        seed_projects(&daemon, &["p"]);
        // Nested layout: the linked checkout lives under the repo root, so
        // both paths are prefixes of a cwd inside it — deepest must win.
        seed_worktree(&daemon, "p", "root", "/nebula-test/p", true);
        seed_worktree(&daemon, "p", "feat", "/nebula-test/p/.wt/feat", false);
        seed_agent(&daemon, "a1", "root", None);

        // cwd inside the root checkout (but outside the nested worktree)
        // keeps the agent where it is.
        daemon.reparent_agent_by_cwd(&AgentId("a1".into()), "/nebula-test/p/src", None, false);
        assert_eq!(agent_worktree(&daemon, "a1"), "root");

        // cwd inside the nested worktree re-homes it there.
        daemon.reparent_agent_by_cwd(
            &AgentId("a1".into()),
            "/nebula-test/p/.wt/feat/src",
            None,
            false,
        );
        assert_eq!(agent_worktree(&daemon, "a1"), "feat");

        // cwd outside every worktree is ignored.
        daemon.reparent_agent_by_cwd(&AgentId("a1".into()), "/elsewhere", None, false);
        assert_eq!(agent_worktree(&daemon, "a1"), "feat");
    }

    #[test]
    fn reparent_by_cwd_ignores_foreign_sessions_unless_capturing() {
        let daemon = test_daemon();
        seed_projects(&daemon, &["p"]);
        seed_worktree(&daemon, "p", "root", "/nebula-test/p", true);
        seed_worktree(&daemon, "p", "feat", "/nebula-test/p-feat", false);
        seed_agent(&daemon, "a1", "root", Some("s1"));

        // A different session id on a non-capturing event (a nested claude
        // launched inside the agent's PTY) must not move the row.
        daemon.reparent_agent_by_cwd(
            &AgentId("a1".into()),
            "/nebula-test/p-feat",
            Some("s2"),
            false,
        );
        assert_eq!(agent_worktree(&daemon, "a1"), "root");

        // A capturing event (re)establishes ownership, so it may move it.
        daemon.reparent_agent_by_cwd(
            &AgentId("a1".into()),
            "/nebula-test/p-feat",
            Some("s2"),
            true,
        );
        assert_eq!(agent_worktree(&daemon, "a1"), "feat");
    }

    #[test]
    fn prewarm_pool_buffers_hooks_and_drops_dead_entries() {
        let daemon = test_daemon();
        let key = (WorktreeId("w1".into()), AgentKind::Claude);
        daemon.prewarmed.lock().unwrap().insert(
            key.clone(),
            PrewarmEntry {
                agent_id: AgentId("warm-1".into()),
                spawned_at: Instant::now(),
                buffered_hooks: Vec::new(),
            },
        );

        // Hooks for the warm (row-less) id are buffered on the entry, not
        // dropped; hooks for unrelated unknown ids still vanish quietly.
        daemon.apply_hook_event(
            &AgentId("warm-1".into()),
            HookEvent::SessionStart { source: None },
            Some("sid-9".into()),
        );
        daemon.apply_hook_event(&AgentId("stranger".into()), HookEvent::Stop, None);
        {
            let pool = daemon.prewarmed.lock().unwrap();
            let entry = pool.get(&key).unwrap();
            assert_eq!(entry.buffered_hooks.len(), 1);
            assert_eq!(
                entry.buffered_hooks[0],
                (
                    HookEvent::SessionStart { source: None },
                    Some("sid-9".to_string())
                )
            );
        }

        // The buffer is bounded: overflow drops the oldest.
        for i in 0..(PREWARM_HOOK_BUFFER_CAP + 5) {
            daemon.apply_hook_event(
                &AgentId("warm-1".into()),
                HookEvent::Notification {
                    notification_type: Some(format!("n{i}")),
                },
                None,
            );
        }
        assert_eq!(
            daemon
                .prewarmed
                .lock()
                .unwrap()
                .get(&key)
                .unwrap()
                .buffered_hooks
                .len(),
            PREWARM_HOOK_BUFFER_CAP
        );

        // No live PTY backs the entry, so take() refuses it (create falls
        // back to a cold spawn) and reap clears it out.
        assert!(daemon
            .take_prewarmed(&WorktreeId("w1".into()), AgentKind::Claude)
            .is_none());
        assert!(daemon.prewarmed.lock().unwrap().is_empty());

        daemon.prewarmed.lock().unwrap().insert(
            key.clone(),
            PrewarmEntry {
                agent_id: AgentId("warm-2".into()),
                spawned_at: Instant::now(),
                buffered_hooks: Vec::new(),
            },
        );
        daemon.reap_prewarmed();
        assert!(daemon.prewarmed.lock().unwrap().is_empty());
    }

    #[test]
    fn kill_prewarmed_in_scopes_to_worktrees() {
        let daemon = test_daemon();
        for (wt, id) in [("w1", "a"), ("w2", "b")] {
            daemon.prewarmed.lock().unwrap().insert(
                (WorktreeId(wt.into()), AgentKind::Codex),
                PrewarmEntry {
                    agent_id: AgentId(id.into()),
                    spawned_at: Instant::now(),
                    buffered_hooks: Vec::new(),
                },
            );
        }
        daemon.kill_prewarmed_in(&[WorktreeId("w1".into())]);
        let pool = daemon.prewarmed.lock().unwrap();
        assert_eq!(pool.len(), 1);
        assert!(pool.contains_key(&(WorktreeId("w2".into()), AgentKind::Codex)));
    }

    #[test]
    fn reparent_by_cwd_skips_archived_agents() {
        let daemon = test_daemon();
        seed_projects(&daemon, &["p"]);
        seed_worktree(&daemon, "p", "root", "/nebula-test/p", true);
        seed_worktree(&daemon, "p", "feat", "/nebula-test/p-feat", false);
        seed_agent(&daemon, "a1", "root", None);
        daemon
            .store
            .set_agent_archived(&AgentId("a1".into()), true)
            .unwrap();

        daemon.reparent_agent_by_cwd(&AgentId("a1".into()), "/nebula-test/p-feat", None, false);
        assert_eq!(agent_worktree(&daemon, "a1"), "root");
    }
}
