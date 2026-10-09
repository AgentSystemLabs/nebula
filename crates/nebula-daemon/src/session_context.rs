//! Agent-readable session context commands: list, read and ask.
//!
//! These are the non-privileged counterparts to the orchestrator's
//! list/read/send machinery. They use the same daemon tree, PTY delivery and
//! status broadcasts, but do not require the caller to be the orchestrator.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use nebula_core::{
    Agent, AgentId, AgentStatus, OutputTail, Project, ServerEvent, SessionRef, SessionSummary,
    TerminalTab, Worktree, MAX_CLOUD_PROMPT_BYTES,
};

use crate::registry::Daemon;

const READ_MAX_BYTES: usize = 256 * 1024;
const ASK_OUTPUT_MAX_BYTES: usize = 64 * 1024;
const DEFAULT_ASK_TIMEOUT: Duration = Duration::from_secs(120);
const PASTE_START: &[u8] = b"\x1b[200~";
const PASTE_END: &[u8] = b"\x1b[201~";

#[derive(Debug, Clone)]
pub struct SessionRead {
    pub session: SessionRef,
    pub text: String,
}

#[derive(Debug, Clone)]
struct SessionLookup {
    summary: SessionSummary,
}

impl Daemon {
    /// Model-facing rows for every non-archived session across every project.
    pub fn session_summaries(&self) -> Result<Vec<SessionSummary>> {
        let (projects, worktrees, mut agents, mut terminals) = self.store.load_tree()?;
        let projects = projects
            .into_iter()
            .map(|p| (p.id.clone(), p))
            .collect::<HashMap<_, _>>();
        let worktrees = worktrees
            .into_iter()
            .map(|w| (w.id.clone(), w))
            .collect::<HashMap<_, _>>();

        for agent in &mut agents {
            agent.alive = self.is_alive(&SessionRef::Agent(agent.id.clone()));
        }
        for terminal in &mut terminals {
            terminal.alive = self.is_alive(&SessionRef::Terminal(terminal.id.clone()));
        }

        let mut rows = Vec::new();
        for agent in agents.into_iter().filter(|a| !a.archived) {
            if let Some(row) = agent_summary(&projects, &worktrees, agent) {
                rows.push(row);
            }
        }
        for terminal in terminals {
            if let Some(row) = terminal_summary(&projects, &worktrees, terminal) {
                rows.push(row);
            }
        }
        rows.sort_by(|a, b| {
            a.project
                .cmp(&b.project)
                .then_with(|| a.branch.cmp(&b.branch))
                .then_with(|| a.name.cmp(&b.name))
        });
        Ok(rows)
    }

    /// Resolve a user-facing session selector by id/name with unambiguous
    /// fuzzy fallback.
    pub fn resolve_session_target(&self, raw: &str) -> Result<SessionRef> {
        let target = raw.trim();
        if target.is_empty() {
            bail!("session name or id is empty");
        }
        let rows = self
            .session_summaries()?
            .into_iter()
            .map(|summary| SessionLookup { summary })
            .collect::<Vec<_>>();
        resolve_in_rows(&rows, target).map(|row| row.summary.session.clone())
    }

    /// Clean recent context for `target`, preferring a known Claude
    /// transcript and falling back to the live PTY scrollback.
    pub fn read_session_context(&self, target: &str, max_lines: u32) -> Result<SessionRead> {
        let session = self.resolve_session_target(target)?;
        let text = self.read_session_ref(&session, max_lines)?;
        Ok(SessionRead { session, text })
    }

    /// Read a known session ref. Orchestrator read calls this after its own
    /// permission check so both surfaces share transcript and cleanup logic.
    pub fn read_session_ref(&self, session: &SessionRef, max_lines: u32) -> Result<String> {
        if let SessionRef::Agent(id) = session {
            if let Some(text) = self.read_agent_transcript(id, max_lines) {
                return Ok(text);
            }
        }
        let max = READ_MAX_BYTES;
        let text = self
            .session(session)
            .map(|s| clean_pty_bytes(&s.tail(max, None).data))
            .unwrap_or_else(|| "(session is not live and no transcript is available)\n".into());
        Ok(limit_lines(&text, max_lines))
    }

    pub fn read_output_tail_clean(
        &self,
        session: &SessionRef,
        max_bytes: u32,
    ) -> Option<OutputTail> {
        let max = (max_bytes as usize).min(READ_MAX_BYTES);
        self.session(session).map(|s| {
            let mut tail = s.tail(max, None);
            tail.data = clean_pty_bytes(&tail.data).into_bytes();
            tail
        })
    }

    fn read_agent_transcript(&self, id: &AgentId, max_lines: u32) -> Option<String> {
        let transcript = self.transcripts.lock().get(id).cloned()?;
        transcript_context(&transcript.transcript_path, max_lines)
    }

    /// Deliver `question` into another agent and return the clean output
    /// produced by that turn.
    pub async fn ask_session(
        self: &Arc<Self>,
        caller: Option<AgentId>,
        target: &str,
        question: &str,
        timeout: Duration,
        wait: bool,
    ) -> Result<(AgentId, String)> {
        let timeout = if timeout.is_zero() {
            DEFAULT_ASK_TIMEOUT
        } else {
            timeout
        };
        let deadline = tokio::time::Instant::now() + timeout;
        let target_ref = self.resolve_session_target(target)?;
        let SessionRef::Agent(target_id) = target_ref else {
            bail!("questions can only be sent to agent sessions");
        };
        if caller.as_ref() == Some(&target_id) {
            bail!("cannot ask the current session; choose another session");
        }
        let target_agent = self
            .store
            .get_agent(&target_id)?
            .context("target agent not found")?;
        if target_agent.archived {
            bail!("target agent is archived");
        }
        if target_agent.cloud_session_id.is_some() {
            bail!("target agent is a Claude Cloud session; use its cloud message path");
        }

        let mut status_rx = self.events.subscribe();
        let current = target_agent.status;
        if !is_idle(current) {
            if !wait {
                bail!(
                    "target session is {}; retry without --no-wait or choose an idle session",
                    current.as_str()
                );
            }
            wait_until_idle(&mut status_rx, &target_id, deadline).await?;
        }

        let caller_name = caller
            .as_ref()
            .and_then(|id| self.store.get_agent(id).ok().flatten())
            .map(|a| a.name)
            .unwrap_or_else(|| "external nebula CLI".into());
        let prompt = labelled_question(&caller_name, question)?;
        let session_ref = SessionRef::Agent(target_id.clone());
        let session = self.ensure_session(
            &session_ref,
            crate::pty::DEFAULT_COLS,
            crate::pty::DEFAULT_ROWS,
        )?;
        let start_seq = session.tail(0, None).end_seq;
        write_prompt(&session, &prompt)?;

        wait_for_answer_turn(&mut status_rx, &target_id, deadline).await?;
        let (_, bytes) = session.snapshot(Some(start_seq));
        let answer = answer_text(&bytes, question);
        Ok((target_id, answer))
    }
}

pub(crate) fn write_prompt(session: &crate::pty::PtySession, text: &str) -> Result<()> {
    let data = if text.contains('\n') {
        bracketed(text)
    } else {
        text.as_bytes().to_vec()
    };
    session.write_input(&data)?;
    session.write_input(b"\r")?;
    Ok(())
}

fn agent_summary(
    projects: &HashMap<nebula_core::ProjectId, Project>,
    worktrees: &HashMap<nebula_core::WorktreeId, Worktree>,
    agent: Agent,
) -> Option<SessionSummary> {
    let worktree = worktrees.get(&agent.worktree_id)?;
    let project = projects.get(&worktree.project_id)?;
    let summary = agent
        .recent_prompts
        .last()
        .map(|p| p.text.clone())
        .unwrap_or_else(|| agent.name.clone());
    Some(SessionSummary {
        session: SessionRef::Agent(agent.id.clone()),
        id: agent.id.to_string(),
        name: agent.name,
        project_id: project.id.clone(),
        project: project.name.clone(),
        worktree_id: worktree.id.clone(),
        worktree: worktree.path.clone(),
        branch: worktree.branch.clone(),
        harness: agent.kind.as_str().to_string(),
        status: public_status(agent.status).to_string(),
        alive: agent.alive,
        summary,
    })
}

fn terminal_summary(
    projects: &HashMap<nebula_core::ProjectId, Project>,
    worktrees: &HashMap<nebula_core::WorktreeId, Worktree>,
    terminal: TerminalTab,
) -> Option<SessionSummary> {
    let worktree = worktrees.get(&terminal.worktree_id)?;
    let project = projects.get(&worktree.project_id)?;
    let summary = terminal
        .run_command
        .as_deref()
        .unwrap_or(&terminal.name)
        .to_string();
    Some(SessionSummary {
        session: SessionRef::Terminal(terminal.id.clone()),
        id: format!("terminal:{}", terminal.id),
        name: terminal.name,
        project_id: project.id.clone(),
        project: project.name.clone(),
        worktree_id: worktree.id.clone(),
        worktree: worktree.path.clone(),
        branch: worktree.branch.clone(),
        harness: "terminal".into(),
        status: if terminal.alive { "running" } else { "idle" }.into(),
        alive: terminal.alive,
        summary,
    })
}

fn public_status(status: AgentStatus) -> &'static str {
    match status {
        AgentStatus::Fresh | AgentStatus::Finished => "idle",
        AgentStatus::Running => "running",
        AgentStatus::NeedsFeedback => "needs_feedback",
        AgentStatus::Terminated => "terminated",
        AgentStatus::Disconnected => "disconnected",
    }
}

fn is_idle(status: AgentStatus) -> bool {
    matches!(
        status,
        AgentStatus::Fresh
            | AgentStatus::Finished
            | AgentStatus::Terminated
            | AgentStatus::Disconnected
    )
}

fn resolve_in_rows<'a>(rows: &'a [SessionLookup], raw: &str) -> Result<&'a SessionLookup> {
    let needle = raw.to_ascii_lowercase();
    let exact = rows
        .iter()
        .filter(|row| {
            row.summary.id.eq_ignore_ascii_case(raw) || row.summary.name.eq_ignore_ascii_case(raw)
        })
        .collect::<Vec<_>>();
    match exact.as_slice() {
        [row] => return Ok(row),
        [] => {}
        _ => bail!("session name is ambiguous: {raw}; use an id"),
    }

    let fuzzy = rows
        .iter()
        .filter(|row| {
            [
                row.summary.id.as_str(),
                row.summary.name.as_str(),
                row.summary.project.as_str(),
                row.summary.branch.as_str(),
            ]
            .iter()
            .any(|field| field.to_ascii_lowercase().contains(&needle))
        })
        .collect::<Vec<_>>();
    match fuzzy.as_slice() {
        [row] => Ok(row),
        [] => bail!("session not found: {raw}"),
        _ => bail!("session match is ambiguous: {raw}; use an id"),
    }
}

fn transcript_context(path: &Path, max_lines: u32) -> Option<String> {
    let tail = crate::session_title::transcript_tail(path)?;
    let mut lines = Vec::new();
    for line in tail.lines() {
        let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        if value.get("type").and_then(|v| v.as_str()) == Some("custom-title") {
            continue;
        }
        if let Some(rendered) = render_transcript_line(&value) {
            lines.push(rendered);
        }
    }
    let text = lines.join("\n");
    (!text.trim().is_empty()).then(|| limit_lines(&(text + "\n"), max_lines))
}

fn render_transcript_line(value: &serde_json::Value) -> Option<String> {
    let (role, content) = if let Some(message) = value.get("message") {
        (
            message.get("role").and_then(|v| v.as_str()),
            message.get("content"),
        )
    } else {
        (
            value
                .get("role")
                .and_then(|v| v.as_str())
                .or_else(|| value.get("type").and_then(|v| v.as_str())),
            value
                .get("content")
                .or_else(|| value.get("text"))
                .or_else(|| value.get("summary")),
        )
    };
    let role = role?;
    let text = content_text(content?)?;
    let role = match role {
        "user" => "User",
        "assistant" => "Assistant",
        "summary" => "Summary",
        other => other,
    };
    Some(format!("{role}: {}", single_line(&text)))
}

fn content_text(value: &serde_json::Value) -> Option<String> {
    if let Some(text) = value.as_str() {
        return Some(text.to_string());
    }
    if let Some(items) = value.as_array() {
        let parts = items
            .iter()
            .filter_map(|item| {
                item.get("text")
                    .and_then(|v| v.as_str())
                    .or_else(|| item.get("content").and_then(|v| v.as_str()))
            })
            .collect::<Vec<_>>();
        if !parts.is_empty() {
            return Some(parts.join("\n"));
        }
    }
    None
}

fn single_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn limit_lines(text: &str, max_lines: u32) -> String {
    let max = max_lines.max(1) as usize;
    let lines = text.lines().collect::<Vec<_>>();
    let start = lines.len().saturating_sub(max);
    let mut out = lines[start..].join("\n");
    if !out.is_empty() {
        out.push('\n');
    }
    if out.len() > READ_MAX_BYTES {
        out = out[out.len().saturating_sub(READ_MAX_BYTES)..].to_string();
    }
    out
}

fn clean_pty_bytes(bytes: &[u8]) -> String {
    strip_ansi(&String::from_utf8_lossy(bytes))
}

fn strip_ansi(input: &str) -> String {
    #[derive(Clone, Copy)]
    enum State {
        Text,
        Esc,
        Csi,
        Osc,
        OscEsc,
    }
    let mut out = String::new();
    let mut state = State::Text;
    for ch in input.chars() {
        state = match state {
            State::Text if ch == '\x1b' => State::Esc,
            State::Text => {
                out.push(ch);
                State::Text
            }
            State::Esc if ch == '[' => State::Csi,
            State::Esc if ch == ']' => State::Osc,
            State::Esc => State::Text,
            State::Csi if ('@'..='~').contains(&ch) => State::Text,
            State::Csi => State::Csi,
            State::Osc if ch == '\x07' => State::Text,
            State::Osc if ch == '\x1b' => State::OscEsc,
            State::Osc => State::Osc,
            State::OscEsc if ch == '\\' => State::Text,
            State::OscEsc => State::Osc,
        };
    }
    out.replace('\r', "\n")
}

fn labelled_question(caller_name: &str, question: &str) -> Result<String> {
    let question = validate_question(question)?;
    Ok(format!(
        "[question from session \"{}\"]\n{}\n\nAnswer concisely; your reply is returned to the asking session.",
        caller_name.trim(),
        question
    ))
}

fn validate_question(question: &str) -> Result<&str> {
    let text = question.trim();
    if text.is_empty() {
        bail!("question is empty");
    }
    if text.contains('\0') {
        bail!("question cannot contain NUL bytes");
    }
    if text.len() > MAX_CLOUD_PROMPT_BYTES {
        bail!(
            "question is too long (max {} KiB)",
            MAX_CLOUD_PROMPT_BYTES / 1024
        );
    }
    Ok(text)
}

fn bracketed(text: &str) -> Vec<u8> {
    let mut data = PASTE_START.to_vec();
    data.extend_from_slice(text.as_bytes());
    data.extend_from_slice(PASTE_END);
    data
}

async fn wait_until_idle(
    rx: &mut tokio::sync::broadcast::Receiver<ServerEvent>,
    target: &AgentId,
    deadline: tokio::time::Instant,
) -> Result<()> {
    loop {
        let event = recv_until(rx, deadline).await?;
        if let ServerEvent::StatusChanged { agent, status, .. } = event {
            if &agent == target && is_idle(status) {
                return Ok(());
            }
        }
    }
}

async fn wait_for_answer_turn(
    rx: &mut tokio::sync::broadcast::Receiver<ServerEvent>,
    target: &AgentId,
    deadline: tokio::time::Instant,
) -> Result<()> {
    let mut saw_activity = false;
    loop {
        let event = recv_until(rx, deadline).await?;
        match event {
            ServerEvent::StatusChanged { agent, status, .. } if agent == *target => {
                if matches!(status, AgentStatus::Running | AgentStatus::NeedsFeedback) {
                    saw_activity = true;
                } else if saw_activity && is_idle(status) {
                    return Ok(());
                }
            }
            ServerEvent::SessionExited {
                session: SessionRef::Agent(agent),
                ..
            } if agent == *target => return Ok(()),
            _ => {}
        }
    }
}

async fn recv_until(
    rx: &mut tokio::sync::broadcast::Receiver<ServerEvent>,
    deadline: tokio::time::Instant,
) -> Result<ServerEvent> {
    loop {
        let now = tokio::time::Instant::now();
        if now >= deadline {
            bail!("timed out waiting for the target session");
        }
        match tokio::time::timeout_at(deadline, rx.recv()).await {
            Ok(Ok(event)) => return Ok(event),
            Ok(Err(tokio::sync::broadcast::error::RecvError::Lagged(_))) => continue,
            Ok(Err(tokio::sync::broadcast::error::RecvError::Closed)) => {
                bail!("daemon event stream closed")
            }
            Err(_) => bail!("timed out waiting for the target session"),
        }
    }
}

fn answer_text(bytes: &[u8], question: &str) -> String {
    let clean = clean_pty_bytes(bytes);
    let question = question.trim();
    let lines = clean
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .filter(|line| !line.contains("[question from session"))
        .filter(|line| *line != question)
        .filter(|line| !line.starts_with("Answer concisely;"))
        .collect::<Vec<_>>();
    let text = lines.join("\n");
    if text.len() > ASK_OUTPUT_MAX_BYTES {
        text[text.len().saturating_sub(ASK_OUTPUT_MAX_BYTES)..].to_string()
    } else {
        text
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hooks::HookEnv;
    use crate::store::Store;
    use nebula_core::{ProjectId, TerminalId, WorktreeId};

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
                name: "nebula".into(),
                repo_path: "/tmp/nebula".into(),
                sort_order: 0,
            })
            .unwrap();
        daemon
            .store
            .insert_worktree(&Worktree {
                id: WorktreeId("w".into()),
                project_id: ProjectId("p".into()),
                path: "/tmp/nebula".into(),
                branch: "main".into(),
                is_main: true,
                sort_order: 0,
            })
            .unwrap();
        daemon
    }

    fn agent(id: &str, name: &str, status: AgentStatus) -> Agent {
        Agent {
            id: AgentId(id.into()),
            worktree_id: WorktreeId("w".into()),
            name: name.into(),
            status,
            archived: false,
            archived_at: 0,
            unseen: false,
            status_changed_at: 0,
            kind: nebula_core::AgentKind::Claude,
            custom_harness: None,
            model: None,
            effort: None,
            session_id: Some("sid".into()),
            cloud_session_id: None,
            issue_url: None,
            role: nebula_core::AgentRole::Worker,
            sort_order: 0,
            alive: false,
            recent_prompts: Vec::new(),
        }
    }

    #[test]
    fn list_output_includes_project_worktree_harness_status_and_summary() {
        let daemon = daemon();
        let mut a = agent("a1", "research", AgentStatus::Finished);
        a.recent_prompts.push(nebula_core::PromptEntry {
            text: "which json parser?".into(),
            submitted_at: 7,
        });
        daemon.store.insert_agent(&a).unwrap();
        daemon
            .store
            .push_prompt(&a.id, a.recent_prompts.last().unwrap())
            .unwrap();
        daemon
            .store
            .insert_terminal(&TerminalTab {
                id: TerminalId("t1".into()),
                worktree_id: WorktreeId("w".into()),
                name: "shell".into(),
                sort_order: 0,
                alive: false,
                run_command: None,
            })
            .unwrap();

        let rows = daemon.session_summaries().unwrap();
        assert!(rows.iter().any(|r| {
            r.name == "research"
                && r.project == "nebula"
                && r.branch == "main"
                && r.harness == "claude"
                && r.status == "idle"
                && r.summary == "which json parser?"
        }));
        assert!(rows
            .iter()
            .any(|r| r.name == "shell" && r.harness == "terminal"));
    }

    #[test]
    fn fuzzy_resolution_rejects_ambiguous_names() {
        let daemon = daemon();
        daemon
            .store
            .insert_agent(&agent("a1", "research alpha", AgentStatus::Finished))
            .unwrap();
        daemon
            .store
            .insert_agent(&agent("a2", "research beta", AgentStatus::Finished))
            .unwrap();

        assert!(daemon
            .resolve_session_target("a1")
            .is_ok_and(|s| matches!(s, SessionRef::Agent(id) if id.0 == "a1")));
        let err = daemon.resolve_session_target("research").unwrap_err();
        assert!(err.to_string().contains("ambiguous"));
    }

    #[test]
    fn transcript_reading_renders_clean_capped_text() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sid.jsonl");
        std::fs::write(
            &path,
            concat!(
                r#"{"type":"user","message":{"role":"user","content":"first"}}"#,
                "\n",
                r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"second"}]}}"#,
                "\n",
                r#"{"type":"user","message":{"role":"user","content":"third"}}"#,
                "\n",
            ),
        )
        .unwrap();
        let text = transcript_context(&path, 2).unwrap();
        assert_eq!(text, "Assistant: second\nUser: third\n");
    }

    #[test]
    fn ansi_is_stripped_and_capped() {
        let clean = strip_ansi("one\x1b[31m red\x1b[0m\r\ntwo\x1b]0;title\x07\nthree");
        assert_eq!(limit_lines(&clean, 2), "two\nthree\n");
    }
}
