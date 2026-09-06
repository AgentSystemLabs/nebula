use super::*;
use crate::{hooks::HookEnv, store::Store};
use nebula_core::AgentKind;

fn run(id: &str) -> WorkflowRun {
    WorkflowRun {
        id: id.into(),
        project: "project".to_string().into(),
        worktree: None,
        branch: format!("workflow-{id}"),
        base: "base".into(),
        task: "Make a plan".into(),
        definition: WorkflowDefinition {
            version: 1,
            timeout_seconds: 60,
            stages: vec![StageDefinition {
                id: "planner".into(),
                kind: AgentKind::Claude,
                model: None,
                effort: None,
                instructions: "Plan the task".into(),
            }],
        },
        stages: vec![StageRun {
            status: StageStatus::Running,
            agent: Some(AgentId("agent".into())),
            started_at: now_ms(),
            result: Some(StageResult {
                outcome: StageOutcome::Completed,
                summary: "Plan ready".into(),
                artifact: "# The plan\n\nVerify the behavior.".into(),
            }),
        }],
        current: 0,
        status: WorkflowStatus::Running,
        message: "waiting for FINISHED".into(),
        created_at: now_ms(),
        updated_at: now_ms(),
    }
}

#[test]
fn workflow_and_artifacts_survive_reopening_the_database() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("nebula.db");
    let expected = run("run-1");
    {
        let store = Store::open(&path).unwrap();
        store.save_workflow(&expected).unwrap();
    }
    let store = Store::open(&path).unwrap();
    let restored = store
        .workflow_for_agent(&AgentId("agent".into()))
        .unwrap()
        .unwrap();
    assert_eq!(
        serde_json::to_string(&restored).unwrap(),
        serde_json::to_string(&expected).unwrap()
    );
    assert_eq!(store.active_workflow_ids().unwrap(), ["run-1"]);
    assert_eq!(store.workflow_summaries().unwrap()[0].total, 1);
}

#[test]
fn conflicting_session_association_rolls_back_the_whole_snapshot() {
    let store = Store::open_in_memory().unwrap();
    store.save_workflow(&run("first")).unwrap();
    assert!(store.save_workflow(&run("second")).is_err());
    assert!(store.workflow("second").is_err());
    assert_eq!(
        store
            .workflow_for_agent(&AgentId("agent".into()))
            .unwrap()
            .unwrap()
            .id,
        "first"
    );
}

#[test]
fn invalid_workflow_inputs_and_empty_success_are_refused() {
    let valid = run("first");
    assert!(prompts::validate(&valid.task, &valid.definition).is_ok());
    let mut invalid = valid.definition.clone();
    invalid.stages.push(invalid.stages[0].clone());
    assert!(prompts::validate(&valid.task, &invalid).is_err());
    invalid = valid.definition.clone();
    invalid.stages[0].id = "../planner".into();
    assert!(prompts::validate(&valid.task, &invalid).is_err());
    assert!(prompts::validate("\0", &valid.definition).is_err());
    assert!(prompts::validate_result(&StageResult {
        outcome: StageOutcome::Completed,
        summary: "done".into(),
        artifact: " ".into()
    })
    .is_err());
}

#[tokio::test]
async fn interrupted_worktree_creation_becomes_blocked_without_retrying() {
    let store = Arc::new(Store::open_in_memory().unwrap());
    let mut interrupted = run("interrupted");
    interrupted.status = WorkflowStatus::Creating;
    store.save_workflow(&interrupted).unwrap();
    let daemon = Daemon::new(
        store,
        HookEnv {
            port: 0,
            token: String::new(),
        },
    );
    daemon.tick_workflows().await.unwrap();
    let blocked = daemon.store.workflow("interrupted").unwrap();
    assert_eq!(blocked.status, WorkflowStatus::Blocked);
    assert!(blocked.message.contains("creation was interrupted"));
    assert!(daemon.store.active_workflow_ids().unwrap().is_empty());
    daemon.tick_workflows().await.unwrap();
    assert!(daemon.store.load_tree().unwrap().1.is_empty());
}
