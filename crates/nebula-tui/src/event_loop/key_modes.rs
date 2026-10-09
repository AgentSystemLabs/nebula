//! Top-level keyboard dispatch split away from daemon event handling.

use super::*;

pub(super) fn handle_key(app: &mut App, key: KeyEvent, out: &mut Vec<ClientRequest>) {
    // The editor modal sits above every overlay: all keys forward to it —
    // vim needs Esc — except Ctrl+Q, the same hatch the terminal lock uses.
    if app.vim.is_some() {
        handle_vim_key(app, key);
        return;
    }

    // Modal overlays swallow all keys — except the HARDWIRED UNLOCK, which
    // closes whatever is open outright, from any state and any nesting
    // depth. Esc is the ordinary way out and several overlays stage it (a
    // typed filter or an open submenu peels first), and a click outside
    // needs a mouse; Ctrl+Q is the one press that always lands back on the
    // panels, the same promise it makes inside a LOCKED PANE.
    if app.overlay.is_some() {
        let chord = crate::keymap::KeyChord::from_event(&key);
        if chord == HARDWIRED_UNLOCK {
            crate::key_combo::note(app, &[chord], Some("Force close"));
            crate::overlay_close::force_close(app);
            return;
        }
        // The KEY COMBO DISPLAY shows a modal's navigation keys bare and
        // never what is typed into its text field (see key_combo.rs).
        if !crate::key_combo::is_text_key(&chord) {
            crate::key_combo::note(app, &[chord], None);
        }
        handle_overlay_key(app, key, out);
        return;
    }

    if handle_locked_pane_key(app, key, out) {
        return;
    }
    // A session card expanded into its FOLLOW-UP COMPOSER: the box owns
    // every key the SESSIONS PANEL would otherwise act on — they are all
    // letters, and `a` in a prompt must not archive the session being
    // prompted. Only the panel walk gets through (Tab / ⇧Tab), and Esc
    // folds the card back up.
    if app.focus == Focus::Sessions && app.follow_up_live() && follow_up_key(app, key, out) {
        return;
    }

    if handle_preview_scroll_key(app, key) {
        return;
    }

    handle_global_key(app, key, out);
}

fn handle_locked_pane_key(app: &mut App, key: KeyEvent, out: &mut Vec<ClientRequest>) -> bool {
    // Terminal input-locked with a live session: forward everything except
    // the escape hatches. Enter locks; an unlocked pane falls through to
    // the grid's keys, so the user always has a way back that isn't a
    // hatch.
    if app.focus == Focus::Terminal && app.term.is_some() && app.term_locked {
        // Ctrl+q is the primary hatch: a plain control byte (0x11) that
        // every emulator delivers — Terminal.app included, no kitty protocol
        // needed — unbound in macOS and unused by Claude Code. The inner
        // session loses XON (unfreeze after an accidental Ctrl+S), which
        // nobody will miss.
        // Fallback hatches: Ctrl+] (telnet's escape char — byte 0x1D,
        // which crossterm spells Ctrl+5 in legacy mode) and Ctrl+Shift+H
        // (needs the kitty protocol, so Ghostty/kitty only, never
        // Terminal.app). Ctrl+← went: it took word-left from the agent.
        // All three are rebindable in Settings → Hotkeys, but Ctrl+q stays
        // wired in on top of whatever is bound: unbinding your way out of
        // a locked session would trap you in it with no way back.
        let chord = crate::keymap::KeyChord::from_event(&key);
        let is_hatch = chord == HARDWIRED_UNLOCK
            || app.keymap.lookup(crate::keymap::Scope::Terminal, &chord)
                == Some(crate::keymap::Action::UnlockTerminal);
        // `^F` full-screens the pane under the cards and brings it back
        // down; in a full-screen session the hatches and the pane fold's
        // `^`` bring it back down too, rather than straight out to the
        // grid — the keys stay in the session, now in its pane.
        let zooms = toggles_full_screen(app, &chord)
            || (app.collapsed && (is_hatch || folds_launcher_pane(app, &chord)));
        if app.launcher_active() && zooms {
            let did = launcher::toggle_full_screen(app, out);
            crate::key_combo::note(app, &[chord], Some(did));
            return true;
        }
        if is_hatch {
            // The one key in a LOCKED PANE the KEY COMBO DISPLAY shows:
            // everything else typed here is the agent's, passwords
            // included, and never lands on the screen.
            crate::key_combo::note(
                app,
                &[chord],
                crate::keymap::spec_of(crate::keymap::Action::UnlockTerminal).map(|s| s.label),
            );
            leave_terminal_lock(app);
            return true;
        }
        // `^`` / `^~` are the way out of the LAUNCHER PANE: the first
        // press hands the keys back to the card the pane reads, and the
        // same chord from the cards then folds the pane away
        // (`launcher::fold_key`). Only the pane under the cards (a
        // full-screen session has none to fold) and only a chord no one
        // types as text: the bare `~` bound beside them is the agent's
        // here.
        if app.launcher_grid() && folds_launcher_pane(app, &chord) {
            let did = launcher::fold_key(app);
            crate::key_combo::note(app, &[chord], Some(did));
            return true;
        }
        // `⌘P` drops the PROJECT DROPDOWN from inside the pane under the
        // cards, as a click on the header's `+` does: no one types a ⌘
        // chord as text, so the agent loses nothing. Esc hands the keys
        // straight back to the pane; a project picked lands on its cards.
        // A full-screen session has no header to hang the list from, and
        // gets the chord.
        if app.launcher_grid() && drops_project_dropdown(app, &chord) {
            crate::key_combo::note(
                app,
                &[chord],
                crate::keymap::spec_of(crate::keymap::Action::ProjectDropdown).map(|s| s.label),
            );
            launcher::open_project_menu(app);
            return true;
        }
        let exited = app.term.as_ref().is_some_and(|t| t.exited);
        // A stand-in pane (QUICK PROMPT, checkout still being cut) has no
        // PTY behind it: the keystroke has nowhere to go until the real
        // session attaches, and must not land in the previous one.
        let stand_in = app.pane_shows_placeholder();
        if !exited {
            if let Some(term) = &mut app.term {
                // Typing changes the content under a persisted selection
                // highlight — drop it. Not one still being dragged: the
                // button is down, and only its release ends that.
                if !app.term_selection.is_some_and(|s| s.dragging) {
                    app.term_selection = None;
                }
                // Typing exits scroll mode (tmux behavior).
                if term.scroll_offset() > 0 {
                    term.set_scroll(0);
                }
                if stand_in {
                    return true;
                }
                if let Some(data) = keys::encode_key(&key, term.kitty_flags) {
                    let session = term.sref.clone();
                    typed_into(app, &session);
                    out.push(ClientRequest::Input { session, data });
                }
            }
            return true;
        }
        // Exited session: there is nothing to type into, so don't swallow
        // keys. Esc/Enter/q go back to the session list; everything else
        // falls through to panel navigation.
        if matches!(key.code, KeyCode::Esc | KeyCode::Enter | KeyCode::Char('q')) {
            crate::key_combo::note(app, &[chord], Some("Back to sessions"));
            leave_terminal_lock(app);
            return true;
        }
    }
    false
}

fn handle_preview_scroll_key(app: &mut App, key: KeyEvent) -> bool {
    // Reading a pull request or an issue in the pane: the diff modal's
    // scroll keys work here too. Page/Home/End only — shift+↑/↓ already
    // move a project, and ↑/↓ have to keep walking the list itself. From
    // either list that can rest on one; a focused pane keeps its keys for
    // the PTY.
    if app.reading_url().is_some() && matches!(app.focus, Focus::Worktrees | Focus::Sessions) {
        let page = app.term_area.height.max(1);
        let max = app.pr_preview_max_scroll();
        let scrolled = match key.code {
            KeyCode::PageDown => Some(app.pr_preview_scroll.saturating_add(page).min(max)),
            KeyCode::PageUp => Some(app.pr_preview_scroll.saturating_sub(page)),
            KeyCode::Home => Some(0),
            KeyCode::End => Some(max),
            _ => None,
        };
        if let Some(to) = scrolled {
            let does = if app.previewed_pr().is_some() {
                "Scroll the pull request"
            } else {
                "Scroll the issue"
            };
            crate::key_combo::note(
                app,
                &[crate::keymap::KeyChord::from_event(&key)],
                Some(does),
            );
            app.dirty |= app.pr_preview_scroll != to;
            app.pr_preview_scroll = to;
            return true;
        }
    }
    false
}

fn handle_global_key(app: &mut App, key: KeyEvent, out: &mut Vec<ClientRequest>) {
    // Panel focus: every key here is a rebindable action (see keymap.rs),
    // so the dispatch is a table lookup rather than a KeyCode match — an
    // unbound press simply falls through.
    let chord = crate::keymap::KeyChord::from_event(&key);
    // A double tap is two of the same key in a row: whatever else arrives
    // in between — bound or not — breaks it, so the arm is taken here and
    // only the edge arms below put one back.
    let armed = app.edge_tap.take();
    let action = app.keymap.lookup(crate::keymap::Scope::Global, &chord);
    // The KEY COMBO DISPLAY: the key and the label of what it fired — an
    // unbound key shows bare, so a watcher sees it did nothing. Noted
    // before the dispatch so a double tap's second press can restate the
    // pair as one combo (focus_walk::double_tapped).
    crate::key_combo::note(
        app,
        &[chord],
        action.and_then(crate::keymap::spec_of).map(|s| s.label),
    );
    // Esc in the LAUNCHER VIEW lets the card under the cursor go
    // (`launcher::escape`). Esc is the modal grammar's key and bound to no
    // action, so the lookup above found nothing and the `else` below would
    // drop it.
    if action.is_none() && app.launcher_grid() && key.code == KeyCode::Esc {
        if app.launcher_tab_cursor.is_some() {
            crate::key_combo::note(app, &[chord], Some("Back to the cards"));
        } else if !app.launcher_unaimed {
            crate::key_combo::note(app, &[chord], Some("Unselect the card"));
        }
        launcher::escape(app);
        return;
    }
    let Some(action) = action else {
        return;
    };
    // Every PROJECT TAB closed: the splash is on screen with the projects'
    // rows still selected under it, so only the keys that open a project,
    // or put up nothing but a modal, mean anything. The rest would walk or
    // attach rows nobody can see.
    if app.projects_closed && !opens_from_closed_splash(action) {
        return;
    }
    // The LAUNCHER VIEW's GRID takes the keys that walk it and open a
    // card; the rest keep their panel meaning. Only the grid: over a
    // full-screen session the keys are the PTY's, and the ones that get
    // past it (an exited pane unlocks) are the pane's own.
    if app.launcher_grid() && launcher::handle_action(app, action, armed, &chord, out) {
        return;
    }
    use crate::keymap::Action;
    match action {
        Action::Quit => app.overlay = Some(Overlay::Confirm(confirm_quit())),
        Action::Help => app.overlay = Some(Overlay::Help(HelpView::default())),
        Action::Settings => open_settings(app),
        Action::Metrics => open_metrics(app, out),
        // Tab walks forward and stops dead at the terminal pane —
        // leaning on the key can't spill past the pane and back round to
        // the first column. Landing on the pane takes the input lock:
        // walking that far means the user is going to type at the agent,
        // and the preview under the Sessions cursor is already the session
        // they picked.
        Action::FocusNext => walk_focus_forward(app, out),
        // h/← and l/→ walk one panel at a time, stopping at the ends of
        // the row: h never wraps into the pane. A single press at an end
        // stays put — leaning on the key can't spill over — and a double
        // tap jumps the boundary the way Tab would: l,l at Sessions
        // crosses into the pane and takes its input.
        Action::FocusLeft => walk_focus_back(app),
        Action::Hosts => open_hosts_picker(app),
        Action::AgentPresets => crate::preset_overlays::open_agent_presets(app),
        // Full-screen over a session with a project anywhere: the same box
        // the GRID's `p` opens, in the checkout under the grid's cursor —
        // the session on screen's own worktree — on the root branch with
        // the aim let go, or on a fresh worktree.
        Action::QuickPrompt if app.launcher_active() => launcher::open_box(app),
        Action::QuickPrompt => crate::quick_prompt::open_quick_prompt(app),
        Action::Issues => crate::issues::open_issues(app),
        Action::PullRequests => crate::pr_modal::open(app),
        Action::SwitchBranch => crate::branch_switch::open_branch_switch(app),
        Action::FocusRight => match app.focus {
            Focus::Sessions => {
                if double_tapped(app, action, armed, &chord, "enter pane") {
                    walk_focus_forward(app, out);
                }
            }
            // Standing in the pane unlocked: l,l takes the lock, as Tab
            // does. A dead or empty pane has nothing to lock into.
            Focus::Terminal => {
                let live = app.term.as_ref().is_some_and(|t| !t.exited);
                if live && double_tapped(app, action, armed, &chord, "type into terminal") {
                    walk_focus_forward(app, out);
                }
            }
            _ => walk_focus_forward(app, out),
        },
        // The pane under the LAUNCHER VIEW's cards. The grid takes this
        // key itself (`launcher::handle_action`); it reaches here with a
        // session full-screen over the view — where folding the pane is
        // still what `^q` comes back to — and before the first project,
        // where there are no cards to have a pane under.
        Action::ToggleLauncherPane if app.launcher_active() => launcher::toggle_pane(app),
        Action::ToggleLauncherPane => {
            app.flash = Some("no cards to fold a pane under — add a project first".into())
        }
        // Full-screen, and back down. The grid takes this key itself
        // (`launcher::handle_action`), as does a LOCKED PANE; it reaches
        // here from a full-screen session that is not typing — one that
        // exited — and before the first project, with no session to show.
        Action::ToggleFullScreen if app.launcher_active() => {
            launcher::toggle_full_screen(app, out);
        }
        Action::ToggleFullScreen => {
            app.flash = Some("no session to full-screen — add a project first".into())
        }
        // The strip across the LAUNCHER PANE's header. The grid takes this
        // key itself (`launcher::handle_action`); it reaches here over a
        // full-screen session, which has no strip, and before the first
        // project, where there is no pane at all.
        Action::PaneTabs if app.launcher_active() => {
            app.flash = Some(launcher::NO_PANE_HERE.into())
        }
        Action::PaneTabs => app.flash = Some("no pane here — add a project first".into()),
        // The header's PROJECT TABS. The grid takes these keys itself
        // (`launcher::handle_action`); they reach here with a session
        // full-screen over it, where the header is not on screen.
        // Every tab closed: `+` drops the same PROJECT DROPDOWN the
        // header's `+` does.
        Action::ProjectDropdown if app.projects_closed => launcher::open_project_menu(app),
        Action::NextProjectTab
        | Action::PrevProjectTab
        | Action::CloseProjectTab
        | Action::SelectProjectTab(_)
        | Action::ProjectDropdown => app.flash = Some(launcher::NO_TABS_HERE.into()),
        Action::MoveDown => move_selection(app, 1, out),
        Action::MoveUp => move_selection(app, -1, out),
        // Ctrl+d / Ctrl+u jump the cursor half a panel at a time in the
        // two columns that routinely outgrow their height — Worktrees once
        // the OPEN PRS group is open, Sessions once the ARCHIVED group is
        // — through every row kind alike, stopping at either end; the
        // draw then scrolls the list after the cursor exactly as it does
        // for a single step, so the row landed on is always in view.
        // Those panels only: a locked pane never gets here (the chords
        // are the shell's EOF and kill-to-start), every line editor keeps
        // ^u for itself, and Projects is short enough that the keys stay
        // unclaimed there.
        Action::HalfPageDown | Action::HalfPageUp => {
            let page = match app.focus {
                Focus::Worktrees => app.worktrees_half_page() as i64,
                Focus::Sessions => app.sessions_half_page() as i64,
                _ => 0,
            };
            if page > 0 {
                let delta = match action {
                    Action::HalfPageDown => page,
                    _ => -page,
                };
                move_selection(app, delta, out);
            }
        }
        // The first-run SPLASH, started inside a git repo: Enter opens it —
        // the one-key way from a fresh install to a project.
        Action::Activate if app.splash_showing() => open_launch_repo(app, out),
        Action::Activate => match app.focus {
            Focus::Projects => app.focus = app.next_visible_focus(Focus::Projects),
            // An open-PR row leads out of nebula, so Enter hands it to the
            // browser and stays put; a checkout hands focus one column right.
            Focus::Worktrees => activate::worktrees_row(app, out),
            Focus::Sessions => attach_selected(app, out),
            // Lock input into an already-focused live pane — or, with the
            // CLOUD SESSION PANEL up, open the session it points at.
            Focus::Terminal => {
                if !activate::cloud_link(app, out) {
                    enter_terminal_pane(app, out);
                }
            }
        },
        // Nothing in the tree yet: the SPLASH is on screen and the only
        // thing `n` can mean there is the project it tells you to open.
        // (With a project anywhere the GRID has this key — it is the box
        // — and only gets here full-screen over a session.)
        Action::New if !app.launcher_active() => open_prompt(app, PromptKind::AddProject),
        Action::New => match app.focus {
            Focus::Projects => open_prompt(app, PromptKind::AddProject),
            Focus::Worktrees => {
                if app.selected_worktree_pr().is_some() {
                    open_pr_agent_picker(app);
                } else if let Some(p) = app.selected_project() {
                    let project = p.id.clone();
                    open_new_worktree_prompt(app, project);
                }
            }
            Focus::Sessions => {
                if let Some(w) = app.selected_worktree() {
                    let worktree = w.id.clone();
                    open_new_agent_picker(app, worktree);
                }
            }
            Focus::Terminal => {}
        },
        Action::Rename => match app.focus {
            Focus::Sessions => match app.selected_session_row() {
                Some(SessionRow::Agent(a)) => {
                    open_prompt(app, PromptKind::RenameAgent { id: a.id })
                }
                Some(SessionRow::Terminal(t)) => {
                    open_prompt(app, PromptKind::RenameTerminal { id: t.id })
                }
                Some(SessionRow::Link(l)) => edit_link(app, &l),
                None => {}
            },
            Focus::Projects => {
                if let Some(p) = app.selected_project() {
                    let id = p.id.clone();
                    open_prompt(app, PromptKind::RenameProject { id });
                }
            }
            // Nothing on the Worktrees panel is renamed, so `r` is RUN
            // there: the checkout's `.nebula.json` RUN COMMAND, started or
            // stopped. The KEY COMBO DISPLAY says which, not "Rename".
            Focus::Worktrees => {
                let running = app
                    .selected_worktree()
                    .is_some_and(|w| app.worktree_running(&w.id));
                let does = if running {
                    "Stop the run"
                } else {
                    "Run worktree"
                };
                crate::key_combo::note(app, &[chord], Some(does));
                toggle_run(app, out);
            }
            Focus::Terminal => {}
        },
        // Its own key rather than another meaning for `r`: the selection it
        // acts on is the selected project and worktree, which every panel
        // has, so it is not scoped to a row.
        Action::RefreshPullRequests => refresh_pull_requests(app),
        Action::Archive => {
            if app.focus == Focus::Sessions {
                match app.selected_session_row() {
                    Some(SessionRow::Agent(a)) if !a.archived => {
                        // With the confirm off, one card per press of
                        // `a`, however long it is held.
                        if archive_agent(app, a.id, out) {
                            release_watch::arm(
                                &mut app.release_watch,
                                chord,
                                std::time::Instant::now(),
                            );
                        }
                    }
                    Some(SessionRow::Terminal(_)) => {
                        app.flash = Some("terminals can't be archived — d closes them".into());
                    }
                    Some(SessionRow::Link(_)) => {
                        app.flash = Some("links can't be archived — d deletes them".into());
                    }
                    _ => {}
                }
            }
        }
        Action::Unarchive => {
            if app.focus == Focus::Sessions {
                if let Some(a) = app.selected_session() {
                    if a.archived {
                        activate::unarchive(app, a.id, out);
                        // One card per press of `u`, however long it is held.
                        release_watch::arm(
                            &mut app.release_watch,
                            chord,
                            std::time::Instant::now(),
                        );
                    }
                }
            }
        }
        // The grid takes this itself (`launcher::handle_action`); it
        // reaches here from the menu and with a session full-screen over
        // the view, where there are no cards to swap.
        Action::ToggleArchived => {
            if app.focus == Focus::Sessions {
                toggle_archived(app, out);
            }
        }
        // Fuzzy-search palette over every project / worktree / session.
        // The config read is per-open so edits apply without restarting.
        Action::Palette => {
            if app.focus != Focus::Terminal {
                app.overlay = Some(Overlay::Palette(Palette::new(
                    &app.tree,
                    crate::config::Config::load().palette_enter_attaches,
                    &app.open_prs,
                    app.hide_draft_prs,
                )));
            }
        }
        // `.` / `,`: the palette's attention order as a ring, no modal —
        // one press lands on the next session that needs you, in whatever
        // project it lives, the way `/` Enter would (same setting, same
        // landing, same always-attach on a red row). Live from any panel.
        Action::NextAttention => {
            let attaches = crate::config::Config::load().palette_enter_attaches;
            jump_attention(app, 1, attaches, out);
        }
        Action::PrevAttention => {
            let attaches = crate::config::Config::load().palette_enter_attaches;
            jump_attention(app, -1, attaches, out);
        }
        Action::Delete => open_delete_confirm(app),
        // Delete EVERY row of the focused panel (behind a confirm that
        // lists the casualties).
        Action::DeleteAll => open_delete_all_confirm(app),
        // Space on a session card: expand it into its FOLLOW-UP COMPOSER,
        // or fold it back up. Sessions only — the other panels have no
        // card to expand, and Space stays unbound there.
        Action::FollowUp => {
            if app.focus == Focus::Sessions {
                activate::follow_up(app);
            }
        }
        // On an open-PR row (either list) `g` reads that pull request's
        // diff off GitHub instead of the checkout's — same modal, different
        // source.
        Action::GitDiff if app.previewed_pr().is_some() => request_pr_diff(app),
        Action::GitDiff => open_diff_view(app),
        Action::CommentPullRequest => open_pr_comment(app),
        Action::OpenRepo => open_repo_in_browser(app),
        // The grid takes this itself (`launcher::handle_action`); it
        // reaches here over a full-screen session the keys got past, which
        // is still the selected session's card.
        Action::OpenPullRequest => launcher::open_pull_request(app, out),
        Action::OpenIssue => launcher::open_issue(app, out),
        Action::DuplicateSession => launcher::duplicate_session(app),
        Action::MoveSession => launcher::move_session(app),
        Action::OpenGhosttyTab => open_ghostty_tab(app),
        // Shift+Enter / Shift+O: the selected worktree's OPEN COMMAND, from
        // any panel — the cursor's worktree is the context wherever the
        // cursor stands, as it is for `g`, `f` and `b`, so the key is never
        // quietly dropped on Sessions.
        Action::OpenWorktree => open_selected_worktree(app),
        // AddProject adds a project from ANY panel — unlike New it never
        // changes meaning with focus, matching the "open a repo" instinct.
        Action::AddProject => open_prompt(app, PromptKind::AddProject),
        Action::FindFile => open_file_finder(app),
        Action::Grep => open_grep_view(app),
        Action::TreeBrowser => open_tree_browser(app),
        // New shell terminal, spawned in the worktree's directory.
        // (Cmd+T never reaches a TUI — the emulator opens its own tab.)
        Action::NewTerminal => create_terminal_for_context(app, out),
        // Terminal-scope only; never resolved here.
        Action::UnlockTerminal => {}
    }
}
