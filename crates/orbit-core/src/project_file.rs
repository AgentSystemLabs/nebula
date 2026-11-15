//! The PROJECT FILE: `.orbit/config.json`, committed at the root of a
//! repository, telling orbit how the project is run, how to open what it
//! serves, and which SKILL WORKFLOWS it keeps.
//!
//! ```json
//! {
//!   "run": "npm run dev",
//!   "open": "open http://localhost:3000",
//!   "workflows": {
//!     "feature": { "skills": ["spec-driven-development"], "text": "Ship it small." }
//!   }
//! }
//! ```
//!
//! THE `.orbit/` FOLDER holds everything orbit keeps inside a project:
//!
//! ```text
//! .orbit/
//! ├── config.json   committed — this file, shared with everyone on the project
//! └── local/        git-excluded — what orbit generates for one checkout
//!     └── graph/    the graphify code graph and its usage log
//! ```
//!
//! The top level is shared; [`LOCAL_DIR`] is never committed (the DAEMON
//! keeps it out of git through the repository's own `info/exclude`).
//!
//! `.orbit.json` at the root is the file's older home and is still read:
//! a checkout holding only that one is answered from it, and one holding
//! both is answered from `.orbit/config.json` alone.
//!
//! Both values are shell command lines, and both run in the selected
//! worktree's checkout. `run` is the RUN COMMAND: a menu's **Run** starts
//! it in a RUN TERMINAL the DAEMON holds, and **Stop run** stops it. `open`
//! is the OPEN COMMAND: `Shift+Enter` on a worktree fires it once, from the
//! TUI.
//!
//! ONE FILE PER PROJECT: the file is always read from the project's main
//! checkout, fresh at every press, whichever worktree asked — so what a
//! project is configured to do never depends on which branch a worktree
//! has checked out, and an edit applies everywhere at once. A copy of the
//! file inside a linked worktree is ignored. [`main_checkout`] finds the
//! main checkout for a caller that only has a worktree's path.
//!
//! `workflows` is the one key orbit writes as well as reads: the SKILL
//! PICKER's `Ctrl+W` saves a named skill set into the main checkout's
//! file ([`write_workflow`]), leaving every other key as it found it.
//!
//! Unlike WORKTREE HOOKS, which orbit runs on its own after a create or a
//! delete and so never takes from a checkout, nothing here runs unless the
//! user presses the key on that worktree — the same trust as typing the
//! command into a shell there.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// The file's path, relative to the root of a checkout.
pub const FILE_NAME: &str = ".orbit/config.json";

/// Where the file used to live, still read when [`FILE_NAME`] is absent.
pub const LEGACY_FILE_NAME: &str = ".orbit.json";

/// What orbit generates for one checkout, relative to its root: never
/// committed, rebuilt at will.
pub const LOCAL_DIR: &str = ".orbit/local";

/// Where graphify writes a checkout's code graph, relative to its root —
/// handed to graphify as `GRAPHIFY_OUT`.
pub const GRAPH_DIR: &str = ".orbit/local/graph";

pub use crate::git::main_checkout;

/// One SKILL WORKFLOW: a named set of Claude skills to use for a kind of
/// task, plus optional extra instructions sent with them.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize)]
pub struct Workflow {
    #[serde(default)]
    pub skills: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
}

/// What the project file says. Unknown keys are ignored, so a file written
/// for a newer orbit still works in this one.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct ProjectFile {
    /// The RUN COMMAND, started and stopped with `r`.
    #[serde(default)]
    pub run: Option<String>,
    /// The OPEN COMMAND, fired with `Shift+Enter`.
    #[serde(default)]
    pub open: Option<String>,
    /// `false` keeps orbit from building a graphify code graph for this
    /// project (the `graphify` setting turns it off everywhere).
    #[serde(default)]
    pub graphify: Option<bool>,
    /// The SKILL WORKFLOWS, by name.
    #[serde(default)]
    pub workflows: BTreeMap<String, Workflow>,
}

/// Which of the file's commands a caller wants.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProjectCommand {
    Run,
    Open,
}

impl ProjectCommand {
    /// The command's key in the file.
    pub fn key(self) -> &'static str {
        match self {
            Self::Run => "run",
            Self::Open => "open",
        }
    }

    /// An example value, for the message that asks for one.
    fn example(self) -> &'static str {
        match self {
            Self::Run => "npm run dev",
            Self::Open => "open http://localhost:3000",
        }
    }
}

impl ProjectFile {
    /// The command line for `which`, trimmed; None when the key is absent
    /// or blank.
    pub fn command(&self, which: ProjectCommand) -> Option<&str> {
        let value = match which {
            ProjectCommand::Run => &self.run,
            ProjectCommand::Open => &self.open,
        };
        value.as_deref().map(str::trim).filter(|c| !c.is_empty())
    }
}

/// Parse one checkout's file — `.orbit/config.json`, else the legacy
/// `.orbit.json`. Ok(None) when the checkout has neither.
pub fn read(dir: &Path) -> Result<Option<ProjectFile>, String> {
    for name in [FILE_NAME, LEGACY_FILE_NAME] {
        let path = dir.join(name);
        match std::fs::read_to_string(&path) {
            Ok(text) => {
                return serde_json::from_str(&text)
                    .map(Some)
                    .map_err(|e| format!("{}: {e}", path.display()))
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => return Err(format!("{}: {e}", path.display())),
        }
    }
    Ok(None)
}

/// One file read as a JSON object: Ok(None) when it is not there, Err for
/// one that is there and is not an object (or not JSON at all).
fn read_object(path: &Path) -> Result<Option<serde_json::Value>, String> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("{}: {e}", path.display())),
    };
    let value: serde_json::Value =
        serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?;
    if value.is_object() {
        Ok(Some(value))
    } else {
        Err(format!("{}: not a JSON object", path.display()))
    }
}

/// Save `workflow` as `name` in `main`'s `.orbit/config.json` — the
/// project's main checkout, so every worktree falls back to it — and
/// return the path written. Only the `workflows` key changes: the file is
/// read as plain JSON, so every other key, known to this orbit or not, is
/// written back as it was. A checkout with only the legacy `.orbit.json`
/// has that file's keys carried into the new one, since the new file is
/// the whole answer once it exists and must not lose the `run` it would
/// otherwise shadow. A file that is not a JSON object is refused rather
/// than overwritten.
pub fn write_workflow(main: &Path, name: &str, workflow: &Workflow) -> Result<PathBuf, String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("a workflow needs a name".into());
    }
    let path = main.join(FILE_NAME);
    let mut root = match read_object(&path)? {
        Some(value) => value,
        None => read_object(&main.join(LEGACY_FILE_NAME))?
            .unwrap_or_else(|| serde_json::Value::Object(Default::default())),
    };
    let Some(object) = root.as_object_mut() else {
        return Err(format!("{}: not a JSON object", path.display()));
    };
    let workflows = object
        .entry("workflows")
        .or_insert_with(|| serde_json::Value::Object(Default::default()));
    if !workflows.is_object() {
        *workflows = serde_json::Value::Object(Default::default());
    }
    let value = serde_json::to_value(workflow).map_err(|e| e.to_string())?;
    if let Some(workflows) = workflows.as_object_mut() {
        workflows.insert(name.to_string(), value);
    }
    let mut bytes = serde_json::to_vec_pretty(&root).map_err(|e| e.to_string())?;
    bytes.push(b'\n');
    crate::settings::write_atomic(&path, &bytes).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(path)
}

/// The project's file, read from its main checkout — the one place the
/// project is configured, whichever worktree asks.
pub fn find(main: &Path) -> Result<Option<ProjectFile>, String> {
    read(main)
}

/// The command line `which` names for the project whose main checkout is
/// `main`: Ok(None) when there is no file, or the file leaves the key out
/// or blank, so a caller with a fallback of its own can take it; Err only
/// for a file that is there and can't be read. The DAEMON's run path looks
/// here after the project's own `run_command` setting.
pub fn lookup(main: &Path, which: ProjectCommand) -> Result<Option<String>, String> {
    Ok(find(main)?.and_then(|file| file.command(which).map(str::to_string)))
}

/// The command line `which` names for the project, or the one-line reason
/// there is none — short enough for the footer flash, and saying what to
/// add.
pub fn command(main: &Path, which: ProjectCommand) -> Result<String, String> {
    let key = which.key();
    let example = which.example();
    match find(main)? {
        Some(file) => file.command(which).map(str::to_string).ok_or_else(|| {
            format!("{FILE_NAME} has no \"{key}\" command — add \"{key}\": \"{example}\"")
        }),
        None => Err(format!(
            "no {FILE_NAME} in this project's main checkout — add one with {{\"{key}\": \"{example}\"}}"
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fresh directory under the system temp dir, removed on drop.
    struct TempDir(PathBuf);

    impl TempDir {
        fn new() -> Self {
            let dir =
                std::env::temp_dir().join(format!("orbit-project-file-{}", ulid::Ulid::generate()));
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }

        fn with_file(self, json: &str) -> Self {
            std::fs::create_dir_all(self.0.join(".orbit")).unwrap();
            std::fs::write(self.0.join(FILE_NAME), json).unwrap();
            self
        }

        fn with_legacy_file(self, json: &str) -> Self {
            std::fs::write(self.0.join(LEGACY_FILE_NAME), json).unwrap();
            self
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn reads_both_commands_and_ignores_unknown_keys() {
        let dir = TempDir::new().with_file(
            r#"{"run": "npm run dev", "open": " open http://localhost:3000 ", "later": 1}"#,
        );
        let run = command(&dir.0, ProjectCommand::Run).unwrap();
        let open = command(&dir.0, ProjectCommand::Open).unwrap();
        assert_eq!(run, "npm run dev");
        assert_eq!(open, "open http://localhost:3000", "trimmed");
    }

    /// `lookup` is `command` without the message: nothing to run is
    /// Ok(None), for the caller that has somewhere else to look, and only
    /// a file that is there but unreadable is an error.
    #[test]
    fn lookup_reads_none_for_a_missing_file_or_key_and_keeps_the_parse_error() {
        let empty = TempDir::new();
        assert_eq!(lookup(&empty.0, ProjectCommand::Run), Ok(None));
        let blank = TempDir::new().with_file(r#"{"run": "   ", "open": "open x"}"#);
        assert_eq!(lookup(&blank.0, ProjectCommand::Run), Ok(None));
        assert_eq!(
            lookup(&blank.0, ProjectCommand::Open),
            Ok(Some("open x".into()))
        );
        let broken = TempDir::new().with_file("{");
        let err = lookup(&broken.0, ProjectCommand::Run).unwrap_err();
        assert!(err.contains(FILE_NAME), "{err}");
    }

    #[test]
    fn missing_file_blank_command_and_bad_json_say_what_to_fix() {
        let empty = TempDir::new();
        let err = command(&empty.0, ProjectCommand::Run).unwrap_err();
        assert!(err.contains("no .orbit/config.json"), "{err}");
        assert!(err.contains(r#"{"run": "npm run dev"}"#), "{err}");

        let blank = TempDir::new().with_file(r#"{"run": "   "}"#);
        let err = command(&blank.0, ProjectCommand::Run).unwrap_err();
        assert!(err.contains("has no \"run\" command"), "{err}");

        let broken = TempDir::new().with_file(r#"{"run": ["npm", "dev"]}"#);
        let err = command(&broken.0, ProjectCommand::Run).unwrap_err();
        assert!(err.contains(FILE_NAME), "names the file: {err}");
        assert!(err.contains("invalid type"), "{err}");
    }

    #[test]
    fn the_legacy_file_is_still_read_and_the_new_one_wins_over_it() {
        let legacy = TempDir::new().with_legacy_file(r#"{"run": "old"}"#);
        assert_eq!(command(&legacy.0, ProjectCommand::Run).unwrap(), "old");
        let both = TempDir::new()
            .with_legacy_file(r#"{"run": "old", "open": "open old"}"#)
            .with_file(r#"{"run": "new"}"#);
        assert_eq!(command(&both.0, ProjectCommand::Run).unwrap(), "new");
        // The new file is the whole answer: no borrowing from the old.
        assert!(command(&both.0, ProjectCommand::Open).is_err());
    }

    #[test]
    fn workflows_parse_with_optional_text() {
        let dir = TempDir::new().with_file(
            r#"{"workflows": {"feature": {"skills": ["a", "b"], "text": "go"}, "bare": {}}}"#,
        );
        let file = read(&dir.0).unwrap().unwrap();
        assert_eq!(
            file.workflows.get("feature"),
            Some(&Workflow {
                skills: vec!["a".into(), "b".into()],
                text: Some("go".into()),
            })
        );
        assert_eq!(file.workflows.get("bare"), Some(&Workflow::default()));
    }

    #[test]
    fn saving_a_workflow_keeps_every_other_key() {
        let dir = TempDir::new().with_file(
            r#"{"run": "npm run dev", "later": {"x": 1}, "workflows": {"old": {"skills": ["z"]}}}"#,
        );
        let wf = Workflow {
            skills: vec!["tdd".into()],
            text: None,
        };
        let path = write_workflow(&dir.0, " feature ", &wf).unwrap();
        assert_eq!(path, dir.0.join(FILE_NAME));
        let value: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(value["run"], "npm run dev");
        assert_eq!(value["later"]["x"], 1);
        assert_eq!(value["workflows"]["old"]["skills"][0], "z");
        assert_eq!(
            value["workflows"]["feature"],
            serde_json::json!({"skills": ["tdd"]})
        );
        assert!(!dir.0.join(".orbit/config.json.tmp").exists());
    }

    #[test]
    fn saving_a_workflow_creates_the_folder_and_carries_the_legacy_keys() {
        let dir = TempDir::new().with_legacy_file(r#"{"run": "bun dev"}"#);
        let wf = Workflow {
            skills: vec!["a".into()],
            text: Some("t".into()),
        };
        write_workflow(&dir.0, "w", &wf).unwrap();
        let file = read(&dir.0).unwrap().unwrap();
        assert_eq!(
            file.run.as_deref(),
            Some("bun dev"),
            "run survives the move"
        );
        assert_eq!(file.workflows.get("w"), Some(&wf));

        let empty = TempDir::new();
        write_workflow(&empty.0, "w", &wf).unwrap();
        assert_eq!(read(&empty.0).unwrap().unwrap().workflows.len(), 1);
    }

    #[test]
    fn saving_a_workflow_refuses_a_file_it_cannot_keep() {
        let dir = TempDir::new().with_file("[1, 2]");
        let err = write_workflow(&dir.0, "w", &Workflow::default()).unwrap_err();
        assert!(err.contains("not a JSON object"), "{err}");
        assert_eq!(
            std::fs::read_to_string(dir.0.join(FILE_NAME)).unwrap(),
            "[1, 2]"
        );
        assert!(write_workflow(&dir.0, "  ", &Workflow::default()).is_err());
    }
}
