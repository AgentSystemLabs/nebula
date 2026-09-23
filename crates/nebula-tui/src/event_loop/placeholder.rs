//! Stand-in rows for a WORKTREE that does not exist yet. The DAEMON's
//! `CreateWorktree` is a fetch plus `git worktree add` plus the WORKTREE
//! HOOK, and the `CreateAgent` after it a CLI spawn — seconds, during
//! which the panels used to show nothing. Now Enter puts the rows up at
//! once, under ids this client made, and the cursors land on them exactly
//! as the real create would leave them: the Ack of each request turns its
//! stand-in into the real row, and an Error takes the stand-ins down and
//! hands the box back.
//!
//! Three boxes create checkouts. The NEW WORKTREE modal (`n` on the
//! WORKTREES PANEL, "New worktree" in a project's menu) puts up the
//! checkout row alone, selected with FOCUS on its empty SESSIONS PANEL —
//! `stage_worktree`. A QUICK PROMPT into a worktree that does not exist
//! yet (`p` on the WORKTREES PANEL) puts up that row and the session row
//! that will follow it — `stage`. A PR SESSION on a pull request whose
//! head branch has no checkout yet (Enter on an OPEN PRS row) puts up the
//! same two rows — `stage` again — behind the one `CreatePrAgent` that
//! fetches the branch, cuts the worktree and spawns in it; there the
//! checkout's upsert adopts its stand-in (`adopt_worktree`), since the
//! Ack names only the session.
//!
//! The rows are ordinary `Worktree` / `Agent` entries in `app.tree` — every
//! list, sort and count treats them as it would the real ones. What marks
//! them is the PENDING INTENT that carries their ids
//! (`App::is_placeholder_worktree` / `is_placeholder_agent`): the intent
//! is removed by the very Ack or Error that resolves them, so nothing has
//! to be kept in sync. Nothing ever reaches the DAEMON under a stand-in
//! id — `send_attach`, `create_agent`, `create_terminal`, the prewarm,
//! the delete and the pane's Input all check first. One launch is held
//! rather than refused: an AGENT PRESET run off the modal's row before
//! the DAEMON has answered waits on the checkout's own Ack
//! (`defer_launch`), and a modal opened on a stand-in — the presets
//! list, its task box — is readdressed to the real row when that Ack
//! lands (`resolve_worktree`).

use super::{
    create_agent, pane_size, reconcile_selection, release_attachment, remove_worktree_rows,
    restore_context, select_worktree_by_id, selection_snapshot, WORKTREE_STILL_CREATING,
};
use crate::app::{
    now_ms, AgentLaunchDraft, App, AttachedTerm, ConfirmDialog, Overlay, PendingAction,
    PendingIntent, PlaceholderRows,
};
use nebula_core::{
    Agent, AgentId, AgentKind, AgentStatus, ClientRequest, ProjectId, SessionRef, Worktree,
    WorktreeId,
};

/// Put the checkout row up and land the cursor on it the way the Ack of
/// its create would (`select_worktree_by_id`: the row selected, FOCUS on
/// its SESSIONS PANEL so `n` starts a session there). With no session
/// under it the row sorts where the real one will — the checkouts of a
/// project without a session share one RECENCY, and a row the DAEMON
/// upserts joins the list at the same end — so the swap does not move
/// it. Returns the id the intent carries.
pub(super) fn stage_worktree(
    app: &mut App,
    project: ProjectId,
    branch: String,
    out: &mut Vec<ClientRequest>,
) -> WorktreeId {
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
    // False when the checkout's project is not the selected one (a
    // project menu's "New worktree" on another row): the row waits in the
    // tree for that project to be selected, as the real one would.
    select_worktree_by_id(app, &worktree, out);
    app.dirty = true;
    worktree
}

/// Put the two rows up and land the cursors on them: the worktree row
/// selected (FOCUS kept where the launch was fired from, as every QUICK
/// PROMPT launch keeps it), its one session row — a `kind` CLI at
/// `model` / `effort`, named as the create will name it — selected, the
/// pane showing "starting…" for it. Returns the ids the intents carry.
#[allow(clippy::too_many_arguments)]
pub(super) fn stage(
    app: &mut App,
    project: ProjectId,
    branch: String,
    kind: AgentKind,
    custom: Option<String>,
    model: Option<String>,
    effort: Option<String>,
    first_prompt: bool,
    out: &mut Vec<ClientRequest>,
) -> PlaceholderRows {
    let focus = app.focus;
    let worktree = stage_worktree(app, project, branch, out);
    let agent = stage_agent(
        app,
        &worktree,
        kind,
        custom,
        model,
        effort,
        first_prompt,
        out,
    );
    app.focus = focus;
    PlaceholderRows { worktree, agent }
}

/// Put the session row up under `worktree` — a `kind` CLI at `model` /
/// `effort`, named as the create will name it — selected, the pane
/// showing "starting…" for it; FOCUS stays where it is. `stage`'s second
/// half, and on its own the row of a launch waiting on the NEW WORKTREE
/// modal's checkout (`defer_launch`). Returns the id the intent carries.
///
/// A BACKGROUND LAUNCH puts the row up and stops there: nothing is
/// selected and the pane is not taken, because the launch landed in a
/// project the user is not looking at.
///
/// `first_prompt` is a launch carrying a task (a QUICK PROMPT's, an AGENT
/// PRESET's): the CLI submits it the moment it boots, so the row is staged
/// `running` exactly as the DAEMON will create it — the same optimism, one
/// checkout earlier.
#[allow(clippy::too_many_arguments)]
pub(super) fn stage_agent(
    app: &mut App,
    worktree: &WorktreeId,
    kind: AgentKind,
    custom: Option<String>,
    model: Option<String>,
    effort: Option<String>,
    first_prompt: bool,
    out: &mut Vec<ClientRequest>,
) -> AgentId {
    // The name the DAEMON will give the real row: `default_session_name`
    // reads the selected worktree, which is the stand-in (no
    // sessions), and `create_agent` takes the name off this row when the
    // create goes out, so the two agree.
    let name = app.default_session_name("agent");
    let agent = AgentId::generate();
    // The stamp below re-sorts the PROJECTS PANEL as well: the newest
    // session under any project, it puts the checkout's project on the
    // top row. `sel_project` is a row index, so the project is held by
    // id across the push — left alone, the cursor would highlight
    // whichever project slid into the vacated row, and `worktree_row_of`
    // below would look for the stand-in among that project's checkouts.
    //
    // A BACKGROUND LAUNCH (`launcher::is_background` — a box aimed at
    // another project with `^P`) is the one that holds nothing: its rows
    // go up in that project's list and the screen stays where the user
    // is, cursor and pane included. See the early return below.
    let project = app.selected_project().map(|p| p.id.clone());
    let background = app
        .tree
        .worktrees
        .iter()
        .find(|w| &w.id == worktree)
        .map(|w| w.project_id.clone())
        .is_some_and(|landed| crate::launcher::is_background(app, &landed));
    app.tree.agents.push(Agent {
        id: agent.clone(),
        worktree_id: worktree.clone(),
        name,
        status: if first_prompt {
            AgentStatus::Running
        } else {
            AgentStatus::Fresh
        },
        archived: false,
        archived_at: 0,
        unseen: false,
        kind,
        custom_harness: custom,
        model,
        effort,
        session_id: None,
        cloud_session_id: None,
        sort_order: 0,
        // The DAEMON stamps a created row with its clock, which puts the
        // checkout at the top of RECENCY ORDER; the same stamp here means
        // the row does not jump when the real one replaces it.
        status_changed_at: now_ms(),
        alive: false,
        issue_url: None,
        recent_prompts: Vec::new(),
    });
    if let Some(i) = project.and_then(|id| {
        app.project_rows()
            .iter()
            .position(|i| app.tree.projects[*i].id == id)
    }) {
        app.sel_project = i;
    }
    // A prompt fired into another project is not a place to go: the row
    // is in the tree for that project's list, the cursor above only held
    // its ground through the re-sort, and nothing else here runs — the
    // grid, the worktree cursor and the pane stay on the work in front
    // of the user. The create's Ack is left behind for the same reason
    // (`quick_launch::submit`).
    if background {
        app.dirty = true;
        return agent;
    }
    // That stamp just moved the row to the top — or, for a PR SESSION's
    // stand-in, under its pull request's row: re-seat the cursor on it.
    if let Some(i) = app.worktree_row_of(worktree) {
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
    let mut term = AttachedTerm::new(SessionRef::Agent(agent.clone()), cols, rows);
    // There is no PTY to hear from until the create lands: the pane's
    // "starting session…" is the whole story for now.
    term.booting = true;
    app.term = Some(term);
    app.dirty = true;
    agent
}

/// The `CreateWorktree` Ack: `real` is the checkout the DAEMON cut. Its
/// upsert usually lands just before the Ack, in which case the real row is
/// already in and the stand-in goes; if the Ack came first the stand-in
/// becomes the real row (same id) and the upsert overwrites it in place.
/// A stand-in session under the checkout (a QUICK PROMPT's) moves under
/// the real one either way, so the row keeps its place in RECENCY ORDER
/// and its selection.
pub(super) fn resolve_worktree(app: &mut App, placeholder: &WorktreeId, real: &WorktreeId) {
    if app.tree.worktrees.iter().any(|w| &w.id == real) {
        app.tree.worktrees.retain(|w| &w.id != placeholder);
    } else if let Some(w) = app.tree.worktrees.iter_mut().find(|w| &w.id == placeholder) {
        w.id = real.clone();
    }
    for a in app
        .tree
        .agents
        .iter_mut()
        .filter(|a| &a.worktree_id == placeholder)
    {
        a.worktree_id = real.clone();
    }
    // The context memory `select_worktree_by_id` wrote for the stand-in
    // now describes the real checkout.
    if let Some(sref) = app.last_session_for_worktree.remove(placeholder) {
        app.last_session_for_worktree.insert(real.clone(), sref);
    }
    for wid in app.last_worktree_for_project.values_mut() {
        if wid == placeholder {
            *wid = real.clone();
        }
    }
    // A modal opened on the stand-in — the AGENT PRESETS list `e` put up
    // on the new row, the task box behind it — is addressed to the real
    // checkout from here, so its Enter names a checkout the DAEMON has
    // rather than an id it never had. The prewarm armed on the row too.
    if let Some(named) = app.overlay.as_mut().and_then(modal_worktree_mut) {
        if *named == *placeholder {
            *named = real.clone();
        }
    }
    if let Some((armed, _)) = &mut app.pending_prewarm {
        if *armed == *placeholder {
            *armed = real.clone();
        }
    }
    forget_worktree(app, placeholder);
    app.dirty = true;
}

/// A checkout the DAEMON just registered, before its upsert is applied:
/// when a PR SESSION's stand-in for that branch of that project is up,
/// the stand-in becomes this row — the upsert then overwrites it in
/// place, the session stand-in under it moves along, and the cursor
/// stays put. Called ahead of the handler's `selection_snapshot`, which
/// names rows by id: the stand-in's is about to become this one's. Any
/// other checkout (the ROOT's, one cut by `n`) matches no stand-in and
/// nothing happens.
pub(super) fn adopt_worktree(app: &mut App, real: &Worktree) {
    let stand_in =
        app.pending
            .values()
            .filter_map(|intent| match intent {
                PendingIntent::AttachCreatedPrSession { placeholder, .. } => {
                    Some(&placeholder.worktree)
                }
                _ => None,
            })
            .find(|id| {
                app.tree.worktrees.iter().any(|w| {
                    &w.id == *id && w.project_id == real.project_id && w.branch == real.branch
                })
            })
            .cloned();
    if let Some(stand_in) = stand_in {
        resolve_worktree(app, &stand_in, &real.id);
    }
}

/// The `CreatePrAgent` Ack, before its session stand-in is resolved
/// (`resolve_agent`, through `attach_created`). The checkout's upsert
/// normally adopted the stand-in worktree as it landed, and there is
/// nothing to do. When the Ack outran the upserts, the created row names
/// the checkout if its own upsert is in; otherwise both stand-ins go —
/// the real rows are moments behind, and the created session's
/// `select_when_seen` lands the cursor on them as they arrive. Left up,
/// the stand-in would outlive the intent that marks it one, and sit
/// beside the real row as a second checkout on the branch.
pub(super) fn settle_pr_worktree(
    app: &mut App,
    rows: &PlaceholderRows,
    real: &AgentId,
    out: &mut Vec<ClientRequest>,
) {
    if !app.tree.worktrees.iter().any(|w| w.id == rows.worktree) {
        return;
    }
    let checkout = app
        .tree
        .agents
        .iter()
        .find(|a| &a.id == real)
        .map(|a| a.worktree_id.clone());
    match checkout {
        Some(checkout) => resolve_worktree(app, &rows.worktree, &checkout),
        None => discard(app, rows, out),
    }
}

/// The `CreatePrAgent` was refused. Before the checkout was cut, both
/// stand-ins go (`discard`); after it — the upsert adopted the worktree
/// row, then the CLI spawn failed — the session stand-in alone goes, the
/// checkout is real.
pub(super) fn discard_pr(app: &mut App, rows: &PlaceholderRows, out: &mut Vec<ClientRequest>) {
    if app.tree.worktrees.iter().any(|w| w.id == rows.worktree) {
        discard(app, rows, out);
    } else {
        discard_agent(app, &rows.agent, out);
    }
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

/// The QUICK PROMPT's `CreateWorktree` was refused: both rows go, the
/// cursor as `discard_worktree` leaves it.
pub(super) fn discard(app: &mut App, rows: &PlaceholderRows, out: &mut Vec<ClientRequest>) {
    blank_pane_if_showing(app, &rows.agent);
    discard_worktree(app, &rows.worktree, out);
}

/// A `CreateWorktree` was refused: the stand-in checkout row goes. A
/// cursor still on it goes back to the row it was on before the box
/// opened — `remember_context` recorded that row when the stand-in took
/// the cursor — and a cursor the user moved elsewhere meanwhile stays
/// put. Returns whether the cursor was on the stand-in, so a caller that
/// moved FOCUS with the row can put that back too.
pub(super) fn discard_worktree(
    app: &mut App,
    placeholder: &WorktreeId,
    out: &mut Vec<ClientRequest>,
) -> bool {
    let on_it = app
        .selected_worktree()
        .is_some_and(|w| &w.id == placeholder);
    let before = selection_snapshot(app);
    let _ = remove_worktree_rows(app, placeholder);
    // A modal opened on the stand-in has nothing to launch into now: it
    // closes with the row. (The NEW WORKTREE modal's refusal puts its
    // own box back over this.)
    if app
        .overlay
        .as_mut()
        .and_then(modal_worktree_mut)
        .is_some_and(|named| *named == *placeholder)
    {
        app.overlay = None;
    }
    app.last_session_for_worktree.remove(placeholder);
    app.last_worktree_for_project
        .retain(|_, wid| wid != placeholder);
    forget_worktree(app, placeholder);
    if on_it {
        restore_context(app, out);
    } else {
        reconcile_selection(app, before, out);
    }
    app.dirty = true;
    on_it
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

/// A launch fired into a stand-in checkout. Nothing goes to the DAEMON
/// under an id it has never seen — but a launch into the NEW WORKTREE
/// modal's row (`e` on the new row, an AGENT PRESET picked, its task
/// typed, all before the DAEMON's `git worktree add` has finished) is
/// not lost either: its session row goes up under the stand-in as a
/// QUICK PROMPT's would, the draft rides the checkout's own PENDING
/// INTENT, and the Ack that names the real checkout sends it there
/// (`replay_launch`). Every other launch waits as before, saying so: one
/// into a QUICK PROMPT's or a PR SESSION's stand-in, whose intent
/// already carries the launch that made it; a second into the modal's
/// while one is waiting; a PR SESSION, which is addressed to the PROJECT
/// and cuts a checkout of its own.
pub(super) fn defer_launch(
    app: &mut App,
    mut draft: AgentLaunchDraft,
    out: &mut Vec<ClientRequest>,
) {
    let slot = app
        .pending
        .iter()
        .find_map(|(req_id, intent)| match intent {
            PendingIntent::SelectCreatedWorktree {
                placeholder,
                launch: None,
                ..
            } if *placeholder == draft.worktree => Some(*req_id),
            _ => None,
        });
    let Some(req_id) = slot.filter(|_| draft.pr.is_none()) else {
        app.flash = Some(WORKTREE_STILL_CREATING.into());
        return;
    };
    let agent = stage_agent(
        app,
        &draft.worktree,
        draft.kind,
        draft.custom.clone(),
        draft.model.clone(),
        draft.effort.clone(),
        draft.starting_prompt.is_some(),
        out,
    );
    draft.placeholder = Some(agent);
    if let Some(PendingIntent::SelectCreatedWorktree { launch, .. }) = app.pending.get_mut(&req_id)
    {
        *launch = Some(Box::new(draft));
    }
}

/// The modal's Ack with a launch waiting on it: the checkout is `real`
/// now, its stand-in session row moved under it by `resolve_worktree`,
/// so the draft — and the box it brings back should the DAEMON refuse
/// the create — is addressed there and goes out as it would have from a
/// real row.
pub(super) fn replay_launch(
    app: &mut App,
    mut draft: AgentLaunchDraft,
    real: &WorktreeId,
    out: &mut Vec<ClientRequest>,
) {
    draft.worktree = real.clone();
    if let Some(named) = draft
        .reopen_on_error
        .as_mut()
        .and_then(|(kind, _)| kind.worktree_mut())
    {
        *named = real.clone();
    }
    create_agent(app, draft, out);
}

/// The checkout a modal is addressed to — the one its Enter launches
/// into, or the AGENT PRESETS list it reopens for. None for a modal that
/// names no checkout, and for a QUICK PROMPT about to cut one.
fn modal_worktree_mut(overlay: &mut Overlay) -> Option<&mut WorktreeId> {
    match overlay {
        Overlay::Prompt(dialog) => dialog.kind.worktree_mut(),
        Overlay::AgentPresets(view) => Some(&mut view.worktree),
        Overlay::AgentPresetEditor(editor) => Some(&mut editor.worktree),
        Overlay::Confirm(ConfirmDialog {
            action: PendingAction::DeleteAgentPreset { worktree, .. },
            ..
        }) => Some(worktree),
        _ => None,
    }
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
        // The stand-in was typed at while the real session booted: taking
        // its pane away takes the keyboard with it, and says so.
        app.release_terminal();
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
        buffer_text, hse, press, seed_feat_worktree, seed_open_prs, seed_tree, with_default_config,
        with_seeded_presets, worktree_branches,
    };
    use super::super::{
        fire_pending_prewarm, handle_server_event, handle_terminal_event, paste_into_overlay,
    };
    use crate::app::{App, Focus, Overlay, PendingIntent, PlaceholderRows, PromptKind};
    use crate::quick_prompt::QuickTarget;
    use crate::ui;
    use crossterm::event::{Event, KeyCode, KeyModifiers, MouseEvent, MouseEventKind};
    use nebula_core::{
        Agent, AgentId, AgentKind, AgentStatus, ClientRequest, Entity, EntityId, ServerEvent,
        SessionRef, WorktreeId,
    };
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    /// `p` on the WORKTREES PANEL with the cursor on `feat`, "Fix auth"
    /// typed, Enter pressed. Returns the branch the box offered, the
    /// stand-in ids the intent carries, and the request id of the
    /// `CreateWorktree` it sent. `p` opens on the checkout (the
    /// new-worktree SETTING is off) and `^N` flips it to a fresh one.
    fn stage_launch(app: &mut App, out: &mut Vec<ClientRequest>) -> (String, PlaceholderRows, u64) {
        seed_tree(app);
        seed_feat_worktree(app, "w2", "feat");
        app.focus = Focus::Worktrees;
        app.sel_worktree = 1;
        assert_eq!(
            app.selected_worktree().map(|w| w.branch.as_str()),
            Some("feat")
        );
        press(app, KeyCode::Char('p'), KeyModifiers::NONE, out);
        press(app, KeyCode::Char('n'), KeyModifiers::CONTROL, out);
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
            custom_harness: None,
            model: None,
            effort: None,
            session_id: None,
            cloud_session_id: None,
            sort_order: 0,
            status_changed_at: crate::app::now_ms(),
            alive: true,
            issue_url: None,
            recent_prompts: Vec::new(),
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

            assert_eq!(
                worktree_branches(&app),
                ["main".into(), branch.clone(), "feat".into()]
            );
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

            // The card is up with the session's name on it and the pane
            // under it says the session is still coming.
            let text = screen(&mut app);
            assert!(text.contains("agent-1"), "{text}");
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
                ["main".into(), branch.clone(), "feat".into()],
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
            assert_eq!(
                worktree_branches(&app),
                ["main".into(), branch.clone(), "feat".into()]
            );
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
                ["main".into(), branch.clone(), "feat".into()],
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
            assert_eq!(worktree_branches(&app), ["main", "feat"]);
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
                ["main".to_string(), "feat".to_string(), branch.clone()]
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

    /// The NEW WORKTREE box, "feat" typed, Enter pressed: the checkout
    /// row is up and selected as the Ack would leave it, and nothing but
    /// the `CreateWorktree` went to the DAEMON. Returns the stand-in id
    /// and the request id.
    fn stage_modal(app: &mut App, out: &mut Vec<ClientRequest>) -> (WorktreeId, u64) {
        seed_tree(app);
        app.focus = Focus::Worktrees;
        let project = app.selected_project().expect("a project").id.clone();
        super::super::open_new_worktree_prompt(app, project);
        assert!(
            matches!(&app.overlay, Some(Overlay::Prompt(p)) if matches!(p.kind, PromptKind::NewWorktree { .. })),
            "the new-worktree box is up: {:?}",
            app.overlay
        );
        assert!(paste_into_overlay(app, "feat"));
        press(app, KeyCode::Enter, KeyModifiers::NONE, out);
        assert!(app.overlay.is_none(), "submitting closes the box");
        let creates: Vec<&ClientRequest> = out
            .iter()
            .filter(|r| matches!(r, ClientRequest::CreateWorktree { .. }))
            .collect();
        let req_id = match creates.as_slice() {
            [ClientRequest::CreateWorktree { req_id, branch, .. }] if branch == "feat" => *req_id,
            other => panic!("one CreateWorktree for feat: {other:?}"),
        };
        assert!(
            !out.iter().any(|r| matches!(
                r,
                ClientRequest::Attach { .. }
                    | ClientRequest::Input { .. }
                    | ClientRequest::PrewarmWorktreeSessions { .. }
            )),
            "nothing under a made-up id: {out:?}"
        );
        let placeholder = match app.pending.get(&req_id) {
            Some(PendingIntent::SelectCreatedWorktree { placeholder, .. }) => placeholder.clone(),
            other => panic!("the intent carries the stand-in: {other:?}"),
        };
        out.clear();
        (placeholder, req_id)
    }

    /// Enter puts the checkout row up at once, selected, with FOCUS on
    /// its empty SESSIONS PANEL — where the Ack used to land seconds
    /// later — wearing the "creating" badge; and nothing can be started
    /// in it until the DAEMON answers.
    #[test]
    fn the_modal_puts_the_row_up_selected_before_the_daemon_answers() {
        with_default_config(|| {
            let mut app = App::new();
            let mut out = Vec::new();
            let (placeholder, _) = stage_modal(&mut app, &mut out);

            assert_eq!(worktree_branches(&app), ["main", "feat"]);
            let selected = app.selected_worktree().expect("a row is selected");
            assert_eq!(selected.id, placeholder, "the cursor is on the stand-in");
            assert!(app.is_placeholder_worktree(&placeholder));
            assert_eq!(app.focus, Focus::Sessions, "as the Ack left it before");
            assert_eq!(app.sel_session, 0);
            assert!(app.visible_session_rows().is_empty());
            assert!(app.term.is_none(), "nothing to show yet");
            let text = screen(&mut app);
            assert!(text.contains("feat"), "{text}");

            // The landing armed the prewarm; it stops at the stand-in.
            assert_eq!(
                app.pending_prewarm.as_ref().map(|(w, _)| w),
                Some(&placeholder)
            );
            fire_pending_prewarm(&mut app, &mut out);
            assert!(out.is_empty(), "{out:?}");

            // A new terminal here would land in a checkout the DAEMON
            // does not have: it stops before anything is sent.
            press(&mut app, KeyCode::Char('t'), KeyModifiers::NONE, &mut out);
            assert_eq!(
                app.flash.as_deref(),
                Some("worktree is still being created")
            );
            assert!(out.is_empty(), "{out:?}");
        });
    }

    /// The DAEMON's answer in its own order (upsert, then Ack): the
    /// stand-in becomes the real row under the same cursor and FOCUS —
    /// one row, not two, the badge gone — and the prewarm the stand-in
    /// could not take is armed for the real checkout.
    #[test]
    fn the_ack_swaps_the_real_row_in_under_the_cursor() {
        with_default_config(|| {
            let mut app = App::new();
            let mut out = Vec::new();
            let (placeholder, req_id) = stage_modal(&mut app, &mut out);

            seed_feat_worktree(&mut app, "w2", "feat");
            handle_server_event(
                &mut app,
                ServerEvent::Ack {
                    req_id,
                    created: Some(EntityId::Worktree(WorktreeId("w2".into()))),
                },
                &mut out,
            );
            assert_eq!(
                worktree_branches(&app),
                ["main", "feat"],
                "one row, not two"
            );
            assert_eq!(app.selected_worktree().map(|w| w.id.0.as_str()), Some("w2"));
            assert!(!app.tree.worktrees.iter().any(|w| w.id == placeholder));
            assert_eq!(app.focus, Focus::Sessions);
            assert_eq!(app.sel_session, 0);
            assert!(app.pending.is_empty(), "{:?}", app.pending);
            assert_eq!(
                app.pending_prewarm.as_ref().map(|(w, _)| w.0.as_str()),
                Some("w2"),
                "the real checkout gets the prewarm"
            );
            assert_eq!(
                app.last_worktree_for_project
                    .values()
                    .filter(|w| **w == placeholder)
                    .count(),
                0,
                "no context memory names the stand-in"
            );
            assert!(out.is_empty(), "the Ack moves nothing: {out:?}");
            let text = screen(&mut app);
            assert!(!text.contains(" creating"), "{text}");
        });
    }

    /// An Ack that lands ahead of its upsert renames the stand-in in place
    /// — one row throughout — and the upsert then fills the path in.
    #[test]
    fn an_ack_ahead_of_its_upsert_renames_the_modal_stand_in_in_place() {
        with_default_config(|| {
            let mut app = App::new();
            let mut out = Vec::new();
            let (_, req_id) = stage_modal(&mut app, &mut out);

            handle_server_event(
                &mut app,
                ServerEvent::Ack {
                    req_id,
                    created: Some(EntityId::Worktree(WorktreeId("w2".into()))),
                },
                &mut out,
            );
            assert_eq!(worktree_branches(&app), ["main", "feat"]);
            assert_eq!(app.selected_worktree().map(|w| w.id.0.as_str()), Some("w2"));
            assert!(!app.is_placeholder_worktree(&WorktreeId("w2".into())));
            assert!(app.select_worktree_when_seen.is_none());

            seed_feat_worktree(&mut app, "w2", "feat");
            assert_eq!(worktree_branches(&app), ["main", "feat"], "still one row");
            assert_eq!(
                app.selected_worktree()
                    .map(|w| w.path.to_string_lossy().into_owned()),
                Some("/tmp/demo-worktrees/feat".into()),
                "the upsert filled the path in"
            );
            assert_eq!(app.focus, Focus::Sessions);
        });
    }

    /// A refused checkout takes the row down: the cursor goes back to the
    /// row it was on, FOCUS back to the panel the box was opened from, and
    /// the box comes back with the name — with nothing sent for a row the
    /// DAEMON never had.
    #[test]
    fn a_refused_modal_worktree_takes_the_row_down_and_reopens_the_box() {
        with_default_config(|| {
            let mut app = App::new();
            let mut out = Vec::new();
            let (placeholder, req_id) = stage_modal(&mut app, &mut out);

            handle_server_event(
                &mut app,
                ServerEvent::Error {
                    req_id: Some(req_id),
                    message: "branch exists".into(),
                },
                &mut out,
            );
            assert_eq!(worktree_branches(&app), ["main"]);
            assert_eq!(app.selected_worktree().map(|w| w.id.0.as_str()), Some("w1"));
            assert_eq!(app.focus, Focus::Worktrees, "back where n was pressed");
            assert!(app.pending.is_empty());
            assert_eq!(app.flash.as_deref(), Some("branch exists"));
            assert!(
                matches!(
                    &app.overlay,
                    Some(Overlay::Prompt(prompt))
                        if matches!(&prompt.kind, PromptKind::NewWorktree { project, .. } if project.0 == "p1")
                            && prompt.input.as_str() == "feat"
                ),
                "{:?}",
                app.overlay
            );
            assert!(!app
                .last_worktree_for_project
                .values()
                .any(|w| *w == placeholder));
            // The cursor's return to the main checkout brings its live
            // session back on the spot; that is the only Attach, and
            // nothing at all is typed anywhere.
            let a1 = SessionRef::Agent(AgentId("a1".into()));
            assert!(
                !out.iter().any(|r| match r {
                    ClientRequest::Attach { session, .. } => *session != a1,
                    ClientRequest::Input { .. } => true,
                    _ => false,
                }),
                "{out:?}"
            );
        });
    }

    /// The cursor moved off the stand-in while the DAEMON worked: the Ack
    /// leaves it where the user put it — the select happened at Enter —
    /// and the real row simply takes the stand-in's place in the list.
    #[test]
    fn a_cursor_moved_off_the_stand_in_stays_put_at_the_ack() {
        with_default_config(|| {
            let mut app = App::new();
            let mut out = Vec::new();
            let (placeholder, req_id) = stage_modal(&mut app, &mut out);

            assert!(super::select_worktree_by_id(
                &mut app,
                &WorktreeId("w1".into()),
                &mut out
            ));
            app.focus = Focus::Worktrees;
            out.clear();

            seed_feat_worktree(&mut app, "w2", "feat");
            handle_server_event(
                &mut app,
                ServerEvent::Ack {
                    req_id,
                    created: Some(EntityId::Worktree(WorktreeId("w2".into()))),
                },
                &mut out,
            );
            assert_eq!(worktree_branches(&app), ["main", "feat"]);
            assert_eq!(app.selected_worktree().map(|w| w.id.0.as_str()), Some("w1"));
            assert_eq!(app.focus, Focus::Worktrees);
            assert!(!app.tree.worktrees.iter().any(|w| w.id == placeholder));
            assert!(app.pending.is_empty());
            assert_ne!(
                app.pending_prewarm.as_ref().map(|(w, _)| w.0.as_str()),
                Some("w2"),
                "no prewarm for a row the cursor is not on"
            );
            assert!(out.is_empty(), "{out:?}");
        });
    }

    // ---- an AGENT PRESET run off the modal's stand-in ----

    /// `n`, "feat", Enter, then `e` on the new row and Enter on the
    /// "reviewer" preset: the task box is open with "Fix auth" typed, and
    /// the DAEMON has not answered the `CreateWorktree` yet. Returns the
    /// stand-in id and that request's id.
    fn type_preset_task(app: &mut App, out: &mut Vec<ClientRequest>) -> (WorktreeId, u64) {
        let (placeholder, req_id) = stage_modal(app, out);
        press(app, KeyCode::Char('e'), KeyModifiers::NONE, out);
        assert!(
            matches!(&app.overlay, Some(Overlay::AgentPresets(view)) if view.worktree == placeholder),
            "e on the new row opens the list for it: {:?}",
            app.overlay
        );
        press(app, KeyCode::Enter, KeyModifiers::NONE, out);
        assert!(
            matches!(
                &app.overlay,
                Some(Overlay::Prompt(prompt))
                    if matches!(&prompt.kind, PromptKind::AgentPresetTask { worktree, preset } if *worktree == placeholder && preset.name == "reviewer")
            ),
            "Enter asks for the task: {:?}",
            app.overlay
        );
        assert!(paste_into_overlay(app, "Fix auth"));
        assert!(out.is_empty(), "{out:?}");
        (placeholder, req_id)
    }

    /// The DAEMON's answer to the modal's `CreateWorktree`, in its own
    /// order: the row's upsert, then the Ack naming it `w2`.
    fn modal_worktree_created(app: &mut App, req_id: u64, out: &mut Vec<ClientRequest>) {
        seed_feat_worktree(app, "w2", "feat");
        handle_server_event(
            app,
            ServerEvent::Ack {
                req_id,
                created: Some(EntityId::Worktree(WorktreeId("w2".into()))),
            },
            out,
        );
    }

    /// The bug as reported: the checkout lands while the task is being
    /// typed. The list and the box were opened on the stand-in's id; the
    /// Ack readdresses the box, so Enter launches into the checkout the
    /// DAEMON cut — not into an id it never had ("worktree not found").
    #[test]
    fn a_task_typed_on_the_stand_in_launches_into_the_real_checkout() {
        with_seeded_presets(|| {
            let mut app = App::new();
            let mut out = Vec::new();
            let (placeholder, req_id) = type_preset_task(&mut app, &mut out);

            modal_worktree_created(&mut app, req_id, &mut out);
            assert!(!app.tree.worktrees.iter().any(|w| w.id == placeholder));
            assert!(
                matches!(
                    &app.overlay,
                    Some(Overlay::Prompt(prompt))
                        if matches!(&prompt.kind, PromptKind::AgentPresetTask { worktree, .. } if worktree.0 == "w2")
                            && prompt.input.as_str() == "Fix auth"
                ),
                "the box is addressed to the real checkout, text kept: {:?}",
                app.overlay
            );
            assert!(out.is_empty(), "the Ack sends nothing: {out:?}");

            press(&mut app, KeyCode::Enter, KeyModifiers::NONE, &mut out);
            assert!(app.overlay.is_none(), "launching closes the box");
            assert!(
                matches!(
                    out.as_slice(),
                    [ClientRequest::CreateAgent {
                        worktree,
                        starting_prompt: Some(text),
                        ..
                    }] if worktree.0 == "w2" && text == "Be strict.\n\nFix auth\n\nRun the tests."
                ),
                "one create, in w2: {out:?}"
            );
        });
    }

    /// The same with the list still open when the checkout lands: the
    /// list is readdressed, and the task box it opens names the real row.
    #[test]
    fn a_presets_list_open_on_the_stand_in_follows_it_to_the_real_row() {
        with_seeded_presets(|| {
            let mut app = App::new();
            let mut out = Vec::new();
            let (placeholder, req_id) = stage_modal(&mut app, &mut out);
            press(&mut app, KeyCode::Char('e'), KeyModifiers::NONE, &mut out);
            assert!(
                matches!(&app.overlay, Some(Overlay::AgentPresets(view)) if view.worktree == placeholder),
                "{:?}",
                app.overlay
            );

            modal_worktree_created(&mut app, req_id, &mut out);
            assert!(
                matches!(&app.overlay, Some(Overlay::AgentPresets(view)) if view.worktree.0 == "w2"),
                "{:?}",
                app.overlay
            );
            press(&mut app, KeyCode::Enter, KeyModifiers::NONE, &mut out);
            assert!(
                matches!(
                    &app.overlay,
                    Some(Overlay::Prompt(prompt))
                        if matches!(&prompt.kind, PromptKind::AgentPresetTask { worktree, .. } if worktree.0 == "w2")
                ),
                "{:?}",
                app.overlay
            );
            // An empty task launches on the preset's prefix and postfix.
            press(&mut app, KeyCode::Enter, KeyModifiers::NONE, &mut out);
            assert!(app.overlay.is_none(), "{:?}", app.overlay);
            assert!(
                matches!(
                    out.as_slice(),
                    [ClientRequest::CreateAgent {
                        worktree,
                        starting_prompt: Some(_),
                        ..
                    }] if worktree.0 == "w2"
                ),
                "{out:?}"
            );
        });
    }

    /// Enter on the task before the DAEMON has answered: nothing can go
    /// out under the stand-in's id, so the launch waits on the checkout's
    /// own Ack — its session row up under the new checkout meanwhile, the
    /// pane showing it starting, nothing sent — and goes out into the
    /// real checkout the moment the Ack names it. A second launch while
    /// one waits, waits.
    #[test]
    fn a_preset_run_before_the_ack_waits_for_the_checkout() {
        with_seeded_presets(|| {
            let mut app = App::new();
            let mut out = Vec::new();
            let (placeholder, req_id) = type_preset_task(&mut app, &mut out);

            press(&mut app, KeyCode::Enter, KeyModifiers::NONE, &mut out);
            assert!(app.overlay.is_none(), "{:?}", app.overlay);
            assert!(out.is_empty(), "nothing under a made-up id: {out:?}");
            assert_ne!(
                app.flash.as_deref(),
                Some("worktree is still being created")
            );
            let agent = match app.pending.get(&req_id) {
                Some(PendingIntent::SelectCreatedWorktree {
                    launch: Some(draft),
                    ..
                }) => draft
                    .placeholder
                    .clone()
                    .expect("the waiting launch has its stand-in row"),
                other => panic!("the launch rides the checkout's intent: {other:?}"),
            };
            assert!(app.is_placeholder_agent(&agent));
            let sessions = app.visible_session_rows();
            assert_eq!(sessions.len(), 1, "{sessions:?}");
            assert_eq!(sessions[0].sref(), Some(SessionRef::Agent(agent.clone())));
            assert!(app.tree.agents.iter().any(|a| a.id == agent
                && a.worktree_id == placeholder
                && a.kind == AgentKind::Claude
                && a.model.as_deref() == Some("opus")));
            assert_eq!(
                app.term.as_ref().map(|t| (t.sref.clone(), t.booting)),
                Some((SessionRef::Agent(agent.clone()), true)),
                "the pane shows the row starting"
            );
            assert_eq!(app.focus, Focus::Sessions);

            // A second launch while the first waits is refused as before.
            press(&mut app, KeyCode::Char('e'), KeyModifiers::NONE, &mut out);
            press(&mut app, KeyCode::Enter, KeyModifiers::NONE, &mut out);
            press(&mut app, KeyCode::Enter, KeyModifiers::NONE, &mut out);
            assert_eq!(
                app.flash.as_deref(),
                Some("worktree is still being created")
            );
            assert_eq!(app.visible_session_rows().len(), 1, "no second stand-in");
            assert!(out.is_empty(), "{out:?}");

            modal_worktree_created(&mut app, req_id, &mut out);
            let create_id = match out.as_slice() {
                [ClientRequest::CreateAgent {
                    req_id,
                    worktree,
                    name,
                    kind: AgentKind::Claude,
                    model: Some(model),
                    effort: Some(effort),
                    auto_title: true,
                    starting_prompt: Some(text),
                    ..
                }] if worktree.0 == "w2"
                    && model == "opus"
                    && effort == "high"
                    && text == "Be strict.\n\nFix auth\n\nRun the tests." =>
                {
                    assert_eq!(name, "agent-1", "the stand-in is not a taken name");
                    *req_id
                }
                other => panic!("one CreateAgent in w2: {other:?}"),
            };
            out.clear();
            assert!(
                app.tree
                    .agents
                    .iter()
                    .any(|a| a.id == agent && a.worktree_id.0 == "w2"),
                "the row moved under the real checkout"
            );
            assert!(
                app.is_placeholder_agent(&agent),
                "still a stand-in until its own Ack"
            );
            assert!(
                matches!(
                    app.pending.get(&create_id),
                    Some(PendingIntent::AttachCreatedWithCloudRetry {
                        kind: PromptKind::AgentPresetTask { worktree, .. },
                        placeholder: Some(stand_in),
                        ..
                    }) if worktree.0 == "w2" && *stand_in == agent
                ),
                "{:?}",
                app.pending
            );

            hse(
                &mut app,
                ServerEvent::EntityUpserted {
                    entity: Entity::Agent(real_agent("a9", "w2")),
                },
            );
            handle_server_event(
                &mut app,
                ServerEvent::Ack {
                    req_id: create_id,
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
            assert!(
                !app.tree.agents.iter().any(|a| a.id == agent),
                "the stand-in is gone"
            );
            assert!(app.pending.is_empty(), "{:?}", app.pending);
            assert!(
                out.iter().any(|r| matches!(r, ClientRequest::Attach { session, .. } if session == &SessionRef::Agent(AgentId("a9".into())))),
                "the real session is attached: {out:?}"
            );
            assert_eq!(app.focus, Focus::Terminal, "a preset launch takes the pane");
            assert!(app.term_locked);
        });
    }

    /// The checkout is refused with a launch waiting on it: both stand-in
    /// rows go, the pane with them, the cursor and FOCUS go back to where
    /// `n` was pressed, and the NEW WORKTREE box comes back with the name
    /// — no create ever went out.
    #[test]
    fn a_refused_checkout_takes_the_waiting_launch_down_with_it() {
        with_seeded_presets(|| {
            let mut app = App::new();
            let mut out = Vec::new();
            let (placeholder, req_id) = type_preset_task(&mut app, &mut out);
            press(&mut app, KeyCode::Enter, KeyModifiers::NONE, &mut out);
            let agent = match app.pending.get(&req_id) {
                Some(PendingIntent::SelectCreatedWorktree {
                    launch: Some(draft),
                    ..
                }) => draft.placeholder.clone().expect("staged"),
                other => panic!("{other:?}"),
            };
            assert_eq!(app.visible_session_rows().len(), 1);

            handle_server_event(
                &mut app,
                ServerEvent::Error {
                    req_id: Some(req_id),
                    message: "branch exists".into(),
                },
                &mut out,
            );
            assert_eq!(worktree_branches(&app), ["main"]);
            assert!(
                !app.tree
                    .agents
                    .iter()
                    .any(|a| a.id == agent || a.worktree_id == placeholder),
                "the waiting row went with the checkout"
            );
            assert!(app.pending.is_empty(), "{:?}", app.pending);
            assert_eq!(app.selected_worktree().map(|w| w.id.0.as_str()), Some("w1"));
            assert_eq!(app.focus, Focus::Worktrees, "back where n was pressed");
            assert_eq!(app.flash.as_deref(), Some("branch exists"));
            assert!(
                matches!(
                    &app.overlay,
                    Some(Overlay::Prompt(prompt))
                        if matches!(&prompt.kind, PromptKind::NewWorktree { .. })
                            && prompt.input.as_str() == "feat"
                ),
                "{:?}",
                app.overlay
            );
            assert_ne!(
                app.term.as_ref().map(|t| t.sref.clone()),
                Some(SessionRef::Agent(agent)),
                "the pane no longer shows the stand-in"
            );
            assert!(
                !out.iter().any(|r| matches!(
                    r,
                    ClientRequest::CreateAgent { .. } | ClientRequest::Input { .. }
                )),
                "{out:?}"
            );
        });
    }

    // ---- a PR SESSION into a checkout that does not exist yet ----

    /// `n` on the OPEN PRS row for #7 opens the PR SESSION picker, and
    /// Enter on its first row (Claude) launches — no box in between. The
    /// checkout row for the head branch and the session row are up, and
    /// nothing but the `CreatePrAgent` went to the DAEMON. Returns the
    /// stand-in ids the intent carries and the request id.
    fn stage_pr_session(app: &mut App, out: &mut Vec<ClientRequest>) -> (PlaceholderRows, u64) {
        seed_tree(app);
        seed_open_prs(app, &[(7, "Attach links")]);
        app.focus = Focus::Worktrees;
        // The checkouts come first; the pull request is the row after.
        app.sel_worktree = 1;
        super::super::open_pr_agent_picker(app);
        assert!(
            matches!(app.overlay, Some(Overlay::Menu(_))),
            "the PR harness picker is up: {:?}",
            app.overlay
        );
        press(app, KeyCode::Enter, KeyModifiers::NONE, out);
        assert!(
            app.overlay.is_none(),
            "the picker's row launches, with no box after it: {:?}",
            app.overlay
        );
        let creates: Vec<&ClientRequest> = out
            .iter()
            .filter(|r| matches!(r, ClientRequest::CreatePrAgent { .. }))
            .collect();
        let req_id = match creates.as_slice() {
            [ClientRequest::CreatePrAgent { req_id, head, .. }] if head == "pr-7-head" => *req_id,
            other => panic!("one CreatePrAgent for pr-7-head: {other:?}"),
        };
        assert!(
            !out.iter().any(|r| matches!(
                r,
                ClientRequest::Attach { .. }
                    | ClientRequest::Input { .. }
                    | ClientRequest::CreateWorktree { .. }
                    | ClientRequest::PrewarmWorktreeSessions { .. }
            )),
            "nothing under a made-up id: {out:?}"
        );
        let rows = match app.pending.get(&req_id) {
            Some(PendingIntent::AttachCreatedPrSession { placeholder, .. }) => placeholder.clone(),
            other => panic!("the intent carries the stand-ins: {other:?}"),
        };
        out.clear();
        (rows, req_id)
    }

    /// No box asks for a name, so the stand-in row wears the generated
    /// one from the start, and the create carries it with AUTO-TITLE on —
    /// the session names itself from its first prompt.
    #[test]
    fn the_pr_stand_in_row_wears_the_generated_name_the_create_carries() {
        with_default_config(|| {
            let mut app = App::new();
            let mut out = Vec::new();
            seed_tree(&mut app);
            seed_open_prs(&mut app, &[(7, "Attach links")]);
            app.focus = Focus::Worktrees;
            app.sel_worktree = 1;
            super::super::open_pr_agent_picker(&mut app);
            press(&mut app, KeyCode::Enter, KeyModifiers::NONE, &mut out);
            assert!(app.overlay.is_none(), "{:?}", app.overlay);

            let sessions = app.visible_session_rows();
            assert_eq!(sessions.len(), 1, "{sessions:?}");
            let shown = sessions[0].name().to_string();
            assert!(
                out.iter().any(|r| matches!(
                    r,
                    ClientRequest::CreatePrAgent { name, auto_title: true, .. } if *name == shown
                )),
                "the row says {shown:?}: {out:?}"
            );
        });
    }

    /// The DAEMON's answer in its own order: the checkout's upsert adopts
    /// the stand-in worktree (one row on the branch, not two, the cursor
    /// still on it, the session stand-in now under the real checkout);
    /// then the session's upsert and the Ack naming it turn the session
    /// stand-in into the created row, attached and entered as every
    /// picker-walked launch is.
    #[test]
    fn the_upserts_and_the_ack_turn_the_pr_stand_ins_into_the_real_rows() {
        with_default_config(|| {
            let mut app = App::new();
            let mut out = Vec::new();
            let (rows, req_id) = stage_pr_session(&mut app, &mut out);

            seed_feat_worktree(&mut app, "w3", "pr-7-head");
            assert_eq!(worktree_branches(&app), ["main", "pr-7-head"]);
            assert!(!app.tree.worktrees.iter().any(|w| w.id == rows.worktree));
            assert_eq!(
                app.selected_worktree().map(|w| w.id.0.as_str()),
                Some("w3"),
                "the cursor stayed on the row"
            );
            assert!(!app.is_placeholder_worktree(&WorktreeId("w3".into())));
            let stand_in = app
                .tree
                .agents
                .iter()
                .find(|a| a.id == rows.agent)
                .expect("the session stand-in is still up");
            assert_eq!(stand_in.worktree_id.0, "w3", "under the real checkout");
            assert!(app.is_placeholder_agent(&rows.agent));
            assert_eq!(app.visible_session_rows().len(), 1);

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
            assert!(app.pending.is_empty());
            assert!(
                !app.tree.agents.iter().any(|a| a.id == rows.agent),
                "the stand-in went"
            );
            let sessions = app.visible_session_rows();
            assert_eq!(sessions.len(), 1, "{sessions:?}");
            assert_eq!(
                app.selected_session().map(|a| a.id.0).as_deref(),
                Some("a9")
            );
            assert_eq!(app.focus, Focus::Terminal);
            assert!(app.term_locked);
            let a9 = SessionRef::Agent(AgentId("a9".into()));
            assert!(
                out.iter()
                    .any(|r| matches!(r, ClientRequest::Attach { session, .. } if *session == a9)),
                "{out:?}"
            );
        });
    }

    /// The Ack outran both upserts (they cross on different DAEMON tasks):
    /// no stand-in outlives the intent that marked it one — both go — and
    /// the cursor lands on the real rows as they arrive.
    #[test]
    fn an_ack_ahead_of_the_upserts_leaves_no_pr_stand_in_behind() {
        with_default_config(|| {
            let mut app = App::new();
            let mut out = Vec::new();
            let (rows, req_id) = stage_pr_session(&mut app, &mut out);

            handle_server_event(
                &mut app,
                ServerEvent::Ack {
                    req_id,
                    created: Some(EntityId::Agent(AgentId("a9".into()))),
                },
                &mut out,
            );
            assert_eq!(worktree_branches(&app), ["main"]);
            assert!(!app.tree.agents.iter().any(|a| a.id == rows.agent));
            assert!(app.pending.is_empty());
            assert_eq!(
                app.select_when_seen,
                Some(SessionRef::Agent(AgentId("a9".into())))
            );

            seed_feat_worktree(&mut app, "w3", "pr-7-head");
            hse(
                &mut app,
                ServerEvent::EntityUpserted {
                    entity: Entity::Agent(real_agent("a9", "w3")),
                },
            );
            assert_eq!(worktree_branches(&app), ["main", "pr-7-head"]);
            assert_eq!(app.selected_worktree().map(|w| w.id.0.as_str()), Some("w3"));
            assert_eq!(
                app.selected_session().map(|a| a.id.0).as_deref(),
                Some("a9")
            );
        });
    }

    /// The DAEMON could not fetch the branch: both stand-ins go, the
    /// message flashes, and nothing is left pending.
    #[test]
    fn a_refused_pr_session_takes_both_stand_ins_down() {
        with_default_config(|| {
            let mut app = App::new();
            let mut out = Vec::new();
            let (rows, req_id) = stage_pr_session(&mut app, &mut out);

            handle_server_event(
                &mut app,
                ServerEvent::Error {
                    req_id: Some(req_id),
                    message: "could not fetch pull request #7".into(),
                },
                &mut out,
            );
            assert_eq!(worktree_branches(&app), ["main"]);
            assert!(!app.tree.agents.iter().any(|a| a.id == rows.agent));
            assert!(app.pending.is_empty());
            assert_eq!(
                app.flash.as_deref(),
                Some("could not fetch pull request #7")
            );
            assert!(
                !out.iter().any(|r| matches!(r, ClientRequest::Input { .. })),
                "{out:?}"
            );
        });
    }

    /// The checkout was cut but the CLI spawn failed: the session
    /// stand-in goes alone — the checkout row is real and stays.
    #[test]
    fn a_pr_session_refused_after_its_checkout_was_cut_keeps_the_checkout() {
        with_default_config(|| {
            let mut app = App::new();
            let mut out = Vec::new();
            let (rows, req_id) = stage_pr_session(&mut app, &mut out);

            seed_feat_worktree(&mut app, "w3", "pr-7-head");
            handle_server_event(
                &mut app,
                ServerEvent::Error {
                    req_id: Some(req_id),
                    message: "no such CLI".into(),
                },
                &mut out,
            );
            assert_eq!(worktree_branches(&app), ["main", "pr-7-head"]);
            assert!(!app.tree.agents.iter().any(|a| a.id == rows.agent));
            assert!(app.pending.is_empty());
            assert!(!app.tree.agents.iter().any(|a| a.worktree_id.0 == "w3"));
        });
    }

    /// The head branch already has a checkout: the DAEMON spawns straight
    /// into it, so nothing is staged — the launch is the plain
    /// `AttachCreated` it always was.
    #[test]
    fn a_pr_session_into_a_checkout_already_on_its_branch_stages_nothing() {
        with_default_config(|| {
            let mut app = App::new();
            let mut out = Vec::new();
            seed_tree(&mut app);
            seed_feat_worktree(&mut app, "w2", "pr-7-head");
            seed_open_prs(&mut app, &[(7, "Attach links")]);
            app.focus = Focus::Worktrees;
            // The pull request's row; its checkout is the row under it.
            app.sel_worktree = 1;
            assert_eq!(app.selected_worktree_pr().map(|p| p.number), Some(7));
            assert_eq!(
                app.worktree_row_of(&WorktreeId("w2".into())),
                Some(2),
                "the checkout on the head branch lists under the pull request"
            );
            super::super::open_pr_agent_picker(&mut app);
            press(&mut app, KeyCode::Enter, KeyModifiers::NONE, &mut out);

            assert!(
                out.iter()
                    .any(|r| matches!(r, ClientRequest::CreatePrAgent { head, .. } if head == "pr-7-head")),
                "{out:?}"
            );
            assert_eq!(worktree_branches(&app), ["main", "pr-7-head"]);
            assert!(app
                .pending
                .values()
                .all(|i| matches!(i, PendingIntent::AttachCreated { .. })));
            assert!(!app
                .tree
                .agents
                .iter()
                .any(|a| app.is_placeholder_agent(&a.id)));
        });
    }

    /// A second PR SESSION on the same pull request while its checkout is
    /// still being cut waits, as every launch into a stand-in does:
    /// nothing is sent and nothing else goes up.
    #[test]
    fn a_second_pr_session_while_the_checkout_is_being_cut_waits() {
        with_default_config(|| {
            let mut app = App::new();
            let mut out = Vec::new();
            let (rows, _) = stage_pr_session(&mut app, &mut out);

            // Back on the pull request's row — the stand-in checkout sits
            // under it, so the pull request is the row after the root.
            app.sel_worktree = 1;
            assert_eq!(app.selected_worktree_pr().map(|p| p.number), Some(7));
            super::super::open_pr_agent_picker(&mut app);
            press(&mut app, KeyCode::Enter, KeyModifiers::NONE, &mut out);
            assert_eq!(
                app.flash.as_deref(),
                Some("worktree is still being created")
            );
            assert!(
                !out.iter()
                    .any(|r| matches!(r, ClientRequest::CreatePrAgent { .. })),
                "{out:?}"
            );
            assert_eq!(worktree_branches(&app), ["main", "pr-7-head"]);
            assert_eq!(
                app.tree
                    .agents
                    .iter()
                    .filter(|a| a.worktree_id == rows.worktree)
                    .count(),
                1
            );
        });
    }

    // ---- a launch the user walked away from while the DAEMON worked ----

    /// The DAEMON's answer to a PR SESSION's `CreatePrAgent`, in its own
    /// order: the checkout's upsert, the session's, then the Ack.
    fn pr_session_created(app: &mut App, req_id: u64, out: &mut Vec<ClientRequest>) {
        seed_feat_worktree(app, "w3", "pr-7-head");
        hse(
            app,
            ServerEvent::EntityUpserted {
                entity: Entity::Agent(real_agent("a9", "w3")),
            },
        );
        handle_server_event(
            app,
            ServerEvent::Ack {
                req_id,
                created: Some(EntityId::Agent(AgentId("a9".into()))),
            },
            out,
        );
    }

    fn attached(out: &[ClientRequest], id: &str) -> bool {
        let sref = SessionRef::Agent(AgentId(id.into()));
        out.iter()
            .any(|r| matches!(r, ClientRequest::Attach { session, .. } if *session == sref))
    }

    /// Input that moves nothing is not a manual move — the pointer
    /// crossing the screen, ↓ on the last row. The Ack follows as ever:
    /// the created session selected, the pane entered and locked.
    #[test]
    fn input_that_moves_nothing_keeps_the_follow() {
        with_default_config(|| {
            let mut app = App::new();
            let mut out = Vec::new();
            let (_, req_id) = stage_pr_session(&mut app, &mut out);

            handle_terminal_event(
                &mut app,
                Event::Mouse(MouseEvent {
                    kind: MouseEventKind::Moved,
                    column: 5,
                    row: 5,
                    modifiers: KeyModifiers::NONE,
                }),
                &mut out,
            );
            assert!(app.left_behind.is_empty());
            out.clear();

            pr_session_created(&mut app, req_id, &mut out);

            assert_eq!(app.selected_worktree().map(|w| w.id.0.as_str()), Some("w3"));
            assert_eq!(
                app.selected_session().map(|a| a.id.0).as_deref(),
                Some("a9")
            );
            assert_eq!(app.focus, Focus::Terminal);
            assert!(app.term_locked);
            assert!(attached(&out, "a9"), "{out:?}");
        });
    }

    /// The project on the BOTTOM row of the PROJECTS PANEL and its two
    /// rows: the second project, whose session finished a minute ago,
    /// heads the column; `demo`, never run, is the last row, and the
    /// cursor is on it.
    fn seed_project_that_ran_first(app: &mut App) {
        use nebula_core::{Project, ProjectId, Worktree};
        seed_tree(app);
        hse(
            app,
            ServerEvent::EntityUpserted {
                entity: Entity::Project(Project {
                    id: ProjectId("p2".into()),
                    name: "two".into(),
                    repo_path: "/tmp/two".into(),
                    sort_order: 1,
                }),
            },
        );
        hse(
            app,
            ServerEvent::EntityUpserted {
                entity: Entity::Worktree(Worktree {
                    id: WorktreeId("w9".into()),
                    project_id: ProjectId("p2".into()),
                    path: "/tmp/two".into(),
                    branch: "main".into(),
                    is_main: true,
                    sort_order: 0,
                }),
            },
        );
        let mut ran = real_agent("a2", "w9");
        ran.status = AgentStatus::Finished;
        ran.status_changed_at = crate::app::now_ms() - 60_000;
        hse(
            app,
            ServerEvent::EntityUpserted {
                entity: Entity::Agent(ran),
            },
        );
        assert_eq!(project_names(app), ["two", "demo"]);
        app.sel_project = 1;
        assert_eq!(
            app.selected_project().map(|p| p.name.as_str()),
            Some("demo")
        );
    }

    /// The PROJECTS PANEL's rows, top down.
    fn project_names(app: &App) -> Vec<String> {
        app.project_rows()
            .into_iter()
            .map(|i| app.tree.projects[i].name.clone())
            .collect()
    }

    /// `p` on the WORKTREES PANEL of the project on the BOTTOM row of the
    /// PROJECTS PANEL. The stand-in session's fresh stamp is the newest
    /// under any project, so the row moves to the top — and the cursor
    /// moves with it. `sel_project` is a row index: left alone it would
    /// highlight whichever project slid into the vacated bottom row, with
    /// that project's checkouts in the WORKTREES PANEL instead of the
    /// stand-in. The cursor keeps following through the Acks that turn
    /// the stand-ins into the real rows.
    #[test]
    fn the_stand_in_moves_its_project_to_the_top_and_the_cursor_follows() {
        with_default_config(|| {
            let mut app = App::new();
            let mut out = Vec::new();
            seed_project_that_ran_first(&mut app);
            let (branch, rows, req_id) = stage_launch(&mut app, &mut out);

            assert_eq!(
                project_names(&app),
                ["demo", "two"],
                "the launch moved demo to the top"
            );
            assert_eq!(app.sel_project, 0, "the cursor moved with it");
            assert_eq!(
                app.selected_project().map(|p| p.name.as_str()),
                Some("demo")
            );
            assert_eq!(
                app.selected_worktree().map(|w| w.id.clone()),
                Some(rows.worktree.clone()),
                "its WORKTREES PANEL holds the stand-in, selected"
            );
            assert_eq!(app.sel_session, 0);
            assert_eq!(app.focus, Focus::Worktrees);

            // The DAEMON's answers keep it there.
            let req_id = worktree_created(&mut app, &branch, req_id, &mut out);
            assert_eq!(
                app.selected_project().map(|p| p.name.as_str()),
                Some("demo")
            );
            assert_eq!(app.selected_worktree().map(|w| w.id.0.as_str()), Some("w3"));
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
            assert_eq!(project_names(&app), ["demo", "two"]);
            assert_eq!(app.sel_project, 0);
            assert_eq!(
                app.selected_project().map(|p| p.name.as_str()),
                Some("demo")
            );
            assert_eq!(app.selected_worktree().map(|w| w.id.0.as_str()), Some("w3"));
            assert_eq!(
                app.selected_session_row().and_then(|r| r.sref()),
                Some(SessionRef::Agent(AgentId("a9".into())))
            );
            assert_eq!(
                app.term.as_ref().map(|t| t.sref.clone()),
                Some(SessionRef::Agent(AgentId("a9".into()))),
                "the pane shows the session the prompt went into"
            );
        });
    }

    /// The same from the PROJECTS PANEL, where `p` launches into the
    /// selected checkout and nothing is staged: the project moves to the
    /// top only when the DAEMON's upsert lands, and the cursor follows
    /// through the upsert, the Ack and the first status change.
    #[test]
    fn a_launch_into_the_bottom_projects_checkout_keeps_the_cursor_on_it() {
        with_default_config(|| {
            let mut app = App::new();
            let mut out = Vec::new();
            seed_project_that_ran_first(&mut app);
            app.focus = Focus::Projects;
            assert_eq!(app.selected_worktree().map(|w| w.id.0.as_str()), Some("w1"));
            press(&mut app, KeyCode::Char('p'), KeyModifiers::NONE, &mut out);
            assert!(paste_into_overlay(&mut app, "Fix auth"));
            press(&mut app, KeyCode::Enter, KeyModifiers::NONE, &mut out);
            let req_id = match out.as_slice() {
                [ClientRequest::CreateAgent {
                    req_id, worktree, ..
                }] if worktree.0 == "w1" => *req_id,
                other => panic!("one CreateAgent in w1 and nothing else: {other:?}"),
            };
            out.clear();
            assert_eq!(project_names(&app), ["two", "demo"], "nothing staged");
            assert_eq!(app.sel_project, 1);

            hse(
                &mut app,
                ServerEvent::EntityUpserted {
                    entity: Entity::Agent(real_agent("a9", "w1")),
                },
            );
            assert_eq!(
                project_names(&app),
                ["demo", "two"],
                "the upsert moved demo up"
            );
            assert_eq!(app.sel_project, 0, "the cursor moved with it");
            handle_server_event(
                &mut app,
                ServerEvent::Ack {
                    req_id,
                    created: Some(EntityId::Agent(AgentId("a9".into()))),
                },
                &mut out,
            );
            hse(
                &mut app,
                ServerEvent::StatusChanged {
                    agent: AgentId("a9".into()),
                    status: AgentStatus::Running,
                    changed_at: crate::app::now_ms(),
                    unseen: false,
                },
            );
            assert_eq!(project_names(&app), ["demo", "two"]);
            assert_eq!(app.sel_project, 0);
            assert_eq!(
                app.selected_project().map(|p| p.name.as_str()),
                Some("demo")
            );
            assert_eq!(app.selected_worktree().map(|w| w.id.0.as_str()), Some("w1"));
            assert_eq!(
                app.selected_session_row().and_then(|r| r.sref()),
                Some(SessionRef::Agent(AgentId("a9".into())))
            );
            assert_eq!(app.focus, Focus::Projects, "quick_prompt_focus is off");
        });
    }
}
