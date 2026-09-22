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
    create_agent, placeholder, remember_launch, schedule_prewarm, select_worktree_by_id,
    send_with_follow, AgentLaunchDraft,
};
use crate::app::{App, PendingIntent, PlaceholderRows, PromptKind};
use crate::quick_prompt::{QuickLaunch, QuickTarget};
use nebula_core::{AgentId, ClientRequest, WorktreeId};

/// Enter in the box, with `text` already sized — and non-empty, unless
/// the box `launches_empty` (one an AGENT PRESET is on, sent as it is).
///
/// `take_pane` is `⌘Enter`: the same launch, with FOCUS going down into
/// the new session's pane once it is acked, whatever the
/// `quick_prompt_focus` SETTING says. It is never a BACKGROUND LAUNCH:
/// from a box re-aimed at another project, asking to be put in the
/// session is asking to be taken there, so the grid goes to that project
/// first — through the PROJECT TAB click's own `open_tab` — and the
/// launch is an ordinary one into the project on screen.
pub(super) fn submit(
    app: &mut App,
    launch: QuickLaunch,
    text: String,
    take_pane: bool,
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
    let project = crate::launcher::project_of(app, &launch.target);
    let elsewhere = project
        .as_ref()
        .is_some_and(|project| crate::launcher::is_background(app, project));
    if let Some(project) = project.as_ref().filter(|_| take_pane && elsewhere) {
        super::launcher::open_tab(app, project, out);
        // An empty project's "no sessions yet" is about to be untrue.
        app.flash = None;
    }
    // The launch lands on a card, so the GRID has an aim again whether or
    // not it had one when the box went up (`launcher::clear_aim`).
    super::launcher::take_aim(app);
    // A box re-aimed with `^P` fires into a project the screen is not
    // showing: the session starts there and the user keeps working here,
    // so nothing this launch does may move a cursor, a tab or the pane. The Acks are born left behind for it, and the stand-in rows
    // stop at going up (`placeholder::stage_agent`).
    let background = elsewhere && !take_pane;
    if background {
        announce_background(app, &launch.target);
    } else {
        reveal_pane(app);
    }
    // The box is the one launch that stays out of the way by default:
    // `p`, type, Enter, keep working, the new session in the pane but the
    // keys still on the cards. `⌘Enter` takes the pane instead, as a
    // picker-walked launch (`n`) does.
    let focus_pane = !background && (take_pane || crate::config::Config::load().quick_prompt_focus);
    match launch.target.clone() {
        QuickTarget::Worktree(worktree) => {
            let draft = AgentLaunchDraft {
                follow: !background,
                ..draft(launch, worktree, text, focus_pane, None)
            };
            create_agent(app, draft, out)
        }
        QuickTarget::NewWorktree { project, branch } => {
            // The rows first, so the panels never wait on git.
            // The composed task the create will carry (`draft`), asked
            // here so the row goes up the way it will come back: working.
            let first_prompt = !launch.compose(&text).is_empty();
            let placeholder = placeholder::stage(
                app,
                project.clone(),
                branch.clone(),
                launch.kind,
                launch.custom.clone(),
                launch.model.clone(),
                launch.effort.clone(),
                first_prompt,
                out,
            );

            // `base: None` is the DAEMON's `worktree_base_branch` SETTING,
            // else its fetched `origin/HEAD` (`git::add_worktree_off_default`)
            // — never this checkout's HEAD.
            send_with_follow(
                app,
                out,
                PendingIntent::LaunchInCreatedWorktree {
                    launch,
                    text,
                    placeholder,
                    focus: focus_pane,
                },
                !background,
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

/// What every launch the screen follows does at once: the new session is
/// put where it can be seen without the keys going with it. The PANE
/// under the grid unfolds if `^~` had folded it away, and comes off any
/// TERMINAL tab it was pinned to — a shell in the same checkout would
/// otherwise go on standing in front of the session the Ack attaches.
/// Where FOCUS goes is `focus_pane`'s alone (`⌘Enter`,
/// `quick_prompt_focus`).
fn reveal_pane(app: &mut App) {
    app.launcher_pane_hidden = false;
    app.launcher_terminal = None;
    app.dirty = true;
}

/// The only trace a BACKGROUND LAUNCH leaves on screen: the footer names
/// the project it went to, since nothing else here moves and Enter would
/// otherwise look like it did nothing.
fn announce_background(app: &mut App, target: &QuickTarget) {
    let Some(name) = crate::launcher::project_of(app, target)
        .and_then(|project| crate::launcher::project_name(app, &project))
    else {
        return;
    };
    app.flash = Some(format!("started a session in {name}"));
}

/// The Ack for that `CreateWorktree`: `worktree` exists now, launch there.
/// Without `follow` — the user navigated away while the checkout was cut
/// (`App::left_behind`) — the launch still goes out, but no cursor moves
/// back onto the row, now or at the session's own Ack. `focus` is the
/// pane the box's Enter settled on (`submit`).
#[allow(clippy::too_many_arguments)]
pub(super) fn launch_in_created_worktree(
    app: &mut App,
    mut launch: QuickLaunch,
    text: String,
    placeholder: PlaceholderRows,
    worktree: WorktreeId,
    follow: bool,
    focus: bool,
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
            ..draft(launch, worktree, text, focus, Some(placeholder.agent))
        },
        out,
    );
}

/// The create itself, the same on both routes. An empty name opts the
/// row into AUTO-TITLE, so the session names itself from the very prompt
/// that started it, and the box comes back with the text should the
/// DAEMON refuse. `focus_pane` is whether the Ack takes the pane.
pub(super) fn draft(
    launch: QuickLaunch,
    worktree: WorktreeId,
    text: String,
    focus_pane: bool,
    placeholder: Option<AgentId>,
) -> AgentLaunchDraft {
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

#[cfg(test)]
mod tests {
    //! The box's two sends in the LAUNCHER VIEW, through the loop's own
    //! entry points: Enter puts the new session in the pane and leaves the
    //! keys on the cards; `⌘Enter` steps down into it.
    use super::super::tests::{buffer_text, hse, seed_tree, with_default_config};
    use super::super::{handle_server_event, handle_terminal_event};
    use crate::app::{App, Focus};
    use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
    use nebula_core::{
        Agent, AgentId, AgentKind, AgentStatus, ClientRequest, Entity, EntityId, Project,
        ProjectId, ServerEvent, SessionRef, TerminalId, TerminalTab, Worktree, WorktreeId,
    };
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    fn draw(app: &mut App) {
        let mut terminal = Terminal::new(TestBackend::new(130, 34)).unwrap();
        terminal.draw(|f| crate::ui::draw(f, app)).unwrap();
    }

    fn key(app: &mut App, code: KeyCode, mods: KeyModifiers) -> Vec<ClientRequest> {
        let mut out = Vec::new();
        handle_terminal_event(app, Event::Key(KeyEvent::new(code, mods)), &mut out);
        out
    }

    fn type_text(app: &mut App, text: &str) {
        for c in text.chars() {
            key(app, KeyCode::Char(c), KeyModifiers::NONE);
        }
    }

    fn pane(app: &App) -> Option<SessionRef> {
        app.term.as_ref().map(|t| t.sref.clone())
    }

    /// The box's send, `mods` held on the Enter: the create it put out.
    fn send(app: &mut App, mods: KeyModifiers) -> (u64, WorktreeId) {
        let out = key(app, KeyCode::Enter, mods);
        assert!(app.overlay.is_none(), "the box launched");
        match out.as_slice() {
            [ClientRequest::CreateAgent {
                req_id, worktree, ..
            }] => (*req_id, worktree.clone()),
            other => panic!("one CreateAgent: {other:?}"),
        }
    }

    /// `p`, a task, and the send.
    fn launch(app: &mut App, mods: KeyModifiers) -> (u64, WorktreeId) {
        key(app, KeyCode::Char('p'), KeyModifiers::NONE);
        type_text(app, "tidy the nav");
        send(app, mods)
    }

    /// The DAEMON's side of it: the row `a9` in `worktree`, then the Ack.
    fn acked(app: &mut App, req_id: u64, worktree: &WorktreeId) {
        hse(
            app,
            ServerEvent::EntityUpserted {
                entity: Entity::Agent(Agent {
                    id: AgentId("a9".into()),
                    worktree_id: worktree.clone(),
                    name: "agent-2".into(),
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
                    recent_prompts: Vec::new(),
                }),
            },
        );
        let mut out = Vec::new();
        handle_server_event(
            app,
            ServerEvent::Ack {
                req_id,
                created: Some(EntityId::Agent(AgentId("a9".into()))),
            },
            &mut out,
        );
        draw(app);
    }

    fn new_session() -> Option<SessionRef> {
        Some(SessionRef::Agent(AgentId("a9".into())))
    }

    /// Enter shows what it started: a pane folded away with `^~` unfolds,
    /// reading the new session once it is acked — and the keys stay on
    /// the cards, so the next `p` is one key away.
    #[test]
    fn enter_unfolds_the_pane_onto_the_new_session_and_keeps_the_keys() {
        with_default_config(|| {
            let mut app = App::new();
            seed_tree(&mut app);
            draw(&mut app);
            key(&mut app, KeyCode::Char('~'), KeyModifiers::NONE);
            assert!(app.launcher_pane_hidden, "folded away to start with");

            let (req_id, worktree) = launch(&mut app, KeyModifiers::NONE);
            assert!(!app.launcher_pane_hidden, "the send unfolds the pane");
            acked(&mut app, req_id, &worktree);

            assert_eq!(pane(&app), new_session(), "the pane reads the new session");
            assert!(
                app.launcher_split(app.launcher_body).1.is_some(),
                "and it is on screen"
            );
            assert_eq!(app.focus, Focus::Sessions, "the keys stay on the cards");
            assert!(!app.term_locked);
        });
    }

    /// The box says `⌘Enter` exists: nothing else in it does.
    #[test]
    fn the_box_border_names_cmd_enter() {
        with_default_config(|| {
            let mut app = App::new();
            seed_tree(&mut app);
            draw(&mut app);
            key(&mut app, KeyCode::Char('p'), KeyModifiers::NONE);
            let mut terminal = Terminal::new(TestBackend::new(130, 34)).unwrap();
            terminal.draw(|f| crate::ui::draw(f, &mut app)).unwrap();
            let screen = buffer_text(&terminal);
            if std::env::var_os("SHOW_BOX").is_some() {
                eprintln!("{screen}");
            }
            assert!(
                screen.contains("⌘Enter launch + focus"),
                "the hint is on the border:\n{screen}"
            );
        });
    }

    /// A pane pinned to a TERMINAL of the same checkout lets it go: the
    /// shell would otherwise go on standing in front of the new session.
    #[test]
    fn enter_takes_the_pane_off_a_terminal_tab() {
        with_default_config(|| {
            let mut app = App::new();
            seed_tree(&mut app);
            hse(
                &mut app,
                ServerEvent::EntityUpserted {
                    entity: Entity::Terminal(TerminalTab {
                        id: TerminalId("t1".into()),
                        worktree_id: WorktreeId("w1".into()),
                        name: "shell-1".into(),
                        sort_order: 0,
                        alive: true,
                        run_command: None,
                    }),
                },
            );
            draw(&mut app);
            key(&mut app, KeyCode::Char('`'), KeyModifiers::NONE);
            assert!(app.pinned_terminal().is_some(), "the pane is on the shell");

            let (req_id, worktree) = launch(&mut app, KeyModifiers::NONE);
            assert_eq!(app.launcher_terminal, None, "the pin is let go of");
            acked(&mut app, req_id, &worktree);
            assert_eq!(pane(&app), new_session());
            assert_eq!(app.focus, Focus::Sessions);
        });
    }

    /// `⌘Enter` is the same launch, stepping down into the new session's
    /// pane with its input locked — `^Enter` too, for the terminal that
    /// keeps `⌘Enter` for itself.
    #[test]
    fn cmd_enter_launches_and_steps_down_into_the_new_session() {
        for mods in [KeyModifiers::SUPER, KeyModifiers::CONTROL] {
            with_default_config(|| {
                let mut app = App::new();
                seed_tree(&mut app);
                draw(&mut app);
                let (req_id, worktree) = launch(&mut app, mods);
                assert!(!app.launcher_pane_hidden);
                acked(&mut app, req_id, &worktree);

                assert_eq!(pane(&app), new_session(), "{mods:?}");
                assert_eq!(app.focus, Focus::Terminal, "{mods:?} takes the pane");
                assert!(app.term_locked, "{mods:?} locks it");
            });
        }
    }

    /// From a box re-aimed with `^P`, Enter is a BACKGROUND LAUNCH, but
    /// `⌘Enter` asks to be put in the session — so it goes there: no
    /// footer note, not born left behind, and the Ack lands the grid on
    /// that project with the pane taken.
    #[test]
    fn cmd_enter_from_a_box_aimed_elsewhere_goes_there() {
        with_default_config(|| {
            let mut app = App::new();
            seed_tree(&mut app);
            hse(
                &mut app,
                ServerEvent::EntityUpserted {
                    entity: Entity::Project(Project {
                        id: ProjectId("p2".into()),
                        name: "web".into(),
                        repo_path: "/tmp/web".into(),
                        sort_order: 1,
                    }),
                },
            );
            hse(
                &mut app,
                ServerEvent::EntityUpserted {
                    entity: Entity::Worktree(Worktree {
                        id: WorktreeId("w2root".into()),
                        project_id: ProjectId("p2".into()),
                        path: "/tmp/web".into(),
                        branch: "main".into(),
                        is_main: true,
                        sort_order: 0,
                    }),
                },
            );
            draw(&mut app);
            assert_eq!(
                app.selected_project().map(|p| p.name.as_str()),
                Some("demo")
            );

            key(&mut app, KeyCode::Char('p'), KeyModifiers::NONE);
            type_text(&mut app, "tidy the nav");
            key(&mut app, KeyCode::Char('p'), KeyModifiers::CONTROL);
            type_text(&mut app, "we");
            key(&mut app, KeyCode::Enter, KeyModifiers::NONE);
            let (req_id, worktree) = send(&mut app, KeyModifiers::SUPER);

            assert_eq!(worktree.0, "w2root", "into the project the box aimed at");
            assert!(!app.left_behind.contains(&req_id), "the Ack may follow");
            assert_ne!(app.flash.as_deref(), Some("started a session in web"));
            acked(&mut app, req_id, &worktree);

            assert_eq!(
                app.selected_project().map(|p| p.name.as_str()),
                Some("web"),
                "the grid went to the new session's project"
            );
            assert_eq!(pane(&app), new_session());
            assert_eq!(app.focus, Focus::Terminal);
        });
    }
}
