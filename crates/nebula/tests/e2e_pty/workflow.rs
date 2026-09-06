//! CLI, real DAEMON, real WORKTREE and PTYs. STUB AGENTS supply explicit hooks/results.
use super::*;
use nebula_core::{workflow::*, AgentId};

fn workflow_cli(env: &TestEnv, caller: &AgentId, args: &[&str]) -> std::process::Output {
    env.cli()
        .args(args)
        .env(env::AGENT_ID, caller.as_str())
        .output()
        .unwrap()
}

fn workflow_status(env: &TestEnv, id: &str) -> WorkflowRun {
    let output = env
        .cli()
        .args(["workflow", "status", id, "--json"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let WorkflowReply::Run(run) = serde_json::from_slice(&output.stdout).unwrap() else {
        panic!("expected a run");
    };
    *run
}

async fn wait_for_run(
    env: &TestEnv,
    id: &str,
    ready: impl Fn(&WorkflowRun) -> bool,
) -> WorkflowRun {
    let deadline = tokio::time::Instant::now() + SPAWN_CHAIN_TIMEOUT;
    loop {
        let run = workflow_status(env, id);
        if ready(&run) {
            return run;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "workflow did not advance: {}",
            run.message
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

async fn workflow_hook(env_dir: &Path, agent: &AgentId, kind: &str, event: &str) {
    let vars = read_env_file(&env_dir.join(format!("{agent}.env"))).await;
    let port = vars[env::API_URL]
        .rsplit(':')
        .next()
        .unwrap()
        .parse()
        .unwrap();
    let (status, _) = hook_post(
        port,
        &format!("/api/hooks/{kind}?agentId={agent}&hookEvent={event}"),
        &vars[env::API_TOKEN],
    )
    .await;
    assert_eq!(status, 200);
}

#[tokio::test]
async fn workflow_cli_handoffs_are_durable_and_require_an_explicit_result() {
    let env = TestEnv::new();
    let repo = env.make_repo();
    env.write_config(
        r#"{"prewarm_agents":false,"prewarm_sessions":false,"session_idle_timeout":"off"}"#,
    );
    let env_dir = env.tmp.path().join("agent-env");
    std::fs::create_dir_all(&env_dir).unwrap();
    let script = env.tmp.path().join("agent.sh");
    std::fs::write(
        &script,
        format!(
            "#!/bin/sh\nenv | grep '^NEBULA_' > '{d}'/$NEBULA_AGENT_ID.env\nexec sleep 600\n",
            d = env_dir.display()
        ),
    )
    .unwrap();
    make_executable(&script);
    let codex_dir = env.tmp.path().join("codex");
    let spawn = || {
        env.spawn_daemon_with(
            script.to_str().unwrap(),
            &[("CODEX_HOME", codex_dir.to_str().unwrap())],
        )
    };
    let mut daemon = spawn();
    let mut c = connect(&env.sock()).await;
    handshake(&mut c).await;
    let root = add_project_get_main_worktree(&mut c, &repo).await;
    let caller = create_agent_get_id(&mut c, &root.id, "kickoff", 2).await;
    let definition = WorkflowDefinition {
        version: 1,
        timeout_seconds: 60,
        stages: vec![
            StageDefinition {
                id: "planner".into(),
                kind: AgentKind::Claude,
                model: Some("test-planner-model".into()),
                effort: Some("high".into()),
                instructions: "Plan it".into(),
            },
            StageDefinition {
                id: "implementer".into(),
                kind: AgentKind::Codex,
                model: Some("test-implementer-model".into()),
                effort: Some("medium".into()),
                instructions: "Implement it".into(),
            },
        ],
    };
    let definition_file = env.tmp.path().join("workflow.json");
    std::fs::write(
        &definition_file,
        serde_json::to_string(&definition).unwrap(),
    )
    .unwrap();
    let out = workflow_cli(
        &env,
        &caller,
        &[
            "workflow",
            "start",
            "Change README",
            "--definition",
            definition_file.to_str().unwrap(),
            "--json",
        ],
    );
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let WorkflowReply::Run(run) = serde_json::from_slice(&out.stdout).unwrap() else {
        panic!("expected run");
    };
    let id = run.id.clone();
    let first = wait_for_run(&env, &id, |r| r.stages[0].agent.is_some()).await;
    let planner = first.stages[0].agent.clone().unwrap();
    assert_ne!(first.worktree.as_ref().unwrap().id, root.id);
    assert!(first
        .worktree
        .as_ref()
        .unwrap()
        .path
        .join("README.md")
        .is_file());
    assert_eq!(
        first.definition.stages[0].model.as_deref(),
        Some("test-planner-model")
    );

    // A stopped turn without an explicit result cannot start the implementer.
    workflow_hook(&env_dir, &planner, "claude", "UserPromptSubmit").await;
    workflow_hook(&env_dir, &planner, "claude", "Stop").await;
    wait_for_run(&env, &id, |r| r.message.contains("explicit stage result")).await;
    assert!(workflow_status(&env, &id).stages[1].agent.is_none());
    let wrong = workflow_cli(
        &env,
        &caller,
        &[
            "workflow",
            "report",
            "--stage",
            "planner",
            "--outcome",
            "blocked",
            "--summary",
            "wrong caller",
        ],
    );
    assert!(!wrong.status.success());
    let wrong_stage = workflow_cli(
        &env,
        &planner,
        &[
            "workflow",
            "report",
            "--stage",
            "implementer",
            "--outcome",
            "blocked",
            "--summary",
            "wrong stage",
        ],
    );
    assert!(!wrong_stage.status.success());

    let pause = env.cli().args(["workflow", "pause", &id]).output().unwrap();
    assert!(pause.status.success());
    let artifact = env.tmp.path().join("plan.md");
    std::fs::write(&artifact, "# Plan\nChange README and check its contents.\n").unwrap();
    let report = workflow_cli(
        &env,
        &planner,
        &[
            "workflow",
            "report",
            "--stage",
            "planner",
            "--outcome",
            "completed",
            "--summary",
            "Plan ready",
            "--file",
            artifact.to_str().unwrap(),
        ],
    );
    assert!(
        report.status.success(),
        "{}",
        String::from_utf8_lossy(&report.stderr)
    );
    assert_eq!(workflow_status(&env, &id).status, WorkflowStatus::Paused);

    // A clean DAEMON restart preserves the pause and the artifact in SQLite.
    write_frame(&mut c, &ClientRequest::Shutdown).await.unwrap();
    wait_for_exit(&mut daemon);
    daemon = spawn();
    c = connect(&env.sock()).await;
    handshake(&mut c).await;
    let restored = workflow_status(&env, &id);
    assert_eq!(restored.status, WorkflowStatus::Paused);
    assert!(restored.stages[0]
        .result
        .as_ref()
        .unwrap()
        .artifact
        .contains("# Plan"));
    let resumed = env
        .cli()
        .args(["workflow", "resume", &id])
        .output()
        .unwrap();
    assert!(
        resumed.status.success(),
        "{}",
        String::from_utf8_lossy(&resumed.stderr)
    );
    let second = wait_for_run(&env, &id, |r| r.stages[1].agent.is_some()).await;
    let implementer = second.stages[1].agent.clone().unwrap();
    assert_eq!(second.current, 1);
    assert_eq!(second.stages[0].agent.as_ref(), Some(&planner));
    assert_eq!(second.definition.stages[1].kind, AgentKind::Codex);
    workflow_hook(&env_dir, &implementer, "codex", "UserPromptSubmit").await;

    // An interrupted active stage blocks; recovery keeps the same SESSION id.
    write_frame(&mut c, &ClientRequest::Shutdown).await.unwrap();
    wait_for_exit(&mut daemon);
    daemon = spawn();
    c = connect(&env.sock()).await;
    handshake(&mut c).await;
    let blocked = wait_for_run(&env, &id, |r| r.status == WorkflowStatus::Blocked).await;
    assert_eq!(blocked.stages[1].agent.as_ref(), Some(&implementer));
    let resume_dead = env
        .cli()
        .args(["workflow", "resume", &id])
        .output()
        .unwrap();
    assert!(!resume_dead.status.success());
    write_frame(
        &mut c,
        &ClientRequest::Attach {
            session: SessionRef::Agent(implementer.clone()),
            from_seq: None,
            cols: 80,
            rows: 24,
        },
    )
    .await
    .unwrap();
    read_events_until(&mut c, EVENT_TIMEOUT, |events| {
        events
            .iter()
            .any(|e| matches!(e, ServerEvent::Scrollback { .. }))
    })
    .await;
    // The stub rewrites its environment on respawn; wait for the new hook port.
    let deadline = tokio::time::Instant::now() + EVENT_TIMEOUT;
    loop {
        let vars = read_env_file(&env_dir.join(format!("{implementer}.env"))).await;
        let url = &vars[env::API_URL];
        if tokio::net::TcpStream::connect(url.trim_start_matches("http://"))
            .await
            .is_ok()
        {
            break;
        }
        assert!(tokio::time::Instant::now() < deadline);
        tokio::time::sleep(POLL_STEP).await;
    }
    workflow_hook(&env_dir, &implementer, "codex", "UserPromptSubmit").await;
    let resumed = env
        .cli()
        .args(["workflow", "resume", &id])
        .output()
        .unwrap();
    assert!(
        resumed.status.success(),
        "{}",
        String::from_utf8_lossy(&resumed.stderr)
    );
    let report_blocked = workflow_cli(
        &env,
        &implementer,
        &[
            "workflow",
            "report",
            "--stage",
            "implementer",
            "--outcome",
            "blocked",
            "--summary",
            "Need clarification",
        ],
    );
    assert!(report_blocked.status.success());
    wait_for_run(&env, &id, |r| r.status == WorkflowStatus::Blocked).await;
    assert!(!env
        .cli()
        .args(["workflow", "resume", &id])
        .output()
        .unwrap()
        .status
        .success());
    let complete = workflow_cli(
        &env,
        &implementer,
        &[
            "workflow",
            "report",
            "--stage",
            "implementer",
            "--outcome",
            "completed",
            "--summary",
            "Implemented and checked",
            "--file",
            artifact.to_str().unwrap(),
        ],
    );
    assert!(complete.status.success());
    let overwrite = workflow_cli(
        &env,
        &implementer,
        &[
            "workflow",
            "report",
            "--stage",
            "implementer",
            "--outcome",
            "blocked",
            "--summary",
            "overwrite completion",
        ],
    );
    assert!(!overwrite.status.success());
    assert!(env
        .cli()
        .args(["workflow", "resume", &id])
        .output()
        .unwrap()
        .status
        .success());
    wait_for_run(&env, &id, |r| r.message.contains("waiting for SESSION")).await;
    assert_eq!(
        workflow_status(&env, &id).current,
        1,
        "report alone cannot hand off a busy SESSION"
    );
    let worktree = second.worktree.as_ref().unwrap();
    assert!(std::process::Command::new("git")
        .args(["checkout", "-b", "unexpected-branch"])
        .current_dir(&worktree.path)
        .output()
        .unwrap()
        .status
        .success());
    workflow_hook(&env_dir, &implementer, "codex", "Stop").await;
    let wrong_branch = wait_for_run(&env, &id, |r| r.status == WorkflowStatus::Blocked).await;
    assert!(wrong_branch.message.contains("changed branches"));
    assert!(std::process::Command::new("git")
        .args(["checkout", &second.branch])
        .current_dir(&worktree.path)
        .output()
        .unwrap()
        .status
        .success());
    assert!(env
        .cli()
        .args(["workflow", "resume", &id])
        .output()
        .unwrap()
        .status
        .success());
    let completed = wait_for_run(&env, &id, |r| r.status == WorkflowStatus::Completed).await;
    assert_eq!(completed.current, 2);
    assert!(completed
        .stages
        .iter()
        .all(|s| s.status == StageStatus::Completed));

    let snapshots = subscribe(&mut c).await;
    let ServerEvent::Snapshot { agents, .. } = snapshots.last().unwrap() else {
        panic!("snapshot");
    };
    assert_eq!(
        agents.len(),
        3,
        "one caller plus exactly two workflow SESSIONS"
    );
    assert_eq!(
        agents.iter().find(|a| a.id == caller).unwrap().worktree_id,
        root.id
    );
    assert_eq!(
        agents.iter().find(|a| a.id == planner).unwrap().worktree_id,
        agents
            .iter()
            .find(|a| a.id == implementer)
            .unwrap()
            .worktree_id
    );
    write_frame(&mut c, &ClientRequest::Shutdown).await.unwrap();
    wait_for_exit(&mut daemon);
}
