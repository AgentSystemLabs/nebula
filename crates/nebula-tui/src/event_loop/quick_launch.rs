//! The QUICK PROMPT's last step: the box's `QuickLaunch` plus the typed
//! text, turned into the create the DAEMON sees. A launch into a selected
//! WORKTREE is one `CreateAgent`. A launch into a worktree that does not
//! exist yet (`QuickTarget::NewWorktree` — `p` on the WORKTREES PANEL) is
//! a `CreateWorktree` first, the launch riding its PENDING INTENT, and
//! the same `CreateAgent` once the Ack names the checkout: retargeted at
//! it, the cursor moved onto its row, FOCUS left on the panel `p` was
//! pressed in. Both rows are on screen from the moment Enter is pressed —
//! stand-ins (`placeholder`) that the Acks turn into the real rows. A
//! launch for a pull request (`QuickLaunch::pr` — `e` on an OPEN PRS
//! row, through the preset picker) is a `CreatePrAgent` instead: the
//! draft carries the PR and `create_agent` addresses it to the PROJECT,
//! whose checkout of the PR's head branch the DAEMON finds or cuts.

use super::{
    create_agent, placeholder, remember_launch, schedule_prewarm, select_worktree_by_id, send_with,
    AgentLaunchDraft,
};
use crate::app::{App, PendingIntent, PlaceholderRows, PromptKind};
use crate::quick_prompt::{QuickLaunch, QuickTarget};
use nebula_core::{AgentId, ClientRequest, WorktreeId};

/// Enter in the box, with `text` already sized — and non-empty, unless
/// the box `launches_empty` (one an AGENT PRESET is on, sent as it is).
pub(super) fn submit(
    app: &mut App,
    launch: QuickLaunch,
    text: String,
    out: &mut Vec<ClientRequest>,
) {
    // REMEMBER HARNESS (Settings → Experimental): a box fired on a harness
    // — or a MODEL / EFFORT — picked through `Tab` makes that the next
    // launch's default. A preset's harness is the preset's own, not a
    // change of default, so a preset launch leaves the rows alone.
    if launch.preset.is_none() {
        remember_launch(
            app,
            launch.kind,
            launch.custom.as_deref(),
            launch.model.as_deref(),
            launch.effort.as_deref(),
        );
    }
    match launch.target.clone() {
        QuickTarget::Worktree(worktree) => {
            create_agent(app, draft(launch, worktree, text, None), out)
        }
        QuickTarget::NewWorktree { project, branch } => {
            // The rows first, so the panels never wait on git.
            let placeholder = placeholder::stage(
                app,
                project.clone(),
                branch.clone(),
                launch.kind,
                launch.custom.clone(),
                launch.model.clone(),
                launch.effort.clone(),
                out,
            );

            // `base: None` is the DAEMON's `worktree_base_branch` SETTING,
            // else its fetched `origin/HEAD` (`git::add_worktree_off_default`)
            // — never this checkout's HEAD.
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
/// Without `follow` — the user navigated away while the checkout was cut
/// (`App::left_behind`) — the launch still goes out, but no cursor moves
/// back onto the row, now or at the session's own Ack.
pub(super) fn launch_in_created_worktree(
    app: &mut App,
    mut launch: QuickLaunch,
    text: String,
    placeholder: PlaceholderRows,
    worktree: WorktreeId,
    follow: bool,
    out: &mut Vec<ClientRequest>,
) {
    // The stand-in checkout becomes the real one — its session row moves
    // under it — before anything is selected or sent by the real id.
    placeholder::resolve_worktree(app, &placeholder.worktree, &worktree);
    if follow {
        // The new row is the context every later `p` / `n` runs in, so the
        // cursor moves onto it — but FOCUS stays on the panel `p` was
        // pressed in, as every QUICK PROMPT launch leaves it
        // (`quick_prompt_focus` decides the pane, in `create_agent`'s
        // intent, not here).
        let focus = app.focus;
        if !select_worktree_by_id(app, &worktree, out) {
            app.select_worktree_when_seen = Some(worktree.clone());
        }
        app.focus = focus;
    }
    // The cursor was already on the row, so the select above did not arm
    // the prewarm a fresh landing would have; the checkout is real now.
    // A cursor the user took elsewhere warms nothing here.
    if app.selected_worktree().is_some_and(|w| w.id == worktree) {
        schedule_prewarm(app);
    }
    launch.target = QuickTarget::Worktree(worktree.clone());
    create_agent(
        app,
        AgentLaunchDraft {
            follow,
            ..draft(launch, worktree, text, Some(placeholder.agent))
        },
        out,
    );
}

/// The create itself, the same on both routes. An empty name opts the
/// row into AUTO-TITLE, so the session names itself from the very prompt
/// that started it, and the box comes back with the text should the
/// DAEMON refuse.
pub(super) fn draft(
    launch: QuickLaunch,
    worktree: WorktreeId,
    text: String,
    placeholder: Option<AgentId>,
) -> AgentLaunchDraft {
    // The box is the one launch that stays out of the way by default:
    // `p`, type, Enter, keep working. A picker-walked launch (`n`) takes
    // the pane instead.
    let focus_pane = crate::config::Config::load().quick_prompt_focus;
    let base = AgentLaunchDraft::new(
        worktree,
        launch.kind,
        launch.model.clone(),
        launch.effort.clone(),
    );
    AgentLaunchDraft {
        custom: launch.custom.clone(),
        // Sized in `submit_prompt`, with the task — composing cannot fail.
        // An empty box (`launches_empty`) sends a preset's prefix + postfix
        // alone; with nothing to wrap it either, there is no first prompt,
        // the CLI's own input is it.
        starting_prompt: Some(launch.compose(&text)).filter(|prompt| !prompt.is_empty()),
        // An ISSUE SESSION's context, persisted by the DAEMON with the row.
        issue_url: launch.issue.as_ref().map(|issue| issue.url.clone()),
        // A PR SESSION's: the create goes to the PROJECT as a
        // `CreatePrAgent`, and `worktree` only names which.
        pr: launch.pr.clone(),
        reopen_on_error: Some((PromptKind::QuickPrompt(launch), text)),
        focus_pane,
        placeholder,
        ..base
    }
}
