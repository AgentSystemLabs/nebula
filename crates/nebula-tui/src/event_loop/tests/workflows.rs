use super::*;
use nebula_core::workflow::{StageStatus, StageSummary, WorkflowStatus, WorkflowSummary};

fn workflow(id: &str, status: WorkflowStatus) -> WorkflowSummary {
    WorkflowSummary {
        id: id.into(),
        workflow: Some("default".into()),
        name: Some("Plan and implement".into()),
        branch: format!("workflow-{id}"),
        status,
        current: 0,
        total: 2,
        message: "Awaiting report".into(),
        project: ProjectId("p1".into()),
        worktree: Some(WorktreeId("w1".into())),
        title: "Fix login redirect".into(),
        created_at: 1,
        stages: vec![
            StageSummary {
                id: "planner".into(),
                agent: Some(AgentId("a1".into())),
                status: StageStatus::Running,
            },
            StageSummary {
                id: "implementer".into(),
                agent: None,
                status: StageStatus::Pending,
            },
        ],
    }
}

fn update(app: &mut App, run: WorkflowSummary) {
    hse(app, ServerEvent::WorkflowUpdated { workflow: run });
}

#[test]
fn workflow_progress_streams_without_moving_the_cursor() {
    let mut app = App::new();
    seed_tree(&mut app);
    app.workflows.show = true;
    app.workflows.width = 38;
    app.tree.agents[0].name = "workflow old-id planner".into();
    update(&mut app, workflow("old-id", WorkflowStatus::Running));
    let mut terminal = Terminal::new(TestBackend::new(160, 40)).unwrap();
    terminal.draw(|f| ui::draw(f, &mut app)).unwrap();
    let text = buffer_text(&terminal);
    for needle in [
        "WORKFLOWS",
        "Fix login redirect",
        "Step 1/2 · planner",
        "Next: implementer",
        "0/2 done · 2 left",
        "◆ 1 running",
    ] {
        assert!(text.contains(needle), "missing {needle}:\n{text}");
    }
    assert!(!text.contains("old-id"), "{text}");
    assert_eq!(app.focus, Focus::Projects);
    app.tree.agents[0].name = "My custom title".into();
    assert_eq!(app.agent_label(&app.tree.agents[0]), "My custom title");
    app.workflows.selected = Some("old-id".into());
    let mut run = workflow("old-id", WorkflowStatus::Completed);
    run.current = 2;
    for s in &mut run.stages {
        s.status = StageStatus::Completed;
    }
    update(&mut app, run);
    terminal.draw(|f| ui::draw(f, &mut app)).unwrap();
    let text = buffer_text(&terminal);
    for needle in ["All steps complete", "2/2 done · 0 left", "◆ 0 running"] {
        assert!(text.contains(needle), "{text}");
    }
    assert!(!text.contains("3/2"), "{text}");
    assert_eq!(app.focus, Focus::Projects);
    assert_eq!(app.workflows.runs.len(), 1);
}

#[test]
fn workflow_scope_retains_every_unfinished_run_before_recent_history() {
    let mut app = App::new();
    seed_tree(&mut app);
    for i in 0..60 {
        let mut run = workflow(&format!("completed-{i}"), WorkflowStatus::Completed);
        run.created_at = i;
        update(&mut app, run);
    }
    for status in [
        WorkflowStatus::Creating,
        WorkflowStatus::Running,
        WorkflowStatus::Waiting,
        WorkflowStatus::Paused,
        WorkflowStatus::Blocked,
    ] {
        update(&mut app, workflow(&format!("{status:?}"), status));
    }
    let mut foreign = workflow("other-project", WorkflowStatus::Running);
    foreign.project = ProjectId("p2".into());
    update(&mut app, foreign);
    let rows = app.workflow_rows();
    assert_eq!(rows.len(), 15);
    assert_eq!(rows.iter().filter(|r| r.status.active()).count(), 3);
    assert_eq!(rows[0].status, WorkflowStatus::Blocked);
    assert_eq!(rows[5].id, "completed-59");
    app.tree.active_workspace = WorkspaceId("other".into());
    assert!(app.workflow_rows().is_empty());
}

#[test]
fn workflow_panel_toggle_walk_click_resize_and_restore() {
    with_default_config(|| {
        let mut app = App::new();
        seed_tree(&mut app);
        update(&mut app, workflow("run", WorkflowStatus::Running));
        let mut out = Vec::new();
        press(&mut app, KeyCode::Char('O'), KeyModifiers::SHIFT, &mut out);
        assert!(app.workflows.show);
        assert!(crate::config::Config::load().show_workflows);
        assert_eq!(app.focus, Focus::Projects);
        assert_eq!(app.next_visible_focus(Focus::Worktrees), Focus::Workflows);
        assert_eq!(
            app.previous_visible_focus(Focus::Sessions),
            Focus::Workflows
        );
        assert_eq!(app.visible_panel_indices(), [0, 1, 3, 2]);
        app.set_splitter(3, 82, 160);
        assert_eq!(app.workflows.width, 40);
        let saved = ui_state_json(&app);
        let mut restored = App::new();
        restore_ui_state(&mut restored, &saved);
        assert_eq!(restored.workflows.width, 40);
        let mut terminal = Terminal::new(TestBackend::new(160, 40)).unwrap();
        terminal.draw(|f| ui::draw(f, &mut app)).unwrap();
        let (rect, _) = app
            .hits
            .iter()
            .find(|(_, hit)| *hit == HitTarget::Workflow(0))
            .unwrap()
            .clone();
        handle_mouse(
            &mut app,
            mev(
                MouseEventKind::Down(MouseButton::Left),
                rect.x + 1,
                rect.y + 1,
            ),
            &mut out,
        );
        assert_eq!(app.focus, Focus::Workflows);
        assert_eq!(app.selected_session().unwrap().id.as_str(), "a1");
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE, &mut out);
        assert_eq!(app.focus, Focus::Sessions);
        app.focus = Focus::Workflows;
        press(&mut app, KeyCode::Char('O'), KeyModifiers::SHIFT, &mut out);
        assert_eq!(app.focus, Focus::Sessions);
        assert!(!app.workflows.show);
        terminal.draw(|f| ui::draw(f, &mut app)).unwrap();
        let (rect, _) = app
            .hits
            .iter()
            .find(|(_, hit)| *hit == HitTarget::FooterWorkflows)
            .unwrap()
            .clone();
        handle_mouse(
            &mut app,
            mev(MouseEventKind::Down(MouseButton::Left), rect.x, rect.y),
            &mut out,
        );
        assert!(app.workflows.show);
        assert_eq!(app.focus, Focus::Workflows);
        for width in [120, 80, 40, 10] {
            let mut terminal = Terminal::new(TestBackend::new(width, 18)).unwrap();
            terminal.draw(|f| ui::draw(f, &mut app)).unwrap();
        }
    });
}

#[test]
fn unavailable_workflow_does_not_select_an_unrelated_worktree() {
    let mut app = App::new();
    seed_tree(&mut app);
    let mut run = workflow("gone", WorkflowStatus::Blocked);
    run.worktree = Some(WorktreeId("deleted".into()));
    update(&mut app, run);
    let mut out = Vec::new();
    super::super::workflows::select(&mut app, 0, &mut out);
    assert_eq!(app.focus, Focus::Workflows);
    assert!(app.flash.as_ref().unwrap().contains("not available"));
    assert!(out.is_empty());
}
