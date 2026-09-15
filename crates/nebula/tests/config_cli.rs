//! `nebula config` against throwaway data dirs, and the receiving half of the
//! settings forward `nebula ssh` does: the bundle a remote nebula finds in
//! `NEBULA_IMPORT_BUNDLE` when it starts.

use nebula_core::env::{CONFIG_FILE, DATA_DIR, IMPORT_BUNDLE};
use serde_json::{json, Value};
use std::io::Write;
use std::path::Path;
use std::process::{Command, Output, Stdio};

fn command(data: &Path, args: &[&str]) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_nebula"));
    cmd.args(args)
        .env(DATA_DIR, data)
        .env_remove(CONFIG_FILE)
        .env_remove(IMPORT_BUNDLE);
    cmd
}

fn nebula(data: &Path, args: &[&str]) -> Output {
    command(data, args).output().expect("failed to run nebula")
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

fn put(path: &Path, value: Value) {
    std::fs::write(path, value.to_string()).unwrap();
}

fn get(path: &Path) -> Value {
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

#[test]
fn path_names_every_settings_file() {
    let data = tempfile::tempdir().unwrap();
    let out = nebula(data.path(), &["config", "path"]);
    assert!(out.status.success(), "{}", text(&out.stderr));
    let stdout = text(&out.stdout);
    for file in [
        "config.json",
        "config.local.json",
        "agent_presets.json",
        "ssh_hosts.json",
    ] {
        let path = data.path().join(file).display().to_string();
        assert!(stdout.contains(&path), "{file} missing from:\n{stdout}");
    }
}

#[test]
fn a_backup_restores_into_another_machines_settings() {
    let (old, new, backups) = (
        tempfile::tempdir().unwrap(),
        tempfile::tempdir().unwrap(),
        tempfile::tempdir().unwrap(),
    );
    put(
        &old.path().join("config.json"),
        json!({"theme": "ocean", "a_newer_key": 1}),
    );
    put(
        &old.path().join("config.local.json"),
        json!({"editor": "nano"}),
    );
    put(
        &old.path().join("agent_presets.json"),
        json!([{"name": "reviewer", "kind": "codex"}]),
    );

    let folder = backups.path().display().to_string();
    let out = nebula(old.path(), &["config", "export", &folder]);
    assert!(out.status.success(), "{}", text(&out.stderr));
    let bundle = get(&backups.path().join("nebula-settings.json"));
    assert_eq!(bundle["config"]["theme"], "ocean");
    assert!(
        bundle["config"].get("editor").is_none(),
        "config.local.json stays behind"
    );

    put(
        &new.path().join("config.json"),
        json!({"animations": false, "theme": "default"}),
    );
    let out = nebula(new.path(), &["config", "import", &folder]);
    assert!(out.status.success(), "{}", text(&out.stderr));
    let stdout = text(&out.stdout);
    assert!(stdout.contains("config: 2 keys changed"), "{stdout}");
    assert_eq!(
        get(&new.path().join("config.json")),
        json!({"animations": false, "theme": "ocean", "a_newer_key": 1})
    );
    assert_eq!(
        get(&new.path().join("agent_presets.json"))[0]["name"],
        "reviewer"
    );
    assert!(!new.path().join("config.local.json").exists());
}

#[test]
fn an_export_on_stdout_imports_from_stdin() {
    let (from, to) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    put(
        &from.path().join("config.json"),
        json!({"focus_tint": false}),
    );
    let out = nebula(from.path(), &["config", "export"]);
    assert!(out.status.success(), "{}", text(&out.stderr));
    let bundle: Value = serde_json::from_slice(&out.stdout).expect("stdout is the bundle");
    assert_eq!(bundle["nebula_bundle"], 1);

    let mut child = command(to.path(), &["config", "import", "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(&out.stdout).unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(out.status.success(), "{}", text(&out.stderr));
    assert_eq!(
        get(&to.path().join("config.json")),
        json!({"focus_tint": false})
    );
}

/// What a remote nebula does with the bundle `nebula ssh` exported into its
/// environment: merged before the command runs, whatever the command is.
#[test]
fn a_bundle_in_the_environment_is_merged_at_startup() {
    let data = tempfile::tempdir().unwrap();
    put(
        &data.path().join("config.json"),
        json!({"theme": "default", "editor": "vim"}),
    );
    let bundle = nebula_tui::bundle::encode(&json!({
        "nebula_bundle": 1,
        "config": {"theme": "forest"},
    }));
    let out = command(data.path(), &["config", "path"])
        .env(IMPORT_BUNDLE, &bundle)
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", text(&out.stderr));
    assert!(
        text(&out.stderr).contains("config: 1 key changed"),
        "{}",
        text(&out.stderr)
    );
    assert_eq!(
        get(&data.path().join("config.json")),
        json!({"theme": "forest", "editor": "vim"})
    );
}

#[test]
fn a_bad_bundle_in_the_environment_never_fails_the_command() {
    let data = tempfile::tempdir().unwrap();
    let out = command(data.path(), &["config", "path"])
        .env(IMPORT_BUNDLE, "%%% not base64")
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", text(&out.stderr));
    assert!(
        text(&out.stderr).contains("ignored the settings"),
        "{}",
        text(&out.stderr)
    );
    assert!(!data.path().join("config.json").exists());
}
