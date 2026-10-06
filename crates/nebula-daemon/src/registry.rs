//! The daemon's world: persisted entity tree + live PTY sessions, and the
//! operations the IPC surface exposes over them.

use crate::claude_bg;
use crate::git;
use crate::hooks::{self, HookEnv};
use crate::pty::{PtyEvent, PtySession, SpawnSpec, DEFAULT_COLS, DEFAULT_ROWS};
use crate::status::{AgentStatusMachine, Effect, HookEvent};
use crate::store::Store;
use crate::worktree_hooks::{self, HookContext, WorktreeHook};
use anyhow::{bail, Context, Result};
use nebula_core::env;
use nebula_core::project_file::{self, ProjectCommand};
use nebula_core::{
    Agent, AgentId, AgentKind, AgentStatus, Entity, EntityId, LinkId, PrewarmInfo, Project,
    ProjectId, ProjectRepository, ServerEvent, SessionRef, TerminalId, TerminalTab, WorkspaceState,
    Worktree, WorktreeId, MAX_CLOUD_PROMPT_BYTES,
};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
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
/// A resumed agent CLI that exits with an error this soon after its spawn
/// is taken to have not found the session it was told to resume. Generous
/// on purpose: the login shell alone takes most of a second, and a worktree
/// prewarm sweep boots CLIs back to back — the 2 s this used to be let slow
/// failures through, leaving the pane on the CLI's error.
const RESUME_FAIL_WINDOW: Duration = Duration::from_secs(10);

/// A resumed spawn, watched for the fast failure of a missing session.
struct ResumeWatch {
    /// The PTY it booted, so a later respawn's exit is never mistaken for it.
    session: std::sync::Weak<PtySession>,
    spawned_at: Instant,
    cols: u16,
    rows: u16,
}
/// `$SHELL -l -i -c <cmd>`: a login *and* interactive shell, so zsh sources
/// ~/.zprofile and ~/.zshrc both and the child sees the PATH the user's
/// terminal has. The CLI probe and the spawn wrapper share it so they can
/// never disagree about what "on the user's PATH" means.
const LOGIN_SHELL_ARGS: [&str; 3] = ["-l", "-i", "-c"];
/// Cap on one CLI probe. A heavy rc file costs ~1s; a hung one must not
/// stall a create forever, so on timeout the CLI is assumed present and
/// the spawn itself gets to report.
const CLI_PROBE_TIMEOUT: Duration = Duration::from_secs(5);
/// A RUN TERMINAL whose command exited on its own: the PTY, kept for the
/// replay, and the exit code the OS reported.
type FinishedRun = (Arc<PtySession>, Option<i32>);
/// What a RUN TERMINAL's row is called in the Sessions panel.
const RUN_TERMINAL_NAME: &str = "run";

fn run_terminal_name(repository: Option<&str>) -> String {
    repository.map_or_else(
        || RUN_TERMINAL_NAME.to_string(),
        |name| format!("run:{name}"),
    )
}
/// `r` on a worktree with nothing to run: the footer line naming both
/// places a RUN COMMAND can come from.
const NO_RUN_COMMAND: &str =
    "no run command for this worktree — set one in Settings (s) → Project, \
                              or add .nebula.json with {\"run\": \"npm run dev\"}";
/// Why a RUN TERMINAL with nothing left to replay will not attach — its
/// run ended before a DAEMON restart, typically.
const RUN_NOT_RUNNING: &str = "this run has stopped — press r on its worktree to start it again";

/// Report user-owned dirt in repository worktrees and unexpected entries at
/// the workspace root. Git status omits ignored files and reports individual
/// untracked paths for actionable refusal messages.
fn session_workspace_dirt(
    repositories: &[nebula_core::WorkspaceRepository],
    folder: &Path,
) -> Result<Vec<String>> {
    let mut faults = Vec::new();
    for repository in repositories {
        if !repository.path.exists() {
            let member_exists = std::fs::symlink_metadata(&folder.join(&repository.name)).is_ok();
            if member_exists {
                faults.push(format!(
                    "repository `{}` folder {} is not a registered Git worktree",
                    repository.name,
                    folder.join(&repository.name).display()
                ));
            }
            continue;
        }
        let output = std::process::Command::new("git")
            .arg("-C")
            .arg(&repository.path)
            .args(["status", "--porcelain=v1", "--untracked-files=all"])
            .output()
            .with_context(|| format!("inspect repository `{}`", repository.name))?;
        if !output.status.success() {
            faults.push(format!(
                "repository `{}` at {}: {}",
                repository.name,
                repository.path.display(),
                String::from_utf8_lossy(&output.stderr).trim()
            ));
            continue;
        }
        for line in String::from_utf8_lossy(&output.stdout).lines() {
            let path = line.get(3..).unwrap_or(line).trim();
            faults.push(format!("repository `{}` path {}", repository.name, path));
        }
    }
    faults.extend(
        workspace_folder_extras_for_names(
            folder,
            &repositories
                .iter()
                .map(|repository| repository.name.as_str())
                .collect::<Vec<_>>(),
        )?
        .into_iter()
        .map(|path| format!("workspace path {}", path.display())),
    );
    Ok(faults)
}

fn workspace_folder_extras_for_names(folder: &Path, repositories: &[&str]) -> Result<Vec<PathBuf>> {
    if !folder.exists() {
        return Ok(Vec::new());
    }
    let mut extras = Vec::new();
    for entry in std::fs::read_dir(folder).with_context(|| format!("read {}", folder.display()))? {
        let entry = entry?;
        let path = entry.path();
        let file_type = entry.file_type()?;
        let rel = path
            .strip_prefix(folder)
            .unwrap_or(&path)
            .to_string_lossy()
            .replace('\\', "/");
        if file_type.is_dir() && repositories.iter().any(|repository| *repository == rel) {
            continue;
        }
        if matches!(rel.as_str(), ".claude" | ".cursor") {
            collect_hook_tree_extras(folder, &path, &mut extras)?;
        } else {
            extras.push(path);
        }
    }
    Ok(extras)
}

fn collect_hook_tree_extras(folder: &Path, dir: &Path, extras: &mut Vec<PathBuf>) -> Result<()> {
    for entry in std::fs::read_dir(dir).with_context(|| format!("read {}", dir.display()))? {
        let entry = entry?;
        let path = entry.path();
        let file_type = entry.file_type()?;
        let rel = path
            .strip_prefix(folder)
            .unwrap_or(&path)
            .to_string_lossy()
            .replace('\\', "/");
        if matches!(
            rel.as_str(),
            ".claude/settings.local.json" | ".cursor/hooks.json" | ".cursor/rules/nebula-title.mdc"
        ) && file_type.is_file()
        {
            continue;
        }
        if file_type.is_dir() && matches!(rel.as_str(), ".claude" | ".cursor" | ".cursor/rules") {
            collect_hook_tree_extras(folder, &path, extras)?;
        } else {
            extras.push(path);
        }
    }
    Ok(())
}

fn remove_workspace_hook_files(folder: &Path) -> Result<()> {
    for (path, cursor) in [
        (folder.join(".claude/settings.local.json"), false),
        (folder.join(".cursor/hooks.json"), true),
    ] {
        remove_nebula_groups_from_file(&path, cursor)?;
        if path.exists() {
            bail!(
                "preserved non-Nebula settings remain in {}; review or remove this file before retrying",
                path.display()
            );
        }
    }
    remove_file_if_present(&folder.join(".cursor/rules/nebula-title.mdc"))?;
    for dir in [
        folder.join(".claude"),
        folder.join(".cursor/rules"),
        folder.join(".cursor"),
    ] {
        let _ = std::fs::remove_dir(dir);
    }
    Ok(())
}

fn remove_nebula_groups_from_file(path: &Path, cursor: bool) -> Result<()> {
    if !path.exists() {
        return Ok(());
    }
    let source =
        std::fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    let mut root: serde_json::Value =
        serde_json::from_str(&source).with_context(|| format!("parse {}", path.display()))?;
    if let Some(hooks) = root
        .get_mut("hooks")
        .and_then(serde_json::Value::as_object_mut)
    {
        for groups in hooks.values_mut() {
            if let Some(groups) = groups.as_array_mut() {
                groups.retain(|group| {
                    group
                        .get("_nebulaManaged")
                        .and_then(serde_json::Value::as_bool)
                        != Some(true)
                        && !group.to_string().contains("/api/hooks/")
                });
            }
        }
        hooks.retain(|_, groups| groups.as_array().is_none_or(|groups| !groups.is_empty()));
    }
    if let Some(allow) = root
        .get_mut("permissions")
        .and_then(serde_json::Value::as_object_mut)
        .and_then(|permissions| permissions.get_mut("allow"))
        .and_then(serde_json::Value::as_array_mut)
    {
        allow.retain(|rule| {
            !matches!(
                rule.as_str(),
                Some("Bash(nebula rename:*)")
                    | Some("Bash(nebula worktree:*)")
                    | Some("Bash(nebula spawn:*)")
                    | Some("Bash(nebula open:*)")
            )
        });
    }
    if let Some(object) = root.as_object_mut() {
        if let Some(permissions) = object
            .get_mut("permissions")
            .and_then(serde_json::Value::as_object_mut)
        {
            permissions.retain(|_, value| {
                value.as_array().is_none_or(|values| !values.is_empty())
                    && value.as_object().is_none_or(|values| !values.is_empty())
            });
            if permissions.is_empty() {
                object.remove("permissions");
            }
        }
        if object
            .get("hooks")
            .and_then(serde_json::Value::as_object)
            .is_some_and(|hooks| hooks.is_empty())
        {
            object.remove("hooks");
        }
    }
    let empty_after_cleanup = root.as_object().is_some_and(|object| object.is_empty());
    if empty_after_cleanup {
        return remove_file_if_present(path);
    }
    let empty = root.as_object().is_some_and(|object| {
        cursor
            && object.keys().all(|key| key == "version")
            && object.get("version").and_then(serde_json::Value::as_i64) == Some(1)
    });
    if empty {
        return remove_file_if_present(path);
    }
    std::fs::write(path, serde_json::to_vec_pretty(&root)?)
        .with_context(|| format!("write {}", path.display()))?;
    Ok(())
}

fn remove_file_if_present(path: &Path) -> Result<()> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error).with_context(|| format!("remove {}", path.display())),
    }
}

async fn validate_workspace_name(name: &str) -> Result<()> {
    if name.is_empty() {
        bail!("workspace name is empty");
    }
    let path = Path::new(name);
    if path.components().count() != 1
        || !matches!(
            path.components().next(),
            Some(std::path::Component::Normal(_))
        )
        || name == "."
        || name == ".."
    {
        bail!("workspace name `{name}` must be a single safe folder name");
    }
    if name.contains(['\\', ':']) || name.ends_with('.') || name.ends_with(' ') {
        bail!("workspace name `{name}` is not safe as a folder name");
    }
    let branch = format!("pilot/{name}");
    if tokio::process::Command::new("git")
        .args(["check-ref-format", "--branch", &branch])
        .status()
        .await
        .map_or(true, |status| !status.success())
    {
        bail!("workspace name `{name}` is not valid in a Git branch name");
    }
    Ok(())
}

pub(crate) struct CreateAgentSpec {
    pub worktree: WorktreeId,
    pub name: String,
    pub kind: AgentKind,
    /// Registry id of the custom harness, when `kind` is
    /// [`AgentKind::Custom`].
    pub custom_harness: Option<String>,
    pub model: Option<String>,
    pub effort: Option<String>,
    pub auto_title: bool,
    pub cloud_prompt: Option<String>,
    /// The CLI's positional first prompt (an AGENT PRESET launch). Request-only.
    pub starting_prompt: Option<String>,
    pub pr_url: Option<String>,
    /// The GitHub issue an ISSUE SESSION was launched for (see
    /// `pr_scope::issue_rule`). Persisted like `pr_url`.
    pub issue_url: Option<String>,
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
    cli_probes: Mutex<HashMap<String, (bool, Instant)>>,
    /// How many client connections are attached per session — a session
    /// with attachments (and its whole worktree) is "in view" and exempt
    /// from idle reaping.
    attach_counts: Mutex<HashMap<SessionRef, usize>>,
    /// When each live session was last "looked at": spawned, prewarmed,
    /// attached, or covered by the in-view sweep refresh. The idle reaper
    /// kills sessions whose stamp ages past `session_idle_timeout`.
    session_interest: Mutex<HashMap<SessionRef, Instant>>,
    /// Where each Claude agent's transcript (and beside it, the session
    /// title `/rename` persists) lives, from its hook payloads. Read by
    /// the CLAUDE TITLE SYNC (`session_title.rs`) when the PTY's window
    /// title changes, which no hook reports.
    pub(crate) transcripts: Mutex<HashMap<AgentId, crate::session_title::TranscriptRef>>,
    /// Serializes the check-and-spawn inside [`Daemon::ensure_session`].
    /// Attach (the request loop) and the worktree prewarm sweep (its own
    /// task) can both reach for the same dead session; without this they
    /// would both miss the registry and fork two CLIs, orphaning one.
    spawn_gate: Mutex<()>,
    /// The worktree prewarm sweep currently running, so a newer one can
    /// cancel it. Walking the project tabs fires a sweep per step, and
    /// only the project the cursor rests on is worth warming.
    prewarm_sweep: Mutex<Option<tokio::task::JoinHandle<()>>>,
    /// Resumed agent spawns, by agent, until their PTY exits: a resume that
    /// dies inside [`RESUME_FAIL_WINDOW`] could not find its session, and
    /// `watch_for_exit` reads that off this record.
    resumes: Mutex<HashMap<AgentId, ResumeWatch>>,
    /// RUN TERMINALS whose command exited on its own, with its exit code.
    /// The PTY is kept — outside `sessions`, so the row reads as not alive
    /// — so an attach replays how the run ended instead of running it
    /// again. Dropped when the run starts again, is stopped, or its row goes.
    finished_runs: Mutex<HashMap<TerminalId, FinishedRun>>,
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
            transcripts: Mutex::new(HashMap::new()),
            spawn_gate: Mutex::new(()),
            prewarm_sweep: Mutex::new(None),
            resumes: Mutex::new(HashMap::new()),
            finished_runs: Mutex::new(HashMap::new()),
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
        let relocating = false;
        let outcome = {
            let mut machines = self.status_machines.lock().unwrap();
            match machines.entry(agent_id.clone()) {
                std::collections::hash_map::Entry::Occupied(e) => {
                    let machine = e.into_mut();
                    machine.set_relocating(relocating);
                    Outcome::Effects(machine.handle(event, session_id.as_deref(), Instant::now()))
                }
                std::collections::hash_map::Entry::Vacant(slot) => {
                    // Lazily seed from the persisted row.
                    match self.store.get_agent(agent_id) {
                        Ok(Some(agent)) => {
                            let machine = slot
                                .insert(AgentStatusMachine::new(agent.status, agent.session_id));
                            machine.set_relocating(relocating);
                            Outcome::Effects(machine.handle(
                                event,
                                session_id.as_deref(),
                                Instant::now(),
                            ))
                        }
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
                            (nebula_core::clock::now_ms(), false)
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

    /// Whether `session` is still the PTY registered for `sref` — not one
    /// a kill (restart, move, relocation) has since replaced.
    fn owns_session(&self, sref: &SessionRef, session: &Arc<PtySession>) -> bool {
        self.sessions
            .lock()
            .unwrap()
            .get(sref)
            .is_some_and(|current| Arc::ptr_eq(current, session))
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

    // ---- in-place restart ----

    /// Every live session, written down for the image an IN-PLACE RESTART
    /// execs into, with its master fd left open across the exec. A warm
    /// spare from the PREWARM POOL goes too, marked, so the new image reaps
    /// it rather than leaving an unreaped child behind.
    pub fn carry_sessions(&self) -> Result<Vec<crate::handoff::CarriedSession>> {
        let spares: std::collections::HashSet<AgentId> = self
            .prewarmed
            .lock()
            .unwrap()
            .values()
            .map(|e| e.agent_id.clone())
            .collect();
        let launching: std::collections::HashSet<AgentId> = self
            .status_machines
            .lock()
            .unwrap()
            .iter()
            .filter(|(_, m)| m.is_launching())
            .map(|(id, _)| id.clone())
            .collect();
        let sessions: Vec<Arc<PtySession>> =
            self.sessions.lock().unwrap().values().cloned().collect();
        let mut carried = Vec::with_capacity(sessions.len());
        for session in sessions {
            let (spare, launching) = match &session.sref {
                SessionRef::Agent(id) => (spares.contains(id), launching.contains(id)),
                SessionRef::Terminal(_) => (false, false),
            };
            match session.carry() {
                Ok(Some(pty)) => carried.push(crate::handoff::CarriedSession {
                    pty,
                    spare,
                    launching,
                }),
                Ok(None) => {}
                Err(e) => {
                    self.uncarry_sessions();
                    return Err(e.context(format!("carry {:?}", session.sref)));
                }
            }
        }
        Ok(carried)
    }

    /// Keep [`Self::ensure_session`] from spawning while the guard lives.
    pub fn hold_spawns(&self) -> std::sync::MutexGuard<'_, ()> {
        self.spawn_gate.lock().unwrap()
    }

    /// The exec [`Self::carry_sessions`] prepared for failed: close the
    /// master fds on exec again.
    pub fn uncarry_sessions(&self) {
        for session in self.sessions.lock().unwrap().values() {
            session.uncarry();
        }
    }

    /// Take over what the previous image carried. Spares are reaped at
    /// once: the pool that would hand them out did not come along. Returns
    /// how many sessions are live here now.
    pub fn adopt_sessions(self: &Arc<Self>, carried: Vec<crate::handoff::CarriedSession>) -> usize {
        let mut adopted = 0;
        for crate::handoff::CarriedSession {
            pty,
            spare,
            launching,
        } in carried
        {
            let sref = pty.sref.clone();
            match PtySession::adopt(pty) {
                Ok(session) if spare => session.kill(),
                Ok(session) => {
                    if let (true, SessionRef::Agent(id)) = (launching, &sref) {
                        self.status_machines
                            .lock()
                            .unwrap()
                            .insert(id.clone(), AgentStatusMachine::launching());
                    }
                    self.install_session(session);
                    adopted += 1;
                }
                Err(e) => tracing::warn!(session = ?sref, error = %e, "could not adopt session"),
            }
        }
        adopted
    }

    /// Relocations still waiting on their turn to end (`nebula worktree`),
    /// for an IN-PLACE RESTART to carry.
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
    /// feedback, agents with a backgrounded tool call still running (a job
    /// detached from their terminal — see `pty::detached_job_under`),
    /// terminals with a command running, and prewarm-pool sessions
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
                    Some(agent)
                        if matches!(
                            agent.status,
                            AgentStatus::Running | AgentStatus::NeedsFeedback
                        ) =>
                    {
                        true
                    }
                    // A backgrounded tool call — Claude's `run_in_background`
                    // Bash, its Monitor watch, a Codex shell command —
                    // outlives the turn that started it, and the hook-fed
                    // status machine read that turn's Stop as Finished. The
                    // process tree still knows (#78). Restamped rather than
                    // just skipped: the job ending is the moment the agent
                    // wakes to read its result, so the clock starts there.
                    Some(_) => {
                        let busy = agent_has_detached_job(&session);
                        if busy {
                            self.touch_session(&sref);
                        }
                        busy
                    }
                    // Row vanished mid-sweep: its delete kills the PTY anyway.
                    None => true,
                },
                // A RUN TERMINAL is the server `r` started: sitting quiet
                // is what it is for, and only `r` again stops it.
                SessionRef::Terminal(id) => {
                    self.is_run_terminal(id) || shell_has_children(&session)
                }
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
        let repositories = projects
            .iter()
            .map(|project| self.store.project_repositories(&project.id))
            .collect::<Result<Vec<_>>>()?
            .into_iter()
            .flatten()
            .collect();
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
            repositories,
            session_workspaces: self.store.session_workspaces()?,
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
        Ok(agent)
    }

    /// Push the agent's current row — liveness included —
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
    pub(crate) fn try_broadcast_agent(&self, id: &AgentId) {
        let _ = self.broadcast_agent(id);
    }

    fn terminal_entity(&self, id: &TerminalId) -> Result<TerminalTab> {
        let mut term = self.store.get_terminal(id)?.context("terminal not found")?;
        term.alive = self.is_alive(&SessionRef::Terminal(id.clone()));
        Ok(term)
    }

    // ---- projects ----

    /// Register an existing project folder and its direct-child repository
    /// checkouts without changing any checkout on disk.
    pub async fn register_project(
        self: &Arc<Self>,
        folder: &Path,
        name: &str,
        repositories: &[String],
    ) -> Result<EntityId> {
        use std::collections::HashSet;

        if repositories.is_empty() {
            bail!("at least one repository folder name is required");
        }
        let folder = tokio::fs::canonicalize(folder)
            .await
            .with_context(|| format!("project folder {} does not exist", folder.display()))?;
        if !folder.is_dir() {
            bail!("{} is not a project folder", folder.display());
        }

        let mut seen = HashSet::new();
        let mut members = Vec::with_capacity(repositories.len());
        for name in repositories {
            let member = Path::new(name);
            if member.components().count() != 1
                || !matches!(
                    member.components().next(),
                    Some(std::path::Component::Normal(_))
                )
            {
                bail!("repository `{name}` must be a direct child folder name");
            }
            if name == "worktrees" {
                bail!("repository name `worktrees` is reserved for session workspaces");
            }
            if !seen.insert(name) {
                bail!("repository `{name}` is listed more than once");
            }
            let path = tokio::fs::canonicalize(folder.join(name))
                .await
                .with_context(|| {
                    format!("repository `{name}` is missing from {}", folder.display())
                })?;
            if path.parent() != Some(folder.as_path()) {
                bail!(
                    "repository `{name}` must be a direct child of {}",
                    folder.display()
                );
            }
            let toplevel = git::repo_toplevel(&path)
                .await
                .with_context(|| format!("repository `{name}` is not a Git checkout"))?;
            if toplevel != path {
                bail!(
                    "repository `{name}` is not a Git checkout directly inside {}",
                    folder.display()
                );
            }
            let branch = git::current_branch(&path).await?;
            members.push(ProjectRepository {
                project_id: ProjectId::generate(),
                name: name.clone(),
                path,
                branch,
                changed_files: None,
            });
        }

        let project = Project {
            id: ProjectId::generate(),
            name: if name.trim().is_empty() {
                Project::folder_name(&folder)
            } else {
                name.trim().to_string()
            },
            repo_path: folder.clone(),
            sort_order: self.store.next_project_sort_order()?,
        };
        let mut members = members;
        for member in &mut members {
            member.project_id = project.id.clone();
        }

        let worktree = Worktree {
            id: WorktreeId::generate(),
            project_id: project.id.clone(),
            path: folder.clone(),
            branch: "main workspace".into(),
            is_main: true,
            sort_order: 0,
        };
        self.store
            .insert_project_with_repositories(&project, &members)?;
        if let Err(error) = self.store.insert_worktree(&worktree) {
            let _ = self.store.delete_project(&project.id);
            return Err(error);
        }
        self.broadcast(ServerEvent::EntityUpserted {
            entity: Entity::Project(project.clone()),
        });
        self.broadcast(ServerEvent::EntityUpserted {
            entity: Entity::Worktree(worktree),
        });
        self.broadcast(ServerEvent::ProjectRepositoriesUpserted {
            project: project.id.clone(),
            repositories: members,
        });
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

    /// Create one branch/worktree per registered repository under the
    /// project's shared workspace folder. Conflicts and path safety are
    /// checked for every member before the first filesystem mutation.
    pub async fn create_session_workspace(
        self: &Arc<Self>,
        project_id: &ProjectId,
        raw_name: &str,
    ) -> Result<(Worktree, Vec<nebula_core::WorkspaceRepository>)> {
        let name = raw_name.trim();
        validate_workspace_name(name).await?;
        let _ops = self.worktree_ops.lock().await;
        let project = self
            .store
            .get_project(project_id)?
            .context("project not found")?;
        if self.store.session_workspaces()?.iter().any(|workspace| {
            workspace.name == name
                && self
                    .store
                    .get_worktree(&workspace.worktree_id)
                    .ok()
                    .flatten()
                    .is_some_and(|worktree| worktree.project_id == *project_id)
        }) {
            bail!("session workspace `{name}` already exists in this project");
        }
        let members = self.store.project_repositories(project_id)?;
        if members.is_empty() {
            bail!("project has no registered repositories");
        }
        let folder = project.repo_path.join("worktrees").join(name);
        if std::fs::symlink_metadata(&folder).is_ok() {
            bail!("workspace folder already exists: {}", folder.display());
        }
        let branch = format!("pilot/{name}");
        for member in &members {
            let _ = git::refresh_origin(&member.path).await;
        }
        for member in &members {
            if git::branch_exists(&member.path, &branch).await {
                bail!("repository `{}` already has branch `{branch}`; delete the kept branch before reusing this workspace name", member.name);
            }
        }

        let mut report = Vec::with_capacity(members.len());
        let mut roots = Vec::with_capacity(members.len());
        let workspace = Worktree {
            id: WorktreeId::generate(),
            project_id: project_id.clone(),
            path: folder.clone(),
            branch: branch.clone(),
            is_main: false,
            sort_order: self.store.next_worktree_sort_order(project_id)?,
        };
        let mut metadata = nebula_core::SessionWorkspace {
            worktree_id: workspace.id.clone(),
            name: name.to_string(),
            state: WorkspaceState::Creating,
            failure: None,
            repositories: vec![],
        };
        self.store.insert_session_workspace(&workspace, &metadata)?;
        self.broadcast(ServerEvent::EntityUpserted {
            entity: Entity::Worktree(workspace.clone()),
        });
        self.broadcast(ServerEvent::SessionWorkspaceUpserted {
            workspace: metadata.clone(),
        });
        let config = crate::config::Config::load();
        for member in members {
            let destination = folder.join(&member.name);
            let creation = match git::add_worktree_at(
                &member.path,
                &branch,
                &destination,
                config.worktree_base_branch().as_deref(),
                true,
            )
            .await
            {
                Ok(creation) => creation,
                Err(error) => {
                    let message =
                        format!("repository `{}`: create {branch}: {error:#}", member.name);
                    let mut attempted = report.clone();
                    attempted.push(nebula_core::WorkspaceRepository {
                        id: WorktreeId::generate(),
                        name: member.name.clone(),
                        path: destination.clone(),
                        base: None,
                        fetch_succeeded: false,
                        warning: None,
                        branch: None,
                        changed_files: None,
                    });
                    let (leftovers, remaining) = self
                        .rollback_session_workspace(&workspace, &attempted)
                        .await;
                    if leftovers.is_empty() {
                        self.store.delete_session_workspace(&workspace.id)?;
                        self.broadcast(ServerEvent::EntityRemoved {
                            id: EntityId::Worktree(workspace.id.clone()),
                        });
                        return Err(anyhow::anyhow!(message));
                    }
                    let failure = format!(
                        "{message}; cleanup failed; leftovers: {}",
                        leftovers.join(", ")
                    );
                    metadata.repositories = attempted
                        .into_iter()
                        .filter(|repository| {
                            remaining.iter().any(|leftover| {
                                leftover.contains(&repository.path.display().to_string())
                            }) || remaining.iter().any(|leftover| {
                                leftover.contains(&repository.name)
                                    && !leftover.starts_with("leftover path ")
                            })
                        })
                        .collect();
                    metadata.state = WorkspaceState::Unavailable;
                    metadata.failure = Some(failure.clone());
                    self.store.set_session_workspace_state(
                        &workspace.id,
                        WorkspaceState::Unavailable,
                        Some(&failure),
                    )?;
                    let _ = self.store.update_session_workspace_repositories(
                        &workspace.id,
                        &metadata.repositories,
                    );
                    self.broadcast(ServerEvent::SessionWorkspaceUpserted {
                        workspace: metadata,
                    });
                    return Err(anyhow::anyhow!(failure));
                }
            };
            let root = Worktree {
                id: WorktreeId::generate(),
                project_id: project_id.clone(),
                path: destination.clone(),
                branch: branch.clone(),
                is_main: false,
                sort_order: 0,
            };
            let mut repository = nebula_core::WorkspaceRepository {
                id: root.id.clone(),
                name: member.name.clone(),
                path: destination.clone(),
                base: creation.base,
                fetch_succeeded: creation.fetch_succeeded,
                warning: None,
                branch: Some(branch.clone()),
                changed_files: git::changed_file_count(&destination).await.ok(),
            };
            // Persist the new member before running hooks, so a daemon
            // interruption during a slow hook can still diagnose and gate it.
            self.store.insert_workspace_repository(
                &workspace.id,
                project_id,
                &branch,
                &repository,
            )?;
            report.push(repository.clone());
            roots.push(root.clone());
            let warning = worktree_hooks::run(
                WorktreeHook::Create,
                HookContext {
                    repo: &member.path,
                    worktree: &destination,
                    branch: &branch,
                    id: &root.id,
                },
            )
            .await
            .err()
            .map(|error| {
                format!(
                    "repository `{}` worktree-create hook: {error:#}",
                    member.name
                )
            });
            if let Some(warning) = &warning {
                self.warn_clients(warning.clone());
            }
            if let Some(warning) = warning {
                repository.warning = Some(warning);
                self.store.update_workspace_repository_warning(
                    &workspace.id,
                    &repository.id,
                    repository.warning.as_deref(),
                )?;
                if let Some(saved) = report.last_mut() {
                    saved.warning = repository.warning.clone();
                }
            }
        }

        metadata.state = WorkspaceState::Ready;
        metadata.repositories = report.clone();
        self.store
            .update_session_workspace_repositories(&workspace.id, &report)?;
        self.store
            .set_session_workspace_state(&workspace.id, WorkspaceState::Ready, None)?;
        for root in roots {
            self.broadcast(ServerEvent::EntityUpserted {
                entity: Entity::Worktree(root),
            });
        }
        self.broadcast(ServerEvent::SessionWorkspaceUpserted {
            workspace: metadata,
        });
        self.sync_workspace_member_order(project_id, &workspace.id)?;
        Ok((workspace, report))
    }

    /// Close a complete session workspace. A refusal during either
    /// cleanliness check leaves its rows and processes intact; after the
    /// workspace enters closing, any failure preserves it as unavailable so
    /// the developer can fix the reported cause and retry.
    pub async fn close_session_workspace(self: &Arc<Self>, id: &WorktreeId) -> Result<()> {
        let _ops = self.worktree_ops.lock().await;
        let metadata = self
            .store
            .session_workspaces()?
            .into_iter()
            .find(|workspace| &workspace.worktree_id == id)
            .context("cannot close the main workspace; only session workspaces can be closed")?;
        let workspace = self
            .store
            .get_worktree(id)?
            .context("session workspace not found")?;
        let project = self
            .store
            .get_project(&workspace.project_id)?
            .context("project not found")?;
        let project_repositories = self.store.project_repositories(&project.id)?;

        let faults = session_workspace_dirt(&metadata.repositories, &workspace.path)?;
        if !faults.is_empty() {
            bail!("session workspace is not clean: {}", faults.join(", "));
        }
        if !matches!(
            metadata.state,
            WorkspaceState::Ready | WorkspaceState::Unavailable
        ) {
            bail!(
                "session workspace cannot be closed while it is {:?}",
                metadata.state
            );
        }

        self.store
            .set_session_workspace_state(id, WorkspaceState::Closing, None)?;
        let mut closing = metadata.clone();
        closing.state = WorkspaceState::Closing;
        closing.failure = None;
        self.broadcast(ServerEvent::SessionWorkspaceUpserted {
            workspace: closing.clone(),
        });

        let (_, _, agents, terminals) = self.store.load_tree()?;
        let ids: Vec<_> = std::iter::once(id.clone())
            .chain(
                metadata
                    .repositories
                    .iter()
                    .map(|repository| repository.id.clone()),
            )
            .collect();
        self.kill_sessions_in(&ids, &agents, &terminals);

        let faults = session_workspace_dirt(&metadata.repositories, &workspace.path)?;
        if !faults.is_empty() {
            let message = format!(
                "session workspace changed while closing: {}",
                faults.join(", ")
            );
            self.mark_session_workspace_unavailable(&closing, &message)?;
            bail!("{message}");
        }

        for repository in &metadata.repositories {
            // Missing members are expected only after a prior close removed
            // them before a later operation failed. A same-named folder left
            // behind is not ours to delete; the final root emptiness check
            // reports it and keeps the workspace unavailable.
            if !repository.path.exists() {
                continue;
            }
            let Some(project_repository) = project_repositories
                .iter()
                .find(|member| member.name == repository.name)
            else {
                let message = format!(
                    "repository `{}` is no longer registered (path {})",
                    repository.name,
                    repository.path.display()
                );
                self.mark_session_workspace_unavailable(&closing, &message)?;
                bail!("{message}");
            };
            if let Err(error) =
                git::remove_worktree(&project_repository.path, &repository.path, false).await
            {
                let message = format!(
                    "could not remove repository `{}` at {}: {error:#}",
                    repository.name,
                    repository.path.display()
                );
                self.mark_session_workspace_unavailable(&closing, &message)?;
                bail!("{message}");
            }
            if let Err(error) = self
                .store
                .delete_workspace_repository(&repository.id)
                .and_then(|()| self.store.delete_worktree(&repository.id))
            {
                let message = format!(
                    "repository `{}` was removed from Git at {}, but its Nebula record could not be removed: {error:#}",
                    repository.name,
                    repository.path.display()
                );
                self.mark_session_workspace_unavailable(&closing, &message)?;
                bail!("{message}");
            }
            // Keep the membership record until the entire workspace is
            // removed. If a later repository fails, retry needs this path
            // to distinguish the already-removed checkout from an extra
            // workspace-folder entry.
            self.broadcast(ServerEvent::EntityRemoved {
                id: EntityId::Worktree(repository.id.clone()),
            });
            let deletion_hook = worktree_hooks::run(
                WorktreeHook::Delete,
                HookContext {
                    repo: &project_repository.path,
                    worktree: &repository.path,
                    branch: &workspace.branch,
                    id: &repository.id,
                },
            )
            .await;
            if let Err(error) = deletion_hook {
                self.warn_clients(format!("{error:#}"));
            }
        }

        if let Err(error) = remove_workspace_hook_files(&workspace.path) {
            let message = format!(
                "could not remove Nebula hook files in {}: {error:#}",
                workspace.path.display()
            );
            closing
                .repositories
                .retain(|repository| repository.path.exists());
            self.store
                .update_session_workspace_repositories(id, &closing.repositories)?;
            self.mark_session_workspace_unavailable(&closing, &message)?;
            bail!("{message}");
        }
        let remaining_members: Vec<_> = metadata
            .repositories
            .iter()
            .filter(|repository| repository.path.exists())
            .map(|repository| repository.name.as_str())
            .collect();
        if let Some(fault) =
            workspace_folder_extras_for_names(&workspace.path, &remaining_members)?.first()
        {
            let message = format!(
                "workspace folder {} has unexpected path {}",
                workspace.path.display(),
                fault.display()
            );
            closing
                .repositories
                .retain(|repository| repository.path.exists());
            self.store
                .update_session_workspace_repositories(id, &closing.repositories)?;
            self.mark_session_workspace_unavailable(&closing, &message)?;
            bail!("{message}");
        }
        if let Err(error) = std::fs::remove_dir(&workspace.path) {
            if error.kind() != std::io::ErrorKind::NotFound {
                let remaining = workspace_folder_extras_for_names(
                    &workspace.path,
                    &metadata
                        .repositories
                        .iter()
                        .map(|repository| repository.name.as_str())
                        .collect::<Vec<_>>(),
                )?
                .into_iter()
                .map(|path| path.display().to_string())
                .collect::<Vec<_>>();
                let detail = if remaining.is_empty() {
                    workspace.path.display().to_string()
                } else {
                    remaining.join(", ")
                };
                let message = format!(
                    "workspace folder {} is not empty: {detail} ({error})",
                    workspace.path.display()
                );
                closing
                    .repositories
                    .retain(|repository| repository.path.exists());
                self.store
                    .update_session_workspace_repositories(id, &closing.repositories)?;
                self.mark_session_workspace_unavailable(&closing, &message)?;
                bail!("{message}");
            }
        }

        self.store.delete_session_workspace(id)?;
        self.broadcast(ServerEvent::EntityRemoved {
            id: EntityId::Worktree(id.clone()),
        });
        Ok(())
    }

    fn mark_session_workspace_unavailable(
        &self,
        workspace: &nebula_core::SessionWorkspace,
        failure: &str,
    ) -> Result<()> {
        let mut unavailable = workspace.clone();
        unavailable.state = WorkspaceState::Unavailable;
        unavailable.failure = Some(failure.to_string());
        self.store.set_session_workspace_state(
            &workspace.worktree_id,
            WorkspaceState::Unavailable,
            Some(failure),
        )?;
        self.broadcast(ServerEvent::SessionWorkspaceUpserted {
            workspace: unavailable,
        });
        Ok(())
    }

    async fn rollback_session_workspace(
        &self,
        workspace: &Worktree,
        repositories: &[nebula_core::WorkspaceRepository],
    ) -> (Vec<String>, Vec<String>) {
        let mut leftovers = Vec::new();
        let mut remaining = Vec::new();
        for repository in repositories.iter().rev() {
            let project_repo = self
                .store
                .project_repositories(&workspace.project_id)
                .unwrap_or_default()
                .into_iter()
                .find(|member| member.name == repository.name);
            let Some(project_repo) = project_repo else {
                leftovers.push(format!(
                    "{} ({})",
                    repository.name,
                    repository.path.display()
                ));
                remaining.push(format!(
                    "{} ({})",
                    repository.name,
                    repository.path.display()
                ));
                continue;
            };
            if repository.path.exists() {
                if let Err(error) =
                    git::remove_worktree(&project_repo.path, &repository.path, true).await
                {
                    leftovers.push(format!(
                        "{} ({}): {error:#}",
                        repository.name,
                        repository.path.display()
                    ));
                    remaining.push(format!(
                        "{} ({})",
                        repository.name,
                        repository.path.display()
                    ));
                    continue;
                }
            }
            if !repository.path.exists() {
                let _ = self.store.delete_workspace_repository(&repository.id);
                let _ = self.store.delete_worktree(&repository.id);
            }
            // Preflight proved this branch absent in every member before
            // creation started, so any matching ref is owned by this attempt,
            // including a git add that failed after creating its branch.
            if git::branch_exists(&project_repo.path, &workspace.branch).await {
                if let Err(error) = git::delete_branch(&project_repo.path, &workspace.branch).await
                {
                    leftovers.push(format!(
                        "{} branch {}: {error:#}",
                        repository.name, workspace.branch
                    ));
                    remaining.push(format!("{} branch {}", repository.name, workspace.branch));
                    continue;
                }
            }
            let _ = self.store.delete_workspace_repository(&repository.id);
            let _ = self.store.delete_worktree(&repository.id);
        }
        let _ = self
            .store
            .update_session_workspace_repositories(&workspace.id, repositories);
        if leftovers.is_empty() {
            match std::fs::remove_dir(&workspace.path) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => {
                    leftovers.push(format!(
                        "workspace folder {}: {error}",
                        workspace.path.display()
                    ));
                    if let Ok(entries) = std::fs::read_dir(&workspace.path) {
                        for entry in entries.flatten() {
                            leftovers.push(format!("leftover path {}", entry.path().display()));
                        }
                    }
                }
            }
        }
        (leftovers, remaining)
    }

    fn sync_workspace_member_order(
        &self,
        project_id: &ProjectId,
        workspace_id: &WorktreeId,
    ) -> Result<()> {
        let (_, worktrees, _, _) = self.store.load_tree()?;
        let root = worktrees
            .iter()
            .find(|worktree| &worktree.id == workspace_id)
            .context("session workspace row missing")?;
        let members = self
            .store
            .session_workspaces()?
            .into_iter()
            .find(|workspace| &workspace.worktree_id == workspace_id)
            .map(|workspace| workspace.repositories)
            .unwrap_or_default();
        for (index, member) in members.iter().enumerate() {
            if let Some(mut worktree) = self.store.get_worktree(&member.id)? {
                worktree.sort_order = root.sort_order * 1000 + index as i64;
                self.store
                    .set_worktree_sort_order(&worktree.id, worktree.sort_order)?;
                let _ = project_id;
            }
        }
        Ok(())
    }

    pub fn session_workspace_ready(&self, id: &WorktreeId) -> Result<bool> {
        let workspaces = self.store.session_workspaces()?;
        Ok(workspaces
            .iter()
            .find(|workspace| {
                &workspace.worktree_id == id
                    || workspace
                        .repositories
                        .iter()
                        .any(|repository| &repository.id == id)
            })
            .is_none_or(|workspace| workspace.state == WorkspaceState::Ready))
    }

    /// Tell every client about something that went wrong after a request
    /// had already succeeded. Rides `ServerEvent::Error` with no `req_id`,
    /// which the TUI shows as a flash and ties to no pending intent.
    fn warn_clients(&self, message: String) {
        tracing::warn!("{message}");
        self.broadcast(ServerEvent::Error {
            req_id: None,
            message,
        });
    }

    /// Refresh explicit project member checkouts and session workspace
    /// observations. Unknown repository worktrees are never adopted.
    pub async fn sync_project_worktrees(self: &Arc<Self>, project: &Project) -> Result<()> {
        let members = self.store.project_repositories(&project.id)?;
        if members.is_empty() {
            bail!("project has no registered repositories");
        }
        self.sync_project_repositories(project, &members).await
    }

    /// Resolve a selected repository's checkout from a workspace identity.
    pub fn workspace_repository_path(
        &self,
        workspace: &WorktreeId,
        repository: Option<&str>,
    ) -> Result<PathBuf> {
        let root = self
            .store
            .get_worktree(workspace)?
            .context("workspace not found")?;
        let members = self.store.project_repositories(&root.project_id)?;
        let selected = repository
            .and_then(|name| members.iter().find(|member| member.name == name))
            .or_else(|| members.first())
            .context("workspace has no registered repositories")?;
        if root.is_main {
            Ok(selected.path.clone())
        } else {
            let session = self
                .store
                .session_workspace(workspace)?
                .context("session workspace metadata missing")?;
            session
                .repositories
                .iter()
                .find(|member| member.name == selected.name)
                .map(|member| member.path.clone())
                .context("selected repository is not a workspace member")
        }
    }

    async fn sync_project_repositories(
        &self,
        project: &Project,
        members: &[ProjectRepository],
    ) -> Result<()> {
        let session_workspaces = self.store.session_workspaces()?;
        let mut refreshed_members = Vec::with_capacity(members.len());
        let mut main_changed = false;
        let mut primary_branches = Vec::with_capacity(members.len());
        let mut unavailable_primary: Vec<String> = Vec::new();
        for member in members {
            let branch = git::current_branch(&member.path).await.ok();
            let changed_files = if member.path.is_dir() {
                git::changed_file_count(&member.path).await.ok()
            } else {
                None
            };
            let observed_branch = branch.as_deref().unwrap_or(&member.branch);
            primary_branches.push((member.name.clone(), branch.clone()));
            if !member.path.is_dir() {
                unavailable_primary.push(member.name.clone());
            }
            main_changed |= branch
                .as_deref()
                .is_some_and(|observed| observed != member.branch)
                || (branch.is_some() && changed_files != member.changed_files);
            let persisted_changed_files = if branch.is_some() {
                changed_files
            } else {
                None
            };
            self.store.update_project_repository_state(
                &project.id,
                &member.name,
                observed_branch,
                persisted_changed_files,
            )?;
            let mut refreshed = member.clone();
            refreshed.branch = observed_branch.to_string();
            refreshed.changed_files = persisted_changed_files;
            refreshed_members.push(refreshed);
        }
        if let Some(mut main) = self
            .store
            .load_tree()?
            .1
            .into_iter()
            .find(|worktree| worktree.project_id == project.id && worktree.is_main)
        {
            let branch = primary_branches
                .iter()
                .find(|(name, _)| main.path.ends_with(name))
                .and_then(|(_, branch)| branch.as_ref());
            if let Some(branch) = branch {
                if main.branch != *branch {
                    main.branch = branch.clone();
                    self.store.update_worktree_branch(&main.id, branch)?;
                    self.broadcast(ServerEvent::EntityUpserted {
                        entity: Entity::Worktree(main),
                    });
                }
            }
        }
        if main_changed {
            self.broadcast(ServerEvent::ProjectRepositoriesUpserted {
                project: project.id.clone(),
                repositories: refreshed_members,
            });
        }
        if !unavailable_primary.is_empty() {
            let reason = format!(
                "primary checkout unavailable: {}",
                unavailable_primary.join(", ")
            );
            for workspace in session_workspaces.iter().filter(|workspace| {
                self.store
                    .get_worktree(&workspace.worktree_id)
                    .ok()
                    .flatten()
                    .is_some_and(|root| root.project_id == project.id)
            }) {
                if workspace.state == WorkspaceState::Ready {
                    let mut updated = workspace.clone();
                    updated.state = WorkspaceState::Unavailable;
                    updated.failure = Some(reason.clone());
                    self.store.set_session_workspace_state(
                        &workspace.worktree_id,
                        WorkspaceState::Unavailable,
                        Some(&reason),
                    )?;
                    self.broadcast(ServerEvent::SessionWorkspaceUpserted { workspace: updated });
                }
            }
        }
        let mut changed_worktrees = Vec::new();
        let mut failed_workspaces = Vec::new();
        let mut refreshed_workspaces = Vec::new();
        let mut workspace_changes = false;
        for workspace in session_workspaces.iter().filter(|workspace| {
            matches!(
                workspace.state,
                WorkspaceState::Ready | WorkspaceState::Unavailable
            ) && (workspace.state == WorkspaceState::Ready
                || workspace
                    .failure
                    .as_deref()
                    .is_some_and(|failure| failure.starts_with("repository checkout unavailable:")))
                && self
                    .store
                    .get_worktree(&workspace.worktree_id)
                    .ok()
                    .flatten()
                    .is_some_and(|root| root.project_id == project.id)
        }) {
            let mut missing = Vec::new();
            let mut updated = workspace.clone();
            for repository in &mut updated.repositories {
                let previous_branch = repository.branch.clone();
                let previous_changes = repository.changed_files;
                let checkout_exists = repository.path.is_dir();
                let branch = if checkout_exists {
                    git::current_branch(&repository.path).await.ok()
                } else {
                    None
                };
                let changed_files = if checkout_exists {
                    git::changed_file_count(&repository.path).await.ok()
                } else {
                    None
                };
                if !checkout_exists || branch.is_none() || changed_files.is_none() {
                    missing.push(repository.name.clone());
                }
                let observed_branch = branch.clone();
                repository.branch = branch.or_else(|| previous_branch.clone());
                repository.changed_files = if checkout_exists { changed_files } else { None };
                if checkout_exists {
                    if let Some(branch) = repository.branch.as_deref() {
                        self.store.update_worktree_branch(&repository.id, branch)?;
                    }
                }
                self.store.update_workspace_repository_state(
                    &repository.id,
                    observed_branch.as_deref(),
                    repository.changed_files,
                )?;
                if let (Some(branch), Some(mut row)) = (
                    repository.branch.as_ref(),
                    self.store.get_worktree(&repository.id)?,
                ) {
                    if row.branch != *branch {
                        row.branch = branch.clone();
                        self.store.update_worktree_branch(&row.id, branch)?;
                        changed_worktrees.push(row);
                    }
                }
                workspace_changes |= previous_branch != repository.branch
                    || previous_changes != repository.changed_files;
            }
            if !missing.is_empty() {
                let reason = format!("repository checkout unavailable: {}", missing.join(", "));
                updated.state = WorkspaceState::Unavailable;
                updated.failure = Some(reason.clone());
                self.store.set_session_workspace_state(
                    &workspace.worktree_id,
                    WorkspaceState::Unavailable,
                    Some(&reason),
                )?;
                failed_workspaces.push(updated);
            } else if workspace.state == WorkspaceState::Ready && workspace_changes {
                refreshed_workspaces.push(updated);
            } else if workspace.state == WorkspaceState::Ready {
                // No changes, no wire traffic; persistence is already current.
            } else if workspace.state == WorkspaceState::Unavailable
                && workspace
                    .failure
                    .as_deref()
                    .is_some_and(|failure| failure.starts_with("repository checkout unavailable:"))
            {
                updated.state = WorkspaceState::Ready;
                updated.failure = None;
                self.store.set_session_workspace_state(
                    &workspace.worktree_id,
                    WorkspaceState::Ready,
                    None,
                )?;
                for repository in &updated.repositories {
                    if let Some(mut row) = self.store.get_worktree(&repository.id)? {
                        let branch = repository.branch.as_deref().unwrap_or("unknown");
                        if row.branch != branch {
                            row.branch = branch.to_string();
                            self.store.update_worktree_branch(&row.id, branch)?;
                            changed_worktrees.push(row);
                        }
                    }
                }
                refreshed_workspaces.push(updated);
            }
        }
        for worktree in changed_worktrees {
            self.broadcast(ServerEvent::EntityUpserted {
                entity: Entity::Worktree(worktree),
            });
        }
        for workspace in failed_workspaces {
            self.broadcast(ServerEvent::SessionWorkspaceUpserted { workspace });
        }
        for workspace in refreshed_workspaces {
            let unchanged = session_workspaces
                .iter()
                .find(|known| known.worktree_id == workspace.worktree_id)
                .is_some_and(|known| known == &workspace);
            if !unchanged {
                self.broadcast(ServerEvent::SessionWorkspaceUpserted { workspace });
            }
        }
        Ok(())
    }

    // ---- agents ----

    pub(crate) async fn create_agent(self: &Arc<Self>, spec: CreateAgentSpec) -> Result<EntityId> {
        let CreateAgentSpec {
            worktree: worktree_id,
            name,
            kind,
            custom_harness,
            model,
            effort,
            auto_title,
            cloud_prompt,
            starting_prompt,
            pr_url,
            issue_url,
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
        let issue_url = match issue_url {
            Some(_) if cloud_prompt.is_some() => {
                bail!("issue launch context is not supported for Claude Cloud")
            }
            Some(url) => Some(crate::pr_scope::validate_issue_url(&url)?),
            None => None,
        };
        // Every harness resolves against the current registry before
        // anything spawns: a missing or broken entry refuses the create
        // with its reason, and a deleted entry breaks respawns the same
        // way at boot.
        let harness = resolve_harness(kind, custom_harness.as_deref())?;
        let program = harness.program.trim().to_string();
        // A launch that hands the CLI a first prompt is working from the
        // moment it spawns, so the row says so now instead of staying gray
        // until the CLI has booted and its first hook has landed — seconds
        // in which the session the user just started looked idle and sorted
        // under every session mid-turn.
        let optimistic_run = Self::launch_submits_first_prompt(
            &harness,
            starting_prompt.as_deref(),
            pr_url.is_some() || issue_url.is_some(),
        ) && cloud_prompt.is_none();
        let requested_worktree = self
            .store
            .get_worktree(&worktree_id)?
            .context("worktree not found")?;
        let workspace = self
            .store
            .session_workspaces()?
            .into_iter()
            .find(|workspace| {
                workspace.worktree_id == worktree_id
                    || workspace
                        .repositories
                        .iter()
                        .any(|repository| repository.id == worktree_id)
            });
        if workspace
            .as_ref()
            .is_some_and(|workspace| workspace.state != nebula_core::WorkspaceState::Ready)
        {
            let workspace = workspace.as_ref().unwrap();
            bail!(
                "session workspace is not ready{}",
                workspace
                    .failure
                    .as_ref()
                    .map(|reason| format!(": {reason}"))
                    .unwrap_or_default()
            );
        }
        let worktree = match workspace {
            Some(workspace) if workspace.worktree_id != requested_worktree.id => self
                .store
                .get_worktree(&workspace.worktree_id)?
                .context("session workspace folder not found")?,
            _ => requested_worktree,
        };
        let owning_worktree_id = worktree.id.clone();
        // A warm session for this (worktree, kind) hands over its PTY and
        // its pre-generated id — the CLI booted while the user typed the
        // name, so the create feels instant. A starting prompt rides the
        // CLI's argv, and a spare already booted bare cannot be handed one;
        // neither can it be handed a PR or issue rule.
        let adopted = (cloud_prompt.is_none()
            && pr_url.is_none()
            && issue_url.is_none()
            && starting_prompt.is_none())
        .then(|| self.take_prewarmed(&worktree_id, kind, model.as_deref(), effort.as_deref()))
        .flatten();
        // Only the cold path needs asking: an adopted warm session is proof
        // the CLI runs. Without this, a missing CLI still "succeeds" — the
        // login shell prints `command not found` into a PTY that dies at
        // once, leaving a dead row that looks identical to a fresh one.
        if adopted.is_none() && !self.cli_available_for_create(&program).await {
            bail!("{}", cli_missing_message(&program));
        }
        let agent = Agent {
            id: adopted
                .as_ref()
                .map(|e| e.agent_id.clone())
                .unwrap_or_else(AgentId::generate),
            worktree_id: owning_worktree_id,
            name: if name.trim().is_empty() {
                "agent".into()
            } else {
                name.trim().to_string()
            },
            status: if optimistic_run {
                AgentStatus::Running
            } else {
                AgentStatus::Fresh
            },
            archived: false,
            archived_at: 0,
            unseen: false,
            kind,
            custom_harness: custom_harness
                .as_deref()
                .map(str::trim)
                .filter(|id| !id.is_empty())
                .map(str::to_string),
            model,
            effort,
            session_id: None,
            cloud_session_id: None,
            sort_order: 0,
            status_changed_at: nebula_core::clock::now_ms(),
            alive: false,
            issue_url: issue_url.clone(),
            recent_prompts: Vec::new(),
        };
        self.store.insert_agent_with_launch_context(
            &agent,
            auto_title,
            pr_url.as_deref(),
            issue_url.as_deref(),
        )?;
        // A cloud task is the session's first prompt, and the only one
        // nebula will ever see typed for it: the agent runs in the sandbox,
        // where no `UserPromptSubmit` hook reaches the daemon. Recorded
        // here, the card says what the session was asked to do like any
        // other card's does (issue #92).
        let mut agent = agent;
        if let Some(task) = cloud_prompt.as_deref() {
            if let Some(text) = crate::prompt_history::condense(task) {
                let entry = nebula_core::PromptEntry {
                    text,
                    submitted_at: nebula_core::clock::now_ms(),
                };
                self.store.push_prompt(&agent.id, &entry)?;
                agent.recent_prompts.push(entry);
            }
        }
        let agent = agent;
        if optimistic_run {
            // Seeded by hand, ahead of the spawn, so the CLI's own startup
            // progress-clear cannot green the row out before its turn has
            // begun — and so nothing seeds a plain `running` machine from
            // the row first.
            self.status_machines
                .lock()
                .unwrap()
                .insert(agent.id.clone(), AgentStatusMachine::launching());
        }
        if adopted.is_none() {
            // Cold path: boot the CLI right away.
            let spawned = self.spawn_agent_session_with(
                &agent,
                &worktree,
                DEFAULT_COLS,
                DEFAULT_ROWS,
                cloud_prompt.as_deref(),
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

    /// Does a cold spawn of this launch hand the CLI a first prompt — the
    /// turn that makes a created row `running` before a single hook has
    /// fired? Asked of the two places that decide it, so it cannot drift
    /// from them: [`crate::pr_scope::launch_prompts`], which folds a launch
    /// rule (a PR SESSION's, an ISSUE SESSION's) into the first prompt on a
    /// harness with no system-prompt flag, and the argv builder's prepend
    /// shape, which puts nebula's guidance there
    /// ([`agent_spawn_command_with`]). The rule's text is built per spawn
    /// from the checkout; only whether there *is* one matters here, so a
    /// stand-in stands in for it.
    fn launch_submits_first_prompt(
        harness: &nebula_core::harness::HarnessDescriptor,
        starting_prompt: Option<&str>,
        rule: bool,
    ) -> bool {
        harness.system.prepend_to_first_prompt
            || crate::pr_scope::launch_prompts(
                harness.system.append_flag.is_some(),
                // A row being created has no session to resume.
                false,
                rule.then_some("<rule>"),
                starting_prompt,
            )
            .initial
            .is_some()
    }

    fn rollback_agent_on_spawn_error<T>(&self, id: &AgentId, result: Result<T>) -> Result<T> {
        match result {
            Ok(value) => Ok(value),
            Err(spawn_error) => {
                self.status_machines.lock().unwrap().remove(id);
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
        if !self.session_workspace_ready(worktree_id)? {
            return Ok(());
        }
        if kind == AgentKind::Custom {
            // No warm spares for custom harnesses: the pool is keyed by
            // kind alone and a spare booted for one entry must never be
            // adopted by another. Custom creates stay cold.
            return Ok(());
        }
        let workspace = self
            .store
            .session_workspaces()?
            .into_iter()
            .find(|workspace| {
                workspace.worktree_id == *worktree_id
                    || workspace
                        .repositories
                        .iter()
                        .any(|repository| repository.id == *worktree_id)
            });
        if workspace
            .as_ref()
            .is_some_and(|workspace| workspace.state != nebula_core::WorkspaceState::Ready)
        {
            return Ok(());
        }
        let Some(worktree) = self.store.get_worktree(worktree_id)? else {
            return Ok(());
        };
        let worktree = match workspace {
            Some(workspace) if workspace.worktree_id != *worktree_id => self
                .store
                .get_worktree(&workspace.worktree_id)?
                .unwrap_or(worktree),
            _ => worktree,
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
        if !self.cli_available(kind.cli_program()).await {
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
            custom_harness: None,
            model: model.clone(),
            effort: effort.clone(),
            session_id: None,
            cloud_session_id: None,
            sort_order: 0,
            status_changed_at: 0,
            alive: false,
            issue_url: None,
            recent_prompts: Vec::new(),
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

    /// Drop warm sessions that died or sat unclaimed past the max age —
    /// and, once `prewarm_agents` is switched off, every one of them: a
    /// spare is a real CLI process the user can see (Claude's `/list-agents`
    /// names it beside their own sessions), so the toggle takes it away on
    /// the next sweep rather than leaving it to age out over 15 minutes.
    /// Runs on the daemon's periodic tick.
    pub fn reap_prewarmed(&self) {
        self.reap_prewarmed_with(&crate::config::Config::load());
    }

    fn reap_prewarmed_with(&self, config: &crate::config::Config) {
        let doomed: Vec<AgentId> = {
            let mut pool = self.prewarmed.lock().unwrap();
            let expired: Vec<_> = pool
                .iter()
                .filter(|(_, e)| {
                    !config.prewarm_agents
                        || e.spawned_at.elapsed() > PREWARM_MAX_AGE
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
            self.finished_runs.lock().unwrap().remove(&t.id);
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

    /// Is the harness's CLI on the user's PATH (as their login shell sees
    /// it)? Cached by program: hits for an hour, misses for a minute so a
    /// just-installed CLI gets picked up quickly. Probe trouble (timeout,
    /// spawn error) fails open — a doomed warm spawn is still graceful.
    /// Custom harnesses pass their entry's program; built-ins their CLI.
    async fn cli_available(&self, program: &str) -> bool {
        if std::env::var(env::AGENT_CMD).is_ok() {
            return true; // test override is spawned verbatim
        }
        const OK_TTL: Duration = Duration::from_secs(3600);
        const FAIL_TTL: Duration = Duration::from_secs(60);
        {
            let probes = self.cli_probes.lock().unwrap();
            if let Some((ok, at)) = probes.get(program) {
                if at.elapsed() < if *ok { OK_TTL } else { FAIL_TTL } {
                    return *ok;
                }
            }
        }
        self.probe_cli(program).await
    }

    /// Fill the availability cache for every harness at boot, off the
    /// request loop. Without it the first CreateAgent of a session pays a
    /// full login-shell probe (~1s with a heavy ~/.zshrc) before it can
    /// answer. Custom entries resolve against the current config; a bare
    /// `Custom` kind never reaches the probe — it has no program.
    pub async fn warm_cli_probes(self: &Arc<Self>) {
        // One probe per launchable program in the effective registry, so
        // a repointed program warms the binary that actually launches.
        let mut programs = Vec::new();
        for entry in harness_registry() {
            if entry.problem().is_none() && !programs.contains(&entry.program) {
                programs.push(entry.program.clone());
            }
        }
        for program in programs {
            self.cli_available(program.trim()).await;
        }
    }

    /// Same question, asked on behalf of a create the user just triggered.
    /// A cached *hit* is trusted; a cached *miss* is re-probed, so someone who
    /// installs the CLI and immediately retries isn't told for another minute
    /// that it's missing. Misses are rare, so this costs nothing in practice.
    async fn cli_available_for_create(&self, program: &str) -> bool {
        self.cli_available(program).await || self.probe_cli(program).await
    }

    /// Uncached `command -v` through the user's login shell; caches the answer.
    async fn probe_cli(&self, program: &str) -> bool {
        let check = cli_probe_line(program);
        let mut probe = tokio::process::Command::new(nebula_core::shell::user_shell());
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
            probe.pre_exec(own_session);
        }
        let status = tokio::time::timeout(CLI_PROBE_TIMEOUT, probe.status()).await;
        match status {
            Ok(Ok(status)) => {
                let ok = status.success();
                self.cli_probes
                    .lock()
                    .unwrap()
                    .insert(program.to_string(), (ok, Instant::now()));
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

    /// A hook payload reported the agent CLI's working directory. When that
    /// directory sits inside a *different* worktree of the same project (the
    /// session entered a worktree it created mid-conversation), re-home the
    /// agent row so the tree reflects where the work actually happens.
    /// Fail-soft: any error leaves the row where it is.
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
        // A Cloud row has no local session to restart: the agent runs in
        // the cloud sandbox, and a plain restart would boot a bare CLI with
        // no link to the work. The row's pane says where the session is.
        if agent.cloud_session_id.is_some() {
            bail!("{CLOUD_ROW_NO_LOCAL_SESSION}");
        }
        let worktree = self
            .store
            .get_worktree(&agent.worktree_id)?
            .context("worktree not found")?;
        if !self.session_workspace_ready(&agent.worktree_id)? {
            bail!("session workspace is not ready");
        }
        self.kill_session(&SessionRef::Agent(id.clone()));
        self.spawn_agent_session(&agent, &worktree, DEFAULT_COLS, DEFAULT_ROWS)?;
        let mut broadcast_agent = agent.clone();
        broadcast_agent.alive = true;
        self.broadcast(ServerEvent::EntityUpserted {
            entity: Entity::Agent(broadcast_agent),
        });
        Ok(())
    }

    /// Queue a message on a Cloud session without leaving nebula.
    /// `claude -p <msg> --cloud <id>` is fire-and-forget — the CLI prints
    /// "Sent to cloud session." and returns, and the reply only ever shows
    /// up on the session's page in the browser, which the row's pane links
    /// to. Runs in the row's own checkout like every other CLI call; the
    /// cloud sandbox is where the work happens, so nothing here switches a
    /// branch or touches the tree.
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
                &nebula_core::shell::user_shell(),
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
        // Its next prompt, for the card: the sandbox reports no hook for
        // it, so this is where nebula learns it was asked.
        if let Some(text) = crate::prompt_history::condense(&message) {
            self.record_prompt(id, text);
        }
        Ok(())
    }

    // ---- terminals ----

    pub fn create_terminal(
        self: &Arc<Self>,
        worktree_id: &WorktreeId,
        name: Option<String>,
    ) -> Result<EntityId> {
        if !self.session_workspace_ready(worktree_id)? {
            bail!("session workspace is not ready");
        }
        let requested = self
            .store
            .get_worktree(worktree_id)?
            .context("worktree not found")?;
        let workspace = self
            .store
            .session_workspaces()?
            .into_iter()
            .find(|workspace| {
                workspace.worktree_id == *worktree_id
                    || workspace
                        .repositories
                        .iter()
                        .any(|repository| repository.id == *worktree_id)
            });
        if workspace
            .as_ref()
            .is_some_and(|workspace| workspace.state != nebula_core::WorkspaceState::Ready)
        {
            bail!("session workspace is not ready");
        }
        let worktree = match workspace {
            Some(workspace) if workspace.worktree_id != *worktree_id => self
                .store
                .get_worktree(&workspace.worktree_id)?
                .context("session workspace folder not found")?,
            _ => requested,
        };
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
            run_command: None,
        };
        self.store.insert_terminal(&terminal)?;
        self.spawn_terminal_session(&terminal, &worktree, DEFAULT_COLS, DEFAULT_ROWS)?;
        let terminal = match self
            .store
            .session_workspaces()?
            .into_iter()
            .find(|workspace| {
                workspace
                    .repositories
                    .iter()
                    .any(|repository| repository.id == *worktree_id)
            }) {
            Some(workspace) => {
                self.store
                    .set_terminal_worktree(&terminal.id, &workspace.worktree_id)?;
                let mut updated = terminal;
                updated.worktree_id = workspace.worktree_id;
                updated
            }
            None => terminal,
        };
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
        self.finished_runs.lock().unwrap().remove(id);
        self.store.delete_terminal(id)?;
        self.broadcast(ServerEvent::EntityRemoved {
            id: EntityId::Terminal(id.clone()),
        });
        Ok(())
    }

    // ---- run terminals ----

    /// `r` on a worktree: start its RUN COMMAND — the project's
    /// `run_command` setting (Settings → Project), else `.nebula.json`'s
    /// `run`, read fresh from the worktree's checkout, else the main
    /// checkout's — in the worktree's RUN TERMINAL. A run that already
    /// exited lends its row; one still going is the answer as it stands,
    /// so a second client's `r` never starts a second server.
    pub fn start_run(self: &Arc<Self>, worktree_id: &WorktreeId) -> Result<EntityId> {
        self.start_run_for_repository(worktree_id, None)
    }

    pub fn start_run_for_repository(
        self: &Arc<Self>,
        workspace_id: &WorktreeId,
        repository_name: Option<&str>,
    ) -> Result<EntityId> {
        self.start_run_with_repository(
            workspace_id,
            repository_name,
            &crate::config::Config::load(),
        )
    }

    /// [`Daemon::start_run`] against a given config, for the tests that
    /// can't pin the settings file.
    pub fn start_run_with(
        self: &Arc<Self>,
        worktree_id: &WorktreeId,
        config: &crate::config::Config,
    ) -> Result<EntityId> {
        self.start_run_with_repository(worktree_id, None, config)
    }

    pub fn start_run_with_repository(
        self: &Arc<Self>,
        worktree_id: &WorktreeId,
        repository_name: Option<&str>,
        config: &crate::config::Config,
    ) -> Result<EntityId> {
        if !self.session_workspace_ready(worktree_id)? {
            bail!("session workspace is not ready");
        }
        let requested = self
            .store
            .get_worktree(worktree_id)?
            .context("worktree not found")?;
        let workspace = self
            .store
            .session_workspaces()?
            .into_iter()
            .find(|workspace| {
                workspace.worktree_id == *worktree_id
                    || workspace
                        .repositories
                        .iter()
                        .any(|repository| repository.id == *worktree_id)
            });
        if workspace
            .as_ref()
            .is_some_and(|workspace| workspace.state != nebula_core::WorkspaceState::Ready)
        {
            bail!("session workspace is not ready");
        }
        let worktree = match workspace {
            Some(workspace) if workspace.worktree_id != *worktree_id => self
                .store
                .get_worktree(&workspace.worktree_id)?
                .context("session workspace folder not found")?,
            _ => requested,
        };
        // Held across the check and the spawn, like `ensure_session`: two
        // presses racing must produce one run, not two.
        let _gate = self.spawn_gate.lock().unwrap();
        let run_repo = self.selected_workspace_repository(worktree_id, repository_name)?;
        let run_id = run_repo
            .as_ref()
            .map(|repository| repository.id.clone())
            .unwrap_or_else(|| worktree_id.clone());
        let terminal_name = run_terminal_name(repository_name);
        let existing = self
            .store
            .run_terminals_in(&run_id)?
            .into_iter()
            .find(|terminal| terminal.name == terminal_name);
        if let Some(term) = &existing {
            if self.is_alive(&SessionRef::Terminal(term.id.clone())) {
                return Ok(EntityId::Terminal(term.id.clone()));
            }
        }
        let project_folder = self
            .store
            .get_project(&worktree.project_id)?
            .map_or_else(|| worktree.path.clone(), |p| p.repo_path);
        let (run_root, settings_root) = run_repo
            .map(|repository| (repository.path.clone(), repository.path))
            .unwrap_or_else(|| (worktree.path.clone(), project_folder.clone()));
        let command = match config.run_command(&run_root) {
            Some(command) => command.to_string(),
            None => project_file::lookup(&run_root, &settings_root, ProjectCommand::Run)
                .map_err(anyhow::Error::msg)?
                .context(NO_RUN_COMMAND)?,
        };
        let mut term = match existing {
            Some(mut term) => {
                self.store.set_terminal_run_command(&term.id, &command)?;
                term.run_command = Some(command.clone());
                term
            }
            None => {
                let term = TerminalTab {
                    id: TerminalId::generate(),
                    worktree_id: run_id.clone(),
                    name: terminal_name,
                    sort_order: 0,
                    alive: false,
                    run_command: Some(command.clone()),
                };
                self.store.insert_terminal(&term)?;
                term
            }
        };
        self.finished_runs.lock().unwrap().remove(&term.id);
        let mut run_worktree = worktree.clone();
        run_worktree.path = run_root;
        let spawned = self.spawn_terminal_session(&term, &run_worktree, DEFAULT_COLS, DEFAULT_ROWS);
        tracing::info!(worktree = %run_id, %command, ok = spawned.is_ok(), "run started");
        let id = term.id.clone();
        {
            // Stamped and sent under the sessions lock: a command that exits
            // at once is dropped from the map, and its not-alive upsert sent,
            // only after this one — never the other way round, which would
            // leave every client showing a finished run as still going.
            let sessions = self.sessions.lock().unwrap();
            term.alive = sessions.contains_key(&SessionRef::Terminal(id.clone()));
            self.broadcast(ServerEvent::EntityUpserted {
                entity: Entity::Terminal(term),
            });
        }
        spawned?;
        Ok(EntityId::Terminal(id))
    }

    /// `r` on a running worktree: kill its RUN TERMINAL and drop the row, so
    /// a stopped run leaves nothing behind. Nothing running is not an
    /// error — two clients may both have pressed it.
    pub fn stop_run(self: &Arc<Self>, worktree_id: &WorktreeId) -> Result<()> {
        self.stop_run_for_repository(worktree_id, None)
    }

    pub fn stop_run_for_repository(
        self: &Arc<Self>,
        worktree_id: &WorktreeId,
        repository_name: Option<&str>,
    ) -> Result<()> {
        let run_id = self
            .selected_workspace_repository(worktree_id, repository_name)?
            .map(|repository| repository.id)
            .unwrap_or_else(|| worktree_id.clone());
        let terminal_name = run_terminal_name(repository_name);
        for term in self
            .store
            .run_terminals_in(&run_id)?
            .into_iter()
            .filter(|terminal| terminal.name == terminal_name)
        {
            tracing::info!(worktree = %run_id, %terminal_name, "run stopped");
            self.close_terminal(&term.id)?;
        }
        Ok(())
    }

    fn selected_workspace_repository(
        &self,
        workspace_id: &WorktreeId,
        repository_name: Option<&str>,
    ) -> Result<Option<nebula_core::WorkspaceRepository>> {
        let Some(name) = repository_name else {
            return Ok(None);
        };
        let workspace = self.store.session_workspace(workspace_id)?;
        if let Some(workspace) = workspace {
            return workspace
                .repositories
                .into_iter()
                .find(|repository| repository.name == name)
                .map(Some)
                .ok_or_else(|| anyhow::anyhow!("repository `{name}` is not in this workspace"));
        }
        let root = self
            .store
            .get_worktree(workspace_id)?
            .context("selected workspace not found")?;
        let repository = self
            .store
            .project_repositories(&root.project_id)?
            .into_iter()
            .find(|repository| repository.name == name)
            .ok_or_else(|| anyhow::anyhow!("repository `{name}` is not in this workspace"))?;
        Ok(Some(nebula_core::WorkspaceRepository {
            id: workspace_id.clone(),
            name: repository.name,
            path: repository.path,
            base: Some(repository.branch.clone()),
            fetch_succeeded: true,
            warning: None,
            branch: Some(repository.branch.clone()),
            changed_files: repository.changed_files,
        }))
    }

    /// Whether `id` is a RUN TERMINAL's row.
    fn is_run_terminal(&self, id: &TerminalId) -> bool {
        self.store.get_terminal(id).ok().flatten().is_some_and(|t| {
            t.run_command.is_some() && (t.name == RUN_TERMINAL_NAME || t.name.starts_with("run:"))
        })
    }

    /// A RUN TERMINAL that exited on its own: its exit code (None when the
    /// OS reported none), for the attach replaying it. None for a live
    /// session or anything that is not a finished run.
    pub fn finished_run_exit(&self, sref: &SessionRef) -> Option<Option<i32>> {
        let SessionRef::Terminal(id) = sref else {
            return None;
        };
        self.finished_runs
            .lock()
            .unwrap()
            .get(id)
            .map(|(_, code)| *code)
    }

    // ---- links ----

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
        if let Some(worktree_id) = match sref {
            SessionRef::Agent(id) => self.store.get_agent(id)?.map(|agent| agent.worktree_id),
            SessionRef::Terminal(id) => self
                .store
                .get_terminal(id)?
                .map(|terminal| terminal.worktree_id),
        } {
            if !self.session_workspace_ready(&worktree_id)? {
                bail!("session workspace is not ready");
            }
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
                // A Cloud row's only PTY is the `claude --cloud <task>`
                // create, gone seconds after it prints the session id. There
                // is nothing to bring back: the agent runs in the cloud, and
                // a spawn here would be a bare local CLI wearing its name.
                if agent.cloud_session_id.is_some() {
                    bail!("{CLOUD_ROW_NO_LOCAL_SESSION}");
                }
                let worktree = self
                    .store
                    .get_worktree(&agent.worktree_id)?
                    .context("worktree not found")?;
                let workspace = self
                    .store
                    .session_workspaces()?
                    .into_iter()
                    .find(|workspace| {
                        workspace.worktree_id == agent.worktree_id
                            || workspace
                                .repositories
                                .iter()
                                .any(|repository| repository.id == agent.worktree_id)
                    });
                if workspace
                    .as_ref()
                    .is_some_and(|workspace| workspace.state != nebula_core::WorkspaceState::Ready)
                {
                    bail!("session workspace is not ready");
                }
                let worktree = match workspace {
                    Some(workspace) if workspace.worktree_id != agent.worktree_id => self
                        .store
                        .get_worktree(&workspace.worktree_id)?
                        .context("session workspace folder not found")?,
                    _ => worktree,
                };
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
                // A RUN TERMINAL is only ever started by `r`: an attach, or
                // the prewarm sweep walking past, must never run a command
                // that exited again. What it can show is how the run ended,
                // while the DAEMON still holds that PTY.
                if term.run_command.is_some()
                    && (term.name == RUN_TERMINAL_NAME || term.name.starts_with("run:"))
                {
                    if let Some((session, _)) = self.finished_runs.lock().unwrap().get(id) {
                        return Ok(session.clone());
                    }
                    bail!("{RUN_NOT_RUNNING}");
                }
                let worktree = self
                    .store
                    .get_worktree(&term.worktree_id)?
                    .context("worktree not found")?;
                let workspace = self
                    .store
                    .session_workspaces()?
                    .into_iter()
                    .find(|workspace| {
                        workspace.worktree_id == term.worktree_id
                            || workspace
                                .repositories
                                .iter()
                                .any(|repository| repository.id == term.worktree_id)
                    });
                if workspace
                    .as_ref()
                    .is_some_and(|workspace| workspace.state != nebula_core::WorkspaceState::Ready)
                {
                    bail!("session workspace is not ready");
                }
                let worktree = match workspace {
                    Some(workspace) if workspace.worktree_id != term.worktree_id => self
                        .store
                        .get_worktree(&workspace.worktree_id)?
                        .context("session workspace folder not found")?,
                    _ => worktree,
                };
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
        if !self.session_workspace_ready(worktree_id).unwrap_or(false) {
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
        if !self.session_workspace_ready(worktree_id).unwrap_or(false)
            && self
                .store
                .is_session_workspace(worktree_id)
                .unwrap_or(false)
        {
            return;
        }
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
                    // A RUN TERMINAL never boots from a sweep (see
                    // `ensure_session`).
                    .filter(|t| &t.worktree_id == worktree_id && t.run_command.is_none())
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

    /// The general spawn: `cloud_task` makes it a Claude Cloud dispatch
    /// (`claude --cloud <task>`, which creates the session, prints its id
    /// and exits), `initial_prompt` a first turn the CLI submits on its own
    /// (the prefix + task + postfix an AGENT PRESET launch composes). Both
    /// are intentionally transient: later restarts/resumes follow the
    /// persisted Agent fields — a Cloud row's `cloud_session_id` makes
    /// `restart_agent` and `ensure_session` refuse to boot a local CLI for
    /// it, everything else takes the plain local-session path.
    fn spawn_agent_session_with(
        self: &Arc<Self>,
        agent: &Agent,
        worktree: &Worktree,
        cols: u16,
        rows: u16,
        cloud_task: Option<&str>,
        initial_prompt: Option<&str>,
    ) -> Result<Arc<PtySession>> {
        // A session the user sent to Claude's background (`/background`)
        // can't be resumed, only attached to — see `claude_bg`. The probe
        // costs a login shell, so it hides behind the one-`stat` hint.
        let attach = if cloud_task.is_none() && self.claude_job_hint(agent) {
            self.claude_background_id(agent)
        } else {
            None
        };
        self.spawn_agent_pty(
            agent,
            worktree,
            cols,
            rows,
            cloud_task,
            initial_prompt,
            attach.as_deref(),
        )
    }

    /// [`Self::spawn_agent_session_with`] past its look for a backgrounded
    /// Claude session: `attach` is the id `claude attach` takes, and wins
    /// over a resume of the stored session id — and over `initial_prompt`,
    /// which `attach` has no way to submit.
    #[allow(clippy::too_many_arguments)]
    fn spawn_agent_pty(
        self: &Arc<Self>,
        agent: &Agent,
        worktree: &Worktree,
        cols: u16,
        rows: u16,
        cloud_task: Option<&str>,
        initial_prompt: Option<&str>,
        attach: Option<&str>,
    ) -> Result<Arc<PtySession>> {
        // Every row resolves its registry descriptor once, up front: a row
        // whose entry was deleted or broken since refuses the boot with
        // its reason rather than launching the wrong CLI, and the same
        // descriptor picks the hook dialect below.
        let harness = resolve_harness(agent.kind, agent.custom_harness.as_deref())?;
        // Managed status hooks; a failure here degrades to "no status
        // updates", never blocks the spawn. The dialect is data: a custom
        // harness naming one reports status, prompts and permission waits
        // exactly like that harness, and a harness with none runs
        // hookless (process-based status until a dialect is mapped).
        let install_result = match harness.hook_dialect() {
            Some(AgentKind::Claude) => hooks::installer::install_claude_hooks(&worktree.path),
            // Codex's hooks live in its home, not the worktree, so one
            // trust approval covers every worktree (see installer docs);
            // any per-worktree copy an older nebula left is pruned.
            Some(AgentKind::Codex) => {
                hooks::installer::install_codex_hooks(&hooks::installer::codex_home())
                    .and_then(|()| hooks::installer::prune_codex_worktree_hooks(&worktree.path))
            }
            // Cursor also gets the managed auto-title project rule — its
            // hook dialect has no context-injection channel.
            Some(AgentKind::Cursor) => hooks::installer::install_cursor_hooks(&worktree.path)
                .and_then(|()| hooks::installer::install_cursor_title_rule(&worktree.path)),
            // Pi runs TypeScript extensions, not shell hooks: one managed
            // extension in its global agent dir (loaded without the trust
            // prompt a worktree-local `.pi/extensions/` would raise) serves
            // every worktree.
            Some(AgentKind::Pi) => {
                hooks::pi_extension::install(&hooks::pi_extension::pi_agent_dir())
            }
            // OpenCode runs TypeScript plugins, likewise: one managed
            // plugin in its global config dir (globbed at startup, no
            // trust prompt) serves every worktree.
            Some(AgentKind::OpenCode) => {
                hooks::opencode_plugin::install(&hooks::opencode_plugin::opencode_config_dir())
            }
            _ => Ok(()),
        };
        if let Err(e) = install_result {
            tracing::warn!(error = %e, cwd = %worktree.path.display(), "hook install failed");
        }

        // NEBULA_AGENT_CMD overrides for tests; default is the kind's CLI.
        let cmd_override = std::env::var(env::AGENT_CMD).ok();
        // A Claude session id with no transcript behind it — a CLI nobody
        // sent a prompt, or a session Claude's cleanup has deleted — resumes
        // into "No conversation found" and a dead pane: boot fresh instead.
        // An override (tests) never resumes, so it skips the look.
        let unresumable;
        let agent = match agent.session_id.as_deref() {
            Some(sid)
                if agent.kind == AgentKind::Claude
                    && cloud_task.is_none()
                    && cmd_override.is_none()
                    && attach.is_none()
                    && claude_transcript_exists(&self.claude_projects_dirs(), sid)
                        == Some(false) =>
            {
                tracing::info!(agent = %agent.id, session = %sid, "no Claude transcript for the session — spawning fresh");
                if let Err(e) = self.store.set_agent_session_id(&agent.id, None) {
                    tracing::warn!(agent = %agent.id, error = %e, "clear session id failed");
                }
                let mut fresh = agent.clone();
                fresh.session_id = None;
                unresumable = fresh;
                &unresumable
            }
            _ => agent,
        };
        // A PR SESSION's rule — or an ISSUE SESSION's — rides Claude's
        // system prompt, or opens a Codex / Cursor cold spawn as its first
        // prompt (see `pr_scope`). Rebuilt from the row's *current*
        // worktree on every spawn, so a relocated session is told where it
        // now works.
        let (pr_url, issue_url) = if cloud_task.is_none() {
            (
                self.store.agent_pr_url(&agent.id)?,
                self.store.agent_issue_url(&agent.id)?,
            )
        } else {
            (None, None)
        };
        let root = match &pr_url {
            Some(_) if !worktree.is_main => self
                .store
                .get_project(&worktree.project_id)?
                .map(|p| p.repo_path),
            _ => None,
        };
        let scope = pr_url.as_deref().map(|url| crate::pr_scope::PrScope {
            url,
            worktree: &worktree.path,
            branch: &worktree.branch,
            root: root.as_deref(),
        });
        let issue_scope = issue_url.as_deref().map(|url| crate::pr_scope::IssueScope {
            url,
            worktree: &worktree.path,
            branch: &worktree.branch,
        });
        let rule = crate::pr_scope::combined_rule(scope.as_ref(), issue_scope.as_ref());
        let prompts = crate::pr_scope::launch_prompts(
            harness.system.append_flag.is_some(),
            agent.session_id.is_some(),
            rule.as_deref(),
            initial_prompt,
        );
        let (program, args, resumed) = match (cloud_task, attach) {
            (Some(task), _) => claude_cloud_spawn_command(
                &harness,
                task,
                agent.model.as_deref(),
                agent.effort.as_deref(),
                cmd_override.as_deref(),
            ),
            // Never a watched resume: an attach that dies at once keeps the
            // row's session id, and the pane keeps the CLI's reason.
            (None, Some(id)) => {
                if prompts.initial.is_some() {
                    tracing::warn!(agent = %agent.id, "backgrounded Claude session — `claude attach` takes no prompt, dropping the initial one");
                }
                (
                    harness.program.trim().to_string(),
                    claude_bg::attach_args(id),
                    false,
                )
            }
            (None, None) => agent_spawn_command_with(
                &harness,
                agent.session_id.as_deref(),
                Some(&worktree.path),
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
        // path_helper) instead of the daemon's inherited-at-boot env, and
        // resolves the CLI the way a typed command would — an alias or
        // function in those files wins over the binary on PATH.
        // Overrides (tests) stay verbatim.
        let (program, args) = if cmd_override.is_some() {
            (program, args)
        } else {
            login_shell_agent_wrap(&nebula_core::shell::user_shell(), &program, &args)?
        };

        let spec = SpawnSpec {
            program,
            args,
            cwd: worktree.path.clone(),
            env: vec![
                // The child agent's shell may resolve `nebula` to an
                // installed upstream binary after loading its login config.
                // Carry this daemon's socket location so any compatible CLI
                // reaches this fork even when PATH order changes.
                (
                    env::RUNTIME_DIR.into(),
                    nebula_core::paths::runtime_dir().display().to_string(),
                ),
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
        // Recorded before the install, so a CLI that dies at once still
        // finds its watch when `watch_for_exit` sees it go.
        {
            let mut resumes = self.resumes.lock().unwrap();
            if resumed {
                resumes.insert(
                    agent.id.clone(),
                    ResumeWatch {
                        session: Arc::downgrade(&session),
                        spawned_at: Instant::now(),
                        cols,
                        rows,
                    },
                );
            } else {
                resumes.remove(&agent.id);
            }
        }
        self.install_session(session.clone());
        // The create prints the session id and exits at once: capture it
        // off the output (`watch_for_exit` persists it and re-broadcasts the
        // row), which is what turns the row's pane into the link panel.
        if cloud_task.is_some() {
            session.arm_cloud_scan();
        }
        Ok(session)
    }

    /// A resumed session (`claude --resume` / `codex resume` /
    /// `cursor-agent --resume`) that died inside [`RESUME_FAIL_WINDOW`]
    /// could not find its session: clear the id and boot fresh instead of
    /// leaving a dead pane. Reached from `watch_for_exit` on a natural death
    /// only, so a restart or relocation that killed the PTY on purpose (and
    /// has respawned it already) never gets a second CLI. (`pi --session-id`
    /// creates a missing id instead of dying, so pi never lands here.)
    fn respawn_failed_resume(self: &Arc<Self>, id: &AgentId, cols: u16, rows: u16) {
        // `ensure_session`'s gate: an Attach reaching for the dead session
        // right now must not fork a CLI beside this one.
        let _gate = self.spawn_gate.lock().unwrap();
        if self.is_alive(&SessionRef::Agent(id.clone())) {
            return; // respawned already, and that spawn is watched itself
        }
        // Archived or deleted inside the window: never resurrect those.
        let Ok(Some(mut agent)) = self.store.get_agent(id) else {
            return;
        };
        if agent.archived || agent.cloud_session_id.is_some() {
            return;
        }
        let Some(sid) = agent.session_id.take() else {
            return;
        };
        // A Claude transcript still on disk says the id is good and the CLI
        // quit over something else — a bad flag, a login: keep the id for
        // the next attach, and the pane keeps the CLI's reason.
        if agent.kind == AgentKind::Claude
            && claude_transcript_exists(&self.claude_projects_dirs(), &sid) == Some(true)
        {
            // One reason Claude refuses a good id: the session runs in its
            // background daemon now, and only `claude attach` opens it. The
            // spawn's own look missed it — its job-dir hint is Claude's
            // private layout, free to move — so this one asks outright.
            agent.session_id = Some(sid);
            if let Some(attach) = self.claude_background_id(&agent) {
                let Ok(Some(worktree)) = self.store.get_worktree(&agent.worktree_id) else {
                    return;
                };
                tracing::info!(agent = %id, attach = %attach, "resume refused for a backgrounded session — attaching");
                if self
                    .spawn_agent_pty(&agent, &worktree, cols, rows, None, None, Some(&attach))
                    .is_ok()
                {
                    agent.alive = true;
                    self.broadcast(ServerEvent::EntityUpserted {
                        entity: Entity::Agent(agent),
                    });
                }
                return;
            }
            tracing::info!(agent = %id, "resume failed fast with its transcript intact — keeping the session id");
            return;
        }
        let Ok(Some(worktree)) = self.store.get_worktree(&agent.worktree_id) else {
            return;
        };
        tracing::info!(agent = %id, "resume failed fast — respawning fresh");
        if let Err(e) = self.store.set_agent_session_id(id, None) {
            tracing::warn!(agent = %id, error = %e, "clear session id failed");
        }
        if self
            .spawn_agent_session(&agent, &worktree, cols, rows)
            .is_ok()
        {
            agent.alive = true;
            self.broadcast(ServerEvent::EntityUpserted {
                entity: Entity::Agent(agent),
            });
        }
    }

    /// The resume watch `session` was spawned under, while it is still the
    /// agent's latest — taken, so each is judged once.
    fn take_resume_watch(&self, id: &AgentId, session: &Arc<PtySession>) -> Option<ResumeWatch> {
        let mut resumes = self.resumes.lock().unwrap();
        match resumes.get(id) {
            Some(watch) if std::ptr::eq(watch.session.as_ptr(), Arc::as_ptr(session)) => {
                resumes.remove(id)
            }
            _ => None,
        }
    }

    /// Whether Claude keeps a background job under `agent`'s session id —
    /// the cheap look that earns [`Self::claude_background_id`] its probe.
    /// The job dirs sit beside the projects dirs, in the same config dir.
    fn claude_job_hint(&self, agent: &Agent) -> bool {
        let Some(sid) = claude_resumable_session(agent) else {
            return false;
        };
        self.claude_projects_dirs()
            .iter()
            .filter_map(|projects| projects.parent())
            .any(|config| claude_bg::job_hint(config, sid))
    }

    /// The id `claude attach` takes for `agent`'s session, when `claude
    /// agents --json` lists it as a background session. The listing runs
    /// through the login shell the agent itself would: the same `claude`,
    /// the same `CLAUDE_CONFIG_DIR`.
    fn claude_background_id(&self, agent: &Agent) -> Option<String> {
        let sid = claude_resumable_session(agent)?;
        let listing = ["agents".to_string(), "--json".to_string()];
        let (program, args) = login_shell_wrap(
            &nebula_core::shell::user_shell(),
            agent.kind.cli_program(),
            &listing,
        );
        let id = claude_bg::probe(&program, &args, sid)?;
        tracing::info!(agent = %agent.id, session = %sid, attach = %id, "Claude session runs in the background");
        Some(id)
    }

    /// Every Claude projects dir a transcript may sit in: the ones this
    /// daemon's Claude hooks reported (wherever the agent's shell pointed
    /// `CLAUDE_CONFIG_DIR`), then the default.
    fn claude_projects_dirs(&self) -> Vec<PathBuf> {
        let mut dirs: Vec<PathBuf> = self
            .transcripts
            .lock()
            .unwrap()
            .values()
            .filter_map(|t| Some(t.transcript_path.parent()?.parent()?.to_path_buf()))
            .collect();
        dirs.extend(nebula_core::paths::claude_config_dir().map(|dir| dir.join("projects")));
        dirs.sort();
        dirs.dedup();
        dirs
    }

    fn spawn_terminal_session(
        self: &Arc<Self>,
        terminal: &TerminalTab,
        worktree: &Worktree,
        cols: u16,
        rows: u16,
    ) -> Result<Arc<PtySession>> {
        let (program, args) = match &terminal.run_command {
            // A RUN TERMINAL runs its command line through the login +
            // interactive shell an agent launch uses, so `npm` or `bun`
            // resolve the way they do typed. The PTY lives exactly as long
            // as the command, which is what makes it the RUNNING state.
            Some(command) => login_shell_line(&nebula_core::shell::user_shell(), command),
            // `-l` makes it a login shell, matching Terminal.app: zsh then
            // sources /etc/zprofile (path_helper), ~/.zprofile, and ~/.zshrc.
            None => (nebula_core::shell::user_shell(), vec!["-l".into()]),
        };
        let spec = SpawnSpec {
            program,
            args,
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
                        // Taken on any exit, so a killed resume leaves no
                        // record behind; acted on for a natural death only.
                        let resume = match &sref {
                            SessionRef::Agent(id) => daemon.take_resume_watch(id, &session),
                            SessionRef::Terminal(_) => None,
                        };
                        if !was_registered {
                            break;
                        }
                        tracing::info!(session = ?sref, exit_code, "session exited");
                        // A run that ended on its own keeps its PTY for the
                        // attach that wants to read how it ended.
                        if let SessionRef::Terminal(id) = &sref {
                            if daemon.is_run_terminal(id) {
                                daemon
                                    .finished_runs
                                    .lock()
                                    .unwrap()
                                    .insert(id.clone(), (session.clone(), exit_code));
                            }
                        }
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
                        if let (SessionRef::Agent(id), Some(watch)) = (&sref, resume) {
                            if exit_code.unwrap_or(1) != 0
                                && watch.spawned_at.elapsed() < RESUME_FAIL_WINDOW
                            {
                                daemon.respawn_failed_resume(id, watch.cols, watch.rows);
                            }
                        }
                        break;
                    }
                    // The CLI's own busy/idle bit, read off its output. It is
                    // the only end-of-turn news after a user cancel: Claude
                    // Code fires no Stop for an interrupted turn, and
                    // suppresses the idle notification because the user just
                    // pressed a key. See `pty::progress`. Only while this PTY
                    // is still the session's: a killed CLI clears its bar as
                    // it dies, and that must not speak for the replacement a
                    // restart or relocation has installed by then.
                    Ok(PtyEvent::Progress { busy }) => {
                        if let SessionRef::Agent(id) = &sref {
                            if daemon.owns_session(&sref, &session) {
                                daemon.apply_hook_event(id, HookEvent::Progress { busy }, None);
                            }
                        }
                    }
                    // The window title carries Claude's session name, and
                    // `/rename` fires no hook — this is the cue to read the
                    // title it persisted (see `session_title`).
                    Ok(PtyEvent::Title { title }) => {
                        if let SessionRef::Agent(id) = &sref {
                            daemon.on_pty_title(id, &title);
                        }
                    }
                    // The Cloud session this row launched, read off the
                    // `claude --cloud` output. Persisted at once — the child
                    // is typically gone within milliseconds of printing it —
                    // and re-broadcast so the row grows its `cloud` badge and
                    // its pane becomes the panel linking to the session.
                    Ok(PtyEvent::CloudTitle { title }) => {
                        // Claude Cloud's own name for the session names the
                        // row, when nothing else has: the agent runs where
                        // no hook reaches nebula, so the AUTO-TITLE a local
                        // session gives itself never comes (issue #92). A
                        // name the user typed stands.
                        if let SessionRef::Agent(id) = &sref {
                            let title = sanitize_title(&title);
                            if title.is_empty() {
                                continue;
                            }
                            match daemon.store.rename_agent_if_auto_pending(id, &title) {
                                Ok(true) => {
                                    tracing::info!(agent = %id, %title, "cloud session title adopted");
                                    daemon.try_broadcast_agent(id);
                                }
                                Ok(false) => {}
                                Err(e) => {
                                    tracing::warn!(agent = %id, error = %e, "cloud session title not persisted")
                                }
                            }
                        }
                    }
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
                            if daemon.owns_session(&sref, &session) {
                                daemon.apply_hook_event(id, HookEvent::Progress { busy }, None);
                            }
                        }
                        continue;
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
        });
    }
}

/// The session id a spawn of `agent` would hand `claude --resume` — the
/// only kind of session Claude can have sent to its background. None under
/// the `NEBULA_AGENT_CMD` override (tests), which never resumes.
fn claude_resumable_session(agent: &Agent) -> Option<&str> {
    if agent.kind != AgentKind::Claude || std::env::var(env::AGENT_CMD).is_ok() {
        return None;
    }
    agent.session_id.as_deref()
}

/// Whether Claude Code still holds the transcript `claude --resume <id>`
/// reads — `<projects>/<cwd slug>/<id>.jsonl` under any of `roots`, in any
/// slug: a session `nebula worktree` relocated keeps its transcript where
/// it began, and resumes from there. `None` when no root could be read at
/// all, which is no verdict.
fn claude_transcript_exists(roots: &[PathBuf], session_id: &str) -> Option<bool> {
    let file = format!("{session_id}.jsonl");
    // An id that isn't one plain file name names no transcript.
    if Path::new(&file).file_name() != Some(std::ffi::OsStr::new(&file)) {
        return Some(false);
    }
    let mut read_any = false;
    for root in roots {
        let Ok(slugs) = std::fs::read_dir(root) else {
            continue;
        };
        read_any = true;
        if slugs
            .flatten()
            .any(|slug| slug.path().join(&file).is_file())
        {
            return Some(true);
        }
    }
    read_any.then_some(false)
}

/// Program + args for an agent PTY. An override (tests) is used verbatim —
/// no resume args. Otherwise the kind picks the CLI and its resume shape:
/// `claude --resume <sid>` and `cursor-agent --resume <sid>` (flag) vs
/// `codex resume <sid> --cd <cwd>` (subcommand, so resume args must lead;
/// the `--cd` because codex reopens a resumed session in the directory its
/// transcript recorded — or asks which to use — unless told one, and a
/// relocated session must land in its new worktree, not the old checkout);
/// pi takes `pi --session-id <sid>`, which resumes the id where it exists
/// and creates it where it doesn't (a relocated session's new cwd). `cwd`
/// is the checkout every local spawn boots in, and None for a Cloud launch,
/// which has no local one. Codex and cursor always get their
/// skip-permissions flag (`--yolo` / `--force`), appended after the resume
/// args — same convention as Mission Control; pi has no permission gate to
/// skip.
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
/// The plain shape, as every restart/resume spawns it: booted in
/// `TEST_CWD`, no initial prompt, guidance on. Tests assert against this;
/// the daemon calls the full form. The descriptor resolves from a pinned
/// registry so tests never touch the user's config.
#[cfg(test)]
fn agent_spawn_command(
    kind: AgentKind,
    session_id: Option<&str>,
    model: Option<&str>,
    effort: Option<&str>,
    cmd_override: Option<&str>,
) -> (String, Vec<String>, bool) {
    let all = test_registry();
    let harness = test_harness(&all, kind);
    agent_spawn_command_with(
        &harness,
        session_id,
        Some(Path::new(TEST_CWD)),
        model,
        effort,
        cmd_override,
        None,
        None,
        true,
    )
}

/// A pinned registry for spawn tests: the compiled-in rows, no overrides,
/// no legacy entries — what a fresh install launches.
#[cfg(test)]
fn test_registry() -> Vec<nebula_core::harness::HarnessDescriptor> {
    harness_registry_in(&std::collections::BTreeMap::new(), &[])
}

/// The pinned descriptor `kind` launches as in spawn tests.
#[cfg(test)]
fn test_harness(
    all: &[nebula_core::harness::HarnessDescriptor],
    kind: AgentKind,
) -> nebula_core::harness::HarnessDescriptor {
    resolve_harness_in(kind, None, all).expect("built-ins resolve from a pinned registry")
}

/// The pinned descriptor for a legacy custom entry in spawn tests.
#[cfg(test)]
fn test_custom_harness(
    all: &[nebula_core::harness::HarnessDescriptor],
    id: &str,
) -> nebula_core::harness::HarnessDescriptor {
    resolve_harness_in(AgentKind::Custom, Some(id), all).expect("the pinned entry resolves")
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

/// The prompt a relocated session is resumed with: it names the checkout
/// the process now runs in and asks for the work to pick back up there, so
/// the user never has to type "continue". Only for the CLIs verified to
/// open a resumed session on a trailing prompt — `claude --resume <sid>
/// "<prompt>"`, `codex resume <sid> [PROMPT]` (the positional its `--help`
/// documents; submitted as the next turn, verified live on codex 0.153.4)
/// and `pi --session-id <sid> "<prompt>"`. Whether `cursor-agent --resume
/// <id> [prompt...]` submits one is not, so a relocated cursor session
/// resumes silent and waits for the user.
fn relocation_prompt(relocate: bool, worktree: &Worktree) -> Option<String> {
    relocate.then(|| {
        format!(
            "[nebula] This session now runs inside the worktree `{}` at {} — your working \
             directory is that checkout. Continue the user's most recent request there.",
            worktree.branch,
            worktree.path.display()
        )
    })
}

/// The checkout the test wrapper boots every spawn in.
#[cfg(test)]
const TEST_CWD: &str = "/nebula-test/p-feat";

// Nine positional knobs are two over clippy's line; the callers are the
// two thin wrappers above and the tests, so a builder would only add
// ceremony. Every behavior comes off `harness` — the registry descriptor
// the launch resolved — never off the kind: a repointed program, a renamed
// flag or a whole new CLI flows through here with no new arms.
#[allow(clippy::too_many_arguments)]
fn agent_spawn_command_with(
    harness: &nebula_core::harness::HarnessDescriptor,
    session_id: Option<&str>,
    cwd: Option<&Path>,
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
            parts.push(harness.program.trim().to_string());
        }
        let program = parts.remove(0);
        return (program, parts, false);
    }
    let program = harness.program.trim().to_string();
    // Resume: a flag (`--resume <id>`), a positional subcommand
    // (`resume <id>`, with `--cd`), or nothing — a stored id with no
    // resume mapping is ignored and the CLI boots fresh.
    let (mut args, resumed) = match (&harness.resume.flag, &harness.resume.subcommand, session_id) {
        (Some(flag), _, Some(sid)) => (vec![flag.clone(), sid.to_string()], true),
        (None, Some(subcommand), Some(sid)) => {
            let mut args = vec![subcommand.clone(), sid.to_string()];
            if harness.resume.cd {
                if let Some(cwd) = cwd {
                    args.extend(["--cd".to_string(), cwd.to_string_lossy().into_owned()]);
                }
            }
            (args, true)
        }
        _ => (Vec::new(), false),
    };
    if let Some(flag) = harness.permissions_flag.as_deref() {
        args.push(flag.to_string());
    }
    // The model rides its flag — composed with the effort into one id for
    // Cursor's family-suffix shape (`claude-opus-5` + `high`: the TUI only
    // sends an effort the family ships; an effort without a family has
    // nothing to hang off and is dropped).
    if let (Some(m), Some(flag)) = (model, harness.model.flag.as_deref()) {
        let id = match (harness.compose_model_effort, effort) {
            (true, Some(e)) => format!("{m}-{e}"),
            _ => m.to_string(),
        };
        args.extend([flag.to_string(), id]);
    }
    // The effort rides its flag, Codex's `-c key=value` pair, or nothing
    // (composed above, or unmapped and dropped like before).
    if let Some(e) = effort {
        if !harness.compose_model_effort {
            if let Some(flag) = harness.effort.flag.as_deref() {
                args.extend([flag.to_string(), e.to_string()]);
            } else if let (Some(flag), Some(key)) = (
                harness.effort.config_flag.as_deref(),
                harness.effort.config_key.as_deref(),
            ) {
                args.extend([flag.to_string(), format!("{key}={e}")]);
            }
        }
    }
    // Guidance (worktree rules, then the PR scope) rides the
    // system-prompt flag where one is mapped, folds into the first prompt
    // where prepend is, and is dropped where neither is.
    let mut initial_prompt = initial_prompt.map(str::to_string);
    if let Some(flag) = harness.system.append_flag.as_deref() {
        push_system_prompt(&mut args, flag, guidance, additional_system_prompt);
    } else if harness.system.prepend_to_first_prompt {
        let mut first = Vec::new();
        if guidance {
            first.push(CLAUDE_WORKTREE_GUIDANCE.to_string());
            first.push(crate::sibling::CLAUDE_SPAWN_GUIDANCE.to_string());
            first.push(crate::open_files::CLAUDE_OPEN_GUIDANCE.to_string());
        }
        if let Some(prompt) = additional_system_prompt {
            first.push(prompt.to_string());
        }
        if !first.is_empty() {
            let head = first.join("\n\n");
            initial_prompt = Some(match initial_prompt {
                Some(task) => format!("{head}\n\n{task}"),
                None => head,
            });
        }
    }
    // The starting prompt rides trailing, like every CLI's positional —
    // or its own flag where the positional is something else (OpenCode's
    // `--prompt`: its positional is the project path).
    if let Some(p) = initial_prompt {
        if let Some(flag) = harness.prompt_flag.as_deref() {
            args.push(flag.to_string());
        }
        args.push(p);
    }
    (program, args, resumed)
}

/// One system-prompt flag carrying nebula's guidance (worktree, spawn,
/// then open) and whatever else the launch adds (the PR scope).
fn push_system_prompt(
    args: &mut Vec<String>,
    flag: &str,
    guidance: bool,
    additional: Option<&str>,
) {
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
        args.extend([flag.to_string(), system_prompt.join("\n\n")]);
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

/// Why a Cloud row's restart and attach are refused: the agent has no
/// local session, and the pane's panel already says where it does run.
const CLOUD_ROW_NO_LOCAL_SESSION: &str =
    "this session runs in Claude Cloud — open it in the browser";

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

/// The Cloud dispatch is a one-shot variation of the normal fresh-Claude
/// command. Keeping it a wrapper leaves every resume/restart caller on
/// the persisted local-session contract, and makes the no-override argument
/// shape directly unit-testable. No worktree guidance either: the Cloud
/// sandbox has no nebula CLI to follow it with. The value binds with `=`
/// (`--cloud=<task>`): the flag takes an *optional* value, so a separate
/// argv item that starts with `--` would be parsed as another Claude flag.
fn claude_cloud_spawn_command(
    harness: &nebula_core::harness::HarnessDescriptor,
    task: &str,
    model: Option<&str>,
    effort: Option<&str>,
    cmd_override: Option<&str>,
) -> (String, Vec<String>, bool) {
    let (program, mut args, resumed) = agent_spawn_command_with(
        harness,
        None,
        None,
        model,
        effort,
        cmd_override,
        None,
        None,
        false,
    );
    if cmd_override.is_none() {
        args.insert(0, format!("--cloud={task}"));
    }
    (program, args, resumed)
}

/// Normalize an agent-supplied title: control characters become spaces,
/// whitespace collapses, and over-long titles are cut — models occasionally
/// hand over a whole sentence no matter what the instruction says.
pub(crate) fn sanitize_title(raw: &str) -> String {
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

/// Does this agent still have a backgrounded tool call running — a job it
/// cut loose from its terminal, which the turn that started it has already
/// left behind? An unknown child pid counts as busy, as for terminals.
fn agent_has_detached_job(session: &PtySession) -> bool {
    match session.child_pid {
        Some(pid) => crate::pty::detached_job_under(pid),
        None => true,
    }
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

/// `pre_exec` hook putting the child in a session of its own (`setsid`):
/// an interactive shell there cannot reach the daemon's controlling
/// terminal, and the child leads a group a timeout can sweep.
pub(crate) fn own_session() -> std::io::Result<()> {
    nix::unistd::setsid()
        .map(drop)
        .map_err(|errno| std::io::Error::from_raw_os_error(errno as i32))
}

/// Why a create was refused when the agent CLI isn't installed. One line —
/// the TUI shows it in the footer flash, which truncates. Unlike git (which
/// the daemon runs with its own inherited PATH), agent CLIs are spawned
/// through the user's login shell, so a fresh install is picked up on the
/// next try with no daemon restart.
fn cli_missing_message(program: &str) -> String {
    format!("{program} was not found on your PATH — install it, then try again.")
}

/// The effective harness registry from the current config: the
/// compiled-in known harnesses with the `harnesses` map applied, then the
/// legacy `custom_harnesses` list, then map-only new ids. Every launch,
/// resume and hook install resolves through here, so a config edit (not a
/// rebuild) is what adds a CLI.
fn harness_registry() -> Vec<nebula_core::harness::HarnessDescriptor> {
    let config = crate::config::Config::load();
    harness_registry_in(&config.harnesses, &config.custom_harnesses)
}

/// [`harness_registry`] against an explicit config, so tests can pin the
/// registry without touching the user's files.
fn harness_registry_in(
    overrides: &std::collections::BTreeMap<String, nebula_core::harness::HarnessOverride>,
    customs: &[nebula_core::harness::CustomHarness],
) -> Vec<nebula_core::harness::HarnessDescriptor> {
    nebula_core::harness::registry(overrides, customs)
}

/// The descriptor a launch or row runs as: built-ins by kind, customs by
/// registry id. A missing id, or a broken entry, refuses the caller with
/// its reason before anything spawns. An `enabled` switch gates the
/// picker, never an existing row — a harness switched off after its
/// sessions were created keeps running them.
fn resolve_harness(
    kind: AgentKind,
    id: Option<&str>,
) -> Result<nebula_core::harness::HarnessDescriptor> {
    resolve_harness_in(kind, id, &harness_registry())
}

/// [`resolve_harness`] against an explicit registry, so tests can pin
/// entries without touching the user's config.
fn resolve_harness_in(
    kind: AgentKind,
    id: Option<&str>,
    all: &[nebula_core::harness::HarnessDescriptor],
) -> Result<nebula_core::harness::HarnessDescriptor> {
    if kind == AgentKind::Custom && id.map(str::trim).filter(|id| !id.is_empty()).is_none() {
        bail!("custom harness launch is missing its registry id");
    }
    nebula_core::harness::resolve(all, kind, id)
        .cloned()
        .map_err(anyhow::Error::msg)
}

/// Wrap `program args…` in a login + interactive shell (`$SHELL -l -i -c
/// 'unset …; export …; prog args'`) so the child gets the user's real
/// environment — ~/.zprofile and ~/.zshrc on zsh — rather than the daemon's.
///
/// The command word goes in bare, so the shell resolves it the way a typed
/// command line would: an alias or function from the rc files wins over the
/// binary on PATH. That is where a work setup reroutes `claude` through a
/// wrapper (another backend, another login), and the earlier `exec env …
/// 'claude'` form skipped it — `env` looks the name up on PATH, and neither
/// zsh nor bash expands the word after an `exec` either — so every session
/// landed on the raw CLI. Without an `exec` the shell stays in charge of the
/// launch: zsh execs a plain last command itself, so the agent is still the
/// PTY's direct child there; bash, and any command that resolves to a
/// function, run it as a job under the shell instead, in a process group of
/// its own — which is why `PtySession::kill` sweeps the whole tree rather
/// than one group.
///
/// The prelude restates what the pane is *after* those files have run:
/// `TERM` and `COLORTERM` name nebula's own grid — 24-bit colour whatever
/// the host terminal — and `NO_COLOR` / `FORCE_COLOR` are dropped. A
/// login-only profile that exports `NO_COLOR` reaches a session here and
/// nowhere else (foot and Ghostty on Linux start non-login shells), and
/// Claude Code takes it as "no colour": its whole UI in the default
/// foreground while the TUI around it stays coloured (#37). The spawn sets
/// the same three against the daemon's inherited environment; this covers
/// the profile's.
fn login_shell_wrap(shell: &str, program: &str, args: &[String]) -> (String, Vec<String>) {
    let mut line = command_word(program);
    for arg in args {
        line.push(' ');
        line.push_str(&nebula_core::shell::single_quote(arg));
    }
    login_shell_line(shell, &line)
}

/// Agent commands run inside a login shell whose startup files can put an
/// installed upstream Nebula before this binary on PATH. Re-prepend the
/// running fork's directory after those files load so its `nebula` commands
/// route to the matching daemon.
fn login_shell_agent_wrap(
    shell: &str,
    program: &str,
    args: &[String],
) -> Result<(String, Vec<String>)> {
    let (shell, mut args) = login_shell_wrap(shell, program, args);
    let line = args.last_mut().expect("login shell command line");
    let executable = std::env::current_exe().context("resolve current Nebula executable")?;
    let binary_dir = executable
        .parent()
        .context("Nebula executable has no parent directory")?;
    let path = nebula_core::shell::single_quote(&binary_dir.display().to_string());
    line.insert_str(0, &format!("PATH={path}:\"$PATH\"; export PATH; "));
    Ok((shell, args))
}

/// [`login_shell_wrap`] for a line that is already shell syntax — a RUN
/// TERMINAL's `.nebula.json` `run`, pipes and `&&` and all — behind the
/// same prelude.
fn login_shell_line(shell: &str, line: &str) -> (String, Vec<String>) {
    let mut cmdline = String::from("unset");
    for name in env::PANE_COLOR_OVERRIDES {
        cmdline.push(' ');
        cmdline.push_str(name);
    }
    cmdline.push_str("; export TERM=");
    cmdline.push_str(env::PANE_TERM);
    cmdline.push_str(" COLORTERM=");
    cmdline.push_str(env::PANE_COLORTERM);
    cmdline.push_str("; ");
    cmdline.push_str(line);
    let args = LOGIN_SHELL_ARGS
        .iter()
        .map(|s| s.to_string())
        .chain([cmdline])
        .collect();
    (shell.to_string(), args)
}

/// `program` as the command word of a shell line: bare when it is a plain
/// name (letters, digits, `-`, `_`, `.`, `/`), since a quoted word is exempt
/// from alias expansion in every shell; single-quoted otherwise.
fn command_word(program: &str) -> String {
    let plain = !program.is_empty()
        && program
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_./".contains(&b));
    if plain {
        program.to_string()
    } else {
        nebula_core::shell::single_quote(program)
    }
}

/// The login-shell line [`Daemon::probe_cli`] runs to ask whether
/// `program` resolves. The word is single-quoted
/// ([`nebula_core::shell::single_quote`]):
/// a `harnesses` entry in config.json can name any string, and pasted in
/// bare a quote would close the word and run the rest as a command — at
/// daemon boot, since [`Daemon::warm_cli_probes`] asks for every entry.
/// Built-in names come out exactly as they always did (`'claude'`).
fn cli_probe_line(program: &str) -> String {
    format!(
        "command -v {} >/dev/null 2>&1",
        nebula_core::shell::single_quote(program)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Claude argv: `args`, then nebula's appended guidance (worktree and
    /// spawn, one `--append-system-prompt`).
    fn guided(flag: &str, args: &[&str]) -> Vec<String> {
        args.iter()
            .map(|s| s.to_string())
            .chain([
                flag.to_string(),
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
    fn claude_transcript_lookup_searches_every_project_slug() {
        let tmp = tempfile::tempdir().unwrap();
        let projects = tmp.path().join("projects");
        for slug in ["-w-issue-68", "-w-root-branch-switcher"] {
            std::fs::create_dir_all(projects.join(slug)).unwrap();
        }
        std::fs::write(projects.join("-w-issue-68").join("sid-1.jsonl"), "{}\n").unwrap();
        let missing = tmp.path().join("no-such-config").join("projects");
        let roots = [missing.clone(), projects];
        // Found under the slug the session began in, whichever checkout
        // resumes it (a relocated session).
        assert_eq!(claude_transcript_exists(&roots, "sid-1"), Some(true));
        // A CLI nobody prompted: an id, and no file behind it.
        assert_eq!(claude_transcript_exists(&roots, "sid-2"), Some(false));
        // An id that tries to be a path names nothing.
        assert_eq!(
            claude_transcript_exists(&roots, "../-w-issue-68/sid-1"),
            Some(false)
        );
        // Nothing readable: no verdict, so the resume is still tried.
        assert_eq!(claude_transcript_exists(&[missing], "sid-1"), None);
    }

    #[test]
    fn spawn_command_per_kind_resume_shapes() {
        // Fresh sessions: bare CLI (Claude plus its system-prompt guidance).
        assert_eq!(
            agent_spawn_command(AgentKind::Claude, None, None, None, None),
            (
                "claude".into(),
                guided("--append-system-prompt", &[]),
                false
            )
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
            ("pi".into(), guided("--append-system-prompt", &[]), false)
        );
        // Pi resumes by exact id — one that is missing is created, so a
        // relocated session's new cwd never dies on a stale id.
        assert_eq!(
            agent_spawn_command(AgentKind::Pi, Some("sid-4"), None, None, None),
            (
                "pi".into(),
                guided("--append-system-prompt", &["--session-id", "sid-4"]),
                true
            )
        );
        // Muse boots bare and fresh: no resume flag is mapped yet, so a
        // stored session id is ignored rather than sent.
        assert_eq!(
            agent_spawn_command(AgentKind::Muse, None, None, None, None),
            ("muse".into(), vec![], false)
        );
        assert_eq!(
            agent_spawn_command(AgentKind::Muse, Some("sid-9"), None, None, None),
            ("muse".into(), vec![], false)
        );
        // OpenCode boots bare like Codex (no system-prompt flag) and with
        // no permission flag (its prompts drive NEEDS FEEDBACK through the
        // managed plugin); a stored id resumes by `--session`.
        assert_eq!(
            agent_spawn_command(AgentKind::OpenCode, None, None, None, None),
            ("opencode".into(), vec![], false)
        );
        assert_eq!(
            agent_spawn_command(AgentKind::OpenCode, Some("ses_9"), None, None, None),
            (
                "opencode".into(),
                vec!["--session".to_string(), "ses_9".to_string()],
                true
            )
        );
        // Claude resumes with a flag; codex with a subcommand (order matters).
        assert_eq!(
            agent_spawn_command(AgentKind::Claude, Some("sid-1"), None, None, None),
            (
                "claude".into(),
                guided("--append-system-prompt", &["--resume", "sid-1"]),
                true
            )
        );
        // Skip-permissions flags trail the resume args. A codex resume is
        // told its checkout (`--cd`): without it codex reopens the session
        // in the directory its transcript recorded, which for a relocated
        // session is the old one.
        assert_eq!(
            agent_spawn_command(AgentKind::Codex, Some("sid-2"), None, None, None),
            (
                "codex".into(),
                vec![
                    "resume".to_string(),
                    "sid-2".to_string(),
                    "--cd".to_string(),
                    TEST_CWD.to_string(),
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
                guided(
                    "--append-system-prompt",
                    &["--model", "opus", "--effort", "high"]
                ),
                false
            )
        );
        assert_eq!(
            agent_spawn_command(AgentKind::Claude, None, None, Some("max"), None),
            (
                "claude".into(),
                guided("--append-system-prompt", &["--effort", "max"]),
                false
            )
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
                guided(
                    "--append-system-prompt",
                    &["--model", "sonnet", "--thinking", "high"]
                ),
                false
            )
        );
        assert_eq!(
            agent_spawn_command(AgentKind::Pi, None, None, Some("off"), None),
            (
                "pi".into(),
                guided("--append-system-prompt", &["--thinking", "off"]),
                false
            )
        );
        // Muse takes `--model` verbatim and no effort flag yet: effort is
        // dropped, never sent.
        assert_eq!(
            agent_spawn_command(AgentKind::Muse, None, Some("spark"), Some("high"), None),
            (
                "muse".into(),
                vec!["--model".to_string(), "spark".to_string()],
                false
            )
        );
        // OpenCode takes a `provider/model` id verbatim and has no effort
        // flag: effort is dropped, never sent.
        assert_eq!(
            agent_spawn_command(
                AgentKind::OpenCode,
                None,
                Some("anthropic/claude-sonnet-5"),
                Some("high"),
                None
            ),
            (
                "opencode".into(),
                vec![
                    "--model".to_string(),
                    "anthropic/claude-sonnet-5".to_string()
                ],
                false
            )
        );
        // Resume keeps the model/effort flags (a fallback fresh spawn needs
        // them, and the CLIs accept them alongside resume).
        assert_eq!(
            agent_spawn_command(AgentKind::Claude, Some("sid"), Some("sonnet"), None, None),
            (
                "claude".into(),
                guided(
                    "--append-system-prompt",
                    &["--resume", "sid", "--model", "sonnet"]
                ),
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
    fn custom_spawn_uses_entry_program_and_model_flag() {
        // Custom entries launch with their own program and model flag; a
        // stored session id is ignored (fresh boot) and effort is dropped.
        let agy = nebula_core::harness::CustomHarness {
            id: "agy".into(),
            label: "Agy".into(),
            program: "agy".into(),
            enabled: true,
            model: "default".into(),
            model_flag: "--model".into(),
            hooks: None,
        };
        let all = harness_registry_in(
            &std::collections::BTreeMap::new(),
            std::slice::from_ref(&agy),
        );
        let harness = test_custom_harness(&all, "agy");
        let (program, args, resumed) = agent_spawn_command_with(
            &harness,
            Some("sid-1"),
            Some(Path::new(TEST_CWD)),
            Some("big-1"),
            Some("high"),
            None,
            Some("do it"),
            None,
            true,
        );
        assert_eq!(program, "agy");
        assert_eq!(args, vec!["--model", "big-1", "do it"]);
        assert!(!resumed);
        // A custom model flag spelling is honored verbatim.
        let gemini = nebula_core::harness::CustomHarness {
            model_flag: "-m".into(),
            ..agy.clone()
        };
        let all = harness_registry_in(&std::collections::BTreeMap::new(), &[gemini]);
        let harness = test_custom_harness(&all, "agy");
        let (_, args, _) = agent_spawn_command_with(
            &harness,
            None,
            Some(Path::new(TEST_CWD)),
            Some("flash"),
            None,
            None,
            None,
            None,
            true,
        );
        assert_eq!(args, vec!["-m", "flash"]);
    }

    #[test]
    fn custom_spawn_honors_map_deltas_for_resume_and_hooks() {
        // A legacy entry gains a resume flag and an effort flag purely
        // through the `harnesses` map — no code change, no new shape.
        use nebula_core::harness::{Clearable, HarnessOverride};
        let agy = nebula_core::harness::CustomHarness {
            id: "agy".into(),
            label: "Agy".into(),
            program: "agy".into(),
            enabled: true,
            model: "default".into(),
            model_flag: "--model".into(),
            hooks: None,
        };
        let mut overrides = std::collections::BTreeMap::new();
        overrides.insert(
            "agy".into(),
            HarnessOverride {
                resume_flag: Clearable::Set("--resume".into()),
                effort_flag: Clearable::Set("--effort".into()),
                effort_offered: Some(true),
                hooks: Clearable::Set("claude".into()),
                ..HarnessOverride::default()
            },
        );
        let all = harness_registry_in(&overrides, &[agy]);
        let harness = test_custom_harness(&all, "agy");
        assert_eq!(
            harness.hook_dialect(),
            Some(AgentKind::Claude),
            "the dialect installs Claude hooks for a third-party CLI"
        );
        let (program, args, resumed) = agent_spawn_command_with(
            &harness,
            Some("sid-1"),
            Some(Path::new(TEST_CWD)),
            Some("big-1"),
            Some("high"),
            None,
            Some("do it"),
            None,
            true,
        );
        assert_eq!(program, "agy");
        assert_eq!(
            args,
            vec!["--resume", "sid-1", "--model", "big-1", "--effort", "high", "do it"]
        );
        assert!(resumed);
    }

    /// A third-party CLI with every row mapped — shaped like xAI's
    /// `grok` (`--model`, `--reasoning-effort`, `--resume <id>`,
    /// `--rules` appending to the system prompt, trailing prompt) —
    /// spawns, resumes and carries guidance with config alone.
    #[test]
    fn third_party_harness_with_all_rows_mapped_spawns_and_resumes() {
        use nebula_core::harness::{Clearable, HarnessOverride};
        let mut overrides = std::collections::BTreeMap::new();
        overrides.insert(
            "grok".into(),
            HarnessOverride {
                program: Clearable::Set("grok".into()),
                model_flag: Clearable::Set("--model".into()),
                effort_flag: Clearable::Set("--reasoning-effort".into()),
                effort_offered: Some(true),
                resume_flag: Clearable::Set("--resume".into()),
                system_append_flag: Clearable::Set("--rules".into()),
                ..HarnessOverride::default()
            },
        );
        let all = harness_registry_in(&overrides, &[]);
        let grok = test_custom_harness(&all, "grok");
        assert_eq!(grok.problem(), None);

        // Fresh boot: model, effort, guidance and prompt in order, no
        // permissions flag the entry never named.
        let (program, args, resumed) = agent_spawn_command_with(
            &grok,
            None,
            Some(Path::new(TEST_CWD)),
            Some("grok-code"),
            Some("high"),
            None,
            Some("fix auth"),
            None,
            true,
        );
        assert_eq!(program, "grok");
        assert!(!resumed);
        let mut expected = vec![
            "--model".to_string(),
            "grok-code".to_string(),
            "--reasoning-effort".to_string(),
            "high".to_string(),
        ];
        expected.append(&mut guided("--rules", &[]));
        expected.push("fix auth".into());
        assert_eq!(args, expected);

        // Resume: the stored id rides the mapped flag.
        let (_, args, resumed) = agent_spawn_command_with(
            &grok,
            Some("sid-9"),
            Some(Path::new(TEST_CWD)),
            None,
            None,
            None,
            None,
            None,
            false,
        );
        assert!(resumed);
        assert_eq!(args, vec!["--resume", "sid-9"]);
    }

    #[test]
    fn harness_resolve_names_missing_unknown_and_broken_entries() {
        use nebula_core::harness::CustomHarness;
        let agy = CustomHarness {
            id: "agy".into(),
            label: String::new(),
            program: "agy".into(),
            enabled: true,
            model: "default".into(),
            model_flag: "--model".into(),
            hooks: None,
        };
        let all = harness_registry_in(
            &std::collections::BTreeMap::new(),
            std::slice::from_ref(&agy),
        );
        // Built-ins resolve by kind, whatever the id says.
        assert_eq!(
            resolve_harness_in(AgentKind::Claude, None, &all)
                .unwrap()
                .program,
            "claude"
        );
        // Custom without an id, or with an unknown one, refuses.
        let err = resolve_harness_in(AgentKind::Custom, None, &all).unwrap_err();
        assert!(err.to_string().contains("registry id"), "{err}");
        let err = resolve_harness_in(AgentKind::Custom, Some("gone"), &all).unwrap_err();
        assert!(err.to_string().contains("no longer defined"), "{err}");
        // A broken entry refuses with its reason, never launches.
        let broken = CustomHarness {
            program: String::new(),
            ..agy.clone()
        };
        let all = harness_registry_in(&std::collections::BTreeMap::new(), &[broken]);
        let err = resolve_harness_in(AgentKind::Custom, Some("agy"), &all).unwrap_err();
        assert!(err.to_string().contains("no program"), "{err}");
        // The usable entry resolves, disabled or not: a harness switched
        // off after its sessions were created keeps running them.
        let off = CustomHarness {
            enabled: false,
            ..agy.clone()
        };
        let all = harness_registry_in(&std::collections::BTreeMap::new(), &[off]);
        assert_eq!(
            resolve_harness_in(AgentKind::Custom, Some("agy"), &all)
                .unwrap()
                .program,
            "agy"
        );
    }

    #[test]
    fn grok_spawn_uses_verified_flags_and_preserves_prompt_boundaries() {
        let grok = nebula_core::harness::builtin("grok").unwrap();
        assert_eq!(
            agent_spawn_command_with(&grok, None, None, None, None, None, None, None, false),
            ("grok".into(), vec![], false)
        );
        let (program, args, resumed) = agent_spawn_command_with(
            &grok,
            Some("session-id"),
            Some(Path::new(TEST_CWD)),
            Some("model-id"),
            Some("high"),
            None,
            Some("fix the bug; keep this as one argument"),
            Some("Review only PR #42"),
            false,
        );
        assert_eq!(program, "grok");
        assert!(resumed);
        assert_eq!(
            args,
            vec![
                "--resume",
                "session-id",
                "--model",
                "model-id",
                "--reasoning-effort",
                "high",
                "--rules",
                "Review only PR #42",
                "fix the bug; keep this as one argument",
            ]
        );
        assert_eq!(grok.hook_dialect(), None);
        assert_eq!(grok.permissions_flag, None);
    }

    #[test]
    fn spawn_command_initial_prompt_is_the_trailing_positional_argument() {
        let all = test_registry();
        let claude = test_harness(&all, AgentKind::Claude);
        let codex = test_harness(&all, AgentKind::Codex);
        let cursor = test_harness(&all, AgentKind::Cursor);
        let pi = test_harness(&all, AgentKind::Pi);
        // The relocation notice trails everything, guidance included.
        let (_, args, resumed) = agent_spawn_command_with(
            &claude,
            Some("sid"),
            Some(Path::new(TEST_CWD)),
            Some("opus"),
            None,
            None,
            Some("carry on"),
            None,
            true,
        );
        assert!(resumed);
        let mut expected = guided(
            "--append-system-prompt",
            &["--resume", "sid", "--model", "opus"],
        );
        expected.push("carry on".into());
        assert_eq!(args, expected);
        // Codex and cursor take it as their trailing positional too.
        assert_eq!(
            agent_spawn_command_with(
                &codex,
                Some("sid"),
                Some(Path::new(TEST_CWD)),
                None,
                None,
                None,
                Some("carry on"),
                None,
                true,
            )
            .1,
            vec!["resume", "sid", "--cd", TEST_CWD, "--yolo", "carry on"]
        );
        assert_eq!(
            agent_spawn_command_with(
                &cursor,
                Some("sid"),
                Some(Path::new(TEST_CWD)),
                None,
                None,
                None,
                Some("carry on"),
                None,
                true,
            )
            .1,
            vec!["--resume", "sid", "--force", "carry on"]
        );
        // A fresh spawn with a starting prompt (an AGENT PRESET launch):
        // model, effort and system prompt all precede it.
        let mut expected = guided(
            "--append-system-prompt",
            &["--model", "opus", "--effort", "high"],
        );
        expected.push("fix auth".into());
        assert_eq!(
            agent_spawn_command_with(
                &claude,
                None,
                Some(Path::new(TEST_CWD)),
                Some("opus"),
                Some("high"),
                None,
                Some("fix auth"),
                None,
                true,
            )
            .1,
            expected
        );
        assert_eq!(
            agent_spawn_command_with(
                &codex,
                None,
                Some(Path::new(TEST_CWD)),
                Some("gpt-5.5"),
                Some("high"),
                None,
                Some("fix auth"),
                None,
                true,
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
                &cursor,
                None,
                Some(Path::new(TEST_CWD)),
                None,
                None,
                None,
                Some("fix auth"),
                None,
                true,
            )
            .1,
            vec!["--force", "fix auth"]
        );
        // Pi: the prompt trails the resume id and the guidance, as pi's
        // `[messages...]` positional.
        let mut expected = guided("--append-system-prompt", &["--session-id", "sid"]);
        expected.push("carry on".into());
        assert_eq!(
            agent_spawn_command_with(
                &pi,
                Some("sid"),
                Some(Path::new(TEST_CWD)),
                None,
                None,
                None,
                Some("carry on"),
                None,
                true,
            )
            .1,
            expected
        );
        // An override is verbatim: no guidance, no prompt.
        assert_eq!(
            agent_spawn_command_with(
                &claude,
                None,
                Some(Path::new(TEST_CWD)),
                None,
                None,
                Some("/bin/sh -i"),
                Some("carry on"),
                None,
                true,
            ),
            ("/bin/sh".into(), vec!["-i".to_string()], false)
        );
    }

    /// OpenCode's positional is the project path, so a starting prompt —
    /// an AGENT PRESET's task, a PR SESSION's rule — rides `--prompt`
    /// after the model flag, where every other CLI takes a trailing
    /// positional; the field changes nothing for them.
    #[test]
    fn opencode_first_prompt_rides_its_flag() {
        let all = test_registry();
        let opencode = test_harness(&all, AgentKind::OpenCode);
        let (program, args, resumed) = agent_spawn_command_with(
            &opencode,
            None,
            Some(Path::new(TEST_CWD)),
            Some("opencode/big-pickle"),
            None,
            None,
            Some("Fix auth"),
            None,
            true,
        );
        assert_eq!(program, "opencode");
        assert_eq!(
            args,
            ["--model", "opencode/big-pickle", "--prompt", "Fix auth"]
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
        );
        assert!(!resumed);
        let claude = test_harness(&all, AgentKind::Claude);
        let (_, args, _) = agent_spawn_command_with(
            &claude,
            None,
            Some(Path::new(TEST_CWD)),
            None,
            None,
            None,
            Some("Fix auth"),
            None,
            false,
        );
        assert_eq!(args, vec!["Fix auth".to_string()]);
    }

    /// The relocation notice reaches the CLIs whose resume submits a
    /// trailing prompt — Claude, codex and pi — and names the checkout;
    /// cursor's is unverified and OpenCode's `--session <id> --prompt` is
    /// not submitted (verified on opencode 1.18.32), so their relocated
    /// sessions reopen silent. (Codex was gated out until #39: `codex
    /// resume <id> --yolo` sat at Ready in the worktree until the user
    /// typed "continue".)
    #[test]
    fn relocation_prompt_reaches_every_kind_but_cursor_and_opencode() {
        let feat = Worktree {
            id: WorktreeId("feat".into()),
            project_id: ProjectId("p".into()),
            path: "/nebula-test/p-feat".into(),
            branch: "feat".into(),
            is_main: false,
            sort_order: 0,
        };
        let all = test_registry();
        for kind in [AgentKind::Claude, AgentKind::Codex, AgentKind::Pi] {
            let harness = test_harness(&all, kind);
            assert!(harness.relocation_prompt, "{kind:?} maps the notice");
            let prompt = relocation_prompt(harness.relocation_prompt, &feat)
                .unwrap_or_else(|| panic!("{kind:?}"));
            assert!(prompt.contains("`feat`"), "{kind:?}: {prompt}");
            assert!(prompt.contains("/nebula-test/p-feat"), "{kind:?}: {prompt}");
            assert!(prompt.contains("Continue the user's most recent request"));
        }
        let cursor = test_harness(&all, AgentKind::Cursor);
        assert_eq!(relocation_prompt(cursor.relocation_prompt, &feat), None);
        let opencode = test_harness(&all, AgentKind::OpenCode);
        assert_eq!(relocation_prompt(opencode.relocation_prompt, &feat), None);

        // And the codex respawn it feeds: resumed, re-rooted in the
        // worktree, and opening on the notice.
        let codex = test_harness(&all, AgentKind::Codex);
        let notice = relocation_prompt(codex.relocation_prompt, &feat).unwrap();
        let (program, args, resumed) = agent_spawn_command_with(
            &codex,
            Some("sid"),
            Some(&feat.path),
            None,
            None,
            None,
            Some(&notice),
            None,
            true,
        );
        assert_eq!(program, "codex");
        assert!(resumed);
        assert_eq!(
            args,
            vec![
                "resume",
                "sid",
                "--cd",
                "/nebula-test/p-feat",
                "--yolo",
                notice.as_str()
            ]
        );
    }

    #[test]
    fn spawn_command_keeps_pr_scope_and_url_in_claudes_system_prompt() {
        let pr_url = "https://github.com/AgentSystemLabs/nebula/pull/42";
        let pr_prompt = crate::pr_scope::rule(&crate::pr_scope::PrScope {
            url: pr_url,
            worktree: Path::new("/w/nebula-worktrees/fix"),
            branch: "fix",
            root: Some(Path::new("/w/nebula")),
        });
        let all = test_registry();
        let claude = test_harness(&all, AgentKind::Claude);
        let (_, args, resumed) = agent_spawn_command_with(
            &claude,
            Some("sid"),
            Some(Path::new(TEST_CWD)),
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
        assert!(prompts[0].contains("/w/nebula-worktrees/fix"));
    }

    #[test]
    fn spawn_command_claude_cloud_passes_the_task_as_one_argument() {
        let all = test_registry();
        let claude = test_harness(&all, AgentKind::Claude);
        assert_eq!(
            claude_cloud_spawn_command(
                &claude,
                "Fix auth\nRun tests; don't stop",
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
            claude_cloud_spawn_command(&claude, "--dangerously-skip-permissions", None, None, None)
                .1,
            vec!["--cloud=--dangerously-skip-permissions"]
        );
        // Overrides (tests) stay verbatim — no cloud flag at all.
        assert_eq!(
            claude_cloud_spawn_command(&claude, "task", None, None, Some("/bin/true")).1,
            Vec::<String>::new()
        );
    }

    #[tokio::test]
    async fn send_cloud_message_requires_a_cloud_session() {
        let daemon = test_daemon();
        seed_projects(&daemon, &["p"]);
        seed_worktree(&daemon, "p", "w", "/tmp", true);
        seed_agent(&daemon, "local", "w", None);
        let err = daemon
            .send_cloud_message(&AgentId("local".into()), "hi")
            .await
            .unwrap_err();
        assert!(
            err.to_string().contains("not launched in Claude Cloud"),
            "{err}"
        );
    }

    /// A Cloud row never gets a local CLI booted in its name: the agent
    /// runs in the cloud sandbox, and the row's pane links there. Both
    /// paths that would otherwise fork a bare `claude` — a restart, and an
    /// attach finding no PTY — refuse instead.
    #[tokio::test]
    async fn cloud_row_is_never_respawned_locally() {
        let daemon = test_daemon();
        seed_projects(&daemon, &["p"]);
        seed_worktree(&daemon, "p", "w", "/tmp", true);
        seed_agent(&daemon, "cloud", "w", None);
        let id = AgentId("cloud".into());
        daemon
            .store
            .set_agent_cloud_session_id(&id, Some("session_016SiQW5Lem2LbnUf1A3undt"))
            .unwrap();

        let err = daemon.restart_agent(&id).await.unwrap_err();
        assert!(err.to_string().contains("runs in Claude Cloud"), "{err}");
        let err = daemon
            .ensure_session(&SessionRef::Agent(id.clone()), 80, 24)
            .err()
            .expect("a cloud row's attach is refused");
        assert!(err.to_string().contains("runs in Claude Cloud"), "{err}");
        assert!(!daemon.is_alive(&SessionRef::Agent(id)));
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

    /// What every agent launch runs once the login shell's files have run,
    /// ahead of the command itself.
    const PANE_ENV: &str =
        "unset NO_COLOR FORCE_COLOR; export TERM=xterm-256color COLORTERM=truecolor;";

    #[test]
    fn cli_probe_line_looks_the_program_up_verbatim() {
        // Built-ins read exactly as before the registry.
        assert_eq!(
            cli_probe_line("claude"),
            "command -v 'claude' >/dev/null 2>&1"
        );
        assert_eq!(
            cli_probe_line("cursor-agent"),
            "command -v 'cursor-agent' >/dev/null 2>&1"
        );
        // A config-named program is any string. A quote in it stays
        // inside the word instead of closing it and opening a command.
        let hostile = "x'; echo INJECTED; echo '";
        let line = cli_probe_line(hostile);
        assert_eq!(
            line,
            "command -v 'x'\\''; echo INJECTED; echo '\\''' >/dev/null 2>&1"
        );
        // And a real shell agrees: the lookup fails quietly, nothing runs.
        let out = std::process::Command::new("/bin/sh")
            .args(["-c", &line])
            .output()
            .expect("/bin/sh");
        assert!(!out.status.success(), "no such program");
        assert!(
            out.stdout.is_empty(),
            "{:?}",
            String::from_utf8_lossy(&out.stdout)
        );
    }

    #[test]
    fn login_shell_wrap_quotes_args_and_leaves_the_command_word_bare() {
        let (program, args) = login_shell_wrap(
            "/bin/zsh",
            "claude",
            &["--resume".to_string(), "sid-1".to_string()],
        );
        assert_eq!(program, "/bin/zsh");
        assert_eq!(
            args,
            vec![
                "-l",
                "-i",
                "-c",
                &format!("{PANE_ENV} claude '--resume' 'sid-1'")
            ]
        );
        // Single quotes in an arg survive the wrapping.
        let (_, args) = login_shell_wrap("/bin/zsh", "echo", &["it's".to_string()]);
        assert_eq!(args[3], format!(r"{PANE_ENV} echo 'it'\''s'"));
        // A command word that isn't a plain name is quoted like an argument.
        let (_, args) = login_shell_wrap("/bin/zsh", "my tool", &[]);
        assert_eq!(args[3], format!("{PANE_ENV} 'my tool'"));
    }

    #[test]
    fn login_shell_agent_wrap_puts_fork_binary_first_on_path() {
        let executable = std::env::current_exe().unwrap();
        let bin_dir = executable.parent().unwrap();
        let (shell, args) = login_shell_agent_wrap("/bin/sh", "claude", &[]).unwrap();
        assert_eq!(shell, "/bin/sh");
        let line = &args[3];
        assert!(line.starts_with(&format!(
            "PATH='{}':\"$PATH\"; export PATH; ",
            bin_dir.display()
        )));
        assert!(line.ends_with(&format!("{PANE_ENV} claude")));
    }

    /// The command word resolves through the shell, so an alias or function
    /// from the rc files takes precedence over the binary on PATH — the
    /// reason a launch goes through the login shell at all. A function
    /// stands in for the alias: every POSIX sh honours one in a `-c` string,
    /// and the old `exec env` form bypassed both alike.
    #[test]
    fn login_shell_wrap_lets_the_shell_resolve_the_command() {
        let (_, args) = login_shell_wrap(
            "/bin/sh",
            "claude",
            &["--resume".to_string(), "sid-1".to_string()],
        );
        let out = std::process::Command::new("/bin/sh")
            .arg("-c")
            .arg(format!(
                "claude() {{ printf 'routed %s' \"$*\"; }}; {}",
                args[3]
            ))
            .output()
            .unwrap();
        assert_eq!(
            String::from_utf8_lossy(&out.stdout),
            "routed --resume sid-1",
            "stderr: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    #[test]
    fn fork_cli_directory_precedes_upstream_after_login_shell_profile() {
        let home = tempfile::tempdir().unwrap();
        let bin = home.path().join("bin");
        let upstream = home.path().join("upstream");
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::create_dir_all(&upstream).unwrap();
        let marker = home.path().join("upstream-used");
        let fork_cli = bin.join("nebula");
        let upstream_cli = upstream.join("nebula");
        std::fs::write(&fork_cli, "#!/bin/sh\nprintf fork\n").unwrap();
        std::fs::write(
            &upstream_cli,
            format!(
                "#!/bin/sh\nprintf upstream > '{}'\nprintf upstream\n",
                marker.display()
            ),
        )
        .unwrap();
        use std::os::unix::fs::PermissionsExt;
        for path in [&fork_cli, &upstream_cli] {
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }

        let profile_path = format!("{}:/usr/bin:/bin", upstream.display());
        let fork_path = nebula_core::shell::single_quote(&bin.display().to_string());
        let line = format!("PATH={fork_path}:\"$PATH\"; export PATH; nebula rename");
        let output = std::process::Command::new("/bin/sh")
            .args(["-c", &line])
            .env("PATH", profile_path)
            .output()
            .unwrap();

        assert!(output.status.success());
        assert_eq!(String::from_utf8_lossy(&output.stdout), "fork");
        assert!(!marker.exists(), "upstream binary was not selected");
    }

    /// The shape of the report itself: zsh, `-l -i -c`, and an alias in
    /// `.zshrc` that reroutes `claude`. Skipped where there is no zsh.
    #[test]
    fn login_shell_wrap_honours_a_zshrc_alias() {
        use std::os::unix::process::CommandExt;
        if std::process::Command::new("zsh")
            .arg("--version")
            .stdout(std::process::Stdio::null())
            .status()
            .is_err()
        {
            return;
        }
        let home = tempfile::tempdir().unwrap();
        std::fs::write(home.path().join(".zshrc"), "alias claude='echo routed'\n").unwrap();
        let (program, args) = login_shell_wrap(
            "zsh",
            "claude",
            &["--resume".to_string(), "sid-1".to_string()],
        );
        let mut cmd = std::process::Command::new(program);
        cmd.args(&args)
            .env("ZDOTDIR", home.path())
            .stdin(std::process::Stdio::null());
        // Own session, as the daemon's CLI probe does: an interactive zsh
        // must not make itself the foreground of the terminal running the
        // tests.
        unsafe {
            cmd.pre_exec(own_session);
        }
        let out = cmd.output().unwrap();
        assert_eq!(
            String::from_utf8_lossy(&out.stdout).trim_end(),
            "routed --resume sid-1",
            "stderr: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    /// The command line's `env` really does undo a profile's colour
    /// overrides and restate the pane: run it through a plain `sh -c` with
    /// `NO_COLOR` and a foreign `TERM` already exported, as a `.profile`
    /// would leave them, and read back what the program sees.
    #[test]
    fn login_shell_wrap_restates_the_pane_after_the_profile() {
        let (_, args) = login_shell_wrap(
            "/bin/sh",
            "sh",
            &[
                "-c".to_string(),
                r#"printf '%s|%s|%s|%s' "$TERM" "$COLORTERM" "${NO_COLOR-unset}" "${FORCE_COLOR-unset}""#
                    .to_string(),
            ],
        );
        let out = std::process::Command::new("/bin/sh")
            .arg("-c")
            .arg(&args[3])
            .env("NO_COLOR", "1")
            .env("FORCE_COLOR", "0")
            .env("TERM", "foot")
            .output()
            .unwrap();
        assert_eq!(
            String::from_utf8_lossy(&out.stdout),
            "xterm-256color|truecolor|unset|unset",
            "stderr: {}",
            String::from_utf8_lossy(&out.stderr)
        );
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

    #[test]
    fn claude_projects_dirs_follow_the_transcripts_hooks_reported() {
        // An agent's shell may point CLAUDE_CONFIG_DIR somewhere the daemon's
        // own env never heard of; its hooks name the real transcript.
        let daemon = test_daemon();
        let reported = crate::session_title::TranscriptRef::from_payload(
            Some("/cfg/alt/projects/-w-feat/sid-9.jsonl"),
            Some("sid-9"),
        )
        .unwrap();
        daemon
            .transcripts
            .lock()
            .unwrap()
            .insert(AgentId("a1".into()), reported);
        let dirs = daemon.claude_projects_dirs();
        assert!(
            dirs.contains(&PathBuf::from("/cfg/alt/projects")),
            "{dirs:?}"
        );
    }

    /// A project whose main checkout is a fresh directory, registered with
    /// `daemon`, for the RUN TERMINAL tests to write `.nebula.json` into.
    fn run_worktree(daemon: &Daemon) -> (tempfile::TempDir, Worktree) {
        let dir = tempfile::tempdir().unwrap();
        let project = Project {
            id: ProjectId::generate(),
            name: "demo".into(),
            repo_path: dir.path().to_path_buf(),
            sort_order: 0,
        };
        daemon.store.insert_project(&project).unwrap();
        let worktree = Worktree {
            id: WorktreeId::generate(),
            project_id: project.id,
            path: dir.path().to_path_buf(),
            branch: "main".into(),
            is_main: true,
            sort_order: 0,
        };
        daemon.store.insert_worktree(&worktree).unwrap();
        (dir, worktree)
    }

    #[tokio::test]
    async fn run_needs_a_project_file_starts_once_and_stops() {
        let daemon = test_daemon();
        let (dir, worktree) = run_worktree(&daemon);

        let missing = daemon.start_run(&worktree.id).unwrap_err();
        assert!(
            missing.to_string().contains("Settings (s) → Project")
                && missing.to_string().contains(".nebula.json"),
            "names both places: {missing}"
        );
        assert!(daemon
            .store
            .run_terminals_in(&worktree.id)
            .unwrap()
            .is_empty());

        std::fs::write(dir.path().join(".nebula.json"), r#"{"run": "sleep 30"}"#).unwrap();
        let EntityId::Terminal(id) = daemon.start_run(&worktree.id).unwrap() else {
            panic!("a run lives in a terminal");
        };
        let sref = SessionRef::Terminal(id.clone());
        assert!(daemon.is_alive(&sref));
        let row = daemon.store.get_terminal(&id).unwrap().unwrap();
        assert_eq!(row.run_command.as_deref(), Some("sleep 30"));

        // A second press while it runs starts nothing new.
        assert_eq!(
            daemon.start_run(&worktree.id).unwrap(),
            EntityId::Terminal(id.clone())
        );
        assert_eq!(
            daemon.store.run_terminals_in(&worktree.id).unwrap().len(),
            1
        );

        daemon.stop_run(&worktree.id).unwrap();
        assert!(!daemon.is_alive(&sref));
        assert!(daemon
            .store
            .run_terminals_in(&worktree.id)
            .unwrap()
            .is_empty());
        // Nothing left to stop is not an error.
        daemon.stop_run(&worktree.id).unwrap();
    }

    #[tokio::test]
    async fn a_workspace_run_uses_the_selected_repository_file_and_has_its_own_terminal() {
        let daemon = test_daemon();
        let root = tempfile::tempdir().unwrap();
        let project = Project {
            id: ProjectId::generate(),
            name: "Pilot".into(),
            repo_path: root.path().into(),
            sort_order: 0,
        };
        daemon.store.insert_project(&project).unwrap();
        let workspace_id = WorktreeId::generate();
        let workspace_path = root.path().join("worktrees/feature");
        std::fs::create_dir_all(&workspace_path).unwrap();
        let workspace = Worktree {
            id: workspace_id.clone(),
            project_id: project.id.clone(),
            path: workspace_path.clone(),
            branch: "pilot/feature".into(),
            is_main: false,
            sort_order: 0,
        };
        let api_id = WorktreeId::generate();
        let api_path = workspace_path.join("api");
        std::fs::create_dir_all(&api_path).unwrap();
        std::fs::write(api_path.join(".nebula.json"), r#"{"run":"sleep 34"}"#).unwrap();
        let repository = nebula_core::WorkspaceRepository {
            id: api_id.clone(),
            name: "api".into(),
            path: api_path.clone(),
            base: Some("main".into()),
            fetch_succeeded: true,
            warning: None,
            branch: Some("pilot/feature".into()),
            changed_files: Some(0),
        };
        let session = nebula_core::SessionWorkspace {
            worktree_id: workspace_id.clone(),
            name: "feature".into(),
            state: nebula_core::WorkspaceState::Ready,
            failure: None,
            repositories: vec![repository],
        };
        daemon
            .store
            .insert_session_workspace(&workspace, &session)
            .unwrap();

        let EntityId::Terminal(run) = daemon
            .start_run_for_repository(&workspace_id, Some("api"))
            .unwrap()
        else {
            panic!("repository run creates a terminal");
        };
        let row = daemon.store.get_terminal(&run).unwrap().unwrap();
        assert_eq!(row.worktree_id, api_id);
        assert_eq!(row.name, "run:api");
        assert_eq!(row.run_command.as_deref(), Some("sleep 34"));
        assert!(daemon.is_alive(&SessionRef::Terminal(run.clone())));
        daemon
            .stop_run_for_repository(&workspace_id, Some("api"))
            .unwrap();
        assert!(daemon.store.run_terminals_in(&api_id).unwrap().is_empty());
    }

    /// The project's `run_command` setting (Settings → Project) is what
    /// `r` runs when it is set, over whatever `.nebula.json` says; blank,
    /// the file decides as before; a project's setting is its own; and a
    /// file that won't parse is still that file's error, not "no command".
    #[tokio::test]
    async fn the_project_setting_wins_over_the_project_file() {
        let daemon = test_daemon();
        let (dir, worktree) = run_worktree(&daemon);
        let config =
            |json: String| -> crate::config::Config { serde_json::from_str(&json).unwrap() };
        let entry = |path: &Path, run: &str| {
            config(format!(
                r#"{{"projects": {{"{}": {{"run_command": "{run}"}}}}}}"#,
                path.display()
            ))
        };
        let run_with = |cfg: &crate::config::Config| -> String {
            let EntityId::Terminal(id) = daemon.start_run_with(&worktree.id, cfg).unwrap() else {
                panic!("a run lives in a terminal");
            };
            let command = daemon.store.get_terminal(&id).unwrap().unwrap().run_command;
            daemon.stop_run(&worktree.id).unwrap();
            command.unwrap()
        };

        // No file: the setting is the whole answer.
        assert_eq!(run_with(&entry(dir.path(), "sleep 31")), "sleep 31");
        // Both: the setting wins.
        std::fs::write(dir.path().join(".nebula.json"), r#"{"run": "sleep 30"}"#).unwrap();
        assert_eq!(run_with(&entry(dir.path(), "sleep 31")), "sleep 31");
        // Blank, or another project's: the file.
        assert_eq!(run_with(&entry(dir.path(), "   ")), "sleep 30");
        assert_eq!(
            run_with(&entry(Path::new("/tmp/other"), "sleep 31")),
            "sleep 30"
        );
        // Neither: the message names both.
        std::fs::remove_file(dir.path().join(".nebula.json")).unwrap();
        let err = daemon
            .start_run_with(&worktree.id, &entry(dir.path(), ""))
            .unwrap_err();
        assert!(err.to_string().contains("Settings (s) → Project"), "{err}");
        // A broken file is its own complaint, whatever the setting isn't.
        std::fs::write(dir.path().join(".nebula.json"), "{").unwrap();
        let err = daemon
            .start_run_with(&worktree.id, &crate::config::Config::default())
            .unwrap_err();
        assert!(err.to_string().contains(".nebula.json:"), "{err}");
        assert_eq!(
            run_with(&entry(dir.path(), "sleep 31")),
            "sleep 31",
            "and the setting never opens the file"
        );
    }

    /// A run that ends on its own keeps its output for the attach that
    /// comes to read it, and never runs again on that attach — only `r`
    /// starts a RUN COMMAND.
    #[tokio::test]
    async fn an_exited_run_replays_instead_of_running_again() {
        let daemon = test_daemon();
        let (dir, worktree) = run_worktree(&daemon);
        std::fs::write(
            dir.path().join(".nebula.json"),
            r#"{"run": "echo run-finished"}"#,
        )
        .unwrap();
        let EntityId::Terminal(id) = daemon.start_run(&worktree.id).unwrap() else {
            panic!("a run lives in a terminal");
        };
        let sref = SessionRef::Terminal(id.clone());
        let deadline = Instant::now() + Duration::from_secs(20);
        while daemon.finished_run_exit(&sref).is_none() {
            assert!(Instant::now() < deadline, "the run never exited");
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        assert!(!daemon.is_alive(&sref));

        let session = daemon.ensure_session(&sref, 80, 24).unwrap();
        assert!(!daemon.is_alive(&sref), "an attach spawned nothing");
        let (_, bytes) = session.snapshot(None);
        assert!(
            String::from_utf8_lossy(&bytes).contains("run-finished"),
            "the replay is the run's output"
        );

        // `r` again reuses the row.
        assert_eq!(
            daemon.start_run(&worktree.id).unwrap(),
            EntityId::Terminal(id)
        );
        assert_eq!(
            daemon.store.run_terminals_in(&worktree.id).unwrap().len(),
            1
        );
        daemon.stop_run(&worktree.id).unwrap();
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
                custom_harness: None,
                model: None,
                effort: None,
                auto_title: false,
                cloud_prompt: Some(" \n ".into()),
                starting_prompt: None,
                pr_url: None,
                issue_url: None,
            })
            .await
            .unwrap_err();
        assert!(empty.to_string().contains("needs a task"));

        let nul = daemon
            .create_agent(CreateAgentSpec {
                worktree: worktree.clone(),
                name: "cloud".into(),
                kind: AgentKind::Claude,
                custom_harness: None,
                model: None,
                effort: None,
                auto_title: false,
                cloud_prompt: Some("fix\0auth".into()),
                starting_prompt: None,
                pr_url: None,
                issue_url: None,
            })
            .await
            .unwrap_err();
        assert!(nul.to_string().contains("NUL"));

        let too_long = daemon
            .create_agent(CreateAgentSpec {
                worktree: worktree.clone(),
                name: "cloud".into(),
                kind: AgentKind::Claude,
                custom_harness: None,
                model: None,
                effort: None,
                auto_title: false,
                cloud_prompt: Some("x".repeat(MAX_CLOUD_PROMPT_BYTES + 1)),
                starting_prompt: None,
                pr_url: None,
                issue_url: None,
            })
            .await
            .unwrap_err();
        assert!(too_long.to_string().contains("too long"));

        let wrong_kind = daemon
            .create_agent(CreateAgentSpec {
                worktree,
                name: "cloud".into(),
                kind: AgentKind::Codex,
                custom_harness: None,
                model: None,
                effort: None,
                auto_title: false,
                cloud_prompt: Some("Fix auth".into()),
                starting_prompt: None,
                pr_url: None,
                issue_url: None,
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
            custom_harness: None,
            model: None,
            effort: None,
            auto_title: false,
            cloud_prompt: cloud.map(String::from),
            starting_prompt: None,
            pr_url: Some("https://github.com/o/r/pull/7".into()),
            issue_url: None,
        };
        for kind in AgentKind::ALL {
            if kind == AgentKind::Custom {
                // Without a registry id the custom refusal comes first.
                let err = daemon.create_agent(spec(kind, None)).await.unwrap_err();
                assert!(err.to_string().contains("registry id"), "{err}");
                continue;
            }
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

    /// A PR SESSION's checkout is made once for the PR's head branch —
    /// fetched from `origin`, under the WORKTREE DIR, never the ROOT
    /// WORKTREE — and every later launch for that PR finds the same row.
    /// Only a PR whose branch the root itself has checked out runs there.
    #[tokio::test]
    async fn issue_launch_context_is_accepted_for_every_kind_but_never_with_cloud() {
        let daemon = test_daemon();
        let spec = |kind: AgentKind, cloud: Option<&str>| CreateAgentSpec {
            worktree: WorktreeId("unused".into()),
            name: "issue".into(),
            kind,
            custom_harness: None,
            model: None,
            effort: None,
            auto_title: true,
            cloud_prompt: cloud.map(String::from),
            starting_prompt: Some("Fix it".into()),
            pr_url: None,
            issue_url: Some("https://github.com/o/r/issues/15".into()),
        };
        for kind in AgentKind::ALL {
            if kind == AgentKind::Custom {
                let err = daemon.create_agent(spec(kind, None)).await.unwrap_err();
                assert!(err.to_string().contains("registry id"), "{err}");
                continue;
            }
            let err = daemon.create_agent(spec(kind, None)).await.unwrap_err();
            assert!(
                err.to_string().contains("worktree not found"),
                "{kind:?}: {err}"
            );
        }
        let cloud = daemon
            .create_agent(CreateAgentSpec {
                starting_prompt: None,
                ..spec(AgentKind::Claude, Some("Fix auth"))
            })
            .await
            .unwrap_err();
        assert!(cloud.to_string().contains("not supported for Claude Cloud"));
        let not_an_issue = CreateAgentSpec {
            issue_url: Some("https://github.com/o/r/pull/7".into()),
            ..spec(AgentKind::Codex, None)
        };
        let err = daemon.create_agent(not_an_issue).await.unwrap_err();
        assert!(err.to_string().contains("not an issue URL"), "{err}");
    }

    #[tokio::test]
    async fn starting_prompt_is_validated_and_never_adopts_a_warm_cli() {
        let daemon = test_daemon();
        let spec = |kind: AgentKind, cloud: Option<&str>, starting: Option<&str>| CreateAgentSpec {
            worktree: WorktreeId("unused".into()),
            name: "preset".into(),
            kind,
            custom_harness: None,
            model: None,
            effort: None,
            auto_title: true,
            cloud_prompt: cloud.map(String::from),
            starting_prompt: starting.map(String::from),
            pr_url: None,
            issue_url: None,
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

    /// Which launches boot straight into a turn — the question the
    /// optimistic `running` hangs on. Claude and Pi take a launch rule on
    /// their system-prompt flag, so only a task makes them work at once;
    /// Codex, Cursor and Muse have no such flag, so the rule itself opens
    /// their first prompt.
    #[test]
    fn a_launch_submits_a_first_prompt_when_it_carries_a_task_or_an_unflagged_rule() {
        for kind in AgentKind::ALL {
            if kind == AgentKind::Custom {
                continue; // no descriptor without a registry entry
            }
            let harness = resolve_harness(kind, None).unwrap();
            let flagged = harness.system.append_flag.is_some();
            assert!(
                Daemon::launch_submits_first_prompt(&harness, Some("Fix auth"), false),
                "{kind:?}: a task is always the first prompt"
            );
            assert!(
                Daemon::launch_submits_first_prompt(&harness, Some("Fix auth"), true),
                "{kind:?}: a task beside a rule too"
            );
            assert_eq!(
                Daemon::launch_submits_first_prompt(&harness, None, true),
                !flagged,
                "{kind:?}: a bare rule opens the first prompt only without the flag"
            );
            assert!(
                !Daemon::launch_submits_first_prompt(&harness, None, false),
                "{kind:?}: a bare launch parks at the CLI's own input"
            );
        }
    }

    /// The row the clients are told about is already `running` when the
    /// launch carries a task: the CLI submits it as it boots, and the
    /// session must not sit gray — and sort under everything mid-turn —
    /// for the seconds until its first hook lands.
    #[tokio::test]
    async fn a_create_with_a_task_broadcasts_a_running_row() {
        let daemon = test_daemon();
        let (dir, worktree) = run_worktree(&daemon);
        // `/bin/cat` stands in for the CLI: spawned verbatim, it blocks on
        // the PTY instead of running anything.
        let _cmd = EnvGuard::set(env::AGENT_CMD, "/bin/cat");
        let spec = |name: &str, task: Option<&str>| CreateAgentSpec {
            worktree: worktree.id.clone(),
            name: name.into(),
            kind: AgentKind::Claude,
            custom_harness: None,
            model: None,
            effort: None,
            auto_title: false,
            cloud_prompt: None,
            starting_prompt: task.map(String::from),
            pr_url: None,
            issue_url: None,
        };

        let created = |mut events: broadcast::Receiver<ServerEvent>| {
            std::iter::from_fn(|| events.try_recv().ok())
                .find_map(|e| match e {
                    ServerEvent::EntityUpserted {
                        entity: Entity::Agent(a),
                    } => Some(a),
                    _ => None,
                })
                .expect("the create broadcasts its row")
        };

        let events = daemon.events.subscribe();
        let EntityId::Agent(with_task) = daemon
            .create_agent(spec("task", Some("Fix auth")))
            .await
            .unwrap()
        else {
            panic!("a create makes an agent");
        };
        assert_eq!(created(events).status, AgentStatus::Running);
        assert_eq!(
            daemon.store.get_agent(&with_task).unwrap().unwrap().status,
            AgentStatus::Running,
            "and that is what a restart reads back"
        );
        // Seeded with its reprieve, so the CLI's startup progress-clear
        // cannot green it out before the turn begins.
        daemon.apply_hook_event(&with_task, HookEvent::Progress { busy: false }, None);
        assert_eq!(
            daemon.store.get_agent(&with_task).unwrap().unwrap().status,
            AgentStatus::Running
        );

        // A launch with nothing to do still parks at the CLI's input box.
        let events = daemon.events.subscribe();
        daemon.create_agent(spec("bare", None)).await.unwrap();
        assert_eq!(created(events).status, AgentStatus::Fresh);
        drop(dir);
    }

    /// Save/restore around a process-wide env var a test has to set.
    struct EnvGuard {
        key: &'static str,
        was: Option<String>,
    }

    impl EnvGuard {
        fn set(key: &'static str, value: &str) -> Self {
            let was = std::env::var(key).ok();
            std::env::set_var(key, value);
            Self { key, was }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            match self.was.take() {
                Some(v) => std::env::set_var(self.key, v),
                None => std::env::remove_var(self.key),
            }
        }
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
                custom_harness: None,
                model: None,
                effort: None,
                session_id: session_id.map(str::to_string),
                cloud_session_id: None,
                sort_order: 0,
                status_changed_at: 0,
                alive: false,
                issue_url: None,
                recent_prompts: Vec::new(),
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
    fn auto_rename_applies_once_and_defers_to_user_titles() {
        let daemon = test_daemon();
        seed_projects(&daemon, &["p"]);
        seed_worktree(&daemon, "p", "root", "/nebula-test/p", true);
        let pending_a1 = Agent {
            id: AgentId("a1".into()),
            worktree_id: WorktreeId("root".into()),
            name: "a1".into(),
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
            sort_order: 0,
            alive: false,
            issue_url: None,
            recent_prompts: Vec::new(),
        };
        daemon
            .store
            .insert_agent_with_auto_title(&pending_a1, true)
            .unwrap();
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
        let pending_a2 = Agent {
            id: AgentId("a2".into()),
            name: "a2".into(),
            ..pending_a1
        };
        daemon
            .store
            .insert_agent_with_auto_title(&pending_a2, true)
            .unwrap();
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
        assert!(cli_missing_message(AgentKind::Cursor.cli_program())
            .starts_with("cursor-agent was not found"));
        assert!(cli_missing_message(AgentKind::Claude.cli_program())
            .starts_with("claude was not found"));
        assert!(
            cli_missing_message(AgentKind::Codex.cli_program()).starts_with("codex was not found")
        );
        assert!(cli_missing_message(AgentKind::Pi.cli_program()).starts_with("pi was not found"));
        assert!(cli_missing_message("agy").starts_with("agy was not found"));
        // No "restart nebula": agent CLIs are spawned through the user's
        // login shell, so a fresh install is picked up on the next try.
        for kind in AgentKind::ALL {
            if kind == AgentKind::Custom {
                continue; // no static program; resolved through the registry
            }
            let msg = cli_missing_message(kind.cli_program());
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

    /// Switching `prewarm_agents` off drains the pool on the next sweep. A
    /// spare is a real CLI process the user can see — Claude's own
    /// `/list-agents` lists it beside their sessions, named after the
    /// directory (issue #15) — so the toggle has to take it away now, not
    /// when it ages out.
    #[tokio::test]
    async fn reap_prewarmed_drains_the_pool_once_prewarming_is_off() {
        let daemon = test_daemon();
        let key = (WorktreeId("w1".into()), AgentKind::Claude);
        let id = AgentId("warm-1".into());
        let sref = SessionRef::Agent(id.clone());
        let session = PtySession::spawn(
            sref.clone(),
            SpawnSpec {
                program: "sleep".into(),
                args: vec!["30".into()],
                cwd: std::env::temp_dir(),
                env: vec![],
                scrub_env: &[],
                cols: 80,
                rows: 24,
            },
        )
        .unwrap();
        daemon.install_session(session);
        daemon.prewarmed.lock().unwrap().insert(
            key.clone(),
            PrewarmEntry {
                agent_id: id.clone(),
                spawned_at: Instant::now(),
                model: None,
                effort: None,
                buffered_hooks: Vec::new(),
            },
        );
        let with_pool = |on: bool| crate::config::Config {
            prewarm_agents: on,
            ..crate::config::Config::default()
        };

        // Live, young and wanted: the ordinary sweep keeps it.
        daemon.reap_prewarmed_with(&with_pool(true));
        assert!(daemon.prewarmed.lock().unwrap().contains_key(&key));
        assert!(daemon.is_alive(&sref), "a kept spare keeps its PTY");

        // Off: the same sweep takes it, PTY and all.
        daemon.reap_prewarmed_with(&with_pool(false));
        assert!(daemon.prewarmed.lock().unwrap().is_empty());
        assert!(
            !daemon.is_alive(&sref),
            "a drained spare's PTY goes with it"
        );
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
