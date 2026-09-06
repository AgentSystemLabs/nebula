use super::*;

fn write(root: &Path, path: &str, text: &str) {
    let path = root.join(path);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, text).unwrap();
}

const AGENT: &str = "kind = 'claude'\nmodel = 'claude-sonnet-5'\neffort = 'medium'\ninstructions = 'Investigate the task.'\n";
const WORKFLOW: &str = "version = 1\nname = 'Display label'\ndescription = 'Use for implementation'\ntimeout_seconds = 60\n[[stages]]\nid = 'plan'\nagent = 'planner'\ninstructions = 'Check edge cases.'\n";

#[test]
fn references_inline_agents_and_overrides_resolve_into_one_definition() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), ".nebula/agents/planner.toml", AGENT);
    let text = format!("{WORKFLOW}effort = 'high'\n[[stages]]\nid = 'implement'\nagent = {{ kind = 'codex', model = 'test-model', instructions = 'Implement the plan.' }}\n");
    write(dir.path(), ".nebula/workflows/build.toml", &text);
    let config = Config {
        codex_effort: "medium".into(),
        ..Config::default()
    };
    let resolved = load(dir.path(), Some("build"), None, &config).unwrap();
    assert_eq!(resolved.definition.id.as_deref(), Some("build"));
    assert_eq!(resolved.definition.name.as_deref(), Some("Display label"));
    let plan = &resolved.definition.stages[0];
    assert_eq!(plan.model.as_deref(), Some("claude-sonnet-5"));
    assert_eq!(plan.effort.as_deref(), Some("high"));
    assert_eq!(
        plan.instructions,
        "Investigate the task.\n\nCheck edge cases."
    );
    assert!(resolved.agent_sources[0]
        .as_ref()
        .unwrap()
        .ends_with("planner.toml"));
    assert!(resolved.agent_sources[1].is_none());
    assert_eq!(
        resolved.definition.stages[1].effort.as_deref(),
        Some("medium")
    );
    // Changing the source does not change an already resolved snapshot.
    write(
        dir.path(),
        ".nebula/agents/planner.toml",
        &AGENT.replace("claude-sonnet-5", "different-model"),
    );
    assert_eq!(plan.model.as_deref(), Some("claude-sonnet-5"));
    assert_eq!(
        load(dir.path(), Some("build"), None, &config)
            .unwrap()
            .definition
            .stages[0]
            .model
            .as_deref(),
        Some("different-model")
    );
}

#[test]
fn selection_is_explicit_default_then_sole_and_never_a_display_name() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), ".nebula/agents/planner.toml", AGENT);
    write(dir.path(), ".nebula/workflows/one.toml", WORKFLOW);
    let config = Config::default();
    assert_eq!(
        load(dir.path(), None, None, &config)
            .unwrap()
            .definition
            .id
            .as_deref(),
        Some("one")
    );
    write(dir.path(), ".nebula/workflows/two.toml", WORKFLOW);
    assert!(load(dir.path(), None, None, &config)
        .unwrap_err()
        .to_string()
        .contains("choose --workflow"));
    assert!(load(dir.path(), Some("Display label"), None, &config).is_err());
    // Duplicate display labels are allowed because filenames are the stable selectors.
    assert_eq!(catalog(dir.path(), &config).unwrap().len(), 2);
    write(
        dir.path(),
        ".nebula/workflows/default.toml",
        &WORKFLOW.replace("name = 'Display label'\n", ""),
    );
    let default = load(dir.path(), None, None, &config).unwrap();
    assert_eq!(default.definition.id.as_deref(), Some("default"));
    assert_eq!(default.definition.name.as_deref(), Some("default"));
    assert_eq!(
        load(dir.path(), Some("two"), None, &config)
            .unwrap()
            .definition
            .id
            .as_deref(),
        Some("two")
    );
}

#[test]
fn references_are_relative_to_the_definition_and_lookup_stops_at_checkout_root() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), ".nebula/agents/planner.toml", AGENT);
    write(dir.path(), ".nebula/workflows/build.toml", WORKFLOW);
    let nested = dir.path().join("project/src");
    fs::create_dir_all(&nested).unwrap();
    assert!(load(&nested, Some("build"), None, &Config::default()).is_ok());
    write(dir.path(), "project/.git", "gitdir: elsewhere");
    assert!(load(&nested, Some("build"), None, &Config::default()).is_err());
    let explicit = dir.path().join(".nebula/workflows/build.toml");
    assert!(load(&nested, None, Some(&explicit), &Config::default()).is_ok());
}

#[test]
fn bad_references_unknown_fields_duplicates_and_oversized_instructions_fail() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), ".nebula/agents/planner.toml", AGENT);
    let cases = [
        WORKFLOW.replace("agent = 'planner'", "agent = 'missing'"),
        WORKFLOW.replace("agent = 'planner'", "agent = '../planner'"),
        WORKFLOW.replace("name =", "unexpected ="),
        WORKFLOW.replace("instructions =", "instruction ="),
        WORKFLOW.replace("version = 1", "version = 2"),
        WORKFLOW.replace("timeout_seconds = 60", "timeout_seconds = 0"),
        WORKFLOW.replace("agent = 'planner'", "agent = { kind = 'cursor' }"),
        WORKFLOW.replace(
            "description = 'Use for implementation'",
            "description = \"bad\\u001Bcontrol\"",
        ),
        format!("{WORKFLOW}[[stages]]\nid = 'plan'\nagent = 'planner'\n"),
        WORKFLOW.replace("Check edge cases.", &"x".repeat(4000)),
    ];
    for invalid in cases {
        write(dir.path(), ".nebula/workflows/bad.toml", &invalid);
        assert!(
            load(dir.path(), Some("bad"), None, &Config::default()).is_err(),
            "accepted {invalid}"
        );
    }
    write(dir.path(), ".nebula/workflows/good.toml", WORKFLOW);
    let entries = catalog(dir.path(), &Config::default()).unwrap();
    assert!(entries[0].error.is_some());
    assert!(entries[1].error.is_none());
    write(
        dir.path(),
        ".nebula/agents/planner.toml",
        &AGENT.replace("model =", "models ="),
    );
    assert!(load(dir.path(), Some("good"), None, &Config::default()).is_err());
    assert!(load(dir.path(), Some("../good"), None, &Config::default()).is_err());
}

#[test]
fn legacy_json_loads_without_metadata_and_does_not_shadow_named_workflows() {
    let dir = tempfile::tempdir().unwrap();
    let json = r#"{"version":1,"timeout_seconds":60,"stages":[{"id":"plan","kind":"claude","model":"claude-sonnet-5","effort":"medium","instructions":"Plan it"}]}"#;
    write(dir.path(), ".nebula/workflow.json", json);
    let config = Config::default();
    let legacy = load(dir.path(), None, None, &config).unwrap();
    assert!(legacy.definition.id.is_none());
    assert!(legacy.definition.name.is_none());
    assert_eq!(catalog(dir.path(), &config).unwrap()[0].id, "legacy");
    write(dir.path(), ".nebula/agents/planner.toml", AGENT);
    write(dir.path(), ".nebula/workflows/one.toml", WORKFLOW);
    write(dir.path(), ".nebula/workflows/two.toml", WORKFLOW);
    assert!(load(dir.path(), None, None, &config).is_err());
    assert!(load(
        dir.path(),
        None,
        Some(Path::new(".nebula/workflow.json")),
        &config
    )
    .is_ok());
}

#[cfg(unix)]
#[test]
fn named_workflows_and_agents_cannot_follow_symlinks_outside_their_directories() {
    use std::os::unix::fs::symlink;
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "outside.toml", AGENT);
    write(dir.path(), ".nebula/workflows/build.toml", WORKFLOW);
    fs::create_dir_all(dir.path().join(".nebula/agents")).unwrap();
    symlink(
        dir.path().join("outside.toml"),
        dir.path().join(".nebula/agents/planner.toml"),
    )
    .unwrap();
    assert!(load(dir.path(), Some("build"), None, &Config::default()).is_err());
    symlink(
        dir.path().join("outside.toml"),
        dir.path().join(".nebula/workflows/escape.toml"),
    )
    .unwrap();
    assert!(load(dir.path(), Some("escape"), None, &Config::default()).is_err());
}
