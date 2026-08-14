//! Agent-CLI hook receiver: a loopback-only HTTP endpoint the shell hook
//! one-liners POST to (`/api/hooks/claude` and `/api/hooks/codex` — codex
//! mirrors Claude's hook events and payload shape, so one handler serves
//! both). Fail-soft on both sides — a malformed payload still gets a 200 so
//! a broken hook never faults the user's agent turn.

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
    tx: mpsc::Sender<(AgentId, HookEvent, Option<String>)>,
}

/// Permissive payload: every field optional, unknown fields ignored. Hook
/// payload shapes are Claude-version-dependent; drift must degrade to
/// "status stops updating", never to an error.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct HookPayload {
    pub hook_event_name: Option<String>,
    pub session_id: Option<String>,
    pub notification_type: Option<String>,
    pub tool_name: Option<String>,
    pub agent_id: Option<String>,
    pub source: Option<String>,
    pub exit_code: Option<i32>,
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
            subagent_id: payload.agent_id.clone(),
        },
        "SubagentStop" => HookEvent::SubagentStop {
            subagent_id: payload.agent_id.clone(),
        },
        _ => return None,
    })
}

/// Bind 127.0.0.1:0 and serve the hook route. Returns the env (port + fresh
/// bearer token) and the receiving end of the event pipe.
pub async fn start_hook_server() -> anyhow::Result<(
    HookEnv,
    mpsc::Receiver<(AgentId, HookEvent, Option<String>)>,
)> {
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
    let _ = state.tx.send((agent_id, event, payload.session_id)).await;
    (StatusCode::OK, Json(serde_json::json!({"ok": true})))
}
