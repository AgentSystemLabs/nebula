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
use axum::extract::{Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::routing::post;
use axum::{Json, Router};
use nebula_core::AgentId;
use serde::Deserialize;
use std::sync::Arc;
use subtle::ConstantTimeEq;
use tokio::sync::mpsc;

#[derive(Clone)]
pub struct HookEnv {
    pub port: u16,
    pub token: String,
}

pub struct HookServerState {
    token: String,
    tx: mpsc::Sender<HookDelivery>,
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
        self.session_id.clone().or_else(|| self.conversation_id.clone())
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
pub async fn start_hook_server() -> anyhow::Result<(HookEnv, mpsc::Receiver<HookDelivery>)> {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let port = listener.local_addr()?.port();
    let token = generate_token();
    let (tx, rx) = mpsc::channel(256);

    let state = Arc::new(HookServerState {
        token: token.clone(),
        tx,
    });
    let app = Router::new()
        .route("/api/hooks/claude", post(receive_hook))
        .route("/api/hooks/codex", post(receive_hook))
        .route("/api/hooks/cursor", post(receive_hook))
        .with_state(state);

    tokio::spawn(async move {
        if let Err(e) = axum::serve(listener, app).await {
            tracing::error!(error = %e, "hook http server died");
        }
    });

    Ok((HookEnv { port, token }, rx))
}

fn generate_token() -> String {
    use rand::RngCore;
    let mut bytes = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut bytes);
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

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

async fn receive_hook(
    State(state): State<Arc<HookServerState>>,
    Query(query): Query<HookQuery>,
    headers: HeaderMap,
    body: String,
) -> (StatusCode, Json<serde_json::Value>) {
    let authorized = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(|presented| presented.as_bytes().ct_eq(state.token.as_bytes()).into())
        .unwrap_or(false);
    if !authorized {
        return (
            StatusCode::UNAUTHORIZED,
            Json(serde_json::json!({"ok": false})),
        );
    }

    // Parse failures still 200 — never fault a hook.
    let payload: HookPayload = serde_json::from_str(&body).unwrap_or_default();
    let Some(event) = parse_event(&query.hook_event, &payload) else {
        return (StatusCode::OK, Json(serde_json::json!({"ok": false})));
    };
    let agent_id = AgentId(query.agent_id);
    tracing::debug!(agent = %agent_id, event = ?event, "hook received");
    let _ = state
        .tx
        .send(HookDelivery {
            agent_id,
            event,
            session_id: payload.session_id(),
            cwd: payload.cwd(),
        })
        .await;
    (StatusCode::OK, Json(serde_json::json!({"ok": true})))
}
