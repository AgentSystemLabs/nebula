use crate::entities::{
    Agent, AgentKind, AgentRole, AgentStatus, Entity, EntityId, Link, Project, TerminalTab,
    Worktree,
};
use crate::ids::{AgentId, LinkId, ProjectId, TerminalId, WorktreeId};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Highest IPC protocol this build speaks.
///
/// Bump on every protocol change. Additive changes keep
/// [`MIN_COMPATIBLE_PROTOCOL`] where it is; breaking changes bump both.
pub const PROTOCOL_VERSION: u32 = 51;

/// Oldest IPC protocol this build can safely talk to.
///
/// Compatibility is a range overlap: two peers can talk when each peer's
/// `[MIN_COMPATIBLE_PROTOCOL, PROTOCOL_VERSION]` range includes at least one
/// version the other peer also supports.
pub const MIN_COMPATIBLE_PROTOCOL: u32 = 49;

/// First protocol whose daemon answers `ClientRequest::CheckWorktree`. A
/// client asks only a daemon at least this new; an older one is never sent
/// a request it could not decode, and its delete confirm simply goes
/// without the check.
pub const CHECK_WORKTREE_PROTOCOL: u32 = 51;

pub fn protocol_ranges_overlap(
    local_min: u32,
    local_current: u32,
    peer_min: u32,
    peer_current: u32,
) -> bool {
    local_min <= peer_current && peer_min <= local_current
}

pub fn protocol_compatible_with(peer_version: u32) -> bool {
    peer_version >= MIN_COMPATIBLE_PROTOCOL
}

/// Max IPC frame size (length prefix sanity bound).
pub const MAX_FRAME_LEN: u32 = 4 * 1024 * 1024;

/// Cloud tasks ultimately cross an OS argv boundary (twice: the login
/// shell's `-c` string and Claude's own argv). Leave ample room for shell
/// quoting expansion and the rest of the environment on every platform.
pub const MAX_CLOUD_PROMPT_BYTES: usize = 16 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SessionRef {
    Agent(AgentId),
    Terminal(TerminalId),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ClientRequest {
    Hello {
        protocol_version: u32,
    },
    /// Reply is one Snapshot, then deltas stream on this connection forever.
    Subscribe,

    // -- PTY plane --
    Attach {
        session: SessionRef,
        /// Resume point for gap-free re-attach; None = replay whole ring.
        from_seq: Option<u64>,
        cols: u16,
        rows: u16,
    },
    Detach {
        session: SessionRef,
    },
    Input {
        session: SessionRef,
        #[serde(with = "serde_bytes")]
        data: Vec<u8>,
    },
    Resize {
        session: SessionRef,
        cols: u16,
        rows: u16,
    },

    // -- entity CRUD (RPC-style; answered by Ack/Error with matching req_id) --
    /// Register a repo as a project. Refused when `path` resolves to a
    /// repo this machine already knows — one repo is one project.
    AddProject {
        req_id: u64,
        path: PathBuf,
        name: Option<String>,
        /// Make `path` a repository if it isn't one: create the directory
        /// when it doesn't exist on disk, and `git init` it when it isn't
        /// inside a repository. Set only after the user confirmed in the
        /// client.
        create_missing: bool,
    },
    RemoveProject {
        req_id: u64,
        id: ProjectId,
    },
    /// Retitle a project's row. Purely cosmetic: `repo_path` — the folder on
    /// disk — is never touched. An empty name resets the row to the folder's
    /// own name, which is the only way back from a rename.
    RenameProject {
        req_id: u64,
        id: ProjectId,
        name: String,
    },
    /// Point a project at the folder its repo now lives in, after the user
    /// renamed or moved it on disk. `path` must be the repo's main checkout
    /// and not another project's. Every worktree row inside the old folder
    /// moves with it, git's own links are repaired, and a row still named
    /// after the old folder takes the new folder's name. Nothing on disk is
    /// moved: the folder is already where `path` says.
    SetProjectPath {
        req_id: u64,
        id: ProjectId,
        path: PathBuf,
    },
    CreateWorktree {
        req_id: u64,
        project: ProjectId,
        branch: String,
        base: Option<String>,
    },
    DeleteWorktree {
        req_id: u64,
        id: WorktreeId,
        force: bool,
    },
    /// What deleting this worktree would interrupt or lose — the DELETE
    /// CHECK a confirm shows before the checkout leaves the disk. Answered
    /// by `ServerEvent::WorktreeChecked` with the same req_id (not an Ack),
    /// or an `Error`. Reads only: nothing is touched. Gated on
    /// [`CHECK_WORKTREE_PROTOCOL`].
    CheckWorktree {
        req_id: u64,
        id: WorktreeId,
    },
    /// Opt-in cleanup for a linked worktree whose PR was detected as merged.
    /// The daemon re-checks the safety invariants before touching disk:
    /// never the root/default branch, branch and HEAD must still match the
    /// trusted PR row, the checkout must be clean, and sessions must be idle.
    CleanupMergedWorktree {
        req_id: u64,
        id: WorktreeId,
        branch: String,
        pr_number: u64,
        pr_url: String,
        head_sha: String,
    },
    CreateAgent {
        req_id: u64,
        worktree: WorktreeId,
        name: String,
        kind: AgentKind,
        /// Registry id of the custom harness, when `kind` is
        /// [`AgentKind::Custom`]. Persisted with the row like `model`.
        #[serde(default)]
        custom_harness: Option<String>,
        /// Model the CLI launches with; None = the CLI's own default.
        model: Option<String>,
        /// Reasoning effort the CLI launches with; None = the CLI's own default.
        effort: Option<String>,
        /// True when the user accepted the generated default name, marking
        /// the session eligible for one agent-driven auto-title (the CLI
        /// runs `nebula rename` on its first prompt).
        auto_title: bool,
        /// One-shot task for a fresh `claude --cloud <task>` launch. This is
        /// deliberately request-only: prompts are not persisted with Agent.
        #[serde(default)]
        cloud_prompt: Option<String>,
        /// The first turn handed to the CLI as its positional prompt
        /// (`claude "<text>"`, `codex "<text>"`, `cursor-agent "<text>"`) —
        /// what an AGENT PRESET launch composes from its prefix, the task
        /// and its postfix. Request-only like `cloud_prompt`: never
        /// persisted, so a RESUME can never replay it. Skips PREWARM POOL
        /// adoption, since a spare booted bare cannot be handed one.
        #[serde(default)]
        starting_prompt: Option<String>,
        /// The GitHub issue this session was created for — an ISSUE
        /// SESSION, launched from the ISSUES MODAL. Persisted with the row
        /// like a PR SESSION's URL, so every cold spawn and RESUME rebuilds
        /// the same issue context: Claude and Pi take it as an appended
        /// system prompt, a Codex / Cursor cold spawn as the opening of its
        /// first prompt. Skips PREWARM POOL adoption like `starting_prompt`,
        /// since a spare booted bare never got it.
        #[serde(default)]
        issue_url: Option<String>,
        /// Special role for the row. Only an orchestrator row may use the
        /// privileged orchestrator command surface.
        #[serde(default)]
        role: AgentRole,
    },
    /// Create a local AGENT of any kind from an OPEN PRS row — a PR
    /// SESSION. It never runs in the ROOT WORKTREE: the daemon finds the
    /// PROJECT's worktree checked out on the PR's head branch, or creates
    /// one (fetching the branch from `origin`, or the PR ref for a fork),
    /// and every PR SESSION for that PR shares it. The PR URL is persisted
    /// as launch context so every cold spawn and RESUME rebuilds the same
    /// PR-scoped rule — naming that worktree — as Claude's appended system
    /// prompt, or the first prompt a Codex / Cursor cold spawn opens with.
    /// Separate from CreateAgent so ordinary callers cannot accidentally
    /// opt into a partial PR launch. The reply's `created` is the AGENT;
    /// a created worktree arrives as its own `EntityUpserted` first.
    CreatePrAgent {
        req_id: u64,
        /// The PROJECT whose repo the pull request is open on.
        project: ProjectId,
        name: String,
        kind: AgentKind,
        /// Registry id of the custom harness, when `kind` is
        /// [`AgentKind::Custom`]. Persisted with the row like `model`.
        #[serde(default)]
        custom_harness: Option<String>,
        /// Model the CLI launches with; None = the CLI's own default.
        model: Option<String>,
        /// Reasoning effort the CLI launches with; None = default.
        effort: Option<String>,
        auto_title: bool,
        pr_url: String,
        /// The branch the PR SESSION's worktree is checked out on: the
        /// pull request's head branch (`gh`'s `headRefName`) for a
        /// same-repo pull request, and `<owner>/<headRefName>` for a
        /// fork's — a fork's `main` is not ours, and must match neither
        /// the ROOT WORKTREE nor a branch on `origin`. The client names
        /// it; the daemon fetches `origin`'s branch of that name and,
        /// finding none, seeds it from `refs/pull/N/head`.
        head: String,
        /// The CLI's positional first prompt — an AGENT PRESET picked on
        /// the OPEN PRS row composes one, under `CreateAgent`'s rules for
        /// `starting_prompt`. It rides beside the PR rule, which stays
        /// Claude's appended system prompt (or the opening of a Codex /
        /// Cursor cold spawn's first prompt). None: the CLI's own input
        /// is the first prompt. Request-only, never persisted.
        #[serde(default)]
        starting_prompt: Option<String>,
    },
    /// Fire-and-forget: pre-spawn an agent CLI for this (worktree, kind) so
    /// the next CreateAgent adopts an already-booted session. Sent the
    /// moment the user picks the kind, before they type the name. No reply;
    /// a missing CLI or failed spawn silently degrades to a cold spawn.
    PrewarmAgent {
        worktree: WorktreeId,
        kind: AgentKind,
        /// Must match the CreateAgent that follows or the warm session is
        /// discarded (a CLI booted with the wrong model can't be adopted).
        model: Option<String>,
        effort: Option<String>,
    },
    /// Fire-and-forget: pre-spawn every dead (non-archived) session under a
    /// worktree so attaching later replays an already-booted screen instead
    /// of watching a login shell + CLI boot. Sent once the worktree
    /// selection has rested (debounced client-side); already-alive sessions
    /// are untouched. No reply; a failed spawn degrades to today's lazy
    /// spawn-on-attach.
    PrewarmWorktreeSessions {
        worktree: WorktreeId,
        /// Pane size the sessions boot at, so the later Attach resizes to
        /// the same grid and full-screen apps need no reflow.
        cols: u16,
        rows: u16,
    },
    RenameAgent {
        req_id: u64,
        id: AgentId,
        name: String,
    },
    /// Agent-initiated one-shot title (`nebula rename` inside the session's
    /// CLI). Applies only while the session still awaits its auto-title;
    /// answered with Error (informational, not a fault) once a title —
    /// user- or agent-set — already sticks, so a user rename is never
    /// clobbered by a late or repeated agent attempt.
    AutoRenameAgent {
        req_id: u64,
        id: AgentId,
        name: String,
    },
    /// `nebula worktree <name>`, run by the agent from inside its own
    /// session: create the worktree `branch` under the agent's project (or
    /// take the existing one with that branch), re-home the agent row under
    /// it at once, and relocate the live session into it when its current
    /// turn ends — killed and respawned resumed there, with a prompt that
    /// tells the CLI where it now is. Replies with `WorktreeEntered`.
    EnterWorktree {
        req_id: u64,
        id: AgentId,
        branch: String,
        base: Option<String>,
    },
    /// `nebula spawn "<task>"`, run by the agent from inside its own
    /// session: start a new AGENT beside it — same WORKTREE unless
    /// `worktree` names a branch, and the same AGENT KIND / MODEL / EFFORT
    /// unless `kind` names another harness — with `starting_prompt` as the
    /// new CLI's first prompt, so it begins the task at once. The caller's
    /// own process is untouched. Answered with `Ack { created:
    /// Some(EntityId::Agent(..)) }`; the row reaches every TUI as an
    /// ordinary `EntityUpserted`.
    SpawnSiblingAgent {
        req_id: u64,
        id: AgentId,
        kind: Option<AgentKind>,
        starting_prompt: String,
        /// `--worktree <branch>`: start the session in the caller's
        /// PROJECT's worktree on this branch instead, created (as
        /// `EnterWorktree` creates one) when there is none. None: the
        /// caller's own worktree.
        #[serde(default)]
        worktree: Option<String>,
        /// `--base <ref>`: start point for a new `worktree` branch, resolved
        /// and refused as `EnterWorktree`'s `base` is (a branch that
        /// already exists is refused). Ignored without `worktree`.
        #[serde(default)]
        base: Option<String>,
    },
    /// `nebula open <file>…`, run by the agent from inside its own session:
    /// show these files to the user in every attached TUI's FILE TABS —
    /// one tab per file, the focused one previewed, Enter editing it.
    /// Paths are absolute (the CLI resolves them against its own cwd, which
    /// is the agent's). Answered with `Ack`; the files reach every
    /// subscriber as `FilesOpened`.
    OpenFiles {
        req_id: u64,
        id: AgentId,
        paths: Vec<PathBuf>,
    },
    /// Privileged orchestrator command: list the whole daemon tree in one
    /// bounded model-readable response. The daemon refuses this unless
    /// `caller` is the persisted orchestrator row.
    OrchestratorList {
        req_id: u64,
        caller: AgentId,
    },
    /// Privileged orchestrator command: read a bounded tail of a session's
    /// output. This is recent output only, not transcript export.
    OrchestratorRead {
        req_id: u64,
        caller: AgentId,
        session: SessionRef,
        max_bytes: u32,
    },
    /// Privileged orchestrator command: send a follow-up prompt to another
    /// local agent through the same PTY path as the TUI follow-up composer.
    OrchestratorSend {
        req_id: u64,
        caller: AgentId,
        target: AgentId,
        message: String,
    },
    /// Privileged orchestrator command: start an agent in any project and
    /// worktree. `worktree` reuses an existing checkout; otherwise `branch`
    /// is found or cut in `project`.
    OrchestratorSpawn {
        req_id: u64,
        caller: AgentId,
        project: ProjectId,
        worktree: Option<WorktreeId>,
        branch: Option<String>,
        base: Option<String>,
        name: String,
        kind: AgentKind,
        #[serde(default)]
        custom_harness: Option<String>,
        model: Option<String>,
        effort: Option<String>,
        starting_prompt: String,
    },
    /// Privileged orchestrator command: ask every TUI client to open the
    /// tabbed review modal.
    OrchestratorOpenReview {
        req_id: u64,
        caller: AgentId,
        sessions: Vec<SessionRef>,
        tabs: Vec<ReviewTabKind>,
    },
    /// Agent-readable list of every session across every project.
    ListSessions {
        req_id: u64,
    },
    /// Agent-readable recent context for one session, addressed by id or
    /// fuzzy name. Prefers harness transcripts when the daemon knows one.
    ReadSession {
        req_id: u64,
        target: String,
        max_lines: u32,
    },
    /// Send a labelled question to another local agent, optionally waiting
    /// for it to become idle first, and answer with clean text from that
    /// turn's output.
    AskSession {
        req_id: u64,
        caller: Option<AgentId>,
        target: String,
        question: String,
        timeout_ms: u64,
        wait: bool,
    },
    /// Kills the PTY, sets archived=1.
    ArchiveAgent {
        req_id: u64,
        id: AgentId,
    },
    UnarchiveAgent {
        req_id: u64,
        id: AgentId,
    },
    DeleteAgent {
        req_id: u64,
        id: AgentId,
    },
    /// Respawn; resumes the stored session id (`claude --resume` /
    /// `codex resume` / `cursor-agent --resume`) when one is stored.
    RestartAgent {
        req_id: u64,
        id: AgentId,
    },
    /// The user moved a session's card onto another checkout, of its own
    /// project or another one: the card menu's **Move to…**, or a drag
    /// onto another band or project tab.
    /// The row moves at once and a live session follows, resumed there:
    /// straight away when idle, at its turn's end when mid-turn. Answered
    /// with `Ack`.
    MoveAgent {
        req_id: u64,
        id: AgentId,
        worktree: WorktreeId,
    },
    /// Queue a message on the Claude Cloud session a row launched
    /// (`claude -p <message> --cloud <id>`). Fire-and-forget by nature: the
    /// CLI acknowledges the send and returns, and the reply only ever
    /// appears in the session's page in the browser. Rejected for rows
    /// without a `cloud_session_id`, and bounded by
    /// [`MAX_CLOUD_PROMPT_BYTES`] like the launch task.
    SendCloudMessage {
        req_id: u64,
        id: AgentId,
        message: String,
    },
    CreateTerminal {
        req_id: u64,
        worktree: WorktreeId,
        name: Option<String>,
    },
    /// Rewrite a link's URL. It is normalized daemon-side (a bare
    /// `github.com/...` gains an https:// scheme) and refused if it can't
    /// be made into an http(s) URL.
    UpdateLink {
        req_id: u64,
        id: LinkId,
        url: String,
    },
    DeleteLink {
        req_id: u64,
        id: LinkId,
    },
    RenameTerminal {
        req_id: u64,
        id: TerminalId,
        name: String,
    },
    CloseTerminal {
        req_id: u64,
        id: TerminalId,
    },
    /// `r` on a worktree: start the project's RUN COMMAND — the `run` of
    /// the `.nebula.json` in that checkout, else the main checkout's, read
    /// fresh — in the worktree's RUN TERMINAL, a login shell running that
    /// line and nothing else. An exited run's row is reused; a run still
    /// going is left alone and named in the reply, so a second client's
    /// press never starts a second one. Answered with
    /// `Ack { created: Some(EntityId::Terminal(..)) }`; no file or no
    /// command is an Error saying what to add.
    StartRun {
        req_id: u64,
        worktree: WorktreeId,
    },
    /// `r` again: kill the worktree's RUN TERMINAL and drop its row.
    /// Nothing running is not an error.
    StopRun {
        req_id: u64,
        worktree: WorktreeId,
    },

    /// Fire-and-forget opaque TUI blob (last selection etc.).
    SaveUiState {
        json: String,
    },

    /// Fire-and-forget: the user just opened this pull request, so
    /// everything up to `marker` has now been read.
    MarkPrSeen {
        url: String,
        marker: String,
    },

    /// Fire-and-forget: this agent's session is on screen, so a turn it
    /// finished unwatched (`Agent::unseen`) has now been looked at. The
    /// daemon answers with the agent's upsert when the flag actually flips.
    MarkAgentSeen {
        id: AgentId,
    },

    /// One point-in-time memory reading — the daemon plus every live
    /// session's process subtree. Answered by `ServerEvent::Metrics` with
    /// the same req_id (not an Ack).
    GetMetrics {
        req_id: u64,
    },

    /// The end of a live session's output ring — what a TERMINAL's card on
    /// the grid shows as the last lines its shell printed. Answered by
    /// `ServerEvent::OutputTail` with the same req_id (not an Ack).
    /// `after_seq` is the ring end the client last heard: a ring that has
    /// not grown past it answers with no bytes, so a grid asking after
    /// every terminal on it once a second costs the idle ones nothing.
    TailOutput {
        req_id: u64,
        session: SessionRef,
        /// At most this many bytes, from the end of the ring.
        max_bytes: u32,
        after_seq: Option<u64>,
    },

    Shutdown,
}

/// How much of a pull request's conversation the user had already seen the
/// last time they opened it. `marker` is the newest thing anyone else had
/// posted at that moment, as GitHub's RFC 3339 stamp — those sort
/// lexicographically, so "arrived since" is a string compare and nebula
/// never has to consult a clock. Empty means the PR was opened while its
/// conversation was still empty.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PrSeen {
    pub url: String,
    pub marker: String,
}

/// Memory usage of one live session: the PTY child plus every descendant
/// (an agent CLI typically fans out into node workers, shells, MCP servers).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionMetrics {
    pub session: SessionRef,
    /// OS pid of the PTY child (the subtree's root).
    pub pid: u32,
    /// Resident set size summed over the whole subtree, bytes.
    pub rss_bytes: u64,
    /// Live processes in the subtree, the root included.
    pub procs: u32,
    /// Set when the session is a prewarm-pool spare: an agent CLI the
    /// daemon booted ahead of time for this worktree, waiting for the next
    /// new-agent request there to adopt it. It has no agent row yet, so
    /// this is the only handle a client has for naming and placing it.
    #[serde(default)]
    pub prewarm: Option<PrewarmInfo>,
}

/// Where a prewarm-pool spare is homed and what it booted as.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PrewarmInfo {
    pub worktree: WorktreeId,
    pub kind: AgentKind,
    pub model: Option<String>,
}

/// The end of a session's ring, for `ClientRequest::TailOutput`: the bytes
/// and the PTY size they were laid out against, so the client's throwaway
/// screen wraps them where the pane would.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OutputTail {
    pub cols: u16,
    pub rows: u16,
    /// Seq the ring's next byte gets — sent back as the next ask's
    /// `after_seq`.
    pub end_seq: u64,
    /// The last `max_bytes` of the ring; empty when it has not grown past
    /// `after_seq`.
    #[serde(with = "serde_bytes")]
    pub data: Vec<u8>,
}

/// One row in the agent-readable `nebula sessions` listing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionSummary {
    pub session: SessionRef,
    pub id: String,
    pub name: String,
    pub project_id: ProjectId,
    pub project: String,
    pub worktree_id: WorktreeId,
    pub worktree: PathBuf,
    pub branch: String,
    /// The agent harness (`claude`, `codex`, ...), or `terminal`.
    pub harness: String,
    /// Model-facing status: `idle`, `running`, `needs_feedback`,
    /// `terminated`, or `disconnected`.
    pub status: String,
    pub alive: bool,
    /// One-line recent context: newest user prompt when known, else title.
    pub summary: String,
}

/// Daemon-side half of the metrics modal's data; the client stacks its own
/// RSS on top. Session subtrees are daemon descendants, so `daemon_rss_bytes`
/// counts the daemon process alone — the total stays double-count-free.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetricsSnapshot {
    pub daemon_pid: u32,
    pub daemon_rss_bytes: u64,
    /// Physical memory installed on the machine, bytes; 0 = unknown.
    pub system_total_bytes: u64,
    pub sessions: Vec<SessionMetrics>,
}

/// What deleting a worktree would interrupt or lose, as the DAEMON found
/// it just now (`ClientRequest::CheckWorktree`). Every part is best effort
/// and says so when it could not look: a check that failed is not a clean
/// bill of health.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorktreeCheck {
    /// Processes whose working directory is inside the checkout — a build,
    /// an editor, a shell, the tool calls of an agent whose own session
    /// lives elsewhere — other than the worktree's own sessions, which the
    /// confirm already counts and which go down with it.
    pub processes: Vec<WorktreeProcess>,
    /// Why the process sweep could not run, when it could not.
    pub process_error: Option<String>,
    /// Paths `git status` reports: uncommitted edits, untracked files, and
    /// submodules with changes or new commits.
    pub changes: u32,
    /// Epoch ms of the newest write among those paths — "still being
    /// worked on" when it is a moment ago. None with no changes.
    pub newest_change_ms: Option<i64>,
    /// Commits HEAD has that `base` lacks. The branch keeps them after the
    /// checkout goes, unless `detached`.
    pub ahead: u32,
    /// What `ahead` was counted against (`origin/main`); None when nothing
    /// could be found to compare with.
    pub base: Option<String>,
    /// HEAD is detached: `ahead` then counts commits no branch or remote
    /// holds, which the delete leaves to the reflog alone.
    pub detached: bool,
    /// Why git could not be asked, when it could not.
    pub git_error: Option<String>,
}

/// One process `WorktreeCheck` found working inside a checkout.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorktreeProcess {
    pub pid: u32,
    /// The executable's short name (`cargo`, `zsh`).
    pub name: String,
}

impl WorktreeCheck {
    /// Did the check find anything a delete would interrupt or lose — or
    /// fail to look at something it should have?
    pub fn has_findings(&self) -> bool {
        !self.processes.is_empty()
            || self.process_error.is_some()
            || self.changes > 0
            || self.ahead > 0
            || self.git_error.is_some()
    }
}

/// What `EnterWorktree` did to the agent's live session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum EnterOutcome {
    /// The agent already lived in that worktree; nothing changed.
    AlreadyThere,
    /// The row moved; the live session respawns inside the worktree, resumed,
    /// once its current turn ends.
    Relocating,
    /// The row moved and nothing was running: the next launch lands there.
    NextLaunch,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewTabKind {
    Terminal,
    Diff,
    History,
    PullRequest,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ServerEvent {
    HelloOk {
        protocol_version: u32,
        daemon_pid: u32,
    },
    Incompatible {
        daemon_protocol_version: u32,
    },
    Snapshot {
        projects: Vec<Project>,
        worktrees: Vec<Worktree>,
        agents: Vec<Agent>,
        terminals: Vec<TerminalTab>,
        links: Vec<Link>,
        /// How far the user has read into each pull request they've opened.
        pr_seen: Vec<PrSeen>,
        ui_state: Option<String>,
    },

    Ack {
        req_id: u64,
        created: Option<EntityId>,
    },
    /// Reply to `EnterWorktree`: the worktree the agent now belongs to, and
    /// what that meant for its process.
    WorktreeEntered {
        req_id: u64,
        worktree: Worktree,
        outcome: EnterOutcome,
    },
    Error {
        req_id: Option<u64>,
        message: String,
    },
    /// Reply to `OrchestratorList`.
    OrchestratorList {
        req_id: u64,
        projects: Vec<Project>,
        worktrees: Vec<Worktree>,
        agents: Vec<Agent>,
        terminals: Vec<TerminalTab>,
        links: Vec<Link>,
    },
    /// Reply to `ListSessions`.
    SessionList {
        req_id: u64,
        sessions: Vec<SessionSummary>,
    },
    /// Reply to `ReadSession`.
    SessionText {
        req_id: u64,
        session: SessionRef,
        text: String,
    },
    /// Reply to `AskSession`.
    SessionAnswer {
        req_id: u64,
        session: AgentId,
        answer: String,
    },
    /// Broadcast by `OrchestratorOpenReview`: clients open a local review
    /// modal for these sessions and tabs.
    ReviewOpened {
        opener: AgentId,
        sessions: Vec<SessionRef>,
        tabs: Vec<ReviewTabKind>,
    },
    /// An `Attach` the DAEMON could not honour: the session's process was
    /// not running and could not be started (its checkout is gone from
    /// disk, its CLI is missing, it is archived, ...). `Attach` carries no
    /// request id, so a plain `Error` could not say which pane it answers;
    /// this one names the session, so the pane that is waiting on it can
    /// say why instead of booting forever.
    AttachRefused {
        session: SessionRef,
        message: String,
    },

    // -- deltas (pushed to all subscribers) --
    EntityUpserted {
        entity: Entity,
    },
    EntityRemoved {
        id: EntityId,
    },
    StatusChanged {
        agent: AgentId,
        status: AgentStatus,
        /// Epoch ms the change was stamped with (matches the persisted
        /// `status_changed_at`, so clients regroup consistently).
        changed_at: i64,
        /// The agent's `unseen` flag after this change: set when a live
        /// turn just finished, cleared when it left `finished`.
        #[serde(default)]
        unseen: bool,
    },

    /// `nebula open` from an agent session: the files the user asked to
    /// see, for every subscriber to raise its FILE TABS on. `root` is the
    /// agent's worktree checkout — the editor's cwd, and what the tab
    /// labels are relative to.
    FilesOpened {
        agent: AgentId,
        root: PathBuf,
        paths: Vec<PathBuf>,
    },

    // -- PTY plane (only to clients attached to that session) --
    /// Ring replay on attach; client resets its parser before applying.
    Scrollback {
        session: SessionRef,
        base_seq: u64,
        #[serde(with = "serde_bytes")]
        data: Vec<u8>,
    },
    /// Live coalesced output. `seq` = byte offset of the first byte.
    Output {
        session: SessionRef,
        seq: u64,
        #[serde(with = "serde_bytes")]
        data: Vec<u8>,
    },
    SessionExited {
        session: SessionRef,
        exit_code: Option<i32>,
    },
    /// The child's kitty-keyboard-protocol flags changed (or, right after
    /// Scrollback on attach, the current value). 0 = legacy encoding.
    KittyFlags {
        session: SessionRef,
        flags: u8,
    },

    /// Reply to `ClientRequest::CheckWorktree`.
    WorktreeChecked {
        req_id: u64,
        id: WorktreeId,
        check: WorktreeCheck,
    },
    /// Reply to `ClientRequest::GetMetrics`.
    Metrics {
        req_id: u64,
        snapshot: MetricsSnapshot,
    },
    /// Reply to `ClientRequest::TailOutput`: `None` when the session has no
    /// live PTY — its ring went with its shell.
    OutputTail {
        req_id: u64,
        session: SessionRef,
        tail: Option<OutputTail>,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn protocol_ranges_overlap_when_either_side_is_additive() {
        assert!(protocol_ranges_overlap(47, 47, 47, 48));
        assert!(protocol_ranges_overlap(47, 48, 47, 47));
        assert!(!protocol_ranges_overlap(48, 48, 47, 47));
        assert!(!protocol_ranges_overlap(47, 47, 48, 48));
    }

    #[test]
    fn protocol_compatibility_accepts_current_and_additive_peers() {
        assert!(protocol_compatible_with(PROTOCOL_VERSION));
        assert!(protocol_compatible_with(PROTOCOL_VERSION + 1));
        assert!(!protocol_compatible_with(MIN_COMPATIBLE_PROTOCOL - 1));
    }
}
