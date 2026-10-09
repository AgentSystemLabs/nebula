//! Server events from the daemon, split away from the terminal input loop.

use super::*;

pub(crate) fn handle_server_event(app: &mut App, event: ServerEvent, out: &mut Vec<ClientRequest>) {
    match event {
        event @ ServerEvent::Snapshot { .. } => handle_snapshot_event(app, event, out),
        event @ ServerEvent::Scrollback { .. }
        | event @ ServerEvent::Output { .. }
        | event @ ServerEvent::SessionExited { .. }
        | event @ ServerEvent::KittyFlags { .. } => handle_terminal_event(app, event, out),
        event @ ServerEvent::StatusChanged { .. } => handle_status_event(app, event, out),
        event @ ServerEvent::Ack { .. } => handle_ack_event(app, event, out),
        event @ ServerEvent::EntityUpserted { .. }
        | event @ ServerEvent::EntityRemoved { .. }
        | event @ ServerEvent::FilesOpened { .. }
        | event @ ServerEvent::Metrics { .. }
        | event @ ServerEvent::OutputTail { .. }
        | event @ ServerEvent::AttachRefused { .. } => handle_tree_event(app, event, out),
        event @ ServerEvent::Error { .. } => handle_error_event(app, event, out),
        _ => {}
    }
}

fn handle_snapshot_event(app: &mut App, event: ServerEvent, out: &mut Vec<ClientRequest>) {
    let _ = &mut *out;
    match event {
        ServerEvent::Snapshot {
            projects,
            worktrees,
            agents,
            terminals,
            links,
            pr_seen,
            ui_state,
        } => {
            app.tree.projects = projects;
            app.tree.worktrees = worktrees;
            app.tree.agents = agents;
            app.tree.terminals = terminals;
            app.tree.links = links;
            app.pr_seen = pr_seen.into_iter().map(|s| (s.url, s.marker)).collect();
            // The cache painted rows for whatever the last run's tree had;
            // the ones this tree no longer has go, before they could be
            // written back.
            prune_pull_requests_to_tree(app);
            let session_restored = ui_state
                .as_deref()
                .is_some_and(|json| restore_ui_state(app, json));
            clamp_selections(app);
            refresh_palette(app);
            // Boot the restored worktree's sessions right away — the first
            // thing the user does after launch is walk into one of them.
            schedule_prewarm(app);
            // And ask for the restored project's open issues, so an `i`
            // straight after launch has rows to paint.
            crate::issues::schedule_prefetch(app);
            // The cursor came back on the session the user left on; bring
            // its terminal back with it, exactly as landing on the row would.
            // No debounce: a boot restores one remembered session once, so
            // there is no cursor sweep to wait out — only the user waiting
            // to see the screen they left. The LAUNCHER VIEW's PANE reads
            // the card its cursor is on, so a boot that remembered no
            // session fills it from the grid's own cursor rather than
            // sitting empty under a grid full of cards.
            if session_restored || app.launcher_grid() {
                preview_selected_now(app, out);
            }
            app.dirty = true;
        }
        _ => unreachable!("server event dispatcher passed the wrong event variant"),
    }
}

fn handle_terminal_event(app: &mut App, event: ServerEvent, out: &mut Vec<ClientRequest>) {
    let _ = &mut *out;
    match event {
        ServerEvent::Scrollback {
            session,
            base_seq,
            data,
        } => {
            if let Some(term) = &mut app.term {
                if term.sref == session {
                    // The session started after all (its checkout came
                    // back, its CLI was installed): the refusal is over, on
                    // the pane and in the status line.
                    term.refused = None;
                    if app
                        .refusal_flash
                        .as_ref()
                        .is_some_and(|(refused, _)| *refused == session)
                    {
                        if let Some((_, line)) = app.refusal_flash.take() {
                            if app.flash.as_deref() == Some(line.as_str()) {
                                app.flash = None;
                            }
                        }
                    }
                    // A replay continuing a kept screen lands on it; any
                    // other rebuilds the screen from scratch, and a
                    // selection anchored to the old cells goes with it.
                    let base_before = term.parser.screen().history_base();
                    if term.apply_scrollback(base_seq, &data) {
                        // The screen was rebuilt from scratch and its
                        // history lines start over. A finished selection
                        // goes; a drag under way is carried across by its
                        // screen rows — the replay lands on the same view
                        // — so the button, not the replay, ends it.
                        let base_after = term.parser.screen().history_base();
                        let rebase = |line: u64| {
                            if line >= base_before {
                                line - base_before + base_after
                            } else {
                                base_after.saturating_sub(base_before - line)
                            }
                        };
                        match &mut app.term_selection {
                            Some(sel) if sel.dragging => {
                                sel.anchor.1 = rebase(sel.anchor.1);
                                sel.head.1 = rebase(sel.head.1);
                            }
                            _ => app.term_selection = None,
                        }
                    }
                    // A replay is history: a clipboard write in it went out
                    // when it happened (or never reached this client), and
                    // redoing it now would clobber whatever the user has
                    // copied since.
                    term.take_clipboard();
                    app.dirty = true;
                }
            }
        }
        ServerEvent::Output { session, seq, data } => {
            if let Some(term) = &mut app.term {
                if term.sref == session {
                    term.apply_output(seq, &data);
                    // The program wrote "the clipboard" with OSC 52 —
                    // claude's fullscreen renderer over ssh, vim, tmux —
                    // and that landed here, in the emulation, not on any
                    // clipboard. Pass it on to the terminal the user is
                    // sitting at, the route nebula's own copy takes on a
                    // remote host.
                    if let Some(payload) = term.take_clipboard() {
                        app.pending_clipboard = Some(payload);
                        app.flash = Some("copied (via terminal)".into());
                    }
                    app.dirty = true;
                }
            }
        }
        ServerEvent::SessionExited { session, .. } => {
            if let Some(term) = &mut app.term {
                if term.sref == session {
                    term.exited = true;
                    app.dirty = true;
                }
            }
        }
        ServerEvent::KittyFlags { session, flags } => {
            if let Some(term) = &mut app.term {
                if term.sref == session {
                    term.kitty_flags = flags;
                }
            }
        }
        _ => unreachable!("server event dispatcher passed the wrong event variant"),
    }
}

fn handle_status_event(app: &mut App, event: ServerEvent, out: &mut Vec<ClientRequest>) {
    let _ = &mut *out;
    match event {
        ServerEvent::StatusChanged {
            agent,
            status,
            changed_at,
            unseen,
        } => {
            // A status flip reorders the sessions list — and, since
            // worktrees and projects sort on their sessions' stamps,
            // re-sorts those columns too.
            // Every cursor stays on the row it was on.
            let before = selection_snapshot(app);
            let mut went_red = false;
            if let Some(a) = app.tree.agents.iter_mut().find(|a| a.id == agent) {
                // The RUNNING / NEEDS FEEDBACK → FINISHED edge — the one
                // that raises UNSEEN — rings the DONE SOUND, whether or not
                // the session is on screen. A re-stamp of a finished row
                // and the startup Snapshot never get here.
                if status == nebula_core::AgentStatus::Finished
                    && matches!(
                        a.status,
                        nebula_core::AgentStatus::Running | nebula_core::AgentStatus::NeedsFeedback
                    )
                {
                    app.pending_ding = true;
                }
                // The edge *into* NEEDS FEEDBACK is the FEEDBACK SOUND's;
                // a re-stamp of a row already red is not.
                went_red = status == nebula_core::AgentStatus::NeedsFeedback
                    && a.status != nebula_core::AgentStatus::NeedsFeedback;
                a.status = status;
                a.status_changed_at = changed_at;
                a.unseen = unseen;
                app.dirty = true;
            }
            // The first turn in a session we just launched: its own stamp
            // leads the list from here, so the launch stops having to.
            if status != nebula_core::AgentStatus::Fresh
                && app.just_launched.as_ref() == Some(&agent)
            {
                app.just_launched = None;
            }
            // A turn that finished in the pane the user is looking at was
            // watched: clear it before it can count anywhere.
            let on_screen = app
                .term
                .as_ref()
                .is_some_and(|t| t.sref == SessionRef::Agent(agent.clone()));
            if unseen && on_screen {
                mark_agent_seen(app, &agent, out);
            }
            // A prompt in the pane the user is locked into typing at, with
            // the window focused, is already under their hands: a sound
            // there is noise. Previewing the pane from a panel is not
            // typing at it, and a window in the background can't be seen —
            // both ring, and the second is the whole point.
            let under_hands = on_screen && app.term_locked && app.window_focused;
            if went_red && !under_hands {
                if let Some(alert) = alerts::alert_for(&app.tree, &agent) {
                    app.pending_feedback.push(alert);
                }
            }
            // Nothing left any list, so this only re-seats the cursors.
            reconcile_selection_inner(app, before, out);
        }
        _ => unreachable!("server event dispatcher passed the wrong event variant"),
    }
}

fn handle_ack_event(app: &mut App, event: ServerEvent, out: &mut Vec<ClientRequest>) {
    let _ = &mut *out;
    match event {
        ServerEvent::Ack { req_id, created } => {
            // False for a create the user has navigated away from since
            // firing it (`App::left_behind`): the rows still become the
            // real ones, and nothing below moves a cursor, the pane or
            // FOCUS back to them.
            let follow = !app.left_behind.remove(&req_id);
            match (app.pending.remove(&req_id), created) {
                (Some(PendingIntent::AttachCreated { focus, placeholder }), Some(id)) => {
                    attach_created(app, id, focus, placeholder, follow, out);
                }
                (
                    Some(PendingIntent::AttachCreatedWithCloudRetry {
                        focus, placeholder, ..
                    }),
                    Some(id),
                ) => attach_created(app, id, focus, placeholder, follow, out),
                (
                    Some(PendingIntent::AttachCreatedPrSession {
                        focus, placeholder, ..
                    }),
                    created,
                ) => {
                    match created {
                        Some(EntityId::Agent(real)) => {
                            // The checkout's upsert usually adopted the
                            // stand-in worktree already; the Ack settles
                            // whatever order the two arrived in, then the
                            // session lands as any created one does.
                            placeholder::settle_pr_worktree(app, &placeholder, &real, out);
                            attach_created(
                                app,
                                EntityId::Agent(real),
                                focus,
                                Some(placeholder.agent),
                                follow,
                                out,
                            );
                        }
                        _ => placeholder::discard_pr(app, &placeholder, out),
                    }
                }
                (Some(PendingIntent::ReopenPromptOnError { note, .. }), _) => {
                    app.flash = Some(note);
                }
                (
                    Some(PendingIntent::ProjectPathSet {
                        project,
                        old_path,
                        new_path,
                    }),
                    _,
                ) => {
                    app.dismissed_repath_projects.remove(&project);
                    rekey_project_config(app, &old_path, &new_path);
                    app.bring_tab_forward(&project);
                    app.flash = Some(format!("project folder updated: {}", new_path.display()));
                }
                (
                    Some(PendingIntent::RunToggled {
                        branch,
                        started: true,
                    }),
                    created,
                ) => {
                    // The reply usually beats the broadcast upsert, so the
                    // row naming the command may not be in the tree yet:
                    // then the flash names it when the row lands.
                    match created {
                        Some(EntityId::Terminal(id)) => match run_command_of(app, &id) {
                            Some(command) => {
                                app.flash = Some(format!("▶ running {command} in {branch}"));
                            }
                            None => {
                                app.flash = Some(format!("▶ running in {branch}"));
                                app.run_flash_when_seen = Some((id, branch));
                            }
                        },
                        _ => app.flash = Some(format!("▶ running in {branch}")),
                    }
                }
                (
                    Some(PendingIntent::RunToggled {
                        branch,
                        started: false,
                    }),
                    _,
                ) => {
                    app.flash = Some(format!("■ stopped the run in {branch}"));
                }
                (Some(PendingIntent::SelectCreatedProject), Some(EntityId::Project(id))) => {
                    // Its upsert usually lands just before this Ack; if not,
                    // stash the id and select once it does.
                    if follow && !select_created_project(app, &id, out) {
                        app.select_project_when_seen = Some(id);
                    }
                }
                (
                    Some(PendingIntent::SelectCreatedWorktree {
                        placeholder,
                        launch,
                        ..
                    }),
                    Some(EntityId::Worktree(id)),
                ) => {
                    // The cursor and FOCUS landed on the stand-in when
                    // Enter was pressed; a cursor the user moved since
                    // stays where they put it. The prewarm that landing
                    // armed was skipped as the checkout was not on disk —
                    // it is now. A modal opened on the stand-in (`e`, the
                    // task box behind it) is readdressed to the real row
                    // inside `resolve_worktree`.
                    placeholder::resolve_worktree(app, &placeholder, &id);
                    if app.selected_worktree().is_some_and(|w| w.id == id) {
                        schedule_prewarm(app);
                    }
                    // A launch fired into the stand-in while the DAEMON
                    // worked goes out now, into the checkout it cut.
                    if let Some(mut draft) = launch {
                        draft.follow = follow;
                        placeholder::replay_launch(app, *draft, &id, out);
                    }
                }
                (
                    Some(PendingIntent::LaunchInCreatedWorktree {
                        launch,
                        text,
                        placeholder,
                        focus,
                    }),
                    Some(EntityId::Worktree(id)),
                ) => quick_launch::launch_in_created_worktree(
                    app,
                    launch,
                    text,
                    placeholder,
                    id,
                    follow,
                    focus,
                    out,
                ),
                // An OPTIMISTIC UPDATE the DAEMON went along with.
                (Some(PendingIntent::Undo(undo)), _) => optimistic::settled(app, undo),
                _ => {}
            }
            app.dirty = true;
        }
        _ => unreachable!("server event dispatcher passed the wrong event variant"),
    }
}

fn handle_tree_event(app: &mut App, event: ServerEvent, out: &mut Vec<ClientRequest>) {
    let _ = &mut *out;
    match event {
        // A straggler for a row deleted here a moment ago: the DAEMON sent
        // it before it got to the delete, and the row stays down.
        ServerEvent::EntityUpserted { entity } if optimistic::is_deleting(app, &entity) => {
            tracing::debug!(?entity, "upsert of a row being deleted — ignored");
        }
        ServerEvent::EntityUpserted { entity } => {
            // A checkout cut for a PR SESSION takes over its stand-in row
            // first: the snapshot below names rows by id, and the
            // stand-in's is about to become this one's.
            if let nebula_core::Entity::Worktree(w) = &entity {
                placeholder::adopt_worktree(app, w);
            }
            let before = selection_snapshot(app);
            // The `claude --cloud <task>` create runs in an ordinary pane
            // until the id it prints makes the row a Cloud row. That is the
            // cue to let the create PTY go and give the keyboard back: the
            // pane is the CLOUD SESSION PANEL from here, and Enter means
            // "open in the browser", not a keystroke into a process that
            // has already exited.
            let became_cloud = match &entity {
                nebula_core::Entity::Agent(a) if a.cloud_session_id.is_some() => {
                    let shown = app
                        .term
                        .as_ref()
                        .is_some_and(|t| t.sref == SessionRef::Agent(a.id.clone()));
                    let was_cloud = app
                        .tree
                        .agents
                        .iter()
                        .any(|x| x.id == a.id && x.cloud_session_id.is_some());
                    shown && !was_cloud
                }
                _ => false,
            };
            apply_upsert(app, entity);
            if became_cloud {
                detach_pane(app, out);
                if app.focus == Focus::Terminal {
                    leave_terminal_lock(app);
                }
            }
            // Cursors follow the row they were on across re-sorts and
            // re-homes; a row that left its list (archived away,
            // moved elsewhere) hands the cursor — and the terminal pane —
            // to its neighbor.
            reconcile_selection(app, before, out);
            // Fix the selection onto a session we just created, or follow
            // one we just moved into another worktree or project.
            land_pending_selection(app, out);
            // ...and onto a project we just added.
            if let Some(pid) = app.select_project_when_seen.clone() {
                if select_created_project(app, &pid, out) {
                    app.select_project_when_seen = None;
                }
            }
            // ...and onto a worktree we just created.
            if let Some(wt_id) = app.select_worktree_when_seen.clone() {
                if select_worktree_by_id(app, &wt_id, out) {
                    app.select_worktree_when_seen = None;
                }
            }
            refresh_palette(app);
            app.dirty = true;
        }
        ServerEvent::EntityRemoved { id } => {
            let before = selection_snapshot(app);
            apply_removal(app, &id);
            app.prune_term_cache();
            // The cursor that was on the removed row now sits on its
            // neighbor — show that neighbor's session/context.
            reconcile_selection(app, before, out);
            refresh_palette(app);
            app.dirty = true;
        }
        // `nebula open` in a session: the user asked to see these files.
        ServerEvent::FilesOpened { root, paths, .. } => {
            crate::file_tabs::open(app, root, paths);
        }
        ServerEvent::Metrics { req_id, snapshot } => {
            // Answered with Metrics, not Ack — clear the pending slot by hand.
            app.pending.remove(&req_id);
            if let Some(Overlay::Metrics(view)) = &mut app.overlay {
                view.snapshot = Some(snapshot.clone());
            }
            // The footer's readout keeps the latest reading either way.
            app.last_metrics = Some(snapshot);
            app.dirty = true;
        }
        ServerEvent::OutputTail {
            req_id,
            session,
            tail,
        } => {
            // Answered with OutputTail, not Ack — clear the slot by hand.
            app.pending.remove(&req_id);
            if let SessionRef::Terminal(id) = session {
                land_terminal_tail(app, id, tail);
            }
        }
        ServerEvent::AttachRefused { session, message } => {
            // The pane waiting on this session says why, in full; the
            // status line gets the first sentence, which fits it.
            let line = refusal_flash(&message);
            app.flash = Some(line.clone());
            app.refusal_flash = Some((session.clone(), line));
            if let Some(term) = app.term.as_mut().filter(|t| t.sref == session) {
                term.refused = Some(message);
            }
            if app.overlay.is_none() {
                if let Some(project) = app.project_of_session(&session).cloned() {
                    prompt_for_missing_project_path(app, &project);
                }
            }
            app.dirty = true;
        }
        _ => unreachable!("server event dispatcher passed the wrong event variant"),
    }
}

fn handle_error_event(app: &mut App, event: ServerEvent, out: &mut Vec<ClientRequest>) {
    let _ = &mut *out;
    match event {
        ServerEvent::Error { req_id, message } => {
            // A failed request's intent never gets an Ack; clear it — and if
            // it was an optimistic worktree delete, put the rows back. A
            // failed Cloud launch reopens its populated task editor.
            if let Some(id) = &req_id {
                app.left_behind.remove(id);
            }
            match req_id.and_then(|id| app.pending.remove(&id)) {
                Some(PendingIntent::DeleteWorktree(rollback)) => {
                    restore_worktree_rows(app, rollback)
                }
                // A rename, an archive or a delete shown on the keypress
                // and then refused: the row goes back to what it was.
                Some(PendingIntent::Undo(undo)) => optimistic::undo(app, undo, out),
                Some(PendingIntent::AttachCreatedWithCloudRetry {
                    kind,
                    task: text,
                    placeholder,
                    ..
                }) => {
                    // The stand-in session a QUICK PROMPT put up comes
                    // down with the refusal; its checkout is real and stays.
                    if let Some(stand_in) = placeholder {
                        placeholder::discard_agent(app, &stand_in, out);
                    }
                    reopen_prompt_with(app, kind, text);
                }
                Some(PendingIntent::ReopenPromptOnError { kind, text, .. }) => {
                    reopen_prompt_with(app, kind, text);
                }
                Some(PendingIntent::ProjectPathSet {
                    project,
                    old_path,
                    new_path,
                }) => {
                    reopen_prompt_with(
                        app,
                        PromptKind::SetProjectPath {
                            id: project,
                            old_path,
                        },
                        new_path.display().to_string(),
                    );
                }
                // The worktree the QUICK PROMPT wanted to cut first was
                // refused (a fetch that failed, a branch that exists):
                // both stand-in rows go, and the box comes back, its
                // target untouched, for a retry.
                Some(PendingIntent::LaunchInCreatedWorktree {
                    launch,
                    text,
                    placeholder,
                    ..
                }) => {
                    placeholder::discard(app, &placeholder, out);
                    reopen_prompt_with(app, PromptKind::QuickPrompt(launch), text);
                }
                // The NEW WORKTREE modal's checkout was refused: its
                // stand-in row goes, and with it the FOCUS it took — the
                // SESSIONS PANEL of a row that is no longer there — goes
                // back to the panel the box was opened from, when the
                // cursor was still on the row. The box comes back with the
                // name for a retry.
                Some(PendingIntent::SelectCreatedWorktree {
                    placeholder,
                    focus,
                    prompt,
                    text,
                    launch,
                }) => {
                    // A launch waiting on the checkout goes down with it:
                    // its stand-in session row, and the pane showing it.
                    if let Some(agent) = launch.and_then(|draft| draft.placeholder) {
                        placeholder::discard_agent(app, &agent, out);
                    }
                    if placeholder::discard_worktree(app, &placeholder, out) {
                        app.focus = focus;
                    }
                    reopen_prompt_with(app, prompt, text);
                }
                // A PR SESSION was refused: its stand-ins go — both rows,
                // or the session's alone once the checkout was cut — and
                // the box it was sent from comes back with its text.
                Some(PendingIntent::AttachCreatedPrSession {
                    placeholder,
                    reopen,
                    pr_url,
                    ..
                }) => {
                    let on_stand_in = app
                        .selected_worktree()
                        .is_some_and(|w| w.id == placeholder.worktree);
                    placeholder::discard_pr(app, &placeholder, out);
                    // A cursor still on the refused checkout goes back to
                    // the pull request it was launched from, not to
                    // wherever `restore_context` falls back to.
                    if on_stand_in {
                        if let Some(row) = app.open_pr_row_of(&pr_url) {
                            if app.sel_worktree != row {
                                select_worktree_row(app, row, out);
                            }
                        }
                    }
                    match reopen {
                        // The refusal lands seconds after Enter: a modal
                        // opened meanwhile keeps its own text, and this
                        // box's waits for the next `p` on the pull request.
                        Some((_, text)) if app.overlay.is_some() => {
                            if !text.is_empty() {
                                app.parked_pr_prompt = Some((pr_url, text));
                            }
                        }
                        Some((kind, text)) => reopen_prompt_with(app, kind, text),
                        None => {}
                    }
                }
                // A launch that waited on the NEW WORKTREE modal's checkout
                // and was refused once it went out: its stand-in row goes,
                // the checkout is real and stays.
                Some(PendingIntent::AttachCreated {
                    placeholder: Some(stand_in),
                    ..
                }) => placeholder::discard_agent(app, &stand_in, out),
                _ => {}
            }
            app.flash = Some(message);
            app.dirty = true;
        }
        _ => unreachable!("server event dispatcher passed the wrong event variant"),
    }
}
