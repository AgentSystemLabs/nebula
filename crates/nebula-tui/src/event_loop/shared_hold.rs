//! The SHARED HOLD: a session about to be started in, or moved into, a
//! linked worktree that already has a live session waits for a yes first.
//! Two sessions in one checkout share its files, its index and its branch:
//! a `git switch` in one moves the other's next commit onto a branch it
//! never chose, and nothing on its card says so. A session that has
//! finished its turn is still live — it picks up again at the next prompt —
//! so it counts.
//!
//! It works on the requests an input event put out rather than on the keys
//! that made them: a quick prompt, a preset, `m` and a card dragged onto a
//! band all end in one `CreateAgent` or `MoveAgent`, and a PR session in a
//! `CreatePrAgent` aimed at its head branch's worktree, so every way in is
//! covered by one check. The request is taken out of the batch whole, its
//! `req_id` and intent already registered, and sent as it was on a yes,
//! with the warm-slot refill a create pushes behind itself. Any other way
//! out of the dialog cancels it as the DAEMON's refusal would, through the
//! same error path: the stand-in card comes down, a move is put back, and a
//! quick prompt reopens with its text. `nebula spawn` asks the same
//! question in its own way (`--same-worktree`).
//!
//! Only a linked worktree is asked about: the project's root checkout is
//! where several sessions side by side is the everyday way to work. The
//! CONFIRM SHARED WORKTREE setting turns it off.

use super::*;
use crate::app::SharedHold;
use nebula_core::Worktree;

/// The worktree a start or move is aimed at, and the session being moved.
fn aim<'a>(app: &'a App, req: &'a ClientRequest) -> Option<(&'a Worktree, Option<&'a AgentId>)> {
    match req {
        ClientRequest::CreateAgent { worktree, .. } => {
            Some((app.tree.worktrees.iter().find(|w| &w.id == worktree)?, None))
        }
        ClientRequest::MoveAgent { worktree, id, .. } => Some((
            app.tree.worktrees.iter().find(|w| &w.id == worktree)?,
            Some(id),
        )),
        // The DAEMON runs every PR SESSION of one pull request in the
        // linked worktree on its head branch, cut on the first.
        ClientRequest::CreatePrAgent { project, head, .. } => Some((
            app.tree
                .worktrees
                .iter()
                .find(|w| &w.project_id == project && &w.branch == head && !w.is_main)?,
            None,
        )),
        _ => None,
    }
}

/// The live sessions already in linked worktree `worktree`, `moving` left
/// out; empty for the root checkout.
pub(crate) fn live_in(app: &App, worktree: &Worktree, moving: Option<&AgentId>) -> Vec<String> {
    // The root checkout is where several sessions working side by side is
    // the everyday way in: only a linked worktree, made for one piece of
    // work, is asked about.
    if worktree.is_main {
        return Vec::new();
    }
    app.tree
        .agents
        .iter()
        .filter(|a| a.worktree_id == worktree.id && a.alive && !a.archived && Some(&a.id) != moving)
        .map(|a| a.name.clone())
        .collect()
}

/// What the hold is guarding against: whether it is a move, the target
/// checkout's branch, and the live sessions already in it.
fn held_target(app: &App, req: &ClientRequest) -> Option<(bool, String, Vec<String>)> {
    let (w, moving) = aim(app, req)?;
    let live = live_in(app, w, moving);
    if live.is_empty() {
        return None;
    }
    let is_move = matches!(req, ClientRequest::MoveAgent { .. });
    Some((is_move, w.branch.clone(), live))
}

/// Whether a move of `id` into `worktree` would be held: a card drop asks
/// this so the move is asked about once, not twice.
pub(crate) fn would_hold_move(app: &App, id: &AgentId, worktree: &WorktreeId) -> bool {
    app.tree
        .worktrees
        .iter()
        .find(|w| &w.id == worktree)
        .is_some_and(|w| !live_in(app, w, Some(id)).is_empty())
        && crate::config::Config::load().confirm_shared_worktree
}

fn req_id_of(req: &ClientRequest) -> Option<u64> {
    match req {
        ClientRequest::CreateAgent { req_id, .. }
        | ClientRequest::CreatePrAgent { req_id, .. }
        | ClientRequest::MoveAgent { req_id, .. } => Some(*req_id),
        _ => None,
    }
}

/// `"a"`, `"a and b"`, `"a, b and 2 more"`.
fn names(live: &[String]) -> String {
    match live {
        [] => String::new(),
        [one] => one.clone(),
        [a, b] => format!("{a} and {b}"),
        [a, b, rest @ ..] => format!("{a}, {b} and {} more", rest.len()),
    }
}

fn ask(app: &mut App, req: &ClientRequest) {
    let Some((moving, branch, live)) = held_target(app, req) else {
        return;
    };
    let (verb, question) = if moving {
        ("moved", "Move it there anyway?")
    } else {
        ("started", "Start it there anyway?")
    };
    let has = if live.len() == 1 { "is" } else { "are" };
    app.modals.overlay = Some(Overlay::Confirm(ConfirmDialog {
        title: "Shared worktree".into(),
        message: format!(
            "{} {has} live in {branch}. A session {verb} there shares its files, index and \
             branch: a branch switch in one moves the other's next commit, and a commit or \
             build can take in the other's half-done work. {question}",
            names(&live)
        ),
        action: PendingAction::SendSharedHold,
        area: ratatui::layout::Rect::default(),
    }));
    app.chrome.dirty = true;
}

/// After an input event. A hold whose dialog the user closed without a yes
/// is cancelled; one whose dialog something else covered waits, and asks
/// again once that is gone. Then a start or move this event aimed at a
/// worktree with a live session is held behind a new dialog. A request let
/// through by a yes is never held again.
pub(crate) fn hold_for_shared_worktree(app: &mut App, out: &mut Vec<ClientRequest>) {
    let asking = matches!(
        &app.modals.overlay,
        Some(Overlay::Confirm(c)) if c.action == PendingAction::SendSharedHold
    );
    if !asking {
        let covered = app.modals.overlay.is_some();
        match app.requests.shared_hold.as_mut() {
            Some(hold) if covered => hold.parked = true,
            Some(hold) if hold.parked => {
                hold.parked = false;
                let req = hold.req.clone();
                ask(app, &req);
            }
            Some(_) => {
                let held = app.requests.shared_hold.take().expect("matched Some");
                cancel(app, &held.req, out);
            }
            None => {}
        }
    }
    if app.requests.shared_hold.is_some() {
        return;
    }
    let allowed = app.requests.shared_allowed.take();
    let Some(at) = out
        .iter()
        .position(|req| req_id_of(req).is_some() && req_id_of(req) != allowed)
    else {
        return;
    };
    // The cheap test first: this runs after every key and mouse move, and
    // the setting is read from disk.
    if held_target(app, &out[at]).is_none()
        || !crate::config::Config::load().confirm_shared_worktree
    {
        return;
    }
    let req = out.remove(at);
    // The warm-slot refill a create pushes right behind itself waits with
    // it: on a no it would boot a CLI for a launch that never ran.
    let mut with = Vec::new();
    if let ClientRequest::CreateAgent { worktree, .. } = &req {
        out.retain(|r| match r {
            ClientRequest::PrewarmAgent { worktree: w, .. } if w == worktree => {
                with.push(r.clone());
                false
            }
            _ => true,
        });
    }
    ask(app, &req);
    app.requests.shared_hold = Some(SharedHold {
        req,
        with,
        flash: app.chrome.flash.take(),
        parked: false,
    });
}

/// The dialog's yes: the held request goes out as it was built.
pub(crate) fn send_shared_hold(app: &mut App, out: &mut Vec<ClientRequest>) {
    if let Some(hold) = app.requests.shared_hold.take() {
        app.requests.shared_allowed = req_id_of(&hold.req);
        if hold.flash.is_some() {
            app.chrome.flash = hold.flash;
        }
        out.push(hold.req);
        out.extend(hold.with);
    }
}

/// Anything but a yes: unwound as the DAEMON's refusal of it would be.
fn cancel(app: &mut App, held: &ClientRequest, out: &mut Vec<ClientRequest>) {
    let message = match held_target(app, held) {
        Some((true, branch, _)) => format!("not moved: {branch} already has a live session"),
        Some((false, branch, _)) => format!("not started: {branch} already has a live session"),
        // The other sessions ended, or the row went, while the dialog was
        // up: still unwound, so no stand-in or intent outlives it.
        None => "not sent".into(),
    };
    handle_server_event(
        app,
        ServerEvent::Error {
            req_id: req_id_of(held),
            message,
        },
        out,
    );
}

#[cfg(test)]
mod tests {
    use super::super::tests::{hse, seed_tree, with_default_config};
    use super::{hold_for_shared_worktree, names, send_shared_hold, would_hold_move};
    use crate::app::{App, ConfirmDialog, Overlay, PendingAction};
    use nebula_core::{
        Agent, AgentId, AgentKind, AgentStatus, ClientRequest, Entity, ProjectId, ServerEvent,
        Worktree, WorktreeId,
    };

    /// `seed_tree`'s project with a linked worktree `feat` (w2) that
    /// `polish-nav` is live in, beside the root checkout's own session.
    fn shared_feat() -> App {
        let mut app = App::new();
        seed_tree(&mut app);
        hse(
            &mut app,
            ServerEvent::EntityUpserted {
                entity: Entity::Worktree(Worktree {
                    id: WorktreeId("w2".into()),
                    project_id: ProjectId("p1".into()),
                    path: "/tmp/demo-feat".into(),
                    branch: "feat".into(),
                    is_main: false,
                    sort_order: 1,
                }),
            },
        );
        hse(
            &mut app,
            ServerEvent::EntityUpserted {
                entity: Entity::Agent(Agent {
                    id: AgentId("a2".into()),
                    worktree_id: WorktreeId("w2".into()),
                    name: "polish-nav".into(),
                    status: AgentStatus::Finished,
                    archived: false,
                    archived_at: 0,
                    unseen: false,
                    kind: AgentKind::Claude,
                    custom_harness: None,
                    model: None,
                    effort: None,
                    session_id: None,
                    cloud_session_id: None,
                    sort_order: 0,
                    status_changed_at: 0,
                    alive: true,
                    issue_url: None,
                    role: nebula_core::AgentRole::Worker,
                    recent_prompts: Vec::new(),
                }),
            },
        );
        app
    }

    fn create(req_id: u64, worktree: &str) -> ClientRequest {
        ClientRequest::CreateAgent {
            req_id,
            worktree: WorktreeId(worktree.into()),
            name: "agent-1".into(),
            kind: AgentKind::Claude,
            custom_harness: None,
            model: None,
            effort: None,
            auto_title: true,
            cloud_prompt: None,
            starting_prompt: None,
            issue_url: None,
            role: nebula_core::AgentRole::Worker,
        }
    }

    fn prewarm(worktree: &str) -> ClientRequest {
        ClientRequest::PrewarmAgent {
            worktree: WorktreeId(worktree.into()),
            kind: AgentKind::Claude,
            model: None,
            effort: None,
        }
    }

    fn asking(app: &App) -> bool {
        matches!(&app.modals.overlay, Some(Overlay::Confirm(c)) if c.title == "Shared worktree")
    }

    /// A finished session is still live. The warm-slot refill a create
    /// pushes behind itself is held with it and goes out after it on a
    /// yes; a create into the root checkout, live session and all, is not
    /// held.
    #[test]
    fn a_create_waits_with_its_refill_and_the_root_is_never_asked_about() {
        with_default_config(|| {
            let mut app = shared_feat();
            let mut out = vec![create(7, "w2"), prewarm("w2"), prewarm("w1")];
            hold_for_shared_worktree(&mut app, &mut out);
            assert!(asking(&app));
            assert!(
                matches!(out.as_slice(), [ClientRequest::PrewarmAgent { worktree, .. }] if worktree.0 == "w1"),
                "only another worktree's refill goes: {out:?}"
            );
            app.modals.overlay = None;
            let mut out = Vec::new();
            send_shared_hold(&mut app, &mut out);
            assert!(matches!(
                out.as_slice(),
                [
                    ClientRequest::CreateAgent { req_id: 7, .. },
                    ClientRequest::PrewarmAgent { .. }
                ]
            ));
            hold_for_shared_worktree(&mut app, &mut out);
            assert_eq!(
                out.len(),
                2,
                "a request a yes let through is not held again"
            );

            let mut out = vec![create(8, "w1"), prewarm("w1")];
            hold_for_shared_worktree(&mut app, &mut out);
            assert_eq!(out.len(), 2);
            assert!(!asking(&app));
        });
    }

    /// A PR SESSION runs in the worktree on its head branch, so a second
    /// one there is asked about like any other start.
    #[test]
    fn a_pr_session_into_its_head_branchs_worktree_asks_first() {
        with_default_config(|| {
            let mut app = shared_feat();
            let mut out = vec![ClientRequest::CreatePrAgent {
                req_id: 9,
                project: ProjectId("p1".into()),
                name: "review".into(),
                kind: AgentKind::Claude,
                custom_harness: None,
                model: None,
                effort: None,
                auto_title: false,
                pr_url: "https://github.com/o/r/pull/1".into(),
                head: "feat".into(),
                starting_prompt: None,
            }];
            hold_for_shared_worktree(&mut app, &mut out);
            assert!(out.is_empty(), "held: {out:?}");
            assert!(asking(&app));
        });
    }

    /// A dialog something else covered (a refused launch's prompt coming
    /// back) is not read as a no: the hold waits, and asks again once
    /// the cover is gone. Only the dialog closed with nothing over it
    /// cancels.
    #[test]
    fn a_covered_dialog_asks_again_rather_than_cancelling() {
        with_default_config(|| {
            let mut app = shared_feat();
            let mut out = vec![create(7, "w2")];
            hold_for_shared_worktree(&mut app, &mut out);
            app.modals.overlay = Some(Overlay::Confirm(ConfirmDialog {
                title: "Something else".into(),
                message: String::new(),
                action: PendingAction::Quit,
                area: ratatui::layout::Rect::default(),
            }));
            let mut out = Vec::new();
            hold_for_shared_worktree(&mut app, &mut out);
            assert!(app.requests.shared_hold.is_some(), "still held");
            app.modals.overlay = None;
            hold_for_shared_worktree(&mut app, &mut out);
            assert!(asking(&app), "asked again");
            assert!(out.is_empty());

            app.modals.overlay = None;
            hold_for_shared_worktree(&mut app, &mut out);
            assert!(app.requests.shared_hold.is_none(), "a plain close cancels");
            assert_eq!(
                app.chrome.flash.as_deref(),
                Some("not started: feat already has a live session")
            );
        });
    }

    /// A card drop asks the shared question in place of its own, so a
    /// drag into a shared checkout asks once.
    #[test]
    fn a_drop_into_a_shared_checkout_is_the_holds_to_ask() {
        with_default_config(|| {
            let app = shared_feat();
            assert!(would_hold_move(
                &app,
                &AgentId("a1".into()),
                &WorktreeId("w2".into())
            ));
            assert!(!would_hold_move(
                &app,
                &AgentId("a2".into()),
                &WorktreeId("w2".into())
            ));
            assert!(!would_hold_move(
                &app,
                &AgentId("a2".into()),
                &WorktreeId("w1".into())
            ));
        });
    }

    #[test]
    fn names_list_the_first_two_and_count_the_rest() {
        let s = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();
        assert_eq!(names(&s(&["a"])), "a");
        assert_eq!(names(&s(&["a", "b"])), "a and b");
        assert_eq!(names(&s(&["a", "b", "c", "d"])), "a, b and 2 more");
    }
}
