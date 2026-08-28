//! SQLite persistence. Write volume is trivial (entity CRUD + status
//! changes), so a mutex-guarded connection is sufficient — no ORM, no
//! connection pool.

use anyhow::{Context, Result};
use nebula_core::{
    Agent, AgentId, AgentKind, AgentStatus, Link, LinkId, PrSeen, Project, ProjectId, TerminalId,
    TerminalTab, Workspace, WorkspaceId, Worktree, WorktreeId, DEFAULT_WORKSPACE_ID,
};
use rusqlite::{params, Connection};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

const MIGRATIONS: &[&str] = &[
    // 1: initial schema
    "
    CREATE TABLE projects (
      id          TEXT PRIMARY KEY,
      name        TEXT NOT NULL,
      repo_path   TEXT NOT NULL UNIQUE,
      sort_order  INTEGER NOT NULL DEFAULT 0,
      created_at  INTEGER NOT NULL
    );
    CREATE TABLE worktrees (
      id          TEXT PRIMARY KEY,
      project_id  TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
      path        TEXT NOT NULL,
      branch      TEXT NOT NULL,
      is_main     INTEGER NOT NULL DEFAULT 0,
      sort_order  INTEGER NOT NULL DEFAULT 0,
      created_at  INTEGER NOT NULL,
      UNIQUE (project_id, path)
    );
    CREATE TABLE agents (
      id                TEXT PRIMARY KEY,
      worktree_id       TEXT NOT NULL REFERENCES worktrees(id) ON DELETE CASCADE,
      name              TEXT NOT NULL,
      status            TEXT NOT NULL DEFAULT 'fresh',
      archived          INTEGER NOT NULL DEFAULT 0,
      claude_session_id TEXT,
      sort_order        INTEGER NOT NULL DEFAULT 0,
      created_at        INTEGER NOT NULL,
      status_changed_at INTEGER NOT NULL DEFAULT 0
    );
    CREATE TABLE terminals (
      id          TEXT PRIMARY KEY,
      worktree_id TEXT NOT NULL REFERENCES worktrees(id) ON DELETE CASCADE,
      name        TEXT NOT NULL DEFAULT 'shell',
      sort_order  INTEGER NOT NULL DEFAULT 0,
      created_at  INTEGER NOT NULL
    );
    CREATE TABLE ui_state (
      id    INTEGER PRIMARY KEY CHECK (id = 1),
      json  TEXT NOT NULL
    );
    ",
    // 2: project group dividers
    "
    ALTER TABLE projects ADD COLUMN divider_after INTEGER NOT NULL DEFAULT 0;
    ",
    // 3: divider labels
    "
    ALTER TABLE projects ADD COLUMN divider_label TEXT;
    ",
    // 4: agent kind (claude | codex); claude_session_id doubles as the
    // resume id for whichever kind the agent runs.
    "
    ALTER TABLE agents ADD COLUMN kind TEXT NOT NULL DEFAULT 'claude';
    ",
    // 5: pinned agents (their own group in the sessions list)
    "
    ALTER TABLE agents ADD COLUMN pinned INTEGER NOT NULL DEFAULT 0;
    ",
    // 6: pinned worktrees (their own group in the worktrees list)
    "
    ALTER TABLE worktrees ADD COLUMN pinned INTEGER NOT NULL DEFAULT 0;
    ",
    // 7: the leading divider — drawn above the whole list, owned by the
    // first project
    "
    ALTER TABLE projects ADD COLUMN divider_before INTEGER NOT NULL DEFAULT 0;
    ALTER TABLE projects ADD COLUMN divider_before_label TEXT;
    ",
    // 8: per-worktree todo notes
    "
    CREATE TABLE todos (
      id          TEXT PRIMARY KEY,
      worktree_id TEXT NOT NULL REFERENCES worktrees(id) ON DELETE CASCADE,
      text        TEXT NOT NULL,
      done        INTEGER NOT NULL DEFAULT 0,
      sort_order  INTEGER NOT NULL DEFAULT 0,
      created_at  INTEGER NOT NULL
    );
    ",
    // 9: per-agent model/effort launch options (NULL = CLI default)
    "
    ALTER TABLE agents ADD COLUMN model TEXT;
    ALTER TABLE agents ADD COLUMN effort TEXT;
    ",
    // 10: todos gain a project scope — exactly one of project_id /
    // worktree_id is set. Table rebuild: SQLite can't relax the old
    // NOT NULL worktree_id in place. Existing rows stay worktree-owned.
    "
    CREATE TABLE todos_new (
      id          TEXT PRIMARY KEY,
      project_id  TEXT REFERENCES projects(id) ON DELETE CASCADE,
      worktree_id TEXT REFERENCES worktrees(id) ON DELETE CASCADE,
      text        TEXT NOT NULL,
      done        INTEGER NOT NULL DEFAULT 0,
      sort_order  INTEGER NOT NULL DEFAULT 0,
      created_at  INTEGER NOT NULL,
      CHECK ((project_id IS NULL) <> (worktree_id IS NULL))
    );
    INSERT INTO todos_new (id, worktree_id, text, done, sort_order, created_at)
      SELECT id, worktree_id, text, done, sort_order, created_at FROM todos;
    DROP TABLE todos;
    ALTER TABLE todos_new RENAME TO todos;
    ",
    // 11: when the agent was archived (orders the ARCHIVED group
    // newest-first; 0 for rows archived before this migration)
    "
    ALTER TABLE agents ADD COLUMN archived_at INTEGER NOT NULL DEFAULT 0;
    ",
    // 12: sessions created with the generated default name await one
    // agent-driven auto-title (`nebula rename` from inside the CLI);
    // cleared by the first rename, user- or agent-made. Daemon-internal —
    // never leaves the store, so pre-existing rows defaulting to 0 simply
    // keep their names.
    "
    ALTER TABLE agents ADD COLUMN auto_title_pending INTEGER NOT NULL DEFAULT 0;
    ",
    // 13: workspaces — named project groups, exactly one open (`active`) at
    // a time. Every install gets the built-in 'default' workspace and all
    // pre-existing projects move into it. The new projects column stays
    // nullable (SQLite forbids a non-NULL default on an added REFERENCES
    // column); reads COALESCE to 'default'.
    "
    CREATE TABLE workspaces (
      id          TEXT PRIMARY KEY,
      name        TEXT NOT NULL UNIQUE,
      active      INTEGER NOT NULL DEFAULT 0,
      created_at  INTEGER NOT NULL
    );
    INSERT INTO workspaces (id, name, active, created_at) VALUES ('default', 'default', 1, 0);
    ALTER TABLE projects ADD COLUMN workspace_id TEXT REFERENCES workspaces(id);
    UPDATE projects SET workspace_id = 'default';
    ",
    // 14: workspaces are free-form groupings, so the same repo may be added
    // to any number of them — uniqueness moves from a global repo_path
    // constraint to (workspace, repo_path). Table rebuild: SQLite can't
    // drop the inline UNIQUE. Runs with foreign keys off (see migrate())
    // so the DROP doesn't cascade into worktrees/agents/terminals/todos.
    "
    CREATE TABLE projects_new (
      id          TEXT PRIMARY KEY,
      name        TEXT NOT NULL,
      repo_path   TEXT NOT NULL,
      sort_order  INTEGER NOT NULL DEFAULT 0,
      created_at  INTEGER NOT NULL,
      divider_after INTEGER NOT NULL DEFAULT 0,
      divider_label TEXT,
      divider_before INTEGER NOT NULL DEFAULT 0,
      divider_before_label TEXT,
      workspace_id TEXT REFERENCES workspaces(id)
    );
    INSERT INTO projects_new (id, name, repo_path, sort_order, created_at, divider_after, divider_label, divider_before, divider_before_label, workspace_id)
      SELECT id, name, repo_path, sort_order, created_at, divider_after, divider_label, divider_before, divider_before_label, workspace_id FROM projects;
    DROP TABLE projects;
    ALTER TABLE projects_new RENAME TO projects;
    CREATE UNIQUE INDEX projects_workspace_repo ON projects (COALESCE(workspace_id, 'default'), repo_path);
    ",
    // 15: todos are now "notes" everywhere — rename the table to match.
    "
    ALTER TABLE todos RENAME TO notes;
    ",
    // 16: per-worktree links — pull requests, tickets, docs. Worktree-only
    // (unlike notes): a link describes the branch's work, and a project's
    // links would be the same for every checkout.
    "
    CREATE TABLE links (
      id          TEXT PRIMARY KEY,
      worktree_id TEXT NOT NULL REFERENCES worktrees(id) ON DELETE CASCADE,
      url         TEXT NOT NULL,
      sort_order  INTEGER NOT NULL DEFAULT 0,
      created_at  INTEGER NOT NULL
    );
    ",
    // 17: how far the user has read into a pull request's conversation.
    // Keyed by URL rather than worktree — the PR is the thing that grows
    // comments, it outlives the checkout, and the same one can be pinned to
    // more than one of them.
    "
    CREATE TABLE pr_seen (
      url      TEXT PRIMARY KEY,
      marker   TEXT NOT NULL,
      seen_at  INTEGER NOT NULL
    );
    ",
    // 18: project group dividers are gone (migrations 2, 3 and 7 added
    // them; 14 carried them through the table rebuild). Plain columns with
    // no index or constraint, so DROP COLUMN is enough.
    "
    ALTER TABLE projects DROP COLUMN divider_after;
    ALTER TABLE projects DROP COLUMN divider_label;
    ALTER TABLE projects DROP COLUMN divider_before;
    ALTER TABLE projects DROP COLUMN divider_before_label;
    ",
    // 19: a turn finished while nobody was looking (see `Agent::unseen`).
    // Set by `set_agent_status` on a live → finished flip, cleared by
    // `mark_agent_seen`, by leaving `finished`, and by archiving.
    "
    ALTER TABLE agents ADD COLUMN unseen INTEGER NOT NULL DEFAULT 0;
    ",
    // 20: the Claude Cloud session a row launched (`Agent::cloud_session_id`),
    // read off the `claude --cloud` spawn's output. Drives the attach /
    // teleport restart path; NULL for every local row.
    "
    ALTER TABLE agents ADD COLUMN cloud_session_id TEXT;
    ",
    // 21: notes are gone (migration 8 created them as `todos`, 10 gave them
    // a project scope, 15 renamed the table). Nothing else references the
    // table, so a plain DROP retires the feature and its rows.
    "
    DROP TABLE IF EXISTS notes;
    ",
];

pub struct Store {
    conn: Mutex<Connection>,
}

pub type TreeRows = (Vec<Project>, Vec<Worktree>, Vec<Agent>, Vec<TerminalTab>);

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

impl Store {
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let conn = Connection::open(path).with_context(|| format!("open {}", path.display()))?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        let store = Self {
            conn: Mutex::new(conn),
        };
        store.migrate()?;
        Ok(store)
    }

    pub fn open_in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory()?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        let store = Self {
            conn: Mutex::new(conn),
        };
        store.migrate()?;
        Ok(store)
    }

    fn migrate(&self) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        let version: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        // Rebuild-style migrations DROP a parent table (14 rebuilds
        // projects); with enforcement on, the DROP's implicit delete would
        // cascade into every child table. Standard SQLite rebuild procedure:
        // foreign keys off for the migration window, back on after. (On a
        // migration error the connection is abandoned with Store::open's
        // failure, so the early return never leaks a live FK-off handle.)
        conn.pragma_update(None, "foreign_keys", "OFF")?;
        for (i, migration) in MIGRATIONS.iter().enumerate().skip(version as usize) {
            conn.execute_batch(&format!(
                "BEGIN; {migration}; PRAGMA user_version = {}; COMMIT;",
                i + 1
            ))
            .with_context(|| format!("migration {}", i + 1))?;
        }
        conn.pragma_update(None, "foreign_keys", "ON")?;
        Ok(())
    }

    // ---- workspaces ----

    pub fn insert_workspace(&self, w: &Workspace) -> Result<()> {
        self.conn.lock().unwrap().execute(
            "INSERT INTO workspaces (id, name, active, created_at) VALUES (?1, ?2, 0, ?3)",
            params![w.id.as_str(), w.name, now_ms()],
        )?;
        Ok(())
    }

    pub fn rename_workspace(&self, id: &WorkspaceId, name: &str) -> Result<()> {
        self.conn.lock().unwrap().execute(
            "UPDATE workspaces SET name = ?2 WHERE id = ?1",
            params![id.as_str(), name],
        )?;
        Ok(())
    }

    /// `DELETE FROM <table> WHERE id = ?1` — every entity delete is exactly
    /// this one statement, the schema's cascades taking the children with
    /// the row.
    fn delete_by_id(&self, table: &'static str, id: &str) -> Result<()> {
        self.conn
            .lock()
            .unwrap()
            .execute(&format!("DELETE FROM {table} WHERE id = ?1"), params![id])?;
        Ok(())
    }

    pub fn delete_workspace(&self, id: &WorkspaceId) -> Result<()> {
        self.delete_by_id("workspaces", id.as_str())
    }

    /// Every workspace, oldest first (the 'default' one leads — it is
    /// created at time 0 by the migration).
    pub fn load_workspaces(&self) -> Result<Vec<Workspace>> {
        let conn = self.conn.lock().unwrap();
        let workspaces = conn
            .prepare(&format!(
                "SELECT {WORKSPACE_COLUMNS} FROM workspaces ORDER BY created_at, id"
            ))?
            .query_map([], row_to_workspace)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(workspaces)
    }

    pub fn get_workspace(&self, id: &WorkspaceId) -> Result<Option<Workspace>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(&format!(
            "SELECT {WORKSPACE_COLUMNS} FROM workspaces WHERE id = ?1"
        ))?;
        let mut rows = stmt.query(params![id.as_str()])?;
        Ok(rows.next()?.map(row_to_workspace).transpose()?)
    }

    pub fn workspace_by_name(&self, name: &str) -> Result<Option<WorkspaceId>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare("SELECT id FROM workspaces WHERE name = ?1")?;
        let mut rows = stmt.query(params![name])?;
        Ok(rows
            .next()?
            .map(|r| r.get::<_, String>(0))
            .transpose()?
            .map(WorkspaceId))
    }

    /// The open workspace. Falls back to 'default' if no row is flagged
    /// (never expected — the migration flags it and switches keep exactly
    /// one flag set).
    pub fn active_workspace_id(&self) -> Result<WorkspaceId> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare("SELECT id FROM workspaces WHERE active = 1 LIMIT 1")?;
        let mut rows = stmt.query([])?;
        Ok(rows
            .next()?
            .map(|r| r.get::<_, String>(0))
            .transpose()?
            .map(WorkspaceId)
            .unwrap_or_default())
    }

    pub fn set_active_workspace(&self, id: &WorkspaceId) -> Result<()> {
        self.conn.lock().unwrap().execute(
            "UPDATE workspaces SET active = (id = ?1)",
            params![id.as_str()],
        )?;
        Ok(())
    }

    pub fn count_workspace_projects(&self, id: &WorkspaceId) -> Result<i64> {
        Ok(self.conn.lock().unwrap().query_row(
            "SELECT COUNT(*) FROM projects WHERE COALESCE(workspace_id, ?2) = ?1",
            params![id.as_str(), DEFAULT_WORKSPACE_ID],
            |r| r.get(0),
        )?)
    }

    pub fn count_workspaces(&self) -> Result<i64> {
        Ok(self
            .conn
            .lock()
            .unwrap()
            .query_row("SELECT COUNT(*) FROM workspaces", [], |r| r.get(0))?)
    }

    // ---- projects ----

    pub fn insert_project(&self, p: &Project) -> Result<()> {
        self.conn.lock().unwrap().execute(
            "INSERT INTO projects (id, name, workspace_id, repo_path, sort_order, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![p.id.as_str(), p.name, p.workspace_id.as_str(), p.repo_path.to_string_lossy(), p.sort_order, now_ms()],
        )?;
        Ok(())
    }

    /// Sort slot for a newly added project: after everything else.
    pub fn next_project_sort_order(&self) -> Result<i64> {
        Ok(self.conn.lock().unwrap().query_row(
            "SELECT COALESCE(MAX(sort_order) + 1, 0) FROM projects",
            [],
            |r| r.get(0),
        )?)
    }

    /// Persist a project's list position (its sort order).
    pub fn set_project_position(&self, p: &Project) -> Result<()> {
        self.conn.lock().unwrap().execute(
            "UPDATE projects SET sort_order = ?2 WHERE id = ?1",
            params![p.id.as_str(), p.sort_order],
        )?;
        Ok(())
    }

    pub fn delete_project(&self, id: &ProjectId) -> Result<()> {
        self.delete_by_id("projects", id.as_str())
    }

    /// The project row for `path` within one workspace. Repo paths may
    /// repeat across workspaces (a workspace is just a grouping), so path
    /// lookups are always workspace-scoped.
    pub fn project_in_workspace(
        &self,
        path: &Path,
        workspace: &WorkspaceId,
    ) -> Result<Option<ProjectId>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id FROM projects WHERE repo_path = ?1 AND COALESCE(workspace_id, ?3) = ?2",
        )?;
        let mut rows = stmt.query(params![
            path.to_string_lossy(),
            workspace.as_str(),
            DEFAULT_WORKSPACE_ID
        ])?;
        Ok(rows
            .next()?
            .map(|r| r.get::<_, String>(0))
            .transpose()?
            .map(ProjectId))
    }

    // ---- worktrees ----

    pub fn insert_worktree(&self, w: &Worktree) -> Result<()> {
        self.conn.lock().unwrap().execute(
            "INSERT INTO worktrees (id, project_id, path, branch, is_main, pinned, sort_order, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                w.id.as_str(),
                w.project_id.as_str(),
                w.path.to_string_lossy(),
                w.branch,
                w.is_main as i64,
                w.pinned as i64,
                w.sort_order,
                now_ms()
            ],
        )?;
        Ok(())
    }

    pub fn delete_worktree(&self, id: &WorktreeId) -> Result<()> {
        self.delete_by_id("worktrees", id.as_str())
    }

    pub fn update_worktree_branch(&self, id: &WorktreeId, branch: &str) -> Result<()> {
        self.conn.lock().unwrap().execute(
            "UPDATE worktrees SET branch = ?2 WHERE id = ?1",
            params![id.as_str(), branch],
        )?;
        Ok(())
    }

    /// Root-ness is derived from git's own checkout list on every reconcile
    /// rather than frozen at insert time, so it needs to be writable.
    pub fn set_worktree_main(&self, id: &WorktreeId, is_main: bool) -> Result<()> {
        self.conn.lock().unwrap().execute(
            "UPDATE worktrees SET is_main = ?2 WHERE id = ?1",
            params![id.as_str(), is_main as i64],
        )?;
        Ok(())
    }

    pub fn set_worktree_pinned(&self, id: &WorktreeId, pinned: bool) -> Result<()> {
        self.conn.lock().unwrap().execute(
            "UPDATE worktrees SET pinned = ?2 WHERE id = ?1",
            params![id.as_str(), pinned as i64],
        )?;
        Ok(())
    }

    // ---- agents ----

    pub fn insert_agent(&self, a: &Agent) -> Result<()> {
        self.insert_agent_with_auto_title(a, false)
    }

    /// `auto_title` marks the row as awaiting one agent-driven title
    /// (`nebula rename` from inside the CLI). The flag is store-internal:
    /// clients never see it, they only observe the eventual rename.
    pub fn insert_agent_with_auto_title(&self, a: &Agent, auto_title: bool) -> Result<()> {
        self.conn.lock().unwrap().execute(
            "INSERT INTO agents (id, worktree_id, name, status, archived, archived_at, pinned, kind, claude_session_id, sort_order, created_at, status_changed_at, model, effort, auto_title_pending, unseen, cloud_session_id)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17)",
            params![
                a.id.as_str(),
                a.worktree_id.as_str(),
                a.name,
                a.status.as_str(),
                a.archived as i64,
                a.archived_at,
                a.pinned as i64,
                a.kind.as_str(),
                a.session_id,
                a.sort_order,
                now_ms(),
                a.status_changed_at,
                a.model,
                a.effort,
                auto_title as i64,
                a.unseen as i64,
                a.cloud_session_id
            ],
        )?;
        Ok(())
    }

    /// User rename: always applies, and retires any pending auto-title so a
    /// late agent attempt can't clobber the user's choice.
    pub fn rename_agent(&self, id: &AgentId, name: &str) -> Result<()> {
        self.conn.lock().unwrap().execute(
            "UPDATE agents SET name = ?2, auto_title_pending = 0 WHERE id = ?1",
            params![id.as_str(), name],
        )?;
        Ok(())
    }

    /// Agent rename: applies only while the auto-title is still pending
    /// (single atomic conditional update — concurrent attempts can't both
    /// win). Returns whether the rename was applied.
    pub fn rename_agent_if_auto_pending(&self, id: &AgentId, name: &str) -> Result<bool> {
        let changed = self.conn.lock().unwrap().execute(
            "UPDATE agents SET name = ?2, auto_title_pending = 0 WHERE id = ?1 AND auto_title_pending = 1",
            params![id.as_str(), name],
        )?;
        Ok(changed == 1)
    }

    /// Whether the session still awaits its agent-driven auto-title (drives
    /// the hook server's decision to inject the titling instruction).
    pub fn agent_auto_title_pending(&self, id: &AgentId) -> Result<bool> {
        let pending: Option<i64> = self
            .conn
            .lock()
            .unwrap()
            .query_row(
                "SELECT auto_title_pending FROM agents WHERE id = ?1",
                params![id.as_str()],
                |r| r.get(0),
            )
            .map(Some)
            .or_else(|e| match e {
                rusqlite::Error::QueryReturnedNoRows => Ok(None),
                e => Err(e),
            })?;
        Ok(pending == Some(1))
    }

    pub fn set_agent_worktree(&self, id: &AgentId, worktree_id: &WorktreeId) -> Result<()> {
        self.conn.lock().unwrap().execute(
            "UPDATE agents SET worktree_id = ?2 WHERE id = ?1",
            params![id.as_str(), worktree_id.as_str()],
        )?;
        Ok(())
    }

    pub fn set_agent_archived(&self, id: &AgentId, archived: bool) -> Result<()> {
        // Stamp the archive time (cleared on unarchive) so the TUI can
        // order the ARCHIVED group newest-first.
        let archived_at = if archived { now_ms() } else { 0 };
        // An archived row is out of sight by definition: nothing left to
        // go and read, so its unseen-finish flag goes with it.
        self.conn.lock().unwrap().execute(
            "UPDATE agents SET archived = ?2, archived_at = ?3,
                    unseen = CASE WHEN ?2 THEN 0 ELSE unseen END
             WHERE id = ?1",
            params![id.as_str(), archived as i64, archived_at],
        )?;
        Ok(())
    }

    pub fn set_agent_pinned(&self, id: &AgentId, pinned: bool) -> Result<()> {
        self.conn.lock().unwrap().execute(
            "UPDATE agents SET pinned = ?2 WHERE id = ?1",
            params![id.as_str(), pinned as i64],
        )?;
        Ok(())
    }

    /// Returns the epoch-ms stamp written to `status_changed_at` and the
    /// row's `unseen` flag after the change, so the caller can broadcast
    /// exactly what it persisted.
    ///
    /// The flag is maintained here, atomically with the status it
    /// qualifies: a live turn (running or needs-feedback) landing on
    /// `finished` raises it — that is the yellow-to-green flip nobody may
    /// have been watching — staying on `finished` keeps it, and leaving
    /// `finished` (a new prompt, a restart, a disconnect) drops it, since
    /// there is no finished turn left to read. Archived rows never raise
    /// it: they are out of sight already.
    pub fn set_agent_status(&self, id: &AgentId, status: AgentStatus) -> Result<(i64, bool)> {
        let stamp = now_ms();
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE agents SET status = ?2, status_changed_at = ?3,
                    unseen = CASE
                      WHEN ?2 = 'finished' THEN
                        CASE WHEN status IN ('running', 'needs_feedback') AND archived = 0
                             THEN 1 ELSE unseen END
                      ELSE 0
                    END
             WHERE id = ?1",
            params![id.as_str(), status.as_str(), stamp],
        )?;
        let unseen: i64 = conn
            .query_row(
                "SELECT unseen FROM agents WHERE id = ?1",
                params![id.as_str()],
                |r| r.get(0),
            )
            .unwrap_or(0);
        Ok((stamp, unseen != 0))
    }

    /// The agent's session is on screen: drop its unseen-finish flag.
    /// Returns whether the flag was actually set, so the caller can skip
    /// broadcasting a row that didn't change.
    pub fn mark_agent_seen(&self, id: &AgentId) -> Result<bool> {
        let changed = self.conn.lock().unwrap().execute(
            "UPDATE agents SET unseen = 0 WHERE id = ?1 AND unseen = 1",
            params![id.as_str()],
        )?;
        Ok(changed > 0)
    }

    pub fn set_agent_cloud_session_id(
        &self,
        id: &AgentId,
        cloud_session_id: Option<&str>,
    ) -> Result<()> {
        self.conn.lock().unwrap().execute(
            "UPDATE agents SET cloud_session_id = ?2 WHERE id = ?1",
            params![id.as_str(), cloud_session_id],
        )?;
        Ok(())
    }

    pub fn set_agent_session_id(&self, id: &AgentId, session_id: Option<&str>) -> Result<()> {
        self.conn.lock().unwrap().execute(
            "UPDATE agents SET claude_session_id = ?2 WHERE id = ?1",
            params![id.as_str(), session_id],
        )?;
        Ok(())
    }

    pub fn delete_agent(&self, id: &AgentId) -> Result<()> {
        self.delete_by_id("agents", id.as_str())
    }

    /// Boot sweep: agents whose PTYs died with the previous daemon.
    pub fn sweep_disconnected(&self) -> Result<Vec<AgentId>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt =
            conn.prepare("SELECT id FROM agents WHERE status IN ('running', 'needs_feedback')")?;
        let ids: Vec<AgentId> = stmt
            .query_map([], |r| r.get::<_, String>(0))?
            .filter_map(|r| r.ok())
            .map(AgentId)
            .collect();
        drop(stmt);
        conn.execute(
            "UPDATE agents SET status = 'disconnected', status_changed_at = ?1 WHERE status IN ('running', 'needs_feedback')",
            params![now_ms()],
        )?;
        Ok(ids)
    }

    // ---- terminals ----

    pub fn insert_terminal(&self, t: &TerminalTab) -> Result<()> {
        self.conn.lock().unwrap().execute(
            "INSERT INTO terminals (id, worktree_id, name, sort_order, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![t.id.as_str(), t.worktree_id.as_str(), t.name, t.sort_order, now_ms()],
        )?;
        Ok(())
    }

    pub fn rename_terminal(&self, id: &TerminalId, name: &str) -> Result<()> {
        self.conn.lock().unwrap().execute(
            "UPDATE terminals SET name = ?2 WHERE id = ?1",
            params![id.as_str(), name],
        )?;
        Ok(())
    }

    pub fn delete_terminal(&self, id: &TerminalId) -> Result<()> {
        self.delete_by_id("terminals", id.as_str())
    }

    // ---- links ----

    pub fn insert_link(&self, l: &Link) -> Result<()> {
        self.conn.lock().unwrap().execute(
            "INSERT INTO links (id, worktree_id, url, sort_order, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                l.id.as_str(),
                l.worktree_id.as_str(),
                l.url,
                l.sort_order,
                now_ms()
            ],
        )?;
        Ok(())
    }

    /// Sort slot for a new link: after everything else on its worktree.
    pub fn next_link_sort_order(&self, worktree_id: &WorktreeId) -> Result<i64> {
        Ok(self.conn.lock().unwrap().query_row(
            "SELECT COALESCE(MAX(sort_order) + 1, 0) FROM links WHERE worktree_id = ?1",
            params![worktree_id.as_str()],
            |r| r.get(0),
        )?)
    }

    pub fn set_link_url(&self, id: &LinkId, url: &str) -> Result<()> {
        self.conn.lock().unwrap().execute(
            "UPDATE links SET url = ?2 WHERE id = ?1",
            params![id.as_str(), url],
        )?;
        Ok(())
    }

    pub fn delete_link(&self, id: &LinkId) -> Result<()> {
        self.delete_by_id("links", id.as_str())
    }

    pub fn get_link(&self, id: &LinkId) -> Result<Option<Link>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(&format!("SELECT {LINK_COLUMNS} FROM links WHERE id = ?1"))?;
        let mut rows = stmt.query(params![id.as_str()])?;
        Ok(rows.next()?.map(row_to_link).transpose()?)
    }

    /// Every link, in per-worktree list order.
    pub fn load_links(&self) -> Result<Vec<Link>> {
        let conn = self.conn.lock().unwrap();
        let links = conn
            .prepare(&format!(
                "SELECT {LINK_COLUMNS} FROM links ORDER BY worktree_id, sort_order, created_at"
            ))?
            .query_map([], row_to_link)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(links)
    }

    // ---- pull-request read marks ----

    /// Remember that this pull request's conversation has been read up to
    /// `marker`. Idempotent, and an empty marker is a real answer: it says
    /// the PR was opened while nobody had posted on it yet.
    pub fn mark_pr_seen(&self, url: &str, marker: &str) -> Result<()> {
        self.conn.lock().unwrap().execute(
            "INSERT INTO pr_seen (url, marker, seen_at) VALUES (?1, ?2, ?3)
             ON CONFLICT(url) DO UPDATE SET marker = excluded.marker, seen_at = excluded.seen_at",
            params![url, marker, now_ms()],
        )?;
        Ok(())
    }

    pub fn load_pr_seen(&self) -> Result<Vec<PrSeen>> {
        let conn = self.conn.lock().unwrap();
        let seen = conn
            .prepare("SELECT url, marker FROM pr_seen")?
            .query_map([], |r| {
                Ok(PrSeen {
                    url: r.get(0)?,
                    marker: r.get(1)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(seen)
    }

    // ---- point lookups ----

    pub fn get_project(&self, id: &ProjectId) -> Result<Option<Project>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(&format!(
            "SELECT {PROJECT_COLUMNS} FROM projects WHERE id = ?2"
        ))?;
        let mut rows = stmt.query(params![DEFAULT_WORKSPACE_ID, id.as_str()])?;
        Ok(rows.next()?.map(row_to_project).transpose()?)
    }

    pub fn get_worktree(&self, id: &WorktreeId) -> Result<Option<Worktree>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(&format!(
            "SELECT {WORKTREE_COLUMNS} FROM worktrees WHERE id = ?1"
        ))?;
        let mut rows = stmt.query(params![id.as_str()])?;
        Ok(rows.next()?.map(row_to_worktree).transpose()?)
    }

    pub fn get_agent(&self, id: &AgentId) -> Result<Option<Agent>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt =
            conn.prepare(&format!("SELECT {AGENT_COLUMNS} FROM agents WHERE id = ?1"))?;
        let mut rows = stmt.query(params![id.as_str()])?;
        Ok(rows.next()?.map(row_to_agent).transpose()?)
    }

    pub fn get_terminal(&self, id: &TerminalId) -> Result<Option<TerminalTab>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(&format!(
            "SELECT {TERMINAL_COLUMNS} FROM terminals WHERE id = ?1"
        ))?;
        let mut rows = stmt.query(params![id.as_str()])?;
        Ok(rows.next()?.map(row_to_terminal).transpose()?)
    }

    pub fn count_terminals(&self, worktree_id: &WorktreeId) -> Result<i64> {
        Ok(self.conn.lock().unwrap().query_row(
            "SELECT COUNT(*) FROM terminals WHERE worktree_id = ?1",
            params![worktree_id.as_str()],
            |r| r.get(0),
        )?)
    }

    // ---- whole tree ----

    pub fn load_tree(&self) -> Result<TreeRows> {
        let conn = self.conn.lock().unwrap();

        let projects = conn
            .prepare(&format!(
                "SELECT {PROJECT_COLUMNS} FROM projects ORDER BY sort_order, created_at"
            ))?
            .query_map(params![DEFAULT_WORKSPACE_ID], row_to_project)?
            .collect::<rusqlite::Result<Vec<_>>>()?;

        let worktrees = conn
            .prepare(&format!(
                "SELECT {WORKTREE_COLUMNS} FROM worktrees ORDER BY is_main DESC, sort_order, created_at"
            ))?
            .query_map([], row_to_worktree)?
            .collect::<rusqlite::Result<Vec<_>>>()?;

        let agents = conn
            .prepare(&format!(
                "SELECT {AGENT_COLUMNS} FROM agents ORDER BY sort_order, created_at"
            ))?
            .query_map([], row_to_agent)?
            .collect::<rusqlite::Result<Vec<_>>>()?;

        let terminals = conn
            .prepare(&format!(
                "SELECT {TERMINAL_COLUMNS} FROM terminals ORDER BY sort_order, created_at"
            ))?
            .query_map([], row_to_terminal)?
            .collect::<rusqlite::Result<Vec<_>>>()?;

        Ok((projects, worktrees, agents, terminals))
    }

    // ---- ui state ----

    pub fn save_ui_state(&self, json: &str) -> Result<()> {
        self.conn.lock().unwrap().execute(
            "INSERT INTO ui_state (id, json) VALUES (1, ?1)
             ON CONFLICT(id) DO UPDATE SET json = excluded.json",
            params![json],
        )?;
        Ok(())
    }

    pub fn load_ui_state(&self) -> Result<Option<String>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare("SELECT json FROM ui_state WHERE id = 1")?;
        let mut rows = stmt.query([])?;
        Ok(rows.next()?.map(|r| r.get::<_, String>(0)).transpose()?)
    }
}

// ---- row shapes ----
//
// One column list and one row mapper per entity, shared by the point
// lookups and `load_tree`, so a row can never read differently depending
// on which path fetched it. The column order is the mapper's contract.

const WORKSPACE_COLUMNS: &str = "id, name";
/// Binds the default workspace id as `?1` (rows from before workspaces
/// existed have a NULL `workspace_id`), so a query over these columns
/// numbers its own parameters from `?2`.
const PROJECT_COLUMNS: &str = "id, name, repo_path, sort_order, COALESCE(workspace_id, ?1)";
const WORKTREE_COLUMNS: &str = "id, project_id, path, branch, is_main, pinned, sort_order";
const AGENT_COLUMNS: &str = "id, worktree_id, name, status, archived, pinned, kind, \
                             claude_session_id, sort_order, status_changed_at, model, effort, \
                             archived_at, unseen, cloud_session_id";
const TERMINAL_COLUMNS: &str = "id, worktree_id, name, sort_order";
const LINK_COLUMNS: &str = "id, worktree_id, url, sort_order";

fn row_to_workspace(r: &rusqlite::Row) -> rusqlite::Result<Workspace> {
    Ok(Workspace {
        id: WorkspaceId(r.get(0)?),
        name: r.get(1)?,
    })
}

fn row_to_project(r: &rusqlite::Row) -> rusqlite::Result<Project> {
    Ok(Project {
        id: ProjectId(r.get(0)?),
        name: r.get(1)?,
        repo_path: PathBuf::from(r.get::<_, String>(2)?),
        sort_order: r.get(3)?,
        workspace_id: WorkspaceId(r.get(4)?),
    })
}

fn row_to_worktree(r: &rusqlite::Row) -> rusqlite::Result<Worktree> {
    Ok(Worktree {
        id: WorktreeId(r.get(0)?),
        project_id: ProjectId(r.get(1)?),
        path: PathBuf::from(r.get::<_, String>(2)?),
        branch: r.get(3)?,
        is_main: r.get::<_, i64>(4)? != 0,
        pinned: r.get::<_, i64>(5)? != 0,
        sort_order: r.get(6)?,
    })
}

/// `alive` and `cloud_mirroring` are daemon state, not columns: the
/// registry fills them in from its session table after the read.
fn row_to_agent(r: &rusqlite::Row) -> rusqlite::Result<Agent> {
    Ok(Agent {
        id: AgentId(r.get(0)?),
        worktree_id: WorktreeId(r.get(1)?),
        name: r.get(2)?,
        status: AgentStatus::parse(&r.get::<_, String>(3)?).unwrap_or(AgentStatus::Fresh),
        archived: r.get::<_, i64>(4)? != 0,
        pinned: r.get::<_, i64>(5)? != 0,
        kind: AgentKind::parse(&r.get::<_, String>(6)?).unwrap_or_default(),
        session_id: r.get(7)?,
        sort_order: r.get(8)?,
        status_changed_at: r.get(9)?,
        model: r.get(10)?,
        effort: r.get(11)?,
        archived_at: r.get(12)?,
        unseen: r.get::<_, i64>(13)? != 0,
        cloud_session_id: r.get(14)?,
        alive: false,
        cloud_mirroring: false,
    })
}

/// `alive` is daemon state, filled in by the registry like the agent's.
fn row_to_terminal(r: &rusqlite::Row) -> rusqlite::Result<TerminalTab> {
    Ok(TerminalTab {
        id: TerminalId(r.get(0)?),
        worktree_id: WorktreeId(r.get(1)?),
        name: r.get(2)?,
        sort_order: r.get(3)?,
        alive: false,
    })
}

fn row_to_link(r: &rusqlite::Row) -> rusqlite::Result<Link> {
    Ok(Link {
        id: LinkId(r.get(0)?),
        worktree_id: WorktreeId(r.get(1)?),
        url: r.get(2)?,
        sort_order: r.get(3)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_tree() {
        let store = Store::open_in_memory().unwrap();
        let project = Project {
            workspace_id: Default::default(),
            id: ProjectId::generate(),
            name: "demo".into(),
            repo_path: "/tmp/demo".into(),
            sort_order: 0,
        };
        store.insert_project(&project).unwrap();
        let worktree = Worktree {
            id: WorktreeId::generate(),
            project_id: project.id.clone(),
            path: "/tmp/demo".into(),
            branch: "main".into(),
            is_main: true,
            pinned: false,
            sort_order: 0,
        };
        store.insert_worktree(&worktree).unwrap();
        let agent = Agent {
            id: AgentId::generate(),
            worktree_id: worktree.id.clone(),
            name: "agent-1".into(),
            status: AgentStatus::Running,
            archived: false,
            archived_at: 0,
            pinned: false,
            unseen: false,
            kind: AgentKind::Claude,
            model: Some("opus".into()),
            effort: Some("high".into()),
            session_id: Some("sess-123".into()),
            cloud_session_id: None,
            sort_order: 0,
            status_changed_at: 0,
            alive: false,
            cloud_mirroring: false,
        };
        store.insert_agent(&agent).unwrap();
        let codex_agent = Agent {
            id: AgentId::generate(),
            worktree_id: worktree.id.clone(),
            name: "agent-2".into(),
            status: AgentStatus::Fresh,
            archived: false,
            archived_at: 0,
            pinned: false,
            unseen: false,
            kind: AgentKind::Codex,
            model: None,
            effort: None,
            session_id: None,
            cloud_session_id: None,
            sort_order: 1,
            status_changed_at: 0,
            alive: false,
            cloud_mirroring: false,
        };
        store.insert_agent(&codex_agent).unwrap();
        let cursor_agent = Agent {
            id: AgentId::generate(),
            worktree_id: worktree.id.clone(),
            name: "agent-3".into(),
            status: AgentStatus::Fresh,
            archived: false,
            archived_at: 0,
            pinned: false,
            unseen: false,
            kind: AgentKind::Cursor,
            model: None,
            effort: None,
            session_id: None,
            cloud_session_id: None,
            sort_order: 2,
            status_changed_at: 0,
            alive: false,
            cloud_mirroring: false,
        };
        store.insert_agent(&cursor_agent).unwrap();

        let (projects, worktrees, agents, _terms) = store.load_tree().unwrap();
        assert_eq!(projects.len(), 1);
        assert_eq!(worktrees.len(), 1);
        assert_eq!(agents.len(), 3);
        assert_eq!(agents[0].status, AgentStatus::Running);
        assert_eq!(agents[0].kind, AgentKind::Claude);
        assert_eq!(agents[0].session_id.as_deref(), Some("sess-123"));
        assert_eq!(agents[0].model.as_deref(), Some("opus"));
        assert_eq!(agents[0].effort.as_deref(), Some("high"));
        assert_eq!(agents[1].kind, AgentKind::Codex);
        assert_eq!(agents[1].model, None);
        assert_eq!(agents[2].kind, AgentKind::Cursor);
    }

    /// Read marks are keyed by PR URL and outlive the worktree they were
    /// noticed on, so they live in their own table with no foreign key: no
    /// row here is ever cascaded away by a checkout being deleted.
    #[test]
    fn pr_seen_marks_roundtrip_and_overwrite() {
        let store = Store::open_in_memory().unwrap();
        assert!(store.load_pr_seen().unwrap().is_empty());

        let url = "https://github.com/o/r/pull/7";
        store.mark_pr_seen(url, "2024-04-25T19:55:42Z").unwrap();
        let seen = store.load_pr_seen().unwrap();
        assert_eq!(seen.len(), 1);
        assert_eq!(seen[0].url, url);
        assert_eq!(seen[0].marker, "2024-04-25T19:55:42Z");

        // Opening it again moves the mark rather than adding a second row.
        store.mark_pr_seen(url, "2024-04-27T09:00:00Z").unwrap();
        let seen = store.load_pr_seen().unwrap();
        assert_eq!(seen.len(), 1);
        assert_eq!(seen[0].marker, "2024-04-27T09:00:00Z");

        // An empty marker is a real answer: opened, nobody had posted yet.
        store.mark_pr_seen(url, "").unwrap();
        assert_eq!(store.load_pr_seen().unwrap()[0].marker, "");
    }

    #[test]
    fn link_crud_roundtrip_and_cascade() {
        let store = Store::open_in_memory().unwrap();
        let project = Project {
            workspace_id: Default::default(),
            id: ProjectId::generate(),
            name: "demo".into(),
            repo_path: "/tmp/demo".into(),
            sort_order: 0,
        };
        store.insert_project(&project).unwrap();
        let worktree = Worktree {
            id: WorktreeId::generate(),
            project_id: project.id.clone(),
            path: "/tmp/demo".into(),
            branch: "main".into(),
            is_main: true,
            pinned: false,
            sort_order: 0,
        };
        store.insert_worktree(&worktree).unwrap();

        assert_eq!(store.next_link_sort_order(&worktree.id).unwrap(), 0);
        let link = Link {
            id: LinkId::generate(),
            worktree_id: worktree.id.clone(),
            url: "https://github.com/o/r/pull/7".into(),
            sort_order: store.next_link_sort_order(&worktree.id).unwrap(),
        };
        store.insert_link(&link).unwrap();
        assert_eq!(store.next_link_sort_order(&worktree.id).unwrap(), 1);

        store
            .set_link_url(&link.id, "https://example.dev/spec")
            .unwrap();
        let read = store.get_link(&link.id).unwrap().unwrap();
        assert_eq!(read.url, "https://example.dev/spec");
        assert_eq!(read.worktree_id, worktree.id);
        assert_eq!(store.load_links().unwrap().len(), 1);

        store.delete_link(&link.id).unwrap();
        assert!(store.get_link(&link.id).unwrap().is_none());

        // Links hang off the worktree: deleting the project cascades
        // through it.
        store.insert_link(&link).unwrap();
        store.delete_project(&project.id).unwrap();
        assert!(store.load_links().unwrap().is_empty());
    }

    /// Real upgrade path: a v9 database still carrying `todos` rows walks
    /// the whole chain — 10's rebuild, 15's rename, 21's DROP — and lands
    /// with the table retired rather than erroring partway.
    #[test]
    fn migration_21_retires_notes_from_a_v9_database() {
        let path =
            std::env::temp_dir().join(format!("nebula-mig21-test-{}.db", std::process::id()));
        let _ = std::fs::remove_file(&path);
        {
            let conn = Connection::open(&path).unwrap();
            conn.pragma_update(None, "foreign_keys", "ON").unwrap();
            for (i, migration) in MIGRATIONS.iter().take(9).enumerate() {
                conn.execute_batch(&format!(
                    "BEGIN; {migration}; PRAGMA user_version = {}; COMMIT;",
                    i + 1
                ))
                .unwrap();
            }
            conn.execute_batch(
                "INSERT INTO projects (id, name, repo_path, sort_order, created_at) VALUES ('p1', 'p', '/tmp/p', 0, 0);
                 INSERT INTO worktrees (id, project_id, path, branch, is_main, sort_order, created_at) VALUES ('w1', 'p1', '/tmp/p', 'main', 1, 0, 0);
                 INSERT INTO todos (id, worktree_id, text, done, sort_order, created_at) VALUES ('t1', 'w1', 'old note', 1, 3, 0);",
            )
            .unwrap();
        }

        let store = Store::open(&path).unwrap();
        // The project survived the walk; neither the original table name nor
        // the renamed one is left behind.
        assert_eq!(store.load_tree().unwrap().0.len(), 1);
        let conn = store.conn.lock().unwrap();
        let tables: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name IN ('notes', 'todos')",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(tables, 0);
        drop(conn);
        drop(store);
        for suffix in ["", "-wal", "-shm"] {
            let _ = std::fs::remove_file(format!("{}{}", path.display(), suffix));
        }
    }

    /// Real upgrade path: a v12 database (pre-workspaces) gains the
    /// 'default' workspace, marked open, with every existing project in it.
    #[test]
    fn migration_13_moves_existing_projects_into_default_workspace() {
        let path =
            std::env::temp_dir().join(format!("nebula-mig13-test-{}.db", std::process::id()));
        let _ = std::fs::remove_file(&path);
        {
            let conn = Connection::open(&path).unwrap();
            conn.pragma_update(None, "foreign_keys", "ON").unwrap();
            for (i, migration) in MIGRATIONS.iter().take(12).enumerate() {
                conn.execute_batch(&format!(
                    "BEGIN; {migration}; PRAGMA user_version = {}; COMMIT;",
                    i + 1
                ))
                .unwrap();
            }
            conn.execute_batch(
                "INSERT INTO projects (id, name, repo_path, sort_order, created_at) VALUES ('p1', 'p', '/tmp/p', 0, 0);",
            )
            .unwrap();
        }

        let store = Store::open(&path).unwrap();
        let workspaces = store.load_workspaces().unwrap();
        assert_eq!(workspaces.len(), 1);
        assert_eq!(workspaces[0].id.as_str(), DEFAULT_WORKSPACE_ID);
        assert_eq!(workspaces[0].name, "default");
        assert_eq!(
            store.active_workspace_id().unwrap().as_str(),
            DEFAULT_WORKSPACE_ID
        );
        let (projects, _, _, _) = store.load_tree().unwrap();
        assert_eq!(projects[0].workspace_id.as_str(), DEFAULT_WORKSPACE_ID);
        drop(store);
        for suffix in ["", "-wal", "-shm"] {
            let _ = std::fs::remove_file(format!("{}{}", path.display(), suffix));
        }
    }

    /// Real upgrade path: a v17 database still carries the project divider
    /// columns (with data in them). Migration 18 drops them and the
    /// projects underneath load untouched.
    #[test]
    fn migration_18_drops_the_divider_columns() {
        let path =
            std::env::temp_dir().join(format!("nebula-mig18-test-{}.db", std::process::id()));
        let _ = std::fs::remove_file(&path);
        {
            let conn = Connection::open(&path).unwrap();
            for (i, migration) in MIGRATIONS.iter().take(17).enumerate() {
                conn.execute_batch(&format!(
                    "BEGIN; {migration}; PRAGMA user_version = {}; COMMIT;",
                    i + 1
                ))
                .unwrap();
            }
            conn.execute_batch(
                "INSERT INTO projects (id, name, repo_path, sort_order, created_at, divider_after, divider_label, divider_before, divider_before_label, workspace_id)
                   VALUES ('p1', 'one', '/tmp/one', 0, 0, 1, 'work', 1, 'top', 'default');
                 INSERT INTO projects (id, name, repo_path, sort_order, created_at, workspace_id)
                   VALUES ('p2', 'two', '/tmp/two', 1, 0, 'default');",
            )
            .unwrap();
        }

        let store = Store::open(&path).unwrap();
        let (projects, _, _, _) = store.load_tree().unwrap();
        assert_eq!(
            projects.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(),
            ["one", "two"]
        );
        assert_eq!(projects[0].sort_order, 0);
        assert_eq!(projects[1].workspace_id.as_str(), DEFAULT_WORKSPACE_ID);
        let columns: Vec<String> = store
            .conn
            .lock()
            .unwrap()
            .prepare("PRAGMA table_info(projects)")
            .unwrap()
            .query_map([], |r| r.get::<_, String>(1))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        assert!(
            !columns.iter().any(|c| c.starts_with("divider")),
            "divider columns survived the migration: {columns:?}"
        );
        drop(store);
        for suffix in ["", "-wal", "-shm"] {
            let _ = std::fs::remove_file(format!("{}{}", path.display(), suffix));
        }
    }

    /// Real upgrade path: a v13 database (global UNIQUE on repo_path) is
    /// rebuilt so the same repo can live in several workspaces. The rebuild
    /// drops the old projects table — child rows must survive it.
    #[test]
    fn migration_14_scopes_repo_uniqueness_to_workspace() {
        let path =
            std::env::temp_dir().join(format!("nebula-mig14-test-{}.db", std::process::id()));
        let _ = std::fs::remove_file(&path);
        {
            let conn = Connection::open(&path).unwrap();
            conn.pragma_update(None, "foreign_keys", "ON").unwrap();
            for (i, migration) in MIGRATIONS.iter().take(13).enumerate() {
                conn.execute_batch(&format!(
                    "BEGIN; {migration}; PRAGMA user_version = {}; COMMIT;",
                    i + 1
                ))
                .unwrap();
            }
            conn.execute_batch(
                "INSERT INTO projects (id, name, repo_path, sort_order, created_at, workspace_id) VALUES ('p1', 'p', '/tmp/p', 0, 0, 'default');
                 INSERT INTO worktrees (id, project_id, path, branch, is_main, sort_order, created_at) VALUES ('w1', 'p1', '/tmp/p', 'main', 1, 0, 0);
                 INSERT INTO agents (id, worktree_id, name, created_at) VALUES ('a1', 'w1', 'agent', 0);",
            )
            .unwrap();
        }

        let store = Store::open(&path).unwrap();
        let (projects, worktrees, agents, _) = store.load_tree().unwrap();
        assert_eq!(projects.len(), 1);
        assert_eq!(worktrees.len(), 1, "worktrees must survive the rebuild");
        assert_eq!(agents.len(), 1, "agents must survive the rebuild");

        // The same repo is now welcome in a second workspace…
        store
            .insert_workspace(&Workspace {
                id: WorkspaceId("w2".into()),
                name: "second".into(),
            })
            .unwrap();
        let dup = |id: &str, workspace: &str| Project {
            id: ProjectId(id.into()),
            name: "p".into(),
            workspace_id: WorkspaceId(workspace.into()),
            repo_path: PathBuf::from("/tmp/p"),
            sort_order: 1,
        };
        store.insert_project(&dup("p2", "w2")).unwrap();
        // …but still refused twice in the same one.
        assert!(store.insert_project(&dup("p3", "default")).is_err());

        // Path lookups resolve per workspace.
        assert_eq!(
            store
                .project_in_workspace(Path::new("/tmp/p"), &WorkspaceId("w2".into()))
                .unwrap(),
            Some(ProjectId("p2".into()))
        );
        assert_eq!(
            store
                .project_in_workspace(Path::new("/tmp/p"), &WorkspaceId("empty".into()))
                .unwrap(),
            None
        );
        drop(store);
        for suffix in ["", "-wal", "-shm"] {
            let _ = std::fs::remove_file(format!("{}{}", path.display(), suffix));
        }
    }

    #[test]
    fn workspace_crud_and_active_flag() {
        let store = Store::open_in_memory().unwrap();
        // The migration seeds the open 'default' workspace.
        let workspaces = store.load_workspaces().unwrap();
        assert_eq!(workspaces.len(), 1);
        assert_eq!(workspaces[0].name, "default");
        assert_eq!(
            store.active_workspace_id().unwrap().as_str(),
            DEFAULT_WORKSPACE_ID
        );

        let client = Workspace {
            id: WorkspaceId("ws-client".into()),
            name: "client".into(),
        };
        store.insert_workspace(&client).unwrap();
        assert_eq!(store.count_workspaces().unwrap(), 2);
        assert_eq!(
            store.workspace_by_name("client").unwrap(),
            Some(client.id.clone())
        );
        // UNIQUE name: a duplicate insert errors.
        assert!(store
            .insert_workspace(&Workspace {
                id: WorkspaceId("ws-dup".into()),
                name: "client".into(),
            })
            .is_err());

        // Exactly one open workspace at a time.
        store.set_active_workspace(&client.id).unwrap();
        assert_eq!(store.active_workspace_id().unwrap(), client.id);
        store
            .set_active_workspace(&WorkspaceId(DEFAULT_WORKSPACE_ID.into()))
            .unwrap();
        assert_eq!(
            store.active_workspace_id().unwrap().as_str(),
            DEFAULT_WORKSPACE_ID
        );

        store.rename_workspace(&client.id, "acme").unwrap();
        assert_eq!(
            store.get_workspace(&client.id).unwrap().unwrap().name,
            "acme"
        );
        assert_eq!(store.workspace_by_name("client").unwrap(), None);

        // Projects count per workspace; inserts land where they say.
        let project = Project {
            workspace_id: client.id.clone(),
            id: ProjectId::generate(),
            name: "demo".into(),
            repo_path: "/tmp/demo".into(),
            sort_order: 0,
        };
        store.insert_project(&project).unwrap();
        assert_eq!(store.count_workspace_projects(&client.id).unwrap(), 1);
        assert_eq!(
            store
                .count_workspace_projects(&WorkspaceId(DEFAULT_WORKSPACE_ID.into()))
                .unwrap(),
            0
        );
        let (projects, _, _, _) = store.load_tree().unwrap();
        assert_eq!(projects[0].workspace_id, client.id);

        // The FK keeps a populated workspace undeletable; empty it first.
        assert!(store.delete_workspace(&client.id).is_err());
        store.delete_project(&project.id).unwrap();
        store.delete_workspace(&client.id).unwrap();
        assert_eq!(store.count_workspaces().unwrap(), 1);
    }

    #[test]
    fn auto_title_pending_lifecycle() {
        let store = Store::open_in_memory().unwrap();
        let project = Project {
            workspace_id: Default::default(),
            id: ProjectId::generate(),
            name: "p".into(),
            repo_path: "/tmp/p".into(),
            sort_order: 0,
        };
        store.insert_project(&project).unwrap();
        let wt = Worktree {
            id: WorktreeId::generate(),
            project_id: project.id.clone(),
            path: "/tmp/p".into(),
            branch: "main".into(),
            is_main: true,
            pinned: false,
            sort_order: 0,
        };
        store.insert_worktree(&wt).unwrap();
        let agent = |id: &str| Agent {
            id: AgentId(id.into()),
            worktree_id: wt.id.clone(),
            name: "agent-1".into(),
            status: AgentStatus::Fresh,
            archived: false,
            archived_at: 0,
            pinned: false,
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
        };

        // Default-named session: pending until the agent titles it, and the
        // conditional rename fires exactly once.
        store
            .insert_agent_with_auto_title(&agent("a1"), true)
            .unwrap();
        let id = AgentId("a1".into());
        assert!(store.agent_auto_title_pending(&id).unwrap());
        assert!(store
            .rename_agent_if_auto_pending(&id, "Fix Login Redirect")
            .unwrap());
        assert!(!store.agent_auto_title_pending(&id).unwrap());
        assert!(!store
            .rename_agent_if_auto_pending(&id, "Second Attempt")
            .unwrap());
        assert_eq!(
            store.get_agent(&id).unwrap().unwrap().name,
            "Fix Login Redirect"
        );

        // A user rename retires the pending flag so a late agent attempt
        // can't clobber the user's choice.
        store
            .insert_agent_with_auto_title(&agent("a2"), true)
            .unwrap();
        let id = AgentId("a2".into());
        store.rename_agent(&id, "my session").unwrap();
        assert!(!store.agent_auto_title_pending(&id).unwrap());
        assert!(!store.rename_agent_if_auto_pending(&id, "Nope").unwrap());
        assert_eq!(store.get_agent(&id).unwrap().unwrap().name, "my session");

        // Custom-named sessions (plain insert) never pend; unknown ids
        // report not-pending instead of erroring.
        store.insert_agent(&agent("a3")).unwrap();
        assert!(!store
            .agent_auto_title_pending(&AgentId("a3".into()))
            .unwrap());
        assert!(!store
            .agent_auto_title_pending(&AgentId("ghost".into()))
            .unwrap());
    }

    #[test]
    fn cascade_delete_project_removes_children() {
        let store = Store::open_in_memory().unwrap();
        let project = Project {
            workspace_id: Default::default(),
            id: ProjectId::generate(),
            name: "demo".into(),
            repo_path: "/tmp/demo".into(),
            sort_order: 0,
        };
        store.insert_project(&project).unwrap();
        let worktree = Worktree {
            id: WorktreeId::generate(),
            project_id: project.id.clone(),
            path: "/tmp/demo".into(),
            branch: "main".into(),
            is_main: true,
            pinned: false,
            sort_order: 0,
        };
        store.insert_worktree(&worktree).unwrap();
        store
            .insert_terminal(&TerminalTab {
                id: TerminalId::generate(),
                worktree_id: worktree.id.clone(),
                name: "shell".into(),
                sort_order: 0,
                alive: false,
            })
            .unwrap();

        store.delete_project(&project.id).unwrap();
        let (projects, worktrees, _agents, terminals) = store.load_tree().unwrap();
        assert!(projects.is_empty());
        assert!(worktrees.is_empty());
        assert!(terminals.is_empty());
    }

    #[test]
    fn sweep_disconnected_only_hits_live_statuses() {
        let store = Store::open_in_memory().unwrap();
        let project = Project {
            workspace_id: Default::default(),
            id: ProjectId::generate(),
            name: "p".into(),
            repo_path: "/tmp/p".into(),
            sort_order: 0,
        };
        store.insert_project(&project).unwrap();
        let wt = Worktree {
            id: WorktreeId::generate(),
            project_id: project.id.clone(),
            path: "/tmp/p".into(),
            branch: "main".into(),
            is_main: true,
            pinned: false,
            sort_order: 0,
        };
        store.insert_worktree(&wt).unwrap();
        for (name, status) in [
            ("a", AgentStatus::Running),
            ("b", AgentStatus::Finished),
            ("c", AgentStatus::NeedsFeedback),
        ] {
            store
                .insert_agent(&Agent {
                    id: AgentId(format!("agent-{name}")),
                    worktree_id: wt.id.clone(),
                    name: name.into(),
                    status,
                    archived: false,
                    archived_at: 0,
                    pinned: false,
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
                })
                .unwrap();
        }
        let swept = store.sweep_disconnected().unwrap();
        assert_eq!(swept.len(), 2);
        let (_, _, agents, _) = store.load_tree().unwrap();
        assert_eq!(
            agents
                .iter()
                .filter(|a| a.status == AgentStatus::Disconnected)
                .count(),
            2
        );
        assert_eq!(
            agents
                .iter()
                .filter(|a| a.status == AgentStatus::Finished)
                .count(),
            1
        );
    }

    /// `Agent::unseen` rides along with the status: a live turn landing on
    /// finished raises it, staying there keeps it, leaving drops it. Fresh
    /// and archived rows never raise it, archiving takes it away, and a
    /// daemon restart leaves finished rows — flag included — alone.
    /// `mark_agent_seen` reports whether it had anything to clear.
    #[test]
    fn unseen_follows_the_status_and_clears_on_seen() {
        let store = Store::open_in_memory().unwrap();
        let project = Project {
            workspace_id: Default::default(),
            id: ProjectId::generate(),
            name: "demo".into(),
            repo_path: "/tmp/demo".into(),
            sort_order: 0,
        };
        store.insert_project(&project).unwrap();
        let worktree = Worktree {
            id: WorktreeId::generate(),
            project_id: project.id.clone(),
            path: "/tmp/demo".into(),
            branch: "main".into(),
            is_main: true,
            pinned: false,
            sort_order: 0,
        };
        store.insert_worktree(&worktree).unwrap();
        let seed = |name: &str, status: AgentStatus| {
            let agent = Agent {
                id: AgentId::generate(),
                worktree_id: worktree.id.clone(),
                name: name.into(),
                status,
                archived: false,
                archived_at: 0,
                pinned: false,
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
            };
            store.insert_agent(&agent).unwrap();
            agent.id
        };
        let unseen = |id: &AgentId| store.get_agent(id).unwrap().unwrap().unseen;
        let flip =
            |id: &AgentId, status: AgentStatus| store.set_agent_status(id, status).unwrap().1;

        let a = seed("a", AgentStatus::Running);
        assert!(!unseen(&a));
        assert!(flip(&a, AgentStatus::Finished), "yellow → green raises it");
        assert!(unseen(&a));
        assert!(flip(&a, AgentStatus::Finished), "staying finished keeps it");
        assert!(!flip(&a, AgentStatus::Running), "a new turn drops it");
        assert!(!unseen(&a));
        assert!(!flip(&a, AgentStatus::NeedsFeedback));
        assert!(
            flip(&a, AgentStatus::Finished),
            "red → green is a finish too"
        );
        assert!(
            store.mark_agent_seen(&a).unwrap(),
            "there was something to clear"
        );
        assert!(!unseen(&a));
        assert!(
            !store.mark_agent_seen(&a).unwrap(),
            "already clear: nothing to broadcast"
        );

        // The tree load carries it, same as the single-row read.
        flip(&a, AgentStatus::Running);
        flip(&a, AgentStatus::Finished);
        let (_, _, agents, _) = store.load_tree().unwrap();
        assert!(agents.iter().find(|x| x.id == a).unwrap().unseen);

        // Archiving takes it away, and an archived row never raises it.
        store.set_agent_archived(&a, true).unwrap();
        assert!(!unseen(&a));
        flip(&a, AgentStatus::Running);
        assert!(
            !flip(&a, AgentStatus::Finished),
            "archived rows are out of sight"
        );

        // A Stop nebula never saw the prompt for is not a yellow → green.
        let b = seed("b", AgentStatus::Fresh);
        assert!(!flip(&b, AgentStatus::Finished));

        // A daemon restart disconnects live rows and leaves finished ones alone.
        let c = seed("c", AgentStatus::Running);
        assert!(flip(&c, AgentStatus::Finished));
        store.sweep_disconnected().unwrap();
        assert!(unseen(&c), "still waiting to be read after the restart");
    }
}
