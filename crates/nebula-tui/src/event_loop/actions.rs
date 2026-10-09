//! Extracted event-loop helper section.

use super::*;

pub(crate) fn handle_vim_key(app: &mut App, key: KeyEvent) {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    if ctrl && key.code == KeyCode::Char('q') {
        if let Some(vim) = &mut app.pane.vim {
            vim.kill();
        }
        close_vim(app);
        return;
    }
    if let Some(vim) = &mut app.pane.vim {
        if let Some(data) = keys::encode_key(&key, 0) {
            vim.input(&data);
        }
    }
}

/// Archive asks first — the CONFIRM DIALOG `d` goes behind — so a letter
/// aimed at an agent that lands on the grid archives nothing until it is
/// answered; the archive itself runs on the dialog's Enter
/// ([`archive_agent_now`]). The **Confirm on archive** SETTING
/// (`ask_before_archive`, on by default) off archives at once — except on
/// a NESTED thread's root, which takes the whole thread and always asks
/// ([`confirm_archive_thread`]). The `a`
/// key and the row menu's Archive both come through here, so the two
/// never differ. True when the session was archived on the spot, so the
/// key can arm the RELEASE WATCH.
pub(crate) fn archive_agent(app: &mut App, id: AgentId, out: &mut Vec<ClientRequest>) -> bool {
    if let Some(band) = crate::launcher::nested_thread(app, &SessionRef::Agent(id.clone())) {
        app.modals.overlay = Some(Overlay::Confirm(confirm_archive_thread(&band)));
        return false;
    }
    if !crate::config::Config::load().ask_before_archive {
        archive_agent_now(app, id, out);
        return true;
    }
    if let Some(a) = app.tree.agents.iter().find(|a| a.id == id) {
        app.modals.overlay = Some(Overlay::Confirm(confirm_archive_agent(&a.name, id)));
    }
    false
}

/// The archive itself: release the pane if it shows the agent, then ask
/// the daemon. From the dialog's Enter (`run_pending_action`), or
/// straight from [`archive_agent`] with the confirm off.
pub(crate) fn archive_agent_now(app: &mut App, id: AgentId, out: &mut Vec<ClientRequest>) {
    detach_if_attached(app, &SessionRef::Agent(id.clone()), out);
    optimistic::set_archived(app, id, true, out);
}

/// `n` of `noun`, the noun plural unless `n` is one.
pub(crate) fn count_of(n: usize, noun: &str) -> String {
    format!("{n} {noun}{}", if n == 1 { "" } else { "s" })
}

/// A NESTED thread's cards split by kind, root first, and their names for
/// the confirm's listing.
pub(crate) fn thread_cards(
    band: &crate::launcher::Band,
) -> (Vec<AgentId>, Vec<TerminalId>, Vec<String>) {
    let mut agents = Vec::new();
    let mut terminals = Vec::new();
    let mut names = Vec::new();
    for i in crate::launcher::thread_order(band) {
        let card = &band.cards[i];
        names.push(card.name().to_string());
        match card {
            crate::launcher::Card::Session(row) => agents.push(row.agent.id.clone()),
            crate::launcher::Card::Terminal(t) => terminals.push(t.id.clone()),
        }
    }
    (agents, terminals, names)
}

/// The confirm before a NESTED thread's root is archived: it takes every
/// session in the thread to the archive and closes every terminal, so the
/// dialog always asks, whatever **Confirm on archive** says — a shell
/// killed is not one `u` brings back — and lists what goes.
pub(crate) fn confirm_archive_thread(band: &crate::launcher::Band) -> ConfirmDialog {
    let (agents, terminals, names) = thread_cards(band);
    let mut message = format!(
        "Archive '{}' and everything nested under it?\n{} archived — u brings {} back.",
        names[0],
        count_of(agents.len(), "session"),
        if agents.len() == 1 { "it" } else { "them" },
    );
    if !terminals.is_empty() {
        message.push_str(&format!(
            "\n{} closed — their shells are killed.",
            count_of(terminals.len(), "terminal")
        ));
    }
    message.push('\n');
    message.push_str(&bulk_confirm_listing(&names));
    ConfirmDialog {
        title: format!("Archive thread · {}", count_of(names.len(), "row")),
        message,
        action: PendingAction::ArchiveThread { agents, terminals },
        area: ratatui::layout::Rect::default(),
    }
}

/// The confirm before a NESTED thread's root is deleted: every session
/// and terminal in the thread goes with it, listed, with the worktree
/// question folded in as for any delete that empties one
/// ([`with_worktree_offer`]) — never asked of the ROOT WORKTREE.
pub(crate) fn confirm_delete_thread(app: &App, band: &crate::launcher::Band) -> ConfirmDialog {
    let (agents, terminals, names) = thread_cards(band);
    let gone: Vec<String> = [(agents.len(), "session"), (terminals.len(), "terminal")]
        .into_iter()
        .filter(|&(n, _)| n > 0)
        .map(|(n, noun)| count_of(n, noun))
        .collect();
    let dialog = ConfirmDialog {
        title: format!("Delete thread · {}", count_of(names.len(), "row")),
        message: format!(
            "Delete '{}' and everything nested under it?\n{} go away, history and shells.\n{}",
            names[0],
            gone.join(" and "),
            bulk_confirm_listing(&names),
        ),
        action: PendingAction::DeleteAllSessions { agents, terminals },
        area: ratatui::layout::Rect::default(),
    };
    with_worktree_offer(app, dialog, &band.worktree, band.cards.len())
}

/// The confirm before an agent is archived. The message says why saying
/// yes is cheap: `u` undoes it.
pub(crate) fn confirm_archive_agent(name: &str, id: AgentId) -> ConfirmDialog {
    ConfirmDialog {
        title: "Archive agent".into(),
        message: format!("Archive agent '{name}'? It leaves the list; u brings it back."),
        action: PendingAction::ArchiveAgent(id),
        area: ratatui::layout::Rect::default(),
    }
}

/// Swap the LAUNCHER VIEW's GRID between a project's live sessions and
/// its archived ones (`⇧A`, the grid menu's **Show/hide archived**).
///
/// INPUT PARITY: the key reaches `launcher::toggle_archived` through the
/// grid's own `handle_action`; the menu and everything else reaches it
/// here, so both ends land the cursor the same way.
pub(crate) fn toggle_archived(app: &mut App, out: &mut Vec<ClientRequest>) {
    launcher::toggle_archived(app, out);
}

/// Fold/unfold the Worktrees panel's OPEN PRS group (header click, context
/// menu; ↓ off the last checkout unfolds it too — see `move_selection`).
/// Folding while the cursor sits on a pull request re-lands it on the last
/// checkout and brings that checkout's session up, as an arrow key onto it
/// would: the PTY underneath is deliberately left attached while the
/// cursor is in the group, so without this the pane would keep showing a
/// session the cursor is no longer on.
pub(crate) fn toggle_open_prs(app: &mut App, out: &mut Vec<ClientRequest>) {
    let on_pr = app.selected_worktree_pr().is_some();
    // A checkout under a pull request rejoins the plain rows when the
    // group folds, and leaves them again when it opens: the cursor keeps
    // the checkout, wherever the fold puts it.
    let checkout = app.selected_worktree().map(|w| w.id.clone());
    app.launcher.open_prs_collapsed = !app.launcher.open_prs_collapsed;
    follow_checkout(app, checkout.as_ref());
    if on_pr {
        // The last checkout — not the last row, which with an ISSUES
        // group open below would be an issue.
        app.nav.sel_worktree = last_checkout_row(app);
        if app.selected_worktree().is_some() {
            restore_session(app, out);
            // A fold is an explicit act, like an archive: the row the
            // cursor got pushed onto is where it stays, so there is no
            // key-walk sweep to wait out before attaching.
            fire_pending_attach(app, out);
        }
        schedule_pr_detail(app);
    }
    app.chrome.dirty = true;
}

/// The last checkout row of the Worktrees panel — where a fold lands a
/// cursor it took the row from. The first row when the project has none.
pub(crate) fn last_checkout_row(app: &App) -> usize {
    app.worktree_rows()
        .iter()
        .rposition(|row| row.checkout().is_some())
        .unwrap_or(0)
}

/// Fold/unfold the Worktrees panel's ISSUES group (header click, context
/// menu; ↓ off the row above it unfolds it too — see `move_selection`).
/// Folding while the cursor sits on an issue re-lands it on the row above
/// the header — the last pull request, whose preview the loop's
/// `note_preview_change` brings up, or the last checkout, whose session
/// comes back the way the OPEN PRS fold brings it back.
pub(crate) fn toggle_issues(app: &mut App, out: &mut Vec<ClientRequest>) {
    let on_issue = app.selected_worktree_issue().is_some();
    app.launcher.issues_collapsed = !app.launcher.issues_collapsed;
    if on_issue {
        app.nav.sel_worktree = app.worktree_row_count().saturating_sub(1);
        if app.selected_worktree().is_some() {
            restore_session(app, out);
            fire_pending_attach(app, out);
        }
    }
    app.chrome.dirty = true;
}

/// Re-seat the Worktrees cursor on checkout `id` after the rows regrouped
/// under it — a fold, a draft toggle, a fresh open-list answer — each
/// of which can move a checkout under its pull request's row or back out
/// among the plain ones (`App::worktree_rows`). The pane needs nothing:
/// the worktree under the cursor is the one it was showing. A cursor
/// that was not on a checkout, or whose checkout is no longer a row, is
/// left for the caller's own landing.
pub(crate) fn follow_checkout(app: &mut App, id: Option<&WorktreeId>) {
    if let Some(i) = id.and_then(|id| app.worktree_row_of(id)) {
        if app.nav.sel_worktree != i {
            app.nav.sel_worktree = i;
            app.chrome.dirty = true;
        }
    }
}

/// Shift+T: create a shell terminal whose pwd is the selection's checkout —
/// the selected worktree, or the project's main checkout (root) when the
/// Projects panel has focus. The daemon names it (`term-N`) and the Ack
/// attaches it, so one keypress lands in a ready shell.
pub(crate) fn create_terminal_for_context(app: &mut App, out: &mut Vec<ClientRequest>) {
    let Some(worktree) = worktree_in_context(app) else {
        app.chrome.flash = Some(SELECT_CONTEXT_FIRST.into());
        return;
    };
    create_terminal(app, worktree, out);
}

/// Ask the daemon for a shell terminal in `worktree`; the Ack attaches it.
pub(crate) fn create_terminal(app: &mut App, worktree: WorktreeId, out: &mut Vec<ClientRequest>) {
    if app.is_placeholder_worktree(&worktree) {
        app.chrome.flash = Some(WORKTREE_STILL_CREATING.into());
        return;
    }
    worked_in(app, &worktree);
    send_with(
        app,
        out,
        PendingIntent::AttachCreated {
            focus: true,
            placeholder: None,
        },
        |req_id| ClientRequest::CreateTerminal {
            req_id,
            worktree,
            name: None,
        },
    );
}

/// The worktree the selection stands for: the selected one, or the selected
/// project's main checkout (root) when the Projects panel has focus.
pub(crate) fn worktree_in_context(app: &App) -> Option<WorktreeId> {
    match app.nav.focus {
        Focus::Projects => app
            .selected_project()
            .and_then(|p| app.root_worktree(&p.id)),
        _ => app.selected_worktree().map(|w| w.id.clone()),
    }
}

pub(crate) fn open_delete_confirm(app: &mut App) {
    match app.nav.focus {
        Focus::Projects => {
            if let Some(p) = app.selected_project() {
                app.modals.overlay = Some(Overlay::Confirm(confirm_remove_project(
                    &p.name,
                    p.id.clone(),
                )));
            }
        }
        Focus::Worktrees => {
            if let Some(id) = app.selected_worktree().map(|w| w.id.clone()) {
                activate::delete_worktree(app, &id);
            }
        }
        // An EMPTY BAND on the grid: the worktree is all there is — even
        // with its pull request's link row under the cursor (#104).
        Focus::Sessions => match launcher::empty_band(app) {
            Some(id) => activate::delete_worktree(app, &id),
            None => match app.selected_session_row() {
                Some(SessionRow::Agent(a)) => {
                    app.modals.overlay = Some(Overlay::Confirm(confirm_delete_agent_in(app, &a)));
                }
                Some(SessionRow::Terminal(t)) => {
                    app.modals.overlay = Some(Overlay::Confirm(confirm_close_terminal_in(app, &t)));
                }
                Some(SessionRow::Link(l)) => delete_link(app, &l),
                None => {}
            },
        },
        Focus::Terminal => {}
    }
}

/// The confirm before an agent is deleted — from the `d` key and the row
/// menu alike, so the two never drift apart in wording.
pub(crate) fn confirm_delete_agent(name: &str, id: AgentId) -> ConfirmDialog {
    ConfirmDialog {
        title: "Delete agent".into(),
        message: format!("Delete agent '{name}'? Its session and history go away."),
        action: PendingAction::DeleteAgent(id),
        area: ratatui::layout::Rect::default(),
    }
}

/// The confirm before a terminal tab is closed (key and menu).
pub(crate) fn confirm_close_terminal(name: &str, id: TerminalId) -> ConfirmDialog {
    ConfirmDialog {
        title: "Close terminal".into(),
        message: format!("Close terminal '{name}'? Its shell is killed."),
        action: PendingAction::CloseTerminal(id),
        area: ratatui::layout::Rect::default(),
    }
}

/// The confirm before the TUI closes. `q` sits one finger away from every
/// other panel hotkey, so a stray letter aimed at an agent used to end the
/// client outright; the gate costs one keystroke and the message says why
/// it is cheap — the daemon keeps every session running.
pub(crate) fn confirm_quit() -> ConfirmDialog {
    ConfirmDialog {
        title: "Quit nebula".into(),
        // Sized to the longest line, never wrapped: keep both under 52.
        message: "Leave the TUI?\nSessions keep running in the daemon.".into(),
        action: PendingAction::Quit,
        area: ratatui::layout::Rect::default(),
    }
}

/// The confirm before a project is dropped from the list (key and menu).
pub(crate) fn confirm_remove_project(name: &str, id: ProjectId) -> ConfirmDialog {
    ConfirmDialog {
        title: "Remove project".into(),
        message: format!("Remove '{name}' from nebula? Nothing on disk is touched."),
        action: PendingAction::RemoveProject(id),
        area: ratatui::layout::Rect::default(),
    }
}

/// Edit the URL behind a link row. The detected pull request has no stored
/// row to rewrite — it comes back from git on every lookup.
pub(crate) fn edit_link(app: &mut App, row: &LinkRow) {
    match row.id() {
        Some(id) => open_prompt(app, PromptKind::EditLink { id: id.clone() }),
        None => {
            app.chrome.flash = Some("the pull request comes from git and can't be edited".into())
        }
    }
}

/// Delete a link row, with the same confirm every other `d` gets. The
/// detected pull request isn't ours to delete: it would be back on the next
/// lookup.
pub(crate) fn delete_link(app: &mut App, row: &LinkRow) {
    let Some(id) = row.id() else {
        app.chrome.flash =
            Some("the pull request link can't be deleted — it comes from git".into());
        return;
    };
    app.modals.overlay = Some(Overlay::Confirm(ConfirmDialog {
        title: "Delete link".into(),
        message: format!(
            "Delete link '{}'? Nothing it points at is touched.",
            row.label()
        ),
        action: PendingAction::DeleteLink(id.clone()),
        area: ratatui::layout::Rect::default(),
    }));
}

/// Cap on itemized rows in the bulk-delete confirm; the rest collapse into
/// an "and N more" line so the dialog always fits on screen.
const BULK_CONFIRM_MAX_LISTED: usize = 8;

/// The itemized body of a bulk-delete confirm: one bullet per doomed row.
pub(crate) fn bulk_confirm_listing(names: &[String]) -> String {
    let mut lines: Vec<String> = names
        .iter()
        .take(BULK_CONFIRM_MAX_LISTED)
        .map(|n| format!("  • {n}"))
        .collect();
    if names.len() > BULK_CONFIRM_MAX_LISTED {
        lines.push(format!(
            "  … and {} more",
            names.len() - BULK_CONFIRM_MAX_LISTED
        ));
    }
    lines.join("\n")
}

/// Shift+D: confirm deleting EVERY row of the focused panel — all worktrees
/// of the selected project, or all sessions the panel shows. The dialog
/// itemizes the casualties so the blast radius is unmistakable.
pub(crate) fn open_delete_all_confirm(app: &mut App) {
    match app.nav.focus {
        Focus::Worktrees => {
            let doomed: Vec<&nebula_core::Worktree> = app
                .visible_worktrees()
                .into_iter()
                .filter(|w| !w.is_main)
                .collect();
            if doomed.is_empty() {
                app.chrome.flash = Some("no deletable worktrees (the main checkout stays)".into());
                return;
            }
            let killed = app
                .tree
                .agents
                .iter()
                .filter(|a| !a.archived && doomed.iter().any(|w| w.id == a.worktree_id))
                .count()
                + app
                    .tree
                    .terminals
                    .iter()
                    .filter(|t| doomed.iter().any(|w| w.id == t.worktree_id))
                    .count();
            let names: Vec<String> = doomed.iter().map(|w| w.branch.clone()).collect();
            let ids: Vec<WorktreeId> = doomed.iter().map(|w| w.id.clone()).collect();
            app.modals.overlay = Some(Overlay::Confirm(ConfirmDialog {
                title: format!("Delete ALL {} worktree(s)", ids.len()),
                message: format!(
                    "Delete these {} worktree(s) from disk? {killed} session(s) will be killed.\n{}\nThe main checkout stays.",
                    ids.len(),
                    bulk_confirm_listing(&names),
                ),
                action: PendingAction::DeleteAllWorktrees(ids),
                area: ratatui::layout::Rect::default(),
            }));
        }
        Focus::Sessions => {
            // What the panel shows is what dies — terminals too, archived
            // rows only when the archived toggle has them visible.
            let doomed = app.visible_session_rows();
            if doomed.is_empty() {
                app.chrome.flash = Some("no sessions to delete".into());
                return;
            }
            // Links are bookmarks, not sessions: `D` never touches them.
            let doomed: Vec<SessionRow> = doomed
                .into_iter()
                .filter(|r| r.as_link().is_none())
                .collect();
            if doomed.is_empty() {
                app.chrome.flash = Some("no sessions to delete".into());
                return;
            }
            let names: Vec<String> = doomed.iter().map(|r| r.name().to_string()).collect();
            let mut agents = Vec::new();
            let mut terminals = Vec::new();
            for row in doomed {
                match row {
                    SessionRow::Agent(a) => agents.push(a.id),
                    SessionRow::Terminal(t) => terminals.push(t.id),
                    SessionRow::Link(_) => unreachable!("filtered out above"),
                }
            }
            // The rows are the selected worktree's: a `D` that takes every
            // live card there asks about the checkout in the same dialog.
            let live_taken = agents
                .iter()
                .filter(|id| app.tree.agents.iter().any(|a| &a.id == *id && !a.archived))
                .count()
                + terminals.len();
            let dialog = ConfirmDialog {
                title: format!("Delete ALL {} session(s)", names.len()),
                message: format!(
                    "Delete these {} session(s)? Their history goes away.\n{}",
                    names.len(),
                    bulk_confirm_listing(&names),
                ),
                action: PendingAction::DeleteAllSessions { agents, terminals },
                area: ratatui::layout::Rect::default(),
            };
            let dialog = match app.selected_worktree().map(|w| w.id.clone()) {
                Some(wt) => with_worktree_offer(app, dialog, &wt, live_taken),
                None => dialog,
            };
            app.modals.overlay = Some(Overlay::Confirm(dialog));
        }
        Focus::Projects | Focus::Terminal => {}
    }
}

/// Row menu for a link: open it, read its diff when it is a pull request,
/// and — unless it's the pull request nebula found in git — edit or delete
/// it.
pub(crate) fn menu_items_for_link(row: &LinkRow) -> Vec<MenuItem> {
    let mut items = vec![MenuItem::new(
        "Open in browser",
        MenuAction::OpenLink(row.url().to_string()),
    )];
    if row.pull_request().is_some() {
        items.push(MenuItem::new("View diff", MenuAction::ViewPrDiff));
        items.push(MenuItem::new("Comment…", MenuAction::CommentPullRequest));
    }
    if let Some(id) = row.id() {
        items.push(MenuItem::new("Edit URL", MenuAction::EditLink(id.clone())));
        items.push(MenuItem::destructive(
            "Delete",
            MenuAction::DeleteLink(id.clone()),
        ));
    }
    items
}

pub(crate) fn menu_items_for_session(a: &nebula_core::Agent) -> Vec<MenuItem> {
    // A Cloud row has no terminal to attach or restart: the agent runs in
    // the cloud sandbox, so its verbs are the browser and the message
    // queue, ahead of the row-keeping ones every session has.
    if let Some(url) = a.cloud_session_url().filter(|_| !a.archived) {
        return vec![
            MenuItem::new("Open in browser", MenuAction::OpenLink(url)),
            MenuItem::new(
                "Send to cloud session",
                MenuAction::SendCloudMessage(a.id.clone()),
            ),
            MenuItem::new("Duplicate", MenuAction::DuplicateAgent(a.id.clone())),
            MenuItem::new("Rename", MenuAction::RenameAgent(a.id.clone())),
            MenuItem::new("Archive", MenuAction::ArchiveAgent(a.id.clone())),
            MenuItem::destructive("Delete", MenuAction::DeleteAgent(a.id.clone())),
        ];
    }
    if a.archived {
        vec![
            MenuItem::new("Unarchive", MenuAction::UnarchiveAgent(a.id.clone())),
            MenuItem::new("Duplicate", MenuAction::DuplicateAgent(a.id.clone())),
            MenuItem::destructive("Delete", MenuAction::DeleteAgent(a.id.clone())),
        ]
    } else {
        vec![
            MenuItem::new(
                "Attach",
                MenuAction::Attach(SessionRef::Agent(a.id.clone())),
            ),
            MenuItem::new("Follow-up prompt", MenuAction::FollowUp),
            MenuItem::new("Restart", MenuAction::RestartAgent(a.id.clone())),
            MenuItem::new("Duplicate", MenuAction::DuplicateAgent(a.id.clone())),
            MenuItem::new("Move to…", MenuAction::MoveAgentPicker(a.id.clone())),
            MenuItem::new("Rename", MenuAction::RenameAgent(a.id.clone())),
            MenuItem::new("Archive", MenuAction::ArchiveAgent(a.id.clone())),
            MenuItem::destructive("Delete", MenuAction::DeleteAgent(a.id.clone())),
        ]
    }
}

/// The card's menu, plus the verbs of the CHECKOUT it runs in — the RUN
/// COMMAND started or stopped, the checkout opened, and a linked one
/// deleted. Those were the WORKTREES PANEL's rows' own, and the grid has
/// no row for a checkout: every card names one instead, so the card's
/// menu is where they go.
pub(crate) fn menu_items_for_session_in(app: &App, a: &nebula_core::Agent) -> Vec<MenuItem> {
    let mut items = menu_items_for_session(a);
    // The card's pull request and the issue it was started from, what `⇧V`
    // and `⇧I` open (`launcher::open_pull_request`, `launcher::open_issue`):
    // ahead of the trailing Delete on an archived card, which keeps no
    // checkout verbs, and after the checkout's Open on a live one.
    let links: Vec<MenuItem> = crate::launcher::row(app, &a.id)
        .and_then(|row| row.pr)
        .map(|pr| MenuItem::new("Open pull request", MenuAction::OpenLink(pr.url)))
        .into_iter()
        .chain(
            a.issue_url
                .clone()
                .map(|url| MenuItem::new("Open issue", MenuAction::OpenLink(url))),
        )
        .collect();
    if a.archived {
        let at = items.len().saturating_sub(1);
        items.splice(at..at, links);
        return items;
    }
    let Some(w) = app
        .tree
        .worktrees
        .iter()
        .find(|w| w.id == a.worktree_id)
        .filter(|w| !app.is_placeholder_worktree(&w.id))
    else {
        return items;
    };
    items.push(MenuItem::new(
        if app.worktree_running(&w.id) {
            "Stop run"
        } else {
            "Run"
        },
        MenuAction::ToggleRun(w.id.clone()),
    ));
    items.push(MenuItem::new(
        "Open",
        MenuAction::OpenWorktree(w.id.clone()),
    ));
    items.extend(links);
    if w.is_main {
        items.push(MenuItem::new(
            "Switch branch…",
            MenuAction::SwitchBranch(w.id.clone()),
        ));
    } else {
        items.push(MenuItem::destructive(
            "Delete worktree",
            MenuAction::DeleteWorktree(w.id.clone()),
        ));
    }
    items
}

pub(crate) fn menu_items_for_terminal(t: &nebula_core::TerminalTab) -> Vec<MenuItem> {
    vec![
        MenuItem::new(
            "Attach",
            MenuAction::Attach(SessionRef::Terminal(t.id.clone())),
        ),
        MenuItem::new("Rename", MenuAction::RenameTerminal(t.id.clone())),
        MenuItem::destructive("Close", MenuAction::CloseTerminal(t.id.clone())),
    ]
}

pub(crate) fn open_menu(app: &mut App, items: Vec<MenuItem>, at: (u16, u16)) {
    if items.is_empty() {
        return;
    }
    app.modals.overlay = Some(Overlay::Menu(ContextMenu {
        title: None,
        items,
        at: Some(at),
        hover: 0,
        area: ratatui::layout::Rect::default(),
        parent: None,
        filter: None,
    }));
}

/// New-session creation, all of it: pick which CLI the session runs, and
/// Enter on the row creates it (`MenuAction::NewAgentOfKind`) — no box
/// follows; the first prompt is typed in the CLI, and a launch that starts
/// from a typed task is the QUICK PROMPT's (`p`). Only the `Claude · cloud`
/// row still asks, a cloud launch being nothing without its task.
/// Claude/Codex rows expand (→) into model and effort submenus; Enter
/// anywhere takes the configured defaults for whatever wasn't drilled into.
/// A plain TERMINAL SESSION is not offered here: NEW TERMINAL (`t`) and the
/// CONTEXT MENU's "New terminal" already cover it.
pub(crate) fn open_new_agent_picker(app: &mut App, worktree: WorktreeId) {
    // A stand-in checkout is not a place the DAEMON knows yet; better to
    // say so here than after a kind and a model were picked.
    if app.is_placeholder_worktree(&worktree) {
        app.chrome.flash = Some(WORKTREE_STILL_CREATING.into());
        return;
    }
    // Only the AGENT KINDS still enabled in the SETTINGS OVERLAY's Agents
    // tab are offered; a disabled harness is absent, not greyed.
    agent_picker::open_kind_picker(app, KindPicker::new_session(worktree));
}

/// ROOT WORKTREE used by PROJECT-scoped actions. An OPEN PRS row has no
/// checkout of its own, so PR-created AGENTS follow the same established
/// fallback as PROJECT-scoped TERMINAL SESSION and LINK creation.
pub(crate) fn selected_project_main_worktree(app: &App) -> Option<WorktreeId> {
    let project = app.selected_project()?;
    app.root_worktree(&project.id)
}

/// `n` on a PROJECT OPEN PRS GROUP row: the NEW SESSION PICKER's harness
/// rows, every one carrying the PR's URL and — through the same MODEL /
/// EFFORT submenus, and as directly on Enter — launching a PR SESSION.
pub(crate) fn open_pr_agent_picker(app: &mut App) {
    let Some(pr) = app.selected_worktree_pr().cloned() else {
        return;
    };
    let Some(worktree) = selected_project_main_worktree(app) else {
        app.chrome.flash = Some("the project has no ROOT WORKTREE for this PR session".into());
        return;
    };
    agent_picker::open_kind_picker(app, KindPicker::pr_session(worktree, &pr));
}

/// The CONTEXT MENU for a PROJECT ISSUES GROUP row: the browser. The
/// launches are the row's keys (`p`, `e`), as the footer says.
pub(crate) fn issue_row_menu_items(issue: &crate::issues::Issue) -> Vec<MenuItem> {
    vec![MenuItem::new(
        "Open in browser",
        MenuAction::OpenLink(issue.url.clone()),
    )]
}

/// The CONTEXT MENU for a PROJECT OPEN PRS GROUP row: a PR SESSION row per
/// enabled harness (none when the PROJECT has no ROOT WORKTREE to launch
/// in), then the row's browser and diff verbs.
pub(crate) fn pr_row_menu_items(app: &App, pr: &crate::pull_request::OpenPr) -> Vec<MenuItem> {
    let mut items = match selected_project_main_worktree(app) {
        Some(worktree) => agent_picker::pr_session_menu_rows(worktree, pr),
        None => Vec::new(),
    };
    items.extend([
        MenuItem::new("Open in browser", MenuAction::OpenLink(pr.url.clone())),
        MenuItem::new("View diff", MenuAction::ViewPrDiff),
        MenuItem::new("Comment…", MenuAction::CommentPullRequest),
    ]);
    items
}

/// Build the submenu a menu row expands into: the model list for a
/// new-session kind row, or the effort list for a model row. Rows carry the
/// full choice so Enter works the same at any depth; the row matching the
/// configured default starts highlighted.
pub(crate) fn build_submenu(item: &MenuItem) -> Option<ContextMenu> {
    let sub = item.action.submenu()?;
    let MenuAction::NewAgentOfKind {
        worktree,
        kind,
        custom,
        model,
        cloud,
        pr,
        quick,
        ..
    } = &item.action
    else {
        return None;
    };
    let cfg = crate::config::Config::load();
    // The ✓ marks what Enter would take: the AGENTS TAB default, or — in a
    // QUICK PROMPT picker still on the box's own harness — what that box is
    // already set to launch with.
    let from_box = quick
        .as_ref()
        .filter(|q| q.launch.kind == *kind && q.launch.custom.as_deref() == custom.as_deref())
        .map(|q| &q.launch);
    let (title, choices, configured) = match sub {
        SubmenuKind::Models => (
            format!(
                "{} model",
                crate::agent_picker::harness_label(*kind, custom.as_deref())
            ),
            crate::config::model_choices(*kind, custom.as_deref()),
            from_box
                .and_then(|l| l.model.clone())
                .or_else(|| cfg.default_model(*kind)),
        ),
        SubmenuKind::Efforts => (
            format!("{} effort", kind_label(*kind)),
            crate::config::effort_choices(*kind, model.as_deref(), custom.as_deref()),
            from_box
                .and_then(|l| l.effort.clone())
                .or_else(|| cfg.default_effort(*kind)),
        ),
    };
    let configured = configured.unwrap_or_else(|| crate::config::DEFAULT_CHOICE.into());
    let items: Vec<MenuItem> = choices
        .iter()
        .map(|choice| {
            MenuItem::new(
                if *choice == configured {
                    format!("{choice} ✓")
                } else {
                    (*choice).to_string()
                },
                MenuAction::NewAgentOfKind {
                    worktree: worktree.clone(),
                    kind: *kind,
                    custom: custom.clone(),
                    model: match sub {
                        SubmenuKind::Models => Some((*choice).to_string()),
                        SubmenuKind::Efforts => model.clone(),
                    },
                    effort: match sub {
                        SubmenuKind::Models => None,
                        SubmenuKind::Efforts => Some((*choice).to_string()),
                    },
                    cloud: *cloud,
                    pr: pr.clone(),
                    quick: quick.clone(),
                },
            )
        })
        .collect();
    let hover = choices.iter().position(|c| *c == configured).unwrap_or(0);
    // Both lists take type-ahead: letters narrow the rows, ↑/↓ move.
    let filter = Some(MenuFilter {
        query: String::new(),
        all: items.clone(),
    });
    Some(ContextMenu {
        title: Some(title),
        items,
        at: None,
        hover,
        area: ratatui::layout::Rect::default(),
        parent: None,
        filter,
    })
}

/// The QUICK PROMPT box a picker menu owes back, read off any of its rows
/// (every row of a quick picker carries the same one).
pub(crate) fn menu_quick_return(menu: &ContextMenu) -> Option<crate::quick_prompt::QuickReturn> {
    menu.items.iter().find_map(|item| match &item.action {
        MenuAction::NewAgentOfKind { quick, .. } => quick.as_deref().cloned(),
        MenuAction::PickLaunchWorktree { back, .. } => Some((**back).clone()),
        _ => None,
    })
}

/// A checkout's context menu: what a right-click on an EMPTY BAND opens.
pub(crate) fn worktree_menu_items(app: &App, w: &nebula_core::Worktree) -> Vec<MenuItem> {
    let run = if app.worktree_running(&w.id) {
        "Stop run"
    } else {
        "Run"
    };
    let mut items = vec![
        MenuItem::new("New agent", MenuAction::NewAgent(w.id.clone())),
        MenuItem::new("New terminal", MenuAction::NewTerminal(w.id.clone())),
        MenuItem::new(run, MenuAction::ToggleRun(w.id.clone())),
        MenuItem::new("Open", MenuAction::OpenWorktree(w.id.clone())),
    ];
    // The ROOT WORKTREE moves between branches; a linked worktree, named
    // for its branch, is deleted instead.
    if w.is_main {
        items.push(MenuItem::new(
            "Switch branch…",
            MenuAction::SwitchBranch(w.id.clone()),
        ));
    } else {
        items.push(MenuItem::destructive(
            "Delete worktree",
            MenuAction::DeleteWorktree(w.id.clone()),
        ));
    }
    items
}

/// The CONTEXT MENU of the row under `focus`'s cursor — what a
/// right-click on a row opens once the click has moved the cursor onto it
/// (`select_clicked_row`). One list per row kind, built here and nowhere
/// else. None where there is no row to have one.
pub(crate) fn context_menu_items(app: &App, focus: Focus) -> Option<Vec<MenuItem>> {
    match focus {
        Focus::Projects => {
            let mut items = vec![MenuItem::new("Add project", MenuAction::AddProject)];
            if let Some(p) = app.selected_project() {
                items.insert(
                    0,
                    MenuItem::new("New worktree", MenuAction::NewWorktree(p.id.clone())),
                );
                items.push(MenuItem::new(
                    "Rename",
                    MenuAction::RenameProject(p.id.clone()),
                ));
                items.push(MenuItem::destructive(
                    "Remove from list",
                    MenuAction::RemoveProject(p.id.clone()),
                ));
            }
            Some(items)
        }
        Focus::Worktrees => match app.selected_worktree_pr() {
            Some(pr) => Some(pr_row_menu_items(app, pr)),
            None => match app.selected_worktree_issue() {
                Some(issue) => Some(issue_row_menu_items(issue)),
                None => app.selected_worktree().map(|w| worktree_menu_items(app, w)),
            },
        },
        // An EMPTY BAND on the grid: its checkout's own menu, the same
        // **Delete worktree** its `d` opens — its pull request's link row
        // under the cursor or not (#104). A folded worktree's header (the
        // NESTED layout) is the checkout too, never the card it hides.
        Focus::Sessions => match launcher::empty_band(app).or_else(|| launcher::folded_band(app)) {
            Some(id) => {
                let w = app.tree.worktrees.iter().find(|w| w.id == id)?;
                Some(worktree_menu_items(app, w))
            }
            None => match app.selected_session_row()? {
                SessionRow::Agent(a) => Some(menu_items_for_session_in(app, &a)),
                SessionRow::Terminal(t) => Some(menu_items_for_terminal(&t)),
                SessionRow::Link(l) => Some(menu_items_for_link(&l)),
            },
        },
        Focus::Terminal => None,
    }
}

/// The menu of a panel's empty background — the right button's alone: the
/// keyboard has no cursor to park there, and reaches every one of these
/// verbs by its own key (`n`, `A`, the OPEN PRS fold, the draft toggle).
pub(crate) fn panel_menu_items(app: &App, focus: Focus) -> Vec<MenuItem> {
    match focus {
        Focus::Projects => vec![MenuItem::new("Add project", MenuAction::AddProject)],
        Focus::Worktrees => app
            .selected_project()
            .map(|p| {
                let mut items = vec![MenuItem::new(
                    "New worktree",
                    MenuAction::NewWorktree(p.id.clone()),
                )];
                // Only once there is a group to fold.
                if !app.listed_open_prs().is_empty() {
                    items.push(MenuItem::new(
                        "Show/hide open PRs",
                        MenuAction::ToggleOpenPrs,
                    ));
                }
                if !app.listed_issues().is_empty() {
                    items.push(MenuItem::new("Show/hide issues", MenuAction::ToggleIssues));
                }
                // And drafts to hide — or, once hidden, a way back that
                // doesn't need the list to still hold one.
                if app.launcher.hide_draft_prs || app.all_open_prs().iter().any(|pr| pr.is_draft) {
                    let label = if app.launcher.hide_draft_prs {
                        "Show draft PRs"
                    } else {
                        "Hide draft PRs"
                    };
                    items.push(MenuItem::new(label, MenuAction::ToggleDraftPrs));
                }
                items
            })
            .unwrap_or_default(),
        Focus::Sessions => app
            .selected_worktree()
            .map(|w| {
                vec![
                    MenuItem::new("New agent", MenuAction::NewAgent(w.id.clone())),
                    MenuItem::new("Show/hide archived", MenuAction::ToggleArchived),
                ]
            })
            .unwrap_or_default(),
        Focus::Terminal => vec![],
    }
}

/// A click landed on a panel row: the cursor goes there exactly as the
/// arrow keys take it — `select_project_row`, `select_worktree_row`, `select_session_row`, each with everything a
/// move entails (the context being left remembered, the one arrived at
/// restored, the pane brought along) — and the panel takes FOCUS. Either
/// button: the left goes on to its double-click, the right to the row's
/// CONTEXT MENU. The right button used to set the cursor fields itself, so
/// a right-click on another checkout left the pane on the old one's
/// session under a cursor that had moved away. False for a target that is
/// not a row. A card's PULL REQUEST LINE is the card to this button: the
/// menu it opens carries **Open pull request** already.
pub(crate) fn select_clicked_row(
    app: &mut App,
    target: &HitTarget,
    out: &mut Vec<ClientRequest>,
) -> bool {
    match *target {
        HitTarget::LauncherCard(at) => launcher::select_card_row(app, at, out),
        HitTarget::LauncherBand(i)
        | HitTarget::LauncherBandMore(i)
        | HitTarget::LauncherBandFold(i) => launcher::select_band_row(app, i, out),
        HitTarget::LauncherBandPr(ref wid) => launcher::select_band_of(app, wid, out),
        HitTarget::LauncherCardIssue(ref id) => launcher::select_issue_card(app, id, out),
        _ => false,
    }
}

/// Move the Sessions cursor to row `i` and show that session in the pane,
/// without FOCUS or the input lock — what ↑/↓ do on the way past (behind
/// `debounce`, so a sweep boots nothing) and what a click does at once. An
/// archived row and a link row are selected and preview nothing
/// (`preview_inner`).
pub(crate) fn select_session_row(
    app: &mut App,
    i: usize,
    debounce: Duration,
    out: &mut Vec<ClientRequest>,
) {
    app.nav.sel_session = i;
    preview_inner(app, debounce, out);
}
