//! SQLite persistence. Write volume is trivial (entity CRUD + status
//! changes), so a mutex-guarded connection is sufficient — no ORM, no
//! connection pool.

use anyhow::{Context, Result};
use nebula_core::{
    Agent, AgentId, AgentKind, AgentStatus, Project, ProjectId, TerminalId, TerminalTab, Todo,
    TodoId, TodoOwner, Worktree, WorktreeId,
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
        for (i, migration) in MIGRATIONS.iter().enumerate().skip(version as usize) {
            conn.execute_batch(&format!(
                "BEGIN; {migration}; PRAGMA user_version = {}; COMMIT;",
                i + 1
            ))
            .with_context(|| format!("migration {}", i + 1))?;
        }
        Ok(())
    }

    // ---- projects ----

    pub fn insert_project(&self, p: &Project) -> Result<()> {
        self.conn.lock().unwrap().execute(
            "INSERT INTO projects (id, name, repo_path, sort_order, divider_after, divider_label, divider_before, divider_before_label, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![p.id.as_str(), p.name, p.repo_path.to_string_lossy(), p.sort_order, p.divider_after as i64, p.divider_label, p.divider_before as i64, p.divider_before_label, now_ms()],
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

    /// Persist a project's list position: sort order plus both dividers.
    pub fn set_project_position(&self, p: &Project) -> Result<()> {
        self.conn.lock().unwrap().execute(
            "UPDATE projects SET sort_order = ?2, divider_after = ?3, divider_label = ?4, divider_before = ?5, divider_before_label = ?6 WHERE id = ?1",
            params![
                p.id.as_str(),
                p.sort_order,
                p.divider_after as i64,
                p.divider_label,
                p.divider_before as i64,
                p.divider_before_label
            ],
        )?;
        Ok(())
    }

    pub fn delete_project(&self, id: &ProjectId) -> Result<()> {
        self.conn
            .lock()
            .unwrap()
            .execute("DELETE FROM projects WHERE id = ?1", params![id.as_str()])?;
        Ok(())
    }

    pub fn project_by_path(&self, path: &Path) -> Result<Option<ProjectId>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare("SELECT id FROM projects WHERE repo_path = ?1")?;
        let mut rows = stmt.query(params![path.to_string_lossy()])?;
        Ok(rows
            .next()?
            .map(|r| ProjectId(r.get::<_, String>(0).unwrap())))
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
        self.conn
            .lock()
            .unwrap()
            .execute("DELETE FROM worktrees WHERE id = ?1", params![id.as_str()])?;
        Ok(())
    }

    pub fn update_worktree_branch(&self, id: &WorktreeId, branch: &str) -> Result<()> {
        self.conn.lock().unwrap().execute(
            "UPDATE worktrees SET branch = ?2 WHERE id = ?1",
            params![id.as_str(), branch],
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
            "INSERT INTO agents (id, worktree_id, name, status, archived, archived_at, pinned, kind, claude_session_id, sort_order, created_at, status_changed_at, model, effort, auto_title_pending)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)",
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
                auto_title as i64
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
        self.conn.lock().unwrap().execute(
            "UPDATE agents SET archived = ?2, archived_at = ?3 WHERE id = ?1",
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

    /// Returns the epoch-ms stamp written to `status_changed_at`, so the
    /// caller can broadcast the exact same timestamp it persisted.
    pub fn set_agent_status(&self, id: &AgentId, status: AgentStatus) -> Result<i64> {
        let stamp = now_ms();
        self.conn.lock().unwrap().execute(
            "UPDATE agents SET status = ?2, status_changed_at = ?3 WHERE id = ?1",
            params![id.as_str(), status.as_str(), stamp],
        )?;
        Ok(stamp)
    }

    pub fn set_agent_session_id(&self, id: &AgentId, session_id: Option<&str>) -> Result<()> {
        self.conn.lock().unwrap().execute(
            "UPDATE agents SET claude_session_id = ?2 WHERE id = ?1",
            params![id.as_str(), session_id],
        )?;
        Ok(())
    }

    pub fn delete_agent(&self, id: &AgentId) -> Result<()> {
        self.conn
            .lock()
            .unwrap()
            .execute("DELETE FROM agents WHERE id = ?1", params![id.as_str()])?;
        Ok(())
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
        self.conn
            .lock()
            .unwrap()
            .execute("DELETE FROM terminals WHERE id = ?1", params![id.as_str()])?;
        Ok(())
    }

    // ---- todos ----

    /// (project_id, worktree_id) column values for an owner — exactly one
    /// is Some, mirroring the table's CHECK.
    fn todo_owner_cols(owner: &TodoOwner) -> (Option<&str>, Option<&str>) {
        match owner {
            TodoOwner::Project(id) => (Some(id.as_str()), None),
            TodoOwner::Worktree(id) => (None, Some(id.as_str())),
        }
    }

    /// Owner from a row's (project_id, worktree_id) pair.
    fn todo_owner_from(project_id: Option<String>, worktree_id: Option<String>) -> TodoOwner {
        match (project_id, worktree_id) {
            (Some(p), _) => TodoOwner::Project(ProjectId(p)),
            (None, Some(w)) => TodoOwner::Worktree(WorktreeId(w)),
            // Unreachable per the CHECK constraint.
            (None, None) => TodoOwner::Worktree(WorktreeId(String::new())),
        }
    }

    pub fn insert_todo(&self, t: &Todo) -> Result<()> {
        let (project_id, worktree_id) = Self::todo_owner_cols(&t.owner);
        self.conn.lock().unwrap().execute(
            "INSERT INTO todos (id, project_id, worktree_id, text, done, sort_order, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                t.id.as_str(),
                project_id,
                worktree_id,
                t.text,
                t.done as i64,
                t.sort_order,
                now_ms()
            ],
        )?;
        Ok(())
    }

    /// Sort slot for a new todo: after everything else in its owner's list.
    pub fn next_todo_sort_order(&self, owner: &TodoOwner) -> Result<i64> {
        let (project_id, worktree_id) = Self::todo_owner_cols(owner);
        Ok(self.conn.lock().unwrap().query_row(
            "SELECT COALESCE(MAX(sort_order) + 1, 0) FROM todos WHERE project_id IS ?1 AND worktree_id IS ?2",
            params![project_id, worktree_id],
            |r| r.get(0),
        )?)
    }

    pub fn set_todo_text(&self, id: &TodoId, text: &str) -> Result<()> {
        self.conn.lock().unwrap().execute(
            "UPDATE todos SET text = ?2 WHERE id = ?1",
            params![id.as_str(), text],
        )?;
        Ok(())
    }

    pub fn set_todo_done(&self, id: &TodoId, done: bool) -> Result<()> {
        self.conn.lock().unwrap().execute(
            "UPDATE todos SET done = ?2 WHERE id = ?1",
            params![id.as_str(), done as i64],
        )?;
        Ok(())
    }

    pub fn delete_todo(&self, id: &TodoId) -> Result<()> {
        self.conn
            .lock()
            .unwrap()
            .execute("DELETE FROM todos WHERE id = ?1", params![id.as_str()])?;
        Ok(())
    }

    pub fn get_todo(&self, id: &TodoId) -> Result<Option<Todo>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, project_id, worktree_id, text, done, sort_order FROM todos WHERE id = ?1",
        )?;
        let mut rows = stmt.query(params![id.as_str()])?;
        Ok(rows.next()?.map(|r| Todo {
            id: TodoId(r.get::<_, String>(0).unwrap()),
            owner: Self::todo_owner_from(r.get(1).unwrap(), r.get(2).unwrap()),
            text: r.get(3).unwrap(),
            done: r.get::<_, i64>(4).unwrap() != 0,
            sort_order: r.get(5).unwrap(),
        }))
    }

    /// Every todo, in per-owner list order.
    pub fn load_todos(&self) -> Result<Vec<Todo>> {
        let conn = self.conn.lock().unwrap();
        let todos = conn
            .prepare("SELECT id, project_id, worktree_id, text, done, sort_order FROM todos ORDER BY COALESCE(project_id, worktree_id), sort_order, created_at")?
            .query_map([], |r| {
                Ok(Todo {
                    id: TodoId(r.get(0)?),
                    owner: Self::todo_owner_from(r.get(1)?, r.get(2)?),
                    text: r.get(3)?,
                    done: r.get::<_, i64>(4)? != 0,
                    sort_order: r.get(5)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(todos)
    }

    // ---- point lookups ----

    pub fn get_project(&self, id: &ProjectId) -> Result<Option<Project>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare("SELECT id, name, repo_path, sort_order, divider_after, divider_label, divider_before, divider_before_label FROM projects WHERE id = ?1")?;
        let mut rows = stmt.query(params![id.as_str()])?;
        Ok(rows.next()?.map(|r| Project {
            id: ProjectId(r.get::<_, String>(0).unwrap()),
            name: r.get(1).unwrap(),
            repo_path: PathBuf::from(r.get::<_, String>(2).unwrap()),
            sort_order: r.get(3).unwrap(),
            divider_after: r.get::<_, i64>(4).unwrap() != 0,
            divider_label: r.get(5).unwrap(),
            divider_before: r.get::<_, i64>(6).unwrap() != 0,
            divider_before_label: r.get(7).unwrap(),
        }))
    }

    pub fn get_worktree(&self, id: &WorktreeId) -> Result<Option<Worktree>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, project_id, path, branch, is_main, pinned, sort_order FROM worktrees WHERE id = ?1",
        )?;
        let mut rows = stmt.query(params![id.as_str()])?;
        Ok(rows.next()?.map(|r| Worktree {
            id: WorktreeId(r.get::<_, String>(0).unwrap()),
            project_id: ProjectId(r.get::<_, String>(1).unwrap()),
            path: PathBuf::from(r.get::<_, String>(2).unwrap()),
            branch: r.get(3).unwrap(),
            is_main: r.get::<_, i64>(4).unwrap() != 0,
            pinned: r.get::<_, i64>(5).unwrap() != 0,
            sort_order: r.get(6).unwrap(),
        }))
    }

    pub fn get_agent(&self, id: &AgentId) -> Result<Option<Agent>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, worktree_id, name, status, archived, pinned, kind, claude_session_id, sort_order, status_changed_at, model, effort, archived_at FROM agents WHERE id = ?1",
        )?;
        let mut rows = stmt.query(params![id.as_str()])?;
        Ok(rows.next()?.map(|r| Agent {
            id: AgentId(r.get::<_, String>(0).unwrap()),
            worktree_id: WorktreeId(r.get::<_, String>(1).unwrap()),
            name: r.get(2).unwrap(),
            status: AgentStatus::parse(&r.get::<_, String>(3).unwrap())
                .unwrap_or(AgentStatus::Fresh),
            archived: r.get::<_, i64>(4).unwrap() != 0,
            pinned: r.get::<_, i64>(5).unwrap() != 0,
            kind: AgentKind::parse(&r.get::<_, String>(6).unwrap()).unwrap_or_default(),
            session_id: r.get(7).unwrap(),
            sort_order: r.get(8).unwrap(),
            status_changed_at: r.get(9).unwrap(),
            model: r.get(10).unwrap(),
            effort: r.get(11).unwrap(),
            archived_at: r.get(12).unwrap(),
            alive: false,
        }))
    }

    pub fn get_terminal(&self, id: &TerminalId) -> Result<Option<TerminalTab>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt =
            conn.prepare("SELECT id, worktree_id, name, sort_order FROM terminals WHERE id = ?1")?;
        let mut rows = stmt.query(params![id.as_str()])?;
        Ok(rows.next()?.map(|r| TerminalTab {
            id: TerminalId(r.get::<_, String>(0).unwrap()),
            worktree_id: WorktreeId(r.get::<_, String>(1).unwrap()),
            name: r.get(2).unwrap(),
            sort_order: r.get(3).unwrap(),
            alive: false,
        }))
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
            .prepare("SELECT id, name, repo_path, sort_order, divider_after, divider_label, divider_before, divider_before_label FROM projects ORDER BY sort_order, created_at")?
            .query_map([], |r| {
                Ok(Project {
                    id: ProjectId(r.get(0)?),
                    name: r.get(1)?,
                    repo_path: PathBuf::from(r.get::<_, String>(2)?),
                    sort_order: r.get(3)?,
                    divider_after: r.get::<_, i64>(4)? != 0,
                    divider_label: r.get(5)?,
                    divider_before: r.get::<_, i64>(6)? != 0,
                    divider_before_label: r.get(7)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;

        let worktrees = conn
            .prepare("SELECT id, project_id, path, branch, is_main, pinned, sort_order FROM worktrees ORDER BY is_main DESC, sort_order, created_at")?
            .query_map([], |r| {
                Ok(Worktree {
                    id: WorktreeId(r.get(0)?),
                    project_id: ProjectId(r.get(1)?),
                    path: PathBuf::from(r.get::<_, String>(2)?),
                    branch: r.get(3)?,
                    is_main: r.get::<_, i64>(4)? != 0,
                    pinned: r.get::<_, i64>(5)? != 0,
                    sort_order: r.get(6)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;

        let agents = conn
            .prepare("SELECT id, worktree_id, name, status, archived, pinned, kind, claude_session_id, sort_order, status_changed_at, model, effort, archived_at FROM agents ORDER BY sort_order, created_at")?
            .query_map([], |r| {
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
                    alive: false,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;

        let terminals = conn
            .prepare("SELECT id, worktree_id, name, sort_order FROM terminals ORDER BY sort_order, created_at")?
            .query_map([], |r| {
                Ok(TerminalTab {
                    id: TerminalId(r.get(0)?),
                    worktree_id: WorktreeId(r.get(1)?),
                    name: r.get(2)?,
                    sort_order: r.get(3)?,
                    alive: false,
                })
            })?
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
        Ok(rows.next()?.map(|r| r.get::<_, String>(0).unwrap()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_tree() {
        let store = Store::open_in_memory().unwrap();
        let project = Project {
            id: ProjectId::generate(),
            name: "demo".into(),
            repo_path: "/tmp/demo".into(),
            sort_order: 0,
            divider_after: false,
            divider_label: None,
            divider_before: false,
            divider_before_label: None,
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
            kind: AgentKind::Claude,
            model: Some("opus".into()),
            effort: Some("high".into()),
            session_id: Some("sess-123".into()),
            sort_order: 0,
            status_changed_at: 0,
            alive: false,
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
            kind: AgentKind::Codex,
            model: None,
            effort: None,
            session_id: None,
            sort_order: 1,
            status_changed_at: 0,
            alive: false,
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
            kind: AgentKind::Cursor,
            model: None,
            effort: None,
            session_id: None,
            sort_order: 2,
            status_changed_at: 0,
            alive: false,
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

    #[test]
    fn todo_crud_roundtrip_and_cascade() {
        let store = Store::open_in_memory().unwrap();
        let project = Project {
            id: ProjectId::generate(),
            name: "demo".into(),
            repo_path: "/tmp/demo".into(),
            sort_order: 0,
            divider_after: false,
            divider_label: None,
            divider_before: false,
            divider_before_label: None,
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

        let wt_owner = TodoOwner::Worktree(worktree.id.clone());
        let todo = Todo {
            id: TodoId::generate(),
            owner: wt_owner.clone(),
            text: "write tests".into(),
            done: false,
            sort_order: store.next_todo_sort_order(&wt_owner).unwrap(),
        };
        store.insert_todo(&todo).unwrap();
        assert_eq!(store.next_todo_sort_order(&wt_owner).unwrap(), 1);

        // Project-scoped notes are their own list: separate sort space.
        let p_owner = TodoOwner::Project(project.id.clone());
        assert_eq!(store.next_todo_sort_order(&p_owner).unwrap(), 0);
        let project_todo = Todo {
            id: TodoId::generate(),
            owner: p_owner.clone(),
            text: "high level plan".into(),
            done: false,
            sort_order: 0,
        };
        store.insert_todo(&project_todo).unwrap();
        let read = store.get_todo(&project_todo.id).unwrap().unwrap();
        assert_eq!(read.owner, p_owner);

        store.set_todo_text(&todo.id, "write MORE tests").unwrap();
        store.set_todo_done(&todo.id, true).unwrap();
        let read = store.get_todo(&todo.id).unwrap().unwrap();
        assert_eq!(read.text, "write MORE tests");
        assert!(read.done);
        assert_eq!(read.owner, wt_owner);
        assert_eq!(store.load_todos().unwrap().len(), 2);

        store.delete_todo(&todo.id).unwrap();
        assert!(store.get_todo(&todo.id).unwrap().is_none());

        // Deleting the project cascades to its own todos AND (via the
        // worktree) its worktrees' todos.
        store.insert_todo(&todo).unwrap();
        store.delete_project(&project.id).unwrap();
        assert!(store.load_todos().unwrap().is_empty());
    }

    /// Real upgrade path: a v9 database (todos still worktree-only) picks
    /// up migration 10's table rebuild without losing any notes.
    #[test]
    fn migration_10_preserves_existing_worktree_todos() {
        let path = std::env::temp_dir().join(format!(
            "nebula-mig10-test-{}.db",
            std::process::id()
        ));
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
        let todos = store.load_todos().unwrap();
        assert_eq!(todos.len(), 1);
        assert_eq!(todos[0].owner, TodoOwner::Worktree(WorktreeId("w1".into())));
        assert_eq!(todos[0].text, "old note");
        assert!(todos[0].done);
        assert_eq!(todos[0].sort_order, 3);
        drop(store);
        for suffix in ["", "-wal", "-shm"] {
            let _ = std::fs::remove_file(format!("{}{}", path.display(), suffix));
        }
    }

    #[test]
    fn auto_title_pending_lifecycle() {
        let store = Store::open_in_memory().unwrap();
        let project = Project {
            id: ProjectId::generate(),
            name: "p".into(),
            repo_path: "/tmp/p".into(),
            sort_order: 0,
            divider_after: false,
            divider_label: None,
            divider_before: false,
            divider_before_label: None,
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
            kind: AgentKind::Claude,
            model: None,
            effort: None,
            session_id: None,
            sort_order: 0,
            status_changed_at: 0,
            alive: false,
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
            id: ProjectId::generate(),
            name: "demo".into(),
            repo_path: "/tmp/demo".into(),
            sort_order: 0,
            divider_after: false,
            divider_label: None,
            divider_before: false,
            divider_before_label: None,
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
            id: ProjectId::generate(),
            name: "p".into(),
            repo_path: "/tmp/p".into(),
            sort_order: 0,
            divider_after: false,
            divider_label: None,
            divider_before: false,
            divider_before_label: None,
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
                    kind: AgentKind::Claude,
                    model: None,
                    effort: None,
                    session_id: None,
                    sort_order: 0,
                    status_changed_at: 0,
                    alive: false,
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
}
