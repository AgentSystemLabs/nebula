use super::*;
use nebula_core::workflow::{WorkflowReply, WorkflowRun};

fn cli(tui: &TuiHarness, args: &[&str], caller: &str) -> std::process::Output {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_nebula"))
        .args(args)
        .env(nebula_core::env::RUNTIME_DIR, &tui.runtime_dir)
        .env(nebula_core::env::DATA_DIR, &tui.data_dir)
        .env(nebula_core::env::AGENT_ID, caller)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    output
}

#[test]
fn workflows_are_visible_live_and_open_their_sessions() {
    use std::os::unix::fs::PermissionsExt;
    let scratch = tempfile::tempdir().unwrap();
    let stub = scratch.path().join("agent");
    std::fs::write(&stub, "#!/bin/sh\nexec sleep 600\n").unwrap();
    std::fs::set_permissions(&stub, std::fs::Permissions::from_mode(0o755)).unwrap();
    let mut tui = TuiHarness::spawn_with_env(&[
        (nebula_core::env::AGENT_CMD, stub.display().to_string()),
        (
            "CODEX_HOME",
            scratch.path().join("codex").display().to_string(),
        ),
    ]);
    let repo = tui.make_repo("demo");
    add_project(&mut tui, &repo, "demo");
    tui.send(ENTER);
    tui.wait_for_text(FOOTER_WORKTREES);
    tui.send(ENTER);
    tui.wait_for_text(FOOTER_SESSIONS);
    tui.send(b"n");
    tui.wait_for_text("New session");
    tui.send(ENTER);
    tui.wait_for_text("New agent");
    tui.type_str("kickoff");
    tui.send(ENTER);
    tui.wait_for_gone("New agent");
    tui.wait_for_text(FOOTER_TERMINAL_LOCKED);
    tui.send(CTRL_Q);
    tui.wait_for_text(FOOTER_SESSIONS);
    let store = nebula_daemon::store::Store::open(&tui.data_dir.join("nebula.db")).unwrap();
    let caller = store
        .load_tree()
        .unwrap()
        .2
        .into_iter()
        .find(|a| a.name == "kickoff")
        .unwrap()
        .id;
    let definition = scratch.path().join("workflow.json");
    std::fs::write(
        &definition,
        r#"{"version":1,"timeout_seconds":600,"stages":[
        {"id":"planner","kind":"claude","model":null,"effort":null,"instructions":"Plan"},
        {"id":"implementer","kind":"claude","model":null,"effort":null,"instructions":"Implement"}
    ]}"#,
    )
    .unwrap();
    let output = cli(
        &tui,
        &[
            "workflow",
            "start",
            "Fix login",
            "--definition",
            definition.to_str().unwrap(),
            "--json",
        ],
        caller.as_str(),
    );
    let WorkflowReply::Run(run) = serde_json::from_slice(&output.stdout).unwrap() else {
        panic!("run");
    };
    let run: WorkflowRun = *run;
    assert_eq!(run.branch, "workflow-fix-login");
    tui.wait_for_text("◆ 1 running");
    tui.wait_for_text("◆ Fix login");
    tui.send(b"O");
    tui.wait_for_text("WORKFLOWS");
    tui.wait_for_text("Step 1/2 · planner");
    tui.wait_for_text("Next: implementer");
    tui.wait_for_text("0/2 done · 2 left");
    // Enter the dedicated panel from SESSIONS, then open the run.
    tui.send(SHIFT_TAB);
    tui.send(ENTER);
    tui.wait_for_text("planner");
    cli(&tui, &["workflow", "pause", &run.id], caller.as_str());
    tui.wait_for_text("Paused");
    tui.wait_for_text("◆ 0 running");
    cli(&tui, &["workflow", "resume", &run.id], caller.as_str());
    tui.wait_for_text("Running");
    tui.wait_for_text("◆ 1 running");
    let text = tui.screen_text();
    assert!(!text.contains(&run.id), "{text}");
    if let Ok(path) = std::env::var("NEBULA_WORKFLOW_SCREENSHOT") {
        std::fs::write(&path, &text).unwrap();
        let parser = tui.parser.lock().unwrap();
        let screen = parser.screen();
        let cells: Vec<_> = (0..ROWS).flat_map(|y| (0..COLS).map(move |x| {
            let c = screen.cell(y, x).unwrap();
            serde_json::json!({"x":x, "y":y, "text":c.contents(), "fg":format!("{:?}",c.fgcolor()), "bg":format!("{:?}",c.bgcolor()), "bold":c.bold()})
        })).collect();
        std::fs::write(format!("{path}.json"), serde_json::to_vec(&cells).unwrap()).unwrap();
    }
    tui.send(b"O");
    tui.wait_for_gone("WORKFLOWS");
    tui.wait_for_text("◆ 1 running");
}
