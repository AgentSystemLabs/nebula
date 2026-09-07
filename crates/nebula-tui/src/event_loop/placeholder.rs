//! Stand-in rows for a QUICK PROMPT into a WORKTREE that does not exist
//! yet (`p` on the WORKTREES PANEL with `hide_root_worktree` on). The
//! DAEMON's `CreateWorktree` is a fetch plus `git worktree add`, and the
//! `CreateAgent` after it a CLI spawn — seconds, during which the panels
//! used to show nothing. Now Enter puts both rows up at once, under ids
//! this client made, and the cursors land on them exactly as the real
//! create would leave them: the Ack of each request turns its stand-in
//! into the real row, and an Error takes the stand-ins down and hands the
//! box back.
//!
//! The rows are ordinary `Worktree` / `Agent` entries in `app.tree` — every
//! list, sort and count treats them as it would the real ones. What marks
//! them is the PENDING INTENT that carries their ids
//! (`App::is_placeholder_worktree` / `is_placeholder_agent`): the intent
//! is removed by the very Ack or Error that resolves them, so nothing has
//! to be kept in sync. Nothing ever reaches the DAEMON under a stand-in
//! id — `send_attach`, `create_agent`, `create_terminal`, the prewarm,
//! the delete and the pane's Input all check first.

use super::{
    pane_size, reconcile_selection, release_attachment, remove_worktree_rows, restore_context,
    select_worktree_by_id, selection_snapshot,
};
use crate::app::{now_ms, App, AttachedTerm, PlaceholderRows};
use crate::quick_prompt::QuickLaunch;
use nebula_core::{
    Agent, AgentId, AgentStatus, ClientRequest, ProjectId, SessionRef, Worktree, WorktreeId,
};

/// Put the two rows up and land the cursors on them: the worktree row
/// selected (FOCUS kept where `p` was pressed, as every QUICK PROMPT
/// launch keeps it), its one session row selected, the pane showing
/// "starting…" for it. Returns the ids the intents carry.
pub(super) fn stage(
    app: &mut App,
    project: ProjectId,
    branch: String,
    launch: &QuickLaunch,
    out: &mut Vec<ClientRequest>,
) -> PlaceholderRows {
    let worktree = WorktreeId::generate();
    app.tree.worktrees.push(Worktree {
        id: worktree.clone(),
        project_id: project,
        // Unknown until the DAEMON's upsert names the checkout. Nothing
        // reads it meanwhile: the PR lookup skips a path that is not a
        // directory, and no launch or diff can open a stand-in.
        path: std::path::PathBuf::new(),
        branch,
        is_main: false,
        sort_order: 0,
    });
    let focus = app.focus;
    select_worktree_by_id(app, &worktree, out);
    // The name the DAEMON will give the real row: `default_session_name`
    // reads the selected worktree, which is the stand-in now (no
    // sessions), and `create_agent` takes the name off this row when the
    // create goes out, so the two agree.
    let name = app.default_session_name("agent");
    let agent = AgentId::generate();
    app.tree.agents.push(Agent {
        id: agent.clone(),
        worktree_id: worktree.clone(),
        name,
        status: AgentStatus::Fresh,
        archived: false,
        archived_at: 0,
        unseen: false,
        kind: launch.kind,
        model: launch.model.clone(),
        effort: launch.effort.clone(),
        session_id: None,
        cloud_session_id: None,
        sort_order: 0,
        // The DAEMON stamps a created row with its clock, which puts the
        // checkout at the top of RECENCY ORDER; the same stamp here means
        // the row does not jump when the real one replaces it.
        status_changed_at: now_ms(),
        alive: false,
        cloud_mirroring: false,
    });
    // That stamp just moved the row to the top: re-seat the cursor on it.
    if let Some(i) = app
        .visible_worktrees()
        .iter()
        .position(|w| w.id == worktree)
    {
        app.sel_worktree = i;
    }
    app.sel_session = 0;
    // The pane shows the row as it would a session whose CLI is booting.
    // Built by hand rather than through `attach`: the intent that marks
    // the row a stand-in is allocated by the caller after this returns,
    // so `send_attach`'s check would not fire yet — and whatever the pane
    // held is let go, so no keystroke lands there through a stale hold.
    release_attachment(app, out);
    let (cols, rows) = pane_size(app);
    app.term_selection = None;
    app.term = Some(AttachedTerm::new(
        SessionRef::Agent(agent.clone()),
        cols,
        rows,
    ));
    app.focus = focus;
    app.dirty = true;
    PlaceholderRows { worktree, agent }
}

/// The `CreateWorktree` Ack: `real` is the checkout the DAEMON cut. Its
/// upsert usually lands just before the Ack, in which case the real row is
/// already in and the stand-in goes; if the Ack came first the stand-in
/// becomes the real row (same id) and the upsert overwrites it in place.
/// The stand-in session moves under the real checkout either way, so the
/// row keeps its place in RECENCY ORDER and its selection.
pub(super) fn resolve_worktree(app: &mut App, rows: &PlaceholderRows, real: &WorktreeId) {
    if app.tree.worktrees.iter().any(|w| &w.id == real) {
        app.tree.worktrees.retain(|w| w.id != rows.worktree);
    } else if let Some(w) = app
        .tree
        .worktrees
        .iter_mut()
        .find(|w| w.id == rows.worktree)
    {
        w.id = real.clone();
    }
    for a in app
        .tree
        .agents
        .iter_mut()
        .filter(|a| a.worktree_id == rows.worktree)
    {
        a.worktree_id = real.clone();
    }
    // The context memory `select_worktree_by_id` wrote for the stand-in
    // now describes the real checkout.
    if let Some(sref) = app.last_session_for_worktree.remove(&rows.worktree) {
        app.last_session_for_worktree.insert(real.clone(), sref);
    }
    for wid in app.last_worktree_for_project.values_mut() {
        if *wid == rows.worktree {
            *wid = real.clone();
        }
    }
    forget_worktree(app, &rows.worktree);
    app.dirty = true;
}

/// The `CreateAgent` Ack: `real` is the session the DAEMON made. Same
/// two orders as `resolve_worktree`: the upsert first and the stand-in
/// goes, or the Ack first and the stand-in takes the real id.
pub(super) fn resolve_agent(app: &mut App, placeholder: &AgentId, real: &AgentId) {
    if app.tree.agents.iter().any(|a| &a.id == real) {
        app.tree.agents.retain(|a| &a.id != placeholder);
    } else if let Some(a) = app.tree.agents.iter_mut().find(|a| &a.id == placeholder) {
        a.id = real.clone();
    }
    let stand_in = SessionRef::Agent(placeholder.clone());
    for sref in app.last_session_for_worktree.values_mut() {
        if *sref == stand_in {
            *sref = SessionRef::Agent(real.clone());
        }
    }
    app.dirty = true;
}

/// The `CreateWorktree` was refused: both rows go. A cursor still on the
/// stand-in goes back to the row it was on before `p` — `remember_context`
/// recorded that row when the stand-in took the cursor — and a cursor the
/// user moved elsewhere meanwhile stays put.
pub(super) fn discard(app: &mut App, rows: &PlaceholderRows, out: &mut Vec<ClientRequest>) {
    let on_it = app
        .selected_worktree()
        .is_some_and(|w| w.id == rows.worktree);
    blank_pane_if_showing(app, &rows.agent);
    let before = selection_snapshot(app);
    let _ = remove_worktree_rows(app, &rows.worktree);
    app.last_session_for_worktree.remove(&rows.worktree);
    app.last_worktree_for_project
        .retain(|_, wid| *wid != rows.worktree);
    forget_worktree(app, &rows.worktree);
    if on_it {
        restore_context(app, out);
    } else {
        reconcile_selection(app, before, out);
    }
    app.dirty = true;
}

/// The `CreateAgent` was refused after the checkout was cut: the session
/// row goes, the worktree row stays — it is real.
pub(super) fn discard_agent(app: &mut App, placeholder: &AgentId, out: &mut Vec<ClientRequest>) {
    blank_pane_if_showing(app, placeholder);
    let before = selection_snapshot(app);
    app.tree.agents.retain(|a| &a.id != placeholder);
    let stand_in = SessionRef::Agent(placeholder.clone());
    app.last_session_for_worktree
        .retain(|_, sref| *sref != stand_in);
    reconcile_selection(app, before, out);
    app.dirty = true;
}

/// A pane showing the stand-in has nothing attached behind it, so it is
/// blanked here rather than detached: a Detach for a session the DAEMON
/// never had would be noise.
fn blank_pane_if_showing(app: &mut App, placeholder: &AgentId) {
    let showing = app
        .term
        .as_ref()
        .is_some_and(|t| t.sref == SessionRef::Agent(placeholder.clone()));
    if showing {
        app.term = None;
        app.term_locked = false;
    }
}

/// Drop the per-worktree PR bookkeeping keyed by a stand-in id.
fn forget_worktree(app: &mut App, id: &WorktreeId) {
    app.pull_requests.remove(id);
    app.pr_recheck.remove(id);
}

#[cfg(test)]
mod tests {
    use super::super::tests::{
        buffer_text, hse, press, seed_feat_worktree, seed_tree, with_default_config,
        worktree_branches,
    };
    use super::super::{fire_pending_prewarm, handle_server_event, paste_into_overlay};
    use crate::app::{App, Focus, Overlay, PendingIntent, PlaceholderRows, PromptKind};
    use crate::quick_prompt::QuickTarget;
    use crate::ui;
    use crossterm::event::{KeyCode, KeyModifiers};
    use nebula_core::{
        Agent, AgentId, AgentKind, AgentStatus, ClientRequest, Entity, EntityId, ServerEvent,
        SessionRef, WorktreeId,
    };
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    /// `p` on the WORKTREES PANEL with the root hidden and the cursor on
    /// `feat`, "Fix auth" typed, Enter pressed. Returns the branch the box
    /// offered, the stand-in ids the intent carries, and the request id of
    /// the `CreateWorktree` it sent.
    fn stage_launch(app: &mut App, out: &mut Vec<ClientRequest>) -> (String, PlaceholderRows, u64) {
        seed_tree(app);
        seed_feat_worktree(app, "w2", "feat");
        app.hide_root_worktree = true;
        app.focus = Focus::Worktrees;
        app.sel_worktree = 0;
        assert_eq!(
            app.selected_worktree().map(|w| w.branch.as_str()),
            Some("feat")
        );
        press(app, KeyCode::Char('p'), KeyModifiers::NONE, out);
        let branch = match &app.overlay {
            Some(Overlay::Prompt(prompt)) => match &prompt.kind {
                PromptKind::QuickPrompt(launch) => match &launch.target {
                    QuickTarget::NewWorktree { branch, .. } => branch.clone(),
                    other => panic!("expected a new-worktree target, got {other:?}"),
                },
                other => panic!("expected the quick prompt, got {other:?}"),
            },
            other => panic!("p should open the quick prompt, got {other:?}"),
        };
        assert!(paste_into_overlay(app, "Fix auth"));
        press(app, KeyCode::Enter, KeyModifiers::NONE, out);
        assert!(app.overlay.is_none(), "launching closes the box");
        let req_id = match out.as_slice() {
            [ClientRequest::CreateWorktree {
                req_id, branch: b, ..
            }] if b == &branch => *req_id,
            other => panic!("one CreateWorktree and nothing else: {other:?}"),
        };
        let placeholder = match app.pending.get(&req_id) {
            Some(PendingIntent::LaunchInCreatedWorktree { placeholder, .. }) => placeholder.clone(),
            other => panic!("the intent carries the stand-ins: {other:?}"),
        };
        out.clear();
        (branch, placeholder, req_id)
    }

    /// The DAEMON's answer to the `CreateWorktree`, in its own order: the
    /// row's upsert, then the Ack naming it. Returns the `CreateAgent`
    /// request id that follows.
    fn worktree_created(
        app: &mut App,
        branch: &str,
        req_id: u64,
        out: &mut Vec<ClientRequest>,
    ) -> u64 {
        seed_feat_worktree(app, "w3", branch);
        handle_server_event(
            app,
            ServerEvent::Ack {
                req_id,
                created: Some(EntityId::Worktree(WorktreeId("w3".into()))),
            },
            out,
        );
        let req_id = match out.as_slice() {
            [ClientRequest::CreateAgent {
                req_id, worktree, ..
            }] if worktree.0 == "w3" => *req_id,
            other => panic!("one CreateAgent in w3 and nothing else: {other:?}"),
        };
        out.clear();
        req_id
    }

    fn real_agent(id: &str, worktree: &str) -> Agent {
        Agent {
            id: AgentId(id.into()),
            worktree_id: WorktreeId(worktree.into()),
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
            status_changed_at: crate::app::now_ms(),
            alive: true,
            cloud_mirroring: false,
        }
    }

    fn screen(app: &mut App) -> String {
        let mut terminal = Terminal::new(TestBackend::new(160, 40)).unwrap();
        terminal.draw(|f| ui::draw(f, app)).unwrap();
        buffer_text(&terminal)
    }

    /// Enter puts the checkout row and its session row up at once, both
    /// selected as the real create would leave them, the pane on the
    /// session's "starting…" — and sends the DAEMON nothing but the
    /// `CreateWorktree`: no Attach, no Input, nothing under a made-up id.
    #[test]
    fn enter_puts_both_rows_up_before_the_daemon_answers() {
        with_default_config(|| {
            let mut app = App::new();
            let mut out = Vec::new();
            let (branch, rows, _) = stage_launch(&mut app, &mut out);

            assert_eq!(worktree_branches(&app), [branch.clone(), "feat".into()]);
            let selected = app.selected_worktree().expect("a row is selected");
            assert_eq!(selected.id, rows.worktree, "the cursor is on the stand-in");
            assert!(app.is_placeholder_worktree(&rows.worktree));
            assert_eq!(
                app.focus,
                Focus::Worktrees,
                "focus stays where p was pressed"
            );

            let sessions = app.visible_session_rows();
            assert_eq!(sessions.len(), 1, "{sessions:?}");
            assert_eq!(sessions[0].name(), "agent-1");
            assert_eq!(
                sessions[0].sref(),
                Some(SessionRef::Agent(rows.agent.clone()))
            );
            assert_eq!(app.sel_session, 0);
            assert!(app.is_placeholder_agent(&rows.agent));

            let term = app.term.as_ref().expect("the pane shows the stand-in");
            assert_eq!(term.sref, SessionRef::Agent(rows.agent.clone()));
            assert!(
                !term.painted,
                "nothing has come off a PTY — it reads as booting"
            );
            assert!(app.attached_sref.is_none(), "nothing is attached behind it");

            // The column truncates a three-word branch, so the badge is
            // what to look for; the session row keeps no "just now".
            let text = screen(&mut app);
            assert!(text.contains(" creating"), "{text}");
            assert!(text.contains("agent-1 starting"), "{text}");
            assert!(text.contains("starting session…"), "{text}");
        });
    }

    /// The Acks, in the DAEMON's order (upsert, then Ack): the checkout
    /// row becomes the real one with the session row moved under it, and
    /// the `CreateAgent` goes out named for a fresh checkout — the stand-in
    /// does not count as a taken name. Then the session row becomes the
    /// real one and the pane attaches it. Cursor and focus never move.
    #[test]
    fn the_acks_turn_the_stand_ins_into_the_real_rows() {
        with_default_config(|| {
            let mut app = App::new();
            let mut out = Vec::new();
            let (branch, rows, req_id) = stage_launch(&mut app, &mut out);

            seed_feat_worktree(&mut app, "w3", &branch);
            handle_server_event(
                &mut app,
                ServerEvent::Ack {
                    req_id,
                    created: Some(EntityId::Worktree(WorktreeId("w3".into()))),
                },
                &mut out,
            );
            assert_eq!(
                worktree_branches(&app),
                [branch.clone(), "feat".into()],
                "one row, not two"
            );
            assert_eq!(app.selected_worktree().map(|w| w.id.0.as_str()), Some("w3"));
            assert!(
                !app.tree.worktrees.iter().any(|w| w.id == rows.worktree),
                "the stand-in is gone"
            );
            assert_eq!(app.focus, Focus::Worktrees);
            let sessions = app.visible_session_rows();
            assert_eq!(sessions.len(), 1, "{sessions:?}");
            assert_eq!(
                sessions[0].sref(),
                Some(SessionRef::Agent(rows.agent.clone()))
            );
            assert!(
                app.is_placeholder_agent(&rows.agent),
                "the session is still a stand-in"
            );
            let req_id = match out.as_slice() {
                [ClientRequest::CreateAgent {
                    req_id,
                    worktree,
                    name,
                    auto_title: true,
                    starting_prompt: Some(text),
                    ..
                }] if worktree.0 == "w3" && text == "Fix auth" => {
                    assert_eq!(name, "agent-1", "the stand-in is not a taken name");
                    *req_id
                }
                other => panic!("one CreateAgent in w3: {other:?}"),
            };
            out.clear();

            hse(
                &mut app,
                ServerEvent::EntityUpserted {
                    entity: Entity::Agent(real_agent("a9", "w3")),
                },
            );
            handle_server_event(
                &mut app,
                ServerEvent::Ack {
                    req_id,
                    created: Some(EntityId::Agent(AgentId("a9".into()))),
                },
                &mut out,
            );
            let sessions = app.visible_session_rows();
            assert_eq!(sessions.len(), 1, "one row, not two: {sessions:?}");
            assert_eq!(
                sessions[0].sref(),
                Some(SessionRef::Agent(AgentId("a9".into())))
            );
            assert_eq!(app.sel_session, 0);
            assert!(
                !app.tree.agents.iter().any(|a| a.id == rows.agent),
                "the stand-in is gone"
            );
            assert!(app.pending.is_empty(), "{:?}", app.pending);
            assert_eq!(
                app.term.as_ref().map(|t| t.sref.clone()),
                Some(SessionRef::Agent(AgentId("a9".into())))
            );
            assert!(
                out.iter().any(|r| matches!(r, ClientRequest::Attach { session, .. } if session == &SessionRef::Agent(AgentId("a9".into())))),
                "the real session is attached: {out:?}"
            );
            assert!(
                !out.iter()
                    .any(|r| matches!(r, ClientRequest::Detach { .. })),
                "nothing was ever attached to let go of: {out:?}"
            );
            assert_eq!(app.focus, Focus::Worktrees, "quick_prompt_focus is off");
            // Both badges are gone; the pane header's own "starting…" is
            // the real session booting.
            let text = screen(&mut app);
            assert!(!text.contains(" creating"), "{text}");
            assert!(!text.contains("agent-1 starting"), "{text}");
        });
    }

    /// An Ack that lands ahead of its upsert renames the stand-in in place
    /// — one row throughout — and the upsert then overwrites it by id.
    #[test]
    fn an_ack_ahead_of_its_upsert_renames_the_stand_in_in_place() {
        with_default_config(|| {
            let mut app = App::new();
            let mut out = Vec::new();
            let (branch, rows, req_id) = stage_launch(&mut app, &mut out);

            handle_server_event(
                &mut app,
                ServerEvent::Ack {
                    req_id,
                    created: Some(EntityId::Worktree(WorktreeId("w3".into()))),
                },
                &mut out,
            );
            assert_eq!(worktree_branches(&app), [branch.clone(), "feat".into()]);
            assert_eq!(app.selected_worktree().map(|w| w.id.0.as_str()), Some("w3"));
            assert!(!app.is_placeholder_worktree(&WorktreeId("w3".into())));
            assert_eq!(
                app.tree
                    .agents
                    .iter()
                    .find(|a| a.id == rows.agent)
                    .map(|a| a.worktree_id.0.as_str()),
                Some("w3"),
                "the session row moved under the real id"
            );
            let req_id = match out.as_slice() {
                [ClientRequest::CreateAgent {
                    req_id, worktree, ..
                }] if worktree.0 == "w3" => *req_id,
                other => panic!("one CreateAgent in w3: {other:?}"),
            };
            out.clear();
            seed_feat_worktree(&mut app, "w3", &branch);
            assert_eq!(
                worktree_branches(&app),
                [branch.clone(), "feat".into()],
                "still one row"
            );
            assert_eq!(
                app.selected_worktree()
                    .map(|w| w.path.to_string_lossy().into_owned()),
                Some(format!("/tmp/demo-worktrees/{branch}")),
                "the upsert filled the path in"
            );

            handle_server_event(
                &mut app,
                ServerEvent::Ack {
                    req_id,
                    created: Some(EntityId::Agent(AgentId("a9".into()))),
                },
                &mut out,
            );
            let sessions = app.visible_session_rows();
            assert_eq!(sessions.len(), 1, "{sessions:?}");
            assert_eq!(
                sessions[0].sref(),
                Some(SessionRef::Agent(AgentId("a9".into())))
            );
            assert!(app.pending.is_empty());
            hse(
                &mut app,
                ServerEvent::EntityUpserted {
                    entity: Entity::Agent(real_agent("a9", "w3")),
                },
            );
            let sessions = app.visible_session_rows();
            assert_eq!(sessions.len(), 1, "still one row: {sessions:?}");
            assert!(app.tree.agents.iter().any(|a| a.id.0 == "a9" && a.alive));
        });
    }

    /// A refused checkout takes both rows down; the cursor goes back to
    /// the row it was on before `p`, the pane to what that row shows, and
    /// the box comes back with the text — with nothing sent to the DAEMON
    /// for rows it never had.
    #[test]
    fn a_refused_worktree_takes_both_rows_down_and_the_cursor_goes_back() {
        with_default_config(|| {
            let mut app = App::new();
            let mut out = Vec::new();
            let (branch, rows, req_id) = stage_launch(&mut app, &mut out);

            handle_server_event(
                &mut app,
                ServerEvent::Error {
                    req_id: Some(req_id),
                    message: "branch exists".into(),
                },
                &mut out,
            );
            assert_eq!(worktree_branches(&app), ["feat"]);
            assert!(!app.tree.agents.iter().any(|a| a.id == rows.agent));
            assert_eq!(
                app.selected_worktree().map(|w| w.branch.as_str()),
                Some("feat")
            );
            assert!(app.pending.is_empty());
            assert!(app.term.is_none(), "feat has no session to show");
            assert!(out.is_empty(), "nothing to detach or attach: {out:?}");
            assert_eq!(app.flash.as_deref(), Some("branch exists"));
            assert!(matches!(
                &app.overlay,
                Some(Overlay::Prompt(prompt))
                    if matches!(
                        &prompt.kind,
                        PromptKind::QuickPrompt(launch)
                            if matches!(&launch.target, QuickTarget::NewWorktree { branch: b, .. } if b == &branch)
                    ) && prompt.input.as_str() == "Fix auth"
            ));
            assert!(!app
                .last_worktree_for_project
                .values()
                .any(|w| *w == rows.worktree));
        });
    }

    /// A refused session after the checkout was cut takes only the session
    /// row down: the worktree is real and stays selected, and the box
    /// comes back aimed at it.
    #[test]
    fn a_refused_agent_takes_its_row_down_and_the_worktree_stays() {
        with_default_config(|| {
            let mut app = App::new();
            let mut out = Vec::new();
            let (branch, rows, req_id) = stage_launch(&mut app, &mut out);
            let req_id = worktree_created(&mut app, &branch, req_id, &mut out);

            handle_server_event(
                &mut app,
                ServerEvent::Error {
                    req_id: Some(req_id),
                    message: "claude is not installed".into(),
                },
                &mut out,
            );
            // Without its session the checkout has no stamp and sorts
            // under `feat` again; the cursor follows the row.
            assert_eq!(
                worktree_branches(&app),
                ["feat".to_string(), branch.clone()]
            );
            assert_eq!(app.selected_worktree().map(|w| w.id.0.as_str()), Some("w3"));
            assert!(app.visible_session_rows().is_empty());
            assert!(!app.tree.agents.iter().any(|a| a.id == rows.agent));
            assert!(app.term.is_none());
            assert!(app.pending.is_empty());
            assert!(matches!(
                &app.overlay,
                Some(Overlay::Prompt(prompt))
                    if matches!(
                        &prompt.kind,
                        PromptKind::QuickPrompt(launch)
                            if launch.target == QuickTarget::Worktree(WorktreeId("w3".into()))
                    ) && prompt.input.as_str() == "Fix auth"
            ));
        });
    }

    /// While the create is in flight the stand-ins are on screen but not
    /// in the DAEMON: a launch, a terminal, a delete, a prewarm, an
    /// attach and a keystroke aimed at them all stop here.
    #[test]
    fn nothing_reaches_the_daemon_under_a_stand_in_id() {
        with_default_config(|| {
            let mut app = App::new();
            let mut out = Vec::new();
            let (_, rows, _) = stage_launch(&mut app, &mut out);

            // The prewarm the landing armed.
            assert_eq!(
                app.pending_prewarm.as_ref().map(|(w, _)| w),
                Some(&rows.worktree)
            );
            fire_pending_prewarm(&mut app, &mut out);
            assert!(out.is_empty(), "{out:?}");

            press(&mut app, KeyCode::Char('d'), KeyModifiers::NONE, &mut out);
            assert!(app.overlay.is_none(), "{:?}", app.overlay);
            assert_eq!(
                app.flash.as_deref(),
                Some("worktree is still being created")
            );

            app.focus = Focus::Sessions;
            press(&mut app, KeyCode::Char('p'), KeyModifiers::NONE, &mut out);
            assert!(app.overlay.is_none(), "{:?}", app.overlay);
            assert_eq!(
                app.flash.as_deref(),
                Some("quick prompt: worktree is still being created")
            );
            press(&mut app, KeyCode::Char('t'), KeyModifiers::NONE, &mut out);
            assert_eq!(
                app.flash.as_deref(),
                Some("worktree is still being created")
            );
            assert!(out.is_empty(), "{out:?}");

            // Enter on the stand-in session row enters the pane as it
            // would any row, but attaches and forwards nothing.
            press(&mut app, KeyCode::Enter, KeyModifiers::NONE, &mut out);
            assert_eq!(app.focus, Focus::Terminal);
            assert!(app.term_locked);
            press(&mut app, KeyCode::Char('x'), KeyModifiers::NONE, &mut out);
            assert!(
                !out.iter().any(|r| matches!(
                    r,
                    ClientRequest::Attach { .. } | ClientRequest::Input { .. }
                )),
                "{out:?}"
            );
        });
    }
}
