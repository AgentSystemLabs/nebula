//! The daemon's world: persisted entity tree + live PTY sessions, and the
//! operations the IPC surface exposes over them.

use crate::git;
use crate::hooks::{self, HookEnv};
use crate::pty::{PtyEvent, PtySession, SpawnSpec, DEFAULT_COLS, DEFAULT_ROWS};
use crate::status::{AgentStatusMachine, Effect, HookEvent};
use crate::store::Store;
use anyhow::{bail, Context, Result};
use nebula_core::env;
use nebula_core::{
    Agent, AgentId, AgentKind, AgentStatus, EnterOutcome, Entity, EntityId, Link, LinkId,
    PrewarmInfo, Project, ProjectId, ServerEvent, SessionRef, TerminalId, TerminalTab, Workspace,
    WorkspaceId, Worktree, WorktreeId, MAX_CLOUD_PROMPT_BYTES,
};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::broadcast;

/// A warm agent CLI older than this is reaped — it holds memory and its
/// conversation context grows stale.
const PREWARM_MAX_AGE: Duration = Duration::from_secs(15 * 60);
/// A live same-spec warm CLI older than this is recycled (killed and
/// re-booted fresh) when its slot is re-requested, instead of being kept.
/// Clients keep-warm the selected worktree on a cadence shorter than
/// `PREWARM_MAX_AGE - PREWARM_RECYCLE_AGE`, so a slot they still care about
/// is always refreshed before the reaper can empty it.
const PREWARM_RECYCLE_AGE: Duration = Duration::from_secs(10 * 60);
/// Gap between the boots of a worktree prewarm sweep. A worktree with five
/// agents must not fork five agent CLIs at once: they would all contend for
/// the CPU with the one session the user is actually waiting to see, which
/// is the whole reason the sweep exists. Nothing is watching these, so
/// warming them slowly costs the user nothing.
const PREWARM_STAGGER: Duration = Duration::from_millis(1500);
/// Hook events buffered on a warm session before its row exists (oldest
/// dropped beyond this).
const PREWARM_HOOK_BUFFER_CAP: usize = 64;
/// How often a Cloud mirror re-teleports to pick up the session's newer
/// turns. `claude --teleport` re-fetches the transcript and re-checks-out
/// the branch each time, so this trades freshness against a git checkout
/// and a CLI boot per tick.
const CLOUD_MIRROR_REFRESH: Duration = Duration::from_secs(45);
/// Floor for the `NEBULA_CLOUD_MIRROR_SECS` override. A teleport is a git
/// checkout plus a CLI boot; below this the row would spend its life
/// respawning.
const CLOUD_MIRROR_MIN: Duration = Duration::from_secs(2);
/// `$SHELL -l -i -c <cmd>`: a login *and* interactive shell, so zsh sources
/// ~/.zprofile and ~/.zshrc both and the child sees the PATH the user's
/// terminal has. The CLI probe and the spawn wrapper share it so they can
/// never disagree about what "on the user's PATH" means.
const LOGIN_SHELL_ARGS: [&str; 3] = ["-l", "-i", "-c"];
/// Cap on one CLI probe. A heavy rc file costs ~1s; a hung one must not
/// stall a create forever, so on timeout the CLI is assumed present and
/// the spawn itself gets to report.
const CLI_PROBE_TIMEOUT: Duration = Duration::from_secs(5);

/// Mirror cadence, `NEBULA_CLOUD_MIRROR_SECS` overriding the default (and
/// `0` disabling the follow entirely — the pane is then only refreshed by
/// hand, from the row's menu). Read once: this is a daemon-wide knob, not
/// something to re-probe per tick.
fn cloud_mirror_refresh() -> Option<Duration> {
    static CADENCE: std::sync::OnceLock<Option<Duration>> = std::sync::OnceLock::new();
    *CADENCE.get_or_init(|| {
        match std::env::var(env::CLOUD_MIRROR_SECS)
            .ok()
            .and_then(|v| v.trim().parse::<u64>().ok())
        {
            Some(0) => None,
            Some(secs) => Some(Duration::from_secs(secs).max(CLOUD_MIRROR_MIN)),
            None => Some(CLOUD_MIRROR_REFRESH),
        }
    })
}

pub(crate) struct CreateAgentSpec {
    pub worktree: WorktreeId,
    pub name: String,
    pub kind: AgentKind,
    pub model: Option<String>,
    pub effort: Option<String>,
    pub auto_title: bool,
    pub cloud_prompt: Option<String>,
    /// The CLI's positional first prompt (an AGENT PRESET launch). Request-only.
    pub starting_prompt: Option<String>,
    pub pr_url: Option<String>,
}

/// A pre-spawned agent CLI waiting to be adopted by the next CreateAgent for
/// the same (worktree, kind). The PTY lives in the normal sessions map under
/// a pre-generated agent id, so its NEBULA_AGENT_ID env is already the id
/// the adopted row will use. Hook events that arrive before the row exists
/// (SessionStart carries the resume session id) are buffered here and
/// replayed at adoption.
struct PrewarmEntry {
    agent_id: AgentId,
    spawned_at: Instant,
    /// Model/effort the warm CLI booted with; a CreateAgent asking for a
    /// different spec can't adopt it (the CLI is already running the wrong
    /// model), so the entry is discarded instead.
    model: Option<String>,
    effort: Option<String>,
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
    /// Shared with the hook HTTP server, which reads agent rows to decide
    /// auto-title injection.
    pub store: Arc<Store>,
    /// Entity/status deltas fanned out to every subscribed client.
    pub events: broadcast::Sender<ServerEvent>,
    /// Every session the registry installs — a spawn, a respawn, a prewarm
    /// adoption — by ref, the moment it is in `sessions`. A client's forward
    /// task (`attach.rs`) listens so a kill-and-respawn behind an attached
    /// pane rebinds it to the new PTY instead of leaving it on a dead one.
    pub session_installs: broadcast::Sender<SessionRef>,
    pub shutdown: tokio_util::sync::CancellationToken,
    /// Serializes worktree create/delete with the background auto-sync so
    /// a checkout is never adopted twice while its row is mid-insert.
    worktree_ops: tokio::sync::Mutex<()>,
    /// Warm agent CLIs awaiting adoption, at most one per (worktree, kind).
    prewarmed: Mutex<HashMap<(WorktreeId, AgentKind), PrewarmEntry>>,
    /// Cached `command -v` results per CLI so a missing binary doesn't get
    /// re-probed (login shell spawn) on every prewarm request.
    cli_probes: Mutex<HashMap<AgentKind, (bool, Instant)>>,
    /// How many client connections are attached per session — a session
    /// with attachments (and its whole worktree) is "in view" and exempt
    /// from idle reaping.
    attach_counts: Mutex<HashMap<SessionRef, usize>>,
    /// When each live session was last "looked at": spawned, prewarmed,
    /// attached, or covered by the in-view sweep refresh. The idle reaper
    /// kills sessions whose stamp ages past `session_idle_timeout`.
    session_interest: Mutex<HashMap<SessionRef, Instant>>,
    /// Last hook-reported cwd per agent, recorded only for payloads that
    /// passed the foreign-session gate. An agent that walks into a checkout
    /// nebula hasn't adopted yet leaves its cwd here, so the worktree sync
    /// can finish the re-home once the row exists.
    last_cwd: Mutex<HashMap<AgentId, PathBuf>>,
    /// Agents that ran `nebula worktree` and are waiting for their turn to
    /// end: the row already sits under the target worktree while the PTY
    /// still runs in the old checkout. Drained by `complete_pending_move`
    /// on the turn-end hook (kill + respawn resumed in the target), cleared
    /// by any other spawn of the agent, and consulted by the cwd reparent so
    /// the old checkout's cwd can't drag the row back in the meantime.
    pending_moves: Mutex<HashMap<AgentId, Worktree>>,
    /// Set the first time `claude --cloud <id>` refuses to attach ("not
    /// enabled for your account"). Live attach is a server-side rollout, so
    /// once it has been refused every later re-entry teleports straight
    /// away rather than flashing the same error again. Deliberately not
    /// persisted: a fresh daemon re-probes, so the day the rollout lands
    /// nebula picks it up without anyone clearing a flag.
    cloud_attach_gated: AtomicBool,
    /// Cloud rows currently being mirrored (periodic re-teleport). Keyed by
    /// agent so a second follow request replaces rather than doubles up.
    cloud_mirrors: Mutex<HashMap<AgentId, Arc<tokio_util::sync::CancellationToken>>>,
    /// Serializes the check-and-spawn inside [`Daemon::ensure_session`].
    /// Attach (the request loop) and the worktree prewarm sweep (its own
    /// task) can both reach for the same dead session; without this they
    /// would both miss the registry and fork two CLIs, orphaning one.
    spawn_gate: Mutex<()>,
    /// The worktree prewarm sweep currently running, so a newer one can
    /// cancel it. Stepping through the Workspaces column fires a sweep per
    /// row, and only the row the cursor rests on is worth warming.
    prewarm_sweep: Mutex<Option<tokio::task::JoinHandle<()>>>,
}

impl Daemon {
    pub fn new(store: Arc<Store>, hook_env: HookEnv) -> Arc<Self> {
        let (events, _) = broadcast::channel(1024);
        let (session_installs, _) = broadcast::channel(64);
        Arc::new(Self {
            sessions: Mutex::new(HashMap::new()),
            status_machines: Mutex::new(HashMap::new()),
            hook_env,
            store,
            events,
            session_installs,
            shutdown: tokio_util::sync::CancellationToken::new(),
            worktree_ops: tokio::sync::Mutex::new(()),
            prewarmed: Mutex::new(HashMap::new()),
            cli_probes: Mutex::new(HashMap::new()),
            attach_counts: Mutex::new(HashMap::new()),
            session_interest: Mutex::new(HashMap::new()),
            last_cwd: Mutex::new(HashMap::new()),
            pending_moves: Mutex::new(HashMap::new()),
            cloud_attach_gated: AtomicBool::new(false),
            cloud_mirrors: Mutex::new(HashMap::new()),
            spawn_gate: Mutex::new(()),
            prewarm_sweep: Mutex::new(None),
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

    fn buffer_prewarm_hook(
        &self,
        agent_id: &AgentId,
        event: HookEvent,
        session_id: Option<String>,
    ) {
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
                    let (changed_at, unseen) = match self.store.set_agent_status(agent_id, status) {
                        Ok(stamped) => stamped,
                        Err(e) => {
                            tracing::warn!(error = %e, "persist status failed");
                            (epoch_ms(), false)
                        }
                    };
                    self.broadcast(ServerEvent::StatusChanged {
                        agent: agent_id.clone(),
                        status,
                        changed_at,
                        unseen,
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

    /// (session, child pid, prewarm-pool home) for every live PTY — the
    /// metrics reading's input. A pool spare has no agent row, so the only
    /// way a client can name or place it is the home reported here.
    pub fn session_pids(&self) -> Vec<(SessionRef, u32, Option<PrewarmInfo>)> {
        // Snapshot the pool first and drop its lock: `prewarm_agent` holds
        // the pool lock while it asks the sessions map, so the two are
        // never held together here in the other order.
        let prewarmed: HashMap<AgentId, PrewarmInfo> = self
            .prewarmed
            .lock()
            .unwrap()
            .iter()
            .map(|((worktree, kind), e)| {
                (
                    e.agent_id.clone(),
                    PrewarmInfo {
                        worktree: worktree.clone(),
                        kind: *kind,
                        model: e.model.clone(),
                    },
                )
            })
            .collect();
        self.sessions
            .lock()
            .unwrap()
            .iter()
            .filter_map(|(sref, s)| {
                let pid = s.child_pid?;
                let prewarm = match sref {
                    SessionRef::Agent(id) => prewarmed.get(id).cloned(),
                    SessionRef::Terminal(_) => None,
                };
                Some((sref.clone(), pid, prewarm))
            })
            .collect()
    }

    pub fn remove_session(&self, sref: &SessionRef) -> Option<Arc<PtySession>> {
        self.session_interest.lock().unwrap().remove(sref);
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

    // ---- attach tracking & idle reaping ----

    /// A client attached to `sref` (the server dedupes re-attaches per
    /// connection). While any attachment exists, the session — and its
    /// whole worktree — counts as "in view".
    pub fn note_attached(&self, sref: &SessionRef) {
        *self
            .attach_counts
            .lock()
            .unwrap()
            .entry(sref.clone())
            .or_insert(0) += 1;
        self.touch_session(sref);
    }

    /// A client detached from `sref` (or its connection dropped). Restamps
    /// the session so the idle clock starts at "stopped looking", not at
    /// spawn time.
    pub fn note_detached(&self, sref: &SessionRef) {
        let mut counts = self.attach_counts.lock().unwrap();
        if let Some(n) = counts.get_mut(sref) {
            *n = n.saturating_sub(1);
            if *n == 0 {
                counts.remove(sref);
            }
        }
        drop(counts);
        self.touch_session(sref);
    }

    /// Stamp `sref` as just-looked-at for the idle reaper.
    fn touch_session(&self, sref: &SessionRef) {
        self.session_interest
            .lock()
            .unwrap()
            .insert(sref.clone(), Instant::now());
    }

    /// Kill idle sessions in worktrees no client is looking at, per
    /// `session_idle_timeout` — this bounds what prewarming and
    /// walked-away-from sessions cost. "In view" = the worktree holding any
    /// attached session; in-view sessions get their stamps refreshed
    /// instead, so the full timeout starts only when the user leaves.
    /// Spared regardless of age: agents that are running or waiting on
    /// feedback, terminals with a command running, and prewarm-pool sessions
    /// (`reap_prewarmed` owns those). A reaped session revives on the next
    /// attach or prewarm; agents resume their conversation.
    pub fn reap_idle_sessions(self: &Arc<Self>) {
        let Some(timeout) = crate::config::Config::load().session_idle_timeout() else {
            return;
        };
        let sessions: Vec<(SessionRef, Arc<PtySession>)> = self
            .sessions
            .lock()
            .unwrap()
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        let attached: std::collections::HashSet<SessionRef> =
            self.attach_counts.lock().unwrap().keys().cloned().collect();
        let viewed_worktrees: std::collections::HashSet<WorktreeId> = attached
            .iter()
            .filter_map(|sref| self.session_worktree(sref))
            .collect();
        let now = Instant::now();
        for (sref, session) in sessions {
            // No store row = prewarm-pool session (or deleted mid-sweep).
            let Some(worktree_id) = self.session_worktree(&sref) else {
                continue;
            };
            if attached.contains(&sref) || viewed_worktrees.contains(&worktree_id) {
                self.touch_session(&sref);
                continue;
            }
            let age = {
                let mut interest = self.session_interest.lock().unwrap();
                // A missing stamp (session predating the map) starts aging now.
                now.duration_since(*interest.entry(sref.clone()).or_insert(now))
            };
            if age < timeout {
                continue;
            }
            let spared = match &sref {
                SessionRef::Agent(id) => match self.store.get_agent(id).ok().flatten() {
                    Some(agent) => matches!(
                        agent.status,
                        AgentStatus::Running | AgentStatus::NeedsFeedback
                    ),
                    // Row vanished mid-sweep: its delete kills the PTY anyway.
                    None => true,
                },
                SessionRef::Terminal(_) => shell_has_children(&session),
            };
            if spared {
                continue;
            }
            tracing::info!(session = ?sref, idle_secs = age.as_secs(), "reaping idle session");
            self.kill_session(&sref);
            let upsert = match &sref {
                SessionRef::Agent(id) => self.agent_entity(id).map(Entity::Agent),
                SessionRef::Terminal(id) => self.terminal_entity(id).map(Entity::Terminal),
            };
            if let Ok(entity) = upsert {
                self.broadcast(ServerEvent::EntityUpserted { entity });
            }
        }
    }

    /// The worktree a session's row lives under; None when the row is gone
    /// or never existed (prewarm pool).
    fn session_worktree(&self, sref: &SessionRef) -> Option<WorktreeId> {
        match sref {
            SessionRef::Agent(id) => self
                .store
                .get_agent(id)
                .ok()
                .flatten()
                .map(|a| a.worktree_id),
            SessionRef::Terminal(id) => self
                .store
                .get_terminal(id)
                .ok()
                .flatten()
                .map(|t| t.worktree_id),
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
            workspaces: self.store.load_workspaces()?,
            active_workspace: self.store.active_workspace_id()?,
            projects,
            worktrees,
            agents,
            terminals,
            links: self.store.load_links()?,
            pr_seen: self.store.load_pr_seen()?,
            ui_state: self.store.load_ui_state()?,
        })
    }

    fn agent_entity(&self, id: &AgentId) -> Result<Agent> {
        let mut agent = self.store.get_agent(id)?.context("agent not found")?;
        agent.alive = self.is_alive(&SessionRef::Agent(id.clone()));
        agent.cloud_mirroring = self.cloud_mirror_active(id);
        Ok(agent)
    }

    /// Push the agent's current row — liveness and mirror flags included —
    /// to every subscriber. The tail of every mutation that changes how
    /// the row renders; fails only when the row is gone.
    fn broadcast_agent(&self, id: &AgentId) -> Result<()> {
        let agent = self.agent_entity(id)?;
        self.broadcast(ServerEvent::EntityUpserted {
            entity: Entity::Agent(agent),
        });
        Ok(())
    }

    /// [`Self::broadcast_agent`] for the best-effort sites — background
    /// tasks and post-respawn refreshes — where a row deleted meanwhile is
    /// not an error: nothing to show, so nothing to say.
    fn try_broadcast_agent(&self, id: &AgentId) {
        let _ = self.broadcast_agent(id);
    }

    fn terminal_entity(&self, id: &TerminalId) -> Result<TerminalTab> {
        let mut term = self.store.get_terminal(id)?.context("terminal not found")?;
        term.alive = self.is_alive(&SessionRef::Terminal(id.clone()));
        Ok(term)
    }

    // ---- workspaces ----

    /// Validated, trimmed workspace name, checked for collisions (excluding
    /// `except` on renames).
    fn checked_workspace_name(&self, name: &str, except: Option<&WorkspaceId>) -> Result<String> {
        let name = name.trim();
        if name.is_empty() {
            bail!("workspace name is empty");
        }
        if let Some(existing) = self.store.workspace_by_name(name)? {
            if Some(&existing) != except {
                bail!("a workspace named '{name}' already exists");
            }
        }
        Ok(name.to_string())
    }

    /// Create a workspace. Does not open it — `workspace open` stays a
    /// separate, explicit step.
    pub fn add_workspace(self: &Arc<Self>, name: &str) -> Result<EntityId> {
        let name = self.checked_workspace_name(name, None)?;
        let workspace = Workspace {
            id: WorkspaceId::generate(),
            name,
        };
        self.store.insert_workspace(&workspace)?;
        self.broadcast(ServerEvent::EntityUpserted {
            entity: Entity::Workspace(workspace.clone()),
        });
        Ok(EntityId::Workspace(workspace.id))
    }

    pub fn rename_workspace(self: &Arc<Self>, id: &WorkspaceId, name: &str) -> Result<()> {
        let mut workspace = self
            .store
            .get_workspace(id)?
            .context("workspace not found")?;
        workspace.name = self.checked_workspace_name(name, Some(id))?;
        self.store.rename_workspace(id, &workspace.name)?;
        self.broadcast(ServerEvent::EntityUpserted {
            entity: Entity::Workspace(workspace),
        });
        Ok(())
    }

    /// Delete a workspace. Only empty ones go — its projects are the user's
    /// to move or remove first — and never the last one. Deleting the
    /// remembered default moves that default to a survivor; clients still
    /// scoped to it re-scope themselves off the EntityRemoved.
    pub fn remove_workspace(self: &Arc<Self>, id: &WorkspaceId) -> Result<()> {
        self.store
            .get_workspace(id)?
            .context("workspace not found")?;
        let projects = self.store.count_workspace_projects(id)?;
        if projects > 0 {
            bail!(
                "workspace still has {projects} project{} — remove them first",
                if projects == 1 { "" } else { "s" }
            );
        }
        if self.store.count_workspaces()? <= 1 {
            bail!("cannot delete the last workspace");
        }
        if self.store.active_workspace_id()? == *id {
            let fallback = self
                .store
                .load_workspaces()?
                .into_iter()
                .find(|w| &w.id != id)
                .context("no workspace left to open")?;
            self.store.set_active_workspace(&fallback.id)?;
        }
        self.store.delete_workspace(id)?;
        self.broadcast(ServerEvent::EntityRemoved {
            id: EntityId::Workspace(id.clone()),
        });
        Ok(())
    }

    /// Remember `id` as the workspace a fresh client opens into. Which
    /// workspace a *live* client is looking at is that client's own state —
    /// see `ClientRequest::OpenWorkspace` — so this deliberately notifies
    /// nobody: one instance switching must leave the others where they are.
    pub fn set_default_workspace(self: &Arc<Self>, id: &WorkspaceId) -> Result<()> {
        self.store
            .get_workspace(id)?
            .context("workspace not found")?;
        if self.store.active_workspace_id()? == *id {
            return Ok(()); // already the default
        }
        self.store.set_active_workspace(id)?;
        Ok(())
    }

    // ---- projects ----

    /// Register a repo as a project. `workspace` is the caller's own scope
    /// (a TUI that switched with OpenWorkspace); `None` — a one-shot
    /// `nebula add`, or a client still on whatever it booted into — means
    /// the remembered default. A scope naming a workspace that has since
    /// been deleted falls back the same way rather than failing the add.
    pub async fn add_project(
        self: &Arc<Self>,
        path: &Path,
        name: Option<String>,
        create_missing: bool,
        workspace: Option<WorkspaceId>,
    ) -> Result<EntityId> {
        if create_missing && !path.exists() {
            tokio::fs::create_dir_all(path)
                .await
                .with_context(|| format!("create {}", path.display()))?;
            if crate::config::Config::load().git_init_on_create {
                git::init(path).await?;
            }
        }
        // "not a git repository" is the right explanation only when git ran and
        // said no — if git itself is missing, that message blames the wrong
        // thing, so let git.rs's own diagnosis through untouched.
        let toplevel = git::repo_toplevel(path).await.map_err(|e| {
            if git::is_missing(&e) {
                e
            } else {
                e.context(format!("{} is not a git repository", path.display()))
            }
        })?;
        // `--show-toplevel` answers with the checkout it was run in, so inside a
        // linked worktree it names the worktree rather than the repo. A project
        // is the repo: root it at the main checkout, which `git worktree list`
        // always puts first. Adding from inside a worktree used to name the
        // project after that worktree and leave its ⌂ root row pointing at a
        // directory the project did not own.
        let entries = git::list_worktrees(&toplevel)
            .await
            .with_context(|| format!("list checkouts of {}", toplevel.display()))?;
        let repo_path = match entries.first() {
            Some(main) => main.path.clone(),
            // git listing no checkout at all for a path it just called a work
            // tree would leave the root unknowable; refuse rather than seed a
            // project with no rows, which is how a project loses its root row.
            None => bail!("git listed no checkout for {}", toplevel.display()),
        };
        // New projects land in the caller's own workspace; the same repo
        // may be added to any number of workspaces, just not twice to one.
        let workspace_id = match workspace {
            Some(id) if self.store.get_workspace(&id)?.is_some() => id,
            _ => self.store.active_workspace_id()?,
        };
        if self
            .store
            .project_in_workspace(&repo_path, &workspace_id)?
            .is_some()
        {
            bail!(
                "project already added to this workspace: {}",
                repo_path.display()
            );
        }
        let name = name.unwrap_or_else(|| Project::folder_name(&repo_path));
        let project = Project {
            id: ProjectId::generate(),
            name,
            workspace_id,
            repo_path: repo_path.clone(),
            sort_order: self.store.next_project_sort_order()?,
        };
        self.store.insert_project(&project)?;
        self.broadcast(ServerEvent::EntityUpserted {
            entity: Entity::Project(project.clone()),
        });

        // Main checkout is modeled as a worktree row; adopt pre-existing
        // worktrees too so `nebula` matches reality on day one. Root-ness is
        // the path test the reconcile uses, not insert order — the two agreeing
        // is what keeps `repo_path` and the ⌂ root row the same directory.
        for entry in entries {
            let worktree = Worktree {
                id: WorktreeId::generate(),
                project_id: project.id.clone(),
                is_main: entry.path == repo_path,
                path: entry.path.clone(),
                branch: entry.branch,
                sort_order: 0,
            };
            self.store.insert_worktree(&worktree)?;
            self.broadcast(ServerEvent::EntityUpserted {
                entity: Entity::Worktree(worktree),
            });
        }
        Ok(EntityId::Project(project.id))
    }

    /// Retitle a project's row. Cosmetic only — the checkout on disk is never
    /// renamed, and every worktree under the project keeps its own path. An
    /// empty name resets the row to the folder's name, which is the only way
    /// back once a project has been renamed.
    pub fn rename_project(self: &Arc<Self>, id: &ProjectId, name: &str) -> Result<()> {
        let mut project = self.store.get_project(id)?.context("project not found")?;
        let name = name.trim();
        project.name = if name.is_empty() {
            Project::folder_name(&project.repo_path)
        } else {
            name.to_string()
        };
        self.store.rename_project(id, &project.name)?;
        self.broadcast(ServerEvent::EntityUpserted {
            entity: Entity::Project(project),
        });
        Ok(())
    }

    pub fn remove_project(self: &Arc<Self>, id: &ProjectId) -> Result<()> {
        // Kill any live sessions under this project first.
        let (_, worktrees, agents, terminals) = self.store.load_tree()?;
        let wt_ids: Vec<WorktreeId> = worktrees
            .into_iter()
            .filter(|w| &w.project_id == id)
            .map(|w| w.id)
            .collect();
        self.kill_sessions_in(&wt_ids, &agents, &terminals);
        // Removing a project only forgets it in nebula — never touches disk.
        self.store.delete_project(id)?;
        self.broadcast(ServerEvent::EntityRemoved {
            id: EntityId::Project(id.clone()),
        });
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
        self.kill_sessions_in(std::slice::from_ref(id), &agents, &terminals);

        git::remove_worktree(&project.repo_path, &worktree.path, force).await?;
        self.store.delete_worktree(id)?;
        self.broadcast(ServerEvent::EntityRemoved {
            id: EntityId::Worktree(id.clone()),
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
        let adopted = {
            let _ops = self.worktree_ops.lock().await;
            self.reconcile_project_worktrees(project).await?
        };
        // Outside the ops lock: the replay only touches agent rows, and a
        // just-adopted checkout is exactly where a session that ran
        // `git worktree add` itself already lives.
        if adopted {
            self.reparent_agents_by_last_cwd(project);
        }
        Ok(())
    }

    /// The reconcile half of `sync_project_worktrees`. Returns whether any
    /// checkout was newly adopted.
    async fn reconcile_project_worktrees(self: &Arc<Self>, project: &Project) -> Result<bool> {
        let mut adopted = false;
        let entries = git::list_worktrees(&project.repo_path).await?;
        // git lists the main checkout first, and that — not the order rows
        // happened to be inserted in — is what makes a row the ⌂ root row.
        // Deriving it here every pass repairs a project whose rows were seeded
        // before the root was known, and keeps root-ness following the repo
        // when the checkouts underneath it change.
        let main_path = entries.first().map(|e| e.path.clone());
        let is_root = |path: &Path| main_path.as_deref() == Some(path);
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
                let root = is_root(&entry.path);
                if known.branch != entry.branch || known.is_main != root {
                    if known.branch != entry.branch {
                        self.store
                            .update_worktree_branch(&known.id, &entry.branch)?;
                    }
                    if known.is_main != root {
                        self.store.set_worktree_main(&known.id, root)?;
                    }
                    let mut updated = (*known).clone();
                    updated.branch = entry.branch.clone();
                    updated.is_main = root;
                    self.broadcast(ServerEvent::EntityUpserted {
                        entity: Entity::Worktree(updated),
                    });
                }
                continue;
            }
            let worktree = Worktree {
                id: WorktreeId::generate(),
                project_id: project.id.clone(),
                is_main: is_root(&entry.path),
                path: entry.path.clone(),
                branch: entry.branch.clone(),
                sort_order: 0,
            };
            self.store.insert_worktree(&worktree)?;
            adopted = true;
            self.broadcast(ServerEvent::EntityUpserted {
                entity: Entity::Worktree(worktree),
            });
        }
        for w in ours {
            // The main checkout is always somewhere in git's list, so a row
            // that isn't there is a linked checkout that went away — including
            // one still carrying an `is_main` from before root-ness was
            // derived, which no longer earns the row a reprieve.
            if entries.iter().any(|e| e.path == w.path) {
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
        Ok(adopted)
    }

    // ---- agents ----

    pub(crate) async fn create_agent(self: &Arc<Self>, spec: CreateAgentSpec) -> Result<EntityId> {
        let CreateAgentSpec {
            worktree: worktree_id,
            name,
            kind,
            model,
            effort,
            auto_title,
            cloud_prompt,
            starting_prompt,
            pr_url,
        } = spec;
        let cloud_prompt = match cloud_prompt {
            Some(_) if kind != AgentKind::Claude => {
                bail!("cloud launch is only supported for Claude")
            }
            Some(prompt) => {
                let prompt = validate_cloud_text(&prompt, "task")?;
                Some(prompt)
            }
            None => None,
        };
        let starting_prompt = match starting_prompt {
            Some(_) if cloud_prompt.is_some() => {
                bail!("a starting prompt is not supported for Claude Cloud")
            }
            Some(prompt) => Some(validate_starting_prompt(&prompt)?),
            None => None,
        };
        let pr_url = match pr_url {
            Some(_) if cloud_prompt.is_some() => {
                bail!("PR launch context is not supported for Claude Cloud")
            }
            Some(url) => Some(crate::pr_scope::validate_pr_url(&url)?),
            None => None,
        };
        let worktree = self
            .store
            .get_worktree(&worktree_id)?
            .context("worktree not found")?;
        // A warm session for this (worktree, kind) hands over its PTY and
        // its pre-generated id — the CLI booted while the user typed the
        // name, so the create feels instant. A starting prompt rides the
        // CLI's argv, and a spare already booted bare cannot be handed one.
        let adopted = (cloud_prompt.is_none() && pr_url.is_none() && starting_prompt.is_none())
            .then(|| self.take_prewarmed(&worktree_id, kind, model.as_deref(), effort.as_deref()))
            .flatten();
        // Only the cold path needs asking: an adopted warm session is proof
        // the CLI runs. Without this, a missing CLI still "succeeds" — the
        // login shell prints `command not found` into a PTY that dies at
        // once, leaving a dead row that looks identical to a fresh one.
        if adopted.is_none() && !self.cli_available_for_create(kind).await {
            bail!("{}", cli_missing_message(kind));
        }
        let agent = Agent {
            id: adopted
                .as_ref()
                .map(|e| e.agent_id.clone())
                .unwrap_or_else(AgentId::generate),
            worktree_id,
            name: if name.trim().is_empty() {
                "agent".into()
            } else {
                name.trim().to_string()
            },
            status: AgentStatus::Fresh,
            archived: false,
            archived_at: 0,
            unseen: false,
            kind,
            model,
            effort,
            session_id: None,
            cloud_session_id: None,
            sort_order: 0,
            status_changed_at: epoch_ms(),
            alive: false,
            cloud_mirroring: false,
        };
        self.store
            .insert_agent_with_launch_context(&agent, auto_title, pr_url.as_deref())?;
        if adopted.is_none() {
            // Cold path: boot the CLI right away.
            let spawned = self.spawn_agent_session_with(
                &agent,
                &worktree,
                DEFAULT_COLS,
                DEFAULT_ROWS,
                cloud_prompt.as_deref().map(CloudLaunch::Create),
                starting_prompt.as_deref(),
            );
            self.rollback_agent_on_spawn_error(&agent.id, spawned)?;
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

    fn rollback_agent_on_spawn_error<T>(&self, id: &AgentId, result: Result<T>) -> Result<T> {
        match result {
            Ok(value) => Ok(value),
            Err(spawn_error) => {
                if let Err(rollback_error) = self.store.delete_agent(id) {
                    return Err(spawn_error.context(format!(
                        "agent spawn failed and its database rollback also failed: {rollback_error:#}"
                    )));
                }
                Err(spawn_error)
            }
        }
    }

    // ---- prewarm pool ----

    /// Pre-spawn an agent CLI for (worktree, kind) so the next create adopts
    /// an already-booted session. Fail-soft by design: a disabled config,
    /// missing CLI, or spawn error just means the create stays cold.
    pub async fn prewarm_agent(
        self: &Arc<Self>,
        worktree_id: &WorktreeId,
        kind: AgentKind,
        model: Option<String>,
        effort: Option<String>,
    ) -> Result<()> {
        if !crate::config::Config::load().prewarm_agents {
            return Ok(());
        }
        let Some(worktree) = self.store.get_worktree(worktree_id)? else {
            return Ok(());
        };
        let stale = {
            // One warm slot per key; keep a live, young one with the same
            // spec, replace a dead, wrong-spec, or aging one (recycling
            // before the reaper hits keeps a re-requested slot gap-free).
            let mut pool = self.prewarmed.lock().unwrap();
            if let Some(entry) = pool.get(&(worktree_id.clone(), kind)) {
                if self.is_alive(&SessionRef::Agent(entry.agent_id.clone()))
                    && entry.model == model
                    && entry.effort == effort
                    && entry.spawned_at.elapsed() < PREWARM_RECYCLE_AGE
                {
                    return Ok(());
                }
                pool.remove(&(worktree_id.clone(), kind))
            } else {
                None
            }
        };
        if let Some(old) = stale {
            self.kill_session(&SessionRef::Agent(old.agent_id));
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
            archived_at: 0,
            unseen: false,
            kind,
            model: model.clone(),
            effort: effort.clone(),
            session_id: None,
            cloud_session_id: None,
            sort_order: 0,
            status_changed_at: 0,
            alive: false,
            cloud_mirroring: false,
        };
        self.spawn_agent_session(&agent, &worktree, DEFAULT_COLS, DEFAULT_ROWS)?;
        tracing::info!(agent = %agent.id, kind = kind.as_str(), worktree = %worktree.branch, "prewarmed agent session");
        let replaced = self.prewarmed.lock().unwrap().insert(
            (worktree_id.clone(), kind),
            PrewarmEntry {
                agent_id: agent.id,
                spawned_at: Instant::now(),
                model,
                effort,
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

    /// Pop the warm entry for (worktree, kind) if its PTY is still running
    /// and it booted with the requested model/effort. A dead entry (CLI
    /// missing/crashed while warm) is dropped, a wrong-spec one is killed;
    /// either way the caller falls back to a cold spawn.
    fn take_prewarmed(
        &self,
        worktree_id: &WorktreeId,
        kind: AgentKind,
        model: Option<&str>,
        effort: Option<&str>,
    ) -> Option<PrewarmEntry> {
        let entry = self
            .prewarmed
            .lock()
            .unwrap()
            .remove(&(worktree_id.clone(), kind))?;
        if !self.is_alive(&SessionRef::Agent(entry.agent_id.clone())) {
            return None;
        }
        if entry.model.as_deref() != model || entry.effort.as_deref() != effort {
            self.kill_session(&SessionRef::Agent(entry.agent_id));
            return None;
        }
        Some(entry)
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

    /// Kill every live agent and terminal PTY homed in these worktrees, and
    /// the warm spares with them — the prelude to dropping their rows
    /// (worktree delete, project remove).
    fn kill_sessions_in(
        &self,
        worktree_ids: &[WorktreeId],
        agents: &[Agent],
        terminals: &[TerminalTab],
    ) {
        for a in agents
            .iter()
            .filter(|a| worktree_ids.contains(&a.worktree_id))
        {
            self.kill_session(&SessionRef::Agent(a.id.clone()));
        }
        for t in terminals
            .iter()
            .filter(|t| worktree_ids.contains(&t.worktree_id))
        {
            self.kill_session(&SessionRef::Terminal(t.id.clone()));
        }
        self.kill_prewarmed_in(worktree_ids);
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
        if std::env::var(env::AGENT_CMD).is_ok() {
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
        self.probe_cli(kind).await
    }

    /// Fill the availability cache for every kind at boot, off the request
    /// loop. Without it the first CreateAgent of a session pays a full
    /// login-shell probe (~1s with a heavy ~/.zshrc) before it can answer.
    pub async fn warm_cli_probes(self: &Arc<Self>) {
        for kind in AgentKind::ALL {
            self.cli_available(kind).await;
        }
    }

    /// Same question, asked on behalf of a create the user just triggered.
    /// A cached *hit* is trusted; a cached *miss* is re-probed, so someone who
    /// installs the CLI and immediately retries isn't told for another minute
    /// that it's missing. Misses are rare, so this costs nothing in practice.
    async fn cli_available_for_create(&self, kind: AgentKind) -> bool {
        self.cli_available(kind).await || self.probe_cli(kind).await
    }

    /// Uncached `command -v` through the user's login shell; caches the answer.
    async fn probe_cli(&self, kind: AgentKind) -> bool {
        let check = format!("command -v '{}' >/dev/null 2>&1", kind.cli_program());
        let mut probe = tokio::process::Command::new(user_shell());
        probe
            .args(LOGIN_SHELL_ARGS)
            .arg(&check)
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
        let status = tokio::time::timeout(CLI_PROBE_TIMEOUT, probe.status()).await;
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
        self.broadcast_agent(id)?;
        Ok(())
    }

    /// Agent-initiated one-shot title (`nebula rename` inside the session's
    /// CLI). Applies only while the auto-title is still pending; afterwards
    /// it reports the standing title as an error so the CLI (and the model
    /// reading its output) knows nothing changed.
    pub fn auto_rename_agent(self: &Arc<Self>, id: &AgentId, name: &str) -> Result<()> {
        let title = sanitize_title(name);
        if title.is_empty() {
            bail!("title is empty");
        }
        let agent = self.store.get_agent(id)?.context("agent not found")?;
        if !self.store.rename_agent_if_auto_pending(id, &title)? {
            bail!(
                "session already has a title ({:?}); leaving it unchanged — a user-set \
                 title is only replaced with `nebula rename --force`",
                agent.name
            );
        }
        self.broadcast_agent(id)?;
        Ok(())
    }

    /// Re-home an agent row under another worktree of the same project. A
    /// live PTY still runs — and its hooks still report a cwd — inside the
    /// old checkout, so left alone `reparent_agent_by_cwd` would snap the
    /// row straight back on the next hook event: kill it and respawn resumed
    /// in the target so the process and the row agree. A respawn failure
    /// degrades to a dead session the next attach/prewarm revives via
    /// `ensure_session`.
    pub fn move_agent(self: &Arc<Self>, id: &AgentId, worktree_id: &WorktreeId) -> Result<()> {
        let agent = self.store.get_agent(id)?.context("agent not found")?;
        if &agent.worktree_id == worktree_id {
            return Ok(());
        }
        let target = self.sibling_worktree(&agent, worktree_id)?;
        self.pending_moves.lock().unwrap().remove(id);
        let sref = SessionRef::Agent(id.clone());
        let was_alive = self.session(&sref).is_some();
        if was_alive {
            self.kill_session(&sref);
        }
        // A deliberate move invalidates the remembered hook cwd: it still
        // points at the old checkout, and the next worktree sync would
        // replay it straight back over the user's choice.
        self.last_cwd.lock().unwrap().remove(id);
        self.store.set_agent_worktree(id, worktree_id)?;
        if was_alive {
            if let Err(e) = self.spawn_agent_session(&agent, &target, DEFAULT_COLS, DEFAULT_ROWS) {
                tracing::warn!(agent = %id, error = %e, "respawn after move failed");
            }
        }
        self.broadcast_agent(id)?;
        Ok(())
    }

    /// The worktree `worktree_id`, checked to belong to the same project as
    /// `agent`'s current one — the only kind of move a row can make.
    fn sibling_worktree(&self, agent: &Agent, worktree_id: &WorktreeId) -> Result<Worktree> {
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
        Ok(target)
    }

    /// `nebula worktree <branch>`, run by the agent inside its own session.
    /// The row moves under `branch`'s worktree of the same project now —
    /// created when the project has no checkout for that branch yet — and
    /// a live PTY follows once its turn ends (`complete_pending_move`),
    /// because the CLI running this command *is* that PTY's foreground
    /// tool call: killing it here would cut the turn off mid-answer.
    pub async fn enter_worktree(
        self: &Arc<Self>,
        id: &AgentId,
        branch: &str,
        base: Option<&str>,
    ) -> Result<(Worktree, EnterOutcome)> {
        let branch = branch.trim();
        if branch.is_empty() {
            bail!("branch name is empty");
        }
        let agent = self.store.get_agent(id)?.context("agent not found")?;
        if agent.archived {
            bail!("agent is archived");
        }
        let current = self
            .store
            .get_worktree(&agent.worktree_id)?
            .context("worktree not found")?;
        let (_, worktrees, _, _) = self.store.load_tree()?;
        let existing = worktrees
            .into_iter()
            .find(|w| w.project_id == current.project_id && w.branch == branch);
        let target = match existing {
            Some(w) => w,
            None => {
                let created = self
                    .create_worktree(&current.project_id, branch, base)
                    .await?;
                let EntityId::Worktree(new_id) = created else {
                    bail!("worktree creation returned a non-worktree entity");
                };
                self.store
                    .get_worktree(&new_id)?
                    .context("worktree not found")?
            }
        };
        if target.id == current.id {
            return Ok((target, EnterOutcome::AlreadyThere));
        }
        let alive = self.session(&SessionRef::Agent(id.clone())).is_some();
        // Same invalidation as `move_agent`: every cwd this process reports
        // until it respawns is the old checkout's.
        self.last_cwd.lock().unwrap().remove(id);
        if alive {
            self.pending_moves
                .lock()
                .unwrap()
                .insert(id.clone(), target.clone());
        }
        self.store.set_agent_worktree(id, &target.id)?;
        self.broadcast_agent(id)?;
        let outcome = if alive {
            EnterOutcome::Relocating
        } else {
            EnterOutcome::NextLaunch
        };
        Ok((target, outcome))
    }

    /// The turn an agent ran `nebula worktree` in has ended: make the
    /// process match its row. Kill it and respawn it resumed in the target,
    /// with a prompt naming the checkout it now runs in so the conversation
    /// carries straight on (Claude and pi take that prompt as an argument;
    /// codex and cursor resume silent and wait for the user). Gated on the
    /// turn-end hooks — Stop, and the idle notification a Stop-less end
    /// still fires — so a Bash hook from the same turn never triggers it.
    pub fn complete_pending_move(self: &Arc<Self>, id: &AgentId, event: &HookEvent) {
        let turn_over = match event {
            HookEvent::Stop => true,
            HookEvent::Notification { notification_type } => {
                notification_type.as_deref() == Some("idle_prompt")
            }
            _ => false,
        };
        if !turn_over {
            return;
        }
        let Some(target) = self.pending_moves.lock().unwrap().remove(id) else {
            return;
        };
        let agent = match self.store.get_agent(id) {
            Ok(Some(agent)) if !agent.archived && agent.worktree_id == target.id => agent,
            // Archived, deleted, or moved elsewhere by hand since: the
            // row's current home wins, nothing to relocate into.
            _ => return,
        };
        let sref = SessionRef::Agent(id.clone());
        if self.session(&sref).is_none() {
            // Died since (or the user closed it): the next launch boots in
            // the target on its own, only without the relocation notice.
            return;
        }
        tracing::info!(agent = %id, to = %target.branch, "relocating session into its worktree");
        self.kill_session(&sref);
        self.last_cwd.lock().unwrap().remove(id);
        // Claude and pi: `claude --resume <sid> "<prompt>"` and
        // `pi --session-id <sid> "<prompt>"` are verified to open on the
        // prompt; whether `codex resume` / `cursor-agent --resume` take a
        // trailing prompt is not, so their relocated sessions keep waiting
        // for the user.
        let prompt = relocation_prompt(&target);
        let prompt =
            matches!(agent.kind, AgentKind::Claude | AgentKind::Pi).then_some(prompt.as_str());
        if let Err(e) =
            self.spawn_agent_session_with(&agent, &target, DEFAULT_COLS, DEFAULT_ROWS, None, prompt)
        {
            tracing::warn!(agent = %id, error = %e, "respawn after worktree relocation failed");
        }
        self.try_broadcast_agent(id);
    }

    /// Whether `id` is between `enter_worktree` and its respawn.
    #[cfg(test)]
    fn relocation_pending(&self, id: &AgentId) -> bool {
        self.pending_moves.lock().unwrap().contains_key(id)
    }

    /// Row-only re-home: store update plus broadcast, never the PTY. The
    /// hook-cwd reparent uses this — there the process already runs in the
    /// target checkout and only the row is stale, so killing it would
    /// interrupt a live conversation for nothing.
    fn move_agent_row(self: &Arc<Self>, id: &AgentId, worktree_id: &WorktreeId) -> Result<()> {
        self.store.set_agent_worktree(id, worktree_id)?;
        self.broadcast_agent(id)?;
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
            self.last_cwd.lock().unwrap().remove(agent_id);
            return Ok(());
        };
        if agent.archived {
            self.last_cwd.lock().unwrap().remove(agent_id);
            return Ok(());
        }
        // Mid-relocation the row already sits under the target while the
        // process still reports the old checkout — ignore it until the
        // respawn lands there.
        if self.pending_moves.lock().unwrap().contains_key(agent_id) {
            return Ok(());
        }
        // Same foreign-session rule as the status machine: a payload from a
        // different CLI session only counts when the event (re)establishes
        // session ownership (UserPromptSubmit / SessionStart).
        if !captures_session {
            if let (Some(mine), Some(theirs)) = (agent.session_id.as_deref(), payload_session_id) {
                if mine != theirs {
                    return Ok(());
                }
            }
        }
        let cwd = canonical_or_raw(Path::new(cwd));
        // Remembered even when it resolves to nothing: an agent that just ran
        // `git worktree add` and stepped into the result reports a cwd nebula
        // has no row for yet, and the worktree sync replays this to finish the
        // re-home the moment that row is adopted.
        self.last_cwd
            .lock()
            .unwrap()
            .insert(agent_id.clone(), cwd.clone());
        self.reparent_agent_to_cwd(&agent, &cwd)
    }

    /// Move `agent`'s row under the worktree owning `cwd` when that is a
    /// different worktree of the same project. `cwd` must already be
    /// canonicalized.
    fn reparent_agent_to_cwd(self: &Arc<Self>, agent: &Agent, cwd: &Path) -> Result<()> {
        let Some(current) = self.store.get_worktree(&agent.worktree_id)? else {
            return Ok(());
        };
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
                    agent = %agent.id,
                    from = %current.branch,
                    to = %worktree.branch,
                    "agent re-homed by hook cwd"
                );
                self.move_agent_row(&agent.id, &worktree.id)?;
            }
        }
        Ok(())
    }

    /// Replay remembered hook cwds for `project`'s agents. Runs after the
    /// worktree sync adopts checkouts: a session that creates a worktree and
    /// enters it reports the new cwd (often on the very next `Stop`) before
    /// the row exists, and without this replay its row would sit under the
    /// old checkout until the user's next prompt.
    fn reparent_agents_by_last_cwd(self: &Arc<Self>, project: &Project) {
        let known: Vec<(AgentId, PathBuf)> = {
            let map = self.last_cwd.lock().unwrap();
            map.iter().map(|(id, p)| (id.clone(), p.clone())).collect()
        };
        for (agent_id, cwd) in known {
            let agent = match self.store.get_agent(&agent_id) {
                Ok(Some(agent)) => agent,
                Ok(None) => {
                    self.last_cwd.lock().unwrap().remove(&agent_id);
                    continue;
                }
                Err(e) => {
                    tracing::warn!(agent = %agent_id, error = %e, "cwd replay lookup failed");
                    continue;
                }
            };
            if agent.archived {
                continue;
            }
            let in_project = matches!(
                self.store.get_worktree(&agent.worktree_id),
                Ok(Some(w)) if w.project_id == project.id
            );
            if !in_project {
                continue;
            }
            if let Err(e) = self.reparent_agent_to_cwd(&agent, &cwd) {
                tracing::warn!(agent = %agent_id, error = %e, "cwd replay reparent failed");
            }
        }
    }

    pub fn archive_agent(self: &Arc<Self>, id: &AgentId) -> Result<()> {
        self.kill_session(&SessionRef::Agent(id.clone()));
        self.store.set_agent_archived(id, true)?;
        self.broadcast_agent(id)?;
        Ok(())
    }

    pub fn unarchive_agent(self: &Arc<Self>, id: &AgentId) -> Result<()> {
        self.store.set_agent_archived(id, false)?;
        self.broadcast_agent(id)?;
        Ok(())
    }

    /// A client put this agent's session on screen: its unseen-finish flag
    /// (`Agent::unseen`) is cleared, and every subscriber gets the row so
    /// their counts drop together. Nothing is sent when the flag was
    /// already clear — re-attaching to a session you've read is free.
    pub fn mark_agent_seen(&self, id: &AgentId) -> Result<()> {
        if self.store.mark_agent_seen(id)? {
            self.broadcast_agent(id)?;
        }
        Ok(())
    }

    pub fn delete_agent(self: &Arc<Self>, id: &AgentId) -> Result<()> {
        self.kill_session(&SessionRef::Agent(id.clone()));
        self.last_cwd.lock().unwrap().remove(id);
        self.pending_moves.lock().unwrap().remove(id);
        self.store.delete_agent(id)?;
        self.broadcast(ServerEvent::EntityRemoved {
            id: EntityId::Agent(id.clone()),
        });
        Ok(())
    }

    pub async fn restart_agent(self: &Arc<Self>, id: &AgentId) -> Result<()> {
        let agent = self.store.get_agent(id)?.context("agent not found")?;
        if agent.archived {
            bail!("agent is archived — unarchive it first");
        }
        // A Cloud row that never became a local session has nothing to
        // resume here: a plain restart would boot a bare CLI with no link
        // to the work. Re-enter the cloud session instead. Once a teleport
        // has produced a local session id, restarts resume that.
        // A teleport leaves a local session id on the row, so `session_id`
        // alone stops distinguishing "never entered the cloud session" from
        // "mirroring it". While the mirror is live the row is still the
        // cloud session's window: restart re-enters it rather than resuming
        // whatever the last pull happened to snapshot.
        if agent.cloud_session_id.is_some()
            && (agent.session_id.is_none() || self.cloud_mirror_active(id))
        {
            return self.attach_cloud_agent(id).await;
        }
        let worktree = self
            .store
            .get_worktree(&agent.worktree_id)?
            .context("worktree not found")?;
        self.kill_session(&SessionRef::Agent(id.clone()));
        self.spawn_agent_session(&agent, &worktree, DEFAULT_COLS, DEFAULT_ROWS)?;
        let mut broadcast_agent = agent.clone();
        broadcast_agent.alive = true;
        self.broadcast(ServerEvent::EntityUpserted {
            entity: Entity::Agent(broadcast_agent),
        });
        Ok(())
    }

    /// Re-enter the Claude Cloud session a row launched, and keep the pane
    /// current from there on.
    ///
    /// The live attach (`claude --cloud <id>`) is tried first, but only
    /// until this daemon has seen it refused once: it is a server-side
    /// rollout, so the second attempt on a gated account would just flash
    /// the same red error at the user. After a refusal every re-entry goes
    /// straight to `--teleport`, which fetches the session's transcript and
    /// branch and renders it locally. Either CLI switches the checkout to
    /// the cloud branch — and teleport refuses a dirty tree outright — so a
    /// row still sitting in the main checkout is first re-homed into a
    /// worktree of its own; the user's checkout is never the one that gets
    /// switched.
    ///
    /// A teleport is a snapshot, not a live link, so the pane it produces is
    /// registered as a *mirror*: [`Self::start_cloud_mirror`] re-teleports it
    /// on a timer until the user types into it.
    pub async fn attach_cloud_agent(self: &Arc<Self>, id: &AgentId) -> Result<()> {
        let agent = self.store.get_agent(id)?.context("agent not found")?;
        if agent.archived {
            bail!("agent is archived — unarchive it first");
        }
        let Some(cloud_id) = agent.cloud_session_id.clone() else {
            bail!("session was not launched in Claude Cloud");
        };
        let worktree = self.cloud_worktree_for(&agent, &cloud_id).await?;
        self.kill_session(&SessionRef::Agent(id.clone()));
        let launch =
            cloud_reentry_launch(&cloud_id, self.cloud_attach_gated.load(Ordering::Relaxed));
        self.spawn_agent_session_with(
            &agent,
            &worktree,
            DEFAULT_COLS,
            DEFAULT_ROWS,
            Some(launch),
            None,
        )?;
        self.start_cloud_mirror(id.clone());
        self.broadcast_agent(id)?;
        Ok(())
    }

    /// The checkout a Cloud row re-enters its session in. A row sitting in
    /// the main checkout is re-homed into a `cloud-<id>` worktree first —
    /// both the attach and the teleport check the cloud branch out where
    /// they run, and the user's main checkout must never be that place.
    async fn cloud_worktree_for(
        self: &Arc<Self>,
        agent: &Agent,
        cloud_id: &str,
    ) -> Result<Worktree> {
        let worktree = self
            .store
            .get_worktree(&agent.worktree_id)?
            .context("worktree not found")?;
        if !worktree.is_main {
            return Ok(worktree);
        }
        let branch = cloud_worktree_branch(cloud_id);
        let EntityId::Worktree(target) = self
            .create_worktree(&worktree.project_id, &branch, None)
            .await?
        else {
            bail!("worktree create returned a non-worktree entity");
        };
        let moved = self
            .store
            .get_worktree(&target)?
            .context("worktree not found")?;
        // Same invalidation as a deliberate move: the remembered hook cwd
        // points at the old checkout and would sync the row back.
        self.last_cwd.lock().unwrap().remove(&agent.id);
        self.store.set_agent_worktree(&agent.id, &target)?;
        tracing::info!(agent = %agent.id, branch, "cloud row re-homed into its own worktree");
        Ok(moved)
    }

    /// Follow a Cloud row's session: re-teleport its pane every
    /// [`CLOUD_MIRROR_REFRESH`] so turns the cloud agent has taken since the
    /// last pull show up without anyone opening a browser.
    ///
    /// The mirror stops for good the moment the pane is typed into. A
    /// teleport is a full kill-and-respawn of the local CLI, so refreshing
    /// under someone mid-sentence would eat their turn — the first keystroke
    /// is the handover: from then on the pane is an ordinary local session
    /// that happens to have started from a cloud transcript.
    fn start_cloud_mirror(self: &Arc<Self>, id: AgentId) {
        let Some(cadence) = cloud_mirror_refresh() else {
            return;
        };
        let token = Arc::new(tokio_util::sync::CancellationToken::new());
        if let Some(previous) = self
            .cloud_mirrors
            .lock()
            .unwrap()
            .insert(id.clone(), token.clone())
        {
            previous.cancel();
        }
        let daemon = self.clone();
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = token.cancelled() => break,
                    _ = daemon.shutdown.cancelled() => break,
                    _ = tokio::time::sleep(cadence) => {}
                }
                match daemon.refresh_cloud_mirror(&id).await {
                    Ok(true) => {}
                    Ok(false) => break,
                    Err(e) => {
                        tracing::warn!(agent = %id, error = %e, "cloud mirror refresh failed");
                        break;
                    }
                }
            }
            // Only clear the slot if it is still ours: a newer mirror may
            // have replaced (and cancelled) this one already.
            let ours = {
                let mut mirrors = daemon.cloud_mirrors.lock().unwrap();
                let ours = mirrors.get(&id).is_some_and(|t| Arc::ptr_eq(t, &token));
                if ours {
                    mirrors.remove(&id);
                }
                ours
            };
            // The row wears a "following" badge while this task runs, so
            // its end is news: re-broadcast so the badge goes back to a
            // plain `cloud` instead of promising refreshes nobody is doing.
            if ours && !daemon.shutdown.is_cancelled() {
                daemon.try_broadcast_agent(&id);
            }
        });
    }

    /// Cancel a row's mirror, if it has one. Called whenever the row is
    /// respawned as something other than a cloud re-entry — a plain restart,
    /// an archive, a delete — so a pending tick cannot teleport over it.
    fn stop_cloud_mirror(&self, id: &AgentId) {
        if let Some(token) = self.cloud_mirrors.lock().unwrap().remove(id) {
            token.cancel();
        }
    }

    pub fn cloud_mirror_active(&self, id: &AgentId) -> bool {
        self.cloud_mirrors.lock().unwrap().contains_key(id)
    }

    /// One mirror tick. `Ok(false)` means stop following: the row was typed
    /// into, archived, deleted, or lost its cloud session id.
    async fn refresh_cloud_mirror(self: &Arc<Self>, id: &AgentId) -> Result<bool> {
        let Some(agent) = self.store.get_agent(id)? else {
            return Ok(false);
        };
        if agent.archived {
            return Ok(false);
        }
        let Some(cloud_id) = agent.cloud_session_id.clone() else {
            return Ok(false);
        };
        let sref = SessionRef::Agent(id.clone());
        let live = self.sessions.lock().unwrap().get(&sref).cloned();
        match live {
            Some(session) if session.input_seen() => {
                tracing::info!(agent = %id, "cloud mirror adopted — the pane has been typed into");
                return Ok(false);
            }
            Some(_) => {}
            // The pane this mirror last spawned is gone. Either the idle
            // reaper took it — nobody has looked at this row in a long
            // time, and respawning it every tick would make cloud rows the
            // one kind of session that can never be reaped — or the
            // teleport itself died, in which case retrying it forever is
            // the wrong answer too. Stop; opening the row re-enters the
            // session and starts a fresh mirror.
            None => {
                tracing::info!(agent = %id, "cloud mirror stopping — its pane is gone");
                return Ok(false);
            }
        }
        let worktree = self.cloud_worktree_for(&agent, &cloud_id).await?;
        self.kill_session(&sref);
        self.spawn_agent_session_with(
            &agent,
            &worktree,
            DEFAULT_COLS,
            DEFAULT_ROWS,
            Some(CloudLaunch::Teleport(&cloud_id)),
            None,
        )?;
        self.try_broadcast_agent(id);
        Ok(true)
    }

    /// Queue a message on a Cloud session without leaving nebula.
    /// `claude -p <msg> --cloud <id>` is fire-and-forget — the CLI prints
    /// "Sent to cloud session." and returns, the reply only ever shows up in
    /// the transcript — so the send is followed by an immediate mirror
    /// refresh, and the answer lands in the pane on a later tick.
    pub async fn send_cloud_message(self: &Arc<Self>, id: &AgentId, message: &str) -> Result<()> {
        let agent = self.store.get_agent(id)?.context("agent not found")?;
        let Some(cloud_id) = agent.cloud_session_id.clone() else {
            bail!("session was not launched in Claude Cloud");
        };
        let message = validate_cloud_text(message, "message")?;
        let worktree = self
            .store
            .get_worktree(&agent.worktree_id)?
            .context("worktree not found")?;

        let cmd_override = std::env::var(env::AGENT_CMD).ok();
        let (program, args) = match cmd_override.as_deref() {
            Some(over) => (over.to_string(), Vec::new()),
            None => login_shell_wrap(
                &user_shell(),
                "claude",
                &[
                    "-p".to_string(),
                    message.clone(),
                    format!("--cloud={cloud_id}"),
                ],
            ),
        };
        let output = tokio::process::Command::new(&program)
            .args(&args)
            .current_dir(&worktree.path)
            .output()
            .await
            .context("run claude -p --cloud")?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let detail = stderr.trim().lines().last().unwrap_or("").to_string();
            bail!(
                "claude could not reach the cloud session{}",
                if detail.is_empty() {
                    String::new()
                } else {
                    format!(": {detail}")
                }
            );
        }
        tracing::info!(agent = %id, cloud_session = %cloud_id, bytes = message.len(), "message sent to cloud session");
        // Pull the transcript now so the send is visibly acknowledged, and
        // make sure the row keeps following from here even if it had been
        // sitting dead since a create.
        if !self.cloud_mirror_active(id) {
            self.start_cloud_mirror(id.clone());
        }
        let _ = self.refresh_cloud_mirror(id).await;
        Ok(())
    }

    /// A `claude --cloud <task>` create prints the new session's id and
    /// exits — on this rollout it never stays attached. Left alone the row
    /// is a dead pane whose last line is "Resume with: claude --teleport
    /// …", which tells the user to go somewhere else to watch their own
    /// agent work. So: capture the id off the output, wait for the create
    /// to finish, and re-enter the session, which leaves the row mirroring
    /// the cloud transcript.
    ///
    /// The id is persisted here as well as in `watch_for_exit` (both listen
    /// to the same broadcast, in no fixed order) so the re-entry cannot read
    /// a row the other task has not written yet. Both writes are the same
    /// value, so whichever lands second is a no-op.
    fn arm_cloud_follow(self: &Arc<Self>, id: AgentId, session: Arc<PtySession>) {
        let daemon = self.clone();
        let mut rx = session.events.subscribe();
        tokio::spawn(async move {
            let mut cloud_id: Option<String> = None;
            loop {
                match rx.recv().await {
                    Ok(PtyEvent::CloudSession { id }) => cloud_id = Some(id),
                    Ok(PtyEvent::Exited { .. }) => break,
                    Ok(_) => continue,
                    Err(broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
            // No id means the create failed (bad task, no auth, offline).
            // Its error text is the most useful thing the pane can show.
            let Some(cloud_id) = cloud_id else { return };
            if daemon.shutdown.is_cancelled() {
                return;
            }
            match daemon.store.get_agent(&id) {
                Ok(Some(agent)) if !agent.archived => {}
                _ => return,
            }
            if let Err(e) = daemon
                .store
                .set_agent_cloud_session_id(&id, Some(&cloud_id))
            {
                tracing::warn!(agent = %id, error = %e, "cloud session id not persisted");
                return;
            }
            tracing::info!(agent = %id, cloud_session = %cloud_id, "created — re-entering to mirror it");
            if let Err(e) = daemon.attach_cloud_agent(&id).await {
                tracing::warn!(agent = %id, error = %e, "cloud follow failed");
            }
        });
    }

    /// `claude --cloud <id>` on an account without the attach rollout
    /// prints "Attaching to an existing cloud session is not enabled for
    /// your account." and exits. The refusal is *read* off the output
    /// (`pty::cloud`), not inferred from the exit: a deliberate kill of an
    /// attach that worked looks identical by exit code and must not spawn
    /// anything. Once the refused child is gone, the same row is respawned
    /// as `claude --teleport <id>` in the same worktree.
    fn arm_cloud_attach_fallback(
        self: &Arc<Self>,
        agent: Agent,
        worktree: Worktree,
        session: Arc<PtySession>,
        cloud_id: String,
        cols: u16,
        rows: u16,
    ) {
        let daemon = self.clone();
        let mut rx = session.events.subscribe();
        tokio::spawn(async move {
            let mut rejected = false;
            loop {
                match rx.recv().await {
                    Ok(PtyEvent::CloudAttachRejected) => rejected = true,
                    Ok(PtyEvent::Exited { .. }) => break,
                    Ok(_) => continue,
                    Err(broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(broadcast::error::RecvError::Closed) => return,
                }
            }
            if !rejected {
                return;
            }
            // Live attach is a server-side rollout, not a per-row accident:
            // remember the refusal so no later re-entry shows it again.
            daemon.cloud_attach_gated.store(true, Ordering::Relaxed);
            // Archived, deleted, or moved inside the window: leave it be.
            match daemon.store.get_agent(&agent.id) {
                Ok(Some(current)) if !current.archived && current.worktree_id == worktree.id => {}
                _ => return,
            }
            tracing::info!(agent = %agent.id, "cloud attach refused — teleporting the session locally");
            match daemon.spawn_agent_session_with(
                &agent,
                &worktree,
                cols,
                rows,
                Some(CloudLaunch::Teleport(&cloud_id)),
                None,
            ) {
                Ok(_) => {
                    daemon.start_cloud_mirror(agent.id.clone());
                    daemon.try_broadcast_agent(&agent.id);
                }
                Err(e) => tracing::warn!(agent = %agent.id, error = %e, "teleport spawn failed"),
            }
        });
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
        self.spawn_terminal_session(&terminal, &worktree, DEFAULT_COLS, DEFAULT_ROWS)?;
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

    // ---- links ----

    pub fn create_link(self: &Arc<Self>, worktree_id: &WorktreeId, url: &str) -> Result<EntityId> {
        let url = normalize_url(url)?;
        self.store
            .get_worktree(worktree_id)?
            .context("worktree not found")?;
        let link = Link {
            id: LinkId::generate(),
            worktree_id: worktree_id.clone(),
            url,
            sort_order: self.store.next_link_sort_order(worktree_id)?,
        };
        self.store.insert_link(&link)?;
        self.broadcast(ServerEvent::EntityUpserted {
            entity: Entity::Link(link.clone()),
        });
        Ok(EntityId::Link(link.id))
    }

    pub fn update_link(self: &Arc<Self>, id: &LinkId, url: &str) -> Result<()> {
        let url = normalize_url(url)?;
        self.store.set_link_url(id, &url)?;
        let link = self.store.get_link(id)?.context("link not found")?;
        self.broadcast(ServerEvent::EntityUpserted {
            entity: Entity::Link(link),
        });
        Ok(())
    }

    pub fn delete_link(self: &Arc<Self>, id: &LinkId) -> Result<()> {
        self.store.delete_link(id)?;
        self.broadcast(ServerEvent::EntityRemoved {
            id: EntityId::Link(id.clone()),
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
        // Hold the gate across the whole check-and-install: an Attach and the
        // prewarm sweep racing the same dead session must produce one CLI,
        // not two. Re-check under it — the winner installed while we waited.
        let _gate = self.spawn_gate.lock().unwrap();
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

    /// Boot every dead, non-archived session under `worktree_id` (agents and
    /// terminals) so a later Attach replays an already-running screen.
    /// Already-alive sessions pass through ensure_session untouched; one
    /// session failing to spawn (missing CLI, deleted checkout) is logged
    /// and doesn't stop the rest.
    pub fn prewarm_worktree_sessions(
        self: &Arc<Self>,
        worktree_id: &WorktreeId,
        cols: u16,
        rows: u16,
    ) {
        if !crate::config::Config::load().prewarm_sessions {
            return;
        }
        let daemon = self.clone();
        let worktree_id = worktree_id.clone();
        let handle = tokio::spawn(async move {
            daemon.run_worktree_prewarm(&worktree_id, cols, rows).await;
        });
        // Supersede whatever sweep was still warming the worktree the user
        // has now left; its remaining boots are wasted work.
        if let Some(old) = self.prewarm_sweep.lock().unwrap().replace(handle) {
            old.abort();
        }
    }

    /// The sweep itself: boot the worktree's dead sessions one at a time,
    /// [`PREWARM_STAGGER`] apart. Deliberately off the connection's request
    /// loop — it used to run inline, which stalled that client's Input and
    /// Attach frames for as long as the whole burst of forks took.
    async fn run_worktree_prewarm(
        self: &Arc<Self>,
        worktree_id: &WorktreeId,
        cols: u16,
        rows: u16,
    ) {
        let Ok((_, _, agents, terminals)) = self.store.load_tree() else {
            return;
        };
        let srefs: Vec<SessionRef> = agents
            .iter()
            .filter(|a| &a.worktree_id == worktree_id && !a.archived)
            .map(|a| SessionRef::Agent(a.id.clone()))
            .chain(
                terminals
                    .iter()
                    .filter(|t| &t.worktree_id == worktree_id)
                    .map(|t| SessionRef::Terminal(t.id.clone())),
            )
            .collect();
        for sref in srefs {
            // The prewarm doubles as a "user is looking here" signal for
            // the idle reaper, for alive sessions as much as fresh spawns.
            self.touch_session(&sref);
            // Already warm — most importantly the one the user just
            // attached to, which Attach spawned a moment ago.
            if self.is_alive(&sref) {
                continue;
            }
            let daemon = self.clone();
            let target = sref.clone();
            // fork/exec blocks; keep it off the async worker threads.
            let spawned = tokio::task::spawn_blocking(move || {
                daemon.ensure_session(&target, cols, rows).map(|_| ())
            })
            .await;
            match spawned {
                Ok(Err(e)) => {
                    tracing::debug!(session = ?sref, error = %e, "session prewarm failed")
                }
                Err(e) => tracing::debug!(session = ?sref, error = %e, "session prewarm panicked"),
                Ok(Ok(())) => {}
            }
            tokio::time::sleep(PREWARM_STAGGER).await;
        }
    }

    fn spawn_agent_session(
        self: &Arc<Self>,
        agent: &Agent,
        worktree: &Worktree,
        cols: u16,
        rows: u16,
    ) -> Result<Arc<PtySession>> {
        self.spawn_agent_session_with(agent, worktree, cols, rows, None, None)
    }

    /// The general spawn: `cloud` makes it a Claude Cloud launch (the
    /// initial dispatch, or a later attach/teleport of the session it
    /// created), `initial_prompt` a first turn the CLI submits on its own
    /// (the relocation notice a `nebula worktree` respawn opens with, or the
    /// prefix + task + postfix an AGENT PRESET launch composes). Both
    /// are intentionally transient: later restarts/resumes follow the
    /// persisted Agent fields — a Cloud row's `cloud_session_id` routes a
    /// restart back through `attach_cloud_agent`, everything else takes the
    /// plain local-session path.
    fn spawn_agent_session_with(
        self: &Arc<Self>,
        agent: &Agent,
        worktree: &Worktree,
        cols: u16,
        rows: u16,
        cloud: Option<CloudLaunch<'_>>,
        initial_prompt: Option<&str>,
    ) -> Result<Arc<PtySession>> {
        // Whatever spawns this agent, it runs in `worktree` from here: a
        // relocation still pending for it has been overtaken.
        self.pending_moves.lock().unwrap().remove(&agent.id);
        // A spawn that isn't a cloud re-entry replaces the pane for good —
        // a pending mirror tick must not teleport over it. (The re-entries
        // re-arm their own mirror; a create arms one once it has an id.)
        if !matches!(
            cloud,
            Some(CloudLaunch::Attach(_)) | Some(CloudLaunch::Teleport(_))
        ) {
            self.stop_cloud_mirror(&agent.id);
        }
        // Managed status hooks; a failure here degrades to "no status
        // updates", never blocks the spawn.
        let install_result = match agent.kind {
            AgentKind::Claude => hooks::installer::install_claude_hooks(&worktree.path),
            // Codex's hooks live in its home, not the worktree, so one
            // trust approval covers every worktree (see installer docs);
            // any per-worktree copy an older nebula left is pruned.
            AgentKind::Codex => {
                hooks::installer::install_codex_hooks(&hooks::installer::codex_home())
                    .and_then(|()| hooks::installer::prune_codex_worktree_hooks(&worktree.path))
            }
            // Cursor also gets the managed auto-title project rule — its
            // hook dialect has no context-injection channel.
            AgentKind::Cursor => hooks::installer::install_cursor_hooks(&worktree.path)
                .and_then(|()| hooks::installer::install_cursor_title_rule(&worktree.path)),
            // Pi runs TypeScript extensions, not shell hooks: one managed
            // extension in its global agent dir (loaded without the trust
            // prompt a worktree-local `.pi/extensions/` would raise) serves
            // every worktree.
            AgentKind::Pi => hooks::pi_extension::install(&hooks::pi_extension::pi_agent_dir()),
        };
        if let Err(e) = install_result {
            tracing::warn!(error = %e, cwd = %worktree.path.display(), "hook install failed");
        }

        // NEBULA_AGENT_CMD overrides for tests; default is the kind's CLI.
        let cmd_override = std::env::var(env::AGENT_CMD).ok();
        // A PR SESSION's rule rides Claude's system prompt, or opens a
        // Codex / Cursor cold spawn as its first prompt (see `pr_scope`).
        let pr_url = if cloud.is_none() {
            self.store.agent_pr_url(&agent.id)?
        } else {
            None
        };
        let prompts = crate::pr_scope::launch_prompts(
            agent.kind,
            agent.session_id.is_some(),
            pr_url.as_deref(),
            initial_prompt,
        );
        let (program, args, resumed) = match cloud {
            Some(launch) => claude_cloud_spawn_command(
                launch,
                agent.model.as_deref(),
                agent.effort.as_deref(),
                cmd_override.as_deref(),
            ),
            None => agent_spawn_command_with(
                agent.kind,
                agent.session_id.as_deref(),
                agent.model.as_deref(),
                agent.effort.as_deref(),
                cmd_override.as_deref(),
                prompts.initial.as_deref(),
                prompts.system.as_deref(),
                true,
            ),
        };
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
                (env::AGENT_ID.into(), agent.id.to_string()),
                (
                    env::API_URL.into(),
                    format!("http://127.0.0.1:{}", self.hook_env.port),
                ),
                (env::API_TOKEN.into(), self.hook_env.token.clone()),
            ],
            scrub_env: env::AGENT_SESSION_VARS,
            cols,
            rows,
        };
        let sref = SessionRef::Agent(agent.id.clone());
        let session = PtySession::spawn(sref, spec)?;
        self.install_session(session.clone());
        if resumed {
            self.arm_resume_fallback(agent.clone(), worktree.clone(), session.clone(), cols, rows);
        }
        match cloud {
            // The create prints the session id and (on accounts without
            // the attach rollout) exits at once: capture it off the output,
            // then re-enter the session it just made so the row shows the
            // cloud agent's work instead of a "resume with" hint.
            Some(CloudLaunch::Create(_)) => {
                session.arm_cloud_scan();
                self.arm_cloud_follow(agent.id.clone(), session.clone());
            }
            Some(CloudLaunch::Attach(id)) => {
                session.arm_cloud_scan();
                self.arm_cloud_attach_fallback(
                    agent.clone(),
                    worktree.clone(),
                    session.clone(),
                    id.to_string(),
                    cols,
                    rows,
                );
            }
            Some(CloudLaunch::Teleport(_)) | None => {}
        }
        Ok(session)
    }

    /// A resumed session (`claude --resume` / `codex resume` /
    /// `cursor-agent --resume`) dies fast when
    /// it is stale/deleted — fall back to a fresh session instead of leaving
    /// a dead pane. (`pi --session-id` creates a missing id instead of
    /// dying, so pi never takes this path; arming it is harmless.)
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
            // A deliberate kill looks identical to a failed resume from here:
            // the agent may have been archived or deleted inside the window —
            // never resurrect those.
            match daemon.store.get_agent(&agent.id) {
                Ok(Some(current)) if !current.archived => {}
                _ => return,
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
            scrub_env: env::AGENT_SESSION_VARS,
            cols,
            rows,
        };
        let sref = SessionRef::Terminal(terminal.id.clone());
        let session = PtySession::spawn(sref, spec)?;
        self.install_session(session.clone());
        Ok(session)
    }

    fn install_session(self: &Arc<Self>, session: Arc<PtySession>) {
        self.touch_session(&session.sref);
        self.sessions
            .lock()
            .unwrap()
            .insert(session.sref.clone(), session.clone());
        // After the insert: a forward task woken by this looks the ref up.
        let _ = self.session_installs.send(session.sref.clone());
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
                        if was_registered {
                            daemon.session_interest.lock().unwrap().remove(&sref);
                        }
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
                    // The CLI's own busy/idle bit, read off its output. It is
                    // the only end-of-turn news after a user cancel: Claude
                    // Code fires no Stop for an interrupted turn, and
                    // suppresses the idle notification because the user just
                    // pressed a key. See `pty::progress`.
                    Ok(PtyEvent::Progress { busy }) => {
                        if let SessionRef::Agent(id) = &sref {
                            daemon.apply_hook_event(id, HookEvent::Progress { busy }, None);
                        }
                    }
                    // The Cloud session this row launched, read off the
                    // `claude --cloud` output. Persisted at once — the child
                    // is typically gone within milliseconds of printing it —
                    // and re-broadcast so the row grows its `cloud` badge and
                    // its attach menu entry.
                    Ok(PtyEvent::CloudSession { id: cloud_id }) => {
                        if let SessionRef::Agent(id) = &sref {
                            match daemon.store.set_agent_cloud_session_id(id, Some(&cloud_id)) {
                                Ok(()) => {
                                    tracing::info!(agent = %id, cloud_session = %cloud_id, "cloud session id captured");
                                    daemon.try_broadcast_agent(id);
                                }
                                Err(e) => {
                                    tracing::warn!(agent = %id, error = %e, "cloud session id not persisted")
                                }
                            }
                        }
                    }
                    Ok(_) => {}
                    Err(broadcast::error::RecvError::Lagged(_)) => {
                        // A fire-hosing child can push progress edges off the
                        // broadcast queue. The scanner itself never lags, so
                        // reconcile from its current reading rather than
                        // leaving the status stuck on a dropped edge.
                        if let (SessionRef::Agent(id), Some(busy)) =
                            (&sref, session.progress_busy())
                        {
                            daemon.apply_hook_event(id, HookEvent::Progress { busy }, None);
                        }
                        continue;
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
        });
    }
}

/// Program + args for an agent PTY. An override (tests) is used verbatim —
/// no resume args. Otherwise the kind picks the CLI and its resume shape:
/// `claude --resume <sid>` and `cursor-agent --resume <sid>` (flag) vs
/// `codex resume <sid>` (subcommand, so resume args must lead); pi takes
/// `pi --session-id <sid>`, which resumes the id where it exists and
/// creates it where it doesn't (a relocated session's new cwd). Codex and
/// cursor always get their skip-permissions flag (`--yolo` / `--force`),
/// appended after the resume args — same convention as Mission Control;
/// pi has no permission gate to skip.
/// Model/effort choices follow: `claude --model m --effort e`,
/// `codex -m m -c model_reasoning_effort=e`, `pi --model m --thinking e`,
/// and for cursor one flat id
/// joined from the two — `cursor-agent --model m-e` (`--model m` when
/// effort is None; the CLI's catalogue bakes the effort into the id and
/// rejects the `m[effort=e]` form its `--help` advertises).
/// Claude and pi then get nebula's worktree guidance appended to the system
/// prompt, any persisted PR scope is composed into that same system-prompt
/// argument, and an `initial_prompt` — the relocation notice a `nebula
/// worktree` respawn opens with, or the starting prompt an AGENT PRESET
/// launch composes — goes last, as the CLI's trailing positional prompt
/// (`claude [prompt]`, `codex [PROMPT]`, `cursor-agent [prompt...]`,
/// `pi [messages...]`).
///
/// The plain shape, as every restart/resume spawns it: no initial prompt,
/// guidance on. Tests assert against this; the daemon calls the full form.
#[cfg(test)]
fn agent_spawn_command(
    kind: AgentKind,
    session_id: Option<&str>,
    model: Option<&str>,
    effort: Option<&str>,
    cmd_override: Option<&str>,
) -> (String, Vec<String>, bool) {
    agent_spawn_command_with(
        kind,
        session_id,
        model,
        effort,
        cmd_override,
        None,
        None,
        true,
    )
}

/// What nebula appends to Claude's system prompt: how to take a "do this
/// in a worktree" request through nebula (`Daemon::enter_worktree`) instead
/// of Claude's own EnterWorktree tool, whose checkout lands under
/// `<repo>/.claude/worktrees/` on a `worktree-*` branch — a layout the
/// worktree list only adopts after the fact, and not where a nebula user
/// keeps their worktrees. Claude and pi (both take `--append-system-prompt`;
/// pi has no EnterWorktree, but does run `git worktree add` on its own
/// unless told otherwise): codex and cursor have no system-prompt flag.
pub const CLAUDE_WORKTREE_GUIDANCE: &str = "[nebula] This session runs inside nebula, which \
manages this project's git worktrees. When the user asks you to work in a worktree (\"do this in a \
worktree\", \"in a new worktree\", \"branch this off in its own checkout\"), do not use the \
EnterWorktree tool and do not run `git worktree add` yourself. Run this shell command instead, \
exactly once:\n\n  nebula worktree <name>\n\nwhere <name> is the branch name the user gave, or a \
short kebab-case name for the task (`nebula worktree` with no name invents one; `--base <ref>` picks \
the start point). nebula creates the worktree, associates this session with it, and relocates the \
session into it once your current turn ends. So when the command succeeds, end your turn at once: \
tell the user in one line that the session is moving into the worktree, and make no further tool \
calls or edits — you will be resumed inside the worktree with a prompt to carry on there. If the \
command fails, report the error and carry on in the current checkout.";

/// The prompt a relocated Claude session is resumed with: it names the
/// checkout the process now runs in and asks for the work to pick back up
/// there, so the user never has to type "continue".
fn relocation_prompt(worktree: &Worktree) -> String {
    format!(
        "[nebula] This session now runs inside the worktree `{}` at {} — your working \
         directory is that checkout. Continue the user's most recent request there.",
        worktree.branch,
        worktree.path.display()
    )
}

// Eight positional knobs is one over clippy's line; the callers are the two
// thin wrappers above and the tests, so a builder would only add ceremony.
#[allow(clippy::too_many_arguments)]
fn agent_spawn_command_with(
    kind: AgentKind,
    session_id: Option<&str>,
    model: Option<&str>,
    effort: Option<&str>,
    cmd_override: Option<&str>,
    initial_prompt: Option<&str>,
    additional_system_prompt: Option<&str>,
    guidance: bool,
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
        (AgentKind::Pi, Some(sid)) => (vec!["--session-id".to_string(), sid.to_string()], true),
        (_, None) => (Vec::new(), false),
    };
    match kind {
        AgentKind::Codex => args.push("--yolo".to_string()),
        AgentKind::Cursor => args.push("--force".to_string()),
        AgentKind::Claude | AgentKind::Pi => {}
    }
    // Claude, codex and pi spell the model flag the same way, and it
    // follows the skip-permissions flag (`codex --yolo --model …`). Cursor
    // composes its own below: family and effort become one id.
    if let (Some(m), AgentKind::Claude | AgentKind::Codex | AgentKind::Pi) = (model, kind) {
        args.extend(["--model".to_string(), m.to_string()]);
    }
    match kind {
        AgentKind::Claude => {
            if let Some(e) = effort {
                args.extend(["--effort".to_string(), e.to_string()]);
            }
            push_system_prompt(&mut args, guidance, additional_system_prompt);
            if let Some(p) = initial_prompt {
                args.push(p.to_string());
            }
        }
        AgentKind::Pi => {
            // pi's reasoning knob is `--thinking <off|minimal|…|max>`.
            if let Some(e) = effort {
                args.extend(["--thinking".to_string(), e.to_string()]);
            }
            push_system_prompt(&mut args, guidance, additional_system_prompt);
            // `pi [options] [--] [@files...] [messages...]` — trailing.
            if let Some(p) = initial_prompt {
                args.push(p.to_string());
            }
        }
        AgentKind::Codex => {
            if let Some(e) = effort {
                args.extend(["-c".to_string(), format!("model_reasoning_effort={e}")]);
            }
            // `codex [OPTIONS] [PROMPT]` — the trailing positional.
            if let Some(p) = initial_prompt {
                args.push(p.to_string());
            }
        }
        AgentKind::Cursor => {
            // `--model <family>-<effort>`: the TUI keeps the family and the
            // effort suffix apart (`claude-opus-5` + `high`) and only sends
            // an effort the family ships; an effort without a family has
            // nothing to hang off and is dropped.
            if let Some(m) = model {
                let id = match effort {
                    Some(e) => format!("{m}-{e}"),
                    None => m.to_string(),
                };
                args.extend(["--model".to_string(), id]);
            }
            // `cursor-agent [options] [prompt...]` — the trailing positional.
            if let Some(p) = initial_prompt {
                args.push(p.to_string());
            }
        }
    }
    (program, args, resumed)
}

/// One `--append-system-prompt` carrying nebula's guidance (worktree, spawn,
/// then open) and whatever else the launch adds (the PR scope), for the CLIs
/// that take the flag — Claude and pi.
fn push_system_prompt(args: &mut Vec<String>, guidance: bool, additional: Option<&str>) {
    let mut system_prompt = Vec::new();
    if guidance {
        system_prompt.push(CLAUDE_WORKTREE_GUIDANCE);
        system_prompt.push(crate::sibling::CLAUDE_SPAWN_GUIDANCE);
        system_prompt.push(crate::open_files::CLAUDE_OPEN_GUIDANCE);
    }
    if let Some(prompt) = additional {
        system_prompt.push(prompt);
    }
    if !system_prompt.is_empty() {
        args.extend([
            "--append-system-prompt".to_string(),
            system_prompt.join("\n\n"),
        ]);
    }
}

/// Validate an AGENT PRESET's composed starting prompt before it becomes the
/// CLI's positional argument. Same bounds as a cloud task — it crosses the
/// same login-shell `-c` string and argv — with its own wording.
fn validate_starting_prompt(raw: &str) -> Result<String> {
    let text = raw.trim().to_string();
    if text.is_empty() {
        bail!("starting prompt is empty");
    }
    if text.contains('\0') {
        bail!("starting prompt cannot contain NUL bytes");
    }
    if text.len() > MAX_CLOUD_PROMPT_BYTES {
        bail!(
            "starting prompt is too long (max {} KiB)",
            MAX_CLOUD_PROMPT_BYTES / 1024
        );
    }
    Ok(text)
}

/// How a Claude PTY enters the Cloud: dispatch a fresh task, attach live
/// to the session it created, or teleport that session into a local one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CloudLaunch<'a> {
    Create(&'a str),
    Attach(&'a str),
    Teleport(&'a str),
}

/// How a Cloud row re-enters its session. The live attach is worth one
/// try per daemon — it is a server-side rollout, so the answer can change
/// between runs — but not a second, because a gated account answers with a
/// red `not enabled for your account` in the user's pane every time.
fn cloud_reentry_launch(cloud_id: &str, attach_gated: bool) -> CloudLaunch<'_> {
    if attach_gated {
        CloudLaunch::Teleport(cloud_id)
    } else {
        CloudLaunch::Attach(cloud_id)
    }
}

/// Trim and bounds-check text handed to the Claude CLI as one argv item —
/// a Cloud task on create, a message queued on an existing session. Both
/// ride the login shell's `-c` string as well as Claude's argv: quoting
/// stops injection, but a NUL would truncate the command and an unbounded
/// string would blow the argv limit, so both are rejected here rather than
/// at the shell.
fn validate_cloud_text(raw: &str, what: &str) -> Result<String> {
    let text = raw.trim().to_string();
    if text.is_empty() {
        bail!("Claude Cloud needs a {what}");
    }
    if text.contains('\0') {
        bail!("Claude Cloud {what} cannot contain NUL bytes");
    }
    if text.len() > MAX_CLOUD_PROMPT_BYTES {
        bail!(
            "Claude Cloud {what} is too long (max {} KiB)",
            MAX_CLOUD_PROMPT_BYTES / 1024
        );
    }
    Ok(text)
}

/// Branch (and so directory) of the worktree a Cloud row is re-homed into
/// before attaching: the CLI checks the cloud branch out on top of it, so
/// the name only has to be stable per session and safe for git.
fn cloud_worktree_branch(cloud_id: &str) -> String {
    let suffix: String = cloud_id
        .trim_start_matches("session_")
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .collect();
    let tail = suffix.len().saturating_sub(8);
    format!("cloud-{}", &suffix[tail..])
}

/// Cloud launches are one-shot variations of the normal fresh-Claude
/// command. Keeping them a wrapper leaves every resume/restart caller on
/// the persisted local-session contract, and makes the no-override argument
/// shape directly unit-testable. No worktree guidance either: the Cloud
/// sandbox has no nebula CLI to follow it with. Values bind with `=`
/// (`--cloud=<task>`, `--cloud=<id>`, `--teleport=<id>`): both flags take an
/// *optional* value, so a separate argv item that starts with `--` would be
/// parsed as another Claude flag.
fn claude_cloud_spawn_command(
    launch: CloudLaunch<'_>,
    model: Option<&str>,
    effort: Option<&str>,
    cmd_override: Option<&str>,
) -> (String, Vec<String>, bool) {
    let (program, mut args, resumed) = agent_spawn_command_with(
        AgentKind::Claude,
        None,
        model,
        effort,
        cmd_override,
        None,
        None,
        false,
    );
    if cmd_override.is_none() {
        let flag = match launch {
            CloudLaunch::Create(task) => format!("--cloud={task}"),
            CloudLaunch::Attach(id) => format!("--cloud={id}"),
            CloudLaunch::Teleport(id) => format!("--teleport={id}"),
        };
        args.insert(0, flag);
    }
    (program, args, resumed)
}

/// Normalize an agent-supplied title: control characters become spaces,
/// whitespace collapses, and over-long titles are cut — models occasionally
/// hand over a whole sentence no matter what the instruction says.
fn sanitize_title(raw: &str) -> String {
    const MAX_CHARS: usize = 60;
    let cleaned: String = raw
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    let mut title = cleaned.split_whitespace().collect::<Vec<_>>().join(" ");
    if title.chars().count() > MAX_CHARS {
        title = title.chars().take(MAX_CHARS).collect();
        title.truncate(title.trim_end().len());
    }
    title
}

/// Canonicalize for path containment tests, falling back to the raw path
/// when it doesn't resolve (deleted checkout, not-yet-created dir). macOS
/// symlinks (`/tmp` → `/private/tmp`) otherwise break `starts_with`.
fn canonical_or_raw(path: &Path) -> std::path::PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

/// Does this terminal's shell have any child processes (a command or job
/// still running)? An unknown child pid or a failed probe counts as busy —
/// never kill what can't be inspected.
fn shell_has_children(session: &PtySession) -> bool {
    let Some(pid) = session.child_pid else {
        return true;
    };
    !matches!(
        std::process::Command::new("pgrep")
            .arg("-P")
            .arg(pid.to_string())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status(),
        Ok(status) if !status.success()
    )
}

/// Canonical form of a user-typed link. Pasting a URL out of a browser is
/// the common case, but people also type `github.com/o/r/pull/7`, so a
/// scheme-less value gets https://. Anything else — another scheme, or no
/// host at all — is refused rather than stored: the TUI hands these to
/// `open(1)`, and only http(s) may ever reach it.
pub(crate) fn normalize_url(url: &str) -> Result<String> {
    let url = url.trim();
    if url.is_empty() {
        bail!("link URL is empty");
    }
    if url.contains(char::is_whitespace) {
        bail!("link URL contains whitespace");
    }
    let normalized = match url.split_once("://") {
        Some(("http" | "https", _)) => url.to_string(),
        Some((scheme, _)) => bail!("only http(s) links are supported (got {scheme}://)"),
        // Scheme-less: a bare host is a URL people type; a bare word is not.
        None => {
            let host = url.split(['/', '?', '#']).next().unwrap_or_default();
            if !host.contains('.') || host.starts_with('.') || host.ends_with('.') {
                bail!("not a URL: {url}");
            }
            format!("https://{url}")
        }
    };
    // Reject "https://" and friends: a scheme with nothing behind it.
    if normalized
        .split_once("://")
        .is_none_or(|(_, rest)| rest.is_empty())
    {
        bail!("not a URL: {url}");
    }
    Ok(normalized)
}

fn user_shell() -> String {
    std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".into())
}

/// Why a create was refused when the agent CLI isn't installed. One line —
/// the TUI shows it in the footer flash, which truncates. Unlike git (which
/// the daemon runs with its own inherited PATH), agent CLIs are spawned
/// through the user's login shell, so a fresh install is picked up on the
/// next try with no daemon restart.
fn cli_missing_message(kind: AgentKind) -> String {
    format!(
        "{} was not found on your PATH — install it, then try again.",
        kind.cli_program()
    )
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
    let args = LOGIN_SHELL_ARGS
        .iter()
        .map(|s| s.to_string())
        .chain([cmdline])
        .collect();
    (shell.to_string(), args)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Claude argv: `args`, then nebula's appended guidance (worktree and
    /// spawn, one `--append-system-prompt`).
    fn guided(args: &[&str]) -> Vec<String> {
        args.iter()
            .map(|s| s.to_string())
            .chain([
                "--append-system-prompt".to_string(),
                [
                    CLAUDE_WORKTREE_GUIDANCE,
                    crate::sibling::CLAUDE_SPAWN_GUIDANCE,
                    crate::open_files::CLAUDE_OPEN_GUIDANCE,
                ]
                .join("\n\n"),
            ])
            .collect()
    }

    #[test]
    fn spawn_command_per_kind_resume_shapes() {
        // Fresh sessions: bare CLI (Claude plus its system-prompt guidance).
        assert_eq!(
            agent_spawn_command(AgentKind::Claude, None, None, None, None),
            ("claude".into(), guided(&[]), false)
        );
        // Codex/cursor always run in skip-permissions mode.
        assert_eq!(
            agent_spawn_command(AgentKind::Codex, None, None, None, None),
            ("codex".into(), vec!["--yolo".to_string()], false)
        );
        // Cursor's agent CLI is `cursor-agent`, not `cursor` (the editor).
        assert_eq!(
            agent_spawn_command(AgentKind::Cursor, None, None, None, None),
            ("cursor-agent".into(), vec!["--force".to_string()], false)
        );
        // Pi has no permission gate to skip and takes the same guidance as
        // Claude (it has the system-prompt flag).
        assert_eq!(
            agent_spawn_command(AgentKind::Pi, None, None, None, None),
            ("pi".into(), guided(&[]), false)
        );
        // Pi resumes by exact id — one that is missing is created, so a
        // relocated session's new cwd never dies on a stale id.
        assert_eq!(
            agent_spawn_command(AgentKind::Pi, Some("sid-4"), None, None, None),
            ("pi".into(), guided(&["--session-id", "sid-4"]), true)
        );
        // Claude resumes with a flag; codex with a subcommand (order matters).
        assert_eq!(
            agent_spawn_command(AgentKind::Claude, Some("sid-1"), None, None, None),
            ("claude".into(), guided(&["--resume", "sid-1"]), true)
        );
        // Skip-permissions flags trail the resume args.
        assert_eq!(
            agent_spawn_command(AgentKind::Codex, Some("sid-2"), None, None, None),
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
            agent_spawn_command(AgentKind::Cursor, Some("sid-3"), None, None, None),
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
            agent_spawn_command(
                AgentKind::Claude,
                Some("sid"),
                None,
                None,
                Some("/bin/sh -i")
            ),
            ("/bin/sh".into(), vec!["-i".to_string()], false)
        );
        assert_eq!(
            agent_spawn_command(AgentKind::Codex, Some("sid"), None, None, Some("/bin/sh")),
            ("/bin/sh".into(), vec![], false)
        );
    }

    #[test]
    fn spawn_command_model_and_effort_flags() {
        // Claude gets --model/--effort; either alone works.
        assert_eq!(
            agent_spawn_command(AgentKind::Claude, None, Some("opus"), Some("high"), None),
            (
                "claude".into(),
                guided(&["--model", "opus", "--effort", "high"]),
                false
            )
        );
        assert_eq!(
            agent_spawn_command(AgentKind::Claude, None, None, Some("max"), None),
            ("claude".into(), guided(&["--effort", "max"]), false)
        );
        // Codex takes --model plus a config override for effort, after --yolo.
        assert_eq!(
            agent_spawn_command(AgentKind::Codex, None, Some("gpt-5.5"), Some("high"), None),
            (
                "codex".into(),
                vec![
                    "--yolo".to_string(),
                    "--model".to_string(),
                    "gpt-5.5".to_string(),
                    "-c".to_string(),
                    "model_reasoning_effort=high".to_string()
                ],
                false
            )
        );
        // Pi: `--model <pattern>` plus `--thinking <level>`, ahead of the
        // guidance like Claude's.
        assert_eq!(
            agent_spawn_command(AgentKind::Pi, None, Some("sonnet"), Some("high"), None),
            (
                "pi".into(),
                guided(&["--model", "sonnet", "--thinking", "high"]),
                false
            )
        );
        assert_eq!(
            agent_spawn_command(AgentKind::Pi, None, None, Some("off"), None),
            ("pi".into(), guided(&["--thinking", "off"]), false)
        );
        // Resume keeps the model/effort flags (a fallback fresh spawn needs
        // them, and the CLIs accept them alongside resume).
        assert_eq!(
            agent_spawn_command(AgentKind::Claude, Some("sid"), Some("sonnet"), None, None),
            (
                "claude".into(),
                guided(&["--resume", "sid", "--model", "sonnet"]),
                true
            )
        );
        // Cursor joins family and effort into the CLI's one flat id; a
        // family alone is passed bare, an effort alone has nothing to
        // join and is dropped.
        assert_eq!(
            agent_spawn_command(
                AgentKind::Cursor,
                None,
                Some("claude-opus-5-thinking"),
                Some("high"),
                None
            ),
            (
                "cursor-agent".into(),
                vec![
                    "--force".to_string(),
                    "--model".to_string(),
                    "claude-opus-5-thinking-high".to_string()
                ],
                false
            )
        );
        assert_eq!(
            agent_spawn_command(AgentKind::Cursor, Some("sid"), Some("auto"), None, None),
            (
                "cursor-agent".into(),
                vec![
                    "--resume".to_string(),
                    "sid".to_string(),
                    "--force".to_string(),
                    "--model".to_string(),
                    "auto".to_string()
                ],
                true
            )
        );
        assert_eq!(
            agent_spawn_command(AgentKind::Cursor, None, None, Some("high"), None),
            ("cursor-agent".into(), vec!["--force".to_string()], false)
        );
        // Override still wins over everything.
        assert_eq!(
            agent_spawn_command(AgentKind::Claude, None, Some("opus"), None, Some("/bin/sh")),
            ("/bin/sh".into(), vec![], false)
        );
    }

    #[test]
    fn spawn_command_initial_prompt_is_the_trailing_positional_argument() {
        // The relocation notice trails everything, guidance included.
        let (_, args, resumed) = agent_spawn_command_with(
            AgentKind::Claude,
            Some("sid"),
            Some("opus"),
            None,
            None,
            Some("carry on"),
            None,
            true,
        );
        assert!(resumed);
        let mut expected = guided(&["--resume", "sid", "--model", "opus"]);
        expected.push("carry on".into());
        assert_eq!(args, expected);
        // Codex and cursor take it as their trailing positional too.
        assert_eq!(
            agent_spawn_command_with(
                AgentKind::Codex,
                Some("sid"),
                None,
                None,
                None,
                Some("carry on"),
                None,
                true
            )
            .1,
            vec!["resume", "sid", "--yolo", "carry on"]
        );
        assert_eq!(
            agent_spawn_command_with(
                AgentKind::Cursor,
                Some("sid"),
                None,
                None,
                None,
                Some("carry on"),
                None,
                true
            )
            .1,
            vec!["--resume", "sid", "--force", "carry on"]
        );
        // A fresh spawn with a starting prompt (an AGENT PRESET launch):
        // model, effort and system prompt all precede it.
        let mut expected = guided(&["--model", "opus", "--effort", "high"]);
        expected.push("fix auth".into());
        assert_eq!(
            agent_spawn_command_with(
                AgentKind::Claude,
                None,
                Some("opus"),
                Some("high"),
                None,
                Some("fix auth"),
                None,
                true
            )
            .1,
            expected
        );
        assert_eq!(
            agent_spawn_command_with(
                AgentKind::Codex,
                None,
                Some("gpt-5.5"),
                Some("high"),
                None,
                Some("fix auth"),
                None,
                true
            )
            .1,
            vec![
                "--yolo",
                "--model",
                "gpt-5.5",
                "-c",
                "model_reasoning_effort=high",
                "fix auth"
            ]
        );
        assert_eq!(
            agent_spawn_command_with(
                AgentKind::Cursor,
                None,
                None,
                None,
                None,
                Some("fix auth"),
                None,
                true
            )
            .1,
            vec!["--force", "fix auth"]
        );
        // Pi: the prompt trails the resume id and the guidance, as pi's
        // `[messages...]` positional.
        let mut expected = guided(&["--session-id", "sid"]);
        expected.push("carry on".into());
        assert_eq!(
            agent_spawn_command_with(
                AgentKind::Pi,
                Some("sid"),
                None,
                None,
                None,
                Some("carry on"),
                None,
                true
            )
            .1,
            expected
        );
        // An override is verbatim: no guidance, no prompt.
        assert_eq!(
            agent_spawn_command_with(
                AgentKind::Claude,
                None,
                None,
                None,
                Some("/bin/sh -i"),
                Some("carry on"),
                None,
                true
            ),
            ("/bin/sh".into(), vec!["-i".to_string()], false)
        );
    }

    #[test]
    fn spawn_command_keeps_pr_scope_and_url_in_claudes_system_prompt() {
        let pr_url = "https://github.com/AgentSystemLabs/nebula/pull/42";
        let pr_prompt = crate::pr_scope::rule(pr_url);
        let (_, args, resumed) = agent_spawn_command_with(
            AgentKind::Claude,
            Some("sid"),
            None,
            None,
            None,
            None,
            Some(&pr_prompt),
            true,
        );
        assert!(resumed);
        let prompts = args
            .windows(2)
            .filter(|pair| pair[0] == "--append-system-prompt")
            .map(|pair| pair[1].as_str())
            .collect::<Vec<_>>();
        assert_eq!(prompts.len(), 1, "Claude gets one composed system prompt");
        assert!(prompts[0].contains(CLAUDE_WORKTREE_GUIDANCE));
        assert!(prompts[0].contains(crate::sibling::CLAUDE_SPAWN_GUIDANCE));
        assert!(prompts[0].contains(crate::open_files::CLAUDE_OPEN_GUIDANCE));
        assert!(prompts[0].contains("All work in this session must be scoped"));
        assert!(prompts[0].contains(pr_url));
    }

    #[test]
    fn spawn_command_claude_cloud_passes_the_task_as_one_argument() {
        assert_eq!(
            claude_cloud_spawn_command(
                CloudLaunch::Create("Fix auth\nRun tests; don't stop"),
                Some("opus"),
                Some("high"),
                None,
            ),
            (
                "claude".into(),
                vec![
                    "--cloud=Fix auth\nRun tests; don't stop".to_string(),
                    "--model".to_string(),
                    "opus".to_string(),
                    "--effort".to_string(),
                    "high".to_string(),
                ],
                false,
            )
        );
        assert_eq!(
            claude_cloud_spawn_command(
                CloudLaunch::Create("--dangerously-skip-permissions"),
                None,
                None,
                None
            )
            .1,
            vec!["--cloud=--dangerously-skip-permissions"]
        );
    }

    #[test]
    fn spawn_command_cloud_attach_and_teleport_bind_the_id() {
        let id = "session_016SiQW5Lem2LbnUf1A3undt";
        assert_eq!(
            claude_cloud_spawn_command(CloudLaunch::Attach(id), None, None, None),
            ("claude".into(), vec![format!("--cloud={id}")], false)
        );
        assert_eq!(
            claude_cloud_spawn_command(CloudLaunch::Teleport(id), Some("opus"), None, None),
            (
                "claude".into(),
                vec![format!("--teleport={id}"), "--model".into(), "opus".into()],
                false
            )
        );
        // Overrides (tests) stay verbatim — no cloud flag at all.
        assert_eq!(
            claude_cloud_spawn_command(CloudLaunch::Attach(id), None, None, Some("/bin/true")).1,
            Vec::<String>::new()
        );
    }

    #[test]
    fn cloud_worktree_branch_is_short_and_git_safe() {
        assert_eq!(
            cloud_worktree_branch("session_016SiQW5Lem2LbnUf1A3undt"),
            "cloud-f1A3undt"
        );
        assert_eq!(cloud_worktree_branch("session_ab"), "cloud-ab");
        assert_eq!(cloud_worktree_branch("session_"), "cloud-");
    }

    #[tokio::test]
    async fn attach_cloud_agent_requires_a_cloud_session() {
        let daemon = test_daemon();
        seed_projects(&daemon, &["p"]);
        seed_worktree(&daemon, "p", "w", "/tmp", true);
        seed_agent(&daemon, "local", "w", None);
        let err = daemon
            .attach_cloud_agent(&AgentId("local".into()))
            .await
            .unwrap_err();
        assert!(
            err.to_string().contains("not launched in Claude Cloud"),
            "{err}"
        );
    }

    #[test]
    fn cloud_reentry_tries_attach_once_then_teleports() {
        let id = "session_abc";
        assert_eq!(cloud_reentry_launch(id, false), CloudLaunch::Attach(id));
        // Once the account has refused, every later re-entry teleports —
        // retrying the attach only reprints the refusal in the user's pane.
        assert_eq!(cloud_reentry_launch(id, true), CloudLaunch::Teleport(id));
    }

    #[test]
    fn cloud_text_is_trimmed_and_bounded() {
        assert_eq!(
            validate_cloud_text("  fix auth  ", "task").unwrap(),
            "fix auth"
        );
        // Newlines are part of a multi-row task/message; only the ends go.
        assert_eq!(
            validate_cloud_text("\nline one\nline two\n", "message").unwrap(),
            "line one\nline two"
        );
        for bad in ["", "   ", "\n"] {
            assert!(validate_cloud_text(bad, "task").is_err(), "{bad:?}");
        }
        // A NUL would truncate the login shell's -c string.
        assert!(validate_cloud_text("fix\0auth", "task").is_err());
        assert!(validate_cloud_text(&"x".repeat(MAX_CLOUD_PROMPT_BYTES), "task").is_ok());
        assert!(validate_cloud_text(&"x".repeat(MAX_CLOUD_PROMPT_BYTES + 1), "task").is_err());
        // The label rides into the message the user sees.
        let err = validate_cloud_text("", "message").unwrap_err().to_string();
        assert!(err.contains("message"), "{err}");
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
        let store = Arc::new(Store::open_in_memory().unwrap());
        Daemon::new(
            store,
            HookEnv {
                port: 0,
                token: String::new(),
            },
        )
    }

    #[tokio::test]
    async fn cloud_create_validates_tasks_and_rejects_non_claude_kinds() {
        let daemon = test_daemon();
        let worktree = WorktreeId("unused".into());

        let empty = daemon
            .create_agent(CreateAgentSpec {
                worktree: worktree.clone(),
                name: "cloud".into(),
                kind: AgentKind::Claude,
                model: None,
                effort: None,
                auto_title: false,
                cloud_prompt: Some(" \n ".into()),
                starting_prompt: None,
                pr_url: None,
            })
            .await
            .unwrap_err();
        assert!(empty.to_string().contains("needs a task"));

        let nul = daemon
            .create_agent(CreateAgentSpec {
                worktree: worktree.clone(),
                name: "cloud".into(),
                kind: AgentKind::Claude,
                model: None,
                effort: None,
                auto_title: false,
                cloud_prompt: Some("fix\0auth".into()),
                starting_prompt: None,
                pr_url: None,
            })
            .await
            .unwrap_err();
        assert!(nul.to_string().contains("NUL"));

        let too_long = daemon
            .create_agent(CreateAgentSpec {
                worktree: worktree.clone(),
                name: "cloud".into(),
                kind: AgentKind::Claude,
                model: None,
                effort: None,
                auto_title: false,
                cloud_prompt: Some("x".repeat(MAX_CLOUD_PROMPT_BYTES + 1)),
                starting_prompt: None,
                pr_url: None,
            })
            .await
            .unwrap_err();
        assert!(too_long.to_string().contains("too long"));

        let wrong_kind = daemon
            .create_agent(CreateAgentSpec {
                worktree,
                name: "cloud".into(),
                kind: AgentKind::Codex,
                model: None,
                effort: None,
                auto_title: false,
                cloud_prompt: Some("Fix auth".into()),
                starting_prompt: None,
                pr_url: None,
            })
            .await
            .unwrap_err();
        assert!(wrong_kind.to_string().contains("only supported for Claude"));
    }

    #[tokio::test]
    async fn pr_launch_context_is_accepted_for_every_kind_but_never_with_cloud() {
        let daemon = test_daemon();
        let spec = |kind: AgentKind, cloud: Option<&str>| CreateAgentSpec {
            worktree: WorktreeId("unused".into()),
            name: "pr".into(),
            kind,
            model: None,
            effort: None,
            auto_title: false,
            cloud_prompt: cloud.map(String::from),
            starting_prompt: None,
            pr_url: Some("https://github.com/o/r/pull/7".into()),
        };
        for kind in AgentKind::ALL {
            // Validation passes for every harness; the missing worktree is
            // what stops this spec, one check later.
            let err = daemon.create_agent(spec(kind, None)).await.unwrap_err();
            assert!(
                err.to_string().contains("worktree not found"),
                "{kind:?}: {err}"
            );
        }
        let cloud = daemon
            .create_agent(spec(AgentKind::Claude, Some("Fix auth")))
            .await
            .unwrap_err();
        assert!(cloud.to_string().contains("not supported for Claude Cloud"));
        let not_a_pr = CreateAgentSpec {
            pr_url: Some("https://github.com/o/r/issues/7".into()),
            ..spec(AgentKind::Codex, None)
        };
        let err = daemon.create_agent(not_a_pr).await.unwrap_err();
        assert!(err.to_string().contains("not a pull request URL"), "{err}");
    }

    #[tokio::test]
    async fn starting_prompt_is_validated_and_never_adopts_a_warm_cli() {
        let daemon = test_daemon();
        let spec = |kind: AgentKind, cloud: Option<&str>, starting: Option<&str>| CreateAgentSpec {
            worktree: WorktreeId("unused".into()),
            name: "preset".into(),
            kind,
            model: None,
            effort: None,
            auto_title: true,
            cloud_prompt: cloud.map(String::from),
            starting_prompt: starting.map(String::from),
            pr_url: None,
        };
        // Validation runs before the worktree lookup, so an unknown
        // worktree is fine here and every failure is the prompt's own.
        for (kind, starting, needle) in [
            (AgentKind::Claude, " \n ", "is empty"),
            (AgentKind::Codex, "fix\0auth", "NUL"),
            (
                AgentKind::Cursor,
                &*"x".repeat(MAX_CLOUD_PROMPT_BYTES + 1),
                "too long",
            ),
        ] {
            let err = daemon
                .create_agent(spec(kind, None, Some(starting)))
                .await
                .unwrap_err();
            assert!(err.to_string().contains(needle), "{kind:?}: {err}");
        }
        let with_cloud = daemon
            .create_agent(spec(AgentKind::Claude, Some("Fix auth"), Some("Fix auth")))
            .await
            .unwrap_err();
        assert!(with_cloud
            .to_string()
            .contains("not supported for Claude Cloud"));

        // A starting prompt rides the CLI's argv, so a warm spare (booted
        // bare) is never adopted: the pool entry survives the create.
        seed_projects(&daemon, &["p"]);
        seed_worktree(&daemon, "p", "w1", "/tmp", true);
        let key = (WorktreeId("w1".into()), AgentKind::Claude);
        daemon.prewarmed.lock().unwrap().insert(
            key.clone(),
            PrewarmEntry {
                agent_id: AgentId("warm-1".into()),
                spawned_at: Instant::now(),
                model: None,
                effort: None,
                buffered_hooks: Vec::new(),
            },
        );
        let mut preset = spec(AgentKind::Claude, None, Some("Fix auth"));
        preset.worktree = WorktreeId("w1".into());
        // Without a CLI on this box the cold path fails at the probe; the
        // result is beside the point — adoption would have emptied the pool.
        let _ = daemon.create_agent(preset).await;
        assert!(
            daemon.prewarmed.lock().unwrap().contains_key(&key),
            "a starting-prompt create must not adopt the warm spare"
        );
    }

    #[test]
    fn failed_agent_spawn_rolls_back_the_persisted_row() {
        let daemon = test_daemon();
        seed_projects(&daemon, &["p"]);
        seed_worktree(&daemon, "p", "w", "/tmp", true);
        seed_agent(&daemon, "cloud", "w", None);
        let id = AgentId("cloud".into());

        let error = daemon
            .rollback_agent_on_spawn_error(&id, Err::<(), _>(anyhow::anyhow!("spawn failed")))
            .unwrap_err();

        assert!(error.to_string().contains("spawn failed"));
        assert!(daemon.store.get_agent(&id).unwrap().is_none());
    }

    fn seed_projects(daemon: &Daemon, names: &[&str]) {
        for (i, name) in names.iter().enumerate() {
            daemon
                .store
                .insert_project(&Project {
                    workspace_id: Default::default(),
                    id: ProjectId((*name).into()),
                    name: (*name).into(),
                    repo_path: format!("/tmp/{name}").into(),
                    sort_order: i as i64,
                })
                .unwrap();
        }
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
                archived_at: 0,
                unseen: false,
                kind: AgentKind::Claude,
                model: None,
                effort: None,
                session_id: session_id.map(str::to_string),
                cloud_session_id: None,
                sort_order: 0,
                status_changed_at: 0,
                alive: false,
                cloud_mirroring: false,
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

    #[tokio::test]
    async fn enter_worktree_takes_an_existing_branch_and_moves_the_row_now() {
        let daemon = test_daemon();
        seed_projects(&daemon, &["p"]);
        seed_worktree(&daemon, "p", "root", "/nebula-test/p", true);
        seed_worktree(&daemon, "p", "feat", "/nebula-test/p-feat", false);
        seed_agent(&daemon, "a1", "root", Some("s1"));
        let a1 = AgentId("a1".into());
        let mut rx = daemon.events.subscribe();

        let (target, outcome) = daemon.enter_worktree(&a1, "feat", None).await.unwrap();
        assert_eq!(target.id.to_string(), "feat");
        // No PTY runs here, so nothing waits on a turn end.
        assert_eq!(outcome, EnterOutcome::NextLaunch);
        assert!(!daemon.relocation_pending(&a1));
        assert_eq!(agent_worktree(&daemon, "a1"), "feat");
        match rx.try_recv().unwrap() {
            ServerEvent::EntityUpserted {
                entity: Entity::Agent(a),
            } => assert_eq!(a.worktree_id.to_string(), "feat"),
            other => panic!("expected agent upsert, got {other:?}"),
        }

        // Already there: a settled answer, no broadcast.
        let (again, outcome) = daemon.enter_worktree(&a1, "feat", None).await.unwrap();
        assert_eq!(again.id, target.id);
        assert_eq!(outcome, EnterOutcome::AlreadyThere);
        assert!(rx.try_recv().is_err(), "no broadcast for a no-op enter");

        // Blank names are refused before anything is touched.
        assert!(daemon.enter_worktree(&a1, "  ", None).await.is_err());
    }

    /// Between `nebula worktree` and the turn's Stop the row already sits
    /// under the target while the process still reports the old checkout:
    /// that cwd must not drag it back, and only a turn-end hook drains the
    /// pending relocation.
    #[test]
    fn pending_relocation_ignores_the_old_cwd_until_the_turn_ends() {
        let daemon = test_daemon();
        seed_projects(&daemon, &["p"]);
        seed_worktree(&daemon, "p", "root", "/nebula-test/p", true);
        seed_worktree(&daemon, "p", "feat", "/nebula-test/p-feat", false);
        seed_agent(&daemon, "a1", "feat", Some("s1"));
        let a1 = AgentId("a1".into());
        let feat = daemon
            .store
            .get_worktree(&WorktreeId("feat".into()))
            .unwrap()
            .unwrap();
        daemon
            .pending_moves
            .lock()
            .unwrap()
            .insert(a1.clone(), feat);

        daemon.reparent_agent_by_cwd(&a1, "/nebula-test/p", Some("s1"), false);
        assert_eq!(
            agent_worktree(&daemon, "a1"),
            "feat",
            "the old checkout's cwd is ignored mid-relocation"
        );

        daemon.complete_pending_move(
            &a1,
            &HookEvent::PostToolUse {
                tool_name: Some("Bash".into()),
                subagent_id: None,
            },
        );
        assert!(
            daemon.relocation_pending(&a1),
            "a tool hook is not a turn end"
        );
        daemon.complete_pending_move(&a1, &HookEvent::Stop);
        assert!(!daemon.relocation_pending(&a1));

        // Drained, the reparent is live again.
        daemon.reparent_agent_by_cwd(&a1, "/nebula-test/p", Some("s1"), false);
        assert_eq!(agent_worktree(&daemon, "a1"), "root");
    }

    #[tokio::test]
    async fn enter_worktree_creates_the_checkout_in_nebulas_layout() {
        let tmp = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(tmp.path()).unwrap();
        let repo = root.join("repo");
        std::fs::create_dir(&repo).unwrap();
        git_in(&repo, &["init", "-b", "main"]);
        git_in(&repo, &["commit", "--allow-empty", "-m", "init"]);
        let daemon = test_daemon();
        daemon
            .store
            .insert_project(&Project {
                workspace_id: Default::default(),
                id: ProjectId("p".into()),
                name: "p".into(),
                repo_path: repo.clone(),
                sort_order: 0,
            })
            .unwrap();
        seed_worktree(&daemon, "p", "root", &repo.to_string_lossy(), true);
        seed_agent(&daemon, "a1", "root", Some("s1"));
        let a1 = AgentId("a1".into());
        let mut rx = daemon.events.subscribe();

        let (target, _) = daemon.enter_worktree(&a1, "feat", None).await.unwrap();
        assert_eq!(target.branch, "feat");
        assert_eq!(target.path, root.join("repo-worktrees").join("feat"));
        assert!(target.path.join(".git").exists(), "a real checkout");
        assert_eq!(agent_worktree(&daemon, "a1"), target.id.to_string());
        // The worktree's upsert lands first, then the agent's.
        assert!(matches!(
            rx.try_recv().unwrap(),
            ServerEvent::EntityUpserted { entity: Entity::Worktree(w) } if w.id == target.id
        ));
        assert!(matches!(
            rx.try_recv().unwrap(),
            ServerEvent::EntityUpserted { entity: Entity::Agent(a) } if a.worktree_id == target.id
        ));
    }

    fn seed_pending_agent(daemon: &Daemon, id: &str, worktree: &str) {
        daemon
            .store
            .insert_agent_with_auto_title(
                &Agent {
                    id: AgentId(id.into()),
                    worktree_id: WorktreeId(worktree.into()),
                    name: format!("{id}-default"),
                    status: AgentStatus::Fresh,
                    archived: false,
                    archived_at: 0,
                    unseen: false,
                    kind: AgentKind::Claude,
                    model: None,
                    effort: None,
                    session_id: None,
                    cloud_session_id: None,
                    sort_order: 0,
                    status_changed_at: 0,
                    alive: false,
                    cloud_mirroring: false,
                },
                true,
            )
            .unwrap();
    }

    #[test]
    fn auto_rename_applies_once_and_defers_to_user_titles() {
        let daemon = test_daemon();
        seed_projects(&daemon, &["p"]);
        seed_worktree(&daemon, "p", "root", "/nebula-test/p", true);
        seed_pending_agent(&daemon, "a1", "root");
        let mut rx = daemon.events.subscribe();

        // First agent attempt lands, sanitized, and is broadcast.
        daemon
            .auto_rename_agent(&AgentId("a1".into()), "  Fix   Login\tRedirect  ")
            .unwrap();
        match rx.try_recv().unwrap() {
            ServerEvent::EntityUpserted {
                entity: Entity::Agent(a),
            } => assert_eq!(a.name, "Fix Login Redirect"),
            other => panic!("expected agent upsert, got {other:?}"),
        }

        // A second attempt is declined with a settled, informative error.
        let err = daemon
            .auto_rename_agent(&AgentId("a1".into()), "Another Title")
            .unwrap_err();
        assert!(err.to_string().contains("already has a title"), "{err}");
        assert_eq!(
            daemon
                .store
                .get_agent(&AgentId("a1".into()))
                .unwrap()
                .unwrap()
                .name,
            "Fix Login Redirect"
        );

        // A user rename beats a pending auto-title: the CLI's later attempt
        // must not clobber it.
        seed_pending_agent(&daemon, "a2", "root");
        daemon
            .rename_agent(&AgentId("a2".into()), "my session")
            .unwrap();
        let err = daemon
            .auto_rename_agent(&AgentId("a2".into()), "Model Title")
            .unwrap_err();
        assert!(err.to_string().contains("already has a title"), "{err}");

        // Garbage titles are rejected outright.
        assert!(daemon
            .auto_rename_agent(&AgentId("a1".into()), " \u{7}\n ")
            .is_err());
        // Unknown agents report cleanly.
        let err = daemon
            .auto_rename_agent(&AgentId("ghost".into()), "Some Title")
            .unwrap_err();
        assert!(err.to_string().contains("agent not found"), "{err}");
    }

    #[test]
    fn sanitize_title_collapses_and_caps() {
        assert_eq!(
            sanitize_title(" Fix   Login\u{7}Redirect \n"),
            "Fix Login Redirect"
        );
        assert_eq!(sanitize_title("\u{1b}[31m"), "[31m");
        assert_eq!(sanitize_title("   "), "");
        let long = "word ".repeat(30);
        assert!(sanitize_title(&long).chars().count() <= 60);
        assert!(!sanitize_title(&long).ends_with(' '));
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

    /// Regression: a session that creates a worktree and steps into it
    /// reports the new cwd *before* the sync has adopted a row for it (the
    /// `Stop` hook fires long before the next 2s sync tick). The cwd must be
    /// remembered and replayed on adoption, or the row sits under the old
    /// checkout until the user's next prompt.
    #[tokio::test]
    async fn worktree_sync_replays_a_cwd_reported_before_adoption() {
        let tmp = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(tmp.path()).unwrap();
        let repo = root.join("repo");
        std::fs::create_dir(&repo).unwrap();
        git_in(&repo, &["init", "-b", "main"]);
        git_in(&repo, &["commit", "--allow-empty", "-m", "init"]);

        let daemon = test_daemon();
        let project = Project {
            workspace_id: Default::default(),
            id: ProjectId("p".into()),
            name: "p".into(),
            repo_path: repo.clone(),
            sort_order: 0,
        };
        daemon.store.insert_project(&project).unwrap();
        seed_worktree(&daemon, "p", "root", &repo.to_string_lossy(), true);
        seed_agent(&daemon, "a1", "root", Some("s1"));

        // The agent creates a sibling worktree and walks into it. The hook
        // lands first: no row exists yet, so nothing moves.
        let feat = root.join("repo-worktrees").join("feat");
        git_in(
            &repo,
            &["worktree", "add", &feat.to_string_lossy(), "-b", "feat"],
        );
        daemon.reparent_agent_by_cwd(
            &AgentId("a1".into()),
            &feat.to_string_lossy(),
            Some("s1"),
            false,
        );
        assert_eq!(agent_worktree(&daemon, "a1"), "root");

        // The sync adopts the checkout and replays the remembered cwd.
        daemon.sync_project_worktrees(&project).await.unwrap();
        let (_, worktrees, _, _) = daemon.store.load_tree().unwrap();
        let adopted = worktrees
            .iter()
            .find(|w| w.branch == "feat")
            .expect("feat worktree adopted");
        assert_eq!(agent_worktree(&daemon, "a1"), adopted.id.to_string());

        // A deliberate move back must survive the next adoption: the move
        // drops the remembered cwd, so replaying it can't overrule the user.
        daemon
            .move_agent(&AgentId("a1".into()), &WorktreeId("root".into()))
            .unwrap();
        let other = root.join("repo-worktrees").join("other");
        git_in(
            &repo,
            &["worktree", "add", &other.to_string_lossy(), "-b", "other"],
        );
        daemon.sync_project_worktrees(&project).await.unwrap();
        assert_eq!(agent_worktree(&daemon, "a1"), "root");
    }

    /// The replay is scoped to the synced project and skips archived rows.
    #[test]
    fn cwd_replay_skips_other_projects_and_archived_agents() {
        let daemon = test_daemon();
        seed_projects(&daemon, &["p", "q"]);
        seed_worktree(&daemon, "p", "p-root", "/nebula-test/p", true);
        seed_worktree(&daemon, "q", "q-root", "/nebula-test/q", true);
        seed_worktree(&daemon, "q", "q-feat", "/nebula-test/q-feat", false);
        seed_agent(&daemon, "a1", "q-root", None);
        seed_agent(&daemon, "a2", "q-root", None);

        // Both agents report a cwd inside q-feat before it exists...
        daemon
            .store
            .delete_worktree(&WorktreeId("q-feat".into()))
            .unwrap();
        daemon.reparent_agent_by_cwd(&AgentId("a1".into()), "/nebula-test/q-feat", None, false);
        daemon.reparent_agent_by_cwd(&AgentId("a2".into()), "/nebula-test/q-feat", None, false);
        seed_worktree(&daemon, "q", "q-feat", "/nebula-test/q-feat", false);

        // ...but a replay for project p touches neither.
        let p = daemon
            .store
            .get_project(&ProjectId("p".into()))
            .unwrap()
            .unwrap();
        daemon.reparent_agents_by_last_cwd(&p);
        assert_eq!(agent_worktree(&daemon, "a1"), "q-root");

        // Archived agents stay put; live ones re-home.
        daemon
            .store
            .set_agent_archived(&AgentId("a2".into()), true)
            .unwrap();
        let q = daemon
            .store
            .get_project(&ProjectId("q".into()))
            .unwrap()
            .unwrap();
        daemon.reparent_agents_by_last_cwd(&q);
        assert_eq!(agent_worktree(&daemon, "a1"), "q-feat");
        assert_eq!(agent_worktree(&daemon, "a2"), "q-root");
    }

    /// Renaming a project relabels its row and nothing else: the checkout on
    /// disk keeps its name, `repo_path` keeps pointing at it, and an empty
    /// name puts the row back on the folder's own name — the only way back
    /// once a project has been renamed.
    #[tokio::test]
    async fn rename_project_relabels_the_row_and_leaves_the_folder_alone() {
        let tmp = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(tmp.path()).unwrap();
        let repo = root.join("acme-api");
        std::fs::create_dir(&repo).unwrap();
        git_in(&repo, &["init", "-b", "main"]);
        git_in(&repo, &["commit", "--allow-empty", "-m", "init"]);

        let daemon = test_daemon();
        let id = match daemon.add_project(&repo, None, false, None).await.unwrap() {
            EntityId::Project(id) => id,
            other => panic!("expected a project id, got {other:?}"),
        };
        let named = |daemon: &Arc<Daemon>| daemon.store.get_project(&id).unwrap().unwrap();
        assert_eq!(named(&daemon).name, "acme-api", "named after the folder");

        daemon.rename_project(&id, "  Acme API  ").unwrap();
        let project = named(&daemon);
        assert_eq!(project.name, "Acme API", "trimmed and stored");
        assert_eq!(project.repo_path, repo, "the folder is untouched");
        assert!(repo.exists(), "and still on disk under its own name");
        assert_eq!(
            project.folder_subtitle().as_deref(),
            Some("acme-api"),
            "a renamed row still shows where it lives"
        );

        daemon.rename_project(&id, "   ").unwrap();
        let project = named(&daemon);
        assert_eq!(project.name, "acme-api", "empty resets to the folder name");
        assert_eq!(project.folder_subtitle(), None, "nothing left to show");
    }

    /// `git rev-parse --show-toplevel` answers with the checkout it ran in, so
    /// `nebula add .` from inside a linked worktree used to make the worktree
    /// the project: named after the branch directory, `repo_path` pointing at
    /// it, and a ⌂ root row for a directory the project did not own. The repo
    /// is the project no matter which of its checkouts you add it from.
    #[tokio::test]
    async fn add_project_from_inside_a_worktree_roots_at_the_repo() {
        let tmp = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(tmp.path()).unwrap();
        let repo = root.join("repo");
        std::fs::create_dir(&repo).unwrap();
        git_in(&repo, &["init", "-b", "main"]);
        git_in(&repo, &["commit", "--allow-empty", "-m", "init"]);
        let feat = root.join("repo-worktrees").join("gentle-narwhal-files");
        git_in(
            &repo,
            &[
                "worktree",
                "add",
                &feat.to_string_lossy(),
                "-b",
                "gentle-narwhal-files",
            ],
        );

        let daemon = test_daemon();
        daemon.add_project(&feat, None, false, None).await.unwrap();

        let (projects, worktrees, _, _) = daemon.store.load_tree().unwrap();
        let project = projects.first().expect("project added");
        assert_eq!(project.repo_path, repo, "project is rooted at the repo");
        assert_eq!(project.name, "repo", "named after the repo, not the branch");

        let main: Vec<&Worktree> = worktrees.iter().filter(|w| w.is_main).collect();
        assert_eq!(main.len(), 1, "exactly one root row: {worktrees:#?}");
        assert_eq!(
            main[0].path, repo,
            "the ⌂ root row is the project's own dir"
        );
        assert_eq!(main[0].branch, "main");
        let linked = worktrees
            .iter()
            .find(|w| !w.is_main)
            .expect("the worktree we added from is a plain row");
        assert_eq!(linked.path, feat);
        assert_eq!(linked.branch, "gentle-narwhal-files");
    }

    /// Adding the repo from a worktree of one already in the workspace is the
    /// same repo, so it collides instead of arriving as a second project.
    #[tokio::test]
    async fn adding_a_worktree_of_a_known_repo_is_a_duplicate() {
        let tmp = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(tmp.path()).unwrap();
        let repo = root.join("repo");
        std::fs::create_dir(&repo).unwrap();
        git_in(&repo, &["init", "-b", "main"]);
        git_in(&repo, &["commit", "--allow-empty", "-m", "init"]);
        let feat = root.join("repo-worktrees").join("feat");
        git_in(
            &repo,
            &["worktree", "add", &feat.to_string_lossy(), "-b", "feat"],
        );

        let daemon = test_daemon();
        daemon.add_project(&repo, None, false, None).await.unwrap();
        let err = daemon
            .add_project(&feat, None, false, None)
            .await
            .unwrap_err();
        assert!(
            err.to_string().contains("already added"),
            "expected a duplicate error, got: {err}"
        );
    }

    /// Root-ness is derived from git's checkout list on every pass, not frozen
    /// at insert time: a project whose rows were seeded before the root was
    /// known (or seeded wrong) has its ⌂ root row repaired in place, and the
    /// stale one loses the reprieve that kept it undeletable.
    #[tokio::test]
    async fn reconcile_moves_root_ness_onto_the_checkout_git_lists_first() {
        let tmp = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(tmp.path()).unwrap();
        let repo = root.join("repo");
        std::fs::create_dir(&repo).unwrap();
        git_in(&repo, &["init", "-b", "main"]);
        git_in(&repo, &["commit", "--allow-empty", "-m", "init"]);
        let feat = root.join("repo-worktrees").join("feat");
        git_in(
            &repo,
            &["worktree", "add", &feat.to_string_lossy(), "-b", "feat"],
        );

        let daemon = test_daemon();
        let project = Project {
            workspace_id: Default::default(),
            id: ProjectId("p".into()),
            name: "p".into(),
            repo_path: repo.clone(),
            sort_order: 0,
        };
        daemon.store.insert_project(&project).unwrap();
        // The wrong way round: the linked checkout wears the root badge and
        // the repo's own checkout is a plain row.
        seed_worktree(&daemon, "p", "wt", &feat.to_string_lossy(), true);
        seed_worktree(&daemon, "p", "rt", &repo.to_string_lossy(), false);

        daemon.sync_project_worktrees(&project).await.unwrap();

        let (_, worktrees, _, _) = daemon.store.load_tree().unwrap();
        let by = |id: &str| worktrees.iter().find(|w| w.id.as_str() == id).unwrap();
        assert!(by("rt").is_main, "the repo's checkout is the root row");
        assert!(!by("wt").is_main, "the linked checkout gave the badge back");
        assert_eq!(by("rt").branch, "main");
        assert_eq!(by("wt").branch, "feat");
    }

    /// A row still carrying a stale `is_main` no longer survives its checkout
    /// going away — the real root is always in git's list, so anything missing
    /// from it is a linked checkout, whatever flag it happens to hold.
    #[tokio::test]
    async fn reconcile_drops_a_vanished_row_that_still_claims_to_be_root() {
        let tmp = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(tmp.path()).unwrap();
        let repo = root.join("repo");
        std::fs::create_dir(&repo).unwrap();
        git_in(&repo, &["init", "-b", "main"]);
        git_in(&repo, &["commit", "--allow-empty", "-m", "init"]);

        let daemon = test_daemon();
        let project = Project {
            workspace_id: Default::default(),
            id: ProjectId("p".into()),
            name: "p".into(),
            repo_path: repo.clone(),
            sort_order: 0,
        };
        daemon.store.insert_project(&project).unwrap();
        seed_worktree(&daemon, "p", "rt", &repo.to_string_lossy(), true);
        seed_worktree(
            &daemon,
            "p",
            "ghost",
            &root.join("repo-worktrees").join("gone").to_string_lossy(),
            true,
        );

        daemon.sync_project_worktrees(&project).await.unwrap();

        let (_, worktrees, _, _) = daemon.store.load_tree().unwrap();
        assert!(
            worktrees.iter().all(|w| w.id.as_str() != "ghost"),
            "the ghost row is gone: {worktrees:#?}"
        );
        let rt = worktrees.iter().find(|w| w.id.as_str() == "rt").unwrap();
        assert!(rt.is_main, "the surviving root row keeps the badge");
    }

    fn git_in(repo: &Path, args: &[&str]) {
        let out = std::process::Command::new("git")
            .arg("-C")
            .arg(repo)
            .args(args)
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@example.com")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@example.com")
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
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
    fn normalize_url_adds_https_and_refuses_non_links() {
        // Pasted URLs pass through untouched.
        assert_eq!(
            normalize_url("https://github.com/o/r/pull/7").unwrap(),
            "https://github.com/o/r/pull/7"
        );
        assert_eq!(normalize_url("  http://x.dev  ").unwrap(), "http://x.dev");
        // Typed hosts gain the scheme.
        assert_eq!(
            normalize_url("github.com/o/r/pull/7").unwrap(),
            "https://github.com/o/r/pull/7"
        );
        // Anything that isn't an http(s) URL is refused, so `open(1)` can
        // never be handed a scheme the user didn't intend.
        for bad in [
            "",
            "   ",
            "file:///etc/passwd",
            "javascript:alert(1)",
            "https://",
            "just a note",
            "notaurl",
        ] {
            assert!(normalize_url(bad).is_err(), "expected refusal: {bad:?}");
        }
    }

    #[test]
    fn cli_missing_message_names_the_binary_not_the_kind() {
        // Cursor ships its agent as `cursor-agent`; naming the kind would
        // send the user off to install the wrong thing.
        assert!(cli_missing_message(AgentKind::Cursor).starts_with("cursor-agent was not found"));
        assert!(cli_missing_message(AgentKind::Claude).starts_with("claude was not found"));
        assert!(cli_missing_message(AgentKind::Codex).starts_with("codex was not found"));
        assert!(cli_missing_message(AgentKind::Pi).starts_with("pi was not found"));
        // No "restart nebula": agent CLIs are spawned through the user's
        // login shell, so a fresh install is picked up on the next try.
        for kind in AgentKind::ALL {
            let msg = cli_missing_message(kind);
            assert!(msg.contains("try again"), "{msg}");
            assert!(!msg.contains("restart"), "{msg}");
        }
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
                model: None,
                effort: None,
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
            .take_prewarmed(&WorktreeId("w1".into()), AgentKind::Claude, None, None)
            .is_none());
        assert!(daemon.prewarmed.lock().unwrap().is_empty());

        daemon.prewarmed.lock().unwrap().insert(
            key.clone(),
            PrewarmEntry {
                agent_id: AgentId("warm-2".into()),
                spawned_at: Instant::now(),
                model: None,
                effort: None,
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
                    model: None,
                    effort: None,
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

    // ---- workspaces ----

    #[test]
    fn workspace_lifecycle_add_open_rename_delete() {
        let daemon = test_daemon();
        let EntityId::Workspace(id) = daemon.add_workspace(" client ").unwrap() else {
            panic!("add returns the workspace id");
        };
        // Name is trimmed; duplicates (trimmed) and blanks are refused.
        assert_eq!(
            daemon.store.get_workspace(&id).unwrap().unwrap().name,
            "client"
        );
        assert!(daemon.add_workspace("client").is_err());
        assert!(daemon.add_workspace("   ").is_err());

        // Adding never opens; opening one moves the remembered default
        // (and re-opening is a quiet no-op).
        assert_eq!(
            daemon.store.active_workspace_id().unwrap().as_str(),
            "default"
        );
        daemon.set_default_workspace(&id).unwrap();
        assert_eq!(daemon.store.active_workspace_id().unwrap(), id);
        daemon.set_default_workspace(&id).unwrap();
        assert!(daemon
            .set_default_workspace(&WorkspaceId("ghost".into()))
            .is_err());

        // Rename keeps names unique (a rename to itself is fine).
        daemon.rename_workspace(&id, "acme").unwrap();
        daemon.rename_workspace(&id, "acme").unwrap();
        assert!(daemon.rename_workspace(&id, "default").is_err());

        // Deleting the default workspace moves the default to a survivor.
        daemon.remove_workspace(&id).unwrap();
        assert_eq!(
            daemon.store.active_workspace_id().unwrap().as_str(),
            "default"
        );
        assert!(daemon.store.get_workspace(&id).unwrap().is_none());

        // The last workspace can't go.
        assert!(daemon
            .remove_workspace(&WorkspaceId("default".into()))
            .is_err());
    }

    #[test]
    fn workspace_with_projects_refuses_deletion() {
        let daemon = test_daemon();
        seed_projects(&daemon, &["p"]); // lands in 'default'
        let EntityId::Workspace(empty) = daemon.add_workspace("empty").unwrap() else {
            panic!("add returns the workspace id");
        };
        let err = daemon
            .remove_workspace(&WorkspaceId("default".into()))
            .unwrap_err();
        assert!(
            err.to_string().contains("1 project"),
            "helpful refusal: {err}"
        );
        // An empty, closed workspace deletes cleanly.
        daemon.remove_workspace(&empty).unwrap();
    }

    /// The status broadcast carries the flag it persisted: a live turn
    /// landing on finished says `unseen`, the next prompt says not.
    #[test]
    fn status_broadcast_carries_the_unseen_flag() {
        let daemon = test_daemon();
        seed_projects(&daemon, &["p"]);
        seed_worktree(&daemon, "p", "root", "/nebula-test/p", true);
        seed_agent(&daemon, "a1", "root", None); // running
        let id = AgentId("a1".into());
        let mut rx = daemon.events.subscribe();

        daemon.apply_status_effects(&id, vec![Effect::SetStatus(AgentStatus::Finished)]);
        match rx.try_recv().unwrap() {
            ServerEvent::StatusChanged { status, unseen, .. } => {
                assert_eq!(status, AgentStatus::Finished);
                assert!(unseen, "yellow → green with nobody told otherwise");
            }
            other => panic!("expected a status change, got {other:?}"),
        }
        daemon.apply_status_effects(&id, vec![Effect::SetStatus(AgentStatus::Running)]);
        match rx.try_recv().unwrap() {
            ServerEvent::StatusChanged { unseen, .. } => {
                assert!(!unseen, "a new turn: nothing finished to read")
            }
            other => panic!("expected a status change, got {other:?}"),
        }
    }

    /// `mark_agent_seen` clears the flag and hands every subscriber the row
    /// — once. Marking a row already read sends nothing.
    #[test]
    fn mark_agent_seen_broadcasts_only_a_flip() {
        let daemon = test_daemon();
        seed_projects(&daemon, &["p"]);
        seed_worktree(&daemon, "p", "root", "/nebula-test/p", true);
        seed_agent(&daemon, "a1", "root", None);
        let id = AgentId("a1".into());
        daemon
            .store
            .set_agent_status(&id, AgentStatus::Finished)
            .unwrap();
        let mut rx = daemon.events.subscribe();

        daemon.mark_agent_seen(&id).unwrap();
        match rx.try_recv().unwrap() {
            ServerEvent::EntityUpserted {
                entity: Entity::Agent(a),
            } => assert!(!a.unseen),
            other => panic!("expected agent upsert, got {other:?}"),
        }
        daemon.mark_agent_seen(&id).unwrap();
        assert!(rx.try_recv().is_err(), "nothing to say twice");
    }
}
