//! Workflow snapshots and SESSION associations commit together in the SQLITE STORE.

use super::Store;
use anyhow::{Context, Result};
use nebula_core::{
    workflow::{WorkflowRun, WorkflowSummary},
    AgentId,
};
use rusqlite::{params, OptionalExtension};

impl Store {
    pub fn save_workflow(&self, run: &WorkflowRun) -> Result<()> {
        let json = serde_json::to_string(run)?;
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        tx.execute(
            "INSERT INTO workflow_runs (id, active, updated_at, json) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(id) DO UPDATE SET active = excluded.active,
                 updated_at = excluded.updated_at, json = excluded.json",
            params![run.id, run.status.active(), run.updated_at, json],
        )?;
        for agent in run.stages.iter().filter_map(|stage| stage.agent.as_ref()) {
            tx.execute(
                "INSERT INTO workflow_sessions (agent_id, run_id) VALUES (?1, ?2)
                 ON CONFLICT(agent_id) DO NOTHING",
                params![agent.as_str(), run.id],
            )?;
            let owner: String = tx.query_row(
                "SELECT run_id FROM workflow_sessions WHERE agent_id = ?1",
                [agent.as_str()],
                |r| r.get(0),
            )?;
            anyhow::ensure!(
                owner == run.id,
                "SESSION already belongs to another workflow"
            );
        }
        tx.commit()?;
        Ok(())
    }

    pub fn workflow(&self, id: &str) -> Result<WorkflowRun> {
        let raw: String = self
            .conn
            .lock()
            .unwrap()
            .query_row("SELECT json FROM workflow_runs WHERE id = ?1", [id], |r| {
                r.get(0)
            })
            .optional()?
            .context("workflow not found")?;
        Ok(serde_json::from_str(&raw)?)
    }

    pub fn workflow_for_agent(&self, agent: &AgentId) -> Result<Option<WorkflowRun>> {
        let raw: Option<String> = self
            .conn
            .lock()
            .unwrap()
            .query_row(
                "SELECT r.json FROM workflow_runs r JOIN workflow_sessions s ON s.run_id = r.id
             WHERE s.agent_id = ?1",
                [agent.as_str()],
                |r| r.get(0),
            )
            .optional()?;
        raw.map(|s| serde_json::from_str(&s).map_err(Into::into))
            .transpose()
    }

    pub fn active_workflow_ids(&self) -> Result<Vec<String>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt =
            conn.prepare("SELECT id FROM workflow_runs WHERE active = 1 ORDER BY updated_at")?;
        let rows = stmt
            .query_map([], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        Ok(rows)
    }

    pub fn workflow_summaries(&self) -> Result<Vec<WorkflowSummary>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare("SELECT json FROM workflow_runs ORDER BY updated_at DESC")?;
        let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
        rows.map(|row| {
            let run: WorkflowRun = serde_json::from_str(&row?)?;
            Ok(run.summary())
        })
        .collect()
    }
}
