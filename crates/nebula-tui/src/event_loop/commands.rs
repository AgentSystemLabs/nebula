//! Extracted event-loop helper section.

use super::*;

pub(crate) fn submit_prompt_now(app: &mut App, kind: PromptKind, out: &mut Vec<ClientRequest>) {
    open_prompt(app, kind);
    if let Some(Overlay::Prompt(prompt)) = app.overlay.take() {
        submit_prompt(app, prompt, out);
    }
}

pub(crate) fn submit_prompt(app: &mut App, prompt: PromptDialog, out: &mut Vec<ClientRequest>) {
    let Some((prompt, value)) = prompt_value_or_cancel(app, prompt) else {
        return;
    };
    dispatch_prompt(app, prompt, value, out);
}

fn prompt_value_or_cancel(app: &mut App, prompt: PromptDialog) -> Option<(PromptDialog, String)> {
    let value = prompt.input.trim().to_string();
    // An ISSUE SESSION's box may be sent empty: the issue is the task.
    let value = match &prompt.kind {
        PromptKind::QuickPrompt(launch) if value.is_empty() => {
            launch.default_task().unwrap_or(value)
        }
        _ => value,
    };
    // A cloud session cannot start without its task. Keep the multiline
    // dialog open on validation so the user can correct it in place (the
    // QUICK PROMPT opts out of the empty half of that — see below).
    if prompt.is_multiline() {
        // The cloud prompts and a preset's task share the bounds (the text
        // crosses the same argv), not the wording.
        let (needs, what) = match &prompt.kind {
            // A preset's task is optional: its prefix and postfix make a
            // first prompt on their own, sized below all the same.
            PromptKind::AgentPresetTask { .. } => (None, "task"),
            // An empty quick prompt starts the CLI with no first prompt
            // (`QuickLaunch::launches_empty`, below). Only its cloud box
            // needs the task, and that one is a change of mind rather than
            // a half-finished launch: it falls through to the generic
            // empty-input cancel below instead of holding the box open.
            PromptKind::QuickPrompt(_) => (None, "prompt"),
            // An empty comment is a change of mind too.
            PromptKind::PrComment { .. } => (None, "comment"),
            // An empty comment box likewise: the cancel below puts the
            // ISSUES MODAL back.
            PromptKind::IssueComment { .. } => (None, "comment"),
            // And an empty follow-up: the turn is simply not sent.
            PromptKind::FollowUp { .. } => (None, "follow-up"),
            _ => (Some("Claude Cloud needs a task"), "Claude Cloud task"),
        };
        let error = if let Some(needs) = needs.filter(|_| value.is_empty()) {
            Some(needs.to_string())
        } else if value.contains('\0') {
            Some(format!("{what} cannot contain NUL bytes"))
        } else if value.len() > MAX_CLOUD_PROMPT_BYTES {
            Some(format!(
                "{what} is too long (max {} KiB)",
                MAX_CLOUD_PROMPT_BYTES / 1024
            ))
        } else if let Some(composed) = match &prompt.kind {
            // The wrapped text crosses the same argv as the task alone, so
            // it gets the same ceiling — a long prefix can push a valid
            // task over it.
            PromptKind::AgentPresetTask { preset, .. } => Some(preset.compose(&value)),
            PromptKind::QuickPrompt(launch) if launch.preset.is_some() => {
                Some(launch.compose(&value))
            }
            _ => None,
        } {
            (composed.len() > MAX_CLOUD_PROMPT_BYTES).then(|| {
                format!(
                    "prefix + task + postfix is too long (max {} KiB)",
                    MAX_CLOUD_PROMPT_BYTES / 1024
                )
            })
        } else {
            None
        };
        if let Some(error) = error {
            app.flash = Some(error);
            app.overlay = Some(Overlay::Prompt(prompt));
            return None;
        }
    }
    // An empty worktree name falls back to the random branch the prompt
    // offered, an empty project name undoes the rename — the row goes back
    // to the folder's own name, which is the only way back from a rename —
    // an empty typed setting is that row's default (`auto`), which is the
    // only way back to it, an AGENT PRESET's task is optional — its box,
    // or a QUICK PROMPT it is on, launches on prefix + postfix alone — and
    // a QUICK PROMPT sent empty starts the CLI with no first prompt, the
    // way `n` does (its CLAUDE CLOUD box, which needs the task, aside).
    // For every other prompt an empty field is a cancel.
    let empty_is_a_default = match &prompt.kind {
        PromptKind::NewWorktree { .. }
        | PromptKind::RenameProject { .. }
        | PromptKind::SettingText { .. }
        | PromptKind::AgentPresetTask { .. } => true,
        PromptKind::QuickPrompt(launch) => launch.launches_empty(),
        _ => false,
    };
    if value.is_empty() && !empty_is_a_default {
        // An empty comment box goes back to the modal it stood in for, and
        // an empty QUICK PROMPT to the one it stood on.
        match &prompt.kind {
            PromptKind::IssueComment { view, .. } => crate::issues::reopen(app, view.clone()),
            PromptKind::PrComment {
                back: Some(view), ..
            } => crate::pr_modal::reopen(app, (**view).clone()),
            PromptKind::QuickPrompt(launch) => {
                if let Some(under) = launch.under.clone() {
                    under.reopen(app);
                }
            }
            _ => {}
        }
        app.flash = Some("cancelled: empty input".into());
        return None;
    }
    Some((prompt, value))
}

fn dispatch_prompt(
    app: &mut App,
    prompt: PromptDialog,
    value: String,
    out: &mut Vec<ClientRequest>,
) {
    match prompt.kind {
        PromptKind::AddProject => open_folder(app, shellexpand_home(&value), out),
        PromptKind::SetProjectPath { id, old_path } => {
            let typed = shellexpand_home(&value);
            let path = std::fs::canonicalize(&typed).unwrap_or(typed);
            let intent = PendingIntent::ProjectPathSet {
                project: id.clone(),
                old_path: old_path.clone(),
                new_path: path.clone(),
            };
            send_with(app, out, intent, |req_id| ClientRequest::SetProjectPath {
                req_id,
                id,
                path,
            });
        }
        PromptKind::NewWorktree {
            project,
            suggestion,
        } => {
            // "fix login redirect" is how a branch gets described out
            // loud; git wants it hyphenated. Nothing typed at all takes
            // the random name the prompt was offering.
            let branch = crate::branch_name::slugify(&value);
            let branch = if branch.is_empty() {
                suggestion.clone()
            } else {
                branch
            };
            // The row first, selected as the Ack would leave it, so the
            // panel never waits on the DAEMON's fetch and `git worktree
            // add`. An Error takes it down and hands this box back.
            let focus = app.focus;
            app.bring_tab_forward(&project);
            let placeholder =
                placeholder::stage_worktree(app, project.clone(), branch.clone(), out);
            send_with(
                app,
                out,
                PendingIntent::SelectCreatedWorktree {
                    placeholder,
                    focus,
                    prompt: PromptKind::NewWorktree {
                        project: project.clone(),
                        suggestion,
                    },
                    text: value,
                    launch: None,
                },
                |req_id| ClientRequest::CreateWorktree {
                    req_id,
                    project,
                    branch,
                    base: None,
                },
            );
        }
        PromptKind::ClaudeCloudTask {
            worktree,
            name,
            model,
            effort,
        } => create_agent(
            app,
            AgentLaunchDraft {
                name,
                cloud_prompt: Some(value),
                ..AgentLaunchDraft::new(worktree, AgentKind::Claude, model, effort)
            },
            out,
        ),
        PromptKind::AgentPresetTask { worktree, preset } => {
            // The launch a QUICK PROMPT with this preset on it sends, built
            // by the same two functions: `QuickLaunch::of_preset` resolves
            // the harness (a preset pins a model / effort or follows
            // Settings → Agents) and `quick_launch::draft` composes prefix
            // + task + postfix — sized above, so composing cannot fail, and
            // an empty task on a bare preset composes to no STARTING PROMPT
            // at all. Only what is this box's own is said here: it was
            // walked to through the presets list, so it takes the pane, and
            // a refusal brings this box back, not the quick prompt's.
            let launch = crate::quick_prompt::QuickLaunch::of_preset(
                crate::quick_prompt::QuickTarget::Worktree(worktree.clone()),
                preset.clone(),
                &crate::config::Config::load(),
            );
            let draft = AgentLaunchDraft {
                reopen_on_error: Some((
                    PromptKind::AgentPresetTask {
                        worktree: worktree.clone(),
                        preset,
                    },
                    value.clone(),
                )),
                ..quick_launch::draft(launch, worktree, value, true, None)
            };
            create_agent(app, draft, out);
        }
        // A box opened over the ISSUES MODAL or the PULL REQUESTS MODAL
        // takes the modal with it when it launches: the grid is where the
        // new session's card goes up — and where the Ack puts the cursor on
        // it (`attach_created`) with FOLLOW NEW SESSION on, or else leaves
        // it on the card it was on. Only a box that goes without launching
        // hands the modal back.
        PromptKind::QuickPrompt(launch) => quick_launch::submit(app, launch, value, out),
        PromptKind::PrComment {
            number,
            url,
            label,
            back,
        } => post_pr_comment(app, number, url, label, value, back),
        PromptKind::CloudMessage { id } => {
            let intent = PendingIntent::ReopenPromptOnError {
                kind: PromptKind::CloudMessage { id: id.clone() },
                text: value.clone(),
                note: "Sent to the cloud session — the reply lands on its page".into(),
            };
            send_with(app, out, intent, |req_id| ClientRequest::SendCloudMessage {
                req_id,
                id,
                message: value,
            });
        }
        // The turn goes down the PTY where the session stands: the pane
        // is not swapped, not unfolded and not focused, so one card after
        // another can be prompted without ever stepping into a session.
        PromptKind::FollowUp { id } => {
            if matches!(send_turn(app, &id, &value, out), TurnSent::Booting) {
                open_follow_up(app, id, value);
            }
        }
        PromptKind::RenameAgent { id } => optimistic::rename_agent(app, id, value, out),
        PromptKind::RenameTerminal { id } => optimistic::rename_terminal(app, id, value, out),
        PromptKind::RenameProject { id } => optimistic::rename_project(app, id, value, out),
        PromptKind::SettingText { kind, project } => {
            // Same path as a toggled row (`apply_setting_at`): write the
            // file, adopt it live, and land back on the overlay — with the
            // row's new value in the notice line so the save is visible
            // even when the label column is what changed. A PROJECT TAB
            // row's value goes into the entry of the project the prompt
            // was opened on, and the notice reads that entry back.
            let mut cfg = crate::config::Config::load();
            match &project {
                Some(path) => cfg.set_project_text(path, kind, &value),
                None => cfg.set_text(kind, &value),
            };
            let saved = save_config(app, &cfg);
            if saved {
                apply_config(app, &cfg);
            }
            reopen_settings(app);
            if saved {
                let label = crate::config::spec_for(kind)
                    .map(|s| s.label)
                    .unwrap_or("setting");
                let shown = match &project {
                    Some(path) => cfg.project(path).value_label(kind),
                    None => cfg.value_label(kind),
                };
                if let Some(view) = settings_mut(app) {
                    view.info(format!("{label}: {shown}"));
                }
            }
        }

        PromptKind::IssueComment { view, issue } => {
            crate::issues::post_comment(app, view, issue, value);
        }
        PromptKind::EditLink { id } => {
            send(app, out, |req_id| ClientRequest::UpdateLink {
                req_id,
                id,
                url: value,
            });
        }
    }
}

pub(crate) fn run_pending_action(
    app: &mut App,
    action: PendingAction,
    out: &mut Vec<ClientRequest>,
) {
    match action {
        PendingAction::LocateProjectPath { id, old_path } => {
            app.dismissed_repath_projects.remove(&id);
            open_prompt(app, PromptKind::SetProjectPath { id, old_path });
        }
        PendingAction::CreateProjectDir(path) | PendingAction::InitProjectRepo(path) => {
            send_with(app, out, PendingIntent::SelectCreatedProject, |req_id| {
                ClientRequest::AddProject {
                    req_id,
                    path,
                    name: None,
                    create_missing: true,
                }
            });
        }
        PendingAction::ArchiveAgent(id) => archive_agent_now(app, id, out),
        PendingAction::ArchiveThread { agents, terminals } => {
            for id in agents {
                archive_agent_now(app, id, out);
            }
            for id in terminals {
                close_terminal(app, id, out);
            }
        }
        PendingAction::DeleteAgent(id) => delete_agent(app, id, out),
        PendingAction::MoveAgent { id, worktree } => launcher::move_agent(app, id, worktree, out),
        PendingAction::CloseTerminal(id) => close_terminal(app, id, out),
        PendingAction::DeleteLink(id) => {
            send(app, out, |req_id| ClientRequest::DeleteLink { req_id, id });
        }
        PendingAction::DeleteWorktree(id) => delete_worktree_and_settle(app, id, out),
        PendingAction::ThenDeleteWorktree {
            first, worktree, ..
        } => {
            // The row's own delete drops its card; the worktree delete
            // then takes the emptied band and settles the cursor once.
            run_pending_action(app, *first, out);
            delete_worktree_and_settle(app, worktree, out);
        }
        PendingAction::DeleteAllWorktrees(ids) => {
            // Each delete is its own request with its own optimistic
            // removal + rollback, so one failure restores only its rows.
            // One reconcile at the end: the cursor settles on a survivor.
            let before = selection_snapshot(app);
            for id in ids {
                delete_worktree(app, id, out);
            }
            reconcile_selection(app, before, out);
        }
        PendingAction::DeleteAllSessions { agents, terminals } => {
            for id in agents {
                delete_agent(app, id, out);
            }
            for id in terminals {
                close_terminal(app, id, out);
            }
        }
        PendingAction::RemoveProject(id) => {
            send(app, out, |req_id| ClientRequest::RemoveProject {
                req_id,
                id,
            });
        }
        PendingAction::CloseProjectTab(id) => launcher::close_cursor_tab(app, &id, out),
        PendingAction::DeleteAgentPreset {
            index,
            worktree,
            quick,
        } => {
            let mut presets = crate::agent_presets::load();
            if index < presets.len() {
                let removed = presets.remove(index);
                match crate::agent_presets::save(&presets) {
                    Ok(()) => app.flash = Some(format!("deleted preset '{}'", removed.name)),
                    Err(err) => app.flash = Some(format!("could not save agent presets: {err}")),
                }
            }
            crate::preset_overlays::reopen_presets_list(
                app,
                worktree,
                quick.map(|back| *back),
                index,
            );
        }
        PendingAction::ResetSettings => reset_settings(app),
        PendingAction::Quit => app.should_quit = true,
    }
}

/// Delete an agent for good, detaching the pane first if it's showing it.
pub(crate) fn delete_agent(app: &mut App, id: AgentId, out: &mut Vec<ClientRequest>) {
    detach_if_attached(app, &SessionRef::Agent(id.clone()), out);
    optimistic::delete_agent(app, id, out);
}

/// Close a terminal tab, detaching the pane first if it's showing it.
pub(crate) fn close_terminal(app: &mut App, id: TerminalId, out: &mut Vec<ClientRequest>) {
    // The grid's cursor may be on this terminal's chip: `keep_cursor`
    // hands it to the neighbour once the row is gone, and the pane
    // follows it there.
    detach_if_attached(app, &SessionRef::Terminal(id.clone()), out);
    optimistic::close_terminal(app, id, out);
}

/// A confirmed worktree delete, and the cursor after it. Optimistic: the
/// rows drop now (the daemon deletes in the background — `git worktree
/// remove` can take seconds); the eventual EntityRemoved is a no-op and an
/// Error for this req_id restores the rows via the rollback stashed in the
/// intent. Deleting the selected worktree lands the cursor on a neighbor —
/// its session comes up like a manual switch would.
pub(crate) fn delete_worktree_and_settle(
    app: &mut App,
    id: WorktreeId,
    out: &mut Vec<ClientRequest>,
) {
    let before = selection_snapshot(app);
    delete_worktree(app, id, out);
    reconcile_selection(app, before, out);
}

/// Live cards a worktree still holds: its unarchived agents and its
/// terminals — what its band on the grid shows.
pub(crate) fn live_cards_in(app: &App, id: &WorktreeId) -> usize {
    app.tree
        .agents
        .iter()
        .filter(|a| &a.worktree_id == id && !a.archived)
        .count()
        + app
            .tree
            .terminals
            .iter()
            .filter(|t| &t.worktree_id == id)
            .count()
}

/// A row delete's confirm, with the checkout's fate folded in when the
/// delete would empty a linked worktree. `dialog` is the row's ordinary
/// confirm; `live_taken` how many live cards it removes from `worktree`
/// (one for a card, zero for an archived row, the doomed count for a
/// `D`). When that is every live card the worktree has — and it is not
/// the ROOT WORKTREE or a stand-in git is still cutting — the dialog asks
/// in the same breath whether the worktree goes too: `Enter`/`y` deletes
/// the card and then the checkout, `n` deletes the card alone, `Esc`
/// keeps the card. Archived sessions still filed under the worktree are
/// counted in the question, since the delete takes their history. With
/// the **Delete emptied worktree** SETTING on and nothing archived left
/// the question is skipped: the dialog stays two-way, says the worktree
/// goes with the card, and `Enter` does both. With **Show all worktrees**
/// on the question is never asked — the emptied checkout keeps its band
/// on the grid, and `d` on that band is the way to delete it — but
/// **Delete emptied worktree** still deletes: that setting is the user
/// saying an emptied worktree goes, whether or not the grid would have
/// kept a band for it. Anything else leaves the dialog as it came.
pub(crate) fn with_worktree_offer(
    app: &App,
    mut dialog: ConfirmDialog,
    worktree: &WorktreeId,
    live_taken: usize,
) -> ConfirmDialog {
    let Some(w) = app.tree.worktrees.iter().find(|w| &w.id == worktree) else {
        return dialog;
    };
    if w.is_main
        || app.is_placeholder_worktree(worktree)
        || live_cards_in(app, worktree) != live_taken
    {
        return dialog;
    }
    let force = crate::config::Config::load().delete_empty_worktree;
    if app.show_all_worktrees && !force {
        return dialog;
    }
    let archived = app
        .tree
        .agents
        .iter()
        .filter(|a| &a.worktree_id == worktree && a.archived)
        .count();
    let offered = !(force && archived == 0);
    let branch = &w.branch;
    dialog.message.push('\n');
    dialog.message.push_str(&match (offered, archived) {
        (false, _) => format!("Worktree '{branch}' goes with it: nothing else is left there."),
        (true, 0) => format!("Nothing else is left in worktree '{branch}': delete it from disk too?"),
        (true, n) => format!(
            "Nothing live is left in worktree '{branch}': delete it from disk too?\nIts {n} archived session(s) would go with it."
        ),
    });
    dialog.action = PendingAction::ThenDeleteWorktree {
        first: Box::new(dialog.action),
        worktree: worktree.clone(),
        offered,
    };
    dialog
}

/// The confirm before an agent is deleted, with the worktree question
/// folded in when the agent is the last card of a linked worktree
/// ([`with_worktree_offer`]). From the `d` key and the row menu alike. On
/// a NESTED thread's root it is the whole thread's
/// ([`confirm_delete_thread`]).
pub(crate) fn confirm_delete_agent_in(app: &App, a: &nebula_core::Agent) -> ConfirmDialog {
    if let Some(band) = crate::launcher::nested_thread(app, &SessionRef::Agent(a.id.clone())) {
        return confirm_delete_thread(app, &band);
    }
    let dialog = confirm_delete_agent(&a.name, a.id.clone());
    with_worktree_offer(app, dialog, &a.worktree_id, usize::from(!a.archived))
}

/// The confirm before a terminal is closed, with the worktree question
/// folded in when the terminal is the last card of a linked worktree —
/// or, on the root of a NESTED thread of terminals, the whole thread's.
pub(crate) fn confirm_close_terminal_in(app: &App, t: &nebula_core::TerminalTab) -> ConfirmDialog {
    if let Some(band) = crate::launcher::nested_thread(app, &SessionRef::Terminal(t.id.clone())) {
        return confirm_delete_thread(app, &band);
    }
    let dialog = confirm_close_terminal(&t.name, t.id.clone());
    with_worktree_offer(app, dialog, &t.worktree_id, 1)
}

/// Delete a worktree optimistically: drop its rows now (the daemon deletes
/// in the background — `git worktree remove` can take seconds). The
/// eventual EntityRemoved is a no-op; an Error for this req_id restores the
/// rows via the rollback stashed in the intent.
pub(crate) fn delete_worktree(app: &mut App, id: WorktreeId, out: &mut Vec<ClientRequest>) {
    let intent = match remove_worktree_rows(app, &id) {
        Some(rollback) => PendingIntent::DeleteWorktree(rollback),
        None => PendingIntent::None,
    };
    send_with(app, out, intent, |req_id| ClientRequest::DeleteWorktree {
        req_id,
        id,
        force: true,
    });
}

pub(crate) fn run_menu_action(app: &mut App, action: MenuAction, out: &mut Vec<ClientRequest>) {
    match action {
        MenuAction::Attach(sref) => activate::attach(app, sref, out),
        MenuAction::RestartAgent(id) => {
            send(app, out, |req_id| ClientRequest::RestartAgent {
                req_id,
                id,
            });
        }
        MenuAction::SendCloudMessage(id) => open_prompt(app, PromptKind::CloudMessage { id }),
        MenuAction::DuplicateAgent(id) => launcher::duplicate_agent(app, id),
        MenuAction::MoveAgentPicker(id) => launcher::open_move_picker(app, id),
        MenuAction::MoveAgent { id, worktree } => launcher::move_agent(app, id, worktree, out),
        MenuAction::RenameAgent(id) => open_prompt(app, PromptKind::RenameAgent { id }),
        MenuAction::ArchiveAgent(id) => {
            archive_agent(app, id, out);
        }
        MenuAction::UnarchiveAgent(id) => activate::unarchive(app, id, out),
        MenuAction::DeleteAgent(id) => {
            if let Some(a) = app.tree.agents.iter().find(|a| a.id == id).cloned() {
                app.overlay = Some(Overlay::Confirm(confirm_delete_agent_in(app, &a)));
            }
        }
        MenuAction::NewAgent(worktree) => open_new_agent_picker(app, worktree),
        MenuAction::NewTerminal(worktree) => create_terminal(app, worktree, out),
        MenuAction::RenameTerminal(id) => open_prompt(app, PromptKind::RenameTerminal { id }),
        MenuAction::CloseTerminal(id) => {
            if let Some(t) = app.tree.terminals.iter().find(|t| t.id == id).cloned() {
                app.overlay = Some(Overlay::Confirm(confirm_close_terminal_in(app, &t)));
            }
        }
        MenuAction::NewAgentOfKind {
            worktree,
            kind,
            custom,
            model,
            effort,
            cloud,
            pr,
            quick,
        } => {
            // Opened from the QUICK PROMPT: the pick rewrites that box's
            // launch and hands it straight back — no session is created
            // here, nothing is warmed (a STARTING PROMPT launch can never
            // adopt a WARM SPARE), and the SETTING is untouched. The
            // resolve/fit below is `QuickLaunch::of_kind`'s job instead.
            if let Some(back) = quick {
                // The picker's `worktree` is only what its menu was built
                // against; where the launch lands is the box's own target.
                let launch = crate::quick_prompt::QuickLaunch::of_kind(
                    back.launch.target.clone(),
                    kind,
                    custom.clone(),
                    model.filter(|m| m != crate::config::DEFAULT_CHOICE),
                    effort.filter(|e| e != crate::config::DEFAULT_CHOICE),
                    &crate::config::Config::load(),
                )
                .with_issue(back.launch.issue.clone())
                .with_pr(back.launch.pr.clone())
                // The Claude row's `Tab` toggle rides the pick: the box
                // comes back a CLAUDE CLOUD one, where it can be one.
                .with_cloud(cloud)
                .with_under(back.launch.under.clone());
                if back.from_box {
                    crate::quick_prompt::reopen(app, launch, &back.text);
                } else {
                    // No box was up (`n`'s NEW SESSION PICKER): the pick
                    // OPENS one, on the spec just chosen.
                    crate::quick_prompt::open_picked_box(app, launch);
                }
                return;
            }
            // Resolve the picker's choice against the configured defaults:
            // an unexpanded submenu (None) and the explicit "default" row
            // both take the setting; the setting's own "default" means
            // "no flag" and reaches the daemon as None.
            let mut cfg = crate::config::Config::load();
            // What the submenus chose, before the defaults fill in the
            // rest: REMEMBER HARNESS below records a pick, never a fallback.
            let picked = (model.clone(), effort.clone());
            let resolve = |choice: Option<String>, configured: Option<String>| match choice {
                None => configured,
                Some(c) if c == crate::config::DEFAULT_CHOICE => configured,
                some => some,
            };
            let harness = cfg.effective_harness(kind, custom.as_deref());
            let model = resolve(model, harness.default_model().map(str::to_string));
            // A Cursor effort only counts for the family it belongs to: a
            // stale setting behind a freshly picked model drops to none.
            let effort = crate::config::fit_effort(
                kind,
                model.as_deref(),
                resolve(effort, harness.default_effort().map(str::to_string)),
                custom.as_deref(),
            );
            // A Claude Cloud launch has its own task box, and nothing to
            // name first.
            if cloud {
                open_prompt(
                    app,
                    PromptKind::ClaudeCloudTask {
                        worktree,
                        name: String::new(),
                        model,
                        effort,
                    },
                );
                return;
            }
            // The pick is the whole flow: the session starts right here,
            // under the generated name and AUTO-TITLE, its first prompt
            // typed in the CLI. No box stands between the picker and the
            // pane — a launch that starts from a typed task is the QUICK
            // PROMPT's (`p`; `e` on an OPEN PRS row for a PR SESSION). The
            // standing default-spec warm slot gets adopted where it
            // matches (never for a PR SESSION: an unscoped warm CLI cannot
            // be adopted for one), and the refill behind the create
            // re-warms it either way.
            //
            // REMEMBER HARNESS (Settings → Experimental): the pick is the
            // next launch's default — this harness, and a model or effort
            // only where a submenu chose one. A failed write flashes; the
            // launch goes ahead regardless.
            if cfg.remember_launch(
                kind,
                custom.as_deref(),
                picked.0.as_deref(),
                picked.1.as_deref(),
            ) {
                save_config(app, &cfg);
            }
            create_agent(
                app,
                AgentLaunchDraft {
                    custom: custom.clone(),
                    pr,
                    ..AgentLaunchDraft::new(worktree, kind, model, effort)
                },
                out,
            );
        }
        MenuAction::NewWorktree(project) => open_new_worktree_prompt(app, project),
        MenuAction::OpenLink(url) => open_link(app, &url, out),
        MenuAction::ViewPrDiff => request_pr_diff(app),
        MenuAction::CommentPullRequest => open_pr_comment(app),
        // In the LAUNCHER VIEW the composer is a STRIP along the bottom
        // of the PANE, which has to be there and reading the right
        // session first: same intent, one step more, so this row and
        // Space on the card end in the same place.
        MenuAction::FollowUp if app.launcher_active() => launcher::follow_up(app),
        MenuAction::FollowUp => activate::follow_up(app),
        MenuAction::EditLink(id) => open_prompt(app, PromptKind::EditLink { id }),
        MenuAction::DeleteLink(id) => {
            if let Some(row) = app
                .visible_links()
                .into_iter()
                .find(|l| l.id() == Some(&id))
            {
                delete_link(app, &row);
            }
        }
        MenuAction::ToggleRun(id) => {
            if let Some(w) = app.tree.worktrees.iter().find(|w| w.id == id).cloned() {
                toggle_run_in(app, &w, out);
            }
        }
        MenuAction::OpenWorktree(id) => {
            if let Some(w) = app.tree.worktrees.iter().find(|w| w.id == id).cloned() {
                open_worktree(app, &w);
            }
        }
        MenuAction::DeleteWorktree(id) => activate::delete_worktree(app, &id),
        MenuAction::SwitchBranch(id) => crate::branch_switch::open_for(app, &id),
        MenuAction::AddProject => open_prompt(app, PromptKind::AddProject),
        MenuAction::RenameProject(id) => open_prompt(app, PromptKind::RenameProject { id }),
        MenuAction::OpenProject(id) => launcher::open_project(app, &id, out),
        MenuAction::RemoveProject(id) => {
            if let Some(p) = app.tree.projects.iter().find(|p| p.id == id).cloned() {
                app.overlay = Some(Overlay::Confirm(confirm_remove_project(&p.name, id)));
            }
        }
        MenuAction::ToggleArchived => toggle_archived(app, out),
        MenuAction::ToggleOpenPrs => toggle_open_prs(app, out),
        MenuAction::ToggleIssues => toggle_issues(app, out),
        MenuAction::ToggleDraftPrs => toggle_hide_draft_prs(app, out),
        MenuAction::PickLaunchWorktree { target, back } => {
            launcher::pick_launch_worktree(app, target, *back)
        }
    }
}
