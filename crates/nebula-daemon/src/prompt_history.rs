//! RECENT PROMPTS — the last few prompts typed into a session, kept on
//! its row so the SESSIONS PANEL can say what each session was last asked
//! to do, not only what it was named after its first prompt.
//!
//! The `UserPromptSubmit` hook payload carries the prompt on every CLI
//! nebula manages (Claude and Codex as `prompt`, Cursor's
//! `beforeSubmitPrompt` the same, pi through its managed extension). The
//! hook receiver condenses it here — one line, whitespace collapsed,
//! clipped past [`MAX_PROMPT_CHARS`] — before it crosses the daemon's
//! channel, so a pasted file never rides the wire whole. The store keeps
//! the newest [`RECENT_PROMPTS_KEPT`] per session and the row's
//! `recent_prompts` reaches every TUI as an ordinary upsert.
//!
//! Prompts nebula itself composes — the PR SESSION scope, the `nebula
//! worktree` relocation notice, anything opening with `[nebula]` — are
//! not the user's and are left out.

use std::sync::Arc;

use nebula_core::{AgentId, PromptEntry};

use crate::registry::Daemon;

/// The longest condensed prompt the store keeps. The SESSIONS PANEL shows
/// far less, but a wider terminal or a later reader should see the whole
/// sentence.
pub const MAX_PROMPT_CHARS: usize = 200;

/// The marker every nebula-authored prompt opens with.
const NEBULA_PREFIX: &str = "[nebula]";

/// One line of the prompt: control characters become spaces (an escape
/// sequence pasted into a prompt must not reach the panel — the same rule
/// `sanitize_title` applies to the row's name), whitespace runs (newlines
/// included) collapse to single spaces, and the result is clipped with an
/// ellipsis past [`MAX_PROMPT_CHARS`]. `None` for a blank prompt or one
/// nebula wrote itself.
pub fn condense(raw: &str) -> Option<String> {
    let cleaned: String = raw
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    let mut text = String::new();
    for word in cleaned.split_whitespace() {
        if !text.is_empty() {
            text.push(' ');
        }
        text.push_str(word);
    }
    if text.is_empty() || text.starts_with(NEBULA_PREFIX) {
        return None;
    }
    if text.chars().count() > MAX_PROMPT_CHARS {
        let mut clipped: String = text.chars().take(MAX_PROMPT_CHARS - 1).collect();
        clipped.push('…');
        return Some(clipped);
    }
    Some(text)
}

impl Daemon {
    /// Append a condensed prompt to the session's history and push the
    /// row to every subscriber. A row that is gone (or was never there —
    /// a prewarm spare's id) is silently nothing to record.
    pub fn record_prompt(self: &Arc<Self>, id: &AgentId, text: String) {
        let entry = PromptEntry {
            text,
            submitted_at: crate::registry::epoch_ms(),
        };
        match self.store.push_prompt(id, &entry) {
            Ok(true) => self.try_broadcast_agent(id),
            Ok(false) => {}
            Err(e) => tracing::warn!(agent = %id, error = %e, "prompt not recorded"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn condense_makes_one_trimmed_line() {
        assert_eq!(
            condense("  fix the\n\n   login   redirect\t\n").as_deref(),
            Some("fix the login redirect")
        );
        assert_eq!(condense("one"), Some("one".into()));
    }

    #[test]
    fn condense_drops_blank_and_nebula_authored_prompts() {
        assert_eq!(condense(""), None);
        assert_eq!(condense("  \n\t "), None);
        assert_eq!(
            condense("[nebula] This session now runs inside the worktree `x`"),
            None
        );
        assert_eq!(condense("\n  [nebula] anything"), None);
        // Only the opening marker counts: a user mentioning nebula stays.
        assert_eq!(
            condense("why does [nebula] show a red dot").as_deref(),
            Some("why does [nebula] show a red dot")
        );
    }

    /// A terminal escape typed or pasted into a prompt is drawn as text,
    /// never sent to the terminal: control characters become spaces before
    /// the whitespace collapse, as the row's title already has them.
    #[test]
    fn condense_turns_control_characters_into_spaces() {
        assert_eq!(
            condense("set the\x1b]0;title\x07 tab").as_deref(),
            Some("set the ]0;title tab")
        );
        assert_eq!(condense("\x1b\x07\r"), None);
    }

    #[test]
    fn condense_clips_long_prompts_on_a_char_boundary() {
        let long = "é".repeat(MAX_PROMPT_CHARS + 50);
        let out = condense(&long).unwrap();
        assert_eq!(out.chars().count(), MAX_PROMPT_CHARS);
        assert!(out.ends_with('…'));
        let exact = "x".repeat(MAX_PROMPT_CHARS);
        assert_eq!(condense(&exact).as_deref(), Some(exact.as_str()));
    }

    /// The whole path past the hook: a recorded prompt lands on the row
    /// and reaches subscribers as the row's upsert; an unknown id is
    /// silent.
    #[tokio::test]
    async fn record_prompt_upserts_the_row_with_its_history() {
        use crate::hooks::HookEnv;
        use crate::store::Store;
        use nebula_core::{
            Agent, AgentKind, AgentStatus, Entity, Project, ProjectId, ServerEvent, Worktree,
            WorktreeId,
        };
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
                sort_order: 0,
            })
            .unwrap();
        let id = AgentId("a1".into());
        store
            .insert_agent(&Agent {
                id: id.clone(),
                worktree_id: WorktreeId("w1".into()),
                name: "agent-1".into(),
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
                recent_prompts: Vec::new(),
            })
            .unwrap();
        let daemon = Daemon::new(
            store,
            HookEnv {
                port: 0,
                token: String::new(),
            },
        );
        let mut events = daemon.events.subscribe();

        daemon.record_prompt(&id, "fix the login redirect".into());
        match events.try_recv() {
            Ok(ServerEvent::EntityUpserted {
                entity: Entity::Agent(a),
            }) => {
                assert_eq!(a.id, id);
                assert_eq!(a.recent_prompts.len(), 1);
                assert_eq!(a.recent_prompts[0].text, "fix the login redirect");
                assert!(a.recent_prompts[0].submitted_at > 0);
            }
            other => panic!("expected the row's upsert, got {other:?}"),
        }

        daemon.record_prompt(&AgentId("ghost".into()), "nothing".into());
        assert!(events.try_recv().is_err(), "an unknown id is silent");
    }
}
