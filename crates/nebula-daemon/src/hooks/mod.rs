//! Agent-CLI hook receiver: a loopback-only HTTP endpoint the shell hook
//! one-liners POST to (`/api/hooks/claude`, `/api/hooks/codex`, and
//! `/api/hooks/cursor`). Codex mirrors Claude's hook events and payload
//! shape; cursor speaks its own dialect, but its installer translates event
//! names into the `hookEvent` query param and the payload fields are aliased
//! here (`conversation_id`, `subagent_id`, `workspace_roots`), so one
//! handler serves all three. Fail-soft on both sides — a malformed payload
//! still gets a 200 so a broken hook never faults the user's agent turn.

pub mod installer;

use crate::status::HookEvent;
use crate::store::Store;
use axum::extract::{Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::routing::post;
use axum::Router;
use nebula_core::AgentId;
use serde::Deserialize;
use std::sync::Arc;
use subtle::ConstantTimeEq;
use tokio::sync::mpsc;

/// The auto-title instruction, in the one wording every channel that
/// carries it uses: the UserPromptSubmit context injection (claude/codex)
/// and cursor's always-on project rule. Repeats on every prompt until a
/// title sticks, so it has to read sanely on a session that already has one.
pub const AUTO_TITLE_INSTRUCTION: &str = "[nebula] Before addressing the \
user's request, run this shell command exactly once:\n\n  nebula rename \
<title>\n\nReplace <title> with 3-4 Title Case words describing the user's \
request, unquoted (example: nebula rename Fix Login Redirect). If it reports \
the session is already titled, accept that and move on. Then continue with \
the request. Don't mention the rename to the user.";

/// The instruction as a UserPromptSubmit hook's stdout. Codex only reads
/// injected context out of this JSON envelope (its hook output schema is
/// strict — bare text is discarded); Claude Code documents the same shape
/// as the equivalent of bare text, so both CLIs share one body.
pub fn auto_title_injection() -> String {
    serde_json::json!({
        "hookSpecificOutput": {
            "hookEventName": "UserPromptSubmit",
            "additionalContext": AUTO_TITLE_INSTRUCTION,
        }
    })
    .to_string()
}

#[derive(Clone)]
pub struct HookEnv {
    pub port: u16,
    pub token: String,
}

pub struct HookServerState {
    token: String,
    tx: mpsc::Sender<HookDelivery>,
    /// Read-only peek at agent rows: drives the auto-title injection
    /// decision without a round-trip through the daemon's drain loop.
    store: Arc<Store>,
}

/// One accepted hook POST, decoded for the daemon's drain loop.
#[derive(Debug)]
pub struct HookDelivery {
    pub agent_id: AgentId,
    pub event: HookEvent,
    pub session_id: Option<String>,
    /// The CLI's working directory as reported in the payload (Claude Code
    /// sends it on every event); drives cwd-based agent re-homing. Absent
    /// when the CLI doesn't report it — re-homing simply never triggers.
    pub cwd: Option<String>,
}

/// Permissive payload: every field optional, unknown fields ignored. Hook
/// payload shapes are Claude-version-dependent; drift must degrade to
/// "status stops updating", never to an error.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct HookPayload {
    pub hook_event_name: Option<String>,
    pub session_id: Option<String>,
    /// Cursor names the resumable chat id `conversation_id` (== its
    /// `session_id`, but only the alias is guaranteed on every event).
    pub conversation_id: Option<String>,
    pub notification_type: Option<String>,
    pub tool_name: Option<String>,
    pub agent_id: Option<String>,
    /// Cursor's name for the subagent id in subagentStart/subagentStop.
    pub subagent_id: Option<String>,
    pub source: Option<String>,
    pub exit_code: Option<i32>,
    pub cwd: Option<String>,
    /// Cursor sends no `cwd`; its first workspace root plays the role.
    pub workspace_roots: Option<Vec<String>>,
}

impl HookPayload {
    fn session_id(&self) -> Option<String> {
        self.session_id
            .clone()
            .or_else(|| self.conversation_id.clone())
    }

    fn cwd(&self) -> Option<String> {
        self.cwd
            .clone()
            .or_else(|| self.workspace_roots.as_ref()?.first().cloned())
    }

    fn subagent_id(&self) -> Option<String> {
        self.agent_id.clone().or_else(|| self.subagent_id.clone())
    }
}

#[derive(Debug, Deserialize)]
pub struct HookQuery {
    #[serde(rename = "agentId")]
    pub agent_id: String,
    #[serde(rename = "hookEvent")]
    pub hook_event: String,
}

pub fn parse_event(hook_event: &str, payload: &HookPayload) -> Option<HookEvent> {
    Some(match hook_event {
        "UserPromptSubmit" => HookEvent::UserPromptSubmit,
        "Stop" => HookEvent::Stop,
        "SessionStart" => HookEvent::SessionStart {
            source: payload.source.clone(),
        },
        "PermissionRequest" => HookEvent::PermissionRequest,
        "Notification" => HookEvent::Notification {
            notification_type: payload.notification_type.clone(),
        },
        "PreToolUse" => HookEvent::PreToolUse {
            tool_name: payload.tool_name.clone(),
        },
        "PostToolUse" => HookEvent::PostToolUse {
            tool_name: payload.tool_name.clone(),
        },
        "SubagentStart" => HookEvent::SubagentStart {
            subagent_id: payload.subagent_id(),
        },
        "SubagentStop" => HookEvent::SubagentStop {
            subagent_id: payload.subagent_id(),
        },
        _ => return None,
    })
}

/// Bind 127.0.0.1:0 and serve the hook route. Returns the env (port + fresh
/// bearer token) and the receiving end of the event pipe.
pub async fn start_hook_server(
    store: Arc<Store>,
) -> anyhow::Result<(HookEnv, mpsc::Receiver<HookDelivery>)> {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let port = listener.local_addr()?.port();
    let token = generate_token();
    let (tx, rx) = mpsc::channel(256);

    let state = Arc::new(HookServerState {
        token: token.clone(),
        tx,
        store,
    });
    // Claude and Codex UserPromptSubmit hooks pipe this server's response
    // body to the CLI's stdout, where it lands in the model's context —
    // that's the auto-title instruction channel. Cursor's dialect has no
    // such channel (its hooks answer with their own gating JSON), so it
    // takes the plain route.
    let app = Router::new()
        .route("/api/hooks/claude", post(receive_injectable_hook))
        .route("/api/hooks/codex", post(receive_injectable_hook))
        .route("/api/hooks/cursor", post(receive_plain_hook))
        .with_state(state);

    tokio::spawn(async move {
        if let Err(e) = axum::serve(listener, app).await {
            tracing::error!(error = %e, "hook http server died");
        }
    });

    Ok((HookEnv { port, token }, rx))
}

fn generate_token() -> String {
    use rand::Rng;
    let mut bytes = [0u8; 32];
    rand::rng().fill_bytes(&mut bytes);
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use nebula_core::{Agent, AgentKind, AgentStatus, Project, ProjectId, Worktree, WorktreeId};

    /// Minimal raw HTTP/1.1 POST (Connection: close), so the real response
    /// body — what the hook one-liner pipes to the CLI's stdout — is under
    /// test, not a re-implementation of the handler's logic.
    async fn http_post(port: u16, path_query: &str, token: &str, body: &str) -> (u16, String) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let mut s = tokio::net::TcpStream::connect(("127.0.0.1", port))
            .await
            .unwrap();
        let req = format!(
            "POST {path_query} HTTP/1.1\r\nHost: 127.0.0.1\r\nAuthorization: Bearer {token}\r\n\
             Content-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        s.write_all(req.as_bytes()).await.unwrap();
        let mut buf = Vec::new();
        s.read_to_end(&mut buf).await.unwrap();
        let text = String::from_utf8_lossy(&buf).to_string();
        let status: u16 = text.split_whitespace().nth(1).unwrap().parse().unwrap();
        let body = text
            .split_once("\r\n\r\n")
            .map(|(_, b)| b.to_string())
            .unwrap_or_default();
        (status, body)
    }

    fn seeded_store() -> Arc<Store> {
        let store = Arc::new(Store::open_in_memory().unwrap());
        store
            .insert_project(&Project {
                workspace_id: Default::default(),
                id: ProjectId("p1".into()),
                name: "p".into(),
                repo_path: "/tmp/p".into(),
                sort_order: 0,
            })
            .unwrap();
        store
            .insert_worktree(&Worktree {
                id: WorktreeId("w1".into()),
                project_id: ProjectId("p1".into()),
                path: "/tmp/p".into(),
                branch: "main".into(),
                is_main: true,
                pinned: false,
                sort_order: 0,
            })
            .unwrap();
        let agent = |id: &str| Agent {
            id: AgentId(id.into()),
            worktree_id: WorktreeId("w1".into()),
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
        store
            .insert_agent_with_auto_title(&agent("pending"), true)
            .unwrap();
        store.insert_agent(&agent("titled")).unwrap();
        store
    }

    #[tokio::test]
    async fn user_prompt_submit_injects_title_instruction_only_while_pending() {
        let store = seeded_store();
        let (env, mut rx) = start_hook_server(store.clone()).await.unwrap();
        let payload = r#"{"session_id":"s1"}"#;

        // Untitled session: the instruction rides the response body (and the
        // status delivery still flows).
        let (status, body) = http_post(
            env.port,
            "/api/hooks/claude?agentId=pending&hookEvent=UserPromptSubmit",
            &env.token,
            payload,
        )
        .await;
        assert_eq!(status, 200);
        // The instruction rides codex's strict JSON envelope, which claude
        // reads as the documented equivalent of bare text.
        assert_eq!(body, auto_title_injection());
        assert!(body.contains("hookSpecificOutput"), "envelope: {body}");
        assert!(body.contains("nebula rename"), "instruction: {body}");
        let delivery = rx.recv().await.unwrap();
        assert_eq!(delivery.agent_id.as_str(), "pending");
        assert_eq!(delivery.event, HookEvent::UserPromptSubmit);

        // Codex shares the injectable dialect.
        let (_, body) = http_post(
            env.port,
            "/api/hooks/codex?agentId=pending&hookEvent=UserPromptSubmit",
            &env.token,
            payload,
        )
        .await;
        assert_eq!(body, auto_title_injection());

        // Titled session: strictly empty — anything else would leak into
        // the model's context.
        let (status, body) = http_post(
            env.port,
            "/api/hooks/claude?agentId=titled&hookEvent=UserPromptSubmit",
            &env.token,
            payload,
        )
        .await;
        assert_eq!((status, body.as_str()), (200, ""));

        // Unknown agent (prewarm/stale env): same silence.
        let (_, body) = http_post(
            env.port,
            "/api/hooks/claude?agentId=ghost&hookEvent=UserPromptSubmit",
            &env.token,
            payload,
        )
        .await;
        assert_eq!(body, "");

        // Other events keep their diagnostic body (discarded by the hooks).
        let (_, body) = http_post(
            env.port,
            "/api/hooks/claude?agentId=pending&hookEvent=Stop",
            &env.token,
            payload,
        )
        .await;
        assert_eq!(body, r#"{"ok": true}"#);

        // Cursor's dialect can't inject — no instruction even while pending.
        let (_, body) = http_post(
            env.port,
            "/api/hooks/cursor?agentId=pending&hookEvent=UserPromptSubmit",
            &env.token,
            payload,
        )
        .await;
        assert_eq!(body, r#"{"ok": true}"#);

        // Bad token on the injectable path: 401 and an EMPTY body, so a
        // misconfigured hook can't inject diagnostics as context.
        let (status, body) = http_post(
            env.port,
            "/api/hooks/claude?agentId=pending&hookEvent=UserPromptSubmit",
            "wrong-token",
            payload,
        )
        .await;
        assert_eq!((status, body.as_str()), (401, ""));
    }

    #[tokio::test]
    async fn bash_tool_use_carries_cwd_but_subagent_traffic_does_not() {
        let store = seeded_store();
        let (env, mut rx) = start_hook_server(store).await.unwrap();

        // The mid-turn position signal: a Bash call that just `cd`ed into a
        // fresh worktree, long before the turn's Stop.
        let (status, _) = http_post(
            env.port,
            "/api/hooks/claude?agentId=titled&hookEvent=PostToolUse",
            &env.token,
            r#"{"session_id":"s1","tool_name":"Bash","cwd":"/w/feat"}"#,
        )
        .await;
        assert_eq!(status, 200);
        let delivery = rx.recv().await.unwrap();
        assert_eq!(delivery.cwd.as_deref(), Some("/w/feat"));
        assert_eq!(
            delivery.event,
            HookEvent::PostToolUse {
                tool_name: Some("Bash".into())
            }
        );

        // Same event from a Task subagent (claude stamps `agent_id` on
        // subagent tool traffic): the status delivery still flows, but the
        // cwd is dropped so an isolated subagent can't re-home the row.
        let (status, _) = http_post(
            env.port,
            "/api/hooks/claude?agentId=titled&hookEvent=PostToolUse",
            &env.token,
            r#"{"session_id":"s1","tool_name":"Bash","cwd":"/w/scratch","agent_id":"sub-1"}"#,
        )
        .await;
        assert_eq!(status, 200);
        let delivery = rx.recv().await.unwrap();
        assert!(delivery.cwd.is_none(), "subagent cwd: {:?}", delivery.cwd);
    }

    #[test]
    fn payload_parses_cwd_and_tolerates_unknown_fields() {
        let payload: HookPayload = serde_json::from_str(
            r#"{"session_id":"s1","cwd":"/w/feat","transcript_path":"/x.jsonl","novel":1}"#,
        )
        .unwrap();
        assert_eq!(payload.cwd.as_deref(), Some("/w/feat"));
        assert_eq!(payload.session_id.as_deref(), Some("s1"));
        // Absent cwd stays None (codex/cursor payloads may not send it).
        let payload: HookPayload = serde_json::from_str(r#"{"session_id":"s1"}"#).unwrap();
        assert!(payload.cwd.is_none());
    }

    #[test]
    fn cursor_payload_aliases_map_to_claude_fields() {
        // Real cursor-agent payload shape: conversation_id + workspace_roots,
        // no cwd; session_id happens to be present too but the aliases must
        // carry the day when it is not.
        let payload: HookPayload = serde_json::from_str(
            r#"{"conversation_id":"c1","workspace_roots":["/w/feat","/w/extra"],
                "hook_event_name":"stop","status":"completed"}"#,
        )
        .unwrap();
        assert_eq!(payload.session_id().as_deref(), Some("c1"));
        assert_eq!(payload.cwd().as_deref(), Some("/w/feat"));
        // Explicit session_id wins over the alias.
        let payload: HookPayload =
            serde_json::from_str(r#"{"session_id":"s1","conversation_id":"c1"}"#).unwrap();
        assert_eq!(payload.session_id().as_deref(), Some("s1"));
        // subagent_id alias feeds the SubagentStart/Stop events.
        let payload: HookPayload =
            serde_json::from_str(r#"{"subagent_id":"sub-1","conversation_id":"c1"}"#).unwrap();
        assert_eq!(payload.subagent_id().as_deref(), Some("sub-1"));
        match parse_event("SubagentStart", &payload) {
            Some(HookEvent::SubagentStart { subagent_id }) => {
                assert_eq!(subagent_id.as_deref(), Some("sub-1"));
            }
            other => panic!("unexpected event: {other:?}"),
        }
    }
}

/// Claude/Codex route: the UserPromptSubmit hook command pipes this
/// response's body to stdout, so it must be empty or the injected
/// instruction — never diagnostic JSON.
async fn receive_injectable_hook(
    State(state): State<Arc<HookServerState>>,
    Query(query): Query<HookQuery>,
    headers: HeaderMap,
    body: String,
) -> (StatusCode, String) {
    receive_hook(true, state, query, headers, body).await
}

/// Cursor route: every hook command answers cursor with its own gating JSON
/// and discards this body, so the `{"ok": ...}` diagnostics stay.
async fn receive_plain_hook(
    State(state): State<Arc<HookServerState>>,
    Query(query): Query<HookQuery>,
    headers: HeaderMap,
    body: String,
) -> (StatusCode, String) {
    receive_hook(false, state, query, headers, body).await
}

async fn receive_hook(
    inject_capable: bool,
    state: Arc<HookServerState>,
    query: HookQuery,
    headers: HeaderMap,
    body: String,
) -> (StatusCode, String) {
    // On this path the response body reaches the model's context, so every
    // outcome (auth failure included) must answer with empty-or-instruction.
    let injectable = inject_capable && query.hook_event == "UserPromptSubmit";
    let quiet_or = |status: StatusCode, diag: &str| {
        let body = if injectable {
            String::new()
        } else {
            diag.to_string()
        };
        (status, body)
    };

    let authorized = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(|presented| presented.as_bytes().ct_eq(state.token.as_bytes()).into())
        .unwrap_or(false);
    if !authorized {
        return quiet_or(StatusCode::UNAUTHORIZED, r#"{"ok": false}"#);
    }

    // Parse failures still 200 — never fault a hook.
    let payload: HookPayload = serde_json::from_str(&body).unwrap_or_default();
    let Some(event) = parse_event(&query.hook_event, &payload) else {
        return quiet_or(StatusCode::OK, r#"{"ok": false}"#);
    };
    let agent_id = AgentId(query.agent_id.clone());
    tracing::debug!(agent = %agent_id, event = ?event, "hook received");
    // A subagent's tool traffic reports the payload's cwd too, but that is
    // the Task's position, not the session's — an isolated subagent working
    // in a scratch checkout must never drag the row out from under the
    // conversation. Only foreground payloads carry a cwd onward.
    let cwd = payload.cwd().filter(|_| payload.subagent_id().is_none());
    let _ = state
        .tx
        .send(HookDelivery {
            agent_id: agent_id.clone(),
            event,
            session_id: payload.session_id(),
            cwd,
        })
        .await;

    if injectable {
        // Prompt submitted on a still-untitled session: hand the CLI the
        // titling instruction. Unknown ids (prewarm, stale env) and store
        // errors degrade to no injection.
        let inject = state
            .store
            .agent_auto_title_pending(&agent_id)
            .unwrap_or(false);
        let body = if inject {
            auto_title_injection()
        } else {
            String::new()
        };
        return (StatusCode::OK, body);
    }
    (StatusCode::OK, r#"{"ok": true}"#.to_string())
}
