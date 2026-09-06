//! Configuration inspection is local and must not need a running DAEMON or an AGENT id.
use std::{
    fs,
    path::Path,
    process::{Command, Output},
};

fn command(root: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_nebula"))
        .args(args)
        .current_dir(root)
        .env_remove("NEBULA_AGENT_ID")
        .env("NEBULA_DATA_DIR", root.join("data"))
        .env("NEBULA_RUNTIME_DIR", root.join("runtime"))
        .output()
        .unwrap()
}

fn fixture(root: &Path) {
    fs::create_dir_all(root.join(".nebula/workflows")).unwrap();
    fs::create_dir_all(root.join(".nebula/agents")).unwrap();
    fs::write(root.join(".nebula/agents/reviewer.toml"), "kind = 'claude'\nmodel = 'claude-sonnet-5'\neffort = 'medium'\ninstructions = 'Review existing code.'\n").unwrap();
    fs::write(root.join(".nebula/workflows/review.toml"), "version = 1\nname = 'Read-only review'\ndescription = 'Review existing code without changing it'\ntimeout_seconds = 60\n[[stages]]\nid = 'review'\nagent = 'reviewer'\neffort = 'high'\ninstructions = 'Focus on the user request.'\n").unwrap();
}

#[test]
fn catalog_and_inspect_show_resolved_values_without_starting_a_daemon() {
    let dir = tempfile::tempdir().unwrap();
    fixture(dir.path());
    let out = command(dir.path(), &["workflow", "catalog", "--json"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let entries: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(entries[0]["id"], "review");
    assert_eq!(entries[0]["name"], "Read-only review");
    assert!(entries[0]["error"].is_null());
    let out = command(dir.path(), &["workflow", "inspect", "review", "--json"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let resolved: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(resolved["definition"]["id"], "review");
    let stage = &resolved["definition"]["stages"][0];
    assert_eq!(stage["model"], "claude-sonnet-5");
    assert_eq!(stage["effort"], "high");
    assert_eq!(
        stage["instructions"],
        "Review existing code.\n\nFocus on the user request."
    );
    assert!(!dir.path().join("runtime").exists());
    assert!(!dir.path().join("data/nebula.db").exists());
    let out = command(dir.path(), &["workflow", "inspect", "review"]);
    let text = String::from_utf8(out.stdout).unwrap();
    assert!(
        text.contains("AGENT:")
            && text.contains("reviewer.toml")
            && text.contains("claude-sonnet-5")
    );
}

#[test]
fn malformed_agents_are_visible_in_catalog_and_rejected_before_launch() {
    let dir = tempfile::tempdir().unwrap();
    fixture(dir.path());
    fs::remove_file(dir.path().join(".nebula/agents/reviewer.toml")).unwrap();
    let out = command(dir.path(), &["workflow", "catalog", "--json"]);
    assert!(out.status.success());
    let entries: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert!(entries[0]["error"].as_str().unwrap().contains("reviewer"));
    assert!(!command(dir.path(), &["workflow", "inspect", "review"])
        .status
        .success());
    let out = Command::new(env!("CARGO_BIN_EXE_nebula"))
        .args(["workflow", "start", "Review it", "--workflow", "review"])
        .current_dir(dir.path())
        .env("NEBULA_AGENT_ID", "no-real-agent")
        .env("NEBULA_DATA_DIR", dir.path().join("data"))
        .env("NEBULA_RUNTIME_DIR", dir.path().join("runtime"))
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("reviewer"));
    assert!(!dir.path().join("runtime").exists());
}
