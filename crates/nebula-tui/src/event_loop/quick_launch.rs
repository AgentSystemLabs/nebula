//! The QUICK PROMPT's last step: the box's `QuickLaunch` plus the typed
//! text, turned into the create the DAEMON sees. A launch into a selected
//! WORKTREE is one `CreateAgent`. A launch into a worktree that does not
//! exist yet (`QuickTarget::NewWorktree` — `p` on the WORKTREES PANEL with
//! the `hide_root_worktree` SETTING on) is a `CreateWorktree` first, the
//! launch riding its PENDING INTENT, and the same `CreateAgent` once the
//! Ack names the checkout: retargeted at it, the cursor moved onto its
//! row, FOCUS left on the panel `p` was pressed in. Both rows are on
//! screen from the moment Enter is pressed — stand-ins (`placeholder`)
//! that the Acks turn into the real rows.

use super::{
    create_agent, placeholder, schedule_prewarm, select_worktree_by_id, send_with, AgentLaunchDraft,
};
use crate::app::{App, PendingIntent, PlaceholderRows, PromptKind};
use crate::quick_prompt::{QuickLaunch, QuickTarget};
use nebula_core::{AgentId, ClientRequest, WorktreeId};

/// Enter in the box, with `text` already sized and non-empty.
pub(super) fn submit(
    app: &mut App,
    launch: QuickLaunch,
    text: String,
    out: &mut Vec<ClientRequest>,
) {
    match launch.target.clone() {
        QuickTarget::Worktree(worktree) => {
            create_agent(app, draft(launch, worktree, text, None), out)
        }
        QuickTarget::NewWorktree { project, branch } => {
            // The rows first, so the panels never wait on git.
            let placeholder =
                placeholder::stage(app, project.clone(), branch.clone(), &launch, out);
            // `base: None` is the DAEMON's fetched `origin/HEAD`
            // (`git::add_worktree_off_default`), never this checkout's HEAD.
            send_with(
                app,
                out,
                PendingIntent::LaunchInCreatedWorktree {
                    launch,
                    text,
                    placeholder,
                },
                |req_id| ClientRequest::CreateWorktree {
                    req_id,
                    project,
                    branch,
                    base: None,
                },
            );
        }
    }
}

/// The Ack for that `CreateWorktree`: `worktree` exists now, launch there.
pub(super) fn launch_in_created_worktree(
    app: &mut App,
    mut launch: QuickLaunch,
    text: String,
    placeholder: PlaceholderRows,
    worktree: WorktreeId,
    out: &mut Vec<ClientRequest>,
) {
    // The stand-in checkout becomes the real one — its session row moves
    // under it — before anything is selected or sent by the real id.
    placeholder::resolve_worktree(app, &placeholder, &worktree);
    // The new row is the context every later `p` / `n` runs in, so the
    // cursor moves onto it — but FOCUS stays on the panel `p` was pressed
    // in, as every QUICK PROMPT launch leaves it (`quick_prompt_focus`
    // decides the pane, in `create_agent`'s intent, not here).
    let focus = app.focus;
    if !select_worktree_by_id(app, &worktree, out) {
        app.select_worktree_when_seen = Some(worktree.clone());
    }
    app.focus = focus;
    // The cursor was already on the row, so the select above did not arm
    // the prewarm a fresh landing would have; the checkout is real now.
    schedule_prewarm(app);
    launch.target = QuickTarget::Worktree(worktree.clone());
    create_agent(
        app,
        draft(launch, worktree, text, Some(placeholder.agent)),
        out,
    );
}

/// The create itself, the same on both routes. An empty name opts the
/// row into AUTO-TITLE, so the session names itself from the very prompt
/// that started it, and the box comes back with the text should the
/// DAEMON refuse.
fn draft(
    launch: QuickLaunch,
    worktree: WorktreeId,
    text: String,
    placeholder: Option<AgentId>,
) -> AgentLaunchDraft {
    AgentLaunchDraft {
        worktree,
        kind: launch.kind,
        model: launch.model.clone(),
        effort: launch.effort.clone(),
        name: String::new(),
        cloud_prompt: None,
        // Sized in `submit_prompt`, with the task — composing cannot fail.
        starting_prompt: Some(launch.compose(&text)),
        reopen_on_error: Some((PromptKind::QuickPrompt(launch), text)),
        pr: None,
        // The QUICK PROMPT is the one launch that stays out of the way by
        // default: `p`, type, Enter, keep working.
        focus_pane: crate::config::Config::load().quick_prompt_focus,
        placeholder,
    }
}
